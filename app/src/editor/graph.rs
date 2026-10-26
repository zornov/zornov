// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The graph editor: the selected clip's animated values as curves over
//! the timeline's own time axis. Drag a key to retime it and change its
//! value; drag the handles of an eased key to shape the curve; right-click
//! a key for its curve; double-click a curve to add a key; Delete removes
//! the selected key. "Normalise" fits every curve to the same height so
//! curves of different units can be compared; "Frame" fits the clip.

use super::timeline::View;
use crate::app::StudioApp;
use crate::kit::{self, IconButton, MenuItem, ShowNoted};
use crate::theme;
use crate::view::{SEC, Time};
use eframe::egui::{self, Align2, Color32, Pos2, Rect, Sense, Stroke, Vec2};
use studio_model as m;

fn on_id() -> egui::Id {
    egui::Id::unique("graph-editor-on")
}

/// Whether the timeline shows the graph editor.
pub(super) fn is_on(ctx: &egui::Context) -> bool {
    ctx.data_mut(|d| *d.get_persisted_mut_or_default::<bool>(on_id()))
}

pub(super) fn toggle(ctx: &egui::Context) {
    let on = is_on(ctx);
    ctx.data_mut(|d| d.insert_persisted(on_id(), !on));
}

/// One curve: a track's component.
#[derive(Clone, Debug, PartialEq)]
pub struct Curve {
    pub label: String,
    pub path: String,
    /// The component of a point track (0 x, 1 y); `None` for numbers.
    pub comp: Option<usize>,
    /// Shown units per stored unit, and the unit.
    pub scale: f64,
    pub unit: &'static str,
}

const COLOURS: [Color32; 8] = [
    Color32::from_rgb(240, 98, 88),
    Color32::from_rgb(98, 200, 120),
    Color32::from_rgb(96, 150, 250),
    Color32::from_rgb(240, 190, 70),
    Color32::from_rgb(190, 120, 240),
    Color32::from_rgb(70, 210, 210),
    Color32::from_rgb(250, 140, 200),
    Color32::from_rgb(200, 200, 200),
];

/// A name a person reads for a model path.
pub fn label(mc: &m::Clip, path: &str) -> String {
    let words = |s: &str| {
        let s = s.replace('_', " ");
        let mut c = s.chars();
        c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
    };
    if let Some(rest) = path.strip_prefix("effect.")
        && let Some((id, k)) = rest.split_once('.')
    {
        let name = mc.effects.iter().find(|e| e.id.0.to_string() == id).map(|e| e.kind.rsplit('.').next().unwrap_or(&e.kind).to_owned()).unwrap_or_else(|| "Effect".into());
        return format!("{} {}", words(&name), k.replace('_', " "));
    }
    // A bone or IK handle by its name ("thigh.front rotation").
    if let Some(rest) = path.strip_prefix("bone.").or_else(|| path.strip_prefix("ik."))
        && let Some((id, k)) = rest.split_once('.')
        && let Some(sk) = &mc.skeleton
    {
        let n = id.parse::<u32>().ok();
        let name = if path.starts_with("bone.") { n.and_then(|i| sk.bone(i)).map(|b| b.name.clone()) } else { n.and_then(|i| sk.handle(i)).map(|h| crate::i18n::tf(format!("IK {}", h.name))) };
        return format!("{} {k}", name.unwrap_or_else(|| id.to_owned()));
    }
    let tail = path.rsplit_once('.').map_or(path, |(_, b)| b);
    match path {
        "transform.position_z" => "Depth".into(),
        "transform.scale_x" => "Scale X".into(),
        "transform.scale_y" => "Scale Y".into(),
        p if p.starts_with("grade.") => crate::i18n::tf(format!("Grade {}", tail.replace('_', " "))),
        _ => words(tail),
    }
}

