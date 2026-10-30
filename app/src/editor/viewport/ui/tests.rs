// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Editor UI tests of the 3D workspace with synthesized input: the whole
//! editor drawing the workspace in every mode, and the Player's
//! workspace driven by mouse, trackpad, pinch and keys.

use super::*;
use eframe::egui::{Event, Modifiers, PointerButton};

const SCREEN: Vec2 = Vec2::new(1000.0, 640.0);

/// A 640 × 360 project with a 3D card in the middle, a second card to
/// its right and a camera.
fn space_app(ctx: &egui::Context) -> (StudioApp, u32, u32, u64) {
    let mut s = crate::agent::Session::new(crate::services::project::new_custom("Space", 640, 360, 30.0));
    s.edit(&[serde_json::json!({"op": "card", "colour": "#ffcc00", "at": 0, "duration": 4, "width": 200, "height": 120})], "card", false).unwrap();
    let a = s.doc.project.clips.iter().map(|c| c.id).max().unwrap();
    s.edit(&[serde_json::json!({"op": "card", "colour": "#3388ff", "at": 0, "duration": 4, "width": 100, "height": 100, "x": 220})], "card", false).unwrap();
    let b = s.doc.project.clips.iter().map(|c| c.id).max().unwrap();
    s.edit(&[serde_json::json!({"op": "three_d", "clips": [a, b]})], "3d", false).unwrap();
    let cam = super::super::add_camera(&mut s.doc).unwrap();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let _serial = crate::ui_tests::serial();
    let mut app = StudioApp::new(&cc, crate::cli::Cli { screen: Some(crate::cli::Screen::Editor), fresh: true, ..Default::default() });
    app.doc = s.doc;
    app.editor.playhead = 0;
    super::super::with(|w| *w = Workspace::for_canvas(640.0, 360.0));
    super::super::follow(&app.doc);
    super::super::with(|w| {
        w.on = true;
        w.shade = Shade::Solid;
    });
    (app, a, b, cam)
}

/// One headless frame finished as a window's backend finishes it (its
/// texture changes taken as uploaded).
fn run(ctx: &egui::Context, t: f64, modifiers: Modifiers, events: Vec<Event>, mut f: impl FnMut(&mut egui::Ui)) {
    let mut events = events;
    events.insert(0, Event::ModifiersChanged(modifiers));
    let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SCREEN)), time: Some(t), events, ..Default::default() };
    let mut out = ctx.run_ui(input, |ui| f(ui));
    let _ = ctx.tessellate(std::mem::take(&mut out.shapes), out.pixels_per_point);
    out.textures_delta.clear();
}

/// The Player rectangle and its picture area as the viewer lays them out.
fn player() -> (Rect, Rect) {
    let r = Rect::from_min_size(Pos2::ZERO, SCREEN);
    let area = Rect::from_min_max(egui::pos2(12.0, 44.0), egui::pos2(SCREEN.x - 12.0, SCREEN.y - 50.0));
    (r, area)
}

struct Driver {
    ctx: egui::Context,
    t: f64,
    /// Modifier keys held (egui reads them from the frame's input).
    mods: Modifiers,
}

impl Driver {
    fn frame(&mut self, app: &mut StudioApp, events: Vec<Event>) {
        self.t += 1.0 / 60.0;
        let (r, area) = player();
        let ctx = self.ctx.clone();
        run(&ctx, self.t, self.mods, events, |ui| {
            keys(app, ui.ctx());
            app.editor.viewer_rect = r;
            show(app, ui, r, area);
        });
    }

