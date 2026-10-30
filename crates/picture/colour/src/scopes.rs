// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Scope computations: luma waveform, RGB parade, vectorscope and
//! histogram.
//!
//! Scopes describe the *signal*, so they first encode the image with the
//! output transfer (BT.1886 by default, i.e. what a Rec. 709 export would
//! contain). The image is composited over black — premultiplied colour
//! already is that — and downscaled with a box filter so that scope cost
//! does not depend on the frame size.
//!
//! The same computations run on the GPU in `shaders/scopes.wgsl`; the CPU
//! versions are the reference and are used for tests and exports.

use crate::image::Image;
use crate::lut::SKIN_LINE_DEG;
use crate::transfer::Transfer;
use crate::ycbcr::YCbCrMatrix;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScopeSettings {
    /// Width the frame is reduced to before analysis (height keeps aspect).
    pub max_width: u32,
    /// Transfer used to turn linear light into signal values.
    pub transfer: Transfer,
    /// Matrix for luma and Cb/Cr.
    pub matrix: YCbCrMatrix,
    /// Vertical resolution of waveforms and number of histogram bins.
    pub levels: u32,
    /// Vectorscope side length in cells.
    pub vectorscope_size: u32,
}

impl Default for ScopeSettings {
    fn default() -> ScopeSettings {
        ScopeSettings {
            max_width: 512,
            transfer: Transfer::Bt1886,
            matrix: YCbCrMatrix::Bt709,
            levels: 256,
            vectorscope_size: 256,
        }
    }
}

/// Box-filter downscale to at most `max_width` wide.
pub fn downscale(img: &Image, max_width: u32) -> Image {
    if img.width <= max_width || img.width == 0 || img.height == 0 {
        return img.clone();
    }
    let w = max_width.max(1);
    let h = ((img.height as u64 * w as u64) / img.width as u64).max(1) as u32;
    Image::from_fn(w, h, |x, y| {
        let x0 = x as u64 * img.width as u64 / w as u64;
        let x1 = ((x as u64 + 1) * img.width as u64 / w as u64).max(x0 + 1);
        let y0 = y as u64 * img.height as u64 / h as u64;
        let y1 = ((y as u64 + 1) * img.height as u64 / h as u64).max(y0 + 1);
        let mut s = [0.0f32; 4];
        for yy in y0..y1 {
            for xx in x0..x1 {
                let p = img.get(xx as u32, yy as u32);
                for i in 0..4 {
                    s[i] += p[i];
                }
            }
        }
        let n = ((x1 - x0) * (y1 - y0)) as f32;
        s.map(|v| v / n)
    })
}

/// Column-by-level counts. `data[level * width + column]`, level 0 = signal
/// 0 (draw it at the bottom).
#[derive(Clone, Debug, PartialEq)]
pub struct Waveform {
    pub width: u32,
    pub levels: u32,
    pub data: Vec<u32>,
}

impl Waveform {
    fn new(width: u32, levels: u32) -> Waveform {
        Waveform { width, levels, data: vec![0; (width * levels) as usize] }
    }

    fn add(&mut self, x: u32, v: f32) {
        let l = level(v, self.levels);
        self.data[(l * self.width + x) as usize] += 1;
    }

    pub fn get(&self, x: u32, level: u32) -> u32 {
        self.data[(level * self.width + x) as usize]
    }

    /// Samples that went into each column (the image height it was read
    /// from): what a trace's brightness is measured against.
    pub fn per_column(&self) -> u32 {
        (0..self.width.min(1)).map(|x| (0..self.levels).map(|l| self.get(x, l)).sum()).next().unwrap_or(0)
    }

    /// The signal level (0..1) of the brightest and the darkest sample, or
    /// `None` for an empty scope.
    pub fn range(&self) -> Option<(f32, f32)> {
        let mut lo = None;
        let mut hi = None;
        for l in 0..self.levels {
            if (0..self.width).any(|x| self.get(x, l) > 0) {
                lo.get_or_insert(l);
                hi = Some(l);
            }
        }
        let f = |l: u32| l as f32 / (self.levels - 1).max(1) as f32;
        Some((f(lo?), f(hi?)))
    }
}

