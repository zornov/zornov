// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! 3D in the editor: the 3D gizmo on a selected 3D layer (arrows that move
//! it along its own axes, rings that turn it about X, Y and Z), the camera
//! tools on the viewer (orbit, pan, dolly, the scroll wheel dollies),
//! adding cameras and lights, and the Inspector's 3D, Camera and Light
//! sections.
//!
//! Geometry is the model's (`studio_model::space`), the same the renderer
//! draws with, so a ring turns exactly the picture it is drawn round.
//! Dragging a property that has keyframes keys it at the playhead, as the
//! 2D handles do.

use crate::app::StudioApp;
use crate::icons::Icon;
use crate::kit::{self, FieldRow, IconButton, MenuItem, Section};
use crate::theme;
use crate::view::{Clip, SEC, Time};
use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, Vec2};
use studio_model as m;
use studio_model::space as sp;

mod air;

// ------------------------------------------------------------ camera tools

/// What a drag on the viewer does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CamTool {
    /// Layers are selected and moved.
    #[default]
    Off,
    /// Swings the camera round what it looks at.
    Orbit,
    /// Slides the camera (and what it looks at) across the view.
    Pan,
    /// Moves the camera in and out along its view.
    Dolly,
}

fn tool_id() -> egui::Id {
    egui::Id::unique("camera-tool")
}

pub fn tool(ctx: &egui::Context) -> CamTool {
    ctx.data(|d| d.get_temp(tool_id())).unwrap_or_default()
}

pub fn set_tool(ctx: &egui::Context, t: CamTool) {
    ctx.data_mut(|d| d.insert_temp(tool_id(), t));
}

fn frame_of(app: &StudioApp) -> Time {
    (SEC as f64 / (app.doc.project.fps as f64).max(1.0)) as Time
}

/// A new camera or light on a lane of its own at the top, over the whole
/// sequence; selected.
pub fn add_rig(app: &mut StudioApp, light: Option<sp::LightKind>) -> Option<u32> {
    let w = app.doc.project.width as f64;
    let seq = app.doc.seq();
    let lane = app.doc.alloc_id();
    let name = match light {
        None => "Camera",
        Some(sp::LightKind::Spot) => "Spot light",
        Some(sp::LightKind::Point) => "Point light",
        Some(sp::LightKind::Parallel) => "Sun light",
        Some(sp::LightKind::Ambient) => "Ambient light",
    };
    let mut edits = vec![m::Edit::AddLane { seq, lane: m::Lane::new(m::LaneId(lane), m::LaneKind::Visual, name), index: Some(0) }];
    let id = app.doc.alloc_id();
    let (kind, params, pos) = match light {
        None => {
            let mut p = sp::CameraSettings::defaults(w);
            p.insert("two_node".into(), m::Value::Bool(true));
            (m::GeneratorKind::Camera, p, [0.0, 0.0, -sp::default_zoom(w)])
        }
        Some(k) => (m::GeneratorKind::Light, sp::LightSettings::defaults(k), if k == sp::LightKind::Ambient { [0.0; 3] } else { [-0.3 * w, -0.3 * w, -0.5 * w] }),
    };
    let mut g = m::Generator::new(kind);
    g.params = params;
    let dur = app.doc.project.duration().max(SEC);
    let mut c = m::Clip::new(m::ClipId(id), m::LaneId(lane), 0, dur, m::ClipSource::Generator(g));
    c.name = name.into();
    c.transform.position = m::Vec2::new(pos[0], pos[1]);
    c.transform.position_z = pos[2];
    edits.push(m::Edit::InsertClip { seq, clip: Box::new(c) });
    if !app.doc.apply(&crate::i18n::tf(format!("Add {}", name.to_lowercase())), m::Edit::Batch { edits }) {
        return None;
    }
    app.editor.selection = vec![id as u32];
    Some(id as u32)
}

/// A particle layer (rain, snow, leaves…) by preset, at the playhead, on
/// a new top lane: a clear full-frame layer carrying the `particles`
/// generator, so its settings are in the Inspector's Effects.
pub fn add_particles(app: &mut StudioApp, preset: &studio_fx::particles::Preset) -> Option<u32> {
    let seq = app.doc.seq();
    let lane = app.doc.alloc_id();
    let mut edits = vec![m::Edit::AddLane { seq, lane: m::Lane::new(m::LaneId(lane), m::LaneKind::Visual, preset.name), index: Some(0) }];
    let id = app.doc.alloc_id();
    let mut g = m::Generator::new(m::GeneratorKind::Solid);
    g.params.insert("colour".into(), m::Value::Colour(m::Rgba { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }));
    let at = app.editor.playhead.clamp(0, app.doc.project.duration().max(0));
    let dur = (app.doc.project.duration() - at).max(10 * SEC);
    let mut c = m::Clip::new(m::ClipId(id), m::LaneId(lane), at, dur, m::ClipSource::Generator(g));
    c.name = format!("{} (particles)", preset.name);
    let mut e = m::Effect::new(m::EffectId(app.doc.alloc_id()), "particles");
    e.params.insert("preset".into(), m::Value::Choice(preset.name.into()));
    c.effects.push(e);
    if preset.glow {
        c.blend = m::BlendMode::Screen;
    }
    edits.push(m::Edit::InsertClip { seq, clip: Box::new(c) });
    if !app.doc.apply(&crate::i18n::tf(format!("Add {}", preset.name.to_lowercase())), m::Edit::Batch { edits }) {
        return None;
    }
    app.editor.selection = vec![id as u32];
    Some(id as u32)
}

/// Changes a model's shape or surface: a new model file, one undo step.
fn mesh_change(app: &mut StudioApp, ctx: &egui::Context, id: u32, label: &str, f: impl FnOnce(&mut studio_mesh::Mesh) -> Result<(), String>) {
    let Some(mc) = app.doc.model.active().clip(m::ClipId(id as u64)).cloned() else { return };
    let r = crate::mesh_store::load(&mc).and_then(|mut mesh| {
        f(&mut mesh)?;
        mesh.tidy();
        crate::mesh_store::save(&app.doc, &mesh)
    });
    match r {
        Ok(file) => change(app, &[id], label, |c| crate::mesh_store::set_file(c, file.clone())),
        Err(e) => {
            kit::push(ctx, e, None, Some(Icon::Warning));
        }
    }
}

