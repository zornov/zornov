// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The editor: Library, Viewer, Inspector and Timeline (and any other
//! docked panels), separated by gaps with draggable splitters (spec §2),
//! laid out by the dock tree (`crate::dock`, drawn by `dock_ui`).

pub(crate) mod agent_digest;
pub(crate) mod agent_panel;
pub(crate) mod dock_ui;
mod ai_marks;
mod inspector;
pub(crate) mod library;
mod library_content;
mod library_plugins;
pub(crate) mod pins;
pub(crate) mod scissor;
pub(crate) mod bones;
#[cfg(feature = "render")]
pub mod render_view;
mod timeline;
pub(crate) mod cutting;
mod topbar;
mod viewer;
mod three_d;
mod anim3d;
mod vector;
pub mod viewport;
#[cfg(feature = "render")]
mod sculpt;
#[cfg(feature = "render")]
pub mod model_edit;
#[cfg(feature = "render")]
pub mod track;
#[cfg(feature = "render")]
pub mod cutout;
pub mod generate;
#[cfg(feature = "render")]
pub mod match_colour;
#[cfg(feature = "render")]
pub mod in_scene;
#[cfg(feature = "render")]
pub mod slowmo;
pub(crate) mod capture;
pub(crate) mod xform;
mod keys;
pub(crate) mod graph;
pub(crate) mod comp;
pub(crate) mod mixer;
mod drivers;
// Motion graphics: templates, shape layers, text animation.
mod mograph;
pub(crate) mod params;
pub(crate) mod clip_menu;
pub(crate) mod audio_fx;
pub(crate) mod world_edit;
pub(crate) mod sim_edit;
pub(crate) mod canvas_text;
pub(crate) mod modifiers;
mod physics;
mod body3d;
pub(crate) mod review;
pub(crate) mod scopes;
mod story;

use crate::app::{Overlay, StudioApp};
use crate::doc::Doc;
use crate::icons::Icon;
use crate::kit;
use crate::view::{self, ClipKind, SEC, Time};
use crate::prefs::Prefs;
use crate::theme;
use studio_model as m;
use eframe::egui::{self, CornerRadius, Rect, Sense, Vec2};

pub use library::RailItem;

pub const GAP: f32 = 3.0;
pub const RAIL_W: f32 = 0.0;
pub const TOPBAR_H: f32 = 40.0;
pub const LIBRARY_MIN: f32 = 260.0;
pub const LIBRARY_DEFAULT: f32 = 560.0;
pub const VIEWER_MIN: f32 = 420.0;
pub const INSPECTOR_MIN: f32 = 280.0;
pub const INSPECTOR_DEFAULT: f32 = 310.0;
pub const TIMELINE_MIN: f32 = 180.0;
pub const TIMELINE_DEFAULT_FRAC: f32 = 0.37;
pub const TOP_MIN: f32 = 240.0;
/// Below this window width the Inspector floats as a sheet.
pub const NARROW: f32 = 1100.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Select,
    Blade,
    Slip,
    Range,
}

/// Something the user is dragging in the timeline.
#[derive(Clone, Debug)]
pub enum TlDrag {
    /// Clips follow the pointer as a preview; the edit is sent on release.
    Move { ids: Vec<u32>, grab: Time, origin: Vec<(u32, Time, u32)>, delta: Time, lane_offset: i32 },
    /// Trims apply live inside one history transaction; `applied` is the
    /// delta already sent.
    Trim { id: u32, left: bool, origin_start: Time, origin_dur: Time, origin_in: Time, applied: Time },
    Slip { id: u32, origin_in: Time, applied: Time },
    Range { from: Time },
    Scrub,
    /// Box selection from `origin` (screen point); `base` is the selection
    /// it adds to (Shift / Cmd held) or empty.
    Marquee { origin: egui::Pos2, base: Vec<u32> },
    LaneHeight { lane: u32, origin: f32 },
}

pub struct EditorState {
    pub selection: Vec<u32>,
    /// A selected transition (its model id); clips are then unselected.
    pub selected_cut: Option<u64>,
    pub playhead: Time,
    pub playing: bool,
    /// Playback speed multiplier (J/K/L shuttle).
    pub rate: f32,
    pub loop_on: bool,
    pub quality: usize,
    /// None = fit.
    pub zoom: Option<f32>,
    pub safe_area: bool,
    pub thirds: bool,
    pub before_after: bool,
    pub split_pos: f32,
    pub fullscreen: bool,
    pub tool: Tool,
    pub px_per_sec: f32,
    pub tl_scroll: Vec2,
    pub rail: RailItem,
    pub library_query: String,
    pub library_chip: [usize; 10],
    pub library_selected: Option<(RailItem, usize)>,
    /// Where every panel is: the one source of truth for the layout.
    pub dock: crate::dock::Tree,
    /// The dock as last drawn (sizes for edits made between frames).
    pub dock_layout: Option<crate::dock::Layout>,
    /// A panel being dragged to a new place.
    pub dock_drag: Option<dock_ui::DockDrag>,
    /// The panel filling the whole dock (View ▸ Maximise panel), if any.
    pub maximised: Option<crate::dock::Panel>,
    /// The open flags of the panels that also show undocked, as the dock
    /// last left them (see `dock_ui::sync_flags`).
    pub dock_flags: [Option<bool>; 5],
    /// A popped-out panel whose window should come forward (⌘K on the AI
    /// editor in its own window).
    pub focus_window: Option<crate::dock::Panel>,
    /// The toast offering to undo the last move, and the layout before it.
    pub dock_undo: Option<(u64, crate::dock::Tree)>,
    /// Inspector sheet in narrow windows.
    pub sheet_open: bool,
    pub tl_drag: Option<TlDrag>,
    pub snap_line: Option<(Time, f64)>,
    pub renaming_lane: Option<(u32, String)>,
    pub renaming_project: Option<String>,
    /// Words being edited on the picture (double-click a title or template text).
    pub text_edit: Option<canvas_text::Editing>,
    pub viewer_rect: Rect,
    pub fit_pending: bool,
    /// Refit when the panel resizes, until the user zooms.
    pub auto_fit: bool,
    pub fit_width: f32,
    pub hover_time: Option<Time>,
    /// Last on-canvas click, for "click again cycles to the layer below".
    pub last_pick: Option<(egui::Pos2, Vec<u32>, usize)>,
    pub undo_toast: Option<u64>,
    pub redo_toast: Option<u64>,
    /// Trim modes, the source monitor and the selected edit point.
    pub cutting: cutting::Cutting,
}

