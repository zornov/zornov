// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Project safety: nothing the person made is lost, and they can always get
//! back to where they were.
//!
//! * history: the autosaves in `backups/`, named snapshots, and restoring
//!   any of them (the current project is kept as a restore point first);
//! * sessions: where the window was (project, playhead, selection), written
//!   as it changes and removed on a clean quit; one left behind with no
//!   process is a crash, and the next start reopens it where it was;
//! * backups: to a chosen folder or drive, incremental (media stored once
//!   by content), verified, with the newest N kept;
//! * collect / archive: a self-contained copy of the project with every file
//!   it uses, checksummed, that opens anywhere;
//! * health: missing files, offline drives, sizes, disk space, backups;
//! * cleanup: removes only regenerable caches and old autosaves, and never
//!   anything a project refers to.
//!
//! Nothing here changes or deletes the person's own media.

use crate::asset_meta;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use studio_model as m;
use studio_model::file::{ProjectFolder, autosave};

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

pub fn when(ms: u64) -> String {
    asset_meta::civil(ms / 1000)
}

pub fn human(bytes: u64) -> String {
    let b = bytes as f64;
    if b >= 1e9 {
        format!("{:.1} GB", b / 1e9)
    } else if b >= 1e6 {
        format!("{:.1} MB", b / 1e6)
    } else if b >= 1e3 {
        format!("{:.0} KB", b / 1e3)
    } else {
        format!("{bytes} B")
    }
}

fn dir_size(p: &Path) -> u64 {
    let Ok(md) = std::fs::symlink_metadata(p) else { return 0 };
    if md.is_file() {
        return md.len();
    }
    if !md.is_dir() {
        return 0;
    }
    std::fs::read_dir(p).map(|r| r.filter_map(|e| e.ok()).map(|e| dir_size(&e.path())).sum()).unwrap_or(0)
}

/// Free bytes on the volume holding `p` (the nearest folder that exists).
pub fn free_space(p: &Path) -> Option<u64> {
    let mut at = p.to_path_buf();
    while !at.exists() {
        at = at.parent()?.to_path_buf();
    }
    free_at(&at)
}

/// Refuses a write of `bytes` to `p` that would leave less than 1 GB free
/// on its disk, with a message that says how much there is (a full disk
/// otherwise fails halfway through a model or a render).
pub fn room_for(p: &Path, bytes: u64) -> Result<(), String> {
    const SPARE: u64 = 1_000_000_000;
    match free_space(p) {
        Some(free) if free < bytes.saturating_add(SPARE) => Err(format!("the disk holding {} has {} free and this needs {} plus 1 GB to spare; free some space (project_care {{\"action\": \"health\"}} shows what the project holds) and try again", p.display(), human(free), human(bytes))),
        _ => Ok(()),
    }
}

#[cfg(windows)]
fn free_at(at: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    let wide: Vec<u16> = at.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    let mut avail = 0u64;
    let ok = unsafe { windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(wide.as_ptr(), &mut avail, std::ptr::null_mut(), std::ptr::null_mut()) };
    (ok != 0).then_some(avail)
}

#[cfg(unix)]
fn free_at(at: &Path) -> Option<u64> {
    let c = std::ffi::CString::new(at.to_string_lossy().as_bytes()).ok()?;
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut s) } != 0 {
        return None;
    }
    Some(s.f_bavail as u64 * s.f_frsize as u64)
}

// ---------------------------------------------------------------- history

const RESTORE: &str = "before-restore-";
const NAMED: &str = "keep-";

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub path: PathBuf,
    pub at_ms: u64,
    pub kind: &'static str,
    pub label: String,
    pub bytes: u64,
    pub clips: usize,
    pub media: usize,
    pub readable: bool,
}

impl Snapshot {
    pub fn json(&self, index: usize) -> Value {
        json!({"n": index, "file": self.path.file_name().map(|n| n.to_string_lossy().into_owned()), "at": when(self.at_ms), "kind": self.kind, "label": self.label, "size": human(self.bytes), "clips": self.clips, "media": self.media, "readable": self.readable})
    }
}

fn stamp(name: &str) -> Option<(&'static str, u64, String)> {
    let stem = name.strip_suffix(".json")?;
    if let Some(r) = stem.strip_prefix("autosave-") {
        return Some(("autosave", r.parse().ok()?, String::new()));
    }
    for (prefix, kind) in [(RESTORE, "restore point"), (NAMED, "kept")] {
        if let Some(r) = stem.strip_prefix(prefix) {
            let (ms, label) = r.split_once('-').map_or((r, ""), |(a, b)| (a, b));
            return Some((kind, ms.parse().ok()?, label.replace('_', " ")));
        }
    }
    None
}

/// Every snapshot of a project, newest first: autosaves, restore points and
/// the ones the person kept.
pub fn history(folder: &Path) -> Vec<Snapshot> {
    let dir = ProjectFolder::new(folder).backups_dir();
    let mut out: Vec<Snapshot> = std::fs::read_dir(dir)
        .map(|r| r.filter_map(|e| e.ok()).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let (kind, at_ms, label) = stamp(&name)?;
            let bytes = e.metadata().map_or(0, |m| m.len());
            let read = std::fs::read_to_string(e.path()).ok().and_then(|t| m::file::from_json(&t).ok());
            let (clips, media, readable) = read.as_ref().map_or((0, 0, false), |f| (f.project.sequences.iter().map(|s| s.clips.len()).sum(), f.project.media.len(), true));
            Some(Snapshot { path: e.path(), at_ms, kind, label, bytes, clips, media, readable })
        })
        .collect();
    out.sort_by_key(|s| std::cmp::Reverse(s.at_ms));
    out
}

/// Writes the project as it is into `backups/` under a name the person
/// chose. Autosave never prunes these.
pub fn keep_snapshot(folder: &Path, project: &m::Project, label: &str) -> Result<PathBuf, String> {
    let pf = ProjectFolder::new(folder);
    std::fs::create_dir_all(pf.backups_dir()).map_err(|e| e.to_string())?;
    let label: String = label.chars().map(|c| if c.is_alphanumeric() { c } else { '_' }).collect::<String>().trim_matches('_').chars().take(40).collect();
    let mut ms = now_ms();
    let path = loop {
        let p = pf.backups_dir().join(format!("{NAMED}{ms:016}{}.json", if label.is_empty() { String::new() } else { format!("-{label}") }));
        if !p.exists() {
            break p;
        }
        ms += 1;
    };
    let doc = pf.document(project, &Default::default());
    m::file::write_atomic(&path, m::file::to_json(&doc).as_bytes()).map_err(|e| e.to_string())?;
    Ok(path)
}

/// A snapshot named as the person would: `latest`, a number from
/// [`history`] (0 is the newest), a file name, or a time prefix.
pub fn pick(folder: &Path, which: &Value) -> Result<Snapshot, String> {
    let all = history(folder);
    if all.is_empty() {
        return Err("this project has no snapshots yet (autosave writes one 30 seconds after a change)".into());
    }
    match which {
        Value::Number(n) => all.into_iter().nth(n.as_u64().unwrap_or(0) as usize).ok_or_else(|| "no snapshot with that number; see history".into()),
        Value::String(s) if s == "latest" || s.is_empty() => all.into_iter().find(|s| s.readable).ok_or_else(|| "no snapshot can be read".to_string()),
        Value::String(s) => {
            if let Ok(n) = s.parse::<usize>() {
                if s.len() <= 3 {
                    return all.into_iter().nth(n).ok_or_else(|| "no snapshot with that number; see history".into());
                }
            }
            let needle = s.to_lowercase();
            let slugged = needle.replace(' ', "_");
            all.into_iter()
                .find(|x| {
                    let name = x.path.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
                    name.contains(&needle) || name.contains(&slugged) || x.label.to_lowercase() == needle || when(x.at_ms).starts_with(s.as_str())
                })
                .ok_or_else(|| format!("no snapshot matches \"{s}\"; see history"))
        }
        _ => Err("say which snapshot: its number from history, \"latest\", or part of its file name or time".into()),
    }
}

