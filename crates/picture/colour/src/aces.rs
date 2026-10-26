// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! ACES 1.3 output transforms (RRT + ODT) and their SDR inverse.
//!
//! A transcription of the Academy's reference CTL (aces-dev v1.3:
//! `RRT.ctl`, `ACESlib.RRT_Common`, `ACESlib.Tonescales`, `ACESlib.SSTS`,
//! `ACESlib.ODT_Common`, `ACESlib.OutputTransforms` and the
//! `ODT.Academy.Rec709_100nits_dim` / `RRTODT.Academy.Rec2020_*nits_15nits`
//! transforms). Copyright © 2015 Academy of Motion Picture Arts and
//! Sciences, used under the ACES licence (see NOTICE).
//!
//! * SDR: RRT, then the 48-nit cinema tone scale rescaled to the display
//!   (dim surround, 0.93 saturation) — the classic Rec.709 / sRGB ODT,
//!   also offered for P3-D65 and Rec.2020 primaries.
//! * HDR: RRT sweeteners and the single-stage tone scale (SSTS) for
//!   0.0001–1000/2000/4000 cd/m² with 18 % grey at 15 cd/m², limited to
//!   Rec.2020 (PQ or HLG).
//!
//! Studio's working space is scene-linear BT.709 with D65 white, so
//! [`DisplayTarget::view`] first converts to ACES2065-1 (AP0, Bradford
//! adaptation to the ACES white) and returns display light back in working
//! primaries, scaled the way render's output encoder expects (1.0 is SDR
//! display white, or 203 cd/m² for HDR).

use crate::lut::Lut3d;
use crate::math::{Mat3, Rgb};
use crate::primaries::{ACES_WHITE, D65, Primaries, bradford};
use crate::transfer::{acescct_to_lin, lin_to_acescct};
use std::sync::OnceLock;

/// Smallest normal half float (the CTL's `HALF_MIN`).
const HALF_MIN: f32 = 6.103_515_6e-5;
/// Largest half float (the CTL's `HALF_MAX`).
const HALF_MAX: f32 = 65504.0;

/// A segmented quadratic B-spline in log-log space (the CTL's
/// `SegmentedSplineParams_c5` / `_c9` and SSTS `TsParams`).
#[derive(Clone, Debug)]
struct Spline {
    low: Vec<f64>,
    high: Vec<f64>,
    min: (f64, f64),
    mid: (f64, f64),
    max: (f64, f64),
    slope_low: f64,
    slope_high: f64,
}

const BASIS: [[f64; 3]; 3] = [[0.5, -1.0, 0.5], [-1.0, 1.0, 0.5], [0.5, 0.0, 0.0]];

fn quad(cf: [f64; 3]) -> [f64; 3] {
    // mult_f3_f33(cf, M): row vector times matrix.
    let mut r = [0.0; 3];
    for (j, rj) in r.iter_mut().enumerate() {
        *rj = cf[0] * BASIS[0][j] + cf[1] * BASIS[1][j] + cf[2] * BASIS[2][j];
    }
    r
}

impl Spline {
    fn knots(&self) -> usize {
        self.low.len() - 2
    }

    fn fwd(&self, x: f64) -> f64 {
        let n = self.knots();
        let logx = x.max(HALF_MIN as f64).log10();
        let (lmin, lmid, lmax) = (self.min.0.log10(), self.mid.0.log10(), self.max.0.log10());
        let logy = if logx <= lmin {
            logx * self.slope_low + (self.min.1.log10() - self.slope_low * lmin)
        } else if logx < lmid {
            let kc = (n - 1) as f64 * (logx - lmin) / (lmid - lmin);
            let j = (kc as usize).min(n - 2);
            let t = kc - j as f64;
            let m = quad([self.low[j], self.low[j + 1], self.low[j + 2]]);
            t * t * m[0] + t * m[1] + m[2]
        } else if logx < lmax {
            let kc = (n - 1) as f64 * (logx - lmid) / (lmax - lmid);
            let j = (kc as usize).min(n - 2);
            let t = kc - j as f64;
            let m = quad([self.high[j], self.high[j + 1], self.high[j + 2]]);
            t * t * m[0] + t * m[1] + m[2]
        } else {
            logx * self.slope_high + (self.max.1.log10() - self.slope_high * lmax)
        };
        10f64.powf(logy)
    }

