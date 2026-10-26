// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Layer placement: the affine map from source pixels to canvas pixels,
//! built from a clip's transform and its motion pose, plus the transformed
//! bounds used for hit-testing and on-canvas handles.
//!
//! Conventions (shared with the project model):
//! - a scale of 1 shows the full source fitted inside the canvas;
//! - `position` is where the anchor lands, in canvas pixels from the
//!   canvas centre; `anchor` is in source pixels from the source centre and
//!   is the centre of scale and rotation;
//! - rotation is clockwise degrees (y points down);
//! - crop removes fractions of each source edge without moving the rest;
//! - flips mirror the source about its centre before anything else.
//!
//! A motion pose (`studio_fx::Pose` or our copy of it) then moves the placed
//! layer about its own centre: offset in fractions of the canvas, scale,
//! rotation, with opacity, blur and clip insets handled by the compositor.

/// Where a layer's picture is placed on the canvas.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    /// Anchor position in canvas pixels from the canvas centre.
    pub position: [f32; 2],
    /// Horizontal and vertical scale; 1 = fitted.
    pub scale: [f32; 2],
    /// Clockwise degrees.
    pub rotation: f32,
    /// Scale/rotation centre in source pixels from the source centre.
    pub anchor: [f32; 2],
    /// 0..1.
    pub opacity: f32,
    /// Fractions removed from the left, top, right and bottom edges.
    pub crop: [f32; 4],
    pub flip_x: bool,
    pub flip_y: bool,
}

impl Default for Placement {
    fn default() -> Placement {
        Placement { position: [0.0; 2], scale: [1.0; 2], rotation: 0.0, anchor: [0.0; 2], opacity: 1.0, crop: [0.0; 4], flip_x: false, flip_y: false }
    }
}

/// A motion preset's momentary displacement of a layer (mirrors
/// `studio_fx::Pose` so the renderer works without the `fx` feature).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    /// Offset as fractions of the canvas width and height.
    pub offset: [f32; 2],
    /// Scale about the layer centre.
    pub scale: [f32; 2],
    /// Clockwise degrees.
    pub rotation: f32,
    /// Multiplies the layer's opacity.
    pub opacity: f32,
    /// Blur radius in percent of the canvas height.
    pub blur: f32,
    /// Insets from the left, top, right and bottom as fractions of the
    /// layer; 0 = no clipping.
    pub clip: [f32; 4],
    /// Turn in 3D about the layer's x and y axes (degrees; positive tips
    /// the top, turns the right edge away), seen in perspective.
    pub turn: [f32; 2],
    /// Pushed away from the viewer, as a fraction of the canvas width
    /// (negative comes closer).
    pub depth: f32,
}

impl Pose {
    pub const REST: Pose = Pose { offset: [0.0; 2], scale: [1.0; 2], rotation: 0.0, opacity: 1.0, blur: 0.0, clip: [0.0; 4], turn: [0.0; 2], depth: 0.0 };

    /// Whether the pose turns or moves the layer in depth.
    pub fn is_3d(&self) -> bool {
        self.turn != [0.0; 2] || self.depth != 0.0
    }

    pub fn is_rest(&self) -> bool {
        *self == Pose::REST
    }
}

impl Default for Pose {
    fn default() -> Pose {
        Pose::REST
    }
}

/// A 2D affine transform `[x', y'] = m × [x, y, 1]`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Affine(pub [[f32; 3]; 2]);

impl Affine {
    pub const IDENTITY: Affine = Affine([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]);

    pub fn translate(tx: f32, ty: f32) -> Affine {
        Affine([[1.0, 0.0, tx], [0.0, 1.0, ty]])
    }

    pub fn scale(sx: f32, sy: f32) -> Affine {
        Affine([[sx, 0.0, 0.0], [0.0, sy, 0.0]])
    }

    /// Clockwise rotation (y down) by `deg`.
    pub fn rotate(deg: f32) -> Affine {
        let (s, c) = deg.to_radians().sin_cos();
        Affine([[c, -s, 0.0], [s, c, 0.0]])
    }

    /// `self ∘ other`: applies `other` first, then `self`.
    pub fn then_after(&self, other: &Affine) -> Affine {
        let a = &self.0;
        let b = &other.0;
        let mut m = [[0.0f32; 3]; 2];
        for r in 0..2 {
            for c in 0..3 {
                m[r][c] = a[r][0] * b[0][c] + a[r][1] * b[1][c] + if c == 2 { a[r][2] } else { 0.0 };
            }
        }
        Affine(m)
    }

