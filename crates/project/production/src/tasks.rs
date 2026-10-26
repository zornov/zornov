// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Task tracking for shots, scenes and assets, in the manner of ShotGrid
//! and ftrack: every task has a status and an assignee, notes, and
//! versions that point at the clips on the timeline; the dashboard says
//! what is done.

use crate::date::Date;
use crate::{Production, now};
use serde::{Deserialize, Serialize};
use crate::story::SECOND;
use crate::story::Story;

/// Where a task stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Not startable yet: needs earlier work.
    Waiting,
    /// Can be started.
    Ready,
    /// Being worked on.
    InProgress,
    /// Done, waiting for someone to look.
    Review,
    /// Looked at; changes asked for.
    Revisions,
    /// Looked at and good.
    Approved,
    /// Locked: delivered.
    Final,
    /// Stuck on something outside the task.
    Blocked,
    /// Dropped from the film.
    Omitted,
}

impl Status {
    /// Every status, in the order work flows.
    pub const ALL: [Status; 9] = [Status::Waiting, Status::Ready, Status::InProgress, Status::Review, Status::Revisions, Status::Approved, Status::Final, Status::Blocked, Status::Omitted];

    /// The snake_case name.
    pub fn name(self) -> &'static str {
        match self {
            Status::Waiting => "waiting",
            Status::Ready => "ready",
            Status::InProgress => "in_progress",
            Status::Review => "review",
            Status::Revisions => "revisions",
            Status::Approved => "approved",
            Status::Final => "final",
            Status::Blocked => "blocked",
            Status::Omitted => "omitted",
        }
    }

    /// Reads a name or what trackers call it (`wtg`, `ip`, `rtk`, `apr`, `fin`, `omt`, `done`).
    pub fn parse(s: &str) -> Option<Status> {
        Some(match s.trim().to_lowercase().replace([' ', '-'], "_").as_str() {
            "waiting" | "wtg" | "waiting_to_start" | "not_started" | "todo" | "to_do" | "pending" => Status::Waiting,
            "ready" | "rdy" | "ready_to_start" | "rts" => Status::Ready,
            "in_progress" | "ip" | "wip" | "started" | "working" | "doing" | "active" => Status::InProgress,
            "review" | "pending_review" | "rev" | "for_review" | "done_for_review" => Status::Review,
            "revisions" | "retake" | "rtk" | "changes" | "needs_changes" | "revise" | "rejected" => Status::Revisions,
            "approved" | "apr" | "ok" | "accepted" => Status::Approved,
            "final" | "fin" | "done" | "complete" | "completed" | "delivered" => Status::Final,
            "blocked" | "hold" | "on_hold" | "stuck" => Status::Blocked,
            "omitted" | "omt" | "omit" | "cut" | "dropped" | "cancelled" => Status::Omitted,
            _ => return None,
        })
    }

    /// How finished the work is, for percentages.
    pub fn progress(self) -> f64 {
        match self {
            Status::Waiting | Status::Ready | Status::Blocked => 0.0,
            Status::InProgress => 0.5,
            Status::Review => 0.8,
            Status::Revisions => 0.6,
            Status::Approved | Status::Final => 1.0,
            Status::Omitted => 0.0,
        }
    }

    /// Approved or better (or dropped): nothing more to do.
    pub fn is_done(self) -> bool {
        matches!(self, Status::Approved | Status::Final | Status::Omitted)
    }
}

/// What a task is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Entity {
    /// A shot (id).
    Shot,
    /// A scene (id).
    Scene,
    /// An asset: a character, prop, place or other element (name).
    Asset,
    /// The film as a whole.
    Project,
}

impl Entity {
    /// Reads `shot`, `scene`, `asset`, `project`.
    pub fn parse(s: &str) -> Option<Entity> {
        Some(match s.trim().to_lowercase().as_str() {
            "shot" => Entity::Shot,
            "scene" => Entity::Scene,
            "asset" | "element" | "character" | "prop" => Entity::Asset,
            "project" | "film" | "show" => Entity::Project,
            _ => return None,
        })
    }
    /// The name.
    pub fn name(self) -> &'static str {
        match self {
            Entity::Shot => "shot",
            Entity::Scene => "scene",
            Entity::Asset => "asset",
            Entity::Project => "project",
        }
    }
}

/// A note on a task.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Note {
    /// Seconds since the epoch.
    pub at: u64,
    /// Who wrote it.
    pub who: String,
    /// What it says.
    pub text: String,
}

