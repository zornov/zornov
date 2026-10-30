// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The Assistant in the AI panel: ask for anything, watch it happen on the
//! timeline. A line starting with `/` runs a Studio command directly (the
//! `studio` prompt's commands), without the model.

use crate::agent::brain::{AgentEvent, Answer, ApiBrain, Brain, ClaudeCodeBrain, CodexBrain, Gate, one_line};
use crate::agent::crew::{self, CrewBrain, Halt, Plan, Role, Status};
use crate::agent::openai::OpenAiBrain;
use crate::agent::live::{Live, ToolCall, ToolResult};
use crate::agent::usage::{self, Usage};
use crate::agent::{Content, keychain};
use crate::app::StudioApp;
use crate::icons::{self, Icon};
use crate::kit::{self, Button, IconButton};
use crate::theme::{self, Tokens};
use eframe::egui::{self, Align2, Color32, CornerRadius, FontFamily, FontId, Pos2, Rect, Sense, Stroke, Vec2};
use eframe::egui::text::{LayoutJob, TextFormat};
use std::path::PathBuf;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use crate::kit::ShowNoted;
use super::agent_digest as digest;

enum Step {
    Running,
    Done(bool),
}

enum Entry {
    You(String),
    Said(String),
    Tool {
        id: u32,
        tool: String,
        brief: String,
        /// The step in plain words (an edit's label), for the panel.
        what: String,
        step: Step,
        result: String,
        pictures: Vec<egui::TextureHandle>,
        started: Instant,
        took: Option<f32>,
    },
    Note(String),
    Error(String),
    /// The plan of a request, kept up to date while it runs.
    Plan(Plan),
    /// A correction sent while a request ran.
    Correction(String),
    /// A request that is over, with what it changed (its Undo).
    Done { request: u64 },
    /// A `/` command and what it printed.
    Command { line: String, result: Option<Result<String, String>>, pictures: Vec<egui::TextureHandle>, reply: Option<Receiver<ToolResult>> },
}

/// What a mark on the picture is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkKind {
    /// "Here": a part of the picture the request is about.
    Area,
    /// A note pinned to a spot or a box.
    Note,
}

/// A mark on the picture for the next message.
#[derive(Clone, Debug)]
pub struct Mark {
    pub kind: MarkKind,
    /// Timeline time it was made at.
    pub time: crate::view::Time,
    /// Canvas pixels from the centre, y down: x0, y0, x1, y1 (a spot has
    /// x0 = x1, y0 = y1).
    pub rect: [f32; 4],
    pub note: String,
}

impl Mark {
    /// For the person: one line of the quote above their message.
    fn quote(&self, n: usize) -> String {
        let when = super::ai_marks::time_label(self.time, 0.0);
        match self.kind {
            MarkKind::Area => format!("> {n} · Area at {when}"),
            MarkKind::Note => format!("> {n} · Note at {when}: “{}”", self.note),
        }
    }

    /// For the AI: where and when, exactly.
    fn for_ai(&self, n: usize) -> String {
        let r = self.rect;
        let at = format!("{:.2}s", self.time as f64 / crate::view::SEC as f64);
        let place = if (r[2] - r[0]).abs() < 2.0 && (r[3] - r[1]).abs() < 2.0 {
            format!("the spot ({:.0}, {:.0})", r[0], r[1])
        } else {
            format!("the box from ({:.0}, {:.0}) to ({:.0}, {:.0})", r[0], r[1], r[2], r[3])
        };
        match self.kind {
            MarkKind::Area => format!("mark {n}: the person selected {place} of the canvas at {at}; \"this\"/\"here\" means that area"),
            MarkKind::Note => format!("mark {n}: the person pinned a note to {place} of the canvas at {at}: \"{}\"", self.note),
        }
    }
}

struct Ask {
    id: u32,
    what: String,
    answer: Sender<Answer>,
}

/// One conversation: a tab of the AI panel, with its own AI session.
struct Conversation {
    entries: Vec<Entry>,
    brain: Arc<Mutex<Option<Box<dyn Brain>>>>,
    /// A brain for the settings just chosen, put in place by the next
    /// request's thread.
    pending_brain: Arc<Mutex<Option<Box<dyn Brain>>>>,
    /// (provider, model, crew) the brain was made for.
    brain_for: Option<(u8, String, bool)>,
    /// The request whose plan the last `Entry::Plan` shows.
    plan_turn: u64,
    /// Tokens the request under way has used, and the whole conversation.
    request_usage: Usage,
    session_usage: Usage,
    /// A picture shown large (texture id).
    enlarged: Option<egui::TextureId>,
}

impl Default for Conversation {
    fn default() -> Self {
        Self { entries: Vec::new(), brain: Arc::new(Mutex::new(None)), pending_brain: Arc::new(Mutex::new(None)), brain_for: None, plan_turn: 0, request_usage: Usage::default(), session_usage: Usage::default(), enlarged: None }
    }
}

impl Conversation {
    /// The tab's name: its first request, short.
    fn title(&self) -> String {
        self.entries
            .iter()
            .find_map(|e| if let Entry::You(s) = e { s.lines().find(|l| !l.starts_with("> ")).map(|l| one_line(l, 22)) } else { None })
            .unwrap_or_else(|| "New chat".into())
    }
}

pub struct AgentUi {
    /// Conversations, as tabs; `active` is shown, `running` (if any) is
    /// the one whose request is under way (one at a time: they share the
    /// project).
    tabs: Vec<Conversation>,
    active: usize,
    running: Option<usize>,
    live: Live,
    calls: Receiver<ToolCall>,
    gate: Arc<Gate>,
    events: Receiver<AgentEvent>,
    socket: Option<PathBuf>,
    asks: Vec<Ask>,
    pub input: String,
    busy: bool,
    pub focus_input: bool,
    /// When the running request started.
    turn_started: Option<Instant>,
    /// The current request's number; a request the person stopped is
    /// written to `stopped_turn` so its thread, if still waiting, does not
    /// start.
    turn: u64,
    /// The running request's number: its changes carry it.
    request: u64,
    /// The clips the AI selected last: it moves the selection only while
    /// the person has not chosen their own.
    ai_selected: Vec<u32>,
    stopped_turn: Arc<AtomicU64>,
    /// Requests sent while one was running (with their tab): they go out
    /// in order after it.
    queued: VecDeque<(usize, String)>,
    /// Media panel ▸ Download from a link: the link being typed (Some while
    /// the dialog is open), and downloads under way.
    link: Option<String>,
    downloads: Vec<(String, Receiver<ToolResult>)>,
    /// Media already checked for a format Studio cannot play.
    media_checked: std::collections::HashSet<u64>,
    /// The viewer's marking tool in use, a box being drawn (canvas
    /// coordinates of its first corner), a note being written, and the
    /// marks that go with the next message.
    pub mark_mode: Option<MarkKind>,
    pub drawing: Option<[f32; 2]>,
    pub pending_note: Option<Mark>,
    pub attachments: Vec<Mark>,
    /// The project folder last told to outside agents.
    registered: Option<Option<PathBuf>>,
    /// Each request's full step list is open (remembered for this panel).
    show_steps: bool,
}

impl Drop for AgentUi {
    fn drop(&mut self) {
        if let Some(s) = &self.socket {
            crate::agent::attach::unregister(s);
        }
    }
}

impl AgentUi {
    pub fn new(ctx: &egui::Context) -> Self {
        let (calls_tx, calls) = mpsc::channel();
        let (events_tx, events) = mpsc::channel();
        let c1 = ctx.clone();
        let c2 = ctx.clone();
        let gate = Arc::new(Gate::new(calls_tx, events_tx, move || c1.request_repaint()));
        let socket = crate::agent::attach::serve(gate.clone()).ok();
        let live = Live::new(move || c2.request_repaint());
        // One owner for the window's AI: Stop ends the programs of its
        // brains and the jobs of its tools together.
        gate.set_owner(live.owner());
        Self {
            live,
            calls,
            gate,
            events,
            socket,
            tabs: vec![Conversation::default()],
            active: 0,
            running: None,
            asks: Vec::new(),
            input: String::new(),
            busy: false,
            focus_input: false,
            turn_started: None,
            turn: 0,
            request: 0,
            ai_selected: Vec::new(),
            stopped_turn: Arc::new(AtomicU64::new(0)),
            queued: VecDeque::new(),
            link: None,
            downloads: Vec::new(),
            media_checked: std::collections::HashSet::new(),
            mark_mode: None,
            drawing: None,
            pending_note: None,
            attachments: Vec::new(),
            registered: None,
            show_steps: ctx.data_mut(|d| d.get_persisted::<bool>(egui::Id::new("agent-show-steps"))).unwrap_or(false),
        }
    }

    /// Visual checks: a question as the Assistant would ask it.
    pub fn demo_ask(&mut self, with_question: bool) {
        let (tx, _rx) = mpsc::channel();
        std::mem::forget(_rx);
        self.tabs[self.active].entries.push(Entry::You("Cut the road trip to the music, a new place on every beat".into()));
        self.tabs[self.active].entries.push(Entry::Said("I'll cut the road trip to the score's beats, **widest views first**:\n\n- open on Glacier National Park\n- a new place every 2 beats, faster at the end\n- end on the lighthouse".into()));
        let now = Instant::now();
        for (i, (tool, brief, ok)) in [("guide", "", Some(true)), ("listen", "Warm acoustic score", Some(true)), ("see", "Road trip · 2s", Some(true)), ("edit", "3 ops: montage, set, music", None)].into_iter().enumerate() {
            self.tabs[self.active].entries.push(Entry::Tool { id: 10 + i as u32, tool: tool.into(), brief: brief.into(), what: String::new(), step: ok.map_or(Step::Running, Step::Done), result: String::new(), pictures: Vec::new(), started: now, took: ok.map(|_| 0.4 + i as f32 * 0.7) });
        }
        self.turn_started = Some(now);
        self.running = Some(self.active);
        if !with_question {
            // A second conversation, in its own tab.
            let mut other = Conversation::default();
            other.entries.push(Entry::You("Warm up the desert shots".into()));
            other.entries.push(Entry::Said("Done: a warmer grade on the two desert shots.".into()));
            self.tabs.push(other);
            self.tabs[self.active].entries.push(Entry::Said("Here's the plan for the opening. I'll keep `Glacier National Park` as the first shot and cut on the beat at **1.8s**.".into()));
            return;
        }
        self.asks.push(Ask { id: 1, what: "Add 8 clips: Glacier National Park (0.0–1.8s), Yosemite Valley (1.8–3.6s), Monument Valley (3.6–5.4s), Grand Canyon (5.4–7.2s) and 4 more\nRemove 1 clip: Road trip rough cut (0.0–14.4s)\n(3 ops: montage, set, music)".into(), answer: tx });
        self.busy = true;
    }

    /// Imports files Studio cannot open as they are: converted first, on
    /// the worker, with a notice when they are in.
    pub fn import_later(&mut self, ctx: &egui::Context, paths: Vec<std::path::PathBuf>) {
        for p in paths {
            let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let (tx, rx) = mpsc::channel();
            self.live.push(ToolCall { tool: "import".into(), args: serde_json::json!({ "paths": [p.display().to_string()] }), reply: tx, epoch: 0 });
            self.downloads.push((format!("converted {name} and imported it"), rx));
            kit::push(ctx, crate::i18n::tf(format!("Converting {name} to a format Zornov plays…")), None, Some(Icon::FilmStrip));
        }
    }

    /// Visual checks: marks on the picture, attached and sent.
    pub fn demo_marks(&mut self, playhead: crate::view::Time, writing: bool) {
        let area = Mark { kind: MarkKind::Area, time: playhead, rect: [-420.0, -260.0, 60.0, 120.0], note: String::new() };
        let note = Mark { kind: MarkKind::Note, time: playhead, rect: [380.0, 200.0, 380.0, 200.0], note: "this rock is distracting, blur it".into() };
        if writing {
            self.mark_mode = Some(MarkKind::Note);
            self.pending_note = Some(Mark { note: "make the sky warmer".into(), rect: [-600.0, -420.0, 200.0, -180.0], ..note });
            return;
        }
        let sent = format!("{}\n{}\nWarm up the mountains and calm that spot", area.quote(1), note.quote(2));
        self.tabs[self.active].entries.push(Entry::You(sent));
        self.tabs[self.active].entries.push(Entry::Said("Warmed the grade on the mountains (box 1) and blurred the rock (spot 2).".into()));
        self.attachments = vec![area, note];
    }

    /// Runs a tool on this window's project, as a `/` command does
    /// (the viewer's shot and lighting menus).
    pub fn queue_call(&mut self, call: crate::agent::live::ToolCall) {
        self.live.push(call);
    }