    #[inline]
    pub fn apply(&self, p: [f32; 2]) -> [f32; 2] {
        let m = &self.0;
        [m[0][0] * p[0] + m[0][1] * p[1] + m[0][2], m[1][0] * p[0] + m[1][1] * p[1] + m[1][2]]
    }

    pub fn determinant(&self) -> f32 {
        self.0[0][0] * self.0[1][1] - self.0[0][1] * self.0[1][0]
    }

    /// Inverse, or `None` when degenerate (zero scale).
    pub fn inverse(&self) -> Option<Affine> {
        let m = &self.0;
        let det = self.determinant();
        if det.abs() < 1e-12 || !det.is_finite() {
            return None;
        }
        let inv = 1.0 / det;
        let a = m[1][1] * inv;
        let b = -m[0][1] * inv;
        let c = -m[1][0] * inv;
        let d = m[0][0] * inv;
        let tx = -(a * m[0][2] + b * m[1][2]);
        let ty = -(c * m[0][2] + d * m[1][2]);
        Some(Affine([[a, b, tx], [c, d, ty]]))
    }

    /// Largest singular value: how much a unit length can grow.
    pub fn max_scale(&self) -> f32 {
        let m = &self.0;
        let (a, b, c, d) = (m[0][0], m[0][1], m[1][0], m[1][1]);
        let s1 = a * a + b * b + c * c + d * d;
        let s2 = ((a * a + b * b - c * c - d * d).powi(2) + 4.0 * (a * c + b * d).powi(2)).sqrt();
        ((s1 + s2) * 0.5).max(0.0).sqrt()
    }

    /// Smallest singular value: how much a unit length can shrink.
    pub fn min_scale(&self) -> f32 {
        let m = &self.0;
        let (a, b, c, d) = (m[0][0], m[0][1], m[1][0], m[1][1]);
        let s1 = a * a + b * b + c * c + d * d;
        let s2 = ((a * a + b * b - c * c - d * d).powi(2) + 4.0 * (a * c + b * d).powi(2)).sqrt();
        ((s1 - s2) * 0.5).max(0.0).sqrt()
    }

    /// Flattened as a `mat3x2`-compatible array of six floats, row major
    /// (a, b, tx, c, d, ty).
    pub fn to_array(&self) -> [f32; 6] {
        let m = &self.0;
        [m[0][0], m[0][1], m[0][2], m[1][0], m[1][1], m[1][2]]
    }
}

/// A projective map `[x', y', w'] = m × [x, y, 1]`, points divided by
/// `w'`: how a flat layer turned in 3D lands on the screen. For layers seen
/// through a camera `w'` is the depth in front of the lens.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Homography(pub [[f32; 3]; 3]);

