// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Masks: shapes as signed distance fields, Gaussian feather, expansion,
//! inversion, opacity, and add / subtract / intersect combination.
//!
//! Masks live in the layer's own space (normalised 0..1 across the cropped
//! layer), so they move with the layer. Feather is a Gaussian of the edge:
//! the coverage of a point is the Gaussian CDF of its signed distance,
//! which equals a Gaussian blur of the hard edge exactly for straight
//! edges and very closely for curved ones, and costs one `erf` per pixel
//! instead of a blur pass. The WGSL in `composite.wgsl` evaluates the same
//! functions, so CPU and GPU masks agree.
//!
//! Units: `feather` and `expansion` are fractions of the layer's shorter
//! side (0..1; the UI shows feather as 0–100 %); feather 1.0 gives a sigma
//! of half the shorter side. `size` is the full extent as fractions of the
//! layer size; `centre` is in 0..1 layer coordinates.

use std::f32::consts::PI;

/// How a mask combines with the ones before it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
#[repr(u32)]
pub enum MaskMode {
    /// Union (`a + b − ab`).
    #[default]
    Add = 0,
    /// Cut out (`a × (1 − b)`).
    Subtract = 1,
    /// Keep the overlap (`a × b`).
    Intersect = 2,
}

/// A point of a free Bézier path with handles relative to the point, all in
/// 0..1 layer coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PathPoint {
    pub point: [f32; 2],
    pub handle_in: [f32; 2],
    pub handle_out: [f32; 2],
}

/// Mask shapes.
#[derive(Clone, Debug, PartialEq)]
pub enum MaskShape {
    /// Rectangle with `roundness` × half the shorter side as corner radius.
    Rectangle,
    Ellipse,
    /// Everything above the line through the centre is inside (rotation
    /// turns the line).
    Line,
    /// A horizontal band of `size.y` height, mirrored around the centre.
    Mirror,
    /// Five-point star fitted into `size`.
    Star,
    /// Heart fitted into `size`.
    Heart,
    /// Closed Bézier path.
    Path(Vec<PathPoint>),
}

impl MaskShape {
    pub fn id(&self) -> &'static str {
        match self {
            MaskShape::Rectangle => "rectangle",
            MaskShape::Ellipse => "ellipse",
            MaskShape::Line => "line",
            MaskShape::Mirror => "mirror",
            MaskShape::Star => "star",
            MaskShape::Heart => "heart",
            MaskShape::Path(_) => "path",
        }
    }

    /// Shape code for the shader.
    pub fn code(&self) -> u32 {
        match self {
            MaskShape::Rectangle => 0,
            MaskShape::Ellipse => 1,
            MaskShape::Line => 2,
            MaskShape::Mirror => 3,
            MaskShape::Star => 4,
            MaskShape::Heart => 5,
            MaskShape::Path(_) => 6,
        }
    }
}

/// One mask with all its parameters.
#[derive(Clone, Debug, PartialEq)]
pub struct MaskSpec {
    pub shape: MaskShape,
    pub mode: MaskMode,
    pub invert: bool,
    /// Centre in 0..1 layer coordinates.
    pub centre: [f32; 2],
    /// Full extent as fractions of the layer width and height.
    pub size: [f32; 2],
    /// Clockwise degrees.
    pub rotation: f32,
    /// Fraction of the shorter layer side (sigma = feather × shorter / 2).
    pub feather: f32,
    /// Grows (positive) or shrinks the shape, fraction of the shorter side.
    pub expansion: f32,
    /// 0..1.
    pub opacity: f32,
    /// Corner roundness of rectangles, 0..1.
    pub roundness: f32,
    /// Pins bending a path (0..1 layer coordinates, like the path).
    pub pins: Vec<crate::puppet::PinDesc>,
}

impl Default for MaskSpec {
    fn default() -> MaskSpec {
        MaskSpec {
            shape: MaskShape::Rectangle,
            mode: MaskMode::Add,
            invert: false,
            centre: [0.5, 0.5],
            size: [0.5, 0.5],
            rotation: 0.0,
            feather: 0.0,
            expansion: 0.0,
            opacity: 1.0,
            roundness: 0.0,
            pins: Vec::new(),
        }
    }
}

