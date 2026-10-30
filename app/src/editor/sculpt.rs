// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The Sculpt tool: brushes that shape the selected 3D model where you
//! drag on it in the viewer. The model under the pointer is found by a
//! ray from the camera; each dab changes a copy in memory that the viewer
//! shows at once, and letting go saves the new shape as one undo step.

use crate::app::StudioApp;
use crate::icons::Icon;
use crate::kit;
use eframe::egui::{self, Color32, Rect, Sense, Stroke};
use std::sync::Arc;
use studio_mesh::ops::Brush;
use studio_mesh::{Mesh, V3};
use studio_model as m;

/// Brush settings, kept while the app runs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    pub on: bool,
    /// 0 inflate, 1 deflate, 2 grab, 3 smooth, 4 flatten, 5 pinch, 6 crease.
    pub brush: usize,
    /// Canvas pixels.
    pub radius: f32,
    /// 0..1.
    pub strength: f32,
    pub symmetry: bool,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings { on: false, brush: 0, radius: 40.0, strength: 0.5, symmetry: true }
    }
}

/// How the tool works, under its rows while it is on.
const HOW: &str = "Drag on the model in the viewer. [ and ] resize, Esc stops";

pub const BRUSHES: [&str; 7] = ["Inflate", "Deflate", "Grab", "Smooth", "Flatten", "Pinch", "Crease"];

fn settings_id() -> egui::Id {
    egui::Id::new("sculpt-settings")
}

pub fn settings(ctx: &egui::Context) -> Settings {
    ctx.data(|d| d.get_temp(settings_id())).unwrap_or_default()
}

pub fn set_settings(ctx: &egui::Context, s: Settings) {
    ctx.data_mut(|d| d.insert_temp(settings_id(), s));
}

/// A stroke in progress.
#[derive(Clone)]
struct Stroke3 {
    id: u32,
    mesh: Arc<Mesh>,
    /// Model → world and back (rows of 3×4 matrices).
    world: [[f32; 4]; 3],
    inverse: [[f32; 4]; 3],
    camera: studio_render::space::CameraDesc,
    last: Option<V3>,
    dabs: u32,
    /// The world size of one model pixel (brushes are in canvas pixels).
    unit: f32,
}

fn invert(m: &[[f32; 4]; 3]) -> Option<[[f32; 4]; 3]> {
    let a = [[m[0][0], m[0][1], m[0][2]], [m[1][0], m[1][1], m[1][2]], [m[2][0], m[2][1], m[2][2]]];
    let det = a[0][0] * (a[1][1] * a[2][2] - a[1][2] * a[2][1]) - a[0][1] * (a[1][0] * a[2][2] - a[1][2] * a[2][0]) + a[0][2] * (a[1][0] * a[2][1] - a[1][1] * a[2][0]);
    if det.abs() < 1e-12 {
        return None;
    }
    let inv: [[f32; 3]; 3] = std::array::from_fn(|i| {
        std::array::from_fn(|j| {
            let (i1, i2) = ((j + 1) % 3, (j + 2) % 3);
            let (j1, j2) = ((i + 1) % 3, (i + 2) % 3);
            (a[i1][j1] * a[i2][j2] - a[i1][j2] * a[i2][j1]) / det
        })
    });
    let t = [m[0][3], m[1][3], m[2][3]];
    Some(std::array::from_fn(|r| [inv[r][0], inv[r][1], inv[r][2], -(inv[r][0] * t[0] + inv[r][1] * t[1] + inv[r][2] * t[2])]))
}

fn apply(m: &[[f32; 4]; 3], p: V3, w: f32) -> V3 {
    std::array::from_fn(|r| m[r][0] * p[0] + m[r][1] * p[1] + m[r][2] * p[2] + m[r][3] * w)
}

