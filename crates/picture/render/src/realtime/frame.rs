// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The numbers a frame of the real-time renderer is made from: the camera
//! and its matrices, the sun's shadow cascades, the sky as an environment
//! picture with its ambient light, and the per-pass uniform block that
//! mirrors `common.wgsl`.

use crate::sky::{self, SkyDesc};
use bytemuck::{Pod, Zeroable};
use studio_world::math::{self, Mat4, V3, add, dot, len, mat_mul, mul, norm, sub};
use studio_world::walk::Pose;
use studio_world::world::{Light, LightKind, Sky, World};

/// Where the camera is and what it sees.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    pub position: V3,
    pub yaw: f32,
    pub pitch: f32,
    pub roll: f32,
    /// Vertical field of view, degrees.
    pub fov: f32,
    pub near: f32,
    pub far: f32,
}

impl View {
    pub fn from_pose(p: &Pose) -> View {
        View { position: p.pos, yaw: p.yaw, pitch: p.pitch, roll: 0.0, fov: p.fov, near: 0.1, far: 1500.0 }
    }
    pub fn pose(&self) -> Pose {
        Pose { pos: self.position, yaw: self.yaw, pitch: self.pitch, fov: self.fov }
    }
    pub fn forward(&self) -> V3 {
        self.pose().forward()
    }
    pub fn right(&self) -> V3 {
        let r = self.pose().right();
        let u = self.pose().up();
        // Rolled about the view axis.
        let (s, c) = self.roll.to_radians().sin_cos();
        add(mul(r, c), mul(u, -s))
    }
    pub fn up(&self) -> V3 {
        let r = self.pose().right();
        let u = self.pose().up();
        let (s, c) = self.roll.to_radians().sin_cos();
        add(mul(u, c), mul(r, s))
    }
    pub fn view_matrix(&self) -> Mat4 {
        math::look_to(self.position, self.forward(), self.up())
    }
    pub fn proj_matrix(&self, aspect: f32, jitter_ndc: [f32; 2]) -> Mat4 {
        let mut p = math::perspective(self.fov.to_radians(), aspect, self.near, self.far);
        p[2][0] += jitter_ndc[0];
        p[2][1] += jitter_ndc[1];
        p
    }
}

/// One slice of the camera's view lit by its own shadow map.
#[derive(Clone, Copy, Debug)]
pub struct Cascade {
    pub vp: Mat4,
    /// View depth where this cascade ends.
    pub split: f32,
    /// World size of a texel.
    pub texel: f32,
}

/// Shadow cascades for light travelling along `light_dir` (unit): split by
/// a blend of even and logarithmic steps up to `shadow_far`, each fitted
/// to a sphere round its slice and snapped to texels so shadows don't
/// shimmer as the camera moves. `reach` is how far above the slice
/// casters may stand.
pub fn cascades(view: &View, aspect: f32, light_dir: V3, count: usize, shadow_far: f32, res: u32, reach: f32) -> Vec<Cascade> {
    let count = count.clamp(1, 4);
    let near = view.near.max(0.05);
    let far = shadow_far.min(view.far).max(near + 1.0);
    let lambda = 0.65;
    let mut splits = Vec::with_capacity(count + 1);
    for i in 0..=count {
        let f = i as f32 / count as f32;
        let log = near * (far / near).powf(f);
        let lin = near + (far - near) * f;
        splits.push(lambda * log + (1.0 - lambda) * lin);
    }
    let (fwd, right, up) = (view.forward(), view.right(), view.up());
    let th = (view.fov.to_radians() * 0.5).tan();
    let mut out = Vec::new();
    for i in 0..count {
        let (z0, z1) = (splits[i], splits[i + 1]);
        let mut centre = [0.0; 3];
        let mut corners = Vec::with_capacity(8);
        for z in [z0, z1] {
            let (hh, hw) = (z * th, z * th * aspect);
            for (sx, sy) in [(-1.0f32, -1.0f32), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
                let p = add(add(add(view.position, mul(fwd, z)), mul(right, sx * hw)), mul(up, sy * hh));
                corners.push(p);
                centre = add(centre, p);
            }
        }
        centre = mul(centre, 1.0 / 8.0);
        let mut radius = corners.iter().map(|c| len(sub(*c, centre))).fold(0.0f32, f32::max);
        // Stable size.
        radius = (radius * 16.0).ceil() / 16.0;
        let texel = 2.0 * radius / res as f32;
        let mut up_l = [0.0, 1.0, 0.0];
        if dot(norm(light_dir), up_l).abs() > 0.99 {
            up_l = [0.0, 0.0, 1.0];
        }
        // A light space that stays put (its origin at the world's), so the
        // texel grid does not slide as the camera moves; the window is
        // snapped to whole texels in it.
        let lv = math::look_to([0.0; 3], light_dir, up_l);
        let c_l = math::mat_vec(&lv, [centre[0], centre[1], centre[2], 1.0]);
        let snap = |v: f32| (v / texel).round() * texel;
        let (cx, cy) = (snap(c_l[0]), snap(c_l[1]));
        let proj = math::ortho(cx - radius, cx + radius, cy - radius, cy + radius, c_l[2] - radius - reach, c_l[2] + radius);
        out.push(Cascade { vp: mat_mul(&proj, &lv), split: z1, texel });
    }
    out
}

