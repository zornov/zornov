// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Start screen: new project from a canvas preset, open or import a
//! project, and the grid of recent projects.

use crate::app::{Overlay, StudioApp};
use crate::icons::{self, Icon};
use crate::kit::{self, Button, IconButton};
use crate::{files, theme, view};
use eframe::egui::{self, Align2, Color32, CornerRadius, Rect, Sense, Vec2};
use crate::kit::ShowNoted;

#[derive(Default)]
pub struct StartState {
    pub search: String,
    pub custom_w: f32,
    pub custom_h: f32,
    pub custom_fps: usize,
    /// Each recent project's cover picture (see `files::cover_of`), looked
    /// up once.
    pub covers: std::collections::HashMap<std::path::PathBuf, Option<(std::path::PathBuf, f64)>>,
}

struct Preset {
    name: &'static str,
    ratio: &'static str,
    w: u32,
    h: u32,
    hint: &'static str,
}

const PRESETS: &[Preset] = &[
    Preset { name: "Landscape", ratio: "16:9", w: 1920, h: 1080, hint: "YouTube, vlogs" },
    Preset { name: "Vertical", ratio: "9:16", w: 1080, h: 1920, hint: "Reels, shorts" },
    Preset { name: "Square", ratio: "1:1", w: 1080, h: 1080, hint: "Feeds" },
    Preset { name: "Portrait", ratio: "4:5", w: 1080, h: 1350, hint: "Feed posts" },
];

/// Up to this many recent projects are shown as picture tiles; more become
/// a list with their folders and dates.
/// Recent projects shown before "Show all" (about two rows).
const RECENT_TILES: usize = 12;

/// Height of the strip at the top kept clear for the window buttons.
pub const TITLE_BAR_H: f32 = 52.0;

