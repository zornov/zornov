// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Editor top bar, left to right: Home and the menus, the workspace tabs
//! (a dropdown when the window is too narrow for them), then the project's
//! name and save state, the Jobs bell and Export. Panels, workspaces and the
//! layout are in the View menu; Settings is ⌘, and File ▸ Settings.

use super::{Action, run};
use crate::app::{Jobs, StudioApp};
use crate::cli::Screen;
use crate::files::SaveState;
use crate::dock::Panel;
use crate::icons::{self, Icon};
use crate::kit::{self, Button, IconButton, MenuItem};
use crate::macmenu::{Do, Item, Menu, Row};
use crate::theme;
use crate::workspaces::Workspace;
use eframe::egui::{self, Align2, CornerRadius, Rect, Sense, Vec2};

/// Space kept clear for the macOS window buttons.
const WINDOW_BUTTONS_W: f32 = 76.0;
/// The project's name keeps at least this much room beside the workspace tabs.
const NAME_MIN_W: f32 = 96.0;
/// The Paint tab: a mode of its own (a screen), after the workspaces.
const PAINT: &str = "Paint";

pub fn show(app: &mut StudioApp, ui: &mut egui::Ui, r: Rect, narrow: bool) {
    let ctx = ui.ctx().clone();
    crate::macmenu::register(|app, ctx| model(app, ctx, ctx.content_rect().width() < super::NARROW), |app, ctx, what| perform(app, ctx, ctx.content_rect().width() < super::NARROW, what));
    crate::screens::start::window_drag_area(ui, r);
    let inner = Rect::from_min_max(egui::pos2(r.left() + WINDOW_BUTTONS_W, r.top()), egui::pos2(r.right() - 8.0, r.bottom()));
    // Right (laid out first, so the left side knows its room): Export, then Jobs.
    let mut right = ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(egui::Layout::right_to_left(egui::Align::Center)));
    right.spacing_mut().item_spacing.x = 6.0;
    let key = |id: &str| crate::keymap::hint(&app.prefs, id);
    let (export_keys, home_keys) = (key("export"), key("all-projects"));
    let export_btn = Button::primary("Export").compact().shortcut(&export_keys).show(&mut right);
    crate::onboarding::note_target(&ctx, "export", export_btn.rect);
    if export_btn.clicked() {
        run(app, &ctx, Action::Export);
    }
    if let Some(id) = jobs_button(&mut app.jobs, &mut right) {
        retry_job(app, &ctx, id);
    }
    let right_start = right.min_rect().left();

    let mut bar = ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(egui::Layout::left_to_right(egui::Align::Center)));
    bar.spacing_mut().item_spacing.x = 4.0;
    if IconButton::new(Icon::House, "All projects").shortcut(&home_keys).show(&mut bar).clicked() {
        app.go_home(&ctx);
    }
    bar.add_space(2.0);

    let spaces = crate::workspaces::all();
    let current = crate::workspaces::saved(app).and_then(|w| spaces.iter().position(|s| s.name.eq_ignore_ascii_case(&w.name)));
    let mut out = Picked::default();
    // On macOS the menus are in the menu bar (see `macmenu`); elsewhere they are this row.
    if !crate::macmenu::installed() {
        let max_h = (ctx.content_rect().bottom() - r.bottom() - 24.0).max(160.0);
        for m in model(app, &ctx, narrow) {
            let scroll = (m.title == "View").then_some(max_h);
            menu(&mut bar, m.title, &m.rows, scroll, &mut out);
        }
    }

    // The workspace tabs when they leave the project's name its room,
    // otherwise one dropdown with the same list.
    let left_end = bar.min_rect().right();
    let gap = (right_start - left_end - 12.0).max(0.0);
    let tabs_w = tabs_width(&bar, &spaces);
    let tabs_fit = gap - tabs_w >= NAME_MIN_W;
    bar.add_space(6.0);
    let used = if tabs_fit {
        workspace_tabs(&mut bar, &spaces, current, &mut out);
        tabs_w
    } else {
        workspace_menu(&mut bar, &spaces, current, &mut out);
        DROPDOWN_W
    };

    // Right-aligned, against the Jobs bell: the name gives way first, and
    // the save state ("Unsaved") before it.
    let room = (gap - used - 6.0).max(0.0);
    let name_w = kit::text_width(ui, &app.doc.project.name, theme::control_medium()).min(240.0) + 16.0;
    let state = app.save_state();
    let want_w = kit::text_width(ui, save_label(&state), theme::caption()) + 12.0;
    let status_w = if room >= name_w.min(120.0) + want_w { want_w } else { 0.0 };
    let w = (name_w + status_w).min(room);
    if w >= 40.0 {
        let slot = Rect::from_min_size(egui::pos2(right_start - 12.0 - w, r.top()), Vec2::new(w, r.height()));
        let mut cui = ui.new_child(egui::UiBuilder::new().max_rect(slot).layout(egui::Layout::left_to_right(egui::Align::Center)));
        cui.set_clip_rect(slot.intersect(ui.clip_rect()));
        cui.spacing_mut().item_spacing.x = 2.0;
        project_name(app, &mut cui, w - status_w, status_w > 0.0);
    }

    if let Some(w) = out.workspace {
        crate::workspaces::switch(app, &ctx, &w);
    }
    if out.paint {
        app.screen = Screen::Paint;
    }
    if let Some(name) = out.save_as {
        let w = crate::workspaces::current(app, &name);
        match crate::workspaces::save(w) {
            Ok(()) => kit::push(&ctx, format!("Saved the layout as \"{}\"", name.trim()), None, Some(Icon::SquaresFour)),
            Err(e) => kit::push(&ctx, e, None, Some(Icon::Warning)),
        };
    }
    if let Some(d) = out.pick {
        perform(app, &ctx, narrow, d);
    }
}

