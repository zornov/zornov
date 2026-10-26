// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `template` and `chart`: motion graphics templates with controls to
//! fill in (Essential Graphics / MOGRT style), and charts animated from
//! data. A placed template is one clip (its layers merged inside it)
//! that remembers its template and controls in `meta["template"]`, so it
//! can be filled in again: `{"op": "template", "clip": 12, "controls":
//! {"name": "Ada"}}` rebuilds it in place, keeping its id, time, lane,
//! transform and effects.
//!
//! * `{"op": "template", "list": true}` (or `"search": "lower third"`,
//!   `"category": "chart"`): what there is, with each one's controls.
//! * `{"op": "template", "id": "lower_third.bar", "controls": {...},
//!   "at": 2, "duration": 5}`: places one (`x`, `y` move it as a whole).
//! * `{"op": "chart", "kind": "bar", "data": [["Mon", 3], …], "title",
//!   "colour", "prefix", "suffix", "decimals", "at", "duration"}`: kinds
//!   bar, line, pie, donut, map (rows `[place, longitude, latitude]`).

use super::{SEC, Session, f, micros, secs};
use serde_json::{Map, Value, json};
use studio_model as m;
use studio_mograph::{Anchor, Kind, Layer, Template};

/// The key a placed template keeps its id and controls under.
pub const META: &str = "template";

fn rgba(s: &str) -> Option<m::Rgba> {
    let k = super::craft::colour(s)?;
    Some(m::Rgba { r: k.r() as f64 / 255.0, g: k.g() as f64 / 255.0, b: k.b() as f64 / 255.0, a: k.a() as f64 / 255.0 })
}

fn hex(s: &str) -> String {
    match super::craft::colour(s) {
        Some(c) if c.a() < 255 => format!("#{:02x}{:02x}{:02x}{:02x}", c.r(), c.g(), c.b(), c.a()),
        Some(c) => format!("#{:02x}{:02x}{:02x}", c.r(), c.g(), c.b()),
        None => s.to_owned(),
    }
}

/// Width and height in pixels of a line of text on a canvas `canvas_h`
/// pixels high.
pub fn measure(canvas_h: f64) -> impl Fn(&str, &str, u16, f64, f64) -> [f64; 2] {
    move |text: &str, font: &str, weight: u16, size: f64, spacing: f64| {
        let engine = crate::text_look::engine();
        let h = canvas_h.max(1.0);
        let canvas = studio_text::Canvas::new((h * 16.0 / 9.0) as u32, h as u32);
        let units = size * 1080.0 / h;
        let mut st = studio_text::TextStyle::new(studio_text::FontRequest::new(font, weight, false), units as f32, studio_text::Fill::default());
        st.letter_spacing = spacing as f32;
        let l = engine.layout(&studio_text::TextBlock::plain(text), &st, &canvas);
        let w = l.lines.iter().map(|ln| ln.width).fold(0.0f32, f32::max) as f64;
        [w, l.bounds.h as f64]
    }
}

/// Where one of a template's text controls shows on a canvas `w × h` px:
/// the text layer that carries its words.
#[derive(Clone, Debug)]
pub struct TextSpot {
    pub control: &'static str,
    pub text: String,
    /// The middle of the text, canvas pixels from the canvas centre.
    pub centre: [f64; 2],
    /// Font size and line width in canvas pixels.
    pub size: f64,
    pub width: f64,
    /// Seconds into the template it shows, and for how long.
    pub start: f64,
    pub duration: f64,
}

