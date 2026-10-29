// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The GPU backend: the same pipeline as [`crate::CpuCompositor`], run as wgpu
//! compute passes with the WGSL in `shaders/` (plus the effect, transition
//! and grade shaders the `fx` and `colour` crates ship).
//!
//! Textures stay on the GPU between steps. Decoded pictures upload as their
//! Y′CbCr planes and convert in `convert.wgsl`; generators and the text
//! renderer draw on the CPU and upload as `rgba16float`. Anything without a
//! shader (effects or transitions the `fx` crate only implements on the
//! CPU, grades with spatial tools) falls back transparently: the picture is
//! read back, processed on the CPU, and uploaded again, so the output is
//! always complete and the two backends agree within tolerance.
//!
//! Every pass is a compute shader with an 8×8 workgroup, so no render
//! pipelines, vertex buffers or surfaces are needed and the same code runs
//! headless for export and tests. Bind group layouts are taken from the
//! shaders (default pipeline layouts).

#[cfg(feature = "fx")]
mod comp;
mod f16;

pub use f16::{f16_to_f32, f32_to_f16, pixels_to_f16_bytes};

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, Weak};

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use crate::blend::BlendMode;
use crate::compositor::{FrameRenderer, target_size};
use crate::effects::{self, EffectCtx};
use crate::filter::gaussian_kernel;
use crate::frame::Frame;
use crate::mask::{self, MaskSpec, PreparedMask};
use crate::output::{EncodedFrame, EncodedPixels, OutputDepth, OutputSettings};
use crate::resolve::{Node, RenderList, resolve};
use crate::reuse::{Content, ContentKey, EntryId, LayerCache, ReuseStats, Sized2d, media_picture};
use crate::scene::{Canvas, ClipDesc, EffectDesc, GradeDesc, Micros, SECOND, Scene, SourceDesc};
use crate::source::Sources;
use crate::compositor::{ProjLayer, dof_sigma, in_space};
use crate::space::{self, CameraDesc, Caster, LightDesc, NEAR, Proj};
use crate::transform::{Affine, Homography, Pose, crop_rect, layer_matrix, visible_rect};
use crate::yuv::{DecodedPicture, PictureRef, PixelData, Planes};

const WORKGROUP: u32 = 8;
const WORKING_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// Converted pictures kept on the GPU (per compositor).
const PICTURE_CACHE: usize = 12;
/// Idle textures kept in the pool (per compositor).
const POOL_SIZE: usize = 24;
/// Bytes of processed layer textures kept between frames by default.
pub const GPU_CACHE_BUDGET: usize = 512 << 20;

fn groups(n: u32) -> u32 {
    n.div_ceil(WORKGROUP)
}

// ------------------------------------------------------------- uniforms
//
// Every struct mirrors the WGSL declaration of the same name; sizes are
// checked in the tests.

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ConvertParams {
    size: [u32; 2],
    chroma_div: [u32; 2],
    quant: [f32; 4],
    coef: [f32; 4],
    flags: [f32; 4],
    m: [[f32; 4]; 3],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct SizeParams {
    size: [u32; 2],
    a: u32,
    b: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct MatteParams {
    size: [u32; 2],
    msize: [u32; 2],
    invert: u32,
    pad: [u32; 3],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct MaskParams {
    size: [u32; 2],
    count: u32,
    init: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct CellParams {
    size: [u32; 2],
    grid: [u32; 2],
    cell: u32,
    pad: [u32; 3],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct WarpParams {
    size: [u32; 2],
    tiles_x: u32,
    tile_count: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct ShadowParams {
    inv0: [f32; 4],
    inv1: [f32; 4],
    row_y: [f32; 4],
    colour: [f32; 4],
    visible: [f32; 4],
    origin: [f32; 2],
    fade: f32,
    gain: f32,
    size: [u32; 2],
    pad1: [u32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct FillParams {
    size: [u32; 2],
    pad: [u32; 2],
    colour: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct CompParams {
    inv0: [f32; 4],
    inv1: [f32; 4],
    visible: [f32; 4],
    canvas_size: [u32; 2],
    opacity: f32,
    blend: u32,
    mask_count: u32,
    mask_init: f32,
    pad: [u32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Comp3dParams {
    inv0: [f32; 4],
    inv1: [f32; 4],
    inv2: [f32; 4],
    hw: [f32; 4],
    origin: [f32; 4],
    eu: [f32; 4],
    ev: [f32; 4],
    normal: [f32; 4],
    eye: [f32; 4],
    mat: [f32; 4],
    visible: [f32; 4],
    canvas_size: [u32; 2],
    opacity: f32,
    blend: u32,
    light_count: u32,
    caster_count: u32,
    lit: u32,
    side: f32,
    accepts_shadows: u32,
    self_caster: u32,
    near: f32,
    pad: u32,
}

/// One light as `composite3d.wgsl` reads it.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct LightGpu {
    pos: [f32; 4],
    dir: [f32; 4],
    colour: [f32; 4],
    cone: [f32; 4],
}

/// One shadow caster as `composite3d.wgsl` reads it.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct CasterGpu {
    origin: [f32; 4],
    du: [f32; 4],
    dv: [f32; 4],
    normal: [f32; 4],
    visible: [f32; 4],
}

fn v4(v: [f32; 3], w: f32) -> [f32; 4] {
    [v[0], v[1], v[2], w]
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct OutParams {
    size: [u32; 2],
    space: u32,
    premultiplied: u32,
    dither: u32,
    frame: u32,
    has_matrix: u32,
    pad: u32,
    m: [[f32; 4]; 3],
}

/// Uniforms of the `fx` crate's effect and transition shaders (`time` is
/// `progress` for transitions).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct FxUniforms {
    size: [f32; 2],
    time: f32,
    seed: f32,
    p: [[f32; 4]; 16],
}

/// One mask as `mask_common.wgsl` reads it.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct MaskGpu {
    kind: [u32; 4],
    geom: [f32; 4],
    rot: [f32; 4],
    misc: [f32; 4],
}

/// The adjustment-mix blend code understood by `composite.wgsl`.
const BLEND_ADJUST: u32 = 100;

// ------------------------------------------------------------- pipelines

#[cfg_attr(not(feature = "fx"), allow(dead_code))]
struct Pipelines {
    yuv: wgpu::ComputePipeline,
    rgba8: wgpu::ComputePipeline,
    composite: wgpu::ComputePipeline,
    composite3d: wgpu::ComputePipeline,
    mask: wgpu::ComputePipeline,
    half: wgpu::ComputePipeline,
    blur: wgpu::ComputePipeline,
    rotate: wgpu::ComputePipeline,
    fill: wgpu::ComputePipeline,
    cells: wgpu::ComputePipeline,
    warp: wgpu::ComputePipeline,
    output8: wgpu::ComputePipeline,
    output16: wgpu::ComputePipeline,
    grade: wgpu::ComputePipeline,
    matte: wgpu::ComputePipeline,
    shadow: wgpu::ComputePipeline,
    /// Linear, clamp-to-edge: what the `fx` shaders expect.
    sampler: wgpu::Sampler,
    /// Effect and transition pipelines by id, built on first use.
    fx: Mutex<HashMap<String, Arc<wgpu::ComputePipeline>>>,
}

/// Kinds of binding a shader declares, in binding order. Layouts are
/// spelled out rather than taken from the shaders because a default layout
/// only holds the bindings an entry point uses, and several passes share a
/// module and a resource list (the resample passes, the fx shaders).
#[derive(Clone, Copy)]
#[cfg_attr(not(feature = "fx"), allow(dead_code))]
enum Binding {
    /// `texture_2d<f32>`.
    Tex,
    /// `texture_2d<u32>` (decoded planes).
    TexUint,
    /// `texture_3d<f32>` (3D LUT, `rgba32float`, never filtered).
    Tex3d,
    /// `texture_2d<f32>` holding `rgba32float` (1D LUT), never filtered.
    TexUnfiltered,
    /// `texture_storage_2d<format, write>`.
    Storage(wgpu::TextureFormat),
    Uniform,
    /// `var<storage, read>`.
    ReadOnly,
    Sampler,
}

fn layout(device: &wgpu::Device, label: &str, bindings: &[Binding]) -> wgpu::PipelineLayout {
    let entries: Vec<wgpu::BindGroupLayoutEntry> = bindings
        .iter()
        .enumerate()
        .map(|(i, b)| wgpu::BindGroupLayoutEntry {
            binding: i as u32,
            visibility: wgpu::ShaderStages::COMPUTE,
            count: None,
            ty: match b {
                Binding::Tex => wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
                Binding::TexUint => wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Uint, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
                Binding::Tex3d => wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: false }, view_dimension: wgpu::TextureViewDimension::D3, multisampled: false },
                Binding::TexUnfiltered => wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: false }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
                Binding::Storage(format) => wgpu::BindingType::StorageTexture { access: wgpu::StorageTextureAccess::WriteOnly, format: *format, view_dimension: wgpu::TextureViewDimension::D2 },
                Binding::Uniform => wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                Binding::ReadOnly => wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None },
                Binding::Sampler => wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            },
        })
        .collect();
    let group = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some(label), entries: &entries });
    device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some(label), bind_group_layouts: &[Some(&group)], immediate_size: 0 })
}

/// The name a shader module goes to the compiler under. DirectX 12 hands it
/// to DXC as the source's file name, and a name like `comp:grade` reads as a
/// drive and a path: DXC goes to the disk for it and fails ("error reading").
fn module_name(label: &str) -> String {
    label.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' }).collect()
}

fn pipeline(device: &wgpu::Device, label: &str, source: &str, entry: &str, bindings: &[Binding]) -> wgpu::ComputePipeline {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some(&module_name(label)), source: wgpu::ShaderSource::Wgsl(source.into()) });
    let layout = layout(device, label, bindings);
    device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some(label),
        layout: Some(&layout),
        module: &module,
        entry_point: Some(entry),
        compilation_options: Default::default(),
        cache: None,
    })
}

