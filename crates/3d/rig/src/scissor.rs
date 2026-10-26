// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Auto scissor: where a character's picture should be cut so its parts
//! can move apart (the cut lines a 2D puppet or skeleton mesh is split
//! along; see `studio_model::Cut`).
//!
//! Everything works on a silhouette grid (`true` = the character) and
//! returns paths in 0..1 picture coordinates:
//!
//! - [`gaps`]: narrow gaps between limbs and the concave notches at the
//!   crotch and armpits. A morphological closing of the silhouette (a disk
//!   about as wide as the gaps worth cutting) fills those places and no
//!   others; each connected piece it adds, open to the outside, is a gap.
//!   The cut runs down its middle from the notch's apex to where it opens.
//! - [`mouth`]: the line of the lips, from the mouth corners (face
//!   landmarks, or an estimate), snapped to the dark line in the picture
//!   when one is there. Its ends stop short of the corners so the corners
//!   stay joined and the cheeks follow them.
//! - [`boundaries`]: the borders between labelled parts (what the part
//!   splitter produces), trimmed round the joints so a limb stays attached
//!   to its body where it turns.

use std::collections::{BinaryHeap, HashMap, VecDeque};

/// A silhouette: `w × h` cells, row-major, `true` where the character is.
#[derive(Clone, Copy, Debug)]
pub struct Mask<'a> {
    pub w: usize,
    pub h: usize,
    pub on: &'a [bool],
}

/// What a suggested cut separates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A narrow gap or notch between two parts (legs, an arm and the torso).
    Gap,
    /// The lips.
    Mouth,
    /// The border of a part found by the part splitter.
    Part,
}

/// A suggested cut.
#[derive(Clone, Debug, PartialEq)]
pub struct Suggestion {
    pub kind: Kind,
    /// A name for it ("gap", "mouth", "part 2/5").
    pub name: String,
    /// The path, 0..1 picture coordinates, at least two points.
    pub points: Vec<[f64; 2]>,
    /// The gap's inside as an sRGB colour (straight alpha), when one makes
    /// sense (a mouth's dark inside).
    pub fill: Option<[f64; 4]>,
    /// How sure the finder is, 0..1.
    pub confidence: f64,
    /// For a part border: the two part labels it lies between.
    pub between: Option<(u32, u32)>,
}

/// Tuning for [`gaps`].
#[derive(Clone, Copy, Debug)]
pub struct GapOptions {
    /// Widest gap worth cutting, as a fraction of the silhouette's longer
    /// side.
    pub width: f64,
    /// Shortest cut, in gap widths (a piece shorter than it is wide is a
    /// corner, not a gap).
    pub min_length: f64,
}

impl Default for GapOptions {
    fn default() -> Self {
        GapOptions { width: 0.06, min_length: 1.0 }
    }
}

// ------------------------------------------------------------ helpers

/// Distance from each cell to the nearest cell where `feature` is true
/// (Felzenszwalb and Huttenlocher's exact Euclidean distance transform).
fn distance(feature: &[bool], w: usize, h: usize) -> Vec<f32> {
    const INF: f32 = 1e20;
    fn pass(f: &[f32], out: &mut [f32], v: &mut [usize], z: &mut [f32]) {
        let n = f.len();
        let mut k = 0usize;
        v[0] = 0;
        z[0] = -INF;
        z[1] = INF;
        for q in 1..n {
            loop {
                let p = v[k];
                let s = ((f[q] + (q * q) as f32) - (f[p] + (p * p) as f32)) / (2.0 * q as f32 - 2.0 * p as f32);
                if s <= z[k] && k > 0 {
                    k -= 1;
                    continue;
                }
                if s <= z[k] {
                    // k == 0: replace the first parabola.
                    v[0] = q;
                    z[0] = -INF;
                    z[1] = INF;
                    break;
                }
                k += 1;
                v[k] = q;
                z[k] = s;
                z[k + 1] = INF;
                break;
            }
        }
        k = 0;
        for (q, o) in out.iter_mut().enumerate().take(n) {
            while z[k + 1] < q as f32 {
                k += 1;
            }
            let d = q as f32 - v[k] as f32;
            *o = d * d + f[v[k]];
        }
    }
    let mut g: Vec<f32> = feature.iter().map(|&b| if b { 0.0 } else { INF }).collect();
    let m = w.max(h);
    let (mut v, mut z, mut line, mut res) = (vec![0usize; m], vec![0f32; m + 1], vec![0f32; m], vec![0f32; m]);
    for x in 0..w {
        for y in 0..h {
            line[y] = g[y * w + x];
        }
        pass(&line[..h], &mut res[..h], &mut v, &mut z);
        for y in 0..h {
            g[y * w + x] = res[y];
        }
    }
    for y in 0..h {
        line[..w].copy_from_slice(&g[y * w..(y + 1) * w]);
        pass(&line[..w], &mut res[..w], &mut v, &mut z);
        g[y * w..(y + 1) * w].copy_from_slice(&res[..w]);
    }
    g.iter().map(|d| d.sqrt()).collect()
}

