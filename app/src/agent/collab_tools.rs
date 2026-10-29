// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The team tools: `comments` (review notes), `versions`, `share`
//! (review packages, merging copies, syncing through a shared folder) and
//! `team` (roles and locks). The work is in [`crate::collab`]; these read
//! the arguments and answer in small JSON.

use super::{Session, f, micros, secs};
use crate::collab::{self, comments, delta, package, sync, team, versions};
use crate::collab::sync::Remote as _;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use studio_model as m;

fn st<'a>(a: &'a Value, k: &str) -> Option<&'a str> {
    a.get(k).and_then(Value::as_str).filter(|s| !s.trim().is_empty())
}

impl Session {
    /// The project folder notes, versions and the team live in.
    fn collab_root(&self) -> Result<PathBuf, String> {
        self.home.clone().or_else(|| self.doc.folder.as_ref().map(|f| f.root().to_path_buf())).ok_or_else(|| "save the project first: notes, versions and the team are kept in its folder".to_owned())
    }

    /// Who the call is signed by: `as`, else the person at this computer,
    /// marked as the AI working for them.
    fn signer(&self, a: &Value) -> String {
        st(a, "as").map(str::to_owned).unwrap_or_else(|| format!("{} (AI)", collab::identity()))
    }

    /// The team's rules for this person, again (after the team changed).
    fn reguard(&mut self, root: &Path) {
        self.doc.guard = collab::team::guard_in(root);
    }

    pub(super) fn collab_tool(&mut self, tool: &str, a: &Value) -> Result<Value, String> {
        match tool {
            "comments" => self.comments_tool(a),
            "versions" => self.versions_tool(a),
            "share" => self.share_tool(a),
            "team" => self.team_tool(a),
            _ => Err(format!("unknown tool {tool}")),
        }
    }

    /// What a note is about, for an agent about to act on it: the clips
    /// under it (or its own clip) with their lanes.
    fn note_context(&self, c: &comments::Comment) -> Value {
        let (t, _) = c.time_in(&self.doc.model);
        let p = &self.doc.project;
        let mut under: Vec<Value> = p
            .clips
            .iter()
            .filter(|x| c.clip.map_or(x.start <= t && t < x.end(), |id| id == x.id as u64))
            .map(|x| json!({"id": x.id, "name": x.name, "lane": p.lane(x.lane).map(|l| l.name.clone()).unwrap_or_default(), "start": secs(x.start), "end": secs(x.end())}))
            .collect();
        under.truncate(6);
        let mut line = comments::line(c, &self.doc.model);
        line["under"] = json!(under);
        line
    }

