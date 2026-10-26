// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Batches of pictures made in parallel.
//!
//! A film needs ten or twenty backdrops and characters; making them one at
//! a time makes the AI wait for the sum. A [`Batch`] runs the pictures on a
//! small pool of workers (each a separate Codex image run with its own work
//! folder), keeps a state per picture, and lets the session land every
//! finished one in the media bin while the rest are still being made.
//!
//! The pool does not know how a picture is made: it is given a [`Maker`].
//! The real one runs `codex exec`; tests give it a fake.

use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Workers at once when nothing says otherwise.
pub const DEFAULT_WORKERS: usize = 4;
/// Most workers a setting or a call may ask for.
pub const MAX_WORKERS: usize = 8;

/// One picture asked for.
#[derive(Clone, Debug, Default)]
pub struct PictureReq {
    pub name: String,
    pub prompt: String,
    pub aspect: Option<String>,
    pub transparent: bool,
    pub avoid: Option<String>,
    /// Pictures to follow (files).
    pub refs: Vec<PathBuf>,
    /// Put it on the timeline too.
    pub to_timeline: bool,
}

/// Makes the files for one picture. Runs on a worker thread.
pub type Maker = Arc<dyn Fn(&PictureReq) -> Result<Vec<PathBuf>, String> + Send + Sync>;

#[derive(Clone, Debug, PartialEq)]
pub enum State {
    Waiting,
    Running,
    Done(Vec<PathBuf>),
    Failed(String),
}

#[derive(Clone, Debug)]
struct Item {
    req: PictureReq,
    state: State,
    attempts: u32,
    /// The wording of the second attempt, when there was one.
    reworded: Option<String>,
    /// What the first attempt failed with, when it was retried.
    first_error: Option<String>,
    landed: bool,
    told: bool,
    media: Vec<Value>,
    started: Option<Instant>,
    seconds: f64,
}

struct Shared {
    items: Mutex<Vec<Item>>,
    next: AtomicUsize,
    cancelled: AtomicBool,
}

impl Shared {
    fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        let mut items = self.items.lock().unwrap_or_else(|e| e.into_inner());
        for it in items.iter_mut().filter(|i| i.state == State::Waiting) {
            it.state = State::Failed("cancelled before it started".into());
        }
    }
}

/// A running batch.
pub struct Batch {
    shared: Arc<Shared>,
    pub workers: usize,
    pub total: usize,
}

/// The number of workers: the call's, else the setting, else the default.
pub fn workers_for(asked: Option<u64>) -> usize {
    let setting = || {
        std::env::var("STUDIO_IMAGE_WORKERS").ok().and_then(|v| v.trim().parse::<usize>().ok()).or_else(|| {
            let text = std::fs::read_to_string(studio_paths::studio().join("generate.json")).ok()?;
            serde_json::from_str::<Value>(&text).ok()?.get("image_workers")?.as_u64().map(|n| n as usize)
        })
    };
    asked.map(|n| n as usize).or_else(setting).unwrap_or(DEFAULT_WORKERS).clamp(1, MAX_WORKERS)
}

/// Whether a failure is worth one more try (the run itself could not be
/// started or was cut short: another go would fail the same way).
pub fn retryable(err: &str) -> bool {
    !(err.contains("is not on this Mac") || err.contains("cannot run codex") || err.contains("was stopped"))
}

/// The request worded a second time: Codex is asked to put it in its own
/// words as an original picture, which is what gets past a refusal of the
/// first wording, and to call its image tool straight away.
pub fn reword(req: &PictureReq, why: &str) -> PictureReq {
    let mut r = req.clone();
    let why: String = why.chars().take(160).collect();
    r.prompt = format!(
        "The first attempt at this picture failed ({why}). Put the request below in your own words as a description of an original artwork (no real people, brands, logos or readable text), keeping the same subject, composition, mood and colours, then call your image generation tool with it right away. Do not ask questions and do not draw it with code. Request: {}",
        req.prompt.trim()
    );
    r
}

