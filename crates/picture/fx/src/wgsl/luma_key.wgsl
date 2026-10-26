// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// p[0].x threshold, p[1].x softness, p[2].x mode (0 darks, 1 brights),
// p[3].x show matte, p[4].x amount
fn enc(v: vec3<f32>) -> vec3<f32> {
    let c = clamp(v, vec3<f32>(0.0), vec3<f32>(1.0));
    let lo = c * 12.92;
    let hi = 1.055 * pow(c, vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(hi, lo, c <= vec3<f32>(0.0031308));
}

fn effect(pos: vec2<f32>) -> vec4<f32> {
    let px = tex(pos);
    var c = vec3<f32>(0.0);
    if (px.a > 0.000001) {
        c = px.rgb / px.a;
    }
    let l = dot(enc(c), vec3<f32>(0.2126, 0.7152, 0.0722));
    let t = u.p[0].x;
    let s = u.p[1].x;
    var m = smoothstep(t, t + s, l);
    if (u.p[2].x > 0.5) {
        m = 1.0 - smoothstep(1.0 - t - s, 1.0 - t, l);
    }
    // The removed end is un-mixed from soft edges (no light or dark fringe).
    var key = vec3<f32>(0.0);
    if (u.p[2].x > 0.5) {
        key = vec3<f32>(1.0);
    }
    var fg = c;
    if (m >= 0.04 && m < 0.999) {
        fg = max((c - (1.0 - m) * key) / m, vec3<f32>(0.0));
    }
    var out = vec4<f32>(fg * px.a * m, px.a * m);
    if (u.p[3].x > 0.5) {
        out = vec4<f32>(vec3<f32>(m * px.a), 1.0);
    }
    return mix(px, out, u.p[4].x);
}
