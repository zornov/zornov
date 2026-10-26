// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The agent on the document open in the window.
//!
//! Tools run on a worker thread against the worker's own copy of the
//! project, so a slow tool (looking, listening, captions, export) never
//! stalls the window. What a tool changed comes back as the edits it made
//! (its undo steps turned forward again) and is played onto the window's
//! document a few pieces at a time, so the person watches the timeline
//! build rather than see it all land at once. Each piece is a step of the
//! agent's own (its request and call): the person's Undo never takes it,
//! and the AI panel's "Undo this request" takes back all of a request.
//!
//! The person keeps working meanwhile. A change that comes back after they
//! edited is merged with what they did, theirs kept where both changed the
//! same thing, and the agent hears what changed outside its work.
//!
//! Before each call the copy is brought up to date when the window's
//! document moved (a hand edit, an undo), so the agent always works on
//! what the user sees. A few tools act on the window's document itself
//! (undo, redo, save, open, new project) and run on the UI thread.

use super::jobs::{self, Progress};
use super::{Content, Session};
use crate::doc::Doc;
use serde_json::Value;
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant};
use studio_model as m;

pub type ToolResult = Result<Vec<Content>, String>;

/// One tool call waiting for the window.
pub struct ToolCall {
    pub tool: String,
    pub args: Value,
    pub reply: Sender<ToolResult>,
    /// The Stop count of the agent's gate when the call began: a call from
    /// before the last Stop is refused (see [`Live::cancel_at`]).
    pub epoch: u64,
}

/// Tools that act on the window's document itself.
const ON_WINDOW: &[&str] = &["undo", "redo", "save", "open", "new_project"];

/// Tools that act on the window itself (its panels), answered by the
/// editor from `window_calls`.
const ON_WINDOW_UI: &[&str] = &["layout"];

/// Tools that read nothing of the project (static text, a search index, the
/// job table): answered the moment they arrive, even while a call runs.
const QUICK: &[&str] = &["guide", "howto", "edit_schema", "capabilities", "job"];

fn is_quick(tool: &str) -> bool {
    QUICK.contains(&tool)
}

/// Ids set aside for the copy each call: what the person makes by hand
/// meanwhile is numbered above them, so neither renumbers the other.
const ID_BLOCK: u64 = 1 << 12;

enum Job {
    Sync(m::Project, Option<m::file::ProjectFolder>),
    /// A call, and the first id the copy may hand out.
    Call(String, Value, u64),
}

struct Done {
    result: ToolResult,
    /// Forward edits with their undo labels, oldest first; `None` when they
    /// could not be recovered (the tool crashed or undid something).
    edits: Option<Vec<(String, m::Edit)>>,
    /// The copy after the call.
    model: m::Project,
}

struct Running {
    reply: Sender<ToolResult>,
    /// A long call (see `jobs`): how long it may run, and when it stops
    /// being waited for and goes to the background.
    long: Option<Duration>,
    started_at: Instant,
    tool: String,
    brief: String,
    progress: Arc<Progress>,
    /// Stopped: its changes are not applied.
    discard: bool,
    /// Whose change it makes.
    author: m::Author,
    /// The window's revision when it started: another one when it ends
    /// means the person changed something meanwhile.
    started: u64,
    /// The copy as the call found it.
    base: Option<m::Project>,
    /// What the agent is told besides the tool's answer.
    notes: Vec<String>,
}

/// A long call moved to the background: its worker goes on with it while a
/// new worker answers the other calls.
struct Detached {
    id: u32,
    run: Running,
    /// Kept so the worker lives until the call is done.
    _jobs: Sender<Job>,
    done: Receiver<Done>,
    deadline: Instant,
}

/// A call's changes being played onto the window.
struct Replay {
    reply: Sender<ToolResult>,
    result: ToolResult,
    author: m::Author,
    /// The copy before and after the call.
    base: Option<m::Project>,
    model: m::Project,
    /// Primitive edits still to apply, in order, with their undo labels.
    left: VecDeque<(String, m::Edit)>,
    per_step: usize,
    next_at: Instant,
    /// The window's revision after the last piece landed.
    expect: u64,
    notes: Vec<String>,
    /// A call the person asked for: its pieces are one step of theirs
    /// (a batch open while it plays).
    own_batch: bool,
    /// The agent was answered when the call finished, not when the last
    /// piece landed: `notes` that come later go with its next answer.
    early: bool,
    /// Pieces already played onto the window, in order. A merge starts from
    /// the base with these applied (see `land_merged`): they are the same
    /// objects, with the same ids, as the ones in the agent's copy, so they
    /// must not look like something both sides made new.
    applied: Vec<m::Edit>,
}

/// Time between two pieces of a change appearing.
const STEP: Duration = Duration::from_millis(40);
/// A change plays out in at most about this many steps.
const MAX_STEPS: usize = 45;

pub struct Live {
    jobs: Sender<Job>,
    done: Receiver<Done>,
    /// What the current worker's long tools report.
    progress: Arc<Progress>,
    detached: Vec<Detached>,
    /// Background jobs, running and finished (the `job` tool).
    table: Vec<jobs::Info>,
    next_job: u32,
    /// Finished jobs the agent has not been told of yet.
    job_notes: Vec<String>,
    /// The window revision the worker's copy matches.
    synced: Option<u64>,
    /// The project as the agent last saw it (its copy after its last call):
    /// what changed in the window since is told to it.
    known: Option<m::Project>,
    running: Option<Running>,
    replay: Option<Replay>,
    /// Calls waiting, and whether the person asked for each (a menu or a
    /// `/` command in the panel) rather than an agent.
    queue: VecDeque<(ToolCall, bool)>,
    repaint: Arc<dyn Fn() + Send + Sync>,
    /// Runs the window tools with the window's document swapped in.
    window: Session,
    /// The undo label of the request being worked on.
    turn: Option<String>,
    /// The request being worked on (0: agents outside the app).
    request: u64,
    requests: u64,
    calls_made: u64,
    /// Clips an agent just added or changed (the window selects them, so
    /// the person sees where the change happened).
    pub touched: Vec<u32>,
    /// An agent started a new project in the window (it gets a folder).
    pub made_project: bool,
    /// Calls for the window itself (see `ON_WINDOW_UI`), for the editor to
    /// answer.
    pub window_calls: Vec<ToolCall>,
    /// Pause: no agent call starts and no background job lands.
    paused: bool,
    /// Calls from before this Stop count are refused.
    floor: u64,
    /// Whose jobs and programs these are (see `procs`).
    owner: u64,
}

/// A worker thread with its own copy of the project: runs calls one at a
/// time and sends back what each did.
fn spawn_worker(repaint: Arc<dyn Fn() + Send + Sync>, owner: u64) -> (Sender<Job>, Receiver<Done>, Arc<Progress>) {
    let (jobs, rx) = mpsc::channel::<Job>();
    let (tx, done) = mpsc::channel::<Done>();
    let progress = Arc::new(Progress::default());
    let mine = progress.clone();
    std::thread::Builder::new()
        .name("studio-agent-tools".into())
        .spawn(move || {
            jobs::install(mine.clone());
            super::procs::set_owner(owner);
            let _ = crate::content::get_blocking();
            let mut s = Session::default();
            for job in rx {
                match job {
                    Job::Sync(model, folder) => s.doc.reset_model(model, folder),
                    Job::Call(tool, args, first_id) => {
                        mine.clear();
                        s.doc.model.next_id = s.doc.model.next_id.max(first_id);
                        let n = s.doc.history.undo_len();
                        let result = super::guard::run(&tool, || s.run(&tool, &args));
                        // A tool that panicked leaves nothing to forward.
                        let (result, edits) = if result.as_ref().err().is_some_and(|e| super::guard::is_bug(e)) { (result, None) } else { let edits = forward(&mut s.doc, n); (result, edits) };
                        if tx.send(Done { result, edits, model: s.doc.model.clone() }).is_err() {
                            break;
                        }
                        repaint();
                    }
                }
            }
        })
        .expect("agent tools thread");
    (jobs, done, progress)
}

/// Gets the slow-to-start parts ready once per run, beside the window
/// opening, so the first look the agent asks for does not wait for them:
/// the renderer's graphics context takes about a second and a half to
/// start the first time in a process, and the library of sounds and
/// pictures is read from disk.
fn warm_up() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    if cfg!(test) {
        return;
    }
    ONCE.call_once(|| {
        let _ = std::thread::Builder::new().name("studio-agent-warm".into()).spawn(|| {
            #[cfg(feature = "render")]
            drop(studio_render::GpuContext::new());
            let _ = crate::content::get_blocking();
        });
    });
}

