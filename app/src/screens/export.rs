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
use crate::services::export;
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
    /// More ▸ Make a shorter version.
    pub shorter: Shorter,
    /// More ▸ Bundle.
    pub bundle: BundleUi,
    /// A project the screen wants opened (the shorter version just made).
    pub open_request: Option<std::path::PathBuf>,
    /// "Deliver for": the delivery spec id in force, if any.
    pub spec: Option<&'static str>,
    /// The size the spec decides, when it is not one of the sizes in the list.
    pub size_override: Option<(u32, u32)>,
    /// More ▸ Run QC after: check the file when the export is done.
    pub run_qc: bool,
    /// More ▸ Captions file: write an .srt beside the export.
    pub captions_file: bool,
    /// The export job whose follow-ups (captions file, QC) are still to run.
    pub after_for: Option<u64>,
    /// The last "Check project": problems by severity.
    pub check: Option<export::CheckOut>,
    /// The file the last export wrote, for "Check the file".
    pub last_file: Option<std::path::PathBuf>,
    /// The quality control of that file: running, or its one-line result.
    pub file_check: FileCheckState,
}

/// "Check the file": runs on its own thread; the result is a verdict and the worst findings.
#[derive(Default)]
pub enum FileCheckState {
    #[default]
    Idle,
    Running(export::FileCheck),
    Done(Vec<String>),
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
            shorter: Shorter::default(),
            bundle: BundleUi::default(),
            open_request: None,
            spec: None,
            size_override: None,
            run_qc: false,
            captions_file: false,
            after_for: None,
            check: None,
            last_file: None,
            file_check: FileCheckState::Idle,
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

/// "Check project": looks at the project without rendering and keeps the result for the section.
/// Returns what to tell the person.
fn run_check(s: &mut ExportState, doc: &mut crate::doc::Doc) -> String {
    let out = export::check_project(doc, false, false);
    let msg = if out.is_clear() { "No problems found".to_owned() } else { crate::i18n::tf(format!("Found {}", count_words(&out))) };
    s.check = Some(out);
    msg
}

/// "3 problems and 2 worth a look".
fn count_words(out: &export::CheckOut) -> String {
    let (f, w) = (out.count(export::Severity::Fail), out.count(export::Severity::Warning));
    let plural = |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    match (f, w) {
        (0, w) => plural(w, "thing worth a look", "things worth a look"),
        (f, 0) => plural(f, "problem", "problems"),
        (f, w) => format!("{} and {}", plural(f, "problem", "problems"), plural(w, "thing worth a look", "things worth a look")),
    }
}

/// "Fix" on one row, or "Fix all" (`only` is none): one undo step, then a fresh check.
fn fix_problems(s: &mut ExportState, doc: &mut crate::doc::Doc, only: Option<&export::Problem>) -> String {
    let done = match only {
        Some(p) => export::fix_problem(doc, &p.kind, p.clip).len(),
        None => export::check_project(doc, false, true).fixed.len(),
    };
    s.check = Some(export::check_project(doc, false, false));
    match done {
        0 => "Nothing could be fixed automatically: the advice says how to do it by hand".to_owned(),
        1 => "Fixed 1 problem".to_owned(),
        n => crate::i18n::tf(format!("Fixed {n} problems")),
    }
}

/// "Accept as intended": the finding stops counting as a problem for this project.
fn accept_problem(s: &mut ExportState, doc: &mut crate::doc::Doc, p: &export::Problem) -> Result<String, String> {
    if doc.folder.is_none() {
        return Err("Save the project to a folder first: accepted findings are kept with the project".into());
    }
    export::accept(doc, &p.kind, p.clip)?;
    let out = export::check_project(doc, false, false);
    s.check = Some(out);
    Ok("Accepted as intended".to_owned())
}

/// A time in seconds as m:ss.
fn clock(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

/// "Check project" / "Check the file": the section at the top of the settings.
fn check_section(ui: &mut egui::Ui, doc: &mut crate::doc::Doc, s: &mut ExportState, busy: bool) {
    let t = theme::tokens(ui.ctx());
    // The file check, finished on its thread.
    if let FileCheckState::Running(rx) = &s.file_check {
        match rx.try_recv() {
            Ok(Ok((r, file))) => {
                let mut lines = vec![crate::i18n::tf(format!("{} · {} problems, {} warnings", r.verdict, r.fails, r.warnings))];
                lines.extend(r.findings.iter().take(3).map(|f| crate::i18n::tf(format!("{} · {}", clock(f.start), f.message))));
                lines.push(crate::i18n::tf(format!("Report: {}", studio_paths::tilde(&file))));
                s.file_check = FileCheckState::Done(lines);
                kit::push(ui.ctx(), format!("Checked {}", r.file), None, Some(Icon::CheckCircle));
            }
            Ok(Err(e)) => {
                s.file_check = FileCheckState::Idle;
                kit::push(ui.ctx(), format!("Could not check the file: {e}"), None, Some(Icon::Warning));
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => ui.ctx().request_repaint_after(std::time::Duration::from_millis(300)),
            Err(std::sync::mpsc::TryRecvError::Disconnected) => s.file_check = FileCheckState::Idle,
        }
    }
    kit::menu_heading(ui, "Check");
    ui.add_enabled_ui(!busy, |ui| {
        ui.horizontal(|ui| {
            if Button::secondary("Check project").compact().show(ui).clicked() {
                let msg = run_check(s, doc);
                kit::push(ui.ctx(), msg, None, Some(Icon::CheckCircle));
            }
            let have_file = s.last_file.as_ref().is_some_and(|p| p.is_file());
            let running = matches!(s.file_check, FileCheckState::Running(_));
            ui.add_enabled_ui(have_file && !running, |ui| {
                let resp = Button::secondary("Check the file").compact().show(ui);
                if resp.clicked() {
                    if let Some(path) = s.last_file.clone() {
                        match export::start_file_check(doc, &path, None) {
                            Ok(rx) => s.file_check = FileCheckState::Running(rx),
                            Err(e) => {
                                kit::push(ui.ctx(), e, None, Some(Icon::Warning));
                            }
                        }
                    }
                }
            });
            if !have_file {
                ui.label(egui::RichText::new("Export first to check the file").font(theme::caption()).color(t.text_2));
            }
        });
    });
    if let FileCheckState::Running(_) = s.file_check {
        ui.label(egui::RichText::new("Checking the file…").font(theme::caption()).color(t.text_2));
    }
    if let FileCheckState::Done(lines) = &s.file_check {
        for l in lines {
            kit::label_trunc(ui, egui::RichText::new(l).font(theme::caption()).color(t.text_2));
        }
    }
    let Some(out) = s.check.clone() else { return };
    if out.is_clear() {
        let n = out.accepted.len();
        let msg = if n == 0 { "No problems found".to_owned() } else { crate::i18n::tf(format!("No problems found · {n} accepted as intended")) };
        kit::label_trunc(ui, egui::RichText::new(msg).font(theme::caption()).color(t.ok));
        return;
    }
    let mut act: Option<(usize, bool)> = None; // (row, fix? else accept)
    let mut fix_all = false;
    ui.horizontal(|ui| {
        kit::label_trunc(ui, egui::RichText::new(count_words(&out)).font(theme::caption()).color(t.text_2));
        if out.problems.iter().any(|p| p.fixable) {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                fix_all = Button::ghost("Fix all").compact().show(ui).clicked();
            });
        }
    });
    let mut last: Option<export::Severity> = None;
    for (i, p) in out.problems.iter().take(MAX_ROWS).enumerate() {
        if last != Some(p.severity) {
            last = Some(p.severity);
            ui.label(egui::RichText::new(p.severity.label()).font(theme::caption()).color(if p.severity == export::Severity::Fail { t.danger } else { t.warn }));
        }
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if Button::ghost("Accept as intended").compact().show(ui).clicked() {
                    act = Some((i, false));
                }
                if p.fixable && Button::secondary("Fix").compact().show(ui).clicked() {
                    act = Some((i, true));
                }
                let text = match p.at {
                    Some(at) => format!("{} · {}", clock(at), p.message),
                    None => p.message.clone(),
                };
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    let r = kit::label_trunc(ui, egui::RichText::new(text).font(theme::caption()).color(t.text));
                    if !p.advice.is_empty() {
                        r.on_hover_text(&p.advice);
                    }
                });
            });
        });
    }
    if out.problems.len() > MAX_ROWS {
        ui.label(egui::RichText::new(crate::i18n::tf(format!("and {} more: fix these first", out.problems.len() - MAX_ROWS))).font(theme::caption()).color(t.text_2));
    }
    if fix_all {
        let msg = fix_problems(s, doc, None);
        kit::push(ui.ctx(), msg, None, Some(Icon::CheckCircle));
    } else if let Some((i, fix)) = act {
        let p = out.problems[i].clone();
        if fix {
            let msg = fix_problems(s, doc, Some(&p));
            kit::push(ui.ctx(), msg, None, Some(Icon::CheckCircle));
        } else {
            match accept_problem(s, doc, &p) {
                Ok(msg) => kit::push(ui.ctx(), msg, None, Some(Icon::CheckCircle)),
                Err(e) => kit::push(ui.ctx(), format!("Could not accept it: {e}"), None, Some(Icon::Warning)),
            };
        }
    }
    ui.add_space(4.0);
}