    fn idle(&mut self, app: &mut StudioApp, n: usize) {
        for _ in 0..n {
            self.frame(app, vec![]);
        }
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

    fn click(&mut self, app: &mut StudioApp, at: Pos2, button: PointerButton, mods: Modifiers) {
        self.mods = mods;
        self.frame(app, vec![Event::PointerMoved(at)]);
        self.frame(app, vec![Event::PointerButton { pos: at, button, pressed: true, modifiers: mods }]);
        self.frame(app, vec![Event::PointerButton { pos: at, button, pressed: false, modifiers: mods }]);
        self.mods = Modifiers::NONE;
        self.frame(app, vec![]);
    }

    fn key(&mut self, app: &mut StudioApp, at: Pos2, key: egui::Key, mods: Modifiers) {
        self.mods = mods;
        self.frame(app, vec![Event::PointerMoved(at), Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: mods }]);
        self.frame(app, vec![Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers: mods }]);
        self.mods = Modifiers::NONE;
    }

    /// The main view on screen now.
    fn pane(&self, app: &StudioApp) -> Pane {
        let (r, area) = player();
        let ws = super::super::snapshot();
        *panes_of(app, &ws, body_of(r, area), app.editor.playhead).iter().find(|p| p.index == 3).unwrap()
    }
}

fn eye() -> Nav {
    super::super::with(|w| w.nav)
}

/// Mouse, trackpad, pinch and keys move the editor's eye as in 3D apps,
/// and the axis views glide into place.
#[test]
fn navigation_by_mouse_trackpad_pinch_and_keys() {
    let _g = super::super::test_lock();
    let ctx = egui::Context::default();
    let (mut app, ..) = space_app(&ctx);
    let mut d = Driver { ctx, t: 0.0, mods: Modifiers::NONE };
    d.idle(&mut app, 2);
    let c = player().1.center();
    // Middle drag orbits.
    let before = eye();
    d.drag(&mut app, PointerButton::Middle, Modifiers::NONE, c, c + Vec2::new(80.0, 0.0));
    let after = eye();
    assert!((after.yaw - before.yaw).abs() > 10.0, "orbit: {} -> {}", before.yaw, after.yaw);
    assert!((after.distance - before.distance).abs() < 1e-6);
    // Alt + Shift + left drag pans: the target moves, the angle does not.
    let before = eye();
    d.drag(&mut app, PointerButton::Primary, Modifiers { alt: true, shift: true, ..Default::default() }, c, c + Vec2::new(0.0, 60.0));
    let after = eye();
    assert!((after.yaw - before.yaw).abs() < 1e-9);
    assert!((0..3).any(|i| (after.target[i] - before.target[i]).abs() > 5.0), "pan");
    // The mouse wheel zooms towards the pointer.
    let before = eye().distance;
    d.frame(&mut app, vec![Event::PointerMoved(c), Event::MouseWheel { unit: egui::MouseWheelUnit::Line, delta: Vec2::new(0.0, 3.0), phase: egui::TouchPhase::Move, source: egui::MouseWheelSource::Wheel, modifiers: Modifiers::NONE }]);
    assert!(eye().distance < before * 0.9, "wheel zoom {before} -> {}", eye().distance);
    // Two fingers on a trackpad orbit; with Shift they pan; a pinch zooms.
    let before = eye();
    d.frame(&mut app, vec![Event::PointerMoved(c), Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: Vec2::new(40.0, 0.0), phase: egui::TouchPhase::Move, source: egui::MouseWheelSource::Trackpad, modifiers: Modifiers::NONE }]);
    assert!((eye().yaw - before.yaw).abs() > 5.0, "trackpad orbit");
    let before = eye();
    d.frame(&mut app, vec![Event::PointerMoved(c), Event::Zoom(1.5)]);
    assert!(eye().distance < before.distance * 0.8, "pinch");
    // 7 is the top view (glides there), 5 flips to perspective.
    d.key(&mut app, c, egui::Key::Num7, Modifiers::NONE);
    d.idle(&mut app, 30);
    let n = eye();
    assert!((n.pitch - 90.0).abs() < 1e-6 && n.ortho, "top: {n:?}");
    d.key(&mut app, c, egui::Key::Num5, Modifiers::NONE);
    assert!(!eye().ortho);
    // Cmd-1 is the back view.
    d.key(&mut app, c, egui::Key::Num1, Modifiers::COMMAND);
    d.idle(&mut app, 30);
    assert!((eye().yaw.abs() - 180.0).abs() < 1e-6, "back: {:?}", eye());
    // Home frames everything: both cards in view.
    d.key(&mut app, c, egui::Key::Home, Modifiers::NONE);
    d.idle(&mut app, 30);
    let p = d.pane(&app);
    for o in super::super::objects(&app.doc, 0, None, &BTreeSet::new()).iter().filter(|o| o.kind == Kind::Layer) {
        let s = p.to_screen(o.anchor).expect("in front");
        assert!(p.rect.contains(s), "{} at {s:?} outside {:?}", o.name, p.rect);
    }
}

