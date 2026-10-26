// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The AI editor as a production crew.
//!
//! A director reads the brief, the story and the person's taste, and posts
//! a plan with the `plan` tool: steps, each for one specialist (editor,
//! animator, sound designer, colourist, VFX artist), with the clips it
//! owns, what it waits for and whether the person approves it before the
//! rest builds on it. The crew works the steps in parallel, each specialist
//! a brain of its own with its own ops and craft book chapters; the
//! [`Board`] keeps them apart (a clip one specialist holds cannot be edited
//! by another) and shows the plan to the person as it moves. When every
//! step is done the director reviews the whole film and approves it or
//! sends steps back with notes.
//!
//! The board also carries what the person says while a request runs: a
//! correction reaches every agent at its next tool call, Pause holds every
//! call until Resume, and checkpoints wait for approval. A solo agent uses
//! the same `plan` tool, so its work reads as a plan with checkpoints too.

use super::brain::{AgentEvent, Answer, Brain, Gate};
use serde_json::{Value, json};
use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, mpsc};
use std::time::{Duration, Instant};

// ---------------------------------------------------------------- roles

/// A member of the crew.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Role {
    Director,
    Editor,
    Animator,
    Sound,
    Colour,
    Vfx,
}

/// Ops every specialist may use on the clips it holds.
const COMMON_OPS: &[&str] = &["set", "marker", "lane", "repeat", "keys", "keyframe", "group", "ungroup", "restack", "delete", "move", "trim", "duplicate"];

impl Role {
    pub const CREW: [Role; 5] = [Role::Editor, Role::Animator, Role::Sound, Role::Colour, Role::Vfx];

    /// The word agents and the proxy use.
    pub fn name(self) -> &'static str {
        match self {
            Role::Director => "director",
            Role::Editor => "editor",
            Role::Animator => "animator",
            Role::Sound => "sound",
            Role::Colour => "colour",
            Role::Vfx => "vfx",
        }
    }

    /// For the person.
    pub fn label(self) -> &'static str {
        match self {
            Role::Director => "Director",
            Role::Editor => "Editor",
            Role::Animator => "Animator",
            Role::Sound => "Sound designer",
            Role::Colour => "Colourist",
            Role::Vfx => "VFX artist",
        }
    }

    pub fn parse(s: &str) -> Option<Role> {
        Some(match s.trim().to_lowercase().replace(['-', '_'], " ").as_str() {
            "director" | "lead" => Role::Director,
            "editor" | "edit" | "cutter" | "editing" => Role::Editor,
            "animator" | "animation" | "motion" | "motion designer" => Role::Animator,
            "sound" | "audio" | "sound designer" | "mixer" | "music" => Role::Sound,
            "colour" | "color" | "colourist" | "colorist" | "grade" | "grading" => Role::Colour,
            "vfx" | "effects" | "vfx artist" | "compositor" | "compositing" => Role::Vfx,
            _ => return None,
        })
    }

    /// What this role owns, in a few sentences.
    fn craft(self) -> &'static str {
        match self {
            Role::Director => "You hold the whole film: the story, the rhythm, the look. You plan, cast the work to your crew and judge the result; you do not edit yourself.",
            Role::Editor => "You own structure and rhythm: which shots, in what order, how long, where the cuts land (on the beat, on action), titles and cards that carry the story, transitions only where they mean something.",
            Role::Animator => "You own everything that moves: characters that act (puppet rigs, motion presets, keys with arcs and easing, follow-through), cameras and 3D layers, staging in space. Characters are animated, never still panels: if a move seems missing, call `capabilities` before choosing anything else.",
            Role::Sound => "You own what is heard: music fitted to the cut, effects on every action, ambience beds for every place, voice clear above everything, fades and ducking, loudness that holds.",
            Role::Colour => "You own the look: exposure, white balance, matching shots to each other, a grade that serves the mood, skin that stays skin, nothing clipped or crushed by accident.",
            Role::Vfx => "You own compositing: keys, cut-outs and roto, tracking and stabilising, masks and mattes, effect layers, particles and anything added to the picture that was not filmed.",
        }
    }

    /// Craft book chapters the role works from (topic words).
    fn chapters(self) -> &'static [&'static str] {
        match self {
            Role::Director => &["staging", "review", "overdoing"],
            Role::Editor => &["cuts", "time", "typography", "placement", "fades"],
            Role::Animator => &["keyframes", "puppetry", "staging", "3d", "modifiers", "physics"],
            Role::Sound => &["sound-design", "sound-editing"],
            Role::Colour => &["colour", "light", "atmosphere", "quality"],
            Role::Vfx => &["keying", "opacity", "shadows", "3d"],
        }
    }

    /// Edit ops the role may use (besides [`COMMON_OPS`]); the director
    /// edits nothing.
    fn ops(self) -> &'static [&'static str] {
        match self {
            Role::Director => &[],
            Role::Editor => &["montage", "place", "add", "split", "reorder", "roll", "slide", "slip", "freeze", "replace", "detach_audio", "link", "unlink", "transition", "text", "card", "reframe", "scene", "narrate", "music", "merge", "unmerge", "align", "distribute", "fx_layer", "motion"],
            Role::Animator => &["animate", "motion", "use_move", "stagger", "modifier", "physics", "driver", "puppet", "lip_sync", "scissor", "mask_puppet", "three_d", "camera", "light", "atmosphere", "model", "mesh", "parallax", "parent", "align", "distribute", "place", "add", "text", "card", "scene", "use_design", "merge", "unmerge", "flatten", "unflatten", "corner_pin", "freeze"],
            Role::Sound => &["music", "sfx", "sing", "narrate", "audio", "place", "add", "detach_audio", "split", "link", "unlink"],
            Role::Colour => &["effect", "apply", "copy_look", "match_colour", "fx_layer"],
            Role::Vfx => &["effect", "apply", "fx_layer", "mask", "matte", "cut_out", "roto", "track", "stabilize", "corner_pin", "smooth_motion", "flatten", "unflatten", "card", "add", "place", "three_d", "camera", "light", "atmosphere", "parallax", "physics", "modifier", "driver", "animate", "merge", "unmerge", "parent"],
        }
    }

    /// Its own ops, the common ones, and any op no role claims (ops
    /// added later are open to all until a role owns them).
    pub fn may_use(self, op: &str) -> bool {
        self != Role::Director && (COMMON_OPS.contains(&op) || self.ops().contains(&op) || Role::owner_of(op).is_none())
    }

    /// The crew member who owns `op`, for a refusal that says who to ask.
    fn owner_of(op: &str) -> Option<Role> {
        Role::CREW.into_iter().find(|r| r.ops().contains(&op))
    }

    /// The role's brief: who it is, what it owns, its chapters of the book
    /// (so it need not call `guide` for them).
    pub fn brief(self) -> String {
        let mut chapters = String::new();
        let mut taught: Vec<String> = Vec::new();
        for c in self.chapters() {
            if let Some(t) = super::craft_book::topic(c).or_else(|| super::skills::brief_chapter(c, &mut taught)) {
                if chapters.len() + t.len() > 26_000 {
                    break;
                }
                chapters.push_str(&t);
                chapters.push_str("\n\n");
            }
        }
        let ops: Vec<&str> = COMMON_OPS.iter().chain(self.ops()).copied().collect();
        let job = if self == Role::Director {
            DIRECTOR_BRIEF.to_owned()
        } else {
            format!(
                "You are the {} on Studio's production crew. {}\n\
                 The director gives you one part of the film. Do that part fully, nothing else: other crew members work on other parts at the same time. Studio refuses an edit to a clip another crew member holds; work on your own clips (new ones you make are yours). Your `edit` ops: {}. Undo is not yours to use (it would take back the others' work too): fix with `edit`.\n\
                 Look before and after you change things (`see`, `frame`, `contact_sheet`, `listen`). Use `plan` with `note` for a short progress line the person sees under your step. Never decide something cannot be done before `capabilities` says so.\n\
                 When your part is done and checked, answer in two or three sentences for the director: what you made (clip ids, times) and anything it should know.",
                self.label().to_lowercase(),
                self.craft(),
                ops.join(", ")
            )
        };
        format!("{job}\n\n# Your chapters of the craft book and skills (more: `skill {{name}}`)\n\n{chapters}")
    }
}

