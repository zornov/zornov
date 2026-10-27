// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shapes drawn straight from their formula, with no mesh: each pixel's
//! ray is marched through the shape's distance field (sphere tracing)
//! until it meets the surface, with a hit tolerance of about half a pixel
//! where it lands, so a gyroid or a Mandelbulb keeps its detail at any
//! size on screen. What it finds (the surface's normal and how far along
//! the ray) goes into the model pass in place of a triangle, so lights,
//! shadow maps, haze, depth and the air pass treat it like any model.
//!
//! The field's formula is written out as WGSL
//! ([`studio_mesh::math::field::Field::wgsl`]) and marched by a compute
//! pass on a device of its own; without a GPU (or with
//! `STUDIO_RENDER_NO_GPU` set) the CPU marches the same formula.
//!
//! A traced model still has its mesh (a coarse one, made with it): that is
//! what casts its shadows, what physics, rigs, booleans and exports use,
//! and what path-traced renders draw.

use crate::space::{V3, add, cross, dot, length, normalise, scale, sub};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use studio_mesh::math::field::Field;
use studio_mesh::sdf::FieldFn;
use wgpu::util::DeviceExt;

/// Most steps a ray takes.
pub const MAX_STEPS: u32 = 256;

/// A shape drawn from its formula.
pub struct Traced {
    pub field: Arc<Field>,
    /// The part's size, px (the field fills a box this big).
    pub size: V3,
    /// Seconds, for formulas that move.
    pub t: f32,
    /// What it is (its recipe), for caches and the GPU's pipelines.
    pub key: String,
}

impl std::fmt::Debug for Traced {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Traced({} chars, t {})", self.key.len(), self.t)
    }
}

/// A shape from its recipe ([`studio_mesh::math::field::Recipe`] as
/// text) at clip time `t`; `None` for no recipe or a bad one. Fields are
/// kept once made (finding their scale costs a few thousand evaluations).
pub fn from_recipe(text: &str, t: f32) -> Option<Arc<Traced>> {
    if text.is_empty() {
        return None;
    }
    type Made = Mutex<HashMap<String, Option<(Arc<Field>, V3)>>>;
    static MADE: OnceLock<Made> = OnceLock::new();
    let mut made = MADE.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    if made.len() > 128 {
        made.clear();
    }
    let got = made
        .entry(text.to_owned())
        .or_insert_with(|| {
            let r = studio_mesh::math::field::Recipe::from_text(text).ok()?;
            Some((Arc::new(r.field(0.0).ok()?), r.size))
        })
        .clone()?;
    Some(Arc::new(Traced { field: got.0, size: got.1, t, key: text.to_owned() }))
}

/// How the picture is seen: the lens, its axes and zoom (px per unit at
/// distance 1), the picture's centre.
#[derive(Clone, Copy, Debug)]
pub struct Lens {
    pub position: V3,
    pub axes: [V3; 3],
    pub zoom: f32,
    pub centre: [f32; 2],
}

impl Lens {
    pub fn dir(&self, x: f32, y: f32) -> V3 {
        let u = (x - self.centre[0]) / self.zoom;
        let v = (y - self.centre[1]) / self.zoom;
        normalise(add(add(scale(self.axes[0], u), scale(self.axes[1], v)), self.axes[2]))
    }
}

/// What one trace cost.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Stats {
    pub pixels: usize,
    pub hits: usize,
    pub gpu: bool,
    pub millis: f64,
}

/// The last traces' costs, for anyone measuring.
pub fn last_stats() -> Stats {
    *stats().lock().unwrap_or_else(|e| e.into_inner())
}

fn stats() -> &'static Mutex<Stats> {
    static S: OnceLock<Mutex<Stats>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(Stats::default()))
}

