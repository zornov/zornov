// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// Fine rasterization: one workgroup per 16×16 tile, one invocation per
// pixel. Each pixel runs its tile's commands in order: fills integrate the
// exact area of the pixel inside the path (the same sum the CPU renderer
// accumulates), paints are evaluated at the pixel centre, and isolated
// groups use a small stack. Formulas mirror cpu.rs, shade.rs and blend.rs.

struct Config {
    origin: vec2<i32>,
    size: vec2<u32>,
    tiles_x: u32,
    dither: u32,
    paint_off: u32,
    has_bg: u32,
    bg: vec4<f32>,
}

@group(0) @binding(0) var<uniform> cfg: Config;
@group(0) @binding(1) var<storage, read> udata: array<u32>;
@group(0) @binding(2) var<storage, read> fdata: array<vec4<f32>>;
@group(0) @binding(3) var<storage, read> images: array<u32>;
@group(0) @binding(4) var<storage, read> descs: array<vec4<u32>>;
@group(0) @binding(5) var<storage, read_write> out_u: array<u32>;
@group(0) @binding(6) var<storage, read_write> out_f: array<vec4<f32>>;

const OP_FILL: u32 = 1u;
const OP_SOLID: u32 = 2u;
const OP_PUSH: u32 = 3u;
const OP_POP: u32 = 4u;
const LUT_SIZE: u32 = 512u;
const TAU: f32 = 6.283185307179586;

fn phi(u: f32) -> f32 {
    if (u <= 0.0) { return 0.0; }
    if (u >= 1.0) { return u - 0.5; }
    return 0.5 * u * u;
}

// Signed area segment `s` contributes to the pixel whose top-left is `p`.
fn seg_cov(s: vec4<f32>, p: vec2<f32>) -> f32 {
    if (s.y == s.w) { return 0.0; }
    var sgn = 1.0;
    var a = s.xy;
    var b = s.zw;
    if (s.w < s.y) { sgn = -1.0; a = s.zw; b = s.xy; }
    let ys = max(a.y, p.y);
    let ye = min(b.y, p.y + 1.0);
    if (ys >= ye) { return 0.0; }
    let dxdy = (b.x - a.x) / (b.y - a.y);
    let xs = a.x + (ys - a.y) * dxdy - p.x;
    let xe = a.x + (ye - a.y) * dxdy - p.x;
    let lo = min(xs, xe);
    let hi = max(xs, xe);
    let dy = (ye - ys) * sgn;
    if (hi <= 0.0) { return dy; }
    if (lo >= 1.0) { return 0.0; }
    if (hi - lo < 1e-3) { return dy * clamp(1.0 - 0.5 * (lo + hi), 0.0, 1.0); }
    return dy * (phi(1.0 - lo) - phi(1.0 - hi)) / (hi - lo);
}

fn fold(flags: u32, w: f32) -> f32 {
    var c: f32;
    if ((flags & 1u) != 0u) {
        let t = abs(w) % 2.0;
        c = select(t, 2.0 - t, t > 1.0);
    } else {
        c = min(abs(w), 1.0);
    }
    if ((flags & 2u) != 0u) { c = 1.0 - c; }
    return c;
}

fn coverage(list: u32, count: u32, backdrop: u32, flags: u32, p: vec2<f32>, row: u32) -> f32 {
    var w = fdata[backdrop + row / 4u][row % 4u];
    for (var i = 0u; i < count; i++) {
        w += seg_cov(fdata[udata[list + i]], p);
    }
    return fold(flags, w);
}

// ---------------------------------------------------------------- paints

fn pw(paint: u32, i: u32) -> u32 { return udata[paint + i]; }
fn pf(paint: u32, i: u32) -> f32 { return bitcast<f32>(udata[paint + i]); }

fn apply_inv(paint: u32, x: f32, y: f32) -> vec2<f32> {
    return vec2<f32>(pf(paint, 4u) * x + pf(paint, 6u) * y + pf(paint, 8u), pf(paint, 5u) * x + pf(paint, 7u) * y + pf(paint, 9u));
}

fn extend_t(t: f32, e: u32) -> f32 {
    if (e == 1u) { return t - floor(t); }
    if (e == 2u) {
        let u = t - 2.0 * floor(t / 2.0);
        return select(u, 2.0 - u, u > 1.0);
    }
    return clamp(t, 0.0, 1.0);
}

