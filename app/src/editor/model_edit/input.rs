// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Edit mode's pointer and keys: clicking picks (Shift toggles, Alt picks
//! a loop, Cmd+Alt a ring), dragging on the model picks by box, lasso or
//! paint; G, R and S move, turn and scale the pick with the pointer (X, Y
//! or Z locks an axis, twice the pick's own; Shift+axis the other two;
//! typed numbers are exact; Enter or a click keeps it, Esc or a right
//! click puts it back), the gizmo does the same by its handles; tools that
//! carry on with the pointer (extrude, inset, bevel…) set their amount by
//! it; K cuts with the knife and Cmd+R places loop cuts.

use super::draw::{self, Space};
use super::{Session, Stage, with};
use crate::app::StudioApp;
use crate::editor::viewport::ui::{self as vui, Pane};
use crate::editor::viewport::{self as vp, Orient, Pivot, Workspace};
use crate::icons::Icon;
use crate::kit;
use eframe::egui::{self, Color32, Key, Modifiers, Pos2, Stroke, Vec2};
use serde_json::{Map, json};
use std::cell::RefCell;
use studio_mesh::V3;
use studio_modeling::poly::Poly;
use studio_modeling::selection::{Combine, Elem, Mode, Selection};
use studio_modeling::tools::{self, Follow, Kind};

/// What a transform does.
#[derive(Clone, Debug, PartialEq)]
pub enum ModalKind {
    Grab,
    Rotate,
    Scale,
    /// An operation run again each frame with one setting from the
    /// pointer (`dir`: the screen direction that grows it, else distance
    /// from the pivot).
    Tool { id: &'static str, key: &'static str, dir: Option<Vec2>, signed: bool },
}

/// A transform under way.
#[derive(Clone, Debug)]
pub struct Modal {
    pub kind: ModalKind,
    pub pane: usize,
    pub start: Pos2,
    /// Part space.
    pub pivot: V3,
    pub base: Poly,
    pub base_sel: Selection,
    /// How much each vertex follows (1 picked, less with proportional).
    pub weights: Vec<f32>,
    /// Locked axis (0 x, 1 y, 2 z) and whether the other two move
    /// instead (Shift).
    pub axis: Option<usize>,
    pub plane: bool,
    /// Pressing the axis again uses the pick's own axes.
    pub own_axes: bool,
    pub typed: String,
    pub label: String,
    /// Driven by the gizmo: lets go with the pointer.
    pub by_drag: bool,
    /// Pieces turning and scaling about their own middles (part space).
    pub islands: Vec<(Vec<u32>, V3)>,
    /// What the transform settles at (shown in the readout).
    pub said: String,
    /// The setting a tool modal reached.
    pub value: f64,
    /// The last pointer position seen (for the vertex slide direction).
    pub last: Pos2,
    /// Other settings the pointer chose (a slide's direction).
    pub extra: Map<String, serde_json::Value>,
    /// An operation ran before the transform in the same step (duplicate):
    /// cancelling keeps it.
    pub keep: bool,
}