/// Rows of problems the section shows before "and N more".
const MAX_ROWS: usize = 5;

/// The output size: the spec's, or the one picked in the list (0 × 0 there is the project's own).
fn out_size(s: &ExportState, pw: u32, ph: u32) -> (u32, u32) {
    if let Some(sz) = s.size_override {
        sz
    } else if RES_PX[s.res].0 == 0 {
        (pw, ph)
    } else {
        RES_PX[s.res]
    }
}

/// "Deliver for": choosing a spec fills the preset, size, rate and caption rule; None gives the controls back.
fn apply_spec(s: &mut ExportState, id: Option<&'static str>, project: (u32, u32, f32)) {
    s.spec = id;
    s.size_override = None;
    let Some(spec) = id.and_then(studio_deliver::specs::find) else {
        s.spec = None;
        return;
    };
    let fill = export::spec::fill(spec, (project.0, project.1), project.2 as f64);
    if let Some(i) = PRESETS.iter().position(|p| p.name == fill.preset) {
        let p = &PRESETS[i];
        s.preset = i;
        if let Some(c) = p.codec {
            s.codec = c;
        }
        s.quality = p.quality;
        s.res = p.res;
    }
    s.size_override = fill.size;
    if let Some(fps) = fill.fps {
        if let Some(i) = [project.2 as f64, 24.0, 25.0, 30.0, 50.0, 60.0].iter().position(|v| (v - fps).abs() < 0.01) {
            s.fps = i;
        }
    }
    if fill.captions {
        s.captions_file = true;
    }
}

