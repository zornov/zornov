// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! TrueType font reading and glyph rasterising.
//!
//! Reads `.ttf` and `.ttc` files with `glyf` outlines: character mapping
//! (cmap formats 4 and 12), metrics (head, hhea, hmtx), simple and composite
//! glyphs. Outlines are flattened to lines and filled with exact-area coverage
//! accumulation, which gives anti-aliased glyphs without supersampling.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FontError(pub &'static str);

impl std::fmt::Display for FontError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "font: {}", self.0)
    }
}

impl std::error::Error for FontError {}

fn rd16(b: &[u8], o: usize) -> Result<u16, FontError> {
    b.get(o..o + 2).map(|s| u16::from_be_bytes([s[0], s[1]])).ok_or(FontError("truncated"))
}
fn rdi16(b: &[u8], o: usize) -> Result<i16, FontError> {
    rd16(b, o).map(|v| v as i16)
}
fn rd32(b: &[u8], o: usize) -> Result<u32, FontError> {
    b.get(o..o + 4).map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]])).ok_or(FontError("truncated"))
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

/// One closed contour of on/off-curve points in font units.
type Contour = Vec<(Point, bool)>;

pub struct Font {
    data: Vec<u8>,
    loca: usize,
    glyf: usize,
    hmtx: usize,
    long_metrics: u16,
    num_glyphs: u16,
    long_loca: bool,
    cmap: Vec<(u32, u32, u32)>, // (start char, end char, start glyph) groups
    pub units_per_em: f32,
    pub ascender: f32,
    pub descender: f32,
    pub line_gap: f32,
}

impl Font {
    /// Parses a font file; `index` selects a face inside a `.ttc` collection.
    pub fn parse(data: Vec<u8>, index: u32) -> Result<Self, FontError> {
        let mut base = 0usize;
        if data.get(..4) == Some(b"ttcf") {
            let n = rd32(&data, 8)?;
            if index >= n {
                return Err(FontError("collection index out of range"));
            }
            base = rd32(&data, 12 + 4 * index as usize)? as usize;
        }
        let num_tables = rd16(&data, base + 4)? as usize;
        let table = |tag: &[u8; 4]| -> Option<usize> {
            (0..num_tables).find_map(|i| {
                let rec = base + 12 + i * 16;
                (data.get(rec..rec + 4)? == tag).then(|| rd32(&data, rec + 8).ok().map(|o| o as usize)).flatten()
            })
        };
        let need = |o: Option<usize>, name: &'static str| o.ok_or(FontError(name));
        let head = need(table(b"head"), "no head table")?;
        let hhea = need(table(b"hhea"), "no hhea table")?;
        let maxp = need(table(b"maxp"), "no maxp table")?;
        let hmtx = need(table(b"hmtx"), "no hmtx table")?;
        let cmap = need(table(b"cmap"), "no cmap table")?;
        let loca = need(table(b"loca"), "no loca table (CFF fonts are not supported)")?;
        let glyf = need(table(b"glyf"), "no glyf table (CFF fonts are not supported)")?;

        let units_per_em = rd16(&data, head + 18)? as f32;
        let long_loca = rdi16(&data, head + 50)? != 0;
        let num_glyphs = rd16(&data, maxp + 4)?;
        let ascender = rdi16(&data, hhea + 4)? as f32;
        let descender = rdi16(&data, hhea + 6)? as f32;
        let line_gap = rdi16(&data, hhea + 8)? as f32;
        let long_metrics = rd16(&data, hhea + 34)?;
        let groups = parse_cmap(&data, cmap)?;
        Ok(Self { data, loca, glyf, hmtx, long_metrics, num_glyphs, long_loca, cmap: groups, units_per_em, ascender, descender, line_gap })
    }

    pub fn glyph_index(&self, c: char) -> u16 {
        let c = c as u32;
        let i = self.cmap.partition_point(|&(_, end, _)| end < c);
        match self.cmap.get(i) {
            Some(&(start, _, glyph)) if start <= c => (glyph + (c - start)) as u16,
            _ => 0,
        }
    }

