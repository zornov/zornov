// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The Grade panel (Colour workspace, a tab beside the scopes): lift /
//! gamma / gain wheels, curves, HSL qualifier presets with a window, a LUT,
//! the ASC CDL, the stills row and the project's colour management. It
//! edits the selected clip's model grade through `services::grade`; each
//! change is one undo step, and a drag is one step in all.

use crate::app::StudioApp;
use crate::kit::{self, Segment};
use crate::services::{self, grade as sg};
use crate::theme;
use eframe::egui::{self, Color32, Pos2, Sense, Stroke, Ui, Vec2};
use studio_model as m;

/// What the user did this frame: the undo label and, for a drag, the key
/// that merges its changes into one step.
#[derive(Default)]
struct Change {
    label: &'static str,
    gesture: Option<String>,
}

impl Change {
    fn set(&mut self, label: &'static str, gesture: Option<String>) {
        self.label = label;
        self.gesture = gesture;
    }
    fn any(&self) -> bool {
        !self.label.is_empty()
    }
}

/// The part of the panel drawn: the clip's grade, or the project colour.
fn state_id() -> egui::Id {
    egui::Id::unique("grade-panel-state")
}

#[derive(Clone, Default)]
struct Ui_ {
    curve: usize,
    node: usize,
    still: String,
}

/// The Grade panel in `ui`.
pub fn panel(app: &mut StudioApp, ui: &mut Ui) {
    let id = app.editor.selection.first().copied();
    let ctx = ui.ctx().clone();
    let mut st = ctx.data_mut(|d| d.get_temp::<Ui_>(state_id())).unwrap_or_default();
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        ui.add_space(theme::space::XS);
        let graded = id.and_then(|id| sg::clip_grade(&app.doc, id).ok().map(|(_, c, g)| (id, c, g)));
        match graded {
            None => {
                ui.label(egui::RichText::new(crate::i18n::shown("Select a clip to grade it")).font(theme::body()).color(theme::tokens(&ctx).text_2));
            }
            Some((id, clip, grade)) => clip_controls(app, ui, &mut st, id, clip, grade),
        }
        ui.add_space(theme::space::SM);
        project_colour(app, ui, id);
    });
    ctx.data_mut(|d| d.insert_temp(state_id(), st));
}

fn clip_controls(app: &mut StudioApp, ui: &mut Ui, st: &mut Ui_, id: u32, clip: m::Clip, grade: m::Grade) {
    let mut ch = Change::default();
    let mut c = clip.clone();
    let mut g = grade.clone();
    let mut nodes = g.nodes();

    let mut on = g.enabled;
    ui.horizontal(|ui| {
        if kit::toggle(ui, &mut on, "Grade on", None).changed() {
            g.enabled = on;
            ch.set(if on { "Turn on grade" } else { "Turn off grade" }, None);
        }
        ui.label(egui::RichText::new(crate::i18n::shown("Grade")).font(theme::body_medium()));
        if kit::Button::ghost("Reset grade").compact().show(ui).clicked() {
            g.params.clear();
            g.lut = None;
            g.set_nodes(&[]);
            nodes.clear();
            c.masks.retain(|mk| !grade.window_masks().contains(&mk.id));
            ch.set("Reset grade", None);
        }
    });

    kit::Section::new("grade-wheels", "Wheels").resettable(sg::WHEELS.iter().any(|(n, _)| sg::wheel_set(&g.params, n))).show(ui, |ui| {
        wheels(ui, id, &mut g.params, &mut ch);
    });
    kit::Section::new("grade-curves", "Curves").resettable(sg::CURVES.iter().any(|(k, _)| g.params.contains_key(*k))).show(ui, |ui| {
        curves(ui, st, id, &mut g.params, &mut ch);
    });
    kit::Section::new("grade-qualifier", "Qualifier and window").start_closed().show(ui, |ui| {
        qualifier(app, ui, st, id, &mut c, &mut nodes, &mut ch);
    });
    kit::Section::new("grade-lut", "LUT").start_closed().show(ui, |ui| {
        lut(app, ui, &mut g, &mut ch);
    });
    kit::Section::new("grade-cdl", "CDL").start_closed().resettable(sg::cdl_set(&g.params)).show(ui, |ui| {
        cdl(app, ui, id, &mut g.params, &mut ch);
    });
    kit::Section::new("grade-stills", "Stills").start_closed().show(ui, |ui| {
        stills(app, ui, st, id, &mut c, &mut g, &mut ch);
    });

    if ch.any() {
        g.set_nodes(&nodes);
        c.grade = Some(g);
        sg::put_clip(&mut app.doc, id, ch.label, ch.gesture.as_deref(), c);
    }
}

// ---------------------------------------------------------------- wheels

