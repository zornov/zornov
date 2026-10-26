// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The path tracer on the CPU: the reference the WGSL shader follows
//! function for function (`trace.wgsl`), used when there is no GPU.
//!
//! Units follow Studio's raster lighting so a scene looks the same lit
//! either way: a light's colour is the light a surface facing it gets
//! (a white diffuse card facing a white light shows white), ambient light
//! and the environment are radiance, emissive colour is what a glowing
//! surface shows.

use super::scene::{HAIR, Mat, NO_SHADOW, Scene, UNLIT};
use super::{Passes, TraceSettings};
use crate::frame::Frame;
use crate::space::{V3, add, cross, dot, length, normalise, scale, sub};
use std::f32::consts::PI;

/// A per-path random number stream (PCG).
pub struct Rng(u32);

impl Rng {
    pub fn new(px: u32, py: u32, sample: u32) -> Rng {
        let mut r = Rng(px.wrapping_mul(1973).wrapping_add(py.wrapping_mul(9277)).wrapping_add(sample.wrapping_mul(26699)) | 1);
        r.next();
        r.next();
        r
    }
    pub fn next(&mut self) -> f32 {
        // PCG-style hash step.
        self.0 = self.0.wrapping_mul(747_796_405).wrapping_add(2_891_336_453);
        let mut w = ((self.0 >> ((self.0 >> 28) + 4)) ^ self.0).wrapping_mul(277_803_737);
        w = (w >> 22) ^ w;
        (w >> 8) as f32 / 16_777_216.0
    }
}

/// Where a ray met the scene.
#[derive(Clone, Copy, Debug)]
pub struct Hit {
    pub t: f32,
    pub tri: u32,
    pub u: f32,
    pub v: f32,
}

fn v3(p: [f32; 4]) -> V3 {
    [p[0], p[1], p[2]]
}

fn corner(s: &Scene, v: u32, time: f32) -> V3 {
    let a = v3(s.pos0[v as usize]);
    if !s.moving || time <= 0.0 {
        return a;
    }
    let b = v3(s.pos1[v as usize]);
    add(a, scale(sub(b, a), time))
}

fn tri_at(s: &Scene, t: u32, time: f32) -> [V3; 3] {
    let c = s.tris[t as usize];
    [corner(s, c[0], time), corner(s, c[1], time), corner(s, c[2], time)]
}

/// The nearest triangle along the ray within (t_min, t_max).
pub fn intersect(s: &Scene, o: V3, d: V3, t_min: f32, t_max: f32, time: f32) -> Option<Hit> {
    if s.nodes.is_empty() {
        return None;
    }
    let inv = d.map(|x| if x.abs() > 1e-12 { 1.0 / x } else { 1e12f32.copysign(x) });
    let mut best: Option<Hit> = None;
    let mut far = t_max;
    let mut stack = [0u32; 64];
    let mut sp = 1usize;
    while sp > 0 {
        sp -= 1;
        let ni = stack[sp];
        let n = &s.nodes[ni as usize];
        let (lo, hi) = (v3(n[0]), v3(n[1]));
        let mut t0 = t_min;
        let mut t1 = far;
        for k in 0..3 {
            let a = (lo[k] - o[k]) * inv[k];
            let b = (hi[k] - o[k]) * inv[k];
            t0 = t0.max(a.min(b));
            t1 = t1.min(a.max(b));
        }
        if t0 > t1 {
            continue;
        }
        let (index, count) = (n[0][3].to_bits(), n[1][3].to_bits());
        if count > 0 {
            for t in index..index + count {
                if let Some((th, u, v)) = studio_mesh::bvh::ray_tri(&tri_at(s, t, time), o, d)
                    && th > t_min
                    && th < far
                {
                    far = th;
                    best = Some(Hit { t: th, tri: t, u, v });
                }
            }
        } else if sp + 2 <= stack.len() {
            stack[sp] = index;
            stack[sp + 1] = ni + 1;
            sp += 2;
        }
    }
    best
}

/// What a surface is at a hit.
pub struct Surface {
    pub p: V3,
    /// Geometric normal (as wound) and shading normal (facing the ray).
    pub ng: V3,
    pub n: V3,
    pub front: bool,
    pub base: [f32; 4],
    pub metallic: f32,
    pub roughness: f32,
    pub transmission: f32,
    pub ior: f32,
    pub emissive: V3,
    pub subsurface: f32,
    pub flags: u32,
    pub object: u32,
}

