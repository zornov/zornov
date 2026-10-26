// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// Camera shake, as `camera_shake` on the CPU. p[0] amount (% height),
// p[1] frequency, p[2] rotation (degrees), p[3] roughness, p[4] hide
// edges, p[5] variation.
fn shake_n(t: f32, freq: f32, rough: f32, seed: u32, k: u32) -> f32 {
    let s = seed + k * 0x632be5abu;
    return (noise1(t * freq, s) + rough * 0.5 * noise1(t * freq * 2.7 + 13.0, s ^ 0xffffu)) / (1.0 + rough * 0.5);
}

fn effect(pos: vec2<f32>) -> vec4<f32> {
    let amount = pct_h(u.p[0].x);
    if (amount <= 0.0) {
        return tex(pos);
    }
    let rot = radians(u.p[2].x);
    let seed = seed_u() ^ (bitcast<u32>(i32(u.p[5].x)) * 0x85ebca6bu);
    let freq = u.p[1].x;
    let rough = u.p[3].x;
    let ox = shake_n(u.time, freq, rough, seed, 1u) * amount;
    let oy = shake_n(u.time, freq, rough, seed, 2u) * amount;
    let a = shake_n(u.time, freq, rough, seed, 3u) * rot;
    let w = u.size.x;
    let h = u.size.y;
    var zoom = 1.0;
    if (u.p[4].x > 0.5) {
        let ratio = max(w / h, h / w);
        zoom = cos(rot) + sin(rot) * ratio + 2.0 * amount / min(w, h);
    }
    let s = sin(-a);
    let c = cos(-a);
    let dx = (pos.x - w * 0.5 - ox) / zoom;
    let dy = (pos.y - h * 0.5 - oy) / zoom;
    return sample_mirror(vec2<f32>(w * 0.5 + dx * c - dy * s, h * 0.5 + dx * s + dy * c));
}
