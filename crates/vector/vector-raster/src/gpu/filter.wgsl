// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// Layer effects on premultiplied floating-point images, mirroring
// filter.rs: separable Gaussian blur, alpha planes shifted (and inverted)
// for shadows and glows, a separable maximum for spread, and the
// combinations that put the effect under or inside the art. `pack`
// stores a finished image, 8 bits per channel, for later passes to sample.

struct Params {
    size: vec2<u32>,
    // blur: direction (0 = x, 1 = y) and kernel radius; shift: integer
    // offset; dilate: direction and radius.
    a: vec2<i32>,
    // shift: fractional offset; dilate: fractional weight of the last tap.
    f: vec2<f32>,
    // combine mode (0 under, 1 atop, 2 scale) / inverted flag / pack offset.
    mode: u32,
    extra: u32,
    color: vec4<f32>,
}

@group(0) @binding(0) var<uniform> prm: Params;
@group(0) @binding(1) var<storage, read> src: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read_write> dst: array<vec4<f32>>;
@group(0) @binding(3) var<storage, read> aux: array<vec4<f32>>;
@group(0) @binding(4) var<storage, read> kern: array<f32>;
@group(0) @binding(5) var<storage, read_write> packed: array<u32>;

fn idx(x: i32, y: i32) -> u32 { return u32(y) * prm.size.x + u32(x); }

fn inside(x: i32, y: i32) -> bool {
    return x >= 0 && y >= 0 && x < i32(prm.size.x) && y < i32(prm.size.y);
}

@compute @workgroup_size(16, 16)
fn blur(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= prm.size.x || id.y >= prm.size.y) { return; }
    let x = i32(id.x);
    let y = i32(id.y);
    let r = prm.a.y;
    var step = vec2<i32>(1, 0);
    if (prm.a.x == 1) { step = vec2<i32>(0, 1); }
    var acc = src[idx(x, y)] * kern[0];
    for (var j = 1; j <= r; j++) {
        let k = kern[j];
        let p = vec2<i32>(x, y) - step * j;
        let q = vec2<i32>(x, y) + step * j;
        if (inside(p.x, p.y)) { acc += src[idx(p.x, p.y)] * k; }
        if (inside(q.x, q.y)) { acc += src[idx(q.x, q.y)] * k; }
    }
    dst[idx(x, y)] = acc;
}

fn alpha_at(x: i32, y: i32, inv: bool) -> f32 {
    if (!inside(x, y)) { return select(0.0, 1.0, inv); }
    let a = src[idx(x, y)].w;
    return select(a, 1.0 - a, inv);
}

// dst = the source's alpha (or 1 - alpha) moved by the offset, in .w.
@compute @workgroup_size(16, 16)
fn shift_alpha(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= prm.size.x || id.y >= prm.size.y) { return; }
    let inv = prm.extra != 0u;
    let sx = i32(id.x) - prm.a.x;
    let sy = i32(id.y) - prm.a.y;
    let fx = prm.f.x;
    let fy = prm.f.y;
    let a = alpha_at(sx, sy, inv) * (1.0 - fx) * (1.0 - fy) + alpha_at(sx - 1, sy, inv) * fx * (1.0 - fy) + alpha_at(sx, sy - 1, inv) * (1.0 - fx) * fy + alpha_at(sx - 1, sy - 1, inv) * fx * fy;
    dst[idx(i32(id.x), i32(id.y))] = vec4<f32>(0.0, 0.0, 0.0, a);
}

@compute @workgroup_size(16, 16)
fn dilate(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= prm.size.x || id.y >= prm.size.y) { return; }
    let x = i32(id.x);
    let y = i32(id.y);
    let r = prm.a.y;
    var step = vec2<i32>(1, 0);
    if (prm.a.x == 1) { step = vec2<i32>(0, 1); }
    var m = src[idx(x, y)].w;
    var edge = 0.0;
    for (var k = 1; k <= r; k++) {
        var best = 0.0;
        let p = vec2<i32>(x, y) - step * k;
        let q = vec2<i32>(x, y) + step * k;
        if (inside(p.x, p.y)) { best = max(best, src[idx(p.x, p.y)].w); }
        if (inside(q.x, q.y)) { best = max(best, src[idx(q.x, q.y)].w); }
        if (k < r) { m = max(m, best); } else { edge = best; }
    }
    dst[idx(x, y)] = vec4<f32>(0.0, 0.0, 0.0, max(m, edge * clamp(prm.f.x, 0.0, 1.0)));
}

// dst = combination of the art (src) with the effect plane (aux .w).
@compute @workgroup_size(16, 16)
fn combine(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= prm.size.x || id.y >= prm.size.y) { return; }
    let i = idx(i32(id.x), i32(id.y));
    let p = src[i];
    let k = aux[i].w;
    if (prm.mode == 0u) {
        // The effect under the art.
        let s = prm.color * k;
        dst[i] = p + s * (1.0 - p.w);
    } else if (prm.mode == 1u) {
        // The effect inside the art, keeping its alpha.
        let s = prm.color * (k * p.w);
        dst[i] = vec4<f32>(s.xyz + p.xyz * (1.0 - s.w), p.w);
    } else {
        dst[i] = p * prm.color.w;
    }
}

@compute @workgroup_size(16, 16)
fn pack(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= prm.size.x || id.y >= prm.size.y) { return; }
    let p = clamp(src[idx(i32(id.x), i32(id.y))], vec4<f32>(0.0), vec4<f32>(1.0));
    let a = p.w;
    packed[prm.mode + idx(i32(id.x), i32(id.y))] = pack4x8unorm(vec4<f32>(min(p.xyz, vec3<f32>(a)), a));
}
