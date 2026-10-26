// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Faces for `puppet`: where a character's eyes and mouth are in its
//! picture, a mouth that follows a voice, blinks, and eyes that look.
//!
//! Everything here is pure (pictures and sounds in, numbers out); the
//! `puppet` op turns the numbers into pin keys (one picture) or layer
//! swaps (a character made of parts). Points are pixels of the analysed
//! picture unless they say otherwise.

/// A character's picture: its pixels and which of them are the character.
pub(super) struct Picture {
    pub w: usize,
    pub h: usize,
    px: Vec<[u8; 3]>,
    on: Vec<bool>,
}

fn luma(p: [u8; 3]) -> f64 {
    0.299 * p[0] as f64 + 0.587 * p[1] as f64 + 0.114 * p[2] as f64
}

fn chroma(p: [u8; 3]) -> i32 {
    let (lo, hi) = (p.iter().min().copied().unwrap_or(0), p.iter().max().copied().unwrap_or(0));
    hi as i32 - lo as i32
}

impl Picture {
    /// From RGBA pixels: the character is what is opaque, or (with no
    /// transparency) what differs from the colour around the border.
    pub fn new(w: usize, h: usize, rgba: &[[u8; 4]]) -> Option<Picture> {
        if w < 8 || h < 8 || rgba.len() < w * h {
            return None;
        }
        let px: Vec<[u8; 3]> = rgba[..w * h].iter().map(|p| [p[0], p[1], p[2]]).collect();
        let mut on: Vec<bool> = rgba[..w * h].iter().map(|p| p[3] >= 128).collect();
        if on.iter().all(|&b| b) {
            let mut border: Vec<[u8; 3]> = (0..w).flat_map(|x| [px[x], px[(h - 1) * w + x]]).collect();
            border.extend((0..h).flat_map(|y| [px[y * w], px[y * w + w - 1]]));
            let mut bg = [0u8; 3];
            for (c, b) in bg.iter_mut().enumerate() {
                let mut v: Vec<u8> = border.iter().map(|p| p[c]).collect();
                v.sort_unstable();
                *b = v[v.len() / 2];
            }
            for (o, p) in on.iter_mut().zip(&px) {
                *o = (0..3).map(|c| (p[c] as i32 - bg[c] as i32).pow(2)).sum::<i32>() > 40 * 40;
            }
        }
        Some(Picture { w, h, px, on })
    }

    /// The part of a PNG a crop leaves (fractions cut from the left, top,
    /// right and bottom), at most about 900 px across.
    pub fn of_png(path: &std::path::Path, crop: (f64, f64, f64, f64)) -> Option<Picture> {
        let mut d = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).ok()?));
        d.set_transformations(png::Transformations::normalize_to_color8());
        let mut r = d.read_info().ok()?;
        let mut buf = vec![0; r.output_buffer_size()?];
        let info = r.next_frame(&mut buf).ok()?;
        let (w, h) = (info.width as usize, info.height as usize);
        let chans = match info.color_type {
            png::ColorType::Grayscale => 1,
            png::ColorType::GrayscaleAlpha => 2,
            png::ColorType::Rgb => 3,
            png::ColorType::Rgba => 4,
            png::ColorType::Indexed => return None,
        };
        let (l, t, rr, b) = crop;
        let x0 = (l.clamp(0.0, 1.0) * w as f64).round() as usize;
        let y0 = (t.clamp(0.0, 1.0) * h as f64).round() as usize;
        let x1 = ((1.0 - rr.clamp(0.0, 1.0)) * w as f64).round() as usize;
        let y1 = ((1.0 - b.clamp(0.0, 1.0)) * h as f64).round() as usize;
        if x1 <= x0 + 8 || y1 <= y0 + 8 {
            return None;
        }
        let step = ((x1 - x0).max(y1 - y0) / 900).max(1);
        let mut out = Vec::new();
        let (mut ow, mut oh) = (0, 0);
        for y in (y0..y1).step_by(step) {
            oh += 1;
            ow = 0;
            for x in (x0..x1).step_by(step) {
                ow += 1;
                let p = &buf[(y * w + x) * chans..(y * w + x + 1) * chans];
                out.push(match chans {
                    1 => [p[0], p[0], p[0], 255],
                    2 => [p[0], p[0], p[0], p[1]],
                    3 => [p[0], p[1], p[2], 255],
                    _ => [p[0], p[1], p[2], p[3]],
                });
            }
        }
        Picture::new(ow, oh, &out)
    }

    fn at(&self, x: i64, y: i64) -> Option<[u8; 3]> {
        (x >= 0 && y >= 0 && (x as usize) < self.w && (y as usize) < self.h && self.on[y as usize * self.w + x as usize]).then(|| self.px[y as usize * self.w + x as usize])
    }

    fn bounds(&self) -> Option<(usize, usize, usize, usize)> {
        let (mut x0, mut y0, mut x1, mut y1) = (self.w, self.h, 0, 0);
        for y in 0..self.h {
            for x in 0..self.w {
                if self.on[y * self.w + x] {
                    (x0, x1, y0, y1) = (x0.min(x), x1.max(x), y0.min(y), y1.max(y));
                }
            }
        }
        (x1 > x0 && y1 > y0).then_some((x0, y0, x1, y1))
    }
}

