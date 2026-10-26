// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The bone tool and the Inspector's "Skeleton" section.
//!
//! On the canvas, with the tool on **Pose**: drag a bone's tip to turn it
//! about its joint (forward kinematics); ⌘-drag a hand or foot to place
//! it and let the limb follow (inverse kinematics, two bones up); drag the
//! root's joint to shift the whole body; drag a live IK target (a
//! diamond) to move it. Each drag writes a key at the playhead once the
//! bone is animated, as every canvas drag does. With **Draw**: drag on
//! the layer to draw a bone, from the tip of the bone it should hang
//! from to start a child. The Inspector builds a skeleton from the
//! silhouette, keys bone rotations, copies, pastes, mirrors and saves
//! poses, bakes motions (walk, run, wave…), splits the picture into part
//! layers and switches swap sets.

use crate::app::StudioApp;
use super::pins::LayerMap;
use crate::kit::FieldRow;
use crate::services::{character as ch, rigging as rg};
use crate::theme;
use crate::view::{SEC, Time};
use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, Vec2};
use studio_model as m;

/// What the bone tool does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum BoneTool {
    #[default]
    Off,
    Pose,
    Draw,
}

fn tool_id() -> egui::Id {
    egui::Id::unique("canvas-bone-tool")
}

pub fn tool(ctx: &egui::Context) -> BoneTool {
    ctx.data(|d| d.get_temp(tool_id())).unwrap_or_default()
}

pub fn set_tool(ctx: &egui::Context, t: BoneTool) {
    ctx.data_mut(|d| d.insert_temp(tool_id(), t));
}

/// The bone picked on the canvas (its Inspector rows open).
fn picked(ctx: &egui::Context) -> Option<u32> {
    ctx.data(|d| d.get_temp(tool_id().with("picked")))
}

fn pick(ctx: &egui::Context, b: Option<u32>) {
    ctx.data_mut(|d| match b {
        Some(b) => {
            d.insert_temp(tool_id().with("picked"), b);
        }
        None => {
            d.remove::<u32>(tool_id().with("picked"));
        }
    });
}

/// A pose copied in the Inspector or with ⌘C on the canvas.
fn clipboard(ctx: &egui::Context) -> Option<m::Pose> {
    ctx.data(|d| d.get_temp::<String>(tool_id().with("pose"))).and_then(|s| serde_json::from_str(&s).ok())
}

