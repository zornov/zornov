// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Modelling by hand in the 3D workspace: a model clip opened in edit
//! mode shows its polygon cage — vertices, edges and faces to pick in
//! vertex, edge or face mode — and every operation of
//! [`studio_modeling::tools`] works on the pick: from the menus, their
//! shortcuts, the operation search (F3), the right-click menu, the
//! transform keys (G R S with X Y Z and typed numbers) and the gizmo.
//! Sculpt mode shapes the same model with brushes.
//!
//! The model's editable source (polygons, modifier stack, materials) is
//! kept as a `.smodel` beside the `.smesh` the renderer draws (see
//! [`crate::mesh_store`]). Each finished operation writes both and points
//! the clip at them as one undo step; undo and redo point it back, and
//! the open session follows. While a transform is under way the model is
//! previewed from memory inside one batch, so a whole drag is one step.

pub mod draw;
pub mod input;
pub mod lattice;
pub mod menus;
pub mod panel;
pub mod sculpt3d;
#[cfg(test)]
mod tests;

use crate::app::StudioApp;
use crate::icons::Icon;
use crate::kit;
use eframe::egui;
use serde_json::{Map, Value};
use std::cell::RefCell;
use std::sync::Arc;
use studio_mesh::{Mesh, V3};
use studio_model as m;
use studio_modeling::asset::{Asset, Part};
use studio_modeling::edit::Falloff;
use studio_modeling::poly::Poly;
use studio_modeling::selection::{Mode, Selection};
use studio_modeling::tools::{self, Ctx};

/// What the workspace does with the selected model.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Stage {
    /// Whole objects (the workspace's own picking and gizmo).
    #[default]
    Object,
    /// Vertices, edges and faces.
    Edit,
    /// Brushes.
    Sculpt,
}

impl Stage {
    pub const ALL: [Stage; 3] = [Stage::Object, Stage::Edit, Stage::Sculpt];

    pub fn label(self) -> &'static str {
        match self {
            Stage::Object => "Object mode",
            Stage::Edit => "Edit mode",
            Stage::Sculpt => "Sculpt mode",
        }
    }
}

/// What a left drag on empty space does in edit mode.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SelectTool {
    /// A rectangle.
    #[default]
    Box,
    /// A free outline.
    Lasso,
    /// A round brush painted over what to pick.
    Paint,
}

impl SelectTool {
    pub const ALL: [SelectTool; 3] = [SelectTool::Box, SelectTool::Lasso, SelectTool::Paint];

    pub fn label(self) -> &'static str {
        match self {
            SelectTool::Box => "Box select",
            SelectTool::Lasso => "Lasso select",
            SelectTool::Paint => "Paint select",
        }
    }
}

/// Where moves snap to (when snapping is on, or Cmd is held).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SnapTo {
    /// Steps of the workspace's grid.
    #[default]
    Grid,
    Vertex,
    Edge,
    Face,
}

impl SnapTo {
    pub const ALL: [SnapTo; 4] = [SnapTo::Grid, SnapTo::Vertex, SnapTo::Edge, SnapTo::Face];

    pub fn label(self) -> &'static str {
        match self {
            SnapTo::Grid => "Grid",
            SnapTo::Vertex => "Vertex",
            SnapTo::Edge => "Edge",
            SnapTo::Face => "Face",
        }
    }
}

/// Proportional editing: nearby unpicked vertices follow a move.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Proportional {
    pub on: bool,
    /// Model pixels.
    pub radius: f32,
    pub falloff: Falloff,
    /// Measure along the surface (separate pieces stay put).
    pub connected: bool,
}

impl Default for Proportional {
    fn default() -> Proportional {
        Proportional { on: false, radius: 0.0, falloff: Falloff::Smooth, connected: false }
    }
}

/// What edit mode draws over the model.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Show {
    pub face_normals: bool,
    pub vertex_normals: bool,
    /// Faces tinted by which way they face (blue toward the eye, red away).
    pub face_orientation: bool,
    pub seams: bool,
    pub sharp: bool,
    pub creases: bool,
    /// Counts in the corner.
    pub stats: bool,
}

impl Default for Show {
    fn default() -> Show {
        Show { face_normals: false, vertex_normals: false, face_orientation: false, seams: true, sharp: true, creases: true, stats: true }
    }
}