const DIRECTOR_BRIEF: &str = "You are the director of Studio's production crew: an editor, an animator, a sound designer, a colourist and a VFX artist, each an AI with its own tools, working in parallel on the project the person sees.\n\
First understand the job: the request, `notes`, `summary` or `script`, the footage (`moments`, `contact_sheet`), the story structure (guide topic `staging`), and the person's taste and style guide (given with the request; `taste` reads them all). Never plan around something you believe Studio cannot do: call `capabilities` first; Studio animates characters, builds 3D, scores, grades and composites.\n\
Then call `plan` once with the steps: `{\"goal\": \"…\", \"steps\": [{\"role\": \"editor|animator|sound|colour|vfx\", \"title\": \"a few words\", \"task\": \"everything the specialist needs, self-contained: what, where (seconds), which clips, the look and feel, what good looks like\", \"clips\": [ids it owns, if they exist], \"after\": [ids of steps it needs first], \"checkpoint\": true when the person should approve it before the rest builds on it}]}`. Steps are numbered from 1 in order. Steps with nothing in `after` start at once, side by side, so give parts that do not overlap (different clips, different stretches, different lanes). A small request is one step. You do not edit: your crew does.\n\
When the crew is done you are asked to review: watch the whole film (`review`, `see`, `contact_sheet`, `listen`, `check`), then call `plan` with `{\"approve\": true, \"summary\": \"…\"}` or `{\"redo\": [{\"step\": n, \"note\": \"what to change\"}]}`. Then answer the person in a short plain paragraph: what the film is now.";

// ---------------------------------------------------------------- the plan

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Pending,
    Running,
    /// Done, waiting for the person to approve it (a checkpoint).
    Waiting,
    Done,
    /// The director or the person sent it back; it runs again.
    SentBack,
    Failed,
    /// Running or about to when the person pressed Stop.
    Stopped,
}

/// Why a plan is not moving: the person's Pause or Stop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Halt {
    Paused,
    Stopped,
}

#[derive(Clone, Debug)]
pub struct Step {
    pub id: usize,
    /// `None`: done by the agent that wrote the plan (a solo plan).
    pub role: Option<Role>,
    pub title: String,
    pub task: String,
    pub clips: Vec<u32>,
    pub after: Vec<usize>,
    pub checkpoint: bool,
    pub status: Status,
    /// Progress lines, the specialist's answer, the director's notes.
    pub notes: Vec<String>,
    pub started: Option<Instant>,
    /// Seconds it took (all its runs).
    pub took: f32,
    pub tokens: u64,
}

impl Step {
    pub fn last_note(&self) -> Option<&str> {
        self.notes.iter().rev().map(String::as_str).find(|n| !n.trim().is_empty())
    }
}

/// How long and how many tokens a plan should take.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Estimate {
    pub minutes: f32,
    pub tokens: u64,
}

#[derive(Clone, Debug, Default)]
pub struct Plan {
    pub goal: String,
    pub steps: Vec<Step>,
    /// Run by a crew (steps go to specialists), not one agent.
    pub crew: bool,
    pub estimate: Option<Estimate>,
    pub started: Option<Instant>,
    /// Review rounds done.
    pub round: u32,
    /// The director's verdict once approved.
    pub verdict: Option<String>,
    /// Paused or stopped by the person: the card shows it and no step
    /// spins, and no time is left to wait.
    pub halted: Option<Halt>,
}

impl Plan {
    pub fn done(&self) -> usize {
        self.steps.iter().filter(|s| s.status == Status::Done).count()
    }

    /// Minutes left by the estimate, scaled by how much is done.
    pub fn minutes_left(&self) -> Option<f32> {
        if self.halted.is_some() {
            return None;
        }
        let e = self.estimate?;
        let spent = self.started.map_or(0.0, |s| s.elapsed().as_secs_f32() / 60.0);
        let n = self.steps.len().max(1) as f32;
        let left_share = 1.0 - self.done() as f32 / n;
        Some((e.minutes * left_share).max(e.minutes - spent).max(0.0))
    }

    /// The plan in a few lines, for agents.
    pub fn text(&self) -> String {
        if self.steps.is_empty() {
            return "No plan yet.".into();
        }
        let mut out = vec![format!("Plan: {} ({} of {} done)", if self.goal.is_empty() { "the request" } else { &self.goal }, self.done(), self.steps.len())];
        for s in &self.steps {
            let who = s.role.map_or(String::new(), |r| format!(" [{}]", r.name()));
            let after = if s.after.is_empty() { String::new() } else { format!(" after {:?}", s.after) };
            let note = s.last_note().map_or(String::new(), |n| format!(" — {}", super::brain::one_line(n, 160)));
            out.push(format!("{}. {:?}{who} {}{after}{note}", s.id, s.status, s.title));
        }
        out.join("\n")
    }
}

/// What the director decided at review.
#[derive(Clone, Debug, PartialEq)]
enum Verdict {
    Approve(String),
    Redo(Vec<(usize, String)>),
}

// ---------------------------------------------------------------- the board

thread_local! {
    /// The specialist this thread works as (its step), if any.
    static ACTING: Cell<Option<(Role, usize)>> = const { Cell::new(None) };
}

/// The specialist the current thread is, and its step.
pub fn acting() -> Option<(Role, usize)> {
    ACTING.with(Cell::get)
}

/// Shared state for one request: the plan, who holds which clips, what
/// the person said mid-run, pause.
#[derive(Default)]
pub struct Board {
    /// Every tool call waits while paused.
    pub paused: AtomicBool,
    /// Checkpoint steps wait for the person's approval.
    pub approve_checkpoints: AtomicBool,
    s: Mutex<State>,
}