/// World → model (the inverse of a 3×4 model matrix), and its 3×3 part.
fn invert(m: &[[f32; 4]; 3]) -> ([V3; 3], V3) {
    let a: [V3; 3] = [[m[0][0], m[0][1], m[0][2]], [m[1][0], m[1][1], m[1][2]], [m[2][0], m[2][1], m[2][2]]];
    let c0 = [a[0][0], a[1][0], a[2][0]];
    let c1 = [a[0][1], a[1][1], a[2][1]];
    let c2 = [a[0][2], a[1][2], a[2][2]];
    let det = dot(c0, cross(c1, c2));
    let k = if det.abs() < 1e-20 { 0.0 } else { 1.0 / det };
    // Rows of the inverse are the cross products of the columns.
    let inv = [scale(cross(c1, c2), k), scale(cross(c2, c0), k), scale(cross(c0, c1), k)];
    let t = [m[0][3], m[1][3], m[2][3]];
    let it = [-dot(inv[0], t), -dot(inv[1], t), -dot(inv[2], t)];
    (inv, it)
}

fn mul(m: &[V3; 3], v: V3) -> V3 {
    [dot(m[0], v), dot(m[1], v), dot(m[2], v)]
}

/// Where a ray enters and leaves the box ±`half` (model space).
fn slab(o: V3, d: V3, half: V3) -> Option<(f32, f32)> {
    let (mut t0, mut t1) = (f32::NEG_INFINITY, f32::INFINITY);
    for i in 0..3 {
        if d[i].abs() < 1e-12 {
            if o[i].abs() > half[i] {
                return None;
            }
            continue;
        }
        let (a, b) = ((-half[i] - o[i]) / d[i], (half[i] - o[i]) / d[i]);
        t0 = t0.max(a.min(b));
        t1 = t1.min(a.max(b));
    }
    (t1 >= t0.max(0.0)).then_some((t0.max(0.0), t1))
}

/// Traces `shape`, placed by `world`, over the pixels `rect` (x0, y0, x1,
/// y1, exclusive ends); `limit` is the view depth already drawn at each of
/// them (row by row). For each pixel: the world normal and the distance
/// along its ray to the surface, or infinity where it misses or is behind.
pub fn trace(shape: &Traced, world: &[[f32; 4]; 3], lens: &Lens, rect: [usize; 4], limit: &[f32]) -> Vec<[f32; 4]> {
    let start = std::time::Instant::now();
    let n = (rect[2] - rect[0]) * (rect[3] - rect[1]);
    if n == 0 {
        return Vec::new();
    }
    let (out, gpu) = match gpu_trace(shape, world, lens, rect, limit) {
        Some(o) => (o, true),
        None => (cpu_trace(shape, world, lens, rect, limit), false),
    };
    let hits = out.iter().filter(|h| h[3].is_finite()).count();
    *stats().lock().unwrap_or_else(|e| e.into_inner()) = Stats { pixels: n, hits, gpu, millis: start.elapsed().as_secs_f64() * 1e3 };
    out
}

