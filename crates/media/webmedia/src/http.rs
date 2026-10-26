// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Fetching: one request with retries and plain error messages, and many
//! segments in parallel into files (finished files are kept, so a second
//! run resumes).

use crate::error::{Error, Result};
use crate::types::{Progress, Report};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Duration;

const UA: &str = "Mozilla/5.0 (compatible; Zornov)";

fn status_error(status: u16, url: &str) -> Error {
    let host = crate::url::host_of(url);
    match status {
        401 | 403 => Error::SignIn(format!("{host} did not hand that over (HTTP {status}). It needs a signed-in account or a licence, and Zornov does not sign in for you or get around that.")),
        404 | 410 => Error::Net(format!("{host} says it is gone (HTTP {status})")),
        _ => Error::Net(format!("{host} answered HTTP {status}")),
    }
}

/// One GET, retried on dropped connections and busy servers. `range` is an
/// inclusive byte range.
pub fn get(url: &str, referer: &str, range: Option<(u64, u64)>) -> Result<studio_net::Reply> {
    let range_header = range.map(|(a, b)| format!("bytes={a}-{b}"));
    let mut last = Error::Net("no answer".into());
    for attempt in 0..3u64 {
        if attempt > 0 {
            std::thread::sleep(Duration::from_millis(400 * attempt));
        }
        let mut headers: Vec<(&str, &str)> = vec![("User-Agent", UA), ("Accept", "*/*")];
        if !referer.is_empty() {
            headers.push(("Referer", referer));
        }
        if let Some(r) = &range_header {
            headers.push(("Range", r));
        }
        match studio_net::get(url, &headers, Duration::from_secs(30)) {
            Ok(r) if r.status == 200 || r.status == 206 => return Ok(r),
            Ok(r) if r.status == 429 || r.status >= 500 => last = status_error(r.status, url),
            Ok(r) => return Err(status_error(r.status, url)),
            Err(e) => last = Error::Net(e),
        }
    }
    Err(last)
}

/// A whole (small) document.
pub struct Doc {
    pub body: Vec<u8>,
    /// Lower-case content type, without parameters trimmed ("text/html; charset=utf-8" stays whole).
    pub ct: String,
    /// Where it came from after redirects.
    pub url: String,
}

impl Doc {
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

pub fn get_doc(url: &str, referer: &str, limit: u64) -> Result<Doc> {
    let r = get(url, referer, None)?;
    let ct = r.content_type.clone().unwrap_or_default().to_ascii_lowercase();
    let final_url = if r.url.is_empty() { url.to_owned() } else { r.url.clone() };
    let body = r.bytes(limit).map_err(|e| Error::Net(format!("the transfer broke off: {e}")))?;
    Ok(Doc { body, ct, url: final_url })
}

/// One piece to fetch.
#[derive(Clone, Debug)]
pub struct Job {
    pub url: String,
    /// Inclusive byte range.
    pub range: Option<(u64, u64)>,
    pub path: PathBuf,
}

/// Fetches `url` (or its `range`) into `path`, through a `.part` file.
fn fetch_one(job: &Job, referer: &str, bytes: &AtomicU64, max_bytes: u64) -> Result<()> {
    let mut last = Error::Net("no answer".into());
    for attempt in 0..3u64 {
        if attempt > 0 {
            std::thread::sleep(Duration::from_millis(500 * attempt));
        }
        let mut reply = match get(&job.url, referer, job.range) {
            Ok(r) => r,
            Err(e @ (Error::SignIn(_) | Error::Cancelled)) => return Err(e),
            Err(e) => {
                last = e;
                continue;
            }
        };
        let part = job.path.with_extension("part");
        let res = (|| -> std::io::Result<u64> {
            // A server that ignored the Range header sent the whole file:
            // skip to the start and keep only what was asked for.
            let mut want = u64::MAX;
            if let Some((a, b)) = job.range {
                want = b - a + 1;
                if reply.status == 200 && a > 0 {
                    std::io::copy(&mut (&mut reply.body).take(a), &mut std::io::sink())?;
                }
            }
            let mut f = std::fs::File::create(&part)?;
            let mut buf = vec![0u8; 64 << 10];
            let mut total = 0u64;
            while total < want {
                let n = (&mut reply.body).take(want - total).read(&mut buf)?;
                if n == 0 {
                    break;
                }
                f.write_all(&buf[..n])?;
                total += n as u64;
                if bytes.fetch_add(n as u64, Ordering::Relaxed) + n as u64 > max_bytes {
                    return Err(std::io::Error::other("over the size limit"));
                }
            }
            f.flush()?;
            if job.range.is_some() && total != want {
                return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "the piece ended early"));
            }
            Ok(total)
        })();
        match res {
            Ok(_) => {
                std::fs::rename(&part, &job.path)?;
                return Ok(());
            }
            Err(e) if e.to_string() == "over the size limit" => {
                let _ = std::fs::remove_file(&part);
                return Err(Error::Bad("the download is larger than the limit".into()));
            }
            Err(e) => {
                let _ = std::fs::remove_file(&part);
                last = Error::Net(format!("the transfer broke off: {e}"));
            }
        }
    }
    Err(last)
}

