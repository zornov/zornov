// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The 3D workspace in the Player: the picture rendered from the editor's
//! eye (or through a camera), the floor grid, camera frustums, lights and
//! outlines drawn over it, the navigation (mouse, trackpad, keys, the
//! view gizmo), picking, the move/rotate/scale gizmo, the header with the
//! shading modes and view tools, the outliner and the quad view.
//!
//! Navigation, as in 3D applications:
//! - middle drag or Alt-drag orbits, Shift with it pans, scroll zooms to
//!   the pointer; on a trackpad two fingers orbit, Shift-two-fingers pan,
//!   a pinch (or Cmd-two-fingers) zooms;
//! - right drag looks round, with W A S D to fly and Q E down and up
//!   (Shift faster); `~` toggles fly mode, Shift-`~` walk mode;
//! - 1, 3, 7 front, right and top (Cmd for back, left, bottom), 5
//!   perspective/orthographic, 0 through the camera, `.` frames the
//!   selection, Home frames everything;
//! - G, R, S and Q pick move, rotate, scale and select; Cmd while
//!   dragging a handle flips snapping; a right click places the 3D cursor.
//!
//! The picture is rendered at a fraction of full size while the view
//! moves and refined to full size a moment after it stops, so navigation
//! keeps the display's rate on heavy scenes.

use super::nav::{Axis, Nav};
use super::{Fly, Kind, Look, Object, Orient, Pivot, Shade, Tool, Workspace};
use crate::app::StudioApp;
use crate::icons::Icon;
use crate::kit::{self, IconButton, MenuItem};
use crate::theme;
use crate::view::Time;
use eframe::egui::{self, Align2, Color32, Pos2, Rect, Sense, Stroke, Vec2};
use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use studio_model as m;
use studio_model::space as sp;
use crate::kit::ShowNoted;

const HEADER_H: f32 = 30.0;
const AXIS_COLOURS: [Color32; 3] = [Color32::from_rgb(239, 83, 80), Color32::from_rgb(130, 200, 90), Color32::from_rgb(66, 150, 245)];
const SELECTED: Color32 = Color32::from_rgb(255, 160, 40);
const CAMERA_COLOUR: Color32 = Color32::from_rgb(200, 200, 210);
const LIGHT_COLOUR: Color32 = Color32::from_rgb(255, 220, 120);
/// Seconds after the view stops moving before the picture refines.
const SETTLE: f64 = 0.18;

thread_local! {
    /// Renderers of the quad view's other panes.
    static PANES: RefCell<Vec<super::super::render_view::RenderView>> = const { RefCell::new(Vec::new()) };
    /// The panes as last drawn (edit mode works in them between frames).
    static LAST_PANES: RefCell<Vec<Pane>> = const { RefCell::new(Vec::new()) };
}

/// The panes as the workspace last drew them.
pub fn last_panes() -> Vec<Pane> {
    LAST_PANES.with(|p| p.borrow().clone())
}

/// Glides the editor's eye to look along an axis (the numpad views).
pub fn view_axis(app: &mut StudioApp, ctx: &egui::Context, a: Axis) {
    let t = now(ctx);
    super::with(|ws| {
        leave_camera(ws, &app.doc, time_of(app));
        let mut n = *ws.nav_mut();
        n.set_axis(a);
        ws.glide_to(n, t);
    });
}

/// A view on screen: where its picture goes and how the world maps there.
#[derive(Clone, Copy, Debug)]
pub struct Pane {
    /// 0, 1, 2: top, front, right of the quad view; 3: the eye (or camera).
    pub index: usize,
    pub rect: Rect,
    /// The picture's rectangle (the canvas aspect, covering `rect`).
    pub picture: Rect,
    /// Screen points per canvas pixel.
    pub scale: f32,
    pub view: sp::View,
    pub nav: Nav,
    /// Looking through this camera clip.
    pub camera: Option<u64>,
}

impl Pane {
    pub fn to_screen(&self, p: [f64; 3]) -> Option<Pos2> {
        self.view.project(p).map(|(c, _)| self.picture.center() + Vec2::new(c.x as f32, c.y as f32) * self.scale)
    }

    /// Canvas pixels from the view centre of a screen point.
    pub fn to_canvas(&self, s: Pos2) -> [f64; 2] {
        let d = (s - self.picture.center()) / self.scale;
        [d.x as f64, d.y as f64]
    }

    /// A segment clipped to in front of the eye, on screen.
    pub(super) fn segment(&self, a: [f64; 3], b: [f64; 3]) -> Option<[Pos2; 2]> {
        let near = 2.0;
        let (va, vb) = (self.view.view(a), self.view.view(b));
        if va[2] < near && vb[2] < near {
            return None;
        }
        let cut = |p: [f64; 3], q: [f64; 3], vp: [f64; 3], vq: [f64; 3]| {
            let t = (near - vp[2]) / (vq[2] - vp[2]);
            [p[0] + (q[0] - p[0]) * t, p[1] + (q[1] - p[1]) * t, p[2] + (q[2] - p[2]) * t]
        };
        let a2 = if va[2] < near { cut(a, b, va, vb) } else { a };
        let b2 = if vb[2] < near { cut(b, a, vb, va) } else { b };
        Some([self.to_screen(a2)?, self.to_screen(b2)?])
    }
}

/// The picture rectangle of the canvas aspect that covers `r`.
fn cover(r: Rect, aspect: f32) -> Rect {
    let (w, h) = if r.width() / r.height().max(1.0) > aspect { (r.width(), r.width() / aspect) } else { (r.height() * aspect, r.height()) };
    Rect::from_center_size(r.center(), Vec2::new(w, h))
}

fn now(ctx: &egui::Context) -> f64 {
    ctx.input(|i| i.time)
}

/// The time the Player shows.
fn time_of(app: &StudioApp) -> Time {
    app.editor.hover_time.unwrap_or(app.editor.playhead)
}

/// The 3D toggle on the Player's toolbar (shown when the project has
/// anything in space, or the workspace is on).
pub fn toolbar_button(app: &mut StudioApp, ui: &mut egui::Ui) {
    super::follow(&app.doc);
    let on = super::with(|w| w.on);
    if !on && !super::has_space(&app.doc) {
        return;
    }
    if IconButton::new(Icon::Shapes, "3D workspace").shortcut("⇧Tab").active(on).show(ui).clicked() {
        set_on(app, ui.ctx(), !on);
    }
}

/// Turns the workspace on or off.
pub fn set_on(app: &mut StudioApp, ctx: &egui::Context, on: bool) {
    super::follow(&app.doc);
    super::with(|w| {
        w.on = on;
        w.rt.fly = None;
        w.changed();
    });
    if !on && let Some(rv) = app.render.as_mut() {
        rv.set_view(None);
    }
    ctx.request_repaint();
}

/// Keys of the workspace, before the editor's own shortcuts: only while
/// the pointer is over the Player and the workspace is on.
pub fn keys(app: &mut StudioApp, ctx: &egui::Context) {
    use egui::{Key, Modifiers as M};
    if ctx.egui_wants_keyboard_input() {
        return;
    }
    super::follow(&app.doc);
    let over = ctx.input(|i| i.pointer.hover_pos()).is_some_and(|p| app.editor.viewer_rect.contains(p));
    let pressed = |m: M, k: Key| ctx.input_mut(|i| i.consume_shortcut(&egui::KeyboardShortcut::new(m, k)));
    if over && pressed(M::SHIFT, Key::Tab) {
        let on = super::with(|w| w.on);
        if on || super::has_space(&app.doc) {
            set_on(app, ctx, !on);
        }
        return;
    }
    let (on, flying) = super::with(|w| (w.on, w.rt.fly.is_some()));
    // A transform under way keeps the keys wherever the pointer is.
    let editing_busy = crate::editor::model_edit::with(|s| s.as_ref().is_some_and(|s| s.modal.is_some()));
    if on && (over || editing_busy) && !flying && crate::editor::model_edit::input::keys(app, ctx) {
        return;
    }
    if !on || !(over || flying) {
        return;
    }
    let t = now(ctx);
    let w = app.doc.project.width as f64;
    let aspect = app.doc.project.width as f64 / app.doc.project.height.max(1) as f64;
    // Fly / walk mode.
    if pressed(M::SHIFT, Key::Backtick) {
        super::with(|ws| ws.rt.fly = if ws.rt.fly == Some(Fly::Walk) { None } else { Some(Fly::Walk) });
        return;
    }
    if pressed(M::NONE, Key::Backtick) {
        super::with(|ws| ws.rt.fly = if ws.rt.fly == Some(Fly::Fly) { None } else { Some(Fly::Fly) });
        return;
    }
    if flying && pressed(M::NONE, Key::Escape) {
        super::with(|ws| ws.rt.fly = None);
        return;
    }
    let axis = |a: Axis| {
        super::with(|ws| {
            leave_camera(ws, &app.doc, time_of(app));
            let mut n = *ws.nav_mut();
            n.set_axis(a);
            ws.glide_to(n, t);
        })
    };
    for (k, a, b) in [(Key::Num1, Axis::Front, Axis::Back), (Key::Num3, Axis::Right, Axis::Left), (Key::Num7, Axis::Top, Axis::Bottom)] {
        if pressed(M::NONE, k) {
            axis(a);
        }
        if pressed(M::COMMAND, k) {
            axis(b);
        }
    }
    if pressed(M::NONE, Key::Num5) {
        super::with(|ws| {
            leave_camera(ws, &app.doc, time_of(app));
            let n = ws.nav_mut();
            n.ortho = !n.ortho;
            ws.rt.moved_at = t;
            ws.changed();
        });
    }
    if pressed(M::NONE, Key::Num0) {
        super::with(|ws| {
            ws.look = if matches!(ws.look, Look::Camera(_)) { Look::Editor } else { Look::Camera(None) };
            ws.changed();
        });
    }
    let sel: Vec<u64> = app.editor.selection.iter().map(|i| *i as u64).collect();
    if pressed(M::NONE, Key::Period) {
        frame(app, &sel, t, aspect);
    }
    if pressed(M::NONE, Key::Home) {
        frame(app, &[], t, aspect);
    }
    for (k, tool) in [(Key::G, Tool::Move), (Key::R, Tool::Rotate), (Key::S, Tool::Scale), (Key::Q, Tool::Select)] {
        if !flying && pressed(M::NONE, k) {
            super::with(|ws| {
                ws.tool = tool;
                ws.changed();
            });
        }
    }
    let _ = w;
}

/// Leaves a camera view for the editor's eye at the camera's place (when
/// not locked to it), as navigating out of a camera does in 3D apps.
fn leave_camera(ws: &mut Workspace, doc: &crate::doc::Doc, t: Time) {
    if matches!(ws.look, Look::Camera(_)) && !ws.lock_camera {
        if let Some(id) = super::look_camera(doc, ws.look, t)
            && let Some(n) = super::camera_nav(doc, id, t)
        {
            ws.nav = n;
        }
        ws.look = Look::Editor;
        ws.changed();
    }
}

/// Frames some objects (all when `ids` is empty) with a smooth move.
pub fn frame(app: &mut StudioApp, ids: &[u64], t: f64, aspect: f64) {
    let time = time_of(app);
    let depths = depths(app, time);
    let ws = super::snapshot();
    let objs = super::objects(&app.doc, time, ws.cards.then_some(&depths), &ws.hidden);
    let Some((c, r)) = super::extent(&objs, ids).or_else(|| super::extent(&objs, &[])) else { return };
    let w = app.doc.project.width as f64;
    super::with(|ws| {
        leave_camera(ws, &app.doc, time);
        let mut n = *ws.nav_mut();
        n.frame(c, r, w, aspect);
        ws.glide_to(n, t);
    });
}

