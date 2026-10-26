// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Reproducible archives: the same files always give the same bytes. Files
//! are sorted, owned by root, dated `SOURCE_DATE_EPOCH` (default: the last
//! commit's time) and gzip carries no timestamp.

use std::io::Write;
use std::path::{Path, PathBuf};

/// A file to archive: its path inside the archive (forward slashes), where
/// it is now and whether it is executable.
#[derive(Clone, Debug)]
pub struct Item {
    pub rel: String,
    pub src: PathBuf,
    pub exec: bool,
}

/// `SOURCE_DATE_EPOCH`, else the time of the last commit, else 1980-01-01.
pub fn epoch() -> u64 {
    std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .or_else(|| {
            let out = std::process::Command::new("git").current_dir(crate::root()).args(["log", "-1", "--format=%ct"]).output().ok()?;
            String::from_utf8_lossy(&out.stdout).trim().parse().ok()
        })
        .unwrap_or(315_532_800)
        .max(315_532_800)
}

fn is_exec(p: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(p).map(|m| m.permissions().mode() & 0o111 != 0).unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        p.extension().is_some_and(|e| e == "exe") || p.file_name().is_some_and(|n| n == "zornov")
    }
}

/// Every file under `dir`, sorted by path (links followed).
pub fn collect(dir: &Path) -> Result<Vec<Item>, String> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<Item>) -> Result<(), String> {
        for e in std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
            let p = e.map_err(|e| e.to_string())?.path();
            if p.is_dir() {
                walk(root, &p, out)?;
            } else {
                let rel = p.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
                out.push(Item { exec: is_exec(&p), rel, src: p });
            }
        }
        Ok(())
    }
    let mut v = Vec::new();
    walk(dir, dir, &mut v)?;
    v.sort_by(|a, b| a.rel.cmp(&b.rel));
    Ok(v)
}

/// A file made up in memory.
pub struct Made {
    pub rel: String,
    pub data: Vec<u8>,
    pub exec: bool,
    /// Make this a symbolic link to the given target (tar only).
    pub link: Option<String>,
}

impl Made {
    pub fn file(rel: &str, data: Vec<u8>, exec: bool) -> Made {
        Made { rel: rel.into(), data, exec, link: None }
    }
    pub fn link(rel: &str, target: &str) -> Made {
        Made { rel: rel.into(), data: Vec::new(), exec: false, link: Some(target.into()) }
    }
}

pub fn read(i: &Item) -> Result<Vec<u8>, String> {
    std::fs::read(&i.src).map_err(|e| format!("{}: {e}", i.src.display()))
}

/// A `.tar.gz` of `items` and `made` files, under `prefix/` when given.
pub fn tar_gz(out: &Path, prefix: Option<&str>, items: &[Item], made: &[Made]) -> Result<(), String> {
    let gz = flate2::GzBuilder::new().mtime(0).write(std::fs::File::create(out).map_err(|e| format!("{}: {e}", out.display()))?, flate2::Compression::best());
    let mut tar = tar::Builder::new(gz);
    tar.mode(tar::HeaderMode::Deterministic);
    let mtime = epoch();
    let mut all: Vec<(String, Vec<u8>, bool, Option<String>)> = Vec::new();
    for m in made {
        all.push((m.rel.clone(), m.data.clone(), m.exec, m.link.clone()));
    }
    for i in items {
        all.push((i.rel.clone(), read(i)?, i.exec, None));
    }
    all.sort_by(|a, b| a.0.cmp(&b.0));
    for (rel, data, exec, link) in all {
        let mut h = tar::Header::new_gnu();
        h.set_size(data.len() as u64);
        h.set_mode(if link.is_some() { 0o777 } else if exec { 0o755 } else { 0o644 });
        if let Some(t) = &link {
            h.set_entry_type(tar::EntryType::Symlink);
            h.set_link_name(t).map_err(|e| e.to_string())?;
        }
        h.set_mtime(mtime);
        h.set_uid(0);
        h.set_gid(0);
        h.set_cksum();
        let path = match prefix {
            Some(p) => format!("{p}/{rel}"),
            None => rel,
        };
        tar.append_data(&mut h, path, data.as_slice()).map_err(|e| e.to_string())?;
    }
    tar.into_inner().and_then(|gz| gz.finish()).map(|_| ()).map_err(|e| e.to_string())
}