thread_local! {
    /// The pane last under the pointer.
    static PANE: std::cell::Cell<usize> = const { std::cell::Cell::new(3) };
    static DRAG: RefCell<Option<Drag>> = const { RefCell::new(None) };
    static KNIFE: RefCell<Option<Vec<Pos2>>> = const { RefCell::new(None) };
    static LOOPCUT: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
    static MENU: RefCell<Option<(Pos2, &'static str)>> = const { RefCell::new(None) };
    static DROP_FOCUS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// At the end of a frame where Tab switched modes: no widget keeps the
/// keyboard focus Tab gave it.
pub fn end_frame(ctx: &egui::Context) {
    if DROP_FOCUS.with(|d| d.replace(false))
        && let Some(f) = ctx.memory(|m| m.focused())
    {
        ctx.memory_mut(|m| m.surrender_focus(f));
    }
}

#[derive(Clone, Debug)]
enum Drag {
    Region { tool: super::SelectTool, points: Vec<Pos2>, how: Combine },
}

pub fn last_pane() -> usize {
    PANE.with(|p| p.get())
}

/// A menu asked for at a point (right click, X, M…): taken by the menus.
pub fn ask_menu(at: Pos2, which: &'static str) {
    MENU.with(|m| *m.borrow_mut() = Some((at, which)));
}

pub fn take_menu() -> Option<(Pos2, &'static str)> {
    MENU.with(|m| m.borrow_mut().take())
}

pub fn knife_on() -> bool {
    KNIFE.with(|k| k.borrow().is_some())
}

pub fn loopcut_cuts() -> Option<usize> {
    LOOPCUT.with(|l| l.get())
}

/// A key with exactly these modifiers (Cmd and Ctrl count as one), taken
/// so nothing else acts on it.
fn pressed(ctx: &egui::Context, m: Modifiers, k: Key) -> bool {
    ctx.input_mut(|i| {
        let have = i.modifiers;
        let exact = have.alt == m.alt && have.shift == m.shift && (have.command || have.ctrl) == (m.command || m.ctrl);
        exact && i.consume_key(m, k)
    })
}

/// Lets go of the keyboard focus at the end of this frame.
pub fn drop_focus() {
    DROP_FOCUS.with(|d| d.set(true));
}

/// A shortcut as the table writes it ("Shift+D", "Cmd+B", "Alt+E").
pub fn parse_keys(s: &str) -> Option<(Modifiers, Key)> {
    if s.is_empty() || s.contains("Click") || s.contains(' ') {
        return None;
    }
    let mut m = Modifiers::NONE;
    let mut key = None;
    for part in s.split('+') {
        match part {
            "Shift" => m = m | Modifiers::SHIFT,
            "Cmd" => m = m | Modifiers::COMMAND,
            "Alt" => m = m | Modifiers::ALT,
            "Ctrl" => m = m | Modifiers::CTRL,
            "=" => key = Some(Key::Equals),
            "-" => key = Some(Key::Minus),
            k => key = Key::from_name(k),
        }
    }
    Some((m, key?))
}

/// The pane a transform or key works in (as the workspace last drew it).
fn pane_of(_app: &StudioApp, index: usize) -> Option<Pane> {
    let panes = vui::last_panes();
    panes.iter().find(|p| p.index == index).copied().or_else(|| panes.first().copied())
}

/// The point transforms turn and scale about (part space).
pub fn pivot_local(app: &StudioApp, s: &Session) -> Option<V3> {
    let ws = vp::snapshot();
    let p = s.poly();
    let space = Space::of(app, s);
    Some(match ws.pivot {
        Pivot::Cursor => space.map(|sp| sp.to_local(ws.cursor)).unwrap_or([0.0; 3]),
        Pivot::Bounds => s.sel.bounds_centre(p)?,
        Pivot::Active => s.sel.active_point(p).or_else(|| s.sel.median(p))?,
        Pivot::Median | Pivot::Individual => s.sel.median(p)?,
    })
}

/// The axes a transform locks to, in part space: the world's, the
/// view's, or the pick's own (normal orientation).
fn lock_axes(s: &Session, space: &Space, pane: &Pane, ws: &Workspace, own: bool) -> [V3; 3] {
    if own || ws.orient == Orient::Normal {
        if let Some(a) = s.sel.normal_axes(s.poly()) {
            return a;
        }
    }
    match ws.orient {
        Orient::View => {
            let a = pane.nav.axes();
            [space.dir_to_local(a[0]), space.dir_to_local(a[1]), space.dir_to_local(a[2])]
        }
        Orient::Local => [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        _ => [space.dir_to_local([1.0, 0.0, 0.0]), space.dir_to_local([0.0, 1.0, 0.0]), space.dir_to_local([0.0, 0.0, 1.0])],
    }
}

/// Starts moving, turning or scaling the pick.
pub fn start(app: &mut StudioApp, ctx: &egui::Context, kind: ModalKind, by_drag: bool, axis: Option<usize>) -> bool {
    let index = last_pane();
    let Some(pane) = pane_of(app, index) else { return false };
    let Some(s) = with(|s| s.clone()) else { return false };
    if s.stage != Stage::Edit || s.modal.is_some() {
        return false;
    }
    let is_tool = matches!(kind, ModalKind::Tool { .. });
    if !is_tool && !s.sel.verts.iter().any(|b| *b) {
        kit::push(ctx, "Pick something to move first", None, Some(Icon::Info));
        return false;
    }
    let Some(pivot) = pivot_local(app, &s).or_else(|| s.sel.median(s.poly())) else { return false };
    let start = ctx.input(|i| i.pointer.hover_pos()).unwrap_or_else(|| pane.rect.center());
    let p = s.poly().clone();
    let weights = if s.prop.on && s.prop.radius > 0.0 { studio_modeling::selection::falloff_weights(&p, &s.sel.verts, s.prop.radius, s.prop.falloff, s.prop.connected) } else { s.sel.verts.iter().map(|b| if *b { 1.0 } else { 0.0 }).collect() };
    let ws = vp::snapshot();
    let islands = if ws.pivot == Pivot::Individual {
        let groups = if s.sel.mode == Mode::Face { face_islands(&p, &s.sel) } else { studio_modeling::ops::islands(&p, &s.sel.verts) };
        groups.into_iter().map(|g| {
            let c = studio_modeling::ops::centre_of(&p, &g);
            (g, c)
        }).collect()
    } else {
        Vec::new()
    };
    let label = match &kind {
        ModalKind::Grab => "Move".to_string(),
        ModalKind::Rotate => "Rotate".to_string(),
        ModalKind::Scale => "Scale".to_string(),
        ModalKind::Tool { id, .. } => tools::find(id).map_or("Edit", |t| t.label).to_string(),
    };
    if !app.doc.in_batch() {
        app.doc.begin_batch(&label);
    }
    let modal = Modal { kind, pane: index, start, pivot, base: p, base_sel: s.sel.clone(), weights, axis, plane: false, own_axes: false, typed: String::new(), label, by_drag, islands, said: String::new(), value: 0.0, last: start, extra: Map::new(), keep: false };
    with(|cur| {
        if let Some(cur) = cur {
            cur.modal = Some(modal);
        }
    });
    ctx.request_repaint();
    true
}

/// Picked faces grouped by shared edges, as vertex lists.
fn face_islands(p: &Poly, sel: &Selection) -> Vec<Vec<u32>> {
    let t = studio_modeling::poly::Topology::of(p);
    let mut seen = vec![false; p.faces()];
    let mut out = Vec::new();
    for seed in 0..p.faces() {
        if !sel.faces[seed] || seen[seed] {
            continue;
        }
        let mut vs = std::collections::BTreeSet::new();
        let mut stack = vec![seed];
        seen[seed] = true;
        while let Some(f) = stack.pop() {
            let c = p.face(f);
            for i in 0..c.len() {
                vs.insert(c[i]);
                for &g in t.edge_faces.get(&studio_modeling::ekey(c[i], c[(i + 1) % c.len()])).map(Vec::as_slice).unwrap_or(&[]) {
                    if sel.faces[g as usize] && !seen[g as usize] {
                        seen[g as usize] = true;
                        stack.push(g as usize);
                    }
                }
            }
        }
        out.push(vs.into_iter().collect());
    }
    out
}

/// Runs a table operation as a person would: at once, or carrying on with
/// the pointer.
pub fn launch(app: &mut StudioApp, ctx: &egui::Context, id: &'static str) {
    let Some(t) = tools::find(id) else { return };
    if with(|s| s.as_ref().map(|s| s.stage)) != Some(Stage::Edit) {
        return;
    }
    match (id, t.follow) {
        ("knife", _) => {
            KNIFE.with(|k| *k.borrow_mut() = Some(Vec::new()));
            kit::push(ctx, "Knife: click along the cut, Enter to cut, Esc to stop", None, Some(Icon::Scissors));
        }
        ("loop_cut", _) => {
            LOOPCUT.with(|l| l.set(Some(1)));
            kit::push(ctx, "Loop cut: point at an edge, scroll for more cuts, click to cut and slide", None, Some(Icon::Scissors));
        }
        (_, Follow::Move) => {
            app.doc.begin_batch(t.label);
            if super::run(app, ctx, id, Map::new()).is_ok() {
                if start(app, ctx, ModalKind::Grab, false, None) {
                    with(|s| {
                        if let Some(m) = s.as_mut().and_then(|s| s.modal.as_mut()) {
                            m.keep = true;
                        }
                    });
                } else {
                    app.doc.commit_batch();
                }
            } else {
                app.doc.rollback_batch();
            }
        }
        (_, Follow::MoveNormal) => {
            // Along the picked faces' normal (open edges: away from them).
            let dir = with(|s| s.as_ref().and_then(|s| s.sel.normal_axes(s.poly()).map(|a| a[2])));
            let screen = dir.and_then(|d| normal_on_screen(app, d));
            start(app, ctx, ModalKind::Tool { id, key: "distance", dir: screen.or(Some(Vec2::new(0.0, -1.0))), signed: true }, false, None);
        }
        (_, Follow::Drag(key)) => {
            let signed = matches!(t.param(key).map(|p| p.kind), Some(Kind::Length { min, .. } | Kind::Number { min, .. }) if min < 0.0);
            let dir = matches!(t.param(key).map(|p| p.kind), Some(Kind::Number { .. })).then_some(Vec2::new(1.0, 0.0));
            start(app, ctx, ModalKind::Tool { id, key, dir, signed }, false, None);
        }
        _ => {
            let _ = super::run(app, ctx, id, Map::new());
        }
    }
}

/// A part-space direction as a unit screen direction at the pick.
fn normal_on_screen(app: &StudioApp, d: V3) -> Option<Vec2> {
    let s = with(|s| s.clone())?;
    let pane = pane_of(app, last_pane())?;
    let space = Space::of(app, &s)?;
    let c = s.sel.median(s.poly())?;
    let size = studio_tools_size(s.poly());
    let a = pane.to_screen(space.world(c))?;
    let b = pane.to_screen(space.world(studio_mesh::add(c, studio_mesh::scale(d, size * 0.1))))?;
    let v = b - a;
    (v.length() > 1e-3).then(|| v.normalized())
}

fn studio_tools_size(p: &Poly) -> f32 {
    let (lo, hi) = p.bounds();
    studio_mesh::length(studio_mesh::sub(hi, lo)).max(1.0)
}

/// Keys of edit mode (and Tab from object mode). Returns whether the key
/// press was edit mode's.
pub fn keys(app: &mut StudioApp, ctx: &egui::Context) -> bool {
    let tab = ctx.input(|i| i.key_pressed(Key::Tab) && i.modifiers.is_none());
    if tab && (super::open_clip().is_some() || super::candidate(app).is_some()) {
        ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Tab));
        // Tab also walks keyboard focus between widgets: here it is ours,
        // so the focus it hands out this frame is taken back at its end.
        DROP_FOCUS.with(|d| d.set(true));
        let next = if super::stage() == Stage::Edit { Stage::Object } else { Stage::Edit };
        if let Err(e) = super::enter(app, ctx, next) {
            kit::push(ctx, e, None, Some(Icon::Info));
        }
        return true;
    }
    let st = super::stage();
    if st == Stage::Sculpt {
        return super::sculpt3d::keys(app, ctx);
    }
    if st != Stage::Edit {
        return false;
    }
    if with(|s| s.as_ref().is_some_and(|s| s.modal.is_some())) {
        modal_keys(app, ctx);
        return true;
    }
    if super::lattice::keys(ctx) {
        return true;
    }
    if knife_on() {
        if pressed(ctx, Modifiers::NONE, Key::Escape) {
            KNIFE.with(|k| *k.borrow_mut() = None);
        } else if pressed(ctx, Modifiers::NONE, Key::Enter) {
            finish_knife(app, ctx);
        }
        return true;
    }
    if loopcut_cuts().is_some() && pressed(ctx, Modifiers::NONE, Key::Escape) {
        LOOPCUT.with(|l| l.set(None));
        return true;
    }
    // Select modes.
    for (k, m) in [(Key::Num1, Mode::Vertex), (Key::Num2, Mode::Edge), (Key::Num3, Mode::Face)] {
        if pressed(ctx, Modifiers::NONE, k) {
            with(|s| {
                if let Some(s) = s {
                    let p = s.poly().clone();
                    s.sel.set_mode(&p, m);
                }
            });
            return true;
        }
    }
    // Views move to Alt (1 and 3 pick select modes here).
    for (k, a, b) in [(Key::Num1, vp::nav::Axis::Front, vp::nav::Axis::Back), (Key::Num3, vp::nav::Axis::Right, vp::nav::Axis::Left), (Key::Num7, vp::nav::Axis::Top, vp::nav::Axis::Bottom)] {
        if pressed(ctx, Modifiers::ALT, k) {
            vui::view_axis(app, ctx, a);
            return true;
        }
        if pressed(ctx, Modifiers::ALT | Modifiers::COMMAND, k) {
            vui::view_axis(app, ctx, b);
            return true;
        }
    }
    let hover = ctx.input(|i| i.pointer.hover_pos()).unwrap_or_default();
    if with(|s| s.as_ref().is_some_and(|s| s.tool == super::SelectTool::Paint)) {
        brush_keys(ctx);
    }
    if pressed(ctx, Modifiers::NONE, Key::F3) || pressed(ctx, Modifiers::COMMAND, Key::K) {
        super::menus::open_search(ctx);
        return true;
    }
    if pressed(ctx, Modifiers::SHIFT, Key::A) {
        ask_menu(hover, "add");
        return true;
    }
    // Menus at the pointer for operations with a choice.
    for (m, k, which) in [(Modifiers::NONE, Key::X, "delete"), (Modifiers::NONE, Key::Delete, "delete"), (Modifiers::NONE, Key::Backspace, "delete"), (Modifiers::NONE, Key::M, "merge"), (Modifiers::SHIFT, Key::G, "similar"), (Modifiers::NONE, Key::P, "separate"), (Modifiers::ALT, Key::N, "normals"), (Modifiers::NONE, Key::U, "uv")] {
        if pressed(ctx, m, k) {
            ask_menu(hover, which);
            return true;
        }
    }
    // Transforms.
    for (k, kind) in [(Key::G, ModalKind::Grab), (Key::R, ModalKind::Rotate), (Key::S, ModalKind::Scale)] {
        if pressed(ctx, Modifiers::NONE, k) {
            start(app, ctx, kind, false, None);
            return true;
        }
    }
    if pressed(ctx, Modifiers::NONE, Key::W) {
        with(|s| {
            if let Some(s) = s {
                let i = super::SelectTool::ALL.iter().position(|t| *t == s.tool).unwrap_or(0);
                s.tool = super::SelectTool::ALL[(i + 1) % 3];
            }
        });
        return true;
    }
    if pressed(ctx, Modifiers::NONE, Key::O) {
        with(|s| {
            if let Some(s) = s {
                s.prop.on = !s.prop.on;
                if s.prop.radius <= 0.0 {
                    s.prop.radius = studio_tools_size(s.poly()) * 0.25;
                }
            }
        });
        return true;
    }
    if pressed(ctx, Modifiers::ALT, Key::Z) {
        vp::with(|w| {
            w.shade = if w.shade == vp::Shade::Xray { vp::Shade::Solid } else { vp::Shade::Xray };
            w.changed();
        });
        return true;
    }
    if pressed(ctx, Modifiers::NONE, Key::Period) {
        frame_selected(app, ctx);
        return true;
    }
    if pressed(ctx, Modifiers::NONE, Key::Slash) {
        toggle_local_view(app);
        return true;
    }
    if pressed(ctx, Modifiers::NONE, Key::L) {
        // Linked from what is under the pointer.
        let pane = pane_of(app, last_pane());
        if let Some(pane) = pane {
            let ws = vp::snapshot();
            let hit = with(|s| s.as_mut().and_then(|s| draw::under(app, s, &pane, &ws, hover, Mode::Face).or_else(|| draw::under(app, s, &pane, &ws, hover, Mode::Vertex))));
            if let Some(e) = hit {
                with(|s| {
                    if let Some(s) = s {
                        let p = s.poly().clone();
                        let mode = s.sel.mode;
                        s.sel.pick(&p, e, Combine::Add);
                        s.sel.linked(&p, false);
                        s.sel.mode = mode;
                        s.sel.sync(&p);
                    }
                });
            }
        }
        return true;
    }
    // Everything else from the table's shortcuts.
    for t in tools::TOOLS {
        if let Some((m, k)) = parse_keys(t.keys)
            && pressed(ctx, m, k)
        {
            launch(app, ctx, t.id);
            return true;
        }
    }
    false
}

fn modal_keys(app: &mut StudioApp, ctx: &egui::Context) {
    // Undo during a transform puts the transform back (nothing to undo yet).
    if pressed(ctx, Modifiers::NONE, Key::Escape) || pressed(ctx, Modifiers::COMMAND, Key::Z) || pressed(ctx, Modifiers::COMMAND | Modifiers::SHIFT, Key::Z) {
        cancel(app, ctx);
        return;
    }
    if pressed(ctx, Modifiers::NONE, Key::Enter) || pressed(ctx, Modifiers::NONE, Key::Space) {
        confirm(app, ctx);
        return;
    }
    // G G: slide along the surface.
    let kind = with(|s| s.as_ref().and_then(|s| s.modal.as_ref().map(|m| m.kind.clone())));
    if kind == Some(ModalKind::Grab) && pressed(ctx, Modifiers::NONE, Key::G) {
        let (mode, single) = with(|s| s.as_ref().map(|s| (s.sel.mode, s.sel.count().0 == 1))).unwrap_or((Mode::Vertex, false));
        cancel(app, ctx);
        if mode == Mode::Edge || !single {
            start(app, ctx, ModalKind::Tool { id: "edge_slide", key: "factor", dir: Some(Vec2::new(1.0, 0.0)), signed: true }, false, None);
        } else {
            start(app, ctx, ModalKind::Tool { id: "vertex_slide", key: "factor", dir: Some(Vec2::new(1.0, 0.0)), signed: false }, false, None);
        }
        return;
    }
    let shift = ctx.input(|i| i.modifiers.shift);
    for (k, a) in [(Key::X, 0usize), (Key::Y, 1), (Key::Z, 2)] {
        let hit = ctx.input_mut(|i| i.consume_key(Modifiers::NONE, k) || i.consume_key(Modifiers::SHIFT, k));
        if hit {
            with(|s| {
                if let Some(m) = s.as_mut().and_then(|s| s.modal.as_mut()) {
                    if m.axis == Some(a) && m.plane == shift {
                        if m.own_axes {
                            m.axis = None;
                            m.own_axes = false;
                        } else {
                            m.own_axes = true;
                        }
                    } else {
                        m.axis = Some(a);
                        m.plane = shift;
                        m.own_axes = false;
                    }
                }
            });
        }
    }
    // Typed numbers.
    let typed: Vec<char> = ctx.input(|i| {
        i.events
            .iter()
            .filter_map(|e| match e {
                egui::Event::Text(t) => Some(t.chars().filter(|c| c.is_ascii_digit() || *c == '.' || *c == '-').collect::<Vec<_>>()),
                _ => None,
            })
            .flatten()
            .collect()
    });
    let back = pressed(ctx, Modifiers::NONE, Key::Backspace);
    if !typed.is_empty() || back {
        with(|s| {
            if let Some(m) = s.as_mut().and_then(|s| s.modal.as_mut()) {
                if back {
                    m.typed.pop();
                }
                for c in typed {
                    if c == '-' {
                        if m.typed.starts_with('-') {
                            m.typed.remove(0);
                        } else {
                            m.typed.insert(0, '-');
                        }
                    } else {
                        m.typed.push(c);
                    }
                }
            }
        });
    }
    // Proportional reach.
    let (up, down) = (pressed(ctx, Modifiers::NONE, Key::PageUp) || pressed(ctx, Modifiers::NONE, Key::CloseBracket), pressed(ctx, Modifiers::NONE, Key::PageDown) || pressed(ctx, Modifiers::NONE, Key::OpenBracket));
    let wheel = ctx.input(|i| i.smooth_scroll_delta.y);
    if up || down || (wheel.abs() > 0.5 && with(|s| s.as_ref().is_some_and(|s| s.prop.on))) {
        let k = if up || wheel > 0.0 { 1.15 } else { 1.0 / 1.15 };
        with(|s| {
            if let Some(s) = s
                && let Some(m) = &mut s.modal
            {
                s.prop.radius = (s.prop.radius * k).max(1e-3);
                if s.prop.on {
                    m.weights = studio_modeling::selection::falloff_weights(&m.base, &m.base_sel.verts, s.prop.radius, s.prop.falloff, s.prop.connected);
                }
            }
        });
    }
}

/// Keeps the transform: one undo step.
pub fn confirm(app: &mut StudioApp, ctx: &egui::Context) {
    let Some(modal) = with(|s| s.as_mut().and_then(|s| s.modal.take())) else { return };
    match &modal.kind {
        ModalKind::Tool { id, key, .. } => {
            // Run for real from where it started.
            with(|s| {
                if let Some(s) = s {
                    let p = s.part;
                    s.asset.parts[p].mesh = modal.base.clone();
                    s.sel = modal.base_sel.clone();
                    s.changed();
                }
            });
            let mut args = modal.extra.clone();
            args.insert((*key).to_string(), json!(modal.value));
            let id: &'static str = id;
            let _ = super::run(app, ctx, id, args);
        }
        _ => {
            let label = modal.label.clone();
            if let Err(e) = super::commit(app, &label) {
                kit::push(ctx, e, None, Some(Icon::Warning));
            }
            with(|s| {
                if let Some(s) = s {
                    s.last = None;
                }
            });
        }
    }
    if app.doc.in_batch() {
        app.doc.commit_batch();
    }
    ctx.request_repaint();
}

/// Puts everything back as it was before the transform.
pub fn cancel(app: &mut StudioApp, ctx: &egui::Context) {
    let Some(modal) = with(|s| s.as_mut().and_then(|s| s.modal.take())) else { return };
    with(|s| {
        if let Some(s) = s {
            let p = s.part;
            s.asset.parts[p].mesh = modal.base.clone();
            s.sel = modal.base_sel.clone();
            s.changed();
        }
    });
    if app.doc.in_batch() && !modal.keep {
        app.doc.rollback_batch();
    } else if app.doc.in_batch() {
        // A tool run before the move (duplicate) stays; the previews go.
        let file = with(|s| s.as_ref().map(|s| (s.clip, s.file.clone())));
        if let Some((clip, file)) = file
            && let Some(mut mc) = app.doc.model.active().clip(studio_model::ClipId(clip)).cloned()
        {
            crate::mesh_store::set_file(&mut mc, file);
            let seq = app.doc.seq();
            let _ = app.doc.apply_replayed("Back", studio_model::Edit::SetClip { seq, clip: Box::new(mc) });
        }
        app.doc.commit_batch();
    }
    ctx.request_repaint();
}

/// The transform's change for this frame, applied to the open model and
/// previewed.
pub fn step(app: &mut StudioApp, ctx: &egui::Context, pane: &Pane) {
    let Some(mut s) = with(|s| s.clone()) else { return };
    let Some(mut m) = s.modal.clone() else { return };
    let Some(space) = Space::of(app, &s) else { return };
    let ws = vp::snapshot();
    let mouse = ctx.input(|i| i.pointer.hover_pos()).unwrap_or(m.start);
    let snap = ws.snap.on ^ ctx.input(|i| i.modifiers.command);
    let typed: Option<f64> = m.typed.trim_start_matches('-').parse::<f64>().ok().map(|v| if m.typed.starts_with('-') { -v } else { v });
    let pw = space.world(m.pivot);
    let Some(ps) = pane.to_screen(pw) else { return };
    let depth = pane.view.view(pw)[2].max(1.0);
    let wpp = depth / pane.view.zoom / pane.scale as f64;
    let axes = lock_axes(&s, &space, pane, &ws, m.own_axes);
    let world_axis = |a: V3| space.dir_to_world(a);
    let unit = |v: [f64; 3]| {
        let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-12);
        v.map(|x| x / l)
    };
    let screen_rate = |a: [f64; 3]| -> Vec2 {
        let a = unit(a);
        let len = 100.0 * wpp;
        match pane.to_screen([pw[0] + a[0] * len, pw[1] + a[1] * len, pw[2] + a[2] * len]) {
            Some(t) => (t - ps) / len as f32,
            None => Vec2::X,
        }
    };
    let base_world: Vec<[f64; 3]> = m.base.positions.iter().map(|q| space.world(*q)).collect();
    let mut out = m.base.positions.clone();
    let d = mouse - m.start;
    match m.kind.clone() {
        ModalKind::Grab => {
            let view_axes = pane.view.axes;
            let free: [f64; 3] = std::array::from_fn(|j| view_axes[0][j] * d.x as f64 * wpp + view_axes[1][j] * d.y as f64 * wpp);
            let mut mv: [f64; 3] = match (m.axis, m.plane) {
                (Some(i), false) => {
                    let a = unit(world_axis(axes[i]));
                    let r = screen_rate(a);
                    let mut k = (d.dot(r) / r.length_sq().max(1e-9)) as f64;
                    if let Some(t) = typed {
                        k = t;
                    } else if snap && s.snap_to == super::SnapTo::Grid {
                        k = vp::Snap::value(k, ws.snap.step);
                    }
                    a.map(|x| x * k)
                }
                (Some(i), true) => {
                    let a = unit(world_axis(axes[i]));
                    let along = free[0] * a[0] + free[1] * a[1] + free[2] * a[2];
                    std::array::from_fn(|j| free[j] - a[j] * along)
                }
                (None, _) => match typed {
                    Some(t) => unit(world_axis(axes[0])).map(|x| x * t),
                    None => free,
                },
            };
            if snap && typed.is_none() {
                match s.snap_to {
                    super::SnapTo::Grid => {
                        if m.axis.is_none() || m.plane {
                            mv = mv.map(|v| vp::Snap::value(v, ws.snap.step));
                        }
                    }
                    target => {
                        if let Some(t) = snap_target(app, &s, &m, pane, &space, mouse, target) {
                            let want = [t[0] - pw[0], t[1] - pw[1], t[2] - pw[2]];
                            mv = match m.axis {
                                Some(i) if !m.plane => {
                                    let a = unit(world_axis(axes[i]));
                                    let k = want[0] * a[0] + want[1] * a[1] + want[2] * a[2];
                                    a.map(|x| x * k)
                                }
                                _ => want,
                            };
                        }
                    }
                }
            }
            for v in 0..out.len() {
                let w = m.weights.get(v).copied().unwrap_or(0.0) as f64;
                if w > 0.0 {
                    let b = base_world[v];
                    out[v] = space.to_local([b[0] + mv[0] * w, b[1] + mv[1] * w, b[2] + mv[2] * w]);
                }
            }
            let local = space.dir_to_local(mv);
            let dist = (mv[0] * mv[0] + mv[1] * mv[1] + mv[2] * mv[2]).sqrt();
            m.said = match m.axis {
                Some(i) => format!("{} {:.1} px", ["X", "Y", "Z"][i], if typed.is_some() { typed.unwrap_or(0.0) } else { dist * (local[i].signum() as f64) }),
                None => format!("{:.1}, {:.1}, {:.1} px", mv[0], mv[1], mv[2]),
            };
        }
        ModalKind::Rotate => {
            let (a0, a1) = ((m.start - ps).angle(), (mouse - ps).angle());
            let mut deg = (a1 - a0).to_degrees() as f64;
            deg -= (deg / 360.0).round() * 360.0;
            let ax = match m.axis {
                Some(i) => unit(world_axis(axes[i])),
                None => {
                    let f = pane.view.axes[2];
                    [-f[0], -f[1], -f[2]]
                }
            };
            let fwd = pane.view.axes[2];
            let away = ax[0] * fwd[0] + ax[1] * fwd[1] + ax[2] * fwd[2];
            deg = if away > 0.0 { deg } else { -deg };
            if let Some(t) = typed {
                deg = t;
            } else if snap {
                deg = vp::Snap::value(deg, ws.snap.angle);
            }
            let r = vp::about(ax, deg);
            let turn = |b: [f64; 3], c: [f64; 3], w: f64| -> [f64; 3] {
                let l = [b[0] - c[0], b[1] - c[1], b[2] - c[2]];
                let q = studio_model::space::apply(&r, l);
                let full = [c[0] + q[0], c[1] + q[1], c[2] + q[2]];
                std::array::from_fn(|j| b[j] + (full[j] - b[j]) * w)
            };
            apply_about(&m, &space, &base_world, pw, &mut out, |b, c, w| turn(b, c, w));
            m.said = format!("{deg:.1}°{}", m.axis.map(|i| format!(" about {}", ["X", "Y", "Z"][i])).unwrap_or_default());
        }
        ModalKind::Scale => {
            let d0 = (m.start - ps).length().max(4.0);
            let mut k = ((mouse - ps).length() / d0) as f64;
            if let Some(t) = typed {
                k = t;
            } else if snap {
                k = 1.0 + vp::Snap::value(k - 1.0, ws.snap.scale);
            }
            let lock = m.axis.map(|i| (unit(world_axis(axes[i])), m.plane));
            let scale = |b: [f64; 3], c: [f64; 3], w: f64| -> [f64; 3] {
                let l = [b[0] - c[0], b[1] - c[1], b[2] - c[2]];
                let q: [f64; 3] = match lock {
                    None => l.map(|x| x * k),
                    Some((a, false)) => {
                        let along = l[0] * a[0] + l[1] * a[1] + l[2] * a[2];
                        std::array::from_fn(|j| l[j] + a[j] * along * (k - 1.0))
                    }
                    Some((a, true)) => {
                        let along = l[0] * a[0] + l[1] * a[1] + l[2] * a[2];
                        std::array::from_fn(|j| (l[j] - a[j] * along) * k + a[j] * along)
                    }
                };
                std::array::from_fn(|j| b[j] + (c[j] + q[j] - b[j]) * w)
            };
            apply_about(&m, &space, &base_world, pw, &mut out, |b, c, w| scale(b, c, w));
            m.said = format!("× {k:.3}{}", m.axis.map(|i| format!(" along {}", ["X", "Y", "Z"][i])).unwrap_or_default());
        }
        ModalKind::Tool { id, key, dir, signed } => {
            let t = tools::find(id);
            let kind = t.and_then(|t| t.param(key)).map(|p| p.kind);
            let mut v = match dir {
                Some(dv) => {
                    let along = d.dot(dv) as f64;
                    match kind {
                        Some(Kind::Number { min, max, .. }) => (along / 250.0 * (max - min) as f64 * 0.5).clamp(min as f64, max as f64),
                        _ => along * wpp,
                    }
                }
                None => {
                    let grow = ((mouse - ps).length() - (m.start - ps).length()) as f64;
                    let v = grow * wpp;
                    if signed { v } else { v.abs() }
                }
            };
            if matches!(kind, Some(Kind::Number { .. })) && dir.is_none() {
                v = (v / (studio_tools_size(&m.base) as f64)).clamp(0.0, 1.0);
            }
            if let Some(tv) = typed {
                v = tv;
            } else if snap {
                v = vp::Snap::value(v, if matches!(kind, Some(Kind::Number { .. })) { 0.05 } else { ws.snap.step });
            }
            if id == "vertex_slide" {
                // Toward where the pointer goes.
                let dirw: [f64; 3] = std::array::from_fn(|j| pane.view.axes[0][j] * d.x as f64 + pane.view.axes[1][j] * d.y as f64);
                v = v.abs();
                m.last = mouse;
                let mut args = Map::new();
                args.insert("factor".into(), json!(v.clamp(0.0, 1.0)));
                let tl = space.dir_to_local(dirw);
                args.insert("toward".into(), json!([tl[0], tl[1], tl[2]]));
                m.extra.insert("toward".into(), json!([tl[0], tl[1], tl[2]]));
                let mut p = m.base.clone();
                let mut sel = m.base_sel.clone();
                let c = super::local_ctx(app, Some(&pane.view));
                if tools::run(&mut p, &mut sel, id, &args, &c).is_ok() {
                    s.asset.parts[s.part].mesh = p;
                }
                m.value = v;
                m.said = format!("{:.0} %", v.clamp(0.0, 1.0) * 100.0);
                s.modal = Some(m);
                with(|cur| *cur = Some(s));
                super::preview(app);
                return;
            }
            if let Some(Param { kind: Kind::Count { min, max, .. }, .. }) = t.and_then(|t| t.param(key)).copied() {
                v = v.round().clamp(min as f64, max as f64);
            }
            m.value = v;
            let mut args = Map::new();
            args.insert(key.to_string(), json!(v));
            let mut p = m.base.clone();
            let mut sel = m.base_sel.clone();
            let mut c = super::local_ctx(app, Some(&pane.view));
            c.pivot = Some(m.pivot);
            match tools::run(&mut p, &mut sel, id, &args, &c) {
                Ok(_) => {
                    s.asset.parts[s.part].mesh = p;
                    s.sel = sel;
                    s.changed();
                }
                Err(e) => m.said = e,
            }
            let unit = match kind {
                Some(Kind::Length { .. }) => " px",
                _ => "",
            };
            if !m.said.starts_with("pick") {
                m.said = format!("{} {v:.2}{unit}", t.map_or("", |t| t.label));
            }
            s.modal = Some(m);
            with(|cur| *cur = Some(s));
            super::preview(app);
            return;
        }
    }
    // Symmetric editing: mirror images follow.
    for (axis, on) in s.mirror.iter().enumerate() {
        if !on {
            continue;
        }
        let (lo, hi) = m.base.bounds();
        let tol = studio_mesh::length(studio_mesh::sub(hi, lo)).max(1.0) * 1e-4;
        let map = studio_modeling::selection::mirror_map(&m.base, axis, tol);
        for v in 0..out.len() {
            if m.weights.get(v).copied().unwrap_or(0.0) <= 0.0 {
                continue;
            }
            match map[v] {
                Some(u) if u as usize == v => out[v][axis] = 0.0,
                Some(u) if m.weights.get(u as usize).copied().unwrap_or(0.0) <= 0.0 => {
                    let mut q = out[v];
                    q[axis] = -q[axis];
                    out[u as usize] = q;
                }
                _ => {}
            }
        }
    }
    let p = s.part;
    s.asset.parts[p].mesh.positions = out;
    s.modal = Some(m);
    with(|cur| *cur = Some(s));
    super::preview(app);
}

