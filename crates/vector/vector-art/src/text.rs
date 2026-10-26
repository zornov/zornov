// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Type: point type (one line per paragraph from an anchor), area type
//! (wrapped in a box, in columns, flowing around wrap objects) and type on
//! a path, with character and paragraph styles and OpenType features. Text
//! is shaped with the fonts Studio finds and drawn as glyph outlines, so it
//! is exact at every size and can be turned into outlines.

use std::sync::{Arc, OnceLock};

use kurbo::{Affine, BezPath, ParamCurve, ParamCurveArclen, ParamCurveDeriv, Point, Rect, Shape};
use rustybuzz::ttf_parser;
use serde::{Deserialize, Serialize};
use studio_text::font::{FontLibrary, FontMatch, FontRequest};
use studio_vector_geom::PathData;
use studio_vector_raster::Color;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
    /// Lines fill the width (the last line of a paragraph aligns left).
    Justify,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Case {
    #[default]
    Normal,
    Upper,
    Lower,
    /// Small capitals: the font's `smcp` feature, or scaled capitals.
    SmallCaps,
    Title,
}

/// Character properties; unset fields come from the named style, then
/// the defaults.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CharProps {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weight: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub italic: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<f64>,
    /// Line spacing in document units (default 1.2 × size).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub leading: Option<f64>,
    /// Extra space between letters, thousandths of an em.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracking: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baseline_shift: Option<f64>,
    /// Horizontal and vertical scale, percent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale: Option<(f64, f64)>,
    /// OpenType features: `"liga"`, `"smcp"`, `"ss01"`, `"onum"`, or off
    /// with a minus: `"-kern"`, `"-liga"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub features: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub case: Option<Case>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub underline: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strikethrough: Option<bool>,
    /// This run's fill colour (otherwise the text object's appearance).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<Color>,
}

impl CharProps {
    /// `self` with `base` filling what it leaves unset.
    pub fn over(&self, base: &CharProps) -> CharProps {
        CharProps {
            font: self.font.clone().or_else(|| base.font.clone()),
            weight: self.weight.or(base.weight),
            italic: self.italic.or(base.italic),
            size: self.size.or(base.size),
            leading: self.leading.or(base.leading),
            tracking: self.tracking.or(base.tracking),
            baseline_shift: self.baseline_shift.or(base.baseline_shift),
            scale: self.scale.or(base.scale),
            features: self.features.clone().or_else(|| base.features.clone()),
            case: self.case.or(base.case),
            underline: self.underline.or(base.underline),
            strikethrough: self.strikethrough.or(base.strikethrough),
            color: self.color.or(base.color),
        }
    }

