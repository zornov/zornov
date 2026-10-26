// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The styling stack: fill, strokes, shadow, glow, background box,
//! extrusion and per-character colour cycles, plus the text block types
//! (spans, alignment, wrapping) that layout consumes.
//!
//! Lengths are in **canvas units**: 1 unit = 1/1080 of the canvas height,
//! so a 40-unit title looks the same on a 1080p and a 4K canvas. Font size
//! 5..300 units matches the inspector range. Letter and line spacing are
//! percentages of the font size (−10..100 in the inspector).

use serde::{Deserialize, Serialize};

use crate::font::FontRequest;
use crate::rgba::Rgba;

/// The canvas the text is laid out for; fixes the unit → pixel scale.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Canvas {
    pub width: u32,
    pub height: u32,
}

impl Canvas {
    pub const HD: Canvas = Canvas { width: 1920, height: 1080 };
    pub const VERTICAL: Canvas = Canvas { width: 1080, height: 1920 };
    pub const SQUARE: Canvas = Canvas { width: 1080, height: 1080 };

    pub const fn new(width: u32, height: u32) -> Canvas {
        Canvas { width, height }
    }

    /// Pixels per canvas unit.
    pub fn px_per_unit(&self) -> f32 {
        self.height as f32 / 1080.0
    }

    pub fn px(&self, units: f32) -> f32 {
        units * self.px_per_unit()
    }

    pub fn units(&self, px: f32) -> f32 {
        px / self.px_per_unit()
    }
}

/// Where a gradient's 0..1 axis spans.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GradientScope {
    /// Across the whole text block.
    #[default]
    Text,
    /// Restarts on every line.
    Line,
    /// Restarts on every glyph.
    Glyph,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GradientStop {
    pub at: f32,
    pub colour: Rgba,
}

/// A linear gradient. `angle` 0 runs left→right, 90 top→bottom.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Gradient {
    pub stops: Vec<GradientStop>,
    #[serde(default)]
    pub angle: f32,
    #[serde(default)]
    pub scope: GradientScope,
}

impl Gradient {
    pub fn new(angle: f32, colours: &[Rgba]) -> Gradient {
        let n = colours.len().max(2) - 1;
        Gradient {
            stops: colours.iter().enumerate().map(|(i, c)| GradientStop { at: i as f32 / n as f32, colour: *c }).collect(),
            angle,
            scope: GradientScope::Text,
        }
    }

    pub fn scope(mut self, scope: GradientScope) -> Gradient {
        self.scope = scope;
        self
    }

    /// Colour at `t` (clamped), interpolated in sRGB between stops.
    pub fn at(&self, t: f32) -> Rgba {
        let t = t.clamp(0.0, 1.0);
        match self.stops.len() {
            0 => Rgba::WHITE,
            1 => self.stops[0].colour,
            _ => {
                if t <= self.stops[0].at {
                    return self.stops[0].colour;
                }
                for w in self.stops.windows(2) {
                    if t <= w[1].at {
                        let span = (w[1].at - w[0].at).max(1e-6);
                        return w[0].colour.lerp(w[1].colour, (t - w[0].at) / span);
                    }
                }
                self.stops.last().unwrap().colour
            }
        }
    }
}

/// How the text face is painted.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Fill {
    Solid(Rgba),
    Gradient(Gradient),
    /// An image registered with the engine under `key`, scaled to cover
    /// the text bounds.
    Image {
        key: String,
        #[serde(default = "one")]
        opacity: f32,
    },
    /// No face: strokes, shadows and glows still draw (outline styles).
    None,
}

fn one() -> f32 {
    1.0
}

impl Default for Fill {
    fn default() -> Self {
        Fill::Solid(Rgba::WHITE)
    }
}

impl Fill {
    pub fn solid(c: Rgba) -> Fill {
        Fill::Solid(c)
    }

    /// A representative colour (first stop for gradients), for UI swatches
    /// and for underlines.
    pub fn representative(&self) -> Rgba {
        match self {
            Fill::Solid(c) => *c,
            Fill::Gradient(g) => g.at(0.0),
            Fill::Image { .. } => Rgba::WHITE,
            Fill::None => Rgba::TRANSPARENT,
        }
    }
}

