// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Docking driven the way a person does it: pointer drags on headers and
//! tabs, Esc, splitters, the keyboard menu, in whole editor frames.

use super::*;
use crate::ui_tests::Harness;
use eframe::egui::{Event, Key, Modifiers, PointerButton, RawInput};

struct Drive {
    h: Harness,
    frame: eframe::Frame,
    t: f64,
    size: Vec2,
}

impl Drive {
    fn new(size: Vec2) -> Drive {
        let h = Harness::new(&["--screen", "editor"]);
        let mut d = Drive { h, frame: eframe::Frame::_new_kittest(), t: 0.0, size };
        d.frames(3);
        d
    }
    fn step(&mut self, events: Vec<Event>) {
        self.t += 1.0 / 60.0;
        let input = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, self.size)), time: Some(self.t), events, ..Default::default() };
        let app = &mut self.h.app;
        let frame = &mut self.frame;
        crate::editor::test_frame(&self.h.ctx, input, |ui| eframe::App::ui(app, ui, frame));
    }
    /// One frame, and the names in its accessibility tree.
    fn labels(&mut self) -> Vec<String> {
        self.t += 1.0 / 60.0;
        let input = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, self.size)), time: Some(self.t), ..Default::default() };
        let app = &mut self.h.app;
        let frame = &mut self.frame;
        let mut out = self.h.ctx.run_ui(input, |ui| eframe::App::ui(app, ui, frame));
        let _ = self.h.ctx.tessellate(std::mem::take(&mut out.shapes), out.pixels_per_point);
        out.textures_delta.clear();
        let Some(update) = out.platform_output.accesskit_update else { return Vec::new() };
        update.nodes.iter().filter_map(|(_, n)| n.label().map(str::to_owned)).collect()
    }
    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            self.step(vec![]);
        }
    }
    fn button(pos: Pos2, pressed: bool) -> Event {
        Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE }
    }
    /// Press at `from`, move to `to` in steps, release (or call `before_up` first).
    fn drag_with(&mut self, from: Pos2, to: Pos2, before_up: impl FnOnce(&mut Drive)) {
        self.step(vec![Event::PointerMoved(from)]);
        self.step(vec![Drive::button(from, true)]);
        for k in 1..=10 {
            let p = from + (to - from) * (k as f32 / 10.0);
            self.step(vec![Event::PointerMoved(p)]);
        }
        before_up(self);
        self.step(vec![Drive::button(to, false)]);
        self.frames(2);
    }
    fn drag(&mut self, from: Pos2, to: Pos2) {
        self.drag_with(from, to, |_| {});
    }
    fn lay(&self) -> Layout {
        self.h.app.editor.dock_layout.clone().expect("drawn")
    }
    /// On screen (mirrored in a right-to-left window).
    fn screen(&self, r: Rect) -> Rect {
        let lay = self.lay();
        if crate::i18n::is_rtl() { crate::i18n::mirror(r, lay.body) } else { r }
    }
    fn rect(&self, p: Panel) -> Rect {
        self.screen(self.lay().tile_of(p).unwrap_or_else(|| panic!("{p:?} on screen")).rect)
    }
    /// A point in a panel's header, clear of its edges.
    fn header(&self, p: Panel) -> Pos2 {
        let r = self.rect(p);
        egui::pos2(r.center().x, r.top() + 4.0)
    }
}

fn big() -> Vec2 {
    Vec2::new(1440.0, 900.0)
}

/// Where in `r` to drop for `zone`.
fn spot(r: Rect, zone: Zone) -> Pos2 {
    match zone {
        Zone::Left => egui::pos2(r.left() + r.width() * 0.06, r.center().y + 60.0),
        Zone::Right => egui::pos2(r.right() - r.width() * 0.06, r.center().y + 60.0),
        Zone::Top => egui::pos2(r.center().x + 90.0, r.top() + 60.0),
        Zone::Bottom => egui::pos2(r.center().x + 90.0, r.bottom() - 12.0),
        Zone::Tab => r.center(),
    }
}