/// The last operation, kept so its settings can be changed afterwards
/// (the mesh goes back to how it was and the operation runs again).
#[derive(Clone, Debug)]
pub struct Last {
    pub id: &'static str,
    pub args: Map<String, Value>,
    pub ctx: Ctx,
    pub before: (Poly, Selection),
    /// The undo step it made (so a re-run replaces it).
    pub step: Option<String>,
    pub said: String,
}

/// The cage seen from a view: computed once per change and view.
#[derive(Clone, Debug, Default)]
pub struct Cage {
    pub key: (u64, [i64; 6]),
    /// Each vertex can be seen (not behind the surface).
    pub seen: Vec<bool>,
}

/// An open model.
#[derive(Clone, Debug)]
pub struct Session {
    pub clip: u64,
    pub part: usize,
    pub asset: Asset,
    /// The clip's source and model files as last written or read.
    pub source: Option<String>,
    pub file: String,
    pub sel: Selection,
    pub stage: Stage,
    pub tool: SelectTool,
    pub prop: Proportional,
    pub snap_to: SnapTo,
    /// Symmetric editing across the model's x, y, z.
    pub mirror: [bool; 3],
    pub show: Show,
    pub last: Option<Last>,
    pub modal: Option<input::Modal>,
    /// Bumped whenever the mesh changes.
    pub version: u64,
    pub cage: Cage,
    /// The rig the model had (carried onto the new surface on each change).
    pub rig: Option<(Arc<studio_mesh::rig::Rig>, Arc<Mesh>)>,
    /// Paint-select brush radius (screen points).
    pub brush: f32,
    /// A local view: only this model shows (the hidden set before it).
    pub local: Option<std::collections::BTreeSet<u64>>,
    /// A lattice modifier's cage being edited.
    pub lattice: Option<lattice::LatticeEdit>,
    previews: u32,
}

impl Session {
    pub fn poly(&self) -> &Poly {
        &self.asset.parts[self.part].mesh
    }

    pub fn poly_mut(&mut self) -> &mut Poly {
        &mut self.asset.parts[self.part].mesh
    }

    pub fn part(&self) -> &Part {
        &self.asset.parts[self.part]
    }

    /// The operations' context for the current pick.
    pub fn ctx(&self, cursor: V3, view: V3) -> Ctx {
        let mut c = Ctx::for_poly(self.poly());
        c.cursor = cursor;
        c.view = view;
        c
    }

    pub fn changed(&mut self) {
        self.version += 1;
        self.cage.key.0 = u64::MAX;
    }
}

thread_local! {
    static SESSION: RefCell<Option<Session>> = const { RefCell::new(None) };
}

/// The open session, if a model is in edit or sculpt mode.
pub fn with<R>(f: impl FnOnce(&mut Option<Session>) -> R) -> R {
    SESSION.with(|s| f(&mut s.borrow_mut()))
}

pub fn stage() -> Stage {
    with(|s| s.as_ref().map_or(Stage::Object, |s| s.stage))
}

pub fn open_clip() -> Option<u64> {
    with(|s| s.as_ref().map(|s| s.clip))
}

/// The model clip edit mode would open: the only selected clip, when it
/// is a model.
pub fn candidate(app: &StudioApp) -> Option<u64> {
    match app.editor.selection.as_slice() {
        [id] => app.doc.model.active().clip(m::ClipId(*id as u64)).filter(|c| crate::mesh_store::file_of(c).is_some() && super::viewport::world::world_file_of(c).is_none()).map(|c| c.id.0),
        _ => None,
    }
}

/// A model's editable source: its `.smodel`, or (for a model that is
/// triangles only) one made from its triangles, joined back into quads
/// where they lie flat enough together.
pub fn load_source(mc: &m::Clip) -> Result<(Asset, Arc<Mesh>), String> {
    let file = crate::mesh_store::file_of(mc).ok_or("not a model")?;
    let mesh = studio_render::mesh_render::load(&file).ok_or_else(|| format!("{}: the model file can't be read", mc.name))?;
    if let Some(src) = crate::mesh_store::source_of(mc).or_else(|| crate::mesh_store::kept_source(&file).map(|p| p.to_string_lossy().into_owned())) {
        match studio_modeling::asset::load(std::path::Path::new(&src)) {
            Ok(a) if !a.parts.is_empty() => return Ok((a, mesh)),
            Ok(_) => {}
            Err(e) => return Err(format!("{}'s editable source: {e}", mc.name)),
        }
    }
    Ok((asset_from_mesh(&mesh, &mc.name), mesh))
}

