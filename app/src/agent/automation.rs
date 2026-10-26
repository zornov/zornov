// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The `automation` tool: macros (record a sequence of edits, replay it on
//! other clips), batch runs over many clips or many projects, and the
//! person's keyboard shortcuts and workspaces.

use std::path::PathBuf;

use serde_json::{Value, json};

use super::{Content, Session};
use crate::macros;

fn s<'a>(a: &'a Value, k: &str) -> Option<&'a str> {
    a.get(k).and_then(Value::as_str)
}

/// Clips named by `clips`: ids, or "all" (every picture clip), "main"
/// (the main lane), "video", "titles", "sound".
fn targets(p: &crate::view::Project, v: Option<&Value>) -> Result<Vec<u32>, String> {
    use crate::view::ClipKind;
    match v {
        None => Ok(Vec::new()),
        Some(Value::Array(a)) => Ok(a.iter().filter_map(Value::as_u64).map(|x| x as u32).collect()),
        Some(Value::String(w)) => {
            let spine = p.spine();
            let keep = |c: &&crate::view::Clip| match w.as_str() {
                "all" => c.is_visual(),
                "main" => Some(c.lane) == spine,
                "video" => matches!(c.kind, ClipKind::Video),
                "titles" | "text" => matches!(c.kind, ClipKind::Title),
                "sound" | "audio" => !c.is_visual(),
                _ => false,
            };
            if !matches!(w.as_str(), "all" | "main" | "video" | "titles" | "text" | "sound" | "audio") {
                return Err(format!("clips \"{w}\": ids, or all, main, video, titles, sound"));
            }
            Ok(p.clips.iter().filter(keep).map(|c| c.id).collect())
        }
        Some(_) => Err("clips: a list of ids or one of all, main, video, titles, sound".into()),
    }
}

impl Session {
    pub(super) fn automation_tool(&mut self, a: &Value) -> Result<Vec<Content>, String> {
        let text = |v: Value| Ok(vec![Content::Text(v.to_string())]);
        match s(a, "action").unwrap_or("macros") {
            "macros" => text(json!({"macros": macros::list().iter().map(macros::json).collect::<Vec<_>>(), "recording": macros::recording()})),
            "record" => {
                let subjects = targets(&self.doc.project, a.get("clips"))?;
                macros::start(&self.doc, &subjects);
                text(json!({"recording": true, "on": subjects, "next": "make the changes with edit, then automation {\"action\":\"stop\",\"name\":...}"}))
            }
            "stop" => {
                let name = s(a, "name").ok_or("stop needs \"name\" for the macro")?;
                let m = macros::stop(name, s(a, "description").unwrap_or("")).ok_or("nothing is being recorded")?;
                if m.steps.is_empty() {
                    return Err("nothing changed while recording, so there is no macro".into());
                }
                let path = macros::save(&m)?;
                text(json!({"saved": macros::json(&m), "file": path.display().to_string()}))
            }
            "run" => {
                let name = s(a, "name").ok_or("run needs \"name\"")?;
                let m = macros::load(name)?;
                let t = targets(&self.doc.project, a.get("clips"))?;
                let (applied, refused) = macros::run(&mut self.doc, &m, &t)?;
                text(json!({"ran": name, "on": t, "changes": applied, "skipped": refused, "revision": self.doc.history.revision()}))
            }
            "delete" => {
                let name = s(a, "name").ok_or("delete needs \"name\"")?;
                macros::delete(name)?;
                text(json!({"deleted": name}))
            }
            "batch" => self.batch(a),
            "shortcuts" => text(json!({
                "shortcuts": crate::screens::settings::SHORTCUTS.iter().map(|(id, name, default)| json!({"action": id, "name": name, "default": default})).collect::<Vec<_>>(),
                "note": "the person changes them in Settings ▸ Shortcuts (clashes are shown there)",
            })),
            "workspaces" => {
                // A saved layout can be removed; built-in ones go back to how they came.
                if let Some(name) = s(a, "delete") {
                    crate::workspaces::delete(name)?;
                }
                text(json!({"workspaces": crate::workspaces::all(), "note": "switch from the Layout menu in the window or with ⌥1-⌥4"}))
            }
            other => Err(format!("automation action \"{other}\": macros, record, stop, run, delete, batch, shortcuts, workspaces")),
        }
    }

