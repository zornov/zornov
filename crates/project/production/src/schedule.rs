// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The stripboard: scenes ordered into shooting days, the day-out-of-days
//! for the cast, and the call sheet for a day.
//!
//! [`Production::auto_schedule`] orders the scenes the way a first
//! assistant director would: scenes at one location together, day and
//! night apart, a day filled to what the crew can shoot, and an actor never
//! called on a date they are unavailable. Scenes can then be moved by hand.

use crate::breakdown::{Category, DayNight, SceneFacts, facts};
use crate::date::{Date, clock, parse_clock};
use crate::{Production, Settings};
use serde::{Deserialize, Serialize};
use crate::story::Story;

/// The shooting days in order.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Schedule {
    /// Day 1, day 2…; each holds its scenes in shooting order.
    #[serde(default)]
    pub days: Vec<ShootDay>,
    /// When cast cannot work.
    #[serde(default)]
    pub unavailable: Vec<Unavailable>,
}

/// One shooting (or working) day.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ShootDay {
    /// The date, `YYYY-MM-DD`, once the schedule has a start date.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
    /// Scene ids in shooting order.
    #[serde(default)]
    pub scenes: Vec<String>,
    /// General call for the day when it is not the default, `HH:MM`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call: Option<String>,
    /// Notes for the call sheet (weather cover, special equipment…).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
}

/// Dates an actor cannot work.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Unavailable {
    /// The cast element (id, name or story character id).
    pub cast: String,
    /// The dates, `YYYY-MM-DD`.
    pub dates: Vec<String>,
}

/// One strip of the stripboard.
#[derive(Clone, Debug, PartialEq)]
pub struct Strip {
    /// Scene facts.
    pub facts: SceneFacts,
    /// Cast numbers in the scene.
    pub cast: Vec<u32>,
    /// Day index (0-based) it is scheduled on.
    pub day: Option<usize>,
}

/// A cast member's status on a day of the day-out-of-days.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cell {
    /// Not working, not held.
    Off,
    /// Start work.
    Sw,
    /// Work.
    W,
    /// Work finish.
    Wf,
    /// Start work finish (one day only).
    Swf,
    /// Hold: paid, not working, between work days.
    H,
}

impl Cell {
    /// The usual letters.
    pub fn code(self) -> &'static str {
        match self {
            Cell::Off => "",
            Cell::Sw => "SW",
            Cell::W => "W",
            Cell::Wf => "WF",
            Cell::Swf => "SWF",
            Cell::H => "H",
        }
    }
}

/// One cast row of the day-out-of-days.
#[derive(Clone, Debug, PartialEq)]
pub struct DoodRow {
    /// Cast number.
    pub number: u32,
    /// Name.
    pub name: String,
    /// One cell per day.
    pub cells: Vec<Cell>,
    /// Days worked.
    pub work: usize,
    /// Hold days.
    pub hold: usize,
    /// First work day to last, counting both.
    pub span: usize,
}

/// The day-out-of-days report.
#[derive(Clone, Debug, PartialEq)]
pub struct Dood {
    /// Per day: its number (1-based) and date.
    pub days: Vec<(usize, Option<String>)>,
    /// A row per cast member.
    pub rows: Vec<DoodRow>,
}

/// What `auto_schedule` did.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct ScheduleReport {
    /// Shooting days.
    pub days: usize,
    /// Scenes scheduled.
    pub scenes: usize,
    /// Scenes that could not be placed (named).
    pub unplaced: Vec<String>,
    /// Last date, when there is a start date.
    pub wrap: Option<String>,
}

/// A scene on a call sheet.
#[derive(Clone, Debug, PartialEq)]
pub struct CallScene {
    /// Scene id.
    pub id: String,
    /// Slug line.
    pub heading: String,
    /// `INT D`, `EXT N`.
    pub kind: String,
    /// The set.
    pub set: String,
    /// Pages.
    pub pages: String,
    /// Cast numbers.
    pub cast: Vec<u32>,
    /// What happens.
    pub summary: String,
    /// Estimated start, `HH:MM`.
    pub start: String,
}

