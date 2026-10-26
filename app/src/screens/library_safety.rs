// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Library, Safety and Rights in one sheet: search the whole media library
//! (by name, label, spoken words, what pictures show), rate and tag, look
//! at the project's health, go back through its history, back up, collect
//! and clean up, and see which licences stand in the way of an export. The
//! same functions the AI uses (`assets`, `project_care`).

use crate::app::{Overlay, StudioApp};
use crate::assets::{self, Query};
use crate::icons::{self, Icon};
use crate::kit::{self, Button};
use crate::{rights, safety, theme};
use eframe::egui::{self, Align2, Rect, Sense, Vec2};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use crate::kit::ShowNoted;

#[derive(Clone, Default)]
struct Row {
    id: i64,
    name: String,
    kind: String,
    seconds: f64,
    rating: i64,
    fav: bool,
    missing: bool,
    matched: String,
    licence: String,
}

#[derive(Clone, Default)]
struct State {
    tab: usize,
    query: String,
    asked: Option<(String, usize, i64, bool)>,
    kind: usize,
    stars: i64,
    fav_only: bool,
    rows: Vec<Row>,
    selected: Option<i64>,
    label: String,
    total: i64,
    health: Option<Value>,
    history: Vec<(String, String, String)>,
    rights: Option<Value>,
    clean: Option<Value>,
    note: String,
    /// Long work (backup, collect) reports here.
    work: Arc<Mutex<Option<String>>>,
    busy: bool,
}

const KINDS: [&str; 4] = ["All", "Video", "Image", "Audio"];
const TABS: [&str; 3] = ["Library", "Safety", "Rights"];

pub fn show(app: &mut StudioApp, ctx: &egui::Context) {
    let screen = ctx.content_rect();
    let w = (screen.width() * 0.5).clamp(640.0, 980.0).min(screen.width() - 24.0);
    let rect = Rect::from_min_max(egui::pos2(screen.center().x - w / 2.0, screen.top() + crate::editor::TOPBAR_H - 20.0), egui::pos2(screen.center().x + w / 2.0, screen.bottom()));
    let id = egui::Id::new("library-safety-state");
    let mut st: State = ctx.data_mut(|d| d.get_temp(id)).unwrap_or_default();
    let out = kit::sheet(ctx, egui::Id::unique("library-safety"), rect, |ui| body(app, ui, &mut st));
    if st.busy {
        if let Some(done) = st.work.lock().unwrap_or_else(|e| e.into_inner()).take() {
            st.note = done;
            st.busy = false;
            st.health = None;
            st.history.clear();
        } else {
            ctx.request_repaint_after(std::time::Duration::from_millis(400));
        }
    }
    ctx.data_mut(|d| d.insert_temp(id, st));
    if out.close {
        app.overlay = Overlay::None;
    }
}

fn root(app: &StudioApp) -> Option<std::path::PathBuf> {
    app.doc.folder.as_ref().map(|f| f.root().to_path_buf())
}