#[derive(Default)]
struct State {
    plan: Plan,
    /// Clip id → the specialist holding it and its step.
    claims: HashMap<u32, (Role, usize)>,
    corrections: Vec<String>,
    /// Corrections each caller has been told.
    seen: HashMap<String, usize>,
    /// What each caller said this request (for the honesty check).
    said: HashMap<String, String>,
    /// Callers that looked up `capabilities`.
    checked: HashSet<String>,
    /// (caller, topic) guide reads this request.
    guide: HashSet<(String, String)>,
    verdict: Option<Verdict>,
    /// Stop was pressed: nothing starts again until the next request.
    stopped: bool,
}

/// The lead agent (solo or director) as a caller.
pub const LEAD: &str = "lead";

impl Board {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.s.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A new request: everything but pause starts again.
    pub fn begin(&self) {
        *self.lock() = State::default();
        self.paused.store(false, Ordering::SeqCst);
    }

    pub fn snapshot(&self) -> Plan {
        let s = self.lock();
        let mut plan = s.plan.clone();
        plan.halted = if s.stopped {
            Some(Halt::Stopped)
        } else if self.paused.load(Ordering::SeqCst) {
            Some(Halt::Paused)
        } else {
            None
        };
        plan
    }

    /// Stop: every step that was running ends as Stopped (its time is
    /// kept), and none starts again.
    pub fn stop(&self) {
        let mut s = self.lock();
        s.stopped = true;
        let now = Instant::now();
        for st in &mut s.plan.steps {
            if matches!(st.status, Status::Running | Status::SentBack) {
                if let Some(t) = st.started.take() {
                    st.took += now.saturating_duration_since(t).as_secs_f32();
                }
                st.status = Status::Stopped;
            }
        }
        s.claims.clear();
    }

    #[cfg(test)]
    pub fn set_status_for_test(&self, id: usize, status: Status) {
        self.set_status(id, status);
    }

    pub fn is_stopped(&self) -> bool {
        self.lock().stopped
    }

    pub fn has_plan(&self) -> bool {
        !self.lock().plan.steps.is_empty()
    }

    /// Who is calling: the lead, or a specialist on its step.
    pub fn caller(&self, role: Option<Role>) -> String {
        match role {
            None | Some(Role::Director) => LEAD.into(),
            Some(r) => match self.running_step(r) {
                Some(id) => format!("{}:{id}", r.name()),
                None => r.name().into(),
            },
        }
    }

    pub fn running_step(&self, role: Role) -> Option<usize> {
        if let Some((r, id)) = acting() {
            if r == role {
                return Some(id);
            }
        }
        self.lock().plan.steps.iter().find(|s| s.role == Some(role) && s.status == Status::Running).map(|s| s.id)
    }

    // ------------------------------------------------ steering

    /// The person corrects the running request.
    pub fn steer(&self, text: &str) {
        self.lock().corrections.push(text.trim().to_owned());
    }

    pub fn corrections(&self) -> Vec<String> {
        self.lock().corrections.clone()
    }

    /// Corrections `caller` has not been told yet (and now has).
    pub fn take_corrections(&self, caller: &str) -> Option<String> {
        let mut s = self.lock();
        let n = s.corrections.len();
        let seen = s.seen.get(caller).copied().unwrap_or(0);
        if seen >= n {
            return None;
        }
        let new = s.corrections[seen..].join("\n");
        s.seen.insert(caller.to_owned(), n);
        Some(new)
    }

    /// A caller starting late has nothing to be told twice: its prompt
    /// already carries the corrections so far.
    fn mark_told(&self, caller: &str) {
        let mut s = self.lock();
        let n = s.corrections.len();
        s.seen.insert(caller.to_owned(), n);
    }

    // ------------------------------------------------ honesty and guide

    pub fn note_said(&self, caller: &str, text: &str) {
        let mut s = self.lock();
        let said = s.said.entry(caller.to_owned()).or_default();
        if said.len() < 40_000 {
            said.push_str(text);
        }
    }

    pub fn said(&self, caller: &str) -> String {
        self.lock().said.get(caller).cloned().unwrap_or_default()
    }

    fn clear_said(&self, caller: &str) {
        self.lock().said.remove(caller);
    }

    pub fn mark_checked(&self, caller: &str) {
        self.lock().checked.insert(caller.to_owned());
    }

    pub fn checked(&self, caller: &str) -> bool {
        self.lock().checked.contains(caller)
    }

    /// True when `caller` already read guide `topic` in this request.
    pub fn guide_read_before(&self, caller: &str, topic: &str) -> bool {
        !self.lock().guide.insert((caller.to_owned(), topic.trim().to_lowercase()))
    }

    // ------------------------------------------------ steps

