// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The Paint workspace (⇧⌘P): raster painting and photo editing on the
//! pictures the AI editor also works on (see [`crate::paint`]).
//!
//! Tools on the left (move, marquee, ellipse, lasso, magic wand, AI object
//! select, crop, brush, pencil, eraser, mixer, smudge, blur, dodge, burn,
//! healing, spot healing, clone, remove, bucket, gradient, eyedropper,
//! type, hand, zoom), their options above the canvas, and on the right the
//! layers, adjustments, AI selections, animation frames and history. A pen
//! tablet's pressure and tilt reach the brushes. Every action runs as a
//! picture op on a worker thread, so the window never waits on a filter
//! or a model; the canvas redraws when the picture changes, also when the
//! AI changed it.

use crate::app::StudioApp;
use crate::cli::Screen;
use crate::theme;
use eframe::egui::{self, Color32, ColorImage, CornerRadius, Pos2, Rect, Sense, Stroke, TextureHandle, TextureOptions, Vec2};
use serde_json::{Value, json};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::mpsc::{Receiver, channel};
use crate::kit::{self, ShowNoted};

/// The tools, in toolbar order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tool {
    Move,
    Marquee,
    Ellipse,
    Lasso,
    Wand,
    AiObject,
    Crop,
    Brush,
    Pencil,
    Eraser,
    Mixer,
    Smudge,
    Blur,
    Dodge,
    Burn,
    Heal,
    SpotHeal,
    Clone,
    Remove,
    Bucket,
    Gradient,
    Eyedropper,
    Text,
    Hand,
    Zoom,
}

impl Tool {
    pub const ALL: [Tool; 25] = [
        Tool::Move,
        Tool::Marquee,
        Tool::Ellipse,
        Tool::Lasso,
        Tool::Wand,
        Tool::AiObject,
        Tool::Crop,
        Tool::Brush,
        Tool::Pencil,
        Tool::Eraser,
        Tool::Mixer,
        Tool::Smudge,
        Tool::Blur,
        Tool::Dodge,
        Tool::Burn,
        Tool::Heal,
        Tool::SpotHeal,
        Tool::Clone,
        Tool::Remove,
        Tool::Bucket,
        Tool::Gradient,
        Tool::Eyedropper,
        Tool::Text,
        Tool::Hand,
        Tool::Zoom,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Tool::Move => "Move",
            Tool::Marquee => "Rectangle select",
            Tool::Ellipse => "Ellipse select",
            Tool::Lasso => "Lasso",
            Tool::Wand => "Magic wand",
            Tool::AiObject => "Select object (AI)",
            Tool::Crop => "Crop",
            Tool::Brush => "Brush",
            Tool::Pencil => "Pencil",
            Tool::Eraser => "Eraser",
            Tool::Mixer => "Mixer brush (wet paint)",
            Tool::Smudge => "Smudge",
            Tool::Blur => "Blur",
            Tool::Dodge => "Dodge",
            Tool::Burn => "Burn",
            Tool::Heal => "Healing brush",
            Tool::SpotHeal => "Spot healing",
            Tool::Clone => "Clone stamp (⌥-click the source)",
            Tool::Remove => "Remove",
            Tool::Bucket => "Paint bucket",
            Tool::Gradient => "Gradient",
            Tool::Eyedropper => "Eyedropper",
            Tool::Text => "Type",
            Tool::Hand => "Hand",
            Tool::Zoom => "Zoom (⌥ to zoom out)",
        }
    }

    pub fn key(self) -> Option<egui::Key> {
        use egui::Key::*;
        Some(match self {
            Tool::Move => V,
            Tool::Marquee => M,
            Tool::Lasso => L,
            Tool::Wand => W,
            Tool::Crop => C,
            Tool::Brush => B,
            Tool::Pencil => N,
            Tool::Eraser => E,
            Tool::Mixer => K,
            Tool::Smudge => R,
            Tool::Dodge => O,
            Tool::Heal => J,
            Tool::Clone => S,
            Tool::Bucket => G,
            Tool::Eyedropper => I,
            Tool::Text => T,
            Tool::Hand => H,
            Tool::Zoom => Z,
            _ => return None,
        })
    }

    fn icon(self) -> &'static str {
        match self {
            Tool::Move => "mouse-pointer-2",
            Tool::Marquee => "square-dashed",
            Tool::Ellipse => "circle-dashed",
            Tool::Lasso => "lasso",
            Tool::Wand => "wand",
            Tool::AiObject => "sparkles",
            Tool::Crop => "crop",
            Tool::Brush => "brush",
            Tool::Pencil => "pencil",
            Tool::Eraser => "eraser",
            Tool::Mixer => "palette",
            Tool::Smudge | Tool::Blur => "droplet",
            Tool::Dodge => "sun",
            Tool::Burn => "moon",
            Tool::Heal => "bandage",
            Tool::SpotHeal => "bandage-sparkle",
            Tool::Clone => "stamp",
            Tool::Remove => "wand-sparkles",
            Tool::Bucket => "paint-bucket",
            Tool::Gradient => "gradient",
            Tool::Eyedropper => "pipette",
            Tool::Text => "type",
            Tool::Hand => "hand",
            Tool::Zoom => "zoom-in",
        }
    }

    /// Tools that paint along a drag.
    fn strokes(self) -> bool {
        matches!(self, Tool::Brush | Tool::Pencil | Tool::Eraser | Tool::Mixer | Tool::Smudge | Tool::Blur | Tool::Dodge | Tool::Burn | Tool::Heal | Tool::SpotHeal | Tool::Clone | Tool::Remove)
    }

    fn paint_tool(self) -> &'static str {
        match self {
            Tool::Pencil => "pixel",
            Tool::Eraser => "eraser",
            Tool::Mixer => "mixer",
            Tool::Smudge => "smudge",
            Tool::Blur => "blur",
            Tool::Dodge => "dodge",
            Tool::Burn => "burn",
            Tool::Heal => "heal",
            Tool::SpotHeal => "spot_heal",
            Tool::Clone => "clone",
            Tool::Remove => "remove",
            _ => "brush",
        }
    }
}

const ICONS: &[(&str, &str)] = &[
    ("mouse-pointer-2", include_str!("../../assets/paint-icons/mouse-pointer-2.svg")),
    ("square-dashed", include_str!("../../assets/paint-icons/square-dashed.svg")),
    ("circle-dashed", include_str!("../../assets/paint-icons/circle-dashed.svg")),
    ("lasso", include_str!("../../assets/paint-icons/lasso.svg")),
    ("wand", include_str!("../../assets/paint-icons/wand.svg")),
    ("sparkles", include_str!("../../assets/paint-icons/sparkles.svg")),
    ("crop", include_str!("../../assets/paint-icons/crop.svg")),
    ("brush", include_str!("../../assets/paint-icons/brush.svg")),
    ("pencil", include_str!("../../assets/paint-icons/pencil.svg")),
    ("eraser", include_str!("../../assets/paint-icons/eraser.svg")),
    ("palette", include_str!("../../assets/paint-icons/palette.svg")),
    ("droplet", include_str!("../../assets/paint-icons/droplet.svg")),
    ("sun", include_str!("../../assets/paint-icons/sun.svg")),
    ("moon", include_str!("../../assets/paint-icons/moon.svg")),
    ("bandage", include_str!("../../assets/paint-icons/bandage.svg")),
    ("bandage-sparkle", include_str!("../../assets/paint-icons/bandage-sparkle.svg")),
    ("stamp", include_str!("../../assets/paint-icons/stamp.svg")),
    ("wand-sparkles", include_str!("../../assets/paint-icons/wand-sparkles.svg")),
    ("paint-bucket", include_str!("../../assets/paint-icons/paint-bucket.svg")),
    ("gradient", include_str!("../../assets/paint-icons/gradient.svg")),
    ("pipette", include_str!("../../assets/paint-icons/pipette.svg")),
    ("type", include_str!("../../assets/paint-icons/type.svg")),
    ("hand", include_str!("../../assets/paint-icons/hand.svg")),
    ("zoom-in", include_str!("../../assets/paint-icons/zoom-in.svg")),
    ("layers", include_str!("../../assets/paint-icons/layers.svg")),
    ("layer-mask", include_str!("../../assets/paint-icons/layer-mask.svg")),
    ("eye", include_str!("../../assets/paint-icons/eye.svg")),
    ("eye-off", include_str!("../../assets/paint-icons/eye-off.svg")),
    ("trash", include_str!("../../assets/paint-icons/trash.svg")),
    ("copy", include_str!("../../assets/paint-icons/copy.svg")),
    ("plus", include_str!("../../assets/paint-icons/plus.svg")),
    ("folder-plus", include_str!("../../assets/paint-icons/folder-plus.svg")),
    ("undo-2", include_str!("../../assets/paint-icons/undo-2.svg")),
    ("redo-2", include_str!("../../assets/paint-icons/redo-2.svg")),
    ("adjustment-layer", include_str!("../../assets/paint-icons/adjustment-layer.svg")),
    ("clip-below", include_str!("../../assets/paint-icons/clip-below.svg")),
    ("lock", include_str!("../../assets/paint-icons/lock.svg")),
];