/// An editable source from triangles.
pub fn asset_from_mesh(mesh: &Mesh, name: &str) -> Asset {
    crate::mesh_store::asset_from_mesh(mesh, name)
}

/// Opens the selected model in edit (or sculpt) mode.
pub fn enter(app: &mut StudioApp, ctx: &egui::Context, stage: Stage) -> Result<(), String> {
    if stage == Stage::Object {
        leave(app, ctx);
        return Ok(());
    }
    let id = match open_clip() {
        Some(id) => id,
        None => candidate(app).ok_or("select one 3D model to edit (click it in the 3D workspace)")?,
    };
    if open_clip() == Some(id) {
        with(|s| {
            if let Some(s) = s {
                s.stage = stage;
                s.modal = None;
            }
        });
        if stage == Stage::Sculpt {
            sculpt3d::begin(ctx);
        }
        return Ok(());
    }
    let mc = app.doc.model.active().clip(m::ClipId(id)).cloned().ok_or("that clip is gone")?;
    let (asset, mesh) = load_source(&mc)?;
    let file = crate::mesh_store::file_of(&mc).unwrap_or_default();
    let rig = mesh.rig.clone().map(|r| (r, mesh.clone()));
    let mode = with(|s| s.as_ref().map(|s| s.sel.mode)).unwrap_or(Mode::Vertex);
    let sel = Selection::new(&asset.parts[0].mesh, mode);
    let session = Session {
        clip: id,
        part: 0,
        asset,
        source: crate::mesh_store::source_of(&mc),
        file,
        sel,
        stage,
        tool: SelectTool::Box,
        prop: Proportional::default(),
        snap_to: SnapTo::Grid,
        mirror: [false; 3],
        show: Show::default(),
        last: None,
        modal: None,
        version: 1,
        cage: Cage { key: (u64::MAX, [0; 6]), seen: Vec::new() },
        rig,
        brush: 24.0,
        local: None,
        lattice: None,
        previews: 0,
    };
    with(|s| *s = Some(session));
    app.editor.selection = vec![id as u32];
    if !super::viewport::with(|w| w.on) {
        super::viewport::ui::set_on(app, ctx, true);
    }
    if stage == Stage::Sculpt {
        sculpt3d::begin(ctx);
    }
    ctx.request_repaint();
    Ok(())
}

/// Back to object mode (a transform under way is confirmed first).
pub fn leave(app: &mut StudioApp, ctx: &egui::Context) {
    if with(|s| s.as_ref().is_some_and(|s| s.modal.is_some())) {
        input::confirm(app, ctx);
    }
    // Materials that need baking were drawn quickly while editing: the
    // full look goes in on the way out.
    let needs_bake = with(|s| s.as_ref().is_some_and(|s| s.asset.parts.iter().any(|p| p.materials.iter().any(|n| s.asset.material(n).is_some_and(|d| !d.layers.is_empty() || d.base.pattern.is_some() || d.micro.is_some())))));
    if needs_bake {
        let _ = commit_with(app, "Bake materials", false);
    }
    if let Some(local) = with(|s| s.as_mut().and_then(|s| s.local.take())) {
        super::viewport::with(|w| {
            w.hidden = local;
            w.changed();
        });
    }
    sculpt3d::end(ctx);
    with(|s| *s = None);
    ctx.request_repaint();
}