    /// Visual and UI tests: a long run as the AI does it, many guide reads and
    /// edits, with a check at the end.
    #[cfg(test)]
    pub(crate) fn demo_long_run(&mut self) {
        let now = Instant::now();
        let c = &mut self.tabs[self.active];
        c.entries.push(Entry::You("Make the product shot".into()));
        c.entries.push(Entry::Said("I'll build the product shot, then mix the sound.".into()));
        let ok = r#"{"changed":{"added":[{"id":1}],"removed":[],"changed":[]}}"#;
        let mut id = 0;
        let mut tool = |c: &mut Conversation, tool: &str, brief: &str, what: &str, result: &str| {
            id += 1;
            c.entries.push(Entry::Tool { id, tool: tool.into(), brief: brief.into(), what: what.into(), step: Step::Done(true), result: result.into(), pictures: Vec::new(), started: now, took: Some(0.3) });
        };
        for _ in 0..6 {
            tool(c, "guide", "", "", "GUIDE TEXT THAT MUST NEVER SHOW");
        }
        tool(c, "summary", "", "", "{}");
        for _ in 0..12 {
            tool(c, "edit", "cut", "Cut on the beat", ok);
        }
        tool(c, "edit", "music", "", ok);
        tool(c, "check", "", "", r#"{"problems":[1,2]}"#);
        tool(c, "check", "", "", r#"{"problems":[]}"#);
        c.entries.push(Entry::Said("Done: the shot is cut on the beat with music under it.".into()));
    }

    #[cfg(test)]
    pub(crate) fn show_steps_open(&self) -> bool {
        self.show_steps
    }

    /// How many result rows the default view of the active chat shows.
    #[cfg(test)]
    pub(crate) fn default_rows(&self) -> usize {
        let e = &self.tabs[self.active].entries;
        let idx: Vec<usize> = (0..e.len()).filter(|&j| matches!(e[j], Entry::Tool { .. })).collect();
        digest::digest(&digest_steps(e, &idx)).rows.len()
    }

    pub fn busy(&self) -> bool {
        self.busy || self.live.busy()
    }

    /// Every frame, panel open or not: agents in the app and outside it
    /// (`zornov mcp`, `zornov agent`) work on the document in this window.
    pub fn tick(app: &mut StudioApp, ctx: &egui::Context) {
        if app.agent.is_none() {
            app.agent = Some(AgentUi::new(ctx));
        }
        let Some(a) = app.agent.as_mut() else { return };
        let folder = app.doc.folder.as_ref().map(|f| f.root().to_path_buf());
        if a.registered.as_ref() != Some(&folder) {
            if let Some(s) = &a.socket {
                crate::agent::attach::register(s, folder.as_deref());
            }
            a.registered = Some(folder);
        }
        a.live.tick(&mut app.doc, &a.calls);
        if std::mem::take(&mut a.live.made_project) {
            app.give_folder();
        }
        // Calls about the window's panels.
        let calls = app.agent.as_mut().map(|a| std::mem::take(&mut a.live.window_calls)).unwrap_or_default();
        for c in calls {
            let r = super::dock_ui::agent_tool(app, ctx, &c.args).map(|v| vec![crate::agent::Content::Text(v.to_string())]);
            let _ = c.reply.send(r);
        }
        let Some(a) = app.agent.as_mut() else { return };
        // Show where an agent just worked: its clips become the selection.
        let touched = std::mem::take(&mut a.live.touched);
        if !touched.is_empty() && (app.editor.selection.is_empty() || app.editor.selection == a.ai_selected) {
            app.editor.selection = touched.into_iter().filter(|id| app.doc.project.clips.iter().any(|c| c.id == *id)).collect();
            a.ai_selected = app.editor.selection.clone();
        }
        let Some(a) = app.agent.as_mut() else { return };
        // Esc stops the AI before the editor's own shortcuts see it.
        let ai_panel = app.editor.dock.is_showing(crate::dock::Panel::Ai) || app.editor.dock.is_popped(crate::dock::Panel::Ai);
        let in_chat = ctx.memory(|m| m.focused()) == Some(egui::Id::unique("agent-input"));
        if a.busy && (ai_panel || in_chat) && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            a.stop(&mut app.doc);
        }
        let mut asked = false;
        let mut send_next = false;
        // Steps and words go to the tab that asked (an agent outside the
        // app: the one in view).
        let ti = a.running.unwrap_or(a.active).min(a.tabs.len() - 1);
        while let Ok(e) = a.events.try_recv() {
            match e {
                AgentEvent::Text(t) => match a.tabs[ti].entries.last_mut() {
                    Some(Entry::Said(s)) => s.push_str(&t),
                    _ => a.tabs[ti].entries.push(Entry::Said(t.trim_start().to_owned())),
                },
                AgentEvent::Note(n) => a.tabs[ti].entries.push(Entry::Note(n)),
                AgentEvent::ToolStart { id, tool, brief, what } => a.tabs[ti].entries.push(Entry::Tool { id, tool, brief, what, step: Step::Running, result: String::new(), pictures: Vec::new(), started: Instant::now(), took: None }),
                AgentEvent::ToolEnd { id, ok, brief, pictures } => {
                    let pics = textures(ctx, &pictures);
                    match a.tabs[ti].entries.iter_mut().rev().find(|e| matches!(e, Entry::Tool { id: i, .. } if *i == id)) {
                        Some(Entry::Tool { tool, step, result, pictures, started, took, .. }) => {
                            *step = Step::Done(ok);
                            *took = Some(started.elapsed().as_secs_f32());
                            // The AI's reading material (guide, notes) is never kept to show.
                            *result = if digest::is_reading(tool) && ok { String::new() } else { brief };
                            *pictures = pics;
                        }
                        _ => a.tabs[ti].entries.push(Entry::Note(if brief == "skipped" { "You skipped that step.".into() } else { brief })),
                    }
                }
                AgentEvent::Ask { id, what, answer } => {
                    a.asks.push(Ask { id, what, answer });
                    asked = true;
                }
                AgentEvent::Usage(u) => {
                    a.tabs[ti].request_usage.add(u);
                    a.tabs[ti].session_usage.add(u);
                }
                AgentEvent::Error(e) => a.tabs[ti].entries.push(Entry::Error(e)),
                AgentEvent::Plan(p) => {
                    let turn = a.turn;
                    let c = &mut a.tabs[ti];
                    let current = if c.plan_turn == turn { c.entries.iter_mut().rev().find_map(|e| if let Entry::Plan(old) = e { Some(old) } else { None }) } else { None };
                    match current {
                        Some(old) => *old = p,
                        None if !p.steps.is_empty() => {
                            c.entries.push(Entry::Plan(p));
                            c.plan_turn = turn;
                        }
                        None => {}
                    }
                }
                AgentEvent::Done { turn, .. } if turn != a.turn => {}
                AgentEvent::Done { stopped, .. } => {
                    // The request's thread is done (a stopped one too): the
                    // gate is free for agents outside the app again.
                    a.gate.end();
                    if a.busy {
                        a.live.end_turn(&mut app.doc);
                        if app.doc.request_undoable(a.request) {
                            a.tabs[ti].entries.push(Entry::Done { request: a.request });
                        }
                        a.busy = false;
                        a.running = None;
                        a.turn_started = None;
                        a.asks.clear();
                        if stopped {
                            a.tabs[ti].entries.push(Entry::Note("Stopped. What was already done is kept; Undo this request takes it back.".into()));
                        }
                        send_next = true;
                    }
                }
            }
        }
        for e in a.tabs.iter_mut().flat_map(|c| c.entries.iter_mut()) {
            if let Entry::Command { result, pictures, reply: r @ Some(_), .. } = e {
                if let Ok(res) = r.as_ref().expect("reply").try_recv() {
                    *r = None;
                    let (text, pics) = match res {
                        Ok(parts) => {
                            let pics: Vec<Vec<u8>> = parts.iter().filter_map(|p| if let Content::Jpeg(b) = p { Some(b.clone()) } else { None }).collect();
                            let text: Vec<String> = parts.into_iter().filter_map(|p| if let Content::Text(t) = p { Some(t) } else { None }).collect();
                            (Ok(text.join("\n")), pics)
                        }
                        Err(e) => (Err(e), Vec::new()),
                    };
                    *result = Some(text);
                    *pictures = textures(ctx, &pics);
                }
            }
        }
        if a.busy() {
            ctx.request_repaint_after(std::time::Duration::from_millis(120));
        }
        if asked {
            // An agent waits for an answer: show the question.
            open(app);
        }
        if send_next {
            AgentUi::send_queued(app);
        }
        link_dialog(app, ctx);
        // Media Studio cannot play (AV1, VP9…), imported by hand, by an agent
        // or in a project just opened: converted in the background.
        if let Some(a) = app.agent.as_mut() {
            let fresh: Vec<(u64, std::path::PathBuf)> = app.doc.model.media.iter().filter(|m| !a.media_checked.contains(&m.id.0)).map(|m| (m.id.0, std::path::PathBuf::from(&m.path))).collect();
            if !fresh.is_empty() {
                let mut need = Vec::new();
                for (id, path) in fresh {
                    a.media_checked.insert(id);
                    if path.is_file() && !crate::convert::playable(&path) {
                        need.push(id);
                    }
                }
                if !need.is_empty() {
                    let (tx, rx) = mpsc::channel();
                    a.live.push(ToolCall { tool: "make_playable".into(), args: serde_json::json!({ "media": need }), reply: tx, epoch: 0 });
                    let what = if need.len() == 1 { "a video".to_owned() } else { format!("{} videos", need.len()) };
                    a.downloads.push((format!("converted {what} so it plays"), rx));
                    kit::push(ctx, crate::i18n::tf(format!("Converting {what} to a format Zornov plays…")), None, Some(Icon::FilmStrip));
                }
            }
        }
        // Downloads from the Media panel: a notice when each lands.
        if let Some(a) = app.agent.as_mut() {
            let mut i = 0;
            while i < a.downloads.len() {
                match a.downloads[i].1.try_recv() {
                    Ok(r) => {
                        let (url, _) = a.downloads.remove(i);
                        let short = one_line(url.rsplit('/').next().unwrap_or(&url), 40);
                        match r {
                            Ok(content) if url.starts_with("converted ") => match failed_in(&content) {
                                None => kit::push(ctx, crate::i18n::tf(format!("Done: {url}")), None, Some(Icon::CheckCircle)),
                                Some(why) => kit::push(ctx, crate::i18n::tf(format!("Could not {}. {}", url.replacen("converted ", "convert ", 1), one_line(&why, 140))), None, Some(Icon::Warning)),
                            },
                            Ok(_) => kit::push(ctx, crate::i18n::tf(format!("Downloaded {short} into the media bin")), None, Some(Icon::CheckCircle)),
                            Err(_) => kit::push(ctx, crate::i18n::tf(format!("Could not download {short}. Check the link and try again")), None, Some(Icon::Warning)),
                        };
                    }
                    Err(mpsc::TryRecvError::Empty) => i += 1,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        a.downloads.remove(i);
                    }
                }
            }
            if !a.downloads.is_empty() {
                ctx.request_repaint_after(std::time::Duration::from_millis(250));
            }
        }
    }

    /// The next request sent while another ran.
    fn send_queued(app: &mut StudioApp) {
        let Some((tab, line)) = app.agent.as_mut().and_then(|a| a.queued.pop_front()) else { return };
        AgentUi::start(app, tab, line);
    }

    /// The brain the settings ask for, made when they change.
    fn ready_brain(&mut self, tab: usize, prefs: &crate::prefs::Prefs, project_dir: Option<PathBuf>) -> Result<(), String> {
        let model = match prefs.ai_provider {
            1 => prefs.ai_claude_code_model.clone(),
            2 => prefs.ai_openai_model.clone(),
            3 => prefs.ai_codex_model.clone(),
            _ => prefs.ai_model.clone(),
        };
        let want = (prefs.ai_provider, model, prefs.ai_crew);
        // A stopped request's thread may still hold the brain for a moment;
        // the next request waits for it on its own thread, never here.
        if self.tabs[tab].brain_for.as_ref() == Some(&want) {
            return Ok(());
        }
        let dir = || {
            let d = project_dir.clone().unwrap_or_else(|| std::env::temp_dir().join("studio-agent"));
            let _ = std::fs::create_dir_all(&d);
            d
        };
        let socket = self.socket.clone().ok_or("The Assistant could not open its connection for the command-line agents.");
        let cast = casting(want.0, want.1.clone(), socket.ok(), dir());
        // A missing key or program is said now, not halfway into a crew.
        cast(None)?;
        let brain: Box<dyn Brain> = if want.2 { Box::new(CrewBrain::new(cast)) } else { cast(None)? };
        // Swapped in by the thread that runs the next request (after the
        // previous one has let go).
        *self.tabs[tab].pending_brain.lock().map_err(|_| "the Assistant crashed; reopen Zornov")? = Some(brain);
        self.tabs[tab].brain_for = Some(want);
        Ok(())
    }

    /// Sends what is in the input.
    fn send(app: &mut StudioApp) {
        let Some(a) = app.agent.as_mut() else { return };
        let line = a.input.trim().to_owned();
        if line.is_empty() && a.attachments.is_empty() {
            return;
        }
        // Marks on the picture go with the message: quoted for the person,
        // exact for the AI.
        let line = if a.attachments.is_empty() || line.starts_with('/') {
            line
        } else {
            let marks = std::mem::take(&mut a.attachments);
            let quotes: Vec<String> = marks.iter().enumerate().map(|(i, m)| m.quote(i + 1)).collect();
            let exact: Vec<String> = marks.iter().enumerate().map(|(i, m)| m.for_ai(i + 1)).collect();
            let said = if line.is_empty() { "Please look at what I marked.".to_owned() } else { line };
            format!("{}\n{said}\n\u{1}{}", quotes.join("\n"), exact.join("; "))
        };
        // A question is waiting: words answer it.
        if !a.asks.is_empty() {
            let w = line.to_lowercase();
            let w = w.trim_matches(|c: char| !c.is_alphanumeric() && c != ' ');
            let answer = match w {
                "apply all" | "all" | "yes to all" | "go ahead" | "do it all" => Some(Answer::ApplyAll),
                "apply" | "yes" | "y" | "ok" | "okay" | "go" | "do it" | "sure" => Some(Answer::Apply),
                "skip" | "no" | "n" | "don't" | "dont" => Some(Answer::Skip),
                _ => None,
            };
            if let Some(ans) = answer {
                a.input.clear();
                let ask = a.asks.remove(0);
                let _ = ask.answer.send(ans);
                return;
            }
            // Words at a checkpoint: what to change; the step goes back.
            if a.asks[0].what.starts_with("Checkpoint:") && !line.starts_with('/') {
                a.input.clear();
                a.gate.steer(&line);
                a.tabs[a.active].entries.push(Entry::Correction(line));
                let ask = a.asks.remove(0);
                let _ = ask.answer.send(Answer::Skip);
                return;
            }
        }
        a.input.clear();
        if a.busy {
            // It goes out when the running request ends.
            let tab = a.active;
            a.queued.push_back((tab, line));
            return;
        }
        let tab = a.active;
        AgentUi::start(app, tab, line);
    }

    /// Runs a request (or a `/` command) in tab `tab`.
    fn start(app: &mut StudioApp, tab: usize, line: String) {
        let Some(a) = app.agent.as_mut() else { return };
        if tab >= a.tabs.len() {
            return;
        }
        if let Some(cmd) = line.strip_prefix('/') {
            let parsed = crate::agent::shell::split_words(cmd).and_then(|w| crate::agent::shell::parse(&w));
            match parsed {
                Ok((tool, args)) => {
                    // For a person, the timeline as lines rather than data.
                    let tool = if tool == "summary" { "script".to_owned() } else { tool };
                    let (tx, rx) = mpsc::channel();
                    a.live.push(ToolCall { tool, args, reply: tx, epoch: 0 });
                    a.tabs[tab].entries.push(Entry::Command { line, result: None, pictures: Vec::new(), reply: Some(rx) });
                }
                Err(e) => a.tabs[tab].entries.push(Entry::Command { line, result: Some(Err(e)), pictures: Vec::new(), reply: None }),
            }
            return;
        }
        let dir = app.doc.folder.as_ref().map(|f| f.root().to_path_buf());
        if let Err(e) = a.ready_brain(tab, &app.prefs, dir) {
            a.tabs[tab].entries.push(Entry::You(line));
            a.tabs[tab].entries.push(Entry::Error(e));
            return;
        }
        // What "this" and "here" mean.
        let sel = &app.editor.selection;
        let context = format!(
            "[editor: playhead {:.2}s; {}]",
            app.editor.playhead as f64 / crate::view::SEC as f64,
            if sel.is_empty() { "nothing selected".to_owned() } else { format!("selected clips {}", sel.iter().map(u32::to_string).collect::<Vec<_>>().join(", ")) }
        );
        // Marks travel after a \u{1}: the person sees the quotes, the AI also
        // gets the exact places.
        let (shown, marks) = match line.split_once('\u{1}') {
            Some((shown, marks)) => (shown.trim_end().to_owned(), format!("\n[{marks}]")),
            None => (line.clone(), String::new()),
        };
        let range = app.doc.project.range.map(|(a, b)| format!("; timeline range {:.2}s to {:.2}s marked", a as f64 / crate::view::SEC as f64, b as f64 / crate::view::SEC as f64)).unwrap_or_default();
        // The overlays the person has on: frame and see read against them.
        let guides = crate::agent::guides::viewer();
        let range = if guides.is_empty() { range } else { format!("{range}; viewer overlays on: {}", crate::agent::guides::describe(&guides)) };
        // A script in the project: the AI builds and times the film from it.
        let range = match crate::agent::script_context(&app.doc.model.story) {
            Some(script) => format!("{range}; {script}"),
            None => range,
        };
        let context = format!("{}{range}]", context.trim_end_matches(']'));
        let ask: String = shown.lines().filter(|l| !l.starts_with("> ")).collect::<Vec<_>>().join("\n");
        let folder = app.doc.folder.as_ref().map(|f| f.root().to_path_buf());
        let taste = crew_taste(folder.as_deref());
        let request = format!("{context}{marks}{taste}\n{ask}");
        let line = shown;
        let crew_on = app.prefs.ai_crew;
        let a = app.agent.as_mut().expect("agent");
        a.tabs[tab].entries.push(Entry::You(line.clone()));
        a.tabs[tab].request_usage = Usage::default();
        a.request = a.live.begin_turn(&crate::i18n::tf(format!("AI: {}", one_line(if ask.trim().is_empty() { &line } else { &ask }, 40))));
        a.gate.ask_first.store(app.prefs.ai_ask_first, Ordering::SeqCst);
        a.gate.set_request_text(if ask.trim().is_empty() { &line } else { &ask });
        a.gate.board.approve_checkpoints.store(app.prefs.ai_checkpoints, Ordering::SeqCst);
        a.gate.set_project(folder);
        a.turn += 1;
        let turn = a.turn;
        a.busy = true;
        a.turn_started = Some(Instant::now());
        let gate = a.gate.clone();
        a.running = Some(tab);
        let brain = a.tabs[tab].brain.clone();
        let pending = a.tabs[tab].pending_brain.clone();
        let stopped_turn = a.stopped_turn.clone();
        std::thread::Builder::new()
            .name("studio-agent-turn".into())
            .spawn(move || {
                // Waits here (not in the window) while a stopped request
                // finishes letting go of the brain.
                let r = match brain.lock() {
                    Ok(mut b) => {
                        if let Some(next) = pending.lock().ok().and_then(|mut p| p.take()) {
                            *b = Some(next);
                        }
                        gate.begin();
                        gate.set_request(&request);
                        if stopped_turn.load(Ordering::SeqCst) == turn {
                            gate.halt();
                        }
                        match b.as_mut() {
                            _ if gate.stopped() => Ok(()),
                            // A crew checks itself; one agent is held to
                            // looking up a tool before saying "impossible".
                            Some(b) if crew_on => std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| b.turn(&request, &gate))).unwrap_or_else(|_| Err("The Assistant crashed during this request.".into())),
                            Some(b) => std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| crew::honest_turn(b.as_mut(), &request, &gate, crew::LEAD))).unwrap_or_else(|_| Err("The Assistant crashed during this request.".into())),
                            None => Err("no Assistant".into()),
                        }
                    }
                    Err(_) => Err("The Assistant crashed earlier; reopen Zornov.".into()),
                };
                if let Err(e) = r {
                    if !gate.stopped() {
                        gate.emit(AgentEvent::Error(e));
                    }
                }
                // Whatever 3D notes are still open close the answer, plainly.
                if !gate.stopped() {
                    gate.report_open_3d();
                }
                gate.emit(AgentEvent::Done { stopped: gate.stopped(), turn });
            })
            .expect("agent turn thread");
    }

    /// Sends what is in the input to the running request as a correction:
    /// every agent on it takes it in at its next step.
    fn correct(app: &mut StudioApp) {
        let Some(a) = app.agent.as_mut() else { return };
        let line = a.input.trim().to_owned();
        if line.is_empty() || line.starts_with('/') {
            return;
        }
        if !a.busy {
            return AgentUi::send(app);
        }
        a.input.clear();
        a.gate.steer(&line);
        let tab = a.running.unwrap_or(a.active);
        a.tabs[tab].entries.push(Entry::Correction(line));
    }

    /// The plan card of the running request shows the board as it is now
    /// (paused, stopped), without waiting for the next event.
    fn refresh_plan(&mut self) {
        if !self.gate.board.has_plan() {
            return;
        }
        let tab = self.running.unwrap_or(self.active).min(self.tabs.len() - 1);
        let turn = self.turn;
        let c = &mut self.tabs[tab];
        if c.plan_turn != turn {
            return;
        }
        if let Some(old) = c.entries.iter_mut().rev().find_map(|e| if let Entry::Plan(old) = e { Some(old) } else { None }) {
            *old = self.gate.board.snapshot();
        }
    }

    /// Pause holds every tool call until Resume.
    fn toggle_pause(&mut self) {
        let on = !self.gate.paused();
        self.gate.pause(on);
        self.live.set_paused(on);
        self.refresh_plan();
        let tab = self.running.unwrap_or(self.active);
        self.tabs[tab].entries.push(Entry::Note(if on { "Paused: the AI holds at its next step until you resume. Type a correction to change course.".into() } else { "Resumed.".into() }));
    }

    /// Stops the running request at once: the panel is free, what already
    /// landed on the timeline stays (Undo this request takes it back), calls still
    /// waiting are dropped and the AI is told to stop. The conversation
    /// goes on: the next request continues it.
    fn stop(&mut self, doc: &mut crate::doc::Doc) {
        if !self.busy {
            return;
        }
        self.stopped_turn.store(self.turn, Ordering::SeqCst);
        // Every crew member's program (with its children) and every job
        // the request started end now; the plan says Stopped.
        let epoch = self.gate.halt();
        for a in self.asks.drain(..) {
            let _ = a.answer.send(Answer::Skip);
        }
        self.live.cancel_at(doc, epoch);
        self.live.end_turn(doc);
        self.refresh_plan();
        if doc.request_undoable(self.request) {
            let tab = self.running.unwrap_or(self.active).min(self.tabs.len() - 1);
            self.tabs[tab].entries.push(Entry::Done { request: self.request });
        }
        self.busy = false;
        self.turn_started = None;
        let dropped = std::mem::take(&mut self.queued).len();
        self.tabs[self.active].entries.push(Entry::Note(if dropped > 0 {
            crate::i18n::tf(format!("Stopped, and {dropped} waiting request{} dropped. What was already done is kept; Undo this request takes it back.", if dropped == 1 { "" } else { "s" }))
        } else {
            "Stopped. What was already done is kept; Undo this request takes it back.".into()
        }));
    }

    /// A new conversation in a tab of its own (with its own AI session);
    /// the others stay as they are, a running request included.
    fn new_tab(&mut self) {
        self.tabs.push(Conversation::default());
        self.active = self.tabs.len() - 1;
        self.input.clear();
        self.focus_input = true;
    }

    /// Closes tab `i` (its request is stopped first; its session ends).
    fn close_tab(&mut self, i: usize, doc: &mut crate::doc::Doc) {
        if i >= self.tabs.len() {
            return;
        }
        if self.running == Some(i) {
            self.stop(doc);
        }
        self.tabs.remove(i);
        if self.tabs.is_empty() {
            self.tabs.push(Conversation::default());
        }
        // Indices after `i` move down one.
        let shift = |t: usize| if t > i { t - 1 } else { t };
        self.running = self.running.map(shift);
        self.queued.retain(|(t, _)| *t != i);
        for q in &mut self.queued {
            q.0 = shift(q.0);
        }
        self.active = shift(self.active).min(self.tabs.len() - 1);
    }
}