    fn step_mut<'a>(s: &'a mut State, id: usize) -> Option<&'a mut Step> {
        s.plan.steps.iter_mut().find(|st| st.id == id)
    }

    pub fn note(&self, id: usize, text: &str) {
        let t = text.trim();
        if t.is_empty() {
            return;
        }
        let mut s = self.lock();
        if let Some(st) = Self::step_mut(&mut s, id) {
            st.notes.push(t.to_owned());
        }
    }

    pub fn add_tokens(&self, id: usize, n: u64) {
        let mut s = self.lock();
        if let Some(st) = Self::step_mut(&mut s, id) {
            st.tokens += n;
        }
    }

    fn set_status(&self, id: usize, status: Status) {
        let mut s = self.lock();
        // After Stop nothing changes a step again (a specialist that is
        // still winding down must not turn its step back to Done).
        if s.stopped {
            return;
        }
        if let Some(st) = Self::step_mut(&mut s, id) {
            match status {
                Status::Running => {
                    st.started = Some(Instant::now());
                }
                Status::Done | Status::Waiting | Status::Failed | Status::SentBack if st.status == Status::Running => {
                    st.took += st.started.map_or(0.0, |t| t.elapsed().as_secs_f32());
                }
                _ => {}
            }
            st.status = status;
        }
        if status != Status::Running {
            s.claims.retain(|_, (_, step)| *step != id);
        }
    }

    /// The `plan` tool. `role` is who called. Returns the answer and, for
    /// a solo checkpoint just finished, the step to ask the person about.
    pub fn plan_tool(&self, role: Option<Role>, args: &Value) -> Result<(String, Option<usize>), String> {
        let mut ask = None;
        if let Some(steps) = args.get("steps").and_then(Value::as_array) {
            if matches!(role, Some(r) if r != Role::Director) {
                return Err("Only the director plans; say your progress with `plan` and `note`.".into());
            }
            let crew = role == Some(Role::Director);
            let mut list = Vec::new();
            for (i, v) in steps.iter().enumerate() {
                let id = i + 1;
                let step_role = match v.get("role").and_then(Value::as_str) {
                    Some(r) if crew => Some(Role::parse(r).filter(|r| *r != Role::Director).ok_or(format!("step {id}: \"{r}\" is not on the crew (editor, animator, sound, colour, vfx)"))?),
                    None if crew => return Err(format!("step {id} needs a role: editor, animator, sound, colour or vfx")),
                    _ => None,
                };
                let title = v.get("title").and_then(Value::as_str).or_else(|| v.as_str()).unwrap_or("").trim().to_owned();
                let task = v.get("task").and_then(Value::as_str).unwrap_or(&title).trim().to_owned();
                if title.is_empty() && task.is_empty() {
                    return Err(format!("step {id} needs a title"));
                }
                let after: Vec<usize> = v.get("after").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_u64).map(|n| n as usize).collect();
                if let Some(bad) = after.iter().find(|a| **a >= id || **a == 0) {
                    return Err(format!("step {id} waits for step {bad}, which does not come before it"));
                }
                list.push(Step {
                    id,
                    role: step_role,
                    title: if title.is_empty() { super::brain::one_line(&task, 48) } else { title },
                    task,
                    clips: v.get("clips").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_u64).map(|n| n as u32).collect(),
                    after,
                    checkpoint: v.get("checkpoint").and_then(Value::as_bool).unwrap_or(false),
                    status: Status::Pending,
                    notes: Vec::new(),
                    started: None,
                    took: 0.0,
                    tokens: 0,
                });
            }
            if list.is_empty() {
                return Err("a plan needs at least one step".into());
            }
            let mut s = self.lock();
            s.plan = Plan { goal: args.get("goal").and_then(Value::as_str).unwrap_or("").to_owned(), estimate: Some(estimate(&list, crew)), steps: list, crew, started: Some(Instant::now()), round: 0, verdict: None, halted: None };
            let e = s.plan.estimate.expect("estimate");
            let answer = if crew {
                format!("Plan posted: {} steps for the crew, about {:.0} min. They start when you end this turn; answer the person in one line meanwhile.", s.plan.steps.len(), e.minutes.max(1.0))
            } else {
                format!("Plan posted: {} steps. Mark each with `plan` `start` and `done` as you go.", s.plan.steps.len())
            };
            return Ok((answer, None));
        }
        if args.get("approve").and_then(Value::as_bool) == Some(true) {
            let summary = args.get("summary").and_then(Value::as_str).unwrap_or("").to_owned();
            let mut s = self.lock();
            s.plan.verdict = Some(summary.clone());
            s.verdict = Some(Verdict::Approve(summary));
            return Ok(("Approved. Now answer the person.".into(), None));
        }
        if let Some(redo) = args.get("redo").and_then(Value::as_array) {
            let mut list = Vec::new();
            for r in redo {
                let id = r.get("step").and_then(Value::as_u64).ok_or("each redo needs a step number")? as usize;
                let note = r.get("note").and_then(Value::as_str).unwrap_or("").to_owned();
                list.push((id, note));
            }
            let mut s = self.lock();
            for (id, _) in &list {
                if !s.plan.steps.iter().any(|st| st.id == *id) {
                    return Err(format!("there is no step {id}"));
                }
            }
            s.verdict = Some(Verdict::Redo(list.clone()));
            return Ok((format!("Sent back {} step(s); the crew starts on them when you end this turn.", list.len()), None));
        }
        let id = |k: &str| args.get(k).and_then(Value::as_u64).map(|n| n as usize);
        let note = args.get("note").and_then(Value::as_str).unwrap_or("");
        if let Some(id) = id("start") {
            self.set_status(id, Status::Running);
            self.note(id, note);
        } else if let Some(id) = id("done") {
            let checkpoint = self.lock().plan.steps.iter().find(|s| s.id == id).map(|s| s.checkpoint);
            let Some(checkpoint) = checkpoint else { return Err(format!("there is no step {id}")) };
            self.note(id, note);
            if checkpoint && self.approve_checkpoints.load(Ordering::SeqCst) {
                self.set_status(id, Status::Waiting);
                ask = Some(id);
            } else {
                self.set_status(id, Status::Done);
            }
        } else if !note.is_empty() {
            let target = match role {
                Some(r) if r != Role::Director => self.running_step(r),
                _ => self.lock().plan.steps.iter().find(|s| s.status == Status::Running).map(|s| s.id),
            };
            match target {
                Some(id) => self.note(id, note),
                None => return Err("no step is running to note against; `start` one first".into()),
            }
        }
        Ok((self.snapshot().text(), ask))
    }

    /// The person's answer to a checkpoint.
    pub fn checkpoint_answer(&self, id: usize, approved: bool) {
        if approved {
            self.set_status(id, Status::Done);
        } else {
            self.note(id, "The person sent this back.");
            self.set_status(id, Status::SentBack);
        }
    }

    // ------------------------------------------------ clips

    /// May `role` run this `edit`? Checks its ops and the clips others hold.
    pub fn check_edit(&self, role: Role, args: &Value) -> Result<(), String> {
        let mut ops = Vec::new();
        collect_ops(args.get("ops").unwrap_or(&Value::Null), &mut ops);
        if role == Role::Director {
            return Err("You direct: give this work to your crew in the plan (or send a step back at review) instead of editing.".into());
        }
        for (op, _) in &ops {
            if !role.may_use(op) {
                let who = Role::owner_of(op).map_or("another crew member".to_owned(), |r| format!("the {}", r.label().to_lowercase()));
                return Err(format!("`{op}` is {who}'s work, not the {}'s. Do your part with your own ops, and say in your answer what the {who} should do.", role.label().to_lowercase()));
            }
        }
        let mine = self.running_step(role);
        let s = self.lock();
        for (_, clips) in &ops {
            for c in clips {
                if let Some((other, step)) = s.claims.get(c) {
                    if *other != role && Some(*step) != mine {
                        return Err(format!("Clip {c} is being worked on by the {} (step {step}). Leave it to them: work on your own clips, or make new ones.", other.label().to_lowercase()));
                    }
                }
            }
        }
        Ok(())
    }

    /// Clips `role` touched in an `edit` are its own until its step ends.
    pub fn claim(&self, role: Role, args: &Value, result: &str) {
        let Some(step) = self.running_step(role) else { return };
        let mut ids = Vec::new();
        let mut ops = Vec::new();
        collect_ops(args.get("ops").unwrap_or(&Value::Null), &mut ops);
        for (_, c) in ops {
            ids.extend(c);
        }
        if let Ok(v) = serde_json::from_str::<Value>(result) {
            for k in ["added", "changed"] {
                ids.extend(v["changed"][k].as_array().into_iter().flatten().filter_map(|c| c["id"].as_u64()).map(|n| n as u32));
            }
        }
        let mut s = self.lock();
        for id in ids {
            s.claims.entry(id).or_insert((role, step));
        }
    }

    fn claim_step_clips(&self, id: usize) {
        let mut s = self.lock();
        let Some(st) = s.plan.steps.iter().find(|st| st.id == id) else { return };
        let (Some(role), clips) = (st.role, st.clips.clone()) else { return };
        for c in clips {
            s.claims.insert(c, (role, id));
        }
    }

    // ------------------------------------------------ scheduling

    /// Steps that can start now: pending or sent back, everything they
    /// wait for done, their specialist free, under `max` running.
    fn ready(&self, max: usize) -> Vec<usize> {
        let s = self.lock();
        if s.stopped {
            return Vec::new();
        }
        let steps = &s.plan.steps;
        let mut busy: HashSet<Role> = steps.iter().filter(|st| st.status == Status::Running).filter_map(|st| st.role).collect();
        let mut running = steps.iter().filter(|st| st.status == Status::Running).count();
        let mut out = Vec::new();
        for st in steps {
            if running >= max {
                break;
            }
            let Some(role) = st.role else { continue };
            if !matches!(st.status, Status::Pending | Status::SentBack) || busy.contains(&role) {
                continue;
            }
            let waits = st.after.iter().any(|a| steps.iter().find(|x| x.id == *a).is_none_or(|x| x.status != Status::Done));
            if waits {
                continue;
            }
            busy.insert(role);
            running += 1;
            out.push(st.id);
        }
        out
    }

    fn all_settled(&self) -> bool {
        self.lock().plan.steps.iter().all(|s| matches!(s.status, Status::Done | Status::Failed | Status::Stopped))
    }

    /// Nothing can move: every step left waits on one that failed.
    fn stuck(&self) -> bool {
        let s = self.lock();
        let steps = &s.plan.steps;
        let open: Vec<&Step> = steps.iter().filter(|st| !matches!(st.status, Status::Done | Status::Failed)).collect();
        !open.is_empty() && open.iter().all(|st| st.status != Status::Running && st.status != Status::Waiting && st.after.iter().any(|a| steps.iter().any(|x| x.id == *a && x.status == Status::Failed)))
    }

    fn take_verdict(&self) -> Option<Verdict> {
        self.lock().verdict.take()
    }

    fn step(&self, id: usize) -> Option<Step> {
        self.lock().plan.steps.iter().find(|s| s.id == id).cloned()
    }

    fn bump_round(&self) {
        self.lock().plan.round += 1;
    }
}

