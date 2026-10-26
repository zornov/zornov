// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A vector critic: how clean vector art is to edit. Read from SVG (what
//! every vector tool writes, Studio's own exports included): how many
//! anchor points it needs, kinks (joins that are almost but not quite
//! smooth, which a designer has to fix by hand), slivers of segments,
//! points that do nothing (a straight line split in two), wiggles (curves
//! that bend one way and back), and, against a reference picture, how
//! faithfully it fills what it should.

use serde::Serialize;

pub type P = [f64; 2];

/// One subpath: cubic segments (lines are cubics with their handles on
/// the line), closed or not.
#[derive(Clone, Debug, Default)]
pub struct Sub {
    pub segs: Vec<[P; 4]>,
    pub closed: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Shape {
    pub subs: Vec<Sub>,
    pub fill: Option<[u8; 3]>,
}

fn num_list(s: &str) -> Vec<f64> {
    let mut out = Vec::new();
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_digit() || c == b'-' || c == b'+' || c == b'.' {
            let start = i;
            i += 1;
            let mut dot = c == b'.';
            while i < b.len() {
                let d = b[i];
                if d.is_ascii_digit() {
                    i += 1;
                } else if d == b'.' && !dot {
                    dot = true;
                    i += 1;
                } else if (d == b'e' || d == b'E') && i + 1 < b.len() {
                    i += 1;
                    if b[i] == b'-' || b[i] == b'+' {
                        i += 1;
                    }
                } else {
                    break;
                }
            }
            if let Ok(v) = s[start..i].parse() {
                out.push(v);
            }
        } else {
            i += 1;
        }
    }
    out
}

fn line(a: P, b: P) -> [P; 4] {
    [a, [a[0] + (b[0] - a[0]) / 3.0, a[1] + (b[1] - a[1]) / 3.0], [a[0] + (b[0] - a[0]) * 2.0 / 3.0, a[1] + (b[1] - a[1]) * 2.0 / 3.0], b]
}

