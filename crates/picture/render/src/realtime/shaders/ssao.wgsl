// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// Screen-space ambient occlusion from the depth and normal prepass, then a
// depth-aware blur. Needs common.wgsl.

@group(1) @binding(0) var depth_tex: texture_depth_2d;
@group(1) @binding(1) var nrm_tex: texture_2d<f32>;
@group(1) @binding(2) var ao_in: texture_2d<f32>;

struct VOut {
    @builtin(position) clip: vec4<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> VOut {
    var o: VOut;
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    o.clip = vec4<f32>(p * 2.0 - vec2<f32>(1.0), 0.0, 1.0);
    return o;
}

fn unproject(px: vec2<f32>, size: vec2<f32>, d: f32) -> vec3<f32> {
    let ndc = vec2<f32>(px.x / size.x * 2.0 - 1.0, 1.0 - px.y / size.y * 2.0);
    let h = fr.inv_view_proj * vec4<f32>(ndc, d, 1.0);
    return h.xyz / h.w;
}

@fragment
fn fs_ao(i: VOut) -> @location(0) vec4<f32> {
    let full = vec2<f32>(textureDimensions(depth_tex));
    let half_size = vec2<f32>(textureDimensions(ao_in));
    let px = vec2<i32>(i.clip.xy * (full / half_size));
    let d = textureLoad(depth_tex, px, 0);
    if (d >= 1.0) {
        return vec4<f32>(1.0);
    }
    let pxf = vec2<f32>(px) + vec2<f32>(0.5);
    let wp = unproject(pxf, full, d);
    let n = normalize(textureLoad(nrm_tex, px, 0).xyz * 2.0 - vec3<f32>(1.0));
    let eye = fr.eye.xyz;
    let dist = length(wp - eye);
    let radius = clamp(0.5 + dist * 0.02, 0.5, 2.5);
    let ang = hash12(i.clip.xy + vec2<f32>(fr.jitter.z * 5.7, fr.jitter.z * 2.3)) * 6.2831853;
    var up = vec3<f32>(0.0, 1.0, 0.0);
    if (abs(n.y) > 0.9) { up = vec3<f32>(1.0, 0.0, 0.0); }
    let t = normalize(cross(n, up));
    let b = cross(n, t);
    var occ = 0.0;
    let count = 14;
    for (var k = 0; k < count; k++) {
        let u = (f32(k) + 0.5) / f32(count);
        let a = f32(k) * 2.39996 + ang;
        let ct = sqrt(1.0 - u * 0.92);
        let st = sqrt(1.0 - ct * ct);
        let dir = t * (cos(a) * st) + b * (sin(a) * st) + n * ct;
        let sc = radius * (0.12 + 0.88 * u * u);
        let sp = wp + dir * sc;
        let clip = fr.view_proj * vec4<f32>(sp, 1.0);
        if (clip.w <= 0.0) { continue; }
        let ndc = clip.xy / clip.w;
        let sx = vec2<f32>((ndc.x * 0.5 + 0.5) * full.x, (0.5 - ndc.y * 0.5) * full.y);
        if (sx.x < 0.0 || sx.y < 0.0 || sx.x >= full.x || sx.y >= full.y) { continue; }
        let sd = textureLoad(depth_tex, vec2<i32>(sx), 0);
        if (sd >= 1.0) { continue; }
        let sw = unproject(sx, full, sd);
        let sample_d = length(sp - eye);
        let scene_d = length(sw - eye);
        let diff = sample_d - scene_d;
        let range = smoothstep(0.0, 1.0, radius / max(abs(dist - scene_d), 1e-3));
        if (diff > 0.02 * radius && diff < radius * 1.5) {
            occ += range;
        }
    }
    let ao = clamp(1.0 - occ / f32(count) * 1.35, 0.0, 1.0);
    return vec4<f32>(ao, ao, ao, 1.0);
}

@fragment
fn fs_blur(i: VOut) -> @location(0) vec4<f32> {
    let size = vec2<i32>(textureDimensions(ao_in));
    let c = vec2<i32>(i.clip.xy);
    let full = vec2<f32>(textureDimensions(depth_tex));
    let scale = full / vec2<f32>(size);
    let d0 = textureLoad(depth_tex, vec2<i32>(vec2<f32>(c) * scale), 0);
    var sum = 0.0;
    var wsum = 0.0;
    for (var y = -2; y <= 2; y++) {
        for (var x = -2; x <= 2; x++) {
            let q = clamp(c + vec2<i32>(x, y), vec2<i32>(0), size - vec2<i32>(1));
            let dq = textureLoad(depth_tex, vec2<i32>(vec2<f32>(q) * scale), 0);
            let w = exp(-abs(dq - d0) * 400.0) * (1.0 - 0.12 * f32(abs(x) + abs(y)));
            sum += textureLoad(ao_in, q, 0).r * w;
            wsum += w;
        }
    }
    let ao = sum / max(wsum, 1e-4);
    return vec4<f32>(ao, ao, ao, 1.0);
}
