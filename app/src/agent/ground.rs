// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `see` with `place`: what a backdrop offers to stand on. From its depth
//! map (Depth Anything V2): where the far land meets the ground, which
//! parts are ground (surfaces that run away from the camera) rather than
//! walls, rocks and trunks, and spots to put characters on, each with
//! the height a character standing there should have so near and far
//! read right.

/// Depth on a small grid, 1 = near.
pub struct DepthGrid {
    pub w: usize,
    pub h: usize,
    pub d: Vec<f32>,
}

impl DepthGrid {
    /// A grid about `w` cells across from a full depth map.
    pub fn shrink(src_w: usize, src_h: usize, src: &[f32], w: usize) -> DepthGrid {
        let h = (w * src_h / src_w.max(1)).max(4);
        let mut d = vec![0f32; w * h];
        for y in 0..h {
            for x in 0..w {
                let (x0, x1) = (x * src_w / w, ((x + 1) * src_w / w).max(x * src_w / w + 1));
                let (y0, y1) = (y * src_h / h, ((y + 1) * src_h / h).max(y * src_h / h + 1));
                let (mut s, mut n) = (0f32, 0f32);
                for yy in y0..y1.min(src_h) {
                    for xx in x0..x1.min(src_w) {
                        s += src[yy * src_w + xx];
                        n += 1.0;
                    }
                }
                d[y * w + x] = s / n.max(1.0);
            }
        }
        DepthGrid { w, h, d }
    }

    fn at(&self, x: usize, y: usize) -> f32 {
        self.d[y.min(self.h - 1) * self.w + x.min(self.w - 1)]
    }

    /// Ground: getting nearer row by row going down, and level across.
    pub fn ground(&self) -> Vec<bool> {
        let mut g = vec![false; self.w * self.h];
        for y in 1..self.h - 1 {
            for x in 1..self.w - 1 {
                let down = self.at(x, y + 1) - self.at(x, y - 1);
                let across = (self.at(x + 1, y) - self.at(x - 1, y)).abs();
                // Per row of a grid ~64 rows tall, a floor gains a little
                // depth each row; a wall or trunk face gains about none.
                let step = 1.0 / self.h as f32;
                g[y * self.w + x] = down > 0.15 * step && down < 12.0 * step && across < down.max(0.6 * step) * 1.5;
            }
        }
        g
    }

    /// The row where the far land ends (the farthest band of the picture
    /// meets nearer things), as a fraction of the height from the top.
    pub fn horizon(&self) -> f32 {
        let row: Vec<f32> = (0..self.h)
            .map(|y| {
                let mut v: Vec<f32> = (0..self.w).map(|x| self.at(x, y)).collect();
                v.sort_by(f32::total_cmp);
                v[v.len() / 2]
            })
            .collect();
        let far = row.iter().cloned().fold(f32::MAX, f32::min);
        let near = row.iter().cloned().fold(f32::MIN, f32::max);
        let cut = far + 0.2 * (near - far);
        let last_far = row.iter().rposition(|&v| v <= cut).unwrap_or(0);
        (last_far as f32 + 0.5) / self.h as f32
    }
}

/// A place to stand: where the feet go (0..1 across and down), how much
/// of the area around it is ground, and its depth.
pub struct Spot {
    pub u: f32,
    pub v: f32,
    pub ground: f32,
    pub depth: f32,
    pub name: String,
}

/// Spots across the ground: near, middle and far rows, left, centre and
/// right columns, each moved to the most ground-like cell near it.
pub fn spots(g: &DepthGrid) -> (f32, Vec<Spot>) {
    let ground = g.ground();
    let horizon = g.horizon().clamp(0.05, 0.9);
    let mut out = Vec::new();
    for (row_name, f) in [("near", 0.82), ("middle", 0.55), ("far", 0.25)] {
        let v = horizon + (1.0 - horizon) * f;
        for (col_name, u) in [("left", 0.2), ("centre", 0.5), ("right", 0.8)] {
            let (cx, cy) = ((u * g.w as f32) as i32, (v * g.h as f32) as i32);
            let r = (g.w / 14).max(2) as i32;
            let mut best: Option<(f32, i32, i32)> = None;
            for y in (cy - r).max(1)..=(cy + r).min(g.h as i32 - 2) {
                for x in (cx - 2 * r).max(1)..=(cx + 2 * r).min(g.w as i32 - 2) {
                    let mut n = 0;
                    let mut tot = 0;
                    for yy in (y - 2).max(0)..=(y + 2).min(g.h as i32 - 1) {
                        for xx in (x - 3).max(0)..=(x + 3).min(g.w as i32 - 1) {
                            tot += 1;
                            n += ground[yy as usize * g.w + xx as usize] as i32;
                        }
                    }
                    let score = n as f32 / tot as f32 - 0.002 * ((x - cx).pow(2) + (y - cy).pow(2)) as f32;
                    if best.is_none_or(|b| score > b.0) {
                        best = Some((score, x, y));
                    }
                }
            }
            let Some((_, x, y)) = best else { continue };
            let mut n = 0;
            let mut tot = 0;
            for yy in (y - 2).max(0)..=(y + 2).min(g.h as i32 - 1) {
                for xx in (x - 3).max(0)..=(x + 3).min(g.w as i32 - 1) {
                    tot += 1;
                    n += ground[yy as usize * g.w + xx as usize] as i32;
                }
            }
            out.push(Spot {
                u: (x as f32 + 0.5) / g.w as f32,
                v: (y as f32 + 0.5) / g.h as f32,
                ground: n as f32 / tot as f32,
                depth: g.at(x as usize, y as usize),
                name: format!("{row_name} {col_name}"),
            });
        }
    }
    (horizon, out)
}

