// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The rest of the timeline, each as one op: transitions that keep the
//! timing, lanes, a clip's effect stack, colour cards, markers.

use super::{Session, f, micros, secs};
use crate::services::library::Panel;
use crate::view::{self, SEC};
use serde_json::{Value, json};
use studio_model as m;

impl Session {
    /// `transition`: put (or replace, or remove) the transition from a
    /// main-lane clip into the next one. Timing is kept: the transition
    /// uses spare footage on whichever side has it, and only shortens the
    /// clips (moving everything after) when `keep_timing` is false.
    pub(super) fn transition(&mut self, op: &Value) -> Result<String, String> {
        let id = op.get("clip").and_then(Value::as_u64).ok_or("transition needs \"clip\" (the clip before the cut)")? as u32;
        let seq = self.doc.seq();
        let c = self.clip(id)?;
        if self.doc.project.spine() != Some(c.lane) {
            return Err("transitions go between clips on the main lane; for overlay clips animate opacity, scale or position instead".into());
        }
        let next = self.doc.project.clips.iter().find(|n| n.lane == c.lane && n.start == c.end()).map(|n| n.id).ok_or_else(|| format!("clip {id} is the last on the main lane: nothing to transition into"))?;
        for old in self.doc.project.transitions.iter().filter(|t| t.left == id).cloned().collect::<Vec<_>>() {
            let ok = self.doc.apply("Transition", m::Edit::RemoveTransition { seq, id: m::TransitionId(old.id) });
            self.check(ok, "")?;
        }
        if op.get("remove").and_then(Value::as_bool).unwrap_or(false) {
            return Ok(format!("cut from clip {id} to {next} is a straight cut"));
        }
        let item = op.get("item").and_then(Value::as_str).ok_or("transition needs \"item\" (a name from catalogue transitions), or \"remove\": true")?;
        let (_, index, found) = self.resolve(item, &[Panel::Cuts])?;
        let kind = crate::catalogue::cuts()[index].id.to_owned();
        let frame = (SEC as f64 / self.doc.project.fps.max(1.0) as f64) as view::Time;
        let want = micros(f(op, "duration").unwrap_or(0.5)).max(2 * frame);
        let left = m::ClipId(id as u64);
        let limit = |a| self.doc.model.max_transition_duration(seq, left, a).unwrap_or(0);
        let wanted_align = match op.get("align").and_then(Value::as_str).unwrap_or("auto") {
            "centre" | "center" => Some(m::TransitionAlign::Centre),
            "end_at_cut" | "before" => Some(m::TransitionAlign::EndAtCut),
            "start_at_cut" | "after" => Some(m::TransitionAlign::StartAtCut),
            "auto" => None,
            other => return Err(format!("align \"{other}\": use auto, centre, end_at_cut or start_at_cut")),
        };
        // The alignment with room for the most of the wanted length.
        let candidates = match wanted_align {
            Some(a) => vec![a],
            None => vec![m::TransitionAlign::Centre, m::TransitionAlign::StartAtCut, m::TransitionAlign::EndAtCut],
        };
        let (align, room) = candidates.iter().map(|&a| (a, limit(a))).max_by_key(|&(_, r)| r.min(want)).unwrap_or((m::TransitionAlign::Centre, 0));
        let mut note = String::new();
        let duration = if room >= 2 * frame {
            let d = want.min(room) / frame * frame;
            if d + frame / 2 < want {
                note = format!(" (shortened to {:.2}s: the clips have only that much spare footage at the cut)", secs(d));
            }
            d
        } else if op.get("keep_timing").and_then(Value::as_bool) == Some(false) {
            // Make room by trimming both sides; everything after moves earlier.
            let half = want / 2;
            for (clip, edge, delta) in [(left, m::Edge::End, -half), (m::ClipId(next as u64), m::Edge::Start, half)] {
                let ok = self.doc.apply("Transition room", m::Edit::Trim { seq, clip, edge, delta, ripple: true });
                self.check(ok, "")?;
            }
            note = format!(" (trimmed {:.2}s from the cut; everything after moved earlier)", secs(want));
            want
        } else {
            return Err(format!("clips {id} and {next} use all their footage at the cut, so a transition needs frames neither has; slip one of them (slip by ±{:.2}), or pass \"keep_timing\": false to trim the cut and move everything after it earlier", secs(want)));
        };
        let tid = self.doc.alloc_id();
        let mut params = m::ParamMap::default();
        if let Some(Value::Object(ps)) = op.get("params") {
            for (k, v) in ps {
                if let Some(x) = v.as_f64() {
                    params.insert(k.clone(), m::Value::Scalar(x));
                }
            }
        }
        let t = m::Transition { id: m::TransitionId(tid), left, right: m::ClipId(next as u64), kind, duration, align, params, extra: Default::default() };
        let ok = self.doc.apply("Transition", m::Edit::AddTransition { seq, transition: t });
        self.check(ok, "")?;
        Ok(format!("{found} from clip {id} into {next}, {:.2}s{note}", secs(duration)))
    }