fn lut(off: u32, t: f32) -> vec4<f32> {
    let x = clamp(t, 0.0, 1.0) * f32(LUT_SIZE - 1u);
    let i = min(u32(floor(x)), LUT_SIZE - 2u);
    let f = x - f32(i);
    return mix(fdata[off + i], fdata[off + i + 1u], f);
}

fn radial_t(p: vec2<f32>, c0: vec2<f32>, r0: f32, c1: vec2<f32>, r1: f32, ok: ptr<function, bool>) -> f32 {
    let cd = c1 - c0;
    let dr = r1 - r0;
    let pd = p - c0;
    let a = dot(cd, cd) - dr * dr;
    let b = dot(pd, cd) + r0 * dr;
    let c = dot(pd, pd) - r0 * r0;
    *ok = true;
    if (abs(a) < 1e-6 * max(dot(cd, cd) + dr * dr, 1e-12)) {
        if (abs(b) < 1e-12) { *ok = false; return 0.0; }
        let t = c / (2.0 * b);
        *ok = r0 + t * dr >= 0.0;
        return t;
    }
    let disc = b * b - a * c;
    if (disc < 0.0) { *ok = false; return 0.0; }
    let sq = sqrt(disc);
    let t1 = (b + sq) / a;
    let t2 = (b - sq) / a;
    let hi = max(t1, t2);
    let lo = min(t1, t2);
    if (r0 + hi * dr >= 0.0) { return hi; }
    if (r0 + lo * dr >= 0.0) { return lo; }
    *ok = false;
    return 0.0;
}

fn wrap(i: i32, n: i32, e: u32, ok: ptr<function, bool>) -> i32 {
    if (e == 0u) { return clamp(i, 0, n - 1); }
    if (e == 3u) {
        if (i < 0 || i >= n) { *ok = false; }
        return clamp(i, 0, n - 1);
    }
    if (e == 1u) { return ((i % n) + n) % n; }
    let m = ((i % (2 * n)) + 2 * n) % (2 * n);
    return select(m, 2 * n - 1 - m, m >= n);
}

fn texel(d: vec4<u32>, x: i32, y: i32, e: u32) -> vec4<f32> {
    var ok = true;
    let xi = wrap(x, i32(d.y), e, &ok);
    let yi = wrap(y, i32(d.z), e, &ok);
    if (!ok) { return vec4<f32>(0.0); }
    return unpack4x8unorm(images[d.x + u32(yi) * d.y + u32(xi)]);
}

fn bilinear(d: vec4<u32>, u: f32, v: f32, e: u32) -> vec4<f32> {
    let x = u - 0.5;
    let y = v - 0.5;
    let x0 = floor(x);
    let y0 = floor(y);
    let fx = x - x0;
    let fy = y - y0;
    let xi = i32(x0);
    let yi = i32(y0);
    let top = mix(texel(d, xi, yi, e), texel(d, xi + 1, yi, e), fx);
    let bot = mix(texel(d, xi, yi + 1, e), texel(d, xi + 1, yi + 1, e), fx);
    return mix(top, bot, fy);
}

fn catmull_rom(t: f32) -> vec4<f32> {
    let t2 = t * t;
    let t3 = t2 * t;
    return vec4<f32>(-0.5 * t3 + t2 - 0.5 * t, 1.5 * t3 - 2.5 * t2 + 1.0, -1.5 * t3 + 2.0 * t2 + 0.5 * t, 0.5 * t3 - 0.5 * t2);
}

fn bicubic(d: vec4<u32>, u: f32, v: f32, e: u32) -> vec4<f32> {
    let x = u - 0.5;
    let y = v - 0.5;
    let x0 = floor(x);
    let y0 = floor(y);
    let wx = catmull_rom(x - x0);
    let wy = catmull_rom(y - y0);
    let xi = i32(x0) - 1;
    let yi = i32(y0) - 1;
    var o = vec4<f32>(0.0);
    for (var j = 0; j < 4; j++) {
        var row = vec4<f32>(0.0);
        for (var i = 0; i < 4; i++) {
            row += texel(d, xi + i, yi + j, e) * wx[i];
        }
        o += row * wy[j];
    }
    let a = clamp(o.w, 0.0, 1.0);
    return vec4<f32>(clamp(o.xyz, vec3<f32>(0.0), vec3<f32>(a)), a);
}

