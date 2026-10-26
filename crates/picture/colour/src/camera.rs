// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Camera log encodings and camera gamuts: the input side of a colour
//! managed pipeline (what ACES calls input transforms).
//!
//! Every curve maps a *full-range normalised code value* (10-bit code ÷
//! 1023) to scene-linear reflectance, so 18 % grey decodes to 0.18. The
//! constants are the manufacturers' published ones, as collected by
//! colour-science (BSD-3-Clause) and the ACES input transforms:
//!
//! * Sony S-Log3 (technical summary for S-Gamut3.Cine/S-Log3, 2015).
//! * ARRI LogC3 at EI 800 (ALEXA Log C curve, "linear scene exposure
//!   factor" form) and LogC4 (ARRI LogC4 specification, 2022).
//! * Panasonic V-Log (V-Log/V-Gamut reference manual).
//! * Canon Log, Log 2 and Log 3 (v1.2 formulae). Canon defines these on
//!   IRE-normalised (legal range) values with 90 % reflectance at 1.0;
//!   [`LogCurve::decode`] converts from full-range code values first.
//! * Apple Log (Apple Log profile white paper, 2023).
//! * RED Log3G10 (v3, IPP2), Fujifilm F-Log and F-Log2, DaVinci
//!   Intermediate, Blackmagic Film Generation 5.
//! * ACEScct and ACEScc (Academy S-2016-001 / S-2014-003).
//!
//! [`Gamut`] lists the matching wide-gamut primaries.

use crate::primaries::{ACES_WHITE, D65, Primaries};
use crate::transfer::{acescc_to_lin, acescct_to_lin, lin_to_acescc, lin_to_acescct};

/// A camera (or ACES) log encoding.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LogCurve {
    SLog3,
    LogC3,
    LogC4,
    VLog,
    CanonLog,
    CanonLog2,
    CanonLog3,
    AppleLog,
    Log3G10,
    FLog,
    FLog2,
    DaVinciIntermediate,
    BmdFilmGen5,
    AcesCct,
    AcesCc,
    /// Kodak Cineon printing density (10-bit film scans, DPX "printing
    /// density"): code 95 is black, 685 is reference white (1.0).
    Cineon,
}

impl LogCurve {
    /// Every curve, in menu order.
    pub const ALL: [LogCurve; 16] = [
        LogCurve::SLog3,
        LogCurve::LogC3,
        LogCurve::LogC4,
        LogCurve::VLog,
        LogCurve::CanonLog,
        LogCurve::CanonLog2,
        LogCurve::CanonLog3,
        LogCurve::AppleLog,
        LogCurve::Log3G10,
        LogCurve::FLog,
        LogCurve::FLog2,
        LogCurve::DaVinciIntermediate,
        LogCurve::BmdFilmGen5,
        LogCurve::AcesCct,
        LogCurve::AcesCc,
        LogCurve::Cineon,
    ];