impl EditorState {
    pub fn new(prefs: &Prefs) -> Self {
        let l = &prefs.layout;
        let mut s = Self {
            selection: Vec::new(),
            selected_cut: None,
            playhead: 4 * SEC + SEC / 3,
            playing: false,
            rate: 1.0,
            loop_on: false,
            quality: 0,
            zoom: None,
            safe_area: false,
            thirds: false,
            before_after: false,
            split_pos: 0.5,
            fullscreen: false,
            tool: Tool::Select,
            px_per_sec: 24.0,
            tl_scroll: Vec2::ZERO,
            rail: RailItem::Media,
            library_query: String::new(),
            library_chip: [0; 10],
            library_selected: None,
            // The saved tree, or the old fixed layout's sizes as a tree.
            dock: prefs.dock.clone().map_or_else(|| crate::dock::Tree::from_layout(l), crate::dock::Tree::sanitized),
            dock_layout: None,
            dock_drag: None,
            dock_flags: [None; 5],
            maximised: None,
            dock_undo: None,
            focus_window: None,
            sheet_open: false,
            tl_drag: None,
            snap_line: None,
            renaming_lane: None,
            renaming_project: None,
            text_edit: None,
            viewer_rect: Rect::NOTHING,
            fit_pending: true,
            auto_fit: true,
            fit_width: 0.0,
            hover_time: None,
            last_pick: None,
            undo_toast: None,
            redo_toast: None,
            cutting: Default::default(),
        };
        // The saved layout is the workspace as it was left; the Library section
        // is the one thing it does not record.
        if let Some(w) = crate::workspaces::find(&prefs.workspace) {
            s.rail = crate::workspaces::rail_of(&w);
        }
        s
    }

    /// The dock as last laid out (a typical window before the first frame).
    pub fn last_layout(&self) -> crate::dock::Layout {
        self.dock_layout.clone().unwrap_or_else(|| dock_ui::fallback_layout(&self.dock))
    }

    /// Whether a panel is docked and open.
    pub fn panel_open(&self, p: crate::dock::Panel) -> bool {
        self.dock.is_open(p)
    }

    /// Opens a docked panel (where it was, or its default place).
    pub fn open_panel(&mut self, p: crate::dock::Panel) {
        let lay = self.last_layout();
        self.dock.open(p, &lay);
    }

    /// Replaces the whole layout (a workspace, Reset layout). A panel that
    /// was docked and is not in the new tree is turned off next frame.
    pub fn set_tree(&mut self, t: crate::dock::Tree) {
        self.dock = t.sanitized();
        self.dock_drag = None;
        self.maximised = None;
        // A workspace switch keeps the zoom and scroll the person chose.
        if self.auto_fit {
            self.fit_pending = true;
        }
    }

    pub fn frame_dur(&self, fps: f32) -> Time {
        (SEC as f64 / fps.max(1.0) as f64).round() as Time
    }
}

fn panel(ui: &egui::Ui, r: Rect) {
    let t = theme::tokens(ui.ctx());
    theme::surface(ui.painter(), r, theme::radius::PANEL, t.panel, &t);
}

/// A splitter between regions. Returns the drag delta along its axis, and
/// whether it was double-clicked.
fn splitter(ui: &egui::Ui, id: egui::Id, line: Rect, vertical: bool) -> (f32, bool, bool) {
    let t = theme::tokens(ui.ctx());
    // 8 px hit area centred on the 4 px gap.
    let hit = if vertical { line.expand2(Vec2::new(2.0, 0.0)) } else { line.expand2(Vec2::new(0.0, 2.0)) };
    let resp = ui.interact(hit, id, Sense::click_and_drag());
    let cursor = if vertical { egui::CursorIcon::ResizeColumn } else { egui::CursorIcon::ResizeRow };
    let resp = resp.on_hover_and_drag_cursor(cursor);
    let active = resp.hovered() || resp.dragged();
    let k = kit::hover_t(ui.ctx(), resp.id, active);
    if k > 0.0 {
        let c = line.center();
        let bar = if vertical { Rect::from_center_size(c, Vec2::new(2.0, line.height() - 16.0)) } else { Rect::from_center_size(c, Vec2::new(line.width() - 16.0, 2.0)) };
        ui.painter().rect_filled(bar, CornerRadius::same(1), theme::alpha(t.line, k));
    }
    let d = if vertical { resp.drag_delta().x } else { resp.drag_delta().y };
    (d, resp.double_clicked(), resp.drag_stopped())
}

