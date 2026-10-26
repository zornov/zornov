// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// p[0] colour (premultiplied), p[1].x mix
fn effect(pos: vec2<f32>) -> vec4<f32> {
    let under = tex(pos);
    let g = u.p[0] * u.p[1].x;
    return g + under * (1.0 - g.a);
}
