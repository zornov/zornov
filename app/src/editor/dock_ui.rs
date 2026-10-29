// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Docking in the editor window: the dock tree (`crate::dock`) drawn as
//! tiles with splitters between them, a tab strip over stacks of two or
//! more panels, and panels moved by dragging their header or tab onto
//! another panel (left, right, above, below, or into it as a tab) or onto
//! the window's edge. A drop overlay shows where the panel will land; Esc
//! cancels. The same moves are in each panel's "Move panel to…" menu
//! (right-click a header or tab, or ⇧F10 on a focused region), so docking
//! works from the keyboard and a screen reader. Double-click a header, or
//! ⌘\, to maximise a panel; again to restore.
//!
//! Review, Record, Editorial, Scopes and Sound keep their own open flag; the
//! flag follows the tree, and turning it on or off (its button or shortcut)
//! opens or closes the docked panel. A panel never opens over the editor:
//! it docks as a tab or beside its zone (Sound beside the Timeline, Scopes
//! beside the Viewer). Only "Float in a window" makes one float.

use super::{GAP, TOPBAR_H};
use crate::a11y::Region;
use crate::app::StudioApp;
use crate::dock::{Axis, Layout, Metrics, Panel, Tile, Tree, Zone};
use crate::icons::{self, Icon};
use crate::kit::{self, IconButton, MenuItem};
use crate::theme;
use eframe::egui::{self, Align2, CornerRadius, Id, Pos2, Rect, Sense, Stroke, Vec2};

/// A tab strip's height.
pub const TAB_H: f32 = 30.0;
/// How close to the window's edge a drop docks along the whole edge.
const EDGE: f32 = 14.0;

/// A panel being dragged to a new place.
#[derive(Clone, Debug, PartialEq)]
pub struct DockDrag {
    pub panel: Panel,
}

/// Where a drop would put the dragged panel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Drop {
    /// Beside a panel's stack, or along the window edge (`None`), in
    /// screen terms (mirrored to the tree's in a right-to-left window).
    Dock(Option<Panel>, Zone),
    /// Among a stack's tabs, before the tab at `index`.
    Tab(Panel, usize),
}

pub fn metrics() -> Metrics {
    Metrics { gap: GAP, tab_h: TAB_H }
}

/// Panels kept out of the layout for now: the Inspector is a sheet in a
/// narrow window.
pub fn hidden(narrow: bool) -> impl Fn(Panel) -> bool {
    move |p| narrow && p == Panel::Inspector
}

/// The area under the top bar the panels share.
pub fn body(full: Rect) -> Rect {
    Rect::from_min_max(egui::pos2(full.left() + GAP, full.top() + TOPBAR_H), full.max - Vec2::splat(GAP))
}

/// The accessibility region a panel is announced as.
pub fn region(p: Panel) -> Region {
    match p {
        Panel::Library => Region::Library,
        Panel::Viewer => Region::Viewer,
        Panel::Inspector => Region::Inspector,
        Panel::Timeline => Region::Timeline,
        Panel::Sound => Region::Sound,
        Panel::Scopes => Region::Scopes,
        Panel::Review => Region::Review,
        Panel::Record => Region::Record,
        Panel::Editorial => Region::Editorial,
        Panel::Ai => Region::Ai,
    }
}

/// The height of a panel's own header (its toolbar or title row), which
/// doubles as the handle to drag it by.
fn header_h(p: Panel) -> f32 {
    match p {
        Panel::Library => super::library::TABS_H,
        Panel::Viewer => super::viewer::TOOLBAR_H,
        Panel::Inspector => theme::space::PANEL_PAD + 28.0,
        Panel::Timeline => super::timeline::TOOLBAR_H,
        Panel::Sound => 38.0,
        Panel::Scopes => 40.0,
        Panel::Review | Panel::Record | Panel::Editorial => 36.0,
        Panel::Ai => 44.0,
    }
}

fn docked_id(p: Panel) -> Id {
    Id::unique(("dock-docked", p.key()))
}

/// Whether `p` is docked this frame (its own window or overlay then stays
/// away; the dock draws it).
pub fn docked(ctx: &egui::Context, p: Panel) -> bool {
    ctx.data(|d| d.get_temp::<bool>(docked_id(p))).unwrap_or(false)
}

/// A panel's own open flag, for the panels that also show undocked.
fn flag(app: &StudioApp, ctx: &egui::Context, p: Panel) -> Option<bool> {
    let _ = app;
    match p {
        Panel::Sound => Some(super::mixer::is_on(ctx)),
        Panel::Scopes => Some(super::scopes::is_on(ctx)),
        Panel::Review => Some(super::review::is_open()),
        Panel::Record => Some(super::capture::is_open()),
        Panel::Editorial => Some(app.editor.cutting.panel.open),
        _ => None,
    }
}

fn set_flag(app: &mut StudioApp, ctx: &egui::Context, p: Panel, on: bool) {
    match p {
        Panel::Sound => super::mixer::set_on(ctx, on),
        Panel::Scopes => super::scopes::set_on(ctx, on),
        Panel::Review => super::review::set_open(on),
        Panel::Record => super::capture::set_open(on),
        Panel::Editorial => app.editor.cutting.panel.open = on,
        _ => {}
    }
}

const FLAGGED: [Panel; 5] = [Panel::Sound, Panel::Scopes, Panel::Review, Panel::Record, Panel::Editorial];

fn flag_slot(p: Panel) -> usize {
    FLAGGED.iter().position(|q| *q == p).unwrap_or(0)
}

/// Keeps the open flags of the panels that can also show undocked in step
/// with the tree: a flag turned on or off since last frame opens or closes
/// the docked panel; otherwise the flag follows the tree. A panel closed out
/// of the dock is off, not suddenly floating.
fn sync_flags(app: &mut StudioApp, ctx: &egui::Context) {
    let lay = app.editor.last_layout();
    for p in FLAGGED {
        let Some(now) = flag(app, ctx, p) else { continue };
        restore_floated(app, ctx, p, now);
        let now = flag(app, ctx, p).unwrap_or(now);
        let slot = flag_slot(p);
        let last = app.editor.dock_flags[slot];
        let ed = &mut app.editor;
        if ed.dock.contains(p) || ed.dock.is_popped(p) {
            if last.is_some_and(|l| l != now) {
                if now {
                    ed.dock.open(p, &lay);
                } else if ed.dock.is_open(p) && !ed.dock.is_showing(p) {
                    // Behind another tab: bring it forward rather than close it.
                    ed.dock.activate(p);
                } else {
                    ed.dock.close(p);
                }
            } else if last.is_none() && now && !ed.dock.is_open(p) {
                ed.dock.open(p, &lay);
            }
            let open = ed.dock.is_shown_anywhere(p);
            ed.dock_flags[slot] = Some(open);
            if open != now {
                set_flag(app, ctx, p, open);
            }
        } else {
            ed.dock_flags[slot] = None;
            if last == Some(true) && now {
                // Docked last frame and closed out of the dock since.
                set_flag(app, ctx, p, false);
            } else if now && !floated(ctx, p) {
                // Turned on by its own button, shortcut or tool: it opens
                // docked as a tab or beside its zone, not over the editor.
                ed.dock.open(p, &lay);
                ed.dock_flags[slot] = Some(true);
            } else if !now {
                set_floated(ctx, p, false);
            }
        }
        // Docked or in a window of its own: its floating window stays away.
        let docked = app.editor.dock.contains(p) || app.editor.dock.is_popped(p);
        ctx.data_mut(|d| d.insert_temp(docked_id(p), docked));
    }
}

