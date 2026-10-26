// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later
//
// The air and look pass on the GPU: the same algorithm as cpu.rs, over the
// same packed parameters (see mod.rs for the layout).

@group(0) @binding(0) var<storage, read> P: array<vec4<f32>>;
@group(0) @binding(1) var<storage, read> SH: array<f32>;
@group(0) @binding(2) var<storage, read> DEPTH: array<f32>;
@group(0) @binding(3) var<storage, read> SURF: array<vec4<f32>>;
@group(0) @binding(4) var<storage, read_write> A: array<vec4<f32>>;
@group(0) @binding(5) var<storage, read_write> B: array<vec4<f32>>;
@group(0) @binding(6) var<storage, read_write> BL: array<vec4<f32>>;
@group(0) @binding(7) var<storage, read_write> BT: array<vec4<f32>>;
// x: bloom level, y: blur direction.
@group(0) @binding(8) var<uniform> U: vec4<u32>;

const P_FOG: u32 = 6u;
const P_PRACTICAL: u32 = 20u;
const PRACTICAL_STRIDE: u32 = 7u;
const P_WEATHER: u32 = 104u;
const WEATHER_STRIDE: u32 = 7u;
const AIR_STEPS: u32 = 32u;
const SSR_STEPS: u32 = 40u;
const BEAM_RHO: f32 = 0.16;
const SCATTER_GAIN: f32 = 0.18;
const BLOOM_LEVELS: u32 = 5u;
const AIR_KNEE: f32 = 1.0;
const AIR_CAP: f32 = 2.5;
const BLOOM_CLAMP: f32 = 6.0;
const BIG: f32 = 1.0e30;
const PI: f32 = 3.14159265;
const NEAR: f32 = 1.0;

fn W() -> u32 { return u32(P[0].x); }
fn H() -> u32 { return u32(P[0].y); }
fn TIME() -> f32 { return P[0].z; }
fn METRE() -> f32 { return P[0].w; }
fn O() -> vec3<f32> { return P[1].xyz; }
fn ZOOM() -> f32 { return P[1].w; }
fn RIGHT() -> vec3<f32> { return P[2].xyz; }
fn DOWN() -> vec3<f32> { return P[3].xyz; }
fn FWD() -> vec3<f32> { return P[4].xyz; }
fn CENTRE() -> vec2<f32> { return vec2<f32>(P[2].w, P[3].w); }
fn N_LIGHT() -> u32 { return u32(P[5].x); }
fn N_WEATHER() -> u32 { return u32(P[5].y); }
fn FLAGS() -> u32 { return u32(P[5].w); }

fn clamp01(x: f32) -> f32 { return clamp(x, 0.0, 1.0); }
fn sstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = clamp01((x - e0) / (e1 - e0));
    return t * t * (3.0 - 2.0 * t);
}
fn lum(c: vec3<f32>) -> f32 { return 0.2126 * c.x + 0.7152 * c.y + 0.0722 * c.z; }
fn mixf(a: f32, b: f32, t: f32) -> f32 { return a + (b - a) * t; }
fn maxc(c: vec3<f32>) -> f32 { return max(c.x, max(c.y, c.z)); }

// ---------------------------------------------------------------- noise

fn hash(xin: u32) -> u32 {
    var x = xin;
    x = x ^ (x >> 16u);
    x = x * 0x7feb352du;
    x = x ^ (x >> 15u);
    x = x * 0x846ca68bu;
    x = x ^ (x >> 16u);
    return x;
}
fn hash3(a: i32, b: i32, c: i32, s: u32) -> u32 {
    var h = hash(s ^ 0x9e3779b9u);
    h = hash(h ^ bitcast<u32>(a));
    h = hash(h ^ (bitcast<u32>(b) * 0x85ebca6bu));
    return hash(h ^ (bitcast<u32>(c) * 0xc2b2ae35u));
}
fn u01(h: u32) -> f32 { return f32(h >> 8u) * (1.0 / 16777216.0); }
fn ign(x: u32, y: u32) -> f32 { return fract(52.982918 * fract(0.06711056 * f32(x) + 0.00583715 * f32(y))); }

fn noise3(p: vec3<f32>) -> f32 {
    let i = floor(p);
    let f = p - i;
    let u = f * f * (3.0 - 2.0 * f);
    let a = vec3<i32>(i);
    let x00 = mixf(u01(hash3(a.x, a.y, a.z, 77u)), u01(hash3(a.x + 1, a.y, a.z, 77u)), u.x);
    let x10 = mixf(u01(hash3(a.x, a.y + 1, a.z, 77u)), u01(hash3(a.x + 1, a.y + 1, a.z, 77u)), u.x);
    let x01 = mixf(u01(hash3(a.x, a.y, a.z + 1, 77u)), u01(hash3(a.x + 1, a.y, a.z + 1, 77u)), u.x);
    let x11 = mixf(u01(hash3(a.x, a.y + 1, a.z + 1, 77u)), u01(hash3(a.x + 1, a.y + 1, a.z + 1, 77u)), u.x);
    return mixf(mixf(x00, x10, u.y), mixf(x01, x11, u.y), u.z);
}
fn fbm3(p: vec3<f32>) -> f32 {
    return noise3(p) * 0.5333 + noise3(p * 2.03) * 0.2667 + noise3(p * 4.1) * 0.1333 + noise3(p * 8.3) * 0.0667;
}

// ---------------------------------------------------------------- context

fn cam_dir(x: f32, y: f32) -> vec3<f32> {
    let u = (x - CENTRE().x) / ZOOM();
    let v = (y - CENTRE().y) / ZOOM();
    return normalize(RIGHT() * u + DOWN() * v + FWD());
}

