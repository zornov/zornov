// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The Review panel: notes, versions, the team and sharing, in a sheet on
//! the right of the editor; notes also show as markers in the timeline's
//! ruler and as boxes on the picture.
//!
//! Everything is read from and written to the project folder through
//! [`crate::collab`], so what the AI editor (or a sync) writes shows up
//! here within a second, and what is written here the AI reads.

use crate::app::StudioApp;
use crate::collab::{self, comments, delta, package, sync, team, versions};
use crate::icons::Icon;
use crate::kit::{self, Button};
use crate::theme;
use crate::view::{self, SEC, Time};
use eframe::egui::{self, CornerRadius, Pos2, Rect, Sense, Stroke, Vec2};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use studio_model as m;
use crate::kit::ShowNoted;

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Tab {
    #[default]
    Notes,
    Versions,
    Team,
    Share,
}

#[derive(Default)]
struct ReviewUi {
    open: bool,
    tab: Tab,
    root: Option<PathBuf>,
    store: comments::Store,
    index: versions::Index,
    team: team::Team,
    /// Modified times of the three files when last read.
    stamps: [Option<std::time::SystemTime>; 3],
    checked: f64,
    show_resolved: bool,
    draft: String,
    pin_clip: bool,
    region: Option<[f32; 4]>,
    drawing: Option<[f32; 2]>,
    draw_mode: bool,
    reply: String,
    selected: Option<String>,
    version_name: String,
    version_note: String,
    compare: Option<(String, String)>,
    member: String,
    role: usize,
    folder: String,
    path: String,
    status: Option<String>,
}

thread_local! {
    static UI: RefCell<ReviewUi> = RefCell::new(ReviewUi::default());
}

fn with<T>(f: impl FnOnce(&mut ReviewUi) -> T) -> T {
    UI.with(|u| f(&mut u.borrow_mut()))
}

fn stamp(p: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

fn files(root: &Path) -> [PathBuf; 3] {
    [comments::file(root), m::file::ProjectFolder::new(root).review_versions_dir().join("index.json"), team::file(root)]
}

/// Every frame: follows the project folder (reloading what changed on
/// disk) and keeps the document's team rules current.
pub fn tick(app: &mut StudioApp, ctx: &egui::Context) {
    let root = app.doc.folder.as_ref().map(|f| f.root().to_path_buf());
    let now = ctx.input(|i| i.time);
    with(|u| {
        let moved = u.root != root;
        if !moved && now - u.checked < 1.0 {
            return;
        }
        u.checked = now;
        if moved {
            u.root = root.clone();
            u.stamps = [None; 3];
            u.store = Default::default();
            u.index = Default::default();
            u.team = Default::default();
            u.selected = None;
        }
        let Some(root) = &root else {
            app.doc.guard = None;
            return;
        };
        let f = files(root);
        let s = [stamp(&f[0]), stamp(&f[1]), stamp(&f[2])];
        if s[0] != u.stamps[0] || moved {
            u.store = comments::Store::load(root).unwrap_or_default();
        }
        if s[1] != u.stamps[1] || moved {
            u.index = versions::Index::load(root).unwrap_or_default();
        }
        if s[2] != u.stamps[2] || moved {
            u.team = team::Team::load(root).unwrap_or_default();
            app.doc.guard = u.team.guard_for(&collab::identity());
        }
        u.stamps = s;
    });
}

/// The top bar button, with the count of open notes.
pub fn topbar_button(ui: &mut egui::Ui) {
    let (open, n) = with(|u| (u.open, u.store.open_count()));
    let label = if n > 0 { format!("Review · {n}") } else { "Review".to_owned() };
    let resp = Button::secondary(&label).compact().show(ui);
    if resp.clicked() {
        with(|u| u.open = !open);
    }
}

fn save_store(u: &mut ReviewUi) {
    if let Some(root) = &u.root {
        if let Err(e) = u.store.save(root) {
            u.status = Some(e);
        }
        u.stamps[0] = stamp(&comments::file(root));
    }
}

fn save_team(app: &mut StudioApp, u: &mut ReviewUi) {
    if let Some(root) = u.root.clone() {
        if let Err(e) = u.team.save(&root) {
            u.status = Some(e);
        }
        u.stamps[2] = stamp(&team::file(&root));
        app.doc.guard = u.team.guard_for(&collab::identity());
    }
}

/// Whether the Review panel is open (docked or in its own place).
pub fn is_open() -> bool {
    with(|u| u.open)
}

pub fn set_open(open: bool) {
    with(|u| u.open = open);
}

/// The panel docked in the editor, in `r`.
pub fn docked(app: &mut StudioApp, ui: &mut egui::Ui, r: Rect) {
    let inner = r.shrink2(Vec2::new(16.0, 12.0));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(egui::Layout::top_down(egui::Align::Min)).id_salt("review-docked"));
    child.set_clip_rect(r.intersect(ui.clip_rect()));
    body(app, &mut child);
}