/// The sky as the shaders use it.
pub struct EnvData {
    /// Mip chain of the equirectangular sky, half floats RGBA: (w, h, bits).
    pub mips: Vec<(u32, u32, Vec<u16>)>,
    pub sh: [[f32; 3]; 9],
    /// Unit direction towards the light that casts shadows (sun or moon).
    pub light_to: V3,
    pub light_colour: V3,
    /// Sine of the sun's elevation (negative at night).
    pub sun_sin: f32,
    /// The actual sun's colour (for the sky disc), zero below the horizon.
    pub sun_colour: V3,
    pub exposure: f32,
    pub clouds: f32,
}

pub const ENV_W: u32 = 256;
pub const ENV_H: u32 = 128;
pub const ENV_MIPS: u32 = 6;
/// How bright the sun is against the sky model (diffuse = albedo * colour).
pub const SUN_GAIN: f32 = 3.4;

fn dir_of_texel(i: u32, j: u32, w: u32, h: u32) -> V3 {
    let az = ((i as f32 + 0.5) / w as f32 - 0.5) * std::f32::consts::TAU;
    let th = (j as f32 + 0.5) / h as f32 * std::f32::consts::PI;
    [th.sin() * az.sin(), th.cos(), th.sin() * az.cos()]
}

fn luma(c: V3) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

/// Studio's sky description for a world's sky.
pub fn sky_desc(s: &Sky) -> SkyDesc {
    let travel = s.sun_direction();
    let mut d = SkyDesc { sun: [travel[0], -travel[1], travel[2]], turbidity: s.turbidity, strength: s.strength, ground: s.ground, background: true, haze: 0.0, hdri: 0, hdri_turn: s.hdri_turn };
    if let Some(p) = &s.hdri
        && let Ok(k) = sky::load_hdri(p)
    {
        d.hdri = k;
    }
    d
}

/// A fingerprint of everything the environment depends on.
pub fn env_key(s: &Sky) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for v in [s.time, s.day, s.latitude, s.heading, s.turbidity, s.strength, s.clouds, s.hdri_turn, s.ground[0], s.ground[1], s.ground[2]] {
        v.to_bits().hash(&mut h);
    }
    s.hdri.hash(&mut h);
    h.finish()
}

