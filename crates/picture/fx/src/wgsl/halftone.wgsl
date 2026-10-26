// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// p[0].x cell (% height), p[1].x angle, p[2].x mode (0 black, 1 colour),
// p[3] paper colour (premultiplied), p[4].x mix
fn to_srgb1(v: f32) -> f32 {
    if (v <= 0.0031308) {
        return v * 12.92;
    }
    return 1.055 * pow(v, 1.0 / 2.4) - 0.055;
}

// Returns (cell-local offset, cell centre in pixels).
fn cell_coords(p: vec2<f32>, cell: f32, angle: f32) -> vec4<f32> {
    let s = sin(angle);
    let c = cos(angle);
    let uv = vec2<f32>(p.x * c + p.y * s, -p.x * s + p.y * c) / cell;
    let cu = floor(uv) + 0.5;
    let centre = vec2<f32>(cu.x * c - cu.y * s, cu.x * s + cu.y * c) * cell;
    return vec4<f32>(uv - cu, centre);
}

fn dot_cov(ink: f32, f: vec2<f32>, aa: f32) -> f32 {
    let r = sqrt(clamp(ink, 0.0, 1.0) / 3.14159265);
    return smoothstep(r + aa, r - aa, length(f));
}

fn straight(c: vec4<f32>) -> vec3<f32> {
    if (c.a <= 0.000001) {
        return vec3<f32>(0.0);
    }
    return c.rgb / c.a;
}

fn effect(pos: vec2<f32>) -> vec4<f32> {
    let src_px = tex(pos);
    let cell = max(pct_h(u.p[0].x), 1.0);
    let angle = radians(u.p[1].x);
    let paper = u.p[3].rgb / max(u.p[3].a, 0.000001);
    let aa = 0.7 / cell;
    var rgb = paper;
    if (u.p[2].x > 0.5) {
        let offsets = vec3<f32>(15.0, 75.0, 0.0);
        for (var ch = 0; ch < 3; ch = ch + 1) {
            let cc = cell_coords(pos, cell, angle + radians(offsets[ch]));
            let s = straight(tex(cc.zw));
            let ink = 1.0 - to_srgb1(max(s[ch], 0.0));
            rgb[ch] = rgb[ch] * (1.0 - dot_cov(ink, cc.xy, aa) * 0.95);
        }
    } else {
        let cc = cell_coords(pos, cell, angle);
        let s = straight(tex(cc.zw));
        let l = to_srgb1(max(dot(s, vec3<f32>(0.2126, 0.7152, 0.0722)), 0.0));
        rgb = paper * (1.0 - dot_cov(1.0 - l, cc.xy, aa) * 0.97);
    }
    return mix(src_px, vec4<f32>(rgb * src_px.a, src_px.a), u.p[4].x);
}