    /// Advance width in font units.
    pub fn advance(&self, glyph: u16) -> f32 {
        let i = glyph.min(self.long_metrics.saturating_sub(1)) as usize;
        rd16(&self.data, self.hmtx + i * 4).unwrap_or(0) as f32
    }

    fn glyph_range(&self, glyph: u16) -> Option<(usize, usize)> {
        if glyph >= self.num_glyphs {
            return None;
        }
        let g = glyph as usize;
        let (a, b) = if self.long_loca {
            (rd32(&self.data, self.loca + g * 4).ok()? as usize, rd32(&self.data, self.loca + g * 4 + 4).ok()? as usize)
        } else {
            (rd16(&self.data, self.loca + g * 2).ok()? as usize * 2, rd16(&self.data, self.loca + g * 2 + 2).ok()? as usize * 2)
        };
        (b > a).then_some((self.glyf + a, self.glyf + b))
    }

    /// Outline contours in font units (y up).
    pub fn outline(&self, glyph: u16) -> Vec<Contour> {
        let mut out = Vec::new();
        self.outline_into(glyph, [1.0, 0.0, 0.0, 1.0, 0.0, 0.0], 0, &mut out);
        out
    }

    fn outline_into(&self, glyph: u16, m: [f32; 6], depth: u32, out: &mut Vec<Contour>) {
        let Some((start, end)) = self.glyph_range(glyph) else { return };
        let d = &self.data[..end.min(self.data.len())];
        let Ok(n) = rdi16(d, start) else { return };
        let apply = |x: f32, y: f32| Point { x: m[0] * x + m[2] * y + m[4], y: m[1] * x + m[3] * y + m[5] };
        if n >= 0 {
            let _ = self.simple(d, start, n as usize, &apply, out);
        } else if depth < 8 {
            // Composite glyph: components with offsets and optional scale.
            let mut p = start + 10;
            loop {
                let (Ok(flags), Ok(child)) = (rd16(d, p), rd16(d, p + 2)) else { return };
                p += 4;
                let (dx, dy) = if flags & 1 != 0 {
                    let r = (rdi16(d, p), rdi16(d, p + 2));
                    p += 4;
                    match r {
                        (Ok(a), Ok(b)) => (a as f32, b as f32),
                        _ => return,
                    }
                } else {
                    let r = (d.get(p).map(|&v| v as i8), d.get(p + 1).map(|&v| v as i8));
                    p += 2;
                    match r {
                        (Some(a), Some(b)) => (a as f32, b as f32),
                        _ => return,
                    }
                };
                let f2 = |o: usize| rdi16(d, o).map(|v| v as f32 / 16384.0).unwrap_or(1.0);
                let (mut a, mut b, mut c, mut e) = (1.0, 0.0, 0.0, 1.0);
                if flags & 0x08 != 0 {
                    a = f2(p);
                    e = a;
                    p += 2;
                } else if flags & 0x40 != 0 {
                    a = f2(p);
                    e = f2(p + 2);
                    p += 4;
                } else if flags & 0x80 != 0 {
                    a = f2(p);
                    b = f2(p + 2);
                    c = f2(p + 4);
                    e = f2(p + 6);
                    p += 8;
                }
                // child transform, then parent transform
                let child_m = [a, b, c, e, dx, dy];
                let combined = [
                    m[0] * child_m[0] + m[2] * child_m[1],
                    m[1] * child_m[0] + m[3] * child_m[1],
                    m[0] * child_m[2] + m[2] * child_m[3],
                    m[1] * child_m[2] + m[3] * child_m[3],
                    m[0] * child_m[4] + m[2] * child_m[5] + m[4],
                    m[1] * child_m[4] + m[3] * child_m[5] + m[5],
                ];
                self.outline_into(child, combined, depth + 1, out);
                if flags & 0x20 == 0 {
                    break;
                }
            }
        }
    }