    fn comments_tool(&mut self, a: &Value) -> Result<Value, String> {
        let root = self.collab_root()?;
        let mut store = comments::Store::load(&root)?;
        let action = st(a, "action").unwrap_or("list").to_lowercase();
        let who = self.signer(a);
        let ids: Vec<String> = match a.get("id").or_else(|| a.get("ids")) {
            Some(Value::Array(v)) => v.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect(),
            Some(Value::String(s)) if s == "all" => store.comments.iter().filter(|c| c.is_open()).map(|c| c.id.clone()).collect(),
            Some(Value::String(s)) => vec![s.clone()],
            _ => Vec::new(),
        };
        let need_ids = || if ids.is_empty() { Err(format!("{action} needs \"id\" (a note id from comments list)")) } else { Ok(()) };
        let note = st(a, "note").or_else(|| st(a, "text"));
        let out = match action.as_str() {
            "list" | "read" | "show" => {
                let filter = comments::Filter::parse(st(a, "status").unwrap_or("open"))?;
                let mut list: Vec<&comments::Comment> = store.list(&self.doc.model, filter);
                if let Some(c) = a.get("clip").and_then(Value::as_u64) {
                    list.retain(|x| x.clip == Some(c));
                }
                if let (Some(from), Some(to)) = (f(a, "from"), f(a, "to")) {
                    let (from, to) = (micros(from), micros(to));
                    list.retain(|x| (from..=to).contains(&x.time_in(&self.doc.model).0));
                }
                let lines: Vec<Value> = list.iter().map(|c| self.note_context(c)).collect();
                return Ok(json!({"open": store.open_count(), "shown": lines.len(), "notes": lines}));
            }
            "add" | "new" | "note" => {
                let text = note.ok_or("add needs \"note\": what the note says")?;
                let clip = a.get("clip").and_then(Value::as_u64);
                let at = match (f(a, "at"), clip) {
                    (Some(t), _) => micros(t),
                    (None, Some(id)) => self.doc.model.active().clip(m::ClipId(id)).map(|c| c.start).ok_or_else(|| format!("no clip {id}"))?,
                    (None, None) => return Err("add needs \"at\" (seconds) or \"clip\"".into()),
                };
                let region = match a.get("region") {
                    Some(Value::Array(r)) if r.len() == 4 => {
                        let v: Vec<f32> = r.iter().map(|x| x.as_f64().unwrap_or(0.0) as f32).collect();
                        Some([v[0].min(v[2]), v[1].min(v[3]), v[0].max(v[2]), v[1].max(v[3])])
                    }
                    Some(_) => return Err("region is [x0, y0, x1, y1] in canvas pixels from the centre, y down".into()),
                    None => None,
                };
                let version = versions::Index::load(&root)?.current;
                let id = store.add(&self.doc.model, &who, text, comments::Pin { at, until: f(a, "until").map(micros), clip, region }, version)?;
                json!({"added": id, "open": store.open_count()})
            }
            "reply" => {
                need_ids()?;
                let text = note.ok_or("reply needs \"note\"")?;
                let mut done = Vec::new();
                for id in &ids {
                    done.push(store.reply(id, &who, text)?);
                }
                json!({"replied": ids, "open": store.open_count()})
            }
            "resolve" | "done" | "close" => {
                need_ids()?;
                for id in &ids {
                    store.resolve(id, &who, note)?;
                }
                json!({"resolved": ids, "open": store.open_count()})
            }
            "reopen" => {
                need_ids()?;
                for id in &ids {
                    store.reopen(id)?;
                }
                json!({"reopened": ids, "open": store.open_count()})
            }
            "edit" => {
                need_ids()?;
                let text = note.ok_or("edit needs \"note\": the new text")?;
                for id in &ids {
                    store.edit_text(id, text)?;
                }
                json!({"edited": ids})
            }
            "delete" | "remove" => {
                need_ids()?;
                for id in &ids {
                    store.delete(id)?;
                }
                json!({"deleted": ids, "open": store.open_count()})
            }
            "export" => {
                let format = st(a, "format").unwrap_or("markdown");
                let filter = comments::Filter::parse(st(a, "status").unwrap_or("all"))?;
                let text = store.export(&self.doc.model, filter, format)?;
                return Ok(match st(a, "path") {
                    Some(p) => {
                        std::fs::write(p, &text).map_err(|e| format!("could not write {p}: {e}"))?;
                        json!({"written": p, "bytes": text.len()})
                    }
                    None => json!({"format": format, "text": text}),
                });
            }
            "import" => {
                let mut n = 0;
                let mut from = Vec::new();
                let author = st(a, "as").unwrap_or("Reviewer");
                match (st(a, "path"), note) {
                    (Some(p), _) => {
                        for (file, text) in package::read_back(Path::new(p))? {
                            n += store.import(&self.doc.model, &text, author)?;
                            from.push(file.display().to_string());
                        }
                    }
                    (None, Some(text)) => n += store.import(&self.doc.model, text, author)?,
                    (None, None) => return Err("import needs \"path\" (a notes file or a review package folder) or \"note\" (the notes as text)".into()),
                }
                json!({"imported": n, "from": from, "open": store.open_count()})
            }
            o => return Err(format!("comments action \"{o}\": list, add, reply, resolve, reopen, edit, delete, export, import")),
        };
        store.save(&root)?;
        Ok(out)
    }

