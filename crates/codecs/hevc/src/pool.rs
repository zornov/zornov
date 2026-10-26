// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A minimal fork-join thread pool (std only) used for picture-level
//! parallelism: wavefront (WPP) CTU-row decoding, deblocking and SAO bands.
//!
//! `Pool::run(n, f)` executes `f(0) .. f(n - 1)` on the pool's worker threads
//! and the calling thread, handing out task indices in increasing order, and
//! returns once every task has finished. Because tasks are started in index
//! order and every started task runs to completion on a live thread, a task
//! may wait for the progress of any task with a smaller index without
//! deadlocking.

use std::any::Any;
use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

type TaskFn = dyn Fn(usize) + Sync;

/// Lifetime-erased pointer to the closure of the running job. It is only
/// dereferenced while `Pool::run` (which owns the borrow) is blocked waiting
/// for the job to complete.
#[derive(Clone, Copy)]
struct JobPtr(*const TaskFn);
// SAFETY: the pointee is `Sync` and outlives every use (see `Pool::run`).
unsafe impl Send for JobPtr {}

struct State {
    job: Option<(JobPtr, usize)>,
    epoch: u64,
    /// Workers currently executing tasks of the posted job.
    active: usize,
    shutdown: bool,
    panic: Option<Box<dyn Any + Send>>,
}

struct Inner {
    state: Mutex<State>,
    work_cv: Condvar,
    done_cv: Condvar,
    next: AtomicUsize,
    done: AtomicUsize,
}

pub struct Pool {
    inner: Arc<Inner>,
    threads: Vec<JoinHandle<()>>,
}

/// Number of threads (including the caller) to use for picture-level
/// parallelism: `STUDIO_HEVC_THREADS` if set, else the available
/// parallelism capped at 8.
pub fn default_threads() -> usize {
    if let Some(n) = std::env::var("STUDIO_HEVC_THREADS").ok().and_then(|s| s.trim().parse::<usize>().ok()) {
        return n.clamp(1, 64);
    }
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).clamp(1, 8)
}

impl Pool {
    /// Creates a pool with `threads` participants (the caller plus
    /// `threads - 1` worker threads).
    pub fn new(threads: usize) -> Pool {
        let inner = Arc::new(Inner {
            state: Mutex::new(State { job: None, epoch: 0, active: 0, shutdown: false, panic: None }),
            work_cv: Condvar::new(),
            done_cv: Condvar::new(),
            next: AtomicUsize::new(0),
            done: AtomicUsize::new(0),
        });
        let mut handles = Vec::new();
        for i in 1..threads.max(1) {
            let inner = inner.clone();
            let h = std::thread::Builder::new()
                .name(format!("studio-hevc-{i}"))
                .spawn(move || worker(&inner));
            match h {
                Ok(h) => handles.push(h),
                Err(_) => break,
            }
        }
        Pool { inner, threads: handles }
    }

    /// Runs `f(i)` for `i` in `0..n` across the pool and waits for completion.
    /// A panic in any task is re-raised here after all tasks have finished.
    pub fn run(&self, n: usize, f: &(dyn Fn(usize) + Sync)) {
        if n == 0 {
            return;
        }
        if self.threads.is_empty() || n == 1 {
            for i in 0..n {
                f(i);
            }
            return;
        }
        // SAFETY: lifetime erasure; the pointer is not used after this
        // function returns because we wait until every task has completed and
        // every worker has left the job (`active == 0`), and the job is
        // removed before returning.
        let ptr: *const TaskFn = unsafe { std::mem::transmute::<&(dyn Fn(usize) + Sync), &'static TaskFn>(f) };
        let inner = &*self.inner;
        {
            let mut st = inner.state.lock().unwrap_or_else(|e| e.into_inner());
            inner.next.store(0, Ordering::Relaxed);
            inner.done.store(0, Ordering::Relaxed);
            st.job = Some((JobPtr(ptr), n));
            st.epoch = st.epoch.wrapping_add(1);
            inner.work_cv.notify_all();
        }
        run_tasks(inner, f, n);
        let panic = {
            let mut st = inner.state.lock().unwrap_or_else(|e| e.into_inner());
            while !(inner.done.load(Ordering::Acquire) >= n && st.active == 0) {
                st = inner.done_cv.wait(st).unwrap_or_else(|e| e.into_inner());
            }
            st.job = None;
            st.panic.take()
        };
        if let Some(p) = panic {
            resume_unwind(p);
        }
    }
}

fn run_tasks(inner: &Inner, f: &(dyn Fn(usize) + Sync + '_), n: usize) {
    loop {
        let i = inner.next.fetch_add(1, Ordering::Relaxed);
        if i >= n {
            break;
        }
        if let Err(p) = catch_unwind(AssertUnwindSafe(|| f(i))) {
            let mut st = inner.state.lock().unwrap_or_else(|e| e.into_inner());
            if st.panic.is_none() {
                st.panic = Some(p);
            }
        }
        if inner.done.fetch_add(1, Ordering::AcqRel) + 1 == n {
            let _g = inner.state.lock().unwrap_or_else(|e| e.into_inner());
            inner.done_cv.notify_all();
        }
    }
}

fn worker(inner: &Inner) {
    let mut seen = 0u64;
    loop {
        let (job, n) = {
            let mut st = inner.state.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                if st.shutdown {
                    return;
                }
                if st.epoch != seen {
                    seen = st.epoch;
                    if let Some(j) = st.job {
                        st.active += 1;
                        break j;
                    }
                }
                st = inner.work_cv.wait(st).unwrap_or_else(|e| e.into_inner());
            }
        };
        // SAFETY: the job stays alive until `active` drops back to zero.
        let f: &TaskFn = unsafe { &*job.0 };
        run_tasks(inner, f, n);
        let mut st = inner.state.lock().unwrap_or_else(|e| e.into_inner());
        st.active -= 1;
        inner.done_cv.notify_all();
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        {
            let mut st = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
            st.shutdown = true;
            self.inner.work_cv.notify_all();
        }
        for h in self.threads.drain(..) {
            let _ = h.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_all_tasks_in_parallel_and_repeatedly() {
        let pool = Pool::new(4);
        for round in 0..200 {
            let n = 1 + round % 37;
            let hits: Vec<AtomicUsize> = (0..n).map(|_| AtomicUsize::new(0)).collect();
            pool.run(n, &|i| {
                hits[i].fetch_add(1, Ordering::Relaxed);
            });
            assert!(hits.iter().all(|h| h.load(Ordering::Relaxed) == 1));
        }
    }

    #[test]
    fn ordered_dependencies_do_not_deadlock() {
        let pool = Pool::new(3);
        let n = 64;
        let progress: Vec<AtomicUsize> = (0..n).map(|_| AtomicUsize::new(0)).collect();
        pool.run(n, &|i| {
            if i > 0 {
                while progress[i - 1].load(Ordering::Acquire) == 0 {
                    std::thread::yield_now();
                }
            }
            progress[i].store(1, Ordering::Release);
        });
        assert!(progress.iter().all(|p| p.load(Ordering::Relaxed) == 1));
    }

    #[test]
    fn panics_propagate_and_pool_stays_usable() {
        let pool = Pool::new(3);
        let r = catch_unwind(AssertUnwindSafe(|| {
            pool.run(8, &|i| {
                if i == 5 {
                    panic!("boom");
                }
            })
        }));
        assert!(r.is_err());
        let count = AtomicUsize::new(0);
        pool.run(10, &|_| {
            count.fetch_add(1, Ordering::Relaxed);
        });
        assert_eq!(count.load(Ordering::Relaxed), 10);
    }
}