/// Who answers, for the panel's header.
pub fn provider_name(p: &crate::prefs::Prefs) -> String {
    let named = |list: &[(&str, &str)], id: &str| list.iter().find(|m| m.0 == id).map_or(id.to_owned(), |m| m.1.to_owned());
    match p.ai_provider {
        1 if p.ai_claude_code_model.is_empty() => "Claude Code".into(),
        1 => crate::i18n::tf(format!("Claude Code · {}", named(crate::agent::brain::MODELS, &p.ai_claude_code_model))),
        2 => named(crate::agent::openai::MODELS, &p.ai_openai_model),
        3 if p.ai_codex_model.is_empty() => "Codex".into(),
        3 => crate::i18n::tf(format!("Codex · {}", named(crate::agent::openai::MODELS, &p.ai_codex_model))),
        _ => named(crate::agent::brain::MODELS, &p.ai_model),
    }
}

/// Makes brains for the provider chosen in Settings: a solo agent
/// (`None`) or a crew member with its brief.
pub fn casting(provider: u8, model: String, socket: Option<PathBuf>, dir: PathBuf) -> crew::Casting {
    Box::new(move |role: Option<Role>| -> Result<Box<dyn Brain>, String> {
        let brief = role.map(Role::brief).unwrap_or_default();
        // Empty for the command-line agents: their own default model.
        let cli_model = Some(model.clone()).filter(|m| !m.is_empty());
        let socket = || socket.clone().ok_or_else(|| "The Assistant could not open its connection for the command-line agents.".to_owned());
        Ok(match provider {
            1 => {
                let mut b = ClaudeCodeBrain::new(socket()?, dir.clone(), cli_model);
                (b.role, b.brief) = (role, brief);
                Box::new(b)
            }
            2 => {
                let key = keychain::api_key(keychain::OPENAI).ok_or("Add your OpenAI API key in Settings ▸ AI, or pick another Assistant there.")?;
                let mut b = OpenAiBrain::new(key, model.clone());
                (b.role, b.brief) = (role, brief);
                Box::new(b)
            }
            3 => {
                let mut b = CodexBrain::new(socket()?, dir.clone(), cli_model);
                (b.role, b.brief) = (role, brief);
                Box::new(b)
            }
            _ => {
                let key = keychain::api_key(keychain::ANTHROPIC).ok_or("Add your Claude API key in Settings ▸ AI, or pick another Assistant there.")?;
                let mut b = ApiBrain::new(key, model.clone());
                (b.role, b.brief) = (role, brief);
                Box::new(b)
            }
        })
    })
}

/// The person's taste and style guide, for the front of a request.
fn crew_taste(folder: Option<&std::path::Path>) -> String {
    let t = crate::agent::taste::brief(folder);
    if t.is_empty() { t } else { format!("\n{t}") }
}

fn textures(ctx: &egui::Context, pictures: &[Vec<u8>]) -> Vec<egui::TextureHandle> {
    pictures
        .iter()
        .enumerate()
        .filter_map(|(i, b)| {
            let img = studio_media::jpeg::decode(b).ok()?;
            let ci = egui::ColorImage::from_rgba_unmultiplied([img.width as usize, img.height as usize], &img.pixels);
            Some(ctx.load_texture(format!("agent-picture-{}-{i}", ctx.cumulative_frame_nr()), ci, egui::TextureOptions::LINEAR))
        })
        .collect()
}

/// Media panel ▸ Download from a link.
pub fn open_link_dialog(app: &mut StudioApp, ctx: &egui::Context) {
    if app.agent.is_none() {
        app.agent = Some(AgentUi::new(ctx));
    }
    if let Some(a) = app.agent.as_mut() {
        a.link = Some(String::new());
    }
}

fn link_dialog(app: &mut StudioApp, ctx: &egui::Context) {
    let Some(a) = app.agent.as_mut() else { return };
    let Some(link) = a.link.as_mut() else { return };
    let t = theme::tokens(ctx);
    let mut go = false;
    let id = egui::Id::unique("download-link");
    let mut cancel = false;
    let out = kit::modal_sized(ctx, id, kit::DialogSize::Medium, "Download from a link", |ui| {
        let body = kit::above_footer(ui).shrink2(Vec2::new(kit::DIALOG_PAD, theme::space::LG));
        let mut ui_body = ui.new_child(egui::UiBuilder::new().max_rect(body).id_salt("download-body"));
        let ui_b = &mut ui_body;
        ui_b.spacing_mut().item_spacing.y = 10.0;
        ui_b.label(egui::RichText::new(crate::i18n::shown("A link to a video, sound or picture file, or to a web page whose video is openly available. It is saved in the project's Downloads folder and added to the media bin.")).font(theme::body()).color(t.text_2));
        let resp = ui_b.add(egui::TextEdit::singleline(link).hint_text("https://…").font(chat_font()).desired_width(f32::INFINITY));
        if !resp.has_focus() && link.is_empty() {
            resp.request_focus();
        }
        if resp.lost_focus() && ui_b.input(|i| i.key_pressed(egui::Key::Enter)) {
            go = true;
        }
        ui_b.label(egui::RichText::new("Pages work when their video is openly exposed; protected or signed-in video does not. Only download what you have the right to use.").font(theme::caption()).color(t.text_2));
        match kit::dialog_footer(ui, "Download", Some("Cancel"), false) {
            kit::Choice::Primary => go = true,
            kit::Choice::Secondary => cancel = true,
            kit::Choice::None => {}
        }
    });
    let url = link.trim().to_owned();
    if go {
        match crate::agent::download::check_url(&url) {
            Ok(()) => {
                let (tx, rx) = mpsc::channel();
                a.live.push(ToolCall { tool: "download".into(), args: serde_json::json!({ "url": url }), reply: tx, epoch: 0 });
                a.downloads.push((url.clone(), rx));
                a.link = None;
                kit::push(ctx, crate::i18n::tf(format!("Downloading {}…", one_line(url.rsplit('/').next().unwrap_or(&url), 40))), None, Some(Icon::DownloadSimple));
            }
            Err(e) => {
                kit::push(ctx, e, None, Some(Icon::Warning));
            }
        }
    } else if out.close || cancel {
        a.link = None;
    }
}

/// Opens the Assistant's panel (where it is docked, in its own window, or
/// where it goes by default) on the conversation with the input focused (⌘K).
pub fn open(app: &mut StudioApp) {
    let p = crate::dock::Panel::Ai;
    if app.editor.dock.is_popped(p) {
        app.editor.focus_window = Some(p);
    } else {
        app.editor.open_panel(p);
    }
    if app.editor.rail == super::library::RailItem::Ai && app.editor.library_chip[super::library::RailItem::Ai.index()] == 0 {
        app.editor.library_chip[super::library::RailItem::Ai.index()] = 1;
    }
    if let Some(a) = app.agent.as_mut() {
        a.focus_input = true;
    }
}


// ------------------------------------------------------------------ look

/// Text in the conversation.
fn chat_font() -> FontId {
    FontId::new(13.0, FontFamily::Name(theme::MEDIUM.into()))
}
fn chat_bold() -> FontId {
    FontId::new(13.0, FontFamily::Name(theme::SEMIBOLD.into()))
}
fn small_bold() -> FontId {
    FontId::new(11.5, FontFamily::Name(theme::SEMIBOLD.into()))
}
fn title_font() -> FontId {
    FontId::new(15.0, FontFamily::Name(theme::SEMIBOLD.into()))
}

const PAD: f32 = 14.0;
const BUBBLE_R: u8 = 12;
const CARD_R: u8 = 10;

/// A card a shade off the panel.
fn card_fill(t: &Tokens) -> Color32 {
    theme::mix(t.panel, t.raised, 0.45)
}
/// The inside of a card, a shade deeper.
fn well_fill(t: &Tokens) -> Color32 {
    theme::mix(t.panel, t.ground, 0.55)
}

/// The sparkle mark: the Assistant's face.
fn mark(p: &egui::Painter, rect: Rect, t: &Tokens) {
    p.rect_filled(rect, CornerRadius::same((rect.width() * 0.3) as u8), t.accent_soft);
    icons::paint(p, rect.shrink(rect.width() * 0.22), Icon::Sparkle, rect.width() * 0.56, t.accent);
}

/// The conversation, in `r`.
pub fn show(app: &mut StudioApp, ui: &mut egui::Ui, r: Rect) {
    if app.agent.is_none() {
        app.agent = Some(AgentUi::new(ui.ctx()));
    }
    let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(r).id_salt("agent-panel"));
    ui.set_clip_rect(r.intersect(ui.clip_rect()));
    let ui = &mut ui;
    if !app.prefs.ai_cloud_ok {
        let inner = r.shrink2(Vec2::new(PAD + 4.0, PAD));
        let mut iui = ui.new_child(egui::UiBuilder::new().max_rect(inner).id_salt("agent-welcome"));
        egui::ScrollArea::vertical().auto_shrink([false, false]).show_v(&mut iui, |ui| welcome(app, ui));
        return;
    }
    let header = Rect::from_min_size(r.min, Vec2::new(r.width(), 44.0));
    header_bar(app, ui, header);
    // Chat and the tools it can run: two tabs of the one Assistant panel.
    let view_id = egui::Id::new("assistant-view");
    let mut view: usize = ui.ctx().data(|d| d.get_temp(view_id)).unwrap_or(0);
    let tabs_r = Rect::from_min_size(egui::pos2(r.left() + PAD, header.bottom()), Vec2::new(r.width() - 2.0 * PAD, 32.0));
    let mut tui = ui.new_child(egui::UiBuilder::new().max_rect(tabs_r).id_salt("assistant-tabs"));
    if kit::tab_bar(&mut tui, "assistant-view-tabs", &mut view, &["Chat", "Tools"]) {
        ui.ctx().data_mut(|d| d.insert_temp(view_id, view));
    }
    if view == 1 {
        let body = Rect::from_min_max(egui::pos2(r.left() + PAD, tabs_r.bottom() + 4.0), egui::pos2(r.right() - PAD, r.bottom() - PAD));
        let mut bui = ui.new_child(egui::UiBuilder::new().max_rect(body).id_salt("assistant-tools"));
        bui.set_clip_rect(body.intersect(ui.clip_rect()));
        tools_tab(app, &mut bui);
        return;
    }
    let header = Rect::from_min_size(r.min, Vec2::new(r.width(), 44.0 + 32.0));
    let strip = Rect::from_min_size(egui::pos2(r.left(), header.bottom()), Vec2::new(r.width(), 36.0));
    tab_strip(app, ui, strip);
    let header = header.union(strip);

    let a = app.agent.as_ref().expect("agent");
    let composer_h = 104.0 + if a.attachments.is_empty() { 0.0 } else { 34.0 };
    let ask_h = if a.asks.is_empty() { 0.0 } else { approval_height(ui, a, r.width() - 2.0 * PAD) + 10.0 };
    let bottom = r.bottom() - PAD + 4.0;
    let composer = Rect::from_min_max(egui::pos2(r.left() + PAD, bottom - composer_h), egui::pos2(r.right() - PAD, bottom));
    let ask = Rect::from_min_max(egui::pos2(composer.left(), composer.top() - ask_h), egui::pos2(composer.right(), composer.top() - 10.0));
    // A quiet line for the token counts, just above the composer.
    let c = &a.tabs[a.active];
    let usage_h = if c.session_usage.is_empty() { 0.0 } else { USAGE_H };
    let usage = Rect::from_min_max(egui::pos2(composer.left(), composer.top() - ask_h - usage_h), egui::pos2(composer.right(), composer.top() - ask_h));
    let convo = Rect::from_min_max(egui::pos2(r.left(), header.bottom()), egui::pos2(r.right(), composer.top() - ask_h - usage_h - 6.0));
    let (request, session) = (c.request_usage, c.session_usage);

    let mut cui = ui.new_child(egui::UiBuilder::new().max_rect(convo).id_salt("agent-convo"));
    conversation(app, &mut cui);
    if ask_h > 0.0 {
        let mut aui = ui.new_child(egui::UiBuilder::new().max_rect(ask).id_salt("agent-ask"));
        approval(app, &mut aui, ask);
    }
    if usage_h > 0.0 {
        usage_line(ui, usage, request, session);
    }
    let mut iui = ui.new_child(egui::UiBuilder::new().max_rect(composer).id_salt("agent-composer"));
    composer_card(app, &mut iui, composer);
}


/// The AI tools the Assistant can run for you, one row each. Tools that need
/// a model say so; tools that do nothing yet are not listed.
fn tools_tab(app: &mut StudioApp, ui: &mut egui::Ui) {
    use crate::services::ai_run::Action;
    let t = theme::tokens(ui.ctx());
    let sel_clip = app.editor.selection.first().copied();
    egui::ScrollArea::vertical().auto_shrink([false, false]).id_salt("assistant-tools-scroll").show_v(ui, |ui| {
        ui.label(egui::RichText::new(crate::i18n::shown("Run a tool on the selected clip, or ask the Assistant in Chat.")).font(theme::caption()).color(t.text_2));
        ui.add_space(theme::space::SM);
        for tool in crate::view::AI_TOOLS {
            let action = Action::for_tool(tool.name);
            let cut = cfg!(feature = "render") && matches!(tool.name, "Remove background" | "Smooth slow motion");
            if action.is_none() && !cut {
                continue;
            }
            let ready = tool_feature(tool.name).map(crate::services::models::feature).is_some_and(|r| r.ready);
            let running = action.is_some_and(|a| super::library::ai_runner().lock().unwrap_or_else(|e| e.into_inner()).is_running(a));
            #[cfg(feature = "render")]
            let running = running || (cut && (super::cutout::busy() || super::slowmo::busy()));
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.set_max_width((ui.available_width() - 110.0).max(80.0));
                    ui.label(egui::RichText::new(crate::i18n::shown(tool.name)).font(theme::control_medium()).color(t.text));
                    ui.label(egui::RichText::new(crate::i18n::shown(tool.blurb.trim_end_matches('.'))).font(theme::caption()).color(t.text_2));
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_enabled_ui(ready && !running, |ui| {
                        let label = if running { "Running" } else if ready { "Run" } else { "Needs model" };
                        let b = Button::secondary(label).icon(Icon::PlayFill).busy(running).compact().show(ui);
                        let tip = if !ready { "Install the model in Settings ▸ Models".to_owned() } else { crate::i18n::tf(format!("Run on the {}", if sel_clip.is_some() { "selected clip" } else { "first clip" })) };
                        let b = kit::tip(b, &tip, None);
                        if b.clicked() {
                            let ctx = ui.ctx().clone();
                            let res = if let Some(a) = action {
                                let sel = app.editor.selection.clone();
                                super::library::ai_runner().lock().unwrap_or_else(|e| e.into_inner()).start(&app.doc, a, &sel).map_err(|e| e.0)
                            } else {
                                run_cut_tool(app, &ctx, tool.name, sel_clip)
                            };
                            match res {
                                Ok(m) => kit::push(&ctx, m, None, Some(Icon::Sparkle)),
                                Err(e) => kit::push(&ctx, e, None, Some(Icon::Warning)),
                            };
                        }
                    });
                });
            });
            ui.add_space(theme::space::SM);
        }
    });
}