/// Puts a snapshot back as the project file. The project as it is now is
/// first kept as a restore point, so a restore can itself be undone.
/// Returns the restore point.
pub fn restore(folder: &Path, snap: &Snapshot) -> Result<PathBuf, String> {
    if !snap.readable {
        return Err("that snapshot is damaged and cannot be read; pick another".into());
    }
    let pf = ProjectFolder::new(folder);
    let text = std::fs::read_to_string(&snap.path).map_err(|e| e.to_string())?;
    let file = m::file::from_json(&text).map_err(|e| e.to_string())?;
    let point = match std::fs::read_to_string(pf.project_file()).ok().and_then(|t| m::file::from_json(&t).ok()) {
        Some(cur) => {
            std::fs::create_dir_all(pf.backups_dir()).map_err(|e| e.to_string())?;
            let p = pf.backups_dir().join(format!("{RESTORE}{:016}.json", now_ms()));
            m::file::write_atomic(&p, m::file::to_json(&cur).as_bytes()).map_err(|e| e.to_string())?;
            Some(p)
        }
        None => None,
    };
    pf.save_with(&file.project, &file.extra).map_err(|e| e.to_string())?;
    Ok(point.unwrap_or_default())
}

// --------------------------------------------------------------- sessions

/// Where a window was.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Session {
    pub pid: u32,
    pub project: PathBuf,
    /// Timeline position, microseconds.
    pub playhead: i64,
    pub selection: Vec<u32>,
    pub sequence: u64,
    /// Anything else the window wants back (zoom, panel, scroll…).
    #[serde(default)]
    pub view: serde_json::Map<String, Value>,
    pub saved_ms: u64,
}

fn sessions_dir(root: &Path) -> PathBuf {
    root.join("Sessions")
}

#[cfg(windows)]
pub fn alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_ACCESS_DENIED, GetLastError, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    if pid == 0 {
        return false;
    }
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            // Someone else's process still exists.
            return GetLastError() == ERROR_ACCESS_DENIED;
        }
        let mut code = 0u32;
        let running = GetExitCodeProcess(h, &mut code) != 0 && code == STILL_ACTIVE as u32;
        CloseHandle(h);
        running
    }
}

#[cfg(unix)]
pub fn alive(pid: u32) -> bool {
    // Signal 0 only asks whether the process exists.
    pid != 0 && (unsafe { libc::kill(pid as i32, 0) } == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM))
}

/// Writes this window's state (cheap; call when it changes).
pub fn save_session(root: &Path, s: &Session) {
    let dir = sessions_dir(root);
    let _ = std::fs::create_dir_all(&dir);
    let mut s = s.clone();
    s.saved_ms = now_ms();
    if let Ok(b) = serde_json::to_vec(&s) {
        let _ = m::file::write_atomic(&dir.join(format!("{}.json", s.pid)), &b);
    }
}

/// A clean quit: nothing to recover.
pub fn end_session(root: &Path, pid: u32) {
    let _ = std::fs::remove_file(sessions_dir(root).join(format!("{pid}.json")));
}

/// Sessions whose window is gone without a clean quit, newest first.
pub fn crashed(root: &Path) -> Vec<Session> {
    let mut out: Vec<Session> = std::fs::read_dir(sessions_dir(root))
        .map(|r| r.filter_map(|e| e.ok()).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|e| serde_json::from_slice::<Session>(&std::fs::read(e.path()).ok()?).ok())
        .filter(|s| !alive(s.pid) && s.project.join(m::file::PROJECT_FILE).is_file())
        .collect();
    out.sort_by_key(|s| std::cmp::Reverse(s.saved_ms));
    out
}

/// Forgets a crashed session once it has been dealt with.
pub fn dismiss(root: &Path, pid: u32) {
    end_session(root, pid);
}

/// What the newest autosave holds that the project file does not (an
/// autosave newer than the saved file means work after the last save).
pub fn newer_autosave(folder: &Path) -> Option<Snapshot> {
    let pf = ProjectFolder::new(folder);
    let saved = std::fs::metadata(pf.project_file()).and_then(|m| m.modified()).ok()?;
    let saved_ms = saved.duration_since(std::time::UNIX_EPOCH).ok()?.as_millis() as u64;
    history(folder).into_iter().find(|s| s.kind == "autosave" && s.readable && s.at_ms > saved_ms + 1000)
}

// ------------------------------------------------------- referenced files

fn strings_of(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::String(s) => out.push(s.clone()),
        Value::Array(a) => a.iter().for_each(|x| strings_of(x, out)),
        Value::Object(o) => o.values().for_each(|x| strings_of(x, out)),
        _ => {}
    }
}

/// Every file or folder a project refers to: any string in its file that
/// names something on disk (absolutely, or relative to the project).
pub fn referenced(folder: &Path) -> HashSet<PathBuf> {
    let mut out = HashSet::new();
    let Ok(text) = std::fs::read_to_string(ProjectFolder::new(folder).project_file()) else { return out };
    let Ok(v) = serde_json::from_str::<Value>(&text) else { return out };
    let mut all = Vec::new();
    strings_of(&v, &mut all);
    let root = std::fs::canonicalize(folder).unwrap_or_else(|_| folder.to_path_buf());
    let skip: HashSet<PathBuf> = [root.clone(), root.join("cache"), root.join("backups"), root.join("media")].into_iter().collect();
    for s in all {
        if s.is_empty() || s.len() > 4096 || s.contains('\n') {
            continue;
        }
        let p = Path::new(&s);
        let cand = if p.is_absolute() { p.to_path_buf() } else { folder.join(p) };
        if cand.exists() {
            let c = std::fs::canonicalize(&cand).unwrap_or(cand);
            // The project's own folders are not "files it uses".
            if !skip.contains(&c) {
                out.insert(c);
            }
        }
    }
    out
}

fn is_protected(candidate: &Path, protected: &HashSet<PathBuf>) -> bool {
    let c = std::fs::canonicalize(candidate).unwrap_or_else(|_| candidate.to_path_buf());
    protected.iter().any(|p| *p == c || p.starts_with(&c) || (p.is_dir() && c.starts_with(p)))
}

// ----------------------------------------------------------------- collect

#[derive(Clone, Debug, Default)]
pub struct CollectOptions {
    /// Leave out media no clip uses.
    pub only_used: bool,
    /// Also copy the project's caches (thumbnails, waveforms, proxies).
    pub with_cache: bool,
    /// Write ARCHIVE.json with a SHA-256 of every file.
    pub checksums: bool,
}

#[derive(Debug, Default)]
pub struct Collected {
    pub dest: PathBuf,
    pub files: usize,
    pub bytes: u64,
    pub missing: Vec<String>,
    pub skipped_unused: Vec<String>,
}

impl Collected {
    pub fn json(&self) -> Value {
        json!({"folder": self.dest.display().to_string(), "files": self.files, "size": human(self.bytes), "missing": self.missing, "left_out_unused": self.skipped_unused})
    }
}

