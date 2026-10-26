// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Compositing graphs on the GPU: per-pixel nodes run as the `fx` crate's
//! WGSL kernels (`studio_fx::comp::gpu`) on textures that stay on the GPU;
//! every other node runs on the CPU (`studio_fx::comp::run_node`, cached
//! by content) with its inputs read back and its result uploaded. A
//! picture's extra channels (depth, motion, id) travel in a second
//! texture.

use std::collections::HashMap;
use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use studio_fx::comp::{self as fxc, Graph, NodeId, Plate};

use super::{Binding, GpuCompositor, Tex, WORK};
use crate::effects::EffectCtx;
use crate::frame::Frame;
use crate::scene::EffectDesc;

const COMP_BINDINGS: &[Binding] = &[Binding::Tex, Binding::Tex, Binding::Tex, Binding::Tex, Binding::Tex, Binding::Sampler, WORK, WORK, Binding::Uniform];

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct CompUniforms {
    size: [f32; 2],
    time: f32,
    seed: f32,
    mix_amount: f32,
    has_b: f32,
    has_mask: f32,
    has_aux: f32,
    p: [[f32; 4]; 16],
}

/// A picture on the GPU with its extra channels (bits: 1 depth, 2 motion,
/// 4 id) in `aux`.
#[derive(Clone)]
struct GPlate {
    rgba: Tex,
    aux: Option<Tex>,
    chans: u32,
}

/// A node's result: on the GPU, or on the CPU.
#[derive(Clone)]
enum Res {
    Gpu(GPlate),
    Cpu(Arc<Plate>),
}

fn chans_of(p: &Plate) -> u32 {
    u32::from(p.depth.is_some()) | (u32::from(p.motion.is_some()) << 1) | (u32::from(p.id.is_some()) << 2)
}

impl GpuCompositor {
    /// Runs a `comp` effect over `tex`.
    pub(super) fn apply_comp(&mut self, tex: Tex, e: &EffectDesc, ctx: EffectCtx) -> Tex {
        let Some(graph) = crate::comp::graph_of(e) else { return tex };
        if graph.check().is_err() {
            return tex;
        }
        match self.run_graph(&graph, &tex, ctx) {
            Some(out) => {
                self.release(tex);
                out
            }
            None => tex,
        }
    }

    fn run_graph(&mut self, graph: &Graph, input: &Tex, ctx: EffectCtx) -> Option<Tex> {
        let out = graph.output_id()?;
        // Only the nodes the output needs.
        let mut need = Vec::new();
        fn visit(g: &Graph, id: NodeId, need: &mut Vec<NodeId>) {
            if need.contains(&id) {
                return;
            }
            if let Some(n) = g.node(id) {
                for i in n.inputs.iter().flatten().chain(n.mask.iter()) {
                    visit(g, *i, need);
                }
            }
            need.push(id);
        }
        visit(graph, out, &mut need);
        let (w, h) = (input.width as usize, input.height as usize);
        let mut input_img: Option<Arc<studio_fx::Image>> = None;
        let mut results: HashMap<NodeId, Res> = HashMap::new();
        for &id in &need {
            let n = graph.node(id)?;
            let t = ctx.time as f64;
            let p = n.params_at(t);
            if n.kind == "input" && !n.disabled {
                results.insert(id, Res::Gpu(GPlate { rgba: input.clone(), aux: None, chans: 0 }));
                continue;
            }
            let ins: Vec<Option<Res>> = n.inputs.iter().map(|i| i.and_then(|i| results.get(&i).cloned())).collect();
            let mask = n.mask.and_then(|m| results.get(&m).cloned());
            let gpu = if n.kind == "blur" && !n.disabled {
                studio_fx::comp::gpu::blur_passes(&p).map(|passes| (None, Some(passes)))
            } else {
                fxc::gpu::kernel(n, &p, w, h, t, ctx.frame_rate as f64).map(|k| (Some(k), None))
            };
            let res = match gpu {
                Some((kernel, passes)) => {
                    let main = self.to_gpu(ins.first().cloned().flatten(), w, h);
                    let second = ins.get(1).cloned().flatten().map(|r| self.to_gpu(Some(r), w, h));
                    let mask = mask.map(|m| self.to_gpu(Some(m), w, h));
                    if let Some(k) = kernel {
                        let chans = fxc::gpu::out_channels(n, &p, main.chans, second.as_ref().map_or(0, |s| s.chans));
                        let o = self.kernel(k.name, k.wgsl, &k.params, &main, second.as_ref(), mask.as_ref(), n.mix, ctx, chans);
                        Res::Gpu(o)
                    } else {
                        let [a, b] = passes.unwrap();
                        let first = self.kernel(a.name, a.wgsl, &a.params, &main, None, None, 1.0, ctx, main.chans);
                        let o = self.kernel(b.name, b.wgsl, &b.params, &first, Some(&main), mask.as_ref(), n.mix, ctx, main.chans);
                        Res::Gpu(o)
                    }
                }
                None => {
                    // CPU: read back what the node needs.
                    let img = match &input_img {
                        Some(i) => i.clone(),
                        None => {
                            let f = self.read_frame(input);
                            let i = Arc::new(f.into_fx());
                            input_img = Some(i.clone());
                            i
                        }
                    };
                    let host = crate::comp::host(&img, ctx);
                    let cpu_ins: Vec<Option<Arc<Plate>>> = ins.iter().map(|r| r.clone().map(|r| self.to_cpu(r))).collect();
                    let cpu_mask = mask.map(|m| self.to_cpu(m));
                    let p = fxc::run_node(graph, &host, id, &cpu_ins, cpu_mask).ok()?;
                    Res::Cpu(p)
                }
            };
            results.insert(id, res);
        }
        let out = results.remove(&out)?;
        // Free what is not the result.
        let keep = match &out {
            Res::Gpu(g) => Some(g.rgba.texture.clone()),
            Res::Cpu(_) => None,
        };
        for (_, r) in results {
            if let Res::Gpu(g) = r {
                if let Some(a) = g.aux {
                    self.release(a);
                }
                let is_input = g.rgba.texture == input.texture;
                let is_out = keep.as_ref().is_some_and(|k| *k == g.rgba.texture);
                if !is_input && !is_out {
                    self.release(g.rgba);
                }
            }
        }
        Some(match out {
            Res::Gpu(g) => {
                if let Some(a) = g.aux {
                    self.release(a);
                }
                if g.rgba.texture == input.texture {
                    // The graph passed its input through: hand back a copy
                    // so the caller can release the original.
                    let t = self.acquire(input.width, input.height);
                    self.copy_region(input, 0, 0, &t);
                    t
                } else {
                    g.rgba
                }
            }
            Res::Cpu(p) => self.upload_frame(&Frame::from_fx(p.rgba.clone())),
        })
    }

