// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Ground shadows for cut-out layers: a character over a painted backdrop
//! sits on its ground when a soft dark patch hugs its feet and its shape
//! falls across the ground away from the light.
//!
//! The shadow is made from the layer's own finished picture (after its
//! pins bend it), so it acts with the layer. Each pass lays the picture on
//! the ground with a shear about the feet: a point `h` pixels above the
//! feet lands `lean × h` away along the ground. Its strength falls off with
//! height (`fade`), it is blurred (`sigma`) and drawn under the layer in
//! the shadow colour. Three passes: a tight contact shadow right under the
//! feet, and the cast shadow twice, sharp and dark near the feet and wide
//! and faint further out, as real shadows soften with distance.
//!
//! The CPU builds each pass's picture here; the GPU does the same in
//! `shaders/shadow.wgsl` and blurs with the shared blur pass.

use crate::frame::Frame;
use crate::transform::{Affine, crop_rect, layer_quad, quad_bounds};

/// A layer's ground shadow, in its own terms.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShadowDesc {
    /// Where the layer meets the ground: 0..1 across and down its picture
    /// after crop, before pins bend it.
    pub feet: [f32; 2],
    /// A ground line (canvas pixels from the top) the shadow stays on
    /// instead of following the feet.
    pub ground: Option<f32>,
    /// Direction the cast shadow falls on screen, degrees clockwise from
    /// pointing right: 90 towards the camera, 270 away towards the horizon.
    pub angle: f32,
    /// Length of the cast shadow, times the layer's height on screen.
    pub length: f32,
    /// Strength of the cast shadow (0..1).
    pub opacity: f32,
    /// Blur of the cast shadow, times the layer's height.
    pub softness: f32,
    /// Strength of the contact shadow under the feet (0..1).
    pub contact: f32,
    /// Linear RGB.
    pub colour: [f32; 3],
}

impl Default for ShadowDesc {
    fn default() -> ShadowDesc {
        ShadowDesc { feet: [0.5, 0.97], ground: None, angle: 60.0, length: 0.45, opacity: 0.45, softness: 0.04, contact: 0.6, colour: [0.02, 0.025, 0.04] }
    }
}

/// One pass of a shadow, in target pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pass {
    /// Layer picture px → target px with the shear applied.
    pub map: Affine,
    /// Target px → layer picture px (inverse of `map`).
    pub inv: Affine,
    /// The layer's own map's y row (picture px → target y), for heights.
    pub row_y: [f32; 3],
    /// Target y of the ground at the feet.
    pub feet_y: f32,
    /// Height (target px) over which the shadow falls to 1/e; 0 = none.
    pub fade: f32,
    /// How quickly the picture's alpha saturates the pass (a contact
    /// shadow is solid where the feet are, even under thin legs).
    pub gain: f32,
    /// Blur, target px.
    pub sigma: f32,
    /// Peak strength (layer opacity included).
    pub strength: f32,
    /// Region of the target the pass draws into: x, y, width, height.
    pub region: [u32; 4],
}

/// Where the feet are (target px) and how tall the layer stands (target
/// px), from the map of its full source picture to the target.
pub fn anchor(desc: &ShadowDesc, source_to_target: &Affine, source_size: [f32; 2], crop: [f32; 4], s: f32) -> ([f32; 2], f32) {
    let r = crop_rect(source_size, crop);
    let at = |u: f32, v: f32| source_to_target.apply([r[0] + u * (r[2] - r[0]), r[1] + v * (r[3] - r[1])]);
    let mut feet = at(desc.feet[0], desc.feet[1]);
    if let Some(g) = desc.ground {
        feet[1] = g * s;
    }
    let (top, bottom) = (at(0.5, 0.0), at(0.5, 1.0));
    let height = ((top[0] - bottom[0]).powi(2) + (top[1] - bottom[1]).powi(2)).sqrt();
    (feet, height)
}