/// Abramowitz & Stegun 7.1.26 error function (max error 1.5e-7); the
/// shader uses the same polynomial.
#[inline]
pub fn erf(x: f32) -> f32 {
    let s = if x < 0.0 { -1.0 } else { 1.0 };
    let x = x.abs();
    let t = 1.0 / (1.0 + 0.3275911 * x);
    let y = 1.0 - (((((1.061405429 * t - 1.453152027) * t) + 1.421413741) * t - 0.284496736) * t + 0.254829592) * t * (-x * x).exp();
    s * y
}

/// Coverage (0..1) of a point with signed distance `d` (negative inside)
/// from an edge feathered with Gaussian `sigma` pixels. Without feather,
/// one pixel of anti-aliasing.
#[inline]
pub fn coverage(d: f32, sigma: f32) -> f32 {
    if sigma <= 1e-4 {
        (0.5 - d).clamp(0.0, 1.0)
    } else {
        0.5 * (1.0 - erf(d / (sigma * std::f32::consts::SQRT_2)))
    }
}

#[inline]
fn length(x: f32, y: f32) -> f32 {
    (x * x + y * y).sqrt()
}

/// Rounded rectangle SDF, half extents `h`, corner radius `r`.
#[inline]
pub fn sdf_rect(p: [f32; 2], h: [f32; 2], r: f32) -> f32 {
    let r = r.min(h[0]).min(h[1]).max(0.0);
    let qx = p[0].abs() - (h[0] - r);
    let qy = p[1].abs() - (h[1] - r);
    length(qx.max(0.0), qy.max(0.0)) + qx.max(qy).min(0.0) - r
}

/// Ellipse SDF (a close approximation that is exact on the axes), radii `r`.
#[inline]
pub fn sdf_ellipse(p: [f32; 2], r: [f32; 2]) -> f32 {
    let rx = r[0].max(1e-6);
    let ry = r[1].max(1e-6);
    let k0 = length(p[0] / rx, p[1] / ry);
    let k1 = length(p[0] / (rx * rx), p[1] / (ry * ry));
    if k1 <= 1e-12 {
        return -rx.min(ry);
    }
    k0 * (k0 - 1.0) / k1
}

/// Exact signed distance to a closed polygon (even-odd inside rule).
pub fn sdf_polygon(p: [f32; 2], v: &[[f32; 2]]) -> f32 {
    if v.len() < 2 {
        return f32::INFINITY;
    }
    let mut d = f32::INFINITY;
    let mut inside = false;
    let n = v.len();
    let mut j = n - 1;
    for i in 0..n {
        let (a, b) = (v[j], v[i]);
        let ex = b[0] - a[0];
        let ey = b[1] - a[1];
        let wx = p[0] - a[0];
        let wy = p[1] - a[1];
        let ee = ex * ex + ey * ey;
        let t = if ee > 0.0 { ((wx * ex + wy * ey) / ee).clamp(0.0, 1.0) } else { 0.0 };
        let dx = wx - ex * t;
        let dy = wy - ey * t;
        d = d.min(dx * dx + dy * dy);
        // Crossing test on the horizontal ray.
        if (a[1] > p[1]) != (b[1] > p[1]) {
            let xint = a[0] + (p[1] - a[1]) * ex / ey;
            if p[0] < xint {
                inside = !inside;
            }
        }
        j = i;
    }
    let d = d.sqrt();
    if inside { -d } else { d }
}

/// Vertices of a five-point star with outer radius 1, first point up.
pub fn star_vertices() -> [[f32; 2]; 10] {
    let mut v = [[0.0f32; 2]; 10];
    for (i, p) in v.iter_mut().enumerate() {
        let r = if i % 2 == 0 { 1.0 } else { 0.42 };
        let a = -PI / 2.0 + i as f32 * PI / 5.0;
        *p = [r * a.cos(), r * a.sin()];
    }
    v
}