/// A version of the work, tied to the timeline.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Version {
    /// 1, 2, 3…
    pub n: u32,
    /// Seconds since the epoch.
    pub at: u64,
    /// Who made it.
    pub by: String,
    /// What changed in it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    /// Timeline clips that show it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub clips: Vec<u32>,
    /// Where it started on the timeline, seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<f64>,
    /// How long it runs, seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,
    /// A media item or file it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media: Option<String>,
}

/// One unit of work.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Task {
    /// Unique id.
    pub id: u64,
    /// Shot, scene, asset or project.
    pub entity: Entity,
    /// The shot id, scene id or element name.
    pub target: String,
    /// The step (`layout`, `animation`, `design`, `generate`…).
    pub step: String,
    /// Where it stands.
    pub status: Status,
    /// Who is doing it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub assignee: String,
    /// Due date, `YYYY-MM-DD`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due: Option<String>,
    /// Notes, oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<Note>,
    /// Versions, oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub versions: Vec<Version>,
    /// Created.
    pub created: u64,
    /// Last changed.
    pub updated: u64,
}

impl Task {
    /// `shot 1A / layout`.
    pub fn label(&self) -> String {
        format!("{} {} / {}", self.entity.name(), self.target, self.step)
    }
}

/// One line of the activity log.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Activity {
    /// Seconds since the epoch.
    pub at: u64,
    /// Who.
    pub who: String,
    /// What happened.
    pub text: String,
}

/// Today's date.
pub fn today() -> Date {
    Date((now() / 86_400) as i32)
}

/// A row of the dashboard per step.
#[derive(Clone, Debug, PartialEq)]
pub struct StepRow {
    /// The step.
    pub step: String,
    /// Tasks in it.
    pub total: usize,
    /// Approved, final or omitted.
    pub done: usize,
    /// Percent of work finished.
    pub percent: f64,
}

/// A row of the dashboard per shot.
#[derive(Clone, Debug, PartialEq)]
pub struct ShotRow {
    /// Shot id.
    pub shot: String,
    /// Scene id.
    pub scene: String,
    /// Percent of its work finished.
    pub percent: f64,
    /// The step it is at: the first not done.
    pub at_step: Option<String>,
    /// That step's status.
    pub status: Option<Status>,
    /// Latest version number of its latest task.
    pub version: u32,
}

/// What is done and what is not.
#[derive(Clone, Debug, PartialEq)]
pub struct Dashboard {
    /// All tasks counted (omitted ones left out).
    pub total: usize,
    /// How many tasks per status.
    pub by_status: Vec<(Status, usize)>,
    /// Percent of the whole film's work finished.
    pub percent: f64,
    /// Per step.
    pub steps: Vec<StepRow>,
    /// Per person: open tasks, done tasks.
    pub people: Vec<(String, usize, usize)>,
    /// Per shot, in cut order.
    pub shots: Vec<ShotRow>,
    /// Shots finished / shots with tasks.
    pub shots_done: (usize, usize),
    /// Open tasks past their due date.
    pub overdue: Vec<u64>,
    /// Blocked tasks.
    pub blocked: Vec<u64>,
    /// Waiting for a look.
    pub in_review: Vec<u64>,
    /// Tasks nobody has.
    pub unassigned: usize,
}

impl Production {
    /// Finds a task by id (`12`, `#12`) or `"1A layout"` / `"1A/layout"`.
    pub fn task_index(&self, key: &str) -> Option<usize> {
        let k = key.trim().trim_start_matches('#');
        if let Ok(id) = k.parse::<u64>() {
            return self.tasks.iter().position(|t| t.id == id);
        }
        let (target, step) = k.split_once(['/', ' ', ':'])?;
        let (target, step) = (target.trim(), step.trim());
        self.tasks.iter().position(|t| t.target.eq_ignore_ascii_case(target) && t.step.eq_ignore_ascii_case(step))
    }

    /// Adds a task (or returns the one that is already there).
    pub fn add_task(&mut self, entity: Entity, target: &str, step: &str, assignee: &str, status: Status) -> u64 {
        if let Some(t) = self.tasks.iter().find(|t| t.entity == entity && t.target.eq_ignore_ascii_case(target) && t.step.eq_ignore_ascii_case(step)) {
            return t.id;
        }
        let id = self.alloc_id();
        let t = now();
        self.tasks.push(Task { id, entity, target: target.to_owned(), step: step.to_owned(), status, assignee: assignee.to_owned(), due: None, notes: Vec::new(), versions: Vec::new(), created: t, updated: t });
        id
    }

