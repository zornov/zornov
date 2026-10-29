// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The vector art window: draw and edit illustration documents (`.svart`
//! in the project's Vector folder) on a live canvas with the selection,
//! direct selection (anchors and handles), pen, curvature, pencil, shape,
//! type, knife, eraser and blob brush tools; a layers list; fill, stroke,
//! opacity and blend; pathfinder, align, arrange, group, clip, outline
//! type, expand and trace; import and export (SVG, PDF, EPS, PNG); and
//! placing the art on the timeline. Every change is one of the vector ops
//! the AI uses, so the app and the AI make the same things.

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};
use serde_json::{Value, json};
use studio_vector_art::raster::kurbo::{Affine, Point};
use studio_vector_art::{Document, NodeId, NodeKind, build, ops};

use crate::app::StudioApp;
use crate::icons::Icon;
use crate::kit::{self, IconButton};
use crate::kit::ShowNoted;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tool {
    #[default]
    Select,
    Direct,
    Pen,
    Curvature,
    Pencil,
    Rect,
    Ellipse,
    Polygon,
    Star,
    Line,
    Text,
    Knife,
    Eraser,
    Blob,
}

impl Tool {
    pub const ALL: [(Tool, &'static str, &'static str); 14] = [
        (Tool::Select, "Select", "V"),
        (Tool::Direct, "Anchors", "A"),
        (Tool::Pen, "Pen", "P"),
        (Tool::Curvature, "Curve", "C"),
        (Tool::Pencil, "Pencil", "N"),
        (Tool::Rect, "Rect", "M"),
        (Tool::Ellipse, "Ellipse", "L"),
        (Tool::Polygon, "Polygon", ""),
        (Tool::Star, "Star", ""),
        (Tool::Line, "Line", "\\"),
        (Tool::Text, "Type", "T"),
        (Tool::Knife, "Knife", ""),
        (Tool::Eraser, "Eraser", "E"),
        (Tool::Blob, "Blob", "B"),
    ];
}

/// A pointer gesture in progress, in document coordinates.
#[derive(Clone, Debug)]
enum Gesture {
    Shape { from: Point, to: Point },
    Move { from: Point, to: Point },
    Free(Vec<Point>),
    Anchor { id: NodeId, sub: usize, index: usize, part: Part, to: Point },
    PenHandle { at: Point, to: Point },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Part {
    Anchor,
    In,
    Out,
}

/// What the window needs from the app (and tests give it without one).
pub trait Host {
    /// Where new documents go.
    fn vector_dir(&self) -> PathBuf;
    /// Puts the document on the timeline; returns the clip id.
    fn place(&mut self, path: &Path) -> Result<u32, String>;
}

pub struct Editor {
    pub open: bool,
    pub path: Option<PathBuf>,
    pub doc: Option<Document>,
    pub tool: Tool,
    pub selection: Vec<NodeId>,
    pub fill: Color32,
    pub stroke: Color32,
    pub stroke_width: f32,
    pub sides: u32,
    pub text: String,
    pub font_size: f32,
    pub brush: f32,
    pub status: String,
    pub file_field: String,
    undo: Vec<String>,
    redo: Vec<String>,
    gesture: Option<Gesture>,
    pen: Vec<(Point, Option<Point>)>,
    texture: Option<egui::TextureHandle>,
    rendered: Option<(u64, [usize; 2])>,
    rev: u64,
    /// The document outline for the layers list, by revision.
    outline: Option<(u64, Value)>,
}

impl Default for Editor {
    fn default() -> Editor {
        Editor {
            open: false,
            path: None,
            doc: None,
            tool: Tool::Select,
            selection: Vec::new(),
            fill: Color32::from_rgb(0xe8, 0x57, 0x3f),
            stroke: Color32::from_rgb(0x1a, 0x20, 0x2c),
            stroke_width: 0.0,
            sides: 6,
            text: "Type here".into(),
            font_size: 72.0,
            brush: 16.0,
            status: String::new(),
            file_field: String::new(),
            undo: Vec::new(),
            redo: Vec::new(),
            gesture: None,
            pen: Vec::new(),
            texture: None,
            rendered: None,
            rev: 0,
            outline: None,
        }
    }
}

thread_local! {
    static EDITOR: RefCell<Editor> = RefCell::new(Editor::default());
}

fn hex(c: Color32) -> String {
    if c.a() == 255 { format!("#{:02x}{:02x}{:02x}", c.r(), c.g(), c.b()) } else { format!("#{:02x}{:02x}{:02x}{:02x}", c.r(), c.g(), c.b(), c.a()) }
}

impl Editor {
    /// Opens (or makes) a document.
    pub fn load(&mut self, path: &Path) -> Result<(), String> {
        let doc = if path.exists() { studio_vector_art::interop::open(path)? } else { Document::new(1920.0, 1080.0) };
        let save = if path.extension().is_some_and(|e| e != "svart") { path.with_extension("svart") } else { path.to_path_buf() };
        self.doc = Some(doc);
        self.path = Some(save);
        self.selection.clear();
        self.undo.clear();
        self.redo.clear();
        self.pen.clear();
        self.rev += 1;
        self.open = true;
        self.save()
    }

    pub fn save(&mut self) -> Result<(), String> {
        let (Some(d), Some(p)) = (&self.doc, &self.path) else { return Ok(()) };
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        std::fs::write(p, d.to_json()).map_err(|e| format!("{}: {e}", p.display()))
    }

    fn context(&self) -> ops::Context {
        let dir = self.path.as_ref().and_then(|p| p.parent()).map(Path::to_path_buf);
        ops::Context { base_dir: dir.clone(), out_dir: dir }
    }