/// Every op in an `ops` list (into `repeat`s too) with the clip ids it
/// works on (`clip` / `clips` numbers).
fn collect_ops(ops: &Value, out: &mut Vec<(String, Vec<u32>)>) {
    let list: Vec<&Value> = match ops {
        Value::Array(a) => a.iter().collect(),
        v @ Value::Object(_) => vec![v],
        _ => Vec::new(),
    };
    for op in list {
        let name = op.get("op").and_then(Value::as_str).unwrap_or("").to_owned();
        let mut clips: Vec<u32> = Vec::new();
        if let Some(c) = op.get("clip").and_then(Value::as_u64) {
            clips.push(c as u32);
        }
        clips.extend(op.get("clips").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_u64).map(|n| n as u32));
        if name == "repeat" {
            collect_ops(op.get("do").unwrap_or(&Value::Null), out);
        }
        if !name.is_empty() {
            out.push((name, clips));
        }
    }
}

// ---------------------------------------------------------------- tools

/// The crew's tools, listed with the others.
pub fn tools() -> Vec<Value> {
    let t = |name: &str, desc: &str, props: Value| json!({"name": name, "description": desc, "inputSchema": {"type": "object", "properties": props, "required": []}});
    let s = |d: &str| json!({"type": "string", "description": d});
    let b = |d: &str| json!({"type": "boolean", "description": d});
    let n = |d: &str| json!({"type": "integer", "description": d});
    vec![
        t("plan", "The plan the person follows while you work: steps with checkpoints. Post it once for a job of several parts: {steps: [{title, checkpoint}]}; then {start: n}, {done: n, note} or {note}. A checkpoint waits for approval. No arguments: the plan as it stands.", json!({
            "goal": s("the job in a few words"),
            "steps": {"type": "array", "items": {"type": "object", "properties": {"title": s("a few words"), "role": s("crew only: editor, animator, sound, colour or vfx"), "task": s("crew only: everything the specialist needs"), "clips": {"type": "array", "items": {"type": "integer"}}, "after": {"type": "array", "items": {"type": "integer"}, "description": "steps (numbered from 1) this one waits for"}, "checkpoint": b("the person approves it before the rest builds on it")}}},
            "start": n("a step you begin"), "done": n("a step you finished"), "note": s("a short progress line for the person"),
            "approve": b("director at review: the film is right"), "summary": s("with approve: what the film is now"),
            "redo": {"type": "array", "items": {"type": "object", "properties": {"step": {"type": "integer"}, "note": {"type": "string"}}}, "description": "director at review: steps to send back with what to change"},
        })),
        t("taste", "What the person likes, remembered across requests and projects (likes, dislikes, rules, corrections, the project's style guide). No arguments reads it. Fields: remember, kind (like, dislike, rule), everywhere, forget (a note id), style_guide, append.", json!({
            "remember": s("the preference, in their words where you can"), "kind": s("like, dislike or rule"), "everywhere": b("for every project, not just this one"),
            "forget": n("a note id"), "style_guide": s("the project's style guide (markdown): look, type, colour, pace, sound, voice"), "append": b("add to the style guide instead of replacing it"),
        })),
        t("capabilities", "Can Studio do this, and with what? Say what you want (\"make the fox wave\", \"fix a blue colour cast\", \"cut on the beat\") and get the tools, edit ops, guide sections and craft book chapters that do it, best first. Call it before telling the person something cannot be done, and before any workaround (a still picture for a moving character).", json!({"query": s("what you want to do, in plain words")})),
    ]
}

/// The `plan` tool outside the window (`studio mcp`, `studio agent`): the
/// plan is kept for the process and read back, with no one to approve.
pub fn headless_plan(args: &Value) -> Result<String, String> {
    static BOARD: std::sync::OnceLock<Board> = std::sync::OnceLock::new();
    let role = args.get("as").and_then(Value::as_str).and_then(Role::parse);
    BOARD.get_or_init(Board::default).plan_tool(role, args).map(|(t, _)| t)
}

// ---------------------------------------------------------------- estimates

/// Minutes and tokens a step of `role` takes, from past runs or a prior.
fn per_step(role: Option<Role>) -> (f32, u64) {
    let key = role.map_or("solo", Role::name);
    if let Some(v) = stats().get(key) {
        if let (Some(m), Some(t)) = (v["minutes"].as_f64(), v["tokens"].as_u64()) {
            return (m as f32, t);
        }
    }
    match role {
        None => (2.0, 60_000),
        Some(Role::Director) => (3.0, 120_000),
        Some(Role::Editor) => (5.0, 250_000),
        Some(Role::Animator) => (7.0, 350_000),
        Some(Role::Sound) => (4.0, 180_000),
        Some(Role::Colour) => (3.0, 150_000),
        Some(Role::Vfx) => (6.0, 300_000),
    }
}