pub fn show(app: &mut StudioApp, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    let full = ui.max_rect();
    crate::a11y::begin_frame(&ctx);
    // Undo puts back the selection an edit was made with.
    app.doc.track_selection(&app.editor.selection);
    // Esc cancels a panel drag before anything else hears it.
    dock_ui::keys(app, &ctx);
    // The editor's keys are for the timeline and the viewer. A panel or
    // editor that has the keyboard (`keymap::claim`) keeps its letters,
    // Delete, ⌘A and arrows; only F6 and the app-wide shortcuts get through.
    if crate::keymap::owner(&ctx).is_none() {
        // Film editing keys before the accessibility keys: `,` `.` `[` `]`
        // edit when there is a source or an edit point, and step otherwise.
        cutting::shortcuts(app, &ctx);
        // Alt combinations first: a plain shortcut also matches them.
        crate::a11y::keys(app, &ctx);
        #[cfg(feature = "render")]
        viewport::ui::keys(app, &ctx);
    } else if app.overlay == Overlay::None && !crate::onboarding::tour_running(app) {
        // F6 is the way out of a panel, even a text field.
        let sf6 = ctx.input_mut(|i| i.consume_key(egui::Modifiers::SHIFT, egui::Key::F6));
        if sf6 || ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::F6)) {
            crate::a11y::cycle_region(&ctx, sf6);
        }
    }
    shortcuts(app, &ctx);
    playback(app, &ctx);
    if app.cli.shot.is_none() {
        live_sound(app);
    }
    library::poll_ai(app, &ctx);
    review::tick(app, &ctx);
    cutting::tick(app, &ctx);
    clip_menu::show(app, &ctx);
    let t = app.editor.hover_time.unwrap_or(app.editor.playhead);
    app.doc.set_time(t);
    if let Some(msg) = app.doc.error.take() {
        kit::push(&ctx, msg, None, Some(Icon::Warning));
    }
    if !ctx.input(|i| i.pointer.any_down()) && !ctx.egui_wants_keyboard_input() {
        app.doc.end_gesture();
    }

    if app.editor.fullscreen {
        let r = full.shrink(GAP);
        panel(ui, r);
        viewer::show(app, ui, r);
        capture::show(app, &ctx);
        vector::show(app, &ctx);
        cutting::panel::show(app, &ctx);
        toasts(app, &ctx);
        return;
    }

    let rtl = crate::i18n::is_rtl();
    let narrow = full.width() < NARROW;
    let topbar_r = Rect::from_min_size(full.min, Vec2::new(full.width(), TOPBAR_H));
    topbar::show(app, ui, topbar_r, narrow);
    // Every panel in its tile, the splitters, dragging panels to dock them.
    let body = dock_ui::body(full);
    dock_ui::show(app, ui, body, narrow, rtl);

    if narrow {
        inspector::sheet(app, ui, body);
    }
    // Windows of their own (they stay when the Viewer is a tab behind another).
    capture::show(app, &ctx);
    vector::show(app, &ctx);
    cutting::panel::show(app, &ctx);
    // The layout persists as it is now (and in the old fields for an older Studio).
    if app.prefs.dock.as_ref() != Some(&app.editor.dock) {
        app.prefs.dock = Some(app.editor.dock.clone());
        let ((lo, lw), (io, iw), (to, th)) = app.editor.dock.legacy_fields();
        app.prefs.layout = crate::prefs::Layout { library_w: Some(lw), inspector_w: Some(iw), timeline_h: th, library_open: Some(lo), inspector_open: Some(io), timeline_open: Some(to) };
    }
    review::show(app, &ctx);
    toasts(app, &ctx);
}

fn toasts(app: &mut StudioApp, ctx: &egui::Context) {
    // Above the floating transport.
    if let Some(id) = kit::show(ctx, app.editor.viewer_rect, 72.0) {
        if let Some((_, before)) = app.editor.dock_undo.take_if(|(did, _)| *did == id) {
            app.editor.set_tree(before);
        } else if Some(id) == app.editor.undo_toast {
            undo_step(&mut app.editor, &mut app.doc, ctx);
        } else if let Some((_, path)) = app.reveal.take().filter(|(rid, _)| *rid == id) {
            let _ = std::process::Command::new("open").arg("-R").arg(&path).spawn();
        } else if Some(id) == app.editor.redo_toast {
            redo_step(&mut app.editor, &mut app.doc, ctx);
        }
    }
}

/// Advance the playhead while playing.
fn playback(app: &mut StudioApp, ctx: &egui::Context) {
    let ed = &mut app.editor;
    if !ed.playing {
        return;
    }
    let dt = ctx.input(|i| i.stable_dt).min(0.1) as f64;
    let dur = app.doc.project.duration().max(SEC);
    let (lo, hi) = match (ed.loop_on, app.doc.project.range) {
        (true, Some(r)) => r,
        _ => (0, dur),
    };
    let mut t = ed.playhead + (dt * ed.rate as f64 * SEC as f64) as Time;
    if t >= hi {
        if ed.loop_on {
            t = lo + (t - hi) % (hi - lo).max(1);
        } else {
            t = hi;
            ed.playing = false;
        }
    }
    if t < 0 {
        t = 0;
        ed.playing = false;
    }
    ed.playhead = t;
    ctx.request_repaint();
}

/// Keeps the timeline's sound in step with the transport (one shared
/// player for the whole app).
fn live_sound(app: &StudioApp) {
    static PLAYER: std::sync::OnceLock<std::sync::Mutex<crate::sound_bridge::LivePlayer>> = std::sync::OnceLock::new();
    let player = PLAYER.get_or_init(|| std::sync::Mutex::new(crate::sound_bridge::LivePlayer::new()));
    let ed = &app.editor;
    player.lock().unwrap_or_else(|e| e.into_inner()).sync(&app.doc.model, app.doc.edits, ed.playing, ed.playhead, ed.rate);
}

pub fn toggle_play(ed: &mut EditorState) {
    ed.playing = !ed.playing;
    ed.rate = 1.0;
}

/// Editor actions shared by shortcuts, menus and buttons.
pub enum Action {
    Undo,
    Redo,
    Split,
    Delete,
    Duplicate,
    SelectAll,
    Marker,
    RangeIn,
    RangeOut,
    ClearRange,
    Export,
    ZoomIn,
    ZoomOut,
    Fit,
    Open,
    Save,
    SaveAs,
    ImportMedia,
    SampleMedia,
    ImportTimeline,
    /// File ▸ Import script.
    ImportScript,
    ExportOtio,
    /// File ▸ Project settings…: the Inspector's Details with the canvas editor open.
    ProjectSettings,
    /// File ▸ Settings….
    Settings,
    /// Project ▸ Production….
    Production,
    /// Project ▸ Project health….
    ProjectHealth,
    /// View ▸ Maximise panel.
    Maximise,
    /// Help ▸ Search help.
    Help,
    /// Help ▸ Welcome tour.
    Tour,
    /// Help ▸ Keyboard shortcuts.
    Shortcuts,
    /// Help ▸ Check for updates.
    CheckUpdates,
}