    fn simple(&self, d: &[u8], start: usize, contours: usize, apply: &dyn Fn(f32, f32) -> Point, out: &mut Vec<Contour>) -> Result<(), FontError> {
        let mut ends = Vec::with_capacity(contours);
        for i in 0..contours {
            ends.push(rd16(d, start + 10 + i * 2)? as usize);
        }
        let n_points = ends.last().map_or(0, |e| e + 1);
        let ins_len = rd16(d, start + 10 + contours * 2)? as usize;
        let mut p = start + 12 + contours * 2 + ins_len;
        let mut flags = Vec::with_capacity(n_points);
        while flags.len() < n_points {
            let f = *d.get(p).ok_or(FontError("truncated glyph"))?;
            p += 1;
            flags.push(f);
            if f & 8 != 0 {
                let r = *d.get(p).ok_or(FontError("truncated glyph"))?;
                p += 1;
                for _ in 0..r {
                    flags.push(f);
                }
            }
        }
        let mut coords = |short: u8, same: u8| -> Result<Vec<i32>, FontError> {
            let mut v = 0i32;
            let mut out = Vec::with_capacity(n_points);
            for &f in flags.iter().take(n_points) {
                if f & short != 0 {
                    let b = *d.get(p).ok_or(FontError("truncated glyph"))? as i32;
                    p += 1;
                    v += if f & same != 0 { b } else { -b };
                } else if f & same == 0 {
                    v += rdi16(d, p)? as i32;
                    p += 2;
                }
                out.push(v);
            }
            Ok(out)
        };
        let xs = coords(0x02, 0x10)?;
        let ys = coords(0x04, 0x20)?;
        let mut first = 0;
        for &end in &ends {
            if end < first || end >= n_points {
                return Err(FontError("bad contour"));
            }
            out.push((first..=end).map(|i| (apply(xs[i] as f32, ys[i] as f32), flags[i] & 1 != 0)).collect());
            first = end + 1;
        }
        Ok(())
    }
}

fn parse_cmap(d: &[u8], cmap: usize) -> Result<Vec<(u32, u32, u32)>, FontError> {
    let n = rd16(d, cmap + 2)? as usize;
    let mut best: Option<(u32, usize)> = None; // (score, offset)
    for i in 0..n {
        let rec = cmap + 4 + i * 8;
        let (pid, eid) = (rd16(d, rec)?, rd16(d, rec + 2)?);
        let off = cmap + rd32(d, rec + 4)? as usize;
        let format = rd16(d, off)?;
        let score = match (pid, eid, format) {
            (3, 10, 12) | (0, 4, 12) | (0, 6, 12) => 3,
            (3, 1, 4) | (0, 3, 4) => 2,
            (0, _, 4) => 1,
            _ => 0,
        };
        if score > 0 && best.is_none_or(|(s, _)| score > s) {
            best = Some((score, off));
        }
    }
    let (_, off) = best.ok_or(FontError("no Unicode cmap"))?;
    let mut groups = Vec::new();
    match rd16(d, off)? {
        4 => {
            let segs = rd16(d, off + 6)? as usize / 2;
            let (ends, starts, deltas, ranges) = (off + 14, off + 16 + segs * 2, off + 16 + segs * 4, off + 16 + segs * 6);
            for s in 0..segs {
                let end = rd16(d, ends + s * 2)? as u32;
                let start = rd16(d, starts + s * 2)? as u32;
                let delta = rd16(d, deltas + s * 2)?;
                let range = rd16(d, ranges + s * 2)? as usize;
                if start > end || start == 0xffff {
                    continue;
                }
                for c in start..=end {
                    let g = if range == 0 {
                        (c as u16).wrapping_add(delta)
                    } else {
                        let at = ranges + s * 2 + range + (c - start) as usize * 2;
                        match rd16(d, at)? {
                            0 => 0,
                            g => g.wrapping_add(delta),
                        }
                    };
                    if g != 0 {
                        groups.push((c, c, g as u32));
                    }
                }
            }
        }
        12 => {
            let count = rd32(d, off + 12)? as usize;
            for i in 0..count {
                let g = off + 16 + i * 12;
                groups.push((rd32(d, g)?, rd32(d, g + 4)?, rd32(d, g + 8)?));
            }
        }
        _ => return Err(FontError("unsupported cmap format")),
    }
    groups.sort_by_key(|g| g.0);
    Ok(groups)
}