fn unique_name(taken: &mut HashSet<String>, name: &str) -> String {
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_owned(), format!(".{e}")),
        _ => (name.to_owned(), String::new()),
    };
    let mut candidate = name.to_owned();
    let mut n = 2;
    while !taken.insert(candidate.to_lowercase()) {
        candidate = format!("{stem}-{n}{ext}");
        n += 1;
    }
    candidate
}

fn copy_verified(src: &Path, dst: &Path) -> Result<u64, String> {
    if let Some(d) = dst.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    let tmp = dst.with_file_name(format!(".{}.partial", dst.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()));
    let n = std::fs::copy(src, &tmp).map_err(|e| format!("copying {}: {e}", src.display()))?;
    let want = std::fs::metadata(src).map_err(|e| e.to_string())?.len();
    if n != want || std::fs::metadata(&tmp).map_or(0, |m| m.len()) != want {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("the copy of {} came out the wrong size", src.display()));
    }
    std::fs::rename(&tmp, dst).map_err(|e| e.to_string())?;
    Ok(n)
}

/// Copies a project and every file it uses into `dest`, a folder that
/// stands alone: the project file there points at the copies. The original
/// is not touched. `dest` must be empty or new.
pub fn collect(folder: &Path, dest: &Path, opts: &CollectOptions) -> Result<Collected, String> {
    let pf = ProjectFolder::new(folder);
    let text = std::fs::read_to_string(pf.project_file()).map_err(|e| format!("{}: {e}", pf.project_file().display()))?;
    let file = m::file::from_json(&text).map_err(|e| e.to_string())?;
    let mut json: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    if dest.exists() && std::fs::read_dir(dest).map_err(|e| e.to_string())?.next().is_some() {
        return Err(format!("{} is not empty; collect into a new or empty folder", dest.display()));
    }
    let src_root = std::fs::canonicalize(folder).unwrap_or_else(|_| folder.to_path_buf());
    if let Ok(d) = std::fs::canonicalize(dest.parent().unwrap_or(dest)) {
        if d.starts_with(&src_root) {
            return Err("the destination is inside the project; choose a folder outside it".into());
        }
    }
    let used = crate::rights::used_media(&file.project);
    let mut skip: HashSet<String> = HashSet::new();
    let mut out = Collected { dest: dest.to_path_buf(), ..Default::default() };
    if opts.only_used {
        for i in file.project.media.iter().filter(|i| !used.contains(&i.id)) {
            skip.insert(i.path.clone());
            out.skipped_unused.push(i.name.clone());
        }
    }
    let media_paths: HashSet<&str> = file.project.media.iter().map(|i| i.path.as_str()).collect();
    // Every string that names a file on disk, and where its copy goes.
    let mut strings = Vec::new();
    strings_of(&json, &mut strings);
    let mut plan: BTreeMap<String, PathBuf> = BTreeMap::new();
    let mut taken_media = HashSet::new();
    let mut taken_assets = HashSet::new();
    for s in strings {
        let p = Path::new(&s);
        if !p.is_absolute() || plan.contains_key(&s) || skip.contains(&s) {
            continue;
        }
        let is_media = media_paths.contains(s.as_str());
        if !p.is_file() {
            if is_media {
                out.missing.push(s.clone());
            }
            continue;
        }
        let canon = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        let in_project = canon.strip_prefix(&src_root).ok().map(Path::to_path_buf);
        if let Some(rel) = &in_project {
            let top = rel.components().next().map(|c| c.as_os_str().to_string_lossy().into_owned()).unwrap_or_default();
            if top == "backups" || (top == "cache" && !opts.with_cache) || top == "project.json" {
                continue;
            }
        }
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let to = match in_project {
            Some(rel) => dest.join(rel),
            None if is_media => dest.join("media").join(unique_name(&mut taken_media, &name)),
            None => dest.join("assets").join(unique_name(&mut taken_assets, &name)),
        };
        plan.insert(s, to);
    }
    // Sidecars that belong to a media file (spoken words Studio kept).
    let mut sidecars: Vec<(PathBuf, PathBuf)> = Vec::new();
    for (from, to) in &plan {
        for ext in ["words.json"] {
            let (sf, st) = (Path::new(from).with_extension(ext), to.with_extension(ext));
            if sf.is_file() && !plan.contains_key(&sf.display().to_string()) {
                sidecars.push((sf, st));
            }
        }
    }
    for extra in ["agent-notes.md"] {
        let sf = folder.join(extra);
        if sf.is_file() {
            sidecars.push((sf, dest.join(extra)));
        }
    }
    let need: u64 = plan.keys().map(|p| std::fs::metadata(p).map_or(0, |m| m.len())).sum::<u64>() + sidecars.iter().map(|(s, _)| std::fs::metadata(s).map_or(0, |m| m.len())).sum::<u64>();
    if let Some(free) = free_space(dest) {
        if free < need + (need / 50).max(50 << 20) {
            return Err(format!("not enough room: the collection needs {} and {} is free there", human(need), human(free)));
        }
    }
    std::fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    let mut done: Vec<(PathBuf, u64)> = Vec::new();
    let result: Result<(), String> = (|| {
        for (from, to) in &plan {
            let n = copy_verified(Path::new(from), to)?;
            out.files += 1;
            out.bytes += n;
            done.push((to.clone(), n));
        }
        for (from, to) in &sidecars {
            let n = copy_verified(from, to)?;
            done.push((to.clone(), n));
        }
        Ok(())
    })();
    if let Err(e) = result {
        // A half collection is worse than none.
        let _ = std::fs::remove_dir_all(dest);
        return Err(e);
    }
    // The project file, pointing at the copies.
    let map: HashMap<&str, String> = plan.iter().map(|(k, v)| (k.as_str(), v.display().to_string())).collect();
    fn rewrite(v: &mut Value, map: &HashMap<&str, String>) {
        match v {
            Value::String(s) => {
                if let Some(n) = map.get(s.as_str()) {
                    *s = n.clone();
                }
            }
            Value::Array(a) => a.iter_mut().for_each(|x| rewrite(x, map)),
            Value::Object(o) => o.values_mut().for_each(|x| rewrite(x, map)),
            _ => {}
        }
    }
    rewrite(&mut json, &map);
    let moved = m::file::from_json(&json.to_string()).map_err(|e| e.to_string())?;
    let target = ProjectFolder::create(dest).map_err(|e| e.to_string())?;
    target.save_with(&moved.project, &moved.extra).map_err(|e| e.to_string())?;
    if opts.checksums {
        let mut files = Vec::new();
        for (to, n) in done.iter().chain(std::iter::once(&(target.project_file(), 0))) {
            let sum = studio_ai::sha256::sha256_file(to, |_| true).ok().flatten().map(|h| studio_ai::sha256::hex(&h)).unwrap_or_default();
            files.push(json!({"path": to.strip_prefix(dest).unwrap_or(to).display().to_string(), "bytes": std::fs::metadata(to).map_or(*n, |m| m.len()), "sha256": sum}));
        }
        let manifest = json!({"format": "studio-archive", "created": when(now_ms()), "project": file.project.sequences.first().map(|s| s.name.clone()), "root": dest.display().to_string(), "source": folder.display().to_string(), "files": files});
        std::fs::write(dest.join("ARCHIVE.json"), serde_json::to_vec_pretty(&manifest).unwrap_or_default()).map_err(|e| e.to_string())?;
    }
    Ok(out)
}