pub fn build_env(s: &Sky, look_exposure: f32) -> EnvData {
    let desc = sky_desc(s);
    let world_radiance = |d: V3| -> V3 {
        let mut c = sky::radiance(&desc, [d[0], -d[1], d[2]]);
        if d[1] < 0.0 {
            // Below the horizon the haze carries on, then the ground takes over.
            let horizon = sky::radiance(&desc, norm([d[0], 0.001, d[2]]).map(|v| v));
            let _ = horizon;
            let h = sky::radiance(&desc, [d[0], -0.001, d[2]]);
            let k = ((-d[1]) / 0.55).clamp(0.0, 1.0);
            let k = k * k * (3.0 - 2.0 * k);
            c = [h[0] * 0.85 + (c[0] - h[0] * 0.85) * k, h[1] * 0.85 + (c[1] - h[1] * 0.85) * k, h[2] * 0.85 + (c[2] - h[2] * 0.85) * k];
        }
        if s.clouds > 0.0 {
            let l = luma(c);
            let grey = [l * 0.92, l * 0.95, l * 1.0];
            let k = s.clouds * 0.45;
            c = [c[0] + (grey[0] - c[0]) * k, c[1] + (grey[1] - c[1]) * k, c[2] + (grey[2] - c[2]) * k];
        }
        c
    };
    // Mip 0, then boxes.
    let mut levels: Vec<(u32, u32, Vec<[f32; 3]>)> = Vec::new();
    let (w, h) = (ENV_W, ENV_H);
    let mut base = Vec::with_capacity((w * h) as usize);
    for j in 0..h {
        for i in 0..w {
            base.push(world_radiance(dir_of_texel(i, j, w, h)));
        }
    }
    levels.push((w, h, base));
    for _ in 1..ENV_MIPS {
        let (pw, ph, prev) = levels.last().unwrap();
        let (nw, nh) = ((pw / 2).max(1), (ph / 2).max(1));
        let mut next = Vec::with_capacity((nw * nh) as usize);
        for j in 0..nh {
            for i in 0..nw {
                let mut acc = [0.0f32; 3];
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let p = prev[(((j * 2 + dy).min(ph - 1)) * pw + ((i * 2 + dx).min(pw - 1))) as usize];
                    for k in 0..3 {
                        acc[k] += p[k] * 0.25;
                    }
                }
                next.push(acc);
            }
        }
        levels.push((nw, nh, next));
    }
    let mips = levels
        .iter()
        .map(|(w, h, px)| {
            let mut bits = Vec::with_capacity(px.len() * 4);
            for p in px {
                bits.extend_from_slice(&[crate::gpu::f32_to_f16(p[0]), crate::gpu::f32_to_f16(p[1]), crate::gpu::f32_to_f16(p[2]), crate::gpu::f32_to_f16(1.0)]);
            }
            (*w, *h, bits)
        })
        .collect();
    // Spherical harmonics of the sky, convolved with the cosine lobe.
    let mut sh = [[0.0f32; 3]; 9];
    let n = 512;
    for i in 0..n {
        let z = 1.0 - 2.0 * (i as f32 + 0.5) / n as f32;
        let r = (1.0 - z * z).max(0.0).sqrt();
        let phi = i as f32 * 2.399_963;
        // y is up in the world: put the spiral's axis there.
        let d = [r * phi.cos(), z, r * phi.sin()];
        let c = world_radiance(d);
        let b = basis(d);
        for k in 0..9 {
            for ch in 0..3 {
                sh[k][ch] += c[ch] * b[k] * (4.0 * std::f32::consts::PI / n as f32);
            }
        }
    }
    let a = [std::f32::consts::PI, 2.094_395, 2.094_395, 2.094_395, 0.785_398, 0.785_398, 0.785_398, 0.785_398, 0.785_398];
    for k in 0..9 {
        for ch in 0..3 {
            sh[k][ch] *= a[k] / std::f32::consts::PI;
        }
    }
    // The light that casts shadows: the sun by day, the moon by night.
    let travel = s.sun_direction();
    let to_sun = mul(travel, -1.0);
    let sun_sin = to_sun[1];
    let cloud_dim = 1.0 - 0.7 * s.clouds;
    let sc = if desc.hdri != 0 { [0.0; 3] } else { sky::sun_colour(&desc) };
    let sun_colour = mul(sc, SUN_GAIN * cloud_dim);
    let (light_to, light_colour) = if sun_sin > -0.04 && luma(sun_colour) > 0.0 {
        (to_sun, sun_colour)
    } else {
        let gain = ((-0.04 - sun_sin) / 0.16).clamp(0.0, 1.0);
        let moon = norm([-to_sun[0], -to_sun[1] * 0.7 + 0.28, -to_sun[2]]);
        (moon, mul([0.16, 0.2, 0.32], 0.55 * s.strength * gain))
    };
    // Exposure so that a typical lit surface lands mid-grey.
    let irr_up = {
        let b = basis([0.0, 1.0, 0.0]);
        let mut c = [0.0f32; 3];
        for k in 0..9 {
            for ch in 0..3 {
                c[ch] += sh[k][ch] * b[k];
            }
        }
        luma(c)
    };
    let sun_l = if sun_sin > 0.0 { luma(sun_colour) } else { 0.0 };
    let moon_l = if sun_sin <= -0.04 { luma(light_colour) * 0.6 } else { 0.0 };
    let exposure = (0.62 / (0.55 * sun_l + irr_up + moon_l + 0.02)).clamp(0.2, 2.2) * 2f32.powf(look_exposure);
    EnvData { mips, sh, light_to, light_colour, sun_sin, sun_colour, exposure, clouds: s.clouds }
}

