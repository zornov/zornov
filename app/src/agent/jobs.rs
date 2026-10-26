// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Long work as background jobs.
//!
//! A tool that can take minutes (a download, a transcode, scouting footage,
//! optical flow, a generation, a render) runs like any call at first. When
//! it has not finished within a few seconds it is moved to the background:
//! the AI gets a job number at once, every other tool keeps working, the
//! job's progress can be read with the `job` tool, a time limit ends it,
//! and when it finishes its change lands in the project and the AI is told
//! in the answer to its next call.

use serde_json::{Value, json};
use std::cell::RefCell;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long a long tool is waited for before it goes to the background.
const GRACE: Duration = Duration::from_secs(6);

/// [`GRACE`] (short in tests).
pub fn grace() -> Duration {
    if cfg!(test) { Duration::from_millis(150) } else { GRACE }
}

/// How long a long tool is waited for when other calls are queued behind it.
const GRACE_BUSY: Duration = Duration::from_secs(1);

/// The wait before a long tool goes to the background, given how many calls
/// wait behind it. An agent that sent several at once (a download for each
/// link, a picture for each place) is not waiting on any one of them, and
/// each used to hold up the start of the next for the whole [`GRACE`]: eight
/// pictures started 42 seconds apart end to end instead of together.
pub fn grace_with(waiting: usize) -> Duration {
    if waiting == 0 {
        return grace();
    }
    grace().min(if cfg!(test) { Duration::from_millis(30) } else { GRACE_BUSY })
}

/// Progress of the call a worker is running; long tools report into it.
#[derive(Default)]
pub struct Progress(Mutex<(f32, String)>, std::sync::atomic::AtomicBool);