/// The curves a clip's tracks give (numbers and points; colours,
/// sound and text are left out).
pub fn curves(mc: &m::Clip) -> Vec<Curve> {
    let mut out = Vec::new();
    for (path, tr) in &mc.keyframes {
        if path.starts_with("audio.") {
            continue;
        }
        let Some(first) = tr.keys.first() else { continue };
        let percent = matches!(path.as_str(), "transform.opacity" | "transform.scale_x" | "transform.scale_y" | "transform.scale_z") || path.starts_with("transform.crop_");
        let (scale, unit) = match (&first.value, percent) {
            (_, true) => (100.0, "%"),
            (m::Value::Angle(_), _) => (1.0, "°"),
            (m::Value::Point(_), _) => (1.0, "px"),
            _ => (1.0, ""),
        };
        match &first.value {
            m::Value::Scalar(_) | m::Value::Angle(_) => out.push(Curve { label: label(mc, path), path: path.clone(), comp: None, scale, unit }),
            m::Value::Point(_) => {
                for (c, axis) in ["X", "Y"].iter().enumerate() {
                    out.push(Curve { label: format!("{} {axis}", label(mc, path)), path: path.clone(), comp: Some(c), scale, unit });
                }
            }
            _ => {}
        }
    }
    out
}

/// One component of a value.
pub fn comp(v: &m::Value, c: Option<usize>) -> Option<f64> {
    match (v, c) {
        (m::Value::Point(p), Some(1)) => Some(p.y),
        (m::Value::Point(p), _) => Some(p.x),
        (v, _) => v.as_f64(),
    }
}

/// `v` with one component set to `x`.
pub fn with_comp(v: &m::Value, c: Option<usize>, x: f64) -> m::Value {
    match (v, c) {
        (m::Value::Point(p), Some(1)) => m::Value::Point(m::Vec2 { x: p.x, y: x }),
        (m::Value::Point(p), _) => m::Value::Point(m::Vec2 { x, y: p.y }),
        (m::Value::Angle(_), _) => m::Value::Angle(x),
        _ => m::Value::Scalar(x),
    }
}

/// Moves key `i` of `tr` to `time` (kept strictly between its
/// neighbours) and gives component `c` the value `x`.
pub fn move_key(tr: &mut m::Track, i: usize, time: Time, c: Option<usize>, x: f64) {
    let lo = if i == 0 { Time::MIN / 4 } else { tr.keys[i - 1].time + 1 };
    let hi = if i + 1 == tr.keys.len() { Time::MAX / 4 } else { tr.keys[i + 1].time - 1 };
    let k = &mut tr.keys[i];
    k.time = time.clamp(lo, hi.max(lo));
    k.value = with_comp(&k.value, c, x);
}

/// Sets the handle leaving key `i` (`out`) or arriving at it (`!out`)
/// from a point (time, value) the person dragged it to.
pub fn set_handle(tr: &mut m::Track, i: usize, out: bool, c: Option<usize>, time: Time, x: f64) {
    let (a, b) = if out { (i, i + 1) } else { (i - 1, i) };
    if b >= tr.keys.len() {
        return;
    }
    let (t0, t1) = (tr.keys[a].time, tr.keys[b].time);
    let (v0, v1) = (comp(&tr.keys[a].value, c).unwrap_or(0.0), comp(&tr.keys[b].value, c).unwrap_or(0.0));
    let dt = (t1 - t0).max(1) as f64;
    let dv = v1 - v0;
    // Value fractions only mean something when the value changes.
    let fy = |v: f64, from: f64| if dv.abs() > 1e-9 { (v - from) / dv } else { 0.0 };
    if out {
        let h = m::Handle { x: ((time - t0) as f64 / dt).clamp(0.0, 1.0), y: fy(x, v0) };
        tr.keys[a].ease_out = h;
        tr.keys[a].interp = m::Interp::Ease;
    } else {
        tr.keys[b].ease_in = m::Handle { x: ((t1 - time) as f64 / dt).clamp(0.0, 1.0), y: -fy(x, v1) };
        tr.keys[a].interp = m::Interp::Ease;
    }
}

/// Where the handles of the segment from key `i` sit: (out of i, in of
/// i + 1) as (time, value).
pub fn handle_points(tr: &m::Track, i: usize, c: Option<usize>) -> Option<((Time, f64), (Time, f64))> {
    let (k0, k1) = (tr.keys.get(i)?, tr.keys.get(i + 1)?);
    let (v0, v1) = (comp(&k0.value, c)?, comp(&k1.value, c)?);
    let dt = (k1.time - k0.time) as f64;
    let dv = v1 - v0;
    let p1 = (k0.time + (k0.ease_out.x * dt) as Time, v0 + k0.ease_out.y * dv);
    let p2 = (k1.time - (k1.ease_in.x * dt) as Time, v1 - k1.ease_in.y * dv);
    Some((p1, p2))
}

