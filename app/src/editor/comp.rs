// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The compositor: the selected clip's node graph (`services::vfx`), in
//! the timeline's place. Nodes are boxes with their inputs on top and the
//! result at the bottom; drag a node to move it, drag from a node's
//! bottom port to another's input to connect, drag an input away to
//! disconnect; the selected node's settings show on the right. "View"
//! makes the selected node the picture (the output); Delete removes it.
//! Scroll to zoom, drag the background to pan.

use crate::app::StudioApp;
use crate::theme;
use eframe::egui::{self, Align2, Color32, Pos2, Rect, Sense, Stroke, Vec2};
use studio_fx::comp::{Graph, NodeCategory, NodeId, json_of, nodes};
use studio_fx::params::{ParamDef, ParamKind, ParamValue};
use studio_model as m;
use crate::kit::{self, ShowNoted};

fn on_id() -> egui::Id {
    egui::Id::unique("comp-editor-on")
}

/// Whether the timeline shows the compositor.
pub(super) fn is_on(ctx: &egui::Context) -> bool {
    ctx.data_mut(|d| *d.get_persisted_mut_or_default::<bool>(on_id()))
}

pub(super) fn toggle(ctx: &egui::Context) {
    let on = is_on(ctx);
    ctx.data_mut(|d| d.insert_persisted(on_id(), !on));
}

/// View state of one clip's graph.
#[derive(Clone, Debug, Default)]
pub struct State {
    pub pan: Vec2,
    pub zoom: f32,
    pub selected: Option<NodeId>,
    /// A connection being dragged from a node's output.
    pub wiring: Option<NodeId>,
}