const N8: [(i64, i64); 8] = [(1, 0), (-1, 0), (0, 1), (0, -1), (1, 1), (1, -1), (-1, 1), (-1, -1)];

fn step_cost(dx: i64, dy: i64) -> f32 {
    if dx != 0 && dy != 0 { std::f32::consts::SQRT_2 } else { 1.0 }
}

/// Cheapest path from `a` to `b` over cells where `allowed`, with a cost
/// per cell, as cell indices. `None` when `b` cannot be reached.
fn cheapest(allowed: &[bool], cost: &[f32], w: usize, h: usize, a: usize, b: usize) -> Option<Vec<usize>> {
    #[derive(PartialEq)]
    struct Item(f32, usize);
    impl Eq for Item {}
    impl PartialOrd for Item {
        fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
            Some(self.cmp(o))
        }
    }
    impl Ord for Item {
        fn cmp(&self, o: &Self) -> std::cmp::Ordering {
            o.0.total_cmp(&self.0).then(o.1.cmp(&self.1))
        }
    }
    let mut d = vec![f32::INFINITY; w * h];
    let mut from = vec![usize::MAX; w * h];
    let mut heap = BinaryHeap::new();
    d[a] = 0.0;
    heap.push(Item(0.0, a));
    while let Some(Item(du, u)) = heap.pop() {
        if u == b {
            break;
        }
        if du > d[u] {
            continue;
        }
        let (ux, uy) = ((u % w) as i64, (u / w) as i64);
        for (dx, dy) in N8 {
            let (vx, vy) = (ux + dx, uy + dy);
            if vx < 0 || vy < 0 || vx >= w as i64 || vy >= h as i64 {
                continue;
            }
            let v = vy as usize * w + vx as usize;
            if !allowed[v] {
                continue;
            }
            let nd = du + step_cost(dx, dy) * cost[v];
            if nd < d[v] {
                d[v] = nd;
                from[v] = u;
                heap.push(Item(nd, v));
            }
        }
    }
    if !d[b].is_finite() {
        return None;
    }
    let mut path = vec![b];
    while *path.last().unwrap() != a {
        path.push(from[*path.last().unwrap()]);
    }
    path.reverse();
    Some(path)
}

/// Breadth-first (chamfer-free) hop distances from `seeds` inside
/// `allowed`; unreached cells stay `u32::MAX`.
fn hops(allowed: &[bool], w: usize, h: usize, seeds: &[usize]) -> Vec<u32> {
    let mut d = vec![u32::MAX; w * h];
    let mut q = VecDeque::new();
    for &s in seeds {
        d[s] = 0;
        q.push_back(s);
    }
    while let Some(u) = q.pop_front() {
        let (ux, uy) = ((u % w) as i64, (u / w) as i64);
        for (dx, dy) in N8 {
            let (vx, vy) = (ux + dx, uy + dy);
            if vx < 0 || vy < 0 || vx >= w as i64 || vy >= h as i64 {
                continue;
            }
            let v = vy as usize * w + vx as usize;
            if allowed[v] && d[v] == u32::MAX {
                d[v] = d[u] + 1;
                q.push_back(v);
            }
        }
    }
    d
}

/// Douglas-Peucker simplification of a polyline.
pub fn simplify(pts: &[[f64; 2]], eps: f64) -> Vec<[f64; 2]> {
    if pts.len() < 3 {
        return pts.to_vec();
    }
    let (a, b) = (pts[0], pts[pts.len() - 1]);
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let len = (dx * dx + dy * dy).sqrt();
    let (mut worst, mut at) = (0.0, 0);
    for (i, p) in pts.iter().enumerate().take(pts.len() - 1).skip(1) {
        let d = if len < 1e-12 { ((p[0] - a[0]).powi(2) + (p[1] - a[1]).powi(2)).sqrt() } else { ((p[0] - a[0]) * dy - (p[1] - a[1]) * dx).abs() / len };
        if d > worst {
            worst = d;
            at = i;
        }
    }
    if worst <= eps {
        return vec![a, b];
    }
    let mut left = simplify(&pts[..=at], eps);
    left.pop();
    left.extend(simplify(&pts[at..], eps));
    left
}

