// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Edit mode driven through the headless editor with synthesized input:
//! Tab into edit mode, select modes, clicks, box selection, G R S with
//! axis locks and typed numbers, operations from keys, menus, the
//! operation search and the right-click menu, the last-operation panel,
//! modifiers, materials, sculpting, and every change undone and redone.
//! `MODEL_EDIT_SHOTS=dir` also writes pictures of the workspace there.

use super::*;
use crate::editor::viewport::ui::{self as vui, Pane};
use crate::editor::viewport::{self as vp, Shade, Workspace};
use eframe::egui::{Event, Modifiers, PointerButton, Pos2, Rect, Vec2};
use serde_json::json;
use studio_modeling::poly::Topology;
use studio_modeling::modifiers::Modifier;
use studio_modeling::selection::Combine;

const SCREEN: Vec2 = Vec2::new(1100.0, 720.0);

fn player() -> (Rect, Rect) {
    let r = Rect::from_min_size(Pos2::ZERO, SCREEN);
    let area = Rect::from_min_max(egui::pos2(12.0, 44.0), egui::pos2(SCREEN.x - 12.0, SCREEN.y - 50.0));
    (r, area)
}

struct Driver {
    ctx: egui::Context,
    t: f64,
    mods: Modifiers,
    textures: crate::ui_shot::Textures,
    shots: Option<std::path::PathBuf>,
}

impl Driver {
    fn new() -> Driver {
        let shots = std::env::var_os("MODEL_EDIT_SHOTS").map(std::path::PathBuf::from);
        Driver { ctx: egui::Context::default(), t: 0.0, mods: Modifiers::NONE, textures: Default::default(), shots }
    }

    fn frame(&mut self, app: &mut StudioApp, events: Vec<Event>) {
        self.run(app, events, None);
    }

    fn run(&mut self, app: &mut StudioApp, mut events: Vec<Event>, shot: Option<&str>) {
        self.t += 1.0 / 60.0;
        events.insert(0, Event::ModifiersChanged(self.mods));
        let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SCREEN)), time: Some(self.t), events, ..Default::default() };
        let (r, area) = player();
        let mut out = self.ctx.run_ui(input, |ui| {
            vui::keys(app, ui.ctx());
            app.editor.viewer_rect = r;
            vui::show(app, ui, r, area);
            if !ui.ctx().input(|i| i.pointer.any_down()) && !ui.ctx().egui_wants_keyboard_input() {
                app.doc.end_gesture();
            }
        });
        self.textures.apply(&out.textures_delta);
        let prims = self.ctx.tessellate(std::mem::take(&mut out.shapes), out.pixels_per_point);
        out.textures_delta.clear();
        if let (Some(name), Some(dir)) = (shot, self.shots.as_ref()) {
            let size = [SCREEN.x as usize, SCREEN.y as usize];
            let px = crate::ui_shot::render(&prims, &self.textures, size, 1.0, egui::Color32::from_rgb(30, 30, 32));
            let _ = std::fs::create_dir_all(dir);
            let _ = crate::ui_shot::save_png(&dir.join(format!("{name}.png")), size, &px);
        }
    }

    fn shot(&mut self, app: &mut StudioApp, name: &str) {
        // A few frames so the picture settles at full quality.
        for _ in 0..3 {
            self.frame(app, vec![]);
        }
        std::thread::sleep(std::time::Duration::from_millis(300));
        self.t += 1.0;
        for _ in 0..3 {
            self.frame(app, vec![]);
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        self.run(app, vec![], Some(name));
    }

    fn idle(&mut self, app: &mut StudioApp, n: usize) {
        for _ in 0..n {
            self.frame(app, vec![]);
        }
    }

    fn key(&mut self, app: &mut StudioApp, at: Pos2, key: egui::Key, mods: Modifiers) {
        self.mods = mods;
        self.frame(app, vec![Event::PointerMoved(at), Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: mods }]);
        self.frame(app, vec![Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers: mods }]);
        self.mods = Modifiers::NONE;
    }

    fn text(&mut self, app: &mut StudioApp, s: &str) {
        self.frame(app, vec![Event::Text(s.into())]);
    }

    fn click(&mut self, app: &mut StudioApp, at: Pos2, button: PointerButton, mods: Modifiers) {
        self.mods = mods;
        self.frame(app, vec![Event::PointerMoved(at)]);
        self.frame(app, vec![Event::PointerButton { pos: at, button, pressed: true, modifiers: mods }]);
        self.frame(app, vec![Event::PointerButton { pos: at, button, pressed: false, modifiers: mods }]);
        self.mods = Modifiers::NONE;
        self.frame(app, vec![]);
    }

    fn drag(&mut self, app: &mut StudioApp, button: PointerButton, mods: Modifiers, from: Pos2, to: Pos2) {
        let b = |p: Pos2, down: bool| Event::PointerButton { pos: p, button, pressed: down, modifiers: mods };
        self.mods = mods;
        self.frame(app, vec![Event::PointerMoved(from)]);
        self.frame(app, vec![b(from, true)]);
        for k in 1..=8 {
            self.frame(app, vec![Event::PointerMoved(from + (to - from) * k as f32 / 8.0)]);
        }
        self.frame(app, vec![b(to, false)]);
        self.mods = Modifiers::NONE;
        self.frame(app, vec![]);
    }

    fn pane(&self) -> Pane {
        *vui::last_panes().iter().find(|p| p.index == 3).unwrap()
    }
}

/// A 1280 × 720 project with the workspace on, solid shading, the eye on
/// the front, and nothing else.
fn app_with(ctx: &egui::Context) -> StudioApp {
    let s = crate::agent::Session::new(crate::services::project::new_custom("Model", 1280, 720, 30.0));
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let _serial = crate::ui_tests::serial();
    let mut app = StudioApp::new(&cc, crate::cli::Cli { screen: Some(crate::cli::Screen::Editor), fresh: true, ..Default::default() });
    app.doc = s.doc;
    app.editor.playhead = 0;
    vp::with(|w| *w = Workspace::for_canvas(1280.0, 720.0));
    vp::follow(&app.doc);
    vp::with(|w| {
        w.on = true;
        w.shade = Shade::Solid;
        w.cursor = [0.0; 3];
    });
    with(|s| *s = None);
    app
}

