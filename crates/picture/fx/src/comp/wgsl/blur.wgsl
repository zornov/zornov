// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// One pass of a separable gaussian blur. p[0].x sigma, p[1].xy
// direction, p[2].x channels (0 all, 1 colour, 2 alpha), p[3].x 1 on the
// last pass. The second input is the unblurred picture, for the
// colour-only and alpha-only modes.
fn node(pos: vec2<f32>) -> vec4<f32> {
    let sigma = u.p[0].x;
    var acc = vec4<f32>(0.0);
    if (sigma <= 0.25) {
        acc = load_a(cur);
    } else {
        let r = i32(ceil(sigma * 3.0));
        var wsum = 0.0;
        let dir = vec2<i32>(u.p[1].xy);
        for (var i = -r; i <= r; i = i + 1) {
            let w = exp(-f32(i * i) / (2.0 * sigma * sigma));
            acc = acc + load_a(cur + dir * i) * w;
            wsum = wsum + w;
        }
        acc = acc / wsum;
    }
    if (u.p[3].x < 0.5 || u.has_b < 0.5) {
        return acc;
    }
    let orig = load_b(cur);
    let ch = i32(u.p[2].x + 0.5);
    if (ch == 1) {
        return vec4<f32>(acc.rgb, orig.a);
    }
    if (ch == 2) {
        var k = 0.0;
        if (orig.a > 1e-6) {
            k = min(acc.a / orig.a, 1.0);
        }
        return vec4<f32>(orig.rgb * k, acc.a);
    }
    return acc;
}