/// The nearest point where a ray meets the mesh (Möller–Trumbore).
fn hit(mesh: &Mesh, origin: V3, dir: V3) -> Option<V3> {
    use studio_mesh::{cross, dot, sub};
    let mut best: Option<f32> = None;
    for t in 0..mesh.triangles() {
        let [a, b, c] = mesh.tri(t).map(|v| mesh.positions[v]);
        let (e1, e2) = (sub(b, a), sub(c, a));
        let p = cross(dir, e2);
        let det = dot(e1, p);
        if det.abs() < 1e-9 {
            continue;
        }
        let inv = 1.0 / det;
        let s = sub(origin, a);
        let u = dot(s, p) * inv;
        if !(0.0..=1.0).contains(&u) {
            continue;
        }
        let q = cross(s, e1);
        let v = dot(dir, q) * inv;
        if v < 0.0 || u + v > 1.0 {
            continue;
        }
        let d = dot(e2, q) * inv;
        if d > 1e-4 && best.is_none_or(|b| d < b) {
            best = Some(d);
        }
    }
    best.map(|d| studio_mesh::add(origin, studio_mesh::scale(dir, d)))
}

/// The selected model clip's place in the world and the camera, at the
/// playhead, as the renderer sees them.
fn placed(app: &StudioApp, id: u32) -> Option<([[f32; 4]; 3], studio_render::space::CameraDesc)> {
    let project = Arc::new(app.doc.model.clone());
    let scene = studio_render::model_scene::ModelScene::for_sequence(project, app.doc.model.active_sequence);
    let list = studio_render::resolve::resolve(&scene, app.editor.playhead);
    for n in &list.nodes {
        if let studio_render::resolve::Node::Layer { clip, .. } = n
            && let studio_render::scene::SourceDesc::Mesh(d) = &clip.source
            && let Some(inst) = d.instances.iter().find(|i| i.id == id as u64)
        {
            return Some((inst.world, d.camera.unwrap_or_else(|| list.camera())));
        }
    }
    None
}