use studio_modeling::tools::Param;

/// Turns or scales each vertex about the pivot (or its piece's middle).
fn apply_about(m: &Modal, space: &Space, base_world: &[[f64; 3]], pw: [f64; 3], out: &mut [V3], f: impl Fn([f64; 3], [f64; 3], f64) -> [f64; 3]) {
    if m.islands.is_empty() {
        for v in 0..out.len() {
            let w = m.weights.get(v).copied().unwrap_or(0.0) as f64;
            if w > 0.0 {
                out[v] = space.to_local(f(base_world[v], pw, w));
            }
        }
    } else {
        for (vs, c) in &m.islands {
            let cw = space.world(*c);
            for &v in vs {
                let v = v as usize;
                out[v] = space.to_local(f(base_world[v], cw, 1.0));
            }
        }
    }
}

/// Where a snapping move lands: the nearest vertex, point on an edge, or
/// point on a face under the pointer (world), from what isn't moving.
fn snap_target(app: &StudioApp, s: &Session, m: &Modal, pane: &Pane, space: &Space, mouse: Pos2, to: super::SnapTo) -> Option<[f64; 3]> {
    let p = &m.base;
    let moving = |v: usize| m.weights.get(v).copied().unwrap_or(0.0) > 0.0;
    match to {
        super::SnapTo::Vertex => {
            let mut best: Option<(f32, usize)> = None;
            for v in 0..p.positions.len() {
                if moving(v) {
                    continue;
                }
                if let Some(q) = pane.to_screen(space.world(p.positions[v])) {
                    let d = q.distance(mouse);
                    if d < 24.0 && best.is_none_or(|b| d < b.0) {
                        best = Some((d, v));
                    }
                }
            }
            best.map(|b| space.world(p.positions[b.1]))
        }
        super::SnapTo::Edge => {
            let mut best: Option<(f32, [f64; 3])> = None;
            for e in p.edges() {
                if moving(e.0 as usize) || moving(e.1 as usize) {
                    continue;
                }
                let (wa, wb) = (space.world(p.positions[e.0 as usize]), space.world(p.positions[e.1 as usize]));
                if let (Some(a), Some(b)) = (pane.to_screen(wa), pane.to_screen(wb)) {
                    let ab = b - a;
                    let t = ((mouse - a).dot(ab) / ab.length_sq().max(1e-6)).clamp(0.0, 1.0);
                    let d = mouse.distance(a + ab * t);
                    if d < 18.0 && best.is_none_or(|x| d < x.0) {
                        best = Some((d, std::array::from_fn(|j| wa[j] + (wb[j] - wa[j]) * t as f64)));
                    }
                }
            }
            best.map(|b| b.1)
        }
        super::SnapTo::Face => {
            let (o, d) = draw::ray_local(space, pane, mouse)?;
            let mut stat = s.clone();
            stat.asset.parts[s.part].mesh = m.base.clone();
            let hit = draw::with_hits(&stat, |h| h.bvh.ray(o, d, 0.0, f32::INFINITY).map(|r| r.t))?;
            let q = studio_mesh::add(o, studio_mesh::scale(d, hit));
            let _ = app;
            Some(space.world(q))
        }
        super::SnapTo::Grid => None,
    }
}

