// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Working with layers the way an editor does, without taking things apart
//! to change them:
//!
//! * **Inside merged clips.** Any op on a clip inside a merged clip runs
//!   in place (its id is enough), and `"inside": merged` puts new things
//!   in one. Times inside are the merged clip's own (0 = its start).
//!   `inspect` on a merged clip lists what it holds.
//! * **flatten / unflatten.** A merged clip rendered once to a video file
//!   that plays in its place (a heavy scene plays smoothly, and can be
//!   used as footage anywhere); `unflatten` brings the live layers back.
//! * **align / distribute.** Lines clips up on the canvas or on each
//!   other, or spaces them evenly.
//! * **parent** and **matte**: a clip follows another's movement; a clip
//!   shows only through the one above it (see `services::layers`).

use super::{Content, Session, f, secs};
use serde_json::{Value, json};
use studio_model as m;

impl Session {
    /// The sequence that holds clip `id`: the timeline, or the inside of
    /// a merged clip that is on it (directly or inside another). Leftovers
    /// of merged clips that were opened or deleted do not count.
    pub(super) fn sequence_of(&self, id: u64) -> Option<m::SequenceId> {
        let model = &self.doc.model;
        let mut reach = vec![self.doc.seq()];
        let mut i = 0;
        while i < reach.len() {
            if let Some(s) = model.sequence(reach[i]) {
                if s.clip(m::ClipId(id)).is_some() {
                    return Some(s.id);
                }
                for c in &s.clips {
                    if let m::ClipSource::Sequence(r) = &c.source {
                        if !reach.contains(&r.sequence) {
                            reach.push(r.sequence);
                        }
                    }
                }
            }
            i += 1;
        }
        None
    }

    /// What a merged clip holds (its own sequence).
    pub(super) fn merged_sequence(&self, id: u64) -> Option<m::SequenceId> {
        let c = self.model_clip(id as u32)?;
        match &c.source {
            m::ClipSource::Sequence(r) => Some(r.sequence),
            _ => None,
        }
    }

    /// Runs `f` with `seq` as the sequence being edited, then switches back
    /// whatever happens.
    pub(super) fn within<R>(&mut self, seq: m::SequenceId, f: impl FnOnce(&mut Self) -> R) -> R {
        let was = self.doc.focus_sequence(seq);
        let r = f(self);
        self.doc.focus_sequence(was);
        r
    }

    /// An op aimed inside a merged clip: by `"inside"`, or by naming clips
    /// that live there. `None` when it is an ordinary op.
    pub(super) fn route_inside(&mut self, op: &Value) -> Option<Result<String, String>> {
        let active = self.doc.seq();
        let (target, op) = if let Some(inside) = op.get("inside") {
            let id = match inside.as_u64() {
                Some(id) => id,
                None => return Some(Err("inside is a merged clip's id".into())),
            };
            let Some(seq) = self.merged_sequence(id) else { return Some(Err(format!("clip {id} is not a merged clip; inside works on merged clips"))) };
            let mut o = op.clone();
            if let Some(map) = o.as_object_mut() {
                map.remove("inside");
            }
            (seq, o)
        } else {
            let mut ids: Vec<u64> = Vec::new();
            if let Some(id) = op.get("clip").and_then(Value::as_u64) {
                ids.push(id);
            }
            if let Some(Value::Array(cs)) = op.get("clips") {
                ids.extend(cs.iter().filter_map(Value::as_u64));
            }
            if ids.is_empty() || ids.iter().any(|id| self.doc.project.clip(*id as u32).is_some()) {
                return None;
            }
            let seqs: std::collections::BTreeSet<u64> = ids.iter().filter_map(|id| self.sequence_of(*id)).map(|s| s.0).collect();
            match seqs.len() {
                1 => {
                    let seq = m::SequenceId(*seqs.iter().next().expect("one"));
                    if seq == active {
                        return None;
                    }
                    (seq, op.clone())
                }
                0 => return None,
                _ => return Some(Err("those clips live in different merged clips; work on one at a time".into())),
            }
        };
        let holder = self.doc.model.sequences.iter().flat_map(|s| s.clips.iter()).find(|c| matches!(&c.source, m::ClipSource::Sequence(r) if r.sequence == target)).map(|c| c.id.0);
        let r = self.within(target, |s| s.op(&op));
        Some(r.map(|said| format!("{said} (inside merged clip {})", holder.map_or("?".into(), |h| h.to_string()))))
    }