/// Opens a panel wherever it goes: docked (where it was or its default
/// place), or the Inspector's sheet in a narrow window.
pub fn open_panel(app: &mut StudioApp, ctx: &egui::Context, p: Panel, narrow: bool) {
    if narrow && p == Panel::Inspector {
        app.editor.sheet_open = true;
        return;
    }
    if app.editor.dock.is_popped(p) {
        // In its own window: bring that window forward.
        ctx.send_viewport_cmd_to(viewport_id(p), egui::ViewportCommand::Focus);
        return;
    }
    let lay = app.editor.last_layout();
    app.editor.dock.open(p, &lay);
    if FLAGGED.contains(&p) {
        set_flag(app, ctx, p, true);
        app.editor.dock_flags[flag_slot(p)] = Some(true);
    }
}

/// Closes a panel (its window too, for a floating one).
pub fn close_panel(app: &mut StudioApp, ctx: &egui::Context, p: Panel, narrow: bool) {
    if narrow && p == Panel::Inspector {
        app.editor.sheet_open = false;
        return;
    }
    if app.editor.dock.contains(p) || app.editor.dock.is_popped(p) {
        app.editor.dock.close(p);
    }
    // The keys were that panel's: they go back to the editor.
    reset_key_scope(ctx);
    if FLAGGED.contains(&p) {
        set_flag(app, ctx, p, false);
        app.editor.dock_flags[flag_slot(p)] = if app.editor.dock.contains(p) { Some(false) } else { None };
    }
}

/// Opens `p` when it is closed, closes it when it is open.
pub fn toggle_panel(app: &mut StudioApp, ctx: &egui::Context, p: Panel, narrow: bool) {
    if panel_shown(app, ctx, p, narrow) {
        close_panel(app, ctx, p, narrow);
    } else {
        open_panel(app, ctx, p, narrow);
    }
}

/// Whether the Panels menu shows `p` checked.
pub fn panel_shown(app: &StudioApp, ctx: &egui::Context, p: Panel, narrow: bool) -> bool {
    if narrow && p == Panel::Inspector {
        return app.editor.sheet_open;
    }
    if app.editor.dock.contains(p) || app.editor.dock.is_popped(p) {
        return app.editor.dock.is_shown_anywhere(p);
    }
    // Undocked: the Sound panel in the Timeline's place, the scopes over
    // the picture, Review and Record in their windows.
    flag(app, ctx, p).unwrap_or(false)
}

/// Back to the layout Studio comes with.
pub fn reset(app: &mut StudioApp, ctx: &egui::Context) {
    app.editor.set_tree(Tree::default());
    crate::a11y::announce(ctx, crate::i18n::t("Layout reset").into_owned());
}

fn floated_id(p: Panel) -> Id {
    Id::unique(("dock-floated", p.key()))
}

/// Whether the person floated `p` ("Float in a window") and it is still open.
fn floated(ctx: &egui::Context, p: Panel) -> bool {
    ctx.data_mut(|d| d.get_persisted::<bool>(floated_id(p))).unwrap_or(false)
}

/// Kept with the window's memory, so a panel floated when Zornov was quit
/// is floated again on the next start.
fn set_floated(ctx: &egui::Context, p: Panel, on: bool) {
    ctx.data_mut(|d| d.insert_persisted(floated_id(p), on));
}

fn restored_id(p: Panel) -> Id {
    Id::unique(("dock-float-restored", p.key()))
}

/// First frame of a run: a panel that was floated last time and is not on
/// yet is turned on, so it comes back where it was. Once per panel per run.
fn restore_floated(app: &mut StudioApp, ctx: &egui::Context, p: Panel, now: bool) {
    let done = ctx.data(|d| d.get_temp::<bool>(restored_id(p))).unwrap_or(false);
    if done {
        return;
    }
    ctx.data_mut(|d| d.insert_temp(restored_id(p), true));
    if !now && floated(ctx, p) {
        set_flag(app, ctx, p, true);
    }
}

/// Takes `p` out of the dock into its own window.
fn float(app: &mut StudioApp, ctx: &egui::Context, p: Panel) {
    set_floated(ctx, p, true);
    app.editor.dock.remove(p);
    app.editor.dock.popped.retain(|w| w.panel != p);
    app.editor.dock.parked.retain(|(q, _)| *q != p);
    app.editor.dock_flags[flag_slot(p)] = None;
    set_flag(app, ctx, p, true);
}

fn menu_id(p: Panel) -> Id {
    Id::unique(("dock-move-menu", p.key()))
}

fn menu_anchor_id(p: Panel) -> Id {
    Id::unique(("dock-move-anchor", p.key()))
}

/// Opens `p`'s "Move panel to…" menu under `anchor` (with keyboard focus
/// on its first button when `focus` is set).
pub fn open_menu(ctx: &egui::Context, p: Panel, anchor: Rect, focus: bool) {
    ctx.data_mut(|d| {
        d.insert_temp(menu_anchor_id(p), anchor);
        d.insert_temp(Id::unique("dock-menu-focus"), focus);
    });
    kit::set_open(ctx, menu_id(p), true);
}

/// Where the keyboard is: the editor (timeline and viewer), a docked panel or
/// a window floating over the editor. It follows the last click, so the keys
/// act on the panel the person is working in.
#[derive(Clone, Copy, Debug, PartialEq)]
enum KeyScope {
    Editor,
    Panel(Panel),
    Floating(egui::LayerId),
}

fn scope_id() -> Id {
    Id::unique("dock-key-scope")
}

fn rects_id() -> Id {
    Id::unique("dock-panel-rects")
}

/// The panels whose keys are their own: they work on a selection of their
/// own (media, parameters, mixer strips, notes) and never on the timeline's.
/// The Timeline and Viewer keep the editor's shortcuts, and the Scopes have no keys.
fn keeps_own_keys(p: Panel) -> bool {
    !matches!(p, Panel::Timeline | Panel::Viewer | Panel::Scopes)
}

/// Gives the keyboard back to the editor (a panel closed, the layout changed).
pub fn reset_key_scope(ctx: &egui::Context) {
    ctx.data_mut(|d| d.insert_temp(scope_id(), KeyScope::Editor));
}

/// The keyboard's owner after `scope` when the panels now drawn are `shown`:
/// a panel that is no longer on screen cannot keep the keys.
fn live_scope(scope: KeyScope, shown: &[Panel]) -> KeyScope {
    match scope {
        KeyScope::Panel(p) if !shown.contains(&p) => KeyScope::Editor,
        s => s,
    }
}

