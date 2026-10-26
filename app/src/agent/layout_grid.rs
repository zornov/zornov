// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The raster and polygon arithmetic behind `layout`: no project, no
//! renderer, only coverage grids and boxes, so each rule can be tested
//! with numbers you can check by hand.
//!
//! A [`Grid`] is one item's coverage (its alpha as the renderer drew it
//! alone, 0 to 255) at a small size. Boxes are `[left, top, right,
//! bottom]` in grid pixels with sub-pixel edges.

/// One item's coverage of the frame.
#[derive(Clone, Debug, PartialEq)]
pub struct Grid {
    pub w: usize,
    pub h: usize,
    pub a: Vec<u8>,
}

/// A box: left, top, right, bottom.
pub type Rect = [f32; 4];

/// Alpha at or above this counts as the item being there.
pub const SOLID: u8 = 128;
/// Alpha below this is nothing (rounding noise of the renderer).
pub const FAINT: u8 = 10;

impl Grid {
    pub fn new(w: usize, h: usize) -> Grid {
        Grid { w, h, a: vec![0; w * h] }
    }

    pub fn at(&self, x: usize, y: usize) -> u8 {
        self.a[y * self.w + x]
    }

    /// The covered area in pixels (partial coverage counts partly).
    pub fn area(&self) -> f32 {
        self.a.iter().map(|&v| v as f32).sum::<f32>() / 255.0
    }

    /// The box of the pixels at or above `thr`, its edges placed within a
    /// pixel by how much the outermost pixels are covered: a picture whose
    /// edge falls half way through a pixel gives that half pixel.
    pub fn bounds(&self, thr: u8) -> Option<Rect> {
        let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0usize, 0usize);
        for y in 0..self.h {
            for x in 0..self.w {
                if self.at(x, y) >= thr {
                    x0 = x0.min(x);
                    x1 = x1.max(x);
                    y0 = y0.min(y);
                    y1 = y1.max(y);
                }
            }
        }
        if x0 == usize::MAX {
            return None;
        }
        // The strongest coverage along each outer line, as a fraction.
        let col_max = |x: usize| (0..self.h).map(|y| self.at(x, y)).max().unwrap_or(0) as f32 / 255.0;
        let row_max = |y: usize| (0..self.w).map(|x| self.at(x, y)).max().unwrap_or(0) as f32 / 255.0;
        let (l, r, t, b) = (col_max(x0), col_max(x1), row_max(y0), row_max(y1));
        Some([x0 as f32 + (1.0 - l), y0 as f32 + (1.0 - t), (x1 + 1) as f32 - (1.0 - r), (y1 + 1) as f32 - (1.0 - b)])
    }

    /// Whether the item reaches the left, top, right and bottom edge of
    /// the frame (an edge line with any solid pixel).
    pub fn touches(&self, thr: u8) -> [bool; 4] {
        let col = |x: usize| (0..self.h).any(|y| self.at(x, y) >= thr);
        let row = |y: usize| (0..self.w).any(|x| self.at(x, y) >= thr);
        if self.w == 0 || self.h == 0 {
            return [false; 4];
        }
        [col(0), row(0), col(self.w - 1), row(self.h - 1)]
    }

    /// The share of the covered area (weighted) at `(x, y)`'s centre of
    /// mass: where the item's weight sits.
    pub fn centroid(&self) -> Option<(f32, f32)> {
        let (mut sx, mut sy, mut s) = (0f64, 0f64, 0f64);
        for y in 0..self.h {
            for x in 0..self.w {
                let v = self.at(x, y) as f64;
                sx += v * (x as f64 + 0.5);
                sy += v * (y as f64 + 0.5);
                s += v;
            }
        }
        (s > 0.0).then(|| ((sx / s) as f32, (sy / s) as f32))
    }

    /// Fraction of the frame covered.
    pub fn frame_share(&self) -> f32 {
        self.area() / (self.w * self.h).max(1) as f32
    }
}

pub fn rect_area(r: &Rect) -> f32 {
    (r[2] - r[0]).max(0.0) * (r[3] - r[1]).max(0.0)
}

pub fn rect_intersect(a: &Rect, b: &Rect) -> Rect {
    [a[0].max(b[0]), a[1].max(b[1]), a[2].min(b[2]), a[3].min(b[3])]
}

