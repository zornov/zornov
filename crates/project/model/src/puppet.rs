// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Puppet pins and corner pins: a layer's picture bent by handles placed
//! on it rather than moved as one rigid piece.
//!
//! Coordinates are those of masks: 0..1 across the layer as it is drawn
//! (after crop), so pins move with the layer and stay put when it is
//! scaled. A pin has a `rest` point (where it was placed on the picture)
//! and a `position` (where that point of the picture now is); moving the
//! position bends the mesh, and only `position` and the bend, starch and
//! overlap settings are animated (`puppet.<pin>.<setting>`).
//!
//! The deformation itself is the renderer's (`studio-render`'s `puppet`
//! module), so the viewer, the export and the AI's frames agree.

use crate::value::{Extra, Vec2};
use serde::{Deserialize, Serialize};

/// Pins bending one layer (or, on a mask, its path).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Puppet {
    /// Off keeps the pins but shows the picture unbent.
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub enabled: bool,
    /// The pins, in the order they were placed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pins: Vec<Pin>,
    /// Mesh cells across the longer side of the picture's content (more
    /// bends more smoothly and costs more).
    #[serde(default = "density")]
    pub density: f64,
    /// How far the mesh reaches past the picture's edges, as a fraction
    /// of its shorter side.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub expansion: f64,
    /// Fields this version does not know about.
    #[serde(flatten)]
    pub extra: Extra,
}

fn yes() -> bool {
    true
}
fn is_true(b: &bool) -> bool {
    *b
}
fn is_zero(v: &f64) -> bool {
    *v == 0.0
}
fn density() -> f64 {
    24.0
}
fn one() -> f64 {
    1.0
}
fn amount() -> f64 {
    0.5
}
fn extent() -> f64 {
    0.15
}

impl Default for Puppet {
    fn default() -> Self {
        Puppet { enabled: true, pins: Vec::new(), density: density(), expansion: 0.0, extra: Extra::new() }
    }
}

impl Puppet {
    /// The pin with id `id`.
    pub fn pin(&self, id: u32) -> Option<&Pin> {
        self.pins.iter().find(|p| p.id == id)
    }

    /// The pin with id `id`, to change.
    pub fn pin_mut(&mut self, id: u32) -> Option<&mut Pin> {
        self.pins.iter_mut().find(|p| p.id == id)
    }

    /// An id no pin has yet.
    pub fn next_id(&self) -> u32 {
        self.pins.iter().map(|p| p.id).max().map_or(1, |m| m + 1)
    }

    /// Adds a pin of `kind` at `at` (both rest and position) and returns
    /// its id.
    pub fn add(&mut self, kind: PinKind, at: Vec2) -> u32 {
        let id = self.next_id();
        self.pins.push(Pin::new(id, kind, at));
        id
    }

    /// Whether the pins bend anything (a position pin moved, a bend pin
    /// turned or scaled).
    pub fn bends(&self) -> bool {
        self.enabled
            && self.pins.iter().any(|p| match p.kind {
                PinKind::Position => p.rest != p.position,
                PinKind::Bend => p.rotation != 0.0 || p.scale != 1.0,
                _ => false,
            })
    }
}

/// What a pin does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PinKind {
    /// Holds its point of the picture where it is moved to.
    #[default]
    Position,
    /// Turns and scales the picture around it without pinning it down.
    Bend,
    /// Stiffens the picture around it (`amount` 0..1 over `extent`).
    Starch,
    /// Brings the picture around it in front (`amount` -1..1, higher in
    /// front) where folds overlap.
    Overlap,
}

/// One pin.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Pin {
    /// Unique within its puppet; keyframe paths use it.
    pub id: u32,
    /// What it does.
    #[serde(default)]
    pub kind: PinKind,
    /// Display name ("left hand").
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// The point of the picture the pin holds, 0..1 layer coordinates.
    pub rest: Vec2,
    /// Where that point now is (animatable as `.position`).
    pub position: Vec2,
    /// Bend pins: clockwise degrees (animatable).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub rotation: f64,
    /// Bend pins: size factor (animatable).
    #[serde(default = "one")]
    pub scale: f64,
    /// Starch: stiffness 0..1; overlap: in front -1..1 (animatable).
    #[serde(default = "amount")]
    pub amount: f64,
    /// Starch and overlap: reach, as a fraction of the shorter side
    /// (animatable).
    #[serde(default = "extent")]
    pub extent: f64,
}

impl Pin {
    /// A pin at rest at `at`.
    pub fn new(id: u32, kind: PinKind, at: Vec2) -> Self {
        Pin { id, kind, name: String::new(), rest: at, position: at, rotation: 0.0, scale: 1.0, amount: amount(), extent: extent() }
    }

