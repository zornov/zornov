// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The AI's 3D models are kept: after a tool call that made or changed a
//! model clip, the model is saved to the person's own models
//! (`crate::my_models`, outside the project), where the Library shows it
//! and `catalogue` finds it, so the next project (or the next request for
//! the same thing) starts from it instead of from nothing.
//!
//! A model the AI carries on with (`mesh`, `rig`, `pose` on a clip that
//! came from a kept model) replaces that entry; the same model saved twice
//! is one entry (see `my_models::save_in`).

use super::Session;
use serde_json::{Value, json};
use std::collections::HashMap;

/// Ops whose models are scenery made of many pieces, simulation frames or
/// lettering, not things to keep and use again.
const NOT_KEPT_OPS: &[&str] = &["simulate", "text_3d", "physics", "particles", "mograph"];
/// Clips the world builder names for its ground and scattered cover.
const NOT_KEPT_NAMES: &[&str] = &["Terrain", "Scatter"];

impl Session {
    /// Each model clip's file now (clip id to path).
    pub(super) fn model_files(&self) -> HashMap<u64, String> {
        self.doc.model.sequences.iter().flat_map(|q| q.clips.iter()).filter_map(|c| crate::mesh_store::file_of(c).map(|f| (c.id.0, f))).collect()
    }

    /// Keeps the models `tool` made or changed since `before`; the names
    /// kept (new or changed), for a line in the answer.
    pub(super) fn keep_models(&self, tool: &str, a: &Value, before: &HashMap<u64, String>) -> Vec<String> {
        let ops: Vec<&str> = a.get("ops").and_then(Value::as_array).map(|v| v.iter().filter_map(|o| o.get("op").and_then(Value::as_str)).collect()).unwrap_or_default();
        if ops.iter().any(|o| NOT_KEPT_OPS.contains(o)) {
            return Vec::new();
        }
        let mut kept = Vec::new();
        for c in self.doc.model.sequences.iter().flat_map(|q| q.clips.iter()) {
            let Some(file) = crate::mesh_store::file_of(c) else { continue };
            let was = before.get(&c.id.0);
            if was == Some(&file) || NOT_KEPT_NAMES.contains(&c.name.as_str()) {
                continue;
            }
            let Ok(mesh) = studio_mesh::io::load(std::path::Path::new(&file)) else { continue };
            // A clip made from a kept model carries on with that entry; the
            // project's file is named by its content hash.
            let from = was.and_then(|f| std::path::Path::new(f).file_stem()).map(|s| s.to_string_lossy().into_owned());
            match crate::my_models::keep(&c.name, &mesh, from.as_deref()) {
                Ok(s) if s.changed => {
                    // Its editable source goes with it.
                    crate::mesh_store::keep_source(&s, c);
                    kept.push(s.name)
                }
                Ok(_) => {}
                Err(e) => eprintln!("studio: could not keep model {} ({tool}): {e}", c.name),
            }
        }
        kept
    }

    /// Models already kept under words in `name`, for a nudge before the
    /// same thing is modelled again.
    pub(super) fn kept_like(&self, name: &str) -> Option<String> {
        let words: Vec<String> = name.split(|c: char| !c.is_alphanumeric()).filter(|w| w.len() > 2).map(str::to_lowercase).collect();
        if words.is_empty() {
            return None;
        }
        let hits: Vec<String> = crate::my_models::list_in(&crate::my_models::root()).into_iter().filter(|m| m.name.to_lowercase().split(|c: char| !c.is_alphanumeric()).any(|t| words.iter().any(|w| w == t))).map(|m| m.name).take(5).collect();
        (!hits.is_empty()).then(|| format!("you already have kept models like this ({}): catalogue {{panel: assets, query}} finds them and add {{item}} places one", hits.join(", ")))
    }

    /// Whether the person has kept a model with exactly this name.
    pub(super) fn has_kept_model(&self, name: &str) -> bool {
        crate::my_models::list_in(&crate::my_models::root()).iter().any(|m| m.name.eq_ignore_ascii_case(name.trim()))
    }

