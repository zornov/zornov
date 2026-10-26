// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Keying against any colour, not only a green or blue screen.
//!
//! A chroma key measures how far a pixel's colour is from the key in the
//! chroma plane, which is right for a saturated screen (shading changes
//! mostly the luma, so it is left out) and wrong for a plain white, black
//! or grey backdrop: the key has no chroma to measure, so every near-grey
//! pixel looks "the same" and the answer is noise. [`distance`] blends the
//! two: the more neutral the key, the more the luma difference counts and
//! the less the chroma difference is stretched.
//!
//! [`unmix`] takes the key colour back out of a partly transparent edge
//! pixel (`observed = a * fg + (1 - a) * key`), which is the edge treatment
//! a white or black key needs (no dark or light fringe round the subject),
//! and [`detect_plain_background`] finds a backdrop colour from the picture's
//! border for the automatic "remove a plain background".

use crate::image::{Image, linear_to_srgb, unpremultiply};
use crate::math::smoothstep;

/// sRGB-encoded luma (Rec. 709) and chroma (Cb, Cr) of a straight linear
/// colour.
pub fn ycc(c: [f32; 3]) -> (f32, [f32; 2]) {
    let e = c.map(|v| linear_to_srgb(v.clamp(0.0, 1.0)));
    let y = 0.2126 * e[0] + 0.7152 * e[1] + 0.0722 * e[2];
    (y, [(e[2] - y) / 1.8556, (e[0] - y) / 1.5748])
}

/// How neutral (white, black or grey) a key colour is: 1 for a grey, 0 for
/// anything with noticeable colour, with a smooth change between.
pub fn neutrality(key: [f32; 3]) -> f32 {
    let (_, kc) = ycc(key);
    1.0 - smoothstep(0.03, 0.12, (kc[0] * kc[0] + kc[1] * kc[1]).sqrt())
}

/// Distance of a straight linear colour from the key: 0 is the key, about
/// 1 is a clearly different colour. For a saturated key this is the chroma
/// distance relative to the key's saturation (as before); for a neutral key
/// it is the colour difference plus the luma difference.
pub fn distance(c: [f32; 3], key: [f32; 3]) -> f32 {
    let (ky, kc) = ycc(key);
    let (y, cc) = ycc(c);
    let ks = (kc[0] * kc[0] + kc[1] * kc[1]).sqrt();
    let w = 1.0 - smoothstep(0.03, 0.12, ks);
    let norm = ks.max(0.02) + (0.35 - ks.max(0.02)) * w;
    let dc = ((cc[0] - kc[0]).powi(2) + (cc[1] - kc[1]).powi(2)).sqrt() / norm;
    let dy = (y - ky).abs() * w;
    (dc * dc + dy * dy).sqrt()
}

/// The foreground colour under a partly transparent pixel: takes
/// `(1 - alpha)` of the key back out of the observed colour. Below a small
/// alpha the pixel is almost invisible and noisy to divide, so it is left.
pub fn unmix(c: [f32; 3], key: [f32; 3], alpha: f32) -> [f32; 3] {
    if alpha >= 0.999 || alpha < 0.04 {
        return c;
    }
    std::array::from_fn(|i| ((c[i] - (1.0 - alpha) * key[i]) / alpha).max(0.0))
}

/// A plain backdrop found from a picture's border.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlainBackground {
    /// Straight linear colour.
    pub colour: [f32; 3],
    /// Share of border pixels that are this colour (0..1).
    pub share: f32,
    /// How much those pixels vary, in [`distance`] units (a clean white
    /// sheet is near 0.01; a noisy or shaded backdrop is 0.1 and up).
    pub spread: f32,
    /// Whether the colour is white, black or grey.
    pub neutral: bool,
}

impl PlainBackground {
    /// `#rrggbb` of the colour.
    pub fn hex(&self) -> String {
        let e = self.colour.map(|v| (linear_to_srgb(v.clamp(0.0, 1.0)) * 255.0).round() as u8);
        format!("#{:02x}{:02x}{:02x}", e[0], e[1], e[2])
    }

    /// A name for the colour a person would use.
    pub fn name(&self) -> &'static str {
        let (y, kc) = ycc(self.colour);
        if self.neutral {
            if y > 0.85 {
                "white"
            } else if y < 0.15 {
                "black"
            } else {
                "grey"
            }
        } else {
            let (cb, cr) = (kc[0], kc[1]);
            // Green screens are the common case among coloured backdrops.
            if cr < 0.0 && cb < 0.0 {
                "green"
            } else if cb > 0.0 && cr < 0.05 {
                "blue"
            } else if cr > 0.0 && cb < 0.0 {
                "red or orange"
            } else {
                "coloured"
            }
        }
    }

    /// Tolerance for the chroma key: wide enough for the backdrop's own
    /// variation, never so wide that light details go with it.
    pub fn tolerance(&self) -> f32 {
        if self.neutral { (self.spread * 1.6 + 0.04).clamp(0.06, 0.2) } else { (self.spread * 1.6 + 0.05).clamp(0.08, 0.3) }
    }

    /// Softness (the ramp above the tolerance). A white, black or grey
    /// backdrop gets a wide ramp: a thin dark or bright detail half over
    /// it then keeps part of its transparency and has the backdrop taken
    /// out of its colour, instead of turning into a grey line.
    pub fn softness(&self) -> f32 {
        if self.neutral { 0.3 } else { (self.tolerance() * 0.6).clamp(0.06, 0.18) }
    }
}