/// Heart SDF in a unit box (−1..1, y down): two discs for the lobes and a
/// triangle for the point. Exact outside, a lower bound inside, which is
/// all feathering needs.
pub fn sdf_heart(p: [f32; 2]) -> f32 {
    // Shift so the heart fills the box: lobes centred at y = −0.35,
    // point at y = 1.
    let x = p[0].abs();
    let y = p[1];
    let lobe = length(x - 0.5, y + 0.35) - 0.5;
    let tri = sdf_polygon([x, y], &[[-1.0, -0.35], [1.0, -0.35], [0.0, 1.0]]);
    lobe.min(tri)
}

/// Flattens a closed cubic Bézier path to a polyline (16 segments per
/// curve), in the same units as the points.
pub fn flatten_path(path: &[PathPoint]) -> Vec<[f32; 2]> {
    let n = path.len();
    let mut out = Vec::with_capacity(n * 16);
    for i in 0..n {
        let a = path[i];
        let b = path[(i + 1) % n];
        let p0 = a.point;
        let p1 = [a.point[0] + a.handle_out[0], a.point[1] + a.handle_out[1]];
        let p2 = [b.point[0] + b.handle_in[0], b.point[1] + b.handle_in[1]];
        let p3 = b.point;
        let straight = a.handle_out == [0.0, 0.0] && b.handle_in == [0.0, 0.0];
        let steps = if straight { 1 } else { 16 };
        for s in 0..steps {
            let t = s as f32 / steps as f32;
            let u = 1.0 - t;
            let x = u * u * u * p0[0] + 3.0 * u * u * t * p1[0] + 3.0 * u * t * t * p2[0] + t * t * t * p3[0];
            let y = u * u * u * p0[1] + 3.0 * u * u * t * p1[1] + 3.0 * u * t * t * p2[1] + t * t * t * p3[1];
            out.push([x, y]);
        }
    }
    out
}

/// A mask prepared for a layer of a given pixel size.
pub struct PreparedMask {
    pub spec: MaskSpec,
    /// Centre in layer pixels.
    pub centre_px: [f32; 2],
    /// Half extents in layer pixels.
    pub half_px: [f32; 2],
    pub cos: f32,
    pub sin: f32,
    pub sigma_px: f32,
    pub expansion_px: f32,
    pub roundness_px: f32,
    /// Flattened path in layer pixels (relative to the centre, unrotated).
    pub polygon: Vec<[f32; 2]>,
}

impl PreparedMask {
    pub fn new(spec: &MaskSpec, width: u32, height: u32) -> PreparedMask {
        let (w, h) = (width as f32, height as f32);
        let short = w.min(h);
        let half_px = [(spec.size[0] * w * 0.5).max(0.0), (spec.size[1] * h * 0.5).max(0.0)];
        let (sin, cos) = spec.rotation.to_radians().sin_cos();
        let centre_px = [spec.centre[0] * w, spec.centre[1] * h];
        let polygon = match &spec.shape {
            MaskShape::Path(pts) => {
                let mut poly: Vec<[f32; 2]> = flatten_path(pts).into_iter().map(|p| [p[0] * w, p[1] * h]).collect();
                // Pins bend the outline in pixels, so a bend is rigid on
                // a non-square layer.
                if !spec.pins.is_empty() {
                    crate::puppet::bend_points(&spec.pins, &mut poly, w, h);
                }
                poly.into_iter().map(|p| [p[0] - centre_px[0], p[1] - centre_px[1]]).collect()
            }
            _ => Vec::new(),
        };
        PreparedMask {
            spec: spec.clone(),
            centre_px,
            half_px,
            cos,
            sin,
            sigma_px: spec.feather.max(0.0) * short * 0.5,
            expansion_px: spec.expansion * short,
            roundness_px: spec.roundness.clamp(0.0, 1.0) * half_px[0].min(half_px[1]),
            polygon,
        }
    }

