// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Scenes: small declarative files describing an animated, multi-layer
//! text composition (a title, a lower third, a callout…). A scene is
//! data (JSON through serde), and [`Scene::render`] plays it at a time.
//!
//! Timing model: a scene has an `intro` and an `outro` length. Played in
//! a clip of any duration, the intro runs from the start, the outro ends
//! at the end, and the middle holds. Tracks and reveals are expressed in
//! seconds within their phase, so a template designed for 5 s works
//! unchanged in a 12 s clip.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::anim::{LoopMotion, Reveal};
use crate::engine::TextEngine;
use crate::geom::{Affine, Ease, Rect};
use crate::paint::{Hooks, UnitPose};
use crate::raster;
use crate::rgba::Rgba;
use crate::style::{Canvas, LayoutParams, TextBlock, TextStyle, TextUnit};
use crate::Image;

/// Template groups, by what the creator is trying to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TemplateCategory {
    Title,
    LowerThird,
    Callout,
    CallToAction,
    EndCard,
    Quote,
    Label,
    Social,
}

impl TemplateCategory {
    pub const ALL: [TemplateCategory; 8] = [
        TemplateCategory::Title,
        TemplateCategory::LowerThird,
        TemplateCategory::Callout,
        TemplateCategory::CallToAction,
        TemplateCategory::EndCard,
        TemplateCategory::Quote,
        TemplateCategory::Label,
        TemplateCategory::Social,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            TemplateCategory::Title => "Titles",
            TemplateCategory::LowerThird => "Lower thirds",
            TemplateCategory::Callout => "Callouts",
            TemplateCategory::CallToAction => "Calls to action",
            TemplateCategory::EndCard => "End cards",
            TemplateCategory::Quote => "Quotes",
            TemplateCategory::Label => "Labels",
            TemplateCategory::Social => "Social",
        }
    }
}

/// An editable text slot.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Field {
    pub key: String,
    pub label: String,
    pub default: String,
}

/// What a layer draws.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum LayerKind {
    Text {
        /// Field key the text comes from, or…
        #[serde(default, skip_serializing_if = "Option::is_none")]
        field: Option<String>,
        /// …a fixed text.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        style: TextStyle,
        #[serde(default)]
        layout: LayoutParams,
    },
    Rect {
        colour: Rgba,
        /// Width and height in canvas units.
        size: [f32; 2],
        #[serde(default)]
        radius: f32,
    },
    Ellipse {
        colour: Rgba,
        size: [f32; 2],
    },
    /// A horizontal rule.
    Line {
        colour: Rgba,
        length: f32,
        thickness: f32,
    },
}

/// Animatable properties.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Property {
    /// Offset in canvas units.
    X,
    Y,
    Scale,
    ScaleX,
    ScaleY,
    /// Degrees.
    Rotation,
    Opacity,
    /// Multiplier on a shape's width / height (grows from an edge set by `anchor`).
    Width,
    Height,
    /// Reveal fraction from the left (0 = hidden, 1 = full).
    WipeX,
    /// Reveal fraction from the top.
    WipeY,
}

impl Property {
    fn default_value(&self) -> f32 {
        match self {
            Property::X | Property::Y | Property::Rotation => 0.0,
            _ => 1.0,
        }
    }
}

/// When a track's times count from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    #[default]
    Intro,
    Outro,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Key {
    pub t: f32,
    pub value: f32,
    #[serde(default)]
    pub ease: Ease,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub property: Property,
    #[serde(default)]
    pub phase: Phase,
    pub keys: Vec<Key>,
}

impl Track {
    /// Value at `t` (seconds into the phase); holds outside the keys.
    /// Easing is the curve *into* a key.
    pub fn value(&self, t: f32) -> f32 {
        match self.keys.len() {
            0 => self.property.default_value(),
            1 => self.keys[0].value,
            _ => {
                if t <= self.keys[0].t {
                    return self.keys[0].value;
                }
                for w in self.keys.windows(2) {
                    if t <= w[1].t {
                        let span = (w[1].t - w[0].t).max(1e-6);
                        let k = w[1].ease.apply((t - w[0].t) / span);
                        return w[0].value + (w[1].value - w[0].value) * k;
                    }
                }
                self.keys.last().unwrap().value
            }
        }
    }
}