/// Keeps the open session in step with the project: closes it when its
/// clip is gone, and reloads the source after undo, redo or the AI
/// changed the model.
pub fn follow(app: &mut StudioApp) {
    let Some((id, source, file, busy)) = with(|s| s.as_ref().map(|s| (s.clip, s.source.clone(), s.file.clone(), s.modal.is_some()))) else { return };
    let Some(mc) = app.doc.model.active().clip(m::ClipId(id)).cloned() else {
        with(|s| *s = None);
        return;
    };
    if busy {
        return;
    }
    // Another clip picked (in the timeline, the outliner): edit mode ends.
    if !app.editor.selection.is_empty() && !app.editor.selection.contains(&(id as u32)) {
        if let Some(local) = with(|s| s.take().and_then(|s| s.local)) {
            super::viewport::with(|w| {
                w.hidden = local;
                w.changed();
            });
        }
        return;
    }
    let now_source = crate::mesh_store::source_of(&mc);
    let now_file = crate::mesh_store::file_of(&mc).unwrap_or_default();
    if now_source == source && (now_file == file || now_file.starts_with("memory:")) {
        return;
    }
    match load_source(&mc) {
        Ok((asset, mesh)) => with(|s| {
            if let Some(s) = s {
                let keep = s.part.min(asset.parts.len().saturating_sub(1));
                s.asset = asset;
                s.part = keep;
                s.source = now_source;
                s.file = now_file;
                let p = s.poly().clone();
                s.sel.fit(&p);
                s.sel.sync(&p);
                s.last = None;
                if mesh.rig.is_some() {
                    s.rig = mesh.rig.clone().map(|r| (r, mesh.clone()));
                }
                s.changed();
            }
        }),
        Err(_) => with(|s| *s = None),
    }
}

/// The model's triangles as drawn: the source evaluated (modifiers run,
/// materials quick or baked), with its rig carried over.
pub fn evaluate(s: &Session, quick: bool) -> Result<Mesh, String> {
    let mut mesh = s.asset.to_mesh(quick)?;
    if mesh.triangles() == 0 {
        return Err("that would leave the model with nothing to draw".into());
    }
    if let Some((rig, was)) = &s.rig {
        mesh.rig = Some(Arc::new(rig.transferred(was, &mesh)));
    }
    Ok(mesh)
}

/// Writes the open model (source and triangles) and points the clip at
/// both: one undo step called `label`. Returns the step's label.
pub fn commit(app: &mut StudioApp, label: &str) -> Result<(), String> {
    commit_with(app, label, true)
}

fn commit_with(app: &mut StudioApp, label: &str, quick: bool) -> Result<(), String> {
    let Some(s) = with(|s| s.clone()) else { return Err("no model is open".into()) };
    let mut a = s.asset.clone();
    // What was done, for the AI and the history.
    if let Some(p) = a.parts.get_mut(s.part) {
        p.history.push(label.to_owned());
        if p.history.len() > 64 {
            p.history.remove(0);
        }
    }
    let staged = Session { asset: a, ..s.clone() };
    let mesh = evaluate(&staged, quick)?;
    let file = crate::mesh_store::save(&app.doc, &mesh)?;
    let source = crate::mesh_store::save_source(&app.doc, &staged.asset)?;
    studio_render::mesh_render::remember(&file, Arc::new(mesh));
    let seq = app.doc.seq();
    let Some(mut mc) = app.doc.model.active().clip(m::ClipId(s.clip)).cloned() else { return Err("the model's clip is gone".into()) };
    crate::mesh_store::set_file(&mut mc, file.clone());
    crate::mesh_store::set_source(&mut mc, Some(source.clone()));
    let edit = m::Edit::SetClip { seq, clip: Box::new(mc) };
    let ok = if app.doc.in_batch() { app.doc.apply_replayed(label, edit).is_ok() } else { app.doc.apply(label, edit) };
    if !ok {
        return Err(app.doc.error.take().unwrap_or_else(|| "the change was refused".into()));
    }
    with(|cur| {
        if let Some(cur) = cur {
            cur.asset = staged.asset;
            cur.source = Some(source);
            cur.file = file;
            cur.changed();
        }
    });
    Ok(())
}

/// Shows the open model as it is in memory (during a transform), without
/// writing anything: the clip points at a copy in memory, inside the
/// open batch.
pub fn preview(app: &mut StudioApp) {
    let Some(s) = with(|s| s.clone()) else { return };
    let Ok(mesh) = evaluate(&s, true) else { return };
    let n = with(|cur| {
        cur.as_mut().map(|c| {
            c.previews += 1;
            c.previews
        })
    })
    .unwrap_or(0);
    let name = format!("memory:edit-{}-{n}", s.clip);
    studio_render::mesh_render::remember(&name, Arc::new(mesh));
    let seq = app.doc.seq();
    if let Some(mut mc) = app.doc.model.active().clip(m::ClipId(s.clip)).cloned() {
        crate::mesh_store::set_file(&mut mc, name);
        let edit = m::Edit::SetClip { seq, clip: Box::new(mc) };
        if app.doc.in_batch() {
            let _ = app.doc.apply_replayed("Preview", edit);
        }
    }
}

