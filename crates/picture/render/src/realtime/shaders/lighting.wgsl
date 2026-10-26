// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// Needs common.wgsl. Resources of the lit passes and the shading itself.

@group(0) @binding(1) var shadow_tex: texture_depth_2d_array;
@group(0) @binding(2) var shadow_samp: sampler_comparison;
@group(0) @binding(3) var env_tex: texture_2d<f32>;
@group(0) @binding(4) var lin_samp: sampler;
@group(0) @binding(5) var ao_tex: texture_2d<f32>;
@group(0) @binding(6) var refl_tex: texture_2d<f32>;

var<private> POISSON: array<vec2<f32>, 12> = array<vec2<f32>, 12>(
    vec2<f32>(-0.326, -0.406), vec2<f32>(-0.840, -0.074), vec2<f32>(-0.696, 0.457), vec2<f32>(-0.203, 0.621),
    vec2<f32>(0.962, -0.195), vec2<f32>(0.473, -0.480), vec2<f32>(0.519, 0.767), vec2<f32>(0.185, -0.893),
    vec2<f32>(0.507, 0.064), vec2<f32>(0.896, 0.412), vec2<f32>(-0.322, -0.933), vec2<f32>(-0.792, -0.598));

fn dir_to_uv(d: vec3<f32>) -> vec2<f32> {
    let u = atan2(d.x, d.z) / (2.0 * PI) + 0.5;
    let v = acos(clamp(d.y, -1.0, 1.0)) / PI;
    return vec2<f32>(u, v);
}

fn env_lod(d: vec3<f32>, lod: f32) -> vec3<f32> {
    return textureSampleLevel(env_tex, lin_samp, dir_to_uv(d), lod).rgb;
}

fn sh_irradiance(n: vec3<f32>) -> vec3<f32> {
    let x = n.x;
    let y = n.y;
    let z = n.z;
    var c = fr.sh[0].rgb * 0.282095;
    c += fr.sh[1].rgb * (0.488603 * y) + fr.sh[2].rgb * (0.488603 * z) + fr.sh[3].rgb * (0.488603 * x);
    c += fr.sh[4].rgb * (1.092548 * x * y) + fr.sh[5].rgb * (1.092548 * y * z) + fr.sh[6].rgb * (0.315392 * (3.0 * z * z - 1.0));
    c += fr.sh[7].rgb * (1.092548 * x * z) + fr.sh[8].rgb * (0.546274 * (x * x - y * y));
    return max(c, vec3<f32>(0.0));
}

fn shadow_cascade(ci: i32, wp: vec3<f32>, rot: vec2<f32>) -> f32 {
    let lp = fr.cascade_vp[ci] * vec4<f32>(wp, 1.0);
    let uv = vec2<f32>(lp.x * 0.5 + 0.5, 0.5 - lp.y * 0.5);
    if (uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0 || lp.z > 1.0) {
        return 1.0;
    }
    let texel = 1.0 / f32(textureDimensions(shadow_tex).x);
    var s = 0.0;
    for (var i = 0; i < 12; i++) {
        let p = POISSON[i];
        let o = vec2<f32>(p.x * rot.x - p.y * rot.y, p.x * rot.y + p.y * rot.x) * texel * 1.6;
        s += textureSampleCompareLevel(shadow_tex, shadow_samp, uv + o, ci, lp.z - 0.0004);
    }
    return s / 12.0;
}

