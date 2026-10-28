// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The pipelines of the real-time renderer: shadow, depth/normal prepass,
//! lit terrain and model passes, the sky, water, ambient occlusion and the
//! post passes, with the bind group layouts they share.

use std::sync::Arc;

pub const HDR: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
pub const DEPTH: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
pub const NORMAL: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
pub const AO: wgpu::TextureFormat = wgpu::TextureFormat::R8Unorm;
pub const LDR: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

const COMMON: &str = include_str!("shaders/common.wgsl");
const LIGHTING: &str = include_str!("shaders/lighting.wgsl");
const MESH: &str = include_str!("shaders/mesh.wgsl");
const TERRAIN: &str = include_str!("shaders/terrain.wgsl");
const SKY: &str = include_str!("shaders/sky.wgsl");
const WATER: &str = include_str!("shaders/water.wgsl");
const SSAO: &str = include_str!("shaders/ssao.wgsl");
const POST: &str = include_str!("shaders/post.wgsl");

/// Each shader module's complete source, for validation in the tests.
pub fn sources() -> Vec<(&'static str, String)> {
    vec![
        ("mesh", format!("{COMMON}\n{LIGHTING}\n{MESH}")),
        ("terrain", format!("{COMMON}\n{LIGHTING}\n{TERRAIN}")),
        ("sky", format!("{COMMON}\n{LIGHTING}\n{SKY}")),
        ("water", format!("{COMMON}\n{LIGHTING}\n{WATER}")),
        ("ssao", format!("{COMMON}\n{SSAO}")),
        ("post", POST.to_string()),
    ]
}

pub struct Layouts {
    pub min: wgpu::BindGroupLayout,
    pub full: wgpu::BindGroupLayout,
    pub mat: wgpu::BindGroupLayout,
    pub terrain: wgpu::BindGroupLayout,
    pub water: wgpu::BindGroupLayout,
    pub ssao: wgpu::BindGroupLayout,
    pub post: wgpu::BindGroupLayout,
}

pub struct Pipes {
    pub layouts: Layouts,
    pub shadow_mesh: wgpu::RenderPipeline,
    pub shadow_terrain: wgpu::RenderPipeline,
    pub pre_mesh: wgpu::RenderPipeline,
    pub pre_terrain: wgpu::RenderPipeline,
    pub main_mesh: wgpu::RenderPipeline,
    pub main_terrain: wgpu::RenderPipeline,
    pub refl_mesh: wgpu::RenderPipeline,
    pub refl_terrain: wgpu::RenderPipeline,
    pub sky: wgpu::RenderPipeline,
    pub water: wgpu::RenderPipeline,
    pub ssao: wgpu::RenderPipeline,
    pub ssao_blur: wgpu::RenderPipeline,
    pub prefilter: wgpu::RenderPipeline,
    pub down: wgpu::RenderPipeline,
    pub up: wgpu::RenderPipeline,
    pub accum: wgpu::RenderPipeline,
    pub tonemap: wgpu::RenderPipeline,
    pub encode: wgpu::RenderPipeline,
    pub lin_samp: wgpu::Sampler,
    pub shadow_samp: wgpu::Sampler,
    pub post_samp: wgpu::Sampler,
    pub tex_samp: wgpu::Sampler,
}

fn entry(binding: u32, vis: wgpu::ShaderStages, ty: wgpu::BindingType) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry { binding, visibility: vis, ty, count: None }
}

fn uniform(binding: u32, vis: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    entry(binding, vis, wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None })
}

fn tex(binding: u32, vis: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    entry(binding, vis, wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false })
}

fn depth_tex(binding: u32, vis: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    entry(binding, vis, wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Depth, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false })
}

fn samp(binding: u32, vis: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    entry(binding, vis, wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering))
}

const VF: wgpu::ShaderStages = wgpu::ShaderStages::VERTEX_FRAGMENT;
const F: wgpu::ShaderStages = wgpu::ShaderStages::FRAGMENT;

impl Layouts {
    fn new(d: &wgpu::Device) -> Layouts {
        let bgl = |label: &str, entries: &[wgpu::BindGroupLayoutEntry]| d.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some(label), entries });
        Layouts {
            min: bgl("rt min", &[uniform(0, VF)]),
            full: bgl(
                "rt full",
                &[
                    uniform(0, VF),
                    entry(1, F, wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Depth, view_dimension: wgpu::TextureViewDimension::D2Array, multisampled: false }),
                    entry(2, F, wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison)),
                    tex(3, VF),
                    samp(4, VF),
                    tex(5, F),
                    tex(6, F),
                ],
            ),
            mat: bgl("rt material", &[uniform(0, VF), tex(1, F), samp(2, F)]),
            terrain: bgl("rt terrain", &[tex(0, F), uniform(1, F)]),
            water: bgl("rt water", &[uniform(0, VF), depth_tex(1, F), tex(2, F)]),
            ssao: bgl("rt ssao", &[depth_tex(0, F), tex(1, F), tex(2, F)]),
            post: bgl("rt post", &[uniform(0, F), tex(1, F), tex(2, F), samp(3, F)]),
        }
    }
}