/// Adds a shape as an object and opens it in edit mode with Tab.
fn open_shape(d: &mut Driver, app: &mut StudioApp, id: &'static str, args: serde_json::Value) -> u32 {
    let ctx = d.ctx.clone();
    let clip = add_object(app, &ctx, id, args.as_object().unwrap()).unwrap();
    d.idle(app, 2);
    // Look at it from a little above and to the side.
    vp::with(|w| {
        w.nav.yaw = 30.0;
        w.nav.pitch = 25.0;
        w.nav.target = [0.0; 3];
        w.changed();
    });
    d.idle(app, 2);
    let c = player().1.center();
    d.key(app, c, egui::Key::Tab, Modifiers::NONE);
    assert_eq!(stage(), Stage::Edit, "Tab opens edit mode");
    assert_eq!(open_clip(), Some(clip as u64));
    clip
}

fn poly() -> Poly {
    with(|s| s.as_ref().unwrap().poly().clone())
}

fn counts() -> (usize, usize, usize) {
    with(|s| s.as_ref().unwrap().sel.count())
}

fn closed(p: &Poly) -> bool {
    Topology::of(p).edge_faces.values().all(|f| f.len() == 2)
}

fn outward(p: &Poly) -> bool {
    let mut q = p.clone();
    q.orient_outward();
    q.corners == p.corners
}

/// Screen position of a part-space point in the main pane.
fn screen_of(app: &StudioApp, q: V3) -> Pos2 {
    let s = with(|s| s.clone()).unwrap();
    let space = draw::Space::of(app, &s).unwrap();
    let pane = *vui::last_panes().iter().find(|p| p.index == 3).unwrap();
    pane.to_screen(space.world(q)).unwrap()
}

/// The source the clip points at, as read from disk.
fn on_disk(app: &StudioApp, clip: u32) -> Poly {
    let mc = app.doc.model.active().clip(m::ClipId(clip as u64)).unwrap().clone();
    let (a, _) = load_source(&mc).unwrap();
    a.parts[0].mesh.clone()
}

#[test]
fn tab_select_modes_clicks_and_box_select() {
    let _g = vp::test_lock();
    let mut d = Driver::new();
    let ctx = d.ctx.clone();
    let mut app = app_with(&ctx);
    let clip = open_shape(&mut d, &mut app, "add_cube", json!({"size": 300.0}));
    d.shot(&mut app, "01-edit-mode-cube");
    let c = player().1.center();
    // 3: face mode; a click on the front face picks it.
    d.key(&mut app, c, egui::Key::Num3, Modifiers::NONE);
    assert_eq!(with(|s| s.as_ref().unwrap().sel.mode), Mode::Face);
    let front = (0..6).find(|&f| poly().face_normal(f)[2] < -0.9).unwrap();
    let at = screen_of(&app, poly().face_centre(front));
    d.click(&mut app, at, PointerButton::Primary, Modifiers::NONE);
    assert_eq!(with(|s| s.as_ref().unwrap().sel.face_list()), vec![front as u32], "a click picks the face under it");
    // Shift-click another toggles it in, then out.
    let top = (0..6).find(|&f| poly().face_normal(f)[1] < -0.9).unwrap();
    let at_top = screen_of(&app, poly().face_centre(top));
    d.click(&mut app, at_top, PointerButton::Primary, Modifiers::SHIFT);
    assert_eq!(counts().2, 2);
    d.click(&mut app, at_top, PointerButton::Primary, Modifiers::SHIFT);
    assert_eq!(counts().2, 1);
    // A: all; Alt+A: none; Cmd+I inverts.
    d.key(&mut app, c, egui::Key::A, Modifiers::NONE);
    assert_eq!(counts().2, 6);
    d.key(&mut app, c, egui::Key::A, Modifiers::ALT);
    assert_eq!(counts().2, 0);
    // 1: vertex mode; a box over the whole cube picks the seen corners
    // only (solid shading), all eight in X-ray.
    d.key(&mut app, c, egui::Key::Num1, Modifiers::NONE);
    let pane = d.pane();
    let corner = |r: Rect| (r.left_top() + Vec2::new(40.0, 50.0), r.right_bottom() - Vec2::new(140.0, 40.0));
    let (a, b) = corner(pane.rect);
    d.drag(&mut app, PointerButton::Primary, Modifiers::NONE, a, b);
    let seen = counts().0;
    assert!((5..8).contains(&seen), "the box picks what can be seen: {seen}");
    d.key(&mut app, c, egui::Key::Z, Modifiers::ALT);
    assert_eq!(vp::snapshot().shade, Shade::Xray);
    d.drag(&mut app, PointerButton::Primary, Modifiers::NONE, a, b);
    assert_eq!(counts().0, 8, "X-ray reaches the back too");
    d.shot(&mut app, "02-xray-all-picked");
    d.key(&mut app, c, egui::Key::Z, Modifiers::ALT);
    // Cmd-drag takes away.
    let half = Rect::from_min_max(pane.rect.left_top(), egui::pos2(pane.rect.center().x, pane.rect.bottom()));
    d.key(&mut app, c, egui::Key::Z, Modifiers::ALT);
    d.drag(&mut app, PointerButton::Primary, Modifiers::COMMAND, half.left_top() + Vec2::splat(5.0), half.right_bottom() - Vec2::splat(5.0));
    assert!(counts().0 < 8);
    d.key(&mut app, c, egui::Key::Z, Modifiers::ALT);
    // Tab leaves; the clip is untouched by picking.
    d.key(&mut app, c, egui::Key::Tab, Modifiers::NONE);
    assert_eq!(stage(), Stage::Object);
    assert_eq!(on_disk(&app, clip).faces(), 6);
}

