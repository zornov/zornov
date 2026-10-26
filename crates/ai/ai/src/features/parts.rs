// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Clean parts from a picture of a person or an object.
//!
//! A first split (each pixel to the bone it is nearest to through the body;
//! SAM 2.1 masks and pose landmarks sharpen that guess upstream) gives a
//! label per pixel. This module turns the labels into layers that can move:
//!
//! * [`separate`] tidies the labels (holes in the body given to the part
//!   they sit in, stray speckle absorbed, loose islands joined to their
//!   neighbour), then cuts the picture so **every visible pixel is in
//!   exactly one part** (no duplicated pixels), and fills what a part hides
//!   behind it: where an arm is in front of the torso, the torso layer gets
//!   the colours the arm covers, painted in from the torso around it with
//!   [`super::fill::inpaint`], so lifting the arm shows body and not a hole.
//!   The layer in front gets a one-pixel soft seam over the fill.
//! * [`recomposite`] stacks the parts back to front; at rest it equals the
//!   picture.
//! * [`diagnose`] draws the original, the parts and where the stack differs
//!   from the picture, side by side, and counts holes, extras and stacked
//!   pixels, so a person (or an AI) can see what to fix.

use super::fill::inpaint;
use crate::frame::Frame;

/// Alpha at or above which a pixel counts as part of the subject.
const SEEN: u8 = 8;
/// Alpha at or above which a pixel is solid (no soft edge).
const SOLID: u8 = 250;

