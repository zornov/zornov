// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! From a picture and a relative depth map to places and sizes in metres.
//!
//! The depth model says which pixels are nearer, not how far: its map is
//! relative inverse depth `d` (1 = nearest, 0 = farthest) up to an unknown
//! scale and offset. Real distance follows once something in view has a
//! known size: a person is about 1.7 m tall, a face 0.22 m, a car 1.5 m. A
//! thing of height `H` metres that is `h` pixels tall lies at
//! `z = fx * H / h` (pinhole camera), which gives its inverse distance
//! `1/z`; a line `1/z = a * d + b` fitted through such anchors turns the
//! whole map into metres. Without any anchor the scale is assumed and
//! the answers are marked as rough.
//!
//! Camera space: x to the right, y up, z forward (metres from the lens).

use serde::{Deserialize, Serialize};

/// Horizontal field of view assumed when the footage does not say (a
/// 35 mm-equivalent lens).
pub const DEFAULT_HFOV_DEG: f32 = 63.0;
/// Nothing is placed farther than this (the model cannot tell far from
/// very far).
pub const MAX_DISTANCE: f32 = 60.0;
/// Nothing is placed nearer than this.
pub const MIN_DISTANCE: f32 = 0.25;

/// A pinhole camera for a `width` x `height` picture.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    pub width: f32,
    pub height: f32,
    /// Focal length in pixels (square pixels).
    pub focal: f32,
}

impl Camera {
    pub fn new(width: f32, height: f32, hfov_deg: f32) -> Camera {
        let hfov = hfov_deg.clamp(10.0, 170.0).to_radians();
        Camera { width, height, focal: (width / 2.0) / (hfov / 2.0).tan() }
    }

    /// The point at pixel `(u, v)` (origin top-left) and depth `z` along the
    /// optical axis, in camera space.
    pub fn unproject(&self, u: f32, v: f32, z: f32) -> [f32; 3] {
        [(u - self.width / 2.0) * z / self.focal, -(v - self.height / 2.0) * z / self.focal, z]
    }

    /// The pixel a camera-space point lands on.
    pub fn project(&self, p: [f32; 3]) -> (f32, f32) {
        let z = p[2].max(1e-6);
        (p[0] * self.focal / z + self.width / 2.0, -p[1] * self.focal / z + self.height / 2.0)
    }

    /// Metres covered by `pixels` at depth `z`.
    pub fn metres(&self, pixels: f32, z: f32) -> f32 {
        pixels * z / self.focal
    }

    /// Depth at which something `metres` long spans `pixels`.
    pub fn depth_of(&self, metres: f32, pixels: f32) -> f32 {
        self.focal * metres / pixels.max(1e-3)
    }
}

/// Which side of a box a known size is measured on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Height,
    Width,
}

/// What a label tells about real size: a typical length of one side of its
/// box, and how far off that usually is (relative; larger = less sure).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Prior {
    pub side: Side,
    pub metres: f32,
    pub spread: f32,
}

/// Typical sizes of things seen upright. A label not here is never used as
/// an anchor, but it still gets a size once the scale is known.
pub fn prior(label: &str) -> Option<Prior> {
    let h = |metres: f32, spread: f32| Some(Prior { side: Side::Height, metres, spread });
    let w = |metres: f32, spread: f32| Some(Prior { side: Side::Width, metres, spread });
    match label {
        "person" => h(1.7, 0.12),
        "face" => h(0.22, 0.12),
        "car" => h(1.5, 0.12),
        "truck" => h(3.2, 0.25),
        "bus" => h(3.2, 0.12),
        "motorcycle" => h(1.1, 0.15),
        "bicycle" => h(1.0, 0.15),
        "horse" => h(1.6, 0.15),
        "cow" => h(1.4, 0.15),
        "door" => h(2.0, 0.1),
        "chair" => h(0.9, 0.2),
        "sofa" => h(0.85, 0.2),
        "bottle" => h(0.25, 0.25),
        "cup" => h(0.10, 0.25),
        "laptop" => w(0.33, 0.12),
        "keyboard" => w(0.45, 0.12),
        "phone" => h(0.15, 0.15),
        "book" => h(0.24, 0.2),
        "traffic light" => h(0.9, 0.3),
        "umbrella" => h(0.9, 0.3),
        "guitar" => h(1.0, 0.1),
        "backpack" => h(0.5, 0.25),
        "suitcase" => h(0.65, 0.25),
        "clock" => h(0.3, 0.5),
        _ => None,
    }
}