    /// Short machine name (`slog3`, `logc3`, …).
    pub fn slug(self) -> &'static str {
        match self {
            LogCurve::SLog3 => "slog3",
            LogCurve::LogC3 => "logc3",
            LogCurve::LogC4 => "logc4",
            LogCurve::VLog => "vlog",
            LogCurve::CanonLog => "clog",
            LogCurve::CanonLog2 => "clog2",
            LogCurve::CanonLog3 => "clog3",
            LogCurve::AppleLog => "applelog",
            LogCurve::Log3G10 => "log3g10",
            LogCurve::FLog => "flog",
            LogCurve::FLog2 => "flog2",
            LogCurve::DaVinciIntermediate => "davinci_intermediate",
            LogCurve::BmdFilmGen5 => "bmdfilm_gen5",
            LogCurve::AcesCct => "acescct",
            LogCurve::AcesCc => "acescc",
            LogCurve::Cineon => "cineon",
        }
    }

    /// Display name.
    pub fn name(self) -> &'static str {
        match self {
            LogCurve::SLog3 => "Sony S-Log3",
            LogCurve::LogC3 => "ARRI LogC3 (EI 800)",
            LogCurve::LogC4 => "ARRI LogC4",
            LogCurve::VLog => "Panasonic V-Log",
            LogCurve::CanonLog => "Canon Log",
            LogCurve::CanonLog2 => "Canon Log 2",
            LogCurve::CanonLog3 => "Canon Log 3",
            LogCurve::AppleLog => "Apple Log",
            LogCurve::Log3G10 => "RED Log3G10",
            LogCurve::FLog => "Fujifilm F-Log",
            LogCurve::FLog2 => "Fujifilm F-Log2",
            LogCurve::DaVinciIntermediate => "DaVinci Intermediate",
            LogCurve::BmdFilmGen5 => "Blackmagic Film Gen 5",
            LogCurve::AcesCct => "ACEScct",
            LogCurve::AcesCc => "ACEScc",
            LogCurve::Cineon => "Cineon (printing density)",
        }
    }

    /// By slug or a loose spelling (`"S-Log3"`, `"logc"`, `"c-log3"`).
    pub fn from_name(s: &str) -> Option<LogCurve> {
        let k: String = s.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_lowercase();
        Some(match k.as_str() {
            "slog3" | "sonyslog3" => LogCurve::SLog3,
            "logc" | "logc3" | "arrilogc" | "arrilogc3" | "alexalogc" => LogCurve::LogC3,
            "logc4" | "arrilogc4" => LogCurve::LogC4,
            "vlog" | "panasonicvlog" => LogCurve::VLog,
            "clog" | "canonlog" | "canonlog1" => LogCurve::CanonLog,
            "clog2" | "canonlog2" => LogCurve::CanonLog2,
            "clog3" | "canonlog3" => LogCurve::CanonLog3,
            "applelog" | "alog" => LogCurve::AppleLog,
            "log3g10" | "redlog3g10" | "redlog" => LogCurve::Log3G10,
            "flog" | "fujiflog" => LogCurve::FLog,
            "flog2" | "fujiflog2" => LogCurve::FLog2,
            "davinciintermediate" | "dintermediate" | "davinci" => LogCurve::DaVinciIntermediate,
            "bmdfilmgen5" | "bmdfilm" | "blackmagicfilmgen5" | "blackmagicfilm" | "bmdfilmgeneration5" => LogCurve::BmdFilmGen5,
            "acescct" => LogCurve::AcesCct,
            "acescc" => LogCurve::AcesCc,
            "cineon" | "printingdensity" | "dpxlog" | "kodaklog" | "filmlog" => LogCurve::Cineon,
            _ => return None,
        })
    }

    /// Index used by the WGSL `camera_decode` function
    /// (`shaders/camera.wgsl`).
    pub fn shader_id(self) -> u32 {
        LogCurve::ALL.iter().position(|c| *c == self).unwrap_or(0) as u32
    }

    /// The camera's native gamut (what it records with this curve unless
    /// told otherwise).
    pub fn native_gamut(self) -> Gamut {
        match self {
            LogCurve::SLog3 => Gamut::SGamut3Cine,
            LogCurve::LogC3 => Gamut::ArriWideGamut3,
            LogCurve::LogC4 => Gamut::ArriWideGamut4,
            LogCurve::VLog => Gamut::VGamut,
            LogCurve::CanonLog | LogCurve::CanonLog2 | LogCurve::CanonLog3 => Gamut::CinemaGamut,
            LogCurve::AppleLog => Gamut::Rec2020,
            LogCurve::Log3G10 => Gamut::RedWideGamut,
            LogCurve::FLog | LogCurve::FLog2 => Gamut::Rec2020,
            LogCurve::DaVinciIntermediate => Gamut::DaVinciWideGamut,
            LogCurve::BmdFilmGen5 => Gamut::BmdWideGamutGen5,
            LogCurve::AcesCct | LogCurve::AcesCc => Gamut::AcesAp1,
            LogCurve::Cineon => Gamut::Rec709,
        }
    }

    /// Full-range code value (0..1) → scene-linear reflectance.
    pub fn decode(self, v: f32) -> f32 {
        match self {
            LogCurve::SLog3 => {
                if v >= 171.210_29 / 1023.0 {
                    10f32.powf((v * 1023.0 - 420.0) / 261.5) * 0.19 - 0.01
                } else {
                    (v * 1023.0 - 95.0) * 0.011_25 / (171.210_29 - 95.0)
                }
            }
            LogCurve::LogC3 => {
                let (cut, a, b, c, d, e, f) = LOGC3;
                if v > e * cut + f { (10f32.powf((v - d) / c) - b) / a } else { (v - f) / e }
            }
            LogCurve::LogC4 => {
                let (a, b, c, s, t) = logc4();
                if v >= 0.0 { ((14.0 * (v - c) / b + 6.0).exp2() - 64.0) / a } else { v * s + t }
            }
            LogCurve::VLog => {
                if v < 0.181 {
                    (v - 0.125) / 5.6
                } else {
                    10f32.powf((v - 0.598_206) / 0.241_514) - 0.008_73
                }
            }
            LogCurve::CanonLog => {
                let x = ire(v);
                0.9 * if x < 0.125_122_48 {
                    -(10f32.powf((0.125_122_48 - x) / 0.453_101_79) - 1.0) / 10.1596
                } else {
                    (10f32.powf((x - 0.125_122_48) / 0.453_101_79) - 1.0) / 10.1596
                }
            }
            LogCurve::CanonLog2 => {
                let x = ire(v);
                0.9 * if x < 0.092_864_125 {
                    -(10f32.powf((0.092_864_125 - x) / 0.241_360_77) - 1.0) / 87.099_375
                } else {
                    (10f32.powf((x - 0.092_864_125) / 0.241_360_77) - 1.0) / 87.099_375
                }
            }
            LogCurve::CanonLog3 => {
                let x = ire(v);
                0.9 * if x < 0.097_465_473 {
                    -(10f32.powf((0.127_839_01 - x) / 0.367_268_45) - 1.0) / 14.983_25
                } else if x <= 0.152_778_91 {
                    (x - 0.125_122_19) / 1.975_479_8
                } else {
                    (10f32.powf((x - 0.122_405_37) / 0.367_268_45) - 1.0) / 14.983_25
                }
            }
            LogCurve::AppleLog => {
                let (r0, rt, sigma, beta, gamma, delta) = APPLE_LOG;
                let pt = sigma * (rt - r0) * (rt - r0);
                if v >= pt {
                    ((v - delta) / gamma).exp2() - beta
                } else if v >= 0.0 {
                    (v / sigma).sqrt() + r0
                } else {
                    r0
                }
            }
            LogCurve::Log3G10 => {
                let (a, b, c, g) = LOG3G10;
                if v < 0.0 { v / g - c } else { (10f32.powf(v / a) - 1.0) / b - c }
            }
            LogCurve::FLog => flog_decode(v, &FLOG),
            LogCurve::FLog2 => flog_decode(v, &FLOG2),
            LogCurve::DaVinciIntermediate => {
                let (a, b, c, m, _lin_cut, log_cut) = DAVINCI;
                if v <= log_cut { v / m } else { (v / c - b).exp2() - a }
            }
            LogCurve::BmdFilmGen5 => {
                let (a, b, c, d, e, lin_cut) = BMD_GEN5;
                if v < d * lin_cut + e { (v - e) / d } else { ((v - c) / a).exp() - b }
            }
            LogCurve::AcesCct => acescct_to_lin(v),
            LogCurve::AcesCc => acescc_to_lin(v),
            LogCurve::Cineon => {
                let black = cineon_black();
                (10f32.powf((v * 1023.0 - 685.0) * 0.002 / 0.6) - black) / (1.0 - black)
            }
        }
    }

    /// Scene-linear reflectance → full-range code value.
    pub fn encode(self, x: f32) -> f32 {
        match self {
            LogCurve::SLog3 => {
                if x >= 0.011_25 {
                    (420.0 + ((x + 0.01) / 0.19).log10() * 261.5) / 1023.0
                } else {
                    (x * (171.210_29 - 95.0) / 0.011_25 + 95.0) / 1023.0
                }
            }
            LogCurve::LogC3 => {
                let (cut, a, b, c, d, e, f) = LOGC3;
                if x > cut { c * (a * x + b).log10() + d } else { e * x + f }
            }
            LogCurve::LogC4 => {
                let (a, b, c, s, t) = logc4();
                if x >= t { ((a * x + 64.0).log2() - 6.0) / 14.0 * b + c } else { (x - t) / s }
            }
            LogCurve::VLog => {
                if x < 0.01 {
                    5.6 * x + 0.125
                } else {
                    0.241_514 * (x + 0.008_73).log10() + 0.598_206
                }
            }
            LogCurve::CanonLog => {
                let x = x / 0.9;
                full(if x < 0.0 {
                    -0.453_101_79 * (-x * 10.1596 + 1.0).log10() + 0.125_122_48
                } else {
                    0.453_101_79 * (x * 10.1596 + 1.0).log10() + 0.125_122_48
                })
            }
            LogCurve::CanonLog2 => {
                let x = x / 0.9;
                full(if x < 0.0 {
                    -0.241_360_77 * (-x * 87.099_375 + 1.0).log10() + 0.092_864_125
                } else {
                    0.241_360_77 * (x * 87.099_375 + 1.0).log10() + 0.092_864_125
                })
            }
            LogCurve::CanonLog3 => {
                let x = x / 0.9;
                // Breaks where the pieces meet (the decode thresholds).
                let lo = (0.097_465_473 - 0.125_122_19) / 1.975_479_8;
                let hi = (0.152_778_91 - 0.125_122_19) / 1.975_479_8;
                full(if x < lo {
                    -0.367_268_45 * (-x * 14.983_25 + 1.0).log10() + 0.127_839_01
                } else if x <= hi {
                    1.975_479_8 * x + 0.125_122_19
                } else {
                    0.367_268_45 * (x * 14.983_25 + 1.0).log10() + 0.122_405_37
                })
            }
            LogCurve::AppleLog => {
                let (r0, rt, sigma, beta, gamma, delta) = APPLE_LOG;
                if x >= rt {
                    gamma * (x + beta).log2() + delta
                } else if x >= r0 {
                    sigma * (x - r0) * (x - r0)
                } else {
                    0.0
                }
            }
            LogCurve::Log3G10 => {
                let (a, b, c, g) = LOG3G10;
                let x = x + c;
                if x < 0.0 { x * g } else { a * (x * b + 1.0).log10() }
            }
            LogCurve::FLog => flog_encode(x, &FLOG),
            LogCurve::FLog2 => flog_encode(x, &FLOG2),
            LogCurve::DaVinciIntermediate => {
                let (a, b, c, m, lin_cut, _log_cut) = DAVINCI;
                if x <= lin_cut { x * m } else { c * ((x + a).log2() + b) }
            }
            LogCurve::BmdFilmGen5 => {
                let (a, b, c, d, e, lin_cut) = BMD_GEN5;
                if x < lin_cut { d * x + e } else { a * (x + b).ln() + c }
            }
            LogCurve::AcesCct => lin_to_acescct(x),
            LogCurve::AcesCc => lin_to_acescc(x),
            LogCurve::Cineon => {
                let black = cineon_black();
                (685.0 + (x * (1.0 - black) + black).max(1e-10).log10() * 0.6 / 0.002) / 1023.0
            }
        }
    }
}

