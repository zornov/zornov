// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// Instanced models: shadow, depth/normal prepass and lit passes. Needs
// common.wgsl and lighting.wgsl.

struct Mat {
    base: vec4<f32>,
    // rgb emissive, w metallic
    emissive: vec4<f32>,
    // x roughness, y subsurface, z textured, w sway metres
    params: vec4<f32>,
    // x model height (for sway), y double sided
    extra: vec4<f32>,
}

@group(1) @binding(0) var<uniform> mat: Mat;
@group(1) @binding(1) var base_tex: texture_2d<f32>;
@group(1) @binding(2) var base_samp: sampler;

struct VIn {
    @location(0) pos: vec3<f32>,
    @location(1) nrm: vec3<f32>,
    @location(2) uv: vec2<f32>,
    // position, scale
    @location(3) ia: vec4<f32>,
    // sin, cos of the turn, rank, unused
    @location(4) ib: vec4<f32>,
}

struct VOut {
    @builtin(position) @invariant clip: vec4<f32>,
    @location(0) wpos: vec3<f32>,
    @location(1) nrm: vec3<f32>,
    @location(2) uv: vec2<f32>,
}

fn turn(p: vec3<f32>, s: f32, c: f32) -> vec3<f32> {
    return vec3<f32>(p.x * c + p.z * s, p.y, -p.x * s + p.z * c);
}

fn place(v: VIn) -> vec3<f32> {
    let sc = v.ia.w;
    var w = v.ia.xyz + turn(v.pos * sc, v.ib.x, v.ib.y);
    let sway = mat.params.w;
    if (sway > 0.0) {
        let h = clamp(v.pos.y / max(mat.extra.x, 0.05), 0.0, 1.0);
        let t = fr.eye.w;
        let ph = v.ib.z * 40.0;
        let k = sway * sc * h * h;
        w.x += k * (sin(t * 1.6 + ph + w.x * 0.21) + 0.5 * sin(t * 2.9 + w.z * 0.33 + ph * 0.7));
        w.z += k * 0.6 * (sin(t * 1.3 + ph * 1.3 + w.z * 0.19));
    }
    return w;
}

@vertex
fn vs_main(v: VIn) -> VOut {
    var o: VOut;
    let w = place(v);
    o.wpos = w;
    o.nrm = turn(v.nrm, v.ib.x, v.ib.y);
    o.uv = v.uv;
    o.clip = fr.view_proj * vec4<f32>(w, 1.0);
    return o;
}

@vertex
fn vs_shadow(v: VIn) -> @builtin(position) vec4<f32> {
    return fr.view_proj * vec4<f32>(place(v), 1.0);
}

fn facing(ff: bool) -> bool {
    return select(ff, !ff, fr.flags.x > 0.5);
}

@fragment
fn fs_pre(i: VOut, @builtin(front_facing) ff: bool) -> @location(0) vec4<f32> {
    var n = normalize(i.nrm);
    if (!facing(ff)) { n = -n; }
    return vec4<f32>(n * 0.5 + vec3<f32>(0.5), 1.0);
}

@fragment
fn fs_main(i: VOut, @builtin(front_facing) ff: bool) -> @location(0) vec4<f32> {
    if (dot(vec4<f32>(i.wpos, 1.0), fr.clip) < 0.0) {
        discard;
    }
    var n = normalize(i.nrm);
    if (!facing(ff)) { n = -n; }
    let tex = textureSample(base_tex, base_samp, i.uv);
    let base = mat.base.rgb * mix(vec3<f32>(1.0), tex.rgb, mat.params.z);
    var s: Surface;
    s.pos = i.wpos;
    s.n = n;
    s.albedo = base;
    s.metal = mat.emissive.w;
    s.rough = mat.params.x;
    s.sss = mat.params.y;
    s.emissive = mat.emissive.rgb;
    s.frag = i.clip.xy;
    let uv = i.clip.xy * fr.screen.zw;
    var ao = 1.0;
    if (fr.jitter.w > 0.0) {
        ao = mix(1.0, textureSampleLevel(ao_tex, lin_samp, uv, 0.0).r, fr.jitter.w);
    }
    let col = shade(s, ao, ao);
    return vec4<f32>(apply_fog(col, i.wpos), 1.0);
}