fn basis(d: V3) -> [f32; 9] {
    let (x, y, z) = (d[0], d[1], d[2]);
    [0.282_095, 0.488_603 * y, 0.488_603 * z, 0.488_603 * x, 1.092_548 * x * y, 1.092_548 * y * z, 0.315_392 * (3.0 * z * z - 1.0), 1.092_548 * x * z, 0.546_274 * (x * x - y * y)]
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct LightU {
    pub pos_range: [f32; 4],
    pub colour: [f32; 4],
    pub dir_cone: [f32; 4],
}

/// Mirrors `Frame` in common.wgsl.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct FrameU {
    pub view_proj: Mat4,
    pub view: Mat4,
    pub inv_view_proj: Mat4,
    pub cascade_vp: [Mat4; 4],
    pub cascade_splits: [f32; 4],
    pub cascade_texel: [f32; 4],
    pub eye: [f32; 4],
    pub view_eye: [f32; 4],
    pub sun_dir: [f32; 4],
    pub sun_colour: [f32; 4],
    pub fog: [f32; 4],
    pub fog_colour: [f32; 4],
    pub sky_misc: [f32; 4],
    pub sh: [[f32; 4]; 9],
    pub screen: [f32; 4],
    pub jitter: [f32; 4],
    pub clip: [f32; 4],
    pub flags: [f32; 4],
    pub counts: [u32; 4],
    pub lights: [LightU; 16],
}