/// A colour wheel: hue is the angle, amount the distance from the centre.
fn wheel_disc(ui: &mut Ui, size: f32, w: &mut [f64; 3]) -> bool {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click_and_drag());
    let t = theme::tokens(ui.ctx());
    let c = rect.center();
    let r = size / 2.0 - 2.0;
    let p = ui.painter();
    for i in 0..48 {
        let a0 = i as f32 / 48.0 * std::f32::consts::TAU;
        let a1 = (i + 1) as f32 / 48.0 * std::f32::consts::TAU;
        let col = egui::ecolor::Hsva::new(i as f32 / 48.0, 0.45, 0.55, 1.0);
        let pts = vec![c, c + Vec2::angled(-a0) * r, c + Vec2::angled(-a1) * r];
        p.add(egui::Shape::convex_polygon(pts, Color32::from(col), Stroke::NONE));
    }
    p.circle_filled(c, r * 0.12, Color32::from_black_alpha(200));
    p.circle_stroke(c, r, Stroke::new(1.0, t.line_strong));
    let dot = c + Vec2::angled(-(w[0] as f32).to_radians()) * (w[1] as f32 * r);
    p.circle_filled(dot, 4.0, Color32::WHITE);
    p.circle_stroke(dot, 4.0, Stroke::new(1.0, Color32::BLACK));
    let mut changed = false;
    if let Some(pos) = resp.interact_pointer_pos().filter(|_| resp.dragged() || resp.clicked()) {
        let d = pos - c;
        let amount = (d.length() / r).clamp(0.0, 1.0) as f64;
        let hue = (-d.y.atan2(d.x)).to_degrees().rem_euclid(360.0) as f64;
        *w = [hue, amount, w[2]];
        changed = true;
    }
    if resp.double_clicked() {
        *w = [0.0, 0.0, w[2]];
        changed = true;
    }
    changed
}

fn wheels(ui: &mut Ui, id: u32, p: &mut m::ParamMap, ch: &mut Change) {
    let size = ((ui.available_width() - 16.0) / 3.0).clamp(60.0, 110.0);
    ui.horizontal_top(|ui| {
        for (name, label) in sg::WHEELS {
            ui.vertical(|ui| {
                ui.set_width(size);
                let mut w = sg::wheel(p, name);
                if wheel_disc(ui, size, &mut w) {
                    sg::set_wheel(p, name, w);
                    ch.set("Colour wheel", Some(format!("grade-{id}-wheel-{name}")));
                }
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(crate::i18n::shown(label)).font(theme::caption()));
                    if sg::wheel_set(p, name) && kit::Button::ghost("Reset").compact().show(ui).on_hover_text(crate::i18n::shown("Reset this wheel")).clicked() {
                        sg::set_wheel(p, name, [0.0, 0.0, 0.0]);
                        ch.set("Reset wheel", None);
                    }
                });
            });
        }
    });
    for (name, label) in sg::WHEELS {
        let mut w = sg::wheel(p, name);
        let mut v = w[2] as f32;
        let text = format!("{label} luma");
        let out = kit::field::FieldRow::new(&text, &mut v, -1.0..=1.0).default(0.0).decimals(2).step(0.01).scrub(0.005).show(ui);
        if out.changed {
            w[2] = v as f64;
            sg::set_wheel(p, name, w);
            ch.set("Colour wheel", Some(format!("grade-{id}-wheel-luma-{name}")));
        }
    }
}

// ---------------------------------------------------------------- curves

fn curves(ui: &mut Ui, st: &mut Ui_, id: u32, p: &mut m::ParamMap, ch: &mut Change) {
    let segs: Vec<Segment> = sg::CURVES.iter().map(|(_, l)| Segment::text(l)).collect();
    st.curve = st.curve.min(sg::CURVES.len() - 1);
    kit::segmented(ui, "grade-curve-which", &mut st.curve, &segs, Some(ui.available_width()), None);
    let (key, _) = sg::CURVES[st.curve];
    let side = ui.available_width().min(220.0);
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(side), Sense::click_and_drag());
    let t = theme::tokens(ui.ctx());
    let mut pts = sg::curve(p, key);
    let to_screen = |q: [f64; 2]| Pos2::new(rect.left() + q[0] as f32 * rect.width(), rect.bottom() - q[1] as f32 * rect.height());
    let from_screen = |pos: Pos2| [((pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0) as f64, ((rect.bottom() - pos.y) / rect.height()).clamp(0.0, 1.0) as f64];
    let near = |pos: Pos2, pts: &[[f64; 2]]| pts.iter().position(|q| to_screen(*q).distance(pos) < 9.0);

    let drag_id = egui::Id::unique("grade-curve-drag").with(key);
    let mut changed = false;
    if let Some(pos) = resp.interact_pointer_pos() {
        if resp.drag_started() || (resp.clicked() && !resp.dragged()) {
            let hit = near(pos, &pts).unwrap_or_else(|| {
                pts.push(from_screen(pos));
                pts.sort_by(|a, b| a[0].total_cmp(&b[0]));
                pts.iter().position(|q| to_screen(*q).distance(pos) < 1.5).unwrap_or(0)
            });
            ui.data_mut(|d| d.insert_temp(drag_id, hit));
            changed = true;
        }
        if resp.dragged() {
            if let Some(i) = ui.data(|d| d.get_temp::<usize>(drag_id)).filter(|i| *i < pts.len()) {
                let mut q = from_screen(pos);
                // Keep the order of the points: stay between the neighbours.
                let lo = if i > 0 { pts[i - 1][0] + 0.005 } else { 0.0 };
                let hi = if i + 1 < pts.len() { pts[i + 1][0] - 0.005 } else { 1.0 };
                q[0] = q[0].clamp(lo.min(hi), hi.max(lo));
                pts[i] = q;
                changed = true;
            }
        }
    }
    if resp.secondary_clicked() || resp.double_clicked() {
        if let Some(i) = resp.interact_pointer_pos().and_then(|pos| near(pos, &pts)) {
            if pts.len() > 2 {
                pts.remove(i);
                changed = true;
            }
        }
    }
    let painter = ui.painter();
    painter.rect_filled(rect, 4, Color32::from_black_alpha(160));
    for k in 1..4 {
        let f = k as f32 / 4.0;
        painter.hline(rect.x_range(), rect.top() + f * rect.height(), Stroke::new(1.0, t.line));
        painter.vline(rect.left() + f * rect.width(), rect.y_range(), Stroke::new(1.0, t.line));
    }
    painter.line_segment([to_screen([0.0, 0.0]), to_screen([1.0, 1.0])], Stroke::new(1.0, t.line_strong));
    let col = match st.curve {
        1 => Color32::from_rgb(235, 90, 90),
        2 => Color32::from_rgb(90, 200, 110),
        3 => Color32::from_rgb(100, 140, 240),
        _ => Color32::WHITE,
    };
    let line: Vec<Pos2> = pts.iter().map(|q| to_screen(*q)).collect();
    painter.add(egui::Shape::line(line.clone(), Stroke::new(1.5, col)));
    for q in &line {
        painter.circle_filled(*q, 3.5, col);
    }
    if changed {
        sg::set_curve(p, key, pts);
        ch.set("Curve", Some(format!("grade-{id}-{key}")));
    }
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(crate::i18n::shown("Click to add a point, double-click one to remove it")).font(theme::caption()).color(t.text_2));
        if p.contains_key(key) && kit::Button::ghost("Reset").compact().show(ui).clicked() {
            p.remove(key);
            ch.set("Reset curve", None);
        }
    });
}

