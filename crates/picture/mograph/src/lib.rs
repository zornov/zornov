// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Motion graphics templates: a designed piece (a lower third, a title,
//! a callout, a chart…) with a few **controls** to fill in (the name, the
//! colour, the numbers), built into layers Studio can play: titles with
//! their text motion and shape layers with their own animation.
//!
//! [`all`] lists the templates, [`find`] one by id, [`Template::build`]
//! makes its layers for a canvas, a length and control values. Charts
//! ([`charts`]) are templates whose main control is the data.
//!
//! Every design, timing and name here is Studio's own.

pub mod catalogue;
pub mod charts;
pub mod data;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

/// What kind of piece a template is (the browser's sections).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    LowerThird,
    Title,
    Callout,
    Social,
    Transition,
    Infographic,
    Chart,
    EndCard,
}

impl Category {
    pub const ALL: [Category; 8] = [Category::LowerThird, Category::Title, Category::Callout, Category::Social, Category::Transition, Category::Infographic, Category::Chart, Category::EndCard];

    pub fn label(self) -> &'static str {
        match self {
            Category::LowerThird => "Lower thirds",
            Category::Title => "Titles",
            Category::Callout => "Callouts",
            Category::Social => "Social",
            Category::Transition => "Transitions",
            Category::Infographic => "Infographics",
            Category::Chart => "Charts",
            Category::EndCard => "End cards",
        }
    }
}

/// What a control holds.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Kind {
    Text,
    Colour,
    Number { min: f64, max: f64 },
    Choice { options: Vec<&'static str> },
    Toggle,
    /// Rows of a table: `[["label", value], …]`, or text "label, value"
    /// a line.
    Data,
    /// A point on the canvas, pixels from the centre.
    Point,
}

/// One control a template shows.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Control {
    pub id: &'static str,
    pub label: &'static str,
    #[serde(flatten)]
    pub kind: Kind,
    pub default: Value,
}

impl Control {
    pub fn text(id: &'static str, label: &'static str, default: &str) -> Control {
        Control { id, label, kind: Kind::Text, default: json!(default) }
    }
    pub fn colour(id: &'static str, label: &'static str, default: &str) -> Control {
        Control { id, label, kind: Kind::Colour, default: json!(default) }
    }
    pub fn number(id: &'static str, label: &'static str, default: f64, min: f64, max: f64) -> Control {
        Control { id, label, kind: Kind::Number { min, max }, default: json!(default) }
    }
    pub fn choice(id: &'static str, label: &'static str, options: &[&'static str], default: &str) -> Control {
        Control { id, label, kind: Kind::Choice { options: options.to_vec() }, default: json!(default) }
    }
    pub fn toggle(id: &'static str, label: &'static str, default: bool) -> Control {
        Control { id, label, kind: Kind::Toggle, default: json!(default) }
    }
    pub fn data(id: &'static str, label: &'static str, default: Value) -> Control {
        Control { id, label, kind: Kind::Data, default }
    }
    pub fn point(id: &'static str, label: &'static str, default: [f64; 2]) -> Control {
        Control { id, label, kind: Kind::Point, default: json!(default) }
    }
}

/// How a text layer's `x` is read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Anchor {
    Left,
    #[default]
    Centre,
    Right,
}

/// A number that counts while it shows (the text is the number).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Counter {
    pub from: f64,
    pub to: f64,
    /// Seconds into the layer it starts counting, and for how long.
    pub start: f64,
    pub duration: f64,
    #[serde(default)]
    pub decimals: u32,
    #[serde(default)]
    pub prefix: String,
    #[serde(default)]
    pub suffix: String,
    /// Thousands separated with commas.
    #[serde(default)]
    pub separator: bool,
}

impl Counter {
    /// The text at `t` seconds into the layer.
    pub fn at(&self, t: f64) -> String {
        let u = if self.duration <= 0.0 { 1.0 } else { ((t - self.start) / self.duration).clamp(0.0, 1.0) };
        // Fast then settling, as counters read best.
        let k = 1.0 - (1.0 - u).powi(3);
        let v = self.from + (self.to - self.from) * k;
        format!("{}{}{}", self.prefix, format_number(v, self.decimals, self.separator), self.suffix)
    }
}

/// `v` with `decimals` places, thousands separated by commas if asked.
pub fn format_number(v: f64, decimals: u32, separator: bool) -> String {
    let s = format!("{:.*}", decimals as usize, v);
    if !separator {
        return s;
    }
    let (neg, s) = s.strip_prefix('-').map_or((false, s.as_str()), |r| (true, r));
    let (int, frac) = s.split_once('.').map_or((s, None), |(a, b)| (a, Some(b)));
    let mut out = String::new();
    for (i, c) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    format!("{}{}{}", if neg { "-" } else { "" }, out, frac.map(|f| format!(".{f}")).unwrap_or_default())
}