/// How bright a scope cell with `count` samples shows (0..1), when a
/// column holds `per_column` samples. A single stray sample is dim but
/// visible; a tenth of the column at one level is full. The square root
/// keeps the thin parts of a trace readable next to flat areas.
pub fn trace_intensity(count: u32, per_column: u32) -> f32 {
    if count == 0 {
        return 0.0;
    }
    let full = (per_column as f32 * 0.1).max(4.0);
    (count as f32 / full).sqrt().clamp(0.22, 1.0)
}

/// Signal value (0..1) as a 10-bit code value (0..1023).
pub fn code_10bit(v: f32) -> u32 {
    (v.clamp(0.0, 1.0) * 1023.0).round() as u32
}

/// The broadcast ("legal") range of the signal: 10-bit codes 64..940.
pub const LEGAL_RANGE: (f32, f32) = (64.0 / 1023.0, 940.0 / 1023.0);

/// Signal value → bin, clamped to the scope range 0..1.
#[inline]
pub fn level(v: f32, levels: u32) -> u32 {
    ((v.clamp(0.0, 1.0) * (levels - 1) as f32).round() as u32).min(levels - 1)
}

fn encoded(img: &Image, s: &ScopeSettings) -> Image {
    let mut d = downscale(img, s.max_width);
    for p in d.pixels.iter_mut() {
        let e = s.transfer.encode_rgb([p[0], p[1], p[2]]);
        *p = [e[0], e[1], e[2], p[3]];
    }
    d
}

pub fn waveform_luma(img: &Image, s: &ScopeSettings) -> Waveform {
    let e = encoded(img, s);
    let lw = s.matrix.luma_weights();
    let mut w = Waveform::new(e.width, s.levels);
    for y in 0..e.height {
        for x in 0..e.width {
            let p = e.get(x, y);
            w.add(x, lw[0] * p[0] + lw[1] * p[1] + lw[2] * p[2]);
        }
    }
    w
}

/// R, G and B waveforms (the parade).
pub fn parade(img: &Image, s: &ScopeSettings) -> [Waveform; 3] {
    let e = encoded(img, s);
    let mut out = [Waveform::new(e.width, s.levels), Waveform::new(e.width, s.levels), Waveform::new(e.width, s.levels)];
    for y in 0..e.height {
        for x in 0..e.width {
            let p = e.get(x, y);
            for (c, w) in out.iter_mut().enumerate() {
                w.add(x, p[c]);
            }
        }
    }
    out
}

/// Cb/Cr density. `data[row * size + col]`; column 0 is Cb = −0.5, row 0
/// is Cr = +0.5 (so the image is drawn the usual way up).
#[derive(Clone, Debug, PartialEq)]
pub struct Vectorscope {
    pub size: u32,
    pub data: Vec<u32>,
}

/// Cb/Cr → (column, row) as fractions 0..1 of the vectorscope square.
#[inline]
pub fn vectorscope_position(cb: f32, cr: f32) -> (f32, f32) {
    (cb + 0.5, 0.5 - cr)
}

pub fn vectorscope(img: &Image, s: &ScopeSettings) -> Vectorscope {
    let e = encoded(img, s);
    let n = s.vectorscope_size;
    let mut v = Vectorscope { size: n, data: vec![0; (n * n) as usize] };
    for p in &e.pixels {
        let ycc = s.matrix.to_ycbcr([p[0], p[1], p[2]]);
        let (u, w) = vectorscope_position(ycc[1], ycc[2]);
        let col = level(u, n);
        let row = level(w, n);
        v.data[(row * n + col) as usize] += 1;
    }
    v
}

/// A graticule target for the vectorscope.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Target {
    pub name: &'static str,
    pub cb: f32,
    pub cr: f32,
}