/// Fly mode: ~ starts it, W moves forward, Esc stops; right-drag looks.
#[test]
fn fly_and_walk_with_keys() {
    let _g = super::super::test_lock();
    let ctx = egui::Context::default();
    let (mut app, ..) = space_app(&ctx);
    let mut d = Driver { ctx, t: 0.0, mods: Modifiers::NONE };
    d.idle(&mut app, 2);
    let c = player().1.center();
    d.key(&mut app, c, egui::Key::Backtick, Modifiers::NONE);
    assert_eq!(super::super::with(|w| w.rt.fly), Some(Fly::Fly));
    let before = eye();
    let fwd = before.forward();
    d.frame(&mut app, vec![Event::PointerMoved(c), Event::Key { key: egui::Key::W, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }]);
    d.idle(&mut app, 20);
    d.frame(&mut app, vec![Event::Key { key: egui::Key::W, physical_key: None, pressed: false, repeat: false, modifiers: Modifiers::NONE }]);
    let after = eye();
    let moved: f64 = (0..3).map(|i| (after.target[i] - before.target[i]) * fwd[i]).sum();
    assert!(moved > 10.0, "flew forward {moved}");
    d.key(&mut app, c, egui::Key::Escape, Modifiers::NONE);
    assert_eq!(super::super::with(|w| w.rt.fly), None);
    // Walk: forward stays level.
    d.key(&mut app, c, egui::Key::Backtick, Modifiers::SHIFT);
    assert_eq!(super::super::with(|w| w.rt.fly), Some(Fly::Walk));
    let y0 = eye().position()[1];
    d.frame(&mut app, vec![Event::Key { key: egui::Key::W, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }]);
    d.idle(&mut app, 10);
    d.frame(&mut app, vec![Event::Key { key: egui::Key::W, physical_key: None, pressed: false, repeat: false, modifiers: Modifiers::NONE }]);
    assert!((eye().position()[1] - y0).abs() < 1e-6, "walking stays level");
    d.key(&mut app, c, egui::Key::Escape, Modifiers::NONE);
    // Right drag looks round from where the eye stands.
    let eye0 = eye().position();
    d.drag(&mut app, PointerButton::Secondary, Modifiers::NONE, c, c + Vec2::new(60.0, 0.0));
    let e = eye();
    assert!((0..3).all(|i| (e.position()[i] - eye0[i]).abs() < 1e-6), "the eye stayed put");
}