/// One layer of a scene.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Layer {
    pub name: String,
    #[serde(flatten)]
    pub kind: LayerKind,
    /// Where the layer's anchor point sits, as a fraction of the canvas.
    #[serde(default = "centre")]
    pub position: [f32; 2],
    /// Which point of the layer's box sits at `position` (0,0 = top-left).
    #[serde(default = "centre")]
    pub anchor: [f32; 2],
    #[serde(default)]
    pub rotation: f32,
    #[serde(default = "one")]
    pub opacity: f32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tracks: Vec<Track>,
    /// Text reveal on entry / exit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intro: Option<Reveal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outro: Option<Reveal>,
    /// Seconds after the scene start when the intro reveal begins.
    #[serde(default)]
    pub intro_delay: f32,
    /// Length of the intro reveal; 0 = the scene's intro minus the delay.
    #[serde(default)]
    pub intro_duration: f32,
    #[serde(default)]
    pub outro_delay: f32,
    #[serde(default)]
    pub outro_duration: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub motion: Option<LoopMotion>,
}

fn centre() -> [f32; 2] {
    [0.5, 0.5]
}

fn one() -> f32 {
    1.0
}

impl Layer {
    pub fn text(name: &str, field: &str, style: TextStyle, layout: LayoutParams) -> Layer {
        Layer::new(name, LayerKind::Text { field: Some(field.into()), text: None, style, layout })
    }

    pub fn fixed_text(name: &str, text: &str, style: TextStyle, layout: LayoutParams) -> Layer {
        Layer::new(name, LayerKind::Text { field: None, text: Some(text.into()), style, layout })
    }

    pub fn rect(name: &str, colour: Rgba, w: f32, h: f32, radius: f32) -> Layer {
        Layer::new(name, LayerKind::Rect { colour, size: [w, h], radius })
    }

    pub fn line(name: &str, colour: Rgba, length: f32, thickness: f32) -> Layer {
        Layer::new(name, LayerKind::Line { colour, length, thickness })
    }

    pub fn ellipse(name: &str, colour: Rgba, w: f32, h: f32) -> Layer {
        Layer::new(name, LayerKind::Ellipse { colour, size: [w, h] })
    }

    pub fn new(name: &str, kind: LayerKind) -> Layer {
        Layer {
            name: name.into(),
            kind,
            position: [0.5, 0.5],
            anchor: [0.5, 0.5],
            rotation: 0.0,
            opacity: 1.0,
            tracks: Vec::new(),
            intro: None,
            outro: None,
            intro_delay: 0.0,
            intro_duration: 0.0,
            outro_delay: 0.0,
            outro_duration: 0.0,
            motion: None,
        }
    }

    pub fn at(mut self, x: f32, y: f32) -> Layer {
        self.position = [x, y];
        self
    }

    pub fn anchor(mut self, x: f32, y: f32) -> Layer {
        self.anchor = [x, y];
        self
    }

    pub fn rotated(mut self, deg: f32) -> Layer {
        self.rotation = deg;
        self
    }

    pub fn reveal(mut self, intro: Reveal, outro: Reveal) -> Layer {
        self.intro = Some(intro);
        self.outro = Some(outro);
        self
    }

    pub fn reveal_timing(mut self, delay: f32, duration: f32) -> Layer {
        self.intro_delay = delay;
        self.intro_duration = duration;
        self
    }

    pub fn outro_timing(mut self, delay: f32, duration: f32) -> Layer {
        self.outro_delay = delay;
        self.outro_duration = duration;
        self
    }

    pub fn motion(mut self, m: LoopMotion) -> Layer {
        self.motion = Some(m);
        self
    }

    /// Adds a track; keys are `(t, value, ease)`.
    pub fn track(mut self, property: Property, phase: Phase, keys: &[(f32, f32, Ease)]) -> Layer {
        self.tracks.push(Track { property, phase, keys: keys.iter().map(|&(t, value, ease)| Key { t, value, ease }).collect() });
        self
    }

    /// A common pair: intro keys and mirrored outro keys for one property.
    pub fn in_out(self, property: Property, from: f32, to: f32, duration: f32, ease: Ease) -> Layer {
        self.track(property, Phase::Intro, &[(0.0, from, Ease::Linear), (duration, to, ease)])
            .track(property, Phase::Outro, &[(0.0, to, Ease::Linear), (duration, from, ease)])
    }
}

/// A complete animated composition.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Scene {
    pub id: String,
    pub name: String,
    pub category: TemplateCategory,
    #[serde(default)]
    pub tags: Vec<String>,
    /// Natural length in seconds.
    pub duration: f32,
    pub intro: f32,
    pub outro: f32,
    pub fields: Vec<Field>,
    pub layers: Vec<Layer>,
}

