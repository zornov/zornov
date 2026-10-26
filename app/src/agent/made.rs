// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Moves and designs an agent makes itself, kept in Studio's folder
//! (`Made/`) for every project:
//!
//! - a **move** is everything animated on a clip (its keyframes on any
//!   property, effect settings, text motion, motion presets), saved with
//!   `save_move` and put on other clips with the `use_move` op, stretched
//!   to their length if wanted, position and scale taken relative to each
//!   clip's own;
//! - a **design** is a group of clips (layers, timing, media, text),
//!   saved with `save_design` and placed anywhere with `use_design`, with
//!   new text if wanted.

use super::{Session, f, micros, secs};
use crate::view;
use serde_json::{Value, json};
use std::path::PathBuf;
use studio_model as m;

fn dir(kind: &str) -> PathBuf {
    studio_paths::studio().join("Made").join(kind)
}

fn file_name(name: &str) -> String {
    let s: String = name.trim().chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c.to_ascii_lowercase() } else { '-' }).collect();
    format!("{}.json", s.trim_matches('-'))
}

fn load(kind: &str, name: &str) -> Result<Value, String> {
    let path = dir(kind).join(file_name(name));
    let bytes = std::fs::read(&path).map_err(|_| {
        let have: Vec<String> = std::fs::read_dir(dir(kind)).map(|d| d.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().trim_end_matches(".json").to_owned()).collect()).unwrap_or_default();
        format!("no {kind} called \"{name}\"; made so far: {}", if have.is_empty() { "none".into() } else { have.join(", ") })
    })?;
    serde_json::from_slice(&bytes).map_err(|e| format!("{kind} \"{name}\" is damaged: {e}"))
}

fn save(kind: &str, name: &str, v: &Value) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir(kind)).map_err(|e| e.to_string())?;
    let path = dir(kind).join(file_name(name));
    std::fs::write(&path, serde_json::to_vec_pretty(v).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    Ok(path)
}

/// How a saved track's values relate to the clip it is put on.
fn relation(path: &str) -> Option<&'static str> {
    match path {
        "transform.scale_x" | "transform.scale_y" | "transform.opacity" => Some("factor"),
        "transform.position" | "transform.rotation" => Some("offset"),
        _ => None,
    }
}

fn scalar(v: &m::Value) -> Option<f64> {
    match v {
        m::Value::Scalar(x) | m::Value::Angle(x) => Some(*x),
        _ => None,
    }
}

impl Session {
    /// `save_move`: what is animated on `clip`, under `name`.
    pub(super) fn save_move(&mut self, a: &Value) -> Result<Value, String> {
        let id = a.get("clip").and_then(Value::as_u64).ok_or("save_move needs \"clip\"")? as u32;
        let name = a.get("name").and_then(Value::as_str).ok_or("save_move needs \"name\"")?;
        let mc = self.model_clip(id).ok_or_else(|| format!("no clip {id}"))?;
        let mut tracks = Vec::new();
        for (path, track) in &mc.keyframes {
            // Effects by kind and place, not by this clip's ids.
            let generic = if let Some(rest) = path.strip_prefix("effect.") {
                let (eid, param) = rest.split_once('.').ok_or("bad effect path")?;
                let e = mc.effects.iter().find(|e| e.id.0.to_string() == eid).ok_or("effect gone")?;
                let nth = mc.effects.iter().filter(|x| x.kind == e.kind).position(|x| x.id == e.id).unwrap_or(0) + 1;
                format!("effect:{}#{nth}.{param}", e.kind)
            } else {
                path.clone()
            };
            let rel = relation(path);
            let base = mc.get_property(path);
            let keys: Vec<Value> = track
                .keys
                .iter()
                .map(|k| {
                    let v = match (rel, &k.value, &base) {
                        (Some("factor"), v, Some(b)) => json!(scalar(v).unwrap_or(1.0) / scalar(b).filter(|b| b.abs() > 1e-9).unwrap_or(1.0)),
                        (Some("offset"), m::Value::Point(p), Some(m::Value::Point(b))) => json!([p.x - b.x, p.y - b.y]),
                        (Some("offset"), v, Some(b)) => json!(scalar(v).unwrap_or(0.0) - scalar(b).unwrap_or(0.0)),
                        (_, v, _) => serde_json::to_value(v).unwrap_or(Value::Null),
                    };
                    json!({"t": secs(k.time), "v": v, "interp": k.interp, "in": [k.ease_in.x, k.ease_in.y], "out": [k.ease_out.x, k.ease_out.y]})
                })
                .collect();
            tracks.push(json!({"path": generic, "relation": rel, "keys": keys}));
        }
        let effects: Vec<Value> = mc.effects.iter().map(|e| json!({"kind": e.kind, "params": e.params})).collect();
        let text_motion = match &mc.source {
            m::ClipSource::Generator(g) => g.params.get("motion").cloned(),
            _ => None,
        };
        let motion = serde_json::to_value(&mc.motion).unwrap_or(Value::Null);
        if tracks.is_empty() && text_motion.is_none() && motion.as_object().is_none_or(|o| o.is_empty()) {
            return Err(format!("clip {id} has nothing animated to save (no keys, text motion or motion presets)"));
        }
        let mv = json!({
            "name": name,
            "description": a.get("description").and_then(Value::as_str).unwrap_or(""),
            "duration": secs(mc.duration),
            "tracks": tracks,
            "effects": effects,
            "text_motion": text_motion,
            "motion": motion,
        });
        let path = save("moves", name, &mv)?;
        Ok(json!({"saved": name, "tracks": mv["tracks"].as_array().map_or(0, |t| t.len()), "path": path.display().to_string(), "use": "edit op {\"op\": \"use_move\", \"move\": name, \"clips\": [...], \"fit\": true}"}))
    }