// --------------------------------------------------------------- gaps

/// Narrow gaps between limbs and notches at the crotch and armpits, as cuts
/// running from the deep end of each to where it opens.
pub fn gaps(mask: Mask, opts: GapOptions) -> Vec<Suggestion> {
    let (w, h) = (mask.w, mask.h);
    if w == 0 || h == 0 || mask.on.len() < w * h {
        return Vec::new();
    }
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0usize, 0usize);
    for y in 0..h {
        for x in 0..w {
            if mask.on[y * w + x] {
                x0 = x0.min(x);
                x1 = x1.max(x);
                y0 = y0.min(y);
                y1 = y1.max(y);
            }
        }
    }
    if x0 > x1 {
        return Vec::new();
    }
    let long = ((x1 - x0 + 1).max(y1 - y0 + 1)) as f64;
    let r = (opts.width * long).max(2.0) as f32;
    let pad = r.ceil() as usize + 2;
    let (pw, ph) = (w + 2 * pad, h + 2 * pad);
    let mut on = vec![false; pw * ph];
    for y in 0..h {
        for x in 0..w {
            on[(y + pad) * pw + x + pad] = mask.on[y * w + x];
        }
    }
    // Closing: dilate by r, then erode by r.
    let d_on = distance(&on, pw, ph);
    let dilated: Vec<bool> = d_on.iter().map(|d| *d <= r).collect();
    let not_dilated: Vec<bool> = dilated.iter().map(|b| !b).collect();
    let d_out = distance(&not_dilated, pw, ph);
    let closed: Vec<bool> = (0..pw * ph).map(|i| dilated[i] && d_out[i] > r).collect();
    // Outside: background reachable from the border.
    let off: Vec<bool> = on.iter().map(|b| !b).collect();
    let reach = hops(&off, pw, ph, &[0]);
    let outside: Vec<bool> = (0..pw * ph).map(|i| reach[i] != u32::MAX).collect();
    // What the closing added, open to the outside.
    let gap: Vec<bool> = (0..pw * ph).map(|i| closed[i] && !on[i] && outside[i]).collect();
    // Gap cells touching background that is not gap: where the gap opens.
    let front: Vec<bool> = (0..pw * ph)
        .map(|i| {
            if !gap[i] {
                return false;
            }
            let (x, y) = ((i % pw) as i64, (i / pw) as i64);
            N8.iter().any(|(dx, dy)| {
                let (nx, ny) = (x + dx, y + dy);
                nx >= 0 && ny >= 0 && nx < pw as i64 && ny < ph as i64 && outside[ny as usize * pw + nx as usize] && !gap[ny as usize * pw + nx as usize]
            })
        })
        .collect();
    // Connected pieces.
    let mut seen = vec![false; pw * ph];
    let mut out = Vec::new();
    for s in 0..pw * ph {
        if !gap[s] || seen[s] {
            continue;
        }
        let d0 = hops(&gap, pw, ph, &[s]);
        let cells: Vec<usize> = (0..pw * ph).filter(|&i| d0[i] != u32::MAX).collect();
        for &c in &cells {
            seen[c] = true;
        }
        // The two ends: farthest from anywhere, then farthest from that.
        let a = *cells.iter().max_by_key(|&&c| d0[c]).unwrap();
        let da = hops(&gap, pw, ph, &[a]);
        let b = *cells.iter().max_by_key(|&&c| da[c]).unwrap();
        let len = da[b] as f32;
        let width = cells.iter().map(|&c| d_on[c]).fold(0.0f32, f32::max);
        if len < opts.min_length as f32 * 2.0 * width.max(2.0) || len < 3.0 {
            continue;
        }
        // An open end sits at the centre of the opening, not at its corner.
        let centre_of_front = |end: usize| -> Option<usize> {
            let near: Vec<usize> = cells.iter().copied().filter(|&c| front[c]).filter(|&c| {
                let (dx, dy) = ((c % pw) as f32 - (end % pw) as f32, (c / pw) as f32 - (end / pw) as f32);
                (dx * dx + dy * dy).sqrt() <= r * 1.5
            }).collect();
            if near.len() < 2 {
                return None;
            }
            let (sx, sy) = near.iter().fold((0.0f32, 0.0f32), |s, &c| (s.0 + (c % pw) as f32, s.1 + (c / pw) as f32));
            let (cx, cy) = (sx / near.len() as f32, sy / near.len() as f32);
            cells.iter().copied().min_by(|&p, &q| {
                let dp = ((p % pw) as f32 - cx).powi(2) + ((p / pw) as f32 - cy).powi(2);
                let dq = ((q % pw) as f32 - cx).powi(2) + ((q / pw) as f32 - cy).powi(2);
                dp.total_cmp(&dq)
            })
        };
        let (a_open, b_open) = (front[a] || centre_of_front(a).is_some() && front.iter().enumerate().any(|(i, f)| *f && d0[i] != u32::MAX && ((i % pw) as f32 - (a % pw) as f32).hypot((i / pw) as f32 - (a / pw) as f32) <= r), front[b] || centre_of_front(b).is_some());
        let a_to = if a_open { centre_of_front(a).unwrap_or(a) } else { a };
        let b_to = if b_open { centre_of_front(b).unwrap_or(b) } else { b };
        // Down the middle: cells far from the silhouette are cheap.
        let dmax = width.max(1.0);
        let cost: Vec<f32> = (0..pw * ph).map(|i| 1.0 + 12.0 * (1.0 - (d_on[i] / dmax).min(1.0)).powi(2)).collect();
        let Some(path) = cheapest(&gap, &cost, pw, ph, a_to, b_to) else { continue };
        let mut pts: Vec<[f64; 2]> = path.iter().map(|&c| [(c % pw) as f64, (c / pw) as f64]).collect();
        pts = simplify(&pts, 0.8);
        let span = (pts[0][0] - pts[pts.len() - 1][0]).hypot(pts[0][1] - pts[pts.len() - 1][1]);
        if pts.len() < 2 || span < (opts.min_length * 2.0 * width.max(2.0) as f64).max(3.0) {
            continue;
        }
        // Past an open end, to clear the silhouette.
        let reach_out = (r as f64) * 0.5;
        let extend = |pts: &mut Vec<[f64; 2]>, at_start: bool| {
            let n = pts.len();
            let (p, q) = if at_start { (pts[0], pts[1]) } else { (pts[n - 1], pts[n - 2]) };
            let (dx, dy) = (p[0] - q[0], p[1] - q[1]);
            let l = (dx * dx + dy * dy).sqrt().max(1e-9);
            let e = [p[0] + dx / l * reach_out, p[1] + dy / l * reach_out];
            if at_start { pts.insert(0, e) } else { pts.push(e) }
        };
        if pts.len() >= 2 {
            if a_open {
                extend(&mut pts, true);
            }
            if b_open {
                extend(&mut pts, false);
            }
        }
        // Deep end first, as a cut is drawn from the notch outwards.
        if (a_open && !b_open) && pts.len() >= 2 {
            pts.reverse();
        }
        let norm: Vec<[f64; 2]> = pts.iter().map(|p| [(p[0] - pad as f64 + 0.5) / w as f64, (p[1] - pad as f64 + 0.5) / h as f64]).collect();
        let confidence = (len as f64 / (4.0 * width.max(2.0) as f64)).clamp(0.2, 1.0);
        out.push(Suggestion { kind: Kind::Gap, name: "gap".into(), points: norm, fill: None, confidence, between: None });
    }
    out.sort_by(|a, b| b.confidence.total_cmp(&a.confidence));
    out
}

