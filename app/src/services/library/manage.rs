// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Looking after the media library: bins, duplicates, missing files and
//! licences. The assistant's `assets` actions and the Library sheet share
//! these; each change returns what it did so the caller can say so and undo it.

use crate::assets::{Asset, Store};
use crate::{rights, safety};
use serde_json::{Value, json};
use std::path::Path;
use studio_model as m;

// ------------------------------------------------------------------ bins

#[derive(Clone, Debug)]
pub struct BinRow {
    pub id: i64,
    pub path: String,
    pub smart_rule: Option<Value>,
    pub assets: usize,
}

pub fn bins(store: &Store) -> Result<Vec<BinRow>, String> {
    Ok(store.bins()?.into_iter().map(|(id, path, smart_rule, assets)| BinRow { id, path, smart_rule, assets }).collect())
}

pub fn bins_json(store: &Store) -> Result<Value, String> {
    Ok(json!({"bins": bins(store)?.into_iter().map(|b| { let mut v = json!({"id": b.id, "bin": b.path, "assets": b.assets}); if let Some(r) = b.smart_rule { v["smart_rule"] = r; } v }).collect::<Vec<_>>()}))
}

/// Puts assets in a bin (made if it is new); returns the ones that were not in it yet,
/// which is what taking the move back removes again.
pub fn move_to_bin(store: &Store, bin: &str, ids: &[i64]) -> Result<Vec<i64>, String> {
    let bin = bin.trim();
    if bin.is_empty() {
        return Err("name the bin".into());
    }
    let before: std::collections::HashSet<i64> = match store.bin_find(bin) {
        Ok(id) => store.bin_assets(id, 1_000_000)?.into_iter().map(|a| a.id).collect(),
        Err(_) => Default::default(),
    };
    store.bin_add(bin, ids)?;
    Ok(ids.iter().copied().filter(|i| !before.contains(i)).collect())
}

pub fn take_out_of_bin(store: &Store, bin: &str, ids: &[i64]) -> Result<(), String> {
    store.bin_remove(bin, ids)
}

/// The assets in a bin, for the list under it.
pub fn bin_contents(store: &Store, bin: &str, limit: usize) -> Result<Vec<Asset>, String> {
    store.bin_assets(store.bin_find(bin)?, limit)
}

// ------------------------------------------------------------ duplicates

pub fn duplicates(store: &Store, near: bool) -> Result<Vec<Vec<Asset>>, String> {
    store.duplicates(near)
}

/// Bytes the copies take beyond one of each.
pub fn wasted_bytes(groups: &[Vec<Asset>]) -> u64 {
    groups.iter().map(|g| g.iter().skip(1).map(|x| x.size).sum::<i64>()).sum::<i64>().max(0) as u64
}

pub fn duplicates_json(store: &Store, near: bool, limit: usize) -> Result<Value, String> {
    let groups = duplicates(store, near)?;
    let wasted = wasted_bytes(&groups);
    Ok(json!({"groups": groups.len(), "wasted": safety::human(wasted), "duplicates": groups.iter().take(limit).map(|g| g.iter().map(|x| json!({"id": x.id, "path": x.path, "bytes": x.size, "missing": x.missing})).collect::<Vec<_>>()).collect::<Vec<_>>()}))
}

/// What keeping one of a group of copies did.
#[derive(Clone, Debug, Default)]
pub struct Merged {
    /// Project media now pointing at the kept file.
    pub repointed: usize,
    /// Library records dropped (the files are untouched): taking the merge
    /// back adds these paths again.
    pub forgotten: Vec<String>,
}

/// Keeps `keep` out of a group of copies: the project's media that used another
/// copy follow to the kept file (one undo step), and the other copies leave
/// the library (their files stay where they are).
pub fn keep_one(doc: &mut crate::doc::Doc, store: &Store, group: &[Asset], keep: i64) -> Result<Merged, String> {
    let kept = group.iter().find(|a| a.id == keep).ok_or("that file is not in this group")?;
    let mut edits = Vec::new();
    for other in group.iter().filter(|a| a.id != keep) {
        if let Some(old) = doc.model.media.iter().find(|m| m.path == other.path).cloned() {
            let mut item = old;
            item.path = kept.path.clone();
            edits.push(m::Edit::SetMedia { item: Box::new(item) });
        }
    }
    let repointed = edits.len();
    if !edits.is_empty() && !doc.apply("Merge duplicate media", m::Edit::Batch { edits }) {
        return Err(doc.error.take().unwrap_or_else(|| "the project could not follow the kept file".into()));
    }
    let mut forgotten = Vec::new();
    for other in group.iter().filter(|a| a.id != keep) {
        store.forget(other.id)?;
        forgotten.push(other.path.clone());
    }
    Ok(Merged { repointed, forgotten })
}

/// Takes a merge back in the library (the project half is the app's undo).
pub fn restore_forgotten(store: &Store, paths: &[String]) -> usize {
    paths.iter().filter(|p| store.add_file(Path::new(p), None).is_ok()).count()
}

// --------------------------------------------------------------- missing

pub fn missing(store: &Store, root: Option<&Path>, all: bool) -> Result<Vec<Asset>, String> {
    store.missing(if all { None } else { root })
}