// Screen x, y and view depth; z < 0 when behind the lens.
fn project(q: vec3<f32>) -> vec3<f32> {
    let v = q - O();
    let z = dot(v, FWD());
    if (z <= NEAR) { return vec3<f32>(0.0, 0.0, -1.0); }
    return vec3<f32>(CENTRE().x + ZOOM() * dot(v, RIGHT()) / z, CENTRE().y + ZOOM() * dot(v, DOWN()) / z, z);
}

fn lp(i: u32, k: u32) -> vec4<f32> { return P[P_PRACTICAL + i * PRACTICAL_STRIDE + k]; }
fn wp(i: u32, k: u32) -> vec4<f32> { return P[P_WEATHER + i * WEATHER_STRIDE + k]; }

fn fog_rho(q: vec3<f32>) -> f32 {
    let f = P[P_FOG];
    if (f.x <= 0.0) { return 0.0; }
    let hm = (f.z - q.y) / METRE();
    var rho = f.x * exp(-f.y * clamp(hm, -30.0, 60.0));
    let patchy = P[P_FOG + 2u].w;
    if (patchy > 0.0) {
        let n = fbm3(q * (1.0 / (7.0 * METRE())));
        rho = rho * mixf(1.0, sstep(0.25, 0.75, n) * 1.8, patchy);
    }
    return rho;
}

fn fog_tau(d: vec3<f32>, t: f32) -> f32 {
    let f = P[P_FOG];
    if (f.x <= 0.0) { return 0.0; }
    let tm = t / METRE();
    let h0 = (f.z - O().y) / METRE();
    let base = f.x * exp(-f.y * clamp(h0, -30.0, 60.0));
    let a = f.y * d.y;
    if (abs(a) < 1e-4) { return base * tm; }
    return base * (exp(clamp(a * tm, -30.0, 30.0)) - 1.0) / a;
}

struct LightAt {
    to: vec3<f32>,
    col: vec3<f32>,
    r: f32,
    cone: f32,
}

fn light_at(i: u32, q: vec3<f32>) -> LightAt {
    let a = lp(i, 0u);
    let b = lp(i, 1u);
    let c = lp(i, 2u);
    let d = lp(i, 3u);
    let col = b.xyz;
    if (a.w > 1.5) {
        return LightAt(-normalize(c.xyz), col, BIG, 1.0);
    }
    let dv = a.xyz - q;
    let r = max(length(dv), 1e-3);
    let to = dv * (1.0 / r);
    let reach = max(lp(i, 5u).z, 1.0);
    let fo = lp(i, 6u).y;
    var atten = 1.0 / (1.0 + (r / reach) * (r / reach));
    if (fo > 0.0) { atten = 1.0 - sstep(lp(i, 6u).x, lp(i, 6u).x + fo, r); }
    var cone = 1.0;
    if (a.w > 0.5) { cone = sstep(c.w, d.x, dot(-to, normalize(c.xyz))); }
    return LightAt(to, col * (atten * cone), r, cone);
}

fn shadow(slot: f32, q: vec3<f32>) -> f32 {
    if (slot < 0.0) { return 1.0; }
    let off = u32(slot);
    let pos = vec3<f32>(SH[off], SH[off + 1u], SH[off + 2u]);
    let zoom = SH[off + 3u];
    let ax0 = vec3<f32>(SH[off + 4u], SH[off + 5u], SH[off + 6u]);
    let ax1 = vec3<f32>(SH[off + 8u], SH[off + 9u], SH[off + 10u]);
    let ax2 = vec3<f32>(SH[off + 12u], SH[off + 13u], SH[off + 14u]);
    let c0 = SH[off + 7u];
    let c1 = SH[off + 11u];
    let ortho = SH[off + 15u] > 0.5;
    let size = i32(SH[off + 16u]);
    let texel = SH[off + 17u];
    let base = u32(SH[off + 18u]);
    let dq = q - pos;
    let v = vec3<f32>(dot(ax0, dq), dot(ax1, dq), dot(ax2, dq));
    if (!ortho && v.z <= 1.0) { return 1.0; }
    var sx = c0 + zoom * v.x;
    var sy = c1 + zoom * v.y;
    var tx = texel;
    if (!ortho) {
        sx = c0 + zoom * v.x / v.z;
        sy = c1 + zoom * v.y / v.z;
        tx = texel * v.z;
    }
    let bias = tx * 2.5 + 0.5;
    var lit = 0.0;
    for (var k = 0; k < 5; k = k + 1) {
        var dx = 0;
        var dy = 0;
        if (k == 1) { dx = 1; }
        if (k == 2) { dx = -1; }
        if (k == 3) { dy = 1; }
        if (k == 4) { dy = -1; }
        let x = i32(round(sx - 0.5)) + dx;
        let y = i32(round(sy - 0.5)) + dy;
        if (x < 0 || y < 0 || x >= size || y >= size) {
            lit = lit + 1.0;
            continue;
        }
        let d = SH[base + u32(y * size + x)];
        if (v.z - bias <= d) { lit = lit + 1.0; }
    }
    return lit / 5.0;
}

fn hg(c: f32, g: f32) -> f32 {
    let g2 = g * g;
    return (1.0 - g2) / pow(max(1.0 + g2 - 2.0 * g * c, 1e-3), 1.5);
}

fn at_px(x: i32, y: i32) -> u32 {
    return u32(clamp(y, 0, i32(H()) - 1)) * W() + u32(clamp(x, 0, i32(W()) - 1));
}

// ---------------------------------------------------------------- sprites

struct Flame {
    col: vec3<f32>,
    occ: f32,
}