/// Frames the picked vertices (or the whole model).
pub fn frame_selected(app: &mut StudioApp, ctx: &egui::Context) {
    let Some(s) = with(|s| s.clone()) else { return };
    let Some(space) = Space::of(app, &s) else { return };
    let p = s.poly();
    let picked: Vec<usize> = (0..p.positions.len()).filter(|&v| s.sel.verts[v]).collect();
    let list: Vec<usize> = if picked.is_empty() { (0..p.positions.len()).collect() } else { picked };
    if list.is_empty() {
        return;
    }
    let pts: Vec<[f64; 3]> = list.iter().map(|&v| space.world(p.positions[v])).collect();
    let n = pts.len() as f64;
    let c: [f64; 3] = std::array::from_fn(|j| pts.iter().map(|q| q[j]).sum::<f64>() / n);
    let r = pts.iter().map(|q| ((q[0] - c[0]).powi(2) + (q[1] - c[1]).powi(2) + (q[2] - c[2]).powi(2)).sqrt()).fold(0.0f64, f64::max).max(10.0);
    let w = app.doc.project.width as f64;
    let aspect = app.doc.project.width as f64 / app.doc.project.height.max(1) as f64;
    let t = ctx.input(|i| i.time);
    vp::with(|ws| {
        let mut nav = *ws.nav_mut();
        nav.frame(c, r * 1.1, w, aspect);
        ws.glide_to(nav, t);
    });
    ctx.request_repaint();
}

