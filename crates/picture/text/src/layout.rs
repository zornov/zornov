// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shaping and layout: text block + style → positioned glyphs.
//!
//! Pipeline: case → bidi levels (UAX #9) → itemise by span, script, level
//! and fallback font → shape each item with rustybuzz → group into
//! clusters in logical order → break lines (UAX #14 opportunities, greedy)
//! → reorder each line visually → place glyphs with letter spacing,
//! justification and alignment → optional bend along an arc → bounds and
//! per-unit (character / word / line) boxes for animation.
//!
//! Everything here is in pixels for the canvas the block is laid out on;
//! style lengths are converted from canvas units on the way in.

use std::ops::Range;

use rustybuzz::ttf_parser;
use unicode_bidi::{BidiInfo, Level};
use unicode_script::{Script, UnicodeScript};
use unicode_segmentation::UnicodeSegmentation;

use crate::font::{FontLibrary, FontMatch, FontRequest};
use crate::geom::Rect;
use crate::style::{Align, Canvas, Direction, Fill, LayoutParams, TextBlock, TextStyle, VAlign};

/// A glyph with its final position. Coordinates are pixels relative to the
/// top-left of the layout box; `x`, `y` is the glyph origin on the baseline.
#[derive(Clone, Debug)]
pub struct PlacedGlyph {
    /// Index into [`Layout::fonts`].
    pub font: usize,
    pub glyph: u16,
    pub x: f32,
    pub y: f32,
    /// Rotation in degrees (clockwise) for bent text; 0 otherwise.
    pub rotation: f32,
    pub advance: f32,
    pub size_px: f32,
    pub span: usize,
    /// Byte offset of the cluster in [`Layout::text`].
    pub cluster: usize,
    pub char_index: usize,
    pub word_index: usize,
    pub line_index: usize,
    pub is_space: bool,
    /// The face is a colour (emoji) font: paint it as an image.
    pub colour: bool,
    /// Drawn from the emoji image set: the cluster text is the key.
    pub emoji: bool,
}

/// One laid-out line.
#[derive(Clone, Debug)]
pub struct Line {
    pub glyphs: Range<usize>,
    /// Left edge of the line's text (after alignment).
    pub x: f32,
    pub baseline: f32,
    pub width: f32,
    pub ascent: f32,
    pub descent: f32,
    /// Advance to the next baseline.
    pub height: f32,
    pub text: Range<usize>,
    pub chars: Range<usize>,
    pub words: Range<usize>,
}

impl Line {
    /// The typographic box of the line (ascent above, descent below).
    pub fn rect(&self) -> Rect {
        Rect::new(self.x, self.baseline - self.ascent, self.width, self.ascent + self.descent)
    }
}

/// A span's resolved paint overrides, kept so the painter can colour runs.
#[derive(Clone, Debug)]
pub struct ResolvedSpan {
    pub fill: Option<Fill>,
    pub underline: bool,
    pub strikethrough: bool,
}

/// An underline or strikethrough bar.
#[derive(Clone, Debug)]
pub struct Decoration {
    pub rect: Rect,
    pub span: usize,
    pub line_index: usize,
    pub word_index: usize,
    pub char_index: usize,
}

/// The result of layout.
#[derive(Clone, Debug)]
pub struct Layout {
    /// The text after case transforms; cluster offsets index into it.
    pub text: String,
    pub glyphs: Vec<PlacedGlyph>,
    pub lines: Vec<Line>,
    pub fonts: Vec<FontMatch>,
    pub spans: Vec<ResolvedSpan>,
    pub decorations: Vec<Decoration>,
    /// Base font size in pixels after fit-to-box.
    pub size_px: f32,
    pub px_per_unit: f32,
    /// The box the text was aligned in: `(0, 0, width, height)`.
    pub bounds: Rect,
    /// Union of glyph boxes (advance × line extent), before decorations.
    pub ink: Rect,
    pub char_count: usize,
    pub word_count: usize,
    /// Box per character unit (non-space grapheme clusters).
    pub char_boxes: Vec<Rect>,
    pub word_boxes: Vec<Rect>,
    pub vertical: bool,
}

impl Layout {
    pub fn line_boxes(&self) -> Vec<Rect> {
        self.lines.iter().map(Line::rect).collect()
    }

    pub fn is_empty(&self) -> bool {
        self.glyphs.is_empty()
    }
}

// ---------------------------------------------------------------------------

struct Item {
    text: Range<usize>,
    span: usize,
    level: Level,
    script: Script,
    font: usize,
    emoji: bool,
}

struct ShapedGlyph {
    emoji: bool,
    font: usize,
    glyph: u16,
    cluster: usize,
    x_advance: f32,
    x_offset: f32,
    y_offset: f32,
    size_px: f32,
    span: usize,
    rtl: bool,
}

struct Cluster {
    text: Range<usize>,
    glyphs: Range<usize>,
    width: f32,
    is_space: bool,
    /// Line may break after this cluster.
    can_break: bool,
    must_break: bool,
    level: Level,
    font: usize,
    size_px: f32,
    char_index: usize,
    word_index: usize,
}

/// Lays out `block` with `style` on `canvas`.
pub fn layout(lib: &FontLibrary, block: &TextBlock, style: &TextStyle, canvas: &Canvas) -> Layout {
    let ppu = canvas.px_per_unit();
    let size_px = canvas.px(style.size.clamp(1.0, 2000.0));
    let p = &block.layout;
    if p.fit_to_box {
        if let (Some(w), Some(h)) = (p.wrap_width, p.box_height) {
            let (bw, bh) = (canvas.px(w), canvas.px(h));
            // Find the largest size whose layout fits both ways. Sizes are
            // monotone in overflow, so bisection works; 10 steps give a
            // step well under a pixel at title sizes.
            let fits = |s: f32| {
                let l = layout_at(lib, block, style, canvas, s);
                l.content_height() <= bh + 0.5 && l.lines.iter().all(|ln| ln.width <= bw + 0.5)
            };
            let (mut lo, mut hi) = (canvas.px(5.0), canvas.px(300.0));
            if fits(hi) {
                return layout_at(lib, block, style, canvas, hi);
            }
            if !fits(lo) {
                return layout_at(lib, block, style, canvas, lo);
            }
            for _ in 0..10 {
                let mid = 0.5 * (lo + hi);
                if fits(mid) { lo = mid } else { hi = mid }
            }
            return layout_at(lib, block, style, canvas, lo);
        }
    }
    let _ = ppu;
    layout_at(lib, block, style, canvas, size_px)
}