/// Follows clicks to find the keyboard's owner, and tells the key dispatcher
/// (`crate::keymap::claim`) when it is not the editor.
fn track_key_scope(ctx: &egui::Context) {
    let press = ctx.input(|i| if i.pointer.primary_pressed() { i.pointer.interact_pos() } else { None });
    if let Some(pos) = press {
        let rects: Vec<(Panel, Rect)> = ctx.data(|d| d.get_temp(rects_id())).unwrap_or_default();
        let scope = match ctx.layer_id_at(pos) {
            Some(layer) if layer.order == egui::Order::Middle => KeyScope::Floating(layer),
            _ => rects.iter().find(|(_, r)| r.contains(pos)).map_or(KeyScope::Editor, |(p, _)| KeyScope::Panel(*p)),
        };
        ctx.data_mut(|d| d.insert_temp(scope_id(), scope));
    }
    let scope = ctx.data(|d| d.get_temp::<KeyScope>(scope_id())).unwrap_or(KeyScope::Editor);
    let shown: Vec<Panel> = ctx.data(|d| d.get_temp::<Vec<(Panel, Rect)>>(rects_id())).unwrap_or_default().into_iter().map(|(p, _)| p).collect();
    let scope = live_scope(scope, &shown);
    use crate::keymap::{KeyOwner, claim};
    match scope {
        // A window that is still open: what is typed there is its own.
        KeyScope::Floating(layer) if ctx.memory(|m| m.areas().is_visible(&layer)) => claim(ctx, KeyOwner::Text),
        KeyScope::Panel(p) if keeps_own_keys(p) => claim(ctx, KeyOwner::Panel),
        // The node and curve editors replace the timeline's lanes: Delete,
        // ⌘A and the letters are theirs while they show.
        KeyScope::Panel(Panel::Timeline) if super::graph::is_on(ctx) || super::comp::is_on(ctx) => claim(ctx, KeyOwner::Panel),
        _ => {}
    }
    // Keyboard focus in a panel (F6, Tab) counts the same as a click there.
    if let Some(p) = crate::a11y::focused_region(ctx).and_then(|r| Panel::ALL.into_iter().find(|p| region(*p) == r))
        && keeps_own_keys(p)
    {
        claim(ctx, KeyOwner::Panel);
    }
}

/// The panel the person last clicked in, if it is docked.
fn focused_panel(ctx: &egui::Context) -> Option<Panel> {
    match ctx.data(|d| d.get_temp::<KeyScope>(scope_id())) {
        Some(KeyScope::Panel(p)) => Some(p),
        _ => None,
    }
}

/// View ▸ Maximise panel: the panel being worked in (the Timeline when none
/// is) fills the whole dock; again, and the layout is back.
pub fn toggle_maximised(app: &mut StudioApp, ctx: &egui::Context) {
    toggle_maximised_panel(app, ctx, focused_panel(ctx).unwrap_or(Panel::Timeline));
}

fn toggle_maximised_panel(app: &mut StudioApp, ctx: &egui::Context, p: Panel) {
    let ed = &mut app.editor;
    if ed.maximised.is_some() {
        ed.maximised = None;
    } else if ed.dock.is_open(p) {
        ed.dock.activate(p);
        ed.maximised = Some(p);
    }
    let said = if ed.maximised.is_some() { "Panel maximised" } else { "Panel restored" };
    crate::a11y::announce(ctx, crate::i18n::t(said).into_owned());
}

/// The layout with only `p`'s stack, filling the body.
fn maximised_layout(lay: &Layout, p: Panel) -> Option<Layout> {
    let tile = lay.tile_of(p)?;
    let mut tile = tile.clone();
    tile.rect = lay.body;
    tile.active = p;
    Some(Layout { tiles: vec![tile], splitters: Vec::new(), rects: Vec::new(), body: lay.body, metrics: lay.metrics })
}

/// Keys: Esc cancels a drag; ⇧F10 opens the focused region's Move menu;
/// ⌘\ maximises the panel being worked in.
pub fn keys(app: &mut StudioApp, ctx: &egui::Context) {
    track_key_scope(ctx);
    let maximise = egui::KeyboardShortcut::new(egui::Modifiers { command: true, mac_cmd: cfg!(target_os = "macos"), ctrl: !cfg!(target_os = "macos"), ..egui::Modifiers::NONE }, egui::Key::Backslash);
    if app.overlay == crate::app::Overlay::None && ctx.input_mut(|i| i.consume_shortcut(&maximise)) {
        toggle_maximised(app, ctx);
    }
    if app.editor.dock_drag.is_some() && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
        app.editor.dock_drag = None;
        ctx.data_mut(|d| d.insert_temp(Id::unique("dock-drag-cancelled"), true));
    }
    if let Some(r) = crate::a11y::focused_region(ctx)
        && ctx.input_mut(|i| i.consume_key(egui::Modifiers::SHIFT, egui::Key::F10))
        && let Some(p) = Panel::ALL.into_iter().find(|p| region(*p) == r)
        && let Some(lay) = app.editor.dock_layout.as_ref()
        && let Some(rect) = lay.tile_of(p).map(|t| on_screen(lay, t.rect))
    {
        open_menu(ctx, p, Rect::from_min_size(rect.min, Vec2::new(rect.width(), header_h(p))), true);
    }
}

/// Draws the docked panels in `body` and handles dragging, splitters and
/// the Move menus.
pub fn show(app: &mut StudioApp, ui: &mut egui::Ui, body: Rect, narrow: bool, rtl: bool) {
    let ctx = ui.ctx().clone();
    sync_flags(app, &ctx);
    let hide = hidden(narrow);
    let lay = app.editor.dock.layout(body, metrics(), &hide);
    // A maximised panel that has gone (closed, or moved out) is not.
    if app.editor.maximised.is_some_and(|p| lay.tile_of(p).is_none()) {
        app.editor.maximised = None;
    }
    let shown = app.editor.maximised.and_then(|p| maximised_layout(&lay, p));
    let draw = shown.as_ref().unwrap_or(&lay);
    // The tree reads left to right; a right-to-left window shows it mirrored.
    let scr = |r: Rect| if rtl { crate::i18n::mirror(r, body) } else { r };
    let mut actions: Vec<Action> = Vec::new();

    let sound_in_timeline = super::mixer::is_on(&ctx) && !app.editor.dock.contains(Panel::Sound);
    // Where each panel is on screen, for the next click to find its keyboard owner.
    let rects: Vec<(Panel, Rect)> = draw.tiles.iter().map(|t| (t.active, scr(t.rect))).collect();
    ctx.data_mut(|d| d.insert_temp(rects_id(), rects));
    for tile in &draw.tiles {
        let r = scr(tile.rect);
        crate::a11y::landmark(ui, region(tile.active), r);
        super::panel(ui, r);
        let content = if tile.panels.len() > 1 {
            tab_strip(ui, tile, r, rtl, &mut actions);
            Rect::from_min_max(egui::pos2(r.left(), r.top() + TAB_H), r.max)
        } else {
            r
        };
        // The header is the handle; the panel's own buttons sit above it
        // and keep their clicks (a drag that starts on one still moves the
        // panel).
        let head = Rect::from_min_size(content.min, Vec2::new(content.width(), header_h(tile.active).min(content.height())));
        let grip = ui.interact(head, Id::unique(("dock-grip", tile.active.key())), Sense::CLICK | Sense::DRAG);
        if grip.drag_started() {
            actions.push(Action::Drag(tile.active));
        }
        if grip.secondary_clicked() {
            actions.push(Action::Menu(tile.active, head));
        }
        if grip.double_clicked() {
            actions.push(Action::Maximise(tile.active));
        }
        let shown = if tile.active == Panel::Timeline && sound_in_timeline { Panel::Sound } else { tile.active };
        draw_panel(app, ui, shown, content);
        if app.editor.maximised == Some(tile.active) && IconButton::new(Icon::CornersIn, "Restore panel").size(24.0, 14.0).show_at(ui, Rect::from_min_size(egui::pos2(r.right() - 30.0, r.top() + 3.0), Vec2::splat(24.0))).clicked() {
            actions.push(Action::Maximise(tile.active));
        }
    }
    if !lay.tiles.iter().any(|t| t.active == Panel::Viewer) {
        // The Viewer is a tab behind another: toasts and the viewport's keys
        // use where it would be.
        if let Some(t) = lay.tile_of(Panel::Viewer) {
            app.editor.viewer_rect = scr(t.rect);
        }
    }

    // Splitters, over the tiles.
    let mut changed = false;
    for s in &draw.splitters {
        let line = scr(s.rect);
        let vertical = s.axis == Axis::Row;
        let (d, dbl, _) = super::splitter(ui, Id::unique(("dock-splitter", &s.path, s.before, s.after)), line, vertical);
        if dbl {
            changed |= app.editor.dock.toggle(s);
        } else if d != 0.0 {
            let d = if rtl && vertical { -d } else { d };
            changed |= app.editor.dock.drag(s, d, &lay, &hide);
        }
    }
    let _ = changed;

    // A drag in progress: the overlay, and the drop on release.
    if app.editor.dock_drag.is_some() && app.editor.maximised.is_none() {
        drag_overlay(app, ui, &lay, body, rtl, &hide);
    }
    for a in actions {
        match a {
            // Nowhere to drop while one panel fills the dock.
            Action::Drag(_) if app.editor.maximised.is_some() => {}
            Action::Drag(p) => {
                ctx.data_mut(|d| d.remove::<bool>(Id::unique("dock-drag-cancelled")));
                app.editor.dock_drag = Some(DockDrag { panel: p });
            }
            Action::Menu(p, anchor) => open_menu(&ctx, p, anchor, false),
            Action::Maximise(p) => toggle_maximised_panel(app, &ctx, p),
            Action::Activate(p) => {
                app.editor.dock.activate(p);
            }
            Action::Close(p) => close_panel(app, &ctx, p, narrow),
        }
    }
    for p in Panel::ALL {
        if !app.editor.dock.is_popped(p) {
            move_menu(app, &ctx, p, &lay, rtl, narrow);
        }
    }
    // Panels in windows of their own.
    if let Some(p) = app.editor.focus_window.take() {
        ctx.send_viewport_cmd_to(viewport_id(p), egui::ViewportCommand::Focus);
    }
    for w in app.editor.dock.popped.clone() {
        popped_window(app, &ctx, w.panel, w.size, &lay, rtl, narrow);
    }
    app.editor.dock_layout = Some(lay);
}