/// An outline drawn outside the glyph edge. Several stack, first = innermost.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stroke {
    pub colour: Rgba,
    /// Width in canvas units, measured outward from the glyph edge.
    pub width: f32,
    #[serde(default = "one")]
    pub opacity: f32,
}

impl Stroke {
    pub fn new(colour: Rgba, width: f32) -> Stroke {
        Stroke { colour, width, opacity: 1.0 }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Shadow {
    pub colour: Rgba,
    /// Gaussian blur radius in units (0 = hard).
    pub blur: f32,
    pub distance: f32,
    /// Degrees, 0 = to the right, 90 = straight down.
    pub angle: f32,
    #[serde(default = "one")]
    pub opacity: f32,
}

impl Shadow {
    pub fn new(colour: Rgba, blur: f32, distance: f32, angle: f32) -> Shadow {
        Shadow { colour, blur, distance, angle, opacity: 1.0 }
    }

    pub fn opacity(mut self, o: f32) -> Shadow {
        self.opacity = o;
        self
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Glow {
    pub colour: Rgba,
    /// 0..1 strength of the halo.
    pub intensity: f32,
    /// How far the halo reaches, in units.
    pub range: f32,
    #[serde(default)]
    pub offset: [f32; 2],
}

impl Glow {
    pub fn new(colour: Rgba, intensity: f32, range: f32) -> Glow {
        Glow { colour, intensity, range, offset: [0.0, 0.0] }
    }
}

/// A rounded box behind the text.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Background {
    pub colour: Rgba,
    pub radius: f32,
    pub padding_x: f32,
    pub padding_y: f32,
    /// Extra width/height as a fraction of the text size (0 = fit the text).
    #[serde(default)]
    pub width: f32,
    #[serde(default)]
    pub height: f32,
    #[serde(default)]
    pub offset: [f32; 2],
    /// One box per line instead of one around the block.
    #[serde(default)]
    pub per_line: bool,
}

impl Background {
    pub fn new(colour: Rgba, radius: f32, padding_x: f32, padding_y: f32) -> Background {
        Background { colour, radius, padding_x, padding_y, width: 0.0, height: 0.0, offset: [0.0, 0.0], per_line: false }
    }

    pub fn per_line(mut self) -> Background {
        self.per_line = true;
        self
    }
}

/// Offset copies behind the face that read as depth.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Extrusion {
    pub colour: Rgba,
    /// Total depth in units.
    pub depth: f32,
    /// Degrees, 0 = to the right, 90 = straight down.
    pub angle: f32,
    /// Darken the colour towards the far end.
    #[serde(default)]
    pub shade: bool,
}

impl Extrusion {
    pub fn new(colour: Rgba, depth: f32, angle: f32) -> Extrusion {
        Extrusion { colour, depth, angle, shade: true }
    }
}

/// What a per-unit effect iterates over.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextUnit {
    #[default]
    Character,
    Word,
    Line,
}

/// Colours that cycle unit by unit ("rainbow" styles).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ColourCycle {
    pub colours: Vec<Rgba>,
    #[serde(default)]
    pub unit: TextUnit,
    /// Starting index into `colours`.
    #[serde(default)]
    pub phase: usize,
}

impl ColourCycle {
    pub fn new(colours: &[Rgba], unit: TextUnit) -> ColourCycle {
        ColourCycle { colours: colours.to_vec(), unit, phase: 0 }
    }