/// One action waiting on the worker.
struct Job {
    label: String,
    rx: Receiver<Result<Value, String>>,
    /// What to do with the result on the UI thread.
    then: Option<Then>,
}

enum Then {
    /// Read a colour sample into the foreground.
    Pick,
}

/// The workspace's state (UI thread only).
pub struct PaintState {
    pub tool: Tool,
    pub size: f32,
    pub hardness: f32,
    pub opacity: f32,
    pub flow: f32,
    pub color: Color32,
    pub back: Color32,
    pub feel: usize,
    pub symmetry: usize,
    pub tolerance: f32,
    pub text: String,
    pub zoom: f32,
    pub pan: Vec2,
    fit_pending: bool,
    texture: Option<TextureHandle>,
    tex_key: Option<(u64, u64, u64, String)>,
    icons: HashMap<(&'static str, u32), TextureHandle>,
    stroke: Vec<[f32; 3]>,
    drag_from: Option<Pos2>,
    clone_source: Option<[f32; 2]>,
    jobs: Vec<Job>,
    pub status: String,
    palette_open: bool,
    palette_query: String,
    palette_params: String,
    palette_pick: Option<String>,
    adjust_params: String,
    new_size: [u32; 2],
    last_picture: Option<String>,
    pen: Option<studio_pixel_tablet::Sample>,
}

impl Default for PaintState {
    fn default() -> Self {
        PaintState {
            tool: Tool::Brush,
            size: 24.0,
            hardness: 80.0,
            opacity: 100.0,
            flow: 100.0,
            color: Color32::from_rgb(0x1d, 0x1d, 0x1f),
            back: Color32::WHITE,
            feel: 0,
            symmetry: 0,
            tolerance: 32.0,
            text: "Text".into(),
            zoom: 1.0,
            pan: Vec2::ZERO,
            fit_pending: true,
            texture: None,
            tex_key: None,
            icons: HashMap::new(),
            stroke: Vec::new(),
            drag_from: None,
            clone_source: None,
            jobs: Vec::new(),
            status: String::new(),
            palette_open: false,
            palette_query: String::new(),
            palette_params: "{}".into(),
            palette_pick: None,
            adjust_params: "{}".into(),
            new_size: [1920, 1080],
            last_picture: None,
            pen: None,
        }
    }
}

thread_local! {
    static STATE: RefCell<PaintState> = RefCell::new(PaintState::default());
    static PEN: std::cell::Cell<Option<studio_pixel_tablet::Sample>> = const { std::cell::Cell::new(None) };
    #[cfg(target_os = "macos")]
    static MONITOR: RefCell<Option<studio_pixel_tablet::macos::Monitor>> = const { RefCell::new(None) };
}

/// Pen pressure and tilt from the tablet (macOS), installed once.
fn pen_sample() -> Option<studio_pixel_tablet::Sample> {
    #[cfg(target_os = "macos")]
    MONITOR.with(|m| {
        let mut m = m.borrow_mut();
        if m.is_none() {
            *m = studio_pixel_tablet::macos::Monitor::install(|s| PEN.with(|p| p.set(s))).ok();
        }
    });
    PEN.with(|p| p.get())
}

const FEELS: &[(&str, &str)] = &[
    ("Hard round", ""),
    ("Soft round", "soft"),
    ("Pencil", "pencil"),
    ("Ink pen", "ink"),
    ("Calligraphy", "calligraphy"),
    ("Airbrush", "airbrush"),
    ("Watercolour", "watercolour"),
    ("Wash", "wash"),
    ("Oil", "oil"),
    ("Gouache", "gouache"),
    ("Dry brush", "dry_brush"),
    ("Chalk", "chalk"),
    ("Charcoal", "charcoal"),
    ("Pastel", "pastel"),
    ("Crayon", "crayon"),
    ("Marker", "marker"),
    ("Spatter", "spatter"),
    ("Stipple", "stipple"),
    ("Grass", "grass"),
    ("Foliage", "foliage"),
];

const SYMMETRY: &[(&str, &str)] = &[("Off", ""), ("Mirror ↔", "vertical"), ("Mirror ↕", "horizontal"), ("Four ways", "dual"), ("Diagonal", "diagonal")];

const ADJUSTMENTS: &[(&str, &str)] = &[
    ("Levels", "levels"),
    ("Curves", "curves"),
    ("Exposure", "exposure"),
    ("Vibrance", "vibrance"),
    ("Hue/Saturation", "hue_saturation"),
    ("Colour balance", "colour_balance"),
    ("Brightness/Contrast", "brightness_contrast"),
    ("Black & white", "black_white"),
    ("Photo filter", "photo_filter"),
    ("Channel mixer", "channel_mixer"),
    ("Selective colour", "selective_colour"),
    ("Gradient map", "gradient_map"),
    ("Colour lookup", "colour_lookup"),
    ("Posterize", "posterize"),
    ("Threshold", "threshold"),
    ("Invert", "invert"),
];

fn hex(c: Color32) -> String {
    format!("#{:02x}{:02x}{:02x}", c.r(), c.g(), c.b())
}

/// Start ops on the worker.
fn run(st: &mut PaintState, label: &str, ops: Vec<Value>) {
    run_then(st, label, ops, None);
}

fn run_then(st: &mut PaintState, label: &str, ops: Vec<Value>, then: Option<Then>) {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let r = crate::paint::apply(&ops);
        let _ = tx.send(r);
    });
    st.status = format!("{label}…");
    st.jobs.push(Job { label: label.to_owned(), rx, then });
}

fn poll_jobs(st: &mut PaintState, ctx: &egui::Context) {
    let mut done = Vec::new();
    st.jobs.retain(|j| match j.rx.try_recv() {
        Ok(r) => {
            done.push((j.label.clone(), r, j.then.as_ref().map(|t| matches!(t, Then::Pick))));
            false
        }
        Err(std::sync::mpsc::TryRecvError::Empty) => true,
        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
            done.push((j.label.clone(), Err("stopped".into()), None));
            false
        }
    });
    for (label, r, pick) in done {
        match r {
            Ok(v) => {
                if pick == Some(true) {
                    if let Some(p) = v["results"][0].as_array().filter(|a| a.len() >= 3) {
                        let c = |i: usize| (p[i].as_f64().unwrap_or(0.0).clamp(0.0, 1.0) * 255.0).round() as u8;
                        st.color = Color32::from_rgb(c(0), c(1), c(2));
                    }
                }
                st.status = format!("{label} ✓");
            }
            Err(e) => st.status = format!("{label}: {e}"),
        }
    }
    if !st.jobs.is_empty() {
        ctx.request_repaint_after(std::time::Duration::from_millis(30));
    }
}

fn icon(st: &mut PaintState, ctx: &egui::Context, name: &'static str, px: f32) -> Option<egui::TextureId> {
    let size = (px * ctx.pixels_per_point()).round().max(1.0) as u32;
    if let Some(t) = st.icons.get(&(name, size)) {
        return Some(t.id());
    }
    let svg = ICONS.iter().find(|(n, _)| *n == name)?.1.replace("currentColor", "#FFFFFF");
    let tree = resvg::usvg::Tree::from_str(&svg, &resvg::usvg::Options::default()).ok()?;
    let mut pm = resvg::tiny_skia::Pixmap::new(size, size)?;
    let k = size as f32 / tree.size().width();
    resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(k, k), &mut pm.as_mut());
    let img = ColorImage::from_rgba_premultiplied([size as usize, size as usize], pm.data());
    let t = ctx.load_texture(format!("paint-icon-{name}-{size}"), img, TextureOptions::LINEAR);
    let id = t.id();
    st.icons.insert((name, size), t);
    Some(id)
}

