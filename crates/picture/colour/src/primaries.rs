// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! RGB colour spaces, XYZ conversion, Bradford chromatic adaptation and
//! correlated colour temperature.
//!
//! Primaries and white points are the published values of ITU-R BT.709,
//! BT.601 (625- and 525-line), BT.2020, SMPTE EG 432-1 (P3 with D65) and
//! the Academy's AP0 / AP1 (S-2014-004).

use crate::math::Mat3;

/// CIE 1931 xy chromaticity.
pub type Xy = [f64; 2];

/// The CIE standard illuminant D65 white point.
pub const D65: Xy = [0.3127, 0.3290];
/// The white point used by ACES (close to, but not exactly, D60).
pub const ACES_WHITE: Xy = [0.32168, 0.33767];

/// Three primaries and a white point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Primaries {
    pub red: Xy,
    pub green: Xy,
    pub blue: Xy,
    pub white: Xy,
}

impl Primaries {
    /// ITU-R BT.709, shared by sRGB.
    pub const BT709: Primaries = Primaries { red: [0.64, 0.33], green: [0.30, 0.60], blue: [0.15, 0.06], white: D65 };
    /// ITU-R BT.601, 625-line systems (EBU).
    pub const BT601_625: Primaries = Primaries { red: [0.64, 0.33], green: [0.29, 0.60], blue: [0.15, 0.06], white: D65 };
    /// ITU-R BT.601, 525-line systems (SMPTE C).
    pub const BT601_525: Primaries = Primaries { red: [0.630, 0.340], green: [0.310, 0.595], blue: [0.155, 0.070], white: D65 };
    /// ITU-R BT.2020 / BT.2100.
    pub const BT2020: Primaries = Primaries { red: [0.708, 0.292], green: [0.170, 0.797], blue: [0.131, 0.046], white: D65 };
    /// P3 primaries with a D65 white (Display P3).
    pub const P3_D65: Primaries = Primaries { red: [0.680, 0.320], green: [0.265, 0.690], blue: [0.150, 0.060], white: D65 };
    /// ACES AP0 (ACES2065-1).
    pub const ACES_AP0: Primaries =
        Primaries { red: [0.7347, 0.2653], green: [0.0, 1.0], blue: [0.0001, -0.0770], white: ACES_WHITE };
    /// ACES AP1 (ACEScg, ACEScct). The grading space uses these primaries.
    pub const ACES_AP1: Primaries =
        Primaries { red: [0.713, 0.293], green: [0.165, 0.830], blue: [0.128, 0.044], white: ACES_WHITE };

    /// Matrix from linear RGB in these primaries to CIE XYZ (Y of white = 1).
    pub fn rgb_to_xyz(&self) -> Mat3 {
        let r = xy_to_xyz(self.red);
        let g = xy_to_xyz(self.green);
        let b = xy_to_xyz(self.blue);
        let m = Mat3::from_columns(r, g, b);
        // Scale each primary so that RGB (1,1,1) lands exactly on the white.
        let s = m.inverse().expect("degenerate primaries").mul_vec64(xy_to_xyz(self.white));
        m.mul(&Mat3::diag(s))
    }

    pub fn xyz_to_rgb(&self) -> Mat3 {
        self.rgb_to_xyz().inverse().expect("degenerate primaries")
    }

    /// Luminance weights of the three primaries (the Y row of
    /// [`Primaries::rgb_to_xyz`]).
    pub fn luminance_weights(&self) -> [f32; 3] {
        let m = self.rgb_to_xyz();
        [m.0[1][0] as f32, m.0[1][1] as f32, m.0[1][2] as f32]
    }
}

/// xy chromaticity → XYZ with Y = 1.
pub fn xy_to_xyz(xy: Xy) -> [f64; 3] {
    [xy[0] / xy[1], 1.0, (1.0 - xy[0] - xy[1]) / xy[1]]
}

/// XYZ → xy chromaticity.
pub fn xyz_to_xy(xyz: [f64; 3]) -> Xy {
    let s = xyz[0] + xyz[1] + xyz[2];
    if s.abs() < 1e-12 { D65 } else { [xyz[0] / s, xyz[1] / s] }
}

/// The Bradford cone-response matrix (Lam 1985, as standardised for
/// ICC / CIECAM97s chromatic adaptation).
const BRADFORD: Mat3 = Mat3([
    [0.8951, 0.2664, -0.1614],
    [-0.7502, 1.7135, 0.0367],
    [0.0389, -0.0685, 1.0296],
]);

/// XYZ → XYZ matrix that adapts colours seen under `src` white so they
/// appear as if seen under `dst` white (von Kries scaling in Bradford
/// cone space).
pub fn bradford(src: Xy, dst: Xy) -> Mat3 {
    if src == dst {
        return Mat3::IDENTITY;
    }
    let ms = BRADFORD.mul_vec64(xy_to_xyz(src));
    let md = BRADFORD.mul_vec64(xy_to_xyz(dst));
    let d = Mat3::diag([md[0] / ms[0], md[1] / ms[1], md[2] / ms[2]]);
    BRADFORD.inverse().expect("bradford").mul(&d).mul(&BRADFORD)
}