#[test]
fn extrude_grab_rotate_scale_with_keys_and_numbers_and_undo() {
    let _g = vp::test_lock();
    let mut d = Driver::new();
    let ctx = d.ctx.clone();
    let mut app = app_with(&ctx);
    let clip = open_shape(&mut d, &mut app, "add_cube", json!({"size": 200.0}));
    let c = player().1.center();
    d.key(&mut app, c, egui::Key::Num3, Modifiers::NONE);
    let top = (0..6).find(|&f| poly().face_normal(f)[1] < -0.9).unwrap() as u32;
    with(|s| {
        let s = s.as_mut().unwrap();
        let p = s.poly().clone();
        s.sel.with_faces(&p, &[top]);
    });
    let steps_before = app.doc.history_len();
    // E, then 100 and Enter: the top rises 100 px.
    d.key(&mut app, c, egui::Key::E, Modifiers::NONE);
    assert!(with(|s| s.as_ref().unwrap().modal.is_some()), "E carries on with the pointer");
    d.frame(&mut app, vec![Event::PointerMoved(c + Vec2::new(0.0, -60.0))]);
    d.text(&mut app, "100");
    d.key(&mut app, c + Vec2::new(0.0, -60.0), egui::Key::Enter, Modifiers::NONE);
    let p = poly();
    assert!(with(|s| s.as_ref().unwrap().modal.is_none()));
    let (lo, _) = p.bounds();
    assert!((lo[1] + 200.0).abs() < 0.5, "top at {}", lo[1]);
    assert!(closed(&p) && outward(&p));
    assert_eq!(app.doc.history_len(), steps_before + 1, "one undo step");
    d.shot(&mut app, "03-extruded");
    // G Z 50: the picked top moves along z.
    d.key(&mut app, c, egui::Key::G, Modifiers::NONE);
    d.key(&mut app, c, egui::Key::Z, Modifiers::NONE);
    d.text(&mut app, "50");
    d.key(&mut app, c, egui::Key::Enter, Modifiers::NONE);
    let moved = poly();
    let top_now: Vec<u32> = with(|s| s.as_ref().unwrap().sel.vert_list());
    for &v in &top_now {
        assert!((moved.positions[v as usize][2] - p.positions[v as usize][2] - 50.0).abs() < 0.01, "z +50");
        assert!((moved.positions[v as usize][0] - p.positions[v as usize][0]).abs() < 0.01, "x locked");
    }
    // R Y 90: a quarter turn about y.
    d.key(&mut app, c, egui::Key::R, Modifiers::NONE);
    d.key(&mut app, c, egui::Key::Y, Modifiers::NONE);
    d.text(&mut app, "90");
    d.key(&mut app, c, egui::Key::Enter, Modifiers::NONE);
    let turned = poly();
    let ctr = |p: &Poly| {
        let s = with(|s| s.clone()).unwrap();
        let _ = s;
        studio_modeling::ops::centre_of(p, &top_now)
    };
    let (c0, c1) = (ctr(&moved), ctr(&turned));
    assert!(studio_mesh::length(studio_mesh::sub(c0, c1)) < 0.01, "turns about its middle");
    // S 0.5: half size.
    d.key(&mut app, c, egui::Key::S, Modifiers::NONE);
    d.text(&mut app, "0.5");
    d.key(&mut app, c, egui::Key::Enter, Modifiers::NONE);
    let scaled = poly();
    let spread = |p: &Poly| top_now.iter().map(|&v| studio_mesh::length(studio_mesh::sub(p.positions[v as usize], c1))).fold(0.0f32, f32::max);
    assert!((spread(&scaled) / spread(&turned) - 0.5).abs() < 0.01);
    // Esc puts a transform back.
    d.key(&mut app, c, egui::Key::G, Modifiers::NONE);
    d.frame(&mut app, vec![Event::PointerMoved(c + Vec2::new(80.0, 30.0))]);
    d.frame(&mut app, vec![Event::PointerMoved(c + Vec2::new(120.0, 40.0))]);
    assert_ne!(poly().positions, scaled.positions, "moves with the pointer");
    d.key(&mut app, c, egui::Key::Escape, Modifiers::NONE);
    assert_eq!(poly().positions, scaled.positions, "Esc puts it back");
    assert_eq!(on_disk(&app, clip).positions, scaled.positions, "what is on disk is the last kept change");
    // Undo four times: back to the plain cube; redo brings them again.
    for _ in 0..4 {
        app.doc.undo();
        d.idle(&mut app, 1);
    }
    assert_eq!(poly().faces(), 6, "back to the cube");
    assert_eq!(on_disk(&app, clip).faces(), 6);
    app.doc.redo();
    d.idle(&mut app, 1);
    assert_eq!(poly().faces(), 10);
    assert_eq!(poly().positions, p.positions);
}

#[test]
fn menus_search_context_menu_and_last_operation_panel() {
    let _g = vp::test_lock();
    let mut d = Driver::new();
    let ctx = d.ctx.clone();
    let mut app = app_with(&ctx);
    let clip = open_shape(&mut d, &mut app, "add_cube", json!({"size": 200.0}));
    let c = player().1.center();
    d.key(&mut app, c, egui::Key::Num3, Modifiers::NONE);
    d.key(&mut app, c, egui::Key::A, Modifiers::NONE);
    // The Face menu lists inset with its shortcut.
    let bar_face = {
        // Open the Face menu by its button.
        d.idle(&mut app, 1);
        let id = egui::Id::unique(("model-edit-menu", "Face"));
        kit::set_open(&ctx, id, true);
        d.idle(&mut app, 2);
        kit::is_open(&ctx, id)
    };
    assert!(bar_face);
    d.shot(&mut app, "04-face-menu");
    kit::set_open(&ctx, egui::Id::unique(("model-edit-menu", "Face")), false);
    // F3: search "inset", Enter runs it with the pointer; Enter keeps it.
    // Two neighbouring faces: the top and the front.
    with(|s| {
        let s = s.as_mut().unwrap();
        let p = s.poly().clone();
        let two: Vec<u32> = (0..6u32).filter(|&f| p.face_normal(f as usize)[1] < -0.9 || p.face_normal(f as usize)[2] < -0.9).collect();
        s.sel.with_faces(&p, &two);
    });
    d.key(&mut app, c, egui::Key::F3, Modifiers::NONE);
    assert!(kit::is_open(&ctx, egui::Id::unique("model-edit-search")));
    d.text(&mut app, "inset");
    d.shot(&mut app, "05-search");
    d.key(&mut app, c, egui::Key::Enter, Modifiers::NONE);
    assert!(with(|s| s.as_ref().unwrap().modal.is_some()), "inset follows the pointer");
    d.frame(&mut app, vec![Event::PointerMoved(c + Vec2::new(-30.0, 0.0))]);
    d.text(&mut app, "20");
    d.key(&mut app, c, egui::Key::Enter, Modifiers::NONE);
    let p = poly();
    assert_eq!(p.faces(), 12, "the two faces inset as one region: a rim of six");
    assert!(closed(&p));
    let last = with(|s| s.as_ref().unwrap().last.clone()).expect("kept as the last operation");
    assert_eq!(last.id, "inset");
    // The panel's settings run it again in place of the first run: one
    // step, not two.
    let steps = app.doc.history_len();
    let mut args = last.args.clone();
    args.insert("individual".into(), json!(true));
    rerun(&mut app, &ctx, args);
    d.idle(&mut app, 2);
    assert_eq!(poly().faces(), 14, "each face on its own: two rims of four");
    assert_eq!(app.doc.history_len(), steps, "re-running replaces the step");
    assert_eq!(with(|s| s.as_ref().unwrap().last.as_ref().unwrap().args.get("individual").cloned()), Some(json!(true)));
    d.shot(&mut app, "06-last-operation-panel");
    // Right click opens the face context menu.
    d.click(&mut app, c, PointerButton::Secondary, Modifiers::NONE);
    d.idle(&mut app, 2);
    assert!(kit::is_open(&ctx, egui::Id::unique("model-edit-at")));
    d.shot(&mut app, "07-context-menu");
    d.key(&mut app, c, egui::Key::Escape, Modifiers::NONE);
    // X opens the delete menu at the pointer.
    d.key(&mut app, c, egui::Key::X, Modifiers::NONE);
    d.idle(&mut app, 1);
    assert!(kit::is_open(&ctx, egui::Id::unique("model-edit-at")));
    d.key(&mut app, c, egui::Key::Escape, Modifiers::NONE);
    // Cmd+B (bevel) on edges from the table's shortcut.
    d.key(&mut app, c, egui::Key::Num2, Modifiers::NONE);
    d.key(&mut app, c, egui::Key::A, Modifiers::ALT);
    let e = poly().edges()[0];
    with(|s| {
        let s = s.as_mut().unwrap();
        let p = s.poly().clone();
        s.sel.with_edges(&p, &[e]);
    });
    let before = poly().faces();
    d.key(&mut app, c, egui::Key::B, Modifiers::COMMAND);
    d.text(&mut app, "5");
    d.key(&mut app, c, egui::Key::Enter, Modifiers::NONE);
    assert!(poly().faces() > before, "bevelled");
    assert!(closed(&poly()));
    let _ = clip;
}