/// One thing of known size seen at relative depth `d`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Anchor {
    /// Relative inverse depth under the thing, 0..1 (1 = nearest).
    pub d: f32,
    /// Its inverse distance, 1 / metres.
    pub inv: f32,
    /// Weight (1 / spread squared, roughly).
    pub weight: f32,
}

impl Anchor {
    /// An anchor from a box `pixels` long on the side the prior measures.
    pub fn from_box(cam: &Camera, p: &Prior, pixels: f32, d: f32) -> Option<Anchor> {
        if !(pixels > 2.0) || !d.is_finite() {
            return None;
        }
        let z = cam.depth_of(p.metres, pixels).clamp(MIN_DISTANCE, MAX_DISTANCE);
        Some(Anchor { d: d.clamp(0.0, 1.0), inv: 1.0 / z, weight: 1.0 / (p.spread * p.spread).max(1e-4) })
    }
}

/// `1/z = a * d + b`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct DepthScale {
    pub a: f32,
    pub b: f32,
    /// How many anchors the fit stands on; 0 means the scale is assumed.
    pub anchors: u32,
}

impl DepthScale {
    /// The scale used when nothing of known size is in view: the nearest
    /// pixels are about 1 m away and the farthest 25 m. Answers built on it
    /// are right in proportion (this is twice as far as that) but not in
    /// metres.
    pub fn assumed() -> DepthScale {
        let (near, far) = (1.0f32, 25.0f32);
        DepthScale { a: 1.0 / near - 1.0 / far, b: 1.0 / far, anchors: 0 }
    }

    /// Fits the line through the anchors (weighted least squares), keeping
    /// it sane: nearer pixels are never farther, and distances stay inside
    /// [`MIN_DISTANCE`], [`MAX_DISTANCE`].
    pub fn fit(anchors: &[Anchor]) -> DepthScale {
        let n = anchors.len() as u32;
        match anchors {
            [] => DepthScale::assumed(),
            [one] => {
                // One anchor fixes the scale; the farthest pixels sit at a
                // fixed far distance.
                let b = 1.0 / 30.0f32;
                let a = ((one.inv - b) / one.d.max(0.15)).max(1e-3);
                DepthScale { a, b, anchors: n }.clamped()
            }
            _ => {
                let (mut sw, mut sd, mut si, mut sdd, mut sdi) = (0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64);
                for x in anchors {
                    let w = x.weight as f64;
                    sw += w;
                    sd += w * x.d as f64;
                    si += w * x.inv as f64;
                    sdd += w * (x.d as f64).powi(2);
                    sdi += w * x.d as f64 * x.inv as f64;
                }
                let det = sw * sdd - sd * sd;
                if det.abs() < 1e-6 * sw * sw {
                    // All anchors at the same depth: treat as one.
                    let mean_d = (sd / sw) as f32;
                    let mean_i = (si / sw) as f32;
                    return DepthScale::fit(&[Anchor { d: mean_d, inv: mean_i, weight: 1.0 }]).with_anchors(n);
                }
                let a = ((sw * sdi - sd * si) / det) as f32;
                let b = ((si - a as f64 * sd) / sw) as f32;
                if a <= 0.0 {
                    // Depth and size disagree: trust the nearest-to-median anchor.
                    let mut inv: Vec<f32> = anchors.iter().map(|x| x.inv).collect();
                    inv.sort_by(f32::total_cmp);
                    let mid = inv[inv.len() / 2];
                    let pick = anchors.iter().min_by(|p, q| (p.inv - mid).abs().total_cmp(&(q.inv - mid).abs())).copied().expect("anchors");
                    return DepthScale::fit(&[pick]).with_anchors(n);
                }
                DepthScale { a, b, anchors: n }.clamped()
            }
        }
    }

    fn with_anchors(mut self, n: u32) -> DepthScale {
        self.anchors = n;
        self
    }

    fn clamped(mut self) -> DepthScale {
        // Inverse distance at d = 0 (far) must stay positive and at d = 1
        // below the nearest distance allowed.
        self.b = self.b.clamp(1.0 / MAX_DISTANCE, 1.0);
        let top = 1.0 / MIN_DISTANCE;
        if self.a + self.b > top {
            self.a = top - self.b;
        }
        self.a = self.a.max(1e-3);
        self
    }

    /// Distance in metres at relative depth `d`.
    pub fn metres(&self, d: f32) -> f32 {
        let inv = self.a * d.clamp(0.0, 1.0) + self.b;
        (1.0 / inv.max(1.0 / MAX_DISTANCE)).clamp(MIN_DISTANCE, MAX_DISTANCE)
    }
}