pub fn run(app: &mut StudioApp, ctx: &egui::Context, action: Action) {
    // File actions open native dialogs; screenshot runs never do.
    let dialogs = app.cli.shot.is_none();
    match action {
        Action::Open if dialogs => return app.open_folder(ctx, None),
        Action::Save if dialogs => return app.save(ctx, false),
        Action::SaveAs if dialogs => return app.save(ctx, true),
        Action::ImportMedia if dialogs => return app.import_media_files(ctx),
        Action::ImportTimeline if dialogs => return app.import_timeline(ctx),
        Action::ImportScript if dialogs => return story::import_dialog(app, ctx),
        Action::ExportOtio if dialogs => return crate::files::export_otio(app, ctx),
        Action::SampleMedia | Action::ImportMedia => return library::import_sample_media(app, ctx),
        Action::Open | Action::Save | Action::SaveAs | Action::ImportTimeline | Action::ImportScript | Action::ExportOtio => return,
        _ => {}
    }
    let ed = &mut app.editor;
    let doc = &mut app.doc;
    match action {
        Action::Undo => {
            undo_step(ed, doc, ctx);
        }
        Action::Redo => {
            redo_step(ed, doc, ctx);
        }
        Action::Split => {
            let t = ed.playhead;
            let before = doc.project.clips.len();
            let clips = (!ed.selection.is_empty()).then(|| ed.selection.iter().map(|id| m::ClipId(*id as u64)).collect());
            let seq = doc.seq();
            doc.apply("Split", m::Edit::Split { seq, at: t, clips });
            if doc.project.clips.len() == before && doc.error.is_none() {
                kit::push(ctx, if ed.selection.is_empty() { "No clip under the playhead to split" } else { "The playhead is not inside the selected clip" }, None, Some(Icon::Info));
            }
            if doc.project.clips.len() > before {
                toast_undo(ed, ctx, crate::i18n::tf(format!("Split at {}", view::timecode(t, doc.project.fps))), Icon::Scissors);
            }
        }
        Action::Delete => {
            if let Some(id) = ed.selected_cut.take() {
                let seq = doc.seq();
                if doc.apply("Remove cut", m::Edit::RemoveTransition { seq, id: m::TransitionId(id) }) {
                    toast_undo(ed, ctx, "Cut removed", Icon::Trash);
                }
                return;
            }
            delete_selected(ed, doc, ctx, false);
        }
        Action::Duplicate => {
            if ed.selection.is_empty() {
                kit::push(ctx, "Select a clip to duplicate", None, Some(Icon::Info));
                return;
            }
            let old: Vec<u32> = doc.project.clips.iter().map(|c| c.id).collect();
            let seq = doc.seq();
            let clips = ed.selection.iter().map(|id| m::ClipId(*id as u64)).collect();
            if doc.apply("Duplicate", m::Edit::Duplicate { seq, clips }) {
                ed.selection = doc.project.clips.iter().map(|c| c.id).filter(|id| !old.contains(id)).collect();
                let n = ed.selection.len();
                toast_undo(ed, ctx, if n == 1 { "Duplicated clip".to_owned() } else { format!("Duplicated {n} clips") }, Icon::Copy);
            }
        }
        Action::SelectAll => ed.selection = doc.project.clips.iter().map(|c| c.id).collect(),
        Action::Marker => {
            let t = ed.playhead;
            let ok = doc.edit("Add marker", |p| {
                let id = p.alloc_id() as u64;
                p.markers.push(view::Marker { id, time: t, note: "Marker".into() });
            });
            range_toast(ctx, doc, ok, format!("Marker added at {}", view::timecode(t, doc.project.fps)));
        }
        Action::RangeIn => {
            let t = ed.playhead;
            let ok = doc.edit("Set in point", |p| {
                let out = p.range.map_or(p.duration(), |r| r.1).max(t + SEC / 10);
                p.range = Some((t, out));
            });
            range_toast(ctx, doc, ok, format!("In point set at {}", view::timecode(t, doc.project.fps)));
        }
        Action::RangeOut => {
            let t = ed.playhead;
            let ok = doc.edit("Set out point", |p| {
                let inn = p.range.map_or(0, |r| r.0).min(t - SEC / 10).max(0);
                p.range = Some((inn, t));
            });
            range_toast(ctx, doc, ok, format!("Out point set at {}", view::timecode(t, doc.project.fps)));
        }
        Action::ClearRange => {
            doc.edit("Clear range", |p| p.range = None);
        }
        Action::Export => app.overlay = Overlay::Export,
        Action::ZoomIn => {
            ed.px_per_sec = (ed.px_per_sec * 1.25).min(600.0);
            ed.auto_fit = false;
        }
        Action::ZoomOut => {
            ed.px_per_sec = (ed.px_per_sec / 1.25).max(2.0);
            ed.auto_fit = false;
        }
        Action::Fit => {
            ed.fit_pending = true;
            ed.auto_fit = true;
        }
        Action::ProjectSettings => {
            // The Details view is what the Inspector shows with nothing
            // selected; "Modify" there is the canvas, frame rate and background.
            app.editor.selection.clear();
            app.editor.selected_cut = None;
            dock_ui::open_panel(app, ctx, crate::dock::Panel::Inspector, ctx.content_rect().width() < NARROW);
            ctx.data_mut(|d| d.insert_temp(egui::Id::unique("details-modify"), true));
        }
        Action::Settings => app.overlay = Overlay::Settings,
        Action::Production => app.overlay = Overlay::Production,
        Action::ProjectHealth => app.overlay = Overlay::Library,
        Action::Maximise => dock_ui::toggle_maximised(app, ctx),
        Action::Help => crate::help::open(app, ctx, None),
        Action::Tour => {
            app.prefs.tour_done = false;
        }
        Action::Shortcuts => {
            app.settings.page = 3;
            app.overlay = Overlay::Settings;
        }
        Action::CheckUpdates => crate::release::check_from_menu(),
        _ => {}
    }
}

