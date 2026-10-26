// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// p[0].x start angle, p[1].x direction, p[2].x softness (degrees),
// p[3].xy centre, p[4].x easing
fn transition(pos: vec2<f32>, t: f32) -> vec4<f32> {
    let d = pos - u.p[3].xy * u.size;
    var ang = atan2(d.x, -d.y) - radians(u.p[0].x);
    if (u.p[1].x > 0.5) {
        ang = -ang;
    }
    let m = (ang - TAU * floor(ang / TAU)) / TAU;
    let k = reveal_mask(m, ease(u.p[4].x, t), u.p[2].x / 360.0);
    return mix(tex_a(pos), tex_b(pos), k);
}