/// A cast member's call.
#[derive(Clone, Debug, PartialEq)]
pub struct CallCast {
    /// Cast number.
    pub number: u32,
    /// Name.
    pub name: String,
    /// Scenes today.
    pub scenes: Vec<String>,
    /// Make-up / hair call.
    pub makeup: String,
    /// On set.
    pub on_set: String,
}

/// A day's call sheet.
#[derive(Clone, Debug, PartialEq)]
pub struct CallSheet {
    /// Film title.
    pub title: String,
    /// Day number, 1-based.
    pub day: usize,
    /// Days in the schedule.
    pub of: usize,
    /// Date and weekday.
    pub date: Option<String>,
    /// General crew call.
    pub general_call: String,
    /// Estimated wrap.
    pub wrap: String,
    /// Director.
    pub director: String,
    /// Producer.
    pub producer: String,
    /// Hospital.
    pub hospital: String,
    /// Production office.
    pub office: String,
    /// Locations with their notes.
    pub locations: Vec<(String, String)>,
    /// Scenes in order.
    pub scenes: Vec<CallScene>,
    /// Cast calls.
    pub cast: Vec<CallCast>,
    /// What each department brings, by category.
    pub requirements: Vec<(Category, Vec<String>)>,
    /// The day's notes.
    pub notes: String,
    /// Total pages today.
    pub pages: String,
    /// Tomorrow's scenes and date.
    pub tomorrow: Option<(Option<String>, Vec<String>)>,
}

impl Production {
    fn cast_key_matches(key: &str, e: &crate::Element) -> bool {
        key.eq_ignore_ascii_case(&e.id) || key.eq_ignore_ascii_case(&e.name) || e.story_id.as_deref().is_some_and(|s| key.eq_ignore_ascii_case(s))
    }

    /// Whether a cast element cannot work on a date.
    pub fn is_unavailable(&self, e: &crate::Element, date: &str) -> bool {
        self.schedule.unavailable.iter().any(|u| Self::cast_key_matches(&u.cast, e) && u.dates.iter().any(|d| d == date))
    }

    /// Marks an actor unavailable on dates.
    pub fn set_unavailable(&mut self, cast: &str, dates: Vec<String>) -> Result<(), String> {
        let e = self.element(cast).filter(|e| e.category == Category::Cast).ok_or_else(|| format!("no cast member \"{cast}\" (auto-tag the script first)"))?;
        let key = e.id.clone();
        for d in &dates {
            Date::parse(d).ok_or_else(|| format!("\"{d}\" is not a date (use YYYY-MM-DD)"))?;
        }
        self.schedule.unavailable.retain(|u| !u.cast.eq_ignore_ascii_case(&key));
        if !dates.is_empty() {
            self.schedule.unavailable.push(Unavailable { cast: key, dates });
        }
        Ok(())
    }

    fn cast_elements_in(&self, scene: &str) -> Vec<&crate::Element> {
        self.in_scene(scene).into_iter().filter(|(e, _)| e.category == Category::Cast).map(|(e, _)| e).collect()
    }