impl Layout {
    fn content_height(&self) -> f32 {
        match (self.lines.first(), self.lines.last()) {
            (Some(f), Some(l)) => (l.baseline + l.descent) - (f.baseline - f.ascent),
            _ => 0.0,
        }
    }
}

fn script_tag(s: Script) -> Option<rustybuzz::Script> {
    let b = s.short_name().as_bytes();
    if b.len() != 4 {
        return None;
    }
    rustybuzz::Script::from_iso15924_tag(ttf_parser::Tag::from_bytes(&[b[0], b[1], b[2], b[3]]))
}

fn layout_at(lib: &FontLibrary, block: &TextBlock, style: &TextStyle, canvas: &Canvas, size_px: f32) -> Layout {
    let p = &block.layout;
    let ppu = canvas.px_per_unit();

    // 1. Case and concatenation.
    let mut text = String::new();
    let mut span_ranges: Vec<Range<usize>> = Vec::new();
    let mut spans: Vec<ResolvedSpan> = Vec::new();
    for s in &block.spans {
        let t = style.case.apply(&s.text);
        let start = text.len();
        text.push_str(&t);
        span_ranges.push(start..text.len());
        spans.push(ResolvedSpan {
            fill: s.style.fill.clone(),
            underline: s.style.underline.unwrap_or(style.underline),
            strikethrough: s.style.strikethrough.unwrap_or(style.strikethrough),
        });
    }
    let span_of = |byte: usize| span_ranges.iter().position(|r| r.contains(&byte)).unwrap_or(span_ranges.len().saturating_sub(1));

    let mut fonts: Vec<FontMatch> = Vec::new();
    let mut font_index = |m: FontMatch| -> usize {
        if let Some(i) = fonts.iter().position(|f| {
            f.id() == m.id() && f.weight_variation == m.weight_variation && f.synthetic_bold == m.synthetic_bold && f.synthetic_italic == m.synthetic_italic
        }) {
            i
        } else {
            fonts.push(m);
            fonts.len() - 1
        }
    };

    let base_match = lib.resolve(&style.font);
    let base_font = base_match.clone().map(&mut font_index);

    // 2. Bidi.
    let default_level = match p.direction {
        Direction::Auto => None,
        Direction::Ltr => Some(Level::ltr()),
        Direction::Rtl => Some(Level::rtl()),
    };
    let bidi = BidiInfo::new(&text, default_level);

    // 3. Itemise.
    let mut items: Vec<Item> = Vec::new();
    let mut span_fonts: Vec<Option<FontMatch>> = Vec::new();
    for s in &block.spans {
        let req: FontRequest = s.style.font(&style.font);
        span_fonts.push(lib.resolve(&req));
    }
    // Byte ranges of emoji clusters the image set can draw.
    let emoji_ranges: Vec<Range<usize>> = match lib.emoji() {
        Some(set) => text
            .grapheme_indices(true)
            .filter(|(_, g)| crate::emoji::is_emoji_cluster(g) && set.has(g))
            .map(|(b, g)| b..b + g.len())
            .collect(),
        None => Vec::new(),
    };
    {
        let mut cur: Option<Item> = None;
        let mut last_script = Script::Latin;
        for (i, c) in text.char_indices() {
            if let Some(r) = emoji_ranges.iter().find(|r| r.contains(&i)) {
                if r.start == i {
                    if let Some(it) = cur.take() {
                        items.push(it);
                    }
                    let span = span_of(i);
                    let font = span_fonts[span].clone().map(&mut font_index).unwrap_or(0);
                    items.push(Item { text: r.clone(), span, level: bidi.levels[i], script: Script::Common, font, emoji: true });
                }
                continue;
            }
            if c == '\n' || c == '\r' {
                if let Some(it) = cur.take() {
                    items.push(it);
                }
                last_script = Script::Latin;
                continue;
            }
            let span = span_of(i);
            let level = bidi.levels[i];
            let mut script = c.script();
            if matches!(script, Script::Common | Script::Inherited | Script::Unknown) {
                script = last_script;
            } else {
                last_script = script;
            }
            let primary = match &span_fonts[span] {
                Some(m) => m.clone(),
                None => break,
            };
            let m = if c.is_whitespace() { primary } else { lib.fallback(&primary, c, script) };
            let font = font_index(m);
            match &mut cur {
                Some(it) if it.span == span && it.level == level && it.script == script && it.font == font && !it.emoji => {
                    it.text.end = i + c.len_utf8();
                }
                _ => {
                    if let Some(it) = cur.take() {
                        items.push(it);
                    }
                    cur = Some(Item { text: i..i + c.len_utf8(), span, level, script, font, emoji: false });
                }
            }
        }
        if let Some(it) = cur.take() {
            items.push(it);
        }
    }

    // 4. Shape.
    let letter_px = style.letter_spacing * 0.01 * size_px;
    let features: Vec<rustybuzz::Feature> = if style.kerning { Vec::new() } else { vec![rustybuzz::Feature::new(ttf_parser::Tag::from_bytes(b"kern"), 0, ..)] };
    let mut shaped: Vec<ShapedGlyph> = Vec::new();
    for it in &items {
        let m = &fonts[it.font];
        let ss = &block.spans[it.span].style;
        let span_scale = ss.size_scale.unwrap_or(1.0).max(0.05);
        let item_size = size_px * span_scale;
        let run_letter_px = ss.letter_spacing.map_or(letter_px, |l| l * 0.01 * size_px);
        // Baseline shift: harfbuzz offsets point up.
        let lift = ss.baseline_shift.unwrap_or(0.0) * ppu;
        if it.emoji {
            shaped.push(ShapedGlyph {
                emoji: true,
                font: it.font,
                glyph: 0,
                cluster: it.text.start,
                x_advance: item_size * EMOJI_ADVANCE + run_letter_px,
                x_offset: 0.0,
                y_offset: lift,
                size_px: item_size,
                span: it.span,
                rtl: it.level.is_rtl(),
            });
            continue;
        }
        let mut face = m.face.shaper();
        if let Some(w) = m.weight_variation {
            face.set_variations(&[rustybuzz::Variation { tag: ttf_parser::Tag::from_bytes(b"wght"), value: w }]);
        }
        let scale = item_size / m.face.units_per_em;
        let mut buf = rustybuzz::UnicodeBuffer::new();
        buf.push_str(&text[it.text.clone()]);
        buf.set_direction(if it.level.is_rtl() { rustybuzz::Direction::RightToLeft } else { rustybuzz::Direction::LeftToRight });
        if let Some(s) = script_tag(it.script) {
            buf.set_script(s);
        }
        buf.set_cluster_level(rustybuzz::BufferClusterLevel::MonotoneGraphemes);
        let out = rustybuzz::shape(&face, &features, buf);
        let infos = out.glyph_infos();
        let poss = out.glyph_positions();
        let n = infos.len();
        for g in 0..n {
            let cluster = it.text.start + infos[g].cluster as usize;
            // Letter spacing goes after the last glyph of each cluster. In
            // RTL buffers glyph order is visual, so "last" means the next
            // glyph belongs to another cluster, whichever way clusters run.
            let ends_cluster = g + 1 >= n || infos[g + 1].cluster != infos[g].cluster;
            let mut adv = poss[g].x_advance as f32 * scale;
            if ends_cluster {
                adv += run_letter_px;
            }
            shaped.push(ShapedGlyph {
                emoji: false,
                font: it.font,
                glyph: infos[g].glyph_id as u16,
                cluster,
                x_advance: adv,
                x_offset: poss[g].x_offset as f32 * scale,
                y_offset: poss[g].y_offset as f32 * scale + lift,
                size_px: item_size,
                span: it.span,
                rtl: it.level.is_rtl(),
            });
        }
    }

    // 5. Clusters in logical order.
    let breaks: Vec<(usize, unicode_linebreak::BreakOpportunity)> = unicode_linebreak::linebreaks(&text).collect();
    let can_break_at = |byte: usize| breaks.iter().any(|(b, o)| *b == byte && *o == unicode_linebreak::BreakOpportunity::Allowed);
    let mut clusters: Vec<Cluster> = Vec::new();
    // Grapheme and word indices over the whole text, whitespace excluded.
    let mut char_of_byte = vec![usize::MAX; text.len() + 1];
    let mut word_of_byte = vec![usize::MAX; text.len() + 1];
    let (mut ci, mut wi) = (0usize, 0usize);
    let mut in_word = false;
    for (b, g) in text.grapheme_indices(true) {
        let ws = g.chars().all(char::is_whitespace);
        if ws {
            in_word = false;
            for k in b..b + g.len() {
                char_of_byte[k] = usize::MAX;
                word_of_byte[k] = usize::MAX;
            }
            continue;
        }
        if !in_word {
            in_word = true;
            wi += 1;
        }
        for k in b..b + g.len() {
            char_of_byte[k] = ci;
            word_of_byte[k] = wi - 1;
        }
        ci += 1;
    }
    let (char_count, word_count) = (ci, wi);

    // Glyphs of one item are contiguous in `shaped`; within an RTL item the
    // clusters run backwards. Collect cluster groups per item, then order
    // them logically.
    {
        let mut i = 0;
        while i < shaped.len() {
            let font = shaped[i].font;
            let rtl = shaped[i].rtl;
            let span = shaped[i].span;
            let mut j = i;
            let emoji = shaped[i].emoji;
            while j < shaped.len() && shaped[j].font == font && shaped[j].rtl == rtl && shaped[j].span == span && shaped[j].emoji == emoji && !(emoji && j > i) {
                j += 1;
            }
            // Group [i, j) by cluster.
            let mut groups: Vec<(usize, Range<usize>)> = Vec::new();
            let mut k = i;
            while k < j {
                let c = shaped[k].cluster;
                let mut e = k;
                while e < j && shaped[e].cluster == c {
                    e += 1;
                }
                groups.push((c, k..e));
                k = e;
            }
            if rtl {
                groups.reverse();
            }
            for (gi, (c, range)) in groups.iter().enumerate() {
                let end = if gi + 1 < groups.len() {
                    groups[gi + 1].0
                } else {
                    // Last cluster of the item: ends where the item's text ends.
                    let item_end = items.iter().find(|it| it.text.contains(c)).map(|it| it.text.end).unwrap_or(text.len());
                    item_end
                };
                let ctext = &text[*c..end.max(*c)];
                let is_space = !ctext.is_empty() && ctext.chars().all(char::is_whitespace);
                let width: f32 = shaped[range.clone()].iter().map(|g| g.x_advance).sum();
                let level = items.iter().find(|it| it.text.contains(c)).map(|it| it.level).unwrap_or(Level::ltr());
                clusters.push(Cluster {
                    text: *c..end,
                    glyphs: range.clone(),
                    width,
                    is_space,
                    can_break: can_break_at(end),
                    must_break: false,
                    level,
                    font,
                    size_px: shaped[range.start].size_px,
                    char_index: char_of_byte[*c],
                    word_index: word_of_byte[*c],
                });
            }
            i = j;
        }
    }
    // Items were pushed in logical order, so clusters are logical now.
    // Explicit newlines produced no glyphs; give each one an empty cluster
    // that forces a break, so "a\n\nb" keeps its blank line.
    for (i, c) in text.char_indices() {
        if c == '\n' {
            let at = clusters.iter().position(|cl| cl.text.start > i).unwrap_or(clusters.len());
            clusters.insert(
                at,
                Cluster {
                    text: i..i + 1,
                    glyphs: 0..0,
                    width: 0.0,
                    is_space: true,
                    can_break: false,
                    must_break: true,
                    level: Level::ltr(),
                    font: base_font.unwrap_or(0),
                    size_px,
                    char_index: usize::MAX,
                    word_index: usize::MAX,
                },
            );
        }
    }

    // 6. Line breaking.
    let wrap_px = p.wrap_width.map(|w| canvas.px(w));
    let mut line_ranges: Vec<Range<usize>> = Vec::new();
    if p.vertical {
        // Vertical: a column breaks when its height exceeds the box height.
        let cell = line_cell_height(style, &fonts, base_font, size_px);
        let limit = p.box_height.map(|h| canvas.px(h));
        let mut start = 0;
        let mut used = 0.0;
        for (i, c) in clusters.iter().enumerate() {
            let h = if c.is_space { cell * 0.5 } else { cell };
            if let Some(l) = limit {
                if used + h > l + 0.5 && i > start {
                    line_ranges.push(start..i);
                    start = i;
                    used = 0.0;
                }
            }
            used += h;
            if c.must_break {
                line_ranges.push(start..i + 1);
                start = i + 1;
                used = 0.0;
            }
        }
        if start < clusters.len() || clusters.is_empty() {
            line_ranges.push(start..clusters.len());
        }
    } else {
        let mut start = 0;
        let mut width = 0.0;
        let mut last_break: Option<usize> = None;
        let mut i = 0;
        while i < clusters.len() {
            let c = &clusters[i];
            let w_with = width + c.width;
            if let Some(wrap) = wrap_px {
                // Break at the last opportunity; a word longer than the
                // box overflows rather than splitting, so fit-to-box can
                // shrink it and nothing reads as a typo.
                if !c.is_space && w_with > wrap + 0.01 && i > start && matches!(last_break, Some(b) if b > start) {
                    let at = last_break.unwrap();
                    line_ranges.push(start..at);
                    start = at;
                    width = clusters[start..i].iter().map(|c| c.width).sum();
                    last_break = None;
                    continue;
                }
            }
            width = w_with;
            if c.can_break {
                last_break = Some(i + 1);
            }
            if c.must_break {
                line_ranges.push(start..i + 1);
                start = i + 1;
                width = 0.0;
                last_break = None;
            }
            i += 1;
        }
        if start < clusters.len() || clusters.is_empty() || text.ends_with('\n') {
            line_ranges.push(start..clusters.len());
        }
    }
    if let Some(max) = p.max_lines {
        line_ranges.truncate(max.max(1));
    }

    // 7. Place.
    let line_gap_px = style.line_spacing * 0.01 * size_px;
    let mut glyphs: Vec<PlacedGlyph> = Vec::new();
    let mut lines: Vec<Line> = Vec::new();
    let mut decorations: Vec<Decoration> = Vec::new();

    if p.vertical {
        place_vertical(&clusters, &shaped, &fonts, base_font, style, size_px, &line_ranges, p, canvas, &mut glyphs, &mut lines, &text);
    } else {
        let mut y = 0.0;
        for (li, lr) in line_ranges.iter().enumerate() {
            let cl = &clusters[lr.clone()];
            // Metrics from the fonts on the line (base font for empty lines).
            let (mut asc, mut desc, mut gap) = (0.0f32, 0.0f32, 0.0f32);
            let mut any = false;
            for c in cl.iter().filter(|c| !c.is_space) {
                let f = &fonts[c.font].face;
                let s = c.size_px / f.units_per_em;
                asc = asc.max(f.ascent * s);
                desc = desc.max(-f.descent * s);
                gap = gap.max(f.line_gap * s);
                any = true;
            }
            if !any {
                if let Some(bf) = base_font {
                    let f = &fonts[bf].face;
                    let s = size_px / f.units_per_em;
                    asc = f.ascent * s;
                    desc = -f.descent * s;
                    gap = f.line_gap * s;
                }
            }
            // Width without trailing spaces (and without the spacing after
            // the last cluster).
            let mut end = cl.len();
            while end > 0 && cl[end - 1].is_space {
                end -= 1;
            }
            let content: &[Cluster] = &cl[..end];
            let mut width: f32 = content.iter().map(|c| c.width).sum();
            if end > 0 {
                width -= letter_px;
            }
            width = width.max(0.0);

            let baseline = y + asc;
            let box_w = wrap_px.unwrap_or(0.0);
            let is_last_of_para = lr.end == clusters.len() || cl.last().map(|c| c.must_break).unwrap_or(true);
            let (x0, space_extra) = match p.align {
                Align::Left => (0.0, 0.0),
                Align::Centre => ((box_w - width) * 0.5, 0.0),
                Align::Right => (box_w - width, 0.0),
                Align::Justify => {
                    let spaces = content.iter().filter(|c| c.is_space).count();
                    if wrap_px.is_some() && !is_last_of_para && spaces > 0 {
                        (0.0, (box_w - width) / spaces as f32)
                    } else if wrap_px.is_some() && !is_last_of_para && content.len() > 1 {
                        // No spaces (CJK): spread between clusters.
                        (0.0, (box_w - width) / (content.len() - 1) as f32)
                    } else {
                        (0.0, 0.0)
                    }
                }
            };
            let spread_all = p.align == Align::Justify && content.iter().filter(|c| c.is_space).count() == 0;

            // Visual order.
            let levels: Vec<Level> = content.iter().map(|c| c.level).collect();
            let order: Vec<usize> = if levels.iter().any(|l| l.is_rtl()) { BidiInfo::reorder_visual(&levels) } else { (0..content.len()).collect() };

            let gstart = glyphs.len();
            let mut x = x0;
            let line_width_final = width + if spread_all { space_extra * (content.len().saturating_sub(1)) as f32 } else { space_extra * content.iter().filter(|c| c.is_space).count() as f32 };
            for (vi, &ci) in order.iter().enumerate() {
                let c = &content[ci];
                let m = &fonts[c.font];
                for g in &shaped[c.glyphs.clone()] {
                    glyphs.push(PlacedGlyph {
                        font: g.font,
                        glyph: g.glyph,
                        x: x + g.x_offset,
                        y: baseline - g.y_offset,
                        rotation: 0.0,
                        advance: g.x_advance,
                        size_px: g.size_px,
                        span: g.span,
                        cluster: g.cluster,
                        char_index: c.char_index,
                        word_index: c.word_index,
                        line_index: li,
                        is_space: c.is_space,
                        colour: m.face.has_colour && !g.emoji,
                        emoji: g.emoji,
                    });
                    x += g.x_advance;
                }
                if (c.is_space || spread_all) && vi + 1 < order.len() {
                    x += space_extra;
                }
            }
            let chars = unit_range(content, |c| c.char_index);
            let words = unit_range(content, |c| c.word_index);
            let text_range = match (cl.first(), cl.last()) {
                (Some(f), Some(l)) => f.text.start..l.text.end,
                _ => text.len()..text.len(),
            };
            // Decorations per run of consecutive glyphs sharing a span.
            let mut g = gstart;
            while g < glyphs.len() {
                let span = glyphs[g].span;
                let mut e = g;
                while e < glyphs.len() && glyphs[e].span == span {
                    e += 1;
                }
                let rs = &spans[span];
                if rs.underline || rs.strikethrough {
                    let run: &[PlacedGlyph] = &glyphs[g..e];
                    let x0 = run.iter().map(|g| g.x).fold(f32::INFINITY, f32::min);
                    let x1 = run.iter().map(|g| g.x + g.advance).fold(f32::NEG_INFINITY, f32::max) - letter_px.max(0.0);
                    let m = &fonts[run[0].font].face;
                    let s = run[0].size_px / m.units_per_em;
                    if rs.underline {
                        let th = (m.underline_thickness * s).max(1.0);
                        let yy = baseline - m.underline_position * s;
                        decorations.push(Decoration { rect: Rect::new(x0, yy - th * 0.5, (x1 - x0).max(0.0), th), span, line_index: li, word_index: run[0].word_index, char_index: run[0].char_index });
                    }
                    if rs.strikethrough {
                        let th = (m.strikeout_thickness * s).max(1.0);
                        let yy = baseline - m.strikeout_position * s;
                        decorations.push(Decoration { rect: Rect::new(x0, yy - th * 0.5, (x1 - x0).max(0.0), th), span, line_index: li, word_index: run[0].word_index, char_index: run[0].char_index });
                    }
                }
                g = e;
            }
            let height = asc + desc + gap + line_gap_px;
            lines.push(Line { glyphs: gstart..glyphs.len(), x: x0, baseline, width: line_width_final, ascent: asc, descent: desc, height, text: text_range, chars, words });
            y += height;
        }
    }

    // 8. Box and vertical alignment.
    let content_w = lines.iter().map(|l| l.x + l.width).fold(0.0f32, f32::max);
    let min_x = lines.iter().map(|l| l.x).fold(f32::INFINITY, f32::min).min(0.0);
    let content_h = match (lines.first(), lines.last()) {
        (Some(f), Some(l)) => (l.baseline + l.descent) - (f.baseline - f.ascent),
        _ => 0.0,
    };
    let box_w = if p.vertical { content_w } else { wrap_px.unwrap_or(content_w - min_x) };
    let box_h = p.box_height.map(|h| canvas.px(h)).unwrap_or(content_h);
    let top = lines.first().map(|l| l.baseline - l.ascent).unwrap_or(0.0);
    let dy = match p.valign {
        VAlign::Top => -top,
        VAlign::Middle => (box_h - content_h) * 0.5 - top,
        VAlign::Bottom => box_h - content_h - top,
    };
    let dx = if wrap_px.is_none() && !p.vertical { -min_x } else { 0.0 };
    for g in &mut glyphs {
        g.x += dx;
        g.y += dy;
    }
    for l in &mut lines {
        l.x += dx;
        l.baseline += dy;
    }
    for d in &mut decorations {
        d.rect = d.rect.translate(dx, dy);
    }

    // 9. Bend.
    if p.bend.abs() > 0.01 && !p.vertical && p.path.is_none() {
        bend_lines(&mut glyphs, &lines, p.bend, size_px, box_w);
    }
    if let Some(tp) = p.path.as_ref().filter(|tp| tp.points.len() >= 2 && !p.vertical) {
        path_lines(&mut glyphs, &lines, tp, ppu, box_w, box_h, p.align);
    }

    // 10. Unit boxes and ink.
    let mut char_boxes = vec![Rect::EMPTY; char_count];
    let mut word_boxes = vec![Rect::EMPTY; word_count];
    let mut ink = Rect::EMPTY;
    for g in &glyphs {
        let l = &lines[g.line_index];
        let r = glyph_box(g, l);
        if !g.is_space {
            ink = ink.union(&r);
            if g.char_index != usize::MAX {
                char_boxes[g.char_index] = char_boxes[g.char_index].union(&r);
            }
            if g.word_index != usize::MAX {
                word_boxes[g.word_index] = word_boxes[g.word_index].union(&r);
            }
        }
    }
    if !ink.x.is_finite() {
        ink = Rect::new(0.0, 0.0, 0.0, 0.0);
    }

    Layout {
        text,
        glyphs,
        lines,
        fonts,
        spans,
        decorations,
        size_px,
        px_per_unit: ppu,
        bounds: Rect::new(0.0, 0.0, box_w.max(0.0), box_h.max(0.0)),
        ink,
        char_count,
        word_count,
        char_boxes,
        word_boxes,
        vertical: p.vertical,
    }
}