/// One part before cleaning.
#[derive(Clone, Debug, PartialEq)]
pub struct PartSpec {
    pub name: String,
    /// Index of the part it hangs from (a forearm on its upper arm).
    pub parent: Option<usize>,
    /// Where it joins its parent, pixels.
    pub joint: [f32; 2],
    /// Drawing order: larger is in front.
    pub depth: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Options {
    /// Repair the labels first (holes, speckle, islands).
    pub tidy: bool,
    /// Fill what each part hides behind it.
    pub fill_behind: bool,
    /// Soften the front layer's one-pixel seam over the fill.
    pub seam_feather: bool,
    /// How wide the strip filled along a contact is, as a share of the
    /// limb's half thickness.
    pub reach: f32,
}

impl Default for Options {
    fn default() -> Options {
        Options { tidy: true, fill_behind: true, seam_feather: true, reach: 0.8 }
    }
}

/// A finished part: a picture the size of the whole.
#[derive(Clone, Debug, PartialEq)]
pub struct CleanPart {
    pub name: String,
    /// RGBA8, the rest transparent.
    pub rgba: Vec<u8>,
    /// Pixels painted in behind another part (not in the picture).
    pub behind: Vec<bool>,
    /// Visible pixels (from the picture).
    pub pixels: usize,
    /// Pixels filled in behind other parts.
    pub filled: usize,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Report {
    pub holes_assigned: usize,
    pub flipped: usize,
    pub islands_merged: usize,
    pub filled: usize,
    pub notes: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Cleaned {
    pub parts: Vec<CleanPart>,
    /// The tidied labels (0 none, part + 1).
    pub labels: Vec<u16>,
    pub report: Report,
}

fn neighbours(i: usize, w: usize, h: usize) -> impl Iterator<Item = usize> {
    let (x, y) = (i % w, i / w);
    [(x > 0).then(|| i - 1), (x + 1 < w).then(|| i + 1), (y > 0).then(|| i - w), (y + 1 < h).then(|| i + w)].into_iter().flatten()
}

fn neighbours8(i: usize, w: usize, h: usize) -> impl Iterator<Item = usize> {
    let (x, y) = ((i % w) as isize, (i / w) as isize);
    (-1isize..=1).flat_map(move |dy| (-1isize..=1).map(move |dx| (dx, dy))).filter(|&(dx, dy)| dx != 0 || dy != 0).filter_map(move |(dx, dy)| {
        let (nx, ny) = (x + dx, y + dy);
        (nx >= 0 && ny >= 0 && nx < w as isize && ny < h as isize).then(|| ny as usize * w + nx as usize)
    })
}

/// Labels spread into unlabelled opaque pixels from their labelled
/// neighbours, breadth first (nearest wins). With `through_all`, the
/// spread crosses transparent pixels too (for a loose opaque island).
fn spread(labels: &mut [u16], alpha: &[u8], w: usize, h: usize, through_all: bool) -> usize {
    let mut carry: Vec<u16> = labels.to_vec();
    let mut queue: std::collections::VecDeque<usize> = (0..w * h).filter(|&i| carry[i] != 0).collect();
    while let Some(i) = queue.pop_front() {
        for n in neighbours(i, w, h) {
            if carry[n] == 0 && (through_all || alpha[n] >= SEEN) {
                carry[n] = carry[i];
                queue.push_back(n);
            }
        }
    }
    let mut n = 0;
    for i in 0..w * h {
        if alpha[i] >= SEEN && labels[i] == 0 && carry[i] != 0 {
            labels[i] = carry[i];
            n += 1;
        }
    }
    n
}

/// Repairs the labels of `alpha`-opaque pixels. Returns the counts.
pub fn tidy_labels(labels: &mut [u16], alpha: &[u8], w: usize, h: usize, rep: &mut Report) {
    for i in 0..w * h {
        if alpha[i] < SEEN {
            labels[i] = 0;
        }
    }
    // Holes: opaque pixels nobody owns go to the nearest part through the body.
    rep.holes_assigned += spread(labels, alpha, w, h, false);
    rep.holes_assigned += spread(labels, alpha, w, h, true);
    // Speckle: a pixel whose neighbours (six of eight) agree on another part.
    for _ in 0..2 {
        let src = labels.to_vec();
        for i in 0..w * h {
            if alpha[i] < SEEN {
                continue;
            }
            let mut counts: [(u16, u8); 8] = [(0, 0); 8];
            let mut used = 0;
            for n in neighbours8(i, w, h) {
                let l = src[n];
                if l == 0 {
                    continue;
                }
                match counts[..used].iter_mut().find(|c| c.0 == l) {
                    Some(c) => c.1 += 1,
                    None => {
                        counts[used] = (l, 1);
                        used += 1;
                    }
                }
            }
            if let Some(&(l, c)) = counts[..used].iter().max_by_key(|c| c.1) {
                if c >= 6 && l != src[i] {
                    labels[i] = l;
                    rep.flipped += 1;
                }
            }
        }
    }
    // Islands: small loose pieces of a part join the part they touch most.
    let opaque = (0..w * h).filter(|&i| alpha[i] >= SEEN).count();
    let small = (opaque / 500).max(12);
    let mut comp = vec![0u32; w * h];
    let mut comps: Vec<(u16, Vec<usize>)> = Vec::new();
    for s in 0..w * h {
        if labels[s] == 0 || comp[s] != 0 {
            continue;
        }
        let id = comps.len() as u32 + 1;
        let mut members = vec![s];
        comp[s] = id;
        let mut k = 0;
        while k < members.len() {
            let i = members[k];
            k += 1;
            for n in neighbours(i, w, h) {
                if comp[n] == 0 && labels[n] == labels[s] {
                    comp[n] = id;
                    members.push(n);
                }
            }
        }
        comps.push((labels[s], members));
    }
    let mut biggest: std::collections::HashMap<u16, usize> = std::collections::HashMap::new();
    for (l, m) in &comps {
        let e = biggest.entry(*l).or_default();
        *e = (*e).max(m.len());
    }
    for (l, m) in &comps {
        if m.len() >= small || m.len() == biggest[l] {
            continue;
        }
        let mut votes: std::collections::HashMap<u16, usize> = std::collections::HashMap::new();
        for &i in m {
            for n in neighbours(i, w, h) {
                if labels[n] != 0 && labels[n] != *l {
                    *votes.entry(labels[n]).or_default() += 1;
                }
            }
        }
        if let Some((&to, _)) = votes.iter().max_by_key(|(k, v)| (**v, std::cmp::Reverse(**k))) {
            for &i in m {
                labels[i] = to;
            }
            rep.islands_merged += 1;
        }
    }
}

/// Distance (steps, eight-connected) from `from` pixels, spreading only
/// through `within` pixels, up to `limit`; `u16::MAX` where not reached.
fn reach(from: &[bool], within: &[bool], w: usize, h: usize, limit: u16) -> Vec<u16> {
    let mut d = vec![u16::MAX; w * h];
    let mut queue: std::collections::VecDeque<usize> = std::collections::VecDeque::new();
    for i in 0..w * h {
        if from[i] {
            d[i] = 0;
            queue.push_back(i);
        }
    }
    while let Some(i) = queue.pop_front() {
        if d[i] >= limit {
            continue;
        }
        for n in neighbours8(i, w, h) {
            if d[n] == u16::MAX && within[n] {
                d[n] = d[i] + 1;
                queue.push_back(n);
            }
        }
    }
    d
}

/// The largest distance from the edge inside `mask`: half its thickness.
fn half_thickness(mask: &[bool], w: usize, h: usize) -> f32 {
    let outside: Vec<bool> = (0..w * h).map(|i| !mask[i] || neighbours(i, w, h).count() < 4 || neighbours(i, w, h).any(|n| !mask[n])).collect();
    // Edge pixels are distance 0; spread inward.
    let edge: Vec<bool> = (0..w * h).map(|i| mask[i] && outside[i]).collect();
    let d = reach(&edge, mask, w, h, u16::MAX - 1);
    d.iter().zip(mask).filter(|(d, m)| **m && **d != u16::MAX).map(|(d, _)| *d).max().map_or(1.0, |m| m as f32 + 1.0)
}

/// Paints `zone` pixels of `back` from the colours of its own solid pixels.
fn fill_into(back: &mut CleanPart, zone: &[bool], w: usize, h: usize) -> usize {
    let known: Vec<bool> = (0..w * h).map(|i| back.rgba[i * 4 + 3] >= 200).collect();
    if !known.iter().any(|&k| k) {
        return 0;
    }
    // Work on a box round what matters.
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0usize, 0usize);
    for i in 0..w * h {
        if known[i] || zone[i] {
            let (x, y) = (i % w, i / w);
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x + 1);
            y1 = y1.max(y + 1);
        }
    }
    let (cw, ch) = (x1 - x0, y1 - y0);
    let mut crop = Frame::filled(cw, ch, [0, 0, 0, 255]);
    let mut hole = vec![true; cw * ch];
    for y in 0..ch {
        for x in 0..cw {
            let s = (y + y0) * w + x + x0;
            if known[s] {
                let (a, b) = ((y * cw + x) * 4, s * 4);
                crop.rgba[a..a + 3].copy_from_slice(&back.rgba[b..b + 3]);
                hole[y * cw + x] = false;
            }
        }
    }
    let filled = inpaint(&crop, &hole);
    let mut n = 0;
    for y in 0..ch {
        for x in 0..cw {
            let s = (y + y0) * w + x + x0;
            if zone[s] && back.rgba[s * 4 + 3] < SOLID {
                let a = (y * cw + x) * 4;
                back.rgba[s * 4..s * 4 + 3].copy_from_slice(&filled.rgba[a..a + 3]);
                back.rgba[s * 4 + 3] = 255;
                back.behind[s] = true;
                n += 1;
            }
        }
    }
    n
}

/// Cuts `frame` into one layer per part along `labels` (0 none, part + 1;
/// one entry per pixel), repairing the labels first and filling what each
/// part hides behind it.
pub fn separate(frame: &Frame, labels: &[u16], specs: &[PartSpec], opts: &Options) -> Cleaned {
    let (w, h) = (frame.width, frame.height);
    assert_eq!(labels.len(), w * h, "one label per pixel");
    let mut labels = labels.to_vec();
    let alpha: Vec<u8> = frame.rgba.chunks_exact(4).map(|p| p[3]).collect();
    let mut report = Report::default();
    if opts.tidy {
        tidy_labels(&mut labels, &alpha, w, h, &mut report);
    } else {
        for i in 0..w * h {
            if alpha[i] < SEEN {
                labels[i] = 0;
            }
        }
    }
    // Every visible pixel into exactly one part.
    let mut parts: Vec<CleanPart> = specs.iter().map(|s| CleanPart { name: s.name.clone(), rgba: vec![0; w * h * 4], behind: vec![false; w * h], pixels: 0, filled: 0 }).collect();
    for i in 0..w * h {
        let l = labels[i] as usize;
        if l == 0 || l > parts.len() {
            continue;
        }
        parts[l - 1].rgba[i * 4..i * 4 + 4].copy_from_slice(&frame.rgba[i * 4..i * 4 + 4]);
        parts[l - 1].pixels += 1;
    }
    if opts.fill_behind {
        for (c, spec) in specs.iter().enumerate() {
            let Some(p) = spec.parent.filter(|&p| p < specs.len() && p != c) else { continue };
            // The part in front hides some of the one behind it.
            let (front, back) = if spec.depth >= specs[p].depth { (c, p) } else { (p, c) };
            let (fl, bl) = (front as u16 + 1, back as u16 + 1);
            let front_mask: Vec<bool> = labels.iter().map(|&l| l == fl).collect();
            let back_mask: Vec<bool> = labels.iter().map(|&l| l == bl).collect();
            let child_mask: Vec<bool> = if front == c { front_mask.clone() } else { back_mask.clone() };
            if !front_mask.iter().any(|&m| m) || !back_mask.iter().any(|&m| m) {
                continue;
            }
            let half = half_thickness(&child_mask, w, h);
            let strip = (opts.reach * half).round().max(2.0) as u16;
            let radius = 1.4 * half;
            let near = reach(&back_mask, &front_mask, w, h, strip);
            let (jx, jy) = (spec.joint[0], spec.joint[1]);
            let mut zone = vec![false; w * h];
            let mut any = false;
            for i in 0..w * h {
                if !front_mask[i] || parts[front].rgba[i * 4 + 3] < SOLID {
                    continue;
                }
                let (x, y) = ((i % w) as f32 - jx, (i / w) as f32 - jy);
                if near[i] <= strip || (x * x + y * y).sqrt() <= radius {
                    zone[i] = true;
                    any = true;
                }
            }
            if !any {
                continue;
            }
            let n = fill_into(&mut parts[back], &zone, w, h);
            parts[back].filled += n;
            report.filled += n;
            // The front layer's seam over the fill: one soft pixel.
            if opts.seam_feather && n > 0 {
                let seam = reach(&back_mask, &zone, w, h, 2);
                for i in 0..w * h {
                    if zone[i] && seam[i] != u16::MAX && seam[i] > 0 {
                        let k = if seam[i] == 1 { 0.5 } else { 0.85 };
                        let a = &mut parts[front].rgba[i * 4 + 3];
                        *a = (*a as f32 * k).round() as u8;
                    }
                }
            }
        }
    }
    Cleaned { parts, labels, report }
}

/// The parts stacked back to front (`order` lists part indices, back
/// first) over a transparent picture, straight-alpha RGBA8.
pub fn recomposite(parts: &[&[u8]], w: usize, h: usize) -> Vec<u8> {
    let mut out = vec![0u8; w * h * 4];
    for layer in parts {
        for i in 0..w * h {
            let (s, d) = (&layer[i * 4..i * 4 + 4], i * 4);
            let sa = s[3] as f32 / 255.0;
            if sa <= 0.0 {
                continue;
            }
            let da = out[d + 3] as f32 / 255.0;
            let oa = sa + da * (1.0 - sa);
            for c in 0..3 {
                let v = (s[c] as f32 * sa + out[d + c] as f32 * da * (1.0 - sa)) / oa.max(1e-6);
                out[d + c] = v.round().clamp(0.0, 255.0) as u8;
            }
            out[d + 3] = (oa * 255.0).round() as u8;
        }
    }
    out
}

/// What the side-by-side check found.
#[derive(Clone, Debug)]
pub struct Diagnosis {
    /// Original | parts | error, left to right.
    pub image: Frame,
    /// The crop shown: `[x0, y0, x1, y1]` in picture pixels.
    pub crop: [usize; 4],
    /// Picture pixels the stack leaves empty (holes).
    pub holes: usize,
    /// Pixels the stack draws where the picture has none.
    pub extras: usize,
    /// Pixels where the stack's colour differs clearly from the picture.
    pub mismatched: usize,
    /// Pixels more than one layer covers (fill behind a part, or duplicates).
    pub stacked: usize,
    /// Visible pixels per part, in the order given.
    pub per_part: Vec<(String, usize)>,
}

const TINTS: [[u8; 3]; 10] = [[230, 80, 80], [80, 170, 240], [90, 200, 110], [240, 190, 60], [180, 100, 220], [70, 210, 200], [240, 130, 50], [150, 150, 250], [200, 90, 140], [120, 120, 120]];

/// Draws the original, the parts and the stack's error side by side over
/// `crop` (the subject's box when `None`) and counts what is wrong.
/// `layers` are `(name, rgba)` from back to front.
pub fn diagnose(original: &Frame, layers: &[(String, Vec<u8>)], crop: Option<[usize; 4]>, panel_width: usize) -> Diagnosis {
    let (w, h) = (original.width, original.height);
    let refs: Vec<&[u8]> = layers.iter().map(|(_, l)| l.as_slice()).collect();
    let stack = recomposite(&refs, w, h);
    let alpha_o = |i: usize| original.rgba[i * 4 + 3];
    // The box shown.
    let crop = crop.map(|c| [c[0].min(w - 1), c[1].min(h - 1), c[2].clamp(c[0] + 1, w), c[3].clamp(c[1] + 1, h)]).unwrap_or_else(|| {
        let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
        for i in 0..w * h {
            if alpha_o(i) >= SEEN || stack[i * 4 + 3] >= SEEN {
                let (x, y) = (i % w, i / w);
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x + 1);
                y1 = y1.max(y + 1);
            }
        }
        if x1 <= x0 {
            return [0, 0, w, h];
        }
        let (mx, my) = ((x1 - x0) / 25 + 2, (y1 - y0) / 25 + 2);
        [x0.saturating_sub(mx), y0.saturating_sub(my), (x1 + mx).min(w), (y1 + my).min(h)]
    });
    let (cw, ch) = (crop[2] - crop[0], crop[3] - crop[1]);
    // Counts over the whole picture.
    let (mut holes, mut extras, mut mismatched, mut stacked) = (0, 0, 0, 0);
    let mut per_part = Vec::new();
    let mut top: Vec<i32> = vec![-1; w * h];
    for (k, (name, l)) in layers.iter().enumerate() {
        let mut n = 0;
        for i in 0..w * h {
            if l[i * 4 + 3] >= 128 {
                top[i] = k as i32;
                n += 1;
            }
        }
        per_part.push((name.clone(), n));
    }
    let mut cover = vec![0u8; w * h];
    for (_, l) in layers {
        for i in 0..w * h {
            cover[i] += (l[i * 4 + 3] >= 128) as u8;
        }
    }
    for i in 0..w * h {
        let (ao, as_) = (alpha_o(i), stack[i * 4 + 3]);
        if ao >= 128 && as_ < 64 {
            holes += 1;
        } else if as_ >= 128 && ao < 32 {
            extras += 1;
        } else if ao >= 128 && as_ >= 128 {
            let d: i32 = (0..3).map(|c| (original.rgba[i * 4 + c] as i32 - stack[i * 4 + c] as i32).abs()).sum();
            if d > 60 {
                mismatched += 1;
            }
        }
        if cover[i] > 1 {
            stacked += 1;
        }
    }
    // Panels.
    let scale = (panel_width.max(64) as f32 / cw as f32).min(4.0);
    let (pw, ph) = (((cw as f32 * scale).round() as usize).max(1), ((ch as f32 * scale).round() as usize).max(1));
    let gap = 6;
    let mut img = Frame::filled(pw * 3 + gap * 2, ph, [255, 255, 255, 255]);
    let at = |px: usize, py: usize| -> usize {
        let sx = (crop[0] as f32 + (px as f32 + 0.5) / scale).floor() as usize;
        let sy = (crop[1] as f32 + (py as f32 + 0.5) / scale).floor() as usize;
        sy.min(h - 1) * w + sx.min(w - 1)
    };
    let checker = |px: usize, py: usize| if (px / 8 + py / 8) % 2 == 0 { 214u8 } else { 238u8 };
    for py in 0..ph {
        for px in 0..pw {
            let i = at(px, py);
            let bg = checker(px, py);
            // Panel 1: the picture.
            let a = original.rgba[i * 4 + 3] as f32 / 255.0;
            let mut put = |panel: usize, rgb: [f32; 3]| {
                let o = (py * img.width + panel * (pw + gap) + px) * 4;
                for c in 0..3 {
                    img.rgba[o + c] = rgb[c].round().clamp(0.0, 255.0) as u8;
                }
            };
            put(0, std::array::from_fn(|c| original.rgba[i * 4 + c] as f32 * a + bg as f32 * (1.0 - a)));
            // Panel 2: each part tinted; seams dark, stacked pixels hatched.
            let k = top[i];
            if k >= 0 {
                let t = TINTS[k as usize % TINTS.len()];
                let right = if px + 1 < pw { at(px + 1, py) } else { i };
                let below = if py + 1 < ph { at(px, py + 1) } else { i };
                let seam = top[right] != k || top[below] != k;
                let mut c: [f32; 3] = std::array::from_fn(|c| original.rgba[i * 4 + c] as f32 * 0.45 + t[c] as f32 * 0.55);
                if seam {
                    c = [30.0, 30.0, 30.0];
                } else if cover[i] > 1 && (px + py) % 6 < 2 {
                    c = [255.0, 255.0, 255.0];
                }
                put(1, c);
            } else {
                put(1, [bg as f32; 3]);
            }
            // Panel 3: where the stack and the picture differ.
            let (ao, as_) = (alpha_o(i), stack[i * 4 + 3]);
            let c3: [f32; 3] = if ao >= 128 && as_ < 64 {
                [230.0, 40.0, 40.0]
            } else if as_ >= 128 && ao < 32 {
                [50.0, 90.0, 235.0]
            } else if ao >= 128 && as_ >= 128 {
                let d: f32 = (0..3).map(|c| (original.rgba[i * 4 + c] as f32 - stack[i * 4 + c] as f32).abs()).sum::<f32>();
                let g = (d * 3.0).min(255.0);
                [g.max(60.0), g.max(60.0), (g * 0.4).max(60.0)]
            } else {
                [bg as f32; 3]
            };
            put(2, c3);
        }
    }
    Diagnosis { image: img, crop, holes, extras, mismatched, stacked, per_part }
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: usize = 120;
    const H: usize = 160;