    pub fn colour(&self, index: usize) -> Rgba {
        if self.colours.is_empty() { Rgba::WHITE } else { self.colours[(index + self.phase) % self.colours.len()] }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Case {
    #[default]
    AsTyped,
    Upper,
    Lower,
    Title,
}

impl Case {
    pub fn apply(self, s: &str) -> String {
        match self {
            Case::AsTyped => s.to_string(),
            Case::Upper => s.to_uppercase(),
            Case::Lower => s.to_lowercase(),
            Case::Title => {
                let mut out = String::with_capacity(s.len());
                let mut start = true;
                for c in s.chars() {
                    if c.is_whitespace() {
                        start = true;
                        out.push(c);
                    } else if start {
                        out.extend(c.to_uppercase());
                        start = false;
                    } else {
                        out.push(c);
                    }
                }
                out
            }
        }
    }
}

/// The complete look of a text block.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TextStyle {
    #[serde(default)]
    pub font: FontRequest,
    /// Font size in canvas units (5..300).
    #[serde(default = "default_size")]
    pub size: f32,
    /// Extra space between characters, % of the font size.
    #[serde(default)]
    pub letter_spacing: f32,
    /// Extra space between lines, % of the font size.
    #[serde(default)]
    pub line_spacing: f32,
    #[serde(default)]
    pub fill: Fill,
    #[serde(default)]
    pub strokes: Vec<Stroke>,
    #[serde(default)]
    pub shadow: Option<Shadow>,
    #[serde(default)]
    pub glow: Option<Glow>,
    #[serde(default)]
    pub background: Option<Background>,
    #[serde(default)]
    pub extrusion: Option<Extrusion>,
    #[serde(default)]
    pub colour_cycle: Option<ColourCycle>,
    #[serde(default)]
    pub underline: bool,
    #[serde(default)]
    pub strikethrough: bool,
    #[serde(default)]
    pub case: Case,
    /// Whole-block opacity 0..1.
    #[serde(default = "one")]
    pub opacity: f32,
    /// The font's own kerning between letter pairs (on by default).
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub kerning: bool,
}

fn yes() -> bool {
    true
}

fn is_true(b: &bool) -> bool {
    *b
}

fn default_size() -> f32 {
    40.0
}

impl Default for TextStyle {
    fn default() -> Self {
        TextStyle {
            font: FontRequest::default(),
            size: 40.0,
            letter_spacing: 0.0,
            line_spacing: 0.0,
            fill: Fill::default(),
            strokes: Vec::new(),
            shadow: None,
            glow: None,
            background: None,
            extrusion: None,
            colour_cycle: None,
            underline: false,
            strikethrough: false,
            case: Case::AsTyped,
            opacity: 1.0,
            kerning: true,
        }
    }
}

impl TextStyle {
    pub fn new(font: FontRequest, size: f32, fill: Fill) -> TextStyle {
        TextStyle { font, size, fill, ..Default::default() }
    }

    pub fn with_stroke(mut self, s: Stroke) -> TextStyle {
        self.strokes.push(s);
        self
    }

    pub fn with_shadow(mut self, s: Shadow) -> TextStyle {
        self.shadow = Some(s);
        self
    }

    pub fn with_glow(mut self, g: Glow) -> TextStyle {
        self.glow = Some(g);
        self
    }

    pub fn with_background(mut self, b: Background) -> TextStyle {
        self.background = Some(b);
        self
    }

    pub fn with_extrusion(mut self, e: Extrusion) -> TextStyle {
        self.extrusion = Some(e);
        self
    }

    pub fn with_cycle(mut self, c: ColourCycle) -> TextStyle {
        self.colour_cycle = Some(c);
        self
    }

    pub fn letter_spacing(mut self, pct: f32) -> TextStyle {
        self.letter_spacing = pct;
        self
    }

    pub fn line_spacing(mut self, pct: f32) -> TextStyle {
        self.line_spacing = pct;
        self
    }

    pub fn case(mut self, c: Case) -> TextStyle {
        self.case = c;
        self
    }

    pub fn size(mut self, s: f32) -> TextStyle {
        self.size = s;
        self
    }