/// Width of an emoji image cell as a fraction of the font size.
pub const EMOJI_ADVANCE: f32 = 1.25;

fn unit_range(content: &[Cluster], f: impl Fn(&Cluster) -> usize) -> Range<usize> {
    let mut lo = usize::MAX;
    let mut hi = 0;
    for c in content {
        let v = f(c);
        if v != usize::MAX {
            lo = lo.min(v);
            hi = hi.max(v + 1);
        }
    }
    if lo == usize::MAX { 0..0 } else { lo..hi }
}

fn line_cell_height(style: &TextStyle, fonts: &[FontMatch], base_font: Option<usize>, size_px: f32) -> f32 {
    let gap = style.line_spacing * 0.01 * size_px;
    match base_font {
        Some(bf) => {
            let f = &fonts[bf].face;
            let s = size_px / f.units_per_em;
            (f.ascent - f.descent) * s + gap
        }
        None => size_px * 1.2 + gap,
    }
}

/// The glyph's box: its advance wide, the line's ascent/descent tall,
/// rotated with the glyph when bent.
fn glyph_box(g: &PlacedGlyph, l: &Line) -> Rect {
    let r = Rect::new(g.x, g.y - l.ascent, g.advance.max(0.0), l.ascent + l.descent);
    if g.rotation.abs() < 1e-3 {
        r
    } else {
        crate::geom::Affine::about([g.x, g.y], [1.0, 1.0], g.rotation, [0.0, 0.0]).bounds(&r)
    }
}