fn flame_at(u: f32, v: f32, time: f32, seed: f32, tongues: f32) -> Flame {
    let n = clamp(round(tongues), 1.0, 5.0);
    var occ_best = 0.0;
    var heat_best = 0.0;
    var blue_best = 0.0;
    var j = 0.0;
    loop {
        if (j >= n) { break; }
        var o = 0.0;
        var hj = 1.0;
        var wj = 1.0;
        if (n > 1.5) {
            o = ((j + 0.5) / n - 0.5) * 1.5;
            hj = 1.0 - 0.38 * (abs(o) / 0.75) - 0.18 * u01(hash(u32(j) + u32(seed * 97.0)));
            wj = 1.25 / sqrt(n);
        }
        let vv = v / hj;
        if (vv > -0.02 && vv < 1.1) {
            let phase = seed * 6.283 + j * 2.1;
            let sway = sin(time * (4.3 + 1.7 * j) + vv * 3.1 + phase) * 0.16 * pow(max(vv, 0.0), 1.4) * wj + (noise3(vec3<f32>(vv * 2.5 + seed, time * 2.4 + j * 7.0, j)) - 0.5) * 0.34 * max(vv, 0.0);
            let uu = (u - o - sway) / wj;
            let prof = sin(PI * pow(clamp(vv, 0.0, 1.0), 0.6));
            let hw = max(0.5 * prof * (1.0 - 0.25 * vv), 1e-3);
            let q = abs(uu) / hw;
            let occ = (1.0 - sstep(0.62, 1.0, q)) * sstep(1.08, 0.88, vv) * sstep(-0.02, 0.03, vv);
            let heat = occ * (1.0 - pow(min(q, 1.0), 1.4)) * (1.0 - 0.5 * vv);
            let blue = sstep(0.2, 0.0, vv) * (1.0 - min(q, 1.0)) * occ;
            if (occ > occ_best) { occ_best = occ; }
            heat_best = max(heat_best, heat);
            blue_best = max(blue_best, blue);
        }
        j = j + 1.0;
    }
    let heat = clamp01(heat_best * 1.25);
    let c0 = vec3<f32>(1.0, 0.1, 0.02);
    let c1 = vec3<f32>(1.0, 0.42, 0.07);
    let c2 = vec3<f32>(1.0, 0.78, 0.28);
    let c3 = vec3<f32>(1.0, 0.95, 0.72);
    var ramp = c3;
    if (heat < 0.33) {
        ramp = mix(c0, c1, heat / 0.33);
    } else if (heat < 0.66) {
        ramp = mix(c1, c2, (heat - 0.33) / 0.33);
    } else {
        ramp = mix(c2, c3, (heat - 0.66) / 0.34);
    }
    let k = 0.7 + 4.5 * heat * heat;
    return Flame(ramp * k + vec3<f32>(0.15, 0.3, 1.0) * (blue_best * 0.9), occ_best);
}

@compute @workgroup_size(8, 8, 1)
fn sprites(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= W() || gid.y >= H()) { return; }
    let idx = gid.y * W() + gid.x;
    let px = f32(gid.x) + 0.5;
    let py = f32(gid.y) + 0.5;
    let zbuf = DEPTH[idx];
    var add_c = vec3<f32>(0.0);
    var cover = 0.0;
    var cover_col = vec3<f32>(0.0);
    let hh = f32(H());
    for (var i = 0u; i < N_LIGHT(); i = i + 1u) {
        let l0 = lp(i, 0u);
        let l1 = lp(i, 1u);
        let l2 = lp(i, 2u);
        let l3 = lp(i, 3u);
        let l4 = lp(i, 4u);
        let l5 = lp(i, 5u);
        let glow = l3.y;
        let flare = l3.z;
        let fh = l4.x;
        if (glow <= 0.0 && flare <= 0.0 && fh <= 0.0) { continue; }
        let pr = project(l0.xyz);
        if (pr.z < 0.0) { continue; }
        let sx = pr.x;
        let sy = pr.y;
        let z = pr.z;
        let vis = l5.x;
        let dx = px - sx;
        let dy = py - sy;
        let mc = maxc(l1.xyz);
        let hue = l1.xyz * (1.0 / max(mc, 0.05));
        let a = max(l1.w * ZOOM() / z, 1.2);
        let dist_m = z / METRE();
        let farf = 1.0 / (1.0 + (dist_m / 30.0) * (dist_m / 30.0));
        var facing = 1.0;
        if (l0.w > 0.5 && l0.w < 1.5) {
            let to_cam = normalize(O() - l0.xyz);
            facing = 0.35 + 0.65 * sstep(-0.2, 0.8, dot(normalize(l2.xyz), to_cam));
        }
        let gi = 2.2 * sqrt(mc + 0.1);
        if (glow > 0.0) {
            let rr = sqrt(dx * dx + dy * dy);
            let ah = max(a * 3.5, 0.012 * hh * (0.5 + 0.5 * min(glow, 2.0)));
            let core = exp(-(rr / a) * (rr / a));
            let q = rr / ah;
            let halo = 1.0 / pow(1.0 + q * q, 1.5);
            let e = vis * facing * farf * glow * (core * 6.0 + halo * 0.6) * gi;
            add_c = add_c + hue * e;
        }
        if (flare > 0.0) {
            let sd = exp(-abs(dy) / (a * 0.5 + 0.5)) / (1.0 + (abs(dx) / (a * 14.0 + 20.0)) * (abs(dx) / (a * 14.0 + 20.0)));
            let e = vis * facing * farf * flare * sd * 0.5 * gi;
            add_c = add_c + hue * e;
        }
        if (fh > 0.0) {
            let ws = l4.y * ZOOM() / z;
            let hs = fh * ZOOM() / z;
            let by = sy + 0.5 * hs;
            let u = dx / max(ws, 0.5);
            let v = (by - py) / max(hs, 0.5);
            if (abs(u) < 1.6 && v > -0.05 && v < 1.15) {
                let f = flame_at(u, v, TIME(), l4.z, l4.w);
                let bias = max(0.5 * fh, 2.0);
                let o = f.occ * clamp01((zbuf - (z - 2.0 * bias)) / bias);
                if (o > cover) {
                    cover = o;
                    cover_col = f.col;
                }
            }
        }
    }
    var c = A[idx];
    let k = 1.0 - 0.55 * cover;
    c = vec4<f32>(c.xyz * k + add_c + cover_col * cover, max(c.w, cover * 0.85));
    A[idx] = c;
}