/// The text controls of `t` (with `vals` over the defaults) and where each
/// is drawn: a control is the layer showing its current words.
pub fn text_spots(t: &Template, vals: &Map<String, Value>, w: f64, h: f64, dur: f64) -> Vec<TextSpot> {
    let meas = measure(h);
    let Ok(layers) = t.build(vals, w, h, dur, &meas) else { return Vec::new() };
    let mut used = vec![false; layers.len()];
    let mut out = Vec::new();
    for c in t.controls.iter().filter(|c| matches!(c.kind, Kind::Text)) {
        let want = vals.get(c.id).or(Some(&c.default)).and_then(Value::as_str).unwrap_or("");
        if want.is_empty() {
            continue;
        }
        let Some(i) = layers.iter().enumerate().position(|(i, l)| !used[i] && matches!(l, Layer::Text(x) if x.counter.is_none() && x.path.is_none() && x.text == want)) else { continue };
        used[i] = true;
        let Layer::Text(x) = &layers[i] else { continue };
        let width = meas(&x.text, &x.font, x.weight, x.size, x.letter_spacing)[0];
        let cx = match x.anchor {
            Anchor::Left => x.x + width / 2.0,
            Anchor::Centre => x.x,
            Anchor::Right => x.x - width / 2.0,
        };
        out.push(TextSpot { control: c.id, text: x.text.clone(), centre: [cx, x.y], size: x.size, width, start: x.start, duration: x.duration });
    }
    out
}

/// Control values from the op, colours as hex, checked against the
/// template.
fn values(t: &Template, given: Option<&Value>) -> Result<Map<String, Value>, String> {
    let mut out = Map::new();
    let Some(given) = given else { return Ok(out) };
    let Value::Object(o) = given else { return Err("\"controls\" is {control id: value}".into()) };
    for (k, v) in o {
        let c = t.controls.iter().find(|c| c.id == k).ok_or_else(|| format!("{} has no control \"{k}\"; controls: {}", t.id, t.controls.iter().map(|c| c.id).collect::<Vec<_>>().join(", ")))?;
        let v = match (&c.kind, v) {
            (Kind::Colour, Value::String(s)) => {
                if super::craft::colour(s).is_none() {
                    return Err(format!("{k}: \"{s}\" is not #rrggbb or a basic name"));
                }
                json!(hex(s))
            }
            (Kind::Number { min, max }, x) => {
                let n = x.as_f64().or_else(|| x.as_str().and_then(|s| s.trim().trim_end_matches('%').parse().ok())).ok_or_else(|| format!("{k} is a number"))?;
                json!(n.clamp(*min, *max))
            }
            (Kind::Choice { options }, Value::String(s)) if !options.contains(&s.as_str()) => return Err(format!("{k}: one of {}", options.join(", "))),
            (_, x) => x.clone(),
        };
        out.insert(k.clone(), v);
    }
    Ok(out)
}

fn describe(t: &Template) -> String {
    let controls: Vec<String> = t
        .controls
        .iter()
        .map(|c| {
            let kind = match &c.kind {
                Kind::Text => "text".to_owned(),
                Kind::Colour => "colour".to_owned(),
                Kind::Number { min, max } => format!("number {min}..{max}"),
                Kind::Choice { options } => format!("one of {}", options.join("/")),
                Kind::Toggle => "true/false".to_owned(),
                Kind::Data => "rows".to_owned(),
                Kind::Point => "[x, y] px".to_owned(),
            };
            format!("{} ({kind}, default {})", c.id, c.default)
        })
        .collect();
    format!("{} — {} [{}], {}s: {}. Controls: {}", t.id, t.name, t.category.label(), t.duration, t.description, controls.join("; "))
}