/// The window's history revision: it moves with every change, hand edits
/// and undo included, and not with the playhead.
fn rev(doc: &Doc) -> u64 {
    doc.history.revision()
}

impl Live {
    pub fn new(repaint: impl Fn() + Send + Sync + 'static) -> Self {
        warm_up();
        let repaint: Arc<dyn Fn() + Send + Sync> = Arc::new(repaint);
        let owner = super::procs::new_owner();
        let (jobs, done, progress) = spawn_worker(repaint.clone(), owner);
        Self { owner, paused: false, floor: 0, jobs, done, progress, detached: Vec::new(), table: Vec::new(), next_job: 1, job_notes: Vec::new(), synced: None, known: None, running: None, replay: None, queue: VecDeque::new(), repaint, window: Session::default(), turn: None, request: 0, requests: 0, calls_made: 0, touched: Vec::new(), made_project: false, window_calls: Vec::new() }
    }

    /// Whether a call is running or waiting.
    pub fn busy(&self) -> bool {
        self.running.as_ref().is_some_and(|r| !r.discard) || self.replay.is_some() || !self.queue.is_empty()
    }

    /// Starts a request; returns its number, which its changes carry (the
    /// panel's "Undo this request" takes them back together).
    pub fn begin_turn(&mut self, label: &str) -> u64 {
        self.turn = Some(label.to_owned());
        self.requests += 1;
        self.request = self.requests;
        self.request
    }

    /// Ends the request: a change still playing lands now.
    pub fn end_turn(&mut self, doc: &mut Doc) {
        while self.replay.is_some() {
            self.advance(doc, true);
        }
        self.turn = None;
        self.request = 0;
    }

    /// Whose jobs and programs this window's AI runs (share it with the
    /// gate: [`Gate::set_owner`](super::brain::Gate::set_owner)).
    pub fn owner(&self) -> u64 {
        self.owner
    }

    /// A background job is still running (tests).
    #[cfg(test)]
    pub fn has_running_jobs(&self) -> bool {
        self.table.iter().any(|j| j.state == jobs::State::Running)
    }

    /// Pause: no agent call starts and no background job lands until
    /// Resume. The call under way finishes.
    pub fn set_paused(&mut self, on: bool) {
        self.paused = on;
        (self.repaint)();
    }

    /// Stop: calls not started are dropped (their callers hear so), the one
    /// running is answered as stopped at once and its change is never
    /// applied, background jobs are ended, and a change already playing
    /// onto the timeline lands at once, as the one undo step it is. What is
    /// on the timeline stays.
    pub fn cancel(&mut self, doc: &mut Doc) {
        self.cancel_at(doc, self.floor);
    }

    /// [`Live::cancel`], and calls the agents made before Stop count
    /// `epoch` are refused from now on, however late they arrive.
    pub fn cancel_at(&mut self, doc: &mut Doc, epoch: u64) {
        self.floor = self.floor.max(epoch);
        self.paused = false;
        for (c, _) in self.queue.drain(..) {
            let _ = c.reply.send(Err("The user stopped this request. Do nothing more.".into()));
        }
        if let Some(r) = self.running.as_mut() {
            r.discard = true;
            r.progress.cancel();
            // Its caller is freed now, not when the tool ends.
            let reply = std::mem::replace(&mut r.reply, mpsc::channel().0);
            let _ = reply.send(Err("The user stopped this request. Do nothing more.".into()));
        }
        // Background jobs end now: marked stopped in the job list, their
        // worker told to give up, their result never applied.
        for d in std::mem::take(&mut self.detached) {
            d.run.progress.cancel();
            self.end_job(d.id, jobs::State::Stopped, String::new());
        }
        // Jobs that go on by themselves (picture batches, renders) and the
        // programs the AI's tools started.
        jobs::cancel_owner(self.owner);
        while self.replay.is_some() {
            self.advance(doc, true);
        }
    }

    /// Queues a call as an agent's (tests: what the gate sends).
    #[cfg(test)]
    pub fn push_agent(&mut self, call: ToolCall) {
        self.queue.push_back((call, false));
    }

    /// Queues a call the person asked for (the panel's `/` commands, the
    /// viewer's menus): its change is theirs to undo.
    pub fn push(&mut self, call: ToolCall) {
        self.queue.push_back((call, true));
    }

    /// Each frame: take finished results onto the document, start the next
    /// call.
    pub fn tick(&mut self, doc: &mut Doc, calls: &Receiver<ToolCall>) {
        while let Ok(c) = calls.try_recv() {
            if c.epoch < self.floor {
                let _ = c.reply.send(Err("The user stopped this request. Do nothing more.".into()));
                continue;
            }
            self.queue.push_back((c, false));
        }
        self.answer_quick();
        if self.replay.is_some() {
            self.advance(doc, false);
            if self.replay.is_some() {
                return;
            }
        }
        if let Some(run) = self.running.take() {
            match self.done.try_recv() {
                Ok(_) if run.discard => {
                    // Stopped while it ran: the copy has changes the window
                    // will not get, so it follows the window again.
                    self.synced = None;
                    self.known = None;
                }
                Ok(d) => {
                    self.take(doc, run, d);
                    if self.replay.is_some() {
                        return;
                    }
                }
                Err(TryRecvError::Empty) => {
                    // Calls waiting behind it shorten the wait: an agent that sent
                    // several at once is not waiting on this one alone.
                    if run.long.is_some() && !run.discard && Instant::now() >= run.started_at + jobs::grace_with(self.queue.len()) {
                        self.detach(run);
                    } else {
                        if run.long.is_some() {
                            // Wake up again to move it to the background in time.
                            (self.repaint)();
                        }
                        self.running = Some(run);
                        return;
                    }
                }
                Err(TryRecvError::Disconnected) => {
                    let _ = run.reply.send(Err("the agent's worker stopped; reopen the panel".into()));
                }
            }
        }
        self.poll_detached(doc);
        if self.replay.is_some() {
            return;
        }
        while self.running.is_none() {
            // Paused: what the person asks for runs, what the agents ask waits.
            if self.paused && self.queue.front().is_some_and(|(_, person)| !person) {
                break;
            }
            let Some((c, by_person)) = self.queue.pop_front() else { break };
            if ON_WINDOW_UI.contains(&c.tool.as_str()) {
                self.window_calls.push(c);
                continue;
            }
            if ON_WINDOW.contains(&c.tool.as_str()) {
                let r = self.on_window(doc, &c.tool, &c.args, by_person);
                let _ = c.reply.send(r);
                continue;
            }
            let mut notes = std::mem::take(&mut self.job_notes);
            if self.synced != Some(rev(doc)) {
                // What changed in the window since the agent last looked.
                if !by_person && let Some(known) = &self.known {
                    notes.extend(outside_changes(known, &doc.model));
                }
                let _ = self.jobs.send(Job::Sync(doc.model.clone(), doc.folder.clone()));
                self.synced = Some(rev(doc));
                self.known = Some(doc.model.clone());
            }
            // The copy numbers what it makes from here; the window from
            // past the block.
            let first_id = doc.model.next_id;
            doc.model.next_id += ID_BLOCK;
            let long = if by_person { None } else { jobs::limit_of(&c.tool, &c.args) };
            let (tool_name, brief) = (c.tool.clone(), jobs::brief(&c.tool, &c.args));
            // A long call is landed by a three-way merge when it ends, so it
            // needs the project as it was when it started.
            let long_base = long.map(|_| doc.model.clone());
            if self.jobs.send(Job::Call(c.tool, c.args, first_id)).is_err() {
                let _ = c.reply.send(Err("the agent's worker stopped; reopen the panel".into()));
                continue;
            }
            self.calls_made += 1;
            let author = if by_person { m::Author::Person } else { m::Author::Agent { request: self.request, call: self.calls_made } };
            let now = Instant::now();
            let base = self.known.take().or(long_base);
            self.running = Some(Running { reply: c.reply, long, started_at: now, tool: tool_name, brief, progress: self.progress.clone(), discard: false, author, started: rev(doc), base, notes });
        }
    }