/// The depth of a box in a relative depth map: the median over its middle
/// part (a person's torso rather than the background at their sides).
/// `depth` is `dw` x `dh`, boxes are normalised 0..1.
pub fn box_depth(depth: &[f32], dw: usize, dh: usize, x: f32, y: f32, w: f32, h: f32) -> Option<f32> {
    if depth.len() != dw * dh || dw == 0 || dh == 0 {
        return None;
    }
    let (cx, cy) = (x + w / 2.0, y + h / 2.0);
    let (hw, hh) = (w * 0.3, h * 0.3);
    let x0 = (((cx - hw) * dw as f32).floor().max(0.0) as usize).min(dw - 1);
    let x1 = (((cx + hw) * dw as f32).ceil() as usize).clamp(x0 + 1, dw);
    let y0 = (((cy - hh) * dh as f32).floor().max(0.0) as usize).min(dh - 1);
    let y1 = (((cy + hh) * dh as f32).ceil() as usize).clamp(y0 + 1, dh);
    let mut v: Vec<f32> = Vec::with_capacity((x1 - x0) * (y1 - y0));
    for yy in y0..y1 {
        v.extend(depth[yy * dw + x0..yy * dw + x1].iter().copied().filter(|d| d.is_finite()));
    }
    if v.is_empty() {
        return None;
    }
    v.sort_by(f32::total_cmp);
    Some(v[v.len() / 2])
}

/// A thing placed in space.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Placed {
    /// Camera-space centre of the box at its depth, metres.
    pub pos: [f32; 3],
    /// Width and height in metres.
    pub size: [f32; 2],
}

/// Places a normalised box (`x`, `y`, `w`, `h`) at relative depth `d`.
pub fn place(cam: &Camera, scale: &DepthScale, bbox: [f32; 4], d: f32) -> Placed {
    let z = scale.metres(d);
    let (u, v) = ((bbox[0] + bbox[2] / 2.0) * cam.width, (bbox[1] + bbox[3] / 2.0) * cam.height);
    Placed { pos: cam.unproject(u, v, z), size: [cam.metres(bbox[2] * cam.width, z), cam.metres(bbox[3] * cam.height, z)] }
}