/// A layer as a model clip (lane set by the caller).
fn layer_clip(layer: &Layer, id: u64, lane: u64, at: i64, w: f64, h: f64, meas: &dyn Fn(&str, &str, u16, f64, f64) -> [f64; 2]) -> Result<m::Clip, String> {
    let unit = 1080.0 / h.max(1.0);
    let (source, name, x, y, start, dur) = match layer {
        Layer::Text(t) => {
            let mut g = m::Generator::new(m::GeneratorKind::Text);
            let mut style = m::ParamMap::new();
            style.insert("font".into(), m::Value::Text(t.font.clone()));
            style.insert("weight".into(), m::Value::Scalar(t.weight as f64));
            if t.italic {
                style.insert("italic".into(), m::Value::Bool(true));
            }
            let text = t.counter.as_ref().map(|c| c.at(f64::MAX)).unwrap_or_else(|| t.text.clone());
            g.text.push(m::TextRun { text: text.clone(), style });
            g.params.insert("size".into(), m::Value::Scalar(t.size * unit));
            g.params.insert("letter_spacing".into(), m::Value::Scalar(t.letter_spacing));
            g.params.insert("colour".into(), m::Value::Colour(rgba(&t.colour).ok_or_else(|| format!("colour \"{}\"", t.colour))?));
            g.params.insert("plate".into(), m::Value::Bool(false));
            // A motion setting even when still: the renderer then hands
            // the text its moment (counters need it).
            let motion = t.motion.clone().unwrap_or_default();
            g.params.insert("motion".into(), m::Value::Text(serde_json::to_string(&motion).map_err(|e| e.to_string())?));
            if let Some(c) = &t.counter {
                g.params.insert("counter".into(), m::Value::Text(serde_json::to_string(c).map_err(|e| e.to_string())?));
            }
            if let Some(p) = &t.path {
                let spec = super::mograph::path_spec(p)?;
                g.params.insert("text_path".into(), m::Value::Text(spec.to_string()));
                if let Some(s) = p.get("start").and_then(Value::as_f64) {
                    g.params.insert("path_start".into(), m::Value::Scalar(s));
                }
            }
            // The anchor: x is where the text starts, its middle or its end.
            let width = if t.path.is_some() { 0.0 } else { meas(&text, &t.font, t.weight, t.size, t.letter_spacing)[0] };
            let x = match t.anchor {
                Anchor::Left => t.x + width / 2.0,
                Anchor::Centre => t.x,
                Anchor::Right => t.x - width / 2.0,
            };
            (m::ClipSource::Generator(g), t.name.clone(), x, t.y, t.start, t.duration)
        }
        Layer::Shape(s) => {
            let mut g = m::Generator::new(m::GeneratorKind::Shape);
            g.params.insert("vector".into(), m::Value::Text(serde_json::to_string(&s.doc).map_err(|e| e.to_string())?));
            (m::ClipSource::Generator(g), s.name.clone(), s.x, s.y, s.start, s.duration)
        }
    };
    let _ = w;
    let mut c = m::Clip::new(m::ClipId(id), m::LaneId(lane), at + micros(start), micros(dur).max(SEC / 30), source);
    c.name = name;
    c.transform.position = m::Vec2::new(x, y);
    // Text path start keys (a path's `start` given as keys).
    if let Layer::Text(t) = layer
        && let Some(Value::Object(keys)) = t.path.as_ref().and_then(|p| p.get("start"))
        && let Some(Value::Array(ks)) = keys.get("keys")
    {
        let track: Vec<m::Keyframe> = ks.iter().filter_map(|k| Some(m::Keyframe::new(micros(k.get(0)?.as_f64()?), m::Value::Scalar(k.get(1)?.as_f64()?), m::Interp::Linear))).collect();
        if !track.is_empty() {
            if let m::ClipSource::Generator(g) = &mut c.source {
                g.params.insert("path_start".into(), track[0].value.clone());
            }
            c.keyframes.insert("generator.path_start".into(), m::Track::new(track));
        }
    }
    Ok(c)
}

