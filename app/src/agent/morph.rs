// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The `morph` edit op: one outline really turning into another, its
//! points matched by `studio-morph` (equal resampling with corners kept,
//! winding matched, the start point that moves least, subpaths paired by
//! place and size, extras growing from a point).
//!
//! * A **shape layer** gets a `morph` item before its paint, its amount a
//!   clip parameter (`generator.morph`) keyed 0 → 1 on the timeline.
//! * A **vector art** clip's object morphs through the document's own
//!   `morph` op (keys on the clip's time).
//! * A **mask** becomes a path whose matched points are keyed from the old
//!   outline to the new one.

use super::{Session, f, micros};
use serde_json::{Value, json};
use studio_model as m;
use view::Time;

use crate::view;

/// A cubic Bézier loop of mask points (handles relative) as a dense polyline.
fn sample_loop(pts: &[m::PathPoint]) -> Vec<[f64; 2]> {
    let n = pts.len();
    let mut out = Vec::new();
    for i in 0..n {
        let (a, b) = (pts[i], pts[(i + 1) % n]);
        let p0 = [a.point.x, a.point.y];
        let p1 = [a.point.x + a.handle_out.x, a.point.y + a.handle_out.y];
        let p2 = [b.point.x + b.handle_in.x, b.point.y + b.handle_in.y];
        let p3 = [b.point.x, b.point.y];
        let straight = a.handle_out.x == 0.0 && a.handle_out.y == 0.0 && b.handle_in.x == 0.0 && b.handle_in.y == 0.0;
        let steps = if straight { 1 } else { 24 };
        for k in 0..steps {
            let t = k as f64 / steps as f64;
            let mt = 1.0 - t;
            let w = [mt * mt * mt, 3.0 * mt * mt * t, 3.0 * mt * t * t, t * t * t];
            out.push([w[0] * p0[0] + w[1] * p1[0] + w[2] * p2[0] + w[3] * p3[0], w[0] * p0[1] + w[1] * p1[1] + w[2] * p2[1] + w[3] * p3[1]]);
        }
    }
    out
}

/// A keyframe track from `(time, value)` keys eased by `ease` (any curve
/// name `animate` takes).
fn eased_track(keys: &[(Time, m::Value)], ease: &str) -> Result<m::Track, String> {
    let (interp, out_h, in_h, shape) = super::craft::curve(ease).ok_or_else(|| format!("unknown ease \"{ease}\" (linear, ease_in_out, ease_in, ease_out, smooth, hold, overshoot…)"))?;
    let mut ks = Vec::new();
    for (i, (t, v)) in keys.iter().enumerate() {
        let mut k = m::Keyframe::new(*t, v.clone(), interp);
        k.curve = shape;
        if let Some(o) = out_h {
            k.ease_out = o;
        }
        if i > 0
            && let Some(h) = in_h
        {
            k.ease_in = h;
        }
        ks.push(k);
    }
    Ok(m::Track::new(ks))
}

/// Shape items for a morph target given to a shape layer.
fn shape_targets(s: &Session, to: &Value) -> Result<Value, String> {
    match to {
        Value::Array(_) => Ok(to.clone()),
        Value::String(d) => Ok(json!([{"type": "path", "d": d}])),
        Value::Object(o) if o.contains_key("type") => Ok(json!([to.clone()])),
        Value::Object(o) if o.contains_key("d") => Ok(json!([{"type": "path", "d": o["d"].clone(), "position": o.get("position").cloned().unwrap_or(json!([0, 0]))}])),
        Value::Object(o) if o.contains_key("clip") => {
            let other = o["clip"].as_u64().ok_or("to.clip is a clip id")? as u32;
            let mc = s.model_clip(other).ok_or_else(|| format!("no clip {other}"))?;
            let m::ClipSource::Generator(g) = &mc.source else { return Err(format!("clip {other} is not a shape layer")) };
            let Some(m::Value::Text(src)) = g.params.get("vector") else { return Err(format!("clip {other} is not a shape layer")) };
            let doc = studio_vector::parse(src)?;
            let items = doc.get("items").cloned().unwrap_or(doc);
            let shapes: Vec<Value> = items.as_array().into_iter().flatten().filter(|it| matches!(it.get("type").and_then(Value::as_str), Some("rect" | "rectangle" | "ellipse" | "circle" | "polygon" | "star" | "line" | "arc" | "pie" | "path"))).cloned().collect();
            if shapes.is_empty() {
                return Err(format!("clip {other} has no shapes to morph into"));
            }
            Ok(json!(shapes))
        }
        Value::Object(o) if o.contains_key("text") => {
            // Type as outlines, centred on `position` (canvas pixels from the centre).
            let text = o["text"].as_str().ok_or("to.text is the words")?;
            let size = o.get("size").and_then(Value::as_f64).unwrap_or(200.0);
            let mut node = studio_vector_art::text::TextNode::point(studio_vector_art::raster::kurbo::Point::ZERO, text, size);
            node.char.font = o.get("font").and_then(Value::as_str).map(String::from);
            node.char.weight = o.get("weight").and_then(Value::as_u64).map(|w| w as u16);
            node.para.align = Some(studio_vector_art::text::Align::Center);
            let laid = studio_vector_art::text::layout(&node, &node.char, &node.para, &[Default::default()], &[]);
            let mut all = studio_vector_art::raster::kurbo::BezPath::new();
            for (_, p) in laid.parts {
                all.extend(p);
            }
            let b = studio_vector_art::raster::kurbo::Shape::bounding_box(&all);
            if b.width() <= 0.0 {
                return Err("no font could draw that text".into());
            }
            let at = o.get("position").and_then(|p| Some([p.get(0)?.as_f64()?, p.get(1)?.as_f64()?])).unwrap_or([0.0, 0.0]);
            let moved = studio_vector_art::raster::kurbo::Affine::translate((at[0] - b.center().x, at[1] - b.center().y)) * all;
            Ok(json!([{"type": "path", "d": moved.to_svg()}]))
        }
        other => Err(format!("morph to what? shape items ({{\"type\": \"polygon\", \"points\": 3, …}}), {{\"clip\": id}}, {{\"text\": \"…\"}} or SVG path data; got {other}")),
    }
}

