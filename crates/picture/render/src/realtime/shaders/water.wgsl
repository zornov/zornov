// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// Water surfaces: waves, depth-tinted refraction of what is below,
// planar reflections of what is above, fresnel, a glint of sun, foam on
// the shore. Needs common.wgsl and lighting.wgsl.

struct Water {
    // level, wave height, wave speed, clarity
    a: vec4<f32>,
    // min x, min z, max x, max z
    bounds: vec4<f32>,
    // rgb deep colour, w 1 when the reflection map is valid
    colour: vec4<f32>,
}

@group(1) @binding(0) var<uniform> wat: Water;
@group(1) @binding(1) var scene_depth: texture_depth_2d;
@group(1) @binding(2) var scene_col: texture_2d<f32>;

struct VIn {
    @location(0) uv: vec2<f32>,
}

struct VOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) wpos: vec3<f32>,
}

fn big_waves(p: vec2<f32>, t: f32) -> f32 {
    let d1 = vec2<f32>(0.8, 0.6);
    let d2 = vec2<f32>(-0.5, 0.86);
    let d3 = vec2<f32>(0.2, -0.98);
    return 0.5 * sin(dot(p, d1) * 0.31 + t * 1.1) + 0.3 * sin(dot(p, d2) * 0.77 + t * 1.7) + 0.2 * sin(dot(p, d3) * 1.63 + t * 2.3);
}

@vertex
fn vs_main(v: VIn) -> VOut {
    var o: VOut;
    var xz = mix(wat.bounds.xy, wat.bounds.zw, v.uv);
    let t = fr.eye.w * wat.a.z;
    let h = wat.a.y * big_waves(xz, t);
    o.wpos = vec3<f32>(xz.x, wat.a.x + h, xz.y);
    o.clip = fr.view_proj * vec4<f32>(o.wpos, 1.0);
    return o;
}

fn ripple(p: vec2<f32>, t: f32) -> f32 {
    return vnoise(p * 1.7 + vec2<f32>(t * 0.35, t * 0.2)) * 0.6 + vnoise(p * 4.1 - vec2<f32>(t * 0.5, -t * 0.3)) * 0.3 + vnoise(p * 9.0 + vec2<f32>(t * 0.9, t * 0.7)) * 0.1;
}

fn water_normal(p: vec2<f32>, t: f32, dist: f32, amp: f32) -> vec3<f32> {
    let e = 0.06;
    let r0 = ripple(p, t);
    let rx = ripple(p + vec2<f32>(e, 0.0), t) - r0;
    let rz = ripple(p + vec2<f32>(0.0, e), t) - r0;
    // Large waves by their slope.
    let e2 = 0.5;
    let b0 = big_waves(p, t);
    let bx = (big_waves(p + vec2<f32>(e2, 0.0), t) - b0) / e2 * wat.a.y;
    let bz = (big_waves(p + vec2<f32>(0.0, e2), t) - b0) / e2 * wat.a.y;
    let fade = 1.0 / (1.0 + dist * 0.015);
    let k = (0.35 + 0.65 * min(amp * 4.0, 1.0)) * fade;
    return normalize(vec3<f32>(-(rx / e) * 0.5 * k - bx, 1.0, -(rz / e) * 0.5 * k - bz));
}

fn world_at(ndc_xy: vec2<f32>, d: f32) -> vec3<f32> {
    let h = fr.inv_view_proj * vec4<f32>(ndc_xy, d, 1.0);
    return h.xyz / h.w;
}