/// Checks an archive against its ARCHIVE.json: every file there and intact.
pub fn verify_archive(dir: &Path) -> Result<Value, String> {
    let text = std::fs::read_to_string(dir.join("ARCHIVE.json")).map_err(|_| format!("{} has no ARCHIVE.json; it was not made with archive", dir.display()))?;
    let v: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let (mut ok, mut bad, mut gone) = (0, Vec::new(), Vec::new());
    for f in v["files"].as_array().cloned().unwrap_or_default() {
        let rel = f["path"].as_str().unwrap_or("");
        let p = dir.join(rel);
        if !p.is_file() {
            gone.push(rel.to_owned());
            continue;
        }
        let sum = studio_ai::sha256::sha256_file(&p, |_| true).ok().flatten().map(|h| studio_ai::sha256::hex(&h)).unwrap_or_default();
        // The project file may have been moved and repathed since; its hash is allowed to differ.
        if sum == f["sha256"].as_str().unwrap_or("") || rel == "project.json" {
            ok += 1;
        } else {
            bad.push(rel.to_owned());
        }
    }
    Ok(json!({"intact": bad.is_empty() && gone.is_empty(), "files_ok": ok, "damaged": bad, "missing": gone, "created": v["created"]}))
}

/// After an archive or collection was moved to another place, points the
/// project at its files there.
pub fn relocate(dir: &Path) -> Result<usize, String> {
    let pf = ProjectFolder::new(dir);
    let text = std::fs::read_to_string(pf.project_file()).map_err(|e| e.to_string())?;
    let mut v: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let old = std::fs::read_to_string(dir.join("ARCHIVE.json")).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).and_then(|a| a["root"].as_str().map(str::to_owned));
    let Some(old) = old else { return Ok(0) };
    let new = dir.display().to_string();
    if old == new {
        return Ok(0);
    }
    fn go(v: &mut Value, old: &str, new: &str, n: &mut usize) {
        match v {
            // `\` too: Windows paths.
            Value::String(s) if s.starts_with(old) && (s.len() == old.len() || s[old.len()..].starts_with(['/', '\\'])) => {
                *s = format!("{new}{}", &s[old.len()..]);
                *n += 1;
            }
            Value::Array(a) => a.iter_mut().for_each(|x| go(x, old, new, n)),
            Value::Object(o) => o.values_mut().for_each(|x| go(x, old, new, n)),
            _ => {}
        }
    }
    let mut n = 0;
    go(&mut v, &old, &new, &mut n);
    if n > 0 {
        let f = m::file::from_json(&v.to_string()).map_err(|e| e.to_string())?;
        pf.save_with(&f.project, &f.extra).map_err(|e| e.to_string())?;
        // So a later move is also found.
        if let Ok(t) = std::fs::read_to_string(dir.join("ARCHIVE.json")) {
            if let Ok(mut a) = serde_json::from_str::<Value>(&t) {
                a["root"] = json!(new);
                let _ = std::fs::write(dir.join("ARCHIVE.json"), serde_json::to_vec_pretty(&a).unwrap_or_default());
            }
        }
    }
    Ok(n)
}

// ------------------------------------------------------------------ backup

#[derive(Debug, Default)]
pub struct BackedUp {
    pub dir: PathBuf,
    pub snapshot: String,
    pub copied: usize,
    pub reused: usize,
    pub bytes: u64,
    pub missing: Vec<String>,
    pub pruned: usize,
}

impl BackedUp {
    pub fn json(&self) -> Value {
        json!({"backup_folder": self.dir.display().to_string(), "snapshot": self.snapshot, "media_copied": self.copied, "media_already_there": self.reused, "copied": human(self.bytes), "missing_media": self.missing, "old_snapshots_removed": self.pruned})
    }
}

fn slug(name: &str) -> String {
    let s: String = name.chars().map(|c| if c.is_alphanumeric() || c == '-' { c } else { '_' }).collect();
    let s = s.trim_matches('_');
    if s.is_empty() { "project".into() } else { s.chars().take(60).collect() }
}

fn folder_key(folder: &Path) -> String {
    let h = std::fs::canonicalize(folder).unwrap_or_else(|_| folder.to_path_buf()).to_string_lossy().bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x0100_0000_01b3));
    format!("{h:08x}")[..8].to_owned()
}

/// The folder a project's backups go to inside `dest`.
pub fn backup_dir(dest: &Path, folder: &Path) -> PathBuf {
    let name = folder.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    dest.join(format!("{}-{}", slug(&name), folder_key(folder)))
}