fn body(app: &mut StudioApp, ui: &mut egui::Ui, st: &mut State) {
    let t = theme::tokens(ui.ctx());
    let r = ui.max_rect();
    let mut head = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(r.min + Vec2::new(20.0, 16.0), Vec2::new(r.width() - 80.0, 32.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
    for (i, name) in TABS.iter().enumerate() {
        let b = if st.tab == i { Button::primary(name) } else { Button::ghost(name) };
        if b.compact().show(&mut head).clicked() {
            st.tab = i;
        }
    }
    let inner = Rect::from_min_max(egui::pos2(r.left() + 20.0, r.top() + 60.0), egui::pos2(r.right() - 20.0, r.bottom() - 16.0));
    let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(inner));
    let ui = &mut ui;
    if !st.note.is_empty() {
        ui.label(egui::RichText::new(&st.note).font(theme::body()).color(t.text_2));
        ui.add_space(6.0);
    }
    match st.tab {
        0 => library(app, ui, st),
        1 => safety_tab(app, ui, st),
        _ => rights_tab(app, ui, st),
    }
}

fn library(app: &mut StudioApp, ui: &mut egui::Ui, st: &mut State) {
    let t = theme::tokens(ui.ctx());
    let store = match assets::global() {
        Ok(s) => s,
        Err(e) => {
            ui.label(e);
            return;
        }
    };
    ui.horizontal(|ui| {
        kit::text_field(ui, &mut st.query, "Search names, tags, words said, what the picture shows", Some(Icon::MagnifyingGlass), ui.available_width() - 190.0, None);
        if Button::secondary("Look into footage").compact().busy(st.busy).show(ui).on_hover_text("Describes frames, groups faces and gives every picture and sound a signature so search and \"more like this\" can see inside. Runs here, in the background.").clicked() {
            if let Some(root) = root(app) {
                crate::agent::care::sync_and_index(root, app.doc.model.clone());
                st.note = "Looking into this project's media in the background.".into();
            } else {
                st.note = "Save the project first.".into();
            }
        }
    });
    ui.horizontal(|ui| {
        for (i, k) in KINDS.iter().enumerate() {
            let b = if st.kind == i { Button::primary(k) } else { Button::ghost(k) };
            if b.compact().show(ui).clicked() {
                st.kind = i;
            }
        }
        if (if st.stars > 0 { Button::primary("4+ stars") } else { Button::ghost("4+ stars") }).compact().show(ui).clicked() {
            st.stars = if st.stars > 0 { 0 } else { 4 };
        }
        if (if st.fav_only { Button::primary("Favourites") } else { Button::ghost("Favourites") }).compact().show(ui).clicked() {
            st.fav_only = !st.fav_only;
        }
    });
    ui.add_space(6.0);
    // Ask again when the question changed, or after a change made here.
    let ask = (st.query.clone(), st.kind, st.stars, st.fav_only);
    if st.asked.as_ref() != Some(&ask) {
        let mut q = Query::from_json(&json!({"query": st.query, "limit": 200, "min_rating": st.stars}));
        if st.kind > 0 {
            q.kinds = vec![KINDS[st.kind].to_lowercase()];
        }
        if st.fav_only {
            q.favourite = Some(true);
        }
        if let Some(root) = root(app) {
            let _ = store.sync_model(&root, &app.doc.model);
        }
        st.rows = store
            .find(&q)
            .unwrap_or_default()
            .into_iter()
            .map(|h| Row {
                id: h.asset.id,
                name: h.asset.name.clone(),
                kind: h.asset.kind.clone(),
                seconds: h.asset.dur,
                rating: h.asset.rating,
                fav: h.asset.fav,
                missing: h.asset.missing,
                matched: h.why.first().map(|w| format!("{}: {}", w.0, w.3)).unwrap_or_default(),
                licence: rights::get(store, h.asset.id).ok().flatten().map(|r| r.licence).unwrap_or_else(|| "unknown".into()),
            })
            .collect();
        st.total = store.counts().ok().and_then(|c| c["assets"].as_i64()).unwrap_or(0);
        st.asked = Some(ask);
    }
    ui.label(egui::RichText::new(format!("{} shown of {} in the library", st.rows.len(), st.total)).font(theme::caption()).color(t.text_2));
    let avail = ui.available_height() - 52.0;
    let mut change = false;
    let mut more_like: Option<i64> = None;
    egui::ScrollArea::vertical().max_height(avail.max(120.0)).auto_shrink([false, false]).show_v(ui, |ui| {
        for row in st.rows.clone() {
            let (rr, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 44.0), Sense::click());
            let sel = st.selected == Some(row.id);
            if sel || resp.hovered() {
                ui.painter().rect_filled(rr, 6, if sel { t.accent_soft } else { t.raised });
            }
            if resp.clicked() {
                st.selected = Some(row.id);
            }
            let name_col = if row.missing { t.danger } else { t.text };
            kit::paint_text_clipped(ui.painter(), egui::pos2(rr.left() + 10.0, rr.top() + 14.0), Align2::LEFT_CENTER, &row.name, theme::body_medium(), name_col, rr.width() - 330.0);
            let sub = if row.matched.is_empty() { format!("{}{} · {}", row.kind, if row.seconds > 0.0 { format!(" · {:.1}s", row.seconds) } else { String::new() }, row.licence) } else { row.matched.replace(['[', ']'], "") };
            kit::paint_text_clipped(ui.painter(), egui::pos2(rr.left() + 10.0, rr.top() + 31.0), Align2::LEFT_CENTER, &sub, theme::caption(), t.text_2, rr.width() - 330.0);
            // Stars: click one to rate, click the same again to clear.
            for s in 1..=5 {
                let c = egui::pos2(rr.right() - 300.0 + s as f32 * 18.0, rr.center().y);
                let box_ = Rect::from_center_size(c, Vec2::splat(18.0));
                let on = row.rating >= s;
                let sr = ui.interact(box_, ui.id().with(("star", row.id, s)), Sense::click());
                if on {
                    icons::paint_fill(ui.painter(), box_, Icon::Star, 14.0, t.accent);
                } else {
                    icons::paint(ui.painter(), box_, Icon::Star, 14.0, if sr.hovered() { t.accent } else { t.text_2 });
                }
                if sr.clicked() {
                    let _ = store.set_rating(row.id, if row.rating == s { 0 } else { s });
                    change = true;
                }
            }
            let fav = Rect::from_center_size(egui::pos2(rr.right() - 190.0, rr.center().y), Vec2::new(40.0, 22.0));
            let fr = ui.interact(fav, ui.id().with(("fav", row.id)), Sense::click());
            ui.painter().text(fav.center(), Align2::CENTER_CENTER, if row.fav { "Fav ✓" } else { "Fav" }, theme::caption(), if row.fav { t.accent } else { t.text_2 });
            if fr.clicked() {
                let _ = store.set_favourite(row.id, !row.fav);
                change = true;
            }
            let like = Rect::from_center_size(egui::pos2(rr.right() - 100.0, rr.center().y), Vec2::new(110.0, 22.0));
            let lr = ui.interact(like, ui.id().with(("like", row.id)), Sense::click());
            ui.painter().text(like.center(), Align2::CENTER_CENTER, crate::i18n::shown("More like this"), theme::caption(), if lr.hovered() { t.accent } else { t.text_2 });
            if lr.clicked() {
                more_like = Some(row.id);
            }
        }
        if st.rows.is_empty() {
            ui.label(egui::RichText::new("Nothing matches. \"Look into footage\" lets search see inside pictures, clips and sounds.").font(theme::body()).color(t.text_2));
        }
    });
    if let Some(id) = more_like {
        match store.similar(id, 40) {
            Ok(like) => {
                let name = store.get(id).map(|a| a.name).unwrap_or_default();
                st.rows = like
                    .into_iter()
                    .map(|(a, s)| Row { id: a.id, name: a.name, kind: a.kind, seconds: a.dur, rating: a.rating, fav: a.fav, missing: a.missing, matched: format!("{:.0}% like {name}", s * 100.0), licence: rights::get(store, a.id).ok().flatten().map(|r| r.licence).unwrap_or_else(|| "unknown".into()) })
                    .collect();
                st.asked = Some(("\u{1}".into(), 0, 0, false));
                st.note = crate::i18n::tf(format!("Showing what is like {name}."));
            }
            Err(e) => st.note = e,
        }
    }
    if let Some(id) = st.selected {
        ui.horizontal(|ui| {
            kit::text_field(ui, &mut st.label, "Add a tag, or place:Goa, person:Asha", Some(Icon::Sparkle), 320.0, None);
            if Button::secondary("Add").compact().show(ui).clicked() && !st.label.trim().is_empty() {
                let (kind, value) = match st.label.split_once(':') {
                    Some((k @ ("place" | "person" | "keyword" | "tag"), v)) => (k.to_owned(), v.trim().to_owned()),
                    _ => ("tag".to_owned(), st.label.trim().to_owned()),
                };
                if let Err(e) = store.label(id, &kind, &[value]) {
                    st.note = e;
                }
                st.label.clear();
                change = true;
            }
        });
    }
    if change {
        st.asked = None;
    }
}

