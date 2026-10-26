// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The air and look pass on the CPU: the reference, line for line what
//! `atmosphere.wgsl` does on the GPU.
//!
//! Five stages over a picture and its depth: [`sprites`] (glow, lens
//! streaks, flames), [`reflect`] (wet surfaces), [`air`] (fog, shafts,
//! weather, ripples), [`bloom`] and [`finish`] (bloom added, exposure,
//! tone map, grade, vignette, grain).

use super::*;

// Shared with the shader.
pub const AIR_STEPS: usize = 32;
pub const SSR_STEPS: usize = 40;
pub const BEAM_RHO: f32 = 0.16;
pub const SCATTER_GAIN: f32 = 0.18;
pub const BLOOM_LEVELS: usize = 5;
/// Light the air adds is left alone up to this and eases toward
/// [`AIR_CAP`] above it, so lamps lined up down a foggy street cannot
/// whiten the picture.
pub const AIR_KNEE: f32 = 1.0;
pub const AIR_CAP: f32 = 2.5;
/// Brightest a pixel is taken to be when it feeds the bloom: a glow at ten
/// times white spills no more than one at six.
pub const BLOOM_CLAMP: f32 = 6.0;
const BIG: f32 = 1.0e30;

type V = [f32; 3];

#[inline]
fn add(a: V, b: V) -> V {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
#[inline]
fn sub(a: V, b: V) -> V {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
#[inline]
fn mul(a: V, k: f32) -> V {
    [a[0] * k, a[1] * k, a[2] * k]
}
#[inline]
fn mulv(a: V, b: V) -> V {
    [a[0] * b[0], a[1] * b[1], a[2] * b[2]]
}
#[inline]
fn dot(a: V, b: V) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
#[inline]
fn len(a: V) -> f32 {
    dot(a, a).sqrt()
}
#[inline]
fn norm(a: V) -> V {
    mul(a, 1.0 / len(a).max(1e-20))
}
#[inline]
fn v3(p: [f32; 4]) -> V {
    [p[0], p[1], p[2]]
}
#[inline]
fn mix(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}
#[inline]
fn clamp01(x: f32) -> f32 {
    x.clamp(0.0, 1.0)
}
#[inline]
fn smooth(e0: f32, e1: f32, x: f32) -> f32 {
    let t = clamp01((x - e0) / (e1 - e0));
    t * t * (3.0 - 2.0 * t)
}
#[inline]
fn fract(x: f32) -> f32 {
    x - x.floor()
}
#[inline]
fn lum(c: V) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

// ---------------------------------------------------------------- noise

#[inline]
fn hash(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^= x >> 16;
    x
}
#[inline]
fn hash3(a: i32, b: i32, c: i32, s: u32) -> u32 {
    let mut h = hash(s ^ 0x9e37_79b9);
    h = hash(h ^ (a as u32));
    h = hash(h ^ (b as u32).wrapping_mul(0x85eb_ca6b));
    hash(h ^ (c as u32).wrapping_mul(0xc2b2_ae35))
}
/// Interleaved gradient noise: a per-pixel offset with no visible clumping.
#[inline]
fn ign(x: usize, y: usize) -> f32 {
    fract(52.982_918 * fract(0.067_110_56 * x as f32 + 0.005_837_15 * y as f32))
}

#[inline]
fn u01(h: u32) -> f32 {
    (h >> 8) as f32 * (1.0 / 16_777_216.0)
}

fn noise3(p: V) -> f32 {
    let (ix, iy, iz) = (p[0].floor(), p[1].floor(), p[2].floor());
    let (fx, fy, fz) = (p[0] - ix, p[1] - iy, p[2] - iz);
    let (ux, uy, uz) = (fx * fx * (3.0 - 2.0 * fx), fy * fy * (3.0 - 2.0 * fy), fz * fz * (3.0 - 2.0 * fz));
    let (ax, ay, az) = (ix as i32, iy as i32, iz as i32);
    let h = |dx: i32, dy: i32, dz: i32| u01(hash3(ax + dx, ay + dy, az + dz, 77));
    let x00 = mix(h(0, 0, 0), h(1, 0, 0), ux);
    let x10 = mix(h(0, 1, 0), h(1, 1, 0), ux);
    let x01 = mix(h(0, 0, 1), h(1, 0, 1), ux);
    let x11 = mix(h(0, 1, 1), h(1, 1, 1), ux);
    mix(mix(x00, x10, uy), mix(x01, x11, uy), uz)
}

pub fn fbm3(p: V) -> f32 {
    noise3(p) * 0.5333 + noise3(mul(p, 2.03)) * 0.2667 + noise3(mul(p, 4.1)) * 0.1333 + noise3(mul(p, 8.3)) * 0.0667
}

// ---------------------------------------------------------------- context

struct Ctx<'a> {
    p: &'a [[f32; 4]],
    shadows: &'a [f32],
    depth: &'a [f32],
    surf: &'a [[f32; 4]],
    w: usize,
    h: usize,
    time: f32,
    metre: f32,
    o: V,
    zoom: f32,
    right: V,
    down: V,
    fwd: V,
    centre: [f32; 2],
    n_light: usize,
    n_weather: usize,
    flags: u32,
}

impl<'a> Ctx<'a> {
    fn new(plan: &'a Plan, pic: &'a Picture) -> Ctx<'a> {
        let p = &plan.params[..];
        Ctx {
            p,
            shadows: &plan.shadows,
            depth: pic.depth,
            surf: pic.surface,
            w: pic.width,
            h: pic.height,
            time: p[0][2],
            metre: p[0][3],
            o: v3(p[1]),
            zoom: p[1][3],
            right: v3(p[2]),
            down: v3(p[3]),
            fwd: v3(p[4]),
            centre: [p[2][3], p[3][3]],
            n_light: p[5][0] as usize,
            n_weather: p[5][1] as usize,
            flags: p[5][3] as u32,
        }
    }

    fn dir(&self, x: f32, y: f32) -> V {
        let u = (x - self.centre[0]) / self.zoom;
        let v = (y - self.centre[1]) / self.zoom;
        norm(add(add(mul(self.right, u), mul(self.down, v)), self.fwd))
    }

    fn project(&self, q: V) -> Option<(f32, f32, f32)> {
        let v = sub(q, self.o);
        let z = dot(v, self.fwd);
        if z <= crate::space::NEAR {
            return None;
        }
        Some((self.centre[0] + self.zoom * dot(v, self.right) / z, self.centre[1] + self.zoom * dot(v, self.down) / z, z))
    }

    fn lp(&self, i: usize) -> &'a [[f32; 4]] {
        &self.p[P_PRACTICAL + i * PRACTICAL_STRIDE..P_PRACTICAL + (i + 1) * PRACTICAL_STRIDE]
    }

    fn wp(&self, i: usize) -> &'a [[f32; 4]] {
        &self.p[P_WEATHER + i * WEATHER_STRIDE..P_WEATHER + (i + 1) * WEATHER_STRIDE]
    }

    fn fog_on(&self) -> bool {
        self.flags & 1 != 0
    }

    /// Fog thickness per metre at a world point.
    fn fog_rho(&self, q: V) -> f32 {
        let f = self.p[P_FOG];
        if f[0] <= 0.0 {
            return 0.0;
        }
        let hm = (f[2] - q[1]) / self.metre;
        let mut rho = f[0] * (-f[1] * hm.clamp(-30.0, 60.0)).exp();
        let patch = self.p[P_FOG + 2][3];
        if patch > 0.0 {
            let n = fbm3(mul(q, 1.0 / (7.0 * self.metre)));
            rho *= mix(1.0, smooth(0.25, 0.75, n) * 1.8, patch);
        }
        rho
    }

    /// Optical depth of the smooth fog along the ray `o + d·t` (t in px).
    fn fog_tau(&self, d: V, t: f32) -> f32 {
        let f = self.p[P_FOG];
        if f[0] <= 0.0 {
            return 0.0;
        }
        let tm = t / self.metre;
        let h0 = (f[2] - self.o[1]) / self.metre;
        let base = f[0] * (-f[1] * h0.clamp(-30.0, 60.0)).exp();
        let a = f[1] * d[1];
        if a.abs() < 1e-4 {
            return base * tm;
        }
        base * ((a * tm).clamp(-30.0, 30.0).exp() - 1.0) / a
    }

    /// Light `i` seen from `q`: unit direction to it, colour after falloff
    /// and cone, distance (px), the cone factor alone.
    fn light_at(&self, i: usize, q: V) -> (V, V, f32, f32) {
        let l = self.lp(i);
        let col = v3(l[1]);
        if l[0][3] > 1.5 {
            // Parallel: travels along `direction`.
            let to = mul(norm(v3(l[2])), -1.0);
            return (to, col, BIG, 1.0);
        }
        let d = sub(v3(l[0]), q);
        let r = len(d).max(1e-3);
        let to = mul(d, 1.0 / r);
        let reach = self.lp(i)[5][2].max(1.0);
        let fo = self.lp(i)[6][1];
        let atten = if fo > 0.0 { 1.0 - smooth(self.lp(i)[6][0], self.lp(i)[6][0] + fo, r) } else { 1.0 / (1.0 + (r / reach) * (r / reach)) };
        let cone = if l[0][3] > 0.5 { smooth(l[2][3], l[3][0], dot(mul(to, -1.0), norm(v3(l[2])))) } else { 1.0 };
        (to, mul(col, atten * cone), r, cone)
    }

    /// Shadow-map lookup (1 = lit) for a world point.
    fn shadow(&self, slot: f32, q: V) -> f32 {
        if slot < 0.0 {
            return 1.0;
        }
        let off = slot as usize;
        let s = self.shadows;
        let pos = [s[off], s[off + 1], s[off + 2]];
        let zoom = s[off + 3];
        let ax = [[s[off + 4], s[off + 5], s[off + 6]], [s[off + 8], s[off + 9], s[off + 10]], [s[off + 12], s[off + 13], s[off + 14]]];
        let c = [s[off + 7], s[off + 11]];
        let ortho = s[off + 15] > 0.5;
        let size = s[off + 16] as i64;
        let texel = s[off + 17];
        let base = s[off + 18] as usize;
        let dq = sub(q, pos);
        let v = [dot(ax[0], dq), dot(ax[1], dq), dot(ax[2], dq)];
        if !ortho && v[2] <= 1.0 {
            return 1.0;
        }
        let (sx, sy) = if ortho { (c[0] + zoom * v[0], c[1] + zoom * v[1]) } else { (c[0] + zoom * v[0] / v[2], c[1] + zoom * v[1] / v[2]) };
        let tx = if ortho { texel } else { texel * v[2] };
        let bias = tx * 2.5 + 0.5;
        let mut lit = 0.0;
        for (dx, dy) in [(0i64, 0i64), (1, 0), (-1, 0), (0, 1), (0, -1)] {
            let x = (sx - 0.5).round() as i64 + dx;
            let y = (sy - 0.5).round() as i64 + dy;
            if x < 0 || y < 0 || x >= size || y >= size {
                lit += 1.0;
                continue;
            }
            let d = s[base + (y * size + x) as usize];
            if v[2] - bias <= d {
                lit += 1.0;
            }
        }
        lit / 5.0
    }

    fn at(&self, x: i64, y: i64) -> usize {
        (y.clamp(0, self.h as i64 - 1) as usize) * self.w + x.clamp(0, self.w as i64 - 1) as usize
    }
}