// ---------------------------------------------------------------- reflect

fn mirror_miss(q: vec3<f32>, r: vec3<f32>, n: vec3<f32>, spread: f32, stretch: f32) -> vec3<f32> {
    let up_k = clamp01(-r.y * 3.0 + 0.3);
    var out = P[P_FOG + 3u].xyz * (0.35 + 0.65 * up_k);
    let up = vec3<f32>(0.0, -1.0, 0.0);
    for (var i = 0u; i < N_LIGHT(); i = i + 1u) {
        let l0 = lp(i, 0u);
        let l1 = lp(i, 1u);
        let l3 = lp(i, 3u);
        let l4 = lp(i, 4u);
        let l5 = lp(i, 5u);
        if (l3.y <= 0.0 && l4.x <= 0.0) { continue; }
        let to = l0.xyz - q;
        let dist = max(length(to), 1.0);
        let lv = to * (1.0 / dist);
        if (dot(lv, n) <= 0.02) { continue; }
        let delta = r - lv;
        let dv = dot(delta, up);
        let dh = length(delta - up * dv);
        let a = max(max(l1.w, 0.08 * METRE()) / dist, 0.004);
        let s = a + spread;
        let ks = s * stretch;
        let kern = exp(-(dh * dh) / (s * s) - (dv * dv) / (ks * ks)) + 0.18 / pow(1.0 + (dh * dh + dv * dv / (stretch * stretch)) / (s * s * 16.0), 1.5);
        let mc = maxc(l1.xyz);
        let fh = l4.x > 0.0;
        var hue = l1.xyz * (1.0 / max(mc, 0.05));
        var gain = 5.3 * sqrt(mc + 0.1) * max(l3.y, 0.5);
        if (fh) {
            hue = vec3<f32>(1.0, 0.55, 0.15);
            gain = 5.0;
        }
        let farf = 1.0 / (1.0 + (dist / METRE() / 30.0) * (dist / METRE() / 30.0));
        out = out + hue * (kern * gain * farf * l5.x * 0.55 * clamp(a / s, 0.25, 1.0));
    }
    return out;
}

@compute @workgroup_size(8, 8, 1)
fn reflect_pass(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= W() || gid.y >= H()) { return; }
    let idx = gid.y * W() + gid.x;
    let s = SURF[idx];
    let refl = s.w;
    let zs = DEPTH[idx];
    if (refl < 0.01 || zs >= 1.0e29) { return; }
    let wf = f32(W());
    let hf = f32(H());
    let d = cam_dir(f32(gid.x) + 0.5, f32(gid.y) + 0.5);
    let t = zs / dot(d, FWD());
    let q = O() + d * t;
    var n = normalize(s.xyz);
    if (dot(n, d) > 0.0) { n = -n; }
    let ndv = max(-dot(n, d), 0.02);
    let f = 0.04 + 0.96 * pow(1.0 - ndv, 5.0);
    let rr = refl * (0.10 + 0.90 * f);
    let r = d - n * (2.0 * dot(d, n));
    let spread = (1.0 - refl) * 0.12 + 0.004;
    let stretch = 1.0 + 7.0 * (1.0 - refl);
    let jit = 0.5;
    let t0 = 3.0;
    let tmax = 70.0 * METRE();
    var hit = false;
    var hx = 0.0;
    var hy = 0.0;
    var prev = t0;
    for (var i = 0u; i < SSR_STEPS; i = i + 1u) {
        let tau = t0 * pow(tmax / t0, (f32(i) + jit) / f32(SSR_STEPS));
        let qq = q + r * tau;
        let pj = project(qq);
        if (pj.z < 0.0) { break; }
        if (pj.x < 0.0 || pj.y < 0.0 || pj.x >= wf || pj.y >= hf) { break; }
        let dz = DEPTH[at_px(i32(pj.x), i32(pj.y))];
        if (pj.z > dz && pj.z - dz < 0.08 * pj.z + 6.0) {
            var lo = prev;
            var hi = tau;
            for (var k = 0; k < 5; k = k + 1) {
                let mid = 0.5 * (lo + hi);
                let pm = project(q + r * mid);
                var ok = false;
                if (pm.z >= 0.0 && pm.x >= 0.0 && pm.y >= 0.0 && pm.x < wf && pm.y < hf) {
                    ok = pm.z > DEPTH[at_px(i32(pm.x), i32(pm.y))];
                }
                if (ok) { hi = mid; } else { lo = mid; }
            }
            let ph = project(q + r * hi);
            if (ph.z >= 0.0) {
                hit = true;
                hx = floor(clamp(ph.x, 0.0, wf - 1.0));
                hy = floor(clamp(ph.y, 0.0, hf - 1.0));
            }
            break;
        }
        prev = tau;
    }
    var seen = vec3<f32>(0.0);
    if (hit) {
        let sp = max((1.0 - refl) * 0.012 * hf, 0.0);
        var acc = vec3<f32>(0.0);
        var wsum = 0.0;
        for (var k = 0; k < 7; k = k + 1) {
            var o = vec3<f32>(0.0, 0.0, 1.0);
            if (k == 1) { o = vec3<f32>(0.0, 1.0, 0.7); }
            if (k == 2) { o = vec3<f32>(0.0, -1.0, 0.7); }
            if (k == 3) { o = vec3<f32>(0.0, 2.0, 0.4); }
            if (k == 4) { o = vec3<f32>(0.0, -2.0, 0.4); }
            if (k == 5) { o = vec3<f32>(0.5, 0.0, 0.5); }
            if (k == 6) { o = vec3<f32>(-0.5, 0.0, 0.5); }
            let i2 = at_px(i32(hx) + i32(round(o.x * sp)), i32(hy) + i32(round(o.y * sp)));
            acc = acc + B[i2].xyz * o.z;
            wsum = wsum + o.z;
        }
        let b = acc * (1.0 / wsum);
        let ex = min(hx, wf - 1.0 - hx) / (0.06 * wf);
        let ey = min(hy, hf - 1.0 - hy) / (0.06 * hf);
        let e = clamp01(min(ex, ey));
        seen = b * e + mirror_miss(q, r, n, spread, stretch) * ((1.0 - e) + 0.5 * e);
    } else {
        seen = mirror_miss(q, r, n, spread, stretch);
    }
    let c = A[idx];
    A[idx] = vec4<f32>(c.xyz * (1.0 - rr) + seen * rr, c.w);
}

