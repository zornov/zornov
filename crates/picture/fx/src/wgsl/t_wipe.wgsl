// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// p[0].x angle, p[1].x softness, p[2].x easing
fn transition(pos: vec2<f32>, t: f32) -> vec4<f32> {
    let a = radians(u.p[0].x);
    let d = vec2<f32>(cos(a), sin(a));
    let c0 = 0.0;
    let c1 = u.size.x * d.x;
    let c2 = u.size.y * d.y;
    let c3 = dot(u.size, d);
    let lo = min(min(c0, c1), min(c2, c3));
    let hi = max(max(c0, c1), max(c2, c3));
    let m = (dot(pos, d) - lo) / max(hi - lo, 0.000001);
    let k = reveal_mask(m, ease(u.p[2].x, t), u.p[1].x);
    return mix(tex_a(pos), tex_b(pos), k);
}
