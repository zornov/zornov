// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Charts animated from data: bars that grow, a line that draws itself
//! with its points popping on, a pie or donut that sweeps round, and a
//! map of places joined by travelling arcs. Each is built from shape
//! layers (the marks) and titles (labels and counting values).

use crate::data::Row;
use crate::{Anchor, Counter, Layer, ShapeLayer, TextLayer};
use serde_json::{Value, json};
use studio_text::{Reveal, RevealKind, TextMotion, TextUnit};

/// How a chart looks and where it sits.
#[derive(Clone, Debug)]
pub struct Look {
    /// Colours for the series or slices, in turn.
    pub palette: Vec<String>,
    pub text: String,
    /// Grid and axis lines.
    pub grid: String,
    pub font: String,
    /// The chart's box: centre and size in canvas pixels.
    pub centre: [f64; 2],
    pub size: [f64; 2],
    /// Seconds into the layer the marks start growing, and how long the
    /// whole build takes.
    pub start: f64,
    pub build: f64,
    /// Seconds the chart is on screen.
    pub duration: f64,
    pub decimals: u32,
    pub prefix: String,
    pub suffix: String,
    /// Draw value labels.
    pub values: bool,
    /// Text size, canvas pixels.
    pub text_size: f64,
}

impl Default for Look {
    fn default() -> Self {
        Look {
            palette: ["#7c5cff", "#21c7a8", "#ffb020", "#ff5c7a", "#3fa9ff", "#a0e05a", "#ff8a3d", "#c86bff"].iter().map(|s| s.to_string()).collect(),
            text: "#ffffff".into(),
            grid: "#ffffff".into(),
            font: "Manrope".into(),
            centre: [0.0, 0.0],
            size: [1200.0, 600.0],
            start: 0.2,
            build: 1.6,
            duration: 6.0,
            decimals: 0,
            prefix: String::new(),
            suffix: String::new(),
            values: true,
            text_size: 30.0,
        }
    }
}

fn colour_of(look: &Look, row: &Row, i: usize) -> String {
    row.colour.clone().unwrap_or_else(|| look.palette[i % look.palette.len().max(1)].clone())
}

fn fade_in(unit: TextUnit, dur: f64) -> TextMotion {
    TextMotion { reveal: Some(Reveal::new(RevealKind::Rise, unit).overlap(0.6).distance(0.5)), reveal_duration: dur as f32, exit: Some(Reveal::new(RevealKind::Fade, TextUnit::Line)), exit_duration: 0.4, ..Default::default() }
}

/// A label that appears at `start` (seconds into the chart).
fn label(look: &Look, name: &str, text: &str, x: f64, y: f64, anchor: Anchor, start: f64, size: f64, weight: u16) -> Layer {
    let start = start.clamp(0.0, (look.duration - 0.3).max(0.0));
    Layer::Text(TextLayer {
        name: name.into(),
        text: text.into(),
        font: look.font.clone(),
        weight,
        italic: false,
        size,
        colour: look.text.clone(),
        letter_spacing: 0.0,
        anchor,
        x,
        y,
        start,
        duration: look.duration - start,
        motion: Some(fade_in(TextUnit::Character, 0.5)),
        counter: None,
        path: None,
    })
}

/// A value that counts up from 0 as its mark grows.
fn counting(look: &Look, name: &str, value: f64, x: f64, y: f64, anchor: Anchor, start: f64, grow: f64, size: f64) -> Layer {
    let start = start.clamp(0.0, (look.duration - 0.3).max(0.0));
    Layer::Text(TextLayer {
        name: name.into(),
        text: crate::format_number(value, look.decimals, true),
        font: look.font.clone(),
        weight: 700,
        italic: false,
        size,
        colour: look.text.clone(),
        letter_spacing: 0.0,
        anchor,
        x,
        y,
        start,
        duration: look.duration - start,
        motion: Some(TextMotion { reveal: Some(Reveal::new(RevealKind::Fade, TextUnit::Line)), reveal_duration: 0.25, exit: Some(Reveal::new(RevealKind::Fade, TextUnit::Line)), exit_duration: 0.4, ..Default::default() }),
        counter: Some(Counter { from: 0.0, to: value, start: 0.0, duration: grow, decimals: look.decimals, prefix: look.prefix.clone(), suffix: look.suffix.clone(), separator: true }),
        path: None,
    })
}