// ------------------------------------------------- qualifier and window

fn qualifier(app: &mut StudioApp, ui: &mut Ui, st: &mut Ui_, id: u32, c: &mut m::Clip, nodes: &mut Vec<m::GradeNode>, ch: &mut Change) {
    let t = theme::tokens(ui.ctx());
    st.node = st.node.min(nodes.len().saturating_sub(1));
    ui.horizontal_wrapped(|ui| {
        for (i, n) in nodes.iter().enumerate() {
            let label = if n.name.is_empty() { format!("Node {}", i + 1) } else { n.name.clone() };
            if kit::chip(ui, &label, st.node == i, None).clicked() {
                st.node = i;
            }
        }
        if kit::Button::ghost("Add node").compact().show(ui).clicked() {
            nodes.push(m::GradeNode::default());
            st.node = nodes.len() - 1;
            ch.set("Add grade node", None);
        }
    });
    if nodes.is_empty() {
        ui.label(egui::RichText::new(crate::i18n::shown("A node grades part of the picture: pick a colour range or a window.")).font(theme::caption()).color(t.text_2));
        return;
    }
    let i = st.node;
    ui.label(egui::RichText::new(crate::i18n::shown("Colour range")).font(theme::caption()).color(t.text_2));
    let current = sg::qualifier_preset(&nodes[i]);
    ui.horizontal_wrapped(|ui| {
        for p in sg::QUALIFIER_PRESETS {
            let mut label = String::from(*p);
            if let Some(f) = label.get_mut(0..1) {
                f.make_ascii_uppercase();
            }
            if kit::chip(ui, &label, current.as_deref() == Some(*p), None).clicked() {
                if sg::set_qualifier_preset(&mut nodes[i], p).is_ok() {
                    ch.set("Colour range", None);
                }
            }
        }
        if (current.is_some() || !nodes[i].qualifier.is_empty()) && kit::Button::ghost("Whole picture").compact().show(ui).clicked() {
            nodes[i].qualifier.clear();
            ch.set("Colour range", None);
        }
    });
    ui.label(egui::RichText::new(crate::i18n::shown("Window")).font(theme::caption()).color(t.text_2));
    let shape = sg::node_window_shape(c, &nodes[i]);
    ui.horizontal_wrapped(|ui| {
        if kit::chip(ui, "None", shape.is_none(), None).clicked() && shape.is_some() {
            sg::set_node_window(c, nodes, i, None, &mut || app.doc.alloc_id());
            ch.set("Remove window", None);
        }
        for (label, s) in sg::WINDOW_SHAPES {
            if kit::chip(ui, label, shape == Some(*s), None).clicked() && shape != Some(*s) {
                sg::set_node_window(c, nodes, i, Some(*s), &mut || app.doc.alloc_id());
                ch.set("Window shape", None);
            }
        }
    });
    if shape.is_some() {
        let mut inv = nodes[i].window_invert;
        if ui.checkbox(&mut inv, crate::i18n::shown("Grade outside the window")).changed() {
            nodes[i].window_invert = inv;
            ch.set("Window", None);
        }
    }
    let mut mix = (nodes[i].mix * 100.0) as f32;
    let out = kit::field::FieldRow::new("Mix", &mut mix, 0.0..=100.0).default(100.0).unit("%").show(ui);
    if out.changed {
        nodes[i].mix = (mix as f64 / 100.0).clamp(0.0, 1.0);
        ch.set("Node mix", Some(format!("grade-{id}-node-{i}-mix")));
    }
    let mut sat = sg::scalar(&nodes[i].params, "saturation", 0.0) as f32;
    let out = kit::field::FieldRow::new("Saturation", &mut sat, -1.0..=1.0).default(0.0).decimals(2).step(0.01).scrub(0.005).show(ui);
    if out.changed {
        sg::set_scalar(&mut nodes[i].params, "saturation", sat as f64, 0.0);
        ch.set("Node saturation", Some(format!("grade-{id}-node-{i}-sat")));
    }
    let mut exp = sg::scalar(&nodes[i].params, "exposure", 0.0) as f32;
    let out = kit::field::FieldRow::new("Exposure", &mut exp, -2.0..=2.0).default(0.0).decimals(2).step(0.01).scrub(0.01).show(ui);
    if out.changed {
        sg::set_scalar(&mut nodes[i].params, "exposure", exp as f64, 0.0);
        ch.set("Node exposure", Some(format!("grade-{id}-node-{i}-exp")));
    }
    ui.horizontal(|ui| {
        let mut on = nodes[i].enabled;
        if ui.checkbox(&mut on, crate::i18n::shown("On")).changed() {
            nodes[i].enabled = on;
            ch.set(if on { "Turn on node" } else { "Turn off node" }, None);
        }
        if kit::Button::ghost("Remove node").compact().show(ui).clicked() {
            sg::set_node_window(c, nodes, i, None, &mut || app.doc.alloc_id());
            nodes.remove(i);
            ch.set("Remove grade node", None);
        }
    });
}