/// What the top bar's controls asked for this frame.
#[derive(Default)]
struct Picked {
    pick: Option<Do>,
    workspace: Option<Workspace>,
    paint: bool,
    save_as: Option<String>,
}

/// The name a workspace tab shows (the short form of a long name).
fn tab_label(name: &str) -> &str {
    if name == "Motion graphics" { "Motion" } else { name }
}

/// The tabs' labels: the built-in workspaces, then Paint.
fn tab_names(spaces: &[Workspace]) -> Vec<&str> {
    let builtin = crate::workspaces::builtin().len();
    spaces.iter().take(builtin).map(|w| tab_label(&w.name)).chain(std::iter::once(PAINT)).collect()
}

/// How wide the tab strip is (as the kit lays out a segmented control).
fn tabs_width(ui: &egui::Ui, spaces: &[Workspace]) -> f32 {
    tab_names(spaces).iter().map(|n| kit::text_width(ui, n, theme::control_medium()) + 24.0).sum::<f32>() + 4.0
}

/// The workspace tabs, with their names. Your own workspaces are in the dropdown and the View menu.
fn workspace_tabs(ui: &mut egui::Ui, spaces: &[Workspace], current: Option<usize>, out: &mut Picked) {
    let names = tab_names(spaces);
    let builtin = names.len() - 1;
    let opts: Vec<kit::Segment> = names.iter().map(|n| kit::Segment::text(n)).collect();
    // A workspace of your own is none of the tabs.
    let mut i = current.filter(|c| *c < builtin).unwrap_or(usize::MAX);
    let tabs = kit::segmented(ui, "workspace-tabs", &mut i, &opts, None, None);
    crate::onboarding::note_target(ui.ctx(), "workspace-tabs", tabs.rect);
    if tabs.changed() {
        if i == builtin {
            out.paint = true;
        } else {
            out.workspace = spaces.get(i).cloned();
        }
    }
}

/// Wide enough for "Motion" and a caret.
const DROPDOWN_W: f32 = 132.0;

/// The same list as a dropdown, for a window too narrow for the tabs.
fn workspace_menu(ui: &mut egui::Ui, spaces: &[Workspace], current: Option<usize>, out: &mut Picked) {
    let mut names: Vec<&str> = spaces.iter().map(|w| tab_label(&w.name)).collect();
    names.push(PAINT);
    let cur = current.unwrap_or(0);
    let before = ui.cursor().left();
    let picked = kit::menu_button(ui, "workspace-menu", cur, &names, DROPDOWN_W, None);
    crate::onboarding::note_target(ui.ctx(), "workspace-tabs", Rect::from_min_size(egui::pos2(before, ui.max_rect().top()), Vec2::new(DROPDOWN_W, 28.0)));
    if let Some(i) = picked {
        if i == spaces.len() {
            out.paint = true;
        } else {
            out.workspace = spaces.get(i).cloned();
        }
    }
}

/// The name (at most `max_w` wide with its padding), double-click to rename.
fn project_name(app: &mut StudioApp, ui: &mut egui::Ui, max_w: f32, with_status: bool) {
    let t = theme::tokens(ui.ctx());
    if let Some(mut name) = app.editor.renaming_project.take() {
        let resp = kit::text_field(ui, &mut name, "Project name", None, 200.0f32.min(max_w + 40.0), None);
        if !resp.has_focus() && !resp.lost_focus() {
            resp.request_focus();
        }
        let (enter, esc) = ui.input(|i| (i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape)));
        if esc {
            return;
        }
        if enter || resp.lost_focus() {
            let name = name.trim().to_owned();
            if !name.is_empty() {
                app.doc.edit("Rename project", |p| p.name = name);
            }
            return;
        }
        app.editor.renaming_project = Some(name);
        return;
    }
    let name = app.doc.project.name.clone();
    let w = (kit::text_width(ui, &name, theme::control_medium()).min(240.0) + 16.0).min(max_w);
    let (r, resp) = ui.allocate_exact_size(Vec2::new(w, 28.0), Sense::click());
    let st = kit::State::from_response(ui, &resp, None);
    ui.painter().rect_filled(r, CornerRadius::same(6), theme::alpha(t.raised, st.hover));
    let truncated = kit::paint_text_clipped(ui.painter(), egui::pos2(r.left() + 8.0, r.center().y), Align2::LEFT_CENTER, &name, theme::control_medium(), t.text, (w - 16.0).min(240.0));
    if st.focused {
        kit::focus_ring(ui.painter(), r, CornerRadius::same(6), &t);
    }
    let resp = if truncated { resp.on_hover_text(&name) } else { kit::tip(resp, "Rename project", Some("double-click")) };
    if resp.double_clicked() || (resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))) {
        app.editor.renaming_project = Some(name);
    }
    if !with_status {
        return;
    }
    let state = app.save_state();
    let colour = match state {
        SaveState::AutosaveFailed(_) => t.danger,
        SaveState::Unsaved => t.warn,
        _ => t.text_2,
    };
    let label = ui.add(egui::Label::new(egui::RichText::new(save_label(&state)).font(theme::caption()).color(colour)).sense(Sense::click()));
    let label = match save_hint(&state) {
        Some(hint) => label.on_hover_text(hint),
        None => label,
    };
    if label.clicked()
        && let Some(action) = save_click(&state)
    {
        run(app, &ui.ctx().clone(), action);
    }
}