/// The panel itself, when open and not docked: along the window's right
/// edge, over the editor.
pub fn show(app: &mut StudioApp, ctx: &egui::Context) {
    if !with(|u| u.open) || super::dock_ui::docked(ctx, crate::dock::Panel::Review) {
        return;
    }
    let screen = ctx.content_rect();
    let w = 380.0f32.min(screen.width() - 32.0);
    let rect = Rect::from_min_max(egui::pos2(screen.right() - w, screen.top() + super::TOPBAR_H), screen.max);
    let t = theme::tokens(ctx);
    // No backdrop: the timeline and the picture stay live beside it (note
    // markers, drawing a region).
    egui::Area::new(egui::Id::unique("review-panel")).order(egui::Order::Foreground).fixed_pos(rect.min).show(ctx, |ui| {
        let (r, _) = ui.allocate_exact_size(rect.size(), Sense::click());
        let p = ui.painter();
        p.add(theme::popover_shadow().as_shape(r, CornerRadius::ZERO));
        p.rect_filled(r, 0, t.panel);
        p.vline(r.left(), r.y_range(), Stroke::new(1.0, t.line));
        let close = Rect::from_center_size(egui::pos2(r.right() - 22.0, r.top() + 22.0), Vec2::splat(24.0));
        let inner = r.shrink2(Vec2::new(16.0, 12.0));
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(egui::Layout::top_down(egui::Align::Min)));
        child.set_clip_rect(r);
        body(app, &mut child);
        // Drag onto the editor to dock it.
        let grip = Rect::from_center_size(close.center() - Vec2::new(28.0, 0.0), Vec2::splat(22.0));
        let mut gui = ui.new_child(egui::UiBuilder::new().max_rect(grip));
        super::dock_ui::float_grip(app, &mut gui, crate::dock::Panel::Review);
        if kit::IconButton::new(Icon::X, "Close").size(24.0, 14.0).show_at(ui, close).clicked() {
            with(|u| u.open = false);
        }
    });
}

fn body(app: &mut StudioApp, ui: &mut egui::Ui) {
    let t = theme::tokens(ui.ctx());
    ui.label(egui::RichText::new("Review").font(theme::body_medium()).color(t.text));
    ui.add_space(6.0);
    let has_root = with(|u| u.root.is_some());
    if !has_root {
        ui.label(egui::RichText::new("Save the project to start reviewing: notes, versions and the team are kept in its folder.").color(t.text_2));
        return;
    }
    kit::scroll_strip(ui, "review-tabs", |ui| {
        for (tab, name) in [(Tab::Notes, "Notes"), (Tab::Versions, "Versions"), (Tab::Team, "Team"), (Tab::Share, "Share")] {
            let on = with(|u| u.tab == tab);
            if ui.selectable_label(on, name).clicked() {
                with(|u| u.tab = tab);
            }
        }
    });
    ui.add_space(4.0);
    kit::hline(ui, ui.max_rect().left()..=ui.max_rect().right(), ui.cursor().top());
    ui.add_space(8.0);
    if let Some(s) = with(|u| u.status.clone()) {
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new(&s).color(t.text_2).font(theme::caption()));
            if ui.small_button("×").clicked() {
                with(|u| u.status = None);
            }
        });
    }
    egui::ScrollArea::vertical().auto_shrink([false, false]).show_v(ui, |ui| match with(|u| u.tab) {
        Tab::Notes => notes_tab(app, ui),
        Tab::Versions => versions_tab(app, ui),
        Tab::Team => team_tab(app, ui),
        Tab::Share => share_tab(app, ui),
    });
}

