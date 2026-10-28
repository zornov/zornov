// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Skin weights: how much each bone carries each point of a shape (a
//! vertex of a 3D model, a node of the mesh over a 2D cut-out), on any
//! graph of points joined by edges.
//!
//! - [`auto`]: automatic weights measured *through the shape* (geodesic
//!   distance along the graph), so an arm resting against the body does
//!   not pull the body, blended softly where two bones are about as near.
//! - [`smooth`], [`Weights::normalise`], [`Weights::limit`]: the clean-up
//!   passes (soft joints, rows summing to one, at most four bones a
//!   point).
//! - [`paint`]: brushes (add, subtract, replace, smooth) for weight
//!   painting, keeping each row summing to one and locked bones as they
//!   are.
//! - [`mirror`]: one side's weights onto the other.

use crate::v::{self, P};
use std::cmp::Ordering;
use std::collections::BinaryHeap;

/// Points joined by edges (each edge listed both ways, with its length).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Graph {
    pub edges: Vec<Vec<(usize, f64)>>,
}

impl Graph {
    /// The edges of triangles over `pos`.
    pub fn from_triangles<const N: usize>(pos: &[P<N>], tris: &[[u32; 3]]) -> Graph {
        let mut edges: Vec<Vec<(usize, f64)>> = vec![Vec::new(); pos.len()];
        let mut join = |a: usize, b: usize| {
            if a < pos.len() && b < pos.len() && a != b && !edges[a].iter().any(|e| e.0 == b) {
                let d = v::dist(pos[a], pos[b]);
                edges[a].push((b, d));
                edges[b].push((a, d));
            }
        };
        for t in tris {
            join(t[0] as usize, t[1] as usize);
            join(t[1] as usize, t[2] as usize);
            join(t[2] as usize, t[0] as usize);
        }
        Graph { edges }
    }

    /// A grid of `(nx + 1) × (ny + 1)` nodes (`j * (nx + 1) + i`), cells
    /// `cell_w × cell_h`, joined across the cells where `on(i, j)` says
    /// the cell is part of the shape (diagonals too).
    pub fn grid(nx: usize, ny: usize, cell_w: f64, cell_h: f64, on: impl Fn(usize, usize) -> bool) -> Graph {
        let idx = |i: usize, j: usize| j * (nx + 1) + i;
        let mut edges: Vec<Vec<(usize, f64)>> = vec![Vec::new(); (nx + 1) * (ny + 1)];
        let diag = (cell_w * cell_w + cell_h * cell_h).sqrt();
        let mut join = |a: usize, b: usize, d: f64| {
            if !edges[a].iter().any(|e| e.0 == b) {
                edges[a].push((b, d));
                edges[b].push((a, d));
            }
        };
        for j in 0..ny {
            for i in 0..nx {
                if !on(i, j) {
                    continue;
                }
                let (a, b, c, d) = (idx(i, j), idx(i + 1, j), idx(i, j + 1), idx(i + 1, j + 1));
                join(a, b, cell_w);
                join(c, d, cell_w);
                join(a, c, cell_h);
                join(b, d, cell_h);
                join(a, d, diag);
                join(b, c, diag);
            }
        }
        Graph { edges }
    }

    pub fn len(&self) -> usize {
        self.edges.len()
    }

    pub fn is_empty(&self) -> bool {
        self.edges.is_empty()
    }
}

#[derive(PartialEq)]
struct Item(f64, usize);
impl Eq for Item {}
impl Ord for Item {
    fn cmp(&self, o: &Self) -> Ordering {
        o.0.total_cmp(&self.0).then(self.1.cmp(&o.1))
    }
}
impl PartialOrd for Item {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

/// Distance along the graph from the nearest seed (`(node, starting
/// distance)`); unreachable nodes stay infinite.
pub fn geodesic(g: &Graph, seeds: &[(usize, f64)]) -> Vec<f64> {
    let mut d = vec![f64::INFINITY; g.len()];
    let mut heap = BinaryHeap::new();
    for &(n, d0) in seeds {
        if n < d.len() && d0 < d[n] {
            d[n] = d0;
            heap.push(Item(d0, n));
        }
    }
    while let Some(Item(dn, n)) = heap.pop() {
        if dn > d[n] {
            continue;
        }
        for &(m, w) in &g.edges[n] {
            let nd = dn + w;
            if nd < d[m] {
                d[m] = nd;
                heap.push(Item(nd, m));
            }
        }
    }
    d
}

/// Distance from `p` to the segment `a`-`b`.
pub fn to_segment<const N: usize>(p: P<N>, a: P<N>, b: P<N>) -> f64 {
    let ab = v::sub(b, a);
    let l2 = v::dot(ab, ab);
    let t = if l2 > 1e-24 { (v::dot(v::sub(p, a), ab) / l2).clamp(0.0, 1.0) } else { 0.0 };
    v::dist(p, v::add(a, v::scale(ab, t)))
}

/// Weights of every point for every bone, row by row (`nodes × bones`).
#[derive(Clone, Debug, PartialEq)]
pub struct Weights {
    pub bones: usize,
    pub w: Vec<f32>,
}

impl Weights {
    /// All zero.
    pub fn new(nodes: usize, bones: usize) -> Weights {
        Weights { bones, w: vec![0.0; nodes * bones] }
    }