/// The tool over the viewer. Returns whether it has the pointer.
pub fn interact(app: &mut StudioApp, ui: &mut egui::Ui, area: Rect, scale: f32) -> bool {
    let ctx = ui.ctx().clone();
    let mut s = settings(&ctx);
    if !s.on {
        return false;
    }
    let id = match app.editor.selection.as_slice() {
        [id] if app.doc.model.active().clip(m::ClipId(*id as u64)).and_then(crate::mesh_store::file_of).is_some() => *id,
        _ => {
            return false;
        }
    };
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        s.on = false;
        set_settings(&ctx, s);
        return false;
    }
    // [ and ] change the brush size, as in other sculpting tools.
    let (smaller, bigger) = ui.input(|i| (i.key_pressed(egui::Key::OpenBracket), i.key_pressed(egui::Key::CloseBracket)));
    if smaller || bigger {
        s.radius = (s.radius * if bigger { 1.2 } else { 1.0 / 1.2 }).clamp(2.0, 2000.0);
        set_settings(&ctx, s);
    }
    let resp = ui.interact(area, egui::Id::new("sculpt-area"), Sense::click_and_drag()).on_hover_cursor(egui::CursorIcon::Crosshair);
    let canvas = [app.doc.project.width as f32, app.doc.project.height as f32];
    let to_canvas = |p: egui::Pos2| [(p.x - area.min.x) / scale, (p.y - area.min.y) / scale];
    // The brush outline under the pointer.
    if let Some(p) = resp.hover_pos() {
        let t = crate::theme::tokens(&ctx);
        ui.painter().circle_stroke(p, s.radius * scale, Stroke::new(1.5, t.accent));
        ui.painter().circle_filled(p, 2.0, Color32::WHITE);
    }
    let drag_id = egui::Id::new("sculpt-stroke");
    let mut stroke: Option<Stroke3> = ui.data(|d| d.get_temp(drag_id));
    if resp.drag_started() {
        let Some(mc) = app.doc.model.active().clip(m::ClipId(id as u64)).cloned() else { return true };
        match (crate::mesh_store::load(&mc), placed(app, id)) {
            (Ok(mesh), Some((world, camera))) => {
                match invert(&world) {
                    Some(inverse) => {
                        let unit = studio_mesh::length(apply(&world, [1.0, 0.0, 0.0], 0.0)).max(1e-6);
                        stroke = Some(Stroke3 { id, mesh: Arc::new(mesh), world, inverse, camera, last: None, dabs: 0, unit });
                    }
                    None => {
                        kit::push(&ctx, "The model is scaled flat at the playhead, so it can't be sculpted", None, Some(Icon::Warning));
                    }
                }
            }
            (Err(e), _) => {
                kit::push(&ctx, e, None, Some(Icon::Warning));
            }
            (_, None) => {
                kit::push(&ctx, "The model is not on screen at the playhead", None, Some(Icon::Warning));
            }
        }
    }
    if let (Some(st), Some(pos)) = (stroke.as_mut(), resp.interact_pointer_pos().filter(|_| resp.dragged())) {
        let c = to_canvas(pos);
        let near = st.camera.unproject(canvas, c, 1.0);
        let far = st.camera.unproject(canvas, c, 2.0);
        let o = apply(&st.inverse, st.camera.position, 1.0);
        let dir = studio_mesh::normalise(apply(&st.inverse, studio_mesh::sub(far, near), 0.0));
        let radius = s.radius / st.unit;
        let on = hit(&st.mesh, o, dir);
        // Grab follows the pointer on the plane of the first hit.
        let target = match (s.brush, st.last) {
            (2, Some(last)) => {
                let depth = st.camera.view(apply(&st.world, last, 1.0))[2];
                Some(apply(&st.inverse, st.camera.unproject(canvas, c, depth), 1.0))
            }
            _ => on,
        };
        if let Some(p) = target {
            let spacing = radius * 0.25;
            let far_enough = st.last.is_none_or(|l| studio_mesh::length(studio_mesh::sub(p, l)) >= spacing.max(0.5));
            if far_enough {
                let mut mesh = (*st.mesh).clone();
                let k = s.strength.clamp(0.0, 1.0);
                let (brush, amount) = match s.brush {
                    0 => (Brush::Inflate, radius * 0.08 * k),
                    1 => (Brush::Inflate, -radius * 0.08 * k),
                    2 => (Brush::Grab { by: st.last.map_or([0.0; 3], |l| studio_mesh::sub(p, l)) }, 0.0),
                    3 => (Brush::Smooth, k),
                    4 => (Brush::Flatten, k * 0.5),
                    5 => (Brush::Pinch, radius * 0.1 * k),
                    _ => (Brush::Crease, radius * 0.06 * k),
                };
                let centre = if s.brush == 2 { st.last.unwrap_or(p) } else { p };
                if s.brush != 2 || st.last.is_some() {
                    studio_mesh::ops::sculpt(&mut mesh, brush, centre, radius, amount, s.symmetry);
                }
                st.mesh = Arc::new(mesh);
                st.last = Some(p);
                st.dabs += 1;
                let name = format!("memory:sculpt-{}-{}", st.id, st.dabs);
                studio_render::mesh_render::remember(&name, st.mesh.clone());
                let seq = app.doc.seq();
                if let Some(mut mc) = app.doc.model.active().clip(m::ClipId(st.id as u64)).cloned() {
                    crate::mesh_store::set_file(&mut mc, name);
                    app.doc.apply_gesture("sculpt", "Sculpt", m::Edit::SetClip { seq, clip: Box::new(mc) });
                }
            }
        }
    }
    if resp.drag_stopped()
        && let Some(st) = stroke.take()
    {
        if st.dabs > 0 {
            let mut mesh = (*st.mesh).clone();
            mesh.tidy();
            match crate::mesh_store::save(&app.doc, &mesh) {
                Ok(file) => {
                    let seq = app.doc.seq();
                    if let Some(mut mc) = app.doc.model.active().clip(m::ClipId(st.id as u64)).cloned() {
                        crate::mesh_store::set_file(&mut mc, file);
                        app.doc.apply_gesture("sculpt", "Sculpt", m::Edit::SetClip { seq, clip: Box::new(mc) });
                    }
                }
                Err(e) => {
                    kit::push(&ctx, e, None, Some(Icon::Warning));
                }
            }
            app.doc.end_gesture();
        }
        ui.data_mut(|d| d.remove::<Stroke3>(drag_id));
    } else if let Some(st) = stroke {
        ui.data_mut(|d| d.insert_temp(drag_id, st));
    }
    true
}

