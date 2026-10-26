// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// Old film, as `film` on the CPU. p[0] amount, p[1] weave, p[2] dust,
// p[3] scratches, p[4] flicker, p[5] grain, p[6] tint, p[7] vignette,
// p[8] variation.
fn effect(pos: vec2<f32>) -> vec4<f32> {
    let amount = u.p[0].x;
    let orig = tex(pos);
    if (amount <= 0.0) {
        return orig;
    }
    let w = u.size.x;
    let h = u.size.y;
    let seed = seed_u() ^ (bitcast<u32>(i32(u.p[8].x)) * 0x9e3779b9u);
    let t = u.time;
    let frame = i32(floor(t * 24.0));
    let weave = u.p[1].x;
    let wo = vec2<f32>(noise1(t * 3.0, seed) * 0.002 * h * weave, noise1(t * 2.3 + 7.0, seed ^ 1u) * 0.004 * h * weave);
    let flicker = 1.0 + u.p[4].x * 0.12 * (hash1(frame, seed ^ 2u) - 0.5) * 2.0;
    let px = tex(pos + wo);
    var c = vec3<f32>(0.0);
    if (px.a > 1e-6) {
        c = px.rgb / px.a;
    }
    c = vec3<f32>(srgb_encode(max(c.r, 0.0)), srgb_encode(max(c.g, 0.0)), srgb_encode(max(c.b, 0.0)));
    let l = 0.299 * c.r + 0.587 * c.g + 0.114 * c.b;
    let aged = vec3<f32>(l * 1.07 + 0.03, l * 0.97 + 0.02, l * 0.82);
    c = c + (aged - c) * u.p[6].x * 0.7;
    let ip = vec2<i32>(pos - vec2<f32>(0.5));
    let g = (hash3(ip.x, ip.y, frame, seed ^ 8u) - 0.5) * u.p[5].x * 0.16;
    let uv = pos / u.size - vec2<f32>(0.5);
    let vig = 1.0 - u.p[7].x * sstep(0.2, 0.75, length(uv) * 1.2);
    c = (c * flicker + vec3<f32>(g)) * vig;
    let specks = i32(u.p[2].x * 40.0);
    for (var i = 0; i < specks; i = i + 1) {
        let sx = hash2(i, frame * 7, seed ^ 3u) * w;
        let sy = hash2(i, frame * 7 + 1, seed ^ 3u) * h;
        let r2 = hash2(i, frame * 7 + 2, seed ^ 3u);
        let r = (0.0008 + 0.003 * r2 * r2) * h;
        let dark = hash2(i, frame * 7 + 3, seed ^ 3u) < 0.75;
        let d = length(pos - vec2<f32>(sx, sy));
        let k = sstep(r, r * 0.4, d);
        if (k > 0.0) {
            let tone = select(0.95, 0.05, dark);
            c = c + (vec3<f32>(tone) - c) * k * 0.85;
        }
    }
    for (var i = 0; i < 3; i = i + 1) {
        let alive = hash1(frame / 6 + i * 31, seed ^ 4u) < u.p[3].x * 0.8;
        if (alive) {
            let base = hash1(i, seed ^ 5u) + noise1(t * 0.4 + f32(i) * 9.0, seed ^ 6u) * 0.05;
            let sx = (base - floor(base)) * w;
            let bright = 0.4 + 0.6 * hash1(frame + i, seed ^ 7u);
            let k = sstep(0.0015 * h, 0.0, abs(pos.x - sx)) * bright;
            c = c + (vec3<f32>(0.9) - c) * k * 0.6;
        }
    }
    c = clamp(c, vec3<f32>(0.0), vec3<f32>(1.0));
    let out = vec4<f32>(vec3<f32>(srgb_decode(c.r), srgb_decode(c.g), srgb_decode(c.b)) * px.a, px.a);
    return mix(orig, out, min(amount, 1.0));
}
