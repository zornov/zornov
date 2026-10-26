// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// Lens distortion. p: mode (0 undistort, 1 distort), k1, k2, centre,
// scale, chroma, edge (0 transparent, 1 clamp, 2 mirror).
fn distort_r(q: vec2<f32>, k1: f32, k2: f32) -> vec2<f32> {
    let r2 = dot(q, q);
    return q * (1.0 + k1 * r2 + k2 * r2 * r2);
}

fn undistort_r(d: vec2<f32>, k1: f32, k2: f32) -> vec2<f32> {
    let rd = length(d);
    if (rd < 1e-12) {
        return d;
    }
    var r = rd;
    for (var i = 0; i < 20; i = i + 1) {
        let f = r * (1.0 + k1 * r * r + k2 * r * r * r * r) - rd;
        let df = 1.0 + 3.0 * k1 * r * r + 5.0 * k2 * r * r * r * r;
        if (abs(df) < 1e-9) {
            break;
        }
        let step = f / df;
        r = r - step;
        if (abs(step) < 1e-7) {
            break;
        }
    }
    return d * (r / rd);
}

fn lens_map(pos: vec2<f32>, chan: f32) -> vec2<f32> {
    let c = u.p[3].xy * u.size;
    let norm = 0.5 * length(u.size);
    let q = (pos - c) / norm / u.p[4].x;
    var s: vec2<f32>;
    if (u.p[0].x < 0.5) {
        s = distort_r(q, u.p[1].x, u.p[2].x);
    } else {
        s = undistort_r(q, u.p[1].x, u.p[2].x);
    }
    return c + s * norm * (1.0 + chan);
}

fn edge_i(i: i32, n: i32, mode: i32) -> i32 {
    if (mode == 2) {
        let period = 2 * n;
        let m = ((i % period) + period) % period;
        return select(period - 1 - m, m, m < n);
    }
    return clamp(i, 0, n - 1);
}

fn fetch_edge(x: i32, y: i32, mode: i32) -> vec4<f32> {
    let w = i32(u.size.x);
    let h = i32(u.size.y);
    if (mode == 0 && (x < 0 || y < 0 || x >= w || y >= h)) {
        return vec4<f32>(0.0);
    }
    return textureLoad(a_tex, vec2<i32>(edge_i(x, w, mode), edge_i(y, h, mode)), 0);
}

fn sample_edge(pos: vec2<f32>, mode: i32) -> vec4<f32> {
    let p = clamp(pos, vec2<f32>(-1.0e7), vec2<f32>(1.0e7)) - vec2<f32>(0.5);
    let p0 = floor(p);
    let f = p - p0;
    let x = i32(p0.x);
    let y = i32(p0.y);
    let top = mix(fetch_edge(x, y, mode), fetch_edge(x + 1, y, mode), f.x);
    let bottom = mix(fetch_edge(x, y + 1, mode), fetch_edge(x + 1, y + 1, mode), f.x);
    return mix(top, bottom, f.y);
}

fn node(pos: vec2<f32>) -> vec4<f32> {
    let mode = i32(u.p[6].x + 0.5);
    let ca = u.p[5].x;
    let g = lens_map(pos, 0.0);
    let here = aux_a(cur);
    let moved = textureSampleLevel(a_aux, samp, g / u.size, 0.0);
    aux_out = vec4<f32>(moved.x, here.y, here.z, moved.w);
    let mid = sample_edge(g, mode);
    if (ca == 0.0) {
        return mid;
    }
    let und = u.p[0].x < 0.5;
    let r = sample_edge(lens_map(pos, select(ca, -ca, und)), mode);
    let b = sample_edge(lens_map(pos, select(-ca, ca, und)), mode);
    return vec4<f32>(r.r, mid.g, b.b, mid.a);
}
