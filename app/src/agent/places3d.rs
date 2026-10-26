// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Places and objects as ONE model, not forty layers.
//!
//! - `build_place`: a building, facade, street, interior, bridge, fence or
//!   stairs generated in a style (classical, art deco, brutalist, modern
//!   glass, medieval, oriental, futuristic, rustic) with a wear, as one
//!   model clip with named parts (walls, window frames, window glass,
//!   cornice, kerbs, lamps...). Windows sit in real openings; nothing is a
//!   cube per pilaster.
//! - `combine`: merges several model clips into one model clip with
//!   named parts (each clip becomes a part named for it; materials,
//!   bones and pivots kept), so a place somebody blocked out in many
//!   clips becomes one clip.
//! - `uncombine`: the reverse; a part, several or all of them go back to
//!   clips of their own, still where they were.

use super::models::{describe, field_v3, num};
use super::*;
use studio_mesh::Mesh;
use studio_mesh::group::{self, PartIn, Xf3};
use studio_modeling::places::{self, Spec, Style};

fn clip_xf(c: &m::Clip) -> Xf3 {
    let w = super::sim3d::model_world(&c.transform);
    std::array::from_fn(|r| std::array::from_fn(|k| w[r][k] as f32))
}

/// A model clip's mesh with its clip, and what stops it being combined.
struct Source {
    id: u32,
    clip: m::Clip,
    mesh: Mesh,
}

fn kinds_said() -> String {
    places::KINDS.iter().map(|k| format!("{} ({})", k.0, k.1)).collect::<Vec<_>>().join("; ")
}

fn styles_said() -> String {
    places::STYLES.iter().map(|s| s.0).collect::<Vec<_>>().join(", ")
}

/// Whether a model clip is animated: keys on its bones, a running action, or
/// keys on its placement.
fn animated(c: &m::Clip) -> bool {
    !c.keyframes.is_empty() || matches!(&c.source, m::ClipSource::Generator(g) if g.params.contains_key("action"))
}

impl Session {
    /// `build_place`: see the module notes.
    pub(super) fn build_place(&mut self, op: &Value) -> Result<String, String> {
        let kind = op.get("kind").and_then(Value::as_str).ok_or_else(|| format!("build_place needs \"kind\": {}", kinds_said()))?.to_lowercase();
        let style = match op.get("style").and_then(Value::as_str) {
            Some(s) => Style::parse(s).ok_or_else(|| format!("style \"{s}\": {}", styles_said()))?,
            None => Style::Classical,
        };
        let mut spec = Spec::new(&kind, style);
        if let Some(v) = num(op, "size") {
            spec.size = v.max(50.0);
        }
        if let Some(v) = num(op, "wear") {
            spec.wear = v.clamp(0.0, 1.0);
        }
        spec.floors = op.get("floors").and_then(Value::as_u64).map(|v| v as usize);
        spec.width = num(op, "width");
        spec.depth = num(op, "depth");
        spec.seed = op.get("seed").and_then(Value::as_u64).unwrap_or(1);
        spec.detail = op.get("detail").and_then(Value::as_u64).unwrap_or(2).clamp(1, 3) as u8;
        spec.lit = num(op, "lit").unwrap_or(0.0).clamp(0.0, 1.0);
        spec.textured = op.get("textured").and_then(Value::as_bool).unwrap_or(false);
        spec.variant = op.get("variant").and_then(Value::as_str).map(str::to_owned);
        let (asset, mut mesh) = places::build_mesh(&spec).map_err(|e| format!("{e}; kinds: {}", kinds_said()))?;
        // 100 px to the metre unless asked; the rest of Studio's 3D sizes people at about 475 px a metre.
        let ppm = num(op, "px_per_metre").unwrap_or(100.0).clamp(1.0, 2000.0);
        if (ppm - 100.0).abs() > 0.01 {
            let k = ppm / 100.0;
            for p in &mut mesh.positions {
                *p = p.map(|v| v * k);
            }
        }
        let name = op.get("name").and_then(Value::as_str).map(str::to_owned).unwrap_or_else(|| format!("{} {}", style.name().replace('_', " "), kind));
        let names: Vec<String> = asset.parts.iter().map(|p| p.name.clone()).collect();
        let shown = names.iter().take(14).cloned().collect::<Vec<_>>().join(", ");
        let notes = vec![
            format!("one clip, {} named parts: {}{}", names.len(), shown, if names.len() > 14 { ", ..." } else { "" }),
            format!("style {}, wear {:.0}%, {} px to the metre; a person is about {:.0} px tall here", style.name(), spec.wear * 100.0, ppm, 1.7 * ppm),
            "the parts are named by their materials; `mesh` `material` with `region` changes one; `uncombine` splits them into clips".to_owned(),
        ];
        self.place_model(op, &mesh, name, notes)
    }