/// A title layer.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TextLayer {
    pub name: String,
    pub text: String,
    pub font: String,
    pub weight: u16,
    pub italic: bool,
    /// Canvas pixels.
    pub size: f64,
    pub colour: String,
    /// Percent of the size.
    pub letter_spacing: f64,
    pub anchor: Anchor,
    pub x: f64,
    pub y: f64,
    /// Seconds into the template, and how long it shows.
    pub start: f64,
    pub duration: f64,
    pub motion: Option<studio_text::TextMotion>,
    pub counter: Option<Counter>,
    /// A path to follow, as the `text` op takes it (`{"circle": r}`,
    /// `{"arc": r, "degrees"}`, `{"wave": …}`, `{"d": …}`, `{"points":
    /// …}`); `start` there may be a number or `{"keys": [[t, v], …]}`.
    pub path: Option<Value>,
}

/// A shape layer (a `studio_vector` document).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ShapeLayer {
    pub name: String,
    pub doc: Value,
    pub x: f64,
    pub y: f64,
    pub start: f64,
    pub duration: f64,
}

/// One layer of a built template, bottom first.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "layer", rename_all = "snake_case")]
pub enum Layer {
    Text(TextLayer),
    Shape(ShapeLayer),
}

impl Layer {
    pub fn start(&self) -> f64 {
        match self {
            Layer::Text(t) => t.start,
            Layer::Shape(s) => s.start,
        }
    }
    pub fn duration(&self) -> f64 {
        match self {
            Layer::Text(t) => t.duration,
            Layer::Shape(s) => s.duration,
        }
    }
}

/// Measures a line of text: (text, font, weight, size px, letter spacing
/// %) → (width, height) in pixels.
pub type Measure<'a> = &'a dyn Fn(&str, &str, u16, f64, f64) -> [f64; 2];

/// What a template is built with.
pub struct Ctx<'a> {
    pub values: &'a Map<String, Value>,
    pub controls: &'a [Control],
    /// Canvas size in pixels.
    pub w: f64,
    pub h: f64,
    /// Seconds the template lasts.
    pub dur: f64,
    pub measure: Measure<'a>,
}

impl Ctx<'_> {
    fn raw(&self, id: &str) -> Value {
        self.values.get(id).cloned().or_else(|| self.controls.iter().find(|c| c.id == id).map(|c| c.default.clone())).unwrap_or(Value::Null)
    }
    pub fn text(&self, id: &str) -> String {
        match self.raw(id) {
            Value::String(s) => s,
            Value::Number(n) => n.to_string(),
            Value::Null => String::new(),
            other => other.to_string(),
        }
    }
    pub fn colour(&self, id: &str) -> String {
        self.text(id)
    }
    pub fn num(&self, id: &str) -> f64 {
        match self.raw(id) {
            Value::Number(n) => n.as_f64().unwrap_or(0.0),
            Value::String(s) => s.trim().trim_end_matches('%').parse().unwrap_or(0.0),
            Value::Bool(b) => b as u8 as f64,
            _ => 0.0,
        }
    }
    pub fn flag(&self, id: &str) -> bool {
        match self.raw(id) {
            Value::Bool(b) => b,
            Value::Number(n) => n.as_f64().unwrap_or(0.0) != 0.0,
            Value::String(s) => matches!(s.as_str(), "yes" | "true" | "on"),
            _ => false,
        }
    }
    pub fn point(&self, id: &str) -> [f64; 2] {
        match self.raw(id) {
            Value::Array(a) => [a.first().and_then(Value::as_f64).unwrap_or(0.0), a.get(1).and_then(Value::as_f64).unwrap_or(0.0)],
            _ => [0.0, 0.0],
        }
    }
    pub fn data(&self, id: &str) -> Vec<data::Row> {
        data::rows(&self.raw(id))
    }
    /// The scale of a design drawn for 1920 × 1080 on this canvas.
    pub fn k(&self) -> f64 {
        (self.w / 1920.0).min(self.h / 1080.0).max(0.05)
    }
    pub fn measure(&self, text: &str, font: &str, weight: u16, size: f64, spacing: f64) -> [f64; 2] {
        (self.measure)(text, font, weight, size, spacing)
    }
}

/// A template.
pub struct Template {
    pub id: &'static str,
    pub name: &'static str,
    pub category: Category,
    pub description: &'static str,
    pub tags: &'static str,
    /// Seconds it is designed to last (it stretches).
    pub duration: f64,
    pub controls: Vec<Control>,
    pub build: fn(&Ctx) -> Vec<Layer>,
}