pub fn surface(s: &Scene, h: &Hit, o: V3, d: V3, time: f32) -> Surface {
    let c = s.tris[h.tri as usize];
    let m: &Mat = &s.mats[c[3] as usize];
    let tri = tri_at(s, h.tri, time);
    let p = add(o, scale(d, h.t));
    let ng = normalise(cross(sub(tri[1], tri[0]), sub(tri[2], tri[0])));
    let w = 1.0 - h.u - h.v;
    let vn = |i: usize| v3(s.nrm[c[i] as usize]);
    let smooth_n = normalise(add(add(scale(vn(0), w), scale(vn(1), h.u)), scale(vn(2), h.v)));
    let smooth = m[3][2];
    let mut n = if smooth_n == [0.0; 3] { ng } else { normalise(add(scale(ng, 1.0 - smooth), scale(if dot(smooth_n, ng) < 0.0 { scale(smooth_n, -1.0) } else { smooth_n }, smooth))) };
    let front = dot(ng, d) < 0.0;
    if dot(n, d) > 0.0 {
        n = scale(n, -1.0);
    }
    let uv = {
        let t = |i: usize| s.uv[c[i] as usize];
        [t(0)[0] * w + t(1)[0] * h.u + t(2)[0] * h.v, t(0)[1] * w + t(1)[1] * h.u + t(2)[1] * h.v]
    };
    let tex = s.texture(m, uv);
    let base = [m[0][0] * tex[0], m[0][1] * tex[1], m[0][2] * tex[2], m[0][3] * tex[3]];
    Surface {
        p,
        ng,
        n,
        front,
        base,
        metallic: m[1][0],
        roughness: m[1][1],
        transmission: m[1][2],
        ior: m[1][3],
        emissive: [m[2][0], m[2][1], m[2][2]],
        subsurface: m[2][3],
        flags: m[3][3].to_bits(),
        object: s.tri_obj[h.tri as usize],
    }
}

fn lum(c: V3) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

fn mul(a: V3, b: V3) -> V3 {
    [a[0] * b[0], a[1] * b[1], a[2] * b[2]]
}

/// An orthonormal frame around `n`.
pub fn frame(n: V3) -> (V3, V3) {
    let s = if n[2] >= 0.0 { 1.0 } else { -1.0 };
    let a = -1.0 / (s + n[2]);
    let b = n[0] * n[1] * a;
    ([1.0 + s * n[0] * n[0] * a, s * b, -s * n[0]], [b, s + n[1] * n[1] * a, -n[1]])
}

fn cosine_dir(n: V3, u1: f32, u2: f32) -> V3 {
    let r = u1.sqrt();
    let phi = 2.0 * PI * u2;
    let (t, b) = frame(n);
    normalise(add(add(scale(t, r * phi.cos()), scale(b, r * phi.sin())), scale(n, (1.0 - u1).max(0.0).sqrt())))
}

fn uniform_sphere(u1: f32, u2: f32) -> V3 {
    let z = 1.0 - 2.0 * u1;
    let r = (1.0 - z * z).max(0.0).sqrt();
    let phi = 2.0 * PI * u2;
    [r * phi.cos(), r * phi.sin(), z]
}

fn ggx_d(ndh: f32, a2: f32) -> f32 {
    let k = ndh * ndh * (a2 - 1.0) + 1.0;
    a2 / (PI * k * k).max(1e-12)
}

fn smith_g1(ndv: f32, a2: f32) -> f32 {
    2.0 * ndv / (ndv + (a2 + (1.0 - a2) * ndv * ndv).sqrt()).max(1e-12)
}

fn schlick(f0: V3, c: f32) -> V3 {
    let k = (1.0 - c).clamp(0.0, 1.0).powi(5);
    f0.map(|f| f + (1.0 - f) * k)
}

/// Exact Fresnel reflectance of a dielectric (`eta` = inside / outside).
fn fresnel_dielectric(cos_i: f32, eta: f32) -> f32 {
    let sin_t2 = (1.0 - cos_i * cos_i).max(0.0) / (eta * eta);
    if sin_t2 >= 1.0 {
        return 1.0;
    }
    let cos_t = (1.0 - sin_t2).sqrt();
    let rs = (cos_i - eta * cos_t) / (cos_i + eta * cos_t);
    let rp = (eta * cos_i - cos_t) / (eta * cos_i + cos_t);
    0.5 * (rs * rs + rp * rp)
}

