// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The background checker: one per process, read by the app's update
//! notice and Settings.
//!
//! A check only reads the feed and checks its signature; the download
//! starts when the person chooses Update now ([`update_now`]). Later
//! ([`later`]) keeps the notice away for a day.

use crate::apply::{self, Pending, Target};
use crate::config::Config;
use crate::feed;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Idle,
    Checking,
    UpToDate,
    /// A newer version this copy can install, waiting for Update now.
    Found(String),
    /// Bytes got, of bytes in all.
    Downloading(u64, u64),
    /// Downloaded and checked: installed at the next start.
    Ready(String),
    /// A newer version exists but this copy cannot install it itself
    /// (a development build, or one a package manager owns).
    Available(String),
    Failed(String),
    /// Updates cannot run, in words (no feed configured...).
    Disabled(String),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Look for updates by itself (about once a day).
    pub auto_check: bool,
    /// Seconds since 1970 of the last check.
    pub last_check: u64,
    /// Seconds since 1970 until which the notice stays away (Later).
    pub snoozed_until: u64,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { auto_check: true, last_check: 0, snoozed_until: 0 }
    }
}

const DAY: u64 = 24 * 3600;

fn settings_file() -> std::path::PathBuf {
    apply::updates_dir().join("settings.json")
}

pub fn settings() -> Settings {
    std::fs::read(settings_file()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn save(s: &Settings) {
    let _ = std::fs::create_dir_all(apply::updates_dir());
    if let Ok(b) = serde_json::to_vec_pretty(s) {
        let _ = std::fs::write(settings_file(), b);
    }
}

pub fn set_auto_check(on: bool) {
    save(&Settings { auto_check: on, ..settings() });
}

/// A newer version found by a check, ready to download.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Offer {
    pub version: String,
    /// The feed key it is signed for ([`Target::feed_key`]).
    pub key: String,
    pub entry: feed::Entry,
}

struct Inner {
    status: Status,
    notes: String,
    offer: Option<Offer>,
    /// The person asked (menu, Settings, Update now), so a result or a
    /// failure is worth telling them.
    asked: bool,
}

static STATE: Mutex<Inner> = Mutex::new(Inner { status: Status::Idle, notes: String::new(), offer: None, asked: false });

fn set(status: Status) {
    STATE.lock().unwrap().status = status;
}

/// Records a failure the person should hear about (an install at restart
/// that did not work).
pub fn fail(why: String) {
    let mut g = STATE.lock().unwrap();
    g.status = Status::Failed(why);
    g.asked = true;
}

/// What Settings shows.
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub status: Status,
    pub notes: String,
    pub current: &'static str,
    pub auto_check: bool,
    /// Seconds since 1970 of the last check (0: never).
    pub last_check: u64,
    /// The person asked for this result.
    pub asked: bool,
    /// Later was chosen less than a day ago.
    pub snoozed: bool,
}

impl Snapshot {
    /// Whether the app's update notice should show: something to offer, or
    /// a failure the person asked about, and not put off for today.
    pub fn notice(&self) -> bool {
        let worth = match self.status {
            Status::Found(_) | Status::Downloading(..) | Status::Ready(_) | Status::Available(_) => true,
            Status::Failed(_) => self.asked,
            _ => false,
        };
        worth && !self.snoozed
    }
}

pub fn snapshot() -> Snapshot {
    let s = settings();
    let g = STATE.lock().unwrap();
    let mut status = g.status.clone();
    if status == Status::Idle {
        if let Some(p) = apply::pending() {
            status = Status::Ready(p.version);
        }
    }
    Snapshot { status, notes: g.notes.clone(), current: crate::CURRENT_VERSION, auto_check: s.auto_check, last_check: s.last_check, asked: g.asked, snoozed: now() < s.snoozed_until }
}

