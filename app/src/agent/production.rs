// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `production`: the production office. The script is broken down scene by
//! scene, scheduled into shooting days, budgeted (AI spend included) and
//! tracked task by task, from the story the project already holds. The
//! office is kept as `production.json` beside the project.

use super::{Session, f};
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};
use studio_production::breakdown::Category;
use studio_production::story::Story;
use studio_production::budget::{Section, Spend};
use studio_production::tasks::{Entity, Status, today};
use studio_production::{Production, report};

fn s<'a>(a: &'a Value, k: &str) -> Option<&'a str> {
    a.get(k).and_then(Value::as_str).filter(|x| !x.trim().is_empty())
}

fn strings(a: &Value, k: &str) -> Vec<String> {
    match a.get(k) {
        Some(Value::Array(v)) => v.iter().filter_map(Value::as_str).map(str::to_owned).collect(),
        Some(Value::String(x)) => x.split(',').map(|p| p.trim().to_owned()).filter(|p| !p.is_empty()).collect(),
        _ => Vec::new(),
    }
}

fn weekday(v: &Value) -> Option<u8> {
    if let Some(n) = v.as_u64() {
        return (n < 7).then_some(n as u8);
    }
    let t = v.as_str()?.trim().to_lowercase();
    ["mon", "tue", "wed", "thu", "fri", "sat", "sun"].iter().position(|d| t.starts_with(d)).map(|i| i as u8)
}

fn r2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

fn line_json(p: &Production, a: &studio_production::Account, l: &studio_production::Line) -> Value {
    let mut o = Map::new();
    o.insert("id".into(), json!(l.id));
    o.insert("account".into(), json!(a.code));
    o.insert("desc".into(), json!(l.desc));
    o.insert("qty".into(), json!(r2(l.qty * l.mult)));
    o.insert("unit".into(), json!(l.unit));
    o.insert("rate".into(), json!(r2(l.rate)));
    o.insert("currency".into(), json!(l.currency.clone().unwrap_or_else(|| p.budget.currency.clone())));
    o.insert("budget".into(), json!(r2(p.budget.line_budget(l))));
    o.insert("actual".into(), json!(r2(p.budget.line_actual(l))));
    for (k, v) in [("scene", &l.scene), ("shot", &l.shot), ("element", &l.element)] {
        if let Some(v) = v {
            o.insert(k.into(), json!(v));
        }
    }
    if l.feed.is_some() {
        o.insert("feed".into(), json!("ai_spend"));
    }
    if l.closed {
        o.insert("closed".into(), json!(true));
    }
    Value::Object(o)
}

fn task_json(t: &studio_production::Task) -> Value {
    let mut o = Map::new();
    o.insert("id".into(), json!(t.id));
    o.insert(t.entity.name().into(), json!(t.target));
    o.insert("step".into(), json!(t.step));
    o.insert("status".into(), json!(t.status.name()));
    if !t.assignee.is_empty() {
        o.insert("assignee".into(), json!(t.assignee));
    }
    if let Some(d) = &t.due {
        o.insert("due".into(), json!(d));
    }
    if let Some(n) = t.notes.last() {
        o.insert("last_note".into(), json!(format!("{}: {}", n.who, n.text)));
    }
    if let Some(v) = t.versions.last() {
        o.insert("version".into(), json!(v.n));
    }
    Value::Object(o)
}

/// What the tool does, for the tool list.
pub(super) const DESCRIPTION: &str = "The production office kept with the project: breakdown, schedule, budget and task tracking. Field `do`: show, settings, breakdown, tag, schedule, availability, dood, call_sheet, pipeline, budget, line, actual, tasks, task, status, version, dashboard, export (others in the long form); plus scene, shot, day, name, qty, task, status, step, assignee, path. Days count from 1. Set your own task in_progress when you start it, review when ready, and report from dashboard. More: guide {topic: \"tool:production\"}.";

/// Every argument: name, type (s string, i integer, n number, b boolean, a array, o object) and what it means.
const ARGS: &[(&str, char, &str)] = &[
    ("do", 's', "show (default), settings, breakdown, elements, tag, untag, schedule, availability, dood, call_sheet, pipeline, budget, account, line, actual, tasks, task, status, version, asset_tasks, dashboard, export"),
    ("who", 's', "who is acting, for the activity log and assignee (default ai)"),
    ("scene", 's', "scene id"),
    ("shot", 's', "shot id"),
    ("day", 'i', "shooting day, from 1"),
    ("category", 's', "tag, elements: cast, extras, location, props, wardrobe, makeup, vehicles, animals, stunts, sfx, vfx, sound, music"),
    ("name", 's', "tag / untag: the element"),
    ("qty", 'n', "tag, line: how many"),
    ("note", 's', "tag, status, version, actual"),
    ("retag", 'b', "breakdown: read the script again"),
    ("auto", 'b', "schedule: make the days"),
    ("move", 's', "schedule: a scene id to move"),
    ("to_day", 'i', "schedule move: the day"),
    ("index", 'i', "schedule move: place in the day, from 1"),
    ("set_day", 'i', "schedule: change a day's call and notes"),
    ("call", 's', "HH:MM"),
    ("notes", 's', "a day's notes"),
    ("start_date", 's', "YYYY-MM-DD"),
    ("days_off", 'a', "weekdays nobody works: sat, sun…"),
    ("blackout", 'a', "dates nobody works"),
    ("cast", 's', "availability: the actor"),
    ("dates", 'a', "availability: dates they cannot work"),
    ("preset", 's', "pipeline: animation or ai"),
    ("artists", 'o', "pipeline: artists per stage, e.g. {\"animation\": 4}"),
    ("tasks", 'b', "pipeline: make a task per shot and stage"),
    ("template", 's', "budget"),
    ("scale", 'n', "budget template: multiplies every rate"),
    ("currency", 's', "budget, line, actual: a code like USD, INR"),
    ("rates", 'o', "budget: what one unit of a currency is worth in the budget currency"),
    ("fringe_pct", 'n', "budget"),
    ("contingency_pct", 'n', "budget"),
    ("view", 's', "budget: top, accounts, scenes"),
    ("code", 's', "account: the code"),
    ("section", 's', "account: atl, btl, post, other"),
    ("account", 's', "line: the account code to add to"),
    ("desc", 's', "line"),
    ("unit", 's', "line"),
    ("rate", 'n', "line"),
    ("element", 's', "line: a breakdown element its cost is spread over"),
    ("feed", 's', "line: ai_spend"),
    ("id", 'i', "line: change this line"),
    ("remove", 'i', "line: delete this line"),
    ("line", 'i', "actual: the line id"),
    ("amount", 'n', "actual"),
    ("close", 'b', "actual: the line is final"),
    ("cents", 'i', "spend: record this many US cents"),
    ("dollars", 'n', "spend"),
    ("model", 's', "spend"),
    ("task", 's', "status, version: a task id or \"1A layout\""),
    ("status", 's', "status"),
    ("step", 's', "task, tasks"),
    ("assignee", 's', "task, status, tasks"),
    ("due", 's', "YYYY-MM-DD"),
    ("entity", 's', "task: shot, scene, asset, project"),
    ("target", 's', "task: the shot id, scene id or asset name"),
    ("media", 's', "version: a media item or file"),
    ("what", 's', "export: breakdown, schedule, call_sheet, all_call_sheets, budget, pipeline, progress"),
    ("pdf", 'b', "call_sheet: also write the PDF"),
    ("path", 's', "export: where to write (a folder or a .pdf)"),
    ("limit", 'i', "tasks: most to list (default 60)"),
    ("director", 's', "settings: director, for the call sheet"),
    ("producer", 's', "settings: producer, for the call sheet"),
    ("hospital", 's', "settings: nearest hospital, for the call sheet"),
    ("office", 's', "settings: production office phone"),
    ("shoot_hours", 'n', "settings: hours in a working day (10)"),
    ("hours_per_screen_minute", 'n', "settings: hours of shooting per minute of film (2.5)"),
    ("general_call", 's', "settings: general crew call, HH:MM"),
    ("max_locations_per_day", 'i', "settings: most locations a day may visit"),
    ("animated", 'b', "settings: the film is animated or generated"),
    ("labour", 'b', "line: pay of people, so fringes are added"),
    ("reschedule", 'b', "availability: make the shooting days again"),
];

