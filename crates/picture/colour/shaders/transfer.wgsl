// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// Transfer functions, mirroring crates/picture/colour/src/transfer.rs.
// Ids match `Transfer::shader_id`: 0 linear, 1 sRGB, 2 BT.709, 3 BT.1886,
// 4 PQ, 5 HLG, 6 ACEScct, 7 ACEScc.

const PQ_REFERENCE_WHITE_NITS: f32 = 203.0;

fn srgb_encode(l: f32) -> f32 {
    if (l <= 0.0031308) { return 12.92 * l; }
    return 1.055 * pow(l, 1.0 / 2.4) - 0.055;
}

fn srgb_decode(v: f32) -> f32 {
    if (v <= 0.04045) { return v / 12.92; }
    return pow((v + 0.055) / 1.055, 2.4);
}

const BT709_ALPHA: f32 = 1.0992968;
const BT709_BETA: f32 = 0.01805397;

fn bt709_oetf(l: f32) -> f32 {
    if (l < BT709_BETA) { return 4.5 * l; }
    return BT709_ALPHA * pow(l, 0.45) - (BT709_ALPHA - 1.0);
}

fn bt709_inverse_oetf(v: f32) -> f32 {
    if (v < 4.5 * BT709_BETA) { return v / 4.5; }
    return pow((v + (BT709_ALPHA - 1.0)) / BT709_ALPHA, 1.0 / 0.45);
}

const PQ_M1: f32 = 0.1593017578125;
const PQ_M2: f32 = 78.84375;
const PQ_C1: f32 = 0.8359375;
const PQ_C2: f32 = 18.8515625;
const PQ_C3: f32 = 18.6875;

fn pq_eotf(e: f32) -> f32 {
    let p = pow(clamp(e, 0.0, 1.0), 1.0 / PQ_M2);
    return pow(max(p - PQ_C1, 0.0) / (PQ_C2 - PQ_C3 * p), 1.0 / PQ_M1) * 10000.0;
}

fn pq_inverse_eotf(nits: f32) -> f32 {
    let y = pow(clamp(nits / 10000.0, 0.0, 1.0), PQ_M1);
    return pow((PQ_C1 + PQ_C2 * y) / (1.0 + PQ_C3 * y), PQ_M2);
}

const HLG_A: f32 = 0.17883277;
const HLG_B: f32 = 0.28466892;
const HLG_C: f32 = 0.55991073;

fn hlg_oetf(e0: f32) -> f32 {
    let e = max(e0, 0.0);
    if (e <= 1.0 / 12.0) { return sqrt(3.0 * e); }
    return HLG_A * log(12.0 * e - HLG_B) + HLG_C;
}

fn hlg_inverse_oetf(v0: f32) -> f32 {
    let v = max(v0, 0.0);
    if (v <= 0.5) { return v * v / 3.0; }
    return (exp((v - HLG_C) / HLG_A) + HLG_B) / 12.0;
}

const CCT_A: f32 = 10.5402377;
const CCT_B: f32 = 0.072905534;
const CCT_LIN_BREAK: f32 = 0.0078125;
const CCT_BREAK: f32 = 0.15525114;
// (log2(65504) + 9.72) / 17.52
const CCT_MAX: f32 = 1.4679964;

fn lin_to_acescct(l: f32) -> f32 {
    if (l <= CCT_LIN_BREAK) { return CCT_A * l + CCT_B; }
    return (log2(l) + 9.72) / 17.52;
}

fn acescct_to_lin(v: f32) -> f32 {
    if (v <= CCT_BREAK) { return (v - CCT_B) / CCT_A; }
    if (v < CCT_MAX) { return exp2(v * 17.52 - 9.72); }
    return 65504.0;
}

fn lin_to_acescc(l: f32) -> f32 {
    if (l <= 0.0) { return (-16.0 + 9.72) / 17.52; }
    if (l < exp2(-15.0)) { return (log2(exp2(-16.0) + l * 0.5) + 9.72) / 17.52; }
    return (log2(l) + 9.72) / 17.52;
}

fn acescc_to_lin(v: f32) -> f32 {
    if (v < (9.72 - 15.0) / 17.52) { return (exp2(v * 17.52 - 9.72) - exp2(-16.0)) * 2.0; }
    if (v < CCT_MAX) { return exp2(v * 17.52 - 9.72); }
    return 65504.0;
}

// SDR curves are mirrored around zero like the CPU path.
fn mirror_sign(x: f32) -> f32 {
    return select(1.0, -1.0, x < 0.0);
}

fn encode_transfer(id: u32, x: f32) -> f32 {
    let s = mirror_sign(x);
    let a = abs(x);
    switch id {
        case 1u: { return s * srgb_encode(a); }
        case 2u: { return s * bt709_oetf(a); }
        case 3u: { return s * pow(a, 1.0 / 2.4); }
        case 4u: { return pq_inverse_eotf(x * PQ_REFERENCE_WHITE_NITS); }
        case 5u: { return hlg_oetf(x); }
        case 6u: { return lin_to_acescct(x); }
        case 7u: { return lin_to_acescc(x); }
        default: { return x; }
    }
}

fn decode_transfer(id: u32, v: f32) -> f32 {
    let s = mirror_sign(v);
    let a = abs(v);
    switch id {
        case 1u: { return s * srgb_decode(a); }
        case 2u: { return s * bt709_inverse_oetf(a); }
        case 3u: { return s * pow(a, 2.4); }
        case 4u: { return pq_eotf(v) / PQ_REFERENCE_WHITE_NITS; }
        case 5u: { return hlg_inverse_oetf(v); }
        case 6u: { return acescct_to_lin(v); }
        case 7u: { return acescc_to_lin(v); }
        default: { return v; }
    }
}

fn encode_transfer3(id: u32, c: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(encode_transfer(id, c.x), encode_transfer(id, c.y), encode_transfer(id, c.z));
}

fn decode_transfer3(id: u32, c: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(decode_transfer(id, c.x), decode_transfer(id, c.y), decode_transfer(id, c.z));
}