fn safety_tab(app: &mut StudioApp, ui: &mut egui::Ui, st: &mut State) {
    let t = theme::tokens(ui.ctx());
    let Some(root) = root(app) else {
        ui.label(egui::RichText::new(crate::i18n::shown("Save the project first; safety works on its folder.")).font(theme::body()).color(t.text_2));
        return;
    };
    if st.health.is_none() {
        st.health = Some(safety::health(&root, None));
        st.history = safety::history(&root).into_iter().take(30).map(|s| (s.path.display().to_string(), safety::when(s.at_ms), format!("{}{}", s.kind, if s.label.is_empty() { String::new() } else { format!(": {}", s.label) }))).collect();
    }
    let h = st.health.clone().unwrap_or(Value::Null);
    let status = h["status"].as_str().unwrap_or("");
    ui.horizontal_wrapped(|ui| {
        ui.label(egui::RichText::new(crate::i18n::tf(format!("Project is {status}"))).font(theme::body_medium()).color(if status == "healthy" { t.accent } else { t.warn }));
        ui.label(egui::RichText::new(format!("· {} media, {} snapshots, {} free", h["media"]["files"], h["snapshots"]["total"], h["disk_free"].as_str().unwrap_or("?"))).font(theme::caption()).color(t.text_2));
    });
    for i in h["issues"].as_array().cloned().unwrap_or_default() {
        ui.label(egui::RichText::new(format!("• {}  —  {}", i["message"].as_str().unwrap_or(""), i["fix"].as_str().unwrap_or(""))).font(theme::caption()).color(t.text_2));
    }
    ui.add_space(8.0);
    ui.horizontal_wrapped(|ui| {
        if Button::secondary("Keep a snapshot").compact().show(ui).clicked() {
            match safety::keep_snapshot(&root, &app.doc.model, "kept in the app") {
                Ok(_) => {
                    st.note = "Snapshot kept. Autosave never removes it.".into();
                    st.health = None;
                }
                Err(e) => st.note = e,
            }
        }
        let busy = st.busy;
        if Button::secondary("Back up to…").compact().busy(busy).show(ui).clicked() && !busy {
            if let Some(dest) = rfd::FileDialog::new().set_title("Back up to a folder or drive").pick_folder() {
                let (r2, w) = (root.clone(), st.work.clone());
                save_now(app);
                st.busy = true;
                st.note = "Backing up…".into();
                std::thread::spawn(move || {
                    let msg = match safety::backup(&r2, &dest, 5, true) {
                        Ok(b) => crate::i18n::tf(format!("Backed up to {}: {} media copied, {} already there.", dest.display(), b.copied, b.reused)),
                        Err(e) => crate::i18n::tf(format!("Backup failed: {e}")),
                    };
                    *w.lock().unwrap_or_else(|e| e.into_inner()) = Some(msg);
                });
            }
        }
        if Button::secondary("Collect project to…").compact().busy(busy).show(ui).clicked() && !busy {
            if let Some(dest) = rfd::FileDialog::new().set_title("Collect into a new folder").set_file_name(format!("{} collected", app.doc.project.name)).save_file() {
                let (r2, w) = (root.clone(), st.work.clone());
                save_now(app);
                st.busy = true;
                st.note = "Collecting…".into();
                std::thread::spawn(move || {
                    let msg = match safety::collect(&r2, &dest, &safety::CollectOptions { checksums: true, ..Default::default() }) {
                        Ok(c) => crate::i18n::tf(format!("Collected {} files ({}) into {}.", c.files, safety::human(c.bytes), dest.display())),
                        Err(e) => crate::i18n::tf(format!("Could not collect: {e}")),
                    };
                    *w.lock().unwrap_or_else(|e| e.into_inner()) = Some(msg);
                });
            }
        }
        if Button::secondary("Check again").compact().show(ui).clicked() {
            st.health = None;
        }
    });
    ui.add_space(6.0);
    ui.horizontal_wrapped(|ui| {
        if Button::ghost("What can be cleaned?").compact().show(ui).clicked() {
            let o = safety::CleanOptions::default();
            st.clean = Some(safety::clean_plan(&root, &o).json(5));
        }
        if let Some(c) = st.clean.clone() {
            ui.label(egui::RichText::new(crate::i18n::tf(format!("Would free {} ({} files). Never media, never anything a project uses.", c["would_free"].as_str().unwrap_or("0"), c["files"]))).font(theme::caption()).color(t.text_2));
            if c["files"].as_u64().unwrap_or(0) > 0 && Button::secondary("Clean now").compact().show(ui).clicked() {
                let o = safety::CleanOptions::default();
                let plan = safety::clean_plan(&root, &o);
                st.note = match safety::clean_apply(&root, &o, &plan) {
                    Ok(freed) => crate::i18n::tf(format!("Freed {}.", safety::human(freed))),
                    Err(e) => e,
                };
                st.clean = None;
                st.health = None;
            }
        }
    });
    ui.add_space(8.0);
    ui.label(egui::RichText::new(crate::i18n::shown("History")).font(theme::body_medium()).color(t.text));
    let mut restore: Option<String> = None;
    egui::ScrollArea::vertical().auto_shrink([false, false]).show_v(ui, |ui| {
        for (path, at, what) in &st.history {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(at).font(theme::caption()).color(t.text_2));
                // Restore keeps its place at the end; a long description
                // ends in an ellipsis and shows whole on hover.
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if Button::ghost("Restore").compact().show(ui).clicked() {
                        restore = Some(path.clone());
                    }
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| kit::label_trunc(ui, egui::RichText::new(what).font(theme::body()).color(t.text)));
                });
            });
        }
        if st.history.is_empty() {
            ui.label(egui::RichText::new(crate::i18n::shown("No snapshots yet. Autosave writes one 30 seconds after a change.")).font(theme::caption()).color(t.text_2));
        }
    });
    if let Some(path) = restore {
        let snap = safety::history(&root).into_iter().find(|s| s.path.display().to_string() == path);
        if let Some(snap) = snap {
            save_now(app);
            match safety::restore(&root, &snap) {
                Ok(_) => {
                    let ctx = ui.ctx().clone();
                    app.open_folder(&ctx, Some(root.clone()));
                    st.note = "Restored. The project as it was is kept as a restore point in the history.".into();
                    st.health = None;
                }
                Err(e) => st.note = e,
            }
        }
    }
}