    fn rev(&self, y: f64) -> f64 {
        let n = self.knots();
        let logy = y.max(1e-10).log10();
        let (lmin, lmid, lmax) = (self.min.0.log10(), self.mid.0.log10(), self.max.0.log10());
        let seg = |coefs: &[f64], base: f64, inc: f64| -> f64 {
            let ky: Vec<f64> = (0..n).map(|i| (coefs[i] + coefs[i + 1]) / 2.0).collect();
            let mut j = 0;
            for i in 0..n - 1 {
                if logy > ky[i] && logy <= ky[i + 1] {
                    j = i;
                    break;
                }
                if logy > ky[i + 1] {
                    j = i + 1;
                }
            }
            let j = j.min(n - 2);
            let m = quad([coefs[j], coefs[j + 1], coefs[j + 2]]);
            let (a, b, c) = (m[0], m[1], m[2] - logy);
            let d = (b * b - 4.0 * a * c).max(0.0).sqrt();
            let t = (2.0 * c) / (-d - b);
            base + (t + j as f64) * inc
        };
        let logx = if logy <= self.min.1.log10() {
            lmin
        } else if logy <= self.mid.1.log10() {
            seg(&self.low, lmin, (lmid - lmin) / (n - 1) as f64)
        } else if logy < self.max.1.log10() {
            seg(&self.high, lmid, (lmax - lmid) / (n - 1) as f64)
        } else {
            lmax
        };
        10f64.powf(logx)
    }
}

fn rrt_spline() -> &'static Spline {
    static S: OnceLock<Spline> = OnceLock::new();
    S.get_or_init(|| Spline {
        low: vec![-4.0, -4.0, -3.157_376_577_3, -0.485_249_995_8, 1.847_732_470_6, 1.847_732_470_6],
        high: vec![-0.718_548_242_5, 2.081_030_717_2, 3.668_124_123_7, 4.0, 4.0, 4.0],
        min: (0.18 * 2f64.powi(-15), 0.0001),
        mid: (0.18, 4.8),
        max: (0.18 * 2f64.powi(18), 10000.0),
        slope_low: 0.0,
        slope_high: 0.0,
    })
}

fn odt48_spline() -> &'static Spline {
    static S: OnceLock<Spline> = OnceLock::new();
    S.get_or_init(|| {
        let c5 = rrt_spline();
        Spline {
            low: vec![-1.698_970_004_3, -1.698_970_004_3, -1.4779, -1.2291, -0.8648, -0.4480, 0.005_18, 0.451_108_033_4, 0.911_374_441_4, 0.911_374_441_4],
            high: vec![0.515_438_696_5, 0.847_043_778_3, 1.1358, 1.3802, 1.5197, 1.5985, 1.6467, 1.674_609_135_7, 1.687_873_339, 1.687_873_339],
            min: (c5.fwd(0.18 * 2f64.powf(-6.5)), 0.02),
            mid: (c5.fwd(0.18), 4.8),
            max: (c5.fwd(0.18 * 2f64.powf(6.5)), 48.0),
            slope_low: 0.0,
            slope_high: 0.04,
        }
    })
}

/// Linear interpolation in a two-row table, clamped at its ends
/// (`interpolate1D` in the CTL).
fn interp(table: [[f64; 2]; 2], p: f64) -> f64 {
    let [[x0, y0], [x1, y1]] = table;
    if p <= x0 {
        y0
    } else if p >= x1 {
        y1
    } else {
        y0 + (p - x0) / (x1 - x0) * (y1 - y0)
    }
}

