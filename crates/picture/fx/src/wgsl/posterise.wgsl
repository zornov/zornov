// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// p[0].x levels, p[1].x mix
fn to_srgb(v: vec3<f32>) -> vec3<f32> {
    let lo = v * 12.92;
    let hi = 1.055 * pow(max(v, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(hi, lo, v <= vec3<f32>(0.0031308));
}

fn to_linear(v: vec3<f32>) -> vec3<f32> {
    let lo = v / 12.92;
    let hi = pow((max(v, vec3<f32>(0.0)) + 0.055) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, v <= vec3<f32>(0.04045));
}

fn effect(pos: vec2<f32>) -> vec4<f32> {
    let c = tex(pos);
    if (c.a <= 0.000001) {
        return c;
    }
    let n = max(u.p[0].x - 1.0, 1.0);
    let s = clamp(to_srgb(c.rgb / c.a), vec3<f32>(0.0), vec3<f32>(1.0));
    let q = floor(s * n + 0.5) / n;
    let out = vec4<f32>(to_linear(q) * c.a, c.a);
    return mix(c, out, u.p[1].x);
}
