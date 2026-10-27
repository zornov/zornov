// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Standard folders on each platform, so nothing assumes a macOS layout.
//!
//! | folder  | macOS                    | Windows                       | Linux                                  |
//! |---------|--------------------------|-------------------------------|----------------------------------------|
//! | home    | `$HOME`                  | `%USERPROFILE%`               | `$HOME`                                |
//! | videos  | `~/Movies`               | `%USERPROFILE%\Videos`        | `$XDG_VIDEOS_DIR` or `~/Videos`        |
//! | studio  | `~/Movies/Studio`        | `%USERPROFILE%\Videos\Studio` | `~/Videos/Studio`                      |
//! | data    | `~/Library/Application Support/Zornov` | `%LOCALAPPDATA%\Zornov` | `$XDG_STATE_HOME/zornov` or `~/.local/state/zornov` |
//! | cache   | `~/Library/Caches/Zornov`| `%LOCALAPPDATA%\Zornov\Cache` | `$XDG_CACHE_HOME/zornov` or `~/.cache/zornov` |
//!
//! The projects folder (projects, models, content) lives with the person's
//! videos on every platform, as it always has on macOS. It keeps its name
//! so nobody's projects move. The data and cache folders were called
//! `Studio` before the app was named Zornov; [`migrate_legacy`] moves them.

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

/// The library of things made and kept (models, moves, designs, sounds):
/// `<studio>/Made`, the one place every part of the app saves to and reads
/// from. A `Made` folder some builds left under `<videos>/Zornov` is moved in
/// on first use.
pub fn made() -> PathBuf {
    static ONCE: std::sync::Once = std::sync::Once::new();
    let dir = studio().join("Made");
    ONCE.call_once(|| move_folder(&videos().join("Zornov").join("Made"), &dir));
    dir
}

/// Disposable caches.
pub fn cache() -> PathBuf {
    migrate_legacy();
    if cfg!(target_os = "macos") {
        home().join("Library/Caches/Zornov")
    } else if cfg!(windows) {
        local_app_data().join("Zornov").join("Cache")
    } else {
        xdg_cache().join("zornov")
    }
}

fn local_app_data() -> PathBuf {
    env_path("LOCALAPPDATA").unwrap_or_else(|| home().join("AppData").join("Local"))
}

fn xdg_cache() -> PathBuf {
    env_path("XDG_CACHE_HOME").unwrap_or_else(|| home().join(".cache"))
}

/// Zornov's per-user data folder (settings, signing key, staged updates,
/// crash reports, account): `~/Library/Application Support/Zornov`,
/// `%LOCALAPPDATA%\Zornov` or `$XDG_STATE_HOME/zornov`. A folder left by
/// the app's earlier name is moved here on first use.
pub fn data_dir() -> PathBuf {
    migrate_legacy();
    data_dir_unmigrated()
}

fn data_dir_unmigrated() -> PathBuf {
    if cfg!(target_os = "macos") {
        home().join("Library/Application Support/Zornov")
    } else if cfg!(windows) {
        local_app_data().join("Zornov")
    } else {
        env_path("XDG_STATE_HOME").unwrap_or_else(|| home().join(".local/state")).join("zornov")
    }
}

/// Moves the folders the app kept under its old name (`Studio`) to the
/// Zornov names, once per run: settings, the update signing key, the window
/// layout and caches all carry over. Nothing is deleted: a folder that
/// cannot be moved is left where it is.
pub fn migrate_legacy() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        for (old, new) in legacy_pairs() {
            move_folder(&old, &new);
        }
    });
}

