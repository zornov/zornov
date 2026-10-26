// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Where things are on the canvas, for agents: the same geometry the
//! editor's handles use (`editor::xform`). A layer's box, what is under a
//! point, its anchor moved by name with the picture staying put, and
//! `set` at a moment, which keys an animated property there (as dragging
//! it on the canvas does) instead of being outvoted by its keys.

use super::{Session, f, micros, secs};
use crate::editor::xform;
use crate::view::{self, SEC};
use serde_json::{Value, json};
use studio_model as m;

fn r1(v: f32) -> f64 {
    (v as f64 * 10.0).round() / 10.0
}

impl Session {
    fn frame_len(&self) -> view::Time {
        (SEC as f64 / (self.doc.project.fps as f64).max(1.0)) as view::Time
    }

    fn geo_at(&self, id: u32, t: view::Time) -> Result<xform::Geo, String> {
        xform::geo(&self.doc.project, &self.doc.model, id, t).ok_or_else(|| format!("clip {id} draws nothing on the canvas at {:.2}s (a sound, an adjustment layer, or empty text)", secs(t)))
    }

    /// Clip `id` on screen at `t`: its box (canvas pixels from the centre,
    /// y down), corners, anchor point and placement.
    pub(super) fn on_screen(&self, id: u32, t: view::Time) -> Result<Value, String> {
        let g = self.geo_at(id, t)?;
        let q = g.quad();
        let (xs, ys): (Vec<f32>, Vec<f32>) = q.iter().map(|p| (p.x, p.y)).unzip();
        let mn = |v: &[f32]| v.iter().copied().fold(f32::MAX, f32::min);
        let mx = |v: &[f32]| v.iter().copied().fold(f32::MIN, f32::max);
        let (w, h) = (self.doc.project.width as f32 / 2.0, self.doc.project.height as f32 / 2.0);
        let b = [mn(&xs), mn(&ys), mx(&xs), mx(&ys)];
        let visible = !(b[2] < -w || b[0] > w || b[3] < -h || b[1] > h);
        Ok(json!({
            "at": secs(t),
            "box": b.map(r1),
            "corners": q.iter().map(|p| [r1(p.x), r1(p.y)]).collect::<Vec<_>>(),
            "anchor_point": [r1(g.xf.pos.x), r1(g.xf.pos.y)],
            "anchor": [r1(g.xf.anchor.x * g.fit()), r1(g.xf.anchor.y * g.fit())],
            "scale_x": r1(g.xf.sx * 100.0), "scale_y": r1(g.xf.sy * 100.0), "rotation": r1(g.xf.rot),
            "on_canvas": visible,
        }))
    }

    /// `pick`: the layers under a canvas point at a moment, topmost first.
    pub(super) fn pick(&self, a: &Value) -> Result<Value, String> {
        let (x, y) = (f(a, "x").ok_or("pick needs \"x\" and \"y\" (canvas pixels from the centre, y down)")?, f(a, "y").ok_or("pick needs \"y\"")?);
        let t = micros(f(a, "at").unwrap_or(0.0));
        let p = egui_point(x, y);
        let stack = self.doc.project.visual_stack_at(t);
        let hits: Vec<Value> = stack
            .iter()
            .filter_map(|c| xform::geo(&self.doc.project, &self.doc.model, c.id, t).filter(|g| g.contains(p)).map(|_| json!({"clip": c.id, "name": c.name})))
            .collect();
        Ok(json!({"at": secs(t), "point": [x, y], "layers": hits}))
    }