impl Homography {
    pub const IDENTITY: Homography = Homography([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);

    pub fn from_affine(a: &Affine) -> Homography {
        let m = &a.0;
        Homography([m[0], m[1], [0.0, 0.0, 1.0]])
    }

    /// `self ∘ other`: applies `other` first, then `self`.
    pub fn then_after(&self, other: &Homography) -> Homography {
        let (a, b) = (&self.0, &other.0);
        Homography(std::array::from_fn(|r| std::array::from_fn(|c| a[r][0] * b[0][c] + a[r][1] * b[1][c] + a[r][2] * b[2][c])))
    }

    /// `self ∘ a`.
    pub fn then_after_affine(&self, a: &Affine) -> Homography {
        self.then_after(&Homography::from_affine(a))
    }

    /// The homogeneous image `[x', y', w']` of a point.
    #[inline]
    pub fn apply_w(&self, p: [f32; 2]) -> [f32; 3] {
        let m = &self.0;
        std::array::from_fn(|r| m[r][0] * p[0] + m[r][1] * p[1] + m[r][2])
    }

    /// The image of a point (divided through).
    #[inline]
    pub fn apply(&self, p: [f32; 2]) -> [f32; 2] {
        let q = self.apply_w(p);
        let w = if q[2].abs() < 1e-12 { 1e-12f32.copysign(q[2]) } else { q[2] };
        [q[0] / w, q[1] / w]
    }

    /// Inverse, or `None` when degenerate (seen edge-on).
    pub fn inverse(&self) -> Option<Homography> {
        let m = &self.0;
        let c = |r0: usize, r1: usize, c0: usize, c1: usize| m[r0][c0] * m[r1][c1] - m[r0][c1] * m[r1][c0];
        let adj = [
            [c(1, 2, 1, 2), -c(0, 2, 1, 2), c(0, 1, 1, 2)],
            [-c(1, 2, 0, 2), c(0, 2, 0, 2), -c(0, 1, 0, 2)],
            [c(1, 2, 0, 1), -c(0, 2, 0, 1), c(0, 1, 0, 1)],
        ];
        let det = m[0][0] * adj[0][0] + m[0][1] * adj[1][0] + m[0][2] * adj[2][0];
        if det.abs() < 1e-20 || !det.is_finite() {
            return None;
        }
        // Scale so the largest entry is about 1: f32 stays precise.
        let big = adj.iter().flatten().fold(0.0f32, |a, v| a.max(v.abs())).max(1e-30);
        Some(Homography(adj.map(|row| row.map(|v| v / big))))
    }

    /// Whether the map is affine (no perspective).
    pub fn is_affine(&self) -> bool {
        let m = &self.0;
        m[2][0] == 0.0 && m[2][1] == 0.0 && m[2][2] == 1.0
    }

    /// The affine part, when [`Homography::is_affine`].
    pub fn to_affine(&self) -> Affine {
        let m = &self.0;
        Affine([m[0], m[1]])
    }

    /// The local linear map at `p` (how a small step there is stretched).
    pub fn jacobian_at(&self, p: [f32; 2]) -> Affine {
        let m = &self.0;
        let q = self.apply_w(p);
        let w = if q[2].abs() < 1e-6 { 1e-6 } else { q[2] };
        let (x, y) = (q[0] / w, q[1] / w);
        Affine([
            [(m[0][0] - x * m[2][0]) / w, (m[0][1] - x * m[2][1]) / w, 0.0],
            [(m[1][0] - y * m[2][0]) / w, (m[1][1] - y * m[2][1]) / w, 0.0],
        ])
    }

    /// Rows for a shader: three `vec4`s.
    pub fn rows(&self) -> [[f32; 4]; 3] {
        self.0.map(|r| [r[0], r[1], r[2], 0.0])
    }
}

/// The scale at which a `source` fits inside a `canvas` (the meaning of
/// scale 1).
pub fn fit_scale(canvas: [f32; 2], source: [f32; 2]) -> f32 {
    if source[0] <= 0.0 || source[1] <= 0.0 {
        return 1.0;
    }
    (canvas[0] / source[0]).min(canvas[1] / source[1])
}

/// Map from full (uncropped) source pixels to canvas pixels for a placement
/// alone.
pub fn placement_matrix(canvas: [f32; 2], source: [f32; 2], p: &Placement) -> Affine {
    let fit = fit_scale(canvas, source);
    let sx = fit * p.scale[0] * if p.flip_x { -1.0 } else { 1.0 };
    let sy = fit * p.scale[1] * if p.flip_y { -1.0 } else { 1.0 };
    // Source centre + anchor → origin, scale, rotate, then to canvas centre
    // + position.
    let to_origin = Affine::translate(-(source[0] * 0.5 + p.anchor[0]), -(source[1] * 0.5 + p.anchor[1]));
    let scaled = Affine::scale(sx, sy).then_after(&to_origin);
    let rotated = Affine::rotate(p.rotation).then_after(&scaled);
    Affine::translate(canvas[0] * 0.5 + p.position[0], canvas[1] * 0.5 + p.position[1]).then_after(&rotated)
}

/// Full map including the motion pose: source pixels → canvas pixels.
pub fn layer_matrix(canvas: [f32; 2], source: [f32; 2], p: &Placement, pose: &Pose) -> Affine {
    let base = placement_matrix(canvas, source, p);
    if pose.offset == [0.0; 2] && pose.scale == [1.0; 2] && pose.rotation == 0.0 {
        return base;
    }
    // The pose turns and scales about the placed layer's centre (the
    // centre of the cropped picture), then shifts by a fraction of the
    // canvas.
    let crop_centre = cropped_centre(source, p.crop);
    let c = base.apply(crop_centre);
    let about = Affine::translate(-c[0], -c[1]);
    let s = Affine::scale(pose.scale[0], pose.scale[1]).then_after(&about);
    let r = Affine::rotate(pose.rotation).then_after(&s);
    let back = Affine::translate(c[0] + pose.offset[0] * canvas[0], c[1] + pose.offset[1] * canvas[1]).then_after(&r);
    back.then_after(&base)
}

/// Pixel rectangle `[x0, y0, x1, y1]` of the cropped region in source
/// pixels (at least one pixel).
pub fn crop_rect(source: [f32; 2], crop: [f32; 4]) -> [f32; 4] {
    let l = crop[0].clamp(0.0, 1.0) * source[0];
    let t = crop[1].clamp(0.0, 1.0) * source[1];
    let r = source[0] - crop[2].clamp(0.0, 1.0) * source[0];
    let b = source[1] - crop[3].clamp(0.0, 1.0) * source[1];
    [l.min(r), t.min(b), r.max(l), b.max(t)]
}

pub fn cropped_centre(source: [f32; 2], crop: [f32; 4]) -> [f32; 2] {
    let r = crop_rect(source, crop);
    [(r[0] + r[2]) * 0.5, (r[1] + r[3]) * 0.5]
}

/// The visible rectangle in source pixels after crop and the pose's clip
/// insets.
pub fn visible_rect(source: [f32; 2], crop: [f32; 4], clip: [f32; 4]) -> [f32; 4] {
    let r = crop_rect(source, crop);
    let w = r[2] - r[0];
    let h = r[3] - r[1];
    let l = r[0] + clip[0].clamp(0.0, 1.0) * w;
    let t = r[1] + clip[1].clamp(0.0, 1.0) * h;
    let rr = r[2] - clip[2].clamp(0.0, 1.0) * w;
    let b = r[3] - clip[3].clamp(0.0, 1.0) * h;
    [l.min(rr), t.min(b), rr.max(l), b.max(t)]
}

/// The four canvas-space corners (top-left, top-right, bottom-right,
/// bottom-left in source order) of a layer's visible rectangle.
pub fn layer_quad(m: &Affine, visible: [f32; 4]) -> [[f32; 2]; 4] {
    [m.apply([visible[0], visible[1]]), m.apply([visible[2], visible[1]]), m.apply([visible[2], visible[3]]), m.apply([visible[0], visible[3]])]
}

/// Axis-aligned canvas bounds `[x0, y0, x1, y1]` of a quad.
pub fn quad_bounds(q: &[[f32; 2]; 4]) -> [f32; 4] {
    let mut b = [f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY];
    for p in q {
        b[0] = b[0].min(p[0]);
        b[1] = b[1].min(p[1]);
        b[2] = b[2].max(p[0]);
        b[3] = b[3].max(p[1]);
    }
    b
}

/// Corners of a visible rectangle through a projective map. Corners behind
/// the lens (depth below `near`) come out far off-screen in their
/// direction, so bounds still cover what is drawn.
pub fn proj_quad(h: &Homography, visible: [f32; 4], near: f32) -> [[f32; 2]; 4] {
    [[visible[0], visible[1]], [visible[2], visible[1]], [visible[2], visible[3]], [visible[0], visible[3]]].map(|p| {
        let q = h.apply_w(p);
        let w = q[2].max(near * 1e-3);
        [q[0] / w, q[1] / w]
    })
}

/// [`hit`] for a projective map: the point must land on the layer in front
/// of the lens (depth at least `near`).
pub fn hit_proj(h: &Homography, visible: [f32; 4], point: [f32; 2], near: f32) -> bool {
    let Some(inv) = h.inverse() else { return false };
    let s = inv.apply(point);
    if h.apply_w(s)[2] < near {
        return false;
    }
    s[0] >= visible[0] && s[0] <= visible[2] && s[1] >= visible[1] && s[1] <= visible[3]
}

/// Whether a canvas point lies inside the layer's visible rectangle: the
/// point is mapped back to source space and tested against the rectangle,
/// which is exact for any rotation, scale, crop and flip.
pub fn hit(m: &Affine, visible: [f32; 4], point: [f32; 2]) -> bool {
    let Some(inv) = m.inverse() else { return false };
    let s = inv.apply(point);
    s[0] >= visible[0] && s[0] <= visible[2] && s[1] >= visible[1] && s[1] <= visible[3]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: [f32; 2], b: [f32; 2]) -> bool {
        (a[0] - b[0]).abs() < 1e-3 && (a[1] - b[1]).abs() < 1e-3
    }

