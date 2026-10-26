// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! WGSL shader sources for the per-pixel tools and the data they need.
//!
//! This crate does not create GPU objects; `render` owns the device. It
//! gets from here:
//!
//! * [`grade_shader_source`] — compute shader `cs_grade` (workgroup 8×8).
//!   Bindings, group 0: `0` source `texture_2d<f32>` (linear premultiplied
//!   RGBA), `1` destination `texture_storage_2d<rgba16float, write>`,
//!   `2` uniform [`pack_grade`] (`GRADE_PARAMS_VEC4S` × `vec4<f32>`),
//!   `3` `texture_3d<f32>` 3D LUT (rgba32float, size³; 1×1×1 dummy when
//!   unused), `4` `texture_2d<f32>` 1D LUT (size × 1; dummy when unused).
//!   Use [`lut3d_texels`] / [`lut1d_texels`] to fill the LUT textures.
//! * [`scopes_shader_source`] — `cs_downscale` (group 1) and `cs_scopes`
//!   (group 0); see the comment at the top of `shaders/scopes.wgsl` and
//!   [`pack_scope_params`].
//!
//! The spatial tools (dehaze, clarity, sharpen) are not in the shader:
//! run them on the CPU ([`crate::spatial`]) or with render's own blur
//! passes, in the order documented in [`crate::grade`].

use crate::grade::{FrameInfo, Grade, GradeProcessor};
use crate::lut::{Lut1d, Lut3d};
use crate::math::{Mat3, Rgb};
use crate::scopes::ScopeSettings;
use crate::spline::{Extrapolate, MAX_KNOTS, Spline};
use crate::wheels::WheelMode;

pub const TRANSFER_WGSL: &str = include_str!("../shaders/transfer.wgsl");
pub const GRADE_WGSL: &str = include_str!("../shaders/grade.wgsl");
pub const SCOPES_WGSL: &str = include_str!("../shaders/scopes.wgsl");
/// `camera_decode(id, v)`: camera log code value → scene linear (ids from
/// [`crate::LogCurve::shader_id`]). Prepended to render's convert shader.
pub const CAMERA_WGSL: &str = include_str!("../shaders/camera.wgsl");

/// Number of vec4 in the grade uniform (must match `P_COUNT` in WGSL).
pub const GRADE_PARAMS_VEC4S: usize = 228;
const P_CURVES: usize = 57;
const CURVE_STRIDE: usize = 1 + MAX_KNOTS;

/// Complete grade compute shader.
pub fn grade_shader_source() -> String {
    format!("{TRANSFER_WGSL}\n{GRADE_WGSL}")
}

/// Complete scopes compute shader.
pub fn scopes_shader_source() -> String {
    format!("{TRANSFER_WGSL}\n{SCOPES_WGSL}")
}

fn flag(b: bool) -> f32 {
    if b { 1.0 } else { 0.0 }
}

fn put_mat(v: &mut [[f32; 4]], at: usize, m: &Mat3) {
    let f = m.to_f32();
    for r in 0..3 {
        v[at + r] = [f[r][0], f[r][1], f[r][2], 0.0];
    }
}

fn v3(c: Rgb, w: f32) -> [f32; 4] {
    [c[0], c[1], c[2], w]
}

fn put_curve(v: &mut [[f32; 4]], index: usize, s: &Spline) {
    let base = P_CURVES + index * CURVE_STRIDE;
    let mode = match s.extrapolate {
        Extrapolate::Linear => 0.0,
        Extrapolate::Clamp => 1.0,
        Extrapolate::Periodic => 2.0,
    };
    let n = s.xs.len().min(MAX_KNOTS);
    v[base] = [n as f32, mode, 0.0, 0.0];
    for k in 0..n {
        v[base + 1 + k] = [s.xs[k], s.ys[k], s.ms[k], 0.0];
    }
}