    /// `use_move`: a saved move onto one clip.
    pub(super) fn use_move(&mut self, op: &Value) -> Result<String, String> {
        let id = op.get("clip").and_then(Value::as_u64).ok_or("use_move needs \"clip\" (or \"clips\")")? as u32;
        let name = op.get("move").and_then(Value::as_str).ok_or("use_move needs \"move\" (a name from made)")?;
        let mv = load("moves", name)?;
        let c = self.clip(id)?;
        let src_dur = mv["duration"].as_f64().unwrap_or(1.0).max(1e-3);
        let fit = op.get("fit").and_then(Value::as_bool).unwrap_or(true);
        let stretch = if fit { secs(c.dur) / src_dur } else { 1.0 };
        let shift = f(op, "at").unwrap_or(0.0);
        let seq = self.doc.seq();
        let cid = m::ClipId(id as u64);
        let mut applied = 0;
        for tr in mv["tracks"].as_array().cloned().unwrap_or_default() {
            let generic = tr["path"].as_str().unwrap_or("").to_owned();
            // Find (or add) the effect a track belongs to.
            let path = if let Some(rest) = generic.strip_prefix("effect:") {
                let (head, param) = rest.split_once('.').ok_or("bad saved path")?;
                let (kind, nth) = head.split_once('#').map_or((head, 1), |(k, n)| (k, n.parse().unwrap_or(1)));
                let mut have: Vec<m::EffectId> = self.model_clip(id).map(|c| c.effects.iter().filter(|e| e.kind == kind).map(|e| e.id).collect()).unwrap_or_default();
                while have.len() < nth {
                    let saved = mv["effects"].as_array().and_then(|es| es.iter().filter(|e| e["kind"] == kind).nth(have.len()).cloned());
                    let eid = m::EffectId(self.doc.alloc_id());
                    let mut e = m::Effect::new(eid, kind.to_owned());
                    if let Some(p) = saved.and_then(|s| serde_json::from_value::<m::ParamMap>(s["params"].clone()).ok()) {
                        e.params = p;
                    }
                    let ok = self.doc.apply("Use move", m::Edit::AddEffect { seq, clip: cid, stack: m::Stack::Video, effect: e, index: None });
                    self.check(ok, "")?;
                    have.push(eid);
                }
                format!("effect.{}.{param}", have[nth - 1].0)
            } else {
                generic.clone()
            };
            let mc = self.model_clip(id).ok_or("clip gone")?;
            if path.starts_with("grade.") && mc.grade.is_none() {
                let ok = self.doc.apply("Use move", m::Edit::SetGrade { seq, clip: cid, grade: Some(m::Grade { enabled: true, ..Default::default() }) });
                self.check(ok, "")?;
            }
            let base = super::props::value_or_default(&self.model_clip(id).ok_or("clip gone")?, &path);
            for t in mc.keyframes.get(&path).map(|t| t.keys.iter().map(|k| k.time).collect::<Vec<_>>()).unwrap_or_default() {
                let ok = self.doc.apply("Use move", m::Edit::RemoveKeyframe { seq, clip: cid, property: path.clone(), time: t });
                self.check(ok, "")?;
            }
            for k in tr["keys"].as_array().cloned().unwrap_or_default() {
                let t = k["t"].as_f64().unwrap_or(0.0) * stretch + shift;
                if t < -1e-6 || t > secs(c.dur) + 1e-6 {
                    continue;
                }
                let value: m::Value = match (tr["relation"].as_str(), &base) {
                    (Some("factor"), Some(b)) => m::Value::Scalar(k["v"].as_f64().unwrap_or(1.0) * scalar(b).unwrap_or(1.0)),
                    (Some("offset"), Some(m::Value::Point(b))) => {
                        let a = k["v"].as_array().cloned().unwrap_or_default();
                        m::Value::Point(m::Vec2 { x: b.x + a.first().and_then(Value::as_f64).unwrap_or(0.0), y: b.y + a.get(1).and_then(Value::as_f64).unwrap_or(0.0) })
                    }
                    (Some("offset"), Some(b)) => m::Value::Angle(scalar(b).unwrap_or(0.0) + k["v"].as_f64().unwrap_or(0.0)),
                    _ => serde_json::from_value(k["v"].clone()).map_err(|e| format!("saved key not readable: {e}"))?,
                };
                let mut key = m::Keyframe::new(micros(t), value, serde_json::from_value(k["interp"].clone()).unwrap_or_default());
                let h = |v: &Value| m::Handle { x: v[0].as_f64().unwrap_or(0.42), y: v[1].as_f64().unwrap_or(0.0) };
                key.ease_in = h(&k["in"]);
                key.ease_out = h(&k["out"]);
                let ok = self.doc.apply("Use move", m::Edit::SetKeyframe { seq, clip: cid, property: path.clone(), key });
                self.check(ok, "")?;
            }
            applied += 1;
        }
        // Text motion and motion presets.
        let mut mc = self.model_clip(id).ok_or("clip gone")?;
        let mut changed = false;
        if let (Some(tm), m::ClipSource::Generator(g)) = (mv.get("text_motion").filter(|v| !v.is_null()), &mut mc.source) {
            if g.kind == m::GeneratorKind::Text {
                if let Ok(v) = serde_json::from_value::<m::Value>(tm.clone()) {
                    g.params.insert("motion".into(), v);
                    changed = true;
                }
            }
        }
        if let Ok(motion) = serde_json::from_value::<m::Motion>(mv["motion"].clone()) {
            if motion != m::Motion::default() {
                mc.motion = motion;
                changed = true;
            }
        }
        if changed {
            let ok = self.doc.apply("Use move", m::Edit::SetClip { seq, clip: Box::new(mc) });
            self.check(ok, "")?;
        }
        Ok(format!("{name} on clip {id}: {applied} track(s){}", if fit && (stretch - 1.0).abs() > 1e-3 { format!(", timed to its {:.2}s", secs(c.dur)) } else { String::new() }))
    }

