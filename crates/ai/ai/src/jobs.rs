// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Background jobs: inference runs on worker threads, never on the UI thread.
//!
//! A job is a closure receiving a [`JobCtx`], through which it reports
//! progress and notices cancellation. The UI keeps the [`JobHandle`] and polls
//! it each frame (progress bar, message, cancel button, result when done).
//! [`Jobs::spawn_cached`] adds result caching keyed by
//! (media hash, model, parameters).

use crate::cache::{CacheKey, Cacheable, ResultCache};
use crate::error::{Error, Result};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};

/// What a running job sees: progress reporting and the cancel flag.
///
/// Progress is a fraction in [0, 1]. [`JobCtx::range`] gives a sub-step its
/// own 0..1 that maps onto part of the parent's range, so helpers report
/// progress without knowing where they sit in a longer job.
#[derive(Clone)]
pub struct JobCtx {
    shared: Arc<Shared>,
    lo: f32,
    hi: f32,
}

#[derive(Default)]
struct Shared {
    progress: AtomicU32,
    cancel: AtomicBool,
    state: Mutex<(JobState, String)>,
}

impl JobCtx {
    /// A context not attached to any job (progress goes nowhere, never
    /// cancelled). For calling features synchronously, e.g. in tests.
    pub fn detached() -> JobCtx {
        JobCtx { shared: Arc::default(), lo: 0.0, hi: 1.0 }
    }

    /// Reports progress `f` in [0, 1] of this context's range.
    pub fn set_progress(&self, f: f32) {
        let v = self.lo + (self.hi - self.lo) * f.clamp(0.0, 1.0);
        self.shared.progress.store(v.to_bits(), Ordering::Relaxed);
    }

