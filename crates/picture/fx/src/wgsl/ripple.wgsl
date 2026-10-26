// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// p[0].xy centre, p[1].x amplitude, p[2].x wavelength, p[3].x speed, p[4].x decay
fn effect(pos: vec2<f32>) -> vec4<f32> {
    let c = u.p[0].xy * u.size;
    let d = pos - c;
    let r = length(d);
    if (r < 0.001) {
        return tex(pos);
    }
    let len = max(pct_h(u.p[2].x), 1.0);
    let k = pct_h(u.p[1].x) * sin(TAU * r / len - TAU * u.p[3].x * u.time) * exp(-u.p[4].x * r / u.size.y);
    return tex(pos + d / r * k);
}