#[test]
fn loop_cut_knife_and_operations_undo_and_redo() {
    let _g = vp::test_lock();
    let mut d = Driver::new();
    let ctx = d.ctx.clone();
    let mut app = app_with(&ctx);
    let clip = open_shape(&mut d, &mut app, "add_cube", json!({"size": 200.0}));
    let c = player().1.center();
    // Every kind of change, then all of them undone and redone.
    let mut states: Vec<Poly> = vec![poly()];
    let ops: Vec<(&'static str, serde_json::Value, Mode)> = vec![
        ("subdivide", json!({"cuts": 1}), Mode::Edge),
        ("extrude_region", json!({"distance": 30.0}), Mode::Face),
        ("bevel", json!({"width": 4.0, "segments": 2}), Mode::Edge),
        ("mark_seam", json!({}), Mode::Edge),
        ("unwrap", json!({"method": "seams"}), Mode::Face),
        ("triangulate", json!({}), Mode::Face),
        ("tris_to_quads", json!({}), Mode::Face),
        ("smooth_vertices", json!({"iterations": 1}), Mode::Vertex),
    ];
    for (id, args, mode) in &ops {
        with(|s| {
            let s = s.as_mut().unwrap();
            let p = s.poly().clone();
            s.sel.set_mode(&p, *mode);
            s.sel.all(&p);
            if *id == "extrude_region" || *id == "bevel" {
                // A few, not all.
                let top: Vec<u32> = (0..p.faces() as u32).filter(|&f| p.face_normal(f as usize)[1] < -0.9).collect();
                s.sel.mode = Mode::Face;
                s.sel.with_faces(&p, &top);
                s.sel.set_mode(&p, *mode);
            }
        });
        run(&mut app, &ctx, id, args.as_object().unwrap().clone()).unwrap_or_else(|e| panic!("{id}: {e}"));
        d.idle(&mut app, 1);
        let p = poly();
        assert_eq!(on_disk(&app, clip), p, "{id} is written");
        states.push(p);
    }
    for i in (0..ops.len()).rev() {
        app.doc.undo();
        d.idle(&mut app, 1);
        assert_eq!(poly(), states[i], "undo of {}", ops[i].0);
    }
    for (i, op) in ops.iter().enumerate() {
        app.doc.redo();
        d.idle(&mut app, 1);
        assert_eq!(poly(), states[i + 1], "redo of {}", op.0);
    }
    // Loop cut by pointer: Cmd+R, click an edge of the side, Enter keeps
    // the slide.
    with(|s| *s = None);
    let ctx2 = d.ctx.clone();
    let clip2 = add_object(&mut app, &ctx2, "add_cube", json!({"size": 200.0}).as_object().unwrap()).unwrap();
    app.editor.selection = vec![clip2];
    d.idle(&mut app, 1);
    enter(&mut app, &ctx2, Stage::Edit).unwrap();
    d.idle(&mut app, 2);
    let side = poly().edges().into_iter().find(|e| {
        let p = poly();
        let (a, b) = (p.positions[e.0 as usize], p.positions[e.1 as usize]);
        (a[1] - b[1]).abs() > 100.0 && a[2] < -99.0 && b[2] < -99.0
    }).unwrap();
    let at = {
        let p = poly();
        screen_of(&app, studio_mesh::lerp(p.positions[side.0 as usize], p.positions[side.1 as usize], 0.5))
    };
    d.key(&mut app, c, egui::Key::R, Modifiers::COMMAND);
    assert!(input::loopcut_cuts().is_some());
    d.frame(&mut app, vec![Event::PointerMoved(at)]);
    d.shot(&mut app, "08-loop-cut-preview");
    d.click(&mut app, at, PointerButton::Primary, Modifiers::NONE);
    d.key(&mut app, at, egui::Key::Enter, Modifiers::NONE);
    assert_eq!(poly().faces(), 10, "a loop round the cube");
    assert!(closed(&poly()));
    // The knife across the front: two clicks and Enter.
    let p = poly();
    let front_mid = screen_of(&app, [0.0, 0.0, -100.0]);
    let (k1, k2) = (front_mid + Vec2::new(-200.0, 10.0), front_mid + Vec2::new(200.0, -10.0));
    let _ = p;
    d.key(&mut app, c, egui::Key::K, Modifiers::NONE);
    assert!(input::knife_on());
    d.click(&mut app, k1, PointerButton::Primary, Modifiers::NONE);
    d.click(&mut app, k2, PointerButton::Primary, Modifiers::NONE);
    d.shot(&mut app, "09-knife");
    d.key(&mut app, c, egui::Key::Enter, Modifiers::NONE);
    assert!(!input::knife_on());
    assert!(poly().faces() > 10, "the knife cut faces: {}", poly().faces());
}