    /// Makes a task for every shot and every pipeline stage that has none,
    /// the first stage ready and the rest waiting. Returns how many.
    pub fn ensure_pipeline_tasks(&mut self, story: &Story) -> usize {
        let steps: Vec<(String, bool)> = self.pipeline.stages.iter().map(|s| (s.key.clone(), s.after.is_empty())).collect();
        let before = self.tasks.len();
        for sc in &story.scenes {
            for sh in &sc.shots {
                for (step, first) in &steps {
                    self.add_task(Entity::Shot, &sh.id, step, "", if *first { Status::Ready } else { Status::Waiting });
                }
            }
        }
        self.tasks.len() - before
    }

    /// Makes `design` (or `step`) tasks for the cast, props, vehicles and
    /// locations of the breakdown that have none.
    pub fn ensure_asset_tasks(&mut self, step: &str) -> usize {
        use crate::breakdown::Category::*;
        let names: Vec<String> = self.elements.iter().filter(|e| matches!(e.category, Cast | Props | Vehicles | Location | Animals | Wardrobe | Vfx)).map(|e| e.name.clone()).collect();
        let before = self.tasks.len();
        for n in names {
            self.add_task(Entity::Asset, &n, step, "", Status::Ready);
        }
        self.tasks.len() - before
    }

    /// Wakes the tasks whose earlier steps are done: a waiting shot task
    /// becomes ready when every stage it comes after is approved.
    pub fn promote_tasks(&mut self) -> Vec<u64> {
        let mut woke = Vec::new();
        let stages = self.pipeline.stages.clone();
        for i in 0..self.tasks.len() {
            let t = &self.tasks[i];
            if t.entity != Entity::Shot || t.status != Status::Waiting {
                continue;
            }
            let Some(stage) = stages.iter().find(|s| s.key == t.step) else { continue };
            let ok = stage.after.iter().all(|a| self.tasks.iter().find(|o| o.entity == Entity::Shot && o.target == t.target && o.step == *a).is_none_or(|o| o.status.is_done()));
            if ok {
                woke.push(t.id);
            }
        }
        for id in &woke {
            if let Some(t) = self.tasks.iter_mut().find(|t| t.id == *id) {
                t.status = Status::Ready;
                t.updated = now();
            }
        }
        woke
    }

    /// Sets a task's status, optionally with a note, and wakes what comes
    /// next. Returns the ids woken.
    pub fn set_status(&mut self, key: &str, status: Status, who: &str, note: &str) -> Result<Vec<u64>, String> {
        let i = self.task_index(key).ok_or_else(|| format!("no task \"{key}\" (tasks lists them; pipeline with tasks: true makes one per shot and stage)"))?;
        let old = self.tasks[i].status;
        let label = self.tasks[i].label();
        {
            let t = &mut self.tasks[i];
            t.status = status;
            t.updated = now();
            if !note.is_empty() {
                t.notes.push(Note { at: now(), who: who.to_owned(), text: note.to_owned() });
            }
            if t.assignee.is_empty() && matches!(status, Status::InProgress | Status::Review) && !who.is_empty() {
                t.assignee = who.to_owned();
            }
        }
        self.log(who, format!("{label}: {} → {}", old.name(), status.name()));
        Ok(if status.is_done() { self.promote_tasks() } else { Vec::new() })
    }

    /// Adds a note.
    pub fn add_note(&mut self, key: &str, who: &str, text: &str) -> Result<(), String> {
        let i = self.task_index(key).ok_or_else(|| format!("no task \"{key}\""))?;
        let t = &mut self.tasks[i];
        t.notes.push(Note { at: now(), who: who.to_owned(), text: text.to_owned() });
        t.updated = now();
        let l = t.label();
        self.log(who, format!("{l}: note"));
        Ok(())
    }