/// Backs the project up into `dest` (a folder, a drive): the project file,
/// notes and history as a dated snapshot, and each media file once, named
/// by its content, so a second backup copies only what changed. Keeps the
/// newest `keep` snapshots and removes media no snapshot needs any more.
pub fn backup(folder: &Path, dest: &Path, keep: usize, with_media: bool) -> Result<BackedUp, String> {
    let pf = ProjectFolder::new(folder);
    let text = std::fs::read_to_string(pf.project_file()).map_err(|e| format!("{}: {e}", pf.project_file().display()))?;
    let file = m::file::from_json(&text).map_err(|e| format!("the project cannot be read, so it was not backed up: {e}"))?;
    let root = backup_dir(dest, folder);
    let store = root.join("media");
    let mut out = BackedUp { dir: root.clone(), ..Default::default() };
    let name = |ms: u64| when(ms).replace([':', '-'], "").replace('T', "-").replace('Z', "");
    let mut ms = now_ms();
    let snap = loop {
        let s = root.join("snapshots").join(name(ms));
        if !s.exists() {
            break s;
        }
        ms += 1000;
    };
    // Media first: a snapshot is only written when its media is safe.
    let mut manifest = Vec::new();
    if with_media {
        let need: u64 = file.project.media.iter().filter_map(|i| std::fs::metadata(&i.path).ok()).map(|m| m.len()).sum();
        if let Some(free) = free_space(dest) {
            // Roughly: what is there already is not copied again, so this is the worst case.
            let present: u64 = dir_size(&store);
            if free + present < need + (50 << 20) {
                return Err(format!("not enough room in {}: the media needs up to {} and {} is free", dest.display(), human(need), human(free)));
            }
        }
    }
    for item in &file.project.media {
        let p = Path::new(&item.path);
        if !p.is_file() {
            out.missing.push(item.name.clone());
            manifest.push(json!({"id": item.id.0, "name": item.name, "path": item.path, "store": Value::Null}));
            continue;
        }
        if !with_media {
            manifest.push(json!({"id": item.id.0, "name": item.name, "path": item.path, "store": Value::Null}));
            continue;
        }
        let fp = asset_meta::fingerprint(p).ok_or_else(|| format!("cannot read {}", p.display()))?;
        let stored = store.join(format!("{fp}-{}", item.name.replace('/', "_")));
        let size = std::fs::metadata(p).map_err(|e| e.to_string())?.len();
        if std::fs::metadata(&stored).is_ok_and(|m| m.len() == size) {
            out.reused += 1;
        } else {
            out.bytes += copy_verified(p, &stored)?;
            out.copied += 1;
        }
        manifest.push(json!({"id": item.id.0, "name": item.name, "path": item.path, "store": stored.file_name().map(|n| n.to_string_lossy().into_owned())}));
    }
    std::fs::create_dir_all(&snap).map_err(|e| e.to_string())?;
    m::file::write_atomic(&snap.join(m::file::PROJECT_FILE), text.as_bytes()).map_err(|e| e.to_string())?;
    for extra in ["agent-notes.md"] {
        if folder.join(extra).is_file() {
            let _ = std::fs::copy(folder.join(extra), snap.join(extra));
        }
    }
    let man = json!({"project": folder.display().to_string(), "created": when(now_ms()), "media": manifest, "with_media": with_media});
    m::file::write_atomic(&snap.join("manifest.json"), serde_json::to_vec_pretty(&man).unwrap_or_default().as_slice()).map_err(|e| e.to_string())?;
    out.snapshot = snap.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    // Prune: the newest `keep` snapshots stay; media they do not name goes.
    let mut snaps: Vec<PathBuf> = std::fs::read_dir(root.join("snapshots")).map(|r| r.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.join("manifest.json").is_file()).collect()).unwrap_or_default();
    snaps.sort();
    snaps.reverse();
    for old in snaps.iter().skip(keep.max(1)) {
        if std::fs::remove_dir_all(old).is_ok() {
            out.pruned += 1;
        }
    }
    if out.pruned > 0 && with_media {
        let mut live: HashSet<String> = HashSet::new();
        for s in snaps.iter().take(keep.max(1)) {
            if let Some(v) = std::fs::read_to_string(s.join("manifest.json")).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()) {
                live.extend(v["media"].as_array().cloned().unwrap_or_default().iter().filter_map(|m| m["store"].as_str().map(str::to_owned)));
            }
        }
        for e in std::fs::read_dir(&store).map(|r| r.filter_map(|e| e.ok()).collect::<Vec<_>>()).unwrap_or_default() {
            if !live.contains(&e.file_name().to_string_lossy().into_owned()) && !e.file_name().to_string_lossy().starts_with('.') {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    // When, for the health check.
    let _ = std::fs::write(folder.join(".last-backup.json"), json!({"at_ms": now_ms(), "to": dest.display().to_string(), "snapshot": out.snapshot}).to_string());
    Ok(out)
}

/// The snapshots in a backup folder (newest first): (name, created, media files).
pub fn backups_in(dest: &Path, folder: &Path) -> Vec<Value> {
    let root = backup_dir(dest, folder);
    let mut snaps: Vec<PathBuf> = std::fs::read_dir(root.join("snapshots")).map(|r| r.filter_map(|e| e.ok()).map(|e| e.path()).collect()).unwrap_or_default();
    snaps.sort();
    snaps.reverse();
    snaps
        .into_iter()
        .filter_map(|s| {
            let v: Value = serde_json::from_str(&std::fs::read_to_string(s.join("manifest.json")).ok()?).ok()?;
            Some(json!({"snapshot": s.file_name()?.to_string_lossy(), "created": v["created"], "media": v["media"].as_array().map_or(0, Vec::len), "with_media": v["with_media"]}))
        })
        .collect()
}

/// Rebuilds a project from a backup snapshot into a new folder: the project
/// file and its media (from the backup's store) copied beside it.
pub fn restore_backup(dest: &Path, folder: &Path, snapshot: &str, into: &Path) -> Result<Value, String> {
    let root = backup_dir(dest, folder);
    let snap = if snapshot.is_empty() || snapshot == "latest" {
        let mut all: Vec<PathBuf> = std::fs::read_dir(root.join("snapshots")).map(|r| r.filter_map(|e| e.ok()).map(|e| e.path()).collect()).unwrap_or_default();
        all.sort();
        all.pop().ok_or_else(|| format!("no backup of this project in {}", dest.display()))?
    } else {
        root.join("snapshots").join(snapshot)
    };
    let man: Value = serde_json::from_str(&std::fs::read_to_string(snap.join("manifest.json")).map_err(|_| format!("no snapshot {} in {}", snapshot, root.display()))?).map_err(|e| e.to_string())?;
    if into.exists() && std::fs::read_dir(into).map_err(|e| e.to_string())?.next().is_some() {
        return Err(format!("{} is not empty; restore into a new folder", into.display()));
    }
    let mut json: Value = serde_json::from_str(&std::fs::read_to_string(snap.join(m::file::PROJECT_FILE)).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    let mut map: HashMap<String, String> = HashMap::new();
    let mut taken = HashSet::new();
    let mut restored = 0;
    for it in man["media"].as_array().cloned().unwrap_or_default() {
        let (Some(store), Some(orig)) = (it["store"].as_str(), it["path"].as_str()) else { continue };
        let src = root.join("media").join(store);
        let name = unique_name(&mut taken, it["name"].as_str().unwrap_or(store));
        let to = into.join("media").join(name);
        copy_verified(&src, &to)?;
        map.insert(orig.to_owned(), to.display().to_string());
        restored += 1;
    }
    fn go(v: &mut Value, map: &HashMap<String, String>) {
        match v {
            Value::String(s) => {
                if let Some(n) = map.get(s.as_str()) {
                    *s = n.clone();
                }
            }
            Value::Array(a) => a.iter_mut().for_each(|x| go(x, map)),
            Value::Object(o) => o.values_mut().for_each(|x| go(x, map)),
            _ => {}
        }
    }
    go(&mut json, &map);
    let f = m::file::from_json(&json.to_string()).map_err(|e| e.to_string())?;
    let target = ProjectFolder::create(into).map_err(|e| e.to_string())?;
    target.save_with(&f.project, &f.extra).map_err(|e| e.to_string())?;
    for extra in ["agent-notes.md"] {
        if snap.join(extra).is_file() {
            let _ = std::fs::copy(snap.join(extra), into.join(extra));
        }
    }
    Ok(json!({"restored_to": into.display().to_string(), "media_restored": restored, "from": snap.file_name().map(|n| n.to_string_lossy().into_owned())}))
}

// ------------------------------------------------------------------ health

pub struct Issue {
    pub level: &'static str,
    pub code: &'static str,
    pub message: String,
    pub fix: String,
}

/// A look at the project's safety and size.
pub fn health(folder: &Path, backup_to: Option<&Path>) -> Value {
    let pf = ProjectFolder::new(folder);
    let mut issues: Vec<Issue> = Vec::new();
    let mut add = |level, code, message: String, fix: &str| issues.push(Issue { level, code, message, fix: fix.into() });
    let loaded = std::fs::read_to_string(pf.project_file()).map_err(|e| e.to_string()).and_then(|t| m::file::from_json(&t).map_err(|e| e.to_string()));
    let (mut media_n, mut missing, mut external, mut unused) = (0usize, Vec::<String>::new(), Vec::<String>::new(), Vec::<String>::new());
    let mut dupes = 0usize;
    let mut media_bytes = 0u64;
    match &loaded {
        Err(e) => add("critical", "unreadable", format!("the project file cannot be read: {e}"), "restore the newest good snapshot: project_care action \"restore\""),
        Ok(f) => {
            let used = crate::rights::used_media(&f.project);
            let mut fps: HashMap<String, usize> = HashMap::new();
            for i in &f.project.media {
                media_n += 1;
                let p = Path::new(&i.path);
                if !p.is_file() {
                    missing.push(i.name.clone());
                    continue;
                }
                media_bytes += std::fs::metadata(p).map_or(0, |m| m.len());
                if p.starts_with("/Volumes") || !(p.starts_with(folder) || p.starts_with(studio_paths::home())) {
                    external.push(i.name.clone());
                }
                if !used.contains(&i.id) {
                    unused.push(i.name.clone());
                }
                if let Some(fp) = asset_meta::fingerprint(p) {
                    *fps.entry(fp).or_default() += 1;
                }
            }
            dupes = fps.values().filter(|n| **n > 1).map(|n| n - 1).sum();
        }
    }
    if !missing.is_empty() {
        add("critical", "missing-media", format!("{} media file{} cannot be found: {}", missing.len(), if missing.len() == 1 { "" } else { "s" }, missing.iter().take(6).cloned().collect::<Vec<_>>().join(", ")), "relink them: assets action \"relink\" (searches this Mac and connected drives)");
    }
    if !external.is_empty() {
        add("warning", "media-outside", format!("{} file{} live outside the project or on another drive, so they go offline if it is unplugged or the folder moves", external.len(), if external.len() == 1 { "" } else { "s" }), "collect the project: project_care action \"collect\"");
    }
    let snaps = history(folder);
    let autos = snaps.iter().filter(|s| s.kind == "autosave").count();
    let damaged = snaps.iter().filter(|s| !s.readable).count();
    match snaps.first() {
        None => add("warning", "no-snapshots", "no autosave snapshots yet".into(), "make sure Autosave is on in Settings; project_care action \"snapshot\" makes one now"),
        Some(s) if now_ms().saturating_sub(s.at_ms) > 7 * 86_400_000 => add("info", "old-snapshots", format!("the newest snapshot is from {}", when(s.at_ms)), "snapshots are written after changes; if you edited since, autosave may be off"),
        _ => {}
    }
    if damaged > 0 {
        add("warning", "damaged-snapshots", format!("{damaged} snapshot{} cannot be read", if damaged == 1 { "" } else { "s" }), "harmless; clean removes old ones");
    }
    let last_backup = std::fs::read_to_string(folder.join(".last-backup.json")).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok());
    match (&last_backup, backup_to) {
        (None, _) => add("warning", "never-backed-up", "this project has never been backed up to another folder or drive".into(), "project_care action \"backup\" with to: a folder on another drive"),
        (Some(b), _) if now_ms().saturating_sub(b["at_ms"].as_u64().unwrap_or(0)) > 7 * 86_400_000 => add("info", "backup-old", format!("the last backup was {}", when(b["at_ms"].as_u64().unwrap_or(0))), "back up again"),
        _ => {}
    }
    let (cache, backups) = (dir_size(&pf.cache_dir()), dir_size(&pf.backups_dir()));
    let total = dir_size(folder);
    if cache > (2 << 30) || (cache > (200 << 20) && cache > total / 2) {
        add("info", "big-cache", format!("caches take {}", human(cache)), "project_care action \"clean\" frees them (they are rebuilt when needed)");
    }
    if backups > (3 << 30) {
        add("info", "big-history", format!("autosave history takes {}", human(backups)), "project_care action \"clean\" keeps the newest and removes the rest");
    }
    if let Some(free) = free_space(folder) {
        if free < (5 << 30) {
            add("warning", "low-disk", format!("only {} free on this drive", human(free)), "free space or move the project; saving can fail on a full drive");
        }
    }
    if dupes > 0 {
        add("info", "duplicates", if dupes == 1 { "1 media file is a copy of another in this project".to_owned() } else { format!("{dupes} media files are copies of others in this project") }, "assets action \"duplicates\"");
    }
    let level = if issues.iter().any(|i| i.level == "critical") { "at risk" } else if issues.iter().any(|i| i.level == "warning") { "needs attention" } else { "healthy" };
    json!({
        "status": level,
        "issues": issues.iter().map(|i| json!({"level": i.level, "code": i.code, "message": i.message, "fix": i.fix})).collect::<Vec<_>>(),
        "media": {"files": media_n, "missing": missing.len(), "outside_project": external.len(), "unused": unused.len(), "size": human(media_bytes)},
        "snapshots": {"autosaves": autos, "total": snaps.len(), "newest": snaps.first().map(|s| when(s.at_ms))},
        "size": {"project_folder": human(total), "cache": human(cache), "history": human(backups)},
        "last_backup": last_backup,
        "disk_free": free_space(folder).map(human),
    })
}

// ----------------------------------------------------------------- cleanup

#[derive(Clone, Debug)]
pub struct Doomed {
    pub path: PathBuf,
    pub bytes: u64,
    pub why: &'static str,
}

#[derive(Default)]
pub struct Plan {
    pub items: Vec<Doomed>,
    /// Things that looked removable but a project needs.
    pub kept_in_use: usize,
}

impl Plan {
    pub fn bytes(&self) -> u64 {
        self.items.iter().map(|d| d.bytes).sum()
    }

    pub fn json(&self, limit: usize) -> Value {
        let mut by_why: BTreeMap<&str, (usize, u64)> = BTreeMap::new();
        for d in &self.items {
            let e = by_why.entry(d.why).or_default();
            e.0 += 1;
            e.1 += d.bytes;
        }
        json!({
            "would_free": human(self.bytes()),
            "files": self.items.len(),
            "by_reason": by_why.iter().map(|(k, (n, b))| json!({"reason": k, "files": n, "size": human(*b)})).collect::<Vec<_>>(),
            "kept_because_a_project_uses_them": self.kept_in_use,
            "examples": self.items.iter().take(limit).map(|d| json!({"path": d.path.display().to_string(), "size": human(d.bytes), "reason": d.why})).collect::<Vec<_>>(),
        })
    }
}

#[derive(Clone, Debug)]
pub struct CleanOptions {
    /// Project caches (thumbnails, waveforms, proxies) nothing refers to.
    pub project_cache: bool,
    /// Autosaves beyond the newest N (restore points and kept snapshots stay).
    pub keep_autosaves: Option<usize>,
    /// Files in Studio's shared cache untouched for this many days.
    pub shared_cache_days: Option<u64>,
    /// Every project folder whose files count as "in use" (the folder itself is always included).
    pub other_projects: Vec<PathBuf>,
    pub shared_cache: PathBuf,
}

impl Default for CleanOptions {
    fn default() -> Self {
        CleanOptions { project_cache: true, keep_autosaves: Some(10), shared_cache_days: Some(30), other_projects: Vec::new(), shared_cache: studio_paths::cache() }
    }
}

fn walk_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.filter_map(|e| e.ok()) {
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_symlink() {
            continue;
        }
        if ft.is_dir() {
            walk_files(&e.path(), out);
        } else {
            out.push(e.path());
        }
    }
}

