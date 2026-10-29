// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Several people, one project: each works in their own copy and syncs
//! through a shared place. A sync takes in what the others published since
//! this copy last synced (a three-way merge, one undoable step), then
//! publishes the result, plus everyone's review notes.
//!
//! The shared place is a [`Remote`]. [`FolderRemote`] is any folder both
//! computers reach: a network share on the local network, a synced folder,
//! a USB stick carried between rooms. A hosted service plugs in later as
//! another `Remote`; nothing here talks to a network by itself, and
//! nothing leaves the computer except when the person asks for a sync.
//!
//! Ids never clash: each person who joins gets their own block of ids
//! (`slot << 24`), so objects made at the same time in two copies are
//! always told apart.
//!
//! Folder layout:
//!
//! ```text
//! studio-sync.json      what this folder is
//! head.json             the latest published revision
//! revs/<rev>.json       the project at each revision
//! log.jsonl             who published what, when
//! members.json          who has joined, and their id blocks
//! notes/<name>.json     each person's review notes
//! team.json             roles and locks everyone follows
//! lock/                 present while someone publishes
//! ```

use super::comments::Store;
use super::team::Team;
use super::{fingerprint, merge, now_ms, read_json, write_json};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use studio_model as m;

/// The latest published state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Head {
    pub rev: u64,
    pub by: String,
    pub at: u64,
    pub fingerprint: String,
}

/// Where copies of a project meet. Implementations must make
/// [`Remote::publish`] atomic: it fails with [`Publish::Stale`] when
/// somebody published after `after`.
pub trait Remote {
    /// For people: the folder path, or later a service address.
    fn describe(&self) -> String;
    /// Sets the place up for a project (first person to share).
    fn create(&self, name: &str) -> Result<(), String>;
    fn head(&self) -> Result<Option<Head>, String>;
    fn fetch(&self, rev: u64) -> Result<m::Project, String>;
    fn publish(&self, after: u64, project: &m::Project, by: &str, summary: &str) -> Result<Publish, String>;
    /// Joins `user`, returning their id block number (0 for the first).
    fn join(&self, user: &str) -> Result<u32, String>;
    fn members(&self) -> Result<BTreeMap<String, u32>, String>;
    fn notes(&self) -> Result<Vec<(String, Store)>, String>;
    fn publish_notes(&self, user: &str, notes: &Store) -> Result<(), String>;
    fn team(&self) -> Result<Option<Team>, String>;
    fn publish_team(&self, team: &Team) -> Result<(), String>;
}

pub enum Publish {
    Done(Head),
    /// Someone else published first; sync again.
    Stale(Head),
}

/// A shared folder as the meeting place.
pub struct FolderRemote {
    pub root: PathBuf,
}

impl FolderRemote {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Holds `lock/` while `f` runs; a lock older than a minute was left by
    /// a crash and is taken over.
    fn locked<T>(&self, f: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
        let lock = self.root.join("lock");
        let started = std::time::Instant::now();
        loop {
            match std::fs::create_dir(&lock) {
                Ok(()) => break,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    let old = std::fs::metadata(&lock).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age.as_secs() > 60);
                    if old {
                        let _ = std::fs::remove_dir(&lock);
                        continue;
                    }
                    if started.elapsed().as_secs() > 10 {
                        return Err(format!("{} is busy (someone is syncing); try again", self.root.display()));
                    }
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                Err(e) => return Err(format!("could not lock {}: {e}", self.root.display())),
            }
        }
        let r = f();
        let _ = std::fs::remove_dir(&lock);
        r
    }

    fn check(&self) -> Result<(), String> {
        if self.root.join("studio-sync.json").exists() {
            Ok(())
        } else {
            Err(format!("{} is not a Zornov sync folder (share the project there first)", self.root.display()))
        }
    }
}

impl Remote for FolderRemote {
    fn describe(&self) -> String {
        self.root.display().to_string()
    }

    fn create(&self, name: &str) -> Result<(), String> {
        if self.root.join("studio-sync.json").exists() {
            return Ok(());
        }
        std::fs::create_dir_all(self.root.join("revs")).map_err(|e| format!("could not create {}: {e}", self.root.display()))?;
        write_json(&self.root.join("studio-sync.json"), &serde_json::json!({"format": "studio-sync", "version": 1, "project": name, "created": now_ms()}))
    }