fn icon_button(st: &mut PaintState, ui: &mut egui::Ui, name: &'static str, tip: &str, on: bool) -> egui::Response {
    let t = theme::tokens(ui.ctx());
    let (r, resp) = ui.allocate_exact_size(Vec2::splat(32.0), Sense::click());
    let fill = if on { t.accent_soft } else if resp.hovered() { t.raised } else { Color32::TRANSPARENT };
    ui.painter().rect_filled(r, CornerRadius::same(6), fill);
    if let Some(id) = icon(st, ui.ctx(), name, 18.0) {
        let ir = Rect::from_center_size(r.center(), Vec2::splat(18.0));
        ui.painter().image(id, ir, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), if on { t.accent } else { t.text });
    }
    resp.on_hover_text(tip)
}

/// No background work pending (tests wait for this).
#[cfg(test)]
pub fn idle() -> bool {
    STATE.with(|s| s.borrow().jobs.is_empty())
}

/// Draw the workspace.
pub fn show(app: &mut StudioApp, ui: &mut egui::Ui) {
    let root = app.doc.folder.as_ref().map(|f| f.root().to_path_buf());
    STATE.with(|s| {
        let mut st = s.borrow_mut();
        workspace(app, &mut st, ui, root.as_deref());
    });
}

fn workspace(app: &mut StudioApp, st: &mut PaintState, ui: &mut egui::Ui, root: Option<&std::path::Path>) {
    let ctx = ui.ctx().clone();
    let t = theme::tokens(&ctx);
    poll_jobs(st, &ctx);
    st.pen = pen_sample();
    let full = ui.max_rect();
    ui.painter().rect_filled(full, 0, t.ground);
    let top = Rect::from_min_size(full.min, Vec2::new(full.width(), 44.0));
    let opts = Rect::from_min_size(Pos2::new(full.left(), top.bottom()), Vec2::new(full.width(), 40.0));
    let right_w = 280.0f32.min(full.width() * 0.35);
    let tools = Rect::from_min_max(Pos2::new(full.left(), opts.bottom()), Pos2::new(full.left() + 48.0, full.bottom() - 24.0));
    let side = Rect::from_min_max(Pos2::new(full.right() - right_w, opts.bottom()), Pos2::new(full.right(), full.bottom() - 24.0));
    let canvas = Rect::from_min_max(Pos2::new(tools.right(), opts.bottom()), Pos2::new(side.left(), full.bottom() - 24.0));
    let status = Rect::from_min_max(Pos2::new(full.left(), full.bottom() - 24.0), full.max);

    // Pictures go in the project's folder.
    if let Some(mut c) = crate::paint::try_canvas() {
        let dir = crate::paint::pictures_dir(root);
        crate::paint::use_dir(&mut c, dir.clone());
        if c.doc().is_none() && st.jobs.is_empty() {
            let _ = crate::paint::bring(&mut c, &dir, None);
        }
    }

    top_bar(app, st, ui, top);
    options_bar(st, ui, opts);
    tool_rail(st, ui, tools);
    canvas_view(st, ui, canvas);
    side_panel(st, ui, side);
    status_bar(st, ui, status);
    if st.palette_open {
        palette(st, &ctx);
    }
    shortcuts(st, &ctx);
}

fn top_bar(app: &mut StudioApp, st: &mut PaintState, ui: &mut egui::Ui, r: Rect) {
    let t = theme::tokens(ui.ctx());
    ui.painter().rect_filled(r, 0, t.panel);
    crate::screens::start::window_drag_area(ui, r);
    let inner = Rect::from_min_max(Pos2::new(r.left() + 78.0, r.top()), Pos2::new(r.right() - 8.0, r.bottom()));
    let mut outer = ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(egui::Layout::left_to_right(egui::Align::Center)));
    // One line: when the pictures and buttons are wider than the window
    // the row scrolls sideways.
    kit::scroll_strip(&mut outer, "paint-top", |bar| top_bar_row(app, st, bar));
}

fn top_bar_row(app: &mut StudioApp, st: &mut PaintState, bar: &mut egui::Ui) {
    bar.spacing_mut().item_spacing.x = 6.0;
    if bar.button("◀ Edit").on_hover_text("Back to the timeline (⇧⌘P)").clicked() {
        app.screen = Screen::Editor;
    }
    bar.separator();
    let (names, active) = match crate::paint::try_canvas() {
        Some(c) => (c.names(), c.doc().map(|d| d.name.clone())),
        None => (Vec::new(), st.last_picture.clone()),
    };
    if active.is_some() {
        st.last_picture = active.clone();
    }
    for n in &names {
        let on = Some(n) == active.as_ref();
        if bar.selectable_label(on, studio_canvas::stem(n)).clicked() && !on {
            run(st, "Switch", vec![json!({"op": "picture", "name": n})]);
            st.fit_pending = true;
        }
    }
    let new = bar.button("+ New");
    let pop = egui::Id::new("paint-new");
    if new.clicked() {
        egui::Popup::toggle_id(bar.ctx(), pop);
    }
    egui::Popup::from_toggle_button_response(&new).id(pop).show(|ui| kit::popup_body(ui, |ui| {
        ui.horizontal(|ui| {
            paint_dim(ui, "Width", &mut st.new_size[0]);
            ui.label("×");
            paint_dim(ui, "Height", &mut st.new_size[1]);
        });
        for (label, bg) in [("White", "white"), ("Transparent", "transparent"), ("Black", "black")] {
            if ui.button(format!("New picture · {label}")).clicked() {
                let [w, h] = st.new_size;
                run(st, "New picture", vec![json!({"op": "new", "name": "Painting", "width": w, "height": h, "background": bg})]);
                st.fit_pending = true;
            }
        }
        if ui.button("New animation (12 drawings, 12 fps on twos)").clicked() {
            let [w, h] = st.new_size;
            run(st, "New animation", vec![json!({"op": "new", "name": "Animation", "width": w, "height": h, "background": "white"}), json!({"op": "animate", "drawings": 12, "fps": 12, "hold": 2})]);
            st.fit_pending = true;
        }
    }));
    if bar.button("Open…").on_hover_text("PSD, TIFF, PNG, JPEG, WebP, HEIC, EXR, camera raw… (the file is only read)").clicked() {
        if let Some(p) = rfd::FileDialog::new().add_filter("Pictures", studio_canvas::files::READS).pick_file() {
            run(st, "Open", vec![json!({"op": "open", "path": p.display().to_string()})]);
            st.fit_pending = true;
        }
    }
    if bar.button("Save").on_hover_text("Into the project's Pictures folder (⌘S)").clicked() {
        run(st, "Save", vec![json!({"op": "save"})]);
    }
    if bar.button("Export…").clicked() {
        if let Some(p) = rfd::FileDialog::new().add_filter("Pictures", studio_canvas::files::WRITES).set_file_name(format!("{}.psd", st.last_picture.as_deref().map(studio_canvas::stem).unwrap_or("Painting"))).save_file() {
            run(st, "Export", vec![json!({"op": "export", "path": p.display().to_string()})]);
        }
    }
    if bar.button("Place on timeline").on_hover_text("Saves the picture and puts it (or the animation) on the timeline at the playhead; again after painting more updates it").clicked() {
        place_on_timeline(app, st);
    }
    bar.separator();
    if icon_button(st, bar, "undo-2", "Undo (⌘Z)", false).clicked() {
        run(st, "Undo", vec![json!({"op": "undo"})]);
    }
    if icon_button(st, bar, "redo-2", "Redo (⇧⌘Z)", false).clicked() {
        run(st, "Redo", vec![json!({"op": "redo"})]);
    }
    if bar.button("Commands…").on_hover_text("Every command: filters, liquify, puppet warp, HDR merge, panorama, focus stacking, camera raw… (⌘K)").clicked() {
        st.palette_open = true;
    }
}

/// Save the picture and lay it on the timeline through the agent's op, so
/// the window and the AI place pictures the same way.
fn place_on_timeline(app: &mut StudioApp, st: &mut PaintState) {
    let at = app.editor.playhead as f64 / crate::view::SEC as f64;
    let placeholder = crate::doc::Doc::new(app.doc.model.clone());
    let mut s = crate::agent::Session::new(std::mem::replace(&mut app.doc, placeholder));
    s.home = s.doc.folder.as_ref().map(|f| f.root().to_path_buf());
    let r = s.call("picture", &json!({"ops": [{"op": "to_timeline", "at": at}]}));
    app.doc = s.doc;
    st.status = match r {
        Ok(_) => "Placed on the timeline ✓".into(),
        Err(e) => format!("Place: {e}"),
    };
}