enum Action {
    Maximise(Panel),
    Drag(Panel),
    Menu(Panel, Rect),
    Activate(Panel),
    Close(Panel),
}

/// Draws one panel's content in `r`.
fn draw_panel(app: &mut StudioApp, ui: &mut egui::Ui, p: Panel, r: Rect) {
    match p {
        Panel::Library => super::library::show(app, ui, r),
        Panel::Viewer => {
            super::viewer::show(app, ui, r);
            super::cutting::viewer(app, ui, r);
        }
        Panel::Inspector => super::inspector::show(app, ui, r),
        Panel::Timeline => super::timeline::show(app, ui, r),
        Panel::Sound => super::mixer::show(app, ui, r),
        Panel::Scopes => super::scopes::docked(app, ui, r),
        Panel::Review => super::review::docked(app, ui, r),
        Panel::Record => super::capture::docked(app, ui, r),
        Panel::Editorial => super::cutting::panel::docked(app, ui, r),
        Panel::Ai => super::agent_panel::show(app, ui, r),
    }
}

/// The tabs over a stack: click to show, drag to move or reorder, the ×
/// (or a middle click) to close, right-click for the Move menu.
fn tab_strip(ui: &mut egui::Ui, tile: &Tile, r: Rect, rtl: bool, actions: &mut Vec<Action>) {
    let t = theme::tokens(ui.ctx());
    let strip = Rect::from_min_size(r.min, Vec2::new(r.width(), TAB_H));
    ui.painter().hline(strip.x_range(), strip.bottom() - 0.5, Stroke::new(1.0, t.line));
    // More tabs than the strip is wide: they keep their size and the strip
    // scrolls sideways (wheel, trackpad or drag on the empty part).
    let key = tile.panels[0].key();
    let area_w = (tabs_width(ui, tile) + 8.0).max(strip.width());
    let mut bar = ui.new_child(egui::UiBuilder::new().max_rect(strip).id_salt(("dock-tabs", key)));
    bar.set_clip_rect(strip.intersect(ui.clip_rect()));
    kit::scroll_strip(&mut bar, ("dock-tabs-scroll", key), |ui| {
        let (area, _) = ui.allocate_exact_size(Vec2::new(area_w, TAB_H), Sense::hover());
        ui.ctx().data_mut(|d| d.insert_temp(scroll_id(tile), strip.left() - area.left()));
        tab_cells(ui, tile, area, rtl, &t, actions);
    });
}

fn scroll_id(tile: &Tile) -> Id {
    Id::unique(("dock-tabs-offset", tile.panels[0].key()))
}

fn tab_cells(ui: &mut egui::Ui, tile: &Tile, strip: Rect, rtl: bool, t: &theme::Tokens, actions: &mut Vec<Action>) {
    for (i, tr) in tab_rects_raw(ui, tile, strip, rtl).into_iter().enumerate() {
        let p = tile.panels[i];
        let active = p == tile.active;
        let resp = ui.interact(tr, Id::unique(("dock-tab", p.key())), Sense::click_and_drag());
        ui.ctx().data_mut(|d| d.insert_temp(tab_rect_id(p), tr));
        let name = crate::i18n::t(p.name()).into_owned();
        resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Tab, true, active, &name));
        let st = kit::State::from_response(ui, &resp, None);
        let pill = tr.shrink2(Vec2::new(1.0, 4.0));
        let fill = if active { t.raised } else { theme::alpha(t.raised, st.hover * 0.6) };
        ui.painter().rect_filled(pill, CornerRadius::same(theme::radius::CONTROL), fill);
        let close_w = if p.closable() && (active || st.hover > 0.0) { 18.0 } else { 0.0 };
        // The name at the start edge, the × at the end.
        let (lead, trail) = if rtl { (8.0 + close_w, 10.0) } else { (10.0, 8.0 + close_w) };
        let text_r = Rect::from_min_max(pill.min + Vec2::new(lead, 0.0), pill.max - Vec2::new(trail, 0.0));
        let colour = if active { t.text } else { theme::mix(t.text_2, t.text, st.hover) };
        let cut = kit::paint_text_clipped(ui.painter(), egui::pos2(crate::i18n::start_x(text_r, 0.0), text_r.center().y), crate::i18n::start_center(), &name, theme::control(), colour, text_r.width());
        // Narrowed tabs: the whole name on hover.
        let resp = kit::tip_if_cut(resp, cut, &name);
        if st.focused {
            kit::focus_ring(ui.painter(), pill, CornerRadius::same(theme::radius::CONTROL), &t);
        }
        if close_w > 0.0 {
            let cx = if rtl { pill.left() + 12.0 } else { pill.right() - 12.0 };
            let cr = Rect::from_center_size(egui::pos2(cx, pill.center().y), Vec2::splat(16.0));
            if kit::IconButton::new(Icon::X, "Close panel").size(16.0, 10.0).show_at(ui, cr).clicked() {
                actions.push(Action::Close(p));
                continue;
            }
        }
        if resp.clicked() {
            actions.push(Action::Activate(p));
        }
        if resp.double_clicked() {
            actions.push(Action::Maximise(p));
        }
        if resp.middle_clicked() && p.closable() {
            actions.push(Action::Close(p));
        }
        if resp.drag_started() {
            actions.push(Action::Drag(p));
        }
        if resp.secondary_clicked() || (resp.has_focus() && ui.input(|i| i.modifiers.shift && i.key_pressed(egui::Key::F10))) {
            actions.push(Action::Menu(p, tr));
        }
    }
}