    /// Short status line for the jobs popover ("Transcribing 2:30 / 10:00").
    pub fn set_message(&self, msg: impl Into<String>) {
        if let Ok(mut s) = self.shared.state.lock() {
            s.1 = msg.into();
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.shared.cancel.load(Ordering::Relaxed)
    }

    /// `Err(Cancelled)` once the user cancelled; call between units of work.
    pub fn check(&self) -> Result<()> {
        if self.is_cancelled() { Err(Error::Cancelled) } else { Ok(()) }
    }

    /// A child context covering `[a, b]` of this one's range.
    pub fn range(&self, a: f32, b: f32) -> JobCtx {
        let span = self.hi - self.lo;
        JobCtx { shared: self.shared.clone(), lo: self.lo + span * a, hi: self.lo + span * b }
    }

    /// Requests cancellation (what [`JobHandle::cancel`] does).
    pub fn cancel(&self) {
        self.shared.cancel.store(true, Ordering::Relaxed);
    }

    /// Current overall progress.
    pub fn progress(&self) -> f32 {
        f32::from_bits(self.shared.progress.load(Ordering::Relaxed))
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum JobState {
    #[default]
    Queued,
    Running,
    Done,
    /// Served from the result cache without running.
    Cached,
    Failed(Error),
    Cancelled,
}

impl JobState {
    pub fn is_finished(&self) -> bool {
        !matches!(self, JobState::Queued | JobState::Running)
    }
}

/// The UI's view of a job.
pub struct JobHandle<T> {
    pub id: u64,
    pub name: String,
    ctx: JobCtx,
    result: Arc<Mutex<Option<Result<T>>>>,
    done: Arc<(Mutex<bool>, Condvar)>,
}

impl<T> JobHandle<T> {
    pub fn progress(&self) -> f32 {
        self.ctx.progress()
    }
    pub fn message(&self) -> String {
        self.ctx.shared.state.lock().map(|s| s.1.clone()).unwrap_or_default()
    }
    pub fn state(&self) -> JobState {
        self.ctx.shared.state.lock().map(|s| s.0.clone()).unwrap_or(JobState::Failed(Error::Io("poisoned".into())))
    }
    pub fn cancel(&self) {
        self.ctx.cancel();
    }
    pub fn is_finished(&self) -> bool {
        self.state().is_finished()
    }
    /// The result once finished (taken: later calls return `None`).
    pub fn take_result(&self) -> Option<Result<T>> {
        self.result.lock().ok()?.take()
    }
    /// Blocks until the job finishes and returns its result.
    pub fn wait(self) -> Result<T> {
        let (m, cv) = &*self.done;
        let mut d = m.lock().map_err(|_| Error::Io("poisoned".into()))?;
        while !*d {
            d = cv.wait(d).map_err(|_| Error::Io("poisoned".into()))?;
        }
        drop(d);
        self.take_result().unwrap_or(Err(Error::Io("result already taken".into())))
    }
}

/// Row in the jobs popover.
#[derive(Clone, Debug)]
pub struct JobInfo {
    pub id: u64,
    pub name: String,
    pub progress: f32,
    pub message: String,
    pub state: JobState,
}

type Task = Box<dyn FnOnce() + Send>;

struct Queue {
    tasks: Mutex<(VecDeque<Task>, bool)>,
    cv: Condvar,
}

/// A pool of worker threads running jobs in submission order.
pub struct Jobs {
    queue: Arc<Queue>,
    workers: Vec<std::thread::JoinHandle<()>>,
    next_id: AtomicU64,
    list: Mutex<Vec<(u64, String, JobCtx)>>,
}

impl Jobs {
    /// `workers` threads (at least 1). Each model already uses every core for
    /// its own kernels, so one or two workers is usually right.
    pub fn new(workers: usize) -> Jobs {
        let queue = Arc::new(Queue { tasks: Mutex::new((VecDeque::new(), false)), cv: Condvar::new() });
        let workers = (0..workers.max(1))
            .map(|i| {
                let q = queue.clone();
                std::thread::Builder::new()
                    .name(format!("studio-ai-{i}"))
                    .spawn(move || {
                        loop {
                            let task = {
                                let Ok(mut g) = q.tasks.lock() else { return };
                                loop {
                                    if let Some(t) = g.0.pop_front() {
                                        break t;
                                    }
                                    if g.1 {
                                        return;
                                    }
                                    let Ok(ng) = q.cv.wait(g) else { return };
                                    g = ng;
                                }
                            };
                            task();
                        }
                    })
                    .expect("spawn AI worker")
            })
            .collect();
        Jobs { queue, workers, next_id: AtomicU64::new(1), list: Mutex::new(Vec::new()) }
    }

    /// Queues `f` and returns its handle.
    pub fn spawn<T, F>(&self, name: impl Into<String>, f: F) -> JobHandle<T>
    where
        T: Send + 'static,
        F: FnOnce(&JobCtx) -> Result<T> + Send + 'static,
    {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let name = name.into();
        let ctx = JobCtx::detached();
        let result: Arc<Mutex<Option<Result<T>>>> = Arc::new(Mutex::new(None));
        let done = Arc::new((Mutex::new(false), Condvar::new()));
        if let Ok(mut l) = self.list.lock() {
            l.retain(|(_, _, c)| !c.shared.state.lock().map(|s| s.0.is_finished()).unwrap_or(true));
            l.push((id, name.clone(), ctx.clone()));
        }
        let handle = JobHandle { id, name, ctx: ctx.clone(), result: result.clone(), done: done.clone() };
        let task: Task = Box::new(move || {
            let set = |st: JobState| {
                if let Ok(mut s) = ctx.shared.state.lock() {
                    s.0 = st;
                }
            };
            let r = if ctx.is_cancelled() {
                Err(Error::Cancelled)
            } else {
                set(JobState::Running);
                // A panicking model must not take the worker down with it.
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&ctx))).unwrap_or_else(|p| {
                    let msg = p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_default();
                    Err(Error::Inference(format!("panic: {msg}")))
                })
            };
            let r = match r {
                Err(_) if ctx.is_cancelled() => Err(Error::Cancelled),
                r => r,
            };
            match &r {
                Ok(_) => {
                    ctx.set_progress(1.0);
                    let cached = ctx.shared.state.lock().map(|s| s.0 == JobState::Cached).unwrap_or(false);
                    if !cached {
                        set(JobState::Done);
                    }
                }
                Err(Error::Cancelled) => set(JobState::Cancelled),
                Err(e) => set(JobState::Failed(e.clone())),
            }
            if let Ok(mut slot) = result.lock() {
                *slot = Some(r);
            }
            let (m, cv) = &*done;
            if let Ok(mut d) = m.lock() {
                *d = true;
            }
            cv.notify_all();
        });
        if let Ok(mut g) = self.queue.tasks.lock() {
            g.0.push_back(task);
        }
        self.queue.cv.notify_one();
        handle
    }