fn tool_feature(name: &str) -> Option<studio_ai::features::Feature> {
    use studio_ai::features::Feature as F;
    Some(match name {
        "Auto captions" => F::Captions,
        "Remove background" => F::RemoveBackground,
        "Voice cleanup" => F::VoiceCleanup,
        "Beat markers" => F::BeatSync,
        "Smooth slow motion" => F::SmoothSlowMotion,
        _ => return None,
    })
}

#[cfg(feature = "render")]
fn run_cut_tool(app: &mut StudioApp, ctx: &egui::Context, name: &str, sel: Option<u32>) -> Result<String, String> {
    let video = |id: &u32| app.doc.project.clip(*id).is_some_and(|c| matches!(c.kind, crate::view::ClipKind::Video));
    let target = sel.filter(video).or_else(|| app.doc.project.clips.iter().filter(|c| matches!(c.kind, crate::view::ClipKind::Video)).min_by_key(|c| (c.start, c.lane)).map(|c| c.id));
    let id = target.ok_or_else(|| "There is no video clip to work on".to_owned())?;
    if name == "Remove background" { super::cutout::start_people(app, ctx, id) } else { super::slowmo::start(app, ctx, id, crate::services::slowmo::Method::Flow) }
}

#[cfg(not(feature = "render"))]
fn run_cut_tool(_: &mut StudioApp, _: &egui::Context, _: &str, _: Option<u32>) -> Result<String, String> {
    Err("This tool is not available in this build".into())
}

/// The commands behind "/": every tool the Assistant uses, with the gist of
/// what it does.
fn slash_commands() -> &'static [(String, String)] {
    static LIST: std::sync::OnceLock<Vec<(String, String)>> = std::sync::OnceLock::new();
    LIST.get_or_init(|| {
        crate::agent::mcp::tools()
            .iter()
            .filter_map(|t| {
                let name = t["name"].as_str()?.to_owned();
                if matches!(name.as_str(), "edit_schema" | "open" | "new_project" | "export_status" | "ai") {
                    return None;
                }
                let d = t["description"].as_str().unwrap_or("");
                let gist = d.split([':', ';', '(']).next().unwrap_or(d).split(". ").next().unwrap_or(d).trim().trim_end_matches(['.', ',']).to_owned();
                Some((name, gist))
            })
            .collect()
    })
}

const USAGE_H: f32 = 18.0;

/// The token count of this reply as it climbs, and the conversation's
/// running total, in the caption voice. Never money.
fn usage_line(ui: &mut egui::Ui, r: Rect, request: Usage, session: Usage) {
    let t = theme::tokens(ui.ctx());
    let text = if request.is_empty() {
        crate::i18n::tf(format!("{} used", usage::short(session.total())))
    } else {
        crate::i18n::tf(format!("{} this reply · {} this chat", usage::short(request.total()), usage::short(session.total())))
    };
    ui.painter().text(egui::pos2(r.right() - 2.0, r.center().y), Align2::RIGHT_CENTER, text, theme::caption(), t.text_2);
}

fn header_bar(app: &mut StudioApp, ui: &mut egui::Ui, r: Rect) {
    let t = theme::tokens(ui.ctx());
    let p = ui.painter().clone();
    let cy = r.center().y;
    mark(&p, Rect::from_center_size(egui::pos2(r.left() + PAD + 12.0, cy), Vec2::splat(24.0)), &t);
    let title_x = r.left() + PAD + 32.0;
    let tg = p.layout_no_wrap("Assistant".into(), title_font(), t.text);
    let tw = tg.size().x;
    p.galley(egui::pos2(title_x, cy - tg.size().y / 2.0), tg, t.text);

    // Who answers: a chip that opens Settings ▸ AI.
    let who = provider_name(&app.prefs);
    // As wide as the name, at most to the panel's edge (then cut short).
    let ww = (kit::text_width(ui, &who, theme::caption()) + 26.0).min(r.right() - PAD - (title_x + tw + 10.0)).max(40.0);
    let chip = Rect::from_min_size(egui::pos2(title_x + tw + 10.0, cy - 11.0), Vec2::new(ww, 22.0));
    let resp = ui.interact(chip, ui.scope_id().with("agent-who"), Sense::click());
    let a = app.agent.as_ref().expect("agent");
    let fill = if resp.hovered() { t.raised } else { well_fill(&t) };
    p.rect_filled(chip, CornerRadius::same(11), fill);
    let dot = if a.busy() { t.warn } else { t.accent };
    p.circle_filled(egui::pos2(chip.left() + 11.0, cy), 3.0, dot);
    let who_cut = kit::paint_text_clipped(&p, egui::pos2(chip.left() + 19.0, cy), Align2::LEFT_CENTER, &who, theme::caption(), t.text_2, chip.width() - 26.0);
    let resp = kit::tip_if_cut(resp, who_cut, &who);
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text("Change who answers in Settings ▸ AI");
    if resp.clicked() {
        app.settings.page = 4;
        app.overlay = crate::app::Overlay::Settings;
    }
    // Crew or solo: a chip that switches.
    let crew = app.prefs.ai_crew;
    let label = if crew { "Crew" } else { "Solo" };
    let cw = kit::text_width(ui, label, theme::caption()) + 30.0;
    let cr = Rect::from_min_size(egui::pos2(chip.right() + 6.0, cy - 11.0), Vec2::new(cw, 22.0));
    if cr.right() < r.right() - PAD {
        let cres = ui.interact(cr, ui.scope_id().with("agent-crew"), Sense::click());
        p.rect_filled(cr, CornerRadius::same(11), if crew { t.accent_soft } else if cres.hovered() { t.raised } else { well_fill(&t) });
        icons::paint(&p, Rect::from_center_size(egui::pos2(cr.left() + 12.0, cy), Vec2::splat(11.0)), if crew { Icon::FilmSlate } else { Icon::Sparkle }, 11.0, if crew { t.accent } else { t.text_2 });
        p.text(egui::pos2(cr.left() + 22.0, cy), Align2::LEFT_CENTER, label, theme::caption(), if crew { t.accent } else { t.text_2 });
        let tip = if crew {
            "Crew: a director plans from your brief, an editor, animator, sound designer, colourist and VFX artist work in parallel, and the director reviews. Click for one AI doing everything (faster for small jobs)."
        } else {
            "Solo: one AI does everything. Click for a crew (a director and specialists working in parallel) for bigger jobs."
        };
        if cres.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(tip).clicked() {
            app.prefs.ai_crew = !crew;
        }
    }
}

/// The conversations as tabs, with + for a new one.
fn tab_strip(app: &mut StudioApp, ui: &mut egui::Ui, r: Rect) {
    let t = theme::tokens(ui.ctx());
    let a = app.agent.as_ref().expect("agent");
    let n = a.tabs.len();
    let plus_w = 28.0;
    let avail = r.width() - 2.0 * PAD - plus_w - 6.0;
    let titles: Vec<String> = a.tabs.iter().map(Conversation::title).collect();
    let natural: Vec<f32> = titles.iter().map(|s| kit::text_width(ui, s, theme::caption()) + 46.0).collect();
    let total: f32 = natural.iter().sum::<f32>() + 4.0 * n as f32;
    let scale = if total > avail { avail / total } else { 1.0 };
    let widths: Vec<f32> = natural.iter().map(|w| (w * scale).max(54.0)).collect();
    let needed = 2.0 * PAD + widths.iter().map(|w| w + 4.0).sum::<f32>() + plus_w + 2.0;
    if needed <= r.width() {
        tab_cells(app, ui, r, &titles, &widths);
    } else {
        // Too many to fit even narrowed: the tabs scroll sideways.
        let mut strip = ui.new_child(egui::UiBuilder::new().max_rect(r).id_salt("agent-tabs"));
        strip.set_clip_rect(r.intersect(ui.clip_rect()));
        kit::scroll_strip(&mut strip, "agent-tabs-scroll", |ui| {
            let (area, _) = ui.allocate_exact_size(Vec2::new(needed, r.height()), Sense::hover());
            tab_cells(app, ui, area, &titles, &widths);
        });
    }
    ui.painter().hline(r.left()..=r.right(), r.bottom() - 0.5, Stroke::new(1.0, t.line));
}

fn tab_cells(app: &mut StudioApp, ui: &mut egui::Ui, r: Rect, titles: &[String], widths: &[f32]) {
    let t = theme::tokens(ui.ctx());
    let p = ui.painter().clone();
    let a = app.agent.as_mut().expect("agent");
    let n = a.tabs.len();
    let plus_w = 28.0;
    let mut x = r.left() + PAD;
    let cy = r.center().y;
    let mut pick = None;
    let mut close = None;
    for (i, title) in titles.iter().enumerate() {
        let w = widths[i];
        let tr = Rect::from_min_size(egui::pos2(x, cy - 13.0), Vec2::new(w, 26.0));
        let resp = ui.interact(tr, ui.scope_id().with(("agent-tab", i)), Sense::click());
        let on = i == a.active;
        let hover = resp.hovered();
        let fill = if on { card_fill(&t) } else if hover { theme::alpha(t.raised, 0.6) } else { Color32::TRANSPARENT };
        p.rect_filled(tr, CornerRadius::same(8), fill);
        if on {
            p.rect_filled(Rect::from_min_size(egui::pos2(tr.left() + 10.0, tr.bottom() - 2.0), Vec2::new(tr.width() - 20.0, 2.0)), CornerRadius::same(1), t.accent);
        }
        // Working here: a spinner; otherwise a dot.
        let dc = egui::pos2(tr.left() + 13.0, cy);
        if a.running == Some(i) && !a.gate.paused() {
            kit::anim::spinner(&p, dc, 4.5, t.accent);
        } else {
            p.circle_filled(dc, 2.5, if on { t.accent } else { t.text_2 });
        }
        let show_x = (on || hover) && (n > 1 || !a.tabs[i].entries.is_empty());
        let text_w = tr.width() - 26.0 - if show_x { 20.0 } else { 8.0 };
        kit::paint_text_clipped(&p, egui::pos2(tr.left() + 24.0, cy), Align2::LEFT_CENTER, title, theme::caption(), if on { t.text } else { t.text_2 }, text_w);
        if show_x {
            let xr = Rect::from_center_size(egui::pos2(tr.right() - 12.0, cy), Vec2::splat(16.0));
            let xresp = ui.interact(xr, ui.scope_id().with(("agent-tab-x", i)), Sense::click());
            if xresp.hovered() {
                p.circle_filled(xr.center(), 8.0, t.raised);
            }
            let d = 3.2;
            let c = xr.center();
            let col = if xresp.hovered() { t.text } else { t.text_2 };
            p.line_segment([c + Vec2::new(-d, -d), c + Vec2::new(d, d)], Stroke::new(1.3, col));
            p.line_segment([c + Vec2::new(d, -d), c + Vec2::new(-d, d)], Stroke::new(1.3, col));
            if xresp.on_hover_text(if a.running == Some(i) { "Close (stops its request)" } else { "Close this conversation" }).clicked() {
                close = Some(i);
            }
        }
        if resp.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(title).clicked() && close.is_none() {
            pick = Some(i);
        }
        x = tr.right() + 4.0;
    }
    let br = Rect::from_center_size(egui::pos2(x + plus_w / 2.0 + 2.0, cy), Vec2::splat(plus_w - 2.0));
    let mut bui = ui.new_child(egui::UiBuilder::new().max_rect(br).id_salt("agent-new-tab"));
    let new_tab = IconButton::new(Icon::Plus, "New conversation (a new tab; the others stay)").show_at(&mut bui, br).clicked();
    if let Some(i) = pick {
        a.active = i;
        a.focus_input = true;
    }
    if new_tab {
        a.new_tab();
    }
    if let Some(i) = close {
        if let Some(a) = app.agent.as_mut() {
            a.close_tab(i, &mut app.doc);
        }
    }
}

// ------------------------------------------------------------------ welcome

fn welcome(app: &mut StudioApp, ui: &mut egui::Ui) {
    let t = theme::tokens(ui.ctx());
    let w = ui.available_width().min(460.0);
    ui.add_space(6.0);
    let (r, _) = ui.allocate_exact_size(Vec2::new(w, 40.0), Sense::hover());
    mark(ui.painter(), Rect::from_min_size(r.min, Vec2::splat(40.0)), &t);
    ui.add_space(10.0);
    ui.label(egui::RichText::new(crate::i18n::shown("Meet the Assistant")).font(FontId::new(19.0, FontFamily::Name(theme::BOLD.into()))).color(t.text));
    ui.add_space(4.0);
    ui.label(egui::RichText::new(crate::i18n::shown("Say what you want in plain words. It cuts, captions, scores and checks your video while you watch.")).font(chat_font()).color(t.text_2));
    ui.add_space(12.0);
    if Button::primary("Turn on the Assistant").icon(Icon::Sparkle).show(ui).clicked() {
        app.prefs.ai_cloud_ok = true;
        if let Some(a) = app.agent.as_mut() {
            a.focus_input = true;
        }
    }
    ui.add_space(4.0);
    ui.label(egui::RichText::new(crate::i18n::shown("Choose Claude, ChatGPT, Claude Code or Codex in Settings ▸ AI.")).font(theme::caption()).color(t.text_2));
    ui.add_space(18.0);
    for (icon, head, body) in [
        (Icon::MagicWand, "Live on your timeline", "Every change appears as it is made, and you can keep editing alongside it."),
        (Icon::ArrowUUpLeft, "Its own Undo", "Undo this request takes back what it did for a request; Undo in the editor only takes back your own edits, so you can keep working while it does."),
        (Icon::Info, "What is sent", "Your requests, descriptions of the project and the frames it looks at go to Claude (Anthropic) or ChatGPT (OpenAI). Media files are never uploaded."),
        (Icon::Cpu, "Local tools stay local", "Captions, background removal and voice cleanup still run on this computer."),
    ] {
        feature_row(ui, w, icon, head, body, &t);
        ui.add_space(8.0);
    }
}

fn feature_row(ui: &mut egui::Ui, w: f32, icon: Icon, head: &str, body: &str, t: &Tokens) {
    let text_w = w - 40.0;
    let hg = ui.painter().layout(head.into(), chat_bold(), t.text, text_w);
    let bg = ui.painter().layout(body.into(), theme::body(), t.text_2, text_w);
    let h = hg.size().y + 2.0 + bg.size().y;
    let (r, _) = ui.allocate_exact_size(Vec2::new(w, h.max(28.0)), Sense::hover());
    let p = ui.painter();
    let ir = Rect::from_min_size(r.min, Vec2::splat(28.0));
    p.rect_filled(ir, CornerRadius::same(8), card_fill(t));
    icons::paint(p, ir.shrink(7.0), icon, 14.0, t.accent);
    let hy = hg.size().y;
    p.galley(egui::pos2(r.left() + 40.0, r.top()), hg, t.text);
    p.galley(egui::pos2(r.left() + 40.0, r.top() + hy + 2.0), bg, t.text_2);
}

// ------------------------------------------------------------- conversation

