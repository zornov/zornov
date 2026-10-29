// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Editor top bar: back to Start, project name, menus, undo/redo, jobs,
//! panel toggles, settings and Export.

use super::{Action, run};
use crate::app::{Jobs, Overlay, StudioApp};
use crate::cli::Screen;
use crate::icons::{self, Icon};
use crate::kit::{self, Button, IconButton, MenuItem};
use crate::theme;
use eframe::egui::{self, Align2, CornerRadius, Rect, Sense, Vec2};

/// Space kept clear for the macOS window buttons.
const WINDOW_BUTTONS_W: f32 = 76.0;

pub fn show(app: &mut StudioApp, ui: &mut egui::Ui, r: Rect, narrow: bool) {
    let t = theme::tokens(ui.ctx());
    let ctx = ui.ctx().clone();
    crate::screens::start::window_drag_area(ui, r);
    let inner = Rect::from_min_max(egui::pos2(r.left() + WINDOW_BUTTONS_W, r.top()), egui::pos2(r.right() - 8.0, r.bottom()));
    // Right (laid out first, so the left side knows its room): Export,
    // then text buttons for Layout and Settings.
    let mut right = ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(egui::Layout::right_to_left(egui::Align::Center)));
    right.spacing_mut().item_spacing.x = 6.0;
    let key = |id: &str| crate::keymap::hint(&app.prefs, id);
    let (export_keys, settings_keys, home_keys) = (key("export"), key("settings"), key("all-projects"));
    let export_btn = Button::primary("Export").compact().shortcut(&export_keys).show(&mut right);
    crate::onboarding::note_target(&ctx, "export", export_btn.rect);
    if export_btn.clicked() {
        app.overlay = Overlay::Export;
    }
    let layout_id = egui::Id::new("topbar-layout");
    let lresp = Button::secondary("Layout").compact().show(&mut right);
    if lresp.clicked() {
        kit::toggle_open(&ctx, layout_id);
    }
    if Button::secondary("Settings").compact().shortcut(&settings_keys).show(&mut right).clicked() {
        app.overlay = Overlay::Settings;
    }
    if Button::secondary("Paint").compact().shortcut("⇧⌘P").show(&mut right).clicked() {
        app.screen = Screen::Paint;
    }
    super::review::topbar_button(&mut right);
    if Button::secondary("Library").compact().show(&mut right).on_hover_text(crate::i18n::shown("Search all your media, project health and history, backups, licences")).clicked() {
        app.overlay = Overlay::Library;
    }
    if Button::secondary("Production").compact().show(&mut right).clicked() {
        app.overlay = Overlay::Production;
    }
    if app.jobs.running() > 0 || !app.jobs.list.is_empty() {
        jobs_button(&mut app.jobs, &mut right);
    }
    let right_start = right.min_rect().left();

    let mut bar = ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(egui::Layout::left_to_right(egui::Align::Center)));
    bar.spacing_mut().item_spacing.x = 4.0;

    if IconButton::new(Icon::House, "All projects").shortcut(&home_keys).show(&mut bar).clicked() {
        app.screen = Screen::Start;
    }
    bar.add_space(2.0);
    let mut action = None;
    let mut toggle: Option<crate::dock::Panel> = None;
    // Every panel that can be docked, checked when it is open.
    let panels: Vec<(crate::dock::Panel, bool, String)> = crate::dock::Panel::ALL
        .into_iter()
        .filter(|p| p.closable())
        .map(|p| (p, super::dock_ui::panel_shown(app, &ctx, p, narrow), crate::keymap::keys(&app.prefs, &format!("panel-{}", p.key())).unwrap_or("").to_owned()))
        .collect();
    for (name, items) in menus(app) {
        let (a, t) = menu(&mut bar, name, &items, &panels);
        if a.is_some() {
            action = a;
        }
        if t.is_some() {
            toggle = t;
        }
    }
    // The workspace tabs only when they leave the project name its room.
    let room = right_start - bar.cursor().left() - 12.0 - 160.0;
    workspace_switcher(app, &mut bar, room);
    let left_end = bar.min_rect().right();

    // Centre: the project name and its save state, in the window's middle
    // when the gap between the menus and the buttons allows, otherwise
    // moved into that gap and cut short to fit it.
    // The save state ("Edited") goes before the name does.
    let gap = (left_end + 12.0)..=(right_start - 12.0);
    let room = (gap.end() - gap.start()).max(0.0);
    let name_w = kit::text_width(ui, &app.doc.project.name, theme::control_medium()).min(240.0) + 16.0;
    let status_w = if room >= name_w.min(120.0) + 60.0 { 60.0 } else { 0.0 };
    let w = (name_w + status_w).min(room);
    let x = (r.center().x - w / 2.0).clamp(*gap.start(), (gap.end() - w).max(*gap.start()));
    let centre = Rect::from_min_size(egui::pos2(x, r.top()), Vec2::new(w, r.height()));
    if w >= 40.0 {
        let mut cui = ui.new_child(egui::UiBuilder::new().max_rect(centre).layout(egui::Layout::left_to_right(egui::Align::Center)));
        cui.set_clip_rect(centre.intersect(ui.clip_rect()));
        cui.spacing_mut().item_spacing.x = 2.0;
        project_name(app, &mut cui, w - status_w, status_w > 0.0);
    }
    // Workspaces: the panels laid out for a task.
    let spaces = crate::workspaces::all();
    let space_keys: Vec<String> = crate::workspaces::ACTIONS.iter().map(|(id, _)| crate::keymap::keys(&app.prefs, id).unwrap_or("").to_owned()).collect();
    let mut pick: Option<crate::workspaces::Workspace> = None;
    let mut save_as: Option<String> = None;
    let mut move_panel: Option<(crate::dock::Panel, Rect)> = None;
    let mut reset = false;
    let max_h = (ctx.content_rect().bottom() - lresp.rect.bottom() - 24.0).max(160.0);
    let app_ro: &StudioApp = app;
    kit::popover(&ctx, layout_id, lresp.rect, kit::Side::BelowRight, 220.0, |ui| {
        egui::ScrollArea::vertical().id_salt("layout-menu-scroll").max_height(max_h).show(ui, |ui| {
        kit::menu_heading(ui, "Panels");
        for (p, on, keys) in &panels {
            if MenuItem::new(p.name()).checked(*on).shortcut(keys).show(ui).clicked() {
                toggle = Some(*p);
            }
        }
        kit::menu_heading(ui, "Move panel");
        if let Some(m) = super::dock_ui::move_panel_items(app_ro, ui) {
            move_panel = Some(m);
        }
        kit::menu_heading(ui, "Workspaces");
        for (i, w) in spaces.iter().enumerate() {
            if MenuItem::new(&w.name).shortcut(space_keys.get(i).map_or("", String::as_str)).show(ui).clicked() {
                pick = Some(w.clone());
            }
        }
        let id = egui::Id::unique("save-workspace-name");
        let mut name: String = ui.ctx().data(|d| d.get_temp(id)).unwrap_or_default();
        let resp = kit::text_field(ui, &mut name, "Save layout as…", None, 196.0, None);
        if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && !name.trim().is_empty() {
            save_as = Some(std::mem::take(&mut name));
        }
        ui.ctx().data_mut(|d| d.insert_temp(id, name));
        kit::menu_separator(ui);
        if MenuItem::new("Reset layout").icon(Icon::ArrowCounterClockwise).show(ui).clicked() {
            reset = true;
        }
        });
    });
    if let Some(p) = toggle {
        super::dock_ui::toggle_panel(app, &ctx, p, narrow);
    }
    if let Some((p, anchor)) = move_panel {
        kit::set_open(&ctx, layout_id, false);
        super::dock_ui::open_menu(&ctx, p, anchor, true);
    }
    if reset {
        kit::set_open(&ctx, layout_id, false);
        super::dock_ui::reset(app, &ctx);
    }
    if let Some(w) = pick {
        crate::workspaces::switch(app, &ctx, &w);
    }
    if let Some(name) = save_as {
        let w = crate::workspaces::current(app, &name);
        match crate::workspaces::save(w) {
            Ok(()) => kit::push(&ctx, format!("Saved the layout as \"{}\"", name.trim()), None, Some(Icon::SquaresFour)),
            Err(e) => kit::push(&ctx, e, None, Some(Icon::Warning)),
        };
    }
    if let Some(a) = action {
        run(app, &ctx, a);
    }
    let _ = t;
}