/// The SSTS tone scale for a display from `min_lum` to `max_lum` cd/m²
/// with 18 % grey shown at `mid_lum`.
fn ssts(min_lum: f64, mid_lum: f64, max_lum: f64) -> Spline {
    let (min_stop_sdr, max_stop_sdr, min_stop_rrt, max_stop_rrt) = (-6.5, 6.5, -15.0, 18.0);
    let (min_lum_sdr, max_lum_sdr, min_lum_rrt, max_lum_rrt) = (0.02f64, 48.0f64, 0.0001f64, 10000.0f64);
    let aces_min = |l: f64| 0.18 * 2f64.powf(interp([[min_lum_rrt.log10(), min_stop_rrt], [min_lum_sdr.log10(), min_stop_sdr]], l.log10()));
    let aces_max = |l: f64| 0.18 * 2f64.powf(interp([[max_lum_sdr.log10(), max_stop_sdr], [max_lum_rrt.log10(), max_stop_rrt]], l.log10()));
    let build = |exp_shift: f64| -> Spline {
        let (lo, md, hi): ((f64, f64, f64), (f64, f64, f64), (f64, f64, f64)) = ((aces_min(min_lum), min_lum, 0.0), (0.18, 4.8, 1.55), (aces_max(max_lum), max_lum, 0.0));
        let line = |p: (f64, f64, f64), x: f64| p.2 * x + (p.1.log10() - p.2 * p.0.log10());
        let inc_low = (md.0.log10() - lo.0.log10()) / 3.0;
        let pct_low = interp([[min_stop_rrt, 0.18], [min_stop_sdr, 0.35]], (lo.0 / 0.18).log2());
        let c_low = [
            line(lo, lo.0.log10() - 0.5 * inc_low),
            line(lo, lo.0.log10() + 0.5 * inc_low),
            lo.1.log10() + pct_low * (md.1.log10() - lo.1.log10()),
            line(md, md.0.log10() - 0.5 * inc_low),
            line(md, md.0.log10() + 0.5 * inc_low),
        ];
        let inc_high = (hi.0.log10() - md.0.log10()) / 3.0;
        let pct_high = interp([[max_stop_sdr, 0.89], [max_stop_rrt, 0.90]], (hi.0 / 0.18).log2());
        let c_high = [
            line(md, md.0.log10() - 0.5 * inc_high),
            line(md, md.0.log10() + 0.5 * inc_high),
            md.1.log10() + pct_high * (hi.1.log10() - md.1.log10()),
            line(hi, hi.0.log10() - 0.5 * inc_high),
            line(hi, hi.0.log10() + 0.5 * inc_high),
        ];
        let shift = |x: f64| 2f64.powf(x.log2() - exp_shift);
        Spline {
            low: vec![c_low[0], c_low[1], c_low[2], c_low[3], c_low[4], c_low[4]],
            high: vec![c_high[0], c_high[1], c_high[2], c_high[3], c_high[4], c_high[4]],
            min: (shift(lo.0), lo.1),
            mid: (shift(md.0), md.1),
            max: (shift(hi.0), hi.1),
            slope_low: 0.0,
            slope_high: 0.0,
        }
    };
    let default = build(0.0);
    let exp_shift = default.rev(mid_lum).log2() - 0.18f64.log2();
    build(exp_shift)
}

// ------------------------------------------------------------- matrices

struct Mats {
    ap0_to_ap1: Mat3,
    ap1_to_ap0: Mat3,
    ap1_to_xyz: Mat3,
    xyz_to_ap1: Mat3,
    d60_to_d65: Mat3,
    d65_to_d60: Mat3,
    rrt_sat: Mat3,
    odt_sat: Mat3,
    working_to_ap0: Mat3,
}

fn sat_matrix(s: f64, y: [f64; 3]) -> Mat3 {
    let mut m = [[0.0; 3]; 3];
    for (i, row) in m.iter_mut().enumerate() {
        for (j, v) in row.iter_mut().enumerate() {
            *v = (1.0 - s) * y[j] + if i == j { s } else { 0.0 };
        }
    }
    Mat3(m)
}

fn mats() -> &'static Mats {
    static M: OnceLock<Mats> = OnceLock::new();
    M.get_or_init(|| {
        let ap0 = Primaries::ACES_AP0;
        let ap1 = Primaries::ACES_AP1;
        let ap0_to_ap1 = ap1.xyz_to_rgb().mul(&ap0.rgb_to_xyz());
        let ap1_to_xyz = ap1.rgb_to_xyz();
        let ap1_y = ap1_to_xyz.0[1];
        Mats {
            ap0_to_ap1,
            ap1_to_ap0: ap0_to_ap1.inverse().expect("ap1"),
            ap1_to_xyz,
            xyz_to_ap1: ap1.xyz_to_rgb(),
            d60_to_d65: bradford(ACES_WHITE, D65),
            d65_to_d60: bradford(D65, ACES_WHITE),
            rrt_sat: sat_matrix(0.96, ap1_y),
            odt_sat: sat_matrix(0.93, ap1_y),
            working_to_ap0: crate::primaries::rgb_to_rgb(&Primaries::BT709, &ap0),
        }
    })
}

fn mul(m: &Mat3, v: [f64; 3]) -> [f64; 3] {
    m.mul_vec64(v)
}