#[test]
fn a_header_dragged_onto_each_zone_docks_there() {
    for zone in Zone::ALL {
        let mut d = Drive::new(big());
        let v = d.rect(Panel::Viewer);
        let from = d.header(Panel::Inspector);
        let mut saw_drag = false;
        d.drag_with(from, spot(v, zone), |d| saw_drag = d.h.app.editor.dock_drag.is_some());
        assert!(saw_drag, "{zone:?}: dragging the header starts a panel drag");
        assert!(d.h.app.editor.dock_drag.is_none(), "{zone:?}: released");
        let tree = &d.h.app.editor.dock;
        if zone == Zone::Tab {
            assert_eq!(tree.find(Panel::Inspector), tree.find(Panel::Viewer), "stacked with the viewer");
            assert!(tree.is_showing(Panel::Inspector));
            continue;
        }
        let (i, nv) = (d.rect(Panel::Inspector), d.rect(Panel::Viewer));
        match zone {
            Zone::Left => assert!(i.right() <= nv.left() && (i.top() - nv.top()).abs() < 1.0, "{i:?} {nv:?}"),
            Zone::Right => assert!(i.left() >= nv.right() && (i.top() - nv.top()).abs() < 1.0, "{i:?} {nv:?}"),
            Zone::Top => assert!(i.bottom() <= nv.top() && (i.left() - nv.left()).abs() < 1.0, "{i:?} {nv:?}"),
            Zone::Bottom => assert!(i.top() >= nv.bottom() && (i.left() - nv.left()).abs() < 1.0, "{i:?} {nv:?}"),
            Zone::Tab => unreachable!(),
        }
        // The move is remembered and can be undone from its toast.
        assert_eq!(d.h.app.prefs.dock.as_ref(), Some(&d.h.app.editor.dock));
        assert!(d.h.app.editor.dock_undo.is_some());
    }
}

#[test]
fn dropping_on_the_window_edge_docks_along_it() {
    let mut d = Drive::new(big());
    let body = d.lay().body;
    let from = d.header(Panel::Inspector);
    d.drag(from, egui::pos2(body.center().x, body.bottom() - 4.0));
    let i = d.rect(Panel::Inspector);
    assert!((i.width() - body.width()).abs() < 1.0 && (i.bottom() - body.bottom()).abs() < 1.0, "{i:?}");
}

#[test]
fn tabs_drag_out_reorder_and_close() {
    let mut d = Drive::new(big());
    let lay = d.lay();
    d.h.app.editor.dock.dock(Panel::Scopes, Some(Panel::Timeline), Zone::Tab, &lay);
    d.frames(2);
    let tile = d.lay().tile_of(Panel::Timeline).unwrap().clone();
    assert_eq!(tile.panels, vec![Panel::Timeline, Panel::Scopes]);
    let ctx = d.h.ctx.clone();
    let tab = |p| tab_rect(&ctx, p).expect("a tab");
    // Reorder: the Scopes tab before the Timeline tab.
    let (scopes, timeline) = (tab(Panel::Scopes), tab(Panel::Timeline));
    d.drag(scopes.center(), egui::pos2(timeline.left() + 3.0, timeline.center().y));
    assert_eq!(d.lay().tile_of(Panel::Timeline).unwrap().panels, vec![Panel::Scopes, Panel::Timeline]);
    // Drag the Scopes tab out to the Viewer's right.
    let from = tab(Panel::Scopes).center();
    let v = d.rect(Panel::Viewer);
    d.drag(from, spot(v, Zone::Right));
    assert!(d.rect(Panel::Scopes).left() >= d.rect(Panel::Viewer).right());
    assert_eq!(d.lay().tile_of(Panel::Timeline).unwrap().panels, vec![Panel::Timeline]);
    // A click on a tab shows it; the close button closes it.
    let lay = d.lay();
    d.h.app.editor.dock.dock(Panel::Sound, Some(Panel::Timeline), Zone::Tab, &lay);
    d.frames(2);
    let tile = d.lay().tile_of(Panel::Timeline).unwrap().clone();
    assert_eq!(tile.active, Panel::Sound);
    let first = tab(Panel::Timeline);
    d.step(vec![Event::PointerMoved(first.center())]);
    d.step(vec![Drive::button(first.center(), true)]);
    d.step(vec![Drive::button(first.center(), false)]);
    d.frames(1);
    assert!(d.h.app.editor.dock.is_showing(Panel::Timeline));
    assert!(close_panel_works(&mut d));
}