    pub fn nodes(&self) -> usize {
        self.w.len().checked_div(self.bones).unwrap_or(0)
    }

    pub fn row(&self, i: usize) -> &[f32] {
        &self.w[i * self.bones..(i + 1) * self.bones]
    }

    pub fn row_mut(&mut self, i: usize) -> &mut [f32] {
        &mut self.w[i * self.bones..(i + 1) * self.bones]
    }

    pub fn get(&self, node: usize, bone: usize) -> f32 {
        self.w.get(node * self.bones + bone).copied().unwrap_or(0.0)
    }

    /// Every row summing to one (rows of all zero stay so).
    pub fn normalise(&mut self) {
        for i in 0..self.nodes() {
            let r = self.row_mut(i);
            let s: f32 = r.iter().sum();
            if s > 1e-9 {
                r.iter_mut().for_each(|x| *x /= s);
            }
        }
    }

    /// At most `n` bones a point (the strongest), then normalised; weights
    /// under `floor` dropped.
    pub fn limit(&mut self, n: usize, floor: f32) {
        for i in 0..self.nodes() {
            let r = self.row_mut(i);
            let mut order: Vec<usize> = (0..r.len()).collect();
            order.sort_by(|&a, &b| r[b].total_cmp(&r[a]));
            for (k, &b) in order.iter().enumerate() {
                if k >= n || r[b] < floor {
                    r[b] = 0.0;
                }
            }
        }
        self.normalise();
    }

    /// A point's four strongest bones and their weights (summing to one),
    /// as 3D skins store them.
    pub fn top4(&self, i: usize) -> ([u16; 4], [f32; 4]) {
        let r = self.row(i);
        let mut order: Vec<usize> = (0..r.len()).collect();
        order.sort_by(|&a, &b| r[b].total_cmp(&r[a]));
        let mut j = [0u16; 4];
        let mut w = [0f32; 4];
        for k in 0..4.min(order.len()) {
            j[k] = order[k] as u16;
            w[k] = r[order[k]];
        }
        let s: f32 = w.iter().sum();
        if s > 1e-9 {
            w.iter_mut().for_each(|x| *x /= s);
        } else if !order.is_empty() {
            w[0] = 1.0;
        }
        (j, w)
    }