    /// The layers a merged clip holds, for `inspect`.
    pub(super) fn contents(&self, id: u64) -> Option<Value> {
        let seq = self.merged_sequence(id)?;
        let inner = self.doc.model.sequence(seq)?;
        let view = crate::bridge::view_of(&{
            let mut p = self.doc.model.clone();
            p.active_sequence = seq;
            p
        }, 0);
        let _ = inner;
        Some(json!({
            "note": "edit these in place by their ids (set, animate, effect, text…): no need to unmerge; \"inside\": this clip puts new ones in. Times inside are this clip's own (0 = its start).",
            "duration": secs(view.duration()),
            "layers": view.clips.iter().map(|c| {
                let mut o = json!({"id": c.id, "name": c.name, "kind": format!("{:?}", c.kind).to_lowercase(), "start": secs(c.start), "dur": secs(c.dur)});
                if !c.text.is_empty() {
                    o["text"] = json!(c.text.chars().take(60).collect::<String>());
                }
                o
            }).collect::<Vec<_>>(),
        }))
    }

    /// `flatten`: a merged clip rendered once to a video file, played in
    /// its place with its own position, animation and effects kept.
    /// `unflatten` puts the live layers back.
    pub(super) fn flatten(&mut self, op: &Value) -> Result<String, String> {
        let id = op.get("clip").and_then(Value::as_u64).ok_or("flatten needs \"clip\": a merged clip (merge the layers first)")?;
        let seq = self.doc.seq();
        let mut mc = self.model_clip(id as u32).ok_or_else(|| format!("no clip {id}"))?;
        if op.get("op").and_then(Value::as_str) == Some("unflatten") {
            let back = mc.extra.get("flattened_from").and_then(Value::as_u64).ok_or_else(|| format!("clip {id} was not flattened"))?;
            if self.doc.model.sequence(m::SequenceId(back)).is_none() {
                return Err("the layers it was made from are gone".into());
            }
            mc.source = m::ClipSource::Sequence(m::SequenceRef { sequence: m::SequenceId(back), extra: Default::default() });
            mc.extra.remove("flattened_from");
            let ok = self.doc.apply("Unflatten", m::Edit::SetClip { seq, clip: Box::new(mc) });
            return self.check(ok, &format!("clip {id} is live layers again"));
        }
        let m::ClipSource::Sequence(r) = &mc.source else { return Err(format!("clip {id} is not a merged clip: merge its layers first, then flatten")) };
        let inner = r.sequence;
        let len = self.doc.model.sequence(inner).map(|s| s.duration()).unwrap_or(0);
        if len <= 0 {
            return Err("the merged clip is empty".into());
        }
        let dir = self.home.clone().map(|h| h.join("media").join("flattened")).unwrap_or_else(|| studio_paths::cache().join("flattened"));
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let name: String = self.doc.model.sequence(inner).map(|s| s.name.clone()).unwrap_or_default().chars().filter(|c| c.is_alphanumeric() || *c == ' ' || *c == '-').collect();
        // Its own file: the folder may be shared (the cache, other windows).
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos() % 1_000_000_000);
        let path = dir.join(format!("{}-{}-{}-{stamp:09}.mp4", if name.trim().is_empty() { "flattened".into() } else { name.trim().replace(' ', "-") }, self.doc.edits, std::process::id()));
        // The whole stretch the clip plays: a merged clip may run on past
        // the end of its layers (lengthened, or slowed down).
        let need = mc.source_out().max(len);
        let args = json!({"preset": "Social 1080p", "path": path.display().to_string(), "range": [0.0, secs(need)]});
        let out = self.within(inner, |s| s.run("export", &args))?;
        let said = out.iter().find_map(|c| match c {
            Content::Text(t) => Some(t.clone()),
            _ => None,
        });
        if !path.is_file() {
            return Err(format!("the render did not come out: {}", said.unwrap_or_default()));
        }
        let report = crate::services::project::import_media(&mut self.doc, &[path.clone()]);
        let media = *report.ids.first().ok_or("the rendered file could not be brought in")?;
        let mut mc = self.model_clip(id as u32).ok_or("the clip is gone")?;
        mc.source = m::ClipSource::Media(m::MediaRef { media: m::MediaId(media as u64), video: true, audio: false, extra: Default::default() });
        mc.extra.insert("flattened_from".into(), json!(inner.0));
        // Should the render stop short of that (it ends with the layers),
        // the clip keeps to the file instead of asking past its end.
        let mut fitted = String::new();
        let total = self.doc.model.media.iter().find(|x| x.id.0 == media as u64).and_then(|x| x.duration());
        if let Some(total) = total
            && mc.source_out() > total
        {
            let &m::Speed::Constant(speed) = &mc.speed else { return Err(format!("the render ({:.2}s) is shorter than the speed-ramped clip {id} plays; remove its speed ramp, then flatten", secs(total))) };
            let dur = ((total - mc.source_in) as f64 / speed.max(1e-6)).floor() as m::Micros;
            if dur <= 0 {
                return Err(format!("clip {id} starts at {:.2}s into layers that are {:.2}s long: nothing of them shows; slip it back first", secs(mc.source_in), secs(total)));
            }
            fitted = format!("; shortened to {:.2}s, where its layers end", secs(dur));
            mc.duration = dur;
        }
        let ok = self.doc.apply("Flatten", m::Edit::SetClip { seq, clip: Box::new(mc) });
        self.check(ok, &format!("clip {id} plays from one rendered file now ({}; its position, animation and effects stay; unflatten brings the layers back; it is opaque: anything it showed through is black){fitted}", path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()))
    }

    /// `align`: clips lined up on the canvas or on the first of them;
    /// `distribute`: spaced evenly between the outermost.
    pub(super) fn align(&mut self, op: &Value) -> Result<String, String> {
        let ids: Vec<u32> = op.get("clips").and_then(Value::as_array).ok_or("align / distribute need \"clips\"")?.iter().filter_map(|c| c.as_u64().map(|c| c as u32)).collect();
        if ids.is_empty() {
            return Err("align / distribute need \"clips\"".into());
        }
        let t = match f(op, "at") {
            Some(a) => super::micros(a),
            None => {
                let c = self.clip(ids[0])?;
                c.start + c.dur / 2
            }
        };
        // Each clip's box on screen [left, top, right, bottom].
        let mut boxes = Vec::new();
        for id in &ids {
            let v = self.on_screen(*id, t)?;
            let b: Vec<f64> = v["box"].as_array().map(|a| a.iter().filter_map(Value::as_f64).collect()).unwrap_or_default();
            let [l, tp, r, bt] = b[..] else { return Err(format!("clip {id} has no box on screen")) };
            boxes.push((*id, l, tp, r, bt));
        }
        let (w, h) = (self.doc.project.width as f64 / 2.0, self.doc.project.height as f64 / 2.0);
        let mut moves: Vec<(u32, f64, f64)> = Vec::new();
        if op.get("op").and_then(Value::as_str) == Some("distribute") {
            let axis = op.get("axis").and_then(Value::as_str).unwrap_or("horizontal");
            if boxes.len() < 3 {
                return Err("distribute needs three or more clips".into());
            }
            let horizontal = !axis.starts_with('v');
            let mut sorted = boxes.clone();
            sorted.sort_by(|a, b| if horizontal { a.1.total_cmp(&b.1) } else { a.2.total_cmp(&b.2) });
            // Equal gaps between the boxes, the outermost staying put.
            let size = |b: &(u32, f64, f64, f64, f64)| if horizontal { b.3 - b.1 } else { b.4 - b.2 };
            let (first, last) = (sorted[0], sorted[sorted.len() - 1]);
            let span = if horizontal { last.3 - first.1 } else { last.4 - first.2 };
            let gap = (span - sorted.iter().map(size).sum::<f64>()) / (sorted.len() - 1) as f64;
            let mut at = if horizontal { first.1 } else { first.2 };
            for b in &sorted {
                let now = if horizontal { b.1 } else { b.2 };
                if horizontal {
                    moves.push((b.0, at - now, 0.0));
                } else {
                    moves.push((b.0, 0.0, at - now));
                }
                at += size(b) + gap;
            }
        } else {
            let to = op.get("to").and_then(Value::as_str).ok_or("align needs \"to\": left, centre, right, top, middle, bottom")?;
            // Against the canvas, or the first clip ("relative": "first").
            let frame = if op.get("relative").and_then(Value::as_str) == Some("first") { (boxes[0].1, boxes[0].2, boxes[0].3, boxes[0].4) } else { (-w, -h, w, h) };
            let margin = f(op, "margin").unwrap_or(0.0);
            for &(id, l, tp, r, bt) in &boxes {
                let (dx, dy) = match to {
                    "left" => (frame.0 + margin - l, 0.0),
                    "right" => (frame.2 - margin - r, 0.0),
                    "centre" | "center" => ((frame.0 + frame.2) / 2.0 - (l + r) / 2.0, 0.0),
                    "top" => (0.0, frame.1 + margin - tp),
                    "bottom" => (0.0, frame.3 - margin - bt),
                    "middle" => (0.0, (frame.1 + frame.3) / 2.0 - (tp + bt) / 2.0),
                    other => return Err(format!("align to \"{other}\": left, centre, right, top, middle or bottom")),
                };
                moves.push((id, dx, dy));
            }
        }
        let mut n = 0;
        for (id, dx, dy) in moves {
            if dx.abs() < 0.05 && dy.abs() < 0.05 {
                continue;
            }
            let c = self.clip(id)?;
            // Animated position moves as a whole; a still one is set.
            let keyed = self.model_clip(id).is_some_and(|mc| mc.keyframes.get("transform.position").is_some_and(|t| !t.keys.is_empty()));
            if keyed {
                let mc = self.model_clip(id).ok_or("gone")?;
                let keys: Vec<Value> = mc.keyframes["transform.position"].keys.iter().filter_map(|k| match &k.value {
                    m::Value::Point(p) => Some(json!([k.time as f64 / 1e6, p.x + dx, p.y + dy])),
                    _ => None,
                }).collect();
                self.op(&json!({"op": "keys", "clip": id, "property": "position", "remove": "all"}))?;
                self.op(&json!({"op": "animate", "clip": id, "property": "position", "keys": keys}))?;
            } else {
                self.op(&json!({"op": "set", "clip": id, "x": c.transform.x as f64 + dx, "y": c.transform.y as f64 + dy}))?;
            }
            n += 1;
        }
        Ok(format!("{n} clip(s) moved"))
    }

    /// `parent`: clips follow another clip's position, scale and rotation
    /// (`"to": null` lets them go again).
    pub(super) fn parent(&mut self, op: &Value) -> Result<String, String> {
        let mut ids: Vec<u64> = op.get("clip").and_then(Value::as_u64).into_iter().collect();
        if let Some(Value::Array(cs)) = op.get("clips") {
            ids.extend(cs.iter().filter_map(Value::as_u64));
        }
        if ids.is_empty() {
            return Err("parent needs \"clip\" (or \"clips\"): the clip(s) that follow, and \"to\": the clip they follow (null to let go)".into());
        }
        let to = match op.get("to") {
            Some(Value::Null) => None,
            Some(v) => Some(v.as_u64().ok_or("\"to\" is a clip id, or null to let go")?),
            None => return Err("parent needs \"to\": the clip to follow, or null to let go".into()),
        };
        let mut said = Vec::new();
        for id in ids {
            let at = match f(op, "at") {
                Some(a) => super::micros(a),
                None => self.doc.model.sequence(self.doc.seq()).and_then(|s| s.clip(m::ClipId(id))).map_or(0, |c| c.start),
            };
            said.push(crate::services::layers::set_parent(&mut self.doc, id, to, at)?);
        }
        Ok(said.join("; "))
    }

    /// `matte`: a clip shows only through the clip above it (`with`, by
    /// default the nearest above), by its shape or brightness.
    pub(super) fn matte(&mut self, op: &Value) -> Result<String, String> {
        let id = op.get("clip").and_then(Value::as_u64).ok_or("matte needs \"clip\": the clip to cut")?;
        if op.get("remove").and_then(Value::as_bool).unwrap_or(false) {
            let (_, inner) = crate::services::layers::matte_of(&self.doc, id).ok_or_else(|| format!("clip {id} has no track matte"))?;
            let seq = self.merged_sequence(id).ok_or("not a matted clip")?;
            let mut c = self.doc.model.sequence(seq).and_then(|s| s.clip(m::ClipId(inner))).cloned().ok_or("the matte is gone")?;
            c.blend = m::BlendMode::Normal;
            let ok = self.doc.apply("Remove matte", m::Edit::SetClip { seq, clip: Box::new(c) });
            self.check(ok, "")?;
            return self.merge(&json!({"op": "unmerge", "clip": id})).map(|s| format!("matte removed; {s}"));
        }
        let with = match op.get("with").and_then(Value::as_u64) {
            Some(w) => w,
            None => crate::services::layers::layer_above(&self.doc, id).ok_or_else(|| format!("nothing sits on a lane above clip {id} at its time; put the matte (a shape, title or picture) above it, or name it with \"with\""))?,
        };
        let mode = op.get("mode").and_then(Value::as_str).unwrap_or("alpha").to_owned();
        let mut ids = vec![self.doc.alloc_id(), self.doc.alloc_id()];
        let mut next = move || ids.remove(0);
        let merged = crate::services::layers::track_matte(&mut self.doc, id, with, &mode, &mut next)?;
        Ok(format!("clip {id} shows only through clip {with} ({mode} matte); together they are clip {merged}, which moves, animates and takes effects as one (edit either inside it by its id; matte remove takes it off)"))
    }

    /// What a clip is linked to, for `inspect`.
    pub(super) fn links(&self, id: u64) -> Option<Value> {
        let c = self.model_clip(id as u32)?;
        let mut o = serde_json::Map::new();
        if let Some(p) = &c.parent {
            o.insert("parent".into(), json!(p.clip.0));
        }
        if let Some((mode, matte)) = crate::services::layers::matte_of(&self.doc, id) {
            o.insert("matte".into(), json!({"mode": mode, "clip": matte}));
        }
        if c.blend.is_matte() {
            o.insert("is_a_matte".into(), json!(true));
        }
        (!o.is_empty()).then(|| Value::Object(o))
    }

    /// `restack`: a clip in front of or behind another, or to the top or
    /// bottom of what shows at its time (lanes higher up draw over lower
    /// ones; the main lane is the bottom).
    pub(super) fn restack(&mut self, op: &Value) -> Result<String, String> {
        let mut ids: Vec<u32> = op.get("clip").and_then(Value::as_u64).map(|c| c as u32).into_iter().collect();
        if let Some(Value::Array(cs)) = op.get("clips") {
            ids.extend(cs.iter().filter_map(|c| c.as_u64().map(|c| c as u32)));
        }
        if ids.is_empty() {
            return Err("restack needs \"clip\" (or \"clips\") and one of in_front_of, behind, or to: top / bottom".into());
        }
        let mut said = Vec::new();
        for id in ids {
            let c = self.clip(id)?;
            let p = &self.doc.project;
            let idx = |l: u32| p.lanes.iter().position(|x| x.id == l);
            let spine = p.spine();
            let spine_i = spine.and_then(idx).unwrap_or(p.lanes.len());
            let free = |l: u32| !p.clips.iter().any(|o| o.id != id && o.lane == l && o.start < c.end() && c.start < o.end());
            let visual = |i: usize| p.lanes.get(i).is_some_and(|l| l.kind == crate::view::LaneKind::Visual && !l.locked && Some(l.id) != spine);
            // (lane to use, or the index to put a new lane at)
            let (pick, new_at): (Option<u32>, usize) = if let Some(o) = op.get("in_front_of").and_then(Value::as_u64) {
                let oi = idx(self.clip(o as u32)?.lane).ok_or("that clip has no lane")?;
                ((0..oi).rev().find(|&i| visual(i) && free(p.lanes[i].id)).map(|i| p.lanes[i].id), oi)
            } else if let Some(o) = op.get("behind").and_then(Value::as_u64) {
                let ol = self.clip(o as u32)?.lane;
                if Some(ol) == spine {
                    return Err(format!("clip {o} is on the main lane, the bottom of the picture: nothing goes behind it; put the background on the main lane instead and clip {o} above it"));
                }
                let oi = idx(ol).ok_or("that clip has no lane")?;
                ((oi + 1..spine_i).find(|&i| visual(i) && free(p.lanes[i].id)).map(|i| p.lanes[i].id), oi + 1)
            } else {
                match op.get("to").and_then(Value::as_str) {
                    Some("top") => {
                        let busy_top = (0..spine_i).filter(|&i| p.lanes.get(i).is_some_and(|l| l.kind == crate::view::LaneKind::Visual) && p.clips.iter().any(|o| o.id != id && o.lane == p.lanes[i].id && o.start < c.end() && c.start < o.end())).min();
                        let top = busy_top.unwrap_or(spine_i);
                        ((0..top).rev().find(|&i| visual(i) && free(p.lanes[i].id)).map(|i| p.lanes[i].id), top)
                    }
                    // The lane right above the main one, if it is free there; otherwise a new one.
                    Some("bottom") => (spine_i.checked_sub(1).filter(|&i| visual(i) && free(p.lanes[i].id)).map(|i| p.lanes[i].id), spine_i),
                    _ => return Err("restack needs in_front_of: <clip>, behind: <clip>, or to: \"top\" / \"bottom\"".into()),
                }
            };
            let lane = match pick {
                Some(l) if l == c.lane => {
                    said.push(format!("clip {id} already sits there"));
                    continue;
                }
                Some(l) => l,
                None => {
                    let lid = self.doc.alloc_id();
                    let seq = self.doc.seq();
                    let ok = self.doc.apply("Add lane", m::Edit::AddLane { seq, lane: m::Lane::new(m::LaneId(lid), m::LaneKind::Visual, "Overlay"), index: Some(new_at) });
                    self.check(ok, "")?;
                    lid as u32
                }
            };
            self.op(&json!({"op": "move", "clips": [id], "lane": lane, "by": 0}))?;
            said.push(format!("clip {id} moved to lane {lane}"));
        }
        Ok(said.join("; "))
    }
}