fn notes_tab(app: &mut StudioApp, ui: &mut egui::Ui) {
    let t = theme::tokens(ui.ctx());
    let me = collab::identity();
    let fps = app.doc.project.fps;
    let playhead = app.editor.playhead;
    let selected_clip = app.editor.selection.first().copied();

    // A new note at the playhead.
    ui.label(egui::RichText::new(format!("New note at {}", view::timecode(playhead, fps))).font(theme::caption()).color(t.text_2));
    let mut draft = with(|u| std::mem::take(&mut u.draft));
    let resp = ui.add(egui::TextEdit::multiline(&mut draft).desired_rows(2).desired_width(f32::INFINITY).hint_text("What should change here?"));
    let enter = resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter) && i.modifiers.command);
    with(|u| u.draft = draft);
    ui.horizontal_wrapped(|ui| {
        let mut pin = with(|u| u.pin_clip);
        ui.add_enabled_ui(selected_clip.is_some(), |ui| {
            ui.checkbox(&mut pin, "On the selected clip");
        });
        with(|u| u.pin_clip = pin);
        let (drawing, has_region) = with(|u| (u.draw_mode, u.region.is_some()));
        let label = if drawing { "Drag on the picture…" } else if has_region { "Region ✓" } else { "Mark a region" };
        if ui.selectable_label(drawing, label).clicked() {
            with(|u| {
                u.draw_mode = !u.draw_mode;
                if !u.draw_mode {
                    u.region = None;
                }
            });
        }
    });
    if Button::primary("Add note").compact().shortcut("⌘↩").show(ui).clicked() || enter {
        with(|u| {
            let text = u.draft.trim().to_owned();
            if text.is_empty() {
                return;
            }
            let clip = if u.pin_clip { selected_clip.map(|c| c as u64) } else { None };
            let version = u.index.current.clone();
            match u.store.add(&app.doc.model, &me, &text, comments::Pin { at: playhead, until: None, clip, region: u.region.take() }, version) {
                Ok(id) => {
                    u.draft.clear();
                    u.selected = Some(id);
                    u.draw_mode = false;
                    save_store(u);
                }
                Err(e) => u.status = Some(e),
            }
        });
    }
    ui.add_space(10.0);

    // The list.
    ui.horizontal_wrapped(|ui| {
        let n = with(|u| u.store.open_count());
        ui.label(egui::RichText::new(format!("{n} open")).color(t.text));
        let mut all = with(|u| u.show_resolved);
        ui.checkbox(&mut all, "Show resolved");
        with(|u| u.show_resolved = all);
    });
    ui.add_space(4.0);
    let notes: Vec<comments::Comment> = with(|u| {
        let f = if u.show_resolved { comments::Filter::All } else { comments::Filter::Open };
        u.store.list(&app.doc.model, f).into_iter().cloned().collect()
    });
    if notes.is_empty() {
        ui.label(egui::RichText::new("No notes yet. Notes made here, by the AI or by a reviewer's package show up in this list and on the timeline.").color(t.text_2));
    }
    for c in notes {
        let (at, gone) = c.time_in(&app.doc.model);
        let selected = with(|u| u.selected.as_deref() == Some(c.id.as_str()));
        let frame = egui::Frame::new().fill(if selected { t.accent_soft } else { t.raised }).corner_radius(CornerRadius::same(6)).inner_margin(8.0);
        let mut clicked = false;
        frame.show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                let colour = if c.status == comments::Status::Resolved { t.text_2 } else { t.accent };
                clicked |= ui.add(egui::Label::new(egui::RichText::new(view::timecode(at, fps)).font(theme::caption()).color(colour)).sense(Sense::click())).on_hover_text("Go there").clicked();
                ui.label(egui::RichText::new(&c.author).font(theme::caption()).color(t.text_2));
                if c.region.is_some() {
                    ui.label(egui::RichText::new("□").font(theme::caption()).color(t.text_2));
                }
                if let Some(id) = c.clip {
                    ui.label(egui::RichText::new(if gone { format!("clip #{id} (gone)") } else { format!("clip #{id}") }).font(theme::caption()).color(t.text_2));
                }
            });
            let text = if c.status == comments::Status::Resolved { egui::RichText::new(&c.text).strikethrough().color(t.text_2) } else { egui::RichText::new(&c.text).color(t.text) };
            clicked |= ui.add(egui::Label::new(text).sense(Sense::click())).clicked();
            for r in &c.replies {
                ui.label(egui::RichText::new(format!("↩ {}: {}", r.author, r.text)).font(theme::caption()).color(t.text_2));
            }
            if selected {
                note_actions(app, ui, &c, &me);
            }
        });
        if clicked {
            with(|u| u.selected = if selected { None } else { Some(c.id.clone()) });
            app.editor.playhead = at;
            app.editor.playing = false;
        }
        ui.add_space(4.0);
    }
    ui.add_space(8.0);
    ui.horizontal_wrapped(|ui| {
        for (label, fmt, ext) in [("Export CSV", "csv", "csv"), ("Markdown", "markdown", "md"), ("EDL markers", "edl", "edl")] {
            if ui.small_button(label).clicked() {
                export_notes(app, fmt, ext);
            }
        }
    });
}

