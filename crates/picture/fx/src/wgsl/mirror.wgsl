// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// p[0].x mode, p[1].x position, p[2].x mix
fn effect(pos: vec2<f32>) -> vec4<f32> {
    let mode = i32(u.p[0].x + 0.5);
    let m = u.p[1].x * u.size;
    var q = pos;
    if (mode == 0 || mode == 4) {
        if (pos.x > m.x) { q.x = 2.0 * m.x - pos.x; }
    }
    if (mode == 1) {
        if (pos.x < m.x) { q.x = 2.0 * m.x - pos.x; }
    }
    if (mode == 2 || mode == 4) {
        if (pos.y > m.y) { q.y = 2.0 * m.y - pos.y; }
    }
    if (mode == 3) {
        if (pos.y < m.y) { q.y = 2.0 * m.y - pos.y; }
    }
    return mix(tex(pos), tex(q), u.p[2].x);
}
