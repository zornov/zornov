// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// Shared by the real-time world renderer's shaders: the per-pass uniform,
// the sky/shadow/ambient-occlusion resources, physically based lighting
// with cascaded sun shadows and image-based ambient light, fog.

const PI: f32 = 3.14159265;

struct Light {
    pos_range: vec4<f32>,
    colour: vec4<f32>,
    dir_cone: vec4<f32>,
}

struct Frame {
    view_proj: mat4x4<f32>,
    view: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    cascade_vp: array<mat4x4<f32>, 4>,
    // View depth where each cascade ends.
    cascade_splits: vec4<f32>,
    // World size of one shadow texel in each cascade.
    cascade_texel: vec4<f32>,
    // xyz eye (the mirrored one in the reflection pass), w seconds.
    eye: vec4<f32>,
    // xyz the real camera's position.
    view_eye: vec4<f32>,
    // xyz towards the sun, w 1 when shadows are on.
    sun_dir: vec4<f32>,
    // rgb sun colour (diffuse = albedo * colour * n.l), w sine of elevation.
    sun_colour: vec4<f32>,
    // x density, y height falloff, z 1 under water.
    fog: vec4<f32>,
    // rgb override colour, w 1 when set.
    fog_colour: vec4<f32>,
    // x cloud cover, y env mip count, z exposure, w bloom amount.
    sky_misc: vec4<f32>,
    sh: array<vec4<f32>, 9>,
    // width, height, 1/width, 1/height.
    screen: vec4<f32>,
    // x, y sub-pixel jitter in NDC; z frame index; w ambient occlusion strength.
    jitter: vec4<f32>,
    // Reflection pass: plane (n, d) whose negative side is discarded.
    clip: vec4<f32>,
    // x 1 in the mirrored pass (winding flips), y saturation, z vignette,
    // w toon bands (0 physical shading, 2 or 3 cel bands).
    flags: vec4<f32>,
    counts: vec4<u32>,
    lights: array<Light, 16>,
}

@group(0) @binding(0) var<uniform> fr: Frame;

fn hash12(p: vec2<f32>) -> f32 {
    var p3 = fract(vec3<f32>(p.x, p.y, p.x) * 0.1031);
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

fn vnoise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let a = hash12(i);
    let b = hash12(i + vec2<f32>(1.0, 0.0));
    let c = hash12(i + vec2<f32>(0.0, 1.0));
    let d = hash12(i + vec2<f32>(1.0, 1.0));
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

fn fbm2(p: vec2<f32>) -> f32 {
    var s = 0.0;
    var a = 0.5;
    var q = p;
    for (var i = 0; i < 4; i++) {
        s += a * vnoise(q);
        q = q * 2.03 + vec2<f32>(17.1, 9.2);
        a *= 0.5;
    }
    return s;
}

fn luma(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}
