// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Bézier paths: sub-paths of vertices with absolute in and out control
//! points, as Lottie holds them. Flattening, lengths,
//! exact trimming (curves are split, not resampled), transforms, and SVG
//! path data in.

/// A point.
pub type P = [f64; 2];

/// A vertex: where the path passes (`p`), the control point the curve
/// arrives along (`i`) and the one it leaves along (`o`). A corner has
/// both equal to `p`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct V {
    pub p: P,
    pub i: P,
    pub o: P,
}

impl V {
    pub fn corner(p: P) -> V {
        V { p, i: p, o: p }
    }

    pub fn is_corner(&self) -> bool {
        self.i == self.p && self.o == self.p
    }
}

/// One continuous run of curves.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Sub {
    pub v: Vec<V>,
    pub closed: bool,
}

/// Several sub-paths drawn as one shape.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Path {
    pub subs: Vec<Sub>,
}

/// A 2D affine transform `[a, b, c, d, e, f]`: x' = a·x + c·y + e,
/// y' = b·x + d·y + f.
pub type M = [f64; 6];

pub const ID: M = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

pub fn mul(a: M, b: M) -> M {
    // a after b.
    [a[0] * b[0] + a[2] * b[1], a[1] * b[0] + a[3] * b[1], a[0] * b[2] + a[2] * b[3], a[1] * b[2] + a[3] * b[3], a[0] * b[4] + a[2] * b[5] + a[4], a[1] * b[4] + a[3] * b[5] + a[5]]
}

pub fn apply(m: &M, p: P) -> P {
    [m[0] * p[0] + m[2] * p[1] + m[4], m[1] * p[0] + m[3] * p[1] + m[5]]
}

pub fn translate(x: f64, y: f64) -> M {
    [1.0, 0.0, 0.0, 1.0, x, y]
}

pub fn scale(x: f64, y: f64) -> M {
    [x, 0.0, 0.0, y, 0.0, 0.0]
}

/// Degrees, clockwise on screen (y down).
pub fn rotate(deg: f64) -> M {
    let (s, c) = deg.to_radians().sin_cos();
    [c, s, -s, c, 0.0, 0.0]
}

/// Shear along x by `deg` measured from the axis at `axis_deg`.
pub fn skew(deg: f64, axis_deg: f64) -> M {
    let k = deg.to_radians().tan();
    mul(rotate(axis_deg), mul([1.0, 0.0, k, 1.0, 0.0, 0.0], rotate(-axis_deg)))
}

/// The usual layer transform: move the anchor to the origin, scale,
/// skew, rotate, then move to the position.
pub fn layer(position: P, anchor: P, scale_xy: P, rotation: f64, skew_deg: f64, skew_axis: f64) -> M {
    let mut m = translate(-anchor[0], -anchor[1]);
    m = mul(scale(scale_xy[0], scale_xy[1]), m);
    if skew_deg != 0.0 {
        m = mul(skew(skew_deg, skew_axis), m);
    }
    m = mul(rotate(rotation), m);
    mul(translate(position[0], position[1]), m)
}

pub fn lerp(a: P, b: P, t: f64) -> P {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
}

pub fn dist(a: P, b: P) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

/// A cubic segment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cubic(pub P, pub P, pub P, pub P);

impl Cubic {
    pub fn at(&self, t: f64) -> P {
        let u = 1.0 - t;
        let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
        [a * self.0[0] + b * self.1[0] + c * self.2[0] + d * self.3[0], a * self.0[1] + b * self.1[1] + c * self.2[1] + d * self.3[1]]
    }

    /// The direction of travel at `t` (not normalised).
    pub fn tangent(&self, t: f64) -> P {
        let u = 1.0 - t;
        let d = [
            3.0 * u * u * (self.1[0] - self.0[0]) + 6.0 * u * t * (self.2[0] - self.1[0]) + 3.0 * t * t * (self.3[0] - self.2[0]),
            3.0 * u * u * (self.1[1] - self.0[1]) + 6.0 * u * t * (self.2[1] - self.1[1]) + 3.0 * t * t * (self.3[1] - self.2[1]),
        ];
        if d[0].abs() + d[1].abs() < 1e-12 { [self.3[0] - self.0[0], self.3[1] - self.0[1]] } else { d }
    }