/// Card depths of the 2D layers now.
fn depths(app: &StudioApp, t: Time) -> HashMap<u64, f32> {
    let (spread, hidden) = super::with(|w| (w.spread as f32, w.hidden.clone()));
    let Some(scene) = app.render.as_ref().and_then(|r| r.scene()) else { return HashMap::new() };
    let mut o = studio_render::viewport::ViewOptions::new(studio_render::space::CameraDesc::default_for(&studio_render::scene::Scene::canvas(scene.as_ref())));
    o.spread = spread;
    o.hidden = hidden.into_iter().collect();
    studio_render::viewport::ViewScene::new(scene, o).card_depths(t)
}

fn camera_desc(v: &sp::View) -> studio_render::space::CameraDesc {
    studio_render::space::CameraDesc { position: v.position.map(|x| x as f32), axes: v.axes.map(|r| r.map(|x| x as f32)), zoom: v.zoom as f32, dof: None, trace: None }
}

/// The renderer's options for a pane.
fn options(ws: &Workspace, pane: &Pane) -> studio_render::viewport::ViewOptions {
    let mut o = studio_render::viewport::ViewOptions::new(camera_desc(&pane.view));
    let through_camera = pane.camera.is_some();
    o.shading = if through_camera && ws.shade != Shade::Wireframe && ws.shade != Shade::Xray { Shade::Rendered.render() } else { ws.shade.render() };
    o.cards = ws.cards && !through_camera;
    o.spread = ws.spread as f32;
    o.hidden = ws.hidden.iter().copied().collect();
    // The grid shows through where nothing is (not through a camera:
    // that is the final picture).
    o.transparent = !through_camera && ws.grid;
    if through_camera {
        o.cards = false;
    }
    o
}

/// The Player as the 3D workspace. Returns false (nothing drawn) while
/// the workspace is off.
pub fn show(app: &mut StudioApp, ui: &mut egui::Ui, r: Rect, area: Rect) -> bool {
    super::follow(&app.doc);
    if !super::with(|w| w.on) {
        return false;
    }
    let ctx = ui.ctx().clone();
    let t_now = now(&ctx);
    let time = time_of(app);
    walk_sync(app, &ctx);
    if super::with(|w| w.step_glide(t_now)) {
        ctx.request_repaint();
    }
    // A selection made by the AI.
    if let Some(sel) = super::with(|w| w.rt.select.take()) {
        app.editor.selection = sel;
    }
    record_tick(app, &ctx);
    crate::editor::model_edit::follow(app);
    let ws = super::snapshot();
    let th = theme::tokens(&ctx);
    let body = body_of(r, area);
    let painter = ui.painter().with_clip_rect(body);
    painter.rect_filled(body, 0, if th.dark { Color32::from_rgb(38, 39, 42) } else { Color32::from_rgb(170, 172, 176) });
    let depth_map = depths(app, time);
    let objs = super::objects(&app.doc, time, ws.cards.then_some(&depth_map), &ws.hidden);
    let panes = panes_of(app, &ws, body, time);
    LAST_PANES.with(|p| *p.borrow_mut() = panes.clone());
    for pane in &panes {
        draw_pane(app, ui, pane, &ws, &objs, time, t_now);
    }
    // Input: the pane under the pointer (or the one being dragged).
    let hover = ctx.input(|i| i.pointer.hover_pos());
    let held = ctx.data(|d| d.get_temp::<usize>(egui::Id::unique("viewport-held-pane")));
    let target = held.and_then(|h| panes.iter().find(|p| p.index == h)).or_else(|| hover.and_then(|h| panes.iter().find(|p| p.rect.contains(h)))).copied();
    if let Some(pane) = target {
        interact(app, ui, &pane, &objs, time, t_now);
    }
    crate::editor::model_edit::input::frame(app, ui, &panes);
    header(app, ui, Rect::from_min_size(egui::pos2(r.left() + 8.0, body.top() - HEADER_H + 2.0), Vec2::new(r.width() - 16.0, HEADER_H - 4.0)), &panes);
    crate::editor::model_edit::menus::bar(app, ui, body);
    crate::editor::model_edit::menus::popups(app, ui, body);
    crate::editor::model_edit::menus::last_panel(app, ui, body);
    outliner(app, ui, body, &objs);
    crate::editor::model_edit::input::end_frame(&ctx);
    super::save(&app.doc);
    if super::save_waiting() {
        ctx.request_repaint_after(std::time::Duration::from_millis(500));
    }
    true
}

/// The workspace's area of the Player: under its header, the whole
/// picture area.
pub fn body_of(r: Rect, area: Rect) -> Rect {
    Rect::from_min_max(egui::pos2(r.left() + 1.0, area.top() - 9.0 + HEADER_H), egui::pos2(r.right() - 1.0, area.bottom() + 9.0))
}

/// The views on screen: one, or four in quad view.
pub fn panes_of(app: &StudioApp, ws: &Workspace, body: Rect, time: Time) -> Vec<Pane> {
    let rects: Vec<(usize, Rect)> = if ws.quad {
        let c = body.center();
        vec![(0, Rect::from_min_max(body.min, c - Vec2::splat(1.0))), (3, Rect::from_min_max(egui::pos2(c.x + 1.0, body.top()), egui::pos2(body.right(), c.y - 1.0))), (1, Rect::from_min_max(egui::pos2(body.left(), c.y + 1.0), egui::pos2(c.x - 1.0, body.bottom()))), (2, Rect::from_min_max(c + Vec2::splat(1.0), body.max))]
    } else {
        vec![(3, body)]
    };
    let w = app.doc.project.width as f64;
    let aspect = app.doc.project.width as f32 / app.doc.project.height.max(1) as f32;
    let mut panes = Vec::new();
    for (index, rect) in rects {
        let camera = (index == 3).then(|| super::look_camera(&app.doc, ws.look, time)).flatten().filter(|_| matches!(ws.look, Look::Camera(_)));
        let (nav, view) = match camera {
            Some(id) => {
                let seq = app.doc.model.active();
                let v = seq.clip(m::ClipId(id)).map(|mc| sp::camera_view(seq, mc, time, w)).unwrap_or_else(|| sp::View::default_for(w));
                (super::camera_nav(&app.doc, id, time).unwrap_or_default(), v)
            }
            None => {
                let n = if index < 3 { ws.panes[index] } else { ws.nav };
                (n, n.view(w))
            }
        };
        // Through a camera the whole frame shows (letterboxed); the
        // editor's eye fills the pane.
        let picture = if camera.is_some() {
            let fit = (rect.width() / aspect).min(rect.height());
            Rect::from_center_size(rect.center(), Vec2::new(fit * aspect, fit))
        } else {
            cover(rect, aspect)
        };
        let scale = picture.width() / w as f32;
        panes.push(Pane { index, rect, picture, scale, view, nav, camera });
    }
    panes
}

fn pane_name(ws: &Workspace, pane: &Pane, doc: &crate::doc::Doc) -> String {
    if let Some(id) = pane.camera {
        let name = doc.model.active().clip(m::ClipId(id)).map(|c| c.name.clone()).unwrap_or_default();
        return format!("Camera · {name}{}", if ws.lock_camera { " (locked to view)" } else { "" });
    }
    let n = pane.nav;
    let named = Axis::ALL.into_iter().find(|a| {
        let (y, p) = a.angles();
        (super::nav::wrap(n.yaw - y)).abs() < 0.01 && (n.pitch - p).abs() < 0.01
    });
    let proj = if n.ortho { "Orthographic" } else { "Perspective" };
    match named {
        Some(a) => format!("{}{} {proj}", a.name()[..1].to_uppercase(), &a.name()[1..]),
        None => format!("User {proj}"),
    }
}

/// Draws one pane: background and grid, the picture, then the overlays.
fn draw_pane(app: &mut StudioApp, ui: &mut egui::Ui, pane: &Pane, ws: &Workspace, objs: &[Object], time: Time, t_now: f64) {
    let ctx = ui.ctx().clone();
    let p = ui.painter().with_clip_rect(pane.rect);
    let w = app.doc.project.width as f64;
    let h = app.doc.project.height as f64;
    let through = pane.camera.is_some();
    if through {
        p.rect_filled(pane.rect, 0, Color32::from_rgb(20, 20, 22));
    }
    if ws.grid && !through {
        grid(&p, pane, ws.floor_y(h), w);
    }
    // The picture.
    if ws.shade != Shade::Wireframe {
        let world = if pane.index == 3 && pane.camera.is_none() { super::world::find(&app.doc, time) } else { None };
        let in_world = ws.rt.walk.is_some() && world.is_some();
        let moving = t_now - ws.rt.moved_at < SETTLE || ws.rt.glide.is_some() || (ws.rt.fly.is_some() && !in_world) || ctx.input(|i| i.pointer.any_down() && i.pointer.is_moving());
        if t_now - ws.rt.moved_at < SETTLE + 0.05 {
            ctx.request_repaint();
        }
        let ppp = ctx.pixels_per_point();
        let q = [1.0, 0.5, 0.25][app.editor.quality.min(2)] * if moving { 0.35 } else { 1.0 };
        let shown = pane.picture.intersect(pane.rect);
        let px = ((pane.picture.width() * ppp * q).round().max(16.0) as u32, (pane.picture.height() * ppp * q).round().max(16.0) as u32);
        let opts = options(ws, pane);
        let fps = (app.doc.project.fps as f64).max(1.0);
        let at = ((time as f64 * fps / 1e6).floor() * 1e6 / fps) as i64;
        let doc = &app.doc;
        let draw = |rv: &mut super::super::render_view::RenderView| {
            rv.sync(doc, at);
            rv.set_view(Some(opts.clone()));
            rv.frame(&ctx, at, px, moving, false)
        };
        // A walkable world is drawn live, straight from the editor's eye.
        let tex = if let Some(mut found) = world {
            // The ground being sculpted (before the strokes are written).
            if let Some(w) = super::world::session_world(&found.file) {
                found.world = w;
            }
            let st = ws.rt.walk.as_deref();
            let desc = camera_desc(&pane.view);
            super::world::draw(&ctx, &found, st, &desc, (app.doc.project.width, app.doc.project.height), pane.picture.size(), app.editor.quality, moving, app.editor.playing, time, t_now)
        } else if pane.index == 3 {
            draw(app.render.get_or_insert_with(super::super::render_view::RenderView::new))
        } else {
            PANES.with(|cell| {
                let mut v = cell.borrow_mut();
                while v.len() < 3 {
                    v.push(super::super::render_view::RenderView::new());
                }
                draw(&mut v[pane.index])
            })
        };
        if let Some(tex) = tex {
            let tint = if ws.shade == Shade::Xray && !through { Color32::from_white_alpha(115) } else { Color32::WHITE };
            let _ = shown;
            p.image(tex, pane.picture, Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), tint);
        }
    }
    // The brush ring on the ground.
    if ws.rt.brush.is_some()
        && pane.index == 3
        && pane.camera.is_none()
        && let Some(found) = super::world::find(&app.doc, time)
    {
        let pts: Vec<Pos2> = super::world::ring(&found).iter().filter_map(|p| pane.to_screen(*p)).collect();
        if pts.len() > 2 {
            p.add(egui::Shape::line(pts, Stroke::new(2.0, Color32::from_rgb(255, 200, 60))));
        }
    }
    if through {
        // The frame's edge and the area outside it.
        p.rect_stroke(pane.picture, 0, Stroke::new(1.0, Color32::from_white_alpha(90)), egui::StrokeKind::Outside);
    }
    let sel: BTreeSet<u64> = app.editor.selection.iter().map(|i| *i as u64).collect();
    if ws.overlays {
        overlays(&p, pane, ws, objs, &sel, w, h);
    }
    if ws.colliders {
        super::colliders::draw(app, &p, pane, objs, time);
    }
    crate::editor::model_edit::draw::overlay(app, &p, pane, ws);
    if !through {
        view_gizmo_draw(&p, pane);
    }
    let name = pane_name(ws, pane, &app.doc);
    let th = theme::tokens(&ctx);
    p.text(pane.rect.left_top() + Vec2::new(10.0, 8.0), Align2::LEFT_TOP, name, theme::caption(), th.text);
    if let Some(rec) = &ws.rt.recording {
        p.circle_filled(pane.rect.left_top() + Vec2::new(16.0, 34.0), 5.0, Color32::from_rgb(230, 60, 60));
        p.text(pane.rect.left_top() + Vec2::new(26.0, 34.0), Align2::LEFT_CENTER, format!("Recording camera path · {} samples", rec.samples.len()), theme::caption(), th.text);
    }
    if let Some(f) = ws.rt.fly {
        let label = match f {
            Fly::Fly => "Fly: look with the mouse · W A S D Q E · Shift faster · scroll speed · Esc stops",
            Fly::Walk if ws.rt.walk.is_some() => "Walk: mouse looks · W A S D move · Shift sprint · Space jump · V third person · Esc stops",
            Fly::Walk => "Walk: look with the mouse · W A S D · Q E up and down · Esc stops",
        };
        p.text(pane.rect.center_bottom() - Vec2::new(0.0, 14.0), Align2::CENTER_BOTTOM, label, theme::caption(), th.text);
    }
    let _ = time;
}

