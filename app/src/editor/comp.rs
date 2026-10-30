// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The compositor: the selected clip's node graph (`services::vfx`), in
//! the timeline's place. The picture flows from the Input through the
//! nodes to the one that is shown (the Output, marked with an eye).
//!
//! Nodes are boxes with their inputs on top and the result at the bottom.
//! Drag a node to move it (all selected nodes move together, snapped to a
//! grid); drag from a node's bottom port to another's input to connect;
//! drag an input away or right-click it to disconnect. Shift-drag on the
//! background selects a box, a plain drag pans, the wheel zooms, F frames
//! everything. Tab, a double-click or a right-click on the background opens
//! the node search at the pointer (on a wire it splices the node in);
//! Delete removes the selected nodes (never the clip), ⌘D duplicates them.
//! Every edit is a step in the document history.

use crate::app::StudioApp;
use crate::doc::Doc;
use crate::kit::{self, ShowNoted};
use crate::theme;
use eframe::egui::{self, Align2, Color32, Pos2, Rect, Sense, Stroke, Vec2};
use studio_fx::comp::{Graph, NodeCategory, NodeDef, NodeId, json_of, nodes};
use studio_fx::params::{ParamDef, ParamKind, ParamValue};
use studio_model as m;

fn on_id() -> egui::Id {
    egui::Id::unique("comp-editor-on")
}

/// Whether the timeline shows the compositor.
pub(super) fn is_on(ctx: &egui::Context) -> bool {
    ctx.data_mut(|d| *d.get_persisted_mut_or_default::<bool>(on_id()))
}

pub(super) fn set_on(ctx: &egui::Context, on: bool) {
    ctx.data_mut(|d| d.insert_persisted(on_id(), on));
}

/// The node search, open at a point of the canvas.
#[derive(Clone, Debug, Default)]
pub struct AddMenu {
    /// Where the menu sits on screen.
    pub screen: Pos2,
    /// Where the node goes in the graph; `None` puts it after `from`.
    pub at: Option<[f32; 2]>,
    /// The node that feeds the new one.
    pub from: Option<NodeId>,
    /// A wire the new node is spliced into: (reading node, input slot).
    pub wire: Option<(NodeId, usize)>,
    pub query: String,
    fresh: bool,
}

/// View state of one clip's graph.
#[derive(Clone, Debug, Default)]
pub struct State {
    pub pan: Vec2,
    /// Zero until the first frame, which frames the whole graph.
    pub zoom: f32,
    /// The node the settings show.
    pub selected: Option<NodeId>,
    /// The other selected nodes.
    pub also: Vec<NodeId>,
    /// A connection being dragged from a node's output.
    pub wiring: Option<NodeId>,
    /// The wire in the air was pulled off an input (dropping it on empty
    /// canvas leaves it cut) rather than started from an output.
    pulled: bool,
    box_from: Option<Pos2>,
    /// Where the moving nodes were when the drag began.
    drag_from: Vec<(NodeId, [f32; 2])>,
    /// Nodes move freely instead of snapping to the grid.
    pub free: bool,
    pub menu: Option<AddMenu>,
    /// A live gesture is open in the document.
    live: bool,
    want_fit: bool,
    note: Option<(String, f64)>,
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
/// The grid nodes snap to, in graph units.
const GRID: f32 = 20.0;

// TODO(kit): node category colours as theme tokens.
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

    /// Scale by `k`, keeping the graph point under `at` where it is.
    fn zoom_at(&mut self, area: Rect, at: Pos2, k: f32) {
        let old = self.scale();
        let new = (old * k).clamp(0.3, 2.5);
        let rel = at - area.center() - self.pan;
        self.pan += rel - rel * (new / old);
        self.zoom = new;
    }

    fn to_screen(&self, area: Rect, p: [f32; 2]) -> Pos2 {
        area.center() + self.pan + Vec2::new(p[0], p[1]) * self.scale()
    }

    fn to_graph(&self, area: Rect, q: Pos2) -> [f32; 2] {
        let v = (q - area.center() - self.pan) / self.scale();
        [v.x, v.y]
    }

    fn node_rect(&self, area: Rect, p: [f32; 2]) -> Rect {
        Rect::from_center_size(self.to_screen(area, p), Vec2::new(NODE_W, NODE_H) * self.scale())
    }

    /// Frame the whole graph in `area`.
    pub fn fit(&mut self, area: Rect, g: &Graph) {
        let [l, t, r, b] = g.bounds().unwrap_or([0.0; 4]);
        // Room for the node itself and the Output tag to its right.
        let (w, h) = (r - l + NODE_W + 110.0, b - t + NODE_H + 80.0);
        self.zoom = (area.width() / w).min(area.height() / h).clamp(0.3, 1.0);
        self.pan = -Vec2::new((l + r) / 2.0 + 40.0, (t + b) / 2.0) * self.zoom;
        self.want_fit = false;
    }

    /// Every selected node, the shown one first.
    pub fn ids(&self) -> Vec<NodeId> {
        self.selected.into_iter().chain(self.also.iter().copied()).collect()
    }

    pub fn is_sel(&self, id: NodeId) -> bool {
        self.selected == Some(id) || self.also.contains(&id)
    }

    pub fn select_only(&mut self, id: NodeId) {
        self.selected = Some(id);
        self.also.clear();
    }

    fn select_many(&mut self, ids: &[NodeId]) {
        self.selected = ids.first().copied();
        self.also = ids.iter().skip(1).copied().collect();
    }

    fn toggle(&mut self, id: NodeId) {
        let mut ids = self.ids();
        match ids.iter().position(|i| *i == id) {
            Some(k) => {
                ids.remove(k);
            }
            None => ids.push(id),
        }
        self.select_many(&ids);
    }

    fn snap(&self, p: [f32; 2]) -> [f32; 2] {
        if self.free { p } else { [(p[0] / GRID).round() * GRID, (p[1] / GRID).round() * GRID] }
    }

    fn tell(&mut self, now: f64, text: &str) {
        self.note = Some((text.to_owned(), now + 4.0));
    }