/// Pixels of a box that fall outside `frame` on each side (left, top,
/// right, bottom), 0 when inside.
pub fn spill(r: &Rect, frame: &Rect) -> [f32; 4] {
    [(frame[0] - r[0]).max(0.0), (frame[1] - r[1]).max(0.0), (r[2] - frame[2]).max(0.0), (r[3] - frame[3]).max(0.0)]
}

/// A box inset on every side by `share` of the width and the height: the
/// action-safe (0.05) and title-safe (0.1) areas of a frame.
pub fn inset(frame: &Rect, share: f32) -> Rect {
    let (w, h) = (frame[2] - frame[0], frame[3] - frame[1]);
    [frame[0] + w * share, frame[1] + h * share, frame[2] - w * share, frame[3] - h * share]
}

/// The area of a convex polygon (shoelace).
pub fn poly_area(p: &[(f32, f32)]) -> f32 {
    let n = p.len();
    if n < 3 {
        return 0.0;
    }
    (0..n).map(|i| p[i].0 * p[(i + 1) % n].1 - p[(i + 1) % n].0 * p[i].1).sum::<f32>().abs() / 2.0
}

/// A convex polygon cut to a box (Sutherland-Hodgman).
pub fn clip_to_rect(poly: &[(f32, f32)], r: &Rect) -> Vec<(f32, f32)> {
    let mut out: Vec<(f32, f32)> = poly.to_vec();
    // Each side: whether a point is kept, and where an edge meets it.
    type Keep = fn((f32, f32), f32) -> bool;
    type Cut = fn((f32, f32), (f32, f32), f32) -> (f32, f32);
    let sides: [(Keep, Cut, f32); 4] = [
        (|p, v| p.0 >= v, |a, b, v| (v, a.1 + (b.1 - a.1) * (v - a.0) / (b.0 - a.0)), r[0]),
        (|p, v| p.0 <= v, |a, b, v| (v, a.1 + (b.1 - a.1) * (v - a.0) / (b.0 - a.0)), r[2]),
        (|p, v| p.1 >= v, |a, b, v| (a.0 + (b.0 - a.0) * (v - a.1) / (b.1 - a.1), v), r[1]),
        (|p, v| p.1 <= v, |a, b, v| (a.0 + (b.0 - a.0) * (v - a.1) / (b.1 - a.1), v), r[3]),
    ];
    for (keep, cut, v) in sides {
        let input = std::mem::take(&mut out);
        for i in 0..input.len() {
            let (a, b) = (input[i], input[(i + 1) % input.len()]);
            match (keep(a, v), keep(b, v)) {
                (true, true) => out.push(b),
                (true, false) => out.push(cut(a, b, v)),
                (false, true) => {
                    out.push(cut(a, b, v));
                    out.push(b);
                }
                (false, false) => {}
            }
        }
        if out.is_empty() {
            break;
        }
    }
    out
}

/// The share of a convex polygon's area inside a box.
pub fn share_inside(poly: &[(f32, f32)], r: &Rect) -> f32 {
    let total = poly_area(poly);
    if total <= 1e-6 {
        return 0.0;
    }
    (poly_area(&clip_to_rect(poly, r)) / total).clamp(0.0, 1.0)
}

/// How one item sits among the others drawn after (above) it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Cover {
    /// Share of the item's own coverage still showing after everything
    /// above it is drawn.
    pub visible: f32,
    /// Items above that hide part of it: (index, share of this item's area
    /// they hide on their own).
    pub covered_by: Vec<(usize, f32)>,
    /// Items below that this one hides: (index, share of that item's area).
    pub covers: Vec<(usize, f32)>,
}