    fn head(&self) -> Result<Option<Head>, String> {
        self.check()?;
        read_json(&self.root.join("head.json"))
    }

    fn fetch(&self, rev: u64) -> Result<m::Project, String> {
        let text = std::fs::read_to_string(self.root.join("revs").join(format!("{rev:06}.json"))).map_err(|e| format!("revision {rev} is missing from {}: {e}", self.root.display()))?;
        Ok(m::file::from_json(&text).map_err(|e| format!("revision {rev} is damaged: {e}"))?.project)
    }

    fn publish(&self, after: u64, project: &m::Project, by: &str, summary: &str) -> Result<Publish, String> {
        self.check()?;
        self.locked(|| {
            let head: Option<Head> = read_json(&self.root.join("head.json"))?;
            if let Some(h) = &head
                && h.rev != after
            {
                return Ok(Publish::Stale(h.clone()));
            }
            let rev = after + 1;
            let doc = m::file::ProjectFolder::new(&self.root).document(project, &m::Extra::new());
            m::file::write_atomic(&self.root.join("revs").join(format!("{rev:06}.json")), m::file::to_json(&doc).as_bytes()).map_err(|e| format!("could not publish: {e}"))?;
            let h = Head { rev, by: by.to_owned(), at: now_ms(), fingerprint: fingerprint(project) };
            write_json(&self.root.join("head.json"), &h)?;
            use std::io::Write;
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(self.root.join("log.jsonl")) {
                let _ = writeln!(f, "{}", serde_json::json!({"rev": rev, "by": by, "at": h.at, "summary": summary}));
            }
            Ok(Publish::Done(h))
        })
    }

    fn join(&self, user: &str) -> Result<u32, String> {
        self.check()?;
        self.locked(|| {
            let path = self.root.join("members.json");
            let mut members: BTreeMap<String, u32> = read_json(&path)?.unwrap_or_default();
            if let Some(s) = members.get(user) {
                return Ok(*s);
            }
            let slot = members.values().max().map_or(0, |m| m + 1);
            if slot > 250 {
                return Err("this project has the most members a sync folder holds".into());
            }
            members.insert(user.to_owned(), slot);
            write_json(&path, &members)?;
            Ok(slot)
        })
    }

    fn members(&self) -> Result<BTreeMap<String, u32>, String> {
        Ok(read_json(&self.root.join("members.json"))?.unwrap_or_default())
    }

    fn notes(&self) -> Result<Vec<(String, Store)>, String> {
        let mut out = Vec::new();
        let Ok(rd) = std::fs::read_dir(self.root.join("notes")) else { return Ok(out) };
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "json")
                && let Some(s) = read_json::<Store>(&p)?
            {
                out.push((p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(), s));
            }
        }
        Ok(out)
    }

    fn publish_notes(&self, user: &str, notes: &Store) -> Result<(), String> {
        let name: String = user.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
        write_json(&self.root.join("notes").join(format!("{name}.json")), notes)
    }

    fn team(&self) -> Result<Option<Team>, String> {
        read_json(&self.root.join("team.json"))
    }

    fn publish_team(&self, team: &Team) -> Result<(), String> {
        write_json(&self.root.join("team.json"), team)
    }
}

/// This copy's link to a remote.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct State {
    /// The shared folder (later: a service address).
    pub remote: String,
    pub user: String,
    /// Id block number.
    pub slot: u32,
    /// The revision this copy last took in or published.
    pub base_rev: u64,
    pub last_sync: u64,
}

fn state_file(root: &Path) -> PathBuf {
    m::file::ProjectFolder::new(root).state(m::file::State::Sync).join("state.json")
}
fn base_file(root: &Path) -> PathBuf {
    m::file::ProjectFolder::new(root).state(m::file::State::Sync).join("base.json")
}

