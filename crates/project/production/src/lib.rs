// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The production office: everything a producer, line producer and
//! production coordinator keep, built on the story the director works from
//! ([`story::Story`], read from the project).
//!
//! * [`breakdown`]: the screenplay broken down scene by scene into cast,
//!   locations, props, wardrobe, VFX, sound, vehicles, animals and the rest,
//!   with a breakdown sheet per scene.
//! * [`schedule`]: the stripboard, shooting days, day-out-of-days for the
//!   cast and call sheets; [`pipeline`]: the same for animated work, a
//!   layout → animation → lighting → comp → sound schedule per shot.
//! * [`budget`]: templates, top sheet and accounts, costs per scene and
//!   shot, actuals against budget, currencies, and the AI generation spend
//!   as it is incurred.
//! * [`tasks`]: per-shot and per-asset tasks with status, assignee, notes
//!   and versions linked to the timeline, and the dashboards over them.
//! * [`pdf`] and [`report`]: Rust-written PDFs and plain text of all of it.
//!
//! The data is a [`Production`], kept as `production.json` beside the
//! project. It refers to scenes and shots by their story ids and
//! never copies the script, so the story stays the one source of truth.

#![warn(missing_docs)]

pub mod breakdown;
pub mod budget;
pub mod date;
pub mod pdf;
pub mod pipeline;
pub mod report;
pub mod schedule;
pub mod story;
pub mod tasks;

pub use breakdown::{Category, Element, SceneSheet, Tag};
pub use budget::{Account, Actual, Budget, Feed, Line, Section, Spend};
pub use pipeline::{PipelineConfig, Stage};
pub use schedule::{Schedule, ShootDay, Unavailable};
pub use tasks::{Activity, Note, Status, Task, Version};

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The file name in the project folder.
pub const FILE: &str = "production.json";

/// Everything the production office keeps for one project.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Production {
    /// Format version of this file.
    #[serde(default = "one")]
    pub version: u32,
    /// Calendar, hours, people and money defaults.
    #[serde(default)]
    pub settings: Settings,
    /// Breakdown elements (cast, props, locations…).
    #[serde(default)]
    pub elements: Vec<Element>,
    /// Which element is in which scene.
    #[serde(default)]
    pub tags: Vec<Tag>,
    /// Shooting days, availability.
    #[serde(default)]
    pub schedule: Schedule,
    /// The animation pipeline.
    #[serde(default)]
    pub pipeline: PipelineConfig,
    /// The budget.
    #[serde(default)]
    pub budget: Budget,
    /// What generating with AI has cost, run by run.
    #[serde(default)]
    pub spend: Vec<Spend>,
    /// Tasks per shot, scene and asset.
    #[serde(default)]
    pub tasks: Vec<Task>,
    /// Who did what, newest last.
    #[serde(default)]
    pub activity: Vec<Activity>,
    /// Next id for tasks, budget lines and spend entries.
    #[serde(default)]
    pub next_id: u64,
}

fn one() -> u32 {
    1
}

/// Calendar and crew defaults.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    /// First shooting day (or first production day), `YYYY-MM-DD`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_date: Option<String>,
    /// Weekdays nobody works, 0 = Monday … 6 = Sunday.
    #[serde(default = "default_days_off")]
    pub days_off: Vec<u8>,
    /// Dates nobody works (holidays), `YYYY-MM-DD`.
    #[serde(default)]
    pub blackout: Vec<String>,
    /// Hours on set (or at the desk) in a working day.
    #[serde(default = "ten")]
    pub shoot_hours: f64,
    /// Hours of shooting for each minute of finished film.
    #[serde(default = "two_and_half")]
    pub hours_per_screen_minute: f64,
    /// General crew call, `HH:MM`.
    #[serde(default = "seven")]
    pub general_call: String,
    /// Most locations a shooting day may visit.
    #[serde(default = "two")]
    pub max_locations_per_day: usize,
    /// The film is animated or generated: no cast days, a pipeline instead.
    #[serde(default)]
    pub animated: bool,
    /// Director, for the call sheet.
    #[serde(default)]
    pub director: String,
    /// Producer, for the call sheet.
    #[serde(default)]
    pub producer: String,
    /// Nearest hospital, for the call sheet.
    #[serde(default)]
    pub hospital: String,
    /// Production office phone, for the call sheet.
    #[serde(default)]
    pub office: String,
}

fn default_days_off() -> Vec<u8> {
    vec![5, 6]
}
fn ten() -> f64 {
    10.0
}
fn two_and_half() -> f64 {
    2.5
}
fn seven() -> String {
    "07:00".into()
}
fn two() -> usize {
    2
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            start_date: None,
            days_off: default_days_off(),
            blackout: Vec::new(),
            shoot_hours: ten(),
            hours_per_screen_minute: two_and_half(),
            general_call: seven(),
            max_locations_per_day: two(),
            animated: false,
            director: String::new(),
            producer: String::new(),
            hospital: String::new(),
            office: String::new(),
        }
    }
}

impl Settings {
    /// Minutes of finished film one working day can shoot.
    pub fn minutes_per_day(&self) -> f64 {
        (self.shoot_hours / self.hours_per_screen_minute.max(0.1)).max(0.25)
    }

