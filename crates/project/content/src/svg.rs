// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A small SVG rasteriser for library thumbnails.
//!
//! Covers what emoji and icon sets use: paths with every command (arcs
//! included), basic shapes, groups, transforms, fill rules, opacity, and
//! strokes with round joins and caps. Gradients are flattened to their
//! average colour; filters, masks, clip paths, text and `<use>` are
//! ignored. Anti-aliasing comes from four vertical sub-scanlines with exact
//! horizontal coverage, which is plenty at thumbnail sizes.

use studio_media::rgba::RgbaImage;
use std::collections::HashMap;

/// Rendering options.
#[derive(Clone, Copy, Debug)]
pub struct RenderOptions {
    /// Colour for `currentColor` (icon sets rely on it).
    pub current_color: [u8; 3],
}

impl Default for RenderOptions {
    fn default() -> Self {
        RenderOptions { current_color: [32, 32, 36] }
    }
}

// ---------------------------------------------------------------- XML

#[derive(Debug, Default)]
struct Element {
    name: String,
    attrs: Vec<(String, String)>,
    children: Vec<Element>,
}

impl Element {
    fn attr(&self, name: &str) -> Option<&str> {
        self.attrs.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }
}

fn unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        if let Some(end) = rest.find(';') {
            let ent = &rest[1..end];
            let rep = match ent {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                _ if ent.starts_with("#x") => u32::from_str_radix(&ent[2..], 16).ok().and_then(char::from_u32),
                _ if ent.starts_with('#') => ent[1..].parse::<u32>().ok().and_then(char::from_u32),
                _ => None,
            };
            match rep {
                Some(c) => out.push(c),
                None => out.push_str(&rest[..=end]),
            }
            rest = &rest[end + 1..];
        } else {
            out.push_str(rest);
            rest = "";
        }
    }
    out.push_str(rest);
    out
}

/// Parses the document into an element tree (text nodes are dropped).
fn parse_xml(src: &str) -> Result<Element, String> {
    let b = src.as_bytes();
    let mut i = 0usize;
    let mut stack: Vec<Element> = vec![Element { name: "#root".into(), ..Default::default() }];
    while i < b.len() {
        let Some(lt) = src[i..].find('<') else { break };
        i += lt;
        if src[i..].starts_with("<!--") {
            i = src[i..].find("-->").map(|e| i + e + 3).ok_or("unterminated comment")?;
            continue;
        }
        if src[i..].starts_with("<![CDATA[") {
            i = src[i..].find("]]>").map(|e| i + e + 3).ok_or("unterminated CDATA")?;
            continue;
        }
        if src[i..].starts_with("<?") || src[i..].starts_with("<!") {
            i = src[i..].find('>').map(|e| i + e + 1).ok_or("unterminated declaration")?;
            continue;
        }
        if src[i..].starts_with("</") {
            let end = src[i..].find('>').map(|e| i + e).ok_or("unterminated close tag")?;
            let name = src[i + 2..end].trim();
            let el = stack.pop().ok_or("stray close tag")?;
            if el.name != name {
                return Err(format!("mismatched close tag </{name}> for <{}>", el.name));
            }
            stack.last_mut().ok_or("stray close tag")?.children.push(el);
            i = end + 1;
            continue;
        }
        // Start tag: find its end, honouring quotes.
        let mut j = i + 1;
        let mut quote: Option<u8> = None;
        while j < b.len() {
            match (quote, b[j]) {
                (Some(q), c) if c == q => quote = None,
                (None, b'"') | (None, b'\'') => quote = Some(b[j]),
                (None, b'>') => break,
                _ => {}
            }
            j += 1;
        }
        if j >= b.len() {
            return Err("unterminated start tag".into());
        }
        let self_closing = b[j - 1] == b'/';
        let inner = &src[i + 1..if self_closing { j - 1 } else { j }];
        let name_end = inner.find(|c: char| c.is_whitespace() || c == '/').unwrap_or(inner.len());
        let mut el = Element { name: inner[..name_end].to_string(), ..Default::default() };
        let mut rest = inner[name_end..].trim_start();
        while !rest.is_empty() {
            let Some(eq) = rest.find('=') else { break };
            let key = rest[..eq].trim().to_string();
            let after = rest[eq + 1..].trim_start();
            let Some(q) = after.chars().next() else { break };
            if q != '"' && q != '\'' {
                break;
            }
            let Some(close) = after[1..].find(q) else { break };
            let val = unescape(&after[1..1 + close]);
            el.attrs.push((key, val));
            rest = after[2 + close..].trim_start();
        }
        if self_closing {
            stack.last_mut().unwrap().children.push(el);
        } else {
            stack.push(el);
        }
        i = j + 1;
    }
    while stack.len() > 1 {
        let el = stack.pop().unwrap();
        stack.last_mut().unwrap().children.push(el);
    }
    let root = stack.pop().unwrap();
    root.children.into_iter().find(|e| e.name == "svg").ok_or_else(|| "no <svg> element".to_string())
}

// ------------------------------------------------------------ geometry

#[derive(Clone, Copy, Debug, PartialEq)]
struct Mat {
    a: f32,
    b: f32,
    c: f32,
    d: f32,
    e: f32,
    f: f32,
}

impl Mat {
    const IDENTITY: Mat = Mat { a: 1.0, b: 0.0, c: 0.0, d: 1.0, e: 0.0, f: 0.0 };

    fn mul(self, o: Mat) -> Mat {
        // self × o: apply o first, then self.
        Mat {
            a: self.a * o.a + self.c * o.b,
            b: self.b * o.a + self.d * o.b,
            c: self.a * o.c + self.c * o.d,
            d: self.b * o.c + self.d * o.d,
            e: self.a * o.e + self.c * o.f + self.e,
            f: self.b * o.e + self.d * o.f + self.f,
        }
    }

    fn apply(&self, x: f32, y: f32) -> (f32, f32) {
        (self.a * x + self.c * y + self.e, self.b * x + self.d * y + self.f)
    }

    /// Mean scale factor, used to pick flattening steps and stroke widths.
    fn scale(&self) -> f32 {
        ((self.a * self.d - self.b * self.c).abs()).sqrt().max(1e-6)
    }
}