// ------------------------------------------------------------------ LUT

fn lut(_app: &mut StudioApp, ui: &mut Ui, g: &mut m::Grade, ch: &mut Change) {
    let t = theme::tokens(ui.ctx());
    let name = g.lut.as_deref().map(|p| std::path::Path::new(p).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.to_string()));
    ui.label(egui::RichText::new(name.clone().unwrap_or_else(|| crate::i18n::shown("No LUT").to_string())).font(theme::body()).color(if name.is_some() { t.text } else { t.text_2 }));
    ui.horizontal(|ui| {
        if kit::Button::secondary("Load LUT…").compact().show(ui).clicked() {
            if let Some(path) = rfd::FileDialog::new().set_title("Load a LUT").add_filter("LUT", &["cube", "3dl"]).pick_file() {
                match studio_colour::read_lut(&path) {
                    Ok(_) => {
                        g.lut = Some(path.to_string_lossy().into_owned());
                        ch.set("Load LUT", None);
                        kit::push(ui.ctx(), format!("Loaded {}", path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()), None, None);
                    }
                    Err(e) => {
                        kit::push_error(ui.ctx(), format!("Could not load the LUT: {e}"));
                    }
                }
            }
        }
        if g.lut.is_some() && kit::Button::ghost("Remove LUT").compact().show(ui).clicked() {
            g.lut = None;
            ch.set("Remove LUT", None);
        }
        if kit::Button::ghost("Export LUT…").compact().show(ui).clicked() {
            if let Some(path) = rfd::FileDialog::new().set_title("Export the grade as a LUT").set_file_name("grade.cube").add_filter("LUT", &["cube"]).save_file() {
                match sg::export_lut(g, &path, 33, "Zornov grade") {
                    Ok(b) => {
                        let note = if b.windows > 0 { " (nodes with windows are not in it)" } else { "" };
                        kit::push(ui.ctx(), format!("Exported {}{note}", path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()), None, None);
                    }
                    Err(e) => {
                        kit::push_error(ui.ctx(), format!("Could not export the LUT: {e}"));
                    }
                }
            }
        }
    });
    if g.lut.is_some() {
        for (key, label, default) in [("lut_intensity", "Intensity", 1.0f32), ("lut_skin_protect", "Skin protect", 0.0)] {
            let mut v = sg::scalar(&g.params, key, default as f64) as f32;
            let out = kit::field::FieldRow::new(label, &mut v, 0.0..=1.0).default(default).decimals(2).step(0.01).scrub(0.005).show(ui);
            if out.changed {
                sg::set_scalar(&mut g.params, key, v as f64, default as f64);
                ch.set("LUT strength", Some(format!("grade-lut-{key}")));
            }
        }
    }
}

// ------------------------------------------------------------------ CDL