/// The floor grid with lines every power of ten that suits the distance,
/// the x axis red and the z axis blue.
fn grid(p: &egui::Painter, pane: &Pane, floor: f64, width: f64) {
    let n = pane.nav;
    // Straight down or up from the top/bottom view, the grid lies in x/z;
    // from the front or side it would be edge-on: lie in the view's plane.
    let edge_on = n.ortho && n.pitch.abs() < 1.0;
    let step0 = {
        let wpp = n.world_per_pixel(width) / pane.scale as f64;
        let raw = wpp * 40.0;
        10f64.powf(raw.max(1.0).log10().ceil())
    };
    let centre = n.target;
    let count = 40;
    let col = |major: bool| if major { Color32::from_white_alpha(40) } else { Color32::from_white_alpha(18) };
    for k in -count..=count {
        let major = k % 10 == 0;
        let o = k as f64 * step0;
        if edge_on {
            // A grid in the plane facing the view, through the target.
            let r = n.right();
            let d = n.down();
            let base = centre;
            let ext = count as f64 * step0;
            let snap = |v: f64| (v / step0).round() * step0;
            let bx = [snap(base[0]), snap(base[1]), snap(base[2])];
            let a = [bx[0] + r[0] * o - d[0] * ext, bx[1] + r[1] * o - d[1] * ext, bx[2] + r[2] * o - d[2] * ext];
            let b = [bx[0] + r[0] * o + d[0] * ext, bx[1] + r[1] * o + d[1] * ext, bx[2] + r[2] * o + d[2] * ext];
            if let Some(s) = pane.segment(a, b) {
                p.line_segment(s, Stroke::new(1.0, col(major)));
            }
            let a = [bx[0] + d[0] * o - r[0] * ext, bx[1] + d[1] * o - r[1] * ext, bx[2] + d[2] * o - r[2] * ext];
            let b = [bx[0] + d[0] * o + r[0] * ext, bx[1] + d[1] * o + r[1] * ext, bx[2] + d[2] * o + r[2] * ext];
            if let Some(s) = pane.segment(a, b) {
                p.line_segment(s, Stroke::new(1.0, col(major)));
            }
            continue;
        }
        let cx = (centre[0] / step0).round() * step0;
        let cz = (centre[2] / step0).round() * step0;
        let ext = count as f64 * step0;
        let x = cx + o;
        let z = cz + o;
        let xs = if x.abs() < step0 * 0.01 { Stroke::new(1.5, theme::alpha(AXIS_COLOURS[2], 0.8)) } else { Stroke::new(1.0, col(major)) };
        if let Some(s) = pane.segment([x, floor, cz - ext], [x, floor, cz + ext]) {
            p.line_segment(s, xs);
        }
        let zs = if z.abs() < step0 * 0.01 { Stroke::new(1.5, theme::alpha(AXIS_COLOURS[0], 0.8)) } else { Stroke::new(1.0, col(major)) };
        if let Some(s) = pane.segment([cx - ext, floor, z], [cx + ext, floor, z]) {
            p.line_segment(s, zs);
        }
    }
}

/// Camera frustums, lights, the canvas frame, outlines of what is
/// selected, the 3D cursor, and wireframes in wireframe/X-ray shading.
fn overlays(p: &egui::Painter, pane: &Pane, ws: &Workspace, objs: &[Object], sel: &BTreeSet<u64>, w: f64, h: f64) {
    let line = |a: [f64; 3], b: [f64; 3], s: Stroke| {
        if let Some(seg) = pane.segment(a, b) {
            p.line_segment(seg, s);
        }
    };
    if ws.frame && pane.camera.is_none() {
        // The canvas plane at its own size: where the default camera's
        // frame is.
        let c = [[-w / 2.0, -h / 2.0, 0.0], [w / 2.0, -h / 2.0, 0.0], [w / 2.0, h / 2.0, 0.0], [-w / 2.0, h / 2.0, 0.0]];
        for i in 0..4 {
            line(c[i], c[(i + 1) % 4], Stroke::new(1.0, Color32::from_white_alpha(60)));
        }
    }
    let wire = matches!(ws.shade, Shade::Wireframe | Shade::Xray);
    for o in objs {
        let picked = sel.contains(&o.id);
        let base = if picked { SELECTED } else { Color32::from_white_alpha(120) };
        match o.kind {
            Kind::Camera => {
                if pane.camera == Some(o.id) {
                    continue;
                }
                let Some(v) = o.view else { continue };
                let col = if picked { SELECTED } else { CAMERA_COLOUR };
                let k = 0.2;
                let corner = |cx: f64, cy: f64| {
                    let local = [cx * w * 0.5 * k, cy * h * 0.5 * k, v.zoom * k];
                    let d = sp::apply(&sp::transpose(&v.axes), local);
                    [v.position[0] + d[0], v.position[1] + d[1], v.position[2] + d[2]]
                };
                let cs = [corner(-1.0, -1.0), corner(1.0, -1.0), corner(1.0, 1.0), corner(-1.0, 1.0)];
                for i in 0..4 {
                    line(v.position, cs[i], Stroke::new(1.2, col));
                    line(cs[i], cs[(i + 1) % 4], Stroke::new(1.2, col));
                }
                // The up triangle over the frame's top edge.
                let up = [corner(-0.4, -1.15), corner(0.4, -1.15), corner(0.0, -1.5)];
                for i in 0..3 {
                    line(up[i], up[(i + 1) % 3], Stroke::new(1.2, col));
                }
                if let Some(a) = o.aim {
                    line(v.position, a, Stroke::new(1.0, theme::alpha(col, 0.4)));
                }
                if let Some(s) = pane.to_screen(v.position) {
                    p.text(s + Vec2::new(8.0, -8.0), Align2::LEFT_BOTTOM, &o.name, theme::caption(), col);
                }
            }
            Kind::Light => {
                let col = if picked { SELECTED } else { LIGHT_COLOUR };
                if let Some(s) = pane.to_screen(o.anchor) {
                    p.circle_stroke(s, 7.0, Stroke::new(1.5, col));
                    p.circle_filled(s, 3.0, col);
                    for k in 0..8 {
                        let a = k as f32 * std::f32::consts::TAU / 8.0;
                        let d = Vec2::angled(a);
                        p.line_segment([s + d * 10.0, s + d * 14.0], Stroke::new(1.2, col));
                    }
                }
                if let Some(a) = o.aim {
                    line(o.anchor, a, Stroke::new(1.0, theme::alpha(col, 0.6)));
                }
            }
            Kind::Layer | Kind::Card => {
                if let Some(c) = o.corners {
                    let s = if picked { Stroke::new(2.0, SELECTED) } else if wire || o.kind == Kind::Layer { Stroke::new(1.0, base) } else { Stroke::new(1.0, Color32::from_white_alpha(45)) };
                    for i in 0..4 {
                        line(c[i], c[(i + 1) % 4], s);
                    }
                }
            }
            Kind::Model => {
                if wire && let Some((file, mx)) = &o.model {
                    wireframe(p, pane, file, mx, if picked { SELECTED } else { Color32::from_white_alpha(150) });
                }
                if picked && crate::editor::model_edit::open_clip() != Some(o.id) {
                    let (lo, hi) = (o.min, o.max);
                    let cn = |i: usize| [if i & 1 == 0 { lo[0] } else { hi[0] }, if i & 2 == 0 { lo[1] } else { hi[1] }, if i & 4 == 0 { lo[2] } else { hi[2] }];
                    for (a, b) in [(0, 1), (1, 3), (3, 2), (2, 0), (4, 5), (5, 7), (7, 6), (6, 4), (0, 4), (1, 5), (2, 6), (3, 7)] {
                        line(cn(a), cn(b), Stroke::new(1.2, SELECTED));
                    }
                }
            }
        }
    }
    // The 3D cursor.
    if let Some(s) = pane.to_screen(ws.cursor) {
        p.circle_stroke(s, 7.0, Stroke::new(1.5, Color32::from_rgb(230, 60, 60)));
        p.circle_stroke(s, 4.0, Stroke::new(1.0, Color32::WHITE));
    }
}

/// A model's edges (a sample of them on very dense models).
fn wireframe(p: &egui::Painter, pane: &Pane, file: &str, mx: &[[f64; 4]; 3], col: Color32) {
    let Some(mesh) = super::mesh_of(file) else { return };
    let tris = mesh.triangles();
    let every = (tris / 6000).max(1);
    let pts: Vec<Option<Pos2>> = mesh.positions.iter().map(|q| pane.to_screen(super::xform(mx, q.map(|v| v as f64)))).collect();
    let s = Stroke::new(0.7, col);
    let mut shapes = Vec::with_capacity(tris / every * 3);
    for t in (0..tris).step_by(every) {
        let [a, b, c] = mesh.tri(t);
        for (i, j) in [(a, b), (b, c), (c, a)] {
            if i < j
                && let (Some(x), Some(y)) = (pts[i], pts[j])
            {
                shapes.push(egui::Shape::line_segment([x, y], s));
            }
        }
    }
    p.extend(shapes);
}

// ------------------------------------------------------------ view gizmo

const GIZMO_R: f32 = 38.0;

fn gizmo_centre(pane: &Pane) -> Pos2 {
    pane.rect.right_top() + Vec2::new(-GIZMO_R - 14.0, GIZMO_R + 14.0)
}