fn parse_transform(s: &str) -> Mat {
    let mut m = Mat::IDENTITY;
    let mut rest = s.trim();
    while let Some(open) = rest.find('(') {
        let name = rest[..open].trim().trim_start_matches(',').trim();
        let Some(close) = rest[open..].find(')') else { break };
        let nums = parse_numbers(&rest[open + 1..open + close]);
        let t = match (name, nums.as_slice()) {
            ("matrix", [a, b, c, d, e, f]) => Mat { a: *a, b: *b, c: *c, d: *d, e: *e, f: *f },
            ("translate", [x]) => Mat { e: *x, ..Mat::IDENTITY },
            ("translate", [x, y]) => Mat { e: *x, f: *y, ..Mat::IDENTITY },
            ("scale", [s]) => Mat { a: *s, d: *s, ..Mat::IDENTITY },
            ("scale", [sx, sy]) => Mat { a: *sx, d: *sy, ..Mat::IDENTITY },
            ("rotate", [deg]) => rotation(*deg),
            ("rotate", [deg, cx, cy]) => Mat { e: *cx, f: *cy, ..Mat::IDENTITY }.mul(rotation(*deg)).mul(Mat { e: -cx, f: -cy, ..Mat::IDENTITY }),
            ("skewX", [deg]) => Mat { c: deg.to_radians().tan(), ..Mat::IDENTITY },
            ("skewY", [deg]) => Mat { b: deg.to_radians().tan(), ..Mat::IDENTITY },
            _ => Mat::IDENTITY,
        };
        m = m.mul(t);
        rest = &rest[open + close + 1..];
    }
    m
}

fn rotation(deg: f32) -> Mat {
    let (s, c) = deg.to_radians().sin_cos();
    Mat { a: c, b: s, c: -s, d: c, e: 0.0, f: 0.0 }
}

/// Numbers separated by whitespace, commas, or sign changes ("1-2" → 1, −2;
/// ".5.5" → 0.5, 0.5).
fn parse_numbers(s: &str) -> Vec<f32> {
    let mut out = Vec::new();
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_digit() || c == b'-' || c == b'+' || c == b'.' {
            let start = i;
            i += 1;
            let mut seen_dot = b[start] == b'.';
            let mut seen_exp = false;
            while i < b.len() {
                let d = b[i];
                if d.is_ascii_digit() {
                    i += 1;
                } else if d == b'.' && !seen_dot && !seen_exp {
                    seen_dot = true;
                    i += 1;
                } else if (d == b'e' || d == b'E') && !seen_exp && i + 1 < b.len() && (b[i + 1].is_ascii_digit() || b[i + 1] == b'-' || b[i + 1] == b'+') {
                    seen_exp = true;
                    i += 2;
                } else {
                    break;
                }
            }
            if let Ok(v) = s[start..i].parse::<f32>() {
                out.push(v);
            }
        } else {
            i += 1;
        }
    }
    out
}

/// A flattened sub-path in device space.
#[derive(Clone, Debug, Default)]
struct SubPath {
    points: Vec<(f32, f32)>,
    closed: bool,
}

struct Flattener<'a> {
    m: &'a Mat,
    subs: Vec<SubPath>,
    tol: f32,
}

impl Flattener<'_> {
    fn move_to(&mut self, x: f32, y: f32) {
        self.subs.push(SubPath { points: vec![self.m.apply(x, y)], closed: false });
    }

    fn line_to(&mut self, x: f32, y: f32) {
        let p = self.m.apply(x, y);
        match self.subs.last_mut() {
            Some(s) => s.points.push(p),
            None => self.subs.push(SubPath { points: vec![p], closed: false }),
        }
    }

    fn cubic_to(&mut self, (x1, y1): (f32, f32), (x2, y2): (f32, f32), (x, y): (f32, f32)) {
        let p0 = self.subs.last().and_then(|s| s.points.last().copied()).unwrap_or(self.m.apply(x1, y1));
        let p1 = self.m.apply(x1, y1);
        let p2 = self.m.apply(x2, y2);
        let p3 = self.m.apply(x, y);
        let len = dist(p0, p1) + dist(p1, p2) + dist(p2, p3);
        let n = ((len / self.tol).ceil() as usize).clamp(1, 64);
        for k in 1..=n {
            let t = k as f32 / n as f32;
            let u = 1.0 - t;
            let px = u * u * u * p0.0 + 3.0 * u * u * t * p1.0 + 3.0 * u * t * t * p2.0 + t * t * t * p3.0;
            let py = u * u * u * p0.1 + 3.0 * u * u * t * p1.1 + 3.0 * u * t * t * p2.1 + t * t * t * p3.1;
            self.subs.last_mut().unwrap().points.push((px, py));
        }
    }

    fn quad_to(&mut self, (x1, y1): (f32, f32), (x, y): (f32, f32), from: (f32, f32)) {
        // Elevate to a cubic (in user space).
        let c1 = (from.0 + 2.0 / 3.0 * (x1 - from.0), from.1 + 2.0 / 3.0 * (y1 - from.1));
        let c2 = (x + 2.0 / 3.0 * (x1 - x), y + 2.0 / 3.0 * (y1 - y));
        self.cubic_to(c1, c2, (x, y));
    }

    fn close(&mut self) {
        if let Some(s) = self.subs.last_mut() {
            s.closed = true;
        }
    }

    /// SVG arc (user space) via centre parameterisation, split into cubics.
    #[allow(clippy::too_many_arguments)]
    fn arc_to(&mut self, from: (f32, f32), rx: f32, ry: f32, rot_deg: f32, large: bool, sweep: bool, to: (f32, f32)) {
        let (mut rx, mut ry) = (rx.abs(), ry.abs());
        if rx < 1e-6 || ry < 1e-6 || (from.0 - to.0).abs() < 1e-6 && (from.1 - to.1).abs() < 1e-6 {
            self.line_to(to.0, to.1);
            return;
        }
        let (sin_phi, cos_phi) = rot_deg.to_radians().sin_cos();
        let dx = (from.0 - to.0) / 2.0;
        let dy = (from.1 - to.1) / 2.0;
        let x1p = cos_phi * dx + sin_phi * dy;
        let y1p = -sin_phi * dx + cos_phi * dy;
        let lambda = x1p * x1p / (rx * rx) + y1p * y1p / (ry * ry);
        if lambda > 1.0 {
            rx *= lambda.sqrt();
            ry *= lambda.sqrt();
        }
        let num = (rx * rx * ry * ry - rx * rx * y1p * y1p - ry * ry * x1p * x1p).max(0.0);
        let den = rx * rx * y1p * y1p + ry * ry * x1p * x1p;
        let mut coef = if den.abs() < 1e-12 { 0.0 } else { (num / den).sqrt() };
        if large == sweep {
            coef = -coef;
        }
        let cxp = coef * rx * y1p / ry;
        let cyp = -coef * ry * x1p / rx;
        let cx = cos_phi * cxp - sin_phi * cyp + (from.0 + to.0) / 2.0;
        let cy = sin_phi * cxp + cos_phi * cyp + (from.1 + to.1) / 2.0;
        let angle = |ux: f32, uy: f32, vx: f32, vy: f32| -> f32 {
            let dot = ux * vx + uy * vy;
            let len = (ux * ux + uy * uy).sqrt() * (vx * vx + vy * vy).sqrt();
            let mut a = (dot / len.max(1e-12)).clamp(-1.0, 1.0).acos();
            if ux * vy - uy * vx < 0.0 {
                a = -a;
            }
            a
        };
        let theta1 = angle(1.0, 0.0, (x1p - cxp) / rx, (y1p - cyp) / ry);
        let mut dtheta = angle((x1p - cxp) / rx, (y1p - cyp) / ry, (-x1p - cxp) / rx, (-y1p - cyp) / ry);
        if large && dtheta.abs() < 1e-3 {
            // Endpoints (almost) coincide with the large-arc flag set: a
            // full circle, which the sign of the noise must not decide.
            dtheta = if sweep { std::f32::consts::TAU } else { -std::f32::consts::TAU };
        } else if !sweep && dtheta > 0.0 {
            dtheta -= 2.0 * std::f32::consts::PI;
        } else if sweep && dtheta < 0.0 {
            dtheta += 2.0 * std::f32::consts::PI;
        }
        let segments = ((dtheta.abs() / (std::f32::consts::FRAC_PI_2)).ceil() as usize).max(1);
        let delta = dtheta / segments as f32;
        let t = 4.0 / 3.0 * (delta / 4.0).tan();
        let mut th = theta1;
        let point = |th: f32| -> (f32, f32) {
            let (s, c) = th.sin_cos();
            (cx + rx * c * cos_phi - ry * s * sin_phi, cy + rx * c * sin_phi + ry * s * cos_phi)
        };
        let deriv = |th: f32| -> (f32, f32) {
            let (s, c) = th.sin_cos();
            (-rx * s * cos_phi - ry * c * sin_phi, -rx * s * sin_phi + ry * c * cos_phi)
        };
        for _ in 0..segments {
            let th2 = th + delta;
            let p0 = point(th);
            let p3 = point(th2);
            let d0 = deriv(th);
            let d3 = deriv(th2);
            let c1 = (p0.0 + t * d0.0, p0.1 + t * d0.1);
            let c2 = (p3.0 - t * d3.0, p3.1 - t * d3.1);
            self.cubic_to(c1, c2, p3);
            th = th2;
        }
    }
}

