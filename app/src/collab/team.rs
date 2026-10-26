// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Who works on a project and what each may change.
//!
//! * **Roles.** An *owner* manages the team; *editors* change the edit;
//!   *reviewers* only add notes (every edit they try is refused, with a
//!   reason). A project with no team file is a one-person project: whoever
//!   opens it is its owner. Once a team exists, people not on it review.
//! * **Locks.** An editor claims a lane or a clip ("my music lane", "the
//!   VFX shot"); nobody else's edit may change it, including a ripple
//!   that would shift it. The person holding a lock, or an owner,
//!   releases it.
//!
//! The rules live in `team.json`; the document enforces them on every edit
//! through a [`Guard`] (`Doc::guard`), for people and the AI alike.

use super::{now_ms, read_json, write_json};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use studio_model as m;

pub fn file(root: &Path) -> PathBuf {
    root.join("team.json")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Owner,
    Editor,
    Reviewer,
}

impl Role {
    pub fn parse(s: &str) -> Result<Self, String> {
        Ok(match s.trim().to_lowercase().as_str() {
            "owner" | "admin" | "producer" => Role::Owner,
            "editor" | "edit" | "assistant" => Role::Editor,
            "reviewer" | "review" | "viewer" | "commenter" | "client" => Role::Reviewer,
            o => return Err(format!("role \"{o}\": owner, editor or reviewer")),
        })
    }
    pub fn name(self) -> &'static str {
        match self {
            Role::Owner => "owner",
            Role::Editor => "editor",
            Role::Reviewer => "reviewer",
        }
    }
    pub fn can_edit(self) -> bool {
        self != Role::Reviewer
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Member {
    pub name: String,
    pub role: Role,
}

/// A lane or clip held by one person.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Lock {
    pub who: String,
    pub sequence: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lane: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<u64>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    pub at: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Team {
    #[serde(default)]
    pub members: Vec<Member>,
    #[serde(default)]
    pub locks: Vec<Lock>,
    /// Last change (a sync keeps the newer team).
    #[serde(default)]
    pub updated: u64,
}

fn same(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

impl Team {
    pub fn load(root: &Path) -> Result<Self, String> {
        Ok(read_json(&file(root))?.unwrap_or_default())
    }

    pub fn save(&mut self, root: &Path) -> Result<(), String> {
        self.updated = now_ms();
        write_json(&file(root), self)
    }

    pub fn role_of(&self, user: &str) -> Role {
        if self.members.is_empty() {
            return Role::Owner;
        }
        self.members.iter().find(|m| same(&m.name, user)).map_or(Role::Reviewer, |m| m.role)
    }

    /// Adds or changes a member. Only owners manage the team; the first
    /// person to set one up becomes its owner.
    pub fn set_member(&mut self, by: &str, name: &str, role: Role) -> Result<(), String> {
        if name.trim().is_empty() {
            return Err("a member needs a name".into());
        }
        if self.members.is_empty() {
            self.members.push(Member { name: by.to_owned(), role: Role::Owner });
        }
        if self.role_of(by) != Role::Owner {
            return Err(format!("only an owner manages the team (you are {})", self.role_of(by).name()));
        }
        if same(name, by) && role != Role::Owner && self.members.iter().filter(|m| m.role == Role::Owner).count() == 1 {
            return Err("you are the only owner: make someone else an owner first".into());
        }
        match self.members.iter_mut().find(|m| same(&m.name, name)) {
            Some(m) => m.role = role,
            None => self.members.push(Member { name: name.trim().to_owned(), role }),
        }
        Ok(())
    }

    pub fn remove_member(&mut self, by: &str, name: &str) -> Result<(), String> {
        if self.role_of(by) != Role::Owner {
            return Err("only an owner manages the team".into());
        }
        if same(name, by) {
            return Err("an owner cannot remove themselves".into());
        }
        let n = self.members.len();
        self.members.retain(|m| !same(&m.name, name));
        if self.members.len() == n {
            return Err(format!("{name} is not on the team"));
        }
        self.locks.retain(|l| !same(&l.who, name));
        Ok(())
    }

    /// Claims a lane or a clip for `who`.
    pub fn lock(&mut self, who: &str, sequence: u64, lane: Option<u64>, clip: Option<u64>, note: &str) -> Result<(), String> {
        if !self.role_of(who).can_edit() {
            return Err("reviewers do not lock: only editors hold lanes and clips".into());
        }
        if lane.is_none() == clip.is_none() {
            return Err("lock a lane or a clip".into());
        }
        if let Some(l) = self.locks.iter().find(|l| l.sequence == sequence && l.lane == lane && l.clip == clip) {
            return if same(&l.who, who) { Ok(()) } else { Err(format!("already locked by {}", l.who)) };
        }
        self.locks.push(Lock { who: who.to_owned(), sequence, lane, clip, note: note.trim().to_owned(), at: now_ms() });
        Ok(())
    }

    /// Releases a lock: its holder or an owner.
    pub fn unlock(&mut self, by: &str, sequence: u64, lane: Option<u64>, clip: Option<u64>) -> Result<(), String> {
        let i = self.locks.iter().position(|l| l.sequence == sequence && l.lane == lane && l.clip == clip).ok_or("nothing is locked there")?;
        if !same(&self.locks[i].who, by) && self.role_of(by) != Role::Owner {
            return Err(format!("locked by {}: they or an owner can release it", self.locks[i].who));
        }
        self.locks.remove(i);
        Ok(())
    }

    /// The rules `user`'s edits follow; `None` when nothing limits them.
    pub fn guard_for(&self, user: &str) -> Option<Guard> {
        let role = self.role_of(user);
        let mut g = Guard { user: user.to_owned(), reviewer: !role.can_edit(), lanes: HashMap::new(), clips: HashMap::new() };
        for l in self.locks.iter().filter(|l| !same(&l.who, user)) {
            if let Some(lane) = l.lane {
                g.lanes.insert((l.sequence, lane), l.who.clone());
            }
            if let Some(clip) = l.clip {
                g.clips.insert((l.sequence, clip), l.who.clone());
            }
        }
        (g.reviewer || !g.lanes.is_empty() || !g.clips.is_empty()).then_some(g)
    }
}

/// What the document checks every edit against.
#[derive(Clone, Debug, PartialEq)]
pub struct Guard {
    pub user: String,
    pub reviewer: bool,
    /// (sequence, lane) → who holds it.
    pub lanes: HashMap<(u64, u64), String>,
    /// (sequence, clip) → who holds it.
    pub clips: HashMap<(u64, u64), String>,
}

impl Guard {
    /// Whether `edit` may be applied to `p`: refused for reviewers, and
    /// when it would change a lane or a clip someone else holds.
    pub fn check(&self, p: &m::Project, edit: &m::Edit) -> Result<(), String> {
        if self.reviewer {
            return Err(format!("{} reviews this project: add a note instead (editors make the changes)", self.user));
        }
        // What the edit changes, in primitives: its inverse, worked out on
        // a copy.
        let mut trial = p.clone();
        let Ok(inverse) = trial.apply(edit.clone()) else { return Ok(()) };
        let (mut clips, mut lanes) = (HashSet::new(), HashSet::new());
        super::delta::touched(std::slice::from_ref(&inverse), &trial, &mut clips, &mut lanes);
        for key in &clips {
            if let Some(who) = self.clips.get(key) {
                return Err(format!("clip #{} is locked by {who}", key.1));
            }
        }
        for key in &lanes {
            if let Some(who) = self.lanes.get(key) {
                let name = p.sequence(m::SequenceId(key.0)).and_then(|s| s.lanes.iter().find(|l| l.id.0 == key.1)).map(|l| l.name.clone()).unwrap_or_default();
                return Err(format!("lane \"{name}\" is locked by {who}"));
            }
        }
        Ok(())
    }
}

/// The guard for the person at this computer, from the project's team file.
pub fn guard_in(root: &Path) -> Option<Guard> {
    Team::load(root).ok()?.guard_for(&super::identity())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collab::testing;

    #[test]
    fn roles_and_who_manages_them() {
        let mut t = Team::default();
        assert_eq!(t.role_of("anyone"), Role::Owner, "no team: a one-person project");
        t.set_member("Ash", "Kim", Role::Editor).unwrap();
        t.set_member("Ash", "Client", Role::Reviewer).unwrap();
        assert_eq!(t.role_of("ash"), Role::Owner);
        assert_eq!(t.role_of("Stranger"), Role::Reviewer);
        assert!(t.set_member("Kim", "Kim", Role::Owner).is_err());
        assert!(t.set_member("Ash", "Ash", Role::Editor).is_err(), "last owner");
        assert!(t.lock("Client", 1, Some(2), None, "").is_err());
        assert!(t.guard_for("Ash").is_none());
        assert!(t.guard_for("Client").unwrap().reviewer);
    }

    #[test]
    fn locks_hold_against_direct_edits_and_ripples() {
        let p = testing::project();
        let seq = p.active_sequence;
        let ids: Vec<m::ClipId> = p.active().clips.iter().map(|c| c.id).collect();
        let mut t = Team::default();
        t.set_member("Ash", "Kim", Role::Editor).unwrap();
        t.lock("Kim", seq.0, None, Some(ids[2].0), "grading").unwrap();
        let g = t.guard_for("Ash").unwrap();
        // Changing the clip itself.
        let mut c = p.active().clip(ids[2]).unwrap().clone();
        c.transform.opacity = 0.5;
        let err = g.check(&p, &m::Edit::SetClip { seq, clip: Box::new(c) }).unwrap_err();
        assert!(err.contains("locked by Kim"), "{err}");
        // A ripple that would shift it.
        assert!(g.check(&p, &m::Edit::RippleDelete { seq, clips: vec![ids[0]] }).is_err());
        // Something that leaves it be.
        let mut c = p.active().clip(ids[0]).unwrap().clone();
        c.transform.opacity = 0.5;
        g.check(&p, &m::Edit::SetClip { seq, clip: Box::new(c) }).unwrap();
        // Kim herself is free; Ash cannot release Kim's lock, an owner can.
        assert!(t.guard_for("Kim").is_none());
        t.set_member("Ash", "Sam", Role::Editor).unwrap();
        assert!(t.unlock("Sam", seq.0, None, Some(ids[2].0)).is_err());
        t.unlock("Ash", seq.0, None, Some(ids[2].0)).unwrap();

        // A whole lane.
        let spine = p.active().lanes.iter().find(|l| l.kind == m::LaneKind::Spine).unwrap().id;
        t.lock("Kim", seq.0, Some(spine.0), None, "").unwrap();
        let g = t.guard_for("Ash").unwrap();
        assert!(g.check(&p, &m::Edit::Split { seq, at: 1_000_000, clips: None }).unwrap_err().contains("lane"));
    }
}