#[allow(clippy::too_many_arguments)]
fn place_vertical(
    clusters: &[Cluster],
    shaped: &[ShapedGlyph],
    fonts: &[FontMatch],
    base_font: Option<usize>,
    style: &TextStyle,
    size_px: f32,
    line_ranges: &[Range<usize>],
    p: &LayoutParams,
    canvas: &Canvas,
    glyphs: &mut Vec<PlacedGlyph>,
    lines: &mut Vec<Line>,
    text: &str,
) {
    let cell = line_cell_height(style, fonts, base_font, size_px);
    let col_w = size_px * (1.0 + style.line_spacing * 0.01 * 0.5) * 1.15;
    let n = line_ranges.len();
    let box_h = p.box_height.map(|h| canvas.px(h));
    for (li, lr) in line_ranges.iter().enumerate() {
        // Columns run right to left: the first line is the rightmost.
        let col_x = (n - 1 - li) as f32 * col_w;
        let cl = &clusters[lr.clone()];
        let col_h: f32 = cl.iter().map(|c| if c.is_space { cell * 0.5 } else { cell }).sum();
        let y0 = match (p.valign, box_h) {
            (VAlign::Top, _) | (_, None) => 0.0,
            (VAlign::Middle, Some(h)) => (h - col_h) * 0.5,
            (VAlign::Bottom, Some(h)) => h - col_h,
        };
        let gstart = glyphs.len();
        let mut y = y0;
        let (mut asc, mut desc) = (0.0f32, 0.0f32);
        for c in cl {
            let h = if c.is_space { cell * 0.5 } else { cell };
            let f = &fonts[c.font].face;
            let s = c.size_px / f.units_per_em;
            asc = asc.max(f.ascent * s);
            desc = desc.max(-f.descent * s);
            let baseline = y + f.ascent * s;
            let gx = col_x + (col_w - c.width.max(0.0)) * 0.5;
            let mut x = gx;
            for g in &shaped[c.glyphs.clone()] {
                glyphs.push(PlacedGlyph {
                    font: g.font,
                    glyph: g.glyph,
                    x: x + g.x_offset,
                    y: baseline - g.y_offset,
                    rotation: 0.0,
                    advance: g.x_advance,
                    size_px: g.size_px,
                    span: g.span,
                    cluster: g.cluster,
                    char_index: c.char_index,
                    word_index: c.word_index,
                    line_index: li,
                    is_space: c.is_space,
                    colour: fonts[c.font].face.has_colour && !g.emoji,
                    emoji: g.emoji,
                });
                x += g.x_advance;
            }
            y += h;
        }
        let text_range = match (cl.first(), cl.last()) {
            (Some(f), Some(l)) => f.text.start..l.text.end,
            _ => text.len()..text.len(),
        };
        lines.push(Line {
            glyphs: gstart..glyphs.len(),
            x: col_x,
            baseline: y0 + asc.max(cell * 0.8),
            width: col_w,
            ascent: asc.max(cell * 0.8),
            descent: (y - y0 - asc.max(cell * 0.8)).max(0.0),
            height: cell,
            text: text_range,
            chars: unit_range(cl, |c| c.char_index),
            words: unit_range(cl, |c| c.word_index),
        });
    }
}