fn sample_level(d: vec4<u32>, u: f32, v: f32, e: u32, q: u32, cubic: bool) -> vec4<f32> {
    if (q == 0u) { return texel(d, i32(floor(u)), i32(floor(v)), e); }
    if (cubic) { return bicubic(d, u, v, e); }
    return bilinear(d, u, v, e);
}

fn sample_image(desc: u32, u: f32, v: f32, e: u32, q: u32, lod: f32) -> vec4<f32> {
    let d = descs[desc * 2u];
    let levels = d.w;
    if (q == 0u || lod <= 1.0 || levels <= 1u) {
        return sample_level(d, u, v, e, q, q == 2u && lod < 1.0);
    }
    let level = log2(lod);
    let l0 = u32(floor(level));
    let f = level - f32(l0);
    let la = min(l0, levels - 1u);
    let lb = min(l0 + 1u, levels - 1u);
    let da = descs[(desc + la) * 2u];
    let db = descs[(desc + lb) * 2u];
    let a = sample_level(da, u * f32(da.y) / f32(d.y), v * f32(da.z) / f32(d.z), e, q, false);
    if (f < 1e-3) { return a; }
    let b = sample_level(db, u * f32(db.y) / f32(d.y), v * f32(db.z) / f32(d.z), e, q, false);
    return mix(a, b, f);
}

fn paint_at(paint: u32, x: f32, y: f32) -> vec4<f32> {
    let kind = pw(paint, 0u);
    if (kind == 0u) {
        return vec4<f32>(pf(paint, 4u), pf(paint, 5u), pf(paint, 6u), pf(paint, 7u));
    }
    let e = pw(paint, 1u);
    if (kind == 1u) {
        let p = apply_inv(paint, x, y);
        return lut(pw(paint, 2u), extend_t(p.x, e));
    }
    if (kind == 2u) {
        let p = apply_inv(paint, x, y);
        var ok = true;
        let t = radial_t(p, vec2<f32>(pf(paint, 10u), pf(paint, 11u)), pf(paint, 12u), vec2<f32>(pf(paint, 13u), pf(paint, 14u)), pf(paint, 15u), &ok);
        if (!ok) { return vec4<f32>(0.0); }
        return lut(pw(paint, 2u), extend_t(t, e));
    }
    if (kind == 3u) {
        let p = apply_inv(paint, x, y);
        var t = atan2(p.y, p.x) / TAU;
        t = t - floor(t);
        return lut(pw(paint, 2u), extend_t(t, e));
    }
    if (kind == 4u) {
        let off = pw(paint, 2u);
        let n = pw(paint, 3u);
        var acc = vec4<f32>(0.0);
        var wsum = 0.0;
        for (var i = 0u; i < n; i++) {
            let pt = fdata[off + i * 2u];
            let c = fdata[off + i * 2u + 1u];
            let d = vec2<f32>(x, y) - pt.xy;
            let d2 = dot(d, d);
            if (d2 < 1e-6) { return c; }
            let w = pt.z / (d2 * sqrt(sqrt(d2)));
            wsum += w;
            acc += c * w;
        }
        return acc / max(wsum, 1e-30);
    }
    // Image.
    let p = apply_inv(paint, x, y);
    return sample_image(pw(paint, 2u), p.x, p.y, e, pw(paint, 3u), pf(paint, 11u)) * pf(paint, 10u);
}

// ---------------------------------------------------------------- blends

fn hard_light(cb: f32, cs: f32) -> f32 {
    if (cs <= 0.5) { return cb * 2.0 * cs; }
    let s = 2.0 * cs - 1.0;
    return cb + s - cb * s;
}

fn sep(mode: u32, cb: f32, cs: f32) -> f32 {
    switch (mode) {
        case 1u: { return cb * cs; }
        case 2u: { return cb + cs - cb * cs; }
        case 3u: { return hard_light(cs, cb); }
        case 4u: { return min(cb, cs); }
        case 5u: { return max(cb, cs); }
        case 6u: {
            if (cb <= 0.0) { return 0.0; }
            if (cs >= 1.0) { return 1.0; }
            return min(cb / (1.0 - cs), 1.0);
        }
        case 7u: {
            if (cb >= 1.0) { return 1.0; }
            if (cs <= 0.0) { return 0.0; }
            return 1.0 - min((1.0 - cb) / cs, 1.0);
        }
        case 8u: { return hard_light(cb, cs); }
        case 9u: {
            if (cs <= 0.5) { return cb - (1.0 - 2.0 * cs) * cb * (1.0 - cb); }
            var d: f32;
            if (cb <= 0.25) { d = ((16.0 * cb - 12.0) * cb + 4.0) * cb; } else { d = sqrt(cb); }
            return cb + (2.0 * cs - 1.0) * (d - cb);
        }
        case 10u: { return abs(cb - cs); }
        case 11u: { return cb + cs - 2.0 * cb * cs; }
        default: { return cs; }
    }
}