/// Linear value of Cineon code 95 before the black offset is removed.
fn cineon_black() -> f32 {
    10f32.powf((95.0 - 685.0) * 0.002 / 0.6)
}

/// LogC3 at EI 800: (cut, a, b, c, d, e, f).
const LOGC3: (f32, f32, f32, f32, f32, f32, f32) = (0.010_591, 5.555_556, 0.052_272, 0.247_190, 0.385_537, 5.367_655, 0.092_809);

/// LogC4: (a, b, c, s, t) derived from the specification's definitions.
fn logc4() -> (f32, f32, f32, f32, f32) {
    let a = ((1u32 << 18) as f32 - 16.0) / 117.45;
    let b = (1023.0 - 95.0) / 1023.0;
    let c = 95.0 / 1023.0;
    let s = (7.0 * std::f32::consts::LN_2 * (7.0f32 - 14.0 * c / b).exp2()) / (a * b);
    let t = ((14.0 * (-c / b) + 6.0).exp2() - 64.0) / a;
    (a, b, c, s, t)
}

/// Apple Log: (R0, Rt, sigma, beta, gamma, delta).
const APPLE_LOG: (f32, f32, f32, f32, f32, f32) = (-0.056_410_88, 0.01, 47.287_113, 0.009_640_52, 0.085_504_79, 0.693_369_45);

