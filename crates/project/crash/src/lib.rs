// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Crash reports.
//!
//! [`install`] makes a panic hook and (on macOS, Linux and Windows) a
//! native crash handler. Either writes a plain-text report to
//! `<Studio data folder>/crashes/` and nothing else: **nothing is sent
//! unless the person agrees** to a particular report, and only to the
//! endpoint in `packaging/release.conf` (or `STUDIO_CRASH_ENDPOINT`). With no
//! endpoint, reports stay on this computer. A report holds the version,
//! system, time, the panic message or signal and a backtrace; the home
//! folder's name is replaced by `~`. It holds no project, file contents or
//! keys.

mod native;

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

pub fn dir() -> PathBuf {
    studio_update::data_dir().join("crashes")
}

static VERSION: OnceLock<String> = OnceLock::new();

/// The text every report starts with.
pub fn header(kind: &str) -> String {
    format!(
        "Studio crash report\nversion: {}\nos: {} {}\nkind: {kind}\ntime: {} (seconds since 1970)\n",
        VERSION.get().map_or("unknown", String::as_str),
        std::env::consts::OS,
        std::env::consts::ARCH,
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()),
    )
}

/// Replaces the home folder's path with `~`.
pub fn scrub(text: &str) -> String {
    let home = if cfg!(windows) { std::env::var("USERPROFILE") } else { std::env::var("HOME") };
    match home {
        Ok(h) if h.len() > 3 => text.replace(&h, "~"),
        _ => text.to_owned(),
    }
}

/// Saves `text` as a new report; the path, if it worked.
pub fn save(kind: &str, body: &str) -> Option<PathBuf> {
    let d = dir();
    std::fs::create_dir_all(&d).ok()?;
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let path = d.join(format!("crash-{t}-{}-{kind}.txt", std::process::id()));
    std::fs::write(&path, scrub(&format!("{}---\n{body}\n", header(kind)))).ok()?;
    Some(path)
}

/// Installs the panic hook and the native crash handler. Call once, first
/// thing in `main`.
pub fn install(version: &str) {
    if VERSION.set(version.to_owned()).is_err() {
        return;
    }
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        let loc = info.location().map_or(String::new(), |l| format!("{}:{}:{}", l.file(), l.line(), l.column()));
        let msg = info.payload().downcast_ref::<&str>().map(|s| (*s).to_owned()).or_else(|| info.payload().downcast_ref::<String>().cloned()).unwrap_or_default();
        let bt = std::backtrace::Backtrace::force_capture();
        save("panic", &format!("thread: {}\nmessage: {msg}\nlocation: {loc}\n\nbacktrace:\n{bt}", thread.name().unwrap_or("unnamed")));
        previous(info);
    }));
    native::install(&dir(), &header("native"));
}

/// A saved report not yet decided on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    pub path: PathBuf,
}

impl Report {
    pub fn text(&self) -> String {
        std::fs::read_to_string(&self.path).unwrap_or_default()
    }
    /// First line of the message, for the dialog.
    pub fn summary(&self) -> String {
        let t = self.text();
        t.lines().find_map(|l| l.strip_prefix("message: ").or_else(|| l.strip_prefix("signal: "))).unwrap_or("a crash").chars().take(160).collect()
    }
}

/// Reports waiting for a decision, oldest first.
pub fn pending() -> Vec<Report> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir()).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "txt")).collect();
    v.sort();
    v.into_iter().map(|path| Report { path }).collect()
}

/// Moves a report out of the waiting list: kept in `crashes/handled/` (for
/// the person to read or attach to a message) or deleted.
pub fn finish(r: &Report, keep: bool) {
    if keep {
        let d = dir().join("handled");
        if std::fs::create_dir_all(&d).is_ok() {
            if let Some(n) = r.path.file_name() {
                let _ = std::fs::rename(&r.path, d.join(n));
                return;
            }
        }
    }
    let _ = std::fs::remove_file(&r.path);
}

/// What the person chose, kept between runs.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Ask about waiting reports at start (when an endpoint is set).
    pub ask: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { ask: true }
    }
}

fn settings_file() -> PathBuf {
    studio_update::data_dir().join("crash-settings.json")
}

pub fn settings() -> Settings {
    std::fs::read(settings_file()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

pub fn set_ask(ask: bool) {
    let _ = std::fs::create_dir_all(studio_update::data_dir());
    if let Ok(b) = serde_json::to_vec(&Settings { ask }) {
        let _ = std::fs::write(settings_file(), b);
    }
}

/// The endpoint, if one is configured and is an http(s) URL.
pub fn endpoint() -> Option<String> {
    Some(studio_update::config::load().crash_endpoint).filter(|e| e.starts_with("https://") || e.starts_with("http://"))
}

/// Sends one report. Call only after the person said yes to this report.
pub fn send(r: &Report, endpoint: &str) -> Result<(), String> {
    let reply = studio_net::post(endpoint, &[("Content-Type", "text/plain; charset=utf-8"), ("User-Agent", "Studio-crash-report")], r.text().as_bytes(), Duration::from_secs(20))?;
    if (200..300).contains(&reply.status) { Ok(()) } else { Err(format!("the report server answered {}", reply.status)) }
}

/// Opens the folder in the system file manager.
pub fn reveal() {
    let d = dir();
    let _ = std::fs::create_dir_all(&d);
    open_path(&d);
}

fn open_path(p: &Path) {
    let cmd = if cfg!(target_os = "macos") { "open" } else if cfg!(windows) { "explorer" } else { "xdg-open" };
    let _ = std::process::Command::new(cmd).arg(p).spawn();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_is_scrubbed() {
        let home = std::env::var("HOME").unwrap();
        assert_eq!(scrub(&format!("at {home}/Projects/x.rs")), "at ~/Projects/x.rs");
    }

    #[test]
    fn endpoint_must_be_a_url() {
        // Not set in the shipped config.
        unsafe { std::env::set_var("STUDIO_CRASH_ENDPOINT", "ftp://x") };
        assert_eq!(endpoint(), None);
        unsafe { std::env::set_var("STUDIO_CRASH_ENDPOINT", "https://crash.example/report") };
        assert_eq!(endpoint().as_deref(), Some("https://crash.example/report"));
    }
}

/// Crashes the process on purpose (`segv` or `abort`), to test the handler.
#[doc(hidden)]
pub fn crash_for_test(kind: &str) -> ! {
    native::crash_for_test(kind)
}
