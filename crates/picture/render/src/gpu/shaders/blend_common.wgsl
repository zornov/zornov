// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

// Premultiplied blending with the W3C Compositing and Blending formulas and
// the track-matte modes, shared by the 2D and 3D composite passes. Mirrors
// `blend.rs`.

fn unpremultiply(p: vec4<f32>) -> vec4<f32> {
    if (p.a <= 0.0) { return vec4<f32>(0.0); }
    return vec4<f32>(p.rgb / p.a, p.a);
}

fn screen_c(cb: f32, cs: f32) -> f32 { return cb + cs - cb * cs; }

fn hard_light(cb: f32, cs: f32) -> f32 {
    if (cs <= 0.5) { return cb * 2.0 * cs; }
    return screen_c(cb, 2.0 * cs - 1.0);
}

fn soft_light(cb: f32, cs: f32) -> f32 {
    if (cs <= 0.5) { return cb - (1.0 - 2.0 * cs) * cb * (1.0 - cb); }
    var d: f32;
    if (cb <= 0.25) { d = ((16.0 * cb - 12.0) * cb + 4.0) * cb; } else { d = sqrt(cb); }
    return cb + (2.0 * cs - 1.0) * (d - cb);
}

fn colour_dodge(cb: f32, cs: f32) -> f32 {
    if (cb <= 0.0) { return 0.0; }
    if (cs >= 1.0) { return 1.0; }
    return min(cb / (1.0 - cs), 1.0);
}

fn colour_burn(cb: f32, cs: f32) -> f32 {
    if (cb >= 1.0) { return 1.0; }
    if (cs <= 0.0) { return 0.0; }
    return 1.0 - min((1.0 - cb) / cs, 1.0);
}

fn blend_channel(mode: u32, cb: f32, cs: f32) -> f32 {
    switch mode {
        case 1u: { return cb + cs; }
        case 2u: { return cb * cs; }
        case 3u: { return screen_c(cb, cs); }
        case 4u: { return hard_light(cs, cb); }
        case 5u: { return soft_light(cb, cs); }
        case 6u: { return hard_light(cb, cs); }
        case 7u: { return colour_dodge(cb, cs); }
        case 8u: { return colour_burn(cb, cs); }
        case 9u: { return min(cb, cs); }
        case 10u: { return max(cb, cs); }
        case 11u: { return abs(cb - cs); }
        case 12u: { return cb + cs - 2.0 * cb * cs; }
        default: { return cs; }
    }
}

fn lum(c: vec3<f32>) -> f32 { return 0.3 * c.r + 0.59 * c.g + 0.11 * c.b; }

fn clip_colour(c0: vec3<f32>) -> vec3<f32> {
    var c = c0;
    let l = lum(c);
    let n = min(c.r, min(c.g, c.b));
    let x = max(c.r, max(c.g, c.b));
    if (n < 0.0) { c = l + (c - l) * l / max(l - n, 1e-9); }
    if (x > 1.0) { c = l + (c - l) * (1.0 - l) / max(x - l, 1e-9); }
    return c;
}

fn set_lum(c: vec3<f32>, l: f32) -> vec3<f32> {
    let d = l - lum(c);
    return clip_colour(c + vec3<f32>(d));
}

fn sat(c: vec3<f32>) -> f32 { return max(c.r, max(c.g, c.b)) - min(c.r, min(c.g, c.b)); }

fn set_sat(c: vec3<f32>, s: f32) -> vec3<f32> {
    var imax = 0u; var imid = 1u; var imin = 2u;
    if (c[imax] < c[imid]) { let t = imax; imax = imid; imid = t; }
    if (c[imid] < c[imin]) { let t = imid; imid = imin; imin = t; }
    if (c[imax] < c[imid]) { let t = imax; imax = imid; imid = t; }
    var out = vec3<f32>(0.0);
    let range = c[imax] - c[imin];
    if (range > 0.0) {
        out[imid] = (c[imid] - c[imin]) * s / range;
        out[imax] = s;
    }
    return out;
}

fn blend_rgb(mode: u32, cb: vec3<f32>, cs: vec3<f32>) -> vec3<f32> {
    switch mode {
        case 13u: { return set_lum(set_sat(cs, sat(cb)), lum(cb)); }
        case 14u: { return set_lum(set_sat(cb, sat(cs)), lum(cb)); }
        case 15u: { return set_lum(cs, lum(cb)); }
        case 16u: { return set_lum(cb, lum(cs)); }
        default: {
            return vec3<f32>(blend_channel(mode, cb.r, cs.r), blend_channel(mode, cb.g, cs.g), blend_channel(mode, cb.b, cs.b));
        }
    }
}

// Track mattes (17-20): the canvas kept by how much the matte covers it.
fn matte(mode: u32, b: vec4<f32>, s: vec4<f32>) -> vec4<f32> {
    let a = clamp(s.a, 0.0, 1.0);
    var k = a;
    if (mode == 18u) {
        k = 1.0 - a;
    } else if (mode >= 19u) {
        let u = unpremultiply(s);
        let l = clamp(0.2126 * u.r + 0.7152 * u.g + 0.0722 * u.b, 0.0, 1.0) * a;
        k = select(1.0 - l, l, mode == 19u);
    }
    return b * k;
}

fn composite(mode: u32, b: vec4<f32>, s: vec4<f32>) -> vec4<f32> {
    if (mode >= 17u && mode <= 20u) { return matte(mode, b, s); }
    let ab = b.a;
    let as_ = s.a;
    if (as_ <= 0.0) { return b; }
    if (mode == 0u) {
        return vec4<f32>(s.rgb + b.rgb * (1.0 - as_), as_ + ab * (1.0 - as_));
    }
    let ub = unpremultiply(b);
    let us = unpremultiply(s);
    var blended: vec3<f32>;
    if (mode >= 13u) {
        blended = blend_rgb(mode, clamp(ub.rgb, vec3<f32>(0.0), vec3<f32>(1.0)), clamp(us.rgb, vec3<f32>(0.0), vec3<f32>(1.0)));
    } else {
        blended = blend_rgb(mode, ub.rgb, us.rgb);
    }
    let k = as_ * ab;
    return vec4<f32>(s.rgb * (1.0 - ab) + b.rgb * (1.0 - as_) + k * blended, as_ + ab * (1.0 - as_));
}