/// Occlusion between items given bottom to top. Each pair is read once, from
/// the overlap of their coverage; a share under `min_share` is left out of
/// the lists (but still counts in `visible`).
pub fn occlusion(grids: &[&Grid], min_share: f32) -> Vec<Cover> {
    let n = grids.len();
    let boxes: Vec<Option<Rect>> = grids.iter().map(|g| g.bounds(FAINT)).collect();
    let areas: Vec<f32> = grids.iter().map(|g| g.area()).collect();
    let mut out = vec![Cover { visible: 1.0, ..Cover::default() }; n];
    // How much of each item is still showing: the product of what is
    // left after each item above it, pixel by pixel.
    for i in 0..n {
        let Some(bi) = boxes[i] else {
            out[i].visible = 0.0;
            continue;
        };
        let g = grids[i];
        let (x0, y0, x1, y1) = (bi[0].floor() as usize, bi[1].floor() as usize, (bi[2].ceil() as usize).min(g.w), (bi[3].ceil() as usize).min(g.h));
        let mut left = vec![1.0f32; (x1 - x0).max(1) * (y1 - y0).max(1)];
        let bw = x1 - x0;
        for j in i + 1..n {
            let Some(bj) = boxes[j] else { continue };
            let ov = rect_intersect(&bi, &bj);
            if rect_area(&ov) <= 0.0 {
                continue;
            }
            let (ox0, oy0, ox1, oy1) = (ov[0].floor() as usize, ov[1].floor() as usize, (ov[2].ceil() as usize).min(g.w), (ov[3].ceil() as usize).min(g.h));
            let mut shared = 0f32;
            for y in oy0.max(y0)..oy1.min(y1) {
                for x in ox0.max(x0)..ox1.min(x1) {
                    let (ai, aj) = (g.at(x, y), grids[j].at(x, y));
                    if ai == 0 || aj == 0 {
                        continue;
                    }
                    shared += ai.min(aj) as f32 / 255.0;
                    left[(y - y0) * bw + (x - x0)] *= 1.0 - aj as f32 / 255.0;
                }
            }
            if shared > 0.0 {
                if areas[i] > 0.0 && shared / areas[i] >= min_share {
                    out[i].covered_by.push((j, shared / areas[i]));
                }
                if areas[i] > 0.0 && shared / areas[i] >= min_share {
                    out[j].covers.push((i, shared / areas[i]));
                }
            }
        }
        let total: f32 = (y0..y1).flat_map(|y| (x0..x1).map(move |x| (x, y))).map(|(x, y)| g.at(x, y) as f32 / 255.0).sum();
        let shown: f32 = (y0..y1).flat_map(|y| (x0..x1).map(move |x| (x, y))).map(|(x, y)| g.at(x, y) as f32 / 255.0 * left[(y - y0) * bw + (x - x0)]).sum();
        out[i].visible = if total > 0.0 { (shown / total).clamp(0.0, 1.0) } else { 0.0 };
    }
    out
}

/// The share of the smaller of two items that the other overlaps, by
/// coverage (0 when they do not meet): for lists of overlapping pairs.
pub fn overlap_share(a: &Grid, b: &Grid) -> f32 {
    let (Some(ba), Some(bb)) = (a.bounds(FAINT), b.bounds(FAINT)) else { return 0.0 };
    let ov = rect_intersect(&ba, &bb);
    if rect_area(&ov) <= 0.0 {
        return 0.0;
    }
    let mut shared = 0f32;
    for y in ov[1].floor() as usize..(ov[3].ceil() as usize).min(a.h) {
        for x in ov[0].floor() as usize..(ov[2].ceil() as usize).min(a.w) {
            shared += a.at(x, y).min(b.at(x, y)) as f32 / 255.0;
        }
    }
    let small = a.area().min(b.area());
    if small > 0.0 { (shared / small).clamp(0.0, 1.0) } else { 0.0 }
}

/// The lines of ink in a text item: runs of rows with anything solid,
/// separated by rows with none, each with its own left and right edge.
pub fn line_boxes(g: &Grid) -> Vec<Rect> {
    let mut lines: Vec<(usize, usize)> = Vec::new();
    for y in 0..g.h {
        let ink = (0..g.w).any(|x| g.at(x, y) >= SOLID / 2);
        match (ink, lines.last_mut()) {
            (true, Some(l)) if l.1 + 1 == y => l.1 = y,
            (true, _) => lines.push((y, y)),
            _ => {}
        }
    }
    lines
        .into_iter()
        .filter_map(|(y0, y1)| {
            let sub = Grid { w: g.w, h: y1 - y0 + 1, a: g.a[y0 * g.w..(y1 + 1) * g.w].to_vec() };
            let b = sub.bounds(SOLID / 2)?;
            Some([b[0], y0 as f32 + b[1], b[2], y0 as f32 + b[3]])
        })
        .collect()
}