fn clamp3(v: [f64; 3], lo: f64, hi: f64) -> [f64; 3] {
    v.map(|c| c.clamp(lo, hi))
}

// --------------------------------------------------------- RRT helpers

fn rgb_2_saturation(c: [f64; 3]) -> f64 {
    let tiny = 1e-10;
    let mx = c[0].max(c[1]).max(c[2]);
    let mn = c[0].min(c[1]).min(c[2]);
    (mx.max(tiny) - mn.max(tiny)) / mx.max(1e-2)
}

fn rgb_2_yc(c: [f64; 3]) -> f64 {
    let (r, g, b) = (c[0], c[1], c[2]);
    let chroma = (b * (b - g) + g * (g - r) + r * (r - b)).max(0.0).sqrt();
    (b + g + r + 1.75 * chroma) / 3.0
}

fn rgb_2_hue(c: [f64; 3]) -> f64 {
    if c[0] == c[1] && c[1] == c[2] {
        return 0.0;
    }
    let h = (3f64.sqrt() * (c[1] - c[2])).atan2(2.0 * c[0] - c[1] - c[2]).to_degrees();
    if h < 0.0 { h + 360.0 } else { h }
}

fn sigmoid_shaper(x: f64) -> f64 {
    let t = (1.0 - (x / 2.0).abs()).max(0.0);
    let y = 1.0 + x.signum() * (1.0 - t * t);
    y / 2.0
}

fn glow_fwd(yc: f64, gain: f64, mid: f64) -> f64 {
    if yc <= 2.0 / 3.0 * mid {
        gain
    } else if yc >= 2.0 * mid {
        0.0
    } else {
        gain * (mid / yc - 0.5)
    }
}

fn glow_inv(yc: f64, gain: f64, mid: f64) -> f64 {
    if yc <= (1.0 + gain) * 2.0 / 3.0 * mid {
        -gain / (1.0 + gain)
    } else if yc >= 2.0 * mid {
        0.0
    } else {
        gain * (mid / yc - 0.5) / (gain / 2.0 - 1.0)
    }
}

fn cubic_basis_shaper(x: f64, w: f64) -> f64 {
    const M: [[f64; 4]; 4] = [
        [-1.0 / 6.0, 3.0 / 6.0, -3.0 / 6.0, 1.0 / 6.0],
        [3.0 / 6.0, -6.0 / 6.0, 3.0 / 6.0, 0.0],
        [-3.0 / 6.0, 0.0, 3.0 / 6.0, 0.0],
        [1.0 / 6.0, 4.0 / 6.0, 1.0 / 6.0, 0.0],
    ];
    let k0 = -w / 2.0;
    let k4 = w / 2.0;
    if x <= k0 || x >= k4 {
        return 0.0;
    }
    let kc = (x - k0) * 4.0 / w;
    let j = (kc as usize).min(3);
    let t = kc - j as f64;
    let mono = [t * t * t, t * t, t, 1.0];
    let col = 3 - j;
    let y = mono[0] * M[0][col] + mono[1] * M[1][col] + mono[2] * M[2][col] + mono[3] * M[3][col];
    y * 1.5
}

fn center_hue(h: f64, c: f64) -> f64 {
    let d = h - c;
    if d < -180.0 {
        d + 360.0
    } else if d > 180.0 {
        d - 360.0
    } else {
        d
    }
}

const GLOW_GAIN: f64 = 0.05;
const GLOW_MID: f64 = 0.08;
const RED_SCALE: f64 = 0.82;
const RED_PIVOT: f64 = 0.03;
const RED_WIDTH: f64 = 135.0;

/// ACES2065-1 → AP1 values ready for the tone scale.
fn rrt_sweeteners(input: [f64; 3]) -> [f64; 3] {
    let m = mats();
    let mut aces = input;
    let sat = rgb_2_saturation(aces);
    let yc = rgb_2_yc(aces);
    let s = sigmoid_shaper((sat - 0.4) / 0.2);
    let glow = 1.0 + glow_fwd(yc, GLOW_GAIN * s, GLOW_MID);
    aces = aces.map(|c| c * glow);
    let hue = rgb_2_hue(aces);
    let w = cubic_basis_shaper(center_hue(hue, 0.0), RED_WIDTH);
    aces[0] += w * sat * (RED_PIVOT - aces[0]) * (1.0 - RED_SCALE);
    let aces = clamp3(aces, 0.0, f64::INFINITY);
    let pre = clamp3(mul(&m.ap0_to_ap1, aces), 0.0, HALF_MAX as f64);
    mul(&m.rrt_sat, pre)
}

