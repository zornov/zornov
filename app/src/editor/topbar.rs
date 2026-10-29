// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Editor top bar, left to right: Home and the menus, the workspace tabs
//! (a dropdown when the window is too narrow for them), then the project's
//! name and save state, the Jobs bell and Export. Panels, workspaces and the
//! layout are in the View menu; Settings is ⌘, and File ▸ Settings.

use super::{Action, run};
use crate::app::{Jobs, StudioApp};
use crate::cli::Screen;
use crate::dock::Panel;
use crate::icons::{self, Icon};
use crate::kit::{self, Button, IconButton, MenuItem};
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
    jobs_button(&mut app.jobs, &mut right);
    let right_start = right.min_rect().left();

    let mut bar = ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(egui::Layout::left_to_right(egui::Align::Center)));
    bar.spacing_mut().item_spacing.x = 4.0;
    if IconButton::new(Icon::House, "All projects").shortcut(&home_keys).show(&mut bar).clicked() {
        app.screen = Screen::Start;
    }
    bar.add_space(2.0);

    // Every panel that can be docked, checked when it is open.
    let panels: Vec<(Panel, bool, String)> = Panel::ALL
        .into_iter()
        .filter(|p| p.closable())
        .map(|p| (p, super::dock_ui::panel_shown(app, &ctx, p, narrow), crate::keymap::keys(&app.prefs, &format!("panel-{}", p.key())).unwrap_or("").to_owned()))
        .collect();
    let spaces = crate::workspaces::all();
    let current = crate::workspaces::saved(app).and_then(|w| spaces.iter().position(|s| s.name.eq_ignore_ascii_case(&w.name)));
    let space_keys: Vec<String> = crate::workspaces::ACTIONS.iter().map(|(id, _)| crate::keymap::keys(&app.prefs, id).unwrap_or("").to_owned()).collect();
    let view = ViewMenu { spaces: &spaces, current, space_keys: &space_keys, panels: &panels, maximised: app.editor.maximised.is_some(), max_h: (ctx.content_rect().bottom() - r.bottom() - 24.0).max(160.0) };
    let mut out = Picked::default();
    for (name, items) in menus(app) {
        menu(&mut bar, name, &items, Some(&view).filter(|_| name == "View"), &mut out);
    }

    // The workspace tabs when they leave the project's name its room,
    // otherwise one dropdown with the same list.
    let left_end = bar.min_rect().right();
    let gap = (right_start - left_end - 12.0).max(0.0);
    let tabs_w = tabs_width(&bar, &spaces);
    let tabs_fit = gap - tabs_w >= NAME_MIN_W;
    bar.add_space(6.0);
    let used = if tabs_fit {
        workspace_tabs(app, &mut bar, &spaces, current, &mut out);
        tabs_w
    } else {
        workspace_menu(app, &mut bar, &spaces, current, &mut out);
        DROPDOWN_W
    };

    // Right-aligned, against the Jobs bell: the name gives way first, and
    // the save state ("Edited") before it.
    let room = (gap - used - 6.0).max(0.0);
    let name_w = kit::text_width(ui, &app.doc.project.name, theme::control_medium()).min(240.0) + 16.0;
    let status_w = if room >= name_w.min(120.0) + 60.0 { 60.0 } else { 0.0 };
    let w = (name_w + status_w).min(room);
    if w >= 40.0 {
        let slot = Rect::from_min_size(egui::pos2(right_start - 12.0 - w, r.top()), Vec2::new(w, r.height()));
        let mut cui = ui.new_child(egui::UiBuilder::new().max_rect(slot).layout(egui::Layout::left_to_right(egui::Align::Center)));
        cui.set_clip_rect(slot.intersect(ui.clip_rect()));
        cui.spacing_mut().item_spacing.x = 2.0;
        project_name(app, &mut cui, w - status_w, status_w > 0.0);
    }

    if let Some(p) = out.panel {
        super::dock_ui::toggle_panel(app, &ctx, p, narrow);
    }
    if out.reset {
        super::dock_ui::reset(app, &ctx);
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
    if let Some(a) = out.action {
        run(app, &ctx, a);
    }
}

/// What the top bar's controls asked for this frame.
#[derive(Default)]
struct Picked {
    action: Option<Action>,
    panel: Option<Panel>,
    workspace: Option<Workspace>,
    paint: bool,
    save_as: Option<String>,
    reset: bool,
}