/// What a frame of the editor did to the graph.
#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    None,
    /// A finished edit, one undo step.
    Edit(&'static str),
    /// A value being dragged: merged into one undo step per gesture.
    Live(&'static str),
}

const NODE_W: f32 = 132.0;
const NODE_H: f32 = 34.0;
const PORT: f32 = 5.0;

fn colour_of(c: NodeCategory) -> Color32 {
    match c {
        NodeCategory::Input => Color32::from_rgb(110, 160, 110),
        NodeCategory::Merge => Color32::from_rgb(90, 130, 200),
        NodeCategory::Transform => Color32::from_rgb(150, 120, 200),
        NodeCategory::Colour => Color32::from_rgb(210, 160, 80),
        NodeCategory::Filter => Color32::from_rgb(120, 170, 190),
        NodeCategory::Keyer => Color32::from_rgb(90, 190, 120),
        NodeCategory::Matte => Color32::from_rgb(200, 200, 200),
        NodeCategory::Channel => Color32::from_rgb(190, 110, 150),
        NodeCategory::Lens => Color32::from_rgb(220, 120, 90),
        NodeCategory::Cleanup => Color32::from_rgb(160, 150, 110),
    }
}

impl State {
    fn scale(&self) -> f32 {
        if self.zoom <= 0.0 { 1.0 } else { self.zoom }
    }

    fn to_screen(&self, area: Rect, p: [f32; 2]) -> Pos2 {
        area.center() + self.pan + Vec2::new(p[0], p[1]) * self.scale()
    }

    fn node_rect(&self, area: Rect, p: [f32; 2]) -> Rect {
        Rect::from_center_size(self.to_screen(area, p), Vec2::new(NODE_W, NODE_H) * self.scale())
    }
}

/// Where input `slot` of a node of `n` inputs sits.
fn in_port(r: Rect, slot: usize, n: usize) -> Pos2 {
    let k = (slot as f32 + 1.0) / (n as f32 + 1.0);
    Pos2::new(r.left() + r.width() * k, r.top())
}

fn out_port(r: Rect) -> Pos2 {
    Pos2::new(r.center().x, r.bottom())
}

/// Draws and edits `g` inside `area`. Pure over the graph so it can run
/// headless in tests.
pub fn graph_ui(ui: &mut egui::Ui, area: Rect, g: &mut Graph, st: &mut State) -> Change {
    let t = theme::tokens(ui.ctx());
    let mut change = Change::None;
    let painter = ui.painter().with_clip_rect(area.intersect(ui.clip_rect()));
    painter.rect_filled(area, 0, t.sunken);
    // Background: pan and zoom.
    let bg = ui.interact(area, ui.id().with("comp-bg"), Sense::click_and_drag());
    if bg.dragged() && st.wiring.is_none() {
        st.pan += bg.drag_delta();
    }
    if bg.clicked() {
        st.selected = None;
    }
    if bg.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll.abs() > 0.0 {
            st.zoom = (st.scale() * (1.0 + scroll * 0.002)).clamp(0.3, 2.5);
        }
    }
    // Grid dots.
    let step = 40.0 * st.scale();
    let origin = area.center() + st.pan;
    let mut x = area.left() + (origin.x - area.left()).rem_euclid(step);
    while x < area.right() {
        let mut y = area.top() + (origin.y - area.top()).rem_euclid(step);
        while y < area.bottom() {
            painter.circle_filled(Pos2::new(x, y), 1.0, theme::alpha(t.text_2, 0.18));
            y += step;
        }
        x += step;
    }
    let out = g.output_id();
    // Wires first, under the nodes.
    for n in &g.nodes {
        let r = st.node_rect(area, n.pos);
        let count = n.def().map_or(0, |d| d.inputs.len());
        for (slot, i) in n.inputs.iter().enumerate() {
            let Some(src) = i.and_then(|i| g.node(i)) else { continue };
            let a = out_port(st.node_rect(area, src.pos));
            let b = in_port(r, slot, count);
            wire(&painter, a, b, Stroke::new(1.6, theme::alpha(t.text, 0.55)));
        }
        if let Some(mk) = n.mask.and_then(|m| g.node(m)) {
            let a = out_port(st.node_rect(area, mk.pos));
            wire(&painter, a, Pos2::new(r.right(), r.center().y), Stroke::new(1.2, theme::alpha(t.warn, 0.6)));
        }
    }
    // Nodes.
    let ids: Vec<NodeId> = g.nodes.iter().map(|n| n.id).collect();
    let mut drop_on: Option<(NodeId, usize)> = None;
    let pointer = ui.input(|i| i.pointer.interact_pos());
    for id in ids {
        let Some(n) = g.node(id).cloned() else { continue };
        let def = n.def();
        let r = st.node_rect(area, n.pos);
        let col = def.map_or(t.text_2, |d| colour_of(d.category));
        let resp = ui.interact(r, ui.id().with(("comp-node", id)), Sense::click_and_drag());
        if resp.clicked() || resp.drag_started() {
            st.selected = Some(id);
        }
        if resp.dragged() && st.wiring.is_none() {
            let d = resp.drag_delta() / st.scale();
            if let Some(nm) = g.node_mut(id) {
                nm.pos = [nm.pos[0] + d.x, nm.pos[1] + d.y];
            }
        }
        if resp.drag_stopped() {
            change = Change::Edit("Move node");
        }
        if resp.double_clicked() {
            g.output = Some(id);
            change = Change::Edit("View node");
        }
        let sel = st.selected == Some(id);
        painter.rect_filled(r, 6, if n.disabled { theme::alpha(t.raised, 0.5) } else { t.raised });
        painter.rect_filled(Rect::from_min_size(r.min, Vec2::new(5.0 * st.scale(), r.height())), 6, col);
        painter.rect_stroke(r, 6, Stroke::new(if sel { 2.0 } else { 1.0 }, if sel { t.select } else { t.line }), egui::StrokeKind::Inside);
        painter.text(r.center(), Align2::CENTER_CENTER, n.label(), theme::caption(), if n.disabled { t.text_2 } else { t.text });
        if Some(id) == out {
            painter.text(Pos2::new(r.right() + 4.0, r.center().y), Align2::LEFT_CENTER, "◀ view", theme::caption(), t.accent);
        }
        // Inputs: dropping a wire, or dragging one away to cut it.
        let count = def.map_or(0, |d| d.inputs.len());
        for slot in 0..count {
            let p = in_port(r, slot, count);
            painter.circle_filled(p, PORT, if n.input(slot).is_some() { t.text } else { t.text_2 });
            let pr = Rect::from_center_size(p, Vec2::splat(PORT * 3.0));
            if let Some(d) = def {
                if pr.contains(pointer.unwrap_or(Pos2::ZERO)) {
                    painter.text(p - Vec2::new(0.0, 8.0), Align2::CENTER_BOTTOM, d.inputs[slot], theme::caption(), t.text_2);
                }
            }
            if st.wiring.is_some() && pointer.is_some_and(|q| pr.contains(q)) {
                drop_on = Some((id, slot));
            }
            let pin = ui.interact(pr, ui.id().with(("comp-in", id, slot)), Sense::drag());
            if pin.drag_started() && n.input(slot).is_some() {
                // Pull the wire off and carry it.
                st.wiring = n.input(slot);
                if let Some(nm) = g.node_mut(id) {
                    nm.inputs[slot] = None;
                }
                change = Change::Edit("Disconnect");
            }
        }
        // Output: start a wire.
        let op = out_port(r);
        painter.circle_filled(op, PORT, col);
        let orr = Rect::from_center_size(op, Vec2::splat(PORT * 3.0));
        let po = ui.interact(orr, ui.id().with(("comp-out", id)), Sense::drag());
        if po.drag_started() {
            st.wiring = Some(id);
        }
    }
    // A wire in the air.
    if let Some(src) = st.wiring {
        if let (Some(s), Some(q)) = (g.node(src), pointer) {
            wire(&painter, out_port(st.node_rect(area, s.pos)), q, Stroke::new(1.6, t.accent));
        }
        if ui.input(|i| i.pointer.any_released()) {
            if let Some((dst, slot)) = drop_on {
                if dst != src {
                    let before = g.clone();
                    if let Some(nm) = g.node_mut(dst) {
                        while nm.inputs.len() <= slot {
                            nm.inputs.push(None);
                        }
                        nm.inputs[slot] = Some(src);
                    }
                    // Never a loop.
                    if g.check().is_err() {
                        *g = before;
                    } else {
                        change = Change::Edit("Connect");
                    }
                }
            }
            st.wiring = None;
        }
    }
    // Delete the selected node.
    if let Some(sel) = st.selected {
        if !ui.ctx().egui_wants_keyboard_input() && ui.input(|i| i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace)) && g.node(sel).is_some_and(|n| n.kind != "input") {
            g.remove(sel);
            st.selected = None;
            change = Change::Edit("Delete node");
        }
    }
    change
}

