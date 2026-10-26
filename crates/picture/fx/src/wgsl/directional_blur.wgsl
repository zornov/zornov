// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// p[0].x length (% height), p[1].x angle (degrees)
fn effect(pos: vec2<f32>) -> vec4<f32> {
    let len = pct_h(u.p[0].x);
    if (len < 0.5) {
        return tex(pos);
    }
    let a = radians(u.p[1].x);
    let d = vec2<f32>(cos(a), sin(a)) * len;
    let taps = i32(clamp(ceil(len), 1.0, 64.0));
    var acc = vec4<f32>(0.0);
    for (var i = 0; i < taps; i = i + 1) {
        let t = (f32(i) + 0.5) / f32(taps) - 0.5;
        acc = acc + tex(pos + d * t);
    }
    return acc / f32(taps);
}