    /// Answers, at once and whatever is running, the calls that read nothing
    /// of the project: the guide, the help, the field lists, the capability
    /// search and the job list. They used to wait their turn behind a render
    /// or an edit, which an agent sees as a hang on a call that costs
    /// nothing. Everything else keeps its order.
    fn answer_quick(&mut self) {
        if !self.queue.iter().any(|(c, _)| is_quick(&c.tool)) {
            return;
        }
        let mut rest = VecDeque::with_capacity(self.queue.len());
        while let Some((c, by_person)) = self.queue.pop_front() {
            if !is_quick(&c.tool) {
                rest.push_back((c, by_person));
                continue;
            }
            let r = if c.tool == "job" {
                jobs::status(&self.table, &c.args).map(|v| vec![Content::Text(v.to_string())])
            } else {
                super::guard::run(&c.tool, || self.window.run(&c.tool, &c.args))
            };
            let notes = std::mem::take(&mut self.job_notes);
            let _ = c.reply.send(with_notes(r, notes));
        }
        self.queue = rest;
    }

    /// Moves the running long call to the background: its worker goes on
    /// with it, a new worker takes the calls that follow, and the agent
    /// is answered now with the job's number.
    fn detach(&mut self, mut run: Running) {
        let id = self.next_job;
        self.next_job += 1;
        let limit = run.long.unwrap_or(Duration::from_secs(600));
        self.table.push(jobs::Info { id, tool: run.tool.clone(), brief: run.brief.clone(), started: run.started_at, took: None, limit, state: jobs::State::Running, progress: run.progress.clone(), result: String::new() });
        // Old finished jobs are forgotten, running ones never.
        while self.table.iter().filter(|j| j.state != jobs::State::Running).count() > 30 {
            if let Some(i) = self.table.iter().position(|j| j.state != jobs::State::Running) {
                self.table.remove(i);
            }
        }
        let (jobs_tx, done, progress) = spawn_worker(self.repaint.clone(), self.owner);
        let old_jobs = std::mem::replace(&mut self.jobs, jobs_tx);
        let old_done = std::mem::replace(&mut self.done, done);
        self.progress = progress;
        // The new worker's copy follows the window again.
        self.synced = None;
        let early = serde_json::json!({
            "job": id,
            "state": "running",
            "what": run.brief,
            "message": format!("{} is still working after {} s, so it carries on in the background (time limit {} minutes). Other tools work meanwhile. `job` with id {id} shows its progress and, when done, its result; you are also told in the answer to your next call when it finishes. Its change lands in the project when it is done.", run.tool, run.started_at.elapsed().as_secs().max(1), limit.as_secs() / 60),
        });
        let reply = std::mem::replace(&mut run.reply, mpsc::channel().0);
        let _ = reply.send(Ok(vec![Content::Text(early.to_string())]));
        let deadline = run.started_at + limit;
        self.detached.push(Detached { id, run, _jobs: old_jobs, done: old_done, deadline });
    }

    /// Background jobs: gives up on the ones past their limit, and lands
    /// the ones that finished (between calls, merged with what happened in
    /// the window meanwhile).
    fn poll_detached(&mut self, doc: &mut Doc) {
        let now = Instant::now();
        let mut i = 0;
        while i < self.detached.len() {
            let id = self.detached[i].id;
            if now >= self.detached[i].deadline {
                self.detached.remove(i);
                self.end_job(id, jobs::State::TimedOut, String::new());
                continue;
            }
            if self.running.is_some() || self.replay.is_some() || self.paused {
                i += 1;
                continue;
            }
            match self.detached[i].done.try_recv() {
                Ok(done) => {
                    let d = self.detached.remove(i);
                    let text = match &done.result {
                        Ok(parts) => parts.iter().filter_map(|p| if let Content::Text(t) = p { Some(t.as_str()) } else { None }).collect::<Vec<_>>().join("\n"),
                        Err(e) => e.clone(),
                    };
                    let state = if d.run.discard {
                        jobs::State::Stopped
                    } else if let Err(e) = &done.result {
                        jobs::State::Failed(e.clone())
                    } else {
                        jobs::State::Done
                    };
                    let apply = state == jobs::State::Done;
                    self.end_job(id, state, text);
                    self.synced = None;
                    if apply {
                        let mut run = d.run;
                        // Always merged: the window has moved on since it started.
                        run.started = u64::MAX - 1;
                        run.reply = mpsc::channel().0;
                        run.notes.clear();
                        self.take(doc, run, done);
                        self.synced = None;
                    }
                    return;
                }
                Err(TryRecvError::Empty) => i += 1,
                Err(TryRecvError::Disconnected) => {
                    self.detached.remove(i);
                    self.end_job(id, jobs::State::Failed("the worker running it stopped".into()), String::new());
                }
            }
        }
    }

    /// Records how a background job ended and queues the word to the agent.
    fn end_job(&mut self, id: u32, state: jobs::State, result: String) {
        if let Some(j) = self.table.iter_mut().find(|j| j.id == id) {
            j.took = Some(j.started.elapsed());
            j.state = state;
            j.result = result;
            let told = j.told();
            if !told.is_empty() {
                self.job_notes.push(told);
            }
        }
        (self.repaint)();
    }

    /// Starts playing what a call changed onto the window's document (or
    /// answers at once when it changed nothing).
    fn take(&mut self, doc: &mut Doc, run: Running, d: Done) {
        let Some(edits) = d.edits else {
            self.synced = None;
            self.settle(run.author, run.base, d.model);
            let _ = run.reply.send(with_notes(d.result, run.notes));
            return;
        };
        let mut left = VecDeque::new();
        for (label, e) in edits {
            flatten(&label, e, &mut left);
        }
        if left.is_empty() {
            self.settle(run.author, run.base, d.model);
            let _ = run.reply.send(with_notes(d.result, run.notes));
            return;
        }
        let per_step = left.len().div_ceil(MAX_STEPS).max(1);
        // The window moved while the call ran: the change is merged in at
        // once rather than played over what the person did.
        let expect = if rev(doc) == run.started { rev(doc) } else { u64::MAX };
        let own_batch = run.author.is_person() && expect != u64::MAX && !doc.in_batch();
        if own_batch {
            doc.begin_batch(&left[0].0.clone());
        }
        // An agent does not wait for the picture to build on the timeline:
        // the call is done, its answer is true, and the next call starts
        // only when the last piece has landed (see `tick`), so nothing it
        // asks next can see a half-built window. Playing a big change out
        // takes up to a couple of seconds, which the agent used to wait
        // for on every edit.
        let early = !run.author.is_person() && expect != u64::MAX;
        let (reply, result, notes) = if early {
            let _ = run.reply.send(with_notes(d.result, run.notes));
            (mpsc::channel().0, Ok(Vec::new()), Vec::new())
        } else {
            (run.reply, d.result, run.notes)
        };
        self.replay = Some(Replay { reply, result, author: run.author, base: run.base, model: d.model, left, per_step, next_at: Instant::now(), expect, notes, own_batch, early, applied: Vec::new() });
        self.advance(doc, false);
    }

    /// Plays the next piece of the change (`all`: the rest at once); when
    /// it has all landed, answers the call.
    fn advance(&mut self, doc: &mut Doc, all: bool) {
        let Some(rp) = self.replay.as_mut() else { return };
        if rp.expect != rev(doc) {
            // The person changed something meanwhile: the rest lands merged
            // with it, theirs kept where both changed the same thing.
            let rp = self.replay.take().expect("replay");
            return self.land_merged(doc, rp);
        }
        if !all && Instant::now() < rp.next_at {
            (self.repaint)();
            return;
        }
        // A drag the person has going becomes its own step first, so
        // cancelling it (Esc) cannot take this change with it, and nothing
        // of theirs joins the agent's step.
        doc.end_gesture();
        let want = if all { rp.left.len() } else { rp.per_step };
        let mut n = want.min(rp.left.len());
        let label = self.turn.clone().unwrap_or_else(|| rp.left[0].0.clone());
        let author = rp.author;
        // A piece the model cannot hold on its own (in between, a lane
        // would overlap) goes with the pieces after it.
        loop {
            let chunk: Vec<m::Edit> = rp.left.iter().take(n).map(|(_, e)| e.clone()).collect();
            match doc.as_author(author, |d| d.apply_replayed(&label, m::Edit::Batch { edits: chunk })) {
                Ok(()) => {
                    let mut touched = Vec::new();
                    for (_, e) in rp.left.drain(..n) {
                        clips_of(&e, &mut touched);
                        rp.applied.push(e);
                    }
                    if !touched.is_empty() {
                        self.touched = touched;
                    }
                    break;
                }
                Err(_) if n < rp.left.len() => n += 1,
                Err(_) => {
                    // What the window will not take as made is merged in.
                    let rp = self.replay.take().expect("replay");
                    return self.land_merged(doc, rp);
                }
            }
        }
        rp.expect = rev(doc);
        rp.next_at = Instant::now() + STEP;
        if !rp.left.is_empty() {
            (self.repaint)();
            return;
        }
        let rp = self.replay.take().expect("replay");
        if rp.own_batch && doc.in_batch() {
            doc.commit_batch();
        }
        doc.model.next_id = doc.model.next_id.max(rp.model.next_id);
        self.synced = Some(rev(doc));
        self.settle(rp.author, rp.base, rp.model);
        self.finish(rp.early, rp.reply, rp.result, rp.notes);
    }