// -------------------------------------------------------------- mouth

/// Where a mouth is, in 0..1 picture coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mouth {
    /// The two corners (either order).
    pub corners: [[f64; 2]; 2],
    /// A point on the upper and lower lip at the middle, when known.
    pub lips: Option<[[f64; 2]; 2]>,
}

/// The inside of a mouth (sRGB, straight alpha).
pub const MOUTH_INSIDE: [f64; 4] = [0.24, 0.04, 0.05, 1.0];

/// How much of the mouth's width at each end stays joined.
const CORNER_KEEP: f64 = 0.1;

/// The line of the lips as a cut. With a picture (`luma`, `w × h`, 0..1),
/// the line is snapped to the darkest row near the landmarks between the
/// corners, which is where drawn lips close; otherwise it joins the
/// corners through the lip points (a smile curves up at the ends).
pub fn mouth(m: &Mouth, luma: Option<(&[f32], usize, usize)>) -> Suggestion {
    let [mut a, mut b] = m.corners;
    if a[0] > b[0] {
        std::mem::swap(&mut a, &mut b);
    }
    let width = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt().max(1e-6);
    let mut centre_y = m.lips.map_or((a[1] + b[1]) * 0.5, |l| (l[0][1] + l[1][1]) * 0.5);
    let mut confidence = 0.5;
    let (mut ax, mut bx) = (a[0], b[0]);
    if let Some((px, w, h)) = luma.filter(|(px, w, h)| *w > 2 && *h > 2 && px.len() >= w * h) {
        // The drawn line: the row near the landmarks with the longest run
        // of pixels darker than the skin around it (eyes are too short to
        // count).
        let width_px = width * w as f64;
        let reach = (width * 0.55 * h as f64).max(2.0);
        let ry0 = (centre_y * h as f64 - reach).max(0.0) as usize;
        let ry1 = (((centre_y * h as f64 + reach).ceil()) as usize).min(h - 1);
        let lo = ((a[0] - width * 0.5) * w as f64).max(0.0) as usize;
        let hi = ((((b[0] + width * 0.5) * w as f64).ceil()) as usize).min(w - 1);
        if ry1 > ry0 && hi > lo {
            let mut skin: Vec<f32> = (ry0..=ry1).flat_map(|y| (lo..=hi).map(move |x| px[y * w + x])).collect();
            skin.sort_by(|p, q| p.total_cmp(q));
            let thr = skin[skin.len() / 2] - 0.2;
            let run_in = |y: usize| -> (usize, usize) {
                let (mut best, mut start, mut cur) = (0usize, lo, 0usize);
                for x in lo..=hi {
                    if px[y * w + x] <= thr {
                        cur += 1;
                        if cur > best {
                            best = cur;
                            start = x + 1 - cur;
                        }
                    } else {
                        cur = 0;
                    }
                }
                (best, start)
            };
            let rows: Vec<(usize, usize)> = (ry0..=ry1).map(run_in).collect();
            let near = |i: usize| ((ry0 + i) as f64 - centre_y * h as f64).abs();
            let best = (0..rows.len()).filter(|&i| rows[i].0 as f64 >= width_px * 0.4).max_by(|&p, &q| rows[p].0.cmp(&rows[q].0).then(near(q).total_cmp(&near(p))));
            if let Some(i) = best {
                let (len, start) = rows[i];
                // The rows of the line (an open mouth is several).
                let overlaps = |j: usize| rows[j].0 * 2 >= len && rows[j].1 < start + len && start < rows[j].1 + rows[j].0;
                let (mut top, mut bottom) = (i, i);
                while top > 0 && overlaps(top - 1) {
                    top -= 1;
                }
                while bottom + 1 < rows.len() && overlaps(bottom + 1) {
                    bottom += 1;
                }
                centre_y = ((ry0 + top + ry0 + bottom + 1) as f64 * 0.5) / h as f64;
                ax = start as f64 / w as f64;
                bx = (start + len) as f64 / w as f64;
                confidence = (0.6 + (len as f64 / width_px).min(1.0) * 0.4).min(1.0);
            }
        }
    }
    let (ay, by) = if luma.is_some() && confidence > 0.55 { (centre_y, centre_y) } else { (a[1], b[1]) };
    // Stop short of the corners.
    let keep = (bx - ax) * CORNER_KEEP;
    let (sx, ex) = (ax + keep, bx - keep);
    let at = |t: f64| -> [f64; 2] {
        let x = sx + (ex - sx) * t;
        let line_y = ay + (by - ay) * ((x - ax) / (bx - ax).max(1e-9));
        // The middle follows the lips, the ends the corners.
        let k = 1.0 - (2.0 * t - 1.0).powi(2);
        [x, line_y + (centre_y - (ay + by) * 0.5) * k]
    };
    let points = (0..=4).map(|i| at(i as f64 / 4.0)).collect();
    Suggestion { kind: Kind::Mouth, name: "mouth".into(), points, fill: Some(MOUTH_INSIDE), confidence, between: None }
}