/// How a surface splits incoming light between its lobes.
struct Lobes {
    f0: V3,
    diffuse: V3,
    a2: f32,
    p_spec: f32,
    p_diff: f32,
    p_trans: f32,
}

fn lobes(sf: &Surface, ndv: f32) -> Lobes {
    let base = [sf.base[0], sf.base[1], sf.base[2]];
    let dielectric_f0 = ((sf.ior - 1.0) / (sf.ior + 1.0)).powi(2);
    let f0: V3 = std::array::from_fn(|i| dielectric_f0 + (base[i] - dielectric_f0) * sf.metallic);
    let hair = sf.flags & HAIR != 0;
    let rough = if hair { sf.roughness.max(0.3) } else { sf.roughness };
    let a = (rough * rough).max(1e-4);
    let fr = lum(schlick(f0, ndv));
    let opaque = (1.0 - sf.metallic) * (1.0 - sf.transmission);
    let diffuse = scale(base, opaque);
    let ws = fr.clamp(0.04, 1.0);
    let wd = lum(diffuse) * (1.0 - fr);
    let wt = (1.0 - sf.metallic) * sf.transmission * (1.0 - fr).max(0.2) + if hair { 0.25 } else { 0.0 };
    let total = (ws + wd + wt).max(1e-6);
    Lobes { f0, diffuse, a2: a * a, p_spec: ws / total, p_diff: wd / total, p_trans: wt / total }
}

/// The surface's reflection for light from `wi` seen from `wo` (cosine
/// included), and the probability the sampler picks `wi`.
fn eval(sf: &Surface, l: &Lobes, wo: V3, wi: V3) -> (V3, f32) {
    let n = sf.n;
    let ndl = dot(n, wi);
    let ndv = dot(n, wo).max(1e-4);
    if ndl <= 0.0 {
        return ([0.0; 3], 0.0);
    }
    let h = normalise(add(wi, wo));
    let ndh = dot(n, h).max(0.0);
    let vdh = dot(wo, h).max(1e-4);
    let d = ggx_d(ndh, l.a2);
    let g = smith_g1(ndv, l.a2) * smith_g1(ndl, l.a2);
    let f = schlick(l.f0, vdh);
    let spec = scale(f, d * g / (4.0 * ndv * ndl).max(1e-6));
    let diff = scale(mul(l.diffuse, [1.0 - lum(f); 3]), 1.0 / PI);
    let value = scale(add(spec, diff), ndl);
    let pdf = l.p_spec * d * ndh / (4.0 * vdh) + l.p_diff * ndl / PI;
    (value, pdf)
}

fn power(a: f32, b: f32) -> f32 {
    let (a2, b2) = (a * a, b * b);
    if a2 + b2 <= 0.0 { 0.0 } else { a2 / (a2 + b2) }
}

/// Light let through to `to` (or `dist` along `d`): coloured by glass,
/// stopped by solids, thinned by smoke.
fn visibility(s: &Scene, from: V3, d: V3, dist: f32, time: f32, rng: &mut Rng) -> V3 {
    let mut t = [1.0f32; 3];
    let eps = s.extent * 1e-5 + 1e-3;
    let mut o = from;
    let mut left = dist;
    for _ in 0..8 {
        let Some(h) = intersect(s, o, d, eps, left - eps, time) else { break };
        let c = s.tris[h.tri as usize];
        let m = &s.mats[c[3] as usize];
        let flags = m[3][3].to_bits();
        if flags & NO_SHADOW == 0 {
            let alpha = m[0][3];
            let pass = (1.0 - alpha) + alpha * m[1][2] * (1.0 - m[1][0]);
            if pass <= 0.0 {
                return [0.0; 3];
            }
            let tint = [m[0][0], m[0][1], m[0][2]].map(|b| (1.0 - alpha) + alpha * m[1][2] * b);
            t = mul(t, tint);
        }
        o = add(o, scale(d, h.t));
        left -= h.t;
        if lum(t) < 1e-4 {
            return [0.0; 3];
        }
    }
    let tv = volume_transmittance(s, from, d, dist, rng);
    scale(t, tv)
}