    /// The two halves at `t` (de Casteljau).
    pub fn split(&self, t: f64) -> (Cubic, Cubic) {
        let ab = lerp(self.0, self.1, t);
        let bc = lerp(self.1, self.2, t);
        let cd = lerp(self.2, self.3, t);
        let abc = lerp(ab, bc, t);
        let bcd = lerp(bc, cd, t);
        let m = lerp(abc, bcd, t);
        (Cubic(self.0, ab, abc, m), Cubic(m, bcd, cd, self.3))
    }

    /// The piece between `a` and `b` (0 ≤ a ≤ b ≤ 1).
    pub fn piece(&self, a: f64, b: f64) -> Cubic {
        if b <= a {
            let p = self.at(a);
            return Cubic(p, p, p, p);
        }
        let right = if a > 0.0 { self.split(a).1 } else { *self };
        if b >= 1.0 {
            return right;
        }
        let u = (b - a) / (1.0 - a);
        right.split(u).0
    }

    pub fn is_line(&self) -> bool {
        self.1 == self.0 && self.2 == self.3
    }

    /// Steps to flatten within `tol`.
    fn steps(&self, tol: f64) -> usize {
        if self.is_line() {
            return 1;
        }
        // Distance of the control points from the chord bounds the error.
        let dd = |a: P, b: P, c: P| -> f64 { ((a[0] - 2.0 * b[0] + c[0]).powi(2) + (a[1] - 2.0 * b[1] + c[1]).powi(2)).sqrt() };
        let m = dd(self.0, self.1, self.2).max(dd(self.1, self.2, self.3));
        ((m * 0.75 / tol.max(1e-6)).sqrt().ceil() as usize).clamp(1, 256)
    }

    /// Points along the curve after the start, within `tol`.
    pub fn flatten_into(&self, tol: f64, out: &mut Vec<P>) {
        let n = self.steps(tol);
        for k in 1..=n {
            out.push(self.at(k as f64 / n as f64));
        }
    }

    /// Arc length, from a fine flattening.
    pub fn length(&self) -> f64 {
        if self.is_line() {
            return dist(self.0, self.3);
        }
        let mut pts = vec![self.0];
        self.flatten_into(0.05, &mut pts);
        pts.windows(2).map(|w| dist(w[0], w[1])).sum()
    }

    /// Curve parameter at a fraction of the length (from a table).
    pub fn t_at_length(&self, frac: f64) -> f64 {
        if self.is_line() || frac <= 0.0 || frac >= 1.0 {
            return frac.clamp(0.0, 1.0);
        }
        const N: usize = 48;
        let mut cum = [0.0f64; N + 1];
        let mut prev = self.0;
        for (k, slot) in cum.iter_mut().enumerate().skip(1) {
            let p = self.at(k as f64 / N as f64);
            *slot = 0.0 + dist(prev, p);
            prev = p;
        }
        for k in 1..=N {
            cum[k] += cum[k - 1];
        }
        let want = frac * cum[N];
        let k = cum.partition_point(|c| *c < want).clamp(1, N);
        let span = (cum[k] - cum[k - 1]).max(1e-12);
        ((k - 1) as f64 + (want - cum[k - 1]) / span) / N as f64
    }
}

impl Sub {
    /// The segments in order (closing one included when closed).
    pub fn segments(&self) -> Vec<Cubic> {
        let n = self.v.len();
        if n < 2 {
            return Vec::new();
        }
        let seg = |a: &V, b: &V| Cubic(a.p, a.o, b.i, b.p);
        let mut out: Vec<Cubic> = self.v.windows(2).map(|w| seg(&w[0], &w[1])).collect();
        if self.closed {
            out.push(seg(&self.v[n - 1], &self.v[0]));
        }
        out
    }

    /// A sub-path through `segs` (each starting where the last ended).
    pub fn from_segments(segs: &[Cubic], closed: bool) -> Sub {
        let mut v: Vec<V> = Vec::with_capacity(segs.len() + 1);
        for (k, s) in segs.iter().enumerate() {
            if k == 0 {
                v.push(V { p: s.0, i: s.0, o: s.1 });
            } else if let Some(last) = v.last_mut() {
                last.o = s.1;
            }
            v.push(V { p: s.3, i: s.2, o: s.3 });
        }
        if closed && v.len() > 1 {
            // The closing segment ends on the first vertex.
            let last = v.pop().unwrap_or(V::corner([0.0; 2]));
            v[0].i = last.i;
        }
        Sub { v, closed }
    }

    /// A polyline (corner vertices).
    pub fn polyline(pts: &[P], closed: bool) -> Sub {
        Sub { v: pts.iter().map(|p| V::corner(*p)).collect(), closed }
    }