/// [`trace`] on the CPU, whatever there is.
pub fn cpu_trace(shape: &Traced, world: &[[f32; 4]; 3], lens: &Lens, rect: [usize; 4], limit: &[f32]) -> Vec<[f32; 4]> {
    let (inv, it) = invert(world);
    let nm = inv; // Normals go through the inverse transpose: rows of inv as columns.
    let half = scale(shape.size, 0.5);
    let f = shape.field.at_time(shape.t as f64);
    let o_m = add(mul(&inv, lens.position), it);
    let rw = rect[2] - rect[0];
    let n = rw * (rect[3] - rect[1]);
    let mut out = vec![[0.0, 0.0, 0.0, f32::INFINITY]; n];
    let threads = std::thread::available_parallelism().map_or(4, |t| t.get()).clamp(1, 16);
    let rows = (rect[3] - rect[1]).div_ceil(threads).max(1);
    let one = |x: usize, y: usize, lim: f32| -> [f32; 4] {
        let dw = lens.dir(x as f32 + 0.5, y as f32 + 0.5);
        let dm = mul(&inv, dw);
        let l = length(dm).max(1e-12);
        let dn = scale(dm, 1.0 / l);
        let Some((t0, t1)) = slab(o_m, dn, half) else { return [0.0, 0.0, 0.0, f32::INFINITY] };
        let ahead = dot(lens.axes[2], dw).max(1e-6);
        let mut t = t0;
        for _ in 0..MAX_STEPS {
            let p = add(o_m, scale(dn, t));
            let d = f.distance(p, shape.size);
            let eps = (0.6 * t / lens.zoom).max(1e-3);
            if d < eps {
                let s = t / l;
                if s * ahead >= lim {
                    break;
                }
                let h = eps.max(1e-3);
                let g = |a: usize| {
                    let (mut p1, mut p2) = (p, p);
                    p1[a] += h;
                    p2[a] -= h;
                    f.distance(p1, shape.size) - f.distance(p2, shape.size)
                };
                let gm = [g(0), g(1), g(2)];
                let nw = normalise([nm[0][0] * gm[0] + nm[1][0] * gm[1] + nm[2][0] * gm[2], nm[0][1] * gm[0] + nm[1][1] * gm[1] + nm[2][1] * gm[2], nm[0][2] * gm[0] + nm[1][2] * gm[1] + nm[2][2] * gm[2]]);
                return [nw[0], nw[1], nw[2], s];
            }
            t += (d * 0.9).max(eps * 0.5);
            if t > t1 {
                break;
            }
        }
        [0.0, 0.0, 0.0, f32::INFINITY]
    };
    std::thread::scope(|sc| {
        for (c, chunk) in out.chunks_mut(rows * rw).enumerate() {
            let one = &one;
            sc.spawn(move || {
                for (k, px) in chunk.iter_mut().enumerate() {
                    let (x, y) = (rect[0] + k % rw, rect[1] + c * rows + k / rw);
                    *px = one(x, y, limit.get(c * rows * rw + k).copied().unwrap_or(f32::INFINITY));
                }
            });
        }
    });
    out
}

const KERNEL: &str = r#"
struct U {
    inv0: vec4<f32>,
    inv1: vec4<f32>,
    inv2: vec4<f32>,
    cam: vec4<f32>,
    ax0: vec4<f32>,
    ax1: vec4<f32>,
    ax2: vec4<f32>,
    size: vec4<f32>,
    lens: vec4<f32>,
    rect: vec4<u32>,
}
@group(0) @binding(0) var<uniform> u: U;
@group(0) @binding(1) var<storage, read> limit: array<f32>;
@group(0) @binding(2) var<storage, read_write> out: array<vec4<f32>>;

fn model(p: vec3<f32>) -> vec3<f32> { return vec3<f32>(dot(u.inv0.xyz, p) + u.inv0.w, dot(u.inv1.xyz, p) + u.inv1.w, dot(u.inv2.xyz, p) + u.inv2.w); }
fn model_dir(d: vec3<f32>) -> vec3<f32> { return vec3<f32>(dot(u.inv0.xyz, d), dot(u.inv1.xyz, d), dot(u.inv2.xyz, d)); }
fn dist(p: vec3<f32>) -> f32 { return field_distance(p, u.size.xyz, u.size.w); }