/// A layer resolved at one time: an image and where it goes.
pub struct LayerFrame {
    pub name: String,
    pub image: Image,
    /// Image pixel → canvas pixel.
    pub transform: Affine,
    pub opacity: f32,
}

impl Scene {
    pub fn new(id: &str, name: &str, category: TemplateCategory, duration: f32, intro: f32, outro: f32) -> Scene {
        Scene { id: id.into(), name: name.into(), category, tags: Vec::new(), duration, intro, outro, fields: Vec::new(), layers: Vec::new() }
    }

    pub fn tags(mut self, tags: &str) -> Scene {
        self.tags = tags.split(',').map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect();
        self
    }

    pub fn field(mut self, key: &str, label: &str, default: &str) -> Scene {
        self.fields.push(Field { key: key.into(), label: label.into(), default: default.into() });
        self
    }

    pub fn layer(mut self, l: Layer) -> Scene {
        self.layers.push(l);
        self
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("scene serialises")
    }

    pub fn from_json(s: &str) -> Result<Scene, serde_json::Error> {
        serde_json::from_str(s)
    }

    /// Field defaults as a value map.
    pub fn default_values(&self) -> HashMap<String, String> {
        self.fields.iter().map(|f| (f.key.clone(), f.default.clone())).collect()
    }

    /// Splits a clip time into (phase, local time).
    fn phase_at(&self, t: f32, duration: f32) -> (Phase, f32, f32) {
        // When the clip is shorter than intro + outro, both shrink
        // proportionally so that nothing is cut off.
        let total = self.intro + self.outro;
        let k = if total > duration && total > 0.0 { duration / total } else { 1.0 };
        let outro = self.outro * k;
        let outro_start = (duration - outro).max(0.0);
        if t >= outro_start && outro > 0.0 { (Phase::Outro, (t - outro_start) / k, k) } else { (Phase::Intro, t / k, k) }
    }

    fn track_value(layer: &Layer, property: Property, phase: Phase, local: f32, intro_len: f32) -> f32 {
        let tr = layer.tracks.iter().filter(|t| t.property == property);
        let intro_track = tr.clone().find(|t| t.phase == Phase::Intro);
        let outro_track = tr.clone().find(|t| t.phase == Phase::Outro);
        match (phase, intro_track, outro_track) {
            (Phase::Outro, _, Some(o)) => o.value(local),
            (Phase::Outro, Some(i), None) => i.value(intro_len.max(local)),
            (_, Some(i), _) => i.value(local),
            (_, None, Some(o)) => o.value(0.0),
            _ => property.default_value(),
        }
    }