    /// Runs one op as one undo step and saves (the timeline clip redraws).
    pub fn run(&mut self, op: Value) -> Result<Value, String> {
        let ctx = self.context();
        let Some(doc) = self.doc.as_mut() else { return Err("no document open".into()) };
        let before = doc.to_json();
        match ops::apply(doc, &op, &ctx) {
            Ok(v) => {
                self.undo.push(before);
                if self.undo.len() > 100 {
                    self.undo.remove(0);
                }
                self.redo.clear();
                self.rev += 1;
                // Objects an op made become the selection.
                let made: Vec<NodeId> = v.get("ids").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_u64).collect()).or_else(|| v.get("id").and_then(Value::as_u64).map(|i| vec![i])).unwrap_or_default();
                let alive = |id: &NodeId| self.doc.as_ref().is_some_and(|d| d.find(*id).is_some());
                if !made.is_empty() && made.iter().all(alive) {
                    self.selection = made;
                }
                self.selection.retain(alive);
                self.status.clear();
                self.save()?;
                Ok(v)
            }
            Err(e) => {
                self.status = e.clone();
                Err(e)
            }
        }
    }

    pub fn undo(&mut self) {
        if let (Some(prev), Some(doc)) = (self.undo.pop(), self.doc.as_mut()) {
            self.redo.push(doc.to_json());
            if let Ok(d) = Document::from_json(&prev) {
                *doc = d;
            }
            self.rev += 1;
            let _ = self.save();
        }
    }

    pub fn redo(&mut self) {
        if let (Some(next), Some(doc)) = (self.redo.pop(), self.doc.as_mut()) {
            self.undo.push(doc.to_json());
            if let Ok(d) = Document::from_json(&next) {
                *doc = d;
            }
            self.rev += 1;
            let _ = self.save();
        }
    }

    fn paint_args(&self) -> Value {
        let mut v = json!({"fill": if self.fill.a() == 0 { json!("none") } else { json!(hex(self.fill)) }});
        if self.stroke_width > 0.0 && self.stroke.a() > 0 {
            v["stroke"] = json!(hex(self.stroke));
            v["stroke_width"] = json!(self.stroke_width);
        }
        v
    }

    fn with_paint(&self, mut op: Value) -> Value {
        if let (Some(o), Value::Object(p)) = (op.as_object_mut(), self.paint_args()) {
            for (k, v) in p {
                o.entry(k).or_insert(v);
            }
        }
        op
    }

    /// The op a finished gesture makes (or `None`).
    fn gesture_op(&self, g: &Gesture, shift: bool) -> Option<Value> {
        match g {
            Gesture::Shape { from, to } => {
                let (mut w, mut h) = (to.x - from.x, to.y - from.y);
                if shift {
                    let s = w.abs().max(h.abs());
                    w = s * w.signum();
                    h = s * h.signum();
                }
                let (x, y) = (from.x.min(from.x + w), from.y.min(from.y + h));
                let (w, h) = (w.abs(), h.abs());
                if w < 1.0 && h < 1.0 && self.tool != Tool::Line {
                    return None;
                }
                let c = (x + w / 2.0, y + h / 2.0);
                Some(match self.tool {
                    Tool::Rect => self.with_paint(json!({"op": "rect", "x": x, "y": y, "w": w, "h": h})),
                    Tool::Ellipse => self.with_paint(json!({"op": "ellipse", "cx": c.0, "cy": c.1, "rx": w / 2.0, "ry": h / 2.0})),
                    Tool::Polygon => self.with_paint(json!({"op": "polygon", "cx": c.0, "cy": c.1, "r": w.max(h) / 2.0, "sides": self.sides})),
                    Tool::Star => self.with_paint(json!({"op": "star", "cx": c.0, "cy": c.1, "r": w.max(h) / 2.0, "points": self.sides.max(3)})),
                    Tool::Line => {
                        let stroke = if self.stroke.a() > 0 { self.stroke } else { Color32::BLACK };
                        json!({"op": "line", "from": [from.x, from.y], "to": [from.x + w * (to.x - from.x).signum(), from.y + h * (to.y - from.y).signum()], "stroke": hex(stroke), "stroke_width": self.stroke_width.max(2.0)})
                    }
                    _ => return None,
                })
            }
            Gesture::Move { from, to } => {
                let d = *to - *from;
                (d.hypot() > 0.01 && !self.selection.is_empty()).then(|| json!({"op": "transform", "ids": self.selection, "translate": [d.x, d.y]}))
            }
            Gesture::Free(pts) => {
                if pts.len() < 2 {
                    return None;
                }
                let line: Vec<[f64; 2]> = pts.iter().map(|p| [p.x, p.y]).collect();
                Some(match self.tool {
                    Tool::Pencil => {
                        let mut op = json!({"op": "pencil", "points": line, "fidelity": 0.5, "fill": "none", "stroke": hex(if self.stroke.a() > 0 { self.stroke } else { Color32::BLACK }), "stroke_width": self.stroke_width.max(2.0)});
                        op["cap"] = json!("round");
                        op
                    }
                    Tool::Knife => json!({"op": "knife", "ids": self.targets(), "line": line}),
                    Tool::Eraser => json!({"op": "eraser", "ids": self.targets(), "line": line, "width": self.brush}),
                    Tool::Blob => json!({"op": "blob", "points": line, "width": self.brush, "color": hex(self.fill)}),
                    _ => return None,
                })
            }
            Gesture::Anchor { id, sub, index, part, to } => {
                let doc = self.doc.as_ref()?;
                let local = doc.world_transform(*id).unwrap_or(Affine::IDENTITY).inverse() * *to;
                let key = match part {
                    Part::Anchor => "at",
                    Part::In => "in",
                    Part::Out => "out",
                };
                let mut op = json!({"op": "anchor", "id": id, "subpath": sub, "index": index});
                op[key] = json!([local.x, local.y]);
                Some(op)
            }
            Gesture::PenHandle { .. } => None,
        }
    }

    /// The objects a cutting tool works on: the selection, or everything.
    fn targets(&self) -> Value {
        if self.selection.is_empty() { json!("all") } else { json!(self.selection) }
    }

    /// The pen's path so far as an op.
    fn pen_op(&self, closed: bool) -> Option<Value> {
        if self.pen.len() < 2 {
            return None;
        }
        let anchors: Vec<Value> = self.pen.iter().map(|(p, h)| match h {
            Some(o) => json!({"at": [p.x, p.y], "out": [o.x, o.y]}),
            None => json!({"at": [p.x, p.y]}),
        }).collect();
        let mut op = self.with_paint(json!({"op": "path", "anchors": anchors, "closed": closed}));
        if !closed {
            op["fill"] = json!("none");
            if op.get("stroke").is_none() {
                op["stroke"] = json!(hex(if self.stroke.a() > 0 { self.stroke } else { Color32::BLACK }));
                op["stroke_width"] = json!(self.stroke_width.max(2.0));
            }
        }
        Some(op)
    }

    /// Finishes the pen or curvature path in progress.
    pub fn finish_path(&mut self, closed: bool) {
        let op = match self.tool {
            Tool::Curvature if self.pen.len() >= 2 => {
                let pts: Vec<[f64; 2]> = self.pen.iter().map(|(p, _)| [p.x, p.y]).collect();
                let mut op = self.with_paint(json!({"op": "curve", "points": pts, "closed": closed}));
                if !closed {
                    op["fill"] = json!("none");
                    op["stroke"] = json!(hex(if self.stroke.a() > 0 { self.stroke } else { Color32::BLACK }));
                    op["stroke_width"] = json!(self.stroke_width.max(2.0));
                }
                Some(op)
            }
            _ => self.pen_op(closed),
        };
        self.pen.clear();
        if let Some(op) = op {
            let _ = self.run(op);
        }
    }
}

/// The map from document points to the canvas on screen (the artboard fitted, with a margin).
fn fit(canvas: Rect, art: studio_vector_art::raster::kurbo::Rect) -> Affine {
    let margin = 24.0;
    let (cw, ch) = ((canvas.width() as f64 - margin * 2.0).max(10.0), (canvas.height() as f64 - margin * 2.0).max(10.0));
    let s = (cw / art.width().max(1.0)).min(ch / art.height().max(1.0));
    let ox = canvas.left() as f64 + (canvas.width() as f64 - art.width() * s) / 2.0;
    let oy = canvas.top() as f64 + (canvas.height() as f64 - art.height() * s) / 2.0;
    Affine::translate((ox, oy)) * Affine::scale(s) * Affine::translate((-art.x0, -art.y0))
}