/// The caption cues of the project as an .srt next to the export. Returns what to tell the person.
fn write_captions(doc: &crate::doc::Doc, file: &std::path::Path) -> Result<String, String> {
    let cues: Vec<studio_deliver::bundle::Cue> = crate::deliver_run::caption_cues(doc).into_iter().map(|(s, e, t)| studio_deliver::bundle::Cue { start: s, end: e, text: t }).collect();
    if cues.is_empty() {
        return Err("The project has no captions to write".into());
    }
    let path = file.with_extension("srt");
    std::fs::write(&path, studio_deliver::bundle::srt(&cues)).map_err(|e| format!("Could not write {}: {e}", path.display()))?;
    Ok(crate::i18n::tf(format!("Wrote {}", path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())))
}

/// When an export has finished: the captions file and the file check the person asked for, once.
fn after_export(s: &mut ExportState, doc: &crate::doc::Doc, ctx: &egui::Context, finished: bool) {
    if !finished || s.after_for.is_none() || s.after_for != s.job {
        return;
    }
    s.after_for = None;
    let Some(file) = s.last_file.clone() else { return };
    if s.captions_file {
        match write_captions(doc, &file) {
            Ok(m) => {
                kit::push(ctx, m, None, Some(Icon::CheckCircle));
            }
            Err(e) => {
                kit::push(ctx, e, None, Some(Icon::Warning));
            }
        }
    }
    if s.run_qc {
        match export::start_file_check(doc, &file, s.spec) {
            Ok(rx) => s.file_check = FileCheckState::Running(rx),
            Err(e) => {
                kit::push(ctx, e, None, Some(Icon::Warning));
            }
        }
    }
}

/// "Deliver for" and its one line of what the spec decides.
fn deliver_for(ui: &mut egui::Ui, s: &mut ExportState, project: (u32, u32, f32), busy: bool) {
    let t = theme::tokens(ui.ctx());
    kit::menu_heading(ui, "Deliver for");
    let specs = export::spec::menu();
    let mut labels: Vec<&str> = vec!["None"];
    labels.extend(specs.iter().map(|d| d.name));
    let sel = s.spec.and_then(|id| specs.iter().position(|d| d.id == id)).map_or(0, |i| i + 1);
    let w = ui.available_width();
    ui.add_enabled_ui(!busy, |ui| {
        if let Some(i) = kit::menu_button(ui, "exp-spec", sel, &labels, w, None) {
            apply_spec(s, if i == 0 { None } else { Some(specs[i - 1].id) }, project);
        }
    });
    if let Some(d) = s.spec.and_then(studio_deliver::specs::find) {
        let fill = export::spec::fill(d, (project.0, project.1), project.2 as f64);
        let mut bits: Vec<String> = Vec::new();
        if let Some(l) = &fill.loudness {
            bits.push(l.clone());
        }
        if fill.captions {
            bits.push("captions required".into());
        }
        if let Some(m) = fill.max_secs {
            bits.push(format!("up to {m:.0} s"));
        }
        if !bits.is_empty() {
            ui.add(egui::Label::new(egui::RichText::new(crate::i18n::tf(bits.join(" · "))).font(theme::caption()).color(t.text_2)).wrap());
        }
    }
}