fn dist(a: (f32, f32), b: (f32, f32)) -> f32 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}

/// Flattens path data to device-space polylines.
fn flatten_path(d: &str, m: &Mat, tol: f32) -> Vec<SubPath> {
    let mut f = Flattener { m, subs: Vec::new(), tol };
    let mut cmd = b'M';
    let mut cur = (0f32, 0f32);
    let mut start = (0f32, 0f32);
    let mut last_ctrl: Option<(f32, f32)> = None;
    let mut last_cmd = b'M';
    let b = d.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_alphabetic() {
            cmd = c;
            i += 1;
        } else if c.is_ascii_whitespace() || c == b',' {
            i += 1;
            continue;
        }
        // Read the numbers for one application of `cmd`.
        let need = match cmd.to_ascii_uppercase() {
            b'M' | b'L' | b'T' => 2,
            b'H' | b'V' => 1,
            b'C' => 6,
            b'S' | b'Q' => 4,
            b'A' => 7,
            b'Z' => 0,
            _ => return f.subs,
        };
        let mut nums = Vec::with_capacity(need);
        let mut j = i;
        while nums.len() < need && j < b.len() {
            while j < b.len() && (b[j].is_ascii_whitespace() || b[j] == b',') {
                j += 1;
            }
            let s = j;
            if j < b.len() && (b[j] == b'-' || b[j] == b'+') {
                j += 1;
            }
            // Flags of an arc are single digits that may be glued together.
            let is_flag = cmd.to_ascii_uppercase() == b'A' && (nums.len() == 3 || nums.len() == 4);
            if is_flag {
                if j < b.len() && (b[j] == b'0' || b[j] == b'1') {
                    nums.push((b[j] - b'0') as f32);
                    j += 1;
                    continue;
                }
                break;
            }
            let mut dot = false;
            let mut exp = false;
            while j < b.len() {
                let ch = b[j];
                if ch.is_ascii_digit() {
                    j += 1;
                } else if ch == b'.' && !dot && !exp {
                    dot = true;
                    j += 1;
                } else if (ch == b'e' || ch == b'E') && !exp {
                    exp = true;
                    j += 1;
                    if j < b.len() && (b[j] == b'-' || b[j] == b'+') {
                        j += 1;
                    }
                } else {
                    break;
                }
            }
            match d[s..j].parse::<f32>() {
                Ok(v) => nums.push(v),
                Err(_) => break,
            }
        }
        if nums.len() < need {
            break;
        }
        i = j;
        let rel = cmd.is_ascii_lowercase();
        let abs = |v: (f32, f32), cur: (f32, f32)| if rel { (v.0 + cur.0, v.1 + cur.1) } else { v };
        match cmd.to_ascii_uppercase() {
            b'M' => {
                cur = abs((nums[0], nums[1]), cur);
                start = cur;
                f.move_to(cur.0, cur.1);
                // Subsequent pairs are implicit line-tos.
                cmd = if rel { b'l' } else { b'L' };
                last_ctrl = None;
            }
            b'L' => {
                cur = abs((nums[0], nums[1]), cur);
                f.line_to(cur.0, cur.1);
                last_ctrl = None;
            }
            b'H' => {
                cur = (if rel { cur.0 + nums[0] } else { nums[0] }, cur.1);
                f.line_to(cur.0, cur.1);
                last_ctrl = None;
            }
            b'V' => {
                cur = (cur.0, if rel { cur.1 + nums[0] } else { nums[0] });
                f.line_to(cur.0, cur.1);
                last_ctrl = None;
            }
            b'C' => {
                let c1 = abs((nums[0], nums[1]), cur);
                let c2 = abs((nums[2], nums[3]), cur);
                let to = abs((nums[4], nums[5]), cur);
                f.cubic_to(c1, c2, to);
                last_ctrl = Some(c2);
                cur = to;
            }
            b'S' => {
                let c1 = match (last_cmd.to_ascii_uppercase(), last_ctrl) {
                    (b'C' | b'S', Some(lc)) => (2.0 * cur.0 - lc.0, 2.0 * cur.1 - lc.1),
                    _ => cur,
                };
                let c2 = abs((nums[0], nums[1]), cur);
                let to = abs((nums[2], nums[3]), cur);
                f.cubic_to(c1, c2, to);
                last_ctrl = Some(c2);
                cur = to;
            }
            b'Q' => {
                let c1 = abs((nums[0], nums[1]), cur);
                let to = abs((nums[2], nums[3]), cur);
                f.quad_to(c1, to, cur);
                last_ctrl = Some(c1);
                cur = to;
            }
            b'T' => {
                let c1 = match (last_cmd.to_ascii_uppercase(), last_ctrl) {
                    (b'Q' | b'T', Some(lc)) => (2.0 * cur.0 - lc.0, 2.0 * cur.1 - lc.1),
                    _ => cur,
                };
                let to = abs((nums[0], nums[1]), cur);
                f.quad_to(c1, to, cur);
                last_ctrl = Some(c1);
                cur = to;
            }
            b'A' => {
                let to = abs((nums[5], nums[6]), cur);
                f.arc_to(cur, nums[0], nums[1], nums[2], nums[3] != 0.0, nums[4] != 0.0, to);
                cur = to;
                last_ctrl = None;
            }
            b'Z' => {
                f.close();
                cur = start;
                f.move_to(cur.0, cur.1);
                last_ctrl = None;
            }
            _ => {}
        }
        last_cmd = cmd;
    }
    f.subs.retain(|s| s.points.len() > 1 || s.closed);
    f.subs
}

