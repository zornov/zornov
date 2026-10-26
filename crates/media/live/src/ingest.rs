// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Camera cards in, safely: every file is copied to the working drive and
//! to one or more backup drives in a single read of the card, then each
//! copy is read back and compared by SHA-256 against what came off the
//! card. A manifest of checksums is written beside each copy, so the
//! same check can be run again later (`verify_manifest`). Nothing on the
//! card is ever changed or deleted, and an existing file with different
//! content is never overwritten.

use crate::hash::{Sha256, hex};
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

const MEDIA: &[&str] = &[
    "mp4", "mov", "m4v", "mxf", "mts", "m2ts", "avi", "mkv", "mpg", "mpeg", "r3d", "braw", "ari", "crm", "wav", "bwf", "aif", "aiff", "mp3", "m4a", "flac", "jpg", "jpeg", "heic", "png", "tif", "tiff", "dng", "arw", "cr2", "cr3", "nef", "raf", "orf", "rw2",
];

/// A file on a card.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CardFile {
    /// Path relative to the card's root.
    pub rel: String,
    pub size: u64,
    /// Picture, sound or still (by extension); `other` are sidecars
    /// (thumbnails, XML, indexes) that are copied too.
    pub kind: String,
}

fn kind_of(p: &Path) -> &'static str {
    let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    if !MEDIA.contains(&ext.as_str()) {
        return "other";
    }
    match ext.as_str() {
        "wav" | "bwf" | "aif" | "aiff" | "mp3" | "m4a" | "flac" => "audio",
        "jpg" | "jpeg" | "heic" | "png" | "tif" | "tiff" | "dng" | "arw" | "cr2" | "cr3" | "nef" | "raf" | "orf" | "rw2" => "still",
        _ => "video",
    }
}

fn skipped(name: &str) -> bool {
    name.starts_with('.') || name == "System Volume Information" || name == "$RECYCLE.BIN" || name == "Thumbs.db" || name == "FOUND.000"
}

/// Every file on the card (or folder) to copy, in a stable order.
pub fn scan(root: &Path) -> Result<Vec<CardFile>, String> {
    if !root.is_dir() {
        return Err(format!("{} is not a folder or card", root.display()));
    }
    let mut out = Vec::new();
    fn walk(dir: &Path, root: &Path, out: &mut Vec<CardFile>) -> Result<(), String> {
        let mut entries: Vec<_> = fs::read_dir(dir).map_err(|e| format!("could not read {}: {e}", dir.display()))?.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let name = e.file_name().to_string_lossy().into_owned();
            if skipped(&name) {
                continue;
            }
            let path = e.path();
            let Ok(md) = fs::symlink_metadata(&path) else { continue };
            if md.is_dir() {
                walk(&path, root, out)?;
            } else if md.is_file() {
                let rel = path.strip_prefix(root).unwrap_or(&path).to_string_lossy().into_owned();
                out.push(CardFile { rel, size: md.len(), kind: kind_of(&path).into() });
            }
        }
        Ok(())
    }
    walk(root, root, &mut out)?;
    Ok(out)
}

/// A mounted volume that looks like a camera card or recorder.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Card {
    pub path: String,
    pub name: String,
    /// What gave it away (`DCIM`, `PRIVATE`, `XDROOT`…).
    pub layout: String,
}

const MARKERS: &[&str] = &["DCIM", "PRIVATE", "AVF_INFO", "XDROOT", "CONTENTS", "BPAV", "M4ROOT", "Clip", "CLIP", "MISC", "MP_ROOT", "AVCHD", "RDM", "PROAV"];