/// The passes for a layer drawn with `matrix` (picture px → target px)
/// over `visible` (picture px), at `opacity`, on a `target`-sized frame.
pub fn passes(desc: &ShadowDesc, matrix: &Affine, visible: [f32; 4], opacity: f32, feet: [f32; 2], height: f32, target: (u32, u32)) -> Vec<Pass> {
    if height <= 1.0 || opacity <= 0.0 {
        return Vec::new();
    }
    let (sin, cos) = desc.angle.to_radians().sin_cos();
    let len = desc.length.max(0.0);
    let soft = desc.softness.max(0.0) * height;
    // (lean across, lean down, drop, fade, gain, sigma, strength); drop
    // moves the pass down the screen, in layer heights.
    let mut list = Vec::new();
    if desc.contact > 0.0 {
        // The legs and feet flattened into a dark patch that straddles the
        // ground line.
        list.push((0.0, -0.2, 0.025, 0.15 * height, 2.5, (0.015 * height).max(1.0), desc.contact));
    }
    if desc.opacity > 0.0 && len > 0.0 {
        let (lx, ly) = (len * cos, len * sin);
        // A shadow lying exactly along the screen would be a line.
        let ly = if ly.abs() < 0.03 { 0.03f32.copysign(if sin == 0.0 { -1.0 } else { sin }) } else { ly };
        list.push((lx, ly, 0.0, 0.8 * height, 1.0, (0.35 * soft).max(0.6), desc.opacity * 0.7));
        list.push((lx, ly, 0.0, 2.5 * height, 1.0, soft.max(1.0), desc.opacity * 0.5));
    }
    let row_y = matrix.0[1];
    list.into_iter()
        .filter_map(|(lx, ly, drop, fade, gain, sigma, strength)| {
            // About the feet: (dx, dy) → (dx − lx·dy, −ly·dy); dy < 0 above.
            let shear = Affine::translate(feet[0], feet[1] + drop * height).then_after(&Affine([[1.0, -lx, 0.0], [0.0, -ly, 0.0]])).then_after(&Affine::translate(-feet[0], -feet[1]));
            let map = shear.then_after(matrix);
            let inv = map.inverse()?;
            let b = quad_bounds(&layer_quad(&map, visible));
            let pad = 3.0 * sigma + 2.0;
            let x0 = (b[0] - pad).floor().max(0.0);
            let y0 = (b[1] - pad).floor().max(0.0);
            let x1 = (b[2] + pad).ceil().min(target.0 as f32);
            let y1 = (b[3] + pad).ceil().min(target.1 as f32);
            if x1 <= x0 || y1 <= y0 {
                return None;
            }
            let region = [x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32];
            Some(Pass { map, inv, row_y, feet_y: feet[1], fade, gain, sigma, strength: strength.clamp(0.0, 1.0) * opacity, region })
        })
        .collect()
}

/// How much of the shadow a picture point gives: its alpha, faded by its
/// height above the feet, saturated by the pass's gain.
#[inline]
pub fn weight(pass: &Pass, alpha: f32, p: [f32; 2]) -> f32 {
    let y = pass.row_y[0] * p[0] + pass.row_y[1] * p[1] + pass.row_y[2];
    let h = (pass.feet_y - y).max(0.0);
    let fall = if pass.fade > 0.0 { (-h / pass.fade).exp() } else { 1.0 };
    (alpha * fall * pass.gain).min(1.0) * pass.strength
}