impl Session {
    pub(super) fn template(&mut self, op: &Value) -> Result<String, String> {
        if op.get("list").is_some() || op.get("search").is_some() || (op.get("id").is_none() && op.get("clip").is_none() && op.get("category").is_some()) {
            let found: Vec<&Template> = match op.get("search").and_then(Value::as_str) {
                Some(q) => studio_mograph::search(q),
                None => studio_mograph::all().iter().collect(),
            };
            let cat = op.get("category").and_then(Value::as_str).map(|c| c.to_ascii_lowercase());
            let lines: Vec<String> = found.into_iter().filter(|t| cat.as_ref().is_none_or(|c| serde_json::to_value(t.category).ok().and_then(|v| v.as_str().map(str::to_owned)).is_some_and(|s| s.contains(c.as_str())))).map(describe).collect();
            return Ok(if lines.is_empty() { "no templates match".into() } else { lines.join("\n") });
        }
        if let Some(id) = op.get("clip").and_then(Value::as_u64) {
            return self.refill(id as u32, op);
        }
        let tid = op.get("id").or(op.get("template")).and_then(Value::as_str).ok_or("template needs \"id\" (see {\"op\": \"template\", \"list\": true}), or \"clip\" to fill one in again")?;
        let t = studio_mograph::find(tid).ok_or_else(|| format!("no template \"{tid}\"; ids: {}", studio_mograph::all().iter().map(|t| t.id).collect::<Vec<_>>().join(", ")))?;
        let vals = values(t, op.get("controls"))?;
        let at = micros(f(op, "at").unwrap_or(0.0));
        let dur = f(op, "duration").unwrap_or(t.duration).max(0.5);
        let id = self.place_template(t, &vals, at, dur, op.get("name").and_then(Value::as_str))?;
        if f(op, "x").is_some() || f(op, "y").is_some() {
            if let Some(mut mc) = self.model_clip(id) {
                mc.transform.position = m::Vec2::new(mc.transform.position.x + f(op, "x").unwrap_or(0.0), mc.transform.position.y + f(op, "y").unwrap_or(0.0));
                let seq = self.doc.seq();
                let ok = self.doc.apply("Template", m::Edit::SetClip { seq, clip: Box::new(mc) });
                self.check(ok, "")?;
            }
        }
        let shown: Vec<String> = t.controls.iter().map(|c| format!("{}={}", c.id, vals.get(c.id).unwrap_or(&c.default))).collect();
        Ok(format!("template clip {id} ({}, {dur}s): {}; fill in again with {{\"op\": \"template\", \"clip\": {id}, \"controls\": {{…}}}}", t.name, shown.join(", ")))
    }

    /// Builds a template's layers into clips and merges them into one.
    fn place_template(&mut self, t: &Template, vals: &Map<String, Value>, at: i64, dur: f64, name: Option<&str>) -> Result<u32, String> {
        let (w, h) = (self.doc.project.width as f64, self.doc.project.height as f64);
        let meas = measure(h);
        let layers = t.build(vals, w, h, dur, &meas)?;
        let lanes_before: Vec<u32> = self.doc.project.lanes.iter().map(|l| l.id).collect();
        let mut ids = Vec::new();
        for layer in &layers {
            let (start, len) = (at + micros(layer.start()), micros(layer.duration()).max(SEC / 30));
            let lane = self.room(None, start, len, None)?;
            let id = self.doc.alloc_id();
            let c = layer_clip(layer, id, lane as u64, at, w, h, &meas)?;
            let seq = self.doc.seq();
            let ok = self.doc.apply("Template layer", m::Edit::InsertClip { seq, clip: Box::new(c) });
            self.check(ok, "")?;
            ids.push(m::ClipId(id));
        }
        let seq = self.doc.seq();
        let out = if ids.len() >= 2 {
            let new_sequence = m::SequenceId(self.doc.alloc_id());
            let new_clip = m::ClipId(self.doc.alloc_id());
            let ok = self.doc.apply("Template", m::Edit::CreateCompound { seq, clips: ids.clone(), name: name.unwrap_or(t.name).to_owned(), new_sequence, new_clip });
            self.check(ok, "")?;
            new_clip
        } else {
            ids[0]
        };
        // Lanes the layers needed and the merge emptied go again.
        let empty: Vec<u32> = self.doc.project.lanes.iter().filter(|l| !lanes_before.contains(&l.id)).filter(|l| !self.doc.project.clips.iter().any(|c| c.lane == l.id)).map(|l| l.id).collect();
        for l in empty {
            let _ = self.doc.apply("Template", m::Edit::RemoveLane { seq, lane: m::LaneId(l as u64) });
            let _ = self.doc.error.take();
        }
        let mut mc = self.model_clip(out.0 as u32).ok_or("the template clip went missing")?;
        mc.name = name.unwrap_or(t.name).to_owned();
        mc.meta.insert(META.into(), json!({"id": t.id, "controls": vals, "duration": dur}));
        let ok = self.doc.apply("Template", m::Edit::SetClip { seq, clip: Box::new(mc) });
        self.check(ok, "")?;
        Ok(out.0 as u32)
    }