/// Eases light that is above [`AIR_KNEE`] toward [`AIR_CAP`], keeping its colour.
fn soft_cap(c: V) -> V {
    let m = c[0].max(c[1]).max(c[2]);
    if m <= AIR_KNEE {
        return c;
    }
    let room = AIR_CAP - AIR_KNEE;
    mul(c, (AIR_KNEE + room * (1.0 - (-(m - AIR_KNEE) / room).exp())) / m)
}

/// Henyey–Greenstein scattering for cosine `c` and anisotropy `g`,
/// normalised so the isotropic case is 1.
fn hg(c: f32, g: f32) -> f32 {
    let g2 = g * g;
    (1.0 - g2) / (1.0 + g2 - 2.0 * g * c).max(1e-3).powf(1.5)
}

// ---------------------------------------------------------------- sprites

/// A flame in screen coordinates `u` (across, in widths) and `v` (up, in
/// heights): colour and how much of the pixel it covers.
fn flame_at(u: f32, v: f32, time: f32, seed: f32, tongues: f32) -> (V, f32) {
    let n = tongues.round().clamp(1.0, 5.0);
    let mut occ_best = 0.0f32;
    let mut heat_best = 0.0f32;
    let mut blue_best = 0.0f32;
    let mut j = 0.0f32;
    while j < n {
        let o = if n > 1.5 { ((j + 0.5) / n - 0.5) * 1.5 } else { 0.0 };
        let hj = if n > 1.5 { 1.0 - 0.38 * (o.abs() / 0.75) - 0.18 * u01(hash(j as u32 + (seed * 97.0) as u32)) } else { 1.0 };
        let wj = if n > 1.5 { 1.25 / n.sqrt() } else { 1.0 };
        let vv = v / hj;
        if vv > -0.02 && vv < 1.1 {
            let phase = seed * 6.283 + j * 2.1;
            let sway = (time * (4.3 + 1.7 * j) + vv * 3.1 + phase).sin() * 0.16 * vv.max(0.0).powf(1.4) * wj + (noise3([vv * 2.5 + seed, time * 2.4 + j * 7.0, j]) - 0.5) * 0.34 * vv.max(0.0);
            let uu = (u - o - sway) / wj;
            let prof = (std::f32::consts::PI * vv.clamp(0.0, 1.0).powf(0.6)).sin();
            let hw = (0.5 * prof * (1.0 - 0.25 * vv)).max(1e-3);
            let q = uu.abs() / hw;
            let occ = (1.0 - smooth(0.62, 1.0, q)) * smooth(1.08, 0.88, vv) * smooth(-0.02, 0.03, vv);
            let heat = occ * (1.0 - q.min(1.0).powf(1.4)) * (1.0 - 0.5 * vv);
            let blue = smooth(0.2, 0.0, vv) * (1.0 - q.min(1.0)) * occ;
            if occ > occ_best {
                occ_best = occ;
            }
            heat_best = heat_best.max(heat);
            blue_best = blue_best.max(blue);
        }
        j += 1.0;
    }
    let heat = clamp01(heat_best * 1.25);
    // Dark red at the edge to white-yellow in the core.
    let c0 = [1.0, 0.1, 0.02];
    let c1 = [1.0, 0.42, 0.07];
    let c2 = [1.0, 0.78, 0.28];
    let c3 = [1.0, 0.95, 0.72];
    let ramp = if heat < 0.33 {
        let t = heat / 0.33;
        [mix(c0[0], c1[0], t), mix(c0[1], c1[1], t), mix(c0[2], c1[2], t)]
    } else if heat < 0.66 {
        let t = (heat - 0.33) / 0.33;
        [mix(c1[0], c2[0], t), mix(c1[1], c2[1], t), mix(c1[2], c2[2], t)]
    } else {
        let t = (heat - 0.66) / 0.34;
        [mix(c2[0], c3[0], t), mix(c2[1], c3[1], t), mix(c2[2], c3[2], t)]
    };
    let k = 0.7 + 4.5 * heat * heat;
    let col = add(mul(ramp, k), mul([0.15, 0.3, 1.0], blue_best * 0.9));
    (col, occ_best)
}