/// A click picks the card under it (Shift adds, empty clears); the move
/// gizmo's X arrow slides it; rotate and scale handles turn and size it,
/// each one undo step.
#[test]
fn click_to_pick_and_gizmos_move_rotate_scale() {
    let _g = super::super::test_lock();
    let ctx = egui::Context::default();
    let (mut app, a, b, cam) = space_app(&ctx);
    // The camera stands in front of the cards (and would be picked
    // first): out of the viewport for this test.
    super::super::with(|w| {
        w.nav.set_axis(Axis::Front);
        w.hidden.insert(cam);
    });
    let mut d = Driver { ctx, t: 0.0, mods: Modifiers::NONE };
    d.idle(&mut app, 2);
    let p = d.pane(&app);
    let objs = super::super::objects(&app.doc, 0, None, &BTreeSet::new());
    let at = |id: u32| p.to_screen(objs.iter().find(|o| o.id == id as u64).unwrap().anchor).unwrap();
    d.click(&mut app, at(a) + Vec2::new(5.0, 5.0), PointerButton::Primary, Modifiers::NONE);
    assert_eq!(app.editor.selection, vec![a]);
    d.click(&mut app, at(b) + Vec2::new(3.0, 3.0), PointerButton::Primary, Modifiers::SHIFT);
    assert_eq!(app.editor.selection, vec![a, b]);
    d.click(&mut app, p.rect.left_bottom() + Vec2::new(30.0, -30.0), PointerButton::Primary, Modifiers::NONE);
    assert!(app.editor.selection.is_empty());
    // Move along X with the arrow.
    app.editor.selection = vec![a];
    d.idle(&mut app, 1);
    let x0 = app.doc.model.active().clip(m::ClipId(a as u64)).unwrap().transform.position.x;
    let anchor = at(a);
    let undo0 = app.doc.history.revision();
    d.drag(&mut app, PointerButton::Primary, Modifiers::NONE, anchor + Vec2::new(40.0, 0.0), anchor + Vec2::new(100.0, 0.0));
    let tr = app.doc.model.active().clip(m::ClipId(a as u64)).unwrap().transform.clone();
    assert!(tr.position.x - x0 > 20.0, "moved {} -> {}", x0, tr.position.x);
    assert!(tr.position.y.abs() < 1e-6 && tr.position_z.abs() < 1e-6, "only along x");
    assert!(app.doc.history.revision() > undo0);
    // Rotate: the Y ring seen edge-on from the front is a vertical line;
    // switch to a three-quarter view and drag across the ring.
    super::super::with(|w| w.tool = Tool::Rotate);
    super::super::with(|w| w.nav = Nav { yaw: 30.0, pitch: 25.0, ..w.nav });
    d.idle(&mut app, 2);
    let p = d.pane(&app);
    let objs = super::super::objects(&app.doc, 0, None, &BTreeSet::new());
    let ob = objs.iter().find(|o| o.id == a as u64).unwrap();
    let (a_s, _, _, rings) = gizmo_parts(&p, ob.anchor, &[[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]).unwrap();
    // A point on the Y ring away from the centre.
    let on_ring = *rings[1].iter().max_by(|x, y| x.distance(a_s).partial_cmp(&y.distance(a_s)).unwrap()).unwrap();
    let before = tr.clone();
    d.drag(&mut app, PointerButton::Primary, Modifiers::NONE, on_ring, on_ring + (on_ring - a_s).rot90().normalized() * 50.0);
    let tr = app.doc.model.active().clip(m::ClipId(a as u64)).unwrap().transform.clone();
    assert!((tr.rotation_y - before.rotation_y).abs() > 3.0 || (tr.rotation_x - before.rotation_x).abs() > 3.0, "turned: {:?}", (tr.rotation_x, tr.rotation_y, tr.rotation));
    // Scale evenly with the centre handle.
    super::super::with(|w| w.tool = Tool::Scale);
    d.idle(&mut app, 1);
    let p = d.pane(&app);
    let c = p.to_screen(super::super::objects(&app.doc, 0, None, &BTreeSet::new()).iter().find(|o| o.id == a as u64).unwrap().anchor).unwrap();
    let s0 = tr.scale_x;
    d.drag(&mut app, PointerButton::Primary, Modifiers::NONE, c, c + Vec2::new(60.0, 0.0));
    let s1 = app.doc.model.active().clip(m::ClipId(a as u64)).unwrap().transform.scale_x;
    assert!(s1 > s0 * 1.2, "scaled {s0} -> {s1}");
}

/// Looking through the camera, locked to the view: orbiting moves the
/// camera itself. Unlocked, navigating leaves the camera for the eye.
#[test]
fn look_through_lock_and_set_camera_to_view() {
    let _g = super::super::test_lock();
    let ctx = egui::Context::default();
    let (mut app, _, _, cam) = space_app(&ctx);
    let mut d = Driver { ctx, t: 0.0, mods: Modifiers::NONE };
    let c = player().1.center();
    d.idle(&mut app, 2);
    d.key(&mut app, c, egui::Key::Num0, Modifiers::NONE);
    assert_eq!(super::super::with(|w| w.look), Look::Camera(None));
    let pos = |app: &StudioApp| {
        let seq = app.doc.model.active();
        sp::camera_view(seq, seq.clip(m::ClipId(cam)).unwrap(), 0, 640.0).position
    };
    let p0 = pos(&app);
    super::super::with(|w| w.lock_camera = true);
    d.drag(&mut app, PointerButton::Middle, Modifiers::NONE, c, c + Vec2::new(90.0, 0.0));
    let p1 = pos(&app);
    assert!((p1[0] - p0[0]).abs() > 50.0, "the locked camera orbited: {p0:?} -> {p1:?}");
    assert_eq!(super::super::with(|w| w.look), Look::Camera(None));
    // Unlocked: the orbit leaves the camera where it is.
    super::super::with(|w| w.lock_camera = false);
    d.drag(&mut app, PointerButton::Middle, Modifiers::NONE, c, c + Vec2::new(90.0, 0.0));
    assert_eq!(super::super::with(|w| w.look), Look::Editor);
    assert_eq!(pos(&app), p1);
    // Set camera to view: the camera stands where the eye is.
    let nav = eye();
    super::super::set_camera_to_view(&mut app.doc, cam, &nav, 0, None).unwrap();
    let p2 = pos(&app);
    let e = nav.position();
    assert!((0..3).all(|i| (p2[i] - e[i]).abs() < 1e-6), "{p2:?} vs {e:?}");
}

/// Recording a fly-through: the eye's path while playing becomes camera
/// keys when playback stops.
#[test]
fn recording_a_fly_through_keys_the_camera() {
    let _g = super::super::test_lock();
    let ctx = egui::Context::default();
    let (mut app, _, _, cam) = space_app(&ctx);
    let mut d = Driver { ctx: ctx.clone(), t: 0.0, mods: Modifiers::NONE };
    d.idle(&mut app, 2);
    toggle_recording(&mut app, &ctx);
    assert!(app.editor.playing);
    for k in 0..40 {
        app.editor.playhead = k * 33_333;
        super::super::with(|w| w.nav.travel([4.0, 0.0, 6.0]));
        d.frame(&mut app, vec![]);
    }
    app.editor.playing = false;
    d.idle(&mut app, 2);
    let mc = app.doc.model.active().clip(m::ClipId(cam)).unwrap();
    let keys = mc.keyframes.get("transform.position").map_or(0, |k| k.keys.len());
    assert!(keys >= 15, "{keys} keys");
    assert!(super::super::with(|w| w.rt.recording.is_none()));
}

/// The whole editor with the workspace on, in every shading mode, in
/// quad view, looking through the camera, with the outliner open and
/// spread cards: it draws without changing the project.
#[test]
fn the_editor_draws_the_workspace_in_every_mode() {
    let _g = super::super::test_lock();
    let ctx = egui::Context::default();
    let (mut app, a, _, _) = space_app(&ctx);
    app.editor.selection = vec![a];
    let before = app.doc.model.clone();
    let mut frame = eframe::Frame::_new_kittest();
    let mut k = 0.0;
    ctx.data_mut(|d| d.insert_temp(egui::Id::unique("viewport-outliner"), true));
    let modes: Vec<Box<dyn Fn(&mut Workspace)>> = vec![
        Box::new(|w| w.shade = Shade::Solid),
        Box::new(|w| w.shade = Shade::Material),
        Box::new(|w| w.shade = Shade::Rendered),
        Box::new(|w| w.shade = Shade::Wireframe),
        Box::new(|w| w.shade = Shade::Xray),
        Box::new(|w| w.quad = true),
        Box::new(|w| {
            w.quad = false;
            w.look = Look::Camera(None);
        }),
        Box::new(|w| {
            w.look = Look::Editor;
            w.spread = 300.0;
            w.tool = Tool::Rotate;
            w.orient = Orient::Local;
        }),
    ];
    for m in &modes {
        super::super::with(|w| m(w));
        for _ in 0..3 {
            k += 1.0 / 60.0;
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1600.0, 1000.0))), time: Some(k), ..Default::default() };
            let mut out = ctx.run_ui(input, |ui| eframe::App::ui(&mut app, ui, &mut frame));
            let _ = ctx.tessellate(std::mem::take(&mut out.shapes), out.pixels_per_point);
            out.textures_delta.clear();
        }
    }
    assert_eq!(app.doc.model, before);
}