// ---------------------------------------------------------------- air

fn particle_light(q: vec3<f32>, d: vec3<f32>, amb_k: f32, g: f32) -> vec3<f32> {
    var out = P[P_FOG + 2u].xyz * amb_k;
    for (var i = 0u; i < N_LIGHT(); i = i + 1u) {
        let la = light_at(i, q);
        let ph = hg(dot(la.to, d), g);
        out = out + la.col * (ph * 0.9);
    }
    return out;
}

struct WeatherOut {
    add_c: vec3<f32>,
    over_c: vec3<f32>,
    over_a: f32,
}

fn weather_ray(wi: u32, d: vec3<f32>, t_surf: f32, x: u32, y: u32) -> WeatherOut {
    let w0 = wp(wi, 0u);
    let w1 = wp(wi, 1u);
    let w2 = wp(wi, 2u);
    let w3 = wp(wi, 3u);
    let w4 = wp(wi, 4u);
    let w5 = wp(wi, 5u);
    let w6 = wp(wi, 6u);
    let mode = w0.x;
    let prob = w0.y;
    let cell = max(w0.z, 1.0);
    let size = w0.w;
    let vel = w1.xyz;
    let streak = w1.w;
    let tint = w2.xyz;
    let bright = w2.w;
    let flutter = w3.x;
    let hz = w3.y;
    let maxd = w3.z;
    let amb_k = w3.w;
    let zon = w4.w > 0.5;
    let zlo = w4.xyz;
    let zhi = w5.xyz;
    let g = w6.x;
    let stp = max(cell * 0.9, maxd / 48.0);
    let n = clamp(u32(ceil(maxd / stp)), 1u, 48u);
    let jit = fract(ign(x, y) + 0.618034 * f32(wi));
    var add_c = vec3<f32>(0.0);
    var over_c = vec3<f32>(0.0);
    var over_a = 0.0;
    let seed = (wi + 1u) * 1013u;
    let tm = TIME();
    for (var i = 0u; i < n; i = i + 1u) {
        let t = (f32(i) + jit) * stp + 0.3 * METRE();
        if (t > t_surf || t > maxd) { break; }
        let q = O() + d * t;
        if (zon && (q.x < zlo.x || q.y < zlo.y || q.z < zlo.z || q.x > zhi.x || q.y > zhi.y || q.z > zhi.z)) { continue; }
        let cx = i32(floor(q.x / cell));
        let cy = i32(floor(q.y / cell));
        let cz = i32(floor(q.z / cell));
        for (var k = 0u; k < 3u; k = k + 1u) {
            let hh = hash3(cx, cy, cz, seed + k * 7919u);
            if (u01(hh) > prob) { continue; }
            let r1 = u01(hash(hh ^ 0x12345678u));
            let r2 = u01(hash(hh ^ 0x23456789u));
            let r3 = u01(hash(hh ^ 0x3456789au));
            let r4 = u01(hash(hh ^ 0x456789abu));
            let pos = vec3<f32>(
                (f32(cx) + fract(r1 + vel.x * tm / cell)) * cell + flutter * sin(6.2832 * (hz * tm + r4)),
                (f32(cy) + fract(r2 + vel.y * tm / cell)) * cell,
                (f32(cz) + fract(r3 + vel.z * tm / cell)) * cell + flutter * cos(6.2832 * (hz * tm * 0.83 + r4 * 1.7)));
            let b = pos - vel * streak;
            let e = pos - b;
            let wv = b - O();
            let ee = dot(d, wv);
            let u = wv - d * ee;
            let v = e - d * dot(e, d);
            let vv = dot(v, v);
            var s = 0.0;
            if (vv > 1e-9) { s = clamp01(-dot(u, v) / vv); }
            let off = u + v * s;
            let dist = length(off);
            let th = ee + s * dot(e, d);
            if (th < 0.2 * METRE() || th > t_surf || th > maxd) { continue; }
            let foot = th / ZOOM() * 0.6;
            let width = max(size, foot);
            var cov = clamp01(1.0 - dist / width);
            var energy = min(size / width, 1.0);
            if (mode > 2.5) {
                cov = exp(-(dist / width) * (dist / width));
                energy = 1.0;
            }
            cov = cov * cov;
            if (cov < 0.004) { continue; }
            let at = O() + d * th;
            let tr = exp(-fog_tau(d, th));
            let fade = (1.0 - sstep(0.7 * maxd, maxd, th)) * sstep(0.4 * METRE(), 2.0 * METRE(), th);
            var lit = vec3<f32>(0.0);
            if (mode > 0.5 && mode < 1.5) {
                let tw = 0.65 + 0.35 * sin(6.2832 * (tm * (1.5 + 2.0 * r4) + r1 * 9.0));
                lit = tint * (bright * tw);
            } else {
                lit = tint * particle_light(at, d, amb_k, g) * bright;
            }
            if (mode > 1.5) {
                var base_a = 0.95;
                if (mode > 2.5) { base_a = 0.30; }
                let a = cov * energy * base_a;
                over_c = over_c + lit * (tr * fade) * (a * (1.0 - over_a));
                over_a = over_a + a * (1.0 - over_a);
            } else {
                add_c = add_c + lit * (cov * energy * tr * fade);
            }
        }
    }
    return WeatherOut(add_c, over_c, over_a);
}

