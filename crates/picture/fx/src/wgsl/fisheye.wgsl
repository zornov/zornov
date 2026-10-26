// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// p[0].x amount, p[1].x zoom
fn effect(pos: vec2<f32>) -> vec4<f32> {
    let amount = u.p[0].x;
    let zoom = u.p[1].x;
    let half_size = u.size * 0.5;
    let half_diag = length(half_size);
    let d = (pos - half_size) / half_diag;
    let r = length(d);
    let theta = abs(amount) * 1.2;
    var k = 1.0 / zoom;
    if (r > 0.000001 && theta > 0.0001) {
        var rs: f32;
        if (amount > 0.0) {
            rs = tan(r * theta) / tan(theta);
        } else {
            rs = atan(r * tan(theta)) / theta;
        }
        k = rs / r / zoom;
    }
    return tex(half_size + d * k * half_diag);
}