/// Makes one picture, and once more with a reworded request when the first
/// try fails. Returns the result, the attempts made and the second wording.
pub fn make_with_retry(req: &PictureReq, maker: &Maker) -> (Result<Vec<PathBuf>, String>, u32, Option<String>, Option<String>) {
    let run = |r: &PictureReq| match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| maker(r))) {
        Ok(v) => v,
        Err(_) => Err("the picture worker stopped unexpectedly".to_owned()),
    };
    match run(req) {
        Ok(files) if !files.is_empty() => (Ok(files), 1, None, None),
        first => {
            let why = match first {
                Err(e) => e,
                _ => "no picture came back".to_owned(),
            };
            if !retryable(&why) {
                return (Err(why), 1, None, None);
            }
            let again = reword(req, &why);
            match run(&again) {
                Ok(files) if !files.is_empty() => (Ok(files), 2, Some(again.prompt), Some(why)),
                second => {
                    let second = second.err().unwrap_or_else(|| "no picture came back".to_owned());
                    (Err(format!("{why}; retried once with the request reworded: {second}")), 2, Some(again.prompt), Some(why))
                }
            }
        }
    }
}

impl Batch {
    /// Starts the pictures on `workers` threads and returns at once.
    pub fn start(reqs: Vec<PictureReq>, workers: usize, maker: Maker) -> Batch {
        let total = reqs.len();
        let items = reqs.into_iter().map(|req| Item { req, state: State::Waiting, attempts: 0, reworded: None, first_error: None, landed: false, told: false, media: Vec::new(), started: None, seconds: 0.0 }).collect();
        let shared = Arc::new(Shared { items: Mutex::new(items), next: AtomicUsize::new(0), cancelled: AtomicBool::new(false) });
        let workers = workers.clamp(1, MAX_WORKERS).min(total.max(1));
        // Stop cancels the batch, and the Codex runs of its workers belong
        // to the same owner so Stop ends them too.
        let owner = super::procs::owner();
        {
            let s = shared.clone();
            super::jobs::on_cancel(move || s.cancel());
        }
        for _ in 0..workers {
            let (shared, maker) = (shared.clone(), maker.clone());
            std::thread::spawn(move || {
                super::procs::set_owner(owner);
                loop {
                    if shared.cancelled.load(Ordering::SeqCst) {
                        break;
                    }
                    let i = shared.next.fetch_add(1, Ordering::SeqCst);
                    let req = {
                        let mut items = shared.items.lock().unwrap_or_else(|e| e.into_inner());
                        let Some(it) = items.get_mut(i) else { break };
                        it.state = State::Running;
                        it.started = Some(Instant::now());
                        it.req.clone()
                    };
                    let (result, attempts, reworded, first_error) = make_with_retry(&req, &maker);
                    let mut items = shared.items.lock().unwrap_or_else(|e| e.into_inner());
                    let it = &mut items[i];
                    it.attempts = attempts;
                    it.reworded = reworded;
                    it.first_error = first_error;
                    it.seconds = it.started.map_or(0.0, |s| s.elapsed().as_secs_f64());
                    it.state = match result {
                        Ok(files) => State::Done(files),
                        Err(e) => State::Failed(e),
                    };
                }
            });
        }
        Batch { shared, workers, total }
    }

    /// Stops pictures not started yet; the ones being made finish.
    pub fn cancel(&self) {
        self.shared.cancel();
    }

    /// Every picture is done or failed.
    pub fn finished(&self) -> bool {
        let items = self.shared.items.lock().unwrap_or_else(|e| e.into_inner());
        items.iter().all(|i| matches!(i.state, State::Done(_) | State::Failed(_)))
    }

    /// Finished pictures not yet put in the media bin: (index, request, files).
    pub fn to_land(&self) -> Vec<(usize, PictureReq, Vec<PathBuf>)> {
        let items = self.shared.items.lock().unwrap_or_else(|e| e.into_inner());
        items.iter().enumerate().filter(|(_, i)| !i.landed).filter_map(|(n, i)| if let State::Done(f) = &i.state { Some((n, i.req.clone(), f.clone())) } else { None }).collect()
    }

    /// Records that picture `i` is in the media bin as `media`.
    pub fn landed(&self, i: usize, media: Vec<Value>) {
        let mut items = self.shared.items.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(it) = items.get_mut(i) {
            it.landed = true;
            it.media = media;
        }
    }