/// The 3D cursor and the eye's direction in the open part's own space.
pub fn local_ctx(app: &StudioApp, view: Option<&studio_model::space::View>) -> Ctx {
    let Some(s) = with(|s| s.clone()) else { return Ctx::default() };
    let ws = super::viewport::snapshot();
    let space = draw::Space::of(app, &s);
    let cursor = space.as_ref().map_or([0.0; 3], |sp| sp.to_local(ws.cursor));
    let dir = match (view, space.as_ref()) {
        (Some(v), Some(sp)) => {
            let f = v.axes[2];
            sp.dir_to_local(f)
        }
        _ => [0.0, 0.0, 1.0],
    };
    s.ctx(cursor, dir)
}

/// Runs an operation on the open model (with `args`, defaults for the
/// rest) and keeps it as the last operation. Errors go to a notice.
pub fn run(app: &mut StudioApp, ctx: &egui::Context, id: &'static str, args: Map<String, Value>) -> Result<String, String> {
    let view = super::viewport::current_view(app.doc.project.width as f64);
    let mut c = local_ctx(app, view.as_ref());
    c.pivot = with(|s| s.as_ref().and_then(|s| input::pivot_local(app, s)));
    run_at(app, ctx, id, args, c, None)
}

/// Runs an operation in a given context; `replace` re-runs the last one
/// (its undo step is taken back first).
pub fn run_at(app: &mut StudioApp, ctx: &egui::Context, id: &'static str, args: Map<String, Value>, c: Ctx, replace: Option<Last>) -> Result<String, String> {
    let Some(mut s) = with(|s| s.clone()) else { return Err("open a model in edit mode first (Tab)".into()) };
    if let Some(r) = &replace {
        // Back to before the operation, then again with the new settings.
        if r.step.is_some() && app.doc.can_undo() {
            app.doc.undo();
        }
        let p = s.part;
        s.asset.parts[p].mesh = r.before.0.clone();
        s.sel = r.before.1.clone();
    }
    let before = (s.poly().clone(), s.sel.clone());
    let p = s.part;
    let mut poly = s.asset.parts[p].mesh.clone();
    let mut sel = s.sel.clone();
    let done = tools::run(&mut poly, &mut sel, id, &args, &c);
    let done = match done {
        Ok(d) => d,
        Err(e) => {
            if replace.is_some() {
                // Put the last result back as it was.
                app.doc.redo();
            }
            kit::push(ctx, e.clone(), None, Some(Icon::Warning));
            return Err(e);
        }
    };
    s.asset.parts[p].mesh = poly;
    s.sel = sel;
    s.changed();
    let label = tools::find(id).map_or("Edit model", |t| t.label).to_owned();
    with(|cur| *cur = Some(s.clone()));
    let mut step = None;
    if done.changed {
        if let Err(e) = commit(app, &label) {
            // Nothing written: the session goes back.
            with(|cur| {
                if let Some(cur) = cur {
                    cur.asset.parts[p].mesh = before.0.clone();
                    cur.sel = before.1.clone();
                    cur.changed();
                }
            });
            kit::push(ctx, e.clone(), None, Some(Icon::Warning));
            return Err(e);
        }
        step = Some(label.clone());
    }
    if !done.pieces.is_empty() {
        separate_into_objects(app, &done.pieces);
    }
    let last = Last { id, args, ctx: c, before, step, said: done.said.clone() };
    with(|cur| {
        if let Some(cur) = cur {
            cur.last = Some(last);
        }
    });
    ctx.request_repaint();
    Ok(done.said)
}

/// Re-runs the last operation with changed settings.
pub fn rerun(app: &mut StudioApp, ctx: &egui::Context, args: Map<String, Value>) {
    let Some(last) = with(|s| s.as_ref().and_then(|s| s.last.clone())) else { return };
    let c = last.ctx;
    let _ = run_at(app, ctx, last.id, args, c, Some(last));
}

