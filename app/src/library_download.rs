// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The content library as a download. Installers ship without it; when it
//! is missing, the Assets and Audio panels offer it. The archive named in
//! `packaging/library.conf` is fetched (resuming after a dropped
//! connection), checked against the SHA-256 this build was made with,
//! unpacked into the data folder and loaded at once, without a restart.

use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// What `packaging/library.conf` says the download is.
#[derive(Clone, Debug, PartialEq)]
pub struct Conf {
    pub url: String,
    pub size: u64,
    pub sha256: String,
}

/// Reads `key = value` lines; `None` when a field is missing.
pub fn parse(text: &str) -> Option<Conf> {
    let (mut url, mut size, mut sha256) = (None, None, None);
    for line in text.lines().map(str::trim).filter(|l| !l.starts_with('#')) {
        let Some((k, v)) = line.split_once('=') else { continue };
        let v = v.trim();
        match k.trim() {
            "url" => url = Some(v.to_owned()),
            "size" => size = v.parse().ok(),
            "sha256" => sha256 = Some(v.to_ascii_lowercase()),
            _ => {}
        }
    }
    Some(Conf { url: url?, size: size?, sha256: sha256? })
}

pub fn conf() -> Option<Conf> {
    parse(include_str!("../../packaging/library.conf"))
}

/// How to get the library, in words for an agent to pass on: the app's
/// own offer, or the shell command that does the same.
pub fn how_to_get() -> String {
    let d = dir();
    match conf() {
        Some(c) => format!(
            "Zornov's content library (pictures, footage, music, sounds) is not installed. Get it in the app (Assets or Audio panel: Download library, {} MB), or from a shell: mkdir -p \"{}\" && curl -L {} | tar xz -C \"{}\" (then restart Zornov), or point STUDIO_CONTENT at a copy you already have",
            c.size.div_ceil(1 << 20),
            d.display(),
            c.url,
            d.display()
        ),
        None => format!("Zornov's content library is not installed: download it from the app's Assets panel, or unpack it into \"{}\", or point STUDIO_CONTENT at a copy", d.display()),
    }
}

/// Where a downloaded library lives: its `core/` folder is inside.
pub fn dir() -> PathBuf {
    studio_paths::data_dir().join("library")
}

#[derive(Clone, Debug, PartialEq)]
pub enum State {
    Idle,
    Downloading { got: u64, total: u64 },
    Unpacking,
    Failed(String),
    Done,
}

static STATE: Mutex<State> = Mutex::new(State::Idle);
static CANCEL: AtomicBool = AtomicBool::new(false);

pub fn state() -> State {
    STATE.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

pub(crate) fn set(s: State) {
    *STATE.lock().unwrap_or_else(|e| e.into_inner()) = s;
}

/// The download's size in whole megabytes, for the offer.
pub fn size_mb() -> Option<u64> {
    conf().map(|c| c.size.div_ceil(1 << 20))
}

/// Starts the download on its own thread (nothing when one is running).
pub fn start(ctx: &eframe::egui::Context) {
    if matches!(state(), State::Downloading { .. } | State::Unpacking) {
        return;
    }
    let Some(c) = conf() else {
        set(State::Failed("this copy of Zornov does not say where the library is".into()));
        return;
    };
    CANCEL.store(false, Ordering::Relaxed);
    set(State::Downloading { got: 0, total: c.size });
    let ctx = ctx.clone();
    let _ = std::thread::Builder::new().name("library-download".into()).spawn(move || {
        set(match fetch(&c, &ctx) {
            Ok(()) => {
                crate::content::installed();
                State::Done
            }
            Err(None) => State::Idle,
            Err(Some(e)) => State::Failed(e),
        });
        ctx.request_repaint();
    });
}

/// Stops a download; what has arrived stays for the next try to resume.
pub fn cancel() {
    CANCEL.store(true, Ordering::Relaxed);
}

/// `Err(None)`: cancelled.
fn fetch(c: &Conf, ctx: &eframe::egui::Context) -> Result<(), Option<String>> {
    let data = studio_paths::data_dir();
    std::fs::create_dir_all(&data).map_err(|e| Some(format!("cannot write {}: {e}", data.display())))?;
    let part = data.join("library.tar.gz.part");
    let opts = studio_net::Download { max_bytes: c.size + (1 << 20), stall: Duration::from_secs(60), total: Duration::from_secs(4 * 3600), ..Default::default() };
    let total = c.size;
    studio_net::download(&c.url, &part, opts, &mut |got| {
        set(State::Downloading { got, total });
        ctx.request_repaint_after(Duration::from_millis(100));
        !CANCEL.load(Ordering::Relaxed)
    })
    .map_err(|e| match e {
        studio_net::Failure::Cancelled => None,
        studio_net::Failure::Failed(e) => Some(e),
    })?;
    let sum = studio_update::feed::sha256_file(&part).map_err(|e| Some(e.to_string()))?;
    if sum != c.sha256 {
        let _ = std::fs::remove_file(&part);
        return Err(Some("the download was damaged on the way; try again".into()));
    }
    set(State::Unpacking);
    ctx.request_repaint();
    let staging = data.join("library.new");
    studio_update::apply::unpack(&part, &staging).map_err(Some)?;
    if !staging.join("core/index.json").is_file() {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(Some("the download is not a Zornov library".into()));
    }
    let dest = dir();
    if dest.exists() {
        std::fs::remove_dir_all(&dest).map_err(|e| Some(format!("cannot replace {}: {e}", dest.display())))?;
    }
    std::fs::rename(&staging, &dest).map_err(|e| Some(format!("cannot move the library into place: {e}")))?;
    let _ = std::fs::remove_file(&part);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_built_in_conf_names_a_release_download() {
        let c = conf().expect("packaging/library.conf has url, size and sha256");
        assert!(c.url.starts_with("https://github.com/zornov/zornov/releases/download/library-"), "{}", c.url);
        assert!(c.size > 1 << 20);
        assert_eq!(c.sha256.len(), 64);
    }

    /// The real thing, end to end: fetched from the release, checked against
    /// the built-in SHA-256, unpacked and readable as a library. Run by hand:
    /// `cargo test -p studio-app --bin zornov real_library -- --ignored`.
    #[test]
    #[ignore = "network: downloads the real library (about 280 MB)"]
    fn the_real_library_downloads_checks_and_unpacks() {
        let home = std::env::temp_dir().join(format!("zornov-library-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        // SAFETY: run alone (ignored by default); nothing else reads HOME meanwhile.
        unsafe { std::env::set_var("HOME", &home) };
        let c = conf().unwrap();
        fetch(&c, &eframe::egui::Context::default()).expect("downloads, checks and unpacks");
        let lib = studio_content::bundle::load_all(&dir()).expect("reads as a library");
        let n = lib.items().count();
        assert!(n > 1000, "{n} items");
        assert!(dir().starts_with(&home));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn a_conf_without_its_checksum_is_refused() {
        assert_eq!(parse("url = https://x/y.tar.gz\nsize = 10\n"), None);
        let c = parse("# note\nurl = https://x/y.tar.gz\nsize = 10\nsha256 = AB\n").unwrap();
        assert_eq!((c.size, c.sha256.as_str()), (10, "ab"));
    }
}