/// A rasterised glyph: coverage 0..=255, `left`/`top` offsets from the pen
/// position (top measured upward from the baseline).
#[derive(Clone, Debug)]
pub struct Bitmap {
    pub width: usize,
    pub height: usize,
    pub left: i32,
    pub top: i32,
    pub coverage: Vec<u8>,
}

/// Accumulates signed area per pixel; integrating each row gives coverage.
struct Raster {
    w: usize,
    h: usize,
    acc: Vec<f32>,
}

impl Raster {
    fn line(&mut self, p0: Point, p1: Point) {
        if (p0.y - p1.y).abs() < f32::EPSILON {
            return;
        }
        let (dir, p0, p1) = if p0.y < p1.y { (1.0, p0, p1) } else { (-1.0, p1, p0) };
        let dxdy = (p1.x - p0.x) / (p1.y - p0.y);
        let mut x = p0.x;
        let y_start = p0.y.max(0.0);
        if p0.y < 0.0 {
            x -= p0.y * dxdy;
        }
        let y_end = p1.y.min(self.h as f32);
        let mut y = y_start.floor() as usize;
        while (y as f32) < y_end {
            let line_start = y * (self.w + 2);
            let dy = (y as f32 + 1.0).min(p1.y) - (y as f32).max(p0.y);
            let x_next = x + dxdy * dy;
            let d = dy * dir;
            let (x0, x1) = if x < x_next { (x, x_next) } else { (x_next, x) };
            let x0 = x0.clamp(0.0, self.w as f32);
            let x1 = x1.clamp(0.0, self.w as f32);
            let x0_floor = x0.floor();
            let x0i = x0_floor as usize;
            let x1_ceil = x1.ceil();
            let x1i = x1_ceil as usize;
            let acc = &mut self.acc;
            if x1i <= x0i + 1 {
                let xmf = 0.5 * (x0 + x1) - x0_floor;
                acc[line_start + x0i] += d - d * xmf;
                acc[line_start + x0i + 1] += d * xmf;
            } else {
                let s = 1.0 / (x1 - x0);
                let x0f = x0 - x0_floor;
                let a0 = 0.5 * s * (1.0 - x0f) * (1.0 - x0f);
                let x1f = x1 - x1_ceil + 1.0;
                let am = 0.5 * s * x1f * x1f;
                acc[line_start + x0i] += d * a0;
                if x1i == x0i + 2 {
                    acc[line_start + x0i + 1] += d * (1.0 - a0 - am);
                } else {
                    let a1 = s * (1.5 - x0f);
                    acc[line_start + x0i + 1] += d * (a1 - a0);
                    for xi in x0i + 2..x1i - 1 {
                        acc[line_start + xi] += d * s;
                    }
                    let a2 = a1 + (x1i - x0i - 3) as f32 * s;
                    acc[line_start + x1i - 1] += d * (1.0 - a2 - am);
                }
                acc[line_start + x1i] += d * am;
            }
            x = x_next;
            y += 1;
        }
    }

    fn quad(&mut self, p0: Point, c: Point, p1: Point) {
        let dev = ((p0.x - 2.0 * c.x + p1.x).powi(2) + (p0.y - 2.0 * c.y + p1.y).powi(2)).sqrt();
        let n = (1 + (dev * 3.0).sqrt() as usize).min(64);
        let mut prev = p0;
        for i in 1..=n {
            let t = i as f32 / n as f32;
            let mt = 1.0 - t;
            let p = Point { x: mt * mt * p0.x + 2.0 * mt * t * c.x + t * t * p1.x, y: mt * mt * p0.y + 2.0 * mt * t * c.y + t * t * p1.y };
            self.line(prev, p);
            prev = p;
        }
    }

    fn coverage(&self) -> Vec<u8> {
        let mut out = vec![0u8; self.w * self.h];
        for y in 0..self.h {
            let mut sum = 0.0f32;
            for x in 0..self.w {
                sum += self.acc[y * (self.w + 2) + x];
                out[y * self.w + x] = (sum.abs().min(1.0) * 255.0).round() as u8;
            }
        }
        out
    }
}