/// The camera button on the viewer's toolbar: the camera tools, and
/// adding cameras and lights.
pub fn toolbar_buttons(app: &mut StudioApp, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    let id = egui::Id::unique("viewer-3d-menu");
    let cur = tool(&ctx);
    let resp = IconButton::new(Icon::Aperture, "3D camera and lights").active(cur != CamTool::Off || kit::is_open(&ctx, id)).show(ui);
    if resp.clicked() {
        kit::toggle_open(&ctx, id);
    }
    let mut add: Option<Option<sp::LightKind>> = None;
    let mut shape: Option<&'static str> = None;
    let mut from_file = false;
    let mut particles: Option<&'static studio_fx::particles::Preset> = None;
    let mut asked: Option<(String, serde_json::Value)> = None;
    let sel = app.editor.selection.clone();
    kit::popover(&ctx, id, resp.rect, kit::Side::Below, 220.0, |ui| {
        // Shots on the selected layers, and lighting set-ups: the same
        // ops the AI editor uses, run on this window's project.
        if !sel.is_empty() {
            kit::menu_heading(ui, "Shot on the selection");
            for (label, size, mv) in [("Wide shot", "wide", ""), ("Medium shot", "medium", ""), ("Close-up", "close_up", ""), ("Slow push in", "medium", "slow push in"), ("Slow orbit", "medium_wide", "slow orbit"), ("Crane up", "wide", "crane up"), ("Handheld", "medium", "handheld")] {
                if MenuItem::new(label).show(ui).clicked() {
                    let mut shot = serde_json::json!({"op": "shot", "on": sel, "size": size});
                    if !mv.is_empty() {
                        shot["move"] = serde_json::json!(mv);
                    }
                    asked = Some((label.to_owned(), serde_json::json!({"ops": [{"op": "three_d", "clips": sel}, shot]})));
                    kit::set_open(&ctx, id, false);
                }
            }
        }
        kit::menu_heading(ui, "Lighting");
        for (label, preset) in [("Three-point", "three_point"), ("Low key (noir)", "low_key"), ("High key", "high_key"), ("Golden hour", "golden_hour"), ("Moonlight", "moonlight"), ("From the backdrop", "motivated")] {
            if MenuItem::new(label).show(ui).clicked() {
                let mut l = serde_json::json!({"op": "lighting", "preset": preset});
                if let Some(s) = sel.first() {
                    l["on"] = serde_json::json!(s);
                }
                asked = Some((format!("{label} lighting"), serde_json::json!({"ops": [l]})));
                kit::set_open(&ctx, id, false);
            }
        }
        kit::menu_heading(ui, "Drag in the viewer to");
        for (t, label) in [(CamTool::Off, "Select and move layers"), (CamTool::Orbit, "Orbit the camera"), (CamTool::Pan, "Pan the camera"), (CamTool::Dolly, "Dolly the camera")] {
            if MenuItem::new(label).checked(cur == t).show(ui).clicked() {
                set_tool(&ctx, t);
                kit::set_open(&ctx, id, false);
            }
        }
        kit::menu_heading(ui, "Add");
        for (label, what) in [("Camera", None), ("Spot light", Some(sp::LightKind::Spot)), ("Point light", Some(sp::LightKind::Point)), ("Sun light", Some(sp::LightKind::Parallel)), ("Ambient light", Some(sp::LightKind::Ambient))] {
            if MenuItem::new(label).show(ui).clicked() {
                add = Some(what);
                kit::set_open(&ctx, id, false);
            }
        }
        kit::menu_heading(ui, "3D model");
        for (label, what) in [("Sphere", "sphere"), ("Sculpting sphere", "ico_sphere"), ("Cube", "cube"), ("Cylinder", "cylinder"), ("Cone", "cone"), ("Torus", "torus"), ("Capsule", "capsule"), ("Plane", "plane"), ("Human head", "head")] {
            if MenuItem::new(label).show(ui).clicked() {
                shape = Some(what);
                kit::set_open(&ctx, id, false);
            }
        }
        if MenuItem::new("From a file…").show(ui).clicked() {
            from_file = true;
            kit::set_open(&ctx, id, false);
        }
        kit::menu_heading(ui, "Particles");
        for p in studio_fx::particles::PRESETS {
            if MenuItem::new(p.name).show(ui).clicked() {
                particles = Some(p);
                kit::set_open(&ctx, id, false);
            }
        }
    });
    if let Some((label, args)) = asked {
        let (tx, rx) = std::sync::mpsc::channel();
        let a = app.agent.get_or_insert_with(|| super::agent_panel::AgentUi::new(&ctx));
        a.queue_call(crate::agent::live::ToolCall { tool: "edit".into(), args, reply: tx, epoch: 0 });
        if let Ok(mut p) = ASKED.lock() {
            p.push((label, rx));
        }
    }
    // What the shot or lighting did, when it is done.
    if let Ok(mut p) = ASKED.lock() {
        p.retain(|(label, rx)| match rx.try_recv() {
            Ok(r) => {
                let said = match r {
                    Ok(parts) => parts.iter().find_map(|c| if let crate::agent::Content::Text(t) = c { Some(t.clone()) } else { None }).map(|t| serde_json::from_str::<serde_json::Value>(&t).ok().and_then(|v| v["notes"].as_array().and_then(|n| n.last()).and_then(|n| n.as_str()).map(str::to_owned)).unwrap_or(t)).unwrap_or_default(),
                    Err(e) => e,
                };
                let short: String = said.chars().take(160).collect();
                kit::push(&ctx, format!("{label}: {short}"), None, Some(Icon::Aperture));
                false
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => true,
            Err(_) => false,
        });
    }
    if let Some(p) = particles
        && add_particles(app, p).is_some()
    {
        kit::push(&ctx, format!("{} added: change it under Effects in the Inspector", p.name), None, Some(Icon::Sparkle));
    }
    if let Some(s) = shape {
        let size = app.doc.project.width.min(app.doc.project.height) as f32 * 0.4;
        let made = if s == "head" { Some(studio_mesh::sdf::head(size * 1.2, 4)) } else { studio_mesh::prim::by_name(s, size, 4) };
        if let Some(mesh) = made {
            let name = match s {
                "ico_sphere" => "Sculpting sphere".to_owned(),
                "head" => "Head".to_owned(),
                s => format!("{}{}", s[..1].to_uppercase(), &s[1..]),
            };
            if let Err(e) = crate::mesh_store::add_model(app, &mesh, &name) {
                kit::push(&ctx, e, None, Some(Icon::Warning));
            }
        }
    }
    if from_file && let Some(paths) = rfd::FileDialog::new().set_title("Add a 3D model").add_filter("3D models", &crate::mesh_store::model_extensions()).pick_files() {
        crate::mesh_store::import_models(app, &ctx, &paths);
    }
    if let Some(what) = add
        && add_rig(app, what).is_some()
    {
        let has_3d = app.doc.model.active().clips.iter().any(|c| c.transform.three_d);
        let note = if has_3d { "Added; it shows on the 3D layers" } else { "Added; switch layers to 3D in the Inspector to see them through it" };
        kit::push(&ctx, note, None, Some(Icon::Aperture));
    }
}

/// Shots and lighting asked for from the menu, waiting for their result.
static ASKED: std::sync::Mutex<Vec<(String, std::sync::mpsc::Receiver<crate::agent::live::ToolResult>)>> = std::sync::Mutex::new(Vec::new());

/// A camera's moving parts at one moment.
#[derive(Clone, Copy, Debug)]
struct Rig {
    pos: [f64; 3],
    poi: [f64; 3],
    two_node: bool,
    zoom: f64,
    roll: f64,
}

fn rig_of(mc: &m::Clip, local: Time, width: f64) -> Rig {
    let num = |p: &str, d: f64| mc.property_at(p, local).and_then(|v| v.as_f64()).unwrap_or(d);
    let pt = |p: &str| match mc.property_at(p, local) {
        Some(m::Value::Point(q)) => [q.x, q.y],
        _ => [0.0, 0.0],
    };
    let s = sp::CameraSettings::read(&mc.generator_params_at(local), width);
    let (a, b) = (pt("transform.position"), pt("generator.poi"));
    Rig { pos: [a[0], a[1], num("transform.position_z", 0.0)], poi: [b[0], b[1], num("generator.poi_z", 0.0)], two_node: s.two_node, zoom: s.zoom, roll: num("transform.rotation", 0.0) }
}

fn v_add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn v_sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn v_mul(a: [f64; 3], k: f64) -> [f64; 3] {
    [a[0] * k, a[1] * k, a[2] * k]
}
fn v_len(a: [f64; 3]) -> f64 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
}

/// `p` turned `deg` about `axis` through `centre` (Rodrigues).
fn turn(p: [f64; 3], centre: [f64; 3], axis: [f64; 3], deg: f64) -> [f64; 3] {
    let k = v_mul(axis, 1.0 / v_len(axis).max(1e-12));
    let v = v_sub(p, centre);
    let (s, c) = deg.to_radians().sin_cos();
    let kxv = [k[1] * v[2] - k[2] * v[1], k[2] * v[0] - k[0] * v[2], k[0] * v[1] - k[1] * v[0]];
    let kv = k[0] * v[0] + k[1] * v[1] + k[2] * v[2];
    v_add(centre, v_add(v_add(v_mul(v, c), v_mul(kxv, s)), v_mul(k, kv * (1.0 - c))))
}

#[derive(Clone, Copy, Debug)]
struct CamDrag {
    id: u32,
    start: Pos2,
    rig: Rig,
    local: Time,
}