fn tab_rect_id(p: Panel) -> Id {
    Id::unique(("dock-tab-rect", p.key()))
}

/// Where `p`'s tab was drawn last frame (it has one in a stack of two or more).
pub fn tab_rect(ctx: &egui::Context, p: Panel) -> Option<Rect> {
    ctx.data(|d| d.get_temp(tab_rect_id(p)))
}

/// The tabs' rectangles in a strip: as wide as their names, shrinking
/// together when they do not fit.
fn tab_rects(ui: &egui::Ui, tile: &Tile, strip: Rect, rtl: bool) -> Vec<Rect> {
    // Where the strip is scrolled to (as drawn last frame).
    let off = ui.ctx().data(|d| d.get_temp::<f32>(scroll_id(tile))).unwrap_or(0.0);
    let mut rects = tab_rects_raw(ui, tile, Rect::from_min_size(strip.min, Vec2::new(strip.width().max(tabs_width(ui, tile) + 8.0), strip.height())), rtl);
    for r in &mut rects {
        *r = r.translate(Vec2::new(off, 0.0));
    }
    rects
}

/// The width the tabs take side by side at their own sizes.
fn tabs_width(ui: &egui::Ui, tile: &Tile) -> f32 {
    let want: f32 = tile.panels.iter().map(|p| kit::text_width(ui, p.name(), theme::control()) + 38.0).sum();
    want + 2.0 * (tile.panels.len().saturating_sub(1)) as f32
}

/// The tabs' rectangles in a strip, each as wide as its name.
fn tab_rects_raw(ui: &egui::Ui, tile: &Tile, strip: Rect, rtl: bool) -> Vec<Rect> {
    let inner = strip.shrink2(Vec2::new(4.0, 0.0));
    let mut x = inner.left();
    tile.panels
        .iter()
        .map(|p| {
            let w = kit::text_width(ui, p.name(), theme::control()) + 38.0;
            let r = Rect::from_min_size(egui::pos2(x, inner.top()), Vec2::new(w, inner.height()));
            x += w + 2.0;
            if rtl { crate::i18n::mirror(r, inner) } else { r }
        })
        .collect()
}

/// The drop target under the pointer.
fn target(lay: &Layout, ui: &egui::Ui, dragged: Panel, pos: Pos2, body: Rect, rtl: bool) -> Option<(Drop, Rect)> {
    let scr = |r: Rect| if rtl { crate::i18n::mirror(r, body) } else { r };
    if !body.expand(EDGE).contains(pos) {
        return None;
    }
    let tile = lay.tiles.iter().find(|t| scr(t.rect).contains(pos));
    // Over a tab strip (or a lone panel's header): among the tabs.
    let strip_of = |t: &Tile| {
        let r = scr(t.rect);
        let h = if t.panels.len() > 1 { TAB_H } else { header_h(t.active).min(r.height() * 0.25) };
        Rect::from_min_size(r.min, Vec2::new(r.width(), h))
    };
    let on_strip = tile.is_some_and(|t| strip_of(t).contains(pos) && t.panels != [dragged]);
    // Along the window's edge.
    let edge = [(Zone::Left, pos.x - body.left()), (Zone::Right, body.right() - pos.x), (Zone::Top, pos.y - body.top()), (Zone::Bottom, body.bottom() - pos.y)];
    if !on_strip && let Some((z, _)) = edge.iter().filter(|(_, d)| *d < EDGE).min_by(|a, b| a.1.total_cmp(&b.1)) {
        return Some((Drop::Dock(None, *z), body));
    }
    let tile = tile?;
    let r = scr(tile.rect);
    if tile.panels == [dragged] {
        // Onto itself: nowhere to go.
        return None;
    }
    if on_strip {
        if tile.panels.len() > 1 {
            let rects = tab_rects(ui, tile, Rect::from_min_size(r.min, Vec2::new(r.width(), TAB_H)), rtl);
            let index = rects.iter().filter(|tr| if rtl { tr.center().x > pos.x } else { tr.center().x < pos.x }).count();
            return Some((Drop::Tab(tile.active, index), r));
        }
        return Some((Drop::Dock(Some(tile.active), Zone::Tab), r));
    }
    // The compass in the middle, then the nearest edge (the middle is a tab).
    if let Some(z) = compass(r).into_iter().find(|(_, cr)| cr.expand(3.0).contains(pos)).map(|(z, _)| z) {
        return Some((Drop::Dock(Some(tile.active), z), r));
    }
    let u = (pos.x - r.left()) / r.width().max(1.0);
    let v = (pos.y - r.top()) / r.height().max(1.0);
    if (u - 0.5).abs() < 0.2 && (v - 0.5).abs() < 0.2 {
        return Some((Drop::Dock(Some(tile.active), Zone::Tab), r));
    }
    let z = [(Zone::Left, u), (Zone::Right, 1.0 - u), (Zone::Top, v), (Zone::Bottom, 1.0 - v)].into_iter().min_by(|a, b| a.1.total_cmp(&b.1)).map(|(z, _)| z)?;
    Some((Drop::Dock(Some(tile.active), z), r))
}

/// The five drop buttons in the middle of a tile (none when it is small).
fn compass(r: Rect) -> Vec<(Zone, Rect)> {
    let s = 26.0;
    let step = s + 6.0;
    if r.width() < step * 3.0 + 24.0 || r.height() < step * 3.0 + 24.0 {
        return Vec::new();
    }
    let c = r.center();
    [(Zone::Tab, Vec2::ZERO), (Zone::Left, Vec2::new(-step, 0.0)), (Zone::Right, Vec2::new(step, 0.0)), (Zone::Top, Vec2::new(0.0, -step)), (Zone::Bottom, Vec2::new(0.0, step))]
        .into_iter()
        .map(|(z, o)| (z, Rect::from_center_size(c + o, Vec2::splat(s))))
        .collect()
}

/// A little picture of a drop zone: a frame with the part that would be
/// taken filled in.
fn zone_glyph(p: &egui::Painter, r: Rect, zone: Option<Zone>, line: egui::Color32, fill: egui::Color32) {
    let f = r.shrink(1.0);
    let part = match zone {
        Some(Zone::Left) => Rect::from_min_max(f.min, egui::pos2(f.center().x, f.bottom())),
        Some(Zone::Right) => Rect::from_min_max(egui::pos2(f.center().x, f.top()), f.max),
        Some(Zone::Top) => Rect::from_min_max(f.min, egui::pos2(f.right(), f.center().y)),
        Some(Zone::Bottom) => Rect::from_min_max(egui::pos2(f.left(), f.center().y), f.max),
        Some(Zone::Tab) => Rect::from_min_max(egui::pos2(f.left(), f.top() + f.height() * 0.3), f.max),
        None => Rect::NOTHING,
    };
    p.rect_filled(part, CornerRadius::same(2), fill);
    if zone == Some(Zone::Tab) {
        p.rect_filled(Rect::from_min_size(f.min, Vec2::new(f.width() * 0.45, f.height() * 0.3)), CornerRadius::same(1), fill);
    }
    p.rect_stroke(f, CornerRadius::same(2), Stroke::new(1.0, line), egui::StrokeKind::Inside);
}