    /// Signed distance of a layer pixel position (pixel centres at +0.5).
    #[inline]
    pub fn distance(&self, x: f32, y: f32) -> f32 {
        let dx = x - self.centre_px[0];
        let dy = y - self.centre_px[1];
        // Rotate into the mask's frame (inverse of a clockwise rotation).
        let lx = dx * self.cos + dy * self.sin;
        let ly = -dx * self.sin + dy * self.cos;
        let h = self.half_px;
        let d = match &self.spec.shape {
            MaskShape::Rectangle => sdf_rect([lx, ly], h, self.roundness_px),
            MaskShape::Ellipse => sdf_ellipse([lx, ly], h),
            MaskShape::Line => ly,
            MaskShape::Mirror => ly.abs() - h[1],
            MaskShape::Star => {
                let v = star_vertices();
                let scaled: Vec<[f32; 2]> = v.iter().map(|p| [p[0] * h[0], p[1] * h[1]]).collect();
                sdf_polygon([lx, ly], &scaled)
            }
            MaskShape::Heart => {
                let s = h[0].min(h[1]).max(1e-6);
                sdf_heart([lx / h[0].max(1e-6), ly / h[1].max(1e-6)]) * s
            }
            MaskShape::Path(_) => sdf_polygon([lx, ly], &self.polygon),
        };
        d - self.expansion_px
    }

    /// Coverage of a layer pixel, 0..1, with invert and opacity applied.
    #[inline]
    pub fn coverage(&self, x: f32, y: f32) -> f32 {
        let mut c = coverage(self.distance(x, y), self.sigma_px);
        if self.spec.invert {
            c = 1.0 - c;
        }
        c * self.spec.opacity.clamp(0.0, 1.0)
    }
}

/// Folds one mask's coverage into the accumulated value.
#[inline]
pub fn combine(acc: f32, mode: MaskMode, c: f32) -> f32 {
    match mode {
        MaskMode::Add => acc + c - acc * c,
        MaskMode::Subtract => acc * (1.0 - c),
        MaskMode::Intersect => acc * c,
    }
}

/// Starting value of the accumulator: nothing selected when the first mask
/// adds, everything when it subtracts or intersects.
pub fn initial(masks: &[MaskSpec]) -> f32 {
    match masks.first().map(|m| m.mode) {
        None | Some(MaskMode::Add) => 0.0,
        _ => 1.0,
    }
}

/// Combined coverage of all masks at a layer pixel.
pub fn coverage_at(masks: &[PreparedMask], init: f32, x: f32, y: f32) -> f32 {
    let mut acc = init;
    for m in masks {
        acc = combine(acc, m.spec.mode, m.coverage(x, y));
    }
    acc.clamp(0.0, 1.0)
}