/// "3.2 m" or "40 cm" for a length.
pub fn length_words(m: f32) -> String {
    if m < 0.95 {
        format!("{:.0} cm", (m * 100.0 / 5.0).round() * 5.0)
    } else if m < 10.0 {
        format!("{:.1} m", m)
    } else {
        format!("{:.0} m", m)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cam() -> Camera {
        Camera::new(1920.0, 1080.0, 63.0)
    }

    #[test]
    fn unproject_and_project_are_inverses() {
        let c = cam();
        for (u, v, z) in [(960.0, 540.0, 3.0), (100.0, 900.0, 7.5), (1800.0, 50.0, 0.8)] {
            let p = c.unproject(u, v, z);
            let (u2, v2) = c.project(p);
            assert!((u - u2).abs() < 1e-2 && (v - v2).abs() < 1e-2, "{u},{v} -> {u2},{v2}");
        }
        // The picture centre is straight ahead; right is +x, up is +y.
        let p = c.unproject(960.0, 540.0, 5.0);
        assert!(p[0].abs() < 1e-4 && p[1].abs() < 1e-4 && p[2] == 5.0);
        assert!(c.unproject(1500.0, 540.0, 5.0)[0] > 0.0);
        assert!(c.unproject(960.0, 100.0, 5.0)[1] > 0.0);
    }

    #[test]
    fn field_of_view_sets_the_focal_length() {
        let c = Camera::new(1000.0, 500.0, 90.0);
        assert!((c.focal - 500.0).abs() < 1e-2);
        // The left edge of the picture at 1 m is half a metre to the left.
        assert!((c.unproject(0.0, 250.0, 1.0)[0] + 1.0).abs() < 1e-3);
    }

    #[test]
    fn a_known_height_gives_the_distance() {
        let c = cam();
        // A 1.7 m person 540 px tall: z = f * H / h.
        let z = c.depth_of(1.7, 540.0);
        assert!((c.metres(540.0, z) - 1.7).abs() < 1e-4);
        assert!((z - c.focal * 1.7 / 540.0).abs() < 1e-3);
        // Twice as tall on screen is half as far.
        assert!((c.depth_of(1.7, 1080.0) * 2.0 - z).abs() < 1e-3);
    }

    #[test]
    fn two_anchors_fix_the_scale_and_it_reproduces_them() {
        let c = cam();
        // Truth: 1/z = 0.5 d + 0.02. A near person (d 0.9) and a far car (d 0.2).
        let truth = |d: f32| 1.0 / (0.5 * d + 0.02);
        let anchors: Vec<Anchor> = [(0.9f32, 1.7f32, "person"), (0.2, 1.5, "car")]
            .iter()
            .map(|&(d, hm, l)| {
                let z = truth(d);
                let px = c.focal * hm / z;
                Anchor::from_box(&c, &prior(l).unwrap(), px, d).unwrap()
            })
            .collect();
        let s = DepthScale::fit(&anchors);
        assert_eq!(s.anchors, 2);
        assert!((s.a - 0.5).abs() < 0.02 && (s.b - 0.02).abs() < 0.01, "{s:?}");
        for d in [0.2, 0.5, 0.9] {
            assert!((s.metres(d) / truth(d) - 1.0).abs() < 0.05, "d {d}: {} vs {}", s.metres(d), truth(d));
        }
    }

    #[test]
    fn one_anchor_is_exact_at_the_anchor() {
        let c = cam();
        let a = Anchor::from_box(&c, &prior("person").unwrap(), 400.0, 0.7).unwrap();
        let s = DepthScale::fit(&[a]);
        assert!((s.metres(0.7) - 1.0 / a.inv).abs() < 0.15, "{} vs {}", s.metres(0.7), 1.0 / a.inv);
        // Nearer pixels are nearer.
        assert!(s.metres(0.9) < s.metres(0.7) && s.metres(0.7) < s.metres(0.1));
    }

    #[test]
    fn contradicting_anchors_fall_back_to_one() {
        // The nearer pixel claims to be farther: depth and size disagree.
        let s = DepthScale::fit(&[Anchor { d: 0.9, inv: 0.1, weight: 1.0 }, Anchor { d: 0.2, inv: 0.5, weight: 1.0 }]);
        assert!(s.a > 0.0);
        assert!(s.metres(1.0) <= s.metres(0.0));
    }

    #[test]
    fn no_anchor_means_an_assumed_scale() {
        let s = DepthScale::fit(&[]);
        assert_eq!(s.anchors, 0);
        assert!((s.metres(1.0) - 1.0).abs() < 1e-3);
        assert!((s.metres(0.0) - 25.0).abs() < 0.1);
    }

    #[test]
    fn distances_stay_inside_the_limits() {
        let s = DepthScale { a: 50.0, b: 5.0, anchors: 1 }.clamped();
        assert!(s.metres(1.0) >= MIN_DISTANCE - 1e-4);
        let far = DepthScale { a: 0.001, b: 0.0001, anchors: 1 }.clamped();
        assert!(far.metres(0.0) <= MAX_DISTANCE + 1e-3);
    }

    #[test]
    fn a_placed_person_is_the_right_size() {
        let c = cam();
        let a = Anchor::from_box(&c, &prior("person").unwrap(), 540.0, 0.8).unwrap();
        let s = DepthScale::fit(&[a]);
        // A person box 540 px tall, 200 px wide, left of the middle.
        let p = place(&c, &s, [0.2, 0.25, 200.0 / 1920.0, 0.5], 0.8);
        assert!((p.size[1] - 1.7).abs() < 0.2, "{p:?}");
        assert!((p.size[0] - 0.63).abs() < 0.12, "{p:?}");
        assert!(p.pos[0] < 0.0, "left of the lens axis: {p:?}");
        assert!((p.pos[2] - c.depth_of(1.7, 540.0)).abs() < 0.5);
    }

    #[test]
    fn the_box_depth_is_the_middle_median() {
        // A 10 x 10 map: a near object (0.9) in the middle of a far room (0.1).
        let mut d = vec![0.1f32; 100];
        for y in 3..7 {
            for x in 3..7 {
                d[y * 10 + x] = 0.9;
            }
        }
        let m = box_depth(&d, 10, 10, 0.2, 0.2, 0.6, 0.6).unwrap();
        assert!((m - 0.9).abs() < 1e-6);
        assert!(box_depth(&d, 10, 9, 0.0, 0.0, 1.0, 1.0).is_none());
    }

    #[test]
    fn lengths_read_naturally() {
        assert_eq!(length_words(0.23), "25 cm");
        assert_eq!(length_words(1.74), "1.7 m");
        assert_eq!(length_words(23.4), "23 m");
    }
}