/// Each old (`Studio`) folder with the Zornov folder it becomes.
fn legacy_pairs() -> Vec<(PathBuf, PathBuf)> {
    let h = home();
    let mut pairs: Vec<(PathBuf, PathBuf)> = Vec::new();
    if cfg!(target_os = "macos") {
        let support = h.join("Library/Application Support");
        pairs.push((support.join("Studio"), support.join("Zornov")));
        // The window layout egui keeps (a case-sensitive disk keeps it apart).
        pairs.push((support.join("studio"), support.join("zornov")));
        pairs.push((h.join("Library/Caches/Studio"), h.join("Library/Caches/Zornov")));
    } else if cfg!(windows) {
        let local = local_app_data();
        pairs.push((local.join("Studio"), local.join("Zornov")));
        if let Some(roaming) = env_path("APPDATA") {
            pairs.push((roaming.join("studio"), roaming.join("zornov")));
        }
    } else {
        let state = env_path("XDG_STATE_HOME").unwrap_or_else(|| h.join(".local/state"));
        pairs.push((state.join("studio"), state.join("zornov")));
        let share = env_path("XDG_DATA_HOME").unwrap_or_else(|| h.join(".local/share"));
        pairs.push((share.join("studio"), share.join("zornov")));
        pairs.push((xdg_cache().join("studio"), xdg_cache().join("zornov")));
    }
    pairs
}

/// Moves `old` to `new`. When `new` already exists (a new build ran first),
/// whatever `new` lacks is moved in from `old`, and `old` is removed once it
/// is empty. Best effort: an error leaves the rest where it was.
pub fn move_folder(old: &std::path::Path, new: &std::path::Path) {
    if !old.is_dir() || old == new {
        return;
    }
    if !new.exists() {
        if let Some(parent) = new.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if std::fs::rename(old, new).is_ok() {
            return;
        }
    }
    if std::fs::create_dir_all(new).is_err() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(old) else { return };
    for e in entries.flatten() {
        let to = new.join(e.file_name());
        if !to.exists() {
            let _ = std::fs::rename(e.path(), &to);
        } else if e.path().is_dir() && to.is_dir() {
            move_folder(&e.path(), &to);
        }
    }
    let _ = std::fs::remove_dir(old);
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
        assert!(data_dir_unmigrated().ends_with("Zornov") || data_dir_unmigrated().ends_with("zornov"));
        assert_eq!(tilde(&home().join("a")), format!("~{}a", std::path::MAIN_SEPARATOR));
    }

    #[test]
    fn the_legacy_folders_are_studio_and_become_zornov_and_the_projects_folder_stays() {
        assert!(!legacy_pairs().is_empty());
        for (old, new) in legacy_pairs() {
            let (o, n) = (old.file_name().unwrap().to_string_lossy().to_lowercase(), new.file_name().unwrap().to_string_lossy().to_lowercase());
            assert_eq!((o.as_str(), n.as_str()), ("studio", "zornov"), "{} -> {}", old.display(), new.display());
        }
        // The folder with the person's projects keeps its name.
        assert_eq!(studio().file_name().unwrap(), "Studio");
        assert!(data_dir_unmigrated().starts_with(home()) || std::env::var_os("XDG_STATE_HOME").is_some());
    }

    #[test]
    fn an_old_folder_moves_to_the_new_name_and_keeps_its_files() {
        let root = std::env::temp_dir().join(format!("zornov-paths-move-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let old = root.join("Studio");
        std::fs::create_dir_all(old.join("signing")).unwrap();
        std::fs::write(old.join("signing/update-key.hex"), "ab12").unwrap();
        std::fs::write(old.join("settings.json"), "old").unwrap();
        let new = root.join("Zornov");
        // A new build already wrote one file: it wins, the rest moves in.
        std::fs::create_dir_all(&new).unwrap();
        std::fs::write(new.join("settings.json"), "new").unwrap();
        move_folder(&old, &new);
        assert_eq!(std::fs::read_to_string(new.join("signing/update-key.hex")).unwrap(), "ab12");
        assert_eq!(std::fs::read_to_string(new.join("settings.json")).unwrap(), "new");
        assert!(old.join("settings.json").exists(), "a file the new folder already has stays put");
        // Without a new folder the whole folder is renamed.
        let old2 = root.join("Studio2");
        std::fs::create_dir_all(&old2).unwrap();
        std::fs::write(old2.join("a"), "1").unwrap();
        let new2 = root.join("Zornov2");
        move_folder(&old2, &new2);
        assert_eq!(std::fs::read_to_string(new2.join("a")).unwrap(), "1");
        assert!(!old2.exists());
        let _ = std::fs::remove_dir_all(&root);
    }
}