fn cdl(app: &mut StudioApp, ui: &mut Ui, id: u32, p: &mut m::ParamMap, ch: &mut Change) {
    let _ = app;
    for (group, range) in [("slope", 0.0..=3.0f32), ("offset", -0.5..=0.5), ("power", 0.2..=3.0)] {
        ui.label(egui::RichText::new(crate::i18n::shown(&format!("{}{}", &group[..1].to_uppercase(), &group[1..]))).font(theme::caption()).color(theme::tokens(ui.ctx()).text_2));
        for ch_name in ["r", "g", "b"] {
            let key = format!("cdl_{group}_{ch_name}");
            let default = sg::CDL_FIELDS.iter().find(|(k, _)| *k == key).map_or(0.0, |(_, d)| *d);
            let mut v = sg::scalar(p, &key, default) as f32;
            let label = ch_name.to_uppercase();
            let out = kit::field::FieldRow::new(&label, &mut v, range.clone()).default(default as f32).decimals(3).step(0.001).scrub(0.005).show(ui);
            if out.changed {
                sg::set_scalar(p, &key, v as f64, default);
                ch.set("CDL", Some(format!("grade-{id}-{key}")));
            }
        }
    }
    let mut sat = sg::scalar(p, "cdl_sat", 1.0) as f32;
    let out = kit::field::FieldRow::new("Saturation", &mut sat, 0.0..=2.0).default(1.0).decimals(2).step(0.01).scrub(0.005).show(ui);
    if out.changed {
        sg::set_scalar(p, "cdl_sat", sat as f64, 1.0);
        ch.set("CDL", Some(format!("grade-{id}-cdl_sat")));
    }
    ui.horizontal(|ui| {
        if kit::Button::ghost("Reset CDL").compact().show(ui).clicked() {
            for (k, _) in sg::CDL_FIELDS {
                p.remove(*k);
            }
            ch.set("Reset CDL", None);
        }
        if kit::Button::ghost("Import CDL…").compact().show(ui).clicked() {
            if let Some(path) = rfd::FileDialog::new().set_title("Import an ASC CDL").add_filter("ASC CDL", &["cdl", "cc", "xml"]).pick_file() {
                match std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|x| sg::cdl_from_xml(&x, &path.to_string_lossy())) {
                    Ok(v) => {
                        for (k, val) in v {
                            if let Some(n) = val.as_f64() {
                                p.insert(k, m::Value::Scalar(n));
                            }
                        }
                        ch.set("Import CDL", None);
                    }
                    Err(e) => {
                        kit::push_error(ui.ctx(), format!("Could not import the CDL: {e}"));
                    }
                }
            }
        }
        if kit::Button::ghost("Export CDL…").compact().show(ui).clicked() {
            if let Some(path) = rfd::FileDialog::new().set_title("Export the CDL").set_file_name("grade.cdl").add_filter("ASC CDL", &["cdl"]).save_file() {
                match std::fs::write(&path, sg::cdl_xml(&[(id, p.clone())])) {
                    Ok(()) => {
                        kit::push(ui.ctx(), format!("Exported {}", path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()), None, None);
                    }
                    Err(e) => {
                        kit::push_error(ui.ctx(), format!("Could not export the CDL: {e}"));
                    }
                }
            }
        }
    });
}

// ---------------------------------------------------------------- stills

/// The last rendered picture as a small JPEG for a still.
fn picture(app: &StudioApp) -> Option<Vec<u8>> {
    #[cfg(feature = "render")]
    {
        let (w, h, px, _) = app.render.as_ref()?.last_pixels.as_ref()?;
        let step = (*w as usize / 480).max(1);
        let (ow, oh) = (*w as usize / step, *h as usize / step);
        let mut out = Vec::with_capacity(ow * oh * 4);
        for y in 0..oh {
            for x in 0..ow {
                let i = ((y * step) * *w as usize + x * step) * 4;
                out.extend_from_slice(px.get(i..i + 4)?);
            }
        }
        Some(studio_content::jpeg::encode(&studio_media::RgbaImage { width: ow as u32, height: oh as u32, pixels: out }, 85))
    }
    #[cfg(not(feature = "render"))]
    {
        let _ = app;
        None
    }
}

fn stills(app: &mut StudioApp, ui: &mut Ui, st: &mut Ui_, id: u32, c: &mut m::Clip, g: &mut m::Grade, ch: &mut Change) {
    ui.horizontal(|ui| {
        kit::text_field(ui, &mut st.still, "Still name", None, (ui.available_width() - 90.0).max(60.0), None);
        let name = st.still.trim().to_string();
        let name = if name.is_empty() { format!("Still {}", services::grade_stills::list(&app.doc).len() + 1) } else { name };
        if kit::Button::secondary("Save still").compact().show(ui).clicked() {
            let jpeg = picture(app);
            let at = app.editor.playhead;
            match services::grade_stills::save(&app.doc, &name, g, jpeg.as_deref(), Some(id as u64), Some(at as f64 / 1_000_000.0)) {
                Ok(s) => {
                    kit::push(ui.ctx(), format!("Saved still {}", s.name), None, None);
                    st.still.clear();
                }
                Err(e) => {
                    kit::push_error(ui.ctx(), format!("Could not save the still: {e}"));
                }
            }
        }
    });
    let list = services::grade_stills::list(&app.doc);
    if list.is_empty() {
        ui.label(egui::RichText::new(crate::i18n::shown("Saved looks appear here to apply to other shots")).font(theme::caption()).color(theme::tokens(ui.ctx()).text_2));
    }
    for s in list {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(&s.name).font(theme::body()));
            if kit::Button::ghost("Apply").compact().show(ui).clicked() {
                let still = services::grade_stills::grade_for(&s);
                sg::apply_still_grade(c, &still);
                *g = c.grade.clone().unwrap_or_default();
                ch.set("Apply still", None);
            }
            if kit::Button::ghost("Remove").compact().show(ui).clicked() {
                match services::grade_stills::remove(&app.doc, &s.name) {
                    Ok(()) => {
                        kit::push(ui.ctx(), format!("Removed still {}", s.name), None, None);
                    }
                    Err(e) => {
                        kit::push_error(ui.ctx(), format!("Could not remove the still: {e}"));
                    }
                }
            }
        });
    }
}

// -------------------------------------------------------- project colour

const OUTPUTS: &[(&str, &str)] = &[("rec709", "Rec.709"), ("p3", "Display P3"), ("rec2020_pq", "Rec.2020 PQ"), ("rec2020_hlg", "Rec.2020 HLG")];

