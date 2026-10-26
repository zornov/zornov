// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// Draws a layer picture through the puppet / corner-pin mesh: for each
// output pixel, the triangles of its 8×8 tile (in order) are tested; the
// last one holding the pixel with the highest depth wins, and the picture
// is sampled bilinearly (transparent outside) at the point the
// barycentric weights give. Mirrors `puppet::warp_cpu` and
// `puppet::barycentric`.

struct WarpParams {
    // output size
    size: vec2<u32>,
    tiles_x: u32,
    tile_count: u32,
}

@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var dst: texture_storage_2d<rgba16float, write>;
@group(0) @binding(2) var<uniform> wp: WarpParams;
// 4 vec4 per triangle: (p0, p1), (p2, uv0), (uv1, uv2), (depth, mode, shade, pad)
// mode 0 draws the picture; 1 a flat premultiplied colour held in the uv
// slots (r, g, b, a: a cut's colour fill); 2 the picture scaled by shade
// (a cut's automatic fill)
@group(0) @binding(3) var<storage, read> tris: array<vec4<f32>>;
// tile_count + 1 offsets, then the triangle lists
@group(0) @binding(4) var<storage, read> bins: array<u32>;

fn fetch(x: i32, y: i32) -> vec4<f32> {
    let d = vec2<i32>(textureDimensions(src));
    if (x < 0 || y < 0 || x >= d.x || y >= d.y) { return vec4<f32>(0.0); }
    return textureLoad(src, vec2<i32>(x, y), 0);
}

fn sample_src(px: f32, py: f32) -> vec4<f32> {
    let x = clamp(px, -1.0e7, 1.0e7) - 0.5;
    let y = clamp(py, -1.0e7, 1.0e7) - 0.5;
    let x0 = floor(x);
    let y0 = floor(y);
    let fx = x - x0;
    let fy = y - y0;
    let xi = i32(x0);
    let yi = i32(y0);
    let a = fetch(xi, yi);
    let b = fetch(xi + 1, yi);
    let c = fetch(xi, yi + 1);
    let d = fetch(xi + 1, yi + 1);
    let top = a + (b - a) * fx;
    let bottom = c + (d - c) * fx;
    return top + (bottom - top) * fy;
}

@compute @workgroup_size(8, 8, 1)
fn cs_warp(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= wp.size.x || gid.y >= wp.size.y) { return; }
    let p = vec2<f32>(f32(gid.x) + 0.5, f32(gid.y) + 0.5);
    let tile = (gid.y / 8u) * wp.tiles_x + gid.x / 8u;
    let start = bins[tile];
    let end = bins[tile + 1u];
    var best = -3.4e38;
    var out = vec4<f32>(0.0);
    for (var i = start; i < end; i = i + 1u) {
        let t = bins[i] * 4u;
        let r0 = tris[t];
        let r1 = tris[t + 1u];
        let r2 = tris[t + 2u];
        let r3 = tris[t + 3u];
        let ax = r0.x; let ay = r0.y; let bx = r0.z; let by = r0.w; let cx = r1.x; let cy = r1.y;
        let den = (bx - ax) * (cy - ay) - (cx - ax) * (by - ay);
        if (abs(den) < 1e-6) { continue; }
        let l1 = ((p.x - ax) * (cy - ay) - (cx - ax) * (p.y - ay)) / den;
        let l2 = ((bx - ax) * (p.y - ay) - (p.x - ax) * (by - ay)) / den;
        let l0 = 1.0 - l1 - l2;
        let e = -1e-4;
        if (l0 < e || l1 < e || l2 < e) { continue; }
        if (r3.x < best) { continue; }
        best = r3.x;
        if (r3.y == 0.0) {
            let u = l0 * r1.z + l1 * r2.x + l2 * r2.z;
            let v = l0 * r1.w + l1 * r2.y + l2 * r2.w;
            out = sample_src(u, v);
        } else if (r3.y < 1.5) {
            out = vec4<f32>(r1.z, r1.w, r2.x, r2.y);
        } else {
            let u = l0 * r1.z + l1 * r2.x + l2 * r2.z;
            let v = l0 * r1.w + l1 * r2.y + l2 * r2.w;
            let s = sample_src(u, v);
            out = vec4<f32>(s.rgb * r3.z, s.a);
        }
    }
    textureStore(dst, vec2<i32>(gid.xy), out);
}
