// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// Mask shapes as signed distance fields, feather and combination.
// Mirrors `mask.rs`: the same functions, the same constants.

struct MaskGpu {
    // shape code (see MaskShape::code), mode (0 add, 1 subtract, 2 intersect), invert, polygon start
    kind: vec4<u32>,
    // centre.xy, half extents .zw (layer pixels)
    geom: vec4<f32>,
    // cos, sin, sigma, expansion
    rot: vec4<f32>,
    // roundness px, opacity, polygon count, 0
    misc: vec4<f32>,
}

fn erf_approx(x: f32) -> f32 {
    let s = select(1.0, -1.0, x < 0.0);
    let a = abs(x);
    let t = 1.0 / (1.0 + 0.3275911 * a);
    let y = 1.0 - (((((1.061405429 * t - 1.453152027) * t) + 1.421413741) * t - 0.284496736) * t + 0.254829592) * t * exp(-a * a);
    return s * y;
}

fn coverage(d: f32, sigma: f32) -> f32 {
    if (sigma <= 1e-4) { return clamp(0.5 - d, 0.0, 1.0); }
    return 0.5 * (1.0 - erf_approx(d / (sigma * 1.4142135)));
}

fn sdf_rect(p: vec2<f32>, h: vec2<f32>, r0: f32) -> f32 {
    let r = max(min(r0, min(h.x, h.y)), 0.0);
    let q = abs(p) - (h - vec2<f32>(r));
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - r;
}

fn sdf_ellipse(p: vec2<f32>, r: vec2<f32>) -> f32 {
    let rx = max(r.x, 1e-6);
    let ry = max(r.y, 1e-6);
    let k0 = length(vec2<f32>(p.x / rx, p.y / ry));
    let k1 = length(vec2<f32>(p.x / (rx * rx), p.y / (ry * ry)));
    if (k1 <= 1e-12) { return -min(rx, ry); }
    return k0 * (k0 - 1.0) / k1;
}

// Signed distance to a polygon stored in `poly[start .. start + n]`.
fn sdf_polygon(p: vec2<f32>, start: u32, n: u32) -> f32 {
    if (n < 2u) { return 1e30; }
    var d = 1e30;
    var inside = false;
    var j = n - 1u;
    for (var i = 0u; i < n; i = i + 1u) {
        let a = poly[start + j];
        let b = poly[start + i];
        let e = b - a;
        let w = p - a;
        let ee = dot(e, e);
        var t = 0.0;
        if (ee > 0.0) { t = clamp(dot(w, e) / ee, 0.0, 1.0); }
        let dv = w - e * t;
        d = min(d, dot(dv, dv));
        if ((a.y > p.y) != (b.y > p.y)) {
            let xint = a.x + (p.y - a.y) * e.x / e.y;
            if (p.x < xint) { inside = !inside; }
        }
        j = i;
    }
    let dd = sqrt(d);
    return select(dd, -dd, inside);
}

// Signed distance to a polygon given by a fixed small array (star, heart).
fn sdf_tri(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>, c: vec2<f32>) -> f32 {
    var d = 1e30;
    var inside = false;
    var vs = array<vec2<f32>, 3>(a, b, c);
    var j = 2u;
    for (var i = 0u; i < 3u; i = i + 1u) {
        let va = vs[j];
        let vb = vs[i];
        let e = vb - va;
        let w = p - va;
        let ee = dot(e, e);
        var t = 0.0;
        if (ee > 0.0) { t = clamp(dot(w, e) / ee, 0.0, 1.0); }
        let dv = w - e * t;
        d = min(d, dot(dv, dv));
        if ((va.y > p.y) != (vb.y > p.y)) {
            let xint = va.x + (p.y - va.y) * e.x / e.y;
            if (p.x < xint) { inside = !inside; }
        }
        j = i;
    }
    let dd = sqrt(d);
    return select(dd, -dd, inside);
}

fn star_vertex(i: u32, h: vec2<f32>) -> vec2<f32> {
    let r = select(0.42, 1.0, (i % 2u) == 0u);
    let a = -1.5707963 + f32(i) * 3.14159265 / 5.0;
    return vec2<f32>(r * cos(a) * h.x, r * sin(a) * h.y);
}

fn sdf_star(p: vec2<f32>, h: vec2<f32>) -> f32 {
    var d = 1e30;
    var inside = false;
    var j = 9u;
    for (var i = 0u; i < 10u; i = i + 1u) {
        let a = star_vertex(j, h);
        let b = star_vertex(i, h);
        let e = b - a;
        let w = p - a;
        let ee = dot(e, e);
        var t = 0.0;
        if (ee > 0.0) { t = clamp(dot(w, e) / ee, 0.0, 1.0); }
        let dv = w - e * t;
        d = min(d, dot(dv, dv));
        if ((a.y > p.y) != (b.y > p.y)) {
            let xint = a.x + (p.y - a.y) * e.x / e.y;
            if (p.x < xint) { inside = !inside; }
        }
        j = i;
    }
    let dd = sqrt(d);
    return select(dd, -dd, inside);
}

fn sdf_heart(p: vec2<f32>) -> f32 {
    let x = abs(p.x);
    let y = p.y;
    let lobe = length(vec2<f32>(x - 0.5, y + 0.35)) - 0.5;
    let tri = sdf_tri(vec2<f32>(x, y), vec2<f32>(-1.0, -0.35), vec2<f32>(1.0, -0.35), vec2<f32>(0.0, 1.0));
    return min(lobe, tri);
}

fn mask_distance(m: MaskGpu, x: f32, y: f32) -> f32 {
    let dx = x - m.geom.x;
    let dy = y - m.geom.y;
    let cs = m.rot.x;
    let sn = m.rot.y;
    let lx = dx * cs + dy * sn;
    let ly = -dx * sn + dy * cs;
    let h = m.geom.zw;
    var d: f32;
    switch m.kind.x {
        case 0u: { d = sdf_rect(vec2<f32>(lx, ly), h, m.misc.x); }
        case 1u: { d = sdf_ellipse(vec2<f32>(lx, ly), h); }
        case 2u: { d = ly; }
        case 3u: { d = abs(ly) - h.y; }
        case 4u: { d = sdf_star(vec2<f32>(lx, ly), h); }
        case 5u: {
            let s = max(min(h.x, h.y), 1e-6);
            d = sdf_heart(vec2<f32>(lx / max(h.x, 1e-6), ly / max(h.y, 1e-6))) * s;
        }
        default: { d = sdf_polygon(vec2<f32>(lx, ly), m.kind.w, u32(m.misc.z)); }
    }
    return d - m.rot.w;
}

fn mask_coverage(m: MaskGpu, x: f32, y: f32) -> f32 {
    var c = coverage(mask_distance(m, x, y), m.rot.z);
    if (m.kind.z != 0u) { c = 1.0 - c; }
    return c * clamp(m.misc.y, 0.0, 1.0);
}

fn combine(acc: f32, mode: u32, c: f32) -> f32 {
    switch mode {
        case 0u: { return acc + c - acc * c; }
        case 1u: { return acc * (1.0 - c); }
        default: { return acc * c; }
    }
}

// Combined coverage of all masks at a pixel position.
fn masks_at(count: u32, init: f32, x: f32, y: f32) -> f32 {
    var acc = init;
    for (var i = 0u; i < count; i = i + 1u) {
        let m = masks[i];
        acc = combine(acc, m.kind.y, mask_coverage(m, x, y));
    }
    return clamp(acc, 0.0, 1.0);
}