fn rights_tab(app: &mut StudioApp, ui: &mut egui::Ui, st: &mut State) {
    let t = theme::tokens(ui.ctx());
    let Some(root) = root(app) else {
        ui.label(egui::RichText::new(crate::i18n::shown("Save the project first.")).font(theme::body()).color(t.text_2));
        return;
    };
    if st.rights.is_none() {
        st.rights = Some(match assets::global().and_then(|s| rights::check(s, &root, &app.doc.model, &rights::Options::default())) {
            Ok(r) => r.json(),
            Err(e) => json!({"error": e}),
        });
    }
    let r = st.rights.clone().unwrap_or(Value::Null);
    if let Some(e) = r["error"].as_str() {
        ui.label(e);
        return;
    }
    ui.horizontal_wrapped(|ui| {
        let ok = r["blocked"].as_u64().unwrap_or(0) == 0;
        ui.label(egui::RichText::new(if ok { "Nothing stands in the way of an export" } else { "Some assets cannot be used as they are" }).font(theme::body_medium()).color(if ok { t.accent } else { t.danger }));
        ui.label(egui::RichText::new(format!("· {} assets on the timeline, {} warnings", r["used_assets"], r["warnings"])).font(theme::caption()).color(t.text_2));
        if Button::secondary("Check again").compact().show(ui).clicked() {
            st.rights = None;
        }
    });
    ui.add_space(6.0);
    // The hint under the list keeps its room: the list scrolls above it.
    let hint = "Record a licence by asking the AI (\"this track is CC BY from incompetech, credit Kevin MacLeod\") or with the assets tool.";
    let hint_h = ui.painter().layout(crate::i18n::shown(hint).into_owned(), theme::caption(), t.text_2, ui.available_width()).size().y + ui.spacing().item_spacing.y;
    egui::ScrollArea::vertical().max_height((ui.available_height() - hint_h).max(60.0)).auto_shrink([false, false]).show_v(ui, |ui| {
        for f in r["findings"].as_array().cloned().unwrap_or_default() {
            let col = if f["level"] == "blocked" { t.danger } else { t.warn };
            ui.label(egui::RichText::new(format!("{}: {}", f["asset"].as_str().unwrap_or(""), f["message"].as_str().unwrap_or(""))).font(theme::body()).color(col));
            ui.label(egui::RichText::new(format!("   {}", f["fix"].as_str().unwrap_or(""))).font(theme::caption()).color(t.text_2));
        }
        let credits: Vec<String> = r["credits"].as_array().cloned().unwrap_or_default().iter().filter_map(|c| c.as_str().map(str::to_owned)).collect();
        if !credits.is_empty() {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(crate::i18n::shown("Credits to show")).font(theme::body_medium()).color(t.text));
                if Button::ghost("Copy").icon(Icon::Copy).compact().show(ui).clicked() {
                    ui.ctx().copy_text(credits.join("\n"));
                    st.note = "Credit lines copied.".into();
                }
            });
            for c in &credits {
                ui.label(egui::RichText::new(c).font(theme::body()).color(t.text_2));
            }
        }
    });
    ui.label(egui::RichText::new(hint).font(theme::caption()).color(t.text_2));
}