    /// Fills a placed template in again.
    fn refill(&mut self, id: u32, op: &Value) -> Result<String, String> {
        let old = self.model_clip(id).ok_or_else(|| format!("no clip {id}"))?;
        let meta = old.meta.get(META).cloned().ok_or_else(|| format!("clip {id} is not a placed template"))?;
        let tid = meta.get("id").and_then(Value::as_str).unwrap_or("");
        let t = studio_mograph::find(tid).ok_or_else(|| format!("clip {id}'s template \"{tid}\" is not in this version"))?;
        let mut vals = meta.get("controls").and_then(Value::as_object).cloned().unwrap_or_default();
        vals.extend(values(t, op.get("controls"))?);
        let dur = f(op, "duration").unwrap_or_else(|| secs(old.duration)).max(0.5);
        let seq = self.doc.seq();
        let fresh = self.place_template(t, &vals, old.start, dur, Some(&old.name))?;
        let mut made = self.model_clip(fresh).ok_or("the rebuilt template went missing")?;
        // Keep what was done to the clip itself.
        made.id = old.id;
        made.lane = old.lane;
        made.transform = old.transform.clone();
        made.effects = old.effects.clone();
        made.grade = old.grade.clone();
        made.masks = old.masks.clone();
        made.keyframes = old.keyframes.clone();
        made.drivers = old.drivers.clone();
        made.motion = old.motion.clone();
        made.blend = old.blend;
        made.parent = old.parent.clone();
        made.label = old.label;
        let ok = self.doc.apply("Template", m::Edit::Lift { seq, clips: vec![old.id, m::ClipId(fresh as u64)] });
        self.check(ok, "")?;
        let ok = self.doc.apply("Template", m::Edit::InsertClip { seq, clip: Box::new(made) });
        self.check(ok, "")?;
        let shown: Vec<String> = t.controls.iter().map(|c| format!("{}={}", c.id, vals.get(c.id).unwrap_or(&c.default))).collect();
        Ok(format!("template clip {id} ({}) filled in: {}", t.name, shown.join(", ")))
    }

