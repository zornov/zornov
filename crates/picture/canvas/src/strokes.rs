// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Strokes from geometry, so an agent can paint: an SVG path (`M L H V C S
//! Q T A Z`, absolute and relative), a polyline, or a shape (line, circle,
//! ellipse, rectangle, polygon, star, arc, spiral, wave), sampled densely
//! into points with a pressure each. Pressure can taper in and out, as a
//! hand-drawn line does; the brush engine turns pressure into size and
//! opacity per the brush's dynamics.

use serde_json::Value;

/// One sampled point: position in document pixels, pen pressure 0..1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pt {
    pub x: f64,
    pub y: f64,
    pub p: f64,
}

/// The points of a stroke description (see the module docs), sampled every
/// `step` pixels. `spec` holds one of `points` ([[x,y(,pressure)],…]),
/// `path` (SVG path data) or `shape` ({"kind": …}). `taper` ([in, out] as
/// shares of the length, or one number for both) and `pressure` (a
/// constant, or [start, end]) shape the pressure.
pub fn sample(spec: &Value, step: f64) -> Result<Vec<Vec<Pt>>, String> {
    let step = step.clamp(0.25, 64.0);
    let mut subpaths: Vec<Vec<Pt>> = if let Some(pts) = spec.get("points").and_then(Value::as_array) {
        let mut v = Vec::new();
        for p in pts {
            let a = p.as_array().ok_or("each point is [x, y] or [x, y, pressure]")?;
            let g = |i: usize| a.get(i).and_then(Value::as_f64);
            v.push(Pt { x: g(0).ok_or("a point needs x")?, y: g(1).ok_or("a point needs y")?, p: g(2).unwrap_or(1.0) });
        }
        vec![resample(&v, step)]
    } else if let Some(d) = spec.get("path").and_then(Value::as_str) {
        parse_path(d)?.into_iter().map(|sp| resample(&sp, step)).collect()
    } else if let Some(s) = spec.get("shape") {
        shape(s)?.into_iter().map(|sp| resample(&sp, step)).collect()
    } else {
        return Err("a stroke needs `points`, `path` (SVG path data) or `shape`".into());
    };
    subpaths.retain(|s| !s.is_empty());
    if subpaths.is_empty() {
        return Err("the stroke has no points".into());
    }
    let (p0, p1) = match spec.get("pressure") {
        Some(Value::Array(a)) => (a.first().and_then(Value::as_f64).unwrap_or(1.0), a.get(1).and_then(Value::as_f64).unwrap_or(1.0)),
        Some(v) => {
            let p = v.as_f64().unwrap_or(1.0);
            (p, p)
        }
        None => (f64::NAN, f64::NAN),
    };
    let (tin, tout) = match spec.get("taper") {
        Some(Value::Array(a)) => (a.first().and_then(Value::as_f64).unwrap_or(0.0), a.get(1).and_then(Value::as_f64).unwrap_or(0.0)),
        Some(v) => {
            let t = v.as_f64().unwrap_or(0.0);
            (t, t)
        }
        None => (0.0, 0.0),
    };
    for sp in &mut subpaths {
        let n = sp.len();
        for (i, q) in sp.iter_mut().enumerate() {
            let t = if n > 1 { i as f64 / (n - 1) as f64 } else { 0.5 };
            if p0.is_finite() {
                q.p = p0 + (p1 - p0) * t;
            }
            let mut k = 1.0;
            if tin > 0.0 && t < tin {
                k *= smooth(t / tin);
            }
            if tout > 0.0 && t > 1.0 - tout {
                k *= smooth((1.0 - t) / tout);
            }
            q.p = (q.p * (0.08 + 0.92 * k)).clamp(0.0, 1.0);
        }
    }
    Ok(subpaths)
}