fn box_span(lo: V3, hi: V3, o: V3, d: V3, t_max: f32) -> Option<(f32, f32)> {
    let mut t0: f32 = 0.0;
    let mut t1 = t_max;
    for k in 0..3 {
        let inv = if d[k].abs() > 1e-12 { 1.0 / d[k] } else { 1e12f32.copysign(d[k]) };
        let a = (lo[k] - o[k]) * inv;
        let b = (hi[k] - o[k]) * inv;
        t0 = t0.max(a.min(b));
        t1 = t1.min(a.max(b));
    }
    (t0 < t1).then_some((t0, t1))
}

/// Ratio tracking through the smoke along a segment.
fn volume_transmittance(s: &Scene, o: V3, d: V3, dist: f32, rng: &mut Rng) -> f32 {
    let mut tr = 1.0;
    for v in &s.volumes {
        let Some((a, b)) = box_span(v.lo, v.hi, o, d, dist.min(1e9)) else { continue };
        let mut t = a;
        loop {
            t -= (1.0 - rng.next()).ln() / v.density.max(1e-6);
            if t >= b {
                break;
            }
            let (den, _) = s.volume_at(v, add(o, scale(d, t)));
            tr *= 1.0 - den;
            if tr < 1e-3 {
                return 0.0;
            }
        }
    }
    tr
}

/// Light fire gives off along a segment, dimmed by the smoke in front of
/// it (marched in steps, as the raster view does).
fn fire_glow(s: &Scene, o: V3, d: V3, t_end: f32) -> V3 {
    let mut out = [0.0f32; 3];
    for v in s.volumes.iter().filter(|v| v.fire > 0.0) {
        let Some((a, b)) = box_span(v.lo, v.hi, o, d, t_end.min(1e9)) else { continue };
        let steps = 32;
        let ds = (b - a) / steps as f32;
        let mut optical = 0.0f32;
        for k in 0..steps {
            let p = add(o, scale(d, a + (k as f32 + 0.5) * ds));
            let (den, heat) = s.volume_at(v, p);
            if heat > 0.0 {
                let e = scale(fire_colour(heat), v.fire * heat * 4.0 * v.density * ds * (-optical).exp());
                out = add(out, e);
            }
            optical += den * v.density * ds;
        }
    }
    out
}

/// The colour of fire at a heat (0..1): deep red to orange to yellow.
pub fn fire_colour(heat: f32) -> V3 {
    let h = heat.clamp(0.0, 1.0);
    [h * 1.6, (h * h) * 1.1, (h * h * h * h) * 0.6]
}

/// A delta light's contribution at a surface point: the light reaching it
/// along `wi` (colour × π to match raster lighting), the direction and
/// distance to it.
fn light_sample(s: &Scene, li: usize, p: V3, rng: &mut Rng) -> Option<(V3, V3, f32)> {
    let l = &s.lights[li];
    let kind = l[0][0] as u32;
    let colour = [l[3][0], l[3][1], l[3][2]];
    match kind {
        0 => {
            // The sun: a direction within its disk.
            let to = scale(normalise(v3(l[2])), -1.0);
            let cos_max = l[0][1];
            let z = 1.0 - rng.next() * (1.0 - cos_max);
            let r = (1.0 - z * z).max(0.0).sqrt();
            let phi = 2.0 * PI * rng.next();
            let (t, b) = frame(to);
            let wi = normalise(add(add(scale(t, r * phi.cos()), scale(b, r * phi.sin())), scale(to, z)));
            Some((scale(colour, PI), wi, f32::INFINITY))
        }
        _ => {
            let pos = v3(l[1]);
            let dv = sub(pos, p);
            let dist = length(dv);
            if dist < 1e-4 {
                return None;
            }
            let wi = scale(dv, 1.0 / dist);
            let (radius, width) = (l[1][3], l[2][3]);
            let mut k = if width > 0.0 { 1.0 - smoothstep(radius, radius + width, dist) } else { 1.0 };
            if kind == 1 {
                k *= smoothstep(l[0][2], l[0][3], dot(scale(wi, -1.0), normalise(v3(l[2]))));
            }
            if k <= 0.0 {
                return None;
            }
            Some((scale(colour, PI * k), wi, dist))
        }
    }
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0).max(1e-6)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// One path through one pixel: radiance, coverage, and the first hit's
/// passes.
struct PathResult {
    rgb: V3,
    alpha: f32,
    depth: f32,
    normal: V3,
    albedo: V3,
    object: u64,
}