/// Later: no notice until this time tomorrow (a check the person asks for
/// shows its result anyway).
pub fn later() {
    save(&Settings { snoozed_until: now() + DAY, ..settings() });
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// At start, and now and then while Studio runs: checks in the background
/// if automatic checks are on and a day has passed since the last one.
pub fn start_background() {
    let s = settings();
    if s.auto_check && now().saturating_sub(s.last_check) > DAY {
        spawn_check(false);
    }
}

/// A check the person asked for (the menu, Settings): runs now, and its
/// result shows even after Later.
pub fn check_now() {
    save(&Settings { snoozed_until: 0, ..settings() });
    spawn_check(true);
}

/// Claims the worker for a check or a download; false if one is running.
fn begin(status: Status, asked: bool) -> bool {
    let mut g = STATE.lock().unwrap();
    if matches!(g.status, Status::Checking | Status::Downloading(..)) {
        return false;
    }
    g.status = status;
    g.asked = asked;
    true
}

fn spawn_check(asked: bool) {
    if !begin(Status::Checking, asked) {
        return;
    }
    let _ = std::thread::Builder::new().name("studio-update".into()).spawn(|| {
        let cfg = crate::config::load();
        let status = match check(&cfg, crate::CURRENT_VERSION, apply::target().as_ref(), &net_fetch) {
            Ok(Found::Offer(o)) if apply::pending().is_some_and(|p| p.version == o.version && p.staged.exists()) => Status::Ready(o.version),
            Ok(Found::Offer(o)) => {
                let v = o.version.clone();
                STATE.lock().unwrap().offer = Some(o);
                Status::Found(v)
            }
            Ok(Found::Available(v)) => Status::Available(v),
            Ok(Found::UpToDate) => Status::UpToDate,
            Err(s) => s,
        };
        if !matches!(status, Status::Disabled(_)) {
            save(&Settings { last_check: now(), ..settings() });
        }
        set(status);
    });
}

/// Update now: downloads and checks the version found, then it is
/// [`Status::Ready`] to install at restart.
pub fn update_now() {
    let Some(offer) = STATE.lock().unwrap().offer.clone() else { return check_now() };
    if !begin(Status::Downloading(0, offer.entry.size), true) {
        return;
    }
    let _ = std::thread::Builder::new().name("studio-update".into()).spawn(move || {
        let cfg = crate::config::load();
        let status = match apply::target() {
            Some(t) => fetch(&cfg, &offer, &t, &net_download).unwrap_or_else(Status::Failed),
            None => Status::Available(offer.version.clone()),
        };
        set(status);
    });
}

/// The feed over HTTP(S).
pub fn net_fetch(url: &str) -> Result<Vec<u8>, String> {
    let r = studio_net::get(url, &[], Duration::from_secs(20))?;
    if r.status != 200 {
        return Err(format!("the update server answered {} for {url}", r.status));
    }
    r.bytes(2 << 20).map_err(|e| e.to_string())
}

/// An archive over HTTP(S), resuming a partial file; progress goes to the
/// status.
pub fn net_download(url: &str, dest: &Path, size: u64) -> Result<(), String> {
    let opts = studio_net::Download { max_bytes: size.max(1) + (1 << 20), ..Default::default() };
    studio_net::download(url, dest, opts, &mut |got| {
        set(Status::Downloading(got, size));
        true
    })
    .map_err(|e| e.to_string())
}

type Fetch<'a> = &'a dyn Fn(&str) -> Result<Vec<u8>, String>;
type Download<'a> = &'a dyn Fn(&str, &Path, u64) -> Result<(), String>;

/// What a check found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Found {
    UpToDate,
    /// Newer, signed for this install, ready to download.
    Offer(Offer),
    /// Newer, but this copy cannot install it itself.
    Available(String),
}

/// A check: the feed and its signature for `target`'s kind of build.
/// Nothing is downloaded. Network access comes in through `fetch` (tests
/// pass fakes). `Err` is the status to show.
pub fn check(cfg: &Config, current: &str, target: Option<&Target>, fetch: Fetch) -> Result<Found, Status> {
    if cfg.feed_url.is_empty() {
        return Err(Status::Disabled("no update feed is configured in this build".into()));
    }
    if cfg.public_key.is_empty() {
        return Err(Status::Disabled("this build has no update key, so it cannot trust an update".into()));
    }
    let go = || -> Result<Found, String> {
        let feed = feed::parse(&fetch(&cfg.feed_url)?)?;
        STATE.lock().unwrap().notes = feed.notes.clone();
        if !feed::is_newer(&feed.version, current) {
            return Ok(Found::UpToDate);
        }
        let Some(target) = target else {
            // Told, so the person can get it from the download page; the
            // folder build's signature still has to hold.
            let key = feed::platform();
            if let Some(e) = feed.platforms.get(&key) {
                feed::verify(&cfg.public_key, &key, &feed.version, e)?;
            }
            return Ok(Found::Available(feed.version));
        };
        let key = target.feed_key();
        let Some(entry) = feed.platforms.get(&key) else {
            return Ok(Found::Available(feed.version));
        };
        // The signature first: nothing is downloaded on a feed we do not trust.
        feed::verify(&cfg.public_key, &key, &feed.version, entry)?;
        Ok(Found::Offer(Offer { version: feed.version.clone(), key, entry: entry.clone() }))
    };
    go().map_err(Status::Failed)
}