/// A part of the backdrop standing up nearer than the ground behind it (a
/// rock, a trunk, a post, a bank of grass): a character whose feet are on
/// that ground further back is hidden by it, once the piece is cut out
/// and laid over the character. Box and points are 0..1 of the frame.
pub struct Piece {
    /// x0, y0, x1, y1.
    pub bbox: [f32; 4],
    /// Its median depth (1 = near).
    pub depth: f32,
    /// A point well inside it.
    pub inside: [f32; 2],
    /// Where to put a character's feet to stand behind it (hidden from
    /// there up to its top edge), and the depth of the land behind it.
    pub behind: [f32; 2],
    pub behind_depth: f32,
    /// A point on the land just behind its top edge.
    pub land: [f32; 2],
    pub kind: &'static str,
}

impl Piece {
    /// A word for it, to name its layer.
    pub fn short(&self) -> &'static str {
        if self.kind.starts_with("tall") {
            "trunk"
        } else if self.kind.starts_with("low") {
            "grass"
        } else {
            "rock"
        }
    }
}

impl DepthGrid {
    /// Pieces that can hide a character, nearest first. A piece starts
    /// where the depth jumps nearer going down a column (its top edge: the
    /// land behind it ends there) and runs down its face while the depth
    /// holds (a face stands up; ground would keep coming nearer). Faces
    /// that touch, at about the same depth, are one piece.
    pub fn pieces(&self) -> Vec<Piece> {
        let (w, h) = (self.w, self.h);
        let mut sorted = self.d.clone();
        sorted.sort_by(f32::total_cmp);
        let (lo, hi) = (sorted[sorted.len() / 50], sorted[sorted.len() - 1 - sorted.len() / 50]);
        let span = (hi - lo).max(1e-3);
        let n = |i: usize| (self.d[i] - lo) / span;
        // Per cell: the row of the top edge of the face it is on.
        let mut top = vec![usize::MAX; w * h];
        for x in 0..w {
            let mut y = 2;
            while y < h {
                let i = y * w + x;
                if n(i) - n((y - 2) * w + x) > 0.09 && n(i) > 0.25 {
                    let r = n(i);
                    let mut yy = y;
                    while yy < h && (n(yy * w + x) - r).abs() < 0.06 {
                        top[yy * w + x] = y;
                        yy += 1;
                    }
                    y = yy.max(y + 1);
                } else {
                    y += 1;
                }
            }
        }
        let mut seen = vec![false; w * h];
        let mut out = Vec::new();
        for start in 0..w * h {
            if top[start] == usize::MAX || seen[start] {
                continue;
            }
            let mut stack = vec![start];
            seen[start] = true;
            let mut cells = Vec::new();
            while let Some(i) = stack.pop() {
                cells.push(i);
                let (x, y) = ((i % w) as i32, (i / w) as i32);
                for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                    let (nx, ny) = (x + dx, y + dy);
                    if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
                        continue;
                    }
                    let j = ny as usize * w + nx as usize;
                    if top[j] != usize::MAX && !seen[j] && (n(j) - n(i)).abs() < 0.06 {
                        seen[j] = true;
                        stack.push(j);
                    }
                }
            }
            let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
            for &i in &cells {
                let (x, y) = (i % w, i / w);
                (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
            }
            let (bw, bh) = (x1 - x0 + 1, y1 - y0 + 1);
            // Big enough to hide part of someone; not the whole near ground.
            if cells.len() < (w * h / 300).max(6) || bh < 3 || bw * 5 > w * 3 {
                continue;
            }
            let mut ds: Vec<f32> = cells.iter().map(|&i| self.d[i]).collect();
            ds.sort_by(f32::total_cmp);
            let depth = ds[ds.len() / 2];
            // The land just behind its top edge, along it.
            let mut back: Vec<f32> = cells.iter().filter(|&&i| i / w == top[i] && top[i] >= 2).map(|&i| self.d[(top[i] - 2) * w + i % w]).collect();
            back.sort_by(f32::total_cmp);
            let Some(&behind_depth) = back.get(back.len() / 2) else { continue };
            if (depth - behind_depth) / span < 0.08 {
                continue;
            }
            cells.sort_unstable();
            // A point well inside it: the cell with the most of it around.
            let inside = cells
                .iter()
                .max_by_key(|&&i| {
                    let (x, y) = ((i % w) as i32, (i / w) as i32);
                    let mut k = 0;
                    for yy in y - 2..=y + 2 {
                        for xx in x - 2..=x + 2 {
                            k += (xx >= 0 && yy >= 0 && (xx as usize) < w && (yy as usize) < h && cells.binary_search(&(yy as usize * w + xx as usize)).is_ok()) as i32;
                        }
                    }
                    k
                })
                .copied()
                .unwrap_or(cells[0]);
            // Feet behind it: in its middle, low enough that it covers the
            // character from there up to its top edge.
            let cx = cells.iter().map(|&i| (i % w) as f32).sum::<f32>() / cells.len() as f32;
            let foot_y = y0 as f32 + 0.65 * (y1 - y0) as f32;
            let kind = if bh > 2 * bw {
                "tall and narrow: a trunk, a post or a reed"
            } else if y1 + 2 >= h && bh * 4 < h {
                "low along the front: grass, plants or a bank"
            } else {
                "a rock, a log, a bush or a mound"
            };
            out.push(Piece {
                bbox: [x0 as f32 / w as f32, y0 as f32 / h as f32, (x1 + 1) as f32 / w as f32, (y1 + 1) as f32 / h as f32],
                depth,
                inside: [((inside % w) as f32 + 0.5) / w as f32, ((inside / w) as f32 + 0.5) / h as f32],
                behind: [(cx + 0.5) / w as f32, (foot_y + 0.5) / h as f32],
                behind_depth,
                land: [(cx + 0.5) / w as f32, (y0 as f32 - 1.5).max(0.5) / h as f32],
                kind,
            });
        }
        out.sort_by(|a, b| b.depth.total_cmp(&a.depth));
        out.truncate(6);
        out
    }
}