/// The workspace tabs: icons with the names as tooltips, in the top bar when
/// there is `room` for them (the Layout menu always has them).
fn workspace_switcher(app: &mut StudioApp, ui: &mut egui::Ui, room: f32) {
    // The built-in workspaces; the person's own are in the Layout menu.
    let spaces = crate::workspaces::builtin();
    if room < 6.0 + 32.0 * spaces.len() as f32 + 4.0 {
        return;
    }
    ui.add_space(6.0);
    let current = crate::workspaces::saved(app).map(|w| w.name);
    let mut i = current.and_then(|n| spaces.iter().position(|w| w.name.eq_ignore_ascii_case(&n))).unwrap_or(0);
    let opts: Vec<kit::Segment> = spaces.iter().map(|w| kit::Segment::icon(crate::workspaces::icon(&w.name), &w.name)).collect();
    let tabs = kit::segmented(ui, "workspace-tabs", &mut i, &opts, None, None);
    crate::onboarding::note_target(ui.ctx(), "workspace-tabs", tabs.rect);
    if tabs.changed() {
        let ctx = ui.ctx().clone();
        crate::workspaces::switch(app, &ctx, &spaces[i]);
    }
}

fn divider(ui: &mut egui::Ui) {
    let (r, _) = ui.allocate_exact_size(Vec2::new(9.0, 20.0), Sense::hover());
    kit::vline(ui, r.center().x, r.top()..=r.bottom());
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
    let status = match (&app.doc.folder, app.doc.dirty()) {
        (None, _) => "Not saved",
        (Some(_), true) => "Edited",
        (Some(_), false) => "Saved",
    };
    if !with_status {
        return;
    }
    ui.label(egui::RichText::new(status).font(theme::caption()).color(t.text_2));
}