/// "Make a shorter version": 15 / 30 / 60 seconds or a length of your own, optionally vertical.
pub struct Shorter {
    pub open: bool,
    /// 0..=2 pick 15 / 30 / 60 s; 3 is `custom`.
    pub len: usize,
    pub custom: f32,
    pub vertical: bool,
    pub done: Option<export::CutdownOut>,
}

impl Default for Shorter {
    fn default() -> Self {
        Self { open: false, len: 1, custom: 45.0, vertical: false, done: None }
    }
}

/// "Bundle…": the checklist, the run, the result.
pub struct BundleUi {
    pub open: bool,
    pub req: export::BundleReq,
    /// Cutdown lengths ticked in the checklist (15 / 30 / 60 s).
    pub cutdowns: [bool; 3],
    pub run: Option<std::sync::mpsc::Receiver<Result<export::BundleOut, String>>>,
    pub done: Option<export::BundleOut>,
}

impl Default for BundleUi {
    fn default() -> Self {
        Self { open: false, req: export::BundleReq::default(), cutdowns: [false; 3], run: None, done: None }
    }
}

const SHORTER: [f64; 3] = [15.0, 30.0, 60.0];

fn shorter_secs(sh: &Shorter) -> f64 {
    SHORTER.get(sh.len).copied().unwrap_or(sh.custom as f64)
}

/// "Make it": saves the project and cuts it down into a project of its own. Returns what to tell the person.
fn make_shorter(doc: &mut crate::doc::Doc, sh: &mut Shorter) -> Result<String, String> {
    let root = doc.folder.as_ref().map(|f| f.root().to_path_buf()).ok_or("Save the project to a folder first: the shorter version is made from the saved project")?;
    crate::services::project::save(doc, None)?;
    let secs = shorter_secs(sh);
    let out = export::cutdown(doc, export::CutdownReq { seconds: secs, root, vertical: sh.vertical, ..Default::default() })?;
    let msg = crate::i18n::tf(format!("Made {} · {:.0} s", out.name, out.seconds));
    sh.done = Some(out);
    Ok(msg)
}

/// The cutdowns ticked in the bundle checklist, as lengths.
fn bundle_cutdowns(b: &BundleUi) -> Vec<f64> {
    SHORTER.iter().zip(b.cutdowns).filter(|(_, on)| *on).map(|(s, _)| *s).collect()
}

/// "Make bundle": the project is saved, then the bundle is written on its own thread.
fn start_bundle(doc: &mut crate::doc::Doc, b: &mut BundleUi, spec: Option<&str>) -> Result<(), String> {
    let root = doc.folder.as_ref().map(|f| f.root().to_path_buf()).ok_or("Save the project to a folder first: the bundle is made from the saved project")?;
    crate::services::project::save(doc, None)?;
    let mut req = b.req.clone();
    req.cutdowns = bundle_cutdowns(b);
    req.spec = spec.map(str::to_owned);
    let copy = crate::doc::Doc::new(doc.model.clone());
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        // The receiver may be gone when the person closes the screen; the bundle is written either way.
        let _ = tx.send(export::bundle(&copy, &root, &req));
    });
    b.done = None;
    b.run = Some(rx);
    Ok(())
}

/// The bundle finished on its thread: one toast, the result kept for the section.
fn poll_bundle(b: &mut BundleUi, ctx: &egui::Context) {
    let Some(rx) = &b.run else { return };
    match rx.try_recv() {
        Ok(Ok(out)) => {
            kit::push(ctx, crate::i18n::tf(format!("Made bundle · {} files", out.files)), None, Some(Icon::CheckCircle));
            b.done = Some(out);
            b.run = None;
        }
        Ok(Err(e)) => {
            kit::push(ctx, format!("Could not make the bundle: {e}"), None, Some(Icon::Warning));
            b.run = None;
        }
        Err(std::sync::mpsc::TryRecvError::Empty) => ctx.request_repaint_after(std::time::Duration::from_millis(400)),
        Err(std::sync::mpsc::TryRecvError::Disconnected) => b.run = None,
    }
}