/// What the top bar says about the project's file.
fn save_label(state: &SaveState) -> &'static str {
    match state {
        SaveState::Scratch => "Not saved",
        SaveState::Unsaved => "Unsaved",
        SaveState::Saving => "Saving…",
        SaveState::Saved => "Saved",
        SaveState::AutosaveFailed(_) => "Autosave failed",
    }
}

fn save_hint(state: &SaveState) -> Option<String> {
    match state {
        SaveState::Scratch => Some("Not saved yet. Click to choose where this project lives".to_owned()),
        SaveState::Unsaved => Some("Autosave writes your changes shortly. Click to save now".to_owned()),
        SaveState::AutosaveFailed(why) => Some(format!("Your changes are not on disk: {why}. Click to save now")),
        SaveState::Saving | SaveState::Saved => None,
    }
}

/// What clicking the state does: save now, or choose a folder first.
fn save_click(state: &SaveState) -> Option<Action> {
    match state {
        SaveState::Scratch => Some(Action::SaveAs),
        SaveState::Unsaved | SaveState::AutosaveFailed(_) => Some(Action::Save),
        SaveState::Saving | SaveState::Saved => None,
    }
}

/// Runs a menu row, from the in-window menu or the macOS menu bar. `narrow` is
/// whether the window is too narrow for docked panels.
fn perform(app: &mut StudioApp, ctx: &egui::Context, narrow: bool, what: Do) {
    match what {
        Do::Act(f) => {
            if let Some(a) = f() {
                run(app, ctx, a);
            }
        }
        // Each clip row says why when the selection does not fit.
        Do::Copy => super::clip_menu::copy_selection(app, ctx, false),
        Do::Cut => super::clip_menu::copy_selection(app, ctx, true),
        Do::Paste => super::clip_menu::paste_selection(app, ctx),
        Do::ClipKey(id) => super::clip_menu::key_action(app, ctx, id),
        Do::Panel(p) => super::dock_ui::toggle_panel(app, ctx, p, narrow),
        Do::Space(name) => {
            if let Some(w) = crate::workspaces::find(&name) {
                crate::workspaces::switch(app, ctx, &w);
            }
        }
        Do::ResetLayout => super::dock_ui::reset(app, ctx),
    }
}

fn item(label: &str, shortcut: &str, icon: Option<Icon>, enabled: bool, action: fn() -> Option<Action>) -> Row {
    Row::Item(Item { what: Do::Act(action), label: label.to_owned(), shortcut: shortcut.to_owned(), icon, enabled, checked: None })
}

fn clip_item(label: &str, shortcut: &str, icon: Option<Icon>, what: Do) -> Row {
    Row::Item(Item { what, label: label.to_owned(), shortcut: shortcut.to_owned(), icon, enabled: true, checked: None })
}