fn drag_overlay(app: &mut StudioApp, ui: &mut egui::Ui, lay: &Layout, body: Rect, rtl: bool, hide: &dyn Fn(Panel) -> bool) {
    let ctx = ui.ctx().clone();
    let Some(drag) = app.editor.dock_drag.clone() else { return };
    let t = theme::tokens(&ctx);
    let pos = ctx.input(|i| i.pointer.interact_pos().or(i.pointer.hover_pos()));
    let released = ctx.input(|i| !i.pointer.primary_down());
    let p = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, Id::unique("dock-overlay")));
    let found = pos.and_then(|pos| target(lay, ui, drag.panel, pos, body, rtl));
    ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
    // What the drop would do, as the tree sees it.
    let tree_drop = |d: Drop| match d {
        Drop::Dock(t, z) => Drop::Dock(t, if rtl { z.mirrored() } else { z }),
        tab => tab,
    };
    if let Some((d, tile_r)) = found {
        // The tiles that could take it, outlined; the compass on the one under the pointer.
        if let Drop::Dock(Some(_), _) | Drop::Tab(..) = d {
            for (z, cr) in compass(tile_r) {
                let on = d == Drop::Dock(drop_target(d), z);
                p.rect_filled(cr, CornerRadius::same(4), theme::alpha(t.panel, 0.92));
                zone_glyph(&p, cr.shrink(5.0), Some(z), if on { t.accent } else { t.text_2 }, theme::alpha(if on { t.accent } else { t.text_2 }, if on { 0.8 } else { 0.35 }));
            }
        }
        let preview = match tree_drop(d) {
            Drop::Dock(target, zone) => app.editor.dock.preview(drag.panel, target, zone, lay, hide).map(|r| if rtl { crate::i18n::mirror(r, body) } else { r }),
            Drop::Tab(..) => Some(tile_r),
        };
        if let Some(pr) = preview {
            p.rect_filled(pr.shrink(1.0), CornerRadius::same(theme::radius::PANEL), theme::alpha(t.accent, 0.16));
            p.rect_stroke(pr.shrink(1.0), CornerRadius::same(theme::radius::PANEL), Stroke::new(2.0, t.accent), egui::StrokeKind::Inside);
        }
        if let Drop::Tab(target, index) = d
            && let Some(tile) = lay.tile_of(target)
        {
            let strip = Rect::from_min_size(tile_r.min, Vec2::new(tile_r.width(), TAB_H));
            let rects = tab_rects(ui, tile, strip, rtl);
            let x = match (rects.get(index), rects.last()) {
                (Some(r), _) => if rtl { r.right() } else { r.left() },
                (None, Some(r)) => if rtl { r.left() } else { r.right() },
                _ => strip.left(),
            };
            p.vline(x, strip.y_range().shrink(4.0), Stroke::new(2.0, t.accent));
        }
    }
    // The panel's name follows the pointer.
    if let Some(pos) = pos {
        let name = crate::i18n::shown(drag.panel.name()).into_owned();
        let w = kit::text_width(ui, drag.panel.name(), theme::control()) + 20.0;
        let chip = Rect::from_min_size(pos + Vec2::new(14.0, 10.0), Vec2::new(w, 26.0));
        p.rect_filled(chip, CornerRadius::same(theme::radius::CONTROL), t.raised);
        p.rect_stroke(chip, CornerRadius::same(theme::radius::CONTROL), Stroke::new(1.0, t.line), egui::StrokeKind::Inside);
        p.text(chip.center(), Align2::CENTER_CENTER, &name, theme::control(), t.text);
    }
    if released {
        app.editor.dock_drag = None;
        if let Some((d, _)) = found {
            apply(app, &ctx, drag.panel, tree_drop(d), lay);
        }
    }
    ctx.request_repaint();
}

fn drop_target(d: Drop) -> Option<Panel> {
    match d {
        Drop::Dock(t, _) => t,
        Drop::Tab(t, _) => Some(t),
    }
}

/// Makes a move (already in the tree's terms) and offers to undo it.
fn apply(app: &mut StudioApp, ctx: &egui::Context, p: Panel, d: Drop, lay: &Layout) {
    let before = app.editor.dock.clone();
    let done = match d {
        Drop::Dock(target, zone) => app.editor.dock.dock(p, target, zone, lay),
        Drop::Tab(target, index) => app.editor.dock.move_tab(p, target, index),
    };
    if !done || app.editor.dock == before {
        return;
    }
    if FLAGGED.contains(&p) {
        // Docked now (a floating panel's window gives way to the tile).
        set_flag(app, ctx, p, true);
        app.editor.dock_flags[flag_slot(p)] = Some(true);
    }
    let name = crate::i18n::t(p.name()).into_owned();
    let msg = crate::i18n::tr_args("dock-moved", &[("panel", name.into())]).into_owned();
    let id = kit::push(ctx, msg, Some("Undo"), Some(Icon::SquaresFour));
    app.editor.dock_undo = Some((id, before));
}

/// A drop made without the mouse (the Move menu): the same as dropping
/// there, in screen terms.
fn apply_screen(app: &mut StudioApp, ctx: &egui::Context, p: Panel, d: Drop, lay: &Layout, rtl: bool) {
    let d = match d {
        Drop::Dock(t, z) => Drop::Dock(t, if rtl { z.mirrored() } else { z }),
        tab => tab,
    };
    apply(app, ctx, p, d, lay);
}