fn options_bar(st: &mut PaintState, ui: &mut egui::Ui, r: Rect) {
    let t = theme::tokens(ui.ctx());
    ui.painter().rect_filled(r, 0, t.sunken);
    let mut outer = ui.new_child(egui::UiBuilder::new().max_rect(r.shrink2(Vec2::new(10.0, 4.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
    // One line: a tool with more options than the window is wide scrolls
    // sideways.
    kit::scroll_strip(&mut outer, "paint-options", |bar| options_row(st, bar));
}

fn options_row(st: &mut PaintState, bar: &mut egui::Ui) {
    bar.spacing_mut().item_spacing.x = 8.0;
    bar.label(egui::RichText::new(st.tool.name()).strong());
    bar.separator();
    egui::color_picker::color_edit_button_srgba(bar, &mut st.color, egui::color_picker::Alpha::Opaque).on_hover_text("Foreground (X swaps)");
    egui::color_picker::color_edit_button_srgba(bar, &mut st.back, egui::color_picker::Alpha::Opaque).on_hover_text("Background");
    if st.tool.strokes() || matches!(st.tool, Tool::Eraser) {
        bar.label("Size");
        kit::Slider::new("Size", &mut st.size, 1.0..=1000.0).default(20.0).unit("px").logarithmic(true).with_value().width(160.0).show(bar);
        if matches!(st.tool, Tool::Brush | Tool::Eraser | Tool::Pencil | Tool::Heal | Tool::Clone | Tool::Dodge | Tool::Burn | Tool::Smudge | Tool::Blur) {
            bar.label("Hardness");
            kit::Slider::new("Hardness", &mut st.hardness, 0.0..=100.0).default(100.0).unit("%").with_value().width(140.0).show(bar);
        }
        bar.label("Opacity");
        kit::Slider::new("Opacity", &mut st.opacity, 1.0..=100.0).default(100.0).unit("%").with_value().width(140.0).show(bar);
        if matches!(st.tool, Tool::Brush | Tool::Mixer | Tool::Eraser) {
            bar.label("Flow");
            kit::Slider::new("Flow", &mut st.flow, 1.0..=100.0).default(100.0).unit("%").with_value().width(140.0).show(bar);
        }
        if matches!(st.tool, Tool::Brush | Tool::Mixer | Tool::Eraser) {
            egui::ComboBox::from_id_salt("paint-feel").selected_text(FEELS[st.feel].0).show_ui(bar, |ui| {
                crate::kit::note_list(ui);
                for (i, (n, _)) in FEELS.iter().enumerate() {
                    ui.selectable_value(&mut st.feel, i, *n);
                }
            });
        }
        let before = st.symmetry;
        egui::ComboBox::from_id_salt("paint-sym").selected_text(format!("Symmetry: {}", SYMMETRY[st.symmetry].0)).show_ui(bar, |ui| {
            crate::kit::note_list(ui);
            for (i, (n, _)) in SYMMETRY.iter().enumerate() {
                ui.selectable_value(&mut st.symmetry, i, *n);
            }
        });
        if before != st.symmetry {
            let m = SYMMETRY[st.symmetry].1;
            run(st, "Symmetry", vec![if m.is_empty() { json!({"op": "paint.symmetryDisable"}) } else { json!({"op": "paint.setSymmetry", "mode": m}) }]);
        }
        if st.tool == Tool::Clone {
            bar.label(match st.clone_source {
                Some([x, y]) => format!("Source {x:.0}, {y:.0}"),
                None => "⌥-click to set the source".into(),
            });
        }
        if let Some(p) = st.pen {
            bar.label(format!("Pen {:.0}%", p.pressure * 100.0));
        }
    }
    if matches!(st.tool, Tool::Wand | Tool::Bucket) {
        bar.label("Tolerance");
        kit::Slider::new("Tolerance", &mut st.tolerance, 0.0..=255.0).default(32.0).with_value().width(140.0).show(bar);
    }
    if st.tool == Tool::Text {
        bar.label("Text");
        bar.add(egui::TextEdit::singleline(&mut st.text).desired_width(240.0));
        bar.label("Size");
        kit::Slider::new("Size", &mut st.size, 4.0..=400.0).default(48.0).unit("pt").with_value().width(160.0).show(bar);
    }
    if matches!(st.tool, Tool::Marquee | Tool::Ellipse | Tool::Lasso | Tool::Wand | Tool::AiObject) {
        bar.label("⇧ adds · ⌥ subtracts");
        if bar.button("Refine edge").on_hover_text("Soft, hair-level edges").clicked() {
            run(st, "Refine edge", vec![json!({"op": "select.refineEdge", "radius": 6, "smartRadius": true})]);
        }
        if bar.button("Feather 4 px").clicked() {
            run(st, "Feather", vec![json!({"op": "select.modify.feather", "radius": 4})]);
        }
    }
}

fn tool_rail(st: &mut PaintState, ui: &mut egui::Ui, r: Rect) {
    let t = theme::tokens(ui.ctx());
    ui.painter().rect_filled(r, 0, t.panel);
    let mut col = ui.new_child(egui::UiBuilder::new().max_rect(r.shrink(8.0)).layout(egui::Layout::top_down(egui::Align::Center)));
    col.spacing_mut().item_spacing.y = 2.0;
    egui::ScrollArea::vertical().id_salt("paint-tools").show_v(&mut col, |ui| {
        for tool in Tool::ALL {
            let key = tool.key().map(|k| format!(" ({})", k.name())).unwrap_or_default();
            if icon_button(st, ui, tool.icon(), &format!("{}{key}", tool.name()), st.tool == tool).clicked() {
                st.tool = tool;
                st.stroke.clear();
            }
        }
    });
}

/// Screen position of document point (x, y), and back.
struct View {
    origin: Pos2,
    zoom: f32,
}

impl View {
    fn to_screen(&self, x: f32, y: f32) -> Pos2 {
        Pos2::new(self.origin.x + x * self.zoom, self.origin.y + y * self.zoom)
    }
    fn to_doc(&self, p: Pos2) -> [f32; 2] {
        [(p.x - self.origin.x) / self.zoom, (p.y - self.origin.y) / self.zoom]
    }
}

fn canvas_view(st: &mut PaintState, ui: &mut egui::Ui, r: Rect) {
    let t = theme::tokens(ui.ctx());
    let ctx = ui.ctx().clone();
    ui.painter().rect_filled(r, 0, if t.dark { Color32::from_gray(38) } else { Color32::from_gray(200) });
    let resp = ui.allocate_rect(r, Sense::click_and_drag());
    let Some(c) = crate::paint::try_canvas() else {
        ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, "Working…", egui::FontId::proportional(15.0), t.text_2);
        ctx.request_repaint_after(std::time::Duration::from_millis(40));
        return;
    };
    let Some(doc) = c.doc() else {
        ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, "No picture open: + New, Open…, or ask the AI to paint one", egui::FontId::proportional(15.0), t.text_2);
        return;
    };
    let (dw, dh) = (doc.size.width as f32, doc.size.height as f32);
    if st.fit_pending {
        st.zoom = ((r.width() - 40.0) / dw).min((r.height() - 40.0) / dh).clamp(0.01, 8.0);
        st.pan = Vec2::ZERO;
        st.fit_pending = false;
    }
    // Zoom with the wheel / pinch about the pointer; pan with two fingers or the hand.
    if resp.hovered() {
        let (scroll, zoom_d, hover) = ctx.input(|i| (i.smooth_scroll_delta, i.zoom_delta(), i.pointer.hover_pos()));
        if zoom_d != 1.0 {
            if let Some(h) = hover {
                let before = (h - r.center() - st.pan) / st.zoom;
                st.zoom = (st.zoom * zoom_d).clamp(0.01, 64.0);
                st.pan = h - r.center() - before * st.zoom;
            }
        } else if scroll != Vec2::ZERO {
            st.pan += scroll;
        }
    }
    let origin = r.center() + st.pan - Vec2::new(dw, dh) * st.zoom / 2.0;
    let view = View { origin, zoom: st.zoom };
    let doc_rect = Rect::from_min_size(origin, Vec2::new(dw, dh) * st.zoom);
    // Checkerboard under transparency.
    let painter = ui.painter_at(r);
    painter.rect_filled(doc_rect, 0, Color32::from_gray(250));
    let cell = 12.0;
    let vis = doc_rect.intersect(r);
    if vis.is_positive() {
        let mut y = vis.top();
        while y < vis.bottom() {
            let mut x = vis.left() + (((y - doc_rect.top()) / cell) as i32 % 2) as f32 * cell;
            while x < vis.right() {
                painter.rect_filled(Rect::from_min_size(Pos2::new(x, y), Vec2::splat(cell)).intersect(vis), 0, Color32::from_gray(222));
                x += cell * 2.0;
            }
            y += cell;
        }
    }
    // The visible part of the picture at screen resolution.
    let ppp = ctx.pixels_per_point();
    let fb = c.meta().and_then(|m| m.flipbook.clone());
    if vis.is_positive() {
        let a = view.to_doc(vis.min);
        let b = view.to_doc(vis.max);
        let (x0, y0) = (a[0].floor().max(0.0) as i32, a[1].floor().max(0.0) as i32);
        let (x1, y1) = (b[0].ceil().min(dw) as i32, b[1].ceil().min(dh) as i32);
        let (rw, rh) = ((x1 - x0).max(1) as u32, (y1 - y0).max(1) as u32);
        let scale = (st.zoom * ppp).min(1.0);
        let (ow, oh) = (((rw as f32 * scale).round() as u32).max(1), ((rh as f32 * scale).round() as u32).max(1));
        let st_doc = c.engine.active().map(|d| (d.doc.id.0, d.revision, d.view_revision)).unwrap_or_default();
        let key = (st_doc.0, st_doc.1, st_doc.2, format!("{x0},{y0},{rw},{rh},{ow},{oh},{:?}", fb.as_ref().map(|f| (f.current, f.onion.on, f.onion.before, f.onion.after))));
        if st.tex_key.as_ref() != Some(&key) {
            let img = match &fb {
                // Animations show the onion skin over the whole drawing.
                Some(f) => f.onion_view(doc, ((dw.max(dh) * scale).round() as u32).clamp(16, 4096)),
                // At 1:1 or closer the GPU composites what is on screen;
                // further out the CPU reduces the whole area.
                None => gpu_view(&c, (x0, y0, rw, rh), scale >= 1.0).unwrap_or_else(|| c.view((x0, y0, rw, rh), ow, oh).unwrap_or_else(|_| studio_canvas::Rgba8Image::new(1, 1))),
            };
            let ci = ColorImage::from_rgba_unmultiplied([img.width as usize, img.height as usize], &img.pixels);
            let opt = if st.zoom * ppp > 1.5 { TextureOptions::NEAREST } else { TextureOptions::LINEAR };
            match &mut st.texture {
                Some(tex) => tex.set(ci, opt),
                None => st.texture = Some(ctx.load_texture("paint-canvas", ci, opt)),
            }
            st.tex_key = Some(key);
        }
        if let Some(tex) = &st.texture {
            let shown = if fb.is_some() { doc_rect } else { Rect::from_min_max(view.to_screen(x0 as f32, y0 as f32), view.to_screen(x1 as f32, y1 as f32)) };
            painter.image(tex.id(), shown, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
        }
    }
    painter.rect_stroke(doc_rect, 0, Stroke::new(1.0, Color32::from_black_alpha(90)), egui::StrokeKind::Outside);
    // The selection's bounds, dashed.
    if let Some(sel) = doc.selection.as_ref() {
        let b = sel.content_bounds();
        if b.width() > 0 {
            let sr = Rect::from_min_max(view.to_screen(b.x0 as f32, b.y0 as f32), view.to_screen(b.x1 as f32, b.y1 as f32));
            let phase = (ctx.input(|i| i.time) * 8.0) as f32 % 8.0;
            dashed_rect(&painter, sr, phase);
            ctx.request_repaint_after(std::time::Duration::from_millis(120));
        }
    }
    drop(c);
    input(st, ui, &resp, &view, r);
    // What is being drawn right now.
    let col = if st.tool == Tool::Eraser { Color32::from_white_alpha(200) } else { st.color.gamma_multiply(st.opacity / 100.0) };
    if st.tool.strokes() && st.stroke.len() > 1 {
        let pts: Vec<Pos2> = st.stroke.iter().map(|p| view.to_screen(p[0], p[1])).collect();
        painter.add(egui::Shape::line(pts, Stroke::new((st.size * st.zoom).max(1.0), col)));
    }
    if let (Some(from), Some(now)) = (st.drag_from, resp.interact_pointer_pos()) {
        match st.tool {
            Tool::Marquee | Tool::Crop => dashed_rect(&painter, Rect::from_two_pos(from, now), 0.0),
            Tool::Ellipse => {
                let rr = Rect::from_two_pos(from, now);
                painter.add(egui::Shape::ellipse_stroke(rr.center(), rr.size() / 2.0, Stroke::new(1.0, Color32::WHITE)));
            }
            Tool::Gradient => {
                painter.line_segment([from, now], Stroke::new(2.0, Color32::WHITE));
            }
            _ => {}
        }
    }
    if st.tool == Tool::Lasso && st.stroke.len() > 1 {
        let pts: Vec<Pos2> = st.stroke.iter().map(|p| view.to_screen(p[0], p[1])).collect();
        painter.add(egui::Shape::line(pts, Stroke::new(1.0, Color32::WHITE)));
    }
    // The brush outline.
    if let Some(h) = resp.hover_pos() {
        if st.tool.strokes() {
            painter.circle_stroke(h, (st.size * st.zoom / 2.0).max(2.0), Stroke::new(1.0, Color32::from_white_alpha(200)));
            painter.circle_stroke(h, (st.size * st.zoom / 2.0).max(2.0) + 1.0, Stroke::new(1.0, Color32::from_black_alpha(120)));
        }
    }
}

/// The visible area composited on the GPU, when there is one and it is worth it.
fn gpu_view(c: &studio_canvas::Canvas, rect: (i32, i32, u32, u32), full_res: bool) -> Option<studio_canvas::Rgba8Image> {
    if !full_res {
        return None;
    }
    #[cfg(feature = "render")]
    {
        // The GPU compositor for the canvas, tried once (a static, not a
        // thread local: a device must not be dropped while threads unwind).
        static GPU: std::sync::Mutex<Option<Option<studio_canvas::gpu::GpuView>>> = std::sync::Mutex::new(None);
        let mut g = GPU.lock().unwrap_or_else(|p| p.into_inner());
        let view = g.get_or_insert_with(|| studio_render::gpu::GpuContext::new().and_then(|ctx| studio_canvas::gpu::GpuView::new(ctx.device.clone(), ctx.queue.clone())));
        view.as_mut().and_then(|v| v.render(c, rect).ok())
    }
    #[cfg(not(feature = "render"))]
    {
        let _ = (c, rect);
        None
    }
}

fn dashed_rect(p: &egui::Painter, r: Rect, phase: f32) {
    let pts = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()];
    for w in pts.windows(2) {
        p.line_segment([w[0], w[1]], Stroke::new(1.0, Color32::BLACK));
        p.extend(egui::Shape::dashed_line_with_offset(&[w[0], w[1]], Stroke::new(1.0, Color32::WHITE), &[4.0], &[4.0], phase));
    }
}

fn mode_of(m: egui::Modifiers) -> &'static str {
    if m.shift {
        "add"
    } else if m.alt {
        "subtract"
    } else {
        "replace"
    }
}

fn input(st: &mut PaintState, ui: &mut egui::Ui, resp: &egui::Response, view: &View, r: Rect) {
    let ctx = ui.ctx().clone();
    let mods = ctx.input(|i| i.modifiers);
    let space = ctx.input(|i| i.key_down(egui::Key::Space));
    let pan_tool = st.tool == Tool::Hand || space || resp.dragged_by(egui::PointerButton::Middle);
    if pan_tool {
        if resp.dragged() {
            st.pan += resp.drag_delta();
        }
        return;
    }
    let Some(pos) = resp.interact_pointer_pos() else { return };
    let d = view.to_doc(pos);
    let pressure = st.pen.map(|p| p.pressure.clamp(0.05, 1.0)).unwrap_or(1.0);
    if resp.drag_started() || (resp.is_pointer_button_down_on() && st.drag_from.is_none() && st.stroke.is_empty()) {
        st.drag_from = Some(pos);
        st.stroke.clear();
        if st.tool.strokes() || st.tool == Tool::Lasso {
            st.stroke.push([d[0], d[1], pressure]);
        }
    }
    if resp.dragged() && (st.tool.strokes() || st.tool == Tool::Lasso) {
        if st.stroke.last().is_none_or(|l| (l[0] - d[0]).hypot(l[1] - d[1]) * view.zoom > 1.0) {
            st.stroke.push([d[0], d[1], pressure]);
        }
        if st.tool == Tool::Move {
            st.pan += Vec2::ZERO;
        }
    }
    let released = resp.drag_stopped() || resp.clicked();
    if !released {
        return;
    }
    let from = st.drag_from.take().map(|p| view.to_doc(p)).unwrap_or(d);
    let stroke = std::mem::take(&mut st.stroke);
    let rect = |a: [f32; 2], b: [f32; 2]| [a[0].min(b[0]).round(), a[1].min(b[1]).round(), (a[0] - b[0]).abs().round().max(1.0), (a[1] - b[1]).abs().round().max(1.0)];
    match st.tool {
        Tool::Move => {
            let (dx, dy) = ((d[0] - from[0]).round(), (d[1] - from[1]).round());
            if dx != 0.0 || dy != 0.0 {
                run(st, "Move", vec![json!({"op": "layer.translate", "dx": dx, "dy": dy})]);
            }
        }
        Tool::Marquee | Tool::Ellipse => {
            if resp.clicked() {
                run(st, "Deselect", vec![json!({"op": "select", "what": "none"})]);
            } else {
                let what = if st.tool == Tool::Marquee { "rect" } else { "ellipse" };
                run(st, "Select", vec![json!({"op": "select", "what": what, "rect": rect(from, d), "mode": mode_of(mods)})]);
            }
        }
        Tool::Crop => {
            if !resp.clicked() {
                let [x, y, w, h] = rect(from, d);
                run(st, "Crop", vec![json!({"op": "image.crop", "x": x, "y": y, "width": w, "height": h})]);
                st.fit_pending = true;
            }
        }
        Tool::Lasso => {
            if stroke.len() > 2 {
                let pts: Vec<[f32; 2]> = stroke.iter().map(|p| [p[0], p[1]]).collect();
                run(st, "Lasso", vec![json!({"op": "select", "what": "lasso", "points": pts, "mode": mode_of(mods)})]);
            }
        }
        Tool::Wand => run(st, "Magic wand", vec![json!({"op": "select", "what": "wand", "point": [d[0].round(), d[1].round()], "tolerance": st.tolerance, "mode": mode_of(mods)})]),
        Tool::AiObject => {
            let op = if resp.clicked() { json!({"op": "select", "what": "object", "point": d, "mode": mode_of(mods)}) } else { json!({"op": "select", "what": "object", "box": [from[0].min(d[0]), from[1].min(d[1]), from[0].max(d[0]), from[1].max(d[1])], "mode": mode_of(mods)}) };
            run(st, "Select object", vec![op]);
        }
        Tool::Bucket => run(st, "Fill", vec![json!({"op": "paint.bucket", "x": d[0].round(), "y": d[1].round(), "tolerance": st.tolerance, "color": hex(st.color)})]),
        Tool::Gradient => {
            if !resp.clicked() {
                run(st, "Gradient", vec![json!({"op": "gradient", "from": from, "to": d, "colors": [hex(st.color), hex(st.back)]})]);
            }
        }
        Tool::Eyedropper => run_then(st, "Pick colour", vec![json!({"op": "document.pixel", "x": d[0].round(), "y": d[1].round()})], Some(Then::Pick)),
        Tool::Text => run(st, "Type", vec![json!({"op": "type.create", "x": d[0].round(), "y": d[1].round(), "text": st.text, "size": st.size, "color": hex(st.color)})]),
        Tool::Zoom => {
            let k = if mods.alt { 0.5 } else { 2.0 };
            let before = (pos - r.center() - st.pan) / st.zoom;
            st.zoom = (st.zoom * k).clamp(0.01, 64.0);
            st.pan = pos - r.center() - before * st.zoom;
        }
        Tool::Clone if mods.alt => {
            st.clone_source = Some(d);
            st.status = format!("Clone source {:.0}, {:.0}", d[0], d[1]);
        }
        tool if tool.strokes() => {
            let mut pts = stroke;
            if pts.is_empty() {
                pts.push([d[0], d[1], pressure]);
            }
            if pts.len() == 1 {
                // A click is a dab.
                let p = pts[0];
                pts.push([p[0] + 0.01, p[1], p[2]]);
            }
            let mut op = json!({"op": "paint", "tool": tool.paint_tool(), "points": pts, "size": st.size, "opacity": st.opacity, "flow": st.flow, "hardness": st.hardness, "color": hex(st.color)});
            if matches!(tool, Tool::Brush | Tool::Mixer | Tool::Eraser) && !FEELS[st.feel].1.is_empty() {
                op["feel"] = json!(FEELS[st.feel].1);
            }
            if matches!(tool, Tool::Clone | Tool::Heal) {
                match st.clone_source {
                    Some(s) => op["source"] = json!(s),
                    None => {
                        st.status = "⌥-click where to copy from first".into();
                        return;
                    }
                }
            }
            run(st, tool.name(), vec![op]);
        }
        _ => {}
    }
}

fn side_panel(st: &mut PaintState, ui: &mut egui::Ui, r: Rect) {
    let t = theme::tokens(ui.ctx());
    ui.painter().rect_filled(r, 0, t.panel);
    let mut col = ui.new_child(egui::UiBuilder::new().max_rect(r.shrink(10.0)).layout(egui::Layout::top_down(egui::Align::Min)));
    egui::ScrollArea::vertical().id_salt("paint-side").auto_shrink([false, false]).show_v(&mut col, |ui| {
        ui.set_width(ui.available_width());
        let snapshot = crate::paint::try_canvas().and_then(|c| c.summary().ok());
        let Some(sum) = snapshot else {
            ui.label("Working…");
            return;
        };
        egui::CollapsingHeader::new("AI selection").default_open(true).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                for (label, what) in [("Subject", "subject"), ("People", "people"), ("Sky", "sky"), ("Background", "background")] {
                    if ui.button(label).clicked() {
                        run(st, &format!("Select {label}"), vec![json!({"op": "select", "what": what})]);
                    }
                }
            });
            ui.horizontal_wrapped(|ui| {
                if ui.button("Remove it").on_hover_text("Fill the selection from its surroundings").clicked() {
                    run(st, "Remove", vec![json!({"op": "remove"})]);
                }
                if ui.button("Mask it").on_hover_text("The selection as the layer's mask").clicked() {
                    run(st, "Mask", vec![json!({"op": "layer", "action": "mask"})]);
                }
                if ui.button("Invert").clicked() {
                    run(st, "Invert", vec![json!({"op": "select", "what": "invert"})]);
                }
                if ui.button("Deselect").clicked() {
                    run(st, "Deselect", vec![json!({"op": "select", "what": "none"})]);
                }
            });
        });
        egui::CollapsingHeader::new("Layers").default_open(true).show(ui, |ui| layers(st, ui, &sum));
        egui::CollapsingHeader::new("Adjustments").default_open(false).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                for (label, kind) in ADJUSTMENTS {
                    if ui.small_button(*label).clicked() {
                        run(st, label, vec![json!({"op": "adjust", "kind": kind})]);
                    }
                }
            });
            ui.label(egui::RichText::new("Settings of the active adjustment layer (JSON, as `commands` lists for layer.setAdjustment):").small());
            ui.add(egui::TextEdit::multiline(&mut st.adjust_params).desired_rows(2).code_editor());
            if ui.button("Apply settings").clicked() {
                match serde_json::from_str::<Value>(&st.adjust_params) {
                    Ok(mut p) => {
                        p["op"] = json!("layer.setAdjustment");
                        run(st, "Adjust", vec![p]);
                    }
                    Err(e) => st.status = format!("Settings: {e}"),
                }
            }
            ui.horizontal_wrapped(|ui| {
                if ui.small_button("Camera raw…").clicked() {
                    st.palette_open = true;
                    st.palette_query = "camera raw".into();
                }
                if ui.small_button("Frequency separation").clicked() {
                    run(st, "Frequency separation", vec![json!({"op": "frequency_separation", "radius": 6})]);
                }
                if ui.small_button("Liquify…").clicked() {
                    st.palette_open = true;
                    st.palette_query = "liquify".into();
                }
            });
        });
        if let Some(fb) = sum.get("flipbook") {
            egui::CollapsingHeader::new("Animation").default_open(true).show(ui, |ui| animation(st, ui, fb));
        } else {
            egui::CollapsingHeader::new("Animation").default_open(false).show(ui, |ui| {
                ui.label("Frame-by-frame drawing with onion skin; or trace footage (ask the AI: “trace the dancer at 12 fps”).");
                if ui.button("Make this picture an animation").clicked() {
                    run(st, "Animate", vec![json!({"op": "animate", "drawings": 12, "fps": 12, "hold": 2})]);
                }
            });
        }
        egui::CollapsingHeader::new("History").default_open(false).show(ui, |ui| history(st, ui));
    });
}