    #[test]
    fn fitted_source_is_centred() {
        let m = placement_matrix([1920.0, 1080.0], [1280.0, 720.0], &Placement::default());
        assert!(close(m.apply([640.0, 360.0]), [960.0, 540.0]));
        assert!(close(m.apply([0.0, 0.0]), [0.0, 0.0]));
        assert!(close(m.apply([1280.0, 720.0]), [1920.0, 1080.0]));
        // Portrait source in a landscape canvas: fits by height.
        let m = placement_matrix([1920.0, 1080.0], [1080.0, 1920.0], &Placement::default());
        assert!(close(m.apply([0.0, 0.0]), [960.0 - 303.75, 0.0]));
    }

    #[test]
    fn position_moves_anchor() {
        let p = Placement { position: [100.0, -50.0], anchor: [10.0, 0.0], ..Default::default() };
        let m = placement_matrix([200.0, 200.0], [200.0, 200.0], &p);
        // The anchor (source centre + 10 px) lands at centre + position.
        assert!(close(m.apply([110.0, 100.0]), [200.0, 50.0]));
    }

    #[test]
    fn rotation_is_clockwise() {
        let p = Placement { rotation: 90.0, ..Default::default() };
        let m = placement_matrix([200.0, 200.0], [200.0, 200.0], &p);
        // The point to the right of the centre goes to below the centre.
        assert!(close(m.apply([150.0, 100.0]), [100.0, 150.0]));
    }