/// A toast with an Undo button that undoes the person's last step: the one
/// way every edit announces itself.
pub(crate) fn toast_undo(ed: &mut EditorState, ctx: &egui::Context, text: impl Into<String>, icon: Icon) {
    ed.undo_toast = Some(kit::push(ctx, text, Some("Undo"), Some(icon)));
}

/// Delete (or cut) the selected clips: the main lane closes up, free lanes
/// just lose the clip. Says what happened; false when nothing was removed.
pub(crate) fn delete_selected(ed: &mut EditorState, doc: &mut Doc, ctx: &egui::Context, cut: bool) -> bool {
    if ed.selection.is_empty() {
        kit::push(ctx, if cut { "Select a clip to cut" } else { "Select a clip to delete" }, None, Some(Icon::Info));
        return false;
    }
    let n = ed.selection.len();
    let seq = doc.seq();
    let spine = doc.project.spine();
    let (on_spine, others): (Vec<u32>, Vec<u32>) = ed.selection.iter().partition(|id| doc.project.clip(**id).is_some_and(|c| Some(c.lane) == spine));
    let ids = |v: &[u32]| v.iter().map(|id| m::ClipId(*id as u64)).collect::<Vec<_>>();
    let mut edits = Vec::new();
    if !others.is_empty() {
        edits.push(m::Edit::Lift { seq, clips: ids(&others) });
    }
    if !on_spine.is_empty() {
        edits.push(m::Edit::RippleDelete { seq, clips: ids(&on_spine) });
    }
    if !doc.apply(if cut { "Cut" } else { "Delete" }, m::Edit::Batch { edits }) {
        return false;
    }
    ed.selection.clear();
    let verb = if cut { "cut" } else { "deleted" };
    toast_undo(ed, ctx, if n == 1 { format!("Clip {verb}") } else { format!("{n} clips {verb}") }, if cut { Icon::Scissors } else { Icon::Trash });
    true
}

/// Say what a marker or in/out edit did; a refusal says so too (the
/// model's own reason, when it gave one, is shown by the error toast).
fn range_toast(ctx: &egui::Context, doc: &Doc, ok: bool, done: String) {
    if ok {
        kit::push(ctx, done, None, Some(Icon::Flag));
    } else if doc.error.is_none() {
        kit::push(ctx, "Nothing changed: it is already set there", None, Some(Icon::Info));
    }
}

/// Undo the person's last step and put the editor where it happened: the
/// changed clips are selected and the playhead moves to them when it is
/// elsewhere. With nothing to undo, say so.
fn undo_step(ed: &mut EditorState, doc: &mut Doc, ctx: &egui::Context) {
    match doc.undo() {
        Some(l) => {
            ed.redo_toast = Some(kit::push(ctx, crate::i18n::tf(format!("Undid {}", l.to_lowercase())), Some("Redo"), Some(Icon::ArrowUUpLeft)));
            show_changed(ed, doc);
        }
        None if doc.error.is_none() => {
            kit::push(ctx, "Nothing to undo", None, Some(Icon::ArrowUUpLeft));
        }
        None => {}
    }
}

fn redo_step(ed: &mut EditorState, doc: &mut Doc, ctx: &egui::Context) {
    match doc.redo() {
        Some(l) => {
            toast_undo(ed, ctx, crate::i18n::tf(format!("Redid {}", l.to_lowercase())), Icon::ArrowUUpRight);
            show_changed(ed, doc);
        }
        None if doc.error.is_none() => {
            kit::push(ctx, "Nothing to redo", None, Some(Icon::ArrowUUpRight));
        }
        None => {}
    }
}

/// After undo or redo: select what changed (clips that are gone drop out)
/// and bring the playhead to it when it is not already over it.
pub(crate) fn show_changed(ed: &mut EditorState, doc: &mut Doc) {
    show_changed_inner(ed, doc);
    doc.track_selection(&ed.selection);
}

fn show_changed_inner(ed: &mut EditorState, doc: &mut Doc) {
    let touched: Vec<u32> = doc.take_history_touched().into_iter().filter(|id| doc.project.clip(*id).is_some()).collect();
    ed.selection.retain(|id| doc.project.clip(*id).is_some());
    ed.selected_cut = None;
    // The selection as it was before the edit (after it, for redo), when
    // the document knows it; clips that are gone drop out. With nothing to
    // put back, the changed clips are selected so the change is seen.
    let restore: Option<Vec<u32>> = doc.take_selection_restore().map(|s| s.into_iter().filter(|id| doc.project.clip(*id).is_some()).collect::<Vec<u32>>()).filter(|s| !s.is_empty());
    if touched.is_empty() {
        if let Some(r) = restore {
            ed.selection = r;
        }
        return;
    }
    let spans: Vec<(Time, Time)> = touched.iter().filter_map(|id| doc.project.clip(*id)).map(|c| (c.start, c.end())).collect();
    let (lo, hi) = (spans.iter().map(|s| s.0).min().unwrap_or(0), spans.iter().map(|s| s.1).max().unwrap_or(0));
    if ed.playhead < lo || ed.playhead > hi {
        ed.playhead = lo;
    }
    ed.selection = restore.unwrap_or(touched);
}

/// Editor actions in Settings ▸ Shortcuts, run from the person's keys.
const EDITOR_KEYS: &[&str] = &[
    "redo", "undo", "duplicate", "select-all", "export", "zoom-in", "zoom-out", "fit", "clear-range", "play", "stop", "shuttle-fwd", "shuttle-back", "frame-back", "frame-fwd", "select", "blade", "slip", "range", "snap", "fullscreen", "in", "out", "split", "marker", "delete",
    "macro-record", "macro-run", "workspace-edit", "workspace-colour", "workspace-audio", "workspace-vfx", "workspace-motion", "workspace-production",
    "panel-library", "panel-inspector", "panel-timeline", "agent", "overlays", "all-projects",
    "second-back", "second-fwd", "to-start", "to-end", "delete-fwd", "zoom-in-plus", "group", "ungroup", "link", "unlink", "freeze", "reverse", "copy-look", "paste-look",
];