/// Glow, lens streak and flame of one light at pixel `(x, y)`: light to
/// add and the share of the picture a flame covers.
fn sprite_pixel(c: &Ctx, x: usize, y: usize, zbuf: f32) -> (V, f32, V) {
    let px = x as f32 + 0.5;
    let py = y as f32 + 0.5;
    let mut add_c = [0.0f32; 3];
    let mut cover = 0.0f32;
    let mut cover_col = [0.0f32; 3];
    for i in 0..c.n_light {
        let l = c.lp(i);
        let glow = l[3][1];
        let flare = l[3][2];
        let fh = l[4][0];
        if glow <= 0.0 && flare <= 0.0 && fh <= 0.0 {
            continue;
        }
        let Some((sx, sy, z)) = c.project(v3(l[0])) else { continue };
        let vis = l[5][0];
        let dx = px - sx;
        let dy = py - sy;
        let maxc = l[1][0].max(l[1][1]).max(l[1][2]);
        let hue = mul(v3(l[1]), 1.0 / maxc.max(0.05));
        let a = (l[1][3] * c.zoom / z).max(1.2);
        let dist_m = z / c.metre;
        let far = 1.0 / (1.0 + (dist_m / 30.0) * (dist_m / 30.0));
        let mut facing = 1.0;
        if l[0][3] > 0.5 && l[0][3] < 1.5 {
            let to_cam = norm(sub(c.o, v3(l[0])));
            facing = 0.35 + 0.65 * smooth(-0.2, 0.8, dot(norm(v3(l[2])), to_cam));
        }
        let gi = 2.2 * (maxc + 0.1).sqrt();
        if glow > 0.0 {
            let rr = (dx * dx + dy * dy).sqrt();
            let ah = (a * 3.5).max(0.012 * c.h as f32 * (0.5 + 0.5 * glow.min(2.0)));
            let core = (-(rr / a) * (rr / a)).exp();
            let q = rr / ah;
            let halo = 1.0 / (1.0 + q * q).powf(1.5);
            let e = vis * facing * far * glow * (core * 6.0 + halo * 0.6) * gi;
            add_c = add(add_c, mul(hue, e));
        }
        if flare > 0.0 {
            let streak = (-(dy.abs()) / (a * 0.5 + 0.5)).exp() / (1.0 + (dx.abs() / (a * 14.0 + 20.0)) * (dx.abs() / (a * 14.0 + 20.0)));
            let e = vis * facing * far * flare * streak * 0.5 * gi;
            add_c = add(add_c, mul(hue, e));
        }
        if fh > 0.0 {
            let ws = l[4][1] * c.zoom / z;
            let hs = fh * c.zoom / z;
            // Base of the flame below the light's centre.
            let by = sy + 0.5 * hs;
            let u = dx / ws.max(0.5);
            let v = (by - py) / hs.max(0.5);
            if u.abs() < 1.6 && v > -0.05 && v < 1.15 {
                let (fc, occ) = flame_at(u, v, c.time, l[4][2], l[4][3]);
                // Surfaces nearer than the flame hide it.
                let bias = (0.5 * fh).max(2.0);
                let o = occ * clamp01((zbuf - (z - 2.0 * bias)) / bias);
                if o > cover {
                    cover = o;
                    cover_col = fc;
                }
            }
        }
    }
    (add_c, cover, cover_col)
}