fn shape(look: &Look, name: &str, items: Value) -> Layer {
    Layer::Shape(ShapeLayer { name: name.into(), doc: json!({"items": items}), x: look.centre[0], y: look.centre[1], start: 0.0, duration: look.duration })
}

/// Fades a shape group out over the last 0.4 s.
fn exit_opacity(look: &Look) -> Value {
    let end = look.duration;
    json!({"keys": [[(end - 0.45).max(0.0), 1], [end, 0, "in"]]})
}

/// A nice round top for an axis holding `max`.
fn nice_max(max: f64) -> f64 {
    if max <= 0.0 {
        return 1.0;
    }
    let p = 10f64.powf(max.log10().floor());
    for k in [1.0, 1.2, 1.5, 2.0, 2.5, 3.0, 4.0, 5.0, 6.0, 8.0, 10.0] {
        if k * p >= max {
            return k * p;
        }
    }
    10.0 * p
}

/// Bars growing up from a baseline, one after another.
pub fn bar(rows: &[Row], look: &Look) -> Vec<Layer> {
    let n = rows.len().max(1);
    let [w, h] = look.size;
    let label_room = look.text_size * 1.8;
    let top_room = if look.values { look.text_size * 1.6 } else { 0.0 };
    let chart_h = (h - label_room - top_room).max(10.0);
    let base = h / 2.0 - label_room;
    let max = nice_max(rows.iter().map(|r| r.value).fold(0.0, f64::max));
    let slot = w / n as f64;
    let bw = slot * 0.62;
    let each = look.build / (1.0 + (n as f64 - 1.0) * 0.35);
    let mut items = vec![
        // Grid lines, drawn on first.
        json!({"type": "group", "opacity": 0.18, "items": (1..=4).map(|k| json!({"type": "line", "from": [-w / 2.0, base - chart_h * k as f64 / 4.0], "to": [w / 2.0, base - chart_h * k as f64 / 4.0]})).chain(std::iter::once(json!({"type": "trim", "end": {"keys": [[look.start * 0.5, 0], [look.start + 0.6, 1, "out"]]}}))).chain(std::iter::once(json!({"type": "stroke", "colour": look.grid, "width": 2, "cap": "butt"}))).collect::<Vec<_>>()}),
        json!({"type": "group", "items": [{"type": "line", "from": [-w / 2.0, base], "to": [w / 2.0, base]}, {"type": "trim", "end": {"keys": [[0, 0], [look.start + 0.4, 1, "out"]]}}, {"type": "stroke", "colour": look.grid, "width": 3, "cap": "round", "opacity": 0.7}]}),
    ];
    let mut layers = Vec::new();
    for (i, r) in rows.iter().enumerate() {
        let x = -w / 2.0 + slot * (i as f64 + 0.5);
        let bh = (r.value.max(0.0) / max) * chart_h;
        let t0 = look.start + each * 0.35 * i as f64;
        let c = colour_of(look, r, i);
        items.push(json!({"type": "group", "position": [x, base], "scale": {"keys": [[t0, [1, 0]], [t0 + each, [1, 1], "overshoot"]]}, "items": [
            {"type": "rect", "position": [0, -bh / 2.0], "size": [bw, bh.max(0.5)], "roundness": (bw * 0.12).min(bh / 2.0)},
            {"type": "fill", "gradient": {"type": "linear", "from": [0, -bh], "to": [0, 0], "stops": [c.clone(), c]}},
        ]}));
        layers.push(label(look, &format!("{} label", r.label), &r.label, look.centre[0] + x, look.centre[1] + base + look.text_size * 0.95, Anchor::Centre, t0, look.text_size * 0.8, 500));
        if look.values {
            layers.push(counting(look, &format!("{} value", r.label), r.value, look.centre[0] + x, look.centre[1] + base - bh - look.text_size * 0.75, Anchor::Centre, t0, each, look.text_size));
        }
    }
    let mut out = vec![shape(look, "Bars", json!([{"type": "group", "opacity": exit_opacity(look), "items": items}]))];
    out.extend(layers);
    out
}

