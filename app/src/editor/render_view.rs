// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Viewer frames from `studio-render`.
//!
//! The renderer runs on its own thread (`Previewer`): the viewer asks for
//! the playhead time at its pixel size every frame and shows the newest
//! finished picture. The scene is the model project itself (`ModelScene`);
//! it is replaced whenever the document changes. Media files are opened by
//! the renderer; the sample footage the app generates is handed over as
//! procedural sources so both paths show the same pictures.

use crate::doc::Doc;
use crate::thumbs;
use crate::view::MediaKind;
use eframe::egui::{self, ColorImage, TextureHandle, TextureOptions};
use std::sync::Arc;
use studio_render::colour_space::ColourDesc;
use studio_render::preview::{PreviewRequest, Previewer};
use studio_render::source::{DecodeCache, ProceduralSource, SourceRegistry, SourceSpec, Sources};
use studio_render::yuv::{DecodedPicture, PixelData};
use studio_render::{MediaKey, OutputSettings};

pub struct RenderView {
    previewer: Previewer,
    registry: Arc<SourceRegistry>,
    revision: Option<u64>,
    /// The stretch of time the renderer's copy of the project covers.
    span: Option<(i64, i64)>,
    sequence: u64,
    asked: Option<(i64, (u32, u32), bool)>,
    texture: Option<TextureHandle>,
    /// Frames shown so far; until the first arrives the simple preview
    /// stays on screen.
    pub frames: u64,
    pub last_ms: f32,
    /// The time of the picture on screen.
    pub shown_time: Option<i64>,
    /// Keep the shown frame's pixels (the scopes read them).
    pub keep_pixels: bool,
    /// The shown frame as RGBA8 (width, height, pixels, frame count),
    /// while `keep_pixels` is on.
    pub last_pixels: Option<(u32, u32, Arc<Vec<u8>>, u64)>,
    /// While playing: the next frame, rendered before it is due and held
    /// until the playhead reaches it.
    ahead: Option<studio_render::preview::PreviewFrame>,
    /// Requests dropped in the last second of playback (information only).
    pub dropping: u64,
    window: (f64, u64),
    scene: Option<Arc<studio_render::model_scene::ModelScene>>,
    /// Sources for hit-testing on the UI thread (the renderer owns its own).
    hit_sources: Sources,
    /// The 3D workspace's view: the scene is seen through it when set.
    view: Option<studio_render::viewport::ViewOptions>,
}

/// Draws text generators for `render` with the `text` crate, using the
/// Manrope faces bundled with the app.
struct AppText {
    engine: Arc<studio_text::TextEngine>,
    /// Project canvas height; text sizes are in canvas pixels.
    canvas_h: std::sync::atomic::AtomicU32,
}

impl studio_render::text::TextRenderer for AppText {
    fn raster(&self, runs: &[studio_render::scene::TextRun], style: &studio_render::scene::Params, width: u32, height: u32) -> Option<studio_render::Frame> {
        use studio_render::scene::ParamsExt;
        let text: String = runs.iter().map(|r| r.text.as_str()).collect();
        if text.trim().is_empty() || width == 0 || height == 0 {
            return None;
        }
        // Sizes are canvas units (1/1080 of the height); the text crate
        // converts them for the picture it draws into.
        let _ = self.canvas_h.load(std::sync::atomic::Ordering::Relaxed);
        let c = style.colour("colour", [1.0, 1.0, 1.0, 1.0]);
        // A style from the Text panel, or plain Manrope in the clip colour.
        let mut ts = crate::text_look::style_of(style.text("style", ""), style.f("size", -1.0), style.f("letter_spacing", 0.0), c, style.flag("plate", true));
        let own = style.text("font", "");
        let (own_weight, own_italic) = (style.f("font_weight", -1.0), if style.flag("font_italic_set", false) { Some(style.flag("font_italic", false)) } else { None });
        crate::text_look::set_font(&mut ts, own, own_weight, own_italic);
        let mut img = studio_colour::Image::new(width, height);
        if !style.flag("kerning", true) {
            ts.kerning = false;
        }
        apply_look(&mut ts, style);
        // Rich runs, and text along a path; sizes in canvas pixels become
        // canvas units (1/1080 of the height).
        let canvas_h = self.canvas_h.load(std::sync::atomic::Ordering::Relaxed).max(1) as f32;
        // A counting number: the text is the count at this moment.
        let counted: Option<Vec<studio_render::scene::TextRun>> = Some(style.text("counter", "")).filter(|c| !c.is_empty()).and_then(|c| serde_json::from_str::<studio_mograph::Counter>(c).ok()).map(|c| {
            let text = c.at(style.f("clip_time", 0.0) as f64);
            vec![studio_render::scene::TextRun { text, style: runs.first().map(|r| r.style.clone()).unwrap_or_default() }]
        });
        let block = text_block(counted.as_deref().unwrap_or(runs), style, 1080.0 / canvas_h, &ts.font);
        // Animated text: entrance, exit, loop, custom motions and animators
        // per character, word or line.
        let motion: Option<studio_text::TextMotion> = Some(style.text("motion", "")).filter(|m| !m.is_empty()).and_then(|m| serde_json::from_str(m).ok());
        let (t, dur) = (style.f("clip_time", 0.0), style.f("clip_duration", 1.0));
        let centre = [width as f32 / 2.0, height as f32 / 2.0];
        match &motion {
            Some(mo) => {
                self.engine.render_motion(&mut img, &block, &ts, centre, mo, t, dur);
            }
            None => {
                self.engine.render_onto(&mut img, &block, &ts, centre, &studio_text::Hooks::NONE);
            }
        }
        Some(studio_render::Frame::from_pixels(width, height, img.pixels))
    }
}

