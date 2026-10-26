// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// p[0].x kind, p[1] colour a, p[2] colour b (premultiplied), p[3].xy start,
// p[4].xy end, p[5].x perceptual, p[6].x mix. The GPU path blends
// straight colours (perceptual blend in sRGB encoding when asked).
fn enc(v: vec3<f32>) -> vec3<f32> {
    let c = clamp(v, vec3<f32>(0.0), vec3<f32>(1.0));
    let lo = c * 12.92;
    let hi = 1.055 * pow(c, vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(hi, lo, c <= vec3<f32>(0.0031308));
}

fn dec(v: vec3<f32>) -> vec3<f32> {
    let lo = v / 12.92;
    let hi = pow((max(v, vec3<f32>(0.0)) + 0.055) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, v <= vec3<f32>(0.04045));
}

fn straight(c: vec4<f32>) -> vec3<f32> {
    return c.rgb / max(c.a, 0.000001);
}

fn effect(pos: vec2<f32>) -> vec4<f32> {
    let s = u.p[3].xy * u.size;
    let e = u.p[4].xy * u.size;
    let d = e - s;
    let len2 = max(dot(d, d), 0.000001);
    let v = pos - s;
    let kind = i32(u.p[0].x + 0.5);
    var t: f32;
    if (kind == 0) {
        t = dot(v, d) / len2;
    } else if (kind == 1) {
        t = sqrt(dot(v, v) / len2);
    } else {
        let a = atan2(v.y, v.x) - atan2(d.y, d.x);
        t = (a - TAU * floor(a / TAU)) / TAU;
    }
    t = clamp(t, 0.0, 1.0);
    let ca = straight(u.p[1]);
    let cb = straight(u.p[2]);
    var rgb = mix(ca, cb, t);
    if (u.p[5].x > 0.5) {
        rgb = dec(mix(enc(ca), enc(cb), t));
    }
    let a = mix(u.p[1].a, u.p[2].a, t);
    let g = vec4<f32>(rgb * a, a) * u.p[6].x;
    return g + tex(pos) * (1.0 - g.a);
}