/// The axis balls of the view gizmo: (screen position, axis index, sign,
/// depth towards the viewer).
fn gizmo_balls(pane: &Pane) -> Vec<(Pos2, usize, f32, f64)> {
    let c = gizmo_centre(pane);
    let a = pane.nav.axes();
    let mut v = Vec::new();
    for i in 0..3 {
        for sgn in [1.0f32, -1.0] {
            let mut d = [0.0; 3];
            d[i] = sgn as f64;
            // Screen right and down, depth away.
            let sx = a[0][0] * d[0] + a[0][1] * d[1] + a[0][2] * d[2];
            let sy = a[1][0] * d[0] + a[1][1] * d[1] + a[1][2] * d[2];
            let sz = a[2][0] * d[0] + a[2][1] * d[1] + a[2][2] * d[2];
            v.push((c + Vec2::new(sx as f32, sy as f32) * (GIZMO_R - 9.0), i, sgn, -sz));
        }
    }
    // Far first, so near balls draw on top.
    v.sort_by(|x, y| x.3.partial_cmp(&y.3).unwrap_or(std::cmp::Ordering::Equal));
    v
}

fn view_gizmo_draw(p: &egui::Painter, pane: &Pane) {
    let c = gizmo_centre(pane);
    p.circle_filled(c, GIZMO_R, Color32::from_black_alpha(50));
    for (pos, i, sgn, _) in gizmo_balls(pane) {
        let col = AXIS_COLOURS[i];
        if sgn > 0.0 {
            p.line_segment([c, pos], Stroke::new(2.0, col));
            p.circle_filled(pos, 8.0, col);
            // y points down in the world: the gizmo names world up "-Y".
            p.text(pos, Align2::CENTER_CENTER, ["X", "Y", "Z"][i], theme::caption(), Color32::BLACK);
        } else {
            p.circle_filled(pos, 6.5, theme::alpha(col, 0.35));
            p.circle_stroke(pos, 6.5, Stroke::new(1.0, col));
        }
    }
    // Buttons under it: perspective/ortho, camera, frame, quad.
}

/// The axis view a gizmo ball names: the eye looks along `-axis` from
/// that side (clicking +Y, below in y-down space, views from below).
fn axis_for_ball(i: usize, sgn: f32) -> Axis {
    match (i, sgn > 0.0) {
        // From +x looking back at the scene: the right view.
        (0, true) => Axis::Right,
        (0, false) => Axis::Left,
        // +y is down: from below.
        (1, true) => Axis::Bottom,
        (1, false) => Axis::Top,
        // From +z (behind the canvas): the back view.
        (2, true) => Axis::Back,
        _ => Axis::Front,
    }
}

// ------------------------------------------------------------ interaction