impl State {
    pub fn load(root: &Path) -> Result<Option<Self>, String> {
        read_json(&state_file(root))
    }
    fn save(&self, root: &Path, base: &m::Project) -> Result<(), String> {
        let doc = m::file::ProjectFolder::new(root).document(base, &m::Extra::new());
        std::fs::create_dir_all(m::file::ProjectFolder::new(root).state(m::file::State::Sync)).map_err(|e| e.to_string())?;
        m::file::write_atomic(&base_file(root), m::file::to_json(&doc).as_bytes()).map_err(|e| format!("could not keep the sync base: {e}"))?;
        write_json(&state_file(root), self)
    }
    fn base(&self, root: &Path) -> Result<m::Project, String> {
        let text = std::fs::read_to_string(base_file(root)).map_err(|e| format!("the sync base is missing ({e}); link the project to the folder again"))?;
        Ok(m::file::from_json(&text).map_err(|e| format!("the sync base is damaged: {e}"))?.project)
    }
}

/// The first id of block `slot`.
pub fn block_start(slot: u32) -> u64 {
    (slot as u64) << 24
}

/// What a sync did.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Outcome {
    /// Revision now shared.
    pub rev: u64,
    /// Changes taken in from others (report lines).
    pub taken: Vec<String>,
    pub conflicts: Vec<merge::Conflict>,
    pub fixes: Vec<String>,
    /// Whether this copy's changes were published.
    pub published: bool,
    pub notes_taken: usize,
    pub team_updated: bool,
    /// Media the others use that is not on this computer.
    pub missing_media: Vec<String>,
    pub members: Vec<String>,
}

/// Links a project to a remote: shares it when the remote is new, or
/// joins it. Returns the project this copy should switch to (the shared
/// one, when joining an existing remote), with this person's id block.
pub fn link(root: &Path, remote: &dyn Remote, project: &m::Project, user: &str) -> Result<(m::Project, State, bool), String> {
    remote.create(&project.active().name)?;
    let slot = remote.join(user)?;
    let head = remote.head()?;
    let (mut base, rev, joined) = match &head {
        Some(h) => (remote.fetch(h.rev)?, h.rev, true),
        None => match remote.publish(0, project, user, "shared")? {
            Publish::Done(h) => (project.clone(), h.rev, false),
            Publish::Stale(h) => (remote.fetch(h.rev)?, h.rev, true),
        },
    };
    m::file::ProjectFolder::new(root).resolve_media(&mut base);
    let state = State { remote: remote.describe(), user: user.to_owned(), slot, base_rev: rev, last_sync: now_ms() };
    state.save(root, &base)?;
    let mut work = if joined { base } else { project.clone() };
    work.next_id = work.next_id.max(block_start(slot));
    Ok((work, state, joined))
}