    /// The farthest any decoration reaches outside the glyph outlines, in
    /// units; used to size layers and hit areas.
    pub fn reach(&self) -> f32 {
        let mut r = self.strokes.iter().map(|s| s.width).fold(0.0, f32::max);
        if let Some(s) = &self.shadow {
            r = r.max(s.distance + s.blur * 3.0);
        }
        if let Some(g) = &self.glow {
            r = r.max(g.range * 3.0 + g.offset[0].abs().max(g.offset[1].abs()));
        }
        if let Some(e) = &self.extrusion {
            r = r.max(e.depth);
        }
        if let Some(b) = &self.background {
            r = r.max(b.padding_x.max(b.padding_y) + b.offset[0].abs().max(b.offset[1].abs()) + self.size * b.width.max(b.height));
        }
        r
    }
}

/// Per-span overrides on top of the block's [`TextStyle`].
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct SpanStyle {
    #[serde(default)]
    pub font: Option<FontRequest>,
    #[serde(default)]
    pub bold: Option<bool>,
    #[serde(default)]
    pub italic: Option<bool>,
    /// Multiplier on the block font size.
    #[serde(default)]
    pub size_scale: Option<f32>,
    #[serde(default)]
    pub fill: Option<Fill>,
    #[serde(default)]
    pub underline: Option<bool>,
    #[serde(default)]
    pub strikethrough: Option<bool>,
    /// CSS weight 100..900 (variable fonts take any value between).
    #[serde(default)]
    pub weight: Option<u16>,
    /// Tracking for this run, percent of the font size like
    /// [`TextStyle::letter_spacing`].
    #[serde(default)]
    pub letter_spacing: Option<f32>,
    /// Lifts the run off the baseline, canvas units (negative lowers it):
    /// superscripts, subscripts, a word jumping out.
    #[serde(default)]
    pub baseline_shift: Option<f32>,
}

impl SpanStyle {
    pub fn bold() -> SpanStyle {
        SpanStyle { bold: Some(true), ..Default::default() }
    }

    pub fn italic() -> SpanStyle {
        SpanStyle { italic: Some(true), ..Default::default() }
    }

    pub fn colour(c: Rgba) -> SpanStyle {
        SpanStyle { fill: Some(Fill::Solid(c)), ..Default::default() }
    }

    /// The font this span actually asks for.
    pub fn font(&self, base: &FontRequest) -> FontRequest {
        let mut f = self.font.clone().unwrap_or_else(|| base.clone());
        if let Some(b) = self.bold {
            f.weight = if b { base.weight.max(700) } else { 400 };
        }
        if let Some(i) = self.italic {
            f.italic = i;
        }
        if let Some(w) = self.weight {
            f.weight = w.clamp(1, 1000);
        }
        f
    }
}

/// A run of text with optional overrides.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Span {
    pub text: String,
    #[serde(default, skip_serializing_if = "is_default_span")]
    pub style: SpanStyle,
}

fn is_default_span(s: &SpanStyle) -> bool {
    *s == SpanStyle::default()
}

impl Span {
    pub fn plain(text: impl Into<String>) -> Span {
        Span { text: text.into(), style: SpanStyle::default() }
    }