    /// A figure: torso, a head, an arm in front hanging at the side and an
    /// arm behind the torso, each a different colour with a little texture,
    /// anti-aliased at the outside edge.
    struct Figure {
        frame: Frame,
        /// Ground-truth part of each pixel (0 none).
        truth: Vec<u16>,
        specs: Vec<PartSpec>,
    }

    fn figure() -> Figure {
        let mut rgba = vec![0u8; W * H * 4];
        let mut truth = vec![0u16; W * H];
        let torso = [(40.0f32, 50.0f32), (80.0, 115.0)];
        let head_c = (60.0f32, 32.0f32);
        let front_arm = [(80.0f32, 52.0f32), (92.0, 112.0)];
        let back_arm = [(40.0f32, 52.0f32), (30.0, 100.0)];
        let colours: [[u8; 3]; 4] = [[60, 90, 170], [225, 190, 160], [170, 60, 60], [60, 140, 90]];
        // 1 torso, 2 head, 3 front arm, 4 back arm.
        let in_rect = |p: (f32, f32), r: &[(f32, f32); 2]| p.0 >= r[0].0 && p.0 < r[1].0 && p.1 >= r[0].1 && p.1 < r[1].1;
        let seg_dist = |p: (f32, f32), a: (f32, f32), b: (f32, f32)| {
            let (dx, dy) = (b.0 - a.0, b.1 - a.1);
            let t = (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
            ((p.0 - a.0 - t * dx).powi(2) + (p.1 - a.1 - t * dy).powi(2)).sqrt()
        };
        for y in 0..H {
            for x in 0..W {
                let p = (x as f32 + 0.5, y as f32 + 0.5);
                // Back to front: back arm, torso, head, front arm.
                let mut part = 0u16;
                let mut cover = 0.0f32;
                let mut consider = |id: u16, d: f32| {
                    // d is the signed distance inside (positive = inside).
                    let c = (d + 0.5).clamp(0.0, 1.0);
                    if c > 0.0 {
                        part = id;
                        cover = cover + c * (1.0 - cover);
                    }
                };
                consider(4, 6.0 - seg_dist(p, back_arm[0], back_arm[1]));
                if in_rect(p, &torso) {
                    consider(1, (p.0 - torso[0].0).min(torso[1].0 - p.0).min(p.1 - torso[0].1).min(torso[1].1 - p.1));
                }
                consider(2, 14.0 - ((p.0 - head_c.0).powi(2) + (p.1 - head_c.1).powi(2)).sqrt());
                consider(3, 7.0 - seg_dist(p, front_arm[0], front_arm[1]));
                if part > 0 {
                    let id = part as usize - 1;
                    let tex = (((x * 7 + y * 13) % 11) as i32 - 5) as i32;
                    let i = (y * W + x) * 4;
                    for c in 0..3 {
                        rgba[i + c] = (colours[id][c] as i32 + tex).clamp(0, 255) as u8;
                    }
                    rgba[i + 3] = (cover.min(1.0) * 255.0).round() as u8;
                    truth[y * W + x] = part;
                }
            }
        }
        let specs = vec![
            PartSpec { name: "torso".into(), parent: None, joint: [60.0, 80.0], depth: 1.0 },
            PartSpec { name: "head".into(), parent: Some(0), joint: [60.0, 46.0], depth: 2.0 },
            PartSpec { name: "arm_front".into(), parent: Some(0), joint: [80.0, 54.0], depth: 3.0 },
            PartSpec { name: "arm_back".into(), parent: Some(0), joint: [40.0, 54.0], depth: 0.0 },
        ];
        Figure { frame: Frame { width: W, height: H, rgba }, truth, specs }
    }

    /// Labels as a rough automatic split would give them: the true parts,
    /// but with the faults seen in practice. A hole in the torso, loose
    /// speckle, a stray island of the arm in the torso.
    fn faulty(truth: &[u16]) -> Vec<u16> {
        let mut l = truth.to_vec();
        for y in 70..76 {
            for x in 52..58 {
                l[y * W + x] = 0;
            }
        }
        for k in 0..30 {
            let (x, y) = (45 + (k * 7) % 30, 55 + (k * 11) % 50);
            if truth[y * W + x] == 1 {
                l[y * W + x] = 3;
            }
        }
        for y in 90..93 {
            for x in 60..63 {
                if truth[y * W + x] == 1 {
                    l[y * W + x] = 3;
                }
            }
        }
        l
    }

    fn coverage(parts: &Cleaned, i: usize) -> u32 {
        parts.parts.iter().enumerate().filter(|(_, p)| !p.behind[i]).map(|(_, p)| p.rgba[i * 4 + 3] as u32).sum()
    }

    #[test]
    fn tidy_labels_closes_holes_and_absorbs_speckle() {
        let f = figure();
        let mut l = faulty(&f.truth);
        let alpha: Vec<u8> = f.frame.rgba.chunks_exact(4).map(|p| p[3]).collect();
        let mut rep = Report::default();
        tidy_labels(&mut l, &alpha, W, H, &mut rep);
        assert!(rep.holes_assigned >= 30, "{rep:?}");
        assert!(rep.flipped + rep.islands_merged > 0, "{rep:?}");
        for i in 0..W * H {
            if alpha[i] >= SEEN {
                assert!(l[i] != 0, "pixel {i} still unowned");
            }
        }
        // The torso got back its hole and its speckle; the arms are intact.
        let wrong = (0..W * H).filter(|&i| f.truth[i] != 0 && alpha[i] >= 200 && l[i] != f.truth[i]).count();
        assert!(wrong < 20, "{wrong} pixels still in the wrong part");
    }

    #[test]
    fn every_visible_pixel_is_in_exactly_one_part() {
        let f = figure();
        let out = separate(&f.frame, &faulty(&f.truth), &f.specs, &Options::default());
        let mut dup = 0;
        let mut missing = 0;
        for i in 0..W * H {
            let src = f.frame.rgba[i * 4 + 3] as i32;
            // Real pixels only: what was painted in behind is not the picture.
            let real: i32 = out.parts.iter().filter(|p| !p.behind[i]).map(|p| p.rgba[i * 4 + 3] as i32).sum();
            if src >= SEEN as i32 {
                if real > src + 1 {
                    dup += 1;
                }
                // A seam pixel of the front layer is softened (and filled
                // under), so only solid-interior pixels must be whole.
                if src >= 250 && !out.parts.iter().any(|p| p.behind[i]) && real < src - 1 {
                    missing += 1;
                }
            } else if real > 0 {
                dup += 1;
            }
            let _ = coverage(&out, i);
        }
        assert_eq!((dup, missing), (0, 0), "duplicated {dup}, missing {missing}");
        // The parts hold the whole picture between them.
        let total: usize = out.parts.iter().map(|p| p.pixels).sum();
        let opaque = (0..W * H).filter(|&i| f.frame.rgba[i * 4 + 3] >= SEEN).count();
        assert_eq!(total, opaque);
    }

    #[test]
    fn what_a_part_hides_is_filled_behind_it() {
        let f = figure();
        let out = separate(&f.frame, &faulty(&f.truth), &f.specs, &Options::default());
        assert!(out.report.filled > 100, "{:?}", out.report);
        let torso = &out.parts[0];
        // The front arm covers the torso's right edge: under it the torso
        // layer now has body colour, not a hole and not the arm's red.
        let mut seen = 0;
        for y in 60..100 {
            for x in 76..80 {
                let i = y * W + x;
                if f.truth[i] == 3 && f.frame.rgba[i * 4 + 3] == 255 {
                    seen += 1;
                    let p = &torso.rgba[i * 4..i * 4 + 4];
                    assert_eq!(p[3], 255, "no fill at ({x}, {y})");
                    assert!(p[2] > p[0], "torso fill should be blue-ish, got {p:?}");
                }
            }
        }
        assert!(seen > 10);
        assert!(torso.filled > 0 && out.parts[2].filled == 0);
        // The back arm hides behind the torso: the arm layer has its
        // hidden shoulder filled instead.
        assert!(out.parts[3].filled > 0, "back arm: {:?}", out.parts[3].filled);
    }

    #[test]
    fn the_stack_at_rest_is_the_picture() {
        let f = figure();
        let out = separate(&f.frame, &faulty(&f.truth), &f.specs, &Options::default());
        // Back to front by depth.
        let mut order: Vec<usize> = (0..f.specs.len()).collect();
        order.sort_by(|&a, &b| f.specs[a].depth.total_cmp(&f.specs[b].depth));
        let layers: Vec<&[u8]> = order.iter().map(|&k| out.parts[k].rgba.as_slice()).collect();
        let stack = recomposite(&layers, W, H);
        // Away from the seams between parts the stack is the picture; on a
        // seam the front layer is one soft pixel over the fill, a blend like
        // an anti-aliased edge.
        let near_seam = |i: usize| {
            let (x, y) = ((i % W) as isize, (i / W) as isize);
            (-2isize..=2).any(|dy| (-2isize..=2).any(|dx| {
                let (nx, ny) = (x + dx, y + dy);
                nx >= 0 && ny >= 0 && nx < W as isize && ny < H as isize && f.truth[ny as usize * W + nx as usize] != f.truth[i]
            }))
        };
        let differs = |stack: &[u8], skip_seams: bool| {
            let mut bad = 0;
            for i in 0..W * H {
                if skip_seams && near_seam(i) {
                    continue;
                }
                let (a, b) = (f.frame.rgba[i * 4 + 3] as i32, stack[i * 4 + 3] as i32);
                if a >= 250 {
                    let d: i32 = (0..3).map(|c| (f.frame.rgba[i * 4 + c] as i32 - stack[i * 4 + c] as i32).abs()).sum();
                    if b < 250 || d > 24 {
                        bad += 1;
                    }
                } else if a == 0 && b > 8 {
                    bad += 1;
                }
            }
            bad
        };
        assert_eq!(differs(&stack, true), 0, "pixels away from the seams differ at rest");
        // With the seam feather off, the stack is the picture everywhere solid.
        let hard = separate(&f.frame, &faulty(&f.truth), &f.specs, &Options { seam_feather: false, ..Options::default() });
        let layers: Vec<&[u8]> = order.iter().map(|&k| hard.parts[k].rgba.as_slice()).collect();
        let exact = recomposite(&layers, W, H);
        assert!(differs(&exact, false) < 8, "{} pixels differ with hard seams", differs(&exact, false));
        // Without fills the same stack is still right at rest; the fills are for motion.
        let plain = separate(&f.frame, &faulty(&f.truth), &f.specs, &Options { fill_behind: false, ..Options::default() });
        assert_eq!(plain.report.filled, 0);
        // Lift the front arm away: the torso under it is body, with no hole.
        let mut lifted = out.parts.clone();
        lifted[2].rgba = vec![0; W * H * 4];
        let order: Vec<&[u8]> = order.iter().map(|&k| lifted[k].rgba.as_slice()).collect();
        let moved = recomposite(&order, W, H);
        let mut hole = 0;
        for y in 62..100 {
            for x in 76..79 {
                if f.truth[y * W + x] == 3 && moved[(y * W + x) * 4 + 3] < 250 {
                    hole += 1;
                }
            }
        }
        assert_eq!(hole, 0, "a hole where the arm was");
        let torso_plain = &plain.parts[0];
        let open = (62..100).flat_map(|y| (76..79).map(move |x| y * W + x)).filter(|&i| f.truth[i] == 3 && torso_plain.rgba[i * 4 + 3] < 250).count();
        assert!(open > 10, "the unfilled torso should show the hole ({open})");
    }

    #[test]
    fn each_part_is_one_connected_piece() {
        let f = figure();
        let out = separate(&f.frame, &faulty(&f.truth), &f.specs, &Options::default());
        for (k, p) in out.parts.iter().enumerate() {
            let real: Vec<bool> = (0..W * H).map(|i| p.rgba[i * 4 + 3] >= SEEN && !p.behind[i]).collect();
            let start = real.iter().position(|&b| b).expect("part has pixels");
            let seen = reach(&(0..W * H).map(|i| i == start).collect::<Vec<_>>(), &real, W, H, u16::MAX - 1);
            let reached = seen.iter().filter(|&&d| d != u16::MAX).count();
            assert_eq!(reached, real.iter().filter(|&&b| b).count(), "part {k} ({}) is in pieces", p.name);
        }
    }

    #[test]
    fn the_diagnosis_counts_and_draws_three_panels() {
        let f = figure();
        let out = separate(&f.frame, &faulty(&f.truth), &f.specs, &Options::default());
        let mut order: Vec<usize> = (0..f.specs.len()).collect();
        order.sort_by(|&a, &b| f.specs[a].depth.total_cmp(&f.specs[b].depth));
        let layers: Vec<(String, Vec<u8>)> = order.iter().map(|&k| (out.parts[k].name.clone(), out.parts[k].rgba.clone())).collect();
        let d = diagnose(&f.frame, &layers, None, 160);
        assert!(d.holes < 5 && d.extras < 5, "holes {} extras {}", d.holes, d.extras);
        assert!(d.stacked > 0, "fills lie under their parts");
        assert_eq!(d.per_part.len(), 4);
        assert!(d.image.width > 160 * 3 && d.image.height > 100);
        // A broken stack (a part missing) shows its hole.
        let mut broken = layers.clone();
        broken.retain(|(n, _)| n != "arm_front");
        let b = diagnose(&f.frame, &broken, None, 160);
        assert!(b.holes > 100, "{}", b.holes);
        assert_ne!(b.image.rgba, d.image.rgba);
        // A crop is honoured.
        let c = diagnose(&f.frame, &layers, Some([70, 40, 110, 120]), 100);
        assert_eq!(c.crop, [70, 40, 110, 120]);
    }
}