/// Every menu and its rows: the one list the in-window menus and the macOS menu bar are built from.
fn model(app: &StudioApp, ctx: &egui::Context, narrow: bool) -> Vec<Menu> {
    let sel = !app.editor.selection.is_empty();
    let d = &app.doc;
    let k = |id: &str| crate::keymap::hint(&app.prefs, id);
    let mut menus = vec![
        Menu { title: "File", rows: vec![
            item("Open…", "⌘O", Some(Icon::FolderOpen), true, || Some(Action::Open)),
            item("Save", "⌘S", None, true, || Some(Action::Save)),
            item("Save as…", "⇧⌘S", None, true, || Some(Action::SaveAs)),
            Row::Sep,
            item("Import media…", "⌘I", Some(Icon::DownloadSimple), true, || Some(Action::ImportMedia)),
            item("Add sample media", "", None, true, || Some(Action::SampleMedia)),
            item("Import timeline…", "", None, true, || Some(Action::ImportTimeline)),
            item("Import script…", "", None, true, || Some(Action::ImportScript)),
            Row::Sep,
            item("Project settings…", "", Some(Icon::Faders), true, || Some(Action::ProjectSettings)),
            Row::Sep,
            item("Export OpenTimelineIO…", "", None, true, || Some(Action::ExportOtio)),
            item("Export…", &k("export"), Some(Icon::Export), true, || Some(Action::Export)),
            Row::Sep,
            item("Settings…", &k("settings"), Some(Icon::GearSix), true, || Some(Action::Settings)),
        ] },
        Menu { title: "Edit", rows: vec![
            item("Undo", &k("undo"), Some(Icon::ArrowUUpLeft), d.can_undo(), || Some(Action::Undo)),
            item("Redo", &k("redo"), Some(Icon::ArrowUUpRight), d.can_redo(), || Some(Action::Redo)),
            Row::Sep,
            clip_item("Cut", &k("cut"), Some(Icon::Scissors), Do::Cut),
            clip_item("Copy", &k("copy"), Some(Icon::Copy), Do::Copy),
            clip_item("Paste", &k("paste"), None, Do::Paste),
            Row::Sep,
            item("Select all", &k("select-all"), None, true, || Some(Action::SelectAll)),
            item("Duplicate", &k("duplicate"), Some(Icon::Copy), sel, || Some(Action::Duplicate)),
            item("Delete", &k("delete"), Some(Icon::Trash), sel, || Some(Action::Delete)),
        ] },
        Menu { title: "Clip", rows: vec![
            item("Split at playhead", &k("split"), Some(Icon::Scissors), true, || Some(Action::Split)),
            item("Add marker", &k("marker"), Some(Icon::Flag), true, || Some(Action::Marker)),
            Row::Sep,
            clip_item("Group", &k("group"), None, Do::ClipKey("group")),
            clip_item("Ungroup", &k("ungroup"), None, Do::ClipKey("ungroup")),
            clip_item("Link", &k("link"), None, Do::ClipKey("link")),
            clip_item("Unlink", &k("unlink"), None, Do::ClipKey("unlink")),
            Row::Sep,
            clip_item("Freeze frame at playhead", &k("freeze"), None, Do::ClipKey("freeze")),
            clip_item("Reverse", &k("reverse"), None, Do::ClipKey("reverse")),
            clip_item("Copy look", &k("copy-look"), None, Do::ClipKey("copy-look")),
            clip_item("Paste look", &k("paste-look"), None, Do::ClipKey("paste-look")),
            Row::Sep,
            item("Set in point", &k("in"), None, true, || Some(Action::RangeIn)),
            item("Set out point", &k("out"), None, true, || Some(Action::RangeOut)),
            item("Clear range", &k("clear-range"), None, d.project.range.is_some(), || Some(Action::ClearRange)),
        ] },
        // What used to be the Production and Library buttons.
        Menu { title: "Project", rows: vec![
            item("Production…", "", Some(Icon::FilmSlate), true, || Some(Action::Production)),
            item("Project health…", "", Some(Icon::LockSimple), true, || Some(Action::ProjectHealth)),
        ] },
        Menu { title: "View", rows: vec![
            item("Zoom in", &k("zoom-in"), Some(Icon::MagnifyingGlassPlus), true, || Some(Action::ZoomIn)),
            item("Zoom out", &k("zoom-out"), Some(Icon::MagnifyingGlassMinus), true, || Some(Action::ZoomOut)),
            item("Fit sequence", &k("fit"), Some(Icon::ArrowsOutLineHorizontal), true, || Some(Action::Fit)),
        ] },
        Menu { title: "Help", rows: vec![
            item("Search help", "F1", Some(Icon::Question), true, || Some(Action::Help)),
            item("Welcome tour", "", Some(Icon::Sparkle), true, || Some(Action::Tour)),
            item("Keyboard shortcuts", "", Some(Icon::Keyboard), true, || Some(Action::Shortcuts)),
            Row::Sep,
            item("Check for updates…", "", Some(Icon::DownloadSimple), true, || Some(Action::CheckUpdates)),
        ] },
    ];
    // The View menu goes on with the workspaces, the panels (checked when
    // open) and the layout commands: the one place that lists them all (the
    // tabs are the quick way to the built-in workspaces).
    let spaces = crate::workspaces::all();
    let current = crate::workspaces::saved(app).and_then(|w| spaces.iter().position(|s| s.name.eq_ignore_ascii_case(&w.name)));
    let view = &mut menus[4].rows;
    view.push(Row::Sep);
    view.push(Row::Heading("Workspaces"));
    for (i, w) in spaces.iter().enumerate() {
        let keys = crate::workspaces::ACTIONS.get(i).map_or("", |(id, _)| crate::keymap::keys(&app.prefs, id).unwrap_or(""));
        view.push(Row::Item(Item { what: Do::Space(w.name.clone()), label: w.name.clone(), shortcut: keys.to_owned(), icon: None, enabled: true, checked: Some(current == Some(i)) }));
    }
    view.push(Row::Heading("Panels"));
    for p in Panel::ALL.into_iter().filter(|p| p.closable()) {
        let keys = crate::keymap::keys(&app.prefs, &format!("panel-{}", p.key())).unwrap_or("");
        let on = super::dock_ui::panel_shown(app, ctx, p, narrow);
        view.push(Row::Item(Item { what: Do::Panel(p), label: p.name().to_owned(), shortcut: keys.to_owned(), icon: None, enabled: true, checked: Some(on) }));
    }
    view.push(Row::Heading("Layout"));
    view.push(Row::Item(Item { what: Do::Act(|| Some(Action::Maximise)), label: "Maximise panel".into(), shortcut: "⌘\\".into(), icon: Some(Icon::CornersOut), enabled: true, checked: Some(app.editor.maximised.is_some()) }));
    view.push(Row::SaveLayout);
    view.push(Row::Item(Item { what: Do::ResetLayout, label: "Reset layout".into(), shortcut: String::new(), icon: Some(Icon::ArrowCounterClockwise), enabled: true, checked: None }));
    menus
}

