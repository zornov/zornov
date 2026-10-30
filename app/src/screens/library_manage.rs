// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Looking after the media library: Bins, Duplicates, Missing and Licences.
//! Every list is collapsed until opened, every row carries its action, results
//! are toasts, and every change can be taken back (the project's changes with
//! Undo like any edit, the library's with "Undo" at the top of the sheet).
//! "Relink missing" looks in chosen folders, shows what it found, and then
//! points the project at it in one undo step.
//!
//! Hook: `show(ui, app)` goes in the Media tab of the Library ▸ Safety sheet
//! (screens/library_safety.rs) and `banner(ui, app)` above the media list; the
//! banner draws only while media is offline.

#![allow(dead_code)] // hooked into the Library sheet by screens/library_safety.rs (see the report)

use crate::app::StudioApp;
use crate::assets::{self, Asset, Store};
use crate::kit::{self, Button, Segment};
use crate::safety;
use crate::services::library::manage::{self, BinRow, Expiring};
use crate::services::library::relink::{self, RelinkReport};
use crate::theme::{self, space};
use eframe::egui::{self, RichText};
use std::collections::HashMap;
use std::path::PathBuf;

const VIEWS: [&str; 4] = ["Bins", "Duplicates", "Missing", "Licences"];
pub const BINS: usize = 0;
pub const DUPLICATES: usize = 1;
pub const MISSING: usize = 2;
pub const LICENCES: usize = 3;

/// How to take the last change back.
#[derive(Clone, Debug)]
pub enum Undo {
    BinAdded { bin: String, ids: Vec<i64> },
    BinRemoved { bin: String, id: i64 },
    /// `repointed`: the project followed the kept file, which is one undo step in the project.
    Merge { forgotten: Vec<String>, repointed: bool },
    Expiry { id: i64, was: String },
    Relink { back: Vec<(i64, PathBuf)> },
}

#[derive(Clone, Default)]
pub struct State {
    pub view: usize,
    /// Set by the banner: the Missing view looks for candidates on its next frame.
    pub wants_plan: bool,
    loaded: bool,
    bins: Vec<(BinRow, Vec<Asset>)>,
    bin_name: String,
    in_project: Vec<Asset>,
    groups: Vec<Vec<Asset>>,
    near: bool,
    missing: Vec<Asset>,
    plan: Option<RelinkReport>,
    expiring: Vec<Expiring>,
    renew: HashMap<i64, String>,
    undo: Option<(String, Undo)>,
    note: String,
}

fn state_id() -> egui::Id {
    egui::Id::new("library-manage-state")
}

fn root(app: &StudioApp) -> Option<PathBuf> {
    app.doc.folder.as_ref().map(|f| f.root().to_path_buf())
}

fn files(n: usize) -> String {
    if n == 1 { "1 file".into() } else { format!("{n} files") }
}

// ------------------------------------------------------------- the actions
// Each returns what happened in words for the toast, and how to take it back.

pub fn reload(st: &mut State, doc: &crate::doc::Doc, root: Option<&std::path::Path>) -> Result<(), String> {
    let store = assets::global()?;
    reload_with(st, store, doc, root)
}

fn reload_with(st: &mut State, store: &Store, doc: &crate::doc::Doc, root: Option<&std::path::Path>) -> Result<(), String> {
    if let Some(r) = root {
        store.sync_model(r, &doc.model)?;
    }
    st.bins = manage::bins(store)?
        .into_iter()
        .map(|b| {
            let items = manage::bin_contents(store, &b.path, 200).unwrap_or_default();
            (b, items)
        })
        .collect();
    st.in_project = doc.model.media.iter().filter_map(|m| store.resolve(&serde_json::json!(m.path)).ok()).collect();
    st.groups = manage::duplicates(store, st.near)?;
    st.missing = manage::missing(store, root, false)?;
    st.expiring = manage::expiring(store, 90)?;
    st.loaded = true;
    Ok(())
}