// ------------------------------------------------------------ raster

#[derive(Clone, Copy, PartialEq)]
enum FillRule {
    NonZero,
    EvenOdd,
}

struct Edge {
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    dir: i32,
}

/// Coverage (0..1) per pixel of the polygons under `rule`.
fn coverage(subs: &[SubPath], rule: FillRule, w: usize, h: usize) -> Vec<f32> {
    const SS: usize = 4;
    let mut edges: Vec<Edge> = Vec::new();
    for s in subs {
        let n = s.points.len();
        if n < 2 {
            continue;
        }
        for k in 0..n {
            let (a, b) = (s.points[k], s.points[(k + 1) % n]);
            if k + 1 == n && !s.closed && (a.0 - b.0).abs() < 1e-6 && (a.1 - b.1).abs() < 1e-6 {
                continue;
            }
            if (a.1 - b.1).abs() < 1e-9 {
                continue;
            }
            let (x0, y0, x1, y1, dir) = if a.1 < b.1 { (a.0, a.1, b.0, b.1, 1) } else { (b.0, b.1, a.0, a.1, -1) };
            edges.push(Edge { x0, y0, x1, y1, dir });
        }
    }
    let mut cov = vec![0f32; w * h];
    if edges.is_empty() {
        return cov;
    }
    edges.sort_by(|a, b| a.y0.partial_cmp(&b.y0).unwrap_or(std::cmp::Ordering::Equal));
    let y_min = edges.iter().map(|e| e.y0).fold(f32::MAX, f32::min).max(0.0);
    let y_max = edges.iter().map(|e| e.y1).fold(f32::MIN, f32::max).min(h as f32);
    let mut next_edge = 0usize;
    let mut active: Vec<usize> = Vec::new();
    let mut xs: Vec<(f32, i32)> = Vec::new();
    let weight = 1.0 / SS as f32;
    let row_start = (y_min.floor().max(0.0)) as usize;
    let row_end = (y_max.ceil().min(h as f32)) as usize;
    for py in row_start..row_end {
        let row = &mut cov[py * w..(py + 1) * w];
        for sub in 0..SS {
            let ys = py as f32 + (sub as f32 + 0.5) * weight;
            while next_edge < edges.len() && edges[next_edge].y0 <= ys {
                active.push(next_edge);
                next_edge += 1;
            }
            active.retain(|&k| edges[k].y1 > ys);
            xs.clear();
            for &k in &active {
                let e = &edges[k];
                if e.y0 <= ys {
                    let x = e.x0 + (ys - e.y0) * (e.x1 - e.x0) / (e.y1 - e.y0);
                    xs.push((x, e.dir));
                }
            }
            if xs.len() < 2 {
                continue;
            }
            xs.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
            let mut wind = 0i32;
            for k in 0..xs.len() - 1 {
                wind += xs[k].1;
                let inside = match rule {
                    FillRule::NonZero => wind != 0,
                    FillRule::EvenOdd => wind % 2 != 0,
                };
                if !inside {
                    continue;
                }
                let xa = xs[k].0.max(0.0);
                let xb = xs[k + 1].0.min(w as f32);
                if xb <= xa {
                    continue;
                }
                let first = xa.floor() as usize;
                let last = (xb.ceil() as usize).min(w);
                for px in first..last {
                    let l = xa.max(px as f32);
                    let r = xb.min(px as f32 + 1.0);
                    if r > l {
                        row[px] += (r - l) * weight;
                    }
                }
            }
        }
    }
    cov
}