    /// From four bones a point (3D skins).
    pub fn from_top4(bones: usize, joints: &[[u16; 4]], weights: &[[f32; 4]]) -> Weights {
        let mut out = Weights::new(joints.len(), bones);
        for (i, (j, w)) in joints.iter().zip(weights).enumerate() {
            for k in 0..4 {
                if (j[k] as usize) < bones {
                    out.w[i * bones + j[k] as usize] += w[k];
                }
            }
        }
        out
    }
}

/// Automatic weights: each point weighed by how far it is, through the
/// shape, from each bone (`(head, tail)`), against the nearest bone, with
/// `falloff` the distance over which two bones blend. Points the graph
/// cannot reach from any bone go to the nearest bone in a straight line.
pub fn auto<const N: usize>(g: &Graph, pos: &[P<N>], bones: &[(P<N>, P<N>)], falloff: f64) -> Weights {
    from_distances(&bone_distances(g, pos, bones, f64::INFINITY), falloff, 0.0)
}

/// For each bone (`(head, tail)`), every point's distance from it through
/// the shape: seeded where the surface comes nearest the bone (at the
/// straight distance there), then along the graph's edges, stopping past
/// `reach` (`f64::INFINITY`: everywhere). A point cut off from a bone, or
/// past the reach, is far: four times its straight distance plus 10⁶, so
/// among bones all cut off the nearest in a straight line still wins.
/// Bones are worked out in parallel.
pub fn bone_distances<const N: usize>(g: &Graph, pos: &[P<N>], bones: &[(P<N>, P<N>)], reach: f64) -> Vec<Vec<f64>> {
    let n = pos.len().min(g.len());
    if n == 0 || bones.is_empty() {
        return vec![Vec::new(); bones.len()];
    }
    // A typical edge: points within it of a bone sit on the bone.
    let mut lens: Vec<f64> = g.edges.iter().flat_map(|e| e.iter().map(|x| x.1)).collect();
    lens.sort_by(f64::total_cmp);
    let edge = lens.get(lens.len() / 2).copied().unwrap_or(1.0).max(1e-9);
    let one = |&(a, b): &(P<N>, P<N>)| -> Vec<f64> {
        let straight: Vec<f64> = pos.iter().take(n).map(|&p| to_segment(p, a, b)).collect();
        let near = straight.iter().copied().fold(f64::INFINITY, f64::min);
        let seeds: Vec<(usize, f64)> = straight.iter().enumerate().filter(|(_, d)| **d <= near + edge * 1.5).map(|(i, d)| (i, *d)).collect();
        let mut d = geodesic_within(g, &seeds, reach);
        d.truncate(n);
        for (x, s) in d.iter_mut().zip(&straight) {
            if !x.is_finite() {
                *x = s * 4.0 + 1e6;
            }
        }
        d
    };
    let threads = std::thread::available_parallelism().map_or(4, |t| t.get()).clamp(1, 16).min(bones.len());
    let mut out: Vec<Vec<f64>> = vec![Vec::new(); bones.len()];
    let next = std::sync::atomic::AtomicUsize::new(0);
    let slots: Vec<std::sync::Mutex<&mut Vec<f64>>> = out.iter_mut().map(std::sync::Mutex::new).collect();
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| {
                loop {
                    let k = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    if k >= bones.len() {
                        break;
                    }
                    let d = one(&bones[k]);
                    **slots[k].lock().unwrap_or_else(|e| e.into_inner()) = d;
                }
            });
        }
    });
    out
}

/// [`geodesic`], stopping once the distance passes `reach` (points beyond
/// stay infinite).
pub fn geodesic_within(g: &Graph, seeds: &[(usize, f64)], reach: f64) -> Vec<f64> {
    let mut d = vec![f64::INFINITY; g.len()];
    let mut heap = BinaryHeap::new();
    for &(n, d0) in seeds {
        if n < d.len() && d0 < d[n] {
            d[n] = d0;
            heap.push(Item(d0, n));
        }
    }
    while let Some(Item(dn, n)) = heap.pop() {
        if dn > d[n] || dn > reach {
            continue;
        }
        for &(m, w) in &g.edges[n] {
            let nd = dn + w;
            if nd < d[m] && nd <= reach {
                d[m] = nd;
                heap.push(Item(nd, m));
            }
        }
    }
    d
}

/// Weights from each bone's distances (`fields[bone][point]`, as
/// [`bone_distances`] gives them): a point's bone weighs
/// `exp(-((d - nearest) / σ)²)` with `σ = falloff + relative × nearest`,
/// so a joint deep in a thick torso blends over a wide band and a finger
/// next to its neighbour over a narrow one. Rows sum to one.
pub fn from_distances(fields: &[Vec<f64>], falloff: f64, relative: f64) -> Weights {
    let n = fields.iter().map(Vec::len).min().unwrap_or(0);
    let mut out = Weights::new(n, fields.len());
    for i in 0..n {
        let near = fields.iter().map(|f| f[i]).fold(f64::INFINITY, f64::min);
        // A point far from every bone measures by its straight distance
        // (see `bone_distances`), not the offset that marks it far.
        let base = if near >= 1e6 { (near - 1e6) / 4.0 } else { near };
        let sigma = (falloff + relative * base.max(0.0)).max(1e-9);
        let r = out.row_mut(i);
        for (b, f) in fields.iter().enumerate() {
            r[b] = (-((f[i] - near) / sigma).powi(2)).exp() as f32;
        }
    }
    out.normalise();
    out
}