impl Session {
    pub(super) fn morph_op(&mut self, op: &Value) -> Result<String, String> {
        let id = op.get("clip").and_then(Value::as_u64).ok_or("morph needs \"clip\"")? as u32;
        let mut mc = self.model_clip(id).ok_or_else(|| format!("no clip {id}"))?;
        let at = f(op, "at").map(micros).unwrap_or(mc.start);
        let local = (at - mc.start).clamp(0, mc.duration);
        let dur = micros(f(op, "duration").unwrap_or(1.0)).max(1);
        let ease = op.get("ease").and_then(Value::as_str).unwrap_or("ease_in_out").to_string();
        let to = op.get("to").ok_or("morph needs \"to\": the shape to become")?;
        // Masks.
        if let Some(n) = op.get("mask").and_then(Value::as_u64) {
            return self.morph_mask(id, mc, n as usize, to, local, dur, &ease);
        }
        let m::ClipSource::Generator(g) = &mut mc.source else { return Err(format!("clip {id} is not a shape layer or vector art (morph a mask with \"mask\")")) };
        if let Some(m::Value::Text(path)) = g.params.get("art").cloned() {
            // Vector art: the document's own morph, on the clip's time.
            let mut doc = studio_vector_art::interop::open(std::path::Path::new(&path))?;
            let from = match op.get("object") {
                Some(o) => o.clone(),
                None => {
                    let top = doc.layers.iter().rev().find_map(|l| l.children().and_then(|c| c.iter().rev().find(|n| n.visible)).map(|n| n.id)).ok_or("the art has nothing to morph")?;
                    json!(top)
                }
            };
            let secs = |t: Time| t as f64 / 1e6;
            let ctx = studio_vector_art::ops::Context { base_dir: std::path::Path::new(&path).parent().map(std::path::Path::to_path_buf), out_dir: None };
            let r = studio_vector_art::ops::apply(&mut doc, &json!({"op": "morph", "id": from, "to": to, "t": secs(local), "duration": secs(dur), "ease": ease.replace("smooth", "ease_in_out")}), &ctx)?;
            std::fs::write(&path, doc.to_json()).map_err(|e| format!("{path}: {e}"))?;
            return Ok(format!("vector art object {} morphs into {} from {:.2}s to {:.2}s (clip time)", r["id"], r["target"], secs(local), secs(local + dur)));
        }
        let Some(m::Value::Text(src)) = g.params.get("vector").cloned() else { return Err(format!("clip {id} is not a shape layer or vector art")) };
        let mut doc = studio_vector::parse(&src)?;
        let targets = shape_targets(self, to)?;
        // Each morph gets its own amount parameter.
        let mut name = "morph".to_string();
        let mut k = 2;
        while g.params.contains_key(&name) {
            name = format!("morph{k}");
            k += 1;
        }
        let item = json!({"type": "morph", "to": targets, "amount": format!("${name}")});
        let items = match doc.get_mut("items") {
            Some(Value::Array(a)) => a,
            _ => doc.as_array_mut().ok_or("a shape document is a list of items")?,
        };
        // Before the first paint, so the paint draws the morphing outline.
        let at_paint = items.iter().position(|it| matches!(it.get("type").and_then(Value::as_str), Some("fill" | "stroke"))).unwrap_or(items.len());
        items.insert(at_paint, item);
        g.params.insert("vector".into(), m::Value::Text(serde_json::to_string(&doc).map_err(|e| e.to_string())?));
        g.params.insert(name.clone(), m::Value::Scalar(0.0));
        let track = eased_track(&[(local, m::Value::Scalar(0.0)), (local + dur, m::Value::Scalar(1.0))], &ease)?;
        mc.keyframes.insert(format!("generator.{name}"), track);
        let seq = self.doc.seq();
        let ok = self.doc.apply("Morph", m::Edit::SetClip { seq, clip: Box::new(mc) });
        self.check(ok, "")?;
        Ok(format!("shape layer {id} morphs (generator.{name} keyed 0 → 1 from {:.2}s to {:.2}s, {ease}); retime or re-ease it with keys on generator.{name}", local as f64 / 1e6, (local + dur) as f64 / 1e6))
    }