    /// Opens the node search with the node after the selection (or the
    /// shown one), as the bar's Add node does.
    pub fn open_menu(&mut self, g: &Graph, screen: Pos2) {
        self.menu = Some(AddMenu { screen, from: self.selected.or(g.output_id()), fresh: true, ..Default::default() });
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

fn mask_port(r: Rect) -> Pos2 {
    Pos2::new(r.right(), r.center().y)
}

/// One wire: what `dst` reads in input `slot` (`None` is its mask).
#[derive(Clone, Copy, Debug)]
struct Edge {
    dst: NodeId,
    slot: Option<usize>,
    a: Pos2,
    b: Pos2,
}

fn edges(g: &Graph, st: &State, area: Rect) -> Vec<Edge> {
    let mut out = Vec::new();
    for n in &g.nodes {
        let r = st.node_rect(area, n.pos);
        let count = n.def().map_or(0, |d| d.inputs.len());
        for (slot, i) in n.inputs.iter().enumerate() {
            let Some(src) = i.and_then(|i| g.node(i)) else { continue };
            out.push(Edge { dst: n.id, slot: Some(slot), a: out_port(st.node_rect(area, src.pos)), b: in_port(r, slot, count) });
        }
        if let Some(mk) = n.mask.and_then(|m| g.node(m)) {
            out.push(Edge { dst: n.id, slot: None, a: out_port(st.node_rect(area, mk.pos)), b: mask_port(r) });
        }
    }
    out
}

fn wire_points(a: Pos2, b: Pos2) -> [Pos2; 4] {
    let d = ((b.y - a.y).abs() * 0.5).max(30.0);
    [a, a + Vec2::new(0.0, d), b - Vec2::new(0.0, d), b]
}

fn wire(p: &egui::Painter, a: Pos2, b: Pos2, stroke: Stroke) {
    p.add(egui::epaint::CubicBezierShape::from_points_stroke(wire_points(a, b), false, Color32::TRANSPARENT, stroke));
}

/// How far `q` is from the wire `a`→`b`.
fn wire_distance(a: Pos2, b: Pos2, q: Pos2) -> f32 {
    let [p0, p1, p2, p3] = wire_points(a, b);
    let at = |t: f32| {
        let u = 1.0 - t;
        (p0.to_vec2() * (u * u * u) + p1.to_vec2() * (3.0 * u * u * t) + p2.to_vec2() * (3.0 * u * t * t) + p3.to_vec2() * (t * t * t)).to_pos2()
    };
    let mut best = f32::MAX;
    let mut prev = at(0.0);
    for k in 1..=24 {
        let next = at(k as f32 / 24.0);
        let seg = next - prev;
        let len2 = seg.length_sq();
        let t = if len2 > 0.0 { ((q - prev).dot(seg) / len2).clamp(0.0, 1.0) } else { 0.0 };
        best = best.min((prev + seg * t - q).length());
        prev = next;
    }
    best
}

/// The wire nearest `q` within `tol` pixels.
fn nearest_wire(g: &Graph, st: &State, area: Rect, q: Pos2, tol: f32) -> Option<Edge> {
    edges(g, st, area).into_iter().map(|e| (wire_distance(e.a, e.b, q), e)).filter(|(d, _)| *d <= tol).min_by(|x, y| x.0.total_cmp(&y.0)).map(|(_, e)| e)
}

/// The node kinds whose name, id, description or group has `query` in it
/// (all of them for an empty query), in catalogue order.
pub fn matching(query: &str) -> Vec<&'static NodeDef> {
    let q = query.trim().to_lowercase();
    nodes::all().iter().filter(|d| q.is_empty() || [d.name, d.id, d.description, d.category.label()].iter().any(|s| s.to_lowercase().contains(&q))).copied().collect()
}

/// What happened to a node in its own menu.
enum NodeAct {
    Duplicate,
    Delete,
    Toggle(NodeId),
    Cut(NodeId),
}

/// Draws and edits `g` inside `area`. Pure over the graph so it can run
/// headless in tests.
pub fn graph_ui(ui: &mut egui::Ui, area: Rect, g: &mut Graph, st: &mut State) -> Change {
    let t = theme::tokens(ui.ctx());
    let now = ui.input(|i| i.time);
    let mut change = Change::None;
    if st.zoom <= 0.0 || st.want_fit {
        st.fit(area, g);
    }
    let painter = ui.painter().with_clip_rect(area.intersect(ui.clip_rect()));
    painter.rect_filled(area, 0, t.sunken);
    let pointer = ui.input(|i| i.pointer.hover_pos()).filter(|q| area.contains(*q));
    // Tab moves egui's focus too, so only a text field in use keeps the keys.
    let typing = ui.ctx().text_edit_focused();
    let shift = ui.input(|i| i.modifiers.shift);

    // Keys. Typing in a field keeps them.
    if !typing {
        use egui::{Key, Modifiers};
        if ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::F)) {
            st.fit(area, g);
        }
        if ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::A)) {
            let ids: Vec<NodeId> = g.nodes.iter().map(|n| n.id).collect();
            st.select_many(&ids);
        }
        if ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::D)) && duplicate_selected(g, st) {
            change = Change::Edit("Duplicate nodes");
        }
        if ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Tab)) && st.menu.is_none() {
            let screen = pointer.unwrap_or_else(|| area.center());
            open_at(g, st, area, screen);
        }
        if ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
            if st.menu.is_some() {
                st.menu = None;
            } else if st.wiring.is_some() {
                st.wiring = None;
            } else {
                st.selected = None;
                st.also.clear();
            }
        }
        // Delete (normally already claimed by `claim_delete`, before the
        // timeline's own keys hear it).
        if ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Delete) || i.consume_key(Modifiers::NONE, Key::Backspace)) && delete_selected(g, st) {
            change = Change::Edit("Delete nodes");
        }
    }

    // The wire under the pointer, for highlighting and cutting.
    let hot = pointer.and_then(|q| nearest_wire(g, st, area, q, 7.0)).filter(|_| st.wiring.is_none());
    let node_rects: Vec<(NodeId, Rect)> = g.nodes.iter().map(|n| (n.id, st.node_rect(area, n.pos))).collect();
    let over_node = pointer.is_some_and(|q| node_rects.iter().any(|(_, r)| r.contains(q)));

    // Background: pan, box select, open the search, cut a wire.
    let bg = ui.interact(area, ui.scope_id().with("comp-bg"), Sense::click_and_drag());
    if bg.drag_started() && shift {
        st.box_from = ui.input(|i| i.pointer.press_origin());
    }
    if bg.dragged() && st.wiring.is_none() && st.box_from.is_none() {
        st.pan += bg.drag_delta();
    }
    if bg.clicked() && !shift {
        st.selected = None;
        st.also.clear();
        st.menu = None;
    }
    if (bg.secondary_clicked() || bg.double_clicked()) && !over_node {
        match (bg.secondary_clicked(), hot) {
            (true, Some(e)) => {
                let cut = match e.slot {
                    Some(s) => g.disconnect(e.dst, s),
                    None => g.disconnect_mask(e.dst),
                };
                if cut {
                    change = Change::Edit("Disconnect");
                }
            }
            _ => {
                let screen = bg.interact_pointer_pos().or(pointer).unwrap_or_else(|| area.center());
                open_at(g, st, area, screen);
            }
        }
    }
    if let Some(from) = st.box_from {
        if let Some(q) = ui.input(|i| i.pointer.interact_pos()) {
            let b = Rect::from_two_pos(from, q);
            painter.rect(b, 2, theme::alpha(t.select, 0.12), Stroke::new(1.0, t.select), egui::StrokeKind::Inside);
            if bg.drag_stopped() || !ui.input(|i| i.pointer.any_down()) {
                let mut ids = st.ids();
                ids.extend(node_rects.iter().filter(|(_, r)| r.intersects(b)).map(|(i, _)| *i));
                ids.dedup();
                st.select_many(&ids);
                st.box_from = None;
            }
        } else {
            st.box_from = None;
        }
    }
    // The wheel (or a pinch) zooms the graph about the pointer, wherever
    // it is over the graph, not only over empty space.
    if let Some(q) = pointer {
        let (scroll, pinch) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
        if scroll.abs() > 0.0 || pinch != 1.0 {
            st.zoom_at(area, q, (scroll * 0.002).exp() * pinch);
        }
    }

    // Grid dots, the grid nodes snap to.
    let mut step = GRID * st.scale();
    while step < 14.0 {
        step *= 2.0;
    }
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
    for e in edges(g, st, area) {
        let lit = hot.is_some_and(|h| h.dst == e.dst && h.slot == e.slot);
        let stroke = match (e.slot, lit) {
            (_, true) => Stroke::new(2.4, t.accent),
            (Some(_), false) => Stroke::new(1.6, theme::alpha(t.text, 0.55)),
            (None, false) => Stroke::new(1.2, theme::alpha(t.warn, 0.6)),
        };
        wire(&painter, e.a, e.b, stroke);
    }
    if let (Some(e), Some(q)) = (hot, pointer) {
        painter.text(q + Vec2::new(12.0, 12.0), Align2::LEFT_TOP, "Right-click to disconnect", theme::caption(), t.text_2);
        let _ = e;
    }

    // Nodes.
    let ids: Vec<NodeId> = g.nodes.iter().map(|n| n.id).collect();
    let s = st.scale();
    let mut drop_slot: Option<(NodeId, usize)> = None;
    let mut drop_mask: Option<NodeId> = None;
    let mut acts: Vec<NodeAct> = Vec::new();
    let mut view: Option<NodeId> = None;
    for id in ids {
        let Some(n) = g.node(id).cloned() else { continue };
        let def = n.def();
        let r = st.node_rect(area, n.pos);
        let col = def.map_or(t.text_2, |d| colour_of(d.category));
        let resp = ui.interact(r, ui.scope_id().with(("comp-node", id)), Sense::click_and_drag());
        if resp.clicked() {
            if shift {
                st.toggle(id);
            } else {
                st.select_only(id);
            }
        }
        if resp.secondary_clicked() && !st.is_sel(id) {
            st.select_only(id);
        }
        if resp.drag_started() && st.wiring.is_none() {
            if !st.is_sel(id) {
                if shift {
                    st.toggle(id);
                } else {
                    st.select_only(id);
                }
            }
            st.drag_from = st.ids().into_iter().filter_map(|i| g.node(i).map(|n| (i, n.pos))).collect();
        }
        if resp.dragged()
            && st.wiring.is_none()
            && let Some(total) = resp.total_drag_delta()
        {
            let d = total / st.scale();
            let from = st.drag_from.clone();
            let mut moved = false;
            for (nid, p0) in from {
                let to = st.snap([p0[0] + d.x, p0[1] + d.y]);
                if let Some(nm) = g.node_mut(nid).filter(|nm| nm.pos != to) {
                    nm.pos = to;
                    moved = true;
                }
            }
            if moved {
                change = Change::Live("Move nodes");
            }
        }
        if resp.drag_stopped() {
            st.drag_from.clear();
        }
        if resp.double_clicked() {
            view = Some(id);
        }
        resp.context_menu(|ui| {
            kit::popup_body(ui, |ui| {
                if ui.button("View").clicked() {
                    view = Some(id);
                    ui.close();
                }
                if ui.button("Duplicate").clicked() {
                    acts.push(NodeAct::Duplicate);
                    ui.close();
                }
                if ui.button(if n.disabled { "Turn on" } else { "Turn off" }).clicked() {
                    acts.push(NodeAct::Toggle(id));
                    ui.close();
                }
                if n.inputs.iter().any(Option::is_some) && ui.button("Disconnect inputs").clicked() {
                    acts.push(NodeAct::Cut(id));
                    ui.close();
                }
                if n.kind != "input" && ui.button("Delete").clicked() {
                    acts.push(NodeAct::Delete);
                    ui.close();
                }
            })
        });
        let sel = st.is_sel(id);
        let is_out = Some(id) == out;
        painter.rect_filled(r, 6, if n.disabled { theme::alpha(t.raised, 0.5) } else { t.raised });
        painter.rect_filled(Rect::from_min_size(r.min, Vec2::new(5.0 * s, r.height())), 6, col);
        let edge = if sel { t.select } else if is_out { theme::alpha(t.accent, 0.8) } else { t.line };
        painter.rect_stroke(r, 6, Stroke::new(if sel || is_out { 2.0 } else { 1.0 }, edge), egui::StrokeKind::Inside);
        // The name, cut to fit beside the eye.
        let text_w = (r.width() - 12.0 * s - 24.0 * s).max(10.0);
        let mut job = egui::text::LayoutJob::simple_singleline(n.label(), theme::caption(), if n.disabled { t.text_2 } else { t.text });
        job.wrap = egui::text::TextWrapping::truncate_at_width(text_w);
        let galley = painter.layout_job(job);
        painter.galley(Pos2::new(r.left() + 11.0 * s, r.center().y - galley.size().y / 2.0), galley, t.text);
        // The eye: shows this node as the picture.
        let eye_c = Pos2::new(r.right() - 13.0 * s, r.center().y);
        let eye = ui.interact(Rect::from_center_size(eye_c, Vec2::splat(18.0 * s.max(0.8))), ui.scope_id().with(("comp-eye", id)), Sense::click());
        let eye_col = if is_out { t.accent } else if eye.hovered() { t.text } else { theme::alpha(t.text_2, 0.7) };
        painter.circle_stroke(eye_c, 5.5 * s, Stroke::new(1.3, eye_col));
        painter.circle_filled(eye_c, 2.2 * s, eye_col);
        if eye.on_hover_text(if is_out { "Showing this node" } else { "Show this node" }).clicked() {
            view = Some(id);
        }
        if is_out {
            painter.text(Pos2::new(r.right() + 16.0, r.center().y), Align2::LEFT_CENTER, "◀ Output", theme::caption(), t.accent);
        }
        // Inputs: dropping a wire, dragging one away to cut it, right-click.
        let count = def.map_or(0, |d| d.inputs.len());
        for slot in 0..count {
            let p = in_port(r, slot, count);
            painter.circle_filled(p, PORT, if n.input(slot).is_some() { t.text } else { t.text_2 });
            let pr = Rect::from_center_size(p, Vec2::splat(PORT * 3.0));
            let hovered = pointer.is_some_and(|q| pr.contains(q));
            if let (Some(d), true) = (def, hovered) {
                painter.text(p - Vec2::new(0.0, 8.0), Align2::CENTER_BOTTOM, d.inputs[slot], theme::caption(), t.text_2);
            }
            if st.wiring.is_some() && hovered {
                drop_slot = Some((id, slot));
            }
            let pin = ui.interact(pr, ui.scope_id().with(("comp-in", id, slot)), Sense::click_and_drag());
            if pin.drag_started() && st.wiring.is_none() && n.input(slot).is_some() {
                // Pull the wire off and carry it.
                st.wiring = n.input(slot);
                st.pulled = true;
                g.disconnect(id, slot);
                change = Change::Edit("Disconnect");
            }
            if pin.secondary_clicked() && g.disconnect(id, slot) {
                change = Change::Edit("Disconnect");
            }
        }
        // The mask port, on the right edge, while it matters.
        let near = pointer.is_some_and(|q| r.expand(10.0).contains(q));
        if n.mask.is_some() || (st.wiring.is_some() && count > 0) || near {
            let mp = mask_port(r);
            painter.circle_stroke(mp, PORT - 1.0, Stroke::new(1.4, if n.mask.is_some() { t.warn } else { t.text_2 }));
            let mr = Rect::from_center_size(mp, Vec2::splat(PORT * 3.0));
            if pointer.is_some_and(|q| mr.contains(q)) {
                painter.text(mp + Vec2::new(8.0, 0.0), Align2::LEFT_CENTER, "Mask", theme::caption(), t.text_2);
                if st.wiring.is_some() {
                    drop_mask = Some(id);
                }
            }
            let mi = ui.interact(mr, ui.scope_id().with(("comp-mask", id)), Sense::click_and_drag());
            if mi.drag_started()
                && st.wiring.is_none()
                && let Some(src) = n.mask
            {
                st.wiring = Some(src);
                st.pulled = true;
                g.disconnect_mask(id);
                change = Change::Edit("Disconnect");
            }
            if mi.secondary_clicked() && g.disconnect_mask(id) {
                change = Change::Edit("Disconnect");
            }
        }
        // Output: start a wire.
        let op = out_port(r);
        painter.circle_filled(op, PORT, col);
        let orr = Rect::from_center_size(op, Vec2::splat(PORT * 3.0));
        let po = ui.interact(orr, ui.scope_id().with(("comp-out", id)), Sense::drag());
        if po.drag_started() && st.wiring.is_none() {
            st.wiring = Some(id);
            st.pulled = false;
        }
    }
    if let Some(v) = view {
        g.output = Some(v);
        change = Change::Edit("View node");
    }
    for act in acts {
        match act {
            NodeAct::Duplicate => {
                if duplicate_selected(g, st) {
                    change = Change::Edit("Duplicate nodes");
                }
            }
            NodeAct::Delete => {
                if delete_selected(g, st) {
                    change = Change::Edit("Delete nodes");
                }
            }
            NodeAct::Toggle(id) => {
                if let Some(nm) = g.node_mut(id) {
                    nm.disabled = !nm.disabled;
                    change = Change::Edit("Node on/off");
                }
            }
            NodeAct::Cut(id) => {
                let count = g.node(id).map_or(0, |n| n.inputs.len());
                let mut cut = false;
                for slot in 0..count {
                    cut |= g.disconnect(id, slot);
                }
                if cut {
                    change = Change::Edit("Disconnect");
                }
            }
        }
    }

    // A wire in the air.
    if let Some(src) = st.wiring {
        if let (Some(s), Some(q)) = (g.node(src), ui.input(|i| i.pointer.interact_pos())) {
            wire(&painter, out_port(st.node_rect(area, s.pos)), q, Stroke::new(1.6, t.accent));
        }
        if ui.input(|i| i.pointer.any_released()) {
            let q = ui.input(|i| i.pointer.interact_pos());
            let body = q.and_then(|q| node_rects.iter().find(|(_, r)| r.contains(q)).map(|(i, _)| *i));
            let target = match (drop_slot, drop_mask, body) {
                (Some((d, slot)), _, _) => Some((d, Some(slot))),
                (_, Some(d), _) => Some((d, None)),
                // Dropped on a node's body: its first free input.
                (_, _, Some(d)) => g.node(d).and_then(|n| {
                    let count = n.def().map_or(0, |d| d.inputs.len());
                    (0..count).find(|k| n.input(*k).is_none()).or((count > 0).then_some(0)).map(|k| (d, Some(k)))
                }),
                _ => None,
            };
            match target {
                Some((dst, _)) if dst == src => {}
                Some((dst, slot)) => {
                    let r = match slot {
                        Some(k) => g.connect(dst, k, src),
                        None => g.connect_mask(dst, src),
                    };
                    match r {
                        Ok(()) => change = Change::Edit("Connect"),
                        Err(e) if e.contains("loop") => st.tell(now, "Can't connect: that would make a loop"),
                        Err(_) => st.tell(now, "Can't connect those nodes"),
                    }
                }
                // Let go over empty canvas: search for a node to put there.
                None if !st.pulled && body.is_none() => {
                    if let Some(q) = q.filter(|q| area.contains(*q)) {
                        st.menu = Some(AddMenu { screen: q, at: Some(st.to_graph(area, q)), from: Some(src), fresh: true, ..Default::default() });
                    }
                }
                None => {}
            }
            st.wiring = None;
            st.pulled = false;
        }
    }

    // Nothing but the input yet: say what this is and where to start.
    if g.nodes.len() <= 1 && st.menu.is_none() {
        let y = g.nodes.first().map_or(area.center().y, |n| st.node_rect(area, n.pos).bottom()) + 44.0;
        let x = area.center().x;
        painter.text(Pos2::new(x, y), Align2::CENTER_TOP, "Build this clip's picture with nodes", theme::body_medium(), t.text);
        painter.text(Pos2::new(x, y + 24.0), Align2::CENTER_TOP, "The Input flows through the nodes you add to the Output.", theme::caption(), t.text_2);
        painter.text(Pos2::new(x, y + 42.0), Align2::CENTER_TOP, "Drag from a node's bottom dot to another's top dot to connect them.", theme::caption(), t.text_2);
        let btn = Rect::from_center_size(Pos2::new(x, y + 82.0), Vec2::new(110.0, 26.0));
        if ui.put(btn, egui::Button::new("Add node")).on_hover_text("Search the node kinds (Tab)").clicked() {
            st.open_menu(g, Pos2::new(x - 120.0, y + 100.0));
        }
    }

    // The search.
    if let Some(c) = menu_ui(ui, area, g, st) {
        change = c;
    }

    // A refusal, for a moment.
    if let Some((text, until)) = st.note.clone() {
        if now < until {
            painter.text(area.left_bottom() + Vec2::new(12.0, -12.0), Align2::LEFT_BOTTOM, text, theme::body(), t.warn);
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
        } else {
            st.note = None;
        }
    }
    change
}