fn trace_path(s: &Scene, px: [f32; 2], rng: &mut Rng, settings: &TraceSettings) -> PathResult {
    let time = if s.moving && settings.shutter > 0.0 { rng.next() } else { 0.0 };
    let (mut o, mut d) = s.camera_ray(px, [rng.next() * 2.0 - 1.0, rng.next() * 2.0 - 1.0]);
    let mut out = PathResult { rgb: [0.0; 3], alpha: 0.0, depth: f32::INFINITY, normal: [0.0; 3], albedo: [0.0; 3], object: 0 };
    let mut tp = [1.0f32; 3];
    let mut last_pdf = 0.0f32;
    let mut last_delta = true;
    let eps = s.extent * 1e-5 + 1e-3;
    let max_b = settings.bounces.clamp(1, 32);
    let mut bounce = 0u32;
    let mut passes_through = 0;
    while bounce <= max_b {
        let hit = intersect(s, o, d, eps, f32::INFINITY, time);
        let seg_end = hit.map_or(1e9f32, |h| h.t);
        // Smoke along the way: the nearest real collision, if any.
        let mut vol_hit: Option<(f32, usize, f32)> = None;
        for (vi, v) in s.volumes.iter().enumerate() {
            let Some((a, b)) = box_span(v.lo, v.hi, o, d, seg_end) else { continue };
            let mut t = a;
            loop {
                t -= (1.0 - rng.next()).ln() / v.density.max(1e-6);
                if t >= b || vol_hit.is_some_and(|x| t >= x.0) {
                    break;
                }
                let (den, heat) = s.volume_at(v, add(o, scale(d, t)));
                if rng.next() < den {
                    vol_hit = Some((t, vi, heat));
                    break;
                }
            }
        }
        // Fire glows along the way (up to where the path scatters).
        let glow = fire_glow(s, o, d, vol_hit.map_or(seg_end, |v| v.0));
        if glow != [0.0; 3] {
            out.rgb = add(out.rgb, mul(tp, glow));
            if bounce == 0 {
                out.alpha = out.alpha.max(lum(glow).min(1.0));
            }
        }
        if let Some((t, vi, _heat)) = vol_hit {
            let v = &s.volumes[vi];
            let p = add(o, scale(d, t));
            if bounce == 0 {
                out.alpha = 1.0;
            }
            let albedo = lum(v.colour).clamp(0.0, 1.0);
            if rng.next() >= albedo.max(0.05) {
                break;
            }
            tp = mul(tp, scale(v.colour, 1.0 / albedo.max(0.05)));
            // Light reaching the smoke (isotropic: a quarter of a surface's).
            for li in 0..s.lights.len() {
                if let Some((e, wi, dist)) = light_sample(s, li, p, rng) {
                    let vis = visibility(s, p, wi, dist, time, rng);
                    out.rgb = add(out.rgb, mul(tp, scale(mul(e, vis), 0.25 / PI)));
                }
            }
            if !s.env.is_empty() {
                let (wi, pdf) = s.sample_env(rng.next(), rng.next());
                if pdf > 0.0 {
                    let vis = visibility(s, p, wi, f32::INFINITY, time, rng);
                    out.rgb = add(out.rgb, mul(tp, scale(mul(s.env_at(wi), vis), 1.0 / (4.0 * PI * pdf) * power(pdf, 1.0 / (4.0 * PI)))));
                }
            }
            o = p;
            d = normalise(uniform_sphere(rng.next(), rng.next()));
            last_pdf = 1.0 / (4.0 * PI);
            last_delta = false;
            bounce += 1;
            continue;
        }
        let Some(h) = hit else {
            // Out to the sky.
            if bounce == 0 {
                if s.background {
                    out.rgb = add(out.rgb, mul(tp, s.env_at(d)));
                    out.alpha = 1.0;
                }
            } else if !s.env.is_empty() {
                let w = if last_delta { 1.0 } else { power(last_pdf, s.env_pdf(d)) };
                out.rgb = add(out.rgb, scale(mul(tp, s.env_at(d)), w));
            }
            break;
        };
        let sf = surface(s, &h, o, d, time);
        // See-through parts let the ray carry on unchanged.
        if sf.base[3] < 1.0 && rng.next() > sf.base[3] && passes_through < 16 {
            o = sf.p;
            passes_through += 1;
            continue;
        }
        if bounce == 0 {
            out.alpha = 1.0;
            out.depth = h.t * dot(d, s.cam.fwd);
            out.normal = sf.n;
            out.albedo = [sf.base[0], sf.base[1], sf.base[2]];
            out.object = s.ids.get(sf.object as usize).copied().unwrap_or(0);
            if s.haze > 0.0 {
                let k = 1.0 - (-h.t * s.haze).exp();
                out.rgb = add(out.rgb, scale(s.haze_colour, k));
                tp = scale(tp, 1.0 - k);
            }
        }
        // Glowing surfaces.
        if sf.emissive != [0.0; 3] {
            let w = if bounce == 0 || last_delta {
                1.0
            } else {
                let cos_l = dot(sf.ng, d).abs().max(1e-4);
                let pdf_light = lum(sf.emissive) / s.emit_total.max(1e-9) * h.t * h.t / cos_l;
                power(last_pdf, pdf_light)
            };
            out.rgb = add(out.rgb, scale(mul(tp, sf.emissive), w));
        }
        if sf.flags & UNLIT != 0 {
            out.rgb = add(out.rgb, mul(tp, [sf.base[0], sf.base[1], sf.base[2]]));
            break;
        }
        let wo = scale(d, -1.0);
        let ndv = dot(sf.n, wo).max(1e-4);
        let l = lobes(&sf, ndv);
        let p_off = add(sf.p, scale(sf.n, eps));
        // Direct light: lights, glowing triangles, the environment.
        if l.p_spec + l.p_diff > 0.0 {
            for li in 0..s.lights.len() {
                let Some((e, wi, dist)) = light_sample(s, li, p_off, rng) else { continue };
                let (f, _) = eval(&sf, &l, wo, wi);
                if lum(f) <= 0.0 {
                    continue;
                }
                let vis = visibility(s, p_off, wi, dist, time, rng);
                let dark = s.lights[li][3][3];
                let vis = vis.map(|x| 1.0 - dark * (1.0 - x));
                out.rgb = add(out.rgb, mul(tp, mul(mul(f, e), vis)));
            }
            if !s.emit.is_empty() && s.emit_total > 0.0 {
                let x = rng.next() * s.emit_total;
                let k = s.emit.partition_point(|e| e[1] < x).min(s.emit.len() - 1);
                let ti = s.emit[k][0].to_bits();
                let tri = tri_at(s, ti, time);
                let (mut a, mut b) = (rng.next(), rng.next());
                if a + b > 1.0 {
                    a = 1.0 - a;
                    b = 1.0 - b;
                }
                let q = add(tri[0], add(scale(sub(tri[1], tri[0]), a), scale(sub(tri[2], tri[0]), b)));
                let dv = sub(q, p_off);
                let dist = length(dv);
                if dist > eps {
                    let wi = scale(dv, 1.0 / dist);
                    let ln = normalise(cross(sub(tri[1], tri[0]), sub(tri[2], tri[0])));
                    let cos_l = dot(ln, wi).abs();
                    let m = &s.mats[s.tris[ti as usize][3] as usize];
                    let le = [m[2][0], m[2][1], m[2][2]];
                    let pdf = lum(le) / s.emit_total * dist * dist / cos_l.max(1e-4);
                    let (f, bpdf) = eval(&sf, &l, wo, wi);
                    if lum(f) > 0.0 && cos_l > 0.0 {
                        let vis = visibility(s, p_off, wi, dist - eps * 2.0, time, rng);
                        out.rgb = add(out.rgb, scale(mul(tp, mul(mul(f, le), vis)), power(pdf, bpdf) / pdf));
                    }
                }
            }
            if !s.env.is_empty() {
                let (wi, pdf) = s.sample_env(rng.next(), rng.next());
                if pdf > 0.0 {
                    let (f, bpdf) = eval(&sf, &l, wo, wi);
                    if lum(f) > 0.0 {
                        let vis = visibility(s, p_off, wi, f32::INFINITY, time, rng);
                        out.rgb = add(out.rgb, scale(mul(tp, mul(mul(f, s.env_at(wi)), vis)), power(pdf, bpdf) / pdf));
                    }
                }
            }
        }
        if bounce == max_b {
            break;
        }
        // The next direction.
        let pick = rng.next();
        let (u1, u2) = (rng.next(), rng.next());
        if pick < l.p_trans {
            // Glass, water, a hair's light passing through.
            if sf.flags & HAIR != 0 {
                tp = mul(tp, [sf.base[0], sf.base[1], sf.base[2]].map(|c| c.sqrt()));
                o = add(sf.p, scale(d, eps * 2.0));
                last_delta = true;
                bounce += 1;
                continue;
            }
            let a = l.a2.sqrt();
            let phi = 2.0 * PI * u1;
            let cos_t = ((1.0 - u2) / (1.0 + (a * a - 1.0) * u2)).max(0.0).sqrt();
            let sin_t = (1.0 - cos_t * cos_t).max(0.0).sqrt();
            let (tx, ty) = frame(sf.n);
            let m = normalise(add(add(scale(tx, sin_t * phi.cos()), scale(ty, sin_t * phi.sin())), scale(sf.n, cos_t)));
            let eta = if sf.front { sf.ior } else { 1.0 / sf.ior };
            let cos_i = dot(wo, m).max(1e-4);
            let fr = fresnel_dielectric(cos_i, eta);
            if rng.next() < fr {
                d = normalise(sub(scale(m, 2.0 * dot(wo, m)), wo));
                o = add(sf.p, scale(sf.n, eps));
            } else {
                // Snell's law through the microfacet.
                let r = 1.0 / eta;
                let k = 1.0 - r * r * (1.0 - cos_i * cos_i);
                d = normalise(add(scale(scale(wo, -1.0), r), scale(m, r * cos_i - k.max(0.0).sqrt())));
                o = add(sf.p, scale(sf.n, -eps));
                // Coloured glass tints what passes.
                tp = mul(tp, [sf.base[0], sf.base[1], sf.base[2]]);
            }
            last_delta = true;
        } else if pick < l.p_trans + l.p_spec {
            let a2 = l.a2;
            let phi = 2.0 * PI * u1;
            let cos_t = ((1.0 - u2) / (1.0 + (a2 - 1.0) * u2)).max(0.0).sqrt();
            let sin_t = (1.0 - cos_t * cos_t).max(0.0).sqrt();
            let (tx, ty) = frame(sf.n);
            let hv = normalise(add(add(scale(tx, sin_t * phi.cos()), scale(ty, sin_t * phi.sin())), scale(sf.n, cos_t)));
            let wi = normalise(sub(scale(hv, 2.0 * dot(wo, hv)), wo));
            let (f, pdf) = eval(&sf, &l, wo, wi);
            if pdf <= 0.0 || lum(f) <= 0.0 {
                break;
            }
            tp = mul(tp, scale(f, 1.0 / pdf));
            o = p_off;
            d = wi;
            last_pdf = pdf;
            last_delta = l.a2 < 1e-3;
        } else {
            // Diffuse, or under the skin first.
            if sf.subsurface > 0.0 && rng.next() < sf.subsurface {
                match random_walk(s, &sf, d, time, rng) {
                    Some((exit, n_exit)) => {
                        let wi = cosine_dir(n_exit, rng.next(), rng.next());
                        tp = mul(tp, [sf.base[0], sf.base[1], sf.base[2]]);
                        o = add(exit, scale(n_exit, eps));
                        d = wi;
                        last_pdf = dot(n_exit, wi).max(1e-4) / PI;
                        last_delta = false;
                        bounce += 1;
                        continue;
                    }
                    None => break,
                }
            }
            let wi = cosine_dir(sf.n, u1, u2);
            let (f, pdf) = eval(&sf, &l, wo, wi);
            if pdf <= 0.0 {
                break;
            }
            tp = mul(tp, scale(f, 1.0 / pdf));
            o = p_off;
            d = wi;
            last_pdf = pdf;
            last_delta = false;
        }
        bounce += 1;
        // Russian roulette after a few bounces.
        if bounce > 3 {
            let q = tp[0].max(tp[1]).max(tp[2]).min(0.95);
            if rng.next() > q {
                break;
            }
            tp = scale(tp, 1.0 / q);
        }
    }
    let m = lum(out.rgb);
    if m > settings.clamp && m.is_finite() {
        out.rgb = scale(out.rgb, settings.clamp / m);
    }
    if !out.rgb.iter().all(|v| v.is_finite()) {
        out.rgb = [0.0; 3];
    }
    out
}