/// Stage 1: draws sources over the picture.
pub fn sprites(plan: &Plan, pic: &Picture, col: &mut [[f32; 4]]) {
    let c = Ctx::new(plan, pic);
    par_rows(col, pic.width, |y, row| {
        for (x, px) in row.iter_mut().enumerate() {
            let zbuf = c.depth[y * c.w + x];
            let (add_c, cover, cover_col) = sprite_pixel(&c, x, y, zbuf);
            let k = 1.0 - 0.55 * cover;
            px[0] = px[0] * k + add_c[0] + cover_col[0] * cover;
            px[1] = px[1] * k + add_c[1] + cover_col[1] * cover;
            px[2] = px[2] * k + add_c[2] + cover_col[2] * cover;
            px[3] = px[3].max(cover * 0.85);
        }
    });
}

// ---------------------------------------------------------------- reflect

/// What a mirror sees in direction `r` where nothing is on screen: the sky
/// and the lights themselves, smeared by roughness `spread` (radians,
/// stretched `stretch` times along the vertical).
fn mirror_miss(c: &Ctx, q: V, r: V, n: V, spread: f32, stretch: f32) -> V {
    let up_k = clamp01(-r[1] * 3.0 + 0.3);
    let mut out = mul(v3(c.p[P_FOG + 3]), 0.35 + 0.65 * up_k);
    for i in 0..c.n_light {
        let l = c.lp(i);
        if l[3][1] <= 0.0 && l[4][0] <= 0.0 {
            continue;
        }
        let to = sub(v3(l[0]), q);
        let dist = len(to).max(1.0);
        let lv = mul(to, 1.0 / dist);
        if dot(lv, n) <= 0.02 {
            continue;
        }
        let delta = sub(r, lv);
        let dv = dot(delta, [0.0, -1.0, 0.0]);
        let dh = len(sub(delta, mul([0.0, -1.0, 0.0], dv)));
        let a = (l[1][3].max(0.08 * c.metre) / dist).max(0.004);
        let s = a + spread;
        let ks = s * stretch;
        let kern = (-(dh * dh) / (s * s) - (dv * dv) / (ks * ks)).exp() + 0.18 / (1.0 + (dh * dh + dv * dv / (stretch * stretch)) / (s * s * 16.0)).powf(1.5);
        let maxc = l[1][0].max(l[1][1]).max(l[1][2]);
        let fh = l[4][0] > 0.0;
        let hue = if fh { [1.0, 0.55, 0.15] } else { mul(v3(l[1]), 1.0 / maxc.max(0.05)) };
        let gain = if fh { 5.0 } else { 5.3 * (maxc + 0.1).sqrt() * l[3][1].max(0.5) };
        let far = 1.0 / (1.0 + (dist / c.metre / 30.0) * (dist / c.metre / 30.0));
        out = add(out, mul(hue, kern * gain * far * l[5][0] * 0.55 * (a / s).clamp(0.25, 1.0)));
    }
    out
}

/// Stage 2: wet surfaces reflect.
pub fn reflect(plan: &Plan, pic: &Picture, col: &mut [[f32; 4]]) {
    let c = Ctx::new(plan, pic);
    let src: Vec<[f32; 4]> = col.to_vec();
    par_rows(col, pic.width, |y, row| {
        for (x, px) in row.iter_mut().enumerate() {
            let idx = y * c.w + x;
            let s = c.surf[idx];
            let refl = s[3];
            let zs = c.depth[idx];
            if refl < 0.01 || !zs.is_finite() {
                continue;
            }
            let d = c.dir(x as f32 + 0.5, y as f32 + 0.5);
            let t = zs / dot(d, c.fwd);
            let q = add(c.o, mul(d, t));
            let mut n = norm([s[0], s[1], s[2]]);
            if dot(n, d) > 0.0 {
                n = mul(n, -1.0);
            }
            let ndv = (-dot(n, d)).max(0.02);
            let f = 0.04 + 0.96 * (1.0 - ndv).powi(5);
            let rr = refl * (0.10 + 0.90 * f);
            let r = sub(d, mul(n, 2.0 * dot(d, n)));
            let spread = (1.0 - refl) * 0.12 + 0.004;
            let stretch = 1.0 + 7.0 * (1.0 - refl);
            let jit = 0.5f32;
            let t0 = 3.0f32;
            let tmax = 70.0 * c.metre;
            let mut hit: Option<(usize, usize)> = None;
            let mut prev = t0;
            for i in 0..SSR_STEPS {
                let tau = t0 * (tmax / t0).powf((i as f32 + jit) / SSR_STEPS as f32);
                let qq = add(q, mul(r, tau));
                let Some((sx, sy, z)) = c.project(qq) else { break };
                if sx < 0.0 || sy < 0.0 || sx >= c.w as f32 || sy >= c.h as f32 {
                    break;
                }
                let dz = c.depth[c.at(sx as i64, sy as i64)];
                if z > dz && z - dz < 0.08 * z + 6.0 {
                    // Refine between the last two steps.
                    let (mut lo, mut hi) = (prev, tau);
                    for _ in 0..5 {
                        let mid = 0.5 * (lo + hi);
                        let qm = add(q, mul(r, mid));
                        let ok = c.project(qm).is_some_and(|(mx, my, mz)| mx >= 0.0 && my >= 0.0 && mx < c.w as f32 && my < c.h as f32 && mz > c.depth[c.at(mx as i64, my as i64)]);
                        if ok {
                            hi = mid;
                        } else {
                            lo = mid;
                        }
                    }
                    let qh = add(q, mul(r, hi));
                    if let Some((hx, hy, _)) = c.project(qh) {
                        hit = Some((hx.clamp(0.0, c.w as f32 - 1.0) as usize, hy.clamp(0.0, c.h as f32 - 1.0) as usize));
                    }
                    break;
                }
                prev = tau;
            }
            let seen: V = match hit {
                Some((hx, hy)) => {
                    // Blurred by roughness, mostly along the vertical.
                    let sp = ((1.0 - refl) * 0.012 * c.h as f32).max(0.0);
                    let mut acc = [0.0f32; 3];
                    let mut wsum = 0.0;
                    for (dx, dy, wt) in [(0.0f32, 0.0f32, 1.0f32), (0.0, 1.0, 0.7), (0.0, -1.0, 0.7), (0.0, 2.0, 0.4), (0.0, -2.0, 0.4), (0.5, 0.0, 0.5), (-0.5, 0.0, 0.5)] {
                        let i2 = c.at(hx as i64 + (dx * sp).round() as i64, hy as i64 + (dy * sp).round() as i64);
                        let p2 = src[i2];
                        acc = add(acc, mul([p2[0], p2[1], p2[2]], wt));
                        wsum += wt;
                    }
                    let b = mul(acc, 1.0 / wsum);
                    // Fade at the picture's edge, where the reflection runs out.
                    let ex = (hx as f32).min(c.w as f32 - 1.0 - hx as f32) / (0.06 * c.w as f32);
                    let ey = (hy as f32).min(c.h as f32 - 1.0 - hy as f32) / (0.06 * c.h as f32);
                    let e = clamp01(ex.min(ey));
                    add(mul(b, e), mul(mirror_miss(&c, q, r, n, spread, stretch), (1.0 - e) + 0.5 * e))
                }
                None => mirror_miss(&c, q, r, n, spread, stretch),
            };
            px[0] = px[0] * (1.0 - rr) + seen[0] * rr;
            px[1] = px[1] * (1.0 - rr) + seen[1] * rr;
            px[2] = px[2] * (1.0 - rr) + seen[2] * rr;
        }
    });
}