/// A 4x4 inverse (general; the matrices here are well conditioned).
pub fn invert(m: &Mat4) -> Mat4 {
    // Column-major m[c][r] -> flat in the same order.
    let a: [f32; 16] = std::array::from_fn(|i| m[i / 4][i % 4]);
    let mut inv = [0.0f32; 16];
    inv[0] = a[5] * a[10] * a[15] - a[5] * a[11] * a[14] - a[9] * a[6] * a[15] + a[9] * a[7] * a[14] + a[13] * a[6] * a[11] - a[13] * a[7] * a[10];
    inv[4] = -a[4] * a[10] * a[15] + a[4] * a[11] * a[14] + a[8] * a[6] * a[15] - a[8] * a[7] * a[14] - a[12] * a[6] * a[11] + a[12] * a[7] * a[10];
    inv[8] = a[4] * a[9] * a[15] - a[4] * a[11] * a[13] - a[8] * a[5] * a[15] + a[8] * a[7] * a[13] + a[12] * a[5] * a[11] - a[12] * a[7] * a[9];
    inv[12] = -a[4] * a[9] * a[14] + a[4] * a[10] * a[13] + a[8] * a[5] * a[14] - a[8] * a[6] * a[13] - a[12] * a[5] * a[10] + a[12] * a[6] * a[9];
    inv[1] = -a[1] * a[10] * a[15] + a[1] * a[11] * a[14] + a[9] * a[2] * a[15] - a[9] * a[3] * a[14] - a[13] * a[2] * a[11] + a[13] * a[3] * a[10];
    inv[5] = a[0] * a[10] * a[15] - a[0] * a[11] * a[14] - a[8] * a[2] * a[15] + a[8] * a[3] * a[14] + a[12] * a[2] * a[11] - a[12] * a[3] * a[10];
    inv[9] = -a[0] * a[9] * a[15] + a[0] * a[11] * a[13] + a[8] * a[1] * a[15] - a[8] * a[3] * a[13] - a[12] * a[1] * a[11] + a[12] * a[3] * a[9];
    inv[13] = a[0] * a[9] * a[14] - a[0] * a[10] * a[13] - a[8] * a[1] * a[14] + a[8] * a[2] * a[13] + a[12] * a[1] * a[10] - a[12] * a[2] * a[9];
    inv[2] = a[1] * a[6] * a[15] - a[1] * a[7] * a[14] - a[5] * a[2] * a[15] + a[5] * a[3] * a[14] + a[13] * a[2] * a[7] - a[13] * a[3] * a[6];
    inv[6] = -a[0] * a[6] * a[15] + a[0] * a[7] * a[14] + a[4] * a[2] * a[15] - a[4] * a[3] * a[14] - a[12] * a[2] * a[7] + a[12] * a[3] * a[6];
    inv[10] = a[0] * a[5] * a[15] - a[0] * a[7] * a[13] - a[4] * a[1] * a[15] + a[4] * a[3] * a[13] + a[12] * a[1] * a[7] - a[12] * a[3] * a[5];
    inv[14] = -a[0] * a[5] * a[14] + a[0] * a[6] * a[13] + a[4] * a[1] * a[14] - a[4] * a[2] * a[13] - a[12] * a[1] * a[6] + a[12] * a[2] * a[5];
    inv[3] = -a[1] * a[6] * a[11] + a[1] * a[7] * a[10] + a[5] * a[2] * a[11] - a[5] * a[3] * a[10] - a[9] * a[2] * a[7] + a[9] * a[3] * a[6];
    inv[7] = a[0] * a[6] * a[11] - a[0] * a[7] * a[10] - a[4] * a[2] * a[11] + a[4] * a[3] * a[10] + a[8] * a[2] * a[7] - a[8] * a[3] * a[6];
    inv[11] = -a[0] * a[5] * a[11] + a[0] * a[7] * a[9] + a[4] * a[1] * a[11] - a[4] * a[3] * a[9] - a[8] * a[1] * a[7] + a[8] * a[3] * a[5];
    inv[15] = a[0] * a[5] * a[10] - a[0] * a[6] * a[9] - a[4] * a[1] * a[10] + a[4] * a[2] * a[9] + a[8] * a[1] * a[6] - a[8] * a[2] * a[5];
    let det = a[0] * inv[0] + a[1] * inv[4] + a[2] * inv[8] + a[3] * inv[12];
    let id = if det.abs() < 1e-30 { 0.0 } else { 1.0 / det };
    std::array::from_fn(|c| std::array::from_fn(|r| inv[c * 4 + r] * id))
}