/// Polygons covering a stroke of `width` with round joins and the given cap.
fn stroke_polygons(subs: &[SubPath], width: f32, cap: &str) -> Vec<SubPath> {
    let hw = width / 2.0;
    let mut out = Vec::new();
    let circle = |c: (f32, f32)| -> SubPath {
        let n = ((hw * 2.0).ceil() as usize).clamp(8, 48);
        SubPath {
            points: (0..n)
                .map(|k| {
                    let t = k as f32 / n as f32 * std::f32::consts::TAU;
                    (c.0 + hw * t.cos(), c.1 + hw * t.sin())
                })
                .collect(),
            closed: true,
        }
    };
    for s in subs {
        let mut pts: Vec<(f32, f32)> = Vec::with_capacity(s.points.len() + 1);
        for &p in &s.points {
            if pts.last().is_none_or(|&q| dist(q, p) > 1e-4) {
                pts.push(p);
            }
        }
        if s.closed && pts.len() > 2 {
            pts.push(pts[0]);
        }
        if pts.len() == 1 {
            if cap != "butt" {
                out.push(circle(pts[0]));
            }
            continue;
        }
        for k in 0..pts.len() - 1 {
            let (a, b) = (pts[k], pts[k + 1]);
            let len = dist(a, b).max(1e-6);
            let (nx, ny) = (-(b.1 - a.1) / len * hw, (b.0 - a.0) / len * hw);
            let (mut a2, mut b2) = (a, b);
            if cap == "square" && !s.closed {
                let (ux, uy) = ((b.0 - a.0) / len * hw, (b.1 - a.1) / len * hw);
                if k == 0 {
                    a2 = (a.0 - ux, a.1 - uy);
                }
                if k + 2 == pts.len() {
                    b2 = (b.0 + ux, b.1 + uy);
                }
            }
            let quad = vec![(a2.0 + nx, a2.1 + ny), (b2.0 + nx, b2.1 + ny), (b2.0 - nx, b2.1 - ny), (a2.0 - nx, a2.1 - ny)];
            out.push(SubPath { points: orient(quad), closed: true });
            // Round joins at interior vertices.
            if k + 2 < pts.len() || s.closed {
                out.push(circle(b));
            }
        }
        if !s.closed && cap == "round" {
            out.push(circle(pts[0]));
            out.push(circle(*pts.last().unwrap()));
        }
    }
    out
}

/// Makes a polygon counter-clockwise so that unions under the non-zero rule
/// never cancel out.
fn orient(mut p: Vec<(f32, f32)>) -> Vec<(f32, f32)> {
    let mut area = 0f32;
    for k in 0..p.len() {
        let (a, b) = (p[k], p[(k + 1) % p.len()]);
        area += a.0 * b.1 - b.0 * a.1;
    }
    if area < 0.0 {
        p.reverse();
    }
    p
}

// --------------------------------------------------------------- paint

#[derive(Clone, Copy, Debug, PartialEq)]
enum Paint {
    None,
    Color([u8; 3], f32),
    Current,
}

fn named_color(name: &str) -> Option<[u8; 3]> {
    Some(match name {
        "black" => [0, 0, 0],
        "white" => [255, 255, 255],
        "red" => [255, 0, 0],
        "lime" => [0, 255, 0],
        "green" => [0, 128, 0],
        "blue" => [0, 0, 255],
        "yellow" => [255, 255, 0],
        "cyan" | "aqua" => [0, 255, 255],
        "magenta" | "fuchsia" => [255, 0, 255],
        "gray" | "grey" => [128, 128, 128],
        "silver" => [192, 192, 192],
        "orange" => [255, 165, 0],
        "purple" => [128, 0, 128],
        "pink" => [255, 192, 203],
        "brown" => [165, 42, 42],
        "navy" => [0, 0, 128],
        "teal" => [0, 128, 128],
        "maroon" => [128, 0, 0],
        "olive" => [128, 128, 0],
        "gold" => [255, 215, 0],
        "darkgray" | "darkgrey" => [169, 169, 169],
        "lightgray" | "lightgrey" => [211, 211, 211],
        _ => return None,
    })
}

fn parse_paint(s: &str, gradients: &HashMap<String, ([u8; 3], f32)>) -> Option<Paint> {
    let s = s.trim();
    if s.is_empty() || s == "inherit" {
        return None;
    }
    if s == "none" || s == "transparent" {
        return Some(Paint::None);
    }
    if s == "currentColor" {
        return Some(Paint::Current);
    }
    if let Some(rest) = s.strip_prefix("url(") {
        let id = rest.trim_end_matches(')').trim().trim_start_matches('#').trim_matches(|c| c == '"' || c == '\'');
        return Some(match gradients.get(id) {
            Some((c, a)) => Paint::Color(*c, *a),
            None => Paint::Color([128, 128, 128], 1.0),
        });
    }
    parse_color(s).map(|(c, a)| Paint::Color(c, a))
}

fn parse_color(s: &str) -> Option<([u8; 3], f32)> {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix('#') {
        let v: Vec<u8> = hex.chars().filter_map(|c| c.to_digit(16).map(|d| d as u8)).collect();
        return match v.len() {
            3 => Some(([v[0] * 17, v[1] * 17, v[2] * 17], 1.0)),
            4 => Some(([v[0] * 17, v[1] * 17, v[2] * 17], v[3] as f32 * 17.0 / 255.0)),
            6 => Some(([v[0] * 16 + v[1], v[2] * 16 + v[3], v[4] * 16 + v[5]], 1.0)),
            8 => Some(([v[0] * 16 + v[1], v[2] * 16 + v[3], v[4] * 16 + v[5]], (v[6] * 16 + v[7]) as f32 / 255.0)),
            _ => None,
        };
    }
    if s.starts_with("rgb") {
        let inner = &s[s.find('(')? + 1..s.rfind(')')?];
        let parts: Vec<&str> = inner.split(|c| c == ',' || c == ' ' || c == '/').filter(|p| !p.is_empty()).collect();
        if parts.len() < 3 {
            return None;
        }
        let chan = |p: &str| -> Option<u8> {
            if let Some(pc) = p.strip_suffix('%') {
                Some((pc.trim().parse::<f32>().ok()? * 2.55).round().clamp(0.0, 255.0) as u8)
            } else {
                Some(p.trim().parse::<f32>().ok()?.round().clamp(0.0, 255.0) as u8)
            }
        };
        let a = parts.get(3).and_then(|p| p.trim().trim_end_matches('%').parse::<f32>().ok()).map(|v| if parts[3].contains('%') { v / 100.0 } else { v }).unwrap_or(1.0);
        return Some(([chan(parts[0])?, chan(parts[1])?, chan(parts[2])?], a.clamp(0.0, 1.0)));
    }
    named_color(&s.to_ascii_lowercase()).map(|c| (c, 1.0))
}