fn module(d: &wgpu::Device, name: &str, src: &str) -> wgpu::ShaderModule {
    d.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some(name), source: wgpu::ShaderSource::Wgsl(src.into()) })
}

const MESH_ATTRS: [wgpu::VertexAttribute; 3] = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2];
const INST_ATTRS: [wgpu::VertexAttribute; 2] = wgpu::vertex_attr_array![3 => Float32x4, 4 => Float32x4];
const TERRAIN_ATTRS: [wgpu::VertexAttribute; 2] = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3];
const WATER_ATTRS: [wgpu::VertexAttribute; 1] = wgpu::vertex_attr_array![0 => Float32x2];

pub fn mesh_buffers() -> [Option<wgpu::VertexBufferLayout<'static>>; 2] {
    [
        Some(wgpu::VertexBufferLayout { array_stride: 32, step_mode: wgpu::VertexStepMode::Vertex, attributes: &MESH_ATTRS }),
        Some(wgpu::VertexBufferLayout { array_stride: 32, step_mode: wgpu::VertexStepMode::Instance, attributes: &INST_ATTRS }),
    ]
}

fn terrain_buffers() -> [Option<wgpu::VertexBufferLayout<'static>>; 1] {
    [Some(wgpu::VertexBufferLayout { array_stride: 24, step_mode: wgpu::VertexStepMode::Vertex, attributes: &TERRAIN_ATTRS })]
}

fn water_buffers() -> [Option<wgpu::VertexBufferLayout<'static>>; 1] {
    [Some(wgpu::VertexBufferLayout { array_stride: 8, step_mode: wgpu::VertexStepMode::Vertex, attributes: &WATER_ATTRS })]
}