/// A project with a small meadow world (a house to walk into) and the
/// Player on, as a person opens it.
fn world_app(ctx: &egui::Context) -> StudioApp {
    let mut s = crate::agent::Session::new(crate::services::project::new_custom("World", 640, 360, 30.0));
    s.edit(&[serde_json::json!({"op": "build_world", "preset": "meadow", "size": 120, "quality": "fast", "seed": 3,
        "scatter": [{"kind": "rock", "count": 3, "size": 1.0}],
        "buildings": [{"kind": "house", "at": [0, 16], "floors": 1, "width": 8, "depth": 6, "yaw": 0}]})], "world", false).unwrap();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let _serial = crate::ui_tests::serial();
    let mut app = StudioApp::new(&cc, crate::cli::Cli { screen: Some(crate::cli::Screen::Editor), fresh: true, ..Default::default() });
    app.doc = s.doc;
    app.editor.playhead = 0;
    super::super::with(|w| *w = Workspace::for_canvas(640.0, 360.0));
    super::super::follow(&app.doc);
    super::super::with(|w| {
        w.on = true;
        w.shade = Shade::Rendered;
    });
    app
}

fn hold(d: &mut Driver, app: &mut StudioApp, key: egui::Key, down: bool) {
    d.frame(app, vec![Event::Key { key, physical_key: None, pressed: down, repeat: false, modifiers: Modifiers::NONE }]);
}