/// Log3G10 v3: (a, b, c, g).
const LOG3G10: (f32, f32, f32, f32) = (0.224_282, 155.975_33, 0.01, 15.1927);

/// DaVinci Intermediate: (A, B, C, M, lin cut, log cut).
const DAVINCI: (f32, f32, f32, f32, f32, f32) = (0.0075, 7.0, 0.073_292_48, 10.444_268_55, 0.002_624_09, 0.027_406_68);

/// Blackmagic Film Generation 5: (A, B, C, D, E, lin cut).
const BMD_GEN5: (f32, f32, f32, f32, f32, f32) = (0.086_928_76, 0.005_494_072, 0.530_013_34, 8.283_606, 0.092_465_75, 0.005);

/// F-Log family: cut1, cut2, a, b, c, d, e, f.
struct FLogK {
    cut1: f32,
    cut2: f32,
    a: f32,
    b: f32,
    c: f32,
    d: f32,
    e: f32,
    f: f32,
}

const FLOG: FLogK = FLogK { cut1: 0.000_89, cut2: 0.100_537_775, a: 0.555_556, b: 0.009_468, c: 0.344_676, d: 0.790_453, e: 8.735_631, f: 0.092_864 };
const FLOG2: FLogK = FLogK { cut1: 0.000_889, cut2: 0.100_686_685, a: 5.555_556, b: 0.064_829, c: 0.245_281, d: 0.384_316, e: 8.799_461, f: 0.092_864 };