/// Saves the project to its folder, quietly (before a backup or restore).
fn save_now(app: &mut StudioApp) {
    if let Some(f) = app.doc.folder.clone() {
        if f.save(&app.doc.model).is_ok() {
            app.doc.history.mark_saved();
        }
    }
}

/// Settings ▸ Storage ▸ Clean up: removes what is safe to remove for the open project.
pub fn clean_for(app: &mut StudioApp, ctx: &egui::Context) {
    let Some(root) = root(app) else {
        kit::push(ctx, "Open or save a project first; cleaning works on its folder", None, Some(Icon::Info));
        return;
    };
    let mut o = safety::CleanOptions::default();
    o.other_projects = std::fs::read_dir(studio_paths::studio().join("Projects")).map(|r| r.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.join("project.json").is_file() && *p != root).collect()).unwrap_or_default();
    let plan = safety::clean_plan(&root, &o);
    match safety::clean_apply(&root, &o, &plan) {
        Ok(freed) => kit::push(ctx, crate::i18n::tf(format!("Cleaned up {}", safety::human(freed))), None, Some(Icon::CheckCircle)),
        Err(e) => kit::push(ctx, e, None, Some(Icon::Warning)),
    };
}

/// What stops an export, worded for the export sheet: `None` when nothing does.
pub fn export_block(doc: &crate::doc::Doc) -> Option<String> {
    let root = doc.folder.as_ref()?.root().to_path_buf();
    let store = assets::global().ok()?;
    let rep = rights::check(store, &root, &doc.model, &rights::Options::default()).ok()?;
    if rep.blocked() == 0 {
        return None;
    }
    Some(rep.findings.iter().filter(|f| f.level == "blocked").map(|f| format!("{}: {}", f.asset, f.message)).collect::<Vec<_>>().join("; "))
}