    fn to_gpu(&mut self, r: Option<Res>, w: usize, h: usize) -> GPlate {
        match r {
            Some(Res::Gpu(g)) => g,
            Some(Res::Cpu(p)) => {
                let rgba = self.upload_frame(&Frame::from_fx(p.rgba.clone()));
                let chans = chans_of(&p);
                let aux = (chans != 0).then(|| {
                    let n = p.rgba.pixels.len();
                    let pixels = (0..n)
                        .map(|i| {
                            let d = p.depth.as_ref().map_or(0.0, |d| d[i].clamp(-65000.0, 65000.0));
                            let m = p.motion.as_ref().map_or([0.0, 0.0], |m| m[i]);
                            let id = p.id.as_ref().map_or(0.0, |v| v[i]);
                            [d, m[0], m[1], id]
                        })
                        .collect();
                    self.upload_frame(&Frame { width: p.width() as u32, height: p.height() as u32, pixels })
                });
                GPlate { rgba, aux, chans }
            }
            None => {
                let rgba = self.upload_frame(&Frame { width: w as u32, height: h as u32, pixels: vec![[0.0; 4]; w * h] });
                GPlate { rgba, aux: None, chans: 0 }
            }
        }
    }

    fn to_cpu(&mut self, r: Res) -> Arc<Plate> {
        match r {
            Res::Cpu(p) => p,
            Res::Gpu(g) => {
                let rgba = self.read_frame(&g.rgba).into_fx();
                let mut p = Plate::new(rgba);
                if let (Some(aux), true) = (&g.aux, g.chans != 0) {
                    let a = self.read_frame(aux);
                    if g.chans & 1 != 0 {
                        p.depth = Some(a.pixels.iter().map(|q| q[0]).collect());
                    }
                    if g.chans & 2 != 0 {
                        p.motion = Some(a.pixels.iter().map(|q| [q[1], q[2]]).collect());
                    }
                    if g.chans & 4 != 0 {
                        p.id = Some(a.pixels.iter().map(|q| q[3]).collect());
                    }
                }
                Arc::new(p)
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn kernel(&mut self, name: &str, wgsl: &str, params: &[[f32; 4]], main: &GPlate, second: Option<&GPlate>, mask: Option<&GPlate>, mix: f32, ctx: EffectCtx, out_chans: u32) -> GPlate {
        let gpu = self.ctx.clone();
        let pipeline = gpu.pipelines.fx_pipeline(&gpu.device, &format!("comp:{name}"), wgsl, COMP_BINDINGS);
        let (w, h) = (main.rgba.width, main.rgba.height);
        let mut p = [[0.0f32; 4]; 16];
        for (d, s) in p.iter_mut().zip(params) {
            *d = *s;
        }
        let flags = fxc::gpu::aux_flags(main.chans, second.map_or(0, |s| s.chans));
        let u = self.uniform(&CompUniforms {
            size: [w as f32, h as f32],
            time: ctx.time,
            seed: f32::from_bits(ctx.seed),
            mix_amount: mix.clamp(0.0, 1.0),
            has_b: f32::from(u8::from(second.is_some())),
            has_mask: f32::from(u8::from(mask.is_some())),
            has_aux: flags as f32,
            p,
        });
        // Stand-ins for missing pictures.
        let blank = self.acquire(1, 1);
        let out = self.acquire(w, h);
        let out_aux = self.acquire(w, h);
        let main_aux = main.aux.as_ref().unwrap_or(&blank).view.clone();
        let (b, b_aux) = match second {
            Some(s) => (s.rgba.view.clone(), s.aux.as_ref().unwrap_or(&blank).view.clone()),
            None => (blank.view.clone(), blank.view.clone()),
        };
        let m = mask.map_or(blank.view.clone(), |m| m.rgba.view.clone());
        self.run(
            &pipeline,
            vec![
                wgpu::BindingResource::TextureView(&main.rgba.view),
                wgpu::BindingResource::TextureView(&main_aux),
                wgpu::BindingResource::TextureView(&b),
                wgpu::BindingResource::TextureView(&b_aux),
                wgpu::BindingResource::TextureView(&m),
                wgpu::BindingResource::Sampler(&gpu.pipelines.sampler),
                wgpu::BindingResource::TextureView(&out.view),
                wgpu::BindingResource::TextureView(&out_aux.view),
                u.as_entire_binding(),
            ],
            w,
            h,
        );
        self.release(blank);
        if out_chans == 0 {
            self.release(out_aux);
            return GPlate { rgba: out, aux: None, chans: 0 };
        }
        GPlate { rgba: out, aux: Some(out_aux), chans: out_chans }
    }
}
