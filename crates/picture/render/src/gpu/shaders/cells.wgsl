// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// The largest alpha in each occupancy cell of a layer picture: where the
// picture draws anything, for the puppet's mesh. One invocation per cell
// of `cell` × `cell` pixels. Mirrors `puppet::occupancy_cpu`.

struct CellParams {
    // picture size
    size: vec2<u32>,
    // cells across and down
    grid: vec2<u32>,
    cell: u32,
    pad0: u32,
    pad1: u32,
    pad2: u32,
}

@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var dst: texture_storage_2d<rgba16float, write>;
@group(0) @binding(2) var<uniform> cp: CellParams;

@compute @workgroup_size(8, 8, 1)
fn cs_cells(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= cp.grid.x || gid.y >= cp.grid.y) { return; }
    let x0 = gid.x * cp.cell;
    let y0 = gid.y * cp.cell;
    // The last row and column take the rest of the picture.
    var x1 = min(x0 + cp.cell, cp.size.x);
    var y1 = min(y0 + cp.cell, cp.size.y);
    if (gid.x == cp.grid.x - 1u) { x1 = cp.size.x; }
    if (gid.y == cp.grid.y - 1u) { y1 = cp.size.y; }
    var a = 0.0;
    for (var y = y0; y < y1; y = y + 1u) {
        for (var x = x0; x < x1; x = x + 1u) {
            a = max(a, textureLoad(src, vec2<i32>(i32(x), i32(y)), 0).a);
        }
    }
    textureStore(dst, vec2<i32>(gid.xy), vec4<f32>(a, a, a, a));
}