/// Linear RGB → linear RGB between two spaces, with Bradford adaptation
/// when their white points differ.
pub fn rgb_to_rgb(src: &Primaries, dst: &Primaries) -> Mat3 {
    if src == dst {
        return Mat3::IDENTITY;
    }
    dst.xyz_to_rgb().mul(&bradford(src.white, dst.white)).mul(&src.rgb_to_xyz())
}

// ----------------------------------------------------------- temperature

/// Temperatures the Planckian approximation below is valid for.
pub const CCT_MIN: f64 = 1667.0;
pub const CCT_MAX: f64 = 25000.0;

/// Chromaticity of a black body at `kelvin`, using the cubic-spline
/// approximation published by Kim et al. (2002), valid 1667–25000 K.
pub fn planckian_xy(kelvin: f64) -> Xy {
    let t = kelvin.clamp(CCT_MIN, CCT_MAX);
    let (t2, t3) = (t * t, t * t * t);
    let x = if t <= 4000.0 {
        -0.266_123_9e9 / t3 - 0.234_358_9e6 / t2 + 0.877_695_6e3 / t + 0.179_910
    } else {
        -3.025_846_9e9 / t3 + 2.107_037_9e6 / t2 + 0.222_634_7e3 / t + 0.240_390
    };
    let (x2, x3) = (x * x, x * x * x);
    let y = if t <= 2222.0 {
        -1.106_381_4 * x3 - 1.348_110_20 * x2 + 2.185_558_32 * x - 0.202_196_83
    } else if t <= 4000.0 {
        -0.954_947_6 * x3 - 1.374_185_93 * x2 + 2.091_370_15 * x - 0.167_488_67
    } else {
        3.081_758_0 * x3 - 5.873_386_70 * x2 + 3.751_129_97 * x - 0.370_014_83
    };
    [x, y]
}

/// CIE 1960 UCS (u, v) from xy.
pub fn xy_to_uv(xy: Xy) -> [f64; 2] {
    let d = -2.0 * xy[0] + 12.0 * xy[1] + 3.0;
    [4.0 * xy[0] / d, 6.0 * xy[1] / d]
}

pub fn uv_to_xy(uv: [f64; 2]) -> Xy {
    let d = 2.0 * uv[0] - 8.0 * uv[1] + 4.0;
    [3.0 * uv[0] / d, 2.0 * uv[1] / d]
}

/// Unit normal to the Planckian locus in uv at `kelvin`, pointing to
/// positive Duv (above the locus, towards green).
fn locus_normal(kelvin: f64) -> [f64; 2] {
    let h = (kelvin * 1e-3).max(1.0);
    let a = xy_to_uv(planckian_xy((kelvin - h).max(CCT_MIN)));
    let b = xy_to_uv(planckian_xy((kelvin + h).min(CCT_MAX)));
    let (tu, tv) = (b[0] - a[0], b[1] - a[1]);
    let len = (tu * tu + tv * tv).sqrt();
    let n = [-tv / len, tu / len];
    if n[1] >= 0.0 { n } else { [-n[0], -n[1]] }
}

/// White point for a correlated colour temperature and a Duv offset from
/// the Planckian locus (positive = greener, negative = more magenta).
pub fn cct_duv_to_xy(kelvin: f64, duv: f64) -> Xy {
    let uv = xy_to_uv(planckian_xy(kelvin));
    let n = locus_normal(kelvin);
    uv_to_xy([uv[0] + duv * n[0], uv[1] + duv * n[1]])
}

/// Inverse of [`cct_duv_to_xy`]: finds the nearest locus point by a golden
/// section search in mired (1e6 / K), which is close to perceptually even.
pub fn xy_to_cct_duv(xy: Xy) -> (f64, f64) {
    let uv = xy_to_uv(xy);
    let dist = |mired: f64| {
        let p = xy_to_uv(planckian_xy(1e6 / mired));
        (p[0] - uv[0]).powi(2) + (p[1] - uv[1]).powi(2)
    };
    let (mut a, mut b) = (1e6 / CCT_MAX, 1e6 / CCT_MIN);
    // Coarse scan first: the distance is not unimodal over the full range
    // for points far from the locus.
    let mut best = a;
    let mut best_d = f64::MAX;
    for i in 0..=200 {
        let m = a + (b - a) * i as f64 / 200.0;
        let d = dist(m);
        if d < best_d {
            best_d = d;
            best = m;
        }
    }
    let step = (b - a) / 200.0;
    a = (best - step).max(1e6 / CCT_MAX);
    b = (best + step).min(1e6 / CCT_MIN);
    let g = (5f64.sqrt() - 1.0) / 2.0;
    for _ in 0..60 {
        let c = b - g * (b - a);
        let d = a + g * (b - a);
        if dist(c) < dist(d) { b = d } else { a = c }
    }
    let kelvin = 1e6 / ((a + b) / 2.0);
    let p = xy_to_uv(planckian_xy(kelvin));
    let n = locus_normal(kelvin);
    let duv = (uv[0] - p[0]) * n[0] + (uv[1] - p[1]) * n[1];
    (kelvin, duv)
}