    /// `lane`: add, rename, lock, hide, mute, solo, move or remove a lane.
    pub(super) fn lane(&mut self, op: &Value) -> Result<String, String> {
        let action = op.get("action").and_then(Value::as_str).ok_or("lane needs \"action\": add, rename, lock, unlock, hide, show, mute, unmute, solo, unsolo, move, remove")?;
        let seq = self.doc.seq();
        if action == "add" {
            let kind = match op.get("kind").and_then(Value::as_str).unwrap_or("overlay") {
                "overlay" | "visual" | "picture" => m::LaneKind::Visual,
                "sound" | "audio" | "music" => m::LaneKind::Audio,
                other => return Err(format!("kind \"{other}\": overlay or sound")),
            };
            let id = self.doc.alloc_id();
            let name = op.get("name").and_then(Value::as_str).unwrap_or(if kind == m::LaneKind::Audio { "Sound" } else { "Overlay" }).to_owned();
            let lanes = &self.doc.project.lanes;
            // Overlays go on top, sound at the bottom, unless told.
            let index = match op.get("position").and_then(Value::as_str) {
                Some("top") => 0,
                Some("bottom") => lanes.len(),
                _ if kind == m::LaneKind::Audio => lanes.len(),
                _ => 0,
            };
            let ok = self.doc.apply("Add lane", m::Edit::AddLane { seq, lane: m::Lane::new(m::LaneId(id), kind, name.clone()), index: Some(index) });
            self.check(ok, "")?;
            return Ok(format!("lane \"{name}\" is lane {id}"));
        }
        let lane_id = super::craft::lane_of(&self.doc.project, op.get("lane").ok_or("lane needs \"lane\" (a name or id)")?, None)?;
        let mut lane = self.doc.model.sequences.iter().find_map(|s| s.lanes.iter().find(|l| l.id.0 == lane_id as u64)).cloned().ok_or("that lane is gone")?;
        let edit = match action {
            "rename" => {
                lane.name = op.get("name").and_then(Value::as_str).ok_or("rename needs \"name\"")?.to_owned();
                m::Edit::SetLane { seq, lane }
            }
            "lock" | "unlock" => {
                lane.locked = action == "lock";
                m::Edit::SetLane { seq, lane }
            }
            "hide" | "show" => {
                lane.hidden = action == "hide";
                m::Edit::SetLane { seq, lane }
            }
            "mute" | "unmute" => {
                lane.muted = action == "mute";
                m::Edit::SetLane { seq, lane }
            }
            "solo" | "unsolo" => {
                lane.solo = action == "solo";
                m::Edit::SetLane { seq, lane }
            }
            "move" => {
                let to = op.get("to").and_then(Value::as_u64).ok_or("move needs \"to\" (0 = top)")? as usize;
                m::Edit::MoveLane { seq, lane: lane.id, to }
            }
            "remove" => m::Edit::DeleteLane { seq, lane: lane.id },
            other => return Err(format!("unknown lane action \"{other}\"")),
        };
        let ok = self.doc.apply("Lane", edit);
        self.check(ok, &format!("lane {lane_id}: {action}"))
    }

    /// `effect`: one effect on a clip's stack: remove it, switch it off or
    /// on, move it in the stack, or change its settings.
    pub(super) fn effect(&mut self, op: &Value) -> Result<String, String> {
        let id = op.get("clip").and_then(Value::as_u64).ok_or("effect needs \"clip\"")? as u32;
        let mc = self.model_clip(id).ok_or_else(|| format!("no clip {id}"))?;
        if mc.effects.is_empty() {
            return Err(format!("clip {id} has no effects; add one with apply"));
        }
        let which = op.get("effect").ok_or("effect needs \"effect\": its name (zoom_blur, zoom_blur#2) or its place in the stack (1 = first)")?;
        let e = match which {
            Value::Number(n) => mc.effects.get((n.as_u64().unwrap_or(1) as usize).saturating_sub(1)),
            Value::String(s) => {
                let (name, nth) = s.split_once('#').map_or((s.as_str(), 1), |(a, b)| (a, b.parse().unwrap_or(1)));
                let want = name.to_lowercase().replace([' ', '_', '-'], "");
                mc.effects.iter().filter(|e| super::props::effect_name(&e.kind).replace('_', "").contains(&want)).nth(nth.max(1) - 1)
            }
            _ => None,
        }
        .cloned()
        .ok_or_else(|| format!("clip {id} has no effect {which}; it has: {}", mc.effects.iter().map(|e| super::props::effect_name(&e.kind)).collect::<Vec<_>>().join(", ")))?;
        let seq = self.doc.seq();
        let clip = m::ClipId(id as u64);
        let name = super::props::effect_name(&e.kind);
        if op.get("remove").and_then(Value::as_bool).unwrap_or(false) {
            let ok = self.doc.apply("Remove effect", m::Edit::RemoveEffect { seq, clip, effect: e.id });
            return self.check(ok, &format!("removed {name} from clip {id}"));
        }
        let mut changed = e.clone();
        if let Some(b) = op.get("enabled").and_then(Value::as_bool) {
            changed.enabled = b;
        }
        if let Some(Value::Object(ps)) = op.get("params") {
            for (k, v) in ps {
                let k = &super::effect_param_name(&e.kind, k)?;
                let path = format!("effect.{}.{k}", e.id.0);
                let prop = super::props::Prop { name: k.clone(), paths: vec![path.clone()], conv: super::props::Conv::Raw };
                let cur = super::props::value_or_default(&mc, &path);
                changed.params.insert(k.clone(), super::props::to_model(&prop, &path, v, cur.as_ref())?);
            }
        }
        if changed != e {
            let ok = self.doc.apply("Effect", m::Edit::SetEffect { seq, clip, effect: changed });
            self.check(ok, "")?;
        }
        if let Some(to) = op.get("move_to").and_then(Value::as_u64) {
            let ok = self.doc.apply("Move effect", m::Edit::MoveEffect { seq, clip, effect: e.id, to: (to as usize).saturating_sub(1) });
            self.check(ok, "")?;
        }
        Ok(format!("{name} on clip {id} changed"))
    }