// ---------------------------------------------------------------- air

/// Light on a particle at `q` seen along `d`: ambient plus every light
/// with forward scattering `g`.
fn particle_light(c: &Ctx, q: V, d: V, amb_k: f32, g: f32) -> V {
    let mut out = mul(v3(c.p[P_FOG + 2]), amb_k);
    for i in 0..c.n_light {
        let (to, lc, _, _) = c.light_at(i, q);
        let ph = hg(dot(to, d), g);
        out = add(out, mul(lc, ph * 0.9));
    }
    out
}

/// One layer of weather along a ray: light to add, share of the picture
/// covered (opaque / soft modes).
fn weather_ray(c: &Ctx, wi: usize, d: V, t_surf: f32, x: usize, y: usize) -> (V, V, f32) {
    let w = c.wp(wi);
    let mode = w[0][0];
    let prob = w[0][1];
    let cell = w[0][2].max(1.0);
    let size = w[0][3];
    let vel = v3(w[1]);
    let streak = w[1][3];
    let tint = v3(w[2]);
    let bright = w[2][3];
    let flutter = w[3][0];
    let hz = w[3][1];
    let maxd = w[3][2];
    let amb_k = w[3][3];
    let zon = w[4][3] > 0.5;
    let zlo = v3(w[4]);
    let zhi = v3(w[5]);
    let g = w[6][0];
    let step = (cell * 0.9).max(maxd / 48.0);
    let n = ((maxd / step).ceil() as usize).clamp(1, 48);
    let jit = fract(ign(x, y) + 0.618_034 * wi as f32);
    let mut add_c = [0.0f32; 3];
    let mut over_c = [0.0f32; 3];
    let mut over_a = 0.0f32;
    let seed = (wi as u32 + 1) * 1013;
    for i in 0..n {
        let t = (i as f32 + jit) * step + 0.3 * c.metre;
        if t > t_surf || t > maxd {
            break;
        }
        let q = add(c.o, mul(d, t));
        if zon && (q[0] < zlo[0] || q[1] < zlo[1] || q[2] < zlo[2] || q[0] > zhi[0] || q[1] > zhi[1] || q[2] > zhi[2]) {
            continue;
        }
        let cx = (q[0] / cell).floor() as i32;
        let cy = (q[1] / cell).floor() as i32;
        let cz = (q[2] / cell).floor() as i32;
        for k in 0..3u32 {
            let hh = hash3(cx, cy, cz, seed + k * 7919);
            if u01(hh) > prob {
                continue;
            }
            let r1 = u01(hash(hh ^ 0x1234_5678));
            let r2 = u01(hash(hh ^ 0x2345_6789));
            let r3 = u01(hash(hh ^ 0x3456_789a));
            let r4 = u01(hash(hh ^ 0x4567_89ab));
            let tm = c.time;
            let pos = [
                (cx as f32 + fract(r1 + vel[0] * tm / cell)) * cell + flutter * (6.2832 * (hz * tm + r4)).sin(),
                (cy as f32 + fract(r2 + vel[1] * tm / cell)) * cell,
                (cz as f32 + fract(r3 + vel[2] * tm / cell)) * cell + flutter * (6.2832 * (hz * tm * 0.83 + r4 * 1.7)).cos(),
            ];
            let b = sub(pos, mul(vel, streak));
            // Closest approach of the ray to the segment b..pos.
            let e = sub(pos, b);
            let wv = sub(b, c.o);
            let ee = dot(d, wv);
            let u = sub(wv, mul(d, ee));
            let v = sub(e, mul(d, dot(e, d)));
            let vv = dot(v, v);
            let s = if vv > 1e-9 { clamp01(-dot(u, v) / vv) } else { 0.0 };
            let off = add(u, mul(v, s));
            let dist = len(off);
            let th = ee + s * dot(e, d);
            if th < 0.2 * c.metre || th > t_surf || th > maxd {
                continue;
            }
            let foot = th / c.zoom * 0.6;
            let width = size.max(foot);
            let (cov, energy) = if mode > 2.5 { ((-(dist / width) * (dist / width)).exp(), 1.0) } else { (clamp01(1.0 - dist / width), (size / width).min(1.0)) };
            let cov = cov * cov;
            if cov < 0.004 {
                continue;
            }
            let at = add(c.o, mul(d, th));
            let tr = (-c.fog_tau(d, th)).exp();
            let fade = (1.0 - smooth(0.7 * maxd, maxd, th)) * smooth(0.4 * c.metre, 2.0 * c.metre, th);
            let lit = if mode > 0.5 && mode < 1.5 {
                let tw = 0.65 + 0.35 * (6.2832 * (tm * (1.5 + 2.0 * r4) + r1 * 9.0)).sin();
                mul(mulv(tint, [1.0; 3]), bright * tw)
            } else {
                mulv(tint, mul(particle_light(c, at, d, amb_k, g), bright))
            };
            if mode > 1.5 {
                let a = cov * energy * if mode > 2.5 { 0.30 } else { 0.95 };
                over_c = add(over_c, mul(mul(lit, tr * fade), a * (1.0 - over_a)));
                over_a += a * (1.0 - over_a);
            } else {
                add_c = add(add_c, mul(lit, cov * energy * tr * fade));
            }
        }
    }
    (add_c, over_c, over_a)
}