/// Downloads `offer` for `target`, checks its size and SHA-256 against the
/// signed entry, unpacks it if it is a folder update, and leaves it waiting
/// for the restart.
pub fn fetch(cfg: &Config, offer: &Offer, target: &Target, download: Download) -> Result<Status, String> {
    if offer.key != target.feed_key() {
        return Err(format!("the update found is for {}, this copy is {}", offer.key, target.feed_key()));
    }
    let entry = &offer.entry;
    let dir = apply::updates_dir().join(&offer.version);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let name = match target {
        Target::Folder(_) => "update.tar.gz",
        Target::Msi => "Studio.msi",
        Target::AppImage(_) => "Studio.AppImage",
    };
    let file = dir.join(name);
    download(&feed::resolve(&cfg.feed_url, &entry.url), &file, entry.size)?;
    let size = std::fs::metadata(&file).map_err(|e| e.to_string())?.len();
    let hash = feed::sha256_file(&file).map_err(|e| e.to_string())?;
    if size != entry.size || !hash.eq_ignore_ascii_case(&entry.sha256) {
        let _ = std::fs::remove_file(&file);
        return Err("the downloaded update does not match the feed (size or SHA-256); it was discarded".into());
    }
    let staged = match target {
        Target::Folder(_) => {
            let staged = dir.join("staged");
            apply::stage(&file, &staged)?;
            let _ = std::fs::remove_file(&file);
            staged
        }
        Target::Msi | Target::AppImage(_) => file,
    };
    apply::save_pending(&Pending { version: offer.version.clone(), platform: offer.key.clone(), staged, entry: entry.clone(), kind: target.kind().into() })?;
    Ok(Status::Ready(offer.version.clone()))
}