impl Font {
    /// Rasterises `glyph` at `px` pixels per em.
    pub fn rasterize(&self, glyph: u16, px: f32) -> Bitmap {
        let contours = self.outline(glyph);
        let scale = px / self.units_per_em;
        let pts = contours.iter().flatten().map(|(p, _)| p);
        let (mut xmin, mut ymin, mut xmax, mut ymax) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for p in pts {
            xmin = xmin.min(p.x);
            xmax = xmax.max(p.x);
            ymin = ymin.min(p.y);
            ymax = ymax.max(p.y);
        }
        if contours.is_empty() || xmin > xmax {
            return Bitmap { width: 0, height: 0, left: 0, top: 0, coverage: Vec::new() };
        }
        let left = (xmin * scale).floor() as i32;
        let top = (ymax * scale).ceil() as i32;
        let w = ((xmax * scale).ceil() as i32 - left).max(1) as usize + 1;
        let h = (top - (ymin * scale).floor() as i32).max(1) as usize + 1;
        let mut r = Raster { w, h, acc: vec![0.0; (w + 2) * (h + 1)] };
        let map = |p: Point| Point { x: p.x * scale - left as f32, y: top as f32 - p.y * scale };
        for c in &contours {
            if c.len() < 2 {
                continue;
            }
            // Expand implied on-curve midpoints between consecutive off-curve points.
            let n = c.len();
            let start = c.iter().position(|(_, on)| *on);
            let first = match start {
                Some(i) => c[i].0,
                None => Point { x: (c[0].0.x + c[1].0.x) / 2.0, y: (c[0].0.y + c[1].0.y) / 2.0 },
            };
            let s = start.unwrap_or(0);
            let mut pen = first;
            let mut ctrl: Option<Point> = None;
            for k in 1..=n {
                let (p, on) = c[(s + k) % n];
                if on {
                    match ctrl.take() {
                        Some(cp) => r.quad(map(pen), map(cp), map(p)),
                        None => r.line(map(pen), map(p)),
                    }
                    pen = p;
                } else if let Some(cp) = ctrl {
                    let mid = Point { x: (cp.x + p.x) / 2.0, y: (cp.y + p.y) / 2.0 };
                    r.quad(map(pen), map(cp), map(mid));
                    pen = mid;
                    ctrl = Some(p);
                } else {
                    ctrl = Some(p);
                }
            }
            match ctrl {
                Some(cp) => r.quad(map(pen), map(cp), map(first)),
                None if pen != first => r.line(map(pen), map(first)),
                None => {}
            }
        }
        Bitmap { width: w, height: h, left, top, coverage: r.coverage() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raster_square_has_full_interior_and_half_edges() {
        let mut r = Raster { w: 10, h: 10, acc: vec![0.0; 12 * 11] };
        let sq = [(2.5, 2.0), (7.5, 2.0), (7.5, 8.0), (2.5, 8.0)];
        for i in 0..4 {
            let (a, b) = (sq[i], sq[(i + 1) % 4]);
            r.line(Point { x: a.0, y: a.1 }, Point { x: b.0, y: b.1 });
        }
        let cov = r.coverage();
        assert_eq!(cov[5 * 10 + 5], 255);
        assert_eq!(cov[5 * 10 + 1], 0);
        assert!((120..=135).contains(&cov[5 * 10 + 2]), "{}", cov[5 * 10 + 2]);
        assert_eq!(cov[0], 0);
    }

    #[test]
    fn system_font_glyphs() {
        let Ok(bytes) = std::fs::read("/System/Library/Fonts/Supplemental/Arial Bold.ttf") else { return };
        let f = Font::parse(bytes, 0).unwrap();
        let g = f.glyph_index('A');
        assert_ne!(g, 0);
        assert!(f.advance(g) > 0.0);
        let bmp = f.rasterize(g, 48.0);
        assert!(bmp.width > 20 && bmp.height > 30);
        let ink: u32 = bmp.coverage.iter().map(|&v| v as u32).sum();
        assert!(ink > 255 * 200, "{ink}");
        // 'o' has a hole: the centre is empty while the ring is inked.
        let o = f.rasterize(f.glyph_index('o'), 64.0);
        assert!(o.coverage[(o.height / 2) * o.width + o.width / 2] < 30);
        assert_eq!(f.glyph_index('\u{10FFFF}'), 0);
    }
}
