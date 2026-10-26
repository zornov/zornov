// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Path tracing for 3D models: final renders with light that bounces.
//!
//! Every pixel sends many rays; each follows light backwards from the lens
//! through bounces off surfaces (diffuse, glossy GGX, metal, glass and
//! water that bend light, skin that scatters it under the surface, hair),
//! through smoke and fire, to lights, glowing surfaces (area lights), the
//! sky or a panorama. Next-event estimation with multiple importance
//! sampling keeps noise down; what is left is smoothed by an edge-aware
//! denoiser guided by the first hit's colour, normal and depth. Depth of
//! field comes from a thin lens, motion blur from rays spread over the
//! shutter between where things were and where they are going.
//!
//! The same algorithm runs on the GPU (a WGSL compute shader, see
//! [`gpu`]) and on the CPU ([`cpu`], the reference, used without a GPU and
//! by the tests). Besides the beauty picture a render gives passes for
//! compositing: depth, normals, albedo and object ids.

pub mod cpu;
pub mod denoise;
pub mod gpu;
pub mod scene;

use crate::frame::Frame;
use crate::mesh_render::MeshDesc;
use crate::scene::Canvas;

/// How a path-traced render is made.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TraceSettings {
    /// Rays per pixel for final renders.
    pub samples: u32,
    /// Rays per pixel when the picture is drawn smaller than the canvas
    /// (previews while editing).
    pub preview_samples: u32,
    /// Most bounces a path takes.
    pub bounces: u32,
    pub denoise: bool,
    /// The brightest a single ray may bring back (tames fireflies).
    pub clamp: f32,
    /// Fraction of a frame the shutter is open for motion blur (0 = none;
    /// 0.5 = the classic 180° shutter).
    pub shutter: f32,
}

impl Default for TraceSettings {
    fn default() -> TraceSettings {
        TraceSettings { samples: 64, preview_samples: 8, bounces: 6, denoise: true, clamp: 24.0, shutter: 0.0 }
    }
}

/// A finished render and its passes (all at the render's size).
#[derive(Clone, Debug)]
pub struct Passes {
    /// The picture: linear, premultiplied.
    pub beauty: Frame,
    /// Distance from the lens to the first surface (px; infinity where
    /// nothing was hit).
    pub depth: Vec<f32>,
    /// World normals of the first surface.
    pub normal: Vec<[f32; 3]>,
    /// Surface colour of the first surface (no light).
    pub albedo: Vec<[f32; 3]>,
    /// The model layer (clip id) seen first; 0 for none.
    pub id: Vec<u64>,
}

impl Passes {
    /// Depth as a grey picture (near white, far black) for compositing.
    pub fn depth_frame(&self, width: u32, height: u32) -> Frame {
        let finite: Vec<f32> = self.depth.iter().copied().filter(|d| d.is_finite()).collect();
        let near = finite.iter().copied().fold(f32::INFINITY, f32::min);
        let far = finite.iter().copied().fold(0.0f32, f32::max);
        let mut f = Frame::new(width, height);
        for (p, d) in f.pixels.iter_mut().zip(&self.depth) {
            let v = if d.is_finite() && far > near { 1.0 - (d - near) / (far - near) } else if d.is_finite() { 1.0 } else { 0.0 };
            *p = [v, v, v, 1.0];
        }
        f
    }

    /// Normals as colour (x, y, z from −1..1 into 0..1).
    pub fn normal_frame(&self, width: u32, height: u32) -> Frame {
        let mut f = Frame::new(width, height);
        for (p, n) in f.pixels.iter_mut().zip(&self.normal) {
            *p = if *n == [0.0; 3] { [0.0; 4] } else { [n[0] * 0.5 + 0.5, n[1] * 0.5 + 0.5, n[2] * 0.5 + 0.5, 1.0] };
        }
        f
    }

    pub fn albedo_frame(&self, width: u32, height: u32) -> Frame {
        let mut f = Frame::new(width, height);
        for ((p, a), d) in f.pixels.iter_mut().zip(&self.albedo).zip(&self.depth) {
            *p = if d.is_finite() { [a[0], a[1], a[2], 1.0] } else { [0.0; 4] };
        }
        f
    }

    /// Object ids as flat colours (each layer its own, steady per id), for
    /// picking mattes in the compositor.
    pub fn id_frame(&self, width: u32, height: u32) -> Frame {
        let mut f = Frame::new(width, height);
        for (p, id) in f.pixels.iter_mut().zip(&self.id) {
            if *id == 0 {
                continue;
            }
            let h = id.wrapping_mul(0x9E37_79B9_7F4A_7C15);
            let c = |s: u32| ((h >> s) & 0xff) as f32 / 255.0 * 0.8 + 0.2;
            *p = [c(8), c(24), c(40), 1.0];
        }
        f
    }
}

/// Renders the models of `desc` path-traced at `target` pixels (the
/// canvas at that scale), on the GPU when there is one.
pub fn render(desc: &MeshDesc, canvas: &Canvas, target: (u32, u32), settings: &TraceSettings) -> Passes {
    let (tw, th) = (target.0.max(1), target.1.max(1));
    let preview = tw < canvas.width || th < canvas.height;
    let samples = if preview { settings.preview_samples } else { settings.samples }.clamp(1, 16_384);
    let scene = scene::Scene::build(desc, canvas, (tw, th));
    let mut passes = gpu::render(&scene, samples, settings).unwrap_or_else(|| cpu::render(&scene, samples, settings));
    if settings.denoise && samples > 1 {
        denoise::denoise(&mut passes, tw as usize, th as usize, samples);
    }
    passes
}