    /// The settings that can carry keyframes, with their names.
    pub const ANIMATED: [&'static str; 5] = ["position", "rotation", "scale", "amount", "extent"];

    /// Reads a setting by name.
    pub fn get(&self, key: &str) -> Option<crate::Value> {
        use crate::Value;
        Some(match key {
            "position" => Value::Point(self.position),
            "rest" => Value::Point(self.rest),
            "rotation" => Value::Angle(self.rotation),
            "scale" => Value::Scalar(self.scale),
            "amount" => Value::Scalar(self.amount),
            "extent" => Value::Scalar(self.extent),
            _ => return None,
        })
    }

    /// Writes a setting by name; `false` for an unknown one or a value of
    /// the wrong kind.
    pub fn set(&mut self, key: &str, v: &crate::Value) -> bool {
        use crate::Value;
        match (key, v) {
            ("position", Value::Point(p)) => self.position = *p,
            ("rest", Value::Point(p)) => self.rest = *p,
            ("rotation", Value::Angle(x) | Value::Scalar(x)) => self.rotation = *x,
            ("scale", Value::Scalar(x)) => self.scale = *x,
            ("amount", Value::Scalar(x)) => self.amount = *x,
            ("extent", Value::Scalar(x)) => self.extent = *x,
            _ => return false,
        }
        true
    }
}

/// How the gap that opens behind a cut is filled.
#[derive(Clone, Copy, Debug, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CutFill {
    /// Nothing: the gap shows what is behind the layer.
    #[default]
    None,
    /// A flat colour (the inside of a mouth).
    Colour(crate::value::Rgba),
    /// Generated from the picture: the colour found along the cut,
    /// darkened and warmed so a gap reads as depth, not as a hole.
    Auto,
}

impl CutFill {
    /// Whether the cut fills its gap at all.
    pub fn is_none(&self) -> bool {
        matches!(self, CutFill::None)
    }
}

/// A cut ("scissor line"): a path drawn across the picture along which the
/// deformation mesh of a puppet or skeleton is split, so the regions on its
/// two sides move on their own (between the legs, along the lips, under an
/// arm, around a tail). The picture stays whole at rest; it opens only
/// where the sides move apart.
///
/// Points are 0..1 layer coordinates (like pins), on the picture as it is
/// at rest. The path is open: where it ends the mesh stays joined, so a cut
/// from a crotch down between the legs leaves them joined at the hip.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Cut {
    /// Unique within its layer.
    pub id: u32,
    /// Display name ("legs", "mouth").
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// Off keeps the cut but joins the mesh across it again.
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub enabled: bool,
    /// The path, at least two points.
    pub points: Vec<Vec2>,
    /// What shows in the gap behind the cut.
    #[serde(default, skip_serializing_if = "CutFill::is_none")]
    pub fill: CutFill,
    /// Fields this version does not know about.
    #[serde(flatten)]
    pub extra: Extra,
}

impl Cut {
    /// A cut along `points` with no fill.
    pub fn new(id: u32, points: Vec<Vec2>) -> Self {
        Cut { id, name: String::new(), enabled: true, points, fill: CutFill::None, extra: Extra::new() }
    }

    /// An id no cut of `cuts` has yet.
    pub fn next_id(cuts: &[Cut]) -> u32 {
        cuts.iter().map(|c| c.id).max().map_or(1, |m| m + 1)
    }

    /// Length of the path in layer units.
    pub fn length(&self) -> f64 {
        self.points.windows(2).map(|w| ((w[1].x - w[0].x).powi(2) + (w[1].y - w[0].y).powi(2)).sqrt()).sum()
    }

    /// Whether the cut can split anything (enabled, two distinct points).
    pub fn is_active(&self) -> bool {
        self.enabled && self.points.len() >= 2 && self.length() > 1e-9
    }

    /// The point of the path nearest `p` and its distance.
    pub fn nearest(&self, p: Vec2) -> Option<(Vec2, f64)> {
        let mut best: Option<(Vec2, f64)> = None;
        for w in self.points.windows(2) {
            let (a, b) = (w[0], w[1]);
            let (dx, dy) = (b.x - a.x, b.y - a.y);
            let l2 = dx * dx + dy * dy;
            let t = if l2 < 1e-18 { 0.0 } else { (((p.x - a.x) * dx + (p.y - a.y) * dy) / l2).clamp(0.0, 1.0) };
            let q = Vec2::new(a.x + dx * t, a.y + dy * t);
            let d = ((p.x - q.x).powi(2) + (p.y - q.y).powi(2)).sqrt();
            if best.is_none_or(|(_, bd)| d < bd) {
                best = Some((q, d));
            }
        }
        best.or_else(|| self.points.first().map(|q| (*q, ((p.x - q.x).powi(2) + (p.y - q.y).powi(2)).sqrt())))
    }
}

