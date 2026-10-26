// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// p[0].x amplitude, p[1].x wavelength (% height), p[2].x direction,
// p[3].x speed (Hz), p[4].x phase (degrees)
fn effect(pos: vec2<f32>) -> vec4<f32> {
    let amp = pct_h(u.p[0].x);
    let len = max(pct_h(u.p[1].x), 1.0);
    let a = radians(u.p[2].x);
    let s = sin(a);
    let c = cos(a);
    let phase = radians(u.p[4].x) - TAU * u.p[3].x * u.time;
    let d = amp * sin(TAU * dot(pos, vec2<f32>(c, s)) / len + phase);
    return tex(pos + vec2<f32>(-s * d, c * d));
}