/// Ripples where rain lands on the ground.
fn ripples(c: &Ctx, q: V, d: V, amount: f32) -> V {
    let cell = 0.5 * c.metre;
    let cx = (q[0] / cell).floor() as i32;
    let cz = (q[2] / cell).floor() as i32;
    let mut ring = 0.0f32;
    for dz in -1..=1 {
        for dx in -1..=1 {
            let hh = hash3(cx + dx, 0, cz + dz, 4242);
            if u01(hh) > 0.55 * amount {
                continue;
            }
            let ox = (cx + dx) as f32 * cell + (0.2 + 0.6 * u01(hash(hh ^ 11))) * cell;
            let oz = (cz + dz) as f32 * cell + (0.2 + 0.6 * u01(hash(hh ^ 22))) * cell;
            let ph = fract(u01(hash(hh ^ 33)) + c.time * (0.9 + 0.5 * u01(hash(hh ^ 44))));
            let r = ph * 0.11 * c.metre;
            let dd = ((q[0] - ox).powi(2) + (q[2] - oz).powi(2)).sqrt();
            let wd = (0.012 + 0.010 * ph) * c.metre;
            let g1 = (-((dd - r) / wd).powi(2)).exp();
            let g2 = (-((dd - r * 0.6) / wd).powi(2)).exp() * 0.5;
            ring += (g1 + g2) * (1.0 - ph) * (1.0 - ph);
        }
    }
    if ring < 0.003 {
        return [0.0; 3];
    }
    let lt = particle_light(c, q, d, 0.5, 0.0);
    mul(lt, ring * 0.45)
}

/// Stage 3: fog, light through it, weather, ripples.
pub fn air(plan: &Plan, pic: &Picture, col: &mut [[f32; 4]]) {
    let c = Ctx::new(plan, pic);
    par_rows(col, pic.width, |y, row| {
        for (x, px) in row.iter_mut().enumerate() {
            let idx = y * c.w + x;
            let d = c.dir(x as f32 + 0.5, y as f32 + 0.5);
            let zs = c.depth[idx];
            let t_surf = if zs.is_finite() { zs / dot(d, c.fwd) } else { BIG };
            let range = c.p[P_FOG + 1][3] * c.metre;
            let mut add_c = [0.0f32; 3];
            let mut trans = 1.0f32;
            let any_beam = (0..c.n_light).any(|i| c.lp(i)[3][3] > 0.0);
            if c.fog_on() || any_beam {
                let t_end = t_surf.min(range);
                let fog_col = v3(c.p[P_FOG + 1]);
                let amb = v3(c.p[P_FOG + 2]);
                let g = c.p[P_FOG][3];
                let jit = ign(x, y);
                let mut tau = 0.0f32;
                for i in 0..AIR_STEPS {
                    let s0 = i as f32 / AIR_STEPS as f32;
                    let s1 = (i + 1) as f32 / AIR_STEPS as f32;
                    let t0 = t_end * s0 * s0;
                    let t1 = t_end * s1 * s1;
                    let t = mix(t0, t1, jit);
                    let dtm = (t1 - t0) / c.metre;
                    let q = add(c.o, mul(d, t));
                    let rho = c.fog_rho(q);
                    let tmid = (-(tau + 0.5 * rho * dtm)).exp();
                    if rho > 0.0 {
                        add_c = add(add_c, mul(mulv(fog_col, amb), rho * dtm * tmid));
                    }
                    for j in 0..c.n_light {
                        let l = c.lp(j);
                        let beam = l[3][3];
                        let fog_w = if rho > 0.0 { 1.0 } else { 0.0 };
                        if beam <= 0.0 && fog_w == 0.0 {
                            continue;
                        }
                        let (to, lc, r, cone) = c.light_at(j, q);
                        if lc[0] + lc[1] + lc[2] <= 0.0 {
                            continue;
                        }
                        // A bare lamp's beam is a halo round it (about two metres), a spot's a cone, a sun's a shaft.
                        let halo = if l[0][3] < 0.5 { 1.0 / (1.0 + (r / (2.0 * c.metre)).powi(2)) } else { 1.0 };
                        let local = rho * fog_w + beam * BEAM_RHO * cone * halo;
                        if local <= 0.0 {
                            continue;
                        }
                        let sh = c.shadow(l[5][1], q);
                        let ph = hg(dot(to, d), g);
                        add_c = add(add_c, mul(lc, SCATTER_GAIN * ph * sh * local * dtm * tmid));
                    }
                    tau += rho * dtm;
                }
                trans = (-tau).exp();
            }
            let mut ocol = [0.0f32; 3];
            let mut oa = 0.0f32;
            for wi in 0..c.n_weather {
                let (a, o, oal) = weather_ray(&c, wi, d, t_surf, x, y);
                add_c = add(add_c, a);
                ocol = add(ocol, mul(o, 1.0 - oa));
                oa = oa + oal * (1.0 - oa);
            }
            if c.flags & 8 != 0 && zs.is_finite() {
                let q = add(c.o, mul(d, t_surf));
                let ground = c.p[P_FOG + 3][3];
                let on_ground = (q[1] - ground).abs() < (0.05 * c.metre).max(0.0015 * t_surf) && (c.flags & 32 == 0 || c.surf[idx][1] < -0.6);
                if on_ground {
                    let mut amt = 0.0;
                    for wi in 0..c.n_weather {
                        amt = f32::max(amt, c.wp(wi)[5][3]);
                    }
                    if amt > 0.0 {
                        add_c = add(add_c, mul(ripples(&c, q, d, amt), trans));
                    }
                }
            }
            let add_c = soft_cap(add_c);
            // Take the picture through the air.
            let a0 = px[3];
            px[0] = px[0] * trans * (1.0 - oa) + add_c[0] + ocol[0];
            px[1] = px[1] * trans * (1.0 - oa) + add_c[1] + ocol[1];
            px[2] = px[2] * trans * (1.0 - oa) + add_c[2] + ocol[2];
            let a1 = 1.0 - (1.0 - a0) * trans;
            px[3] = 1.0 - (1.0 - a1) * (1.0 - oa);
        }
    });
}

// ---------------------------------------------------------------- bloom

/// One level of the bloom pyramid.
#[derive(Clone)]
pub struct Level {
    pub w: usize,
    pub h: usize,
    pub px: Vec<[f32; 3]>,
}

fn prefilter(c: [f32; 3], thr: f32, knee: f32) -> [f32; 3] {
    let l = c[0].max(c[1]).max(c[2]);
    let soft = (l - thr + knee).clamp(0.0, 2.0 * knee);
    let soft = soft * soft / (4.0 * knee);
    let k = soft.max(l - thr) / l.max(1e-4);
    mul(c, k.max(0.0))
}