/// An eye: the middle of its white, its half-width and half-height, and
/// where its pupil sits now (pixels).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Eye {
    pub c: (f64, f64),
    pub rx: f64,
    pub ry: f64,
    pub pupil: (f64, f64),
}

/// The mouth: the middle of its line and its width (pixels).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Mouth {
    pub c: (f64, f64),
    pub w: f64,
}

/// What `find` saw.
#[derive(Clone, Debug)]
pub(super) struct Found {
    pub eyes: Vec<Eye>,
    pub mouth: Option<Mouth>,
    /// The mouth was placed from where the eyes are (no clear line seen).
    pub mouth_guessed: bool,
}

/// A dark, roundish patch: a pupil candidate.
struct Blob {
    c: (f64, f64),
    r: f64,
    white: f64,
}

/// Dark round patches with white around them in the upper part of the
/// character: pupils in their eye whites.
fn pupils(p: &Picture, (x0, y0, x1, y1): (usize, usize, usize, usize)) -> Vec<Blob> {
    let s = (y1 - y0).max(x1 - x0) as f64;
    let top = y0 + ((y1 - y0) as f64 * 0.72) as usize;
    let mut seen = vec![false; p.w * p.h];
    let mut out = Vec::new();
    // Dark pixels with dark all round them: thin lash and outline lines
    // drop out, solid pupils stay.
    let raw: Vec<bool> = (0..p.w * p.h).map(|i| p.on[i] && luma(p.px[i]) < 62.0).collect();
    let e = ((0.005 * s).round() as i64).max(1);
    let core: Vec<bool> = (0..p.w * p.h)
        .map(|i| {
            let (x, y) = ((i % p.w) as i64, (i / p.w) as i64);
            raw[i] && (-e..=e).all(|dy| (-e..=e).all(|dx| { let (nx, ny) = (x + dx, y + dy); nx >= 0 && ny >= 0 && (nx as usize) < p.w && (ny as usize) < p.h && raw[ny as usize * p.w + nx as usize] }))
        })
        .collect();
    let dark = |x: usize, y: usize| core[y * p.w + x];
    for sy in y0..=top {
        for sx in x0..=x1 {
            if seen[sy * p.w + sx] || !dark(sx, sy) {
                continue;
            }
            let mut stack = vec![(sx, sy)];
            seen[sy * p.w + sx] = true;
            let (mut n, mut cx, mut cy) = (0usize, 0.0, 0.0);
            let (mut bx0, mut by0, mut bx1, mut by1) = (sx, sy, sx, sy);
            while let Some((x, y)) = stack.pop() {
                n += 1;
                cx += x as f64;
                cy += y as f64;
                (bx0, bx1, by0, by1) = (bx0.min(x), bx1.max(x), by0.min(y), by1.max(y));
                for (dx, dy) in [(-1i64, 0i64), (1, 0), (0, -1), (0, 1)] {
                    let (nx, ny) = (x as i64 + dx, y as i64 + dy);
                    if nx < 0 || ny < 0 || nx as usize >= p.w || ny as usize >= p.h {
                        continue;
                    }
                    let (nx, ny) = (nx as usize, ny as usize);
                    if !seen[ny * p.w + nx] && dark(nx, ny) {
                        seen[ny * p.w + nx] = true;
                        stack.push((nx, ny));
                    }
                }
            }
            let r = (n as f64 / std::f64::consts::PI).sqrt() + e as f64;
            let (bw, bh) = ((bx1 - bx0 + 1) as f64, (by1 - by0 + 1) as f64);
            if r < (0.005 * s).max(1.5) || r > 0.07 * s || n as f64 / (bw * bh) < 0.45 || bw / bh > 2.6 || bh / bw > 2.6 {
                continue;
            }
            let c = (cx / n as f64, cy / n as f64);
            // White in a ring around it: the eye's white.
            let (mut all, mut white) = (0usize, 0usize);
            let (ri, ro) = (1.2 * r, 2.4 * r + 2.0);
            for y in (c.1 - ro) as i64..=(c.1 + ro) as i64 {
                for x in (c.0 - ro) as i64..=(c.0 + ro) as i64 {
                    let d = ((x as f64 - c.0).powi(2) + (y as f64 - c.1).powi(2)).sqrt();
                    if d < ri || d > ro {
                        continue;
                    }
                    if let Some(q) = p.at(x, y) {
                        all += 1;
                        if luma(q) > 150.0 && chroma(q) < 40 {
                            white += 1;
                        }
                    }
                }
            }
            let white = white as f64 / all.max(1) as f64;
            if white >= 0.045 {
                out.push(Blob { c, r, white });
            }
        }
    }
    out
}

