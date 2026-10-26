// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// Motion blur along the motion channel. p: shutter, samples, offset
// (0 centred, 1 trailing, 2 leading).
fn node(pos: vec2<f32>) -> vec4<f32> {
    let v = aux_a(cur).yz;
    let shutter = u.p[0].x;
    if ((u32(u.has_aux) & 8u) == 0u || abs(v.x) + abs(v.y) < 1e-3 || shutter <= 0.0) {
        return load_a(cur);
    }
    let n = max(i32(u.p[1].x + 0.5), 2);
    var a = -0.5;
    var b = 0.5;
    let off = i32(u.p[2].x + 0.5);
    if (off == 1) {
        a = -1.0;
        b = 0.0;
    } else if (off == 2) {
        a = 0.0;
        b = 1.0;
    }
    var acc = vec4<f32>(0.0);
    for (var k = 0; k < n; k = k + 1) {
        let t = a + (b - a) * (f32(k) / f32(n - 1));
        acc = acc + sample_a(pos + v * shutter * t);
    }
    return acc / f32(n);
}