    /// `card`: a solid colour clip (black for a title card or a beat of
    /// darkness, white for a flash). On an overlay lane it covers what is
    /// below; on the main lane it is part of the story.
    pub(super) fn card(&mut self, op: &Value) -> Result<String, String> {
        let colour = op.get("colour").and_then(Value::as_str).unwrap_or("black");
        let k = super::craft::colour(colour).ok_or_else(|| format!("colour \"{colour}\" is not #rrggbb or a basic name"))?;
        let at = micros(f(op, "at").ok_or("card needs \"at\"")?);
        let dur = micros(f(op, "duration").unwrap_or(1.0)).max(SEC / 30);
        let explicit = op.get("lane").is_some();
        let wanted = match op.get("lane") {
            Some(v) => super::craft::lane_of(&self.doc.project, v, None)?,
            None => super::craft::lane_of(&self.doc.project, &json!("overlay"), None)?,
        };
        let mut spine = self.doc.project.spine() == Some(wanted);
        // `behind`: just under that clip; a lane named: that lane if free;
        // otherwise on top of what shows then.
        let mut under_spine = false;
        let behind = match op.get("behind").and_then(super::craft::as_id) {
            Some(b) => {
                let lane = self.clip(b as u32)?.lane;
                // Only overlay lanes sit above the main lane. Behind a clip
                // on the main lane (or a sound lane) the only place under the
                // picture is the main lane itself: the card goes there, and
                // what it covers moves up to an overlay lane, unchanged
                // (this was "Not possible: LaneOrder").
                let overlay = self.doc.project.lane(lane).is_some_and(|l| l.kind == view::LaneKind::Visual) && self.doc.project.spine() != Some(lane);
                if overlay {
                    Some(lane)
                } else {
                    under_spine = true;
                    spine = true;
                    None
                }
            }
            None => None,
        };
        let lane = if spine {
            let main = self.doc.project.spine().ok_or("the project has no main lane")?;
            if under_spine {
                self.clear_main_for(at, dur)?;
            }
            main
        } else {
            self.room(explicit.then_some(wanted), at, dur, behind)?
        };
        let id = self.doc.alloc_id() as u32;
        let rgba = m::Rgba { r: k.r() as f64 / 255.0, g: k.g() as f64 / 255.0, b: k.b() as f64 / 255.0, a: 1.0 };
        // With width / height (pixels) it is a box, not the whole frame.
        // A gradient fills the frame: a box the frame's size is the frame;
        // a smaller one can't take it (it was dropped without a word).
        let (pw, ph) = (self.doc.project.width as f64, self.doc.project.height as f64);
        let size = match (f(op, "width"), f(op, "height")) {
            (w, h) if op.get("gradient").is_some() && w.is_none_or(|w| w >= pw) && h.is_none_or(|h| h >= ph) => (None, None),
            (w, h) if op.get("gradient").is_some() && (w.is_some() || h.is_some()) => {
                return Err(format!("a card's gradient fills the whole frame: leave out width and height (or make them {pw} x {ph}); a box with a gradient is a `shape` with a gradient fill or vector art"));
            }
            wh => wh,
        };
        let g = match size {
            (None, None) => {
                // A gradient over the whole frame (a backdrop that is not
                // flat), or one colour.
                match op.get("gradient").and_then(Value::as_array) {
                    Some(stops) => {
                        let at = |i: usize| -> Result<m::Rgba, String> {
                            let s = stops.get(i).and_then(Value::as_str).ok_or("gradient is two colours: [\"#from\", \"#to\"]")?;
                            let k = super::craft::colour(s).ok_or_else(|| format!("gradient colour \"{s}\" is not #rrggbb or a basic name"))?;
                            Ok(m::Rgba { r: k.r() as f64 / 255.0, g: k.g() as f64 / 255.0, b: k.b() as f64 / 255.0, a: 1.0 })
                        };
                        let mut g = m::Generator::new(m::GeneratorKind::Gradient);
                        if stops.len() < 2 {
                            return Err("gradient takes two or more colours (more make a smoother, richer ramp)".into());
                        }
                        g.params.insert("start".into(), m::Value::Colour(at(0)?));
                        g.params.insert("end".into(), m::Value::Colour(at(stops.len() - 1)?));
                        // Three or more: stops evenly along it, the curve
                        // passing smoothly through each.
                        if stops.len() > 2 {
                            for i in 0..stops.len().min(16) {
                                g.params.insert(format!("stop{i}"), m::Value::Colour(at(i)?));
                            }
                        }
                        g.params.insert("angle".into(), m::Value::Scalar(f(op, "angle").unwrap_or(90.0)));
                        if op.get("radial").and_then(Value::as_bool).unwrap_or(false) {
                            g.params.insert("radial".into(), m::Value::Bool(true));
                            g.params.insert("radius".into(), m::Value::Scalar(f(op, "radius").unwrap_or(0.75)));
                        }
                        g
                    }
                    None => {
                        let mut g = m::Generator::new(m::GeneratorKind::Solid);
                        g.params.insert("colour".into(), m::Value::Colour(rgba));
                        g
                    }
                }
            }
            (w, h) => {
                let (cw, ch) = (self.doc.project.width as f64, self.doc.project.height as f64);
                let mut g = m::Generator::new(m::GeneratorKind::Shape);
                let shape = op.get("shape").and_then(Value::as_str).unwrap_or("rectangle");
                if !["rectangle", "ellipse", "polygon", "star", "arrow", "line"].contains(&shape) {
                    return Err(format!("shape \"{shape}\": rectangle, ellipse, polygon, star, arrow or line"));
                }
                g.params.insert(crate::bridge::SHAPE_KEY.into(), m::Value::Choice(shape.into()));
                if let Some(n) = f(op, "sides") {
                    g.params.insert("sides".into(), m::Value::Scalar(n));
                }
                // An outline: a colour and a width in pixels.
                if let Some(sc) = op.get("stroke").and_then(Value::as_str) {
                    let k = super::craft::colour(sc).ok_or_else(|| format!("stroke colour \"{sc}\" is not #rrggbb or a basic name"))?;
                    g.params.insert("stroke".into(), m::Value::Colour(m::Rgba { r: k.r() as f64 / 255.0, g: k.g() as f64 / 255.0, b: k.b() as f64 / 255.0, a: 1.0 }));
                    g.params.insert("stroke_width".into(), m::Value::Scalar(f(op, "stroke_width").unwrap_or(4.0) / cw.min(ch)));
                }
                g.params.insert("fill".into(), m::Value::Colour(rgba));
                g.params.insert("size".into(), m::Value::Point(m::Vec2 { x: (w.unwrap_or(cw) / cw).min(1.0), y: (h.unwrap_or(ch) / ch).min(1.0) }));
                g.params.insert("roundness".into(), m::Value::Scalar(f(op, "roundness").unwrap_or(0.0)));
                g
            }
        };
        let mut c = m::Clip::new(m::ClipId(id as u64), m::LaneId(lane as u64), at, dur, m::ClipSource::Generator(g));
        c.name = format!("{colour} card");
        // `shadow`: true for a soft one under it, or its settings.
        match op.get("shadow") {
            None | Some(Value::Bool(false)) | Some(Value::Null) => {}
            Some(spec) => {
                let mut e = m::Effect::new(m::EffectId(self.doc.alloc_id()), "drop_shadow");
                if let Value::Object(o) = spec {
                    for (k, v) in o {
                        if let Some(x) = v.as_f64() {
                            e.params.insert(k.clone(), m::Value::Scalar(x));
                        }
                    }
                }
                c.effects.push(e);
            }
        }
        c.transform.position = m::Vec2 { x: f(op, "x").unwrap_or(0.0), y: f(op, "y").unwrap_or(0.0) };
        if let Some(o) = f(op, "opacity") {
            c.transform.opacity = (o / 100.0).clamp(0.0, 1.0);
        }
        let seq = self.doc.seq();
        let ok = if under_spine {
            // Into the room cleared on the main lane: nothing after it moves.
            self.doc.apply("Card", m::Edit::Overwrite { seq, lane: Some(m::LaneId(lane as u64)), at, clip: Box::new(c) })
        } else if spine {
            self.doc.apply("Card", m::Edit::Insert { seq, lane: Some(m::LaneId(lane as u64)), at, clip: Box::new(c) })
        } else {
            self.doc.apply("Card", m::Edit::InsertClip { seq, clip: Box::new(c) })
        };
        self.check(ok, &format!("{colour} card as clip {id}"))
    }