const WORK: Binding = Binding::Storage(WORKING_FORMAT);
const CONVERT_BINDINGS: &[Binding] = &[Binding::TexUint, Binding::TexUint, Binding::TexUint, WORK, Binding::Uniform];
const RGBA8_BINDINGS: &[Binding] = &[Binding::Tex, WORK, Binding::Uniform];
const COMPOSITE_BINDINGS: &[Binding] = &[Binding::Tex, WORK, Binding::Tex, Binding::Uniform, Binding::ReadOnly, Binding::ReadOnly];
const COMPOSITE3D_BINDINGS: &[Binding] =
    &[Binding::Tex, WORK, Binding::Tex, Binding::Uniform, Binding::ReadOnly, Binding::ReadOnly, Binding::Tex, Binding::Tex, Binding::Tex, Binding::Tex];
const MASK_BINDINGS: &[Binding] = &[Binding::Tex, WORK, Binding::Uniform, Binding::ReadOnly, Binding::ReadOnly];
const RESAMPLE_BINDINGS: &[Binding] = &[Binding::Tex, WORK, Binding::Uniform, Binding::ReadOnly];
const FILL_BINDINGS: &[Binding] = &[WORK, Binding::Uniform];
const CELLS_BINDINGS: &[Binding] = &[Binding::Tex, WORK, Binding::Uniform];
const WARP_BINDINGS: &[Binding] = &[Binding::Tex, WORK, Binding::Uniform, Binding::ReadOnly, Binding::ReadOnly];
const GRADE_BINDINGS: &[Binding] = &[Binding::Tex, WORK, Binding::Uniform, Binding::Tex3d, Binding::TexUnfiltered];
const MATTE_BINDINGS: &[Binding] = &[Binding::Tex, WORK, Binding::Uniform, Binding::Tex];
const SHADOW_BINDINGS: &[Binding] = &[Binding::Tex, WORK, Binding::Uniform];
#[cfg(feature = "fx")]
const EFFECT_BINDINGS: &[Binding] = &[Binding::Tex, Binding::Sampler, WORK, Binding::Uniform];
/// An effect shader that also reads CPU-prepared data (binding 4).
#[cfg(feature = "fx")]
const EFFECT_DATA_BINDINGS: &[Binding] = &[Binding::Tex, Binding::Sampler, WORK, Binding::Uniform, Binding::ReadOnly];
#[cfg(feature = "fx")]
const TRANSITION_BINDINGS: &[Binding] = &[Binding::Tex, Binding::Tex, Binding::Sampler, WORK, Binding::Uniform];

/// The complete WGSL modules this crate builds, by name. Exposed so the
/// tests can validate them with naga without a GPU.
pub fn shader_sources() -> Vec<(&'static str, String)> {
    let mask_common = include_str!("shaders/mask_common.wgsl");
    let output = include_str!("shaders/output.wgsl");
    let blend = include_str!("shaders/blend_common.wgsl");
    vec![
        ("convert", format!("{}\n{}", studio_colour::gpu::CAMERA_WGSL, include_str!("shaders/convert.wgsl"))),
        ("convert_rgba", include_str!("shaders/convert_rgba.wgsl").to_string()),
        ("composite", format!("{mask_common}\n{blend}\n{}", include_str!("shaders/composite.wgsl"))),
        ("composite3d", format!("{blend}\n{}", include_str!("shaders/composite3d.wgsl"))),
        ("mask", format!("{mask_common}\n{}", include_str!("shaders/mask.wgsl"))),
        ("resample", include_str!("shaders/resample.wgsl").to_string()),
        ("fill", include_str!("shaders/fill.wgsl").to_string()),
        ("cells", include_str!("shaders/cells.wgsl").to_string()),
        ("warp", include_str!("shaders/warp.wgsl").to_string()),
        ("matte", include_str!("shaders/matte.wgsl").to_string()),
        ("shadow", include_str!("shaders/shadow.wgsl").to_string()),
        ("output8", output.replace("OUT_FORMAT", "rgba8unorm").replace("OUT_SCALE", "255.0").replace("OUT_STORE", "q / 255.0")),
        ("output16", output.replace("OUT_FORMAT", "rgba16uint").replace("OUT_SCALE", "65535.0").replace("OUT_STORE", "vec4<u32>(q)")),
    ]
}

impl Pipelines {
    fn new(device: &wgpu::Device) -> Pipelines {
        let src: HashMap<&str, String> = shader_sources().into_iter().collect();
        let resample = &src["resample"];
        Pipelines {
            yuv: pipeline(device, "convert_yuv", &src["convert"], "cs_yuv", CONVERT_BINDINGS),
            rgba8: pipeline(device, "convert_rgba8", &src["convert_rgba"], "cs_rgba8", RGBA8_BINDINGS),
            composite: pipeline(device, "composite", &src["composite"], "cs_composite", COMPOSITE_BINDINGS),
            composite3d: pipeline(device, "composite3d", &src["composite3d"], "cs_composite3d", COMPOSITE3D_BINDINGS),
            mask: pipeline(device, "mask", &src["mask"], "cs_mask", MASK_BINDINGS),
            half: pipeline(device, "half", resample, "cs_half", RESAMPLE_BINDINGS),
            blur: pipeline(device, "blur", resample, "cs_blur", RESAMPLE_BINDINGS),
            rotate: pipeline(device, "rotate", resample, "cs_rotate", RESAMPLE_BINDINGS),
            fill: pipeline(device, "fill", &src["fill"], "cs_fill", FILL_BINDINGS),
            cells: pipeline(device, "cells", &src["cells"], "cs_cells", CELLS_BINDINGS),
            warp: pipeline(device, "warp", &src["warp"], "cs_warp", WARP_BINDINGS),
            output8: pipeline(device, "output8", &src["output8"], "cs_output", &[Binding::Tex, Binding::Storage(wgpu::TextureFormat::Rgba8Unorm), Binding::Uniform]),
            output16: pipeline(device, "output16", &src["output16"], "cs_output", &[Binding::Tex, Binding::Storage(wgpu::TextureFormat::Rgba16Uint), Binding::Uniform]),
            grade: pipeline(device, "grade", &studio_colour::gpu::grade_shader_source(), "cs_grade", GRADE_BINDINGS),
            matte: pipeline(device, "matte", &src["matte"], "cs_matte", MATTE_BINDINGS),
            shadow: pipeline(device, "shadow", &src["shadow"], "cs_shadow", SHADOW_BINDINGS),
            sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("fx linear clamp"),
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                address_mode_w: wgpu::AddressMode::ClampToEdge,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                mipmap_filter: wgpu::MipmapFilterMode::Nearest,
                ..Default::default()
            }),
            fx: Mutex::new(HashMap::new()),
        }
    }

    /// An `fx` shader pipeline by id (effect ids and transition ids do not
    /// collide: transitions are prefixed).
    #[cfg(feature = "fx")]
    fn fx_pipeline(&self, device: &wgpu::Device, key: &str, source: &str, bindings: &[Binding]) -> Arc<wgpu::ComputePipeline> {
        let mut g = self.fx.lock().unwrap_or_else(|e| e.into_inner());
        g.entry(key.to_string()).or_insert_with(|| Arc::new(pipeline(device, key, source, "main", bindings))).clone()
    }
}

// --------------------------------------------------------------- context

/// A device, its queue and the compiled pipelines. Shared by every
/// [`GpuCompositor`] (preview and export workers) and, in the application,
/// with the user interface's own wgpu renderer.
pub struct GpuContext {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    /// Adapter name, for the about box and the benchmark table.
    pub adapter_name: String,
    /// A software adapter (llvmpipe, WARP): right pictures at CPU speed, so
    /// frame times say nothing about a GPU.
    pub software: bool,
    pipelines: Pipelines,
    /// Held from pushing an error scope to popping it: the device is shared
    /// by every thread, and wgpu's scopes must pop in the order they were
    /// pushed.
    scopes: Mutex<()>,
}

impl GpuContext {
    /// The process's GPU context: the best available adapter with its
    /// pipelines built, opened on first use and shared from then on. `None`
    /// when there is no usable GPU, when a shader fails to compile on this
    /// driver, or when `STUDIO_RENDER_NO_GPU` is set (which the tests and
    /// the benchmark use to force the CPU path).
    ///
    /// One context, never dropped, like `direct`, `trace` and `atmosphere`
    /// keep theirs: opening and dropping devices on several threads at once
    /// crashes the Vulkan loader (a dropped instance unloads the driver while
    /// another thread is still inside it) and WARP on Windows. It also spares
    /// every renderer building the device and its pipelines again.
    pub fn new() -> Option<Arc<GpuContext>> {
        if std::env::var_os("STUDIO_RENDER_NO_GPU").is_some() {
            return None;
        }
        static SHARED: OnceLock<Option<Arc<GpuContext>>> = OnceLock::new();
        SHARED.get_or_init(GpuContext::open).clone()
    }

    /// Hold this while pushing and popping error scopes on the device.
    pub fn scopes(&self) -> std::sync::MutexGuard<'_, ()> {
        self.scopes.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn open() -> Option<Arc<GpuContext>> {
        let g = studio_gpu_device::shared()?;
        GpuContext::build(g.device.clone(), g.queue.clone(), g.adapter_name.clone(), g.software)
    }

    /// Wraps an existing device (the application shares egui's). `None` if
    /// a shader does not compile on it.
    pub fn from_device(device: wgpu::Device, queue: wgpu::Queue, adapter_name: String) -> Option<Arc<GpuContext>> {
        GpuContext::build(device, queue, adapter_name, false)
    }

    fn build(device: wgpu::Device, queue: wgpu::Queue, adapter_name: String, software: bool) -> Option<Arc<GpuContext>> {
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let pipelines = Pipelines::new(&device);
        let err = pollster::block_on(scope.pop());
        if let Some(e) = err {
            eprintln!("studio-render: GPU pipelines unavailable: {e}");
            return None;
        }
        Some(Arc::new(GpuContext { device, queue, adapter_name, software, pipelines, scopes: Mutex::new(()) }))
    }
}