/// The cards among the volumes under `volumes_dir` (`/Volumes`).
pub fn cards_in(volumes_dir: &Path) -> Vec<Card> {
    let mut out = Vec::new();
    let Ok(rd) = fs::read_dir(volumes_dir) else { return out };
    for e in rd.flatten() {
        let p = e.path();
        if !p.is_dir() {
            continue;
        }
        // Exact names (a Mac's own disk has a folder called private), and never a system disk.
        let names: Vec<String> = fs::read_dir(&p).map(|r| r.flatten().map(|x| x.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default();
        if names.iter().any(|n| matches!(n.as_str(), "System" | "Applications" | "Library" | "Users" | "Windows" | "bin" | "usr")) || fs::canonicalize(&p).is_ok_and(|c| c == Path::new("/")) {
            continue;
        }
        if let Some(m) = MARKERS.iter().find(|m| names.iter().any(|n| n == **m)) {
            out.push(Card { path: p.to_string_lossy().into_owned(), name: e.file_name().to_string_lossy().into_owned(), layout: (*m).into() });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// What to copy and where.
#[derive(Clone, Debug)]
pub struct Plan {
    pub from: PathBuf,
    /// The working copy.
    pub primary: PathBuf,
    /// Backup drives: each gets the same files.
    pub backups: Vec<PathBuf>,
    /// Folder name under each destination (default: the card's name and
    /// the day).
    pub name: Option<String>,
    /// Allow a destination on the card's own drive (for testing, or when
    /// the person says so); by default it is refused.
    pub allow_same_drive: bool,
}

/// How one destination's copy of one file came out.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CopyResult {
    pub dest: String,
    /// `verified`, `already_there` (same bytes were already in place),
    /// `renamed` (a different file was in the way; this one is kept
    /// beside it) or `MISMATCH`.
    pub status: String,
    pub path: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct FileReport {
    pub rel: String,
    pub size: u64,
    pub sha256: String,
    pub kind: String,
    pub copies: Vec<CopyResult>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Report {
    pub folder: String,
    pub files: Vec<FileReport>,
    pub bytes: u64,
    /// Files whose every copy was read back and matched.
    pub verified: usize,
    pub problems: Vec<String>,
    pub cancelled: bool,
    pub manifests: Vec<String>,
}

impl Report {
    pub fn ok(&self) -> bool {
        self.problems.is_empty() && !self.cancelled
    }
}

/// The drive a path is on (so a backup on the same drive can be refused).
#[cfg(unix)]
fn device(p: &Path) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    let mut q = p;
    loop {
        if let Ok(m) = fs::metadata(q) {
            return Some(m.dev());
        }
        q = q.parent()?;
    }
}

#[cfg(not(unix))]
fn device(_: &Path) -> Option<u64> {
    None
}

/// The destination folders for a plan, checked: all different drives.
fn destinations(plan: &Plan, name: &str) -> Result<Vec<PathBuf>, String> {
    let mut dests = vec![plan.primary.join(name)];
    dests.extend(plan.backups.iter().map(|b| b.join(name)));
    let src_dev = device(&plan.from);
    let devs: Vec<Option<u64>> = dests.iter().map(|d| device(d)).collect();
    for (i, d) in devs.iter().enumerate() {
        if d.is_some() && *d == src_dev && !plan.allow_same_drive {
            return Err(format!("{} is on the same drive as the card; a copy there protects nothing", dests[i].display()));
        }
        for (j, e) in devs.iter().enumerate().skip(i + 1) {
            if d.is_some() && d == e && !plan.allow_same_drive {
                return Err(format!("{} and {} are on the same drive; a backup must be on a different drive", dests[i].display(), dests[j].display()));
            }
        }
    }
    Ok(dests)
}

fn unique(path: &Path) -> PathBuf {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("file").to_owned();
    let ext = path.extension().and_then(|s| s.to_str()).map(|e| format!(".{e}")).unwrap_or_default();
    for n in 2.. {
        let c = path.with_file_name(format!("{stem}-{n}{ext}"));
        if !c.exists() {
            return c;
        }
    }
    unreachable!()
}

fn hash_file(path: &Path) -> std::io::Result<(String, u64)> {
    let mut f = File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    let mut n = 0u64;
    loop {
        let k = f.read(&mut buf)?;
        if k == 0 {
            break;
        }
        h.update(&buf[..k]);
        n += k as u64;
    }
    Ok((hex(&h.finish()), n))
}

/// Progress: bytes done of total, and the file being copied.
pub type Progress<'a> = &'a mut dyn FnMut(u64, u64, &str);

/// Copies the card to every destination and verifies each copy.
pub fn ingest(plan: &Plan, cancel: &AtomicBool, progress: Progress) -> Result<Report, String> {
    let files = scan(&plan.from)?;
    if files.is_empty() {
        return Err(format!("nothing to copy on {}", plan.from.display()));
    }
    let name = plan.name.clone().unwrap_or_else(|| {
        let card = plan.from.file_name().and_then(|s| s.to_str()).unwrap_or("card");
        format!("{card}-{}", day_stamp())
    });
    let dests = destinations(plan, &name)?;
    let total: u64 = files.iter().map(|f| f.size).sum::<u64>() * dests.len() as u64;
    // Space check first: a card that does not fit is found out now, not
    // halfway.
    for d in &dests {
        if let Some(free) = free_space(d) {
            let need: u64 = files.iter().map(|f| f.size).sum();
            if need + (64 << 20) > free {
                return Err(format!("{} has {} MB free; the card holds {} MB", d.display(), free >> 20, need >> 20));
            }
        }
    }
    for d in &dests {
        fs::create_dir_all(d).map_err(|e| format!("could not make {}: {e}", d.display()))?;
    }
    let mut report = Report { folder: name.clone(), files: Vec::new(), bytes: 0, verified: 0, problems: Vec::new(), cancelled: false, manifests: Vec::new() };
    let mut done = 0u64;
    for cf in &files {
        if cancel.load(Ordering::Relaxed) {
            report.cancelled = true;
            break;
        }
        let src = plan.from.join(&cf.rel);
        let mut input = File::open(&src).map_err(|e| format!("could not read {}: {e}", cf.rel))?;
        let mtime = input.metadata().and_then(|m| m.modified()).ok();
        // One read of the card feeds every destination.
        struct Out {
            tmp: PathBuf,
            fin: PathBuf,
            f: File,
            exists_same: bool,
        }
        let mut outs: Vec<Out> = Vec::new();
        for d in &dests {
            let fin = d.join(&cf.rel);
            if let Some(parent) = fin.parent() {
                fs::create_dir_all(parent).map_err(|e| format!("could not make {}: {e}", parent.display()))?;
            }
            let tmp = fin.with_file_name(format!(".{}.partial", fin.file_name().and_then(|s| s.to_str()).unwrap_or("f")));
            let f = File::create(&tmp).map_err(|e| format!("could not write {}: {e}", tmp.display()))?;
            outs.push(Out { tmp, fin, f, exists_same: false });
        }
        let mut h = Sha256::new();
        let mut buf = vec![0u8; 4 << 20];
        let mut size = 0u64;
        loop {
            let k = input.read(&mut buf).map_err(|e| format!("could not read {}: {e}", cf.rel))?;
            if k == 0 {
                break;
            }
            h.update(&buf[..k]);
            size += k as u64;
            for o in &mut outs {
                o.f.write_all(&buf[..k]).map_err(|e| format!("could not write {}: {e}", o.tmp.display()))?;
            }
            done += k as u64 * outs.len() as u64;
            progress(done, total, &cf.rel);
            if cancel.load(Ordering::Relaxed) {
                break;
            }
        }
        if cancel.load(Ordering::Relaxed) {
            for o in &outs {
                let _ = fs::remove_file(&o.tmp);
            }
            report.cancelled = true;
            break;
        }
        let sha = hex(&h.finish());
        let mut copies = Vec::new();
        for o in &mut outs {
            o.f.sync_all().map_err(|e| format!("could not flush {}: {e}", o.tmp.display()))?;
            if let Some(t) = mtime {
                let _ = o.f.set_modified(t);
            }
            // Something already there under this name?
            let mut status = "verified";
            let mut target = o.fin.clone();
            if o.fin.exists() {
                match hash_file(&o.fin) {
                    Ok((hh, _)) if hh == sha => {
                        o.exists_same = true;
                        status = "already_there";
                    }
                    _ => {
                        target = unique(&o.fin);
                        status = "renamed";
                    }
                }
            }
            if o.exists_same {
                let _ = fs::remove_file(&o.tmp);
            } else {
                fs::rename(&o.tmp, &target).map_err(|e| format!("could not finish {}: {e}", target.display()))?;
                // Read back what is on the drive now.
                match hash_file(&target) {
                    Ok((hh, n)) if hh == sha && n == size => {}
                    Ok(_) => {
                        status = "MISMATCH";
                        report.problems.push(format!("{}: the copy at {} does not match the card", cf.rel, target.display()));
                    }
                    Err(e) => {
                        status = "MISMATCH";
                        report.problems.push(format!("{}: could not read back {}: {e}", cf.rel, target.display()));
                    }
                }
            }
            copies.push(CopyResult { dest: o.fin.parent().map(|_| name.clone()).unwrap_or_default(), status: status.into(), path: target.to_string_lossy().into_owned() });
        }
        if copies.iter().all(|c| c.status != "MISMATCH") {
            report.verified += 1;
        }
        report.bytes += size;
        report.files.push(FileReport { rel: cf.rel.clone(), size, sha256: sha, kind: cf.kind.clone(), copies });
    }
    // Manifests: beside each copy, so each drive can be checked alone.
    if !report.cancelled {
        for d in &dests {
            let m = d.join("ingest-manifest.json");
            let body = serde_json::to_vec_pretty(&report).map_err(|e| e.to_string())?;
            fs::write(&m, body).map_err(|e| format!("could not write {}: {e}", m.display()))?;
            let mut sums = String::new();
            for f in &report.files {
                sums.push_str(&format!("{}  {}\n", f.sha256, f.rel));
            }
            let _ = fs::write(d.join("sha256sums.txt"), sums);
            report.manifests.push(m.to_string_lossy().into_owned());
        }
    }
    Ok(report)
}

/// Checks a copied folder against its manifest: every file present, the
/// right size, the same checksum. Returns the problems (empty = all good)
/// and how many files were checked.
pub fn verify_manifest(dir: &Path) -> Result<(usize, Vec<String>), String> {
    let body = fs::read(dir.join("ingest-manifest.json")).map_err(|e| format!("no manifest in {}: {e}", dir.display()))?;
    let rep: Report = serde_json::from_slice(&body).map_err(|e| format!("manifest unreadable: {e}"))?;
    let mut bad = Vec::new();
    for f in &rep.files {
        let p = dir.join(&f.rel);
        let alt = f.copies.iter().map(|c| PathBuf::from(&c.path)).find(|c| c.starts_with(dir));
        let p = if p.exists() { p } else { alt.unwrap_or(p) };
        match hash_file(&p) {
            Ok((h, n)) if h == f.sha256 && n == f.size => {}
            Ok(_) => bad.push(format!("{}: changed since it was copied", f.rel)),
            Err(_) => bad.push(format!("{}: missing", f.rel)),
        }
    }
    Ok((rep.files.len(), bad))
}

fn day_stamp() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) as i64;
    let days = secs.div_euclid(86400);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

#[cfg(unix)]
fn free_space(p: &Path) -> Option<u64> {
    use std::ffi::CString;
    let mut q = p;
    while !q.exists() {
        q = q.parent()?;
    }
    let c = CString::new(q.to_str()?).ok()?;
    // SAFETY: statvfs fills the struct for a valid NUL-terminated path.
    unsafe {
        let mut s: libc_statvfs::Statvfs = std::mem::zeroed();
        if libc_statvfs::statvfs(c.as_ptr(), &mut s) == 0 { Some(s.f_bavail as u64 * s.f_frsize as u64) } else { None }
    }
}

#[cfg(not(unix))]
fn free_space(_: &Path) -> Option<u64> {
    None
}

/// The one libc call used, bound by hand (the workspace has no libc dep here).
#[cfg(target_os = "macos")]
mod libc_statvfs {
    #[repr(C)]
    pub struct Statvfs {
        pub f_bsize: std::ffi::c_ulong,
        pub f_frsize: std::ffi::c_ulong,
        pub f_blocks: u32,
        pub f_bfree: u32,
        pub f_bavail: u32,
        pub f_files: u32,
        pub f_ffree: u32,
        pub f_favail: u32,
        pub f_fsid: std::ffi::c_ulong,
        pub f_flag: std::ffi::c_ulong,
        pub f_namemax: std::ffi::c_ulong,
    }
    unsafe extern "C" {
        pub fn statvfs(path: *const std::ffi::c_char, buf: *mut Statvfs) -> i32;
    }
}
#[cfg(all(unix, not(target_os = "macos")))]
mod libc_statvfs {
    #[repr(C)]
    pub struct Statvfs {
        pub f_bsize: std::ffi::c_ulong,
        pub f_frsize: std::ffi::c_ulong,
        pub f_blocks: u64,
        pub f_bfree: u64,
        pub f_bavail: u64,
        pub rest: [u64; 16],
    }
    unsafe extern "C" {
        pub fn statvfs(path: *const std::ffi::c_char, buf: *mut Statvfs) -> i32;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("studio-live-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn fake_card(root: &Path) {
        fs::create_dir_all(root.join("DCIM/100MEDIA")).unwrap();
        fs::create_dir_all(root.join("PRIVATE/M4ROOT/CLIP")).unwrap();
        fs::create_dir_all(root.join(".Spotlight-V100")).unwrap();
        let big: Vec<u8> = (0..9_000_000u32).map(|i| (i.wrapping_mul(2654435761) >> 24) as u8).collect();
        fs::write(root.join("DCIM/100MEDIA/C0001.MP4"), &big).unwrap();
        fs::write(root.join("DCIM/100MEDIA/A0002.WAV"), b"RIFFxxxxWAVE").unwrap();
        fs::write(root.join("PRIVATE/M4ROOT/CLIP/C0001M01.XML"), b"<x/>").unwrap();
        fs::write(root.join(".Spotlight-V100/junk"), b"x").unwrap();
        fs::write(root.join("._C0001.MP4"), b"x").unwrap();
    }

    #[test]
    fn scan_finds_media_and_sidecars_and_skips_system_files() {
        let d = scratch("scan");
        fake_card(&d.join("CARD"));
        let files = scan(&d.join("CARD")).unwrap();
        let rels: Vec<&str> = files.iter().map(|f| f.rel.as_str()).collect();
        assert_eq!(rels, ["DCIM/100MEDIA/A0002.WAV", "DCIM/100MEDIA/C0001.MP4", "PRIVATE/M4ROOT/CLIP/C0001M01.XML"]);
        assert_eq!(files[0].kind, "audio");
        assert_eq!(files[1].kind, "video");
        assert_eq!(files[2].kind, "other");
        assert_eq!(cards_in(&d).len(), 1);
        assert_eq!(cards_in(&d)[0].layout, "DCIM");
        // A disk with a private folder and a system is not a card.
        fs::create_dir_all(d.join("Mac/private")).unwrap();
        fs::create_dir_all(d.join("Mac/System")).unwrap();
        fs::create_dir_all(d.join("Plain/private")).unwrap();
        assert_eq!(cards_in(&d).len(), 1, "{:?}", cards_in(&d));
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn copies_to_two_drives_verified_and_detects_later_damage() {
        let d = scratch("copy");
        fake_card(&d.join("CARD"));
        let plan = Plan { from: d.join("CARD"), primary: d.join("work"), backups: vec![d.join("backupA"), d.join("backupB")], name: Some("day1".into()), allow_same_drive: true };
        let mut last = 0;
        let rep = ingest(&plan, &AtomicBool::new(false), &mut |done, total, _| {
            assert!(done <= total);
            last = done;
        })
        .unwrap();
        assert!(rep.ok(), "{:?}", rep.problems);
        assert_eq!(rep.files.len(), 3);
        assert_eq!(rep.verified, 3);
        assert!(rep.files.iter().all(|f| f.copies.len() == 3 && f.copies.iter().all(|c| c.status == "verified")));
        assert_eq!(last, rep.bytes * 3);
        for dest in ["work", "backupA", "backupB"] {
            let (n, bad) = verify_manifest(&d.join(dest).join("day1")).unwrap();
            assert_eq!((n, bad.len()), (3, 0), "{dest}: {bad:?}");
        }
        // The card is untouched.
        assert_eq!(scan(&d.join("CARD")).unwrap().len(), 3);
        // One bit flips on a backup drive later: found.
        let victim = d.join("backupB/day1/DCIM/100MEDIA/C0001.MP4");
        let mut bytes = fs::read(&victim).unwrap();
        bytes[1234] ^= 1;
        fs::write(&victim, bytes).unwrap();
        let (_, bad) = verify_manifest(&d.join("backupB/day1")).unwrap();
        assert_eq!(bad, ["DCIM/100MEDIA/C0001.MP4: changed since it was copied"]);
        // Running again: identical files are recognised, not copied twice.
        let again = ingest(&plan, &AtomicBool::new(false), &mut |_, _, _| {}).unwrap();
        let w = again.files.iter().find(|f| f.rel.ends_with("A0002.WAV")).unwrap();
        assert!(w.copies.iter().any(|c| c.status == "already_there"));
        // The damaged backup is not overwritten: the good copy sits beside it.
        let c = again.files.iter().find(|f| f.rel.ends_with("C0001.MP4")).unwrap();
        assert!(c.copies.iter().any(|c| c.status == "renamed"), "{:?}", c.copies);
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn cancelling_leaves_no_partial_files() {
        let d = scratch("cancel");
        fake_card(&d.join("CARD"));
        let plan = Plan { from: d.join("CARD"), primary: d.join("work"), backups: vec![], name: Some("x".into()), allow_same_drive: true };
        let stop = AtomicBool::new(false);
        let rep = ingest(&plan, &stop, &mut |done, _, _| {
            if done > 1000 {
                stop.store(true, Ordering::Relaxed);
            }
        })
        .unwrap();
        assert!(rep.cancelled);
        let mut partial = 0;
        for f in scan(&d.join("work")).unwrap_or_default() {
            let _ = f;
        }
        fn count(p: &Path, n: &mut usize) {
            for e in fs::read_dir(p).into_iter().flatten().flatten() {
                if e.path().is_dir() {
                    count(&e.path(), n)
                } else if e.file_name().to_string_lossy().ends_with(".partial") {
                    *n += 1
                }
            }
        }
        count(&d.join("work"), &mut partial);
        assert_eq!(partial, 0);
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn a_backup_on_the_card_drive_is_refused() {
        // /tmp and the card share a drive in this test.
        let d = scratch("same");
        fake_card(&d.join("CARD"));
        let plan = Plan { from: d.join("CARD"), primary: d.join("work"), backups: vec![], name: Some("x".into()), allow_same_drive: false };
        let e = ingest(&plan, &AtomicBool::new(false), &mut |_, _, _| {}).err();
        assert!(e.unwrap().contains("same drive"));
        let _ = fs::remove_dir_all(d);
    }
}