/// Evens the weights out over the graph `iterations` times (`amount`
/// 0..1 a pass): softer joints. Locked bones keep their weights.
pub fn smooth(g: &Graph, w: &mut Weights, iterations: usize, amount: f32, locked: &[bool]) {
    let nb = w.bones;
    for _ in 0..iterations {
        let before = w.clone();
        for i in 0..w.nodes().min(g.len()) {
            if g.edges[i].is_empty() {
                continue;
            }
            let mut avg = vec![0f32; nb];
            let mut total = 0f32;
            for &(j, d) in &g.edges[i] {
                let k = 1.0 / (d as f32).max(1e-6);
                for (b, a) in avg.iter_mut().enumerate() {
                    *a += before.get(j, b) * k;
                }
                total += k;
            }
            let lock_sum: f32 = (0..nb).filter(|&b| locked.get(b).copied().unwrap_or(false)).map(|b| before.get(i, b)).sum();
            let r = w.row_mut(i);
            for (b, a) in avg.iter().enumerate() {
                if !locked.get(b).copied().unwrap_or(false) {
                    r[b] += (a / total - r[b]) * amount;
                }
            }
            // The free bones share what the locked ones leave.
            let free: f32 = (0..nb).filter(|&b| !locked.get(b).copied().unwrap_or(false)).map(|b| r[b]).sum();
            if free > 1e-9 {
                let k = (1.0 - lock_sum).max(0.0) / free;
                for (b, x) in r.iter_mut().enumerate() {
                    if !locked.get(b).copied().unwrap_or(false) {
                        *x *= k;
                    }
                }
            }
        }
    }
}

/// A weight-painting brush.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Brush {
    /// More of the bone.
    Add,
    /// Less of it.
    Subtract,
    /// Toward the brush's strength exactly.
    Replace,
    /// Toward the neighbours' average.
    Smooth,
}

impl Brush {
    pub fn parse(s: &str) -> Option<Brush> {
        Some(match s.to_ascii_lowercase().as_str() {
            "add" | "paint" | "more" => Brush::Add,
            "subtract" | "remove" | "erase" | "less" => Brush::Subtract,
            "replace" | "set" => Brush::Replace,
            "smooth" | "blur" => Brush::Smooth,
            _ => return None,
        })
    }
}

/// One dab of a brush on `bone`: each `(node, falloff 0..1)` changes by
/// `strength × falloff`; the row's other free bones make room (or fill
/// in) so it still sums to one. Taking weight from a point that only
/// this bone carries hands it to the bone its neighbours lean on most.
pub fn paint(g: &Graph, w: &mut Weights, bone: usize, dabs: &[(usize, f64)], brush: Brush, strength: f64, locked: &[bool]) {
    let nb = w.bones;
    if bone >= nb || locked.get(bone).copied().unwrap_or(false) {
        return;
    }
    let before = w.clone();
    for &(i, fall) in dabs {
        if i >= w.nodes() {
            continue;
        }
        let a = (strength * fall).clamp(0.0, 1.0) as f32;
        let old = before.get(i, bone);
        let lock_sum: f32 = (0..nb).filter(|&b| b != bone && locked.get(b).copied().unwrap_or(false)).map(|b| before.get(i, b)).sum();
        let room = (1.0 - lock_sum).max(0.0);
        let new = match brush {
            Brush::Add => old + a * (1.0 - old),
            Brush::Subtract => old - a * old,
            Brush::Replace => old + (strength as f32 - old) * (fall as f32).clamp(0.0, 1.0),
            Brush::Smooth => {
                let e = g.edges.get(i).map(|e| e.as_slice()).unwrap_or(&[]);
                if e.is_empty() {
                    old
                } else {
                    let avg = e.iter().map(|&(j, _)| before.get(j, bone)).sum::<f32>() / e.len() as f32;
                    old + (avg - old) * a
                }
            }
        }
        .clamp(0.0, room);
        let free: Vec<usize> = (0..nb).filter(|&b| b != bone && !locked.get(b).copied().unwrap_or(false)).collect();
        let others: f32 = free.iter().map(|&b| before.get(i, b)).sum();
        let r = w.row_mut(i);
        r[bone] = new;
        let left = (room - new).max(0.0);
        if others > 1e-9 {
            for &b in &free {
                r[b] = before.get(i, b) / others * left;
            }
        } else if left > 1e-9 {
            // Nobody else here: the bone the neighbours lean on most.
            let mut votes = vec![0f32; nb];
            for &(j, _) in g.edges.get(i).map(|e| e.as_slice()).unwrap_or(&[]) {
                for &b in &free {
                    votes[b] += before.get(j, b);
                }
            }
            match free.iter().copied().max_by(|&a, &b| votes[a].total_cmp(&votes[b])).filter(|&b| votes[b] > 0.0) {
                Some(b) => r[b] = left,
                // No one to hand it to: the bone keeps it.
                None => r[bone] = room,
            }
        }
    }
}