    /// Orders every scene into shooting days. Replaces the days (keeping
    /// availability); the start date, days off and capacity come from
    /// [`Settings`].
    pub fn auto_schedule(&mut self, story: &Story) -> ScheduleReport {
        let all = facts(story);
        let cap = self.settings.minutes_per_day();
        let max_loc = self.settings.max_locations_per_day.max(1);
        // Where a location first appears in the film ranks its group.
        let rank = |f: &SceneFacts| all.iter().filter(|g| g.place == f.place).map(|g| g.index).min().unwrap_or(f.index);
        let mut remaining: Vec<&SceneFacts> = all.iter().collect();
        remaining.sort_by_key(|f| (rank(f), f.day_night, f.tod as u8, f.index));
        let start = self.settings.start_date.as_deref().and_then(Date::parse);
        let mut date = start.map(|d| self.settings.next_working(d));
        let mut days: Vec<ShootDay> = Vec::new();
        let mut guard = 0;
        while !remaining.is_empty() && guard < 2000 {
            guard += 1;
            let dstr = date.map(|d| d.to_string());
            let mut day: Vec<&SceneFacts> = Vec::new();
            let mut mins = 0.0;
            let mut places: Vec<&str> = Vec::new();
            let mut dn: Option<DayNight> = None;
            loop {
                let pick = remaining.iter().position(|f| {
                    let m = f.secs / 60.0;
                    let fits_time = day.is_empty() || mins + m <= cap + 1e-9;
                    // A second location only when all of it still fits today: a
                    // location is not split over days to squeeze one more in.
                    let group: f64 = remaining.iter().filter(|g| g.place == f.place && g.day_night == f.day_night).map(|g| g.secs / 60.0).sum();
                    let fits_place = places.is_empty() || places.contains(&f.place.as_str()) || (places.len() < max_loc && group <= cap - mins + 1e-9);
                    let fits_dn = dn.is_none_or(|d| d == f.day_night);
                    let cast_ok = match &dstr {
                        Some(d) => self.cast_elements_in(&f.id).iter().all(|e| !self.is_unavailable(e, d)),
                        None => true,
                    };
                    fits_time && fits_place && fits_dn && cast_ok
                });
                let Some(i) = pick else { break };
                let f = remaining.remove(i);
                mins += f.secs / 60.0;
                if !places.contains(&f.place.as_str()) {
                    places.push(&f.place);
                }
                dn = Some(f.day_night);
                day.push(f);
            }
            if day.is_empty() {
                // Everyone left is unavailable today: the date passes.
                match date {
                    Some(d) if guard < 800 => {
                        date = Some(self.settings.next_working(d.plus(1)));
                        continue;
                    }
                    _ => break,
                }
            }
            // Within a day: interiors last-minute flexible; keep location
            // runs together, then story order.
            day.sort_by_key(|f| (places.iter().position(|p| *p == f.place).unwrap_or(0), f.tod as u8, f.index));
            days.push(ShootDay { date: dstr, scenes: day.iter().map(|f| f.id.clone()).collect(), call: None, notes: String::new() });
            date = date.map(|d| self.settings.next_working(d.plus(1)));
        }
        let unplaced: Vec<String> = remaining.iter().map(|f| f.id.clone()).collect();
        let report = ScheduleReport { days: days.len(), scenes: all.len() - unplaced.len(), unplaced, wrap: days.last().and_then(|d| d.date.clone()) };
        self.schedule.days = days;
        self.log("studio", format!("scheduled {} scenes over {} days", report.scenes, report.days));
        report
    }

    /// Gives the days dates from the start date, skipping days off.
    pub fn redate(&mut self) {
        let start = self.settings.start_date.as_deref().and_then(Date::parse);
        for (i, d) in self.schedule.days.iter_mut().enumerate() {
            d.date = start.map(|s| self.settings.working_day(s, i).to_string());
        }
    }

    /// The day (0-based) a scene is shot on.
    pub fn day_of(&self, scene: &str) -> Option<usize> {
        self.schedule.days.iter().position(|d| d.scenes.iter().any(|s| s.eq_ignore_ascii_case(scene)))
    }

