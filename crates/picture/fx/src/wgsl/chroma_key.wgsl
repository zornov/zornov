// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// p[0] key colour, p[1].x tolerance, p[2].x softness, p[3].x spill,
// p[4].x feather, p[5].x choke, p[6].x show matte, p[7].x amount.
// The GPU path skips choke and feather (they need neighbourhood passes);
// the renderer runs those as separate blur / min-max passes.
fn enc(v: vec3<f32>) -> vec3<f32> {
    let c = clamp(v, vec3<f32>(0.0), vec3<f32>(1.0));
    let lo = c * 12.92;
    let hi = 1.055 * pow(c, vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(hi, lo, c <= vec3<f32>(0.0031308));
}

fn chroma(c: vec3<f32>) -> vec2<f32> {
    let e = enc(c);
    let y = dot(e, vec3<f32>(0.2126, 0.7152, 0.0722));
    return vec2<f32>((e.b - y) / 1.8556, (e.r - y) / 1.5748);
}

fn luma_enc(c: vec3<f32>) -> f32 {
    return dot(enc(c), vec3<f32>(0.2126, 0.7152, 0.0722));
}

// Matches keycolour::neutrality.
fn neutrality(kc: vec2<f32>) -> f32 {
    return 1.0 - smoothstep(0.03, 0.12, length(kc));
}

// Matches keycolour::distance: a saturated key by chroma, a white, black
// or grey key by chroma and luma.
fn key_distance(c: vec3<f32>, key: vec3<f32>) -> f32 {
    let kc = chroma(key);
    let ks = length(kc);
    let w = neutrality(kc);
    let k2 = max(ks, 0.02);
    let norm = k2 + (0.35 - k2) * w;
    let dc = length(chroma(c) - kc) / norm;
    let dy = abs(luma_enc(c) - luma_enc(key)) * w;
    return sqrt(dc * dc + dy * dy);
}

fn effect(pos: vec2<f32>) -> vec4<f32> {
    let px = tex(pos);
    let key = u.p[0].rgb / max(u.p[0].a, 0.000001);
    var c = vec3<f32>(0.0);
    if (px.a > 0.000001) {
        c = px.rgb / px.a;
    }
    let neutral = neutrality(chroma(key));
    let d = key_distance(c, key);
    let m = smoothstep(u.p[1].x, u.p[1].x + u.p[2].x, d);
    var dom = 1;
    if (key.r >= key.g && key.r >= key.b) {
        dom = 0;
    } else if (key.b > key.g) {
        dom = 2;
    }
    let limit = 0.5 * (c[(dom + 1) % 3] + c[(dom + 2) % 3]);
    c[dom] = c[dom] - u.p[3].x * (1.0 - neutral) * max(c[dom] - limit, 0.0);
    // A white, black or grey backdrop: take it back out of soft edges.
    if (neutral > 0.0 && m >= 0.04 && m < 0.999) {
        let unmixed = max((c - (1.0 - m) * key) / m, vec3<f32>(0.0));
        c = mix(c, unmixed, neutral * clamp(u.p[3].x / 0.6, 0.0, 1.0));
    }
    var out = vec4<f32>(c * px.a * m, px.a * m);
    if (u.p[6].x > 0.5) {
        out = vec4<f32>(vec3<f32>(m * px.a), 1.0);
    }
    return mix(px, out, u.p[7].x);
}