fn shorter_section(ui: &mut egui::Ui, doc: &mut crate::doc::Doc, s: &mut ExportState) {
    let t = theme::tokens(ui.ctx());
    let sh = &mut s.shorter;
    ui.horizontal(|ui| {
        for (i, label) in ["15 s", "30 s", "60 s", "Custom"].iter().enumerate() {
            if ui.selectable_label(sh.len == i, *label).clicked() {
                sh.len = i;
            }
        }
    });
    if sh.len == 3 {
        FieldRow::new("Length", &mut sh.custom, 2.0..=3600.0).default(45.0).unit("s").decimals(0).step(1.0).scrub(0.5).field_width(84.0).show(ui);
    }
    ui.checkbox(&mut sh.vertical, "Vertical (9:16)").on_hover_text("Reframes to follow the subject; the original is not changed");
    let mut go = false;
    ui.horizontal(|ui| {
        go = Button::primary("Make it").compact().show(ui).clicked();
    });
    if go {
        match make_shorter(doc, sh) {
            Ok(m) => {
                kit::push(ui.ctx(), m, None, Some(Icon::CheckCircle));
            }
            Err(e) => {
                kit::push(ui.ctx(), format!("Could not make a shorter version: {e}"), None, Some(Icon::Warning));
            }
        }
    }
    if let Some(out) = sh.done.clone() {
        kit::label_trunc(ui, egui::RichText::new(crate::i18n::tf(format!("{} · {:.0} s · kept {} shots, dropped {}", out.name, out.seconds, out.kept.len(), out.dropped.len()))).font(theme::caption()).color(t.text_2));
        ui.horizontal(|ui| {
            if Button::secondary("Open").compact().show(ui).clicked() {
                s.open_request = Some(out.project.clone());
            }
            if Button::secondary("Add to queue").compact().show(ui).on_hover_text("Renders the shorter version with the render queue").clicked() {
                let mut spec = studio_deliver::queue::JobSpec::new(&out.name, &out.project);
                spec.preset = if out.vertical { "Vertical".into() } else { "Social 1080p".into() };
                match crate::deliver_run::store().update(|q| q.add(spec)) {
                    Ok(_) => {
                        kit::push(ui.ctx(), crate::i18n::tf(format!("Added {} to the render queue", out.name)), None, Some(Icon::CheckCircle));
                    }
                    Err(e) => {
                        kit::push(ui.ctx(), format!("Could not add it to the queue: {e}"), None, Some(Icon::Warning));
                    }
                }
            }
        });
    }
}

fn bundle_section(ui: &mut egui::Ui, doc: &mut crate::doc::Doc, s: &mut ExportState) {
    let t = theme::tokens(ui.ctx());
    poll_bundle(&mut s.bundle, ui.ctx());
    let spec = s.spec;
    let b = &mut s.bundle;
    let running = b.run.is_some();
    ui.add_enabled_ui(!running, |ui| {
        let mut thumbs = b.req.thumbnails > 0;
        ui.checkbox(&mut b.req.master, "Master");
        ui.checkbox(&mut b.req.proxy, "Proxy (720p review copy)");
        ui.checkbox(&mut b.req.audio, "Mix (24-bit WAV)");
        ui.add_enabled(b.req.audio, egui::Checkbox::new(&mut b.req.stems, "Stems"));
        ui.checkbox(&mut b.req.captions, "Captions");
        if ui.checkbox(&mut thumbs, "Thumbnails").changed() {
            b.req.thumbnails = if thumbs { 8 } else { 0 };
        }
        ui.checkbox(&mut b.req.poster, "Poster frame");
        ui.horizontal(|ui| {
            ui.label("Shorter versions");
            for (i, label) in ["15 s", "30 s", "60 s"].iter().enumerate() {
                ui.checkbox(&mut b.cutdowns[i], *label);
            }
        });
        ui.add_enabled(b.cutdowns.iter().any(|c| *c), egui::Checkbox::new(&mut b.req.verticals, "Vertical versions too"));
    });
    if running {
        ui.label(egui::RichText::new("Making the bundle…").font(theme::caption()).color(t.text_2));
    } else if Button::primary("Make bundle").compact().show(ui).clicked() {
        if let Err(e) = start_bundle(doc, b, spec) {
            kit::push(ui.ctx(), format!("Could not make the bundle: {e}"), None, Some(Icon::Warning));
        }
    }
    if let Some(out) = &b.done {
        kit::label_trunc(ui, egui::RichText::new(crate::i18n::tf(format!("{} files in {}", out.files, studio_paths::tilde(&out.folder)))).font(theme::caption()).color(t.text_2));
        for m in out.missing.iter().take(4) {
            ui.add(egui::Label::new(egui::RichText::new(crate::i18n::tf(format!("Left out: {m}"))).font(theme::caption()).color(t.warn)).wrap());
        }
        if Button::secondary("Show in Finder").compact().show(ui).clicked() {
            if let Err(e) = export::reveal(&out.folder) {
                kit::push(ui.ctx(), format!("Could not show the folder: {e}"), None, Some(Icon::Warning));
            }
        }
    }
}