/// Opens the search at `screen`: on a wire it splices, elsewhere it puts
/// a loose node at the point.
fn open_at(g: &Graph, st: &mut State, area: Rect, screen: Pos2) {
    let wire = nearest_wire(g, st, area, screen, 9.0).and_then(|e| e.slot.map(|s| (e.dst, s)));
    st.menu = Some(AddMenu { screen, at: Some(st.to_graph(area, screen)), wire, fresh: true, ..Default::default() });
}

/// The node search: type to filter, Enter or a click adds. Returns the
/// change made.
fn menu_ui(ui: &mut egui::Ui, area: Rect, g: &mut Graph, st: &mut State) -> Option<Change> {
    let mut menu = st.menu.take()?;
    let t = theme::tokens(ui.ctx());
    let pos = Pos2::new(menu.screen.x.min(area.right() - 250.0).max(area.left()), menu.screen.y.min(area.bottom() - 330.0).max(area.top()));
    let mut picked: Option<&'static str> = None;
    let mut close = false;
    let shown = egui::Area::new(ui.scope_id().with("comp-add")).order(egui::Order::Foreground).fixed_pos(pos).show(ui.ctx(), |ui| {
        egui::Frame::popup(ui.style()).show(ui, |ui| {
            ui.set_width(240.0);
            if menu.wire.is_some() {
                ui.label(egui::RichText::new("Adds the node onto the wire").small().color(t.text_2));
            }
            let te = ui.add(egui::TextEdit::singleline(&mut menu.query).hint_text("Search nodes").desired_width(f32::INFINITY));
            if menu.fresh {
                te.request_focus();
                menu.fresh = false;
            }
            let found = matching(&menu.query);
            if te.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                picked = found.first().map(|d| d.id);
            }
            egui::ScrollArea::vertical().max_height(280.0).auto_shrink([false, true]).show(ui, |ui| {
                if found.is_empty() {
                    ui.label(egui::RichText::new("No node matches").color(t.text_2));
                }
                let grouped = menu.query.trim().is_empty();
                for cat in NodeCategory::ALL {
                    let mut head = grouped;
                    for d in found.iter().filter(|d| d.category == cat) {
                        if head {
                            ui.label(egui::RichText::new(cat.label()).small().color(t.text_2));
                            head = false;
                        }
                        let row = ui.selectable_label(false, d.name).on_hover_text(d.description);
                        if row.clicked() {
                            picked = Some(d.id);
                        }
                    }
                }
            });
        });
    });
    let outside = ui.input(|i| i.pointer.any_pressed()) && ui.input(|i| i.pointer.interact_pos()).is_some_and(|q| !shown.response.rect.contains(q));
    if outside || shown.response.clicked_elsewhere() && false {
        close = true;
    }
    match picked {
        Some(kind) => {
            add_at(g, st, area, &menu, kind);
            Some(Change::Edit("Add node"))
        }
        None => {
            if !close {
                st.menu = Some(menu);
            }
            None
        }
    }
}