pub fn move_to_bin(store: &Store, bin: &str, assets: &[Asset]) -> Result<(String, Undo), String> {
    let ids: Vec<i64> = assets.iter().map(|a| a.id).collect();
    let added = manage::move_to_bin(store, bin, &ids)?;
    let what = if assets.len() == 1 { assets[0].name.clone() } else { files(assets.len()) };
    Ok((format!("Moved {what} to {}", bin.trim()), Undo::BinAdded { bin: bin.trim().to_owned(), ids: added }))
}

pub fn take_out(store: &Store, bin: &str, asset: &Asset) -> Result<(String, Undo), String> {
    manage::take_out_of_bin(store, bin, &[asset.id])?;
    Ok((format!("Took {} out of {bin}", asset.name), Undo::BinRemoved { bin: bin.to_owned(), id: asset.id }))
}

pub fn keep_one(doc: &mut crate::doc::Doc, store: &Store, group: &[Asset], keep: i64) -> Result<(String, Undo), String> {
    let name = group.iter().find(|a| a.id == keep).map(|a| a.name.clone()).unwrap_or_default();
    let merged = manage::keep_one(doc, store, group, keep)?;
    Ok((format!("Kept {name} and merged {} copies", merged.forgotten.len()), Undo::Merge { forgotten: merged.forgotten, repointed: merged.repointed > 0 }))
}

pub fn renew(store: &Store, a: &Expiring, until: &str) -> Result<(String, Undo), String> {
    let was = manage::renew(store, a.asset.id, until)?;
    Ok((format!("Renewed {} until {}", a.asset.name, until.trim()), Undo::Expiry { id: a.asset.id, was }))
}

pub fn remove_expiry(store: &Store, a: &Expiring) -> Result<(String, Undo), String> {
    let was = manage::remove_expiry(store, a.asset.id)?;
    Ok((format!("Removed the end date from {}", a.asset.name), Undo::Expiry { id: a.asset.id, was }))
}

/// Looks for the project's missing files; changes nothing.
pub fn find_candidates(store: &Store, root: Option<&std::path::Path>, search: &[PathBuf]) -> Result<RelinkReport, String> {
    relink::plan(store, root, false, search, true)
}

pub fn relink_now(doc: &mut crate::doc::Doc, store: &Store, report: &mut RelinkReport) -> Result<(String, Undo), String> {
    let back: Vec<(i64, PathBuf)> = report.found.iter().map(|c| (c.asset.id, PathBuf::from(&c.asset.path))).collect();
    let n = relink::apply(doc, store, report)?;
    Ok((format!("Relinked {}", files(n)), Undo::Relink { back }))
}

pub fn take_back(doc: &mut crate::doc::Doc, store: &Store, undo: &Undo) -> Result<String, String> {
    match undo {
        Undo::BinAdded { bin, ids } => {
            manage::take_out_of_bin(store, bin, ids)?;
            Ok(format!("Took {} out of {bin} again", files(ids.len())))
        }
        Undo::BinRemoved { bin, id } => {
            manage::move_to_bin(store, bin, &[*id])?;
            Ok(format!("Put the file back in {bin}"))
        }
        Undo::Merge { forgotten, repointed } => {
            if *repointed {
                project_undo(doc, "Merge duplicate media")?;
            }
            let n = manage::restore_forgotten(store, forgotten);
            Ok(format!("Brought back {}", files(n)))
        }
        Undo::Expiry { id, was } => {
            if was.is_empty() {
                manage::remove_expiry(store, *id)?;
            } else {
                manage::renew(store, *id, was)?;
            }
            Ok("Put the end date back".into())
        }
        Undo::Relink { back } => {
            project_undo(doc, "Relink media")?;
            for (id, path) in back {
                store.moved(*id, path)?;
            }
            Ok(format!("Took back the relink of {}", files(back.len())))
        }
    }
}

/// Undoes the project's last step only when it is the one we made.
fn project_undo(doc: &mut crate::doc::Doc, label: &str) -> Result<(), String> {
    if doc.undo_label() != Some(label) {
        return Err(format!("The project changed since; use Edit ▸ Undo to take back \"{label}\""));
    }
    doc.undo();
    Ok(())
}