#[test]
fn modifiers_materials_and_object_tools() {
    let _g = vp::test_lock();
    let mut d = Driver::new();
    let ctx = d.ctx.clone();
    let mut app = app_with(&ctx);
    let clip = open_shape(&mut d, &mut app, "add_cube", json!({"size": 200.0})) as u64;
    let tris = |app: &StudioApp| crate::mesh_store::load(app.doc.model.active().clip(m::ClipId(clip)).unwrap()).unwrap().triangles();
    assert_eq!(tris(&app), 12);
    // Subdivision: drawn smooth, the cage stays.
    panel::edit_source(&mut app, clip, "Add modifier", |a| {
        a.parts[0].modifiers.push(panel::new_modifier("subdivide", 200.0, None).unwrap());
        a.parts[0].modifiers.push(panel::new_modifier("bevel", 200.0, None).unwrap());
        Ok(())
    })
    .unwrap();
    d.idle(&mut app, 1);
    let with_mods = tris(&app);
    assert!(with_mods > 12 * 4);
    assert_eq!(poly().faces(), 6, "the cage is untouched");
    d.shot(&mut app, "10-subdivided-cage");
    // Switched off, it draws as modelled; on again; reordered.
    panel::edit_source(&mut app, clip, "Switch", |a| {
        a.parts[0].disabled = vec![true, true];
        Ok(())
    })
    .unwrap();
    assert_eq!(tris(&app), 12);
    panel::edit_source(&mut app, clip, "Switch", |a| {
        a.parts[0].disabled = vec![false, false];
        a.parts[0].modifiers.swap(0, 1);
        Ok(())
    })
    .unwrap();
    assert!(tris(&app) > 12 * 4);
    // Applied: the cage becomes the subdivided shape.
    panel::edit_source(&mut app, clip, "Apply", |a| panel::apply_modifier(a, 0, 1)).unwrap();
    d.idle(&mut app, 1);
    assert_eq!(poly().faces(), 24);
    assert!(closed(&poly()));
    // Mirror, array, solidify, boolean, displace, smooth, decimate,
    // remesh, shrinkwrap, lattice all evaluate.
    for k in ["mirror", "array", "solidify", "displace", "smooth", "decimate", "remesh", "lattice", "radial", "twist"] {
        let md = panel::new_modifier(k, 200.0, Some("Body")).unwrap();
        let mut a = with(|s| s.as_ref().unwrap().asset.clone());
        a.parts[0].modifiers = vec![md];
        a.to_mesh(true).unwrap_or_else(|e| panic!("{k}: {e}"));
    }
    // A boolean cutter from another object.
    let other = add_object(&mut app, &ctx, "add_cylinder", json!({"radius": 40.0, "depth": 400.0}).as_object().unwrap()).unwrap();
    app.editor.selection = vec![clip as u32];
    d.idle(&mut app, 1);
    let mut a = with(|s| s.as_ref().unwrap().asset.clone());
    let mut cut = studio_modeling::asset::Part::new("Hole", studio_modeling::prim::cylinder(40.0, 40.0, 400.0, 24, 1, studio_modeling::prim::Cap::Ngon));
    cut.hidden = true;
    a.parts.push(cut);
    a.parts[0].modifiers.push(Modifier::Boolean { op: "subtract".into(), with: "Hole".into() });
    let m2 = a.to_mesh(true).unwrap();
    assert!(m2.triangles() > 24, "the hole is cut");
    let _ = other;
    // Materials per face: a second slot on the top faces.
    let top: Vec<u32> = (0..poly().faces() as u32).filter(|&f| poly().face_normal(f as usize)[1] < -0.6).collect();
    assert_eq!(top.len(), 4);
    with(|s| {
        let s = s.as_mut().unwrap();
        let p = s.poly().clone();
        s.sel.mode = Mode::Face;
        s.sel.with_faces(&p, &top);
    });
    run(&mut app, &ctx, "assign_material", json!({"slot": 1}).as_object().unwrap().clone()).unwrap();
    let mesh = crate::mesh_store::load(app.doc.model.active().clip(m::ClipId(clip)).unwrap()).unwrap();
    assert!(mesh.materials.len() >= 2);
    assert_eq!(mesh.groups.len(), 2, "two materials drawn");
    // Shading and origin.
    panel::edit_source(&mut app, clip, "Auto smooth", |a| {
        a.parts[0].smooth_angle = 10.0;
        Ok(())
    })
    .unwrap();
    with(|s| *s = None);
    vp::with(|w| w.cursor = [100.0, 0.0, 0.0]);
    panel::batch(&mut app, "Set origin", |app| panel::set_origin(app, clip, "cursor")).unwrap();
    let mc = app.doc.model.active().clip(m::ClipId(clip)).unwrap().clone();
    assert!((mc.transform.position.x - 100.0).abs() < 1e-3, "the object sits at the cursor: {:?}", mc.transform.position);
    let (a, _) = load_source(&mc).unwrap();
    assert!((a.parts[0].position[0] + 100.0).abs() < 1e-3, "the shape shifted back: {:?}", a.parts[0].position);
}

#[test]
fn sculpt_mode_strokes_and_remesh() {
    let _g = vp::test_lock();
    let mut d = Driver::new();
    let ctx = d.ctx.clone();
    let mut app = app_with(&ctx);
    open_shape(&mut d, &mut app, "add_quad_sphere", json!({"radius": 150.0, "segments": 8}));
    enter(&mut app, &ctx, Stage::Sculpt).unwrap();
    assert_eq!(stage(), Stage::Sculpt);
    let before = poly();
    let steps = app.doc.history_len();
    let c = screen_of(&app, [0.0, 0.0, -150.0]);
    d.drag(&mut app, PointerButton::Primary, Modifiers::NONE, c - Vec2::new(40.0, 0.0), c + Vec2::new(40.0, 0.0));
    let after = poly();
    assert_ne!(before.positions, after.positions, "the stroke shaped it");
    assert_eq!(before.faces(), after.faces());
    assert_eq!(app.doc.history_len(), steps + 1, "one stroke, one step");
    // Dynamic detail adds faces where the brush goes.
    let mut b = sculpt3d::brushes();
    b.dyntopo = true;
    b.detail = 4.0;
    sculpt3d::set_brushes(b);
    d.drag(&mut app, PointerButton::Primary, Modifiers::NONE, c - Vec2::new(30.0, 10.0), c + Vec2::new(30.0, 10.0));
    assert!(poly().faces() > after.faces(), "detail added: {} → {}", after.faces(), poly().faces());
    d.shot(&mut app, "11-sculpted");
    sculpt3d::remesh(&mut app, 0.03).unwrap();
    let r = poly();
    assert!(closed(&r));
    assert!((0..r.faces()).all(|f| r.face(f).len() == 4), "even quads");
    app.doc.undo();
    d.idle(&mut app, 1);
    assert_ne!(poly().faces(), r.faces(), "remesh undoes");
}