/// How far the eye's white reaches from the pupil along `dir`.
fn reach(p: &Picture, b: &Blob, dir: (f64, f64)) -> f64 {
    let mut last = b.r;
    let mut d = b.r;
    while d < 5.0 * b.r + 3.0 {
        let (x, y) = ((b.c.0 + dir.0 * d).round() as i64, (b.c.1 + dir.1 * d).round() as i64);
        match p.at(x, y) {
            Some(q) if (luma(q) > 160.0 && chroma(q) < 48) || luma(q) < 70.0 => last = d,
            Some(_) if d - last > 1.2 * b.r + 1.0 => break,
            None => break,
            _ => {}
        }
        d += 0.5;
    }
    (last + 0.5).clamp(1.2 * b.r, 4.0 * b.r)
}

fn eye_of(p: &Picture, b: &Blob) -> Eye {
    let (l, r) = (reach(p, b, (-1.0, 0.0)), reach(p, b, (1.0, 0.0)));
    let (u, d) = (reach(p, b, (0.0, -1.0)), reach(p, b, (0.0, 1.0)));
    Eye { c: (b.c.0 + (r - l) / 2.0, b.c.1 + (d - u) / 2.0), rx: (l + r) / 2.0, ry: (u + d) / 2.0, pupil: b.c }
}