impl Pipes {
    pub fn new(ctx: &Arc<crate::gpu::GpuContext>) -> Result<Pipes, String> {
        let d = &ctx.device;
        let _scopes = ctx.scopes();
        let scope = d.push_error_scope(wgpu::ErrorFilter::Validation);
        let layouts = Layouts::new(d);
        let pl = |label: &str, groups: &[&wgpu::BindGroupLayout]| {
            let v: Vec<Option<&wgpu::BindGroupLayout>> = groups.iter().map(|g| Some(*g)).collect();
            d.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some(label), bind_group_layouts: &v, immediate_size: 0 })
        };
        let l_mesh_full = pl("mesh full", &[&layouts.full, &layouts.mat]);
        let l_mesh_min = pl("mesh min", &[&layouts.min, &layouts.mat]);
        let l_terrain_full = pl("terrain full", &[&layouts.full, &layouts.terrain]);
        let l_terrain_min = pl("terrain min", &[&layouts.min]);
        let l_sky = pl("sky", &[&layouts.full]);
        let l_water = pl("water", &[&layouts.full, &layouts.water]);
        let l_ssao = pl("ssao", &[&layouts.min, &layouts.ssao]);
        let l_post = pl("post", &[&layouts.post]);
        let m_mesh = module(d, "rt mesh", &format!("{COMMON}\n{LIGHTING}\n{MESH}"));
        let m_terrain = module(d, "rt terrain", &format!("{COMMON}\n{LIGHTING}\n{TERRAIN}"));
        let m_sky = module(d, "rt sky", &format!("{COMMON}\n{LIGHTING}\n{SKY}"));
        let m_water = module(d, "rt water", &format!("{COMMON}\n{LIGHTING}\n{WATER}"));
        let m_ssao = module(d, "rt ssao", &format!("{COMMON}\n{SSAO}"));
        let m_post = module(d, "rt post", POST);
        let mb = mesh_buffers();
        let tb = terrain_buffers();
        let wb = water_buffers();
        let no_bias = wgpu::DepthBiasState::default();
        let shadow_bias = wgpu::DepthBiasState { constant: 2, slope_scale: 2.0, clamp: 0.0 };
        use wgpu::CompareFunction as C;
        let colour = |format: wgpu::TextureFormat| [Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })];
        let full = |label: &str, layout: &wgpu::PipelineLayout, m: &wgpu::ShaderModule, vs: &str, fs: &str, buffers: &[Option<wgpu::VertexBufferLayout>], format: wgpu::TextureFormat, depth: Option<(bool, C, wgpu::DepthBiasState)>| {
            let t = colour(format);
            d.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(layout),
                vertex: wgpu::VertexState { module: m, entry_point: Some(vs), buffers, compilation_options: Default::default() },
                primitive: wgpu::PrimitiveState { front_face: wgpu::FrontFace::Cw, cull_mode: None, ..Default::default() },
                depth_stencil: depth.map(|(write, cmp, bias)| wgpu::DepthStencilState { format: DEPTH, depth_write_enabled: Some(write), depth_compare: Some(cmp), stencil: Default::default(), bias }),
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState { module: m, entry_point: Some(fs), targets: &t, compilation_options: Default::default() }),
                multiview_mask: None,
                cache: None,
            })
        };
        let depth_only = |label: &str, layout: &wgpu::PipelineLayout, m: &wgpu::ShaderModule, vs: &str, buffers: &[Option<wgpu::VertexBufferLayout>]| {
            d.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(layout),
                vertex: wgpu::VertexState { module: m, entry_point: Some(vs), buffers, compilation_options: Default::default() },
                primitive: wgpu::PrimitiveState { front_face: wgpu::FrontFace::Cw, cull_mode: None, ..Default::default() },
                depth_stencil: Some(wgpu::DepthStencilState { format: DEPTH, depth_write_enabled: Some(true), depth_compare: Some(C::Less), stencil: Default::default(), bias: shadow_bias }),
                multisample: wgpu::MultisampleState::default(),
                fragment: None,
                multiview_mask: None,
                cache: None,
            })
        };
        let post = |label: &str, fs: &str, format: wgpu::TextureFormat| full(label, &l_post, &m_post, "vs_main", fs, &[], format, None);
        let pipes = Pipes {
            shadow_mesh: depth_only("shadow mesh", &l_mesh_min, &m_mesh, "vs_shadow", &mb),
            shadow_terrain: depth_only("shadow terrain", &l_terrain_min, &m_terrain, "vs_shadow", &tb),
            pre_mesh: full("pre mesh", &l_mesh_min, &m_mesh, "vs_main", "fs_pre", &mb, NORMAL, Some((true, C::Less, no_bias))),
            pre_terrain: full("pre terrain", &l_terrain_min, &m_terrain, "vs_main", "fs_pre", &tb, NORMAL, Some((true, C::Less, no_bias))),
            main_mesh: full("main mesh", &l_mesh_full, &m_mesh, "vs_main", "fs_main", &mb, HDR, Some((false, C::LessEqual, no_bias))),
            main_terrain: full("main terrain", &l_terrain_full, &m_terrain, "vs_main", "fs_main", &tb, HDR, Some((false, C::LessEqual, no_bias))),
            refl_mesh: full("refl mesh", &l_mesh_full, &m_mesh, "vs_main", "fs_main", &mb, HDR, Some((true, C::Less, no_bias))),
            refl_terrain: full("refl terrain", &l_terrain_full, &m_terrain, "vs_main", "fs_main", &tb, HDR, Some((true, C::Less, no_bias))),
            sky: full("sky", &l_sky, &m_sky, "vs_main", "fs_main", &[], HDR, Some((false, C::LessEqual, no_bias))),
            water: full("water", &l_water, &m_water, "vs_main", "fs_main", &wb, HDR, Some((false, C::LessEqual, no_bias))),
            ssao: full("ssao", &l_ssao, &m_ssao, "vs_main", "fs_ao", &[], AO, None),
            ssao_blur: full("ssao blur", &l_ssao, &m_ssao, "vs_main", "fs_blur", &[], AO, None),
            prefilter: post("prefilter", "fs_prefilter", HDR),
            down: post("down", "fs_down", HDR),
            up: post("up", "fs_up", HDR),
            accum: post("accum", "fs_accum", HDR),
            tonemap: post("tonemap", "fs_tonemap", HDR),
            encode: post("encode", "fs_encode", LDR),
            lin_samp: d.create_sampler(&wgpu::SamplerDescriptor { address_mode_u: wgpu::AddressMode::Repeat, address_mode_v: wgpu::AddressMode::ClampToEdge, mag_filter: wgpu::FilterMode::Linear, min_filter: wgpu::FilterMode::Linear, mipmap_filter: wgpu::MipmapFilterMode::Linear, ..Default::default() }),
            shadow_samp: d.create_sampler(&wgpu::SamplerDescriptor { address_mode_u: wgpu::AddressMode::ClampToEdge, address_mode_v: wgpu::AddressMode::ClampToEdge, mag_filter: wgpu::FilterMode::Linear, min_filter: wgpu::FilterMode::Linear, compare: Some(C::LessEqual), ..Default::default() }),
            post_samp: d.create_sampler(&wgpu::SamplerDescriptor { address_mode_u: wgpu::AddressMode::ClampToEdge, address_mode_v: wgpu::AddressMode::ClampToEdge, mag_filter: wgpu::FilterMode::Linear, min_filter: wgpu::FilterMode::Linear, ..Default::default() }),
            tex_samp: d.create_sampler(&wgpu::SamplerDescriptor { address_mode_u: wgpu::AddressMode::Repeat, address_mode_v: wgpu::AddressMode::Repeat, mag_filter: wgpu::FilterMode::Linear, min_filter: wgpu::FilterMode::Linear, mipmap_filter: wgpu::MipmapFilterMode::Linear, anisotropy_clamp: 4, ..Default::default() }),
            layouts,
        };
        if let Some(e) = pollster::block_on(scope.pop()) {
            return Err(format!("real-time renderer shaders: {e}"));
        }
        Ok(pipes)
    }
}