/// How long the plan takes: its longest chain of waiting steps (steps
/// side by side overlap), plus planning and review for a crew.
fn estimate(steps: &[Step], crew: bool) -> Estimate {
    let mut finish: HashMap<usize, f32> = HashMap::new();
    let mut tokens = 0u64;
    let mut serial = 0.0f32;
    for s in steps {
        let (m, t) = per_step(s.role);
        tokens += t;
        serial += m;
        let start = s.after.iter().filter_map(|a| finish.get(a)).fold(0.0f32, |x, y| x.max(*y));
        finish.insert(s.id, start + m);
    }
    let chain = finish.values().fold(0.0f32, |x, y| x.max(*y));
    // At most MAX_PARALLEL at once.
    let minutes = if crew { chain.max(serial / MAX_PARALLEL as f32) } else { serial };
    let (dm, dt) = per_step(Some(Role::Director));
    if crew { Estimate { minutes: minutes + dm * 2.0, tokens: tokens + dt * 2 } } else { Estimate { minutes, tokens } }
}

fn stats_path() -> std::path::PathBuf {
    super::taste::ai_dir().join("crew-stats.json")
}

fn stats() -> serde_json::Map<String, Value> {
    std::fs::read_to_string(stats_path()).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).and_then(|v| v.as_object().cloned()).unwrap_or_default()
}

/// Folds a finished plan's real times and tokens into the averages.
fn learn(plan: &Plan) {
    let mut all = stats();
    for s in plan.steps.iter().filter(|s| s.status == Status::Done && s.took > 1.0) {
        let key = s.role.map_or("solo", Role::name);
        let (m0, t0) = per_step(s.role);
        let (m, t) = (s.took / 60.0, s.tokens.max(1));
        // A slow moving average: one odd run does not swing it.
        all.insert(key.into(), json!({"minutes": m0 * 0.7 + m * 0.3, "tokens": (t0 as f64 * 0.7 + t as f64 * 0.3) as u64}));
    }
    if let Some(dir) = stats_path().parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(stats_path(), Value::Object(all).to_string());
}

// ---------------------------------------------------------------- honesty

/// Words that say Studio cannot do something.
const CANNOT: &[&str] = &[
    "not possible", "isn't possible", "impossible", "can't animate", "cannot animate", "not supported", "isn't supported", "unsupported", "doesn't support", "does not support", "no way to", "unable to", "not available", "isn't available", "studio can't", "studio cannot", "not able to", "no tool for", "no tool to", "don't have a tool", "do not have a tool", "lacks", "as still", "static panel", "still panel", "still image instead", "instead of animating", "can't be animated", "cannot be animated", "beyond what", "outside what studio",
];

/// True when `said` claims something cannot be done.
pub fn claims_cannot(said: &str) -> bool {
    let s = said.to_lowercase().replace('’', "'");
    CANNOT.iter().any(|w| s.contains(w))
}

const NUDGE: &str = "You said something cannot be done without checking. Studio has far more than you assumed (characters that act, 3D, rigs, generation, compositing). Call `capabilities` with what you wanted now, and if it exists, do it properly instead of the workaround; if it truly does not, say exactly what you searched for.";

/// One turn, then once more if the agent claimed something is impossible
/// without calling `capabilities`.
pub fn honest_turn(brain: &mut dyn Brain, request: &str, gate: &Gate, caller: &str) -> Result<(), String> {
    gate.board.clear_said(caller);
    brain.turn(request, gate)?;
    if gate.stopped() {
        return Ok(());
    }
    if !gate.board.checked(caller) && claims_cannot(&gate.board.said(caller)) {
        gate.emit(AgentEvent::Note("It said something can't be done without checking; asked it to look up Studio's tools first.".into()));
        gate.board.clear_said(caller);
        brain.turn(NUDGE, gate)?;
    }
    // 3D work is not done while the critic has notes: one more go at them.
    if !gate.stopped() {
        let (open, _) = gate.open_3d_notes();
        if !open.is_empty() {
            gate.emit(AgentEvent::Note(format!("{} 3D notes are still open; asked it to fix them.", open.len())));
            gate.board.clear_said(caller);
            brain.turn(&three_d_nudge(&open), gate)?;
        }
    }
    Ok(())
}

/// The prompt for a request that ended with 3D notes open.
pub fn three_d_nudge(open: &[String]) -> String {
    let list: Vec<String> = open.iter().take(12).map(|n| format!("- {n}")).collect();
    format!("You stopped with 3D notes still open on the film:\n{}\nThey are not optional polish: a primitive, a part that is missing, a model swapped for another, flat light or a static camera is unfinished 3D work. Fix each properly (read `guide` topic \"models\" for the hero-object recipe), then `review` again. Do not say review passed or the film is done while any stands; if one truly cannot be fixed, say so plainly in your final answer and list it.", list.join("\n"))
}

// ---------------------------------------------------------------- the crew

/// Specialists working at once.
pub const MAX_PARALLEL: usize = 3;
/// Review rounds before the director must approve or say what is left.
const ROUNDS: u32 = 2;

/// Makes the brain for a role (`None`: a solo agent).
pub type Casting = Box<dyn Fn(Option<Role>) -> Result<Box<dyn Brain>, String> + Send>;

/// A request through a crew: the director plans, specialists work in
/// parallel, the director reviews.
pub struct CrewBrain {
    cast: Casting,
    director: Option<Box<dyn Brain>>,
}

impl CrewBrain {
    pub fn new(cast: Casting) -> Self {
        Self { cast, director: None }
    }

    fn publish(gate: &Gate) {
        gate.emit(AgentEvent::Plan(gate.board.snapshot()));
    }

    /// The prompt a specialist gets for its step.
    fn task_prompt(gate: &Gate, request: &str, step: &Step) -> String {
        let plan = gate.board.snapshot();
        let mut p = format!("The person asked: {request}\n\nThe director's plan:\n{}\n\nYour step is {}: {}\n{}\n", plan.text(), step.id, step.title, step.task);
        if !step.clips.is_empty() {
            p.push_str(&format!("The clips you hold for it: {:?}.\n", step.clips));
        }
        let done: Vec<String> = plan.steps.iter().filter(|s| step.after.contains(&s.id)).filter_map(|s| s.last_note().map(|n| format!("- step {} ({}): {}", s.id, s.role.map_or("", Role::label), n))).collect();
        if !done.is_empty() {
            p.push_str(&format!("What the steps before yours made:\n{}\n", done.join("\n")));
        }
        let fixes = gate.board.corrections();
        if !fixes.is_empty() {
            p.push_str(&format!("The person has corrected the job while it ran (these win over the plan):\n- {}\n", fixes.join("\n- ")));
        }
        p
    }

    fn redo_prompt(gate: &Gate, step: &Step, note: &str) -> String {
        let fixes = gate.board.corrections();
        let mut p = format!("The director sent step {} back after watching the film: {note}\nFix your part, check it again, and answer as before.", step.id);
        if !fixes.is_empty() {
            p.push_str(&format!("\nThe person's corrections so far: {}", fixes.join("; ")));
        }
        p
    }