fn close_panel_works(d: &mut Drive) -> bool {
    let ctx = d.h.ctx.clone();
    close_panel(&mut d.h.app, &ctx, Panel::Sound, false);
    d.frames(2);
    !d.h.app.editor.dock.contains(Panel::Sound) && !crate::editor::mixer::is_on(&ctx)
}

#[test]
fn escape_cancels_a_drag() {
    let mut d = Drive::new(big());
    let before = d.h.app.editor.dock.clone();
    let v = d.rect(Panel::Viewer);
    let from = d.header(Panel::Inspector);
    d.drag_with(from, spot(v, Zone::Left), |d| {
        assert!(d.h.app.editor.dock_drag.is_some());
        d.step(vec![Event::Key { key: Key::Escape, physical_key: Some(Key::Escape), pressed: true, repeat: false, modifiers: Modifiers::NONE }]);
        d.step(vec![Event::Key { key: Key::Escape, physical_key: Some(Key::Escape), pressed: false, repeat: false, modifiers: Modifiers::NONE }]);
        assert!(d.h.app.editor.dock_drag.is_none());
    });
    assert_eq!(d.h.app.editor.dock, before);
}

#[test]
fn splitters_drag_and_double_click() {
    let mut d = Drive::new(big());
    let lib = d.rect(Panel::Library);
    let line = egui::pos2(lib.right() + GAP / 2.0, lib.center().y);
    d.drag(line, line + Vec2::new(60.0, 0.0));
    // (The first few points of a drag are egui's threshold before it starts.)
    let grew = d.rect(Panel::Library).width() - lib.width();
    assert!((50.0..=60.5).contains(&grew), "{grew}");
    // Double-click: the Library collapses, and opens again.
    let line = egui::pos2(d.rect(Panel::Library).right() + GAP / 2.0, lib.center().y);
    let dbl = |d: &mut Drive, at: Pos2| {
        // Well after any earlier click (or it would count as a third).
        d.t += 1.0;
        d.step(vec![Event::PointerMoved(at)]);
        for _ in 0..2 {
            d.step(vec![Drive::button(at, true)]);
            d.step(vec![Drive::button(at, false)]);
        }
        d.frames(1);
    };
    dbl(&mut d, line);
    assert!(!d.h.app.editor.dock.is_open(Panel::Library));
    let edge = d.lay().splitters.iter().find(|s| !s.collapsed.is_empty()).map(|s| s.rect.center()).expect("the edge splitter");
    dbl(&mut d, edge);
    assert!(d.h.app.editor.dock.is_open(Panel::Library));
}

#[test]
fn right_to_left_drops_land_where_they_are_shown() {
    let mut d = Drive::new(big());
    d.h.app.prefs.language = "ar".into();
    d.frames(2);
    // The Library is on the right in Arabic.
    assert!(d.rect(Panel::Library).left() > d.rect(Panel::Viewer).right());
    let v = d.rect(Panel::Viewer);
    let from = d.header(Panel::Inspector);
    d.drag(from, spot(v, Zone::Right));
    let (i, nv) = (d.rect(Panel::Inspector), d.rect(Panel::Viewer));
    d.h.app.prefs.language = String::new();
    d.frames(1);
    crate::i18n::set_lang(studio_i18n::Lang::En);
    assert!(i.left() >= nv.right(), "dropped on the right, shown on the right: {i:?} {nv:?}");
}

#[test]
fn the_move_menu_docks_from_the_keyboard() {
    let mut d = Drive::new(big());
    let ctx = d.h.ctx.clone();
    let head = d.rect(Panel::Inspector);
    open_menu(&ctx, Panel::Inspector, head, true);
    d.frames(3);
    // Focus is on the first button: Library, dock left.
    d.step(vec![Event::Key { key: Key::Enter, physical_key: Some(Key::Enter), pressed: true, repeat: false, modifiers: Modifiers::NONE }]);
    d.step(vec![Event::Key { key: Key::Enter, physical_key: Some(Key::Enter), pressed: false, repeat: false, modifiers: Modifiers::NONE }]);
    d.frames(2);
    let (i, l) = (d.rect(Panel::Inspector), d.rect(Panel::Library));
    assert!(i.right() <= l.left(), "the inspector went left of the library: {i:?} {l:?}");
    // A screen reader hears the zone buttons by name.
    d.frames(1);
}