/// Adds `kind` as the search `menu` asked: spliced onto its wire, after
/// its `from` node, or loose at its point. Selects the new node.
fn add_at(g: &mut Graph, st: &mut State, area: Rect, menu: &AddMenu, kind: &str) -> NodeId {
    let was = g.output_id();
    let id = match (menu.wire, menu.from) {
        (Some((dst, slot)), _) => g.splice(kind, dst, slot).unwrap_or_else(|| g.add(kind, &[])),
        (None, Some(from)) if menu.at.is_none() => return add_after_node(g, st, kind, Some(from)),
        (None, Some(from)) => {
            let ins = if nodes::find(kind).is_some_and(|d| !d.inputs.is_empty()) { vec![Some(from)] } else { Vec::new() };
            g.add(kind, &ins)
        }
        (None, None) => g.add(kind, &[]),
    };
    // Only extending the shown chain changes what is shown.
    if menu.from.is_none() || menu.from != was {
        g.output = was;
    }
    let at = menu.at.unwrap_or_else(|| st.to_graph(area, menu.screen));
    let at = st.snap(at);
    if let Some(n) = g.node_mut(id) {
        n.pos = at;
    }
    st.select_only(id);
    id
}

/// Remove the selected nodes, except the input. True if any went.
fn delete_selected(g: &mut Graph, st: &mut State) -> bool {
    let mut gone = false;
    for id in st.ids() {
        if g.node(id).is_some_and(|n| n.kind != "input") {
            gone |= g.remove(id);
        }
    }
    if gone {
        st.selected = None;
        st.also.clear();
    }
    gone
}