/// Builds the blurred pyramid of what is bright.
pub fn bloom(plan: &Plan, pic: &Picture, col: &[[f32; 4]]) -> Vec<Level> {
    let p = &plan.params;
    let (thr, knee) = (p[P_FOG + 4][1], p[P_FOG + 4][2]);
    let (w, h) = (pic.width, pic.height);
    let mut levels: Vec<Level> = Vec::new();
    let (mut lw, mut lh) = (w.div_ceil(2).max(1), h.div_ceil(2).max(1));
    let mut l0 = vec![[0.0f32; 3]; lw * lh];
    for y in 0..lh {
        for x in 0..lw {
            let mut acc = [0.0f32; 3];
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let sx = (x * 2 + dx).min(w - 1);
                let sy = (y * 2 + dy).min(h - 1);
                let c = col[sy * w + sx];
                acc = add(acc, prefilter([c[0].min(BLOOM_CLAMP), c[1].min(BLOOM_CLAMP), c[2].min(BLOOM_CLAMP)], thr, knee));
            }
            l0[y * lw + x] = mul(acc, 0.25);
        }
    }
    levels.push(Level { w: lw, h: lh, px: l0 });
    for _ in 1..BLOOM_LEVELS {
        let prev = levels.last().unwrap();
        let (nw, nh) = (prev.w.div_ceil(2).max(1), prev.h.div_ceil(2).max(1));
        let mut px = vec![[0.0f32; 3]; nw * nh];
        for y in 0..nh {
            for x in 0..nw {
                let mut acc = [0.0f32; 3];
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let sx = (x * 2 + dx).min(prev.w - 1);
                    let sy = (y * 2 + dy).min(prev.h - 1);
                    acc = add(acc, prev.px[sy * prev.w + sx]);
                }
                px[y * nw + x] = mul(acc, 0.25);
            }
        }
        lw = nw;
        lh = nh;
        levels.push(Level { w: lw, h: lh, px });
    }
    // Gaussian blur of each level.
    const WT: [f32; 5] = [0.227_027_03, 0.194_594_6, 0.121_621_62, 0.054_054_05, 0.016_216_22];
    for lv in levels.iter_mut() {
        let (w, h) = (lv.w, lv.h);
        let mut tmp = vec![[0.0f32; 3]; w * h];
        for y in 0..h {
            for x in 0..w {
                let mut acc = mul(lv.px[y * w + x], WT[0]);
                for k in 1..5 {
                    let a = lv.px[y * w + (x + k).min(w - 1)];
                    let b = lv.px[y * w + x.saturating_sub(k)];
                    acc = add(acc, mul(add(a, b), WT[k]));
                }
                tmp[y * w + x] = acc;
            }
        }
        for y in 0..h {
            for x in 0..w {
                let mut acc = mul(tmp[y * w + x], WT[0]);
                for k in 1..5 {
                    let a = tmp[(y + k).min(h - 1) * w + x];
                    let b = tmp[y.saturating_sub(k) * w + x];
                    acc = add(acc, mul(add(a, b), WT[k]));
                }
                lv.px[y * w + x] = acc;
            }
        }
    }
    levels
}

fn sample_level(lv: &Level, u: f32, v: f32) -> [f32; 3] {
    let x = u * lv.w as f32 - 0.5;
    let y = v * lv.h as f32 - 0.5;
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let at = |xx: f32, yy: f32| lv.px[(yy.clamp(0.0, lv.h as f32 - 1.0) as usize) * lv.w + xx.clamp(0.0, lv.w as f32 - 1.0) as usize];
    let a = at(x0, y0);
    let b = at(x0 + 1.0, y0);
    let c = at(x0, y0 + 1.0);
    let d = at(x0 + 1.0, y0 + 1.0);
    std::array::from_fn(|i| mix(mix(a[i], b[i], fx), mix(c[i], d[i], fx), fy))
}

/// The weights of the pyramid's levels for a radius 0..1 (they sum to 1).
pub fn bloom_weights(radius: f32) -> [f32; BLOOM_LEVELS] {
    let mut wt = [0.0f32; BLOOM_LEVELS];
    let mut sum = 0.0;
    for (k, w) in wt.iter_mut().enumerate() {
        *w = (-(k as f32) * (1.6 - 1.3 * radius)).exp();
        sum += *w;
    }
    wt.map(|w| w / sum)
}

// ---------------------------------------------------------------- finish

fn aces(x: f32) -> f32 {
    let x = x.max(0.0) * 0.9;
    ((x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14)).clamp(0.0, 1.0)
}

/// The filmic curve on a colour: the curve on each channel (rich, but
/// bright colours go pale) mixed with the curve on its brightness (the
/// colour kept), brought back inside white without changing hue.
pub fn filmic(c: V) -> V {
    let l = lum(c);
    let per = c.map(aces);
    let k = aces(l) / l.max(1e-4);
    let kept = mul(c, k);
    let out = [mix(per[0], kept[0], 0.5), mix(per[1], kept[1], 0.5), mix(per[2], kept[2], 0.5)];
    let m = out[0].max(out[1]).max(out[2]);
    if m > 1.0 { mul(out, 1.0 / m) } else { out }
}

fn soft_shoulder(x: f32) -> f32 {
    let x = x.max(0.0);
    if x < 0.6 { x } else { 0.6 + 0.4 * (1.0 - (-(x - 0.6) / 0.4).exp()) }
}

