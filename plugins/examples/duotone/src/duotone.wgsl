// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// Duotone. Studio puts its effect prelude (bindings, `u`, `tex`, `luma`)
// in front of this. p[0] shadows, p[1] highlights (premultiplied colours),
// p[2].x contrast, p[3].x mix.
fn effect(pos: vec2<f32>) -> vec4<f32> {
    let c = tex(pos);
    if (c.a <= 0.000001) {
        return c;
    }
    let l = clamp(luma(c) / c.a, 0.0, 1.0);
    let k = u.p[2].x;
    let t = clamp((l - 0.5) * k + 0.5, 0.0, 1.0);
    let s = t * t * (3.0 - 2.0 * t);
    let tone = mix(u.p[0], u.p[1], s);
    let out = vec4<f32>(tone.rgb * c.a, c.a);
    return mix(c, out, u.p[3].x);
}