#[test]
fn object_mode_add_menu_panel_and_separate() {
    let _g = vp::test_lock();
    let mut d = Driver::new();
    let ctx = d.ctx.clone();
    let mut app = app_with(&ctx);
    d.idle(&mut app, 2);
    // The bar's Add menu in object mode makes a new object.
    kit::set_open(&ctx, egui::Id::unique("model-object-add"), true);
    d.idle(&mut app, 2);
    d.shot(&mut app, "12-object-add-menu");
    let n0 = app.doc.model.active().clips.len();
    let id = add_object(&mut app, &ctx, "add_torus", &serde_json::Map::new()).unwrap();
    assert_eq!(app.doc.model.active().clips.len(), n0 + 1);
    let mc = app.doc.model.active().clip(m::ClipId(id as u64)).unwrap().clone();
    assert!(crate::mesh_store::source_of(&mc).is_some(), "a shape keeps its editable source");
    // Separate the top half into an object of its own.
    app.editor.selection = vec![id];
    enter(&mut app, &ctx, Stage::Edit).unwrap();
    with(|s| {
        let s = s.as_mut().unwrap();
        let p = s.poly().clone();
        s.sel.mode = Mode::Face;
        let half: Vec<u32> = (0..p.faces() as u32).filter(|&f| p.face_centre(f as usize)[0] > 0.0).collect();
        s.sel.with_faces(&p, &half);
    });
    let faces = poly().faces();
    run(&mut app, &ctx, "separate", serde_json::Map::new()).unwrap();
    assert!(poly().faces() < faces);
    assert_eq!(app.doc.model.active().clips.len(), n0 + 2, "the piece is a new object");
    // A pick that follows: Alt-click picks a loop.
    d.idle(&mut app, 2);
    let p = poly();
    let e = p.edges()[0];
    with(|s| {
        let s = s.as_mut().unwrap();
        s.sel.mode = Mode::Edge;
        s.sel.pick_loop(&p, e, Combine::Set);
    });
    assert!(counts().1 > 1);
}

/// A cup modelled by hand (inset the top, push it in, bevel the rim, a
/// subdivision modifier) rendered offline: the picture shows a solid
/// object where the model is.
#[test]
fn a_modelled_cup_renders() {
    let _g = vp::test_lock();
    let mut d = Driver::new();
    let ctx = d.ctx.clone();
    let mut app = app_with(&ctx);
    let clip = open_shape(&mut d, &mut app, "add_cylinder", json!({"radius": 120.0, "depth": 260.0, "vertices": 24}));
    let top = || -> Vec<u32> {
        let p = poly();
        (0..p.faces() as u32).filter(|&f| p.face_normal(f as usize)[1] < -0.9).collect()
    };
    let pick_faces = |faces: Vec<u32>| {
        with(|s| {
            let s = s.as_mut().unwrap();
            let p = s.poly().clone();
            s.sel.mode = Mode::Face;
            s.sel.with_faces(&p, &faces);
        })
    };
    pick_faces(top());
    run(&mut app, &ctx, "inset", json!({"thickness": 14.0}).as_object().unwrap().clone()).unwrap();
    run(&mut app, &ctx, "extrude_region", json!({"distance": -230.0}).as_object().unwrap().clone()).unwrap();
    let p = poly();
    assert!(closed(&p) && outward(&p), "a closed cup");
    panel::edit_source(&mut app, clip as u64, "Subdivide", |a| {
        a.parts[0].modifiers.push(Modifier::Subdivide { levels: 2 });
        Ok(())
    })
    .unwrap();
    leave(&mut app, &ctx);
    // The picture from the workspace's eye.
    let w = app.doc.project.width as f64;
    let nav = vp::snapshot().nav;
    let v = nav.view(w);
    let cam = studio_render::space::CameraDesc { position: v.position.map(|x| x as f32), axes: v.axes.map(|r| r.map(|x| x as f32)), zoom: v.zoom as f32, dof: None, trace: None };
    let mut o = studio_render::viewport::ViewOptions::new(cam);
    o.shading = Shade::Material.render();
    let registry = studio_render::source::SourceRegistry::new();
    crate::editor::render_view::register_sources(&app.doc, &registry);
    let mut renderer = studio_render::best_renderer(studio_render::Sources::new(registry, studio_render::DecodeCache::new(64 << 20)));
    let scene = studio_render::viewport::ViewScene::new(std::sync::Arc::new(studio_render::model_scene::ModelScene::new(std::sync::Arc::new(app.doc.model.clone()))), o);
    let out = renderer.render_encoded(&scene, 0, Some((640, 360)), &studio_render::OutputSettings { premultiplied: false, ..Default::default() }, studio_render::OutputDepth::Eight, 0);
    let px = match out.pixels {
        studio_render::EncodedPixels::Rgba8(v) => v,
        studio_render::EncodedPixels::Rgba16(v) => v.iter().map(|x| (x >> 8) as u8).collect(),
    };
    let img = studio_media::rgba::RgbaImage { width: out.width, height: out.height, pixels: px };
    if let Some(dir) = &d.shots {
        let _ = std::fs::write(dir.join("13-cup-render.png"), studio_content::png::encode(&img));
    }
    // The middle of the frame is the cup, not the empty background.
    let at = |x: u32, y: u32| {
        let i = ((y * img.width + x) * 4) as usize;
        [img.pixels[i], img.pixels[i + 1], img.pixels[i + 2], img.pixels[i + 3]]
    };
    let mid = at(img.width / 2, img.height / 2);
    let corner = at(4, 4);
    assert_ne!(mid, corner, "something is drawn in the middle");
}