/// Finds the eyes (a pair, or one in a side view) and the mouth. `right`
/// is which way the character faces in its picture (`None`: the camera).
pub(super) fn find(p: &Picture, right: Option<bool>) -> Option<Found> {
    let bx = p.bounds()?;
    let s = (bx.3 - bx.1).max(bx.2 - bx.0) as f64;
    let blobs = pupils(p, bx);
    let single = blobs.iter().max_by(|a, b| (a.white * a.r).total_cmp(&(b.white * b.r)));
    let mut pair: Option<(usize, usize, f64)> = None;
    for i in 0..blobs.len() {
        for j in i + 1..blobs.len() {
            let (a, b) = (&blobs[i], &blobs[j]);
            let (dx, dy) = ((a.c.0 - b.c.0).abs(), (a.c.1 - b.c.1).abs());
            let ratio = a.r.max(b.r) / a.r.min(b.r);
            if ratio > 2.2 || dy > 0.06 * s + a.r.max(b.r) || dx < 3.0 * a.r.max(b.r) || dx > 0.45 * s {
                continue;
            }
            let score = a.white.min(b.white) * 2.0 - dy / s;
            if pair.is_none_or(|q| score > q.2) {
                pair = Some((i, j, score));
            }
        }
    }
    let eyes: Vec<Eye> = match (pair, single) {
        (Some((i, j, _)), _) => {
            let mut e = vec![eye_of(p, &blobs[i]), eye_of(p, &blobs[j])];
            e.sort_by(|a, b| a.c.0.total_cmp(&b.c.0));
            e
        }
        (_, Some(one)) => vec![eye_of(p, one)],
        _ => Vec::new(),
    };
    let Some(e0) = eyes.first() else { return Some(Found { eyes, mouth: None, mouth_guessed: false }) };
    let n = eyes.len() as f64;
    let ec = (eyes.iter().map(|e| e.c.0).sum::<f64>() / n, eyes.iter().map(|e| e.c.1).sum::<f64>() / n);
    let unit = if eyes.len() == 2 { (eyes[1].c.0 - eyes[0].c.0).abs().max(3.0 * e0.rx) } else { 3.2 * e0.rx };
    // Which way the face points: as told, or from where the pupils sit
    // in their whites (characters look where they face).
    let fwd = match right {
        Some(true) => 1.0,
        Some(false) => -1.0,
        None => {
            let lean = eyes.iter().map(|e| e.pupil.0 - e.c.0).sum::<f64>() / n / e0.rx;
            if lean.abs() > 0.25 { lean.signum() * 0.5 } else { 0.0 }
        }
    };
    let side = if eyes.len() == 1 { 0.75 } else { 0.25 };
    let guess = (ec.0 + fwd * side * unit, ec.1 + 1.05 * unit);
    // The mouth is the strongest short dark line below the eyes: darker
    // than the face just above and just below it.
    let k = (0.05 * unit).max(2.0) as i64;
    let half = (0.18 * unit).max(3.0) as i64;
    let mut best = (0.0f64, guess.0, guess.1);
    let crease = |x: i64, y: i64| -> Option<f64> {
        let (a, m, b) = (p.at(x, y - k)?, p.at(x, y)?, p.at(x, y + k)?);
        Some(((luma(a) + luma(b)) / 2.0 - luma(m)).max(0.0).min(90.0))
    };
    let (ya, yb) = ((ec.1 + 0.45 * unit) as i64, (ec.1 + 1.9 * unit) as i64);
    let (xa, xb) = ((guess.0 - 0.75 * unit) as i64, (guess.0 + 0.75 * unit) as i64);
    for y in ya..=yb {
        for x in xa..=xb {
            if p.at(x, y).is_none() {
                continue;
            }
            let mut sum = 0.0;
            for dx in -half..=half {
                sum += (-1..=1).filter_map(|dy| crease(x + dx, y + dy)).fold(0.0, f64::max);
            }
            let sum = sum / (2 * half + 1) as f64;
            // Nearer the expected place wins ties.
            let near = 1.0 - 0.25 * (((x as f64 - guess.0) / unit).powi(2) + ((y as f64 - guess.1) / unit).powi(2)).sqrt();
            if sum * near > best.0 {
                best = (sum * near, x as f64, y as f64);
            }
        }
    }
    let (mouth, guessed) = if best.0 >= 9.0 {
        // Its width: how far the line carries either side.
        let (cx, cy) = (best.1 as i64, best.2 as i64);
        let line = |x: i64| (-2..=2).filter_map(|dy| crease(x, cy + dy)).fold(0.0, f64::max) > 0.3 * best.0 + 3.0;
        let (mut l, mut r) = (cx, cx);
        let gap = (0.06 * unit).max(2.0) as i64;
        let (mut miss_l, mut miss_r) = (0, 0);
        while cx - l < unit as i64 && miss_l <= gap {
            l -= 1;
            miss_l = if line(l) { 0 } else { miss_l + 1 };
        }
        while r - cx < unit as i64 && miss_r <= gap {
            r += 1;
            miss_r = if line(r) { 0 } else { miss_r + 1 };
        }
        let (l, r) = ((l + miss_l) as f64, (r - miss_r) as f64);
        let w = (r - l).clamp(0.3 * unit, 1.1 * unit);
        (Mouth { c: ((l + r) / 2.0, best.2), w }, false)
    } else {
        (Mouth { c: guess, w: 0.55 * unit }, true)
    };
    Some(Found { eyes, mouth: Some(mouth), mouth_guessed: guessed })
}

/// One moment of a mouth: how open (0 shut … 1 wide open) and its shape
/// (-1 rounded "oo" … 0 relaxed … 1 stretched "ee").
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Pose {
    pub t: f64,
    pub open: f64,
    pub wide: f64,
}

/// A mouth shape a character made of parts has a layer for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum Shape {
    Closed,
    Open,
    Wide,
    Round,
}

pub(super) const SHAPES: &str = "closed (m b p, rest), open (a), wide (e i, teeth), round (o u w)";

impl Shape {
    pub fn named(n: &str) -> Option<Shape> {
        match n.to_lowercase().trim() {
            "closed" | "rest" | "m" | "mbp" | "shut" | "x" => Some(Shape::Closed),
            "open" | "a" | "ah" | "aa" => Some(Shape::Open),
            "wide" | "e" | "ee" | "i" | "smile" | "teeth" => Some(Shape::Wide),
            "round" | "o" | "oo" | "u" | "w" => Some(Shape::Round),
            _ => None,
        }
    }

