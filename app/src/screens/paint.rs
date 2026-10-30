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
use crate::icons::Icon;
use crate::theme::{self, space};
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
    /// In rail order: grouped as [`Tool::GROUPS`] shows.
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
        Tool::Bucket,
        Tool::Gradient,
        Tool::Smudge,
        Tool::Blur,
        Tool::Dodge,
        Tool::Burn,
        Tool::Heal,
        Tool::SpotHeal,
        Tool::Clone,
        Tool::Remove,
        Tool::Text,
        Tool::Eyedropper,
        Tool::Hand,
        Tool::Zoom,
    ];

    /// The rail's groups, with separators between them.
    pub const GROUPS: [(&'static str, &'static [Tool]); 5] = [
        ("Select", &[Tool::Move, Tool::Marquee, Tool::Ellipse, Tool::Lasso, Tool::Wand, Tool::AiObject, Tool::Crop]),
        ("Paint", &[Tool::Brush, Tool::Pencil, Tool::Eraser, Tool::Mixer, Tool::Bucket, Tool::Gradient]),
        ("Retouch", &[Tool::Smudge, Tool::Blur, Tool::Dodge, Tool::Burn, Tool::Heal, Tool::SpotHeal, Tool::Clone, Tool::Remove]),
        ("Type and colour", &[Tool::Text, Tool::Eyedropper]),
        ("View", &[Tool::Hand, Tool::Zoom]),
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
            Tool::Mixer => "Mixer brush",
            Tool::Smudge => "Smudge",
            Tool::Blur => "Blur",
            Tool::Dodge => "Dodge",
            Tool::Burn => "Burn",
            Tool::Heal => "Healing brush",
            Tool::SpotHeal => "Spot healing",
            Tool::Clone => "Clone stamp",
            Tool::Remove => "Remove",
            Tool::Bucket => "Paint bucket",
            Tool::Gradient => "Gradient",
            Tool::Eyedropper => "Eyedropper",
            Tool::Text => "Type",
            Tool::Hand => "Hand",
            Tool::Zoom => "Zoom",
        }
    }

    pub fn key(self) -> Option<egui::Key> {
        use egui::Key::*;
        Some(match self {
            Tool::Move => V,
            Tool::Marquee => M,
            Tool::Ellipse => Q,
            Tool::AiObject => A,
            Tool::Lasso => L,
            Tool::Wand => W,
            Tool::Crop => C,
            Tool::Brush => B,
            Tool::Pencil => N,
            Tool::Eraser => E,
            Tool::Mixer => K,
            Tool::Smudge => R,
            Tool::Blur => F,
            Tool::Dodge => O,
            Tool::Burn => P,
            Tool::SpotHeal => Y,
            Tool::Remove => U,
            Tool::Gradient => D,
            Tool::Heal => J,
            Tool::Clone => S,
            Tool::Bucket => G,
            Tool::Eyedropper => I,
            Tool::Text => T,
            Tool::Hand => H,
            Tool::Zoom => Z,
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
            Tool::Smudge => "smudge",
            Tool::Blur => "blur",
            Tool::Dodge => "sun",
            Tool::Burn => "moon",
            Tool::Heal => "bandage",
            Tool::SpotHeal => "bandage-sparkle",
            Tool::Clone => "stamp",
            Tool::Remove => "remove",
            Tool::Bucket => "paint-bucket",
            Tool::Gradient => "gradient",
            Tool::Eyedropper => "pipette",
            Tool::Text => "type",
            Tool::Hand => "hand",
            Tool::Zoom => "zoom-in",
        }
    }

    /// The tool's name and key, as its tooltip reads.
    pub fn tip(self) -> String {
        match self.key() {
            Some(k) => format!("{} ({})", self.name(), k.name()),
            None => self.name().to_owned(),
        }
    }

    /// One line on what to do, for the status bar.
    pub fn hint(self) -> &'static str {
        match self {
            Tool::Move => "Drag to move the layer",
            Tool::Marquee | Tool::Ellipse => "Drag to select · ⇧ adds · ⌥ subtracts · click to deselect",
            Tool::Lasso => "Draw around what to select · ⇧ adds · ⌥ subtracts",
            Tool::Wand => "Click a colour to select it · ⇧ adds · ⌥ subtracts",
            Tool::AiObject => "Click or drag a box around an object",
            Tool::Crop => "Drag the area to keep",
            Tool::Brush | Tool::Pencil | Tool::Mixer => "Drag to paint · [ ] change the size",
            Tool::Eraser => "Drag to erase · [ ] change the size",
            Tool::Smudge | Tool::Blur | Tool::Dodge | Tool::Burn => "Drag over the area · [ ] change the size",
            Tool::Heal | Tool::Clone => "⌥-click the source, then drag",
            Tool::SpotHeal => "Click or drag over a blemish",
            Tool::Remove => "Paint over what to remove",
            Tool::Bucket => "Click to fill a region",
            Tool::Gradient => "Drag from the start to the end",
            Tool::Eyedropper => "Click to pick a colour",
            Tool::Text => "Click to place the text",
            Tool::Hand => "Drag to pan · Space pans with any tool",
            Tool::Zoom => "Click to zoom in · ⌥-click to zoom out",
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

/// An icon in the rail's line style (24-unit grid, 2-unit stroke).
// TODO(kit): these drawn icons belong in the icon set once it has room for them.
macro_rules! svg {
    ($body:expr) => {
        concat!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">"#, $body, "</svg>")
    };
}

const ICONS: &[(&str, &str)] = &[
    ("smudge", svg!(r#"<path d="M3 17c4-1 6-5 9-8s5-3 8-2"/><path d="M4 21h9"/><path d="M17 12l4-1"/>"#)),
    ("blur", svg!(r#"<circle cx="12" cy="12" r="3"/><circle cx="12" cy="12" r="7" stroke-dasharray="2 3.4"/>"#)),
    ("remove", svg!(r#"<circle cx="12" cy="12" r="8" stroke-dasharray="3.4 3"/><path d="M9 9l6 6"/><path d="M15 9l-6 6"/>"#)),
    ("search", svg!(r#"<circle cx="11" cy="11" r="7"/><path d="M21 21l-4.3-4.3"/>"#)),
    ("swap", svg!(r#"<path d="M7 4L3 8l4 4"/><path d="M3 8h13"/><path d="M17 20l4-4-4-4"/><path d="M21 16H8"/>"#)),
    ("reset-colours", svg!(r#"<rect x="3" y="3" width="11" height="11" rx="2"/><rect x="10" y="10" width="11" height="11" rx="2" fill="currentColor"/>"#)),
    ("arrow-up", svg!(r#"<path d="M12 19V5"/><path d="M5 12l7-7 7 7"/>"#)),
    ("arrow-down", svg!(r#"<path d="M12 5v14"/><path d="M19 12l-7 7-7-7"/>"#)),
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
    ("sun", include_str!("../../assets/paint-icons/sun.svg")),
    ("moon", include_str!("../../assets/paint-icons/moon.svg")),
    ("bandage", include_str!("../../assets/paint-icons/bandage.svg")),
    ("bandage-sparkle", include_str!("../../assets/paint-icons/bandage-sparkle.svg")),
    ("stamp", include_str!("../../assets/paint-icons/stamp.svg")),
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

#[derive(Clone)]
enum Then {
    /// Read a colour sample into the foreground.
    Pick,
    /// Say this once it worked.
    Say(String),
    /// A save nobody asked for: quiet unless it fails.
    Autosave,
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
    new_bg: usize,
    last_picture: Option<String>,
    pen: Option<studio_pixel_tablet::Sample>,
    /// A first picture was made for this visit (so closing all of them leaves the empty state).
    auto_new: bool,
    /// The New sheet starts at the project's size, once.
    size_seeded: bool,
    /// The panel's last picture summary, kept while a long run holds the picture.
    last_sum: Option<Value>,
    dirty_since: Option<std::time::Instant>,
    autosave_off: bool,
    recent: Vec<Color32>,
    hex_text: String,
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
            palette_params: String::new(),
            palette_pick: None,
            adjust_params: String::new(),
            new_size: [1920, 1080],
            new_bg: 0,
            last_picture: None,
            pen: None,
            auto_new: false,
            size_seeded: false,
            last_sum: None,
            dirty_since: None,
            autosave_off: false,
            recent: Vec::new(),
            hex_text: String::new(),
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

/// `#rrggbb` (the `#` is optional) as a colour.
fn parse_hex(text: &str) -> Option<Color32> {
    let t = text.trim().trim_start_matches('#');
    if t.len() != 6 || !t.is_ascii() {
        return None;
    }
    let v = u32::from_str_radix(t, 16).ok()?;
    Some(Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

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
        // The window may have gone; nobody is left to read the answer.
        tx.send(r).ok();
    });
    st.status.clear();
    st.jobs.push(Job { label: label.to_owned(), rx, then });
}

/// Run ops and say `done` once they worked.
fn run_say(st: &mut PaintState, label: &str, ops: Vec<Value>, done: String) {
    run_then(st, label, ops, Some(Then::Say(done)));
}

/// After undo, redo or a history jump the animation re-reads its drawings
/// from the picture: it kept its own count, which a deleted (or brought-back)
/// drawing leaves stale until the next frame operation. A picture that is
/// not an animation says so, which is fine here.
fn resync_animation() {
    if let Err(e) = crate::paint::apply(&[json!({"op": "flipbook"})]) {
        eprintln!("animation not re-read after history: {e}");
    }
}

fn poll_jobs(st: &mut PaintState, ctx: &egui::Context) {
    let mut done = Vec::new();
    st.jobs.retain(|j| match j.rx.try_recv() {
        Ok(r) => {
            done.push((j.label.clone(), r, j.then.clone()));
            false
        }
        Err(std::sync::mpsc::TryRecvError::Empty) => true,
        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
            done.push((j.label.clone(), Err("it stopped before it finished".into()), j.then.clone()));
            false
        }
    });
    for (label, r, then) in done {
        match r {
            Ok(v) => {
                match then {
                    Some(Then::Pick) => {
                        if let Some(p) = v["results"][0].as_array().filter(|a| a.len() >= 3) {
                            let c = |i: usize| (p[i].as_f64().unwrap_or(0.0).clamp(0.0, 1.0) * 255.0).round() as u8;
                            st.color = Color32::from_rgb(c(0), c(1), c(2));
                        }
                    }
                    Some(Then::Say(m)) => {
                        st.autosave_off = false;
                        kit::push(ctx, m, None, Some(Icon::Check));
                    }
                    Some(Then::Autosave) | None => {}
                }
                if matches!(label.as_str(), "Undo" | "Redo" | "History") {
                    std::thread::spawn(resync_animation);
                }
            }
            Err(e) => match then {
                Some(Then::Autosave) => {
                    // Once, not every few seconds.
                    if !st.autosave_off {
                        st.autosave_off = true;
                        kit::push_error(ctx, format!("The picture could not be saved: {e}. Use Save to try again"));
                    }
                }
                _ => {
                    kit::push_error(ctx, format!("{label} failed: {}", friendly(&e)));
                }
            },
        }
    }
    if !st.jobs.is_empty() {
        ctx.request_repaint_after(std::time::Duration::from_millis(30));
    }
}

/// An engine refusal in words for the person who asked.
fn friendly(e: &str) -> String {
    let e = e.trim();
    if e.contains("can't move further") {
        return "the layer is already as far as it goes".into();
    }
    // "layer: …" names the engine's area, which means nothing here.
    e.split_once(": ").filter(|(head, _)| !head.contains(' ')).map_or(e, |(_, rest)| rest).to_owned()
}

/// How long a changed picture waits before it saves itself.
const AUTOSAVE_AFTER: std::time::Duration = std::time::Duration::from_secs(3);

/// Whether a picture changed `since` ago is due its quiet save.
fn autosave_due(dirty: bool, since: Option<std::time::Instant>, now: std::time::Instant) -> bool {
    dirty && since.is_some_and(|s| now.duration_since(s) >= AUTOSAVE_AFTER)
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
    let (r, resp) = ui.allocate_exact_size(Vec2::splat(space::CONTROL_H_LARGE + space::XS), Sense::click());
    let fill = if on { t.accent_soft } else if resp.hovered() { t.raised } else { Color32::TRANSPARENT };
    ui.painter().rect_filled(r, CornerRadius::same(theme::radius::CONTROL), fill);
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

/// Where each part of the workspace sits.
struct Layout {
    top: Rect,
    opts: Rect,
    tools: Rect,
    canvas: Rect,
    side: Rect,
    status: Rect,
}

/// The width of the tool rail: two columns of tools.
const RAIL_W: f32 = 80.0;

fn layout(full: Rect) -> Layout {
    let top = Rect::from_min_size(full.min, Vec2::new(full.width(), 44.0));
    let opts = Rect::from_min_size(Pos2::new(full.left(), top.bottom()), Vec2::new(full.width(), 40.0));
    let right_w = 280.0f32.min(full.width() * 0.35);
    let bottom = full.bottom() - 24.0;
    let tools = Rect::from_min_max(Pos2::new(full.left(), opts.bottom()), Pos2::new(full.left() + RAIL_W, bottom));
    let side = Rect::from_min_max(Pos2::new(full.right() - right_w, opts.bottom()), Pos2::new(full.right(), bottom));
    let canvas = Rect::from_min_max(Pos2::new(tools.right(), opts.bottom()), Pos2::new(side.left(), bottom));
    let status = Rect::from_min_max(Pos2::new(full.left(), bottom), full.max);
    Layout { top, opts, tools, canvas, side, status }
}

/// A name for a new picture that no open picture has.
fn unique_name(base: &str, taken: &[String]) -> String {
    let has = |n: &str| taken.iter().any(|t| studio_canvas::stem(t).eq_ignore_ascii_case(n));
    if !has(base) {
        return base.to_owned();
    }
    (2..).map(|i| format!("{base} {i}")).find(|n| !has(n)).unwrap_or_else(|| base.to_owned())
}

thread_local! {
    /// A picture file another screen wants open in Paint.
    static WANTED: RefCell<Option<std::path::PathBuf>> = const { RefCell::new(None) };
}

/// Open the picture at `path` (a still, or a frame saved from a clip) in
/// Paint, as the screen next draws. The file is only read; Paint works on a copy.
#[allow(dead_code)] // the clip and library menus call it once they offer "Edit in Paint" (see the report)
pub fn edit_in_paint(app: &mut StudioApp, path: std::path::PathBuf) {
    WANTED.with(|w| *w.borrow_mut() = Some(path));
    app.screen = Screen::Paint;
}

fn workspace(app: &mut StudioApp, st: &mut PaintState, ui: &mut egui::Ui, root: Option<&std::path::Path>) {
    let ctx = ui.ctx().clone();
    let t = theme::tokens(&ctx);
    poll_jobs(st, &ctx);
    st.pen = pen_sample();
    let full = ui.max_rect();
    ui.painter().rect_filled(full, 0, t.ground);
    let lay = layout(full);
    if !st.size_seeded {
        let s = &app.doc.model.settings;
        if s.width > 0 && s.height > 0 {
            st.new_size = [s.width.min(16000), s.height.min(16000)];
        }
        st.size_seeded = true;
    }

    // Pictures go in the project's folder; with none open, the last one comes
    // back, or a new one at the project's size.
    let mut empty = false;
    if let Some(mut c) = crate::paint::try_canvas() {
        let dir = crate::paint::pictures_dir(root);
        crate::paint::use_dir(&mut c, dir.clone());
        if c.doc().is_none() && st.jobs.is_empty() && let Err(e) = crate::paint::bring(&mut c, &dir, None) {
            kit::push_error(&ctx, format!("The last picture could not be opened: {e}"));
        }
        empty = c.doc().is_none();
    }
    if let Some(path) = WANTED.with(|w| w.borrow_mut().take()) {
        run(st, "Open", vec![json!({"op": "open", "path": path.display().to_string()})]);
        st.fit_pending = true;
    } else if empty && st.jobs.is_empty() && !st.auto_new {
        st.auto_new = true;
        let [w, h] = st.new_size;
        run(st, "New picture", vec![json!({"op": "new", "name": "Painting", "width": w, "height": h, "background": "white"})]);
        st.fit_pending = true;
    }
    autosave(st, &ctx);

    top_bar(app, st, ui, lay.top);
    options_bar(st, ui, lay.opts);
    tool_rail(st, ui, lay.tools);
    canvas_view(st, ui, lay.canvas);
    side_panel(st, ui, lay.side);
    status_bar(st, ui, lay.status);
    if st.palette_open {
        palette(st, &ctx);
    }
    shortcuts(st, &ctx);
}

/// Save a changed picture a moment after the last change, so leaving, a
/// crash or quitting loses nothing. Not while tests run: they must not write
/// into the user's folders.
fn autosave(st: &mut PaintState, ctx: &egui::Context) {
    // Tests must not write into the user's folders.
    if cfg!(test) {
        return;
    }
    {
        let dirty = crate::paint::try_canvas().and_then(|c| c.engine.active().map(|d| d.is_dirty())).unwrap_or(false);
        if !dirty || st.autosave_off {
            st.dirty_since = None;
            return;
        }
        if !st.jobs.is_empty() {
            return;
        }
        let now = std::time::Instant::now();
        let since = *st.dirty_since.get_or_insert(now);
        if autosave_due(true, Some(since), now) {
            st.dirty_since = None;
            run_then(st, "Save", vec![json!({"op": "save"})], Some(Then::Autosave));
        } else {
            ctx.request_repaint_after(std::time::Duration::from_millis(500));
        }
    }
}

/// Go back to the edit; a changed picture is saved first, so what was painted is in the project's Pictures.
fn leave(app: &mut StudioApp, st: &mut PaintState) {
    let (dirty, name) = match crate::paint::try_canvas() {
        Some(c) => (c.engine.active().is_some_and(|d| d.is_dirty()), c.doc().map(|d| studio_canvas::stem(&d.name).to_owned())),
        None => (false, None),
    };
    if dirty {
        let name = name.unwrap_or_else(|| "the picture".into());
        run_say(st, "Save", vec![json!({"op": "save"})], format!("Saved {name} in Pictures"));
    }
    app.screen = Screen::Editor;
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
    bar.spacing_mut().item_spacing.x = space::SM;
    if kit::Button::ghost("Back to edit").icon(Icon::CaretLeft).shortcut("⇧⌘P").show(bar).on_hover_text("Back to the timeline. A changed picture is saved in the project's Pictures first").clicked() {
        leave(app, st);
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
        if kit::chip(bar, studio_canvas::stem(n), on, None).clicked() && !on {
            run(st, "Switch", vec![json!({"op": "picture", "name": n})]);
            st.fit_pending = true;
        }
    }
    let new_id = new_sheet_id();
    let new = kit::Button::secondary("New").icon(Icon::Plus).show(bar);
    if new.clicked() {
        kit::toggle_open(bar.ctx(), new_id);
    }
    kit::popover(bar.ctx(), new_id, new.rect, kit::Side::Below, 280.0, |ui| new_sheet(st, ui, &names));
    let open = kit::Button::secondary("Open").icon(Icon::FolderOpen).show(bar).on_hover_text("PSD, TIFF, PNG, JPEG, WebP, HEIC, EXR or camera raw. The file is only read");
    if open.clicked() && let Some(p) = rfd::FileDialog::new().add_filter("Pictures", studio_canvas::files::READS).pick_file() {
        run(st, "Open", vec![json!({"op": "open", "path": p.display().to_string()})]);
        st.fit_pending = true;
    }
    bar.separator();
    if kit::Button::secondary("Save").shortcut("⌘S").show(bar).on_hover_text("Into the project's Pictures folder").clicked() {
        let name = st.last_picture.as_deref().map(studio_canvas::stem).unwrap_or("the picture").to_owned();
        run_say(st, "Save", vec![json!({"op": "save"})], format!("Saved {name} in Pictures"));
    }
    if kit::Button::secondary("Export").icon(Icon::Export).show(bar).on_hover_text("To a file: PSD, TIFF, PNG, JPEG, WebP, EXR").clicked() {
        let stem = st.last_picture.as_deref().map(studio_canvas::stem).unwrap_or("Painting").to_owned();
        if let Some(p) = rfd::FileDialog::new().add_filter("Pictures", studio_canvas::files::WRITES).set_file_name(format!("{stem}.psd")).save_file() {
            let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            run_say(st, "Export", vec![json!({"op": "export", "path": p.display().to_string()})], format!("Exported {name}"));
        }
    }
    bar.separator();
    if icon_button(st, bar, "undo-2", "Undo (⌘Z)", false).clicked() {
        run(st, "Undo", vec![json!({"op": "undo"})]);
    }
    if icon_button(st, bar, "redo-2", "Redo (⇧⌘Z)", false).clicked() {
        run(st, "Redo", vec![json!({"op": "redo"})]);
    }
    bar.separator();
    if kit::Button::primary("Place on timeline").show(bar).on_hover_text("Puts the picture (or the animation) on the timeline at the playhead and goes back to the edit. Placing it again updates it").clicked() {
        place_on_timeline(app, st, &bar.ctx().clone());
    }
}

fn new_sheet_id() -> egui::Id {
    egui::Id::NULL.with("paint-new")
}

/// The "New" sheet: size (with presets), background, then a picture or an
/// animation.
fn new_sheet(st: &mut PaintState, ui: &mut egui::Ui, taken: &[String]) {
    ui.spacing_mut().item_spacing = Vec2::splat(space::SM);
    kit::menu_heading(ui, "Size");
    ui.horizontal(|ui| {
        paint_dim(ui, "Width", &mut st.new_size[0]);
        ui.label("×");
        paint_dim(ui, "Height", &mut st.new_size[1]);
    });
    ui.horizontal_wrapped(|ui| {
        for (label, size) in [("HD", [1920, 1080]), ("4K", [3840, 2160]), ("Square", [2048, 2048]), ("Poster", [2400, 3600])] {
            if kit::chip(ui, label, st.new_size == size, None).clicked() {
                st.new_size = size;
            }
        }
    });
    kit::menu_heading(ui, "Background");
    kit::segmented(ui, "paint-new-bg", &mut st.new_bg, &[kit::Segment::text("White"), kit::Segment::text("Transparent"), kit::Segment::text("Black")], Some(256.0), None);
    ui.add_space(space::XS);
    let bg = ["white", "transparent", "black"][st.new_bg.min(2)];
    let [w, h] = st.new_size;
    if kit::Button::primary("Create picture").min_width(256.0).show(ui).clicked() {
        let name = unique_name("Painting", taken);
        run(st, "New picture", vec![json!({"op": "new", "name": name, "width": w, "height": h, "background": bg})]);
        st.fit_pending = true;
        kit::set_open(ui.ctx(), new_sheet_id(), false);
    }
    if kit::Button::secondary("Create animation").min_width(256.0).show(ui).on_hover_text("12 drawings at 12 fps, each held for two frames").clicked() {
        let name = unique_name("Animation", taken);
        run(st, "New animation", vec![json!({"op": "new", "name": name, "width": w, "height": h, "background": bg}), json!({"op": "animate", "drawings": 12, "fps": 12, "hold": 2})]);
        st.fit_pending = true;
        kit::set_open(ui.ctx(), new_sheet_id(), false);
    }
}

/// What placing a picture says, by whether it made a clip or updated the ones already there.
fn placed_message(name: &str, reply: &[crate::agent::Content]) -> String {
    let updated = reply.iter().any(|c| matches!(c, crate::agent::Content::Text(t) if t.contains("\"updated\"")));
    format!("{} {name} on the timeline", if updated { "Updated" } else { "Placed" })
}

/// Save the picture and lay it on the timeline through the agent's op, so
/// the window and the AI place pictures the same way; then back to the edit.
fn place_on_timeline(app: &mut StudioApp, st: &mut PaintState, ctx: &egui::Context) {
    let name = st.last_picture.as_deref().map(studio_canvas::stem).unwrap_or("the picture").to_owned();
    let at = app.editor.playhead as f64 / crate::view::SEC as f64;
    let placeholder = crate::doc::Doc::new(app.doc.model.clone());
    let mut s = crate::agent::Session::new(std::mem::replace(&mut app.doc, placeholder));
    s.home = s.doc.folder.as_ref().map(|f| f.root().to_path_buf());
    let r = s.call("picture", &json!({"ops": [{"op": "to_timeline", "at": at}]}));
    app.doc = s.doc;
    match r {
        Ok(reply) => {
            kit::push(ctx, placed_message(&name, &reply), None, Some(Icon::Check));
            app.screen = Screen::Editor;
        }
        Err(e) => {
            kit::push_error(ctx, format!("{name} could not be placed: {e}"));
        }
    }
}

fn options_bar(st: &mut PaintState, ui: &mut egui::Ui, r: Rect) {
    let t = theme::tokens(ui.ctx());
    ui.painter().rect_filled(r, 0, t.sunken);
    let mut outer = ui.new_child(egui::UiBuilder::new().max_rect(r.shrink2(Vec2::new(10.0, 4.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
    // One line: a tool with more options than the window is wide scrolls
    // sideways.
    kit::scroll_strip(&mut outer, "paint-options", |bar| options_row(st, bar));
}

/// One slider with its name before and its value after, in a fixed width.
fn option(bar: &mut egui::Ui, name: &str, value: &mut f32, range: std::ops::RangeInclusive<f32>, default: f32, unit: &str, log: bool) {
    kit::Slider::new(name, value, range).default(default).unit(unit).logarithmic(log).with_label().with_value().width(OPTION_W).show(bar);
}

/// The width of one option (name, slider, value).
const OPTION_W: f32 = 230.0;

fn options_row(st: &mut PaintState, bar: &mut egui::Ui) {
    bar.spacing_mut().item_spacing.x = 8.0;
    bar.label(egui::RichText::new(st.tool.name()).strong());
    bar.separator();
    colour_wells(st, bar);
    bar.separator();
    let tool = st.tool;
    if tool.strokes() {
        option(bar, "Size", &mut st.size, 1.0..=1000.0, 20.0, "px", true);
        if matches!(tool, Tool::Brush | Tool::Eraser | Tool::Pencil | Tool::Heal | Tool::Clone | Tool::Dodge | Tool::Burn | Tool::Smudge | Tool::Blur) {
            option(bar, "Hardness", &mut st.hardness, 0.0..=100.0, 100.0, "%", false);
        }
        option(bar, "Opacity", &mut st.opacity, 1.0..=100.0, 100.0, "%", false);
        if matches!(tool, Tool::Brush | Tool::Mixer | Tool::Eraser) {
            option(bar, "Flow", &mut st.flow, 1.0..=100.0, 100.0, "%", false);
            // One menu for the brush presets.
            egui::ComboBox::from_id_salt("paint-feel").selected_text(FEELS[st.feel].0).show_ui(bar, |ui| {
                crate::kit::note_list(ui);
                for (i, (n, _)) in FEELS.iter().enumerate() {
                    ui.selectable_value(&mut st.feel, i, *n);
                }
            }).response.on_hover_text("Brush preset");
        }
        if matches!(tool, Tool::Brush | Tool::Pencil | Tool::Mixer | Tool::Eraser) {
            let before = st.symmetry;
            egui::ComboBox::from_id_salt("paint-sym").selected_text(format!("Symmetry: {}", SYMMETRY[st.symmetry].0)).show_ui(bar, |ui| {
                crate::kit::note_list(ui);
                for (i, (n, _)) in SYMMETRY.iter().enumerate() {
                    ui.selectable_value(&mut st.symmetry, i, *n);
                }
            }).response.on_hover_text("Mirror what you paint");
            if before != st.symmetry {
                let m = SYMMETRY[st.symmetry].1;
                run(st, "Symmetry", vec![if m.is_empty() { json!({"op": "paint.symmetryDisable"}) } else { json!({"op": "paint.setSymmetry", "mode": m}) }]);
            }
        }
        if tool == Tool::Clone || tool == Tool::Heal {
            bar.label(match st.clone_source {
                Some([x, y]) => format!("Source {x:.0}, {y:.0}"),
                None => "No source yet".into(),
            });
        }
        if let Some(p) = st.pen {
            bar.label(format!("Pen {:.0}%", p.pressure * 100.0));
        }
    }
    if matches!(tool, Tool::Wand | Tool::Bucket) {
        option(bar, "Tolerance", &mut st.tolerance, 0.0..=255.0, 32.0, "", false);
    }
    if tool == Tool::Text {
        bar.add(egui::TextEdit::singleline(&mut st.text).hint_text("Text").desired_width(240.0));
        option(bar, "Size", &mut st.size, 4.0..=400.0, 48.0, "pt", false);
    }
    if matches!(tool, Tool::Marquee | Tool::Ellipse | Tool::Lasso | Tool::Wand | Tool::AiObject) {
        if kit::Button::secondary("Refine edge").compact().show(bar).on_hover_text("Soft, hair-level edges").clicked() {
            run(st, "Refine edge", vec![json!({"op": "select.refineEdge", "radius": 6, "smartRadius": true})]);
        }
        if kit::Button::secondary("Feather 4 px").compact().show(bar).clicked() {
            run(st, "Feather", vec![json!({"op": "select.modify.feather", "radius": 4})]);
        }
        if kit::Button::ghost("Deselect").compact().shortcut("⌘D").show(bar).clicked() {
            run(st, "Deselect", vec![json!({"op": "select", "what": "none"})]);
        }
    }
}

/// The two colours, swap and reset.
fn colour_wells(st: &mut PaintState, bar: &mut egui::Ui) {
    egui::color_picker::color_edit_button_srgba(bar, &mut st.color, egui::color_picker::Alpha::Opaque).on_hover_text("Foreground colour");
    egui::color_picker::color_edit_button_srgba(bar, &mut st.back, egui::color_picker::Alpha::Opaque).on_hover_text("Background colour");
    if icon_button(st, bar, "swap", "Swap colours (X)", false).clicked() {
        std::mem::swap(&mut st.color, &mut st.back);
    }
    if icon_button(st, bar, "reset-colours", "Reset to black and white", false).clicked() {
        reset_colours(st);
    }
}

fn reset_colours(st: &mut PaintState) {
    st.color = Color32::BLACK;
    st.back = Color32::WHITE;
}

fn tool_rail(st: &mut PaintState, ui: &mut egui::Ui, r: Rect) {
    let t = theme::tokens(ui.ctx());
    ui.painter().rect_filled(r, 0, t.panel);
    let mut col = ui.new_child(egui::UiBuilder::new().max_rect(r.shrink2(Vec2::new(6.0, 8.0))).layout(egui::Layout::top_down(egui::Align::Min)));
    col.spacing_mut().item_spacing = Vec2::new(2.0, 2.0);
    egui::ScrollArea::vertical().id_salt("paint-tools").show_v(&mut col, |ui| {
        for (i, (_, tools)) in Tool::GROUPS.iter().enumerate() {
            if i > 0 {
                ui.add_space(space::XS);
                ui.separator();
                ui.add_space(space::XS);
            }
            for pair in tools.chunks(2) {
                ui.horizontal(|ui| {
                    for &tool in pair {
                        if icon_button(st, ui, tool.icon(), &tool.tip(), st.tool == tool).clicked() {
                            st.tool = tool;
                            st.stroke.clear();
                            st.status.clear();
                        }
                    }
                });
            }
        }
        ui.add_space(space::XS);
        ui.separator();
        ui.add_space(space::XS);
        if icon_button(st, ui, "search", "More tools: filters, liquify, puppet warp, HDR, panorama, camera raw (⌘K)", st.palette_open).clicked() {
            st.palette_open = !st.palette_open;
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
        // The picture is busy with the last change: a spinner, and the
        // canvas comes back on its own.
        kit::spinner(ui.painter(), r.center(), 10.0, t.text_2);
        ctx.request_repaint_after(std::time::Duration::from_millis(40));
        return;
    };
    let Some(doc) = c.doc() else {
        let mut empty = ui.new_child(egui::UiBuilder::new().max_rect(r));
        if kit::empty_state(&mut empty, Icon::Image, "No picture open. Start a new one or open a file.", Some(("New picture", Icon::Plus))) {
            kit::set_open(&ctx, new_sheet_id(), true);
        }
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
    if let Some(h) = resp.hover_pos() && st.tool.strokes() {
        painter.circle_stroke(h, (st.size * st.zoom / 2.0).max(2.0), Stroke::new(1.0, Color32::from_white_alpha(200)));
        painter.circle_stroke(h, (st.size * st.zoom / 2.0).max(2.0) + 1.0, Stroke::new(1.0, Color32::from_black_alpha(120)));
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
    if resp.dragged() && (st.tool.strokes() || st.tool == Tool::Lasso) && st.stroke.last().is_none_or(|l| (l[0] - d[0]).hypot(l[1] - d[1]) * view.zoom > 1.0) {
        st.stroke.push([d[0], d[1], pressure]);
    }
    let released = resp.drag_stopped() || resp.clicked();
    if !released {
        return;
    }
    let from = st.drag_from.take().map(|p| view.to_doc(p)).unwrap_or(d);
    let stroke = std::mem::take(&mut st.stroke);
    let clicked = resp.clicked();
    match st.tool {
        Tool::Zoom => {
            let k = if mods.alt { 0.5 } else { 2.0 };
            let before = (pos - r.center() - st.pan) / st.zoom;
            st.zoom = (st.zoom * k).clamp(0.01, 64.0);
            st.pan = pos - r.center() - before * st.zoom;
            return;
        }
        Tool::Clone if mods.alt => {
            st.clone_source = Some(d);
            st.status = format!("Clone source {:.0}, {:.0}", d[0], d[1]);
            return;
        }
        _ => {}
    }
    match gesture_ops(st, from, d, &stroke, clicked, mods, pressure) {
        Gesture::Run(label, ops, then) => {
            if st.tool == Tool::Crop {
                st.fit_pending = true;
            }
            if st.tool.strokes() || matches!(st.tool, Tool::Bucket | Tool::Gradient | Tool::Text) {
                remember_colour(st);
            }
            run_then(st, &label, ops, then);
        }
        Gesture::Hint(h) => st.status = h,
        Gesture::Nothing => {}
    }
}

/// What a finished gesture asks of the picture.
enum Gesture {
    Run(String, Vec<Value>, Option<Then>),
    Hint(String),
    Nothing,
}

/// The picture ops one finished mouse gesture of the current tool makes
/// (pure, so each tool can be proven without a window). `from` and `d` are
/// the press and release points in the picture.
fn gesture_ops(st: &PaintState, from: [f32; 2], d: [f32; 2], stroke: &[[f32; 3]], clicked: bool, mods: egui::Modifiers, pressure: f32) -> Gesture {
    let rect = |a: [f32; 2], b: [f32; 2]| [a[0].min(b[0]).round(), a[1].min(b[1]).round(), (a[0] - b[0]).abs().round().max(1.0), (a[1] - b[1]).abs().round().max(1.0)];
    let one = |l: &str, op: Value| Gesture::Run(l.to_owned(), vec![op], None);
    match st.tool {
        Tool::Move => {
            let (dx, dy) = ((d[0] - from[0]).round(), (d[1] - from[1]).round());
            if dx != 0.0 || dy != 0.0 { one("Move", json!({"op": "layer.translate", "dx": dx, "dy": dy})) } else { Gesture::Nothing }
        }
        Tool::Marquee | Tool::Ellipse => {
            if clicked {
                one("Deselect", json!({"op": "select", "what": "none"}))
            } else {
                // The picture op "ellipse" names an engine command that does not
                // exist, so both marquees go to the engine's own rectangle command.
                let [x, y, w, h] = rect(from, d);
                one("Select", json!({"op": "select.rect", "x": x, "y": y, "width": w, "height": h, "ellipse": st.tool == Tool::Ellipse, "mode": mode_of(mods)}))
            }
        }
        Tool::Crop => {
            if clicked {
                return Gesture::Nothing;
            }
            let [x, y, w, h] = rect(from, d);
            one("Crop", json!({"op": "image.crop", "x": x, "y": y, "width": w, "height": h}))
        }
        Tool::Lasso => {
            if stroke.len() > 2 {
                let pts: Vec<[f32; 2]> = stroke.iter().map(|p| [p[0], p[1]]).collect();
                one("Lasso", json!({"op": "select", "what": "lasso", "points": pts, "mode": mode_of(mods)}))
            } else {
                Gesture::Nothing
            }
        }
        Tool::Wand => one("Magic wand", json!({"op": "select", "what": "wand", "point": [d[0].round(), d[1].round()], "tolerance": st.tolerance, "mode": mode_of(mods)})),
        Tool::AiObject => {
            let op = if clicked { json!({"op": "select", "what": "object", "point": d, "mode": mode_of(mods)}) } else { json!({"op": "select", "what": "object", "box": [from[0].min(d[0]), from[1].min(d[1]), from[0].max(d[0]), from[1].max(d[1])], "mode": mode_of(mods)}) };
            one("Select object", op)
        }
        Tool::Bucket => one("Fill", json!({"op": "paint.bucket", "x": d[0].round(), "y": d[1].round(), "tolerance": st.tolerance, "color": hex(st.color)})),
        Tool::Gradient => {
            if clicked {
                Gesture::Nothing
            } else {
                one("Gradient", json!({"op": "gradient", "from": from, "to": d, "colors": [hex(st.color), hex(st.back)]}))
            }
        }
        Tool::Eyedropper => Gesture::Run("Pick colour".into(), vec![json!({"op": "document.pixel", "x": d[0].round(), "y": d[1].round()})], Some(Then::Pick)),
        Tool::Text => one("Type", json!({"op": "type.create", "x": d[0].round(), "y": d[1].round(), "text": st.text, "size": st.size, "color": hex(st.color)})),
        Tool::Hand | Tool::Zoom => Gesture::Nothing,
        tool => {
            let mut pts = stroke.to_vec();
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
                    None => return Gesture::Hint("⌥-click where to copy from first".into()),
                }
            }
            one(tool.name(), op)
        }
    }
}

fn side_panel(st: &mut PaintState, ui: &mut egui::Ui, r: Rect) {
    let t = theme::tokens(ui.ctx());
    ui.painter().rect_filled(r, 0, t.panel);
    let mut col = ui.new_child(egui::UiBuilder::new().max_rect(r.shrink(space::SM)).layout(egui::Layout::top_down(egui::Align::Min)));
    // A run that holds the picture leaves the last summary standing; no
    // picture at all clears it.
    match crate::paint::try_canvas() {
        Some(c) => st.last_sum = c.summary().ok(),
        None => ui.ctx().request_repaint_after(std::time::Duration::from_millis(80)),
    }
    let sum = st.last_sum.clone();
    egui::ScrollArea::vertical().id_salt("paint-side").auto_shrink([false, false]).show_v(&mut col, |ui| {
        ui.set_width(ui.available_width());
        kit::Section::new("paint-layers", "Layers").show(ui, |ui| match &sum {
            Some(sum) => layers(st, ui, sum),
            None => {
                ui.label(egui::RichText::new("No picture open").color(t.text_2));
                if kit::Button::secondary("New picture").icon(Icon::Plus).compact().show(ui).clicked() {
                    kit::set_open(ui.ctx(), new_sheet_id(), true);
                }
            }
        });
        kit::Section::new("paint-colour", "Colour").show(ui, |ui| colour_panel(st, ui));
        let Some(sum) = sum else { return };
        kit::Section::new("paint-ai", "Selection").show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                for (label, what) in [("Subject", "subject"), ("People", "people"), ("Sky", "sky"), ("Background", "background")] {
                    if kit::Button::secondary(label).compact().show(ui).on_hover_text("Select it with AI").clicked() {
                        run(st, &format!("Select {label}"), vec![json!({"op": "select", "what": what})]);
                    }
                }
            });
            ui.horizontal_wrapped(|ui| {
                if kit::Button::secondary("Remove it").compact().show(ui).on_hover_text("Fill the selection from its surroundings").clicked() {
                    run(st, "Remove", vec![json!({"op": "remove"})]);
                }
                if kit::Button::secondary("Mask it").compact().show(ui).on_hover_text("The selection as the layer's mask").clicked() {
                    run(st, "Mask", vec![json!({"op": "layer", "action": "mask"})]);
                }
                if kit::Button::ghost("Invert").compact().shortcut("⇧⌘I").show(ui).clicked() {
                    run(st, "Invert", vec![json!({"op": "select", "what": "invert"})]);
                }
                if kit::Button::ghost("Deselect").compact().shortcut("⌘D").show(ui).clicked() {
                    run(st, "Deselect", vec![json!({"op": "select", "what": "none"})]);
                }
            });
        });
        kit::Section::new("paint-adjust", "Adjustments").start_closed().show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                for (label, kind) in ADJUSTMENTS {
                    if kit::Button::secondary(label).compact().show(ui).clicked() {
                        run(st, label, vec![json!({"op": "adjust", "kind": kind})]);
                    }
                }
            });
            ui.label(egui::RichText::new("Options for the selected adjustment, like: amount 20, tint 0.3").font(theme::caption()).color(t.text_2));
            ui.add(egui::TextEdit::singleline(&mut st.adjust_params).hint_text("amount 20, tint 0.3").desired_width(f32::INFINITY));
            if kit::Button::secondary("Apply to adjustment").compact().show(ui).clicked() {
                match crate::paint::options_to_params(&st.adjust_params) {
                    Ok(mut p) => {
                        p["op"] = json!("layer.setAdjustment");
                        run(st, "Adjust", vec![p]);
                    }
                    Err(e) => {
                        kit::push_error(ui.ctx(), e);
                    }
                }
            }
            ui.horizontal_wrapped(|ui| {
                if kit::Button::ghost("Camera raw").compact().show(ui).clicked() {
                    st.palette_open = true;
                    st.palette_query = "camera raw".into();
                }
                if kit::Button::ghost("Frequency separation").compact().show(ui).clicked() {
                    run(st, "Frequency separation", vec![json!({"op": "frequency_separation", "radius": 6})]);
                }
                if kit::Button::ghost("Liquify").compact().show(ui).clicked() {
                    st.palette_open = true;
                    st.palette_query = "liquify".into();
                }
                if kit::Button::ghost("More tools").compact().shortcut("⌘K").show(ui).clicked() {
                    st.palette_open = true;
                }
            });
        });
        if let Some(fb) = sum.get("flipbook") {
            kit::Section::new("paint-animation", "Animation").show(ui, |ui| animation(st, ui, fb));
        } else {
            kit::Section::new("paint-animation", "Animation").start_closed().show(ui, |ui| {
                ui.label(egui::RichText::new("Draw frame by frame with onion skin").font(theme::caption()).color(t.text_2));
                if kit::Button::secondary("Make this picture an animation").compact().show(ui).clicked() {
                    run(st, "Animate", vec![json!({"op": "animate", "drawings": 12, "fps": 12, "hold": 2})]);
                }
            });
        }
        kit::Section::new("paint-history", "History").start_closed().show(ui, |ui| history(st, ui));
    });
}

/// The colours: hex entry, swap and reset, the last colours used, and a few to start from.
fn colour_panel(st: &mut PaintState, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        let (fg, _) = ui.allocate_exact_size(Vec2::splat(space::CONTROL_H_LARGE), Sense::hover());
        let (bg, _) = ui.allocate_exact_size(Vec2::splat(space::CONTROL_H_LARGE), Sense::hover());
        ui.painter().rect_filled(bg, CornerRadius::same(theme::radius::CONTROL), st.back);
        ui.painter().rect_filled(fg, CornerRadius::same(theme::radius::CONTROL), st.color);
        let before = hex(st.color);
        if st.hex_text.is_empty() || (parse_hex(&st.hex_text) != Some(st.color) && !ui.memory(|m| m.focused().is_some())) {
            st.hex_text = before;
        }
        let edit = ui.add(egui::TextEdit::singleline(&mut st.hex_text).desired_width(76.0).hint_text("#rrggbb"));
        if edit.changed() && let Some(c) = parse_hex(&st.hex_text) {
            st.color = c;
        }
        if icon_button(st, ui, "swap", "Swap colours (X)", false).clicked() {
            std::mem::swap(&mut st.color, &mut st.back);
        }
        if icon_button(st, ui, "reset-colours", "Reset to black and white", false).clicked() {
            reset_colours(st);
        }
    });
    let starters = [Color32::BLACK, Color32::WHITE, Color32::from_rgb(0xe5, 0x3e, 0x3e), Color32::from_rgb(0xf0, 0x9b, 0x2c), Color32::from_rgb(0xf2, 0xd3, 0x3a), Color32::from_rgb(0x3d, 0xb4, 0x6d), Color32::from_rgb(0x2f, 0x7f, 0xe0), Color32::from_rgb(0x8a, 0x4f, 0xd8)];
    let mut pick = None;
    if !st.recent.is_empty() {
        ui.label(egui::RichText::new("Recent").font(theme::caption()).color(theme::tokens(ui.ctx()).text_2));
        ui.horizontal_wrapped(|ui| {
            for &c in &st.recent {
                if swatch(ui, c).clicked() {
                    pick = Some(c);
                }
            }
        });
    }
    ui.label(egui::RichText::new("Swatches").font(theme::caption()).color(theme::tokens(ui.ctx()).text_2));
    ui.horizontal_wrapped(|ui| {
        for c in starters {
            if swatch(ui, c).clicked() {
                pick = Some(c);
            }
        }
    });
    if let Some(c) = pick {
        st.color = c;
    }
}

fn swatch(ui: &mut egui::Ui, c: Color32) -> egui::Response {
    let (r, resp) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::click());
    ui.painter().rect_filled(r, CornerRadius::same(theme::radius::CONTROL), c);
    ui.painter().rect_stroke(r, CornerRadius::same(theme::radius::CONTROL), Stroke::new(1.0, theme::tokens(ui.ctx()).edge), egui::StrokeKind::Inside);
    resp.on_hover_text(hex(c))
}

/// Remember a colour that was painted with (the last ten, newest first).
fn remember_colour(st: &mut PaintState) {
    let c = st.color;
    st.recent.retain(|&x| x != c);
    st.recent.insert(0, c);
    st.recent.truncate(10);
}

fn layers(st: &mut PaintState, ui: &mut egui::Ui, sum: &Value) {
    ui.horizontal_wrapped(|ui| {
        for (icon, tip, label, op) in [
            ("plus", "New layer", "New layer", json!({"op": "layer", "action": "new"})),
            ("folder-plus", "Group", "Group", json!({"op": "layer", "action": "group"})),
            ("layer-mask", "Add a mask from the selection", "Mask", json!({"op": "layer", "action": "mask"})),
            ("adjustment-layer", "Curves adjustment layer", "Curves", json!({"op": "adjust", "kind": "curves"})),
            ("clip-below", "Clip to the layer below", "Clip", json!({"op": "layer", "action": "clip"})),
            ("copy", "Duplicate", "Duplicate", json!({"op": "layer", "action": "duplicate"})),
            ("arrow-up", "Move up", "Move up", json!({"op": "layer", "action": "up"})),
            ("arrow-down", "Move down", "Move down", json!({"op": "layer", "action": "down"})),
            ("trash", "Delete layer (⌘Z brings it back)", "Delete layer", json!({"op": "layer", "action": "delete"})),
        ] {
            if icon_button(st, ui, icon, tip, false).clicked() {
                run(st, label, vec![op]);
            }
        }
    });
    let empty = Vec::new();
    let rows = sum["layers"].as_array().unwrap_or(&empty);
    for l in rows {
        let id = l["id"].as_u64().unwrap_or(0);
        let active = l["active"].as_bool().unwrap_or(false);
        let hidden = l["hidden"].as_bool().unwrap_or(false);
        let depth = l["in_group"].as_u64().unwrap_or(0) as f32;
        ui.horizontal(|ui| {
            ui.add_space(depth * 12.0);
            if icon_button(st, ui, if hidden { "eye-off" } else { "eye" }, if hidden { "Show layer" } else { "Hide layer" }, false).clicked() {
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
                    for b in BLENDS {
                        if ui.selectable_label(blend == b, b).clicked() {
                            run(st, "Blend", vec![json!({"op": "layer.setProps", "layer": id, "blend": b})]);
                        }
                    }
                });
            });
        }
    }
}

const BLENDS: [&str; 21] = ["Normal", "Multiply", "Screen", "Overlay", "SoftLight", "HardLight", "ColorDodge", "ColorBurn", "Darken", "Lighten", "Difference", "Exclusion", "Hue", "Saturation", "Color", "Luminosity", "LinearLight", "LinearDodge", "LinearBurn", "VividLight", "PinLight"];

fn animation(st: &mut PaintState, ui: &mut egui::Ui, fb: &Value) {
    let n = fb["drawings"].as_u64().unwrap_or(0) as usize;
    let cur = fb["current"].as_u64().unwrap_or(1) as usize;
    ui.label(format!("{n} drawings · {} frames · {} fps · {}s", fb["frames"], fb["fps"], fb["seconds"]));
    ui.horizontal_wrapped(|ui| {
        for i in 1..=n {
            let hold = fb["holds"][i - 1].as_u64().unwrap_or(1);
            if kit::chip(ui, &format!("{i}{}", if hold > 1 { format!("×{hold}") } else { String::new() }), i == cur, None).clicked() {
                run(st, "Drawing", vec![json!({"op": "frame", "go": i})]);
            }
        }
    });
    ui.horizontal_wrapped(|ui| {
        if kit::Button::secondary("◀").compact().show(ui).clicked() {
            run(st, "Previous", vec![json!({"op": "frame", "action": "prev"})]);
        }
        if kit::Button::secondary("▶").compact().show(ui).clicked() {
            run(st, "Next", vec![json!({"op": "frame", "action": "next"})]);
        }
        if kit::Button::secondary("+ Blank").compact().show(ui).clicked() {
            run(st, "New drawing", vec![json!({"op": "frame", "action": "add"})]);
        }
        if kit::Button::secondary("+ Copy").compact().show(ui).clicked() {
            run(st, "Copy drawing", vec![json!({"op": "frame", "action": "copy"})]);
        }
        if kit::Button::secondary("Delete").compact().show(ui).clicked() {
            run(st, "Delete drawing", vec![json!({"op": "frame", "action": "delete"})]);
        }
    });
    ui.horizontal_wrapped(|ui| {
        ui.label(egui::RichText::new("Hold").font(theme::caption()));
        for h in [1u32, 2, 3, 4] {
            if kit::Button::ghost(&h.to_string()).compact().show(ui).on_hover_text(format!("Hold this drawing for {h} frames")).clicked() {
                run(st, "Hold", vec![json!({"op": "frame", "action": "hold", "hold": h})]);
            }
        }
        let on = fb["onion"]["on"].as_bool().unwrap_or(true);
        if kit::chip(ui, "Onion skin", on, None).clicked() {
            run(st, "Onion skin", vec![json!({"op": "flipbook", "onion": !on})]);
        }
    });
}

fn history(st: &mut PaintState, ui: &mut egui::Ui) {
    let rows: Vec<String> = match crate::paint::try_canvas() {
        Some(c) => c.engine.active().map(|d| d.history.entries()).unwrap_or_default(),
        None => return,
    };
    if kit::Button::secondary("Snapshot").compact().show(ui).on_hover_text("Keep this state to come back to").clicked() {
        run(st, "Snapshot", vec![json!({"op": "snapshot", "name": "Snapshot"})]);
    }
    for (i, label) in rows.iter().enumerate().rev().take(40) {
        let row = format!("{} {label}", i + 1);
        if ui.add(egui::Button::selectable(false, row.as_str()).truncate()).on_hover_text(&row).clicked() {
            run(st, "History", vec![json!({"op": "history.selectState", "state": i})]);
        }
    }
}

/// What the status bar says on the left: the work in progress, a note, or what the tool does.
fn status_text(st: &PaintState) -> String {
    if let Some(j) = st.jobs.first() {
        format!("{}…", j.label)
    } else if !st.status.is_empty() {
        st.status.clone()
    } else {
        st.tool.hint().to_owned()
    }
}

fn status_bar(st: &mut PaintState, ui: &mut egui::Ui, r: Rect) {
    let t = theme::tokens(ui.ctx());
    ui.painter().rect_filled(r, 0, t.panel);
    // Right: the picture and the zoom. Left: what the tool does.
    let info = crate::paint::try_canvas().and_then(|c| c.doc().map(|d| format!("{} · {}×{} px", studio_canvas::stem(&d.name), d.size.width, d.size.height))).or_else(|| st.last_sum.as_ref().map(|s| format!("{} · {}×{} px", s["picture"].as_str().map(studio_canvas::stem).unwrap_or(""), s["size"][0], s["size"][1]))).unwrap_or_default();
    let right = format!("{info} · {:.0}%", st.zoom * 100.0);
    let galley = ui.painter().layout_no_wrap(right.clone(), theme::caption(), t.text_2);
    let rw = galley.size().x.min(r.width() * 0.5);
    kit::paint_text_clipped(ui.painter(), Pos2::new(r.right() - 10.0, r.center().y), egui::Align2::RIGHT_CENTER, &right, theme::caption(), t.text_2, rw);
    let text = status_text(st);
    // One line: a long note ends in an ellipsis and shows whole on hover.
    let cut = kit::paint_text_clipped(ui.painter(), Pos2::new(r.left() + 10.0, r.center().y), egui::Align2::LEFT_CENTER, &text, theme::caption(), t.text_2, (r.width() - rw - 40.0).max(40.0));
    if cut {
        ui.interact(r, egui::Id::NULL.with("paint-status"), Sense::hover()).on_hover_text(text);
    }
}

/// Every engine command, searchable by name.
fn palette(st: &mut PaintState, ctx: &egui::Context) {
    let t = theme::tokens(ctx);
    let out = kit::modal(ctx, egui::Id::new("paint-palette"), Vec2::new(560.0, 480.0), "More tools", |ui| kit::scroll_panel(ui, "paint-commands", |ui| {
        ui.add(egui::TextEdit::singleline(&mut st.palette_query).hint_text("liquify, puppet, HDR, panorama, blur, camera raw").desired_width(f32::INFINITY));
        let found = studio_canvas::Canvas::commands(&st.palette_query);
        if found.is_empty() {
            let mut empty = ui.new_child(egui::UiBuilder::new().max_rect(ui.available_rect_before_wrap().shrink(space::SM)));
            kit::empty_state(&mut empty, Icon::MagnifyingGlass, "No tool matches that search.", None);
        }
        egui::ScrollArea::vertical().max_height(220.0).show_v(ui, |ui| {
            for c in found.iter().take(120) {
                let id = c["id"].as_str().unwrap_or("").to_owned();
                let label = c["label"].as_str().unwrap_or("").to_owned();
                let on = st.palette_pick.as_deref() == Some(id.as_str());
                if kit::ListRow::new(&label).selected(on).show(ui).response.clicked() {
                    st.palette_pick = Some(id.clone());
                    st.palette_params.clear();
                }
            }
        });
        if let Some(id) = st.palette_pick.clone() {
            ui.separator();
            let spec = found.iter().find(|c| c["id"] == json!(id));
            let label = spec.and_then(|c| c["label"].as_str()).unwrap_or("this tool").to_owned();
            let doc = spec.and_then(|c| c["params"].as_str()).unwrap_or("").trim().to_owned();
            let has_options = !(doc.is_empty() || doc == "{}" || doc.eq_ignore_ascii_case("none"));
            if has_options {
                let hint = doc.trim_matches(|c| c == '{' || c == '}').to_owned();
                ui.label(egui::RichText::new(format!("Options: {hint}")).font(theme::caption()).color(t.text_2));
                ui.add(egui::TextEdit::singleline(&mut st.palette_params).hint_text("radius 4, mode soft").desired_width(f32::INFINITY));
            }
            if kit::Button::primary(&format!("Apply {label}")).show(ui).clicked() {
                match crate::paint::options_to_params(&st.palette_params) {
                    Ok(mut p) => {
                        if !p.is_object() {
                            p = json!({});
                        }
                        p["op"] = json!(id);
                        run(st, &label, vec![p]);
                    }
                    Err(e) => st.status = e,
                }
            }
        }
    }));
    if out.close {
        st.palette_open = false;
    }
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
                if let Some(k) = tool.key() && i.key_pressed(k) {
                    st.tool = tool;
                    st.status.clear();
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

    fn layer_count() -> usize {
        crate::paint::canvas().summary().ok().and_then(|s| s["layers"].as_array().map(Vec::len)).unwrap_or(0)
    }

    /// Delete layer is a history step: Undo brings the layer back, Redo removes it again.
    #[test]
    fn deleting_a_layer_can_be_undone() {
        let _turn = crate::paint::test_lock();
        crate::paint::apply(&[json!({"op": "new", "name": "Delete test", "width": 64, "height": 64, "background": "white"})]).unwrap();
        crate::paint::apply(&[json!({"op": "layer", "action": "new"})]).unwrap();
        let before = layer_count();
        assert!(before >= 2, "{before}");
        crate::paint::apply(&[json!({"op": "layer", "action": "delete"})]).unwrap();
        assert_eq!(layer_count(), before - 1);
        crate::paint::apply(&[json!({"op": "undo"})]).unwrap();
        assert_eq!(layer_count(), before, "undo brings the deleted layer back");
        crate::paint::apply(&[json!({"op": "redo"})]).unwrap();
        assert_eq!(layer_count(), before - 1);
    }

    /// Delete drawing, in an animation, is a history step too.
    #[test]
    fn deleting_a_drawing_can_be_undone() {
        let _turn = crate::paint::test_lock();
        crate::paint::apply(&[json!({"op": "new", "name": "Frame test", "width": 64, "height": 64, "background": "white"})]).unwrap();
        crate::paint::apply(&[json!({"op": "animate", "drawings": 4, "fps": 12, "hold": 1})]).unwrap();
        let drawings = || crate::paint::canvas().summary().ok().and_then(|s| s["flipbook"]["drawings"].as_u64().or_else(|| s["flipbook"]["drawings"].as_array().map(|a| a.len() as u64)));
        let before = drawings();
        assert!(before.is_some_and(|n| n >= 2), "{before:?}");
        crate::paint::apply(&[json!({"op": "frame", "action": "delete"})]).unwrap();
        assert_eq!(drawings().map(|n| n + 1), before);
        crate::paint::apply(&[json!({"op": "undo"})]).unwrap();
        // The panel's counts come from the animation, which re-reads the picture after history.
        assert_eq!(drawings(), Some(before.unwrap() - 1), "stale until it is re-read");
        resync_animation();
        assert_eq!(drawings(), before, "undo brings the drawing back");
    }

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
        let mid = layout(screen).canvas.center();
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

    /// The New sheet opens and offers a picture; the tool search lists names,
    /// never command IDs or parameter text.
    #[test]
    fn new_sheet_and_tool_search_show_no_developer_text() {
        let _turn = crate::paint::test_lock();
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let _serial = crate::ui_tests::serial();
        let mut app = StudioApp::new(&cc, crate::cli::Cli { screen: Some(Screen::Paint), fresh: true, ..Default::default() });
        let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(1280.0, 800.0));
        let labels = |app: &mut StudioApp| {
            let input = egui::RawInput { screen_rect: Some(screen), ..Default::default() };
            let mut out = ctx.run_ui(input, |ui| show(app, ui));
            let l: Vec<String> = out.platform_output.accesskit_update.iter().flat_map(|u| u.nodes.iter()).filter_map(|(_, n)| n.label().map(str::to_owned)).collect();
            let _ = ctx.tessellate(std::mem::take(&mut out.shapes), out.pixels_per_point);
            out.textures_delta.clear();
            l
        };
        labels(&mut app);
        kit::set_open(&ctx, new_sheet_id(), true);
        for _ in 0..8 {
            labels(&mut app);
        }
        let l = labels(&mut app);
        assert!(l.iter().any(|s| s == "Create picture"), "the New sheet offers a picture: {l:?}");
        STATE.with(|s| {
            let mut st = s.borrow_mut();
            st.palette_open = true;
            st.palette_pick = Some("file.new".into());
        });
        let l = labels(&mut app);
        let l = [l, labels(&mut app)].concat();
        for s in &l {
            assert!(!s.contains("file.") && !s.contains("layer.") && !s.contains("JSON") && !s.contains("{"), "developer text on screen: {s}");
        }
        STATE.with(|s| s.borrow_mut().palette_open = false);
    }

    /// Run what a tool does for a drag from `from` to `to` on the open picture.
    fn drag(tool: Tool, from: [f32; 2], to: [f32; 2]) -> Result<Value, String> {
        let mut st = PaintState::default();
        st.tool = tool;
        st.color = Color32::from_rgb(255, 0, 0);
        st.back = Color32::from_rgb(0, 0, 255);
        st.size = 20.0;
        st.clone_source = Some([from[0], from[1] + 40.0]);
        let stroke: Vec<[f32; 3]> = (0..=20).map(|k| {
            let f = k as f32 / 20.0;
            [from[0] + (to[0] - from[0]) * f, from[1] + (to[1] - from[1]) * f, 1.0]
        }).collect();
        let stroke = if tool == Tool::Lasso { vec![[from[0], from[1], 1.0], [to[0], from[1], 1.0], [to[0], to[1], 1.0], [from[0], to[1], 1.0]] } else { stroke };
        match gesture_ops(&st, from, to, &stroke, false, egui::Modifiers::NONE, 1.0) {
            Gesture::Run(_, ops, _) => crate::paint::apply(&ops),
            Gesture::Hint(h) => Err(h),
            Gesture::Nothing => Err("nothing".into()),
        }
    }

    fn pixels() -> Vec<u8> {
        crate::paint::canvas().render(0).unwrap().pixels
    }

    fn fresh(w: u32, h: u32) {
        crate::paint::apply(&[json!({"op": "new", "name": "Wire test", "width": w, "height": h, "background": "white"})]).unwrap();
    }

    fn changed(a: &[u8], b: &[u8]) -> usize {
        a.chunks_exact(4).zip(b.chunks_exact(4)).filter(|(x, y)| x != y).count()
    }

    /// Every tool that paints or edits pixels changes the picture where it
    /// is dragged, and one Undo puts the picture back.
    #[test]
    fn every_pixel_tool_changes_the_picture_and_undoes() {
        let _turn = crate::paint::test_lock();
        for tool in [Tool::Brush, Tool::Pencil, Tool::Eraser, Tool::Mixer, Tool::Smudge, Tool::Blur, Tool::Dodge, Tool::Burn, Tool::Heal, Tool::SpotHeal, Tool::Clone, Tool::Remove, Tool::Bucket, Tool::Gradient, Tool::Text] {
            fresh(200, 200);
            // Something to work on: a red bar and a blue bar side by side.
            crate::paint::apply(&[json!({"op": "paint", "tool": "brush", "points": [[20.0, 100.0, 1.0], [180.0, 100.0, 1.0]], "size": 60, "color": "#cc2222", "hardness": 100}),
                                  json!({"op": "paint", "tool": "brush", "points": [[100.0, 20.0, 1.0], [100.0, 180.0, 1.0]], "size": 40, "color": "#2222cc", "hardness": 100})]).unwrap();
            let before = pixels();
            let r = drag(tool, [60.0, 90.0], [140.0, 110.0]);
            assert!(r.is_ok(), "{tool:?}: {r:?}");
            let after = pixels();
            assert!(changed(&before, &after) > 20, "{tool:?} changed {} pixels", changed(&before, &after));
            crate::paint::apply(&[json!({"op": "undo"})]).unwrap();
            assert_eq!(changed(&before, &pixels()), 0, "{tool:?}: undo restores the picture");
        }
    }

    /// The selection tools select, and the selection can be undone.
    #[test]
    fn selection_tools_select_and_undo() {
        let _turn = crate::paint::test_lock();
        for tool in [Tool::Marquee, Tool::Ellipse, Tool::Lasso, Tool::Wand] {
            fresh(200, 200);
            let r = drag(tool, [50.0, 50.0], [150.0, 150.0]);
            assert!(r.is_ok(), "{tool:?}: {r:?}");
            assert!(crate::paint::canvas().doc().unwrap().selection.is_some(), "{tool:?} selected");
            crate::paint::apply(&[json!({"op": "undo"})]).unwrap();
            assert!(crate::paint::canvas().doc().unwrap().selection.is_none(), "{tool:?}: undo clears the selection");
        }
    }

    #[test]
    fn crop_and_move_work_and_undo() {
        let _turn = crate::paint::test_lock();
        fresh(200, 200);
        drag(Tool::Crop, [20.0, 20.0], [120.0, 100.0]).unwrap();
        let size = || { let c = crate::paint::canvas(); let d = c.doc().unwrap(); (d.size.width, d.size.height) };
        assert_eq!(size(), (100, 80));
        crate::paint::apply(&[json!({"op": "undo"})]).unwrap();
        assert_eq!(size(), (200, 200));
        crate::paint::apply(&[json!({"op": "layer", "action": "new"}), json!({"op": "paint", "tool": "brush", "points": [[20.0, 20.0, 1.0], [40.0, 20.0, 1.0]], "size": 20, "color": "#ff0000", "hardness": 100})]).unwrap();
        let before = pixels();
        drag(Tool::Move, [10.0, 10.0], [60.0, 10.0]).unwrap();
        assert!(changed(&before, &pixels()) > 20, "the layer moved");
        crate::paint::apply(&[json!({"op": "undo"})]).unwrap();
        assert_eq!(changed(&before, &pixels()), 0);
    }

    #[test]
    fn eyedropper_reads_the_colour_and_type_adds_a_layer() {
        let _turn = crate::paint::test_lock();
        fresh(100, 100);
        crate::paint::apply(&[json!({"op": "paint", "tool": "brush", "points": [[50.0, 50.0, 1.0], [55.0, 50.0, 1.0]], "size": 40, "color": "#00ff00", "hardness": 100})]).unwrap();
        let v = drag(Tool::Eyedropper, [50.0, 50.0], [50.0, 50.0]).unwrap();
        let p = v["results"][0].as_array().expect("a colour comes back");
        assert!(p[1].as_f64().unwrap() > 0.9 && p[0].as_f64().unwrap() < 0.2, "{p:?}");
        let n = layer_count();
        drag(Tool::Text, [10.0, 10.0], [10.0, 10.0]).unwrap();
        assert_eq!(layer_count(), n + 1, "type makes a text layer");
        crate::paint::apply(&[json!({"op": "undo"})]).unwrap();
        assert_eq!(layer_count(), n);
    }

    fn active_layer() -> Value {
        let s = crate::paint::canvas().summary().unwrap();
        s["layers"].as_array().unwrap().iter().find(|l| l["active"] == json!(true)).cloned().unwrap()
    }

    fn layer_names() -> Vec<String> {
        let s = crate::paint::canvas().summary().unwrap();
        s["layers"].as_array().unwrap().iter().map(|l| l["name"].as_str().unwrap_or("").to_owned()).collect()
    }

    /// Opacity, every blend mode, visibility and the order of a layer take
    /// effect on the picture, and each is one undo.
    #[test]
    fn layer_controls_are_wired_and_undo() {
        let _turn = crate::paint::test_lock();
        fresh(64, 64);
        crate::paint::apply(&[json!({"op": "layer", "action": "new"}), json!({"op": "layer", "action": "new"})]).unwrap();
        let id = active_layer()["id"].as_u64().unwrap();
        crate::paint::apply(&[json!({"op": "layer.setProps", "layer": id, "opacity": 0.5})]).unwrap();
        assert_eq!(active_layer()["opacity"], json!(50.0), "opacity 50 %");
        crate::paint::apply(&[json!({"op": "undo"})]).unwrap();
        assert_eq!(active_layer()["opacity"], json!(100.0), "undo");
        for b in BLENDS {
            crate::paint::apply(&[json!({"op": "layer.setProps", "layer": id, "blend": b})]).unwrap_or_else(|e| panic!("blend {b}: {e}"));
            assert_eq!(active_layer()["blend"], json!(b), "blend {b} is the one shown");
        }
        crate::paint::apply(&[json!({"op": "layer.setProps", "layer": id, "visible": false})]).unwrap();
        assert_eq!(active_layer()["hidden"], json!(true));
        crate::paint::apply(&[json!({"op": "undo"})]).unwrap();
        assert!(active_layer().get("hidden").is_none(), "undo shows it again");
        let before = layer_names();
        crate::paint::apply(&[json!({"op": "layer", "action": "down"})]).unwrap();
        let after = layer_names();
        assert_ne!(before, after, "the layer moved down");
        crate::paint::apply(&[json!({"op": "layer", "action": "up"})]).unwrap();
        assert_eq!(layer_names(), before, "and back up");
        crate::paint::apply(&[json!({"op": "undo"}), json!({"op": "undo"})]).unwrap();
        assert_eq!(layer_names(), before);
    }

    #[test]
    fn symmetry_and_history_are_wired() {
        let _turn = crate::paint::test_lock();
        fresh(100, 100);
        // Off after a mode, as the options bar changes it.
        for (_, m) in SYMMETRY.iter().filter(|(_, m)| !m.is_empty()) {
            crate::paint::apply(&[json!({"op": "paint.setSymmetry", "mode": m})]).unwrap_or_else(|e| panic!("symmetry {m:?}: {e}"));
            crate::paint::apply(&[json!({"op": "paint.symmetryDisable"})]).unwrap_or_else(|e| panic!("symmetry off after {m:?}: {e}"));
        }
        let blank = pixels();
        // Mirrored left to right: a stroke on the left also lands on the right.
        crate::paint::apply(&[json!({"op": "paint.setSymmetry", "mode": "vertical"})]).unwrap();
        drag(Tool::Brush, [10.0, 50.0], [30.0, 50.0]).unwrap();
        let right = pixels().chunks_exact(4).enumerate().filter(|(i, p)| i % 100 >= 50 && p != &&blank[i * 4..i * 4 + 4]).count();
        assert!(right > 20, "the mirror painted on the other half ({right})");
        crate::paint::apply(&[json!({"op": "paint.symmetryDisable"}), json!({"op": "undo"})]).unwrap();
        assert_eq!(changed(&blank, &pixels()), 0);
        drag(Tool::Brush, [20.0, 20.0], [60.0, 20.0]).unwrap();
        let one = pixels();
        drag(Tool::Brush, [20.0, 60.0], [60.0, 60.0]).unwrap();
        let two = pixels();
        assert!(changed(&blank, &one) > 0 && changed(&one, &two) > 0);
        let rows = crate::paint::canvas().engine.active().unwrap().history.entries().len();
        assert!(rows >= 3, "{rows} history rows");
        crate::paint::apply(&[json!({"op": "history.selectState", "state": rows - 2})]).unwrap();
        assert_eq!(changed(&one, &pixels()), 0, "the history row before the last stroke");
    }

    /// Each adjustment in the panel adds an adjustment layer.
    #[test]
    fn every_adjustment_adds_a_layer() {
        let _turn = crate::paint::test_lock();
        fresh(64, 64);
        let mut bad = Vec::new();
        for (label, kind) in ADJUSTMENTS {
            let n = layer_count();
            match crate::paint::apply(&[json!({"op": "adjust", "kind": kind})]) {
                Ok(_) if layer_count() == n + 1 => {}
                other => bad.push(format!("{label}: {other:?}")),
            }
        }
        assert!(bad.is_empty(), "{bad:#?}");
    }

    #[test]
    fn tools_are_grouped_once_each_with_their_own_icon_and_key_hint() {
        let grouped: Vec<Tool> = Tool::GROUPS.iter().flat_map(|(_, t)| t.iter().copied()).collect();
        assert_eq!(grouped, Tool::ALL.to_vec(), "the rail shows every tool once, in order");
        let mut icons = std::collections::HashSet::new();
        for t in Tool::ALL {
            assert!(icons.insert(t.icon()), "{t:?} shares its icon");
            assert!(!t.hint().is_empty(), "{t:?} has a hint");
            assert!(t.tip().starts_with(t.name()), "{t:?}");
            if let Some(k) = t.key() {
                assert!(t.tip().contains(k.name()), "{t:?}: the tooltip names its key");
            }
        }
        assert!(Tool::ALL.iter().filter(|t| t.key().is_none()).count() == 0, "every tool has a key");
    }

    /// The parts of the workspace tile the window without overlapping.
    #[test]
    fn the_workspace_parts_do_not_overlap() {
        for (w, h) in [(1280.0, 800.0), (900.0, 600.0), (700.0, 500.0)] {
            let full = Rect::from_min_size(Pos2::ZERO, Vec2::new(w, h));
            let l = layout(full);
            let parts = [("top", l.top), ("options", l.opts), ("tools", l.tools), ("canvas", l.canvas), ("side", l.side), ("status", l.status)];
            for (i, (a, ra)) in parts.iter().enumerate() {
                assert!(full.contains_rect(*ra), "{a} inside at {w}×{h}");
                assert!(ra.width() > 0.0 && ra.height() > 0.0, "{a} has room at {w}×{h}");
                for (b, rb) in &parts[i + 1..] {
                    let o = ra.intersect(*rb);
                    assert!(o.width() <= 0.0 || o.height() <= 0.0, "{a} and {b} overlap at {w}×{h}");
                }
            }
        }
    }

    #[test]
    fn refusals_read_as_sentences() {
        assert_eq!(friendly("layer: layer can't move further"), "the layer is already as far as it goes");
        assert_eq!(friendly("select: nothing is selected"), "nothing is selected");
        assert_eq!(friendly("no picture open"), "no picture open");
    }

    #[test]
    fn small_helpers() {
        assert_eq!(unique_name("Painting", &[]), "Painting");
        assert_eq!(unique_name("Painting", &["Painting".into(), "painting 2".into()]), "Painting 3");
        assert_eq!(parse_hex("#ff0080"), Some(Color32::from_rgb(255, 0, 128)));
        assert_eq!(parse_hex("00ff00"), Some(Color32::from_rgb(0, 255, 0)));
        assert_eq!(parse_hex("#12"), None);
        assert_eq!(parse_hex("zzzzzz"), None);
        let t0 = std::time::Instant::now();
        assert!(!autosave_due(true, Some(t0), t0));
        assert!(autosave_due(true, Some(t0), t0 + AUTOSAVE_AFTER));
        assert!(!autosave_due(false, Some(t0), t0 + AUTOSAVE_AFTER * 2));
        assert!(!autosave_due(true, None, t0 + AUTOSAVE_AFTER * 2));
        let say = |t: &str| vec![crate::agent::Content::Text(t.into())];
        assert_eq!(placed_message("Sky", &say("{\"updated\":\"Sky\"}")), "Updated Sky on the timeline");
        assert_eq!(placed_message("Sky", &say("{\"placed\":1}")), "Placed Sky on the timeline");
        let mut st = PaintState::default();
        assert_eq!(status_text(&st), Tool::Brush.hint());
        st.status = "⌥-click where to copy from first".into();
        assert_eq!(status_text(&st), "⌥-click where to copy from first");
    }

    /// A finished job leaves no "✓" in the status line.
    #[test]
    fn finished_work_leaves_the_tool_hint_not_a_tick() {
        let ctx = egui::Context::default();
        let mut st = PaintState::default();
        let (tx, rx) = channel();
        tx.send(Ok(json!({}))).unwrap();
        st.jobs.push(Job { label: "Redo".into(), rx, then: None });
        assert_eq!(status_text(&st), "Redo…");
        poll_jobs(&mut st, &ctx);
        assert!(st.jobs.is_empty());
        assert_eq!(status_text(&st), st.tool.hint());
        assert!(!status_text(&st).contains('✓'));
    }

    /// Paint opens straight into a canvas, offers Back to edit and Place on
    /// the timeline, and Back to edit returns to the editor.
    #[test]
    fn opens_into_a_picture_and_goes_back_to_the_edit() {
        let _turn = crate::paint::test_lock();
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let _serial = crate::ui_tests::serial();
        let mut app = StudioApp::new(&cc, crate::cli::Cli { screen: Some(Screen::Paint), fresh: true, ..Default::default() });
        let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(1280.0, 800.0));
        let labels = |app: &mut StudioApp| {
            let input = egui::RawInput { screen_rect: Some(screen), ..Default::default() };
            let mut out = ctx.run_ui(input, |ui| show(app, ui));
            let l: Vec<String> = out.platform_output.accesskit_update.iter().flat_map(|u| u.nodes.iter()).filter_map(|(_, n)| n.label().map(str::to_owned)).collect();
            let _ = ctx.tessellate(std::mem::take(&mut out.shapes), out.pixels_per_point);
            out.textures_delta.clear();
            l
        };
        let _ = crate::paint::apply(&[json!({"op": "close"})]);
        let mut seen = Vec::new();
        for _ in 0..400 {
            seen = labels(&mut app);
            if idle() && crate::paint::canvas().doc().is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        seen.extend(labels(&mut app));
        assert!(crate::paint::canvas().doc().is_some(), "a picture is open without anyone asking");
        for want in ["Back to edit", "Place on timeline", "Save", "Export", "Layers", "Colour"] {
            assert!(seen.iter().any(|s| s == want || s.starts_with(want)), "{want} is on screen: {seen:?}");
        }
        STATE.with(|s| {
            let st = s.borrow();
            let m = &app.doc.model.settings;
            if m.width > 0 && m.height > 0 {
                assert_eq!(st.new_size, [m.width.min(16000), m.height.min(16000)], "new pictures start at the project's size");
            }
        });
        STATE.with(|s| leave(&mut app, &mut s.borrow_mut()));
        assert_eq!(app.screen, Screen::Editor);
        let _ = crate::paint::apply(&[json!({"op": "close"})]);
    }

    /// Another screen can hand Paint a picture file to edit.
    #[test]
    fn a_picture_file_handed_to_paint_opens_as_a_copy() {
        let _turn = crate::paint::test_lock();
        let dir = std::env::temp_dir().join(format!("zornov-paint-open-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("handed.png");
        fresh(40, 30);
        crate::paint::apply(&[json!({"op": "export", "path": file.display().to_string()})]).unwrap();
        crate::paint::apply(&[json!({"op": "close"})]).unwrap();
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let _serial = crate::ui_tests::serial();
        let mut app = StudioApp::new(&cc, crate::cli::Cli { fresh: true, ..Default::default() });
        edit_in_paint(&mut app, file.clone());
        assert_eq!(app.screen, Screen::Paint);
        let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(1280.0, 800.0));
        for _ in 0..400 {
            let mut out = ctx.run_ui(egui::RawInput { screen_rect: Some(screen), ..Default::default() }, |ui| show(&mut app, ui));
            let _ = ctx.tessellate(std::mem::take(&mut out.shapes), out.pixels_per_point);
            out.textures_delta.clear();
            if idle() && crate::paint::canvas().doc().is_some_and(|d| d.size.width == 40) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let size = crate::paint::canvas().doc().map(|d| (d.size.width, d.size.height));
        assert_eq!(size, Some((40, 30)), "the handed picture is open");
        assert!(file.exists(), "the original is only read");
        let _ = crate::paint::apply(&[json!({"op": "close"})]);
        let _ = std::fs::remove_dir_all(&dir);
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