fn ripples(q: vec3<f32>, d: vec3<f32>, amount: f32) -> vec3<f32> {
    let cell = 0.5 * METRE();
    let cx = i32(floor(q.x / cell));
    let cz = i32(floor(q.z / cell));
    var ring = 0.0;
    for (var dz = -1; dz <= 1; dz = dz + 1) {
        for (var dx = -1; dx <= 1; dx = dx + 1) {
            let hh = hash3(cx + dx, 0, cz + dz, 4242u);
            if (u01(hh) > 0.55 * amount) { continue; }
            let ox = f32(cx + dx) * cell + (0.2 + 0.6 * u01(hash(hh ^ 11u))) * cell;
            let oz = f32(cz + dz) * cell + (0.2 + 0.6 * u01(hash(hh ^ 22u))) * cell;
            let ph = fract(u01(hash(hh ^ 33u)) + TIME() * (0.9 + 0.5 * u01(hash(hh ^ 44u))));
            let r = ph * 0.11 * METRE();
            let dd = sqrt((q.x - ox) * (q.x - ox) + (q.z - oz) * (q.z - oz));
            let wd = (0.012 + 0.010 * ph) * METRE();
            let g1 = exp(-((dd - r) / wd) * ((dd - r) / wd));
            let g2 = exp(-((dd - r * 0.6) / wd) * ((dd - r * 0.6) / wd)) * 0.5;
            ring = ring + (g1 + g2) * (1.0 - ph) * (1.0 - ph);
        }
    }
    if (ring < 0.003) { return vec3<f32>(0.0); }
    return particle_light(q, d, 0.5, 0.0) * (ring * 0.45);
}

fn soft_cap(c: vec3<f32>) -> vec3<f32> {
    let m = maxc(c);
    if (m <= AIR_KNEE) { return c; }
    let room = AIR_CAP - AIR_KNEE;
    return c * ((AIR_KNEE + room * (1.0 - exp(-(m - AIR_KNEE) / room))) / m);
}

@compute @workgroup_size(8, 8, 1)
fn air(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= W() || gid.y >= H()) { return; }
    let idx = gid.y * W() + gid.x;
    let d = cam_dir(f32(gid.x) + 0.5, f32(gid.y) + 0.5);
    let zs = DEPTH[idx];
    var t_surf = BIG;
    if (zs < 1.0e29) { t_surf = zs / dot(d, FWD()); }
    let range = P[P_FOG + 1u].w * METRE();
    var add_c = vec3<f32>(0.0);
    var trans = 1.0;
    var any_beam = false;
    for (var j = 0u; j < N_LIGHT(); j = j + 1u) {
        if (lp(j, 3u).w > 0.0) { any_beam = true; }
    }
    if ((FLAGS() & 1u) != 0u || any_beam) {
        let t_end = min(t_surf, range);
        let fog_col = P[P_FOG + 1u].xyz;
        let amb = P[P_FOG + 2u].xyz;
        let g = P[P_FOG].w;
        let jit = ign(gid.x, gid.y);
        var tau = 0.0;
        for (var i = 0u; i < AIR_STEPS; i = i + 1u) {
            let s0 = f32(i) / f32(AIR_STEPS);
            let s1 = f32(i + 1u) / f32(AIR_STEPS);
            let t0 = t_end * s0 * s0;
            let t1 = t_end * s1 * s1;
            let t = mixf(t0, t1, jit);
            let dtm = (t1 - t0) / METRE();
            let q = O() + d * t;
            let rho = fog_rho(q);
            let tmid = exp(-(tau + 0.5 * rho * dtm));
            if (rho > 0.0) {
                add_c = add_c + fog_col * amb * (rho * dtm * tmid);
            }
            for (var j = 0u; j < N_LIGHT(); j = j + 1u) {
                let beam = lp(j, 3u).w;
                var fog_w = 0.0;
                if (rho > 0.0) { fog_w = 1.0; }
                if (beam <= 0.0 && fog_w == 0.0) { continue; }
                let la = light_at(j, q);
                if (la.col.x + la.col.y + la.col.z <= 0.0) { continue; }
                var halo = 1.0;
                if (lp(j, 0u).w < 0.5) {
                    let rr = la.r / (2.0 * METRE());
                    halo = 1.0 / (1.0 + rr * rr);
                }
                let local = rho * fog_w + beam * BEAM_RHO * la.cone * halo;
                if (local <= 0.0) { continue; }
                let sh = shadow(lp(j, 5u).y, q);
                let ph = hg(dot(la.to, d), g);
                add_c = add_c + la.col * (SCATTER_GAIN * ph * sh * local * dtm * tmid);
            }
            tau = tau + rho * dtm;
        }
        trans = exp(-tau);
    }
    var ocol = vec3<f32>(0.0);
    var oa = 0.0;
    for (var wi = 0u; wi < N_WEATHER(); wi = wi + 1u) {
        let w = weather_ray(wi, d, t_surf, gid.x, gid.y);
        add_c = add_c + w.add_c;
        ocol = ocol + w.over_c * (1.0 - oa);
        oa = oa + w.over_a * (1.0 - oa);
    }
    if ((FLAGS() & 8u) != 0u && zs < 1.0e29) {
        let q = O() + d * t_surf;
        let ground = P[P_FOG + 3u].w;
        var flat_ok = true;
        if ((FLAGS() & 32u) != 0u) { flat_ok = SURF[idx].y < -0.6; }
        let on_ground = abs(q.y - ground) < max(0.05 * METRE(), 0.0015 * t_surf) && flat_ok;
        if (on_ground) {
            var amt = 0.0;
            for (var wi = 0u; wi < N_WEATHER(); wi = wi + 1u) { amt = max(amt, wp(wi, 5u).w); }
            if (amt > 0.0) { add_c = add_c + ripples(q, d, amt) * trans; }
        }
    }
    add_c = soft_cap(add_c);
    let c = A[idx];
    let a1 = 1.0 - (1.0 - c.w) * trans;
    A[idx] = vec4<f32>(c.xyz * (trans * (1.0 - oa)) + add_c + ocol, 1.0 - (1.0 - a1) * (1.0 - oa));
}

