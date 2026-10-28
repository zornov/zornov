// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! API keys kept in the system's own secret store, on every platform:
//!
//! | platform | where                                                         |
//! |----------|---------------------------------------------------------------|
//! | macOS    | the login Keychain (Security framework)                       |
//! | Windows  | Windows Credential Manager (a generic credential per key)     |
//! | Linux    | the Secret Service over D-Bus (GNOME Keyring, KWallet, KeePassXC) |
//!
//! Where no store answers (a Linux desktop with no keyring running, a
//! Windows account whose Credential Manager refuses), the key goes to a
//! file only the person can read (`0600`) in Zornov's data folder, and
//! [`set`] says so with [`Kept::File`] so the app can tell the person
//! plainly. On macOS a Keychain that refuses is an error, not a reason to
//! write a file. Reading looks in the store first, then the file.
//!
//! A key is found by a service and an account name. It never appears on a
//! command line.

use std::path::{Path, PathBuf};

#[cfg(target_os = "macos")]
mod mac;
#[cfg(all(unix, not(target_os = "macos")))]
mod freedesktop;
#[cfg(windows)]
mod windows;

pub mod file;

/// Where a key was saved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kept {
    /// The system's store ([`system_name`]).
    System,
    /// The private file at this path: no system store answered.
    File(PathBuf),
}

/// Why the system's store did not do what was asked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fail {
    /// There is no store to ask (no keyring running, no D-Bus session):
    /// the private file stands in.
    Unavailable(String),
    /// The store answered no (the person declined, the key was refused).
    Refused(String),
}

impl Fail {
    fn message(self) -> String {
        match self {
            Fail::Unavailable(m) | Fail::Refused(m) => m,
        }
    }
}

/// The system's store, in words: "your login Keychain", "Windows
/// Credential Manager", "the system keyring".
pub fn system_name() -> &'static str {
    if cfg!(target_os = "macos") {
        "your login Keychain"
    } else if cfg!(windows) {
        "Windows Credential Manager"
    } else {
        "the system keyring"
    }
}

/// The private file keys go to when no system store answers.
pub fn fallback_file() -> PathBuf {
    studio_paths::data_dir().join("api-keys.json")
}

/// What to tell the person when a key went to the private file.
pub fn file_notice(path: &Path) -> String {
    if cfg!(windows) {
        format!("Windows Credential Manager did not take the key, so it is kept in a file in your own app data folder: {}", path.display())
    } else {
        format!("No system keyring answered (GNOME Keyring, KWallet or another Secret Service), so the key is kept in a file only you can read: {}. Start a keyring and save it again to keep it there instead.", path.display())
    }
}

fn store_get(service: &str, account: &str) -> Result<Option<String>, Fail> {
    #[cfg(target_os = "macos")]
    return mac::get(service, account);
    #[cfg(windows)]
    return windows::get(service, account);
    #[cfg(all(unix, not(target_os = "macos")))]
    return freedesktop::get(service, account);
    #[allow(unreachable_code)]
    {
        let _ = (service, account);
        Err(Fail::Unavailable("no secret store on this system".into()))
    }
}

fn store_set(service: &str, account: &str, secret: &str) -> Result<(), Fail> {
    #[cfg(target_os = "macos")]
    return mac::set(service, account, secret);
    #[cfg(windows)]
    return windows::set(service, account, secret);
    #[cfg(all(unix, not(target_os = "macos")))]
    return freedesktop::set(service, account, secret);
    #[allow(unreachable_code)]
    {
        let _ = (service, account, secret);
        Err(Fail::Unavailable("no secret store on this system".into()))
    }
}

fn store_remove(service: &str, account: &str) {
    #[cfg(target_os = "macos")]
    mac::remove(service, account);
    #[cfg(windows)]
    windows::remove(service, account);
    #[cfg(all(unix, not(target_os = "macos")))]
    freedesktop::remove(service, account);
    let _ = (service, account);
}

/// The saved key, if any: the system's store first, then the private file.
pub fn get(service: &str, account: &str) -> Option<String> {
    let clean = |s: String| Some(s.trim().to_owned()).filter(|s| !s.is_empty());
    match store_get(service, account) {
        Ok(Some(s)) => clean(s),
        _ => file::get(&fallback_file(), service, account).and_then(clean),
    }
}

/// Where a saved key is, if there is one.
pub fn kept(service: &str, account: &str) -> Option<Kept> {
    if matches!(store_get(service, account), Ok(Some(s)) if !s.trim().is_empty()) {
        return Some(Kept::System);
    }
    let path = fallback_file();
    file::get(&path, service, account).filter(|s| !s.trim().is_empty()).map(|_| Kept::File(path))
}

/// Saves (or replaces) a key in the system's store; where there is none
/// (not on macOS), in the private file. Says where it went.
pub fn set(service: &str, account: &str, secret: &str) -> Result<Kept, String> {
    set_with(service, account, secret, store_set, &fallback_file())
}

/// [`set`] with the store and the file given (tests).
fn set_with(service: &str, account: &str, secret: &str, store: impl Fn(&str, &str, &str) -> Result<(), Fail>, path: &Path) -> Result<Kept, String> {
    match store(service, account, secret) {
        Ok(()) => {
            // An older copy in the file would shadow nothing, but would linger.
            file::remove(path, service, account);
            Ok(Kept::System)
        }
        Err(Fail::Unavailable(why)) if !cfg!(target_os = "macos") => {
            file::set(path, service, account, secret).map_err(|e| format!("{why}; the key could not be kept in {} either: {e}", path.display()))?;
            Ok(Kept::File(path.to_owned()))
        }
        Err(f) => Err(f.message()),
    }
}

/// Removes a key from the system's store and the private file.
pub fn remove(service: &str, account: &str) {
    store_remove(service, account);
    file::remove(&fallback_file(), service, account);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("studio-keystore-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("api-keys.json")
    }

    #[test]
    fn a_store_that_answers_keeps_the_key_and_clears_the_file() {
        let path = temp("answers");
        file::set(&path, "svc", "acct", "old-key").unwrap();
        let got = set_with("svc", "acct", "new-key", |_, _, _| Ok(()), &path).unwrap();
        assert_eq!(got, Kept::System);
        assert_eq!(file::get(&path, "svc", "acct"), None, "the file no longer holds it");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn with_no_store_the_key_goes_to_the_private_file_and_says_so() {
        let path = temp("none");
        let got = set_with("svc", "acct", "the-key", |_, _, _| Err(Fail::Unavailable("no keyring".into())), &path);
        if cfg!(target_os = "macos") {
            // The Keychain is always there: refusing is an error, not a file.
            assert_eq!(got, Err("no keyring".into()));
        } else {
            assert_eq!(got, Ok(Kept::File(path.clone())));
            assert_eq!(file::get(&path, "svc", "acct").as_deref(), Some("the-key"));
            assert!(file_notice(&path).contains(&path.display().to_string()));
        }
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_refusal_is_an_error_not_a_file() {
        let path = temp("refused");
        let got = set_with("svc", "acct", "the-key", |_, _, _| Err(Fail::Refused("you said no".into())), &path);
        assert_eq!(got, Err("you said no".into()));
        assert!(!path.exists());
    }

    #[test]
    fn every_platform_names_its_store() {
        assert!(!system_name().is_empty());
        if cfg!(windows) {
            assert_eq!(system_name(), "Windows Credential Manager");
        }
    }
}