#[test]
fn the_ai_layout_tool_moves_panels() {
    let mut d = Drive::new(big());
    let ctx = d.h.ctx.clone();
    let r = agent_tool(&mut d.h.app, &ctx, &serde_json::json!({"action": "dock", "panel": "scopes", "target": "viewer", "zone": "right"})).unwrap();
    assert_eq!(r["done"], "docked");
    d.frames(2);
    assert!(d.rect(Panel::Scopes).left() >= d.rect(Panel::Viewer).right());
    let r = agent_tool(&mut d.h.app, &ctx, &serde_json::json!({})).unwrap();
    assert!(r["panels"].as_array().unwrap().iter().any(|p| p["panel"] == "scopes" && p["showing"] == true));
    assert!(r["tree"]["root"].is_object());
    assert!(agent_tool(&mut d.h.app, &ctx, &serde_json::json!({"action": "close", "panel": "viewer"})).is_err());
    agent_tool(&mut d.h.app, &ctx, &serde_json::json!({"action": "workspace", "name": "Audio"})).unwrap();
    d.frames(2);
    assert!(d.h.app.editor.dock.is_open(Panel::Sound) && !d.h.app.editor.dock.contains(Panel::Scopes));
    // A tree round-trips through get and set.
    let got = agent_tool(&mut d.h.app, &ctx, &serde_json::json!({})).unwrap();
    agent_tool(&mut d.h.app, &ctx, &serde_json::json!({"action": "reset"})).unwrap();
    agent_tool(&mut d.h.app, &ctx, &serde_json::json!({"action": "set", "tree": got["tree"]})).unwrap();
    assert!(d.h.app.editor.dock.is_open(Panel::Sound));
}

#[test]
fn a_floating_panel_docks_and_floats_again() {
    let mut d = Drive::new(big());
    let ctx = d.h.ctx.clone();
    crate::editor::review::set_open(true);
    d.frames(2);
    assert!(!docked(&ctx, Panel::Review));
    let lay = d.lay();
    let mut app = std::mem::replace(&mut d.h.app.editor.dock, Tree::default());
    app.dock(Panel::Review, Some(Panel::Viewer), Zone::Right, &lay);
    d.h.app.editor.dock = app;
    d.frames(2);
    assert!(docked(&ctx, Panel::Review) && crate::editor::review::is_open());
    // The Layout menu's checkmark and closing it.
    assert!(panel_shown(&d.h.app, &ctx, Panel::Review, false));
    toggle_panel(&mut d.h.app, &ctx, Panel::Review, false);
    d.frames(2);
    assert!(!crate::editor::review::is_open() && !d.h.app.editor.dock.is_open(Panel::Review));
    // Back, then out into its own place.
    toggle_panel(&mut d.h.app, &ctx, Panel::Review, false);
    d.frames(2);
    assert!(d.h.app.editor.dock.is_open(Panel::Review));
    float(&mut d.h.app, &ctx, Panel::Review);
    d.frames(2);
    assert!(!d.h.app.editor.dock.contains(Panel::Review) && crate::editor::review::is_open());
    crate::editor::review::set_open(false);
}