    /// `set` with `anchor` or `at`. The anchor moves without the picture
    /// moving; with `at`, an animated property gets a key at that moment
    /// and a still one its value. Whatever is left goes through `set`.
    pub(super) fn set_staged(&mut self, id: u32, op: &Value) -> Result<String, String> {
        let vc = self.clip(id)?;
        let at = f(op, "at").map(micros);
        if let Some(t) = at {
            if t < vc.start || t > vc.start + vc.dur {
                return Err(format!("\"at\" {:.2}s is outside clip {id} ({:.2}–{:.2}s)", secs(t), secs(vc.start), secs(vc.start + vc.dur)));
            }
        }
        let t = at.unwrap_or(vc.start);
        let local = t - vc.start;
        let frame = self.frame_len();
        let mut said = Vec::new();
        let mut rest = op.as_object().cloned().unwrap_or_default();
        rest.remove("at");
        if let Some(spec) = rest.remove("anchor") {
            let g = self.geo_at(id, t)?;
            let point = match &spec {
                Value::String(s) => {
                    let (u, v) = xform::named_point(s).ok_or_else(|| format!("anchor \"{s}\": centre, top_left, top, top_right, left, right, bottom_left, bottom, bottom_right, or [x, y] pixels from the layer's centre"))?;
                    g.content_point(u, v)
                }
                Value::Array(a) if a.len() == 2 => {
                    let (x, y) = (a[0].as_f64().unwrap_or(0.0) as f32, a[1].as_f64().unwrap_or(0.0) as f32);
                    g.content_point(0.0, 0.0) + egui_point(x as f64, y as f64) / g.fit().max(1e-6)
                }
                _ => return Err("anchor is a name (top_left, centre…) or [x, y] pixels from the layer's centre".into()),
            };
            let mc = self.model_clip(id).ok_or_else(|| format!("no clip {id}"))?;
            let moved = xform::re_anchored(&g, &mc, point - g.source() / 2.0, local, frame);
            let seq = self.doc.seq();
            let ok = self.doc.apply("Anchor point", m::Edit::SetClip { seq, clip: Box::new(moved) });
            self.check(ok, "")?;
            said.push(format!("anchor at {spec} (the picture stays put; scale and rotation turn about it)"));
        }
        if at.is_some() {
            // Each animated property named here: a key at that moment.
            let mut mc = self.model_clip(id).ok_or_else(|| format!("no clip {id}"))?;
            let mut keyed = Vec::new();
            for (k, v) in rest.clone() {
                if matches!(k.as_str(), "op" | "clip") || super::args::is_note(&k) {
                    continue;
                }
                let name = if k == "colour" { "colour".to_owned() } else { k.clone() };
                let Ok(prop) = super::props::resolve(&mc, &name) else { continue };
                if !prop.paths.iter().any(|p| mc.keyframes.get(p).is_some_and(|tr| !tr.keys.is_empty())) {
                    continue;
                }
                for path in &prop.paths {
                    let cur = mc.property_at(path, local).or_else(|| super::props::value_or_default(&mc, path));
                    let val = super::props::to_model(&prop, path, &v, cur.as_ref())?;
                    xform::put(&mut mc, path, val, local, frame);
                }
                rest.remove(&k);
                keyed.push(k);
            }
            if !keyed.is_empty() {
                let seq = self.doc.seq();
                let ok = self.doc.apply("Key", m::Edit::SetClip { seq, clip: Box::new(mc) });
                self.check(ok, "")?;
                said.push(format!("keyed {} at {:.2}s", keyed.join(", "), secs(t)));
            }
        }
        // Anything left is an ordinary set.
        if rest.keys().any(|k| !matches!(k.as_str(), "op" | "clip") && !super::args::is_note(k)) {
            said.push(self.op(&Value::Object(rest))?);
        }
        Ok(if said.is_empty() { format!("clip {id} unchanged") } else { format!("clip {id}: {}", said.join("; ")) })
    }
}

fn egui_point(x: f64, y: f64) -> eframe::egui::Vec2 {
    eframe::egui::Vec2::new(x as f32, y as f32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::Doc;

    fn card() -> (Session, u32) {
        let mut s = Session::new(Doc::from_view(&view::demo()));
        let r = s.edit(&[json!({"op": "card", "colour": "#ff6b5b", "at": 0.0, "duration": 4.0, "width": 300, "height": 180, "x": 100, "y": 50})], "card", false).unwrap();
        let id = r["changed"]["added"][0]["id"].as_u64().unwrap() as u32;
        (s, id)
    }

    #[test]
    fn re_anchoring_keeps_the_picture_and_turns_about_the_corner() {
        let (mut s, id) = card();
        let before = s.on_screen(id, SEC).unwrap();
        assert_eq!(before["box"], json!([-50.0, -40.0, 250.0, 140.0]));
        s.edit(&[json!({"op": "set", "clip": id, "anchor": "top_left"})], "a", false).unwrap();
        let after = s.on_screen(id, SEC).unwrap();
        assert_eq!(after["box"], before["box"]);
        assert_eq!(after["anchor_point"], json!([-50.0, -40.0]));
        // Turned a quarter about its top-left corner, the corner stays.
        s.edit(&[json!({"op": "set", "clip": id, "rotation": 90})], "r", false).unwrap();
        let turned = s.on_screen(id, SEC).unwrap();
        assert_eq!(turned["anchor_point"], json!([-50.0, -40.0]));
        assert_eq!(turned["box"], json!([-230.0, -40.0, -50.0, 260.0]));
        // And something is there to pick.
        let p = s.pick(&json!({"x": -100, "y": 100, "at": 1.0})).unwrap();
        assert_eq!(p["layers"][0]["clip"], json!(id));
    }

    #[test]
    fn set_at_a_moment_keys_what_is_animated_and_sets_what_is_not() {
        let (mut s, id) = card();
        s.edit(&[json!({"op": "animate", "clip": id, "property": "rotation", "keys": [[0, 0], [4, 90]]})], "k", false).unwrap();
        let r = s.edit(&[json!({"op": "set", "clip": id, "at": 2.0, "rotation": 10, "opacity": 50})], "s", false).unwrap();
        assert!(r["notes"][0].as_str().unwrap().contains("keyed rotation"), "{r}");
        let mc = s.model_clip(id).unwrap();
        assert_eq!(mc.keyframes["transform.rotation"].keys.len(), 3);
        assert_eq!(mc.property_at("transform.rotation", 2 * SEC), Some(m::Value::Angle(10.0)));
        assert_eq!(mc.get_property("transform.opacity"), Some(m::Value::Scalar(0.5)));
        // Every key at a moment eases the same way.
        s.edit(&[json!({"op": "keys", "clip": id, "property": "all", "curve": [{"t": 0, "curve": "snap"}]})], "c", false).unwrap();
        let mc = s.model_clip(id).unwrap();
        let tr = &mc.keyframes["transform.rotation"];
        assert_eq!(super::super::props::curve_name(&tr.keys[0], tr.keys.get(1)), "snap");
    }
}