    /// Resolves every layer at clip time `t` for a clip of `duration`.
    pub fn frame(&self, engine: &TextEngine, t: f32, duration: f32, canvas: &Canvas, values: &HashMap<String, String>) -> Vec<LayerFrame> {
        let duration = duration.max(0.01);
        let t = t.clamp(0.0, duration);
        let (phase, local, k) = self.phase_at(t, duration);
        let ppu = canvas.px_per_unit();
        let mut out = Vec::new();
        for layer in &self.layers {
            let v = |p: Property| Self::track_value(layer, p, phase, local, self.intro);
            let opacity = (layer.opacity * v(Property::Opacity)).clamp(0.0, 1.0);
            if opacity <= 0.0 {
                continue;
            }
            let scale = v(Property::Scale);
            let (sx, sy) = (scale * v(Property::ScaleX), scale * v(Property::ScaleY));
            let rotation = layer.rotation + v(Property::Rotation);
            let (dx, dy) = (v(Property::X) * ppu, v(Property::Y) * ppu);
            let (wmul, hmul) = (v(Property::Width).max(0.0), v(Property::Height).max(0.0));
            let (wipe_x, wipe_y) = (v(Property::WipeX).clamp(0.0, 1.0), v(Property::WipeY).clamp(0.0, 1.0));

            // Reveal progress for text layers.
            let intro_len = if layer.intro_duration > 0.0 { layer.intro_duration } else { (self.intro - layer.intro_delay).max(0.05) };
            let outro_len = if layer.outro_duration > 0.0 { layer.outro_duration } else { (self.outro - layer.outro_delay).max(0.05) };
            let reveal_progress: Option<(bool, f32)> = match phase {
                Phase::Intro => layer.intro.as_ref().map(|_| (true, ((local - layer.intro_delay) / intro_len).clamp(0.0, 1.0))),
                Phase::Outro => layer.outro.as_ref().map(|_| (false, ((local - layer.outro_delay) / outro_len).clamp(0.0, 1.0))),
            };
            // In the outro phase a layer without an outro reveal is fully
            // shown; in the intro phase one without an intro is too.
            let _ = k;

            let (image, left, top, box_w, box_h) = match &layer.kind {
                LayerKind::Text { field, text, style, layout } => {
                    let txt = field.as_ref().and_then(|f| values.get(f).cloned()).or_else(|| text.clone()).unwrap_or_default();
                    let block = TextBlock::plain(txt).with_layout(layout.clone());
                    let l = engine.layout(&block, style, canvas);
                    let size_px = l.size_px;
                    let reveal = match (reveal_progress, phase) {
                        (Some((true, p)), _) => layer.intro.as_ref().map(|r| (r.clone(), p)),
                        (Some((false, p)), _) => layer.outro.as_ref().map(|r| (r.clone(), 1.0 - p)),
                        _ => None,
                    };
                    let motion = layer.motion.clone();
                    let time = t;
                    let unit_of = |r: &Reveal| r.unit;
                    let make = move |unit: TextUnit| {
                        let reveal = reveal.clone();
                        let motion = motion.clone();
                        move |i: usize, n: usize| -> UnitPose {
                            let mut pose = UnitPose::REST;
                            if let Some((r, p)) = &reveal {
                                if unit_of(r) == unit {
                                    pose = r.pose(*p, i, n, size_px);
                                }
                            }
                            if let Some(m) = &motion {
                                if m.unit == unit {
                                    let mp = m.pose(time, i, n, size_px);
                                    pose.offset[0] += mp.offset[0];
                                    pose.offset[1] += mp.offset[1];
                                    pose.scale[0] *= mp.scale[0];
                                    pose.scale[1] *= mp.scale[1];
                                    pose.rotation += mp.rotation;
                                    pose.opacity *= mp.opacity;
                                    if let Some(c) = mp.colour {
                                        pose.colour = Some(match pose.colour {
                                            Some(pc) => pc.lerp(c, c.a),
                                            None => {
                                                let base = style.fill.representative();
                                                base.lerp(c.alpha(1.0), c.a)
                                            }
                                        });
                                    }
                                }
                            }
                            pose
                        }
                    };
                    let fc = make(TextUnit::Character);
                    let fw = make(TextUnit::Word);
                    let fl = make(TextUnit::Line);
                    let hooks = Hooks { chars: Some(&fc), words: Some(&fw), lines: Some(&fl) };
                    let r = engine.paint(&l, style, &hooks);
                    (r.image, r.left as f32, r.top as f32, l.bounds.w, l.bounds.h)
                }
                LayerKind::Rect { colour, size, radius } => {
                    let (w, h) = ((size[0] * wmul * ppu).max(0.0), (size[1] * hmul * ppu).max(0.0));
                    let path = raster::rounded_rect_path(&Rect::new(0.0, 0.0, w, h), radius * ppu);
                    let (img, l, tp) = raster::path_image(&path, colour.linear_premul());
                    // Anchor-relative growth: the box the anchor refers to
                    // is the full-size one, so the shrunk shape sits inside it
                    // with its anchor point fixed (bottom anchor → grows up).
                    let full_w = size[0] * ppu;
                    let full_h = size[1] * ppu;
                    (img, l as f32 + (full_w - w) * layer.anchor[0], tp as f32 + (full_h - h) * layer.anchor[1], full_w, full_h)
                }
                LayerKind::Ellipse { colour, size } => {
                    let (w, h) = ((size[0] * wmul * ppu).max(0.0), (size[1] * hmul * ppu).max(0.0));
                    let path = raster::rounded_rect_path(&Rect::new(0.0, 0.0, w, h), w.max(h));
                    let (img, l, tp) = raster::path_image(&path, colour.linear_premul());
                    let full_w = size[0] * ppu;
                    let full_h = size[1] * ppu;
                    (img, l as f32 + (full_w - w) * layer.anchor[0], tp as f32 + (full_h - h) * layer.anchor[1], full_w, full_h)
                }
                LayerKind::Line { colour, length, thickness } => {
                    let (w, h) = ((length * wmul * ppu).max(0.0), (thickness * hmul * ppu).max(0.5));
                    let path = raster::rect_path(&Rect::new(0.0, 0.0, w, h));
                    let (img, l, tp) = raster::path_image(&path, colour.linear_premul());
                    let full_w = length * ppu;
                    let full_h = thickness * ppu;
                    (img, l as f32 + (full_w - w) * layer.anchor[0], tp as f32 + (full_h - h) * layer.anchor[1], full_w, full_h)
                }
            };
            if image.width == 0 || image.height == 0 {
                continue;
            }
            let mut image = image;
            if wipe_x < 1.0 || wipe_y < 1.0 {
                apply_wipe(&mut image, left, top, box_w, box_h, wipe_x, wipe_y);
            }
            // Image pixel → canvas: image origin is at (left, top) in box
            // space; box anchor at position (+ offset); then scale/rotate
            // about the anchor.
            let ax = canvas.width as f32 * layer.position[0] + dx;
            let ay = canvas.height as f32 * layer.position[1] + dy;
            let anchor_box = [box_w * layer.anchor[0], box_h * layer.anchor[1]];
            let transform = Affine::translate(left - anchor_box[0], top - anchor_box[1])
                .then(&Affine::scale(sx, sy))
                .then(&Affine::rotate(rotation))
                .then(&Affine::translate(ax, ay));
            out.push(LayerFrame { name: layer.name.clone(), image, transform, opacity });
        }
        out
    }