/// The brush settings, for the Inspector's Model section.
pub fn settings_rows(ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    let mut s = settings(&ctx);
    let before = s;
    kit::labeled_row(ui, "Sculpt", |ui| {
        kit::toggle(ui, &mut s.on, "Sculpt with the pointer", None);
    });
    if s.on {
        kit::labeled_row(ui, "Brush", |ui| {
            if let Some(i) = kit::menu_button(ui, "sculpt-brush", s.brush, &BRUSHES, ui.available_width() - 40.0, None) {
                s.brush = i;
            }
        });
        kit::labeled_row(ui, "Size", |ui| {
            kit::Slider::new("Size", &mut s.radius, 2.0..=600.0).default(40.0).unit("px").logarithmic(true).with_value().show(ui);
        });
        kit::labeled_row(ui, "Strength", |ui| {
            let mut pct = s.strength * 100.0;
            if kit::Slider::new("Strength", &mut pct, 1.0..=100.0).default(50.0).unit("%").with_value().show(ui).changed() {
                s.strength = pct / 100.0;
            }
        });
        kit::labeled_row(ui, "Symmetry", |ui| {
            kit::toggle(ui, &mut s.symmetry, "Mirror across X", None);
        });
        let t = crate::theme::tokens(&ctx);
        ui.label(egui::RichText::new(crate::i18n::shown(HOW)).font(crate::theme::caption()).color(t.text_2));
    }
    if s != before {
        set_settings(&ctx, s);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ray_finds_the_front_of_a_ball_and_matrices_invert() {
        let ball = studio_mesh::prim::ico_sphere(100.0, 3);
        let p = hit(&ball, [0.0, 0.0, -500.0], [0.0, 0.0, 1.0]).unwrap();
        assert!((p[2] + 100.0).abs() < 3.0, "{p:?}");
        let m = [[0.0, -2.0, 0.0, 10.0], [1.0, 0.0, 0.0, 20.0], [0.0, 0.0, 3.0, 30.0]];
        let inv = invert(&m).unwrap();
        let q = apply(&inv, apply(&m, [1.0, 2.0, 3.0], 1.0), 1.0);
        assert!((q[0] - 1.0).abs() < 1e-4 && (q[1] - 2.0).abs() < 1e-4 && (q[2] - 3.0).abs() < 1e-4, "{q:?}");
    }

    /// The brush rows at the Inspector's width: one label column, each
    /// value shown, nothing cut, and a line on how to use it.
    #[test]
    fn the_brush_rows_read_cleanly() {
        let _serial = crate::ui_tests::serial();
        let (ctx, _app) = crate::editor::physics::cloud_ui_check::editor();
        set_settings(&ctx, Settings { on: true, ..Default::default() });
        let drawn = crate::editor::physics::cloud_ui_check::draw(&ctx, 3, settings_rows);
        drawn.assert_tidy("sculpt", &[]);
        assert!(drawn.shows(HOW) && drawn.shows("Symmetry"));
        // Labels share one column.
        let xs: Vec<f32> = ["Sculpt", "Brush", "Size", "Strength", "Symmetry"].iter().map(|l| drawn.rect_of(l).unwrap_or_else(|| panic!("{l}")).left()).collect();
        assert!(xs.iter().all(|x| (x - xs[0]).abs() < 0.5), "{xs:?}");
    }
}