fn shortcuts(app: &mut StudioApp, ctx: &egui::Context) {
    use egui::{Key, KeyboardShortcut as S, Modifiers as M};
    // A text field keeps the keys; so does a focused button (Space and Enter
    // press it). A focused clip or region does not: they are how the keyboard
    // reaches the timeline.
    if ctx.text_edit_focused() || app.overlay != Overlay::None || crate::onboarding::tour_running(app) {
        return;
    }
    let widget_focus = ctx.memory(|m| m.focused()).is_some() && crate::a11y::focused_region(ctx).is_none() && !crate::a11y::clip_has_focus(ctx);
    let pressed = |m: M, k: Key| ctx.input_mut(|i| i.consume_shortcut(&S::new(m, k)));
    // None of the plain keys while a panel or editor owns the keyboard.
    let free = crate::keymap::owner(ctx).is_none();
    // ⌘C / ⌘X / ⌘V arrive as clipboard events, not key presses.
    if free {
        let (copy, cut, paste) = ctx.input(|i| (i.events.iter().any(|e| matches!(e, egui::Event::Copy)), i.events.iter().any(|e| matches!(e, egui::Event::Cut)), i.events.iter().any(|e| matches!(e, egui::Event::Paste(_)))));
        if cut {
            clip_menu::copy_selection(app, ctx, true);
        } else if copy {
            clip_menu::copy_selection(app, ctx, false);
        }
        if paste {
            clip_menu::paste_selection(app, ctx);
        }
    }
    let ids = crate::keymap::scoped(ctx, &app.prefs, EDITOR_KEYS);
    let mut hits = crate::keymap::pressed(ctx, &app.prefs, &ids);
    // A focused button takes Space (it presses the button); the timeline does not.
    if widget_focus {
        hits.retain(|h| *h != "play");
    }
    // In table order, so in and out come before split, as they always did.
    hits.sort_by_key(|h| EDITOR_KEYS.iter().position(|k| k == h));
    for id in hits {
        let ed = &mut app.editor;
        match id {
            "redo" => run(app, ctx, Action::Redo),
            "undo" => run(app, ctx, Action::Undo),
            "duplicate" => run(app, ctx, Action::Duplicate),
            "select-all" => run(app, ctx, Action::SelectAll),
            "export" => run(app, ctx, Action::Export),
            "zoom-in" => run(app, ctx, Action::ZoomIn),
            "zoom-out" => run(app, ctx, Action::ZoomOut),
            "fit" => run(app, ctx, Action::Fit),
            "clear-range" => run(app, ctx, Action::ClearRange),
            "play" => toggle_play(ed),
            "stop" => {
                ed.playing = false;
                ed.rate = 1.0;
            }
            "shuttle-fwd" => {
                ed.rate = if ed.playing && ed.rate > 0.0 { (ed.rate * 2.0).min(8.0) } else { 1.0 };
                ed.playing = true;
            }
            "shuttle-back" => {
                ed.rate = if ed.playing && ed.rate < 0.0 { (ed.rate * 2.0).max(-8.0) } else { -1.0 };
                ed.playing = true;
            }
            "frame-back" => {
                ed.playing = false;
                ed.playhead = crate::services::transport::step(&app.doc, ed.playhead, -1);
            }
            "frame-fwd" => {
                ed.playing = false;
                ed.playhead = crate::services::transport::step(&app.doc, ed.playhead, 1);
            }
            "select" => ed.tool = Tool::Select,
            "blade" => ed.tool = Tool::Blade,
            "slip" => ed.tool = Tool::Slip,
            "range" => ed.tool = Tool::Range,
            "snap" => app.prefs.snapping = !app.prefs.snapping,
            "fullscreen" => ed.fullscreen = !ed.fullscreen,
            "in" => run(app, ctx, Action::RangeIn),
            "out" => run(app, ctx, Action::RangeOut),
            "split" => run(app, ctx, Action::Split),
            "marker" => run(app, ctx, Action::Marker),
            "delete" | "delete-fwd" => run(app, ctx, Action::Delete),
            "zoom-in-plus" => run(app, ctx, Action::ZoomIn),
            "second-back" => app.editor.playhead = (app.editor.playhead - SEC).max(0),
            "second-fwd" => app.editor.playhead = (app.editor.playhead + SEC).min(crate::services::transport::duration(&app.doc)),
            "to-start" => app.editor.playhead = 0,
            "to-end" => app.editor.playhead = crate::services::transport::duration(&app.doc),
            "group" | "ungroup" | "link" | "unlink" | "freeze" | "reverse" | "copy-look" | "paste-look" => clip_menu::key_action(app, ctx, id),
            "agent" => {
                agent_panel::open(app);
                return;
            }
            "overlays" => kit::toggle_open(ctx, viewer::overlays_id()),
            "all-projects" => app.screen = crate::cli::Screen::Start,
            "macro-record" => library_plugins::toggle_recording(app, ctx),
            "macro-run" => library_plugins::run_last_macro(app, ctx),
            "panel-library" | "panel-inspector" | "panel-timeline" => {
                let p = crate::dock::Panel::parse(&id["panel-".len()..]).unwrap_or(crate::dock::Panel::Library);
                let narrow = ctx.content_rect().width() < NARROW;
                dock_ui::toggle_panel(app, ctx, p, narrow);
            }
            w if w.starts_with("workspace-") => {
                let name = crate::workspaces::for_action(w).unwrap_or("Edit");
                if let Some(ws) = crate::workspaces::find(name) {
                    crate::workspaces::switch(app, ctx, &ws);
                }
            }
            _ => {}
        }
    }
    let ed = &mut app.editor;
    // An open popover closes itself on Escape.
    if !kit::any_popover_open(ctx) && pressed(M::NONE, Key::Escape) {
        escape(ed, &mut app.doc, ctx.input(|i| i.pointer.any_down()));
    }
}