fn to_pos(m: Affine, p: Point) -> Pos2 {
    let q = m * p;
    Pos2::new(q.x as f32, q.y as f32)
}

fn to_point(m: Affine, p: Pos2) -> Point {
    m.inverse() * Point::new(p.x as f64, p.y as f64)
}

/// The topmost object under `p`: a layer's direct child (a group as one),
/// or with `deep` the innermost shape.
pub fn hit(doc: &Document, p: Point, deep: bool, tolerance: f64) -> Option<NodeId> {
    fn inside(doc: &Document, id: NodeId, p: Point, tol: f64) -> bool {
        let Some(b) = build::world_bounds(doc, id) else { return false };
        if !b.inflate(tol, tol).contains(p) {
            return false;
        }
        match studio_vector_art::edit::outline_of(doc, id) {
            Some((path, _)) => {
                let bp = path.to_bezpath();
                studio_vector_art::raster::kurbo::Shape::winding(&bp, p) != 0 || studio_vector_art::geom::hit::distance_to_outline(&bp, p) <= tol
            }
            None => true,
        }
    }
    fn walk(doc: &Document, nodes: &[studio_vector_art::Node], p: Point, deep: bool, tol: f64) -> Option<NodeId> {
        for n in nodes.iter().rev() {
            if !n.visible || n.locked {
                continue;
            }
            if let Some(kids) = n.children()
                && deep
                && matches!(n.kind, NodeKind::Group { .. })
            {
                if let Some(h) = walk(doc, kids, p, deep, tol) {
                    return Some(h);
                }
                continue;
            }
            if inside(doc, n.id, p, tol) {
                return Some(n.id);
            }
        }
        None
    }
    for l in doc.layers.iter().rev() {
        if !l.visible || l.locked {
            continue;
        }
        if let Some(kids) = l.children()
            && let Some(h) = walk(doc, kids, p, deep, tolerance)
        {
            return Some(h);
        }
    }
    None
}

/// Anchors and handles of a path in document space: (subpath, index, part, point).
fn handles(doc: &Document, id: NodeId) -> Vec<(usize, usize, Part, Point)> {
    let Some(n) = doc.find(id) else { return Vec::new() };
    let path = match &n.kind {
        NodeKind::Path { path, .. } => path.clone(),
        NodeKind::Shape { shape } => shape.to_path(),
        _ => return Vec::new(),
    };
    let m = doc.world_transform(id).unwrap_or(Affine::IDENTITY);
    let mut out = Vec::new();
    for (si, sp) in path.subpaths.iter().enumerate() {
        for (ai, a) in sp.anchors.iter().enumerate() {
            out.push((si, ai, Part::Anchor, m * a.p));
            if a.has_in() {
                out.push((si, ai, Part::In, m * a.h_in));
            }
            if a.has_out() {
                out.push((si, ai, Part::Out, m * a.h_out));
            }
        }
    }
    out
}

/// Draws the editor's contents into `ui` (the window body), with `host`
/// for what lives in the app.
pub fn body(ui: &mut egui::Ui, e: &mut Editor, host: &mut dyn Host) {
    claim_keys(ui);
    toolbar(ui, e, host);
    ui.separator();
    let avail = ui.available_size();
    ui.horizontal_top(|ui| {
        ui.allocate_ui(Vec2::new(210.0, avail.y), |ui| layers(ui, e));
        ui.separator();
        canvas(ui, e, Vec2::new((avail.x - 230.0).max(200.0), (avail.y - 6.0).max(200.0)));
    });
}

/// The drawing's keys are its own while the pointer is over it or a field in
/// it has focus: Delete, ⌘A and the letters never reach the timeline, and the
/// editor has its own undo. A text field takes every printable key.
fn claim_keys(ui: &egui::Ui) {
    use crate::keymap::{KeyOwner, claim};
    let ctx = ui.ctx();
    if ctx.egui_wants_keyboard_input() {
        claim(ctx, KeyOwner::Text);
    } else if ui.rect_contains_pointer(ui.max_rect()) {
        claim(ctx, KeyOwner::PanelUndo);
    }
}

/// The icon a tool wears in the strip; its name and key go in the tooltip.
fn tool_icon(t: Tool) -> Icon {
    match t {
        Tool::Select => Icon::Cursor,
        Tool::Direct => Icon::Selection,
        Tool::Pen => Icon::Path,
        Tool::Curvature => Icon::Waveform,
        Tool::Pencil => Icon::Eyedropper,
        Tool::Rect => Icon::Rectangle,
        Tool::Ellipse => Icon::Circle,
        Tool::Polygon => Icon::Diamond,
        Tool::Star => Icon::Star,
        Tool::Line => Icon::Minus,
        Tool::Text => Icon::TextT,
        Tool::Knife => Icon::Scissors,
        Tool::Eraser => Icon::XCircle,
        Tool::Blob => Icon::DropHalf,
    }
}

/// A button that opens a popover menu below it; `add` fills the menu. The
/// menu closes when `add` returns true.
fn menu_popover(ui: &mut egui::Ui, name: &str, add: impl FnOnce(&mut egui::Ui) -> bool) {
    let id = ui.scope_id().with(("vector-menu", name));
    let resp = kit::Button::secondary(name).trailing(Icon::CaretDown).compact().show(ui);
    if resp.clicked() {
        kit::toggle_open(ui.ctx(), id);
    }
    if kit::popover(ui.ctx(), id, resp.rect, kit::Side::Below, 200.0, add) == Some(true) {
        kit::set_open(ui.ctx(), id, false);
    }
}

