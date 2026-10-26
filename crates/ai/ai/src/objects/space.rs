// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Free space in the frame: where a title, a lower third or a placed thing
//! will not cover a face, a subject or a busy part of the picture. The
//! frame is a grid; objects over the whole time asked about, faces, text
//! and the busiest parts of the picture mark cells as taken, and the
//! largest empty rectangles are what is left.

use super::{BBox, Kind, ObjectMap};

pub const COLS: usize = 48;
pub const ROWS: usize = 27;

/// What to look for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpaceOptions {
    /// Time range (seconds into the footage) the space must stay free for.
    pub from: f32,
    pub to: f32,
    /// Keep this share of the frame clear around the edge (safe area).
    pub margin: f32,
    /// Smallest rectangle worth reporting, as a share of the frame's width
    /// and height.
    pub min_w: f32,
    pub min_h: f32,
    /// At most this many rectangles.
    pub max: usize,
}

impl Default for SpaceOptions {
    fn default() -> Self {
        SpaceOptions { from: 0.0, to: f32::MAX, margin: 0.05, min_w: 0.2, min_h: 0.1, max: 4 }
    }
}

/// An empty rectangle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Free {
    pub rect: BBox,
    /// Share of the frame it covers.
    pub share: f32,
}

/// Objects larger than this share of the frame are the place, not things in
/// it: a sofa or a table does not make its whole area unusable.
const BACKDROP_SHARE: f32 = 0.35;

/// Cells taken by something between `from` and `to`.
pub fn occupancy(map: &ObjectMap, from: f32, to: f32, margin: f32) -> Vec<bool> {
    let mut taken = vec![false; COLS * ROWS];
    let mut mark = |b: &BBox| {
        let b = b.clamped();
        if b.w <= 0.0 || b.h <= 0.0 {
            return;
        }
        let x0 = ((b.x * COLS as f32).floor() as usize).min(COLS - 1);
        let x1 = ((b.right() * COLS as f32).ceil() as usize).clamp(x0 + 1, COLS);
        let y0 = ((b.y * ROWS as f32).floor() as usize).min(ROWS - 1);
        let y1 = ((b.bottom() * ROWS as f32).ceil() as usize).clamp(y0 + 1, ROWS);
        for y in y0..y1 {
            for x in x0..x1 {
                taken[y * COLS + x] = true;
            }
        }
    };
    // Safe margin.
    mark(&BBox::new(0.0, 0.0, 1.0, margin));
    mark(&BBox::new(0.0, 1.0 - margin, 1.0, margin));
    mark(&BBox::new(0.0, 0.0, margin, 1.0));
    mark(&BBox::new(1.0 - margin, 0.0, margin, 1.0));
    let (lo, hi) = (from.min(to), from.max(to));
    for o in &map.objects {
        if o.kind == Kind::Scenery || o.leave < lo || o.enter > hi {
            continue;
        }
        let mut boxes: Vec<BBox> = o.obs.iter().filter(|ob| ob.t >= lo - 0.5 && ob.t <= hi + 0.5).map(|ob| ob.bbox).collect();
        // Between the observations too: the path matters, not just the dots.
        let (t0, t1) = (lo.max(o.enter), hi.min(o.leave));
        let mut t = t0;
        while t <= t1 {
            if let Some(ob) = o.at(t) {
                boxes.push(ob.bbox);
            }
            t += 0.25;
        }
        for b in boxes {
            if b.area() > BACKDROP_SHARE && o.kind != Kind::Person {
                continue;
            }
            mark(&b.grown(1.08));
        }
    }
    for tx in &map.texts {
        if tx.t >= lo - 0.5 && tx.t <= hi + 0.5 {
            mark(&tx.bbox.grown(1.15));
        }
    }
    // The busiest parts of the picture are poor places for lettering.
    if !map.detail.is_empty() {
        for (i, kt) in map.keyframes.iter().enumerate() {
            if *kt < lo - 0.75 || *kt > hi + 0.75 && i > 0 {
                continue;
            }
            let Some(grid) = map.detail.get(i) else { continue };
            if grid.len() != 16 * 9 {
                continue;
            }
            for gy in 0..9 {
                for gx in 0..16 {
                    if grid[gy * 16 + gx] >= 150 {
                        mark(&BBox::new(gx as f32 / 16.0, gy as f32 / 9.0, 1.0 / 16.0, 1.0 / 9.0));
                    }
                }
            }
        }
    }
    taken
}

/// The largest empty rectangle in `taken` (cell coordinates `x0, y0, w, h`).
fn largest_empty(taken: &[bool]) -> Option<(usize, usize, usize, usize)> {
    let mut heights = [0usize; COLS];
    let mut best: Option<(usize, usize, usize, usize)> = None;
    for y in 0..ROWS {
        for x in 0..COLS {
            heights[x] = if taken[y * COLS + x] { 0 } else { heights[x] + 1 };
        }
        // Largest rectangle under the histogram.
        let mut stack: Vec<(usize, usize)> = Vec::new();
        for x in 0..=COLS {
            let h = if x < COLS { heights[x] } else { 0 };
            let mut start = x;
            while let Some(&(sx, sh)) = stack.last() {
                if sh <= h {
                    break;
                }
                stack.pop();
                let (w, area) = (x - sx, sh * (x - sx));
                if best.is_none_or(|b| area > b.2 * b.3) {
                    best = Some((sx, y + 1 - sh, w, sh));
                }
                start = sx;
            }
            stack.push((start, h));
        }
    }
    best.filter(|b| b.2 * b.3 > 0)
}