    /// Points along the sub-path within `tol` (the first point once; a
    /// closed one does not repeat it at the end).
    pub fn flatten(&self, tol: f64) -> Vec<P> {
        let Some(first) = self.v.first() else { return Vec::new() };
        let mut out = vec![first.p];
        for s in self.segments() {
            s.flatten_into(tol, &mut out);
        }
        if self.closed && out.len() > 1 && dist(out[0], out[out.len() - 1]) < 1e-9 {
            out.pop();
        }
        out
    }

    pub fn length(&self) -> f64 {
        self.segments().iter().map(Cubic::length).sum()
    }

    pub fn transform(&self, m: &M) -> Sub {
        Sub { v: self.v.iter().map(|v| V { p: apply(m, v.p), i: apply(m, v.i), o: apply(m, v.o) }).collect(), closed: self.closed }
    }

    pub fn reversed(&self) -> Sub {
        let mut v: Vec<V> = self.v.iter().rev().map(|v| V { p: v.p, i: v.o, o: v.i }).collect();
        if self.closed && !v.is_empty() {
            // Keep the same first vertex.
            v.rotate_right(1);
        }
        Sub { v, closed: self.closed }
    }

    /// The part between lengths `a` and `b` (`a < b`, both within
    /// 0..length), as open sub-paths: one, or two when a closed path is
    /// cut across its start.
    pub fn cut(&self, a: f64, b: f64) -> Vec<Sub> {
        let segs = self.segments();
        let lens: Vec<f64> = segs.iter().map(Cubic::length).collect();
        let total: f64 = lens.iter().sum();
        if total <= 0.0 || b <= a {
            return Vec::new();
        }
        let mut out = Vec::new();
        let mut at = 0.0;
        for (s, l) in segs.iter().zip(&lens) {
            let (s0, s1) = (at, at + l);
            at = s1;
            if s1 <= a || s0 >= b || *l <= 0.0 {
                continue;
            }
            let ta = if a > s0 { s.t_at_length((a - s0) / l) } else { 0.0 };
            let tb = if b < s1 { s.t_at_length((b - s0) / l) } else { 1.0 };
            out.push(s.piece(ta, tb));
        }
        if out.is_empty() {
            return Vec::new();
        }
        vec![Sub::from_segments(&out, false)]
    }

    /// The centre of the vertices.
    pub fn centroid(&self) -> P {
        let n = self.v.len().max(1) as f64;
        let s = self.v.iter().fold([0.0, 0.0], |a, v| [a[0] + v.p[0], a[1] + v.p[1]]);
        [s[0] / n, s[1] / n]
    }

    /// Signed area of the flattened outline (positive clockwise on screen).
    pub fn area(&self) -> f64 {
        let pts = self.flatten(0.5);
        let n = pts.len();
        (0..n).map(|k| {
            let (a, b) = (pts[k], pts[(k + 1) % n]);
            a[0] * b[1] - b[0] * a[1]
        })
        .sum::<f64>()
            * 0.5
    }
}

impl Path {
    pub fn new(subs: Vec<Sub>) -> Path {
        Path { subs }
    }

    pub fn is_empty(&self) -> bool {
        self.subs.iter().all(|s| s.v.is_empty())
    }

    pub fn transform(&self, m: &M) -> Path {
        Path { subs: self.subs.iter().map(|s| s.transform(m)).collect() }
    }

    pub fn length(&self) -> f64 {
        self.subs.iter().map(Sub::length).sum()
    }

    /// Bounds of the vertices and control points: (min, max).
    pub fn bounds(&self) -> Option<(P, P)> {
        let mut it = self.subs.iter().flat_map(|s| s.v.iter().flat_map(|v| [v.p, v.i, v.o]));
        let first = it.next()?;
        Some(it.fold((first, first), |(lo, hi), p| ([lo[0].min(p[0]), lo[1].min(p[1])], [hi[0].max(p[0]), hi[1].max(p[1])])))
    }

    /// For Studio's vector rasterizer (`None` when empty).
    pub fn to_bez(&self) -> Option<studio_vector_raster::kurbo::BezPath> {
        use studio_vector_raster::kurbo::{BezPath, Point};
        let mut bp = BezPath::new();
        let pt = |p: P| Point::new(p[0], p[1]);
        for s in &self.subs {
            let Some(first) = s.v.first() else { continue };
            bp.move_to(pt(first.p));
            for c in s.segments() {
                if c.is_line() {
                    bp.line_to(pt(c.3));
                } else {
                    bp.curve_to(pt(c.1), pt(c.2), pt(c.3));
                }
            }
            if s.closed {
                bp.close_path();
            }
        }
        (!bp.elements().is_empty()).then_some(bp)
    }