impl Template {
    /// The layers for a canvas `w × h`, lasting `dur` seconds, with
    /// `values` for the controls (missing ones take their defaults).
    pub fn build(&self, values: &Map<String, Value>, w: f64, h: f64, dur: f64, measure: Measure) -> Result<Vec<Layer>, String> {
        for k in values.keys() {
            if !self.controls.iter().any(|c| c.id == k) {
                return Err(format!("{} has no control \"{k}\"; controls: {}", self.id, self.controls.iter().map(|c| c.id).collect::<Vec<_>>().join(", ")));
            }
        }
        let cx = Ctx { values, controls: &self.controls, w, h, dur: dur.max(0.5), measure };
        Ok((self.build)(&cx))
    }

    /// The template as the browser and the AI see it.
    pub fn describe(&self) -> Value {
        json!({"id": self.id, "name": self.name, "category": self.category, "description": self.description, "tags": self.tags, "duration": self.duration, "controls": self.controls})
    }
}

/// Every template.
pub fn all() -> &'static [Template] {
    catalogue::all()
}

pub fn find(id: &str) -> Option<&'static Template> {
    let id = id.trim().to_ascii_lowercase();
    all().iter().find(|t| t.id == id || t.name.to_ascii_lowercase() == id)
}

/// Templates matching words in their id, name, tags or description.
pub fn search(query: &str) -> Vec<&'static Template> {
    let words: Vec<String> = query.to_lowercase().split_whitespace().map(str::to_owned).collect();
    let mut hits: Vec<(usize, &Template)> = all()
        .iter()
        .map(|t| {
            let hay = format!("{} {} {} {} {}", t.id, t.name, t.tags, t.description, t.category.label()).to_lowercase();
            (words.iter().filter(|w| hay.contains(w.as_str())).count(), t)
        })
        .filter(|(n, _)| *n > 0 || words.is_empty())
        .collect();
    hits.sort_by(|a, b| b.0.cmp(&a.0));
    hits.into_iter().map(|(_, t)| t).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn fake_measure(text: &str, _font: &str, _w: u16, size: f64, spacing: f64) -> [f64; 2] {
        [text.chars().count() as f64 * size * (0.56 + spacing / 100.0), size * 1.2]
    }

    #[test]
    fn every_template_builds_and_its_shapes_draw() {
        for t in all() {
            for (w, h) in [(1920.0, 1080.0), (1080.0, 1920.0)] {
                let layers = t.build(&Map::new(), w, h, t.duration, &fake_measure).unwrap_or_else(|e| panic!("{}: {e}", t.id));
                assert!(!layers.is_empty(), "{} builds nothing", t.id);
                for l in &layers {
                    assert!(l.start() >= 0.0 && l.start() + l.duration() <= t.duration + 1e-6, "{}: a layer runs outside the template", t.id);
                    if let Layer::Shape(s) = l {
                        let warns = studio_vector::check(&s.doc);
                        assert!(warns.is_empty(), "{} {}: {warns:?}", t.id, s.name);
                    }
                    if let Layer::Text(x) = l {
                        assert!(!x.text.is_empty() || x.counter.is_some(), "{}: empty text", t.id);
                        assert!(x.size > 4.0);
                    }
                }
            }
        }
        assert!(all().len() >= 20, "{}", all().len());
        let ids: std::collections::HashSet<&str> = all().iter().map(|t| t.id).collect();
        assert_eq!(ids.len(), all().len(), "ids are unique");
    }

    #[test]
    fn controls_fill_in_and_strangers_are_named() {
        let t = find("lower_third.bar").unwrap();
        let mut v = Map::new();
        v.insert("name".into(), json!("Ada Lovelace"));
        let layers = t.build(&v, 1920.0, 1080.0, 5.0, &fake_measure).unwrap();
        assert!(layers.iter().any(|l| matches!(l, Layer::Text(x) if x.text == "Ada Lovelace")));
        v.insert("nope".into(), json!(1));
        assert!(t.build(&v, 1920.0, 1080.0, 5.0, &fake_measure).unwrap_err().contains("nope"));
        assert!(!search("lower third news").is_empty());
        assert_eq!(format_number(1234567.891, 1, true), "1,234,567.9");
        assert_eq!(format_number(-999.0, 0, true), "-999");
        let c = Counter { from: 0.0, to: 100.0, start: 0.0, duration: 1.0, decimals: 0, prefix: "$".into(), suffix: "k".into(), separator: false };
        assert_eq!(c.at(5.0), "$100k");
        assert_eq!(c.at(0.0), "$0k");
    }
}
