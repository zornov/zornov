// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// Plasma generator. Studio puts its effect prelude in front of this.
// p[0].x scale, p[1].x speed, p[2] colour, p[3].x mix (opacity).
fn effect(pos: vec2<f32>) -> vec4<f32> {
    let uv = pos / u.size.y * u.p[0].x;
    let t = u.time * u.p[1].x;
    let v = sin(uv.x * 6.0 + t) + sin((uv.y * 5.0 - t) * 1.3) + sin((uv.x + uv.y) * 4.0 + t * 0.7) + sin(length(uv - vec2<f32>(0.5)) * 9.0 - t * 1.6);
    let w = 0.5 + 0.5 * sin(v * 1.5707963);
    let col = vec4<f32>(u.p[2].rgb * (0.25 + 0.75 * w) + vec3<f32>(0.08) * (1.0 - w), u.p[2].a);
    let src = tex(pos);
    let over = col + src * (1.0 - col.a);
    return mix(src, over, u.p[3].x);
}