/// Finds the backdrop colour from the pixels along the picture's border:
/// `None` when there is no dominant colour there (a busy picture), so the
/// caller can say so instead of keying something arbitrary. `img` is a
/// straight-or-premultiplied linear picture (premultiplied is undone).
pub fn detect_plain_background(img: &Image) -> Option<PlainBackground> {
    let (w, h) = (img.width, img.height);
    if w < 8 || h < 8 {
        return None;
    }
    let band = (w.min(h) / 50).clamp(2, 24);
    let mut samples: Vec<[f32; 3]> = Vec::new();
    let mut push = |x: usize, y: usize| {
        let p = img.get(x, y);
        if p[3] > 0.5 {
            samples.push(unpremultiply(p));
        }
    };
    let (sx, sy) = ((w / 400).max(1), (h / 400).max(1));
    for y in (0..h).step_by(sy) {
        for x in (0..w).step_by(sx) {
            if x < band || y < band || x >= w - band || y >= h - band {
                push(x, y);
            }
        }
    }
    if samples.len() < 24 {
        return None;
    }
    // The most common colour: a histogram of the encoded colour in 16
    // steps a channel, then the mean of everything near its peak.
    let bin = |c: [f32; 3]| {
        let e = c.map(|v| (linear_to_srgb(v.clamp(0.0, 1.0)) * 15.0).round() as usize);
        e[0] * 256 + e[1] * 16 + e[2]
    };
    let mut hist = vec![0u32; 4096];
    for s in &samples {
        hist[bin(*s)] += 1;
    }
    let peak = (0..4096).max_by_key(|&i| hist[i])?;
    let centre = [(peak / 256) as f32 / 15.0, ((peak / 16) % 16) as f32 / 15.0, (peak % 16) as f32 / 15.0].map(crate::image::srgb_to_linear);
    let near: Vec<[f32; 3]> = samples.iter().copied().filter(|s| distance(*s, centre) < 0.2).collect();
    let share = near.len() as f32 / samples.len() as f32;
    if near.is_empty() || share < 0.55 {
        return None;
    }
    let colour: [f32; 3] = std::array::from_fn(|i| near.iter().map(|s| s[i]).sum::<f32>() / near.len() as f32);
    let mut d: Vec<f32> = near.iter().map(|s| distance(*s, colour)).collect();
    d.sort_by(f32::total_cmp);
    let spread = d[(d.len() * 9 / 10).min(d.len() - 1)];
    Some(PlainBackground { colour, share, spread, neutral: neutrality(colour) > 0.5 })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_neutral_key_separates_by_luma_and_a_green_key_by_chroma() {
        let white = [1.0; 3];
        assert!(distance([0.98, 0.98, 0.98], white) < 0.1);
        assert!(distance([0.0, 0.0, 0.0], white) > 0.9);
        assert!(distance([0.2, 0.2, 0.2], white) > 0.5);
        let black = [0.0; 3];
        assert!(distance([0.01, 0.01, 0.01], black) < 0.1);
        assert!(distance([0.9, 0.9, 0.9], black) > 0.9);
        let green = [0.05, 0.55, 0.1];
        assert!(distance([0.04, 0.45, 0.08], green) < 0.2, "shaded green stays green");
        assert!(distance([0.6, 0.3, 0.2], green) > 1.0);
        assert!(neutrality(white) > 0.99 && neutrality(black) > 0.99 && neutrality(green) < 0.01);
    }

    #[test]
    fn unmix_takes_the_key_out_of_an_edge() {
        // Half white over black observed on white: 0.5*0 + 0.5*1 = 0.5.
        let fg = unmix([0.5; 3], [1.0; 3], 0.5);
        assert!(fg.iter().all(|v| v.abs() < 1e-5), "{fg:?}");
        assert_eq!(unmix([0.3; 3], [1.0; 3], 1.0), [0.3; 3]);
    }

    #[test]
    fn the_border_gives_the_backdrop() {
        let img = Image::from_fn(64, 64, |x, y| if (16..48).contains(&x) && (16..48).contains(&y) { [0.1, 0.2, 0.6, 1.0] } else { [1.0, 1.0, 1.0, 1.0] });
        let bg = detect_plain_background(&img).expect("a white border");
        assert!(bg.neutral && bg.share > 0.99 && bg.spread < 0.02, "{bg:?}");
        assert_eq!(bg.name(), "white");
        assert_eq!(bg.hex(), "#ffffff");
        // A busy border has no backdrop.
        let busy = Image::from_fn(64, 64, |x, y| {
            let v = ((x * 7 + y * 13) % 17) as f32 / 17.0;
            [v, 1.0 - v, (v * 3.0) % 1.0, 1.0]
        });
        assert!(detect_plain_background(&busy).is_none());
    }
}