fn flog_decode(v: f32, k: &FLogK) -> f32 {
    if v < k.cut2 { (v - k.f) / k.e } else { 10f32.powf((v - k.d) / k.c) / k.a - k.b / k.a }
}

fn flog_encode(x: f32, k: &FLogK) -> f32 {
    if x < k.cut1 { k.e * x + k.f } else { k.c * (k.a * x + k.b).log10() + k.d }
}

/// Full-range code value → Canon's IRE-normalised (legal range) value.
fn ire(v: f32) -> f32 {
    (v * 1023.0 - 64.0) / 876.0
}

/// Canon IRE-normalised value → full-range code value.
fn full(ire: f32) -> f32 {
    (ire * 876.0 + 64.0) / 1023.0
}

/// Limited-range normalised signal (0 at code 64, 1 at code 940, as a
/// video decoder hands it out) → full-range code value.
pub fn legal_to_full(v: f32) -> f32 {
    full(v)
}

/// Camera and ACES gamuts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Gamut {
    Rec709,
    Rec2020,
    P3D65,
    SGamut3,
    SGamut3Cine,
    ArriWideGamut3,
    ArriWideGamut4,
    VGamut,
    CinemaGamut,
    RedWideGamut,
    DaVinciWideGamut,
    BmdWideGamutGen5,
    AcesAp0,
    AcesAp1,
}