    fn size(&self) -> f64 {
        self.size.unwrap_or(24.0).clamp(0.1, 10_000.0)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ParaProps {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub align: Option<Align>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_indent: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub left_indent: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub right_indent: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space_before: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space_after: Option<f64>,
}

impl ParaProps {
    pub fn over(&self, base: &ParaProps) -> ParaProps {
        ParaProps {
            align: self.align.or(base.align),
            first_indent: self.first_indent.or(base.first_indent),
            left_indent: self.left_indent.or(base.left_indent),
            right_indent: self.right_indent.or(base.right_indent),
            space_before: self.space_before.or(base.space_before),
            space_after: self.space_after.or(base.space_after),
        }
    }
}

/// A named character style.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CharStyle {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(flatten)]
    pub props: CharProps,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ParaStyle {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(flatten)]
    pub props: ParaProps,
    /// The character style the paragraph's text starts from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub char_style: Option<String>,
}

/// A stretch of text with its own character properties.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Run {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
    #[serde(flatten)]
    pub props: CharProps,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TextKind {
    /// From an anchor: the first baseline starts at `at`; lines break only
    /// at newlines.
    Point { at: Point },
    /// Inside `rect`, wrapped, in `columns` separated by `gutter`.
    Area {
        rect: Rect,
        #[serde(default = "one")]
        columns: u32,
        #[serde(default)]
        gutter: f64,
    },
    /// Along a path, starting `offset` units in; `flip` puts the text on
    /// the other side, running the other way.
    OnPath {
        path: PathData,
        #[serde(default)]
        offset: f64,
        #[serde(default)]
        flip: bool,
    },
}

fn one() -> u32 {
    1
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TextNode {
    #[serde(flatten)]
    pub kind: TextKind,
    pub runs: Vec<Run>,
    /// The base character properties for every run.
    #[serde(default, skip_serializing_if = "is_default_char")]
    pub char: CharProps,
    #[serde(default, skip_serializing_if = "is_default_para")]
    pub para: ParaProps,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub para_style: Option<String>,
}

fn is_default_char(c: &CharProps) -> bool {
    *c == CharProps::default()
}

fn is_default_para(p: &ParaProps) -> bool {
    *p == ParaProps::default()
}

impl TextNode {
    pub fn point(at: Point, text: &str, size: f64) -> TextNode {
        TextNode {
            kind: TextKind::Point { at },
            runs: vec![Run { text: text.into(), style: None, props: CharProps::default() }],
            char: CharProps { size: Some(size), ..Default::default() },
            para: ParaProps::default(),
            para_style: None,
        }
    }

    pub fn plain(&self) -> String {
        self.runs.iter().map(|r| r.text.as_str()).collect()
    }
}

// ------------------------------------------------------------------ fonts

/// The fonts available to vector type: the system's and Studio's bundled
/// library, found once.
pub fn fonts() -> &'static FontLibrary {
    static LIB: OnceLock<FontLibrary> = OnceLock::new();
    LIB.get_or_init(FontLibrary::discover)
}

/// The face for a family, weight and slant (the closest available).
pub fn resolve(family: &str, weight: u16, italic: bool) -> Option<FontMatch> {
    let lib = fonts();
    let req = FontRequest::new(family, weight, italic);
    lib.resolve(&req).or_else(|| {
        // Any face at all, so text always shows.
        let fallback = ["Helvetica", "Arial", "Inter", "DejaVu Sans", "Roboto", "Noto Sans"];
        fallback.iter().find_map(|f| lib.resolve(&FontRequest::new(f, weight, italic))).or_else(|| {
            let first = lib.families().into_iter().next()?;
            lib.resolve(&FontRequest::new(&first, weight, italic))
        })
    })
}

struct Outliner<'a> {
    path: &'a mut BezPath,
    m: Affine,
}

impl ttf_parser::OutlineBuilder for Outliner<'_> {
    fn move_to(&mut self, x: f32, y: f32) {
        self.path.move_to(self.m * Point::new(x as f64, y as f64));
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.path.line_to(self.m * Point::new(x as f64, y as f64));
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        self.path.quad_to(self.m * Point::new(x1 as f64, y1 as f64), self.m * Point::new(x as f64, y as f64));
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        self.path.curve_to(self.m * Point::new(x1 as f64, y1 as f64), self.m * Point::new(x2 as f64, y2 as f64), self.m * Point::new(x as f64, y as f64));
    }
    fn close(&mut self) {
        self.path.close_path();
    }
}

/// A shaped glyph: its font, id, advance and offset, in document units at
/// the run's size.
#[derive(Clone)]
struct Glyph {
    font: Arc<FontMatch>,
    id: u16,
    advance: f64,
    dx: f64,
    dy: f64,
    /// Units per document unit (size / units per em).
    scale: f64,
    /// Horizontal and vertical scale of the run.
    sxy: (f64, f64),
    baseline_shift: f64,
    color: Option<Color>,
    underline: bool,
    strike: bool,
    /// The glyph is a space (a place lines may break and justify stretches).
    space: bool,
    size: f64,
}

fn features(list: &[String]) -> Vec<rustybuzz::Feature> {
    list.iter().filter_map(|f| f.parse::<rustybuzz::Feature>().ok()).collect()
}