/// The input schema.
pub(super) fn schema() -> Value {
    let mut props = Map::new();
    for (name, kind, desc) in ARGS {
        let ty = match kind {
            'i' => "integer",
            'n' => "number",
            'b' => "boolean",
            'a' => "array",
            'o' => "object",
            _ => "string",
        };
        props.insert((*name).to_owned(), json!({"type": ty, "description": desc}));
    }
    json!({"type": "object", "properties": props, "required": []})
}

/// The project's story as the office reads it: the project saved as JSON,
/// its `story` part read into the office's own view of it (nothing is
/// written back, and a project without a story gives an empty one).
pub fn story_of(p: &studio_model::Project) -> Story {
    serde_json::to_value(p).ok().and_then(|mut v| v.get_mut("story").map(Value::take)).and_then(|s| serde_json::from_value(s).ok()).unwrap_or_default()
}

impl Session {
    /// The story this project holds.
    fn production_story(&self) -> Story {
        story_of(&self.doc.model)
    }

    /// Where the production papers are kept: the project folder.
    fn production_dir(&self) -> Option<PathBuf> {
        self.home.clone().or_else(|| self.doc.folder.as_ref().map(|f| f.root().to_path_buf()))
    }

    /// The office, read from the project folder (or kept in the session
    /// until the project is saved).
    fn production_load(&self) -> Result<Production, String> {
        match self.production_dir() {
            Some(d) => Production::load(&d),
            None => Ok(self.production.clone().unwrap_or_default()),
        }
    }

    fn production_save(&mut self, p: &Production) -> Result<(), String> {
        match self.production_dir() {
            Some(d) => p.save(&d),
            None => {
                self.production = Some(p.clone());
                Ok(())
            }
        }
    }

    /// Records what an AI run cost (and, when it was for a shot, a task and
    /// a version of that shot's generation) in the production office.
    pub(super) fn production_record_generation(&mut self, a: &Value, spend: Spend, media: &[String]) {
        let Ok(mut p) = self.production_load() else { return };
        let who = s(a, "who").unwrap_or("ai").to_owned();
        let shot = spend.shot.clone();
        p.record_spend(Spend { by: who.clone(), ..spend });
        if let Some(sh) = shot {
            let story = self.production_story();
            if story.find_shot(&sh).is_some() {
                let key = p.add_task(Entity::Shot, &sh, "generate", &who, Status::Review);
                let _ = p.set_status(&key.to_string(), Status::Review, &who, "generated");
                let _ = p.add_version(&story, &key.to_string(), &who, "generated", media.first().map(String::as_str));
            }
        }
        let _ = self.production_save(&p);
    }

    /// The `production` tool.
    pub(super) fn production_tool(&mut self, a: &Value) -> Result<Value, String> {
        let story = self.production_story();
        let mut p = self.production_load()?;
        let before = p.clone();
        let who = s(a, "who").unwrap_or("ai").to_owned();
        let what = s(a, "do").unwrap_or("show").to_lowercase();
        let out = self.production_do(&what, a, &story, &mut p, &who);
        if p != before {
            self.production_save(&p)?;
        }
        out
    }

    fn production_out_dir(&self) -> PathBuf {
        match self.production_dir() {
            Some(d) => d.join("production"),
            None => studio_paths::cache().join("production"),
        }
    }

    /// Writes a paper to `path` (a file, or a folder to put `name` in).
    fn production_write(&self, a: &Value, name: &str, bytes: &[u8]) -> Result<String, String> {
        let target = match s(a, "path") {
            Some(x) => {
                let x = Path::new(x);
                if x.is_dir() || x.extension().is_none() { x.join(name) } else { x.to_path_buf() }
            }
            None => self.production_out_dir().join(name),
        };
        if let Some(d) = target.parent() {
            std::fs::create_dir_all(d).map_err(|e| format!("could not make {}: {e}", d.display()))?;
        }
        std::fs::write(&target, bytes).map_err(|e| format!("could not write {}: {e}", target.display()))?;
        Ok(target.display().to_string())
    }

