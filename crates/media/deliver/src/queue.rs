// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The render queue: jobs with priorities, pause and resume, presets and
//! notices, kept in a small JSON file so the Studio window, a headless
//! agent and `zornov queue` all see the same queue.
//!
//! The file is the truth. Every change goes through [`Store::update`],
//! which takes a lock file, reads, changes and writes atomically; a runner
//! (in the window or headless) polls the file, so a pause, cancel or new
//! priority made anywhere reaches the job that is drawing.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct JobSpec {
    pub name: String,
    /// The project folder to render.
    pub project: PathBuf,
    /// A delivery spec id (youtube, netflix, dcp-smpte ...), or empty to use `preset`.
    #[serde(default)]
    pub delivery: String,
    /// An export preset name (Social 1080p ...).
    #[serde(default)]
    pub preset: String,
    /// The file (or package folder) to write; empty: next to the project in `exports/`.
    #[serde(default)]
    pub out: PathBuf,
    /// Seconds into the sequence.
    #[serde(default)]
    pub range: Option<(f64, f64)>,
    #[serde(default = "yes")]
    pub qc: bool,
    #[serde(default)]
    pub bundle: bool,
    /// Higher goes first.
    #[serde(default)]
    pub priority: i32,
    /// Share the chunks with render nodes on the network.
    #[serde(default)]
    pub distributed: bool,
    /// Seconds per chunk (0: the default).
    #[serde(default)]
    pub chunk_secs: f64,
    /// Output size, overriding the preset's or the spec's.
    #[serde(default)]
    pub size: Option<(u32, u32)>,
}

fn yes() -> bool {
    true
}

impl JobSpec {
    pub fn new(name: &str, project: &Path) -> Self {
        JobSpec { name: name.into(), project: project.into(), delivery: String::new(), preset: String::new(), out: PathBuf::new(), range: None, qc: true, bundle: false, priority: 0, distributed: false, chunk_secs: 0.0, size: None }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Queued,
    Running,
    /// Held: a queued job does not start; a running one stops starting new chunks.
    Paused,
    Done,
    Failed,
    Cancelled,
}

impl State {
    pub fn finished(self) -> bool {
        matches!(self, State::Done | State::Failed | State::Cancelled)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Job {
    pub id: u32,
    pub spec: JobSpec,
    pub state: State,
    /// Paused while it was running: resume returns it to Running.
    #[serde(default)]
    pub was_running: bool,
    pub progress: f32,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub outputs: Vec<PathBuf>,
    /// QC verdict ("PASS", "FAIL"...) and the report file.
    #[serde(default)]
    pub qc: Option<String>,
    #[serde(default)]
    pub qc_report: Option<PathBuf>,
    /// Machines that drew chunks, with how many.
    #[serde(default)]
    pub machines: Vec<(String, usize)>,
    pub added: u64,
    #[serde(default)]
    pub started: Option<u64>,
    #[serde(default)]
    pub ended: Option<u64>,
}

/// A saved set of job settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QueuePreset {
    pub name: String,
    pub delivery: String,
    pub preset: String,
    pub qc: bool,
    pub bundle: bool,
    pub distributed: bool,
    pub priority: i32,
}

/// Something worth telling the user.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Notice {
    pub job: u32,
    pub title: String,
    pub body: String,
    pub ok: bool,
    /// Already shown as a desktop notification.
    #[serde(default)]
    pub notified: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Queue {
    pub jobs: Vec<Job>,
    pub next_id: u32,
    #[serde(default)]
    pub presets: Vec<QueuePreset>,
    /// Not yet shown; the window or agent drains them.
    #[serde(default)]
    pub notices: Vec<Notice>,
}

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

impl Queue {
    pub fn add(&mut self, spec: JobSpec) -> u32 {
        self.next_id += 1;
        let id = self.next_id;
        self.jobs.push(Job { id, spec, state: State::Queued, was_running: false, progress: 0.0, error: None, outputs: Vec::new(), qc: None, qc_report: None, machines: Vec::new(), added: now(), started: None, ended: None });
        id
    }

    pub fn get(&self, id: u32) -> Option<&Job> {
        self.jobs.iter().find(|j| j.id == id)
    }

    pub fn get_mut(&mut self, id: u32) -> Result<&mut Job, String> {
        self.jobs.iter_mut().find(|j| j.id == id).ok_or_else(|| format!("No job {id} in the queue"))
    }