    /// Source clips for `combine`: listed ids or names, clips whose name
    /// holds a word (`like`), or every plain model (`all`).
    fn combine_sources(&self, op: &Value) -> Result<(Vec<Source>, Vec<String>), String> {
        let mut ids: Vec<u32> = Vec::new();
        let mut said: Vec<String> = Vec::new();
        let explicit = match op.get("clips") {
            Some(Value::Array(a)) => a.iter().filter_map(|v| super::sim3d::id_of(&self.doc, v)).collect::<Vec<_>>(),
            Some(v) => super::sim3d::id_of(&self.doc, v).map(|x| vec![x]).unwrap_or_default(),
            None => Vec::new(),
        };
        ids.extend(explicit.iter().copied());
        let like: Vec<String> = match op.get("like") {
            Some(Value::String(s)) => vec![s.to_lowercase()],
            Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).map(str::to_lowercase).collect(),
            _ => Vec::new(),
        };
        let all = op.get("all").and_then(Value::as_bool) == Some(true);
        if !like.is_empty() || all {
            let seq = self.doc.model.sequence(self.doc.model.active_sequence).ok_or("no active sequence")?;
            for c in &seq.clips {
                if crate::mesh_store::file_of(c).is_none() || ids.contains(&(c.id.0 as u32)) {
                    continue;
                }
                let n = c.name.to_lowercase();
                let hit = all || like.iter().any(|w| n.contains(w.as_str()));
                if !hit {
                    continue;
                }
                // Characters and animated parts stay as they are unless named.
                let rigged = crate::mesh_store::load(c).map(|mesh| mesh.rig.is_some()).unwrap_or(false);
                if animated(c) || rigged {
                    said.push(format!("left out clip {} \"{}\" ({})", c.id.0, c.name, if rigged { "it has bones" } else { "it is animated" }));
                    continue;
                }
                ids.push(c.id.0 as u32);
            }
        }
        if ids.len() < 2 {
            return Err("combine needs at least two model clips: \"clips\": [ids or names], or \"like\": [\"window\", \"cornice\"] (names containing a word), or \"all\": true (every plain model)".into());
        }
        let mut out = Vec::new();
        for id in ids {
            let (clip, mesh) = self.model_of(id)?;
            if animated(&clip) && !explicit.contains(&id) {
                continue;
            }
            if animated(&clip) {
                return Err(format!("clip {id} \"{}\" is animated (keys or a running action), which a combined model would lose; key the group clip instead, or leave clip {id} out of the list (it can stay its own clip beside the rest)", clip.name));
            }
            out.push(Source { id, clip, mesh });
        }
        Ok((out, said))
    }

    /// `combine`: see the module notes.
    pub(super) fn combine(&mut self, op: &Value) -> Result<String, String> {
        let (sources, mut said) = self.combine_sources(op)?;
        if sources.len() < 2 {
            return Err("fewer than two clips could be combined".into());
        }
        // World placements; the group sits at the middle of what it holds.
        let xfs: Vec<Xf3> = sources.iter().map(|s| clip_xf(&s.clip)).collect();
        let mut lo = [f32::INFINITY; 3];
        let mut hi = [f32::NEG_INFINITY; 3];
        for (s, x) in sources.iter().zip(&xfs) {
            let (a, b) = s.mesh.bounds();
            for k in 0..8 {
                let c = [if k & 1 == 0 { a[0] } else { b[0] }, if k & 2 == 0 { a[1] } else { b[1] }, if k & 4 == 0 { a[2] } else { b[2] }];
                let w: [f32; 3] = std::array::from_fn(|r| x[r][0] * c[0] + x[r][1] * c[1] + x[r][2] * c[2] + x[r][3]);
                for i in 0..3 {
                    lo[i] = lo[i].min(w[i]);
                    hi[i] = hi[i].max(w[i]);
                }
            }
        }
        let centre = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, (lo[2] + hi[2]) / 2.0];
        let parts: Vec<PartIn> = sources
            .iter()
            .zip(&xfs)
            .map(|(s, x)| {
                let mut x = *x;
                for r in 0..3 {
                    x[r][3] -= centre[r];
                }
                PartIn { name: &s.clip.name, mesh: &s.mesh, xf: x }
            })
            .collect();
        let (mesh, names) = group::combine(&parts)?;
        if mesh.triangles() > 1_500_000 {
            return Err(format!("the combined model would have {} triangles, over 1500000; combine fewer or decimate first", mesh.triangles()));
        }
        let first = &sources[0].clip;
        let start = sources.iter().map(|s| s.clip.start).min().unwrap_or(first.start);
        let end = sources.iter().map(|s| s.clip.start + s.clip.duration).max().unwrap_or(first.start + first.duration);
        if sources.iter().any(|s| s.clip.start != start || s.clip.start + s.clip.duration != end) {
            said.push(format!("the clips ran for different times; the group shows {:.1}-{:.1} s (the union)", secs(start), secs(end)));
        }
        let file = crate::mesh_store::save(&self.doc, &mesh)?;
        let lane0 = first.lane.0 as u32;
        let name = op.get("name").and_then(Value::as_str).map(str::to_owned).unwrap_or_else(|| {
            let common = first.name.split_whitespace().next().unwrap_or("Model").to_owned();
            format!("{common} set")
        });
        let seq = self.doc.seq();
        // The old clips go, then the group takes the first one's lane if it is free.
        let removed: Vec<u32> = sources.iter().map(|s| s.id).collect();
        for id in &removed {
            let ok = self.doc.apply("Combine models", m::Edit::RemoveClip { seq, clip: m::ClipId(*id as u64) });
            self.check(ok, "")?;
        }
        let at = op.get("at").and_then(Value::as_f64).map_or(start, micros);
        let dur = op.get("duration").and_then(Value::as_f64).map_or(end - start, micros).max(SEC / 10);
        let lane = self.room(Some(lane0), at, dur, None)?;
        let id = self.doc.alloc_id() as u32;
        let mut c = crate::mesh_store::new_clip(id as u64, lane as u64, at, dur, file, &name);
        c.transform.position = m::Vec2::new(centre[0] as f64, centre[1] as f64);
        c.transform.position_z = centre[2] as f64;
        if first.material.is_some() {
            c.material = first.material.clone();
        }
        let ok = self.doc.apply("Combine models", m::Edit::InsertClip { seq, clip: Box::new(c) });
        self.check(ok, "")?;
        let shown = names.iter().take(16).cloned().collect::<Vec<_>>().join(", ");
        let mut out = format!("model clip {id} \"{name}\": {} clips became one model with {} named parts ({}{}); {}", removed.len(), names.len(), shown, if names.len() > 16 { ", ..." } else { "" }, describe(&mesh));
        if sources.iter().any(|s| s.mesh.rig.is_some()) {
            out.push_str("; bones and pivots kept as \"Part/bone\" (shape keys and actions of the parts are not carried)");
        }
        if !said.is_empty() {
            out.push_str(&format!("; {}", said.join("; ")));
        }
        Ok(out)
    }

    /// `uncombine`: a combined model back into clips (all its parts, or the
    /// ones named in `parts`; the rest stay together in the group).
    pub(super) fn uncombine(&mut self, op: &Value) -> Result<String, String> {
        let id = op.get("clip").and_then(|v| super::sim3d::id_of(&self.doc, v)).ok_or("uncombine needs \"clip\" (a model clip made of parts)")?;
        let (clip, mesh) = self.model_of(id)?;
        let all_parts = group::parts_of(&mesh);
        if all_parts.len() < 2 {
            return Err(format!("clip {id} is one part; nothing to separate (parts are the model's materials, named \"Part\" or \"Part/material\")"));
        }
        let wanted: Vec<String> = match op.get("parts") {
            Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).map(str::to_owned).collect(),
            Some(Value::String(s)) => vec![s.clone()],
            _ => all_parts.iter().map(|p| p.0.clone()).collect(),
        };
        for w in &wanted {
            if !all_parts.iter().any(|p| &p.0 == w) {
                return Err(format!("no part \"{w}\" in clip {id}; its parts: {}", all_parts.iter().map(|p| p.0.as_str()).collect::<Vec<_>>().join(", ")));
            }
        }
        let rest: Vec<String> = all_parts.iter().map(|p| p.0.clone()).filter(|n| !wanted.contains(n)).collect();
        let seq = self.doc.seq();
        // The rest stay in the group clip; with nothing left, the group goes.
        if rest.is_empty() {
            let ok = self.doc.apply("Separate parts", m::Edit::RemoveClip { seq, clip: m::ClipId(id as u64) });
            self.check(ok, "")?;
        } else {
            let remaining: Vec<group::PartOut> = rest.iter().map(|n| group::take(&mesh, n)).collect();
            let inputs: Vec<PartIn> = remaining.iter().map(|p| PartIn { name: &p.name, mesh: &p.mesh, xf: group::IDENTITY }).collect();
            let (rest_mesh, _) = group::combine(&inputs)?;
            let file = crate::mesh_store::save(&self.doc, &rest_mesh)?;
            let mut mc = clip.clone();
            crate::mesh_store::set_file(&mut mc, file);
            let ok = self.doc.apply("Separate parts", m::Edit::SetClip { seq, clip: Box::new(mc) });
            self.check(ok, "")?;
        }
        let (at, dur) = (clip.start, clip.duration);
        let mut made: Vec<String> = Vec::new();
        for n in &wanted {
            let part = group::take(&mesh, n);
            let file = crate::mesh_store::save(&self.doc, &part.mesh)?;
            let lane = self.room(None, at, dur, None)?;
            let nid = self.doc.alloc_id() as u32;
            let mut c = crate::mesh_store::new_clip(nid as u64, lane as u64, at, dur, file, &part.name);
            // Where the group was, so nothing moves.
            c.transform = clip.transform.clone();
            c.material = clip.material.clone();
            let ok = self.doc.apply("Separate parts", m::Edit::InsertClip { seq, clip: Box::new(c) });
            self.check(ok, "")?;
            made.push(format!("{nid} \"{}\"", part.name));
        }
        let shown = made.iter().take(20).cloned().collect::<Vec<_>>().join(", ");
        Ok(format!("{} part{} are clips of their own now: {}{}{}", made.len(), if made.len() == 1 { "" } else { "s" }, shown, if made.len() > 20 { ", ..." } else { "" }, if rest.is_empty() { String::new() } else { format!("; {} other parts stay in clip {id}", rest.len()) }))
    }
}

/// The offset a part keeps in its group (used by tests).
#[allow(dead_code)]
fn _unused(op: &Value) -> Option<[f32; 3]> {
    field_v3(op, "position").ok().flatten()
}

#[cfg(test)]
mod tests;
