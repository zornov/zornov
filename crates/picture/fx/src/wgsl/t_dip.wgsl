// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// p[0] colour (premultiplied), p[1].x hold
fn transition(pos: vec2<f32>, t: f32) -> vec4<f32> {
    let hold = u.p[1].x;
    let half_t = (1.0 - hold) * 0.5;
    let col = u.p[0];
    if (t < half_t) {
        return mix(tex_a(pos), col, smoothstep(0.0, 1.0, t / half_t));
    }
    if (t <= half_t + hold) {
        return col;
    }
    return mix(col, tex_b(pos), smoothstep(0.0, 1.0, (t - half_t - hold) / half_t));
}