#[derive(Clone, Copy, Debug)]
enum NavDrag {
    Orbit,
    Pan,
    Zoom,
    Look,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Handle {
    Axis(usize),
    /// Move in the view plane / scale evenly.
    Centre,
    Ring(usize),
}

#[derive(Clone, Debug)]
struct GizmoDrag {
    handle: Handle,
    tool: Tool,
    start: Pos2,
    pivot: [f64; 3],
    axes: [[f64; 3]; 3],
    /// Screen pixels per world unit along the held axis.
    rate: Vec2,
    /// The gizmo's length in world pixels.
    len: f64,
    /// The first selected object's own axes (columns: x, y, z).
    local: sp::Mat3,
    applied: super::Change,
    ids: Vec<u64>,
    individual: bool,
}

fn gizmo_axes(ws: &Workspace, pane: &Pane, objs: &[Object], sel: &[u64]) -> [[f64; 3]; 3] {
    match ws.orient {
        Orient::World => [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        Orient::View => {
            let a = pane.nav.axes();
            // Right, down, towards the viewer.
            [a[0], a[1], [-a[2][0], -a[2][1], -a[2][2]]]
        }
        Orient::Local | Orient::Normal => {
            let o = sel.first().and_then(|id| objs.iter().find(|o| o.id == *id));
            match o {
                Some(o) => {
                    let col = |c: usize| [o.axes[0][c], o.axes[1][c], o.axes[2][c]];
                    [col(0), col(1), col(2)]
                }
                None => [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            }
        }
    }
}

fn pivot_of(ws: &Workspace, objs: &[Object], sel: &[u64]) -> Option<[f64; 3]> {
    let pts: Vec<[f64; 3]> = objs.iter().filter(|o| sel.contains(&o.id)).map(|o| o.anchor).collect();
    if pts.is_empty() {
        return None;
    }
    Some(match ws.pivot {
        Pivot::Cursor => ws.cursor,
        Pivot::Active => pts[0],
        Pivot::Bounds => {
            let (lo, hi) = super::bounds_of(&pts);
            std::array::from_fn(|i| (lo[i] + hi[i]) / 2.0)
        }
        _ => {
            let n = pts.len() as f64;
            std::array::from_fn(|i| pts.iter().map(|p| p[i]).sum::<f64>() / n)
        }
    })
}

fn seg_dist(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_sq().max(1e-6)).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

/// The gizmo's parts on screen: per axis the tip, per axis the ring.
pub fn gizmo_parts(pane: &Pane, pivot: [f64; 3], axes: &[[f64; 3]; 3]) -> Option<(Pos2, [Option<Pos2>; 3], f64, [Vec<Pos2>; 3])> {
    let a = pane.to_screen(pivot)?;
    let depth = pane.view.view(pivot)[2].max(1.0);
    let len = 80.0 / pane.scale as f64 * depth / pane.view.zoom;
    let tips = std::array::from_fn(|i| pane.to_screen([pivot[0] + axes[i][0] * len, pivot[1] + axes[i][1] * len, pivot[2] + axes[i][2] * len]));
    let rings = std::array::from_fn(|i| {
        let u = axes[(i + 1) % 3];
        let v = axes[(i + 2) % 3];
        (0..=48)
            .filter_map(|k| {
                let th = k as f64 / 48.0 * std::f64::consts::TAU;
                let (s, c) = th.sin_cos();
                pane.to_screen([pivot[0] + (u[0] * c + v[0] * s) * len * 0.8, pivot[1] + (u[1] * c + v[1] * s) * len * 0.8, pivot[2] + (u[2] * c + v[2] * s) * len * 0.8])
            })
            .collect()
    });
    Some((a, tips, len, rings))
}

pub fn handle_under(tool: Tool, p: Pos2, a: Pos2, tips: &[Option<Pos2>; 3], rings: &[Vec<Pos2>; 3]) -> Option<Handle> {
    if p.distance(a) < 9.0 && tool != Tool::Rotate {
        return Some(Handle::Centre);
    }
    match tool {
        Tool::Move | Tool::Scale => {
            let mut best: Option<(f32, Handle)> = None;
            for (i, t) in tips.iter().enumerate() {
                if let Some(t) = t {
                    let d = seg_dist(p, a, *t);
                    if d < 8.0 && best.is_none_or(|b| d < b.0) {
                        best = Some((d, Handle::Axis(i)));
                    }
                }
            }
            best.map(|b| b.1)
        }
        Tool::Rotate => {
            let mut best: Option<(f32, Handle)> = None;
            for (i, r) in rings.iter().enumerate() {
                for s in r.windows(2) {
                    let d = seg_dist(p, s[0], s[1]);
                    if d < 7.0 && best.is_none_or(|b| d < b.0) {
                        best = Some((d, Handle::Ring(i)));
                    }
                }
            }
            best.map(|b| b.1)
        }
        Tool::Select => None,
    }
}

pub fn draw_gizmo(p: &egui::Painter, tool: Tool, a: Pos2, tips: &[Option<Pos2>; 3], rings: &[Vec<Pos2>; 3], hot: Option<Handle>) {
    match tool {
        Tool::Move | Tool::Scale => {
            for (i, tip) in tips.iter().enumerate() {
                let Some(tip) = *tip else { continue };
                let on = hot == Some(Handle::Axis(i));
                let col = if on { Color32::WHITE } else { AXIS_COLOURS[i] };
                p.line_segment([a, tip], Stroke::new(if on { 3.0 } else { 2.0 }, col));
                let dir = (tip - a).normalized();
                if !dir.is_finite() {
                    continue;
                }
                if tool == Tool::Move {
                    let side = dir.rot90() * 5.0;
                    p.add(egui::Shape::convex_polygon(vec![tip + dir * 11.0, tip + side, tip - side], col, Stroke::NONE));
                } else {
                    p.rect_filled(Rect::from_center_size(tip, Vec2::splat(9.0)), 1, col);
                }
            }
            let on = hot == Some(Handle::Centre);
            p.circle_stroke(a, 7.0, Stroke::new(if on { 2.5 } else { 1.5 }, if on { Color32::WHITE } else { Color32::from_white_alpha(200) }));
        }
        Tool::Rotate => {
            for (i, r) in rings.iter().enumerate() {
                let on = hot == Some(Handle::Ring(i));
                if r.len() > 2 {
                    p.add(egui::Shape::line(r.clone(), Stroke::new(if on { 3.0 } else { 1.8 }, if on { Color32::WHITE } else { AXIS_COLOURS[i] })));
                }
            }
        }
        Tool::Select => {}
    }
}

pub fn readout(ui: &egui::Ui, pos: Pos2, text: &str) {
    let t = theme::tokens(ui.ctx());
    let lp = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::unique("viewport-readout")));
    let g = lp.layout_no_wrap(text.to_owned(), theme::body_medium(), t.text);
    let br = Rect::from_min_size(pos + Vec2::new(14.0, 14.0), g.size() + Vec2::new(12.0, 6.0));
    lp.rect(br, 6, t.panel, Stroke::new(1.0, t.line), egui::StrokeKind::Inside);
    lp.galley(br.min + Vec2::new(6.0, 3.0), g, t.text);
}

/// Applies a navigation step to the pane's view: the editor's eye, a quad
/// pane, or the camera looked through (when locked to the view).
fn navigate(app: &mut StudioApp, pane: &Pane, t_now: f64, f: impl FnOnce(&mut Nav)) {
    let time = time_of(app);
    let locked_cam = super::with(|ws| {
        ws.rt.glide = None;
        ws.rt.moved_at = t_now;
        if pane.index == 3 && pane.camera.is_some() && ws.lock_camera {
            return pane.camera;
        }
        if pane.index == 3 {
            leave_camera(ws, &app.doc, time);
        } else {
            ws.active = pane.index;
        }
        None
    });
    match locked_cam {
        Some(id) => {
            let mut n = super::camera_nav(&app.doc, id, time).unwrap_or(pane.nav);
            f(&mut n);
            n.ortho = false;
            let _ = super::set_camera_to_view(&mut app.doc, id, &n, time, Some(&format!("camera-lock-{id}")));
        }
        None => super::with(|ws| {
            let n = if pane.index < 3 { &mut ws.panes[pane.index] } else { &mut ws.nav };
            f(n);
            ws.changed();
        }),
    }
}

fn interact(app: &mut StudioApp, ui: &mut egui::Ui, pane: &Pane, objs: &[Object], time: Time, t_now: f64) {
    let ctx = ui.ctx().clone();
    let w = app.doc.project.width as f64;
    let ws = super::snapshot();
    let held_id = egui::Id::unique("viewport-held-pane");
    let nav_id = egui::Id::unique("viewport-nav-drag");
    let giz_id = egui::Id::unique("viewport-gizmo-drag");
    // Not focusable: keyboard focus on the pane would keep the workspace's
    // own keys (Tab, the modelling keys) from it.
    let resp = ui.interact(pane.rect, egui::Id::unique(("viewport-pane", pane.index)), Sense::CLICK | Sense::DRAG);
    let (mods, pointer, delta) = ctx.input(|i| (i.modifiers, i.pointer.hover_pos(), i.pointer.delta()));
    let sel: Vec<u64> = app.editor.selection.iter().map(|i| *i as u64).filter(|id| objs.iter().any(|o| o.id == *id)).collect();

    // The terrain brush paints on the ground under the pointer.
    if pane.index == 3 && pane.camera.is_none() && brush_step(app, &ctx, pane, t_now) {
        return;
    }

    // The view gizmo: click a ball for that axis, drag to orbit.
    if pane.camera.is_none()
        && let Some(p) = pointer
        && p.distance(gizmo_centre(pane)) <= GIZMO_R
    {
        if resp.clicked() {
            if let Some((_, i, sgn, _)) = gizmo_balls(pane).into_iter().rev().find(|b| b.0.distance(p) < 10.0) {
                let a = axis_for_ball(i, sgn);
                super::with(|ws| {
                    let mut n = if pane.index < 3 { ws.panes[pane.index] } else { ws.nav };
                    n.set_axis(a);
                    if pane.index < 3 {
                        ws.active = pane.index;
                    }
                    ws.glide_to(n, t_now);
                });
            }
            return;
        }
        if resp.drag_started() {
            ctx.data_mut(|d| d.insert_temp(nav_id, (NavDrag::Orbit, true)));
        }
    }

    // Fly / walk: the pointer looks, keys travel.
    if let Some(mode) = ws.rt.fly {
        fly_step(app, &ctx, pane, mode, t_now, true);
        return;
    }

    // Wheel, trackpad and pinch.
    let (wheel, pinch) = ctx.input(|i| {
        let mut lines = Vec2::ZERO;
        let mut points = Vec2::ZERO;
        for e in &i.events {
            if let egui::Event::MouseWheel { unit, delta, source, .. } = e {
                // Fingers on a trackpad (and their momentum) navigate; a
                // wheel zooms.
                let trackpad = match source {
                    egui::MouseWheelSource::Trackpad | egui::MouseWheelSource::Momentum => true,
                    egui::MouseWheelSource::Wheel => false,
                    _ => *unit == egui::MouseWheelUnit::Point,
                };
                let d = match unit {
                    egui::MouseWheelUnit::Point => *delta,
                    egui::MouseWheelUnit::Line => *delta * 40.0,
                    egui::MouseWheelUnit::Page => *delta * 400.0,
                };
                if trackpad {
                    points += d;
                } else {
                    lines += d;
                }
            }
        }
        ((lines, points), i.zoom_delta())
    });
    if resp.hovered() {
        let at = pointer.map(|p| pane.to_canvas(p)).unwrap_or([0.0, 0.0]);
        if wheel.0.y.abs() > 0.0 {
            let f = (-(wheel.0.y as f64) / 400.0).exp();
            navigate(app, pane, t_now, |n| n.zoom_at(f, at, w));
        }
        if wheel.1 != Vec2::ZERO {
            let d = wheel.1;
            if mods.command || mods.ctrl {
                let f = (-(d.y as f64) / 200.0).exp();
                navigate(app, pane, t_now, |n| n.zoom_at(f, at, w));
            } else if mods.shift {
                let k = 1.0 / pane.scale as f64;
                navigate(app, pane, t_now, |n| n.pan(-(d.x as f64) * k, -(d.y as f64) * k, w));
            } else {
                navigate(app, pane, t_now, |n| {
                    n.ortho = n.ortho && pane.index < 3;
                    n.orbit(-(d.x as f64) * 0.35, -(d.y as f64) * 0.35);
                });
            }
        }
        if (pinch - 1.0).abs() > 1e-4 {
            let f = 1.0 / pinch as f64;
            navigate(app, pane, t_now, |n| n.zoom_at(f, at, w));
        }
    }

    // Edit and sculpt mode pick and shape the open model instead of
    // objects (navigation stays).
    let editing = crate::editor::model_edit::stage() != crate::editor::model_edit::Stage::Object;
    // Starting a drag: a gizmo handle, else navigation by button.
    let gizmo_now = (ws.tool != Tool::Select && !sel.is_empty() && !editing)
        .then(|| {
            let pivot = pivot_of(&ws, objs, &sel)?;
            let axes = gizmo_axes(&ws, pane, objs, &sel);
            gizmo_parts(pane, pivot, &axes).map(|g| (pivot, axes, g))
        })
        .flatten();
    let mut gdrag: Option<GizmoDrag> = ctx.data(|d| d.get_temp(giz_id));
    let hot = match (&gdrag, &gizmo_now, pointer) {
        (Some(g), _, _) => Some(g.handle),
        (None, Some((_, _, (a, tips, _, rings))), Some(p)) => handle_under(ws.tool, p, *a, tips, rings),
        _ => None,
    };
    if let Some((_, _, (a, tips, _, rings))) = &gizmo_now {
        draw_gizmo(&ui.painter().with_clip_rect(pane.rect), ws.tool, *a, tips, rings, hot);
    }
    let primary = ctx.input(|i| i.pointer.primary_down());
    let middle = ctx.input(|i| i.pointer.middle_down());
    let secondary = ctx.input(|i| i.pointer.secondary_down());
    if resp.drag_started() {
        ctx.data_mut(|d| d.insert_temp(held_id, pane.index));
        let nav_drag = if middle {
            Some(if mods.shift { NavDrag::Pan } else if mods.command || mods.ctrl { NavDrag::Zoom } else { NavDrag::Orbit })
        } else if secondary {
            Some(NavDrag::Look)
        } else if primary && mods.alt {
            Some(if mods.shift { NavDrag::Pan } else if mods.command || mods.ctrl { NavDrag::Zoom } else { NavDrag::Orbit })
        } else {
            None
        };
        if let Some(nd) = nav_drag {
            ctx.data_mut(|d| d.insert_temp(nav_id, (nd, false)));
        } else if let (Some(h), Some((pivot, axes, (a, tips, len, _)))) = (hot, &gizmo_now) {
            let start = ctx.input(|i| i.pointer.press_origin()).unwrap_or(*a);
            let rate = match h {
                Handle::Axis(i) => {
                    let s = tips[i].map_or(Vec2::X, |tip| (tip - *a) / *len as f32);
                    s / s.length_sq().max(1e-9)
                }
                _ => Vec2::X,
            };
            let local = sel.first().and_then(|id| objs.iter().find(|o| o.id == *id)).map_or(sp::rotation([0.0; 3], 0.0, 0.0, 0.0), |o| o.axes);
            gdrag = Some(GizmoDrag { handle: h, tool: ws.tool, start, pivot: *pivot, axes: *axes, rate, len: *len, local, applied: super::Change::default(), ids: sel.clone(), individual: ws.pivot == Pivot::Individual });
        }
    }
    if let Some((nd, _)) = ctx.data(|d| d.get_temp::<(NavDrag, bool)>(nav_id))
        && delta != Vec2::ZERO
    {
        let (dx, dy) = (delta.x as f64, delta.y as f64);
        let k = 1.0 / pane.scale as f64;
        let pivot = pivot_of(&ws, objs, &sel);
        match nd {
            NavDrag::Orbit => navigate(app, pane, t_now, |n| {
                if pane.index == 3 {
                    n.ortho = false;
                }
                match pivot {
                    Some(p) if pane.index == 3 => n.orbit_about(p, dx * 0.35, dy * 0.35),
                    _ => n.orbit(dx * 0.35, dy * 0.35),
                }
            }),
            NavDrag::Pan => navigate(app, pane, t_now, |n| n.pan(dx * k, dy * k, w)),
            NavDrag::Zoom => navigate(app, pane, t_now, |n| n.dolly((dy / 150.0).exp())),
            NavDrag::Look => {
                navigate(app, pane, t_now, |n| n.look(dx * 0.25, dy * 0.25));
                fly_step(app, &ctx, pane, Fly::Fly, t_now, false);
            }
        }
        ctx.request_repaint();
    } else if let Some(NavDrag::Look) = ctx.data(|d| d.get_temp::<(NavDrag, bool)>(nav_id)).map(|x| x.0) {
        // Holding the right button still: keys keep flying.
        fly_step(app, &ctx, pane, Fly::Fly, t_now, false);
    }
    if let (Some(mut g), Some(pos)) = (gdrag.clone(), pointer) {
        let total = gizmo_change(&g, pane, pos, &ws, w);
        let step = super::Change {
            moved: std::array::from_fn(|i| total.moved[i] - g.applied.moved[i]),
            turned: std::array::from_fn(|i| total.turned[i] - g.applied.turned[i]),
            turn_about: total.turn_about.map(|(ax, deg)| (ax, deg - g.applied.turn_about.map_or(0.0, |a| a.1))),
            scaled: total.scaled.map(|k| {
                let was = g.applied.scaled.unwrap_or([1.0; 3]);
                std::array::from_fn(|i| k[i] / was[i].max(1e-9))
            }),
        };
        let any = step.moved.iter().chain(step.turned.iter()).any(|v| v.abs() > 1e-9) || step.turn_about.is_some_and(|a| a.1.abs() > 1e-9) || step.scaled.is_some_and(|k| k.iter().any(|v| (v - 1.0).abs() > 1e-9));
        if any {
            let label = match g.tool {
                Tool::Rotate => "Rotate in the viewport",
                Tool::Scale => "Scale in the viewport",
                _ => "Move in the viewport",
            };
            let pivot = (!g.individual).then_some(g.pivot);
            if super::change(&mut app.doc, &g.ids, time, &step, pivot, label, Some("viewport-gizmo")).is_ok() {
                g.applied = total;
            }
        }
        let txt = match g.tool {
            Tool::Rotate => format!("{:.1}°", total.turn_about.map_or(0.0, |a| a.1)),
            Tool::Scale => format!("× {:.2}", total.scaled.map_or(1.0, |k| k[0].max(k[1]).max(k[2]))),
            _ => format!("{:.0}, {:.0}, {:.0}", total.moved[0], total.moved[1], total.moved[2]),
        };
        readout(ui, pos, &txt);
        gdrag = Some(g);
    }
    let navigating = ctx.data(|d| d.get_temp::<(NavDrag, bool)>(nav_id)).is_some();
    if editing && !navigating {
        match crate::editor::model_edit::stage() {
            crate::editor::model_edit::Stage::Sculpt => {
                crate::editor::model_edit::sculpt3d::interact(app, ui, pane, &resp);
            }
            _ => crate::editor::model_edit::input::interact(app, ui, pane, &resp),
        }
    }
    if ctx.input(|i| i.pointer.any_released()) {
        ctx.data_mut(|d| {
            d.remove::<GizmoDrag>(giz_id);
            d.remove::<(NavDrag, bool)>(nav_id);
            d.remove::<usize>(held_id);
        });
        app.doc.end_gesture();
    } else if let Some(g) = gdrag {
        ctx.data_mut(|d| d.insert_temp(giz_id, g));
    }
    if editing {
        return;
    }

    // Click: pick (Shift adds, removes when already picked); clicking
    // nothing clears; double click frames what was clicked.
    if resp.clicked() && !mods.alt
        && let Some(p) = pointer
    {
        let hit = super::pick(objs, &pane.view, pane.to_canvas(p), 12.0 / pane.scale as f64, &ws.locked).map(|h| h.0 as u32);
        let s = &mut app.editor.selection;
        match (hit, mods.shift) {
            (Some(id), true) => {
                if let Some(i) = s.iter().position(|x| *x == id) {
                    s.remove(i);
                } else {
                    s.push(id);
                }
            }
            (Some(id), false) => *s = vec![id],
            (None, false) => s.clear(),
            (None, true) => {}
        }
    }
    if resp.double_clicked()
        && let Some(id) = app.editor.selection.first().copied()
    {
        let aspect = pane.rect.width() as f64 / pane.rect.height().max(1.0) as f64;
        frame(app, &[id as u64], t_now, aspect);
    }
    // Right click without a drag: put the 3D cursor where the ray meets
    // what is under it, or the floor.
    if resp.secondary_clicked()
        && let Some(p) = pointer
    {
        let at = pane.to_canvas(p);
        let o = pane.view.position;
        let d = pane.view.ray(m::Vec2::new(at[0], at[1]));
        let l = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-12);
        let d = d.map(|v| v / l);
        let hit = super::pick(objs, &pane.view, at, 0.0, &BTreeSet::new()).map(|h| h.1);
        let floor = ws.floor_y(app.doc.project.height as f64);
        let t_floor = (d[1].abs() > 1e-9).then(|| (floor - o[1]) / d[1]).filter(|t| *t > 0.0);
        if let Some(t) = hit.or(t_floor) {
            super::with(|ws| {
                ws.cursor = [o[0] + d[0] * t, o[1] + d[1] * t, o[2] + d[2] * t];
                ws.changed();
            });
        }
    }
}