fn note_actions(app: &mut StudioApp, ui: &mut egui::Ui, c: &comments::Comment, me: &str) {
    let mut reply = with(|u| std::mem::take(&mut u.reply));
    ui.add(egui::TextEdit::singleline(&mut reply).desired_width(f32::INFINITY).hint_text("Reply"));
    with(|u| u.reply = reply);
    ui.horizontal_wrapped(|ui| {
        if ui.small_button("Reply").clicked() {
            with(|u| {
                let text = std::mem::take(&mut u.reply);
                if let Err(e) = u.store.reply(&c.id, me, &text) {
                    u.status = Some(e);
                }
                save_store(u);
            });
        }
        let open = c.status == comments::Status::Open;
        if ui.small_button(if open { "Resolve" } else { "Reopen" }).clicked() {
            with(|u| {
                let text = std::mem::take(&mut u.reply);
                let r = if open { u.store.resolve(&c.id, me, Some(&text)) } else { u.store.reopen(&c.id) };
                if let Err(e) = r {
                    u.status = Some(e);
                }
                save_store(u);
            });
        }
        if ui.small_button("Ask the AI").on_hover_text("Puts this note in the AI editor's message box").clicked() {
            let (at, _) = c.time_in(&app.doc.model);
            let ask = format!("Please address review note {} at {:.2}s: \"{}\". When done, resolve it with comments (say what changed).", c.id, at as f64 / SEC as f64, c.text);
            let a = app.agent.get_or_insert_with(|| super::agent_panel::AgentUi::new(ui.ctx()));
            a.input = ask;
            super::agent_panel::open(app);
        }
        if ui.small_button("Delete").clicked() {
            with(|u| {
                let _ = u.store.delete(&c.id);
                u.selected = None;
                save_store(u);
            });
        }
    });
}

fn export_notes(app: &mut StudioApp, format: &str, ext: &str) {
    with(|u| {
        let Some(root) = u.root.clone() else { return };
        let text = match u.store.export(&app.doc.model, comments::Filter::All, format) {
            Ok(t) => t,
            Err(e) => return u.status = Some(e),
        };
        let dir = m::file::ProjectFolder::new(&root).export_dir(m::file::ExportKind::Review);
        let path = dir.join(format!("{} notes.{ext}", app.doc.project.name));
        let r = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&path, text));
        u.status = Some(match r {
            Ok(()) => format!("Wrote {}", studio_paths::tilde(&path)),
            Err(e) => format!("Could not write the list: {e}"),
        });
    });
}

fn versions_tab(app: &mut StudioApp, ui: &mut egui::Ui) {
    let t = theme::tokens(ui.ctx());
    let me = collab::identity();
    let Some(root) = with(|u| u.root.clone()) else { return };
    let mut name = with(|u| std::mem::take(&mut u.version_name));
    let mut note = with(|u| std::mem::take(&mut u.version_note));
    ui.add(egui::TextEdit::singleline(&mut name).desired_width(f32::INFINITY).hint_text("Version name, e.g. Director's cut"));
    ui.add(egui::TextEdit::singleline(&mut note).desired_width(f32::INFINITY).hint_text("What changed (optional)"));
    if Button::primary("Save version").compact().show(ui).clicked() {
        match versions::save(&root, &app.doc.model, &name, &note, &me) {
            Ok(v) => {
                with(|u| u.status = Some(format!("Saved {} \"{}\"", v.id, v.name)));
                name.clear();
                note.clear();
            }
            Err(e) => with(|u| u.status = Some(e)),
        }
        with(|u| u.index = versions::Index::load(&root).unwrap_or_default());
    }
    with(|u| {
        u.version_name = name;
        u.version_note = note;
    });
    ui.add_space(10.0);
    let (list, current, line) = with(|u| (u.index.versions.clone(), u.index.current.clone(), u.index.branch.clone()));
    let now = collab::fingerprint(&app.doc.model);
    ui.label(egui::RichText::new(format!("Branch: {line} · {} cuts in this project", app.doc.model.sequences.len())).font(theme::caption()).color(t.text_2));
    if list.is_empty() {
        ui.label(egui::RichText::new("No versions yet. A version is the whole edit as it is now, to compare with, go back to, or branch an alternate cut from.").color(t.text_2));
    }
    for v in list.iter().rev() {
        egui::Frame::new().fill(t.raised).corner_radius(CornerRadius::same(6)).inner_margin(8.0).show(ui, |ui| {
            ui.set_width(ui.available_width());
            let mark = if v.fingerprint == now { " · matches now" } else if current.as_deref() == Some(v.id.as_str()) { " · current" } else { "" };
            ui.label(egui::RichText::new(format!("{} {}{mark}", v.id, v.name)).color(t.text));
            ui.label(egui::RichText::new(format!("{} · {} clips · {:.1} s · {}", v.author, v.clips, v.duration as f64 / SEC as f64, v.branch)).font(theme::caption()).color(t.text_2));
            if !v.note.is_empty() {
                ui.label(egui::RichText::new(&v.note).font(theme::caption()).color(t.text_2));
            }
            ui.horizontal_wrapped(|ui| {
                if ui.small_button("Compare with now").clicked() {
                    let text = versions::load(&root, &v.id).map(|(_, p)| delta::report(&p, &app.doc.model).text()).unwrap_or_else(|e| e);
                    with(|u| u.compare = Some((v.id.clone(), text)));
                }
                if ui.small_button("Restore").on_hover_text("Back to this version, as one undo step").clicked() {
                    match versions::load(&root, &v.id).and_then(|(_, p)| app.doc.take_in(&format!("Restore {}", v.name), &p)) {
                        Ok(_) => {
                            let _ = versions::set_current(&root, &v.id);
                            with(|u| u.status = Some(format!("Restored {} (undo takes it back)", v.name)));
                        }
                        Err(e) => with(|u| u.status = Some(e)),
                    }
                }
                if ui.small_button("Branch").on_hover_text("This version's cut joins the project as another sequence").clicked() {
                    branch(app, &root, &v.id, &format!("{} (alt)", v.name));
                }
            });
            if let Some((id, text)) = with(|u| u.compare.clone())
                && id == v.id
            {
                ui.label(egui::RichText::new(text).font(theme::caption()).color(t.text));
            }
        });
        ui.add_space(4.0);
    }
    if app.doc.model.sequences.len() > 1 {
        ui.add_space(8.0);
        ui.label(egui::RichText::new("Cuts").color(t.text));
        let active = app.doc.model.active_sequence;
        for s in app.doc.model.sequences.clone() {
            if ui.selectable_label(s.id == active, &s.name).clicked() && s.id != active {
                app.doc.apply("Switch cut", m::Edit::SetActiveSequence { id: s.id });
            }
        }
    }
}