    /// Answers a call whose change has landed; an agent that was answered
    /// already hears what came up since with its next answer.
    fn finish(&mut self, early: bool, reply: Sender<ToolResult>, result: ToolResult, notes: Vec<String>) {
        if early {
            self.job_notes.extend(notes);
        } else {
            let _ = reply.send(with_notes(result, notes));
        }
    }

    /// After a call: the copy is `model`. An agent now knows the project
    /// as that; a call the person made leaves what the agent knew (`base`),
    /// so it hears of the change at its next call.
    fn settle(&mut self, author: m::Author, base: Option<m::Project>, model: m::Project) {
        if author.is_person() {
            self.known = base;
            self.synced = None;
        } else {
            self.known = Some(model);
        }
    }

    /// Lands a call's change merged with what the person did while it ran:
    /// one step, theirs kept on what both changed; the agent is told.
    fn land_merged(&mut self, doc: &mut Doc, mut rp: Replay) {
        if rp.own_batch && doc.in_batch() {
            doc.commit_batch();
        }
        let base = rp.base.take().unwrap_or_else(|| doc.model.clone());
        // The pieces that already landed are in the window and in the agent's
        // copy alike. Merged against the old base they look like objects both
        // sides made new with the same ids, and the merge makes a second set
        // of them. Against the base with them applied, what differs in the
        // window is the person's work alone, and what differs in the copy is
        // the pieces still to come: each object is there once, by id. The
        // agent is told of the person's work, not of its own pieces.
        let mut from = base.clone();
        for e in &rp.applied {
            if from.apply(e.clone()).is_err() {
                from = base.clone();
                break;
            }
        }
        if !rp.author.is_person() {
            rp.notes.extend(outside_changes(&from, &doc.model));
        }
        match crate::collab::merge::three_way(&from, &doc.model, &rp.model) {
            Ok(out) => {
                let edits = crate::collab::delta::edits_between(&doc.model, &out.merged);
                let label = self.turn.clone().unwrap_or_else(|| "AI change".into());
                doc.end_gesture();
                if !edits.is_empty()
                    && let Err(e) = doc.as_author(rp.author, |d| d.apply_replayed(&label, m::Edit::Batch { edits }))
                {
                    rp.notes.push(format!("Note: the project changed in the window while this ran, and this change could not be applied ({e}). Read `summary` again before going on."));
                }
                if !out.conflicts.is_empty() {
                    let kept: Vec<String> = out.conflicts.iter().take(8).map(|c| format!("{} {}{}", c.what, c.id, if c.name.is_empty() { String::new() } else { format!(" ({})", c.name) })).collect();
                    rp.notes.push(format!("Note: the person changed the same things while this ran; their version is kept on {}. Work with it rather than redo it.", kept.join(", ")));
                }
                for f in out.fixes.iter().take(4) {
                    rp.notes.push(format!("Note: {f}"));
                }
                doc.model.next_id = doc.model.next_id.max(out.merged.next_id);
            }
            Err(e) => rp.notes.push(format!("Note: the project changed in the window while this ran, and this change could not be merged with it ({e}). Read `summary` again before going on.")),
        }
        // The copy has the agent's version: it follows the window again.
        self.synced = None;
        self.known = if rp.author.is_person() { Some(base) } else { Some(doc.model.clone()) };
        self.finish(rp.early, rp.reply, rp.result, rp.notes);
    }

    fn on_window(&mut self, doc: &mut Doc, tool: &str, args: &Value, by_person: bool) -> ToolResult {
        self.synced = None;
        // Undo and redo take back the asker's own work only: an agent's
        // its last call, never what the person did.
        if matches!(tool, "undo" | "redo") {
            let r = match (tool, by_person) {
                ("undo", true) => Ok(doc.undo()),
                ("redo", true) => Ok(doc.redo()),
                ("undo", false) => doc.undo_agent_call(),
                _ => doc.redo_agent_call(),
            };
            let key = if tool == "undo" { "undone" } else { "redone" };
            return r.map(|l| vec![Content::Text(serde_json::json!({key: l, "revision": rev(doc)}).to_string())]);
        }
        std::mem::swap(doc, &mut self.window.doc);
        let r = super::guard::run(tool, || self.window.run(tool, args));
        std::mem::swap(doc, &mut self.window.doc);
        if tool == "new_project" && r.is_ok() {
            self.made_project = true;
        }
        r
    }
}

/// A tool's answer with notes for the agent after it.
fn with_notes(r: ToolResult, notes: Vec<String>) -> ToolResult {
    r.map(|mut parts| {
        parts.extend(notes.into_iter().map(Content::Text));
        parts
    })
}

/// What changed from `known` (the project as the agent last saw it) to
/// `now`, said to the agent; `None` when nothing did.
fn outside_changes(known: &m::Project, now: &m::Project) -> Option<String> {
    use m::Edit::*;
    let edits = crate::collab::delta::edits_between(known, now);
    if edits.is_empty() {
        return None;
    }
    let (mut changed, mut added, mut gone, mut media_gone, mut other) = (Vec::new(), Vec::new(), Vec::new(), Vec::new(), 0usize);
    for e in &edits {
        match e {
            SetClip { clip, .. } => changed.push(clip.id.0),
            InsertClip { clip, .. } => added.push(clip.id.0),
            RemoveClip { clip, .. } => gone.push(clip.0),
            RemoveMedia { id } => media_gone.push(id.0),
            _ => other += 1,
        }
    }
    let list = |what: &str, ids: &[u64]| -> Option<String> {
        if ids.is_empty() {
            return None;
        }
        let shown: Vec<String> = ids.iter().take(12).map(u64::to_string).collect();
        Some(format!("{what} {}{}", shown.join(", "), if ids.len() > 12 { format!(" and {} more", ids.len() - 12) } else { String::new() }))
    };
    let parts: Vec<String> = [list("clips changed:", &changed), list("clips added:", &added), list("clips gone:", &gone), list("media gone:", &media_gone), (other > 0).then(|| format!("{other} other changes (media, lanes, markers or settings)"))].into_iter().flatten().collect();
    Some(format!("Note: the project in the window changed outside your work since your last call (the person's edits or Undo): {}. Their changes stand: build on them, and do not use ids that are gone. `summary` shows the project as it is now.", parts.join("; ")))
}

/// An edit as its primitive edits, in order.
fn flatten(label: &str, e: m::Edit, out: &mut VecDeque<(String, m::Edit)>) {
    match e {
        m::Edit::Batch { edits } => edits.into_iter().for_each(|e| flatten(label, e, out)),
        e => out.push_back((label.to_owned(), e)),
    }
}

/// The clips an edit adds or changes.
fn clips_of(e: &m::Edit, out: &mut Vec<u32>) {
    match e {
        m::Edit::InsertClip { clip, .. } | m::Edit::SetClip { clip, .. } => out.push(clip.id.0 as u32),
        m::Edit::Batch { edits } => edits.iter().for_each(|e| clips_of(e, out)),
        _ => {}
    }
}