/// What the View menu lists besides its own commands.
struct ViewMenu<'a> {
    spaces: &'a [Workspace],
    current: Option<usize>,
    space_keys: &'a [String],
    panels: &'a [(Panel, bool, String)],
    maximised: bool,
    max_h: f32,
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
fn workspace_tabs(app: &mut StudioApp, ui: &mut egui::Ui, spaces: &[Workspace], current: Option<usize>, out: &mut Picked) {
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
    let _ = app;
}

/// Wide enough for "Motion" and a caret.
const DROPDOWN_W: f32 = 132.0;

/// The same list as a dropdown, for a window too narrow for the tabs.
fn workspace_menu(app: &mut StudioApp, ui: &mut egui::Ui, spaces: &[Workspace], current: Option<usize>, out: &mut Picked) {
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
    let _ = app;
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
            item("Project settings…", "", Some(Icon::Faders), true, || Some(Action::ProjectSettings)),
            None,
            item("Export OpenTimelineIO…", "", None, true, || Some(Action::ExportOtio)),
            item("Export…", &k("export"), Some(Icon::Export), true, || Some(Action::Export)),
            None,
            item("Settings…", &k("settings"), Some(Icon::GearSix), true, || Some(Action::Settings)),
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
        // What used to be the Production and Library buttons.
        ("Project", vec![
            item("Production…", "", Some(Icon::FilmSlate), true, || Some(Action::Production)),
            item("Project health…", "", Some(Icon::LockSimple), true, || Some(Action::ProjectHealth)),
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

/// A menu of `items`. The View menu goes on with the workspaces, the panels
/// (checked when open) and the layout commands (`view`); what is chosen goes in `out`.
fn menu(ui: &mut egui::Ui, name: &str, items: &[Option<Item>], view: Option<&ViewMenu>, out: &mut Picked) {
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
    kit::popover(ui.ctx(), id, r, kit::Side::Below, 248.0, |ui| {
        let scroll = view.map_or(f32::INFINITY, |v| v.max_h);
        egui::ScrollArea::vertical().id_salt(("menu-scroll", name)).max_height(scroll).show(ui, |ui| {
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
                            out.action = (it.action)();
                            kit::set_open(ui.ctx(), id, false);
                        }
                    }
                }
            }
            if let Some(v) = view {
                view_sections(ui, id, v, out);
            }
        });
    });
}

/// The View menu's workspaces, panels and layout: the one place that lists
/// them all (the tabs are the quick way to the built-in workspaces).
fn view_sections(ui: &mut egui::Ui, menu_id: egui::Id, v: &ViewMenu, out: &mut Picked) {
    kit::menu_separator(ui);
    kit::menu_heading(ui, "Workspaces");
    for (i, w) in v.spaces.iter().enumerate() {
        if MenuItem::new(&w.name).checked(v.current == Some(i)).shortcut(v.space_keys.get(i).map_or("", String::as_str)).show(ui).clicked() {
            out.workspace = Some(w.clone());
            kit::set_open(ui.ctx(), menu_id, false);
        }
    }
    kit::menu_heading(ui, "Panels");
    for (p, on, keys) in v.panels {
        if MenuItem::new(p.name()).checked(*on).shortcut(keys).show(ui).clicked() {
            out.panel = Some(*p);
            kit::set_open(ui.ctx(), menu_id, false);
        }
    }
    kit::menu_heading(ui, "Layout");
    if MenuItem::new("Maximise panel").checked(v.maximised).shortcut("⌘\\").icon(Icon::CornersOut).show(ui).clicked() {
        out.action = Some(Action::Maximise);
        kit::set_open(ui.ctx(), menu_id, false);
    }
    let id = egui::Id::unique("save-workspace-name");
    let mut name: String = ui.ctx().data(|d| d.get_temp(id)).unwrap_or_default();
    let resp = kit::text_field(ui, &mut name, "Save layout as…", None, 196.0, None);
    if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && !name.trim().is_empty() {
        out.save_as = Some(std::mem::take(&mut name));
        kit::set_open(ui.ctx(), menu_id, false);
    }
    ui.ctx().data_mut(|d| d.insert_temp(id, name));
    if MenuItem::new("Reset layout").icon(Icon::ArrowCounterClockwise).show(ui).clicked() {
        out.reset = true;
        kit::set_open(ui.ctx(), menu_id, false);
    }
}

/// Jobs button: a spinner while anything runs; opens the jobs popover.
fn jobs_button(jobs: &mut Jobs, ui: &mut egui::Ui) {
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