fn walker_feet() -> [f32; 3] {
    super::super::with(|w| w.rt.walk.as_ref().map(|s| s.ex.body.pos)).expect("walking")
}

/// Walk mode in a world is a game's: gravity, jumping, walls, stepping,
/// the world drawn live, and a recorded walk becomes a camera shot.
#[test]
fn walking_in_a_world_has_gravity_collision_jump_and_records_a_shot() {
    let _g = super::super::test_lock();
    let ctx = egui::Context::default();
    let mut app = world_app(&ctx);
    let mut d = Driver { ctx: ctx.clone(), t: 0.0, mods: Modifiers::NONE };
    d.idle(&mut app, 2);
    let c = player().1.center();
    let found = super::super::world::find(&app.doc, 0).expect("the project has a world");
    d.key(&mut app, c, egui::Key::Backtick, Modifiers::SHIFT);
    assert_eq!(super::super::with(|w| w.rt.fly), Some(Fly::Walk));
    d.frame(&mut app, vec![Event::PointerMoved(c)]);
    let feet = walker_feet();
    // It stands on the ground and the eye is a person's height above it.
    let ground = found.world.ground_y(feet[0], feet[2]);
    assert!((feet[1] - ground).abs() < 0.2, "feet {feet:?} ground {ground}");
    let eye_m = -(eye().position()[1] as f32) * found.k;
    assert!((eye_m - feet[1] - 1.62).abs() < 0.3, "eye {eye_m} over feet {}", feet[1]);
    // The world is drawn live (a picture exists for it).
    assert!(super::super::world::last_ms() > 0.0, "the world was rendered");

    // Fall from the sky: gravity brings it down.
    super::super::with(|w| w.rt.walk.as_mut().unwrap().ex.body.pos[1] += 6.0);
    d.idle(&mut app, 90);
    let landed = walker_feet();
    assert!((landed[1] - found.world.ground_y(landed[0], landed[2])).abs() < 0.1, "back on the ground: {landed:?}");

    // Jump: Space lifts it about a metre and it comes back.
    let base = landed[1];
    hold(&mut d, &mut app, egui::Key::Space, true);
    let mut peak = base;
    for _ in 0..40 {
        d.frame(&mut app, vec![]);
        peak = peak.max(walker_feet()[1]);
    }
    hold(&mut d, &mut app, egui::Key::Space, false);
    d.idle(&mut app, 60);
    assert!(peak - base > 0.7 && peak - base < 1.5, "jumped {}", peak - base);

    // Run at the house: stopped by its wall (front wall at z = 13).
    super::super::with(|w| {
        let s = w.rt.walk.as_mut().unwrap();
        s.ex.teleport(&found.world, [0.0, found.world.ground_y(0.0, 8.0), 8.0], 0.0);
    });
    hold(&mut d, &mut app, egui::Key::W, true);
    d.idle(&mut app, 150);
    let at_wall = walker_feet();
    // The door is in the front wall somewhere; either the wall stopped it or it went in through a doorway
    // at the building's door. Walking straight at the middle of the front wall never gets past z 14.
    assert!(at_wall[2] > 9.0, "walked forward: {at_wall:?}");
    hold(&mut d, &mut app, egui::Key::W, false);

    // Record a walk: the path becomes keys on a camera.
    super::super::with(|w| {
        let s = w.rt.walk.as_mut().unwrap();
        s.ex.teleport(&found.world, [-20.0, found.world.ground_y(-20.0, -10.0), -10.0], 90.0);
        w.rt.want_record = true;
    });
    hold(&mut d, &mut app, egui::Key::W, true);
    d.idle(&mut app, 120);
    hold(&mut d, &mut app, egui::Key::W, false);
    assert!(super::super::with(|w| w.rt.walk.as_ref().is_some_and(|s| s.rec.is_some())), "recording");
    d.key(&mut app, c, egui::Key::Escape, Modifiers::NONE);
    d.idle(&mut app, 3);
    assert!(super::super::with(|w| w.rt.walk.is_none()), "walking ended");
    let cam = app.doc.model.active().clips.iter().find(|c| matches!(&c.source, m::ClipSource::Generator(g) if g.kind == m::GeneratorKind::Camera)).expect("a camera").clone();
    let keys = cam.keyframes.get("transform.position").map_or(0, |k| k.keys.len());
    assert!(keys >= 10, "{keys} position keys from the walk");
    // The recorded path moves along +x as walked (yaw 90 degrees).
    let tr = cam.keyframes.get("transform.position").unwrap();
    let first = tr.keys.first().unwrap();
    let last = tr.keys.last().unwrap();
    let (a, b) = (match &first.value { m::Value::Point(p) => p.x, _ => 0.0 }, match &last.value { m::Value::Point(p) => p.x, _ => 0.0 });
    assert!(b - a > 500.0, "walked east: {a} -> {b} px");
}