fn layers(st: &mut PaintState, ui: &mut egui::Ui, sum: &Value) {
    ui.horizontal_wrapped(|ui| {
        if icon_button(st, ui, "plus", "New layer", false).clicked() {
            run(st, "New layer", vec![json!({"op": "layer", "action": "new"})]);
        }
        if icon_button(st, ui, "folder-plus", "Group", false).clicked() {
            run(st, "Group", vec![json!({"op": "layer", "action": "group"})]);
        }
        if icon_button(st, ui, "layer-mask", "Add a mask (from the selection)", false).clicked() {
            run(st, "Mask", vec![json!({"op": "layer", "action": "mask"})]);
        }
        if icon_button(st, ui, "adjustment-layer", "Curves adjustment layer", false).clicked() {
            run(st, "Curves", vec![json!({"op": "adjust", "kind": "curves"})]);
        }
        if icon_button(st, ui, "clip-below", "Clip to the layer below", false).clicked() {
            run(st, "Clip", vec![json!({"op": "layer", "action": "clip"})]);
        }
        if icon_button(st, ui, "copy", "Duplicate", false).clicked() {
            run(st, "Duplicate", vec![json!({"op": "layer", "action": "duplicate"})]);
        }
        if icon_button(st, ui, "trash", "Delete", false).clicked() {
            run(st, "Delete layer", vec![json!({"op": "layer", "action": "delete"})]);
        }
    });
    let empty = Vec::new();
    for l in sum["layers"].as_array().unwrap_or(&empty) {
        let id = l["id"].as_u64().unwrap_or(0);
        let active = l["active"].as_bool().unwrap_or(false);
        let hidden = l["hidden"].as_bool().unwrap_or(false);
        let depth = l["in_group"].as_u64().unwrap_or(0) as f32;
        ui.horizontal(|ui| {
            ui.add_space(depth * 12.0);
            if icon_button(st, ui, if hidden { "eye-off" } else { "eye" }, "Show / hide", false).clicked() {
                run(st, "Visibility", vec![json!({"op": "layer.setProps", "layer": id, "visible": hidden})]);
            }
            let mut label = l["name"].as_str().unwrap_or("").to_owned();
            if l["mask"].as_bool() == Some(true) {
                label.push_str("  ●");
            }
            if l["clipped"].as_bool() == Some(true) {
                label = format!("↩ {label}");
            }
            let kind = l["kind"].as_str().unwrap_or("");
            // A long name ends in an ellipsis; the hover shows it whole.
            let tip = format!("{label}\n{kind} · {} · {}%", l["blend"].as_str().unwrap_or(""), l["opacity"]);
            if ui.add(egui::Button::selectable(active, label).truncate()).on_hover_text(tip).clicked() && !active {
                run(st, "Select layer", vec![json!({"op": "layer.select", "layer": id})]);
            }
        });
        if active {
            ui.horizontal_wrapped(|ui| {
                ui.add_space(depth * 12.0 + 36.0);
                // The slider takes the room the blend menu and the value
                // leave; when even that is too little the menu wraps.
                let gap = ui.spacing().item_spacing.x;
                let left = ui.max_rect().right() - ui.cursor().left();
                let sw = (left - 96.0 - 56.0 - 3.0 * gap).clamp(40.0, 100.0);
                let mut o = l["opacity"].as_f64().unwrap_or(100.0) as f32;
                let sr = kit::Slider::new("Opacity", &mut o, 0.0..=100.0).default(100.0).unit("%").with_value().width(sw + 40.0).show(ui);
                if sr.drag_stopped() || sr.clicked() {
                    run(st, "Opacity", vec![json!({"op": "layer.setProps", "layer": id, "opacity": o / 100.0})]);
                }
                let blend = l["blend"].as_str().unwrap_or("Normal").to_owned();
                egui::ComboBox::from_id_salt(("blend", id)).selected_text(&blend).width(96.0).show_ui(ui, |ui| {
                    crate::kit::note_list(ui);
                    for b in ["Normal", "Multiply", "Screen", "Overlay", "SoftLight", "HardLight", "ColorDodge", "ColorBurn", "Darken", "Lighten", "Difference", "Exclusion", "Hue", "Saturation", "Color", "Luminosity", "LinearLight", "LinearDodge", "LinearBurn", "VividLight", "PinLight"] {
                        if ui.selectable_label(blend == b, b).clicked() {
                            run(st, "Blend", vec![json!({"op": "layer.setProps", "layer": id, "blend": b})]);
                        }
                    }
                });
            });
        }
    }
}