#[derive(Clone)]
struct Style {
    fill: Paint,
    stroke: Paint,
    stroke_width: f32,
    cap: String,
    opacity: f32,
    fill_opacity: f32,
    stroke_opacity: f32,
    rule: FillRule,
    m: Mat,
}

/// Attributes plus `style="..."` declarations, later ones winning.
fn props(el: &Element) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = el.attrs.iter().filter(|(k, _)| k != "style").cloned().collect();
    if let Some(style) = el.attr("style") {
        for decl in style.split(';') {
            if let Some((k, v)) = decl.split_once(':') {
                out.push((k.trim().to_string(), v.trim().to_string()));
            }
        }
    }
    out
}

fn get<'a>(p: &'a [(String, String)], key: &str) -> Option<&'a str> {
    p.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

fn num(s: Option<&str>, default: f32) -> f32 {
    s.and_then(|v| v.trim().trim_end_matches("px").trim_end_matches('%').parse::<f32>().ok()).unwrap_or(default)
}

fn collect_gradients(el: &Element, map: &mut HashMap<String, ([u8; 3], f32)>, pending: &mut Vec<(String, String)>) {
    if el.name == "linearGradient" || el.name == "radialGradient" {
        if let Some(id) = el.attr("id") {
            let mut sum = [0f32; 3];
            let mut asum = 0f32;
            let mut n = 0f32;
            for stop in el.children.iter().filter(|c| c.name == "stop") {
                let p = props(stop);
                let (c, a) = get(&p, "stop-color").and_then(parse_color).unwrap_or(([0, 0, 0], 1.0));
                let a = a * num(get(&p, "stop-opacity"), 1.0);
                for k in 0..3 {
                    sum[k] += c[k] as f32;
                }
                asum += a;
                n += 1.0;
            }
            if n > 0.0 {
                map.insert(id.to_string(), ([(sum[0] / n) as u8, (sum[1] / n) as u8, (sum[2] / n) as u8], asum / n));
            } else if let Some(href) = el.attr("href").or(el.attr("xlink:href")) {
                pending.push((id.to_string(), href.trim_start_matches('#').to_string()));
            }
        }
    }
    for c in &el.children {
        collect_gradients(c, map, pending);
    }
}

struct Canvas {
    w: usize,
    h: usize,
    /// Premultiplied RGBA.
    px: Vec<f32>,
}

impl Canvas {
    fn fill(&mut self, subs: &[SubPath], rule: FillRule, color: [u8; 3], alpha: f32) {
        if alpha <= 0.0 || subs.is_empty() {
            return;
        }
        let cov = coverage(subs, rule, self.w, self.h);
        let (r, g, b) = (color[0] as f32 / 255.0, color[1] as f32 / 255.0, color[2] as f32 / 255.0);
        for (i, &c) in cov.iter().enumerate() {
            if c <= 0.0 {
                continue;
            }
            let a = (c.min(1.0) * alpha).min(1.0);
            let d = &mut self.px[i * 4..i * 4 + 4];
            let inv = 1.0 - a;
            d[0] = r * a + d[0] * inv;
            d[1] = g * a + d[1] * inv;
            d[2] = b * a + d[2] * inv;
            d[3] = a + d[3] * inv;
        }
    }
}

fn draw(el: &Element, parent: &Style, opts: &RenderOptions, gradients: &HashMap<String, ([u8; 3], f32)>, canvas: &mut Canvas) {
    match el.name.as_str() {
        "defs" | "clipPath" | "mask" | "symbol" | "filter" | "linearGradient" | "radialGradient" | "pattern" | "marker" | "metadata" | "title" | "desc" | "style" | "script" => return,
        _ => {}
    }
    let p = props(el);
    if get(&p, "display") == Some("none") || get(&p, "visibility") == Some("hidden") {
        return;
    }
    let mut st = parent.clone();
    if let Some(t) = get(&p, "transform") {
        st.m = st.m.mul(parse_transform(t));
    }
    if let Some(f) = get(&p, "fill").and_then(|v| parse_paint(v, gradients)) {
        st.fill = f;
    }
    if let Some(s) = get(&p, "stroke").and_then(|v| parse_paint(v, gradients)) {
        st.stroke = s;
    }
    if let Some(w) = get(&p, "stroke-width") {
        st.stroke_width = num(Some(w), st.stroke_width);
    }
    if let Some(c) = get(&p, "stroke-linecap") {
        st.cap = c.to_string();
    }
    if let Some(r) = get(&p, "fill-rule") {
        st.rule = if r == "evenodd" { FillRule::EvenOdd } else { FillRule::NonZero };
    }
    st.opacity *= num(get(&p, "opacity"), 1.0).clamp(0.0, 1.0);
    if let Some(v) = get(&p, "fill-opacity") {
        st.fill_opacity = num(Some(v), 1.0).clamp(0.0, 1.0);
    }
    if let Some(v) = get(&p, "stroke-opacity") {
        st.stroke_opacity = num(Some(v), 1.0).clamp(0.0, 1.0);
    }
    let tol = 0.25;
    let subs: Option<Vec<SubPath>> = match el.name.as_str() {
        "path" => get(&p, "d").map(|d| flatten_path(d, &st.m, tol)),
        "rect" => {
            let (x, y) = (num(get(&p, "x"), 0.0), num(get(&p, "y"), 0.0));
            let (w, h) = (num(get(&p, "width"), 0.0), num(get(&p, "height"), 0.0));
            let mut rx = num(get(&p, "rx"), -1.0);
            let mut ry = num(get(&p, "ry"), -1.0);
            if rx < 0.0 && ry < 0.0 {
                rx = 0.0;
                ry = 0.0;
            } else if rx < 0.0 {
                rx = ry;
            } else if ry < 0.0 {
                ry = rx;
            }
            let rx = rx.min(w / 2.0);
            let ry = ry.min(h / 2.0);
            let d = if rx > 0.0 && ry > 0.0 {
                format!(
                    "M{} {}H{}A{} {} 0 0 1 {} {}V{}A{} {} 0 0 1 {} {}H{}A{} {} 0 0 1 {} {}V{}A{} {} 0 0 1 {} {}Z",
                    x + rx, y, x + w - rx, rx, ry, x + w, y + ry, y + h - ry, rx, ry, x + w - rx, y + h, x + rx, rx, ry, x, y + h - ry, y + ry, rx, ry, x + rx, y
                )
            } else {
                format!("M{x} {y}h{w}v{h}h{}Z", -w)
            };
            Some(flatten_path(&d, &st.m, tol))
        }
        "circle" | "ellipse" => {
            let (cx, cy) = (num(get(&p, "cx"), 0.0), num(get(&p, "cy"), 0.0));
            let r = num(get(&p, "r"), 0.0);
            let rx = if el.name == "circle" { r } else { num(get(&p, "rx"), 0.0) };
            let ry = if el.name == "circle" { r } else { num(get(&p, "ry"), 0.0) };
            let d = format!("M{} {}A{rx} {ry} 0 1 0 {} {}A{rx} {ry} 0 1 0 {} {}Z", cx - rx, cy, cx + rx, cy, cx - rx, cy);
            Some(flatten_path(&d, &st.m, tol))
        }
        "line" => {
            let d = format!("M{} {}L{} {}", num(get(&p, "x1"), 0.0), num(get(&p, "y1"), 0.0), num(get(&p, "x2"), 0.0), num(get(&p, "y2"), 0.0));
            Some(flatten_path(&d, &st.m, tol))
        }
        "polyline" | "polygon" => {
            let nums = parse_numbers(get(&p, "points").unwrap_or(""));
            let mut d = String::new();
            for (k, pair) in nums.chunks_exact(2).enumerate() {
                d.push_str(&format!("{}{} {}", if k == 0 { "M" } else { "L" }, pair[0], pair[1]));
            }
            if el.name == "polygon" {
                d.push('Z');
            }
            Some(flatten_path(&d, &st.m, tol))
        }
        _ => None,
    };
    if let Some(subs) = subs {
        let resolve = |paint: Paint| -> Option<([u8; 3], f32)> {
            match paint {
                Paint::None => None,
                Paint::Color(c, a) => Some((c, a)),
                Paint::Current => Some((opts.current_color, 1.0)),
            }
        };
        if let Some((c, a)) = resolve(st.fill) {
            // Open sub-paths are implicitly closed for filling.
            canvas.fill(&subs, st.rule, c, a * st.fill_opacity * st.opacity);
        }
        if let Some((c, a)) = resolve(st.stroke) {
            let width = st.stroke_width * st.m.scale();
            if width > 0.01 {
                let polys = stroke_polygons(&subs, width, &st.cap);
                canvas.fill(&polys, FillRule::NonZero, c, a * st.stroke_opacity * st.opacity);
            }
        }
    }
    for c in &el.children {
        draw(c, &st, opts, gradients, canvas);
    }
}