/// Where the menu button `name` was drawn last frame.
pub(crate) fn menu_rect_id(name: &str) -> egui::Id {
    egui::Id::new(("topbar-menu-rect", name.to_owned()))
}

/// A menu of `rows`; what is chosen goes in `out`. `scroll` is the most height it takes before it scrolls.
fn menu(ui: &mut egui::Ui, name: &str, rows: &[Row], scroll: Option<f32>, out: &mut Picked) {
    let t = theme::tokens(ui.ctx());
    let id = ui.scope_id().with(("menu", name));
    let w = kit::text_width(ui, name, theme::control()) + 20.0;
    let (r, resp) = ui.allocate_exact_size(Vec2::new(w, 28.0), Sense::click());
    let open = kit::is_open(ui.ctx(), id);
    let st = kit::State::from_response(ui, &resp, None);
    ui.painter().rect_filled(r, CornerRadius::same(6), theme::alpha(t.raised, st.hover.max(open as u8 as f32)));
    ui.painter().text(r.center(), Align2::CENTER_CENTER, crate::i18n::shown(name), theme::control(), if open { t.text } else { theme::mix(t.text_2, t.text, st.hover) });
    if st.focused {
        kit::focus_ring(ui.painter(), r, CornerRadius::same(6), &t);
    }
    ui.ctx().data_mut(|d| d.insert_temp(menu_rect_id(name), r));
    if resp.clicked() {
        kit::toggle_open(ui.ctx(), id);
    }
    // A scrolling menu keeps its Layout block (Maximise, Save, Reset) below the
    // scroll area, so those rows are never hidden past the fold.
    let split = if scroll.is_some() { rows.iter().position(|r| matches!(r, Row::Heading("Layout"))) } else { None };
    let (body, pinned) = rows.split_at(split.unwrap_or(rows.len()));
    kit::popover(ui.ctx(), id, r, kit::Side::Below, 248.0, |ui| {
        let room = scroll.map(|h| if pinned.is_empty() { h } else { (h - PINNED_H).max(120.0) });
        egui::ScrollArea::vertical().id_salt(("menu-scroll", name)).max_height(room.unwrap_or(f32::INFINITY)).show(ui, |ui| {
            for row in body {
                menu_row(ui, id, row, out);
            }
        });
        if !pinned.is_empty() {
            kit::menu_separator(ui);
            for row in pinned {
                menu_row(ui, id, row, out);
            }
        }
    });
}

/// Height kept for a scrolling menu's pinned Layout block.
const PINNED_H: f32 = 150.0;

fn menu_row(ui: &mut egui::Ui, id: egui::Id, row: &Row, out: &mut Picked) {
    match row {
        Row::Sep => kit::menu_separator(ui),
        Row::Heading(h) => kit::menu_heading(ui, h),
        Row::SaveLayout => save_layout_field(ui, id, out),
        Row::Item(it) => {
            let mut m = MenuItem::new(&it.label).enabled(it.enabled);
            if !it.shortcut.is_empty() {
                m = m.shortcut(&it.shortcut);
            }
            if let Some(on) = it.checked {
                m = m.checked(on);
            }
            m = match it.icon {
                Some(i) => m.icon(i),
                // Keep labels aligned with rows that have icons.
                None if it.checked.is_none() => m.reserve_icon(),
                None => m,
            };
            if m.show(ui).clicked() {
                out.pick = Some(it.what.clone());
                kit::set_open(ui.ctx(), id, false);
            }
        }
    }
}

/// The View menu's "Save layout as…" field.
fn save_layout_field(ui: &mut egui::Ui, menu_id: egui::Id, out: &mut Picked) {
    let id = egui::Id::unique("save-workspace-name");
    let mut name: String = ui.ctx().data(|d| d.get_temp(id)).unwrap_or_default();
    let resp = kit::text_field(ui, &mut name, "Save layout as…", None, 196.0, None);
    if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && !name.trim().is_empty() {
        out.save_as = Some(std::mem::take(&mut name));
        kit::set_open(ui.ctx(), menu_id, false);
    }
    ui.ctx().data_mut(|d| d.insert_temp(id, name));
}

