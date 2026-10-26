// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Rotoscoping: an object's per-frame masks (from SAM) turned into one
//! editable outline that moves from frame to frame, the way a roto artist
//! keys a mask path.
//!
//! Each frame's mask is traced (marching squares, sub-pixel) and its
//! largest outline is resampled to the same number of points, evenly along
//! its length, turning the same way, and started where the last frame's
//! points were (the cyclic shift that moves them least), so point *k* stays
//! on the same part of the object and the path interpolates cleanly.
//! A light smoothing over time takes out the boil.

use crate::frame::Mask;
use std::collections::HashMap;

/// The outline of the largest region of `m` above 0.5, as a closed
/// polygon in pixel coordinates (centres of pixels at `x + 0.5`), turning
/// clockwise on screen. `None` when nothing is inside.
pub fn largest_outline(m: &Mask) -> Option<Vec<(f32, f32)>> {
    let (w, h) = (m.width, m.height);
    if w < 2 || h < 2 {
        return None;
    }
    // A one-pixel empty border so every outline closes.
    let (pw, ph) = (w + 2, h + 2);
    let v = |x: usize, y: usize| -> f32 {
        if x == 0 || y == 0 || x > w || y > h {
            0.0
        } else {
            m.data[(y - 1) * w + (x - 1)]
        }
    };
    // Edge ids: horizontal (x, y)→(x+1, y) is 2·(y·pw + x), vertical
    // (x, y)→(x, y+1) is 2·(y·pw + x) + 1.
    let hid = |x: usize, y: usize| 2 * (y * pw + x);
    let vid = |x: usize, y: usize| 2 * (y * pw + x) + 1;
    let mut pos: HashMap<usize, (f32, f32)> = HashMap::new();
    let cross = |a: f32, b: f32| ((0.5 - a) / (b - a)).clamp(0.0, 1.0);
    let mut adj: HashMap<usize, Vec<usize>> = HashMap::new();
    let mut link = |a: usize, b: usize, adj: &mut HashMap<usize, Vec<usize>>| {
        adj.entry(a).or_default().push(b);
        adj.entry(b).or_default().push(a);
    };
    for y in 0..ph - 1 {
        for x in 0..pw - 1 {
            let (tl, tr, br, bl) = (v(x, y), v(x + 1, y), v(x + 1, y + 1), v(x, y + 1));
            let case = (usize::from(tl > 0.5) << 3) | (usize::from(tr > 0.5) << 2) | (usize::from(br > 0.5) << 1) | usize::from(bl > 0.5);
            if case == 0 || case == 15 {
                continue;
            }
            let (t, r, b, l) = (hid(x, y), vid(x + 1, y), hid(x, y + 1), vid(x, y));
            // Where each edge crosses 0.5 (in padded pixel coordinates).
            let mut at = |id: usize, p: (f32, f32)| {
                pos.entry(id).or_insert(p);
            };
            at(t, (x as f32 + cross(tl, tr), y as f32));
            at(b, (x as f32 + cross(bl, br), y as f32 + 1.0));
            at(l, (x as f32, y as f32 + cross(tl, bl)));
            at(r, (x as f32 + 1.0, y as f32 + cross(tr, br)));
            let centre = (tl + tr + br + bl) / 4.0 > 0.5;
            let segs: &[(usize, usize)] = match case {
                1 | 14 => &[(l, b)],
                2 | 13 => &[(b, r)],
                3 | 12 => &[(l, r)],
                4 | 11 => &[(t, r)],
                6 | 9 => &[(t, b)],
                7 | 8 => &[(t, l)],
                5 => {
                    if centre {
                        &[(t, l), (r, b)]
                    } else {
                        &[(t, r), (l, b)]
                    }
                }
                10 => {
                    if centre {
                        &[(t, r), (l, b)]
                    } else {
                        &[(t, l), (r, b)]
                    }
                }
                _ => &[],
            };
            for &(a, b2) in segs {
                link(a, b2, &mut adj);
            }
        }
    }
    // Walk the loops.
    let mut seen: std::collections::HashSet<usize> = Default::default();
    let mut best: Option<(f32, Vec<(f32, f32)>)> = None;
    let mut starts: Vec<usize> = adj.keys().copied().collect();
    starts.sort_unstable();
    for s in starts {
        if seen.contains(&s) {
            continue;
        }
        let mut loop_pts = Vec::new();
        let (mut prev, mut cur) = (usize::MAX, s);
        loop {
            seen.insert(cur);
            loop_pts.push(pos[&cur]);
            let next = adj[&cur].iter().copied().find(|&n| n != prev && !(n == s && loop_pts.len() < 3) && (!seen.contains(&n) || n == s));
            match next {
                Some(n) if n == s => break,
                Some(n) => {
                    prev = cur;
                    cur = n;
                }
                None => break,
            }
        }
        if loop_pts.len() < 3 {
            continue;
        }
        let a = area(&loop_pts);
        if best.as_ref().is_none_or(|b| a.abs() > b.0) {
            best = Some((a.abs(), loop_pts));
        }
    }
    let (_, mut pts) = best?;
    // Back to mask pixel coordinates (the border shifted everything by 1,
    // and a pixel's centre is at +0.5).
    for p in &mut pts {
        p.0 -= 0.5;
        p.1 -= 0.5;
    }
    if area(&pts) < 0.0 {
        pts.reverse();
    }
    Some(pts)
}