    /// Whether a date is a working day.
    pub fn is_working(&self, d: date::Date) -> bool {
        !self.days_off.contains(&d.weekday()) && !self.blackout.iter().any(|b| date::Date::parse(b) == Some(d))
    }

    /// The first working day on or after `d`.
    pub fn next_working(&self, mut d: date::Date) -> date::Date {
        // Seven days off a week would never end: stop after a year.
        for _ in 0..366 {
            if self.is_working(d) {
                return d;
            }
            d = d.plus(1);
        }
        d
    }

    /// The date `n` working days after `start` (0 = `start` itself, moved
    /// to a working day).
    pub fn working_day(&self, start: date::Date, n: usize) -> date::Date {
        let mut d = self.next_working(start);
        for _ in 0..n {
            d = self.next_working(d.plus(1));
        }
        d
    }
}

impl Production {
    /// A fresh id.
    pub fn alloc_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    /// Reads `production.json` from a project folder; an empty production
    /// when there is none.
    pub fn load(dir: &Path) -> Result<Production, String> {
        let p = dir.join(FILE);
        match std::fs::read_to_string(&p) {
            Ok(s) => serde_json::from_str(&s).map_err(|e| format!("{} is not readable: {e}", p.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Production::default()),
            Err(e) => Err(format!("could not read {}: {e}", p.display())),
        }
    }

    /// Writes `production.json`, replacing the old file in one step.
    pub fn save(&self, dir: &Path) -> Result<(), String> {
        let p = dir.join(FILE);
        let tmp = dir.join(format!(".{FILE}.tmp"));
        let bytes = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(&tmp, bytes).map_err(|e| format!("could not write {}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, &p).map_err(|e| format!("could not write {}: {e}", p.display()))
    }

    /// Adds a line to the activity log (kept to the last 500).
    pub fn log(&mut self, who: &str, text: impl Into<String>) {
        self.activity.push(Activity { at: now(), who: who.to_owned(), text: text.into() });
        let n = self.activity.len();
        if n > 500 {
            self.activity.drain(..n - 500);
        }
    }

    /// Records what an AI generation cost, against a shot or scene when
    /// known, and returns the entry's id. Called by the app after every
    /// paid run, so the budget follows spending without anyone typing it in.
    pub fn record_spend(&mut self, mut s: Spend) -> u64 {
        s.id = self.alloc_id();
        if s.at == 0 {
            s.at = now();
        }
        let id = s.id;
        let note = format!("{} {} ${:.2}", s.provider, s.model, s.cents as f64 / 100.0);
        self.spend.push(s);
        self.log("studio", format!("spent {note}"));
        id
    }
}

/// Appends a spend entry to a project's `production.json` (creating it),
/// for callers that only have the folder.
pub fn record_spend_in(dir: &Path, s: Spend) -> Result<u64, String> {
    let mut p = Production::load(dir)?;
    let id = p.record_spend(s);
    p.save(dir)?;
    Ok(id)
}

/// Where a project's production file is.
pub fn path_in(dir: &Path) -> PathBuf {
    dir.join(FILE)
}

/// Seconds since the Unix epoch.
pub fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Lower-case slug for ids.
pub(crate) fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("studio-production-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn the_office_is_saved_beside_the_project_and_read_back() {
        let d = scratch("save");
        assert_eq!(Production::load(&d).unwrap(), Production::default(), "no file is an empty office");
        let mut p = Production::default();
        p.settings.start_date = Some("2026-11-02".into());
        p.tag("1", Category::Props, "bell", 2, "").unwrap();
        p.add_task(tasks::Entity::Shot, "1A", "layout", "ira", Status::Ready);
        p.budget.accounts.push(Account { code: "1".into(), name: "A".into(), section: Section::Atl, lines: vec![] });
        p.add_line("1", "Writer", 1.0, "flat", 100.0, Some("INR"), true).unwrap();
        p.save(&d).unwrap();
        assert!(!d.join(".production.json.tmp").exists());
        assert_eq!(Production::load(&d).unwrap(), p);
        std::fs::write(d.join(FILE), "{ nope").unwrap();
        assert!(Production::load(&d).unwrap_err().contains("not readable"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn spend_is_appended_without_touching_the_rest() {
        let d = scratch("spend");
        let mut p = Production::default();
        p.settings.director = "Ira".into();
        p.save(&d).unwrap();
        let id = record_spend_in(&d, Spend { provider: "openai".into(), model: "image-model".into(), kind: "picture".into(), cents: 8, ..Default::default() }).unwrap();
        let id2 = record_spend_in(&d, Spend { provider: "openai".into(), model: "video-model".into(), kind: "video".into(), cents: 250, shot: Some("1A".into()), ..Default::default() }).unwrap();
        assert!(id2 > id);
        let back = Production::load(&d).unwrap();
        assert_eq!(back.settings.director, "Ira");
        assert_eq!(back.spend.len(), 2);
        assert!(back.spend[0].at > 0);
        assert_eq!(back.spend[1].shot.as_deref(), Some("1A"));
        let _ = std::fs::remove_dir_all(&d);
    }
}