// ------------------------------------------------------------- textures

/// A working-format texture and its view.
#[derive(Clone)]
struct Tex {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    width: u32,
    height: u32,
}

/// The two canvas textures a frame alternates between: every composite
/// pass reads one and writes the other.
struct Canvases {
    cur: Tex,
    other: Tex,
}

/// A layer ready to be placed (the GPU form of `LayerPicture`).
struct GpuLayer {
    tex: Tex,
    /// Whether the texture is this layer's to release (otherwise it
    /// belongs to the layer cache).
    owned: bool,
    matrix: Affine,
    visible: [f32; 4],
    opacity: f32,
    proj: Option<ProjLayer>,
}

/// Renders scenes on the GPU.
pub struct GpuCompositor {
    ctx: Arc<GpuContext>,
    sources: Sources,
    encoder: Option<wgpu::CommandEncoder>,
    /// Idle working textures for reuse.
    pool: Vec<Tex>,
    /// The staging buffer exact readbacks reuse.
    readback: crate::realtime::readback::Readback,
    /// Converted pictures, keyed by the decoded picture's address and the
    /// tone-mapping flag. The weak reference tells a stale address apart
    /// from a live picture.
    pictures: HashMap<(usize, bool), (Weak<DecodedPicture>, Tex, u64)>,
    stamp: u64,
    /// LUT textures by `.cube` file address: (3D, 1D) views.
    luts: HashMap<usize, (wgpu::TextureView, wgpu::TextureView)>,
    dummy_lut3d: wgpu::TextureView,
    dummy_lut1d: wgpu::TextureView,
    /// A transparent texel bound in place of missing shadow casters.
    dummy_caster: wgpu::TextureView,
    depth: u32,
    /// Processed layers kept between frames.
    cache: LayerCache<Tex>,
    /// Textures the cache let go of this frame; pooled at the next.
    retired: Vec<Tex>,
}

impl GpuCompositor {
    pub fn new(ctx: Arc<GpuContext>, sources: Sources) -> GpuCompositor {
        let d3 = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("lut3d dummy"),
            size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D3,
            format: wgpu::TextureFormat::Rgba32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let d1 = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("lut1d dummy"),
            size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let dc = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("caster dummy"),
            size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: WORKING_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        GpuCompositor {
            dummy_caster: dc.create_view(&Default::default()),
            dummy_lut3d: d3.create_view(&Default::default()),
            dummy_lut1d: d1.create_view(&Default::default()),
            ctx,
            sources,
            encoder: None,
            pool: Vec::new(),
            readback: crate::realtime::readback::Readback::new(),
            pictures: HashMap::new(),
            stamp: 0,
            luts: HashMap::new(),
            depth: 0,
            cache: LayerCache::new(GPU_CACHE_BUDGET),
            retired: Vec::new(),
        }
    }

    /// Layer cache counters.
    pub fn reuse_stats(&self) -> ReuseStats {
        self.cache.stats()
    }

    /// Keeps up to `bytes` of processed layer textures between frames.
    pub fn set_cache_budget(&mut self, bytes: usize) {
        let gone = self.cache.set_budget(bytes);
        self.retire(gone);
    }

    /// Pools what the cache let go of last frame and drops pictures not
    /// asked for again.
    fn begin_frame(&mut self) {
        let gone = self.cache.begin_frame();
        for t in std::mem::take(&mut self.retired).into_iter().chain(gone) {
            self.release(t);
        }
    }

    pub fn context(&self) -> &Arc<GpuContext> {
        &self.ctx
    }

    // ---- plumbing ----