@compute @workgroup_size(8, 8)
fn trace_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let rw = u.rect.z;
    let rh = u.rect.w;
    if (id.x >= rw || id.y >= rh) { return; }
    let k = id.y * rw + id.x;
    out[k] = vec4<f32>(0.0, 0.0, 0.0, -1.0);
    let x = f32(u.rect.x + id.x) + 0.5;
    let y = f32(u.rect.y + id.y) + 0.5;
    let dw = normalize(u.ax0.xyz * ((x - u.lens.y) / u.lens.x) + u.ax1.xyz * ((y - u.lens.z) / u.lens.x) + u.ax2.xyz);
    let om = model(u.cam.xyz);
    let dm = model_dir(dw);
    let l = max(length(dm), 1e-12);
    let dn = dm / l;
    let half = u.size.xyz * 0.5;
    var t0 = -3.0e38;
    var t1 = 3.0e38;
    for (var a = 0; a < 3; a++) {
        if (abs(dn[a]) < 1e-12) {
            if (abs(om[a]) > half[a]) { return; }
            continue;
        }
        let ta = (-half[a] - om[a]) / dn[a];
        let tb = (half[a] - om[a]) / dn[a];
        t0 = max(t0, min(ta, tb));
        t1 = min(t1, max(ta, tb));
    }
    if (t1 < max(t0, 0.0)) { return; }
    let ahead = max(dot(u.ax2.xyz, dw), 1e-6);
    var t = max(t0, 0.0);
    let steps = u32(u.lens.w);
    for (var i = 0u; i < steps; i++) {
        let p = om + dn * t;
        let d = dist(p);
        let eps = max(0.6 * t / u.lens.x, 1e-3);
        if (d < eps) {
            let s = t / l;
            if (s * ahead >= limit[k]) { return; }
            let h = max(eps, 1e-3);
            let g = vec3<f32>(
                dist(p + vec3<f32>(h, 0.0, 0.0)) - dist(p - vec3<f32>(h, 0.0, 0.0)),
                dist(p + vec3<f32>(0.0, h, 0.0)) - dist(p - vec3<f32>(0.0, h, 0.0)),
                dist(p + vec3<f32>(0.0, 0.0, h)) - dist(p - vec3<f32>(0.0, 0.0, h)));
            // Normals through the inverse transpose: the inverse's rows as columns.
            let n = normalize(u.inv0.xyz * g.x + u.inv1.xyz * g.y + u.inv2.xyz * g.z);
            out[k] = vec4<f32>(n, s);
            return;
        }
        t += max(d * 0.9, eps * 0.5);
        if (t > t1) { return; }
    }
}
"#;

/// The whole shader for a field: its formula's WGSL, then the marcher.
pub fn shader(field: &Field) -> Result<String, String> {
    Ok(format!("{}{KERNEL}", field.wgsl()?))
}

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    layout: wgpu::BindGroupLayout,
    pipes: HashMap<String, Option<wgpu::ComputePipeline>>,
}

fn device() -> Option<&'static Mutex<Gpu>> {
    static G: OnceLock<Option<Mutex<Gpu>>> = OnceLock::new();
    G.get_or_init(|| {
        if std::env::var_os("STUDIO_RENDER_NO_GPU").is_some() {
            return None;
        }
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions { power_preference: wgpu::PowerPreference::HighPerformance, ..Default::default() })).ok()?;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor { label: Some("studio-direct"), required_features: wgpu::Features::empty(), required_limits: adapter.limits(), ..Default::default() })).ok()?;
        let storage = |binding: u32, read_only: bool| wgpu::BindGroupLayoutEntry { binding, visibility: wgpu::ShaderStages::COMPUTE, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only }, has_dynamic_offset: false, min_binding_size: None }, count: None };
        let entries = [wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::COMPUTE, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None }, storage(1, true), storage(2, false)];
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("direct"), entries: &entries });
        Some(Mutex::new(Gpu { device, queue, layout, pipes: HashMap::new() }))
    })
    .as_ref()
}

fn bytes_of<T: Copy>(v: &[T]) -> &[u8] {
    // Plain arrays of f32 / u32 groups.
    unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, std::mem::size_of_val(v)) }
}

