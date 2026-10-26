// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// One pass of a ground shadow: each pixel of the pass's region is mapped
// back onto the layer picture through the sheared map, and the picture's
// alpha there, faded by its height above the feet and saturated by the
// gain, is the shadow's strength. Mirrors `shadow::silhouette` step for step; the blur and the
// compositing are the shared passes.

struct ShadowParams {
    // inverse map rows: (a, b, tx, 0), (c, d, ty, 0): target px -> layer px
    inv0: vec4<f32>,
    inv1: vec4<f32>,
    // the layer's own map, y row (layer px -> target y), and the feet's y
    row_y: vec4<f32>,
    // premultiplied by nothing yet: rgb and peak strength
    colour: vec4<f32>,
    // visible rectangle in layer pixels: x0, y0, x1, y1
    visible: vec4<f32>,
    origin: vec2<f32>,
    fade: f32,
    gain: f32,
    size: vec2<u32>,
    pad1: u32,
    pad2: u32,
}

@group(0) @binding(0) var layer: texture_2d<f32>;
@group(0) @binding(1) var dst: texture_storage_2d<rgba16float, write>;
@group(0) @binding(2) var<uniform> sp: ShadowParams;

fn fetch(x: i32, y: i32) -> vec4<f32> {
    let d = vec2<i32>(textureDimensions(layer));
    if (x < 0 || y < 0 || x >= d.x || y >= d.y) { return vec4<f32>(0.0); }
    return textureLoad(layer, vec2<i32>(x, y), 0);
}

fn sample_layer(px: f32, py: f32) -> vec4<f32> {
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
fn cs_shadow(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= sp.size.x || gid.y >= sp.size.y) { return; }
    let q = vec2<f32>(sp.origin.x + f32(gid.x) + 0.5, sp.origin.y + f32(gid.y) + 0.5);
    let p = vec2<f32>(sp.inv0.x * q.x + sp.inv0.y * q.y + sp.inv0.z, sp.inv1.x * q.x + sp.inv1.y * q.y + sp.inv1.z);
    var out = vec4<f32>(0.0);
    let v = sp.visible;
    if (p.x >= v.x && p.x <= v.z && p.y >= v.y && p.y <= v.w) {
        let y = sp.row_y.x * p.x + sp.row_y.y * p.y + sp.row_y.z;
        let h = max(sp.row_y.w - y, 0.0);
        var fall = 1.0;
        if (sp.fade > 0.0) { fall = exp(-h / sp.fade); }
        let a = min(sample_layer(p.x, p.y).a * fall * sp.gain, 1.0) * sp.colour.a;
        if (a > 0.0) { out = vec4<f32>(sp.colour.rgb * a, a); }
    }
    textureStore(dst, vec2<i32>(gid.xy), out);
}