#[derive(Clone)]
enum Grab {
    Key,
    Out,
    In,
}

#[derive(Clone)]
struct Drag {
    curve: Curve,
    key: usize,
    grab: Grab,
    origin: m::Clip,
    /// The value range of the plot when the drag began.
    range: (f64, f64),
}

#[derive(Clone, Default)]
struct State {
    hidden: Vec<(String, Option<usize>)>,
    normalise: bool,
    /// (path, component, key index)
    selected: Option<(String, Option<usize>, usize)>,
}

/// Draws the graph editor in `area` (left: the curve list in `list`;
/// right: the plot in `plot`), on the timeline's time axis `view`.
pub(super) fn show(app: &mut StudioApp, ui: &mut egui::Ui, list: Rect, plot: Rect, view: &View) {
    let t = theme::tokens(ui.ctx());
    let p = ui.painter().with_clip_rect(plot.intersect(ui.clip_rect()));
    let Some(&id) = app.editor.selection.first() else {
        kit::paint_text_clipped(&p, plot.center(), Align2::CENTER_CENTER, "Select a clip to see its animation as curves", theme::body(), t.text_2, plot.width() - 24.0);
        return;
    };
    let Some(mc) = app.doc.model.active().clip(m::ClipId(id as u64)).cloned() else { return };
    let all = curves(&mc);
    let sid = egui::Id::unique(("graph-state", id));
    let mut st: State = ui.data(|d| d.get_temp(sid)).unwrap_or_default();
    // The curve list: more curves than fit above the buttons scroll.
    let row_h = 20.0;
    let rows = Rect::from_min_max(egui::pos2(list.left(), list.top() + 6.0), egui::pos2(list.right(), list.bottom() - 34.0));
    let mut lui = ui.new_child(egui::UiBuilder::new().max_rect(rows).id_salt(("graph-list", id)));
    lui.set_clip_rect(rows.intersect(ui.clip_rect()));
    egui::ScrollArea::vertical().id_salt(("graph-list-scroll", id)).auto_shrink([false, false]).show_v(&mut lui, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        for (n, c) in all.iter().enumerate() {
            let (full, _) = ui.allocate_exact_size(Vec2::new(rows.width(), row_h), Sense::hover());
            let r = Rect::from_min_size(egui::pos2(full.left() + 6.0, full.top()), Vec2::new(full.width() - 10.0, row_h));
            let key = (c.path.clone(), c.comp);
            let hidden = st.hidden.contains(&key);
            let resp = ui.interact(r, egui::Id::unique(("graph-row", id, n)), Sense::click()).on_hover_text(crate::i18n::shown("Show or hide this curve"));
            if resp.clicked() {
                if hidden {
                    st.hidden.retain(|h| *h != key);
                } else {
                    st.hidden.push(key);
                }
            }
            let col = COLOURS[n % COLOURS.len()];
            ui.painter().circle_filled(egui::pos2(r.left() + 6.0, r.center().y), 4.0, if hidden { theme::alpha(col, 0.25) } else { col });
            kit::paint_text_clipped(ui.painter(), egui::pos2(r.left() + 16.0, r.center().y), Align2::LEFT_CENTER, &c.label, theme::caption(), if hidden { t.text_2 } else { t.text }, r.width() - 18.0);
        }
    });
    if all.is_empty() {
        kit::paint_text_clipped(&p, plot.center(), Align2::CENTER_CENTER, "This clip has no keyframes yet: key a value in the Inspector, or ask the AI", theme::body(), t.text_2, plot.width() - 24.0);
    }
    // Normalise and frame.
    let mut bar = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_max(egui::pos2(list.left() + 4.0, list.bottom() - 30.0), list.max)).layout(egui::Layout::left_to_right(egui::Align::Center)));
    if IconButton::new(crate::icons::Icon::ArrowsOutCardinal, "Normalise: every curve the same height").active(st.normalise).show(&mut bar).clicked() {
        st.normalise = !st.normalise;
    }
    if IconButton::new(crate::icons::Icon::CornersOut, "Frame the clip").show(&mut bar).clicked() {
        let secs = (mc.duration.max(1) as f64 / SEC as f64) as f32;
        app.editor.px_per_sec = ((plot.width() - 48.0) / secs).clamp(2.0, 600.0);
        app.editor.tl_scroll.x = ((mc.start as f64 / SEC as f64) as f32 * app.editor.px_per_sec - 24.0).max(0.0);
        app.editor.auto_fit = false;
    }
    let visible: Vec<(usize, &Curve)> = all.iter().enumerate().filter(|(_, c)| !st.hidden.contains(&(c.path.clone(), c.comp))).collect();

    // Sample every curve (with its modifiers) across the clip.
    let x_lo = view.x(mc.start).max(plot.left());
    let x_hi = view.x(mc.start + mc.duration).min(plot.right());
    let step = 2.0;
    let samples: Vec<Vec<(f32, f64)>> = visible
        .iter()
        .map(|(_, c)| {
            let mut out = Vec::new();
            let mut x = x_lo;
            while x <= x_hi {
                let local = (view.t(x) - mc.start).clamp(0, mc.duration);
                if let Some(v) = mc.property_at(&c.path, local).and_then(|v| comp(&v, c.comp)) {
                    out.push((x, v));
                }
                x += step;
            }
            out
        })
        .collect();
    let drag_id = egui::Id::unique(("graph-drag", id));
    let dragging: Option<Drag> = ui.data(|d| d.get_temp(drag_id));
    // Each curve's range: its own when normalised, all together otherwise;
    // held still while a drag is on so the curve doesn't swim.
    let range_of = |vals: &mut dyn Iterator<Item = f64>| {
        let (lo, hi) = vals.fold((f64::MAX, f64::MIN), |(a, b), v| (a.min(v), b.max(v)));
        if lo > hi {
            (0.0, 1.0)
        } else if (hi - lo).abs() < 1e-9 {
            (lo - 1.0, hi + 1.0)
        } else {
            let pad = (hi - lo) * 0.08;
            (lo - pad, hi + pad)
        }
    };
    let key_vals = |c: &Curve| mc.keyframes.get(&c.path).map(|tr| tr.keys.iter().filter_map(|k| comp(&k.value, c.comp)).collect::<Vec<_>>()).unwrap_or_default();
    let shared = range_of(&mut samples.iter().flatten().map(|s| s.1).chain(visible.iter().flat_map(|(_, c)| key_vals(c))));
    let ranges: Vec<(f64, f64)> = visible
        .iter()
        .zip(&samples)
        .map(|((_, c), s)| match &dragging {
            Some(d) if d.curve == **c => d.range,
            _ if st.normalise => range_of(&mut s.iter().map(|x| x.1).chain(key_vals(c))),
            _ => shared,
        })
        .collect();
    let inner = plot.shrink2(Vec2::new(0.0, 14.0));
    let y_of = |v: f64, r: (f64, f64)| inner.bottom() - inner.height() * ((v - r.0) / (r.1 - r.0)) as f32;
    let v_of = |y: f32, r: (f64, f64)| r.0 + (inner.bottom() - y) as f64 / inner.height() as f64 * (r.1 - r.0);

    // Grid: the clip's span and a zero line when it is in view.
    p.rect_filled(Rect::from_min_max(egui::pos2(x_lo, plot.top()), egui::pos2(x_hi, plot.bottom())), 0, theme::alpha(t.text, 0.03));
    if !st.normalise && shared.0 < 0.0 && shared.1 > 0.0 {
        p.hline(plot.x_range(), y_of(0.0, shared), Stroke::new(1.0, theme::alpha(t.text_2, 0.4)));
    }
    if !st.normalise && let Some((_, c)) = visible.first() {
        for k in [0.0, 0.5, 1.0] {
            let v = shared.0 + (shared.1 - shared.0) * k;
            p.text(egui::pos2(plot.left() + 4.0, y_of(v, shared)), Align2::LEFT_CENTER, format!("{:.1}{}", v * c.scale, c.unit), theme::caption(), theme::alpha(t.text_2, 0.8));
        }
    }

    let fps = (app.doc.project.fps as f64).max(1.0);
    // The nearest frame, by its number (frame lengths are not whole
    // microseconds).
    let on_frame = |t: Time| ((t as f64 * fps / SEC as f64).round() * SEC as f64 / fps).round() as Time;
    let pointer = ui.input(|i| i.pointer.interact_pos());
    // While the button is down, what is under the press picks: a drag
    // only starts once the pointer has left the key a little.
    let probe = ui.input(|i| i.pointer.press_origin().filter(|_| i.pointer.any_down())).or(pointer);
    let mut hot: Option<(Curve, usize, Grab, (f64, f64))> = None;
    // Where each key was drawn this frame (tests drive the pointer by it).
    let mut drawn: Vec<(String, Option<usize>, usize, Pos2)> = Vec::new();
    for (n, ((ci, c), s)) in visible.iter().zip(&samples).enumerate() {
        let col = COLOURS[ci % COLOURS.len()];
        let r = ranges[n];
        let pts: Vec<Pos2> = s.iter().map(|(x, v)| egui::pos2(*x, y_of(*v, r))).collect();
        p.add(egui::Shape::line(pts, Stroke::new(1.6, col)));
        let Some(tr) = mc.keyframes.get(&c.path) else { continue };
        for (i, k) in tr.keys.iter().enumerate() {
            let Some(v) = comp(&k.value, c.comp) else { continue };
            let at = egui::pos2(view.x(mc.start + k.time), y_of(v, r));
            let selected = st.selected.as_ref() == Some(&(c.path.clone(), c.comp, i));
            if selected {
                // Its handles, on each eased side.
                for (seg, out) in [(Some(i), true), (i.checked_sub(1), false)] {
                    let Some(seg) = seg else { continue };
                    if tr.keys[seg].interp != m::Interp::Ease {
                        continue;
                    }
                    let Some((p1, p2)) = handle_points(tr, seg, c.comp) else { continue };
                    let (ht, hv) = if out { p1 } else { p2 };
                    let hp = egui::pos2(view.x(mc.start + ht), y_of(hv, r));
                    p.line_segment([at, hp], Stroke::new(1.0, theme::alpha(col, 0.7)));
                    p.circle_filled(hp, 3.5, col);
                    if probe.is_some_and(|q| q.distance(hp) < 7.0) {
                        hot = Some(((*c).clone(), i, if out { Grab::Out } else { Grab::In }, r));
                    }
                }
            }
            drawn.push((c.path.clone(), c.comp, i, at));
            let s = if selected { 5.0 } else { 4.0 };
            p.add(egui::Shape::convex_polygon(vec![at + Vec2::new(0.0, -s), at + Vec2::new(s, 0.0), at + Vec2::new(0.0, s), at + Vec2::new(-s, 0.0)], if selected { t.select } else { Color32::WHITE }, Stroke::new(1.0, col)));
            if hot.is_none() && probe.is_some_and(|q| q.distance(at) < 7.0) {
                hot = Some(((*c).clone(), i, Grab::Key, r));
            }
        }
    }

    // Pointer: pick, drag, add, curve menu.
    let resp = ui.interact(plot, egui::Id::unique(("graph-plot", id)), Sense::click_and_drag());
    let seq = app.doc.seq();
    if resp.drag_started()
        && let Some((c, i, grab, r)) = hot.clone()
    {
        st.selected = Some((c.path.clone(), c.comp, i));
        ui.data_mut(|d| d.insert_temp(drag_id, Drag { curve: c, key: i, grab, origin: mc.clone(), range: r }));
    } else if resp.clicked() {
        st.selected = hot.as_ref().map(|(c, i, _, _)| (c.path.clone(), c.comp, *i));
    }
    if resp.dragged()
        && let (Some(d), Some(q)) = (dragging.clone(), pointer)
    {
        let mut changed = d.origin.clone();
        if let Some(tr) = changed.keyframes.get_mut(&d.curve.path) {
            let raw = view.t(q.x) - mc.start;
            let snapped = on_frame(raw);
            let v = v_of(q.y, d.range);
            match d.grab {
                Grab::Key => {
                    // Shift holds the time still.
                    let time = if ui.input(|i| i.modifiers.shift) { tr.keys[d.key].time } else { snapped };
                    move_key(tr, d.key, time, d.curve.comp, v);
                }
                Grab::Out => set_handle(tr, d.key, true, d.curve.comp, raw, v),
                Grab::In => set_handle(tr, d.key, false, d.curve.comp, raw, v),
            }
            if let Some(k) = tr.keys.get(d.key) {
                let tip = format!("{:.2}s  {:.1}{}", (mc.start + k.time) as f64 / SEC as f64, comp(&k.value, d.curve.comp).unwrap_or(0.0) * d.curve.scale, d.curve.unit);
                ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Tooltip, drag_id)).text(q + Vec2::new(12.0, -14.0), Align2::LEFT_BOTTOM, tip, theme::caption(), t.text);
            }
        }
        app.doc.apply_gesture(&format!("graph-{id}"), "Edit curve", m::Edit::SetClip { seq, clip: Box::new(changed) });
    }
    if resp.drag_stopped() {
        ui.data_mut(|d| d.remove::<Drag>(drag_id));
    }
    // Double-click on a curve: a key there.
    if resp.double_clicked()
        && hot.is_none()
        && let Some(q) = pointer
    {
        let local = (view.t(q.x) - mc.start).clamp(0, mc.duration);
        let local = on_frame(local);
        let near = visible.iter().zip(&ranges).find(|((_, c), r)| mc.property_at(&c.path, local).and_then(|v| comp(&v, c.comp)).is_some_and(|v| (y_of(v, **r) - q.y).abs() < 8.0));
        if let Some(((_, c), _)) = near
            && let Some(tr) = mc.keyframes.get(&c.path)
            && tr.key_at(local).is_none()
            && let Some(v) = tr.eval(local)
        {
            let mut changed = mc.clone();
            let interp = tr.keys.iter().rev().find(|k| k.time < local).map_or(m::Interp::Ease, |k| k.interp);
            if let Some(t2) = changed.keyframes.get_mut(&c.path) {
                t2.set_key(m::Keyframe::new(local, v, interp));
            }
            app.doc.apply("Add keyframe", m::Edit::SetClip { seq, clip: Box::new(changed) });
        }
    }
    // Delete the selected key.
    if resp.hovered()
        && !ui.ctx().egui_wants_keyboard_input()
        && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Delete) || i.consume_key(egui::Modifiers::NONE, egui::Key::Backspace))
        && let Some((path, _, i)) = st.selected.take()
    {
        let mut changed = mc.clone();
        if let Some(tr) = changed.keyframes.get_mut(&path)
            && i < tr.keys.len()
        {
            let k = tr.keys.remove(i);
            if tr.keys.is_empty() {
                changed.keyframes.remove(&path);
                changed.set_property(&path, k.value);
            }
            app.doc.apply("Delete keyframe", m::Edit::SetClip { seq, clip: Box::new(changed) });
        }
    }
    // Right-click a key: its curve into the next key.
    let menu_id = egui::Id::unique(("graph-menu", id));
    if resp.secondary_clicked()
        && let Some((c, i, Grab::Key, _)) = hot.clone()
    {
        st.selected = Some((c.path.clone(), c.comp, i));
        kit::set_open(ui.ctx(), menu_id, true);
    }
    let anchor = Rect::from_center_size(pointer.unwrap_or(plot.center()), Vec2::splat(2.0));
    let mut pick: Option<&str> = None;
    kit::popover(ui.ctx(), menu_id, anchor, kit::Side::Below, 200.0, |ui| {
        kit::menu_heading(ui, "Into the next key");
        for (label, name) in super::keys::CURVES {
            if MenuItem::new(label).show(ui).clicked() {
                pick = Some(name);
            }
        }
    });
    if let (Some(name), Some((path, _, i))) = (pick, st.selected.clone()) {
        kit::set_open(ui.ctx(), menu_id, false);
        if let Some((interp, out, inn, shape)) = crate::agent::curve(name) {
            let mut changed = mc.clone();
            if let Some(tr) = changed.keyframes.get_mut(&path)
                && i < tr.keys.len()
            {
                tr.keys[i].interp = interp;
                tr.keys[i].curve = shape;
                if let Some(h) = out {
                    tr.keys[i].ease_out = h;
                }
                if let (Some(h), Some(next)) = (inn, tr.keys.get_mut(i + 1)) {
                    next.ease_in = h;
                }
                app.doc.apply("Keyframe curve", m::Edit::SetClip { seq, clip: Box::new(changed) });
            }
        }
    }
    // The selected key's numbers.
    if let Some((path, c, i)) = &st.selected
        && let Some(k) = mc.keyframes.get(path).and_then(|tr| tr.keys.get(*i))
        && let Some(cv) = all.iter().find(|x| x.path == *path && x.comp == *c)
    {
        let txt = format!("{}  {:.2}s  {:.2}{}", cv.label, (mc.start + k.time) as f64 / SEC as f64, comp(&k.value, *c).unwrap_or(0.0) * cv.scale, cv.unit);
        p.text(egui::pos2(plot.left() + 6.0, plot.bottom() - 4.0), Align2::LEFT_BOTTOM, txt, theme::caption(), t.text);
    }
    if hot.is_some() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
    }
    ui.data_mut(|d| {
        d.insert_temp(sid, st);
        d.insert_temp(egui::Id::unique("graph-drawn"), drawn);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track() -> m::Track {
        m::Track::new(vec![m::Keyframe::new(0, 0.0, m::Interp::Ease), m::Keyframe::new(1_000_000, 100.0, m::Interp::Linear), m::Keyframe::new(2_000_000, 50.0, m::Interp::Linear)])
    }

    #[test]
    fn keys_move_between_their_neighbours() {
        let mut tr = track();
        move_key(&mut tr, 1, 5_000_000, None, 80.0);
        assert_eq!(tr.keys[1].time, 1_999_999, "stays before the next key");
        assert_eq!(tr.keys[1].value, m::Value::Scalar(80.0));
        move_key(&mut tr, 0, -500, None, 10.0);
        assert_eq!(tr.keys[0].time, -500);
        assert!(tr.validate().is_ok());
    }

    #[test]
    fn handles_round_trip_through_their_points() {
        let mut tr = track();
        set_handle(&mut tr, 0, true, None, 300_000, 0.0);
        set_handle(&mut tr, 1, false, None, 800_000, 100.0);
        assert!((tr.keys[0].ease_out.x - 0.3).abs() < 1e-9 && tr.keys[0].ease_out.y.abs() < 1e-9);
        assert!((tr.keys[1].ease_in.x - 0.2).abs() < 1e-9 && tr.keys[1].ease_in.y.abs() < 1e-9);
        let ((t1, v1), (t2, v2)) = handle_points(&tr, 0, None).unwrap();
        assert_eq!((t1, t2), (300_000, 800_000));
        assert!(v1.abs() < 1e-9 && (v2 - 100.0).abs() < 1e-9);
        // Pulled up past the target: an overshoot.
        set_handle(&mut tr, 0, true, None, 300_000, 150.0);
        assert!((tr.keys[0].ease_out.y - 1.5).abs() < 1e-9);
        assert!(tr.eval(400_000).unwrap().as_f64().unwrap() > 60.0);
    }

    #[test]
    fn points_give_two_curves_and_audio_none() {
        let mut mc = m::Clip::new(m::ClipId(1), m::LaneId(1), 0, 2_000_000, m::ClipSource::Gap);
        mc.keyframes.insert("transform.position".into(), m::Track::new(vec![m::Keyframe::new(0, m::Vec2::new(1.0, 2.0), m::Interp::Linear)]));
        mc.keyframes.insert("transform.opacity".into(), m::Track::new(vec![m::Keyframe::new(0, 1.0, m::Interp::Linear)]));
        mc.keyframes.insert("audio.volume_db".into(), m::Track::new(vec![m::Keyframe::new(0, -6.0, m::Interp::Linear)]));
        let cs = curves(&mc);
        let names: Vec<&str> = cs.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(names, vec!["Opacity", "Position X", "Position Y"]);
        assert_eq!(cs[0].scale, 100.0);
        let v = with_comp(&m::Value::Point(m::Vec2::new(1.0, 2.0)), Some(1), 9.0);
        assert_eq!(comp(&v, Some(1)), Some(9.0));
        assert_eq!(comp(&v, Some(0)), Some(1.0));
    }

    /// The whole editor, headless: the graph editor shows the selected
    /// clip's curves, a key dragged up gets a bigger value, a
    /// double-click on the curve adds a key.
    #[test]
    fn the_graph_editor_drags_and_adds_keys() {
        use crate::app::StudioApp;
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = StudioApp::new(&cc, crate::cli::Cli { screen: Some(crate::cli::Screen::Editor), fresh: true, ..Default::default() });
        let mut s = crate::agent::Session::new(crate::services::project::new_custom("Graph", 640, 360, 30.0));
        s.edit(&[serde_json::json!({"op": "card", "colour": "#ffcc00", "at": 0, "duration": 4, "width": 200, "height": 100}), serde_json::json!({"op": "animate", "clip": "$1", "property": "rotation", "keys": [[0, 0], [2, 90]], "ease": "linear"})], "anim", false).unwrap();
        let id = s.doc.project.clips.iter().map(|c| c.id).max().unwrap();
        app.doc = s.doc;
        app.editor.selection = vec![id];
        toggle(&ctx);
        let mut frame = eframe::Frame::_new_kittest();
        let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(1600.0, 1000.0));
        let mut clock = 0.0;
        let mut run = |app: &mut StudioApp, events: Vec<egui::Event>| {
            clock += 1.0 / 60.0;
            let input = egui::RawInput { screen_rect: Some(screen), time: Some(clock), events, ..Default::default() };
            crate::editor::test_frame(&ctx, input, |ui| eframe::App::ui(app, ui, &mut frame));
        };
        for _ in 0..3 {
            run(&mut app, vec![]);
        }
        let drawn: Vec<(String, Option<usize>, usize, Pos2)> = ctx.data(|d| d.get_temp(egui::Id::unique("graph-drawn"))).unwrap_or_default();
        let key = drawn.iter().find(|k| k.0 == "transform.rotation" && k.2 == 1).expect("the second rotation key is drawn").3;
        let press = |pos: Pos2, pressed: bool| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
        run(&mut app, vec![egui::Event::PointerMoved(key)]);
        run(&mut app, vec![press(key, true)]);
        for k in 1..=6 {
            run(&mut app, vec![egui::Event::PointerMoved(key - Vec2::new(0.0, 5.0 * k as f32))]);
        }
        run(&mut app, vec![press(key - Vec2::new(0.0, 30.0), false)]);
        run(&mut app, vec![]);
        let mc = app.doc.model.active().clip(m::ClipId(id as u64)).unwrap().clone();
        let tr = &mc.keyframes["transform.rotation"];
        let v = tr.keys[1].value.as_f64().unwrap();
        assert!(v > 91.0, "dragged up, the value grew: {v}");
        assert_eq!(tr.keys[1].time, 2_000_000, "a straight-up drag keeps it on its frame");
        assert_eq!(app.doc.undo_label(), Some("Edit curve"));
        // A double-click on the curve halfway between the keys.
        let k0 = drawn.iter().find(|k| k.0 == "transform.rotation" && k.2 == 0).unwrap().3;
        let mid_x = (k0.x + key.x) / 2.0;
        let drawn: Vec<(String, Option<usize>, usize, Pos2)> = ctx.data(|d| d.get_temp(egui::Id::unique("graph-drawn"))).unwrap_or_default();
        let k1 = drawn.iter().find(|k| k.0 == "transform.rotation" && k.2 == 1).unwrap().3;
        let mid = Pos2::new(mid_x, (k0.y + k1.y) / 2.0);
        run(&mut app, vec![egui::Event::PointerMoved(mid)]);
        for _ in 0..2 {
            run(&mut app, vec![press(mid, true)]);
            run(&mut app, vec![press(mid, false)]);
        }
        run(&mut app, vec![]);
        let n = app.doc.model.active().clip(m::ClipId(id as u64)).unwrap().keyframes["transform.rotation"].keys.len();
        assert_eq!(n, 3, "a key was added on the curve");
    }
}