/// SVG path data (M L H V C S Q T Z, absolute and relative; arcs as lines).
pub fn parse_path(d: &str) -> Vec<Sub> {
    let mut subs: Vec<Sub> = Vec::new();
    let mut cur = Sub::default();
    let mut pos: P = [0.0, 0.0];
    let mut start: P = [0.0, 0.0];
    let mut last_ctrl: Option<P> = None;
    // Split into commands with their numbers.
    let mut cmds: Vec<(char, Vec<f64>)> = Vec::new();
    let mut i = 0;
    let chars: Vec<(usize, char)> = d.char_indices().collect();
    while i < chars.len() {
        let (k, c) = chars[i];
        if c.is_ascii_alphabetic() && c != 'e' && c != 'E' {
            let mut j = i + 1;
            while j < chars.len() && !(chars[j].1.is_ascii_alphabetic() && chars[j].1 != 'e' && chars[j].1 != 'E') {
                j += 1;
            }
            let end = if j < chars.len() { chars[j].0 } else { d.len() };
            cmds.push((c, num_list(&d[k + 1..end])));
            i = j;
        } else {
            i += 1;
        }
    }
    let flush = |cur: &mut Sub, subs: &mut Vec<Sub>| {
        if !cur.segs.is_empty() {
            subs.push(std::mem::take(cur));
        } else {
            *cur = Sub::default();
        }
    };
    for (c, nums) in cmds {
        let rel = c.is_ascii_lowercase();
        let off = |p: P, base: P| if rel { [p[0] + base[0], p[1] + base[1]] } else { p };
        match c.to_ascii_uppercase() {
            'M' => {
                flush(&mut cur, &mut subs);
                for (k, xy) in nums.chunks_exact(2).enumerate() {
                    let p = off([xy[0], xy[1]], pos);
                    if k == 0 {
                        start = p;
                    } else {
                        cur.segs.push(line(pos, p));
                    }
                    pos = p;
                }
                last_ctrl = None;
            }
            'L' => {
                for xy in nums.chunks_exact(2) {
                    let p = off([xy[0], xy[1]], pos);
                    cur.segs.push(line(pos, p));
                    pos = p;
                }
                last_ctrl = None;
            }
            'H' => {
                for &x in &nums {
                    let p = [if rel { pos[0] + x } else { x }, pos[1]];
                    cur.segs.push(line(pos, p));
                    pos = p;
                }
                last_ctrl = None;
            }
            'V' => {
                for &y in &nums {
                    let p = [pos[0], if rel { pos[1] + y } else { y }];
                    cur.segs.push(line(pos, p));
                    pos = p;
                }
                last_ctrl = None;
            }
            'C' => {
                for v in nums.chunks_exact(6) {
                    let (c1, c2, p) = (off([v[0], v[1]], pos), off([v[2], v[3]], pos), off([v[4], v[5]], pos));
                    cur.segs.push([pos, c1, c2, p]);
                    last_ctrl = Some(c2);
                    pos = p;
                }
            }
            'S' => {
                for v in nums.chunks_exact(4) {
                    let c1 = last_ctrl.map_or(pos, |c| [2.0 * pos[0] - c[0], 2.0 * pos[1] - c[1]]);
                    let (c2, p) = (off([v[0], v[1]], pos), off([v[2], v[3]], pos));
                    cur.segs.push([pos, c1, c2, p]);
                    last_ctrl = Some(c2);
                    pos = p;
                }
            }
            'Q' | 'T' => {
                let step = if c.to_ascii_uppercase() == 'Q' { 4 } else { 2 };
                for v in nums.chunks_exact(step) {
                    let (q, p) = if step == 4 { (off([v[0], v[1]], pos), off([v[2], v[3]], pos)) } else { (last_ctrl.map_or(pos, |c| [2.0 * pos[0] - c[0], 2.0 * pos[1] - c[1]]), off([v[0], v[1]], pos)) };
                    let c1 = [pos[0] + 2.0 / 3.0 * (q[0] - pos[0]), pos[1] + 2.0 / 3.0 * (q[1] - pos[1])];
                    let c2 = [p[0] + 2.0 / 3.0 * (q[0] - p[0]), p[1] + 2.0 / 3.0 * (q[1] - p[1])];
                    cur.segs.push([pos, c1, c2, p]);
                    last_ctrl = Some(q);
                    pos = p;
                }
            }
            'A' => {
                for v in nums.chunks_exact(7) {
                    let p = off([v[5], v[6]], pos);
                    cur.segs.push(line(pos, p));
                    pos = p;
                }
                last_ctrl = None;
            }
            'Z' => {
                if (pos[0] - start[0]).abs() > 1e-9 || (pos[1] - start[1]).abs() > 1e-9 {
                    cur.segs.push(line(pos, start));
                }
                cur.closed = true;
                pos = start;
                flush(&mut cur, &mut subs);
                last_ctrl = None;
            }
            _ => {}
        }
    }
    flush(&mut cur, &mut subs);
    subs
}

fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let mut from = 0;
    while let Some(k) = tag[from..].find(name) {
        let at = from + k;
        let before_ok = at == 0 || tag.as_bytes()[at - 1].is_ascii_whitespace();
        let rest = tag[at + name.len()..].trim_start();
        if before_ok && let Some(rest) = rest.strip_prefix('=') {
            let rest = rest.trim_start();
            let q = rest.chars().next()?;
            if q == '"' || q == '\'' {
                let body = &rest[1..];
                return body.find(q).map(|e| &body[..e]);
            }
        }
        from = at + name.len();
    }
    None
}

fn colour(s: &str) -> Option<[u8; 3]> {
    let s = s.trim();
    if let Some(h) = s.strip_prefix('#') {
        let v = |i: usize, n: usize| u8::from_str_radix(&h[i..i + n], 16).ok();
        return match h.len() {
            6 => Some([v(0, 2)?, v(2, 2)?, v(4, 2)?]),
            3 => Some([v(0, 1)? * 17, v(1, 1)? * 17, v(2, 1)? * 17]),
            _ => None,
        };
    }
    if let Some(r) = s.strip_prefix("rgb(") {
        let n = num_list(r);
        return (n.len() >= 3).then(|| [n[0] as u8, n[1] as u8, n[2] as u8]);
    }
    match s {
        "none" => None,
        "black" => Some([0, 0, 0]),
        "white" => Some([255, 255, 255]),
        _ => Some([0, 0, 0]),
    }
}

