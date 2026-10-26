// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Tiny parallel-for helpers: a persistent worker pool, with
//! `std::thread::scope` as the fallback for nested or concurrent calls.

use std::sync::atomic::{AtomicUsize, Ordering};

/// A raw pointer that may cross threads; users guarantee disjoint writes.
pub struct SendPtr<T>(pub *mut T);
// Manual impls: a raw pointer is Copy whatever `T` is.
impl<T> Clone for SendPtr<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for SendPtr<T> {}
unsafe impl<T> Send for SendPtr<T> {}
unsafe impl<T> Sync for SendPtr<T> {}

impl<T> SendPtr<T> {
    /// # Safety
    /// The caller guarantees the offset is in bounds and writes are disjoint.
    #[inline(always)]
    pub unsafe fn add(self, n: usize) -> *mut T {
        unsafe { self.0.add(n) }
    }
    /// The pointer (a method so closures capture the whole Send wrapper).
    #[inline(always)]
    pub fn get(self) -> *mut T {
        self.0
    }
}

/// Resolves a requested thread count (0 = all hardware threads).
pub fn resolve_threads(requested: usize) -> usize {
    if requested > 0 {
        return requested;
    }
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4)
}

// ------------------------------------------------------------ worker pool
//
// Spawning OS threads per op costs tens of microseconds per thread, which
// dominates small GEMMs and element-wise ops. A lazily created global pool of
// `available_parallelism() - 1` sleeping workers runs the jobs instead. One
// job runs on the pool at a time; a concurrent or nested parallel call (from
// another session thread, or from inside a pool job) falls back to scoped
// threads, so behaviour never depends on pool availability.

use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::sync::atomic::AtomicBool;
use std::sync::{Condvar, Mutex, OnceLock};

type JobRef = &'static (dyn Fn() + Sync);

struct Slot {
    job: Option<JobRef>,
    epoch: u64,
    /// Workers still allowed to join the current job.
    wanted: usize,
    /// Workers currently executing the current job.
    running: usize,
    panicked: bool,
}

struct Pool {
    slot: Mutex<Slot>,
    work: Condvar,
    done: Condvar,
    workers: usize,
}

static POOL: OnceLock<&'static Pool> = OnceLock::new();
static BUSY: AtomicBool = AtomicBool::new(false);
/// Mirrors `Slot::epoch` so idle workers can spin on it without the lock.
static EPOCH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// How long an idle worker spins before sleeping (back-to-back ops in one
/// inference reuse awake workers; a sleeping thread takes tens of µs to wake).
const SPIN: std::time::Duration = std::time::Duration::from_micros(200);

fn pool() -> &'static Pool {
    POOL.get_or_init(|| {
        let workers = resolve_threads(0).saturating_sub(1);
        let p: &'static Pool = Box::leak(Box::new(Pool { slot: Mutex::new(Slot { job: None, epoch: 0, wanted: 0, running: 0, panicked: false }), work: Condvar::new(), done: Condvar::new(), workers }));
        for i in 0..workers {
            let spawned = std::thread::Builder::new().name(format!("studio-nn-{i}")).spawn(move || worker(p));
            if spawned.is_err() {
                break;
            }
        }
        p
    })
}

fn lock(p: &Pool) -> std::sync::MutexGuard<'_, Slot> {
    p.slot.lock().unwrap_or_else(|e| e.into_inner())
}

fn worker(p: &'static Pool) {
    let mut seen = 0u64;
    loop {
        // spin briefly for the next job before blocking on the condvar
        let t0 = std::time::Instant::now();
        let mut spins = 0u32;
        while EPOCH.load(Ordering::Acquire) == seen {
            std::hint::spin_loop();
            spins += 1;
            if spins % 256 == 0 && t0.elapsed() > SPIN {
                break;
            }
        }
        let job = {
            let mut s = lock(p);
            loop {
                if s.epoch != seen {
                    seen = s.epoch;
                    if let Some(j) = s.job
                        && s.wanted > 0
                    {
                        s.wanted -= 1;
                        s.running += 1;
                        break j;
                    }
                }
                s = p.work.wait(s).unwrap_or_else(|e| e.into_inner());
            }
        };
        let r = catch_unwind(AssertUnwindSafe(job));
        let mut s = lock(p);
        s.running -= 1;
        if r.is_err() {
            s.panicked = true;
        }
        if s.running == 0 {
            p.done.notify_all();
        }
    }
}

/// Runs `body` on the calling thread plus up to `helpers` pool workers and
/// returns once every participant has finished. `false` = pool unavailable
/// (busy or nested), nothing was run.
fn run_on_pool(helpers: usize, body: &(dyn Fn() + Sync)) -> bool {
    if BUSY.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
        return false;
    }
    let p = pool();
    let helpers = helpers.min(p.workers);
    // SAFETY: the reference is only used by workers that join this job, and
    // this function does not return before all of them have finished
    // (`running == 0` after the job slot is cleared), so `body` outlives every use.
    let job: JobRef = unsafe { std::mem::transmute::<&(dyn Fn() + Sync), JobRef>(body) };
    if helpers > 0 {
        let mut s = lock(p);
        s.job = Some(job);
        s.epoch = s.epoch.wrapping_add(1);
        EPOCH.store(s.epoch, Ordering::Release);
        s.wanted = helpers;
        s.panicked = false;
        drop(s);
        p.work.notify_all();
    }
    let mine = catch_unwind(AssertUnwindSafe(body));
    let worker_panicked = {
        let mut s = lock(p);
        s.job = None;
        s.wanted = 0;
        while s.running > 0 {
            s = p.done.wait(s).unwrap_or_else(|e| e.into_inner());
        }
        std::mem::replace(&mut s.panicked, false)
    };
    BUSY.store(false, Ordering::Release);
    if let Err(e) = mine {
        resume_unwind(e);
    }
    if worker_panicked {
        panic!("a studio-nn worker thread panicked");
    }
    true
}