    /// The job to start next: the highest priority queued job, oldest first.
    pub fn next(&self) -> Option<u32> {
        self.jobs.iter().filter(|j| j.state == State::Queued).min_by_key(|j| (-j.spec.priority, j.id)).map(|j| j.id)
    }

    pub fn pause(&mut self, id: u32) -> Result<(), String> {
        let j = self.get_mut(id)?;
        match j.state {
            State::Queued => j.state = State::Paused,
            State::Running => {
                j.was_running = true;
                j.state = State::Paused;
            }
            State::Paused => {}
            s => return Err(format!("Job {id} is {s:?}; only queued or running jobs pause")),
        }
        Ok(())
    }

    pub fn resume(&mut self, id: u32) -> Result<(), String> {
        let j = self.get_mut(id)?;
        match j.state {
            State::Paused => {
                j.state = if j.was_running { State::Running } else { State::Queued };
                j.was_running = false;
            }
            State::Queued | State::Running => {}
            s => return Err(format!("Job {id} is {s:?}; only paused jobs resume")),
        }
        Ok(())
    }

    pub fn cancel(&mut self, id: u32) -> Result<(), String> {
        let j = self.get_mut(id)?;
        if !j.state.finished() {
            j.state = State::Cancelled;
            j.ended = Some(now());
        }
        Ok(())
    }

    /// Run a finished (failed or cancelled) job again from the start.
    pub fn retry(&mut self, id: u32) -> Result<(), String> {
        let j = self.get_mut(id)?;
        if !j.state.finished() {
            return Err(format!("Job {id} is still {:?}", j.state));
        }
        j.state = State::Queued;
        j.progress = 0.0;
        j.error = None;
        j.ended = None;
        j.started = None;
        Ok(())
    }

    pub fn set_priority(&mut self, id: u32, priority: i32) -> Result<(), String> {
        self.get_mut(id)?.spec.priority = priority;
        Ok(())
    }

    /// Drop finished jobs; returns how many.
    pub fn clear_finished(&mut self) -> usize {
        let n = self.jobs.len();
        self.jobs.retain(|j| !j.state.finished());
        n - self.jobs.len()
    }

    pub fn begin(&mut self, id: u32) -> Result<(), String> {
        let j = self.get_mut(id)?;
        if j.state != State::Queued {
            return Err(format!("Job {id} is {:?}", j.state));
        }
        j.state = State::Running;
        j.started = Some(now());
        j.progress = 0.0;
        Ok(())
    }

    pub fn set_progress(&mut self, id: u32, f: f32) {
        if let Ok(j) = self.get_mut(id) {
            if !j.state.finished() {
                j.progress = f.clamp(0.0, 1.0);
            }
        }
    }

    pub fn finish(&mut self, id: u32, outputs: Vec<PathBuf>, qc: Option<(String, PathBuf)>, machines: Vec<(String, usize)>) {
        let mut note = None;
        if let Ok(j) = self.get_mut(id) {
            if j.state == State::Cancelled {
                return;
            }
            j.state = State::Done;
            j.progress = 1.0;
            j.ended = Some(now());
            j.outputs = outputs;
            j.machines = machines;
            if let Some((v, p)) = qc {
                j.qc = Some(v);
                j.qc_report = Some(p);
            }
            let ok = j.qc.as_deref().is_none_or(|v| v != "FAIL");
            let secs = j.ended.unwrap_or(0).saturating_sub(j.started.unwrap_or(0));
            note = Some(Notice {
                job: id,
                title: format!("{} {}", j.spec.name, if ok { "is ready" } else { "needs attention" }),
                body: format!("Rendered in {secs} s{}", j.qc.as_ref().map_or(String::new(), |v| format!("; QC {v}"))),
                ok,
                notified: false,
            });
        }
        self.notices.extend(note);
    }

    pub fn fail(&mut self, id: u32, error: &str) {
        let mut note = None;
        if let Ok(j) = self.get_mut(id) {
            if j.state == State::Cancelled {
                return;
            }
            j.state = State::Failed;
            j.error = Some(error.into());
            j.ended = Some(now());
            note = Some(Notice { job: id, title: format!("{} failed", j.spec.name), body: error.into(), ok: false, notified: false });
        }
        self.notices.extend(note);
    }