fn wire(p: &egui::Painter, a: Pos2, b: Pos2, stroke: Stroke) {
    let d = ((b.y - a.y).abs() * 0.5).max(30.0);
    let pts = [a, a + Vec2::new(0.0, d), b - Vec2::new(0.0, d), b];
    p.add(egui::epaint::CubicBezierShape::from_points_stroke(pts, false, Color32::TRANSPARENT, stroke));
}

/// The selected node's settings. Returns the change made.
pub fn settings_ui(ui: &mut egui::Ui, g: &mut Graph, st: &mut State, at_seconds: f64) -> Change {
    let mut change = Change::None;
    let Some(id) = st.selected else {
        ui.label(egui::RichText::new("Select a node to see its settings. Double-click a node to view it.").color(theme::tokens(ui.ctx()).text_2));
        return change;
    };
    let Some(n) = g.node(id).cloned() else { return change };
    ui.horizontal_wrapped(|ui| {
        ui.strong(n.label());
        ui.label(egui::RichText::new(n.def().map_or("", |d| d.name)).weak());
    });
    if let Some(d) = n.def() {
        ui.label(egui::RichText::new(d.description).small().weak());
    }
    let mut name = n.name.clone();
    ui.horizontal_wrapped(|ui| {
        ui.label("Name");
        if ui.text_edit_singleline(&mut name).lost_focus() && name != n.name {
            g.node_mut(id).unwrap().name = name.clone();
            change = Change::Edit("Rename node");
        }
    });
    ui.horizontal_wrapped(|ui| {
        if ui.button("View").on_hover_text("Show this node's picture").clicked() {
            g.output = Some(id);
            change = Change::Edit("View node");
        }
        let mut on = !n.disabled;
        if ui.checkbox(&mut on, "On").changed() {
            g.node_mut(id).unwrap().disabled = !on;
            change = Change::Edit("Node on/off");
        }
    });
    let mut mix = n.mix;
    if kit::Slider::new("Mix", &mut mix, 0.0..=1.0).default(1.0).decimals(2).with_label().with_value().show(ui).changed() {
        g.node_mut(id).unwrap().mix = mix;
        change = Change::Live("Node mix");
    }
    let values = n.params_at(at_seconds);
    for def in n.param_defs() {
        let keyed = n.keys.get(def.id).is_some_and(|k| !k.is_empty());
        let v = values.get(def.id).unwrap_or(def.default);
        if let Some(nv) = param_row(ui, def, v, keyed) {
            let node = g.node_mut(id).unwrap();
            node.params.insert(def.id.to_owned(), json_of(nv, def));
            // A typed value replaces the animation.
            node.keys.remove(def.id);
            change = Change::Live("Node setting");
        }
    }
    if !n.data.is_null() {
        ui.label(egui::RichText::new(format!("Data: {}", n.def().map_or("", |d| d.data))).small().weak());
    }
    change
}