/// The camera tools on the viewer. Returns whether a tool has the pointer
/// (the layer handles then stay out of the way).
pub fn camera_interact(app: &mut StudioApp, ui: &mut egui::Ui, area: Rect, scale: f32) -> bool {
    let ctx = ui.ctx().clone();
    let t = tool(&ctx);
    if t == CamTool::Off {
        return false;
    }
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        set_tool(&ctx, CamTool::Off);
        return false;
    }
    let w = app.doc.project.width as f64;
    let resp = ui.interact(area, egui::Id::unique("camera-tool-area"), Sense::click_and_drag()).on_hover_cursor(match t {
        CamTool::Pan => egui::CursorIcon::Grab,
        CamTool::Dolly => egui::CursorIcon::ResizeVertical,
        _ => egui::CursorIcon::AllScroll,
    });
    let drag_id = egui::Id::unique("camera-tool-drag");
    let mut drag: Option<CamDrag> = ui.data(|d| d.get_temp(drag_id));
    let playhead = app.editor.playhead;
    let camera_now = |app: &StudioApp| sp::active_camera(app.doc.model.active(), playhead).map(|c| c.id.0 as u32);
    if resp.drag_started() {
        let id = match camera_now(app) {
            Some(id) => Some(id),
            None => {
                let made = add_rig(app, None);
                if made.is_some() {
                    kit::push(&ctx, "Added a camera to move", None, Some(Icon::Aperture));
                }
                made
            }
        };
        if let (Some(id), Some(start)) = (id, ui.input(|i| i.pointer.press_origin())) {
            if let (Some(mc), Some(vc)) = (app.doc.model.active().clip(m::ClipId(id as u64)), app.doc.project.clip(id)) {
                let local = (playhead - vc.start).clamp(0, vc.dur);
                drag = Some(CamDrag { id, start, rig: rig_of(mc, local, w), local });
            }
        }
    }
    let scroll = if resp.hovered() { ui.input(|i| i.smooth_scroll_delta.y) } else { 0.0 };
    let mut moved: Option<(u32, Rig, Time, &str)> = None;
    if let (Some(d), Some(pos)) = (drag, ui.input(|i| i.pointer.interact_pos())) {
        let delta = (pos - d.start) / scale;
        let r0 = d.rig;
        let dist = v_len(v_sub(r0.poi, r0.pos)).max(1.0);
        let axes = if r0.two_node { sp::look_at(r0.pos, r0.poi, r0.roll) } else { sp::look_at(r0.pos, v_add(r0.pos, v_mul(fwd_of(app, d.id, d.local), dist)), r0.roll) };
        let mut r = r0;
        match t {
            CamTool::Orbit => {
                // A one-node camera orbits the point it looks at, a lens's
                // distance ahead; it becomes aimed at it.
                let centre = if r0.two_node { r0.poi } else { v_add(r0.pos, v_mul(axes[2], r0.zoom)) };
                let p = turn(r0.pos, centre, [0.0, 1.0, 0.0], -(delta.x as f64) * 0.3);
                let side = turn(axes[0], [0.0; 3], [0.0, 1.0, 0.0], -(delta.x as f64) * 0.3);
                r.pos = turn(p, centre, side, delta.y as f64 * 0.3);
                r.poi = centre;
                r.two_node = true;
            }
            CamTool::Pan => {
                let k = dist / r0.zoom;
                let shift = v_add(v_mul(axes[0], -(delta.x as f64) * k), v_mul(axes[1], -(delta.y as f64) * k));
                r.pos = v_add(r0.pos, shift);
                r.poi = v_add(r0.poi, shift);
            }
            CamTool::Dolly => {
                let step = (-(delta.y as f64) * dist / 300.0).min(dist - 10.0);
                r.pos = v_add(r0.pos, v_mul(axes[2], step));
            }
            CamTool::Off => {}
        }
        moved = Some((d.id, r, d.local, match t {
            CamTool::Orbit => "Orbit camera",
            CamTool::Pan => "Pan camera",
            _ => "Dolly camera",
        }));
        let readout = match t {
            CamTool::Dolly => format!("{:.0} px to the target", v_len(v_sub(r.poi, r.pos))),
            _ => format!("camera {:.0}, {:.0}, {:.0}", r.pos[0], r.pos[1], r.pos[2]),
        };
        let lp = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::unique("camera-tool-readout")));
        let th = theme::tokens(ui.ctx());
        let g = lp.layout_no_wrap(readout, theme::body_medium(), th.text);
        let br = Rect::from_min_size(pos + Vec2::new(14.0, 14.0), g.size() + Vec2::new(12.0, 6.0));
        lp.rect(br, 6, th.panel, Stroke::new(1.0, th.line), egui::StrokeKind::Inside);
        lp.galley(br.min + Vec2::new(6.0, 3.0), g, th.text);
    } else if scroll.abs() > 0.5
        && let Some(id) = camera_now(app)
        && let (Some(mc), Some(vc)) = (app.doc.model.active().clip(m::ClipId(id as u64)), app.doc.project.clip(id))
    {
        // The wheel dollies whatever the tool.
        let local = (playhead - vc.start).clamp(0, vc.dur);
        let r0 = rig_of(mc, local, w);
        let dist = v_len(v_sub(r0.poi, r0.pos)).max(1.0);
        let fwd = if r0.two_node { v_mul(v_sub(r0.poi, r0.pos), 1.0 / dist) } else { fwd_of(app, id, local) };
        let step = (scroll as f64 * dist / 400.0).min(dist - 10.0);
        moved = Some((id, Rig { pos: v_add(r0.pos, v_mul(fwd, step)), ..r0 }, local, "Dolly camera"));
    }
    if let Some((id, r, local, label)) = moved
        && let Some(mut mc) = app.doc.model.active().clip(m::ClipId(id as u64)).cloned()
    {
        let frame = frame_of(app);
        super::xform::put(&mut mc, "transform.position", m::Value::Point(m::Vec2::new(r.pos[0], r.pos[1])), local, frame);
        super::xform::put(&mut mc, "transform.position_z", m::Value::Scalar(r.pos[2]), local, frame);
        if r.two_node {
            if let m::ClipSource::Generator(g) = &mut mc.source {
                g.params.insert("two_node".into(), m::Value::Bool(true));
            }
            super::xform::put(&mut mc, "generator.poi", m::Value::Point(m::Vec2::new(r.poi[0], r.poi[1])), local, frame);
            super::xform::put(&mut mc, "generator.poi_z", m::Value::Scalar(r.poi[2]), local, frame);
        }
        let seq = app.doc.seq();
        app.doc.apply_gesture(&format!("camera-tool-{id}"), label, m::Edit::SetClip { seq, clip: Box::new(mc) });
    }
    if ui.input(|i| i.pointer.any_released()) {
        ui.data_mut(|d| d.remove::<CamDrag>(drag_id));
    } else if let Some(d) = drag {
        ui.data_mut(|m| m.insert_temp(drag_id, d));
    }
    true
}

/// Where a camera faces at `local` (unit).
fn fwd_of(app: &StudioApp, id: u32, local: Time) -> [f64; 3] {
    let seq = app.doc.model.active();
    let Some(mc) = seq.clip(m::ClipId(id as u64)) else { return [0.0, 0.0, 1.0] };
    sp::camera_view(seq, mc, mc.start + local, app.doc.project.width as f64).axes[2]
}

// -------------------------------------------------------------- the gizmo

/// What part of the gizmo is held.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Part {
    /// Move along the layer's own x, y or z axis.
    Arrow(usize),
    /// Turn about X, Y or Z (the rotation properties).
    Ring(usize),
}

#[derive(Clone, Copy, Debug)]
struct GizmoDrag {
    id: u32,
    part: Part,
    start: Pos2,
    local: Time,
    /// The layer's own (unparented) values when the drag began: position
    /// x, y, z and rotation x, y, z.
    own: [f64; 6],
    /// World units per screen pixel along the held axis (arrows), or
    /// degrees per pixel along the ring's screen direction.
    rate: Vec2,
    /// A ring whose axis points at the viewer turns with the pointer's
    /// angle round the anchor: the sign of that turn.
    angular: Option<f32>,
}

const AXIS_COLOURS: [Color32; 3] = [Color32::from_rgb(239, 83, 80), Color32::from_rgb(102, 187, 106), Color32::from_rgb(66, 165, 245)];

fn seg_dist(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_sq().max(1e-6)).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