/// "Move panel to…": every other panel on screen with its five places,
/// the window's edges, float, close.
fn move_menu(app: &mut StudioApp, ctx: &egui::Context, p: Panel, lay: &Layout, rtl: bool, narrow: bool) {
    let id = menu_id(p);
    if !kit::is_open(ctx, id) {
        return;
    }
    let anchor = ctx.data(|d| d.get_temp::<Rect>(menu_anchor_id(p))).unwrap_or(lay.body);
    let focus_first = ctx.data_mut(|d| d.remove_temp::<bool>(Id::unique("dock-menu-focus"))).unwrap_or(false);
    let others: Vec<Panel> = lay.tiles.iter().flat_map(|t| t.panels.clone()).filter(|q| *q != p).collect();
    let mut pick: Option<Drop> = None;
    let mut float_it = false;
    let mut close_it = false;
    let mut pop = false;
    let mut back = false;
    let mut maximise = false;
    let maximised = app.editor.maximised == Some(p);
    let popped = app.editor.dock.is_popped(p);
    let t = theme::tokens(ctx);
    kit::popover(ctx, id, anchor, kit::Side::Below, 272.0, |ui| {
        kit::menu_heading(ui, &crate::i18n::tr_args("dock-move-heading", &[("panel", crate::i18n::t(p.name()).into_owned().into())]));
        let mut first = focus_first;
        let mut row = |ui: &mut egui::Ui, label: &str, target: Option<Panel>, zones: &[Zone]| {
            ui.horizontal(|ui| {
                ui.set_min_height(26.0);
                let lw = ui.available_width() - zones.len() as f32 * 30.0;
                let (lr, _) = ui.allocate_exact_size(Vec2::new(lw.max(40.0), 24.0), Sense::hover());
                kit::paint_text_clipped(ui.painter(), egui::pos2(crate::i18n::start_x(lr, 4.0), lr.center().y), crate::i18n::start_center(), label, theme::body(), t.text, lr.width() - 8.0);
                for z in zones {
                    let (zr, resp) = ui.allocate_exact_size(Vec2::new(28.0, 24.0), Sense::click());
                    let what = format!("{} · {}", crate::i18n::t(zone_name(*z)), label);
                    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &what));
                    let st = kit::State::from_response(ui, &resp, None);
                    ui.painter().rect_filled(zr, CornerRadius::same(theme::radius::CONTROL), theme::alpha(t.raised, st.hover.max(st.focused as u8 as f32)));
                    let c = theme::mix(t.text_2, t.accent, st.hover);
                    zone_glyph(ui.painter(), Rect::from_center_size(zr.center(), Vec2::new(18.0, 14.0)), Some(*z), c, theme::alpha(c, 0.55));
                    if st.focused {
                        kit::focus_ring(ui.painter(), zr, CornerRadius::same(theme::radius::CONTROL), &t);
                    }
                    if first {
                        resp.request_focus();
                        first = false;
                    }
                    let resp = resp.on_hover_text(crate::i18n::visual(&what).into_owned());
                    if resp.clicked() {
                        pick = Some(Drop::Dock(target, *z));
                    }
                }
            });
        };
        for q in &others {
            row(ui, &crate::i18n::t(q.name()), Some(*q), &Zone::ALL);
        }
        row(ui, &crate::i18n::t("Window edge"), None, &[Zone::Left, Zone::Right, Zone::Top, Zone::Bottom]);
        if p.floats() || p.closable() || p.pops_out() {
            kit::menu_separator(ui);
        }
        if popped {
            if MenuItem::new("Dock back").icon(Icon::SquaresFour).show(ui).clicked() {
                back = true;
            }
        } else if p.pops_out() && MenuItem::new("Pop out").icon(Icon::ArrowSquareOut).show(ui).clicked() {
            pop = true;
        }
        if p.floats() && app.editor.dock.contains(p) && MenuItem::new("Float in a window").icon(Icon::ArrowSquareOut).show(ui).clicked() {
            float_it = true;
        }
        if !popped && MenuItem::new("Maximise panel").checked(maximised).shortcut("⌘\\").icon(Icon::CornersOut).show(ui).clicked() {
            maximise = true;
        }
        if p.closable() && MenuItem::new("Close panel").icon(Icon::X).show(ui).clicked() {
            close_it = true;
        }
    });
    if let Some(d) = pick {
        kit::set_open(ctx, id, false);
        apply_screen(app, ctx, p, d, lay, rtl);
    } else if pop {
        kit::set_open(ctx, id, false);
        app.editor.dock.pop_out(p, lay);
    } else if back {
        kit::set_open(ctx, id, false);
        app.editor.dock.dock_back(p, lay);
    } else if maximise {
        kit::set_open(ctx, id, false);
        toggle_maximised_panel(app, ctx, p);
    } else if float_it {
        kit::set_open(ctx, id, false);
        float(app, ctx, p);
    } else if close_it {
        kit::set_open(ctx, id, false);
        close_panel(app, ctx, p, narrow);
    }
}

/// The system window a popped-out panel lives in.
pub fn viewport_id(p: Panel) -> egui::ViewportId {
    egui::ViewportId::from_hash_of(("studio-panel-window", p.key()))
}

/// A popped-out panel in its own window: a slim bar with its name, the
/// Move menu and Dock back, then the panel. Closing the window docks it
/// back. Where windows of their own are not possible (and in tests and
/// screenshots) it is a window inside the editor instead.
fn popped_window(app: &mut StudioApp, ctx: &egui::Context, p: Panel, size: Option<[f32; 2]>, lay: &Layout, rtl: bool, narrow: bool) {
    let min = p.min_size() + Vec2::new(0.0, BAR_H);
    let size = size.map_or_else(|| p.preferred_size(), |s| Vec2::new(s[0], s[1])).max(min);
    let title = format!("{} · Zornov", crate::i18n::t(p.name()));
    let mut out = (false, None);
    if ctx.embed_viewports() {
        egui::Window::new(crate::i18n::visual(&title).into_owned())
            .id(Id::unique(("dock-panel-window", p.key())))
            .default_size(size)
            .min_size(min)
            .resizable(true)
            .collapsible(false)
            .show(ctx, |ui| out = popped_body(app, ui, p, min, true, lay, rtl, narrow));
    } else {
        let builder = egui::ViewportBuilder::default().with_title(title).with_inner_size(size).with_min_inner_size(min);
        ctx.show_viewport_immediate(viewport_id(p), builder, |ui, _| out = popped_body(app, ui, p, min, false, lay, rtl, narrow));
    }
    let (dock_back, seen_size) = out;
    if !app.editor.dock.is_popped(p) {
        // Its Move menu docked it somewhere this frame.
        return;
    }
    if dock_back {
        app.editor.dock.dock_back(p, lay);
    } else if let Some(s) = seen_size {
        app.editor.dock.set_popped_size(p, s);
    }
}

/// What a popped-out window shows; says whether to dock back and the
/// window's size (a window of its own).
#[allow(clippy::too_many_arguments)]
fn popped_body(app: &mut StudioApp, ui: &mut egui::Ui, p: Panel, min: Vec2, embedded: bool, lay: &Layout, rtl: bool, narrow: bool) -> (bool, Option<Vec2>) {
    let ctx = ui.ctx().clone();
    let t = theme::tokens(&ctx);
    let full = if embedded {
        let r = Rect::from_min_size(ui.cursor().min, ui.available_size().max(min));
        ui.allocate_rect(r, Sense::hover());
        r
    } else {
        let r = ui.max_rect();
        ui.painter().rect_filled(r, 0, t.ground);
        r
    };
    let mut dock_back = false;
    // The bar: name, Move menu, Dock back.
    let bar = Rect::from_min_size(full.min, Vec2::new(full.width(), BAR_H));
    if !embedded {
        // (Inside the editor the window's own title bar names it.)
        kit::paint_text_clipped(ui.painter(), egui::pos2(crate::i18n::start_x(bar, 12.0), bar.center().y), crate::i18n::start_center(), p.name(), theme::control(), t.text, (bar.width() - 150.0).max(20.0));
    }
    let bw = kit::text_width(ui, "Dock back", theme::control()) + 28.0;
    let back_r = Rect::from_min_size(egui::pos2(if rtl { bar.left() + 8.0 } else { bar.right() - 8.0 - bw }, bar.top() + 5.0), Vec2::new(bw, BAR_H - 10.0));
    let mut bui = ui.new_child(egui::UiBuilder::new().max_rect(back_r).layout(egui::Layout::left_to_right(egui::Align::Center)));
    if kit::Button::secondary("Dock back").compact().show(&mut bui).clicked() {
        dock_back = true;
    }
    let menu_r = Rect::from_center_size(egui::pos2(if rtl { back_r.right() + 18.0 } else { back_r.left() - 18.0 }, bar.center().y), Vec2::splat(24.0));
    if kit::IconButton::new(Icon::DotsThree, "Move panel").size(24.0, 14.0).show_at(ui, menu_r).clicked() {
        open_menu(&ctx, p, menu_r, ctx.input(|i| !i.pointer.any_released()));
    }
    let content = Rect::from_min_max(egui::pos2(full.left() + GAP, bar.bottom()), full.max - Vec2::splat(GAP));
    if content.width() > 1.0 && content.height() > 1.0 {
        super::panel(ui, content);
        draw_panel(app, ui, p, content);
    }
    move_menu(app, &ctx, p, lay, rtl, narrow);
    let mut seen = None;
    if !embedded {
        if ui.input(|i| i.viewport().close_requested()) {
            // Closing its window puts the panel back, never loses it.
            dock_back = true;
        }
        seen = ui.input(|i| i.viewport().inner_rect.map(|r| r.size()));
    }
    (dock_back, seen)
}