fn branch(app: &mut StudioApp, root: &Path, version: &str, name: &str) {
    let r = versions::load(root, version).and_then(|(_, src)| {
        let mut counter = app.doc.model.clone();
        let (mut edits, id) = versions::branch_edits(&mut counter, &src, None, name)?;
        edits.push(m::Edit::SetActiveSequence { id });
        app.doc.model.next_id = counter.next_id;
        if app.doc.apply(&format!("Branch {name}"), m::Edit::Batch { edits }) { Ok(()) } else { Err(app.doc.error.take().unwrap_or_default()) }
    });
    let _ = versions::set_branch(root, name);
    with(|u| {
        u.index = versions::Index::load(root).unwrap_or_default();
        u.status = Some(match r {
            Ok(()) => format!("\"{name}\" is now a cut of its own"),
            Err(e) => e,
        });
    });
}

const ROLES: [team::Role; 3] = [team::Role::Editor, team::Role::Reviewer, team::Role::Owner];

fn team_tab(app: &mut StudioApp, ui: &mut egui::Ui) {
    let t = theme::tokens(ui.ctx());
    let me = collab::identity();
    let role = with(|u| u.team.role_of(&me));
    ui.label(egui::RichText::new(format!("You are {me}: {}", role.name())).color(t.text));
    if !role.can_edit() {
        ui.label(egui::RichText::new("Reviewers add notes; edits are made by editors.").font(theme::caption()).color(t.text_2));
    }
    ui.add_space(8.0);
    let members = with(|u| u.team.members.clone());
    if members.is_empty() {
        ui.label(egui::RichText::new("Just you so far. Add people to give them roles; anyone not on the team then reviews.").color(t.text_2));
    }
    for mem in &members {
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new(&mem.name).color(t.text));
            ui.label(egui::RichText::new(mem.role.name()).font(theme::caption()).color(t.text_2));
            if role == team::Role::Owner && !mem.name.eq_ignore_ascii_case(&me) && ui.small_button("Remove").clicked() {
                with(|u| {
                    if let Err(e) = u.team.remove_member(&me, &mem.name) {
                        u.status = Some(e);
                    }
                });
                with(|u| save_team(app, u));
            }
        });
    }
    if role == team::Role::Owner {
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            let mut name = with(|u| std::mem::take(&mut u.member));
            ui.add(egui::TextEdit::singleline(&mut name).desired_width(140.0).hint_text("Name"));
            let mut r = with(|u| u.role);
            egui::ComboBox::from_id_salt("review-role").selected_text(ROLES[r].name()).show_ui(ui, |ui| {
                crate::kit::note_list(ui);
                for (i, x) in ROLES.iter().enumerate() {
                    ui.selectable_value(&mut r, i, x.name());
                }
            });
            let add = ui.button("Add").clicked();
            with(|u| {
                u.role = r;
                if add {
                    match u.team.set_member(&me, &name, ROLES[r]) {
                        Ok(()) => name.clear(),
                        Err(e) => u.status = Some(e),
                    }
                }
                u.member = name;
            });
            if add {
                with(|u| save_team(app, u));
            }
        });
    }
    ui.add_space(12.0);
    ui.label(egui::RichText::new("Locks").color(t.text));
    let seq = app.doc.model.active_sequence.0;
    let locks = with(|u| u.team.locks.clone());
    if locks.is_empty() {
        ui.label(egui::RichText::new("Nothing is held. Lock what you are working on so nobody else's edit changes it.").font(theme::caption()).color(t.text_2));
    }
    for l in &locks {
        ui.horizontal_wrapped(|ui| {
            let what = match (l.lane, l.clip) {
                (Some(id), _) => format!("lane {}", app.doc.project.lane(id as u32).map(|x| x.name.clone()).unwrap_or_else(|| id.to_string())),
                (_, Some(id)) => format!("clip #{id} {}", app.doc.project.clip(id as u32).map(|c| c.name.clone()).unwrap_or_default()),
                _ => "?".into(),
            };
            ui.label(egui::RichText::new(format!("{what} · {}", l.who)).color(t.text));
            if (l.who.eq_ignore_ascii_case(&me) || role == team::Role::Owner) && ui.small_button("Release").clicked() {
                with(|u| {
                    if let Err(e) = u.team.unlock(&me, l.sequence, l.lane, l.clip) {
                        u.status = Some(e);
                    }
                    save_team(app, u);
                });
            }
        });
    }
    if role.can_edit() {
        ui.horizontal_wrapped(|ui| {
            let sel = app.editor.selection.clone();
            if ui.add_enabled(!sel.is_empty(), egui::Button::new("Lock selected clips")).clicked() {
                with(|u| {
                    for id in &sel {
                        if let Err(e) = u.team.lock(&me, seq, None, Some(*id as u64), "") {
                            u.status = Some(e);
                        }
                    }
                    save_team(app, u);
                });
            }
            let lanes: Vec<(u32, String)> = app.doc.project.lanes.iter().map(|l| (l.id, l.name.clone())).collect();
            ui.menu_button("Lock a lane", |ui| {
                kit::popup_body(ui, |ui| {
                    for (id, name) in lanes {
                        if ui.button(&name).clicked() {
                            with(|u| {
                                if let Err(e) = u.team.lock(&me, seq, Some(id as u64), None, "") {
                                    u.status = Some(e);
                                }
                                save_team(app, u);
                            });
                            ui.close();
                        }
                    }
                })
            });
        });
    }
}

