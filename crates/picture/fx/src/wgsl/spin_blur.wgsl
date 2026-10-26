// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// p[0].x angle (degrees), p[1].xy centre (normalised)
fn effect(pos: vec2<f32>) -> vec4<f32> {
    let angle = radians(u.p[0].x);
    if (angle <= 0.0) {
        return tex(pos);
    }
    let c = u.p[1].xy * u.size;
    let v = pos - c;
    let taps = i32(clamp(ceil(length(v) * angle), 1.0, 64.0));
    var acc = vec4<f32>(0.0);
    for (var i = 0; i < taps; i = i + 1) {
        let t = ((f32(i) + 0.5) / f32(taps) - 0.5) * angle;
        let r = vec2<f32>(v.x * cos(t) - v.y * sin(t), v.x * sin(t) + v.y * cos(t));
        acc = acc + tex(c + r);
    }
    return acc / f32(taps);
}