/// The empty cells: not covered by anything in `occupied`, with the covered
/// ones grown by `margin` pixels so a placement keeps its distance.
pub fn empty_cells(occupied: &[bool], w: usize, h: usize, margin: usize) -> Vec<bool> {
    if margin == 0 {
        return occupied.iter().map(|o| !o).collect();
    }
    // Two passes of a running count: rows, then columns.
    let mut rows = vec![false; w * h];
    for y in 0..h {
        for x in 0..w {
            rows[y * w + x] = (x.saturating_sub(margin)..(x + margin + 1).min(w)).any(|k| occupied[y * w + k]);
        }
    }
    let mut free = vec![true; w * h];
    for y in 0..h {
        for x in 0..w {
            free[y * w + x] = !(y.saturating_sub(margin)..(y + margin + 1).min(h)).any(|k| rows[k * w + x]);
        }
    }
    free
}

/// The biggest empty rectangles (`[left, top, right, bottom]` in cells),
/// largest first, each at least `min_w` by `min_h`, no two overlapping by
/// more than half of the smaller one.
pub fn free_rects(free: &[bool], w: usize, h: usize, min_w: usize, min_h: usize, k: usize) -> Vec<[usize; 4]> {
    let mut heights = vec![0usize; w];
    let mut found: Vec<[usize; 4]> = Vec::new();
    for y in 0..h {
        for x in 0..w {
            heights[x] = if free[y * w + x] { heights[x] + 1 } else { 0 };
        }
        // Rectangles whose bottom edge is this row: the histogram's maximal ones.
        let mut stack: Vec<(usize, usize)> = Vec::new();
        for x in 0..=w {
            let cur = if x < w { heights[x] } else { 0 };
            let mut start = x;
            while let Some(&(sx, sh)) = stack.last() {
                if sh <= cur {
                    break;
                }
                stack.pop();
                if sh >= min_h && x - sx >= min_w {
                    found.push([sx, y + 1 - sh, x, y + 1]);
                }
                start = sx;
            }
            if cur > 0 && stack.last().is_none_or(|&(_, sh)| sh < cur) {
                stack.push((start, cur));
            }
        }
    }
    found.sort_by_key(|r| std::cmp::Reverse((r[2] - r[0]) * (r[3] - r[1])));
    let mut picked: Vec<[usize; 4]> = Vec::new();
    for r in found {
        let area = (r[2] - r[0]) * (r[3] - r[1]);
        let clash = picked.iter().any(|p| {
            let ox = r[2].min(p[2]).saturating_sub(r[0].max(p[0]));
            let oy = r[3].min(p[3]).saturating_sub(r[1].max(p[1]));
            ox * oy * 2 > area.min((p[2] - p[0]) * (p[3] - p[1]))
        });
        if !clash {
            picked.push(r);
            if picked.len() >= k {
                break;
            }
        }
    }
    picked
}

/// Where a `need_w` by `need_h` box fits in the empty cells, inside
/// `within`, best first: closest to `prefer` (cell coordinates), up to `k`
/// spots that do not overlap each other.
pub fn fit(free: &[bool], w: usize, h: usize, need_w: usize, need_h: usize, within: [usize; 4], prefer: (f32, f32), k: usize) -> Vec<[usize; 4]> {
    if need_w == 0 || need_h == 0 || within[2] <= within[0] || within[3] <= within[1] || need_w > within[2] - within[0] || need_h > within[3] - within[1] {
        return Vec::new();
    }
    // Running count of blocked cells, so each window is read in one step.
    let mut blocked = vec![0u32; (w + 1) * (h + 1)];
    for y in 0..h {
        for x in 0..w {
            blocked[(y + 1) * (w + 1) + x + 1] = (!free[y * w + x]) as u32 + blocked[y * (w + 1) + x + 1] + blocked[(y + 1) * (w + 1) + x] - blocked[y * (w + 1) + x];
        }
    }
    let count = |x0: usize, y0: usize, x1: usize, y1: usize| blocked[y1 * (w + 1) + x1] + blocked[y0 * (w + 1) + x0] - blocked[y0 * (w + 1) + x1] - blocked[y1 * (w + 1) + x0];
    let mut spots: Vec<(f32, [usize; 4])> = Vec::new();
    for y in within[1]..=within[3] - need_h {
        for x in within[0]..=within[2] - need_w {
            if count(x, y, x + need_w, y + need_h) == 0 {
                let (cx, cy) = (x as f32 + need_w as f32 / 2.0, y as f32 + need_h as f32 / 2.0);
                spots.push((((cx - prefer.0).powi(2) + (cy - prefer.1).powi(2)).sqrt(), [x, y, x + need_w, y + need_h]));
            }
        }
    }
    spots.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut out: Vec<[usize; 4]> = Vec::new();
    for (_, r) in spots {
        if out.iter().all(|p| r[2] <= p[0] || r[0] >= p[2] || r[3] <= p[1] || r[1] >= p[3]) {
            out.push(r);
            if out.len() >= k {
                break;
            }
        }
    }
    out
}

