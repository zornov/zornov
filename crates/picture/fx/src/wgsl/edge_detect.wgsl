// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// p[0].x mode, p[1].x strength, p[2].x mix
fn enc(v: vec3<f32>) -> vec3<f32> {
    let lo = v * 12.92;
    let hi = 1.055 * pow(max(v, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(hi, lo, v <= vec3<f32>(0.0031308));
}

fn dec(v: vec3<f32>) -> vec3<f32> {
    let lo = v / 12.92;
    let hi = pow((max(v, vec3<f32>(0.0)) + 0.055) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, v <= vec3<f32>(0.04045));
}

// Perceptual r, g, b and luma at an offset.
fn s(pos: vec2<f32>, dx: f32, dy: f32) -> vec4<f32> {
    let c = tex(pos + vec2<f32>(dx, dy));
    let e = enc(max(c.rgb, vec3<f32>(0.0)));
    return vec4<f32>(e, enc(vec3<f32>(max(luma(c), 0.0))).x);
}

fn effect(pos: vec2<f32>) -> vec4<f32> {
    let c = tex(pos);
    let gx = s(pos, 1.0, -1.0) + 2.0 * s(pos, 1.0, 0.0) + s(pos, 1.0, 1.0)
        - s(pos, -1.0, -1.0) - 2.0 * s(pos, -1.0, 0.0) - s(pos, -1.0, 1.0);
    let gy = s(pos, -1.0, 1.0) + 2.0 * s(pos, 0.0, 1.0) + s(pos, 1.0, 1.0)
        - s(pos, -1.0, -1.0) - 2.0 * s(pos, 0.0, -1.0) - s(pos, 1.0, -1.0);
    let mag = clamp(sqrt(gx * gx + gy * gy) * 0.5 * u.p[1].x, vec4<f32>(0.0), vec4<f32>(1.0));
    let mode = i32(u.p[0].x + 0.5);
    var rgb = vec3<f32>(mag.w);
    if (mode == 1) {
        rgb = mag.rgb;
    } else if (mode == 2) {
        rgb = vec3<f32>(1.0 - mag.w);
    }
    let out = vec4<f32>(dec(rgb) * c.a, c.a);
    return mix(c, out, u.p[2].x);
}
