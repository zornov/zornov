// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Export dialog: presets on the left, settings on the right, size
//! estimate and progress with ETA in the footer. Exports run as background
//! jobs, so the dialog can be closed while one runs.

use crate::app::{Jobs, Overlay, StudioApp};
#[cfg(feature = "render")]
use crate::export_run;
use crate::formats::Format;
use crate::icons::{self, Icon};
use crate::kit::{self, Button, FieldRow, Segment, ShowNoted};
use crate::view::{self, SEC};
use crate::theme;
use eframe::egui::{self, Align2, CornerRadius, Rect, Sense, Vec2};

pub struct Preset {
    pub name: &'static str,
    pub sub: &'static str,
    pub icon: Icon,
    pub res: usize,
    /// Index into `export_run::Format::ALL`; `None` when not available yet.
    pub codec: Option<usize>,
    pub quality: f32,
}

pub const PRESETS: &[Preset] = &[
    Preset { name: "Social 1080p", sub: "H.264 · 1920 × 1080", icon: Icon::MonitorPlay, res: 2, codec: Some(0), quality: 70.0 },
    Preset { name: "4K master", sub: "H.264 · 3840 × 2160", icon: Icon::Monitor, res: 0, codec: Some(0), quality: 90.0 },
    Preset { name: "Vertical", sub: "H.264 · 1080 × 1920", icon: Icon::DeviceMobile, res: 5, codec: Some(0), quality: 70.0 },
    Preset { name: "GIF", sub: "Looping · 720p", icon: Icon::Image, res: 3, codec: Some(3), quality: 50.0 },
    Preset { name: "Intermediate", sub: "Zornov codec · CLV", icon: Icon::FilmSlate, res: 4, codec: Some(1), quality: 95.0 },
    Preset { name: "PNG sequence", sub: "Lossless stills", icon: Icon::Stack, res: 4, codec: Some(2), quality: 100.0 },
    Preset { name: "Web (AV1)", sub: "AV1 · small file, slower", icon: Icon::MonitorPlay, res: 2, codec: Some(18), quality: 70.0 },
    Preset { name: "Audio only", sub: "AAC · M4A", icon: Icon::MusicNote, res: 2, codec: Some(5), quality: 80.0 },
];

const RES: [&str; 7] = ["3840 × 2160", "2560 × 1440", "1920 × 1080", "1280 × 720", "Project size", "1080 × 1920", "1080 × 1080"];
const RES_PX: [(u32, u32); 7] = [(3840, 2160), (2560, 1440), (1920, 1080), (1280, 720), (0, 0), (1080, 1920), (1080, 1080)];
const FPS: [&str; 6] = ["Project rate", "24 fps", "25 fps", "30 fps", "50 fps", "60 fps"];


pub struct ExportState {
    pub preset: usize,
    pub name: String,
    pub folder: String,
    pub res: usize,
    pub fps: usize,
    pub codec: usize,
    pub quality: f32,
    pub range: usize,
    /// H.264 bitrate in Mb/s; `None` follows the resolution table.
    pub bitrate: Option<f32>,
    /// AV1: how hard the encoder searches.
    pub av1_speed: crate::formats::Av1Speed,
    pub job: Option<u64>,
    /// What stops the export under the licences, if something does.
    pub rights: Option<String>,
    /// The project this state belongs to (see [`sync_project`]).
    pub project_key: String,
}

/// Suggested H.264 bitrate in Mb/s for a size and rate (by
/// resolution table).
pub fn suggested_mbps(w: u32, h: u32, fps: f32) -> f32 {
    let px = w as f32 * h as f32;
    let base = if px <= 1280.0 * 720.0 { 6.0 } else if px <= 1920.0 * 1080.0 { 12.0 } else if px <= 2560.0 * 1440.0 { 20.0 } else { 45.0 };
    if fps > 40.0 { base * 1.5 } else { base }
}

impl Default for ExportState {
    fn default() -> Self {
        Self {
            preset: 0,
            name: String::new(),
            folder: studio_paths::videos().display().to_string(),
            res: 2,
            fps: 0,
            codec: 0,
            quality: 70.0,
            range: 0,
            bitrate: None,
            av1_speed: Default::default(),
            job: None,
            rights: None,
            project_key: String::new(),
        }
    }
}