/// Weights mirrored: point `i` takes the weights of point `point_map[i]`
/// (its mirror image), each bone's weight going to `bone_map[bone]`.
pub fn mirror(w: &Weights, point_map: &[usize], bone_map: &[usize]) -> Weights {
    let mut out = Weights::new(point_map.len(), w.bones);
    for (i, &src) in point_map.iter().enumerate() {
        if src >= w.nodes() {
            continue;
        }
        for b in 0..w.bones {
            let to = bone_map.get(b).copied().unwrap_or(b).min(w.bones - 1);
            out.w[i * w.bones + to] += w.get(src, b);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An L of grid cells: a vertical bar (x 0..2, y 0..8) and an arm
    /// from its foot to the right (x 2..8, y 6..8).
    fn ell() -> (Graph, Vec<P<2>>) {
        let (nx, ny) = (8, 8);
        let g = Graph::grid(nx, ny, 1.0, 1.0, |i, j| i < 2 || j >= 6);
        let pos = (0..=ny).flat_map(|j| (0..=nx).map(move |i| [i as f64, j as f64])).collect();
        (g, pos)
    }

    #[test]
    fn automatic_weights_go_through_the_shape_and_sum_to_one() {
        let (g, pos) = ell();
        let bones = [([1.0, 0.0], [1.0, 7.0]), ([1.0, 7.0], [8.0, 7.0])];
        let w = auto(&g, &pos, &bones, 1.0);
        // Rows sum to one.
        for i in 0..w.nodes() {
            let s: f32 = w.row(i).iter().sum();
            assert!((s - 1.0).abs() < 1e-5);
        }
        let at = |x: usize, y: usize| y * 9 + x;
        // The top of the bar is the bar's; the arm's tip the arm's.
        assert!(w.get(at(1, 0), 0) > 0.99 && w.get(at(8, 7), 1) > 0.99);
        // The joint blends.
        let j = w.get(at(2, 7), 0);
        assert!(j > 0.05 && j < 0.95, "{j}");
        // A ring and its graph cut across: the far side of a gap is far.
        let d = geodesic(&g, &[(at(0, 0), 0.0)]);
        assert!(d[at(8, 8)] > 10.0 && !d[at(5, 2)].is_finite());
    }

    #[test]
    fn brushes_paint_keep_rows_whole_and_respect_locks() {
        let (g, pos) = ell();
        let bones = [([1.0, 0.0], [1.0, 7.0]), ([1.0, 7.0], [8.0, 7.0]), ([0.0, 8.0], [0.0, 8.5])];
        let mut w = auto(&g, &pos, &bones, 1.0);
        let at = |x: usize, y: usize| y * 9 + x;
        let n = at(1, 3);
        paint(&g, &mut w, 1, &[(n, 1.0)], Brush::Add, 0.5, &[]);
        assert!(w.get(n, 1) >= 0.49 && (w.row(n).iter().sum::<f32>() - 1.0).abs() < 1e-5);
        // Subtracting the only bone hands it to the neighbours' bone.
        let mut solo = Weights::new(pos.len(), 3);
        for i in 0..pos.len() {
            solo.w[i * 3 + if i == n { 1 } else { 0 }] = 1.0;
        }
        paint(&g, &mut solo, 1, &[(n, 1.0)], Brush::Subtract, 1.0, &[]);
        assert_eq!(solo.get(n, 1), 0.0);
        assert!((solo.get(n, 0) - 1.0).abs() < 1e-6);
        // A locked bone keeps its share.
        let before = w.get(n, 0);
        paint(&g, &mut w, 1, &[(n, 1.0)], Brush::Replace, 0.2, &[true, false, false]);
        assert!((w.get(n, 0) - before).abs() < 1e-6 && (w.row(n).iter().sum::<f32>() - 1.0).abs() < 1e-5);
        // Smoothing and limiting keep rows whole.
        smooth(&g, &mut w, 3, 0.5, &[]);
        w.limit(1, 0.0);
        for i in 0..w.nodes() {
            assert_eq!(w.row(i).iter().filter(|x| **x > 0.0).count(), 1);
        }
        let (j, k) = w.top4(n);
        assert!((k.iter().sum::<f32>() - 1.0).abs() < 1e-6 && w.get(n, j[0] as usize) > 0.99);
        assert_eq!(Weights::from_top4(3, &[j], &[k]).row(0), w.row(n));
        assert_eq!(Brush::parse("erase"), Some(Brush::Subtract));
    }

    #[test]
    fn mirrored_weights_swap_sides() {
        let mut w = Weights::new(2, 2);
        w.w = vec![1.0, 0.0, 0.3, 0.7];
        let m = mirror(&w, &[1, 0], &[1, 0]);
        assert_eq!(m.w, vec![0.7, 0.3, 0.0, 1.0]);
    }
}
