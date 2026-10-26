// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// p[0].x strength, p[1].x ripple (% height), p[2].x ripple size (% height),
// p[3].x flow direction (degrees), p[4].x flow speed (% height / s),
// p[5].x glints, p[6].x variation. The CPU's `water` is the same sum.

fn vnoise(x: f32, y: f32, seed: u32) -> f32 {
    let xf = floor(x);
    let yf = floor(y);
    let xi = i32(xf);
    let yi = i32(yf);
    let tx = fade(x - xf);
    let ty = fade(y - yf);
    let a = hash2(xi, yi, seed);
    let b = hash2(xi + 1, yi, seed);
    let c = hash2(xi, yi + 1, seed);
    let d = hash2(xi + 1, yi + 1, seed);
    return mix(mix(a, b, tx), mix(c, d, tx), ty);
}

fn effect(pos: vec2<f32>) -> vec4<f32> {
    let amt = pct_h(u.p[1].x) * u.p[0].x;
    let shimmer = u.p[5].x * u.p[0].x;
    if (amt <= 0.0 && shimmer <= 0.0) {
        return tex(pos);
    }
    let scale = max(pct_h(u.p[2].x), 1.0);
    let a = radians(u.p[3].x);
    let s = sin(a);
    let c = cos(a);
    let v = pct_h(u.p[4].x) * u.time;
    let seed = seed_u() ^ (u32(i32(round(u.p[6].x))) * 0x9e3779b9u);
    let u1 = (pos.x - c * v) / scale;
    let v1 = (pos.y - s * v) / scale * 2.0;
    let u2 = (pos.x - c * v * 0.6 + s * v * 0.25) / (scale * 0.7);
    let v2 = (pos.y - s * v * 0.6 - c * v * 0.25) / (scale * 0.7) * 2.0;
    let dx = amt * (vnoise(u1, v1, seed) + vnoise(u2, v2, seed + 1u) - 1.0);
    let dy = amt * 0.5 * (vnoise(u1 + 5.2, v1 + 1.3, seed + 2u) + vnoise(u2 + 3.1, v2 + 7.7, seed + 3u) - 1.0);
    let src = tex(pos + vec2<f32>(dx, dy));
    let n = vnoise((pos.x - c * v * 1.3) / (scale * 0.25), (pos.y - s * v * 1.3) / (scale * 0.12), seed + 4u);
    let g = smoothstep(0.78, 0.97, n) * shimmer * (0.25 + max(luma(src), 0.0)) * src.a;
    return vec4<f32>(src.rgb + vec3<f32>(g), src.a);
}
