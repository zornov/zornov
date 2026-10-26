// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The size-dependent textures of the real-time renderer and the bind
//! groups that point at them.

use super::gpu::{AO, DEPTH, HDR, LDR, NORMAL, Pipes};
use wgpu::util::DeviceExt;

#[allow(dead_code)]
pub struct Tex {
    pub tex: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub w: u32,
    pub h: u32,
}

pub fn tex2d(d: &wgpu::Device, label: &str, w: u32, h: u32, format: wgpu::TextureFormat, usage: wgpu::TextureUsages) -> Tex {
    let (w, h) = (w.max(1), h.max(1));
    let tex = d.create_texture(&wgpu::TextureDescriptor { label: Some(label), size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 }, mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format, usage, view_formats: &[] });
    let view = tex.create_view(&Default::default());
    Tex { tex, view, w, h }
}

/// One post pass: its parameters and the bind groups for its two ping-pong
/// variants.
pub struct PostPass {
    pub params: wgpu::Buffer,
    pub groups: Vec<wgpu::BindGroup>,
}

pub const BLOOM_LEVELS: usize = 5;

#[allow(dead_code)]
pub struct Targets {
    pub w: u32,
    pub h: u32,
    pub shadow_res: u32,
    pub hdr: Tex,
    pub depth: Tex,
    pub normal: Tex,
    pub ao_raw: Tex,
    pub ao: Tex,
    pub depth_copy: Tex,
    pub col_copy: Tex,
    pub refl: Tex,
    pub refl_depth: Tex,
    pub accum: [Tex; 2],
    pub down: Vec<Tex>,
    pub up: Vec<Tex>,
    pub ldr_lin: Tex,
    pub ldr8: Tex,
    pub shadow: wgpu::Texture,
    pub shadow_layers: Vec<wgpu::TextureView>,
    pub shadow_array: wgpu::TextureView,
    pub white: Tex,
    pub black: Tex,
    pub bg_main: wgpu::BindGroup,
    pub bg_refl: wgpu::BindGroup,
    pub bg_ssao: wgpu::BindGroup,
    pub bg_ssao_blur: wgpu::BindGroup,
    pub prefilter: PostPass,
    pub down_pass: Vec<PostPass>,
    pub up_pass: Vec<PostPass>,
    pub accum_pass: PostPass,
    pub tonemap: PostPass,
    pub encode: PostPass,
}

fn buf(d: &wgpu::Device) -> wgpu::Buffer {
    d.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("post params"), contents: &[0u8; 32], usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST })
}

fn post_group(d: &wgpu::Device, p: &Pipes, params: &wgpu::Buffer, a: &wgpu::TextureView, b: &wgpu::TextureView) -> wgpu::BindGroup {
    d.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("post"),
        layout: &p.layouts.post,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: params.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(a) },
            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(b) },
            wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(&p.post_samp) },
        ],
    })
}