/// The 3D gizmo of the selected layer when it is in 3D. Call after the
/// layer's other handles are made, so it is on top. Returns whether it
/// has the pointer.
pub fn gizmo(app: &mut StudioApp, ui: &mut egui::Ui, canvas: Rect, scale: f32, id: u32, now: Time) -> bool {
    let seq = app.doc.model.active();
    let Some(mc) = seq.clip(m::ClipId(id as u64)) else { return false };
    if !mc.transform.three_d {
        return false;
    }
    let Some(vc) = app.doc.project.clip(id).cloned() else { return false };
    let w = app.doc.project.width as f64;
    let tr = sp::oriented_transform(seq, mc, now, w);
    let view = sp::view_at(seq, now, w);
    let anchor = [tr.position.x, tr.position.y, tr.position_z];
    let screen = |p: [f64; 3]| view.project(p).map(|(c, _)| canvas.center() + Vec2::new(c.x as f32, c.y as f32) * scale);
    let Some(a) = screen(anchor) else { return false };
    let depth = view.view(anchor)[2].max(1.0);
    // About 70 screen pixels long wherever it is.
    let len = 70.0 / scale as f64 * depth / view.zoom;
    let o = sp::mul(&sp::mul(&sp::rot_x(tr.orientation[0]), &sp::rot_y(tr.orientation[1])), &sp::rot_z(tr.orientation[2]));
    let full = sp::rotation(tr.orientation, tr.rotation_x, tr.rotation_y, tr.rotation);
    let col = |m: &sp::Mat3, c: usize| [m[0][c], m[1][c], m[2][c]];
    let own_axes = [col(&full, 0), col(&full, 1), col(&full, 2)];
    // What each rotation property turns about.
    let ring_axes = [col(&o, 0), sp::apply(&sp::mul(&o, &sp::rot_x(tr.rotation_x)), [0.0, 1.0, 0.0]), sp::apply(&sp::mul(&o, &sp::mul(&sp::rot_x(tr.rotation_x), &sp::rot_y(tr.rotation_y))), [0.0, 0.0, 1.0])];
    let ring = |i: usize| -> Vec<Option<Pos2>> {
        let ax = ring_axes[i];
        let any = if ax[1].abs() < 0.9 { [0.0, 1.0, 0.0] } else { [1.0, 0.0, 0.0] };
        let u = {
            let c = [ax[1] * any[2] - ax[2] * any[1], ax[2] * any[0] - ax[0] * any[2], ax[0] * any[1] - ax[1] * any[0]];
            v_mul(c, 1.0 / v_len(c).max(1e-12))
        };
        let v = [ax[1] * u[2] - ax[2] * u[1], ax[2] * u[0] - ax[0] * u[2], ax[0] * u[1] - ax[1] * u[0]];
        (0..=64).map(|k| {
            let th = k as f64 / 64.0 * std::f64::consts::TAU;
            screen(v_add(anchor, v_add(v_mul(u, len * 0.85 * th.cos()), v_mul(v, len * 0.85 * th.sin()))))
        }).collect()
    };
    let rings: Vec<Vec<Option<Pos2>>> = (0..3).map(ring).collect();
    let tips: Vec<Option<Pos2>> = own_axes.iter().map(|ax| screen(v_add(anchor, v_mul(*ax, len)))).collect();

    let drag_id = egui::Id::unique("gizmo-3d-drag");
    let mut drag: Option<GizmoDrag> = ui.data(|d| d.get_temp(drag_id)).filter(|d: &GizmoDrag| d.id == id);
    let pointer = ui.input(|i| i.pointer.hover_pos());
    // The part under the pointer: arrows first, then rings; never right
    // on the anchor (that is the anchor handle's).
    let near = pointer.filter(|p| p.distance(a) > 9.0).and_then(|p| {
        let mut best: Option<(f32, Part)> = None;
        for (i, tip) in tips.iter().enumerate() {
            if let Some(tip) = tip {
                let d = seg_dist(p, a, *tip);
                if d < 7.0 && best.is_none_or(|b| d < b.0) {
                    best = Some((d, Part::Arrow(i)));
                }
            }
        }
        if best.is_none() {
            for (i, r) in rings.iter().enumerate() {
                for s in r.windows(2) {
                    if let (Some(x), Some(y)) = (s[0], s[1]) {
                        let d = seg_dist(p, x, y);
                        if d < 6.0 && best.is_none_or(|b| d < b.0) {
                            best = Some((d, Part::Ring(i)));
                        }
                    }
                }
            }
        }
        best.map(|b| b.1)
    });
    let hot = drag.map(|d| d.part).or(near);

    // Draw.
    let p = ui.painter().with_clip_rect(canvas.expand(40.0));
    for (i, r) in rings.iter().enumerate() {
        let on = hot == Some(Part::Ring(i));
        let c = theme::alpha(AXIS_COLOURS[i], if on { 1.0 } else { 0.55 });
        let pts: Vec<Pos2> = r.iter().flatten().copied().collect();
        if pts.len() == r.len() {
            p.add(egui::Shape::line(pts, Stroke::new(if on { 2.5 } else { 1.5 }, c)));
        }
    }
    for (i, tip) in tips.iter().enumerate() {
        let Some(tip) = *tip else { continue };
        let on = hot == Some(Part::Arrow(i));
        let c = AXIS_COLOURS[i];
        p.line_segment([a, tip], Stroke::new(if on { 3.0 } else { 2.0 }, c));
        let dir = (tip - a).normalized();
        if dir.is_finite() {
            let side = dir.rot90() * 4.5;
            p.add(egui::Shape::convex_polygon(vec![tip + dir * 9.0, tip + side, tip - side], c, Stroke::NONE));
        }
        p.text(tip + dir * 16.0, egui::Align2::CENTER_CENTER, ["X", "Y", "Z"][i], theme::caption(), c);
    }

    // Grab: a small hit area follows the pointer while it is over a part,
    // made after the other handles so it wins.
    let mut taken = false;
    if let Some(pos) = pointer.filter(|_| hot.is_some()) {
        let resp = ui.interact(Rect::from_center_size(pos, Vec2::splat(18.0)), egui::Id::unique("gizmo-3d-grab"), Sense::click_and_drag()).on_hover_cursor(match hot {
            Some(Part::Ring(_)) => egui::CursorIcon::Alias,
            _ => egui::CursorIcon::Move,
        });
        taken = true;
        if resp.drag_started()
            && let Some(part) = near
        {
            let start = ui.input(|i| i.pointer.press_origin()).unwrap_or(pos);
            let local = (app.editor.playhead - vc.start).clamp(0, vc.dur);
            let own = mc.transform_at(local);
            let rate = match part {
                Part::Arrow(i) => {
                    // Screen pixels per world unit along the axis.
                    let s = tips[i].map_or(Vec2::X, |tip| (tip - a) / len as f32);
                    s / s.length_sq().max(1e-9)
                }
                Part::Ring(i) => {
                    // Which way a degree more moves the ring's point that
                    // moves most on screen (the grabbed one may be seen
                    // edge-on).
                    let ax = ring_axes[i];
                    let look = v_sub(anchor, view.position);
                    let side = [ax[1] * look[2] - ax[2] * look[1], ax[2] * look[0] - ax[0] * look[2], ax[0] * look[1] - ax[1] * look[0]];
                    let at = v_add(anchor, v_mul(side, len / v_len(side).max(1e-12)));
                    let moved = turn(at, anchor, ax, 1.0);
                    match (screen(at), screen(moved)) {
                        (Some(x), Some(y)) if (y - x).length() > 0.05 => (y - x) / (y - x).length_sq(),
                        _ => Vec2::X,
                    }
                }
            };
            // An axis pointing at the viewer: its ring is a circle on
            // screen, turned by going round it.
            let angular = match part {
                Part::Ring(i) => {
                    let ax = ring_axes[i];
                    let look = v_sub(anchor, view.position);
                    let along = (ax[0] * look[0] + ax[1] * look[1] + ax[2] * look[2]) / v_len(look).max(1e-12);
                    (along.abs() > 0.8).then(|| {
                        // Which way round a positive turn goes on screen.
                        let any = if ax[1].abs() < 0.9 { [0.0, 1.0, 0.0] } else { [1.0, 0.0, 0.0] };
                        let u = [ax[1] * any[2] - ax[2] * any[1], ax[2] * any[0] - ax[0] * any[2], ax[0] * any[1] - ax[1] * any[0]];
                        let p0 = v_add(anchor, v_mul(u, len / v_len(u).max(1e-12)));
                        match (screen(p0), screen(turn(p0, anchor, ax, 5.0))) {
                            (Some(x), Some(y)) => (x - a).x * (y - a).y - (x - a).y * (y - a).x,
                            _ => 1.0,
                        }
                        .signum()
                    })
                }
                Part::Arrow(_) => None,
            };
            drag = Some(GizmoDrag { id, part, start, local, own: [own.position.x, own.position.y, own.position_z, own.rotation_x, own.rotation_y, own.rotation], rate, angular });
        }
    }
    if let (Some(d), Some(pos)) = (drag, ui.input(|i| i.pointer.interact_pos())) {
        taken = true;
        let k = match d.angular {
            Some(sign) => {
                let (a0, a1) = ((d.start - a).angle(), (pos - a).angle());
                let mut deg = (a1 - a0).to_degrees();
                deg -= (deg / 360.0).round() * 360.0;
                (deg * sign) as f64
            }
            None => (pos - d.start).dot(d.rate) as f64,
        };
        let shift = ui.input(|i| i.modifiers.shift);
        if let Some(mut mc) = app.doc.model.active().clip(m::ClipId(id as u64)).cloned() {
            let frame = frame_of(app);
            let label = match d.part {
                Part::Arrow(i) => {
                    let step = if shift { (k / 10.0).round() * 10.0 } else { k };
                    let ax = own_axes[i];
                    let np = [d.own[0] + ax[0] * step, d.own[1] + ax[1] * step, d.own[2] + ax[2] * step];
                    super::xform::put(&mut mc, "transform.position", m::Value::Point(m::Vec2::new(np[0], np[1])), d.local, frame);
                    super::xform::put(&mut mc, "transform.position_z", m::Value::Scalar(np[2]), d.local, frame);
                    show_readout(ui, pos, &format!("{:.0}, {:.0}, {:.0}", np[0], np[1], np[2]));
                    "Move in 3D"
                }
                Part::Ring(i) => {
                    let deg = if shift { (k / 15.0).round() * 15.0 } else { k };
                    let (path, base) = [("transform.rotation_x", d.own[3]), ("transform.rotation_y", d.own[4]), ("transform.rotation", d.own[5])][i];
                    super::xform::put(&mut mc, path, m::Value::Angle(base + deg), d.local, frame);
                    show_readout(ui, pos, &format!("{} {:.1}°", ["X", "Y", "Z"][i], base + deg));
                    "Rotate in 3D"
                }
            };
            let seq = app.doc.seq();
            app.doc.apply_gesture(&format!("gizmo-{id}-{label}"), label, m::Edit::SetClip { seq, clip: Box::new(mc) });
        }
    }
    if ui.input(|i| i.pointer.any_released()) {
        ui.data_mut(|m| m.remove::<GizmoDrag>(drag_id));
    } else if let Some(d) = drag {
        ui.data_mut(|m| m.insert_temp(drag_id, d));
    }
    taken
}

fn show_readout(ui: &egui::Ui, pos: Pos2, text: &str) {
    let t = theme::tokens(ui.ctx());
    let lp = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::unique("gizmo-3d-readout")));
    let g = lp.layout_no_wrap(text.to_owned(), theme::body_medium(), t.text);
    let br = Rect::from_min_size(pos + Vec2::new(14.0, 14.0), g.size() + Vec2::new(12.0, 6.0));
    lp.rect(br, 6, t.panel, Stroke::new(1.0, t.line), egui::StrokeKind::Inside);
    lp.galley(br.min + Vec2::new(6.0, 3.0), g, t.text);
}

// --------------------------------------------------------------- inspector

/// What a clip is, 3D-wise.
fn kind_of(app: &StudioApp, id: u32) -> Option<m::GeneratorKind> {
    match &app.doc.model.active().clip(m::ClipId(id as u64))?.source {
        m::ClipSource::Generator(g) if matches!(g.kind, m::GeneratorKind::Camera | m::GeneratorKind::Light) => Some(g.kind),
        _ => None,
    }
}

/// The Inspector's tabs for a camera or a light (`None` for other clips).
pub fn tabs_for(app: &StudioApp, c: &Clip) -> Option<Vec<(&'static str, Vec<&'static str>)>> {
    Some(camera_light_tabs(kind_of(app, c.id)?))
}

/// The tabs of a camera or a light; the render and sky settings and the
/// raw settings are in the same tab, under the main ones.
pub(crate) fn camera_light_tabs(kind: m::GeneratorKind) -> Vec<(&'static str, Vec<&'static str>)> {
    match kind {
        m::GeneratorKind::Camera => vec![("Camera", vec!["clip", "camera", "render3d", "generator", "data"])],
        _ => vec![("Light", vec!["clip", "light", "light-look", "air", "sky3d", "generator", "data"])],
    }
}