/// One sync of `mine` (the copy in `root`): returns the project to switch
/// to (merge applied) and what happened. Notes in `notes` are merged with
/// everyone's in place; the team file follows the remote's when newer.
pub fn sync(root: &Path, remote: &dyn Remote, mine: &m::Project, notes: &mut Store, team: &mut Team) -> Result<(m::Project, Outcome), String> {
    let mut state = State::load(root)?.ok_or("this project is not linked to a sync folder yet")?;
    let mut out = Outcome::default();
    let mut current = mine.clone();
    let mut base = state.base(root)?;
    for _ in 0..5 {
        let head = remote.head()?.ok_or("the sync folder has no published revision")?;
        if head.rev != state.base_rev {
            let mut theirs = remote.fetch(head.rev)?;
            m::file::ProjectFolder::new(root).resolve_media(&mut theirs);
            let o = merge::three_way(&base, &current, &theirs)?;
            let mut merged = o.merged;
            // Ids stay in this person's block.
            merged.next_id = current.next_id.max(block_start(state.slot));
            out.taken.extend(super::delta::report(&current, &merged).changes.iter().map(|c| format!("{} {} \"{}\"{}", c.what, c.kind, c.name, if c.detail.is_empty() { String::new() } else { format!(": {}", c.detail) })));
            out.conflicts.extend(o.conflicts);
            out.fixes.extend(o.fixes);
            current = merged;
            base = theirs;
            state.base_rev = head.rev;
        }
        if fingerprint(&current) == fingerprint(&base) {
            break;
        }
        let summary = super::delta::report(&base, &current).counts.iter().map(|(k, v)| format!("{v} {k}")).collect::<Vec<_>>().join(", ");
        match remote.publish(state.base_rev, &current, &state.user, &summary)? {
            Publish::Done(h) => {
                state.base_rev = h.rev;
                base = current.clone();
                out.published = true;
                break;
            }
            Publish::Stale(_) => continue,
        }
    }
    out.rev = state.base_rev;
    for item in &current.media {
        if !Path::new(&item.path).exists() && !out.missing_media.contains(&item.name) {
            out.missing_media.push(item.name.clone());
        }
    }

    // Notes: everyone's, merged; then ours published.
    for (_, theirs) in remote.notes()? {
        out.notes_taken += notes.merge(&theirs);
    }
    remote.publish_notes(&state.user, notes)?;
    // Team: the newer copy wins.
    match remote.team()? {
        Some(t) if t.updated > team.updated => {
            *team = t;
            out.team_updated = true;
        }
        _ if team.updated > 0 => remote.publish_team(team)?,
        _ => {}
    }
    out.members = remote.members()?.into_keys().collect();
    state.last_sync = now_ms();
    state.save(root, &base)?;
    Ok((current, out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collab::{comments, testing};

    #[test]
    fn two_people_one_folder() {
        let shared = testing::Scratch::new("sync-remote");
        let (ash_dir, kim_dir) = (testing::Scratch::new("sync-ash"), testing::Scratch::new("sync-kim"));
        let remote = FolderRemote::new(&shared.0);
        let start = testing::project();

        // Ash shares; Kim joins and gets Ash's edit and her own id block.
        let (mut ash, _, joined) = link(&ash_dir.0, &remote, &start, "Ash").unwrap();
        assert!(!joined);
        let (mut kim, st, joined) = link(&kim_dir.0, &remote, &m::Project::default(), "Kim").unwrap();
        assert!(joined && st.slot == 1);
        assert!(kim.content_eq(&start));
        assert!(kim.next_id >= block_start(1));

        // Both edit: Ash trims the first clip, Kim fades the last and adds a note.
        let ids: Vec<m::ClipId> = start.active().clips.iter().map(|c| c.id).collect();
        let s = ash.active_sequence;
        ash.apply(m::Edit::Trim { seq: s, clip: ids[0], edge: m::Edge::End, delta: -1_000_000, ripple: true }).unwrap();
        let mut c = kim.active().clip(ids[2]).unwrap().clone();
        c.transform.opacity = 0.25;
        kim.apply(m::Edit::SetClip { seq: s, clip: Box::new(c) }).unwrap();
        let (mut ash_notes, mut kim_notes) = (Store::default(), Store::default());
        kim_notes.add(&kim, "Kim", "is the fade too much?", comments::Pin { at: 7_000_000, ..Default::default() }, None).unwrap();
        let (mut ash_team, mut kim_team) = (Team::default(), Team::default());

        let (ash2, o) = sync(&ash_dir.0, &remote, &ash, &mut ash_notes, &mut ash_team).unwrap();
        assert!(o.published && o.taken.is_empty());
        let (kim2, o) = sync(&kim_dir.0, &remote, &kim, &mut kim_notes, &mut kim_team).unwrap();
        assert!(o.published, "Kim's fade goes out merged with Ash's trim");
        assert!(o.conflicts.is_empty(), "{:?}", o.conflicts);
        assert!(!o.taken.is_empty());
        let (ash3, o) = sync(&ash_dir.0, &remote, &ash2, &mut ash_notes, &mut ash_team).unwrap();
        assert!(!o.published, "nothing new from Ash");
        assert_eq!(o.notes_taken, 1);
        assert_eq!(ash_notes.open_count(), 1);
        assert_eq!(fingerprint(&ash3), fingerprint(&kim2), "both copies show the same edit");
        let last = ash3.active().clip(ids[2]).unwrap();
        assert_eq!((last.transform.opacity, last.start), (0.25, 5_000_000));
        assert_eq!(remote.members().unwrap().len(), 2);
        assert!(std::fs::read_to_string(shared.0.join("log.jsonl")).unwrap().lines().count() >= 3);
    }
}