struct Item {
    label: &'static str,
    /// Its keys: an action's from the keymap, or a fixed key.
    shortcut: String,
    icon: Option<Icon>,
    enabled: bool,
    action: fn() -> Option<Action>,
}

fn item(label: &'static str, shortcut: &str, icon: Option<Icon>, enabled: bool, action: fn() -> Option<Action>) -> Option<Item> {
    Some(Item { label, shortcut: shortcut.to_owned(), icon, enabled, action })
}

fn menus(app: &StudioApp) -> Vec<(&'static str, Vec<Option<Item>>)> {
    let sel = !app.editor.selection.is_empty();
    let d = &app.doc;
    let k = |id: &str| crate::keymap::hint(&app.prefs, id);
    vec![
        ("File", vec![
            item("Open…", "⌘O", Some(Icon::FolderOpen), true, || Some(Action::Open)),
            item("Save", "⌘S", None, true, || Some(Action::Save)),
            item("Save as…", "⇧⌘S", None, true, || Some(Action::SaveAs)),
            None,
            item("Import media…", "⌘I", Some(Icon::DownloadSimple), true, || Some(Action::ImportMedia)),
            item("Add sample media", "", None, true, || Some(Action::SampleMedia)),
            item("Import timeline…", "", None, true, || Some(Action::ImportTimeline)),
            item("Import script…", "", None, true, || Some(Action::ImportScript)),
            None,
            item("Export OpenTimelineIO…", "", None, true, || Some(Action::ExportOtio)),
            item("Export…", &k("export"), Some(Icon::Export), true, || Some(Action::Export)),
        ]),
        ("Edit", vec![
            item("Undo", &k("undo"), Some(Icon::ArrowUUpLeft), d.can_undo(), || Some(Action::Undo)),
            item("Redo", &k("redo"), Some(Icon::ArrowUUpRight), d.can_redo(), || Some(Action::Redo)),
            None,
            item("Select all", &k("select-all"), None, true, || Some(Action::SelectAll)),
            item("Duplicate", &k("duplicate"), Some(Icon::Copy), sel, || Some(Action::Duplicate)),
            item("Delete", &k("delete"), Some(Icon::Trash), sel, || Some(Action::Delete)),
        ]),
        ("Clip", vec![
            item("Split at playhead", &k("split"), Some(Icon::Scissors), true, || Some(Action::Split)),
            item("Add marker", &k("marker"), Some(Icon::Flag), true, || Some(Action::Marker)),
            None,
            item("Set in point", &k("in"), None, true, || Some(Action::RangeIn)),
            item("Set out point", &k("out"), None, true, || Some(Action::RangeOut)),
            item("Clear range", &k("clear-range"), None, d.project.range.is_some(), || Some(Action::ClearRange)),
        ]),
        ("View", vec![
            item("Zoom in", &k("zoom-in"), Some(Icon::MagnifyingGlassPlus), true, || Some(Action::ZoomIn)),
            item("Zoom out", &k("zoom-out"), Some(Icon::MagnifyingGlassMinus), true, || Some(Action::ZoomOut)),
            item("Fit sequence", &k("fit"), Some(Icon::ArrowsOutLineHorizontal), true, || Some(Action::Fit)),
        ]),
        ("Help", vec![
            item("Search help", "F1", Some(Icon::Question), true, || Some(Action::Help)),
            item("Welcome tour", "", Some(Icon::Sparkle), true, || Some(Action::Tour)),
            item("Keyboard shortcuts", "", Some(Icon::Keyboard), true, || Some(Action::Shortcuts)),
            None,
            item("Check for updates…", "", Some(Icon::DownloadSimple), true, || Some(Action::CheckUpdates)),
        ]),
    ]
}