/// Copy the selected nodes a step down and right; the copies become the
/// selection. True if any were made.
fn duplicate_selected(g: &mut Graph, st: &mut State) -> bool {
    let copies = g.duplicate(&st.ids(), [GRID * 2.0, GRID * 2.0]);
    if copies.is_empty() {
        return false;
    }
    st.select_many(&copies);
    true
}

/// With the compositor open and a node selected, Delete and Backspace
/// belong to the node: this takes them before the timeline's keys can
/// delete the clip. Called ahead of the shortcut dispatcher.
pub(super) fn claim_delete(app: &mut StudioApp, ctx: &egui::Context) {
    if !is_on(ctx) {
        return;
    }
    let Some(&id) = app.editor.selection.first() else { return };
    let sid = egui::Id::unique(("comp-state", id));
    let Some(mut st) = ctx.data(|d| d.get_temp::<State>(sid)) else { return };
    if st.selected.is_none() {
        return;
    }
    let pressed = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Delete) | i.consume_key(egui::Modifiers::NONE, egui::Key::Backspace));
    if !pressed {
        return;
    }
    let Some(mc) = app.doc.model.active().clip(m::ClipId(id as u64)).cloned() else { return };
    let Some((_, mut g)) = crate::services::vfx::graph_of(&mc) else { return };
    if delete_selected(&mut g, &mut st) {
        if let Err(e) = crate::services::vfx::store_graph(&mut app.doc, id, &g, "Delete nodes") {
            app.doc.error = Some(e);
        }
        ctx.data_mut(|d| d.insert_temp(sid, st));
    }
}

/// Adds a node of `kind` after the selected node (or the output).
#[cfg(test)]
fn add_after(g: &mut Graph, st: &mut State, kind: &str) -> NodeId {
    let after = st.selected.or(g.output_id());
    add_after_node(g, st, kind, after)
}

fn add_after_node(g: &mut Graph, st: &mut State, kind: &str, after: Option<NodeId>) -> NodeId {
    let ins = match nodes::find(kind) {
        Some(d) if !d.inputs.is_empty() => vec![after],
        _ => Vec::new(),
    };
    let id = g.add(kind, &ins);
    if let (Some(a), Some(n)) = (after.and_then(|a| g.node(a)).map(|a| a.pos), g.node_mut(id)) {
        n.pos = [a[0] + if ins.is_empty() { 160.0 } else { 0.0 }, a[1] + 70.0];
    }
    st.select_only(id);
    id
}

/// "3 nodes · showing Blur2".
fn status(g: &Graph) -> String {
    let n = g.nodes.len();
    let shown = g.output_id().and_then(|o| g.node(o)).map_or_else(|| "nothing".to_owned(), |n| n.label());
    format!("{n} {} · showing {shown}", if n == 1 { "node" } else { "nodes" })
}