fn conversation(app: &mut StudioApp, ui: &mut egui::Ui) {
    let t = theme::tokens(ui.ctx());
    let mut stop_clicked = false;
    let mut undo_request = None;
    let mut pause_clicked = false;
    let a = app.agent.as_mut().expect("agent");
    let paused = a.gate.paused();
    let tab = a.active;
    let here_running = a.running == Some(tab);
    let c = &mut a.tabs[tab];
    egui::ScrollArea::vertical().id_salt(("agent-convo-scroll", tab)).stick_to_bottom(true).auto_shrink([false, false]).show_v(ui, |ui| {
        egui::Frame::new().inner_margin(egui::Margin { left: PAD as i8, right: PAD as i8, top: 12, bottom: 12 }).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            let w = ui.available_width();
            if c.entries.is_empty() && !a.queued.iter().any(|q| q.0 == tab) {
                empty_state(&mut a.input, &mut a.focus_input, ui, w, &t);
                return;
            }
            // After the person speaks, Studio's answer gets its name once.
            let mut named = false;
            let mut i = 0;
            // The steps of the request under way are told once, in one card.
            let mut told_until = 0;
            let plan_title = c.entries.iter().rev().find_map(|e| if let Entry::Plan(p) = e { p.steps.iter().find(|s| s.status == Status::Running).map(|s| s.title.clone()) } else { None });
            while i < c.entries.len() {
                let is_reply = matches!(c.entries[i], Entry::Said(_) | Entry::Tool { .. } | Entry::Plan(_));
                if is_reply && !named {
                    ui.add_space(14.0);
                    speaker(ui, &t);
                    ui.add_space(6.0);
                    named = true;
                }
                match &mut c.entries[i] {
                    Entry::You(s) => {
                        ui.add_space(if i == 0 { 2.0 } else { 16.0 });
                        you_bubble(ui, s, w, &t);
                        named = false;
                    }
                    Entry::Said(s) => {
                        ui.add_space(4.0);
                        markdown(ui, s.trim(), w, &t);
                        ui.add_space(4.0);
                    }
                    Entry::Tool { .. } => {
                        // All of a request's steps share one card, where the first one is.
                        if i >= told_until {
                            let end = (i..c.entries.len()).find(|&j| matches!(c.entries[j], Entry::You(_))).unwrap_or(c.entries.len());
                            told_until = end;
                            let idx: Vec<usize> = (i..end).filter(|&j| matches!(c.entries[j], Entry::Tool { .. })).collect();
                            ui.add_space(6.0);
                            if summary_card(ui, &c.entries, &idx, &mut c.enlarged, a.show_steps, w, &t) {
                                a.show_steps = !a.show_steps;
                                let (ctx, on) = (ui.ctx().clone(), a.show_steps);
                                ctx.data_mut(|d| d.insert_persisted(egui::Id::new("agent-show-steps"), on));
                            }
                            ui.add_space(6.0);
                        }
                    }
                    Entry::Note(n) => {
                        if !note_is_harness(n) {
                            ui.add_space(10.0);
                            note_row(ui, n, w, &t);
                        }
                    }
                    Entry::Done { request } => {
                        if app.doc.request_undoable(*request) {
                            ui.add_space(6.0);
                            if ui.button(crate::i18n::shown("Undo this request")).on_hover_text("Takes back what the AI changed for this request; what you changed since stays.").clicked() {
                                undo_request = Some(*request);
                            }
                        }
                    }
                    Entry::Error(e) => {
                        ui.add_space(8.0);
                        error_card(ui, e, w, &t);
                    }
                    Entry::Plan(plan) => {
                        ui.add_space(6.0);
                        plan_card(ui, plan, w, &t);
                        ui.add_space(6.0);
                    }
                    Entry::Correction(s) => {
                        ui.add_space(12.0);
                        correction_bubble(ui, s, w, &t);
                    }
                    Entry::Command { line, result, pictures, .. } => {
                        ui.add_space(12.0);
                        command_card(ui, line, result, pictures, &mut c.enlarged, w, &t);
                        named = false;
                    }
                }
                i += 1;
            }
            if a.busy && here_running && a.asks.is_empty() {
                ui.add_space(10.0);
                (stop_clicked, pause_clicked) = working_row(ui, &c.entries, plan_title.as_deref(), a.turn_started, paused, &t);
            } else if a.busy && !here_running {
                ui.add_space(10.0);
                note_row(ui, "Another tab's request is running; a request sent here waits for it.", w, &t);
            } else if a.live.busy() && !a.busy {
                ui.add_space(10.0);
                outside_row(ui, &t);
            }
            // Requests waiting their turn.
            for (_, q) in a.queued.iter().filter(|q| q.0 == tab) {
                ui.add_space(10.0);
                queued_bubble(ui, q, w, &t);
            }
        });
    });
    if let Some(request) = undo_request {
        let ctx = ui.ctx().clone();
        match app.doc.undo_request(request) {
            Ok(_) => kit::push(&ctx, crate::i18n::shown("Took back the AI's request"), None, Some(Icon::ArrowUUpLeft)),
            Err(e) => kit::push(&ctx, e, None, Some(Icon::Warning)),
        };
    }
    if stop_clicked {
        if let Some(a) = app.agent.as_mut() {
            a.stop(&mut app.doc);
        }
    } else if pause_clicked {
        if let Some(a) = app.agent.as_mut() {
            a.toggle_pause();
        }
    }
}

fn speaker(ui: &mut egui::Ui, t: &Tokens) {
    let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 20.0), Sense::hover());
    let p = ui.painter();
    mark(p, Rect::from_min_size(r.min, Vec2::splat(20.0)), t);
    p.text(egui::pos2(r.left() + 28.0, r.center().y), Align2::LEFT_CENTER, "Zornov", small_bold(), t.text_2);
}

fn you_bubble(ui: &mut egui::Ui, text: &str, w: f32, t: &Tokens) {
    let max = (w * 0.82).max(120.0);
    // Lines starting "> " quote what the person marked on the picture.
    let quotes: Vec<&str> = text.lines().filter_map(|l| l.strip_prefix("> ")).collect();
    let body: String = text.lines().filter(|l| !l.starts_with("> ")).collect::<Vec<_>>().join("\n");
    let qg: Vec<_> = quotes.iter().map(|q| ui.painter().layout(q.to_string(), theme::caption(), t.text_2, max - 40.0)).collect();
    let g = ui.painter().layout(body.trim().to_owned(), chat_font(), t.text, max - 24.0);
    let qh: f32 = qg.iter().map(|g| g.size().y + 4.0).sum::<f32>() + if qg.is_empty() { 0.0 } else { 6.0 };
    let qw = qg.iter().map(|g| g.size().x + 16.0).fold(0.0, f32::max);
    let size = Vec2::new(g.size().x.max(qw) + 24.0, g.size().y + qh + 16.0);
    let (row, _) = ui.allocate_exact_size(Vec2::new(w, size.y), Sense::hover());
    let r = Rect::from_min_size(egui::pos2(row.right() - size.x, row.top()), size);
    let p = ui.painter();
    p.rect_filled(r, CornerRadius { nw: BUBBLE_R, ne: BUBBLE_R, sw: BUBBLE_R, se: 4 }, theme::mix(t.raised, t.accent, 0.10));
    let mut y = r.top() + 8.0;
    if !qg.is_empty() {
        let top = y;
        for g in qg {
            let h = g.size().y;
            p.galley(egui::pos2(r.left() + 22.0, y), g, t.text_2);
            y += h + 4.0;
        }
        p.rect_filled(Rect::from_min_max(egui::pos2(r.left() + 12.0, top), egui::pos2(r.left() + 14.5, y - 4.0)), CornerRadius::same(1), theme::alpha(t.accent, 0.7));
        y += 6.0;
    }
    p.galley(egui::pos2(r.left() + 12.0, y), g, t.text);
}

fn queued_bubble(ui: &mut egui::Ui, text: &str, w: f32, t: &Tokens) {
    let max = (w * 0.82).max(120.0);
    let g = ui.painter().layout(text.to_owned(), chat_font(), theme::alpha(t.text, 0.6), max - 24.0);
    let size = g.size() + Vec2::new(24.0, 16.0);
    let (row, _) = ui.allocate_exact_size(Vec2::new(w, size.y + 16.0), Sense::hover());
    let r = Rect::from_min_size(egui::pos2(row.right() - size.x, row.top()), size);
    let p = ui.painter();
    p.rect_stroke(r, CornerRadius { nw: BUBBLE_R, ne: BUBBLE_R, sw: BUBBLE_R, se: 4 }, Stroke::new(1.0, t.line), egui::StrokeKind::Inside);
    p.galley(r.min + Vec2::new(12.0, 8.0), g, t.text);
    p.text(egui::pos2(r.right(), r.bottom() + 8.0), Align2::RIGHT_CENTER, crate::i18n::shown("Sends when this one finishes"), theme::caption(), t.text_2);
}

fn empty_state(input: &mut String, focus_input: &mut bool, ui: &mut egui::Ui, w: f32, t: &Tokens) {
    ui.add_space(22.0);
    let (r, _) = ui.allocate_exact_size(Vec2::new(w, 44.0), Sense::hover());
    mark(ui.painter(), Rect::from_center_size(egui::pos2(r.center().x, r.center().y), Vec2::splat(44.0)), t);
    ui.add_space(12.0);
    centred(ui, "What should we make?", FontId::new(17.0, FontFamily::Name(theme::BOLD.into())), t.text, w);
    ui.add_space(4.0);
    centred(ui, "Ask in plain words. Changes appear on your timeline as they happen.", theme::body(), t.text_2, w);
    ui.add_space(18.0);
    let ideas = [
        (Icon::Scissors, "Cut the pauses", "and add captions"),
        (Icon::DeviceMobile, "Make it vertical", "for Reels, faces kept in frame"),
        (Icon::MusicNotes, "Score it", "upbeat, under the voice"),
        (Icon::CheckCircle, "Check it", "and fix what you can"),
    ];
    let cols = if w > 380.0 { 2 } else { 1 };
    let gap = 8.0;
    let cw = (w - gap * (cols as f32 - 1.0)) / cols as f32;
    for row in ideas.chunks(cols) {
        let (rr, _) = ui.allocate_exact_size(Vec2::new(w, 58.0), Sense::hover());
        for (k, (icon, head, rest)) in row.iter().enumerate() {
            let cr = Rect::from_min_size(egui::pos2(rr.left() + k as f32 * (cw + gap), rr.top()), Vec2::new(cw, 58.0));
            let resp = ui.interact(cr, ui.scope_id().with(("agent-idea", *head)), Sense::click());
            let hover = kit::anim::hover_t(ui.ctx(), resp.id, resp.hovered());
            let p = ui.painter();
            p.rect_filled(cr, CornerRadius::same(CARD_R), theme::mix(card_fill(t), t.raised, hover * 0.7));
            if hover > 0.0 {
                p.rect_stroke(cr, CornerRadius::same(CARD_R), Stroke::new(1.0, theme::alpha(t.accent, 0.35 * hover)), egui::StrokeKind::Inside);
            }
            let ir = Rect::from_min_size(cr.min + Vec2::new(12.0, 13.0), Vec2::splat(16.0));
            icons::paint(p, ir, *icon, 16.0, t.accent);
            p.text(egui::pos2(cr.left() + 38.0, cr.top() + 20.0), Align2::LEFT_CENTER, *head, chat_bold(), t.text);
            kit::paint_text_clipped(p, egui::pos2(cr.left() + 38.0, cr.top() + 38.0), Align2::LEFT_CENTER, rest, theme::caption(), t.text_2, cw - 48.0);
            if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                *input = format!("{head} {rest}");
                *focus_input = true;
            }
        }
        ui.add_space(gap);
    }
    ui.add_space(8.0);
    centred(ui, "Type / for Zornov commands", theme::caption(), t.text_2, w);
}

fn centred(ui: &mut egui::Ui, text: &str, font: FontId, colour: Color32, w: f32) {
    let mut job = LayoutJob::simple(text.to_owned(), font, colour, w);
    job.halign = egui::Align::Center;
    let g = ui.painter().layout_job(job);
    let (r, _) = ui.allocate_exact_size(Vec2::new(w, g.size().y), Sense::hover());
    ui.painter().galley(egui::pos2(r.center().x, r.top()), g, colour);
}

fn note_row(ui: &mut egui::Ui, text: &str, w: f32, t: &Tokens) {
    let g = ui.painter().layout(text.to_owned(), theme::caption(), t.text_2, w - 24.0);
    let (r, _) = ui.allocate_exact_size(Vec2::new(w, g.size().y.max(14.0)), Sense::hover());
    let p = ui.painter();
    icons::paint(p, Rect::from_min_size(r.min + Vec2::new(0.0, 0.5), Vec2::splat(13.0)), Icon::Info, 13.0, t.text_2);
    p.galley(egui::pos2(r.left() + 20.0, r.top()), g, t.text_2);
}

fn error_card(ui: &mut egui::Ui, text: &str, w: f32, t: &Tokens) {
    egui::Frame::new()
        .fill(theme::alpha(t.danger, 0.10))
        .stroke(Stroke::new(1.0, theme::alpha(t.danger, 0.35)))
        .corner_radius(CornerRadius::same(CARD_R))
        .inner_margin(egui::Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.set_width(w - 24.0);
            ui.horizontal_top(|ui| {
                let (ir, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                icons::paint(ui.painter(), ir, Icon::Warning, 15.0, t.danger);
                ui.add_space(6.0);
                // One plain line with the reason; the whole text on hover.
                let first = text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
                ui.add(egui::Label::new(egui::RichText::new(one_line(first, 220)).font(theme::body()).color(t.text)).truncate()).on_hover_text(text);
            });
        });
}

/// A tool's icon in the full step list.
fn step_icon(tool: &str) -> Icon {
    match tool {
        "guide" | "edit_schema" => Icon::Info,
        "summary" | "script" | "notes" => Icon::List,
        "inspect" | "pick" | "canvas_layout" => Icon::Cursor,
        "media" | "make_playable" => Icon::FilmStrip,
        "see" => Icon::Eye,
        "frame" | "contact_sheet" | "match" => Icon::Image,
        "listen" => Icon::Headphones,
        "moments" | "analyse" => Icon::Waveform,
        "catalogue" | "made" | "voices" => Icon::MagnifyingGlass,
        "edit" => Icon::Scissors,
        "undo" => Icon::ArrowUUpLeft,
        "redo" => Icon::ArrowUUpRight,
        "ai" | "generate" => Icon::Sparkle,
        "check" => Icon::CheckCircle,
        "export" | "export_status" => Icon::Export,
        "import" | "download" => Icon::DownloadSimple,
        "save" | "open" | "new_project" => Icon::FolderOpen,
        "save_move" | "save_design" => Icon::Star,
        _ => Icon::MagicWand,
    }
}

/// What a step is, in plain words, for the full step list. Never the
/// call's arguments (those are the detail line, trimmed).
fn step_name(tool: &str) -> String {
    let w = |s: &str| crate::i18n::t(s).into_owned();
    match tool {
        "guide" | "edit_schema" => w("Reading Zornov's guide"),
        "summary" | "script" => w("Reading the timeline"),
        "inspect" | "pick" | "canvas_layout" => w("Inspecting"),
        "media" => w("Checking the media"),
        "notes" => w("Updating its notes"),
        "see" => w("Looking"),
        "frame" | "contact_sheet" => w("Looking at frames"),
        "match" => w("Finding matching shots"),
        "listen" => w("Listening"),
        "moments" | "analyse" => w("Studying the footage"),
        "catalogue" | "made" | "voices" => w("Browsing the library"),
        "edit" => w("Editing"),
        "undo" => w("Undoing"),
        "redo" => w("Redoing"),
        "check" => w("Checking the edit"),
        "export" | "export_status" => w("Exporting"),
        "import" => w("Importing media"),
        "download" => w("Downloading"),
        "generate" => w("Generating"),
        "make_playable" => w("Converting media so it plays"),
        "save" => w("Saving"),
        "open" | "new_project" => w("Opening"),
        "save_move" | "save_design" => w("Saving to your library"),
        _ => w("Working on it"),
    }
}

/// Status lines about the AI's own set-up, not about the person's work.
fn note_is_harness(n: &str) -> bool {
    n.starts_with("The director is") || n.contains("look up Zornov's tools") || n.ends_with("steps left for this request")
}

fn digest_steps<'a>(entries: &'a [Entry], idx: &[usize]) -> Vec<digest::Step<'a>> {
    idx.iter()
        .filter_map(|&j| match &entries[j] {
            Entry::Tool { tool, brief, what, step, result, .. } => Some(digest::Step {
                tool,
                brief,
                what,
                state: match step {
                    Step::Running => digest::State::Running,
                    Step::Done(true) => digest::State::Ok,
                    Step::Done(false) => digest::State::Failed,
                },
                result,
            }),
            _ => None,
        })
        .collect()
}

/// The compact account of one request: what it changed, made, looked at
/// and checked, then a "Show steps" disclosure for the full list. Returns
/// true when the disclosure was clicked.
fn summary_card(ui: &mut egui::Ui, entries: &[Entry], idx: &[usize], enlarged: &mut Option<egui::TextureId>, show_steps: bool, w: f32, t: &Tokens) -> bool {
    let steps = digest_steps(entries, idx);
    let d = digest::digest(&steps);
    let pics = |which: &[usize]| -> Vec<egui::TextureHandle> {
        which.iter().filter_map(|&k| if let Entry::Tool { pictures, .. } = &entries[idx[k]] { Some(pictures.iter().cloned()) } else { None }).flatten().collect()
    };
    let looked = pics(&d.looked);
    let made = pics(&d.made);
    let mut toggled = false;
    egui::Frame::new().fill(card_fill(t)).corner_radius(CornerRadius::same(CARD_R)).inner_margin(egui::Margin::symmetric(8, 6)).show(ui, |ui| {
        ui.set_width(w - 16.0);
        ui.spacing_mut().item_spacing.y = 0.0;
        let rw = w - 16.0;
        // A calm list: the latest few results; older ones fold into one line.
        let cap = 6;
        let skip = d.rows.len().saturating_sub(cap);
        if skip > 0 {
            result_row_ui(ui, rw, Icon::List, &format!("{} {}", skip, crate::i18n::t("earlier changes")), false, t);
        }
        for r in &d.rows[skip..] {
            result_row_ui(ui, rw, r.icon, &r.shown(), true, t);
        }
        if !made.is_empty() {
            thumb_strip(ui, &made, enlarged, rw, t);
        }
        if !looked.is_empty() {
            result_row_ui(ui, rw, Icon::Eye, &crate::i18n::t("Looked at the result"), false, t);
            thumb_strip(ui, &looked, enlarged, rw, t);
        }
        if d.rows.is_empty() && looked.is_empty() && made.is_empty() {
            result_row_ui(ui, rw, Icon::Info, &crate::i18n::t("Reading the project"), false, t);
        }
        ui.add_space(2.0);
        let label = format!("{} {}", if show_steps { "▾" } else { "▸" }, crate::i18n::t(if show_steps { "Hide steps" } else { "Show steps" }));
        let (r, resp) = ui.allocate_exact_size(Vec2::new(rw, theme::caption().size * 1.9), Sense::click());
        ui.painter().text(egui::pos2(r.left() + 4.0, r.center().y), Align2::LEFT_CENTER, crate::i18n::shown(&label), theme::caption(), if resp.hovered() { t.text } else { t.text_2 });
        if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            toggled = true;
        }
        if show_steps {
            full_steps(ui, &steps, entries, idx, rw, t);
        }
    });
    toggled
}