/// The unblurred picture of one pass over its region (premultiplied
/// `colour`), sampled from the layer's picture.
pub fn silhouette(pass: &Pass, image: &Frame, visible: [f32; 4], colour: [f32; 3]) -> Frame {
    let [x0, y0, w, h] = pass.region;
    let mut out = Frame::new(w, h);
    studio_media::par::par_rows(&mut out.pixels, w as usize, 8, |ry, row| {
        let y = (y0 as usize + ry) as f32 + 0.5;
        for (rx, px) in row.iter_mut().enumerate() {
            let p = pass.inv.apply([(x0 as usize + rx) as f32 + 0.5, y]);
            if p[0] < visible[0] || p[0] > visible[2] || p[1] < visible[1] || p[1] > visible[3] {
                continue;
            }
            let a = weight(pass, image.sample(p[0], p[1])[3], p);
            if a > 0.0 {
                *px = [colour[0] * a, colour[1] * a, colour[2] * a, a];
            }
        }
    });
    out
}

/// Linear RGB of an sRGB-encoded colour (for colours given in 0..1 sRGB).
pub fn srgb_to_linear(c: [f32; 3]) -> [f32; 3] {
    c.map(|v| if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 20 × 40 opaque bar standing with its feet at (100, 100).
    fn bar() -> (Frame, Affine, [f32; 4]) {
        let f = Frame::filled(20, 40, [1.0, 1.0, 1.0, 1.0]);
        (f, Affine::translate(90.0, 60.0), [0.0, 0.0, 20.0, 40.0])
    }

    #[test]
    fn the_cast_shadow_falls_along_its_angle_and_fades_with_height() {
        let (img, m, vis) = bar();
        let d = ShadowDesc { contact: 0.0, angle: 60.0, length: 1.0, ..Default::default() };
        let ps = passes(&d, &m, vis, 1.0, [100.0, 100.0], 40.0, (400, 200));
        assert_eq!(ps.len(), 2);
        let p = &ps[0];
        let s = silhouette(p, &img, vis, [0.0; 3]);
        let r = p.region;
        // Its weight lies down and to the right of the feet, along 60°.
        let (mut sx, mut sy, mut n) = (0.0, 0.0, 0.0);
        for y in 0..r[3] {
            for x in 0..r[2] {
                let a = s.get(x, y)[3];
                sx += a * (r[0] + x) as f32;
                sy += a * (r[1] + y) as f32;
                n += a;
            }
        }
        let dir = ((sy / n - 100.0).atan2(sx / n - 100.0)).to_degrees();
        assert!((dir - 60.0).abs() < 12.0, "{dir}");
        // Strong where the bar meets the ground, faint from its top.
        let g = |px: [f32; 2]| {
            let q = p.map.apply(px);
            s.get(q[0] as u32 - r[0], q[1] as u32 - r[1])[3]
        };
        let (near, far) = (g([10.0, 36.0]), g([10.0, 4.0]));
        assert!(near > 0.15 && far < near * 0.4, "near {near} far {far}");
    }

    #[test]
    fn the_contact_shadow_hugs_the_feet() {
        let (img, m, vis) = bar();
        let d = ShadowDesc { opacity: 0.0, ..Default::default() };
        let ps = passes(&d, &m, vis, 1.0, [100.0, 100.0], 40.0, (400, 200));
        assert_eq!(ps.len(), 1);
        let s = silhouette(&ps[0], &img, vis, [0.0; 3]);
        let r = ps[0].region;
        // Flat: a quarter of the bar's height plus the blur, its width.
        assert!(r[3] < 24 && r[2] >= 20, "{r:?}");
        let under = s.get(100 - r[0], 99 - r[1])[3];
        assert!(under > 0.3, "{under}");
    }

    #[test]
    fn a_ground_line_holds_the_shadow_when_the_layer_rises() {
        let d = ShadowDesc { ground: Some(100.0), ..Default::default() };
        let m = Affine::translate(90.0, 20.0);
        let (feet, h) = anchor(&d, &m, [20.0, 40.0], [0.0; 4], 1.0);
        assert_eq!(feet[1], 100.0);
        assert!((h - 40.0).abs() < 1e-3);
        let none = ShadowDesc { ground: None, ..d };
        assert!((anchor(&none, &m, [20.0, 40.0], [0.0; 4], 1.0).0[1] - 58.8).abs() < 0.01);
    }
}
