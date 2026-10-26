// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// p[0].x distance (% height), p[1].x angle
fn effect(pos: vec2<f32>) -> vec4<f32> {
    let a = radians(u.p[1].x);
    let d = vec2<f32>(cos(a), sin(a)) * pct_h(u.p[0].x) * 0.5;
    let r = tex(pos - d);
    let g = tex(pos);
    let b = tex(pos + d);
    return vec4<f32>(r.r, g.g, b.b, max(max(r.a, g.a), b.a));
}
