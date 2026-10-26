// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Studio's frame compositor.
//!
//! Turns a timeline ([`Scene`]) at a time into pixels: resolve the active
//! clips, decode their sources (with an LRU byte-budgeted cache), convert
//! to a linear-light premultiplied working space, run effects, grades and
//! masks, place each layer with its transform and motion pose, blend, and
//! convert to the output space. There are two backends with the same
//! pipeline: the GPU ([`gpu::GpuCompositor`], wgpu + WGSL) for the viewer
//! and export, and the CPU ([`CpuCompositor`]) as the reference and
//! fallback. Golden and agreement tests keep them within tolerance.
//!
//! - [`preview::Previewer`] drives a renderer from the viewer: always the
//!   newest request, stale ones dropped, never a lower resolution than
//!   asked for.
//! - [`export::Exporter`] renders a range in order through a worker pool
//!   with back-pressure.
//! - [`hit::layer_at`] answers "which layer is under this point", using
//!   each layer's real transformed bounds in stacking order.
//! - [`model_scene::ModelScene`] (feature `model`) adapts a project.

pub mod atmosphere;
pub mod blend;
pub mod colour_space;
#[cfg(feature = "fx")]
pub mod comp;
pub mod compositor;
pub mod effects;
pub mod export;
pub mod filter;
pub mod frame;
pub mod gradient;
pub mod gpu;
pub mod hit;
pub mod mask;
pub mod matte;
pub mod mesh_render;
pub mod model_anim;
#[cfg(feature = "model")]
pub mod model_scene;
pub mod nodes;
pub mod output;
pub mod preview;
pub mod prores_source;
pub mod proxy;
pub mod puppet;
pub mod realtime;
pub mod resolve;
pub mod reuse;
pub mod scene;
pub mod sky;
pub mod shadow;
pub mod source;
pub mod space;
pub mod stills;
pub mod text;
pub mod trace;
pub mod transform;
pub mod vector_art;
pub mod view;
pub mod viewport;
pub mod world_scene;
pub mod yuv;

pub use blend::BlendMode;
pub use compositor::{CpuCompositor, FrameRenderer, target_size};
pub use frame::Frame;
pub use gpu::{GpuCompositor, GpuContext};
pub use mask::{MaskMode, MaskShape, MaskSpec};
pub use output::{EncodedFrame, EncodedPixels, OutputDepth, OutputSettings, OutputSpace};
pub use resolve::{RenderList, resolve};
pub use scene::{Canvas, ClipDesc, MediaKey, Micros, Scene, SourceDesc, StaticScene};
pub use source::{DecodeCache, SourceRegistry, SourceSpec, Sources};
pub use transform::{Placement, Pose};

/// Creates the best available renderer: the GPU when an adapter exists,
/// otherwise the CPU reference. Both share the given sources.
pub fn best_renderer(sources: Sources) -> Box<dyn FrameRenderer> {
    match GpuContext::new() {
        Some(ctx) => Box::new(GpuCompositor::new(ctx, sources)),
        None => Box::new(CpuCompositor::new(sources)),
    }
}