    /// Makes room on the main lane for `at..at+dur` without changing the
    /// picture: the clips there move up to overlay lanes at the same time.
    fn clear_main_for(&mut self, at: view::Time, dur: view::Time) -> Result<(), String> {
        let Some(main) = self.doc.project.spine() else { return Ok(()) };
        let mut over: Vec<(u32, view::Time, view::Time)> = self.doc.project.clips.iter().filter(|c| c.lane == main && c.start < at + dur && at < c.end()).map(|c| (c.id, c.start, c.dur)).collect();
        over.sort_by_key(|c| c.1);
        for (id, start, d) in over {
            let up = self.room(None, start, d, None)?;
            let seq = self.doc.seq();
            let ok = self.doc.apply("Move up", m::Edit::Move { seq, clips: vec![m::ClipId(id as u64)], delta: 0, to_lane: Some(m::LaneId(up as u64)) });
            self.check(ok, "")?;
        }
        Ok(())
    }

    /// `marker`: add one, or with `remove` / `move` change existing ones
    /// (found by time, nearest within half a second).
    pub(super) fn marker(&mut self, op: &Value) -> Result<String, String> {
        // Named, coloured, spanning or on a clip: a full model marker.
        if ["name", "colour", "color", "duration", "clip"].iter().any(|k| op.get(*k).is_some()) && op.get("remove").is_none() && op.get("move").is_none() {
            let at = micros(f(op, "at").ok_or("marker needs \"at\" (timeline seconds)")?);
            let seq = self.doc.seq();
            let clip = op.get("clip").and_then(Value::as_u64).map(m::ClipId);
            let start = match clip {
                Some(c) => self.doc.model.active().clip(c).map(|c| c.start).ok_or_else(|| format!("no clip {}", c.0))?,
                None => 0,
            };
            let mut mk = m::Marker::new(m::MarkerId(self.doc.alloc_id()), at - start, op.get("name").and_then(Value::as_str).unwrap_or(""));
            mk.note = op.get("note").and_then(Value::as_str).unwrap_or("").to_owned();
            mk.duration = micros(f(op, "duration").unwrap_or(0.0)).max(0);
            if let Some(c) = op.get("colour").or(op.get("color")).and_then(Value::as_str) {
                mk.colour = studio_model::editorial::marker_colour(c).ok_or_else(|| format!("marker colour \"{c}\": {}", studio_model::editorial::MARKER_COLOURS.map(|(n, _)| n).join(", ")))?;
            }
            let what = format!("{} marker at {}s", studio_model::editorial::colour_name(mk.colour), f(op, "at").unwrap_or(0.0));
            let ok = self.doc.apply("Marker", m::Edit::AddMarker { seq, clip, marker: mk });
            return self.check(ok, &what);
        }
        let near = |p: &view::Project, t: f64| p.markers.iter().min_by_key(|mk| (mk.time - micros(t)).abs()).filter(|mk| (mk.time - micros(t)).abs() <= SEC / 2).map(|mk| mk.id);
        if let Some(t) = op.get("remove").and_then(Value::as_f64) {
            let id = near(&self.doc.project, t).ok_or_else(|| format!("no marker near {t}s"))?;
            let ok = self.doc.edit("Marker", |p| p.markers.retain(|mk| mk.id != id));
            return self.check(ok, "marker removed");
        }
        if let Some(Value::Array(mv)) = op.get("move") {
            let (Some(from), Some(to)) = (mv.first().and_then(Value::as_f64), mv.get(1).and_then(Value::as_f64)) else { return Err("move is [from, to] seconds".into()) };
            let id = near(&self.doc.project, from).ok_or_else(|| format!("no marker near {from}s"))?;
            let ok = self.doc.edit("Marker", |p| {
                if let Some(mk) = p.markers.iter_mut().find(|mk| mk.id == id) {
                    mk.time = micros(to);
                }
            });
            return self.check(ok, "marker moved");
        }
        let at = micros(f(op, "at").ok_or("marker needs \"at\" (or \"remove\": t, \"move\": [from, to])")?);
        let note = op.get("note").and_then(Value::as_str).unwrap_or("").to_owned();
        let id = self.doc.alloc_id();
        let ok = self.doc.edit("Marker", |p| p.markers.push(view::Marker { id, time: at, note }));
        self.check(ok, "marker added")
    }
}

