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
    fn double_click(&mut self, at: Pos2) {
        self.step(vec![Event::PointerMoved(at)]);
        for _ in 0..2 {
            self.step(vec![Drive::button(at, true)]);
            self.step(vec![Drive::button(at, false)]);
        }
        self.frames(2);
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
fn a_secondary_panel_opens_docked_and_floats_only_by_choice() {
    let mut d = Drive::new(big());
    let ctx = d.h.ctx.clone();
    crate::editor::review::set_open(true);
    d.frames(2);
    assert!(docked(&ctx, Panel::Review), "turning Review on docks it");
    let tree = &d.h.app.editor.dock;
    assert!(tree.is_showing(Panel::Review) && tree.find(Panel::Review) == tree.find(Panel::Inspector), "a tab in the Inspector's zone");
    // The Panels checkmark and closing it.
    assert!(panel_shown(&d.h.app, &ctx, Panel::Review, false));
    toggle_panel(&mut d.h.app, &ctx, Panel::Review, false);
    d.frames(2);
    assert!(!crate::editor::review::is_open() && !d.h.app.editor.dock.is_open(Panel::Review));
    // Back, then out into a window of its own because the person said so.
    toggle_panel(&mut d.h.app, &ctx, Panel::Review, false);
    d.frames(2);
    assert!(d.h.app.editor.dock.is_open(Panel::Review));
    float(&mut d.h.app, &ctx, Panel::Review);
    d.frames(2);
    assert!(!d.h.app.editor.dock.contains(Panel::Review) && crate::editor::review::is_open() && !docked(&ctx, Panel::Review));
    // Closed, it is not a floating panel any more: the next time it docks.
    crate::editor::review::set_open(false);
    d.frames(2);
    crate::editor::review::set_open(true);
    d.frames(2);
    assert!(docked(&ctx, Panel::Review));
    crate::editor::review::set_open(false);
}

/// The four zones are browser | viewer | inspector | timeline: tools open as
/// tabs or beside their zone, never over the picture or in the timeline's place.
#[test]
fn secondary_panels_never_cover_the_viewer_or_replace_the_timeline() {
    for p in [Panel::Review, Panel::Record, Panel::Editorial, Panel::Scopes, Panel::Sound, Panel::Ai] {
        let mut d = Drive::new(big());
        let ctx = d.h.ctx.clone();
        open_panel(&mut d.h.app, &ctx, p, false);
        d.frames(3);
        assert!(d.h.app.editor.dock.is_showing(p), "{p:?} is docked and showing");
        assert!(!d.h.app.editor.dock.is_popped(p), "{p:?} is not in a window");
        assert!(d.h.app.editor.dock.is_showing(Panel::Timeline), "{p:?}: the timeline is still showing");
        assert!(d.h.app.editor.dock.is_showing(Panel::Viewer), "{p:?}: the viewer is still showing");
        let (panel, viewer, timeline) = (d.rect(p), d.rect(Panel::Viewer), d.rect(Panel::Timeline));
        assert!(!panel.intersects(viewer.shrink(1.0)), "{p:?} {panel:?} is over the viewer {viewer:?}");
        assert!(viewer.width() >= crate::editor::VIEWER_MIN, "{p:?}: the viewer keeps its width ({viewer:?})");
        assert!(timeline.height() >= crate::editor::TIMELINE_MIN, "{p:?}: the timeline keeps its height ({timeline:?})");
    }
}

#[test]
fn the_mixer_sits_beside_the_timeline() {
    let mut d = Drive::new(big());
    let ctx = d.h.ctx.clone();
    open_panel(&mut d.h.app, &ctx, Panel::Sound, false);
    d.frames(3);
    let (t, s) = (d.rect(Panel::Timeline), d.rect(Panel::Sound));
    assert!(t.right() <= s.left() + 1.0 && (t.center().y - s.center().y).abs() < 4.0, "the mixer {s:?} is right of the timeline {t:?}");
    assert!(d.h.app.editor.dock.is_showing(Panel::Timeline));
}

#[test]
fn a_panel_maximises_and_restores() {
    let mut d = Drive::new(big());
    let ctx = d.h.ctx.clone();
    let before = d.rect(Panel::Timeline);
    // Double-click on the timeline's empty header space (the padding above its buttons).
    d.double_click(egui::pos2(before.left() + before.width() * 0.05, before.top() + 5.0));
    assert_eq!(d.h.app.editor.maximised, Some(Panel::Timeline), "double-click on a header maximises");
    let body = d.lay().body;
    assert_eq!(d.screen(d.lay().tile_of(Panel::Timeline).unwrap().rect).height(), before.height(), "the stored layout is the real one");
    let drawn = d.h.app.editor.last_layout().body;
    assert_eq!(drawn, body);
    // The shortcut restores; View ▸ Maximise panel is the same toggle.
    let chord = crate::keymap::parse("⌘\\").unwrap();
    d.step(vec![Event::Key { key: chord.logical_key, physical_key: Some(chord.logical_key), pressed: true, repeat: false, modifiers: chord.modifiers }]);
    d.frames(2);
    assert_eq!(d.h.app.editor.maximised, None, "⌘\\ restores");
    d.step(vec![Event::Key { key: chord.logical_key, physical_key: Some(chord.logical_key), pressed: true, repeat: false, modifiers: chord.modifiers }]);
    d.frames(2);
    assert!(d.h.app.editor.maximised.is_some(), "and maximises the panel last clicked (or the timeline)");
    // Closing the maximised panel leaves no blank dock.
    close_panel(&mut d.h.app, &ctx, Panel::Timeline, false);
    d.frames(3);
    assert_eq!(d.h.app.editor.maximised, None);
}

#[test]
fn a_tab_maximises_its_panel_on_double_click() {
    let mut d = Drive::new(big());
    crate::editor::agent_panel::open(&mut d.h.app);
    d.frames(3);
    let ctx = d.h.ctx.clone();
    let tab = tab_rect(&ctx, Panel::Library).expect("the Library's tab");
    d.double_click(tab.center());
    assert_eq!(d.h.app.editor.maximised, Some(Panel::Library));
    assert!(d.h.app.editor.dock.is_showing(Panel::Library));
}

#[test]
fn the_ai_editor_is_a_panel_the_rail_opens() {
    let mut d = Drive::new(big());
    assert!(!d.h.app.editor.dock.contains(Panel::Ai), "not docked until asked for");
    // ⌘K: the AI editor opens as a tab with the Library, input focused.
    // (The modifiers this platform's keyboard sends for ⌘: Cmd on a Mac, Ctrl elsewhere.)
    let cmd = crate::keymap::parse("⌘K").unwrap().modifiers;
    d.step(vec![Event::Key { key: Key::K, physical_key: Some(Key::K), pressed: true, repeat: false, modifiers: cmd }]);
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
fn editorial_opens_docked_in_the_browser_zone() {
    let mut d = Drive::new(big());
    let ctx = d.h.ctx.clone();
    d.h.app.editor.cutting.panel.open = true;
    d.frames(2);
    assert!(docked(&ctx, Panel::Editorial) && d.h.app.editor.cutting.panel.open);
    let tree = &d.h.app.editor.dock;
    assert!(tree.find(Panel::Editorial) == tree.find(Panel::Library), "a tab with the Library");
    // Its shortcut (the flag) closes the docked panel.
    d.h.app.editor.cutting.panel.open = false;
    d.frames(2);
    assert!(!d.h.app.editor.dock.is_open(Panel::Editorial));
    // Dragged out beside the viewer it is still a docked panel.
    d.h.app.editor.cutting.panel.open = true;
    d.frames(2);
    let lay = d.lay();
    d.h.app.editor.dock.dock(Panel::Editorial, Some(Panel::Viewer), Zone::Right, &lay);
    d.frames(2);
    assert!(d.rect(Panel::Editorial).left() >= d.rect(Panel::Viewer).right());
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

#[test]
fn closing_a_panel_gives_its_keys_back_to_the_editor() {
    let mut d = Drive::new(big());
    let ctx = d.h.ctx.clone();
    ctx.data_mut(|x| x.insert_temp(scope_id(), KeyScope::Panel(Panel::Sound)));
    close_panel(&mut d.h.app, &ctx, Panel::Sound, false);
    assert_eq!(ctx.data(|x| x.get_temp::<KeyScope>(scope_id())), Some(KeyScope::Editor));
    // A panel that is not on screen cannot keep the keys either.
    assert_eq!(live_scope(KeyScope::Panel(Panel::Review), &[Panel::Timeline]), KeyScope::Editor);
    assert_eq!(live_scope(KeyScope::Panel(Panel::Timeline), &[Panel::Timeline]), KeyScope::Panel(Panel::Timeline));
}

#[test]
fn switching_workspace_gives_the_keys_back_to_the_editor() {
    let mut d = Drive::new(big());
    let ctx = d.h.ctx.clone();
    ctx.data_mut(|x| x.insert_temp(scope_id(), KeyScope::Panel(Panel::Review)));
    let w = crate::workspaces::all().remove(0);
    crate::workspaces::switch(&mut d.h.app, &ctx, &w);
    assert_eq!(ctx.data(|x| x.get_temp::<KeyScope>(scope_id())), Some(KeyScope::Editor));
}

#[test]
fn a_cut_that_is_gone_leaves_the_inspector_on_the_project() {
    let mut d = Drive::new(big());
    d.h.app.editor.selection.clear();
    d.h.app.editor.selected_cut = Some(987_654);
    d.frames(3);
    assert_eq!(d.h.app.editor.selected_cut, None);
}

#[test]
fn a_panel_floated_last_run_comes_back_floated() {
    let h = Harness::new(&["--screen", "editor"]);
    let ctx = h.ctx.clone();
    let mut d = Drive { h, frame: eframe::Frame::_new_kittest(), t: 0.0, size: big() };
    // As the memory of the last run: Review was floated, and is off now.
    set_floated(&ctx, Panel::Review, true);
    crate::editor::review::set_open(false);
    d.frames(3);
    assert!(crate::editor::review::is_open(), "Review is open again");
    assert!(!d.h.app.editor.dock.contains(Panel::Review), "in its own window, not docked");
    // Closing it is remembered too.
    close_panel(&mut d.h.app, &ctx, Panel::Review, false);
    d.frames(3);
    assert!(!floated(&ctx, Panel::Review));
}
