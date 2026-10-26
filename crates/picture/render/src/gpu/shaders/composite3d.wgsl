// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// Places one slice of a 3D layer on the canvas: each canvas pixel is
// mapped back through the layer's inverse homography (nothing behind the
// lens), sampled bilinearly, lit (ambient, parallel, point and spot lights
// with Blinn-Phong highlights) with shadows cast through up to four layers,
// and blended. Mirrors `CpuCompositor::draw_proj` and `space::{shade,
// shadow}` line for line. Needs `blend_common.wgsl`.

struct Comp3dParams {
    // canvas px -> picture px (homogeneous rows)
    inv0: vec4<f32>,
    inv1: vec4<f32>,
    inv2: vec4<f32>,
    // forward map's depth row: depth = dot(hw.xyz, (u, v, 1))
    hw: vec4<f32>,
    // picture px -> world: origin + u * eu + v * ev
    origin: vec4<f32>,
    eu: vec4<f32>,
    ev: vec4<f32>,
    normal: vec4<f32>,
    eye: vec4<f32>,
    // ambient, diffuse, specular, shininess
    mat: vec4<f32>,
    visible: vec4<f32>,
    canvas_size: vec2<u32>,
    opacity: f32,
    blend: u32,
    light_count: u32,
    caster_count: u32,
    lit: u32,
    side: f32,
    accepts_shadows: u32,
    // index of this layer among the casters (it never shadows itself), or 99
    self_caster: u32,
    near: f32,
    pad: u32,
}

struct LightGpu {
    // xyz position, w kind (0 parallel, 1 spot, 2 point, 3 ambient)
    pos: vec4<f32>,
    // xyz direction of travel, w casts shadows
    dir: vec4<f32>,
    // rgb colour times intensity, w shadow darkness
    colour: vec4<f32>,
    // cos outer, cos inner, radius, falloff
    cone: vec4<f32>,
}

struct CasterGpu {
    origin: vec4<f32>,
    du: vec4<f32>,
    dv: vec4<f32>,
    normal: vec4<f32>,
    visible: vec4<f32>,
}

@group(0) @binding(0) var canvas_in: texture_2d<f32>;
@group(0) @binding(1) var canvas_out: texture_storage_2d<rgba16float, write>;
@group(0) @binding(2) var layer: texture_2d<f32>;
@group(0) @binding(3) var<uniform> cp: Comp3dParams;
@group(0) @binding(4) var<storage, read> lights: array<LightGpu>;
@group(0) @binding(5) var<storage, read> casters: array<CasterGpu>;
@group(0) @binding(6) var caster0: texture_2d<f32>;
@group(0) @binding(7) var caster1: texture_2d<f32>;
@group(0) @binding(8) var caster2: texture_2d<f32>;
@group(0) @binding(9) var caster3: texture_2d<f32>;

fn fetch_layer(x: i32, y: i32) -> vec4<f32> {
    let d = vec2<i32>(textureDimensions(layer));
    if (x < 0 || y < 0 || x >= d.x || y >= d.y) { return vec4<f32>(0.0); }
    return textureLoad(layer, vec2<i32>(x, y), 0);
}

fn caster_alpha_at(i: u32, x: i32, y: i32) -> f32 {
    var d: vec2<i32>;
    switch i {
        case 0u: { d = vec2<i32>(textureDimensions(caster0)); }
        case 1u: { d = vec2<i32>(textureDimensions(caster1)); }
        case 2u: { d = vec2<i32>(textureDimensions(caster2)); }
        default: { d = vec2<i32>(textureDimensions(caster3)); }
    }
    if (x < 0 || y < 0 || x >= d.x || y >= d.y) { return 0.0; }
    let p = vec2<i32>(x, y);
    switch i {
        case 0u: { return textureLoad(caster0, p, 0).a; }
        case 1u: { return textureLoad(caster1, p, 0).a; }
        case 2u: { return textureLoad(caster2, p, 0).a; }
        default: { return textureLoad(caster3, p, 0).a; }
    }
}

fn bilinear_split(p: vec2<f32>) -> vec4<f32> {
    let q = clamp(p, vec2<f32>(-1.0e7), vec2<f32>(1.0e7)) - vec2<f32>(0.5);
    let f = floor(q);
    return vec4<f32>(f, q - f);
}

fn sample_layer3d(p: vec2<f32>) -> vec4<f32> {
    let s = bilinear_split(p);
    let xi = i32(s.x);
    let yi = i32(s.y);
    let a = fetch_layer(xi, yi);
    let b = fetch_layer(xi + 1, yi);
    let c = fetch_layer(xi, yi + 1);
    let d = fetch_layer(xi + 1, yi + 1);
    let top = a + (b - a) * s.z;
    let bottom = c + (d - c) * s.z;
    return top + (bottom - top) * s.w;
}

fn caster_alpha(i: u32, u: f32, v: f32) -> f32 {
    let s = bilinear_split(vec2<f32>(u, v));
    let xi = i32(s.x);
    let yi = i32(s.y);
    let a = caster_alpha_at(i, xi, yi);
    let b = caster_alpha_at(i, xi + 1, yi);
    let c = caster_alpha_at(i, xi, yi + 1);
    let d = caster_alpha_at(i, xi + 1, yi + 1);
    let top = a + (b - a) * s.z;
    let bottom = c + (d - c) * s.z;
    return top + (bottom - top) * s.w;
}

