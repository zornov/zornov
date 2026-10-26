// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// Shared prelude for Studio effect shaders.
//
// Binding layout (group 0):
//   0: input texture (linear premultiplied RGBA)
//   1: sampler, linear filtering, clamp-to-edge
//   2: output storage texture, rgba16float
//   3: uniforms, see `Uniforms`
// Each parameter of the effect takes one vec4 in `p`, in declaration order
// (see `ParamSet::pack_vec4`). Dispatch ceil(w/8) x ceil(h/8) workgroups.

struct Uniforms {
    size: vec2<f32>,
    time: f32,
    seed: f32,
    p: array<vec4<f32>, 16>,
}

@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;
@group(0) @binding(2) var dst: texture_storage_2d<rgba16float, write>;
@group(0) @binding(3) var<uniform> u: Uniforms;

const TAU: f32 = 6.283185307;

// Bilinear sample at a position in pixels (pixel centres at +0.5).
fn tex(pos: vec2<f32>) -> vec4<f32> {
    return textureSampleLevel(src, samp, pos / u.size, 0.0);
}

// Percent of frame height to pixels.
fn pct_h(v: f32) -> f32 {
    return v * 0.01 * u.size.y;
}

fn luma(c: vec4<f32>) -> f32 {
    return dot(c.rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
}

fn hash_u32(x0: u32) -> u32 {
    var x = x0;
    x = x ^ (x >> 16u);
    x = x * 0x7feb352du;
    x = x ^ (x >> 15u);
    x = x * 0x846ca68bu;
    x = x ^ (x >> 16u);
    return x;
}

fn hash2(x: i32, y: i32, seed: u32) -> f32 {
    let h = hash_u32((u32(x) * 0x8da6b343u) ^ (u32(y) * 0xd8163841u) ^ hash_u32(seed));
    return f32(h >> 8u) / 16777216.0;
}


// The per-clip seed, exactly as the CPU has it (passed as raw bits).
fn seed_u() -> u32 {
    return bitcast<u32>(u.seed);
}

fn hash1(x: i32, seed: u32) -> f32 {
    return hash2(x, 0x51ed270b, seed);
}

fn hash3(x: i32, y: i32, z: i32, seed: u32) -> f32 {
    return hash2(x, y ^ (z * bitcast<i32>(0x2c1b3c6du)), seed + bitcast<u32>(z));
}

fn fade(t: f32) -> f32 {
    return t * t * t * (t * (t * 6.0 - 15.0) + 10.0);
}

// Smooth 1D value noise in -1..1, as the CPU's `noise1`.
fn noise1(t: f32, seed: u32) -> f32 {
    let f = floor(t);
    let i = i32(f);
    let k = fade(t - f);
    return mix(hash1(i, seed), hash1(i + 1, seed), k) * 2.0 - 1.0;
}

// Smoothstep that also runs backwards (e0 > e1), as the CPU's.
fn sstep(e0: f32, e1: f32, x: f32) -> f32 {
    if (abs(e1 - e0) < 1e-9) {
        return select(1.0, 0.0, x < e0);
    }
    let t = clamp((x - e0) / (e1 - e0), 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
}

fn srgb_encode(v: f32) -> f32 {
    if (v <= 0.0031308) {
        return v * 12.92;
    }
    return 1.055 * pow(v, 1.0 / 2.4) - 0.055;
}

fn srgb_decode(v: f32) -> f32 {
    if (v <= 0.04045) {
        return v / 12.92;
    }
    return pow((v + 0.055) / 1.055, 2.4);
}

fn mirror_i(i: i32, n: i32) -> i32 {
    let period = 2 * n;
    let m = ((i % period) + period) % period;
    return select(period - 1 - m, m, m < n);
}

fn fetch_mirror(x: i32, y: i32) -> vec4<f32> {
    let w = i32(u.size.x);
    let h = i32(u.size.y);
    return textureLoad(src, vec2<i32>(mirror_i(x, w), mirror_i(y, h)), 0);
}

// Bilinear sample at pixel position `pos`, mirrored past the edges.
fn sample_mirror(pos: vec2<f32>) -> vec4<f32> {
    let p = clamp(pos, vec2<f32>(-1.0e7), vec2<f32>(1.0e7)) - vec2<f32>(0.5);
    let p0 = floor(p);
    let f = p - p0;
    let x = i32(p0.x);
    let y = i32(p0.y);
    let top = mix(fetch_mirror(x, y), fetch_mirror(x + 1, y), f.x);
    let bottom = mix(fetch_mirror(x, y + 1), fetch_mirror(x + 1, y + 1), f.x);
    return mix(top, bottom, f.y);
}

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (f32(gid.x) >= u.size.x || f32(gid.y) >= u.size.y) {
        return;
    }
    let pos = vec2<f32>(gid.xy) + vec2<f32>(0.5, 0.5);
    textureStore(dst, vec2<i32>(gid.xy), effect(pos));
}