/// Font, weight, outline, shadow, case and line spacing set on the title
/// itself (the Inspector's Text section writes these settings): they win
/// over the style preset's.
pub(crate) fn apply_look(ts: &mut studio_text::TextStyle, p: &studio_render::scene::Params) {
    use studio_render::scene::ParamsExt;
    use studio_text::{Case, Rgba, Shadow, Stroke};
    let rgba = |k: &str, d: [f32; 4]| {
        let c = p.colour(k, d);
        Rgba { r: c[0], g: c[1], b: c[2], a: c[3] }
    };
    let family = p.text("font", "");
    if !family.is_empty() {
        ts.font.family = family.to_owned();
    }
    if p.get("weight").is_some() {
        ts.font.weight = p.f("weight", 400.0).round().clamp(100.0, 900.0) as u16;
    }
    if p.get("italic").is_some() {
        ts.font.italic = p.flag("italic", false);
    }
    if p.get("line_spacing").is_some() {
        ts.line_spacing = p.f("line_spacing", 0.0);
    }
    if p.get("outline").is_some() {
        let w = p.f("outline_width", 4.0);
        ts.strokes = if w > 0.0 { vec![Stroke::new(rgba("outline", [0.0, 0.0, 0.0, 1.0]), w)] } else { Vec::new() };
    }
    if p.get("shadow").is_some() {
        let mut sh = Shadow::new(rgba("shadow", [0.0, 0.0, 0.0, 1.0]), p.f("shadow_blur", 8.0), p.f("shadow_distance", 6.0), p.f("shadow_angle", 90.0));
        sh.opacity = p.f("shadow_opacity", 0.6).clamp(0.0, 1.0);
        ts.shadow = Some(sh);
    }
    match p.text("case", "") {
        "upper" => ts.case = Case::Upper,
        "lower" => ts.case = Case::Lower,
        "title" => ts.case = Case::Title,
        "as_typed" => ts.case = Case::AsTyped,
        _ => {}
    }
    if p.get("underline").is_some() {
        ts.underline = p.flag("underline", false);
    }
}