impl Progress {
    pub fn set(&self, fraction: f32, message: &str) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = (fraction.clamp(0.0, 1.0), message.to_owned());
    }

    pub fn get(&self) -> (f32, String) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn clear(&self) {
        self.set(0.0, "");
        self.1.store(false, std::sync::atomic::Ordering::SeqCst);
    }

    /// Stop asked for: the call this progress belongs to should end.
    pub fn cancel(&self) {
        self.1.store(true, std::sync::atomic::Ordering::SeqCst);
    }

    pub fn cancelled(&self) -> bool {
        self.1.load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// Whether the call this thread is running was stopped (long loops check it
/// and end early).
pub fn cancelled() -> bool {
    CURRENT.with(|c| c.borrow().as_ref().is_some_and(|p| p.cancelled()))
}

type Hook = Box<dyn Fn() + Send>;

/// What ends work that goes on by itself (a picture batch, an export, a
/// queued render), by owner (see `procs`).
static HOOKS: Mutex<Vec<(u64, Hook)>> = Mutex::new(Vec::new());

/// Registers how to end a job this thread's owner started; Stop calls it.
pub fn on_cancel(f: impl Fn() + Send + 'static) {
    let mut h = HOOKS.lock().unwrap_or_else(|e| e.into_inner());
    // Jobs that ended long ago are not worth keeping the way to end.
    if h.len() > 256 {
        h.remove(0);
    }
    h.push((super::procs::owner(), Box::new(f)));
}

/// Stop: ends every job `owner` started and every program it ran. Returns
/// how many jobs and programs.
pub fn cancel_owner(owner: u64) -> usize {
    let mine: Vec<Hook> = {
        let mut h = HOOKS.lock().unwrap_or_else(|e| e.into_inner());
        let (mine, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut *h).into_iter().partition(|(o, _)| *o == owner);
        *h = rest;
        mine.into_iter().map(|(_, f)| f).collect()
    };
    for f in &mine {
        f();
    }
    mine.len() + super::procs::kill_owner(owner)
}

thread_local! {
    static CURRENT: RefCell<Option<Arc<Progress>>> = const { RefCell::new(None) };
}

/// Makes `p` the progress the calling thread's tools report into.
pub fn install(p: Arc<Progress>) {
    CURRENT.with(|c| *c.borrow_mut() = Some(p));
}

/// A long tool says how far it is (`fraction` 0..1, or 0 when unknown) and
/// what it is doing.
pub fn report(fraction: f32, message: &str) {
    CURRENT.with(|c| {
        if let Some(p) = c.borrow().as_ref() {
            p.set(fraction, message);
        }
    });
}

/// The longest a background job may run, or `None` for a call that is
/// quick and stays in the foreground.
pub fn limit_of(tool: &str, args: &Value) -> Option<Duration> {
    let mins = |m: u64| Some(Duration::from_secs(m * 60));
    let has = |k: &str| args.get(k).is_some_and(|v| !v.is_null());
    match tool {
        #[cfg(test)]
        "__slow" => Some(Duration::from_millis(args.get("limit_ms").and_then(Value::as_u64).unwrap_or(60_000))),
        "download" | "make_playable" | "convert" => mins(40),
        "search_media" if has("fetch") => mins(40),
        "generate" if args.get("estimate").and_then(Value::as_bool) != Some(true) && args.get("list").and_then(Value::as_bool) != Some(true) => mins(30),
        "enhance" | "render3d" | "bundle" | "cutdown" | "deliver" => mins(60),
        "ai" => mins(30),
        "analyse" | "analyze" => mins(15),
        "see" | "listen" | "contact_sheet" | "frame" if has("media") => mins(10),
        "queue" if args.get("action").and_then(Value::as_str) == Some("run") => mins(120),
        "export" if args.get("wait").and_then(Value::as_bool) != Some(false) => mins(120),
        "edit" => {
            let slow = ["smooth_motion", "stabilize", "track", "cut_out", "roto", "remove_background", "enhance", "mocap", "simulate", "bake"];
            let ops = args.get("ops").and_then(Value::as_array);
            ops.is_some_and(|o| o.iter().any(|op| op.get("op").and_then(Value::as_str).is_some_and(|n| slow.contains(&n)))).then(|| Duration::from_secs(45 * 60))
        }
        _ => None,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum State {
    Running,
    Done,
    Failed(String),
    TimedOut,
    Stopped,
}

/// One background job, running or finished.
pub struct Info {
    pub id: u32,
    pub tool: String,
    pub brief: String,
    pub started: Instant,
    pub took: Option<Duration>,
    pub limit: Duration,
    pub state: State,
    pub progress: Arc<Progress>,
    pub result: String,
}

impl Info {
    pub fn json(&self, with_result: bool) -> Value {
        let (fraction, message) = self.progress.get();
        let secs = self.took.unwrap_or_else(|| self.started.elapsed()).as_secs();
        let mut v = json!({
            "job": self.id,
            "tool": self.tool,
            "what": self.brief,
            "state": match &self.state { State::Running => "running", State::Done => "done", State::Failed(_) => "failed", State::TimedOut => "timed_out", State::Stopped => "stopped" },
            "elapsed_secs": secs,
            "time_limit_secs": self.limit.as_secs(),
        });
        if self.state == State::Running {
            v["progress"] = json!({"fraction": (fraction * 100.0).round() / 100.0, "doing": message});
        }
        match &self.state {
            State::Failed(e) => v["error"] = json!(e),
            State::TimedOut => v["error"] = json!(format!("it took longer than its {} minute limit and was given up on; nothing from it was applied", self.limit.as_secs() / 60)),
            State::Stopped => v["error"] = json!("the request was stopped; nothing from it was applied"),
            _ => {}
        }
        if with_result && self.state == State::Done {
            v["result"] = json!(self.result);
        }
        v
    }

    /// The line the AI is told when the job ends.
    pub fn told(&self) -> String {
        match &self.state {
            State::Done => format!("Note: background job {} ({}) finished after {} s and its change is in the project. Its answer: {}", self.id, self.brief, self.took.map_or(0, |t| t.as_secs()), self.result.chars().take(1500).collect::<String>()),
            State::Failed(e) => format!("Note: background job {} ({}) failed: {e}", self.id, self.brief),
            State::TimedOut => format!("Note: background job {} ({}) took longer than its {} minute limit and was given up on; nothing from it was applied. Try a smaller piece.", self.id, self.brief, self.limit.as_secs() / 60),
            State::Stopped => format!("Note: background job {} ({}) was stopped.", self.id, self.brief),
            State::Running => String::new(),
        }
    }
}

/// What a call is in a few words (for the job list).
pub fn brief(tool: &str, args: &Value) -> String {
    let pick = ["url", "media", "action", "path", "prompt", "preset"].iter().find_map(|k| args.get(*k)).map(|v| v.as_str().map_or_else(|| v.to_string(), str::to_owned));
    match pick {
        Some(p) => format!("{tool} {}", p.chars().take(60).collect::<String>()),
        None => tool.to_owned(),
    }
}

/// The `job` tool: the list of jobs, or one with its progress and result.
pub fn status(table: &[Info], args: &Value) -> Result<Value, String> {
    match args.get("id").or_else(|| args.get("job")).and_then(super::craft::as_id) {
        Some(id) => table.iter().find(|j| j.id as u64 == id).map(|j| j.json(true)).ok_or_else(|| format!("no job {id}; jobs: {}", table.iter().map(|j| j.id.to_string()).collect::<Vec<_>>().join(", "))),
        None => Ok(json!({"jobs": table.iter().map(|j| j.json(false)).collect::<Vec<_>>(), "note": "a job's id with `job` gives its result"})),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_work_is_known_and_quick_work_is_not() {
        assert!(limit_of("download", &json!({"url": "https://example.com/a.mp4"})).is_some());
        assert!(limit_of("see", &json!({"media": "trailer.mp4"})).is_some(), "scouting footage");
        assert!(limit_of("see", &json!({"t": 2})).is_none());
        assert!(limit_of("edit", &json!({"ops": [{"op": "smooth_motion", "clip": 3}]})).is_some(), "optical flow");
        assert!(limit_of("edit", &json!({"ops": [{"op": "card", "at": 0}]})).is_none());
        assert!(limit_of("export", &json!({})).is_some() && limit_of("export", &json!({"wait": false})).is_none());
        assert!(limit_of("generate", &json!({"estimate": true})).is_none());
        assert!(limit_of("summary", &json!({})).is_none());
    }

    #[test]
    fn the_job_tool_lists_and_reads_jobs() {
        let table = vec![Info { id: 4, tool: "download".into(), brief: "download x".into(), started: Instant::now(), took: Some(Duration::from_secs(3)), limit: Duration::from_secs(600), state: State::Done, progress: Arc::new(Progress::default()), result: "ok".into() }];
        assert_eq!(status(&table, &json!({"id": "4"})).unwrap()["result"], "ok");
        assert_eq!(status(&table, &json!({})).unwrap()["jobs"][0]["state"], "done");
        assert!(status(&table, &json!({"id": 9})).unwrap_err().contains("no job 9"));
    }
}
