// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Recording a frame: shadow cascades, the water reflection, the depth and
//! normal prepass, ambient occlusion, the lit pass with the sky, water,
//! accumulation, bloom, tone mapping and encoding, then reading it back.

use super::*;
use super::readback::Latest;

fn clear(c: [f32; 4]) -> wgpu::LoadOp<wgpu::Color> {
    wgpu::LoadOp::Clear(wgpu::Color { r: c[0] as f64, g: c[1] as f64, b: c[2] as f64, a: c[3] as f64 })
}

fn colour_attachment(view: &wgpu::TextureView, load: wgpu::LoadOp<wgpu::Color>) -> Option<wgpu::RenderPassColorAttachment<'_>> {
    Some(wgpu::RenderPassColorAttachment { view, depth_slice: None, resolve_target: None, ops: wgpu::Operations { load, store: wgpu::StoreOp::Store } })
}

impl Renderer {
    /// A finished 8-bit sRGB picture of the world.
    pub fn render_srgb(&mut self, w: &World, v: &View, o: &Options) -> Result<(Image, Stats), String> {
        let (bytes, stats, _) = self.run(w, v, o, false, None)?;
        Ok((Image { width: o.width.max(16), height: o.height.max(16), rgba: bytes }, stats))
    }

    /// A finished 8-bit sRGB picture without stalling on the GPU: the frame
    /// recorded now is copied into a ring of staging buffers and `out` gets
    /// the newest one that has already finished, normally the frame before
    /// (`Latest::lag` is 1). Only the first frame after a size change waits,
    /// and then `lag` is 0; if no frame has finished `out` is left as it was.
    /// For previews: exact frames (export, stills) use `render_srgb`.
    pub fn render_srgb_pipelined(&mut self, w: &World, v: &View, o: &Options, out: &mut Vec<u8>) -> Result<(Latest, Stats), String> {
        let (_, stats, latest) = self.run(w, v, o, false, Some(out))?;
        Ok((latest.expect("ring read"), stats))
    }

    /// Readback counters, for tests and benchmarks.
    #[doc(hidden)]
    pub fn readback_stats(&self) -> readback::ReadbackStats {
        self.rb.stats()
    }

    /// The picture as display-linear light, for compositing in a project.
    pub fn render_linear(&mut self, w: &World, v: &View, o: &Options) -> Result<(Frame, Stats), String> {
        let (bytes, stats, _) = self.run(w, v, o, true, None)?;
        let (width, height) = (o.width.max(16), o.height.max(16));
        let pixels = bytes.chunks_exact(8).map(|c| [f16_to_f32(u16::from_le_bytes([c[0], c[1]])), f16_to_f32(u16::from_le_bytes([c[2], c[3]])), f16_to_f32(u16::from_le_bytes([c[4], c[5]])), 1.0]).collect();
        Ok((Frame { width, height, pixels }, stats))
    }

    fn write_post(&self, buf: &wgpu::Buffer, a: [f32; 4], b: [f32; 4]) {
        let v: [f32; 8] = [a[0], a[1], a[2], a[3], b[0], b[1], b[2], b[3]];
        self.ctx.queue.write_buffer(buf, 0, bytemuck::cast_slice(&v));
    }