/// Where the weight of the frame sits: the centre of mass of `grids` (in
/// grid pixels) and the share of weight in the left, right, top and bottom
/// halves.
#[derive(Clone, Debug, PartialEq)]
pub struct Balance {
    pub centre: (f32, f32),
    pub left: f32,
    pub right: f32,
    pub top: f32,
    pub bottom: f32,
    pub weight: f32,
}

pub fn balance(grids: &[&Grid], w: usize, h: usize) -> Option<Balance> {
    let (mut sx, mut sy, mut s) = (0f64, 0f64, 0f64);
    let (mut l, mut t) = (0f64, 0f64);
    for y in 0..h {
        for x in 0..w {
            // Where items overlap, the weight is the strongest, not the sum.
            let v = grids.iter().map(|g| g.at(x, y)).max().unwrap_or(0) as f64 / 255.0;
            if v == 0.0 {
                continue;
            }
            sx += v * (x as f64 + 0.5);
            sy += v * (y as f64 + 0.5);
            s += v;
            if (x as f64 + 0.5) < w as f64 / 2.0 {
                l += v;
            }
            if (y as f64 + 0.5) < h as f64 / 2.0 {
                t += v;
            }
        }
    }
    (s > 0.0).then(|| Balance { centre: ((sx / s) as f32, (sy / s) as f32), left: (l / s) as f32, right: (1.0 - l / s) as f32, top: (t / s) as f32, bottom: (1.0 - t / s) as f32, weight: (s / (w * h).max(1) as f64) as f32 })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filled(w: usize, h: usize, r: [usize; 4], v: u8) -> Grid {
        let mut g = Grid::new(w, h);
        for y in r[1]..r[3] {
            for x in r[0]..r[2] {
                g.a[y * w + x] = v;
            }
        }
        g
    }

    #[test]
    fn bounds_are_found_within_a_pixel() {
        let mut g = filled(20, 10, [4, 2, 12, 8], 255);
        assert_eq!(g.bounds(SOLID), Some([4.0, 2.0, 12.0, 8.0]));
        // A right edge half way through a pixel.
        for y in 2..8 {
            g.a[y * 20 + 12] = 128;
        }
        let b = g.bounds(SOLID).unwrap();
        assert!((b[2] - 12.5).abs() < 0.01, "{b:?}");
        assert_eq!(Grid::new(5, 5).bounds(FAINT), None);
        assert_eq!(g.touches(SOLID), [false; 4]);
        assert_eq!(filled(6, 6, [0, 0, 6, 3], 255).touches(SOLID), [true, true, true, false]);
    }

    #[test]
    fn a_box_is_cut_by_the_frame() {
        let quad = [(-10.0, 0.0), (10.0, 0.0), (10.0, 10.0), (-10.0, 10.0)];
        let frame: Rect = [0.0, -50.0, 50.0, 50.0];
        assert!((share_inside(&quad, &frame) - 0.5).abs() < 1e-4);
        assert_eq!(share_inside(&quad, &[100.0, 100.0, 200.0, 200.0]), 0.0);
        assert!((share_inside(&quad, &[-50.0, -50.0, 50.0, 50.0]) - 1.0).abs() < 1e-4);
        // A turned square stays one piece.
        let diamond = [(0.0, -10.0), (10.0, 0.0), (0.0, 10.0), (-10.0, 0.0)];
        assert!((poly_area(&diamond) - 200.0).abs() < 1e-3);
        assert!((share_inside(&diamond, &[0.0, -50.0, 50.0, 50.0]) - 0.5).abs() < 1e-4);
        assert_eq!(spill(&[-5.0, 0.0, 30.0, 12.0], &[0.0, 0.0, 20.0, 10.0]), [5.0, 0.0, 10.0, 2.0]);
        assert_eq!(inset(&[0.0, 0.0, 100.0, 50.0], 0.1), [10.0, 5.0, 90.0, 45.0]);
    }

    #[test]
    fn the_item_above_hides_the_one_below() {
        // Bottom: a 10x10 square. Top: a 5x10 bar across its right half.
        let low = filled(20, 10, [0, 0, 10, 10], 255);
        let high = filled(20, 10, [5, 0, 10, 10], 255);
        let far = filled(20, 10, [15, 0, 20, 10], 255);
        let c = occlusion(&[&low, &high, &far], 0.01);
        assert!((c[0].visible - 0.5).abs() < 1e-3, "{:?}", c[0]);
        assert_eq!(c[1].visible, 1.0);
        assert_eq!(c[2].visible, 1.0);
        assert_eq!(c[0].covered_by.len(), 1);
        assert_eq!(c[0].covered_by[0].0, 1);
        assert!((c[0].covered_by[0].1 - 0.5).abs() < 1e-3);
        assert_eq!(c[1].covers[0].0, 0);
        assert!(c[2].covers.is_empty() && c[2].covered_by.is_empty());
        // A half see-through cover hides half.
        let veil = filled(20, 10, [0, 0, 10, 10], 128);
        let c = occlusion(&[&low, &veil], 0.01);
        assert!((c[0].visible - 0.498).abs() < 0.01, "{:?}", c[0]);
        // Fully hidden.
        let c = occlusion(&[&high, &low], 0.01);
        assert!(c[0].visible < 1e-3);
    }

    #[test]
    fn overlap_is_a_share_of_the_smaller() {
        let a = filled(20, 10, [0, 0, 10, 10], 255);
        let b = filled(20, 10, [8, 0, 12, 10], 255);
        assert!((overlap_share(&a, &b) - 0.5).abs() < 1e-3);
        assert_eq!(overlap_share(&a, &filled(20, 10, [12, 0, 20, 10], 255)), 0.0);
    }

    #[test]
    fn lines_of_text_are_separate_boxes() {
        let mut g = filled(40, 20, [2, 2, 30, 6], 255);
        for y in 9..13 {
            for x in 5..20 {
                g.a[y * 40 + x] = 255;
            }
        }
        let lines = line_boxes(&g);
        assert_eq!(lines, vec![[2.0, 2.0, 30.0, 6.0], [5.0, 9.0, 20.0, 13.0]]);
    }

    #[test]
    fn empty_space_is_found_and_filled() {
        // A 20 by 10 frame, the left 8 columns taken.
        let (w, h) = (20, 10);
        let occ: Vec<bool> = (0..w * h).map(|i| i % w < 8).collect();
        let free = empty_cells(&occ, w, h, 0);
        let rects = free_rects(&free, w, h, 3, 3, 3);
        assert_eq!(rects[0], [8, 0, 20, 10]);
        // With a margin of 2 the free region starts 2 further in.
        let grown = empty_cells(&occ, w, h, 2);
        assert_eq!(free_rects(&grown, w, h, 3, 3, 1)[0], [10, 0, 20, 10]);
        // A 6 by 4 box wanted near the bottom right.
        let spots = fit(&free, w, h, 6, 4, [0, 0, w, h], (19.0, 9.0), 2);
        assert_eq!(spots[0], [14, 6, 20, 10]);
        assert!(spots.len() == 2 && spots[1][0] >= 8);
        // Too big for the gap.
        assert!(fit(&free, w, h, 13, 4, [0, 0, w, h], (10.0, 5.0), 1).is_empty());
    }

    #[test]
    fn balance_leans_to_the_heavy_side() {
        let left = filled(20, 10, [0, 0, 6, 10], 255);
        let b = balance(&[&left], 20, 10).unwrap();
        assert!(b.left > 0.99 && b.right < 0.01 && (b.top - 0.5).abs() < 0.01, "{b:?}");
        assert!((b.centre.0 - 3.0).abs() < 0.01);
        let right = filled(20, 10, [14, 0, 20, 10], 255);
        let both = balance(&[&left, &right], 20, 10).unwrap();
        assert!((both.left - 0.5).abs() < 0.01);
        assert!(balance(&[&Grid::new(4, 4)], 4, 4).is_none());
    }
}