fn ids_of(op: &Value, k: &str) -> Vec<m::ClipId> {
    match op.get(k) {
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_u64).map(m::ClipId).collect(),
        Some(v) => v.as_u64().map(|x| vec![m::ClipId(x)]).unwrap_or_default(),
        None => Vec::new(),
    }
}

impl Session {
    /// The trims and rearrangements editors reach for, one op each.
    pub(super) fn editing(&mut self, name: &str, op: &Value) -> Result<String, String> {
        let seq = self.doc.seq();
        let clip = || op.get("clip").and_then(Value::as_u64).map(m::ClipId).ok_or_else(|| format!("{name} needs \"clip\""));
        let by = || f(op, "by").map(micros).ok_or_else(|| format!("{name} needs \"by\" (seconds)"));
        let (label, edit, what) = match name {
            // Move the cut between a clip and the next: one gets longer, the
            // other shorter, nothing else moves.
            "roll" => ("Roll", m::Edit::Roll { seq, left: clip()?, delta: by()? }, "rolled the cut"),
            // Move a clip along between its neighbours, which give and take.
            "slide" => ("Slide", m::Edit::Slide { seq, clip: clip()?, delta: by()? }, "slid the clip"),
            "duplicate" => ("Duplicate", m::Edit::Duplicate { seq, clips: ids_of(op, "clips") }, "duplicated"),
            "freeze" => {
                let at = micros(f(op, "at").ok_or("freeze needs \"at\" (timeline seconds)")?);
                let duration = micros(f(op, "duration").unwrap_or(1.0)).max(SEC / 30);
                let new_clip = m::ClipId(self.doc.alloc_id());
                ("Freeze", m::Edit::FreezeFrame { seq, clip: clip()?, at, duration, new_clip }, "froze the frame")
            }
            // With marks it is a source edit (the mark lines up with the
            // record mark).
            "replace" if ["src_in", "src_out", "at", "rec_out", "sequence"].iter().any(|k| op.get(*k).is_some()) => return self.source_op("replace", op),
            "replace" => {
                let media = super::craft::media_of(&self.doc.project, op.get("media").ok_or("replace needs \"media\"")?)?;
                ("Replace", m::Edit::ReplaceMedia { seq, clip: clip()?, media: m::MediaId(media as u64) }, "replaced the footage, timing kept")
            }
            "reorder" => {
                let before = match op.get("before") {
                    Some(v) => Some(m::ClipId(v.as_u64().ok_or("before is a clip")?)),
                    None => None,
                };
                ("Reorder", m::Edit::Reorder { seq, clip: clip()?, before }, "reordered")
            }
            "group" => ("Group", m::Edit::Group { seq, clips: ids_of(op, "clips"), group: m::GroupId(self.doc.alloc_id()) }, "grouped"),
            "ungroup" => ("Ungroup", m::Edit::Ungroup { seq, clips: ids_of(op, "clips") }, "ungrouped"),
            "link" => ("Link", m::Edit::Link { seq, clips: ids_of(op, "clips"), link: m::LinkId(self.doc.alloc_id()) }, "linked"),
            "copy_look" => {
                // Copy chosen parts of one clip onto others.
                let from = op.get("from").and_then(Value::as_u64).ok_or("copy_look needs \"from\" (the clip to copy)")?;
                let source = self.model_clip(from as u32).ok_or_else(|| format!("no clip {from}"))?;
                let parts: Vec<String> = match op.get("parts") {
                    Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).map(str::to_owned).collect(),
                    _ => vec!["effects".into(), "grade".into(), "blend".into(), "masks".into()],
                };
                let mut attrs = m::AttrSet::default();
                for p in &parts {
                    match p.as_str() {
                        "transform" => attrs.transform = true,
                        "blend" => attrs.blend = true,
                        "audio" => attrs.audio = true,
                        "effects" => attrs.effects = true,
                        "grade" | "look" => attrs.grade = true,
                        "masks" => attrs.masks = true,
                        other => return Err(format!("part \"{other}\": transform, blend, audio, effects, grade, masks")),
                    }
                }
                ("Copy look", m::Edit::PasteAttributes { seq, targets: ids_of(op, "clips"), source: Box::new(source), attrs }, "copied")
            }
            other => return Err(format!("unknown op {other}")),
        };
        let ok = self.doc.apply(label, edit);
        self.check(ok, what)
    }

    /// `mask`: add a shaped mask (with its settings), change or remove one.
    pub(super) fn mask(&mut self, op: &Value) -> Result<String, String> {
        let id = op.get("clip").and_then(Value::as_u64).ok_or("mask needs \"clip\"")? as u32;
        // A region of the backdrop (sky, water, trees...) as masks.
        #[cfg(feature = "render")]
        if op.get("region").is_some() {
            return self.mask_region(op);
        }
        let mc = self.model_clip(id).ok_or_else(|| format!("no clip {id}"))?;
        let seq = self.doc.seq();
        let cid = m::ClipId(id as u64);
        let existing = |n: &Value| -> Result<m::Mask, String> {
            let i = n.as_u64().unwrap_or(1).max(1) as usize;
            mc.masks.get(i - 1).cloned().ok_or_else(|| format!("clip {id} has {} mask(s)", mc.masks.len()))
        };
        if let Some(n) = op.get("remove") {
            let mk = existing(n)?;
            let ok = self.doc.apply("Mask", m::Edit::RemoveMask { seq, clip: cid, mask: mk.id });
            return self.check(ok, "mask removed");
        }
        let (mut mk, adding) = match op.get("mask") {
            Some(n) => (existing(n)?, false),
            None => {
                let shape = op.get("shape").and_then(Value::as_str).unwrap_or("rectangle");
                let shape: m::MaskShape = serde_json::from_value(json!(shape)).map_err(|_| format!("shape \"{shape}\": rectangle, ellipse, line, mirror, star, heart"))?;
                (m::Mask::new(m::MaskId(self.doc.alloc_id()), shape), true)
            }
        };
        if let Some(b) = op.get("invert").and_then(Value::as_bool) {
            mk.invert = b;
        }
        if let Some(Value::Object(ps)) = op.get("params") {
            for (k, v) in ps {
                let val = match v {
                    Value::Array(a) if a.len() == 2 => m::Value::Point(m::Vec2 { x: a[0].as_f64().unwrap_or(0.0), y: a[1].as_f64().unwrap_or(0.0) }),
                    Value::Bool(b) => m::Value::Bool(*b),
                    _ => m::Value::Scalar(v.as_f64().ok_or_else(|| format!("mask setting {k} takes a number or [x, y]"))?),
                };
                mk.params.insert(k.clone(), val);
            }
        }
        let ok = if adding { self.doc.apply("Mask", m::Edit::AddMask { seq, clip: cid, mask: mk, index: None }) } else { self.doc.apply("Mask", m::Edit::SetMask { seq, clip: cid, mask: mk }) };
        self.check(ok, if adding { "mask added (inspect lists its settings as mask1.<setting>)" } else { "mask changed" })
    }
}

