// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// Places one layer on the canvas: inverse-map each canvas pixel into the
// layer picture, sample bilinearly (transparent outside), apply opacity and
// blend with the W3C formulas. `blend == 100` is the adjustment mix: the
// layer is a processed copy of the canvas, mixed in by mask coverage.
// Mirrors `blend.rs` and `CpuCompositor::draw`.

struct CompParams {
    // inverse matrix rows: (a, b, tx, 0), (c, d, ty, 0): canvas px -> layer px
    inv0: vec4<f32>,
    inv1: vec4<f32>,
    // visible rectangle in layer pixels: x0, y0, x1, y1
    visible: vec4<f32>,
    canvas_size: vec2<u32>,
    opacity: f32,
    blend: u32,
    mask_count: u32,
    mask_init: f32,
    pad0: u32,
    pad1: u32,
}

@group(0) @binding(0) var canvas_in: texture_2d<f32>;
@group(0) @binding(1) var canvas_out: texture_storage_2d<rgba16float, write>;
@group(0) @binding(2) var layer: texture_2d<f32>;
@group(0) @binding(3) var<uniform> cp: CompParams;
@group(0) @binding(4) var<storage, read> masks: array<MaskGpu>;
@group(0) @binding(5) var<storage, read> poly: array<vec2<f32>>;

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
fn cs_composite(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= cp.canvas_size.x || gid.y >= cp.canvas_size.y) { return; }
    let pos = vec2<i32>(gid.xy);
    let b = textureLoad(canvas_in, pos, 0);
    let cx = f32(gid.x) + 0.5;
    let cy = f32(gid.y) + 0.5;
    if (cp.blend == 100u) {
        // Adjustment mix: layer is the processed canvas, same size.
        var c = cp.opacity;
        if (cp.mask_count > 0u) { c = c * masks_at(cp.mask_count, cp.mask_init, cx, cy); }
        let q = textureLoad(layer, pos, 0);
        textureStore(canvas_out, pos, b + (q - b) * c);
        return;
    }
    let lx = cp.inv0.x * cx + cp.inv0.y * cy + cp.inv0.z;
    let ly = cp.inv1.x * cx + cp.inv1.y * cy + cp.inv1.z;
    if (lx < cp.visible.x || lx > cp.visible.z || ly < cp.visible.y || ly > cp.visible.w) {
        // Outside a matte nothing covers, so a stencil clears the canvas.
        if (cp.blend >= 17u && cp.blend <= 20u) {
            textureStore(canvas_out, pos, matte(cp.blend, b, vec4<f32>(0.0)));
        } else {
            textureStore(canvas_out, pos, b);
        }
        return;
    }
    let s = sample_layer(lx, ly) * cp.opacity;
    textureStore(canvas_out, pos, composite(cp.blend, b, s));
}