    fn versions_tool(&mut self, a: &Value) -> Result<Value, String> {
        let root = self.collab_root()?;
        let action = st(a, "action").unwrap_or("list").to_lowercase();
        let who = self.signer(a);
        let load = |key: &str, now: &m::Project| -> Result<(String, m::Project), String> {
            if matches!(key.trim().to_lowercase().as_str(), "now" | "current" | "working") {
                return Ok(("now".into(), now.clone()));
            }
            let (v, p) = versions::load(&root, key)?;
            Ok((format!("{} \"{}\"", v.id, v.name), p))
        };
        match action.as_str() {
            "list" => {
                let i = versions::Index::load(&root)?;
                let now = collab::fingerprint(&self.doc.model);
                Ok(json!({
                    "current": i.current,
                    "branch": i.branch,
                    "matches_now": i.versions.iter().rev().find(|v| v.fingerprint == now).map(|v| v.id.clone()),
                    "versions": i.versions.iter().map(|v| json!({"id": v.id, "name": v.name, "note": v.note, "by": v.author, "branch": v.branch, "after": v.parent, "clips": v.clips, "duration": secs(v.duration), "sequence": v.sequence})).collect::<Vec<_>>(),
                }))
            }
            "save" => {
                let v = versions::save(&root, &self.doc.model, st(a, "name").unwrap_or(""), st(a, "note").unwrap_or(""), &who)?;
                Ok(json!({"saved": v.id, "name": v.name, "branch": v.branch}))
            }
            "compare" | "diff" => {
                let i = versions::Index::load(&root)?;
                let from = st(a, "from").map(str::to_owned).or_else(|| i.current.clone()).ok_or("compare needs \"from\" (no version saved yet)")?;
                let to = st(a, "to").unwrap_or("now");
                let (an, ap) = load(&from, &self.doc.model)?;
                let (bn, bp) = load(to, &self.doc.model)?;
                let r = delta::report(&ap, &bp);
                Ok(json!({"from": an, "to": bn, "counts": r.counts, "changes": r.changes.iter().take(200).collect::<Vec<_>>(), "more": r.changes.len().saturating_sub(200)}))
            }
            "restore" => {
                let key = st(a, "version").or_else(|| st(a, "name")).ok_or("restore needs \"version\"")?;
                let (v, p) = versions::load(&root, key)?;
                let r = delta::report(&self.doc.model, &p);
                let n = self.doc.take_in(&format!("Restore {}", v.name), &p)?;
                versions::set_current(&root, &v.id)?;
                Ok(json!({"restored": v.id, "name": v.name, "edits": n, "counts": r.counts, "undo": "undo takes it back"}))
            }
            "branch" => {
                let from = st(a, "version").unwrap_or("now");
                let (label, src) = load(from, &self.doc.model)?;
                let name = st(a, "name").map(str::to_owned).unwrap_or_else(|| format!("{} (alt)", src.active().name));
                let mut counter = self.doc.model.clone();
                let (mut edits, id) = versions::branch_edits(&mut counter, &src, None, &name)?;
                if a.get("switch").and_then(Value::as_bool).unwrap_or(true) {
                    edits.push(m::Edit::SetActiveSequence { id });
                }
                self.doc.model.next_id = counter.next_id;
                if !self.doc.apply(&format!("Branch {name}"), m::Edit::Batch { edits }) {
                    return Err(self.doc.error.take().unwrap_or_else(|| "the branch could not be made".into()));
                }
                versions::set_branch(&root, &name)?;
                Ok(json!({"branched": name, "from": label, "sequence": id.0, "active": self.doc.model.active_sequence == id, "note": "both cuts are sequences of this project; versions saved now belong to this branch"}))
            }
            "switch" => {
                // Between the cuts (sequences) of the project.
                let key = st(a, "name").or_else(|| st(a, "sequence")).ok_or("switch needs \"name\": a sequence (cut)")?;
                let s = self.doc.model.sequences.iter().find(|s| s.name.eq_ignore_ascii_case(key) || s.id.0.to_string() == key).ok_or_else(|| format!("no cut \"{key}\"; cuts: {}", self.doc.model.sequences.iter().map(|s| s.name.clone()).collect::<Vec<_>>().join(", ")))?;
                let id = s.id;
                if !self.doc.apply("Switch cut", m::Edit::SetActiveSequence { id }) && self.doc.model.active_sequence != id {
                    return Err(self.doc.error.take().unwrap_or_default());
                }
                Ok(json!({"active": key, "sequence": id.0}))
            }
            o => Err(format!("versions action \"{o}\": list, save, compare, restore, branch, switch")),
        }
    }

