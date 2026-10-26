// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A render split into frame ranges ("chunks") that any machine can draw:
//! this one, or render nodes on the network. Each chunk is a video-only MP4
//! that starts on a keyframe; [`crate::merge`] joins them without
//! re-encoding.
//!
//! Chunks also make pausing, resuming after a crash and spreading work over
//! machines the same thing: finished chunks stay on disk, and a run picks
//! up the missing ones.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// One range to draw: frames `first..last` of the job.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChunkTask {
    pub job: String,
    pub index: usize,
    pub first: u64,
    pub last: u64,
    /// What the renderer needs besides the range (size, rate, codec...).
    pub settings: Value,
}

/// Something that can draw a chunk to `out` (a machine).
pub trait ChunkRenderer: Send + Sync {
    fn name(&self) -> String;
    fn render(&self, task: &ChunkTask, out: &Path, progress: &mut dyn FnMut(f32), cancel: &AtomicBool) -> Result<(), String>;
}

/// `first..last` cut into ranges of about `chunk` frames; a short tail
/// joins the range before it.
pub fn plan(first: u64, last: u64, chunk: u64) -> Vec<(u64, u64)> {
    let chunk = chunk.max(1);
    let mut out = Vec::new();
    let mut a = first;
    while a < last {
        let mut b = (a + chunk).min(last);
        if last - b < chunk / 2 {
            b = last;
        }
        out.push((a, b));
        a = b;
    }
    out
}

/// Pause and cancel switches shared with whoever drives the run.
#[derive(Default)]
pub struct RunControl {
    pub cancel: AtomicBool,
    pub pause: AtomicBool,
}

pub fn part_path(dir: &Path, index: usize) -> PathBuf {
    dir.join(format!("part-{index:05}.mp4"))
}

/// What a run did.
#[derive(Debug, Default)]
pub struct RunReport {
    pub parts: Vec<PathBuf>,
    /// Chunks that were already on disk from an earlier run.
    pub reused: usize,
    /// (machine, chunks drawn)
    pub by_machine: Vec<(String, usize)>,
}

const ATTEMPTS: u32 = 3;

