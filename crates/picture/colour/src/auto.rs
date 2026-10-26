// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Automatic tools: white balance, levels and colour matching.

use crate::image::{Image, unpremultiply};
use crate::math::{Mat3, Rgb, dot3};
use crate::primaries::{Primaries, TINT_DUV_SCALE, bradford, cct_duv_to_xy, rgb_to_rgb, xy_to_cct_duv, xyz_to_xy};

/// Pixels used by the estimators: straight colour of mostly opaque pixels,
/// subsampled to at most ~250 000 so estimates stay fast on 4K frames.
fn samples(img: &Image) -> Vec<Rgb> {
    let step = (img.len() / 250_000).max(1);
    img.pixels
        .iter()
        .step_by(step)
        .filter(|p| p[3] > 0.5)
        .map(|p| {
            let s = unpremultiply(*p);
            [s[0], s[1], s[2]]
        })
        .collect()
}

/// Value at fraction `q` (0..1) of `v`, reordering `v`.
fn percentile(v: &mut [f32], q: f32) -> f32 {
    if v.is_empty() {
        return 0.0;
    }
    let k = ((v.len() - 1) as f32 * q.clamp(0.0, 1.0)).round() as usize;
    *v.select_nth_unstable_by(k, f32::total_cmp).1
}

// --------------------------------------------------------- white balance

/// Result of [`auto_white_balance`]: settings for [`crate::Basic`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WhiteBalanceEstimate {
    /// Estimated colour of the light, working linear RGB, luminance 1.
    pub illuminant: Rgb,
    pub temperature: f32,
    pub tint: f32,
}

/// Estimates the scene light from a blend of grey-world (the mean of
/// well-exposed pixels) and white-patch (the mean of the brightest 2 % of
/// unclipped pixels). `white_patch_weight` 0..1 sets the blend; 0.5 is a
/// good default because grey-world fails on scenes dominated by one colour
/// and white-patch fails when there is no white object.
///
/// The returned temperature / tint make [`crate::primaries::white_balance_matrix`]
/// map the estimated light exactly to the working white: the matrix adapts
/// `W(T, tint)` to the neutral reference `W₀`, so we choose
/// `W = Bradford(white → W₀)(illuminant)`, which von Kries scaling then
/// sends to the working white.
pub fn auto_white_balance(img: &Image, working: &Primaries, white_patch_weight: f32) -> Option<WhiteBalanceEstimate> {
    let lw = working.luminance_weights();
    let px = samples(img);
    let max = px.iter().map(|c| c[0].max(c[1]).max(c[2])).fold(0.0f32, f32::max);
    if max <= 0.0 {
        return None;
    }
    // Footage decoded from display-referred video clips at 1.0; pixels near
    // that level have lost their colour and would bias the estimate. Scene-
    // referred material that never reaches 1.0 has no such ceiling.
    let clip = if max >= 0.99 { 0.98 * max } else { f32::INFINITY };
    let usable: Vec<Rgb> = px
        .into_iter()
        .filter(|c| {
            let y = dot3(*c, lw);
            y > 0.01 * max && c.iter().all(|&v| v < clip && v >= 0.0)
        })
        .collect();
    if usable.len() < 16 {
        return None;
    }
    let mean = |set: &[Rgb]| {
        let mut s = [0.0f64; 3];
        for c in set {
            for i in 0..3 {
                s[i] += c[i] as f64;
            }
        }
        let n = set.len() as f64;
        [(s[0] / n) as f32, (s[1] / n) as f32, (s[2] / n) as f32]
    };
    let unit = |c: Rgb| {
        let y = dot3(c, lw).max(1e-9);
        c.map(|v| v / y)
    };
    let grey = unit(mean(&usable));
    let mut ys: Vec<f32> = usable.iter().map(|c| dot3(*c, lw)).collect();
    let cut = percentile(&mut ys, 0.98);
    let bright: Vec<Rgb> = usable.iter().copied().filter(|c| dot3(*c, lw) >= cut).collect();
    let patch = unit(mean(&bright));
    let k = white_patch_weight.clamp(0.0, 1.0);
    let illum = unit([0, 1, 2].map(|i| grey[i] + (patch[i] - grey[i]) * k));

    let xyz = working.rgb_to_xyz().mul_vec64(illum.map(|v| v as f64));
    let w0 = cct_duv_to_xy(crate::primaries::NEUTRAL_KELVIN as f64, 0.0);
    let w = xyz_to_xy(bradford(working.white, w0).mul_vec64(xyz));
    let (t, duv) = xy_to_cct_duv(w);
    Some(WhiteBalanceEstimate {
        illuminant: illum,
        temperature: t as f32,
        tint: (duv / TINT_DUV_SCALE).clamp(-1.0, 1.0) as f32,
    })
}