    /// Records a new version of a task's work. For a shot task the clips
    /// and span come from where the shot sits on the timeline now.
    pub fn add_version(&mut self, story: &Story, key: &str, who: &str, note: &str, media: Option<&str>) -> Result<u32, String> {
        let i = self.task_index(key).ok_or_else(|| format!("no task \"{key}\""))?;
        let (entity, target) = (self.tasks[i].entity, self.tasks[i].target.clone());
        let (mut clips, mut start, mut duration) = (Vec::new(), None, None);
        if entity == Entity::Shot {
            if let Some((si, hi)) = story.find_shot(&target) {
                let sh = &story.scenes[si].shots[hi];
                clips = sh.clips.iter().map(|c| c.0 as u32).collect();
                start = sh.start.map(|s| s as f64 / SECOND as f64);
                duration = Some(sh.duration as f64 / SECOND as f64);
                if sh.clips.is_empty() {
                    start = None;
                }
            }
        }
        let t = &mut self.tasks[i];
        let n = t.versions.last().map(|v| v.n).unwrap_or(0) + 1;
        t.versions.push(Version { n, at: now(), by: who.to_owned(), note: note.to_owned(), clips, start, duration, media: media.map(str::to_owned) });
        t.updated = now();
        let l = t.label();
        self.log(who, format!("{l}: version {n}"));
        Ok(n)
    }