/// The document's intrinsic size in user units (`viewBox`, else `width`/`height`).
pub fn size(svg: &str) -> Option<(f32, f32)> {
    let root = parse_xml(svg).ok()?;
    root_size(&root)
}

fn root_size(root: &Element) -> Option<(f32, f32)> {
    if let Some(vb) = root.attr("viewBox") {
        let n = parse_numbers(vb);
        if n.len() == 4 && n[2] > 0.0 && n[3] > 0.0 {
            return Some((n[2], n[3]));
        }
    }
    let w = num(root.attr("width"), 0.0);
    let h = num(root.attr("height"), 0.0);
    (w > 0.0 && h > 0.0).then_some((w, h))
}

/// Renders the document into a `width × height` picture, fitted and centred.
pub fn render(svg: &str, width: u32, height: u32, opts: &RenderOptions) -> Result<RgbaImage, String> {
    let root = parse_xml(svg)?;
    let (vx, vy, vw, vh) = match root.attr("viewBox").map(parse_numbers) {
        Some(n) if n.len() == 4 && n[2] > 0.0 && n[3] > 0.0 => (n[0], n[1], n[2], n[3]),
        _ => {
            let (w, h) = root_size(&root).unwrap_or((width as f32, height as f32));
            (0.0, 0.0, w, h)
        }
    };
    let (w, h) = (width.max(1) as usize, height.max(1) as usize);
    let s = (w as f32 / vw).min(h as f32 / vh);
    let ox = (w as f32 - vw * s) / 2.0 - vx * s;
    let oy = (h as f32 - vh * s) / 2.0 - vy * s;
    let base = Style {
        fill: Paint::Color([0, 0, 0], 1.0),
        stroke: Paint::None,
        stroke_width: 1.0,
        cap: "butt".into(),
        opacity: 1.0,
        fill_opacity: 1.0,
        stroke_opacity: 1.0,
        rule: FillRule::NonZero,
        m: Mat { a: s, b: 0.0, c: 0.0, d: s, e: ox, f: oy },
    };
    let mut gradients = HashMap::new();
    let mut pending = Vec::new();
    collect_gradients(&root, &mut gradients, &mut pending);
    for _ in 0..4 {
        for (id, href) in &pending {
            if let Some(v) = gradients.get(href).copied() {
                gradients.insert(id.clone(), v);
            }
        }
    }
    let mut canvas = Canvas { w, h, px: vec![0f32; w * h * 4] };
    // The root element's own presentation attributes apply to its children.
    draw_children(&root, &base, opts, &gradients, &mut canvas);
    let mut img = RgbaImage::new(w as u32, h as u32);
    for (i, p) in canvas.px.chunks_exact(4).enumerate() {
        let a = p[3].clamp(0.0, 1.0);
        let d = &mut img.pixels[i * 4..i * 4 + 4];
        if a > 0.0 {
            d[0] = (p[0] / a * 255.0).round().clamp(0.0, 255.0) as u8;
            d[1] = (p[1] / a * 255.0).round().clamp(0.0, 255.0) as u8;
            d[2] = (p[2] / a * 255.0).round().clamp(0.0, 255.0) as u8;
        }
        d[3] = (a * 255.0).round() as u8;
    }
    Ok(img)
}