    /// A dense polyline of the whole path, for text on a path and
    /// sampling.
    pub fn polyline(&self, tol: f64) -> Vec<P> {
        let mut out = Vec::new();
        for s in &self.subs {
            let mut pts = s.flatten(tol);
            if s.closed
                && let Some(f) = pts.first().copied()
            {
                pts.push(f);
            }
            out.extend(pts);
        }
        out
    }
}

/// Smooth handles through `pts` (Catmull-Rom as Béziers).
pub fn smooth(pts: &[P], closed: bool, tension: f64) -> Sub {
    let n = pts.len();
    if n < 3 {
        return Sub::polyline(pts, closed);
    }
    let get = |k: isize| -> P {
        if closed {
            pts[k.rem_euclid(n as isize) as usize]
        } else {
            pts[k.clamp(0, n as isize - 1) as usize]
        }
    };
    let k = tension / 6.0;
    let v = (0..n as isize)
        .map(|j| {
            let (a, p, b) = (get(j - 1), get(j), get(j + 1));
            let d = [(b[0] - a[0]) * k, (b[1] - a[1]) * k];
            V { p, i: [p[0] - d[0], p[1] - d[1]], o: [p[0] + d[0], p[1] + d[1]] }
        })
        .collect();
    Sub { v, closed }
}