/// A title's runs as a text block: each run's own bold, italic, colour,
/// size (× the title's), weight, tracking, baseline shift and font; a path
/// to follow when the title has one. `unit` turns canvas pixels into
/// canvas units.
fn text_block(runs: &[studio_render::scene::TextRun], style: &studio_render::scene::Params, unit: f32, base: &studio_text::FontRequest) -> studio_text::TextBlock {
    use studio_render::scene::{Param, ParamsExt};
    use studio_text::{Span, SpanStyle};
    let styled = runs.iter().any(|r| !r.style.is_empty());
    let mut block = if !styled {
        studio_text::TextBlock::plain(runs.iter().map(|r| r.text.as_str()).collect::<String>())
    } else {
        let spans = runs
            .iter()
            .map(|r| {
                let st = &r.style;
                let flag = |k: &str| st.get(k).and_then(Param::as_bool);
                let num = |k: &str| st.get(k).and_then(Param::as_f32);
                let fill = st.get("colour").and_then(Param::as_colour).map(|c| studio_text::Fill::Solid(studio_text::Rgba { r: c[0], g: c[1], b: c[2], a: c[3] }));
                let weight = num("weight").map(|w| w.round().clamp(100.0, 1000.0) as u16);
                let font = match st.get("font") {
                    Some(Param::Text(f)) | Some(Param::Choice(f)) if !f.trim().is_empty() => Some(studio_text::FontRequest::new(f.trim(), weight.unwrap_or(base.weight), flag("italic").unwrap_or(base.italic))),
                    _ => None,
                };
                Span::styled(
                    r.text.clone(),
                    SpanStyle {
                        font,
                        bold: flag("bold"),
                        italic: flag("italic"),
                        size_scale: num("size"),
                        fill,
                        underline: flag("underline"),
                        strikethrough: flag("strikethrough"),
                        weight,
                        letter_spacing: num("letter_spacing"),
                        baseline_shift: num("baseline_shift").map(|b| b * unit),
                    },
                )
            })
            .collect();
        studio_text::TextBlock::spans(spans)
    };
    let path = style.text("text_path", "");
    if !path.is_empty()
        && let Ok(mut tp) = serde_json::from_str::<studio_text::TextPath>(path)
    {
        for q in &mut tp.points {
            *q = [q[0] * unit, q[1] * unit];
        }
        tp.offset *= unit;
        tp.start = style.f("path_start", 0.5);
        block.layout.path = Some(tp);
        block.layout.align = match style.text("align", "centre") {
            "left" => studio_text::Align::Left,
            "right" => studio_text::Align::Right,
            _ => studio_text::Align::Centre,
        };
    }
    block
}

/// The installed text renderer, so its canvas size can follow the project.
static TEXT: std::sync::OnceLock<Arc<AppText>> = std::sync::OnceLock::new();

fn install_text() -> &'static Arc<AppText> {
    TEXT.get_or_init(|| {
        let t = Arc::new(AppText { engine: crate::text_look::engine().clone(), canvas_h: 1080.into() });
        studio_render::text::set_text_renderer(t.clone());
        t
    })
}

/// How far either side of the playhead the renderer's copy of the project
/// reaches (playback renders ahead; a new copy is cut before the edge).
const MARGIN: i64 = 120_000_000;

/// `p` with the active sequence cut down to the clips that touch
/// `from..to` (and the last one, so its length stays); everything else
/// (other sequences, media, settings) whole.
pub fn around(p: &studio_model::Project, from: i64, to: i64) -> studio_model::Project {
    use studio_model as m;
    let Some(seq) = p.sequence(p.active_sequence) else { return p.clone() };
    if seq.clips.len() < 2_000 {
        return p.clone();
    }
    let last = seq.clips.iter().max_by_key(|c| c.end()).map(|c| c.id);
    let keep = |c: &m::Clip| (c.end() > from && c.start < to) || Some(c.id) == last;
    let clips: Vec<m::Clip> = seq.clips.iter().filter(|c| keep(c)).cloned().collect();
    let kept: std::collections::HashSet<m::ClipId> = clips.iter().map(|c| c.id).collect();
    let transitions = seq.transitions.iter().filter(|t| kept.contains(&t.left) && kept.contains(&t.right)).cloned().collect();
    let mut cut = Some(m::Sequence { id: seq.id, name: seq.name.clone(), lanes: seq.lanes.clone(), clips, transitions, markers: seq.markers.clone(), extra: seq.extra.clone() });
    m::Project {
        sequences: p.sequences.iter().map(|s| if s.id == seq.id { cut.take().unwrap_or_else(|| s.clone()) } else { s.clone() }).collect(),
        media: p.media.clone(),
        presets: p.presets.clone(),
        // The plan does not draw.
        story: Default::default(),
        settings: p.settings.clone(),
        active_sequence: p.active_sequence,
        next_id: p.next_id,
        extra: p.extra.clone(),
    }
}