@fragment
fn fs_main(i: VOut, @builtin(front_facing) ff: bool) -> @location(0) vec4<f32> {
    if (fr.fog.z > 0.5) {
        discard;
    }
    let t = fr.eye.w * wat.a.z;
    let eye = fr.eye.xyz;
    let to = i.wpos - eye;
    let dist = length(to);
    let v = -to / max(dist, 1e-4);
    let n = water_normal(i.wpos.xz, t, dist, max(wat.a.y, 0.03));
    let ndv = clamp(dot(n, v), 0.0, 1.0);
    let size = vec2<f32>(textureDimensions(scene_depth));
    let px = vec2<i32>(i.clip.xy);
    let uv = i.clip.xy / size;
    let sd = textureLoad(scene_depth, px, 0);
    let ndc = vec2<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0);
    var scene_world = world_at(ndc, sd);
    var thick = max(length(scene_world - eye) - dist, 0.0);
    if (sd >= 1.0) {
        thick = 60.0;
    }
    // Refraction: shift what is seen, but never pull in things nearer than the water.
    let shift = n.xz * 0.05 * clamp(thick * 0.5, 0.0, 1.0) * size.y * 0.02;
    var px2 = clamp(px + vec2<i32>(i32(shift.x), i32(shift.y)), vec2<i32>(0), vec2<i32>(size) - vec2<i32>(1));
    let sd2 = textureLoad(scene_depth, px2, 0);
    if (sd2 > sd || sd2 < i.clip.z) {
        // Offset found something in front or sky behind: keep the straight view.
        if (sd2 < i.clip.z) { px2 = px; }
    }
    let below = textureLoad(scene_col, px2, 0).rgb;
    let clarity = max(wat.a.w, 0.2);
    let ext = vec3<f32>(1.0, 0.32, 0.18) * (2.6 / clarity);
    let tr = exp(-ext * thick);
    let irr_up = sh_irradiance(vec3<f32>(0.0, 1.0, 0.0)) + fr.sun_colour.rgb * max(fr.sun_dir.y, 0.0) * 0.5;
    let body = wat.colour.rgb * irr_up;
    let refracted = below * tr + body * (vec3<f32>(1.0) - tr);
    // Reflection.
    var refl: vec3<f32>;
    let rd = reflect(-v, n);
    if (wat.colour.w > 0.5) {
        let ruv = clamp(uv + n.xz * vec2<f32>(0.03, -0.03) * (0.4 + 0.6 * ndv), vec2<f32>(0.002), vec2<f32>(0.998));
        refl = textureSampleLevel(refl_tex, lin_samp, ruv, 0.0).rgb;
    } else {
        refl = env_lod(vec3<f32>(rd.x, abs(rd.y), rd.z), 0.5);
    }
    let f = 0.02 + 0.98 * pow(1.0 - ndv, 5.0);
    var col = mix(refracted, refl, f);
    // The sun on the water.
    if (fr.sun_colour.w > 0.0) {
        let l = fr.sun_dir.xyz;
        let h = normalize(l + v);
        let ndh = max(dot(n, h), 0.0);
        let a = 0.09;
        let spec = ggx_d(ndh, a) * smith_v(max(dot(n, l), 0.0), ndv, a) * f * PI * max(dot(n, l), 0.0);
        col += fr.sun_colour.rgb * spec;
    }
    // Foam where it shallows.
    let depth_v = max(wat.a.x - scene_world.y, 0.0);
    var foam = 0.0;
    if (sd < 1.0) {
        let edge = 1.0 - smoothstep(0.0, 0.55, depth_v);
        let pat = vnoise(i.wpos.xz * 3.2 + vec2<f32>(t * 0.6, -t * 0.4)) * 0.6 + vnoise(i.wpos.xz * 8.0 - vec2<f32>(t * 0.9, t * 0.5)) * 0.4;
        foam = clamp(edge * smoothstep(0.35, 0.75, pat + edge * 0.35), 0.0, 1.0);
    }
    col = mix(col, vec3<f32>(0.9, 0.95, 1.0) * (irr_up * 0.7 + fr.sun_colour.rgb * max(fr.sun_dir.y, 0.0) * 0.5 + vec3<f32>(0.1)), foam * 0.8);
    return vec4<f32>(apply_fog(col, i.wpos), 1.0);
}