    /// The shape a pose reads as.
    pub fn of(p: &Pose) -> Shape {
        if p.open < 0.18 {
            Shape::Closed
        } else if p.wide > 0.45 {
            Shape::Wide
        } else if p.wide < -0.45 {
            Shape::Round
        } else {
            Shape::Open
        }
    }
}

/// The vowel sound of a run of letters, as a mouth shape (-1 round … 1
/// wide) and how far it opens.
fn vowel(v: &str) -> (f64, f64) {
    match v {
        "oo" | "ou" | "ow" | "u" | "ue" | "ew" | "w" => (-0.9, 0.45),
        "o" | "oa" | "oe" | "oy" | "oi" => (-0.7, 0.75),
        "ee" | "ea" | "ie" | "ei" | "i" | "y" | "ey" | "ay" | "ai" => (0.85, 0.5),
        "e" => (0.55, 0.6),
        "a" | "au" | "aw" | "ar" => (0.1, 1.0),
        _ => (0.0, 0.7),
    }
}

/// The vowel groups of a word, in order (a written word's syllables).
fn syllables(word: &str) -> Vec<(f64, f64)> {
    let w: String = word.to_lowercase().chars().filter(|c| c.is_ascii_alphabetic()).collect();
    let b = w.as_bytes();
    let is_v = |i: usize| matches!(b[i], b'a' | b'e' | b'i' | b'o' | b'u') || (b[i] == b'y' && i > 0 && !matches!(b[i - 1], b'a' | b'e' | b'i' | b'o' | b'u'));
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if is_v(i) {
            let s = i;
            while i < b.len() && is_v(i) {
                i += 1;
            }
            let mut g = w[s..i].to_owned();
            // A w after a vowel rounds it ("ow", "ew").
            if i < b.len() && b[i] == b'w' {
                g.push('w');
            }
            // A silent final e.
            if !(g == "e" && i == b.len() && !out.is_empty()) {
                out.push(vowel(&g));
            }
        } else {
            i += 1;
        }
    }
    if out.is_empty() && !w.is_empty() {
        out.push((0.0, 0.6));
    }
    out
}

/// Lips that close at the start of a word (m, b, p) or touch the teeth
/// (f, v).
fn closes(word: &str) -> Option<f64> {
    match word.to_lowercase().chars().find(|c| c.is_ascii_alphabetic())? {
        'm' | 'b' | 'p' => Some(0.0),
        'f' | 'v' => Some(0.12),
        _ => None,
    }
}

/// Loudness of a voice, 100 times a second, 0 (silence) … 1 (its loud
/// syllables), and how hissy each moment is (zero crossings).
fn loudness(mono: &[f32], rate: u32) -> (Vec<f64>, Vec<f64>) {
    let hop = (rate as usize / 100).max(1);
    let win = 2 * hop;
    let mut db = Vec::new();
    let mut zc = Vec::new();
    let mut i = 0;
    while i < mono.len() {
        let w = &mono[i..(i + win).min(mono.len())];
        let e = w.iter().map(|&x| (x as f64).powi(2)).sum::<f64>() / w.len().max(1) as f64;
        db.push(10.0 * (e + 1e-10).log10());
        zc.push(w.windows(2).filter(|p| (p[0] >= 0.0) != (p[1] >= 0.0)).count() as f64 / w.len().max(1) as f64);
        i += hop;
    }
    let mut sorted = db.clone();
    sorted.sort_by(f64::total_cmp);
    let pc = |q: f64| sorted.get(((sorted.len() as f64 - 1.0) * q) as usize).copied().unwrap_or(-100.0);
    let peak = pc(0.95);
    let floor = (pc(0.1) + 6.0).max(peak - 38.0).min(peak - 12.0);
    (db.iter().map(|&d| ((d - floor) / (peak - floor)).clamp(0.0, 1.0)).collect(), zc)
}