/// What cleaning would remove. Nothing is removed here.
pub fn clean_plan(folder: &Path, o: &CleanOptions) -> Plan {
    let pf = ProjectFolder::new(folder);
    let mut protected = referenced(folder);
    for p in &o.other_projects {
        protected.extend(referenced(p));
    }
    let mut plan = Plan::default();
    let consider = |plan: &mut Plan, path: PathBuf, why: &'static str| {
        if is_protected(&path, &protected) {
            plan.kept_in_use += 1;
        } else {
            let bytes = std::fs::metadata(&path).map_or(0, |m| m.len());
            plan.items.push(Doomed { path, bytes, why });
        }
    };
    if o.project_cache {
        let mut files = Vec::new();
        walk_files(&pf.cache_dir(), &mut files);
        for f in files {
            // The undo history lives in the cache folder but is the person's work, not a cache.
            if f.file_name().is_some_and(|n| n.to_string_lossy().starts_with("history.")) {
                continue;
            }
            consider(&mut plan, f, "project cache (rebuilt when needed)");
        }
    }
    if let Some(keep) = o.keep_autosaves {
        // Only autosaves; the newest `keep` readable ones stay, whatever their age.
        let autos: Vec<Snapshot> = history(folder).into_iter().filter(|s| s.kind == "autosave").collect();
        let mut kept = 0;
        for s in autos {
            if s.readable && kept < keep.max(3) {
                kept += 1;
            } else {
                consider(&mut plan, s.path, "old autosave");
            }
        }
    }
    if let Some(days) = o.shared_cache_days {
        let cutoff = std::time::SystemTime::now() - std::time::Duration::from_secs(days * 86_400);
        let mut files = Vec::new();
        walk_files(&o.shared_cache, &mut files);
        for f in files {
            let name = f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            // Databases and notes about where things are live beside the caches but are not caches.
            if name.contains(".db") || name.ends_with(".txt") || name.ends_with(".sqlite") || name.ends_with("-wal") || name.ends_with("-shm") {
                continue;
            }
            let used = std::fs::metadata(&f).ok().and_then(|m| m.accessed().ok().into_iter().chain(m.modified().ok()).max());
            if used.is_some_and(|t| t < cutoff) {
                consider(&mut plan, f, "shared cache, not used for a long time");
            }
        }
    }
    plan
}

