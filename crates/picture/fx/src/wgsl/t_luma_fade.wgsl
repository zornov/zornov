// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// p[0].x map, p[1].x softness, p[2].x angle, p[3].x noise scale (% height),
// p[4].x invert, p[5].x easing. The noise map here is a simpler single-
// octave-pair value noise than the CPU version, so patterns differ.
fn hash2(x: i32, y: i32) -> f32 {
    var h = (u32(x) * 0x8da6b343u) ^ (u32(y) * 0xd8163841u);
    h = h ^ (h >> 16u);
    h = h * 0x7feb352du;
    h = h ^ (h >> 15u);
    return f32(h >> 8u) / 16777216.0;
}

fn vnoise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = p - i;
    let s = f * f * (3.0 - 2.0 * f);
    let a = hash2(i32(i.x), i32(i.y));
    let b = hash2(i32(i.x) + 1, i32(i.y));
    let c = hash2(i32(i.x), i32(i.y) + 1);
    let d = hash2(i32(i.x) + 1, i32(i.y) + 1);
    return mix(mix(a, b, s.x), mix(c, d, s.x), s.y);
}

fn enc(v: f32) -> f32 {
    let c = clamp(v, 0.0, 1.0);
    if (c <= 0.0031308) {
        return c * 12.92;
    }
    return 1.055 * pow(c, 1.0 / 2.4) - 0.055;
}

fn bright(c: vec4<f32>) -> f32 {
    if (c.a <= 0.000001) {
        return 0.0;
    }
    return enc(dot(c.rgb / c.a, vec3<f32>(0.2126, 0.7152, 0.0722)));
}

fn transition(pos: vec2<f32>, t: f32) -> vec4<f32> {
    let a = tex_a(pos);
    let b = tex_b(pos);
    let kind = i32(u.p[0].x + 0.5);
    var m: f32;
    if (kind == 0) {
        let an = radians(u.p[2].x);
        let d = vec2<f32>(cos(an), sin(an));
        let c1 = u.size.x * d.x;
        let c2 = u.size.y * d.y;
        let c3 = dot(u.size, d);
        let lo = min(min(0.0, c1), min(c2, c3));
        let hi = max(max(0.0, c1), max(c2, c3));
        m = (dot(pos, d) - lo) / max(hi - lo, 0.000001);
    } else if (kind == 1) {
        m = length(pos - u.size * 0.5) / length(u.size * 0.5);
    } else if (kind == 2) {
        let q = pos / max(u.size.y * u.p[3].x * 0.01, 1.0);
        let n = vnoise(q) * 0.65 + vnoise(q * 2.03 + 17.0) * 0.35;
        m = clamp((n - 0.5) * 2.2 + 0.5, 0.0, 1.0);
    } else if (kind == 3) {
        m = 1.0 - bright(b);
    } else {
        m = 1.0 - bright(a);
    }
    if (u.p[4].x > 0.5) {
        m = 1.0 - m;
    }
    return mix(a, b, reveal_mask(m, ease(u.p[5].x, t), u.p[1].x));
}