/// One result line: a tick (or icon) and the words.
fn result_row_ui(ui: &mut egui::Ui, rw: f32, icon: Icon, text: &str, done: bool, t: &Tokens) {
    let h = theme::body().size * 1.9;
    let (row, _) = ui.allocate_exact_size(Vec2::new(rw, h), Sense::hover());
    let p = ui.painter();
    let cy = row.center().y;
    let sc = egui::pos2(row.left() + h * 0.5, cy);
    if done {
        p.circle_filled(sc, h * 0.26, theme::alpha(t.accent, 0.18));
        check_glyph(p, sc, h * 0.12, t.accent);
    } else {
        icons::paint(p, Rect::from_center_size(sc, Vec2::splat(h * 0.5)), icon, h * 0.5, t.text_2);
    }
    let _ = icon;
    let lx = row.left() + h + 2.0;
    kit::paint_text_clipped(p, egui::pos2(lx, cy), Align2::LEFT_CENTER, &crate::i18n::shown(text), theme::body(), if done { t.text } else { t.text_2 }, row.right() - lx);
}

/// Pictures in one small strip: as many as fit in a row, then "+n".
fn thumb_strip(ui: &mut egui::Ui, pictures: &[egui::TextureHandle], enlarged: &mut Option<egui::TextureId>, rw: f32, t: &Tokens) {
    let h = theme::body().size * 3.2;
    let gap = 4.0;
    let indent = theme::body().size * 1.9 + 2.0;
    let big = pictures.iter().any(|p| Some(p.id()) == *enlarged);
    if big {
        thumbs(ui, pictures, enlarged, rw, indent, h, t);
        return;
    }
    let mut used = indent;
    let mut fit = 0;
    for tex in pictures {
        let tw = tex.size_vec2().x / tex.size_vec2().y.max(1.0) * h;
        if fit > 0 && used + tw + gap + h > rw {
            break;
        }
        used += tw + gap;
        fit += 1;
    }
    let fit = if fit < pictures.len() { fit.max(1) } else { fit };
    thumbs(ui, &pictures[..fit], enlarged, rw, indent, h, t);
    if fit < pictures.len() {
        ui.add_space(-2.0);
        let (r, _) = ui.allocate_exact_size(Vec2::new(rw, theme::caption().size * 1.6), Sense::hover());
        ui.painter().text(egui::pos2(r.left() + indent, r.center().y), Align2::LEFT_CENTER, format!("+{} more", pictures.len() - fit), theme::caption(), t.text_2);
    }
}

/// The full step list, for debugging: each step by name, time and one
/// trimmed line.
fn full_steps(ui: &mut egui::Ui, steps: &[digest::Step], entries: &[Entry], idx: &[usize], rw: f32, t: &Tokens) {
    let h = theme::body().size * 2.0;
    for (n, &j) in idx.iter().enumerate() {
        let Entry::Tool { tool, step, started, took, .. } = &entries[j] else { continue };
        let line = digest::detail(&steps[n]);
        let (row, _) = ui.allocate_exact_size(Vec2::new(rw, h), Sense::hover());
        let p = ui.painter();
        let cy = row.center().y;
        let sc = egui::pos2(row.left() + h * 0.5, cy);
        match step {
            Step::Running => kit::anim::spinner(p, sc, h * 0.2, t.accent),
            Step::Done(true) => {
                p.circle_filled(sc, h * 0.24, theme::alpha(t.accent, 0.18));
                check_glyph(p, sc, h * 0.11, t.accent);
            }
            Step::Done(false) => {
                p.circle_filled(sc, h * 0.24, theme::alpha(t.danger, 0.18));
                let d = h * 0.09;
                p.line_segment([sc + Vec2::new(-d, -d), sc + Vec2::new(d, d)], Stroke::new(1.5, t.danger));
                p.line_segment([sc + Vec2::new(d, -d), sc + Vec2::new(-d, d)], Stroke::new(1.5, t.danger));
            }
        }
        let secs = took.unwrap_or_else(|| started.elapsed().as_secs_f32());
        let time = if secs < 10.0 { format!("{secs:.1}s") } else { format!("{secs:.0}s") };
        let tw = kit::text_width(ui, &time, theme::mono_small());
        let p = ui.painter();
        let lx = row.left() + h + 2.0;
        p.text(egui::pos2(row.right() - 4.0, cy), Align2::RIGHT_CENTER, &time, theme::mono_small(), t.text_2);
        kit::paint_text_clipped(p, egui::pos2(lx, cy), Align2::LEFT_CENTER, &crate::i18n::shown(&step_name(tool)), theme::caption(), t.text, row.right() - lx - tw - 12.0);
        if !line.is_empty() {
            let (dr, _) = ui.allocate_exact_size(Vec2::new(rw, theme::caption().size * 1.5), Sense::hover());
            kit::paint_text_clipped(ui.painter(), egui::pos2(lx, dr.center().y), Align2::LEFT_CENTER, &line, theme::caption(), t.text_2, dr.right() - lx - 4.0);
        }
        let _ = step_icon(tool);
    }
}

fn check_glyph(p: &egui::Painter, c: Pos2, s: f32, colour: Color32) {
    let pts = vec![c + Vec2::new(-s, 0.1 * s), c + Vec2::new(-0.3 * s, 0.75 * s), c + Vec2::new(s, -0.7 * s)];
    p.add(egui::Shape::line(pts, Stroke::new(1.6, colour)));
}

/// Pictures the agent looked at: small, rounded; a click shows one large.
fn thumbs(ui: &mut egui::Ui, pictures: &[egui::TextureHandle], enlarged: &mut Option<egui::TextureId>, w: f32, indent: f32, h: f32, t: &Tokens) {
    let big = pictures.iter().find(|p| Some(p.id()) == *enlarged);
    ui.add_space(2.0);
    if let Some(tex) = big {
        let size = tex.size_vec2();
        let s = ((w - indent - 8.0) / size.x.max(1.0)).min(1.0);
        let (r, resp) = ui.allocate_exact_size(Vec2::new(w, size.y * s + 6.0), Sense::click());
        let ir = Rect::from_min_size(egui::pos2(r.left() + indent, r.top()), size * s);
        egui::Image::new(tex).corner_radius(8).paint_at(ui, ir);
        if resp.on_hover_cursor(egui::CursorIcon::ZoomOut).on_hover_text(crate::i18n::shown("Click to make it small")).clicked() {
            *enlarged = None;
        }
        return;
    }
    // More pictures than one row holds: they wrap onto the next rows.
    let widths: Vec<f32> = pictures.iter().map(|tex| (tex.size_vec2().x / tex.size_vec2().y.max(1.0) * h).min(w - indent - 8.0)).collect();
    let mut at = Vec::with_capacity(widths.len());
    let (mut cx, mut cy) = (indent, 0.0f32);
    for tw in &widths {
        if cx > indent && cx + tw > w {
            cx = indent;
            cy += h + 6.0;
        }
        at.push(egui::pos2(cx, cy));
        cx += tw + 6.0;
    }
    let (row, _) = ui.allocate_exact_size(Vec2::new(w, cy + h + 8.0), Sense::hover());
    for ((tex, tw), p) in pictures.iter().zip(&widths).zip(&at) {
        let r = Rect::from_min_size(row.min + p.to_vec2(), Vec2::new(*tw, h));
        let resp = ui.interact(r, ui.scope_id().with(("agent-thumb", tex.id())), Sense::click());
        egui::Image::new(tex).corner_radius(6).paint_at(ui, r);
        if resp.hovered() {
            ui.painter().rect_stroke(r, CornerRadius::same(6), Stroke::new(1.5, t.accent), egui::StrokeKind::Inside);
        }
        if resp.on_hover_cursor(egui::CursorIcon::ZoomIn).on_hover_text(crate::i18n::shown("What it looked at; click to enlarge")).clicked() {
            *enlarged = Some(tex.id());
        }
    }
}

fn command_card(ui: &mut egui::Ui, line: &str, result: &Option<Result<String, String>>, pictures: &[egui::TextureHandle], enlarged: &mut Option<egui::TextureId>, w: f32, t: &Tokens) {
    egui::Frame::new().fill(well_fill(t)).corner_radius(CornerRadius::same(CARD_R)).inner_margin(egui::Margin::symmetric(12, 10)).show(ui, |ui| {
        ui.set_width(w - 24.0);
        ui.spacing_mut().item_spacing.y = 6.0;
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(crate::i18n::shown("›")).font(FontId::new(14.0, FontFamily::Monospace)).color(t.accent));
            kit::label_trunc(ui, egui::RichText::new(format!("/{}", line.trim_start_matches('/'))).font(theme::timecode()).color(t.text));
        });
        match result {
            None => {
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::hover());
                    kit::anim::spinner(ui.painter(), r.center(), 5.0, t.accent);
                    ui.label(egui::RichText::new(crate::i18n::shown("Running")).font(theme::caption()).color(t.text_2));
                });
            }
            Some(Ok(text)) => {
                let shown: String = if text.len() > 6000 { format!("{}…", &text[..text.floor_char_boundary(6000)]) } else { text.clone() };
                egui::ScrollArea::vertical().id_salt(("agent-cmd", line.len(), shown.len())).max_height(220.0).show_v(ui, |ui| {
                    ui.add(egui::Label::new(egui::RichText::new(shown).font(theme::mono_small()).color(t.text_2)).wrap());
                });
            }
            Some(Err(e)) => {
                ui.add(egui::Label::new(egui::RichText::new(e.as_str()).font(theme::body()).color(t.danger)).wrap());
            }
        }
        if !pictures.is_empty() {
            thumbs(ui, pictures, enlarged, w - 24.0, 0.0, theme::body().size * 4.6, t);
        }
    });
}

/// Returns true when Stop was clicked.
/// The working line: what it is doing, Pause and Stop. Returns (stop,
/// pause) clicks.
fn working_row(ui: &mut egui::Ui, entries: &[Entry], plan_title: Option<&str>, turn_started: Option<Instant>, paused: bool, t: &Tokens) -> (bool, bool) {
    let doing = if paused {
        crate::i18n::t("Paused at the next step").into_owned()
    } else {
        // Only the steps of this request, the last stretch of the chat.
        let from = entries.iter().rposition(|e| matches!(e, Entry::You(_))).unwrap_or(0);
        let idx: Vec<usize> = (from..entries.len()).collect();
        digest::doing(&digest_steps(entries, &idx), plan_title)
    };
    let secs = turn_started.map_or(0, |s| s.elapsed().as_secs());
    let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 22.0), Sense::hover());
    let p = ui.painter();
    if paused {
        icons::paint(p, Rect::from_center_size(egui::pos2(r.left() + 8.0, r.center().y), Vec2::splat(11.0)), Icon::PauseFill, 11.0, t.warn);
    } else {
        kit::anim::spinner(p, egui::pos2(r.left() + 8.0, r.center().y), 6.5, t.accent);
    }
    // A soft pulse on the words: alive, not busy.
    let pulse = if kit::anim::reduce_motion(ui.ctx()) { 1.0 } else { 0.72 + 0.28 * ((kit::anim::clock(ui.ctx()) * 2.6).sin() as f32 * 0.5 + 0.5) };
    let g = p.layout_no_wrap(format!("{}…", crate::i18n::shown(&doing)), theme::body(), theme::alpha(t.text, pulse));
    let gw = g.size().x;
    p.galley(egui::pos2(r.left() + 24.0, r.center().y - g.size().y / 2.0), g, t.text);
    let elapsed = if secs >= 60 { format!("{}:{:02}", secs / 60, secs % 60) } else { format!("{secs}s") };
    p.text(egui::pos2(r.left() + 32.0 + gw, r.center().y), Align2::LEFT_CENTER, elapsed, theme::caption(), t.text_2);
    ui.ctx().request_repaint();
    // Stop, always in reach.
    let sw = kit::text_width(ui, "Stop", theme::caption()) + 34.0;
    let sr = Rect::from_min_size(egui::pos2(r.right() - sw, r.center().y - 11.0), Vec2::new(sw, 22.0));
    let resp = ui.interact(sr, ui.scope_id().with("agent-stop-pill"), Sense::click());
    let p = ui.painter();
    p.rect_filled(sr, CornerRadius::same(11), if resp.hovered() { theme::alpha(t.danger, 0.22) } else { card_fill(t) });
    p.rect_filled(Rect::from_center_size(egui::pos2(sr.left() + 14.0, sr.center().y), Vec2::splat(7.0)), CornerRadius::same(1), if resp.hovered() { t.danger } else { t.text_2 });
    p.text(egui::pos2(sr.left() + 24.0, sr.center().y), Align2::LEFT_CENTER, "Stop", theme::caption(), if resp.hovered() { t.text } else { t.text_2 });
    let stop = resp.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text("Stop this request (Esc). What is done stays; Undo this request takes it back.").clicked();
    // Pause / Resume, left of Stop.
    let label = if paused { "Resume" } else { "Pause" };
    let pw = kit::text_width(ui, label, theme::caption()) + 34.0;
    let pr = Rect::from_min_size(egui::pos2(sr.left() - 6.0 - pw, sr.top()), Vec2::new(pw, 22.0));
    let presp = ui.interact(pr, ui.scope_id().with("agent-pause-pill"), Sense::click());
    let p = ui.painter();
    let on = presp.hovered() || paused;
    p.rect_filled(pr, CornerRadius::same(11), if on { theme::alpha(t.accent, 0.18) } else { card_fill(t) });
    icons::paint(p, Rect::from_center_size(egui::pos2(pr.left() + 14.0, pr.center().y), Vec2::splat(10.0)), if paused { Icon::PlayFill } else { Icon::PauseFill }, 10.0, if on { t.accent } else { t.text_2 });
    p.text(egui::pos2(pr.left() + 24.0, pr.center().y), Align2::LEFT_CENTER, label, theme::caption(), if on { t.text } else { t.text_2 });
    let pause = presp.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(if paused { "Go on from where it held" } else { "Hold at the next step; nothing is lost. Type a correction meanwhile." }).clicked();
    (stop, pause)
}

/// A correction the person sent mid-run: their bubble, marked.
fn correction_bubble(ui: &mut egui::Ui, text: &str, w: f32, t: &Tokens) {
    you_bubble(ui, text, w, t);
    let (r, _) = ui.allocate_exact_size(Vec2::new(w, 16.0), Sense::hover());
    ui.painter().text(egui::pos2(r.right(), r.center().y), Align2::RIGHT_CENTER, "Correction · picked up at the next step", theme::caption(), t.accent);
}

fn role_icon(role: Option<Role>) -> Icon {
    match role {
        None => Icon::Sparkle,
        Some(Role::Director) => Icon::FilmSlate,
        Some(Role::Editor) => Icon::Scissors,
        Some(Role::Animator) => Icon::Path,
        Some(Role::Sound) => Icon::Waveform,
        Some(Role::Colour) => Icon::Palette,
        Some(Role::Vfx) => Icon::MagicWand,
    }
}

/// Minutes as people say them.
fn minutes(m: f32) -> String {
    if m < 1.0 { "under a minute".into() } else if m < 90.0 { format!("{:.0} min", m) } else { format!("{:.1} h", m / 60.0) }
}

fn tokens_short(n: u64) -> String {
    if n >= 1_000_000 { format!("{:.1}M", n as f64 / 1e6) } else { format!("{}k", n / 1000) }
}

/// What one plan step shows in the card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Marker {
    Spinner,
    Done,
    /// Held by Pause, or waiting for approval.
    Held,
    Stopped,
    SentBack,
    Failed,
    Pending,
}

/// A step's marker. Nothing spins while the plan is paused or stopped.
fn step_marker(plan: &Plan, s: &crew::Step) -> Marker {
    match s.status {
        Status::Running if plan.halted == Some(Halt::Stopped) => Marker::Stopped,
        Status::Running if plan.halted == Some(Halt::Paused) => Marker::Held,
        Status::Running => Marker::Spinner,
        Status::Done => Marker::Done,
        Status::Waiting => Marker::Held,
        Status::SentBack => Marker::SentBack,
        Status::Failed => Marker::Failed,
        Status::Stopped => Marker::Stopped,
        Status::Pending => Marker::Pending,
    }
}

/// The card's head line on the right: Stopped or Paused when it is, with no
/// time left; otherwise how far and about how long.
fn plan_status_text(plan: &Plan) -> String {
    let done = plan.done();
    let mut right = match plan.halted {
        Some(Halt::Stopped) => "Stopped".to_owned(),
        Some(Halt::Paused) => "Paused".to_owned(),
        None => String::new(),
    };
    if !right.is_empty() {
        right.push_str(" · ");
    }
    right.push_str(&format!("{done} of {} done", plan.steps.len()));
    if plan.halted.is_none() {
        if done < plan.steps.len() {
            if let Some(m) = plan.minutes_left() {
                right.push_str(&format!(" · about {} left", minutes(m)));
            }
        } else if let Some(s) = plan.started {
            right.push_str(&format!(" · {}", minutes(s.elapsed().as_secs_f32() / 60.0)));
        }
    }
    right
}