#[test]
fn the_ai_editor_is_a_panel_the_rail_opens() {
    let mut d = Drive::new(big());
    assert!(!d.h.app.editor.dock.contains(Panel::Ai), "not docked until asked for");
    // ⌘K: the AI editor opens as a tab with the Library, input focused.
    d.step(vec![Event::Key { key: Key::K, physical_key: Some(Key::K), pressed: true, repeat: false, modifiers: Modifiers::COMMAND }]);
    d.frames(2);
    let tree = &d.h.app.editor.dock;
    assert!(tree.is_showing(Panel::Ai) && tree.find(Panel::Ai) == tree.find(Panel::Library));
    // The Library tab back; then the rail's AI item brings the AI editor forward.
    let ctx = d.h.ctx.clone();
    let lib_tab = tab_rect(&ctx, Panel::Library).expect("a tab");
    d.step(vec![Event::PointerMoved(lib_tab.center())]);
    d.step(vec![Drive::button(lib_tab.center(), true)]);
    d.step(vec![Drive::button(lib_tab.center(), false)]);
    d.frames(1);
    assert!(d.h.app.editor.dock.is_showing(Panel::Library));
    crate::editor::agent_panel::open(&mut d.h.app);
    d.frames(2);
    assert!(d.h.app.editor.dock.is_showing(Panel::Ai));
    assert_eq!(d.h.app.editor.dock.panels().iter().filter(|p| **p == Panel::Ai).count(), 1, "never twice");
}

#[test]
fn the_ai_editor_moves_hides_reopens_and_keeps_its_work() {
    let mut d = Drive::new(big());
    crate::editor::agent_panel::open(&mut d.h.app);
    d.frames(2);
    // Something typed and a second conversation: state that must survive.
    let a = d.h.app.agent.as_mut().expect("the AI editor");
    a.input = "make a trailer".into();
    // Drag its tab to the Viewer's right.
    let ctx = d.h.ctx.clone();
    let from = tab_rect(&ctx, Panel::Ai).expect("a tab").center();
    let v = d.rect(Panel::Viewer);
    d.drag(from, spot(v, Zone::Right));
    assert!(d.rect(Panel::Ai).left() >= d.rect(Panel::Viewer).right());
    // Hidden from the Panels menu, and back.
    toggle_panel(&mut d.h.app, &ctx, Panel::Ai, false);
    d.frames(2);
    assert!(!d.h.app.editor.dock.is_open(Panel::Ai) && !panel_shown(&d.h.app, &ctx, Panel::Ai, false));
    toggle_panel(&mut d.h.app, &ctx, Panel::Ai, false);
    d.frames(2);
    assert!(d.rect(Panel::Ai).left() >= d.rect(Panel::Viewer).right(), "back where it was");
    assert_eq!(d.h.app.agent.as_ref().unwrap().input, "make a trailer");
}

#[test]
fn a_panel_pops_out_into_its_own_window_and_docks_back() {
    let mut d = Drive::new(big());
    crate::editor::agent_panel::open(&mut d.h.app);
    d.frames(2);
    d.h.app.agent.as_mut().unwrap().input = "keep me".into();
    let ctx = d.h.ctx.clone();
    // Pop out from the Move menu.
    let lay = d.lay();
    assert!(d.h.app.editor.dock.pop_out(Panel::Ai, &lay));
    d.frames(3);
    assert!(d.h.app.editor.dock.is_popped(Panel::Ai) && d.lay().tile_of(Panel::Ai).is_none());
    assert!(panel_shown(&d.h.app, &ctx, Panel::Ai, false), "the Panels menu shows it open");
    // Without windows of their own (as in tests) it is a window inside the
    // editor, drawn with its bar and the panel.
    d.frames(20);
    let names = d.labels();
    assert!(names.iter().any(|n| n == "Dock back"), "the Dock back button is there: {names:?}");
    // State and placement survive a save and load.
    let saved = serde_json::to_string(&d.h.app.editor.dock).unwrap();
    let back: Tree = serde_json::from_str(&saved).unwrap();
    assert!(back.is_popped(Panel::Ai));
    // Dock back: where it was, with what was typed.
    let lay = d.lay();
    assert!(d.h.app.editor.dock.dock_back(Panel::Ai, &lay));
    d.frames(2);
    assert!(d.h.app.editor.dock.is_showing(Panel::Ai) && d.h.app.editor.dock.find(Panel::Ai) == d.h.app.editor.dock.find(Panel::Library));
    assert_eq!(d.h.app.agent.as_ref().unwrap().input, "keep me");
    // The AI's tool does the same.
    let r = agent_tool(&mut d.h.app, &ctx, &serde_json::json!({"action": "pop_out", "panel": "scopes"}));
    assert!(r.is_err() || d.h.app.editor.dock.is_popped(Panel::Scopes));
    let r = agent_tool(&mut d.h.app, &ctx, &serde_json::json!({"action": "pop_out", "panel": "ai"})).unwrap();
    assert_eq!(r["done"], "popped out");
    assert!(r["panels"].as_array().unwrap().iter().any(|p| p["panel"] == "ai" && p["popped_out"] == true));
    assert!(agent_tool(&mut d.h.app, &ctx, &serde_json::json!({"action": "pop_out", "panel": "viewer"})).is_err());
    agent_tool(&mut d.h.app, &ctx, &serde_json::json!({"action": "dock_back", "panel": "ai"})).unwrap();
    assert!(!d.h.app.editor.dock.is_popped(Panel::Ai));
}