/// Corner pin: the layer's picture (the `rest` rectangle of it) stretched
/// so its corners land on four points, with perspective. A skew is a
/// corner pin whose corners make a parallelogram.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CornerPin {
    /// Off keeps the corners but shows the picture flat.
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub enabled: bool,
    /// The part of the picture that is pinned, 0..1 layer coordinates
    /// (top-left and bottom-right corners); usually what the layer draws.
    pub rest: [Vec2; 2],
    /// Where the rectangle's top-left corner lands (animatable as
    /// `corner_pin.top_left`).
    pub top_left: Vec2,
    /// Top-right corner.
    pub top_right: Vec2,
    /// Bottom-right corner.
    pub bottom_right: Vec2,
    /// Bottom-left corner.
    pub bottom_left: Vec2,
}

impl CornerPin {
    /// The corner names, in order.
    pub const CORNERS: [&'static str; 4] = ["top_left", "top_right", "bottom_right", "bottom_left"];

    /// A corner pin that changes nothing yet over `rest`.
    pub fn new(rest: [Vec2; 2]) -> Self {
        let [a, b] = rest;
        CornerPin { enabled: true, rest, top_left: a, top_right: Vec2::new(b.x, a.y), bottom_right: b, bottom_left: Vec2::new(a.x, b.y) }
    }

    /// The corners in order: top-left, top-right, bottom-right,
    /// bottom-left.
    pub fn corners(&self) -> [Vec2; 4] {
        [self.top_left, self.top_right, self.bottom_right, self.bottom_left]
    }

    /// A corner by name.
    pub fn corner_mut(&mut self, name: &str) -> Option<&mut Vec2> {
        Some(match name {
            "top_left" => &mut self.top_left,
            "top_right" => &mut self.top_right,
            "bottom_right" => &mut self.bottom_right,
            "bottom_left" => &mut self.bottom_left,
            _ => return None,
        })
    }

    /// Whether the corners still sit on the rest rectangle.
    pub fn is_identity(&self) -> bool {
        CornerPin::new(self.rest).corners() == self.corners()
    }

    /// Homography taking the unit square to the corners (row-major 3×3),
    /// as the renderer builds it.
    fn matrix(&self) -> [f64; 9] {
        let [a, b, c, d] = self.corners();
        let (sx, sy) = (a.x - b.x + c.x - d.x, a.y - b.y + c.y - d.y);
        if sx.abs() < 1e-12 && sy.abs() < 1e-12 {
            return [b.x - a.x, c.x - b.x, a.x, b.y - a.y, c.y - b.y, a.y, 0.0, 0.0, 1.0];
        }
        let (dx1, dx2, dy1, dy2) = (b.x - c.x, d.x - c.x, b.y - c.y, d.y - c.y);
        let den = dx1 * dy2 - dx2 * dy1;
        let den = if den.abs() < 1e-12 { 1e-12 } else { den };
        let g = (sx * dy2 - dx2 * sy) / den;
        let h = (dx1 * sy - sx * dy1) / den;
        [b.x - a.x + g * b.x, d.x - a.x + h * d.x, a.x, b.y - a.y + g * b.y, d.y - a.y + h * d.y, a.y, g, h, 1.0]
    }

    /// Where a point of the flat picture (0..1 layer coordinates) lands.
    pub fn map(&self, p: Vec2) -> Vec2 {
        let [r0, r1] = self.rest;
        let u = (p.x - r0.x) / (r1.x - r0.x).abs().max(1e-9).copysign(r1.x - r0.x);
        let v = (p.y - r0.y) / (r1.y - r0.y).abs().max(1e-9).copysign(r1.y - r0.y);
        let m = self.matrix();
        let w = m[6] * u + m[7] * v + m[8];
        let w = if w.abs() < 1e-12 { 1e-12 } else { w };
        Vec2::new((m[0] * u + m[1] * v + m[2]) / w, (m[3] * u + m[4] * v + m[5]) / w)
    }

    /// The point of the flat picture that lands on `p`, if any.
    pub fn unmap(&self, p: Vec2) -> Option<Vec2> {
        let m = self.matrix();
        // Inverse by the adjugate.
        let inv = [
            m[4] * m[8] - m[5] * m[7],
            m[2] * m[7] - m[1] * m[8],
            m[1] * m[5] - m[2] * m[4],
            m[5] * m[6] - m[3] * m[8],
            m[0] * m[8] - m[2] * m[6],
            m[2] * m[3] - m[0] * m[5],
            m[3] * m[7] - m[4] * m[6],
            m[1] * m[6] - m[0] * m[7],
            m[0] * m[4] - m[1] * m[3],
        ];
        let w = inv[6] * p.x + inv[7] * p.y + inv[8];
        if w.abs() < 1e-12 {
            return None;
        }
        let (u, v) = ((inv[0] * p.x + inv[1] * p.y + inv[2]) / w, (inv[3] * p.x + inv[4] * p.y + inv[5]) / w);
        let [r0, r1] = self.rest;
        Some(Vec2::new(r0.x + u * (r1.x - r0.x), r0.y + v * (r1.y - r0.y)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Clip, ClipId, ClipSource, Generator, GeneratorKind, Interp, Keyframe, LaneId, Mask, MaskId, MaskShape, Track, Value};

    fn clip() -> Clip {
        Clip::new(ClipId(1), LaneId(1), 0, 2_000_000, ClipSource::Generator(Generator::new(GeneratorKind::Solid)))
    }

    #[test]
    fn pins_are_properties_and_animate() {
        let mut c = clip();
        let mut p = Puppet::default();
        let id = p.add(PinKind::Position, Vec2::new(0.5, 0.5));
        c.puppet = Some(p);
        let path = format!("puppet.{id}.position");
        assert_eq!(c.get_property(&path), Some(Value::Point(Vec2::new(0.5, 0.5))));
        assert!(c.set_property(&path, Value::Point(Vec2::new(0.6, 0.5))));
        assert!(!c.set_property(&path, Value::Scalar(1.0)));
        assert!(c.get_property("puppet.99.position").is_none());
        c.keyframes.insert(path.clone(), Track::new(vec![Keyframe::new(0, Vec2::new(0.5, 0.5), Interp::Linear), Keyframe::new(1_000_000, Vec2::new(0.7, 0.5), Interp::Linear)]));
        let at = c.puppet_at(500_000).unwrap();
        assert!((at.pins[0].position.x - 0.6).abs() < 1e-9);
        assert!(at.bends());
    }

    #[test]
    fn cuts_round_trip_and_measure() {
        let mut c = clip();
        let mut cut = Cut::new(Cut::next_id(&c.cuts), vec![Vec2::new(0.5, 0.5), Vec2::new(0.5, 0.9)]);
        cut.fill = CutFill::Colour(crate::value::Rgba::new(0.4, 0.05, 0.05, 1.0));
        assert!(cut.is_active() && (cut.length() - 0.4).abs() < 1e-9);
        let (q, d) = cut.nearest(Vec2::new(0.6, 0.7)).unwrap();
        assert!((q.y - 0.7).abs() < 1e-9 && (d - 0.1).abs() < 1e-9);
        c.cuts.push(cut);
        assert_eq!(Cut::next_id(&c.cuts), 2);
        let back: Clip = serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
        assert_eq!(back, c);
        // A clip without cuts writes none.
        assert!(!serde_json::to_string(&clip()).unwrap().contains("cuts"));
        c.cuts[0].enabled = false;
        assert!(!c.cuts[0].is_active());
    }

    #[test]
    fn corners_and_mask_points_animate() {
        let mut c = clip();
        c.corner_pin = Some(CornerPin::new([Vec2::ZERO, Vec2::ONE]));
        assert!(c.corner_pin.as_ref().unwrap().is_identity());
        assert!(c.set_property("corner_pin.top_left", Value::Point(Vec2::new(0.1, 0.1))));
        let cp = c.corner_pin.clone().unwrap();
        assert!(!cp.is_identity());
        let p = Vec2::new(0.3, 0.6);
        let back = cp.unmap(cp.map(p)).unwrap();
        assert!((back.x - p.x).abs() < 1e-9 && (back.y - p.y).abs() < 1e-9);
        assert!((cp.map(Vec2::ZERO).x - 0.1).abs() < 1e-9);
        let mut m = Mask::new(MaskId(7), MaskShape::Ellipse);
        m.shape = MaskShape::Path;
        m.path = Mask::new(MaskId(7), MaskShape::Ellipse).outline().unwrap();
        assert_eq!(m.path.len(), 4);
        m.pins.push(Pin::new(1, PinKind::Position, Vec2::new(0.5, 0.25)));
        c.masks.push(m);
        assert!(c.set_property("mask.7.point.0", Value::Point(Vec2::new(0.5, 0.1))));
        assert_eq!(c.masks[0].path[0].point, Vec2::new(0.5, 0.1));
        c.keyframes.insert("mask.7.pin.1.position".into(), Track::new(vec![Keyframe::new(0, Vec2::new(0.5, 0.25), Interp::Linear), Keyframe::new(1_000_000, Vec2::new(0.5, 0.05), Interp::Linear)]));
        let at = c.mask_at(&c.masks[0], 1_000_000);
        assert_eq!(at.pins[0].position, Vec2::new(0.5, 0.05));
        // Round trip.
        let back: Clip = serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
        assert_eq!(back, c);
    }
}
