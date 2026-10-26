// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// Shared prelude for Studio transition shaders.
//
// Binding layout (group 0):
//   0: outgoing picture A, 1: incoming picture B (linear premultiplied)
//   2: sampler, linear filtering, clamp-to-edge
//   3: output storage texture, rgba16float
//   4: uniforms; `progress` is 0..1. Parameters are packed as for effects.
// The renderer copies A at progress 0 and B at progress 1 instead of
// running the shader, which keeps the ends exact.

struct Uniforms {
    size: vec2<f32>,
    progress: f32,
    seed: f32,
    p: array<vec4<f32>, 16>,
}

@group(0) @binding(0) var src_a: texture_2d<f32>;
@group(0) @binding(1) var src_b: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;
@group(0) @binding(3) var dst: texture_storage_2d<rgba16float, write>;
@group(0) @binding(4) var<uniform> u: Uniforms;

const TAU: f32 = 6.283185307;
const PI: f32 = 3.14159265;

fn tex_a(pos: vec2<f32>) -> vec4<f32> {
    return textureSampleLevel(src_a, samp, pos / u.size, 0.0);
}

fn tex_b(pos: vec2<f32>) -> vec4<f32> {
    return textureSampleLevel(src_b, samp, pos / u.size, 0.0);
}

fn inside(pos: vec2<f32>) -> bool {
    return pos.x >= 0.0 && pos.y >= 0.0 && pos.x <= u.size.x && pos.y <= u.size.y;
}

// Easing curves, by index into EASING_NAMES (a subset: others fall back to
// smooth in-out; the CPU path implements all of them).
fn ease(i: f32, t: f32) -> f32 {
    let k = i32(i + 0.5);
    if (k == 0) {
        return t;
    }
    if (k == 3) {
        return 0.5 - 0.5 * cos(PI * t);
    }
    if (k == 9) {
        if (t < 0.5) {
            return 4.0 * t * t * t;
        }
        let v = 2.0 - 2.0 * t;
        return 1.0 - v * v * v * 0.5;
    }
    return t * t * (3.0 - 2.0 * t);
}

fn reveal_mask(m: f32, e: f32, soft: f32) -> f32 {
    let edge = e * (1.0 + soft);
    return smoothstep(edge, edge - soft, m);
}

// Direction choice (0 left, 1 right, 2 up, 3 down) as a pixel offset of a
// whole frame.
fn dir_vec(d: f32) -> vec2<f32> {
    let k = i32(d + 0.5);
    if (k == 0) { return vec2<f32>(-u.size.x, 0.0); }
    if (k == 1) { return vec2<f32>(u.size.x, 0.0); }
    if (k == 2) { return vec2<f32>(0.0, -u.size.y); }
    return vec2<f32>(0.0, u.size.y);
}

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (f32(gid.x) >= u.size.x || f32(gid.y) >= u.size.y) {
        return;
    }
    let pos = vec2<f32>(gid.xy) + vec2<f32>(0.5, 0.5);
    textureStore(dst, vec2<i32>(gid.xy), transition(pos, u.progress));
}