pub fn show(app: &mut StudioApp, ui: &mut egui::Ui) {
    let t = theme::tokens(ui.ctx());
    let full = ui.max_rect();
    window_drag_area(ui, Rect::from_min_size(full.min, Vec2::new(full.width(), TITLE_BAR_H)));
    let dialogs = app.cli.shot.is_none();
    app.files_frame(&ui.ctx().clone());
    app.recovered_panel(&ui.ctx().clone());

    // Shortcuts.
    let cmd = egui::Modifiers::COMMAND;
    if dialogs && ui.input_mut(|i| i.consume_shortcut(&egui::KeyboardShortcut::new(cmd, egui::Key::S))) {
        // Save works here as it does in the editor: the project left open
        // behind this screen, if it has anything to save.
        let ctx = ui.ctx().clone();
        if app.doc.dirty() {
            app.save(&ctx, false);
        } else {
            kit::push(&ctx, "Nothing to save", None, Some(Icon::Info));
        }
        return;
    }
    if ui.input_mut(|i| i.consume_shortcut(&egui::KeyboardShortcut::new(cmd, egui::Key::N))) {
        new_project(app, ui.ctx(), &PRESETS[0]);
        return;
    }
    if dialogs && ui.input_mut(|i| i.consume_shortcut(&egui::KeyboardShortcut::new(cmd, egui::Key::O))) {
        let ctx = ui.ctx().clone();
        app.open_folder(&ctx, None);
        return;
    }

    // One centred column, so wide windows keep a comfortable measure.
    // Right-to-left languages lay each row out from the right.
    let content_w = (full.width() - 80.0).clamp(320.0, 1320.0);
    let col = Rect::from_center_size(full.center(), Vec2::new(content_w, full.height()));
    let rtl = crate::i18n::is_rtl();
    let at = |x: f32, w: f32| if rtl { col.right() - x - w } else { col.left() + x };

    // Header: the wordmark, then search and settings at the far end.
    let hy = full.top() + TITLE_BAR_H + 14.0;
    wordmark(ui.painter(), egui::pos2(at(0.0, 110.0), hy), &t);
    let gear = Rect::from_center_size(egui::pos2(at(content_w - 28.0, 28.0) + 14.0, hy), Vec2::splat(28.0));
    let settings_keys = crate::keymap::hint(&app.prefs, "settings");
    if IconButton::new(Icon::GearSix, "Settings").shortcut(&settings_keys).show_at(ui, gear).clicked() {
        app.overlay = Overlay::Settings;
    }
    let search_w = 240.0_f32.min(content_w - 160.0).max(120.0);
    let search_rect = Rect::from_min_size(egui::pos2(at(content_w - 40.0 - search_w, search_w), hy - 13.0), Vec2::new(search_w, 26.0));
    let mut sui = ui.new_child(egui::UiBuilder::new().max_rect(search_rect));
    kit::text_field(&mut sui, &mut app.start.search, "Search projects", Some(Icon::MagnifyingGlass), search_w, None);

    // New project: the frame shapes to start from, the other ways in
    // beside them (below them in a narrow window).
    let mut y = hy + 46.0;
    let label_pos = egui::pos2(if rtl { col.right() } else { col.left() }, y);
    ui.painter().text(label_pos, if rtl { Align2::RIGHT_CENTER } else { Align2::LEFT_CENTER }, crate::i18n::shown("New project"), theme::section_title(), t.text);
    y += 20.0;
    let gap = 12.0;
    let side_w = 260.0;
    let wide = content_w >= 5.0 * 96.0 + 4.0 * gap + 32.0 + side_w;
    let cards_w = if wide { content_w - 32.0 - side_w } else { content_w };
    let card_w = ((cards_w - 4.0 * gap) / 5.0).clamp(60.0, 140.0);
    let card_h = 124.0;
    let mut custom_rect = Rect::NOTHING;
    for i in 0..5 {
        let r = Rect::from_min_size(egui::pos2(at(i as f32 * (card_w + gap), card_w), y), Vec2::new(card_w, card_h));
        if let Some(pr) = PRESETS.get(i) {
            let size = format!("{}×{}", pr.w, pr.h);
            if preset_card(ui, r, pr.name, pr.ratio, &size, "", Some((pr.w, pr.h))).on_hover_text(pr.hint).clicked() {
                new_project(app, ui.ctx(), pr);
                return;
            }
        } else {
            custom_rect = r;
            if preset_card(ui, r, "Custom", "", "Any size", "", None).clicked() {
                kit::toggle_open(ui.ctx(), egui::Id::unique("start-custom"));
                if app.start.custom_w == 0.0 {
                    app.start.custom_w = 2560.0;
                    app.start.custom_h = 1440.0;
                    app.start.custom_fps = 1;
                }
            }
        }
    }
    let actions = [
        (Icon::FolderOpen, "Open project", "A project folder on this computer", Side::Open),
        (Icon::DownloadSimple, "Import timeline", "OpenTimelineIO or FCPXML", Side::Import),
        (Icon::Sparkle, "Demo project", "Sample clips and music, ready to edit", Side::Demo),
    ];
    let mut go = None;
    for (i, (icon, title, sub, side)) in actions.into_iter().enumerate() {
        let r = if wide {
            let x = 5.0 * card_w + 4.0 * gap + 32.0;
            Rect::from_min_size(egui::pos2(at(x, side_w), y + i as f32 * 42.0), Vec2::new(side_w, 40.0))
        } else {
            let w = (content_w - 2.0 * gap) / 3.0;
            Rect::from_min_size(egui::pos2(at(i as f32 * (w + gap), w), y + card_h + 12.0), Vec2::new(w, 40.0))
        };
        if action_row(ui, r, icon, title, sub).clicked() {
            go = Some(side);
        }
    }
    match go {
        Some(Side::Demo) => {
            let ctx = ui.ctx().clone();
            app.replace(&ctx, files::Replace::Sample(view::demo()));
            return;
        }
        Some(Side::Open) if dialogs => {
            let ctx = ui.ctx().clone();
            app.open_folder(&ctx, None);
            return;
        }
        Some(Side::Import) if dialogs => {
            let ctx = ui.ctx().clone();
            app.import_timeline(&ctx);
            return;
        }
        _ => {}
    }
    let custom_id = egui::Id::unique("start-custom");
    let mut create = None;
    kit::popover(ui.ctx(), custom_id, custom_rect, kit::Side::BelowRight, 260.0, |ui| {
        ui.add_space(theme::space::XS);
        ui.horizontal(|ui| {
            ui.add_space(theme::space::SM);
            ui.label(egui::RichText::new(crate::i18n::shown("Custom canvas")).font(theme::section_title()).color(t.text));
        });
        egui::Frame::new().inner_margin(egui::Margin::symmetric(6, 6)).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 4.0;
            kit::FieldRow::new("Width", &mut app.start.custom_w, 16.0..=8192.0).default(1920.0).unit("px").no_slider().step(2.0).show(ui);
            kit::FieldRow::new("Height", &mut app.start.custom_h, 16.0..=8192.0).default(1080.0).unit("px").no_slider().step(2.0).show(ui);
            kit::labeled_row(ui, "Frame rate", |ui| {
                if let Some(i) = kit::menu_button(ui, "custom-fps", app.start.custom_fps, &["24 fps", "30 fps", "60 fps"], 112.0, None) {
                    app.start.custom_fps = i;
                }
            });
            ui.add_space(theme::space::SM);
            ui.horizontal(|ui| {
                ui.add_space(kit::LABEL_W);
                if Button::primary("Create").show(ui).clicked() {
                    create = Some((app.start.custom_w as u32, app.start.custom_h as u32, [24.0, 30.0, 60.0][app.start.custom_fps.min(2)]));
                }
            });
        });
    });
    if let Some((w, h, fps)) = create {
        kit::set_open(ui.ctx(), custom_id, false);
        let mut p = view::empty();
        p.width = w;
        p.height = h;
        p.fps = fps;
        // Like a preset: its own folder at once, so it is saved and autosaved.
        p.name = crate::i18n::tf("Untitled custom".to_string());
        let ctx = ui.ctx().clone();
        app.replace(&ctx, files::Replace::New(p));
        return;
    }

    // Projects: recent ones, then the samples. Projects made elsewhere (a
    // terminal, an agent) join the list; looked for every few seconds.
    {
        static LAST: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);
        let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
        // Tests never read this computer's real list of projects.
        if !cfg!(test) && last.is_none_or(|t| t.elapsed() > std::time::Duration::from_secs(3)) {
            *last = Some(std::time::Instant::now());
            files::merge_shared(&mut app.prefs.recents);
        }
    }
    y += card_h + if wide { 36.0 } else { 88.0 };
    let has_recents = !app.prefs.recents.is_empty() && !app.cli.flag("no-recents");

    let q = app.start.search.to_lowercase();
    let recents: Vec<files::Recent> = app.prefs.recents.iter().filter(|r| q.is_empty() || r.name.to_lowercase().contains(&q)).cloned().collect();
    let samples: Vec<&view::Recent> = if app.cli.flag("no-samples") { vec![] } else { view::RECENTS.iter().filter(|r| q.is_empty() || r.name.to_lowercase().contains(&q)).collect() };
    let area = Rect::from_min_max(egui::pos2(col.left() - 4.0, y), egui::pos2(col.right() + 4.0, full.bottom()));
    let mut gui = ui.new_child(egui::UiBuilder::new().max_rect(area));
    if recents.is_empty() && samples.is_empty() {
        let (sentence, action) = if q.is_empty() {
            ("Projects you open or create will appear here.", Some(("New landscape project", Icon::Plus)))
        } else {
            ("No projects match your search.", Some(("Clear search", Icon::X)))
        };
        let r = Rect::from_min_size(area.min + Vec2::new(4.0, 0.0), Vec2::new(content_w, (area.height() - 24.0).clamp(160.0, 260.0)));
        gui.painter().rect_filled(r, CornerRadius::same(theme::radius::PANEL), t.panel);
        let mut eui = gui.new_child(egui::UiBuilder::new().max_rect(r));
        if kit::empty_state(&mut eui, Icon::FilmSlate, sentence, action) {
            if q.is_empty() {
                new_project(app, ui.ctx(), &PRESETS[0]);
            } else {
                app.start.search.clear();
            }
        }
        return;
    }
    let mut open: Option<std::path::PathBuf> = None;
    let mut open_sample: Option<&view::Recent> = None;
    let mut new_from: Option<view::Project> = None;
    egui::ScrollArea::vertical().auto_shrink([false, false]).show_v(&mut gui, |ui| {
        ui.add_space(theme::space::XS);
        ui.horizontal(|ui| {
            ui.add_space(theme::space::XS);
            ui.vertical(|ui| {
                ui.set_width(content_w);
                if recents.is_empty() && q.is_empty() {
                    // First run: say where to begin.
                    ui.label(egui::RichText::new(crate::i18n::shown("Welcome to Zornov")).font(theme::section_title()).color(t.text));
                    ui.label(egui::RichText::new(crate::i18n::shown("Pick a canvas above to start, open a project, or try a sample below")).font(theme::caption()).color(t.text_2));
                    ui.add_space(theme::space::MD);
                }
                // Searching shows every match; otherwise the latest few, unless
                // the person asked for all of them.
                let show_all_id = egui::Id::unique("start-show-all-recents");
                let show_all = !q.is_empty() || ui.ctx().data(|d| d.get_temp::<bool>(show_all_id).unwrap_or(false));
                let hidden = if show_all { 0 } else { recents.len().saturating_sub(RECENT_TILES) };
                let recents = &recents[..recents.len() - hidden];
                // Each project shows a picture from its own media, looked up a
                // few projects per frame so a long list never stalls a frame.
                let mut looked = 0;
                for r in recents {
                    if looked < 4 && !app.start.covers.contains_key(&r.path) {
                        app.start.covers.insert(r.path.clone(), files::cover_of(&r.path));
                        looked += 1;
                    }
                }
                if looked > 0 {
                    ui.ctx().request_repaint();
                }
                let covers: Vec<Option<u32>> = recents.iter().map(|r| app.start.covers.get(&r.path).cloned().flatten().map(|(p, secs)| app.thumbs.file_seed(&p, secs))).collect();
                if has_recents && !recents.is_empty() {
                    ui.add_space(theme::space::SM);
                    ui.label(egui::RichText::new(crate::i18n::shown("Recent projects")).font(theme::section_title()).color(t.text));
                    ui.add_space(theme::space::SM);
                    {
                        let thumbs = &mut app.thumbs;
                        let badges: Vec<String> = recents.iter().map(|r| view::format_duration(r.length)).collect();
                        let subs: Vec<String> = recents.iter().map(|r| files::edited_label(r.edited)).collect();
                        // Where each one lives, on hover (same names stay apart).
                        let tips: Vec<String> = recents.iter().map(|r| studio_paths::tilde(&r.path)).collect();
                        let out = kit::TileGrid::new("recents").min_width(168.0).surface(t.ground).show(
                            ui,
                            recents.len(),
                            |i| kit::TileInfo { label: recents[i].name.as_str(), badge: Some(badges[i].as_str()), sub: Some(subs[i].as_str()), selected: false, addable: false, progress: None, tip: Some(tips[i].as_str()), added: false },
                            |i| i,
                            &mut |p, r, i, hover, _| {
                                let rec = &recents[i];
                                let seed = covers[i].unwrap_or_else(|| files::seed_of(&rec.path));
                                letterboxed(p, thumbs, r, (rec.width.max(1), rec.height.max(1)), seed, hover);
                            },
                        );
                        if let Some(i) = out.clicked.or(out.double_clicked) {
                            open = Some(recents[i].path.clone());
                        }
                    }
                    if q.is_empty() && (hidden > 0 || show_all) {
                        ui.add_space(theme::space::SM);
                        let label = if hidden > 0 { crate::i18n::tf(format!("Show all {} projects", recents.len() + hidden)) } else { crate::i18n::tf("Show fewer".to_owned()) };
                        if kit::Button::ghost(&label).compact().show(ui).clicked() {
                            ui.ctx().data_mut(|d| d.insert_temp(show_all_id, hidden > 0));
                        }
                    }
                }
                if recents.is_empty() && app.start.search.is_empty() && !app.cli.flag("no-recents") {
                    // First run: say where projects will be, instead of
                    // leaving a gap.
                    ui.add_space(theme::space::SM);
                    ui.label(egui::RichText::new(crate::i18n::shown("Recent projects")).font(theme::section_title()).color(t.text));
                    ui.add_space(theme::space::SM);
                    let (r, _) = ui.allocate_exact_size(Vec2::new(content_w, 160.0), Sense::hover());
                    ui.painter().rect_filled(r, CornerRadius::same(theme::radius::PANEL), t.panel);
                    let mut eui = ui.new_child(egui::UiBuilder::new().max_rect(r));
                    kit::empty_state(&mut eui, Icon::FilmSlate, "Your projects will appear here. Start a new one above, or try a template below.", None);
                }
                // Returning users see their projects first; the templates
                // and samples follow.
                if app.start.search.is_empty() {
                    new_from = crate::templates::show_rows(app, ui);
                }
                if !samples.is_empty() {
                    let thumbs = &mut app.thumbs;
                    ui.add_space(theme::space::XL);
                    ui.label(egui::RichText::new(crate::i18n::shown("Samples")).font(theme::section_title()).color(t.text));
                    ui.add_space(theme::space::SM);
                    let badges: Vec<String> = samples.iter().map(|r| view::format_duration(r.seconds as i64 * view::SEC)).collect();
                    let out = kit::TileGrid::new("samples").min_width(168.0).surface(t.ground).show(
                        ui,
                        samples.len(),
                        |i| kit::TileInfo { label: samples[i].name, badge: Some(badges[i].as_str()), sub: Some(samples[i].edited), selected: false, addable: false, progress: None, tip: None, added: false },
                        |i| i,
                        &mut |p, r, i, hover, _| {
                            let rec = samples[i];
                            letterboxed(p, thumbs, r, rec.aspect, rec.seed, hover);
                        },
                    );
                    if let Some(i) = out.clicked.or(out.double_clicked) {
                        open_sample = Some(samples[i]);
                    }
                }
            });
        });
    });
    if let Some(p) = new_from {
        crate::templates::open(app, ui.ctx(), p);
        return;
    }
    if let Some(path) = open {
        let ctx = ui.ctx().clone();
        app.open_folder(&ctx, Some(path));
    } else if let Some(rec) = open_sample {
        let mut p = view::demo();
        if rec.name != "Weekend away" {
            p.name = rec.name.into();
        }
        let (aw, ah) = rec.aspect;
        p.height = (p.width as f32 * ah as f32 / aw as f32) as u32 / 2 * 2;
        if aw < ah {
            p.width = 1080;
            p.height = (1080.0 * ah as f32 / aw as f32) as u32 / 2 * 2;
        }
        let ctx = ui.ctx().clone();
        app.replace(&ctx, files::Replace::Sample(p));
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Side {
    Demo,
    Open,
    Import,
}

/// Another way in: an icon, a name and one line, on a quiet row.
fn action_row(ui: &mut egui::Ui, r: Rect, icon: Icon, title: &str, sub: &str) -> egui::Response {
    let t = theme::tokens(ui.ctx());
    let resp = ui.interact(r, egui::Id::unique(("home-action", title)), Sense::click());
    let st = kit::State::from_response(ui, &resp, None);
    let p = ui.painter();
    let cr = CornerRadius::same(theme::radius::PANEL);
    if st.hover > 0.0 {
        p.rect_filled(r, cr, theme::alpha(t.alt, st.hover));
    }
    let rtl = crate::i18n::is_rtl();
    let ic = if rtl { r.right() - 20.0 } else { r.left() + 20.0 };
    icons::paint(p, Rect::from_center_size(egui::pos2(ic, r.center().y), Vec2::splat(16.0)), icon, 16.0, t.accent);
    let (tx, align) = if rtl { (r.right() - 40.0, Align2::RIGHT_CENTER) } else { (r.left() + 40.0, Align2::LEFT_CENTER) };
    let title_cut = kit::paint_text_clipped(p, egui::pos2(tx, r.center().y - 8.0), align, title, theme::control_medium(), t.text, r.width() - 48.0);
    let sub_cut = kit::paint_text_clipped(p, egui::pos2(tx, r.center().y + 9.0), align, sub, theme::caption(), t.text_2, r.width() - 48.0);
    let resp = kit::tip_if_cut(resp, title_cut || sub_cut, &format!("{title}\n{sub}"));
    if st.focused {
        kit::focus_ring(p, r, cr, &t);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Draw a project picture in its own aspect, letterboxed in the tile.
fn letterboxed(p: &egui::Painter, thumbs: &mut crate::thumbs::Thumbs, r: Rect, aspect: (u32, u32), seed: u32, hover: f32) {
    let (aw, ah) = (aspect.0 as f32, aspect.1 as f32);
    let s = (r.width() / aw).min(r.height() / ah);
    let inner = Rect::from_center_size(r.center(), Vec2::new(aw * s, ah * s) * (1.0 + 0.03 * hover));
    // Size the picture from the tile at rest, in 64 px steps, so the hover
    // zoom never asks for a new picture every frame.
    let rest_w = aw * s * p.ctx().pixels_per_point();
    let pw = ((rest_w / 64.0).ceil() * 64.0).min(512.0) as u16;
    let ph = (pw as f32 * ah / aw) as u16;
    if let Some(tex) = thumbs.scene(seed, 0.3, pw.max(8), ph.max(8)) {
        p.image(tex, inner, Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
    }
}

fn new_project(app: &mut StudioApp, ctx: &egui::Context, p: &Preset) {
    let mut proj = view::empty();
    proj.width = p.w;
    proj.height = p.h;
    proj.name = crate::i18n::tf(format!("Untitled {}", p.name.to_lowercase()));
    app.replace(ctx, files::Replace::New(proj));
}

/// The Zornov wordmark: an accent glyph and the name.
pub fn wordmark(p: &egui::Painter, left_center: egui::Pos2, t: &theme::Tokens) {
    let c = left_center + Vec2::new(12.0, 0.0);
    // Glyph: a rounded square with a play notch, our own mark.
    let r = Rect::from_center_size(c, Vec2::splat(24.0));
    p.rect_filled(r, CornerRadius::same(7), t.accent);
    let tri = vec![c + Vec2::new(-3.5, -6.0), c + Vec2::new(6.5, 0.0), c + Vec2::new(-3.5, 6.0)];
    p.add(egui::Shape::convex_polygon(tri, t.on_accent(), egui::Stroke::NONE));
    p.text(c + Vec2::new(22.0, 0.0), Align2::LEFT_CENTER, "Zornov", egui::FontId::new(17.0, egui::FontFamily::Name(theme::SEMIBOLD.into())), t.text);
}

fn preset_card(ui: &mut egui::Ui, r: Rect, name: &str, ratio: &str, size: &str, hint: &str, aspect: Option<(u32, u32)>) -> egui::Response {
    let t = theme::tokens(ui.ctx());
    let resp = ui.interact(r, ui.scope_id().with(("preset", name)), Sense::click());
    let st = kit::State::from_response(ui, &resp, None);
    let p = ui.painter();
    let cr = CornerRadius::same(theme::radius::PANEL);
    p.rect_filled(r, cr, theme::mix(t.panel, t.hover(t.panel), st.hover));
    if st.hover > 0.0 {
        p.rect_stroke(r, cr, egui::Stroke::new(1.0, theme::alpha(t.accent, st.hover)), egui::StrokeKind::Inside);
    }
    if st.focused {
        kit::focus_ring(p, r, cr, &t);
    }
    // Aspect frame, fitted into a 64 px box at the top.
    let boxr = Rect::from_center_size(egui::pos2(r.center().x, r.top() + 46.0), Vec2::splat(52.0));
    let frame_c = theme::mix(t.text_2, t.accent, st.hover);
    match aspect {
        Some((w, h)) => {
            let s = (boxr.width() / w as f32).min(boxr.height() / h as f32);
            let fr = Rect::from_center_size(boxr.center(), Vec2::new(w as f32 * s, h as f32 * s));
            p.rect(fr, CornerRadius::same(4), theme::alpha(t.accent, 0.12 * st.hover), egui::Stroke::new(1.5, frame_c), egui::StrokeKind::Inside);
            p.text(fr.center(), Align2::CENTER_CENTER, ratio, theme::caption(), frame_c);
        }
        None => {
            let fr = Rect::from_center_size(boxr.center(), Vec2::new(46.0, 34.0));
            dashed_rect(p, fr, frame_c);
            icons::paint(p, fr, Icon::Plus, 16.0, frame_c);
        }
    }
    let name_cut = kit::paint_text_clipped(p, egui::pos2(r.center().x, r.bottom() - 40.0), Align2::CENTER_CENTER, name, theme::control_medium(), t.text, r.width() - 16.0);
    let sub = if hint.is_empty() { size.to_owned() } else { format!("{size} · {hint}") };
    let sub_cut = kit::paint_text_clipped(p, egui::pos2(r.center().x, r.bottom() - 21.0), Align2::CENTER_CENTER, &sub, theme::caption(), t.text_2, r.width() - 16.0);
    kit::tip_if_cut(resp, name_cut || sub_cut, &format!("{name}\n{sub}")).on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn dashed_rect(p: &egui::Painter, r: Rect, c: Color32) {
    let s = egui::Stroke::new(1.5, c);
    let pts = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()];
    for w in pts.windows(2) {
        p.extend(egui::Shape::dashed_line(&[w[0], w[1]], s, 4.0, 3.0));
    }
}

/// Lets the user drag the window from an empty strip (the title bar is
/// hidden so content runs under the window buttons).
pub fn window_drag_area(ui: &egui::Ui, rect: Rect) {
    let resp = ui.interact(rect, egui::Id::unique("window-drag"), Sense::click_and_drag());
    if resp.drag_started() {
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
    }
    if resp.double_clicked() {
        let max = ui.input(|i| i.viewport().maximized.unwrap_or(false));
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Maximized(!max));
    }
}

#[cfg(test)]
mod tests {
    use crate::ui_tests::Harness;
    use eframe::egui::{self, Event, PointerButton};

    /// Typing in the search field narrows the projects and samples.
    #[test]
    fn typing_in_search_filters_the_projects() {
        let mut h = Harness::new(&[]);
        h.app.prefs.recents = vec![crate::files::Recent { path: "/tmp/zq-test/Zq project".into(), name: "Zq project".into(), edited: 0, length: 0, width: 1920, height: 1080 }];
        h.frames(3);
        let before = h.labels();
        assert!(before.iter().any(|l| l.contains("Zq project")), "a recent project is listed first: {before:?}");
        // The search field sits at the right end of the header.
        let at = egui::pos2(1200.0, 66.0);
        let press = |down| Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: down, modifiers: egui::Modifiers::NONE };
        h.with_events(vec![Event::PointerMoved(at)]);
        h.with_events(vec![press(true)]);
        h.with_events(vec![press(false)]);
        h.with_events(vec![Event::Text("zzqx".into())]);
        h.frames(2);
        assert_eq!(h.app.start.search, "zzqx", "the typed text reaches the field");
        let after = h.labels();
        assert!(!after.iter().any(|l| l.contains("Zq project")), "projects are filtered out: {after:?}");
        assert!(after.iter().any(|l| l == "Clear search"), "the empty search offers to clear it: {after:?}");
    }

    /// Recent projects come before the templates, then the samples.
    #[test]
    fn recent_projects_come_first() {
        let mut h = Harness::new(&[]);
        h.app.prefs.recents = vec![crate::files::Recent { path: "/tmp/zq-test/Zq project".into(), name: "Zq project".into(), edited: 0, length: 0, width: 1920, height: 1080 }];
        h.frames(3);
        let nodes = &h.last.as_ref().unwrap().platform_output.accesskit_update.as_ref().unwrap().nodes;
        let top = |name: &str| nodes.iter().filter(|(_, n)| n.label().or(n.value()) == Some(name)).filter_map(|(_, n)| n.bounds()).map(|b| b.y0).fold(f64::INFINITY, f64::min);
        let (recent, templates, samples) = (top("Recent projects"), top("Start from a template"), top("Samples"));
        assert!(recent.is_finite() && templates.is_finite(), "both headings are drawn");
        assert!(recent < templates, "recent projects ({recent}) above templates ({templates})");
        assert!(samples.is_infinite() || templates < samples, "samples last");
    }

    /// A long history shows the latest projects as covers, and the rest
    /// behind "Show all"; search still finds every one.
    #[test]
    fn a_long_history_shows_the_latest_and_the_rest_on_request() {
        let mut h = Harness::new(&[]);
        h.app.prefs.recents = (0..20).map(|i| crate::files::Recent { path: format!("/tmp/zq-test/P{i}").into(), name: format!("Zq list {i}"), edited: 0, length: 0, width: 1920, height: 1080 }).collect();
        h.frames(3);
        let l = h.labels();
        let shown = |l: &[String], i: usize| l.iter().any(|s| s == &format!("Zq list {i}"));
        assert!((0..super::RECENT_TILES).all(|i| shown(&l, i)), "the latest are shown: {l:?}");
        assert!(!shown(&l, 19), "the oldest waits behind Show all: {l:?}");
        assert!(l.iter().any(|s| s == "Show all 20 projects"), "{l:?}");
        h.app.start.search = "list 19".into();
        h.frames(3);
        assert!(shown(&h.labels(), 19), "search finds an old one");
    }

}