// Fraction of the sun that reaches `p` (1 lit).
fn sun_shadow(p: vec3<f32>, n: vec3<f32>, ndl: f32, frag: vec2<f32>) -> f32 {
    if (fr.sun_dir.w < 0.5) {
        return 1.0;
    }
    let vz = (fr.view * vec4<f32>(p, 1.0)).z;
    var ci = 0;
    if (vz > fr.cascade_splits.x) { ci = 1; }
    if (vz > fr.cascade_splits.y) { ci = 2; }
    if (vz > fr.cascade_splits.z) { ci = 3; }
    if (vz > fr.cascade_splits.w) {
        return 1.0;
    }
    let ang = hash12(frag + vec2<f32>(fr.jitter.z * 7.3, fr.jitter.z * 3.1)) * 6.2831853;
    let rot = vec2<f32>(cos(ang), sin(ang));
    var texel = fr.cascade_texel.x;
    if (ci == 1) { texel = fr.cascade_texel.y; }
    if (ci == 2) { texel = fr.cascade_texel.z; }
    if (ci == 3) { texel = fr.cascade_texel.w; }
    let wp = p + n * (texel * 2.5 * (1.0 - 0.6 * ndl) + 0.01);
    var s = shadow_cascade(ci, wp, rot);
    // Blend into the next cascade near the end of this one.
    var end = fr.cascade_splits.x;
    if (ci == 1) { end = fr.cascade_splits.y; }
    if (ci == 2) { end = fr.cascade_splits.z; }
    if (ci == 3) { end = fr.cascade_splits.w; }
    let fade = smoothstep(end * 0.9, end, vz);
    if (fade > 0.0 && ci < 3) {
        s = mix(s, shadow_cascade(ci + 1, wp, rot), fade);
    } else if (ci == 3) {
        s = mix(s, 1.0, fade);
    }
    return s;
}

fn ggx_d(ndh: f32, a: f32) -> f32 {
    let a2 = a * a;
    let d = ndh * ndh * (a2 - 1.0) + 1.0;
    return a2 / (PI * d * d + 1e-7);
}

fn smith_v(ndl: f32, ndv: f32, a: f32) -> f32 {
    let k = a * 0.5;
    return 0.25 / (max(ndl * (1.0 - k) + k, 1e-4) * max(ndv * (1.0 - k) + k, 1e-4));
}

fn fresnel(f0: vec3<f32>, vdh: f32) -> vec3<f32> {
    return f0 + (vec3<f32>(1.0) - f0) * pow(1.0 - clamp(vdh, 0.0, 1.0), 5.0);
}

fn env_brdf(f0: vec3<f32>, rough: f32, ndv: f32) -> vec3<f32> {
    let c0 = vec4<f32>(-1.0, -0.0275, -0.572, 0.022);
    let c1 = vec4<f32>(1.0, 0.0425, 1.04, -0.04);
    let r = rough * c0 + c1;
    let a004 = min(r.x * r.x, exp2(-9.28 * ndv)) * r.x + r.y;
    let ab = vec2<f32>(-1.04, 1.04) * a004 + r.zw;
    return f0 * ab.x + vec3<f32>(ab.y);
}

struct Surface {
    pos: vec3<f32>,
    n: vec3<f32>,
    albedo: vec3<f32>,
    metal: f32,
    rough: f32,
    // 0 none .. 1 leaves and skin: light through thin parts.
    sss: f32,
    emissive: vec3<f32>,
    frag: vec2<f32>,
}

// Distance fog: thicker low, thinner high; tinted by the sky at the
// horizon with a glow toward the sun.
fn fog_colour_toward(dir: vec3<f32>) -> vec3<f32> {
    if (fr.fog_colour.w > 0.5) {
        return fr.fog_colour.rgb;
    }
    let flat = normalize(vec3<f32>(dir.x, max(dir.y, 0.02), dir.z));
    return env_lod(flat, 3.0);
}

fn apply_fog(col: vec3<f32>, p: vec3<f32>) -> vec3<f32> {
    let to = p - fr.eye.xyz;
    let dist = length(to);
    let dir = to / max(dist, 1e-4);
    let k = fr.fog.y;
    let h0 = fr.eye.y;
    let h1 = p.y;
    var od = fr.fog.x * dist * exp(-k * h0);
    if (abs(h1 - h0) > 1e-3 && k > 1e-5) {
        od = fr.fog.x * dist * (exp(-k * h0) - exp(-k * h1)) / (k * (h1 - h0));
    }
    let f = 1.0 - exp(-max(od, 0.0));
    var fc = fog_colour_toward(dir);
    let sunk = pow(clamp(dot(dir, fr.sun_dir.xyz), 0.0, 1.0), 8.0);
    fc += fr.sun_colour.rgb * sunk * 0.25 * f * select(0.0, 1.0, fr.fog.z < 0.5);
    return mix(col, fc, f);
}