/// Positions of 75 % colour bars (R, Mg, B, Cy, G, Yl at 0.75 signal).
pub fn vectorscope_targets(matrix: YCbCrMatrix) -> [Target; 6] {
    let bars: [(&str, [f32; 3]); 6] = [
        ("R", [0.75, 0.0, 0.0]),
        ("Mg", [0.75, 0.0, 0.75]),
        ("B", [0.0, 0.0, 0.75]),
        ("Cy", [0.0, 0.75, 0.75]),
        ("G", [0.0, 0.75, 0.0]),
        ("Yl", [0.75, 0.75, 0.0]),
    ];
    bars.map(|(name, rgb)| {
        let ycc = matrix.to_ycbcr(rgb);
        Target { name, cb: ycc[1], cr: ycc[2] }
    })
}

/// End point (Cb, Cr) of the skin-tone line at the given length.
pub fn skin_line(length: f32) -> (f32, f32) {
    let a = SKIN_LINE_DEG.to_radians();
    (length * a.cos(), length * a.sin())
}

/// Per-channel histograms of the signal.
#[derive(Clone, Debug, PartialEq)]
pub struct Histogram {
    pub bins: u32,
    pub red: Vec<u32>,
    pub green: Vec<u32>,
    pub blue: Vec<u32>,
    pub luma: Vec<u32>,
}

pub fn histogram(img: &Image, s: &ScopeSettings) -> Histogram {
    let e = encoded(img, s);
    let b = s.levels;
    let lw = s.matrix.luma_weights();
    let mut h = Histogram {
        bins: b,
        red: vec![0; b as usize],
        green: vec![0; b as usize],
        blue: vec![0; b as usize],
        luma: vec![0; b as usize],
    };
    for p in &e.pixels {
        h.red[level(p[0], b) as usize] += 1;
        h.green[level(p[1], b) as usize] += 1;
        h.blue[level(p[2], b) as usize] += 1;
        h.luma[level(lw[0] * p[0] + lw[1] * p[1] + lw[2] * p[2], b) as usize] += 1;
    }
    h
}

impl Histogram {
    /// The shares of samples stacked in the first and the last bin of
    /// the busiest channel: black crushed against the left edge, and a
    /// channel clipped against the right.
    pub fn edge_shares(&self) -> (f32, f32) {
        let total = self.red.iter().sum::<u32>().max(1) as f32;
        let ends = |v: &Vec<u32>| (v.first().copied().unwrap_or(0) as f32 / total, v.last().copied().unwrap_or(0) as f32 / total);
        let chans = [ends(&self.red), ends(&self.green), ends(&self.blue)];
        (chans.iter().map(|c| c.0).fold(0.0, f32::max), chans.iter().map(|c| c.1).fold(0.0, f32::max))
    }
}

/// Like [`vectorscope_targets`] with the bars at `level` of full signal
/// (the vectorscope draws 75 % and 100 % boxes).
pub fn vectorscope_targets_at(matrix: YCbCrMatrix, level: f32) -> [Target; 6] {
    let mut t = vectorscope_targets(matrix);
    let k = level / 0.75;
    for x in &mut t {
        x.cb *= k;
        x.cr *= k;
    }
    t
}

/// What a colourist reads off the scopes, as numbers: where the levels
/// sit, what clips, the cast in shadows, mids and highlights, and how
/// vivid and how skin-like the colours are. Levels are 0..1 of the
/// encoded signal (the waveform's scale), shares are fractions of the
/// picture.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Readings {
    /// Luma at the 1st, 5th, 50th, 95th and 99th percentiles.
    pub luma: [f32; 5],
    /// Mean luma.
    pub luma_mean: f32,
    /// Share with a channel at the top (blown out).
    pub clipped: f32,
    /// Share at the bottom (crushed to black).
    pub crushed: f32,
    /// Share outside broadcast range (luma under 16 or over 235 of 255).
    pub outside_legal: f32,
    /// Mean (R, G, B) minus the zone's luma, for shadows (luma < 0.25),
    /// mids and highlights (> 0.75): a colour cast shows as one channel
    /// above the others. `None` when the zone is nearly empty.
    pub cast: [Option<[f32; 3]>; 3],
    /// Chroma as a share of the most a pixel can have: mean and 95th
    /// percentile.
    pub saturation: [f32; 2],
    /// Share of strongly coloured pixels (over 80 % chroma): likely
    /// oversaturated.
    pub oversaturated: f32,
    /// The mean chroma's hue on the vectorscope (degrees, 0 = +Cb, 90 =
    /// +Cr) and how strong it is (0..1 of the most): the picture's
    /// overall tint.
    pub tint: (f32, f32),
    /// Of the coloured pixels, the share within 10° of the skin line.
    pub near_skin_line: f32,
}