/// Signed area (positive: clockwise on screen, y down).
pub fn area(p: &[(f32, f32)]) -> f32 {
    let n = p.len();
    (0..n).map(|i| p[i].0 * p[(i + 1) % n].1 - p[(i + 1) % n].0 * p[i].1).sum::<f32>() / 2.0
}

/// `n` points evenly spaced along the closed polygon `p`, starting at its
/// first point.
pub fn resample(p: &[(f32, f32)], n: usize) -> Vec<(f32, f32)> {
    let m = p.len();
    if m == 0 || n == 0 {
        return Vec::new();
    }
    let seg = |i: usize| {
        let (a, b) = (p[i], p[(i + 1) % m]);
        ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt()
    };
    let total: f32 = (0..m).map(seg).sum();
    if total <= 1e-6 {
        return vec![p[0]; n];
    }
    let step = total / n as f32;
    let mut out = Vec::with_capacity(n);
    let (mut i, mut into) = (0usize, 0.0f32);
    for k in 0..n {
        let want = k as f32 * step;
        while into + seg(i) < want && i < m - 1 {
            into += seg(i);
            i += 1;
        }
        let l = seg(i).max(1e-6);
        let t = ((want - into) / l).clamp(0.0, 1.0);
        let (a, b) = (p[i], p[(i + 1) % m]);
        out.push((a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t));
    }
    out
}

/// `cur` turned to start where `prev`'s points are (the cyclic shift with
/// the least movement).
pub fn align(prev: &[(f32, f32)], cur: &[(f32, f32)]) -> Vec<(f32, f32)> {
    let n = cur.len();
    if prev.len() != n || n == 0 {
        return cur.to_vec();
    }
    let cost = |s: usize| (0..n).map(|k| (cur[(k + s) % n].0 - prev[k].0).powi(2) + (cur[(k + s) % n].1 - prev[k].1).powi(2)).sum::<f32>();
    let best = (0..n).min_by(|&a, &b| cost(a).total_cmp(&cost(b))).unwrap_or(0);
    (0..n).map(|k| cur[(k + best) % n]).collect()
}

/// The roto outline of each frame's mask: `n` points per frame, aligned
/// frame to frame. Frames where the object is missing hold the last
/// outline (`None` before the first).
pub fn outlines(masks: &[Option<Mask>], n: usize) -> Vec<Option<Vec<(f32, f32)>>> {
    let mut out: Vec<Option<Vec<(f32, f32)>>> = Vec::with_capacity(masks.len());
    let mut last: Option<Vec<(f32, f32)>> = None;
    for m in masks {
        let o = m.as_ref().and_then(largest_outline).map(|p| {
            let r = resample(&p, n);
            match &last {
                Some(prev) => align(prev, &r),
                None => {
                    // Start at the topmost point (then leftmost): the same
                    // place whichever frame the outline begins on.
                    let s = (0..r.len()).min_by(|&a, &b| r[a].1.total_cmp(&r[b].1).then(r[a].0.total_cmp(&r[b].0))).unwrap_or(0);
                    (0..r.len()).map(|k| r[(k + s) % r.len()]).collect()
                }
            }
        });
        if o.is_some() {
            last = o.clone();
        }
        out.push(o.or_else(|| last.clone()));
    }
    out
}