/// Shapes of an SVG document (paths, rects, circles, ellipses, polygons)
/// and its view box (x, y, width, height).
pub fn parse_svg(doc: &str) -> (Vec<Shape>, [f64; 4]) {
    let mut shapes = Vec::new();
    let mut view = [0.0, 0.0, 100.0, 100.0];
    for raw in doc.split('<').skip(1) {
        let tag = raw.split('>').next().unwrap_or("");
        let name = tag.split_whitespace().next().unwrap_or("");
        let fill_of = |t: &str| {
            let style_fill = attr(t, "style").and_then(|s| s.split(';').find_map(|kv| kv.trim().strip_prefix("fill:").map(str::to_owned)));
            match style_fill.as_deref().or(attr(t, "fill")) {
                Some(f) => colour(f),
                None => Some([0, 0, 0]),
            }
        };
        let n = |k: &str| attr(tag, k).and_then(|v| num_list(v).first().copied()).unwrap_or(0.0);
        match name {
            "svg" => {
                if let Some(vb) = attr(tag, "viewBox") {
                    let v = num_list(vb);
                    if v.len() == 4 {
                        view = [v[0], v[1], v[2], v[3]];
                    }
                } else if n("width") > 0.0 {
                    view = [0.0, 0.0, n("width"), n("height")];
                }
            }
            "path" => shapes.push(Shape { subs: parse_path(attr(tag, "d").unwrap_or("")), fill: fill_of(tag) }),
            "rect" => {
                let (x, y, w, h) = (n("x"), n("y"), n("width"), n("height"));
                shapes.push(Shape { subs: parse_path(&format!("M{x} {y}H{}V{}H{x}Z", x + w, y + h)), fill: fill_of(tag) });
            }
            "polygon" | "polyline" => {
                let pts = num_list(attr(tag, "points").unwrap_or(""));
                if pts.len() >= 4 {
                    let mut d = format!("M{} {}", pts[0], pts[1]);
                    for xy in pts[2..].chunks_exact(2) {
                        d.push_str(&format!("L{} {}", xy[0], xy[1]));
                    }
                    if name == "polygon" {
                        d.push('Z');
                    }
                    shapes.push(Shape { subs: parse_path(&d), fill: fill_of(tag) });
                }
            }
            "circle" | "ellipse" => {
                let (cx, cy) = (n("cx"), n("cy"));
                let (rx, ry) = if name == "circle" { (n("r"), n("r")) } else { (n("rx"), n("ry")) };
                let k = 0.552_284_75;
                let d = format!(
                    "M{} {}C{} {} {} {} {} {}C{} {} {} {} {} {}C{} {} {} {} {} {}C{} {} {} {} {} {}Z",
                    cx + rx, cy,
                    cx + rx, cy + ry * k, cx + rx * k, cy + ry, cx, cy + ry,
                    cx - rx * k, cy + ry, cx - rx, cy + ry * k, cx - rx, cy,
                    cx - rx, cy - ry * k, cx - rx * k, cy - ry, cx, cy - ry,
                    cx + rx * k, cy - ry, cx + rx, cy - ry * k, cx + rx, cy
                );
                shapes.push(Shape { subs: parse_path(&d), fill: fill_of(tag) });
            }
            _ => {}
        }
    }
    (shapes, view)
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct VectorQuality {
    pub shapes: usize,
    pub anchors: usize,
    /// Joins bent by 2–15°: meant to be smooth, are not.
    pub kinks: f32,
    /// Anchors that could go (a straight line split, or a curve that
    /// carries on unchanged through them).
    pub redundant: f32,
    /// Segments shorter than 0.3% of the art's size.
    pub tiny: f32,
    /// Curves that bend one way then the other within one segment.
    pub wiggles: f32,
    pub colours: usize,
    /// Against a reference picture: overlap of what is filled (IoU) and
    /// mean colour error (0–1). Unset without one.
    pub fill_iou: Option<f32>,
    pub colour_error: Option<f32>,
}

fn sub2(a: P, b: P) -> P {
    [a[0] - b[0], a[1] - b[1]]
}

fn norm(a: P) -> f64 {
    (a[0] * a[0] + a[1] * a[1]).sqrt()
}

fn out_tangent(s: &[P; 4]) -> P {
    for k in [1, 2, 3] {
        let d = sub2(s[k], s[0]);
        if norm(d) > 1e-9 {
            return d;
        }
    }
    [0.0, 0.0]
}

fn in_tangent(s: &[P; 4]) -> P {
    for k in [2, 1, 0] {
        let d = sub2(s[3], s[k]);
        if norm(d) > 1e-9 {
            return d;
        }
    }
    [0.0, 0.0]
}

fn turn_deg(a: P, b: P) -> f64 {
    let (na, nb) = (norm(a), norm(b));
    if na < 1e-12 || nb < 1e-12 {
        return 0.0;
    }
    ((a[0] * b[0] + a[1] * b[1]) / (na * nb)).clamp(-1.0, 1.0).acos().to_degrees()
}

fn is_line(s: &[P; 4]) -> bool {
    let d = sub2(s[3], s[0]);
    let l = norm(d).max(1e-12);
    let off = |p: P| ((p[0] - s[0][0]) * d[1] - (p[1] - s[0][1]) * d[0]).abs() / l;
    off(s[1]) < l * 1e-3 && off(s[2]) < l * 1e-3
}

fn wiggly(s: &[P; 4]) -> bool {
    if is_line(s) {
        return false;
    }
    // Sign of the curvature (cross of B' and B'') along the segment.
    let mut last = 0.0f64;
    let mut flips = 0;
    for k in 1..16 {
        let t = k as f64 / 16.0;
        let mt = 1.0 - t;
        let d1: P = std::array::from_fn(|i| 3.0 * mt * mt * (s[1][i] - s[0][i]) + 6.0 * mt * t * (s[2][i] - s[1][i]) + 3.0 * t * t * (s[3][i] - s[2][i]));
        let d2: P = std::array::from_fn(|i| 6.0 * mt * (s[2][i] - 2.0 * s[1][i] + s[0][i]) + 6.0 * t * (s[3][i] - 2.0 * s[2][i] + s[1][i]));
        let c = d1[0] * d2[1] - d1[1] * d2[0];
        let mag = norm(d1).powi(3).max(1e-12);
        let k = c / mag;
        if k.abs() > 1e-4 {
            if last != 0.0 && (k > 0.0) != (last > 0.0) {
                flips += 1;
            }
            last = k;
        }
    }
    flips > 0
}

pub fn analyse(shapes: &[Shape], view: [f64; 4]) -> VectorQuality {
    let size = view[2].max(view[3]).max(1e-9);
    let mut q = VectorQuality { shapes: shapes.len(), ..Default::default() };
    let (mut joins, mut kinks, mut redundant, mut tiny, mut wiggles, mut segs) = (0usize, 0usize, 0usize, 0usize, 0usize, 0usize);
    let mut colours = std::collections::HashSet::new();
    for sh in shapes {
        if let Some(c) = sh.fill {
            colours.insert(c);
        }
        for sub in &sh.subs {
            let n = sub.segs.len();
            q.anchors += if sub.closed { n } else { n + 1 };
            for (k, s) in sub.segs.iter().enumerate() {
                segs += 1;
                if norm(sub2(s[3], s[0])) < size * 0.003 {
                    tiny += 1;
                }
                if wiggly(s) {
                    wiggles += 1;
                }
                let next = if k + 1 < n { Some(&sub.segs[k + 1]) } else if sub.closed { sub.segs.first() } else { None };
                let Some(nx) = next else { continue };
                joins += 1;
                let turn = turn_deg(in_tangent(s), out_tangent(nx));
                if turn > 2.0 && turn < 15.0 {
                    kinks += 1;
                }
                if turn < 0.5 && is_line(s) && is_line(nx) {
                    redundant += 1;
                }
            }
        }
    }
    q.kinks = kinks as f32 / joins.max(1) as f32;
    q.redundant = redundant as f32 / joins.max(1) as f32;
    q.tiny = tiny as f32 / segs.max(1) as f32;
    q.wiggles = wiggles as f32 / segs.max(1) as f32;
    q.colours = colours.len();
    q
}

/// The art drawn (non-zero fill, painter's order) at `w × h`, RGBA.
pub fn rasterise(shapes: &[Shape], view: [f64; 4], w: usize, h: usize) -> Vec<u8> {
    let mut out = vec![0u8; w * h * 4];
    let sx = w as f64 / view[2].max(1e-9);
    let sy = h as f64 / view[3].max(1e-9);
    for sh in shapes {
        let Some(c) = sh.fill else { continue };
        let mut edges: Vec<(P, P)> = Vec::new();
        for sub in &sh.subs {
            let mut pts: Vec<P> = Vec::new();
            for s in &sub.segs {
                let steps = if is_line(s) { 1 } else { 12 };
                if pts.is_empty() {
                    pts.push(s[0]);
                }
                for k in 1..=steps {
                    let t = k as f64 / steps as f64;
                    let mt = 1.0 - t;
                    pts.push(std::array::from_fn(|i| mt * mt * mt * s[0][i] + 3.0 * mt * mt * t * s[1][i] + 3.0 * mt * t * t * s[2][i] + t * t * t * s[3][i]));
                }
            }
            for k in 0..pts.len() {
                let a = pts[k];
                let b = pts[(k + 1) % pts.len()];
                edges.push(([(a[0] - view[0]) * sx, (a[1] - view[1]) * sy], [(b[0] - view[0]) * sx, (b[1] - view[1]) * sy]));
            }
        }
        for y in 0..h {
            let fy = y as f64 + 0.5;
            let mut xs: Vec<(f64, i32)> = Vec::new();
            for &(a, b) in &edges {
                if (a[1] <= fy) != (b[1] <= fy) {
                    let t = (fy - a[1]) / (b[1] - a[1]);
                    xs.push((a[0] + t * (b[0] - a[0]), if b[1] > a[1] { 1 } else { -1 }));
                }
            }
            xs.sort_by(|p, q| p.0.total_cmp(&q.0));
            let mut wind = 0;
            for k in 0..xs.len() {
                wind += xs[k].1;
                if wind != 0 && k + 1 < xs.len() {
                    let x0 = (xs[k].0 - 0.5).ceil().max(0.0) as usize;
                    let x1 = ((xs[k + 1].0 - 0.5).ceil().max(0.0) as usize).min(w);
                    for x in x0..x1 {
                        out[(y * w + x) * 4..(y * w + x) * 4 + 4].copy_from_slice(&[c[0], c[1], c[2], 255]);
                    }
                }
            }
        }
    }
    out
}

/// Fill overlap and colour error of the art against a reference picture
/// (straight RGBA, `w × h`; transparent = empty).
pub fn compare(shapes: &[Shape], view: [f64; 4], w: usize, h: usize, reference: &[u8]) -> (f32, f32) {
    let ours = rasterise(shapes, view, w, h);
    let (mut i, mut u, mut err, mut n) = (0usize, 0usize, 0.0f64, 0usize);
    for k in 0..w * h {
        let a = ours[k * 4 + 3] > 127;
        let b = reference[k * 4 + 3] > 127;
        if a && b {
            i += 1;
        }
        if a || b {
            u += 1;
        }
        if b {
            n += 1;
            err += (0..3).map(|c| (ours[k * 4 + c] as f64 - reference[k * 4 + c] as f64).abs()).sum::<f64>() / (3.0 * 255.0);
        }
    }
    (if u == 0 { 1.0 } else { i as f32 / u as f32 }, (err / n.max(1) as f64) as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_judge() {
        let svg = r##"<svg viewBox="0 0 100 100"><path d="M10 10 L50 10 L90 10 L90 90 L10 90 Z" fill="#ff0000"/><circle cx="50" cy="50" r="20" fill="#00f"/></svg>"##;
        let (shapes, view) = parse_svg(svg);
        assert_eq!(shapes.len(), 2);
        let q = analyse(&shapes, view);
        // One straight line split in two.
        assert!(q.redundant > 0.0);
        assert_eq!(q.colours, 2);
        let px = rasterise(&shapes, view, 100, 100);
        assert_eq!(&px[(50 * 100 + 50) * 4..(50 * 100 + 50) * 4 + 3], &[0, 0, 255]);
        assert_eq!(&px[(15 * 100 + 15) * 4..(15 * 100 + 15) * 4 + 3], &[255, 0, 0]);
        let (iou, err) = compare(&shapes, view, 100, 100, &px);
        assert!(iou > 0.999 && err < 1e-6);
    }
}