/// A job that stopped on an error (a cancelled one stopped on request).
fn failed(j: &crate::app::Job) -> bool {
    j.error.is_some()
}

/// What the popover says under a job's name.
fn job_status(j: &crate::app::Job) -> String {
    if failed(j) {
        "Failed · export again from File ▸ Export".to_owned()
    } else if j.cancelled {
        "Cancelled".to_owned()
    } else if j.done {
        "Done".to_owned()
    } else {
        format!("{} · {}%  ·  {}", j.detail, (j.progress * 100.0) as u32, Jobs::eta(j))
    }
}

/// How many finished jobs the popover keeps listed.
const KEPT_FINISHED: usize = Jobs::KEPT_FINISHED;

/// Every running job and the last few finished ones, newest first.
fn shown_jobs(list: &[crate::app::Job]) -> Vec<&crate::app::Job> {
    let mut finished = 0;
    list.iter()
        .rev()
        .filter(|j| {
            if !j.done && !j.cancelled {
                return true;
            }
            finished += 1;
            finished <= KEPT_FINISHED
        })
        .collect()
}

/// Jobs button: a spinner while anything runs; opens the jobs popover.
fn jobs_button(jobs: &mut Jobs, ui: &mut egui::Ui) -> Option<u64> {
    let mut retry = None;
    let t = theme::tokens(ui.ctx());
    let running = jobs.running();
    let id = ui.scope_id().with("jobs");
    let resp = IconButton::new(Icon::Bell, if running > 0 { "Jobs (running)" } else { "Jobs" }).active(kit::is_open(ui.ctx(), id)).show(ui);
    if running > 0 {
        let c = resp.rect.right_top() + Vec2::new(-7.0, 7.0);
        ui.painter().circle_filled(c, 6.5, t.panel.gamma_multiply(0.0));
        ui.painter().circle_filled(c, 4.0, t.accent);
        kit::spinner(ui.painter(), resp.rect.center(), 10.5, theme::alpha(t.accent, 0.8));
    }
    if resp.clicked() {
        kit::toggle_open(ui.ctx(), id);
    }
    kit::popover(ui.ctx(), id, resp.rect, kit::Side::BelowRight, 320.0, |ui| {
        kit::menu_heading(ui, "Jobs");
        #[cfg(feature = "render")]
        crate::screens::queue::section(ui);
        if jobs.list.is_empty() {
            let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 72.0), Sense::hover());
            icons::paint(ui.painter(), Rect::from_center_size(r.center() - Vec2::new(0.0, 12.0), Vec2::splat(20.0)), Icon::CheckCircle, 20.0, t.text_2);
            ui.painter().text(r.center() + Vec2::new(0.0, 14.0), Align2::CENTER_CENTER, crate::i18n::shown("Nothing running. Exports and AI tasks show here."), theme::body(), t.text_2);
            return;
        }
        let mut cancel = None;
        for j in shown_jobs(&jobs.list) {
            let (r, row) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 52.0), Sense::hover());
            if let Some(e) = &j.error {
                row.on_hover_text(e);
            }
            let p = ui.painter();
            kit::paint_text_clipped(p, egui::pos2(r.left() + 8.0, r.top() + 14.0), Align2::LEFT_CENTER, &j.name, theme::control_medium(), t.text, r.width() - 50.0);
            let status = job_status(j);
            kit::paint_text_clipped(p, egui::pos2(r.left() + 8.0, r.top() + 32.0), Align2::LEFT_CENTER, &status, theme::caption(), if failed(j) { t.danger } else { t.text_2 }, r.width() - 50.0);
            let bar = Rect::from_min_size(egui::pos2(r.left() + 8.0, r.bottom() - 6.0), Vec2::new(r.width() - 50.0, 3.0));
            p.rect_filled(bar, CornerRadius::same(2), t.raised);
            p.rect_filled(Rect::from_min_size(bar.min, Vec2::new(bar.width() * j.progress, 3.0)), CornerRadius::same(2), if failed(j) { t.danger } else if j.cancelled { t.text_2 } else { t.accent });
            if can_retry(j) {
                let br = Rect::from_center_size(egui::pos2(r.right() - 18.0, r.center().y), Vec2::splat(24.0));
                if IconButton::new(Icon::ArrowCounterClockwise, "Retry").size(24.0, 14.0).show_at(ui, br).clicked() {
                    retry = Some(j.id);
                }
            }
            if !j.done && !j.cancelled {
                let br = Rect::from_center_size(egui::pos2(r.right() - 18.0, r.center().y), Vec2::splat(24.0));
                if IconButton::new(Icon::X, "Cancel job").size(24.0, 14.0).show_at(ui, br).clicked() {
                    cancel = Some(j.id);
                }
            }
        }
        if let Some(id) = cancel {
            jobs.cancel(id);
        }
    });
    retry
}

/// A failed or cancelled export that kept its request can run again.
fn can_retry(j: &crate::app::Job) -> bool {
    #[cfg(feature = "render")]
    return (j.error.is_some() || j.cancelled) && j.request.is_some();
    #[cfg(not(feature = "render"))]
    return {
        let _ = j;
        false
    };
}