// ------------------------------------------------------------------ screen

fn done(ui: &egui::Ui, st: &mut State, r: Result<(String, Undo), String>) {
    match r {
        Ok((text, undo)) => {
            kit::push(ui.ctx(), text.clone(), None, None);
            st.note = text.clone();
            st.undo = Some((text, undo));
        }
        Err(e) => {
            kit::push_error(ui.ctx(), e.clone());
            st.note = e;
        }
    }
    st.loaded = false;
}

/// A bar that appears only while media is offline and sends the person to Missing.
pub fn banner(ui: &mut egui::Ui, app: &mut StudioApp) -> bool {
    let n = crate::services::library::offline_ids(&app.doc.project.media).len();
    if n == 0 {
        return false;
    }
    let t = theme::tokens(ui.ctx());
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("{} offline", files(n))).font(theme::body()).color(t.text_2));
        if Button::secondary("Relink missing…").compact().show(ui).clicked() {
            let mut st: State = ui.ctx().data_mut(|d| d.get_temp(state_id())).unwrap_or_default();
            st.view = MISSING;
            st.wants_plan = true;
            ui.ctx().data_mut(|d| d.insert_temp(state_id(), st));
        }
    });
    true
}

pub fn show(ui: &mut egui::Ui, app: &mut StudioApp) {
    match assets::global() {
        Ok(store) => show_with(ui, app, store),
        Err(e) => {
            let t = theme::tokens(ui.ctx());
            ui.label(RichText::new(e).font(theme::body()).color(t.text_2));
        }
    }
}

/// [`show`] over a given library (the tests use a scratch one).
pub fn show_with(ui: &mut egui::Ui, app: &mut StudioApp, store: &Store) {
    let mut st: State = ui.ctx().data_mut(|d| d.get_temp(state_id())).unwrap_or_default();
    let t = theme::tokens(ui.ctx());
    if !st.loaded {
        if let Err(e) = reload_with(&mut st, store, &app.doc, root(app).as_deref()) {
            st.note = e;
        }
    }
    ui.horizontal(|ui| {
        let segs: Vec<Segment> = VIEWS.iter().map(|v| Segment::text(v)).collect();
        kit::segmented(ui, "library-manage-views", &mut st.view, &segs, None, None);
        if let Some((label, undo)) = st.undo.clone() {
            if Button::ghost("Undo").compact().show(ui).on_hover_text(label).clicked() {
                let r = take_back(&mut app.doc, store, &undo);
                st.undo = None;
                match r {
                    Ok(text) => {
                        kit::push(ui.ctx(), text.clone(), None, None);
                        st.note = text;
                    }
                    Err(e) => {
                        kit::push_error(ui.ctx(), e.clone());
                        st.note = e;
                    }
                }
                st.loaded = false;
            }
        }
    });
    ui.add_space(space::SM);
    if !st.note.is_empty() {
        ui.label(RichText::new(&st.note).font(theme::body()).color(t.text_2));
        ui.add_space(space::SM);
    }
    match st.view {
        BINS => bins_view(ui, app, store, &mut st),
        DUPLICATES => duplicates_view(ui, app, store, &mut st),
        MISSING => missing_view(ui, app, store, &mut st),
        _ => licences_view(ui, store, &mut st),
    }
    ui.ctx().data_mut(|d| d.insert_temp(state_id(), st));
}

fn muted(ui: &mut egui::Ui, text: &str) {
    let t = theme::tokens(ui.ctx());
    ui.label(RichText::new(text).font(theme::body()).color(t.text_3));
}

fn name_label(ui: &mut egui::Ui, text: &str, width: f32) {
    ui.add_sized([width.max(40.0), 18.0], egui::Label::new(RichText::new(text).font(theme::body())).truncate());
}