/// The selected node's settings. Returns the change made.
pub fn settings_ui(ui: &mut egui::Ui, g: &mut Graph, st: &mut State, at_seconds: f64) -> Change {
    let t = theme::tokens(ui.ctx());
    let mut change = Change::None;
    let Some(id) = st.selected else {
        ui.label(egui::RichText::new("Select a node to edit it").strong());
        ui.label(egui::RichText::new("Click the eye on a node to show it in the viewer.").small().color(t.text_2));
        return change;
    };
    let Some(n) = g.node(id).cloned() else { return change };
    let many = st.ids().len();
    ui.horizontal_wrapped(|ui| {
        ui.strong(n.label());
        ui.label(egui::RichText::new(n.def().map_or("", |d| d.name)).weak());
    });
    if many > 1 {
        ui.label(egui::RichText::new(format!("{many} nodes selected")).small().color(t.text_2));
    } else if let Some(d) = n.def() {
        ui.label(egui::RichText::new(d.description).small().color(t.text_2));
    }
    ui.horizontal_wrapped(|ui| {
        if ui.button("Duplicate").on_hover_text("Copy the selected nodes (⌘D)").clicked() && duplicate_selected(g, st) {
            change = Change::Edit("Duplicate nodes");
        }
        let can_delete = st.ids().iter().any(|i| g.node(*i).is_some_and(|n| n.kind != "input"));
        if ui.add_enabled(can_delete, egui::Button::new("Delete")).on_hover_text("Remove the selected nodes (Delete). The clip stays").clicked() && delete_selected(g, st) {
            change = Change::Edit("Delete nodes");
        }
    });
    if change != Change::None {
        return change;
    }
    let mut name = n.name.clone();
    ui.horizontal_wrapped(|ui| {
        ui.label("Name");
        let hint = n.def().map_or("Name", |d| d.name);
        if ui.add(egui::TextEdit::singleline(&mut name).hint_text(hint)).lost_focus() && name != n.name {
            g.node_mut(id).unwrap().name = name.clone();
            change = Change::Edit("Rename node");
        }
    });
    ui.horizontal_wrapped(|ui| {
        let shown = g.output_id() == Some(id);
        let label = if shown { "Showing in viewer" } else { "Show in viewer" };
        if ui.add_enabled(!shown, egui::Button::new(label)).on_hover_text("The shown node is the clip's picture").clicked() {
            g.output = Some(id);
            change = Change::Edit("View node");
        }
        let mut on = !n.disabled;
        if ui.checkbox(&mut on, "Enabled").on_hover_text("Off passes the input through unchanged").changed() {
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

/// The bar on top, the canvas, and the settings on the right.
fn layout(rect: Rect) -> (Rect, Rect, Rect) {
    let bar = Rect::from_min_size(rect.min, Vec2::new(rect.width(), 28.0));
    let side_w = 280.0f32.min(rect.width() * 0.4);
    let area = Rect::from_min_max(Pos2::new(rect.left(), bar.bottom()), Pos2::new(rect.right() - side_w, rect.bottom()));
    let side = Rect::from_min_max(Pos2::new(area.right(), bar.bottom()), rect.max);
    (bar, area, side)
}

/// The compositor in the timeline's place, on the selected clip.
pub(super) fn show(app: &mut StudioApp, ui: &mut egui::Ui, rect: Rect) {
    let t = theme::tokens(ui.ctx());
    let Some(&id) = app.editor.selection.first() else {
        ui.painter().text(rect.center(), Align2::CENTER_CENTER, "Select a clip to composite it with nodes", theme::body(), t.text_2);
        return;
    };
    let sid = egui::Id::unique(("comp-state", id));
    let mut st: State = ui.data(|d| d.get_temp(sid)).unwrap_or_default();
    frame(&mut app.doc, ui, rect, id, app.editor.playhead, &mut st);
    ui.data_mut(|d| d.insert_temp(sid, st));
}

/// One frame of the compositor for clip `id`: draws it and writes what the
/// person did into `doc`, as undo steps.
fn frame(doc: &mut Doc, ui: &mut egui::Ui, rect: Rect, id: u32, playhead: crate::view::Time, st: &mut State) {
    let t = theme::tokens(ui.ctx());
    let Some(mc) = doc.model.active().clip(m::ClipId(id as u64)).cloned() else { return };
    let (bar, area, side) = layout(rect);
    let Some(mut g) = crate::services::vfx::graph_of(&mc).map(|(_, g)| g) else {
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink(12.0)));
        child.label(egui::RichText::new(format!("{} has no node graph", mc.name)).strong());
        child.label(egui::RichText::new("Nodes process this clip's picture: the Input flows through effects to the Output.").color(t.text_2));
        if child.button("Composite with nodes").clicked() {
            let mut g = Graph::passthrough();
            g.input_media = crate::services::vfx::input_media(doc, &mc);
            if let Err(e) = crate::services::vfx::store_graph(doc, id, &g, "Comp") {
                doc.error = Some(e);
            }
        }
        return;
    };
    let mut bar_ui = ui.new_child(egui::UiBuilder::new().max_rect(bar).layout(egui::Layout::left_to_right(egui::Align::Center)));
    let mut change = Change::None;
    if bar_ui.button("Add node").on_hover_text("Search the node kinds (Tab)").clicked() {
        st.open_menu(&g, area.left_top() + Vec2::new(12.0, 8.0));
    }
    if bar_ui.button("Frame all").on_hover_text("Fit every node in view (F)").clicked() {
        st.want_fit = true;
    }
    if bar_ui.selectable_label(!st.free, "Snap").on_hover_text("Snap nodes to the grid").clicked() {
        st.free = !st.free;
    }
    bar_ui.label(egui::RichText::new(status(&g)).small().color(t.text_2));
    if bar_ui.button("Remove comp").on_hover_text("Take the node graph off this clip").clicked() {
        if let Err(e) = crate::services::vfx::remove_graph(doc, id) {
            doc.error = Some(e);
        }
        return;
    }
    let c = graph_ui(ui, area, &mut g, st);
    if c != Change::None {
        change = c;
    }
    let at = (playhead - mc.start) as f64 / crate::view::SEC as f64;
    let mut side_ui = ui.new_child(egui::UiBuilder::new().max_rect(side.shrink(8.0)));
    egui::ScrollArea::vertical().id_salt(("comp-side", id)).show_v(&mut side_ui, |ui| {
        let c = settings_ui(ui, &mut g, st, at);
        if c != Change::None {
            change = c;
        }
    });
    let result = match change {
        Change::None => Ok(()),
        Change::Edit(label) => crate::services::vfx::store_graph(doc, id, &g, label),
        Change::Live(label) => {
            st.live = true;
            crate::services::vfx::store_graph_live(doc, id, &g, label)
        }
    };
    if let Err(e) = result {
        doc.error = Some(e);
    }
    // A drag is one undo step: it closes when the pointer lets go.
    if st.live && !ui.input(|i| i.pointer.any_down()) {
        doc.end_gesture();
        st.live = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Event, Modifiers, PointerButton};

    /// A real document with one clip carrying a graph, and the editor
    /// drawn over it headless, frame by frame, as the timeline does.
    struct Rig {
        ctx: egui::Context,
        doc: Doc,
        id: u32,
        st: State,
        rect: Rect,
        pointer: Pos2,
    }

    impl Rig {
        fn new(build: impl FnOnce(&mut Graph)) -> Rig {
            let mut doc = Doc::new(crate::perf::heavy(0.05, 3, 1));
            let id = doc.project.clips.first().map(|c| c.id).expect("a clip");
            let mut g = Graph::passthrough();
            build(&mut g);
            crate::services::vfx::store_graph(&mut doc, id, &g, "Comp").unwrap();
            let mut r = Rig { ctx: egui::Context::default(), doc, id, st: State::default(), rect: Rect::from_min_size(Pos2::ZERO, Vec2::new(1000.0, 600.0)), pointer: Pos2::ZERO };
            r.frame(vec![], Modifiers::NONE);
            r
        }

        fn graph(&self) -> Graph {
            let mc = self.doc.model.active().clip(m::ClipId(self.id as u64)).cloned().unwrap();
            crate::services::vfx::graph_of(&mc).unwrap().1
        }

        fn area(&self) -> Rect {
            layout(self.rect).1
        }

        fn frame(&mut self, events: Vec<Event>, modifiers: Modifiers) {
            let mut events = events;
            events.insert(0, Event::ModifiersChanged(modifiers));
            let input = egui::RawInput { screen_rect: Some(self.rect), events, ..Default::default() };
            let (doc, id, st, rect) = (&mut self.doc, self.id, &mut self.st, self.rect);
            crate::editor::test_frame(&self.ctx, input, |ui| frame(doc, ui, rect, id, 0, st));
        }

        /// Where node `id` is on screen.
        fn at(&self, id: NodeId) -> Rect {
            self.st.node_rect(self.area(), self.graph().node(id).unwrap().pos)
        }

        fn moved(&mut self, to: Pos2, modifiers: Modifiers) {
            self.pointer = to;
            self.frame(vec![Event::PointerMoved(to)], modifiers);
        }

        fn button(&mut self, down: bool, b: PointerButton, modifiers: Modifiers) {
            let e = Event::PointerButton { pos: self.pointer, button: b, pressed: down, modifiers };
            self.frame(vec![e], modifiers);
        }

        /// Press at `from`, move through `via`, release at the last point.
        fn drag(&mut self, from: Pos2, via: &[Pos2], modifiers: Modifiers) {
            self.moved(from, modifiers);
            self.button(true, PointerButton::Primary, modifiers);
            for p in via {
                self.moved(*p, modifiers);
            }
            self.button(false, PointerButton::Primary, modifiers);
            self.frame(vec![], modifiers);
        }

        fn click(&mut self, at: Pos2, b: PointerButton) {
            self.moved(at, Modifiers::NONE);
            self.button(true, b, Modifiers::NONE);
            self.button(false, b, Modifiers::NONE);
            self.frame(vec![], Modifiers::NONE);
        }

        fn key(&mut self, key: egui::Key, modifiers: Modifiers) {
            self.frame(vec![Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }], modifiers);
        }

        fn texts(&mut self) -> String {
            let input = egui::RawInput { screen_rect: Some(self.rect), ..Default::default() };
            let (doc, id, st, rect) = (&mut self.doc, self.id, &mut self.st, self.rect);
            let mut out = self.ctx.run_ui(input, |ui| frame(doc, ui, rect, id, 0, st));
            out.textures_delta.clear();
            let mut all = String::new();
            for s in &out.shapes {
                collect_text(&s.shape, &mut all);
            }
            all
        }
    }

    fn collect_text(s: &egui::epaint::Shape, out: &mut String) {
        match s {
            egui::epaint::Shape::Text(t) => {
                out.push_str(t.galley.text());
                out.push('\n');
            }
            egui::epaint::Shape::Vec(v) => v.iter().for_each(|s| collect_text(s, out)),
            _ => {}
        }
    }

    fn two_blurs(g: &mut Graph) {
        let a = g.add("blur", &[Some(1)]);
        g.add("blur", &[Some(a)]);
    }

    fn mid(a: Rect, b: Rect) -> Pos2 {
        Pos2::new((a.center().x + b.center().x) / 2.0, (a.center().y + b.center().y) / 2.0)
    }

    #[test]
    fn dragging_a_node_moves_it_in_the_document_as_one_undo_step() {
        let mut rig = Rig::new(two_blurs);
        let before = rig.graph().node(2).unwrap().pos;
        let steps = rig.doc.undo_label().map(str::to_owned);
        let r = rig.at(2);
        let c = r.center();
        rig.drag(c, &[c + Vec2::new(20.0, 0.0), c + Vec2::new(80.0, 10.0), c + Vec2::new(100.0, 10.0)], Modifiers::NONE);
        let after = rig.graph().node(2).unwrap().pos;
        assert!(after[0] > before[0] + 40.0, "moved in the document: {before:?} -> {after:?}");
        assert_eq!(after[0] % GRID, 0.0, "snapped to the grid: {after:?}");
        assert_eq!(rig.st.selected, Some(2));
        // The whole drag is one step.
        assert_eq!(rig.doc.undo_label(), Some("Move nodes"));
        rig.doc.undo();
        assert_eq!(rig.graph().node(2).unwrap().pos, before);
        assert_eq!(rig.doc.undo_label().map(str::to_owned), steps, "and nothing else was recorded");
        rig.doc.redo();
        assert_eq!(rig.graph().node(2).unwrap().pos, after);
    }

    #[test]
    fn a_free_drag_does_not_snap() {
        let mut rig = Rig::new(two_blurs);
        rig.st.free = true;
        let c = rig.at(2).center();
        rig.drag(c, &[c + Vec2::new(7.0, 0.0), c + Vec2::new(13.0, 0.0)], Modifiers::NONE);
        let x = rig.graph().node(2).unwrap().pos[0];
        assert!((x - 13.0 / rig.st.scale()).abs() < 0.6, "{x}");
    }

    #[test]
    fn dragging_one_selected_node_moves_all_selected() {
        let mut rig = Rig::new(two_blurs);
        rig.st.select_many(&[2, 3]);
        let (p2, p3) = (rig.graph().node(2).unwrap().pos, rig.graph().node(3).unwrap().pos);
        let c = rig.at(3).center();
        rig.drag(c, &[c + Vec2::new(30.0, 0.0), c + Vec2::new(60.0, 0.0)], Modifiers::NONE);
        let g = rig.graph();
        assert!(g.node(2).unwrap().pos[0] > p2[0] + 20.0 && g.node(3).unwrap().pos[0] > p3[0] + 20.0);
        assert_eq!(g.node(3).unwrap().pos[0] - g.node(2).unwrap().pos[0], p3[0] - p2[0], "they keep their distance");
        assert_eq!(g.node(1).unwrap().pos, [0.0, 0.0], "the unselected one stays");
    }

    #[test]
    fn dragging_from_an_output_to_an_input_connects_and_undoes() {
        let mut rig = Rig::new(|g| {
            let a = g.add("blur", &[Some(1)]);
            let b = g.add("blur", &[None]);
            g.node_mut(b).unwrap().pos = [200.0, 140.0];
            assert!(a == 2 && b == 3);
        });
        assert_eq!(rig.graph().node(3).unwrap().input(0), None);
        let (a, b) = (rig.at(2), rig.at(3));
        let (from, to) = (out_port(a), in_port(b, 0, 1));
        rig.drag(from, &[mid_pos(from, to), to], Modifiers::NONE);
        assert_eq!(rig.graph().node(3).unwrap().input(0), Some(2));
        assert_eq!(rig.doc.undo_label(), Some("Connect"));
        rig.doc.undo();
        assert_eq!(rig.graph().node(3).unwrap().input(0), None);
    }

    fn mid_pos(a: Pos2, b: Pos2) -> Pos2 {
        Pos2::new((a.x + b.x) / 2.0, (a.y + b.y) / 2.0)
    }

    #[test]
    fn a_loop_is_refused_with_a_message() {
        let mut rig = Rig::new(two_blurs);
        // Node 3 reads 2; wire 3's output back into 2's input.
        let (from, to) = (out_port(rig.at(3)), in_port(rig.at(2), 0, 1));
        rig.drag(from, &[mid_pos(from, to), to], Modifiers::NONE);
        assert_eq!(rig.graph().node(2).unwrap().input(0), Some(1), "unchanged");
        assert!(rig.texts().contains("loop"), "says why");
    }

    #[test]
    fn right_click_on_an_input_cuts_its_wire() {
        let mut rig = Rig::new(two_blurs);
        let port = in_port(rig.at(3), 0, 1);
        rig.click(port, PointerButton::Secondary);
        assert_eq!(rig.graph().node(3).unwrap().input(0), None);
        assert_eq!(rig.doc.undo_label(), Some("Disconnect"));
    }

    #[test]
    fn right_click_on_a_wire_cuts_it() {
        let mut rig = Rig::new(two_blurs);
        let (a, b) = (rig.at(2), rig.at(3));
        let (p, q) = (out_port(a), in_port(b, 0, 1));
        rig.click(mid_pos(p, q), PointerButton::Secondary);
        assert_eq!(rig.graph().node(3).unwrap().input(0), None);
    }

    #[test]
    fn delete_removes_the_node_and_not_the_clip() {
        let mut rig = Rig::new(two_blurs);
        rig.click(rig.at(2).center(), PointerButton::Primary);
        assert_eq!(rig.st.selected, Some(2));
        rig.key(egui::Key::Delete, Modifiers::NONE);
        let g = rig.graph();
        assert!(g.node(2).is_none());
        assert_eq!(g.node(3).unwrap().input(0), Some(1), "what read it now reads its input");
        assert!(rig.doc.model.active().clip(m::ClipId(rig.id as u64)).is_some(), "the clip is still there");
        assert!(rig.doc.project.clips.iter().any(|c| c.id == rig.id));
        rig.doc.undo();
        assert!(rig.graph().node(2).is_some(), "undo brings the node back");
    }

    #[test]
    fn the_input_node_cannot_be_deleted() {
        let mut rig = Rig::new(two_blurs);
        rig.click(rig.at(1).center(), PointerButton::Primary);
        rig.key(egui::Key::Delete, Modifiers::NONE);
        assert!(rig.graph().node(1).is_some());
    }

    #[test]
    fn shift_drag_on_the_background_selects_a_box_and_delete_takes_them() {
        let mut rig = Rig::new(two_blurs);
        let a = rig.area();
        let (r2, r3) = (rig.at(2), rig.at(3));
        let from = Pos2::new(r2.left() - 30.0, r2.top() - 6.0);
        let to = Pos2::new(r3.right() + 30.0, r3.bottom() + 6.0);
        assert!(a.contains(from) && a.contains(to));
        rig.drag(from, &[mid_pos(from, to), to], Modifiers::SHIFT);
        let mut ids = rig.st.ids();
        ids.sort();
        assert_eq!(ids, vec![2, 3], "the box holds the two blurs, not the input");
        rig.key(egui::Key::Delete, Modifiers::NONE);
        assert_eq!(rig.graph().nodes.len(), 1);
    }

    #[test]
    fn a_plain_drag_on_the_background_pans() {
        let mut rig = Rig::new(two_blurs);
        let pan = rig.st.pan;
        let from = rig.area().left_top() + Vec2::new(30.0, 30.0);
        rig.drag(from, &[from + Vec2::new(25.0, 10.0), from + Vec2::new(50.0, 20.0)], Modifiers::NONE);
        assert!((rig.st.pan - pan - Vec2::new(50.0, 20.0)).length() < 2.0, "{:?}", rig.st.pan - pan);
        assert_eq!(rig.graph(), { let mut g = Graph::passthrough(); two_blurs(&mut g); g }, "the graph is untouched");
    }

    #[test]
    fn duplicate_copies_the_selection_with_its_inner_wire() {
        let mut rig = Rig::new(two_blurs);
        rig.st.select_many(&[2, 3]);
        rig.key(egui::Key::D, Modifiers::COMMAND);
        let g = rig.graph();
        assert_eq!(g.nodes.len(), 5);
        assert_eq!(rig.st.ids(), vec![4, 5], "the copies are selected");
        assert_eq!(g.node(5).unwrap().input(0), Some(4));
        assert_eq!(g.node(4).unwrap().input(0), Some(1));
        assert_eq!(rig.doc.undo_label(), Some("Duplicate nodes"));
    }

    #[test]
    fn f_frames_every_node() {
        let mut rig = Rig::new(|g| {
            let a = g.add("blur", &[Some(1)]);
            g.node_mut(a).unwrap().pos = [900.0, 700.0];
        });
        rig.st.pan = Vec2::new(-2000.0, 500.0);
        rig.st.zoom = 2.0;
        rig.key(egui::Key::F, Modifiers::NONE);
        let area = rig.area();
        for id in [1, 2] {
            assert!(area.contains_rect(rig.at(id)), "node {id} in view: {:?} in {area:?}", rig.at(id));
        }
    }

    #[test]
    fn the_wheel_zooms_about_the_pointer() {
        let mut rig = Rig::new(two_blurs);
        let q = rig.area().center() + Vec2::new(60.0, 30.0);
        let under = rig.st.to_graph(rig.area(), q);
        let z = rig.st.scale();
        rig.moved(q, Modifiers::NONE);
        rig.frame(vec![Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: Vec2::new(0.0, 120.0), phase: egui::TouchPhase::Move, source: egui::MouseWheelSource::Unknown, modifiers: Modifiers::NONE }], Modifiers::NONE);
        assert!(rig.st.scale() > z, "zoomed in");
        let now = rig.st.to_graph(rig.area(), q);
        assert!((now[0] - under[0]).abs() < 1.0 && (now[1] - under[1]).abs() < 1.0);
    }

    #[test]
    fn eye_and_double_click_show_a_node() {
        let mut rig = Rig::new(two_blurs);
        assert_eq!(rig.graph().output_id(), Some(3));
        let r = rig.at(2);
        let eye = Pos2::new(r.right() - 13.0 * rig.st.scale(), r.center().y);
        rig.click(eye, PointerButton::Primary);
        assert_eq!(rig.graph().output_id(), Some(2));
        assert_eq!(rig.doc.undo_label(), Some("View node"));
        let texts = rig.texts();
        assert!(texts.contains("Output"), "{texts}");
    }

    #[test]
    fn search_finds_by_name_description_and_group() {
        assert!(matching("").len() == nodes::all().len(), "all with no query");
        assert!(matching("blur").iter().any(|d| d.id == "blur"));
        assert!(matching("  BLUR ").iter().any(|d| d.id == "blur"), "case and spaces do not matter");
        let group = NodeCategory::Keyer.label();
        assert!(matching(group).iter().any(|d| d.category == NodeCategory::Keyer), "by group");
        assert!(matching("zzzzqqq").is_empty());
    }

    #[test]
    fn tab_on_a_wire_opens_the_search_and_enter_splices_the_node_in() {
        let mut rig = Rig::new(two_blurs);
        let (p, q) = (out_port(rig.at(2)), in_port(rig.at(3), 0, 1));
        rig.moved(mid_pos(p, q), Modifiers::NONE);
        rig.key(egui::Key::Tab, Modifiers::NONE);
        let menu = rig.st.menu.clone().expect("the search is open");
        assert_eq!(menu.wire, Some((3, 0)), "on the wire between the blurs");
        rig.frame(vec![], Modifiers::NONE);
        rig.frame(vec![Event::Text("grade".into())], Modifiers::NONE);
        rig.key(egui::Key::Enter, Modifiers::NONE);
        let g = rig.graph();
        assert!(rig.st.menu.is_none(), "closed after picking");
        let n = g.nodes.iter().find(|n| n.kind == "grade").expect("a grade node");
        assert_eq!(n.inputs, vec![Some(2)]);
        assert_eq!(g.node(3).unwrap().input(0), Some(n.id), "spliced into the wire");
        assert_eq!(g.output_id(), Some(3), "the shown node does not change");
        let at = rig.st.to_graph(rig.area(), mid_pos(p, q));
        assert!((n.pos[0] - at[0]).abs() <= GRID && (n.pos[1] - at[1]).abs() <= GRID, "it appears at the pointer: {:?} vs {at:?}", n.pos);
        assert_eq!(rig.doc.undo_label(), Some("Add node"));
    }

    #[test]
    fn right_click_on_empty_canvas_opens_the_search_there() {
        let mut rig = Rig::new(two_blurs);
        let q = rig.area().right_bottom() - Vec2::new(120.0, 80.0);
        rig.click(q, PointerButton::Secondary);
        let menu = rig.st.menu.clone().expect("open");
        assert_eq!(menu.wire, None);
        assert!(menu.at.is_some());
        let _ = mid(rig.area(), rig.area());
    }

    #[test]
    fn letting_a_wire_go_on_empty_canvas_searches_for_a_node_to_feed() {
        let mut rig = Rig::new(two_blurs);
        let from = out_port(rig.at(3));
        let to = rig.area().right_bottom() - Vec2::new(100.0, 60.0);
        rig.drag(from, &[mid_pos(from, to), to], Modifiers::NONE);
        let menu = rig.st.menu.clone().expect("open");
        assert_eq!(menu.from, Some(3));
        let area = rig.area();
        let id = add_at(&mut rig.graph(), &mut State::default(), area, &menu, "blur");
        assert!(id > 3);
    }

    #[test]
    fn add_after_wires_the_new_node_in_and_views_it() {
        let mut g = Graph::passthrough();
        let mut st = State { selected: Some(1), ..Default::default() };
        let k = add_after(&mut g, &mut st, "keyer");
        assert_eq!(g.node(k).unwrap().inputs, vec![Some(1)]);
        assert_eq!(g.output_id(), Some(k));
        let r = add_after(&mut g, &mut st, "constant");
        assert!(g.node(r).unwrap().inputs.is_empty());
    }

    #[test]
    fn the_empty_graph_says_what_nodes_are_for() {
        let mut rig = Rig::new(|_| {});
        let texts = rig.texts();
        assert!(texts.contains("Build this clip's picture with nodes"), "{texts}");
        assert!(texts.contains("Input flows through"), "{texts}");
        assert!(texts.contains("Add node"), "{texts}");
        assert!(texts.contains("1 node · showing"), "singular: {texts}");
        let mut rig = Rig::new(two_blurs);
        let texts = rig.texts();
        assert!(!texts.contains("Build this clip's picture"), "gone once there is something: {texts}");
        assert!(texts.contains("3 nodes · showing"), "{texts}");
        assert!(!texts.contains("1 nodes"), "{texts}");
    }

    #[test]
    fn the_inspector_names_the_node_and_its_settings() {
        let mut rig = Rig::new(|g| {
            g.add("grade", &[Some(1)]);
        });
        rig.st.select_only(2);
        let texts = rig.texts();
        for want in ["Name", "Enabled", "Mix", "Duplicate", "Delete", "Showing in viewer"] {
            assert!(texts.contains(want), "{want}: {texts}");
        }
        assert!(texts.contains("Gain") || texts.contains("gain"), "its real parameters: {texts}");
    }

    #[test]
    fn status_pluralises() {
        let g = Graph::passthrough();
        assert!(status(&g).starts_with("1 node ·"));
        let mut g = g;
        g.add("blur", &[Some(1)]);
        assert!(status(&g).starts_with("2 nodes ·"));
    }

    #[test]
    fn zoom_keeps_the_point_under_the_pointer() {
        let area = Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 500.0));
        let mut st = State::default();
        let at = Pos2::new(600.0, 100.0);
        let before = (at - area.center() - st.pan) / st.scale();
        st.zoom_at(area, at, 1.5);
        assert!((st.scale() - 1.5).abs() < 1e-4);
        let after = (at - area.center() - st.pan) / st.scale();
        assert!((before - after).length() < 1e-3);
        st.zoom_at(area, at, 100.0);
        assert!(st.scale() <= 2.5);
    }

    #[test]
    fn delete_removes_the_node_not_the_input() {
        let mut g = Graph::passthrough();
        let b = g.add("blur", &[Some(1)]);
        let mut st = State { selected: Some(b), ..Default::default() };
        assert!(delete_selected(&mut g, &mut st));
        assert!(g.node(b).is_none());
        assert_eq!(st.selected, None);
        let mut st = State { selected: Some(1), ..Default::default() };
        assert!(!delete_selected(&mut g, &mut st), "the input stays");
        assert!(g.node(1).is_some());
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