    pub fn save_preset(&mut self, p: QueuePreset) {
        self.presets.retain(|x| x.name != p.name);
        self.presets.push(p);
    }

    pub fn drain_notices(&mut self) -> Vec<Notice> {
        std::mem::take(&mut self.notices)
    }

    /// Notices not yet shown as desktop notifications (they stay for `drain_notices`); the oldest go past 50.
    pub fn take_unnotified(&mut self) -> Vec<Notice> {
        let fresh: Vec<Notice> = self.notices.iter().filter(|n| !n.notified).cloned().collect();
        for n in &mut self.notices {
            n.notified = true;
        }
        let extra = self.notices.len().saturating_sub(50);
        self.notices.drain(..extra);
        fresh
    }
}

/// The queue file with its lock.
#[derive(Clone, Debug)]
pub struct Store {
    pub path: PathBuf,
}

struct Lock(PathBuf);

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

impl Store {
    pub fn new(path: &Path) -> Store {
        Store { path: path.into() }
    }

    fn lock(&self) -> Result<Lock, String> {
        let lp = self.path.with_extension("lock");
        if let Some(d) = lp.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        for _ in 0..200 {
            match std::fs::OpenOptions::new().write(true).create_new(true).open(&lp) {
                Ok(_) => return Ok(Lock(lp)),
                Err(_) => {
                    // A lock older than 10 s belongs to a process that died.
                    if std::fs::metadata(&lp).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|a| a > Duration::from_secs(10)) {
                        let _ = std::fs::remove_file(&lp);
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
        }
        Err("the queue file is locked".into())
    }

    /// The queue as stored; a job left Running by a process that died is
    /// not distinguishable here, so [`Store::recover`] fixes those at start.
    pub fn read(&self) -> Queue {
        std::fs::read(&self.path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    /// Change the queue atomically.
    pub fn update<T>(&self, f: impl FnOnce(&mut Queue) -> T) -> Result<T, String> {
        let _l = self.lock()?;
        let mut q = self.read();
        let r = f(&mut q);
        let tmp = self.path.with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(&q).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &self.path).map_err(|e| e.to_string())?;
        Ok(r)
    }

    /// Jobs that were Running when the last runner died go back to the queue.
    pub fn recover(&self) -> Result<usize, String> {
        self.update(|q| {
            let mut n = 0;
            for j in &mut q.jobs {
                if j.state == State::Running || (j.state == State::Paused && j.was_running) {
                    j.state = State::Queued;
                    j.was_running = false;
                    j.progress = 0.0;
                    n += 1;
                }
            }
            n
        })
    }
}

/// Only one process draws the queue: a lock file holding its pid.
pub struct RunnerLock(PathBuf);

impl RunnerLock {
    pub fn acquire(store: &Store) -> Result<RunnerLock, String> {
        let p = store.path.with_extension("runner");
        if let Ok(s) = std::fs::read_to_string(&p) {
            if let Ok(pid) = s.trim().parse::<u32>() {
                let age_ok = std::fs::metadata(&p).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|a| a < Duration::from_secs(5));
                if pid != std::process::id() && age_ok {
                    return Err(format!("another Zornov process (pid {pid}) is drawing the queue"));
                }
            }
        }
        std::fs::write(&p, std::process::id().to_string()).map_err(|e| e.to_string())?;
        Ok(RunnerLock(p))
    }

    /// Call every second or so: a lock not touched for 5 s is stale.
    pub fn touch(&self) {
        let _ = std::fs::write(&self.0, std::process::id().to_string());
    }
}

impl Drop for RunnerLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(name: &str, prio: i32) -> JobSpec {
        JobSpec { priority: prio, ..JobSpec::new(name, Path::new("/p")) }
    }

    #[test]
    fn priority_then_age_decides_what_runs_next() {
        let mut q = Queue::default();
        let a = q.add(spec("a", 0));
        let b = q.add(spec("b", 5));
        let c = q.add(spec("c", 5));
        assert_eq!(q.next(), Some(b));
        q.begin(b).unwrap();
        assert_eq!(q.next(), Some(c));
        q.set_priority(a, 9).unwrap();
        assert_eq!(q.next(), Some(a));
        q.pause(a).unwrap();
        assert_eq!(q.next(), Some(c), "a paused job is skipped");
        q.resume(a).unwrap();
        assert_eq!(q.next(), Some(a));
        assert!(q.begin(b).is_err());
        assert!(q.pause(99).is_err());
    }

    #[test]
    fn pause_resume_cancel_and_retry() {
        let mut q = Queue::default();
        let a = q.add(spec("a", 0));
        q.begin(a).unwrap();
        q.set_progress(a, 0.4);
        q.pause(a).unwrap();
        assert_eq!(q.get(a).unwrap().state, State::Paused);
        q.resume(a).unwrap();
        assert_eq!(q.get(a).unwrap().state, State::Running, "a job paused while drawing resumes drawing");
        assert_eq!(q.get(a).unwrap().progress, 0.4);
        q.cancel(a).unwrap();
        q.set_progress(a, 0.9);
        assert_eq!((q.get(a).unwrap().state, q.get(a).unwrap().progress), (State::Cancelled, 0.4));
        q.finish(a, vec![], None, vec![]);
        assert_eq!(q.get(a).unwrap().state, State::Cancelled, "a cancelled job stays cancelled");
        q.retry(a).unwrap();
        assert_eq!(q.get(a).unwrap().state, State::Queued);
        assert!(q.retry(a).is_err());
        q.begin(a).unwrap();
        q.fail(a, "disk full");
        assert_eq!(q.get(a).unwrap().error.as_deref(), Some("disk full"));
        assert_eq!(q.clear_finished(), 1);
    }

    #[test]
    fn notices_say_what_happened() {
        let mut q = Queue::default();
        let a = q.add(spec("Trailer", 0));
        q.begin(a).unwrap();
        q.finish(a, vec!["/x.mp4".into()], Some(("PASS".into(), "/x.qc.json".into())), vec![("this Mac".into(), 4)]);
        let b = q.add(spec("Film", 0));
        q.begin(b).unwrap();
        q.finish(b, vec![], Some(("FAIL".into(), "/y.qc.json".into())), vec![]);
        // Desktop notifications take each notice once and leave it for the agent.
        assert_eq!(q.take_unnotified().len(), 2);
        assert!(q.take_unnotified().is_empty());
        let n = q.drain_notices();
        assert_eq!(n.len(), 2);
        assert!(n[0].ok && n[0].title.contains("ready") && n[0].body.contains("QC PASS"));
        assert!(!n[1].ok && n[1].title.contains("needs attention"));
        assert!(q.drain_notices().is_empty());
    }

    #[test]
    fn the_file_is_shared_and_survives_a_dead_runner() {
        let d = std::env::temp_dir().join(format!("studio-queue-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let st = Store::new(&d.join("queue.json"));
        let id = st.update(|q| q.add(spec("a", 1))).unwrap();
        st.update(|q| q.begin(id)).unwrap().unwrap();
        // A second store handle (another process) sees it and can pause it.
        let other = Store::new(&d.join("queue.json"));
        assert_eq!(other.read().get(id).unwrap().state, State::Running);
        other.update(|q| q.pause(id)).unwrap().unwrap();
        assert_eq!(st.read().get(id).unwrap().state, State::Paused);
        // The runner dies; a new one recovers it to the queue.
        assert_eq!(st.recover().unwrap(), 1);
        assert_eq!(st.read().get(id).unwrap().state, State::Queued);
        // Many threads adding at once lose nothing.
        let hs: Vec<_> = (0..8).map(|i| { let s = st.clone(); std::thread::spawn(move || { for k in 0..5 { s.update(|q| q.add(spec(&format!("t{i}-{k}"), 0))).unwrap(); } }) }).collect();
        for h in hs { h.join().unwrap(); }
        let q = st.read();
        assert_eq!(q.jobs.len(), 41);
        let mut ids: Vec<u32> = q.jobs.iter().map(|j| j.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), 41);
        // Only one runner at a time.
        let lock = RunnerLock::acquire(&st).unwrap();
        lock.touch();
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn presets_replace_by_name() {
        let mut q = Queue::default();
        let p = |prio| QueuePreset { name: "Review".into(), delivery: "web-preview".into(), preset: String::new(), qc: true, bundle: false, distributed: false, priority: prio };
        q.save_preset(p(0));
        q.save_preset(p(3));
        assert_eq!(q.presets.len(), 1);
        assert_eq!(q.presets[0].priority, 3);
    }
}
