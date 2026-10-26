// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// p[0].x direction, p[1].x shadow, p[2].x motion blur, p[3].x easing
fn transition(pos: vec2<f32>, t: f32) -> vec4<f32> {
    // Shutter motion blur: average 8 positions over the open shutter,
    // as on the CPU (see `with_motion_blur`).
    let span = u.p[2].x / 15.0;
    var acc = vec4<f32>(0.0);
    for (var i = 0; i < 8; i = i + 1) {
        let ti = clamp(t + span * (f32(i) / 7.0 - 0.5), 0.0, 1.0);
        acc = acc + slide_layers(pos, ease(u.p[3].x, ti), dir_vec(u.p[0].x), 0.0, true, true, u.p[1].x, 0.0);
    }
    return acc / 8.0;
}