// ----------------------------------------------------- part boundaries

/// The borders between parts of a labelled picture: `labels` is `w × h`,
/// each cell the part it belongs to (`0` = no part). Borders between two
/// parts, trimmed near each of `joints` (a point and a radius, 0..1
/// picture coordinates, in units of the picture's width) so a limb stays
/// attached where it turns, come back as cuts. Borders shorter than
/// `min_length` (0..1 of the longer side) are dropped.
pub fn boundaries(labels: &[u32], w: usize, h: usize, joints: &[([f64; 2], f64)], min_length: f64) -> Vec<Suggestion> {
    if w < 2 || h < 2 || labels.len() < w * h {
        return Vec::new();
    }
    // Lattice vertices are (x, y) corners; an edge between two cells with
    // different non-zero labels is a unit segment on the lattice.
    type V = (i32, i32);
    let mut edges: HashMap<(u32, u32), Vec<(V, V)>> = HashMap::new();
    for y in 0..h {
        for x in 0..w {
            let a = labels[y * w + x];
            if a == 0 {
                continue;
            }
            if x + 1 < w {
                let b = labels[y * w + x + 1];
                if b != 0 && b != a {
                    edges.entry((a.min(b), a.max(b))).or_default().push(((x as i32 + 1, y as i32), (x as i32 + 1, y as i32 + 1)));
                }
            }
            if y + 1 < h {
                let b = labels[(y + 1) * w + x];
                if b != 0 && b != a {
                    edges.entry((a.min(b), a.max(b))).or_default().push(((x as i32, y as i32 + 1), (x as i32 + 1, y as i32 + 1)));
                }
            }
        }
    }
    let long = w.max(h) as f64;
    let mut out = Vec::new();
    let mut keys: Vec<_> = edges.keys().copied().collect();
    keys.sort_unstable();
    for key in keys {
        let segs = &edges[&key];
        let mut adj: HashMap<V, Vec<V>> = HashMap::new();
        for &(p, q) in segs {
            adj.entry(p).or_default().push(q);
            adj.entry(q).or_default().push(p);
        }
        let mut used: std::collections::HashSet<(V, V)> = std::collections::HashSet::new();
        let norm = |p: V, q: V| if p <= q { (p, q) } else { (q, p) };
        let mut starts: Vec<V> = adj.iter().filter(|(_, n)| n.len() != 2).map(|(p, _)| *p).collect();
        starts.sort_unstable();
        // Then loops, which have no ends.
        let mut all: Vec<V> = adj.keys().copied().collect();
        all.sort_unstable();
        starts.extend(all);
        for s in starts {
            for first in adj[&s].clone() {
                if used.contains(&norm(s, first)) {
                    continue;
                }
                let mut chain = vec![s];
                let (mut prev, mut cur) = (s, first);
                used.insert(norm(prev, cur));
                chain.push(cur);
                while adj[&cur].len() == 2 {
                    let next = *adj[&cur].iter().find(|&&n| n != prev).unwrap_or(&prev);
                    if used.contains(&norm(cur, next)) {
                        break;
                    }
                    used.insert(norm(cur, next));
                    prev = cur;
                    cur = next;
                    chain.push(cur);
                }
                let pts: Vec<[f64; 2]> = chain.iter().map(|&(x, y)| [x as f64 / w as f64, y as f64 / h as f64]).collect();
                // Cut off what lies at a joint.
                let mut runs: Vec<Vec<[f64; 2]>> = vec![Vec::new()];
                for p in simplify_keep(&pts, 0.75 / long) {
                    let near = joints.iter().any(|(j, rad)| ((p[0] - j[0]).powi(2) + ((p[1] - j[1]) * h as f64 / w as f64).powi(2)).sqrt() < *rad);
                    if near {
                        if !runs.last().unwrap().is_empty() {
                            runs.push(Vec::new());
                        }
                    } else {
                        runs.last_mut().unwrap().push(p);
                    }
                }
                for run in runs {
                    let len: f64 = run.windows(2).map(|s| ((s[1][0] - s[0][0]) * w as f64).hypot((s[1][1] - s[0][1]) * h as f64)).sum::<f64>() / long;
                    if run.len() >= 2 && len >= min_length {
                        out.push(Suggestion { kind: Kind::Part, name: format!("part {}/{}", key.0, key.1), points: run, fill: None, confidence: (len * 4.0).clamp(0.3, 0.9), between: Some(key) });
                    }
                }
            }
        }
    }
    out
}