impl Session {
    /// A picture lane free over `at..at + dur`: `want` if it is free, else
    /// another overlay lane at the same depth, else a new one. With
    /// `below`, only lanes drawn under that lane count (a backdrop goes
    /// under its title).
    pub(super) fn room(&mut self, want: Option<u32>, at: view::Time, dur: view::Time, below: Option<u32>) -> Result<u32, String> {
        let p = &self.doc.project;
        let free = |l: u32| !p.clips.iter().any(|c| c.lane == l && c.start < at + dur && at < c.end());
        let idx = |l: u32| p.lanes.iter().position(|x| x.id == l);
        let spine_i = p.spine().and_then(idx).unwrap_or(p.lanes.len());
        if let Some(w) = want {
            if free(w) && below.is_none_or(|b| idx(w) > idx(b)) {
                return Ok(w);
            }
        }
        let floor = below.and_then(idx).map_or(0, |i| i + 1);
        // Newest on top: above everything already showing at that time
        // (unless it goes under `below`), as in every design tool.
        let top_busy = if below.is_some() {
            None
        } else {
            p.lanes.iter().enumerate().filter(|(i, l)| *i < spine_i && l.kind == view::LaneKind::Visual && !free(l.id)).map(|(i, _)| i).min()
        };
        let ceiling = top_busy.unwrap_or(spine_i);
        if let Some(l) = p.lanes.iter().enumerate().filter(|(i, l)| *i >= floor && *i < ceiling && l.kind == view::LaneKind::Visual && !l.locked && free(l.id)).map(|(_, l)| l.id).last() {
            return Ok(l);
        }
        // A new overlay lane just under `below`, or on top of what shows
        // then (or just above the main lane when nothing does).
        let index = below.and_then(idx).map_or(ceiling, |i| i + 1);
        let id = self.doc.alloc_id();
        let seq = self.doc.seq();
        let ok = self.doc.apply("Add lane", m::Edit::AddLane { seq, lane: m::Lane::new(m::LaneId(id), m::LaneKind::Visual, if below.is_some() { "Backings" } else { "Overlay" }), index: Some(index) });
        self.check(ok, "")?;
        Ok(id as u32)
    }

    /// `backing: false`: no box behind the title at all, neither one
    /// added for it nor the plate its text style draws.
    pub(super) fn no_backing(&mut self, title: u32) -> Result<(), String> {
        let old: Vec<m::ClipId> = self.doc.model.sequences.iter().flat_map(|q| q.clips.iter()).filter(|k| k.name == format!("backing for {title}")).map(|k| k.id).collect();
        if !old.is_empty() {
            let seq = self.doc.seq();
            let ok = self.doc.apply("Backing", m::Edit::Lift { seq, clips: old });
            self.check(ok, "")?;
        }
        let mut f = serde_json::Map::new();
        f.insert("generator.plate".into(), json!(false));
        self.set_properties(title, &f)?;
        Ok(())
    }