/// The edits behind the undo steps added since `n`, forward and oldest
/// first: each step's inverse applied to a copy gives back its own inverse,
/// the edit as it was made.
pub(crate) fn forward(doc: &mut Doc, n: usize) -> Option<Vec<(String, m::Edit)>> {
    let saved = doc.history.snapshot();
    let steps = saved.undo.get(n..)?;
    let mut model = doc.model.clone();
    let mut out = Vec::with_capacity(steps.len());
    for s in steps.iter().rev() {
        out.push((s.label.clone(), model.apply(s.inverse.clone()).ok()?));
    }
    out.reverse();
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn call(live: &mut Live, doc: &mut Doc, calls: &Receiver<ToolCall>, tool: &str, args: Value) -> ToolResult {
        let (tx, rx) = mpsc::channel();
        live.push_agent(ToolCall { tool: tool.into(), args, reply: tx, epoch: 0 });
        // Waits for the answer, not for a number of ticks: some calls do real
        // work (flatten renders and encodes a 1080p file), and how long that
        // takes depends on the load the other tests put on the machine. The
        // limit is only a guard against a call that never answers, set as far
        // from any loaded run as the export tests' own.
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(300) {
            live.tick(doc, calls);
            if let Ok(r) = rx.try_recv() {
                // An agent is answered before its change has played out; its
                // next call starts after that, so a test reading the window
                // does the same.
                while live.replay.is_some() {
                    live.tick(doc, calls);
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                return r;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("{tool} did not finish");
    }

    fn text(r: ToolResult) -> String {
        match r.expect("tool ok").into_iter().next() {
            Some(Content::Text(t)) => t,
            _ => String::new(),
        }
    }

    /// A tool that takes `ms` and then makes a card (tests only).
    fn push_slow(live: &mut Live, ms: u64, limit_ms: u64) -> mpsc::Receiver<ToolResult> {
        let (tx, rx) = mpsc::channel();
        live.push_agent(ToolCall { tool: "__slow".into(), args: json!({"ms": ms, "limit_ms": limit_ms, "edit": [{"op": "card", "colour": "#00ff00", "at": 0.0, "duration": 1.0}]}), reply: tx, epoch: 0 });
        rx
    }

    fn tick_until(live: &mut Live, doc: &mut Doc, calls: &Receiver<ToolCall>, what: &str, mut done: impl FnMut(&Live, &Doc) -> bool) {
        // A guard against waiting forever, not a budget: see `call`.
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(300) {
            live.tick(doc, calls);
            if done(live, doc) {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("{what} did not happen");
    }

    /// A long call goes to the background: the AI is answered at once with a
    /// job, other tools work meanwhile, the change lands when it finishes
    /// and the AI is told in its next answer.
    #[test]
    fn a_long_call_runs_in_the_background_while_other_calls_work() {
        let mut doc = Session::default().doc;
        let (_tx, calls) = mpsc::channel();
        let mut live = Live::new(|| {});
        let gate = crate::agent::test_gate::new();
        let slow = push_slow_gated(&mut live, gate, 60_000);
        let mut early = None;
        tick_until(&mut live, &mut doc, &calls, "the early answer", |_, _| {
            early = early.take().or_else(|| slow.try_recv().ok());
            early.is_some()
        });
        let early = text(early.unwrap());
        assert!(early.contains("\"job\":1") && early.contains("background"), "{early}");
        // Other calls are answered meanwhile (the slow one is still going).
        // (The gate is shut: the call cannot finish until the test opens it.)
        let s = text(call(&mut live, &mut doc, &calls, "summary", json!({})));
        assert!(s.contains("\"revision\""), "{s}");
        assert!(doc.project.clips.is_empty(), "the card has not landed yet");
        let status = text(call(&mut live, &mut doc, &calls, "job", json!({"id": 1})));
        assert!(status.contains("\"state\":\"running\""), "{status}");
        // It finishes and its change lands.
        crate::agent::test_gate::open(gate);
        tick_until(&mut live, &mut doc, &calls, "the card landing", |l, d| l.table.iter().all(|j| j.state != jobs::State::Running) && l.replay.is_none() && !d.project.clips.is_empty());
        // And the next answer says it finished.
        let next = match call(&mut live, &mut doc, &calls, "summary", json!({})) {
            Ok(parts) => parts.iter().filter_map(|p| if let Content::Text(t) = p { Some(t.clone()) } else { None }).collect::<Vec<_>>().join("\n"),
            Err(e) => e,
        };
        assert!(next.contains("background job 1") && next.contains("finished"), "{next}");
        let status = text(call(&mut live, &mut doc, &calls, "job", json!({"id": "1"})));
        assert!(status.contains("\"state\":\"done\"") && status.contains("result"), "{status}");
    }

    /// [`push_slow`], held until the test opens `gate`.
    fn push_slow_gated(live: &mut Live, gate: u64, limit_ms: u64) -> mpsc::Receiver<ToolResult> {
        let (tx, rx) = mpsc::channel();
        live.push_agent(ToolCall { tool: "__slow".into(), args: json!({"ms": 0, "gate": gate, "limit_ms": limit_ms, "edit": [{"op": "card", "colour": "#00ff00", "at": 0.0, "duration": 1.0}]}), reply: tx, epoch: 0 });
        rx
    }

    /// A call that is not a background job and takes a while (tests only).
    fn push_block(live: &mut Live, ms: u64) -> mpsc::Receiver<ToolResult> {
        let (tx, rx) = mpsc::channel();
        live.push_agent(ToolCall { tool: "__block".into(), args: json!({"ms": ms}), reply: tx, epoch: 0 });
        rx
    }

    /// The guide, the help, the field lists and the job list read nothing of
    /// the project: they are answered at once, not after the render or the
    /// edit in front of them.
    #[test]
    fn calls_that_read_nothing_do_not_wait_behind_a_running_call() {
        let mut doc = Session::default().doc;
        let (_tx, calls) = mpsc::channel();
        let mut live = Live::new(|| {});
        let slow = push_block(&mut live, 900);
        live.tick(&mut doc, &calls);
        let started = Instant::now();
        let mut answers = Vec::new();
        for (tool, args) in [("guide", json!({"topic": "motion"})), ("edit_schema", json!({})), ("job", json!({})), ("capabilities", json!({"query": "fade"})), ("howto", json!({"query": "undo"}))] {
            let (tx, rx) = mpsc::channel();
            live.push_agent(ToolCall { tool: tool.into(), args, reply: tx, epoch: 0 });
            answers.push((tool, rx));
        }
        let mut got = Vec::new();
        while got.len() < answers.len() && started.elapsed() < Duration::from_millis(700) {
            live.tick(&mut doc, &calls);
            for (tool, rx) in &answers {
                if let Ok(r) = rx.try_recv() {
                    got.push((*tool, r));
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(got.len(), answers.len(), "all five answered while the block still ran: {:?}", got.iter().map(|g| g.0).collect::<Vec<_>>());
        assert!(slow.try_recv().is_err(), "the long call was still running");
        assert!(got.iter().all(|(_, r)| r.is_ok()));
        // Answered with text, whatever the guide's wording is.
        let at = got.iter().position(|g| g.0 == "guide").unwrap();
        assert!(!text(got.remove(at).1).is_empty(), "the guide answered with text");
        // The ordinary calls still wait for their turn, in order.
        let s = text(call(&mut live, &mut doc, &calls, "summary", json!({})));
        assert!(s.contains("\"revision\""));
        assert!(slow.try_recv().is_ok(), "the block was answered before the summary");
    }

    /// Several long calls sent together start together: each one waits only
    /// a moment for the calls queued behind it, not the whole grace.
    #[test]
    fn long_calls_sent_together_start_together() {
        assert_eq!(jobs::grace_with(0), jobs::grace());
        assert!(jobs::grace_with(1) < jobs::grace());
        let mut doc = Session::default().doc;
        let (_tx, calls) = mpsc::channel();
        let mut live = Live::new(|| {});
        let rxs: Vec<_> = (0..4).map(|_| push_slow(&mut live, 3000, 60_000)).collect();
        let started = Instant::now();
        // Three of the four are on their way once each has been let go in
        // turn; with the whole grace for each it would take 3 x 150 ms.
        tick_until(&mut live, &mut doc, &calls, "three jobs started", |l, _| l.table.len() >= 3);
        assert!(started.elapsed() < Duration::from_millis(330), "{:?}", started.elapsed());
        assert!(rxs.iter().filter(|r| r.try_recv().is_ok()).count() >= 3, "each was answered with its job");
    }

    /// The agent is answered when its call is done, not when the last piece
    /// has played onto the timeline; the window ends up the same, and the
    /// next call sees all of it.
    #[test]
    fn an_agents_answer_does_not_wait_for_the_change_to_play_out() {
        let ops: Vec<Value> = (0..100).map(|i| json!({"op": "card", "colour": "#336699", "at": i as f64, "duration": 1})).collect();
        // What the same ops make on a plain session, for comparison.
        let mut plain = Session::default();
        plain.run("edit", &json!({"ops": ops})).unwrap();
        let mut want: Vec<(i64, i64)> = plain.doc.project.clips.iter().map(|c| (c.start, c.dur)).collect();
        want.sort_unstable();

        let mut doc = Session::default().doc;
        let (_tx, calls) = mpsc::channel();
        let mut live = Live::new(|| {});
        live.begin_turn("AI: cards");
        let (tx, rx) = mpsc::channel();
        live.push_agent(ToolCall { tool: "edit".into(), args: json!({"ops": ops}), reply: tx, epoch: 0 });
        let reply = loop {
            live.tick(&mut doc, &calls);
            if let Ok(r) = rx.try_recv() {
                break r;
            }
            std::thread::sleep(Duration::from_millis(2));
        };
        let said = text(reply);
        assert!(said.contains("\"ok\":true"), "{said}");
        assert!(live.replay.is_some() && doc.project.clips.len() < 100, "answered while the change was still playing ({} of 100 clips)", doc.project.clips.len());
        // The next call starts only after everything has landed, and sees it.
        let s = text(call(&mut live, &mut doc, &calls, "summary", json!({})));
        assert_eq!(doc.project.clips.len(), 100);
        assert!(s.contains("\"clips\""));
        live.end_turn(&mut doc);
        let mut got: Vec<(i64, i64)> = doc.project.clips.iter().map(|c| (c.start, c.dur)).collect();
        got.sort_unstable();
        assert_eq!(got, want, "the same clips as running the ops directly");
        assert!(doc.request_undoable(1) || !doc.can_undo(), "still taken back with the request");
    }

    /// When the person changes the window while an answered change is still
    /// playing, the merge note is not lost: it comes with the next answer.
    #[test]
    fn what_comes_up_after_an_early_answer_comes_with_the_next_one() {
        let ops: Vec<Value> = (0..100).map(|i| json!({"op": "card", "colour": "#336699", "at": i as f64, "duration": 1})).collect();
        let mut doc = Session::default().doc;
        let (_tx, calls) = mpsc::channel();
        let mut live = Live::new(|| {});
        live.begin_turn("AI: cards");
        let (tx, rx) = mpsc::channel();
        live.push_agent(ToolCall { tool: "edit".into(), args: json!({"ops": ops}), reply: tx, epoch: 0 });
        loop {
            live.tick(&mut doc, &calls);
            if rx.try_recv().is_ok() {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(live.replay.is_some());
        doc.edit("Marker", |p| p.markers.push(crate::view::Marker { id: 1, time: 0, note: "mine".into() }));
        let said: Vec<String> = call(&mut live, &mut doc, &calls, "summary", json!({})).unwrap().into_iter().filter_map(|c| if let Content::Text(t) = c { Some(t) } else { None }).collect();
        live.end_turn(&mut doc);
        assert!(doc.project.clips.len() >= 100, "everything landed, merged with the person's marker");
        assert_eq!(doc.project.markers.len(), 1);
        assert!(said.iter().any(|t| t.contains("outside your work")), "{said:?}");
    }

    /// Starts a 100-card edit as the agent, waits for its early answer and
    /// for the first pieces to land, and returns the answer and the number
    /// of cards on the timeline so far.
    fn start_big_edit(live: &mut Live, doc: &mut Doc, calls: &Receiver<ToolCall>) -> (mpsc::Receiver<ToolResult>, usize) {
        let ops: Vec<Value> = (0..100).map(|i| json!({"op": "card", "colour": "#336699", "at": i as f64, "duration": 1})).collect();
        let (tx, rx) = mpsc::channel();
        live.push_agent(ToolCall { tool: "edit".into(), args: json!({"ops": ops}), reply: tx, epoch: 0 });
        let mut waited = 0;
        while doc.project.clips.is_empty() && waited < 2000 {
            live.tick(doc, calls);
            std::thread::sleep(Duration::from_millis(2));
            waited += 1;
        }
        let landed = doc.project.clips.len();
        assert!(live.replay.is_some() && landed > 0 && landed < 100, "mid-replay ({landed} of 100)");
        (rx, landed)
    }

    /// The person edits while a big change is still being played onto the
    /// timeline: the pieces already played are not made a second time, the
    /// AI's clips are there exactly once, and the person's edit is kept.
    #[test]
    fn a_hand_edit_mid_replay_does_not_duplicate_the_pieces_already_played() {
        let mut doc = Session::default().doc;
        let (_tx, calls) = mpsc::channel();
        let mut live = Live::new(|| {});
        live.begin_turn("AI: cards");
        let (rx, _) = start_big_edit(&mut live, &mut doc, &calls);
        doc.edit("Marker", |p| p.markers.push(crate::view::Marker { id: 1, time: 0, note: "mine".into() }));
        let said: Vec<String> = call(&mut live, &mut doc, &calls, "summary", json!({})).unwrap().into_iter().filter_map(|c| if let Content::Text(t) = c { Some(t) } else { None }).collect();
        live.end_turn(&mut doc);
        assert!(rx.try_recv().is_ok(), "the edit was answered");
        assert_eq!(doc.project.clips.len(), 100, "the AI's clips, once each");
        let mut starts: Vec<i64> = doc.project.clips.iter().map(|c| c.start).collect();
        starts.sort_unstable();
        starts.dedup();
        assert_eq!(starts.len(), 100, "no clip twice");
        let mut ids: Vec<u32> = doc.project.clips.iter().map(|c| c.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 100);
        assert_eq!(doc.project.markers.len(), 1, "the person's marker is kept");
        assert_eq!(doc.project.markers[0].note, "mine");
        // The AI is told of the marker, and not of its own clips as the person's.
        let note = said.iter().find(|t| t.contains("outside your work")).unwrap_or_else(|| panic!("{said:?}"));
        assert!(note.contains("1 other changes"), "{note}");
        assert!(!note.contains("clips added"), "its own pieces are not the person's change: {note}");
        doc.model.check_invariants().unwrap();
    }

    /// The person changes a clip the AI has already placed, mid-replay: it
    /// stays as they left it, and the AI hears which clip.
    #[test]
    fn a_hand_edit_to_a_clip_already_played_is_kept_and_told() {
        let mut doc = Session::default().doc;
        let (_tx, calls) = mpsc::channel();
        let mut live = Live::new(|| {});
        live.begin_turn("AI: cards");
        let (_rx, _) = start_big_edit(&mut live, &mut doc, &calls);
        let id = doc.project.clips[0].id;
        doc.edit("Opacity", |p| p.clips.iter_mut().find(|c| c.id == id).unwrap().transform.opacity = 0.25);
        let said: Vec<String> = call(&mut live, &mut doc, &calls, "summary", json!({})).unwrap().into_iter().filter_map(|c| if let Content::Text(t) = c { Some(t) } else { None }).collect();
        live.end_turn(&mut doc);
        assert_eq!(doc.project.clips.len(), 100);
        assert_eq!(doc.project.clips.iter().find(|c| c.id == id).unwrap().transform.opacity, 0.25);
        let note = said.iter().find(|t| t.contains("outside your work")).unwrap_or_else(|| panic!("{said:?}"));
        assert!(note.contains(&format!("clips changed: {id}")), "{note}");
    }

    /// After a mid-replay merge each side still undoes only its own work.
    #[test]
    fn undo_still_works_for_each_side_after_a_mid_replay_merge() {
        let mut doc = Session::default().doc;
        let (_tx, calls) = mpsc::channel();
        let mut live = Live::new(|| {});
        let request = live.begin_turn("AI: cards");
        let (_rx, _) = start_big_edit(&mut live, &mut doc, &calls);
        doc.edit("Marker", |p| p.markers.push(crate::view::Marker { id: 1, time: 0, note: "mine".into() }));
        call(&mut live, &mut doc, &calls, "summary", json!({})).unwrap();
        live.end_turn(&mut doc);
        assert_eq!(doc.project.clips.len(), 100);
        // The person's undo takes the marker and leaves the cards.
        assert_eq!(doc.undo().as_deref(), Some("Marker"));
        assert!(doc.project.markers.is_empty());
        assert_eq!(doc.project.clips.len(), 100, "the AI's clips stay");
        assert!(doc.redo().is_some());
        assert_eq!(doc.project.markers.len(), 1);
        // The request's Undo takes every card and leaves the marker.
        doc.undo_request(request).unwrap();
        assert!(doc.project.clips.is_empty(), "{} clips left", doc.project.clips.len());
        assert_eq!(doc.project.markers.len(), 1, "the person's marker stays");
    }

    /// A job past its time limit is given up on: nothing of it is applied.
    #[test]
    fn a_job_past_its_time_limit_is_given_up_on() {
        let mut doc = Session::default().doc;
        let (_tx, calls) = mpsc::channel();
        let mut live = Live::new(|| {});
        let slow = push_slow(&mut live, 1500, 400);
        tick_until(&mut live, &mut doc, &calls, "the early answer", |_, _| false || slow.try_recv().is_ok());
        tick_until(&mut live, &mut doc, &calls, "the limit", |l, _| l.table.iter().any(|j| j.state == jobs::State::TimedOut));
        let status = text(call(&mut live, &mut doc, &calls, "job", json!({})));
        assert!(status.contains("timed_out"), "{status}");
        std::thread::sleep(Duration::from_millis(1700));
        live.tick(&mut doc, &calls);
        assert!(doc.project.clips.is_empty(), "a job that was given up on changes nothing");
        // The worker for later calls is a fresh one and works.
        text(call(&mut live, &mut doc, &calls, "summary", json!({})));
    }

    #[test]
    fn edits_land_on_the_window_document_as_one_undo_step() {
        let mut doc = Session::default().doc;
        let (_tx, calls) = mpsc::channel();
        let mut live = Live::new(|| {});
        let request = live.begin_turn("AI: two cards");
        call(&mut live, &mut doc, &calls, "edit", json!({"ops": [{"op": "card", "colour": "red", "at": 0, "duration": 2}]})).unwrap();
        call(&mut live, &mut doc, &calls, "edit", json!({"ops": [{"op": "card", "colour": "#2050e0", "at": 2, "duration": 2}]})).unwrap();
        live.end_turn(&mut doc);
        assert_eq!(doc.project.clips.len(), 2, "both cards are in the window");
        assert!(!doc.can_undo(), "the person's Undo has nothing of the AI's");
        assert!(doc.request_undoable(request));
        doc.undo_request(request).unwrap();
        assert!(doc.project.clips.is_empty(), "Undo this request takes it all back");
    }

    /// The person's Undo and the AI's are separate: ⌘Z takes back the
    /// person's own trim made after the AI's card, never the card; the
    /// request's Undo takes the card and leaves the person's later work.
    #[test]
    fn the_person_and_the_ai_undo_their_own_work() {
        let mut doc = Session::default().doc;
        let (_tx, calls) = mpsc::channel();
        let mut live = Live::new(|| {});
        doc.edit("Marker", |p| p.markers.push(crate::view::Marker { id: 1, time: 0, note: "mine".into() }));
        let request = live.begin_turn("AI: card");
        call(&mut live, &mut doc, &calls, "edit", json!({"ops": [{"op": "card", "colour": "red", "at": 0, "duration": 2}]})).unwrap();
        live.end_turn(&mut doc);
        doc.edit("Rename", |p| p.markers[0].note = "renamed".into());
        assert_eq!(doc.undo().as_deref(), Some("Rename"));
        assert_eq!(doc.project.markers[0].note, "mine");
        assert_eq!(doc.project.clips.len(), 1, "the AI's card stays");
        // The person's earlier step, from under the AI's card.
        assert_eq!(doc.undo().as_deref(), Some("Marker"));
        assert!(doc.project.markers.is_empty());
        assert_eq!(doc.project.clips.len(), 1, "the AI's card still stays");
        assert_eq!(doc.redo().as_deref(), Some("Marker"));
        doc.edit("Again", |p| p.markers[0].note = "again".into());
        doc.undo_request(request).unwrap();
        assert!(doc.project.clips.is_empty(), "the request is taken back");
        assert_eq!(doc.project.markers[0].note, "again", "what the person did since stays");
    }

    /// The person changes a clip while the AI's call that changes the same
    /// clip runs: the person's version is kept, the AI's other changes land,
    /// and the AI is told.
    #[test]
    fn a_hand_edit_during_a_call_wins_and_the_agent_hears_of_it() {
        let mut doc = Session::default().doc;
        let (_tx, calls) = mpsc::channel();
        let mut live = Live::new(|| {});
        call(&mut live, &mut doc, &calls, "edit", json!({"ops": [{"op": "card", "colour": "red", "at": 0, "duration": 2}]})).unwrap();
        let id = doc.project.clips[0].id;
        live.begin_turn("AI: fade and card");
        let (tx, rx) = mpsc::channel();
        live.push_agent(ToolCall { tool: "edit".into(), args: json!({"ops": [{"op": "set", "clip": id, "opacity": 40}, {"op": "card", "colour": "blue", "at": 4, "duration": 1}]}), reply: tx, epoch: 0 });
        live.tick(&mut doc, &calls);
        // While it runs, the person moves the same clip.
        doc.edit("Move", |p| p.clips.iter_mut().find(|c| c.id == id).unwrap().start = crate::view::SEC);
        let r = loop {
            live.tick(&mut doc, &calls);
            if let Ok(r) = rx.try_recv() {
                break r;
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        let said: Vec<String> = r.unwrap().into_iter().filter_map(|c| if let Content::Text(t) = c { Some(t) } else { None }).collect();
        live.end_turn(&mut doc);
        let c = doc.project.clips.iter().find(|c| c.id == id).unwrap();
        assert_eq!(c.start, crate::view::SEC, "the person's move stands");
        assert_eq!(doc.project.clips.len(), 2, "the AI's new card landed");
        assert!(said.iter().any(|t| t.contains("outside your work") || t.contains("their version is kept")), "{said:?}");
        // The next call hears nothing new.
        let quiet: Vec<String> = call(&mut live, &mut doc, &calls, "summary", json!({})).unwrap().into_iter().filter_map(|c| if let Content::Text(t) = c { Some(t) } else { None }).collect();
        assert_eq!(quiet.len(), 1, "{quiet:?}");
    }

    /// A hand edit between calls is told to the agent with its next answer.
    #[test]
    fn the_agent_is_told_of_hand_edits_between_calls() {
        let mut doc = Session::default().doc;
        let (_tx, calls) = mpsc::channel();
        let mut live = Live::new(|| {});
        call(&mut live, &mut doc, &calls, "edit", json!({"ops": [{"op": "card", "colour": "red", "at": 0, "duration": 2}]})).unwrap();
        let id = doc.project.clips[0].id;
        doc.edit("Delete", |p| p.clips.clear());
        let said: Vec<String> = call(&mut live, &mut doc, &calls, "summary", json!({})).unwrap().into_iter().filter_map(|c| if let Content::Text(t) = c { Some(t) } else { None }).collect();
        assert!(said.iter().any(|t| t.contains("outside your work") && t.contains(&format!("clips gone: {id}"))), "{said:?}");
    }

    #[test]
    fn a_big_change_appears_piece_by_piece() {
        let mut doc = Session::default().doc;
        let (_tx, calls) = mpsc::channel();
        let mut live = Live::new(|| {});
        live.begin_turn("AI: cards");
        let ops: Vec<Value> = (0..12).map(|i| json!({"op": "card", "colour": "#ff0000", "at": i as f64, "duration": 1})).collect();
        let (tx, rx) = mpsc::channel();
        live.push(ToolCall { tool: "edit".into(), args: json!({"ops": ops}), reply: tx, epoch: 0 });
        let mut seen = std::collections::BTreeSet::new();
        let done = loop {
            live.tick(&mut doc, &calls);
            seen.insert(doc.project.clips.len());
            if let Ok(r) = rx.try_recv() {
                break r;
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        done.unwrap();
        live.end_turn(&mut doc);
        assert_eq!(doc.project.clips.len(), 12);
        assert!(seen.len() > 3, "the cards arrived a few at a time: {seen:?}");
        doc.undo();
        assert!(doc.project.clips.is_empty(), "still one undo for the request");
    }

    #[test]
    fn stop_keeps_what_landed_and_drops_the_rest() {
        let mut doc = Session::default().doc;
        let (_tx, calls) = mpsc::channel();
        let mut live = Live::new(|| {});
        live.begin_turn("AI: cards");
        call(&mut live, &mut doc, &calls, "edit", json!({"ops": [{"op": "card", "colour": "#ff0000", "at": 0, "duration": 1}]})).unwrap();
        // The next call is under way when the person stops.
        let (tx, rx) = mpsc::channel();
        live.push_agent(ToolCall { tool: "edit".into(), args: json!({"ops": [{"op": "card", "colour": "#00ff00", "at": 1, "duration": 1}]}), reply: tx, epoch: 0 });
        live.tick(&mut doc, &calls);
        let (tx2, rx2) = mpsc::channel();
        live.push_agent(ToolCall { tool: "summary".into(), args: json!({}), reply: tx2, epoch: 0 });
        live.cancel(&mut doc);
        live.end_turn(&mut doc);
        assert!(!live.busy(), "nothing left to wait for");
        assert!(rx2.recv().map_or(true, |r| r.is_err()), "a call not started is refused");
        for _ in 0..400 {
            live.tick(&mut doc, &calls);
            std::thread::sleep(Duration::from_millis(5));
        }
        drop(rx);
        assert_eq!(doc.project.clips.len(), 1, "what landed stays; the stopped call is not applied");
        // The copy follows the window again for the next request.
        let s = text(call(&mut live, &mut doc, &calls, "summary", json!({})));
        assert!(!s.contains("00ff00"), "{s}");
    }

    #[test]
    fn the_agent_sees_hand_edits() {
        let mut doc = Session::default().doc;
        let (_tx, calls) = mpsc::channel();
        let mut live = Live::new(|| {});
        call(&mut live, &mut doc, &calls, "edit", json!({"ops": [{"op": "card", "colour": "red", "at": 0, "duration": 2}]})).unwrap();
        // The user deletes it by hand.
        doc.edit("Delete", |p| p.clips.clear());
        let s = text(call(&mut live, &mut doc, &calls, "summary", json!({})));
        assert!(!s.contains("red"), "the copy followed the window: {s}");
    }

    fn still(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("studio-live-media-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let pic = dir.join(name);
        std::fs::write(&pic, studio_media::png::encode(&studio_media::RgbaImage { width: 8, height: 8, pixels: vec![200; 8 * 8 * 4] })).unwrap();
        pic
    }

    fn media_ids(r: ToolResult) -> Vec<u64> {
        let v: Value = serde_json::from_str(&text(r)).unwrap();
        v["media"].as_array().unwrap().iter().filter_map(Value::as_u64).collect()
    }

    fn err_of(r: &ToolResult) -> String {
        r.as_ref().err().cloned().unwrap_or_default()
    }

    /// Media an agent imports keeps its id after a hand edit moves the
    /// window on (the copy follows the window again).
    #[test]
    fn imported_media_survives_a_hand_edit() {
        let mut doc = Session::default().doc;
        let (_tx, calls) = mpsc::channel();
        let mut live = Live::new(|| {});
        live.begin_turn("AI: picture");
        let id = media_ids(call(&mut live, &mut doc, &calls, "import", json!({"paths": [still("hand.png").display().to_string()]})))[0];
        assert!(doc.model.media.iter().any(|m| m.id.0 == id), "the window has media {id}");
        doc.edit("Marker", |p| p.markers.push(crate::view::Marker { id: 1, time: 0, note: "x".into() }));
        let placed = call(&mut live, &mut doc, &calls, "edit", json!({"ops": [{"op": "place", "media": id, "at": 0, "duration": 1}]}));
        assert!(placed.is_ok(), "{}", err_of(&placed));
        live.end_turn(&mut doc);
    }

    /// A drag the person has going while the agent's change lands, then
    /// cancelled with Esc: the drag goes back, the change stays.
    #[test]
    fn cancelling_a_drag_keeps_what_the_agent_did() {
        let mut doc = Session::default().doc;
        let (_tx, calls) = mpsc::channel();
        let mut live = Live::new(|| {});
        call(&mut live, &mut doc, &calls, "edit", json!({"ops": [{"op": "card", "colour": "red", "at": 0, "duration": 2}]})).unwrap();
        live.begin_turn("AI: picture");
        let start = doc.project.clips[0].start;
        doc.edit_gesture("drag", "Move", |p| p.clips[0].start += 1000);
        let id = media_ids(call(&mut live, &mut doc, &calls, "import", json!({"paths": [still("drag.png").display().to_string()]})))[0];
        doc.edit_gesture("drag", "Move", |p| p.clips[0].start += 1000);
        doc.cancel_gesture();
        assert_eq!(doc.project.clips[0].start, start + 1000, "the drag since the change landed goes back");
        assert!(doc.model.media.iter().any(|m| m.id.0 == id), "Esc on a drag took the agent's import {id} too");
        live.end_turn(&mut doc);
    }

    /// The reproduction of media vanishing from a project: a button
    /// that runs ops on the window's document (the Inspector's 3D and
    /// camera buttons) fails while a request is open; its rollback took the
    /// whole request back with it, so the ids the agent was given were gone
    /// at its next edit ("no media N").
    #[test]
    fn a_failed_edit_in_the_window_keeps_the_request() {
        let mut doc = Session::default().doc;
        let (_tx, calls) = mpsc::channel();
        let mut live = Live::new(|| {});
        live.begin_turn("AI: picture");
        let id = media_ids(call(&mut live, &mut doc, &calls, "import", json!({"paths": [still("failed.png").display().to_string()]})))[0];
        let mut s = Session::new(std::mem::replace(&mut doc, Session::default().doc));
        assert!(s.edit(&[json!({"op": "trim", "clip": 99999, "start": 1})], "Trim", false).is_err());
        doc = s.doc;
        assert!(doc.model.media.iter().any(|m| m.id.0 == id), "a failed window edit took the agent's import {id}");
        let placed = call(&mut live, &mut doc, &calls, "edit", json!({"ops": [{"op": "place", "media": id, "at": 0, "duration": 1}]}));
        assert!(placed.is_ok(), "{}", err_of(&placed));
        live.end_turn(&mut doc);
    }

    /// A dry run on the window's document mid-request changes nothing,
    /// the request's work included.
    #[test]
    fn a_dry_run_in_the_window_keeps_the_request() {
        let mut doc = Session::default().doc;
        let (_tx, calls) = mpsc::channel();
        let mut live = Live::new(|| {});
        live.begin_turn("AI: picture");
        let id = media_ids(call(&mut live, &mut doc, &calls, "import", json!({"paths": [still("dry.png").display().to_string()]})))[0];
        let mut s = Session::new(std::mem::replace(&mut doc, Session::default().doc));
        s.edit(&[json!({"op": "card", "colour": "blue", "at": 0, "duration": 1})], "Card", true).unwrap();
        doc = s.doc;
        assert!(doc.model.media.iter().any(|m| m.id.0 == id), "a dry run on the window took the agent's import {id}");
        live.end_turn(&mut doc);
    }

    /// Merge, flatten, import and place in one request: each step's media
    /// is in the window for the next.
    #[cfg(feature = "render")]
    #[test]
    fn flattened_and_imported_media_reach_the_window() {
        let mut doc = Session::default().doc;
        let (_tx, calls) = mpsc::channel();
        let mut live = Live::new(|| {});
        live.begin_turn("AI: scene");
        call(&mut live, &mut doc, &calls, "edit", json!({"ops": [{"op": "card", "colour": "red", "at": 0, "duration": 1}, {"op": "card", "colour": "blue", "at": 1, "duration": 1}]})).unwrap();
        let ids: Vec<u32> = doc.project.clips.iter().map(|c| c.id).collect();
        let r = call(&mut live, &mut doc, &calls, "edit", json!({"ops": [{"op": "merge", "clips": ids}]}));
        assert!(r.is_ok(), "{}", err_of(&r));
        let merged = doc.project.clips.iter().map(|c| c.id).max().unwrap();
        let r = call(&mut live, &mut doc, &calls, "edit", json!({"ops": [{"op": "flatten", "clip": merged}]}));
        assert!(r.is_ok(), "{}", err_of(&r));
        let flat = doc.project.clip(merged).and_then(|c| c.media);
        assert!(flat.is_some(), "the window shows the flattened clip");
        let id = media_ids(call(&mut live, &mut doc, &calls, "import", json!({"paths": [still("flat.png").display().to_string()]})))[0];
        let placed = call(&mut live, &mut doc, &calls, "edit", json!({"ops": [{"op": "place", "media": id, "at": 3, "duration": 1}]}));
        assert!(placed.is_ok(), "{}", err_of(&placed));
        live.end_turn(&mut doc);
        assert!(doc.model.media.iter().any(|m| m.id.0 == flat.unwrap() as u64));
    }

    #[test]
    fn undo_runs_on_the_window() {
        let mut doc = Session::default().doc;
        let (_tx, calls) = mpsc::channel();
        let mut live = Live::new(|| {});
        call(&mut live, &mut doc, &calls, "edit", json!({"ops": [{"op": "card", "colour": "red", "at": 0, "duration": 2}]})).unwrap();
        assert_eq!(doc.project.clips.len(), 1);
        // The person adds a marker after it: the agent's undo leaves it.
        doc.edit("Marker", |p| p.markers.push(crate::view::Marker { id: 1, time: 0, note: "mine".into() }));
        call(&mut live, &mut doc, &calls, "undo", json!({})).unwrap();
        assert!(doc.project.clips.is_empty());
        assert_eq!(doc.project.markers.len(), 1, "the agent's undo takes only its own work");
        call(&mut live, &mut doc, &calls, "redo", json!({})).unwrap();
        assert_eq!(doc.project.clips.len(), 1);
    }
}