fn gpu_trace(shape: &Traced, world: &[[f32; 4]; 3], lens: &Lens, rect: [usize; 4], limit: &[f32]) -> Option<Vec<[f32; 4]>> {
    let lock = device()?;
    let mut g = lock.lock().unwrap_or_else(|e| e.into_inner());
    if !g.pipes.contains_key(&shape.key) {
        let pipe = (|| {
            let src = shader(&shape.field).ok()?;
            let scope = g.device.push_error_scope(wgpu::ErrorFilter::Validation);
            let module = g.device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("direct"), source: wgpu::ShaderSource::Wgsl(src.into()) });
            let pl = g.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("direct"), bind_group_layouts: &[Some(&g.layout)], immediate_size: 0 });
            let p = g.device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor { label: Some("direct"), layout: Some(&pl), module: &module, entry_point: Some("trace_main"), compilation_options: Default::default(), cache: None });
            if let Some(e) = pollster::block_on(scope.pop()) {
                eprintln!("studio-render: a traced shape can't run on the GPU, tracing it on the CPU: {e}");
                return None;
            }
            Some(p)
        })();
        if g.pipes.len() > 64 {
            g.pipes.clear();
        }
        g.pipes.insert(shape.key.clone(), pipe);
    }
    let pipe = g.pipes.get(&shape.key)?.as_ref()?.clone();
    let (inv, it) = invert(world);
    let v4 = |v: V3, w: f32| [v[0], v[1], v[2], w];
    let rw = (rect[2] - rect[0]) as u32;
    let rh = (rect[3] - rect[1]) as u32;
    let mut uni: Vec<[f32; 4]> = vec![v4(inv[0], it[0]), v4(inv[1], it[1]), v4(inv[2], it[2]), v4(lens.position, 0.0), v4(lens.axes[0], 0.0), v4(lens.axes[1], 0.0), v4(lens.axes[2], 0.0), v4(shape.size, shape.t), [lens.zoom, lens.centre[0], lens.centre[1], MAX_STEPS as f32]];
    uni.push([f32::from_bits(rect[0] as u32), f32::from_bits(rect[1] as u32), f32::from_bits(rw), f32::from_bits(rh)]);
    let n = (rw * rh) as usize;
    let lim: Vec<f32> = (0..n).map(|k| limit.get(k).copied().filter(|d| d.is_finite()).unwrap_or(3.0e38)).collect();
    let dev = &g.device;
    let init = |label: &str, bytes: &[u8], usage: wgpu::BufferUsages| dev.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some(label), contents: bytes, usage });
    let ub = init("direct uniforms", bytes_of(&uni), wgpu::BufferUsages::UNIFORM);
    let lb = init("direct limit", bytes_of(&lim), wgpu::BufferUsages::STORAGE);
    let size = (n * 16) as u64;
    let ob = dev.create_buffer(&wgpu::BufferDescriptor { label: Some("direct out"), size, usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC, mapped_at_creation: false });
    let rb = dev.create_buffer(&wgpu::BufferDescriptor { label: Some("direct read"), size, usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
    let entries = [wgpu::BindGroupEntry { binding: 0, resource: ub.as_entire_binding() }, wgpu::BindGroupEntry { binding: 1, resource: lb.as_entire_binding() }, wgpu::BindGroupEntry { binding: 2, resource: ob.as_entire_binding() }];
    let group = dev.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("direct"), layout: &g.layout, entries: &entries });
    let mut enc = dev.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("direct") });
    {
        let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("trace"), timestamp_writes: None });
        pass.set_pipeline(&pipe);
        pass.set_bind_group(0, &group, &[]);
        pass.dispatch_workgroups(rw.div_ceil(8), rh.div_ceil(8), 1);
    }
    enc.copy_buffer_to_buffer(&ob, 0, &rb, 0, size);
    g.queue.submit([enc.finish()]);
    let slice = rb.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    let _ = dev.poll(wgpu::PollType::wait_indefinitely());
    if !matches!(rx.recv(), Ok(Ok(()))) {
        return None;
    }
    let range = slice.get_mapped_range().ok()?;
    let out: Vec<[f32; 4]> = range
        .chunks_exact(16)
        .map(|b| {
            let v: [f32; 4] = std::array::from_fn(|k| f32::from_le_bytes([b[k * 4], b[k * 4 + 1], b[k * 4 + 2], b[k * 4 + 3]]));
            if v[3] < 0.0 { [0.0, 0.0, 0.0, f32::INFINITY] } else { v }
        })
        .collect();
    drop(range);
    rb.unmap();
    Some(out)
}