// ---------------------------------------------------------------- bloom

// Width, height and offset of a pyramid level.
fn level_dims(k: u32) -> vec3<u32> {
    var w = (W() + 1u) / 2u;
    var h = (H() + 1u) / 2u;
    var off = 0u;
    for (var i = 0u; i < k; i = i + 1u) {
        off = off + w * h;
        w = (w + 1u) / 2u;
        h = (h + 1u) / 2u;
    }
    return vec3<u32>(max(w, 1u), max(h, 1u), off);
}

fn prefilter(c: vec3<f32>) -> vec3<f32> {
    let thr = P[P_FOG + 4u].y;
    let knee = P[P_FOG + 4u].z;
    let l = maxc(c);
    var soft = clamp(l - thr + knee, 0.0, 2.0 * knee);
    soft = soft * soft / (4.0 * knee);
    let k = max(soft, l - thr) / max(l, 1e-4);
    return c * max(k, 0.0);
}

@compute @workgroup_size(8, 8, 1)
fn bloom_down(@builtin(global_invocation_id) gid: vec3<u32>) {
    let k = U.x;
    let dm = level_dims(k);
    if (gid.x >= dm.x || gid.y >= dm.y) { return; }
    var acc = vec3<f32>(0.0);
    if (k == 0u) {
        for (var i = 0u; i < 4u; i = i + 1u) {
            let sx = min(gid.x * 2u + (i & 1u), W() - 1u);
            let sy = min(gid.y * 2u + (i >> 1u), H() - 1u);
            let c = A[sy * W() + sx].xyz;
            acc = acc + prefilter(min(c, vec3<f32>(BLOOM_CLAMP)));
        }
    } else {
        let pd = level_dims(k - 1u);
        for (var i = 0u; i < 4u; i = i + 1u) {
            let sx = min(gid.x * 2u + (i & 1u), pd.x - 1u);
            let sy = min(gid.y * 2u + (i >> 1u), pd.y - 1u);
            acc = acc + BL[pd.z + sy * pd.x + sx].xyz;
        }
    }
    BL[dm.z + gid.y * dm.x + gid.x] = vec4<f32>(acc * 0.25, 0.0);
}

fn wt(k: i32) -> f32 {
    if (k == 0) { return 0.22702703; }
    if (k == 1) { return 0.1945946; }
    if (k == 2) { return 0.12162162; }
    if (k == 3) { return 0.05405405; }
    return 0.01621622;
}

@compute @workgroup_size(8, 8, 1)
fn bloom_blur(@builtin(global_invocation_id) gid: vec3<u32>) {
    let k = U.x;
    let dm = level_dims(k);
    if (gid.x >= dm.x || gid.y >= dm.y) { return; }
    let w = i32(dm.x);
    let h = i32(dm.y);
    let x = i32(gid.x);
    let y = i32(gid.y);
    var acc = vec3<f32>(0.0);
    if (U.y == 0u) {
        acc = BL[dm.z + gid.y * dm.x + gid.x].xyz * wt(0);
        for (var i = 1; i < 5; i = i + 1) {
            let a = BL[dm.z + u32(y * w + min(x + i, w - 1))].xyz;
            let b = BL[dm.z + u32(y * w + max(x - i, 0))].xyz;
            acc = acc + (a + b) * wt(i);
        }
        BT[dm.z + gid.y * dm.x + gid.x] = vec4<f32>(acc, 0.0);
    } else {
        acc = BT[dm.z + gid.y * dm.x + gid.x].xyz * wt(0);
        for (var i = 1; i < 5; i = i + 1) {
            let a = BT[dm.z + u32(min(y + i, h - 1) * w + x)].xyz;
            let b = BT[dm.z + u32(max(y - i, 0) * w + x)].xyz;
            acc = acc + (a + b) * wt(i);
        }
        BL[dm.z + gid.y * dm.x + gid.x] = vec4<f32>(acc, 0.0);
    }
}

fn sample_level(k: u32, u: f32, v: f32) -> vec3<f32> {
    let dm = level_dims(k);
    let x = u * f32(dm.x) - 0.5;
    let y = v * f32(dm.y) - 0.5;
    let x0 = floor(x);
    let y0 = floor(y);
    let fx = x - x0;
    let fy = y - y0;
    let mx = f32(dm.x) - 1.0;
    let my = f32(dm.y) - 1.0;
    let a = BL[dm.z + u32(clamp(y0, 0.0, my)) * dm.x + u32(clamp(x0, 0.0, mx))].xyz;
    let b = BL[dm.z + u32(clamp(y0, 0.0, my)) * dm.x + u32(clamp(x0 + 1.0, 0.0, mx))].xyz;
    let c = BL[dm.z + u32(clamp(y0 + 1.0, 0.0, my)) * dm.x + u32(clamp(x0, 0.0, mx))].xyz;
    let d = BL[dm.z + u32(clamp(y0 + 1.0, 0.0, my)) * dm.x + u32(clamp(x0 + 1.0, 0.0, mx))].xyz;
    return mix(mix(a, b, fx), mix(c, d, fx), fy);
}

// ---------------------------------------------------------------- finish

fn aces(xin: f32) -> f32 {
    let x = max(xin, 0.0) * 0.9;
    return clamp((x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14), 0.0, 1.0);
}