fn lum(c: vec3<f32>) -> f32 { return 0.3 * c.x + 0.59 * c.y + 0.11 * c.z; }

fn clip_color(c0: vec3<f32>) -> vec3<f32> {
    var c = c0;
    let l = lum(c);
    let n = min(c.x, min(c.y, c.z));
    let x = max(c.x, max(c.y, c.z));
    if (n < 0.0) { c = l + (c - l) * l / max(l - n, 1e-9); }
    if (x > 1.0) { c = l + (c - l) * (1.0 - l) / max(x - l, 1e-9); }
    return c;
}

fn set_lum(c: vec3<f32>, l: f32) -> vec3<f32> { return clip_color(c + (l - lum(c))); }

fn sat(c: vec3<f32>) -> f32 { return max(c.x, max(c.y, c.z)) - min(c.x, min(c.y, c.z)); }

fn set_sat(c: vec3<f32>, s: f32) -> vec3<f32> {
    let mx = max(c.x, max(c.y, c.z));
    let mn = min(c.x, min(c.y, c.z));
    if (mx - mn <= 1e-9) { return vec3<f32>(0.0); }
    return (c - mn) * s / (mx - mn);
}

fn blend(d: vec4<f32>, s: vec4<f32>, mode: u32) -> vec4<f32> {
    if (mode == 0u) { return s + d * (1.0 - s.w); }
    if (mode == 16u) { return min(d + s, vec4<f32>(1.0)); }
    let sa = s.w;
    let da = d.w;
    if (sa <= 0.0) { return d; }
    if (da <= 0.0) { return s; }
    let cs = s.xyz / sa;
    let cb = d.xyz / da;
    var m: vec3<f32>;
    if (mode == 12u) { m = set_lum(set_sat(cs, sat(cb)), lum(cb)); }
    else if (mode == 13u) { m = set_lum(set_sat(cb, sat(cs)), lum(cb)); }
    else if (mode == 14u) { m = set_lum(cs, lum(cb)); }
    else if (mode == 15u) { m = set_lum(cb, lum(cs)); }
    else { m = vec3<f32>(sep(mode, cb.x, cs.x), sep(mode, cb.y, cs.y), sep(mode, cb.z, cs.z)); }
    let rgb = s.xyz * (1.0 - da) + d.xyz * (1.0 - sa) + sa * da * clamp(m, vec3<f32>(0.0), vec3<f32>(1.0));
    return vec4<f32>(rgb, sa + da - sa * da);
}

fn mask_value(desc: u32, flags: u32, x: i32, y: i32) -> f32 {
    let d = descs[desc * 2u];
    let o = descs[desc * 2u + 1u];
    let lx = x - bitcast<i32>(o.x);
    let ly = y - bitcast<i32>(o.y);
    var v = 0.0;
    if (lx >= 0 && ly >= 0 && lx < i32(d.y) && ly < i32(d.z)) {
        let p = unpack4x8unorm(images[d.x + u32(ly) * d.y + u32(lx)]);
        if ((flags & 1u) != 0u) { v = p.w; } else { v = 0.2125 * p.x + 0.7154 * p.y + 0.0721 * p.z; }
    }
    v = clamp(v, 0.0, 1.0);
    if ((flags & 2u) != 0u) { v = 1.0 - v; }
    return v;
}

// ---------------------------------------------------------------- main

const BAYER: array<u32, 64> = array<u32, 64>(
    0u, 32u, 8u, 40u, 2u, 34u, 10u, 42u, 48u, 16u, 56u, 24u, 50u, 18u, 58u, 26u, 12u, 44u, 4u, 36u, 14u, 46u, 6u, 38u, 60u, 28u, 52u, 20u, 62u, 30u, 54u, 22u,
    3u, 35u, 11u, 43u, 1u, 33u, 9u, 41u, 51u, 19u, 59u, 27u, 49u, 17u, 57u, 25u, 15u, 47u, 7u, 39u, 13u, 45u, 5u, 37u, 63u, 31u, 55u, 23u, 61u, 29u, 53u, 21u);