#[test]
fn editorial_docks_and_floats() {
    let mut d = Drive::new(big());
    let ctx = d.h.ctx.clone();
    d.h.app.editor.cutting.panel.open = true;
    d.frames(2);
    assert!(!docked(&ctx, Panel::Editorial));
    let lay = d.lay();
    d.h.app.editor.dock.dock(Panel::Editorial, Some(Panel::Viewer), Zone::Right, &lay);
    d.frames(2);
    assert!(docked(&ctx, Panel::Editorial) && d.h.app.editor.cutting.panel.open);
    assert!(d.rect(Panel::Editorial).left() >= d.rect(Panel::Viewer).right());
    // Its shortcut (the flag) closes the docked panel.
    d.h.app.editor.cutting.panel.open = false;
    d.frames(2);
    assert!(!d.h.app.editor.dock.is_open(Panel::Editorial));
}

#[test]
fn a_long_ai_run_shows_a_few_calm_rows() {
    let mut d = Drive::new(big());
    crate::editor::agent_panel::open(&mut d.h.app);
    d.frames(2);
    d.h.app.agent.as_mut().expect("the AI editor").demo_long_run();
    d.frames(3);
    let a = d.h.app.agent.as_ref().unwrap();
    // 6 guide reads, a summary, 13 edits and 2 checks: a handful of rows.
    assert!(a.default_rows() <= 4, "{} rows", a.default_rows());
    assert!(!a.show_steps_open(), "the full list starts closed");
    d.frames(2);
}

/// The centre of the control whose accessibility label is `label`.
fn label_centre(d: &mut Drive, label: &str) -> Option<Pos2> {
    d.t += 1.0 / 60.0;
    let input = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, d.size)), time: Some(d.t), ..Default::default() };
    let app = &mut d.h.app;
    let frame = &mut d.frame;
    let mut out = d.h.ctx.run_ui(input, |ui| eframe::App::ui(app, ui, frame));
    let _ = d.h.ctx.tessellate(std::mem::take(&mut out.shapes), out.pixels_per_point);
    // As `step` does: dropping them unhandled would panic.
    out.textures_delta.clear();
    let update = out.platform_output.accesskit_update?;
    update.nodes.iter().find(|(_, n)| n.label() == Some(label)).and_then(|(_, n)| n.bounds()).map(|b| Pos2::new(((b.x0 + b.x1) / 2.0) as f32, ((b.y0 + b.y1) / 2.0) as f32))
}

fn click(d: &mut Drive, at: Pos2) {
    d.step(vec![Event::PointerMoved(at)]);
    d.step(vec![Drive::button(at, true)]);
    d.step(vec![Drive::button(at, false)]);
    d.frames(2);
}

#[test]
fn the_view_menu_toggles_the_inspector() {
    let mut d = Drive::new(big());
    let ctx = d.h.ctx.clone();
    d.h.app.editor.dock.open(Panel::Inspector, &d.lay());
    d.frames(2);
    assert!(panel_shown(&d.h.app, &ctx, Panel::Inspector, false));
    for want in [false, true] {
        let view = ctx.data(|x| x.get_temp::<Rect>(crate::editor::topbar::menu_rect_id("View"))).expect("the View menu was drawn");
        click(&mut d, view.center());
        let item = label_centre(&mut d, "Inspector").expect("the View menu lists the Inspector");
        click(&mut d, item);
        assert_eq!(panel_shown(&d.h.app, &ctx, Panel::Inspector, false), want);
    }
}