/// Where the menu button `name` was drawn last frame.
pub(crate) fn menu_rect_id(name: &str) -> egui::Id {
    egui::Id::new(("topbar-menu-rect", name.to_owned()))
}

/// A menu of `items`; the View menu also lists every dock panel in `panels`,
/// checked when open, and returns the one chosen.
fn menu(ui: &mut egui::Ui, name: &str, items: &[Option<Item>], panels: &[(crate::dock::Panel, bool, String)]) -> (Option<Action>, Option<crate::dock::Panel>) {
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
    let mut out = None;
    let mut chosen = None;
    kit::popover(ui.ctx(), id, r, kit::Side::Below, 248.0, |ui| {
        for it in items {
            match it {
                None => kit::menu_separator(ui),
                Some(it) => {
                    let mut m = MenuItem::new(it.label).enabled(it.enabled);
                    if !it.shortcut.is_empty() {
                        m = m.shortcut(&it.shortcut);
                    }
                    m = match it.icon {
                        Some(i) => m.icon(i),
                        // Keep labels aligned with rows that have icons.
                        None => m.reserve_icon(),
                    };
                    if m.show(ui).clicked() {
                        out = (it.action)();
                        kit::set_open(ui.ctx(), id, false);
                    }
                }
            }
        }
        if name == "View" {
            kit::menu_separator(ui);
            kit::menu_heading(ui, "Panels");
            for (p, on, keys) in panels {
                if MenuItem::new(p.name()).checked(*on).shortcut(keys).show(ui).clicked() {
                    chosen = Some(*p);
                    kit::set_open(ui.ctx(), id, false);
                }
            }
        }
    });
    (out, chosen)
}

/// Jobs button: a spinner while anything runs; opens the jobs popover.
fn jobs_button(jobs: &mut Jobs, ui: &mut egui::Ui) {
    let t = theme::tokens(ui.ctx());
    let running = jobs.running();
    let id = ui.scope_id().with("jobs");
    let resp = IconButton::new(Icon::Bell, if running > 0 { "Background jobs (running)" } else { "Background jobs" }).active(kit::is_open(ui.ctx(), id)).show(ui);
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
        kit::menu_heading(ui, "Background jobs");
        #[cfg(feature = "render")]
        crate::screens::queue::section(ui);
        if jobs.list.is_empty() {
            let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 72.0), Sense::hover());
            icons::paint(ui.painter(), Rect::from_center_size(r.center() - Vec2::new(0.0, 12.0), Vec2::splat(20.0)), Icon::CheckCircle, 20.0, t.text_2);
            ui.painter().text(r.center() + Vec2::new(0.0, 14.0), Align2::CENTER_CENTER, crate::i18n::shown("Nothing running. Exports and AI tasks show here."), theme::body(), t.text_2);
            return;
        }
        let mut cancel = None;
        for j in jobs.list.iter().rev().take(6) {
            let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 52.0), Sense::hover());
            let p = ui.painter();
            kit::paint_text_clipped(p, egui::pos2(r.left() + 8.0, r.top() + 14.0), Align2::LEFT_CENTER, &j.name, theme::control_medium(), t.text, r.width() - 50.0);
            let status = if j.cancelled { "Cancelled".to_owned() } else if j.done { "Done".to_owned() } else { format!("{} · {}%  ·  {}", j.detail, (j.progress * 100.0) as u32, Jobs::eta(j)) };
            kit::paint_text_clipped(p, egui::pos2(r.left() + 8.0, r.top() + 32.0), Align2::LEFT_CENTER, &status, theme::caption(), t.text_2, r.width() - 50.0);
            let bar = Rect::from_min_size(egui::pos2(r.left() + 8.0, r.bottom() - 6.0), Vec2::new(r.width() - 50.0, 3.0));
            p.rect_filled(bar, CornerRadius::same(2), t.raised);
            p.rect_filled(Rect::from_min_size(bar.min, Vec2::new(bar.width() * j.progress, 3.0)), CornerRadius::same(2), if j.cancelled { t.text_2 } else { t.accent });
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
}
