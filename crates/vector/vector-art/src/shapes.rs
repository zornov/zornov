// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Live shapes: rectangles, ellipses, polygons, stars, spirals, arcs,
//! lines and grids that keep their parameters (corner radii, sides,
//! points) until expanded into plain paths.

use kurbo::{Point, Rect};
use serde::{Deserialize, Serialize};
use studio_vector_geom::shapes::{self as gs, CornerKind};
use studio_vector_geom::PathData;

fn is_zero4(r: &[f64; 4]) -> bool {
    r.iter().all(|v| *v == 0.0)
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LiveShape {
    Rect {
        rect: Rect,
        /// Corner radii: top-left, top-right, bottom-right, bottom-left.
        #[serde(default, skip_serializing_if = "is_zero4")]
        radii: [f64; 4],
        /// How each corner is cut (round, inverted round, chamfer).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        corners: Option<[CornerKind; 4]>,
    },
    Ellipse { rect: Rect },
    /// A pie slice or arc of an ellipse: `start`/`end` degrees clockwise from 3 o'clock.
    Pie { rect: Rect, start: f64, end: f64 },
    Polygon {
        center: Point,
        radius: f64,
        sides: u32,
        #[serde(default)]
        rotation: f64,
    },
    Star {
        center: Point,
        radius: f64,
        inner: f64,
        points: u32,
        #[serde(default)]
        rotation: f64,
    },
    Spiral {
        center: Point,
        radius: f64,
        /// Each quarter turn's radius as a percent of the last.
        #[serde(default = "eighty")]
        decay: f64,
        /// Quarter turns.
        #[serde(default = "ten")]
        segments: u32,
        #[serde(default = "yes")]
        clockwise: bool,
    },
    /// A quarter arc from `from` to `to`, bulging by `slope` (-100..100).
    Arc {
        from: Point,
        to: Point,
        #[serde(default)]
        slope: f64,
        #[serde(default)]
        closed: bool,
    },
    Line { from: Point, to: Point },
    /// A rectangular grid of lines (drawn as one compound path).
    Grid {
        rect: Rect,
        rows: u32,
        cols: u32,
        #[serde(default)]
        skew: (f64, f64),
    },
    /// Concentric ellipses and radial dividers.
    PolarGrid {
        rect: Rect,
        rings: u32,
        spokes: u32,
    },
}

fn eighty() -> f64 {
    80.0
}

fn ten() -> u32 {
    10
}

fn yes() -> bool {
    true
}

impl LiveShape {
    pub fn kind(&self) -> &'static str {
        match self {
            LiveShape::Rect { .. } => "rect",
            LiveShape::Ellipse { .. } => "ellipse",
            LiveShape::Pie { .. } => "pie",
            LiveShape::Polygon { .. } => "polygon",
            LiveShape::Star { .. } => "star",
            LiveShape::Spiral { .. } => "spiral",
            LiveShape::Arc { .. } => "arc",
            LiveShape::Line { .. } => "line",
            LiveShape::Grid { .. } => "grid",
            LiveShape::PolarGrid { .. } => "polar_grid",
        }
    }

    /// Whether the outline is open (lines, arcs, spirals, grids are stroked).
    pub fn is_open(&self) -> bool {
        matches!(self, LiveShape::Spiral { .. } | LiveShape::Line { .. } | LiveShape::Grid { .. } | LiveShape::PolarGrid { .. } | LiveShape::Arc { closed: false, .. })
    }

    /// The shape as a path.
    pub fn to_path(&self) -> PathData {
        match self {
            LiveShape::Rect { rect, radii, corners } => {
                if is_zero4(radii) {
                    gs::rectangle(*rect)
                } else {
                    gs::rectangle_with_corners(*rect, *radii, corners.unwrap_or([CornerKind::Round; 4]))
                }
            }
            LiveShape::Ellipse { rect } => gs::ellipse(*rect),
            LiveShape::Pie { rect, start, end } => gs::ellipse_pie(*rect, *start, *end),
            LiveShape::Polygon { center, radius, sides, rotation } => gs::polygon(*center, *radius, (*sides).clamp(3, 1000), *rotation),
            LiveShape::Star { center, radius, inner, points, rotation } => gs::star(*center, *radius, *inner, (*points).clamp(2, 1000), *rotation),
            LiveShape::Spiral { center, radius, decay, segments, clockwise } => gs::spiral(*center, *radius, *decay, (*segments).clamp(1, 1000), *clockwise),
            LiveShape::Arc { from, to, slope, closed } => gs::arc(*from, *to, *slope, *closed, true),
            LiveShape::Line { from, to } => gs::line(*from, *to),
            LiveShape::Grid { rect, rows, cols, skew } => {
                let parts = gs::rectangular_grid(*rect, ((*rows).saturating_sub(1).min(1000), skew.1), ((*cols).saturating_sub(1).min(1000), skew.0), true);
                PathData::new(parts.into_iter().flat_map(|p| p.subpaths).collect())
            }
            LiveShape::PolarGrid { rect, rings, spokes } => {
                let parts = gs::polar_grid(*rect, ((*rings).min(1000), 0.0), ((*spokes).min(1000), 0.0), false);
                PathData::new(parts.into_iter().flat_map(|p| p.subpaths).collect())
            }
        }
    }

    /// A star whose arms are straight (inner radius on the shoulder line).
    pub fn straight_star(center: Point, radius: f64, points: u32) -> LiveShape {
        LiveShape::Star { center, radius, inner: gs::straight_star_inner_radius(radius, points), points, rotation: 0.0 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shapes_make_paths() {
        let r = Rect::new(0.0, 0.0, 100.0, 50.0);
        assert_eq!(LiveShape::Rect { rect: r, radii: [0.0; 4], corners: None }.to_path().anchor_count(), 4);
        assert_eq!(LiveShape::Rect { rect: r, radii: [10.0; 4], corners: None }.to_path().anchor_count(), 8);
        assert_eq!(LiveShape::Polygon { center: Point::ZERO, radius: 10.0, sides: 6, rotation: 0.0 }.to_path().anchor_count(), 6);
        assert_eq!(LiveShape::Star { center: Point::ZERO, radius: 10.0, inner: 4.0, points: 5, rotation: 0.0 }.to_path().anchor_count(), 10);
        let e = LiveShape::Ellipse { rect: r }.to_path();
        assert!((e.subpaths[0].area().abs() - std::f64::consts::PI * 50.0 * 25.0).abs() < 5.0);
        let g = LiveShape::Grid { rect: r, rows: 3, cols: 4, skew: (0.0, 0.0) }.to_path();
        assert_eq!(g.subpaths.len(), 1 + 2 + 3);
        let j = serde_json::to_string(&LiveShape::Ellipse { rect: r }).unwrap();
        assert!(j.contains("\"kind\":\"ellipse\""));
    }
}