fn project_colour(app: &mut StudioApp, ui: &mut Ui, selected: Option<u32>) {
    kit::Section::new("grade-project-colour", "Project colour").start_closed().show(ui, |ui| {
        let st = app.doc.model.settings.clone();
        // Output space.
        let mut o = match st.colour_space {
            m::ColourSpace::Rec709 => 0,
            m::ColourSpace::DisplayP3 => 1,
            m::ColourSpace::Rec2020Pq => 2,
            m::ColourSpace::Rec2020Hlg => 3,
        };
        let segs: Vec<Segment> = OUTPUTS.iter().map(|(_, l)| Segment::text(l)).collect();
        ui.label(egui::RichText::new(crate::i18n::shown("Output")).font(theme::caption()).color(theme::tokens(ui.ctx()).text_2));
        if kit::segmented(ui, "grade-output-space", &mut o, &segs, Some(ui.available_width()), None).changed() {
            apply_settings(app, ui.ctx(), "Output colour space", |s| sg::set_output(s, OUTPUTS[o].0, None).map(|_| ()));
        }
        // HDR peak.
        let mut peak = st.colour.hdr_peak.unwrap_or(1000) as f32;
        if kit::field::FieldRow::new("HDR peak", &mut peak, 100.0..=10000.0).default(1000.0).unit("nits").show(ui).changed {
            apply_settings(app, ui.ctx(), "HDR peak", |s| {
                s.colour.hdr_peak = Some(peak.round() as u32);
                Ok(())
            });
        }
        // View transform.
        let view_id = egui::Id::unique("grade-view-text");
        let mut view = ui.data_mut(|d| d.get_temp::<String>(view_id)).unwrap_or_else(|| st.colour.view.clone());
        ui.label(egui::RichText::new(crate::i18n::shown("View transform (none, aces, ocio:display/view)")).font(theme::caption()).color(theme::tokens(ui.ctx()).text_2));
        let r = kit::text_field(ui, &mut view, "none", None, ui.available_width(), None);
        ui.data_mut(|d| d.insert_temp(view_id, view.clone()));
        if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) || r.lost_focus() && view != st.colour.view {
            let v = if view.trim().is_empty() { "none".to_string() } else { view.clone() };
            apply_settings(app, ui.ctx(), "View transform", |s| sg::set_view(s, &v, None));
            ui.data_mut(|d| d.remove::<String>(view_id));
        }
        // OCIO config.
        let name = st.colour.ocio_config.as_deref().map(|p| std::path::Path::new(p).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.to_string()));
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(name.clone().unwrap_or_else(|| crate::i18n::shown("No OCIO config").to_string())).font(theme::body()));
            if kit::Button::ghost("Choose OCIO config…").compact().show(ui).clicked() {
                if let Some(path) = rfd::FileDialog::new().set_title("Choose an OCIO config").add_filter("OCIO", &["ocio"]).pick_file() {
                    let p = path.to_string_lossy().into_owned();
                    apply_settings(app, ui.ctx(), "OCIO config", |s| sg::set_ocio(s, &p).map(|_| ()));
                }
            }
            if name.is_some() && kit::Button::ghost("Clear").compact().show(ui).clicked() {
                apply_settings(app, ui.ctx(), "OCIO config", |s| sg::set_ocio(s, "").map(|_| ()));
            }
        });
        // Input space of the selected clip's media.
        if let Some(media) = selected.and_then(|id| app.doc.project.clips.iter().find(|c| c.id == id)).and_then(|c| c.media) {
            let item = app.doc.model.media_item(m::MediaId(media as u64)).cloned();
            if let Some(mut item) = item {
                let id = egui::Id::unique("grade-input-text").with(media);
                let mut text = ui.data_mut(|d| d.get_temp::<String>(id)).unwrap_or_else(|| item.input_colour.clone().unwrap_or_default());
                ui.label(egui::RichText::new(crate::i18n::shown("Input colour space of this clip's media (empty: from the file)")).font(theme::caption()).color(theme::tokens(ui.ctx()).text_2));
                let r = kit::text_field(ui, &mut text, "from the file", None, ui.available_width(), None);
                ui.data_mut(|d| d.insert_temp(id, text.clone()));
                if r.lost_focus() && text.trim() != item.input_colour.clone().unwrap_or_default() {
                    let cfg = st.colour.ocio_config.as_deref().map(std::path::PathBuf::from);
                    let auto = matches!(text.trim().to_ascii_lowercase().as_str(), "" | "auto" | "file" | "default");
                    match studio_render::colour_space::InputColour::parse(text.trim(), cfg.as_deref()) {
                        Ok(_) => {
                            item.input_colour = (!auto).then(|| text.trim().to_string());
                            if app.doc.apply("Input colour space", m::Edit::SetMedia { item: Box::new(item) }) {
                                kit::push(ui.ctx(), "Set the input colour space", None, None);
                            }
                        }
                        Err(e) => {
                            kit::push_error(ui.ctx(), format!("Could not set the input colour space: {e}"));
                        }
                    }
                    ui.data_mut(|d| d.remove::<String>(id));
                }
            }
        }
    });
}