/// Light that enters skin, wax or marble wanders under the surface and
/// comes out nearby: a random walk inside the model. Returns where it
/// leaves and the surface normal there.
fn random_walk(s: &Scene, sf: &Surface, d: V3, time: f32, rng: &mut Rng) -> Option<(V3, V3)> {
    let mfp = (sf.subsurface * 6.0).max(0.2);
    let eps = s.extent * 1e-5 + 1e-3;
    let mut p = add(sf.p, scale(sf.n, -eps));
    // In through the surface, mostly straight.
    let mut dir = cosine_dir(scale(sf.n, -1.0), rng.next(), rng.next());
    let _ = d;
    for _ in 0..24 {
        let step = -(1.0 - rng.next()).ln() * mfp;
        match intersect(s, p, dir, eps, step, time) {
            Some(h) => {
                let tri = tri_at(s, h.tri, time);
                let mut n = normalise(cross(sub(tri[1], tri[0]), sub(tri[2], tri[0])));
                if dot(n, dir) < 0.0 {
                    n = scale(n, -1.0);
                }
                return Some((add(p, scale(dir, h.t)), n));
            }
            None => {
                p = add(p, scale(dir, step));
                dir = normalise(uniform_sphere(rng.next(), rng.next()));
            }
        }
    }
    None
}