fn toolbar(ui: &mut egui::Ui, e: &mut Editor, host: &mut dyn Host) {
    use crate::theme;
    let gap = theme::space::SECTION_GAP;
    ui.horizontal_wrapped(|ui| {
        // Groups: select, paths, shapes, type, erasing.
        for (i, (t, name, key)) in Tool::ALL.into_iter().enumerate() {
            if matches!(i, 2 | 5 | 10 | 11) {
                ui.add_space(gap);
            }
            let mut b = IconButton::new(tool_icon(t), name).active(e.tool == t).compact();
            if !key.is_empty() {
                b = b.shortcut(key);
            }
            if b.show(ui).clicked() {
                if e.tool != t {
                    e.pen.clear();
                }
                e.tool = t;
            }
        }
    });
    ui.horizontal_wrapped(|ui| {
        let t = theme::tokens(ui.ctx());
        let cap = |ui: &mut egui::Ui, s: &str| ui.label(egui::RichText::new(s).font(theme::caption()).color(t.text_2));
        cap(ui, "Fill");
        ui.color_edit_button_srgba(&mut e.fill);
        if kit::Button::ghost("No fill").compact().show(ui).clicked() {
            e.fill = Color32::TRANSPARENT;
        }
        ui.add_space(theme::space::GAP);
        cap(ui, "Stroke");
        ui.color_edit_button_srgba(&mut e.stroke);
        kit::Slider::new("Stroke width", &mut e.stroke_width, 0.0..=200.0).default(1.0).unit("pt").decimals(1).with_value().width(140.0).show(ui);
        if !e.selection.is_empty() && kit::Button::secondary("Apply to selection").compact().show(ui).clicked() {
            let mut op = e.paint_args();
            op["op"] = json!("style");
            op["ids"] = json!(e.selection);
            if e.stroke_width <= 0.0 {
                op["stroke"] = json!("none");
            }
            let _ = e.run(op);
        }
        ui.add_space(gap);
        match e.tool {
            Tool::Polygon | Tool::Star => {
                cap(ui, if e.tool == Tool::Star { "Points" } else { "Sides" });
                let mut sides = e.sides as i32;
                kit::slider_i32(ui, "Sides", &mut sides, 3..=64, Some(140.0));
                e.sides = sides as _;
            }
            Tool::Text => {
                kit::text_field(ui, &mut e.text, "Text", None, 160.0, None);
                kit::Slider::new("Font size", &mut e.font_size, 4.0..=1000.0).default(16.0).unit("pt").logarithmic(true).with_value().width(140.0).show(ui);
            }
            Tool::Eraser | Tool::Blob => {
                cap(ui, "Size");
                kit::Slider::new("Size", &mut e.brush, 1.0..=500.0).default(10.0).unit("px").logarithmic(true).with_value().width(140.0).show(ui);
            }
            Tool::Pen | Tool::Curvature => {
                if kit::Button::secondary("Finish").shortcut("Enter").compact().show(ui).clicked() {
                    e.finish_path(false);
                }
                if kit::Button::ghost("Close path").compact().show(ui).clicked() {
                    e.finish_path(true);
                }
            }
            _ => {}
        }
    });
    ui.horizontal_wrapped(|ui| {
        let sel = e.selection.clone();
        let many = sel.len() >= 2;
        let one = sel.len() == 1;
        let some = !sel.is_empty();
        let mut ops_to_run: Vec<Value> = Vec::new();
        menu_popover(ui, "Combine", |ui| {
            let mut hit = false;
            for (label, mode) in [("Unite", "unite"), ("Minus front", "minus_front"), ("Intersect", "intersect"), ("Exclude", "exclude"), ("Divide", "divide"), ("Trim", "trim"), ("Merge", "merge"), ("Crop", "crop")] {
                let on = many || (mode == "divide" && some);
                if kit::MenuItem::new(label).enabled(on).show(ui).clicked() {
                    ops_to_run.push(json!({"op": "pathfinder", "ids": sel, "mode": mode}));
                    hit = true;
                }
            }
            hit
        });
        menu_popover(ui, "Align", |ui| {
            let mut hit = false;
            kit::menu_heading(ui, if one { "Align to the artboard" } else { "Align selection" });
            for (label, how) in [("Left", "left"), ("Centre", "center"), ("Right", "right"), ("Top", "top"), ("Middle", "middle"), ("Bottom", "bottom")] {
                if kit::MenuItem::new(label).enabled(some).show(ui).clicked() {
                    let mut op = json!({"op": "align", "ids": sel, "how": how});
                    if one {
                        op["to"] = json!("artboard");
                    }
                    ops_to_run.push(op);
                    hit = true;
                }
            }
            kit::menu_separator(ui);
            if kit::MenuItem::new("Distribute horizontally").enabled(sel.len() >= 3).show(ui).clicked() {
                ops_to_run.push(json!({"op": "distribute", "ids": sel, "axis": "horizontal"}));
                hit = true;
            }
            hit
        });
        let mut clear = false;
        menu_popover(ui, "Arrange", |ui| {
            let mut hit = false;
            let mut item = |ui: &mut egui::Ui, label: &str, on: bool, op: Value| {
                if kit::MenuItem::new(label).enabled(on).show(ui).clicked() {
                    ops_to_run.push(op);
                    hit = true;
                }
            };
            if let Some(&first) = sel.first() {
                item(ui, "Bring to front", one, json!({"op": "arrange", "id": first, "how": "front"}));
                item(ui, "Send to back", one, json!({"op": "arrange", "id": first, "how": "back"}));
            } else {
                item(ui, "Bring to front", false, Value::Null);
                item(ui, "Send to back", false, Value::Null);
            }
            let id0 = sel.first().copied().unwrap_or(0);
            kit::menu_separator(ui);
            item(ui, "Group", many, json!({"op": "group", "ids": sel}));
            item(ui, "Ungroup", one, json!({"op": "ungroup", "id": id0}));
            item(ui, "Clip with the front shape", many, json!({"op": "clip", "ids": sel}));
            kit::menu_separator(ui);
            item(ui, "Outline type", one, json!({"op": "outline_text", "id": id0}));
            item(ui, "Outline stroke", one, json!({"op": "outline_stroke", "id": id0}));
            item(ui, "Expand", one, json!({"op": "expand", "id": id0}));
            item(ui, "Trace image into shapes", one, json!({"op": "trace", "id": id0, "preset": "6 colors"}));
            item(ui, "Add drop shadow", one, json!({"op": "effect", "ids": sel, "effect": {"type": "drop_shadow", "dx": 6, "dy": 8, "blur": 18, "color": "#00000059"}}));
            item(ui, "Repeat around a circle", one, json!({"op": "repeat", "ids": sel, "kind": "radial", "count": 8}));
            hit
        });
        ui.add_space(theme::space::GAP);
        ui.add_enabled_ui(some, |ui| {
            if IconButton::new(Icon::Copy, "Duplicate").compact().show(ui).clicked() {
                ops_to_run.push(json!({"op": "duplicate", "ids": sel}));
            }
            if IconButton::new(Icon::Trash, "Delete").compact().show(ui).clicked() {
                ops_to_run.push(json!({"op": "delete", "ids": sel}));
                clear = true;
            }
        });
        ui.add_space(gap);
        ui.add_enabled_ui(!e.undo.is_empty(), |ui| {
            if IconButton::new(Icon::ArrowUUpLeft, "Undo").compact().show(ui).clicked() {
                e.undo();
            }
        });
        ui.add_enabled_ui(!e.redo.is_empty(), |ui| {
            if IconButton::new(Icon::ArrowUUpRight, "Redo").compact().show(ui).clicked() {
                e.redo();
            }
        });
        for op in ops_to_run {
            if !op.is_null() {
                let _ = e.run(op);
            }
        }
        if clear {
            e.selection.clear();
        }
    });
    ui.horizontal_wrapped(|ui| {
        let t = theme::tokens(ui.ctx());
        kit::text_field(ui, &mut e.file_field, "Path to a drawing", None, 220.0, None);
        if kit::Button::secondary("Import").compact().show(ui).clicked() && !e.file_field.is_empty() {
            let f = e.file_field.clone();
            let _ = e.run(json!({"op": "import", "file": f}));
        }
        let mut export: Option<&str> = None;
        menu_popover(ui, "Export", |ui| {
            let mut hit = false;
            for (label, ext) in [("SVG", "svg"), ("PDF", "pdf"), ("EPS", "eps"), ("PNG", "png")] {
                if kit::MenuItem::new(label).show(ui).clicked() {
                    export = Some(ext);
                    hit = true;
                }
            }
            hit
        });
        if let Some(ext) = export {
            let file = if e.file_field.ends_with(&format!(".{ext}")) {
                e.file_field.clone()
            } else {
                e.path.as_ref().map(|p| p.with_extension(ext).display().to_string()).unwrap_or_else(|| format!("art.{ext}"))
            };
            match e.run(json!({"op": "export", "file": file, "scale": 4})) {
                Ok(v) => e.status = format!("Saved {}", v["file"].as_str().unwrap_or("")),
                Err(err) => e.status = err,
            }
        }
        if kit::Button::secondary("Place on timeline").compact().show(ui).clicked()
            && let Some(p) = e.path.clone()
        {
            e.status = match host.place(&p) {
                Ok(id) => format!("On the timeline as clip {id}"),
                Err(err) => err,
            };
        }
        if !e.status.is_empty() {
            ui.label(egui::RichText::new(&e.status).font(theme::caption()).color(t.text_2));
        }
    });
}