/// Pieces taken out of the model become model clips of their own, placed
/// where the model is.
fn separate_into_objects(app: &mut StudioApp, pieces: &[Poly]) {
    let Some(s) = with(|s| s.clone()) else { return };
    let Some(mc) = app.doc.model.active().clip(m::ClipId(s.clip)).cloned() else { return };
    for (i, piece) in pieces.iter().enumerate() {
        let mut a = Asset::new(&format!("{} {}", mc.name, i + 2));
        let mut part = s.part().clone();
        part.mesh = piece.clone();
        part.name = "Body".into();
        a.parts.push(part);
        a.materials = s.asset.materials.clone();
        let Ok(mesh) = a.to_mesh(true) else { continue };
        let (Ok(file), Ok(source)) = (crate::mesh_store::save(&app.doc, &mesh), crate::mesh_store::save_source(&app.doc, &a)) else { continue };
        let lane = app.doc.alloc_id();
        let id = app.doc.alloc_id();
        let mut c = mc.clone();
        c.id = m::ClipId(id);
        c.lane = m::LaneId(lane);
        c.name = a.name.clone();
        crate::mesh_store::set_file(&mut c, file);
        crate::mesh_store::set_source(&mut c, Some(source));
        let seq = app.doc.seq();
        let index = app.doc.model.active().lanes.iter().position(|l| l.id == mc.lane);
        let edits = vec![m::Edit::AddLane { seq, lane: m::Lane::new(m::LaneId(lane), m::LaneKind::Visual, &c.name), index }, m::Edit::InsertClip { seq, clip: Box::new(c.clone()) }];
        let edit = m::Edit::Batch { edits };
        let label = format!("Separate {}", c.name);
        if app.doc.in_batch() {
            let _ = app.doc.apply_replayed(&label, edit);
        } else {
            app.doc.apply(&label, edit);
        }
    }
}

/// A new model clip of a shape (object mode's Add menu): on a lane of
/// its own, at the 3D cursor, selected.
pub fn add_object(app: &mut StudioApp, ctx: &egui::Context, id: &'static str, args: &Map<String, Value>) -> Result<u32, String> {
    let w = app.doc.project.width.min(app.doc.project.height) as f32;
    let c = Ctx { size: w * 1.6, ..Ctx::default() };
    let poly = tools::primitive(id, args, &c)?;
    let label = tools::find(id).map_or("Shape", |t| t.label);
    let mut a = Asset::new(label);
    let mut part = Part::new("Body", poly);
    part.smooth_angle = if matches!(id, "add_cube" | "add_plane" | "add_grid" | "add_cylinder" | "add_cone" | "add_circle") { 30.0 } else { 180.0 };
    a.parts.push(part);
    let mesh = a.to_mesh(true)?;
    let file = crate::mesh_store::save(&app.doc, &mesh)?;
    let source = crate::mesh_store::save_source(&app.doc, &a)?;
    let cursor = super::viewport::snapshot().cursor;
    let seq = app.doc.seq();
    let lane = app.doc.alloc_id();
    let clip = app.doc.alloc_id();
    let dur = app.doc.project.duration().max(crate::view::SEC);
    let mut mc = crate::mesh_store::new_clip(clip, lane, 0, dur, file, label);
    crate::mesh_store::set_source(&mut mc, Some(source));
    mc.transform.position = m::Vec2::new(cursor[0], cursor[1]);
    mc.transform.position_z = cursor[2];
    let edits = vec![m::Edit::AddLane { seq, lane: m::Lane::new(m::LaneId(lane), m::LaneKind::Visual, label), index: Some(0) }, m::Edit::InsertClip { seq, clip: Box::new(mc) }];
    if !app.doc.apply(&format!("Add {}", label.to_lowercase()), m::Edit::Batch { edits }) {
        return Err(app.doc.error.take().unwrap_or_else(|| "could not add the shape".into()));
    }
    app.editor.selection = vec![clip as u32];
    ctx.request_repaint();
    Ok(clip as u32)
}

/// A short status line for the open model (counts of what is picked).
pub fn status() -> Option<String> {
    with(|s| {
        s.as_ref().map(|s| {
            let (v, e, f) = s.sel.count();
            let p = s.poly();
            let tris: usize = (0..p.faces()).map(|f| p.face(f).len().saturating_sub(2)).sum();
            crate::i18n::tf(format!("Vertices {v}/{} · Edges {e}/{} · Faces {f}/{} · Triangles {tris}", p.positions.len(), p.edges().len(), p.faces()))
        })
    })
}