fn bins_view(ui: &mut egui::Ui, _app: &mut StudioApp, store: &Store, st: &mut State) {
    ui.horizontal(|ui| {
        ui.label("Bin");
        ui.add(egui::TextEdit::singleline(&mut st.bin_name).hint_text("Footage/Day 1").desired_width(200.0));
    });
    ui.add_space(space::SM);
    let mut action: Option<Result<(String, Undo), String>> = None;
    egui::CollapsingHeader::new(format!("In this project ({})", st.in_project.len())).id_salt("manage-in-project").default_open(false).show(ui, |ui| {
        if st.in_project.is_empty() {
            muted(ui, "This project has no media yet.");
        }
        for a in &st.in_project {
            ui.horizontal(|ui| {
                name_label(ui, &a.name, ui.available_width() - 120.0);
                let ready = !st.bin_name.trim().is_empty();
                if ui.add_enabled_ui(ready, |ui| Button::secondary("Move to bin").compact().show(ui)).inner.on_disabled_hover_text("Name a bin above first").clicked() {
                    action = Some(move_to_bin(store, &st.bin_name, std::slice::from_ref(a)));
                }
            });
        }
    });
    if st.bins.is_empty() {
        muted(ui, "No bins yet. Name one above and move media into it.");
    }
    let mut out: Option<(String, Asset)> = None;
    for (b, items) in &st.bins {
        let kind = if b.smart_rule.is_some() { " · smart" } else { "" };
        egui::CollapsingHeader::new(format!("{} ({}){kind}", b.path, b.assets)).id_salt(("manage-bin", b.id)).default_open(false).show(ui, |ui| {
            for a in items {
                ui.horizontal(|ui| {
                    name_label(ui, &a.name, ui.available_width() - 90.0);
                    if b.smart_rule.is_none() && Button::ghost("Take out").compact().show(ui).clicked() {
                        out = Some((b.path.clone(), a.clone()));
                    }
                });
            }
        });
    }
    if let Some((bin, a)) = out {
        action = Some(take_out(store, &bin, &a));
    }
    if let Some(r) = action {
        done(ui, st, r);
    }
}

fn duplicates_view(ui: &mut egui::Ui, app: &mut StudioApp, store: &Store, st: &mut State) {
    ui.horizontal(|ui| {
        let before = st.near;
        ui.checkbox(&mut st.near, "Include look-alikes");
        if st.near != before {
            st.loaded = false;
        }
        muted(ui, &format!("{} groups · {} wasted", st.groups.len(), safety::human(manage::wasted_bytes(&st.groups))));
    });
    if st.groups.is_empty() {
        muted(ui, "No duplicates.");
    }
    let mut keep: Option<(usize, i64)> = None;
    for (i, g) in st.groups.iter().enumerate() {
        egui::CollapsingHeader::new(format!("{} · {} copies", g[0].name, g.len())).id_salt(("manage-dup", i)).default_open(false).show(ui, |ui| {
            for a in g {
                ui.horizontal(|ui| {
                    name_label(ui, &a.path, ui.available_width() - 110.0);
                    if Button::secondary("Keep this one").compact().show(ui).clicked() {
                        keep = Some((i, a.id));
                    }
                });
            }
        });
    }
    if let Some((i, id)) = keep {
        let group = st.groups[i].clone();
        let r = keep_one(&mut app.doc, store, &group, id);
        done(ui, st, r);
    }
}