    /// Moves a scene to `day` (0-based; one past the last adds a day) at
    /// `index` in it (default: the end).
    pub fn move_scene(&mut self, story: &Story, scene: &str, day: usize, index: Option<usize>) -> Result<(), String> {
        let id = story.scene(scene).map(|s| s.id.clone()).ok_or_else(|| format!("no scene \"{scene}\""))?;
        if day > self.schedule.days.len() {
            return Err(format!("there are {} days; day {} would leave a gap", self.schedule.days.len(), day + 1));
        }
        for d in &mut self.schedule.days {
            d.scenes.retain(|s| *s != id);
        }
        if day == self.schedule.days.len() {
            self.schedule.days.push(ShootDay::default());
        }
        let d = &mut self.schedule.days[day];
        let at = index.unwrap_or(d.scenes.len()).min(d.scenes.len());
        d.scenes.insert(at, id);
        self.schedule.days.retain(|d| !d.scenes.is_empty() || d.notes.len() > 0);
        self.redate();
        Ok(())
    }

    /// The stripboard: a strip per scene, in shooting order, then the
    /// unscheduled ones.
    pub fn strips(&self, story: &Story) -> Vec<Strip> {
        let all = facts(story);
        let number = |id: &str| self.elements.iter().find(|e| e.story_id.as_deref() == Some(id) && e.category == Category::Cast).and_then(|e| e.number);
        let mk = |f: &SceneFacts, day: Option<usize>| {
            let mut cast: Vec<u32> = self.cast_elements_in(&f.id).iter().filter_map(|e| e.number).collect();
            cast.sort();
            let _ = number;
            Strip { facts: f.clone(), cast, day }
        };
        let mut out = Vec::new();
        for (di, d) in self.schedule.days.iter().enumerate() {
            for s in &d.scenes {
                if let Some(f) = all.iter().find(|f| f.id == *s) {
                    out.push(mk(f, Some(di)));
                }
            }
        }
        for f in &all {
            if self.day_of(&f.id).is_none() {
                out.push(mk(f, None));
            }
        }
        out
    }

    /// Day-out-of-days for every cast member.
    pub fn dood(&self) -> Dood {
        let days: Vec<(usize, Option<String>)> = self.schedule.days.iter().enumerate().map(|(i, d)| (i + 1, d.date.clone())).collect();
        let mut rows = Vec::new();
        let mut cast: Vec<&crate::Element> = self.of(Category::Cast).collect();
        cast.sort_by_key(|e| e.number.unwrap_or(u32::MAX));
        for e in cast {
            let works: Vec<usize> = self
                .schedule
                .days
                .iter()
                .enumerate()
                .filter(|(_, d)| d.scenes.iter().any(|s| self.tags.iter().any(|t| t.element == e.id && t.qty > 0 && t.scene == *s)))
                .map(|(i, _)| i)
                .collect();
            let mut cells = vec![Cell::Off; days.len()];
            let (mut work, mut hold) = (0, 0);
            if let (Some(&a), Some(&b)) = (works.first(), works.last()) {
                for (i, c) in cells.iter_mut().enumerate().take(b + 1).skip(a) {
                    *c = if works.contains(&i) {
                        work += 1;
                        match (a == b, i == a, i == b) {
                            (true, _, _) => Cell::Swf,
                            (_, true, _) => Cell::Sw,
                            (_, _, true) => Cell::Wf,
                            _ => Cell::W,
                        }
                    } else {
                        hold += 1;
                        Cell::H
                    };
                }
            }
            let span = works.first().zip(works.last()).map(|(a, b)| b - a + 1).unwrap_or(0);
            rows.push(DoodRow { number: e.number.unwrap_or(0), name: e.name.clone(), cells, work, hold, span });
        }
        Dood { days, rows }
    }

    /// Days a cast element works (from the schedule).
    pub fn work_days(&self, element: &str) -> usize {
        self.dood().rows.iter().zip(self.of(Category::Cast).collect::<Vec<_>>()).find(|(_, e)| e.id == element).map(|(r, _)| r.work).unwrap_or(0)
    }

