// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The last resort: keys in a JSON file only the person can read. On Unix
//! the file is created `0600` (never readable by others, not even for a
//! moment) and written whole to a new file that replaces the old one; a
//! file found with wider permissions is narrowed before it is read. On
//! Windows it lives in the person's own app data folder, which only they
//! can open.

use serde_json::{Map, Value};
use std::io::Write as _;
use std::path::Path;

fn name(service: &str, account: &str) -> String {
    format!("{service}/{account}")
}

fn all(path: &Path) -> Map<String, Value> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(m) = std::fs::metadata(path)
            && m.permissions().mode() & 0o077 != 0
        {
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
    }
    std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn save(path: &Path, m: &Map<String, Value>) -> Result<(), String> {
    let dir = path.parent().ok_or("the key file has no folder")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let tmp = path.with_extension("json.new");
    let _ = std::fs::remove_file(&tmp);
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    let mut f = o.open(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
    let text = Value::Object(m.clone()).to_string();
    f.write_all(text.as_bytes()).and_then(|_| f.sync_all()).map_err(|e| format!("{}: {e}", tmp.display()))?;
    drop(f);
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

/// The key saved for `service` and `account`.
pub fn get(path: &Path, service: &str, account: &str) -> Option<String> {
    all(path).get(&name(service, account)).and_then(Value::as_str).map(str::to_owned)
}

/// Saves (or replaces) a key.
pub fn set(path: &Path, service: &str, account: &str, secret: &str) -> Result<(), String> {
    let mut m = all(path);
    m.insert(name(service, account), Value::String(secret.to_owned()));
    save(path, &m)
}

/// Removes a key (the file goes when it holds none).
pub fn remove(path: &Path, service: &str, account: &str) {
    let mut m = all(path);
    if m.remove(&name(service, account)).is_some() {
        if m.is_empty() {
            let _ = std::fs::remove_file(path);
        } else {
            let _ = save(path, &m);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("studio-keystore-file-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("api-keys.json")
    }

    #[test]
    fn keys_are_kept_replaced_and_removed() {
        let p = temp("keep");
        assert_eq!(get(&p, "s", "a"), None);
        set(&p, "s", "a", "one").unwrap();
        set(&p, "s", "b", "other").unwrap();
        assert_eq!(get(&p, "s", "a").as_deref(), Some("one"));
        set(&p, "s", "a", "two").unwrap();
        assert_eq!(get(&p, "s", "a").as_deref(), Some("two"));
        assert_eq!(get(&p, "t", "a"), None, "another service's key is its own");
        remove(&p, "s", "a");
        assert_eq!(get(&p, "s", "a"), None);
        assert_eq!(get(&p, "s", "b").as_deref(), Some("other"));
        remove(&p, "s", "b");
        assert!(!p.exists(), "an empty file goes");
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn only_the_person_can_read_the_file() {
        use std::os::unix::fs::PermissionsExt;
        let p = temp("mode");
        set(&p, "s", "a", "secret").unwrap();
        assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
        // A file someone widened is narrowed again when it is read.
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(get(&p, "s", "a").as_deref(), Some("secret"));
        assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }
}
