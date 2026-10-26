// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// p: blackpoint, whitepoint, lift, gain, multiply, offset, gamma, tint,
// saturation, clamp black.
fn grade_v(v: f32) -> f32 {
    let bp = u.p[0].x;
    let wp = u.p[1].x;
    let lift = u.p[2].x;
    let gain = u.p[3].x;
    let a = u.p[4].x * (gain - lift) / max(wp - bp, 1e-6);
    let b = u.p[5].x + lift - a * bp;
    var o = a * v + b;
    if (u.p[9].x > 0.5) {
        o = max(o, 0.0);
    }
    let g = u.p[6].x;
    if (abs(g - 1.0) > 1e-6 && o > 0.0) {
        o = pow(o, 1.0 / g);
    }
    return o;
}

fn node(pos: vec2<f32>) -> vec4<f32> {
    let q = load_a(cur);
    if (q.a <= 1e-6) {
        return q;
    }
    let c = q.rgb / q.a;
    let tint = u.p[7].rgb / max(u.p[7].a, 1e-6);
    let g = vec3<f32>(grade_v(c.r), grade_v(c.g), grade_v(c.b)) * tint;
    let l = luma(g);
    let s = vec3<f32>(l) + (g - vec3<f32>(l)) * u.p[8].x;
    return vec4<f32>(s * q.a, q.a);
}