/// Removes what [`clean_plan`] found, re-checking each file first. Returns bytes freed.
pub fn clean_apply(folder: &Path, o: &CleanOptions, plan: &Plan) -> Result<u64, String> {
    let mut protected = referenced(folder);
    for p in &o.other_projects {
        protected.extend(referenced(p));
    }
    let allowed = [ProjectFolder::new(folder).cache_dir(), ProjectFolder::new(folder).backups_dir(), o.shared_cache.clone()];
    let mut freed = 0;
    for d in &plan.items {
        let canon = std::fs::canonicalize(&d.path).unwrap_or_else(|_| d.path.clone());
        let inside = allowed.iter().any(|a| std::fs::canonicalize(a).map_or(false, |a| canon.starts_with(a)));
        let is_link = std::fs::symlink_metadata(&d.path).map_or(true, |m| m.file_type().is_symlink());
        if !inside || is_link || is_protected(&d.path, &protected) {
            continue;
        }
        if std::fs::remove_file(&d.path).is_ok() {
            freed += d.bytes;
        }
    }
    // Empty folders left behind in the caches.
    for a in [ProjectFolder::new(folder).cache_dir(), o.shared_cache.clone()] {
        prune_empty(&a, false);
    }
    Ok(freed)
}

fn prune_empty(dir: &Path, remove_self: bool) {
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.filter_map(|e| e.ok()) {
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                prune_empty(&e.path(), true);
            }
        }
    }
    if remove_self {
        let _ = std::fs::remove_dir(dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("studio-safety-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn bytes(seed: u32, n: usize) -> Vec<u8> {
        (0..n as u32).map(|i| i.wrapping_mul(seed | 1).wrapping_add(i >> 5) as u8).collect()
    }

    /// A saved project whose media items point at real files.
    fn project_with_media(dir: &Path, files: &[(&str, Vec<u8>)]) -> (PathBuf, m::Project) {
        let root = dir.join("proj");
        let pf = ProjectFolder::create(&root).unwrap();
        let mut p = m::Project::new(m::ProjectSettings::default());
        for (k, (name, data)) in files.iter().enumerate() {
            let path = dir.join("shoot").join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, data).unwrap();
            p.media.push(media_item(k as u64 + 1, name, &path));
        }
        p.next_id = 100;
        pf.save(&p).unwrap();
        (root, p)
    }

    fn media_item(id: u64, name: &str, path: &Path) -> m::MediaItem {
        let mut v = serde_json::json!({"id": id, "name": name, "path": path.display().to_string(), "info": {"kind": "video"}, "imported_at": 0});
        v["info"]["container"] = "mp4".into();
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn history_and_restore_keep_the_current_project() {
        let d = scratch("hist");
        let (root, mut p) = project_with_media(&d, &[("a.mp4", bytes(3, 4000))]);
        let pf = ProjectFolder::new(&root);
        let before = autosave::write_backup(&pf, &p, std::time::SystemTime::now(), 20).unwrap();
        assert!(before.exists());
        p.media.clear();
        pf.save(&p).unwrap();
        let k = keep_snapshot(&root, &p, "empty cut").unwrap();
        assert!(k.file_name().unwrap().to_string_lossy().contains("empty_cut"));
        let h = history(&root);
        assert_eq!(h.len(), 2);
        assert!(h.iter().any(|s| s.kind == "kept" && s.label == "empty cut"));
        let auto = pick(&root, &json!("autosave")).unwrap();
        assert_eq!((auto.kind, auto.media), ("autosave", 1));
        let point = restore(&root, &auto).unwrap();
        assert!(point.exists(), "the current project is kept first");
        assert_eq!(pf.load().unwrap().project.media.len(), 1);
        // The restore itself can be undone from the restore point.
        let rp = pick(&root, &json!("before-restore")).unwrap();
        restore(&root, &rp).unwrap();
        assert_eq!(pf.load().unwrap().project.media.len(), 0);
        // A damaged snapshot is refused, not applied.
        std::fs::write(root.join("backups/autosave-0000000000000009.json"), b"{nope").unwrap();
        let bad = pick(&root, &json!("0000000000000009")).unwrap();
        assert!(restore(&root, &bad).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn crashed_sessions_are_found_and_clean_ones_are_not() {
        let d = scratch("sess");
        let (root, _) = project_with_media(&d, &[]);
        let home = d.join("studio-home");
        // A window that is still running is not a crash.
        save_session(&home, &Session { pid: std::process::id(), project: root.clone(), playhead: 5_000_000, selection: vec![3], ..Default::default() });
        assert!(crashed(&home).is_empty());
        // One whose process is gone is; it remembers where the person was.
        let dead = {
            let mut c = std::process::Command::new("true").spawn().unwrap();
            c.wait().unwrap();
            c.id()
        };
        save_session(&home, &Session { pid: dead, project: root.clone(), playhead: 7_250_000, selection: vec![1, 2], sequence: 4, ..Default::default() });
        let found = crashed(&home);
        assert_eq!(found.len(), 1);
        assert_eq!((found[0].playhead, found[0].selection.clone(), found[0].sequence), (7_250_000, vec![1, 2], 4));
        dismiss(&home, dead);
        assert!(crashed(&home).is_empty());
        end_session(&home, std::process::id());
        assert!(!sessions_dir(&home).join(format!("{}.json", std::process::id())).exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn collect_makes_a_standalone_copy_and_leaves_the_original() {
        let d = scratch("collect");
        let (root, _) = project_with_media(&d, &[("a.mp4", bytes(5, 70_000)), ("b.mp4", bytes(7, 30_000)), ("clash/a.mp4", bytes(9, 100))]);
        std::fs::write(d.join("shoot/a.words.json"), b"[]").unwrap();
        let before = std::fs::read(root.join("project.json")).unwrap();
        let dest = d.join("collected");
        let c = collect(&root, &dest, &CollectOptions { checksums: true, ..Default::default() }).unwrap();
        assert_eq!(c.files, 3);
        assert!(c.missing.is_empty());
        assert_eq!(before, std::fs::read(root.join("project.json")).unwrap(), "the original project file is untouched");
        let f = ProjectFolder::new(&dest).load().unwrap();
        assert!(f.project.media.iter().all(|i| Path::new(&i.path).starts_with(&dest) && Path::new(&i.path).is_file()));
        let names: HashSet<_> = f.project.media.iter().map(|i| i.path.clone()).collect();
        assert_eq!(names.len(), 3, "same-named files do not overwrite each other");
        assert!(dest.join("media/a.words.json").is_file(), "spoken words travel with the media");
        assert!(verify_archive(&dest).unwrap()["intact"].as_bool().unwrap());
        // Damage one file: the archive says so.
        let victim = f.project.media[0].path.clone();
        let mut b = std::fs::read(&victim).unwrap();
        b[10] ^= 1;
        std::fs::write(&victim, b).unwrap();
        assert!(!verify_archive(&dest).unwrap()["intact"].as_bool().unwrap());
        // Moved elsewhere, it relocates.
        let moved = d.join("moved");
        std::fs::rename(&dest, &moved).unwrap();
        assert!(relocate(&moved).unwrap() >= 3);
        assert!(ProjectFolder::new(&moved).load().unwrap().project.media.iter().all(|i| Path::new(&i.path).starts_with(&moved)));
        // Refuses a non-empty destination.
        assert!(collect(&root, &moved, &CollectOptions::default()).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn backup_is_incremental_verified_and_restorable() {
        let d = scratch("backup");
        let (root, mut p) = project_with_media(&d, &[("a.mp4", bytes(5, 70_000)), ("b.mp4", bytes(7, 30_000))]);
        let drive = d.join("drive");
        let one = backup(&root, &drive, 2, true).unwrap();
        assert_eq!((one.copied, one.reused), (2, 0));
        let two = backup(&root, &drive, 2, true).unwrap();
        assert_eq!((two.copied, two.reused), (0, 2), "unchanged media is not copied again");
        assert_ne!(one.snapshot, two.snapshot);
        // Lose the originals; the backup brings the project back whole.
        std::fs::remove_dir_all(d.join("shoot")).unwrap();
        let back = d.join("restored");
        let r = restore_backup(&drive, &root, "latest", &back).unwrap();
        assert_eq!(r["media_restored"], 2);
        let f = ProjectFolder::new(&back).load().unwrap();
        assert!(f.project.media.iter().all(|i| Path::new(&i.path).is_file() && Path::new(&i.path).starts_with(&back)));
        // Keeping 2: a third backup prunes the oldest snapshot, and media nobody needs goes with it.
        p.media.remove(0);
        ProjectFolder::new(&root).save(&p).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let three = backup(&root, &drive, 2, true).unwrap();
        assert_eq!(three.pruned, 1);
        assert_eq!(backups_in(&drive, &root).len(), 2);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn health_reports_what_matters() {
        let d = scratch("health");
        let (root, mut p) = project_with_media(&d, &[("a.mp4", bytes(5, 3000)), ("b.mp4", bytes(5, 3000))]);
        std::fs::remove_file(d.join("shoot/b.mp4")).unwrap();
        let h = health(&root, None);
        let codes: Vec<String> = h["issues"].as_array().unwrap().iter().map(|i| i["code"].as_str().unwrap().to_owned()).collect();
        assert!(codes.contains(&"missing-media".to_owned()), "{codes:?}");
        assert!(codes.contains(&"never-backed-up".to_owned()));
        assert_eq!(h["status"], "at risk");
        p.media.pop();
        ProjectFolder::new(&root).save(&p).unwrap();
        autosave::write_backup(&ProjectFolder::new(&root), &p, std::time::SystemTime::now(), 20).unwrap();
        backup(&root, &d.join("drive"), 3, false).unwrap();
        let h = health(&root, None);
        let codes: Vec<&str> = h["issues"].as_array().unwrap().iter().map(|i| i["code"].as_str().unwrap()).collect();
        // The shoot folder is outside the project; it counts as outside only
        // when it is outside home too, and Windows keeps temp files in home.
        let outside: Vec<&str> = if d.starts_with(studio_paths::home()) { vec![] } else { vec!["media-outside"] };
        assert_eq!(codes, outside, "everything else is fine: {h}");
        assert_eq!(h["status"], if outside.is_empty() { "healthy" } else { "needs attention" });
        // A broken project file is critical, and says how to get out.
        std::fs::write(root.join("project.json"), b"{").unwrap();
        assert_eq!(health(&root, None)["status"], "at risk");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn cleanup_never_removes_what_a_project_uses() {
        let d = scratch("clean");
        let (root, mut p) = project_with_media(&d, &[("a.mp4", bytes(5, 3000))]);
        let pf = ProjectFolder::new(&root);
        // A proxy the project points at, a thumbnail it does not, and old autosaves.
        let proxies = pf.cache_dir().join("proxies");
        std::fs::create_dir_all(&proxies).unwrap();
        std::fs::write(proxies.join("a-proxy.mp4"), bytes(1, 5000)).unwrap();
        std::fs::write(pf.cache_dir().join("stale-thumb.png"), bytes(2, 4000)).unwrap();
        std::fs::write(pf.cache_dir().join("history.jsonl"), b"{}").unwrap();
        p.media[0].proxy_path = Some("cache/proxies/a-proxy.mp4".into());
        pf.save(&p).unwrap();
        for i in 0..8u64 {
            let name = format!("autosave-{:016}.json", 1000 + i);
            std::fs::write(pf.backups_dir().join(name), m::file::to_json(&pf.document(&p, &Default::default()))).unwrap();
        }
        let named = keep_snapshot(&root, &p, "milestone").unwrap();
        let shared = d.join("shared-cache");
        std::fs::create_dir_all(&shared).unwrap();
        std::fs::write(shared.join("library.db"), b"db").unwrap();
        std::fs::write(shared.join("old.bin"), bytes(3, 1000)).unwrap();
        std::fs::write(shared.join("new.bin"), bytes(4, 1000)).unwrap();
        // A shared-cache file a project names stays even when it is old.
        std::fs::write(shared.join("used.bin"), bytes(6, 1000)).unwrap();
        let mut j: Value = serde_json::from_str(&std::fs::read_to_string(root.join("project.json")).unwrap()).unwrap();
        j["project"]["extra_note"] = json!(shared.join("used.bin").display().to_string());
        std::fs::write(root.join("project.json"), j.to_string()).unwrap();
        let long_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(90 * 86_400);
        for n in ["old.bin", "used.bin"] {
            let f = std::fs::File::options().write(true).open(shared.join(n)).unwrap();
            f.set_times(std::fs::FileTimes::new().set_accessed(long_ago).set_modified(long_ago)).unwrap();
        }
        let o = CleanOptions { keep_autosaves: Some(3), shared_cache: shared.clone(), ..Default::default() };
        let plan = clean_plan(&root, &o);
        let gone: HashSet<String> = plan.items.iter().map(|d| d.path.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert!(gone.contains("stale-thumb.png") && gone.contains("old.bin"));
        assert!(!gone.contains("a-proxy.mp4"), "a proxy the project points at stays");
        assert!(!gone.contains("used.bin") && !gone.contains("new.bin") && !gone.contains("library.db"));
        assert_eq!(gone.iter().filter(|n| n.starts_with("autosave-")).count(), 5, "autosaves beyond the newest 3");
        assert!(plan.kept_in_use >= 2);
        // A dry run changes nothing.
        assert!(root.join("cache/stale-thumb.png").exists());
        let freed = clean_apply(&root, &o, &plan).unwrap();
        assert!(freed > 0);
        assert!(!root.join("cache/stale-thumb.png").exists() && !shared.join("old.bin").exists());
        assert!(proxies.join("a-proxy.mp4").exists() && shared.join("used.bin").exists() && named.exists());
        assert!(root.join("cache/history.jsonl").exists(), "the undo history is not a cache");
        assert!(d.join("shoot/a.mp4").exists(), "media is never touched");
        assert_eq!(history(&root).iter().filter(|s| s.kind == "autosave").count(), 3);
        let _ = std::fs::remove_dir_all(&d);
    }
}
