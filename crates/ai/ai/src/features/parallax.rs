// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A still brought to life on the machine: a camera move through the
//! picture's depth (the "3D photo" or 2.5D look), near things sliding past
//! far ones. Each output pixel looks back into the picture along the
//! parallax its depth gives it, so nothing tears; the move is kept small
//! enough that stretched edges stay out of sight. This is image-to-video
//! with no key and no network; animating what is in
//! the picture (people walking, water running) is not available yet.

use crate::frame::{bilinear_rgb, Frame, Mask};

/// The kinds of move.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Move {
    PushIn,
    PullOut,
    PanLeft,
    PanRight,
    Rise,
    Fall,
    OrbitLeft,
    OrbitRight,
    /// The background grows while the subject holds its size.
    DollyZoom,
}

impl Move {
    pub const NAMES: &'static str = "push_in, pull_out, pan_left, pan_right, rise, fall, orbit_left, orbit_right, dolly_zoom";

    pub fn parse(s: &str) -> Option<Move> {
        Some(match s.trim().to_ascii_lowercase().replace([' ', '-'], "_").as_str() {
            "push_in" | "push" | "dolly_in" | "zoom_in" | "in" => Move::PushIn,
            "pull_out" | "pull" | "dolly_out" | "zoom_out" | "out" => Move::PullOut,
            "pan_left" | "truck_left" | "left" => Move::PanLeft,
            "pan_right" | "truck_right" | "right" => Move::PanRight,
            "rise" | "crane_up" | "pedestal_up" | "up" => Move::Rise,
            "fall" | "crane_down" | "pedestal_down" | "down" => Move::Fall,
            "orbit_left" | "arc_left" | "orbit" => Move::OrbitLeft,
            "orbit_right" | "arc_right" => Move::OrbitRight,
            "dolly_zoom" | "vertigo" => Move::DollyZoom,
            _ => return None,
        })
    }

    /// Camera at `t` (0..1, eased): sideways and vertical shift as a share
    /// of the width, and the zoom of the nearest and farthest planes.
    fn at(self, t: f32, amount: f32) -> (f32, f32, f32, f32) {
        let e = t * t * (3.0 - 2.0 * t);
        let a = amount;
        let s = (e - 0.5) * 2.0;
        match self {
            Move::PushIn => (0.0, 0.0, 1.0 + 0.16 * a * e, 1.0 + 0.06 * a * e),
            Move::PullOut => (0.0, 0.0, 1.0 + 0.16 * a * (1.0 - e), 1.0 + 0.06 * a * (1.0 - e)),
            Move::PanLeft => (-0.04 * a * s, 0.0, 1.06, 1.06),
            Move::PanRight => (0.04 * a * s, 0.0, 1.06, 1.06),
            Move::Rise => (0.0, -0.035 * a * s, 1.06, 1.06),
            Move::Fall => (0.0, 0.035 * a * s, 1.06, 1.06),
            Move::OrbitLeft => (-0.05 * a * s, 0.0, 1.08, 1.04),
            Move::OrbitRight => (0.05 * a * s, 0.0, 1.08, 1.04),
            Move::DollyZoom => (0.0, 0.0, 1.06, 1.06 + 0.18 * a * e),
        }
    }
}

/// Frame `i` of `n` of `still` moved by `mv` over its `depth` (1 = near;
/// same size or resized). `amount` scales the move (1 = a gentle move).
pub fn frame(still: &Frame, depth: &Mask, mv: Move, amount: f32, i: usize, n: usize) -> Frame {
    let (w, h) = (still.width, still.height);
    let d = if (depth.width, depth.height) == (w, h) { depth.clone() } else { depth.resized(w, h) };
    let t = if n <= 1 { 0.0 } else { i as f32 / (n - 1) as f32 };
    let (sx, sy, zoom_near, zoom_far) = mv.at(t, amount.clamp(0.0, 3.0));
    let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
    let depth_at = |x: f32, y: f32| d.data[(y.clamp(0.0, h as f32 - 1.0) as usize) * w + x.clamp(0.0, w as f32 - 1.0) as usize];
    let mut out = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let (qx, qy) = (x as f32 + 0.5, y as f32 + 0.5);
            // Solve p + offset(depth(p)) = q by fixed-point steps.
            let (mut px, mut py) = (qx, qy);
            for _ in 0..4 {
                let z = depth_at(px, py);
                let zoom = zoom_far + (zoom_near - zoom_far) * z;
                // Nearer planes shift further: parallax.
                let (ox, oy) = (sx * w as f32 * (0.3 + z), sy * w as f32 * (0.3 + z));
                px = cx + (qx - cx - ox) / zoom;
                py = cy + (qy - cy - oy) / zoom;
            }
            let rgb = bilinear_rgb(still, px - 0.5, py - 0.5);
            let o = (y * w + x) * 4;
            for c in 0..3 {
                out[o + c] = (rgb[c] + 0.5).clamp(0.0, 255.0) as u8;
            }
            out[o + 3] = still.px(px as isize, py as isize)[3];
        }
    }
    Frame { width: w, height: h, rgba: out }
}

/// A depth guess for a picture with no depth model: lower in the frame is
/// nearer (the ground plane), which suits landscapes and rooms.
pub fn ground_plane(w: usize, h: usize) -> Mask {
    let mut d = vec![0f32; w * h];
    for y in 0..h {
        let v = (y as f32 / h.max(1) as f32).powf(1.5);
        d[y * w..(y + 1) * w].fill(v);
    }
    Mask::new(w, h, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn near_things_move_more_than_far_ones() {
        let (w, h) = (64, 32);
        let mut f = Frame::filled(w, h, [0, 0, 0, 255]);
        for y in 0..h {
            for x in 0..w {
                let v = ((x * 4) % 256) as u8;
                f.rgba[(y * w + x) * 4..(y * w + x) * 4 + 3].copy_from_slice(&[v, v, v]);
            }
        }
        // Top half far, bottom half near.
        let mut d = vec![0f32; w * h];
        d[w * h / 2..].fill(1.0);
        let depth = Mask::new(w, h, d);
        let a = frame(&f, &depth, Move::PanRight, 1.0, 0, 10);
        let b = frame(&f, &depth, Move::PanRight, 1.0, 9, 10);
        let shift = |row: usize| (0..w).map(|x| (a.px(x as isize, row as isize)[0] as i32 - b.px(x as isize, row as isize)[0] as i32).abs()).sum::<i32>();
        assert!(shift(28) > shift(4), "near {} far {}", shift(28), shift(4));
        assert_eq!(Move::parse("push in"), Some(Move::PushIn));
        let g = ground_plane(4, 4);
        assert!(g.data[12] > g.data[0]);
    }
}