/// Smooths each point's motion over frames (Gaussian, `sigma` frames).
pub fn smooth(paths: &mut [Option<Vec<(f32, f32)>>], sigma: f32) {
    if sigma < 0.3 {
        return;
    }
    let r = (sigma * 2.5).ceil() as isize;
    let w: Vec<f32> = (-r..=r).map(|k| (-(k * k) as f32 / (2.0 * sigma * sigma)).exp()).collect();
    let src = paths.to_vec();
    let n = src.len() as isize;
    for i in 0..n {
        let Some(base) = &src[i as usize] else { continue };
        let mut acc = vec![(0.0f32, 0.0f32); base.len()];
        let mut ws = 0.0;
        for (j, k) in (-r..=r).enumerate() {
            let q = (i + k).clamp(0, n - 1) as usize;
            let Some(p) = &src[q] else { continue };
            if p.len() != base.len() {
                continue;
            }
            for (a, b) in acc.iter_mut().zip(p) {
                a.0 += b.0 * w[j];
                a.1 += b.1 * w[j];
            }
            ws += w[j];
        }
        if ws > 0.0 {
            paths[i as usize] = Some(acc.into_iter().map(|(x, y)| (x / ws, y / ws)).collect());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn disc(w: usize, h: usize, cx: f32, cy: f32, r: f32) -> Mask {
        let d = (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as f32 + 0.5, (i / w) as f32 + 0.5);
                (r - ((x - cx).powi(2) + (y - cy).powi(2)).sqrt() + 0.5).clamp(0.0, 1.0)
            })
            .collect();
        Mask::new(w, h, d)
    }

    #[test]
    fn traces_a_disc_with_subpixel_accuracy() {
        let m = disc(80, 60, 40.0, 30.0, 18.0);
        let o = largest_outline(&m).unwrap();
        assert!(area(&o) > 0.0, "clockwise");
        for p in &o {
            let r = ((p.0 - 40.0).powi(2) + (p.1 - 30.0).powi(2)).sqrt();
            assert!((r - 18.0).abs() < 0.3, "{p:?} at r {r}");
        }
        let a = area(&o);
        assert!((a - std::f32::consts::PI * 18.0 * 18.0).abs() < 25.0, "{a}");
    }

    #[test]
    fn keeps_the_largest_region_and_the_picture_edge_closes_it() {
        let mut m = disc(80, 60, 20.0, 30.0, 6.0);
        let big = disc(80, 60, 70.0, 30.0, 25.0);
        for (a, b) in m.data.iter_mut().zip(&big.data) {
            *a = a.max(*b);
        }
        let o = largest_outline(&m).unwrap();
        let cx = o.iter().map(|p| p.0).sum::<f32>() / o.len() as f32;
        assert!(cx > 55.0, "the big one, clipped by the right edge: {cx}");
        assert!(largest_outline(&Mask::zeros(10, 10)).is_none());
    }

    #[test]
    fn outlines_keep_their_points_on_the_same_part() {
        // A disc moving right: point k should move right too, not around.
        let masks: Vec<Option<Mask>> = (0..6).map(|i| Some(disc(100, 60, 30.0 + 6.0 * i as f32, 30.0, 15.0))).chain([None]).collect();
        let o = outlines(&masks, 24);
        assert_eq!(o.len(), 7);
        for i in 1..6 {
            let (a, b) = (o[i - 1].as_ref().unwrap(), o[i].as_ref().unwrap());
            for k in 0..24 {
                let (dx, dy) = (b[k].0 - a[k].0, b[k].1 - a[k].1);
                assert!((dx - 6.0).abs() < 1.5 && dy.abs() < 1.5, "frame {i} point {k}: {dx},{dy}");
            }
        }
        assert_eq!(o[6], o[5], "a lost frame holds the last outline");
        let mut s = o.clone();
        smooth(&mut s, 1.0);
        assert_eq!(s[0].as_ref().unwrap().len(), 24);
    }
}