/// Multiplies a frame's pixels by the combined mask coverage.
pub fn apply(frame: &mut crate::frame::Frame, masks: &[MaskSpec]) {
    if masks.is_empty() {
        return;
    }
    let prepared: Vec<PreparedMask> = masks.iter().map(|m| PreparedMask::new(m, frame.width, frame.height)).collect();
    let init = initial(masks);
    let w = frame.width as usize;
    studio_media::par::par_rows(&mut frame.pixels, w, 16, |row, out| {
        for (x, p) in out.iter_mut().enumerate() {
            let c = coverage_at(&prepared, init, x as f32 + 0.5, row as f32 + 0.5);
            for v in p.iter_mut() {
                *v *= c;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn erf_reference_points() {
        assert!(erf(0.0).abs() < 1e-7);
        assert!((erf(0.5) - 0.5204999).abs() < 1e-6);
        assert!((erf(1.0) - 0.8427008).abs() < 1e-6);
        assert!((erf(-1.0) + 0.8427008).abs() < 1e-6);
        assert!((erf(3.0) - 0.9999779).abs() < 1e-6);
    }

    #[test]
    fn rect_sdf() {
        assert!((sdf_rect([0.0, 0.0], [2.0, 1.0], 0.0) + 1.0).abs() < 1e-6);
        assert!((sdf_rect([3.0, 0.0], [2.0, 1.0], 0.0) - 1.0).abs() < 1e-6);
        assert!((sdf_rect([2.0, 1.0], [2.0, 1.0], 0.0)).abs() < 1e-6);
        // Rounded corner: the corner point is now outside by r(√2 − 1).
        let r = 1.0;
        assert!((sdf_rect([2.0, 1.0], [2.0, 1.0], r) - r * (2f32.sqrt() - 1.0)).abs() < 1e-5);
    }

    #[test]
    fn ellipse_sdf_on_axes() {
        assert!((sdf_ellipse([3.0, 0.0], [2.0, 1.0]) - 1.0).abs() < 1e-5);
        assert!((sdf_ellipse([0.0, 2.0], [2.0, 1.0]) - 1.0).abs() < 1e-5);
        assert!(sdf_ellipse([0.0, 0.0], [2.0, 1.0]) < 0.0);
    }

    #[test]
    fn polygon_sdf_square() {
        let sq = [[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]];
        assert!((sdf_polygon([0.0, 0.0], &sq) + 1.0).abs() < 1e-6);
        assert!((sdf_polygon([2.0, 0.0], &sq) - 1.0).abs() < 1e-6);
        assert!((sdf_polygon([2.0, 2.0], &sq) - 2f32.sqrt()).abs() < 1e-6);
    }

    #[test]
    fn star_and_heart_contain_centre() {
        let v = star_vertices();
        assert!(sdf_polygon([0.0, 0.0], &v) < 0.0);
        assert!(sdf_polygon([0.0, -0.99], &v) < 0.0, "tip up");
        assert!(sdf_polygon([0.0, 0.99], &v) > 0.0, "nothing straight down between points");
        assert!(sdf_heart([0.0, 0.0]) < 0.0);
        assert!(sdf_heart([0.5, -0.5]) < 0.0, "lobe");
        assert!(sdf_heart([0.0, 0.95]) < 0.0, "tip");
        assert!(sdf_heart([0.0, -0.9]) > 0.0, "notch");
        assert!(sdf_heart([0.95, 0.9]) > 0.0, "corner");
    }

    #[test]
    fn coverage_without_feather_antialiases_one_pixel() {
        assert_eq!(coverage(-5.0, 0.0), 1.0);
        assert_eq!(coverage(5.0, 0.0), 0.0);
        assert!((coverage(0.0, 0.0) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn feather_matches_gaussian_cdf() {
        // At one sigma outside, coverage is about 16 %.
        assert!((coverage(4.0, 4.0) - 0.1587).abs() < 1e-3);
        assert!((coverage(-4.0, 4.0) - 0.8413).abs() < 1e-3);
    }

    #[test]
    fn combine_rules() {
        assert_eq!(combine(0.0, MaskMode::Add, 1.0), 1.0);
        assert_eq!(combine(1.0, MaskMode::Subtract, 1.0), 0.0);
        assert_eq!(combine(1.0, MaskMode::Intersect, 0.5), 0.5);
        assert_eq!(initial(&[]), 0.0);
        assert_eq!(initial(&[MaskSpec { mode: MaskMode::Subtract, ..Default::default() }]), 1.0);
    }

    #[test]
    fn rotated_line_splits_frame() {
        let spec = MaskSpec { shape: MaskShape::Line, rotation: 90.0, ..Default::default() };
        let m = PreparedMask::new(&spec, 100, 100);
        // Rotated 90° clockwise the line is vertical and "above" has
        // turned into the right-hand side.
        assert!(m.coverage(10.0, 50.0) < 0.01);
        assert!(m.coverage(90.0, 50.0) > 0.99);
        let m = PreparedMask::new(&MaskSpec { shape: MaskShape::Line, ..Default::default() }, 100, 100);
        assert!(m.coverage(50.0, 10.0) > 0.99);
        assert!(m.coverage(50.0, 90.0) < 0.01);
    }
}