fn shade(s: Surface, ao: f32, ao_sun: f32) -> vec3<f32> {
    let v = normalize(fr.eye.xyz - s.pos);
    let n = s.n;
    let ndv = max(dot(n, v), 1e-3);
    let rough = clamp(s.rough, 0.04, 1.0);
    let a = rough * rough;
    let f0 = mix(vec3<f32>(0.04), s.albedo, s.metal);
    let kd = s.albedo * (1.0 - s.metal);
    var out = s.emissive;

    // The sun.
    let l = fr.sun_dir.xyz;
    let ndl = max(dot(n, l), 0.0);
    let sunc = fr.sun_colour.rgb;
    let wrap = s.sss * 0.35;
    let ndl_w = clamp((dot(n, l) + wrap) / (1.0 + wrap), 0.0, 1.0);
    if (fr.sun_colour.w > 0.0 && (ndl_w > 0.0)) {
        let sh = sun_shadow(s.pos, n, ndl, s.frag);
        let h = normalize(l + v);
        let ndh = max(dot(n, h), 0.0);
        let spec = ggx_d(ndh, a) * smith_v(ndl, ndv, a) * fresnel(f0, max(dot(v, h), 0.0)) * PI;
        let lit = sh * mix(1.0, ao_sun, 0.7);
        out += (kd * ndl_w + spec * ndl) * sunc * lit;
        // Light through leaves when the sun is behind them.
        let back = pow(clamp(dot(v, -l), 0.0, 1.0), 3.0) * s.sss * clamp(-dot(n, l) + 0.4, 0.0, 1.0);
        out += s.albedo * sunc * back * 0.6 * sh;
    }

    // The sky around.
    let irr = sh_irradiance(n);
    var amb = kd * irr * ao;
    let r = reflect(-v, n);
    let lod = rough * (fr.sky_misc.y - 1.0);
    // Reflections of the ground are shaded by the occlusion too.
    let spec_ao = clamp(pow(ndv + ao, exp2(-16.0 * rough - 1.0)) - 1.0 + ao, 0.0, 1.0);
    amb += env_lod(r, lod) * env_brdf(f0, rough, ndv) * spec_ao;
    // A hint of light bleeding through thin things.
    amb += s.albedo * irr * s.sss * 0.25 * ao;
    out += amb;

    // Lamps.
    for (var i = 0u; i < fr.counts.x; i++) {
        let lt = fr.lights[i];
        let d = lt.pos_range.xyz - s.pos;
        let d2 = max(dot(d, d), 0.01);
        let dist = sqrt(d2);
        let range = lt.pos_range.w;
        let w = clamp(1.0 - pow(dist / range, 4.0), 0.0, 1.0);
        if (w <= 0.0) { continue; }
        let ld = d / dist;
        var att = w * w / (d2 + 0.25);
        if (lt.dir_cone.w > 0.0) {
            let cd = dot(-ld, lt.dir_cone.xyz);
            att *= smoothstep(lt.dir_cone.w, mix(lt.dir_cone.w, 1.0, 0.35), cd);
        }
        let lndl = max(dot(n, ld), 0.0);
        let h = normalize(ld + v);
        let lspec = ggx_d(max(dot(n, h), 0.0), a) * smith_v(lndl, ndv, a) * fresnel(f0, max(dot(v, h), 0.0));
        out += (kd / PI + lspec) * lt.colour.rgb * att * lndl * PI * 0.5;
    }
    return out;
}