impl Gamut {
    pub const ALL: [Gamut; 14] = [
        Gamut::Rec709,
        Gamut::Rec2020,
        Gamut::P3D65,
        Gamut::SGamut3,
        Gamut::SGamut3Cine,
        Gamut::ArriWideGamut3,
        Gamut::ArriWideGamut4,
        Gamut::VGamut,
        Gamut::CinemaGamut,
        Gamut::RedWideGamut,
        Gamut::DaVinciWideGamut,
        Gamut::BmdWideGamutGen5,
        Gamut::AcesAp0,
        Gamut::AcesAp1,
    ];

    pub fn slug(self) -> &'static str {
        match self {
            Gamut::Rec709 => "rec709",
            Gamut::Rec2020 => "rec2020",
            Gamut::P3D65 => "p3d65",
            Gamut::SGamut3 => "sgamut3",
            Gamut::SGamut3Cine => "sgamut3cine",
            Gamut::ArriWideGamut3 => "awg3",
            Gamut::ArriWideGamut4 => "awg4",
            Gamut::VGamut => "vgamut",
            Gamut::CinemaGamut => "cinemagamut",
            Gamut::RedWideGamut => "rwg",
            Gamut::DaVinciWideGamut => "dwg",
            Gamut::BmdWideGamutGen5 => "bmdwg",
            Gamut::AcesAp0 => "ap0",
            Gamut::AcesAp1 => "ap1",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Gamut::Rec709 => "Rec. 709",
            Gamut::Rec2020 => "Rec. 2020",
            Gamut::P3D65 => "P3-D65",
            Gamut::SGamut3 => "Sony S-Gamut3",
            Gamut::SGamut3Cine => "Sony S-Gamut3.Cine",
            Gamut::ArriWideGamut3 => "ARRI Wide Gamut 3",
            Gamut::ArriWideGamut4 => "ARRI Wide Gamut 4",
            Gamut::VGamut => "Panasonic V-Gamut",
            Gamut::CinemaGamut => "Canon Cinema Gamut",
            Gamut::RedWideGamut => "REDWideGamutRGB",
            Gamut::DaVinciWideGamut => "DaVinci Wide Gamut",
            Gamut::BmdWideGamutGen5 => "Blackmagic Wide Gamut (Gen 5)",
            Gamut::AcesAp0 => "ACES AP0",
            Gamut::AcesAp1 => "ACES AP1",
        }
    }

    pub fn from_name(s: &str) -> Option<Gamut> {
        let k: String = s.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_lowercase();
        Some(match k.as_str() {
            "rec709" | "bt709" | "srgb" | "709" => Gamut::Rec709,
            "rec2020" | "bt2020" | "2020" | "fgamut" | "applelog" => Gamut::Rec2020,
            "p3d65" | "p3" | "displayp3" | "dcip3d65" => Gamut::P3D65,
            "sgamut3" | "sonysgamut3" => Gamut::SGamut3,
            "sgamut3cine" | "sonysgamut3cine" => Gamut::SGamut3Cine,
            "awg3" | "awg" | "arriwidegamut3" | "arriwidegamut" | "alexawidegamut" => Gamut::ArriWideGamut3,
            "awg4" | "arriwidegamut4" => Gamut::ArriWideGamut4,
            "vgamut" | "panasonicvgamut" => Gamut::VGamut,
            "cinemagamut" | "canoncinemagamut" | "cgamut" => Gamut::CinemaGamut,
            "rwg" | "redwidegamut" | "redwidegamutrgb" => Gamut::RedWideGamut,
            "dwg" | "davinciwidegamut" => Gamut::DaVinciWideGamut,
            "bmdwg" | "bmdwidegamut" | "blackmagicwidegamut" | "bmdwidegamutgen5" => Gamut::BmdWideGamutGen5,
            "ap0" | "aces" | "aces20651" | "acesap0" => Gamut::AcesAp0,
            "ap1" | "acescg" | "acesap1" => Gamut::AcesAp1,
            _ => return None,
        })
    }

    pub fn primaries(self) -> Primaries {
        let p = |r: [f64; 2], g: [f64; 2], b: [f64; 2]| Primaries { red: r, green: g, blue: b, white: D65 };
        match self {
            Gamut::Rec709 => Primaries::BT709,
            Gamut::Rec2020 => Primaries::BT2020,
            Gamut::P3D65 => Primaries::P3_D65,
            Gamut::SGamut3 => p([0.730, 0.280], [0.140, 0.855], [0.100, -0.050]),
            Gamut::SGamut3Cine => p([0.766, 0.275], [0.225, 0.800], [0.089, -0.087]),
            Gamut::ArriWideGamut3 => p([0.6840, 0.3130], [0.2210, 0.8480], [0.0861, -0.1020]),
            Gamut::ArriWideGamut4 => p([0.7347, 0.2653], [0.1424, 0.8576], [0.0991, -0.0308]),
            Gamut::VGamut => p([0.730, 0.280], [0.165, 0.840], [0.100, -0.030]),
            Gamut::CinemaGamut => p([0.740, 0.270], [0.170, 1.140], [0.080, -0.100]),
            Gamut::RedWideGamut => p([0.780_308, 0.304_253], [0.121_595, 1.493_994], [0.095_612, -0.084_589]),
            Gamut::DaVinciWideGamut => p([0.8000, 0.3130], [0.1682, 0.9877], [0.0790, -0.1155]),
            Gamut::BmdWideGamutGen5 => p([0.717_721_5, 0.317_118_1], [0.228_041, 0.861_569], [0.100_584_1, -0.082_045_2]),
            Gamut::AcesAp0 => Primaries::ACES_AP0,
            Gamut::AcesAp1 => Primaries::ACES_AP1,
        }
    }

    /// Whether the white point is ACES's (≈ D60) rather than D65.
    pub fn aces_white(self) -> bool {
        self.primaries().white == ACES_WHITE
    }
}