fn missing_view(ui: &mut egui::Ui, app: &mut StudioApp, store: &Store, st: &mut State) {
    let root = root(app);
    if st.wants_plan {
        st.wants_plan = false;
        match find_candidates(store, root.as_deref(), &[]) {
            Ok(p) => st.plan = Some(p),
            Err(e) => st.note = e,
        }
    }
    ui.horizontal(|ui| {
        muted(ui, &format!("{} missing", files(st.missing.len())));
        if Button::secondary("Relink missing…").compact().show(ui).clicked() {
            st.wants_plan = true;
        }
        if Button::ghost("Choose folder…").compact().show(ui).clicked() {
            if let Some(dir) = rfd::FileDialog::new().set_title("Look for missing media in").pick_folder() {
                match find_candidates(store, root.as_deref(), &[dir]) {
                    Ok(p) => st.plan = Some(p),
                    Err(e) => st.note = e,
                }
            }
        }
    });
    if st.missing.is_empty() && st.plan.is_none() {
        muted(ui, "Nothing is missing.");
    }
    egui::CollapsingHeader::new(format!("Missing ({})", st.missing.len())).id_salt("manage-missing").default_open(false).show(ui, |ui| {
        for a in &st.missing {
            name_label(ui, &format!("{}  ·  was {}", a.name, a.path), ui.available_width());
        }
    });
    let mut apply = false;
    if let Some(p) = &st.plan {
        ui.add_space(space::SM);
        if p.found.is_empty() {
            muted(ui, "Nothing found in those folders. Choose another folder.");
        } else {
            ui.label(RichText::new(format!("Found {} of {}", p.found.len(), p.wanted)).font(theme::body()));
            for c in &p.found {
                name_label(ui, &format!("{} → {}  ({})", c.asset.name, c.to.display(), c.how), ui.available_width());
            }
            if Button::primary(&format!("Relink {}", files(p.found.len()))).show(ui).clicked() {
                apply = true;
            }
        }
    }
    if apply {
        if let Some(mut p) = st.plan.take() {
            let r = relink_now(&mut app.doc, store, &mut p);
            done(ui, st, r);
        }
    }
}

