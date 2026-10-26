// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// Terrain chunks painted from a layer-weight map with procedural detail.
// Needs common.wgsl and lighting.wgsl.

struct Layers {
    // rgb colour, w roughness
    colour: array<vec4<f32>, 4>,
    // x metres per repeat of the detail pattern (four layers)
    tile: vec4<f32>,
    // xy world origin of the weight map, zw size
    map: vec4<f32>,
}

@group(1) @binding(0) var splat_tex: texture_2d<f32>;
@group(1) @binding(1) var<uniform> layers: Layers;

struct VIn {
    @location(0) pos: vec3<f32>,
    @location(1) nrm: vec3<f32>,
}

struct VOut {
    @builtin(position) @invariant clip: vec4<f32>,
    @location(0) wpos: vec3<f32>,
    @location(1) nrm: vec3<f32>,
}

@vertex
fn vs_main(v: VIn) -> VOut {
    var o: VOut;
    o.wpos = v.pos;
    o.nrm = v.nrm;
    o.clip = fr.view_proj * vec4<f32>(v.pos, 1.0);
    return o;
}

@vertex
fn vs_shadow(v: VIn) -> @builtin(position) vec4<f32> {
    return fr.view_proj * vec4<f32>(v.pos, 1.0);
}

@fragment
fn fs_pre(i: VOut) -> @location(0) vec4<f32> {
    let n = normalize(i.nrm);
    return vec4<f32>(n * 0.5 + vec3<f32>(0.5), 1.0);
}

@fragment
fn fs_main(i: VOut) -> @location(0) vec4<f32> {
    if (dot(vec4<f32>(i.wpos, 1.0), fr.clip) < 0.0) {
        discard;
    }
    let dims = vec2<f32>(textureDimensions(splat_tex));
    let uv = ((i.wpos.xz - layers.map.xy) / layers.map.zw * (dims - vec2<f32>(1.0)) + vec2<f32>(0.5)) / dims;
    var w = textureSampleLevel(splat_tex, lin_samp, uv, 0.0);
    w = w / max(w.x + w.y + w.z + w.w, 1e-3);
    var albedo = vec3<f32>(0.0);
    var rough = 0.0;
    for (var k = 0; k < 4; k++) {
        let wk = w[k];
        if (wk < 0.004) { continue; }
        let tile = layers.tile[k];
        let p = i.wpos.xz / tile;
        let d = 0.62 + 0.55 * (0.5 * vnoise(p) + 0.3 * vnoise(p * 4.7 + vec2<f32>(3.1, 1.7)) + 0.2 * vnoise(p * 17.3 + vec2<f32>(9.9, 4.2)));
        albedo += layers.colour[k].rgb * d * wk;
        rough += layers.colour[k].w * wk;
    }
    // Large patches of tint so no stretch looks the same as its neighbour.
    let tint = 0.88 + 0.24 * fbm2(i.wpos.xz * 0.045);
    albedo *= tint;
    // Small bumps tilt the normal.
    let e = 0.35;
    let bx = vnoise((i.wpos.xz + vec2<f32>(e, 0.0)) * 2.3) - vnoise((i.wpos.xz - vec2<f32>(e, 0.0)) * 2.3);
    let bz = vnoise((i.wpos.xz + vec2<f32>(0.0, e)) * 2.3) - vnoise((i.wpos.xz - vec2<f32>(0.0, e)) * 2.3);
    let n = normalize(normalize(i.nrm) + vec3<f32>(-bx, 0.0, -bz) * 0.28);
    var s: Surface;
    s.pos = i.wpos;
    s.n = n;
    s.albedo = albedo;
    s.metal = 0.0;
    s.rough = rough;
    s.sss = 0.0;
    s.emissive = vec3<f32>(0.0);
    s.frag = i.clip.xy;
    let suv = i.clip.xy * fr.screen.zw;
    var ao = 1.0;
    if (fr.jitter.w > 0.0) {
        ao = mix(1.0, textureSampleLevel(ao_tex, lin_samp, suv, 0.0).r, fr.jitter.w);
    }
    let col = shade(s, ao, ao);
    return vec4<f32>(apply_fog(col, i.wpos), 1.0);
}