/// Only the open model shows (again: everything as it was).
pub fn toggle_local_view(app: &mut StudioApp) {
    let Some(clip) = super::open_clip() else { return };
    let was = with(|s| s.as_mut().and_then(|s| s.local.take()));
    match was {
        Some(hidden) => vp::with(|w| {
            w.hidden = hidden;
            w.changed();
        }),
        None => {
            let before = vp::snapshot().hidden;
            let others: std::collections::BTreeSet<u64> = app.doc.model.active().clips.iter().map(|c| c.id.0).filter(|id| *id != clip).collect();
            vp::with(|w| {
                w.hidden = others;
                w.changed();
            });
            with(|s| {
                if let Some(s) = s {
                    s.local = Some(before);
                }
            });
        }
    }
}

/// The pointer over a pane in edit mode: picks, region drags, the gizmo,
/// the knife and the loop cut. Navigation stays the workspace's.
pub fn interact(app: &mut StudioApp, ui: &mut egui::Ui, pane: &Pane, resp: &egui::Response) {
    let ctx = ui.ctx().clone();
    PANE.with(|p| p.set(pane.index));
    let Some(mut s) = with(|s| s.clone()) else { return };
    if s.stage != Stage::Edit || s.modal.is_some() {
        return;
    }
    let ws = vp::snapshot();
    let (mods, pointer) = ctx.input(|i| (i.modifiers, i.pointer.hover_pos()));
    let primary_down = ctx.input(|i| i.pointer.primary_down());
    // The knife: clicks add points along the cut.
    if knife_on() {
        if resp.clicked()
            && let Some(p) = pointer
        {
            KNIFE.with(|k| {
                if let Some(v) = k.borrow_mut().as_mut() {
                    v.push(p);
                }
            });
        }
        if resp.secondary_clicked() {
            finish_knife(app, &ctx);
        }
        return;
    }
    // Loop cut: the ring under the pointer, a click cuts and slides.
    if let Some(cuts) = loopcut_cuts() {
        let wheel = ctx.input(|i| i.smooth_scroll_delta.y);
        if wheel.abs() > 0.5 {
            LOOPCUT.with(|l| l.set(Some(((cuts as i64) + if wheel > 0.0 { 1 } else { -1 }).clamp(1, 64) as usize)));
        }
        if resp.clicked()
            && let Some(p) = pointer
            && let Some(Elem::Edge(e)) = draw::under(app, &mut s, pane, &ws, p, Mode::Edge)
        {
            LOOPCUT.with(|l| l.set(None));
            app.doc.begin_batch("Loop cut and slide");
            let mut args = Map::new();
            args.insert("edge".into(), json!([e.0, e.1]));
            args.insert("cuts".into(), json!(cuts));
            if super::run(app, &ctx, "loop_cut", args).is_ok() {
                // Slide what was cut (one cut: along its rails).
                if cuts == 1 {
                    app.doc.commit_batch();
                    start(app, &ctx, ModalKind::Tool { id: "edge_slide", key: "factor", dir: Some(Vec2::new(1.0, 0.0)), signed: true }, false, None);
                } else {
                    app.doc.commit_batch();
                }
            } else {
                app.doc.rollback_batch();
            }
        }
        return;
    }
    // A lattice cage takes the pointer while it is edited.
    if super::lattice::on() {
        super::lattice::interact(app, &ctx, pane, resp);
        return;
    }
    // The gizmo.
    if ws.tool != vp::Tool::Select && s.sel.verts.iter().any(|b| *b)
        && let Some(space) = Space::of(app, &s)
        && let Some(piv) = pivot_local(app, &s)
    {
        let pw = space.world(piv);
        let axes3 = lock_axes(&s, &space, pane, &ws, false).map(|a| space.dir_to_world(a)).map(|a| {
            let l = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt().max(1e-12);
            a.map(|x| x / l)
        });
        if let Some((a, tips, _len, rings)) = vui::gizmo_parts(pane, pw, &axes3) {
            let hot = pointer.and_then(|p| vui::handle_under(ws.tool, p, a, &tips, &rings));
            vui::draw_gizmo(&ui.painter().with_clip_rect(pane.rect), ws.tool, a, &tips, &rings, hot);
            if resp.drag_started() && primary_down && !mods.alt
                && let Some(h) = hot
            {
                let kind = match ws.tool {
                    vp::Tool::Rotate => ModalKind::Rotate,
                    vp::Tool::Scale => ModalKind::Scale,
                    _ => ModalKind::Grab,
                };
                let axis = match h {
                    vui::Handle::Axis(i) | vui::Handle::Ring(i) => Some(i),
                    vui::Handle::Centre => None,
                };
                with(|cur| {
                    if let Some(cur) = cur {
                        cur.modal = None;
                    }
                });
                if start(app, &ctx, kind, true, axis) {
                    // The gizmo's axes are the orientation's: keep them.
                    with(|cur| {
                        if let Some(m) = cur.as_mut().and_then(|c| c.modal.as_mut()) {
                            m.start = ctx.input(|i| i.pointer.press_origin()).unwrap_or(m.start);
                        }
                    });
                }
                return;
            }
        }
    }
    // Region picks.
    let how = if mods.shift { Combine::Add } else if mods.command || mods.ctrl { Combine::Subtract } else { Combine::Set };
    if resp.drag_started() && primary_down && !mods.alt
        && let Some(p) = ctx.input(|i| i.pointer.press_origin())
    {
        DRAG.with(|d| *d.borrow_mut() = Some(Drag::Region { tool: s.tool, points: vec![p], how }));
        if s.tool == super::SelectTool::Paint && how == Combine::Set {
            with(|cur| {
                if let Some(cur) = cur {
                    cur.sel.clear();
                }
            });
        }
    }
    let dragging = DRAG.with(|d| d.borrow().is_some());
    if dragging && let Some(p) = pointer {
        let tool = DRAG.with(|d| {
            let mut d = d.borrow_mut();
            let Some(Drag::Region { tool, points, .. }) = d.as_mut() else { return None };
            if points.last().is_none_or(|l| l.distance(p) > 2.0) {
                points.push(p);
            }
            Some(*tool)
        });
        if tool == Some(super::SelectTool::Paint) {
            let r = s.brush;
            region_pick(app, pane, &ws, &move |q: Pos2| q.distance(p) <= r, if how == Combine::Subtract { Combine::Subtract } else { Combine::Add });
        }
    }
    if dragging && ctx.input(|i| i.pointer.any_released()) {
        let d = DRAG.with(|d| d.borrow_mut().take());
        if let Some(Drag::Region { tool, points, how }) = d
            && points.len() >= 2
        {
            match tool {
                super::SelectTool::Box => {
                    let r = egui::Rect::from_two_pos(points[0], *points.last().unwrap());
                    region_pick(app, pane, &ws, &move |q: Pos2| r.contains(q), how);
                }
                super::SelectTool::Lasso => {
                    let poly = points.clone();
                    region_pick(app, pane, &ws, &move |q: Pos2| inside_polygon(&poly, q), how);
                }
                super::SelectTool::Paint => {}
            }
        }
    }
    // Clicks.
    if resp.clicked()
        && let Some(p) = pointer
    {
        let ring = mods.alt && (mods.command || mods.ctrl);
        if mods.alt {
            if let Some(Elem::Edge(e)) = draw::under(app, &mut s, pane, &ws, p, Mode::Edge) {
                with(|cur| {
                    if let Some(cur) = cur {
                        let poly = cur.poly().clone();
                        let how = if mods.shift { Combine::Add } else { Combine::Set };
                        if ring { cur.sel.pick_ring(&poly, e, how) } else { cur.sel.pick_loop(&poly, e, how) }
                    }
                });
            }
        } else {
            let want = s.sel.mode;
            let hit = draw::under(app, &mut s, pane, &ws, p, want);
            with(|cur| {
                if let Some(cur) = cur {
                    let poly = cur.poly().clone();
                    match hit {
                        Some(e) => cur.sel.pick(&poly, e, if mods.shift { Combine::Toggle } else if mods.command { Combine::Subtract } else { Combine::Set }),
                        None if !mods.shift && !mods.command => cur.sel.clear(),
                        None => {}
                    }
                }
            });
        }
    }
    if resp.double_clicked()
        && let Some(p) = pointer
        && let want = s.sel.mode
        && let Some(e) = draw::under(app, &mut s, pane, &ws, p, want)
    {
        with(|cur| {
            if let Some(cur) = cur {
                let poly = cur.poly().clone();
                cur.sel.pick(&poly, e, Combine::Add);
                cur.sel.linked(&poly, false);
            }
        });
    }
    // Right click: the menu for this select mode; Shift: the 3D cursor.
    if resp.secondary_clicked()
        && let Some(p) = pointer
    {
        if mods.shift {
            place_cursor(app, pane, p);
        } else {
            ask_menu(p, "context");
        }
    }
}