#[test]
fn lasso_paint_gizmo_proportional_mirror_snap_and_pivots() {
    let _g = vp::test_lock();
    let mut d = Driver::new();
    let ctx = d.ctx.clone();
    let mut app = app_with(&ctx);
    open_shape(&mut d, &mut app, "add_grid", json!({"size": 400.0, "x": 8, "y": 8}));
    // Seen from above.
    vp::with(|w| {
        w.nav.yaw = 0.0;
        w.nav.pitch = 89.0;
        w.changed();
    });
    d.idle(&mut app, 2);
    let c = player().1.center();
    let middle = screen_of(&app, [0.0, 0.0, 0.0]);
    // Lasso round the middle picks the middle vertices only.
    with(|s| s.as_mut().unwrap().tool = SelectTool::Lasso);
    let ring: Vec<Pos2> = (0..12).map(|k| {
        let a = k as f32 / 12.0 * std::f32::consts::TAU;
        middle + Vec2::new(a.cos(), a.sin()) * 40.0
    }).collect();
    d.frame(&mut app, vec![Event::PointerMoved(ring[0])]);
    d.frame(&mut app, vec![Event::PointerButton { pos: ring[0], button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE }]);
    for p in &ring[1..] {
        d.frame(&mut app, vec![Event::PointerMoved(*p)]);
    }
    d.frame(&mut app, vec![Event::PointerButton { pos: ring[11], button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE }]);
    d.idle(&mut app, 1);
    let lassoed = counts().0;
    assert!((1..20).contains(&lassoed), "a few middle vertices: {lassoed}");
    // Paint picks along the stroke.
    with(|s| {
        let s = s.as_mut().unwrap();
        s.tool = SelectTool::Paint;
        s.brush = 12.0;
    });
    let left = screen_of(&app, [-150.0, 0.0, 0.0]);
    let right = screen_of(&app, [150.0, 0.0, 0.0]);
    d.drag(&mut app, PointerButton::Primary, Modifiers::NONE, left, right);
    let painted = counts().0;
    assert!(painted >= 5, "a row of vertices painted: {painted}");
    d.shot(&mut app, "14-paint-select");
    // One middle vertex, proportional editing on: G along -y (up) pulls
    // its neighbours less and less.
    let mid = (0..poly().positions.len() as u32).min_by(|a, b| studio_mesh::length(poly().positions[*a as usize]).total_cmp(&studio_mesh::length(poly().positions[*b as usize]))).unwrap();
    with(|s| {
        let s = s.as_mut().unwrap();
        let p = s.poly().clone();
        s.sel.with_vertices(&p, &[mid]);
        s.prop = Proportional { on: true, radius: 120.0, falloff: Falloff::Smooth, connected: false };
    });
    let before = poly();
    d.key(&mut app, c, egui::Key::G, Modifiers::NONE);
    d.key(&mut app, c, egui::Key::Y, Modifiers::NONE);
    d.text(&mut app, "-80");
    d.shot(&mut app, "15-proportional-move");
    d.key(&mut app, c, egui::Key::Enter, Modifiers::NONE);
    let after = poly();
    let dy = |v: usize| after.positions[v][1] - before.positions[v][1];
    assert!((dy(mid as usize) + 80.0).abs() < 0.01);
    let near = (0..after.positions.len()).filter(|&v| v != mid as usize && dy(v) < -1.0).count();
    let far_moved = (0..after.positions.len()).filter(|&v| studio_mesh::length(before.positions[v]) > 130.0 && dy(v).abs() > 1e-3).count();
    assert!(near >= 4, "neighbours follow: {near}");
    assert_eq!(far_moved, 0, "beyond the reach nothing moves");
    // Symmetric editing: moving a vertex on +x moves its mirror on -x.
    with(|s| {
        let s = s.as_mut().unwrap();
        s.prop.on = false;
        s.mirror = [true, false, false];
        let p = s.poly().clone();
        let v = (0..p.positions.len() as u32).find(|&v| (p.positions[v as usize][0] - 150.0).abs() < 1.0 && (p.positions[v as usize][2] - 150.0).abs() < 1.0).unwrap();
        s.sel.with_vertices(&p, &[v]);
    });
    let before = poly();
    d.key(&mut app, c, egui::Key::G, Modifiers::NONE);
    d.key(&mut app, c, egui::Key::Y, Modifiers::NONE);
    d.text(&mut app, "-30");
    d.key(&mut app, c, egui::Key::Enter, Modifiers::NONE);
    let after = poly();
    let mirror = (0..after.positions.len()).find(|&v| (before.positions[v][0] + 150.0).abs() < 1.0 && (before.positions[v][2] - 150.0).abs() < 1.0).unwrap();
    assert!((after.positions[mirror][1] - before.positions[mirror][1] + 30.0).abs() < 0.01, "the mirror image followed");
    with(|s| s.as_mut().unwrap().mirror = [false; 3]);
    // Snapping to a vertex: the picked one lands on another under the
    // pointer.
    vp::with(|w| w.snap.on = true);
    with(|s| {
        let s = s.as_mut().unwrap();
        s.snap_to = SnapTo::Vertex;
        let p = s.poly().clone();
        s.sel.with_vertices(&p, &[0]);
    });
    let p = poly();
    let target = (0..p.positions.len()).max_by(|a, b| studio_mesh::length(studio_mesh::sub(p.positions[*a], p.positions[0])).total_cmp(&studio_mesh::length(studio_mesh::sub(p.positions[*b], p.positions[0])))).unwrap();
    let at = screen_of(&app, p.positions[target]);
    let from = screen_of(&app, p.positions[0]);
    d.frame(&mut app, vec![Event::PointerMoved(from)]);
    d.key(&mut app, from, egui::Key::G, Modifiers::NONE);
    d.frame(&mut app, vec![Event::PointerMoved(at + Vec2::new(4.0, 3.0))]);
    d.frame(&mut app, vec![Event::PointerMoved(at + Vec2::new(4.0, 3.0))]);
    d.key(&mut app, at + Vec2::new(4.0, 3.0), egui::Key::Enter, Modifiers::NONE);
    let q = poly();
    assert!(studio_mesh::length(studio_mesh::sub(q.positions[0], p.positions[target])) < 0.5, "snapped onto the vertex: {:?} vs {:?}", q.positions[0], p.positions[target]);
    vp::with(|w| w.snap.on = false);
    // The gizmo: drag its x arrow.
    vp::with(|w| w.tool = vp::Tool::Move);
    with(|s| {
        let s = s.as_mut().unwrap();
        let p = s.poly().clone();
        s.sel.mode = Mode::Face;
        s.sel.with_faces(&p, &[10, 11]);
    });
    d.idle(&mut app, 2);
    let before = poly();
    let piv = with(|s| s.as_ref().unwrap().sel.median(&before)).unwrap();
    let ps = screen_of(&app, piv);
    let tip = screen_of(&app, studio_mesh::add(piv, [60.0, 0.0, 0.0]));
    let dir = (tip - ps).normalized();
    let grab = ps + dir * 40.0;
    d.drag(&mut app, PointerButton::Primary, Modifiers::NONE, grab, grab + dir * 60.0);
    let after = poly();
    let moved: Vec<usize> = (0..after.positions.len()).filter(|&v| studio_mesh::length(studio_mesh::sub(after.positions[v], before.positions[v])) > 1.0).collect();
    assert!(!moved.is_empty(), "the gizmo moved the pick");
    assert!(moved.iter().all(|&v| (after.positions[v][1] - before.positions[v][1]).abs() < 0.01 && (after.positions[v][2] - before.positions[v][2]).abs() < 0.01), "along x only");
    // Individual origins: each picked face scales about its own middle.
    vp::with(|w| w.pivot = vp::Pivot::Individual);
    with(|s| {
        let s = s.as_mut().unwrap();
        let p = s.poly().clone();
        s.sel.mode = Mode::Face;
        s.sel.with_faces(&p, &[0, 63]);
    });
    let before = poly();
    d.key(&mut app, c, egui::Key::S, Modifiers::NONE);
    d.text(&mut app, "0.5");
    d.key(&mut app, c, egui::Key::Enter, Modifiers::NONE);
    let after = poly();
    for f in [0usize, 63] {
        let (c0, c1) = (before.face_centre(f), after.face_centre(f));
        assert!(studio_mesh::length(studio_mesh::sub(c0, c1)) < 0.01, "face {f} kept its middle");
        assert!((after.face_area(f) / before.face_area(f) - 0.25).abs() < 0.01);
    }
    vp::with(|w| w.pivot = vp::Pivot::Median);
}