/// The height (fraction of the frame) for a character standing at a spot
/// of `depth`, given the nearest ground's depth: things shrink with
/// distance, and inverse depth is proportional to size.
pub fn height_at(depth: f32, nearest: f32, near_height: f32) -> f32 {
    (near_height * depth / nearest.max(1e-3)).clamp(0.04, near_height)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sky (far, flat) over a floor that comes nearer toward the bottom,
    /// with a wall (constant depth) standing on the right of the floor.
    fn scene() -> DepthGrid {
        let (w, h) = (64, 36);
        let mut d = vec![0f32; w * h];
        for y in 0..h {
            for x in 0..w {
                let floor = if y < 12 { 0.02 } else { 0.1 + 0.9 * (y - 12) as f32 / (h - 12) as f32 };
                d[y * w + x] = if x >= 48 && (14..30).contains(&y) { 0.55 } else { floor };
            }
        }
        DepthGrid { w, h, d }
    }

    #[test]
    fn the_floor_is_ground_and_the_wall_is_not() {
        let g = scene();
        let ground = g.ground();
        assert!(ground[25 * 64 + 20], "floor");
        assert!(!ground[20 * 64 + 56], "wall");
        assert!(!ground[5 * 64 + 20], "sky");
        let hz = g.horizon();
        assert!((0.25..0.45).contains(&hz), "{hz}");
    }

    #[test]
    fn spots_land_on_ground_and_far_ones_are_smaller() {
        let g = scene();
        let (_, s) = spots(&g);
        assert_eq!(s.len(), 9);
        assert!(s.iter().filter(|p| p.name.ends_with("left")).all(|p| p.ground > 0.7));
        let near = s.iter().find(|p| p.name == "near centre").unwrap();
        let far = s.iter().find(|p| p.name == "far centre").unwrap();
        assert!(near.v > far.v && near.depth > far.depth);
        assert!(height_at(far.depth, near.depth, 0.5) < height_at(near.depth, near.depth, 0.5));
    }

    #[test]
    fn a_rock_on_the_floor_is_a_piece_to_hide_behind() {
        let mut g = scene();
        // A rock face standing where the floor is 0.7 deep (row 28),
        // reaching up to row 22, across x 16..28.
        let floor = |y: usize| 0.1 + 0.9 * (y as f32 - 12.0) / 24.0;
        for y in 22..=28 {
            for x in 16..28 {
                g.d[y * 64 + x] = floor(28);
            }
        }
        let ps = g.pieces();
        let rock = ps.iter().find(|p| p.bbox[0] < 0.3 && p.bbox[2] < 0.5).expect("the rock");
        assert!((rock.depth - floor(28)).abs() < 0.02);
        assert!(rock.behind_depth < rock.depth - 0.05, "{} vs {}", rock.behind_depth, rock.depth);
        assert!(rock.kind.starts_with("a rock"), "{}", rock.kind);
        let (x, y) = (rock.inside[0] * 64.0, rock.inside[1] * 36.0);
        assert!((16.0..28.0).contains(&x) && (22.0..29.0).contains(&y), "{x} {y}");
        // The wall on the right is one too; the sky and the floor are not.
        assert!(ps.iter().any(|p| p.bbox[0] > 0.7));
        assert!(ps.iter().all(|p| p.bbox[1] > 0.3));
    }
}