    /// A box behind a title so it reads on any picture: sized from the
    /// text, same place and time, drawn under it.
    pub(super) fn backing(&mut self, title: u32, spec: &Value) -> Result<u32, String> {
        let c = self.clip(title)?;
        let p = self.doc.project.clone();
        let colour = spec.get("colour").and_then(Value::as_str).or_else(|| spec.as_str()).unwrap_or("black");
        let k = super::craft::colour(colour).ok_or_else(|| format!("backing colour \"{colour}\" is not #rrggbb or a basic name"))?;
        let opacity = spec.get("opacity").and_then(Value::as_f64).unwrap_or(70.0);
        let size = c.extra.text_size as f64 * p.height as f64 / 1080.0 * c.transform.scale as f64 / 100.0;
        let lines: Vec<&str> = c.text.lines().collect();
        let longest = lines.iter().map(|l| l.chars().count()).max().unwrap_or(1) as f64;
        let pad = spec.get("padding").and_then(Value::as_f64).unwrap_or(size * 0.45);
        let (mut bw, mut bh) = (longest * size * 0.62 + 2.0 * pad, lines.len().max(1) as f64 * size * 1.25 + pad);
        let (mut bx, mut by) = (c.transform.x as f64, c.transform.y as f64);
        // Sized from the text as drawn: render the moment with and without
        // the title and box what changed.
        #[cfg(feature = "render")]
        {
            let t = c.start + c.dur / 2;
            if let (Ok(a), Ok(b)) = (self.frames.rgba(&self.doc, t, 480), self.frames.rgba_without(&self.doc, &[title], t, 480)) {
                if let Some(m) = super::quality::measure(&a, &b, &[]) {
                    let [x0, y0, x1, y1] = m.bbox;
                    let (cw, ch) = (p.width as f64, p.height as f64);
                    bw = (x1 - x0) * cw + 2.0 * pad;
                    bh = (y1 - y0) * ch + pad * 1.2;
                    bx = ((x0 + x1) / 2.0 - 0.5) * cw;
                    by = ((y0 + y1) / 2.0 - 0.5) * ch;
                }
            }
        }
        // One backing per title: a new one replaces the old.
        let old: Vec<m::ClipId> = self.doc.model.sequences.iter().flat_map(|q| q.clips.iter()).filter(|k| k.name == format!("backing for {title}")).map(|k| k.id).collect();
        if !old.is_empty() {
            let seq = self.doc.seq();
            let ok = self.doc.apply("Backing", m::Edit::Lift { seq, clips: old });
            self.check(ok, "")?;
        }
        let lane = self.room(None, c.start, c.dur, Some(c.lane))?;
        let id = self.doc.alloc_id() as u32;
        let mut g = m::Generator::new(m::GeneratorKind::Shape);
        g.params.insert(crate::bridge::SHAPE_KEY.into(), m::Value::Choice("rectangle".into()));
        g.params.insert("fill".into(), m::Value::Colour(m::Rgba { r: k.r() as f64 / 255.0, g: k.g() as f64 / 255.0, b: k.b() as f64 / 255.0, a: 1.0 }));
        g.params.insert("size".into(), m::Value::Point(m::Vec2 { x: (bw / p.width as f64).min(1.0), y: (bh / p.height as f64).min(1.0) }));
        g.params.insert("roundness".into(), m::Value::Scalar(spec.get("roundness").and_then(Value::as_f64).unwrap_or(0.2)));
        let mut mc = m::Clip::new(m::ClipId(id as u64), m::LaneId(lane as u64), c.start, c.dur, m::ClipSource::Generator(g));
        mc.name = format!("backing for {title}");
        mc.transform.position = m::Vec2 { x: bx, y: by };
        mc.transform.opacity = (opacity / 100.0).clamp(0.0, 1.0);
        let seq = self.doc.seq();
        let ok = self.doc.apply("Backing", m::Edit::InsertClip { seq, clip: Box::new(mc) });
        self.check(ok, "")?;
        Ok(id)
    }
}

impl Session {
    /// `motion`: a clip's motion presets, all three at once if wanted: an
    /// `entry`, an `exit` and a `loop`, each a name (catalogue motions) or
    /// `{"item", "duration", "params"}`; null removes one.
    pub(super) fn motion(&mut self, op: &Value) -> Result<String, String> {
        let id = op.get("clip").and_then(Value::as_u64).ok_or("motion needs \"clip\"")? as u32;
        let mut mc = self.model_clip(id).ok_or_else(|| format!("no clip {id}"))?;
        let mut said = Vec::new();
        for slot in ["entry", "exit", "loop"] {
            let Some(v) = op.get(slot) else { continue };
            let preset = match v {
                Value::Null | Value::Bool(false) => None,
                Value::String(_) | Value::Object(_) => {
                    let name = v.as_str().or_else(|| v.get("item").and_then(Value::as_str)).ok_or_else(|| format!("{slot} needs \"item\" (a name from catalogue motions)"))?;
                    let (_, index, found) = self.resolve(name, &[Panel::Motions])?;
                    let kind = crate::catalogue::motions()[index].id.to_owned();
                    let default = if slot == "loop" { secs(mc.duration) } else { 0.5 };
                    let dur = v.get("duration").and_then(Value::as_f64).unwrap_or(default).max(1.0 / 30.0);
                    let mut params = m::ParamMap::default();
                    if let Some(Value::Object(ps)) = v.get("params") {
                        for (k, x) in ps {
                            if let Some(n) = x.as_f64() {
                                params.insert(k.clone(), m::Value::Scalar(n));
                            }
                        }
                    }
                    said.push(format!("{slot} {found} {dur:.2}s"));
                    Some(m::MotionPreset { kind, duration: micros(dur), params })
                }
                _ => return Err(format!("{slot} is a motion name, {{\"item\", \"duration\", \"params\"}}, or null")),
            };
            if preset.is_none() {
                said.push(format!("no {slot}"));
            }
            match slot {
                "entry" => mc.motion.entry = preset,
                "exit" => mc.motion.exit = preset,
                _ => mc.motion.looping = preset,
            }
        }
        if said.is_empty() {
            return Err("motion needs entry, exit and/or loop".into());
        }
        let seq = self.doc.seq();
        let ok = self.doc.apply("Motion", m::Edit::SetClip { seq, clip: Box::new(mc) });
        self.check(ok, &format!("clip {id}: {}", said.join(", ")))
    }