/// SVG path data (`M L H V C S Q T A Z`, upper and lower case) as a
/// path.
pub fn parse_svg(d: &str) -> Result<Path, String> {
    let mut toks: Vec<Result<f64, char>> = Vec::new();
    let b = d.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let c = b[i] as char;
        if c.is_ascii_alphabetic() && c != 'e' && c != 'E' {
            toks.push(Err(c));
            i += 1;
        } else if c.is_ascii_digit() || c == '-' || c == '+' || c == '.' {
            let s = i;
            i += 1;
            let mut dot = c == '.';
            while i < b.len() {
                let ch = b[i] as char;
                if ch.is_ascii_digit() {
                    i += 1;
                } else if ch == '.' && !dot {
                    dot = true;
                    i += 1;
                } else if (ch == 'e' || ch == 'E') && i + 1 < b.len() {
                    i += 1;
                    if b[i] == b'-' || b[i] == b'+' {
                        i += 1;
                    }
                } else {
                    break;
                }
            }
            toks.push(Ok(d[s..i].parse::<f64>().map_err(|_| format!("bad number {}", &d[s..i]))?));
        } else {
            i += 1;
        }
    }
    let mut subs: Vec<Sub> = Vec::new();
    let mut cur: Option<Sub> = None;
    let mut pen = [0.0, 0.0];
    let mut start = [0.0, 0.0];
    let mut last_ctrl: Option<(char, P)> = None;
    let mut k = 0;
    let mut cmd = 'M';
    let num = |k: &mut usize| -> Result<f64, String> {
        match toks.get(*k) {
            Some(Ok(v)) => {
                *k += 1;
                Ok(*v)
            }
            _ => Err("path data: a number is missing".into()),
        }
    };
    while k < toks.len() {
        if let Err(c) = toks[k] {
            cmd = c;
            k += 1;
        } else if cmd == 'M' {
            cmd = 'L';
        } else if cmd == 'm' {
            cmd = 'l';
        }
        let rel = cmd.is_ascii_lowercase();
        let base = if rel { pen } else { [0.0, 0.0] };
        let pt = |k: &mut usize| -> Result<P, String> { Ok([base[0] + num(k)?, base[1] + num(k)?]) };
        let push_seg = |cur: &mut Option<Sub>, c1: P, c2: P, to: P, pen: P| {
            let sub = cur.get_or_insert_with(|| Sub { v: vec![V::corner(pen)], closed: false });
            if let Some(last) = sub.v.last_mut() {
                last.o = c1;
            }
            sub.v.push(V { p: to, i: c2, o: to });
        };
        match cmd.to_ascii_uppercase() {
            'M' => {
                let p = pt(&mut k)?;
                if let Some(s) = cur.take() {
                    subs.push(s);
                }
                cur = Some(Sub { v: vec![V::corner(p)], closed: false });
                pen = p;
                start = p;
                last_ctrl = None;
            }
            'L' => {
                let p = pt(&mut k)?;
                push_seg(&mut cur, pen, p, p, pen);
                pen = p;
                last_ctrl = None;
            }
            'H' => {
                let x = num(&mut k)? + if rel { pen[0] } else { 0.0 };
                let p = [x, pen[1]];
                push_seg(&mut cur, pen, p, p, pen);
                pen = p;
                last_ctrl = None;
            }
            'V' => {
                let y = num(&mut k)? + if rel { pen[1] } else { 0.0 };
                let p = [pen[0], y];
                push_seg(&mut cur, pen, p, p, pen);
                pen = p;
                last_ctrl = None;
            }
            'C' | 'S' => {
                let c1 = if cmd.eq_ignore_ascii_case(&'S') {
                    match last_ctrl {
                        Some(('C', c)) => [2.0 * pen[0] - c[0], 2.0 * pen[1] - c[1]],
                        _ => pen,
                    }
                } else {
                    pt(&mut k)?
                };
                let c2 = pt(&mut k)?;
                let p = pt(&mut k)?;
                push_seg(&mut cur, c1, c2, p, pen);
                pen = p;
                last_ctrl = Some(('C', c2));
            }
            'Q' | 'T' => {
                let q = if cmd.eq_ignore_ascii_case(&'T') {
                    match last_ctrl {
                        Some(('Q', c)) => [2.0 * pen[0] - c[0], 2.0 * pen[1] - c[1]],
                        _ => pen,
                    }
                } else {
                    pt(&mut k)?
                };
                let p = pt(&mut k)?;
                let c1 = lerp(pen, q, 2.0 / 3.0);
                let c2 = lerp(p, q, 2.0 / 3.0);
                push_seg(&mut cur, c1, c2, p, pen);
                pen = p;
                last_ctrl = Some(('Q', q));
            }
            'A' => {
                let (rx, ry, rot) = (num(&mut k)?, num(&mut k)?, num(&mut k)?);
                let (large, sweep) = (num(&mut k)? != 0.0, num(&mut k)? != 0.0);
                let p = pt(&mut k)?;
                for c in arc(pen, p, rx, ry, rot, large, sweep) {
                    push_seg(&mut cur, c.1, c.2, c.3, pen);
                }
                pen = p;
                last_ctrl = None;
            }
            'Z' => {
                if let Some(mut s) = cur.take() {
                    // A last point back on the first is the same vertex.
                    if s.v.len() > 1 && dist(s.v[s.v.len() - 1].p, s.v[0].p) < 1e-9 {
                        let last = s.v.pop().unwrap_or(V::corner(start));
                        s.v[0].i = last.i;
                    }
                    s.closed = true;
                    subs.push(s);
                }
                pen = start;
                last_ctrl = None;
            }
            other => return Err(format!("path data: unknown command {other}")),
        }
        if k > toks.len() + 1 {
            break;
        }
    }
    if let Some(s) = cur.take() {
        subs.push(s);
    }
    Ok(Path { subs })
}