/// Neutral temperature of the white balance control. At this setting (and
/// zero tint) white balance is exactly the identity.
pub const NEUTRAL_KELVIN: f32 = 6504.0;

/// Duv per unit of the tint control (tint runs −1..+1).
pub const TINT_DUV_SCALE: f64 = 0.02;

/// White balance as a linear RGB matrix in `working` primaries.
///
/// `kelvin` and `tint` describe the light the footage was shot under. The
/// matrix adapts that light onto the neutral reference (the locus point at
/// [`NEUTRAL_KELVIN`], zero tint). So raising the temperature claims the
/// light was bluer and makes the picture warmer; positive tint claims it
/// was greener and makes the picture more magenta. Using the same locus
/// function for both ends makes the default exactly the identity and the
/// control continuous around it.
pub fn white_balance_matrix(working: &Primaries, kelvin: f32, tint: f32) -> Mat3 {
    if kelvin == NEUTRAL_KELVIN && tint == 0.0 {
        return Mat3::IDENTITY;
    }
    let src = cct_duv_to_xy(kelvin as f64, tint as f64 * TINT_DUV_SCALE);
    let dst = cct_duv_to_xy(NEUTRAL_KELVIN as f64, 0.0);
    working.xyz_to_rgb().mul(&bradford(src, dst)).mul(&working.rgb_to_xyz())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bt709_to_xyz_matches_published() {
        // ITU-R BT.709 / sRGB matrix, as published to four decimals.
        let m = Primaries::BT709.rgb_to_xyz().0;
        let expect = [[0.4124, 0.3576, 0.1805], [0.2126, 0.7152, 0.0722], [0.0193, 0.1192, 0.9505]];
        for i in 0..3 {
            for j in 0..3 {
                assert!((m[i][j] - expect[i][j]).abs() < 1e-4, "{i},{j}: {}", m[i][j]);
            }
        }
        let w = Primaries::BT2020.luminance_weights();
        assert!((w[0] - 0.2627).abs() < 1e-4 && (w[1] - 0.6780).abs() < 1e-4 && (w[2] - 0.0593).abs() < 1e-4);
    }

    #[test]
    fn ap0_to_ap1_matches_published() {
        // AP0 → AP1 from the ACES documentation (same white, no adaptation).
        let m = rgb_to_rgb(&Primaries::ACES_AP0, &Primaries::ACES_AP1).0;
        let expect = [
            [1.4514393161, -0.2365107469, -0.2149285693],
            [-0.0765537734, 1.1762296998, -0.0996759264],
            [0.0083161484, -0.0060324498, 0.9977163014],
        ];
        for i in 0..3 {
            for j in 0..3 {
                assert!((m[i][j] - expect[i][j]).abs() < 2e-4, "{i},{j}: {}", m[i][j]);
            }
        }
    }

    #[test]
    fn bradford_maps_white_to_white() {
        let m = bradford(D65, ACES_WHITE);
        let w = m.mul_vec64(xy_to_xyz(D65));
        let t = xy_to_xyz(ACES_WHITE);
        for c in 0..3 {
            assert!((w[c] - t[c]).abs() < 1e-9);
        }
    }

    #[test]
    fn planckian_near_d65() {
        let xy = planckian_xy(6504.0);
        assert!((xy[0] - 0.3135).abs() < 5e-4 && (xy[1] - 0.3236).abs() < 5e-4, "{xy:?}");
        // D65 sits slightly above the locus, near 6504 K.
        let (t, duv) = xy_to_cct_duv(D65);
        assert!((t - 6504.0).abs() < 25.0, "{t}");
        assert!(duv > 0.002 && duv < 0.0045, "{duv}");
    }

    #[test]
    fn cct_round_trip() {
        for &t in &[2000.0, 3200.0, 4500.0, 5600.0, 6504.0, 9000.0, 15000.0] {
            for &d in &[-0.01, 0.0, 0.012] {
                let (t2, d2) = xy_to_cct_duv(cct_duv_to_xy(t, d));
                assert!((t2 - t).abs() / t < 2e-3, "{t} {d} → {t2}");
                assert!((d2 - d).abs() < 3e-4, "{t} {d} → {d2}");
            }
        }
    }

    #[test]
    fn white_balance_directions() {
        let p = Primaries::BT709;
        assert_eq!(white_balance_matrix(&p, NEUTRAL_KELVIN, 0.0), Mat3::IDENTITY);
        let warm = white_balance_matrix(&p, 9000.0, 0.0).apply([0.5, 0.5, 0.5]);
        assert!(warm[0] > warm[2], "higher temperature warms: {warm:?}");
        let magenta = white_balance_matrix(&p, NEUTRAL_KELVIN, 1.0).apply([0.5, 0.5, 0.5]);
        assert!(magenta[1] < magenta[0] && magenta[1] < magenta[2], "positive tint is magenta: {magenta:?}");
    }
}