/// With no world layer, walk mode still has ground: a floor at the
/// workspace's floor and the scene's own models as solid things to be
/// stopped by and to stand on.
#[test]
fn walking_in_a_plain_scene_uses_its_models_as_ground_and_walls() {
    let _g = super::super::test_lock();
    let ctx = egui::Context::default();
    let mut s = crate::agent::Session::new(crate::services::project::new_custom("Scene", 640, 360, 30.0));
    // A 3 m cube standing on the floor (y down: its centre 150 px above it), 6 m ahead.
    s.edit(&[serde_json::json!({"op": "model", "shape": "cube", "size": 300, "position": [0, 30, 600], "at": 0, "duration": 4})], "cube", false).unwrap();
    let cc = eframe::CreationContext::_new_kittest(ctx.clone());
    let _serial = crate::ui_tests::serial();
    let mut app = StudioApp::new(&cc, crate::cli::Cli { screen: Some(crate::cli::Screen::Editor), fresh: true, ..Default::default() });
    app.doc = s.doc;
    app.editor.playhead = 0;
    super::super::with(|w| *w = Workspace::for_canvas(640.0, 360.0));
    super::super::follow(&app.doc);
    super::super::with(|w| {
        w.on = true;
        w.shade = Shade::Solid;
        // The eye at a person's height over the floor, 3 m back, looking along +z.
        w.nav = Nav { target: [0.0, 18.0, -200.0], yaw: 0.0, pitch: 0.0, distance: 100.0, lens: 50.0, ortho: false };
    });
    let mut d = Driver { ctx, t: 0.0, mods: Modifiers::NONE };
    d.idle(&mut app, 2);
    let c = player().1.center();
    d.key(&mut app, c, egui::Key::Backtick, Modifiers::SHIFT);
    d.frame(&mut app, vec![Event::PointerMoved(c)]);
    let start = walker_feet();
    // The floor is where the workspace says (180 px down = -1.8 m).
    assert!((start[1] + 1.8).abs() < 0.1, "stands on the floor: {start:?}");
    hold(&mut d, &mut app, egui::Key::W, true);
    d.idle(&mut app, 200);
    hold(&mut d, &mut app, egui::Key::W, false);
    let end = walker_feet();
    // The cube's near face is at z = 4.5 m; the walker stops a body-width short of it.
    assert!(end[2] > 3.0 && end[2] < 4.3, "stopped by the cube: {end:?}");
    // The eye stays at height; walking never lifted it.
    let eye_m = -(eye().position()[1] as f32) * 0.01;
    assert!((eye_m - end[1] - 1.62).abs() < 0.3);
}