    fn production_do(&mut self, what: &str, a: &Value, story: &Story, p: &mut Production, who: &str) -> Result<Value, String> {
        let day_arg = |a: &Value| -> Result<usize, String> {
            let n = a.get("day").and_then(Value::as_u64).ok_or("needs \"day\" (1 is the first shooting day)")?;
            n.checked_sub(1).map(|n| n as usize).ok_or_else(|| "days count from 1".to_owned())
        };
        match what {
            "show" | "overview" => {
                let d = p.dashboard(story, today());
                let mut o = json!({
                    "overview": report::overview_line(p, story),
                    "progress": format!("{:.0}% of {} tasks; shots finished {}/{}", d.percent, d.total, d.shots_done.0, d.shots_done.1),
                    "budget": report::budget_text(&p.budget_report(), None).lines().rev().take(2).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>(),
                });
                let warns = p.schedule_warnings(story);
                if !warns.is_empty() {
                    o["schedule_warnings"] = json!(warns);
                }
                if p.elements.is_empty() && !story.scenes.is_empty() {
                    o["next"] = json!("run do: breakdown to tag the script, then schedule (auto), budget (a template), pipeline (tasks: true)");
                } else if story.scenes.is_empty() {
                    o["next"] = json!("there is no story yet: use the story tool to import the screenplay first");
                }
                if !d.blocked.is_empty() || !d.overdue.is_empty() {
                    o["attention"] = json!({"blocked": d.blocked, "overdue": d.overdue});
                }
                Ok(o)
            }
            "settings" => {
                let st = &mut p.settings;
                if let Some(x) = s(a, "start_date") {
                    studio_production::date::Date::parse(x).ok_or_else(|| format!("\"{x}\" is not a date (YYYY-MM-DD)"))?;
                    st.start_date = Some(x.to_owned());
                }
                if let Some(v) = a.get("days_off").and_then(Value::as_array) {
                    st.days_off = v.iter().filter_map(weekday).collect();
                }
                if a.get("blackout").is_some() {
                    let b = strings(a, "blackout");
                    for d in &b {
                        studio_production::date::Date::parse(d).ok_or_else(|| format!("\"{d}\" is not a date (YYYY-MM-DD)"))?;
                    }
                    st.blackout = b;
                }
                if let Some(x) = f(a, "shoot_hours") {
                    st.shoot_hours = x.clamp(1.0, 24.0);
                }
                if let Some(x) = f(a, "hours_per_screen_minute") {
                    st.hours_per_screen_minute = x.clamp(0.1, 100.0);
                }
                if let Some(x) = s(a, "general_call") {
                    studio_production::date::parse_clock(x).ok_or("general_call is HH:MM")?;
                    st.general_call = x.to_owned();
                }
                if let Some(x) = a.get("max_locations_per_day").and_then(Value::as_u64) {
                    st.max_locations_per_day = x.max(1) as usize;
                }
                if let Some(x) = a.get("animated").and_then(Value::as_bool) {
                    st.animated = x;
                }
                for (k, slot) in [("director", &mut st.director), ("producer", &mut st.producer), ("hospital", &mut st.hospital), ("office", &mut st.office)] {
                    if let Some(x) = a.get(k).and_then(Value::as_str) {
                        *slot = x.to_owned();
                    }
                }
                p.redate();
                Ok(json!({"settings": serde_json::to_value(&p.settings).unwrap_or_default(), "minutes_of_film_per_day": r2(p.settings.minutes_per_day())}))
            }
            "breakdown" => {
                if story.scenes.is_empty() {
                    return Err("there are no scenes: import or write the screenplay with the story tool first".into());
                }
                let mut o = Map::new();
                if a.get("retag").and_then(Value::as_bool).unwrap_or(p.tags.is_empty()) {
                    let r = p.auto_tag(story);
                    o.insert("tagged".into(), json!({"scenes": r.scenes, "tags": r.tags, "elements": r.elements, "hand_made_kept": r.kept_manual}));
                }
                let scene = s(a, "scene");
                o.insert("sheets".into(), json!(report::breakdown_text(p, story, scene)?));
                if !p.elements.is_empty() {
                    let counts: Vec<String> = Category::ALL.iter().filter_map(|c| Some(format!("{} {}", c.name(), p.of(*c).count())).filter(|_| p.of(*c).count() > 0)).collect();
                    o.insert("elements".into(), json!(counts.join(", ")));
                }
                o.insert("hint".into(), json!("The script is read by keyword: fix what it got wrong with tag / untag (scene, category, name)"));
                Ok(Value::Object(o))
            }
            "elements" => {
                let cat = s(a, "category").map(|c| Category::parse(c).ok_or_else(|| format!("no category \"{c}\"")));
                let cat = cat.transpose()?;
                let list: Vec<Value> = p
                    .elements
                    .iter()
                    .filter(|e| cat.is_none_or(|c| e.category == c))
                    .map(|e| json!({"id": e.id, "category": e.category.name(), "name": e.name, "number": e.number, "scenes": p.scenes_of(&e.id)}))
                    .collect();
                Ok(json!({"elements": list}))
            }
            "tag" => {
                let scene = story.scene(s(a, "scene").ok_or("tag needs \"scene\"")?).ok_or("no such scene")?.id.clone();
                let cat = Category::parse(s(a, "category").ok_or("tag needs \"category\" (cast, extras, location, props, wardrobe, makeup, vehicles, animals, stunts, sfx, vfx, sound, music)")?).ok_or("unknown category")?;
                let name = s(a, "name").ok_or("tag needs \"name\"")?;
                p.tag(&scene, cat, name, a.get("qty").and_then(Value::as_u64).unwrap_or(1) as u32, s(a, "note").unwrap_or(""))?;
                Ok(json!({"tagged": name, "scene": scene, "sheet": report::breakdown_text(p, story, Some(&scene))?}))
            }
            "untag" => {
                let scene = story.scene(s(a, "scene").ok_or("untag needs \"scene\"")?).ok_or("no such scene")?.id.clone();
                let name = s(a, "name").ok_or("untag needs \"name\"")?;
                if !p.untag(&scene, name) {
                    return Err(format!("\"{name}\" is not tagged in scene {scene}"));
                }
                Ok(json!({"removed": name, "scene": scene}))
            }
            "schedule" => {
                if story.scenes.is_empty() {
                    return Err("there are no scenes to schedule".into());
                }
                if p.tags.is_empty() {
                    p.auto_tag(story);
                }
                let mut o = Map::new();
                if let Some(x) = s(a, "start_date") {
                    studio_production::date::Date::parse(x).ok_or_else(|| format!("\"{x}\" is not a date (YYYY-MM-DD)"))?;
                    p.settings.start_date = Some(x.to_owned());
                }
                if a.get("auto").and_then(Value::as_bool).unwrap_or(p.schedule.days.is_empty()) {
                    let r = p.auto_schedule(story);
                    o.insert("scheduled".into(), json!({"days": r.days, "scenes": r.scenes, "unplaced": r.unplaced, "wrap": r.wrap}));
                }
                if let (Some(scene), Some(day)) = (s(a, "move"), a.get("to_day").and_then(Value::as_u64)) {
                    let index = a.get("index").and_then(Value::as_u64).map(|i| (i as usize).saturating_sub(1));
                    p.move_scene(story, scene, (day as usize).saturating_sub(1), index)?;
                    o.insert("moved".into(), json!(format!("scene {scene} to day {day}")));
                }
                if let Some(n) = a.get("set_day").and_then(Value::as_u64) {
                    let i = (n as usize).saturating_sub(1);
                    let d = p.schedule.days.get_mut(i).ok_or_else(|| format!("no day {n}"))?;
                    if let Some(c) = s(a, "call") {
                        studio_production::date::parse_clock(c).ok_or("call is HH:MM")?;
                        d.call = Some(c.to_owned());
                    }
                    if let Some(t) = a.get("notes").and_then(Value::as_str) {
                        d.notes = t.to_owned();
                    }
                }
                o.insert("stripboard".into(), json!(report::stripboard_text(p, story)));
                let w = p.schedule_warnings(story);
                if !w.is_empty() {
                    o.insert("warnings".into(), json!(w));
                }
                Ok(Value::Object(o))
            }
            "availability" => {
                let cast = s(a, "cast").ok_or("availability needs \"cast\" (a cast member) and \"dates\" (the dates they cannot work; none clears them)")?;
                p.set_unavailable(cast, strings(a, "dates"))?;
                if a.get("reschedule").and_then(Value::as_bool).unwrap_or(false) {
                    p.auto_schedule(story);
                }
                Ok(json!({"unavailable": p.schedule.unavailable.iter().map(|u| json!({"cast": u.cast, "dates": u.dates})).collect::<Vec<_>>(), "warnings": p.schedule_warnings(story)}))
            }
            "dood" => {
                let d = p.dood();
                if d.rows.is_empty() {
                    return Err("no cast is scheduled: breakdown and schedule first".into());
                }
                Ok(json!({"day_out_of_days": report::dood_text(&d)}))
            }
            "call_sheet" => {
                let i = day_arg(a)?;
                let c = p.call_sheet(story, i)?;
                let mut o = json!({"call_sheet": report::call_sheet_text(&c)});
                if a.get("pdf").and_then(Value::as_bool).unwrap_or(false) || s(a, "path").is_some() {
                    o["pdf"] = json!(self.production_write(a, &format!("call-sheet-day-{}.pdf", i + 1), &report::call_sheet_pdf(&c))?);
                }
                Ok(o)
            }
            "pipeline" => {
                let artists: Vec<(String, u32)> = a.get("artists").and_then(Value::as_object).map(|m| m.iter().filter_map(|(k, v)| v.as_u64().map(|n| (k.clone(), n as u32))).collect()).unwrap_or_default();
                p.set_pipeline(s(a, "preset"), &artists)?;
                let mut o = Map::new();
                if a.get("tasks").and_then(Value::as_bool).unwrap_or(false) {
                    let n = p.ensure_pipeline_tasks(story);
                    o.insert("tasks_made".into(), json!(n));
                }
                if let Some(x) = s(a, "start_date") {
                    studio_production::date::Date::parse(x).ok_or_else(|| format!("\"{x}\" is not a date (YYYY-MM-DD)"))?;
                    p.settings.start_date = Some(x.to_owned());
                }
                let plan = p.pipeline_plan(story)?;
                if plan.shots.is_empty() {
                    return Err("the story has no shots to plan (plan them with the story tool)".into());
                }
                o.insert("pipeline".into(), json!(report::pipeline_text(p, &plan)));
                o.insert("total_days".into(), json!(r2(plan.total_days)));
                if let Some(d) = plan.end_date {
                    o.insert("done_by".into(), json!(d));
                }
                if let Some(b) = plan.bottleneck {
                    o.insert("bottleneck".into(), json!(b));
                }
                Ok(Value::Object(o))
            }
            "budget" => {
                let mut o = Map::new();
                if let Some(c) = s(a, "currency") {
                    p.budget.set_currency(c)?;
                }
                if let Some(r) = a.get("rates").and_then(Value::as_object) {
                    for (k, v) in r {
                        p.budget.set_rate(k, v.as_f64().ok_or("rates are numbers: what one unit is worth in the budget currency")?)?;
                    }
                }
                if let Some(x) = f(a, "fringe_pct") {
                    p.budget.fringe_pct = x.clamp(0.0, 100.0);
                }
                if let Some(x) = f(a, "contingency_pct") {
                    p.budget.contingency_pct = x.clamp(0.0, 100.0);
                }
                if let Some(t) = s(a, "template") {
                    if p.tags.is_empty() && !story.scenes.is_empty() {
                        p.auto_tag(story);
                    }
                    let n = p.apply_budget_template(story, t, f(a, "scale").unwrap_or(1.0))?;
                    o.insert("template".into(), json!(format!("{t}: {n} lines, sized from the breakdown and schedule; replace the placeholder rates with real quotes")));
                }
                let r = p.budget_report();
                match s(a, "view").unwrap_or("top") {
                    "accounts" | "lines" => {
                        let lines: Vec<Value> = p.budget.accounts.iter().flat_map(|ac| ac.lines.iter().map(move |l| (ac, l))).map(|(ac, l)| line_json(p, ac, l)).collect();
                        o.insert("lines".into(), json!(lines));
                    }
                    "scenes" | "shots" => {
                        o.insert("sheet".into(), json!(report::budget_text(&r, Some(&p.scene_costs(story)))));
                    }
                    _ => {
                        o.insert("sheet".into(), json!(report::budget_text(&r, None)));
                    }
                }
                o.insert("total".into(), json!({"currency": r.currency, "budget": r2(r.total), "spent": r2(r.actual), "at_completion": r2(r.eac), "headroom": r2(r.variance), "ai_spend": r2(r.ai_spend)}));
                Ok(Value::Object(o))
            }
            "account" => {
                let code = s(a, "code").ok_or("account needs \"code\"")?;
                let b = &mut p.budget;
                let i = match b.accounts.iter().position(|x| x.code == code) {
                    Some(i) => i,
                    None => {
                        b.accounts.push(studio_production::Account { code: code.to_owned(), name: code.to_owned(), section: Section::Btl, lines: Vec::new() });
                        b.accounts.len() - 1
                    }
                };
                if let Some(n) = s(a, "name") {
                    b.accounts[i].name = n.to_owned();
                }
                if let Some(sec) = s(a, "section") {
                    b.accounts[i].section = Section::parse(sec).ok_or("section: atl, btl, post, other")?;
                }
                Ok(json!({"account": code, "name": b.accounts[i].name}))
            }
            "line" => {
                if let Some(id) = a.get("remove").and_then(Value::as_u64) {
                    let mut gone = false;
                    for ac in &mut p.budget.accounts {
                        let n = ac.lines.len();
                        ac.lines.retain(|l| l.id != id);
                        gone |= ac.lines.len() != n;
                    }
                    return if gone { Ok(json!({"removed": id})) } else { Err(format!("no budget line {id}")) };
                }
                let id = match a.get("id").and_then(Value::as_u64) {
                    Some(id) => {
                        let known: Vec<String> = p.budget.rates.keys().cloned().collect();
                        let l = p.line_mut(id).ok_or_else(|| format!("no budget line {id}"))?;
                        if let Some(x) = s(a, "desc") {
                            l.desc = x.to_owned();
                        }
                        if let Some(x) = f(a, "qty") {
                            l.qty = x;
                        }
                        if let Some(x) = f(a, "rate") {
                            l.rate = x;
                        }
                        if let Some(x) = f(a, "mult") {
                            l.mult = x;
                        }
                        if let Some(x) = s(a, "unit") {
                            l.unit = x.to_owned();
                        }
                        if let Some(c) = s(a, "currency") {
                            let c = c.to_uppercase();
                            if !known.contains(&c) {
                                return Err(format!("no exchange rate for {c}: set it with budget rates first"));
                            }
                            l.currency = Some(c);
                        }
                        if let Some(x) = a.get("closed").and_then(Value::as_bool) {
                            l.closed = x;
                        }
                        id
                    }
                    None => p.add_line(
                        s(a, "account").ok_or("line needs \"account\" (a code) to add to, or \"id\" to change")?,
                        s(a, "desc").ok_or("line needs \"desc\"")?,
                        f(a, "qty").unwrap_or(1.0),
                        s(a, "unit").unwrap_or("flat"),
                        f(a, "rate").ok_or("line needs \"rate\"")?,
                        s(a, "currency"),
                        a.get("labour").and_then(Value::as_bool).unwrap_or(false),
                    )?,
                };
                let story_ref = |k: &str| s(a, k).map(str::to_owned);
                let (sc, sh, el) = (story_ref("scene"), story_ref("shot"), story_ref("element"));
                if let Some(x) = &sh {
                    story.find_shot(x).ok_or_else(|| format!("no shot \"{x}\""))?;
                }
                if let Some(x) = &sc {
                    story.scene(x).ok_or_else(|| format!("no scene \"{x}\""))?;
                }
                let el = match el {
                    Some(e) => Some(p.element(&e).map(|x| x.id.clone()).ok_or_else(|| format!("no breakdown element \"{e}\""))?),
                    None => None,
                };
                let l = p.line_mut(id).unwrap();
                if sc.is_some() {
                    l.scene = sc;
                }
                if sh.is_some() {
                    l.shot = sh;
                }
                if el.is_some() {
                    l.element = el;
                }
                if s(a, "feed") == Some("ai_spend") {
                    l.feed = Some(studio_production::Feed::AiSpend);
                }
                let (ac, l) = p.budget.accounts.iter().find_map(|ac| ac.lines.iter().find(|l| l.id == id).map(|l| (ac, l))).unwrap();
                Ok(json!({"line": line_json(p, ac, l)}))
            }
            "actual" => {
                let id = a.get("line").and_then(Value::as_u64).ok_or("actual needs \"line\" (a budget line id; budget view: accounts lists them)")?;
                let amount = f(a, "amount").ok_or("actual needs \"amount\"")?;
                p.add_actual(id, amount, s(a, "currency"), s(a, "note").unwrap_or(""))?;
                if a.get("close").and_then(Value::as_bool).unwrap_or(false) {
                    if let Some(l) = p.line_mut(id) {
                        l.closed = true;
                    }
                }
                let (ac, l) = p.budget.accounts.iter().find_map(|ac| ac.lines.iter().find(|l| l.id == id).map(|l| (ac, l))).unwrap();
                Ok(json!({"line": line_json(p, ac, l)}))
            }
            "spend" => {
                if let Some(cents) = a.get("cents").and_then(Value::as_u64).or_else(|| f(a, "dollars").map(|d| (d * 100.0).round() as u64)) {
                    let id = p.record_spend(Spend {
                        provider: s(a, "provider").unwrap_or("manual").into(),
                        model: s(a, "model").unwrap_or("-").into(),
                        kind: s(a, "kind").unwrap_or("").into(),
                        cents,
                        prompt: s(a, "note").unwrap_or("").into(),
                        scene: s(a, "scene").map(str::to_owned),
                        shot: s(a, "shot").map(str::to_owned),
                        by: who.into(),
                        ..Default::default()
                    });
                    return Ok(json!({"recorded": id}));
                }
                let total: u64 = p.spend.iter().map(|x| x.cents).sum();
                let mut by_model: Vec<(String, u64, usize)> = Vec::new();
                for x in &p.spend {
                    match by_model.iter_mut().find(|m| m.0 == x.model) {
                        Some(m) => {
                            m.1 += x.cents;
                            m.2 += 1;
                        }
                        None => by_model.push((x.model.clone(), x.cents, 1)),
                    }
                }
                let recent: Vec<Value> = p.spend.iter().rev().take(15).map(|x| json!({"model": x.model, "kind": x.kind, "usd": r2(x.cents as f64 / 100.0), "shot": x.shot, "scene": x.scene, "by": x.by})).collect();
                Ok(json!({"total_usd": r2(total as f64 / 100.0), "runs": p.spend.len(), "by_model": by_model.iter().map(|m| json!({"model": m.0, "usd": r2(m.1 as f64 / 100.0), "runs": m.2})).collect::<Vec<_>>(), "recent": recent}))
            }
            "tasks" => {
                let status = s(a, "status").map(|x| Status::parse(x).ok_or_else(|| format!("no status \"{x}\" ({})", Status::ALL.map(Status::name).join(", ")))).transpose()?;
                let limit = a.get("limit").and_then(Value::as_u64).unwrap_or(60) as usize;
                let list: Vec<&studio_production::Task> = p
                    .tasks
                    .iter()
                    .filter(|t| status.is_none_or(|x| t.status == x))
                    .filter(|t| s(a, "step").is_none_or(|x| t.step.eq_ignore_ascii_case(x)))
                    .filter(|t| s(a, "assignee").is_none_or(|x| t.assignee.eq_ignore_ascii_case(x)))
                    .filter(|t| s(a, "shot").is_none_or(|x| t.target.eq_ignore_ascii_case(x)))
                    .filter(|t| s(a, "entity").and_then(Entity::parse).is_none_or(|e| t.entity == e))
                    .collect();
                Ok(json!({"count": list.len(), "tasks": list.iter().take(limit).map(|t| task_json(t)).collect::<Vec<_>>()}))
            }
            "task" => {
                let entity = s(a, "entity").map(|e| Entity::parse(e).ok_or("entity: shot, scene, asset, project")).transpose()?.unwrap_or(Entity::Shot);
                let target = s(a, "target").or_else(|| s(a, "shot")).ok_or("task needs \"target\" (a shot id, scene id or asset name) and \"step\"")?;
                if entity == Entity::Shot && story.find_shot(target).is_none() {
                    return Err(format!("no shot \"{target}\""));
                }
                let step = s(a, "step").ok_or("task needs \"step\" (layout, animation, generate, design…)")?;
                let status = s(a, "status").map(|x| Status::parse(x).ok_or_else(|| format!("no status \"{x}\""))).transpose()?.unwrap_or(Status::Ready);
                let id = p.add_task(entity, target, step, s(a, "assignee").unwrap_or(""), status);
                if let Some(d) = s(a, "due") {
                    studio_production::date::Date::parse(d).ok_or("due is YYYY-MM-DD")?;
                    let i = p.task_index(&id.to_string()).unwrap();
                    p.tasks[i].due = Some(d.to_owned());
                }
                let i = p.task_index(&id.to_string()).unwrap();
                Ok(json!({"task": task_json(&p.tasks[i])}))
            }
            "status" | "set" | "update" => {
                let key = s(a, "task").ok_or("status needs \"task\": an id, or \"1A layout\"")?;
                let i = p.task_index(key).ok_or_else(|| format!("no task \"{key}\""))?;
                let mut woke = Vec::new();
                if let Some(x) = s(a, "status") {
                    let st = Status::parse(x).ok_or_else(|| format!("no status \"{x}\" ({})", Status::ALL.map(Status::name).join(", ")))?;
                    woke = p.set_status(key, st, who, s(a, "note").unwrap_or(""))?;
                } else if let Some(n) = s(a, "note") {
                    p.add_note(key, who, n)?;
                }
                if let Some(x) = a.get("assignee").and_then(Value::as_str) {
                    p.tasks[i].assignee = x.to_owned();
                }
                if let Some(d) = s(a, "due") {
                    studio_production::date::Date::parse(d).ok_or("due is YYYY-MM-DD")?;
                    p.tasks[i].due = Some(d.to_owned());
                }
                let mut o = json!({"task": task_json(&p.tasks[i])});
                if !woke.is_empty() {
                    o["now_ready"] = json!(woke.iter().filter_map(|id| p.tasks.iter().find(|t| t.id == *id)).map(|t| t.label()).collect::<Vec<_>>());
                }
                Ok(o)
            }
            "version" => {
                let key = s(a, "task").ok_or("version needs \"task\"")?;
                let n = p.add_version(story, key, who, s(a, "note").unwrap_or(""), s(a, "media"))?;
                let i = p.task_index(key).unwrap();
                Ok(json!({"version": n, "task": task_json(&p.tasks[i]), "linked_clips": p.tasks[i].versions.last().map(|v| v.clips.clone()).unwrap_or_default()}))
            }
            "asset_tasks" => {
                let n = p.ensure_asset_tasks(s(a, "step").unwrap_or("design"));
                Ok(json!({"tasks_made": n}))
            }
            "dashboard" | "progress" => {
                let d = p.dashboard(story, today());
                Ok(json!({"dashboard": report::dashboard_text(&d, p), "percent": r2(d.percent), "shots_done": [d.shots_done.0, d.shots_done.1], "recent": p.activity.iter().rev().take(8).map(|x| format!("{}: {}", x.who, x.text)).collect::<Vec<_>>()}))
            }
            "export" => {
                let paper = s(a, "what").ok_or("export needs \"what\": breakdown, schedule, call_sheet, budget, pipeline, progress")?;
                let (name, bytes): (String, Vec<u8>) = match paper {
                    "breakdown" => ("breakdown.pdf".into(), report::breakdown_pdf(p, story, s(a, "scene"))?),
                    "schedule" | "stripboard" => ("schedule.pdf".into(), report::schedule_pdf(p, story)),
                    "call_sheet" => {
                        let i = day_arg(a)?;
                        (format!("call-sheet-day-{}.pdf", i + 1), report::call_sheet_pdf(&p.call_sheet(story, i)?))
                    }
                    "all_call_sheets" => {
                        let mut paths = Vec::new();
                        for i in 0..p.schedule.days.len() {
                            paths.push(self.production_write(a, &format!("call-sheet-day-{}.pdf", i + 1), &report::call_sheet_pdf(&p.call_sheet(story, i)?))?);
                        }
                        return Ok(json!({"written": paths}));
                    }
                    "budget" => ("budget.pdf".into(), report::budget_pdf(p, story)),
                    "pipeline" => ("pipeline.pdf".into(), report::pipeline_pdf(p, story, &p.pipeline_plan(story)?)),
                    "progress" | "dashboard" => ("progress.pdf".into(), report::dashboard_pdf(&p.dashboard(story, today()), p, story)),
                    other => return Err(format!("no paper \"{other}\" (breakdown, schedule, call_sheet, all_call_sheets, budget, pipeline, progress)")),
                };
                Ok(json!({"written": self.production_write(a, &name, &bytes)?}))
            }
            other => Err(format!("production has no \"{other}\"; do: show, settings, breakdown, elements, tag, untag, schedule, availability, dood, call_sheet, pipeline, budget, account, line, actual, spend, tasks, task, status, version, asset_tasks, dashboard, export")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Content;
    use crate::services;

    /// A small film in the project's own story JSON: three scenes, shots, a bible.
    const FILM: &str = r#"{"title":"The Lantern","places":[{"id":"meadow","name":"Meadow"},{"id":"den","name":"Den","interior":true}],"cast":[{"id":"fern","name":"Fern"},{"id":"heron","name":"Heron"}],"scenes":[{"id":"1","heading":"EXT. MEADOW - DAWN","place":"meadow","cast":["fern","heron"],"lines":[{"kind":"action","text":"Fern the fox carries a lantern across the meadow. Thunder rolls as rain falls."},{"kind":"dialogue","who":"fern","text":"Is anyone there?"},{"kind":"dialogue","who":"heron","text":"Only me!"}],"shots":[{"id":"1A","duration":3500000},{"id":"1B","duration":2000000,"subjects":["fern","heron"]},{"id":"1C","duration":1600000}]},{"id":"2","heading":"INT. DEN - NIGHT","place":"den","cast":["fern"],"lines":[{"kind":"action","text":"A candle burns. A dragon glows in the smoke while villagers sing a song."},{"kind":"dialogue","who":"fern","text":"I am home."}],"shots":[{"id":"2A","duration":4000000},{"id":"2B","duration":2000000}]},{"id":"3","heading":"EXT. MEADOW - DAY","place":"meadow","cast":["fern","heron"],"lines":[{"kind":"action","text":"Fern rides a horse to the river."}],"shots":[{"id":"3A","duration":5000000}]}]}"#;

    /// Puts the film in a session's project the way a saved project holds it.
    fn give_story(s: &mut Session) {
        let mut v = serde_json::to_value(&s.doc.model).unwrap();
        v["story"] = serde_json::from_str(FILM).unwrap();
        s.doc.model = serde_json::from_value(v).unwrap();
    }

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("studio-production-agent-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn json_of(r: Result<Vec<Content>, String>, what: &Value) -> Value {
        match r.unwrap_or_else(|e| panic!("{what}: {e}")).into_iter().next() {
            Some(Content::Text(t)) => serde_json::from_str(&t).unwrap(),
            _ => panic!("no text"),
        }
    }

    fn prod(s: &mut Session, a: Value) -> Value {
        json_of(s.call("production", &a), &a)
    }

    fn with_story(dir: &Path) -> Session {
        let mut s = Session::at(dir).unwrap();
        give_story(&mut s);
        s
    }

    #[test]
    fn a_script_becomes_a_breakdown_schedule_budget_and_task_board() {
        let dir = scratch("flow");
        let mut s = with_story(&dir);
        let shows = prod(&mut s, json!({}));
        assert!(shows["next"].as_str().unwrap().contains("breakdown"), "{shows}");

        let b = prod(&mut s, json!({"do": "breakdown"}));
        let sheets = b["sheets"].as_str().unwrap();
        assert!(sheets.contains("SCENE 1") && sheets.contains("lantern") && sheets.contains("thunder"), "{sheets}");
        assert!(sheets.contains("dragon") && sheets.contains("villagers"), "{sheets}");
        assert!(prod(&mut s, json!({"do": "elements", "category": "cast"}))["elements"].as_array().unwrap().len() >= 2);
        // Correct the reading by hand.
        let t = prod(&mut s, json!({"do": "tag", "scene": "1", "category": "props", "name": "silver bell", "qty": 2}));
        assert!(t["sheet"].as_str().unwrap().contains("silver bell x2"));
        assert!(s.call("production", &json!({"do": "untag", "scene": "1", "name": "lantern"})).is_ok());
        assert!(s.call("production", &json!({"do": "untag", "scene": "1", "name": "lantern"})).is_err());
        let again = prod(&mut s, json!({"do": "breakdown", "retag": true}));
        assert!(!again["sheets"].as_str().unwrap().contains("lantern,") || again["tagged"]["hand_made_kept"].as_u64().unwrap() >= 2, "{again}");
        assert!(again["sheets"].as_str().unwrap().contains("silver bell"), "a hand-made tag survives");

        prod(&mut s, json!({"do": "settings", "start_date": "2026-11-02", "director": "Ira Sen", "hospital": "City Hospital", "shoot_hours": 8}));
        let sc = prod(&mut s, json!({"do": "schedule"}));
        assert!(sc["scheduled"]["days"].as_u64().unwrap() >= 2, "{sc}");
        assert!(sc["stripboard"].as_str().unwrap().contains("DAY 1"), "{sc}");
        let m = prod(&mut s, json!({"do": "schedule", "move": "3", "to_day": 1, "index": 1}));
        assert!(m["moved"].as_str().is_some(), "{m}");
        assert!(s.call("production", &json!({"do": "schedule", "move": "99", "to_day": 1})).is_err());
        assert!(s.call("production", &json!({"do": "availability", "cast": "Nobody", "dates": ["2026-11-02"]})).is_err());
        prod(&mut s, json!({"do": "availability", "cast": "Fern", "dates": ["2026-11-04"], "reschedule": true}));
        assert!(prod(&mut s, json!({"do": "dood"}))["day_out_of_days"].as_str().unwrap().contains("Fern"));
        let cs = prod(&mut s, json!({"do": "call_sheet", "day": 1, "pdf": true}));
        assert!(cs["call_sheet"].as_str().unwrap().contains("Ira Sen"));
        let pdf = std::fs::read(cs["pdf"].as_str().unwrap()).unwrap();
        assert!(pdf.starts_with(b"%PDF-1.4"));
        assert!(s.call("production", &json!({"do": "call_sheet", "day": 99})).is_err());
        assert!(s.call("production", &json!({"do": "call_sheet"})).is_err(), "a day is required");

        // Budget, in rupees.
        let b = prod(&mut s, json!({"do": "budget", "template": "live_action_short", "currency": "INR", "contingency_pct": 12}));
        assert!(b["template"].as_str().unwrap().contains("lines"), "{b}");
        assert!(b["total"]["budget"].as_f64().unwrap() > 0.0 && b["total"]["currency"] == "INR", "{b}");
        let lines = prod(&mut s, json!({"do": "budget", "view": "accounts"}));
        let first = lines["lines"].as_array().unwrap().iter().find(|l| l["desc"].as_str().unwrap().starts_with("Screenplay")).unwrap();
        let id = first["id"].as_u64().unwrap();
        let l = prod(&mut s, json!({"do": "line", "id": id, "rate": 100000, "currency": "INR", "scene": "1"}));
        assert_eq!(l["line"]["budget"], 100000.0, "{l}");
        let a = prod(&mut s, json!({"do": "actual", "line": id, "amount": 120000, "currency": "INR", "note": "invoice", "close": true}));
        assert_eq!(a["line"]["actual"], 120000.0);
        let sheet = prod(&mut s, json!({"do": "budget", "view": "scenes"}));
        assert!(sheet["sheet"].as_str().unwrap().contains("scene 1") && sheet["sheet"].as_str().unwrap().contains('₹'), "{sheet}");
        assert!(s.call("production", &json!({"do": "line", "account": "9999", "desc": "x", "rate": 1, "currency": "ZZZ"})).is_err());
        let added = prod(&mut s, json!({"do": "line", "account": "2100", "desc": "Kite", "qty": 2, "rate": 50, "currency": "USD", "element": "lantern"}));
        assert!(added["line"]["element"].as_str().is_some() || added["line"]["budget"].as_f64().unwrap() > 0.0, "{added}");

        // Pipeline and tasks.
        let p = prod(&mut s, json!({"do": "pipeline", "preset": "animation", "tasks": true, "artists": {"animation": 2}}));
        assert!(p["tasks_made"].as_u64().unwrap() >= 5, "{p}");
        assert!(p["total_days"].as_f64().unwrap() > 0.0);
        let shot = story_of(&s.doc.model).scenes[0].shots[0].id.clone();
        let key = format!("{shot} layout");
        let st = prod(&mut s, json!({"do": "status", "task": key, "status": "in_progress", "who": "claude", "note": "blocking"}));
        assert_eq!(st["task"]["assignee"], "claude");
        let st = prod(&mut s, json!({"do": "status", "task": key, "status": "approved"}));
        assert!(st["now_ready"].as_array().unwrap().iter().any(|t| t.as_str().unwrap().contains("animation")), "{st}");
        assert!(s.call("production", &json!({"do": "status", "task": "nope", "status": "final"})).is_err());
        assert!(s.call("production", &json!({"do": "status", "task": key, "status": "sideways"})).is_err());
        let v = prod(&mut s, json!({"do": "version", "task": key, "note": "v1"}));
        assert_eq!(v["version"], 1);
        let list = prod(&mut s, json!({"do": "tasks", "status": "ready"}));
        assert!(list["count"].as_u64().unwrap() >= 1);
        let d = prod(&mut s, json!({"do": "dashboard"}));
        assert!(d["percent"].as_f64().unwrap() > 0.0 && d["dashboard"].as_str().unwrap().contains(&shot), "{d}");

        // Every paper as a PDF.
        for what in ["breakdown", "schedule", "budget", "pipeline", "progress"] {
            let e = prod(&mut s, json!({"do": "export", "what": what}));
            let path = e["written"].as_str().unwrap();
            assert!(path.contains("/production/"), "{path}");
            assert!(std::fs::read(path).unwrap().starts_with(b"%PDF"), "{what}");
        }
        let all = prod(&mut s, json!({"do": "export", "what": "all_call_sheets"}));
        assert!(all["written"].as_array().unwrap().len() >= 2);
        assert!(s.call("production", &json!({"do": "export", "what": "poem"})).is_err());
        assert!(s.call("production", &json!({"do": "dance"})).is_err());

        // Kept beside the project, and read back by a new session.
        assert!(dir.join("production.json").exists());
        let mut s2 = Session::at(&dir).unwrap();
        let again = prod(&mut s2, json!({}));
        assert!(again["overview"].as_str().unwrap().contains("tasks"), "{again}");
        assert!(prod(&mut s2, json!({"do": "tasks", "assignee": "claude"}))["count"].as_u64().unwrap() >= 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn generation_spend_lands_on_the_shot_and_the_budget() {
        let dir = scratch("spend");
        let mut s = with_story(&dir);
        prod(&mut s, json!({"do": "budget", "template": "ai_film"}));
        let shot = story_of(&s.doc.model).scenes[0].shots[0].id.clone();
        let spend = Spend { provider: "openai".into(), model: "video-model".into(), kind: "video".into(), cents: 250, shot: Some(shot.clone()), prompt: "kite".into(), ..Default::default() };
        s.production_record_generation(&json!({"shot": shot, "who": "claude"}), spend, &["a.mp4".into()]);
        let t = prod(&mut s, json!({"do": "tasks", "shot": shot, "step": "generate"}));
        assert_eq!(t["tasks"][0]["status"], "review", "{t}");
        assert_eq!(t["tasks"][0]["version"], 1);
        let sp = prod(&mut s, json!({"do": "spend"}));
        assert_eq!(sp["total_usd"], 2.5);
        assert_eq!(sp["recent"][0]["shot"], shot.as_str());
        let b = prod(&mut s, json!({"do": "budget", "view": "scenes"}));
        assert_eq!(b["total"]["ai_spend"], 2.5, "{b}");
        let manual = prod(&mut s, json!({"do": "spend", "dollars": 4, "model": "midjourney", "note": "outside Studio"}));
        assert!(manual["recorded"].as_u64().is_some());
        assert_eq!(prod(&mut s, json!({"do": "spend"}))["total_usd"], 6.5);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_project_not_yet_saved_keeps_its_office_in_the_session() {
        let mut s = Session::new(services::project::new_custom("Scratch", 640, 360, 30.0));
        give_story(&mut s);
        prod(&mut s, json!({"do": "breakdown"}));
        prod(&mut s, json!({"do": "schedule", "auto": true}));
        let e = prod(&mut s, json!({"do": "elements"}));
        assert!(!e["elements"].as_array().unwrap().is_empty());
        assert!(s.production.as_ref().is_some_and(|p| !p.schedule.days.is_empty()));
        let t = s.call("production", &json!({"do": "export", "what": "budget", "path": std::env::temp_dir().join("studio-production-unsaved").display().to_string()}));
        assert!(t.is_ok(), "an unsaved project can still export its papers");
        let _ = std::fs::remove_dir_all(std::env::temp_dir().join("studio-production-unsaved"));
    }

    #[test]
    fn every_argument_the_tool_reads_is_in_its_schema() {
        let schema = schema();
        let props = schema["properties"].as_object().unwrap();
        for k in ["do", "who", "director", "producer", "hospital", "office", "shoot_hours", "hours_per_screen_minute", "general_call", "max_locations_per_day", "animated", "start_date", "days_off", "blackout", "scene", "shot", "day", "category", "name", "qty", "note", "retag", "auto", "move", "to_day", "index", "set_day", "call", "notes", "cast", "dates", "reschedule", "preset", "artists", "tasks", "template", "scale", "currency", "rates", "fringe_pct", "contingency_pct", "view", "code", "section", "account", "desc", "unit", "rate", "labour", "element", "feed", "id", "remove", "line", "amount", "close", "cents", "dollars", "model", "task", "status", "step", "assignee", "due", "entity", "target", "media", "what", "pdf", "path", "limit"] {
            assert!(props.contains_key(k), "{k} is read by the tool but the schema refuses it");
        }
    }

    #[test]
    fn no_story_means_a_helpful_refusal() {
        let mut s = Session::new(services::project::new_custom("Empty", 640, 360, 30.0));
        let e = s.call("production", &json!({"do": "breakdown"})).err().expect("nothing to break down");
        assert!(e.contains("story tool"), "{e}");
        assert!(prod(&mut s, json!({}))["next"].as_str().unwrap().contains("story"));
    }
}