/// The plan as the person follows it: each step, who does it, how far
/// along, the last word from it; time and tokens, estimated and spent.
fn plan_card(ui: &mut egui::Ui, plan: &Plan, w: f32, t: &Tokens) {
    let row_h = 22.0;
    let note_h = 16.0;
    let steps_h: f32 = plan.steps.iter().map(|s| row_h + if s.last_note().is_some() { note_h } else { 0.0 } + 4.0).sum();
    let foot = plan.verdict.as_ref().map(|v| ui.painter().layout(format!("Director: {}", v), theme::caption(), t.text_2, w - 28.0));
    let h = 14.0 + 22.0 + 8.0 + steps_h + foot.as_ref().map_or(0.0, |g| g.size().y + 8.0) + 10.0;
    let (r, _) = ui.allocate_exact_size(Vec2::new(w, h), Sense::hover());
    let p = ui.painter().clone();
    p.rect_filled(r, CornerRadius::same(CARD_R), card_fill(t));
    let x0 = r.left() + 14.0;
    let mut y = r.top() + 14.0 + 11.0;
    // Head: what, how far, how long.
    icons::paint(&p, Rect::from_center_size(egui::pos2(x0 + 7.0, y), Vec2::splat(14.0)), if plan.crew { Icon::FilmSlate } else { Icon::List }, 14.0, t.accent);
    let head = if plan.crew { "Crew plan" } else { "Plan" };
    let hg = p.layout_no_wrap(head.into(), chat_bold(), t.text);
    let hw = hg.size().x;
    p.galley(egui::pos2(x0 + 22.0, y - hg.size().y / 2.0), hg, t.text);
    let done = plan.done();
    let tokens: u64 = plan.steps.iter().map(|s| s.tokens).sum();
    let mut right = plan_status_text(plan);
    if let Some(e) = plan.estimate {
        right.push_str(&format!(" · {} of about {} used", tokens_short(tokens), tokens_short(e.tokens)));
    }
    if plan.round > 0 && plan.crew {
        right.push_str(&format!(" · review {}", plan.round));
    }
    kit::paint_text_clipped(&p, egui::pos2(r.right() - 14.0, y), Align2::RIGHT_CENTER, &right, theme::caption(), t.text_2, (r.width() - hw - 70.0).max(40.0));
    y += 11.0 + 8.0;
    for s in &plan.steps {
        let cy = y + row_h / 2.0;
        let c = egui::pos2(x0 + 7.0, cy);
        match step_marker(plan, s) {
            Marker::Spinner => kit::anim::spinner(&p, c, 5.5, t.accent),
            Marker::Done => {
                p.circle_filled(c, 7.0, theme::alpha(t.accent, 0.18));
                check_glyph(&p, c, 7.0, t.accent);
            }
            Marker::Held => {
                p.circle_filled(c, 7.0, theme::alpha(t.warn, 0.22));
                icons::paint(&p, Rect::from_center_size(c, Vec2::splat(9.0)), Icon::PauseFill, 9.0, t.warn);
            }
            Marker::Stopped => {
                p.circle_filled(c, 7.0, theme::alpha(t.warn, 0.22));
                icons::paint(&p, Rect::from_center_size(c, Vec2::splat(9.0)), Icon::X, 9.0, t.warn);
            }
            Marker::SentBack => {
                p.circle_stroke(c, 6.0, Stroke::new(1.4, t.warn));
                icons::paint(&p, Rect::from_center_size(c, Vec2::splat(9.0)), Icon::ArrowCounterClockwise, 9.0, t.warn);
            }
            Marker::Failed => {
                p.circle_filled(c, 7.0, theme::alpha(t.danger, 0.2));
                icons::paint(&p, Rect::from_center_size(c, Vec2::splat(9.0)), Icon::X, 9.0, t.danger);
            }
            Marker::Pending => {
                p.circle_stroke(c, 6.0, Stroke::new(1.2, theme::alpha(t.text_2, 0.6)));
            }
        }
        let tx = x0 + 22.0;
        // Who does it (a crew), and how long it took.
        let mut rx = r.right() - 14.0;
        let took = s.took + if s.status == Status::Running && plan.halted.is_none() { s.started.map_or(0.0, |t| t.elapsed().as_secs_f32()) } else { 0.0 };
        if took > 0.5 {
            let g = p.layout_no_wrap(if took < 60.0 { format!("{took:.0}s") } else { format!("{:.0}m", took / 60.0) }, theme::caption(), t.text_2);
            rx -= g.size().x;
            p.galley(egui::pos2(rx, cy - g.size().y / 2.0), g, t.text_2);
            rx -= 8.0;
        }
        if let Some(role) = s.role {
            let label = role.label();
            let lw = kit::text_width(ui, label, theme::caption()) + 26.0;
            let chip = Rect::from_min_size(egui::pos2(rx - lw, cy - 9.0), Vec2::new(lw, 18.0));
            p.rect_filled(chip, CornerRadius::same(9), well_fill(t));
            icons::paint(&p, Rect::from_center_size(egui::pos2(chip.left() + 10.0, cy), Vec2::splat(10.0)), role_icon(Some(role)), 10.0, t.text_2);
            p.text(egui::pos2(chip.left() + 19.0, cy), Align2::LEFT_CENTER, label, theme::caption(), t.text_2);
            rx = chip.left() - 8.0;
        }
        let colour = if s.status == Status::Pending { t.text_2 } else { t.text };
        let title = if s.checkpoint { format!("{}  ◆", s.title) } else { s.title.clone() };
        kit::paint_text_clipped(&p, egui::pos2(tx, cy), Align2::LEFT_CENTER, &title, theme::body(), colour, (rx - tx).max(30.0));
        let hover_r = Rect::from_min_max(egui::pos2(x0, y), egui::pos2(r.right() - 14.0, y + row_h));
        let tip = format!("{}{}", if s.checkpoint { "Checkpoint (◆): you approve it before the rest builds on it.\n" } else { "" }, s.last_note().map(|n| one_line(n, 160)).unwrap_or_default());
        ui.interact(hover_r, ui.scope_id().with(("plan-step", s.id)), Sense::hover()).on_hover_text(tip.trim().to_owned());
        y += row_h;
        if let Some(n) = s.last_note() {
            kit::paint_text_clipped(&p, egui::pos2(tx, y + note_h / 2.0 - 2.0), Align2::LEFT_CENTER, &one_line(n, 200), theme::caption(), t.text_2, r.right() - 14.0 - tx);
            y += note_h;
        }
        y += 4.0;
    }
    if let Some(g) = foot {
        y += 4.0;
        p.galley(egui::pos2(x0, y), g, t.text_2);
    }
    if plan.halted.is_none() && plan.steps.iter().any(|s| s.status == Status::Running) {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
    }
}

fn outside_row(ui: &mut egui::Ui, t: &Tokens) {
    let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 22.0), Sense::hover());
    let p = ui.painter();
    kit::anim::spinner(p, egui::pos2(r.left() + 8.0, r.center().y), 6.5, t.accent);
    p.text(egui::pos2(r.left() + 24.0, r.center().y), Align2::LEFT_CENTER, crate::i18n::shown("An agent outside the app is working on this project"), theme::body(), t.text_2);
    ui.ctx().request_repaint();
}

// ------------------------------------------------------------------ markdown

/// The AI's answers: paragraphs, headings, lists, **bold**, `code` and
/// code blocks.
fn markdown(ui: &mut egui::Ui, text: &str, w: f32, t: &Tokens) {
    let mut code: Option<String> = None;
    let mut first = true;
    for raw in text.lines() {
        let line = raw.trim_end();
        if line.trim_start().starts_with("```") {
            match code.take() {
                Some(c) => code_block(ui, &c, w, t),
                None => code = Some(String::new()),
            }
            continue;
        }
        if let Some(c) = code.as_mut() {
            c.push_str(line);
            c.push('\n');
            continue;
        }
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            ui.add_space(7.0);
            continue;
        }
        if !first {
            ui.add_space(3.0);
        }
        first = false;
        let indent = ((line.len() - trimmed.len()) as f32 / 2.0).min(4.0) * 14.0;
        if let Some(h) = trimmed.strip_prefix("### ").or_else(|| trimmed.strip_prefix("## ")).or_else(|| trimmed.strip_prefix("# ")) {
            ui.add_space(4.0);
            let job = inline(h, FontId::new(14.0, FontFamily::Name(theme::SEMIBOLD.into())), t.text, w, t);
            ui.add(egui::Label::new(job));
            continue;
        }
        let bullet = ["- ", "* ", "• "].iter().find_map(|b| trimmed.strip_prefix(b));
        let numbered = trimmed.split_once(". ").filter(|(n, _)| !n.is_empty() && n.len() <= 3 && n.chars().all(|c| c.is_ascii_digit()));
        let (marker, body) = match (bullet, numbered) {
            (Some(b), _) => (Some("•".to_owned()), b),
            (None, Some((n, b))) => (Some(format!("{n}.")), b),
            _ => (None, trimmed),
        };
        match marker {
            Some(m) => {
                let mx = indent + 4.0;
                let body_x = indent + 20.0;
                let job = inline(body, chat_font(), t.text, w - body_x, t);
                let g = ui.painter().layout_job(job);
                let (r, _) = ui.allocate_exact_size(Vec2::new(w, g.size().y), Sense::hover());
                let p = ui.painter();
                let font = if m == "•" { chat_bold() } else { chat_font() };
                p.text(egui::pos2(r.left() + mx, r.top() + 9.0), Align2::LEFT_CENTER, m, font, t.text_2);
                p.galley(egui::pos2(r.left() + body_x, r.top()), g, t.text);
            }
            None => {
                let job = inline(body, chat_font(), t.text, w - indent, t);
                let g = ui.painter().layout_job(job);
                let (r, _) = ui.allocate_exact_size(Vec2::new(w, g.size().y), Sense::hover());
                ui.painter().galley(egui::pos2(r.left() + indent, r.top()), g, t.text);
            }
        }
    }
    if let Some(c) = code {
        code_block(ui, &c, w, t);
    }
}

/// One line's inline styles.
fn inline(text: &str, font: FontId, colour: Color32, wrap: f32, t: &Tokens) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.wrap.max_width = wrap;
    let bold = FontId::new(font.size, FontFamily::Name(theme::SEMIBOLD.into()));
    let plain = TextFormat { font_id: font.clone(), color: colour, line_height: Some(font.size * 1.45), ..Default::default() };
    let strong = TextFormat { font_id: bold, color: t.text, ..plain.clone() };
    let mono = TextFormat { font_id: FontId::new(font.size - 1.5, FontFamily::Monospace), color: theme::mix(t.text, t.accent, 0.45), background: well_fill(t), ..plain.clone() };
    let mut rest = text;
    while !rest.is_empty() {
        let next = [("**", 0), ("`", 1)].iter().filter_map(|(m, k)| rest.find(m).map(|i| (i, *m, *k))).min_by_key(|x| x.0);
        match next {
            Some((i, m, k)) if rest[i + m.len()..].contains(m) => {
                job.append(&rest[..i], 0.0, plain.clone());
                let after = &rest[i + m.len()..];
                let end = after.find(m).expect("closing mark");
                job.append(&after[..end], 0.0, if k == 0 { strong.clone() } else { mono.clone() });
                rest = &after[end + m.len()..];
            }
            _ => {
                job.append(rest, 0.0, plain.clone());
                rest = "";
            }
        }
    }
    job
}

fn code_block(ui: &mut egui::Ui, code: &str, w: f32, t: &Tokens) {
    ui.add_space(4.0);
    egui::Frame::new().fill(well_fill(t)).corner_radius(CornerRadius::same(8)).inner_margin(egui::Margin::symmetric(10, 8)).show(ui, |ui| {
        ui.set_width(w - 20.0);
        ui.add(egui::Label::new(egui::RichText::new(code.trim_end()).font(theme::mono_small()).color(t.text)).wrap());
    });
    ui.add_space(4.0);
}

// ------------------------------------------------------------------ approval

fn approval_lines(what: &str) -> (Vec<&str>, Option<&str>) {
    let mut lines: Vec<&str> = what.lines().filter(|l| !l.trim().is_empty()).collect();
    let foot = lines.last().filter(|l| l.starts_with('(') && l.ends_with(')')).copied();
    if foot.is_some() {
        lines.pop();
    }
    (lines, foot)
}

fn approval_height(ui: &egui::Ui, a: &AgentUi, w: f32) -> f32 {
    let Some(ask) = a.asks.first() else { return 0.0 };
    let (lines, foot) = approval_lines(&ask.what);
    let bw = w - 28.0 - 26.0;
    let body: f32 = lines.iter().map(|l| ui.painter().layout(l.to_string(), theme::body(), Color32::WHITE, bw).size().y * 17.0 / 15.0 + 3.0).sum();
    // Head, body, footnote, buttons.
    (12.0 + 26.0 + body.min(130.0) + if foot.is_some() { 18.0 } else { 0.0 } + 10.0 + 28.0 + 12.0).min(260.0)
}

fn approval(app: &mut StudioApp, ui: &mut egui::Ui, r: Rect) {
    let t = theme::tokens(ui.ctx());
    let a = app.agent.as_mut().expect("agent");
    let Some(ask) = a.asks.first() else { return };
    let p = ui.painter().clone();
    p.rect_filled(r, CornerRadius::same(CARD_R), card_fill(&t));
    p.rect_stroke(r, CornerRadius::same(CARD_R), Stroke::new(1.0, theme::alpha(t.accent, 0.55)), egui::StrokeKind::Inside);
    let inner = r.shrink2(Vec2::new(14.0, 12.0));
    // Head.
    let hy = inner.top() + 10.0;
    p.circle_filled(egui::pos2(inner.left() + 9.0, hy), 9.0, theme::alpha(t.accent, 0.16));
    icons::paint(&p, Rect::from_center_size(egui::pos2(inner.left() + 9.0, hy), Vec2::splat(12.0)), Icon::Question, 12.0, t.accent);
    p.text(egui::pos2(inner.left() + 26.0, hy), Align2::LEFT_CENTER, crate::i18n::shown("Approve this change?"), chat_bold(), t.text);
    if a.asks.len() > 1 {
        p.text(egui::pos2(inner.right(), hy), Align2::RIGHT_CENTER, format!("1 of {}", a.asks.len()), theme::caption(), t.text_2);
    }
    // What it would do.
    let (lines, foot) = approval_lines(&ask.what);
    let body = Rect::from_min_max(egui::pos2(inner.left() + 26.0, hy + 16.0), egui::pos2(inner.right(), inner.bottom() - 40.0));
    let mut bui = ui.new_child(egui::UiBuilder::new().max_rect(body).id_salt(("agent-ask-body", ask.id)));
    egui::ScrollArea::vertical().auto_shrink([false, false]).show_v(&mut bui, |ui| {
        ui.spacing_mut().item_spacing.y = 3.0;
        let bw = ui.available_width();
        for l in &lines {
            let (verb, rest) = l.split_once(": ").unwrap_or((l, ""));
            let mut job = LayoutJob::default();
            job.wrap.max_width = bw;
            let f = TextFormat { font_id: theme::body(), color: t.text_2, line_height: Some(17.0), ..Default::default() };
            job.append(verb, 0.0, TextFormat { font_id: small_bold(), color: t.text, ..f.clone() });
            if !rest.is_empty() {
                job.append(rest, 6.0, f);
            }
            ui.add(egui::Label::new(job));
        }
        if let Some(f) = foot {
            ui.label(egui::RichText::new(f.trim_matches(|c| c == '(' || c == ')')).font(theme::caption()).color(t.text_2));
        }
    });
    // Answers.
    let row = Rect::from_min_max(egui::pos2(inner.left(), inner.bottom() - 28.0), inner.max);
    let mut rui = ui.new_child(egui::UiBuilder::new().max_rect(row).layout(egui::Layout::right_to_left(egui::Align::Center)).id_salt(("agent-ask-row", ask.id)));
    rui.spacing_mut().item_spacing.x = 6.0;
    let mut answer = None;
    if Button::primary("Apply").icon(Icon::Check).show(&mut rui).clicked() {
        answer = Some(Answer::Apply);
    }
    if Button::secondary("Apply all").show(&mut rui).on_hover_text(crate::i18n::shown("This and the rest of this request, without asking")).clicked() {
        answer = Some(Answer::ApplyAll);
    }
    if Button::ghost("Skip").show(&mut rui).clicked() {
        answer = Some(Answer::Skip);
    }
    if let Some(ans) = answer {
        let ask = a.asks.remove(0);
        let _ = ask.answer.send(ans);
    }
}

// ------------------------------------------------------------------ composer