    /// Renders the scene at `t` onto a transparent canvas-sized image.
    pub fn render(&self, engine: &TextEngine, t: f32, duration: f32, canvas: &Canvas, values: &HashMap<String, String>) -> Image {
        let mut img = Image::new(canvas.width, canvas.height);
        self.render_onto(engine, &mut img, t, duration, values);
        img
    }

    /// Renders the scene over an existing canvas-sized image.
    pub fn render_onto(&self, engine: &TextEngine, target: &mut Image, t: f32, duration: f32, values: &HashMap<String, String>) {
        let canvas = Canvas::new(target.width, target.height);
        for lf in self.frame(engine, t, duration, &canvas, values) {
            raster::draw_image_affine(target, &lf.image, &lf.transform, lf.opacity);
        }
    }
}

/// Clears the part of a layer image outside the wipe fraction of its box.
fn apply_wipe(img: &mut Image, left: f32, top: f32, box_w: f32, box_h: f32, wx: f32, wy: f32) {
    let limit_x = -left + box_w * wx;
    let limit_y = -top + box_h * wy;
    let w = img.width as usize;
    for y in 0..img.height as usize {
        for x in 0..w {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let kx = (limit_x - px + 0.5).clamp(0.0, 1.0);
            let ky = (limit_y - py + 0.5).clamp(0.0, 1.0);
            let k = kx * ky;
            if k < 1.0 {
                let p = &mut img.pixels[y * w + x];
                for c in p.iter_mut() {
                    *c *= k;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn track_interpolates_and_holds() {
        let t = Track { property: Property::Opacity, phase: Phase::Intro, keys: vec![Key { t: 0.0, value: 0.0, ease: Ease::Linear }, Key { t: 1.0, value: 1.0, ease: Ease::Linear }] };
        assert_eq!(t.value(-1.0), 0.0);
        assert!((t.value(0.5) - 0.5).abs() < 1e-6);
        assert_eq!(t.value(5.0), 1.0);
    }

    #[test]
    fn phases_split_and_shrink() {
        let s = Scene::new("t", "t", TemplateCategory::Title, 5.0, 1.0, 1.0);
        assert_eq!(s.phase_at(0.5, 5.0).0, Phase::Intro);
        assert_eq!(s.phase_at(4.5, 5.0).0, Phase::Outro);
        assert!((s.phase_at(4.5, 5.0).1 - 0.5).abs() < 1e-6);
        // A 1 s clip: both phases shrink to 0.5 s each.
        let (p, local, k) = s.phase_at(0.75, 1.0);
        assert_eq!(p, Phase::Outro);
        assert!((k - 0.5).abs() < 1e-6);
        assert!((local - 0.5).abs() < 1e-6);
    }

    #[test]
    fn scene_json_round_trip() {
        let s = Scene::new("x", "X", TemplateCategory::Callout, 4.0, 0.8, 0.6)
            .field("title", "Title", "Hello")
            .layer(Layer::rect("bg", Rgba::BLACK, 100.0, 40.0, 4.0).in_out(Property::Width, 0.0, 1.0, 0.5, Ease::Out))
            .layer(Layer::text("t", "title", TextStyle::default(), LayoutParams::default()).reveal(Reveal::default(), Reveal::default()));
        let j = s.to_json();
        let back = Scene::from_json(&j).unwrap();
        assert_eq!(s, back);
    }
}