/// A number row bound to a model property of every selected clip, with
/// a keyframe diamond. `to_ui` and `from_ui` convert the stored value
/// (with the canvas width) to what is shown and back.
#[allow(clippy::too_many_arguments)]
pub(super) fn row(app: &mut StudioApp, ui: &mut egui::Ui, ids: &[u32], path: &str, label: &str, range: std::ops::RangeInclusive<f32>, default: f32, unit: &str, decimals: usize, to_ui: fn(f64, f64) -> f64, from_ui: fn(f64, f64) -> f64) {
    let Some(vc) = app.doc.project.clip(ids[0]).cloned() else { return };
    let Some(mc) = app.doc.model.active().clip(m::ClipId(ids[0] as u64)).cloned() else { return };
    let w = app.doc.project.width as f64;
    let local = (app.editor.playhead - vc.start).clamp(0, vc.dur);
    let now = mc.property_at(path, local);
    let mut v = to_ui(now.as_ref().and_then(|v| v.as_f64()).unwrap_or(from_ui(default as f64, w)), w) as f32;
    let half = frame_of(app) / 2;
    let key_here = mc.keyframes.get(path).and_then(|tr| tr.keys.iter().map(|k| k.time).find(|t| (t - local).abs() <= half));
    let mut keyed = key_here.is_some();
    let out = FieldRow::new(label, &mut v, range.clone()).default(default).unit(unit).decimals(decimals).scrub(((range.end() - range.start()) / 400.0).max(0.01)).keyframe(&mut keyed).show(ui);
    let angle = matches!(now, Some(m::Value::Angle(_))) || path.contains("rotation") || path.contains("orientation") || path.ends_with("cone_angle");
    let value = |x: f32| {
        let s = from_ui(x as f64, w);
        if angle { m::Value::Angle(s) } else { m::Value::Scalar(s) }
    };
    let seq = app.doc.seq();
    if out.changed {
        let mut edits = Vec::new();
        for id in ids {
            let (Some(mut mc), Some(vc)) = (app.doc.model.active().clip(m::ClipId(*id as u64)).cloned(), app.doc.project.clip(*id)) else { continue };
            let local = (app.editor.playhead - vc.start).clamp(0, vc.dur);
            if path.starts_with("generator.") && mc.get_property(path).is_none() {
                // Not stored yet: make the setting so it can be set.
                if let m::ClipSource::Generator(g) = &mut mc.source {
                    g.params.insert(path["generator.".len()..].to_owned(), value(v));
                }
            }
            super::xform::put(&mut mc, path, value(v), local, frame_of(app));
            edits.push(m::Edit::SetClip { seq, clip: Box::new(mc) });
        }
        app.doc.apply_gesture(&format!("insp3d-{path}-{ids:?}"), label, m::Edit::Batch { edits });
    }
    if out.keyframe_toggled {
        let mut edits = Vec::new();
        for id in ids {
            let (Some(mc), Some(vc)) = (app.doc.model.active().clip(m::ClipId(*id as u64)), app.doc.project.clip(*id)) else { continue };
            let local = (app.editor.playhead - vc.start).clamp(0, vc.dur);
            match key_here {
                Some(t) if !keyed => edits.push(m::Edit::RemoveKeyframe { seq, clip: mc.id, property: path.into(), time: t }),
                _ if keyed => {
                    let value = mc.property_at(path, local).unwrap_or_else(|| value(v));
                    edits.push(m::Edit::SetKeyframe { seq, clip: mc.id, property: path.into(), key: m::Keyframe::new(local, value, m::Interp::Ease) });
                }
                _ => {}
            }
        }
        if !edits.is_empty() {
            app.doc.apply(if keyed { "Add keyframe" } else { "Remove keyframe" }, m::Edit::Batch { edits });
        }
    }
}

pub(super) fn same(v: f64, _: f64) -> f64 {
    v
}
pub(super) fn pct(v: f64, _: f64) -> f64 {
    v * 100.0
}
pub(super) fn from_pct(v: f64, _: f64) -> f64 {
    v / 100.0
}
fn to_mm(v: f64, w: f64) -> f64 {
    sp::focal_length_from_zoom(v, w)
}
fn from_mm(v: f64, w: f64) -> f64 {
    sp::zoom_from_focal_length(v, w)
}

/// Changes every selected clip's model clip with `f`, as one step.
pub(super) fn change(app: &mut StudioApp, ids: &[u32], label: &str, f: impl Fn(&mut m::Clip)) {
    let seq = app.doc.seq();
    let edits: Vec<m::Edit> = ids
        .iter()
        .filter_map(|id| app.doc.model.active().clip(m::ClipId(*id as u64)).cloned())
        .map(|mut mc| {
            f(&mut mc);
            m::Edit::SetClip { seq, clip: Box::new(mc) }
        })
        .collect();
    app.doc.apply(label, m::Edit::Batch { edits });
}

pub(super) fn flag_row(app: &mut StudioApp, ui: &mut egui::Ui, ids: &[u32], label: &str, on: bool, set: fn(&mut m::Clip, bool)) {
    let mut v = on;
    kit::labeled_row(ui, label, |ui| {
        if kit::toggle(ui, &mut v, label, None).changed() {
            change(app, ids, label, |c| set(c, v));
        }
    });
}

pub(super) fn param_bool(mc: &m::Clip, k: &str, d: bool) -> bool {
    match &mc.source {
        m::ClipSource::Generator(g) => g.params.get(k).and_then(m::Value::as_bool).unwrap_or(d),
        _ => d,
    }
}

pub(super) fn set_param(mc: &mut m::Clip, k: &str, v: m::Value) {
    if let m::ClipSource::Generator(g) = &mut mc.source {
        g.params.insert(k.into(), v);
    }
}

/// A model's surface and the changes that make a new shape.
/// The camera's lens kit and sensor: changing either keeps the focal
/// length (so the view widens or narrows as on a real camera body).
fn lens_rows(app: &mut StudioApp, ui: &mut egui::Ui, ids: &[u32], mc: &m::Clip, w: f64) {
    use crate::agent::cine::{KITS, SENSORS, lens_of};
    let p = mc.generator_params_at(0);
    let lens = lens_of(&p);
    let kits: Vec<&str> = std::iter::once("None").chain(KITS.iter().map(|k| k.label)).collect();
    let ki = lens.kit.and_then(|k| KITS.iter().position(|x| x.name == k.name)).map_or(0, |i| i + 1);
    // A new lens keeps the real focal length the camera has now.
    let refit = |c: &mut m::Clip| {
        let now = c.generator_params_at(0);
        let before = lens_of(&now);
        (now, before)
    };
    kit::labeled_row(ui, "Lens kit", |ui| {
        if let Some(i) = kit::menu_button(ui, "camera-lens-kit", ki, &kits, ui.available_width() - 40.0, None) {
            change(app, ids, "Lens kit", |c| {
                let (now, before) = refit(c);
                let mm = before.focal(now.get("zoom").and_then(m::Value::as_f64).unwrap_or(sp::default_zoom(w)), w);
                if let m::ClipSource::Generator(g) = &mut c.source {
                    for k in ["lens_kit", "squeeze", "breathing", "bokeh", "distortion", "fringe"] {
                        g.params.remove(k);
                    }
                    if let Some(k) = i.checked_sub(1).and_then(|i| KITS.get(i)) {
                        g.params.insert("lens_kit".into(), m::Value::Choice(k.name.into()));
                        if k.bokeh != "round" {
                            g.params.insert("bokeh".into(), m::Value::Choice(k.bokeh.into()));
                        }
                        g.params.insert("distortion".into(), m::Value::Scalar(k.distortion));
                        g.params.insert("fringe".into(), m::Value::Scalar(k.fringe));
                    }
                    let after = lens_of(&g.params);
                    g.params.insert("zoom".into(), m::Value::Scalar(after.zoom(after.nearest(mm), w)));
                }
            });
        }
    });
    let si = SENSORS.iter().position(|s| s.0 == lens.sensor_name).unwrap_or(0);
    let sensors: Vec<&str> = SENSORS.iter().map(|s| s.2).collect();
    kit::labeled_row(ui, "Sensor", |ui| {
        if let Some(i) = kit::menu_button(ui, "camera-sensor", si, &sensors, ui.available_width() - 40.0, None) {
            change(app, ids, "Sensor", |c| {
                let (now, before) = refit(c);
                let mm = before.focal(now.get("zoom").and_then(m::Value::as_f64).unwrap_or(sp::default_zoom(w)), w);
                if let m::ClipSource::Generator(g) = &mut c.source {
                    g.params.insert("sensor".into(), m::Value::Choice(SENSORS[i].0.into()));
                    let after = lens_of(&g.params);
                    g.params.insert("zoom".into(), m::Value::Scalar(after.zoom(mm, w)));
                }
            });
        }
    });
    let zoom = p.get("zoom").and_then(m::Value::as_f64).unwrap_or(sp::default_zoom(w));
    kit::labeled_row(ui, "Lens", |ui| {
        ui.label(egui::RichText::new(lens.words(zoom, w)).font(crate::theme::caption()));
    });
}