/// A line through the values that draws itself on, its points popping in
/// as it reaches them, over a soft area.
pub fn line(rows: &[Row], look: &Look) -> Vec<Layer> {
    let n = rows.len().max(2);
    let [w, h] = look.size;
    let label_room = look.text_size * 1.8;
    let chart_h = (h - label_room - look.text_size * 1.6).max(10.0);
    let base = h / 2.0 - label_room;
    let max = nice_max(rows.iter().map(|r| r.value).fold(0.0, f64::max));
    let pts: Vec<[f64; 2]> = rows.iter().enumerate().map(|(i, r)| [-w / 2.0 + w * i as f64 / (n - 1) as f64, base - r.value.max(0.0) / max * chart_h]).collect();
    let c = look.palette.first().cloned().unwrap_or_else(|| "#7c5cff".into());
    let (t0, t1) = (look.start, look.start + look.build);
    let mut area = pts.clone();
    if let (Some(l), Some(f)) = (pts.last(), pts.first()) {
        area.push([l[0], base]);
        area.push([f[0], base]);
    }
    let mut items = vec![
        json!({"type": "group", "opacity": 0.18, "items": (1..=4).map(|k| json!({"type": "line", "from": [-w / 2.0, base - chart_h * k as f64 / 4.0], "to": [w / 2.0, base - chart_h * k as f64 / 4.0]})).chain([json!({"type": "stroke", "colour": look.grid, "width": 2})]).collect::<Vec<_>>()}),
        // The area under the line, fading in behind it.
        json!({"type": "group", "opacity": {"keys": [[t0 + look.build * 0.5, 0], [t1 + 0.3, 1]]}, "items": [
            {"type": "path", "points": area, "closed": true},
            {"type": "fill", "gradient": {"type": "linear", "from": [0, base - chart_h], "to": [0, base], "stops": [[0, format!("{c}66")], [1, format!("{c}00")]]}},
        ]}),
        json!({"type": "group", "items": [
            {"type": "path", "points": pts, "smooth": true},
            {"type": "trim", "end": {"keys": [[t0, 0], [t1, 1, "in_out"]]}},
            {"type": "stroke", "colour": c, "width": (look.text_size * 0.18).max(3.0), "cap": "round", "join": "round"},
        ]}),
    ];
    let mut layers = Vec::new();
    for (i, (r, p)) in rows.iter().zip(&pts).enumerate() {
        let at = t0 + look.build * i as f64 / (n - 1) as f64;
        items.push(json!({"type": "group", "position": p, "scale": {"keys": [[at - 0.05, 0], [at + 0.3, 1, "overshoot"]]}, "items": [
            {"type": "circle", "radius": look.text_size * 0.32},
            {"type": "fill", "colour": "#ffffff"},
            {"type": "stroke", "colour": c, "width": (look.text_size * 0.14).max(2.0)},
        ]}));
        layers.push(label(look, &format!("{} label", r.label), &r.label, look.centre[0] + p[0], look.centre[1] + base + look.text_size * 0.95, Anchor::Centre, at, look.text_size * 0.75, 500));
        if look.values {
            layers.push(counting(look, &format!("{} value", r.label), r.value, look.centre[0] + p[0], look.centre[1] + p[1] - look.text_size * 0.95, Anchor::Centre, at, 0.4, look.text_size * 0.85));
        }
    }
    let mut out = vec![shape(look, "Line", json!([{"type": "group", "opacity": exit_opacity(look), "items": items}]))];
    out.extend(layers);
    out
}