fn inv_rrt_sweeteners(input: [f64; 3]) -> [f64; 3] {
    let m = mats();
    let post = clamp3(mul(&m.rrt_sat.inverse().expect("sat"), input), 0.0, HALF_MAX as f64);
    let mut aces = clamp3(mul(&m.ap1_to_ap0, post), 0.0, HALF_MAX as f64);
    let hue = rgb_2_hue(aces);
    let centered = center_hue(hue, 0.0);
    let w = cubic_basis_shaper(centered, RED_WIDTH);
    let min_chan = if centered < 0.0 { aces[1] } else { aces[2] };
    let a = w * (1.0 - RED_SCALE) - 1.0;
    let b = aces[0] - w * (RED_PIVOT + min_chan) * (1.0 - RED_SCALE);
    let c = w * RED_PIVOT * min_chan * (1.0 - RED_SCALE);
    aces[0] = (-b - (b * b - 4.0 * a * c).max(0.0).sqrt()) / (2.0 * a);
    let sat = rgb_2_saturation(aces);
    let yc = rgb_2_yc(aces);
    let s = sigmoid_shaper((sat - 0.4) / 0.2);
    let glow = 1.0 + glow_inv(yc, GLOW_GAIN * s, GLOW_MID);
    aces.map(|c| c * glow)
}

/// The Reference Rendering Transform: ACES2065-1 → OCES.
pub fn rrt(aces: [f64; 3]) -> [f64; 3] {
    let pre = rrt_sweeteners(aces);
    let s = rrt_spline();
    mul(&mats().ap1_to_ap0, pre.map(|c| s.fwd(c)))
}

/// Inverse of [`rrt`].
pub fn inv_rrt(oces: [f64; 3]) -> [f64; 3] {
    let s = rrt_spline();
    let pre = mul(&mats().ap0_to_ap1, oces).map(|c| s.rev(c));
    inv_rrt_sweeteners(pre)
}

fn xyz_to_xyy(v: [f64; 3]) -> [f64; 3] {
    let s = (v[0] + v[1] + v[2]).max(1e-10);
    [v[0] / s, v[1] / s, v[1]]
}

fn xyy_to_xyz(v: [f64; 3]) -> [f64; 3] {
    let y = v[2];
    let yy = v[1].max(1e-10);
    [v[0] * y / yy, y, (1.0 - v[0] - v[1]) * y / yy]
}

fn surround(lin_ap1: [f64; 3], gamma: f64) -> [f64; 3] {
    let m = mats();
    let mut xyy = xyz_to_xyy(mul(&m.ap1_to_xyz, lin_ap1));
    xyy[2] = xyy[2].max(0.0).powf(gamma);
    mul(&m.xyz_to_ap1, xyy_to_xyz(xyy))
}

const DIM_SURROUND_GAMMA: f64 = 0.9811;
const CINEMA_WHITE: f64 = 48.0;
const CINEMA_BLACK: f64 = 0.02;

/// What a view is shown on: decides between the SDR ODT and an HDR
/// output transform, and the display primaries.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DisplayTarget {
    /// sRGB or Rec.709 display (100 cd/m², dim surround). The curve is the
    /// output encoder's; both use the same display-linear values.
    Rec709,
    /// P3-D65 display (Display P3 monitors).
    P3D65,
    /// Rec.2020 SDR.
    Rec2020,
    /// Rec.2020 PQ with a peak of this many cd/m² (1000, 2000 or 4000).
    Rec2020Pq(u16),
    /// Rec.2020 HLG, 1000 cd/m² display.
    Rec2020Hlg,
}

impl DisplayTarget {
    fn primaries(self) -> Primaries {
        match self {
            DisplayTarget::Rec709 => Primaries::BT709,
            DisplayTarget::P3D65 => Primaries::P3_D65,
            _ => Primaries::BT2020,
        }
    }

    pub fn is_hdr(self) -> bool {
        matches!(self, DisplayTarget::Rec2020Pq(_) | DisplayTarget::Rec2020Hlg)
    }

    fn peak(self) -> f64 {
        match self {
            DisplayTarget::Rec2020Pq(n) => (n as f64).clamp(400.0, 10000.0),
            DisplayTarget::Rec2020Hlg => 1000.0,
            _ => 100.0,
        }
    }