fn bitrate_mbps(s: &ExportState, w: u32, h: u32, fps: f32) -> f32 {
    // Rough bits-per-pixel model, scaled by the quality slider.
    let bpp = match Format::ALL[s.codec.min(Format::ALL.len() - 1)] {
        Format::Clv => 0.9,
        Format::Png => 2.0,
        Format::Gif => 0.25,
        Format::H264 => 0.1,
        Format::Hevc10 => 0.07,
        // Constant quantiser, no rate control: a rough guide only.
        Format::Av1 => 0.45,
        Format::M4a => 0.0,
        Format::ProRes422Hq | Format::DnxHr | Format::DnxHrHq => 1.1,
        Format::DnxHrSq => 0.7,
        Format::DnxHrLb => 0.25,
        Format::DnxHr444 => 2.2,
        Format::ProRes4444 => 1.7,
        Format::ProResProxy => 0.25,
        Format::Exr => 3.0,
        Format::Dpx => 4.0,
        Format::Tiff16 | Format::Png16 => 4.5,
    } * (0.35 + s.quality / 100.0 * 1.3);
    w as f32 * h as f32 * fps * bpp / 1e6
}

/// Which project an export state belongs to: its folder and name.
/// Whether the sequence has anything to mix: an audio clip, or a picture clip whose file carries sound.
fn has_sound(p: &view::Project) -> bool {
    p.clips.iter().any(|c| {
        c.audio.enabled
            && match c.kind {
                view::ClipKind::Audio => true,
                view::ClipKind::Video => c.media.and_then(|m| p.media.iter().find(|i| i.id == m)).is_some_and(|i| i.kind != view::MediaKind::Image),
                _ => false,
            }
    })
}

fn project_key(doc: &crate::doc::Doc) -> String {
    format!("{}|{}", doc.folder.as_ref().map(|f| f.root().display().to_string()).unwrap_or_default(), doc.project.name)
}

/// A different project than the one this state was made for starts over: the file name, the
/// licence problem and the running job of the last project do not carry across.
fn sync_project(s: &mut ExportState, doc: &crate::doc::Doc) {
    let key = project_key(doc);
    if s.project_key.is_empty() {
        // First look: the state is the one the screen was opened with.
        s.project_key = key;
    } else if s.project_key != key {
        *s = ExportState { project_key: key, ..Default::default() };
    }
}

/// "Export": the licences are checked afresh every time; true when the export may start.
fn press_export(s: &mut ExportState, blocked: impl FnOnce() -> Option<String>) -> bool {
    s.rights = blocked();
    s.rights.is_none()
}

/// "Export anyway": lets this one export start. Nothing is remembered, so the next one is
/// checked again.
fn press_export_anyway(s: &mut ExportState) -> bool {
    s.rights = None;
    true
}

pub fn show(app: &mut StudioApp, ctx: &egui::Context) {
    sync_project(&mut app.export, &app.doc);
    if app.export.name.is_empty() {
        app.export.name = app.doc.project.name.clone();
    }
    // A tall sheet over the middle half of the window, below the top bar.
    let screen = ctx.content_rect();
    let w = (screen.width() * 0.49).clamp(720.0, 1100.0).min(screen.width() - 24.0);
    let rect = Rect::from_min_max(egui::pos2(screen.center().x - w / 2.0, screen.top() + crate::editor::TOPBAR_H - 20.0), egui::pos2(screen.center().x + w / 2.0, screen.bottom()));
    let out = kit::sheet(ctx, egui::Id::unique("export-dialog"), rect, |ui| body(app, ui));
    if out.close {
        app.overlay = Overlay::None;
    }
}