/// Draw every range with `workers`, in parallel (a thread per machine),
/// into `parts_dir`. Chunks already there are kept. While `ctl.pause` is
/// set no new chunk starts (those under way finish). A machine that fails
/// twice in a row is dropped; its chunk goes to another; if no machine is
/// left the run fails.
pub fn run(job: &str, settings: &Value, ranges: &[(u64, u64)], parts_dir: &Path, workers: Vec<Arc<dyn ChunkRenderer>>, ctl: &RunControl, on_progress: &(dyn Fn(f32) + Sync)) -> Result<RunReport, String> {
    std::fs::create_dir_all(parts_dir).map_err(|e| e.to_string())?;
    if workers.is_empty() {
        return Err("no machine to render on".into());
    }
    let total: u64 = ranges.iter().map(|r| r.1 - r.0).sum::<u64>().max(1);
    // Fraction done per chunk (finished = 1.0).
    let done: Mutex<Vec<f32>> = Mutex::new(vec![0.0; ranges.len()]);
    let report = |done: &Vec<f32>| {
        let f: f64 = ranges.iter().zip(done).map(|(r, d)| (r.1 - r.0) as f64 * *d as f64).sum::<f64>() / total as f64;
        on_progress(f as f32);
    };
    let mut pending: VecDeque<usize> = VecDeque::new();
    let mut reused = 0;
    for i in 0..ranges.len() {
        if part_path(parts_dir, i).exists() {
            done.lock().unwrap()[i] = 1.0;
            reused += 1;
        } else {
            pending.push_back(i);
        }
    }
    report(&done.lock().unwrap());
    let queue = Mutex::new(pending);
    let attempts = Mutex::new(vec![0u32; ranges.len()]);
    let failed: Mutex<Option<String>> = Mutex::new(None);
    let alive = Mutex::new(workers.len());
    let counts: Mutex<Vec<(String, usize)>> = Mutex::new(workers.iter().map(|w| (w.name(), 0)).collect());
    let remaining = Mutex::new(queue.lock().unwrap().len());

    std::thread::scope(|scope| {
        for (wi, w) in workers.iter().enumerate() {
            let (queue, attempts, failed, alive, counts, remaining, done) = (&queue, &attempts, &failed, &alive, &counts, &remaining, &done);
            let report = &report;
            scope.spawn(move || {
                let mut strikes = 0;
                loop {
                    if ctl.cancel.load(Ordering::Relaxed) || failed.lock().unwrap().is_some() {
                        return;
                    }
                    if *remaining.lock().unwrap() == 0 {
                        return;
                    }
                    if ctl.pause.load(Ordering::Relaxed) {
                        std::thread::sleep(Duration::from_millis(50));
                        continue;
                    }
                    let Some(i) = queue.lock().unwrap().pop_front() else {
                        // Others are still drawing the rest (one may fail and requeue).
                        std::thread::sleep(Duration::from_millis(20));
                        continue;
                    };
                    let task = ChunkTask { job: job.into(), index: i, first: ranges[i].0, last: ranges[i].1, settings: settings.clone() };
                    let tmp = parts_dir.join(format!("part-{i:05}.tmp-{wi}"));
                    let mut prog = |f: f32| {
                        let mut d = done.lock().unwrap();
                        d[i] = f.clamp(0.0, 0.999);
                        report(&d);
                    };
                    match w.render(&task, &tmp, &mut prog, &ctl.cancel) {
                        Ok(()) => {
                            strikes = 0;
                            if std::fs::rename(&tmp, part_path(parts_dir, i)).is_err() {
                                *failed.lock().unwrap() = Some(format!("could not store chunk {i}"));
                                return;
                            }
                            let mut d = done.lock().unwrap();
                            d[i] = 1.0;
                            report(&d);
                            counts.lock().unwrap()[wi].1 += 1;
                            *remaining.lock().unwrap() -= 1;
                        }
                        Err(e) => {
                            let _ = std::fs::remove_file(&tmp);
                            if ctl.cancel.load(Ordering::Relaxed) {
                                return;
                            }
                            done.lock().unwrap()[i] = 0.0;
                            let busy = e.ends_with("busy");
                            if !busy {
                                strikes += 1;
                                let mut a = attempts.lock().unwrap();
                                a[i] += 1;
                                if a[i] >= ATTEMPTS {
                                    *failed.lock().unwrap() = Some(format!("chunk {i} (frames {}-{}) failed on {}: {e}", task.first, task.last, w.name()));
                                    return;
                                }
                            }
                            queue.lock().unwrap().push_back(i);
                            if busy {
                                std::thread::sleep(Duration::from_millis(250));
                            }
                            if strikes >= 2 {
                                let mut al = alive.lock().unwrap();
                                *al -= 1;
                                if *al == 0 {
                                    *failed.lock().unwrap() = Some(format!("every machine failed; last error on {}: {e}", w.name()));
                                }
                                return;
                            }
                        }
                    }
                }
            });
        }
    });

    if ctl.cancel.load(Ordering::Relaxed) {
        return Err("cancelled".into());
    }
    if let Some(e) = failed.into_inner().unwrap() {
        return Err(e);
    }
    if *remaining.lock().unwrap() != 0 {
        return Err("the render stopped before every chunk was drawn".into());
    }
    on_progress(1.0);
    Ok(RunReport { parts: (0..ranges.len()).map(|i| part_path(parts_dir, i)).collect(), reused, by_machine: counts.into_inner().unwrap() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    struct Fake {
        name: &'static str,
        fail_first: usize,
        calls: AtomicUsize,
        delay: u64,
    }

    impl ChunkRenderer for Fake {
        fn name(&self) -> String {
            self.name.into()
        }

        fn render(&self, t: &ChunkTask, out: &Path, progress: &mut dyn FnMut(f32), _c: &AtomicBool) -> Result<(), String> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            if n < self.fail_first {
                return Err("boom".into());
            }
            progress(0.5);
            std::thread::sleep(Duration::from_millis(self.delay));
            std::fs::write(out, format!("{}-{}", t.first, t.last)).map_err(|e| e.to_string())
        }
    }

    fn dir(n: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("studio-chunks-{n}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn plans_cover_the_range_with_no_tiny_tail() {
        assert_eq!(plan(0, 100, 30), vec![(0, 30), (30, 60), (60, 100)]);
        assert_eq!(plan(0, 90, 30), vec![(0, 30), (30, 60), (60, 90)]);
        assert_eq!(plan(5, 6, 30), vec![(5, 6)]);
        assert!(plan(3, 3, 30).is_empty());
        let p = plan(0, 1000, 37);
        assert_eq!(p.first().unwrap().0, 0);
        assert_eq!(p.last().unwrap().1, 1000);
        assert!(p.windows(2).all(|w| w[0].1 == w[1].0));
    }

    #[test]
    fn two_machines_share_the_chunks_and_a_rerun_reuses_them() {
        let d = dir("share");
        let ranges = plan(0, 400, 40);
        let a: Arc<dyn ChunkRenderer> = Arc::new(Fake { name: "a", fail_first: 0, calls: AtomicUsize::new(0), delay: 15 });
        let b: Arc<dyn ChunkRenderer> = Arc::new(Fake { name: "b", fail_first: 0, calls: AtomicUsize::new(0), delay: 15 });
        let ctl = RunControl::default();
        let last = Mutex::new(0f32);
        let r = run("j", &Value::Null, &ranges, &d, vec![a, b], &ctl, &|f| *last.lock().unwrap() = f).unwrap();
        assert_eq!(r.parts.len(), 10);
        assert!(r.by_machine.iter().all(|(_, n)| *n >= 2), "{:?}", r.by_machine);
        assert_eq!(r.by_machine.iter().map(|m| m.1).sum::<usize>(), 10);
        assert_eq!(*last.lock().unwrap(), 1.0);
        assert_eq!(std::fs::read_to_string(&r.parts[3]).unwrap(), "120-160");
        // Delete two parts: only those come back.
        std::fs::remove_file(&r.parts[2]).unwrap();
        std::fs::remove_file(&r.parts[7]).unwrap();
        let c: Arc<dyn ChunkRenderer> = Arc::new(Fake { name: "c", fail_first: 0, calls: AtomicUsize::new(0), delay: 0 });
        let r = run("j", &Value::Null, &ranges, &d, vec![c], &ctl, &|_| {}).unwrap();
        assert_eq!((r.reused, r.by_machine[0].1), (8, 2));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_failing_machine_is_dropped_and_its_chunks_move() {
        let d = dir("fail");
        let ranges = plan(0, 200, 40);
        let bad: Arc<dyn ChunkRenderer> = Arc::new(Fake { name: "bad", fail_first: 100, calls: AtomicUsize::new(0), delay: 0 });
        let good: Arc<dyn ChunkRenderer> = Arc::new(Fake { name: "good", fail_first: 0, calls: AtomicUsize::new(0), delay: 5 });
        let r = run("j", &Value::Null, &ranges, &d, vec![bad, good], &RunControl::default(), &|_| {}).unwrap();
        assert_eq!(r.parts.len(), 5);
        assert_eq!(r.by_machine[1].1, 5);
        // All machines bad: an error, not a hang.
        let d2 = dir("fail2");
        let bad: Arc<dyn ChunkRenderer> = Arc::new(Fake { name: "bad", fail_first: 100, calls: AtomicUsize::new(0), delay: 0 });
        assert!(run("j", &Value::Null, &ranges, &d2, vec![bad], &RunControl::default(), &|_| {}).is_err());
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::remove_dir_all(&d2);
    }

    #[test]
    fn pause_holds_new_chunks_and_resume_finishes() {
        let d = dir("pause");
        let ranges = plan(0, 400, 40);
        let ctl = Arc::new(RunControl::default());
        ctl.pause.store(true, Ordering::SeqCst);
        let a: Arc<dyn ChunkRenderer> = Arc::new(Fake { name: "a", fail_first: 0, calls: AtomicUsize::new(0), delay: 1 });
        let (c2, d2) = (ctl.clone(), d.clone());
        let h = std::thread::spawn(move || run("j", &Value::Null, &ranges, &d2, vec![a], &c2, &|_| {}));
        std::thread::sleep(Duration::from_millis(300));
        let during = std::fs::read_dir(&d).map(|r| r.count()).unwrap_or(0);
        assert_eq!(during, 0, "nothing starts while paused");
        ctl.pause.store(false, Ordering::SeqCst);
        assert_eq!(h.join().unwrap().unwrap().parts.len(), 10);
        // Cancel stops a run.
        let d3 = dir("cancel");
        let ctl = Arc::new(RunControl::default());
        let slow: Arc<dyn ChunkRenderer> = Arc::new(Fake { name: "s", fail_first: 0, calls: AtomicUsize::new(0), delay: 100 });
        let (c2, d4) = (ctl.clone(), d3.clone());
        let h = std::thread::spawn(move || run("j", &Value::Null, &plan(0, 4000, 40), &d4, vec![slow], &c2, &|_| {}));
        std::thread::sleep(Duration::from_millis(250));
        ctl.cancel.store(true, Ordering::SeqCst);
        assert_eq!(h.join().unwrap().unwrap_err(), "cancelled");
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::remove_dir_all(&d3);
    }
}