fn param_row(ui: &mut egui::Ui, def: &ParamDef, v: ParamValue, keyed: bool) -> Option<ParamValue> {
    let mut out = None;
    ui.horizontal_wrapped(|ui| {
        ui.label(if keyed { format!("{} ◆", def.label) } else { def.label.to_owned() });
        match (def.kind, v) {
            (ParamKind::Float { min, max }, ParamValue::Float(mut f)) => {
                let wide = max - min > 1000.0;
                let r = kit::Slider::new(def.label, &mut f, min..=max).default(match def.default { ParamValue::Float(d) => d, _ => min }).decimals(if wide { 0 } else { 2 }).with_value().width(160.0).show(ui);
                if r.changed() {
                    out = Some(ParamValue::Float(f));
                }
            }
            (ParamKind::Int { min, max }, ParamValue::Int(mut i)) => {
                if kit::slider_i32(ui, def.label, &mut i, min..=max, Some(160.0)).changed() {
                    out = Some(ParamValue::Int(i));
                }
            }
            (ParamKind::Bool, ParamValue::Bool(mut b)) => {
                if ui.checkbox(&mut b, "").changed() {
                    out = Some(ParamValue::Bool(b));
                }
            }
            (ParamKind::Choice(names), ParamValue::Choice(c)) => {
                let mut sel = c as usize;
                egui::ComboBox::from_id_salt(def.id).selected_text(names.get(sel).copied().unwrap_or("")).show_ui(ui, |ui| {
                    crate::kit::note_list(ui);
                    for (i, nm) in names.iter().enumerate() {
                        if ui.selectable_value(&mut sel, i, *nm).changed() {
                            out = Some(ParamValue::Choice(i as u32));
                        }
                    }
                });
            }
            (ParamKind::Colour, ParamValue::Colour(c)) => {
                let mut rgba = egui::Rgba::from_rgba_unmultiplied(c[0], c[1], c[2], c[3]);
                if egui::widgets::color_picker::color_edit_button_rgba(ui, &mut rgba, egui::widgets::color_picker::Alpha::OnlyBlend).changed() {
                    let u = rgba.to_rgba_unmultiplied();
                    out = Some(ParamValue::Colour(u));
                }
            }
            (ParamKind::Point, ParamValue::Point(mut p)) => {
                let a = kit::Slider::new("X", &mut p[0], -1.0..=2.0).default(0.5).decimals(3).with_label().with_value().width(140.0).show(ui).changed();
                let b = kit::Slider::new("Y", &mut p[1], -1.0..=2.0).default(0.5).decimals(3).with_label().with_value().width(140.0).show(ui).changed();
                if a || b {
                    out = Some(ParamValue::Point(p));
                }
            }
            _ => {}
        }
    });
    out
}