/// Up to 16 lamps worth drawing, the ones lighting most of what is seen.
pub fn pick_lights(w: &World, eye: V3, frustum: &math::Frustum) -> Vec<LightU> {
    let mut ranked: Vec<(f32, &Light)> = w
        .lights
        .iter()
        .filter(|l| frustum.sphere(l.position, l.range))
        .map(|l| {
            let d = math::dist(l.position, eye);
            // Brighter and nearer first.
            (d - l.range, l)
        })
        .collect();
    ranked.sort_by(|a, b| a.0.total_cmp(&b.0));
    ranked
        .into_iter()
        .take(16)
        .map(|(_, l)| {
            let cone = if l.kind == LightKind::Spot { l.cone.to_radians().cos() } else { 0.0 };
            LightU { pos_range: [l.position[0], l.position[1], l.position[2], l.range], colour: [l.colour[0], l.colour[1], l.colour[2], 0.0], dir_cone: [l.direction[0], l.direction[1], l.direction[2], cone] }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_frame_block_has_the_size_the_shader_expects() {
        // 3 + 4 matrices, 14 other vec4s, 9 of SH and 16 lights of 3 vec4s.
        let expect = 7 * 64 + 14 * 16 + 9 * 16 + 16 * 48;
        assert_eq!(std::mem::size_of::<FrameU>(), expect);
    }

    #[test]
    fn matrices_invert() {
        let v = View { position: [3.0, 4.0, -5.0], yaw: 30.0, pitch: -12.0, roll: 0.0, fov: 60.0, near: 0.1, far: 500.0 };
        let m = mat_mul(&v.proj_matrix(1.7, [0.0; 2]), &v.view_matrix());
        let i = invert(&m);
        let id = mat_mul(&m, &i);
        for c in 0..4 {
            for r in 0..4 {
                let want = if c == r { 1.0 } else { 0.0 };
                assert!((id[c][r] - want).abs() < 2e-3, "{c},{r}: {}", id[c][r]);
            }
        }
        // The eye unprojects from the near plane's centre line.
        let p = math::mat_vec(&i, [0.0, 0.0, 0.5, 1.0]);
        let q = [p[0] / p[3], p[1] / p[3], p[2] / p[3]];
        assert!(dot(norm(sub(q, v.position)), v.forward()) > 0.999);
    }

    #[test]
    fn cascades_cover_the_view_in_growing_steps_and_hold_still_between_texels() {
        let v = View { position: [0.0, 2.0, 0.0], yaw: 0.0, pitch: -5.0, roll: 0.0, fov: 60.0, near: 0.1, far: 1000.0 };
        let sun = norm([0.3, -0.8, 0.4]);
        let c = cascades(&v, 16.0 / 9.0, sun, 4, 200.0, 2048, 120.0);
        assert_eq!(c.len(), 4);
        assert!(c.windows(2).all(|w| w[0].split < w[1].split && w[0].texel < w[1].texel));
        assert!((c[3].split - 200.0).abs() < 1.0);
        // A point in the view lands inside the map of the cascade for its depth.
        let target = add(v.position, mul(v.forward(), 12.0));
        let lp = math::mat_vec(&c[1].vp, [target[0], target[1], target[2], 1.0]);
        assert!(lp[0].abs() < 1.0 && lp[1].abs() < 1.0 && lp[2] > 0.0 && lp[2] < 1.0, "{lp:?}");
        // A tiny shift of the camera moves the light's picture by whole texels only.
        let v2 = View { position: add(v.position, [0.003, 0.0, 0.0]), ..v };
        let c2 = cascades(&v2, 16.0 / 9.0, sun, 4, 200.0, 2048, 120.0);
        let probe = [5.0, 0.0, 9.0, 1.0];
        let a = math::mat_vec(&c[0].vp, probe);
        let b = math::mat_vec(&c2[0].vp, probe);
        let texel_ndc = 2.0 / 2048.0;
        let shift = ((a[0] - b[0]) / texel_ndc).abs();
        assert!(shift < 1e-2 || (shift - shift.round()).abs() < 1e-2, "{shift}");
    }

    #[test]
    fn the_sky_is_bright_by_day_dark_at_night_and_the_moon_takes_over() {
        let day = build_env(&Sky { time: 12.5, ..Sky::default() }, 0.0);
        let night = build_env(&Sky { time: 23.5, ..Sky::default() }, 0.0);
        let dusk = build_env(&Sky { time: 19.0, ..Sky::default() }, 0.0);
        assert!(day.sun_sin > 0.7 && luma(day.light_colour) > 1.0, "{} {:?}", day.sun_sin, day.light_colour);
        assert!(night.sun_sin < -0.2 && luma(night.light_colour) < 0.25 && luma(night.light_colour) > 0.02, "moonlight: {:?}", night.light_colour);
        assert!(night.light_to[1] > 0.1, "the moon is up");
        assert!(day.mips.len() == ENV_MIPS as usize && day.mips[0].2.len() == (ENV_W * ENV_H * 4) as usize);
        // Dusk light is redder than noon light.
        assert!(dusk.sun_colour[0] / dusk.sun_colour[2].max(1e-4) > day.sun_colour[0] / day.sun_colour[2].max(1e-4));
        // Darker nights are exposed up; exposure stays in range.
        assert!(night.exposure > day.exposure * 2.0, "{} vs {}", night.exposure, day.exposure);
        // Overcast dims the sun.
        let grey = build_env(&Sky { time: 12.5, clouds: 1.0, ..Sky::default() }, 0.0);
        assert!(luma(grey.light_colour) < luma(day.light_colour) * 0.5);
        assert_ne!(env_key(&Sky::default()), env_key(&Sky { time: 9.0, ..Sky::default() }));
    }
}