/// Packs a compiled grade into the uniform array read by `cs_grade`.
/// `width` / `height` are the frame size (for the vignette); `info`
/// animates grain.
pub fn pack_grade(grade: &Grade, p: &GradeProcessor, width: u32, height: u32, info: FrameInfo) -> Vec<[f32; 4]> {
    let mut v = vec![[0.0f32; 4]; GRADE_PARAMS_VEC4S];
    let b = &p.basic;
    v[0] = [flag(!p.levels.is_identity()), flag(p.colour_match.is_some()), flag(p.linear.is_some()), flag(p.grading)];
    v[1] = [
        flag(!b.tone_is_identity()),
        flag(!p.wheels.identity),
        flag(!p.curves.tone_identity),
        flag(!p.curves.colour_identity),
    ];
    v[2] = [
        flag(!p.hsl.is_identity()),
        flag(!b.colour_is_identity()),
        flag(p.lut.is_some()),
        flag(p.wheels.mode == WheelMode::ShadowsMidtonesHighlights),
    ];
    v[3] = [p.levels.black, 1.0 / (p.levels.white - p.levels.black).max(1e-6), 0.0, 0.0];
    if let Some(m) = &p.colour_match {
        put_mat(&mut v, 4, &m.to709);
        put_mat(&mut v, 7, &m.from709);
        v[10] = v3(m.src_mean, m.strength);
        v[11] = v3(m.ref_mean, 0.0);
        v[12] = v3(m.scale, 0.0);
    }
    put_mat(&mut v, 13, &p.linear.unwrap_or(Mat3::IDENTITY));
    put_mat(&mut v, 16, &p.space.to_ap1);
    put_mat(&mut v, 19, &p.space.from_ap1);
    v[22] = [b.contrast.exp2(), b.highlights, b.shadows, b.whites];
    v[23] = [b.blacks, b.fade, b.saturation, b.vibrance];
    let cdl = &p.wheels.cdl;
    v[24] = v3(cdl.slope, p.wheels.intensity);
    v[25] = v3(cdl.offset, 0.0);
    v[26] = v3(cdl.power, 0.0);
    for k in 0..3 {
        v[27 + k] = v3(p.wheels.smh[k], 0.0);
    }
    for (i, band) in p.hsl.bands.iter().enumerate() {
        v[30 + i] = [band.hue, band.saturation, band.lightness, 0.0];
    }
    if let Some(l) = &p.lut {
        let s3 = l.lut.lut3d.as_ref().map(|t| t.size).unwrap_or(0);
        let s1 = l.lut.lut1d.as_ref().map(|t| t.data.len()).unwrap_or(0);
        v[38] = [l.intensity, l.skin_protect, l.transfer.shader_id() as f32, s3 as f32];
        v[39] = [s1 as f32, 0.0, 0.0, 0.0];
        if let Some(t) = &l.lut.lut3d {
            v[40] = v3(t.domain_min, 0.0);
            v[41] = v3(t.domain_max, 0.0);
        }
        if let Some(t) = &l.lut.lut1d {
            v[42] = v3(t.domain_min, 0.0);
            v[43] = v3(t.domain_max, 0.0);
        }
        put_mat(&mut v, 44, &l.to_lut);
        put_mat(&mut v, 47, &l.from_lut);
        put_mat(&mut v, 50, &l.to_709);
    }
    let vg = &grade.basic.vignette;
    v[53] = [vg.amount, vg.midpoint, vg.roundness, vg.feather];
    v[54] = [vg.shape, width as f32, height as f32, 0.0];
    let g = &grade.basic.grain;
    let seed = crate::spatial::grain_seed(g, info.frame);
    v[55] = [g.amount, g.size, g.roughness, f32::from_bits(seed)];
    v[56] = v3(p.luma_weights, 0.0);
    for (i, s) in p.curves.tone.iter().enumerate() {
        put_curve(&mut v, i, s);
    }
    for (i, s) in p.curves.hue.iter().enumerate() {
        put_curve(&mut v, 4 + i, s);
    }
    put_curve(&mut v, 7, &p.curves.lum_sat);
    put_curve(&mut v, 8, &p.curves.sat_sat);
    v
}

/// RGBA texels for a 3D LUT texture (x = red, y = green, z = blue).
pub fn lut3d_texels(l: &Lut3d) -> Vec<[f32; 4]> {
    l.data.iter().map(|c| [c[0], c[1], c[2], 1.0]).collect()
}

/// RGBA texels for a 1D LUT texture of `size × 1`.
pub fn lut1d_texels(l: &Lut1d) -> Vec<[f32; 4]> {
    l.data.iter().map(|c| [c[0], c[1], c[2], 1.0]).collect()
}

/// The three vec4 of `ScopeParams`, as raw 32-bit words.
pub fn pack_scope_params(s: &ScopeSettings, downscaled_width: u32) -> [[u32; 4]; 3] {
    let lw = s.matrix.luma_weights();
    let (kr, kb) = s.matrix.kr_kb();
    [
        [s.transfer.shader_id(), s.levels, s.vectorscope_size, downscaled_width],
        [lw[0].to_bits(), lw[1].to_bits(), lw[2].to_bits(), (2.0 * (1.0 - kb)).to_bits()],
        [(2.0f32 * (1.0 - kr)).to_bits(), 0, 0, 0],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn validate(src: &str) {
        let module = naga::front::wgsl::parse_str(src).unwrap_or_else(|e| panic!("{}", e.emit_to_string(src)));
        let mut v = naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all());
        if let Err(e) = v.validate(&module) {
            panic!("{}", e.emit_to_string(src));
        }
    }

    #[test]
    fn shaders_validate() {
        validate(&grade_shader_source());
        validate(&scopes_shader_source());
        validate(CAMERA_WGSL);
        // One case per curve, in shader-id order.
        for c in crate::LogCurve::ALL {
            let id = c.shader_id();
            assert!(id == 15 || CAMERA_WGSL.contains(&format!("case {id}u:")), "{c:?}");
        }
    }

    #[test]
    fn param_count_matches_shader() {
        assert!(GRADE_WGSL.contains(&format!("array<vec4<f32>, {GRADE_PARAMS_VEC4S}>")));
        assert!(GRADE_WGSL.contains(&format!("P_COUNT: u32 = {GRADE_PARAMS_VEC4S}u")));
        assert_eq!(P_CURVES + 9 * CURVE_STRIDE, GRADE_PARAMS_VEC4S);
        let g = Grade::default();
        let v = pack_grade(&g, &g.prepare(), 1920, 1080, FrameInfo::default());
        assert_eq!(v.len(), GRADE_PARAMS_VEC4S);
        // Default grade: every stage flag is off.
        assert_eq!(&v[0..3], &[[0.0; 4]; 3]);
    }
}
