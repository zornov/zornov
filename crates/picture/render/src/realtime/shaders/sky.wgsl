// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// The sky behind everything: the daylight environment map, the sun disk,
// and a layer of clouds. Needs common.wgsl and lighting.wgsl.

struct VOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) ndc: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> VOut {
    var o: VOut;
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    o.clip = vec4<f32>(p * 2.0 - vec2<f32>(1.0), 1.0, 1.0);
    o.ndc = p * 2.0 - vec2<f32>(1.0);
    return o;
}

@fragment
fn fs_main(i: VOut) -> @location(0) vec4<f32> {
    let h = fr.inv_view_proj * vec4<f32>(i.ndc, 1.0, 1.0);
    var dir = normalize(h.xyz / h.w - fr.view_eye.xyz);
    if (fr.flags.x > 0.5) { dir.y = -dir.y; }
    var col = env_lod(dir, 0.0);
    let sun = fr.sun_dir.xyz;
    let cs = dot(dir, sun);
    // The sun's disk and its halo.
    if (fr.sun_colour.w > 0.0) {
        let disk = smoothstep(0.99985, 0.99996, cs);
        let halo = pow(max(cs, 0.0), 220.0) * 0.5;
        col += fr.sun_colour.rgb * (disk * 60.0 + halo) * clamp(dir.y * 8.0 + 0.4, 0.0, 1.0);
    }
    // Clouds on a plane high above.
    let cover = fr.sky_misc.x;
    if (dir.y > 0.01 && cover > 0.02) {
        let t = fr.eye.w * 0.004;
        let q = (dir.xz / (dir.y + 0.15)) * 0.7 + vec2<f32>(t, t * 0.4);
        let n = fbm2(q * 1.5) * 0.75 + 0.25 * vnoise(q * 9.0);
        let dens = smoothstep(0.62 - cover * 0.32, 0.9 - cover * 0.25, n) * clamp(dir.y * 5.0, 0.0, 1.0);
        let lit = fr.sun_colour.rgb * (0.5 + 0.5 * clamp(cs, 0.0, 1.0)) * 0.55 + sh_irradiance(vec3<f32>(0.0, 1.0, 0.0)) * 0.9;
        col = mix(col, lit * vec3<f32>(0.95, 0.96, 1.0), dens * 0.85);
    }
    return vec4<f32>(col, 1.0);
}