    fn run(&mut self, w: &World, view: &View, o: &Options, linear: bool, ring: Option<&mut Vec<u8>>) -> Result<(Vec<u8>, Stats, Option<Latest>), String> {
        let t0 = std::time::Instant::now();
        let mut prep = self.prepare(w, view, o)?;
        self.ensure_water(prep.water_visible.len());
        let q = &self.ctx.queue;
        q.write_buffer(&self.frame_bufs[0], 0, bytemuck::bytes_of(&prep.frame_main));
        if let Some(f) = &prep.frame_refl {
            q.write_buffer(&self.frame_bufs[1], 0, bytemuck::bytes_of(f));
        }
        for (i, f) in prep.frame_cascade.iter().enumerate() {
            q.write_buffer(&self.frame_bufs[2 + i], 0, bytemuck::bytes_of(f));
        }
        let t = self.targets.as_ref().unwrap();
        let cur = self.accum_cur;
        let n = prep.refine_n.max(1);
        self.write_post(&t.prefilter.params, [1.0, 0.6, 0.0, 0.0], [prep.exposure, 0.0, 0.0, 0.0]);
        for p in &t.down_pass {
            self.write_post(&p.params, [0.0; 4], [0.0; 4]);
        }
        for p in &t.up_pass {
            self.write_post(&p.params, [1.0, 0.0, 0.0, 0.0], [0.0; 4]);
        }
        self.write_post(&t.accum_pass.params, [1.0 / n as f32, 0.0, 0.0, 0.0], [0.0; 4]);
        let bloom = if prep.bloom > 0.001 { prep.bloom } else { 0.0 };
        self.write_post(&t.tonemap.params, [prep.exposure, bloom, prep.saturation, prep.vignette], [0.0; 4]);
        self.write_post(&t.encode.params, [if prep.fxaa { 1.0 } else { 0.0 }, 0.0, 0.0, 0.0], [0.0; 4]);
        // Water uniforms.
        let refl_valid = if prep.refl.is_some() { 1.0 } else { 0.0 };
        for (slot, wi) in prep.water_visible.iter().enumerate().take(self.water.len()) {
            let wt = &w.water[*wi];
            let u: [f32; 12] = [wt.level, wt.wave_height, wt.wave_speed, wt.clarity, wt.min[0], wt.min[1], wt.max[0], wt.max[1], wt.colour[0], wt.colour[1], wt.colour[2], refl_valid];
            q.write_buffer(&self.water[slot].ubuf, 0, bytemuck::cast_slice(&u));
        }

        let mut enc = self.ctx.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("rt frame") });
        let mut calls = 0usize;
        let refl_idx = prep.refl.is_some() as usize;
        let casc_base = 1 + refl_idx;
        let t = self.targets.as_ref().unwrap();
        // Shadows.
        if prep.use_shadows {
            for c in 0..prep.cascades.len() {
                let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("shadow"),
                    color_attachments: &[],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment { view: &t.shadow_layers[c], depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }), stencil_ops: None }),
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                pass.set_bind_group(0, &self.bg_min[2 + c], &[]);
                calls += self.draw_world(&mut pass, &prep, casc_base + c, &self.p.shadow_terrain, &self.p.shadow_mesh, false);
            }
        }
        // Reflection.
        if prep.refl.is_some() {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("reflection"),
                color_attachments: &[colour_attachment(&t.refl.view, clear([0.0, 0.0, 0.0, 1.0]))],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment { view: &t.refl_depth.view, depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }), stencil_ops: None }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &t.bg_refl, &[]);
            calls += self.draw_world(&mut pass, &prep, 1, &self.p.refl_terrain, &self.p.refl_mesh, true);
            pass.set_pipeline(&self.p.sky);
            pass.draw(0..3, 0..1);
        }
        // Depth and normals.
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("prepass"),
                color_attachments: &[colour_attachment(&t.normal.view, clear([0.5, 1.0, 0.5, 1.0]))],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment { view: &t.depth.view, depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }), stencil_ops: None }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &self.bg_min[0], &[]);
            calls += self.draw_world(&mut pass, &prep, 0, &self.p.pre_terrain, &self.p.pre_mesh, false);
        }
        // Ambient occlusion.
        if prep.use_ssao {
            for (target, pipe, group) in [(&t.ao_raw.view, &self.p.ssao, &t.bg_ssao), (&t.ao.view, &self.p.ssao_blur, &t.bg_ssao_blur)] {
                let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor { label: Some("ssao"), color_attachments: &[colour_attachment(target, clear([1.0; 4]))], depth_stencil_attachment: None, timestamp_writes: None, occlusion_query_set: None, multiview_mask: None });
                pass.set_pipeline(pipe);
                pass.set_bind_group(0, &self.bg_min[0], &[]);
                pass.set_bind_group(1, group, &[]);
                pass.draw(0..3, 0..1);
            }
        }
        // The lit scene and the sky.
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("scene"),
                color_attachments: &[colour_attachment(&t.hdr.view, clear([0.0, 0.0, 0.0, 1.0]))],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment { view: &t.depth.view, depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store }), stencil_ops: None }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &t.bg_main, &[]);
            calls += self.draw_world(&mut pass, &prep, 0, &self.p.main_terrain, &self.p.main_mesh, true);
            pass.set_pipeline(&self.p.sky);
            pass.draw(0..3, 0..1);
        }
        // Water.
        if !prep.water_visible.is_empty() {
            let ext = wgpu::Extent3d { width: t.w, height: t.h, depth_or_array_layers: 1 };
            enc.copy_texture_to_texture(t.depth.tex.as_image_copy(), t.depth_copy.tex.as_image_copy(), ext);
            enc.copy_texture_to_texture(t.hdr.tex.as_image_copy(), t.col_copy.tex.as_image_copy(), ext);
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("water"),
                color_attachments: &[colour_attachment(&t.hdr.view, wgpu::LoadOp::Load)],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment { view: &t.depth.view, depth_ops: None, stencil_ops: None }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.p.water);
            pass.set_bind_group(0, &t.bg_main, &[]);
            pass.set_vertex_buffer(0, self.water_grid.0.slice(..));
            pass.set_index_buffer(self.water_grid.1.slice(..), wgpu::IndexFormat::Uint32);
            for slot in 0..prep.water_visible.len().min(self.water.len()) {
                pass.set_bind_group(1, &self.water[slot].bind, &[]);
                pass.draw_indexed(0..self.water_grid.2, 0, 0..1);
                calls += 1;
            }
        }
        // Average the jittered frames, bloom, tone map, encode.
        let full = |enc: &mut wgpu::CommandEncoder, label: &str, target: &wgpu::TextureView, pipe: &wgpu::RenderPipeline, group: &wgpu::BindGroup| {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor { label: Some(label), color_attachments: &[colour_attachment(target, wgpu::LoadOp::Load)], depth_stencil_attachment: None, timestamp_writes: None, occlusion_query_set: None, multiview_mask: None });
            pass.set_pipeline(pipe);
            pass.set_bind_group(0, group, &[]);
            pass.draw(0..3, 0..1);
        };
        full(&mut enc, "accumulate", &t.accum[cur].view, &self.p.accum, &t.accum_pass.groups[cur]);
        if bloom > 0.0 {
            full(&mut enc, "prefilter", &t.down[0].view, &self.p.prefilter, &t.prefilter.groups[cur]);
            for i in 0..BLOOM_LEVELS - 1 {
                full(&mut enc, "bloom down", &t.down[i + 1].view, &self.p.down, &t.down_pass[i].groups[0]);
            }
            for i in (0..BLOOM_LEVELS - 1).rev() {
                full(&mut enc, "bloom up", &t.up[i].view, &self.p.up, &t.up_pass[i].groups[0]);
            }
        }
        full(&mut enc, "tonemap", &t.ldr_lin.view, &self.p.tonemap, &t.tonemap.groups[cur]);
        if !linear {
            full(&mut enc, "encode", &t.ldr8.view, &self.p.encode, &t.encode.groups[0]);
        }
        let (tex, bpp) = if linear { (&t.ldr_lin.tex, 8u32) } else { (&t.ldr8.tex, 4u32) };
        let (width, height) = (t.w, t.h);
        let extent = wgpu::Extent3d { width, height, depth_or_array_layers: 1 };
        let mut out = Vec::new();
        let mut latest = None;
        if let Some(ring_out) = ring {
            let (buffer, padded) = self.rb.ring_target(&self.ctx.device, width, height, bpp)?;
            enc.copy_texture_to_buffer(tex.as_image_copy(), wgpu::TexelCopyBufferInfo { buffer, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(padded), rows_per_image: Some(height) } }, extent);
            self.ctx.queue.submit([enc.finish()]);
            latest = Some(self.rb.ring_latest(&self.ctx.device, ring_out)?);
        } else {
            let (buffer, padded) = self.rb.sync_target(&self.ctx.device, width, height, bpp);
            enc.copy_texture_to_buffer(tex.as_image_copy(), wgpu::TexelCopyBufferInfo { buffer, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(padded), rows_per_image: Some(height) } }, extent);
            self.ctx.queue.submit([enc.finish()]);
            self.rb.read_sync(&self.ctx.device, &mut out)?;
        }
        self.accum_cur = 1 - self.accum_cur;
        prep.stats.draw_calls = calls;
        prep.stats.ms = t0.elapsed().as_secs_f32() * 1000.0;
        Ok((out, prep.stats, latest))
    }

    /// Draws the terrain chunks, then the models, of list `idx`. Returns
    /// the number of draw calls.
    fn draw_world<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>, prep: &Prepared, idx: usize, terrain_pipe: &'a wgpu::RenderPipeline, mesh_pipe: &'a wgpu::RenderPipeline, lit: bool) -> usize {
        let list = &prep.lists[idx];
        let mut calls = 0;
        if let Some(tg) = &self.terrain {
            pass.set_pipeline(terrain_pipe);
            if lit {
                pass.set_bind_group(1, &tg.bind, &[]);
            }
            for (i, j, lod) in &list.chunks {
                if let Some(c) = tg.chunks.get(&(*i, *j, *lod)) {
                    pass.set_vertex_buffer(0, c.vb.slice(..));
                    pass.set_index_buffer(c.ib.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..c.count, 0, 0..1);
                    calls += 1;
                }
            }
        }
        if list.batches.is_empty() && prep.dyn_draws[idx].is_empty() {
            return calls;
        }
        pass.set_pipeline(mesh_pipe);
        pass.set_vertex_buffer(1, self.inst.slice(..));
        for (bi, b) in list.batches.iter().enumerate() {
            let Some(key) = self.asset_key(b.asset) else { continue };
            let Some(m) = self.meshes.get(&(key, b.lod)) else { continue };
            pass.set_vertex_buffer(0, m.vb.slice(..));
            pass.set_index_buffer(m.ib.slice(..), wgpu::IndexFormat::Uint32);
            let s = prep.starts[idx][bi];
            for g in &m.groups {
                pass.set_bind_group(1, &g.bind, &[]);
                pass.draw_indexed(g.first..g.first + g.count, 0, s..s + b.instances.len() as u32);
                calls += 1;
            }
        }
        // Characters, posed.
        for (key, at) in &prep.dyn_draws[idx] {
            let Some(p) = self.posed.get(key) else { continue };
            pass.set_vertex_buffer(0, p.mesh.vb.slice(..));
            pass.set_index_buffer(p.mesh.ib.slice(..), wgpu::IndexFormat::Uint32);
            for g in &p.mesh.groups {
                pass.set_bind_group(1, &g.bind, &[]);
                pass.draw_indexed(g.first..g.first + g.count, 0, *at..*at + 1);
                calls += 1;
            }
        }
        calls
    }

    fn asset_key(&self, asset: usize) -> Option<u64> {
        self.keys.get(asset).copied()
    }
}
