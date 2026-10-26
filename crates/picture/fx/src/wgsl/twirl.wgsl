// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// p[0].x angle, p[1].x radius (% height), p[2].xy centre
fn effect(pos: vec2<f32>) -> vec4<f32> {
    let radius = max(pct_h(u.p[1].x), 0.001);
    let c = u.p[2].xy * u.size;
    let d = pos - c;
    let r = length(d);
    if (r >= radius) {
        return tex(pos);
    }
    let k = 1.0 - r / radius;
    let a = radians(u.p[0].x) * k * k;
    return tex(c + vec2<f32>(d.x * cos(a) - d.y * sin(a), d.x * sin(a) + d.y * cos(a)));
}
