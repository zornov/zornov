// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// Cut-out matte: picture * bilinear(matte red channel), edges clamped.
// Mirrors `matte.rs` step for step.

struct MatteParams {
    size: vec2<u32>,
    msize: vec2<u32>,
    invert: u32,
    pad0: u32,
    pad1: u32,
    pad2: u32,
}

@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var dst: texture_storage_2d<rgba16float, write>;
@group(0) @binding(2) var<uniform> p: MatteParams;
@group(0) @binding(3) var matte: texture_2d<f32>;

fn m_at(x: f32, y: f32) -> f32 {
    let xi = clamp(i32(x), 0, i32(p.msize.x) - 1);
    let yi = clamp(i32(y), 0, i32(p.msize.y) - 1);
    return textureLoad(matte, vec2<i32>(xi, yi), 0).r;
}

@compute @workgroup_size(8, 8, 1)
fn cs_matte(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= p.size.x || gid.y >= p.size.y) { return; }
    let fx = (f32(gid.x) + 0.5) * f32(p.msize.x) / f32(p.size.x) - 0.5;
    let fy = (f32(gid.y) + 0.5) * f32(p.msize.y) / f32(p.size.y) - 0.5;
    let x0 = floor(fx);
    let y0 = floor(fy);
    let tx = fx - x0;
    let ty = fy - y0;
    let top = m_at(x0, y0) + (m_at(x0 + 1.0, y0) - m_at(x0, y0)) * tx;
    let bottom = m_at(x0, y0 + 1.0) + (m_at(x0 + 1.0, y0 + 1.0) - m_at(x0, y0 + 1.0)) * tx;
    var a = top + (bottom - top) * ty;
    if (p.invert != 0u) { a = 1.0 - a; }
    textureStore(dst, vec2<i32>(gid.xy), textureLoad(src, vec2<i32>(gid.xy), 0) * a);
}