/// The bar over a popped-out panel.
const BAR_H: f32 = 36.0;

fn zone_name(z: Zone) -> &'static str {
    match z {
        Zone::Left => "Dock left",
        Zone::Right => "Dock right",
        Zone::Top => "Dock above",
        Zone::Bottom => "Dock below",
        Zone::Tab => "Add as a tab",
    }
}

/// The handle on a floating panel's window: drag it onto the editor to
/// dock the panel; click it (or Enter) for the Move menu.
pub fn float_grip(app: &mut StudioApp, ui: &mut egui::Ui, p: Panel) {
    let t = theme::tokens(ui.ctx());
    let (r, resp) = ui.allocate_exact_size(Vec2::splat(22.0), Sense::click_and_drag());
    let st = kit::State::from_response(ui, &resp, None);
    ui.painter().rect_filled(r, CornerRadius::same(theme::radius::CONTROL), theme::alpha(t.raised, st.hover));
    icons::paint(ui.painter(), r, Icon::DotsSixVertical, 14.0, theme::mix(t.text_2, t.text, st.hover));
    if st.focused {
        kit::focus_ring(ui.painter(), r, CornerRadius::same(theme::radius::CONTROL), &t);
    }
    let resp = kit::tip(resp, "Drag onto the editor to dock it", None);
    if resp.drag_started() {
        app.editor.dock_drag = Some(DockDrag { panel: p });
    }
    if resp.clicked() {
        open_menu(ui.ctx(), p, r, ui.ctx().input(|i| !i.pointer.any_released()));
    }
}

/// A rectangle of the tree's layout where it is on screen (mirrored in a
/// right-to-left window).
fn on_screen(lay: &Layout, r: Rect) -> Rect {
    if crate::i18n::is_rtl() { crate::i18n::mirror(r, lay.body) } else { r }
}

/// The AI's `layout` tool, run by the window (see `agent::live`).
pub fn agent_tool(app: &mut StudioApp, ctx: &egui::Context, a: &serde_json::Value) -> Result<serde_json::Value, String> {
    use serde_json::json;
    let narrow = ctx.content_rect().width() < super::NARROW;
    let str_arg = |k: &str| a.get(k).and_then(serde_json::Value::as_str);
    let panel = |k: &str| -> Result<Panel, String> {
        let v = str_arg(k).ok_or_else(|| format!("{k} is needed: one of {}", Panel::ALL.map(Panel::key).join(", ")))?;
        Panel::parse(v).ok_or_else(|| format!("no panel \"{v}\": one of {}", Panel::ALL.map(Panel::key).join(", ")))
    };
    let action = str_arg("action").unwrap_or("get");
    let lay = app.editor.last_layout();
    let done = match action {
        "get" => "",
        "open" => {
            let p = panel("panel")?;
            open_panel(app, ctx, p, narrow);
            "opened"
        }
        "close" => {
            let p = panel("panel")?;
            if !p.closable() {
                return Err("the viewer stays; dock it somewhere else instead".into());
            }
            close_panel(app, ctx, p, narrow);
            "closed"
        }
        "dock" => {
            let p = panel("panel")?;
            let zone = str_arg("zone").and_then(Zone::parse).ok_or("zone is needed: left, right, top, bottom or tab")?;
            let target = match str_arg("target") {
                None | Some("window") => None,
                Some(_) => Some(panel("target")?),
            };
            if target.is_some_and(|t| !app.editor.dock.is_open(t)) {
                return Err(format!("{} is not open; open it first or dock against another panel", str_arg("target").unwrap_or_default()));
            }
            if target.is_none() && zone == Zone::Tab {
                return Err("the window's edge takes left, right, top or bottom".into());
            }
            // The AI speaks in reading order, as the tree does (no mirroring).
            let before = app.editor.dock.clone();
            if !app.editor.dock.dock(p, target, zone, &lay) || app.editor.dock == before {
                return Err("nothing to move: the panel is already there".into());
            }
            if FLAGGED.contains(&p) {
                set_flag(app, ctx, p, true);
                app.editor.dock_flags[flag_slot(p)] = Some(true);
            }
            "docked"
        }
        "float" => {
            let p = panel("panel")?;
            if !p.floats() {
                return Err(format!("{} cannot float; review and record can", p.key()));
            }
            float(app, ctx, p);
            "floating"
        }
        "pop_out" => {
            let p = panel("panel")?;
            if !p.pops_out() {
                return Err("the viewer stays in the editor".into());
            }
            if !app.editor.dock.pop_out(p, &lay) {
                return Err(format!("{} is already in a window of its own", p.key()));
            }
            "popped out"
        }
        "dock_back" => {
            let p = panel("panel")?;
            if !app.editor.dock.dock_back(p, &lay) {
                return Err(format!("{} is not in a window of its own", p.key()));
            }
            "docked back"
        }
        "workspace" => {
            let name = str_arg("name").ok_or("name is needed")?;
            let w = crate::workspaces::find(name).ok_or_else(|| format!("no workspace \"{name}\""))?;
            crate::workspaces::switch(app, ctx, &w);
            "switched"
        }
        "save_workspace" => {
            let name = str_arg("name").ok_or("name is needed")?;
            crate::workspaces::save(crate::workspaces::current(app, name))?;
            "saved"
        }
        "delete_workspace" => {
            crate::workspaces::delete(str_arg("name").ok_or("name is needed")?)?;
            "deleted"
        }
        "reset" => {
            reset(app, ctx);
            "reset"
        }
        "set" => {
            let t: Tree = serde_json::from_value(a.get("tree").cloned().ok_or("tree is needed (as get returns it)")?).map_err(|e| format!("tree: {e}"))?;
            app.editor.set_tree(t);
            "set"
        }
        other => return Err(format!("layout action \"{other}\": get, open, close, dock, float, pop_out, dock_back, workspace, save_workspace, delete_workspace, reset, set")),
    };
    let ed = &app.editor;
    let panels: Vec<serde_json::Value> = Panel::ALL
        .into_iter()
        .map(|p| {
            let docked = ed.dock.contains(p);
            json!({
                "panel": p.key(),
                "open": panel_shown(app, ctx, p, narrow),
                "docked": docked,
                "showing": docked && ed.dock.is_showing(p),
                "floating": p.floats() && !docked && !ed.dock.is_popped(p) && flag(app, ctx, p).unwrap_or(false),
                "popped_out": ed.dock.is_popped(p),
            })
        })
        .collect();
    let mut out = json!({
        "panels": panels,
        "tree": ed.dock,
        "workspace": app.prefs.workspace,
        "workspaces": crate::workspaces::all().into_iter().map(|w| w.name).collect::<Vec<_>>(),
    });
    if !done.is_empty() {
        out["done"] = json!(done);
    }
    if narrow {
        out["note"] = json!("the window is narrow: the inspector shows as a sheet over the right side");
    }
    Ok(out)
}

/// The window's layout as it would be now: for edits made outside a frame
/// (the AI's tools, menus before the first frame).
pub fn fallback_layout(tree: &Tree) -> Layout {
    // A typical window until the editor has drawn once (sizes then come
    // from the real one).
    let full = Rect::from_min_size(Pos2::ZERO, Vec2::new(1440.0, 900.0));
    tree.layout(body(full), metrics(), &|_| false)
}

#[cfg(test)]
mod tests;