fn animation(st: &mut PaintState, ui: &mut egui::Ui, fb: &Value) {
    let n = fb["drawings"].as_u64().unwrap_or(0) as usize;
    let cur = fb["current"].as_u64().unwrap_or(1) as usize;
    ui.label(format!("{n} drawings · {} frames · {} fps · {}s", fb["frames"], fb["fps"], fb["seconds"]));
    ui.horizontal_wrapped(|ui| {
        for i in 1..=n {
            let hold = fb["holds"][i - 1].as_u64().unwrap_or(1);
            if ui.selectable_label(i == cur, format!("{i}{}", if hold > 1 { format!("×{hold}") } else { String::new() })).clicked() {
                run(st, "Drawing", vec![json!({"op": "frame", "go": i})]);
            }
        }
    });
    ui.horizontal_wrapped(|ui| {
        if ui.button("◀").clicked() {
            run(st, "Previous", vec![json!({"op": "frame", "action": "prev"})]);
        }
        if ui.button("▶").clicked() {
            run(st, "Next", vec![json!({"op": "frame", "action": "next"})]);
        }
        if ui.button("+ Blank").clicked() {
            run(st, "New drawing", vec![json!({"op": "frame", "action": "add"})]);
        }
        if ui.button("+ Copy").clicked() {
            run(st, "Copy drawing", vec![json!({"op": "frame", "action": "copy"})]);
        }
        if ui.button("Delete").clicked() {
            run(st, "Delete drawing", vec![json!({"op": "frame", "action": "delete"})]);
        }
    });
    ui.horizontal_wrapped(|ui| {
        ui.label("Hold");
        for h in [1u32, 2, 3, 4] {
            if ui.small_button(format!("{h}")).on_hover_text(format!("This drawing on {h}s")).clicked() {
                run(st, "Hold", vec![json!({"op": "frame", "action": "hold", "hold": h})]);
            }
        }
        let on = fb["onion"]["on"].as_bool().unwrap_or(true);
        if ui.selectable_label(on, "Onion skin").clicked() {
            run(st, "Onion skin", vec![json!({"op": "flipbook", "onion": !on})]);
        }
    });
}