    /// The same work over many clips of this project, or over many
    /// project folders: a macro and/or edit ops, then optionally an
    /// export, each project saved.
    fn batch(&mut self, a: &Value) -> Result<Vec<Content>, String> {
        let mac = match s(a, "macro") {
            Some(n) => Some(macros::load(n)?),
            None => None,
        };
        let ops = a.get("ops").filter(|o| o.as_array().is_some_and(|x| !x.is_empty())).cloned();
        let export = a.get("export").cloned();
        if mac.is_none() && ops.is_none() && export.is_none() {
            return Err("batch needs \"macro\", \"ops\" or \"export\" (and \"projects\" for other projects)".into());
        }
        let projects: Vec<PathBuf> = a.get("projects").and_then(Value::as_array).map(|v| v.iter().filter_map(Value::as_str).map(PathBuf::from).collect()).unwrap_or_default();
        if projects.is_empty() {
            return Ok(vec![Content::Text(self.batch_here(mac.as_ref(), ops.as_ref(), export.as_ref(), a.get("clips"))?.to_string())]);
        }
        let mut report = Vec::new();
        for dir in &projects {
            let r = Session::at(dir).and_then(|mut s| {
                let v = s.batch_here(mac.as_ref(), ops.as_ref(), export.as_ref(), a.get("clips"))?;
                crate::services::project::save(&mut s.doc, None)?;
                Ok(v)
            });
            report.push(match r {
                Ok(v) => json!({"project": dir.display().to_string(), "ok": true, "result": v}),
                Err(e) => json!({"project": dir.display().to_string(), "ok": false, "error": e}),
            });
        }
        let failed = report.iter().filter(|r| r["ok"] != true).count();
        Ok(vec![Content::Text(json!({"projects": report, "done": projects.len() - failed, "failed": failed}).to_string())])
    }

    fn batch_here(&mut self, mac: Option<&macros::Macro>, ops: Option<&Value>, export: Option<&Value>, clips: Option<&Value>) -> Result<Value, String> {
        let mut out = json!({});
        if let Some(m) = mac {
            let t = targets(&self.doc.project, clips.or(Some(&json!("all"))))?;
            let (applied, refused) = macros::run(&mut self.doc, m, &t)?;
            out["macro"] = json!({"name": m.name, "clips": t.len(), "changes": applied, "skipped": refused});
        }
        if let Some(ops) = ops {
            let r = self.call("edit", &json!({"ops": ops, "label": "Batch"}))?;
            out["edit"] = r.iter().find_map(|c| if let Content::Text(t) = c { serde_json::from_str(t).ok() } else { None }).unwrap_or(Value::Null);
        }
        if let Some(e) = export {
            let mut args = if e.is_object() { e.clone() } else { json!({"preset": e}) };
            args["wait"] = json!(true);
            let r = self.call("export", &args)?;
            out["export"] = r.iter().find_map(|c| if let Content::Text(t) = c { serde_json::from_str(t).ok() } else { None }).unwrap_or(Value::Null);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services;

    #[test]
    fn a_macro_recorded_on_one_clip_replays_on_another_as_one_step() {
        let mut s = Session::new(services::project::new_custom("Macro", 640, 360, 30.0));
        s.edit(&[json!({"op": "card", "colour": "#ffcc00", "at": 0, "duration": 4, "width": 320, "height": 180}), json!({"op": "card", "colour": "#0044ff", "at": 0, "duration": 3, "width": 200, "height": 100})], "cards", false).unwrap();
        let mut ids: Vec<u32> = s.doc.project.clips.iter().map(|c| c.id).collect();
        ids.sort();
        let (a, b) = (ids[0], ids[1]);
        let b_before = s.model_clip(b).unwrap();

        macros::start(&s.doc, &[a]);
        s.edit(&[json!({"op": "apply", "item": "gaussian blur", "clips": [a]})], "look", false).unwrap();
        s.edit(&[json!({"op": "effect", "clip": a, "effect": "gaussian_blur", "params": {"radius": 12}})], "look", false).unwrap();
        let mac = macros::stop("Soft", "blur 12").unwrap();
        assert!(!macros::recording());
        // Adding the effect changes the clip; its settings are an effect edit.
        assert!(matches!(mac.steps[0], macros::Step::ClipChange { .. }), "{:?}", mac.steps);

        let rev = s.doc.history.revision();
        let (applied, refused) = macros::run(&mut s.doc, &mac, &[b]).unwrap();
        assert!(applied > 0 && refused.is_empty(), "{refused:?}");
        let (ca, cb) = (s.model_clip(a).unwrap(), s.model_clip(b).unwrap());
        assert_eq!(cb.effects.len(), 1);
        assert_eq!(cb.effects[0].kind, "gaussian_blur");
        assert_eq!(cb.effects[0].params.get("radius"), ca.effects[0].params.get("radius"));
        assert_ne!(cb.effects[0].id, ca.effects[0].id, "its own effect id");
        // Where B is and what it shows are its own.
        assert_eq!((cb.start, cb.duration, cb.lane, &cb.source), (b_before.start, b_before.duration, b_before.lane, &b_before.source));
        // One undo step takes it all back.
        assert!(s.doc.history.revision() > rev);
        s.doc.undo();
        assert!(s.model_clip(b).unwrap().effects.is_empty());
        assert_eq!(s.model_clip(a).unwrap().effects.len(), 1, "the subject keeps its own");
    }

    #[test]
    fn targets_by_word() {
        let mut s = Session::new(services::project::new_custom("T", 640, 360, 30.0));
        s.edit(&[json!({"op": "card", "colour": "black", "at": 0, "duration": 2, "lane": "main"}), json!({"op": "card", "colour": "white", "at": 0, "duration": 1})], "c", false).unwrap();
        assert_eq!(targets(&s.doc.project, Some(&json!("all"))).unwrap().len(), 2);
        assert_eq!(targets(&s.doc.project, Some(&json!("main"))).unwrap().len(), 1);
        assert!(targets(&s.doc.project, Some(&json!("everything"))).is_err());
    }
}