/// A pie (or donut, with `hole` 0..1 of the radius) sweeping round, with
/// a legend of shares, and the total counting in the hole.
pub fn pie(rows: &[Row], look: &Look, hole: f64, title: &str) -> Vec<Layer> {
    let total: f64 = rows.iter().map(|r| r.value.max(0.0)).sum::<f64>().max(1e-9);
    let r = (look.size[1] / 2.0).min(look.size[0] * 0.3);
    let cx = -look.size[0] / 2.0 + r;
    let inner = r * hole.clamp(0.0, 0.95);
    let mut items = Vec::new();
    let mut a0 = 0.0;
    let sweep = look.build;
    let mut layers = Vec::new();
    let legend_x = cx + r + look.size[0] * 0.08;
    let row_h = look.text_size * 1.6;
    let legend_top = -row_h * (rows.len() as f64 - 1.0) / 2.0;
    for (i, row) in rows.iter().enumerate() {
        let a1 = a0 + row.value.max(0.0) / total * 360.0;
        let (s0, s1) = (look.start + sweep * a0 / 360.0, look.start + sweep * a1 / 360.0);
        let c = colour_of(look, row, i);
        items.push(json!({"type": "group", "items": [
            {"type": "pie", "position": [cx, 0], "radius": r, "inner": inner, "start": a0, "end": {"keys": [[s0, a0], [s1, a1, "linear"]]}},
            {"type": "fill", "colour": c},
            {"type": "stroke", "colour": "#00000055", "width": 2},
        ]}));
        let y = legend_top + row_h * i as f64;
        items.push(json!({"type": "group", "position": [legend_x, y], "scale": {"keys": [[s0, 0], [s0 + 0.3, 1, "overshoot"]]}, "items": [
            {"type": "rect", "size": [look.text_size * 0.7, look.text_size * 0.7], "roundness": look.text_size * 0.18},
            {"type": "fill", "colour": colour_of(look, row, i)},
        ]}));
        let pct = row.value.max(0.0) / total * 100.0;
        layers.push(label(look, &format!("{} label", row.label), &format!("{}  {}%", row.label, crate::format_number(pct, look.decimals.min(1), false)), look.centre[0] + legend_x + look.text_size * 0.75, look.centre[1] + y, Anchor::Left, s0, look.text_size * 0.85, 600));
        a0 = a1;
    }
    let mut out = vec![shape(look, "Pie", json!([{"type": "group", "opacity": exit_opacity(look), "items": items}]))];
    if hole > 0.2 {
        out.push(counting(look, "Total", total, look.centre[0] + cx, look.centre[1] - look.text_size * 0.2, Anchor::Centre, look.start, look.build, look.text_size * 1.5));
        if !title.is_empty() {
            out.push(label(look, "Total label", title, look.centre[0] + cx, look.centre[1] + look.text_size * 1.2, Anchor::Centre, look.start + 0.2, look.text_size * 0.7, 500));
        }
    }
    out.extend(layers);
    out
}