    fn enc(&mut self) -> &mut wgpu::CommandEncoder {
        if self.encoder.is_none() {
            self.encoder = Some(self.ctx.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("studio-render") }));
        }
        self.encoder.as_mut().unwrap()
    }

    /// Submits the recorded work.
    fn flush(&mut self) {
        if let Some(e) = self.encoder.take() {
            self.ctx.queue.submit([e.finish()]);
        }
    }

    fn acquire(&mut self, width: u32, height: u32) -> Tex {
        let (width, height) = (width.max(1), height.max(1));
        if let Some(i) = self.pool.iter().position(|t| t.width == width && t.height == height) {
            return self.pool.swap_remove(i);
        }
        let texture = self.ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("working"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: WORKING_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        Tex { texture, view, width, height }
    }

    fn release(&mut self, t: Tex) {
        if self.pool.len() >= POOL_SIZE {
            self.pool.remove(0);
        }
        self.pool.push(t);
    }

    fn uniform<T: Pod>(&self, v: &T) -> wgpu::Buffer {
        self.ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("uniform"), contents: bytemuck::bytes_of(v), usage: wgpu::BufferUsages::UNIFORM })
    }

    fn storage(&self, bytes: &[u8]) -> wgpu::Buffer {
        self.ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("storage"), contents: bytes, usage: wgpu::BufferUsages::STORAGE })
    }

    /// Records one compute pass over `width × height` pixels with the
    /// resources bound in order.
    fn run(&mut self, pipeline: &wgpu::ComputePipeline, resources: Vec<wgpu::BindingResource<'_>>, width: u32, height: u32) {
        let entries: Vec<wgpu::BindGroupEntry> = resources.into_iter().enumerate().map(|(i, r)| wgpu::BindGroupEntry { binding: i as u32, resource: r }).collect();
        let layout = pipeline.get_bind_group_layout(0);
        let group = self.ctx.device.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: &layout, entries: &entries });
        let enc = self.enc();
        let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor { label: None, timestamp_writes: None });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &group, &[]);
        pass.dispatch_workgroups(groups(width), groups(height), 1);
    }

    fn write_texture(&self, texture: &wgpu::Texture, data: &[u8], bytes_per_row: u32, width: u32, height: u32, depth: u32) {
        self.ctx.queue.write_texture(
            texture.as_image_copy(),
            data,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(bytes_per_row), rows_per_image: Some(height) },
            wgpu::Extent3d { width, height, depth_or_array_layers: depth },
        );
    }

    fn plane_texture(&self, label: &str, plane: &Planes, width: u32, height: u32) -> wgpu::TextureView {
        let (format, bpp) = match plane {
            Planes::U8(_) => (wgpu::TextureFormat::R8Uint, 1),
            Planes::U16(_) => (wgpu::TextureFormat::R16Uint, 2),
        };
        let texture = self.ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let bytes: &[u8] = match plane {
            Planes::U8(v) => v,
            Planes::U16(v) => bytemuck::cast_slice(v),
        };
        let rows = (bytes.len() / (width as usize * bpp)).min(height as usize) as u32;
        self.write_texture(&texture, &bytes[..rows as usize * width as usize * bpp], width * bpp as u32, width, rows, 1);
        texture.create_view(&Default::default())
    }

    /// Reads a texture back: `bpp` bytes per pixel, rows unpadded.
    fn read_bytes(&mut self, texture: &wgpu::Texture, width: u32, height: u32, bpp: u32) -> Vec<u8> {
        let (buffer, padded) = self.readback.sync_target(&self.ctx.device, width, height, bpp);
        let buffer = buffer.clone();
        self.enc().copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo { buffer: &buffer, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(padded), rows_per_image: Some(height) } },
            wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        );
        self.flush();
        let mut out = Vec::new();
        self.readback.read_sync(&self.ctx.device, &mut out).expect("buffer map");
        out
    }

    fn read_frame(&mut self, t: &Tex) -> Frame {
        let bytes = self.read_bytes(&t.texture, t.width, t.height, 8);
        let pixels = bytes.chunks_exact(8).map(|c| std::array::from_fn(|i| f16_to_f32(u16::from_le_bytes([c[i * 2], c[i * 2 + 1]])))).collect();
        Frame { width: t.width, height: t.height, pixels }
    }

    /// Uploads a CPU frame into a pooled texture. The copy is recorded in
    /// the command encoder (not `Queue::write_texture`, which would run
    /// before the passes already recorded and could overwrite a texture
    /// one of them still reads).
    fn upload_frame(&mut self, f: &Frame) -> Tex {
        let t = self.acquire(f.width, f.height);
        let row = f.width as usize * 8;
        let padded = row.div_ceil(256) * 256;
        let mut bytes = vec![0u8; padded * f.height as usize];
        for (y, src) in f.pixels.chunks_exact(f.width as usize).enumerate() {
            let dst = &mut bytes[y * padded..y * padded + row];
            for (d, p) in dst.chunks_exact_mut(8).zip(src) {
                for (i, c) in p.iter().enumerate() {
                    d[i * 2..i * 2 + 2].copy_from_slice(&f32_to_f16(*c).to_le_bytes());
                }
            }
        }
        let staging = self.ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("upload"), contents: &bytes, usage: wgpu::BufferUsages::COPY_SRC });
        self.enc().copy_buffer_to_texture(
            wgpu::TexelCopyBufferInfo { buffer: &staging, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(padded as u32), rows_per_image: Some(f.height) } },
            t.texture.as_image_copy(),
            wgpu::Extent3d { width: f.width, height: f.height, depth_or_array_layers: 1 },
        );
        t
    }

    fn fill(&mut self, t: &Tex, colour: [f32; 4]) {
        let ctx = self.ctx.clone();
        let u = self.uniform(&FillParams { size: [t.width, t.height], pad: [0; 2], colour });
        self.run(&ctx.pipelines.fill, vec![wgpu::BindingResource::TextureView(&t.view), u.as_entire_binding()], t.width, t.height);
    }

    fn copy_region(&mut self, src: &Tex, x: u32, y: u32, dst: &Tex) {
        self.enc().copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo { texture: &src.texture, mip_level: 0, origin: wgpu::Origin3d { x, y, z: 0 }, aspect: wgpu::TextureAspect::All },
            dst.texture.as_image_copy(),
            wgpu::Extent3d { width: dst.width, height: dst.height, depth_or_array_layers: 1 },
        );
    }

    // ---- pictures ----

    /// Converts a decoded picture into the working space on the GPU.
    fn convert_picture(&mut self, pic: &DecodedPicture, to_sdr: bool) -> Tex {
        // OCIO input processors and non-sRGB stills convert on the CPU.
        let custom_still = matches!(pic.data, PixelData::Rgba8(_)) && (pic.colour.transfer != crate::colour_space::SourceTransfer::Srgb || pic.colour.primaries != crate::colour_space::SourcePrimaries::Bt709);
        if pic.colour.transfer.is_custom() || custom_still {
            return self.upload_frame(&pic.to_frame(to_sdr));
        }
        let ctx = self.ctx.clone();
        let out = self.acquire(pic.width, pic.height);
        match &pic.data {
            PixelData::Rgba8(rgba) => {
                let texture = ctx.device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("still"),
                    size: wgpu::Extent3d { width: pic.width, height: pic.height, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                });
                self.write_texture(&texture, rgba, pic.width * 4, pic.width, pic.height, 1);
                let view = texture.create_view(&Default::default());
                let u = self.uniform(&SizeParams { size: [pic.width, pic.height], a: 0, b: 0 });
                self.run(&ctx.pipelines.rgba8, vec![wgpu::BindingResource::TextureView(&view), wgpu::BindingResource::TextureView(&out.view), u.as_entire_binding()], pic.width, pic.height);
            }
            PixelData::Yuv { y, u, v, chroma_width, chroma_height } => {
                let w = pic.width as usize;
                let cw = *chroma_width as usize;
                // Same subsampling decision as `DecodedPicture::to_frame`.
                let xdiv = if cw >= w { 1 } else { 2 };
                let ydiv = if (y.len() / w.max(1)) <= (u.len() / cw.max(1)) { 1 } else { 2 };
                let ch = (u.len() / cw.max(1)).max(1).min(*chroma_height as usize) as u32;
                let yv = self.plane_texture("y", y, pic.width, pic.height);
                let uv = self.plane_texture("u", u, *chroma_width, ch);
                let vv = self.plane_texture("v", v, *chroma_width, ch);
                let desc = &pic.colour;
                let (yo, ys, co, cs) = desc.quantisation();
                let (kr, kb) = desc.matrix.ycbcr().kr_kb();
                let m = desc.to_working();
                let has_m = m != studio_colour::math::Mat3::IDENTITY;
                let mf = m.to_f32();
                let params = ConvertParams {
                    size: [pic.width, pic.height],
                    chroma_div: [xdiv as u32, ydiv as u32],
                    quant: [yo, ys, co, cs],
                    coef: [kr, kb, if desc.matrix == crate::colour_space::Matrix::Rgb { 1.0 } else { 0.0 }, desc.transfer.shader_id() as f32],
                    flags: [if to_sdr && desc.transfer.is_hdr() { 1.0 } else { 0.0 }, if has_m { 1.0 } else { 0.0 }, 0.0, 0.0],
                    m: std::array::from_fn(|r| [mf[r][0], mf[r][1], mf[r][2], 0.0]),
                };
                let ub = self.uniform(&params);
                self.run(
                    &ctx.pipelines.yuv,
                    vec![
                        wgpu::BindingResource::TextureView(&yv),
                        wgpu::BindingResource::TextureView(&uv),
                        wgpu::BindingResource::TextureView(&vv),
                        wgpu::BindingResource::TextureView(&out.view),
                        ub.as_entire_binding(),
                    ],
                    pic.width,
                    pic.height,
                );
            }
        }
        out
    }

    /// The converted texture of a cached picture, converting on a miss.
    /// The returned texture belongs to the picture cache; callers copy
    /// from it (the crop step) and never release it.
    fn picture_tex(&mut self, pic: &PictureRef, to_sdr: bool) -> Tex {
        self.stamp += 1;
        let key = (Arc::as_ptr(pic) as usize, to_sdr);
        if let Some((weak, tex, stamp)) = self.pictures.get_mut(&key)
            && weak.upgrade().is_some_and(|p| Arc::ptr_eq(&p, pic))
        {
            *stamp = self.stamp;
            return tex.clone();
        }
        let tex = self.convert_picture(pic, to_sdr);
        if self.pictures.len() >= PICTURE_CACHE
            && let Some((&k, _)) = self.pictures.iter().min_by_key(|(_, (_, _, s))| *s)
            && let Some((_, old, _)) = self.pictures.remove(&k)
        {
            self.release(old);
        }
        self.pictures.insert(key, (Arc::downgrade(pic), tex.clone(), self.stamp));
        tex
    }

    fn rotate(&mut self, src: &Tex, degrees: u32) -> Tex {
        let ctx = self.ctx.clone();
        let q = (degrees % 360) / 90;
        let (w, h) = if q % 2 == 1 { (src.height, src.width) } else { (src.width, src.height) };
        let out = self.acquire(w, h);
        let kernel = self.storage(&[0u8; 4]);
        let u = self.uniform(&SizeParams { size: [w, h], a: q, b: 0 });
        self.run(&ctx.pipelines.rotate, vec![wgpu::BindingResource::TextureView(&src.view), wgpu::BindingResource::TextureView(&out.view), u.as_entire_binding(), kernel.as_entire_binding()], w, h);
        out
    }

    /// The raw picture of a clip and how many source pixels one texel
    /// covers (`CpuCompositor::raw_picture`).
    fn raw_picture(&mut self, clip: &ClipDesc, canvas: &Canvas, target: (u32, u32), picture: Option<(PictureRef, u32)>) -> Option<(Tex, f32)> {
        match &clip.source {
            SourceDesc::Media { .. } => {
                let (pic, rotation) = picture?;
                let mut tex = self.picture_tex(&pic, !canvas.output.is_hdr());
                // A cut-out's matte makes a copy of its own.
                if let Some((m, invert)) = crate::reuse::matte_picture(&mut self.sources, clip) {
                    tex = self.apply_matte(&tex, &m, invert);
                    if rotation != 0 {
                        let r = self.rotate(&tex, rotation);
                        self.release(tex);
                        let k = crate::reuse::media_scale(&mut self.sources, clip, r.width);
                        return Some((r, k));
                    }
                    let k = crate::reuse::media_scale(&mut self.sources, clip, tex.width);
                    return Some((tex, k));
                }
                // The cached texture is copied by the crop step; rotation
                // produces its own copy already.
                if rotation != 0 {
                    let r = self.rotate(&tex, rotation);
                    let k = crate::reuse::media_scale(&mut self.sources, clip, r.width);
                        return Some((r, k));
                }
                // Copy so the layer owns a poolable texture.
                let own = self.acquire(tex.width, tex.height);
                self.copy_region(&tex, 0, 0, &own);
                let k = crate::reuse::media_scale(&mut self.sources, clip, own.width);
                Some((own, k))
            }
            SourceDesc::Generator(g) => {
                let frame = effects::generate(g, target.0, target.1);
                let tex = self.upload_frame(&frame);
                Some((tex, canvas.width as f32 / target.0 as f32))
            }
            // Models are drawn by the CPU rasteriser and uploaded.
            SourceDesc::Mesh(m) => {
                let frame = crate::mesh_render::render(m, canvas, target);
                let tex = self.upload_frame(&frame);
                Some((tex, canvas.width as f32 / target.0 as f32))
            }
            // Worlds are drawn by their own renderer and uploaded.
            SourceDesc::World(w) => {
                let frame = crate::world_scene::render(w, canvas, target);
                let tex = self.upload_frame(&frame);
                Some((tex, canvas.width as f32 / target.0 as f32))
            }
            SourceDesc::Nested { scene, time } => {
                if self.depth >= 8 {
                    return None;
                }
                let nested = scene.canvas();
                let s = target.0 as f32 / canvas.width.max(1) as f32;
                let size = (((nested.width as f32 * s).round() as u32).max(1), ((nested.height as f32 * s).round() as u32).max(1));
                self.depth += 1;
                let tex = self.render_scene_tex(scene.as_ref(), *time, size);
                self.depth -= 1;
                Some((tex, nested.width as f32 / size.0 as f32))
            }
            SourceDesc::Adjustment | SourceDesc::Camera(_) | SourceDesc::Light(_) => None,
        }
    }

    // ---- processing passes ----

    /// `src` multiplied by a cut-out matte (`matte.wgsl`), as a new texture.
    fn apply_matte(&mut self, src: &Tex, matte: &DecodedPicture, invert: bool) -> Tex {
        let ctx = self.ctx.clone();
        let out = self.acquire(src.width, src.height);
        let PixelData::Rgba8(px) = &matte.data else {
            self.copy_region(src, 0, 0, &out);
            return out;
        };
        let texture = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("matte"),
            size: wgpu::Extent3d { width: matte.width, height: matte.height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.write_texture(&texture, px, matte.width * 4, matte.width, matte.height, 1);
        let view = texture.create_view(&Default::default());
        let u = self.uniform(&MatteParams { size: [src.width, src.height], msize: [matte.width, matte.height], invert: u32::from(invert), pad: [0; 3] });
        self.run(&ctx.pipelines.matte, vec![wgpu::BindingResource::TextureView(&src.view), wgpu::BindingResource::TextureView(&out.view), u.as_entire_binding(), wgpu::BindingResource::TextureView(&view)], src.width, src.height);
        out
    }

    fn crop(&mut self, src: Tex, x: u32, y: u32, w: u32, h: u32) -> Tex {
        if x == 0 && y == 0 && w == src.width && h == src.height {
            return src;
        }
        let out = self.acquire(w.min(src.width - x.min(src.width)).max(1), h.min(src.height - y.min(src.height)).max(1));
        self.copy_region(&src, x.min(src.width - 1), y.min(src.height - 1), &out);
        self.release(src);
        out
    }

    #[cfg(feature = "fx")]
    fn fx_uniforms(&self, size: (u32, u32), time: f32, seed: u32, params: &[[f32; 4]]) -> wgpu::Buffer {
        let mut p = [[0.0f32; 4]; 16];
        for (d, s) in p.iter_mut().zip(params) {
            *d = *s;
        }
        self.uniform(&FxUniforms { size: [size.0 as f32, size.1 as f32], time, seed: f32::from_bits(seed), p })
    }

    /// Runs the effect stack: shader effects on the GPU, the rest on the
    /// CPU in runs (one round trip per run of CPU-only effects).
    fn apply_effects(&mut self, mut tex: Tex, list: &[EffectDesc], ctx: EffectCtx) -> Tex {
        let gpu = self.ctx.clone();
        let mut i = 0;
        while i < list.len() {
            #[cfg(feature = "fx")]
            if list[i].kind == crate::comp::KIND {
                tex = self.apply_comp(tex, &list[i], ctx);
                i += 1;
                continue;
            }
            #[cfg(feature = "fx")]
            if let Some((wgsl, params)) = effects::effect_shader(&list[i]) {
                let data = effects::effect_storage(&list[i], ctx, tex.width, tex.height);
                let bindings = if data.is_some() { EFFECT_DATA_BINDINGS } else { EFFECT_BINDINGS };
                let pipeline = gpu.pipelines.fx_pipeline(&gpu.device, &format!("effect:{}", list[i].kind), wgsl, bindings);
                let out = self.acquire(tex.width, tex.height);
                let u = self.fx_uniforms((tex.width, tex.height), ctx.time, ctx.seed, &params);
                let db = data.map(|d| self.storage(bytemuck::cast_slice(&d)));
                let mut resources = vec![wgpu::BindingResource::TextureView(&tex.view), wgpu::BindingResource::Sampler(&gpu.pipelines.sampler), wgpu::BindingResource::TextureView(&out.view), u.as_entire_binding()];
                if let Some(b) = &db {
                    resources.push(b.as_entire_binding());
                }
                self.run(&pipeline, resources, tex.width, tex.height);
                self.release(tex);
                tex = out;
                i += 1;
                continue;
            }
            // A run of effects without shaders.
            let start = i;
            while i < list.len() && !effects::effect_has_shader(&list[i].kind) && (i == start || list[i].kind != "comp") {
                i += 1;
            }
            let frame = self.read_frame(&tex);
            let processed = effects::apply_effects(frame, &list[start..i], ctx);
            self.release(tex);
            tex = self.upload_frame(&processed);
        }
        let _ = gpu;
        tex
    }

    fn lut_views(&mut self, lut: &studio_colour::LutSettings) -> (wgpu::TextureView, wgpu::TextureView) {
        let key = Arc::as_ptr(&lut.lut) as usize;
        if let Some(v) = self.luts.get(&key) {
            return v.clone();
        }
        let device = &self.ctx.device;
        let v3 = match &lut.lut.lut3d {
            Some(l) => {
                let n = l.size as u32;
                let t = device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("lut3d"),
                    size: wgpu::Extent3d { width: n, height: n, depth_or_array_layers: n },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D3,
                    format: wgpu::TextureFormat::Rgba32Float,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                });
                let texels = studio_colour::gpu::lut3d_texels(l);
                self.write_texture(&t, bytemuck::cast_slice(&texels), n * 16, n, n, n);
                t.create_view(&Default::default())
            }
            None => self.dummy_lut3d.clone(),
        };
        let v1 = match &lut.lut.lut1d {
            Some(l) => {
                let n = l.data.len() as u32;
                let t = device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("lut1d"),
                    size: wgpu::Extent3d { width: n, height: 1, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba32Float,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                });
                let texels = studio_colour::gpu::lut1d_texels(l);
                self.write_texture(&t, bytemuck::cast_slice(&texels), n * 16, n, 1, 1);
                t.create_view(&Default::default())
            }
            None => self.dummy_lut1d.clone(),
        };
        self.luts.insert(key, (v3.clone(), v1.clone()));
        (v3, v1)
    }

    /// Applies a grade: the per-pixel shader when it covers the whole
    /// grade, otherwise (dehaze, clarity, sharpen) the CPU implementation.
    fn apply_grade(&mut self, tex: Tex, desc: &GradeDesc, frame_index: u64) -> Tex {
        let grade = effects::grade_from_desc(desc);
        if grade.is_identity() {
            return tex;
        }
        let b = &grade.basic;
        if b.dehaze != 0.0 || b.clarity != 0.0 || b.sharpen.amount != 0.0 {
            let frame = self.read_frame(&tex);
            let out = effects::apply_grade(frame, desc, frame_index);
            self.release(tex);
            return self.upload_frame(&out);
        }
        self.run_grade(tex, &grade, frame_index)
    }

    /// Runs the per-pixel grade shader (no spatial tools).
    fn run_grade(&mut self, tex: Tex, grade: &studio_colour::Grade, frame_index: u64) -> Tex {
        let ctx = self.ctx.clone();
        let p = grade.prepare();
        let packed = studio_colour::gpu::pack_grade(grade, &p, tex.width, tex.height, studio_colour::FrameInfo { frame: frame_index });
        let u = ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("grade"), contents: bytemuck::cast_slice(&packed), usage: wgpu::BufferUsages::UNIFORM });
        let (v3, v1) = match &grade.lut {
            Some(l) => self.lut_views(l),
            None => (self.dummy_lut3d.clone(), self.dummy_lut1d.clone()),
        };
        let out = self.acquire(tex.width, tex.height);
        self.run(
            &ctx.pipelines.grade,
            vec![
                wgpu::BindingResource::TextureView(&tex.view),
                wgpu::BindingResource::TextureView(&out.view),
                u.as_entire_binding(),
                wgpu::BindingResource::TextureView(&v3),
                wgpu::BindingResource::TextureView(&v1),
            ],
            tex.width,
            tex.height,
        );
        self.release(tex);
        out
    }

    /// Packs masks for a layer of `width × height` pixels: the mask array,
    /// the polygon points and the accumulator start value.
    fn pack_masks(masks: &[MaskSpec], width: u32, height: u32) -> (Vec<MaskGpu>, Vec<[f32; 2]>, f32) {
        let mut out = Vec::with_capacity(masks.len());
        let mut poly: Vec<[f32; 2]> = Vec::new();
        for m in masks {
            let p = PreparedMask::new(m, width, height);
            let start = poly.len() as u32;
            let count = p.polygon.len() as u32;
            poly.extend_from_slice(&p.polygon);
            out.push(MaskGpu {
                kind: [m.shape.code(), m.mode as u32, m.invert as u32, start],
                geom: [p.centre_px[0], p.centre_px[1], p.half_px[0], p.half_px[1]],
                rot: [p.cos, p.sin, p.sigma_px, p.expansion_px],
                misc: [p.roundness_px, m.opacity, count as f32, 0.0],
            });
        }
        if poly.is_empty() {
            poly.push([0.0, 0.0]);
        }
        (out, poly, mask::initial(masks))
    }

    fn apply_masks(&mut self, tex: Tex, masks: &[MaskSpec]) -> Tex {
        if masks.is_empty() {
            return tex;
        }
        let ctx = self.ctx.clone();
        let (packed, poly, init) = Self::pack_masks(masks, tex.width, tex.height);
        let mb = self.storage(bytemuck::cast_slice(&packed));
        let pb = self.storage(bytemuck::cast_slice(&poly));
        let u = self.uniform(&MaskParams { size: [tex.width, tex.height], count: packed.len() as u32, init });
        let out = self.acquire(tex.width, tex.height);
        self.run(
            &ctx.pipelines.mask,
            vec![wgpu::BindingResource::TextureView(&tex.view), wgpu::BindingResource::TextureView(&out.view), u.as_entire_binding(), mb.as_entire_binding(), pb.as_entire_binding()],
            tex.width,
            tex.height,
        );
        self.release(tex);
        out
    }

    /// Bends a layer picture by its pins (`CpuCompositor::layer_picture`'s
    /// warp): occupancy on the GPU (a small read-back) when a puppet needs
    /// it, the mesh on the CPU, the drawing in `warp.wgsl`. Returns the
    /// picture and its new origin.
    fn warp(&mut self, tex: Tex, warp: &crate::puppet::WarpDesc) -> (Tex, [f32; 2]) {
        use crate::puppet;
        let ctx = self.ctx.clone();
        let occ = if warp.needs_occupancy() {
            let g = puppet::OccupancyGrid::for_size(tex.width, tex.height);
            let cells = self.acquire(g.nx, g.ny);
            let u = self.uniform(&CellParams { size: [tex.width, tex.height], grid: [g.nx, g.ny], cell: g.cell, pad: [0; 3] });
            self.run(&ctx.pipelines.cells, vec![wgpu::BindingResource::TextureView(&tex.view), wgpu::BindingResource::TextureView(&cells.view), u.as_entire_binding()], g.nx, g.ny);
            let f = self.read_frame(&cells);
            self.release(cells);
            Some(f.pixels.iter().map(|p| p[3]).collect::<Vec<f32>>())
        } else {
            None
        };
        let Some(mesh) = puppet::build_mesh(warp, tex.width, tex.height, occ.as_deref()) else {
            let out = self.acquire(1, 1);
            self.fill(&out, [0.0; 4]);
            self.release(tex);
            return (out, [0.0; 2]);
        };
        let (tiles_x, bins) = puppet::bin_tiles(&mesh);
        let (w, h) = mesh.size;
        let tile_count = tiles_x * h.div_ceil(puppet::TILE);
        let flat: Vec<f32> = if mesh.tris.is_empty() { vec![0.0; 16] } else { mesh.tris.iter().flatten().copied().collect() };
        let tb = self.storage(bytemuck::cast_slice(&flat));
        let bb = self.storage(bytemuck::cast_slice(&bins));
        let u = self.uniform(&WarpParams { size: [w, h], tiles_x, tile_count });
        let out = self.acquire(w, h);
        self.run(
            &ctx.pipelines.warp,
            vec![wgpu::BindingResource::TextureView(&tex.view), wgpu::BindingResource::TextureView(&out.view), u.as_entire_binding(), tb.as_entire_binding(), bb.as_entire_binding()],
            w,
            h,
        );
        self.release(tex);
        (out, mesh.origin)
    }

    /// `src` blurred, or `None` when the blur is too small to see.
    fn blur(&mut self, src: &Tex, sigma: f32) -> Option<Tex> {
        if sigma <= 0.05 {
            return None;
        }
        let ctx = self.ctx.clone();
        let kernel = gaussian_kernel(sigma);
        let r = (kernel.len() / 2) as u32;
        let kb = self.storage(bytemuck::cast_slice(&kernel));
        let mut cur: Option<Tex> = None;
        for dir in 0..2u32 {
            let from = cur.as_ref().unwrap_or(src).clone();
            let out = self.acquire(from.width, from.height);
            let u = self.uniform(&SizeParams { size: [from.width, from.height], a: dir, b: r });
            self.run(&ctx.pipelines.blur, vec![wgpu::BindingResource::TextureView(&from.view), wgpu::BindingResource::TextureView(&out.view), u.as_entire_binding(), kb.as_entire_binding()], from.width, from.height);
            if let Some(c) = cur.replace(out) {
                self.release(c);
            }
        }
        cur
    }

    fn half(&mut self, tex: &Tex) -> Tex {
        let ctx = self.ctx.clone();
        let (w, h) = ((tex.width / 2).max(1), (tex.height / 2).max(1));
        let out = self.acquire(w, h);
        let kernel = self.storage(&[0u8; 4]);
        let u = self.uniform(&SizeParams { size: [w, h], a: 0, b: 0 });
        self.run(&ctx.pipelines.half, vec![wgpu::BindingResource::TextureView(&tex.view), wgpu::BindingResource::TextureView(&out.view), u.as_entire_binding(), kernel.as_entire_binding()], w, h);
        out
    }

    /// Hands textures the layer cache let go of back to the pool, once
    /// nothing recorded this frame can still read them.
    fn retire(&mut self, gone: Vec<Tex>) {
        self.retired.extend(gone);
    }

    /// A layer's processed texture before placement (`CpuCompositor::content`):
    /// from the layer cache, or made and kept there. The second value is
    /// the cache entry; a kept texture belongs to the cache. A step added
    /// here must put what it reads into `ContentKey`.
    fn content(&mut self, clip: &ClipDesc, canvas: &Canvas, target: (u32, u32), t: Micros) -> Option<(Content<Tex>, Option<EntryId>)> {
        let picture = media_picture(&mut self.sources, clip);
        let key = ContentKey::of(clip, canvas, target, t, picture.as_ref());
        if let Some(key) = &key
            && let Some((c, id)) = self.cache.get(key)
        {
            return Some((c, Some(id)));
        }
        let (raw, k) = self.raw_picture(clip, canvas, target, picture)?;
        let source_size = [raw.width as f32 * k, raw.height as f32 * k];
        let crop_img = crop_rect([raw.width as f32, raw.height as f32], clip.placement.crop);
        let (cx, cy) = (crop_img[0].floor() as u32, crop_img[1].floor() as u32);
        let (cw, ch) = (((crop_img[2] - crop_img[0]).round() as u32).max(1), ((crop_img[3] - crop_img[1]).round() as u32).max(1));
        let mut tex = self.crop(raw, cx, cy, cw, ch);
        let rel = (t - clip.start).max(0);
        let ctx = EffectCtx { time: rel as f32 / SECOND as f32, frame_rate: canvas.frame_rate as f32, seed: clip.seed };
        if !clip.effects.is_empty() {
            tex = self.apply_effects(tex, &clip.effects, ctx);
        }
        if let Some(g) = &clip.grade {
            tex = self.apply_grade(tex, g, (t as f64 / SECOND as f64 * canvas.frame_rate).floor().max(0.0) as u64);
        }
        if !clip.grade_nodes.is_empty() {
            // Nodes run on the CPU (see `nodes`).
            let frame = self.read_frame(&tex);
            self.release(tex);
            let out = crate::nodes::apply_nodes(frame, &clip.grade_nodes, (t as f64 / SECOND as f64 * canvas.frame_rate).floor().max(0.0) as u64);
            tex = self.upload_frame(&out);
        }
        if !clip.masks.is_empty() {
            tex = self.apply_masks(tex, &clip.masks);
        }
        let mut origin = [0.0f32; 2];
        if let Some(w) = clip.warp.as_ref().filter(|w| !w.is_identity()) {
            (tex, origin) = self.warp(tex, w);
        }
        let content = Content { image: tex, k, origin, source_size };
        let Some(key) = key else { return Some((content, None)) };
        let (gone, id) = self.cache.insert(key, content.clone());
        self.retire(gone);
        Some((content, id))
    }

    /// Builds the processed, placed layer (`CpuCompositor::layer_picture`
    /// step for step).
    fn layer(&mut self, clip: &ClipDesc, pose: &Pose, canvas: &Canvas, target: (u32, u32), t: Micros, camera: &CameraDesc, lit: bool) -> Option<GpuLayer> {
        let s = target.0 as f32 / canvas.width.max(1) as f32;
        let (Content { image: mut tex, k, origin, source_size }, mut kept) = self.content(clip, canvas, target, t)?;
        let mut owned = kept.is_none();
        let m = layer_matrix(canvas.size(), source_size, &clip.placement, pose);
        let crop_src = crop_rect(source_size, clip.placement.crop);
        let to_source = Affine::translate(crop_src[0], crop_src[1]).then_after(&Affine::scale(k, k)).then_after(&Affine::translate(origin[0], origin[1]));
        let mut matrix = Affine::scale(s, s).then_after(&m).then_after(&to_source);
        let (tw, th) = (tex.width as f32, tex.height as f32);
        let mut proj = space::source_plane(canvas, source_size, clip, pose, camera).map(|(plane, cam)| {
            let plane = plane.then_after(&to_source);
            let h = Homography::from_affine(&Affine::scale(s, s)).then_after(&plane.homography(&cam, canvas.size()));
            let material = clip.space.map(|sp| sp.material).unwrap_or_default();
            let depth = space::plane_depth(&cam, &plane, [0.0, 0.0, tw, th]);
            let proj = Proj { h, plane, material, lit: lit && clip.space.is_some() && material.accepts_lights, depth };
            ProjLayer { proj, camera: cam, scale: s, canvas: canvas.size(), id: clip.id }
        });
        let centre = [tw * 0.5, th * 0.5];
        let local_scale = |matrix: &Affine, proj: &Option<ProjLayer>| match proj {
            Some(p) => p.proj.h.jacobian_at(centre).max_scale(),
            None => matrix.max_scale(),
        };
        let mut sigma_target = if pose.blur > 0.0 { pose.blur * 0.01 * target.1 as f32 * 0.5 } else { 0.0 };
        if let Some(p) = &proj
            && clip.space.is_some()
        {
            sigma_target = sigma_target.max(dof_sigma(&p.camera, p.proj.depth, s));
        }
        if sigma_target > 0.0 {
            let sigma_img = sigma_target / local_scale(&matrix, &proj).max(1e-6);
            if let Some(b) = self.blur(&tex, sigma_img) {
                if owned {
                    self.release(tex);
                }
                (tex, owned, kept) = (b, true, None);
            }
        }
        let mut level = 0;
        while local_scale(&matrix, &proj) < 0.5 && tex.width > 2 && tex.height > 2 {
            level += 1;
            let (pw, ph) = (tex.width as f32, tex.height as f32);
            // Minified versions of kept content are kept with it.
            let (next, next_owned) = match kept.and_then(|id| self.cache.mip(id, level)) {
                Some(m) => (m, false),
                None => {
                    let h = self.half(&tex);
                    match kept.and_then(|id| self.cache.add_mip(id, level, h.clone())) {
                        Some(gone) => {
                            self.retire(gone);
                            (h, false)
                        }
                        _ => {
                            kept = None;
                            (h, true)
                        }
                    }
                }
            };
            if owned {
                self.release(tex);
            }
            (tex, owned) = (next, next_owned);
            // An odd side loses its last pixel when halved: stretched by the
            // exact ratio, the picture still reaches its edge.
            let two = Affine::scale(pw / tex.width as f32, ph / tex.height as f32);
            matrix = matrix.then_after(&two);
            if let Some(p) = &mut proj {
                p.proj.h = p.proj.h.then_after_affine(&two);
                p.proj.plane = p.proj.plane.then_after(&two);
            }
        }
        let visible = visible_rect([tex.width as f32, tex.height as f32], [0.0; 4], pose.clip);
        let opacity = (clip.placement.opacity * pose.opacity).clamp(0.0, 1.0);
        Some(GpuLayer { tex, owned, matrix, visible, opacity, proj })
    }

    /// One composite pass: `layer` over the current canvas with `blend`,
    /// into the other canvas, then swap. `masks` (with their layer size)
    /// are only used by the adjustment mix.
    fn composite(&mut self, canvases: &mut Canvases, layer: &Tex, inv: &Affine, visible: [f32; 4], opacity: f32, blend: u32, masks: &[MaskSpec]) {
        let ctx = self.ctx.clone();
        let (w, h) = (canvases.cur.width, canvases.cur.height);
        let (packed, poly, init) = Self::pack_masks(masks, layer.width, layer.height);
        let mb = self.storage(if packed.is_empty() { &[0u8; 64] } else { bytemuck::cast_slice(&packed) });
        let pb = self.storage(bytemuck::cast_slice(&poly));
        let a = inv.to_array();
        let u = self.uniform(&CompParams {
            inv0: [a[0], a[1], a[2], 0.0],
            inv1: [a[3], a[4], a[5], 0.0],
            visible,
            canvas_size: [w, h],
            opacity,
            blend,
            mask_count: packed.len() as u32,
            mask_init: init,
            pad: [0; 2],
        });
        self.run(
            &ctx.pipelines.composite,
            vec![
                wgpu::BindingResource::TextureView(&canvases.cur.view),
                wgpu::BindingResource::TextureView(&canvases.other.view),
                wgpu::BindingResource::TextureView(&layer.view),
                u.as_entire_binding(),
                mb.as_entire_binding(),
                pb.as_entire_binding(),
            ],
            w,
            h,
        );
        std::mem::swap(&mut canvases.cur, &mut canvases.other);
    }

    /// One slice of a perspective layer (`CpuCompositor::draw_proj`).
    #[allow(clippy::too_many_arguments)]
    fn composite_proj(&mut self, canvases: &mut Canvases, layer: &GpuLayer, pl: &ProjLayer, d: f32, side: f32, blend: u32, lights: &[LightGpu], casters: &[(CasterGpu, Tex)], self_caster: u32) {
        let (h, plane) = pl.slice(d);
        let Some(inv) = h.inverse() else { return };
        let ctx = self.ctx.clone();
        let (w, hh) = (canvases.cur.width, canvases.cur.height);
        let lb = self.storage(if lights.is_empty() { &[0u8; 64] } else { bytemuck::cast_slice(lights) });
        let packed: Vec<CasterGpu> = casters.iter().map(|c| c.0).collect();
        let cb = self.storage(if packed.is_empty() { &[0u8; 80] } else { bytemuck::cast_slice(&packed) });
        let [i0, i1, i2] = inv.rows();
        let m = &pl.proj.material;
        let u = self.uniform(&Comp3dParams {
            inv0: i0,
            inv1: i1,
            inv2: i2,
            hw: [h.0[2][0], h.0[2][1], h.0[2][2], 0.0],
            origin: v4(plane.origin, 0.0),
            eu: v4(plane.eu, 0.0),
            ev: v4(plane.ev, 0.0),
            normal: v4(plane.normal(), 0.0),
            eye: v4(pl.camera.position, 0.0),
            mat: [m.ambient, m.diffuse, m.specular, m.shininess],
            visible: layer.visible,
            canvas_size: [w, hh],
            opacity: layer.opacity,
            blend,
            light_count: if pl.proj.lit { lights.len() as u32 } else { 0 },
            caster_count: packed.len() as u32,
            lit: pl.proj.lit as u32,
            side,
            accepts_shadows: m.accepts_shadows as u32,
            self_caster,
            near: NEAR,
            pad: 0,
        });
        let view = |i: usize| casters.get(i).map_or_else(|| self.dummy_caster.clone(), |c| c.1.view.clone());
        let (c0, c1, c2, c3) = (view(0), view(1), view(2), view(3));
        self.run(
            &ctx.pipelines.composite3d,
            vec![
                wgpu::BindingResource::TextureView(&canvases.cur.view),
                wgpu::BindingResource::TextureView(&canvases.other.view),
                wgpu::BindingResource::TextureView(&layer.tex.view),
                u.as_entire_binding(),
                lb.as_entire_binding(),
                cb.as_entire_binding(),
                wgpu::BindingResource::TextureView(&c0),
                wgpu::BindingResource::TextureView(&c1),
                wgpu::BindingResource::TextureView(&c2),
                wgpu::BindingResource::TextureView(&c3),
            ],
            w,
            hh,
        );
        std::mem::swap(&mut canvases.cur, &mut canvases.other);
    }

    /// Draws a perspective layer, its extruded slices far to near.
    fn draw_proj(&mut self, canvases: &mut Canvases, layer: &GpuLayer, pl: &ProjLayer, blend: u32, lights: &[LightGpu], casters: &[(CasterGpu, Tex)], self_caster: u32) {
        if layer.opacity <= 0.0 && !(17..=20).contains(&blend) {
            return;
        }
        for (d, side) in pl.slices(layer.visible) {
            self.composite_proj(canvases, layer, pl, d, side, blend, lights, casters, self_caster);
        }
    }

    /// A 2D layer's ground shadow under it (`CpuCompositor::draw_layer`):
    /// each pass drawn in `shadow.wgsl`, blurred and laid over the canvas.
    #[allow(clippy::too_many_arguments)]
    fn draw_shadow(&mut self, canvases: &mut Canvases, clip: &ClipDesc, pose: &Pose, canvas: &Canvas, target: (u32, u32), camera: &CameraDesc, layer: &GpuLayer, sh: &crate::shadow::ShadowDesc) {
        let ctx = self.ctx.clone();
        for p in crate::compositor::shadow_passes(&mut self.sources, clip, pose, canvas, target, camera, &layer.matrix, layer.visible, layer.opacity, sh) {
            let [x0, y0, w, h] = p.region;
            let out = self.acquire(w, h);
            let a = p.inv.to_array();
            let u = self.uniform(&ShadowParams {
                inv0: [a[0], a[1], a[2], 0.0],
                inv1: [a[3], a[4], a[5], 0.0],
                row_y: [p.row_y[0], p.row_y[1], p.row_y[2], p.feet_y],
                colour: [sh.colour[0], sh.colour[1], sh.colour[2], p.strength],
                visible: layer.visible,
                origin: [x0 as f32, y0 as f32],
                fade: p.fade,
                gain: p.gain,
                size: [w, h],
                pad1: [0; 2],
            });
            self.run(&ctx.pipelines.shadow, vec![wgpu::BindingResource::TextureView(&layer.tex.view), wgpu::BindingResource::TextureView(&out.view), u.as_entire_binding()], w, h);
            let shape = match self.blur(&out, p.sigma) {
                Some(b) => {
                    self.release(out);
                    b
                }
                None => out,
            };
            let inv = Affine::translate(-(x0 as f32), -(y0 as f32));
            self.composite(canvases, &shape, &inv, [0.0, 0.0, w as f32, h as f32], 1.0, crate::blend::BlendMode::Normal as u32, &[]);
            self.release(shape);
        }
    }

    fn draw_layer(&mut self, canvases: &mut Canvases, clip: &ClipDesc, pose: &Pose, canvas: &Canvas, target: (u32, u32), t: Micros, camera: &CameraDesc) {
        let Some(layer) = self.layer(clip, pose, canvas, target, t, camera, false) else { return };
        if layer.proj.is_none()
            && let Some(sh) = &clip.shadow
        {
            self.draw_shadow(canvases, clip, pose, canvas, target, camera, &layer, sh);
        }
        if let Some(pl) = layer.proj {
            self.draw_proj(canvases, &layer, &pl, clip.blend as u32, &[], &[], 99);
        } else if layer.opacity > 0.0
            && let Some(inv) = layer.matrix.inverse()
        {
            self.composite(canvases, &layer.tex, &inv, layer.visible, layer.opacity, clip.blend as u32, &[]);
        }
        if layer.owned {
            self.release(layer.tex);
        }
    }

    /// A run of 3D layers (`CpuCompositor::draw_run`).
    fn draw_run(&mut self, canvases: &mut Canvases, run: &[(&ClipDesc, &Pose)], canvas: &Canvas, target: (u32, u32), t: Micros, camera: &CameraDesc, lights: &[LightDesc]) {
        let lit = !lights.is_empty();
        let mut layers: Vec<(GpuLayer, u32)> = run.iter().filter_map(|(c, p)| self.layer(c, p, canvas, target, t, camera, lit).map(|l| (l, c.blend as u32))).collect();
        layers.sort_by(|a, b| {
            let d = |l: &GpuLayer| l.proj.map_or(f32::NEG_INFINITY, |p| p.proj.depth);
            d(&b.0).partial_cmp(&d(&a.0)).unwrap_or(std::cmp::Ordering::Equal)
        });
        let lights_gpu: Vec<LightGpu> = lights
            .iter()
            .map(|l| LightGpu {
                pos: v4(l.position, l.kind as u32 as f32),
                dir: v4(l.direction, if l.casts_shadows { 1.0 } else { 0.0 }),
                colour: v4(l.colour, l.shadow_darkness),
                cone: [l.cone[0], l.cone[1], l.radius, l.falloff],
            })
            .collect();
        let mut casters: Vec<(CasterGpu, Tex)> = Vec::new();
        let mut caster_of: Vec<u32> = vec![99; layers.len()];
        if lit && lights.iter().any(|l| l.casts_shadows) {
            for (i, (l, _)) in layers.iter().enumerate() {
                if let Some(pl) = &l.proj
                    && pl.proj.material.casts_shadows
                    && casters.len() < space::MAX_CASTERS
                    && let Some(c) = Caster::new(&pl.proj.plane, l.visible, pl.id)
                {
                    caster_of[i] = casters.len() as u32;
                    casters.push((CasterGpu { origin: v4(c.origin, 0.0), du: v4(c.du, 0.0), dv: v4(c.dv, 0.0), normal: v4(c.normal, 0.0), visible: c.visible }, l.tex.clone()));
                }
            }
        }
        for (i, (l, blend)) in layers.iter().enumerate() {
            match &l.proj {
                Some(pl) => self.draw_proj(canvases, l, pl, *blend, &lights_gpu, &casters, caster_of[i]),
                None => {
                    if l.opacity > 0.0
                        && let Some(inv) = l.matrix.inverse()
                    {
                        self.composite(canvases, &l.tex, &inv, l.visible, l.opacity, *blend, &[]);
                    }
                }
            }
        }
        drop(casters);
        for (l, _) in layers {
            if l.owned {
                self.release(l.tex);
            }
        }
    }

    fn apply_adjustment(&mut self, canvases: &mut Canvases, clip: &ClipDesc, canvas: &Canvas, t: Micros) {
        let rel = (t - clip.start).max(0);
        let ctx = EffectCtx { time: rel as f32 / SECOND as f32, frame_rate: canvas.frame_rate as f32, seed: clip.seed };
        let mut processed = self.acquire(canvases.cur.width, canvases.cur.height);
        self.copy_region(&canvases.cur, 0, 0, &processed);
        if !clip.effects.is_empty() {
            processed = self.apply_effects(processed, &clip.effects, ctx);
        }
        if let Some(g) = &clip.grade {
            processed = self.apply_grade(processed, g, (t as f64 / SECOND as f64 * canvas.frame_rate).floor().max(0.0) as u64);
        }
        if !clip.grade_nodes.is_empty() {
            let frame = self.read_frame(&processed);
            self.release(processed);
            let out = crate::nodes::apply_nodes(frame, &clip.grade_nodes, (t as f64 / SECOND as f64 * canvas.frame_rate).floor().max(0.0) as u64);
            processed = self.upload_frame(&out);
        }
        let opacity = clip.placement.opacity.clamp(0.0, 1.0);
        let full = [0.0, 0.0, processed.width as f32, processed.height as f32];
        self.composite(canvases, &processed, &Affine::IDENTITY, full, opacity, BLEND_ADJUST, &clip.masks);
        self.release(processed);
    }

    /// Renders one clip alone on a transparent canvas (a transition side).
    fn render_alone(&mut self, clip: &ClipDesc, pose: &Pose, canvas: &Canvas, target: (u32, u32), t: Micros, camera: &CameraDesc) -> Tex {
        let cur = self.acquire(target.0, target.1);
        let other = self.acquire(target.0, target.1);
        self.fill(&cur, [0.0; 4]);
        let mut c = Canvases { cur, other };
        self.draw_layer(&mut c, clip, pose, canvas, target, t, camera);
        self.release(c.other);
        c.cur
    }

    /// Combines two transition sides at `progress`: the `fx` shader when
    /// there is one, the CPU transition otherwise.
    fn transition(&mut self, kind: &str, a: Tex, b: Tex, progress: f32, params: &crate::scene::Params, seed: u32) -> Tex {
        let progress = progress.clamp(0.0, 1.0);
        if progress <= 0.0 {
            self.release(b);
            return a;
        }
        if progress >= 1.0 {
            self.release(a);
            return b;
        }
        #[cfg(feature = "fx")]
        if let Some((wgsl, packed)) = effects::transition_shader(kind, params) {
            let ctx = self.ctx.clone();
            let pipeline = ctx.pipelines.fx_pipeline(&ctx.device, &format!("transition:{kind}"), wgsl, TRANSITION_BINDINGS);
            let out = self.acquire(a.width, a.height);
            let u = self.fx_uniforms((a.width, a.height), progress, seed, &packed);
            self.run(
                &pipeline,
                vec![
                    wgpu::BindingResource::TextureView(&a.view),
                    wgpu::BindingResource::TextureView(&b.view),
                    wgpu::BindingResource::Sampler(&ctx.pipelines.sampler),
                    wgpu::BindingResource::TextureView(&out.view),
                    u.as_entire_binding(),
                ],
                a.width,
                a.height,
            );
            self.release(a);
            self.release(b);
            return out;
        }
        let fa = self.read_frame(&a);
        let fb = self.read_frame(&b);
        self.release(a);
        self.release(b);
        let mixed = effects::transition(kind, fa, fb, progress, params, seed);
        self.upload_frame(&mixed)
    }

    /// Draws a resolved list into a new canvas texture.
    fn render_list_tex(&mut self, list: &RenderList, target: (u32, u32)) -> Tex {
        let canvas = &list.canvas;
        let t = list.time;
        let cur = self.acquire(target.0, target.1);
        let other = self.acquire(target.0, target.1);
        self.fill(&cur, canvas.background);
        let mut c = Canvases { cur, other };
        let camera = list.camera();
        let mut i = 0;
        while i < list.nodes.len() {
            if in_space(&list.nodes[i]) {
                let mut run = Vec::new();
                while i < list.nodes.len() && in_space(&list.nodes[i]) {
                    if let Node::Layer { clip, pose } = &list.nodes[i] {
                        run.push((clip, pose));
                    }
                    i += 1;
                }
                self.draw_run(&mut c, &run, canvas, target, t, &camera, &list.lights);
                continue;
            }
            let node = &list.nodes[i];
            i += 1;
            match node {
                Node::Layer { clip, pose } => self.draw_layer(&mut c, clip, pose, canvas, target, t, &camera),
                Node::Adjustment { clip } => self.apply_adjustment(&mut c, clip, canvas, t),
                Node::Transition { desc, left_pose, right_pose } => {
                    let a = self.render_alone(&desc.left, left_pose, canvas, target, t, &camera);
                    let b = self.render_alone(&desc.right, right_pose, canvas, target, t, &camera);
                    let mixed = self.transition(&desc.kind, a, b, desc.progress, &desc.params, desc.seed);
                    let full = [0.0, 0.0, target.0 as f32, target.1 as f32];
                    self.composite(&mut c, &mixed, &Affine::IDENTITY, full, 1.0, BlendMode::Normal as u32, &[]);
                    self.release(mixed);
                }
            }
        }
        self.release(c.other);
        c.cur
    }

    /// Renders a scene at a time and size to a texture.
    fn render_scene_tex(&mut self, scene: &dyn Scene, t: Micros, size: (u32, u32)) -> Tex {
        let list = resolve(scene, t);
        self.render_list_tex(&list, size)
    }

    /// The view transform for an output space, as the last colour step.
    fn apply_view(&mut self, tex: Tex, view: crate::view::ViewTransform, space: crate::output::OutputSpace) -> Tex {
        match crate::view::view_grade(view, space) {
            Some(g) => self.run_grade(tex, &g, 0),
            None => tex,
        }
    }

    /// Output conversion on the GPU, read back as encoded pixels.
    fn encode(&mut self, src: &Tex, settings: &OutputSettings, depth: OutputDepth, frame_index: u32) -> EncodedFrame {
        let ctx = self.ctx.clone();
        let (w, h) = (src.width, src.height);
        let (format, pipeline, bpp) = match depth {
            OutputDepth::Eight => (wgpu::TextureFormat::Rgba8Unorm, &ctx.pipelines.output8, 4),
            OutputDepth::Sixteen => (wgpu::TextureFormat::Rgba16Uint, &ctx.pipelines.output16, 8),
        };
        let out = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("output"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = out.create_view(&Default::default());
        let m = settings.space.from_working();
        let has_matrix = m != studio_colour::math::Mat3::IDENTITY;
        let mf = m.to_f32();
        let u = self.uniform(&OutParams {
            size: [w, h],
            space: settings.space.shader_id(),
            premultiplied: settings.premultiplied as u32,
            dither: settings.dither as u32,
            frame: frame_index,
            has_matrix: has_matrix as u32,
            pad: 0,
            m: std::array::from_fn(|r| [mf[r][0], mf[r][1], mf[r][2], 0.0]),
        });
        self.run(pipeline, vec![wgpu::BindingResource::TextureView(&src.view), wgpu::BindingResource::TextureView(&view), u.as_entire_binding()], w, h);
        let bytes = self.read_bytes(&out, w, h, bpp);
        let pixels = match depth {
            OutputDepth::Eight => EncodedPixels::Rgba8(bytes),
            OutputDepth::Sixteen => EncodedPixels::Rgba16(bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect()),
        };
        EncodedFrame { width: w, height: h, pixels }
    }
}