    /// Problems with the schedule worth a producer's attention.
    pub fn schedule_warnings(&self, story: &Story) -> Vec<String> {
        let all = facts(story);
        let cap = self.settings.minutes_per_day();
        let mut w = Vec::new();
        for f in &all {
            if self.day_of(&f.id).is_none() {
                w.push(format!("scene {} is not scheduled", f.id));
            }
        }
        for (i, d) in self.schedule.days.iter().enumerate() {
            let fs: Vec<&SceneFacts> = d.scenes.iter().filter_map(|s| all.iter().find(|f| f.id == *s)).collect();
            let mins: f64 = fs.iter().map(|f| f.secs / 60.0).sum();
            if mins > cap * 1.15 && fs.len() > 1 {
                w.push(format!("day {} carries {:.1} min of film, over the {:.1} the crew can shoot", i + 1, mins, cap));
            }
            let mut places: Vec<&str> = fs.iter().map(|f| f.place.as_str()).collect();
            places.sort();
            places.dedup();
            if places.len() > self.settings.max_locations_per_day {
                w.push(format!("day {} visits {} locations", i + 1, places.len()));
            }
            if fs.iter().any(|f| f.day_night == DayNight::Day) && fs.iter().any(|f| f.day_night == DayNight::Night) {
                w.push(format!("day {} mixes day and night scenes", i + 1));
            }
            if let Some(date) = &d.date {
                for s in &d.scenes {
                    for e in self.cast_elements_in(s) {
                        if self.is_unavailable(e, date) {
                            w.push(format!("day {} ({date}): {} is unavailable but is in scene {s}", i + 1, e.name));
                        }
                    }
                }
            }
        }
        w
    }