/// (year, month, day, hour, minute, second) of a Unix time, UTC.
pub fn civil(t: u64) -> (u16, u8, u8, u8, u8, u8) {
    let (days, rem) = ((t / 86_400) as i64, t % 86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y as u16, m as u8, d as u8, (rem / 3600) as u8, (rem % 3600 / 60) as u8, (rem % 60) as u8)
}

/// A `.zip` of `items` under `prefix/`, with every date the epoch.
pub fn zip(out: &Path, prefix: &str, items: &[Item], made: &[Made]) -> Result<(), String> {
    let (y, mo, d, h, mi, s) = civil(epoch());
    let when = zip::DateTime::from_date_and_time(y, mo, d, h, mi, s).map_err(|_| "bad zip date".to_owned())?;
    let mut z = zip::ZipWriter::new(std::fs::File::create(out).map_err(|e| format!("{}: {e}", out.display()))?);
    let mut all: Vec<(String, Vec<u8>, bool)> = made.iter().filter(|m| m.link.is_none()).map(|m| (m.rel.clone(), m.data.clone(), m.exec)).collect();
    for i in items {
        all.push((i.rel.clone(), read(i)?, i.exec));
    }
    all.sort_by(|a, b| a.0.cmp(&b.0));
    for (rel, data, exec) in all {
        let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated).last_modified_time(when).unix_permissions(if exec { 0o755 } else { 0o644 });
        z.start_file(format!("{prefix}/{rel}"), opts).map_err(|e| e.to_string())?;
        z.write_all(&data).map_err(|e| e.to_string())?;
    }
    z.finish().map(|_| ()).map_err(|e| e.to_string())
}

/// The update archive for an install folder: its files, a manifest of them
/// and (off macOS) the marker that lets that folder update itself.
pub fn update_archive(dir: &Path, out: &Path, marker: bool) -> Result<(), String> {
    let items: Vec<Item> = collect(dir)?.into_iter().filter(|i| i.rel != studio_update::apply::MANIFEST).collect();
    let mut made = Vec::new();
    let mut names: Vec<String> = items.iter().map(|i| i.rel.clone()).collect();
    if marker && !names.iter().any(|n| n == studio_update::apply::MARKER) {
        made.push(Made::file(studio_update::apply::MARKER, Vec::new(), false));
        names.push(studio_update::apply::MARKER.into());
    }
    names.sort();
    made.push(Made::file(studio_update::apply::MANIFEST, format!("{}\n", names.join("\n")).into_bytes(), false));
    tar_gz(out, None, &items, &made)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("xtask-repro-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("sub")).unwrap();
        std::fs::write(d.join("zornov"), "binary").unwrap();
        std::fs::write(d.join("sub/z.txt"), "z").unwrap();
        std::fs::write(d.join("a.txt"), "a").unwrap();
        d
    }

    #[test]
    fn archives_are_byte_identical_whatever_the_file_dates() {
        let d = tree("same");
        let (a, b, c) = (d.join("../r1.tar.gz"), d.join("../r2.tar.gz"), d.join("../r3.zip"));
        update_archive(&d, &a, true).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        std::fs::write(d.join("a.txt"), "a").unwrap(); // newer mtime, same bytes
        update_archive(&d, &b, true).unwrap();
        assert_eq!(std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap());
        let items = collect(&d).unwrap();
        zip(&c, "zornov", &items, &[]).unwrap();
        let z1 = std::fs::read(&c).unwrap();
        zip(&c, "zornov", &items, &[]).unwrap();
        assert_eq!(z1, std::fs::read(&c).unwrap());
        let _ = std::fs::remove_dir_all(&d);
        for f in [a, b, c] {
            let _ = std::fs::remove_file(f);
        }
    }

    #[test]
    fn the_update_archive_stages_with_a_manifest_and_marker() {
        let d = tree("stage");
        let arc = d.join("../r4.tar.gz");
        update_archive(&d, &arc, true).unwrap();
        let out = d.join("../r4-out");
        studio_update::apply::stage(&arc, &out).unwrap();
        let m = std::fs::read_to_string(out.join(studio_update::apply::MANIFEST)).unwrap();
        assert_eq!(m, "a.txt\nstudio.install\nsub/z.txt\nzornov\n");
        assert!(out.join("studio.install").is_file());
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::remove_dir_all(&out);
        let _ = std::fs::remove_file(arc);
    }

    #[test]
    fn dates_convert() {
        assert_eq!(civil(315_532_800), (1980, 1, 1, 0, 0, 0));
        assert_eq!(civil(1_700_000_000), (2023, 11, 14, 22, 13, 20));
    }
}