/// Every shape and text in the drawing, for ⌘A (a group is not picked: the
/// nodes inside it are).
fn shape_ids(doc: &Document) -> Vec<NodeId> {
    let mut ids = Vec::new();
    doc.walk(&mut |n| {
        if n.children().is_none() {
            ids.push(n.id);
        }
    });
    ids
}

fn layers(ui: &mut egui::Ui, e: &mut Editor) {
    use crate::{icons, theme};
    let Some(doc) = e.doc.as_ref() else { return };
    if e.outline.as_ref().is_none_or(|(r, _)| *r != e.rev) {
        e.outline = Some((e.rev, ops::summary(doc)));
    }
    let summary = e.outline.as_ref().map(|(_, s)| s.clone()).unwrap_or(Value::Null);
    let t = theme::tokens(ui.ctx());
    egui::ScrollArea::vertical().id_salt("vector-layers").show_v(ui, |ui| {
        for o in summary["objects"].as_array().into_iter().flatten() {
            let id = o["id"].as_u64().unwrap_or(0);
            let depth = o["depth"].as_u64().unwrap_or(0) as f32;
            let kind = o["type"].as_str().unwrap_or("");
            let name = o["name"].as_str().map(String::from).or_else(|| o["text"].as_str().map(|t| format!("\u{201c}{t}\u{201d}"))).unwrap_or_else(|| o["shape"].as_str().unwrap_or(kind).to_string());
            let icon = match kind {
                "layer" => Icon::Stack,
                "group" => Icon::Folder,
                "text" => Icon::TextT,
                "image" => Icon::Image,
                "path" => Icon::Path,
                _ => Icon::Shapes,
            };
            let selected = e.selection.contains(&id);
            let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), theme::space::ROW_H), Sense::click());
            let p = ui.painter();
            if selected {
                p.rect_filled(rect, theme::radius::CONTROL, t.select);
            } else if resp.hovered() {
                p.rect_filled(rect, theme::radius::CONTROL, t.alt);
            }
            let x = rect.left() + 6.0 + depth * 12.0; // TODO(kit): indent step
            icons::paint(p, Rect::from_center_size(egui::pos2(x + 7.0, rect.center().y), Vec2::splat(14.0)), icon, 14.0, if selected { t.text } else { t.text_2 });
            // A long name ends in an ellipsis; the hover shows it whole.
            let font = if kind == "layer" { theme::body_medium() } else { theme::body() };
            let cut = kit::paint_text_clipped(p, egui::pos2(x + 22.0, rect.center().y), egui::Align2::LEFT_CENTER, &name, font, t.text, (rect.right() - x - 28.0).max(0.0));
            let resp = if cut { resp.on_hover_text(format!("{name} (#{id})")) } else { resp.on_hover_text(format!("#{id}")) };
            if resp.clicked() && kind != "layer" {
                if ui.input(|i| i.modifiers.shift) {
                    if selected {
                        e.selection.retain(|s| *s != id);
                    } else {
                        e.selection.push(id);
                    }
                } else {
                    e.selection = vec![id];
                }
            }
        }
    });
}