    /// `save_design`: a group of clips (layers, relative timing, media,
    /// text) under `name`.
    pub(super) fn save_design(&mut self, a: &Value) -> Result<Value, String> {
        let name = a.get("name").and_then(Value::as_str).ok_or("save_design needs \"name\"")?;
        let ids: Vec<u32> = match a.get("clips") {
            Some(Value::Array(v)) => v.iter().filter_map(Value::as_u64).map(|x| x as u32).collect(),
            _ => return Err("save_design needs \"clips\"".into()),
        };
        let p = self.doc.project.clone();
        let lane_index = |l: u32| p.lanes.iter().position(|x| x.id == l).unwrap_or(0);
        let start = ids.iter().filter_map(|&i| p.clip(i)).map(|c| c.start).min().ok_or("none of those clips exist")?;
        let mut parts = Vec::new();
        for &i in &ids {
            let mc = self.model_clip(i).ok_or_else(|| format!("no clip {i}"))?;
            if matches!(mc.source, m::ClipSource::Sequence(_)) {
                return Err(format!("clip {i} is merged; unmerge it first, then save its parts (merge again after use_design)"));
            }
            let media = match &mc.source {
                m::ClipSource::Media(r) => self.doc.project.media(r.media.0 as u32).and_then(|mi| mi.path.clone()).map(|p| p.display().to_string()),
                _ => None,
            };
            let vc = p.clip(i).ok_or("clip gone")?;
            parts.push(json!({"clip": mc, "start": secs(vc.start - start), "layer": lane_index(vc.lane), "media_path": media}));
        }
        let end = ids.iter().filter_map(|&i| p.clip(i)).map(|c| c.end()).max().unwrap_or(start);
        let d = json!({"name": name, "description": a.get("description").and_then(Value::as_str).unwrap_or(""), "duration": secs(end - start), "parts": parts});
        let path = save("designs", name, &d)?;
        Ok(json!({"saved": name, "parts": ids.len(), "path": path.display().to_string(), "use": "edit op {\"op\": \"use_design\", \"design\": name, \"at\": t, \"text\": {\"OLD\": \"NEW\"}}"}))
    }

