// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// Iris: the incoming picture opens in a circle. Studio puts its transition
// prelude (bindings, `u`, `tex_a`, `tex_b`) in front of this.
// p[0].xy centre (0..1), p[1].x softness (% of height).
fn transition(pos: vec2<f32>, t: f32) -> vec4<f32> {
    let c = u.p[0].xy * u.size;
    let corner = max(max(length(c), length(c - vec2<f32>(u.size.x, 0.0))), max(length(c - vec2<f32>(0.0, u.size.y)), length(c - u.size)));
    let soft = max(u.p[1].x * 0.01 * u.size.y, 0.5);
    let r = t * (corner + soft);
    let k = clamp((r - length(pos - c)) / soft, 0.0, 1.0);
    return mix(tex_a(pos), tex_b(pos), k);
}