fn filmic(c: vec3<f32>) -> vec3<f32> {
    let l = lum(c);
    let per = vec3<f32>(aces(c.x), aces(c.y), aces(c.z));
    let kept = c * (aces(l) / max(l, 1e-4));
    let out = mix(per, kept, vec3<f32>(0.5));
    let m = maxc(out);
    if (m > 1.0) { return out / m; }
    return out;
}

fn soft_shoulder(xin: f32) -> f32 {
    let x = max(xin, 0.0);
    if (x < 0.6) { return x; }
    return 0.6 + 0.4 * (1.0 - exp(-(x - 0.6) / 0.4));
}

fn grade_px(cin: vec3<f32>, alpha: f32, x: u32, y: u32) -> vec3<f32> {
    let g = P[P_FOG + 5u];
    let g2 = P[P_FOG + 6u];
    let g3 = P[P_FOG + 7u];
    var c = cin * exp2(g.x);
    let t = g2.x;
    let tint = g2.y;
    c = c * vec3<f32>(exp(0.20 * t), exp(0.02 * t - 0.12 * tint), exp(-0.20 * t));
    if (abs(g.y - 1.0) > 1e-4) {
        c = 0.18 * pow(max(c, vec3<f32>(0.0)) / 0.18, vec3<f32>(g.y));
    }
    let mode = i32(g.w);
    if (mode == 1) {
        c = filmic(c);
    } else if (mode == 2) {
        c = vec3<f32>(soft_shoulder(c.x), soft_shoulder(c.y), soft_shoulder(c.z));
    }
    var l = lum(c);
    let rich = g2.z;
    let sat = g.z * (1.0 + rich * 0.5 * (1.0 - sstep(0.0, 0.2, l)));
    if (abs(sat - 1.0) > 1e-4) {
        c = vec3<f32>(l) + (c - vec3<f32>(l)) * sat;
    }
    if (rich > 0.0) {
        let bp = 0.012 * rich;
        c = min(max(c - vec3<f32>(bp), vec3<f32>(0.0)) / (1.0 - bp), vec3<f32>(1.0e6));
        for (var i = 0; i < 3; i = i + 1) {
            let v = c[i];
            let s = clamp(v, 0.0, 1.0);
            let toe = s * s * (3.0 - 2.0 * s);
            if (v <= 1.0) { c[i] = mixf(v, toe, 0.35 * rich); }
        }
    }
    l = lum(c);
    let st = P[P_FOG + 8u];
    let ht = P[P_FOG + 9u];
    if (st.w > 0.0) { c = c + st.xyz * (st.w * 0.06 * (1.0 - sstep(0.0, 0.3, l))); }
    if (ht.w > 0.0) { c = c + ht.xyz * (ht.w * 0.06 * sstep(0.5, 1.0, l)); }
    let vig = g2.w;
    if (vig > 0.0) {
        let u = (f32(x) + 0.5) / f32(W()) * 2.0 - 1.0;
        let v = (f32(y) + 0.5) / f32(H()) * 2.0 - 1.0;
        let r2 = (u * u + v * v) * 0.5;
        c = c * (1.0 - vig * sstep(0.25, 1.0, r2));
    }
    if (g3.x > 0.0) {
        let frame = i32(floor(TIME() * 24.0));
        let n = u01(hash3(i32(x), i32(y), frame, 31u)) + u01(hash3(i32(x), i32(y), frame, 47u)) - 1.0;
        let l2 = clamp(lum(c), 0.0, 1.0);
        c = c + vec3<f32>(n * g3.x * 0.06 * (0.4 + 2.4 * l2 * (1.0 - l2)) * min(alpha, 1.0));
    }
    return max(c, vec3<f32>(0.0));
}

@compute @workgroup_size(8, 8, 1)
fn finish(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= W() || gid.y >= H()) { return; }
    let idx = gid.y * W() + gid.x;
    let u = (f32(gid.x) + 0.5) / f32(W());
    let v = (f32(gid.y) + 0.5) / f32(H());
    let px = A[idx];
    var c = px.xyz;
    let bl = P[P_FOG + 4u];
    let halation = P[P_FOG + 7u].y;
    let fringe = P[P_FOG + 7u].z;
    if (fringe > 0.0) {
        let ux = u * 2.0 - 1.0;
        let uy = v * 2.0 - 1.0;
        let r2 = (ux * ux + uy * uy) * 0.5;
        let sh = fringe * 0.004 * r2 * f32(W());
        let rl = max(sqrt(ux * ux + uy * uy), 1e-4);
        let dirx = ux / rl;
        let diry = uy / rl;
        let sxr = i32(floor(f32(gid.x) + 0.5 + dirx * sh));
        let syr = i32(floor(f32(gid.y) + 0.5 + diry * sh));
        let sxb = i32(floor(f32(gid.x) + 0.5 - dirx * sh));
        let syb = i32(floor(f32(gid.y) + 0.5 - diry * sh));
        c.x = A[at_px(sxr, syr)].x;
        c.z = A[at_px(sxb, syb)].z;
    }
    if (bl.x > 0.0) {
        // Weights of the levels from the radius.
        var wts = array<f32, 5>(0.0, 0.0, 0.0, 0.0, 0.0);
        var sum = 0.0;
        for (var k = 0u; k < BLOOM_LEVELS; k = k + 1u) {
            wts[k] = exp(-f32(k) * (1.6 - 1.3 * bl.w));
            sum = sum + wts[k];
        }
        var b = vec3<f32>(0.0);
        for (var k = 0u; k < BLOOM_LEVELS; k = k + 1u) {
            b = b + sample_level(k, u, v) * (wts[k] / sum);
        }
        c = c + b * bl.x;
        if (halation > 0.0) { c = c + b * (bl.x * halation * 1.5) * vec3<f32>(1.0, 0.32, 0.1); }
    }
    let out = grade_px(c, px.w, gid.x, gid.y);
    B[idx] = vec4<f32>(out, px.w);
}