fn apply_case(s: &str, c: Case) -> String {
    match c {
        Case::Upper => s.to_uppercase(),
        Case::Lower => s.to_lowercase(),
        Case::Title => {
            let mut out = String::with_capacity(s.len());
            let mut start = true;
            for ch in s.chars() {
                if start && ch.is_alphabetic() {
                    out.extend(ch.to_uppercase());
                    start = false;
                } else {
                    if ch.is_whitespace() {
                        start = true;
                    }
                    out.push(ch);
                }
            }
            out
        }
        _ => s.to_string(),
    }
}

/// Shapes one paragraph's runs (text without newlines).
fn shape_runs(runs: &[(String, CharProps)]) -> Vec<Glyph> {
    let mut out = Vec::new();
    for (text, props) in runs {
        if text.is_empty() {
            continue;
        }
        let case = props.case.unwrap_or_default();
        let shown = apply_case(text, case);
        let Some(m) = resolve(props.font.as_deref().unwrap_or("Helvetica"), props.weight.unwrap_or(400), props.italic.unwrap_or(false)) else {
            continue;
        };
        let m = Arc::new(m);
        let mut face = m.face.shaper();
        let vars = m.variations();
        if !vars.is_empty() {
            face.set_variations(&vars);
        }
        let mut feats: Vec<String> = props.features.clone().unwrap_or_default();
        if case == Case::SmallCaps {
            feats.push("smcp".into());
        }
        let feats = features(&feats);
        let mut buf = rustybuzz::UnicodeBuffer::new();
        buf.push_str(&shown);
        buf.guess_segment_properties();
        let gb = rustybuzz::shape(&face, &feats, buf);
        let size = props.size();
        let scale = size / m.face.units_per_em as f64;
        let sxy = props.scale.map(|(x, y)| (x / 100.0, y / 100.0)).unwrap_or((1.0, 1.0));
        let tracking = props.tracking.unwrap_or(0.0) / 1000.0 * size;
        for (info, pos) in gb.glyph_infos().iter().zip(gb.glyph_positions()) {
            let cl = info.cluster as usize;
            let ch = shown.get(cl..).and_then(|s| s.chars().next()).unwrap_or(' ');
            out.push(Glyph {
                font: m.clone(),
                id: info.glyph_id as u16,
                advance: pos.x_advance as f64 * scale * sxy.0 + tracking,
                dx: pos.x_offset as f64 * scale * sxy.0,
                dy: pos.y_offset as f64 * scale * sxy.1,
                scale,
                sxy,
                baseline_shift: props.baseline_shift.unwrap_or(0.0),
                color: props.color,
                underline: props.underline.unwrap_or(false),
                strike: props.strikethrough.unwrap_or(false),
                space: ch.is_whitespace(),
                size,
            });
        }
    }
    out
}

/// The outline of one glyph drawn with its baseline origin at `at`,
/// rotated by `angle` radians.
fn glyph_outline(g: &Glyph, at: Point, angle: f64, out: &mut BezPath) {
    let mut face = g.font.face.ttf();
    if let Some(w) = g.font.weight_variation {
        face.set_variation(ttf_parser::Tag::from_bytes(b"wght"), w);
    }
    // Font units are y-up; the document is y-down.
    let mut m = Affine::translate(at.to_vec2()) * Affine::rotate(angle) * Affine::translate((g.dx, -g.dy - g.baseline_shift)) * Affine::scale_non_uniform(g.scale * g.sxy.0, -g.scale * g.sxy.1);
    if g.font.synthetic_italic {
        // A slant of about 11 degrees, in font units (y up).
        m *= Affine::new([1.0, 0.0, 0.2, 1.0, 0.0, 0.0]);
    }
    let mut b = Outliner { path: out, m };
    face.outline_glyph(ttf_parser::GlyphId(g.id), &mut b);
}