fn sstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = clamp((x - e0) / max(e1 - e0, 1e-6), 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
}

fn shadow(p: vec3<f32>, l: LightGpu) -> f32 {
    let kind = u32(l.pos.w + 0.5);
    var ray: vec3<f32>;
    var reach: f32;
    if (kind == 0u) {
        ray = -l.dir.xyz;
        reach = 3.0e38;
    } else {
        let d = l.pos.xyz - p;
        reach = length(d);
        ray = d / max(reach, 1e-6);
    }
    var lit = 1.0;
    for (var i = 0u; i < cp.caster_count; i = i + 1u) {
        if (i == cp.self_caster) { continue; }
        let c = casters[i];
        let denom = dot(c.normal.xyz, ray);
        if (abs(denom) < 1e-9) { continue; }
        let t = dot(c.normal.xyz, c.origin.xyz - p) / denom;
        if (t <= 0.01 || t >= reach) { continue; }
        let d = p + ray * t - c.origin.xyz;
        let u = dot(d, c.du.xyz);
        let v = dot(d, c.dv.xyz);
        if (u < c.visible.x || u > c.visible.z || v < c.visible.y || v > c.visible.w) { continue; }
        lit = lit * (1.0 - l.colour.w * clamp(caster_alpha(i, u, v), 0.0, 1.0));
    }
    return lit;
}

fn shade(px: vec4<f32>, p: vec3<f32>) -> vec4<f32> {
    let view = normalize(cp.eye.xyz - p);
    var n = cp.normal.xyz;
    if (dot(n, view) < 0.0) { n = -n; }
    var amb = vec3<f32>(0.0);
    var dif = vec3<f32>(0.0);
    var spec = vec3<f32>(0.0);
    for (var i = 0u; i < cp.light_count; i = i + 1u) {
        let l = lights[i];
        let kind = u32(l.pos.w + 0.5);
        if (kind == 3u) {
            amb = amb + l.colour.rgb;
            continue;
        }
        var to: vec3<f32>;
        var atten = 1.0;
        if (kind == 0u) {
            to = -l.dir.xyz;
        } else {
            let d = l.pos.xyz - p;
            let dist = length(d);
            if (l.cone.w > 0.0) { atten = 1.0 - sstep(l.cone.z, l.cone.z + l.cone.w, dist); }
            to = d / max(dist, 1e-6);
        }
        if (kind == 1u) {
            atten = atten * sstep(l.cone.x, l.cone.y, dot(-to, l.dir.xyz));
        }
        let ndl = dot(n, to);
        if (ndl <= 0.0 || atten <= 0.0) { continue; }
        var sh = 1.0;
        if (l.dir.w > 0.5 && cp.accepts_shadows != 0u) { sh = shadow(p, l); }
        let k = atten * sh;
        dif = dif + l.colour.rgb * (ndl * k);
        let h = normalize(to + view);
        spec = spec + l.colour.rgb * (pow(max(dot(n, h), 1e-8), cp.mat.w) * k);
    }
    let a = px.a;
    let litc = (amb * cp.mat.x + dif * cp.mat.y) * cp.side;
    return vec4<f32>(px.rgb * litc + spec * cp.mat.z * a, a);
}

@compute @workgroup_size(8, 8, 1)
fn cs_composite3d(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= cp.canvas_size.x || gid.y >= cp.canvas_size.y) { return; }
    let pos = vec2<i32>(gid.xy);
    let b = textureLoad(canvas_in, pos, 0);
    let c = vec3<f32>(f32(gid.x) + 0.5, f32(gid.y) + 0.5, 1.0);
    let q = vec3<f32>(dot(cp.inv0.xyz, c), dot(cp.inv1.xyz, c), dot(cp.inv2.xyz, c));
    let is_matte = cp.blend >= 17u && cp.blend <= 20u;
    var on = abs(q.z) > 1e-20;
    var uv = vec2<f32>(0.0);
    if (on) {
        uv = q.xy / q.z;
        on = uv.x >= cp.visible.x && uv.x <= cp.visible.z && uv.y >= cp.visible.y && uv.y <= cp.visible.w
            && dot(cp.hw.xyz, vec3<f32>(uv, 1.0)) >= cp.near;
    }
    if (!on) {
        if (is_matte) {
            textureStore(canvas_out, pos, matte(cp.blend, b, vec4<f32>(0.0)));
        } else {
            textureStore(canvas_out, pos, b);
        }
        return;
    }
    var s = sample_layer3d(uv) * cp.opacity;
    if (!is_matte) {
        if (s.a <= 0.0) {
            textureStore(canvas_out, pos, b);
            return;
        }
        if (cp.lit != 0u) {
            s = shade(s, cp.origin.xyz + cp.eu.xyz * uv.x + cp.ev.xyz * uv.y);
        } else if (cp.side != 1.0) {
            s = vec4<f32>(s.rgb * cp.side, s.a);
        }
    }
    textureStore(canvas_out, pos, composite(cp.blend, b, s));
}