    #[allow(clippy::too_many_arguments)]
    fn morph_mask(&mut self, id: u32, mut mc: m::Clip, n: usize, to: &Value, local: Time, dur: Time, ease: &str) -> Result<String, String> {
        let mk = mc.masks.get(n.max(1) - 1).cloned().ok_or_else(|| format!("clip {id} has {} mask(s)", mc.masks.len()))?;
        let shown = mc.mask_at(&mk, local);
        let from_path = crate::editor::pins::as_path(&mut mc, &shown).ok_or("a line or mirror mask has no outline to morph")?;
        // The target outline, in the same 0..1 layer coordinates.
        let target: Vec<[f64; 2]> = match to {
            Value::Object(o) if o.contains_key("mask") => {
                let other = o["mask"].as_u64().ok_or("to.mask is a mask number")? as usize;
                let om = mc.masks.get(other.max(1) - 1).cloned().ok_or("no such mask")?;
                let om = mc.mask_at(&om, local);
                sample_loop(&if om.shape == m::MaskShape::Path { om.path.clone() } else { om.outline().ok_or("that mask has no outline")? })
            }
            Value::Object(o) if o.contains_key("points") => o["points"].as_array().into_iter().flatten().filter_map(|p| Some([p.get(0)?.as_f64()?, p.get(1)?.as_f64()?])).collect(),
            Value::Object(o) if o.contains_key("shape") => {
                let mut t = shown.clone();
                t.shape = serde_json::from_value(o["shape"].clone()).map_err(|_| "to.shape is rectangle, ellipse, star or heart".to_string())?;
                for k in ["position", "size"] {
                    if let Some(p) = o.get(k).and_then(|p| Some(m::Vec2::new(p.get(0)?.as_f64()?, p.get(1)?.as_f64()?))) {
                        t.params.insert(k.into(), m::Value::Point(p));
                    }
                }
                if let Some(r) = o.get("rotation").and_then(Value::as_f64) {
                    t.params.insert("rotation".into(), m::Value::Scalar(r));
                }
                sample_loop(&t.outline().ok_or("that shape has no outline")?)
            }
            other => return Err(format!("a mask morphs to {{\"shape\": \"star\"…}}, {{\"mask\": n}} or {{\"points\": [[x, y], …]}} (0..1 of the layer); got {other}")),
        };
        if target.len() < 3 {
            return Err("the target outline needs three or more points".into());
        }
        let src = studio_morph::Contour::new(sample_loop(&from_path.path), true);
        let morph = studio_morph::Morph::new(&[src], &[studio_morph::Contour::new(target, true)], studio_morph::Options { points: 96, ..Default::default() });
        let (a, b) = morph.ends();
        let (a, b) = (&a[0].pts, &b[0].pts);
        let mut out = from_path;
        out.path = a.iter().map(|p| m::PathPoint { point: m::Vec2::new(p[0], p[1]), handle_in: m::Vec2::new(0.0, 0.0), handle_out: m::Vec2::new(0.0, 0.0) }).collect();
        let mid = out.id.0;
        // The old point keys no longer fit the new points.
        mc.keyframes.retain(|k, _| !k.starts_with(&format!("mask.{mid}.point.")));
        for (i, (p, q)) in a.iter().zip(b).enumerate() {
            let track = eased_track(&[(local, m::Value::Point(m::Vec2::new(p[0], p[1]))), (local + dur, m::Value::Point(m::Vec2::new(q[0], q[1])))], ease)?;
            mc.keyframes.insert(format!("mask.{mid}.point.{i}"), track);
        }
        mc.masks[n.max(1) - 1] = out;
        let seq = self.doc.seq();
        let ok = self.doc.apply("Morph mask", m::Edit::SetClip { seq, clip: Box::new(mc) });
        self.check(ok, "")?;
        Ok(format!("mask{n} of clip {id} morphs: {} matched points keyed from {:.2}s to {:.2}s ({ease})", a.len(), local as f64 / 1e6, (local + dur) as f64 / 1e6))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_loops_sample_curves_and_corners() {
        let sq = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)].map(|(x, y)| m::PathPoint { point: m::Vec2::new(x, y), handle_in: m::Vec2::new(0.0, 0.0), handle_out: m::Vec2::new(0.0, 0.0) });
        assert_eq!(sample_loop(&sq).len(), 4);
        let t = eased_track(&[(0, m::Value::Scalar(0.0)), (1_000_000, m::Value::Scalar(1.0))], "linear").unwrap();
        assert!(matches!(t.eval(500_000), Some(m::Value::Scalar(v)) if (v - 0.5).abs() < 1e-6));
    }
}