/// Fetches all `jobs`, `parallel` at a time. Files that already exist are
/// kept (that is the resume). `stage` names the work for progress.
pub fn fetch_all(jobs: &[Job], referer: &str, parallel: usize, max_bytes: u64, limit: Duration, stage: &'static str, report: Report) -> Result<()> {
    let deadline = std::time::Instant::now() + limit;
    let total = jobs.len() as u64;
    let done = AtomicU64::new(0);
    let bytes = AtomicU64::new(0);
    // What is already there counts as done (and as bytes).
    for j in jobs {
        if let Ok(m) = std::fs::metadata(&j.path) {
            if m.len() > 0 {
                done.fetch_add(1, Ordering::Relaxed);
                bytes.fetch_add(m.len(), Ordering::Relaxed);
            }
        }
    }
    if !report(&Progress { stage, done: done.load(Ordering::Relaxed), total, bytes: bytes.load(Ordering::Relaxed) }) {
        return Err(Error::Cancelled);
    }
    let next = AtomicUsize::new(0);
    let stop = AtomicBool::new(false);
    let failure: Mutex<Option<Error>> = Mutex::new(None);
    let workers = parallel.clamp(1, 16).min(jobs.len().max(1));
    std::thread::scope(|s| {
        for _ in 0..workers {
            s.spawn(|| {
                loop {
                    if stop.load(Ordering::Relaxed) {
                        return;
                    }
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(job) = jobs.get(i) else { return };
                    if std::time::Instant::now() > deadline {
                        stop.store(true, Ordering::Relaxed);
                        let mut f = failure.lock().unwrap();
                        if f.is_none() {
                            *f = Some(Error::Net("the download took longer than the time allowed; what arrived is kept, so trying again continues".into()));
                        }
                        return;
                    }
                    if std::fs::metadata(&job.path).is_ok_and(|m| m.len() > 0) {
                        continue;
                    }
                    if let Err(e) = fetch_one(job, referer, &bytes, max_bytes) {
                        stop.store(true, Ordering::Relaxed);
                        let mut f = failure.lock().unwrap();
                        if f.is_none() {
                            *f = Some(e);
                        }
                        return;
                    }
                    let d = done.fetch_add(1, Ordering::Relaxed) + 1;
                    if !report(&Progress { stage, done: d, total, bytes: bytes.load(Ordering::Relaxed) }) {
                        stop.store(true, Ordering::Relaxed);
                        let mut f = failure.lock().unwrap();
                        if f.is_none() {
                            *f = Some(Error::Cancelled);
                        }
                        return;
                    }
                }
            });
        }
    });
    match failure.into_inner().unwrap() {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

/// A short stable name for a set of URLs (for the resume folder).
pub fn fingerprint(parts: &[&str]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for p in parts {
        for b in p.bytes().chain(std::iter::once(0u8)) {
            h ^= b as u64;
            h = h.wrapping_mul(0x100_0000_01b3);
        }
    }
    format!("{h:016x}")
}

pub fn ensure_dir(p: &Path) -> Result<()> {
    std::fs::create_dir_all(p).map_err(|e| Error::Io(format!("cannot make {}: {e}", p.display())))
}
