// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// Decoded Y'CbCr planes -> linear premultiplied RGBA (working space).
// Mirrors `yuv.rs` / `colour_space.rs` step for step.

struct ConvertParams {
    size: vec2<u32>,
    chroma_div: vec2<u32>,
    // y_off, y_scale, c_off, c_scale
    quant: vec4<f32>,
    // kr, kb, matrix_mode (0 = Y'CbCr, 1 = planes hold G,B,R), transfer id
    coef: vec4<f32>,
    // tone map (1 = yes), has primaries matrix, 0, 0
    flags: vec4<f32>,
    m0: vec4<f32>,
    m1: vec4<f32>,
    m2: vec4<f32>,
}

@group(0) @binding(0) var y_tex: texture_2d<u32>;
@group(0) @binding(1) var u_tex: texture_2d<u32>;
@group(0) @binding(2) var v_tex: texture_2d<u32>;
@group(0) @binding(3) var dst: texture_storage_2d<rgba16float, write>;
@group(0) @binding(4) var<uniform> p: ConvertParams;

fn mirror_pow(x: f32, e: f32) -> f32 {
    if (x < 0.0) { return -pow(-x, e); }
    return pow(x, e);
}

fn srgb_decode(v: f32) -> f32 {
    let a = abs(v);
    var r: f32;
    if (a <= 0.04045) { r = a / 12.92; } else { r = pow((a + 0.055) / 1.055, 2.4); }
    return select(r, -r, v < 0.0);
}

const PQ_M1: f32 = 0.1593017578125;
const PQ_M2: f32 = 78.84375;
const PQ_C1: f32 = 0.8359375;
const PQ_C2: f32 = 18.8515625;
const PQ_C3: f32 = 18.6875;

fn pq_decode(e: f32) -> f32 {
    let q = pow(clamp(e, 0.0, 1.0), 1.0 / PQ_M2);
    let nits = pow(max(q - PQ_C1, 0.0) / (PQ_C2 - PQ_C3 * q), 1.0 / PQ_M1) * 10000.0;
    return nits / 203.0;
}

const HLG_A: f32 = 0.17883277;
const HLG_B: f32 = 0.28466892;
const HLG_C: f32 = 0.55991073;

fn hlg_decode(v0: f32) -> f32 {
    let v = max(v0, 0.0);
    var scene: f32;
    if (v <= 0.5) { scene = v * v / 3.0; } else { scene = (exp((v - HLG_C) / HLG_A) + HLG_B) / 12.0; }
    // System gamma 1.2 per channel, then 203 nits -> 1.0 on a 1000 nit display.
    return mirror_pow(scene, 1.2) / (203.0 / 1000.0);
}

// Ids match `SourceTransfer::shader_id`; 16 and up are camera curves
// (`camera_decode`, prepended from the colour crate), two ids per curve:
// even = full range, odd = limited range mapped back to full first.
// The input is clamped to the same
// domain as the CPU's `DecodeLut` (-0.1 .. 1.2: limited-range super-blacks
// and super-whites pass, wilder values do not), so both paths agree.
fn decode_source(id: u32, v0: f32) -> f32 {
    let v = clamp(v0, -0.1, 1.2);
    if (id >= 16u) {
        let k = id - 16u;
        var c = v;
        if ((k & 1u) == 1u) { c = (v * 876.0 + 64.0) / 1023.0; }
        return camera_decode(k / 2u, c);
    }
    switch id {
        case 0u: { return srgb_decode(v); }
        case 1u: { return mirror_pow(v, 2.4); }
        case 2u: { return mirror_pow(v, 2.2); }
        case 3u: { return mirror_pow(v, 2.8); }
        case 4u: { return pq_decode(v); }
        case 5u: { return hlg_decode(v); }
        default: { return v; }
    }
}

fn tone_map(rgb: vec3<f32>) -> vec3<f32> {
    let k = 0.8;
    let top = 2.0;
    let m = max(rgb.r, max(rgb.g, rgb.b));
    if (m <= k) { return rgb; }
    let mapped = k + (top - k) * (1.0 - exp(-(m - k) / (top - k)));
    return rgb * (mapped / m);
}

@compute @workgroup_size(8, 8, 1)
fn cs_yuv(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= p.size.x || gid.y >= p.size.y) { return; }
    let yy = f32(textureLoad(y_tex, vec2<i32>(gid.xy), 0).r);
    let cpos = vec2<i32>(gid.xy / p.chroma_div);
    let cb = f32(textureLoad(u_tex, cpos, 0).r);
    let cr = f32(textureLoad(v_tex, cpos, 0).r);
    let yn = (yy - p.quant.x) * p.quant.y;
    var rgbp: vec3<f32>;
    if (p.coef.z > 0.5) {
        rgbp = vec3<f32>((cr - p.quant.x) * p.quant.y, yn, (cb - p.quant.x) * p.quant.y);
    } else {
        let cbn = (cb - p.quant.z) * p.quant.w;
        let crn = (cr - p.quant.z) * p.quant.w;
        let kr = p.coef.x;
        let kb = p.coef.y;
        let kg = 1.0 - kr - kb;
        let r = yn + 2.0 * (1.0 - kr) * crn;
        let b = yn + 2.0 * (1.0 - kb) * cbn;
        let g = (yn - kr * r - kb * b) / kg;
        rgbp = vec3<f32>(r, g, b);
    }
    let id = u32(p.coef.w);
    var rgb = vec3<f32>(decode_source(id, rgbp.r), decode_source(id, rgbp.g), decode_source(id, rgbp.b));
    if (p.flags.y > 0.5) {
        rgb = vec3<f32>(dot(p.m0.xyz, rgb), dot(p.m1.xyz, rgb), dot(p.m2.xyz, rgb));
    }
    if (p.flags.x > 0.5) {
        rgb = tone_map(rgb);
    }
    textureStore(dst, vec2<i32>(gid.xy), vec4<f32>(rgb, 1.0));
}
