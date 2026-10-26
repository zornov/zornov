// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// Shared prelude for compositing node shaders.
//
// Binding layout (group 0):
//   0: A (the main input), linear premultiplied RGBA
//   1: A's extra channels: depth, motion x, motion y, id
//   2: B (second input), 3: B's extra channels
//   4: the mask node's picture (its alpha limits the node)
//   5: sampler, linear filtering, clamp-to-edge
//   6: output RGBA (rgba16float), 7: output extra channels (rgba16float)
//   8: uniforms, see `Uniforms`
// A node shader defines `node(pos) -> vec4<f32>` and may set `aux_out`.
// Missing inputs are bound to a 1×1 transparent picture with the flags
// below at zero. Parameters arrive one vec4 each in `p` (see the node's
// packer). Dispatch ceil(w/8) × ceil(h/8) workgroups.

struct Uniforms {
    size: vec2<f32>,
    time: f32,
    seed: f32,
    mix_amount: f32,
    has_b: f32,
    has_mask: f32,
    has_aux: f32,
    p: array<vec4<f32>, 16>,
}

@group(0) @binding(0) var a_tex: texture_2d<f32>;
@group(0) @binding(1) var a_aux: texture_2d<f32>;
@group(0) @binding(2) var b_tex: texture_2d<f32>;
@group(0) @binding(3) var b_aux: texture_2d<f32>;
@group(0) @binding(4) var m_tex: texture_2d<f32>;
@group(0) @binding(5) var samp: sampler;
@group(0) @binding(6) var dst: texture_storage_2d<rgba16float, write>;
@group(0) @binding(7) var dst_aux: texture_storage_2d<rgba16float, write>;
@group(0) @binding(8) var<uniform> u: Uniforms;

var<private> aux_out: vec4<f32>;
var<private> cur: vec2<i32>;

fn clampi(q: vec2<i32>) -> vec2<i32> {
    return clamp(q, vec2<i32>(0), vec2<i32>(u.size) - vec2<i32>(1));
}

fn load_a(q: vec2<i32>) -> vec4<f32> {
    return textureLoad(a_tex, clampi(q), 0);
}

fn load_b(q: vec2<i32>) -> vec4<f32> {
    if (u.has_b < 0.5) {
        return vec4<f32>(0.0);
    }
    return textureLoad(b_tex, clampi(q), 0);
}

fn aux_a(q: vec2<i32>) -> vec4<f32> {
    return textureLoad(a_aux, clampi(q), 0);
}

fn aux_b(q: vec2<i32>) -> vec4<f32> {
    return textureLoad(b_aux, clampi(q), 0);
}

// Bilinear sample of A at a pixel position (centres at +0.5).
fn sample_a(pos: vec2<f32>) -> vec4<f32> {
    return textureSampleLevel(a_tex, samp, pos / u.size, 0.0);
}

// Bilinear sample of A that is transparent outside the picture.
fn sample_a_clear(pos: vec2<f32>) -> vec4<f32> {
    if (pos.x < 0.0 || pos.y < 0.0 || pos.x > u.size.x || pos.y > u.size.y) {
        return vec4<f32>(0.0);
    }
    return sample_a(pos);
}

fn straight(c: vec4<f32>) -> vec3<f32> {
    if (c.a > 0.000001) {
        return c.rgb / c.a;
    }
    return vec3<f32>(0.0);
}

fn luma(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}

fn srgb_encode(v: f32) -> f32 {
    let c = clamp(v, 0.0, 1.0);
    if (c <= 0.0031308) {
        return c * 12.92;
    }
    return 1.055 * pow(c, 1.0 / 2.4) - 0.055;
}

fn sstep(e0: f32, e1: f32, x: f32) -> f32 {
    if (abs(e1 - e0) < 1e-9) {
        return select(1.0, 0.0, x < e0);
    }
    let t = clamp((x - e0) / (e1 - e0), 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
}

// Maps an output pixel through the 3×3 matrix in p[o], p[o+1], p[o+2].
fn mat_apply(o: i32, pos: vec2<f32>) -> vec2<f32> {
    let r0 = u.p[o].xyz;
    let r1 = u.p[o + 1].xyz;
    let r2 = u.p[o + 2].xyz;
    let v = vec3<f32>(pos, 1.0);
    let z = dot(r2, v);
    if (abs(z) < 1e-12) {
        return vec2<f32>(-1.0e7);
    }
    return vec2<f32>(dot(r0, v), dot(r1, v)) / z;
}

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (f32(gid.x) >= u.size.x || f32(gid.y) >= u.size.y) {
        return;
    }
    cur = vec2<i32>(gid.xy);
    let pos = vec2<f32>(gid.xy) + vec2<f32>(0.5, 0.5);
    aux_out = aux_a(cur);
    var out = node(pos);
    var k = u.mix_amount;
    if (u.has_mask > 0.5) {
        k = k * clamp(textureLoad(m_tex, cur, 0).a, 0.0, 1.0);
    }
    if (k < 1.0) {
        out = mix(load_a(cur), out, k);
    }
    textureStore(dst, cur, out);
    textureStore(dst_aux, cur, aux_out);
}