impl Sized2d for Tex {
    fn bytes(&self) -> usize {
        self.width as usize * self.height as usize * 8
    }
}

impl FrameRenderer for GpuCompositor {
    fn render(&mut self, scene: &dyn Scene, t: Micros, target: Option<(u32, u32)>) -> Frame {
        self.begin_frame();
        let canvas = scene.canvas();
        let size = target_size(&canvas, target);
        let tex = self.render_scene_tex(scene, t, size);
        let tex = self.apply_view(tex, canvas.view, canvas.output);
        let frame = self.read_frame(&tex);
        self.release(tex);
        frame
    }

    fn render_encoded(&mut self, scene: &dyn Scene, t: Micros, target: Option<(u32, u32)>, settings: &OutputSettings, depth: OutputDepth, frame_index: u32) -> EncodedFrame {
        self.begin_frame();
        let canvas = scene.canvas();
        let size = crate::compositor::output_size(&canvas, target, settings);
        let tex = self.render_scene_tex(scene, t, size);
        let tex = self.apply_view(tex, canvas.view, settings.space);
        let out = self.encode(&tex, settings, depth, frame_index);
        self.release(tex);
        out
    }

    fn sources(&mut self) -> &mut Sources {
        &mut self.sources
    }

    fn name(&self) -> &'static str {
        "gpu"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shader_modules_have_names_dxc_can_take_as_files() {
        assert_eq!(module_name("comp:grade"), "comp_grade");
        assert_eq!(module_name("transition:dip-to-black"), "transition_dip_to_black");
        assert_eq!(module_name("output8"), "output8");
    }

    #[test]
    fn uniform_sizes_match_wgsl() {
        assert_eq!(std::mem::size_of::<ConvertParams>(), 112);
        assert_eq!(std::mem::size_of::<SizeParams>(), 16);
        assert_eq!(std::mem::size_of::<MaskParams>(), 16);
        assert_eq!(std::mem::size_of::<FillParams>(), 32);
        assert_eq!(std::mem::size_of::<CompParams>(), 80);
        assert_eq!(std::mem::size_of::<Comp3dParams>(), 224);
        assert_eq!(std::mem::size_of::<LightGpu>(), 64);
        assert_eq!(std::mem::size_of::<CasterGpu>(), 80);
        assert_eq!(std::mem::size_of::<OutParams>(), 80);
        assert_eq!(std::mem::size_of::<FxUniforms>(), 272);
        assert_eq!(std::mem::size_of::<MaskGpu>(), 64);
        assert_eq!(std::mem::size_of::<CellParams>(), 32);
        assert_eq!(std::mem::size_of::<WarpParams>(), 16);
        assert_eq!(std::mem::size_of::<MatteParams>(), 32);
        assert_eq!(std::mem::size_of::<ShadowParams>(), 112);
    }
}