/// Runs `f` on a copy of the project settings and applies the result as one
/// undo step; a refusal becomes an error toast.
fn apply_settings(app: &mut StudioApp, ctx: &egui::Context, label: &str, f: impl FnOnce(&mut m::ProjectSettings) -> Result<(), String>) {
    let mut s = app.doc.model.settings.clone();
    match f(&mut s) {
        Ok(()) if s != app.doc.model.settings => {
            if app.doc.apply(label, m::Edit::SetSettings { settings: Box::new(s) }) {
                kit::push(ctx, format!("Changed {}", label.to_lowercase()), None, None);
            }
        }
        Ok(()) => {}
        Err(e) => {
            kit::push_error(ctx, format!("Could not change {}: {e}", label.to_lowercase()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Session;
    use crate::ui_tests::Harness;
    use eframe::egui::{Event, PointerButton, RawInput};

    /// An editor with one selected clip, and a way to draw the panel in it.
    struct Bench {
        h: Harness,
        id: u32,
        time: f64,
    }

    impl Bench {
        fn new() -> Self {
            let mut h = Harness::new(&["--screen", "editor"]);
            h.frames(2);
            let doc = std::mem::replace(&mut h.app.doc, Session::default().doc);
            let mut s = Session::new(doc);
            s.edit(&[serde_json::json!({"op": "card", "colour": "#336699", "at": 0.0, "duration": 3.0, "lane": "overlay"})], "card", false).expect("a card");
            let id = s.doc.model.active().clips.iter().map(|c| c.id.0).max().unwrap() as u32;
            h.app.doc = s.doc;
            h.app.editor.selection = vec![id];
            Bench { h, id, time: 10.0 }
        }

        fn open_all(&self) {
            for k in ["grade-wheels", "grade-curves", "grade-qualifier", "grade-lut", "grade-cdl", "grade-stills", "grade-project-colour"] {
                self.h.ctx.data_mut(|d| d.insert_persisted(egui::Id::unique(("section-collapsed", k)), false));
            }
        }

        fn frame(&mut self, events: Vec<Event>) -> egui::FullOutput {
            self.time += 1.0 / 60.0;
            let raw = RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(340.0, 3200.0))), time: Some(self.time), events, ..Default::default() };
            let app = &mut self.h.app;
            let mut out = self.h.ctx.run_ui(raw, |ui| panel(app, ui));
            out.textures_delta.clear();
            out
        }

        fn text(&mut self) -> String {
            let mut out = self.frame(vec![]);
            for _ in 0..40 {
                self.time += 0.05;
                out = self.frame(vec![]);
            }
            out.shapes.iter().filter_map(|c| if let egui::epaint::Shape::Text(t) = &c.shape { Some(t.galley.text().to_owned()) } else { None }).collect::<Vec<_>>().join("\n")
        }

        fn grade(&self) -> m::Grade {
            self.h.app.doc.model.active().clip(m::ClipId(self.id as u64)).unwrap().grade.clone().unwrap_or_default()
        }
    }

    fn press(pos: egui::Pos2) -> Vec<Event> {
        vec![Event::PointerMoved(pos), Event::PointerButton { pos, button: PointerButton::Primary, pressed: true, modifiers: Default::default() }]
    }

    fn release(pos: egui::Pos2) -> Vec<Event> {
        vec![Event::PointerMoved(pos), Event::PointerButton { pos, button: PointerButton::Primary, pressed: false, modifiers: Default::default() }]
    }

    #[test]
    fn the_panel_shows_wheels_curves_cdl_stills_and_project_colour() {
        let mut b = Bench::new();
        b.open_all();
        let text = b.text();
        for want in ["Wheels", "Lift", "Gamma", "Gain", "Lift luma", "Curves", "Master", "Qualifier and window", "Add node", "LUT", "Load LUT…", "CDL", "Import CDL…", "Stills", "Save still", "Project colour", "Output", "HDR peak", "Choose OCIO config…"] {
            assert!(text.contains(want), "\"{want}\" is in the Grade panel:\n{text}");
        }
    }

    #[test]
    fn with_nothing_selected_it_says_so_and_still_offers_project_colour() {
        let mut b = Bench::new();
        b.h.app.editor.selection.clear();
        let text = b.text();
        assert!(text.contains("Select a clip to grade it") && text.contains("Project colour"), "{text}");
        assert!(!text.contains("Wheels"), "{text}");
    }

    #[test]
    fn a_wheel_drag_is_one_undoable_step_that_marks_the_project_dirty() {
        let mut b = Bench::new();
        b.open_all();
        b.text();
        let before = b.h.app.doc.undo_label().map(str::to_owned);
        let saved = b.h.app.doc.dirty();
        // The first wheel is the first disc under the toggle row: find it as the
        // largest square of the 48 wedge polygons.
        let out = b.frame(vec![]);
        let mut wedge = None;
        let mut n = 0;
        for c in &out.shapes {
            if let egui::epaint::Shape::Path(p) = &c.shape {
                if p.points.len() == 3 {
                    let r = egui::Rect::from_points(&p.points);
                    wedge = Some(wedge.map_or(r, |w: egui::Rect| w.union(r)));
                    n += 1;
                    if n == 48 {
                        break;
                    }
                }
            }
        }
        let disc = wedge.expect("a wheel was drawn");
        let c = disc.center();
        let r = disc.width() / 2.0;
        b.frame(press(c + egui::vec2(r * 0.5, 0.0)));
        b.frame(vec![Event::PointerMoved(c + egui::vec2(r * 0.6, 0.0))]);
        b.frame(vec![Event::PointerMoved(c + egui::vec2(0.0, -r * 0.4))]);
        b.frame(release(c + egui::vec2(0.0, -r * 0.4)));
        // The app's own frame loop ends a gesture when the pointer is up.
        b.h.app.doc.end_gesture();
        let g = b.grade();
        let lift = sg::wheel(&g.params, "lift");
        assert!(lift[1] > 0.1 && lift[1] < 0.6, "lift amount {lift:?}");
        assert!((lift[0] - 90.0).abs() < 25.0, "hue points up {lift:?}");
        assert!(b.h.app.doc.dirty());
        let _ = saved;
        assert_eq!(b.h.app.doc.undo_label(), Some("Colour wheel"));
        assert_ne!(b.h.app.doc.undo_label().map(str::to_owned), before);
        // One undo step takes the whole drag back.
        b.h.app.doc.undo();
        assert!(!sg::wheel_set(&b.grade().params, "lift"));
    }

    #[test]
    fn a_change_is_one_step_and_a_drag_of_it_merges() {
        let mut b = Bench::new();
        let id = b.id;
        let doc = &mut b.h.app.doc;
        for v in [0.1, 0.2, 0.3] {
            assert!(sg::edit_grade(doc, id, "Colour wheel", Some("grade-test"), |_, g| sg::set_wheel(&mut g.params, "gain", [30.0, v, 0.0])));
        }
        doc.end_gesture();
        assert_eq!(doc.undo_label(), Some("Colour wheel"));
        doc.undo();
        assert!(!sg::wheel_set(&b.grade().params, "gain"), "the three drag moves were one step");
        assert!(sg::edit_grade(&mut b.h.app.doc, id, "Reset wheel", None, |_, g| sg::set_wheel(&mut g.params, "gain", [0.0, 0.0, 0.2])));
        assert!(!sg::edit_grade(&mut b.h.app.doc, id, "Reset wheel", None, |_, g| sg::set_wheel(&mut g.params, "gain", [0.0, 0.0, 0.2])), "no change, no step");
    }

    #[test]
    fn the_project_colour_change_is_one_undo_step() {
        let mut b = Bench::new();
        assert!(sg::edit_settings(&mut b.h.app.doc, "Output colour space", |s| {
            sg::set_output(s, "p3", None).unwrap();
        }));
        assert_eq!(b.h.app.doc.model.settings.colour_space, m::ColourSpace::DisplayP3);
        b.h.app.doc.undo();
        assert_eq!(b.h.app.doc.model.settings.colour_space, m::ColourSpace::Rec709);
    }

    #[test]
    fn a_node_with_a_window_and_a_range_survives_undo_and_redo_together_with_its_mask() {
        let mut b = Bench::new();
        let id = b.id;
        let mut next = b.h.app.doc.alloc_id();
        assert!(sg::edit_grade(&mut b.h.app.doc, id, "Add grade node", None, |c, g| {
            let mut nodes = vec![m::GradeNode::default()];
            sg::set_qualifier_preset(&mut nodes[0], "sky").unwrap();
            sg::set_node_window(c, &mut nodes, 0, Some(m::MaskShape::Ellipse), &mut || {
                next += 1;
                next
            });
            g.set_nodes(&nodes);
        }));
        let mc = b.h.app.doc.model.active().clip(m::ClipId(id as u64)).unwrap().clone();
        assert_eq!(mc.masks.len(), 1);
        assert_eq!(mc.grade.as_ref().unwrap().nodes().len(), 1);
        b.h.app.doc.undo();
        let mc = b.h.app.doc.model.active().clip(m::ClipId(id as u64)).unwrap().clone();
        assert!(mc.masks.is_empty() && mc.grade.map_or(true, |g| g.nodes().is_empty()));
        b.h.app.doc.redo();
        let mc = b.h.app.doc.model.active().clip(m::ClipId(id as u64)).unwrap().clone();
        assert_eq!((mc.masks.len(), mc.grade.unwrap().nodes().len()), (1, 1));
    }

    #[test]
    fn the_scopes_panel_has_a_grade_tab_beside_the_scopes() {
        let mut b = Bench::new();
        let mut shown = |b: &mut Bench| {
            b.time += 1.0;
            let raw = RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(340.0, 700.0))), time: Some(b.time), ..Default::default() };
            let app = &mut b.h.app;
            let mut out = b.h.ctx.run_ui(raw, |ui| {
                let r = ui.max_rect();
                super::super::scopes::docked(app, ui, r);
            });
            out.textures_delta.clear();
            out.shapes.iter().filter_map(|c| if let egui::epaint::Shape::Text(t) = &c.shape { Some(t.galley.text().to_owned()) } else { None }).collect::<Vec<_>>().join("\n")
        };
        super::super::scopes::set_grade_tab(&b.h.ctx, false);
        let text = shown(&mut b);
        assert!(text.contains("Scopes") && text.contains("Grade") && text.contains("Waveform") && !text.contains("Wheels"), "{text}");
        super::super::scopes::set_grade_tab(&b.h.ctx, true);
        let text = shown(&mut b);
        assert!(text.contains("Scopes") && text.contains("Grade") && text.contains("Wheels") && !text.contains("Waveform"), "{text}");
    }
}
