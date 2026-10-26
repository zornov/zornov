// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// Scope computations on the GPU, mirroring crates/picture/colour/src/scopes.rs.
// Concatenated after transfer.wgsl by `gpu::scopes_shader_source()`.
//
// Pass 1, `cs_downscale` (group 1): box-filters the full frame into a
// small rgba32float texture of the scope size.
// Pass 2, `cs_scopes` (group 0): one invocation per downscaled pixel adds
// to the counters with atomics. Clear the buffers to zero before pass 2.
//
// Buffer layouts (u32 counters):
//   waveform: levels × width, index = level * width + column
//   parade:   3 × levels × width, channel-major
//   vscope:   size × size, index = row * size + column (row 0 = Cr +0.5)
//   hist:     4 × levels: red, green, blue, luma

struct ScopeParams {
    // transfer id, levels, vectorscope size, downscaled width
    cfg: vec4<u32>,
    // luma weights (xyz); w = 2·(1 − Kb), the Cb divisor
    luma: vec4<f32>,
    // x = 2·(1 − Kr), the Cr divisor
    chroma: vec4<f32>,
}

@group(0) @binding(0) var small: texture_2d<f32>;
@group(0) @binding(1) var<uniform> sp: ScopeParams;
@group(0) @binding(2) var<storage, read_write> waveform: array<atomic<u32>>;
@group(0) @binding(3) var<storage, read_write> parade: array<atomic<u32>>;
@group(0) @binding(4) var<storage, read_write> vscope: array<atomic<u32>>;
@group(0) @binding(5) var<storage, read_write> hist: array<atomic<u32>>;

fn level_of(v: f32, levels: u32) -> u32 {
    return min(u32(round(clamp(v, 0.0, 1.0) * f32(levels - 1u))), levels - 1u);
}

@compute @workgroup_size(8, 8, 1)
fn cs_scopes(@builtin(global_invocation_id) gid: vec3<u32>) {
    let dims = textureDimensions(small);
    if (gid.x >= dims.x || gid.y >= dims.y) { return; }
    let tid = sp.cfg.x;
    let levels = sp.cfg.y;
    let vs = sp.cfg.z;
    let width = sp.cfg.w;
    let p = textureLoad(small, vec2<i32>(gid.xy), 0);
    let e = encode_transfer3(tid, p.rgb);
    let y = dot(e, sp.luma.xyz);
    let x = gid.x;

    atomicAdd(&waveform[level_of(y, levels) * width + x], 1u);
    for (var c = 0u; c < 3u; c++) {
        atomicAdd(&parade[(c * levels + level_of(e[c], levels)) * width + x], 1u);
        atomicAdd(&hist[c * levels + level_of(e[c], levels)], 1u);
    }
    atomicAdd(&hist[3u * levels + level_of(y, levels)], 1u);

    let cb = (e.z - y) / sp.luma.w;
    let cr = (e.x - y) / sp.chroma.x;
    let col = level_of(cb + 0.5, vs);
    let row = level_of(0.5 - cr, vs);
    atomicAdd(&vscope[row * vs + col], 1u);
}

@group(1) @binding(0) var full: texture_2d<f32>;
@group(1) @binding(1) var small_out: texture_storage_2d<rgba32float, write>;

@compute @workgroup_size(8, 8, 1)
fn cs_downscale(@builtin(global_invocation_id) gid: vec3<u32>) {
    let od = textureDimensions(small_out);
    if (gid.x >= od.x || gid.y >= od.y) { return; }
    let id = textureDimensions(full);
    let x0 = gid.x * id.x / od.x;
    let x1 = max((gid.x + 1u) * id.x / od.x, x0 + 1u);
    let y0 = gid.y * id.y / od.y;
    let y1 = max((gid.y + 1u) * id.y / od.y, y0 + 1u);
    var s = vec4<f32>(0.0);
    for (var yy = y0; yy < y1; yy++) {
        for (var xx = x0; xx < x1; xx++) {
            s += textureLoad(full, vec2<i32>(i32(xx), i32(yy)), 0);
        }
    }
    textureStore(small_out, vec2<i32>(gid.xy), s / f32((x1 - x0) * (y1 - y0)));
}