fn canvas(ui: &mut egui::Ui, e: &mut Editor, size: Vec2) {
    let (resp, painter) = ui.allocate_painter(size, Sense::click_and_drag());
    let rect = resp.rect;
    painter.rect_filled(rect, 0.0, Color32::from_gray(48));
    let Some(doc) = e.doc.clone() else {
        painter.text(rect.center(), egui::Align2::CENTER_CENTER, "Open or make a drawing", egui::FontId::proportional(16.0), Color32::GRAY);
        return;
    };
    let Some(board) = doc.artboards.first().cloned() else { return };
    let m = fit(rect, board.rect);
    // The art, drawn by the vector renderer at the canvas's own pixels.
    let ppp = ui.ctx().pixels_per_point();
    let tl = to_pos(m, Point::new(board.rect.x0, board.rect.y0));
    let br = to_pos(m, Point::new(board.rect.x1, board.rect.y1));
    let art_rect = Rect::from_min_max(tl, br);
    let px = [(art_rect.width() * ppp).round().max(1.0) as usize, (art_rect.height() * ppp).round().max(1.0) as usize];
    if e.rendered != Some((e.rev, px)) || e.texture.is_none() {
        let pm = build::render_artboard(&doc, None, px[0] as u32, px[1] as u32, &build::BuildOptions { base_dir: e.path.as_ref().and_then(|p| p.parent()).map(Path::to_path_buf), show_hidden: false });
        let img = egui::ColorImage::from_rgba_premultiplied(px, &pm.data);
        match &mut e.texture {
            Some(t) => t.set(img, egui::TextureOptions::LINEAR),
            None => e.texture = Some(ui.ctx().load_texture("vector-art-canvas", img, egui::TextureOptions::LINEAR)),
        }
        e.rendered = Some((e.rev, px));
    }
    // A checkerboard shows where the artboard is see-through.
    if board.background.is_none_or(|c| c.a < 1.0) {
        let cell = 10.0;
        let mut y = art_rect.top();
        let mut row = 0;
        while y < art_rect.bottom() {
            let mut x = art_rect.left() + if row % 2 == 0 { 0.0 } else { cell };
            while x < art_rect.right() {
                painter.rect_filled(Rect::from_min_size(Pos2::new(x, y), Vec2::splat(cell)).intersect(art_rect), 0.0, Color32::from_gray(205));
                x += cell * 2.0;
            }
            y += cell;
            row += 1;
        }
    }
    if let Some(t) = &e.texture {
        painter.image(t.id(), art_rect, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
    }
    painter.rect_stroke(art_rect, 0.0, Stroke::new(1.0, Color32::from_gray(90)), StrokeKind::Outside);
    let tol = 6.0 / m.as_coeffs()[0].abs().max(1e-6);
    let pointer = resp.interact_pointer_pos().or_else(|| resp.hover_pos());
    let at = pointer.map(|p| to_point(m, p));
    let shift = ui.input(|i| i.modifiers.shift);
    // Keys.
    // A text field (the Type tool's text, the file name) keeps its own keys.
    let typing = ui.ctx().egui_wants_keyboard_input() && !ui.memory(|mm| mm.has_focus(resp.id));
    if !typing && (resp.hovered() || ui.memory(|mm| mm.has_focus(resp.id))) {
        let select_all = ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::A));
        if select_all {
            e.selection = shape_ids(&doc);
        }
        let (enter, esc, back, del, undo, redo) = ui.input(|i| {
            (i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape), i.key_pressed(egui::Key::Backspace), i.key_pressed(egui::Key::Delete), i.modifiers.command && i.key_pressed(egui::Key::Z) && !i.modifiers.shift, i.modifiers.command && i.key_pressed(egui::Key::Z) && i.modifiers.shift)
        });
        if enter {
            e.finish_path(false);
        }
        if esc {
            e.pen.clear();
            e.gesture = None;
            e.selection.clear();
        }
        if back && !e.pen.is_empty() {
            e.pen.pop();
        } else if (back || del) && !e.selection.is_empty() && e.pen.is_empty() {
            let sel = e.selection.clone();
            let _ = e.run(json!({"op": "delete", "ids": sel}));
            e.selection.clear();
        }
        if undo {
            e.undo();
        }
        if redo {
            e.redo();
        }
    }
    // Pointer.
    if resp.drag_started()
        && let Some(p) = at
    {
        e.gesture = match e.tool {
            Tool::Select => {
                let h = hit(&doc, p, false, tol);
                match h {
                    Some(id) => {
                        if !e.selection.contains(&id) {
                            e.selection = if shift { e.selection.iter().copied().chain([id]).collect() } else { vec![id] };
                        }
                        Some(Gesture::Move { from: p, to: p })
                    }
                    None => {
                        e.selection.clear();
                        None
                    }
                }
            }
            Tool::Direct => {
                let near = e.selection.iter().flat_map(|id| handles(&doc, *id).into_iter().map(move |h| (*id, h))).find(|(_, (_, _, _, q))| q.distance(p) <= tol * 1.5);
                match near {
                    Some((id, (sub, index, part, _))) => Some(Gesture::Anchor { id, sub, index, part, to: p }),
                    None => {
                        e.selection = hit(&doc, p, true, tol).into_iter().collect();
                        e.selection.first().map(|_| Gesture::Move { from: p, to: p })
                    }
                }
            }
            Tool::Rect | Tool::Ellipse | Tool::Polygon | Tool::Star | Tool::Line => Some(Gesture::Shape { from: p, to: p }),
            Tool::Pencil | Tool::Knife | Tool::Eraser | Tool::Blob => Some(Gesture::Free(vec![p])),
            Tool::Pen => Some(Gesture::PenHandle { at: p, to: p }),
            _ => None,
        };
    }
    if resp.dragged()
        && let (Some(p), Some(g)) = (at, e.gesture.as_mut())
    {
        match g {
            Gesture::Shape { to, .. } | Gesture::Move { to, .. } | Gesture::Anchor { to, .. } | Gesture::PenHandle { to, .. } => *to = p,
            Gesture::Free(pts) => {
                if pts.last().is_none_or(|q| q.distance(p) > tol * 0.3) {
                    pts.push(p);
                }
            }
        }
    }
    if resp.drag_stopped()
        && let Some(g) = e.gesture.take()
    {
        if let Gesture::PenHandle { at, to } = g {
            // A pen drag sets the anchor's handle.
            let close = e.pen.len() > 2 && e.pen[0].0.distance(at) <= tol * 1.5;
            if close {
                e.finish_path(true);
            } else {
                e.pen.push((at, (to.distance(at) > tol * 0.5).then_some(to)));
            }
        } else if let Some(op) = e.gesture_op(&g, shift) {
            let _ = e.run(op);
        }
    }
    if resp.clicked()
        && let Some(p) = at
    {
        match e.tool {
            Tool::Select | Tool::Direct => {
                let h = hit(&doc, p, e.tool == Tool::Direct, tol);
                match (h, shift) {
                    (Some(id), true) => {
                        if e.selection.contains(&id) {
                            e.selection.retain(|s| *s != id);
                        } else {
                            e.selection.push(id);
                        }
                    }
                    (Some(id), false) => e.selection = vec![id],
                    (None, _) => e.selection.clear(),
                }
            }
            Tool::Pen | Tool::Curvature => {
                if e.pen.len() > 2 && e.pen[0].0.distance(p) <= tol * 1.5 {
                    e.finish_path(true);
                } else {
                    e.pen.push((p, None));
                }
            }
            Tool::Text => {
                let t = e.text.clone();
                let _ = e.run(json!({"op": "text", "x": p.x, "y": p.y, "text": t, "size": e.font_size, "fill": hex(if e.fill.a() > 0 { e.fill } else { Color32::BLACK })}));
            }
            _ => {}
        }
    }
    // Overlays: selection boxes, anchors, gestures in progress.
    let accent = Color32::from_rgb(0x3b, 0x82, 0xf6);
    if let Some(d) = &e.doc {
        for id in &e.selection {
            if let Some(b) = build::world_bounds(d, *id) {
                let r = Rect::from_min_max(to_pos(m, Point::new(b.x0, b.y0)), to_pos(m, Point::new(b.x1, b.y1)));
                let off = match &e.gesture {
                    Some(Gesture::Move { from, to }) => {
                        let v = to_pos(m, *to) - to_pos(m, *from);
                        Vec2::new(v.x, v.y)
                    }
                    _ => Vec2::ZERO,
                };
                painter.rect_stroke(r.translate(off), 0.0, Stroke::new(1.0, accent), StrokeKind::Outside);
            }
            if e.tool == Tool::Direct {
                for (_, _, part, q) in handles(d, *id) {
                    let c = to_pos(m, q);
                    if part == Part::Anchor {
                        painter.rect_filled(Rect::from_center_size(c, Vec2::splat(6.0)), 0.0, Color32::WHITE);
                        painter.rect_stroke(Rect::from_center_size(c, Vec2::splat(6.0)), 0.0, Stroke::new(1.0, accent), StrokeKind::Middle);
                    } else {
                        painter.circle_filled(c, 3.0, accent);
                    }
                }
            }
        }
    }
    match &e.gesture {
        Some(Gesture::Shape { from, to }) => {
            let r = Rect::from_two_pos(to_pos(m, *from), to_pos(m, *to));
            if e.tool == Tool::Line {
                painter.line_segment([to_pos(m, *from), to_pos(m, *to)], Stroke::new(1.5, accent));
            } else if e.tool == Tool::Ellipse {
                painter.add(egui::Shape::ellipse_stroke(r.center(), r.size() / 2.0, Stroke::new(1.5, accent)));
            } else {
                painter.rect_stroke(r, 0.0, Stroke::new(1.5, accent), StrokeKind::Middle);
            }
        }
        Some(Gesture::Free(pts)) => {
            let line: Vec<Pos2> = pts.iter().map(|p| to_pos(m, *p)).collect();
            painter.add(egui::Shape::line(line, Stroke::new(if matches!(e.tool, Tool::Eraser | Tool::Blob) { (e.brush as f64 * m.as_coeffs()[0]) as f32 } else { 1.5 }, accent.gamma_multiply(0.6))));
        }
        Some(Gesture::Anchor { to, .. }) => {
            painter.circle_stroke(to_pos(m, *to), 5.0, Stroke::new(1.5, accent));
        }
        _ => {}
    }
    if !e.pen.is_empty() {
        let mut pts: Vec<Pos2> = e.pen.iter().map(|(p, _)| to_pos(m, *p)).collect();
        if let Some(p) = pointer {
            pts.push(p);
        }
        painter.add(egui::Shape::line(pts, Stroke::new(1.5, accent)));
        for (p, h) in &e.pen {
            painter.rect_filled(Rect::from_center_size(to_pos(m, *p), Vec2::splat(6.0)), 0.0, accent);
            if let Some(h) = h {
                painter.line_segment([to_pos(m, *p), to_pos(m, *h)], Stroke::new(1.0, accent));
                painter.circle_filled(to_pos(m, *h), 3.0, accent);
            }
        }
    }
    if let Some(p) = at
        && resp.hovered()
    {
        painter.text(rect.left_bottom() + Vec2::new(8.0, -8.0), egui::Align2::LEFT_BOTTOM, format!("{:.0}, {:.0}", p.x, p.y), egui::FontId::monospace(11.0), Color32::from_gray(170));
    }
}