/// The largest empty rectangles, biggest first, that are at least
/// `min_w` x `min_h` of the frame.
pub fn free_space(map: &ObjectMap, o: &SpaceOptions) -> Vec<Free> {
    let mut taken = occupancy(map, o.from, o.to.min(map.duration.max(o.from)), o.margin);
    let mut out = Vec::new();
    while out.len() < o.max {
        let Some((x, y, w, h)) = largest_empty(&taken) else { break };
        let rect = BBox::new(x as f32 / COLS as f32, y as f32 / ROWS as f32, w as f32 / COLS as f32, h as f32 / ROWS as f32);
        if rect.w < o.min_w || rect.h < o.min_h {
            break;
        }
        out.push(Free { rect, share: rect.area() });
        for yy in y..y + h {
            for xx in x..x + w {
                taken[yy * COLS + xx] = true;
            }
        }
    }
    out
}

/// "top", "bottom left", "centre", ... for a box.
pub fn place_words(b: &BBox) -> String {
    let h = if b.cx() < 0.38 { "left" } else if b.cx() > 0.62 { "right" } else { "" };
    let v = if b.cy() < 0.36 { "top" } else if b.cy() > 0.64 { "bottom" } else { "" };
    match (v, h) {
        ("", "") => "centre".into(),
        (v, "") => v.into(),
        ("", h) => h.into(),
        (v, h) => format!("{v} {h}"),
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing::*;
    use super::*;

    #[test]
    fn a_subject_on_the_right_leaves_the_left() {
        let person = object(1, "person", Kind::Person, vec![obs(0.0, 0.55, 0.15, 0.3, 0.75), obs(4.0, 0.55, 0.15, 0.3, 0.75)]);
        let m = map(vec![person]);
        let f = free_space(&m, &SpaceOptions::default());
        assert!(!f.is_empty());
        let best = f[0].rect;
        assert!(best.right() <= 0.55 + 1e-3, "{best:?}");
        assert!(f[0].share > 0.3, "{:?}", f[0]);
        assert!(place_words(&best).contains("left") || place_words(&best) == "centre", "{}", place_words(&best));
    }

    #[test]
    fn the_whole_time_asked_is_kept_clear() {
        // A dog crosses the left side during the second half.
        let dog = object(1, "dog", Kind::Animal, vec![obs(2.0, 0.0, 0.6, 0.25, 0.3), obs(4.0, 0.3, 0.6, 0.25, 0.3)]);
        let m = map(vec![dog]);
        let early = free_space(&m, &SpaceOptions { from: 0.0, to: 1.0, ..Default::default() });
        let whole = free_space(&m, &SpaceOptions { from: 0.0, to: 4.0, ..Default::default() });
        assert!(early[0].share > whole[0].share + 0.02, "{} vs {}", early[0].share, whole[0].share);
    }

    #[test]
    fn a_backdrop_sized_object_does_not_take_the_frame() {
        let sofa = object(1, "sofa", Kind::Thing, vec![obs(0.0, 0.05, 0.3, 0.9, 0.65)]);
        let m = map(vec![sofa]);
        assert!(free_space(&m, &SpaceOptions::default())[0].share > 0.6);
    }

    #[test]
    fn text_and_busy_cells_are_avoided() {
        let mut m = map(vec![]);
        m.duration = 2.0;
        m.texts.push(super::super::TextItem { t: 0.0, text: "SALE".into(), bbox: BBox::new(0.1, 0.1, 0.8, 0.2), score: 0.9, object: None });
        let f = free_space(&m, &SpaceOptions::default());
        assert!(f[0].rect.y >= 0.3 - 1e-3 || f[0].rect.bottom() <= 0.1 + 1e-3, "{:?}", f[0]);
        // Busy bottom half.
        let mut m2 = map(vec![]);
        m2.duration = 2.0;
        m2.keyframes = vec![0.0];
        let mut grid = vec![0u8; 16 * 9];
        for i in 16 * 5..16 * 9 {
            grid[i] = 220;
        }
        m2.detail = vec![grid];
        let f = free_space(&m2, &SpaceOptions::default());
        assert!(f[0].rect.bottom() <= 5.0 / 9.0 + 0.04, "{:?}", f[0]);
    }

    #[test]
    fn rectangles_do_not_overlap_and_a_full_frame_has_none() {
        let wall = object(1, "person", Kind::Person, vec![obs(0.0, 0.0, 0.0, 1.0, 1.0)]);
        // A person-kind box over everything: nothing is free.
        assert!(free_space(&map(vec![wall]), &SpaceOptions::default()).is_empty());
        let a = object(1, "person", Kind::Person, vec![obs(0.0, 0.4, 0.2, 0.2, 0.6)]);
        let f = free_space(&map(vec![a]), &SpaceOptions { min_w: 0.1, ..Default::default() });
        assert!(f.len() >= 2);
        for i in 0..f.len() {
            for j in i + 1..f.len() {
                assert!(f[i].rect.inter(&f[j].rect) < 1e-6, "{:?} {:?}", f[i], f[j]);
            }
        }
    }
}
