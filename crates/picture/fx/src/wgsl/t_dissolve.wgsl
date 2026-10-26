// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// p[0].x curve (0 linear, 1 smooth)
fn transition(pos: vec2<f32>, t: f32) -> vec4<f32> {
    var k = t;
    if (u.p[0].x > 0.5) {
        k = smoothstep(0.0, 1.0, t);
    }
    return mix(tex_a(pos), tex_b(pos), k);
}