    /// The call sheet of a day (0-based).
    pub fn call_sheet(&self, story: &Story, day: usize) -> Result<CallSheet, String> {
        let d = self.schedule.days.get(day).ok_or_else(|| format!("there is no day {} (the schedule has {} days; run schedule with auto first)", day + 1, self.schedule.days.len()))?;
        let all = facts(story);
        let s: &Settings = &self.settings;
        let general = d.call.clone().unwrap_or_else(|| s.general_call.clone());
        let g = parse_clock(&general).unwrap_or(7 * 60);
        // Camera is ready an hour after crew call; each scene takes the
        // hours its minutes of film are worth.
        let mut t = g + 60;
        let mut scenes = Vec::new();
        let mut eighths = 0;
        let mut first_in: Vec<(u32, String, String, i32, Vec<String>)> = Vec::new();
        for sid in &d.scenes {
            let Some(f) = all.iter().find(|f| f.id == *sid) else { continue };
            let cast: Vec<&crate::Element> = self.cast_elements_in(&f.id);
            for e in &cast {
                match first_in.iter_mut().find(|c| c.0 == e.number.unwrap_or(0)) {
                    Some(c) => c.4.push(f.id.clone()),
                    None => first_in.push((e.number.unwrap_or(0), e.name.clone(), String::new(), t, vec![f.id.clone()])),
                }
            }
            let mut nums: Vec<u32> = cast.iter().filter_map(|e| e.number).collect();
            nums.sort();
            scenes.push(CallScene { id: f.id.clone(), heading: f.heading.clone(), kind: format!("{} {}", f.int_ext(), f.dn()), set: f.set.clone(), pages: f.pages(), cast: nums, summary: f.summary.clone(), start: clock(t) });
            eighths += f.eighths();
            t += ((f.secs / 60.0) * s.hours_per_screen_minute * 60.0).round() as i32;
        }
        first_in.sort_by_key(|c| c.0);
        let cast = first_in.into_iter().map(|(number, name, _, on, scenes)| CallCast { number, name, scenes, makeup: clock(on - 75), on_set: clock(on) }).collect();
        let mut places: Vec<(String, String)> = Vec::new();
        for sid in &d.scenes {
            if let Some(f) = all.iter().find(|f| f.id == *sid) {
                if !places.iter().any(|(n, _)| *n == f.set) {
                    let note = story.place(&f.place).map(|p| p.notes.clone()).unwrap_or_default();
                    places.push((f.set.clone(), note));
                }
            }
        }
        let mut requirements: Vec<(Category, Vec<String>)> = Vec::new();
        for c in Category::ALL {
            if matches!(c, Category::Cast | Category::Location) {
                continue;
            }
            let mut items: Vec<String> = Vec::new();
            for sid in &d.scenes {
                for (e, tag) in self.in_scene(sid) {
                    if e.category == c {
                        let label = if tag.qty > 1 { format!("{} x{} (sc {sid})", e.name, tag.qty) } else { format!("{} (sc {sid})", e.name) };
                        items.push(label);
                    }
                }
            }
            if !items.is_empty() {
                requirements.push((c, items));
            }
        }
        let tomorrow = self.schedule.days.get(day + 1).map(|n| (n.date.clone(), n.scenes.clone()));
        Ok(CallSheet {
            title: if story.title.is_empty() { "Untitled".into() } else { story.title.clone() },
            day: day + 1,
            of: self.schedule.days.len(),
            date: d.date.clone(),
            general_call: general,
            wrap: clock(t + 30),
            director: s.director.clone(),
            producer: s.producer.clone(),
            hospital: s.hospital.clone(),
            office: s.office.clone(),
            locations: places,
            scenes,
            cast,
            requirements,
            notes: d.notes.clone(),
            pages: crate::breakdown::pages_text(eighths),
            tomorrow,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::story::{Character, Line, Place, Scene, Shot};
    use crate::story::SECOND;

    /// Six scenes at three places: two in the field (day), two in the hut
    /// (one night), two at the river.
    fn film() -> Story {
        let mut st = Story::default();
        for c in ["a", "b", "c"] {
            st.cast.push(Character { id: c.into(), name: c.to_uppercase(), ..Default::default() });
        }
        for (id, interior) in [("field", false), ("hut", true), ("river", false)] {
            st.places.push(Place { id: id.into(), name: id.to_uppercase(), interior, ..Default::default() });
        }
        let plan: [(&str, &str, &str, &[&str], i64); 6] = [
            ("1", "field", "EXT. FIELD - DAY", &["a"], 100),
            ("2", "hut", "INT. HUT - DAY", &["a", "b"], 100),
            ("3", "field", "EXT. FIELD - DAY", &["b"], 100),
            ("4", "river", "EXT. RIVER - DAY", &["c"], 100),
            ("5", "hut", "INT. HUT - NIGHT", &["a"], 100),
            ("6", "river", "EXT. RIVER - DAY", &["a", "c"], 100),
        ];
        for (id, place, head, cast, secs) in plan {
            st.scenes.push(Scene {
                id: id.into(),
                heading: head.into(),
                place: Some(place.into()),
                cast: cast.iter().map(|c| c.to_string()).collect(),
                lines: vec![Line { text: "x".into(), ..Default::default() }],
                shots: vec![Shot { id: format!("{id}A"), duration: secs * SECOND, ..Default::default() }],
                ..Default::default()
            });
        }
        st
    }

    fn prod(st: &Story) -> Production {
        let mut p = Production::default();
        p.auto_tag(st);
        p.settings.start_date = Some("2026-11-02".into()); // a Monday
        // 100 s a scene; two scenes fit a day (3.3 min/day).
        p.settings.shoot_hours = 8.0;
        p.settings.hours_per_screen_minute = 2.4;
        p
    }

    #[test]
    fn scenes_group_by_location_and_day_and_night_stay_apart() {
        let st = film();
        let mut p = prod(&st);
        let r = p.auto_schedule(&st);
        assert!(r.unplaced.is_empty());
        let days: Vec<Vec<&str>> = p.schedule.days.iter().map(|d| d.scenes.iter().map(String::as_str).collect()).collect();
        assert!(days.contains(&vec!["1", "3"]), "the two field scenes are one day: {days:?}");
        assert!(days.contains(&vec!["4", "6"]), "the two river scenes are one day: {days:?}");
        let night = days.iter().find(|d| d.contains(&"5")).unwrap();
        assert!(!night.contains(&"2"), "night apart from day: {days:?}");
        assert!(p.schedule_warnings(&st).is_empty(), "{:?}", p.schedule_warnings(&st));
    }

    #[test]
    fn days_skip_weekends_and_actors_who_are_away() {
        let st = film();
        let mut p = prod(&st);
        // B is away on the first shooting Monday.
        p.set_unavailable("B", vec!["2026-11-02".into()]).unwrap();
        p.auto_schedule(&st);
        let dates: Vec<_> = p.schedule.days.iter().map(|d| d.date.clone().unwrap()).collect();
        assert!(dates.iter().all(|d| Date::parse(d).unwrap().weekday() < 5), "no weekends: {dates:?}");
        let monday = &p.schedule.days[0];
        assert_eq!(monday.date.as_deref(), Some("2026-11-02"));
        for s in &monday.scenes {
            assert!(!["2", "3"].contains(&s.as_str()), "B's scenes are not on Monday: {:?}", monday.scenes);
        }
        assert!(p.schedule_warnings(&st).is_empty());
        // Moving a scene onto a bad date is flagged.
        p.move_scene(&st, "3", 0, None).unwrap();
        assert!(p.schedule_warnings(&st).iter().any(|w| w.contains("B is unavailable")), "{:?}", p.schedule_warnings(&st));
    }

    #[test]
    fn day_out_of_days_marks_start_work_hold_and_finish() {
        let st = film();
        let mut p = prod(&st);
        p.auto_schedule(&st);
        let d = p.dood();
        assert_eq!(d.rows.len(), 3);
        for row in &d.rows {
            assert_eq!(row.work + row.hold, row.span, "{row:?}");
            let codes: Vec<&str> = row.cells.iter().map(|c| c.code()).filter(|c| !c.is_empty()).collect();
            assert!(codes.first().is_some_and(|c| *c == "SW" || *c == "SWF"), "{codes:?}");
            assert!(codes.last().is_some_and(|c| *c == "WF" || *c == "SWF"), "{codes:?}");
        }
        // Hand-built case: an actor on days 1 and 3 is held on day 2.
        let mut q = Production::default();
        q.auto_tag(&st);
        q.schedule.days = vec![ShootDay { scenes: vec!["1".into()], ..Default::default() }, ShootDay { scenes: vec!["4".into()], ..Default::default() }, ShootDay { scenes: vec!["3".into(), "5".into()], ..Default::default() }];
        let a = &q.dood().rows[0];
        assert_eq!(a.name, "A");
        assert_eq!(a.cells.iter().map(|c| c.code()).collect::<Vec<_>>(), vec!["SW", "H", "WF"]);
        assert_eq!((a.work, a.hold, a.span), (2, 1, 3));
    }

    #[test]
    fn strips_move_and_call_sheets_list_what_the_day_needs() {
        let st = film();
        let mut p = prod(&st);
        p.settings.director = "Ira".into();
        p.auto_schedule(&st);
        p.move_scene(&st, "6", 0, Some(0)).unwrap();
        assert_eq!(p.day_of("6"), Some(0));
        assert_eq!(p.schedule.days[0].scenes[0], "6");
        assert!(p.move_scene(&st, "6", 99, None).is_err());
        let strips = p.strips(&st);
        assert_eq!(strips.len(), 6);
        assert!(strips.iter().all(|s| s.day.is_some()));
        let cs = p.call_sheet(&st, 0).unwrap();
        assert_eq!(cs.day, 1);
        assert_eq!(cs.general_call, "07:00");
        assert_eq!(cs.scenes[0].id, "6");
        assert_eq!(cs.scenes[0].start, "08:00");
        assert!(cs.cast.iter().all(|c| c.makeup < c.on_set), "make-up before set: {:?}", cs.cast);
        assert_eq!(cs.director, "Ira");
        assert!(p.call_sheet(&st, 99).is_err());
    }
}