/// An SVG elliptical arc as cubic segments.
fn arc(from: P, to: P, rx: f64, ry: f64, rot_deg: f64, large: bool, sweep: bool) -> Vec<Cubic> {
    if dist(from, to) < 1e-12 {
        return Vec::new();
    }
    let (mut rx, mut ry) = (rx.abs(), ry.abs());
    if rx < 1e-12 || ry < 1e-12 {
        return vec![Cubic(from, from, to, to)];
    }
    let (s, c) = rot_deg.to_radians().sin_cos();
    let dx = (from[0] - to[0]) / 2.0;
    let dy = (from[1] - to[1]) / 2.0;
    let x1 = c * dx + s * dy;
    let y1 = -s * dx + c * dy;
    let lam = (x1 * x1) / (rx * rx) + (y1 * y1) / (ry * ry);
    if lam > 1.0 {
        rx *= lam.sqrt();
        ry *= lam.sqrt();
    }
    let num = rx * rx * ry * ry - rx * rx * y1 * y1 - ry * ry * x1 * x1;
    let den = rx * rx * y1 * y1 + ry * ry * x1 * x1;
    let mut co = (num / den).max(0.0).sqrt();
    if large == sweep {
        co = -co;
    }
    let cx1 = co * rx * y1 / ry;
    let cy1 = -co * ry * x1 / rx;
    let cx = c * cx1 - s * cy1 + (from[0] + to[0]) / 2.0;
    let cy = s * cx1 + c * cy1 + (from[1] + to[1]) / 2.0;
    let ang = |ux: f64, uy: f64, vx: f64, vy: f64| -> f64 {
        let a = (ux * vy - uy * vx).atan2(ux * vx + uy * vy);
        a
    };
    let t1 = ang(1.0, 0.0, (x1 - cx1) / rx, (y1 - cy1) / ry);
    let mut dt = ang((x1 - cx1) / rx, (y1 - cy1) / ry, (-x1 - cx1) / rx, (-y1 - cy1) / ry);
    if !sweep && dt > 0.0 {
        dt -= std::f64::consts::TAU;
    } else if sweep && dt < 0.0 {
        dt += std::f64::consts::TAU;
    }
    let n = (dt.abs() / (std::f64::consts::FRAC_PI_2)).ceil().max(1.0) as usize;
    let step = dt / n as f64;
    let k = 4.0 / 3.0 * (step / 4.0).tan();
    let point = |t: f64| -> (P, P) {
        let (st, ct) = t.sin_cos();
        let p = [cx + c * rx * ct - s * ry * st, cy + s * rx * ct + c * ry * st];
        let d = [-c * rx * st - s * ry * ct, -s * rx * st + c * ry * ct];
        (p, d)
    };
    let mut out = Vec::with_capacity(n);
    for j in 0..n {
        let (a, b) = (t1 + step * j as f64, t1 + step * (j + 1) as f64);
        let (p0, d0) = point(a);
        let (p3, d3) = point(b);
        let p0 = if j == 0 { from } else { p0 };
        let p3 = if j == n - 1 { to } else { p3 };
        out.push(Cubic(p0, [p0[0] + k * d0[0], p0[1] + k * d0[1]], [p3[0] - k * d3[0], p3[1] - k * d3[1]], p3));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square() -> Sub {
        Sub::polyline(&[[0.0, 0.0], [100.0, 0.0], [100.0, 100.0], [0.0, 100.0]], true)
    }

    #[test]
    fn lengths_and_cuts() {
        let s = square();
        assert!((s.length() - 400.0).abs() < 1e-9);
        let half = s.cut(50.0, 250.0);
        assert_eq!(half.len(), 1);
        assert!((half[0].length() - 200.0).abs() < 1e-6);
        assert_eq!(half[0].v[0].p, [50.0, 0.0]);
        assert_eq!(half[0].v.last().unwrap().p, [50.0, 100.0]);
        // A quarter circle's length.
        let k = 0.552_284_749_8 * 100.0;
        let q = Cubic([100.0, 0.0], [100.0, k], [k, 100.0], [0.0, 100.0]);
        assert!((q.length() - std::f64::consts::FRAC_PI_2 * 100.0).abs() < 0.1);
        let mid = q.at(q.t_at_length(0.5));
        assert!((mid[0] - mid[1]).abs() < 0.2, "{mid:?}");
    }

    #[test]
    fn svg_paths_parse() {
        let p = parse_svg("M10 10 h 80 v 80 H 10 z").unwrap();
        assert_eq!(p.subs.len(), 1);
        assert!(p.subs[0].closed);
        assert_eq!(p.subs[0].v.len(), 4);
        assert!((p.length() - 320.0).abs() < 1e-9);
        let c = parse_svg("M0,0 C 0,50 50,100 100,100 S 200,50 200,0 Q 150 -50 100 -50 T 0 0").unwrap();
        assert_eq!(c.subs[0].v.len(), 5);
        let a = parse_svg("M 0 0 A 50 50 0 0 1 100 0").unwrap();
        // A half circle of radius 50.
        assert!((a.length() - std::f64::consts::PI * 50.0).abs() < 0.5, "{}", a.length());
        assert!(parse_svg("M 0 0 L 5").is_err());
    }

    #[test]
    fn transforms_compose() {
        let m = layer([10.0, 0.0], [0.0, 0.0], [2.0, 2.0], 90.0, 0.0, 0.0);
        let p = apply(&m, [1.0, 0.0]);
        assert!((p[0] - 10.0).abs() < 1e-9 && (p[1] - 2.0).abs() < 1e-9, "{p:?}");
        let r = square().reversed();
        assert_eq!(r.v[0].p, [0.0, 0.0]);
        assert!(square().area() > 0.0 && r.area() < 0.0);
    }
}