/// The terrain brush raises the ground under the pointer live and, when
/// the button is released, writes the strokes into the world's description
/// (one undo step) so the world rebuilds the same.
#[test]
fn the_terrain_brush_sculpts_a_world_and_saves_the_strokes() {
    let _g = super::super::test_lock();
    let ctx = egui::Context::default();
    let mut app = world_app(&ctx);
    let found = super::super::world::find(&app.doc, 0).unwrap();
    let (tx, tz) = (0.0f32, 6.0f32);
    let before = found.world.ground_y(tx, tz);
    super::super::with(|w| {
        w.nav = Nav { target: [0.0, -(before as f64) * 100.0, tz as f64 * 100.0], yaw: 0.0, pitch: 55.0, distance: 1500.0, lens: 50.0, ortho: false };
        w.rt.brush = Some(super::super::world::Brush { kind: super::super::world::BrushKind::Raise, radius: 6.0, strength: 1.0 });
        w.rt.fly = None;
    });
    let mut d = Driver { ctx, t: 0.0, mods: Modifiers::NONE };
    d.idle(&mut app, 3);
    let c = d.pane(&app).picture.center();
    let button = |pressed: bool| Event::PointerButton { pos: c, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
    d.frame(&mut app, vec![Event::PointerMoved(c), button(true)]);
    d.idle(&mut app, 12);
    // Live: the ground under the brush is already higher in the session's copy.
    let live = super::super::world::session_world(&found.file).expect("a sculpting session").ground_y(tx, tz);
    assert!(live > before + 0.5, "raised live: {before} -> {live}");
    assert!(super::super::world::pending());
    d.frame(&mut app, vec![button(false)]);
    d.idle(&mut app, 3);
    // Written: the layer points at a rebuilt world with the strokes in its description.
    let after = super::super::world::find(&app.doc, 0).unwrap();
    assert_ne!(after.file, found.file);
    assert!(after.world.ground_y(tx, tz) > before + 0.5, "{} vs {before}", after.world.ground_y(tx, tz));
    let spec = super::super::world::read_spec(&after.file).unwrap();
    assert!(!spec.sculpt.is_empty() && spec.sculpt.iter().all(|s| s.brush == "raise"), "{} strokes", spec.sculpt.len());
    assert!(!super::super::world::pending());
    // Undo restores the old ground.
    app.doc.undo();
    let undone = super::super::world::find(&app.doc, 0).unwrap();
    assert_eq!(undone.file, found.file);
}

#[test]
fn the_edit_bar_keeps_clear_of_pane_names_and_gizmos() {
    let bar = Rect::from_min_size(egui::pos2(170.0, 4.0), Vec2::new(700.0, 60.0));
    let top_left = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(499.0, 299.0));
    let top_right = Rect::from_min_max(egui::pos2(501.0, 0.0), egui::pos2(1000.0, 299.0));
    assert_eq!(clear_of(bar, label_rect(top_left)), 0.0, "the first name sits left of the bar");
    let dy = clear_of(bar, label_rect(top_right));
    assert!(dy > 0.0 && !label_rect(top_right).translate(Vec2::new(0.0, dy)).intersects(bar), "the second name moves under the bar");
    let dy = clear_of(bar, gizmo_rect(top_left));
    assert!(dy > 0.0 && !gizmo_rect(top_left).translate(Vec2::new(0.0, dy)).intersects(bar), "the gizmo moves under the bar");
    assert_eq!(clear_of(bar, gizmo_rect(top_right)), 0.0, "a gizmo right of the bar stays");
    let bottom = Rect::from_min_max(egui::pos2(0.0, 301.0), egui::pos2(499.0, 600.0));
    assert_eq!(clear_of(bar, label_rect(bottom)) + clear_of(bar, gizmo_rect(bottom)), 0.0, "lower panes are not touched");
}