/// The add-node menu: kinds by category. Returns the kind picked.
pub fn add_menu(ui: &mut egui::Ui) -> Option<&'static str> {
    let mut picked = None;
    ui.menu_button("Add node", |ui| {
        kit::popup_body(ui, |ui| {
            for cat in NodeCategory::ALL {
                ui.menu_button(cat.label(), |ui| {
                    kit::popup_body(ui, |ui| {
                        for d in nodes::all().iter().filter(|d| d.category == cat) {
                            if ui.button(d.name).on_hover_text(d.description).clicked() {
                                picked = Some(d.id);
                                ui.close();
                            }
                        }
                    })
                });
            }
        })
    });
    picked
}

/// Adds a node of `kind` after the selected node (or the output).
pub fn add_after(g: &mut Graph, st: &mut State, kind: &str) -> NodeId {
    let after = st.selected.or(g.output_id());
    let ins = match nodes::find(kind) {
        Some(d) if !d.inputs.is_empty() => vec![after],
        _ => Vec::new(),
    };
    let id = g.add(kind, &ins);
    if let (Some(a), Some(n)) = (after.and_then(|a| g.node(a)).map(|a| a.pos), g.node_mut(id)) {
        n.pos = [a[0] + if ins.is_empty() { 160.0 } else { 0.0 }, a[1] + 70.0];
    }
    st.selected = Some(id);
    id
}