/// [`Readings`] of an image (linear values, encoded with `s.transfer`).
pub fn readings(img: &Image, s: &ScopeSettings) -> Readings {
    let e = encoded(img, s);
    let lw = s.matrix.luma_weights();
    let n = e.pixels.len().max(1) as f32;
    let mut lumas: Vec<f32> = Vec::with_capacity(e.pixels.len());
    let mut sats: Vec<f32> = Vec::with_capacity(e.pixels.len());
    let (mut clipped, mut crushed, mut legal, mut over, mut skin, mut coloured) = (0.0f32, 0.0f32, 0.0f32, 0.0f32, 0.0f32, 0.0f32);
    let mut zones = [[0.0f64; 4]; 3];
    let (mut mcb, mut mcr) = (0.0f64, 0.0f64);
    // The most chroma any colour reaches: a primary or secondary at full.
    let most = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [1.0, 1.0, 0.0], [0.0, 1.0, 1.0], [1.0, 0.0, 1.0]]
        .iter()
        .map(|c| {
            let y = s.matrix.to_ycbcr(*c);
            y[1].hypot(y[2])
        })
        .fold(0.0f32, f32::max)
        .max(1e-6);
    for p in &e.pixels {
        let (r, g, b) = (p[0].clamp(0.0, 1.0), p[1].clamp(0.0, 1.0), p[2].clamp(0.0, 1.0));
        let y = lw[0] * r + lw[1] * g + lw[2] * b;
        lumas.push(y);
        if r.max(g).max(b) >= 0.995 {
            clipped += 1.0;
        }
        if y <= 0.005 {
            crushed += 1.0;
        }
        if !(16.0 / 255.0..=235.0 / 255.0).contains(&y) {
            legal += 1.0;
        }
        let z = if y < 0.25 {
            0
        } else if y > 0.75 {
            2
        } else {
            1
        };
        for (k, c) in [r, g, b].into_iter().enumerate() {
            zones[z][k] += (c - y) as f64;
        }
        zones[z][3] += 1.0;
        let ycc = s.matrix.to_ycbcr([r, g, b]);
        let sat = ycc[1].hypot(ycc[2]) / most;
        sats.push(sat);
        mcb += ycc[1] as f64;
        mcr += ycc[2] as f64;
        if sat > 0.8 {
            over += 1.0;
        }
        if sat > 0.05 {
            coloured += 1.0;
            let mut d = (ycc[2].atan2(ycc[1]).to_degrees() - SKIN_LINE_DEG).abs() % 360.0;
            if d > 180.0 {
                d = 360.0 - d;
            }
            if d <= 10.0 {
                skin += 1.0;
            }
        }
    }
    let pct = |v: &mut Vec<f32>, q: f32| -> f32 {
        if v.is_empty() {
            return 0.0;
        }
        let k = ((v.len() - 1) as f32 * q).round() as usize;
        *v.select_nth_unstable_by(k, |a, b| a.total_cmp(b)).1
    };
    let luma = [0.01, 0.05, 0.5, 0.95, 0.99].map(|q| pct(&mut lumas, q));
    let luma_mean = lumas.iter().sum::<f32>() / n;
    let saturation = [sats.iter().sum::<f32>() / n, pct(&mut sats, 0.95)];
    let cast = zones.map(|z| (z[3] >= (n as f64 * 0.02).max(1.0)).then(|| [(z[0] / z[3]) as f32, (z[1] / z[3]) as f32, (z[2] / z[3]) as f32]));
    let (cb, cr) = ((mcb / n as f64) as f32, (mcr / n as f64) as f32);
    let hue = cr.atan2(cb).to_degrees().rem_euclid(360.0);
    Readings {
        luma,
        luma_mean,
        clipped: clipped / n,
        crushed: crushed / n,
        outside_legal: legal / n,
        cast,
        saturation,
        oversaturated: over / n,
        tint: (hue, cb.hypot(cr) / most),
        near_skin_line: if coloured > 0.0 { skin / coloured } else { 0.0 },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bars() -> Image {
        // 75 % bars, already encoded values decoded back to linear.
        let cols = [[0.75, 0.75, 0.75], [0.75, 0.75, 0.0], [0.0, 0.75, 0.75], [0.0, 0.75, 0.0]];
        Image::from_fn(400, 100, |x, _| {
            let c = Transfer::Bt1886.decode_rgb(cols[(x / 100) as usize]);
            [c[0], c[1], c[2], 1.0]
        })
    }

    #[test]
    fn downscale_keeps_mean() {
        let img = bars();
        let d = downscale(&img, 100);
        assert_eq!((d.width, d.height), (100, 25));
        let m: f32 = d.pixels.iter().map(|p| p[1]).sum::<f32>() / d.len() as f32;
        let m0: f32 = img.pixels.iter().map(|p| p[1]).sum::<f32>() / img.len() as f32;
        assert!((m - m0).abs() < 1e-5);
    }

    #[test]
    fn waveform_and_parade_levels() {
        let s = ScopeSettings { max_width: 400, ..ScopeSettings::default() };
        let img = bars();
        let w = waveform_luma(&img, &s);
        let l75 = level(0.75, 256);
        assert_eq!(w.get(50, l75), 100, "grey bar sits at 75 %");
        let p = parade(&img, &s);
        assert_eq!(p[2].get(150, 0), 100, "yellow has no blue");
        assert_eq!(p[0].get(150, l75), 100);
        let total: u32 = w.data.iter().sum();
        assert_eq!(total, 400 * 100);
    }

    fn flat(v: f32) -> Image {
        let c = Transfer::Bt1886.decode_rgb([v; 3]);
        Image::from_fn(64, 36, move |_, _| [c[0], c[1], c[2], 1.0])
    }

    #[test]
    fn known_frames_sit_where_a_scope_puts_them() {
        let s = ScopeSettings { max_width: 64, ..ScopeSettings::default() };
        // Black is the bottom row, white the top row, whatever the width.
        let (lo, hi) = waveform_luma(&flat(0.0), &s).range().unwrap();
        assert_eq!((lo, hi), (0.0, 0.0));
        let (lo, hi) = waveform_luma(&flat(1.0), &s).range().unwrap();
        assert_eq!((lo, hi), (1.0, 1.0));
        // Mid grey at 50 %: within one bin of the middle.
        let (m, _) = waveform_luma(&flat(0.5), &s).range().unwrap();
        assert!((m - 0.5).abs() < 1.0 / 255.0, "{m}");
        assert_eq!(waveform_luma(&flat(0.5), &s).per_column(), 36);
        // A ramp left to right climbs from black to white, one level per column.
        let ramp = Image::from_fn(256, 8, |x, _| {
            let c = Transfer::Bt1886.decode_rgb([x as f32 / 255.0; 3]);
            [c[0], c[1], c[2], 1.0]
        });
        let w = waveform_luma(&ramp, &ScopeSettings { max_width: 256, ..ScopeSettings::default() });
        for x in [0u32, 64, 128, 255] {
            assert_eq!(w.get(x, x), 8, "column {x} holds its own level");
        }
    }

    #[test]
    fn colour_bars_land_on_the_targets_and_grey_on_the_centre() {
        let s = ScopeSettings::default();
        let v = vectorscope(&bars(), &s);
        let n = s.vectorscope_size;
        // All the 75 % bars (and no more) sit on a 75 % target or the centre.
        let on_target = |col: u32, row: u32| {
            vectorscope_targets(YCbCrMatrix::Bt709).iter().any(|t| {
                let (u, w) = vectorscope_position(t.cb, t.cr);
                level(u, n).abs_diff(col) <= 1 && level(w, n).abs_diff(row) <= 1
            }) || (col.abs_diff(n / 2) <= 1 && row.abs_diff(n / 2) <= 1)
        };
        let mut off = 0;
        for row in 0..n {
            for col in 0..n {
                if v.data[(row * n + col) as usize] > 0 && !on_target(col, row) {
                    off += 1;
                }
            }
        }
        assert_eq!(off, 0);
        // The 100 % targets lie further out along the same angle.
        let (a, b) = (vectorscope_targets_at(YCbCrMatrix::Bt709, 0.75), vectorscope_targets_at(YCbCrMatrix::Bt709, 1.0));
        for (x, y) in a.iter().zip(&b) {
            assert!(((y.cb.hypot(y.cr)) / x.cb.hypot(x.cr) - 4.0 / 3.0).abs() < 1e-4);
        }
    }

    #[test]
    fn traces_show_one_sample_and_saturate_on_a_flat_area() {
        assert_eq!(trace_intensity(0, 100), 0.0);
        let one = trace_intensity(1, 144);
        assert!(one >= 0.22 && one < 0.5, "{one}");
        assert_eq!(trace_intensity(144, 144), 1.0);
        assert!(trace_intensity(5, 144) > one);
        assert_eq!((code_10bit(0.0), code_10bit(1.0)), (0, 1023));
        assert_eq!((LEGAL_RANGE.0 * 1023.0).round(), 64.0);
    }

    #[test]
    fn histogram_edges_tell_clipping_from_a_normal_frame() {
        let s = ScopeSettings::default();
        let (low, high) = histogram(&flat(1.0), &s).edge_shares();
        assert_eq!((low, high), (0.0, 1.0));
        let (low, high) = histogram(&flat(0.0), &s).edge_shares();
        assert_eq!((low, high), (1.0, 0.0));
        let (low, high) = histogram(&flat(0.5), &s).edge_shares();
        assert_eq!((low, high), (0.0, 0.0));
    }

    #[test]
    fn vectorscope_hits_targets() {
        let s = ScopeSettings::default();
        let img = bars();
        let v = vectorscope(&img, &s);
        let t = vectorscope_targets(YCbCrMatrix::Bt709);
        let yl = t.iter().find(|t| t.name == "Yl").unwrap();
        let (u, w) = vectorscope_position(yl.cb, yl.cr);
        let idx = (level(w, 256) * 256 + level(u, 256)) as usize;
        assert!(v.data[idx] > 0);
        let centre = (level(0.5, 256) * 256 + level(0.5, 256)) as usize;
        assert!(v.data[centre] > 0, "grey bar lands in the centre");
        let (cb, cr) = skin_line(0.3);
        assert!(cb < 0.0 && cr > 0.0);
    }

    #[test]
    fn histogram_counts() {
        let h = histogram(&bars(), &ScopeSettings::default());
        assert_eq!(h.red.iter().sum::<u32>(), 400 * 100);
        assert_eq!(h.blue[0], 200 * 100, "yellow and green bars");
    }

    #[test]
    fn readings_measure_levels_casts_and_skin() {
        let s = ScopeSettings::default();
        let r = readings(&bars(), &s);
        // Three of four bars are bright; none blown out or crushed.
        assert!(r.clipped == 0.0 && r.crushed == 0.0, "{r:?}");
        assert!((r.luma[2] - 0.6).abs() < 0.2, "{r:?}");
        // A warm grey: red above blue in the mids.
        let warm = Image::from_fn(64, 64, |_, _| {
            let c = Transfer::Bt1886.decode_rgb([0.8, 0.6, 0.5]);
            [c[0], c[1], c[2], 1.0]
        });
        let w = readings(&warm, &s);
        let mids = w.cast[1].unwrap();
        assert!(mids[0] > 0.0 && mids[2] < 0.0, "{mids:?}");
        assert!(w.cast[0].is_none() && w.cast[2].is_none());
        assert!(w.near_skin_line > 0.99, "a skin tone lies on the skin line: {w:?}");
        // Pure white is blown out and outside broadcast range.
        let white = Image::from_fn(8, 8, |_, _| [1.0, 1.0, 1.0, 1.0]);
        let x = readings(&white, &s);
        assert_eq!((x.clipped, x.outside_legal), (1.0, 1.0));
        let red = Image::from_fn(8, 8, |_, _| [1.0, 0.0, 0.0, 1.0]);
        assert!(readings(&red, &s).oversaturated == 1.0);
    }
}