/// A mouth that follows a voice. `mono` at `rate` is the line; `words`
/// (text, start, end in seconds from its start) shape the vowels when
/// known. Poses are at syllables and the dips between them, shut through
/// pauses, and lead the sound by a frame or so (lips move before the
/// sound is heard).
pub(super) fn lip_sync(mono: &[f32], rate: u32, words: &[(String, f64, f64)]) -> Vec<Pose> {
    let (loud, zc) = loudness(mono, rate);
    let n = loud.len();
    if n < 3 {
        return Vec::new();
    }
    // Smoothed a little (30 ms) so a syllable is one hump.
    let sm: Vec<f64> = (0..n).map(|i| loud[i.saturating_sub(1)..(i + 2).min(n)].iter().sum::<f64>() / (i + 2).min(n).saturating_sub(i.saturating_sub(1)) as f64).collect();
    let t = |i: usize| i as f64 / 100.0;
    // Syllables: humps at least 0.1 s apart with a dip between them.
    let mut peaks: Vec<usize> = Vec::new();
    for i in 1..n - 1 {
        if sm[i] >= sm[i - 1] && sm[i] > sm[i + 1] && sm[i] > 0.22 {
            if let Some(&last) = peaks.last() {
                let dip = sm[last..=i].iter().cloned().fold(f64::INFINITY, f64::min);
                if i - last < 10 || dip > sm[i].min(sm[last]) - 0.1 {
                    if sm[i] > sm[last] {
                        *peaks.last_mut().unwrap() = i;
                    }
                    continue;
                }
            }
            peaks.push(i);
        }
    }
    let zmid = {
        let mut z: Vec<f64> = peaks.iter().map(|&i| zc[i]).collect();
        z.sort_by(f64::total_cmp);
        z.get(z.len() / 2).copied().unwrap_or(0.1)
    };
    // Which written syllable each heard one is: spread a word's vowel
    // groups over the humps that fall inside it.
    let mut shape: Vec<Option<(f64, f64)>> = vec![None; peaks.len()];
    for (text, ws, we) in words {
        let inside: Vec<usize> = (0..peaks.len()).filter(|&k| t(peaks[k]) >= ws - 0.03 && t(peaks[k]) < we + 0.03).collect();
        let syl = syllables(text);
        for (j, &k) in inside.iter().enumerate() {
            let g = syl[(j * syl.len() / inside.len().max(1)).min(syl.len().saturating_sub(1))];
            shape[k] = Some(g);
        }
    }
    let mut poses = vec![Pose { t: 0.0, open: 0.0, wide: 0.0 }];
    for (k, &i) in peaks.iter().enumerate() {
        let (wide, size) = shape[k].unwrap_or_else(|| {
            // Unknown words: hiss reads as teeth (wide), a dull hum as round.
            let z = ((zc[i] - zmid) / zmid.max(1e-3)).clamp(-1.0, 1.0);
            (0.6 * z, 0.8)
        });
        // The dip before this syllable: shut if it is a real pause.
        if let Some(&prev) = k.checked_sub(1).and_then(|p| peaks.get(p)) {
            let (j, low) = (prev..=i).map(|j| (j, sm[j])).min_by(|a, b| a.1.total_cmp(&b.1)).unwrap();
            let quiet = (prev..=i).filter(|&j| sm[j] < 0.08).count();
            if quiet >= 18 {
                // A pause: shut soon after the last syllable, open just before the next.
                poses.push(Pose { t: t(prev) + 0.12, open: 0.0, wide: 0.0 });
                poses.push(Pose { t: t(i) - 0.1, open: 0.0, wide: 0.0 });
            } else {
                poses.push(Pose { t: t(j), open: (0.6 * low).min(0.3), wide: 0.5 * poses.last().map_or(0.0, |p| p.wide) });
            }
        } else if t(i) > 0.12 {
            poses.push(Pose { t: t(i) - 0.1, open: 0.0, wide: 0.0 });
        }
        poses.push(Pose { t: t(i), open: (size * (0.45 + 0.55 * sm[i])).clamp(0.2, 1.0), wide });
    }
    // Lips shut on m, b and p as the word starts.
    for (text, ws, _) in words {
        if let Some(open) = closes(text) {
            if *ws > 0.05 {
                poses.push(Pose { t: ws - 0.02, open, wide: 0.0 });
            }
        }
    }
    if let Some(&last) = peaks.last() {
        let end = (n - 1).max(last);
        let close = (last..n).find(|&j| sm[j] < 0.08).unwrap_or(end);
        poses.push(Pose { t: t(close) + 0.08, open: 0.0, wide: 0.0 });
    }
    poses.sort_by(|a, b| a.t.total_cmp(&b.t));
    // Lead the sound by 40 ms; drop poses closer than a frame at 30 fps.
    let mut out: Vec<Pose> = Vec::new();
    for p in poses {
        let p = Pose { t: (p.t - 0.04).max(0.0), ..p };
        match out.last_mut() {
            Some(q) if p.t - q.t < 0.033 => {
                if p.open < 0.05 || p.open > q.open {
                    *q = Pose { t: q.t, ..p };
                }
            }
            _ => out.push(p),
        }
    }
    out
}