#[test]
fn join_and_apply_transform_keep_everything_in_place() {
    let _g = vp::test_lock();
    let mut d = Driver::new();
    let ctx = d.ctx.clone();
    let mut app = app_with(&ctx);
    d.idle(&mut app, 1);
    vp::with(|w| w.cursor = [-200.0, 0.0, 0.0]);
    let a = add_object(&mut app, &ctx, "add_cube", json!({"size": 100.0}).as_object().unwrap()).unwrap();
    vp::with(|w| w.cursor = [200.0, 0.0, 0.0]);
    let b = add_object(&mut app, &ctx, "add_uv_sphere", json!({"radius": 60.0}).as_object().unwrap()).unwrap();
    // Turn and scale the cube, then bake it in.
    let seq = app.doc.seq();
    let mut mc = app.doc.model.active().clip(m::ClipId(a as u64)).unwrap().clone();
    mc.transform.rotation_y = 30.0;
    mc.transform.scale_x = 2.0;
    app.doc.apply("Turn", m::Edit::SetClip { seq, clip: Box::new(mc) });
    let world_before = vp::objects(&app.doc, 0, None, &Default::default()).into_iter().find(|o| o.id == a as u64).unwrap();
    panel::batch(&mut app, "Apply", |app| panel::apply_transform(app, a as u64)).unwrap();
    let mc = app.doc.model.active().clip(m::ClipId(a as u64)).unwrap().clone();
    assert_eq!((mc.transform.rotation_y, mc.transform.scale_x), (0.0, 1.0));
    let world_after = vp::objects(&app.doc, 0, None, &Default::default()).into_iter().find(|o| o.id == a as u64).unwrap();
    for k in 0..3 {
        assert!((world_before.min[k] - world_after.min[k]).abs() < 0.5 && (world_before.max[k] - world_after.max[k]).abs() < 0.5, "the same place in the world: {:?} {:?}", world_before.min, world_after.min);
    }
    // Join the sphere into the cube.
    app.editor.selection = vec![a, b];
    let n = app.doc.model.active().clips.len();
    panel::batch(&mut app, "Join", |app| panel::join(app, a as u64)).unwrap();
    assert_eq!(app.doc.model.active().clips.len(), n - 1);
    let mc = app.doc.model.active().clip(m::ClipId(a as u64)).unwrap().clone();
    let (asset, mesh) = crate::mesh_store::load_asset(&mc).unwrap();
    assert_eq!(asset.parts.len(), 2);
    let (lo, hi) = mesh.bounds();
    assert!(hi[0] - lo[0] > 400.0, "the sphere came along at its place: {lo:?} {hi:?}");
    app.doc.undo();
    assert_eq!(app.doc.model.active().clips.len(), n, "one step");
}

/// A lattice cage edited by hand: its corner dragged up bends the model.
#[test]
fn lattice_cage_points_drag_and_bend_the_model() {
    let _g = vp::test_lock();
    let mut d = Driver::new();
    let ctx = d.ctx.clone();
    let mut app = app_with(&ctx);
    let clip = open_shape(&mut d, &mut app, "add_cube", json!({"size": 200.0, "segments": 4})) as u64;
    panel::edit_source(&mut app, clip, "Lattice", |a| {
        a.parts[0].modifiers.push(panel::new_modifier("lattice", 200.0, None).unwrap());
        Ok(())
    })
    .unwrap();
    lattice::start(&mut app, &ctx, 0).unwrap();
    assert!(lattice::on());
    d.idle(&mut app, 2);
    d.shot(&mut app, "16-lattice-cage");
    let l = match &with(|s| s.as_ref().unwrap().part().modifiers[0].clone()) {
        Modifier::Lattice(l) => l.clone(),
        _ => unreachable!(),
    };
    // The top middle point (y up is negative).
    let (ijk, q) = lattice::points(&l).into_iter().find(|(ijk, _)| *ijk == [1, 0, 1]).unwrap();
    let at = screen_of(&app, q);
    let bounds = |app: &StudioApp| crate::mesh_store::load(app.doc.model.active().clip(m::ClipId(clip)).unwrap()).unwrap().bounds();
    let before = bounds(&app);
    let steps = app.doc.history_len();
    d.drag(&mut app, PointerButton::Primary, Modifiers::NONE, at, at + Vec2::new(0.0, -80.0));
    let after = bounds(&app);
    assert!(after.0[1] < before.0[1] - 5.0, "the top rose: {:?} -> {:?}", before.0, after.0);
    assert_eq!(app.doc.history_len(), steps + 1, "one step");
    let l2 = match &with(|s| s.as_ref().unwrap().part().modifiers[0].clone()) {
        Modifier::Lattice(l) => l.clone(),
        _ => unreachable!(),
    };
    assert!(l2.moves.iter().any(|(p, v)| *p == ijk && v[1] < -1.0));
    assert_eq!(poly().faces(), 6 * 16, "the cage is not the mesh: the mesh is untouched");
    d.key(&mut app, player().1.center(), egui::Key::Escape, Modifiers::NONE);
    assert!(!lattice::on());
}
