// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A small persistent worker pool for data-parallel pixel loops. Spawning
//! threads per call (as `std::thread::scope` does) costs tens of
//! microseconds per thread, which adds up to milliseconds per preview frame;
//! these workers start once and wait. Calls are scoped: they return only
//! when every index has run, so the closure may borrow from the caller.
//! The calling thread works too, so a busy pool never stalls a caller, and a
//! call made from inside a pool task runs serially instead of deadlocking.

use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};

struct Job {
    next: AtomicUsize,
    n: usize,
    left: AtomicUsize,
    panicked: std::sync::atomic::AtomicBool,
    /// Lifetime-erased `&(dyn Fn(usize) + Sync)`; valid until `left` is 0,
    /// which `par_for` waits for before returning.
    f: *const (dyn Fn(usize) + Sync),
}

unsafe impl Send for Job {}
unsafe impl Sync for Job {}

struct Pool {
    jobs: Mutex<Vec<Arc<Job>>>,
    work: Condvar,
    done: Condvar,
    threads: usize,
}

thread_local! {
    static IN_POOL: Cell<bool> = const { Cell::new(false) };
}

fn pool() -> &'static Pool {
    static POOL: OnceLock<&'static Pool> = OnceLock::new();
    POOL.get_or_init(|| {
        let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(2, 16) - 1;
        let p: &'static Pool = Box::leak(Box::new(Pool { jobs: Mutex::new(Vec::new()), work: Condvar::new(), done: Condvar::new(), threads }));
        for i in 0..threads {
            let _ = std::thread::Builder::new().name(format!("studio-pool-{i}")).spawn(move || worker(p));
        }
        p
    })
}

fn worker(p: &'static Pool) {
    IN_POOL.with(|c| c.set(true));
    let mut q = p.jobs.lock().unwrap_or_else(|e| e.into_inner());
    loop {
        q.retain(|j| j.next.load(Ordering::Relaxed) < j.n);
        let Some(job) = q.first().cloned() else {
            q = p.work.wait(q).unwrap();
            continue;
        };
        drop(q);
        run(p, &job);
        q = p.jobs.lock().unwrap_or_else(|e| e.into_inner());
    }
}

fn run(p: &Pool, job: &Job) {
    loop {
        let i = job.next.fetch_add(1, Ordering::AcqRel);
        if i >= job.n {
            return;
        }
        // SAFETY: the caller of `par_for` keeps the closure alive until `left` reaches 0.
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe { (*job.f)(i) })).is_err() {
            job.panicked.store(true, Ordering::Relaxed);
        }
        if job.left.fetch_sub(1, Ordering::AcqRel) == 1 {
            let _g = p.jobs.lock().unwrap_or_else(|e| e.into_inner());
            p.done.notify_all();
        }
    }
}

/// Runs `f(0..n)` across the pool and the calling thread; returns when all are done.
pub fn par_for(n: usize, f: impl Fn(usize) + Sync) {
    if n == 0 {
        return;
    }
    if n == 1 || IN_POOL.with(Cell::get) {
        (0..n).for_each(f);
        return;
    }
    let p = pool();
    let fr: &(dyn Fn(usize) + Sync) = &f;
    // SAFETY: lifetime erasure only; see `Job::f`.
    let fp: *const (dyn Fn(usize) + Sync + 'static) = unsafe { std::mem::transmute(fr as *const (dyn Fn(usize) + Sync)) };
    let job = Arc::new(Job { next: AtomicUsize::new(0), n, left: AtomicUsize::new(n), panicked: Default::default(), f: fp });
    {
        let mut q = p.jobs.lock().unwrap_or_else(|e| e.into_inner());
        q.push(job.clone());
        if n > 2 {
            p.work.notify_all();
        } else {
            p.work.notify_one();
        }
    }
    IN_POOL.with(|c| c.set(true));
    run(p, &job);
    IN_POOL.with(|c| c.set(false));
    let mut q = p.jobs.lock().unwrap_or_else(|e| e.into_inner());
    while job.left.load(Ordering::Acquire) != 0 {
        q = p.done.wait(q).unwrap();
    }
    q.retain(|j| !Arc::ptr_eq(j, &job));
    drop(q);
    if job.panicked.load(Ordering::Relaxed) {
        panic!("a parallel pixel task panicked");
    }
}

/// Pool threads plus the caller: a sensible number of chunks to split into.
pub fn width() -> usize {
    pool().threads + 1
}

/// Splits `data` into `chunk`-long pieces and runs `f(index, piece)` on each in parallel.
pub fn par_chunks_mut<T: Send>(data: &mut [T], chunk: usize, f: impl Fn(usize, &mut [T]) + Sync) {
    let chunk = chunk.max(1);
    let len = data.len();
    let n = len.div_ceil(chunk);
    let base = data.as_mut_ptr() as usize;
    par_for(n, |i| {
        let start = i * chunk;
        let end = (start + chunk).min(len);
        // SAFETY: chunks are disjoint and `data` is exclusively borrowed for the call.
        let piece = unsafe { std::slice::from_raw_parts_mut((base as *mut T).add(start), end - start) };
        f(i, piece);
    });
}

/// Row-parallel helper for packed images: `f(y, row)` for every `stride`-long
/// row, in bands of at least `min_rows` rows.
pub fn par_rows<T: Send>(data: &mut [T], stride: usize, min_rows: usize, f: impl Fn(usize, &mut [T]) + Sync) {
    if stride == 0 {
        return;
    }
    let rows = data.len() / stride;
    let per = rows.div_ceil(width() * 2).max(min_rows.max(1));
    par_chunks_mut(&mut data[..rows * stride], per * stride, |ci, band| {
        for (ri, row) in band.chunks_mut(stride).enumerate() {
            f(ci * per + ri, row);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;

    #[test]
    fn every_index_runs_once_and_nesting_works() {
        let sum = AtomicU64::new(0);
        par_for(1000, |i| {
            sum.fetch_add(i as u64, Ordering::Relaxed);
            // Nested call runs serially on the pool thread.
            par_for(3, |_| {
                sum.fetch_add(0, Ordering::Relaxed);
            });
        });
        assert_eq!(sum.load(Ordering::Relaxed), 999 * 1000 / 2);
        let mut v = vec![0u32; 10_007];
        par_chunks_mut(&mut v, 100, |ci, c| c.iter_mut().enumerate().for_each(|(k, x)| *x = (ci * 100 + k) as u32));
        assert!(v.iter().enumerate().all(|(i, &x)| x == i as u32));
        let mut img = vec![0u8; 7 * 33];
        par_rows(&mut img, 7, 1, |y, row| row.fill(y as u8));
        assert!(img.chunks(7).enumerate().all(|(y, r)| r.iter().all(|&v| v == y as u8)));
        // Concurrent callers from several threads.
        std::thread::scope(|s| {
            for _ in 0..4 {
                s.spawn(|| {
                    let c = AtomicU64::new(0);
                    par_for(500, |_| {
                        c.fetch_add(1, Ordering::Relaxed);
                    });
                    assert_eq!(c.load(Ordering::Relaxed), 500);
                });
            }
        });
    }
}