    /// Runs the plan's steps until all are settled (or stopped).
    fn work(&mut self, request: &str, gate: &Gate, brains: &mut HashMap<usize, Box<dyn Brain>>, redo_notes: &HashMap<usize, String>) {
        let (done_tx, done_rx) = mpsc::channel::<(usize, Box<dyn Brain>, Result<(), String>)>();
        let mut asks: HashMap<usize, mpsc::Receiver<Answer>> = HashMap::new();
        let mut running = 0usize;
        std::thread::scope(|scope| {
            loop {
                if gate.stopped() {
                    break;
                }
                if !gate.board.paused.load(Ordering::SeqCst) {
                    for id in gate.board.ready(MAX_PARALLEL) {
                        let Some(step) = gate.board.step(id) else { continue };
                        let Some(role) = step.role else { continue };
                        let brain = match brains.remove(&id).map(Ok).unwrap_or_else(|| (self.cast)(Some(role))) {
                            Ok(b) => b,
                            Err(e) => {
                                gate.board.note(id, &e);
                                gate.board.set_status(id, Status::Failed);
                                Self::publish(gate);
                                continue;
                            }
                        };
                        let prompt = match (step.status, redo_notes.get(&id)) {
                            (Status::SentBack, Some(note)) => Self::redo_prompt(gate, &step, note),
                            (Status::SentBack, None) => Self::redo_prompt(gate, &step, step.last_note().unwrap_or("the person sent it back")),
                            _ => Self::task_prompt(gate, request, &step),
                        };
                        gate.board.set_status(id, Status::Running);
                        gate.board.claim_step_clips(id);
                        gate.board.mark_told(&format!("{}:{id}", role.name()));
                        Self::publish(gate);
                        running += 1;
                        let tx = done_tx.clone();
                        scope.spawn(move || {
                            ACTING.with(|a| a.set(Some((role, id))));
                            let mut brain = brain;
                            let caller = format!("{}:{id}", role.name());
                            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| honest_turn(brain.as_mut(), &prompt, gate, &caller))).unwrap_or_else(|p| Err(super::guard::describe("the specialist", p.as_ref())));
                            ACTING.with(|a| a.set(None));
                            let _ = tx.send((id, brain, r));
                        });
                    }
                }
                // Finished specialists.
                while let Ok((id, brain, r)) = done_rx.try_recv() {
                    running -= 1;
                    brains.insert(id, brain);
                    match r {
                        Err(e) if !gate.stopped() => {
                            gate.board.note(id, &format!("Failed: {e}"));
                            gate.board.set_status(id, Status::Failed);
                        }
                        _ if gate.stopped() => {}
                        _ => {
                            let step = gate.board.step(id);
                            if step.as_ref().is_some_and(|s| s.checkpoint) && gate.board.approve_checkpoints.load(Ordering::SeqCst) {
                                gate.board.set_status(id, Status::Waiting);
                                let s = step.expect("step");
                                let (tx, rx) = mpsc::channel();
                                let what = format!("Checkpoint: the {} finished “{}”.\n{}\nApprove to build on it, or skip to send it back (type what to change and it goes with it).", s.role.map_or("crew", Role::label).to_lowercase(), s.title, super::brain::one_line(s.last_note().unwrap_or(""), 300));
                                gate.ask(what, tx);
                                asks.insert(id, rx);
                            } else {
                                gate.board.set_status(id, Status::Done);
                            }
                        }
                    }
                    Self::publish(gate);
                }
                // Checkpoints answered.
                let answered: Vec<(usize, Answer)> = asks.iter().filter_map(|(id, rx)| rx.try_recv().ok().map(|a| (*id, a))).collect();
                for (id, a) in answered {
                    asks.remove(&id);
                    let ok = a != Answer::Skip;
                    gate.board.checkpoint_answer(id, ok);
                    if !ok {
                        super::taste::rejected(gate.project().as_deref(), &format!("sent back the step “{}”", gate.board.step(id).map(|s| s.title).unwrap_or_default()));
                    }
                    Self::publish(gate);
                }
                if running == 0 && asks.is_empty() && (gate.board.all_settled() || gate.board.stuck() || gate.board.ready(MAX_PARALLEL).is_empty() && !gate.board.paused.load(Ordering::SeqCst)) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(60));
            }
            // Stopped: the specialists end on their own (their brains see
            // the stop); wait for them so the scope can close.
            while running > 0 {
                if let Ok((id, brain, _)) = done_rx.recv() {
                    brains.insert(id, brain);
                }
                running -= 1;
            }
        });
    }
}

impl Brain for CrewBrain {
    fn forget(&mut self) {
        self.director = None;
    }