/// Renders the scene on the CPU.
pub fn render(s: &Scene, samples: u32, settings: &TraceSettings) -> Passes {
    let (w, h) = (s.width as usize, s.height as usize);
    let mut rgba = vec![[0.0f32; 4]; w * h];
    let mut depth = vec![f32::INFINITY; w * h];
    let mut normal = vec![[0.0f32; 3]; w * h];
    let mut albedo = vec![[0.0f32; 3]; w * h];
    let mut id = vec![0u64; w * h];
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(1, 16);
    let rows_per = h.div_ceil(threads * 4).max(1);
    let next = std::sync::atomic::AtomicUsize::new(0);
    type Band<'a> = (&'a mut [[f32; 4]], &'a mut [f32], &'a mut [[f32; 3]], &'a mut [[f32; 3]], &'a mut [u64]);
    let bands: Vec<std::sync::Mutex<Band>> = rgba
        .chunks_mut(rows_per * w)
        .zip(depth.chunks_mut(rows_per * w))
        .zip(normal.chunks_mut(rows_per * w))
        .zip(albedo.chunks_mut(rows_per * w))
        .zip(id.chunks_mut(rows_per * w))
        .map(|((((a, b), c), d), e)| std::sync::Mutex::new((a, b, c, d, e)))
        .collect();
    let n_bands = bands.len();
    std::thread::scope(|sc| {
        for _ in 0..threads.min(n_bands) {
            sc.spawn(|| {
                loop {
                    let k = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    if k >= n_bands {
                        break;
                    }
                    let mut band = bands[k].lock().unwrap_or_else(|e| e.into_inner());
                    let (out, dep, nor, alb, ids) = &mut *band;
                    for (i, px) in out.iter_mut().enumerate() {
                        let gi = k * rows_per * w + i;
                        let (x, y) = ((gi % w) as u32, (gi / w) as u32);
                        let mut acc = [0.0f32; 4];
                        for sample in 0..samples {
                            let mut rng = Rng::new(x, y, sample);
                            let r = trace_path(s, [x as f32 + rng.next(), y as f32 + rng.next()], &mut rng, settings);
                            acc = [acc[0] + r.rgb[0], acc[1] + r.rgb[1], acc[2] + r.rgb[2], acc[3] + r.alpha];
                            if sample == 0 || (dep[i].is_infinite() && r.depth.is_finite()) {
                                dep[i] = r.depth;
                                nor[i] = r.normal;
                                alb[i] = r.albedo;
                                ids[i] = r.object;
                            }
                        }
                        *px = acc.map(|v| v / samples as f32);
                    }
                }
            });
        }
    });
    drop(bands);
    Passes { beauty: Frame { width: s.width, height: s.height, pixels: rgba }, depth, normal, albedo, id }
}
