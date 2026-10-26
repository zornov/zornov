// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A picture critic: the faults generated pictures have, measured from the
//! pixels. Exposure (crushed shadows, blown highlights), contrast,
//! colourfulness (Hasler and Süsstrunk), sharpness (variance of the
//! Laplacian), noise (Immerkær's estimate), compression blocks (8×8 grid
//! edges stronger than the rest), banding (smooth gradients broken into
//! visible steps), and how the picture is composed (where its detail sits
//! against the thirds and how balanced it is). Text in a picture is scored
//! against what it should say with [`text_accuracy`].

use serde::Serialize;

#[derive(Clone, Debug, Default, Serialize)]
pub struct PictureQuality {
    pub width: usize,
    pub height: usize,
    /// Mean brightness 0–1.
    pub exposure: f32,
    pub clipped_shadows: f32,
    pub clipped_highlights: f32,
    /// RMS contrast of brightness.
    pub contrast: f32,
    /// Hasler–Süsstrunk colourfulness (0 grey; 30+ colourful; 100 vivid).
    pub colourfulness: f32,
    /// Variance of the Laplacian of brightness (0–255 scale), as log10.
    pub sharpness: f32,
    /// Noise standard deviation in 8-bit levels.
    pub noise: f32,
    /// How much stronger edges are on the 8×8 grid than off it (1 = none).
    pub blockiness: f32,
    /// Share of smooth areas broken into steps.
    pub banding: f32,
    /// 1 when the detail's centre sits on a thirds line or the centre.
    pub composition: f32,
    /// Distinct colours (to 15 bits): flat art has few, photos many.
    pub colours: usize,
    /// Share of the picture that is transparent.
    pub transparent: f32,
}

fn lum(p: &[u8]) -> f32 {
    0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32
}

pub fn analyse(width: usize, height: usize, rgba: &[u8]) -> PictureQuality {
    let mut q = PictureQuality { width, height, ..Default::default() };
    let n = width * height;
    if n == 0 || rgba.len() < n * 4 {
        return q;
    }
    let l: Vec<f32> = rgba.chunks(4).take(n).map(lum).collect();
    q.transparent = rgba.chunks(4).take(n).filter(|p| p[3] < 8).count() as f32 / n as f32;
    let mean = l.iter().sum::<f32>() / n as f32;
    q.exposure = mean / 255.0;
    q.clipped_shadows = l.iter().filter(|&&v| v < 3.0).count() as f32 / n as f32;
    q.clipped_highlights = l.iter().filter(|&&v| v > 252.0).count() as f32 / n as f32;
    q.contrast = (l.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / n as f32).sqrt() / 255.0;
    // Colourfulness.
    let (mut srg, mut syb, mut mrg, mut myb) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
    for p in rgba.chunks(4).take(n) {
        let rg = p[0] as f64 - p[1] as f64;
        let yb = 0.5 * (p[0] as f64 + p[1] as f64) - p[2] as f64;
        mrg += rg;
        myb += yb;
        srg += rg * rg;
        syb += yb * yb;
    }
    let nn = n as f64;
    let (mrg, myb) = (mrg / nn, myb / nn);
    let sd = ((srg / nn - mrg * mrg).max(0.0) + (syb / nn - myb * myb).max(0.0)).sqrt();
    q.colourfulness = (sd + 0.3 * (mrg * mrg + myb * myb).sqrt()) as f32;
    let mut colours = std::collections::HashSet::new();
    for p in rgba.chunks(4).take(n) {
        colours.insert(((p[0] as u16 >> 3) << 10) | ((p[1] as u16 >> 3) << 5) | (p[2] as u16 >> 3));
    }
    q.colours = colours.len();
    if width < 8 || height < 8 {
        return q;
    }
    let at = |x: usize, y: usize| l[y * width + x];
    // Laplacian variance, and Immerkær's noise from the [1 -2 1]² mask.
    let (mut lap_sum, mut lap_sq, mut cnt) = (0.0f64, 0.0f64, 0usize);
    let mut noise_sum = 0.0f64;
    for y in 1..height - 1 {
        for x in 1..width - 1 {
            let c = at(x, y);
            let lap = at(x - 1, y) + at(x + 1, y) + at(x, y - 1) + at(x, y + 1) - 4.0 * c;
            lap_sum += lap as f64;
            lap_sq += (lap * lap) as f64;
            cnt += 1;
            let m = at(x - 1, y - 1) - 2.0 * at(x, y - 1) + at(x + 1, y - 1) - 2.0 * at(x - 1, y) + 4.0 * c - 2.0 * at(x + 1, y) + at(x - 1, y + 1) - 2.0 * at(x, y + 1) + at(x + 1, y + 1);
            noise_sum += m.abs() as f64;
        }
    }
    let lm = lap_sum / cnt as f64;
    q.sharpness = ((lap_sq / cnt as f64 - lm * lm).max(1e-6)).log10() as f32;
    q.noise = ((std::f64::consts::PI / 2.0).sqrt() * noise_sum / (6.0 * cnt as f64)) as f32;
    // Blockiness: horizontal differences across columns 8k−1|8k vs elsewhere.
    let (mut on, mut on_n, mut off, mut off_n) = (0.0f64, 0usize, 0.0f64, 0usize);
    for y in 0..height {
        for x in 1..width {
            let d = (at(x, y) - at(x - 1, y)).abs() as f64;
            if x % 8 == 0 {
                on += d;
                on_n += 1;
            } else {
                off += d;
                off_n += 1;
            }
        }
    }
    q.blockiness = ((on / on_n.max(1) as f64) / (off / off_n.max(1) as f64).max(1e-6)) as f32;
    // Banding: in smooth areas, flat runs ending in a small step.
    let (mut smooth, mut steps) = (0usize, 0usize);
    for y in 2..height - 2 {
        for x in 2..width - 2 {
            let mut lo = f32::INFINITY;
            let mut hi = f32::NEG_INFINITY;
            for dy in 0..5 {
                for dx in 0..5 {
                    let v = at(x + dx - 2, y + dy - 2);
                    lo = lo.min(v);
                    hi = hi.max(v);
                }
            }
            if hi - lo > 12.0 {
                continue;
            }
            smooth += 1;
            let d = (at(x + 1, y) - at(x, y)).abs();
            let before = (at(x, y) - at(x - 1, y)).abs();
            let after = (at(x + 2, y) - at(x + 1, y)).abs();
            if (0.9..=12.0).contains(&d) && before < 0.35 && after < 0.35 {
                steps += 1;
            }
        }
    }
    q.banding = if smooth > n / 50 { steps as f32 / smooth as f32 * 8.0 } else { 0.0 }.min(1.0);
    // Composition: the centre of detail (gradient energy).
    let (mut ex, mut ey, mut et) = (0.0f64, 0.0f64, 0.0f64);
    for y in 1..height - 1 {
        for x in 1..width - 1 {
            let g = ((at(x + 1, y) - at(x - 1, y)).abs() + (at(x, y + 1) - at(x, y - 1)).abs()) as f64;
            ex += g * x as f64;
            ey += g * y as f64;
            et += g;
        }
    }
    if et > 0.0 {
        let (cx, cy) = ((ex / et) / width as f64, (ey / et) / height as f64);
        let near = |v: f64| [1.0 / 3.0, 0.5, 2.0 / 3.0].iter().map(|t| (v - t).abs()).fold(1.0, f64::min);
        q.composition = (1.0 - (near(cx) + near(cy)) * 4.0).clamp(0.0, 1.0) as f32;
    }
    q
}

