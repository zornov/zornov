// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// p[0].x cell, p[1].x width (% height), p[2] line colour, p[3] background, p[4].x mix
fn dist(v: f32, cell: f32) -> f32 {
    let f = fract(v / cell);
    return min(f, 1.0 - f) * cell;
}

fn effect(pos: vec2<f32>) -> vec4<f32> {
    let cell = max(pct_h(u.p[0].x), 1.0);
    let half_w = pct_h(u.p[1].x) * 0.5;
    let c = pos - u.size * 0.5;
    let d = min(dist(c.x, cell), dist(c.y, cell));
    let k = smoothstep(half_w + 0.5, half_w - 0.5, d);
    let g = mix(u.p[3], u.p[2], k) * u.p[4].x;
    return g + tex(pos) * (1.0 - g.a);
}
