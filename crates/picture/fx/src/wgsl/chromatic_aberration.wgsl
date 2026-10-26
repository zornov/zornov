// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// p[0].x amount (% height), p[1].xy centre
fn effect(pos: vec2<f32>) -> vec4<f32> {
    let k = pct_h(u.p[0].x) / (0.5 * length(u.size));
    let c = u.p[1].xy * u.size;
    let d = pos - c;
    let r = tex(c + d * (1.0 - k));
    let g = tex(pos);
    let b = tex(c + d * (1.0 + k));
    return vec4<f32>(r.r, g.g, b.b, max(max(r.a, g.a), b.a));
}