    /// Everything about what is done and what is not.
    pub fn dashboard(&self, story: &Story, today: Date) -> Dashboard {
        let live: Vec<&Task> = self.tasks.iter().filter(|t| t.status != Status::Omitted).collect();
        let pct = |ts: &[&Task]| if ts.is_empty() { 0.0 } else { ts.iter().map(|t| t.status.progress()).sum::<f64>() / ts.len() as f64 * 100.0 };
        let by_status = Status::ALL.iter().map(|s| (*s, self.tasks.iter().filter(|t| t.status == *s).count())).filter(|(_, n)| *n > 0).collect();
        let mut step_names: Vec<&str> = Vec::new();
        for t in &live {
            if !step_names.contains(&t.step.as_str()) {
                step_names.push(&t.step);
            }
        }
        let steps = step_names
            .iter()
            .map(|s| {
                let ts: Vec<&Task> = live.iter().copied().filter(|t| t.step == *s).collect();
                StepRow { step: (*s).to_owned(), total: ts.len(), done: ts.iter().filter(|t| t.status.is_done()).count(), percent: pct(&ts) }
            })
            .collect();
        let mut names: Vec<&str> = live.iter().map(|t| t.assignee.as_str()).collect();
        names.sort();
        names.dedup();
        let people = names
            .into_iter()
            .filter(|n| !n.is_empty())
            .map(|n| {
                let mine = live.iter().filter(|t| t.assignee == n);
                (n.to_owned(), mine.clone().filter(|t| !t.status.is_done()).count(), mine.filter(|t| t.status.is_done()).count())
            })
            .collect();
        let order: Vec<(&str, &str)> = story.scenes.iter().flat_map(|s| s.shots.iter().map(move |h| (s.id.as_str(), h.id.as_str()))).collect();
        let mut shots = Vec::new();
        for (scene, shot) in order {
            let ts: Vec<&Task> = live.iter().copied().filter(|t| t.entity == Entity::Shot && t.target == shot).collect();
            if ts.is_empty() {
                continue;
            }
            // The step it is at: the first task (in pipeline order) not done.
            let rank = |t: &Task| self.pipeline.stages.iter().position(|s| s.key == t.step).unwrap_or(usize::MAX);
            let mut sorted = ts.clone();
            sorted.sort_by_key(|t| rank(t));
            let at = sorted.iter().find(|t| !t.status.is_done());
            let version = ts.iter().flat_map(|t| t.versions.iter().map(|v| v.n)).max().unwrap_or(0);
            shots.push(ShotRow { shot: shot.to_owned(), scene: scene.to_owned(), percent: pct(&ts), at_step: at.map(|t| t.step.clone()), status: at.map(|t| t.status), version });
        }
        let done = shots.iter().filter(|s| s.at_step.is_none()).count();
        let ids = |f: &dyn Fn(&Task) -> bool| live.iter().filter(|t| f(t)).map(|t| t.id).collect::<Vec<_>>();
        Dashboard {
            total: live.len(),
            by_status,
            percent: pct(&live),
            steps,
            people,
            shots_done: (done, shots.len()),
            shots,
            overdue: ids(&|t| !t.status.is_done() && t.due.as_deref().and_then(Date::parse).is_some_and(|d| d < today)),
            blocked: ids(&|t| t.status == Status::Blocked),
            in_review: ids(&|t| t.status == Status::Review),
            unassigned: live.iter().filter(|t| t.assignee.is_empty() && !t.status.is_done()).count(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::story::ClipId;
    use crate::story::{Scene, Shot};

    fn story() -> Story {
        let mut st = Story::default();
        st.scenes.push(Scene {
            id: "1".into(),
            shots: vec![Shot { id: "1A".into(), duration: 4 * SECOND, start: Some(2 * SECOND), clips: vec![ClipId(7), ClipId(8)], ..Default::default() }, Shot { id: "1B".into(), duration: 3 * SECOND, ..Default::default() }],
            ..Default::default()
        });
        st
    }

    #[test]
    fn finishing_a_stage_wakes_the_next() {
        let st = story();
        let mut p = Production::default();
        assert_eq!(p.ensure_pipeline_tasks(&st), 10);
        assert_eq!(p.ensure_pipeline_tasks(&st), 0, "no doubles");
        let status = |p: &Production, k: &str| p.tasks[p.task_index(k).unwrap()].status;
        assert_eq!(status(&p, "1A layout"), Status::Ready);
        assert_eq!(status(&p, "1A animation"), Status::Waiting);
        p.set_status("1A/layout", Status::InProgress, "claude", "blocking out").unwrap();
        assert_eq!(p.tasks[0].assignee, "claude", "whoever starts it has it");
        let woke = p.set_status("1A/layout", Status::Approved, "ira", "").unwrap();
        assert_eq!(woke.len(), 1);
        assert_eq!(status(&p, "1A animation"), Status::Ready);
        assert_eq!(status(&p, "1A lighting"), Status::Waiting);
        assert!(p.set_status("nope", Status::Final, "x", "").is_err());
        assert_eq!(Status::parse("RTK"), Some(Status::Revisions));
        assert_eq!(Status::parse("wip"), Some(Status::InProgress));
    }

    #[test]
    fn versions_point_at_the_timeline() {
        let st = story();
        let mut p = Production::default();
        p.ensure_pipeline_tasks(&st);
        let n = p.add_version(&st, "1A animation", "claude", "first pass", None).unwrap();
        assert_eq!(n, 1);
        let n = p.add_version(&st, "1A animation", "claude", "retimed", Some("anim_v2.mp4")).unwrap();
        assert_eq!(n, 2);
        let v = &p.tasks[p.task_index("1A animation").unwrap()].versions[1];
        assert_eq!(v.clips, vec![7, 8]);
        assert_eq!(v.start, Some(2.0));
        assert_eq!(v.duration, Some(4.0));
        let w = p.add_version(&st, "1B layout", "x", "", None).unwrap();
        assert_eq!(w, 1);
        assert!(p.tasks[p.task_index("1B layout").unwrap()].versions[0].clips.is_empty(), "an unbuilt shot is on no clips");
    }

    #[test]
    fn the_dashboard_counts_what_is_done() {
        let st = story();
        let mut p = Production::default();
        p.ensure_pipeline_tasks(&st);
        for step in ["layout", "animation", "lighting", "comp", "sound"] {
            p.set_status(&format!("1A/{step}"), Status::Final, "ira", "").unwrap();
        }
        p.set_status("1B/layout", Status::Review, "ira", "").unwrap();
        let i = p.task_index("1B layout").unwrap();
        p.tasks[i].due = Some("2026-01-01".into());
        p.set_status("1B/sound", Status::Blocked, "ira", "waiting on the score").unwrap();
        let d = p.dashboard(&st, Date::parse("2026-02-01").unwrap());
        assert_eq!(d.total, 10);
        assert_eq!(d.shots_done, (1, 2));
        assert_eq!(d.shots[0].percent, 100.0);
        assert_eq!(d.shots[1].at_step.as_deref(), Some("layout"));
        assert_eq!(d.shots[1].status, Some(Status::Review));
        assert_eq!(d.overdue.len(), 1);
        assert_eq!(d.blocked.len(), 1);
        assert_eq!(d.in_review.len(), 1);
        assert!((d.percent - (5.0 + 0.8) / 10.0 * 100.0).abs() < 1e-9);
        assert_eq!(d.people, vec![("ira".to_owned(), 1, 0)]);
        assert_eq!(d.unassigned, 4);
        assert!(d.steps.iter().any(|s| s.step == "layout" && s.done == 1 && s.total == 2));
        assert!(p.activity.len() >= 7);
    }
}