fn model_section(app: &mut StudioApp, ui: &mut egui::Ui, id: u32, mc: &m::Clip) {
    let ctx = ui.ctx().clone();
    let ids = [id];
    let mesh = crate::mesh_store::load(mc);
    Section::new("model", "Model").icon(Icon::Shapes).show(ui, |ui| {
        #[cfg(feature = "render")]
        ui.horizontal_wrapped(|ui| {
            let stage = super::model_edit::stage();
            let open = super::model_edit::open_clip() == Some(id as u64);
            for (label, st, key) in [("Edit mode", super::model_edit::Stage::Edit, "Tab"), ("Sculpt mode", super::model_edit::Stage::Sculpt, "")] {
                let on = open && stage == st;
                let b = if on { kit::Button::secondary(label) } else { kit::Button::ghost(label) };
                let b = if key.is_empty() { b } else { b.shortcut(key) };
                if b.compact().show(ui).clicked() {
                    app.editor.selection = vec![id];
                    let to = if on { super::model_edit::Stage::Object } else { st };
                    if let Err(e) = super::model_edit::enter(app, &ctx, to) {
                        kit::push(&ctx, e, None, Some(Icon::Info));
                    }
                }
            }
        });
        match &mesh {
            Ok(mesh) => {
                ui.label(egui::RichText::new(mesh.stats()).font(theme::caption()).color(theme::tokens(ui.ctx()).text_2));
            }
            Err(e) => {
                ui.label(egui::RichText::new(e).font(theme::caption()).color(theme::tokens(ui.ctx()).text_2));
            }
        }
        let base = match &mc.source {
            m::ClipSource::Generator(g) => match g.params.get("base_colour") {
                Some(m::Value::Colour(k)) => Some(*k),
                _ => None,
            },
            _ => None,
        };
        let shown = base.map(|k| (k.r, k.g, k.b)).or_else(|| {
            mesh.as_ref().ok().and_then(|m| m.materials.first()).map(|mat| {
                let enc = |v: f32| if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
                (enc(mat.base_colour[0]) as f64, enc(mat.base_colour[1]) as f64, enc(mat.base_colour[2]) as f64)
            })
        });
        let (r, g, b) = shown.unwrap_or((0.8, 0.8, 0.8));
        let mut col = Color32::from_rgb((r.clamp(0.0, 1.0) * 255.0) as u8, (g.clamp(0.0, 1.0) * 255.0) as u8, (b.clamp(0.0, 1.0) * 255.0) as u8);
        kit::labeled_row(ui, "Colour", |ui| {
            if kit::colour_well(ui, "model-colour", &mut col, None).changed() {
                let rgba = m::Value::Colour(m::Rgba::new(col.r() as f64 / 255.0, col.g() as f64 / 255.0, col.b() as f64 / 255.0, 1.0));
                change(app, &ids, "Model colour", |c| set_param(c, "base_colour", rgba.clone()));
            }
        });
        let first = mesh.as_ref().ok().and_then(|m| m.materials.first().cloned()).unwrap_or_default();
        row(app, ui, &ids, "generator.metallic", "Metallic", 0.0..=100.0, first.metallic * 100.0, "%", 0, pct, from_pct);
        row(app, ui, &ids, "generator.roughness", "Roughness", 0.0..=100.0, first.roughness * 100.0, "%", 0, pct, from_pct);
        row(app, ui, &ids, "generator.subsurface", "Subsurface", 0.0..=100.0, first.subsurface * 100.0, "%", 0, pct, from_pct);
        row(app, ui, &ids, "generator.smooth", "Smooth shading", 0.0..=100.0, 100.0, "%", 0, pct, from_pct);
        row(app, ui, &ids, "generator.opacity", "Opacity", 0.0..=100.0, 100.0, "%", 0, pct, from_pct);
        air::surface_rows(app, ui, &ids);
        #[cfg(feature = "render")]
        super::sculpt::settings_rows(ui);
        kit::menu_heading(ui, "Shape");
        ui.horizontal_wrapped(|ui| {
            if kit::Button::secondary("Subdivide").compact().show(ui).clicked() {
                mesh_change(app, &ctx, id, "Subdivide model", |m| {
                    if m.triangles() * 4 > 1_500_000 {
                        return Err("the model would have too many triangles; decimate first".into());
                    }
                    studio_mesh::ops::subdivide(m, 1);
                    Ok(())
                });
            }
            if kit::Button::secondary("Smooth").compact().show(ui).clicked() {
                mesh_change(app, &ctx, id, "Smooth model", |m| {
                    studio_mesh::ops::smooth(m, &studio_mesh::region::Region::All, 5, 1.0);
                    Ok(())
                });
            }
            if kit::Button::secondary("Symmetrize").compact().show(ui).clicked() {
                mesh_change(app, &ctx, id, "Symmetrize model", |m| {
                    studio_mesh::ops::symmetrize(m, 1.0);
                    Ok(())
                });
            }
            if kit::Button::secondary("Halve triangles").compact().show(ui).clicked() {
                mesh_change(app, &ctx, id, "Decimate model", |m| {
                    let t = m.triangles() / 2;
                    studio_mesh::decimate::decimate(m, t);
                    Ok(())
                });
            }
            if kit::Button::secondary("Flat shading").compact().show(ui).clicked() {
                mesh_change(app, &ctx, id, "Flat shading", |m| {
                    studio_mesh::ops::recompute_normals(m, 30.0);
                    Ok(())
                });
            }
            if kit::Button::secondary("Export…").compact().show(ui).clicked()
                && let Ok(mesh) = &mesh
                && let Some(path) = crate::mesh_store::EXPORT_FILTERS.iter().fold(rfd::FileDialog::new().set_title("Export 3D model").set_file_name(format!("{}.glb", mc.name)), |d, (name, exts)| d.add_filter(*name, exts)).save_file()
            {
                let r = crate::mesh_store::export_model(&mc, mesh, &path);
                let note = match r {
                    Ok(()) => crate::i18n::tf(format!("Exported {}", path.display())),
                    Err(e) => crate::i18n::tf(format!("Could not export: {e}")),
                };
                kit::push(&ctx, note, None, Some(Icon::Export));
            }
        });
    });
}