/// Esc, one step at a time: an open drag goes back, then full screen, the
/// Inspector sheet, and last the selection (a selected cut included).
fn escape(ed: &mut EditorState, doc: &mut Doc, pointer_down: bool) {
    if ed.tl_drag.is_some() || pointer_down {
        // Esc during a drag puts everything back.
        ed.tl_drag = None;
        ed.snap_line = None;
        doc.cancel_gesture();
    } else if ed.fullscreen {
        ed.fullscreen = false;
    } else if ed.sheet_open {
        ed.sheet_open = false;
    } else {
        ed.selection.clear();
        ed.selected_cut = None;
    }
}

/// An outline over the editor and a word while files from the desktop are
/// dragged over it (they are imported when dropped).
pub fn drop_hint(ctx: &egui::Context) {
    if ctx.input(|i| i.raw.hovered_files.is_empty()) {
        return;
    }
    let t = theme::tokens(ctx);
    let r = ctx.content_rect().shrink(6.0);
    let p = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::unique("window-drop")));
    p.rect_filled(r, CornerRadius::same(theme::radius::CARD), t.accent_soft.gamma_multiply(0.35));
    p.rect_stroke(r, CornerRadius::same(theme::radius::CARD), egui::Stroke::new(2.0, t.accent), egui::StrokeKind::Inside);
    p.text(r.center(), egui::Align2::CENTER_CENTER, crate::i18n::shown("Drop to import"), theme::body_medium(), t.accent);
}

/// Fill colour for a clip kind.
pub fn clip_fill(t: &theme::Tokens, kind: ClipKind) -> egui::Color32 {
    match kind {
        ClipKind::Video => t.video,
        ClipKind::Audio => t.audio,
        ClipKind::Title => t.title,
        ClipKind::Fx => t.fx,
    }
}

/// One headless frame for tests, finished the way a window's backend
/// finishes it: the shapes tessellated and the frame's texture changes
/// (the font atlas, images) taken as uploaded. egui panics when a frame's
/// texture changes are dropped unhandled, or when its clock runs back (a
/// helper that counts from 0 again on the same context), so a time at or
/// before the last frame's becomes the frame after it.
#[cfg(test)]
pub(crate) fn test_frame(ctx: &egui::Context, mut input: egui::RawInput, ui: impl FnMut(&mut egui::Ui)) {
    let last = ctx.input(|i| i.time);
    if input.time.is_some_and(|t| t <= last) && ctx.cumulative_pass_nr() > 0 {
        input.time = Some(last + 1.0 / 60.0);
    }
    let mut out = ctx.run_ui(input, ui);
    let _ = ctx.tessellate(std::mem::take(&mut out.shapes), out.pixels_per_point);
    out.textures_delta.clear();
}

/// Who started a background job: the app it was started from. A result
/// lands in that app only, not in another one drawn at the same time (the
/// tests run several side by side, each polling every job).
pub(crate) fn owner_of(app: &crate::app::StudioApp) -> usize {
    app as *const crate::app::StudioApp as usize
}

#[cfg(test)]
mod history_focus_tests {
    use super::*;
    use crate::view::SEC;

    #[test]
    fn undo_selects_what_changed_and_brings_the_playhead() {
        let mut doc = Doc::from_view(&view::demo());
        let mut ed = EditorState::new(&crate::prefs::Prefs::default());
        let spine = doc.project.spine();
        let id = doc.project.clips.iter().find(|c| Some(c.lane) != spine).unwrap().id;
        let start = doc.project.clip(id).unwrap().start;
        let seq = doc.seq();
        doc.apply("Trim", m::Edit::Trim { seq, clip: m::ClipId(id as u64), edge: m::Edge::End, delta: -SEC / 4, ripple: false });
        // The person looked elsewhere meanwhile.
        ed.selection.clear();
        ed.playhead = doc.project.duration() + 10 * SEC;
        assert!(doc.undo().is_some());
        show_changed(&mut ed, &mut doc);
        assert_eq!(ed.selection, vec![id]);
        assert_eq!(ed.playhead, start);
        // A playhead already over the clip stays where it is.
        let mid = start + SEC / 20;
        ed.playhead = mid;
        assert!(doc.redo().is_some());
        show_changed(&mut ed, &mut doc);
        assert_eq!(ed.playhead, mid);
    }

    /// Undo puts back the selection the edit was made with, redo the one
    /// it left, through the same path as the key, the menu and the toast.
    #[test]
    fn undo_restores_the_selection_the_edit_was_made_with() {
        let mut doc = Doc::from_view(&view::demo());
        let mut ed = EditorState::new(&crate::prefs::Prefs::default());
        let spine = doc.project.spine();
        let free: Vec<u32> = doc.project.clips.iter().filter(|c| Some(c.lane) != spine).map(|c| c.id).collect();
        let (a, b) = (free[0], free[1]);
        let seq = doc.seq();
        // Frame 1: a is selected; the edit trims b, and leaves b selected.
        ed.selection = vec![a];
        doc.track_selection(&ed.selection);
        assert!(doc.apply("Trim", m::Edit::Trim { seq, clip: m::ClipId(b as u64), edge: m::Edge::End, delta: -SEC / 4, ripple: false }));
        ed.selection = vec![b];
        // Frame 2 starts: the document learns the edit left b selected.
        doc.track_selection(&ed.selection);
        // The person clicks elsewhere meanwhile.
        ed.selection.clear();
        doc.track_selection(&ed.selection);
        assert!(doc.undo().is_some());
        show_changed(&mut ed, &mut doc);
        assert_eq!(ed.selection, vec![a], "undo: what was selected before the edit");
        assert!(doc.redo().is_some());
        show_changed(&mut ed, &mut doc);
        assert_eq!(ed.selection, vec![b], "redo: what the edit left selected");
        // A new edit forgets the redo marks and undo still works.
        doc.track_selection(&ed.selection);
        assert!(doc.apply("Trim", m::Edit::Trim { seq, clip: m::ClipId(a as u64), edge: m::Edge::End, delta: -SEC / 4, ripple: false }));
        ed.selection = vec![a];
        doc.track_selection(&ed.selection);
        assert!(doc.undo().is_some());
        show_changed(&mut ed, &mut doc);
        assert_eq!(ed.selection, vec![b]);
    }
}