// ------------------------------------------------------------------ the app

struct AppHost<'a> {
    app: &'a mut StudioApp,
}

impl Host for AppHost<'_> {
    fn vector_dir(&self) -> PathBuf {
        vector_dir(self.app)
    }

    fn place(&mut self, path: &Path) -> Result<u32, String> {
        let at = self.app.editor.playhead as f64 / 1e6;
        let doc = std::mem::replace(&mut self.app.doc, crate::agent::Session::default().doc);
        let mut s = crate::agent::Session::new(doc);
        let r = s.call("vector", &json!({"doc": path.display().to_string(), "place": {"at": at}}));
        self.app.doc = s.doc;
        let content = r?;
        let text = content.iter().find_map(|c| match c {
            crate::agent::Content::Text(t) => Some(t.clone()),
            _ => None,
        });
        let v: Value = text.and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null);
        let id = v["clip"].as_u64().ok_or("the art was not placed")? as u32;
        self.app.editor.selection = vec![id];
        Ok(id)
    }
}

fn vector_dir(app: &StudioApp) -> PathBuf {
    match &app.doc.folder {
        Some(f) => studio_model::file::ProjectFolder::new(f.root()).media_of(studio_model::file::MediaKind::Vector),
        None => std::env::temp_dir().join("studio-vector"),
    }
}

/// The vector art clip selected on the timeline, and its document.
fn selected_art(app: &StudioApp) -> Option<PathBuf> {
    let id = *app.editor.selection.first()?;
    app.doc.model.sequences.iter().find_map(|q| q.clip(studio_model::ClipId(id as u64))).and_then(|c| match &c.source {
        studio_model::ClipSource::Generator(g) => match g.params.get("art") {
            Some(studio_model::Value::Text(t)) if !t.trim_start().starts_with('{') => Some(PathBuf::from(t)),
            _ => None,
        },
        _ => None,
    })
}

/// The vector part of the viewer's Add menu: a new drawing, or one to edit.
/// `q` is the menu's search; `close` closes the menu after a pick.
pub fn add_items(app: &mut StudioApp, ui: &mut egui::Ui, q: &str, close: egui::Id) {
    use super::viewer::{add_item, add_shows};
    let ctx = ui.ctx().clone();
    let dir = vector_dir(app);
    let selected = selected_art(app);
    let mut pick: Option<PathBuf> = None;
    let mut docs: Vec<PathBuf> = std::fs::read_dir(&dir).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "svart")).collect();
    docs.sort();
    let edit = selected.as_ref().map(|p| format!("Edit {}", p.file_stem().and_then(|s| s.to_str()).unwrap_or("art")));
    let names: Vec<String> = docs.iter().take(30).map(|d| d.file_stem().and_then(|s| s.to_str()).unwrap_or("?").to_owned()).collect();
    let all = edit.iter().map(String::as_str).chain(["New drawing"]).chain(names.iter().map(String::as_str));
    if !add_shows(q, "Vector drawing", all) {
        return;
    }
    kit::menu_heading(ui, "Vector drawing");
    if let (Some(p), Some(label)) = (&selected, &edit)
        && add_item(ui, q, "Vector drawing", label)
    {
        pick = Some(p.clone());
    }
    if add_item(ui, q, "Vector drawing", "New drawing") {
        let mut n = 1;
        while dir.join(format!("Drawing {n}.svart")).exists() {
            n += 1;
        }
        pick = Some(dir.join(format!("Drawing {n}.svart")));
    }
    for (d, name) in docs.into_iter().zip(&names) {
        if add_item(ui, q, "Vector drawing", name) {
            pick = Some(d);
        }
    }
    if let Some(p) = pick {
        kit::set_open(&ctx, close, false);
        EDITOR.with(|e| {
            let mut e = e.borrow_mut();
            if let Err(err) = e.load(&p) {
                kit::push(&ctx, err, None, Some(Icon::Warning));
            } else if !p.exists() {
                let _ = e.save();
            }
        });
    }
}