fn body(app: &mut StudioApp, ui: &mut egui::Ui) {
    let t = theme::tokens(ui.ctx());
    let r = ui.max_rect();
    let left_w = (r.width() * 0.4).round();
    let left = Rect::from_min_max(r.min, egui::pos2(r.left() + left_w, r.bottom() - 64.0));
    let right = Rect::from_min_max(egui::pos2(left.right(), r.top() + 24.0), egui::pos2(r.right(), r.bottom() - 64.0));
    let footer = Rect::from_min_max(egui::pos2(r.left(), r.bottom() - 64.0), r.max);
    kit::vline(ui, left.right(), r.top()..=footer.top());
    kit::hline(ui, r.left()..=r.right(), footer.top());

    // Left column: the first frame, the project, then the presets.
    let proj = &app.doc.project;
    let aspect = proj.width as f32 / proj.height.max(1) as f32;
    let pw = left.width() - 16.0;
    let preview = Rect::from_min_size(left.min + Vec2::new(8.0, 8.0), Vec2::new(pw, (pw / aspect.max(0.5)).min(pw * 0.75)));
    ui.painter().rect_filled(preview, CornerRadius::same(4), egui::Color32::BLACK);
    let seed = proj.clips.iter().filter(|c| c.kind == view::ClipKind::Video).min_by_key(|c| c.start).and_then(|c| c.media).and_then(|m| proj.media(m)).map(|m| m.seed);
    if let Some(seed) = seed {
        let fit = (preview.width() / aspect).min(preview.height());
        let fr = Rect::from_center_size(preview.center(), Vec2::new(fit * aspect, fit));
        let ppp = ui.ctx().pixels_per_point();
        if let Some(tex) = app.thumbs.scene(seed, 0.0, (fr.width() * ppp) as u16, (fr.height() * ppp) as u16) {
            ui.painter().image(tex, fr, Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
        }
    }
    let proj = &app.doc.project;
    // One line each: a long name ends in an ellipsis, whole on hover.
    let name_cut = kit::paint_text_clipped(ui.painter(), egui::pos2(left.left() + 8.0, preview.bottom() + 18.0), Align2::LEFT_CENTER, &proj.name, theme::body(), t.text, left.width() - 16.0);
    if name_cut {
        ui.interact(Rect::from_min_size(egui::pos2(left.left() + 8.0, preview.bottom() + 8.0), Vec2::new(left.width() - 16.0, 20.0)), egui::Id::unique("export-name"), Sense::hover()).on_hover_text(&proj.name);
    }
    kit::paint_text_clipped(ui.painter(), egui::pos2(left.left() + 8.0, preview.bottom() + 38.0), Align2::LEFT_CENTER, &format!("{} × {}  ·  {}  ·  {} fps", proj.width, proj.height, view::format_duration(proj.duration()), proj.fps), theme::body(), t.text_2, left.width() - 16.0);
    let left = Rect::from_min_max(egui::pos2(left.left(), preview.bottom() + 54.0), left.max);

    // Presets.
    let mut col = ui.new_child(egui::UiBuilder::new().max_rect(left.shrink2(Vec2::new(8.0, 4.0))));
    col.set_clip_rect(left.intersect(ui.clip_rect()));
    kit::menu_heading(&mut col, "Presets");
    let s = &mut app.export;
    // A short window: the presets scroll above the footer.
    egui::ScrollArea::vertical().id_salt("export-presets").auto_shrink([false, false]).show_v(&mut col, |pui| {
        for (i, p) in PRESETS.iter().enumerate() {
            let (rr, resp) = pui.allocate_exact_size(Vec2::new(pui.available_width(), 38.0), Sense::click());
            let available = p.codec.is_some();
            let st = kit::State::from_response(&pui, &resp, None);
            let sel = s.preset == i && available;
            let pp = pui.painter();
            let cr = CornerRadius::same(theme::radius::CONTROL);
            if sel {
                pp.rect_filled(rr, cr, t.accent_soft);
            } else if st.hover > 0.0 {
                pp.rect_filled(rr, cr, theme::alpha(t.raised, st.hover));
            }
            icons::paint(pp, Rect::from_center_size(egui::pos2(rr.left() + 18.0, rr.center().y), Vec2::splat(16.0)), p.icon, 16.0, if sel { t.accent } else { t.text_2 });
            pp.text(egui::pos2(rr.left() + 38.0, rr.center().y - 8.0), Align2::LEFT_CENTER, p.name, theme::control_medium(), if available { t.text } else { t.text_2 });
            pp.text(egui::pos2(rr.left() + 38.0, rr.center().y + 9.0), Align2::LEFT_CENTER, p.sub, theme::caption(), t.text_2);
            if st.focused {
                kit::focus_ring(pp, rr, cr, &t);
            }
            let name = p.name;
            resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::RadioButton, true, sel, name));
            if resp.clicked() {
                if let Some(c) = p.codec {
                    s.preset = i;
                    s.res = p.res;
                    s.codec = c;
                    s.quality = p.quality;
                } else {
                    kit::push(pui.ctx(), "Audio export arrives with the audio engine", None, Some(Icon::Info));
                }
            }
            pui.add_space(2.0);
        }
    });

    // Settings.
    let proj = &app.doc.project;
    let (pw, ph) = (proj.width, proj.height);
    let pfps = proj.fps;
    let dur = proj.duration();
    let has_range = proj.range.is_some();
    let range_len = proj.range.map_or(dur, |(a, b)| b - a);
    let mut sui = ui.new_child(egui::UiBuilder::new().max_rect(right.shrink2(Vec2::new(20.0, 16.0))));
    sui.spacing_mut().interact_size.y = 26.0;
    sui.spacing_mut().item_spacing.y = 6.0;
    let s = &mut app.export;
    let busy = s.job.and_then(|id| app.jobs.get(id)).is_some_and(|j| !j.done && !j.cancelled);
    sui.add_enabled_ui(!busy, |ui| {
        let w = ui.available_width();
        kit::labeled_row(ui, "File name", |ui| {
            let ext = Format::ALL[s.codec.min(Format::ALL.len() - 1)].extension();
            // The field takes the row less the extension after it.
            let ext_w = kit::text_width(ui, ext, theme::body()) + ui.spacing().item_spacing.x;
            let fw = (ui.available_width() - ext_w - 4.0).max(80.0);
            kit::text_field(ui, &mut s.name, "Name", None, fw, None);
            ui.label(egui::RichText::new(ext).font(theme::body()).color(t.text_2));
        });
        kit::labeled_row(ui, "Export to", |ui| {
            let (rr, _) = ui.allocate_exact_size(Vec2::new(ui.available_width() - 92.0, 28.0), Sense::hover());
            ui.painter().rect_filled(rr, CornerRadius::same(6), kit::field_fill(ui));
            icons::paint(ui.painter(), Rect::from_center_size(egui::pos2(rr.left() + 16.0, rr.center().y), Vec2::splat(16.0)), Icon::Folder, 16.0, t.text_2);
            let short = studio_paths::tilde(std::path::Path::new(&s.folder));
            kit::paint_text_clipped(ui.painter(), egui::pos2(rr.left() + 32.0, rr.center().y), Align2::LEFT_CENTER, &short, theme::control(), t.text, rr.width() - 40.0);
            if Button::secondary("Change…").compact().show(ui).clicked() {
                if let Some(dir) = rfd::FileDialog::new().set_title("Export to").set_directory(&s.folder).pick_folder() {
                    s.folder = dir.display().to_string();
                }
            }
        });
        ui.add_space(6.0);
        let menu_w = (w - kit::LABEL_W - 8.0).min(260.0);
        kit::labeled_row(ui, "Format", |ui| {
            let labels: Vec<&str> = Format::ALL.iter().map(|f| f.label()).collect();
            if let Some(i) = kit::menu_button(ui, "exp-codec", s.codec, &labels, menu_w, None) {
                s.codec = i;
            }
        });
        ui.add_enabled_ui(true, |ui| {
            kit::labeled_row(ui, "Resolution", |ui| {
                if let Some(i) = kit::menu_button(ui, "exp-res", s.res, &RES, menu_w, None) {
                    s.res = i;
                }
            });
            kit::labeled_row(ui, "Frame rate", |ui| {
                if let Some(i) = kit::menu_button(ui, "exp-fps", s.fps, &FPS, menu_w, None) {
                    s.fps = i;
                }
            });
        });
        ui.add_space(6.0);
        let (bw, bh) = if RES_PX[s.res].0 == 0 { (pw, ph) } else { RES_PX[s.res] };
        let bfps = [pfps, 24.0, 25.0, 30.0, 50.0, 60.0][s.fps];
        match Format::ALL[s.codec.min(Format::ALL.len() - 1)] {
            Format::H264 | Format::Hevc10 => {
                let suggested = suggested_mbps(bw, bh, bfps);
                let mut mbps = s.bitrate.unwrap_or(suggested);
                if FieldRow::new("Bitrate", &mut mbps, 1.0..=200.0).default(suggested).unit("Mb/s").decimals(0).step(1.0).scrub(0.2).field_width(84.0).show(ui).changed {
                    s.bitrate = Some(mbps);
                }
            }
            Format::Clv => {
                FieldRow::new("Quality", &mut s.quality, 1.0..=100.0).default(90.0).unit("%").show(ui);
            }
            Format::Av1 => {
                FieldRow::new("Quality", &mut s.quality, 1.0..=100.0).default(70.0).unit("%").show(ui);
                kit::labeled_row(ui, "Speed", |ui| {
                    let mut sel = crate::formats::Av1Speed::ALL.iter().position(|v| *v == s.av1_speed).unwrap_or(0);
                    let segs: Vec<Segment> = crate::formats::Av1Speed::ALL.iter().map(|v| Segment::text(v.label())).collect();
                    if kit::segmented(ui, "exp-av1-speed", &mut sel, &segs, Some(menu_w), None).changed() {
                        s.av1_speed = crate::formats::Av1Speed::ALL[sel];
                    }
                });
            }
            Format::Png | Format::Gif | Format::M4a => {}
            // Masters are full quality.
            _ => {}
        }
        if let Some(note) = Format::ALL[s.codec.min(Format::ALL.len() - 1)].note() {
            ui.horizontal(|ui| {
                ui.add_space(kit::LABEL_W);
                ui.add(egui::Label::new(egui::RichText::new(crate::i18n::shown(note)).font(theme::caption()).color(t.text_2)).wrap());
            });
        }
        kit::labeled_row(ui, "Range", |ui| {
            let mut sel = if has_range { s.range } else { 0 };
            let segs = [Segment::text("Whole sequence"), Segment::text("In–out range")];
            ui.add_enabled_ui(has_range, |ui| {
                if kit::segmented(ui, "exp-range", &mut sel, &segs, Some(menu_w), None).changed() {
                    s.range = sel;
                }
            });
        });
        if !has_range {
            ui.horizontal(|ui| {
                ui.add_space(kit::LABEL_W);
                ui.add(egui::Label::new(egui::RichText::new(crate::i18n::shown("Set in and out points with I and O to export part of the sequence.")).font(theme::caption()).color(t.text_2)).wrap());
            });
        }
    });

    // Footer: estimate, progress, actions.
    let (w, h) = if RES_PX[s.res].0 == 0 { (pw, ph) } else { RES_PX[s.res] };
    let fps = [pfps, 24.0, 25.0, 30.0, 50.0, 60.0][s.fps];
    let len = if s.range == 1 && has_range { range_len } else { dur };
    let mbps = if matches!(Format::ALL[s.codec.min(Format::ALL.len() - 1)], Format::H264 | Format::Hevc10) { s.bitrate.unwrap_or(suggested_mbps(w, h, fps)) } else { bitrate_mbps(s, w, h, fps) };
    let mb = mbps * (len as f32 / SEC as f32) / 8.0;
    let size = if mb > 1024.0 { format!("{:.1} GB", mb / 1024.0) } else { format!("{:.0} MB", mb.max(1.0)) };
    let sound = if has_sound(proj) { "with sound" } else { "no sound" };
    let est = crate::i18n::tf(format!("≈ {size} · {} · {w} × {h} · {:.0} Mbps · {}", view::format_duration(len), mbps, sound));
    let mut fui = ui.new_child(egui::UiBuilder::new().max_rect(footer.shrink2(Vec2::new(20.0, 18.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
    let job = s.job.and_then(|id| app.jobs.get(id).cloned());
    match job {
        Some(j) if !j.done && !j.cancelled => {
            let (rr, _) = fui.allocate_exact_size(Vec2::new(fui.available_width() - 260.0, 28.0), Sense::hover());
            let p = fui.painter();
            p.text(egui::pos2(rr.left(), rr.top() + 6.0), Align2::LEFT_CENTER, crate::i18n::tf(format!("Exporting · {}% · {}", (j.progress * 100.0) as u32, Jobs::eta(&j))), theme::body(), t.text);
            let bar = Rect::from_min_size(egui::pos2(rr.left(), rr.bottom() - 6.0), Vec2::new(rr.width(), 4.0));
            p.rect_filled(bar, CornerRadius::same(2), t.raised);
            p.rect_filled(Rect::from_min_size(bar.min, Vec2::new(bar.width() * j.progress, 4.0)), CornerRadius::same(2), t.accent);
            fui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if Button::secondary("Cancel").show(ui).clicked() {
                    app.jobs.cancel(j.id);
                }
                if Button::ghost("Run in background").show(ui).clicked() {
                    app.overlay = Overlay::None;
                }
            });
        }
        _ => {
            let done = job.as_ref().is_some_and(|j| j.done);
            let text = if let Some(r) = &s.rights { crate::i18n::tf(format!("Licence problem: {r}. Fix it in Library ▸ Rights.")) } else if done { crate::i18n::tf(format!("Saved to {}", studio_paths::tilde(std::path::Path::new(&s.folder)))) } else { est };
            if done {
                let (ir, _) = fui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                icons::paint(fui.painter(), ir, Icon::CheckCircle, 16.0, t.accent);
            }
            // The buttons keep their place at the end; the estimate (or the
            // licence problem) ends in an ellipsis before them, whole on hover.
            fui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let mut go = Button::primary("Export").icon(Icon::Export).shortcut("⌘↩").min_width(112.0).show(ui).clicked()
                    || ui.input_mut(|i| i.consume_shortcut(&egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::Enter)));
                // Licences first: something that cannot be used stops the export until the person decides.
                if go {
                    go = press_export(s, || crate::screens::library_safety::export_block(&app.doc));
                }
                if s.rights.is_some() && Button::ghost("Export anyway").show(ui).clicked() {
                    go = press_export_anyway(s);
                }
                if go {
                    let format = Format::ALL[s.codec.min(Format::ALL.len() - 1)];
                    let file = format!("{}{}", s.name.trim(), if format == Format::Png { "" } else { format.extension() });
                    let path = std::path::Path::new(&s.folder).join(file);
                    let (a, b) = if s.range == 1 && has_range { app.doc.project.range.unwrap_or((0, dur)) } else { (0, dur) };
                    #[cfg(feature = "render")]
                    if app.cli.shot.is_none() {
                        let kbps = (s.bitrate.unwrap_or(suggested_mbps(w, h, fps)) * 1000.0) as u32;
                        let req = export_run::Request { path: path.clone(), format, size: (w, h), fps: fps as f64, range: (a, b), quality: s.quality, bitrate_kbps: kbps, av1_speed: s.av1_speed };
                        // The request is kept on the job, so a failed export can be retried.
                        let task = export_run::start(&app.doc, req.clone());
                        let id = app.jobs.start_export(crate::i18n::tf(format!("Export {}", s.name)), format.label(), task, req);
                        s.job = Some(id);
                    }
                    #[cfg(not(feature = "render"))]
                    {
                        let _ = (path, a, b, w, h, fps);
                        kit::push(ui.ctx(), "This build has no renderer, so it cannot export", None, Some(Icon::Warning));
                    }
                }
                #[cfg(feature = "render")]
                if !done && Button::secondary("Add to queue").show(ui).clicked() {
                    // Rendered later by the queue, one export at a time, with the preset picked on the left.
                    let preset = PRESETS[s.preset.min(PRESETS.len() - 1)].name;
                    match crate::screens::queue::enqueue(&mut app.doc, s.name.trim(), preset, true) {
                        Ok(_) => kit::push(ui.ctx(), format!("Added to the render queue ({preset})"), None, Some(Icon::CheckCircle)),
                        Err(e) => kit::push(ui.ctx(), e, None, Some(Icon::Warning)),
                    };
                }
                if Button::secondary(if done { "Close" } else { "Cancel" }).show(ui).clicked() {
                    app.overlay = Overlay::None;
                }
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| kit::label_trunc(ui, egui::RichText::new(text).font(theme::body()).color(t.text_2)));
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Overlay;
    use crate::ui_tests::Harness;

    #[test]
    fn av1_is_a_choice_on_the_export_screen_with_an_honest_note() {
        let index = Format::ALL.iter().position(|f| *f == Format::Av1).expect("AV1 is in the format list");
        assert_eq!(Format::ALL[index], Format::Av1);
        let web = PRESETS.iter().find(|p| p.name == "Web (AV1)").expect("a Web (AV1) preset");
        assert_eq!(web.codec, Some(index), "the preset points at the AV1 format");
        let mut h = Harness::new(&["--screen", "editor"]);
        h.app.overlay = Overlay::Export;
        h.app.export.codec = index;
        h.frames(4);
        let labels = h.labels();
        assert!(labels.iter().any(|l| l.contains("AV1 (MP4)")), "the format menu shows AV1: {labels:?}");
        assert!(labels.iter().any(|l| l.contains("Early encoder: slower than H.264, no rate control yet.")), "the note is shown: {labels:?}");
        assert!(labels.iter().any(|l| l.contains("Quality")), "AV1 has a quality control");
        assert!(labels.iter().any(|l| l.contains("Balanced")), "AV1 has a speed control: {labels:?}");
        assert!(!labels.iter().any(|l| l == "Small"), "Small is not offered yet: {labels:?}");
        assert!(h.cut_text().is_empty(), "nothing is cut off: {:?}", h.cut_text());
        assert!(h.overlaps().is_empty(), "nothing is drawn over: {:?}", h.overlaps());
    }

    #[test]
    fn the_estimate_says_whether_there_is_sound() {
        let mut p = crate::view::demo();
        assert!(has_sound(&p), "the demo has clips with sound");
        p.clips.retain(|c| c.kind == crate::view::ClipKind::Title);
        assert!(!has_sound(&p), "titles alone are silent");
        let mut a = p.clips.first().cloned().expect("a title");
        a.kind = crate::view::ClipKind::Audio;
        p.clips.push(a.clone());
        assert!(has_sound(&p));
        p.clips.last_mut().unwrap().audio.enabled = false;
        assert!(!has_sound(&p), "muted audio is silent");
    }

    #[test]
    fn export_anyway_is_for_one_export_and_a_new_project_starts_over() {
        let mut s = ExportState::default();
        let a = crate::doc::Doc::from_view(&crate::view::demo());
        sync_project(&mut s, &a);
        s.name = "A cut".into();
        s.job = Some(7);
        // Project A has a licence problem: Export is stopped, Export anyway lets that one through.
        assert!(!press_export(&mut s, || Some("clip.mp4: licence ran out".into())));
        assert!(s.rights.is_some());
        assert!(press_export_anyway(&mut s));
        assert!(s.rights.is_none());
        // The next export from the same project is checked again.
        assert!(!press_export(&mut s, || Some("clip.mp4: licence ran out".into())));
        // Project B: nothing of A is left, and its problem blocks it.
        let mut doc_b = crate::doc::Doc::from_view(&crate::view::demo());
        std::sync::Arc::make_mut(&mut doc_b.project).name = "Project B".into();
        s.rights = None;
        sync_project(&mut s, &doc_b);
        assert!(s.name.is_empty() && s.job.is_none() && s.rights.is_none(), "the state of project A is gone");
        assert!(!press_export(&mut s, || Some("other.mp4: no licence".into())));
        assert_eq!(s.rights.as_deref(), Some("other.mp4: no licence"));
    }
}