#[cfg(test)]
mod edit_flow_tests {
    use super::*;
    use crate::app::StudioApp;

    fn editor_app() -> (StudioApp, egui::Context) {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let app = StudioApp::new(&cc, crate::cli::Cli { screen: Some(crate::cli::Screen::Editor), fresh: true, ..Default::default() });
        (app, ctx)
    }

    fn two_cards(app: &mut StudioApp) -> Vec<u32> {
        let mut s = crate::agent::Session::new(crate::services::project::new_custom("Flow", 320, 180, 24.0));
        s.edit(&[serde_json::json!({"op": "card", "colour": "#ff0000", "at": 0.0, "duration": 3.0}), serde_json::json!({"op": "card", "colour": "#00ff00", "at": 3.0, "duration": 3.0})], "cards", false).unwrap();
        app.doc = s.doc;
        app.doc.project.clips.iter().map(|c| c.id).collect()
    }

    fn said(ctx: &egui::Context) -> Vec<String> {
        kit::recent(ctx).into_iter().map(|t| t.text).collect()
    }

    #[test]
    fn keys_with_nothing_selected_say_so() {
        let _serial = crate::ui_tests::serial();
        let (mut app, ctx) = editor_app();
        two_cards(&mut app);
        app.editor.selection.clear();
        for (action, text) in [(Action::Delete, "Select a clip to delete"), (Action::Duplicate, "Select a clip to duplicate")] {
            run(&mut app, &ctx, action);
            assert!(said(&ctx).iter().any(|t| t == text), "{text}: {:?}", said(&ctx));
        }
        clip_menu::copy_selection(&mut app, &ctx, false);
        clip_menu::paste_selection(&mut app, &ctx);
        let all = said(&ctx);
        assert!(all.iter().any(|t| t == "Select a clip to copy") && all.iter().any(|t| t.starts_with("Nothing to paste")), "{all:?}");
    }

    #[test]
    fn marker_and_range_edits_say_what_they_did() {
        let _serial = crate::ui_tests::serial();
        let (mut app, ctx) = editor_app();
        two_cards(&mut app);
        app.editor.playhead = SEC;
        run(&mut app, &ctx, Action::Marker);
        run(&mut app, &ctx, Action::RangeIn);
        run(&mut app, &ctx, Action::RangeOut);
        let all = said(&ctx).join("|");
        assert!(all.contains("Marker added at") && all.contains("In point set at"), "{all}");
        // Out at the in point is refused by the model's minimum range: it still answers.
        assert!(all.contains("Out point set at") || all.contains("Nothing changed"), "{all}");
    }

    #[test]
    fn split_away_from_the_clip_says_why() {
        let _serial = crate::ui_tests::serial();
        let (mut app, ctx) = editor_app();
        let ids = two_cards(&mut app);
        app.editor.selection = vec![ids[0]];
        app.editor.playhead = 5 * SEC;
        let before = app.doc.project.clips.len();
        run(&mut app, &ctx, Action::Split);
        assert_eq!(app.doc.project.clips.len(), before);
        assert!(!said(&ctx).is_empty() || app.doc.error.is_some());
    }

    #[test]
    fn cut_says_cut_only_when_the_clips_are_gone_and_paste_brings_them_back() {
        let _serial = crate::ui_tests::serial();
        let (mut app, ctx) = editor_app();
        let ids = two_cards(&mut app);
        app.editor.selection = vec![ids[0]];
        let n = app.doc.project.clips.len();
        clip_menu::copy_selection(&mut app, &ctx, true);
        assert_eq!(app.doc.project.clips.len(), n - 1);
        let all = said(&ctx);
        assert!(all.iter().any(|t| t == "Clip cut") && !all.iter().any(|t| t.starts_with("Copied")), "{all:?}");
        assert_eq!(kit::recent(&ctx)[0].action.as_deref(), Some("Undo"));
        assert_eq!(app.editor.undo_toast, Some(kit::recent(&ctx)[0].id), "the Undo button belongs to this toast");
        app.editor.playhead = 0;
        clip_menu::paste_selection(&mut app, &ctx);
        assert_eq!(app.doc.project.clips.len(), n);
        assert!(said(&ctx).iter().any(|t| t == "Pasted clip"));
    }

    #[test]
    fn escape_clears_a_selected_cut_too() {
        let _serial = crate::ui_tests::serial();
        let (mut app, _ctx) = editor_app();
        let ids = two_cards(&mut app);
        app.editor.selection = vec![ids[0]];
        app.editor.selected_cut = Some(7);
        escape(&mut app.editor, &mut app.doc, false);
        assert!(app.editor.selection.is_empty() && app.editor.selected_cut.is_none());
    }

    #[test]
    fn one_helper_gives_every_edit_toast_a_working_undo() {
        let _serial = crate::ui_tests::serial();
        let (mut app, ctx) = editor_app();
        let ids = two_cards(&mut app);
        app.editor.selection = vec![ids[0]];
        run(&mut app, &ctx, Action::Duplicate);
        let t = &kit::recent(&ctx)[0];
        assert_eq!((t.action.as_deref(), Some(t.id)), (Some("Undo"), app.editor.undo_toast));
        assert!(app.doc.undo().is_some());
        assert_eq!(app.doc.project.clips.len(), ids.len());
    }

    #[test]
    fn stepping_a_frame_pauses_and_stays_inside() {
        let _serial = crate::ui_tests::serial();
        let (mut app, _ctx) = editor_app();
        two_cards(&mut app);
        let end = crate::services::transport::duration(&app.doc);
        assert_eq!(crate::services::transport::step(&app.doc, end, 1), end);
        assert_eq!(crate::services::transport::step(&app.doc, 0, -1), 0);
    }
}