fn share_tab(app: &mut StudioApp, ui: &mut egui::Ui) {
    let t = theme::tokens(ui.ctx());
    let me = collab::identity();
    let Some(root) = with(|u| u.root.clone()) else { return };
    let caption = |ui: &mut egui::Ui, s: &str| {
        ui.label(egui::RichText::new(s).font(theme::caption()).color(t.text_2));
    };

    ui.label(egui::RichText::new("Review package").color(t.text));
    caption(ui, "A folder with the notes, the version and a notes template, in the project's Review folder. Render a proxy with Export (540p is plenty) and put it beside them. Sending it is up to you.");
    if ui.button("Write package").clicked() {
        let store = with(|u| u.store.clone());
        let index = with(|u| u.index.clone());
        let now = collab::fingerprint(&app.doc.model);
        let version = index.versions.iter().rev().find(|v| v.fingerprint == now).map(|v| (v.id.clone(), v.name.clone()));
        let dir = package::folder_in(&m::file::ProjectFolder::new(&root).export_dir(m::file::ExportKind::Review), &app.doc.model.active().name);
        let r = package::write(&dir, &app.doc.model, &store, version.as_ref().map(|(a, b)| (a.as_str(), b.as_str())), &me, None);
        with(|u| u.status = Some(r.map(|_| format!("Wrote {}", studio_paths::tilde(&dir))).unwrap_or_else(|e| e)));
    }
    ui.add_space(10.0);

    ui.label(egui::RichText::new("Notes back, or another copy").color(t.text));
    let mut path = with(|u| std::mem::take(&mut u.path));
    ui.horizontal_wrapped(|ui| {
        ui.add(egui::TextEdit::singleline(&mut path).desired_width(220.0).hint_text("Package, notes file or project folder"));
        if ui.small_button("Choose…").clicked()
            && let Some(p) = rfd::FileDialog::new().pick_folder()
        {
            path = p.display().to_string();
        }
    });
    with(|u| u.path = path.clone());
    ui.horizontal_wrapped(|ui| {
        if ui.button("Read notes").clicked() && !path.is_empty() {
            with(|u| {
                let r = package::read_back(Path::new(&path)).and_then(|files| {
                    let mut n = 0;
                    for (_, text) in files {
                        n += u.store.import(&app.doc.model, &text, "Reviewer")?;
                    }
                    Ok(n)
                });
                u.status = Some(match r {
                    Ok(n) => format!("{n} notes read"),
                    Err(e) => e,
                });
                save_store(u);
            });
        }
        if ui.button("Merge copy").on_hover_text("Takes in the edits made in another copy of this project (one undo step)").clicked() && !path.is_empty() {
            merge_copy(app, &root, Path::new(&path));
        }
    });
    ui.add_space(10.0);

    ui.label(egui::RichText::new("Work together").color(t.text));
    caption(ui, "A folder everyone can reach (a network drive, a synced folder). Each person links their copy to it; Sync exchanges edits and notes. Nothing leaves this computer except into that folder, when you press Sync.");
    let state = sync::State::load(&root).ok().flatten();
    match &state {
        Some(s) => {
            caption(ui, &format!("Linked to {} as {} · revision {}", studio_paths::tilde(Path::new(&s.remote)), s.user, s.base_rev));
            if Button::primary("Sync").compact().show(ui).clicked() {
                sync_now(app, &root, &s.remote);
            }
        }
        None => {
            let mut folder = with(|u| std::mem::take(&mut u.folder));
            ui.horizontal_wrapped(|ui| {
                ui.add(egui::TextEdit::singleline(&mut folder).desired_width(220.0).hint_text("Shared folder"));
                if ui.small_button("Choose…").clicked()
                    && let Some(p) = rfd::FileDialog::new().pick_folder()
                {
                    folder = p.display().to_string();
                }
            });
            if ui.button("Link").clicked() && !folder.is_empty() {
                link(app, &root, &folder);
            }
            with(|u| u.folder = folder);
        }
    }
}

