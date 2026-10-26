// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Standard folders on each platform, so nothing assumes a macOS layout.
//!
//! | folder  | macOS                    | Windows                       | Linux                                  |
//! |---------|--------------------------|-------------------------------|----------------------------------------|
//! | home    | `$HOME`                  | `%USERPROFILE%`               | `$HOME`                                |
//! | videos  | `~/Movies`               | `%USERPROFILE%\Videos`        | `$XDG_VIDEOS_DIR` or `~/Videos`        |
//! | studio  | `~/Movies/Studio`        | `%USERPROFILE%\Videos\Studio` | `~/Videos/Studio`                      |
//! | cache   | `~/Library/Caches/Studio`| `%LOCALAPPDATA%\Studio\Cache` | `$XDG_CACHE_HOME/studio` or `~/.cache/studio` |
//!
//! The Studio folder (projects, models, content) lives with the person's
//! videos on every platform, as it always has on macOS.

use std::path::PathBuf;

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// The person's home folder.
pub fn home() -> PathBuf {
    if cfg!(windows) {
        env_path("USERPROFILE").or_else(|| env_path("HOME")).unwrap_or_else(|| PathBuf::from("."))
    } else {
        env_path("HOME").unwrap_or_else(|| PathBuf::from("."))
    }
}

/// The person's videos folder.
pub fn videos() -> PathBuf {
    if cfg!(target_os = "macos") {
        home().join("Movies")
    } else if cfg!(windows) {
        home().join("Videos")
    } else {
        env_path("XDG_VIDEOS_DIR").unwrap_or_else(|| home().join("Videos"))
    }
}

/// Studio's own folder: projects, models, content.
pub fn studio() -> PathBuf {
    videos().join("Studio")
}

/// Disposable caches.
pub fn cache() -> PathBuf {
    if cfg!(target_os = "macos") {
        home().join("Library/Caches/Studio")
    } else if cfg!(windows) {
        env_path("LOCALAPPDATA").unwrap_or_else(|| home().join("AppData").join("Local")).join("Studio").join("Cache")
    } else {
        env_path("XDG_CACHE_HOME").unwrap_or_else(|| home().join(".cache")).join("studio")
    }
}

/// Replaces the home folder at the start of `p` with `~`, for display.
pub fn tilde(p: &std::path::Path) -> String {
    let h = home();
    match p.strip_prefix(&h) {
        Ok(rest) if !h.as_os_str().is_empty() => format!("~{}{}", std::path::MAIN_SEPARATOR, rest.display()),
        _ => p.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folders_nest_under_home() {
        assert!(studio().starts_with(home()) || std::env::var_os("XDG_VIDEOS_DIR").is_some());
        assert!(studio().ends_with("Studio"));
        assert!(!cache().as_os_str().is_empty());
        assert_eq!(tilde(&home().join("a")), format!("~{}a", std::path::MAIN_SEPARATOR));
    }
}