/// How close `read` (what the picture says) is to `expected`: 1 minus the
/// edit distance over the longer length, ignoring case and spacing runs.
pub fn text_accuracy(expected: &str, read: &str) -> f32 {
    let norm = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase().chars().collect::<Vec<char>>();
    let (a, b) = (norm(expected), norm(read));
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut cur = vec![i; b.len() + 1];
        for j in 1..=b.len() {
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + usize::from(a[i - 1] != b[j - 1]));
        }
        prev = cur;
    }
    1.0 - prev[b.len()] as f32 / a.len().max(b.len()) as f32
}

/// Faults worth telling the person, in words, worst first.
pub fn faults(q: &PictureQuality) -> Vec<String> {
    let mut out = Vec::new();
    if q.clipped_highlights > 0.08 {
        out.push(format!("{:.0}% of the picture is blown out white", q.clipped_highlights * 100.0));
    }
    if q.clipped_shadows > 0.15 {
        out.push(format!("{:.0}% of the picture is crushed black", q.clipped_shadows * 100.0));
    }
    if q.blockiness > 1.35 {
        out.push(format!("compression blocks show ({:.2}× stronger edges on the 8-pixel grid)", q.blockiness));
    }
    if q.banding > 0.12 {
        out.push(format!("smooth gradients break into bands ({:.0}% of smooth areas)", q.banding * 100.0));
    }
    if q.noise > 6.0 && q.colours > 2000 {
        out.push(format!("noisy ({:.1} levels of noise)", q.noise));
    }
    if q.sharpness < 1.2 && q.colours > 2000 {
        out.push("soft: little fine detail anywhere".into());
    }
    if q.contrast < 0.06 && q.transparent < 0.5 {
        out.push("flat: very low contrast".into());
    }
    if q.width.min(q.height) < 768 && q.width > 0 {
        out.push(format!("small for print or a 4K frame ({}×{})", q.width, q.height));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(w: usize, h: usize, levels: f32) -> Vec<u8> {
        let mut v = Vec::with_capacity(w * h * 4);
        for _y in 0..h {
            for x in 0..w {
                let t = x as f32 / (w - 1) as f32;
                let g = ((t * levels).floor() / levels * 200.0 + 20.0) as u8;
                v.extend_from_slice(&[g, g, g, 255]);
            }
        }
        v
    }

    #[test]
    fn banding_is_seen_in_a_stepped_gradient_and_not_a_smooth_one() {
        let banded = analyse(512, 64, &gradient(512, 64, 24.0));
        let mut smooth_px = Vec::new();
        for y in 0..64 {
            for x in 0..512 {
                // Dithered: noise of a level breaks the plateaus.
                let t = x as f32 / 511.0 * 200.0 + 20.0 + (((x * 7 + y * 13) % 5) as f32 - 2.0) * 0.6;
                let g = t.round() as u8;
                smooth_px.extend_from_slice(&[g, g, g, 255]);
            }
        }
        let smooth = analyse(512, 64, &smooth_px);
        assert!(banded.banding > smooth.banding * 3.0 && banded.banding > 0.1, "{} vs {}", banded.banding, smooth.banding);
    }

    #[test]
    fn text_accuracy_counts_edits() {
        assert_eq!(text_accuracy("Grand Opening", "grand  opening"), 1.0);
        assert!(text_accuracy("Grand Opening", "Grnad Openign") < 0.9);
    }
}