fn history(st: &mut PaintState, ui: &mut egui::Ui) {
    let rows: Vec<String> = match crate::paint::try_canvas() {
        Some(c) => c.engine.active().map(|d| d.history.entries()).unwrap_or_default(),
        None => return,
    };
    if ui.button("Snapshot").on_hover_text("Keep this state to come back to").clicked() {
        run(st, "Snapshot", vec![json!({"op": "snapshot", "name": "Snapshot"})]);
    }
    for (i, label) in rows.iter().enumerate().rev().take(40) {
        let row = format!("{} {label}", i + 1);
        if ui.add(egui::Button::selectable(false, row.as_str()).truncate()).on_hover_text(&row).clicked() {
            run(st, "History", vec![json!({"op": "history.selectState", "state": i})]);
        }
    }
}

fn status_bar(st: &mut PaintState, ui: &mut egui::Ui, r: Rect) {
    let t = theme::tokens(ui.ctx());
    ui.painter().rect_filled(r, 0, t.panel);
    let info = crate::paint::try_canvas().and_then(|c| c.doc().map(|d| format!("{} · {}×{} · {:?} · {:?}", studio_canvas::stem(&d.name), d.size.width, d.size.height, d.mode, d.depth))).unwrap_or_default();
    let text = format!("{info}   {:.0}%   {}", st.zoom * 100.0, st.status);
    // One line: a long status ends in an ellipsis and shows whole on hover.
    let cut = kit::paint_text_clipped(ui.painter(), Pos2::new(r.left() + 10.0, r.center().y), egui::Align2::LEFT_CENTER, &text, egui::FontId::proportional(12.0), t.text_2, r.width() - 20.0);
    if cut {
        ui.interact(r, egui::Id::unique("paint-status"), Sense::hover()).on_hover_text(text);
    }
}