impl Targets {
    pub fn new(ctx: &crate::gpu::GpuContext, p: &Pipes, w: u32, h: u32, shadow_res: u32, frame_bufs: &[wgpu::Buffer], env_view: &wgpu::TextureView) -> Targets {
        let d = &ctx.device;
        let ra = wgpu::TextureUsages::RENDER_ATTACHMENT;
        let tb = wgpu::TextureUsages::TEXTURE_BINDING;
        let cs = wgpu::TextureUsages::COPY_SRC;
        let cd = wgpu::TextureUsages::COPY_DST;
        let (hw, hh) = ((w / 2).max(1), (h / 2).max(1));
        let hdr = tex2d(d, "rt hdr", w, h, HDR, ra | tb | cs | cd);
        let depth = tex2d(d, "rt depth", w, h, DEPTH, ra | tb | cs);
        let normal = tex2d(d, "rt normal", w, h, NORMAL, ra | tb);
        let ao_raw = tex2d(d, "rt ao raw", hw, hh, AO, ra | tb);
        let ao = tex2d(d, "rt ao", hw, hh, AO, ra | tb);
        let depth_copy = tex2d(d, "rt depth copy", w, h, DEPTH, tb | cd);
        let col_copy = tex2d(d, "rt color copy", w, h, HDR, tb | cd);
        let refl = tex2d(d, "rt reflection", hw, hh, HDR, ra | tb);
        let refl_depth = tex2d(d, "rt reflection depth", hw, hh, DEPTH, ra);
        let accum = [tex2d(d, "rt accum 0", w, h, HDR, ra | tb | cs), tex2d(d, "rt accum 1", w, h, HDR, ra | tb | cs)];
        let mut down = Vec::new();
        let mut up = Vec::new();
        let (mut bw, mut bh) = (hw, hh);
        for i in 0..BLOOM_LEVELS {
            down.push(tex2d(d, "rt bloom down", bw, bh, HDR, ra | tb));
            if i + 1 < BLOOM_LEVELS {
                up.push(tex2d(d, "rt bloom up", bw, bh, HDR, ra | tb));
            }
            bw = (bw / 2).max(1);
            bh = (bh / 2).max(1);
        }
        let ldr_lin = tex2d(d, "rt ldr linear", w, h, HDR, ra | tb | cs);
        let ldr8 = tex2d(d, "rt ldr 8", w, h, LDR, ra | cs);
        let shadow = d.create_texture(&wgpu::TextureDescriptor { label: Some("rt shadow"), size: wgpu::Extent3d { width: shadow_res, height: shadow_res, depth_or_array_layers: 4 }, mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: DEPTH, usage: ra | tb, view_formats: &[] });
        let shadow_layers = (0..4).map(|i| shadow.create_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::D2), base_array_layer: i, array_layer_count: Some(1), ..Default::default() })).collect();
        let shadow_array = shadow.create_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::D2Array), ..Default::default() });
        // One-texel stand-ins for what a pass must bind but does not read.
        let white = tex2d(d, "rt white", 1, 1, AO, tb | cd);
        ctx.queue.write_texture(white.tex.as_image_copy(), &[255], wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(1), rows_per_image: Some(1) }, wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 });
        let black = tex2d(d, "rt black", 1, 1, HDR, tb | cd);
        let full = |frame: &wgpu::Buffer, ao: &wgpu::TextureView, refl: &wgpu::TextureView| {
            d.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("rt frame"),
                layout: &p.layouts.full,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: frame.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&shadow_array) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&p.shadow_samp) },
                    wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(env_view) },
                    wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::Sampler(&p.lin_samp) },
                    wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::TextureView(ao) },
                    wgpu::BindGroupEntry { binding: 6, resource: wgpu::BindingResource::TextureView(refl) },
                ],
            })
        };
        let bg_main = full(&frame_bufs[0], &ao.view, &refl.view);
        let bg_refl = full(&frame_bufs[1], &white.view, &black.view);
        let ssao_group = |a: &wgpu::TextureView| {
            d.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("rt ssao"),
                layout: &p.layouts.ssao,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&depth.view) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&normal.view) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(a) },
                ],
            })
        };
        // The first pass binds the (not yet written) final map only for its size.
        let bg_ssao = ssao_group(&ao.view);
        let bg_ssao_blur = ssao_group(&ao_raw.view);
        let prefilter = {
            let params = buf(d);
            let groups = (0..2).map(|i| post_group(d, p, &params, &accum[i].view, &black.view)).collect();
            PostPass { params, groups }
        };
        let down_pass = (0..BLOOM_LEVELS - 1)
            .map(|i| {
                let params = buf(d);
                PostPass { groups: vec![post_group(d, p, &params, &down[i].view, &black.view)], params }
            })
            .collect();
        // up[i] = up(src, down[i]) where src is down[i+1] for the deepest, else up[i+1].
        let up_pass = (0..BLOOM_LEVELS - 1)
            .map(|i| {
                let params = buf(d);
                let src = if i + 1 == BLOOM_LEVELS - 1 { &down[i + 1].view } else { &up[i + 1].view };
                PostPass { groups: vec![post_group(d, p, &params, src, &down[i].view)], params }
            })
            .collect();
        let accum_pass = {
            let params = buf(d);
            // Writing accum[i]: the history is the other one.
            let groups = (0..2).map(|i| post_group(d, p, &params, &hdr.view, &accum[1 - i].view)).collect();
            PostPass { params, groups }
        };
        let tonemap = {
            let params = buf(d);
            let groups = (0..2).map(|i| post_group(d, p, &params, &accum[i].view, &up[0].view)).collect();
            PostPass { params, groups }
        };
        let encode = {
            let params = buf(d);
            PostPass { groups: vec![post_group(d, p, &params, &ldr_lin.view, &black.view)], params }
        };
        Targets { w, h, shadow_res, hdr, depth, normal, ao_raw, ao, depth_copy, col_copy, refl, refl_depth, accum, down, up, ldr_lin, ldr8, shadow, shadow_layers, shadow_array, white, black, bg_main, bg_refl, bg_ssao, bg_ssao_blur, prefilter, down_pass, up_pass, accum_pass, tonemap, encode }
    }
}
