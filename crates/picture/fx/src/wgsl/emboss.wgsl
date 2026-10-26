// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// p[0].x light angle, p[1].x depth, p[2].x keep colour, p[3].x mix
fn enc(v: f32) -> f32 {
    if (v <= 0.0031308) {
        return v * 12.92;
    }
    return 1.055 * pow(v, 1.0 / 2.4) - 0.055;
}

fn dec(v: f32) -> f32 {
    if (v <= 0.04045) {
        return v / 12.92;
    }
    return pow((v + 0.055) / 1.055, 2.4);
}

fn l(pos: vec2<f32>, dx: f32, dy: f32) -> f32 {
    return enc(max(luma(tex(pos + vec2<f32>(dx, dy))), 0.0));
}

fn effect(pos: vec2<f32>) -> vec4<f32> {
    let c = tex(pos);
    let gx = l(pos, 1.0, -1.0) + 2.0 * l(pos, 1.0, 0.0) + l(pos, 1.0, 1.0)
        - l(pos, -1.0, -1.0) - 2.0 * l(pos, -1.0, 0.0) - l(pos, -1.0, 1.0);
    let gy = l(pos, -1.0, 1.0) + 2.0 * l(pos, 0.0, 1.0) + l(pos, 1.0, 1.0)
        - l(pos, -1.0, -1.0) - 2.0 * l(pos, 0.0, -1.0) - l(pos, 1.0, -1.0);
    let a = radians(u.p[0].x);
    let shade = clamp(0.5 + (gx * cos(a) - gy * sin(a)) * u.p[1].x * 0.125, 0.0, 1.0);
    var out: vec4<f32>;
    if (u.p[2].x > 0.5) {
        let k = dec(shade) / dec(0.5);
        out = vec4<f32>(c.rgb * k, c.a);
    } else {
        out = vec4<f32>(vec3<f32>(dec(shade) * c.a), c.a);
    }
    return mix(c, out, u.p[3].x);
}