    pub fn styled(text: impl Into<String>, style: SpanStyle) -> Span {
        Span { text: text.into(), style }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Align {
    Left,
    #[default]
    Centre,
    Right,
    Justify,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VAlign {
    Top,
    #[default]
    Middle,
    Bottom,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    #[default]
    Auto,
    Ltr,
    Rtl,
}

/// How the block sits in its box.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LayoutParams {
    #[serde(default)]
    pub align: Align,
    #[serde(default)]
    pub valign: VAlign,
    /// Wrap width in units; `None` breaks only at newlines.
    #[serde(default)]
    pub wrap_width: Option<f32>,
    /// Box height in units, for vertical alignment and fit-to-box.
    #[serde(default)]
    pub box_height: Option<f32>,
    /// Shrink (or grow) the font size so the text fits the box.
    #[serde(default)]
    pub fit_to_box: bool,
    /// Lay characters top to bottom, lines right to left.
    #[serde(default)]
    pub vertical: bool,
    #[serde(default)]
    pub direction: Direction,
    /// Bend each line along an arc: degrees of arc the line spans,
    /// −360..360. Positive arches upward (ends drop), negative bowls.
    #[serde(default)]
    pub bend: f32,
    #[serde(default)]
    pub max_lines: Option<usize>,
    /// Lay the text along a path instead of straight lines.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<TextPath>,
}

/// A path for text to follow.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TextPath {
    /// The path as a polyline, canvas units from the text's centre (y
    /// down). Curves come in densely sampled.
    pub points: Vec<[f32; 2]>,
    /// The path joins its end to its start (text can run round).
    #[serde(default)]
    pub closed: bool,
    /// Where along the path (0..1) the line sits: its left edge, centre or
    /// right edge as the text is aligned. Key it to run text along.
    #[serde(default = "half")]
    pub start: f32,
    /// Distance off the path, canvas units (positive above it).
    #[serde(default)]
    pub offset: f32,
    /// Letters turn to follow the path (off: they stay upright).
    #[serde(default = "yes")]
    pub perpendicular: bool,
    /// Runs the other way along the path (text on the other side).
    #[serde(default)]
    pub reverse: bool,
}

fn half() -> f32 {
    0.5
}

impl Default for LayoutParams {
    fn default() -> Self {
        LayoutParams {
            align: Align::Centre,
            valign: VAlign::Middle,
            wrap_width: None,
            box_height: None,
            fit_to_box: false,
            vertical: false,
            direction: Direction::Auto,
            bend: 0.0,
            max_lines: None,
            path: None,
        }
    }
}

impl LayoutParams {
    pub fn align(mut self, a: Align) -> LayoutParams {
        self.align = a;
        self
    }

    pub fn wrap(mut self, width: f32) -> LayoutParams {
        self.wrap_width = Some(width);
        self
    }

    pub fn boxed(mut self, width: f32, height: f32) -> LayoutParams {
        self.wrap_width = Some(width);
        self.box_height = Some(height);
        self
    }

    pub fn fit(mut self) -> LayoutParams {
        self.fit_to_box = true;
        self
    }

    pub fn bend(mut self, degrees: f32) -> LayoutParams {
        self.bend = degrees.clamp(-360.0, 360.0);
        self
    }
}

/// Text plus how to lay it out; the style comes separately so one style
/// can serve many blocks.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TextBlock {
    pub spans: Vec<Span>,
    #[serde(default)]
    pub layout: LayoutParams,
}

impl TextBlock {
    pub fn plain(text: impl Into<String>) -> TextBlock {
        TextBlock { spans: vec![Span::plain(text)], layout: LayoutParams::default() }
    }

    pub fn spans(spans: Vec<Span>) -> TextBlock {
        TextBlock { spans, layout: LayoutParams::default() }
    }

    pub fn with_layout(mut self, layout: LayoutParams) -> TextBlock {
        self.layout = layout;
        self
    }

    /// All span text joined.
    pub fn text(&self) -> String {
        self.spans.iter().map(|s| s.text.as_str()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gradient_interpolates() {
        let g = Gradient::new(0.0, &[Rgba::BLACK, Rgba::WHITE]);
        let mid = g.at(0.5);
        assert!((mid.r - 0.5).abs() < 1e-6);
        assert_eq!(g.at(-1.0), Rgba::BLACK);
        assert_eq!(g.at(2.0), Rgba::WHITE);
    }

    #[test]
    fn title_case() {
        assert_eq!(Case::Title.apply("hello big world"), "Hello Big World");
        assert_eq!(Case::Upper.apply("straße"), "STRASSE");
    }

    #[test]
    fn style_round_trips_through_json() {
        let s = TextStyle::default().with_stroke(Stroke::new(Rgba::BLACK, 2.0)).with_shadow(Shadow::new(Rgba::BLACK, 3.0, 2.0, 90.0));
        let j = serde_json::to_string(&s).unwrap();
        let back: TextStyle = serde_json::from_str(&j).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn canvas_units_scale_with_height() {
        assert_eq!(Canvas::HD.px(40.0), 40.0);
        assert_eq!(Canvas::new(3840, 2160).px(40.0), 80.0);
        assert_eq!(Canvas::VERTICAL.px(10.8), 19.2);
    }
}