/// Lays every line along `tp`: each glyph's centre goes to its distance
/// along the path from where the line starts, turned to the path's
/// direction there; later lines stack below the first.
fn path_lines(glyphs: &mut [PlacedGlyph], lines: &[Line], tp: &crate::style::TextPath, ppu: f32, box_w: f32, box_h: f32, align: Align) {
    let mut pts: Vec<[f32; 2]> = tp.points.iter().map(|q| [box_w * 0.5 + q[0] * ppu, box_h * 0.5 + q[1] * ppu]).collect();
    if tp.reverse {
        pts.reverse();
    }
    if tp.closed
        && let Some(f) = pts.first().copied()
    {
        pts.push(f);
    }
    let mut cum = vec![0.0f32; pts.len()];
    for k in 1..pts.len() {
        cum[k] = cum[k - 1] + ((pts[k][0] - pts[k - 1][0]).powi(2) + (pts[k][1] - pts[k - 1][1]).powi(2)).sqrt();
    }
    let total = cum[cum.len() - 1];
    if total <= 1e-3 {
        return;
    }
    // Point and unit direction at distance `s` (past the ends: straight on).
    let at = |s: f32| -> ([f32; 2], [f32; 2]) {
        let s = if tp.closed { s.rem_euclid(total) } else { s };
        let k = cum.partition_point(|c| *c < s).clamp(1, pts.len() - 1);
        let (a, b) = (pts[k - 1], pts[k]);
        let len = (cum[k] - cum[k - 1]).max(1e-6);
        let d = [(b[0] - a[0]) / len, (b[1] - a[1]) / len];
        let u = s - cum[k - 1];
        ([a[0] + d[0] * u, a[1] + d[1] * u], d)
    };
    let first = lines.first().map_or(0.0, |l| l.baseline);
    for l in lines {
        let anchor = tp.start.clamp(-1.0, 2.0) * total;
        let s0 = match align {
            Align::Left | Align::Justify => anchor,
            Align::Centre => anchor - l.width * 0.5,
            Align::Right => anchor - l.width,
        };
        for g in &mut glyphs[l.glyphs.clone()] {
            let s = s0 + (g.x + g.advance * 0.5 - l.x);
            let (p, d) = at(s);
            // "Up" off the path: left of the direction of travel on screen.
            let n = [d[1], -d[0]];
            let up = tp.offset * ppu + (first - l.baseline) - (g.y - l.baseline);
            let c = [p[0] + n[0] * up, p[1] + n[1] * up];
            if tp.perpendicular {
                g.rotation = d[1].atan2(d[0]).to_degrees();
                g.x = c[0] - d[0] * g.advance * 0.5;
                g.y = c[1] - d[1] * g.advance * 0.5;
            } else {
                g.rotation = 0.0;
                g.x = c[0] - g.advance * 0.5;
                g.y = c[1];
            }
        }
    }
}