    #[test]
    fn flip_mirrors_about_centre() {
        let p = Placement { flip_x: true, ..Default::default() };
        let m = placement_matrix([200.0, 200.0], [200.0, 200.0], &p);
        assert!(close(m.apply([0.0, 0.0]), [200.0, 0.0]));
    }

    #[test]
    fn inverse_round_trips() {
        let p = Placement { position: [33.0, -12.0], scale: [0.7, 1.3], rotation: 37.0, anchor: [5.0, -9.0], flip_y: true, ..Default::default() };
        let m = placement_matrix([1920.0, 1080.0], [640.0, 480.0], &p);
        let inv = m.inverse().unwrap();
        for pt in [[0.0, 0.0], [100.0, 200.0], [640.0, 480.0]] {
            assert!(close(inv.apply(m.apply(pt)), pt));
        }
        assert!(Affine::scale(0.0, 1.0).inverse().is_none());
    }

    #[test]
    fn pose_offset_is_canvas_fraction() {
        let pose = Pose { offset: [0.5, 0.0], ..Pose::REST };
        let m = layer_matrix([200.0, 100.0], [200.0, 100.0], &Placement::default(), &pose);
        assert!(close(m.apply([100.0, 50.0]), [200.0, 50.0]));
        // Pose scale is about the layer centre, so the centre stays put.
        let pose = Pose { scale: [0.5, 0.5], ..Pose::REST };
        let m = layer_matrix([200.0, 100.0], [200.0, 100.0], &Placement::default(), &pose);
        assert!(close(m.apply([100.0, 50.0]), [100.0, 50.0]));
        assert!(close(m.apply([0.0, 0.0]), [50.0, 25.0]));
    }

    #[test]
    fn hit_respects_crop_and_rotation() {
        let p = Placement { rotation: 45.0, crop: [0.0, 0.0, 0.5, 0.0], ..Default::default() };
        let m = placement_matrix([200.0, 200.0], [200.0, 200.0], &p);
        let vis = visible_rect([200.0, 200.0], p.crop, [0.0; 4]);
        assert_eq!(vis, [0.0, 0.0, 100.0, 200.0]);
        // The right half is cropped away: the point just right of centre
        // along the rotated x axis is not hit.
        let right = m.apply([150.0, 100.0]);
        assert!(!hit(&m, vis, right));
        let left = m.apply([50.0, 100.0]);
        assert!(hit(&m, vis, left));
        let b = quad_bounds(&layer_quad(&m, vis));
        assert!(b[0] < b[2] && b[1] < b[3]);
    }

    #[test]
    fn homography_inverse_and_jacobian() {
        let h = Homography([[2.0, 0.3, 5.0], [0.1, 1.5, -3.0], [0.001, 0.002, 1.0]]);
        let inv = h.inverse().unwrap();
        for p in [[0.0, 0.0], [100.0, 50.0], [-20.0, 300.0]] {
            assert!(close(inv.apply(h.apply(p)), p), "{p:?}");
        }
        let a = Affine::rotate(30.0).then_after(&Affine::scale(2.0, 0.5));
        let ha = Homography::from_affine(&a);
        assert!(ha.is_affine());
        assert!((ha.jacobian_at([3.0, 4.0]).max_scale() - 2.0).abs() < 1e-5);
    }

    #[test]
    fn singular_values() {
        let m = Affine::rotate(30.0).then_after(&Affine::scale(2.0, 0.5));
        assert!((m.max_scale() - 2.0).abs() < 1e-5);
        assert!((m.min_scale() - 0.5).abs() < 1e-5);
    }
}