    /// ACES2065-1 → display light in the display's primaries: 0..1 of
    /// display white for SDR, cd/m² for HDR.
    pub fn output_transform(self, aces: [f64; 3]) -> [f64; 3] {
        let m = mats();
        let to_display = self.primaries().xyz_to_rgb();
        if !self.is_hdr() {
            let oces = rrt(aces);
            let s = odt48_spline();
            let y = mul(&m.ap0_to_ap1, oces).map(|c| s.fwd(c));
            let lin = y.map(|v| (v - CINEMA_BLACK) / (CINEMA_WHITE - CINEMA_BLACK));
            let lin = surround(lin, DIM_SURROUND_GAMMA);
            let lin = mul(&m.odt_sat, lin);
            let xyz = mul(&m.d60_to_d65, mul(&m.ap1_to_xyz, lin));
            return clamp3(mul(&to_display, xyz), 0.0, 1.0);
        }
        let ts = hdr_tonescale(self.peak());
        let pre = rrt_sweeteners(aces);
        let y = pre.map(|c| ts.fwd(c));
        let (ymin, ymax) = (0.0001, self.peak());
        let lin = y.map(|v| (v - ymin) / (ymax - ymin));
        let xyz = mul(&m.ap1_to_xyz, lin);
        // Limit to the display's own primaries (Rec.2020 here).
        let lim = Primaries::BT2020;
        let rgb = clamp3(mul(&lim.xyz_to_rgb(), xyz), 0.0, 1.0);
        let xyz = mul(&m.d60_to_d65, mul(&lim.rgb_to_xyz(), rgb));
        let lin = clamp3(mul(&to_display, xyz), 0.0, f64::INFINITY);
        // Stretch black: code 0 is true black.
        lin.map(|c| c * ymax)
    }

    /// Inverse of the SDR output transform: display light (0..1, display
    /// primaries) → ACES2065-1. Used to bring display-referred material
    /// (graphics, Rec.709 deliverables) into an ACES timeline unchanged.
    pub fn inverse_output_transform(self, display: [f64; 3]) -> [f64; 3] {
        let m = mats();
        let from_display = self.primaries().rgb_to_xyz();
        let xyz = mul(&m.d65_to_d60, mul(&from_display, clamp3(display, 0.0, 1.0)));
        let lin = mul(&m.xyz_to_ap1, xyz);
        let lin = mul(&m.odt_sat.inverse().expect("sat"), lin);
        let lin = surround(lin, 1.0 / DIM_SURROUND_GAMMA);
        let y = lin.map(|v| v * (CINEMA_WHITE - CINEMA_BLACK) + CINEMA_BLACK);
        let s = odt48_spline();
        let oces = mul(&m.ap1_to_ap0, y.map(|c| s.rev(c)));
        inv_rrt(oces)
    }

    /// Studio working linear (BT.709, D65) → what render's output encoder
    /// expects for this display: display-linear in working primaries, 1.0
    /// = SDR white or 203 cd/m² (HDR).
    pub fn view(self, working: Rgb) -> Rgb {
        let m = mats();
        let aces = mul(&m.working_to_ap0, working.map(|c| c as f64));
        let out = self.output_transform(aces);
        let to_working = crate::primaries::rgb_to_rgb(&self.primaries(), &Primaries::BT709);
        let scale = if self.is_hdr() { 1.0 / 203.0 } else { 1.0 };
        mul(&to_working, out.map(|c| c * scale)).map(|c| c as f32)
    }

    /// Inverse of [`DisplayTarget::view`] for SDR targets.
    pub fn unview(self, display_working: Rgb) -> Rgb {
        let to_display = crate::primaries::rgb_to_rgb(&Primaries::BT709, &self.primaries());
        let disp = mul(&to_display, display_working.map(|c| c as f64));
        let aces = self.inverse_output_transform(disp);
        mul(&mats().working_to_ap0.inverse().expect("ap0"), aces).map(|c| c as f32)
    }
}

fn hdr_tonescale(peak: f64) -> Spline {
    ssts(0.0001, 15.0, peak)
}

/// Lattice size of baked view LUTs.
pub const VIEW_LUT_SIZE: usize = 65;