fn merge_copy(app: &mut StudioApp, root: &Path, other: &Path) {
    let r = (|| -> Result<String, String> {
        let theirs = crate::services::project::open(other)?.model;
        let (a, b) = (versions::Index::load(root)?, versions::Index::load(other)?);
        let shared = a.versions.iter().rev().find(|v| b.versions.iter().any(|w| w.id == v.id && w.fingerprint == v.fingerprint)).ok_or("these copies share no saved version: save a version before handing a copy over")?;
        let base = versions::load(root, &shared.id)?.1;
        let o = collab::merge::three_way(&base, &app.doc.model, &theirs)?;
        let n = app.doc.take_in("Merge copy", &o.merged)?;
        with(|u| {
            u.store.merge(&comments::Store::load(other).unwrap_or_default());
            save_store(u);
        });
        Ok(format!("Merged: {n} edits, {} conflicts (yours kept){}", o.conflicts.len(), if o.fixes.is_empty() { String::new() } else { format!("; {}", o.fixes.join("; ")) }))
    })();
    with(|u| u.status = Some(r.unwrap_or_else(|e| e)));
}

fn link(app: &mut StudioApp, root: &Path, folder: &str) {
    let remote = sync::FolderRemote::new(folder);
    let r = sync::link(root, &remote, &app.doc.model, &collab::identity()).and_then(|(work, _, joined)| {
        if joined {
            app.doc.take_in("Join shared project", &work)?;
        }
        app.doc.model.next_id = app.doc.model.next_id.max(work.next_id);
        Ok(joined)
    });
    with(|u| {
        u.status = Some(match r {
            Ok(true) => "Joined: this copy now shows the shared edit".into(),
            Ok(false) => "Shared: others link to the same folder to join".into(),
            Err(e) => e,
        })
    });
}

fn sync_now(app: &mut StudioApp, root: &Path, remote: &str) {
    let remote = sync::FolderRemote::new(remote);
    with(|u| {
        let r = sync::sync(root, &remote, &app.doc.model, &mut u.store, &mut u.team).and_then(|(merged, o)| {
            app.doc.take_in("Sync with the team", &merged)?;
            app.doc.model.next_id = merged.next_id;
            Ok(o)
        });
        save_store(u);
        if let Ok(o) = &r
            && o.team_updated
        {
            let _ = collab::write_json(&team::file(root), &u.team);
            app.doc.guard = u.team.guard_for(&collab::identity());
        }
        u.status = Some(match r {
            Ok(o) => format!(
                "Synced (revision {}): {} changes in, {}{} conflicts, {} notes in{}",
                o.rev,
                o.taken.len(),
                if o.published { "yours sent, " } else { "" },
                o.conflicts.len(),
                o.notes_taken,
                if o.missing_media.is_empty() { String::new() } else { format!("; media not on this computer: {}", o.missing_media.join(", ")) }
            ),
            Err(e) => e,
        });
    });
}