pub fn missing_json(store: &Store, root: Option<&Path>, all: bool) -> Result<Value, String> {
    let list = missing(store, root, all)?;
    Ok(json!({"missing": list.len(), "assets": list.iter().map(|x| json!({"id": x.id, "name": x.name, "was": x.path})).collect::<Vec<_>>(), "next": if list.is_empty() { Value::Null } else { json!("assets action \"relink\" searches for them") }}))
}

// -------------------------------------------------------------- licences

#[derive(Clone, Debug)]
pub struct Expiring {
    pub asset: Asset,
    pub expires: String,
    pub days_left: i64,
}

pub fn expiring(store: &Store, days: i64) -> Result<Vec<Expiring>, String> {
    Ok(rights::expiring(store, days)?.into_iter().map(|(asset, expires, days_left)| Expiring { asset, expires, days_left }).collect())
}

pub fn expiring_json(store: &Store, days: i64) -> Result<Value, String> {
    Ok(json!({"expiring": expiring(store, days)?.into_iter().map(|e| json!({"asset": e.asset.name, "expires": e.expires, "days_left": e.days_left})).collect::<Vec<_>>()}))
}

/// Gives the licence a new end date; returns the date it had.
pub fn renew(store: &Store, id: i64, until: &str) -> Result<String, String> {
    set_expiry(store, id, until.trim())
}

/// Drops the end date (the licence does not run out); returns the date it had.
pub fn remove_expiry(store: &Store, id: i64) -> Result<String, String> {
    set_expiry(store, id, "")
}

fn set_expiry(store: &Store, id: i64, to: &str) -> Result<String, String> {
    let was = rights::get(store, id)?.map(|r| r.expires).unwrap_or_default();
    rights::set(store, id, &json!({"expires": to}))?;
    Ok(was)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> (Store, std::path::PathBuf) {
        let d = std::env::temp_dir().join(format!("studio-manage-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        (Store::open(&d.join("a.db")).unwrap(), d)
    }

    #[test]
    fn bins_move_and_take_back() {
        let (store, d) = scratch("bins");
        let (a, b) = (d.join("a.txt"), d.join("b.txt"));
        std::fs::write(&a, "one").unwrap();
        std::fs::write(&b, "two").unwrap();
        let (ia, ib) = (store.add_file(&a, None).unwrap(), store.add_file(&b, None).unwrap());
        assert_eq!(move_to_bin(&store, "Footage/Day 1", &[ia]).unwrap(), vec![ia]);
        // Only what was not there yet is reported, so taking it back leaves the rest.
        assert_eq!(move_to_bin(&store, "Footage/Day 1", &[ia, ib]).unwrap(), vec![ib]);
        assert_eq!(bin_contents(&store, "Footage/Day 1", 10).unwrap().len(), 2);
        take_out_of_bin(&store, "Footage/Day 1", &[ib]).unwrap();
        assert_eq!(bin_contents(&store, "Footage/Day 1", 10).unwrap().len(), 1);
        assert!(move_to_bin(&store, " ", &[ia]).is_err());
        let rows = bins(&store).unwrap();
        assert!(rows.iter().any(|r| r.path == "Footage/Day 1" && r.assets == 1));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn licence_dates_renew_and_clear() {
        let (store, d) = scratch("lic");
        let a = d.join("song.txt");
        std::fs::write(&a, "x").unwrap();
        let id = store.add_file(&a, None).unwrap();
        rights::set(&store, id, &json!({"licence": "licensed", "expires": "2020-01-01"})).unwrap();
        let list = expiring(&store, 60).unwrap();
        assert_eq!(list.len(), 1);
        assert!(list[0].days_left < 0, "already over");
        assert_eq!(renew(&store, id, "2099-01-01").unwrap(), "2020-01-01");
        assert!(expiring(&store, 60).unwrap().is_empty());
        assert_eq!(remove_expiry(&store, id).unwrap(), "2099-01-01");
        assert_eq!(rights::get(&store, id).unwrap().unwrap().expires, "");
        assert!(renew(&store, id, "next week").is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn duplicates_report_waste_and_merge_the_library() {
        let (store, d) = scratch("dup");
        let (a, b) = (d.join("a.txt"), d.join("a copy.txt"));
        std::fs::write(&a, "same bytes here").unwrap();
        std::fs::copy(&a, &b).unwrap();
        let (ia, _ib) = (store.add_file(&a, None).unwrap(), store.add_file(&b, None).unwrap());
        let groups = duplicates(&store, false).unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(wasted_bytes(&groups), "same bytes here".len() as u64);
        let mut doc = crate::services::project::new_custom("t", 640, 360, 30.0);
        let merged = keep_one(&mut doc, &store, &groups[0], ia).unwrap();
        assert_eq!(merged.repointed, 0);
        assert_eq!(merged.forgotten.len(), 1);
        assert!(duplicates(&store, false).unwrap().is_empty());
        assert_eq!(restore_forgotten(&store, &merged.forgotten), 1);
        assert_eq!(duplicates(&store, false).unwrap().len(), 1);
        assert!(a.exists() && b.exists(), "files are never touched");
        let _ = std::fs::remove_dir_all(&d);
    }
}