    /// The line added to a tool's answer when models were kept.
    pub(super) fn kept_line(names: &[String]) -> String {
        json!({"kept_models": names, "where": "your models in the Library (Assets, Models): any project can use them"}).to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::{self, library::Panel};

    fn rover(name: &str, length: u32) -> Value {
        json!({"ops": [{"op": "asset", "name": name, "parts": [
            {"name": "body", "prim": "box", "size": [length, 50, 80], "colour": "#cc2222"},
            {"name": "lid", "prim": "box", "size": [100, 10, 70], "at": [0, -30, 0], "colour": "#2222cc", "bone": {"head": [0, -30, -35], "tail": [0, -30, 35]}}]}]})
    }

    fn text_of(parts: &[super::super::Content]) -> String {
        parts.iter().filter_map(|p| if let super::super::Content::Text(t) = p { Some(t.as_str()) } else { None }).collect::<Vec<_>>().join("\n")
    }

    fn session() -> Session {
        Session::new(services::project::new_custom("Keep", 640, 360, 30.0))
    }

    fn newest_model(s: &Session) -> (u32, studio_mesh::Mesh) {
        let id = s.doc.project.clips.iter().map(|c| c.id).max().unwrap();
        (id, s.model_of(id).unwrap().1)
    }

    fn kept(name: &str) -> Vec<crate::my_models::Meta> {
        crate::my_models::list_in(&crate::my_models::root()).into_iter().filter(|m| m.name == name).collect()
    }

    #[test]
    fn a_model_the_ai_makes_is_kept_with_its_parts_materials_and_rig() {
        let mut s = session();
        let out = text_of(&s.run("edit", &rover("Zq Rover", 120)).unwrap());
        assert!(out.contains("kept_models") && out.contains("Zq Rover"), "{out}");
        let (_, mesh) = newest_model(&s);
        let metas = kept("Zq Rover");
        assert_eq!(metas.len(), 1);
        let m = &metas[0];
        assert!(m.parts.contains(&"body".to_owned()) && m.parts.contains(&"lid".to_owned()), "{m:?}");
        assert!(m.bones.contains(&"lid".to_owned()) && m.bones.contains(&"root".to_owned()), "{m:?}");
        // Geometry, groups, materials and rig come back exactly.
        let back = crate::my_models::load_in(&crate::my_models::root(), &m.slug).unwrap();
        assert_eq!(back, mesh);
        assert!(back.rig.is_some() && back.materials.len() >= 2);
        // Making it again (even in another session) keeps one entry.
        let mut again = session();
        again.run("edit", &rover("Zq Rover", 120)).unwrap();
        assert_eq!(kept("Zq Rover").len(), 1);
    }

    #[test]
    fn carrying_on_with_a_kept_model_updates_it_and_does_not_add_another() {
        let mut s = session();
        s.run("edit", &rover("Zq Lineage", 130)).unwrap();
        let (id, before) = newest_model(&s);
        s.run("edit", &json!({"ops": [{"op": "mesh", "clip": id, "steps": [{"do": "subdivide"}]}]})).unwrap();
        let (_, after) = newest_model(&s);
        assert!(after.triangles() > before.triangles());
        let metas = kept("Zq Lineage");
        assert_eq!(metas.len(), 1, "{metas:?}");
        assert_eq!(metas[0].triangles, after.triangles());
        assert!(after.rig.is_some());
    }

    #[test]
    fn add_and_place_find_a_kept_model_by_name_without_a_panel() {
        let mut s = session();
        s.run("edit", &rover("Zq Named", 150)).unwrap();
        let (_, mesh) = newest_model(&s);
        let mut other = session();
        let r = other.run("edit", &json!({"ops": [{"op": "add", "item": "Zq Named"}]}));
        assert!(r.is_ok(), "{:?}", r.as_ref().err());
        assert_eq!(newest_model(&other).1, mesh);
        let mut third = session();
        let r = third.run("edit", &json!({"ops": [{"op": "place", "media": "zq named", "at": 0}]}));
        assert!(r.is_ok(), "{:?}", r.as_ref().err());
        assert_eq!(newest_model(&third).1, mesh);
        // A name that is nothing still says so.
        assert!(third.run("edit", &json!({"ops": [{"op": "place", "media": "no such thing zz"}]})).is_err());
    }

    #[test]
    fn a_kept_model_is_in_the_library_and_the_catalogue_and_loads_into_another_project() {
        let mut s = session();
        s.run("edit", &rover("Zq Gadget", 140)).unwrap();
        let (_, mesh) = newest_model(&s);
        // The Library's Assets panel lists it.
        let _ = crate::content::get_blocking();
        let doc = &s.doc;
        let found = services::library::search(doc, Panel::Assets, 0, "zq gadget");
        assert_eq!(found.len(), 1);
        let item = services::library::item(doc, Panel::Assets, found[0] as usize);
        assert_eq!(item.name, "Zq Gadget");
        assert!(item.description.contains("rigged with") && item.description.contains("lid"), "{}", item.description);
        let models = services::library::search(doc, Panel::Assets, 11, "zq gadget");
        assert_eq!(models.len(), 1);
        assert!(matches!(services::library::thumb(doc, Panel::Assets, found[0] as usize), services::library::Thumb::Content { .. }));
        // The AI's catalogue finds it, and the overview counts it.
        let cat = text_of(&s.run("catalogue", &json!({"panel": "assets", "query": "gadget"})).unwrap());
        assert!(cat.contains("Zq Gadget"), "{cat}");
        let all = text_of(&s.run("catalogue", &json!({})).unwrap());
        assert!(all.contains("kept_models") && all.contains("Zq Gadget"), "{all}");
        // Another project adds it from the Library and gets the same model.
        let mut other = session();
        let out = services::library::add(&mut other.doc, Panel::Assets, found[0] as usize, 0, None).unwrap();
        let (_, there) = (out.select[0], other.model_of(out.select[0]).unwrap().1);
        assert_eq!(there, mesh);
        // And the AI can place it by name.
        let mut third = session();
        let r = text_of(&third.run("edit", &json!({"ops": [{"op": "add", "item": "Zq Gadget", "panel": "assets"}]})).unwrap());
        assert!(!r.contains("error"), "{r}");
        assert_eq!(third.model_of(third.doc.project.clips.iter().map(|c| c.id).max().unwrap()).unwrap().1, mesh);
    }
}
