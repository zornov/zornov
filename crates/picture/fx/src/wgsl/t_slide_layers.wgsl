// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// Two-layer slide shared by slide, push, cover and reveal.
fn shifted_a(pos: vec2<f32>, off: vec2<f32>) -> vec4<f32> {
    let q = pos - off;
    if (!inside(q)) {
        return vec4<f32>(0.0);
    }
    return tex_a(q);
}

fn shifted_b(pos: vec2<f32>, off: vec2<f32>) -> vec4<f32> {
    let q = pos - off;
    if (!inside(q)) {
        return vec4<f32>(0.0);
    }
    return tex_b(q);
}

fn over(top: vec4<f32>, bottom: vec4<f32>) -> vec4<f32> {
    return top + bottom * (1.0 - top.a);
}

// Signed distance (pixels) from a moving layer's leading or trailing edge.
fn edge_dist(pos: vec2<f32>, off: vec2<f32>, dir: vec2<f32>, leading: bool) -> f32 {
    let n = dir / max(abs(dir.x) + abs(dir.y), 1.0);
    let along = dot(pos - off, n);
    let size = u.size.x * abs(n.x) + u.size.y * abs(n.y);
    var p = along;
    if (n.x + n.y <= 0.0) {
        p = along + size;
    }
    if (leading) {
        return p - size;
    }
    return -p;
}

fn slide_layers(pos: vec2<f32>, e: f32, dir: vec2<f32>, ka: f32, b_moves: bool, b_on_top: bool, shadow: f32, dim_a: f32) -> vec4<f32> {
    let ao = dir * ka * e;
    var bo = vec2<f32>(0.0);
    if (b_moves) {
        bo = dir * (e - 1.0);
    }
    let pa = shifted_a(pos, ao) * (1.0 - dim_a * e);
    let pb = shifted_b(pos, bo);
    var d: f32;
    if (b_on_top) {
        d = edge_dist(pos, bo, dir, true);
    } else {
        d = edge_dist(pos, ao, dir, false);
    }
    var s = 0.0;
    if (shadow > 0.0 && d > 0.0) {
        s = shadow * 0.6 * exp(-d / (0.06 * u.size.y)) * smoothstep(0.0, 0.05, e);
    }
    if (b_on_top) {
        return over(pb, pa * (1.0 - s));
    }
    return over(pa, pb * (1.0 - s));
}