// ---------------------------------------------------------------- levels

/// Linear black and white points: `out = (in − black) / (white − black)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Levels {
    pub black: f32,
    pub white: f32,
}

impl Default for Levels {
    fn default() -> Levels {
        Levels { black: 0.0, white: 1.0 }
    }
}

impl Levels {
    pub fn is_identity(&self) -> bool {
        self.black == 0.0 && self.white == 1.0
    }

    #[inline]
    pub fn apply(&self, c: Rgb) -> Rgb {
        let k = 1.0 / (self.white - self.black).max(1e-6);
        c.map(|v| (v - self.black) * k)
    }
}

/// Robust levels from luminance percentiles (`low`/`high` as fractions,
/// e.g. 0.005 and 0.995), so a few stray pixels do not decide the result.
/// The white point is never set below 18 % grey, to avoid blowing out
/// genuinely dark scenes.
pub fn auto_levels(img: &Image, working: &Primaries, low: f32, high: f32) -> Levels {
    let lw = working.luminance_weights();
    let mut ys: Vec<f32> = samples(img).iter().map(|c| dot3(*c, lw)).collect();
    if ys.is_empty() {
        return Levels::default();
    }
    let black = percentile(&mut ys, low).max(0.0);
    let white = percentile(&mut ys, high).max(0.18).max(black + 0.01);
    Levels { black, white }
}

// --------------------------------------------------------- match colour

/// Linear sRGB/BT.709 → Oklab (Björn Ottosson, 2020, public domain
/// matrices). Oklab is a simple perceptual space with good hue linearity,
/// which suits statistics transfer.
pub fn linear709_to_oklab(c: Rgb) -> Rgb {
    let l = 0.412_221_46 * c[0] + 0.536_332_55 * c[1] + 0.051_445_995 * c[2];
    let m = 0.211_903_5 * c[0] + 0.680_699_5 * c[1] + 0.107_396_96 * c[2];
    let s = 0.088_302_46 * c[0] + 0.281_718_85 * c[1] + 0.629_978_7 * c[2];
    let (l, m, s) = (l.cbrt(), m.cbrt(), s.cbrt());
    [
        0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
        1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    ]
}

pub fn oklab_to_linear709(c: Rgb) -> Rgb {
    let l = c[0] + 0.396_337_78 * c[1] + 0.215_803_76 * c[2];
    let m = c[0] - 0.105_561_346 * c[1] - 0.063_854_17 * c[2];
    let s = c[0] - 0.089_484_18 * c[1] - 1.291_485_5 * c[2];
    let (l, m, s) = (l * l * l, m * m * m, s * s * s);
    [
        4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_94 * s,
        -1.268_438 * l + 2.609_757_4 * m - 0.341_319_38 * s,
        -0.004_196_086_3 * l - 0.703_418_6 * m + 1.707_614_7 * s,
    ]
}

/// Mean and standard deviation of a frame in Oklab.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColourStats {
    pub mean: Rgb,
    pub std: Rgb,
}

pub fn colour_stats(img: &Image, working: &Primaries) -> ColourStats {
    let to709 = rgb_to_rgb(working, &Primaries::BT709);
    let px = samples(img);
    let mut s = [0.0f64; 3];
    let mut s2 = [0.0f64; 3];
    for c in &px {
        let lab = linear709_to_oklab(to709.apply(*c));
        for i in 0..3 {
            s[i] += lab[i] as f64;
            s2[i] += (lab[i] as f64) * (lab[i] as f64);
        }
    }
    let n = px.len().max(1) as f64;
    let mean = [0, 1, 2].map(|i| (s[i] / n) as f32);
    let std = [0, 1, 2].map(|i| ((s2[i] / n - (s[i] / n).powi(2)).max(0.0).sqrt()) as f32);
    ColourStats { mean, std }
}

/// Match colour: moves the statistics of a clip towards a reference frame
/// (mean / standard-deviation transfer per Oklab channel, after Reinhard
/// et al. 2001), mixed in by `strength` 0..1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColourMatch {
    pub source: ColourStats,
    pub reference: ColourStats,
    pub strength: f32,
}

impl ColourMatch {
    pub fn new(clip: &Image, reference: &Image, working: &Primaries, strength: f32) -> ColourMatch {
        ColourMatch { source: colour_stats(clip, working), reference: colour_stats(reference, working), strength }
    }

