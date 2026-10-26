// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// Picture-space helpers: 2x2 box downsample (minification prefilter),
// separable Gaussian blur, 90-degree rotations.

struct ResampleParams {
    // destination size
    size: vec2<u32>,
    // blur: direction (0 horizontal, 1 vertical) and kernel radius;
    // rotate: degrees / 90
    a: u32,
    b: u32,
}

@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var dst: texture_storage_2d<rgba16float, write>;
@group(0) @binding(2) var<uniform> rp: ResampleParams;
@group(0) @binding(3) var<storage, read> kernel: array<f32>;

fn load_clamped(x: i32, y: i32) -> vec4<f32> {
    let d = vec2<i32>(textureDimensions(src));
    return textureLoad(src, vec2<i32>(clamp(x, 0, d.x - 1), clamp(y, 0, d.y - 1)), 0);
}

@compute @workgroup_size(8, 8, 1)
fn cs_half(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= rp.size.x || gid.y >= rp.size.y) { return; }
    let x0 = i32(gid.x) * 2;
    let y0 = i32(gid.y) * 2;
    let s = load_clamped(x0, y0) + load_clamped(x0 + 1, y0) + load_clamped(x0, y0 + 1) + load_clamped(x0 + 1, y0 + 1);
    textureStore(dst, vec2<i32>(gid.xy), s * 0.25);
}

@compute @workgroup_size(8, 8, 1)
fn cs_blur(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= rp.size.x || gid.y >= rp.size.y) { return; }
    let d = vec2<i32>(textureDimensions(src));
    let r = i32(rp.b);
    var acc = vec4<f32>(0.0);
    for (var i = -r; i <= r; i = i + 1) {
        let k = kernel[u32(i + r)];
        var sx = i32(gid.x);
        var sy = i32(gid.y);
        if (rp.a == 0u) { sx = sx + i; } else { sy = sy + i; }
        if (sx >= 0 && sy >= 0 && sx < d.x && sy < d.y) {
            acc = acc + textureLoad(src, vec2<i32>(sx, sy), 0) * k;
        }
    }
    textureStore(dst, vec2<i32>(gid.xy), acc);
}

@compute @workgroup_size(8, 8, 1)
fn cs_rotate(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= rp.size.x || gid.y >= rp.size.y) { return; }
    let d = vec2<i32>(textureDimensions(src));
    let x = i32(gid.x);
    let y = i32(gid.y);
    var s: vec2<i32>;
    switch rp.a {
        case 1u: { s = vec2<i32>(y, d.y - 1 - x); }
        case 2u: { s = vec2<i32>(d.x - 1 - x, d.y - 1 - y); }
        case 3u: { s = vec2<i32>(d.x - 1 - y, x); }
        default: { s = vec2<i32>(x, y); }
    }
    textureStore(dst, vec2<i32>(gid.xy), textureLoad(src, s, 0));
}