/// Every engine command, searchable, with its parameters.
fn palette(st: &mut PaintState, ctx: &egui::Context) {
    let mut open = st.palette_open;
    egui::Window::new("Commands").open(&mut open).default_width(520.0).default_height(460.0).collapsible(false).show(ctx, |ui| kit::scroll_panel(ui, "paint-commands", |ui| {
        ui.add(egui::TextEdit::singleline(&mut st.palette_query).hint_text("liquify, puppet, HDR, panorama, blur, camera raw…").desired_width(f32::INFINITY));
        let found = studio_canvas::Canvas::commands(&st.palette_query);
        egui::ScrollArea::vertical().max_height(220.0).show_v(ui, |ui| {
            for c in found.iter().take(120) {
                let id = c["id"].as_str().unwrap_or("").to_owned();
                let on = st.palette_pick.as_deref() == Some(id.as_str());
                if ui.selectable_label(on, format!("{}   {}", c["label"].as_str().unwrap_or(""), id)).clicked() {
                    st.palette_pick = Some(id.clone());
                    st.palette_params = "{}".into();
                }
            }
        });
        if let Some(id) = st.palette_pick.clone() {
            ui.separator();
            let doc = found.iter().find(|c| c["id"] == json!(id)).and_then(|c| c["params"].as_str()).unwrap_or("").to_owned();
            ui.label(egui::RichText::new(&doc).small().monospace());
            ui.add(egui::TextEdit::multiline(&mut st.palette_params).desired_rows(3).code_editor().desired_width(f32::INFINITY));
            if ui.button(format!("Run {id}")).clicked() {
                match serde_json::from_str::<Value>(&st.palette_params) {
                    Ok(mut p) => {
                        if !p.is_object() {
                            p = json!({});
                        }
                        p["op"] = json!(id);
                        run(st, &id, vec![p]);
                    }
                    Err(e) => st.status = format!("Parameters: {e}"),
                }
            }
        }
    }));
    st.palette_open = open;
}

fn shortcuts(st: &mut PaintState, ctx: &egui::Context) {
    if ctx.egui_wants_keyboard_input() {
        return;
    }
    use egui::{Key, KeyboardShortcut, Modifiers};
    let cmd_shift = Modifiers::COMMAND | Modifiers::SHIFT;
    let mut ops: Vec<(&str, Value)> = Vec::new();
    ctx.input_mut(|i| {
        if i.consume_shortcut(&KeyboardShortcut::new(cmd_shift, Key::Z)) {
            ops.push(("Redo", json!({"op": "redo"})));
        } else if i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::Z)) {
            ops.push(("Undo", json!({"op": "undo"})));
        }
        if i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::D)) {
            ops.push(("Deselect", json!({"op": "select", "what": "none"})));
        }
        if i.consume_shortcut(&KeyboardShortcut::new(cmd_shift, Key::I)) {
            ops.push(("Invert", json!({"op": "select", "what": "invert"})));
        }
        if i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::S)) {
            ops.push(("Save", json!({"op": "save"})));
        }
        if i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::K)) {
            st.palette_open = true;
        }
        if i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::Num0)) {
            st.fit_pending = true;
        }
        if i.modifiers.is_none() {
            for tool in Tool::ALL {
                if let Some(k) = tool.key() {
                    if i.key_pressed(k) {
                        st.tool = tool;
                    }
                }
            }
            if i.key_pressed(Key::OpenBracket) {
                st.size = (st.size / 1.25).max(1.0);
            }
            if i.key_pressed(Key::CloseBracket) {
                st.size = (st.size * 1.25).min(1000.0);
            }
            if i.key_pressed(Key::X) {
                std::mem::swap(&mut st.color, &mut st.back);
            }
            if i.key_pressed(Key::Delete) || i.key_pressed(Key::Backspace) {
                ops.push(("Clear", json!({"op": "edit.clear"})));
            }
        }
    });
    for (label, op) in ops {
        run(st, label, vec![op]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tool_has_an_icon_that_rasterises() {
        let ctx = egui::Context::default();
        let mut st = PaintState::default();
        for t in Tool::ALL {
            assert!(icon(&mut st, &ctx, t.icon(), 18.0).is_some(), "{t:?}");
        }
        for (name, _) in ICONS {
            assert!(icon(&mut st, &ctx, name, 18.0).is_some(), "{name}");
        }
    }

    /// The workspace, headless: a drag with the brush paints on the
    /// picture, a marquee drag selects, and every tool takes a click.
    #[test]
    fn the_workspace_paints_selects_and_takes_every_tool() {
        let _turn = crate::paint::test_lock();
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let _serial = crate::ui_tests::serial();
        let mut app = StudioApp::new(&cc, crate::cli::Cli { screen: Some(Screen::Paint), fresh: true, ..Default::default() });
        crate::paint::apply(&[json!({"op": "new", "name": "UI test", "width": 400, "height": 200, "background": "white"})]).unwrap();
        let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(1280.0, 800.0));
        let mut t = 0.0;
        let mut frame = |app: &mut StudioApp, events: Vec<egui::Event>| {
            t += 1.0 / 60.0;
            let input = egui::RawInput { screen_rect: Some(screen), time: Some(t), events, ..Default::default() };
            // As a window's backend finishes a frame (egui panics when texture
            // changes are dropped unhandled).
            let mut out = ctx.run_ui(input, |ui| show(app, ui));
            let _ = ctx.tessellate(std::mem::take(&mut out.shapes), out.pixels_per_point);
            out.textures_delta.clear();
        };
        let press = |p: Pos2, down: bool| egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: down, modifiers: egui::Modifiers::NONE };
        let settle = |app: &mut StudioApp, frame: &mut dyn FnMut(&mut StudioApp, Vec<egui::Event>)| {
            for _ in 0..400 {
                frame(app, vec![]);
                if STATE.with(|s| s.borrow().jobs.is_empty()) {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        };
        settle(&mut app, &mut frame);
        STATE.with(|s| {
            let mut st = s.borrow_mut();
            st.tool = Tool::Brush;
            st.color = Color32::from_rgb(255, 0, 0);
            st.size = 12.0;
            st.hardness = 100.0;
            st.fit_pending = true;
        });
        frame(&mut app, vec![]);
        // The canvas sits between the tools and the side panel: its middle.
        let mid = Pos2::new((48.0 + 1280.0 - 280.0) / 2.0, (84.0 + 776.0) / 2.0);
        frame(&mut app, vec![egui::Event::PointerMoved(mid - Vec2::new(100.0, 0.0))]);
        frame(&mut app, vec![press(mid - Vec2::new(100.0, 0.0), true)]);
        for k in 1..=10 {
            frame(&mut app, vec![egui::Event::PointerMoved(mid - Vec2::new(100.0 - 20.0 * k as f32, 0.0))]);
        }
        frame(&mut app, vec![press(mid + Vec2::new(100.0, 0.0), false)]);
        settle(&mut app, &mut frame);
        let img = crate::paint::canvas().render(0).unwrap();
        let red = img.pixels.chunks_exact(4).filter(|p| p[0] > 200 && p[1] < 80).count();
        assert!(red > 200, "the brush drag painted ({red} red pixels); status {}", STATE.with(|s| s.borrow().status.clone()));
        // A marquee drag selects.
        STATE.with(|s| s.borrow_mut().tool = Tool::Marquee);
        frame(&mut app, vec![egui::Event::PointerMoved(mid)]);
        frame(&mut app, vec![press(mid, true)]);
        for k in 1..=5 {
            frame(&mut app, vec![egui::Event::PointerMoved(mid + Vec2::splat(10.0 * k as f32))]);
        }
        frame(&mut app, vec![press(mid + Vec2::splat(50.0), false)]);
        settle(&mut app, &mut frame);
        assert!(crate::paint::canvas().doc().unwrap().selection.is_some(), "selected");
        // Every tool takes a click without trouble.
        for tool in Tool::ALL {
            STATE.with(|s| s.borrow_mut().tool = tool);
            frame(&mut app, vec![egui::Event::PointerMoved(mid)]);
            frame(&mut app, vec![press(mid, true)]);
            frame(&mut app, vec![press(mid, false)]);
            settle(&mut app, &mut frame);
        }
        let _ = crate::paint::apply(&[json!({"op": "close"})]);
    }

    #[test]
    fn tool_keys_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for t in Tool::ALL {
            if let Some(k) = t.key() {
                assert!(seen.insert(k), "{k:?} twice");
            }
        }
    }
}

/// A pixel size entry for the new-canvas popup, in the kit's number field.
fn paint_dim(ui: &mut egui::Ui, name: &str, v: &mut u32) {
    let mut f = *v as f32;
    if kit::NumberField::new(name, &mut f).range(1.0..=30000.0).unit("px").width(84.0).show(ui).changed() {
        *v = f.round().max(1.0) as u32;
    }
}
