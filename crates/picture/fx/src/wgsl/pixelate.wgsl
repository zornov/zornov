// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// p[0].x size (% height). The GPU path samples a 4x4 grid inside each
// cell instead of averaging every pixel; cells are anchored at the centre.
fn effect(pos: vec2<f32>) -> vec4<f32> {
    let cell = pct_h(u.p[0].x);
    if (cell < 1.0) {
        return tex(pos);
    }
    let half_size = u.size * 0.5;
    let base = floor((pos - half_size) / cell) * cell + half_size;
    var acc = vec4<f32>(0.0);
    for (var j = 0; j < 4; j = j + 1) {
        for (var i = 0; i < 4; i = i + 1) {
            let q = base + (vec2<f32>(f32(i), f32(j)) + 0.5) * cell * 0.25;
            acc = acc + tex(clamp(q, vec2<f32>(0.5), u.size - 0.5));
        }
    }
    return acc / 16.0;
}