/// When to blink over `dur` seconds: about every `every` seconds (4 is
/// natural), irregularly, now and then twice in a row, and with each of
/// `also` (a look away). The same `seed` gives the same blinks.
pub(super) fn blinks(seed: u64, dur: f64, every: f64, also: &[f64]) -> Vec<f64> {
    let mut h = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ 0xd1b5_4a32_d192_ed03;
    let mut next = || {
        h ^= h << 13;
        h ^= h >> 7;
        h ^= h << 17;
        (h >> 11) as f64 / (1u64 << 53) as f64
    };
    let mut out: Vec<f64> = also.iter().copied().filter(|&a| a >= 0.0 && a < dur - BLINK).collect();
    let mut t = 0.4 + 2.0 * next();
    while t < dur - BLINK {
        out.push(t);
        if next() < 0.15 && t + 0.35 < dur - BLINK {
            out.push(t + 0.3);
        }
        t += every / 3.4 * (2.0 + 4.0 * next() * next().max(0.35));
    }
    out.sort_by(f64::total_cmp);
    out.dedup_by(|b, a| *b - *a < 0.28);
    out
}

/// How long a blink lasts, closing to open.
pub(super) const BLINK: f64 = 0.22;

/// How shut the eyes are `t` seconds after a blink starts (0 open, 1
/// shut): quick down, a moment shut, slower up.
pub(super) fn lid(t: f64) -> f64 {
    if !(0.0..BLINK).contains(&t) {
        0.0
    } else if t < 0.07 {
        let k = t / 0.07;
        k * k
    } else if t < 0.1 {
        1.0
    } else {
        let k = (t - 0.1) / 0.12;
        1.0 - k * (2.0 - k)
    }
}

/// Times at which blinks starting at `at` need keys (relative to each).
pub(super) const LID_KEYS: [f64; 5] = [0.0, 0.07, 0.1, 0.16, BLINK];

#[cfg(test)]
mod tests {
    use super::*;

    /// A cartoon face on transparency: a tan head, two eyes (white with
    /// a dark pupil) and a dark mouth line below them.
    fn face(pupils_right: bool) -> Picture {
        let (w, h) = (200, 200);
        let mut px = vec![[0u8; 4]; w * h];
        let mut put = |x: usize, y: usize, c: [u8; 4]| px[y * w + x] = c;
        for y in 20..190 {
            for x in 20..180 {
                if ((x as f64 - 100.0) / 80.0).powi(2) + ((y as f64 - 105.0) / 85.0).powi(2) < 1.0 {
                    put(x, y, [200, 150, 90, 255]);
                }
            }
        }
        for ex in [70.0, 130.0] {
            for y in 60..100 {
                for x in (ex as usize - 20)..(ex as usize + 20) {
                    let (dx, dy) = (x as f64 - ex, y as f64 - 80.0);
                    if (dx / 14.0).powi(2) + (dy / 16.0).powi(2) < 1.0 {
                        put(x, y, [245, 245, 240, 255]);
                    }
                    let px_ = ex + if pupils_right { 5.0 } else { 0.0 };
                    if (x as f64 - px_).powi(2) + dy.powi(2) < 49.0 {
                        put(x, y, [20, 15, 10, 255]);
                    }
                }
            }
        }
        for x in 82..120 {
            let y = 140 + ((x as f64 - 100.0).powi(2) / 120.0) as usize;
            put(x, y, [90, 40, 30, 255]);
            put(x, y + 1, [90, 40, 30, 255]);
        }
        Picture::new(w, h, &px).unwrap()
    }

    #[test]
    fn finds_two_eyes_and_the_mouth_below_them() {
        let f = find(&face(false), None).unwrap();
        assert_eq!(f.eyes.len(), 2, "{f:?}");
        assert!((f.eyes[0].c.0 - 70.0).abs() < 4.0 && (f.eyes[1].c.0 - 130.0).abs() < 4.0, "{f:?}");
        assert!((f.eyes[0].c.1 - 80.0).abs() < 4.0, "{f:?}");
        assert!(f.eyes[0].rx > 9.0 && f.eyes[0].rx < 20.0 && f.eyes[0].ry > 10.0, "{f:?}");
        let m = f.mouth.unwrap();
        assert!(!f.mouth_guessed, "{f:?}");
        assert!((m.c.0 - 100.0).abs() < 8.0 && (m.c.1 - 142.0).abs() < 6.0, "{m:?}");
        assert!(m.w > 20.0 && m.w < 60.0, "{m:?}");
        // Pupils to one side say which way it faces.
        let g = find(&face(true), None).unwrap();
        assert!(g.eyes[0].pupil.0 > g.eyes[0].c.0, "{g:?}");
    }

