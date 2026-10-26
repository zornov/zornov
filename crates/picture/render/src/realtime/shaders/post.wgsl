// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// Bloom, temporal accumulation, tone mapping and anti-aliasing. One
// resource layout for all: params, two textures, a sampler.
//
// Methods: the bloom's down- and up-sampling follows the dual-filter blur
// (Marius Bjorge, SIGGRAPH 2015) with Karis-weighted first taps (Brian
// Karis, 2014); the tone curve is Krzysztof Narkowicz's fit of the ACES
// filmic curve; the edge smoothing is the FXAA 3.11 idea of Timothy Lottes
// (NVIDIA). Written here from the published descriptions.

struct Params {
    a: vec4<f32>,
    b: vec4<f32>,
}

@group(0) @binding(0) var<uniform> pr: Params;
@group(0) @binding(1) var tex_a: texture_2d<f32>;
@group(0) @binding(2) var tex_b: texture_2d<f32>;
@group(0) @binding(3) var samp: sampler;

struct VOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> VOut {
    var o: VOut;
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    o.clip = vec4<f32>(p * 2.0 - vec2<f32>(1.0), 0.0, 1.0);
    o.uv = vec2<f32>(p.x, 1.0 - p.y);
    return o;
}

fn px(t: texture_2d<f32>) -> vec2<f32> {
    return vec2<f32>(1.0) / vec2<f32>(textureDimensions(t));
}

fn karis(c: vec3<f32>) -> f32 {
    return 1.0 / (1.0 + dot(c, vec3<f32>(0.2126, 0.7152, 0.0722)));
}

// Brightness above the knee, softly, from the first downsample.
@fragment
fn fs_prefilter(i: VOut) -> @location(0) vec4<f32> {
    let o = px(tex_a);
    var s = vec3<f32>(0.0);
    var ws = 0.0;
    for (var y = -1; y <= 1; y += 2) {
        for (var x = -1; x <= 1; x += 2) {
            let c = min(textureSampleLevel(tex_a, samp, i.uv + vec2<f32>(f32(x), f32(y)) * o, 0.0).rgb * pr.b.x, vec3<f32>(60000.0));
            let w = karis(c);
            s += c * w;
            ws += w;
        }
    }
    let c = s / ws;
    // pr.a.x: threshold in exposed units, pr.a.y: knee.
    let br = max(c.r, max(c.g, c.b));
    let soft = clamp(br - pr.a.x + pr.a.y, 0.0, 2.0 * pr.a.y);
    let contrib = max(soft * soft / (4.0 * pr.a.y + 1e-4), br - pr.a.x) / max(br, 1e-4);
    return vec4<f32>(c * max(contrib, 0.0), 1.0);
}

@fragment
fn fs_down(i: VOut) -> @location(0) vec4<f32> {
    let o = px(tex_a);
    var s = textureSampleLevel(tex_a, samp, i.uv, 0.0).rgb * 4.0;
    s += textureSampleLevel(tex_a, samp, i.uv + vec2<f32>(-o.x, -o.y), 0.0).rgb;
    s += textureSampleLevel(tex_a, samp, i.uv + vec2<f32>(o.x, -o.y), 0.0).rgb;
    s += textureSampleLevel(tex_a, samp, i.uv + vec2<f32>(-o.x, o.y), 0.0).rgb;
    s += textureSampleLevel(tex_a, samp, i.uv + vec2<f32>(o.x, o.y), 0.0).rgb;
    return vec4<f32>(s / 8.0, 1.0);
}

// tex_a is the smaller level, tex_b the larger one it is added to.
@fragment
fn fs_up(i: VOut) -> @location(0) vec4<f32> {
    let o = px(tex_a);
    var s = vec3<f32>(0.0);
    s += textureSampleLevel(tex_a, samp, i.uv + vec2<f32>(-o.x * 2.0, 0.0), 0.0).rgb;
    s += textureSampleLevel(tex_a, samp, i.uv + vec2<f32>(-o.x, o.y), 0.0).rgb * 2.0;
    s += textureSampleLevel(tex_a, samp, i.uv + vec2<f32>(0.0, o.y * 2.0), 0.0).rgb;
    s += textureSampleLevel(tex_a, samp, i.uv + vec2<f32>(o.x, o.y), 0.0).rgb * 2.0;
    s += textureSampleLevel(tex_a, samp, i.uv + vec2<f32>(o.x * 2.0, 0.0), 0.0).rgb;
    s += textureSampleLevel(tex_a, samp, i.uv + vec2<f32>(o.x, -o.y), 0.0).rgb * 2.0;
    s += textureSampleLevel(tex_a, samp, i.uv + vec2<f32>(0.0, -o.y * 2.0), 0.0).rgb;
    s += textureSampleLevel(tex_a, samp, i.uv + vec2<f32>(-o.x, -o.y), 0.0).rgb * 2.0;
    let up = s / 12.0;
    let base = textureSampleLevel(tex_b, samp, i.uv, 0.0).rgb;
    return vec4<f32>(base + up * pr.a.x, 1.0);
}