    /// As [`Jobs::spawn`], but first looks the result up in `cache` under
    /// `key` and stores a fresh result there. Cache errors never fail the job.
    pub fn spawn_cached<T, F>(&self, name: impl Into<String>, cache: ResultCache, key: CacheKey, f: F) -> JobHandle<T>
    where
        T: Cacheable + Send + 'static,
        F: FnOnce(&JobCtx) -> Result<T> + Send + 'static,
    {
        self.spawn(name, move |ctx| {
            if let Some(hit) = cache.get::<T>(&key) {
                if let Ok(mut s) = ctx.shared.state.lock() {
                    s.0 = JobState::Cached;
                }
                return Ok(hit);
            }
            let v = f(ctx)?;
            let _ = cache.put(&key, &v);
            Ok(v)
        })
    }

    /// Unfinished and recently finished jobs, oldest first.
    pub fn list(&self) -> Vec<JobInfo> {
        let Ok(l) = self.list.lock() else { return Vec::new() };
        l.iter()
            .map(|(id, name, ctx)| {
                let (state, message) = ctx.shared.state.lock().map(|s| s.clone()).unwrap_or_default();
                JobInfo { id: *id, name: name.clone(), progress: ctx.progress(), message, state }
            })
            .collect()
    }

    /// Cancels every queued and running job.
    pub fn cancel_all(&self) {
        if let Ok(l) = self.list.lock() {
            l.iter().for_each(|(_, _, c)| c.cancel());
        }
    }
}

impl Drop for Jobs {
    fn drop(&mut self) {
        self.cancel_all();
        if let Ok(mut g) = self.queue.tasks.lock() {
            g.1 = true;
        }
        self.queue.cv.notify_all();
        for w in self.workers.drain(..) {
            let _ = w.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_reports_progress_and_returns() {
        let jobs = Jobs::new(1);
        let h = jobs.spawn("sum", |ctx| {
            let mut s = 0u64;
            for i in 0..100u64 {
                ctx.check()?;
                s += i;
                ctx.range(0.0, 0.5).set_progress(i as f32 / 99.0);
            }
            Ok(s)
        });
        assert_eq!(h.wait().unwrap(), 4950);
    }

    #[test]
    fn cancel_stops_a_running_job() {
        let jobs = Jobs::new(1);
        let (tx, rx) = std::sync::mpsc::channel();
        let h = jobs.spawn("forever", move |ctx| {
            tx.send(()).ok();
            loop {
                ctx.check()?;
                std::thread::yield_now();
            }
            #[allow(unreachable_code)]
            Ok(())
        });
        rx.recv().unwrap();
        h.cancel();
        let id = h.id;
        assert_eq!(h.wait(), Err(Error::Cancelled));
        assert!(jobs.list().iter().any(|j| j.id == id && j.state == JobState::Cancelled));
    }

    #[test]
    fn sub_ranges_map_progress() {
        let ctx = JobCtx::detached();
        ctx.range(0.5, 1.0).range(0.5, 1.0).set_progress(0.5);
        assert!((ctx.progress() - 0.875).abs() < 1e-6);
    }

    #[test]
    fn panics_become_errors() {
        let jobs = Jobs::new(1);
        let h = jobs.spawn::<(), _>("boom", |_| panic!("kaput"));
        assert!(matches!(h.wait(), Err(Error::Inference(m)) if m.contains("kaput")));
        // the worker survives
        assert_eq!(jobs.spawn("after", |_| Ok(7)).wait().unwrap(), 7);
    }

    #[test]
    fn cached_jobs_skip_work() {
        let dir = std::env::temp_dir().join(format!("studio-ai-jobs-{}", std::process::id()));
        let cache = ResultCache::new(&dir);
        let key = CacheKey::new("media", "none", "p=1");
        let jobs = Jobs::new(2);
        let a = jobs.spawn_cached("a", cache.clone(), key.clone(), |_| Ok(vec![1.5f32, 2.5]));
        assert_eq!(a.wait().unwrap(), vec![1.5, 2.5]);
        let b = jobs.spawn_cached::<Vec<f32>, _>("b", cache, key, |_| Err(Error::Invalid("should not run".into())));
        let st = loop {
            let s = b.state();
            if s.is_finished() {
                break s;
            }
            std::thread::yield_now();
        };
        assert_eq!(st, JobState::Cached);
        assert_eq!(b.wait().unwrap(), vec![1.5, 2.5]);
        let _ = std::fs::remove_dir_all(dir);
    }
}
