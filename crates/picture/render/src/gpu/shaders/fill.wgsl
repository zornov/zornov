// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// Fills a texture with one premultiplied colour (the canvas background, or
// transparent for a transition side rendered alone).

struct FillParams {
    size: vec2<u32>,
    pad: vec2<u32>,
    colour: vec4<f32>,
}

@group(0) @binding(0) var dst: texture_storage_2d<rgba16float, write>;
@group(0) @binding(1) var<uniform> fp: FillParams;

@compute @workgroup_size(8, 8, 1)
fn cs_fill(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= fp.size.x || gid.y >= fp.size.y) { return; }
    textureStore(dst, vec2<i32>(gid.xy), fp.colour);
}
