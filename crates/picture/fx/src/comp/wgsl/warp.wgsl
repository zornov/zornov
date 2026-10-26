// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// Warps by an output → source matrix in p[0..3] with the edge mode in
// p[3].x (0 transparent, 1 clamp, 2 mirror, 3 wrap). Depth and ids move
// with the picture; motion stays.
fn edge_i(i: i32, n: i32, mode: i32) -> i32 {
    if (mode == 2) {
        let period = 2 * n;
        let m = ((i % period) + period) % period;
        return select(period - 1 - m, m, m < n);
    }
    if (mode == 3) {
        return ((i % n) + n) % n;
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
    if (!(abs(pos.x) < 1.0e7) || !(abs(pos.y) < 1.0e7)) {
        return vec4<f32>(0.0);
    }
    let p = pos - vec2<f32>(0.5);
    let p0 = floor(p);
    let f = p - p0;
    let x = i32(p0.x);
    let y = i32(p0.y);
    let top = mix(fetch_edge(x, y, mode), fetch_edge(x + 1, y, mode), f.x);
    let bottom = mix(fetch_edge(x, y + 1, mode), fetch_edge(x + 1, y + 1, mode), f.x);
    return mix(top, bottom, f.y);
}

fn node(pos: vec2<f32>) -> vec4<f32> {
    let s = mat_apply(0, pos);
    let here = aux_a(cur);
    let moved = textureSampleLevel(a_aux, samp, s / u.size, 0.0);
    aux_out = vec4<f32>(moved.x, here.y, here.z, moved.w);
    return sample_edge(s, i32(u.p[3].x + 0.5));
}