    fn share_tool(&mut self, a: &Value) -> Result<Value, String> {
        let root = self.collab_root()?;
        let action = st(a, "action").unwrap_or("status").to_lowercase();
        let who = st(a, "as").map(str::to_owned).unwrap_or_else(collab::identity);
        match action.as_str() {
            "package" => {
                let parent = st(a, "path").map(PathBuf::from).unwrap_or_else(|| m::file::ProjectFolder::new(&root).export_dir(m::file::ExportKind::Review));
                let dir = package::folder_in(&parent, &self.doc.model.active().name);
                let store = comments::Store::load(&root)?;
                let index = versions::Index::load(&root)?;
                let now = collab::fingerprint(&self.doc.model);
                let version = index.versions.iter().rev().find(|v| v.fingerprint == now).map(|v| (v.id.clone(), v.name.clone()));
                std::fs::create_dir_all(&dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;
                let proxy = if a.get("proxy").and_then(Value::as_bool).unwrap_or(true) { self.render_proxy(&dir)? } else { None };
                let info = package::write(&dir, &self.doc.model, &store, version.as_ref().map(|(i, n)| (i.as_str(), n.as_str())), &who, proxy.as_deref())?;
                Ok(json!({"package": dir.display().to_string(), "proxy": info.proxy, "notes": info.notes, "version": info.version_name, "note": "a folder on this computer; sending it is up to you"}))
            }
            "import_notes" | "notes_back" => self.comments_tool(&json!({"action": "import", "path": st(a, "path").ok_or("import_notes needs \"path\": the package folder or notes file that came back")?, "as": st(a, "as").unwrap_or("Reviewer")})),
            "merge" => {
                // Another copy of this project, worked on elsewhere.
                let other = PathBuf::from(st(a, "path").ok_or("merge needs \"path\": the other copy's project folder")?);
                let theirs_doc = crate::services::project::open(&other)?;
                let theirs = theirs_doc.model;
                let base = match st(a, "base") {
                    Some(k) => versions::load(&root, k)?.1,
                    None => common_version(&root, &other)?,
                };
                let o = collab::merge::three_way(&base, &self.doc.model, &theirs)?;
                let r = delta::report(&self.doc.model, &o.merged);
                let n = self.doc.take_in(&format!("Merge {}", other.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()), &o.merged)?;
                // Their notes too.
                let mut store = comments::Store::load(&root)?;
                let notes = store.merge(&comments::Store::load(&other)?);
                store.save(&root)?;
                Ok(json!({"edits": n, "taken": r.text(), "counts": r.counts, "conflicts": o.conflicts, "fixes": o.fixes, "notes_taken": notes, "undo": "undo takes the merge back"}))
            }
            "link" => {
                let folder = st(a, "folder").or_else(|| st(a, "path")).ok_or("link needs \"folder\": the shared folder (a network drive, a synced folder)")?;
                let remote = sync::FolderRemote::new(folder);
                let (work, state, joined) = sync::link(&root, &remote, &self.doc.model, &who)?;
                let n = if joined { self.doc.take_in("Join shared project", &work)? } else { 0 };
                self.doc.model.next_id = self.doc.model.next_id.max(work.next_id);
                // Joining brings the team and the notes along.
                let mut store = comments::Store::load(&root)?;
                if joined && let Some(mut team) = remote.team()? {
                    team.save(&root)?;
                }
                for (_, s) in remote.notes()? {
                    store.merge(&s);
                }
                store.save(&root)?;
                self.reguard(&root);
                crate::services::project::save(&mut self.doc, None)?;
                Ok(json!({"linked": folder, "joined": joined, "as": state.user, "slot": state.slot, "rev": state.base_rev, "edits": n, "note": if joined { "this copy now shows the shared edit; sync to exchange changes" } else { "shared: others link to the same folder to join" }}))
            }
            "sync" => {
                let state = sync::State::load(&root)?.ok_or("not linked: share link with a folder first")?;
                let remote = sync::FolderRemote::new(&state.remote);
                let mut store = comments::Store::load(&root)?;
                let mut team = team::Team::load(&root)?;
                let (merged, o) = sync::sync(&root, &remote, &self.doc.model, &mut store, &mut team)?;
                let n = self.doc.take_in("Sync with the team", &merged)?;
                self.doc.model.next_id = merged.next_id;
                store.save(&root)?;
                if o.team_updated {
                    crate::collab::write_json(&team::file(&root), &team)?;
                }
                self.reguard(&root);
                // Saved even when only the id block moved.
                crate::services::project::save(&mut self.doc, None)?;
                let mut v = serde_json::to_value(&o).unwrap_or(Value::Null);
                v["edits"] = json!(n);
                Ok(v)
            }
            "status" => {
                let Some(state) = sync::State::load(&root)? else { return Ok(json!({"linked": false, "note": "share link {folder} connects this project to a shared folder"})) };
                let remote = sync::FolderRemote::new(&state.remote);
                let head = remote.head()?;
                let now = collab::fingerprint(&self.doc.model);
                Ok(json!({"linked": state.remote, "as": state.user, "rev": state.base_rev, "shared_rev": head.as_ref().map(|h| h.rev), "last_by": head.as_ref().map(|h| h.by.clone()), "others_published": head.as_ref().is_some_and(|h| h.rev != state.base_rev), "in_step": head.as_ref().is_some_and(|h| h.fingerprint == now), "members": remote.members()?}))
            }
            o => Err(format!("share action \"{o}\": package, import_notes, merge, link, sync, status")),
        }
    }

    #[cfg(feature = "render")]
    fn render_proxy(&mut self, dir: &Path) -> Result<Option<String>, String> {
        use crate::services::export;
        if self.doc.project.duration() <= 0 {
            return Ok(None);
        }
        const PROXY: export::Preset = export::Preset { name: "Review proxy", sub: "H.264 · 540p", format: crate::formats::Format::H264, size: export::Size::ShortSide(540), quality: 55.0, available: true };
        let path = package::proxy_path(dir);
        let req = export::request(&self.doc, &PROXY, path, None);
        let mut job = export::ExportJob::start(&self.doc, req);
        match job.wait() {
            export::Event::Finished { path } => Ok(path.file_name().map(|n| n.to_string_lossy().into_owned())),
            export::Event::Failed(e) => Err(format!("the proxy render failed: {e}")),
            _ => Err("the proxy render was cancelled".into()),
        }
    }

    #[cfg(not(feature = "render"))]
    fn render_proxy(&mut self, _dir: &Path) -> Result<Option<String>, String> {
        Ok(None)
    }

    fn team_tool(&mut self, a: &Value) -> Result<Value, String> {
        let action = st(a, "action").unwrap_or("show").to_lowercase();
        if action == "set_name" || action == "identity" {
            if let Some(n) = st(a, "name") {
                collab::set_identity(n)?;
            }
            return Ok(json!({"you": collab::identity()}));
        }
        let root = self.collab_root()?;
        let mut t = team::Team::load(&root)?;
        let me = st(a, "as").map(str::to_owned).unwrap_or_else(collab::identity);
        let seq = self.doc.model.active_sequence.0;
        let target = || -> Result<(Option<u64>, Option<u64>), String> {
            match (a.get("lane").and_then(Value::as_u64), a.get("clip").and_then(Value::as_u64)) {
                (Some(l), None) => Ok((Some(l), None)),
                (None, Some(c)) => Ok((None, Some(c))),
                _ => Err("give \"lane\" (a lane id from summary) or \"clip\" (a clip id)".into()),
            }
        };
        match action.as_str() {
            "show" | "list" | "whoami" => {}
            "add" | "set" | "role" => {
                let name = st(a, "name").ok_or("set needs \"name\" and \"role\"")?;
                t.set_member(&me, name, team::Role::parse(st(a, "role").unwrap_or("editor"))?)?;
            }
            "remove" => t.remove_member(&me, st(a, "name").ok_or("remove needs \"name\"")?)?,
            "lock" => {
                let (lane, clip) = target()?;
                if let Some(l) = lane
                    && !self.doc.model.active().lanes.iter().any(|x| x.id.0 == l)
                {
                    return Err(format!("no lane {l} in this cut"));
                }
                if let Some(c) = clip
                    && self.doc.model.active().clip(m::ClipId(c)).is_none()
                {
                    return Err(format!("no clip {c} in this cut"));
                }
                t.lock(&me, seq, lane, clip, st(a, "note").unwrap_or(""))?;
            }
            "unlock" => {
                let (lane, clip) = target()?;
                t.unlock(&me, seq, lane, clip)?;
            }
            o => return Err(format!("team action \"{o}\": show, set, remove, lock, unlock, set_name")),
        }
        if action != "show" && action != "list" && action != "whoami" {
            t.save(&root)?;
            self.reguard(&root);
        }
        let lane_name = |id: u64| self.doc.model.active().lanes.iter().find(|l| l.id.0 == id).map(|l| l.name.clone()).unwrap_or_default();
        Ok(json!({
            "you": me,
            "role": t.role_of(&me).name(),
            "can_edit": t.role_of(&me).can_edit(),
            "members": t.members.iter().map(|m| json!({"name": m.name, "role": m.role.name()})).collect::<Vec<_>>(),
            "locks": t.locks.iter().map(|l| {
                let mut o = json!({"by": l.who, "note": l.note});
                if let Some(x) = l.lane { o["lane"] = json!(x); o["lane_name"] = json!(lane_name(x)); }
                if let Some(x) = l.clip { o["clip"] = json!(x); }
                o
            }).collect::<Vec<_>>(),
        }))
    }
}

/// The newest version both copies have (same id, same content): the base a
/// merge of the two starts from.
fn common_version(mine: &Path, other: &Path) -> Result<m::Project, String> {
    let (a, b) = (versions::Index::load(mine)?, versions::Index::load(other)?);
    let shared = a.versions.iter().rev().find(|v| b.versions.iter().any(|w| w.id == v.id && w.fingerprint == v.fingerprint));
    match shared {
        Some(v) => Ok(versions::load(mine, &v.id)?.1),
        None => Err("these copies share no saved version: save a version before handing a copy over, or give \"base\" (a version id)".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Content;
    use crate::collab::testing::Scratch;

    fn call(s: &mut Session, tool: &str, a: Value) -> Value {
        let out = s.run(tool, &a).unwrap_or_else(|e| panic!("{tool} {a}: {e}"));
        match &out[0] {
            Content::Text(t) => serde_json::from_str(t).unwrap(),
            _ => panic!("not text"),
        }
    }

    fn session(dir: &Path) -> Session {
        let mut s = Session::at(dir).unwrap();
        call(&mut s, "edit", json!({"ops": [
            {"op": "card", "colour": "#ff0000", "at": 0, "duration": 3, "lane": "main"},
            {"op": "card", "colour": "#00ff00", "at": 3, "duration": 3, "lane": "main"},
        ]}));
        s
    }

    #[test]
    fn notes_the_ai_reads_fixes_and_resolves() {
        let dir = Scratch::new("tools-notes");
        let proj = dir.0.join("film");
        let mut s = session(&proj);
        let clips: Vec<u32> = s.doc.project.clips.iter().map(|c| c.id).collect();
        let a = call(&mut s, "comments", json!({"action": "add", "note": "too bright", "clip": clips[1], "at": 4, "region": [-100, -50, 100, 50], "as": "Director"}));
        assert_eq!(a["added"], "director-1");
        call(&mut s, "comments", json!({"action": "add", "note": "hold longer", "at": 1, "as": "Director"}));
        let list = call(&mut s, "comments", json!({}));
        assert_eq!(list["open"], 2);
        assert_eq!(list["notes"][0]["note"], "hold longer", "time order");
        assert_eq!(list["notes"][1]["under"][0]["id"], clips[1], "{list}");
        call(&mut s, "edit", json!({"ops": [{"op": "set", "clip": clips[1], "opacity": 60}]}));
        let r = call(&mut s, "comments", json!({"action": "resolve", "id": "director-1", "note": "opacity 60%"}));
        assert_eq!(r["open"], 1);
        let done = call(&mut s, "comments", json!({"status": "resolved"}));
        assert!(done["notes"][0]["replies"][0].as_str().unwrap().ends_with("(AI): opacity 60%"), "{done}");
        let csv = call(&mut s, "comments", json!({"action": "export", "format": "csv", "status": "all"}));
        assert!(csv["text"].as_str().unwrap().contains("hold longer"));
        // Notes are files beside the project, not edits.
        assert!(crate::collab::comments::file(&proj).exists());
        let err = s.run("comments", &json!({"action": "resolve", "id": "nope"})).err().unwrap();
        assert!(err.contains("director-2"), "names the notes there are: {err}");
    }

    #[test]
    fn versions_compare_restore_and_branch() {
        let dir = Scratch::new("tools-versions");
        let proj = dir.0.join("film");
        let mut s = session(&proj);
        call(&mut s, "versions", json!({"action": "save", "name": "Assembly"}));
        let first = s.doc.project.clips[0].id;
        call(&mut s, "edit", json!({"ops": [{"op": "delete", "clip": first}]}));
        let c = call(&mut s, "versions", json!({"action": "compare", "from": "v1", "to": "now"}));
        assert_eq!(c["counts"]["removed"], 1, "{c}");
        let r = call(&mut s, "versions", json!({"action": "restore", "version": "assembly"}));
        assert_eq!(r["restored"], "v1");
        assert_eq!(s.doc.project.clips.len(), 2);
        call(&mut s, "undo", json!({}));
        assert_eq!(s.doc.project.clips.len(), 1, "a restore undoes as one step");
        let b = call(&mut s, "versions", json!({"action": "branch", "version": "v1", "name": "Long cut"}));
        assert_eq!(b["active"], true);
        assert_eq!(s.doc.project.clips.len(), 2, "the long cut is active");
        call(&mut s, "versions", json!({"action": "switch", "name": "film"}));
        assert_eq!(s.doc.project.clips.len(), 1);
        let l = call(&mut s, "versions", json!({}));
        assert_eq!(l["branch"], "Long cut");
    }

    #[test]
    fn reviewers_cannot_edit_and_locks_hold() {
        let dir = Scratch::new("tools-team");
        let proj = dir.0.join("film");
        let mut s = session(&proj);
        // Someone else owns the project; the person here is not on the team.
        call(&mut s, "team", json!({"action": "set", "name": "Kim", "role": "editor", "as": "Producer-7c1"}));
        let err = s.run("edit", &json!({"ops": [{"op": "card", "colour": "#ffffff", "at": 0, "duration": 1}]})).err().unwrap();
        assert!(err.contains("reviews this project"), "{err}");
        // Notes still work for a reviewer.
        call(&mut s, "comments", json!({"action": "add", "note": "nice", "at": 0}));
        // Make the person here an editor, then Kim holds the second clip.
        let me = collab::identity();
        call(&mut s, "team", json!({"action": "set", "name": me, "role": "editor", "as": "Producer-7c1"}));
        let second = s.doc.project.clips[1].id;
        call(&mut s, "team", json!({"action": "lock", "clip": second, "as": "Kim"}));
        let err = s.run("edit", &json!({"ops": [{"op": "set", "clip": second, "opacity": 10}]})).err().unwrap();
        assert!(err.contains("locked by Kim"), "{err}");
        let first = s.doc.project.clips[0].id;
        call(&mut s, "edit", json!({"ops": [{"op": "set", "clip": first, "opacity": 10}]}));
        let t = call(&mut s, "team", json!({}));
        assert_eq!(t["role"], "editor");
        assert_eq!(t["locks"][0]["by"], "Kim");
    }

    #[test]
    fn a_copy_merges_back_and_a_package_goes_out() {
        let dir = Scratch::new("tools-share");
        let (a_dir, b_dir) = (dir.0.join("a"), dir.0.join("b"));
        let mut a = session(&a_dir);
        call(&mut a, "versions", json!({"action": "save", "name": "Handover"}));
        // The copy goes to someone else (the whole folder).
        let cp = |from: &Path, to: &Path| {
            std::fs::create_dir_all(to).unwrap();
            for e in walk(from) {
                let rel = e.strip_prefix(from).unwrap();
                std::fs::create_dir_all(to.join(rel).parent().unwrap()).unwrap();
                std::fs::copy(&e, to.join(rel)).unwrap();
            }
        };
        cp(&a_dir, &b_dir);
        let mut b = Session::at(&b_dir).unwrap();
        let (c0, c1) = (a.doc.project.clips[0].id, a.doc.project.clips[1].id);
        call(&mut b, "edit", json!({"ops": [{"op": "set", "clip": c1, "opacity": 50}]}));
        call(&mut b, "comments", json!({"action": "add", "note": "from the other room", "at": 2, "as": "Kim"}));
        call(&mut a, "edit", json!({"ops": [{"op": "set", "clip": c0, "scale": 120}]}));
        let m = call(&mut a, "share", json!({"action": "merge", "path": b_dir.display().to_string()}));
        assert_eq!(m["conflicts"].as_array().unwrap().len(), 0, "{m}");
        assert_eq!(m["notes_taken"], 1);
        assert!((a.doc.project.clip(c1).unwrap().transform.opacity - 50.0).abs() < 0.5, "their change");
        assert!((a.doc.project.clip(c0).unwrap().transform.scale - 120.0).abs() < 0.5, "yours");
        let p = call(&mut a, "share", json!({"action": "package", "proxy": false}));
        let pkg = PathBuf::from(p["package"].as_str().unwrap());
        assert!(pkg.join("notes.csv").exists() && pkg.join("README.txt").exists());
        std::fs::write(pkg.join("notes-template.csv"), "timecode,note,author\n00:00:01:00,cut the first second,Client\n").unwrap();
        let back = call(&mut a, "share", json!({"action": "import_notes", "path": pkg.display().to_string()}));
        assert_eq!(back["imported"], 1, "{back}");
    }

    fn walk(d: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        for e in std::fs::read_dir(d).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() { out.extend(walk(&p)) } else { out.push(p) }
        }
        out
    }
}