/// A check and, when there is something newer, its download: what Update
/// now does after a check, in one call.
pub fn run(cfg: &Config, current: &str, target: Option<&Target>, fetch_feed: Fetch, download: Download) -> Status {
    match check(cfg, current, target, fetch_feed) {
        Ok(Found::UpToDate) => Status::UpToDate,
        Ok(Found::Available(v)) => Status::Available(v),
        Ok(Found::Offer(o)) => {
            if apply::pending().is_some_and(|p| p.version == o.version && p.staged.exists()) {
                return Status::Ready(o.version);
            }
            fetch(cfg, &o, target.expect("an offer is made only for a target"), download).unwrap_or_else(Status::Failed)
        }
        Err(s) => s,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feed::tests::{keypair, signed};
    use std::collections::BTreeMap;

    fn feed_bytes(entry: Option<feed::Entry>, version: &str) -> Vec<u8> {
        let mut platforms = BTreeMap::new();
        if let Some(e) = entry {
            platforms.insert(feed::platform(), e);
        }
        serde_json::to_vec(&feed::Feed { schema: 1, version: version.into(), notes: "fixes".into(), date: String::new(), platforms }).unwrap()
    }

    fn cfg(public_key: &str) -> Config {
        Config { feed_url: "https://u.test/feed.json".into(), public_key: public_key.into(), crash_endpoint: String::new() }
    }

    fn snap(status: Status, asked: bool, snoozed: bool) -> Snapshot {
        Snapshot { status, notes: String::new(), current: "0.1.0", auto_check: true, last_check: 0, asked, snoozed }
    }

    #[test]
    fn the_notice_shows_offers_and_asked_failures_and_respects_later() {
        assert!(snap(Status::Found("0.2.0".into()), false, false).notice());
        assert!(snap(Status::Ready("0.2.0".into()), false, false).notice());
        assert!(!snap(Status::Found("0.2.0".into()), false, true).notice(), "Later keeps it away");
        assert!(!snap(Status::Failed("offline".into()), false, false).notice(), "a background check failing is quiet");
        assert!(snap(Status::Failed("offline".into()), true, false).notice());
        assert!(!snap(Status::UpToDate, true, false).notice());
        assert!(!snap(Status::Disabled("no feed".into()), true, false).notice());
    }

    #[test]
    fn a_check_finds_the_build_for_its_kind_of_install_and_downloads_nothing() {
        let (kp, public) = keypair();
        let folder = signed(&kp, &feed::platform(), "0.2.0", b"tar");
        let appimage = signed(&kp, &format!("{}-appimage", feed::platform()), "0.2.0", b"appimage");
        let mut platforms = BTreeMap::new();
        platforms.insert(feed::platform(), folder.clone());
        platforms.insert(format!("{}-appimage", feed::platform()), appimage.clone());
        let bytes = serde_json::to_vec(&feed::Feed { schema: 1, version: "0.2.0".into(), notes: String::new(), date: String::new(), platforms }).unwrap();
        let fetch = |_: &str| Ok(bytes.clone());
        let t = Target::Folder("/nonexistent".into());
        assert_eq!(check(&cfg(&public), "0.1.0", Some(&t), &fetch), Ok(Found::Offer(Offer { version: "0.2.0".into(), key: feed::platform(), entry: folder })));
        let a = Target::AppImage("/nonexistent/Studio.AppImage".into());
        assert!(matches!(check(&cfg(&public), "0.1.0", Some(&a), &fetch), Ok(Found::Offer(o)) if o.entry == appimage));
        // No MSI in this feed: the person is told, nothing more.
        assert_eq!(check(&cfg(&public), "0.1.0", Some(&Target::Msi), &fetch), Ok(Found::Available("0.2.0".into())));
        // An offer is fetched only for the install it was signed for.
        let o = Offer { version: "0.2.0".into(), key: feed::platform(), entry: signed(&kp, &feed::platform(), "0.2.0", b"tar") };
        let dl = |_: &str, _: &Path, _: u64| -> Result<(), String> { panic!("must not download") };
        assert!(fetch_offer_err(&cfg(&public), &o, &a, &dl).contains("this copy is"));
    }

    fn fetch_offer_err(c: &Config, o: &Offer, t: &Target, dl: Download) -> String {
        fetch(c, o, t, dl).unwrap_err()
    }

    #[test]
    fn not_configured_is_said_plainly() {
        let none = |_: &str| -> Result<Vec<u8>, String> { panic!("no network without a feed") };
        let dl = |_: &str, _: &Path, _: u64| -> Result<(), String> { panic!() };
        assert!(matches!(run(&Config::default(), "0.1.0", None, &none, &dl), Status::Disabled(m) if m.contains("feed")));
        let c = Config { feed_url: "https://x/f".into(), ..Config::default() };
        assert!(matches!(run(&c, "0.1.0", None, &none, &dl), Status::Disabled(m) if m.contains("key")));
    }

    #[test]
    fn current_and_older_feeds_are_up_to_date() {
        let (kp, public) = keypair();
        let bytes = feed_bytes(Some(signed(&kp, &feed::platform(), "0.1.0", b"x")), "0.1.0");
        let fetch = |_: &str| Ok(bytes.clone());
        let dl = |_: &str, _: &Path, _: u64| -> Result<(), String> { panic!("nothing to download") };
        assert_eq!(run(&cfg(&public), "0.1.0", None, &fetch, &dl), Status::UpToDate);
    }

    #[test]
    fn a_feed_signed_with_another_key_downloads_nothing() {
        let (kp, _) = keypair();
        let (_, trusted) = keypair();
        let bytes = feed_bytes(Some(signed(&kp, &feed::platform(), "0.2.0", b"x")), "0.2.0");
        let fetch = |_: &str| Ok(bytes.clone());
        let dl = |_: &str, _: &Path, _: u64| -> Result<(), String> { panic!("must not download") };
        let r = run(&cfg(&trusted), "0.1.0", Some(&Target::Folder("/nonexistent".into())), &fetch, &dl);
        assert!(matches!(r, Status::Failed(m) if m.contains("signature")));
    }

    #[test]
    fn a_dev_build_is_told_but_does_not_download() {
        let (kp, public) = keypair();
        let bytes = feed_bytes(Some(signed(&kp, &feed::platform(), "0.2.0", b"x")), "0.2.0");
        let fetch = |_: &str| Ok(bytes.clone());
        let dl = |_: &str, _: &Path, _: u64| -> Result<(), String> { panic!() };
        assert_eq!(run(&cfg(&public), "0.1.0", None, &fetch, &dl), Status::Available("0.2.0".into()));
    }

    #[test]
    fn a_tampered_download_is_discarded() {
        let (kp, public) = keypair();
        let bytes = feed_bytes(Some(signed(&kp, &feed::platform(), "9.9.9", b"the real archive")), "9.9.9");
        let fetch = |_: &str| Ok(bytes.clone());
        let dl = |_: &str, dest: &Path, _: u64| std::fs::write(dest, b"evil archive!!!!").map_err(|e| e.to_string());
        let r = run(&cfg(&public), "0.1.0", Some(&Target::Folder("/nonexistent".into())), &fetch, &dl);
        assert!(matches!(r, Status::Failed(ref m) if m.contains("does not match")), "{r:?}");
        let _ = std::fs::remove_dir_all(apply::updates_dir().join("9.9.9"));
    }
}