/// The 3D section of a picture clip, and the Camera and Light sections of
/// cameras and lights (each shows only on its own tab).
pub fn sections(app: &mut StudioApp, ui: &mut egui::Ui, ids: &[u32], c: &Clip) {
    let Some(mc) = app.doc.model.active().clip(m::ClipId(c.id as u64)).cloned() else { return };
    let cw = app.doc.project.width as f32;
    let deep = cw * 8.0;
    if crate::mesh_store::file_of(&mc).is_some() {
        model_section(app, ui, c.id, &mc);
        #[cfg(feature = "render")]
        super::model_edit::panel::sections(app, ui, &mc);
    }
    match kind_of(app, c.id) {
        Some(m::GeneratorKind::Camera) => {
            Section::new("camera", "Camera").icon(Icon::Aperture).show(ui, |ui| {
                let two = param_bool(&mc, "two_node", false);
                let mut k = two as usize;
                kit::labeled_row(ui, "Type", |ui| {
                    if let Some(i) = kit::menu_button(ui, "camera-type", k, &["One-node", "Two-node (aimed)"], ui.available_width() - 40.0, None) {
                        k = i;
                        change(app, ids, "Camera type", |c| set_param(c, "two_node", m::Value::Bool(k == 1)));
                    }
                });
                point_rows(app, ui, ids, "transform.position", "transform.position_z", "Position", deep);
                if two {
                    point_rows(app, ui, ids, "generator.poi", "generator.poi_z", "Target", deep);
                } else {
                    row(app, ui, ids, "transform.rotation_x", "Tilt (X)", -360.0..=360.0, 0.0, "°", 1, same, same);
                    row(app, ui, ids, "transform.rotation_y", "Pan (Y)", -360.0..=360.0, 0.0, "°", 1, same, same);
                }
                row(app, ui, ids, "transform.rotation", "Roll (Z)", -360.0..=360.0, 0.0, "°", 1, same, same);
                let lens = crate::agent::cine::lens_of(&mc.generator_params_at(0));
                let spherical_ff = lens.sensor_name == "full_frame" && lens.squeeze <= 1.0;
                row(app, ui, ids, "generator.zoom", if spherical_ff { "Focal length" } else { "Focal (full-frame equiv.)" }, 8.0..=400.0, 50.0, "mm", 0, to_mm, from_mm);
                lens_rows(app, ui, ids, &mc, cw as f64);
                let dof = param_bool(&mc, "dof", false);
                flag_row(app, ui, ids, "Depth of field", dof, |c, v| set_param(c, "dof", m::Value::Bool(v)));
                if dof {
                    row(app, ui, ids, "generator.focus_distance", "Focus distance", 1.0..=deep, sp::default_zoom(cw as f64) as f32, "px", 0, same, same);
                    row(app, ui, ids, "generator.aperture", "Aperture", 0.0..=500.0, 25.0, "px", 1, same, same);
                    row(app, ui, ids, "generator.blur_level", "Blur level", 0.0..=400.0, 100.0, "%", 0, pct, from_pct);
                }
                auto_orient_row(app, ui, ids, &mc, false);
            });
        }
        Some(_) => {
            let s_kind = sp::LightSettings::read(&mc.generator_params_at(0)).kind;
            Section::new("light", "Light").icon(Icon::Sun).show(ui, |ui| {
                let s = sp::LightSettings::read(&mc.generator_params_at(0));
                let names = ["Parallel (sun)", "Spot", "Point", "Ambient"];
                kit::labeled_row(ui, "Type", |ui| {
                    if let Some(i) = kit::menu_button(ui, "light-type", s.kind as usize, &names, ui.available_width() - 40.0, None) {
                        change(app, ids, "Light type", |c| set_param(c, "light_type", m::Value::Choice(sp::LightKind::NAMES[i].into())));
                    }
                });
                let mut col = Color32::from_rgb((s.colour.r.clamp(0.0, 1.0) * 255.0) as u8, (s.colour.g.clamp(0.0, 1.0) * 255.0) as u8, (s.colour.b.clamp(0.0, 1.0) * 255.0) as u8);
                kit::labeled_row(ui, "Colour", |ui| {
                    if kit::colour_well(ui, "light-colour", &mut col, None).changed() {
                        let rgba = m::Value::Colour(m::Rgba::new(col.r() as f64 / 255.0, col.g() as f64 / 255.0, col.b() as f64 / 255.0, 1.0));
                        change(app, ids, "Light colour", |c| set_param(c, "colour", rgba.clone()));
                    }
                });
                row(app, ui, ids, "generator.intensity", "Intensity", 0.0..=400.0, 100.0, "%", 0, pct, from_pct);
                if s.kind != sp::LightKind::Ambient {
                    point_rows(app, ui, ids, "transform.position", "transform.position_z", "Position", deep);
                    if matches!(s.kind, sp::LightKind::Spot | sp::LightKind::Parallel) {
                        point_rows(app, ui, ids, "generator.poi", "generator.poi_z", "Target", deep);
                    }
                    if s.kind == sp::LightKind::Spot {
                        row(app, ui, ids, "generator.cone_angle", "Cone angle", 1.0..=179.0, 90.0, "°", 0, same, same);
                        row(app, ui, ids, "generator.cone_feather", "Cone feather", 0.0..=100.0, 50.0, "%", 0, pct, from_pct);
                    }
                    if s.kind != sp::LightKind::Parallel {
                        row(app, ui, ids, "generator.radius", "Radius", 0.0..=deep, 0.0, "px", 0, same, same);
                        row(app, ui, ids, "generator.falloff", "Falloff", 0.0..=deep, 0.0, "px", 0, same, same);
                    }
                    flag_row(app, ui, ids, "Casts shadows", s.casts_shadows, |c, v| set_param(c, "casts_shadows", m::Value::Bool(v)));
                    if s.casts_shadows {
                        row(app, ui, ids, "generator.shadow_darkness", "Shadow darkness", 0.0..=100.0, 60.0, "%", 0, pct, from_pct);
                    }
                }
            });
            air::light_look(app, ui, ids, &mc, s_kind);
        }
        None if c.is_visual() => {
            let was = mc.transform.three_d;
            let mut on = was;
            let out = Section::new("space", "3D").icon(Icon::Aperture).enable(&mut on).resettable(was).show(ui, |ui| {
                row(app, ui, ids, "transform.position_z", "Position Z", -deep..=deep, 0.0, "px", 0, same, same);
                row(app, ui, ids, "transform.rotation_x", "Rotation X", -360.0..=360.0, 0.0, "°", 1, same, same);
                row(app, ui, ids, "transform.rotation_y", "Rotation Y", -360.0..=360.0, 0.0, "°", 1, same, same);
                row(app, ui, ids, "transform.orientation_x", "Orientation X", -360.0..=360.0, 0.0, "°", 1, same, same);
                row(app, ui, ids, "transform.orientation_y", "Orientation Y", -360.0..=360.0, 0.0, "°", 1, same, same);
                row(app, ui, ids, "transform.orientation_z", "Orientation Z", -360.0..=360.0, 0.0, "°", 1, same, same);
                row(app, ui, ids, "transform.scale_z", "Scale Z", 1.0..=1000.0, 100.0, "%", 0, pct, from_pct);
                row(app, ui, ids, "transform.anchor_z", "Anchor Z", -deep..=deep, 0.0, "px", 0, same, same);
                auto_orient_row(app, ui, ids, &mc, true);
                let mat = mc.material.clone().unwrap_or_default();
                kit::menu_heading(ui, "Material");
                flag_row(app, ui, ids, "Accepts lights", mat.accepts_lights, |c, v| {
                    c.material.get_or_insert_with(Default::default).accepts_lights = v;
                });
                flag_row(app, ui, ids, "Casts shadows", mat.casts_shadows, |c, v| {
                    c.material.get_or_insert_with(Default::default).casts_shadows = v;
                });
                flag_row(app, ui, ids, "Accepts shadows", mat.accepts_shadows, |c, v| {
                    c.material.get_or_insert_with(Default::default).accepts_shadows = v;
                });
                row(app, ui, ids, "material.extrude", "Extrude", 0.0..=cw, 0.0, "px", 0, same, same);
                row(app, ui, ids, "material.side_shade", "Side shade", 0.0..=100.0, 55.0, "%", 0, pct, from_pct);
                row(app, ui, ids, "material.ambient", "Ambient", 0.0..=200.0, 100.0, "%", 0, pct, from_pct);
                row(app, ui, ids, "material.diffuse", "Diffuse", 0.0..=200.0, 100.0, "%", 0, pct, from_pct);
                row(app, ui, ids, "material.specular", "Specular", 0.0..=200.0, 25.0, "%", 0, pct, from_pct);
                row(app, ui, ids, "material.shininess", "Shininess", 1.0..=200.0, 20.0, "", 0, same, same);
            });
            if out.enable_changed {
                change(app, ids, if on { "3D layer on" } else { "3D layer off" }, |c| c.transform.three_d = on);
            }
            if out.reset {
                change(app, ids, "Reset 3D", |c| {
                    let t = &mut c.transform;
                    (t.position_z, t.rotation_x, t.rotation_y, t.orientation, t.scale_z, t.anchor_z, t.auto_orient) = (0.0, 0.0, 0.0, [0.0; 3], 1.0, 0.0, sp::AutoOrient::Off);
                    c.material = None;
                    c.keyframes.retain(|k, _| !matches!(k.as_str(), "transform.position_z" | "transform.rotation_x" | "transform.rotation_y" | "transform.orientation_x" | "transform.orientation_y" | "transform.orientation_z" | "transform.scale_z" | "transform.anchor_z") && !k.starts_with("material."));
                });
            }
        }
        None => {}
    }
}

/// X, Y and Z rows for a point in space (`path` holds x and y as one
/// point, `path_z` the depth).
fn point_rows(app: &mut StudioApp, ui: &mut egui::Ui, ids: &[u32], path: &str, path_z: &str, label: &str, deep: f32) {
    for axis in 0..2 {
        axis_row(app, ui, ids, path, axis, &format!("{label} {}", ["X", "Y"][axis]), deep);
    }
    row(app, ui, ids, path_z, &format!("{label} Z"), -deep..=deep, 0.0, "px", 0, same, same);
}

/// One axis of a point property, the other axis kept.
fn axis_row(app: &mut StudioApp, ui: &mut egui::Ui, ids: &[u32], path: &str, axis: usize, label: &str, deep: f32) {
    let Some(vc) = app.doc.project.clip(ids[0]).cloned() else { return };
    let Some(mc) = app.doc.model.active().clip(m::ClipId(ids[0] as u64)).cloned() else { return };
    let local = (app.editor.playhead - vc.start).clamp(0, vc.dur);
    let get = |mc: &m::Clip, local: Time| match mc.property_at(path, local) {
        Some(m::Value::Point(q)) => q,
        _ => m::Vec2::ZERO,
    };
    let now = get(&mc, local);
    let mut v = if axis == 0 { now.x } else { now.y } as f32;
    let half = frame_of(app) / 2;
    let key_here = mc.keyframes.get(path).and_then(|tr| tr.keys.iter().map(|k| k.time).find(|t| (t - local).abs() <= half));
    let mut keyed = key_here.is_some();
    let out = FieldRow::new(label, &mut v, -deep..=deep).default(0.0).unit("px").decimals(0).scrub((deep / 200.0).max(0.01)).keyframe(&mut keyed).show(ui);
    let seq = app.doc.seq();
    if out.changed {
        let mut edits = Vec::new();
        for id in ids {
            let (Some(mut mc), Some(vc)) = (app.doc.model.active().clip(m::ClipId(*id as u64)).cloned(), app.doc.project.clip(*id)) else { continue };
            let local = (app.editor.playhead - vc.start).clamp(0, vc.dur);
            let mut q = get(&mc, local);
            if axis == 0 { q.x = v as f64 } else { q.y = v as f64 }
            if path.starts_with("generator.") && mc.get_property(path).is_none() {
                set_param(&mut mc, &path["generator.".len()..], m::Value::Point(q));
            }
            super::xform::put(&mut mc, path, m::Value::Point(q), local, frame_of(app));
            edits.push(m::Edit::SetClip { seq, clip: Box::new(mc) });
        }
        app.doc.apply_gesture(&format!("insp3d-{path}-{axis}-{ids:?}"), label, m::Edit::Batch { edits });
    }
    if out.keyframe_toggled {
        let mut edits = Vec::new();
        for id in ids {
            let (Some(mc), Some(vc)) = (app.doc.model.active().clip(m::ClipId(*id as u64)), app.doc.project.clip(*id)) else { continue };
            let local = (app.editor.playhead - vc.start).clamp(0, vc.dur);
            match key_here {
                Some(t) if !keyed => edits.push(m::Edit::RemoveKeyframe { seq, clip: mc.id, property: path.into(), time: t }),
                _ if keyed => edits.push(m::Edit::SetKeyframe { seq, clip: mc.id, property: path.into(), key: m::Keyframe::new(local, m::Value::Point(get(mc, local)), m::Interp::Ease) }),
                _ => {}
            }
        }
        if !edits.is_empty() {
            app.doc.apply(if keyed { "Add keyframe" } else { "Remove keyframe" }, m::Edit::Batch { edits });
        }
    }
}