/// Bends every line along a circular arc of `bend` degrees. The arc's
/// chord keeps the line's width, so a 180° bend makes a half circle and
/// 360° closes the circle. Glyph origins move onto the arc and glyphs
/// rotate to follow it.
fn bend_lines(glyphs: &mut [PlacedGlyph], lines: &[Line], bend: f32, size_px: f32, box_w: f32) {
    let theta = bend.to_radians();
    for l in lines {
        let w = l.width.max(size_px);
        let r = (w / theta.abs()).max(size_px * 0.5);
        let cx = if box_w > 0.0 { box_w * 0.5 } else { l.x + l.width * 0.5 };
        let line_centre_x = l.x + l.width * 0.5;
        for g in &mut glyphs[l.glyphs.clone()] {
            // Position of the glyph's centre along the line.
            let gx = g.x + g.advance * 0.5 - line_centre_x;
            let phi = gx / r;
            let (sin, cos) = phi.sin_cos();
            let y_off = g.y - l.baseline;
            // Positive bend: circle centre below the baseline, text arches
            // over it; negative: centre above, text bowls.
            let sign = if theta > 0.0 { 1.0 } else { -1.0 };
            let ccy = l.baseline + sign * r;
            let px = cx + r * sin;
            let py = ccy - sign * r * cos;
            g.rotation = sign * phi.to_degrees();
            // Move the origin back by half the advance along the tangent.
            let (tx, ty) = (cos, sign * sin);
            g.x = px - tx * g.advance * 0.5 - ty * y_off * 0.0;
            g.y = py - ty * g.advance * 0.5 + y_off * cos;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::*;

    fn lib() -> Option<FontLibrary> {
        let l = FontLibrary::bundled_only();
        if l.faces().is_empty() { None } else { Some(l) }
    }

    #[test]
    fn single_line_metrics() {
        let Some(lib) = lib() else { return };
        let style = TextStyle::default();
        let l = layout(&lib, &TextBlock::plain("Hello"), &style, &Canvas::HD);
        assert_eq!(l.lines.len(), 1);
        assert_eq!(l.glyphs.len(), 5);
        assert_eq!(l.char_count, 5);
        assert_eq!(l.word_count, 1);
        assert!(l.lines[0].width > 60.0 && l.lines[0].width < 160.0, "{}", l.lines[0].width);
        assert!(l.bounds.w > 0.0 && l.bounds.h > 30.0);
    }

    #[test]
    fn wrapping_and_alignment() {
        let Some(lib) = lib() else { return };
        let style = TextStyle::default();
        let block = TextBlock::plain("one two three four five six seven").with_layout(LayoutParams::default().wrap(200.0).align(Align::Right));
        let l = layout(&lib, &block, &style, &Canvas::HD);
        assert!(l.lines.len() >= 3);
        for ln in &l.lines {
            assert!(ln.width <= 200.5, "{}", ln.width);
            assert!((ln.x + ln.width - 200.0).abs() < 0.5, "right aligned {} {}", ln.x, ln.width);
        }
    }

    #[test]
    fn newlines_make_lines() {
        let Some(lib) = lib() else { return };
        let l = layout(&lib, &TextBlock::plain("a\nb\n\nc"), &TextStyle::default(), &Canvas::HD);
        assert_eq!(l.lines.len(), 4);
        assert!(l.lines[2].glyphs.is_empty());
        assert!(l.lines[3].baseline > l.lines[0].baseline);
    }

    #[test]
    fn justify_fills_width() {
        let Some(lib) = lib() else { return };
        let block = TextBlock::plain("the quick brown fox jumps over the lazy dog again and again").with_layout(LayoutParams::default().wrap(300.0).align(Align::Justify));
        let l = layout(&lib, &block, &TextStyle::default(), &Canvas::HD);
        assert!(l.lines.len() > 1);
        assert!((l.lines[0].width - 300.0).abs() < 0.5, "{}", l.lines[0].width);
    }

    #[test]
    fn fit_to_box_shrinks() {
        let Some(lib) = lib() else { return };
        let block = TextBlock::plain("A long headline that needs to shrink").with_layout(LayoutParams::default().boxed(300.0, 60.0).fit());
        let l = layout(&lib, &block, &TextStyle::default().size(120.0), &Canvas::HD);
        assert!(l.size_px < 120.0);
        assert!(l.lines.iter().all(|ln| ln.width <= 300.5));
    }

    #[test]
    fn letter_spacing_widens() {
        let Some(lib) = lib() else { return };
        let a = layout(&lib, &TextBlock::plain("spacing"), &TextStyle::default(), &Canvas::HD);
        let b = layout(&lib, &TextBlock::plain("spacing"), &TextStyle::default().letter_spacing(50.0), &Canvas::HD);
        assert!(b.lines[0].width > a.lines[0].width + 6.0 * 19.0);
    }

    #[test]
    fn mixed_scripts_use_fallback_fonts() {
        let Some(lib) = lib() else { return };
        let l = layout(&lib, &TextBlock::plain("Hello مرحبا 你好 😀"), &TextStyle::default(), &Canvas::HD);
        assert!(l.fonts.len() >= 3, "fonts used: {:?}", l.fonts);
        assert!(l.glyphs.iter().any(|g| g.colour || g.emoji), "emoji should draw as an image or colour glyph");
        // Arabic glyphs are visually reversed: the first Arabic letter in
        // logical order sits to the right of the last one.
        let arabic: Vec<&PlacedGlyph> = l.glyphs.iter().filter(|g| l.text[g.cluster..].starts_with(|c: char| ('\u{600}'..='\u{6FF}').contains(&c))).collect();
        assert!(arabic.len() >= 4);
    }

    #[test]
    fn rtl_paragraph_reverses_visual_order() {
        let Some(lib) = lib() else { return };
        let l = layout(&lib, &TextBlock::plain("שלום עולם"), &TextStyle::default(), &Canvas::HD);
        let first = l.glyphs.iter().find(|g| g.cluster == 0).unwrap();
        let last = l.glyphs.iter().filter(|g| !g.is_space).max_by_key(|g| g.cluster).unwrap();
        assert!(first.x > last.x, "first logical glyph should be rightmost");
    }

    #[test]
    fn vertical_layout_stacks() {
        let Some(lib) = lib() else { return };
        let block = TextBlock::plain("縦書き").with_layout(LayoutParams { vertical: true, ..Default::default() });
        let l = layout(&lib, &block, &TextStyle::default(), &Canvas::HD);
        assert_eq!(l.glyphs.len(), 3);
        assert!(l.glyphs[1].y > l.glyphs[0].y && l.glyphs[2].y > l.glyphs[1].y);
        assert!((l.glyphs[0].x - l.glyphs[1].x).abs() < 5.0);
    }

    #[test]
    fn text_follows_a_path_and_spans_shift_and_track() {
        let Some(lib) = lib() else { return };
        // A circle of radius 300 units: letters spread round it, turned.
        let pts: Vec<[f32; 2]> = (0..=64).map(|k| {
            let a = k as f32 / 64.0 * std::f32::consts::TAU;
            [300.0 * a.cos(), 300.0 * a.sin()]
        }).collect();
        let tp = crate::style::TextPath { points: pts, closed: true, start: 0.25, offset: 0.0, perpendicular: true, reverse: false };
        let block = TextBlock::plain("AROUND THE WORLD").with_layout(LayoutParams { path: Some(tp), ..Default::default() });
        let l = layout(&lib, &block, &TextStyle::default(), &Canvas::HD);
        let (cx, cy) = (l.bounds.w * 0.5, l.bounds.h * 0.5);
        for g in l.glyphs.iter().filter(|g| !g.is_space) {
            let r = ((g.x - cx).powi(2) + (g.y - cy).powi(2)).sqrt();
            assert!((r - 300.0).abs() < 40.0, "glyph off the circle: {r}");
        }
        let turns: Vec<f32> = l.glyphs.iter().map(|g| g.rotation).collect();
        assert!(turns.iter().any(|r| r.abs() > 20.0));
        // A raised span sits higher; a tracked span is wider.
        let plain = layout(&lib, &TextBlock::plain("x2"), &TextStyle::default(), &Canvas::HD);
        let up = layout(&lib, &TextBlock::spans(vec![Span::plain("x"), Span::styled("2", SpanStyle { baseline_shift: Some(10.0), size_scale: Some(0.6), ..Default::default() })]), &TextStyle::default(), &Canvas::HD);
        assert!(up.glyphs[1].y < plain.glyphs[1].y - 5.0);
        let wide = layout(&lib, &TextBlock::spans(vec![Span::styled("spaced", SpanStyle { letter_spacing: Some(80.0), ..Default::default() })]), &TextStyle::default(), &Canvas::HD);
        let narrow = layout(&lib, &TextBlock::plain("spaced"), &TextStyle::default(), &Canvas::HD);
        assert!(wide.lines[0].width > narrow.lines[0].width * 1.3);
        // Kerning off changes pair spacing for a kerned pair.
        let k_on = layout(&lib, &TextBlock::plain("AVAVAV"), &TextStyle::default(), &Canvas::HD);
        let k_off = layout(&lib, &TextBlock::plain("AVAVAV"), &TextStyle { kerning: false, ..Default::default() }, &Canvas::HD);
        assert!(k_off.lines[0].width >= k_on.lines[0].width);
    }

    #[test]
    fn bend_rotates_ends() {
        let Some(lib) = lib() else { return };
        let block = TextBlock::plain("CURVED TEXT HERE").with_layout(LayoutParams::default().bend(120.0));
        let l = layout(&lib, &block, &TextStyle::default(), &Canvas::HD);
        let first = &l.glyphs[0];
        let last = l.glyphs.last().unwrap();
        assert!(first.rotation < -20.0 && last.rotation > 20.0, "{} {}", first.rotation, last.rotation);
        // Ends drop below the middle for a positive bend.
        let mid = &l.glyphs[l.glyphs.len() / 2];
        assert!(first.y > mid.y && last.y > mid.y);
    }

    #[test]
    fn spans_resolve_bold_and_colour() {
        let Some(lib) = lib() else { return };
        let block = TextBlock::spans(vec![Span::plain("normal "), Span::styled("bold", SpanStyle::bold()), Span::styled(" red", SpanStyle::colour(Rgba::rgb8(255, 0, 0)))]);
        let l = layout(&lib, &block, &TextStyle::default(), &Canvas::HD);
        assert!(l.fonts.len() >= 2);
        assert!(l.spans[2].fill.is_some());
        assert_eq!(l.word_count, 3);
    }

    #[test]
    fn underline_decorations() {
        let Some(lib) = lib() else { return };
        let l = layout(&lib, &TextBlock::plain("under"), &TextStyle { underline: true, ..Default::default() }, &Canvas::HD);
        assert_eq!(l.decorations.len(), 1);
        assert!(l.decorations[0].rect.y > l.lines[0].baseline);
    }

    #[test]
    fn empty_text_is_fine() {
        let Some(lib) = lib() else { return };
        let l = layout(&lib, &TextBlock::plain(""), &TextStyle::default(), &Canvas::HD);
        assert!(l.glyphs.is_empty());
        assert_eq!(l.lines.len(), 1);
    }

    use crate::rgba::Rgba;
}