    /// What is new since the last call: pictures that landed and failures,
    /// each told once.
    pub fn news(&self) -> (Vec<Value>, Vec<Value>) {
        let mut items = self.shared.items.lock().unwrap_or_else(|e| e.into_inner());
        let (mut ready, mut failed) = (Vec::new(), Vec::new());
        for it in items.iter_mut().filter(|i| !i.told) {
            match &it.state {
                State::Done(_) if it.landed => {
                    it.told = true;
                    ready.push(json!({"name": it.req.name, "media": it.media}));
                }
                State::Failed(e) => {
                    it.told = true;
                    failed.push(json!({"name": it.req.name, "error": e}));
                }
                _ => {}
            }
        }
        (ready, failed)
    }

    /// The batch as the AI reads it: progress and one line per picture.
    pub fn report(&self) -> Value {
        let items = self.shared.items.lock().unwrap_or_else(|e| e.into_inner());
        let count = |f: &dyn Fn(&State) -> bool| items.iter().filter(|i| f(&i.state)).count();
        let done = count(&|s| matches!(s, State::Done(_)));
        let failed = count(&|s| matches!(s, State::Failed(_)));
        let running = count(&|s| *s == State::Running);
        let waiting = count(&|s| *s == State::Waiting);
        let list: Vec<Value> = items
            .iter()
            .map(|i| {
                let mut v = json!({"name": i.req.name});
                match &i.state {
                    State::Waiting => v["status"] = json!("waiting"),
                    State::Running => {
                        v["status"] = json!("making");
                        v["seconds"] = json!(i.started.map_or(0.0, |s| (s.elapsed().as_secs_f64() * 10.0).round() / 10.0));
                    }
                    State::Done(files) => {
                        v["status"] = json!(if i.landed { "ready" } else { "made" });
                        v["files"] = json!(files.iter().map(|p| p.display().to_string()).collect::<Vec<_>>());
                        v["media"] = json!(i.media);
                    }
                    State::Failed(e) => {
                        v["status"] = json!("failed");
                        v["error"] = json!(e);
                    }
                }
                if i.attempts > 1 {
                    v["attempts"] = json!(i.attempts);
                    if let Some(e) = &i.first_error {
                        v["first_error"] = json!(e);
                    }
                }
                v
            })
            .collect();
        json!({
            "progress": {"total": items.len(), "ready": done, "failed": failed, "making": running, "waiting": waiting, "fraction": if items.is_empty() { 1.0 } else { ((done + failed) as f64 / items.len() as f64 * 100.0).round() / 100.0 }},
            "finished": running + waiting == 0,
            "pictures": list,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU32;
    use std::time::Duration;

    fn req(n: usize) -> PictureReq {
        PictureReq { name: format!("pic {n}"), prompt: format!("picture number {n}"), ..Default::default() }
    }

    fn wait(b: &Batch) {
        let t = Instant::now();
        while !b.finished() {
            assert!(t.elapsed() < Duration::from_secs(20), "batch never finished");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// A maker that sleeps, counts how many run together and fails what `fail` says.
    fn fake(live: Arc<AtomicU32>, peak: Arc<AtomicU32>, fail: fn(&PictureReq) -> Option<String>) -> Maker {
        Arc::new(move |r: &PictureReq| {
            let now = live.fetch_add(1, Ordering::SeqCst) + 1;
            peak.fetch_max(now, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(60));
            live.fetch_sub(1, Ordering::SeqCst);
            match fail(r) {
                Some(e) => Err(e),
                None => Ok(vec![PathBuf::from(format!("{}.png", r.name))]),
            }
        })
    }

    #[test]
    fn pictures_are_made_at_the_same_time() {
        let (live, peak) = (Arc::new(AtomicU32::new(0)), Arc::new(AtomicU32::new(0)));
        let t = Instant::now();
        let b = Batch::start((0..4).map(req).collect(), 4, fake(live, peak.clone(), |_| None));
        wait(&b);
        assert_eq!(peak.load(Ordering::SeqCst), 4, "all four ran together");
        // Four 60 ms pictures in a row would take 240 ms.
        assert!(t.elapsed() < Duration::from_millis(220), "took {:?}", t.elapsed());
        assert_eq!(b.report()["progress"]["ready"], 4);
    }

    #[test]
    fn the_worker_cap_is_respected() {
        let (live, peak) = (Arc::new(AtomicU32::new(0)), Arc::new(AtomicU32::new(0)));
        let b = Batch::start((0..9).map(req).collect(), 3, fake(live, peak.clone(), |_| None));
        wait(&b);
        assert_eq!(peak.load(Ordering::SeqCst), 3);
        assert_eq!(b.report()["progress"]["ready"], 9);
        assert_eq!(workers_for(Some(99)), MAX_WORKERS);
        assert_eq!(workers_for(Some(0)), 1);
    }

    #[test]
    fn a_failed_picture_does_not_stop_the_others_and_is_reported_by_name() {
        let (live, peak) = (Arc::new(AtomicU32::new(0)), Arc::new(AtomicU32::new(0)));
        let b = Batch::start((0..4).map(req).collect(), 2, fake(live, peak, |r| (r.name == "pic 2").then(|| "Codex made no picture: refused".to_owned())));
        wait(&b);
        let r = b.report();
        assert_eq!(r["progress"]["ready"], 3);
        assert_eq!(r["progress"]["failed"], 1);
        let bad = &r["pictures"][2];
        assert_eq!(bad["status"], "failed");
        assert!(bad["error"].as_str().unwrap().contains("refused"));
        assert_eq!(bad["attempts"], 2, "it was retried before failing");
        assert_eq!(r["pictures"][0]["status"], "made");
        let (ready, failed) = b.news();
        assert!(ready.is_empty(), "nothing landed yet");
        assert_eq!(failed.len(), 1);
        assert!(b.news().1.is_empty(), "told once");
    }

    #[test]
    fn a_failure_is_retried_once_with_the_request_reworded() {
        let seen = Arc::new(Mutex::new(Vec::<String>::new()));
        let s = seen.clone();
        let maker: Maker = Arc::new(move |r: &PictureReq| {
            let mut v = s.lock().unwrap();
            v.push(r.prompt.clone());
            if v.len() == 1 { Err("Codex made no picture: the image tool rejected the request".into()) } else { Ok(vec![PathBuf::from("a.png")]) }
        });
        let (res, attempts, reworded, first) = make_with_retry(&req(1), &maker);
        assert_eq!(res.unwrap(), vec![PathBuf::from("a.png")]);
        assert_eq!(attempts, 2);
        let seen = seen.lock().unwrap();
        assert_eq!(seen[0], "picture number 1");
        assert!(seen[1].contains("rejected the request") && seen[1].contains("picture number 1") && seen[1].contains("own words"));
        assert_eq!(reworded.as_deref(), Some(seen[1].as_str()));
        assert!(first.unwrap().contains("rejected"));
    }

    #[test]
    fn two_failures_report_the_real_reasons_and_a_missing_codex_is_not_retried() {
        let calls = Arc::new(AtomicU32::new(0));
        let c = calls.clone();
        let maker: Maker = Arc::new(move |_: &PictureReq| {
            let n = c.fetch_add(1, Ordering::SeqCst);
            Err(if n == 0 { "first reason".to_owned() } else { "second reason".to_owned() })
        });
        let (res, attempts, ..) = make_with_retry(&req(1), &maker);
        let e = res.unwrap_err();
        assert!(e.contains("first reason") && e.contains("second reason"), "{e}");
        assert_eq!(attempts, 2);

        let calls = Arc::new(AtomicU32::new(0));
        let c = calls.clone();
        let maker: Maker = Arc::new(move |_: &PictureReq| {
            c.fetch_add(1, Ordering::SeqCst);
            Err("the codex command is not on this Mac".to_owned())
        });
        let (res, attempts, ..) = make_with_retry(&req(1), &maker);
        assert!(res.is_err());
        assert_eq!((attempts, calls.load(Ordering::SeqCst)), (1, 1));
    }

    #[test]
    fn cancel_stops_pictures_not_started() {
        let (live, peak) = (Arc::new(AtomicU32::new(0)), Arc::new(AtomicU32::new(0)));
        let b = Batch::start((0..6).map(req).collect(), 1, fake(live, peak, |_| None));
        b.cancel();
        wait(&b);
        let r = b.report();
        assert!(r["progress"]["failed"].as_u64().unwrap() >= 4, "{r}");
    }
}