/// The change a gizmo drag makes so far (snapped when snapping is on or
/// Cmd is held).
fn gizmo_change(g: &GizmoDrag, pane: &Pane, pos: Pos2, ws: &Workspace, width: f64) -> super::Change {
    let snap = ws.snap.on ^ pane_ctx_cmd();
    let mut c = super::Change::default();
    let d = pos - g.start;
    let wpp = pane.view.view(g.pivot)[2].max(1.0) / pane.view.zoom / pane.scale as f64;
    let _ = width;
    match (g.tool, g.handle) {
        (Tool::Move, Handle::Axis(i)) => {
            let mut k = d.dot(g.rate) as f64;
            if snap {
                k = super::Snap::value(k, ws.snap.step);
            }
            c.moved = g.axes[i].map(|v| v * k);
        }
        (Tool::Move, Handle::Centre) => {
            let a = pane.view.axes;
            let mut mv: [f64; 3] = std::array::from_fn(|j| a[0][j] * d.x as f64 * wpp + a[1][j] * d.y as f64 * wpp);
            if snap {
                mv = mv.map(|v| super::Snap::value(v, ws.snap.step));
            }
            c.moved = mv;
        }
        (Tool::Rotate, Handle::Ring(i)) => {
            let centre = pane.to_screen(g.pivot).unwrap_or(g.start);
            let (a0, a1) = ((g.start - centre).angle(), (pos - centre).angle());
            let mut deg = (a1 - a0).to_degrees() as f64;
            deg -= (deg / 360.0).round() * 360.0;
            // Screen angles grow clockwise (y down); a right-handed turn
            // about an axis pointing at the viewer goes anticlockwise.
            let ax = g.axes[i];
            let fwd = pane.view.axes[2];
            let away = ax[0] * fwd[0] + ax[1] * fwd[1] + ax[2] * fwd[2];
            let mut deg = if away > 0.0 { deg } else { -deg };
            if snap {
                deg = super::Snap::value(deg, ws.snap.angle);
            }
            c.turn_about = Some((ax, deg));
        }
        (Tool::Scale, Handle::Axis(i)) => {
            let mut k = 1.0 + d.dot(g.rate) as f64 / g.len.max(1e-9);
            if snap {
                k = 1.0 + super::Snap::value(k - 1.0, ws.snap.scale);
            }
            // Scale is the object's own: the own axis nearest the handle.
            let ax = g.axes[i];
            let along = |j: usize| (g.local[0][j] * ax[0] + g.local[1][j] * ax[1] + g.local[2][j] * ax[2]).abs();
            let j = (0..3).max_by(|x, y| along(*x).partial_cmp(&along(*y)).unwrap_or(std::cmp::Ordering::Equal)).unwrap_or(i);
            let mut s = [1.0; 3];
            s[j] = k.max(0.01);
            c.scaled = Some(s);
        }
        (Tool::Scale, Handle::Centre) => {
            let mut k = (1.0 + (d.x - d.y) as f64 / 150.0).max(0.01);
            if snap {
                k = 1.0 + super::Snap::value(k - 1.0, ws.snap.scale);
            }
            c.scaled = Some([k; 3]);
        }
        _ => {}
    }
    c
}