    /// `merge`: clips become one clip (a compound) that moves, animates
    /// and takes effects as a unit; `unmerge` opens it back up.
    pub(super) fn merge(&mut self, op: &Value) -> Result<String, String> {
        let seq = self.doc.seq();
        if op.get("op").and_then(Value::as_str) == Some("unmerge") {
            let id = op.get("clip").and_then(Value::as_u64).ok_or("unmerge needs \"clip\"")?;
            let ok = self.doc.apply("Unmerge", m::Edit::ExpandCompound { seq, clip: m::ClipId(id) });
            if ok {
                return self.check(ok, &format!("clip {id} opened back into its clips"));
            }
            let _ = self.doc.error.take();
            // The parts do not fit back on one lane (they were stacked on
            // several): lay each on a free lane at its time.
            let mc = self.model_clip(id as u32).ok_or_else(|| format!("no clip {id}"))?;
            let m::ClipSource::Sequence(r) = &mc.source else { return Err(format!("clip {id} is not a merged clip")) };
            let inner = self.doc.model.sequence(r.sequence).ok_or("the merged clip's contents are gone")?.clone();
            let ok = self.doc.apply("Unmerge", m::Edit::Lift { seq, clips: vec![m::ClipId(id)] });
            self.check(ok, "")?;
            let audio_lane = super::craft::lane_of(&self.doc.project, &json!("sound"), None)?;
            let mut placed = Vec::new();
            let mut parts: Vec<m::Clip> = inner.clips.clone();
            // Lower lanes first, so what sat on top inside stays on top.
            parts.sort_by_key(|c| std::cmp::Reverse(inner.lane_index(c.lane).unwrap_or(0)));
            for part in parts {
                let start = mc.start + part.start - mc.source_in;
                let end = (start + part.duration).min(mc.start + mc.duration);
                if end <= start.max(mc.start) {
                    continue;
                }
                let is_audio = inner.lanes.iter().find(|l| l.id == part.lane).is_some_and(|l| l.kind == m::LaneKind::Audio);
                let lane = if is_audio { self.room(Some(audio_lane), start, end - start, None)? } else { self.room(None, start, end - start, None)? };
                let mut c = part.clone();
                c.id = m::ClipId(self.doc.alloc_id());
                c.lane = m::LaneId(lane as u64);
                c.start = start;
                c.link = None;
                c.group = None;
                let seq = self.doc.seq();
                let ok = self.doc.apply("Unmerge", m::Edit::InsertClip { seq, clip: Box::new(c.clone()) });
                self.check(ok, "")?;
                placed.push(c.id.0.to_string());
            }
            return Ok(format!("clip {id} opened into clips {}", placed.join(", ")));
        }
        let clips: Vec<m::ClipId> = match op.get("clips") {
            Some(Value::Array(a)) => a.iter().filter_map(Value::as_u64).map(m::ClipId).collect(),
            _ => Vec::new(),
        };
        if clips.len() < 2 {
            return Err("merge needs \"clips\" (two or more)".into());
        }
        let name = op.get("name").and_then(Value::as_str).unwrap_or("Merged").to_owned();
        // The middle of the parts: the merged clip turns and scales there.
        let centre = {
            let ps: Vec<m::Vec2> = clips.iter().filter_map(|c| self.model_clip(c.0 as u32)).map(|c| c.transform.position).collect();
            let n = ps.len().max(1) as f64;
            m::Vec2 { x: ps.iter().map(|p| p.x).sum::<f64>() / n, y: ps.iter().map(|p| p.y).sum::<f64>() / n }
        };
        let new_sequence = m::SequenceId(self.doc.alloc_id());
        let new_clip = m::ClipId(self.doc.alloc_id());
        let n = clips.len();
        let ok = self.doc.apply("Merge", m::Edit::CreateCompound { seq, clips, name, new_sequence, new_clip });
        self.check(ok, "")?;
        if let Some(mut mc) = self.model_clip(new_clip.0 as u32) {
            mc.transform.anchor = centre;
            mc.transform.position = m::Vec2 { x: mc.transform.position.x + centre.x, y: mc.transform.position.y + centre.y };
            let ok = self.doc.apply("Merge", m::Edit::SetClip { seq, clip: Box::new(mc) });
            self.check(ok, "")?;
        }
        Ok(format!("{n} clips are now clip {} (animate, apply effects, move it as one; it turns around its middle; unmerge opens it)", new_clip.0))
    }
}
