// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// Working canvas -> output colour space, dithered to the storage format.
// `OUT_FORMAT`, `OUT_SCALE` and `OUT_STORE` are substituted when the module
// is built: rgba8unorm / 255 / `q / 255.0`, or rgba16uint / 65535 /
// `vec4<u32>(q)` (16-bit normalised formats are not storage formats in
// WebGPU, so the 16-bit path stores integer counts). Mirrors `output.rs`.

struct OutParams {
    size: vec2<u32>,
    space: u32,
    premultiplied: u32,
    dither: u32,
    frame: u32,
    has_matrix: u32,
    pad: u32,
    m0: vec4<f32>,
    m1: vec4<f32>,
    m2: vec4<f32>,
}

@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var dst: texture_storage_2d<OUT_FORMAT, write>;
@group(0) @binding(2) var<uniform> op: OutParams;

fn mirror_pow(x: f32, e: f32) -> f32 {
    if (x < 0.0) { return -pow(-x, e); }
    return pow(x, e);
}

fn srgb_encode(l: f32) -> f32 {
    let a = abs(l);
    var r: f32;
    if (a <= 0.0031308) { r = 12.92 * a; } else { r = 1.055 * pow(a, 1.0 / 2.4) - 0.055; }
    return select(r, -r, l < 0.0);
}

const PQ_M1: f32 = 0.1593017578125;
const PQ_M2: f32 = 78.84375;
const PQ_C1: f32 = 0.8359375;
const PQ_C2: f32 = 18.8515625;
const PQ_C3: f32 = 18.6875;

fn pq_encode(l: f32) -> f32 {
    let y = pow(clamp(l * 203.0 / 10000.0, 0.0, 1.0), PQ_M1);
    return pow((PQ_C1 + PQ_C2 * y) / (1.0 + PQ_C3 * y), PQ_M2);
}

const HLG_A: f32 = 0.17883277;
const HLG_B: f32 = 0.28466892;
const HLG_C: f32 = 0.55991073;

fn hlg_encode(l: f32) -> f32 {
    let e = max(mirror_pow(l * 203.0 / 1000.0, 1.0 / 1.2), 0.0);
    if (e <= 1.0 / 12.0) { return sqrt(3.0 * e); }
    return HLG_A * log(12.0 * e - HLG_B) + HLG_C;
}

// Ids match `OutputSpace::shader_id`.
fn encode_output(id: u32, l: f32) -> f32 {
    switch id {
        case 1u: { return mirror_pow(l, 1.0 / 2.4); }
        case 3u: { return pq_encode(l); }
        case 4u: { return hlg_encode(l); }
        case 5u: {
            if (l <= 0.0078125) { return 10.5402377416545 * l + 0.0729055341958355; }
            return (log2(l) + 9.72) / 17.52;
        }
        default: { return srgb_encode(l); }
    }
}

fn hash32(x0: u32) -> u32 {
    var x = x0;
    x = x ^ (x >> 16u);
    x = x * 0x7feb352du;
    x = x ^ (x >> 15u);
    x = x * 0x846ca68bu;
    x = x ^ (x >> 16u);
    return x;
}

fn dither_noise(x: u32, y: u32, channel: u32, frame: u32) -> f32 {
    let seed = hash32((x * 0x9E3779B1u) ^ (y * 0x85EBCA77u) ^ (channel * 0xC2B2AE3Du) ^ (frame * 0x27D4EB2Fu));
    let a = f32(seed & 0xffffu) / 65536.0;
    let b = f32(seed >> 16u) / 65536.0;
    return a + b - 1.0;
}

@compute @workgroup_size(8, 8, 1)
fn cs_output(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= op.size.x || gid.y >= op.size.y) { return; }
    let pos = vec2<i32>(gid.xy);
    let p = textureLoad(src, pos, 0);
    let a = clamp(p.a, 0.0, 1.0);
    var rgb = p.rgb;
    if (op.premultiplied == 0u) {
        if (p.a <= 0.0) { rgb = vec3<f32>(0.0); } else { rgb = p.rgb / p.a; }
    }
    if (op.has_matrix != 0u) {
        rgb = vec3<f32>(dot(op.m0.xyz, rgb), dot(op.m1.xyz, rgb), dot(op.m2.xyz, rgb));
    }
    var e = vec3<f32>(encode_output(op.space, rgb.r), encode_output(op.space, rgb.g), encode_output(op.space, rgb.b));
    var lim = 1.0;
    if (op.premultiplied != 0u) { lim = a; }
    e = clamp(e, vec3<f32>(0.0), vec3<f32>(lim));
    var v = vec4<f32>(e, a) * OUT_SCALE;
    if (op.dither != 0u) {
        v = v + vec4<f32>(
            dither_noise(gid.x, gid.y, 0u, op.frame),
            dither_noise(gid.x, gid.y, 1u, op.frame),
            dither_noise(gid.x, gid.y, 2u, op.frame),
            dither_noise(gid.x, gid.y, 3u, op.frame));
    }
    let q = clamp(floor(v + 0.5), vec4<f32>(0.0), vec4<f32>(OUT_SCALE));
    textureStore(dst, pos, OUT_STORE);
}
