// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Consolidate (collect files): copy every media file a project uses into
//! one folder — whole files, frame sequences with all their frames, and the
//! proxies and LUTs that go with them — then relink the project to the
//! copies, as one undo step. Files already inside the folder are left
//! where they are; names that clash get a number.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use studio_model as m;

/// What consolidating did.
#[derive(Clone, Debug, Default)]
pub struct Report {
    pub copied: usize,
    pub bytes: u64,
    pub skipped: Vec<String>,
}

fn free_name(dir: &Path, name: &str, taken: &mut HashSet<PathBuf>) -> PathBuf {
    let p = Path::new(name);
    let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "media".into());
    let ext = p.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    for n in 1.. {
        let candidate = if n == 1 { dir.join(name) } else { dir.join(format!("{stem} {n}{ext}")) };
        if !candidate.exists() && !taken.contains(&candidate) {
            taken.insert(candidate.clone());
            return candidate;
        }
    }
    unreachable!()
}

fn copy(from: &Path, to: &Path, r: &mut Report) -> Result<(), String> {
    let n = std::fs::copy(from, to).map_err(|e| format!("{}: {e}", from.display()))?;
    r.copied += 1;
    r.bytes += n;
    Ok(())
}

/// Copies the project's media into `dir` and returns the relinking edit.
/// Only media used by some sequence are copied unless `all`.
pub fn consolidate(p: &m::Project, dir: &Path, all: bool) -> Result<(m::Edit, Report), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let dir_canon = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    let used: HashSet<m::MediaId> = p.sequences.iter().flat_map(|s| s.clips.iter().filter_map(|c| c.media_id())).collect();
    let mut taken = HashSet::new();
    let mut report = Report::default();
    let mut edits = Vec::new();
    for item in &p.media {
        if !all && !used.contains(&item.id) {
            continue;
        }
        let src = PathBuf::from(&item.path);
        if item.path.starts_with("offline:") || item.path.starts_with("sample:") {
            report.skipped.push(format!("{}: offline", item.name));
            continue;
        }
        let inside = |p: &Path| std::fs::canonicalize(p).ok().is_some_and(|c| c.starts_with(&dir_canon));
        let mut it = item.clone();
        if studio_imageio::sequence::Sequence::is_pattern(&src) {
            let seq = match studio_imageio::sequence::Sequence::from_pattern(&src) {
                Ok(s) => s,
                Err(e) => {
                    report.skipped.push(format!("{}: {e}", item.name));
                    continue;
                }
            };
            if inside(&seq.dir) {
                continue;
            }
            let folder = free_name(dir, &format!("{}{}", seq.prefix.trim_end_matches(['.', '_']), "_frames"), &mut taken);
            std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
            for n in seq.first..=seq.last {
                let f = seq.frame(n);
                if f.exists() {
                    copy(&f, &folder.join(f.file_name().unwrap_or_default()), &mut report)?;
                }
            }
            it.path = folder.join(seq.pattern().file_name().unwrap_or_default()).display().to_string();
        } else {
            if !src.is_file() {
                report.skipped.push(format!("{}: file missing", item.name));
                continue;
            }
            if inside(&src) {
                continue;
            }
            let to = free_name(dir, &src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "media".into()), &mut taken);
            copy(&src, &to, &mut report)?;
            it.path = to.display().to_string();
        }
        if let Some(px) = item.proxy_path.as_deref().map(PathBuf::from).filter(|p| p.is_file() && !inside(p)) {
            let pdir = dir.join("Proxies");
            std::fs::create_dir_all(&pdir).map_err(|e| e.to_string())?;
            let to = free_name(&pdir, &px.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), &mut taken);
            copy(&px, &to, &mut report)?;
            it.proxy_path = Some(to.display().to_string());
        }
        it.relative_path = None;
        edits.push(m::Edit::SetMedia { item: Box::new(it) });
    }
    // LUT files graded clips use.
    let luts: HashSet<String> = p.sequences.iter().flat_map(|s| s.clips.iter()).filter_map(|c| c.grade.as_ref()?.lut.clone()).collect();
    let mut lut_map = std::collections::HashMap::new();
    for l in luts {
        let src = PathBuf::from(&l);
        if !src.is_file() || std::fs::canonicalize(&src).ok().is_some_and(|c| c.starts_with(&dir_canon)) {
            continue;
        }
        let ldir = dir.join("LUTs");
        std::fs::create_dir_all(&ldir).map_err(|e| e.to_string())?;
        let to = free_name(&ldir, &src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), &mut taken);
        copy(&src, &to, &mut report)?;
        lut_map.insert(l, to.display().to_string());
    }
    for s in &p.sequences {
        for c in &s.clips {
            if let Some(g) = &c.grade
                && let Some(new) = g.lut.as_ref().and_then(|l| lut_map.get(l))
            {
                let mut nc = c.clone();
                if let Some(ng) = nc.grade.as_mut() {
                    ng.lut = Some(new.clone());
                }
                edits.push(m::Edit::SetClip { seq: s.id, clip: Box::new(nc) });
            }
        }
    }
    Ok((m::Edit::Batch { edits }, report))
}
