// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Appearance stacks: any number of fills and strokes per object, each
//! with its own paint, opacity, blend and vector effects, drawn bottom to
//! top, then the object's own effects (shadows, glows, blur, distortions).
//! Graphic styles are saved appearances objects can follow.

use serde::{Deserialize, Serialize};
use studio_vector_raster::{BlendMode, Color, Join, Paint, StrokeStyle};

use crate::doc::WarpKind;

fn one() -> f32 {
    1.0
}

fn yes() -> bool {
    true
}

fn is_one(v: &f32) -> bool {
    *v == 1.0
}

fn is_true(b: &bool) -> bool {
    *b
}

fn is_normal(b: &BlendMode) -> bool {
    *b == BlendMode::Normal
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Fill {
    pub paint: Paint,
    /// A global swatch this fill follows: its colour comes from the swatch
    /// (times `tint`), so changing the swatch recolours every user.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub swatch: Option<String>,
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub tint: f32,
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub opacity: f32,
    #[serde(default, skip_serializing_if = "is_normal")]
    pub blend: BlendMode,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub visible: bool,
    /// Effects on this fill's own outline (offset, roughen, …).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effects: Vec<Effect>,
}

impl PartialEq for Fill {
    fn eq(&self, o: &Fill) -> bool {
        serde_json::to_value(self).ok() == serde_json::to_value(o).ok()
    }
}

impl Fill {
    pub fn new(paint: impl Into<Paint>) -> Fill {
        Fill { paint: paint.into(), swatch: None, tint: 1.0, opacity: 1.0, blend: BlendMode::Normal, visible: true, effects: Vec::new() }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Stroke {
    pub paint: Paint,
    #[serde(flatten)]
    pub style: StrokeStyle,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub swatch: Option<String>,
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub tint: f32,
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub opacity: f32,
    #[serde(default, skip_serializing_if = "is_normal")]
    pub blend: BlendMode,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub visible: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effects: Vec<Effect>,
}

impl PartialEq for Stroke {
    fn eq(&self, o: &Stroke) -> bool {
        serde_json::to_value(self).ok() == serde_json::to_value(o).ok()
    }
}

impl Stroke {
    pub fn new(paint: impl Into<Paint>, width: f64) -> Stroke {
        Stroke {
            paint: paint.into(),
            style: StrokeStyle::new(width),
            swatch: None,
            tint: 1.0,
            opacity: 1.0,
            blend: BlendMode::Normal,
            visible: true,
            effects: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Item {
    Fill(Fill),
    Stroke(Stroke),
}

/// An effect. Distances are in the object's own units, so effects scale
/// with it and stay sharp at any resolution.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Effect {
    // Raster effects (applied to the object's pixels).
    /// Gaussian blur; `radius` is about three deviations.
    Blur { radius: f64 },
    DropShadow {
        #[serde(default)]
        dx: f64,
        #[serde(default)]
        dy: f64,
        #[serde(default)]
        blur: f64,
        color: Color,
        #[serde(default)]
        spread: f64,
    },
    InnerShadow {
        #[serde(default)]
        dx: f64,
        #[serde(default)]
        dy: f64,
        #[serde(default)]
        blur: f64,
        color: Color,
    },
    OuterGlow {
        blur: f64,
        color: Color,
        #[serde(default)]
        spread: f64,
    },
    InnerGlow { blur: f64, color: Color },
    // Vector effects (applied to the outline, live).
    /// Grows (or with a negative amount shrinks) the outline.
    Offset {
        amount: f64,
        #[serde(default)]
        join: Join,
    },
    /// Jitters the outline: `size` (absolute), `detail` points per 100 units.
    Roughen {
        size: f64,
        #[serde(default = "ten")]
        detail: f64,
        #[serde(default)]
        smooth: bool,
        #[serde(default)]
        seed: u64,
    },
    /// Anchors move in (negative, pucker) or out (positive, bloat), -100..100.
    PuckerBloat { amount: f64 },
    ZigZag {
        size: f64,
        #[serde(default = "four")]
        ridges: f64,
        #[serde(default)]
        smooth: bool,
    },
    /// Turns the outline more towards the centre (degrees).
    Twist { angle: f64 },
    RoundCorners { radius: f64 },
    /// Copies of the outline, each moved, turned and scaled from the last.
    Transform {
        #[serde(default)]
        copies: u32,
        #[serde(default)]
        dx: f64,
        #[serde(default)]
        dy: f64,
        #[serde(default)]
        rotate: f64,
        #[serde(default = "one64")]
        scale: f64,
    },
    Warp {
        style: WarpKind,
        #[serde(default = "half")]
        bend: f64,
        #[serde(default)]
        horizontal: f64,
        #[serde(default)]
        vertical: f64,
    },
}

fn ten() -> f64 {
    10.0
}

fn four() -> f64 {
    4.0
}

fn one64() -> f64 {
    1.0
}

fn half() -> f64 {
    0.5
}

impl Effect {
    pub fn is_raster(&self) -> bool {
        matches!(self, Effect::Blur { .. } | Effect::DropShadow { .. } | Effect::InnerShadow { .. } | Effect::OuterGlow { .. } | Effect::InnerGlow { .. })
    }
}

/// Fills and strokes, bottom first, plus the object's effects.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Appearance {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<Item>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effects: Vec<Effect>,
}

impl Appearance {
    pub fn is_empty(&self) -> bool {
        self.items.is_empty() && self.effects.is_empty()
    }

    /// One fill.
    pub fn fill(paint: impl Into<Paint>) -> Appearance {
        Appearance { items: vec![Item::Fill(Fill::new(paint))], effects: Vec::new() }
    }

    /// One fill under one stroke.
    pub fn fill_stroke(fill: impl Into<Paint>, stroke: impl Into<Paint>, width: f64) -> Appearance {
        Appearance { items: vec![Item::Fill(Fill::new(fill)), Item::Stroke(Stroke::new(stroke, width))], effects: Vec::new() }
    }

    pub fn stroke(stroke: impl Into<Paint>, width: f64) -> Appearance {
        Appearance { items: vec![Item::Stroke(Stroke::new(stroke, width))], effects: Vec::new() }
    }

    /// The topmost visible fill.
    pub fn top_fill(&self) -> Option<&Fill> {
        self.items.iter().rev().find_map(|i| match i {
            Item::Fill(f) if f.visible => Some(f),
            _ => None,
        })
    }

    pub fn top_stroke(&self) -> Option<&Stroke> {
        self.items.iter().rev().find_map(|i| match i {
            Item::Stroke(s) if s.visible => Some(s),
            _ => None,
        })
    }
}

/// A named, saved appearance.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GraphicStyle {
    pub id: String,
    #[serde(default)]
    pub name: String,
    pub appearance: Appearance,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appearance_json_is_compact_and_reads_shorthands() {
        let a: Appearance = serde_json::from_str(r##"{"items":[{"type":"fill","paint":"#ff0000"},{"type":"stroke","paint":"#000","width":2,"join":"round"}],"effects":[{"type":"drop_shadow","dx":2,"dy":3,"blur":4,"color":"#00000080"}]}"##).unwrap();
        assert_eq!(a.items.len(), 2);
        assert!(matches!(&a.items[1], Item::Stroke(s) if s.style.width == 2.0 && s.style.join == Join::Round));
        let j = serde_json::to_string(&a).unwrap();
        assert!(j.contains("\"#ff0000\""), "{j}");
        let back: Appearance = serde_json::from_str(&j).unwrap();
        assert_eq!(back, a);
    }
}