    #[test]
    fn nothing_dark_and_round_means_no_eyes() {
        let px = vec![[200u8, 150, 90, 255]; 50 * 50];
        let mut px = px;
        for p in px.iter_mut().take(50 * 5) {
            p[3] = 0;
        }
        let f = find(&Picture::new(50, 50, &px).unwrap(), None).unwrap();
        assert!(f.eyes.is_empty() && f.mouth.is_none());
    }

    /// Three syllables "ba-na-na" of a 140 Hz buzz, with a pause.
    fn speech() -> (Vec<f32>, u32) {
        let rate = 16_000;
        let mut s = vec![0.0f32; rate as usize * 2];
        for (from, to) in [(0.2, 0.4), (0.5, 0.7), (0.8, 1.0), (1.6, 1.85)] {
            for i in (from * rate as f64) as usize..(to * rate as f64) as usize {
                let t = i as f64 / rate as f64;
                let env = (std::f64::consts::PI * (t - from) / (to - from)).sin();
                s[i] = (0.4 * env * (std::f64::consts::TAU * 140.0 * t).sin()) as f32;
            }
        }
        (s, rate)
    }

    /// Whether the mouth is shut at `t` (the poses either side of it are).
    fn pose_shut(p: &[Pose], t: f64) -> bool {
        let i = p.iter().position(|q| q.t > t).unwrap_or(p.len());
        i > 0 && i < p.len() && p[i - 1].open == 0.0 && p[i].open == 0.0
    }

    #[test]
    fn the_mouth_opens_on_syllables_and_shuts_in_pauses() {
        let (s, rate) = speech();
        let p = lip_sync(&s, rate, &[]);
        let open_near = |t: f64| p.iter().filter(|q| (q.t - t).abs() < 0.08).map(|q| q.open).fold(0.0, f64::max);
        for t in [0.3, 0.6, 0.9, 1.72] {
            assert!(open_near(t - 0.04) > 0.35, "syllable at {t}: {p:?}");
        }
        assert!(pose_shut(&p, 1.0) && pose_shut(&p, 1.5), "shut in the pause: {p:?}");
        assert_eq!(p.first().unwrap().open, 0.0);
        assert_eq!(p.last().unwrap().open, 0.0);
        assert!(p.windows(2).all(|w| w[1].t > w[0].t));
        // Words shape the vowels: "boo" is round and shuts for the b.
        let w = lip_sync(&s, rate, &[("bee".into(), 0.18, 0.42), ("boo".into(), 0.48, 0.72)]);
        let at = |t: f64| w.iter().filter(|q| (q.t - t).abs() < 0.06).max_by(|a, b| a.open.total_cmp(&b.open)).copied().unwrap();
        assert!(at(0.26).wide > 0.5 && Shape::of(&at(0.26)) == Shape::Wide, "{w:?}");
        assert!(at(0.56).wide < -0.5 && Shape::of(&at(0.56)) == Shape::Round, "{w:?}");
        assert!(w.iter().any(|q| (q.t - 0.42).abs() < 0.03 && q.open == 0.0), "b shuts the lips: {w:?}");
    }

    #[test]
    fn blinks_come_every_few_seconds_and_differ_per_character() {
        let a = blinks(7, 30.0, 4.0, &[]);
        assert!(a.len() >= 5 && a.len() <= 16, "{a:?}");
        assert!(a.windows(2).all(|w| w[1] - w[0] >= 0.28));
        assert!(a.iter().all(|&t| t >= 0.0 && t + BLINK <= 30.0));
        assert_ne!(a, blinks(8, 30.0, 4.0, &[]));
        assert!(blinks(7, 30.0, 4.0, &[12.34]).iter().any(|&t| (t - 12.34).abs() < 0.3));
        assert_eq!(lid(-0.1), 0.0);
        assert_eq!(lid(0.08), 1.0);
        assert_eq!(lid(BLINK), 0.0);
        assert!(lid(0.035) < lid(0.06) && lid(0.15) > lid(0.2));
        assert_eq!(syllables("banana").len(), 3);
        assert_eq!(syllables("make").len(), 1);
        assert_eq!(Shape::named("oo"), Some(Shape::Round));
    }
}