fn licences_view(ui: &mut egui::Ui, store: &Store, st: &mut State) {
    if st.expiring.is_empty() {
        muted(ui, "No licence runs out in the next 90 days.");
    }
    let mut act: Option<Result<(String, Undo), String>> = None;
    for e in &st.expiring {
        let when = if e.days_left < 0 { format!("ran out {}", e.expires) } else { format!("{} days left", e.days_left) };
        egui::CollapsingHeader::new(format!("{} · {when}", e.asset.name)).id_salt(("manage-lic", e.asset.id)).default_open(false).show(ui, |ui| {
            ui.horizontal(|ui| {
                let text = st.renew.entry(e.asset.id).or_default();
                ui.add(egui::TextEdit::singleline(text).hint_text("2027-03-31").desired_width(110.0));
                if Button::secondary("Renew").compact().show(ui).clicked() {
                    act = Some(renew(store, e, text));
                }
                if Button::ghost("Remove end date").compact().show(ui).clicked() {
                    act = Some(remove_expiry(store, e));
                }
            });
        });
    }
    if let Some(r) = act {
        done(ui, st, r);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn png(path: &std::path::Path, seed: u8) {
        let mut enc = png::Encoder::new(std::io::BufWriter::new(std::fs::File::create(path).unwrap()), 48, 32);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        let mut wr = enc.write_header().unwrap();
        let data: Vec<u8> = (0..48 * 32u32).flat_map(|i| [seed.wrapping_mul(40).wrapping_add((i % 7) as u8 * seed), (i / 48) as u8, 90]).collect();
        wr.write_image_data(&data).unwrap();
    }

    fn scratch(tag: &str) -> (Store, std::path::PathBuf) {
        let d = std::env::temp_dir().join(format!("studio-lib-manage-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("shoot")).unwrap();
        (Store::open(&d.join("a.db")).unwrap(), d)
    }

    /// A project with two pictures, one of them moved away afterwards.
    fn project_with_moved_file(tag: &str) -> (Store, std::path::PathBuf, crate::doc::Doc, PathBuf) {
        let (store, d) = scratch(tag);
        let (keep, gone) = (d.join("shoot/keep.png"), d.join("shoot/gone.png"));
        png(&keep, 1);
        png(&gone, 2);
        let mut doc = crate::services::project::new_custom("t", 640, 360, 30.0);
        crate::services::project::import_media(&mut doc, &[keep.clone(), gone.clone()]);
        assert_eq!(doc.model.media.len(), 2);
        store.sync_model(&d, &doc.model).unwrap();
        std::fs::create_dir_all(d.join("elsewhere")).unwrap();
        let moved = d.join("elsewhere/gone.png");
        std::fs::rename(&gone, &moved).unwrap();
        (store, d, doc, moved)
    }

    #[test]
    fn relink_previews_applies_in_one_step_and_takes_back() {
        let (store, d, mut doc, moved) = project_with_moved_file("relink");
        let mut st = State::default();
        reload_with(&mut st, &store, &doc, Some(&d)).unwrap();
        assert_eq!(st.missing.len(), 1, "the moved file is missing");
        // Preview: found, project untouched.
        let mut plan = find_candidates(&store, Some(&d), &[d.join("elsewhere")]).unwrap();
        assert_eq!((plan.wanted, plan.found.len()), (1, 1));
        assert_eq!(plan.found[0].to, moved);
        assert!(doc.model.media.iter().all(|m| !m.path.contains("elsewhere")));
        // Apply: one undo step, project and library follow.
        let (text, undo) = relink_now(&mut doc, &store, &mut plan).unwrap();
        assert_eq!(text, "Relinked 1 file");
        assert_eq!(doc.undo_label(), Some("Relink media"));
        assert!(doc.model.media.iter().any(|m| m.path == moved.display().to_string()));
        assert!(manage::missing(&store, Some(&d), false).unwrap().is_empty());
        // Undo puts both back.
        let back = take_back(&mut doc, &store, &undo).unwrap();
        assert!(back.starts_with("Took back the relink"), "{back}");
        assert!(doc.model.media.iter().all(|m| !m.path.contains("elsewhere")));
        assert_eq!(manage::missing(&store, Some(&d), false).unwrap().len(), 1);
        // Nothing found in an empty folder.
        std::fs::create_dir_all(d.join("empty")).unwrap();
        assert!(find_candidates(&store, Some(&d), &[d.join("empty")]).unwrap().found.is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn merging_duplicates_repoints_the_project_and_undo_restores() {
        let (store, d) = scratch("dups");
        let (a, b) = (d.join("shoot/a.png"), d.join("shoot/a copy.png"));
        png(&a, 3);
        std::fs::copy(&a, &b).unwrap();
        let mut doc = crate::services::project::new_custom("t", 640, 360, 30.0);
        crate::services::project::import_media(&mut doc, &[b.clone()]);
        store.add_file(&a, None).unwrap();
        store.sync_model(&d, &doc.model).unwrap();
        let groups = manage::duplicates(&store, false).unwrap();
        assert_eq!(groups.len(), 1);
        let keep = groups[0].iter().find(|x| x.path == a.display().to_string()).unwrap().id;
        let (text, undo) = keep_one(&mut doc, &store, &groups[0], keep).unwrap();
        assert_eq!(text, "Kept a.png and merged 1 copies");
        assert_eq!(doc.model.media[0].path, a.display().to_string(), "the project follows the kept file");
        assert!(manage::duplicates(&store, false).unwrap().is_empty());
        take_back(&mut doc, &store, &undo).unwrap();
        assert_eq!(doc.model.media[0].path, b.display().to_string());
        assert_eq!(manage::duplicates(&store, false).unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn bins_and_licences_report_and_take_back() {
        let (store, d) = scratch("binlic");
        let f = d.join("shoot/clip.png");
        png(&f, 4);
        let id = store.add_file(&f, None).unwrap();
        let asset = store.get(id).unwrap();
        let mut doc = crate::services::project::new_custom("t", 640, 360, 30.0);
        let (text, undo) = move_to_bin(&store, "Footage/Day 1", std::slice::from_ref(&asset)).unwrap();
        assert_eq!(text, "Moved clip.png to Footage/Day 1");
        take_back(&mut doc, &store, &undo).unwrap();
        assert!(manage::bin_contents(&store, "Footage/Day 1", 10).unwrap().is_empty());
        move_to_bin(&store, "Footage/Day 1", std::slice::from_ref(&asset)).unwrap();
        let (text, undo) = take_out(&store, "Footage/Day 1", &asset).unwrap();
        assert_eq!(text, "Took clip.png out of Footage/Day 1");
        take_back(&mut doc, &store, &undo).unwrap();
        assert_eq!(manage::bin_contents(&store, "Footage/Day 1", 10).unwrap().len(), 1);
        crate::rights::set(&store, id, &json!({"licence": "licensed", "expires": "2020-01-01"})).unwrap();
        let e = manage::expiring(&store, 60).unwrap().remove(0);
        let (text, undo) = renew(&store, &e, "2099-01-01").unwrap();
        assert_eq!(text, "Renewed clip.png until 2099-01-01");
        assert!(manage::expiring(&store, 60).unwrap().is_empty());
        take_back(&mut doc, &store, &undo).unwrap();
        assert_eq!(manage::expiring(&store, 60).unwrap().len(), 1);
        let (text, _) = remove_expiry(&store, &e).unwrap();
        assert_eq!(text, "Removed the end date from clip.png");
        assert!(manage::expiring(&store, 60).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Draws one frame of the sheet on the CPU, no window.
    fn frame(h: &mut crate::ui_tests::Harness, store: &Store, tags: &mut Vec<String>) -> egui::FullOutput {
        let raw = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(760.0, 520.0))), ..Default::default() };
        let (ctx, app) = (h.ctx.clone(), &mut h.app);
        let out = ctx.run_ui(raw, |ui| {
            egui::CentralPanel::default().show_inside(ui, |ui| {
                banner(ui, app);
                show_with(ui, app, store);
            });
        });
        if let Some(u) = out.platform_output.accesskit_update.as_ref() {
            tags.extend(u.nodes.iter().filter_map(|(_, n)| n.label().map(str::to_owned)));
        }
        out
    }

    #[test]
    fn each_sub_view_draws_with_its_rows_and_actions() {
        let (store, d, doc, _moved) = project_with_moved_file("frames");
        let mut h = crate::ui_tests::Harness::new(&[]).sized(760.0, 520.0);
        h.app.doc = doc;
        h.app.doc.folder = None;
        crate::rights::set(&store, store.add_file(&d.join("shoot/keep.png"), None).unwrap(), &json!({"licence": "licensed", "expires": "2020-01-01"})).unwrap();
        store.bin_create("Footage/Day 1", None).unwrap();
        store.bin_add("Footage/Day 1", &[store.add_file(&d.join("shoot/keep.png"), None).unwrap()]).unwrap();
        let mut seen: Vec<(usize, Vec<String>)> = Vec::new();
        for view in [BINS, DUPLICATES, MISSING, LICENCES] {
            let mut st: State = h.ctx.data_mut(|x| x.get_temp(state_id())).unwrap_or_default();
            st.view = view;
            st.loaded = false;
            h.ctx.data_mut(|x| x.insert_temp(state_id(), st));
            let mut tags = Vec::new();
            for _ in 0..3 {
                tags.clear();
                let out = frame(&mut h, &store, &mut tags);
                assert!(!out.shapes.is_empty());
            }
            seen.push((view, tags));
        }
        let has = |view: usize, text: &str| seen.iter().find(|s| s.0 == view).unwrap().1.iter().any(|l| l.contains(text));
        for (view, label) in VIEWS.iter().enumerate() {
            assert!(has(view, label), "the {label} segment is there in view {view}");
        }
        assert!(has(BINS, "In this project") && has(BINS, "Footage/Day 1"));
        assert!(has(DUPLICATES, "No duplicates"));
        assert!(has(MISSING, "Relink missing…") && has(MISSING, "Missing (1)"));
        assert!(has(LICENCES, "keep.png"));
        // Offline media shows the banner.
        assert!(has(MISSING, "offline"), "the banner appears while media is offline");
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/ui-shots");
        std::fs::create_dir_all(&dir).unwrap();
        h.with_events(vec![]);
        h.shot(dir.join("library-manage.png"));
        let _ = std::fs::remove_dir_all(&d);
    }
}