fn run(tile: vec2<u32>, local: vec2<u32>) -> vec4<f32> {
    let gx = cfg.origin.x + i32(tile.x * 16u + local.x);
    let gy = cfg.origin.y + i32(tile.y * 16u + local.y);
    let p = vec2<f32>(f32(gx), f32(gy));
    let centre = p + vec2<f32>(0.5);
    let t = tile.y * cfg.tiles_x + tile.x;
    let off = udata[t * 2u];
    let end = off + udata[t * 2u + 1u];
    var color = select(vec4<f32>(0.0), cfg.bg, cfg.has_bg != 0u);
    var stack: array<vec4<f32>, 8>;
    var sp = 0u;
    var i = off;
    loop {
        if (i >= end) { break; }
        let op = udata[i];
        if (op == OP_FILL) {
            let c = coverage(udata[i + 1u], udata[i + 2u], udata[i + 3u], udata[i + 4u], p, local.y);
            if (c > 0.0) {
                let k = c * bitcast<f32>(udata[i + 7u]);
                let s = paint_at(udata[i + 5u], centre.x, centre.y) * k;
                let mode = udata[i + 6u];
                if (s.w > 0.0 || mode == 16u) { color = blend(color, s, mode); }
            }
            i += 8u;
        } else if (op == OP_SOLID) {
            let k = bitcast<f32>(udata[i + 1u]) * bitcast<f32>(udata[i + 4u]);
            let s = paint_at(udata[i + 2u], centre.x, centre.y) * k;
            let mode = udata[i + 3u];
            if (s.w > 0.0 || mode == 16u) { color = blend(color, s, mode); }
            i += 5u;
        } else if (op == OP_PUSH) {
            if (sp < 8u) { stack[sp] = color; }
            sp += 1u;
            color = vec4<f32>(0.0);
            i += 1u;
        } else {
            var k = bitcast<f32>(udata[i + 2u]);
            let flags = udata[i + 6u];
            if ((flags & 4u) != 0u) {
                k *= coverage(udata[i + 3u], udata[i + 4u], udata[i + 5u], flags, p, local.y);
            } else if ((flags & 8u) != 0u) {
                k *= bitcast<f32>(udata[i + 3u]);
            }
            let mdesc = udata[i + 7u];
            if (mdesc != 0xffffffffu) {
                k *= mask_value(mdesc, udata[i + 8u], gx, gy);
            }
            let layer = color * k;
            sp -= 1u;
            color = select(vec4<f32>(0.0), stack[min(sp, 7u)], sp < 8u);
            color = blend(color, layer, udata[i + 1u]);
            i += 9u;
        }
    }
    return color;
}

fn to_u8(v: f32, d: f32) -> u32 {
    return u32(clamp(floor(v * 255.0 + d + 0.5), 0.0, 255.0));
}

@compute @workgroup_size(16, 16)
fn fine_rgba8(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_id) lid: vec3<u32>) {
    let x = wg.x * 16u + lid.x;
    let y = wg.y * 16u + lid.y;
    if (x >= cfg.size.x || y >= cfg.size.y) { return; }
    let c = run(wg.xy, lid.xy);
    var d = 0.0;
    if (cfg.dither != 0u) {
        let gx = u32(cfg.origin.x) + x;
        let gy = u32(cfg.origin.y) + y;
        d = (f32(BAYER[(gy & 7u) * 8u + (gx & 7u)]) + 0.5) / 64.0 - 0.5;
    }
    let a = to_u8(c.w, d);
    let r = min(to_u8(c.x, d), a);
    let g = min(to_u8(c.y, d), a);
    let b = min(to_u8(c.z, d), a);
    out_u[y * cfg.size.x + x] = r | (g << 8u) | (b << 16u) | (a << 24u);
}

@compute @workgroup_size(16, 16)
fn fine_f32(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_id) lid: vec3<u32>) {
    let x = wg.x * 16u + lid.x;
    let y = wg.y * 16u + lid.y;
    if (x >= cfg.size.x || y >= cfg.size.y) { return; }
    out_f[y * cfg.size.x + x] = run(wg.xy, lid.xy);
}