/// Places on a map: pins popping in, joined in order by arcs that travel
/// from one to the next, over a latitude / longitude grid. Rows carry
/// `[longitude, latitude]` before their value. `bounds` is (west, south,
/// east, north) in degrees; unset, it fits the places.
pub fn map(rows: &[Row], look: &Look, bounds: Option<[f64; 4]>, route: bool) -> Vec<Layer> {
    let places: Vec<(&Row, [f64; 2])> = rows.iter().filter_map(|r| (r.more.len() >= 2).then(|| (r, [r.more[0], r.more[1]]))).collect();
    let [w, h] = look.size;
    let b = bounds.unwrap_or_else(|| {
        let (mut lo, mut hi) = ([180.0f64, 90.0f64], [-180.0f64, -90.0f64]);
        for (_, p) in &places {
            lo = [lo[0].min(p[0]), lo[1].min(p[1])];
            hi = [hi[0].max(p[0]), hi[1].max(p[1])];
        }
        if places.is_empty() {
            return [-180.0, -60.0, 180.0, 80.0];
        }
        // Some room round the places, and the frame's shape.
        let pad = ((hi[0] - lo[0]).max(hi[1] - lo[1]) * 0.25).max(4.0);
        let (mut west, mut east, mut south, mut north) = (lo[0] - pad, hi[0] + pad, lo[1] - pad, hi[1] + pad);
        let want = w / h;
        let (dx, dy) = (east - west, north - south);
        if dx / dy < want {
            let grow = (dy * want - dx) / 2.0;
            west -= grow;
            east += grow;
        } else {
            let grow = (dx / want - dy) / 2.0;
            south -= grow;
            north += grow;
        }
        [west.max(-180.0), south.max(-85.0), east.min(180.0), north.min(85.0)]
    });
    let proj = |p: [f64; 2]| -> [f64; 2] { [-w / 2.0 + (p[0] - b[0]) / (b[2] - b[0]).max(1e-6) * w, h / 2.0 - (p[1] - b[1]) / (b[3] - b[1]).max(1e-6) * h] };
    let step = {
        let span = (b[2] - b[0]).max(b[3] - b[1]);
        [1.0, 2.0, 5.0, 10.0, 15.0, 30.0].into_iter().find(|s| span / s <= 12.0).unwrap_or(30.0)
    };
    let mut grid = Vec::new();
    let mut lon = (b[0] / step).ceil() * step;
    while lon <= b[2] {
        grid.push(json!({"type": "line", "from": proj([lon, b[1]]), "to": proj([lon, b[3]])}));
        lon += step;
    }
    let mut lat = (b[1] / step).ceil() * step;
    while lat <= b[3] {
        grid.push(json!({"type": "line", "from": proj([b[0], lat]), "to": proj([b[2], lat])}));
        lat += step;
    }
    grid.push(json!({"type": "trim", "end": {"keys": [[0, 0], [look.start + 0.8, 1, "out"]]}, "mode": "together"}));
    grid.push(json!({"type": "stroke", "colour": look.grid, "width": 1.5, "opacity": 0.22}));
    let mut items = vec![json!({"type": "rect", "size": [w, h], "roundness": 18}), json!({"type": "fill", "colour": "#0b1020", "opacity": 0.55}), json!({"type": "group", "items": grid})];
    let n = places.len().max(1);
    let each = look.build / n as f64;
    let mut layers = Vec::new();
    let c = look.palette.first().cloned().unwrap_or_else(|| "#7c5cff".into());
    for (i, (row, ll)) in places.iter().enumerate() {
        let p = proj(*ll);
        let at = look.start + each * i as f64;
        if route && i > 0 {
            let q = proj(places[i - 1].1);
            // A curve bowing up between the two.
            let mid = [(p[0] + q[0]) / 2.0, (p[1] + q[1]) / 2.0 - ((p[0] - q[0]).hypot(p[1] - q[1]) * 0.25)];
            items.push(json!({"type": "group", "items": [
                {"type": "path", "points": [q, mid, p], "smooth": true},
                {"type": "trim", "end": {"keys": [[at - each * 0.8, 0], [at, 1, "in_out"]]}},
                {"type": "stroke", "colour": c, "width": 4, "dash": [14, 10], "cap": "round"},
            ]}));
        }
        let pin = look.text_size * 0.3;
        items.push(json!({"type": "group", "position": p, "items": [
            {"type": "circle", "radius": format!("={} * fract((t - {at}) / 1.6) * step({at}, t)", pin * 4.0)},
            {"type": "stroke", "colour": colour_of(look, row, i), "width": 3, "opacity": format!("=(1 - fract((t - {at}) / 1.6)) * step({at}, t)")},
        ]}));
        items.push(json!({"type": "group", "position": p, "scale": {"keys": [[at, 0], [at + 0.35, 1, "overshoot"]]}, "items": [
            {"type": "circle", "radius": pin},
            {"type": "fill", "colour": colour_of(look, row, i)},
            {"type": "stroke", "colour": "#ffffff", "width": 3},
        ]}));
        let text = if row.value != 0.0 && look.values { format!("{}  {}{}{}", row.label, look.prefix, crate::format_number(row.value, look.decimals, true), look.suffix) } else { row.label.clone() };
        layers.push(label(look, &format!("{} label", row.label), &text, look.centre[0] + p[0] + pin * 2.2, look.centre[1] + p[1] - pin * 2.0, Anchor::Left, at + 0.1, look.text_size * 0.8, 600));
    }
    let mut out = vec![shape(look, "Map", json!([{"type": "group", "opacity": exit_opacity(look), "items": items}]))];
    out.extend(layers);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::rows;

    #[test]
    fn charts_build_marks_and_labels() {
        let look = Look::default();
        let r = rows(&json!([["Mon", 12], ["Tue", 30], ["Wed", 18]]));
        let b = bar(&r, &look);
        assert_eq!(b.len(), 1 + 2 * 3);
        assert!(studio_vector::check(match &b[0] { Layer::Shape(s) => &s.doc, _ => panic!() }).is_empty());
        let l = line(&r, &look);
        assert!(matches!(&l[0], Layer::Shape(_)));
        let p = pie(&r, &look, 0.6, "Visits");
        assert!(p.iter().any(|x| matches!(x, Layer::Text(t) if t.counter.as_ref().is_some_and(|c| (c.to - 60.0).abs() < 1e-9))));
        let m = map(&rows(&json!([["London", -0.13, 51.5, 0], ["Paris", 2.35, 48.86, 0], ["Rome", 12.5, 41.9, 0]])), &look, None, true);
        assert_eq!(m.len(), 4);
        // Drawn at the end of the build: the bars are up.
        let Layer::Shape(s) = &b[0] else { panic!() };
        let pic = studio_vector::render(&s.doc, &studio_vector::Ctx::at(3.0, 6.0), 192, 108, [1920.0, 1080.0]);
        assert!(pic.rgba.chunks_exact(4).filter(|q| q[3] > 200).count() > 200);
        assert_eq!(nice_max(37.0), 40.0);
        assert_eq!(nice_max(0.0), 1.0);
    }
}
