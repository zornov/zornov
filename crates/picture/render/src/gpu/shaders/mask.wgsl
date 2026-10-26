// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// Multiplies a layer picture by its combined mask coverage.

struct MaskParams {
    size: vec2<u32>,
    count: u32,
    init: f32,
}

@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var dst: texture_storage_2d<rgba16float, write>;
@group(0) @binding(2) var<uniform> mp: MaskParams;
@group(0) @binding(3) var<storage, read> masks: array<MaskGpu>;
@group(0) @binding(4) var<storage, read> poly: array<vec2<f32>>;

@compute @workgroup_size(8, 8, 1)
fn cs_mask(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= mp.size.x || gid.y >= mp.size.y) { return; }
    let c = masks_at(mp.count, mp.init, f32(gid.x) + 0.5, f32(gid.y) + 0.5);
    let p = textureLoad(src, vec2<i32>(gid.xy), 0);
    textureStore(dst, vec2<i32>(gid.xy), p * c);
}