/// Exposure, tone map and grade of one colour at pixel `(x, y)`, with
/// bloom already summed in.
pub fn grade_px(p: &[[f32; 4]], c: [f32; 3], alpha: f32, x: usize, y: usize, w: usize, h: usize, time: f32) -> [f32; 3] {
    let g = &p[P_FOG + 5];
    let g2 = &p[P_FOG + 6];
    let g3 = &p[P_FOG + 7];
    let mut c = c;
    // Exposure and white balance.
    c = mul(c, g[0].exp2());
    let (t, tint) = (g2[0], g2[1]);
    c = mulv(c, [(0.20 * t).exp(), (0.02 * t - 0.12 * tint).exp(), (-0.20 * t).exp()]);
    if (g[1] - 1.0).abs() > 1e-4 {
        c = c.map(|v| 0.18 * (v.max(0.0) / 0.18).powf(g[1]));
    }
    c = match g[3] as i32 {
        1 => filmic(c),
        2 => c.map(soft_shoulder),
        _ => c,
    };
    let mut l = lum(c);
    // Saturation, deeper in the shadows when asked for richness.
    let rich = g2[2];
    let sat = g[2] * (1.0 + rich * 0.5 * (1.0 - smooth(0.0, 0.2, l)));
    if (sat - 1.0).abs() > 1e-4 {
        c = c.map(|v| l + (v - l) * sat);
    }
    if rich > 0.0 {
        let bp = 0.012 * rich;
        c = c.map(|v| ((v - bp).max(0.0) / (1.0 - bp)).min(1.0e6));
        c = c.map(|v| {
            let s = v.clamp(0.0, 1.0);
            let toe = s * s * (3.0 - 2.0 * s);
            if v > 1.0 { v } else { mix(v, toe, 0.35 * rich) }
        });
    }
    l = lum(c);
    let st = &p[P_FOG + 8];
    let ht = &p[P_FOG + 9];
    if st[3] > 0.0 {
        let k = st[3] * 0.06 * (1.0 - smooth(0.0, 0.3, l));
        c = add(c, mul([st[0], st[1], st[2]], k));
    }
    if ht[3] > 0.0 {
        let k = ht[3] * 0.06 * smooth(0.5, 1.0, l);
        c = add(c, mul([ht[0], ht[1], ht[2]], k));
    }
    let vig = g2[3];
    if vig > 0.0 {
        let u = (x as f32 + 0.5) / w as f32 * 2.0 - 1.0;
        let v = (y as f32 + 0.5) / h as f32 * 2.0 - 1.0;
        let r2 = (u * u + v * v) * 0.5;
        c = mul(c, 1.0 - vig * smooth(0.25, 1.0, r2));
    }
    if g3[0] > 0.0 {
        let frame = (time * 24.0).floor() as i32;
        let n = u01(hash3(x as i32, y as i32, frame, 31)) + u01(hash3(x as i32, y as i32, frame, 47)) - 1.0;
        let l2 = lum(c).clamp(0.0, 1.0);
        c = c.map(|v| v + n * g3[0] * 0.06 * (0.4 + 2.4 * l2 * (1.0 - l2)) * alpha.min(1.0));
    }
    c.map(|v| v.max(0.0))
}

/// Stage 5: bloom in, then the grade.
pub fn finish(plan: &Plan, pic: &Picture, col: &mut [[f32; 4]], levels: &[Level]) {
    let p = &plan.params;
    let (w, h) = (pic.width, pic.height);
    let bl = p[P_FOG + 4];
    let wt = bloom_weights(bl[3]);
    let halation = p[P_FOG + 7][1];
    let fringe = p[P_FOG + 7][2];
    let time = p[0][2];
    let src: Vec<[f32; 4]> = if fringe > 0.0 { col.to_vec() } else { Vec::new() };
    par_rows(col, w, |y, row| {
        for (x, px) in row.iter_mut().enumerate() {
            let u = (x as f32 + 0.5) / w as f32;
            let v = (y as f32 + 0.5) / h as f32;
            let mut c = [px[0], px[1], px[2]];
            if fringe > 0.0 {
                let ux = u * 2.0 - 1.0;
                let uy = v * 2.0 - 1.0;
                let r2 = (ux * ux + uy * uy) * 0.5;
                let sh = fringe * 0.004 * r2 * w as f32;
                let dirx = ux / (ux * ux + uy * uy).sqrt().max(1e-4);
                let diry = uy / (ux * ux + uy * uy).sqrt().max(1e-4);
                let at = |k: f32| {
                    let sx = ((x as f32 + 0.5 + dirx * sh * k).floor() as i64).clamp(0, w as i64 - 1) as usize;
                    let sy = ((y as f32 + 0.5 + diry * sh * k).floor() as i64).clamp(0, h as i64 - 1) as usize;
                    src[sy * w + sx]
                };
                c[0] = at(1.0)[0];
                c[2] = at(-1.0)[2];
            }
            if bl[0] > 0.0 && !levels.is_empty() {
                let mut b = [0.0f32; 3];
                for (k, lv) in levels.iter().enumerate() {
                    b = add(b, mul(sample_level(lv, u, v), wt[k]));
                }
                c = add(c, mul(b, bl[0]));
                if halation > 0.0 {
                    c = add(c, mulv(mul(b, bl[0] * halation * 1.5), [1.0, 0.32, 0.1]));
                }
            }
            let out = grade_px(p, c, px[3], x, y, w, h, time);
            px[0] = out[0];
            px[1] = out[1];
            px[2] = out[2];
        }
    });
}

/// Runs a rows-at-a-time job over threads.
fn par_rows(buf: &mut [[f32; 4]], width: usize, f: impl Fn(usize, &mut [[f32; 4]]) + Sync) {
    let rows = buf.len() / width.max(1);
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(1, 16);
    let per = rows.div_ceil(threads * 4).max(1);
    let chunks: Vec<std::sync::Mutex<(usize, &mut [[f32; 4]])>> = buf.chunks_mut(per * width).enumerate().map(|(i, c)| std::sync::Mutex::new((i * per, c))).collect();
    let next = std::sync::atomic::AtomicUsize::new(0);
    std::thread::scope(|s| {
        for _ in 0..threads.min(chunks.len()) {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    if i >= chunks.len() {
                        break;
                    }
                    let mut g = chunks[i].lock().unwrap_or_else(|e| e.into_inner());
                    let y0 = g.0;
                    for (r, row) in g.1.chunks_mut(width).enumerate() {
                        f(y0 + r, row);
                    }
                }
            });
        }
    });
}

/// The whole pass on the CPU.
pub fn run(plan: &Plan, pic: &Picture, col: &mut [[f32; 4]]) {
    if plan.stages.sprites {
        sprites(plan, pic, col);
    }
    if plan.stages.reflect {
        reflect(plan, pic, col);
    }
    if plan.stages.air {
        air(plan, pic, col);
    }
    let metered;
    let plan = if plan.metered() {
        metered = plan.exposed(meter(col, plan.params[P_FOG + 5][3]));
        &metered
    } else {
        plan
    };
    let levels = if plan.stages.bloom { bloom(plan, pic, col) } else { Vec::new() };
    if plan.stages.finish {
        finish(plan, pic, col, &levels);
    }
}