fn composer_card(app: &mut StudioApp, ui: &mut egui::Ui, r: Rect) {
    let t = theme::tokens(ui.ctx());
    let ask_first = app.prefs.ai_ask_first;
    let a = app.agent.as_mut().expect("agent");
    let busy = a.busy;
    let focused = ui.ctx().memory(|m| m.focused()) == Some(egui::Id::unique("agent-input"));
    let p = ui.painter().clone();
    p.rect_filled(r, CornerRadius::same(12), kit::field_fill(ui));
    p.rect_stroke(r, CornerRadius::same(12), Stroke::new(1.0, if focused { theme::alpha(t.accent, 0.7) } else { t.line }), egui::StrokeKind::Inside);

    // Marks on the picture that go with this message.
    let mut chips_h = 0.0;
    if !a.attachments.is_empty() {
        chips_h = 34.0;
        let chips: Vec<(Color32, String, Icon)> = a
            .attachments
            .iter()
            .enumerate()
            .map(|(i, m)| match m.kind {
                MarkKind::Area => (t.accent, format!("{} · Area · {}", i + 1, super::ai_marks::time_label(m.time, 0.0)), Icon::Selection),
                MarkKind::Note => (t.warn, format!("{} · “{}”", i + 1, one_line(&m.note, 26)), Icon::Subtitles),
            })
            .collect();
        let widths: Vec<f32> = chips.iter().map(|(_, label, _)| kit::text_width(ui, label, theme::caption()) + 46.0).collect();
        let total = widths.iter().sum::<f32>() + 6.0 * (widths.len() as f32 - 1.0).max(0.0);
        // More marks than the row holds: the row scrolls sideways.
        let row = Rect::from_min_max(egui::pos2(r.left() + 10.0, r.top() + 10.0), egui::pos2(r.right() - 10.0, r.top() + 34.0));
        let mut strip = ui.new_child(egui::UiBuilder::new().max_rect(row).id_salt("agent-marks"));
        strip.set_clip_rect(row.intersect(ui.clip_rect()));
        let mut remove = None;
        egui::ScrollArea::horizontal().id_salt("agent-marks-scroll").auto_shrink([false, false]).show_h(&mut strip, |ui| {
            let (area, _) = ui.allocate_exact_size(Vec2::new(total.max(row.width()), 24.0), Sense::hover());
            let p = ui.painter().clone();
            let mut x = area.left();
            for (i, ((colour, label, icon), w)) in chips.iter().zip(&widths).enumerate() {
                let cr = Rect::from_min_size(egui::pos2(x, area.top()), Vec2::new(*w, 24.0));
                p.rect_filled(cr, CornerRadius::same(12), theme::alpha(*colour, 0.14));
                p.rect_stroke(cr, CornerRadius::same(12), Stroke::new(1.0, theme::alpha(*colour, 0.45)), egui::StrokeKind::Inside);
                icons::paint(&p, Rect::from_center_size(egui::pos2(cr.left() + 13.0, cr.center().y), Vec2::splat(12.0)), *icon, 12.0, *colour);
                p.text(egui::pos2(cr.left() + 24.0, cr.center().y), Align2::LEFT_CENTER, label, theme::caption(), t.text);
                let xr = Rect::from_center_size(egui::pos2(cr.right() - 11.0, cr.center().y), Vec2::splat(14.0));
                let xresp = ui.interact(xr, ui.scope_id().with(("agent-mark-x", i)), Sense::click());
                let col = if xresp.hovered() { t.text } else { t.text_2 };
                let c = xr.center();
                p.line_segment([c + Vec2::new(-3.0, -3.0), c + Vec2::new(3.0, 3.0)], Stroke::new(1.3, col));
                p.line_segment([c + Vec2::new(3.0, -3.0), c + Vec2::new(-3.0, 3.0)], Stroke::new(1.3, col));
                if xresp.on_hover_text(crate::i18n::shown("Remove this mark")).clicked() {
                    remove = Some(i);
                }
                x = cr.right() + 6.0;
            }
        });
        if let Some(i) = remove {
            a.attachments.remove(i);
        }
    }

    // The text.
    let text_r = Rect::from_min_max(r.min + Vec2::new(12.0, 10.0 + chips_h), egui::pos2(r.right() - 12.0, r.bottom() - 40.0));
    let hint = if !a.asks.is_empty() {
        "Type apply, apply all or skip…"
    } else if !a.attachments.is_empty() {
        "Say what to do with what you marked…"
    } else if busy {
        "Working… send a correction now, or queue the next request"
    } else {
        "Ask for anything, or / for a command"
    };
    let mut tui = ui.new_child(egui::UiBuilder::new().max_rect(text_r).id_salt("agent-input-area"));
    let resp = egui::ScrollArea::vertical()
        .id_salt("agent-input-scroll")
        .max_height(text_r.height())
        .show_v(&mut tui, |ui| {
            ui.add_sized(
                Vec2::new(text_r.width(), text_r.height()),
                egui::TextEdit::multiline(&mut a.input)
                    .id(egui::Id::unique("agent-input"))
                    .frame(egui::Frame::NONE)
                    .hint_text(egui::RichText::new(hint).color(t.text_2))
                    .font(chat_font())
                    .text_color(t.text)
                    .desired_rows(2)
                    .return_key(Some(egui::KeyboardShortcut::new(egui::Modifiers::SHIFT, egui::Key::Enter))),
            )
        })
        .inner;
    if std::mem::take(&mut a.focus_input) {
        resp.request_focus();
    }
    // "/" opens the list of commands (the tools the Assistant itself uses);
    // a click puts the name in the box, ready for its arguments.
    if a.input.starts_with('/') && !a.input.contains(char::is_whitespace) {
        let q = a.input[1..].to_lowercase();
        let hits: Vec<&(String, String)> = slash_commands().iter().filter(|(n, _)| n.contains(&q)).take(8).collect();
        if !hits.is_empty() {
            let mut picked = None;
            egui::Area::new(egui::Id::new("assistant-slash")).order(egui::Order::Foreground).fixed_pos(r.left_top() - Vec2::new(0.0, 4.0)).pivot(Align2::LEFT_BOTTOM).show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_width((r.width() - 16.0).max(160.0));
                    for (name, gist) in hits {
                        let row = ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(format!("/{name}")).font(theme::control_medium()).color(t.text));
                            ui.label(egui::RichText::new(gist).font(theme::caption()).color(t.text_2));
                        });
                        if ui.interact(row.response.rect, ui.id().with(name), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                            picked = Some(name.clone());
                        }
                    }
                });
            });
            if let Some(n) = picked {
                a.input = format!("/{n} ");
                resp.request_focus();
            }
        }
    }
    let mut send = false;
    let mut correct = false;
    let mut stop = false;
    if resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift) {
        while a.input.ends_with('\n') {
            a.input.pop();
        }
        // Cmd+Enter while working: a correction it takes in now.
        if busy && ui.input(|i| i.modifiers.command) {
            correct = true;
        } else {
            send = true;
        }
    }

    // Bottom row: mode switch, hint, send / stop.
    let row_y = r.bottom() - 20.0;
    let mut new_mode = None;
    let mut x = r.left() + 10.0;
    for (i, (label, tip)) in [("Auto", "Changes land on the timeline as they are made"), ("Ask first", "You approve each change before it is made")].into_iter().enumerate() {
        let on = (i == 1) == ask_first;
        let lw = kit::text_width(ui, label, theme::caption()) + 18.0;
        let pr = Rect::from_min_size(egui::pos2(x, row_y - 11.0), Vec2::new(lw, 22.0));
        let pres = ui.interact(pr, ui.scope_id().with(("agent-mode", i)), Sense::click());
        let fill = if on { t.accent_soft } else if pres.hovered() { theme::alpha(t.raised, 0.8) } else { Color32::TRANSPARENT };
        p.rect_filled(pr, CornerRadius::same(11), fill);
        p.text(pr.center(), Align2::CENTER_CENTER, label, theme::caption(), if on { t.accent } else { t.text_2 });
        if pres.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(tip).clicked() {
            new_mode = Some(i == 1);
        }
        x = pr.right() + 2.0;
    }
    let can_send = !a.input.trim().is_empty() || !a.attachments.is_empty();
    let show_correct = busy && can_send && a.asks.is_empty();
    let correct_w = kit::text_width(ui, "Correct now", theme::caption()) + 20.0;
    // The keys hint, only where it fits between the modes and what is at
    // the end of the row (the Correct now chip, the round button).
    let hint = if busy { "Cmd+Enter corrects now  ·  Enter queues  ·  Esc stops" } else { "Enter sends  ·  Shift+Enter adds a line" };
    let hint_end = if show_correct { r.right() - 44.0 - correct_w - 8.0 } else { r.right() - 48.0 };
    if r.width() > 360.0 && kit::text_width(ui, hint, theme::caption()) <= hint_end - x - 8.0 {
        p.text(egui::pos2(hint_end, row_y), Align2::RIGHT_CENTER, hint, theme::caption(), theme::alpha(t.text_2, 0.8));
    }
    // While working, typed words can go in now as a correction.
    if show_correct {
        let label = "Correct now";
        let lw = correct_w;
        let cr = Rect::from_min_size(egui::pos2(r.right() - 44.0 - lw, row_y - 11.0), Vec2::new(lw, 22.0));
        let cres = ui.interact(cr, ui.scope_id().with("agent-correct"), Sense::click());
        p.rect_filled(cr, CornerRadius::same(11), if cres.hovered() { t.hover(t.accent_soft) } else { t.accent_soft });
        p.text(cr.center(), Align2::CENTER_CENTER, label, theme::caption(), t.accent);
        if cres.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text("Send this to the running request now (Cmd+Enter): it changes course at its next step").clicked() {
            correct = true;
        }
    }
    // The round button: send, or stop while working.
    let bc = egui::pos2(r.right() - 22.0, row_y);
    let br = Rect::from_center_size(bc, Vec2::splat(28.0));
    let bres = ui.interact(br, ui.scope_id().with("agent-send"), Sense::click());
    let hover = bres.hovered();
    if busy && !can_send {
        let base = theme::mix(kit::field_fill(ui), t.text, 0.14);
        p.circle_filled(bc, 14.0, if hover { theme::mix(base, t.text, 0.1) } else { base });
        p.rect_filled(Rect::from_center_size(bc, Vec2::splat(9.0)), CornerRadius::same(2), t.text);
        if bres.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(crate::i18n::shown("Stop (Esc)")).clicked() {
            stop = true;
        }
    } else {
        let (fill, ink) = if can_send { (if hover { t.hover(t.accent_fill) } else { t.accent_fill }, t.accent_ink) } else { (theme::mix(kit::field_fill(ui), t.text, 0.08), t.text_2) };
        p.circle_filled(bc, 14.0, fill);
        let s = Stroke::new(1.8, ink);
        p.line_segment([bc + Vec2::new(0.0, 5.5), bc + Vec2::new(0.0, -5.5)], s);
        p.line_segment([bc + Vec2::new(-4.5, -1.0), bc + Vec2::new(0.0, -5.5)], s);
        p.line_segment([bc + Vec2::new(4.5, -1.0), bc + Vec2::new(0.0, -5.5)], s);
        if bres.on_hover_cursor(if can_send { egui::CursorIcon::PointingHand } else { egui::CursorIcon::Default }).on_hover_text(if busy { "Send when this one finishes (Enter)" } else { "Send (Enter)" }).clicked() && can_send {
            send = true;
        }
    }
    if stop {
        if let Some(a) = app.agent.as_mut() {
            a.stop(&mut app.doc);
        }
    }
    if let Some(m) = new_mode {
        app.prefs.ai_ask_first = m;
    }
    if let Some(a) = app.agent.as_ref() {
        a.gate.ask_first.store(app.prefs.ai_ask_first, Ordering::SeqCst);
    }
    if send {
        AgentUi::send(app);
        if let Some(a) = app.agent.as_mut() {
            a.focus_input = true;
        }
    }
    if correct {
        AgentUi::correct(app);
        if let Some(a) = app.agent.as_mut() {
            a.focus_input = true;
        }
    }
}

/// Why a tool that reports `failed` entries (import, make_playable) did not
/// do everything it was asked: None when nothing failed.
fn failed_in(content: &[Content]) -> Option<String> {
    let text = content.iter().find_map(|c| match c {
        Content::Text(t) => Some(t.as_str()),
        _ => None,
    })?;
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    let failed = v.get("failed")?.as_array()?;
    if failed.is_empty() {
        return None;
    }
    let why: Vec<String> = failed
        .iter()
        .map(|f| match f {
            serde_json::Value::String(s) => s.clone(),
            other => other.get("error").and_then(|e| e.as_str()).map_or_else(|| other.to_string(), str::to_owned),
        })
        .collect();
    Some(why.join("; "))
}

#[cfg(test)]
mod tests {
    #[test]
    fn show_steps_never_prints_a_tool_name() {
        for tool in ["guide", "edit", "ai", "dub", "plan", "mix_stems", "subtitles"] {
            let name = step_name(tool);
            assert!(!name.contains('_') && name.chars().next().is_some_and(|c| c.is_uppercase()), "{tool} -> {name}");
        }
    }

    #[test]
    fn a_converted_import_with_failures_is_not_done() {
        let ok = vec![Content::Text(r#"{"media":[3],"failed":[]}"#.into())];
        assert_eq!(failed_in(&ok), None);
        let bad = vec![Content::Text(r#"{"media":[],"failed":["clip.mxf: no video stream"]}"#.into())];
        assert_eq!(failed_in(&bad).as_deref(), Some("clip.mxf: no video stream"));
        let playable = vec![Content::Text(r#"{"made_playable":[],"failed":[{"media":4,"error":"disk full"}]}"#.into())];
        assert_eq!(failed_in(&playable).as_deref(), Some("disk full"));
        assert_eq!(failed_in(&[Content::Jpeg(vec![1])]), None);
    }

    use super::*;

    /// A panel with a crew plan on the card and a request running.
    fn crew_card(a: &mut AgentUi) {
        a.gate.begin();
        a.gate.board.plan_tool(Some(Role::Director), &serde_json::json!({"goal": "film", "steps": [
            {"role": "editor", "title": "Cut", "task": "cut"},
            {"role": "sound", "title": "Mix", "task": "mix"},
            {"role": "colour", "title": "Grade", "task": "grade", "after": [1]},
        ]})).unwrap();
        a.gate.board.set_status_for_test(1, Status::Running);
        a.gate.board.set_status_for_test(2, Status::Running);
        a.busy = true;
        a.turn = 1;
        a.running = Some(0);
        a.tabs[0].plan_turn = 1;
        a.tabs[0].entries.push(Entry::Plan(a.gate.board.snapshot()));
    }

    fn card_plan(a: &AgentUi) -> Plan {
        a.tabs[0].entries.iter().rev().find_map(|e| if let Entry::Plan(p) = e { Some(p.clone()) } else { None }).expect("a plan card")
    }

    #[test]
    fn stop_makes_the_crew_card_say_stopped_with_nothing_spinning() {
        let ctx = egui::Context::default();
        let mut a = AgentUi::new(&ctx);
        let mut doc = crate::agent::Session::default().doc;
        crew_card(&mut a);
        let plan = card_plan(&a);
        assert_eq!(step_marker(&plan, &plan.steps[0]), Marker::Spinner);
        assert!(plan_status_text(&plan).contains("about"), "time left shown while it runs: {}", plan_status_text(&plan));
        a.stop(&mut doc);
        let plan = card_plan(&a);
        assert_eq!(plan.halted, Some(Halt::Stopped));
        assert!(plan.steps.iter().all(|s| step_marker(&plan, s) != Marker::Spinner), "nothing spins");
        assert_eq!(step_marker(&plan, &plan.steps[0]), Marker::Stopped);
        assert_eq!(step_marker(&plan, &plan.steps[2]), Marker::Pending, "a step that never started stays waiting");
        let text = plan_status_text(&plan);
        assert!(text.starts_with("Stopped") && !text.contains("left"), "{text}");
        assert!(!a.busy);
    }

    #[test]
    fn pause_makes_the_crew_card_say_paused_and_resume_brings_it_back() {
        let ctx = egui::Context::default();
        let mut a = AgentUi::new(&ctx);
        crew_card(&mut a);
        a.toggle_pause();
        let plan = card_plan(&a);
        assert_eq!(plan.halted, Some(Halt::Paused));
        assert!(plan.steps.iter().all(|s| step_marker(&plan, s) != Marker::Spinner), "nothing spins while paused");
        let text = plan_status_text(&plan);
        assert!(text.starts_with("Paused") && !text.contains("left"), "{text}");
        a.toggle_pause();
        let plan = card_plan(&a);
        assert_eq!(plan.halted, None);
        assert_eq!(step_marker(&plan, &plan.steps[0]), Marker::Spinner);
    }

    #[test]
    fn closing_a_tab_keeps_the_others_in_step() {
        let ctx = egui::Context::default();
        let mut a = AgentUi::new(&ctx);
        let mut doc = crate::agent::Session::default().doc;
        a.new_tab();
        a.new_tab();
        assert_eq!((a.tabs.len(), a.active), (3, 2), "+ adds a tab and shows it");
        a.tabs[0].entries.push(Entry::You("first".into()));
        a.tabs[2].entries.push(Entry::You("third".into()));
        a.running = Some(2);
        a.queued.push_back((1, "for the second".into()));
        a.queued.push_back((2, "for the third".into()));
        a.close_tab(1, &mut doc);
        assert_eq!(a.tabs.len(), 2);
        assert_eq!(a.tabs[1].title(), "third", "the third tab moved down one");
        assert_eq!(a.running, Some(1), "its request still belongs to it");
        assert_eq!(a.queued.iter().cloned().collect::<Vec<_>>(), vec![(1, "for the third".to_owned())], "the closed tab's waiting request went with it");
        assert_eq!(a.active, 1);
        assert_eq!(a.tabs[0].title(), "first", "the first conversation is untouched");
    }

    #[test]
    fn marks_read_as_quotes_and_as_exact_places() {
        let area = Mark { kind: MarkKind::Area, time: 12 * crate::view::SEC + crate::view::SEC * 4 / 10, rect: [-100.0, -50.0, 200.0, 150.0], note: String::new() };
        let spot = Mark { kind: MarkKind::Note, time: 3 * crate::view::SEC, rect: [40.0, 60.0, 40.0, 60.0], note: "blur this".into() };
        assert_eq!(area.quote(1), "> 1 · Area at 0:12.4");
        assert_eq!(spot.quote(2), "> 2 · Note at 0:03.0: “blur this”");
        assert!(area.for_ai(1).contains("box from (-100, -50) to (200, 150)") && area.for_ai(1).contains("12.40s"), "{}", area.for_ai(1));
        assert!(spot.for_ai(2).contains("spot (40, 60)") && spot.for_ai(2).contains("blur this"), "{}", spot.for_ai(2));
    }

    #[test]
    fn closing_the_last_tab_leaves_a_fresh_one() {
        let ctx = egui::Context::default();
        let mut a = AgentUi::new(&ctx);
        let mut doc = crate::agent::Session::default().doc;
        a.tabs[0].entries.push(Entry::You("hello".into()));
        a.close_tab(0, &mut doc);
        assert_eq!(a.tabs.len(), 1);
        assert!(a.tabs[0].entries.is_empty());
    }
}