/// [`simplify`] that also keeps points at least a joint-test apart, so a
/// long straight border can still be trimmed at a joint in its middle.
fn simplify_keep(pts: &[[f64; 2]], eps: f64) -> Vec<[f64; 2]> {
    let s = simplify(pts, eps);
    let mut out = Vec::with_capacity(s.len());
    for seg in s.windows(2) {
        let l = ((seg[1][0] - seg[0][0]).powi(2) + (seg[1][1] - seg[0][1]).powi(2)).sqrt();
        let n = ((l / 0.01).ceil() as usize).max(1);
        for k in 0..n {
            let t = k as f64 / n as f64;
            out.push([seg[0][0] + (seg[1][0] - seg[0][0]) * t, seg[0][1] + (seg[1][1] - seg[0][1]) * t]);
        }
    }
    if let Some(l) = s.last() {
        out.push(*l);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(w: usize, h: usize, f: impl Fn(usize, usize) -> bool) -> Vec<bool> {
        (0..w * h).map(|i| f(i % w, i / w)).collect()
    }

    /// A body with two legs 3 cells apart, crotch at y = 50.
    fn legs(gap: (usize, usize)) -> (usize, usize, Vec<bool>) {
        let (w, h) = (96, 128);
        let on = grid(w, h, |x, y| {
            let body = (28..68).contains(&x) && (10..50).contains(&y);
            let legs = (28..68).contains(&x) && !(gap.0..gap.1).contains(&x) && (50..120).contains(&y);
            body || legs
        });
        (w, h, on)
    }

    #[test]
    fn finds_the_gap_between_two_legs() {
        let (w, h, on) = legs((46, 49));
        let cuts = gaps(Mask { w, h, on: &on }, GapOptions::default());
        assert_eq!(cuts.len(), 1, "{cuts:?}");
        let c = &cuts[0];
        assert_eq!(c.kind, Kind::Gap);
        // In the gap's columns, from the crotch down past the feet.
        let (top, bottom) = (c.points[0], *c.points.last().unwrap());
        assert!(c.points.iter().all(|p| p[0] * (w as f64) > 45.0 && p[0] * (w as f64) < 50.0), "{:?}", c.points);
        assert!((top[1] * h as f64 - 50.0).abs() < 4.0, "starts at the crotch: {top:?}");
        assert!(bottom[1] * h as f64 > 100.0, "ends where the legs end: {bottom:?}");
    }

    #[test]
    fn finds_the_notch_of_legs_that_are_apart() {
        // A walking stance: the legs diverge from the crotch.
        let (w, h) = (96, 128);
        let on = grid(w, h, |x, y| {
            let body = (30..66).contains(&x) && (10..50).contains(&y);
            let t = y.saturating_sub(50) as f64 / 70.0;
            let left = y >= 50 && y < 120 && (x as f64) >= 30.0 - 14.0 * t && (x as f64) < 46.0 - 14.0 * t + 0.0;
            let right = y >= 50 && y < 120 && (x as f64) >= 49.0 + 14.0 * t && (x as f64) < 66.0 + 14.0 * t;
            body || left || right
        });
        let cuts = gaps(Mask { w, h, on: &on }, GapOptions::default());
        assert!(!cuts.is_empty());
        let c = &cuts[0];
        assert!((c.points[0][1] * h as f64 - 50.0).abs() < 6.0, "the apex is the crotch: {:?}", c.points[0]);
        assert!((c.points[0][0] * w as f64 - 47.5).abs() < 4.0);
    }

    #[test]
    fn finds_the_slit_under_an_arm() {
        // An arm hanging beside the torso, 2 cells away.
        let (w, h) = (96, 128);
        let on = grid(w, h, |x, y| {
            let torso = (30..60).contains(&x) && (10..100).contains(&y);
            let arm = (61 + 2..75).contains(&x) && (24..80).contains(&y);
            let shoulder = (60..75).contains(&x) && (20..30).contains(&y);
            torso || arm || shoulder
        });
        let cuts = gaps(Mask { w, h, on: &on }, GapOptions::default());
        assert_eq!(cuts.len(), 1, "{cuts:?}");
        let c = &cuts[0];
        assert!(c.points.iter().all(|p| (p[0] * w as f64 - 61.5).abs() < 3.0), "{:?}", c.points);
        assert!((c.points[0][1] * h as f64 - 30.0).abs() < 5.0, "starts at the armpit: {:?}", c.points[0]);
    }

    #[test]
    fn leaves_a_plain_shape_and_a_hole_alone() {
        let (w, h) = (96, 96);
        let blob = grid(w, h, |x, y| (x as f64 - 48.0).hypot(y as f64 - 48.0) < 30.0);
        assert!(gaps(Mask { w, h, on: &blob }, GapOptions::default()).is_empty());
        let ring = grid(w, h, |x, y| {
            let d = (x as f64 - 48.0).hypot(y as f64 - 48.0);
            d < 36.0 && d > 8.0
        });
        assert!(gaps(Mask { w, h, on: &ring }, GapOptions::default()).is_empty(), "a hole is not a gap");
        assert!(gaps(Mask { w, h, on: &[] }, GapOptions::default()).is_empty());
    }

    #[test]
    fn a_wide_gap_is_not_a_cut() {
        let (w, h, on) = legs((36, 60));
        let found = gaps(Mask { w, h, on: &on }, GapOptions::default());
        assert!(found.is_empty(), "legs far apart are already apart: {found:?}");
    }

    /// A face: skin disc, two eye dots and a dark mouth line at `row`.
    fn face(row: usize) -> (usize, usize, Vec<f32>) {
        let (w, h) = (80, 80);
        let mut px = vec![1.0f32; w * h];
        for y in 0..h {
            for x in 0..w {
                let d = (x as f64 - 40.0).hypot(y as f64 - 40.0);
                px[y * w + x] = if d < 34.0 { 0.8 } else { 1.0 };
                if (x as f64 - 28.0).hypot(y as f64 - 30.0) < 3.0 || (x as f64 - 52.0).hypot(y as f64 - 30.0) < 3.0 {
                    px[y * w + x] = 0.1;
                }
                if (y == row || y == row + 1) && (28..52).contains(&x) {
                    px[y * w + x] = 0.2;
                }
            }
        }
        (w, h, px)
    }

    #[test]
    fn finds_the_mouth_line_on_a_face() {
        let (w, h, px) = face(56);
        // Landmarks a few cells off (a detector's estimate).
        let m = Mouth { corners: [[27.0 / 80.0, 53.0 / 80.0], [53.0 / 80.0, 53.0 / 80.0]], lips: None };
        let cut = mouth(&m, Some((&px, w, h)));
        assert_eq!(cut.kind, Kind::Mouth);
        assert!(cut.fill.is_some());
        for p in &cut.points {
            assert!((p[1] * h as f64 - 57.0).abs() < 1.6, "on the drawn line: {p:?}");
        }
        // Inside the corners, so they stay joined.
        let (a, b) = (cut.points[0][0] * w as f64, cut.points.last().unwrap()[0] * w as f64);
        assert!(a > 28.0 && b < 52.0 && b - a > 14.0, "{a} {b}");
        assert!(cut.confidence > 0.6);
    }

    #[test]
    fn the_mouth_without_a_picture_follows_the_landmarks() {
        let m = Mouth { corners: [[0.6, 0.7], [0.4, 0.68]], lips: Some([[0.5, 0.67], [0.5, 0.73]]) };
        let cut = mouth(&m, None);
        assert_eq!(cut.points.len(), 5);
        assert!(cut.points[0][0] > 0.4 && cut.points[4][0] < 0.6);
        // The middle sits on the lips' middle.
        assert!((cut.points[2][1] - 0.70).abs() < 0.02, "{:?}", cut.points[2]);
        assert!(cut.points[0][0] < cut.points[4][0]);
    }

    #[test]
    fn part_borders_become_cuts_trimmed_at_joints() {
        // Torso left, arm right of x = 30; border along x = 30 from y = 10 to 70.
        let (w, h) = (60, 80);
        let labels: Vec<u32> = (0..w * h).map(|i| { let (x, y) = (i % w, i / w); if !(10..70).contains(&y) || !(10..50).contains(&x) { 0 } else if x < 30 { 1 } else { 2 } }).collect();
        let cuts = boundaries(&labels, w, h, &[], 0.05);
        assert_eq!(cuts.len(), 1, "{cuts:?}");
        let c = &cuts[0];
        assert!(c.points.iter().all(|p| (p[0] * w as f64 - 30.0).abs() < 0.01));
        let (lo, hi) = c.points.iter().fold((f64::MAX, f64::MIN), |a, p| (a.0.min(p[1]), a.1.max(p[1])));
        assert!((lo * h as f64 - 10.0).abs() < 0.5 && (hi * h as f64 - 70.0).abs() < 0.5);
        // The shoulder joint at the top keeps the arm attached.
        let trimmed = boundaries(&labels, w, h, &[([0.5, 10.0 / 80.0], 0.12)], 0.05);
        assert_eq!(trimmed.len(), 1);
        let top = trimmed[0].points.iter().map(|p| p[1]).fold(f64::MAX, f64::min);
        assert!(top * h as f64 > 15.0, "the joint end is cut off: {top}");
    }
}