    pub(super) fn chart(&mut self, op: &Value) -> Result<String, String> {
        let kind = op.get("kind").and_then(Value::as_str).unwrap_or("bar").to_ascii_lowercase();
        let (tid, extra): (&str, Vec<(&str, Value)>) = match kind.as_str() {
            "bar" | "bars" | "column" => ("chart.bar", vec![]),
            "line" => ("chart.line", vec![]),
            "pie" => ("chart.pie", vec![("hole", json!(0))]),
            "donut" | "doughnut" | "ring" => ("chart.pie", vec![]),
            "map" | "route" | "places" => ("chart.map", vec![]),
            other => return Err(format!("chart kind \"{other}\": bar, line, pie, donut or map")),
        };
        let data = op.get("data").cloned().ok_or("chart needs \"data\": [[\"label\", value], …] (a map: [[\"place\", longitude, latitude], …])")?;
        if studio_mograph::data::rows(&data).is_empty() {
            return Err("chart data has no rows: [[\"label\", value], …], [{\"label\", \"value\"}], or text with \"label, value\" a line".into());
        }
        let mut controls = Map::new();
        controls.insert("data".into(), data);
        for (k, v) in extra {
            controls.insert(k.into(), v);
        }
        for k in ["title", "colour", "text", "prefix", "suffix", "decimals", "hole", "total_label", "route"] {
            if let Some(v) = op.get(k).or(if k == "colour" { op.get("color") } else { None }) {
                controls.insert(k.into(), v.clone());
            }
        }
        let mut t_op = json!({"op": "template", "id": tid, "controls": controls});
        for k in ["at", "duration", "x", "y", "name"] {
            if let Some(v) = op.get(k) {
                t_op[k] = v.clone();
            }
        }
        self.template(&t_op)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services;

    fn session() -> Session {
        Session::new(services::project::new_custom("Templates", 1280, 720, 30.0))
    }

    fn template_clips(s: &Session) -> Vec<m::Clip> {
        s.doc.model.active().clips.iter().filter(|c| c.meta.contains_key(META)).cloned().collect()
    }

    #[test]
    fn every_template_places_as_one_clip_and_fills_in_again() {
        let mut s = session();
        let lanes = s.doc.project.lanes.len();
        for (i, t) in studio_mograph::all().iter().enumerate() {
            s.edit(&[json!({"op": "template", "id": t.id, "at": i as f64 * 10.0})], t.id, false).unwrap_or_else(|e| panic!("{}: {e}", t.id));
        }
        let placed = template_clips(&s);
        assert_eq!(placed.len(), studio_mograph::all().len());
        assert!(s.doc.project.lanes.len() <= lanes + 2, "lanes pile up: {} → {}", lanes, s.doc.project.lanes.len());
        let lt = placed.iter().find(|c| c.meta[META]["id"] == "lower_third.bar").unwrap().clone();
        s.edit(&[json!({"op": "set", "clip": lt.id.0, "x": 40})], "nudge", false).unwrap();
        let r = s.edit(&[json!({"op": "template", "clip": lt.id.0, "controls": {"name": "Ada Lovelace", "accent": "orange"}})], "refill", false).unwrap();
        assert!(r.to_string().contains("Ada Lovelace"), "{r}");
        let again = s.model_clip(lt.id.0 as u32).unwrap();
        assert_eq!(again.transform.position.x, 40.0, "the nudge stays");
        assert!(again.meta[META]["controls"]["accent"].as_str().unwrap().starts_with('#'));
        let m::ClipSource::Sequence(r) = &again.source else { panic!("a merged clip") };
        let inner = s.doc.model.sequence(r.sequence).unwrap();
        assert!(inner.clips.iter().any(|c| matches!(&c.source, m::ClipSource::Generator(g) if g.plain_text() == "Ada Lovelace")));
        assert!(s.edit(&[json!({"op": "template", "id": "lower_third.bar", "controls": {"nope": 1}})], "bad", false).unwrap_err().contains("nope"));
        let list = s.edit(&[json!({"op": "template", "search": "news"})], "list", false).unwrap();
        assert!(list.to_string().contains("lower_third.news"));
    }

    #[test]
    fn charts_from_data() {
        let mut s = session();
        let r = s.edit(&[json!({"op": "chart", "kind": "bar", "data": [["Q1", 12], ["Q2", 19], ["Q3", 7]], "title": "Sales", "prefix": "$", "suffix": "k"})], "bars", false).unwrap();
        assert!(r.to_string().contains("Bar Chart"), "{r}");
        s.edit(&[json!({"op": "chart", "kind": "donut", "data": "Video, 46\nPhoto, 28\nAudio, 26", "at": 8})], "donut", false).unwrap();
        s.edit(&[json!({"op": "chart", "kind": "map", "data": [["Lisbon", -9.14, 38.72], ["Paris", 2.35, 48.86]], "at": 16})], "map", false).unwrap();
        assert_eq!(template_clips(&s).len(), 3);
        assert!(s.edit(&[json!({"op": "chart", "kind": "radar", "data": [1, 2]})], "bad", false).is_err());
    }
}