/// One placed line: glyphs from `start..end` of the paragraph's glyphs.
struct LineBox {
    origin: Point,
    glyphs: std::ops::Range<usize>,
    justify: f64,
}

/// What the layout made: outlines grouped by colour (`None` = the text
/// object's own fill), and the text's bounds.
#[derive(Clone, Debug, Default)]
pub struct TextOutlines {
    pub parts: Vec<(Option<Color>, BezPath)>,
    /// Lines placed, and characters that did not fit (area type overflow).
    pub lines: usize,
    pub overflow: bool,
}

/// Horizontal ranges a line band `[y0, y1]` leaves free between `x0` and `x1`
/// once the wrap obstacles are taken out.
fn free_spans(x0: f64, x1: f64, y0: f64, y1: f64, obstacles: &[BezPath]) -> Vec<(f64, f64)> {
    let mut blocked: Vec<(f64, f64)> = Vec::new();
    for o in obstacles {
        // The obstacle's horizontal extent within the band (flattened).
        let (mut lo, mut hi) = (f64::MAX, f64::MIN);
        let mut prev: Option<Point> = None;
        let mut first: Option<Point> = None;
        let mut visit = |a: Point, b: Point| {
            let (ya, yb) = (a.y.min(b.y), a.y.max(b.y));
            if yb < y0 || ya > y1 {
                return;
            }
            let clip = |t: f64| a.lerp(b, t.clamp(0.0, 1.0));
            let dy = b.y - a.y;
            let (t0, t1) = if dy.abs() < 1e-12 { (0.0, 1.0) } else { let (u, v) = ((y0 - a.y) / dy, (y1 - a.y) / dy); (u.min(v).max(0.0), u.max(v).min(1.0)) };
            for t in [t0, t1] {
                let p = clip(t);
                lo = lo.min(p.x);
                hi = hi.max(p.x);
            }
        };
        kurbo::flatten(o.iter(), 0.5, |el| match el {
            kurbo::PathEl::MoveTo(p) => {
                if let (Some(a), Some(f)) = (prev, first) {
                    visit(a, f);
                }
                prev = Some(p);
                first = Some(p);
            }
            kurbo::PathEl::LineTo(p) => {
                if let Some(a) = prev {
                    visit(a, p);
                }
                prev = Some(p);
            }
            kurbo::PathEl::ClosePath => {
                if let (Some(a), Some(f)) = (prev, first) {
                    visit(a, f);
                }
                prev = first;
            }
            _ => {}
        });
        if let (Some(a), Some(f)) = (prev, first) {
            visit(a, f);
        }
        if lo < hi {
            blocked.push((lo, hi));
        }
    }
    blocked.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut spans = Vec::new();
    let mut cur = x0;
    for (lo, hi) in blocked {
        if lo > cur {
            spans.push((cur, lo.min(x1)));
        }
        cur = cur.max(hi);
        if cur >= x1 {
            break;
        }
    }
    if cur < x1 {
        spans.push((cur, x1));
    }
    spans.retain(|(a, b)| b - a > 1.0);
    spans
}