fn auto_orient_row(app: &mut StudioApp, ui: &mut egui::Ui, ids: &[u32], mc: &m::Clip, layer: bool) {
    let names: &[&str] = if layer { &["Off", "Along path", "Towards camera"] } else { &["Off", "Along path"] };
    let cur = match mc.transform.auto_orient {
        sp::AutoOrient::Off => 0,
        sp::AutoOrient::Path => 1,
        sp::AutoOrient::Camera => 2,
    }
    .min(names.len() - 1);
    kit::labeled_row(ui, "Auto-orient", |ui| {
        if let Some(i) = kit::menu_button(ui, "auto-orient", cur, names, ui.available_width() - 40.0, None) {
            let a = [sp::AutoOrient::Off, sp::AutoOrient::Path, sp::AutoOrient::Camera][i];
            change(app, ids, "Auto-orient", |c| c.transform.auto_orient = a);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::Event;

    /// A 320 × 180 card in 3D in the middle of a 640 × 360 canvas.
    fn space_doc() -> (crate::doc::Doc, u32) {
        let mut s = crate::agent::Session::new(crate::services::project::new_custom("Space", 640, 360, 30.0));
        s.edit(&[serde_json::json!({"op": "card", "colour": "#ffcc00", "at": 0, "duration": 2, "width": 320, "height": 180})], "card", false).unwrap();
        let id = s.doc.project.clips.iter().map(|c| c.id).max().unwrap();
        s.edit(&[serde_json::json!({"op": "three_d", "clip": id})], "3d", false).unwrap();
        (s.doc, id)
    }

    fn app_with(doc: crate::doc::Doc, ctx: &egui::Context) -> StudioApp {
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let _serial = crate::ui_tests::serial();
        let mut app = StudioApp::new(&cc, crate::cli::Cli { screen: Some(crate::cli::Screen::Editor), fresh: true, ..Default::default() });
        app.doc = doc;
        app.editor.playhead = 0;
        app
    }

    fn press(p: Pos2, down: bool) -> Event {
        Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: down, modifiers: egui::Modifiers::NONE }
    }

    /// Drags from `from` to `to` in six steps, running `ui` each frame.
    fn drag(ctx: &egui::Context, app: &mut StudioApp, from: Pos2, to: Pos2, ui_fn: &dyn Fn(&mut StudioApp, &mut egui::Ui)) {
        let canvas = Rect::from_min_size(Pos2::ZERO, Vec2::new(640.0, 360.0));
        let mut t = 0.0;
        let mut frame = |app: &mut StudioApp, events: Vec<Event>| {
            t += 1.0 / 60.0;
            let input = egui::RawInput { screen_rect: Some(canvas), time: Some(t), events, ..Default::default() };
            crate::editor::test_frame(&ctx, input, |ui| ui_fn(app, ui));
        };
        frame(app, vec![Event::PointerMoved(from)]);
        frame(app, vec![press(from, true)]);
        for k in 1..=6 {
            frame(app, vec![Event::PointerMoved(from + (to - from) * k as f32 / 6.0)]);
        }
        frame(app, vec![press(to, false)]);
        frame(app, vec![]);
    }

    fn handles(app: &mut StudioApp, ui: &mut egui::Ui, id: u32) {
        let canvas = Rect::from_min_size(Pos2::ZERO, Vec2::new(640.0, 360.0));
        super::super::xform::interact(app, ui, canvas, canvas, &[id], 1.0);
    }

    /// The X arrow moves the layer along its own x; the Y ring turns it
    /// about y; the Z ring (seen face on) turns it as the pointer goes
    /// round.
    #[test]
    fn the_gizmo_moves_and_turns_a_3d_layer() {
        let ctx = egui::Context::default();
        let (doc, id) = space_doc();
        let mut app = app_with(doc, &ctx);
        app.editor.selection = vec![id];
        let own = |app: &StudioApp| app.doc.model.active().clip(m::ClipId(id as u64)).unwrap().transform.clone();
        drag(&ctx, &mut app, Pos2::new(370.0, 180.0), Pos2::new(410.0, 180.0), &|app, ui| handles(app, ui, id));
        let t = own(&app);
        assert!((t.position.x - 40.0).abs() < 2.0 && t.position.y.abs() < 1e-6, "moved to {:?}", t.position);
        // The Y ring is edge-on at rest: a line through the anchor; drag
        // across it to the right turns the right edge away.
        drag(&ctx, &mut app, Pos2::new(330.0, 180.0), Pos2::new(380.0, 180.0), &|app, ui| handles(app, ui, id));
        let ry = own(&app).rotation_y;
        assert!(ry > 5.0, "turned {ry}");
    }

    /// With the orbit tool a drag on the picture makes a camera (there
    /// was none) and swings it round what it looks at.
    #[test]
    fn camera_tools_orbit_pan_and_dolly() {
        let ctx = egui::Context::default();
        let (doc, _) = space_doc();
        let mut app = app_with(doc, &ctx);
        let area = Rect::from_min_size(Pos2::ZERO, Vec2::new(640.0, 360.0));
        set_tool(&ctx, CamTool::Orbit);
        drag(&ctx, &mut app, Pos2::new(320.0, 180.0), Pos2::new(420.0, 180.0), &|app, ui| {
            camera_interact(app, ui, area, 1.0);
        });
        let seq = app.doc.model.active();
        let cam = sp::active_camera(seq, 0).expect("a camera was made");
        let w = 640.0;
        let r = rig_of(cam, 0, w);
        let dist = v_len(v_sub(r.poi, r.pos));
        assert!((dist - sp::default_zoom(w)).abs() < 1.0, "{dist}");
        assert!(r.pos[0].abs() > 100.0, "swung round: {:?}", r.pos);
        set_tool(&ctx, CamTool::Dolly);
        drag(&ctx, &mut app, Pos2::new(320.0, 200.0), Pos2::new(320.0, 100.0), &|app, ui| {
            camera_interact(app, ui, area, 1.0);
        });
        let cam = sp::active_camera(app.doc.model.active(), 0).unwrap();
        let r2 = rig_of(cam, 0, w);
        assert!(v_len(v_sub(r2.poi, r2.pos)) < dist - 50.0, "dollied in");
        set_tool(&ctx, CamTool::Pan);
        drag(&ctx, &mut app, Pos2::new(320.0, 200.0), Pos2::new(320.0, 250.0), &|app, ui| {
            camera_interact(app, ui, area, 1.0);
        });
        let r3 = rig_of(sp::active_camera(app.doc.model.active(), 0).unwrap(), 0, w);
        assert!((r3.poi[1] - r2.poi[1]).abs() > 10.0, "panned");
        assert!(tool(&ctx) == CamTool::Pan);
    }

    /// The whole editor, headless, with a 3D layer, a camera and a light
    /// each selected in turn, and each camera tool on: the viewer, the
    /// gizmo and the Inspector's 3D, Camera and Light sections draw
    /// without changing anything.
    #[test]
    fn the_editor_draws_3d_cameras_and_lights() {
        let ctx = egui::Context::default();
        let (doc, id) = space_doc();
        let mut s = crate::agent::Session::new(doc);
        s.edit(&[serde_json::json!({"op": "camera", "move": "orbit"}), serde_json::json!({"op": "light", "type": "spot", "shadows": true}), serde_json::json!({"op": "three_d", "clip": id, "extrude": 20, "casts_shadows": true, "rotation_y": 30})], "rig", false).unwrap();
        // The look sections: a flame lamp, the air and look carrier with weather, a wet glowing model.
        s.edit(&[serde_json::json!({"op": "light", "type": "point", "glow": 1, "flame": true, "flame_height": 0.1, "flicker": 0.3}), serde_json::json!({"op": "atmosphere", "preset": "neon_rain"}), serde_json::json!({"op": "model", "shape": "sphere", "emissive": "#ffb060", "emissive_strength": 6, "wetness": 0.5})], "look", false).unwrap();
        let before = s.doc.model.clone();
        let ids: Vec<u32> = s.doc.project.clips.iter().map(|c| c.id).collect();
        let mut app = app_with(s.doc, &ctx);
        let mut frame = eframe::Frame::_new_kittest();
        let mut k = 0.0;
        for sel in ids {
            app.editor.selection = vec![sel];
            for t in [CamTool::Off, CamTool::Orbit] {
                set_tool(&ctx, t);
                for _ in 0..3 {
                    k += 1.0 / 60.0;
                    let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1600.0, 1000.0))), time: Some(k), ..Default::default() };
                    crate::editor::test_frame(&ctx, input, |ui| eframe::App::ui(&mut app, ui, &mut frame));
                }
            }
        }
        assert_eq!(app.doc.model, before);
    }
}