/// Splits `0..n` into at most `threads` contiguous chunks and runs
/// `f(start, end)` on each, the first chunk on the calling thread.
pub fn par_range(n: usize, threads: usize, f: impl Fn(usize, usize) + Sync) {
    let t = threads.max(1).min(n.max(1));
    if t <= 1 {
        if n > 0 {
            f(0, n);
        }
        return;
    }
    let chunk = n.div_ceil(t);
    let next = AtomicUsize::new(0);
    let body = || {
        loop {
            let i = next.fetch_add(1, Ordering::Relaxed);
            if i >= t {
                break;
            }
            let (a, b) = (i * chunk, ((i + 1) * chunk).min(n));
            if a < b {
                f(a, b);
            }
        }
    };
    if run_on_pool(t - 1, &body) {
        return;
    }
    std::thread::scope(|s| {
        let f = &f;
        for i in 1..t {
            let (a, b) = (i * chunk, ((i + 1) * chunk).min(n));
            if a < b {
                s.spawn(move || f(a, b));
            }
        }
        f(0, chunk.min(n));
    });
}

/// Runs `f(i)` for every `i in 0..n` on up to `threads` workers that pull
/// items from a shared counter (good for uneven work).
pub fn par_dynamic(n: usize, threads: usize, f: impl Fn(usize) + Sync) {
    let t = threads.max(1).min(n.max(1));
    if t <= 1 {
        (0..n).for_each(&f);
        return;
    }
    let next = AtomicUsize::new(0);
    let body = || {
        loop {
            let i = next.fetch_add(1, Ordering::Relaxed);
            if i >= n {
                break;
            }
            f(i);
        }
    };
    if run_on_pool(t - 1, &body) {
        return;
    }
    std::thread::scope(|s| {
        let work = || loop {
            let i = next.fetch_add(1, Ordering::Relaxed);
            if i >= n {
                break;
            }
            f(i);
        };
        for _ in 1..t {
            s.spawn(work);
        }
        work();
    });
}

/// Applies `f` to disjoint chunks of `out` in parallel when it is large.
pub fn par_chunks_mut<T: Send>(out: &mut [T], threads: usize, min_chunk: usize, f: impl Fn(usize, &mut [T]) + Sync) {
    let n = out.len();
    let t = threads.max(1).min(n / min_chunk.max(1)).max(1);
    if t <= 1 {
        f(0, out);
        return;
    }
    let chunk = n.div_ceil(t);
    let pieces = n.div_ceil(chunk);
    let base = SendPtr(out.as_mut_ptr());
    let next = AtomicUsize::new(0);
    let body = || {
        loop {
            let i = next.fetch_add(1, Ordering::Relaxed);
            if i >= pieces {
                break;
            }
            let off = i * chunk;
            let len = chunk.min(n - off);
            // SAFETY: pieces are disjoint sub-slices of `out`, each handed out once.
            let piece = unsafe { std::slice::from_raw_parts_mut(base.add(off), len) };
            f(off, piece);
        }
    };
    if run_on_pool(pieces - 1, &body) {
        return;
    }
    std::thread::scope(|s| {
        let f = &f;
        let mut rest = out;
        let mut off = 0;
        let mut first = None;
        while !rest.is_empty() {
            let take = chunk.min(rest.len());
            let (a, b) = rest.split_at_mut(take);
            if first.is_none() {
                first = Some((off, a));
            } else {
                let o = off;
                s.spawn(move || f(o, a));
            }
            off += take;
            rest = b;
        }
        if let Some((o, a)) = first {
            f(o, a);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;

    #[test]
    fn helpers_cover_every_item_once() {
        for threads in [1, 2, 5, 32] {
            for n in [0usize, 1, 7, 100, 1000] {
                let hits: Vec<AtomicU64> = (0..n).map(|_| AtomicU64::new(0)).collect();
                par_dynamic(n, threads, |i| {
                    hits[i].fetch_add(1, Ordering::Relaxed);
                });
                par_range(n, threads, |a, b| {
                    for h in &hits[a..b] {
                        h.fetch_add(10, Ordering::Relaxed);
                    }
                });
                assert!(hits.iter().all(|h| h.load(Ordering::Relaxed) == 11), "n={n} t={threads}");
                let mut v = vec![0u32; n];
                par_chunks_mut(&mut v, threads, 3, |off, ch| {
                    for (i, x) in ch.iter_mut().enumerate() {
                        *x += (off + i) as u32 + 1;
                    }
                });
                assert!(v.iter().enumerate().all(|(i, &x)| x == i as u32 + 1));
            }
        }
    }

    #[test]
    fn nested_and_concurrent_calls() {
        let total = AtomicU64::new(0);
        std::thread::scope(|s| {
            for _ in 0..4 {
                s.spawn(|| {
                    for _ in 0..50 {
                        par_dynamic(8, 4, |_| {
                            par_range(16, 3, |a, b| {
                                total.fetch_add((b - a) as u64, Ordering::Relaxed);
                            });
                        });
                    }
                });
            }
        });
        assert_eq!(total.load(Ordering::Relaxed), 4 * 50 * 8 * 16);
    }

    #[test]
    fn panics_propagate_and_pool_recovers() {
        let r = std::panic::catch_unwind(|| par_dynamic(64, 8, |i| assert!(i != 40, "boom")));
        assert!(r.is_err());
        let count = AtomicU64::new(0);
        par_dynamic(64, 8, |_| {
            count.fetch_add(1, Ordering::Relaxed);
        });
        assert_eq!(count.load(Ordering::Relaxed), 64);
    }
}