/// Lays out `node` (styles already resolved into `runs` and `para`) and
/// returns its glyph outlines. `wrap` are outlines area type flows around.
pub fn layout(node: &TextNode, char_base: &CharProps, para: &ParaProps, run_props: &[CharProps], wrap: &[BezPath]) -> TextOutlines {
    let mut out = TextOutlines::default();
    // Paragraphs: split runs at newlines.
    let mut paras: Vec<Vec<(String, CharProps)>> = vec![Vec::new()];
    for (r, p) in node.runs.iter().zip(run_props) {
        let props = p.over(char_base);
        for (i, piece) in r.text.split('\n').enumerate() {
            if i > 0 {
                paras.push(Vec::new());
            }
            if !piece.is_empty()
                && let Some(last) = paras.last_mut()
            {
                last.push((piece.to_string(), props.clone()));
            }
        }
    }
    let base_size = char_base.size();
    let leading_of = |gs: &[Glyph], props: &[(String, CharProps)]| -> f64 {
        let size = gs.iter().map(|g| g.size).fold(0.0, f64::max).max(if gs.is_empty() { props.first().map(|p| p.1.size()).unwrap_or(base_size) } else { 0.0 });
        props.iter().filter_map(|p| p.1.leading).fold(None, |a: Option<f64>, l| Some(a.map_or(l, |a| a.max(l)))).unwrap_or(size * 1.2)
    };
    let align = para.align.unwrap_or_default();
    let emit = |gs: &[Glyph], line: &LineBox, out: &mut TextOutlines| {
        let mut x = line.origin.x;
        for (k, g) in gs[line.glyphs.clone()].iter().enumerate() {
            let mut p = BezPath::new();
            glyph_outline(g, Point::new(x, line.origin.y), 0.0, &mut p);
            let deco_y = |frac: f64| line.origin.y - g.size * frac;
            if g.underline || g.strike {
                let thick = (g.size * 0.06).max(0.5);
                for (on, frac) in [(g.underline, -0.12), (g.strike, 0.3)] {
                    if on {
                        let y = deco_y(frac);
                        p.extend(Rect::new(x, y - thick / 2.0, x + g.advance, y + thick / 2.0).path_elements(0.1));
                    }
                }
            }
            push_part(out, g.color, p);
            x += g.advance;
            if g.space && k + 1 < line.glyphs.len() {
                x += line.justify;
            }
        }
    };
    match &node.kind {
        TextKind::Point { at } => {
            let mut y = at.y;
            for (pi, runs) in paras.iter().enumerate() {
                let gs = shape_runs(runs);
                let lead = leading_of(&gs, runs);
                if pi > 0 {
                    y += lead + para.space_before.unwrap_or(0.0);
                }
                let width: f64 = gs.iter().map(|g| g.advance).sum();
                let x = match align {
                    Align::Center => at.x - width / 2.0,
                    Align::Right => at.x - width,
                    _ => at.x,
                };
                emit(&gs, &LineBox { origin: Point::new(x, y), glyphs: 0..gs.len(), justify: 0.0 }, &mut out);
                out.lines += 1;
                y += para.space_after.unwrap_or(0.0);
            }
        }
        TextKind::Area { rect, columns, gutter } => {
            let cols = (*columns).clamp(1, 32) as usize;
            let col_w = ((rect.width() - gutter * (cols - 1) as f64) / cols as f64).max(1.0);
            let mut col = 0usize;
            let first_lead = paras.first().map(|r| leading_of(&shape_runs(r), r)).unwrap_or(base_size * 1.2);
            let ascent_guess = first_lead / 1.2 * 0.8;
            let mut y = rect.y0 + ascent_guess;
            'paras: for (pi, runs) in paras.iter().enumerate() {
                let gs = shape_runs(runs);
                let lead = leading_of(&gs, runs);
                if pi > 0 {
                    y += para.space_before.unwrap_or(0.0);
                }
                let mut i = 0usize;
                let mut first_line = true;
                if gs.is_empty() {
                    y += lead;
                    continue;
                }
                while i < gs.len() {
                    if y > rect.y1 {
                        col += 1;
                        if col >= cols {
                            out.overflow = true;
                            break 'paras;
                        }
                        y = rect.y0 + ascent_guess;
                    }
                    let cx0 = rect.x0 + col as f64 * (col_w + gutter);
                    let indent = para.left_indent.unwrap_or(0.0) + if first_line { para.first_indent.unwrap_or(0.0) } else { 0.0 };
                    let (lx0, lx1) = (cx0 + indent, cx0 + col_w - para.right_indent.unwrap_or(0.0));
                    let spans = free_spans(lx0, lx1, y - lead * 0.8, y + lead * 0.2, wrap);
                    if spans.is_empty() {
                        y += lead;
                        continue;
                    }
                    for (sx0, sx1) in spans {
                        if i >= gs.len() {
                            break;
                        }
                        let avail = sx1 - sx0;
                        // Greedy: take glyphs while they fit, then back up to the last space.
                        let mut w = 0.0;
                        let mut j = i;
                        let mut last_space: Option<usize> = None;
                        while j < gs.len() && (w + gs[j].advance <= avail || j == i) {
                            if gs[j].space {
                                last_space = Some(j);
                            }
                            w += gs[j].advance;
                            j += 1;
                        }
                        let mut end = j;
                        if j < gs.len()
                            && let Some(s) = last_space.filter(|s| *s > i)
                        {
                            end = s + 1;
                        }
                        // Trailing spaces neither count for alignment nor justify.
                        let mut vis_end = end;
                        while vis_end > i && gs[vis_end - 1].space {
                            vis_end -= 1;
                        }
                        let width: f64 = gs[i..vis_end].iter().map(|g| g.advance).sum();
                        let last = end >= gs.len();
                        let spaces = gs[i..vis_end].iter().filter(|g| g.space).count();
                        let (x, justify) = match align {
                            Align::Center => (sx0 + (avail - width) / 2.0, 0.0),
                            Align::Right => (sx1 - width, 0.0),
                            Align::Justify if !last && spaces > 0 => (sx0, (avail - width) / spaces as f64),
                            _ => (sx0, 0.0),
                        };
                        emit(&gs, &LineBox { origin: Point::new(x, y), glyphs: i..vis_end, justify }, &mut out);
                        out.lines += 1;
                        i = end;
                    }
                    first_line = false;
                    y += lead;
                }
                y += para.space_after.unwrap_or(0.0);
            }
        }
        TextKind::OnPath { path, offset, flip } => {
            let mut bp = path.to_bezpath();
            if *flip {
                bp = bp.reverse_subpaths();
            }
            let segs: Vec<kurbo::PathSeg> = bp.segments().collect();
            let lens: Vec<f64> = segs.iter().map(|s| s.arclen(1e-3)).collect();
            let total: f64 = lens.iter().sum();
            let gs: Vec<Glyph> = paras.iter().flat_map(|r| shape_runs(r)).collect();
            let width: f64 = gs.iter().map(|g| g.advance).sum();
            let start = match align {
                Align::Center => offset + (total - width) / 2.0,
                Align::Right => total - width - offset,
                _ => *offset,
            };
            let at = |s: f64| -> Option<(Point, f64)> {
                if s < 0.0 || s > total || segs.is_empty() {
                    return None;
                }
                let mut acc = 0.0;
                for (seg, l) in segs.iter().zip(&lens) {
                    if s <= acc + l || std::ptr::eq(seg, segs.last()?) {
                        let t = seg.inv_arclen((s - acc).max(0.0), 1e-4);
                        let d = match seg {
                            kurbo::PathSeg::Line(l) => l.p1 - l.p0,
                            kurbo::PathSeg::Quad(q) => q.deriv().eval(t).to_vec2(),
                            kurbo::PathSeg::Cubic(c) => c.deriv().eval(t).to_vec2(),
                        };
                        return Some((seg.eval(t), d.y.atan2(d.x)));
                    }
                    acc += l;
                }
                None
            };
            let mut x = start;
            for g in &gs {
                // Each glyph stands on the path at its centre, turned to the tangent.
                if let Some((p, ang)) = at(x + g.advance / 2.0) {
                    let back = kurbo::Vec2::new(-g.advance / 2.0 * ang.cos(), -g.advance / 2.0 * ang.sin());
                    let mut bpth = BezPath::new();
                    glyph_outline(g, p + back, ang, &mut bpth);
                    push_part(&mut out, g.color, bpth);
                } else if x + g.advance / 2.0 > total {
                    out.overflow = true;
                }
                x += g.advance;
            }
            out.lines = 1;
        }
    }
    out
}

