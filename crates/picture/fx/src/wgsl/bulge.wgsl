// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// p[0].x amount, p[1].x radius (% height), p[2].xy centre
fn effect(pos: vec2<f32>) -> vec4<f32> {
    let radius = max(pct_h(u.p[1].x), 0.001);
    let c = u.p[2].xy * u.size;
    let d = pos - c;
    let r = length(d);
    if (r >= radius || r < 0.000001) {
        return tex(pos);
    }
    let t = r / radius;
    let k = pow(t, exp2(u.p[0].x)) / t;
    return tex(c + d * k);
}