/// Retry: the export runs again as a new job with the same request.
#[cfg(feature = "render")]
fn retry_job(app: &mut StudioApp, ctx: &egui::Context, id: u64) {
    let doc = &app.doc;
    match app.jobs.retry_with(id, |req| crate::export_run::start(doc, req.clone())) {
        Some(_) => {
            kit::push(ctx, "Export started again", None, Some(Icon::ArrowCounterClockwise));
        }
        None => {
            kit::push(ctx, "That export can't be run again; start it from Export", None, Some(Icon::Info));
        }
    }
}

#[cfg(not(feature = "render"))]
fn retry_job(_app: &mut StudioApp, _ctx: &egui::Context, _id: u64) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn menus_of(h: &crate::ui_tests::Harness) -> Vec<Menu> {
        model(&h.app, &h.ctx, false)
    }

    fn items(all: &[Menu]) -> Vec<(&'static str, &Item)> {
        all.iter().flat_map(|m| m.items().map(move |i| (m.title, i))).collect()
    }

    fn action_of(i: &Item) -> Option<std::mem::Discriminant<Action>> {
        match &i.what {
            Do::Act(f) => f().map(|a| std::mem::discriminant(&a)),
            _ => None,
        }
    }

    /// Every Edit and Clip row that works on the selection has an item with
    /// its keys, and the menus list them once.
    #[test]
    fn the_clip_rows_are_in_the_menus_once_with_their_keys() {
        let h = crate::ui_tests::Harness::new(&["--screen", "editor"]);
        let all = menus_of(&h);
        let rows: Vec<(&str, String)> = items(&all).into_iter().filter(|(_, i)| matches!(i.what, Do::Cut | Do::Copy | Do::Paste | Do::ClipKey(_))).map(|(_, i)| (i.label.as_str(), i.shortcut.clone())).collect();
        assert_eq!(rows.len(), 11);
        for want in ["Cut", "Copy", "Paste", "Group", "Ungroup", "Link", "Unlink", "Freeze frame at playhead", "Reverse", "Copy look", "Paste look"] {
            assert_eq!(rows.iter().filter(|r| r.0 == want).count(), 1, "{want}");
        }
        assert_eq!(rows.iter().find(|r| r.0 == "Copy").unwrap().1, "⌘C");
        assert_eq!(rows.iter().find(|r| r.0 == "Group").unwrap().1, "⌘G");
    }

    /// The menus the window and the menu bar are built from: these titles in
    /// this order, each row doing the action it names.
    #[test]
    fn every_menu_row_is_in_the_model_with_its_action() {
        let h = crate::ui_tests::Harness::new(&["--screen", "editor"]);
        let all = menus_of(&h);
        assert_eq!(all.iter().map(|m| m.title).collect::<Vec<_>>(), ["File", "Edit", "Clip", "Project", "View", "Help"]);
        let d = |a: Action| Some(std::mem::discriminant(&a));
        let want: Vec<(&str, &str, Option<std::mem::Discriminant<Action>>)> = vec![
            ("File", "Open…", d(Action::Open)),
            ("File", "Save", d(Action::Save)),
            ("File", "Save as…", d(Action::SaveAs)),
            ("File", "Import media…", d(Action::ImportMedia)),
            ("File", "Add sample media", d(Action::SampleMedia)),
            ("File", "Import timeline…", d(Action::ImportTimeline)),
            ("File", "Import script…", d(Action::ImportScript)),
            ("File", "Project settings…", d(Action::ProjectSettings)),
            ("File", "Export OpenTimelineIO…", d(Action::ExportOtio)),
            ("File", "Export…", d(Action::Export)),
            ("File", "Settings…", d(Action::Settings)),
            ("Edit", "Undo", d(Action::Undo)),
            ("Edit", "Redo", d(Action::Redo)),
            ("Edit", "Select all", d(Action::SelectAll)),
            ("Edit", "Duplicate", d(Action::Duplicate)),
            ("Edit", "Delete", d(Action::Delete)),
            ("Clip", "Split at playhead", d(Action::Split)),
            ("Clip", "Add marker", d(Action::Marker)),
            ("Clip", "Set in point", d(Action::RangeIn)),
            ("Clip", "Set out point", d(Action::RangeOut)),
            ("Clip", "Clear range", d(Action::ClearRange)),
            ("Project", "Production…", d(Action::Production)),
            ("Project", "Project health…", d(Action::ProjectHealth)),
            ("View", "Zoom in", d(Action::ZoomIn)),
            ("View", "Zoom out", d(Action::ZoomOut)),
            ("View", "Fit sequence", d(Action::Fit)),
            ("View", "Maximise panel", d(Action::Maximise)),
            ("Help", "Search help", d(Action::Help)),
            ("Help", "Welcome tour", d(Action::Tour)),
            ("Help", "Keyboard shortcuts", d(Action::Shortcuts)),
            ("Help", "Check for updates…", d(Action::CheckUpdates)),
        ];
        let got = items(&all);
        for (menu, label, action) in want {
            let hits: Vec<_> = got.iter().filter(|(m, i)| *m == menu && i.label == label).collect();
            assert_eq!(hits.len(), 1, "{menu} ▸ {label}");
            assert!(action_of(hits[0].1) == action, "{menu} ▸ {label} runs another action");
        }
        // The View menu lists the workspaces and every closable panel, and resets the layout.
        let view = got.iter().filter(|(m, _)| *m == "View").count();
        assert!(view >= 3 + 1 + crate::workspaces::all().len() + 1, "{view} View rows");
        assert!(got.iter().any(|(m, i)| *m == "View" && matches!(i.what, Do::ResetLayout)));
        assert!(got.iter().any(|(m, i)| *m == "View" && matches!(&i.what, Do::Space(n) if n == "Edit")));
        assert!(got.iter().any(|(m, i)| *m == "View" && matches!(i.what, Do::Panel(Panel::Inspector)) && i.checked.is_some()));
    }

    #[test]
    fn rows_are_disabled_when_they_cannot_run() {
        let h = crate::ui_tests::Harness::new(&["--screen", "editor"]);
        let all = menus_of(&h);
        let row = |label: &str| items(&all).into_iter().find(|(_, i)| i.label == label).map(|(_, i)| i.enabled).unwrap();
        // A fresh project: nothing to undo or redo, nothing selected.
        for label in ["Undo", "Redo", "Duplicate", "Delete"] {
            assert!(!row(label), "{label}");
        }
        assert_eq!(row("Clear range"), h.app.doc.project.range.is_some());
        for label in ["Save", "Export…", "Split at playhead", "Zoom in"] {
            assert!(row(label), "{label}");
        }
    }

    /// Two rows with the same ⌘ key would race for it in the menu bar.
    #[test]
    fn no_command_key_belongs_to_two_rows() {
        let h = crate::ui_tests::Harness::new(&["--screen", "editor"]);
        let all = menus_of(&h);
        let mut seen: Vec<(String, &str)> = Vec::new();
        for (_, i) in items(&all) {
            if crate::macmenu::native_key(&i.shortcut).is_some() {
                assert!(!seen.iter().any(|(k, _)| *k == i.shortcut), "{} shares {}", i.label, i.shortcut);
                seen.push((i.shortcut.clone(), &i.label));
            }
        }
        // And the menu bar binds each of them once.
        let flat: Vec<&Item> = all.iter().flat_map(Menu::items).collect();
        let owned = crate::macmenu::owned_keys(flat.iter().copied(), &[]);
        let bound: Vec<_> = owned.iter().flatten().collect();
        for (n, k) in bound.iter().enumerate() {
            assert!(!bound[n + 1..].contains(k), "{k:?} bound twice");
        }
    }

    fn job(id: u64, done: bool, cancelled: bool, error: Option<&str>) -> crate::app::Job {
        crate::app::Job { id, name: format!("Export {id}"), detail: String::new(), progress: 0.5, length: 10.0, done, cancelled, task: None, started: 0.0, error: error.map(str::to_owned), path: None, #[cfg(feature = "render")] request: None }
    }

    #[test]
    fn a_failed_job_says_failed_and_a_cancelled_one_says_cancelled() {
        // The task loop marks a failed export cancelled and records why.
        let failed_job = job(1, false, true, Some("No space left on disk"));
        assert!(job_status(&failed_job).starts_with("Failed"));
        assert!(job_status(&job(2, false, true, None)).starts_with("Cancelled"));
        assert_eq!(job_status(&job(3, true, false, None)), "Done");
        assert!(job_status(&job(4, false, false, None)).contains('%'));
    }

    #[test]
    fn the_popover_lists_running_jobs_and_the_last_six_finished() {
        let mut list: Vec<_> = (1..=9).map(|i| job(i, true, false, None)).collect();
        list.insert(0, job(100, false, false, None)); // the oldest, still running
        let ids: Vec<u64> = shown_jobs(&list).iter().map(|j| j.id).collect();
        assert_eq!(ids.len(), 7, "{ids:?}");
        assert!(ids.contains(&100), "a running job is never dropped");
        assert_eq!(&ids[..6], &[9, 8, 7, 6, 5, 4]);
        assert!(!ids.contains(&3));
    }

    #[test]
    fn save_state_names_each_case_and_failure_saves_now() {
        assert_eq!(save_label(&SaveState::Scratch), "Not saved");
        assert_eq!(save_label(&SaveState::Unsaved), "Unsaved");
        assert_eq!(save_label(&SaveState::Saving), "Saving…");
        assert_eq!(save_label(&SaveState::Saved), "Saved");
        let failed = SaveState::AutosaveFailed("disk full".into());
        assert_eq!(save_label(&failed), "Autosave failed");
        assert!(save_hint(&failed).unwrap().contains("disk full"));
        assert!(matches!(save_click(&failed), Some(Action::Save)));
        assert!(matches!(save_click(&SaveState::Scratch), Some(Action::SaveAs)));
        assert!(save_click(&SaveState::Saved).is_none());
    }
}