/// [`DisplayTarget::view`] baked into a 3D LUT whose input and output
/// are ACEScct on AP1 primaries (use with [`crate::LutSpace::AcesCct`]).
/// The log shaper spreads the lattice over scene-linear −0.007 … 222.
pub fn bake_view(target: DisplayTarget, size: usize) -> Lut3d {
    let ap1 = Primaries::ACES_AP1;
    let to_working = crate::primaries::rgb_to_rgb(&ap1, &Primaries::BT709);
    let to_ap1 = crate::primaries::rgb_to_rgb(&Primaries::BT709, &ap1);
    Lut3d::from_fn(size, |cct| {
        let lin = to_working.apply(cct.map(acescct_to_lin));
        let out = target.view(lin);
        to_ap1.apply(out).map(lin_to_acescct)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rrt_spline_hits_its_points() {
        let s = rrt_spline();
        assert!((s.fwd(0.18) - 4.8).abs() < 1e-6);
        for x in [0.001, 0.01, 0.18, 1.0, 10.0, 100.0] {
            assert!((s.rev(s.fwd(x)) / x - 1.0).abs() < 1e-5, "c5 at {x}");
        }
        let c9 = odt48_spline();
        for y in [0.05, 1.0, 4.8, 20.0, 40.0] {
            assert!((c9.fwd(c9.rev(y)) / y - 1.0).abs() < 1e-5, "c9 at {y}");
        }
    }

    /// Mid grey through the Rec.709 output transform lands near code 0.39
    /// (BT.1886), the well-known ACES 1.x value; white saturates.
    #[test]
    fn sdr_grey_and_white() {
        let t = DisplayTarget::Rec709;
        let g = t.view([0.18, 0.18, 0.18]);
        let code = g[0].powf(1.0 / 2.4);
        assert!((code - 0.389).abs() < 0.005, "grey code {code}");
        assert!((g[0] - g[1]).abs() < 1e-3 && (g[1] - g[2]).abs() < 1e-3, "grey stays neutral {g:?}");
        let w = t.view([16.0, 16.0, 16.0]);
        assert!(w[1] > 0.97, "bright white {w:?}");
        let b = t.view([0.0, 0.0, 0.0]);
        assert!(b[1] < 0.001, "black {b:?}");
    }

    #[test]
    fn sdr_monotone() {
        let t = DisplayTarget::Rec709;
        let mut last = -1.0;
        for i in 0..200 {
            let x = 0.0005 * 1.06f32.powi(i);
            let y = t.view([x, x, x])[1];
            assert!(y >= last - 1e-6, "at {x}");
            last = y;
        }
    }

    #[test]
    fn hdr_grey_at_15_nits() {
        for peak in [1000, 2000, 4000] {
            let t = DisplayTarget::Rec2020Pq(peak);
            let g = t.view([0.18, 0.18, 0.18]);
            let nits = g[1] * 203.0;
            assert!((nits - 15.0).abs() < 0.6, "{peak}: grey at {nits} nits");
            let hi = t.view([60000.0, 60000.0, 60000.0])[1] * 203.0;
            assert!(hi > peak as f32 * 0.97 && hi <= peak as f32 * 1.01, "{peak}: highlight at {hi}");
        }
    }

    #[test]
    fn sdr_inverse_round_trips() {
        let t = DisplayTarget::Rec709;
        for c in [[0.5, 0.5, 0.5], [0.2, 0.4, 0.6], [0.8, 0.3, 0.1], [0.05, 0.05, 0.05], [0.9, 0.9, 0.9]] {
            let back = t.view(t.unview(c));
            for k in 0..3 {
                // The inverse red modifier is approximate in the reference too.
                assert!((back[k] - c[k]).abs() < 1e-2, "{c:?} -> {back:?}");
            }
        }
    }

    #[test]
    fn baked_view_matches_direct() {
        let t = DisplayTarget::Rec709;
        let lut = bake_view(t, 33);
        let ap1 = Primaries::ACES_AP1;
        let to_ap1 = crate::primaries::rgb_to_rgb(&Primaries::BT709, &ap1);
        let to_working = crate::primaries::rgb_to_rgb(&ap1, &Primaries::BT709);
        for c in [[0.18f32, 0.18, 0.18], [0.5, 0.2, 0.1], [2.0, 1.5, 1.0], [0.02, 0.03, 0.05]] {
            let want = t.view(c);
            let cct = to_ap1.apply(c).map(lin_to_acescct);
            let got = to_working.apply(lut.sample(cct).map(acescct_to_lin));
            for k in 0..3 {
                assert!((got[k] - want[k]).abs() < 0.01, "{c:?}: {got:?} vs {want:?}");
            }
        }
    }
}