fn inside_polygon(poly: &[Pos2], q: Pos2) -> bool {
    let mut inside = false;
    let n = poly.len();
    let mut j = n - 1;
    for i in 0..n {
        let (a, b) = (poly[i], poly[j]);
        if (a.y > q.y) != (b.y > q.y) && q.x < (b.x - a.x) * (q.y - a.y) / (b.y - a.y) + a.x {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Picks what falls inside a screen shape (seen vertices only unless the
/// view is see-through).
fn region_pick(app: &StudioApp, pane: &Pane, ws: &Workspace, inside: &dyn Fn(Pos2) -> bool, how: Combine) {
    let Some(mut s) = with(|s| s.clone()) else { return };
    let Some(space) = Space::of(app, &s) else { return };
    let seen = draw::seen(&mut s, &space, pane, ws);
    let p = s.poly().clone();
    let test = |q: V3| pane.to_screen(space.world(q)).is_some_and(|sp| inside(sp));
    let reach = (!draw::see_through(ws)).then_some(&seen[..]);
    s.sel.region(&p, &test, reach, how);
    with(|cur| {
        if let Some(cur) = cur {
            cur.sel = s.sel;
            cur.cage = s.cage;
        }
    });
}

/// Puts the 3D cursor where the pointer meets the model (or the floor).
fn place_cursor(app: &StudioApp, pane: &Pane, at: Pos2) {
    let Some(s) = with(|s| s.clone()) else { return };
    let Some(space) = Space::of(app, &s) else { return };
    if let Some((o, d)) = draw::ray_local(&space, pane, at)
        && let Some(t) = draw::with_hits(&s, |h| h.bvh.ray(o, d, 0.0, f32::INFINITY).map(|r| r.t))
    {
        let q = studio_mesh::add(o, studio_mesh::scale(d, t));
        let w = space.world(q);
        vp::with(|ws| {
            ws.cursor = w;
            ws.changed();
        });
    }
}

/// Cuts along the knife's line: each segment is a plane through the eye,
/// limited to the strip between its ends.
fn finish_knife(app: &mut StudioApp, ctx: &egui::Context) {
    let pts = KNIFE.with(|k| k.borrow_mut().take()).unwrap_or_default();
    if pts.len() < 2 {
        return;
    }
    let Some(pane) = pane_of(app, last_pane()) else { return };
    let Some(mut s) = with(|s| s.clone()) else { return };
    let Some(space) = Space::of(app, &s) else { return };
    let eye = space.to_local(pane.view.position);
    let before = (s.poly().clone(), s.sel.clone());
    let mut poly = s.poly().clone();
    let pick: Vec<bool> = if s.sel.faces.iter().any(|b| *b) { s.sel.faces.clone() } else { s.sel.visible(&poly).faces };
    let mut made = Vec::new();
    for w in pts.windows(2) {
        let (Some((_, da)), Some((_, db))) = (draw::ray_local(&space, &pane, w[0]), draw::ray_local(&space, &pane, w[1])) else { continue };
        let normal = studio_mesh::normalise(studio_mesh::cross(da, db));
        if studio_mesh::length(normal) < 0.5 {
            continue;
        }
        // Between the two side planes through the eye and each end.
        let mid = studio_mesh::normalise(studio_mesh::add(da, db));
        let side_a = studio_mesh::cross(normal, da);
        let side_b = studio_mesh::cross(db, normal);
        let (sa, sb) = (studio_mesh::dot(side_a, mid).signum(), studio_mesh::dot(side_b, mid).signum());
        let within = move |q: V3| {
            let r = studio_mesh::sub(q, eye);
            studio_mesh::dot(side_a, r) * sa >= -1e-3 && studio_mesh::dot(side_b, r) * sb >= -1e-3
        };
        let marked = studio_modeling::ops::keep_face_pick(&mut poly, &pick, |_| {});
        let plane = studio_modeling::edit::Plane { at: eye, normal };
        let all: Vec<bool> = marked.iter().map(|_| true).collect();
        let new = studio_modeling::edit::knife(&mut poly, &all, plane, &within);
        made.extend(new);
    }
    if made.is_empty() {
        kit::push(ctx, "The line didn't cross any faces", None, Some(Icon::Info));
        return;
    }
    let p = s.part;
    s.asset.parts[p].mesh = poly.clone();
    s.sel.fit(&poly);
    let mode = s.sel.mode;
    s.sel.mode = Mode::Edge;
    s.sel.with_edges(&poly, &made);
    s.sel.mode = if mode == Mode::Face { Mode::Edge } else { mode };
    s.sel.sync(&poly);
    s.changed();
    s.last = Some(super::Last { id: "knife", args: Map::new(), ctx: super::local_ctx(app, Some(&pane.view)), before, step: Some("Knife".into()), said: format!("{} new edges", made.len()) });
    with(|cur| *cur = Some(s));
    if let Err(e) = super::commit(app, "Knife") {
        kit::push(ctx, e, None, Some(Icon::Warning));
    }
}

/// What edit mode draws over the panes each frame: the region being
/// dragged, the knife's line, the loop cut to come, and the transform
/// readout; and the transform itself steps here.
pub fn frame(app: &mut StudioApp, ui: &mut egui::Ui, panes: &[Pane]) {
    let ctx = ui.ctx().clone();
    if super::stage() != Stage::Edit {
        return;
    }
    let modal_pane = with(|s| s.as_ref().and_then(|s| s.modal.as_ref().map(|m| m.pane)));
    if let Some(index) = modal_pane {
        let Some(pane) = panes.iter().find(|p| p.index == index).copied().or_else(|| panes.first().copied()) else { return };
        step(app, &ctx, &pane);
        // A click keeps it, a right click puts it back; a gizmo drag
        // keeps it when let go.
        let (clicked, right, released) = ctx.input(|i| (i.pointer.button_clicked(egui::PointerButton::Primary), i.pointer.button_clicked(egui::PointerButton::Secondary), i.pointer.any_released()));
        let by_drag = with(|s| s.as_ref().and_then(|s| s.modal.as_ref().map(|m| m.by_drag))).unwrap_or(false);
        if right {
            cancel(app, &ctx);
        } else if clicked || (by_drag && released) {
            confirm(app, &ctx);
        }
        if let Some(said) = with(|s| s.as_ref().and_then(|s| s.modal.as_ref().map(|m| format!("{}  {}{}", m.label, m.said, if m.typed.is_empty() { String::new() } else { format!("  [{}]", m.typed) })))) {
            let at = ctx.input(|i| i.pointer.hover_pos()).unwrap_or(pane.rect.center());
            vui::readout(ui, at, &said);
            let th = crate::theme::tokens(&ctx);
            ui.painter().text(pane.rect.center_bottom() - egui::vec2(0.0, 30.0), egui::Align2::CENTER_BOTTOM, crate::i18n::t("X Y Z lock an axis (twice: its own) · Shift+axis the other two · type a number · Enter or click keeps · Esc or right click puts it back"), crate::theme::caption(), th.text);
        }
        ctx.request_repaint();
        return;
    }
    let pane = panes.iter().find(|p| p.index == last_pane()).copied().or_else(|| panes.first().copied());
    let Some(pane) = pane else { return };
    let painter = ui.painter().with_clip_rect(pane.rect);
    let pointer = ctx.input(|i| i.pointer.hover_pos());
    DRAG.with(|d| {
        if let Some(Drag::Region { tool, points, .. }) = d.borrow().as_ref() {
            let st = Stroke::new(1.0, Color32::WHITE);
            match tool {
                super::SelectTool::Box if points.len() >= 2 => {
                    painter.rect_stroke(egui::Rect::from_two_pos(points[0], *points.last().unwrap()), 0, st, egui::StrokeKind::Inside);
                }
                super::SelectTool::Lasso if points.len() >= 2 => {
                    let mut l = points.clone();
                    l.push(points[0]);
                    painter.add(egui::Shape::line(l, st));
                }
                _ => {}
            }
        }
    });
    if let Some(p) = pointer
        && with(|s| s.as_ref().is_some_and(|s| s.tool == super::SelectTool::Paint))
        && pane.rect.contains(p)
    {
        let r = with(|s| s.as_ref().map_or(24.0, |s| s.brush));
        painter.circle_stroke(p, r, Stroke::new(1.0, Color32::from_white_alpha(170)));
    }
    if let Some(pts) = KNIFE.with(|k| k.borrow().clone()) {
        let mut l = pts.clone();
        if let Some(p) = pointer {
            l.push(p);
        }
        if l.len() >= 2 {
            painter.add(egui::Shape::line(l.clone(), Stroke::new(1.5, Color32::from_rgb(120, 255, 120))));
        }
        for p in &pts {
            painter.circle_filled(*p, 3.0, Color32::from_rgb(120, 255, 120));
        }
        let th = crate::theme::tokens(&ctx);
        painter.text(pane.rect.center_bottom() - egui::vec2(0.0, 30.0), egui::Align2::CENTER_BOTTOM, crate::i18n::t("Knife: click points along the cut · Enter or right click cuts · Esc stops"), crate::theme::caption(), th.text);
    }
    if let (Some(cuts), Some(p)) = (loopcut_cuts(), pointer) {
        let ws = vp::snapshot();
        let mut s = with(|s| s.clone()).unwrap();
        if let Some(Elem::Edge(e)) = draw::under(app, &mut s, &pane, &ws, p, Mode::Edge)
            && let Some(space) = Space::of(app, &s)
        {
            // The loops to come, across the ring.
            let poly = s.poly();
            let t = studio_modeling::poly::Topology::of(poly);
            for (r, _) in studio_modeling::select::edge_ring(poly, &t, e) {
                let (a, b) = (poly.positions[r.0 as usize], poly.positions[r.1 as usize]);
                for k in 1..=cuts {
                    let f = k as f32 / (cuts + 1) as f32;
                    let q = studio_mesh::lerp(a, b, f);
                    if let Some(sp) = pane.to_screen(space.world(q)) {
                        painter.circle_filled(sp, 2.5, Color32::from_rgb(255, 230, 60));
                    }
                }
            }
        }
        let th = crate::theme::tokens(&ctx);
        painter.text(pane.rect.center_bottom() - egui::vec2(0.0, 30.0), egui::Align2::CENTER_BOTTOM, crate::i18n::tf(format!("Loop cut: {cuts} · scroll for more · click to cut and slide · Esc stops")), crate::theme::caption(), th.text);
    }
}

/// Settings of the select tools a person changes with keys while
/// painting: [ and ] the brush size.
pub fn brush_keys(ctx: &egui::Context) {
    let (smaller, bigger) = ctx.input(|i| (i.key_pressed(Key::OpenBracket), i.key_pressed(Key::CloseBracket)));
    if smaller || bigger {
        with(|s| {
            if let Some(s) = s {
                s.brush = (s.brush * if bigger { 1.2 } else { 1.0 / 1.2 }).clamp(4.0, 400.0);
            }
        });
    }
}
