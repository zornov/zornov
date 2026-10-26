// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// p[0].x size (% height), p[1] colour a, p[2] colour b, p[3].x angle, p[4].x mix
fn edge(t: f32, size: f32) -> f32 {
    let f = fract(t);
    return min(f, 1.0 - f) * size;
}

fn effect(pos: vec2<f32>) -> vec4<f32> {
    let size = max(pct_h(u.p[0].x), 1.0);
    let a = radians(u.p[3].x);
    let d = pos - u.size * 0.5;
    let uv = vec2<f32>(d.x * cos(a) + d.y * sin(a), -d.x * sin(a) + d.y * cos(a)) / size;
    let parity = (i32(floor(uv.x)) + i32(floor(uv.y))) & 1;
    let soft = smoothstep(0.0, 1.0, min(edge(uv.x, size), edge(uv.y, size)));
    var k = 0.5 - 0.5 * soft;
    if (parity == 0) {
        k = 0.5 + 0.5 * soft;
    }
    let g = mix(u.p[2], u.p[1], k) * u.p[4].x;
    return g + tex(pos) * (1.0 - g.a);
}