fn set_clipboard(ctx: &egui::Context, p: &m::Pose) {
    if let Ok(s) = serde_json::to_string(p) {
        ctx.copy_text(s.clone());
        ctx.data_mut(|d| d.insert_temp(tool_id().with("pose"), s));
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Grab {
    /// Turn a bone about its joint.
    Tip(u32),
    /// Reach with a limb: the tip of the last bone, two up turning.
    Reach(u32),
    /// Shift the root.
    Root(u32),
    /// A live IK handle's target.
    Target(u32),
    /// Drawing a new bone from a point (layer coordinates).
    New([f64; 2], Option<u32>),
}

#[derive(Clone, Copy, Debug)]
struct Drag {
    id: u32,
    grab: Grab,
    local: Time,
    /// Rotation of the bone and the pointer's angle when the drag began.
    start: (f64, f64),
}

fn frame_len(app: &StudioApp) -> Time {
    (SEC as f64 / (app.doc.project.fps as f64).max(1.0)) as Time
}

fn mv(v: Vec2) -> m::Vec2 {
    m::Vec2::new(v.x as f64, v.y as f64)
}

/// Bones of the selected layer on the canvas, and the gestures on them.
/// Returns `true` when the tool took the pointer.
pub fn interact(app: &mut StudioApp, ui: &mut egui::Ui, canvas: Rect, area: Rect, scale: f32) -> bool {
    let mut tool = tool(ui.ctx());
    if tool != BoneTool::Off && (ui.input(|i| i.key_pressed(egui::Key::Escape)) || app.editor.selection.is_empty()) {
        set_tool(ui.ctx(), BoneTool::Off);
        tool = BoneTool::Off;
    }
    if tool == BoneTool::Off {
        return false;
    }
    let Some(id) = app.editor.selection.first().copied() else { return false };
    let now = app.editor.hover_time.unwrap_or(app.editor.playhead);
    let Some(map) = LayerMap::at(&app.doc.project, &app.doc.model, id, now) else { return false };
    let Some(vc) = app.doc.project.clip(id).cloned() else { return false };
    let Some(mc) = app.doc.model.active().clip(m::ClipId(id as u64)).cloned() else { return false };
    let local = (app.editor.playhead - vc.start).clamp(0, vc.dur.max(0));
    let shown = (now - vc.start).clamp(0, vc.dur.max(0));
    let frame = frame_len(app);
    let screen = |c: Vec2| canvas.center() + c * scale;
    let from_screen = |p: Pos2| (p - canvas.center()) / scale;
    let p = ui.painter().with_clip_rect(area.expand(8.0));
    let drag_id = egui::Id::unique("canvas-bone-drag");
    let mut drag: Option<Drag> = ui.data(|d| d.get_temp(drag_id));
    let t = theme::tokens(ui.ctx());
    let sel = picked(ui.ctx());
    let posed = mc.skeleton_at(shown).map(|s| (s.forward(), s));
    let cmd = ui.input(|i| i.modifiers.command);
    // The bones: a tapered blade from joint to tip.
    let mut handles: Vec<(Grab, Pos2)> = Vec::new();
    if let Some((pz, sk)) = &posed {
        for b in &sk.bones {
            let Some(pb) = pz.bone(b.id) else { continue };
            let (a, z) = (screen(map.canvas(pb.head)), screen(map.canvas(pb.tail)));
            let d = z - a;
            let n = Vec2::new(-d.y, d.x).normalized() * (d.length() * 0.12).clamp(2.0, 7.0);
            let colour = if sel == Some(b.id) { t.select } else if b.deform { Color32::from_rgb(250, 204, 21) } else { Color32::from_rgb(148, 163, 184) };
            let blade = vec![a, a + d * 0.18 + n, z, a + d * 0.18 - n];
            p.add(egui::Shape::convex_polygon(blade, colour.gamma_multiply(0.35), Stroke::new(1.5, colour)));
            p.circle_stroke(a, 3.5, Stroke::new(1.5, colour));
            let leaf = sk.children(b.id).is_empty();
            handles.push((if leaf && cmd { Grab::Reach(b.id) } else { Grab::Tip(b.id) }, z));
            if b.parent.is_none() {
                handles.push((Grab::Root(b.id), a));
            }
        }
        for h in sk.ik.iter().filter(|h| h.enabled) {
            handles.push((Grab::Target(h.id), screen(map.canvas(h.target))));
        }
    }
    let body = (tool == BoneTool::Draw && drag.is_none()).then(|| ui.interact(area, egui::Id::unique("canvas-bone-body"), Sense::click_and_drag()));
    // Posing a layer without bones leaves the pointer to the transform
    // handles.
    let mut took = tool == BoneTool::Draw || posed.is_some();
    for (g, at) in &handles {
        let resp = ui.interact(Rect::from_center_size(*at, Vec2::splat(14.0)), egui::Id::unique(("canvas-bone", id, format!("{g:?}"))), Sense::click_and_drag());
        let hot = resp.hovered() || resp.dragged();
        match g {
            Grab::Target(_) => {
                let r = if hot { 7.0 } else { 5.5 };
                let pts = vec![*at + Vec2::new(0.0, -r), *at + Vec2::new(r, 0.0), *at + Vec2::new(0.0, r), *at + Vec2::new(-r, 0.0)];
                p.add(egui::Shape::convex_polygon(pts, Color32::from_rgb(56, 189, 248), Stroke::new(1.5, Color32::from_black_alpha(200))));
            }
            Grab::Root(_) => {
                p.rect(Rect::from_center_size(*at, Vec2::splat(if hot { 10.0 } else { 8.0 })), 1, Color32::WHITE, Stroke::new(1.0, t.select), egui::StrokeKind::Inside);
            }
            _ => {
                p.circle_filled(*at, if hot { 5.5 } else { 4.0 }, if matches!(g, Grab::Reach(_)) { Color32::from_rgb(56, 189, 248) } else { Color32::WHITE });
            }
        }
        if hot {
            let name = match g {
                Grab::Tip(b) | Grab::Reach(b) | Grab::Root(b) => posed.as_ref().and_then(|(_, s)| s.bone(*b)).map(|b| b.name.clone()),
                Grab::Target(h) => posed.as_ref().and_then(|(_, s)| s.handle(*h)).map(|h| crate::i18n::tf(format!("IK {}", h.name))),
                Grab::New(..) => None,
            };
            if let Some(n) = name.filter(|n| !n.is_empty()) {
                resp.clone().on_hover_text(n);
            }
        }
        let resp = resp.on_hover_cursor(egui::CursorIcon::Grab);
        if resp.clicked() {
            if let Grab::Tip(b) | Grab::Reach(b) | Grab::Root(b) = g {
                pick(ui.ctx(), Some(*b));
            }
        }
        if resp.drag_started() && tool == BoneTool::Pose {
            let (rot, ang) = match (g, &posed, resp.interact_pointer_pos()) {
                (Grab::Tip(b), Some((pz, sk)), Some(ptr)) => {
                    let head = pz.bone(*b).map(|pb| screen(map.canvas(pb.head))).unwrap_or(ptr);
                    (sk.bone(*b).map_or(0.0, |x| x.rotation), ((ptr.y - head.y) as f64).atan2((ptr.x - head.x) as f64).to_degrees())
                }
                _ => (0.0, 0.0),
            };
            drag = Some(Drag { id, grab: *g, local, start: (rot, ang) });
        }
        if resp.drag_started() && tool == BoneTool::Draw {
            if let (Grab::Tip(b), Some((pz, _))) = (g, &posed) {
                let tip = pz.bone(*b).map(|pb| pb.tail).unwrap_or(m::Vec2::ZERO);
                drag = Some(Drag { id, grab: Grab::New([tip.x, tip.y], Some(*b)), local, start: (0.0, 0.0) });
            }
        }
    }
    if let Some(body) = &body {
        if body.drag_started() {
            if let Some(at) = body.interact_pointer_pos().and_then(|q| map.layer(from_screen(q))) {
                drag = Some(Drag { id, grab: Grab::New([at.x, at.y], sel), local, start: (0.0, 0.0) });
            }
        }
    }
    if let (Some(d), Some(ptr)) = (drag, ui.input(|i| i.pointer.interact_pos())) {
        took = true;
        let c = from_screen(ptr);
        let released = ui.input(|i| i.pointer.any_released());
        if let Some(mut mc) = app.doc.model.active().clip(m::ClipId(d.id as u64)).cloned() {
            let label = match d.grab {
                Grab::Tip(b) => {
                    if let Some((pz, _)) = &posed {
                        if let Some(pb) = pz.bone(b) {
                            let head = screen(map.canvas(pb.head));
                            let ang = ((ptr.y - head.y) as f64).atan2((ptr.x - head.x) as f64).to_degrees();
                            let limit = mc.skeleton.as_ref().and_then(|s| s.bone(b)).map(|x| x.limit(d.start.0 + m::skeleton::wrap(ang - d.start.1))).unwrap_or(0.0);
                            super::xform::put(&mut mc, &format!("bone.{b}.rotation"), m::Value::Angle(limit), d.local, frame);
                        }
                    }
                    Some("Turn bone")
                }
                Grab::Reach(b) => {
                    if let Some(target) = map.layer(c) {
                        if let Ok(rot) = ch::reach(&app.doc, d.id, b, 2, target, false, vc.start + d.local) {
                            for (bid, r) in rot {
                                super::xform::put(&mut mc, &format!("bone.{bid}.rotation"), m::Value::Angle(r), d.local, frame);
                            }
                        }
                    }
                    Some("Reach")
                }
                Grab::Root(b) => {
                    if let (Some(at), Some(rest)) = (map.layer(c), mc.skeleton.as_ref().and_then(|s| s.bone(b)).map(|x| x.head)) {
                        super::xform::put(&mut mc, &format!("bone.{b}.offset"), m::Value::Point(m::Vec2::new(at.x - rest.x, at.y - rest.y)), d.local, frame);
                    }
                    Some("Move body")
                }
                Grab::Target(h) => {
                    if let Some(at) = map.layer(c) {
                        super::xform::put(&mut mc, &format!("ik.{h}.target"), m::Value::Point(at), d.local, frame);
                    }
                    Some("Move IK target")
                }
                Grab::New(from, parent) => {
                    let a = screen(map.canvas(m::Vec2::new(from[0], from[1])));
                    p.line_segment([a, ptr], Stroke::new(2.0, t.select));
                    if released && (ptr - a).length() > 6.0 {
                        if let Some(to) = map.layer(c) {
                            let px = map.pixels();
                            let sk = mc.skeleton.get_or_insert_with(|| m::Skeleton { aspect: (px.x / px.y.max(1.0)) as f64, ..m::Skeleton::default() });
                            let n = sk.bones.len() + 1;
                            let b = sk.add(format!("bone.{n}"), parent, m::Vec2::new(from[0], from[1]), to);
                            pick(ui.ctx(), Some(b));
                            let seq = app.doc.seq();
                            app.doc.apply("Draw bone", m::Edit::SetClip { seq, clip: Box::new(mc.clone()) });
                        }
                    }
                    None
                }
            };
            if let Some(label) = label {
                let seq = app.doc.seq();
                app.doc.apply_gesture(&format!("canvas-bone-{}-{:?}", d.id, d.grab), label, m::Edit::SetClip { seq, clip: Box::new(mc) });
            }
        }
        let _ = mv;
    }
    if ui.input(|i| i.pointer.any_released()) {
        ui.data_mut(|m| m.remove::<Drag>(drag_id));
    } else if let Some(d) = drag {
        ui.data_mut(|m| m.insert_temp(drag_id, d));
    }
    took
}

// ------------------------------------------------------------ Inspector

const KINDS: [&str; 7] = ["biped", "quadruped", "bird", "fish", "snake", "plant", "object"];
const MOTIONS: [&str; 16] = ["idle", "walk", "run", "sneak", "hop", "jump", "wave", "point", "talk", "nod", "look", "sad", "celebrate", "wag", "swim", "sway"];

fn set_clip(app: &mut StudioApp, label: &str, mc: m::Clip) {
    let seq = app.doc.seq();
    app.doc.apply(label, m::Edit::SetClip { seq, clip: Box::new(mc) });
}

/// A slider that edits one number of the skeleton: `set` writes the new
/// value into a copy of the clip when it changes.
fn skeleton_slider(app: &mut StudioApp, ui: &mut egui::Ui, mc: &m::Clip, label: &str, value: f32, range: std::ops::RangeInclusive<f32>, unit: &str, set: impl FnOnce(&mut m::Skeleton, f32)) {
    let mut v = value;
    if crate::kit::Slider::new(label, &mut v, range).unit(unit).default(value).show(ui).changed() {
        let mut c = mc.clone();
        if let Some(sk) = c.skeleton.as_mut() {
            set(sk, v);
        }
        set_clip(app, "Skeleton", c);
    }
}

/// An IK handle's chain length, stretch, soft reach, pole and pin.
fn ik_settings(app: &mut StudioApp, ui: &mut egui::Ui, mc: &m::Clip, posed: &m::Skeleton, h: &m::IkHandle) {
    let hid = h.id;
    let longest = mc.skeleton.as_ref().map_or(2, |s| s.chain_up(h.bone).len()).clamp(1, 8) as f32;
    skeleton_slider(app, ui, mc, "Chain bones", h.chain as f32, 1.0..=longest.max(2.0), "", move |s, v| {
        if let Some(x) = s.handle_mut(hid) {
            x.chain = v.round().max(1.0) as u32;
        }
    });
    skeleton_slider(app, ui, mc, "Stretch", (h.stretch * 100.0) as f32, 100.0..=300.0, "%", move |s, v| {
        if let Some(x) = s.handle_mut(hid) {
            x.stretch = f64::from(v) / 100.0;
        }
    });
    skeleton_slider(app, ui, mc, "Soft reach", (h.soft * 100.0) as f32, 0.0..=50.0, "%", move |s, v| {
        if let Some(x) = s.handle_mut(hid) {
            x.soft = f64::from(v) / 100.0;
        }
    });
    let mut has_pole = h.pole.is_some();
    crate::kit::labeled_row(ui, "Pole", |ui| {
        if crate::kit::toggle(ui, &mut has_pole, "Bend toward a pole", None).changed() {
            let mut c = mc.clone();
            if let Some(x) = c.skeleton.as_mut().and_then(|s| s.handle_mut(hid)) {
                // A new pole sits beside the middle joint, out from the limb.
                let chain = posed.chain_up(h.bone);
                let mid = chain.get(chain.len().min(h.chain.max(2) as usize).saturating_sub(1)).copied();
                let joint = mid.and_then(|b| posed.forward().bone(b).map(|p| p.head));
                x.pole = has_pole.then(|| joint.unwrap_or(h.target));
            }
            set_clip(app, "IK pole", c);
        }
    });
    if let Some(p) = h.pole {
        skeleton_slider(app, ui, mc, "Pole X", (p.x * 100.0) as f32, -50.0..=150.0, "%", move |s, v| {
            if let Some(x) = s.handle_mut(hid) {
                x.pole = x.pole.map(|p| m::Vec2::new(f64::from(v) / 100.0, p.y));
            }
        });
        skeleton_slider(app, ui, mc, "Pole Y", (p.y * 100.0) as f32, -50.0..=150.0, "%", move |s, v| {
            if let Some(x) = s.handle_mut(hid) {
                x.pole = x.pole.map(|p| m::Vec2::new(p.x, f64::from(v) / 100.0));
            }
        });
    }
    if crate::kit::Button::ghost("Pin here").compact().show(ui).on_hover_text(crate::i18n::shown("Plant the end where it is now: it stays while the body moves")).clicked() {
        let mut c = mc.clone();
        if let (Some(x), Some(tip)) = (c.skeleton.as_mut().and_then(|s| s.handle_mut(hid)), posed.forward().bone(h.bone).map(|p| p.tail)) {
            x.target = tip;
        }
        c.keyframes.remove(&format!("ik.{hid}.target"));
        set_clip(app, "Pin", c);
    }
}

/// A bone's joint limits: off, or a range of degrees from rest.
fn joint_limits(app: &mut StudioApp, ui: &mut egui::Ui, mc: &m::Clip, b: &m::Bone) {
    let bid = b.id;
    let mut limited = b.min.is_some() || b.max.is_some();
    crate::kit::labeled_row(ui, "Limits", |ui| {
        if crate::kit::toggle(ui, &mut limited, "Limit this joint", None).changed() {
            let mut c = mc.clone();
            if let Some(x) = c.skeleton.as_mut().and_then(|s| s.bone_mut(bid)) {
                (x.min, x.max) = if limited { (Some(-90.0), Some(90.0)) } else { (None, None) };
            }
            set_clip(app, "Joint limits", c);
        }
    });
    if limited {
        skeleton_slider(app, ui, mc, "Lowest", b.min.unwrap_or(-360.0) as f32, -360.0..=360.0, "°", move |s, v| {
            if let Some(x) = s.bone_mut(bid) {
                x.min = Some(f64::from(v));
            }
        });
        skeleton_slider(app, ui, mc, "Highest", b.max.unwrap_or(360.0) as f32, -360.0..=360.0, "°", move |s, v| {
            if let Some(x) = s.bone_mut(bid) {
                x.max = Some(f64::from(v));
            }
        });
    }
}

/// The Inspector's "Skeleton" section for clip `id`.
pub fn inspector(app: &mut StudioApp, ui: &mut egui::Ui, id: u32) {
    use crate::icons::Icon;
    use crate::kit::{Button, Section, Segment};
    let Some(mc) = app.doc.model.active().clip(m::ClipId(id as u64)).cloned() else { return };
    // Only pictures carry bones.
    if !matches!(mc.source, m::ClipSource::Media(_)) || mc.is_camera_or_light() {
        return;
    }
    let ctx = ui.ctx().clone();
    let at = app.editor.playhead;
    let out = Section::new("skeleton", "Skeleton").icon(Icon::Intersect).resettable(mc.skeleton.is_some()).show(ui, |ui| {
        let t = theme::tokens(ui.ctx());
        let w = ui.available_width();
        let note = |ui: &mut egui::Ui, s: &str| {
            ui.label(egui::RichText::new(s).font(theme::caption()).color(t.text_2));
        };
        let kind_id = egui::Id::unique(("skeleton-kind", id));
        let mut kind: usize = ui.data(|d| d.get_temp(kind_id)).unwrap_or(0);
        crate::kit::labeled_row(ui, "Body", |ui| {
            egui::ComboBox::from_id_salt(("skeleton-kind-box", id)).selected_text(KINDS[kind]).width(120.0).show_ui(ui, |ui| {
                crate::kit::note_list(ui);
                for (i, k) in KINDS.iter().enumerate() {
                    ui.selectable_value(&mut kind, i, *k);
                }
            });
        });
        ui.data_mut(|d| d.insert_temp(kind_id, kind));
        // Which way it faces as the picture shows it.
        const FACING: [&str; 4] = ["Guess", "Right", "Left", "Camera"];
        let facing_id = egui::Id::unique(("skeleton-facing", id));
        let mut facing: usize = ui.data(|d| d.get_temp(facing_id)).unwrap_or(0);
        crate::kit::labeled_row(ui, "Facing", |ui| {
            egui::ComboBox::from_id_salt(("skeleton-facing-box", id)).selected_text(FACING[facing]).width(120.0).show_ui(ui, |ui| {
                crate::kit::note_list(ui);
                for (i, k) in FACING.iter().enumerate() {
                    ui.selectable_value(&mut facing, i, *k);
                }
            });
        });
        ui.data_mut(|d| d.insert_temp(facing_id, facing));
        let label = if mc.skeleton.is_some() { "Rebuild from silhouette" } else { "Build from silhouette" };
        if Button::secondary(label).icon(Icon::MagicWand).show(ui).clicked() {
            // The canvas shows the picture flipped when the layer is.
            let flip = mc.transform.flip_x;
            let faces = [None, Some(Some(!flip)), Some(Some(flip)), Some(None)][facing];
            match ch::build(&mut app.doc, id, KINDS[kind], faces, at) {
                // A rigged character gets the cuts that let its legs, arms
                // and mouth move apart.
                Ok(_) => super::scissor::after_build(app, &ctx, id, KINDS[kind]),
                Err(e) => app.doc.error = Some(e),
            }
        }
        // The canvas tool.
        let mut cur = match tool(&ctx) {
            BoneTool::Off => 0,
            BoneTool::Pose => 1,
            BoneTool::Draw => 2,
        };
        if crate::kit::segmented(ui, "bone-tool", &mut cur, &[Segment::text("Off"), Segment::text("Pose"), Segment::text("Draw")], Some(w), None).changed() {
            set_tool(&ctx, [BoneTool::Off, BoneTool::Pose, BoneTool::Draw][cur]);
            if cur != 0 {
                super::pins::set_tool(&ctx, super::pins::PinTool::Off);
            }
        }
        match tool(&ctx) {
            BoneTool::Pose => note(ui, "Drag a bone's tip to turn it; ⌘-drag a hand or foot to place it (the limb follows); drag the white square to move the body; drag a blue diamond to move an IK target. Keys go in at the playhead once a bone is animated."),
            BoneTool::Draw => note(ui, "Drag on the layer to draw a bone; start on a bone's tip to hang the new one from it."),
            BoneTool::Off => {}
        }
        let Some(sk) = mc.skeleton.clone() else {
            note(ui, "Bones bend the picture like a puppet's mesh, carry part layers and take motions, poses and motion capture.");
            return;
        };
        let local = (at - mc.start).clamp(0, mc.duration);
        let posed = mc.skeleton_at(local).unwrap_or(sk.clone());
        let mut enabled = sk.enabled;
        crate::kit::labeled_row(ui, "Bones", |ui| {
            if crate::kit::toggle(ui, &mut enabled, "Bones on", None).changed() {
                let mut c = mc.clone();
                if let Some(s) = &mut c.skeleton {
                    s.enabled = enabled;
                }
                set_clip(app, "Skeleton", c);
            }
        });
        let mut falloff = (sk.falloff * 100.0) as f32;
        if FieldRow::new("Reach", &mut falloff, 0.5..=30.0).default(6.0).unit("%").decimals(1).show(ui).changed {
            let mut c = mc.clone();
            if let Some(s) = &mut c.skeleton {
                s.falloff = falloff as f64 / 100.0;
            }
            let seq = app.doc.seq();
            app.doc.apply_gesture(&format!("bone-falloff-{id}"), "Bone reach", m::Edit::SetClip { seq, clip: Box::new(c) });
        }
        // Motions.
        let act_id = egui::Id::unique(("skeleton-act", id));
        let mut act: usize = ui.data(|d| d.get_temp(act_id)).unwrap_or(1);
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt(("skeleton-act-box", id)).selected_text(MOTIONS[act]).width(100.0).show_ui(ui, |ui| {
                crate::kit::note_list(ui);
                for (i, k) in MOTIONS.iter().enumerate() {
                    ui.selectable_value(&mut act, i, *k);
                }
            });
            if Button::secondary("Act from playhead").icon(Icon::Play).show(ui).clicked() {
                let from = (local as f64 / SEC as f64).min((mc.duration as f64 / SEC as f64 - 0.5).max(0.0));
                let dur = mc.duration as f64 / SEC as f64 - from;
                if let Err(e) = ch::act(&mut app.doc, id, &[MOTIONS[act].to_owned()], from, dur, 1.0) {
                    app.doc.error = Some(e);
                }
            }
        });
        ui.data_mut(|d| d.insert_temp(act_id, act));
        // Poses.
        ui.horizontal_wrapped(|ui| {
            if Button::ghost("Copy pose").icon(Icon::Copy).show(ui).clicked() {
                set_clipboard(&ctx, &posed.pose_now("copied"));
            }
            if Button::ghost("Paste pose").show(ui).clicked() {
                if let Some(p) = clipboard(&ctx) {
                    let targets = sk.pose_targets(&p);
                    let rot: Vec<(u32, f64)> = targets.iter().map(|x| (x.0, x.1)).collect();
                    let off: Vec<(u32, m::Vec2)> = targets.iter().filter_map(|x| x.2.map(|o| (x.0, o))).collect();
                    if let Err(e) = ch::pose(&mut app.doc, id, &rot, &off, at, false) {
                        app.doc.error = Some(e);
                    }
                }
            }
            if Button::ghost("Mirror").show(ui).clicked() {
                let rot: Vec<(u32, f64)> = posed
                    .bones
                    .iter()
                    .filter_map(|b| {
                        let other = if b.name.contains("front") { b.name.replace("front", "back") } else if b.name.contains("back") { b.name.replace("back", "front") } else { return None };
                        posed.named(&other).map(|o| (b.id, o.rotation))
                    })
                    .collect();
                let _ = ch::pose(&mut app.doc, id, &rot, &[], at, false);
            }
            if Button::ghost("Rest").show(ui).clicked() {
                let rot: Vec<(u32, f64)> = sk.bones.iter().map(|b| (b.id, 0.0)).collect();
                let off: Vec<(u32, m::Vec2)> = sk.bones.iter().filter(|b| b.offset != m::Vec2::ZERO).map(|b| (b.id, m::Vec2::ZERO)).collect();
                let _ = ch::pose(&mut app.doc, id, &rot, &off, at, false);
            }
        });
        let name_id = egui::Id::unique(("pose-name", id));
        let mut name: String = ui.data(|d| d.get_temp(name_id)).unwrap_or_default();
        ui.horizontal_wrapped(|ui| {
            crate::kit::text_field(ui, &mut name, "Pose name", None, (w - 90.0).max(80.0), None);
            if Button::ghost("Save pose").show(ui).clicked() && !name.trim().is_empty() {
                let mut c = mc.clone();
                if let Some(s) = &mut c.skeleton {
                    s.poses.retain(|p| !p.name.eq_ignore_ascii_case(name.trim()));
                    s.poses.push(posed.pose_now(name.trim()));
                }
                set_clip(app, "Save pose", c);
                name.clear();
            }
        });
        ui.data_mut(|d| d.insert_temp(name_id, name));
        if !sk.poses.is_empty() {
            ui.horizontal_wrapped(|ui| {
                for p in &sk.poses {
                    if Button::ghost(&p.name).compact().show(ui).on_hover_text(crate::i18n::shown("Apply at the playhead")).clicked() {
                        let targets = sk.pose_targets(p);
                        let rot: Vec<(u32, f64)> = targets.iter().map(|x| (x.0, x.1)).collect();
                        let off: Vec<(u32, m::Vec2)> = targets.iter().filter_map(|x| x.2.map(|o| (x.0, o))).collect();
                        let _ = ch::pose(&mut app.doc, id, &rot, &off, at, true);
                    }
                }
            });
        }
        // Parts and swap sets.
        let split = app.doc.model.active().clips.iter().any(|c| c.meta.get("part").and_then(|p| p.get("rig")).and_then(|r| r.as_u64()) == Some(id as u64));
        if !split && Button::secondary("Split into parts").icon(Icon::Intersect).show(ui).on_hover_text(crate::i18n::shown("Head, torso, arms, legs, tail, eyes and mouth onto their own layers, each following its bone")).clicked() {
            if let Err(e) = ch::split_layers(&mut app.doc, id, at, true) {
                app.doc.error = Some(e);
            }
        }
        for (set, options) in ch::swap_sets(&app.doc, id) {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(&set).font(theme::body()).color(t.text_2));
                for o in &options {
                    if Button::ghost(o).compact().show(ui).on_hover_text(crate::i18n::shown("Show from the playhead")).clicked() {
                        let _ = ch::swap(&mut app.doc, id, &set, &[(at as f64 / SEC as f64, o.clone())]);
                    }
                }
            });
        }
        // IK handles.
        for h in &sk.ik {
            let mut live = h.enabled;
            crate::kit::labeled_row(ui, &crate::i18n::tf(format!("IK {}", if h.name.is_empty() { h.id.to_string() } else { h.name.clone() })), |ui| {
                if crate::kit::toggle(ui, &mut live, "Live IK", None).changed() {
                    let mut c = mc.clone();
                    if let Some(x) = c.skeleton.as_mut().and_then(|s| s.handle_mut(h.id)) {
                        x.enabled = live;
                        // A live handle starts where the limb now reaches.
                        if live {
                            if let Some(pb) = posed.forward().bone(h.bone) {
                                x.target = pb.tail;
                            }
                        }
                    }
                    set_clip(app, "IK", c);
                }
            });
            if h.enabled {
                super::pins::model_row(app, ui, id, &format!("ik.{}.target", h.id), Some(0), "Target X", -50.0..=150.0, h.target.x as f32, "%", 100.0);
                super::pins::model_row(app, ui, id, &format!("ik.{}.target", h.id), Some(1), "Target Y", -50.0..=150.0, h.target.y as f32, "%", 100.0);
                super::pins::model_row(app, ui, id, &format!("ik.{}.blend", h.id), None, "Blend", 0.0..=100.0, 1.0, "%", 100.0);
                ik_settings(app, ui, &mc, &posed, h);
            }
        }
        // Bones: the picked one first.
        let sel = picked(&ctx);
        let mut order: Vec<&m::Bone> = sk.bones.iter().collect();
        order.sort_by_key(|b| Some(b.id) != sel);
        for b in order {
            let label = if b.name.is_empty() { crate::i18n::tf(format!("Bone {}", b.id)) } else { b.name.clone() };
            let strong = Some(b.id) == sel;
            crate::kit::name_row(ui, egui::RichText::new(&label).font(if strong { theme::body_medium() } else { theme::body() }).color(if strong { t.text } else { t.text_2 }), |ui| {
                {
                    if Button::ghost("").icon(Icon::Trash).compact().show(ui).on_hover_text(crate::i18n::shown("Remove bone")).clicked() {
                        let mut c = mc.clone();
                        if let Some(s) = &mut c.skeleton {
                            let up = s.bone(b.id).and_then(|x| x.parent);
                            for x in &mut s.bones {
                                if x.parent == Some(b.id) {
                                    x.parent = up;
                                }
                            }
                            s.bones.retain(|x| x.id != b.id);
                            s.ik.retain(|h| h.bone != b.id);
                        }
                        c.keyframes.retain(|k, _| !k.starts_with(&format!("bone.{}.", b.id)));
                        set_clip(app, "Remove bone", c);
                    }
                }
            });
            let lo = b.min.unwrap_or(-360.0) as f32;
            let hi = b.max.unwrap_or(360.0) as f32;
            super::pins::model_row(app, ui, id, &format!("bone.{}.rotation", b.id), None, "Rotation", lo..=hi, 0.0, "°", 1.0);
            joint_limits(app, ui, &mc, b);
            if b.parent.is_none() {
                super::pins::model_row(app, ui, id, &format!("bone.{}.offset", b.id), Some(0), "Shift X", -100.0..=100.0, 0.0, "%", 100.0);
                super::pins::model_row(app, ui, id, &format!("bone.{}.offset", b.id), Some(1), "Shift Y", -100.0..=100.0, 0.0, "%", 100.0);
            }
        }
        let _ = rg::KINDS;
    });
    if out.reset {
        let mut c = mc.clone();
        c.skeleton = None;
        c.keyframes.retain(|k, _| !k.starts_with("bone.") && !k.starts_with("ik."));
        set_clip(app, "Remove skeleton", c);
        set_tool(&ctx, BoneTool::Off);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_skeleton_section_and_bone_tool_draw_without_panicking() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = StudioApp::new(&cc, crate::cli::Cli { screen: Some(crate::cli::Screen::Editor), fresh: true, ..Default::default() });
        let (doc, id) = crate::services::character::tests::figure_doc();
        app.doc = doc;
        app.editor.selection = vec![id];
        let start = app.doc.project.clip(id).unwrap().start;
        ch::build(&mut app.doc, id, "biped", Some(Some(true)), start).unwrap();
        let before = app.doc.model.active().clip(m::ClipId(id as u64)).cloned();
        let mut frame = eframe::Frame::_new_kittest();
        for (k, on) in [BoneTool::Off, BoneTool::Pose, BoneTool::Draw].into_iter().enumerate() {
            set_tool(&ctx, on);
            for i in 0..3 {
                let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1600.0, 1000.0))), time: Some((k * 3 + i) as f64 / 60.0), ..Default::default() };
                let mut out = ctx.run_ui(input, |ui| eframe::App::ui(&mut app, ui, &mut frame));
                out.textures_delta.clear();
            }
        }
        // Drawing changed nothing.
        assert_eq!(app.doc.model.active().clip(m::ClipId(id as u64)).cloned(), before);
    }
}