fn push_part(out: &mut TextOutlines, color: Option<Color>, p: BezPath) {
    if p.elements().is_empty() {
        return;
    }
    if let Some(last) = out.parts.last_mut()
        && last.0 == color
    {
        last.1.extend(p);
        return;
    }
    out.parts.push((color, p));
}

/// The characters a font can draw, for the glyph panel: (character, glyph id).
pub fn glyphs_of(family: &str, limit: usize) -> Vec<(char, u16)> {
    let Some(m) = resolve(family, 400, false) else { return Vec::new() };
    let face = m.face.ttf();
    let mut out = Vec::new();
    if let Some(cmap) = face.tables().cmap {
        for sub in cmap.subtables {
            if !sub.is_unicode() {
                continue;
            }
            sub.codepoints(|cp| {
                if out.len() < limit
                    && let Some(c) = char::from_u32(cp)
                    && let Some(g) = sub.glyph_index(cp)
                    && !c.is_control()
                {
                    out.push((c, g.0));
                }
            });
        }
    }
    out.sort();
    out.dedup();
    out
}

/// OpenType features the font offers (from its GSUB and GPOS tables).
pub fn features_of(family: &str) -> Vec<String> {
    let Some(m) = resolve(family, 400, false) else { return Vec::new() };
    let face = m.face.ttf();
    let mut out: Vec<String> = Vec::new();
    for t in [face.tables().gsub, face.tables().gpos].into_iter().flatten() {
        for f in t.features {
            let tag = f.tag.to_string();
            if !out.contains(&tag) {
                out.push(tag);
            }
        }
    }
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn have_fonts() -> bool {
        resolve("Helvetica", 400, false).is_some()
    }

    #[test]
    fn point_text_has_outlines_and_width() {
        if !have_fonts() {
            eprintln!("no fonts: skipping");
            return;
        }
        let n = TextNode::point(Point::new(10.0, 100.0), "Hello", 48.0);
        let o = layout(&n, &n.char, &n.para, &[CharProps::default()], &[]);
        assert_eq!(o.lines, 1);
        let b = o.parts[0].1.bounding_box();
        assert!(b.width() > 48.0 && b.y1 <= 101.0 && b.y0 > 40.0, "{b:?}");
    }

    #[test]
    fn area_text_wraps_and_flows_around() {
        if !have_fonts() {
            return;
        }
        let text = "The quick brown fox jumps over the lazy dog. ".repeat(6);
        let mut n = TextNode::point(Point::ZERO, &text, 20.0);
        n.kind = TextKind::Area { rect: Rect::new(0.0, 0.0, 300.0, 1000.0), columns: 1, gutter: 0.0 };
        let o = layout(&n, &n.char, &n.para, &[CharProps::default()], &[]);
        assert!(o.lines >= 4, "{}", o.lines);
        let b = o.parts[0].1.bounding_box();
        assert!(b.x1 <= 301.0, "{b:?}");
        // A wrap obstacle on the left pushes the lines it meets to its right.
        let obstacle = Rect::new(0.0, 0.0, 150.0, 60.0).to_path(0.1);
        let o2 = layout(&n, &n.char, &n.para, &[CharProps::default()], &[obstacle]);
        assert!(o2.lines > o.lines);
    }

    #[test]
    fn type_on_a_path_follows_it() {
        if !have_fonts() {
            return;
        }
        let mut n = TextNode::point(Point::ZERO, "ALONG THE ARC", 24.0);
        let arc = crate::shapes::LiveShape::Arc { from: Point::new(0.0, 200.0), to: Point::new(400.0, 0.0), slope: 50.0, closed: false }.to_path();
        n.kind = TextKind::OnPath { path: arc, offset: 10.0, flip: false };
        let o = layout(&n, &n.char, &n.para, &[CharProps::default()], &[]);
        let b = o.parts[0].1.bounding_box();
        assert!(b.height() > 50.0, "glyphs climb with the path: {b:?}");
    }
}