/// The compositor in the timeline's place, on the selected clip.
pub(super) fn show(app: &mut StudioApp, ui: &mut egui::Ui, rect: Rect) {
    let t = theme::tokens(ui.ctx());
    let Some(&id) = app.editor.selection.first() else {
        ui.painter().text(rect.center(), Align2::CENTER_CENTER, "Select a clip (or an adjustment clip for the whole shot) to composite it with nodes", theme::body(), t.text_2);
        return;
    };
    let Some(mc) = app.doc.model.active().clip(m::ClipId(id as u64)).cloned() else { return };
    let existing = crate::services::vfx::graph_of(&mc).map(|(_, g)| g);
    let sid = egui::Id::unique(("comp-state", id));
    let mut st: State = ui.data(|d| d.get_temp(sid)).unwrap_or_default();
    let bar = Rect::from_min_size(rect.min, Vec2::new(rect.width(), 28.0));
    let side_w = 280.0f32.min(rect.width() * 0.4);
    let area = Rect::from_min_max(Pos2::new(rect.left(), bar.bottom()), Pos2::new(rect.right() - side_w, rect.bottom()));
    let side = Rect::from_min_max(Pos2::new(area.right(), bar.bottom()), rect.max);
    let Some(mut g) = existing.clone() else {
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink(12.0)));
        child.label(egui::RichText::new(format!("{} has no node graph yet.", mc.name)).color(t.text));
        if child.button("Composite this clip with nodes").clicked() {
            let mut g = Graph::passthrough();
            g.input_media = crate::services::vfx::input_media(&app.doc, &mc);
            if let Err(e) = crate::services::vfx::store_graph(&mut app.doc, id, &g, "Comp") {
                app.doc.error = Some(e);
            }
        }
        return;
    };
    let mut bar_ui = ui.new_child(egui::UiBuilder::new().max_rect(bar).layout(egui::Layout::left_to_right(egui::Align::Center)));
    let mut change = Change::None;
    if let Some(kind) = add_menu(&mut bar_ui) {
        add_after(&mut g, &mut st, kind);
        change = Change::Edit("Add node");
    }
    bar_ui.label(egui::RichText::new(format!("{} nodes · output {}", g.nodes.len(), g.output_id().and_then(|o| g.node(o)).map_or(String::new(), |n| n.label()))).small().color(t.text_2));
    if bar_ui.button("Remove comp").on_hover_text("Take the node graph off this clip").clicked() {
        if let Err(e) = crate::services::vfx::remove_graph(&mut app.doc, id) {
            app.doc.error = Some(e);
        }
        return;
    }
    let c = graph_ui(ui, area, &mut g, &mut st);
    if c != Change::None {
        change = c;
    }
    let at = (app.editor.playhead - mc.start) as f64 / crate::view::SEC as f64;
    let mut side_ui = ui.new_child(egui::UiBuilder::new().max_rect(side.shrink(8.0)));
    egui::ScrollArea::vertical().id_salt(("comp-side", id)).show_v(&mut side_ui, |ui| {
        let c = settings_ui(ui, &mut g, &mut st, at);
        if c != Change::None {
            change = c;
        }
    });
    ui.data_mut(|d| d.insert_temp(sid, st));
    let result = match change {
        Change::None => Ok(()),
        Change::Edit(label) => crate::services::vfx::store_graph(&mut app.doc, id, &g, label),
        Change::Live(label) => crate::services::vfx::store_graph_live(&mut app.doc, id, &g, label),
    };
    if let Err(e) = result {
        app.doc.error = Some(e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(ctx: &egui::Context, g: &mut Graph, st: &mut State, events: Vec<egui::Event>) -> Change {
        let mut change = Change::None;
        let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 500.0))), events, ..Default::default() };
        crate::editor::test_frame(&ctx, input, |ui| {
            let area = Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 500.0));
            change = graph_ui(ui, area, g, st);
        });
        change
    }

    #[test]
    fn nodes_draw_and_a_drag_moves_one() {
        let mut g = Graph::passthrough();
        let b = g.add("blur", &[Some(1)]);
        let mut st = State::default();
        let ctx = egui::Context::default();
        run(&ctx, &mut g, &mut st, vec![]);
        // The blur sits at (0, 70) from the centre (400, 250).
        let at = Pos2::new(400.0, 320.0);
        let mv = |p: Pos2| egui::Event::PointerMoved(p);
        let press = |p: Pos2, down: bool| egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: down, modifiers: Default::default() };
        run(&ctx, &mut g, &mut st, vec![mv(at), press(at, true)]);
        run(&ctx, &mut g, &mut st, vec![mv(at + Vec2::new(20.0, 0.0))]);
        run(&ctx, &mut g, &mut st, vec![mv(at + Vec2::new(60.0, 10.0))]);
        let c = run(&ctx, &mut g, &mut st, vec![press(at + Vec2::new(60.0, 10.0), false)]);
        assert_eq!(st.selected, Some(b));
        assert!(g.node(b).unwrap().pos[0] > 30.0, "{:?}", g.node(b).unwrap().pos);
        assert_eq!(c, Change::Edit("Move node"));
    }

    #[test]
    fn adding_after_the_selection_wires_it_in() {
        let mut g = Graph::passthrough();
        let mut st = State { selected: Some(1), ..Default::default() };
        let k = add_after(&mut g, &mut st, "keyer");
        assert_eq!(g.node(k).unwrap().inputs, vec![Some(1)]);
        assert_eq!(g.output_id(), Some(k));
        let r = add_after(&mut g, &mut st, "constant");
        assert!(g.node(r).unwrap().inputs.is_empty());
    }

    #[test]
    fn settings_change_a_value() {
        let mut g = Graph::passthrough();
        let gr = g.add("grade", &[Some(1)]);
        let mut st = State { selected: Some(gr), ..Default::default() };
        let ctx = egui::Context::default();
        let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 800.0))), ..Default::default() };
        let mut c = Change::None;
        crate::editor::test_frame(&ctx, input, |ui| {
            c = settings_ui(ui, &mut g, &mut st, 0.0);
        });
        assert_eq!(c, Change::None);
        // Values written through the same path as a slider edit.
        let def = g.node(gr).unwrap().param_defs().iter().find(|d| d.id == "gain").unwrap();
        g.node_mut(gr).unwrap().params.insert("gain".into(), json_of(ParamValue::Float(2.0), def));
        assert_eq!(g.node(gr).unwrap().params_at(0.0).f("gain"), 2.0);
    }
}