    pub fn prepare(&self, working: &Primaries) -> PreparedMatch {
        let mut scale = [1.0; 3];
        for (i, s) in scale.iter_mut().enumerate() {
            // Limit the stretch: a flat source channel would otherwise
            // amplify noise without bound.
            *s = if self.source.std[i] > 1e-5 {
                (self.reference.std[i] / self.source.std[i]).clamp(0.25, 4.0)
            } else {
                1.0
            };
        }
        PreparedMatch {
            to709: rgb_to_rgb(working, &Primaries::BT709),
            from709: rgb_to_rgb(&Primaries::BT709, working),
            src_mean: self.source.mean,
            ref_mean: self.reference.mean,
            scale,
            strength: self.strength.clamp(0.0, 1.0),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PreparedMatch {
    pub to709: Mat3,
    pub from709: Mat3,
    pub src_mean: Rgb,
    pub ref_mean: Rgb,
    pub scale: Rgb,
    pub strength: f32,
}

impl PreparedMatch {
    pub fn apply(&self, lin: Rgb) -> Rgb {
        if self.strength == 0.0 {
            return lin;
        }
        let lab = linear709_to_oklab(self.to709.apply(lin));
        let mut out = [0.0; 3];
        for i in 0..3 {
            let t = (lab[i] - self.src_mean[i]) * self.scale[i] + self.ref_mean[i];
            out[i] = lab[i] + self.strength * (t - lab[i]);
        }
        self.from709.apply(oklab_to_linear709(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primaries::white_balance_matrix;

    fn scene(tint: Rgb) -> Image {
        // A varied scene (greys, colours, a white card) under coloured light.
        Image::from_fn(64, 64, |x, y| {
            let base = match (x / 16 + y / 16 * 4) % 6 {
                0 => [0.18, 0.18, 0.18],
                1 => [0.4, 0.3, 0.2],
                2 => [0.2, 0.3, 0.4],
                3 => [0.9, 0.9, 0.9],
                // Complementary pairs, so the scene averages to grey.
                4 => [0.25, 0.35, 0.3],
                _ => [0.35, 0.25, 0.3],
            };
            [base[0] * tint[0], base[1] * tint[1], base[2] * tint[2], 1.0]
        })
    }

    #[test]
    fn oklab_round_trip_and_white() {
        let w = linear709_to_oklab([1.0; 3]);
        assert!((w[0] - 1.0).abs() < 1e-3 && w[1].abs() < 1e-3 && w[2].abs() < 1e-3);
        let c = [0.2, 0.5, 0.9];
        let r = oklab_to_linear709(linear709_to_oklab(c));
        for i in 0..3 {
            assert!((r[i] - c[i]).abs() < 1e-4);
        }
    }

    #[test]
    fn auto_white_balance_neutralises_a_cast() {
        let p = Primaries::BT709;
        let img = scene([1.0, 0.85, 0.6]); // warm light
        let est = auto_white_balance(&img, &p, 0.5).unwrap();
        assert!(est.temperature < 5000.0, "{est:?}");
        let m = white_balance_matrix(&p, est.temperature, est.tint);
        let card = m.apply([0.9, 0.9 * 0.85, 0.9 * 0.6]);
        let spread = card[0].max(card[1]).max(card[2]) / card[0].min(card[1]).min(card[2]);
        assert!(spread < 1.03, "{card:?}");
        // A neutral scene gives (almost) neutral settings.
        let n = auto_white_balance(&scene([1.0; 3]), &p, 0.5).unwrap();
        let m = white_balance_matrix(&p, n.temperature, n.tint).apply([0.5; 3]);
        assert!((m[0] - m[2]).abs() < 0.01, "{n:?} {m:?}");
    }

    #[test]
    fn auto_levels_stretch() {
        let img = Image::from_fn(100, 10, |x, _| {
            let v = 0.05 + 0.5 * x as f32 / 99.0;
            [v, v, v, 1.0]
        });
        let l = auto_levels(&img, &Primaries::BT709, 0.0, 1.0);
        assert!((l.black - 0.05).abs() < 1e-4 && (l.white - 0.55).abs() < 1e-4, "{l:?}");
        let o = l.apply([0.55; 3]);
        assert!((o[0] - 1.0).abs() < 1e-4);
    }

    #[test]
    fn match_colour_transfers_statistics() {
        let p = Primaries::BT709;
        let clip = scene([1.0, 0.9, 0.7]);
        let reference = scene([0.7, 0.9, 1.1]);
        let m = ColourMatch::new(&clip, &reference, &p, 1.0);
        let prep = m.prepare(&p);
        let mut out = clip.clone();
        for px in out.pixels.iter_mut() {
            let c = prep.apply([px[0], px[1], px[2]]);
            *px = [c[0], c[1], c[2], 1.0];
        }
        let s = colour_stats(&out, &p);
        for i in 0..3 {
            assert!((s.mean[i] - m.reference.mean[i]).abs() < 0.01, "{s:?} vs {:?}", m.reference);
        }
        let half = ColourMatch { strength: 0.0, ..m }.prepare(&p);
        assert_eq!(half.apply([0.3, 0.2, 0.1]), [0.3, 0.2, 0.1]);
    }
}