/// Tell `registry` where each media item's pictures come from: files by
/// path, generated sample footage as procedural sources; with the media's
/// input colour space, and its proxy when the project uses proxies.
pub fn register_sources(doc: &Doc, registry: &SourceRegistry) {
    register_sources_with(doc, registry, crate::services::proxies::enabled(&doc.model));
}

/// [`register_sources`] for export: the original files, never proxies
/// (a proxy stands in only when its original is missing).
pub fn register_sources_export(doc: &Doc, registry: &SourceRegistry) {
    register_sources_with(doc, registry, false);
}

fn register_sources_with(doc: &Doc, registry: &SourceRegistry, proxies: bool) {
    register_files(doc, registry);
    for item in &doc.model.media {
        let key = MediaKey(item.id.0);
        registry.set_input(key, studio_render::model_scene::media_input(&doc.model, item));
        registry.set_proxy(key, crate::services::proxies::proxy_for(item, proxies));
    }
}

fn register_files(doc: &Doc, registry: &SourceRegistry) {
    // Cut-out mattes, read from their files a frame at a time.
    crate::services::cutout::register_mattes(&doc.model, registry);
    // Graphs read matte and depth files the same way.
    crate::services::vfx::register();
    for mi in &doc.project.media {
        let key = MediaKey(mi.id as u64);
        match &mi.path {
            // Vector stickers (the emoji sets are SVG): drawn once to a
            // still the renderer can use like any picture.
            Some(p) if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("svg")) => {
                if let Some((w, h, bytes)) = rasterise_svg(p, mi.width.max(mi.height).clamp(256, 2048)) {
                    let bytes = std::sync::Arc::new(bytes);
                    let source = ProceduralSource::new(w, h, 30.0, 1, move |_| DecodedPicture { width: w, height: h, data: PixelData::Rgba8(bytes.as_ref().clone()), colour: ColourDesc::SRGB8 });
                    registry.register(key, SourceSpec::Procedural(source));
                }
            }
            Some(p) => registry.register(key, SourceSpec::Path(p.clone())),
            None if mi.kind != MediaKind::Audio => {
                let (w, h) = (mi.width.max(16), mi.height.max(16));
                // Generate at a modest size; the renderer scales.
                let (gw, gh) = if w >= h { (640, (640 * h / w).max(2)) } else { ((640 * w / h).max(2), 640) };
                let frames = ((mi.dur as f64 / 1e6) * 30.0).ceil().max(1.0) as u64;
                let seed = mi.seed;
                let source = ProceduralSource::new(gw, gh, 30.0, frames, move |i| {
                    let img = thumbs::sample_frame(seed, i as f32 / frames as f32, gw as usize, gh as usize);
                    let bytes: Vec<u8> = img.pixels.iter().flat_map(|p| p.to_srgba_unmultiplied()).collect();
                    DecodedPicture { width: gw, height: gh, data: PixelData::Rgba8(bytes), colour: ColourDesc::SRGB8 }
                });
                registry.register(key, SourceSpec::Procedural(source));
            }
            None => {}
        }
    }
}