// Running average of jittered frames: tex_a is this frame, tex_b the history.
@fragment
fn fs_accum(i: VOut) -> @location(0) vec4<f32> {
    let cur = max(min(textureSampleLevel(tex_a, samp, i.uv, 0.0), vec4<f32>(60000.0)), vec4<f32>(0.0));
    let hist = textureSampleLevel(tex_b, samp, i.uv, 0.0);
    if (pr.a.x >= 0.999) {
        return cur;
    }
    return mix(hist, cur, pr.a.x);
}

fn aces(x: vec3<f32>) -> vec3<f32> {
    let a = 2.51;
    let b = 0.03;
    let c = 2.43;
    let d = 0.59;
    let e = 0.14;
    return clamp((x * (a * x + b)) / (x * (c * x + d) + e), vec3<f32>(0.0), vec3<f32>(1.0));
}

// Scene colour (tex_a) plus bloom (tex_b) to display-linear light.
@fragment
fn fs_tonemap(i: VOut) -> @location(0) vec4<f32> {
    var c = textureSampleLevel(tex_a, samp, i.uv, 0.0).rgb;
    let bl = textureSampleLevel(tex_b, samp, i.uv, 0.0).rgb;
    c = c * pr.a.x + bl * pr.a.y;
    // pr.a.z saturation, pr.a.w vignette
    var t = aces(c);
    let l = dot(t, vec3<f32>(0.2126, 0.7152, 0.0722));
    t = mix(vec3<f32>(l), t, pr.a.z);
    let q = i.uv - vec2<f32>(0.5);
    let vig = 1.0 - pr.a.w * smoothstep(0.25, 0.85, length(q * vec2<f32>(1.0, 0.8)) * 1.4);
    return vec4<f32>(t * vig, 1.0);
}

fn to_srgb(c: vec3<f32>) -> vec3<f32> {
    let x = clamp(c, vec3<f32>(0.0), vec3<f32>(1.0));
    let lo = x * 12.92;
    let hi = 1.055 * pow(x, vec3<f32>(1.0 / 2.4)) - vec3<f32>(0.055);
    return select(hi, lo, x <= vec3<f32>(0.0031308));
}

fn tap(uv: vec2<f32>) -> vec3<f32> {
    return to_srgb(textureSampleLevel(tex_a, samp, uv, 0.0).rgb);
}

// Display-linear (tex_a) to 8-bit sRGB, smoothing jagged edges when pr.a.x is 1.
@fragment
fn fs_encode(i: VOut) -> @location(0) vec4<f32> {
    let o = px(tex_a);
    let m = tap(i.uv);
    if (pr.a.x < 0.5) {
        return vec4<f32>(m, 1.0);
    }
    let nw = tap(i.uv + vec2<f32>(-o.x, -o.y));
    let ne = tap(i.uv + vec2<f32>(o.x, -o.y));
    let sw = tap(i.uv + vec2<f32>(-o.x, o.y));
    let se = tap(i.uv + vec2<f32>(o.x, o.y));
    let lw = vec3<f32>(0.299, 0.587, 0.114);
    let l_nw = dot(nw, lw);
    let l_ne = dot(ne, lw);
    let l_sw = dot(sw, lw);
    let l_se = dot(se, lw);
    let l_m = dot(m, lw);
    let l_min = min(l_m, min(min(l_nw, l_ne), min(l_sw, l_se)));
    let l_max = max(l_m, max(max(l_nw, l_ne), max(l_sw, l_se)));
    var dir = vec2<f32>(-((l_nw + l_ne) - (l_sw + l_se)), (l_nw + l_sw) - (l_ne + l_se));
    let reduce = max((l_nw + l_ne + l_sw + l_se) * 0.25 * 0.125, 1.0 / 128.0);
    let rcp = 1.0 / (min(abs(dir.x), abs(dir.y)) + reduce);
    dir = clamp(dir * rcp, vec2<f32>(-8.0), vec2<f32>(8.0)) * o;
    let a = 0.5 * (tap(i.uv + dir * (1.0 / 3.0 - 0.5)) + tap(i.uv + dir * (2.0 / 3.0 - 0.5)));
    let b = a * 0.5 + 0.25 * (tap(i.uv + dir * -0.5) + tap(i.uv + dir * 0.5));
    let l_b = dot(b, lw);
    var out = b;
    if (l_b < l_min || l_b > l_max) { out = a; }
    return vec4<f32>(out, 1.0);
}
