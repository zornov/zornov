// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// Straight-alpha sRGB RGBA8 still -> linear premultiplied RGBA.

struct RgbaParams {
    size: vec2<u32>,
    pad: vec2<u32>,
}

@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var dst: texture_storage_2d<rgba16float, write>;
@group(0) @binding(2) var<uniform> p: RgbaParams;

fn srgb_decode(v: f32) -> f32 {
    if (v <= 0.04045) { return v / 12.92; }
    return pow((v + 0.055) / 1.055, 2.4);
}

@compute @workgroup_size(8, 8, 1)
fn cs_rgba8(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= p.size.x || gid.y >= p.size.y) { return; }
    let c = textureLoad(src, vec2<i32>(gid.xy), 0);
    let rgb = vec3<f32>(srgb_decode(c.r), srgb_decode(c.g), srgb_decode(c.b)) * c.a;
    textureStore(dst, vec2<i32>(gid.xy), vec4<f32>(rgb, c.a));
}