/// Draws the window (every frame, from the viewer).
pub fn show(app: &mut StudioApp, ctx: &egui::Context) {
    EDITOR.with(|cell| {
        let mut e = cell.borrow_mut();
        if !e.open {
            return;
        }
        let mut open = true;
        let title = e.path.as_ref().and_then(|p| p.file_stem()).and_then(|s| s.to_str()).map(|s| format!("Vector art: {s}")).unwrap_or_else(|| "Vector art".into());
        let mut host = AppHost { app };
        // No bigger than the window it opens in.
        let screen = ctx.content_rect();
        let size = [1100.0f32.min(screen.width() - 2.0 * kit::EDGE), 760.0f32.min(screen.height() - 2.0 * kit::EDGE - 40.0)];
        egui::Window::new(title).id(egui::Id::new("vector-art-window")).open(&mut open).default_size(size).resizable(true).show(ctx, |ui| body(ui, &mut e, &mut host));
        e.open = open;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NoHost(PathBuf);

    impl Host for NoHost {
        fn vector_dir(&self) -> PathBuf {
            self.0.clone()
        }
        fn place(&mut self, _: &Path) -> Result<u32, String> {
            Err("no timeline in tests".into())
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("studio-vector-ui-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).ok();
        d
    }

    #[test]
    fn tools_make_art_through_ops() {
        let dir = scratch("tools");
        let mut e = Editor::default();
        e.load(&dir.join("t.svart")).unwrap();
        e.tool = Tool::Rect;
        let op = e.gesture_op(&Gesture::Shape { from: Point::new(100.0, 100.0), to: Point::new(300.0, 200.0) }, false).unwrap();
        e.run(op).unwrap();
        let rect = e.selection[0];
        assert_eq!(build::world_bounds(e.doc.as_ref().unwrap(), rect).map(|b| (b.width(), b.height())), Some((200.0, 100.0)));
        // The pen: three clicks and a close.
        e.tool = Tool::Pen;
        e.pen = vec![(Point::new(500.0, 500.0), None), (Point::new(700.0, 500.0), Some(Point::new(750.0, 550.0))), (Point::new(600.0, 700.0), None)];
        e.finish_path(true);
        let path = e.selection[0];
        assert!(matches!(e.doc.as_ref().unwrap().find(path).map(|n| &n.kind), Some(NodeKind::Path { .. })));
        // Select by clicking inside the rectangle, then move it.
        let d = e.doc.clone().unwrap();
        assert_eq!(hit(&d, Point::new(150.0, 150.0), false, 1.0), Some(rect));
        assert_eq!(hit(&d, Point::new(1000.0, 1000.0), false, 1.0), None);
        e.selection = vec![rect];
        e.tool = Tool::Select;
        let op = e.gesture_op(&Gesture::Move { from: Point::new(150.0, 150.0), to: Point::new(170.0, 160.0) }, false).unwrap();
        e.run(op).unwrap();
        assert_eq!(build::world_bounds(e.doc.as_ref().unwrap(), rect).map(|b| (b.x0, b.y0)), Some((120.0, 110.0)));
        // Unite the two, then undo and redo.
        e.selection = vec![rect, path];
        e.run(json!({"op": "pathfinder", "ids": [rect, path], "mode": "unite"})).unwrap();
        assert_eq!(e.selection.len(), 1);
        e.undo();
        assert!(e.doc.as_ref().unwrap().find(rect).is_some());
        e.redo();
        assert!(e.doc.as_ref().unwrap().find(rect).is_none());
        // The document on disk follows every change.
        let saved = Document::from_json(&std::fs::read_to_string(dir.join("t.svart")).unwrap()).unwrap();
        assert_eq!(saved, *e.doc.as_ref().unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_window_draws_and_takes_a_drag_headless() {
        let dir = scratch("ui");
        let mut e = Editor::default();
        e.load(&dir.join("ui.svart")).unwrap();
        e.tool = Tool::Ellipse;
        let ctx = egui::Context::default();
        let mut host = NoHost(dir.clone());
        let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(1280.0, 900.0));
        let frame = |ctx: &egui::Context, e: &mut Editor, host: &mut NoHost, events: Vec<egui::Event>| {
            let input = egui::RawInput { screen_rect: Some(screen), events, ..Default::default() };
            ctx.run_ui(input, |ui| body(ui, e, host)).drop_without_applying_deltas();
        };
        frame(&ctx, &mut e, &mut host, Vec::new());
        assert!(e.texture.is_some(), "the canvas rendered the artboard");
        // Drag across the middle of the canvas.
        let (a, b) = (Pos2::new(700.0, 450.0), Pos2::new(900.0, 600.0));
        let press = |p: Pos2, down: bool| egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: down, modifiers: Default::default() };
        frame(&ctx, &mut e, &mut host, vec![egui::Event::PointerMoved(a), press(a, true)]);
        for k in 1..=10 {
            let p = a + (b - a) * (k as f32 / 10.0);
            frame(&ctx, &mut e, &mut host, vec![egui::Event::PointerMoved(p)]);
        }
        frame(&ctx, &mut e, &mut host, vec![press(b, false)]);
        frame(&ctx, &mut e, &mut host, Vec::new());
        let d = e.doc.as_ref().unwrap();
        let mut shapes = 0;
        d.walk(&mut |n| {
            if matches!(n.kind, NodeKind::Shape { .. }) {
                shapes += 1;
            }
        });
        assert_eq!(shapes, 1, "the drag drew an ellipse: {}", e.status);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_drawing_keeps_its_keys_and_select_all_picks_shapes() {
        use crate::keymap::{KeyOwner, owner};
        let dir = scratch("keys");
        let mut e = Editor::default();
        e.load(&dir.join("k.svart")).unwrap();
        e.tool = Tool::Rect;
        for (a, b) in [((100.0, 100.0), (300.0, 200.0)), ((400.0, 400.0), (500.0, 450.0))] {
            let op = e.gesture_op(&Gesture::Shape { from: Point::new(a.0, a.1), to: Point::new(b.0, b.1) }, false).unwrap();
            e.run(op).unwrap();
        }
        e.selection.clear();
        e.tool = Tool::Select;
        let ctx = egui::Context::default();
        let mut host = NoHost(dir.clone());
        let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(1280.0, 900.0));
        let frame = |ctx: &egui::Context, e: &mut Editor, host: &mut NoHost, events: Vec<egui::Event>| {
            let input = egui::RawInput { screen_rect: Some(screen), events, ..Default::default() };
            ctx.run_ui(input, |ui| body(ui, e, host)).drop_without_applying_deltas();
        };
        // Over the canvas the drawing owns the keyboard, undo included.
        let over = egui::Event::PointerMoved(Pos2::new(800.0, 450.0));
        frame(&ctx, &mut e, &mut host, vec![over.clone()]);
        frame(&ctx, &mut e, &mut host, vec![over.clone()]);
        assert_eq!(owner(&ctx), Some(KeyOwner::PanelUndo));
        // ⌘A picks the shapes, and Delete removes them, in the drawing.
        let key = |k| egui::Event::Key { key: k, physical_key: Some(k), pressed: true, repeat: false, modifiers: egui::Modifiers::COMMAND };
        frame(&ctx, &mut e, &mut host, vec![key(egui::Key::A)]);
        assert_eq!(e.selection.len(), 2, "select all picks both shapes");
        let del = egui::Event::Key { key: egui::Key::Delete, physical_key: Some(egui::Key::Delete), pressed: true, repeat: false, modifiers: Default::default() };
        frame(&ctx, &mut e, &mut host, vec![del]);
        assert!(e.selection.is_empty());
        let mut left = 0;
        e.doc.as_ref().unwrap().walk(&mut |n| left += usize::from(matches!(n.kind, NodeKind::Shape { .. })));
        assert_eq!(left, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