/// A camera colour space: its log curve and gamut, e.g. S-Log3 /
/// S-Gamut3.Cine.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CameraSpace {
    pub curve: LogCurve,
    pub gamut: Gamut,
}

impl CameraSpace {
    /// `"slog3_sgamut3cine"`; a bare curve (`"slog3"`) takes the camera's
    /// native gamut; `"logc3/awg3"`, `"S-Log3 S-Gamut3"` also work.
    pub fn parse(s: &str) -> Option<CameraSpace> {
        let s = s.trim();
        if let Some(c) = LogCurve::from_name(s) {
            return Some(CameraSpace { curve: c, gamut: c.native_gamut() });
        }
        for sep in ['_', '/', ' ', ','] {
            for (i, _) in s.match_indices(sep) {
                let (a, b) = (&s[..i], &s[i + 1..]);
                if let (Some(curve), Some(gamut)) = (LogCurve::from_name(a), Gamut::from_name(b)) {
                    return Some(CameraSpace { curve, gamut });
                }
            }
        }
        None
    }

    pub fn slug(self) -> String {
        format!("{}_{}", self.curve.slug(), self.gamut.slug())
    }

    pub fn name(self) -> String {
        format!("{} / {}", self.curve.name(), self.gamut.name())
    }

    /// The usual pairings, for menus and the AI.
    pub fn presets() -> Vec<CameraSpace> {
        use Gamut as G;
        use LogCurve as L;
        let pairs = [
            (L::SLog3, G::SGamut3Cine),
            (L::SLog3, G::SGamut3),
            (L::LogC3, G::ArriWideGamut3),
            (L::LogC4, G::ArriWideGamut4),
            (L::VLog, G::VGamut),
            (L::CanonLog, G::CinemaGamut),
            (L::CanonLog2, G::CinemaGamut),
            (L::CanonLog3, G::CinemaGamut),
            (L::AppleLog, G::Rec2020),
            (L::Log3G10, G::RedWideGamut),
            (L::FLog, G::Rec2020),
            (L::FLog2, G::Rec2020),
            (L::DaVinciIntermediate, G::DaVinciWideGamut),
            (L::BmdFilmGen5, G::BmdWideGamutGen5),
            (L::AcesCct, G::AcesAp1),
        ];
        pairs.into_iter().map(|(curve, gamut)| CameraSpace { curve, gamut }).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Published 18 % grey code values (10-bit, full range).
    #[test]
    fn grey_points() {
        let cases: [(LogCurve, f32); 13] = [
            (LogCurve::SLog3, 420.0),
            (LogCurve::LogC3, 400.0),
            (LogCurve::LogC4, 284.8),
            (LogCurve::VLog, 433.0),
            (LogCurve::CanonLog, 364.8),
            (LogCurve::CanonLog2, 412.9),
            (LogCurve::CanonLog3, 364.8),
            (LogCurve::AppleLog, 499.5),
            (LogCurve::Log3G10, 341.0),
            (LogCurve::FLog, 470.0),
            (LogCurve::FLog2, 400.0),
            (LogCurve::DaVinciIntermediate, 344.0),
            (LogCurve::BmdFilmGen5, 392.4),
        ];
        for (c, code) in cases {
            let v = c.encode(0.18) * 1023.0;
            assert!((v - code).abs() < 2.0, "{c:?}: 18% grey at code {v}, expected about {code}");
            assert!((c.decode(code / 1023.0) - 0.18).abs() < 0.004, "{c:?}: decode");
        }
    }

    #[test]
    fn round_trips_and_monotone() {
        for c in LogCurve::ALL {
            let mut last = f32::MIN;
            for i in 0..=400 {
                let x = -0.01 + i as f32 / 400.0 * 16.0;
                let v = c.encode(x);
                let back = c.decode(v);
                // Apple Log is clamped below R0; ACEScct/ACEScc have their own floors.
                if x > 0.0 {
                    assert!((back - x).abs() <= 2e-4 * x.max(1.0), "{c:?} at {x}: {v} -> {back}");
                }
                assert!(v >= last - 1e-6, "{c:?} not monotone at {x}");
                last = v;
            }
        }
    }

    #[test]
    fn cineon_points() {
        assert!((LogCurve::Cineon.decode(685.0 / 1023.0) - 1.0).abs() < 1e-5);
        assert!(LogCurve::Cineon.decode(95.0 / 1023.0).abs() < 1e-6);
    }

    #[test]
    fn continuous_at_breaks() {
        for c in LogCurve::ALL {
            for i in 1..2000 {
                let v = i as f32 / 2000.0;
                let a = c.decode(v - 1e-4);
                let b = c.decode(v + 1e-4);
                assert!((b - a).abs() < 0.05 * a.abs().max(0.02), "{c:?} jumps at code {v}: {a} -> {b}");
            }
        }
    }

    #[test]
    fn names_parse() {
        for c in LogCurve::ALL {
            assert_eq!(LogCurve::from_name(c.slug()), Some(c));
        }
        for g in Gamut::ALL {
            assert_eq!(Gamut::from_name(g.slug()), Some(g));
        }
        assert_eq!(CameraSpace::parse("slog3_sgamut3cine"), Some(CameraSpace { curve: LogCurve::SLog3, gamut: Gamut::SGamut3Cine }));
        assert_eq!(CameraSpace::parse("S-Log3 / S-Gamut3"), Some(CameraSpace { curve: LogCurve::SLog3, gamut: Gamut::SGamut3 }));
        assert_eq!(CameraSpace::parse("logc").map(|c| c.gamut), Some(Gamut::ArriWideGamut3));
        assert_eq!(CameraSpace::parse("davinci_intermediate_dwg").map(|c| c.curve), Some(LogCurve::DaVinciIntermediate));
        for p in CameraSpace::presets() {
            assert_eq!(CameraSpace::parse(&p.slug()), Some(p));
        }
    }

    #[test]
    fn gamut_white_maps_to_white() {
        for g in Gamut::ALL {
            let m = crate::primaries::rgb_to_rgb(&g.primaries(), &Primaries::BT709);
            let w = m.apply([1.0, 1.0, 1.0]);
            for c in w {
                assert!((c - 1.0).abs() < 1e-3, "{g:?}: {w:?}");
            }
        }
    }
}