/// The pixels a placed shape can cover in a `w` × `h` picture (its box's
/// corners seen through the lens), or the whole picture when the box
/// reaches behind the lens.
pub fn screen_rect(shape: &Traced, world: &[[f32; 4]; 3], lens: &Lens, w: usize, h: usize) -> Option<[usize; 4]> {
    let half = scale(shape.size, 0.5);
    let (mut x0, mut y0, mut x1, mut y1) = (f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);
    for k in 0..8 {
        let c = [if k & 1 == 0 { -half[0] } else { half[0] }, if k & 2 == 0 { -half[1] } else { half[1] }, if k & 4 == 0 { -half[2] } else { half[2] }];
        let p = [dot([world[0][0], world[0][1], world[0][2]], c) + world[0][3], dot([world[1][0], world[1][1], world[1][2]], c) + world[1][3], dot([world[2][0], world[2][1], world[2][2]], c) + world[2][3]];
        let d = sub(p, lens.position);
        let z = dot(lens.axes[2], d);
        if z <= 1e-3 {
            return Some([0, 0, w, h]);
        }
        let sx = lens.centre[0] + lens.zoom * dot(lens.axes[0], d) / z;
        let sy = lens.centre[1] + lens.zoom * dot(lens.axes[1], d) / z;
        (x0, y0, x1, y1) = (x0.min(sx), y0.min(sy), x1.max(sx), y1.max(sy));
    }
    let r = [(x0.floor().max(0.0)) as usize, (y0.floor().max(0.0)) as usize, (x1.ceil().max(0.0) as usize + 1).min(w), (y1.ceil().max(0.0) as usize + 1).min(h)];
    (r[2] > r[0] && r[3] > r[1]).then_some(r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_mesh::math::field::{Clip, NAMES};

    fn ball() -> Traced {
        let f = Field::new(vec![studio_mesh::math::compile("t", "x^2 + y^2 + z^2 - 1", NAMES).unwrap()], None, [-1.25; 3], [1.25; 3], Clip::Box, 0.0).unwrap();
        Traced { field: Arc::new(f), size: [125.0; 3], t: 0.0, key: "ball".into() }
    }

    #[test]
    fn a_traced_ball_is_hit_where_it_is_and_its_normals_face_the_lens() {
        // The lens 500 px in front of the ball, looking at it.
        let lens = Lens { position: [0.0, 0.0, -500.0], axes: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]], zoom: 500.0, centre: [100.0, 100.0] };
        let world = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]];
        let b = ball();
        let rect = screen_rect(&b, &world, &lens, 200, 200).unwrap();
        let lim = vec![f32::INFINITY; (rect[2] - rect[0]) * (rect[3] - rect[1])];
        let out = cpu_trace(&b, &world, &lens, rect, &lim);
        let rw = rect[2] - rect[0];
        let at = |x: usize, y: usize| out[(y - rect[1]) * rw + (x - rect[0])];
        let mid = at(100, 100);
        // The front of a ball of radius 50 is 450 px away.
        assert!((mid[3] - 450.0).abs() < 1.0, "{mid:?}");
        assert!(mid[2] < -0.99, "faces the lens: {mid:?}");
        // The silhouette: 50 px at 450..500 px away at zoom 500 is about 50 px across a side.
        assert!(at(140, 100)[3].is_finite() && !at(160, 100)[3].is_finite());
        // Something nearer already drawn hides it.
        let near = vec![100.0; lim.len()];
        assert!(cpu_trace(&b, &world, &lens, rect, &near).iter().all(|h| !h[3].is_finite()));
        // The GPU, when there is one, finds the same.
        if let Some(g) = gpu_trace(&b, &world, &lens, rect, &lim) {
            let gm = g[(100 - rect[1]) * rw + (100 - rect[0])];
            assert!((gm[3] - mid[3]).abs() < 0.5, "{gm:?} vs {mid:?}");
            let agree = g.iter().zip(&out).filter(|(a, c)| a[3].is_finite() == c[3].is_finite()).count();
            assert!(agree as f32 > out.len() as f32 * 0.99);
        }
    }

    #[test]
    fn every_implicit_preset_s_shader_is_valid_wgsl() {
        for p in studio_mesh::math::field::PRESETS {
            let src = studio_mesh::math::field::bind(p.name, p.params, &[], p.formula).unwrap();
            let f = Field::new(vec![studio_mesh::math::compile(p.name, &src, NAMES).unwrap()], None, [-p.bounds; 3], [p.bounds; 3], p.clip, 0.0).unwrap();
            let s = shader(&f).unwrap();
            let module = naga::front::wgsl::parse_str(&s).unwrap_or_else(|e| panic!("{}: {}", p.name, e.emit_to_string(&s)));
            naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all()).validate(&module).unwrap_or_else(|e| panic!("{}: {e:?}", p.name));
        }
    }
}