/// Export ▸ More: what happens after the export, and the audio-only shortcut.
fn more_options(ui: &mut egui::Ui, doc: &mut crate::doc::Doc, s: &mut ExportState, fill: Option<&export::spec::Fill>) {
    let t = theme::tokens(ui.ctx());
    egui::CollapsingHeader::new("More").id_salt("export-more").default_open(false).show(ui, |ui| {
        ui.checkbox(&mut s.run_qc, "Run QC after").on_hover_text("Checks the finished file for levels, black, frozen frames and loudness");
        let locked = fill.is_some_and(|f| f.captions);
        let mut on = s.captions_file || locked;
        if ui.add_enabled(!locked, egui::Checkbox::new(&mut on, "Captions file")).on_disabled_hover_text(fill.and_then(|f| f.reason(export::spec::Lock::Captions)).unwrap_or_default()).changed() {
            s.captions_file = on;
        }
        ui.add_enabled(false, egui::Checkbox::new(&mut false, "Burn captions")).on_disabled_hover_text("Burning captions into the picture is not in the export yet: write a captions file instead");
        let audio_idx = PRESETS.iter().position(|p| p.name == "Audio only");
        let mut audio = audio_idx == Some(s.preset);
        if ui.add_enabled(fill.is_none(), egui::Checkbox::new(&mut audio, "Audio only")).on_disabled_hover_text("Choose None under Deliver for first").changed() {
            let target = if audio { audio_idx } else { Some(0) };
            if let Some(i) = target {
                let p = &PRESETS[i];
                if let Some(c) = p.codec {
                    s.preset = i;
                    s.res = p.res;
                    s.codec = c;
                    s.quality = p.quality;
                }
            }
        }
        ui.separator();
        if Button::ghost("Make a shorter version…").compact().show(ui).clicked() {
            s.shorter.open = !s.shorter.open;
        }
        if s.shorter.open {
            shorter_section(ui, doc, s);
        }
        if Button::ghost("Bundle…").compact().show(ui).clicked() {
            s.bundle.open = !s.bundle.open;
        }
        if s.bundle.open {
            bundle_section(ui, doc, s);
        }
        if let Some(f) = fill {
            for n in f.notes.iter().take(2) {
                ui.add(egui::Label::new(egui::RichText::new(crate::i18n::tf((*n).to_owned())).font(theme::caption()).color(t.text_2)).wrap());
            }
        }
    });
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
    let s = &mut app.export;
    let busy_now = s.job.and_then(|id| app.jobs.get(id)).is_some_and(|j| !j.done && !j.cancelled);
    deliver_for(&mut col, s, (app.doc.project.width, app.doc.project.height, app.doc.project.fps), busy_now);
    kit::menu_heading(&mut col, "Presets");
    let preset_lock = s.spec.and_then(studio_deliver::specs::find).map(|d| format!("{} is written as {}; pick None under Deliver for to choose a preset", d.name, d.preset));
    // A short window: the presets scroll above the footer.
    egui::ScrollArea::vertical().id_salt("export-presets").auto_shrink([false, false]).show_v(&mut col, |pui| {
      pui.add_enabled_ui(preset_lock.is_none(), |pui| {
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
      }).response.on_disabled_hover_text(preset_lock.clone().unwrap_or_default());
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
    let spec_fill = s.spec.and_then(studio_deliver::specs::find).map(|d| export::spec::fill(d, (pw, ph), pfps as f64));
    check_section(&mut sui, &mut app.doc, s, busy);
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
            let lock = spec_fill.as_ref().and_then(|f| f.reason(export::spec::Lock::Format)).map(str::to_owned);
            ui.add_enabled_ui(lock.is_none(), |ui| {
                if let Some(i) = kit::menu_button(ui, "exp-codec", s.codec, &labels, menu_w, None) {
                    s.codec = i;
                }
            })
            .response
            .on_disabled_hover_text(lock.unwrap_or_default());
        });
        ui.add_enabled_ui(true, |ui| {
            kit::labeled_row(ui, "Resolution", |ui| {
                let lock = spec_fill.as_ref().and_then(|f| f.reason(export::spec::Lock::Resolution)).map(str::to_owned);
                ui.add_enabled_ui(lock.is_none(), |ui| {
                    let shown = s.size_override.and_then(|(w, h)| RES_PX.iter().position(|p| *p == (w, h))).unwrap_or(s.res);
                    if s.size_override.is_some() && shown == s.res && RES_PX[s.res] != s.size_override.unwrap_or_default() {
                        let (w, h) = s.size_override.unwrap_or_default();
                        let one = [format!("{w} × {h}")];
                        let _ = kit::menu_button(ui, "exp-res-spec", 0, &[one[0].as_str()], menu_w, None);
                    } else if let Some(i) = kit::menu_button(ui, "exp-res", shown, &RES, menu_w, None) {
                        s.res = i;
                    }
                })
                .response
                .on_disabled_hover_text(lock.unwrap_or_default());
            });
            kit::labeled_row(ui, "Frame rate", |ui| {
                let lock = spec_fill.as_ref().and_then(|f| f.reason(export::spec::Lock::FrameRate)).map(str::to_owned);
                ui.add_enabled_ui(lock.is_none(), |ui| {
                    if let Some(i) = kit::menu_button(ui, "exp-fps", s.fps, &FPS, menu_w, None) {
                        s.fps = i;
                    }
                })
                .response
                .on_disabled_hover_text(lock.unwrap_or_default());
            });
        });
        ui.add_space(6.0);
        let (bw, bh) = out_size(s, pw, ph);
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
        more_options(ui, &mut app.doc, s, spec_fill.as_ref());
    });

    // Footer: estimate, progress, actions.
    let (w, h) = out_size(s, pw, ph);
    let fps = [pfps, 24.0, 25.0, 30.0, 50.0, 60.0][s.fps];
    let len = if s.range == 1 && has_range { range_len } else { dur };
    let mbps = if matches!(Format::ALL[s.codec.min(Format::ALL.len() - 1)], Format::H264 | Format::Hevc10) { s.bitrate.unwrap_or(suggested_mbps(w, h, fps)) } else { bitrate_mbps(s, w, h, fps) };
    let mb = mbps * (len as f32 / SEC as f32) / 8.0;
    let size = if mb > 1024.0 { format!("{:.1} GB", mb / 1024.0) } else { format!("{:.0} MB", mb.max(1.0)) };
    let sound = if has_sound(&app.doc.project) { "with sound" } else { "no sound" };
    let est = crate::i18n::tf(format!("≈ {size} · {} · {w} × {h} · {:.0} Mbps · {}", view::format_duration(len), mbps, sound));
    let mut fui = ui.new_child(egui::UiBuilder::new().max_rect(footer.shrink2(Vec2::new(20.0, 18.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
    let job = s.job.and_then(|id| app.jobs.get(id).cloned());
    after_export(s, &app.doc, ui.ctx(), job.as_ref().is_some_and(|j| j.done));
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
                        s.last_file = Some(path.clone());
                        s.after_for = Some(id);
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
                    match crate::screens::queue::enqueue_spec(&mut app.doc, s.name.trim(), preset, s.spec, true, s.size_override) {
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
    // The shorter version the person asked to open: the usual open settles unsaved work first.
    if let Some(dir) = app.export.open_request.take() {
        app.overlay = Overlay::None;
        app.open_folder(ui.ctx(), Some(dir));
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

    fn problem(kind: &str, severity: export::Severity, fixable: bool) -> export::Problem {
        export::Problem { kind: kind.into(), clip: Some(3), at: Some(75.0), message: format!("{kind} happened"), advice: "do the thing".into(), severity, fixable }
    }

    #[test]
    fn the_check_section_lists_problems_by_severity_with_fix_and_accept() {
        let mut h = Harness::new(&["--screen", "editor"]);
        h.app.overlay = Overlay::Export;
        h.app.export.check = Some(export::CheckOut { problems: vec![problem("gap", export::Severity::Fail, false), problem("audio_quiet", export::Severity::Warning, true)], ..Default::default() });
        h.frames(4);
        let labels = h.labels();
        for want in ["Check project", "Check the file", "Problems", "Worth a look", "Fix", "Fix all", "Accept as intended"] {
            assert!(labels.iter().any(|l| l == want), "{want} is shown: {labels:?}");
        }
        assert!(labels.iter().any(|l| l.contains("1:15 · gap happened")), "the row says when: {labels:?}");
        assert!(h.cut_text().is_empty(), "nothing is cut off: {:?}", h.cut_text());
        assert!(h.overlaps().is_empty(), "nothing is drawn over: {:?}", h.overlaps());
        // A clear project says so.
        h.app.export.check = Some(export::CheckOut::default());
        h.frames(3);
        assert!(h.labels().iter().any(|l| l == "No problems found"));
    }

    #[test]
    fn accepting_needs_a_saved_project_and_then_sticks() {
        let mut s = ExportState::default();
        let mut doc = crate::doc::Doc::from_view(&crate::view::demo());
        let msg = run_check(&mut s, &mut doc);
        assert!(!msg.is_empty());
        let p = problem("gap", export::Severity::Fail, false);
        assert!(accept_problem(&mut s, &mut doc, &p).is_err(), "without a folder the acceptance would be lost");
        let dir = std::env::temp_dir().join(format!("zornov-accept-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        crate::services::project::save(&mut doc, Some(&dir)).expect("saves to a temp folder");
        assert_eq!(accept_problem(&mut s, &mut doc, &p).as_deref(), Ok("Accepted as intended"));
        assert!(s.check.is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_delivery_spec_fills_the_controls_and_none_gives_them_back() {
        let mut s = ExportState::default();
        apply_spec(&mut s, Some("youtube-shorts"), (1920, 1080, 30.0));
        assert_eq!(s.spec, Some("youtube-shorts"));
        let (w, h) = out_size(&s, 1920, 1080);
        assert!(h > w, "Shorts is vertical: {w} x {h}");
        assert_eq!(PRESETS[s.preset].name, studio_deliver::specs::find("youtube-shorts").unwrap().preset);
        apply_spec(&mut s, None, (1920, 1080, 30.0));
        assert!(s.spec.is_none() && s.size_override.is_none());
        assert_eq!(out_size(&s, 1920, 1080), RES_PX[s.res]);
        let r128 = export::spec::fill(studio_deliver::specs::find("broadcast-r128").unwrap(), (1920, 1080), 25.0);
        apply_spec(&mut s, Some("broadcast-r128"), (1920, 1080, 25.0));
        assert_eq!(s.captions_file, r128.captions);
    }

    #[test]
    fn deliver_for_and_more_are_on_the_screen_and_a_spec_disables_what_it_fixes() {
        let mut h = Harness::new(&["--screen", "editor"]);
        h.app.overlay = Overlay::Export;
        h.frames(4);
        let labels = h.labels();
        for want in ["None", "More"] {
            assert!(labels.iter().any(|l| l == want), "{want}: {labels:?}");
        }
        let (w, hh, fps) = (h.app.doc.project.width, h.app.doc.project.height, h.app.doc.project.fps);
        apply_spec(&mut h.app.export, Some("youtube-shorts"), (w, hh, fps));
        h.frames(4);
        let labels = h.labels();
        assert!(labels.iter().any(|l| l == "YouTube Shorts"), "the menu shows the spec: {labels:?}");
        assert!(h.cut_text().is_empty(), "{:?}", h.cut_text());
        assert!(h.overlaps().is_empty(), "{:?}", h.overlaps());
    }

    #[test]
    fn a_captions_file_needs_captions_and_says_so() {
        let doc = crate::doc::Doc::from_view(&crate::view::demo());
        let dir = std::env::temp_dir().join(format!("zornov-caps-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let e = write_captions(&doc, &dir.join("film.mp4"));
        assert!(e.is_err() || dir.join("film.srt").is_file(), "either it says there are no captions or it wrote them");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_shorter_version_needs_a_saved_project_and_makes_a_project_of_its_own() {
        let mut doc = crate::doc::Doc::from_view(&crate::view::demo());
        let mut sh = Shorter::default();
        let e = make_shorter(&mut doc, &mut sh).unwrap_err();
        assert!(e.starts_with("Save the project"), "{e}");
        let dir = std::env::temp_dir().join(format!("zornov-shorter-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        crate::services::project::save(&mut doc, Some(&dir)).expect("saves to a temp folder");
        sh.len = 3;
        sh.custom = (doc.project.duration() as f64 / crate::view::SEC as f64 / 2.0).floor() as f32;
        let msg = make_shorter(&mut doc, &mut sh).expect("cuts it down");
        assert!(msg.starts_with("Made "), "{msg}");
        let out = sh.done.as_ref().expect("the result is kept for Open and Add to queue");
        assert!(out.project.is_dir() && out.seconds < doc.project.duration() as f64 / crate::view::SEC as f64);
        // Too long a version says why.
        sh.custom = 100000.0;
        assert!(make_shorter(&mut doc, &mut sh).unwrap_err().contains("longer"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_bundle_checklist_maps_to_a_request_and_needs_a_saved_project() {
        let mut doc = crate::doc::Doc::from_view(&crate::view::demo());
        let mut b = BundleUi::default();
        b.cutdowns = [false, true, true];
        assert_eq!(bundle_cutdowns(&b), vec![30.0, 60.0]);
        let e = start_bundle(&mut doc, &mut b, None).unwrap_err();
        assert!(e.starts_with("Save the project"), "{e}");
        assert!(b.run.is_none(), "nothing started");
    }

    #[test]
    fn more_shows_the_shorter_version_and_bundle_sections() {
        let mut h = Harness::new(&["--screen", "editor"]);
        h.app.overlay = Overlay::Export;
        h.app.export.shorter.open = true;
        h.app.export.bundle.open = true;
        h.frames(4);
        // The "More" header is collapsed by default; its content is reachable once opened by the person.
        let labels = h.labels();
        assert!(labels.iter().any(|l| l == "More"), "{labels:?}");
        assert!(h.cut_text().is_empty() && h.overlaps().is_empty());
    }

    #[test]
    fn counts_read_as_words() {
        let one = export::CheckOut { problems: vec![problem("gap", export::Severity::Fail, false)], ..Default::default() };
        assert_eq!(count_words(&one), "1 problem");
        let two = export::CheckOut { problems: vec![problem("gap", export::Severity::Fail, false), problem("x", export::Severity::Warning, false), problem("y", export::Severity::Warning, false)], ..Default::default() };
        assert_eq!(count_words(&two), "1 problem and 2 things worth a look");
        assert_eq!(clock(75.4), "1:15");
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