thread_local! {
    static CMD: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn pane_ctx_cmd() -> bool {
    CMD.with(|c| c.get())
}

/// Writes the terrain strokes into the world and puts the brush away.
fn finish_brush(app: &mut StudioApp, ctx: &egui::Context) {
    super::with(|w| w.rt.brush = None);
    match super::world::commit(&mut app.doc) {
        Ok(msg) if !msg.is_empty() => {
            kit::push(ctx, msg, None, Some(Icon::Aperture));
        }
        Ok(_) => super::world::discard(),
        Err(e) => {
            super::world::discard();
            kit::push(ctx, e, None, Some(Icon::Warning));
        }
    }
    super::world::clear_hover();
}

/// The terrain brush under the pointer: shows where, and strokes while
/// the primary button is held. Returns true while it holds the pointer.
fn brush_step(app: &mut StudioApp, ctx: &egui::Context, pane: &Pane, t_now: f64) -> bool {
    let Some(brush) = super::with(|w| w.rt.brush) else { return false };
    let Some(found) = super::world::find(&app.doc, time_of(app)) else { return false };
    let (pos, down, released) = ctx.input(|i| (i.pointer.hover_pos(), i.pointer.primary_down(), i.pointer.primary_released()));
    let Some(pos) = pos.filter(|p| pane.rect.contains(*p)) else {
        super::world::clear_hover();
        if released && super::world::pending() {
            finish_stroke(app, ctx);
        }
        return false;
    };
    let hit = super::world::hover(&found, &pane.view, pane.to_canvas(pos), &brush);
    ctx.request_repaint();
    if down && let Some(h) = hit {
        super::world::stroke(&found, &brush, h, t_now);
        return true;
    }
    if released && super::world::pending() {
        finish_stroke(app, ctx);
    }
    false
}

/// A stroke ended: written into the world (one undo step), the brush stays.
fn finish_stroke(app: &mut StudioApp, ctx: &egui::Context) {
    match super::world::commit(&mut app.doc) {
        Ok(_) => {}
        Err(e) => {
            kit::push(ctx, e, None, Some(Icon::Warning));
        }
    }
}

/// Leaves walk mode properly when the mode was switched off anywhere
/// (keys, menu, the AI): the pointer is let go and a recording in
/// progress is keyed.
fn walk_sync(app: &mut StudioApp, ctx: &egui::Context) {
    let (walking, st) = super::with(|w| (w.rt.fly == Some(Fly::Walk), w.rt.walk.is_some()));
    if walking || !st {
        if !walking {
            super::world::grab(ctx, false);
        }
        return;
    }
    let state = super::with(|w| w.rt.walk.take());
    super::world::grab(ctx, false);
    if let Some(mut s) = state
        && let Some(rec) = s.rec.take()
    {
        finish_walk(app, ctx, &rec, s.k);
    }
}

fn finish_walk(app: &mut StudioApp, ctx: &egui::Context, rec: &super::world::WalkRec, k: f32) {
    let id = rec.camera.or_else(|| super::look_camera(&app.doc, Look::Camera(None), rec.at)).or_else(|| super::super::three_d::add_rig(app, None).map(|i| i as u64));
    let Some(id) = id else { return };
    match super::world::finish_recording(&mut app.doc, id, rec, k) {
        Ok(msg) => kit::push(ctx, msg, None, Some(Icon::Aperture)),
        Err(e) => kit::push(ctx, e, None, Some(Icon::Warning)),
    };
}

/// One frame of walking in a world.
fn walk_world(app: &mut StudioApp, ctx: &egui::Context, found: &super::world::Found, t_now: f64) {
    let (w, h) = (app.doc.project.width as f64, app.doc.project.height as f64);
    let existing = super::with(|ws| ws.rt.walk.take()).map(|b| *b);
    let mut st = match existing {
        Some(s) if s.file == found.file => s,
        _ => {
            let nav = super::with(|ws| ws.nav);
            super::world::WalkState::enter(found, &nav, w, h)
        }
    };
    if ctx.input(|i| i.key_pressed(egui::Key::V)) {
        st.ex.third_person = !st.ex.third_person;
    }
    // "Record walk" asked for: start from here.
    if super::with(|ws| std::mem::take(&mut ws.rt.want_record)) && st.rec.is_none() {
        let camera = super::look_camera(&app.doc, Look::Camera(None), app.editor.playhead);
        st.rec = Some(super::world::WalkRec { recorder: Default::default(), t0: t_now, camera, at: app.editor.playhead });
        if let Some(r) = &mut st.rec {
            r.recorder.start();
        }
        kit::push(ctx, "Recording the walk: move; choose Record walk again to stop", None, Some(Icon::Aperture));
    }
    let mut nav = super::with(|ws| ws.nav);
    let active = super::world::step(ctx, &mut st, found, &mut nav, w, h, t_now);
    super::world::grab(ctx, true);
    super::with(|ws| {
        ws.nav = nav;
        ws.look = Look::Editor;
        if active {
            ws.rt.moved_at = t_now;
        }
        ws.rt.glide = None;
        ws.rt.walk = Some(Box::new(st));
        ws.changed();
    });
    if active {
        ctx.request_repaint();
    }
}

/// One frame of flying or walking: the pointer looks (in fly mode) and
/// W A S D Q E travel.
fn fly_step(app: &mut StudioApp, ctx: &egui::Context, pane: &Pane, mode: Fly, t_now: f64, look: bool) {
    // Walking in a world is a game's walk: gravity, collision, jumping.
    let found = if mode == Fly::Walk && look {
        let time = time_of(app);
        super::world::find(&app.doc, time).or_else(|| {
            let (h, hidden) = super::with(|w| (app.doc.project.height as f64, w.hidden.clone()));
            let floor = super::with(|w| w.floor_y(h));
            super::world::scene_found(&app.doc, time, floor, &hidden)
        })
    } else {
        None
    };
    if let Some(found) = found {
        walk_world(app, ctx, &found, t_now);
        return;
    }
    let (dt, delta, mods, keys, scroll) = ctx.input(|i| {
        let k = |key: egui::Key| i.key_down(key);
        (i.stable_dt.min(0.1) as f64, i.pointer.delta(), i.modifiers, [k(egui::Key::W), k(egui::Key::S), k(egui::Key::A), k(egui::Key::D), k(egui::Key::Q), k(egui::Key::E)], i.smooth_scroll_delta.y)
    });
    // Keys the editor would act on are taken while flying.
    ctx.input_mut(|i| {
        for k in [egui::Key::W, egui::Key::S, egui::Key::A, egui::Key::D, egui::Key::Q, egui::Key::E] {
            i.events.retain(|e| !matches!(e, egui::Event::Key { key, .. } if *key == k));
        }
    });
    if scroll.abs() > 0.5 {
        super::with(|ws| {
            ws.speed = (ws.speed * (scroll as f64 / 300.0).exp()).clamp(10.0, 1e6);
            ws.changed();
        });
    }
    let speed = super::with(|w| w.speed) * if mods.shift { 4.0 } else { 1.0 } * dt;
    let f = (keys[0] as i32 - keys[1] as i32) as f64 * speed;
    let r = (keys[3] as i32 - keys[2] as i32) as f64 * speed;
    let u = (keys[5] as i32 - keys[4] as i32) as f64 * speed;
    let looking = look && delta != Vec2::ZERO;
    if f != 0.0 || r != 0.0 || u != 0.0 || looking {
        navigate(app, pane, t_now, |n| {
            n.ortho = false;
            if looking {
                n.look(delta.x as f64 * 0.25, delta.y as f64 * 0.25);
            }
            match mode {
                Fly::Fly => n.travel([r, -u, f]),
                Fly::Walk => n.walk(f, r, u),
            }
        });
    }
    ctx.request_repaint();
}

/// While recording a fly-through: the view at each frame of playback;
/// when playback stops the path is keyed into the camera.
fn record_tick(app: &mut StudioApp, ctx: &egui::Context) {
    let Some(rec) = super::with(|w| w.rt.recording.clone()) else { return };
    if app.editor.playing {
        let nav = super::with(|w| w.nav);
        let t = app.editor.playhead;
        super::with(|w| {
            if let Some(r) = &mut w.rt.recording
                && r.samples.last().is_none_or(|s| s.0 != t)
            {
                r.samples.push((t, nav));
            }
        });
        ctx.request_repaint();
        return;
    }
    if rec.samples.is_empty() {
        return;
    }
    super::with(|w| w.rt.recording = None);
    finish_recording(app, ctx, rec);
}

fn finish_recording(app: &mut StudioApp, ctx: &egui::Context, rec: super::Recording) {
    let id = rec.camera.or_else(|| super::super::three_d::add_rig(app, None).map(|i| i as u64));
    let Some(id) = id else { return };
    match super::record_path(&mut app.doc, id, &rec.samples) {
        Ok(n) => kit::push(ctx, format!("Recorded the camera path: {n} keys"), None, Some(Icon::Aperture)),
        Err(e) => kit::push(ctx, e, None, Some(Icon::Warning)),
    };
}

/// Starts or stops recording a fly-through into the camera looked
/// through (or a new one).
pub fn toggle_recording(app: &mut StudioApp, ctx: &egui::Context) {
    let time = time_of(app);
    if let Some(rec) = super::with(|w| w.rt.recording.take()) {
        app.editor.playing = false;
        if !rec.samples.is_empty() {
            finish_recording(app, ctx, rec);
        }
        return;
    }
    let look = super::with(|w| w.look);
    let camera = super::look_camera(&app.doc, look, time);
    super::with(|w| {
        // Fly with the editor's eye from where the camera is.
        if let Some(id) = camera
            && let Some(n) = super::camera_nav(&app.doc, id, time)
        {
            w.nav = n;
        }
        w.look = Look::Editor;
        w.rt.recording = Some(super::Recording { camera, samples: Vec::new() });
        w.rt.fly = Some(Fly::Fly);
    });
    app.editor.playing = true;
    kit::push(ctx, "Recording: fly with the mouse and W A S D while it plays; stop playback to key the path", None, Some(Icon::Aperture));
}

// ------------------------------------------------------------ header

fn header(app: &mut StudioApp, ui: &mut egui::Ui, bar: Rect, panes: &[Pane]) {
    let ctx = ui.ctx().clone();
    CMD.with(|c| c.set(ctx.input(|i| i.modifiers.command)));
    let mut bui = ui.new_child(egui::UiBuilder::new().max_rect(bar).layout(egui::Layout::left_to_right(egui::Align::Center)));
    bui.spacing_mut().item_spacing.x = 3.0;
    let ws = super::snapshot();
    let time = time_of(app);
    let t_now = now(&ctx);
    // Shading.
    for s in Shade::ALL {
        let icon = match s {
            Shade::Solid => Icon::Circle,
            Shade::Material => Icon::DropHalf,
            Shade::Rendered => Icon::Sun,
            Shade::Wireframe => Icon::GridFour,
            Shade::Xray => Icon::Intersect,
        };
        if IconButton::new(icon, s.label()).active(ws.shade == s).compact().show(&mut bui).clicked() {
            super::with(|w| {
                w.shade = s;
                w.changed();
            });
        }
    }
    bui.add_space(8.0);
    for (tool, icon, name, key) in [(Tool::Select, Icon::Cursor, "Select", "Q"), (Tool::Move, Icon::ArrowsOutCardinal, "Move", "G"), (Tool::Rotate, Icon::ArrowCounterClockwise, "Rotate", "R"), (Tool::Scale, Icon::CornersOut, "Scale", "S")] {
        if IconButton::new(icon, name).shortcut(key).active(ws.tool == tool).compact().show(&mut bui).clicked() {
            super::with(|w| {
                w.tool = tool;
                w.changed();
            });
        }
    }
    let orients = ["World", "Local", "View", "Normal"];
    let all_orients = [Orient::World, Orient::Local, Orient::View, Orient::Normal];
    let oi = all_orients.iter().position(|o| *o == ws.orient).unwrap_or(0);
    let ow = super::super::viewer::menu_w(&bui, &orients);
    if let Some(i) = kit::menu_button(&mut bui, "viewport-orient", oi, &orients, ow, None) {
        super::with(|w| {
            w.orient = all_orients[i];
            w.changed();
        });
    }
    let pivots = ["Median", "Each", "Cursor", "Bounds", "Active"];
    let all_pivots = [Pivot::Median, Pivot::Individual, Pivot::Cursor, Pivot::Bounds, Pivot::Active];
    let pi = all_pivots.iter().position(|o| *o == ws.pivot).unwrap_or(0);
    let pw = super::super::viewer::menu_w(&bui, &pivots);
    if let Some(i) = kit::menu_button(&mut bui, "viewport-pivot", pi, &pivots, pw, None) {
        super::with(|w| {
            w.pivot = all_pivots[i];
            w.changed();
        });
    }
    if IconButton::new(Icon::Magnet, "Snap (hold Cmd to flip)").active(ws.snap.on).compact().show(&mut bui).clicked() {
        super::with(|w| {
            w.snap.on = !w.snap.on;
            w.changed();
        });
    }
    bui.add_space(8.0);
    // Terrain brushes, when there is a world to sculpt.
    if super::world::find(&app.doc, time_of(app)).is_some() {
        let id = egui::Id::unique("viewport-terrain-menu");
        let sculpting = ws.rt.brush.is_some();
        let resp = kit::Button::ghost(if sculpting { "Terrain ●" } else { "Terrain" }).compact().show(&mut bui);
        if resp.clicked() {
            kit::toggle_open(&ctx, id);
        }
        let mut picked: Option<Option<super::world::BrushKind>> = None;
        kit::popover(&ctx, id, resp.rect, kit::Side::Below, 230.0, |ui| {
            kit::menu_heading(ui, "Sculpt and paint the ground");
            let cur = ws.rt.brush.unwrap_or_default();
            for kind in super::world::BrushKind::ALL {
                if MenuItem::new(&kind.label()).checked(sculpting && cur.kind == kind).show(ui).clicked() {
                    picked = Some(Some(kind));
                }
            }
            if sculpting {
                kit::labeled_row(ui, "Size", |ui| {
                    let mut r = cur.radius;
                    if kit::Slider::new("terrain-brush-size", &mut r, 1.0..=60.0).unit("m").width(110.0).show(ui).changed() {
                        super::with(|w| w.rt.brush = Some(super::world::Brush { radius: r, ..cur }));
                    }
                });
                kit::labeled_row(ui, "Strength", |ui| {
                    let mut s = cur.strength;
                    if kit::Slider::new("terrain-brush-strength", &mut s, 0.05..=2.0).width(110.0).show(ui).changed() {
                        super::with(|w| w.rt.brush = Some(super::world::Brush { strength: s, ..cur }));
                    }
                });
                kit::menu_separator(ui);
                if MenuItem::new("Done").show(ui).clicked() {
                    picked = Some(None);
                }
            }
        });
        match picked {
            Some(Some(kind)) => super::with(|w| {
                let b = w.rt.brush.unwrap_or_default();
                w.rt.brush = Some(super::world::Brush { kind, ..b });
                // Brushes are used from an orbiting eye, not while walking.
                w.rt.fly = None;
            }),
            Some(None) => finish_brush(app, &ctx),
            None => {}
        }
        bui.add_space(8.0);
    }
    // View menu.
    let view_id = egui::Id::unique("viewport-view-menu");
    let resp = kit::Button::ghost("View").compact().show(&mut bui);
    if resp.clicked() {
        kit::toggle_open(&ctx, view_id);
    }
    let aspect = panes.iter().find(|p| p.index == 3).map_or(16.0 / 9.0, |p| p.rect.width() as f64 / p.rect.height().max(1.0) as f64);
    let mut act: Option<&str> = None;
    let mut go_axis: Option<Axis> = None;
    let mut go_mark: Option<usize> = None;
    kit::popover(&ctx, view_id, resp.rect, kit::Side::Below, 240.0, |ui| super::super::viewer::tall_menu(ui, |ui| {
        kit::menu_heading(ui, "Look through");
        if MenuItem::new("Editor view").checked(ws.look == Look::Editor).show(ui).clicked() {
            act = Some("editor");
        }
        if MenuItem::new("Active camera").shortcut("0").checked(ws.look == Look::Camera(None)).show(ui).clicked() {
            act = Some("camera");
        }
        let cams: Vec<(u64, String)> = app.doc.model.active().clips.iter().filter(|c| matches!(&c.source, m::ClipSource::Generator(g) if g.kind == m::GeneratorKind::Camera)).map(|c| (c.id.0, c.name.clone())).collect();
        for (id, name) in &cams {
            if MenuItem::new(name).checked(ws.look == Look::Camera(Some(*id))).show(ui).clicked() {
                super::with(|w| {
                    w.look = Look::Camera(Some(*id));
                    w.changed();
                });
                kit::set_open(ui.ctx(), view_id, false);
            }
        }
        if MenuItem::new("Set camera to view").show(ui).clicked() {
            act = Some("camera_to_view");
        }
        if MenuItem::new("Lock camera to view").checked(ws.lock_camera).show(ui).clicked() {
            act = Some("lock");
        }
        if MenuItem::new(if ws.rt.recording.is_some() { "Stop recording" } else { "Record fly-through…" }).show(ui).clicked() {
            act = Some("record");
        }
        kit::menu_separator(ui);
        kit::menu_heading(ui, "Views");
        for (a, key) in [(Axis::Front, "1"), (Axis::Right, "3"), (Axis::Top, "7"), (Axis::Back, "⌘1"), (Axis::Left, "⌘3"), (Axis::Bottom, "⌘7")] {
            if MenuItem::new(&format!("{}{}", a.name()[..1].to_uppercase(), &a.name()[1..])).shortcut(key).show(ui).clicked() {
                go_axis = Some(a);
            }
        }
        if MenuItem::new("Perspective / orthographic").shortcut("5").checked(ws.nav.ortho).show(ui).clicked() {
            act = Some("ortho");
        }
        if MenuItem::new("Frame selected").shortcut(".").show(ui).clicked() {
            act = Some("frame_sel");
        }
        if MenuItem::new("Frame all").shortcut("Home").show(ui).clicked() {
            act = Some("frame_all");
        }
        if MenuItem::new("Quad view").checked(ws.quad).show(ui).clicked() {
            act = Some("quad");
        }
        if MenuItem::new("Fly mode").shortcut("`").checked(ws.rt.fly == Some(Fly::Fly)).show(ui).clicked() {
            act = Some("fly");
        }
        if MenuItem::new("Walk mode").shortcut("⇧`").checked(ws.rt.fly == Some(Fly::Walk)).show(ui).clicked() {
            act = Some("walk");
        }
        if super::world::find(&app.doc, time_of(app)).is_some() {
            let third = ws.rt.walk.as_ref().is_some_and(|s| s.ex.third_person);
            if MenuItem::new("Third person").shortcut("V").checked(third).show(ui).clicked() {
                act = Some("third");
            }
            let recording = ws.rt.walk.as_ref().is_some_and(|s| s.rec.is_some());
            if MenuItem::new(if recording { "Stop recording walk" } else { "Record walk" }).show(ui).clicked() {
                act = Some("record_walk");
            }
        }
        kit::menu_separator(ui);
        kit::menu_heading(ui, "Show");
        for (label, on, which) in [("Floor grid", ws.grid, "grid"), ("Cameras, lights, outlines", ws.overlays, "overlays"), ("Canvas frame", ws.frame, "frame"), ("2D layers as cards", ws.cards, "cards"), ("Physics colliders", ws.colliders, "colliders")] {
            if MenuItem::new(label).checked(on).show(ui).clicked() {
                act = Some(which);
            }
        }
        kit::labeled_row(ui, "Spread layers", |ui| {
            let mut v = ws.spread as f32;
            if kit::Slider::new("spread", &mut v, 0.0..=2000.0).unit("px").width(120.0).show(ui).changed() {
                super::with(|w| {
                    w.spread = v as f64;
                    w.changed();
                });
            }
        });
        kit::labeled_row(ui, "Lens", |ui| {
            let mut v = ws.nav.lens as f32;
            if kit::Slider::new("lens", &mut v, 10.0..=200.0).unit("mm").width(120.0).show(ui).changed() {
                super::with(|w| {
                    w.nav.lens = v as f64;
                    w.changed();
                });
            }
        });
        kit::menu_separator(ui);
        kit::menu_heading(ui, "Saved views");
        for (i, b) in ws.bookmarks.iter().enumerate() {
            if MenuItem::new(&b.name).show(ui).clicked() {
                go_mark = Some(i);
            }
        }
        if MenuItem::new("Save this view").show(ui).clicked() {
            act = Some("bookmark");
        }
    }));
    if let Some(a) = go_axis {
        super::with(|w| {
            leave_camera(w, &app.doc, time);
            let mut n = *w.nav_mut();
            n.set_axis(a);
            w.glide_to(n, t_now);
        });
    }
    if let Some(i) = go_mark {
        super::with(|w| {
            if let Some(b) = w.bookmarks.get(i).cloned() {
                w.look = Look::Editor;
                w.glide_to(b.nav, t_now);
            }
        });
    }
    match act {
        Some("editor") => super::with(|w| {
            w.look = Look::Editor;
            w.changed();
        }),
        Some("camera") => super::with(|w| {
            w.look = Look::Camera(None);
            w.changed();
        }),
        Some("camera_to_view") => {
            let nav = super::with(|w| w.nav);
            let id = super::look_camera(&app.doc, Look::Camera(None), time).or_else(|| super::super::three_d::add_rig(app, None).map(|i| i as u64));
            if let Some(id) = id {
                match super::set_camera_to_view(&mut app.doc, id, &nav, time, None) {
                    Ok(()) => kit::push(&ctx, "The camera now sees this view", None, Some(Icon::Aperture)),
                    Err(e) => kit::push(&ctx, e, None, Some(Icon::Warning)),
                };
            }
        }
        Some("lock") => super::with(|w| {
            w.lock_camera = !w.lock_camera;
            if w.lock_camera && w.look == Look::Editor {
                w.look = Look::Camera(None);
            }
            w.changed();
        }),
        Some("record") => toggle_recording(app, &ctx),
        Some("third") => super::with(|w| {
            if let Some(s) = w.rt.walk.as_mut() {
                s.ex.third_person = !s.ex.third_person;
            }
        }),
        Some("record_walk") => {
            let recording = super::with(|w| w.rt.walk.as_ref().is_some_and(|s| s.rec.is_some()));
            if recording {
                let taken = super::with(|w| w.rt.walk.as_mut().and_then(|s| s.rec.take().map(|r| (r, s.k))));
                if let Some((mut rec, k)) = taken {
                    rec.recorder.stop();
                    finish_walk(app, &ctx, &rec, k);
                }
            } else {
                super::with(|w| {
                    w.rt.want_record = true;
                    w.rt.fly = Some(Fly::Walk);
                });
            }
        }
        Some("ortho") => super::with(|w| {
            w.nav.ortho = !w.nav.ortho;
            w.changed();
        }),
        Some("frame_sel") => {
            let sel: Vec<u64> = app.editor.selection.iter().map(|i| *i as u64).collect();
            frame(app, &sel, t_now, aspect);
        }
        Some("frame_all") => frame(app, &[], t_now, aspect),
        Some(which) => super::with(|w| {
            match which {
                "quad" => w.quad = !w.quad,
                "fly" => w.rt.fly = if w.rt.fly == Some(Fly::Fly) { None } else { Some(Fly::Fly) },
                "walk" => w.rt.fly = if w.rt.fly == Some(Fly::Walk) { None } else { Some(Fly::Walk) },
                "grid" => w.grid = !w.grid,
                "overlays" => w.overlays = !w.overlays,
                "frame" => w.frame = !w.frame,
                "cards" => w.cards = !w.cards,
                "colliders" => w.colliders = !w.colliders,
                "bookmark" => {
                    let n = w.bookmarks.len() + 1;
                    let nav = w.nav;
                    w.bookmarks.push(super::Bookmark { name: format!("View {n}"), nav });
                }
                _ => {}
            }
            w.changed();
        }),
        None => {}
    }
    if IconButton::new(Icon::Aperture, "Look through the camera").shortcut("0").active(matches!(ws.look, Look::Camera(_))).compact().show(&mut bui).clicked() {
        super::with(|w| {
            w.look = if matches!(w.look, Look::Camera(_)) { Look::Editor } else { Look::Camera(None) };
            w.changed();
        });
    }
    if IconButton::new(Icon::SquaresFour, "Quad view").active(ws.quad).compact().show(&mut bui).clicked() {
        super::with(|w| {
            w.quad = !w.quad;
            w.changed();
        });
    }
    if IconButton::new(Icon::List, "Outliner").active(outliner_open(&ctx)).compact().show(&mut bui).clicked() {
        let open = outliner_open(&ctx);
        ctx.data_mut(|d| d.insert_temp(egui::Id::unique("viewport-outliner"), !open));
    }
}

// ------------------------------------------------------------ outliner

pub fn outliner_open(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp(egui::Id::unique("viewport-outliner"))).unwrap_or(false)
}