fn draw_children(root: &Element, base: &Style, opts: &RenderOptions, gradients: &HashMap<String, ([u8; 3], f32)>, canvas: &mut Canvas) {
    // Treat the root like a group so its fill/stroke attributes inherit.
    let shell = Element { name: "g".into(), attrs: root.attrs.iter().filter(|(k, _)| k != "transform" && k != "width" && k != "height").cloned().collect(), children: Vec::new() };
    let p = props(&shell);
    let mut st = base.clone();
    if let Some(f) = get(&p, "fill").and_then(|v| parse_paint(v, gradients)) {
        st.fill = f;
    }
    if let Some(s) = get(&p, "stroke").and_then(|v| parse_paint(v, gradients)) {
        st.stroke = s;
    }
    if let Some(w) = get(&p, "stroke-width") {
        st.stroke_width = num(Some(w), st.stroke_width);
    }
    if let Some(c) = get(&p, "stroke-linecap") {
        st.cap = c.to_string();
    }
    for c in &root.children {
        draw(c, &st, opts, gradients, canvas);
    }
}

/// Renders fitted inside a `max_side` square, keeping the document's aspect.
pub fn render_fit(svg: &str, max_side: u32, opts: &RenderOptions) -> Result<RgbaImage, String> {
    let (vw, vh) = size(svg).unwrap_or((1.0, 1.0));
    let (w, h) = crate::image::fit((vw * 1000.0) as u32, (vh * 1000.0) as u32, max_side);
    render(svg, w, h, opts)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn px(img: &RgbaImage, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * img.width + x) * 4) as usize;
        img.pixels[i..i + 4].try_into().unwrap()
    }

    #[test]
    fn parses_numbers_in_svg_style() {
        assert_eq!(parse_numbers("1-2 .5.5,3e1"), vec![1.0, -2.0, 0.5, 0.5, 30.0]);
    }

    #[test]
    fn fills_a_circle_with_antialiased_edge() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 36 36"><circle fill="#FFCC4D" cx="18" cy="18" r="15.7"/></svg>"##;
        let img = render(svg, 72, 72, &RenderOptions::default()).unwrap();
        assert_eq!(px(&img, 36, 36), [255, 204, 77, 255]);
        assert_eq!(px(&img, 1, 1)[3], 0);
        // Somewhere on the rim the alpha is partial.
        let partial = (0..72).any(|x| {
            let a = px(&img, x, 36)[3];
            a > 10 && a < 245
        });
        assert!(partial);
        let cov = crate::image::coverage(&img, 128);
        let expect = std::f64::consts::PI * (31.4f64 * 31.4) / (72.0 * 72.0);
        assert!((cov - expect).abs() < 0.02, "{cov} vs {expect}");
    }

    #[test]
    fn even_odd_makes_a_hole_and_nonzero_does_not() {
        let ring = |rule: &str| {
            format!(
                r##"<svg viewBox="0 0 40 40"><path fill-rule="{rule}" fill="#000" d="M20 2a18 18 0 1 0 0.001 0zM20 10a10 10 0 1 0 0.001 0z"/></svg>"##
            )
        };
        let eo = render(&ring("evenodd"), 40, 40, &RenderOptions::default()).unwrap();
        let nz = render(&ring("nonzero"), 40, 40, &RenderOptions::default()).unwrap();
        assert_eq!(px(&eo, 20, 20)[3], 0);
        assert_eq!(px(&nz, 20, 20)[3], 255);
        assert_eq!(px(&eo, 20, 5)[3], 255);
    }

    #[test]
    fn strokes_with_current_color_and_round_caps() {
        let svg = r##"<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path stroke="none" d="M0 0h24v24H0z" fill="none"/><path d="M4 12h16"/></svg>"##;
        let opts = RenderOptions { current_color: [200, 30, 30] };
        let img = render(svg, 96, 96, &opts).unwrap();
        // Middle of the line: 2 user units = 8 px tall, centred at y=48.
        assert_eq!(px(&img, 48, 48), [200, 30, 30, 255]);
        assert_eq!(px(&img, 48, 40)[3], 0);
        assert_eq!(px(&img, 48, 56)[3], 0);
        // Round cap extends past x=20 user units (80 px) by one unit (4 px).
        assert!(px(&img, 82, 48)[3] > 128);
        assert_eq!(px(&img, 88, 48)[3], 0);
        // The background rect with fill="none" drew nothing.
        assert_eq!(px(&img, 2, 2)[3], 0);
    }

    #[test]
    fn transforms_gradients_and_groups() {
        let svg = r##"<svg viewBox="0 0 10 10"><defs><linearGradient id="g"><stop stop-color="#ff0000"/><stop stop-color="#0000ff"/></linearGradient></defs>
            <g transform="translate(5 5) scale(0.5)" opacity="0.5"><rect x="-5" y="-5" width="10" height="10" fill="url(#g)"/></g></svg>"##;
        let img = render(svg, 20, 20, &RenderOptions::default()).unwrap();
        let p = px(&img, 10, 10);
        assert_eq!(p[3], 128);
        assert!((p[0] as i32 - 127).abs() <= 1 && p[1] == 0 && (p[2] as i32 - 127).abs() <= 1, "{p:?}");
        assert_eq!(px(&img, 1, 1)[3], 0);
    }

    #[test]
    fn style_attribute_and_entities() {
        let svg = r##"<svg viewBox="0 0 4 4"><!-- c --><rect width="4" height="4" style="fill:#00ff00;fill-opacity:1" title="a &amp; b"/></svg>"##;
        let img = render(svg, 4, 4, &RenderOptions::default()).unwrap();
        assert_eq!(px(&img, 2, 2), [0, 255, 0, 255]);
        assert_eq!(size(svg), Some((4.0, 4.0)));
        assert!(render("<notsvg/>", 4, 4, &RenderOptions::default()).is_err());
    }

    #[test]
    fn arcs_and_relative_commands() {
        let svg = r##"<svg viewBox="0 0 24 24"><path d="M3 12a9 9 0 1 0 18 0a9 9 0 1 0 -18 0" fill="#000"/></svg>"##;
        let img = render(svg, 48, 48, &RenderOptions::default()).unwrap();
        assert_eq!(px(&img, 24, 24)[3], 255);
        assert_eq!(px(&img, 2, 2)[3], 0);
        let cov = crate::image::coverage(&img, 128);
        let expect = std::f64::consts::PI * 18.0 * 18.0 / (48.0 * 48.0);
        assert!((cov - expect).abs() < 0.03, "{cov} vs {expect}");
    }
}

