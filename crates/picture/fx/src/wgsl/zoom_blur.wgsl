// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// p[0].x amount (0..1), p[1].xy centre (normalised)
fn effect(pos: vec2<f32>) -> vec4<f32> {
    let amount = u.p[0].x;
    if (amount <= 0.0) {
        return tex(pos);
    }
    let c = u.p[1].xy * u.size;
    let v = pos - c;
    let taps = i32(clamp(ceil(length(v) * amount), 1.0, 64.0));
    var acc = vec4<f32>(0.0);
    for (var i = 0; i < taps; i = i + 1) {
        let k = 1.0 - amount * (f32(i) + 0.5) / f32(taps);
        acc = acc + tex(c + v * k);
    }
    return acc / f32(taps);
}