/// The scene tree: lanes as collections (their eye and lock are the
/// lane's own), and in each its clips with a viewport eye and a lock.
fn outliner(app: &mut StudioApp, ui: &mut egui::Ui, body: Rect, objs: &[Object]) {
    let ctx = ui.ctx().clone();
    if !outliner_open(&ctx) {
        return;
    }
    let th = theme::tokens(&ctx);
    let r = Rect::from_min_size(body.left_top() + Vec2::new(8.0, 30.0), Vec2::new(230.0, (body.height() - 40.0).min(460.0)));
    ui.painter().rect(r, 8, theme::alpha(th.panel, 0.94), Stroke::new(1.0, th.line), egui::StrokeKind::Inside);
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r.shrink(6.0)).layout(egui::Layout::top_down(egui::Align::Min)));
    let ws = super::snapshot();
    let seq = app.doc.model.active().clone();
    let time = time_of(app);
    let mut lane_edit: Option<m::Lane> = None;
    egui::ScrollArea::vertical().id_salt("viewport-outliner-scroll").show_v(&mut child, |ui| {
        for lane in seq.lanes.iter().filter(|l| l.kind == m::LaneKind::Visual) {
            let clips: Vec<&m::Clip> = seq.clips.iter().filter(|c| c.lane == lane.id).collect();
            if clips.is_empty() {
                continue;
            }
            ui.horizontal(|ui| {
                if IconButton::new(if lane.hidden { Icon::EyeSlash } else { Icon::Eye }, "Show the lane").compact().show(ui).clicked() {
                    let mut l = lane.clone();
                    l.hidden = !l.hidden;
                    lane_edit = Some(l);
                }
                if IconButton::new(if lane.locked { Icon::LockSimple } else { Icon::LockSimpleOpen }, "Lock the lane").compact().show(ui).clicked() {
                    let mut l = lane.clone();
                    l.locked = !l.locked;
                    lane_edit = Some(l);
                }
                crate::kit::label_trunc(ui, egui::RichText::new(&lane.name).font(theme::body_medium()).color(th.text));
            });
            for c in clips {
                let id = c.id.0;
                let here = time >= c.start && time < c.start + c.duration;
                let kind = objs.iter().find(|o| o.id == id).map(|o| o.kind);
                let selected = app.editor.selection.contains(&(id as u32));
                ui.horizontal(|ui| {
                    ui.add_space(16.0);
                    let hidden = ws.hidden.contains(&id);
                    if IconButton::new(if hidden { Icon::EyeSlash } else { Icon::Eye }, "Show in the viewport").compact().show(ui).clicked() {
                        super::with(|w| {
                            if !w.hidden.remove(&id) {
                                w.hidden.insert(id);
                            }
                            w.changed();
                        });
                    }
                    let locked = ws.locked.contains(&id);
                    if IconButton::new(if locked { Icon::LockSimple } else { Icon::LockSimpleOpen }, "Lock in the viewport").compact().show(ui).clicked() {
                        super::with(|w| {
                            if !w.locked.remove(&id) {
                                w.locked.insert(id);
                            }
                            w.changed();
                        });
                    }
                    let tag = match kind {
                        Some(Kind::Camera) => "camera",
                        Some(Kind::Light) => "light",
                        Some(Kind::Model) => "model",
                        Some(Kind::Layer) => "3D layer",
                        Some(Kind::Card) => "2D",
                        None if !here => "not now",
                        None => "",
                    };
                    let col = if selected { SELECTED } else if here { th.text } else { th.text_2 };
                    // A long name ends in an ellipsis, leaving its tag room.
                    let tag_w = crate::kit::text_width(ui, tag, theme::caption()) + ui.spacing().item_spacing.x;
                    let resp = crate::kit::within(ui, ui.available_width() - tag_w, |ui| ui.add(egui::Label::new(egui::RichText::new(format!("{}  ", c.name)).font(theme::body()).color(col)).sense(Sense::click()).truncate()));
                    ui.label(egui::RichText::new(tag).font(theme::caption()).color(th.text_2));
                    if resp.clicked() {
                        let mods = ui.input(|i| i.modifiers);
                        let s = &mut app.editor.selection;
                        if mods.shift {
                            if let Some(i) = s.iter().position(|x| *x == id as u32) {
                                s.remove(i);
                            } else {
                                s.push(id as u32);
                            }
                        } else {
                            *s = vec![id as u32];
                        }
                    }
                    if resp.double_clicked() {
                        let aspect = body.width() as f64 / body.height().max(1.0) as f64;
                        frame(app, &[id], now(ui.ctx()), aspect);
                    }
                });
            }
        }
    });
    if let Some(l) = lane_edit {
        let seq_id = app.doc.seq();
        app.doc.apply(if l.hidden { "Hide lane" } else { "Show lane" }, m::Edit::SetLane { seq: seq_id, lane: l });
    }
}

#[cfg(test)]
mod tests;