/// An SVG drawn with its longer side `long` pixels: (width, height,
/// straight-alpha RGBA).
fn rasterise_svg(path: &std::path::Path, long: u32) -> Option<(u32, u32, Vec<u8>)> {
    let data = std::fs::read(path).ok()?;
    let tree = resvg::usvg::Tree::from_data(&data, &resvg::usvg::Options::default()).ok()?;
    let size = tree.size();
    let scale = long as f32 / size.width().max(size.height()).max(1.0);
    let (w, h) = (((size.width() * scale).round() as u32).max(1), ((size.height() * scale).round() as u32).max(1));
    let mut pixmap = resvg::tiny_skia::Pixmap::new(w, h)?;
    resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
    // tiny-skia holds premultiplied pixels.
    let mut out = pixmap.take();
    for px in out.chunks_exact_mut(4) {
        let a = px[3] as u32;
        if a > 0 && a < 255 {
            for c in &mut px[..3] {
                *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
    Some((w, h, out))
}

/// Install the app's text renderer (idempotent) for a project of the given
/// canvas height.
pub fn install_text_renderer(canvas_h: u32) {
    install_text().canvas_h.store(canvas_h, std::sync::atomic::Ordering::Relaxed);
}

impl RenderView {
    pub fn new() -> Self {
        install_text();
        let registry = SourceRegistry::new();
        let cache = DecodeCache::new(512 << 20);
        let renderer = studio_render::best_renderer(Sources::new(registry.clone(), cache));
        let hit_sources = Sources::new(registry.clone(), DecodeCache::new(32 << 20));
        Self { previewer: Previewer::new(renderer), registry, revision: None, span: None, sequence: 0, asked: None, texture: None, frames: 0, last_ms: 0.0, shown_time: None, keep_pixels: false, last_pixels: None, ahead: None, dropping: 0, window: (0.0, 0), scene: None, hit_sources, view: None }
    }

    /// Hand the current project to the renderer when it changed. Follows
    /// `Doc::edits`, not the view's revision: playing over keyframes moves
    /// the revision every frame, and a new scene each frame made the
    /// renderer draw every picture several times over.
    pub fn sync(&mut self, doc: &Doc, time: i64) {
        // The renderer gets the part of the project around `time`: a copy
        // of a whole long project on every edit (every drag step) was most
        // of a frame. A new part is cut when the project changes or the
        // playhead nears the edge of this one.
        let inside = self.span.is_some_and(|(a, b)| time >= a + MARGIN / 4 && time <= b - MARGIN / 4);
        if self.revision == Some(doc.edits) && inside {
            return;
        }
        let fresh = self.revision != Some(doc.edits);
        self.revision = Some(doc.edits);
        self.span = Some((time - MARGIN, time + MARGIN));
        if fresh {
            install_text().canvas_h.store(doc.project.height, std::sync::atomic::Ordering::Relaxed);
            register_sources(doc, &self.registry);
        }
        let scene = Arc::new(studio_render::model_scene::ModelScene::new(Arc::new(around(&doc.model, time - MARGIN, time + MARGIN))));
        self.previewer.set_scene(self.seen(scene.clone()));
        self.scene = Some(scene);
        self.asked = None;
    }

    /// The scene as the renderer gets it: through the workspace's view
    /// when there is one.
    fn seen(&self, scene: Arc<studio_render::model_scene::ModelScene>) -> Arc<dyn studio_render::scene::Scene> {
        match &self.view {
            Some(o) => Arc::new(studio_render::viewport::ViewScene::new(scene, o.clone())),
            None => scene,
        }
    }

    /// Sees the scene through the 3D workspace's view (`None`: the final
    /// picture). A new view is rendered at once.
    pub fn set_view(&mut self, view: Option<studio_render::viewport::ViewOptions>) {
        if self.view == view {
            return;
        }
        self.view = view;
        if let Some(s) = self.scene.clone() {
            self.previewer.set_scene(self.seen(s));
            self.asked = None;
        }
    }

    /// The renderer's copy of the project.
    pub fn scene(&self) -> Option<Arc<studio_render::model_scene::ModelScene>> {
        self.scene.clone()
    }

    /// Every visible layer's real bounds at `t` (canvas pixels), topmost
    /// first: what the canvas uses for picking and handles.
    pub fn bounds(&mut self, t: i64) -> Option<Vec<(u32, [[f32; 2]; 4])>> {
        let scene = self.scene.as_ref()?;
        let mut b = studio_render::hit::layer_bounds(scene.as_ref(), &mut self.hit_sources, t);
        b.sort_by_key(|l| std::cmp::Reverse(l.order));
        Some(b.into_iter().map(|l| (l.clip as u32, l.quad)).collect())
    }

    /// Count dropped requests per second while playing.
    pub fn track_drops(&mut self, now: f64, playing: bool) {
        let (dropped, _) = self.previewer.stats();
        if !playing {
            self.window = (now, dropped);
            self.dropping = 0;
        } else if now - self.window.0 >= 1.0 {
            self.dropping = dropped - self.window.1;
            self.window = (now, dropped);
        }
    }

    /// The newest rendered frame for `time` at `size` pixels, if any.
    /// `fast`: scrubbing, so key frames may stand in for exact ones; the
    /// exact frame is asked for again when scrubbing stops. `playing`: the
    /// frame after this one is rendered while this one is on screen, so
    /// each picture is ready the moment it is due instead of a render
    /// later.
    pub fn frame(&mut self, ctx: &egui::Context, time: i64, size: (u32, u32), fast: bool, playing: bool) -> Option<egui::TextureId> {
        let step = self.scene.as_ref().map_or(33_333, |s| studio_render::scene::Scene::canvas(s.as_ref()).frame_duration()).max(1);
        // Within a few microseconds counts as the same frame (times are
        // whole microseconds on a fractional grid).
        let same = |a: i64, b: i64| (a - b).abs() < 8;
        // The frame rendered ahead is due: on screen now.
        if let Some(f) = self.ahead.take() {
            if f.time <= time + 8 {
                self.show(ctx, f);
            } else if f.time > time && f.time - time <= 2 * step && playing {
                self.ahead = Some(f);
            }
        }
        let on_screen = self.shown_time.is_some_and(|t| same(t, time));
        let next = time + step;
        let want = if on_screen && playing && !fast {
            // This frame is up: start on the next one.
            (!self.ahead.as_ref().is_some_and(|f| same(f.time, next))).then_some(next)
        } else {
            Some(time)
        };
        if let Some(t) = want {
            if self.asked.is_none_or(|(at, sz, fa)| !same(at, t) || sz != size || fa != fast) {
                self.sequence += 1;
                crate::bench::requested(self.sequence);
                self.previewer.request(PreviewRequest { time: t, size, sequence: self.sequence, output: OutputSettings { premultiplied: false, ..Default::default() }, fast });
                self.asked = Some((t, size, fast));
            }
        }
        if let Some(f) = self.previewer.poll() {
            if playing && f.time > time + 8 {
                // Early: held until its time.
                self.ahead = Some(f);
            } else {
                self.show(ctx, f);
            }
        }
        if !self.shown_time.is_some_and(|t| same(t, time)) || self.ahead.is_some() {
            // Keep polling while a frame is on its way or waiting its turn.
            ctx.request_repaint();
        }
        self.texture.as_ref().map(|t| t.id())
    }

    fn show(&mut self, ctx: &egui::Context, f: studio_render::preview::PreviewFrame) {
        crate::bench::shown(f.sequence);
        let img = ColorImage::from_rgba_unmultiplied([f.width as usize, f.height as usize], &f.rgba);
        match &mut self.texture {
            Some(t) => t.set(img, TextureOptions::LINEAR),
            None => self.texture = Some(ctx.load_texture("render-preview", img, TextureOptions::LINEAR)),
        }
        self.frames += 1;
        self.shown_time = Some(f.time);
        self.last_ms = f.render_time.as_secs_f32() * 1000.0;
        self.last_pixels = self.keep_pixels.then(|| (f.width, f.height, Arc::new(f.rgba), self.frames));
    }
}

#[cfg(test)]
mod tests {
    /// Plays a file in the viewer as the app does (a repaint every display
    /// refresh, the playhead moved by each frame's real time) and reports
    /// how evenly its frames reach the screen. Headless: no window.
    /// `STUDIO_CADENCE_FILE=clip.mp4 cargo test --release cadence -- --nocapture`
    #[test]
    fn cadence() {
        let Ok(file) = std::env::var("STUDIO_CADENCE_FILE") else { return };
        let hz: f64 = std::env::var("STUDIO_CADENCE_HZ").ok().and_then(|v| v.parse().ok()).unwrap_or(120.0);
        let mut doc = crate::services::project::new_project(&crate::services::project::CANVASES[0]);
        crate::services::project::import_to_timeline(&mut doc, &[std::path::PathBuf::from(&file)]).unwrap();
        let fps = doc.project.fps as f64;
        let ctx = eframe::egui::Context::default();
        let mut v = super::RenderView::new();
        v.sync(&doc, 0);
        // Paused on the first frame, as before pressing Play.
        for _ in 0..200 {
            let _ = v.frame(&ctx, 0, (1280, 720), false, false);
            if v.shown_time.is_some() { break; }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let tick = std::time::Duration::from_secs_f64(1.0 / hz);
        let start = std::time::Instant::now();
        let mut last = start;
        let mut playhead = 0i64;
        let (mut wrong, mut ticks, mut ui_worst, mut ui_total) = (0u32, 0u32, 0f64, 0f64);
        let mut holds: Vec<u32> = Vec::new();
        let mut shown_prev = None;
        let mut lag_total = 0f64;
        for i in 1..(hz as u64 * 6) {
            let due = start + tick * i as u32;
            if let Some(w) = due.checked_duration_since(std::time::Instant::now()) { std::thread::sleep(w); }
            let now = std::time::Instant::now();
            playhead += ((now - last).as_secs_f64().min(0.1) * 1e6) as i64;
            last = now;
            let t0 = std::time::Instant::now();
            // On the frame grid, as the viewer asks.
            let asked = ((playhead as f64 * fps / 1e6).floor() * 1e6 / fps) as i64;
            let _ = v.frame(&ctx, asked, (1280, 720), false, true);
            let ui = t0.elapsed().as_secs_f64() * 1000.0;
            ui_worst = ui_worst.max(ui);
            ui_total += ui;
            let shown = (v.shown_time.unwrap_or(0) as f64 / 1e6 * fps + 1e-3).floor() as i64;
            let ideal = (playhead as f64 / 1e6 * fps + 1e-3).floor() as i64;
            ticks += 1;
            if shown != ideal { wrong += 1; }
            lag_total += (ideal - shown) as f64;
            if shown_prev == Some(shown) { *holds.last_mut().unwrap() += 1 } else { holds.push(1) }
            shown_prev = Some(shown);
        }
        let ideal_hold = hz / fps;
        let (_, rendered) = v.previewer.stats();
        let uneven = holds.iter().filter(|h| (**h as f64 - ideal_hold).abs() > 1.0).count();
        let mut hist = std::collections::BTreeMap::new();
        for h in &holds { *hist.entry(*h).or_insert(0) += 1; }
        eprintln!(
            "CADENCE {file} {fps}fps on {hz}Hz: distinct frames {} of {:.0}, wrong frame {:.0}% of refreshes, mean lag {:.2} frames, holds {hist:?} (ideal {ideal_hold:.1}), uneven {uneven}, renders {rendered}, ui frame() avg {:.2} ms worst {:.1} ms",
            holds.len(),
            6.0 * fps,
            wrong as f64 * 100.0 / ticks as f64,
            lag_total / ticks as f64,
            ui_total / ticks as f64,
            ui_worst
        );
    }

    use studio_render::text::TextRenderer;

    /// Titles are drawn centred in the picture the renderer asks for.
    #[test]
    fn text_is_centred() {
        let t = super::install_text().clone();
        let runs = vec![studio_render::scene::TextRun { text: "Lisbon, day one".into(), style: Default::default() }];
        let f = t.raster(&runs, &Default::default(), 600, 338).expect("text drawn");
        let (mut x0, mut x1) = (f.width, 0);
        for y in 0..f.height {
            for x in 0..f.width {
                if f.get(x, y)[3] > 0.2 {
                    x0 = x0.min(x);
                    x1 = x1.max(x);
                }
            }
        }
        let centre = (x0 + x1) as f32 / 2.0;
        assert!((centre - 300.0).abs() < 12.0, "text spans {x0}..{x1}");
    }

    #[test]
    fn a_titles_own_font_outline_and_shadow_win_over_its_style() {
        use studio_render::scene::Param;
        let mut ts = crate::text_look::style_of("", -1.0, 0.0, [1.0; 4], true);
        let mut p = studio_render::scene::Params::new();
        p.insert("font".into(), Param::Choice("Georgia".into()));
        p.insert("weight".into(), Param::Float(300.0));
        p.insert("outline".into(), Param::Colour([1.0, 0.0, 0.0, 1.0]));
        p.insert("outline_width".into(), Param::Float(6.0));
        p.insert("shadow".into(), Param::Colour([0.0, 0.0, 0.0, 1.0]));
        p.insert("shadow_blur".into(), Param::Float(12.0));
        p.insert("case".into(), Param::Choice("upper".into()));
        super::apply_look(&mut ts, &p);
        assert_eq!((ts.font.family.as_str(), ts.font.weight), ("Georgia", 300));
        assert_eq!(ts.strokes.len(), 1);
        assert_eq!(ts.strokes[0].width, 6.0);
        assert_eq!(ts.shadow.as_ref().map(|s| s.blur), Some(12.0));
        assert_eq!(ts.case, studio_text::Case::Upper);
        p.insert("outline_width".into(), Param::Float(0.0));
        super::apply_look(&mut ts, &p);
        assert!(ts.strokes.is_empty(), "width 0 is no outline");
    }
}
