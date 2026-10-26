// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// p[0].x segments, p[1].x rotation, p[2].xy centre, p[3].x zoom, p[4].x mix
fn mirror_coord(v: f32, n: f32) -> f32 {
    let m = v - 2.0 * n * floor(v / (2.0 * n));
    return select(2.0 * n - m, m, m < n);
}

fn effect(pos: vec2<f32>) -> vec4<f32> {
    let n = max(u.p[0].x, 2.0);
    let seg = TAU / n;
    let rot = radians(u.p[1].x);
    let c = u.p[2].xy * u.size;
    let d = pos - c;
    let r = length(d) / u.p[3].x;
    var a = atan2(d.y, d.x) - rot;
    a = a - seg * floor(a / seg);
    if (a > seg * 0.5) {
        a = seg - a;
    }
    a = a + rot;
    let q = c + r * vec2<f32>(cos(a), sin(a));
    let m = vec2<f32>(mirror_coord(q.x, u.size.x), mirror_coord(q.y, u.size.y));
    return mix(tex(pos), tex(m), u.p[4].x);
}