    fn turn(&mut self, request: &str, gate: &Gate) -> Result<(), String> {
        if self.director.is_none() {
            self.director = Some((self.cast)(Some(Role::Director))?);
        }
        gate.set_step_limit(super::brain::MAX_STEPS * 4);
        gate.emit(AgentEvent::Note("The director is reading the brief and planning for the crew.".into()));
        let mut director = self.director.take().expect("director");
        let r = honest_turn(director.as_mut(), request, gate, LEAD);
        if let Err(e) = r {
            self.director = Some(director);
            return Err(e);
        }
        if gate.stopped() {
            self.director = Some(director);
            return Ok(());
        }
        if !gate.board.has_plan() {
            // It answered without a plan (a question, or nothing to do).
            self.director = Some(director);
            return Ok(());
        }
        Self::publish(gate);
        let mut brains: HashMap<usize, Box<dyn Brain>> = HashMap::new();
        let mut redo: HashMap<usize, String> = HashMap::new();
        let mut result = Ok(());
        loop {
            self.work(request, gate, &mut brains, &redo);
            if gate.stopped() {
                break;
            }
            let plan = gate.board.snapshot();
            let left = ROUNDS.saturating_sub(plan.round);
            let made: Vec<String> = plan.steps.iter().map(|s| format!("- step {} ({}, {:?}): {}", s.id, s.role.map_or("", Role::label), s.status, s.last_note().unwrap_or("no answer"))).collect();
            let fixes = gate.board.corrections();
            let mut review = format!("Your crew has finished:\n{}\n", made.join("\n"));
            if !fixes.is_empty() {
                review.push_str(&format!("The person corrected the job while it ran: {}\n", fixes.join("; ")));
            }
            review.push_str(&if left > 0 {
                format!("Review the whole film now (`review`, `see`, `contact_sheet`, `listen`, `check`), then call `plan` with approve or redo ({left} review round(s) left), then answer the person.")
            } else {
                "This is the last review: call `plan` with approve and a summary that says plainly anything still not right, then answer the person.".into()
            });
            gate.emit(AgentEvent::Note("The director is reviewing the crew's work.".into()));
            gate.board.bump_round();
            Self::publish(gate);
            if let Err(e) = honest_turn(director.as_mut(), &review, gate, LEAD) {
                result = Err(e);
                break;
            }
            match gate.board.take_verdict() {
                Some(Verdict::Redo(list)) if left > 0 && !gate.stopped() => {
                    redo.clear();
                    for (id, note) in list {
                        gate.board.note(id, &format!("Director: {note}"));
                        gate.board.set_status(id, Status::SentBack);
                        redo.insert(id, note);
                    }
                    Self::publish(gate);
                }
                _ => break,
            }
        }
        learn(&gate.board.snapshot());
        Self::publish(gate);
        self.director = Some(director);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(board: &Board, steps: Value) {
        board.plan_tool(Some(Role::Director), &json!({"goal": "test", "steps": steps})).unwrap();
    }

    #[test]
    fn roles_parse_and_own_their_ops() {
        assert_eq!(Role::parse("Colorist"), Some(Role::Colour));
        assert_eq!(Role::parse("sound designer"), Some(Role::Sound));
        assert!(Role::Animator.may_use("puppet") && !Role::Animator.may_use("match_colour"));
        assert!(Role::Colour.may_use("set"), "everyone may set properties on their own clips");
        assert!(!Role::Director.may_use("set"), "the director edits nothing");
        assert!(Role::Sound.may_use("some_new_op"), "an op no role owns is open to all");
        for r in Role::CREW {
            let b = r.brief();
            assert!(b.contains("capabilities") && b.contains("craft book"), "{r:?}");
            assert!(b.len() > 2_000, "{r:?} gets its chapters");
        }
        assert!(Role::Animator.brief().contains("# Skill: puppetry-rigging"), "the animator reads the puppetry skill");
    }

    #[test]
    fn the_plan_is_checked_and_scheduled_in_parallel() {
        let b = Board::default();
        assert!(b.plan_tool(Some(Role::Director), &json!({"steps": [{"role": "baker", "title": "x"}]})).is_err());
        assert!(b.plan_tool(Some(Role::Director), &json!({"steps": [{"role": "editor", "title": "x", "after": [1]}]})).is_err(), "a step cannot wait for itself");
        plan(&b, json!([
            {"role": "editor", "title": "Cut", "task": "cut it"},
            {"role": "colour", "title": "Grade"},
            {"role": "sound", "title": "Score", "after": [1]},
            {"role": "editor", "title": "Titles"},
        ]));
        let p = b.snapshot();
        assert!(p.crew && p.estimate.is_some_and(|e| e.minutes > 0.0 && e.tokens > 0));
        // Steps 1 and 2 start at once; 4 waits for the editor; 3 for step 1.
        assert_eq!(b.ready(3), vec![1, 2]);
        b.set_status(1, Status::Running);
        b.set_status(2, Status::Running);
        assert!(b.ready(3).is_empty(), "the editor is busy and the score waits for the cut");
        b.set_status(1, Status::Done);
        assert_eq!(b.ready(3), vec![3, 4]);
        assert!(b.plan_tool(Some(Role::Editor), &json!({"steps": []})).is_err(), "only the director plans");
    }

    #[test]
    fn specialists_keep_off_each_others_clips() {
        let b = Board::default();
        plan(&b, json!([{"role": "animator", "title": "Fox", "clips": [7]}, {"role": "colour", "title": "Grade"}]));
        b.set_status(1, Status::Running);
        b.claim_step_clips(1);
        b.set_status(2, Status::Running);
        ACTING.with(|a| a.set(Some((Role::Colour, 2))));
        let e = b.check_edit(Role::Colour, &json!({"ops": [{"op": "set", "clip": 7, "saturation": 120}]})).unwrap_err();
        assert!(e.contains("animator"), "{e}");
        assert!(b.check_edit(Role::Colour, &json!({"ops": [{"op": "set", "clip": 8, "saturation": 120}]})).is_ok());
        let e = b.check_edit(Role::Colour, &json!({"ops": [{"op": "puppet", "clip": 8}]})).unwrap_err();
        assert!(e.contains("animator's work"), "{e}");
        // Inside a repeat too.
        assert!(b.check_edit(Role::Colour, &json!({"ops": [{"op": "repeat", "times": 2, "do": [{"op": "puppet", "clip": 9}]}]})).is_err());
        // New clips the colourist makes become its own.
        b.claim(Role::Colour, &json!({"ops": [{"op": "fx_layer", "item": "Glow"}]}), r#"{"ok":true,"changed":{"added":[{"id":12}],"changed":[]}}"#);
        ACTING.with(|a| a.set(Some((Role::Animator, 1))));
        assert!(b.check_edit(Role::Animator, &json!({"ops": [{"op": "set", "clip": 12, "x": 4}]})).is_err());
        // A finished step lets go.
        b.set_status(2, Status::Done);
        assert!(b.check_edit(Role::Animator, &json!({"ops": [{"op": "set", "clip": 12, "x": 4}]})).is_ok());
        ACTING.with(|a| a.set(None));
        assert!(b.check_edit(Role::Director, &json!({"ops": []})).is_err());
    }

    #[test]
    fn a_correction_reaches_each_agent_once() {
        let b = Board::default();
        b.steer("make it night");
        assert_eq!(b.take_corrections("lead").as_deref(), Some("make it night"));
        assert_eq!(b.take_corrections("lead"), None);
        assert_eq!(b.take_corrections("animator:2").as_deref(), Some("make it night"), "each agent hears it");
        b.mark_told("sound:3");
        assert_eq!(b.take_corrections("sound:3"), None, "a step that starts after it has it in its prompt");
        b.steer("and slower");
        assert_eq!(b.take_corrections("lead").as_deref(), Some("and slower"));
    }

    #[test]
    fn a_solo_plan_ticks_and_holds_at_checkpoints() {
        let b = Board::default();
        b.plan_tool(None, &json!({"steps": [{"title": "Rough cut"}, {"title": "Music", "checkpoint": true}]})).unwrap();
        assert!(!b.snapshot().crew);
        b.plan_tool(None, &json!({"start": 1, "note": "cutting"})).unwrap();
        assert_eq!(b.snapshot().steps[0].status, Status::Running);
        b.plan_tool(None, &json!({"done": 1})).unwrap();
        b.approve_checkpoints.store(true, Ordering::SeqCst);
        b.plan_tool(None, &json!({"start": 2})).unwrap();
        let (_, ask) = b.plan_tool(None, &json!({"done": 2})).unwrap();
        assert_eq!(ask, Some(2), "the person approves the checkpoint");
        assert_eq!(b.snapshot().steps[1].status, Status::Waiting);
        b.checkpoint_answer(2, false);
        assert_eq!(b.snapshot().steps[1].status, Status::SentBack);
        assert!(b.snapshot().text().contains("SentBack"));
    }

    #[test]
    fn impossibility_claims_are_caught() {
        assert!(claims_cannot("Animation isn’t possible here, so I used still panels."));
        assert!(claims_cannot("Studio can't animate characters"));
        assert!(!claims_cannot("I animated the fox's wave and added birdsong."));
    }

    #[test]
    fn an_estimate_counts_the_longest_chain() {
        let mk = |id, role, after: Vec<usize>| Step { id, role: Some(role), title: String::new(), task: String::new(), clips: vec![], after, checkpoint: false, status: Status::Pending, notes: vec![], started: None, took: 0.0, tokens: 0 };
        let side = estimate(&[mk(1, Role::Colour, vec![]), mk(2, Role::Colour, vec![])], true);
        let chain = estimate(&[mk(1, Role::Colour, vec![]), mk(2, Role::Colour, vec![1])], true);
        assert!(chain.minutes > side.minutes, "{chain:?} vs {side:?}");
        assert_eq!(chain.tokens, side.tokens);
    }
}