fn smooth(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Points every `step` pixels along a polyline, pressure interpolated.
pub fn resample(pts: &[Pt], step: f64) -> Vec<Pt> {
    let Some(&first) = pts.first() else { return Vec::new() };
    let mut out = vec![first];
    let mut carry = 0.0;
    for w in pts.windows(2) {
        let (a, b) = (w[0], w[1]);
        let len = ((b.x - a.x).powi(2) + (b.y - a.y).powi(2)).sqrt();
        if len <= 1e-9 {
            continue;
        }
        let mut d = step - carry;
        while d <= len {
            let t = d / len;
            out.push(Pt { x: a.x + (b.x - a.x) * t, y: a.y + (b.y - a.y) * t, p: a.p + (b.p - a.p) * t });
            d += step;
        }
        carry = len - (d - step);
    }
    let last = *pts.last().unwrap_or(&first);
    if out.last().is_none_or(|q| (q.x - last.x).abs() + (q.y - last.y).abs() > 1e-6) {
        out.push(last);
    }
    out
}

fn num(v: &Value, k: &str) -> Option<f64> {
    v.get(k).and_then(Value::as_f64)
}

fn pt(v: &Value, k: &str) -> Option<(f64, f64)> {
    let a = v.get(k)?.as_array()?;
    Some((a.first()?.as_f64()?, a.get(1)?.as_f64()?))
}

/// The outline of a shape as closed or open polylines.
pub fn shape(s: &Value) -> Result<Vec<Vec<Pt>>, String> {
    let kind = s.get("kind").and_then(Value::as_str).ok_or("a shape needs `kind`: line, circle, ellipse, rect, polygon, star, arc, spiral or wave")?;
    let p = |x: f64, y: f64| Pt { x, y, p: 1.0 };
    let ring = |cx: f64, cy: f64, rx: f64, ry: f64, a0: f64, a1: f64, rot: f64| -> Vec<Pt> {
        let n = ((rx.abs().max(ry.abs()) * (a1 - a0).abs()).ceil() as usize).clamp(12, 4096);
        (0..=n)
            .map(|i| {
                let a = a0 + (a1 - a0) * i as f64 / n as f64;
                let (x, y) = (rx * a.cos(), ry * a.sin());
                p(cx + x * rot.cos() - y * rot.sin(), cy + x * rot.sin() + y * rot.cos())
            })
            .collect()
    };
    let (cx, cy) = pt(s, "center").unwrap_or((0.0, 0.0));
    let rot = num(s, "rotation").unwrap_or(0.0).to_radians();
    Ok(match kind {
        "line" => {
            let a = pt(s, "from").ok_or("a line needs `from` and `to`")?;
            let b = pt(s, "to").ok_or("a line needs `from` and `to`")?;
            vec![vec![p(a.0, a.1), p(b.0, b.1)]]
        }
        "circle" => {
            let r = num(s, "radius").ok_or("a circle needs `center` and `radius`")?;
            vec![ring(cx, cy, r, r, 0.0, std::f64::consts::TAU, 0.0)]
        }
        "ellipse" => {
            let (rx, ry) = pt(s, "radii").ok_or("an ellipse needs `center` and `radii` [rx, ry]")?;
            vec![ring(cx, cy, rx, ry, 0.0, std::f64::consts::TAU, rot)]
        }
        "arc" => {
            let r = num(s, "radius").ok_or("an arc needs `center`, `radius`, `from` and `to` (degrees)")?;
            let a0 = num(s, "from").unwrap_or(0.0).to_radians();
            let a1 = num(s, "to").unwrap_or(180.0).to_radians();
            vec![ring(cx, cy, r, r, a0, a1, 0.0)]
        }
        "rect" => {
            let x = num(s, "x").unwrap_or(0.0);
            let y = num(s, "y").unwrap_or(0.0);
            let w = num(s, "width").ok_or("a rect needs x, y, width, height")?;
            let h = num(s, "height").ok_or("a rect needs x, y, width, height")?;
            vec![vec![p(x, y), p(x + w, y), p(x + w, y + h), p(x, y + h), p(x, y)]]
        }
        "polygon" | "star" => {
            let r = num(s, "radius").ok_or("a polygon needs `center`, `radius` and `sides`")?;
            let n = num(s, "sides").or_else(|| num(s, "points")).unwrap_or(5.0).clamp(3.0, 256.0) as usize;
            let inner = num(s, "inner").unwrap_or(0.45) * r;
            let k = if kind == "star" { 2 * n } else { n };
            let mut v: Vec<Pt> = (0..=k)
                .map(|i| {
                    let a = -std::f64::consts::FRAC_PI_2 + rot + std::f64::consts::TAU * i as f64 / k as f64;
                    let rr = if kind == "star" && i % 2 == 1 { inner } else { r };
                    p(cx + rr * a.cos(), cy + rr * a.sin())
                })
                .collect();
            if let Some(last) = v.first().copied() {
                *v.last_mut().unwrap_or(&mut p(0.0, 0.0)) = last;
            }
            vec![v]
        }
        "spiral" => {
            let r = num(s, "radius").ok_or("a spiral needs `center`, `radius` and `turns`")?;
            let turns = num(s, "turns").unwrap_or(3.0).clamp(0.1, 100.0);
            let n = ((r * turns * 4.0).ceil() as usize).clamp(32, 20000);
            vec![(0..=n).map(|i| {
                let t = i as f64 / n as f64;
                let a = rot + std::f64::consts::TAU * turns * t;
                p(cx + r * t * a.cos(), cy + r * t * a.sin())
            }).collect()]
        }
        "wave" => {
            let a = pt(s, "from").ok_or("a wave needs `from`, `to`, `amplitude` and `waves`")?;
            let b = pt(s, "to").ok_or("a wave needs `from`, `to`, `amplitude` and `waves`")?;
            let amp = num(s, "amplitude").unwrap_or(20.0);
            let waves = num(s, "waves").unwrap_or(3.0);
            let (dx, dy) = (b.0 - a.0, b.1 - a.1);
            let len = (dx * dx + dy * dy).sqrt().max(1e-9);
            let (nx, ny) = (-dy / len, dx / len);
            let n = (len.ceil() as usize).clamp(16, 20000);
            vec![(0..=n).map(|i| {
                let t = i as f64 / n as f64;
                let o = amp * (std::f64::consts::TAU * waves * t).sin();
                p(a.0 + dx * t + nx * o, a.1 + dy * t + ny * o)
            }).collect()]
        }
        other => return Err(format!("unknown shape `{other}`: line, circle, ellipse, rect, polygon, star, arc, spiral, wave")),
    })
}

/// SVG path data as polylines (one per subpath), curves flattened.
pub fn parse_path(d: &str) -> Result<Vec<Vec<Pt>>, String> {
    let toks = tokenize(d)?;
    let mut out: Vec<Vec<Pt>> = Vec::new();
    let mut cur: Vec<Pt> = Vec::new();
    let (mut x, mut y) = (0.0f64, 0.0f64);
    let (mut sx, mut sy) = (0.0, 0.0);
    let mut last_ctrl: Option<(f64, f64)> = None;
    let mut last_cmd = ' ';
    let mut i = 0;
    let mut cmd = ' ';
    let push = |cur: &mut Vec<Pt>, x: f64, y: f64| cur.push(Pt { x, y, p: 1.0 });
    while i < toks.len() {
        if let Tok::Cmd(c) = toks[i] {
            cmd = c;
            i += 1;
        } else if cmd == ' ' {
            return Err("path data must start with a command (M)".into());
        }
        let rel = cmd.is_ascii_lowercase();
        let mut take = |n: usize| -> Result<Vec<f64>, String> {
            let mut v = Vec::with_capacity(n);
            for _ in 0..n {
                match toks.get(i) {
                    Some(Tok::Num(f)) => {
                        v.push(*f);
                        i += 1;
                    }
                    _ => return Err(format!("`{cmd}` needs {n} numbers")),
                }
            }
            Ok(v)
        };
        let (ox, oy) = if rel { (x, y) } else { (0.0, 0.0) };
        match cmd.to_ascii_uppercase() {
            'M' => {
                let v = take(2)?;
                if cur.len() > 1 {
                    out.push(std::mem::take(&mut cur));
                }
                cur.clear();
                x = ox + v[0];
                y = oy + v[1];
                sx = x;
                sy = y;
                push(&mut cur, x, y);
                // Further pairs after M are line-tos.
                cmd = if rel { 'l' } else { 'L' };
                last_ctrl = None;
            }
            'L' => {
                let v = take(2)?;
                x = ox + v[0];
                y = oy + v[1];
                push(&mut cur, x, y);
                last_ctrl = None;
            }
            'H' => {
                let v = take(1)?;
                x = if rel { x + v[0] } else { v[0] };
                push(&mut cur, x, y);
                last_ctrl = None;
            }
            'V' => {
                let v = take(1)?;
                y = if rel { y + v[0] } else { v[0] };
                push(&mut cur, x, y);
                last_ctrl = None;
            }
            'C' | 'S' => {
                let smooth = cmd.eq_ignore_ascii_case(&'S');
                let v = take(if smooth { 4 } else { 6 })?;
                let (c1x, c1y) = if smooth {
                    match (last_ctrl, last_cmd.to_ascii_uppercase()) {
                        (Some((lx, ly)), 'C' | 'S') => (2.0 * x - lx, 2.0 * y - ly),
                        _ => (x, y),
                    }
                } else {
                    (ox + v[0], oy + v[1])
                };
                let o = if smooth { 0 } else { 2 };
                let (c2x, c2y, ex, ey) = (ox + v[o], oy + v[o + 1], ox + v[o + 2], oy + v[o + 3]);
                cubic(&mut cur, (x, y), (c1x, c1y), (c2x, c2y), (ex, ey));
                last_ctrl = Some((c2x, c2y));
                x = ex;
                y = ey;
            }
            'Q' | 'T' => {
                let smooth = cmd.eq_ignore_ascii_case(&'T');
                let v = take(if smooth { 2 } else { 4 })?;
                let (qx, qy) = if smooth {
                    match (last_ctrl, last_cmd.to_ascii_uppercase()) {
                        (Some((lx, ly)), 'Q' | 'T') => (2.0 * x - lx, 2.0 * y - ly),
                        _ => (x, y),
                    }
                } else {
                    (ox + v[0], oy + v[1])
                };
                let o = if smooth { 0 } else { 2 };
                let (ex, ey) = (ox + v[o], oy + v[o + 1]);
                // Quadratic as cubic.
                let c1 = (x + 2.0 / 3.0 * (qx - x), y + 2.0 / 3.0 * (qy - y));
                let c2 = (ex + 2.0 / 3.0 * (qx - ex), ey + 2.0 / 3.0 * (qy - ey));
                cubic(&mut cur, (x, y), c1, c2, (ex, ey));
                last_ctrl = Some((qx, qy));
                x = ex;
                y = ey;
            }
            'A' => {
                let v = take(7)?;
                let (ex, ey) = (ox + v[5], oy + v[6]);
                arc(&mut cur, (x, y), v[0], v[1], v[2], v[3] != 0.0, v[4] != 0.0, (ex, ey));
                x = ex;
                y = ey;
                last_ctrl = None;
            }
            'Z' => {
                push(&mut cur, sx, sy);
                x = sx;
                y = sy;
                if cur.len() > 1 {
                    out.push(std::mem::take(&mut cur));
                }
                cur.clear();
                push(&mut cur, x, y);
                last_ctrl = None;
                // Z takes no numbers; a number after it is an error unless a command follows.
                if matches!(toks.get(i), Some(Tok::Num(_))) {
                    return Err("numbers after Z need a command".into());
                }
            }
            other => return Err(format!("unknown path command `{other}`")),
        }
        last_cmd = cmd;
    }
    if cur.len() > 1 {
        out.push(cur);
    }
    if out.is_empty() {
        return Err("the path draws nothing".into());
    }
    Ok(out)
}

fn cubic(out: &mut Vec<Pt>, p0: (f64, f64), p1: (f64, f64), p2: (f64, f64), p3: (f64, f64)) {
    let len = ((p1.0 - p0.0).hypot(p1.1 - p0.1) + (p2.0 - p1.0).hypot(p2.1 - p1.1) + (p3.0 - p2.0).hypot(p3.1 - p2.1)).max(1.0);
    let n = (len / 2.0).ceil().clamp(4.0, 2000.0) as usize;
    for i in 1..=n {
        let t = i as f64 / n as f64;
        let u = 1.0 - t;
        let x = u * u * u * p0.0 + 3.0 * u * u * t * p1.0 + 3.0 * u * t * t * p2.0 + t * t * t * p3.0;
        let y = u * u * u * p0.1 + 3.0 * u * u * t * p1.1 + 3.0 * u * t * t * p2.1 + t * t * t * p3.1;
        out.push(Pt { x, y, p: 1.0 });
    }
}

/// SVG elliptical arc (endpoint parameterisation, SVG 1.1 appendix F.6).
#[allow(clippy::too_many_arguments)]
fn arc(out: &mut Vec<Pt>, p0: (f64, f64), rx: f64, ry: f64, phi_deg: f64, large: bool, sweep: bool, p1: (f64, f64)) {
    let (mut rx, mut ry) = (rx.abs(), ry.abs());
    if rx < 1e-9 || ry < 1e-9 || (p0.0 - p1.0).abs() + (p0.1 - p1.1).abs() < 1e-9 {
        out.push(Pt { x: p1.0, y: p1.1, p: 1.0 });
        return;
    }
    let phi = phi_deg.to_radians();
    let (cp, sp) = (phi.cos(), phi.sin());
    let dx = (p0.0 - p1.0) / 2.0;
    let dy = (p0.1 - p1.1) / 2.0;
    let x1 = cp * dx + sp * dy;
    let y1 = -sp * dx + cp * dy;
    let lam = x1 * x1 / (rx * rx) + y1 * y1 / (ry * ry);
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
    let cx = cp * cx1 - sp * cy1 + (p0.0 + p1.0) / 2.0;
    let cy = sp * cx1 + cp * cy1 + (p0.1 + p1.1) / 2.0;
    let ang = |ux: f64, uy: f64, vx: f64, vy: f64| (ux * vy - uy * vx).atan2(ux * vx + uy * vy);
    let t1 = ang(1.0, 0.0, (x1 - cx1) / rx, (y1 - cy1) / ry);
    let mut dt = ang((x1 - cx1) / rx, (y1 - cy1) / ry, (-x1 - cx1) / rx, (-y1 - cy1) / ry);
    if !sweep && dt > 0.0 {
        dt -= std::f64::consts::TAU;
    } else if sweep && dt < 0.0 {
        dt += std::f64::consts::TAU;
    }
    let n = ((rx.max(ry) * dt.abs() / 2.0).ceil() as usize).clamp(4, 2000);
    for i in 1..=n {
        let t = t1 + dt * i as f64 / n as f64;
        let (ex, ey) = (rx * t.cos(), ry * t.sin());
        out.push(Pt { x: cp * ex - sp * ey + cx, y: sp * ex + cp * ey + cy, p: 1.0 });
    }
}

#[derive(Clone, Copy, Debug)]
enum Tok {
    Cmd(char),
    Num(f64),
}

fn tokenize(d: &str) -> Result<Vec<Tok>, String> {
    let b = d.as_bytes();
    let mut i = 0;
    let mut out = Vec::new();
    while i < b.len() {
        let c = b[i] as char;
        if c.is_ascii_whitespace() || c == ',' {
            i += 1;
        } else if "MmLlHhVvCcSsQqTtAaZz".contains(c) {
            out.push(Tok::Cmd(c));
            i += 1;
        } else if c == '-' || c == '+' || c == '.' || c.is_ascii_digit() {
            let start = i;
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
            let s = &d[start..i];
            out.push(Tok::Num(s.parse::<f64>().map_err(|_| format!("bad number `{s}` in the path"))?));
        } else {
            return Err(format!("unexpected `{c}` in the path"));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_path_samples_evenly_along_lines_and_curves() {
        let s = sample(&json!({"path": "M0 0 L100 0 C100 50 50 100 0 100 Z"}), 2.0).unwrap();
        assert_eq!(s.len(), 1);
        let pts = &s[0];
        for w in pts.windows(2) {
            let d = (w[1].x - w[0].x).hypot(w[1].y - w[0].y);
            assert!(d <= 2.0 + 1e-6, "gap {d}");
        }
        let last = pts.last().unwrap();
        assert!(last.x.abs() < 1e-6 && last.y.abs() < 1e-6, "Z closes back to the start");
    }

    #[test]
    fn relative_commands_and_arcs_land_where_they_say() {
        let s = parse_path("m10 10 h20 v20 a10 10 0 0 1 -20 0 z").unwrap();
        let p = &s[0];
        assert!((p[1].x - 30.0).abs() < 1e-9 && (p[2].y - 30.0).abs() < 1e-9);
        // The arc ends at (10, 30).
        let end_of_arc = p[p.len() - 2];
        assert!((end_of_arc.x - 10.0).abs() < 1e-6 && (end_of_arc.y - 30.0).abs() < 1e-6, "{end_of_arc:?}");
    }

    #[test]
    fn tapers_thin_both_ends() {
        let s = sample(&json!({"shape": {"kind": "line", "from": [0, 0], "to": [200, 0]}, "taper": 0.3}), 1.0).unwrap();
        let p = &s[0];
        assert!(p[0].p < 0.15 && p[p.len() - 1].p < 0.15);
        assert!((p[p.len() / 2].p - 1.0).abs() < 1e-9);
    }

    #[test]
    fn shapes_close_and_bad_input_says_why() {
        let s = shape(&json!({"kind": "star", "center": [50, 50], "radius": 40, "points": 5})).unwrap();
        let v = &s[0];
        assert_eq!(v.len(), 11);
        assert!((v[0].x - v[10].x).abs() < 1e-9 && (v[0].y - v[10].y).abs() < 1e-9);
        assert!(sample(&json!({}), 1.0).unwrap_err().contains("points"));
        assert!(parse_path("L 1 2").is_err());
        assert!(parse_path("M 1 2 X").is_err());
    }
}