/// Note markers in the timeline's ruler: a speech-bubble tick per note;
/// a click goes there and opens it.
pub fn ruler(app: &mut StudioApp, ui: &mut egui::Ui, p: &egui::Painter, r: Rect, x_of: impl Fn(Time) -> f32) {
    let t = theme::tokens(ui.ctx());
    let notes: Vec<(String, Time, Option<Time>, String, bool)> = with(|u| u.store.list(&app.doc.model, comments::Filter::All).into_iter().map(|c| (c.id.clone(), c.time_in(&app.doc.model).0, c.until, format!("{}: {}", c.author, c.text), c.is_open())).collect());
    for (id, at, until, text, open) in notes {
        let x = x_of(at);
        if x < r.left() - 8.0 || x > r.right() + 8.0 {
            continue;
        }
        let colour = if open { t.accent } else { theme::alpha(t.text_2, 0.6) };
        if let Some(u) = until {
            let band = Rect::from_min_max(egui::pos2(x, r.top() + 2.0), egui::pos2(x_of(u), r.top() + 5.0));
            p.rect_filled(band, CornerRadius::same(1), theme::alpha(colour, 0.5));
        }
        let bubble = Rect::from_center_size(egui::pos2(x, r.top() + 8.0), Vec2::new(10.0, 8.0));
        p.rect_filled(bubble, CornerRadius::same(2), colour);
        p.add(egui::Shape::convex_polygon(vec![egui::pos2(x - 2.0, bubble.bottom()), egui::pos2(x + 2.0, bubble.bottom()), egui::pos2(x, bubble.bottom() + 3.0)], colour, Stroke::NONE));
        let resp = ui.interact(bubble.expand(3.0), egui::Id::unique(("review-note", &id)), Sense::click());
        if resp.clicked() {
            app.editor.playhead = at;
            app.editor.playing = false;
            with(|u| {
                u.open = true;
                u.tab = Tab::Notes;
                u.selected = Some(id.clone());
            });
        }
        resp.on_hover_text(text);
    }
}

/// On the picture: the regions of notes at the playhead, and while "Mark
/// a region" is on, the pointer draws the next note's region.
pub fn viewer(app: &mut StudioApp, ui: &mut egui::Ui, canvas: Rect, scale: f32) {
    let t = theme::tokens(ui.ctx());
    let playhead = app.editor.playhead;
    let to_screen = |c: [f32; 2]| -> Pos2 { canvas.center() + Vec2::new(c[0], c[1]) * scale };
    let to_canvas = |p: Pos2| -> [f32; 2] { [(p.x - canvas.center().x) / scale, (p.y - canvas.center().y) / scale] };
    let p = ui.painter().with_clip_rect(canvas);
    let boxes: Vec<([f32; 4], String, bool)> = with(|u| {
        if !u.open && u.store.open_count() == 0 {
            return Vec::new();
        }
        u.store
            .list(&app.doc.model, comments::Filter::Open)
            .into_iter()
            .filter_map(|c| {
                let (at, _) = c.time_in(&app.doc.model);
                let end = c.until.unwrap_or(at + SEC / 2);
                let near = playhead >= at - SEC / 4 && playhead <= end;
                c.region.filter(|_| near).map(|r| (r, c.text.clone(), u.selected.as_deref() == Some(c.id.as_str())))
            })
            .collect()
    });
    for (r, text, sel) in boxes {
        let rect = Rect::from_two_pos(to_screen([r[0], r[1]]), to_screen([r[2], r[3]]));
        p.rect_stroke(rect, CornerRadius::same(3), Stroke::new(if sel { 2.5 } else { 1.5 }, t.accent), egui::StrokeKind::Outside);
        let label: String = text.chars().take(48).collect();
        let galley = p.layout_no_wrap(label, theme::caption(), t.accent_ink);
        let tag = Rect::from_min_size(rect.left_top() - Vec2::new(0.0, galley.size().y + 6.0), galley.size() + Vec2::new(10.0, 6.0));
        p.rect_filled(tag, CornerRadius::same(3), t.accent_fill);
        p.galley(tag.min + Vec2::new(5.0, 3.0), galley, t.accent_ink);
    }
    if let Some(r) = with(|u| u.region) {
        p.rect_stroke(Rect::from_two_pos(to_screen([r[0], r[1]]), to_screen([r[2], r[3]])), CornerRadius::same(3), Stroke::new(2.0, t.warn), egui::StrokeKind::Outside);
    }
    if !with(|u| u.draw_mode) {
        return;
    }
    let resp = ui.interact(canvas, egui::Id::unique("review-region"), Sense::click_and_drag()).on_hover_cursor(egui::CursorIcon::Crosshair);
    if resp.drag_started() {
        let start = resp.interact_pointer_pos().map(to_canvas);
        with(|u| u.drawing = start);
    }
    let now = ui.input(|i| i.pointer.interact_pos()).map(to_canvas);
    if let (Some(a), Some(b)) = (with(|u| u.drawing), now) {
        if resp.dragged() {
            p.rect_stroke(Rect::from_two_pos(to_screen(a), to_screen(b)), CornerRadius::same(3), Stroke::new(2.0, t.warn), egui::StrokeKind::Outside);
        }
        if resp.drag_stopped() {
            with(|u| {
                u.region = Some([a[0].min(b[0]), a[1].min(b[1]), a[0].max(b[0]), a[1].max(b[1])]);
                u.drawing = None;
                u.draw_mode = false;
            });
        }
    }
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        with(|u| {
            u.draw_mode = false;
            u.drawing = None;
        });
    }
}