    /// `use_design`: a saved design placed at `at`, layers kept in order,
    /// optionally moved (`x`, `y`) and with text swapped (`text`).
    pub(super) fn use_design(&mut self, op: &Value) -> Result<String, String> {
        let name = op.get("design").and_then(Value::as_str).ok_or("use_design needs \"design\" (a name from made)")?;
        let d = load("designs", name)?;
        let at = micros(f(op, "at").ok_or("use_design needs \"at\"")?);
        let (dx, dy) = (f(op, "x").unwrap_or(0.0), f(op, "y").unwrap_or(0.0));
        let swaps: Vec<(String, String)> = op.get("text").and_then(Value::as_object).map(|o| o.iter().filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_owned()))).collect()).unwrap_or_default();
        let mut parts = d["parts"].as_array().cloned().unwrap_or_default();
        // Top layer first: each next one goes on a lane drawn under it.
        parts.sort_by_key(|p| p["layer"].as_u64().unwrap_or(0));
        let mut above: Option<u32> = None;
        let mut made = Vec::new();
        for part in parts {
            let mut c: m::Clip = serde_json::from_value(part["clip"].clone()).map_err(|e| format!("design part not readable: {e}"))?;
            let start = at + micros(part["start"].as_f64().unwrap_or(0.0));
            // Media from the file it came from (imported once).
            if let (m::ClipSource::Media(r), Some(path)) = (&mut c.source, part["media_path"].as_str()) {
                let have = self.doc.project.media.iter().find(|mi| mi.path.as_ref().is_some_and(|p| p.to_string_lossy() == path)).map(|mi| mi.id);
                let id = match have {
                    Some(id) => id,
                    None => *crate::services::project::import_media(&mut self.doc, &[PathBuf::from(path)]).ids.first().ok_or_else(|| format!("{path} could not be imported"))?,
                };
                r.media = m::MediaId(id as u64);
            }
            if let m::ClipSource::Generator(g) = &mut c.source {
                for run in g.text.iter_mut() {
                    for (from, to) in &swaps {
                        if run.text.contains(from.as_str()) {
                            run.text = run.text.replace(from.as_str(), to);
                        }
                    }
                }
            }
            let is_audio = matches!(&c.source, m::ClipSource::Media(r) if !r.video && r.audio);
            let lane = if is_audio {
                let l = super::craft::lane_of(&self.doc.project, &json!("sound"), None)?;
                self.room(Some(l), start, c.duration, None)?
            } else {
                self.room(None, start, c.duration, above)?
            };
            if !is_audio {
                above = Some(lane);
            }
            c.id = m::ClipId(self.doc.alloc_id());
            c.lane = m::LaneId(lane as u64);
            c.start = start;
            c.link = None;
            c.group = None;
            c.transform.position.x += dx;
            c.transform.position.y += dy;
            if let Some(tr) = c.keyframes.get_mut("transform.position") {
                for k in tr.keys.iter_mut() {
                    if let m::Value::Point(p) = &mut k.value {
                        p.x += dx;
                        p.y += dy;
                    }
                }
            }
            for e in c.effects.iter_mut() {
                e.id = m::EffectId(self.doc.alloc_id());
            }
            // Keyframe paths name effects by id: rename them to the new ids.
            let old_effects: Vec<m::Effect> = serde_json::from_value::<m::Clip>(part["clip"].clone()).map(|o| o.effects).unwrap_or_default();
            let renames: Vec<(String, String)> = old_effects.iter().zip(c.effects.iter()).map(|(o, n)| (format!("effect.{}.", o.id.0), format!("effect.{}.", n.id.0))).collect();
            let keys = std::mem::take(&mut c.keyframes);
            for (path, track) in keys {
                let p = renames.iter().find(|(o, _)| path.starts_with(o.as_str())).map(|(o, n)| path.replacen(o.as_str(), n, 1)).unwrap_or(path);
                c.keyframes.insert(p, track);
            }
            let seq = self.doc.seq();
            let ok = self.doc.apply("Use design", m::Edit::InsertClip { seq, clip: Box::new(c.clone()) });
            self.check(ok, "")?;
            made.push(c.id.0);
        }
        let _: Option<view::Time> = None;
        Ok(format!("{name} at {:.2}s as clips {made:?}", secs(at)))
    }

    /// `made`: the moves and designs saved so far.
    pub(super) fn made(&self) -> Value {
        let list = |kind: &str| -> Vec<Value> {
            std::fs::read_dir(dir(kind))
                .map(|d| {
                    d.filter_map(|e| e.ok())
                        .filter_map(|e| std::fs::read(e.path()).ok())
                        .filter_map(|b| serde_json::from_slice::<Value>(&b).ok())
                        .map(|v| json!({"name": v["name"], "description": v["description"], "duration": v["duration"]}))
                        .collect()
                })
                .unwrap_or_default()
        };
        json!({"moves": list("moves"), "designs": list("designs"), "folder": studio_paths::studio().join("Made").display().to_string()})
    }
}
