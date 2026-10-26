// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Library: the icon rail and the content pane (search, category chips,
//! grid or list). Items apply to the selection on click, drag onto the
//! timeline or viewer, and add without selecting via the "+" button.

use super::clip_fill;
use crate::app::StudioApp;
use crate::icons::{self, Icon};
use crate::kit::{self, Button, IconButton, TileGrid, TileInfo};
use egui::Stroke;
use crate::view::{self, ClipKind, LaneKind, MediaKind, SEC, Time};
use crate::catalogue;
use crate::theme;
use crate::thumbs::{self, Key, Pic};
use studio_model as m;
use eframe::egui::{self, Align2, Color32, CornerRadius, Painter, Pos2, Rect, Sense, Vec2};
use crate::kit::ShowNoted;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RailItem {
    Media,
    Assets,
    Text,
    Motions,
    Effects,
    Cuts,
    Looks,
    Audio,
    Ai,
    Story,
}

impl RailItem {
    pub const ALL: [RailItem; 10] = [RailItem::Media, RailItem::Assets, RailItem::Text, RailItem::Motions, RailItem::Effects, RailItem::Cuts, RailItem::Looks, RailItem::Audio, RailItem::Ai, RailItem::Story];
    pub fn name(self) -> &'static str {
        match self {
            RailItem::Media => "Media",
            RailItem::Assets => "Assets",
            RailItem::Text => "Text",
            RailItem::Motions => "Motions",
            RailItem::Effects => "Effects",
            RailItem::Cuts => "Cuts",
            RailItem::Looks => "Looks",
            RailItem::Audio => "Audio",
            RailItem::Ai => "AI",
            RailItem::Story => "Story",
        }
    }
    pub fn icon(self) -> Icon {
        match self {
            RailItem::Media => Icon::FilmStrip,
            RailItem::Assets => Icon::Shapes,
            RailItem::Text => Icon::TextT,
            RailItem::Motions => Icon::Path,
            RailItem::Effects => Icon::MagicWand,
            RailItem::Cuts => Icon::Intersect,
            RailItem::Looks => Icon::Palette,
            RailItem::Audio => Icon::MusicNotes,
            RailItem::Ai => Icon::Sparkle,
            RailItem::Story => Icon::FilmSlate,
        }
    }
    pub fn index(self) -> usize {
        Self::ALL.iter().position(|r| *r == self).unwrap_or(0)
    }
    fn shortcut(self) -> &'static str {
        ["1", "2", "3", "4", "5", "6", "7", "8", "9", "0"][self.index()]
    }
    fn groups(self) -> &'static [&'static str] {
        match self {
            RailItem::Media => &["All", "Video", "Images", "Audio"],
            RailItem::Assets if crate::content::find_dir().is_some() => super::library_content::ASSET_GROUPS,
            RailItem::Assets => view::ASSET_GROUPS,
            RailItem::Text => TEXT_GROUPS,
            RailItem::Motions => catalogue::MOTION_GROUPS,
            RailItem::Effects => catalogue::EFFECT_GROUPS,
            RailItem::Cuts => catalogue::CUT_GROUPS,
            RailItem::Looks => catalogue::LOOK_GROUPS,
            RailItem::Audio if crate::content::find_dir().is_some() => super::library_content::AUDIO_GROUPS,
            RailItem::Audio => view::SOUND_GROUPS,
            RailItem::Ai => &["AI editor", "All tools", "Installed", "Available"],
            RailItem::Story => super::story::GROUPS,
        }
    }
}

/// Text style groups (the `text` crate's categories).
pub const TEXT_GROUPS: &[&str] = &["All", "Clean", "Bold", "Outline", "Neon", "Retro", "Playful", "Elegant", "Cinematic", "Social", "Handmade"];

/// Payload carried while dragging a Library item.
#[derive(Clone, Copy, Debug)]
pub struct LibDrag {
    pub rail: RailItem,
    pub index: usize,
}

/// Height of the section tab strip at the top of the library card.
pub const TABS_H: f32 = 50.0;
/// Width of the category column.
pub const CATEGORY_W: f32 = 84.0;

/// The section tabs: a row of icon-over-label tabs, the active one in the
/// accent. Number keys 1-9 switch sections; clicking the active one
/// collapses the library.
pub fn rail(app: &mut StudioApp, ui: &mut egui::Ui, r: Rect) {
    let t = theme::tokens(ui.ctx());
    let widths: Vec<f32> = RailItem::ALL.iter().map(|item| (ui.fonts_mut(|f| f.layout_no_wrap(item.name().to_string(), theme::caption(), t.text).size().x) + 14.0).max(40.0)).collect();
    let natural = 6.0 + widths.iter().map(|w| w + 2.0).sum::<f32>() + 4.0;
    // A library narrower than its tabs scrolls them sideways.
    let mut strip = ui.new_child(egui::UiBuilder::new().max_rect(r).id_salt("lib-rail"));
    strip.set_clip_rect(r.intersect(ui.clip_rect()));
    kit::scroll_strip(&mut strip, "lib-rail-scroll", |ui| {
        let (area, _) = ui.allocate_exact_size(Vec2::new(natural.max(r.width()), r.height()), Sense::hover());
        rail_cells(app, ui, area, &widths);
    });
    ui.painter().hline(r.x_range(), r.bottom() - 0.5, Stroke::new(1.0, t.line));
}

fn rail_cells(app: &mut StudioApp, ui: &mut egui::Ui, r: Rect, widths: &[f32]) {
    let t = theme::tokens(ui.ctx());
    let ed = &mut app.editor;
    let mut open_ai = false;
    let mut x = r.left() + 6.0;
    for (item, w) in RailItem::ALL.into_iter().zip(widths.iter().copied()) {
        let name = item.name();
        let cell = Rect::from_min_size(egui::pos2(x, r.top() + 4.0), Vec2::new(w, r.height() - 8.0));
        let resp = ui.interact(cell, egui::Id::unique(("rail", item.index())), Sense::click());
        let st = kit::State::from_response(ui, &resp, None);
        let active = ed.rail == item && ed.dock.is_open(crate::dock::Panel::Library);
        let p = ui.painter();
        let fg = if active { t.accent } else { theme::mix(t.text, t.text, st.hover) };
        let ir = Rect::from_center_size(egui::pos2(cell.center().x, cell.top() + 13.0), Vec2::splat(16.0));
        icons::paint(p, ir, item.icon(), 16.0, fg);
        p.text(egui::pos2(cell.center().x, cell.top() + 32.0), Align2::CENTER_CENTER, name, theme::caption(), fg);
        if st.focused {
            kit::focus_ring(p, cell, CornerRadius::same(theme::radius::CONTROL), &t);
        }
        resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Tab, true, active, name));
        let resp = kit::tip(resp, name, Some(item.shortcut()));
        let key_hit = !ui.ctx().egui_wants_keyboard_input()
            && ui.input_mut(|i| i.consume_shortcut(&egui::KeyboardShortcut::new(egui::Modifiers::NONE, [egui::Key::Num1, egui::Key::Num2, egui::Key::Num3, egui::Key::Num4, egui::Key::Num5, egui::Key::Num6, egui::Key::Num7, egui::Key::Num8, egui::Key::Num9, egui::Key::Num0][item.index()])));
        if resp.clicked() || key_hit {
            ed.rail = item;
            if item == RailItem::Ai {
                // The AI editor is a panel of its own: this opens (or shows) it,
                // and the Library keeps the AI tools.
                open_ai = true;
            } else {
                ed.open_panel(crate::dock::Panel::Library);
            }
        }
        x += w + 2.0;
    }
    if open_ai {
        super::agent_panel::open(app);
    }
}

/// The category column: one text row per group, the selected row on the
/// alternate panel tone with accent text.
fn categories(ui: &mut egui::Ui, r: Rect, groups: &[&str], chip: &mut usize) {
    // More groups than the column is tall: it scrolls.
    let mut col = ui.new_child(egui::UiBuilder::new().max_rect(r).id_salt("lib-cats"));
    col.set_clip_rect(r.intersect(ui.clip_rect()));
    egui::ScrollArea::vertical().id_salt("lib-cats-scroll").auto_shrink([false, false]).show_v(&mut col, |ui| {
        let t = theme::tokens(ui.ctx());
        ui.spacing_mut().item_spacing.y = 1.0;
        ui.add_space(6.0);
        for (i, g) in groups.iter().enumerate() {
            let (full, _) = ui.allocate_exact_size(Vec2::new(r.width(), 24.0), Sense::hover());
            let row = full.shrink2(Vec2::new(4.0, 0.0));
            let resp = ui.interact(row, egui::Id::unique(("lib-cat", i, g.len())), Sense::click());
            let sel = *chip == i;
            let p = ui.painter();
            if sel || resp.hovered() {
                p.rect_filled(row, CornerRadius::same(theme::radius::CONTROL), t.alt);
            }
            // A name wider than the column ends in an ellipsis; the hover
            // shows it whole.
            let cut = kit::paint_text_clipped(p, egui::pos2(row.left() + 8.0, row.center().y), Align2::LEFT_CENTER, g, theme::body(), if sel { t.accent } else { t.text }, row.width() - 12.0);
            let resp = kit::tip_if_cut(resp, cut, g);
            if resp.clicked() {
                *chip = i;
            }
        }
        ui.add_space(6.0);
    });
}

pub fn show(app: &mut StudioApp, ui: &mut egui::Ui, r: Rect) {
    let t = theme::tokens(ui.ctx());
    let pad = theme::space::PANEL_PAD;
    let inner = r.shrink(pad);
    let rail = app.editor.rail;

    let _ = inner;
    // Section tabs across the top of the card.
    let tabs = Rect::from_min_size(r.min, Vec2::new(r.width(), TABS_H));
    self::rail(app, ui, tabs);
    let rail = app.editor.rail;
    // Category column on the left, content on the right.
    let col = Rect::from_min_max(egui::pos2(r.left(), tabs.bottom()), egui::pos2(r.left() + CATEGORY_W, r.bottom()));
    let mut chip = app.editor.library_chip[rail.index()].min(rail.groups().len().saturating_sub(1));
    categories(ui, col, rail.groups(), &mut chip);
    ui.painter().vline(col.right() - 0.5, col.y_range(), Stroke::new(1.0, t.line));
    if rail == RailItem::Ai && chip == 0 {
        // "AI editor" opens the AI editor's own panel; the Library goes on
        // with the AI tools.
        super::agent_panel::open(app);
        chip = 1;
        app.editor.library_chip[rail.index()] = chip;
    }
    if rail == RailItem::Story {
        // The production plan: storyboard, script, places, cast, check.
        app.editor.library_chip[rail.index()] = chip;
        let area = Rect::from_min_max(egui::pos2(col.right(), tabs.bottom() + 6.0), r.max - Vec2::new(0.0, 4.0));
        super::story::panel(app, ui, chip, area);
        return;
    }
    let inner = Rect::from_min_max(egui::pos2(col.right() + 8.0, tabs.bottom() + 8.0), r.max - Vec2::new(8.0, 8.0));

    // Search, with the import action beside it for media.
    let mut search_w = inner.width();
    if rail == RailItem::Media {
        // Download from a link, always; import from this Mac once there is media.
        search_w -= 30.0;
        let lr = Rect::from_min_size(egui::pos2(inner.right() - 26.0, inner.top()), Vec2::splat(26.0));
        let mut lui = ui.new_child(egui::UiBuilder::new().max_rect(lr));
        if IconButton::new(Icon::LinkSimple, "Download from a link").show(&mut lui).clicked() {
            super::agent_panel::open_link_dialog(app, ui.ctx());
        }
        if !app.doc.project.media.is_empty() {
            search_w -= 30.0;
            let br = Rect::from_min_size(egui::pos2(inner.right() - 56.0, inner.top()), Vec2::splat(26.0));
            let mut bui = ui.new_child(egui::UiBuilder::new().max_rect(br));
            if IconButton::new(Icon::DownloadSimple, "Import media").shortcut("⌘I").show(&mut bui).clicked() {
                super::run(app, &ui.ctx().clone(), super::Action::ImportMedia);
            }
        }
    }
    let search = Rect::from_min_size(inner.min, Vec2::new(search_w, 26.0));
    let mut sui = ui.new_child(egui::UiBuilder::new().max_rect(search));
    let hint = crate::i18n::tf(format!("Search {}", rail.name().to_lowercase()));
    kit::text_field(&mut sui, &mut app.editor.library_query, if rail == RailItem::Ai { "Search AI tools" } else { &hint }, Some(Icon::MagnifyingGlass), search_w, None);
    let chips_r = Rect::from_min_max(egui::pos2(inner.left(), search.bottom()), egui::pos2(inner.right(), search.bottom()));
    let r = Rect::from_min_max(egui::pos2(inner.left() - pad, r.top()), r.max);
    app.editor.library_chip[rail.index()] = chip;

    let body = Rect::from_min_max(egui::pos2(r.left(), chips_r.bottom() + 12.0), r.max - Vec2::new(0.0, 4.0));
    let mut bui = ui.new_child(egui::UiBuilder::new().max_rect(body));
    bui.set_clip_rect(body.intersect(ui.clip_rect()));
    let query = app.editor.library_query.to_lowercase();

    if matches!(rail, RailItem::Assets | RailItem::Audio) && super::library_content::panel(app, ui, rail, chip, &query, body) {
        return;
    }
    let items = items_for(app, rail, chip, &query);
    if items.is_empty() {
        let mut eui = bui.new_child(egui::UiBuilder::new().max_rect(body.shrink(pad)));
        let clicked = if rail == RailItem::Media && app.doc.project.media.is_empty() && query.is_empty() {
            kit::empty_state(&mut eui, Icon::FilmStrip, "Bring in video, photos and sound to start cutting.", Some(("Import media", Icon::DownloadSimple)))
        } else {
            kit::empty_state(&mut eui, Icon::MagnifyingGlass, "Nothing matches that search.", Some(("Clear search", Icon::X)))
        };
        if clicked {
            if rail == RailItem::Media && app.doc.project.media.is_empty() && query.is_empty() {
                super::run(app, &ui.ctx().clone(), super::Action::ImportMedia);
            } else {
                app.editor.library_query.clear();
                app.editor.library_chip[rail.index()] = 0;
            }
        }
        return;
    }

    egui::ScrollArea::vertical().id_salt(("lib-scroll", rail.index())).auto_shrink([false, false]).show_v(&mut bui, |ui| {
        egui::Frame::new().inner_margin(egui::Margin { left: pad as i8, right: pad as i8, top: 2, bottom: 8 }).show(ui, |ui| {
            if rail == RailItem::Ai {
                ai_list(app, ui, &items);
            } else {
                // The category as a heading over the grid.
                if rail != RailItem::Media {
                    let chip = app.editor.library_chip[rail.index()];
                    let name = rail.groups().get(chip).copied().unwrap_or("All");
                    ui.label(egui::RichText::new(name).font(theme::body()).color(theme::tokens(ui.ctx()).text));
                    ui.add_space(2.0);
                }
                grid(app, ui, rail, &items);
            }
        });
    });
}

/// Indices into the catalogue for this rail item, after chip and search
/// filters.
fn items_for(app: &StudioApp, rail: RailItem, chip: usize, q: &str) -> Vec<usize> {
    let names: Vec<String> = match rail {
        RailItem::Media => app.doc.project.media.iter().map(|m| m.name.clone()).collect(),
        RailItem::Assets => view::ASSETS.iter().map(|s| s.to_string()).collect(),
        RailItem::Text => studio_text::styles::catalogue().iter().map(|s| s.name.clone()).collect(),
        RailItem::Motions => catalogue::motions().iter().map(|s| s.name.to_string()).collect(),
        RailItem::Effects => catalogue::effects().iter().map(|s| s.name.to_string()).collect(),
        RailItem::Cuts => catalogue::cuts().iter().map(|s| s.name.to_string()).collect(),
        RailItem::Looks => catalogue::looks().iter().map(|s| s.name.to_string()).collect(),
        RailItem::Audio => view::SOUNDS.iter().map(|s| s.0.to_string()).collect(),
        RailItem::Ai => view::AI_TOOLS.iter().map(|s| s.name.to_string()).collect(),
        RailItem::Story => Vec::new(),
    };
    let n = rail.groups().len().max(1);
    names
        .iter()
        .enumerate()
        .filter(|(i, name)| {
            let in_group = chip == 0
                || match rail {
                    RailItem::Media => {
                        let k = app.doc.project.media[*i].kind;
                        matches!((chip, k), (1, MediaKind::Video) | (2, MediaKind::Image) | (3, MediaKind::Audio))
                    }
                    RailItem::Audio => {
                        let d = view::SOUNDS[*i].1;
                        matches!(chip, 1 if d > 20.0 && !name.contains("ambience") && !name.contains("hum")) || matches!(chip, 2 if d < 5.0) || matches!(chip, 3 if name.contains("ambience") || name.contains("hum"))
                    }
                    // Installed: it runs now (its model is in the models
                    // folder, or it needs none). It was a fixed list.
                    RailItem::Ai => chip == 1 || (chip == 2) == ai_feature(view::AI_TOOLS[*i].name).map(crate::services::models::feature).is_some_and(|r| r.ready),
                    RailItem::Text => studio_text::styles::StyleCategory::ALL.get(chip - 1) == Some(&studio_text::styles::catalogue()[*i].category),
                    RailItem::Motions => catalogue::motions()[*i].group == chip,
                    RailItem::Effects => catalogue::effects()[*i].group == chip,
                    RailItem::Cuts => catalogue::cuts()[*i].group == chip,
                    RailItem::Looks => catalogue::looks()[*i].group == chip,
                    // Spread catalogue items over the groups.
                    _ => (i % (n - 1)) + 1 == chip,
                };
            in_group && (q.is_empty() || name.to_lowercase().contains(q))
        })
        .map(|(i, _)| i)
        .collect()
}

fn grid(app: &mut StudioApp, ui: &mut egui::Ui, rail: RailItem, items: &[usize]) {
    let t = theme::tokens(ui.ctx());
    let names: Vec<String> = items.iter().map(|&i| item_name(app, rail, i)).collect();
    let badges: Vec<Option<String>> = items.iter().map(|&i| badge(app, rail, i)).collect();
    let selected = app.editor.library_selected;
    let now = ui.input(|i| i.time);
    let progress: Vec<Option<f32>> = items
        .iter()
        .map(|&i| {
            if rail != RailItem::Media {
                return None;
            }
            let id = app.doc.project.media[i].id;
            app.editor_import_progress(id, now)
        })
        .collect();
    let tips: Vec<Option<&'static str>> = items
        .iter()
        .map(|&i| match rail {
            RailItem::Effects => Some(catalogue::effects()[i].description),
            RailItem::Cuts => Some(catalogue::cuts()[i].description),
            RailItem::Motions => Some(catalogue::motions()[i].description),
            _ => None,
        })
        .collect();
    // Media already on the timeline gets the "added" badge.
    let used: std::collections::HashSet<u32> = app.doc.project.clips.iter().filter_map(|c| c.media).collect();
    let added: Vec<bool> = items.iter().map(|&i| rail == RailItem::Media && used.contains(&app.doc.project.media[i].id)).collect();
    let info = |k: usize| TileInfo {
        label: names[k].as_str(),
        badge: badges[k].as_deref(),
        sub: None,
        selected: selected == Some((rail, items[k])),
        addable: rail != RailItem::Ai,
        progress: progress[k],
        tip: tips[k],
        added: added[k],
    };
    let media = app.doc.project.media.clone();
    let thumbs = &mut app.thumbs;
    let mut paint = |p: &Painter, r: Rect, k: usize, hover: f32, live: bool| paint_thumb(thumbs, &media, rail, items[k], p, r, hover, live, &t);
    let out = if app.prefs.library_list {
        kit::tile_list(ui, rail.name(), items.len(), info, |k| LibDrag { rail, index: items[k] }, &mut paint)
    } else {
        // Text styles as wide tiles, five or so a row; effects, cuts and
        // looks small and square, six a row; media four a row.
        let (aspect, min) = match rail {
            RailItem::Text => (1.6, 78.0),
            RailItem::Assets => (1.0, 70.0),
            RailItem::Effects | RailItem::Cuts | RailItem::Looks | RailItem::Motions => (1.0, 62.0),
            _ => (1.45, 88.0),
        };
        TileGrid::new(rail.name()).aspect(aspect).min_width(min).centred().show(ui, items.len(), info, |k| LibDrag { rail, index: items[k] }, &mut paint)
    };
    let ctx = ui.ctx().clone();
    if let Some(k) = out.clicked {
        app.editor.library_selected = Some((rail, items[k]));
        if rail != RailItem::Media {
            apply(app, &ctx, rail, items[k], None);
        }
    }
    if let Some(k) = out.added.or(if rail == RailItem::Media { out.double_clicked } else { None }) {
        add(app, &ctx, rail, items[k], None, None);
    }
}

fn item_name(app: &StudioApp, rail: RailItem, i: usize) -> String {
    match rail {
        RailItem::Media => app.doc.project.media[i].name.clone(),
        RailItem::Assets => view::ASSETS[i].into(),
        RailItem::Text => studio_text::styles::catalogue()[i].name.clone(),
        RailItem::Motions => catalogue::motions()[i].name.into(),
        RailItem::Effects => catalogue::effects()[i].name.into(),
        RailItem::Cuts => catalogue::cuts()[i].name.into(),
        RailItem::Looks => catalogue::looks()[i].name.into(),
        RailItem::Audio => view::SOUNDS[i].0.into(),
        RailItem::Ai => view::AI_TOOLS[i].name.into(),
        RailItem::Story => String::new(),
    }
}

fn badge(app: &StudioApp, rail: RailItem, i: usize) -> Option<String> {
    match rail {
        RailItem::Media => {
            let m = &app.doc.project.media[i];
            match m.kind {
                MediaKind::Image => None,
                _ => Some(view::format_short(m.dur)),
            }
        }
        RailItem::Audio => Some(view::format_short((view::SOUNDS[i].1 as f64 * SEC as f64) as Time)),
        _ => None,
    }
}

const FULL_UV: Rect = Rect { min: Pos2 { x: 0.0, y: 0.0 }, max: Pos2 { x: 1.0, y: 1.0 } };

fn thumb_px(p: &Painter, r: Rect) -> (u16, u16) {
    // Pick from a few fixed sizes so the cache stays small.
    let w = r.width() * p.ctx().pixels_per_point();
    let w = if w <= 200.0 { 192 } else if w <= 330.0 { 320 } else { 480 };
    (w, (w as f32 * 9.0 / 16.0) as u16)
}

#[allow(clippy::too_many_arguments)]
fn paint_thumb(thumbs: &mut thumbs::Thumbs, media: &[view::MediaItem], rail: RailItem, i: usize, p: &Painter, r: Rect, hover: f32, live: bool, t: &theme::Tokens) {
    let clock = if live { kit::clock(p.ctx()) } else { 0.0 };
    let cover = |r: Rect, aspect: f32| {
        // Crop-to-fill UVs for a 16:9 picture shown in `r`.
        let ra = r.width() / r.height();
        if ra > aspect {
            let h = aspect / ra;
            Rect::from_min_max(egui::pos2(0.0, 0.5 - h / 2.0), egui::pos2(1.0, 0.5 + h / 2.0))
        } else {
            let w = ra / aspect;
            Rect::from_min_max(egui::pos2(0.5 - w / 2.0, 0.0), egui::pos2(0.5 + w / 2.0, 1.0))
        }
    };
    match rail {
        RailItem::Media => {
            let m = &media[i];
            if m.kind == MediaKind::Audio {
                waveform_tile(p, r, m.seed, live.then_some(clock), t);
                return;
            }
            // Hover plays through the footage.
            let tt = if live && m.kind == MediaKind::Video { ((clock * 0.25) % 1.0) as f32 } else { 0.2 };
            let tt = (tt * 24.0).floor() / 24.0;
            let (w, h) = thumb_px(p, r);
            let seed = thumbs.seed_of(m);
            if let Some(tex) = thumbs.scene(seed, tt, w, h) {
                p.image(tex, r, cover(r, 16.0 / 9.0), Color32::WHITE);
            }
            if m.kind == MediaKind::Image {
                icons::paint(p, Rect::from_min_size(r.left_bottom() + Vec2::new(6.0, -22.0), Vec2::splat(16.0)), Icon::Image, 14.0, Color32::from_white_alpha(230));
            }
        }
        RailItem::Effects => {
            let frame = if live { ((clock * 10.0) as u8) % thumbs::PREVIEW_FRAMES } else { 0 };
            let (w, h) = thumb_px(p, r);
            let key = Key { pic: Pic::Effect { index: i as u8, frame }, w, h };
            let id = thumbs.get(key).or_else(|| thumbs.get(Key { pic: Pic::Effect { index: i as u8, frame: 0 }, w, h }));
            if let Some(tex) = id {
                p.image(tex, r, FULL_UV, Color32::WHITE);
            }
        }
        RailItem::Looks => {
            let (w, h) = thumb_px(p, r);
            let looked = thumbs.get(Key { pic: Pic::Look { index: i as u8 }, w, h });
            if live {
                // Hover: a wipe sweeps between the plain photo and the look.
                let plain = thumbs.get(Key { pic: Pic::Sample { frame: 0 }, w, h });
                let k = (0.5 - 0.5 * (clock * 2.2).cos()) as f32;
                let split = egui::lerp(r.left()..=r.right(), 0.15 + 0.7 * k);
                if let Some(tex) = looked {
                    p.image(tex, r, FULL_UV, Color32::WHITE);
                }
                if let Some(tex) = plain {
                    let rr = Rect::from_min_max(egui::pos2(split, r.top()), r.max);
                    let u = (split - r.left()) / r.width();
                    p.image(tex, rr, Rect::from_min_max(egui::pos2(u, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
                }
                p.vline(split, r.y_range(), egui::Stroke::new(1.5, Color32::WHITE));
            } else if let Some(tex) = looked {
                p.image(tex, r, FULL_UV, Color32::WHITE);
            }
        }
        RailItem::Motions => {
            let (w, h) = thumb_px(p, r);
            p.rect_filled(r, 0, t.sunken);
            let Some(tex) = thumbs.get(Key { pic: Pic::Sample { frame: 0 }, w, h }) else { return };
            let item = &catalogue::motions()[i];
            let looping = item.group == 2;
            let pose = if live { motion_preview(item.id, clock as f32) } else { studio_fx::Pose::REST };
            let base = Rect::from_center_size(r.center(), r.size() * 0.62);
            draw_posed(p, tex, base, r, &pose);
            if !live {
                // Idle: a quiet glyph says what kind of motion it is.
                let g = Rect::from_min_size(r.right_top() + Vec2::new(-24.0, 6.0), Vec2::splat(18.0));
                p.rect_filled(g, CornerRadius::same(5), Color32::from_black_alpha(140));
                icons::paint(p, g, if looping { Icon::Repeat } else { Icon::Path }, 12.0, Color32::WHITE);
            }
        }
        RailItem::Cuts => {
            let (w, h) = thumb_px(p, r);
            // Idle: one frame a third of the way through; hover loops it.
            let frame = if live { let c = (clock * 0.8) % 1.0; ((((c - 0.15) / 0.7).clamp(0.0, 1.0)) * (thumbs::PREVIEW_FRAMES - 1) as f64).round() as u8 } else { 4 };
            let key = Key { pic: Pic::Cut { index: i as u8, frame }, w, h };
            let id = thumbs.get(key).or_else(|| thumbs.get(Key { pic: Pic::Cut { index: i as u8, frame: 4 }, w, h }));
            if let Some(tex) = id {
                p.image(tex, r, FULL_UV, Color32::WHITE);
            }
        }
        RailItem::Text => {
            // The style itself, rendered by the text engine: "Aa" at rest,
            // its sample line on hover.
            let (w, h) = (thumb_px(p, r).0, thumb_px(p, r).0);
            let key = Key { pic: Pic::TextStyle { index: i as u16, sample: live }, w, h };
            let id = thumbs.get(key).or_else(|| thumbs.get(Key { pic: Pic::TextStyle { index: i as u16, sample: false }, w, h }));
            match id {
                Some(tex) => {
                    p.image(tex, r, FULL_UV, Color32::WHITE);
                }
                None => {
                    p.rect_filled(r, 0, t.sunken);
                }
            }
        }
        RailItem::Assets => {
            p.rect_filled(r, 0, t.sunken);
            let bob = if live { (clock * 3.0).sin() as f32 * 0.06 } else { 0.0 };
            asset_shape(p, Rect::from_center_size(r.center(), r.size() * (0.55 + bob)), i, if live { clock as f32 } else { 0.0 }, t);
        }
        RailItem::Audio => waveform_tile(p, r, 200 + i as u32, live.then_some(clock), t),
        RailItem::Ai | RailItem::Story => {}
    }
    let _ = hover;
}

/// Pose of a motion preset in a looping hover preview `t` seconds in.
pub fn motion_preview(id: &str, t: f32) -> studio_fx::Pose {
    let Some(def) = studio_fx::motion::find(id) else { return studio_fx::Pose::REST };
    let params = def.defaults();
    match def.kind {
        studio_fx::motion::MotionKind::Loop => def.loop_pose(t, &params),
        // Play the entry, hold at rest a moment, repeat.
        studio_fx::motion::MotionKind::InOut => def.pose(studio_fx::Phase::Entry, (t % 1.4) / 0.9, &params),
    }
}

/// Draw `tex` filling `base` (its resting place) moved by `pose`; offsets
/// are fractions of `frame`.
pub fn draw_posed(p: &Painter, tex: egui::TextureId, base: Rect, frame: Rect, pose: &studio_fx::Pose) {
    let c = base.center() + Vec2::new(pose.offset[0] * frame.width(), pose.offset[1] * frame.height());
    let size = Vec2::new(base.width() * pose.scale[0], base.height() * pose.scale[1]);
    let [l, tp, rr, b] = pose.clip;
    let rect = Rect::from_center_size(c, size);
    // Clip insets crop the layer (wipe-style reveals).
    let shown = Rect::from_min_max(rect.min + Vec2::new(l * rect.width(), tp * rect.height()), rect.max - Vec2::new(rr * rect.width(), b * rect.height()));
    let uv = Rect::from_min_max(egui::pos2(l, tp), egui::pos2(1.0 - rr, 1.0 - b));
    let tint = Color32::from_white_alpha((pose.opacity.clamp(0.0, 1.0) * 255.0) as u8);
    image_rotated_uv(p, tex, shown, uv, pose.rotation.to_radians(), tint);
}

/// Draw a texture rotated by `angle` radians about the rect centre.
pub fn image_rotated(p: &Painter, tex: egui::TextureId, r: Rect, angle: f32, tint: Color32) {
    image_rotated_uv(p, tex, r, FULL_UV, angle, tint);
}

pub fn image_rotated_uv(p: &Painter, tex: egui::TextureId, r: Rect, uv: Rect, angle: f32, tint: Color32) {
    if angle.abs() < 1e-4 {
        p.image(tex, r, uv, tint);
        return;
    }
    let c = r.center();
    let rot = egui::emath::Rot2::from_angle(angle);
    let mut mesh = egui::Mesh::with_texture(tex);
    let corners = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom()];
    let uvs = [uv.left_top(), uv.right_top(), uv.right_bottom(), uv.left_bottom()];
    for (pt, uv) in corners.iter().zip(uvs) {
        mesh.vertices.push(egui::epaint::Vertex { pos: c + rot * (*pt - c), uv, color: tint });
    }
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    p.add(egui::Shape::mesh(mesh));
}

fn waveform_tile(p: &Painter, r: Rect, seed: u32, clock: Option<f64>, t: &theme::Tokens) {
    p.rect_filled(r, 0, theme::mix(t.sunken, t.audio, if t.dark { 0.35 } else { 0.55 }));
    let bars = (r.width() / 3.0) as usize;
    let mid = r.center().y;
    let play = clock.map(|c| ((c * 0.3) % 1.0) as f32);
    for k in 0..bars {
        let x = r.left() + 2.0 + k as f32 * 3.0;
        let h = waveform_sample(seed, k as f32 / bars as f32) * r.height() * 0.36;
        let played = play.is_some_and(|pl| (k as f32 / bars as f32) < pl);
        let c = if played { Color32::WHITE } else { Color32::from_white_alpha(170) };
        p.line_segment([egui::pos2(x, mid - h), egui::pos2(x, mid + h)], egui::Stroke::new(1.6, c));
    }
    if let Some(pl) = play {
        let x = egui::lerp(r.left()..=r.right(), pl);
        p.vline(x, r.y_range(), egui::Stroke::new(1.0, Color32::WHITE));
    }
}

/// Deterministic pseudo-waveform amplitude in 0..1 at `u` (0..1).
pub fn waveform_sample(seed: u32, u: f32) -> f32 {
    let s = seed as f32 * 0.37;
    let env = 0.55 + 0.45 * ((u * 3.1 + s).sin() * 0.5 + 0.5);
    let fine = ((u * 97.0 + s * 3.0).sin() * (u * 41.0 + s).cos()).abs();
    let beat = (((u * 16.0 + s).fract() - 0.1).abs() < 0.12) as u8 as f32 * 0.25;
    (env * (0.25 + 0.65 * fine) + beat).min(1.0)
}


fn asset_shape(p: &Painter, r: Rect, i: usize, time: f32, t: &theme::Tokens) {
    let palette = [Color32::from_rgb(0xFF, 0xC8, 0x57), Color32::from_rgb(0x7F, 0xD1, 0xB9), Color32::from_rgb(0xF2, 0x6B, 0x5B), t.accent, Color32::from_rgb(0x6F, 0xA8, 0xFF)];
    let col = palette[i % palette.len()];
    let c = r.center();
    let rad = r.width().min(r.height()) / 2.0;
    let stroke = egui::Stroke::new(rad * 0.12, col);
    match view::ASSETS[i] {
        "Circle" => {
            p.circle_filled(c, rad * 0.85, col);
        }
        "Rounded box" => {
            p.rect_filled(r.shrink(rad * 0.15), CornerRadius::same((rad * 0.3) as u8), col);
        }
        "Arrow" => {
            p.arrow(c - Vec2::new(rad * 0.8, -rad * 0.4), Vec2::new(rad * 1.6, -rad * 0.8), stroke);
        }
        "Star" | "Spark" | "Burst" => {
            let pts_n = if view::ASSETS[i] == "Burst" { 12 } else if view::ASSETS[i] == "Spark" { 4 } else { 5 };
            let inner = if pts_n == 4 { 0.25 } else { 0.45 };
            let mut pts = Vec::new();
            for k in 0..pts_n * 2 {
                let a = time * 0.8 - std::f32::consts::FRAC_PI_2 + k as f32 * std::f32::consts::PI / pts_n as f32;
                let rr = if k % 2 == 0 { rad } else { rad * inner };
                pts.push(c + Vec2::angled(a) * rr);
            }
            // Fan from the centre: star shapes are not convex.
            for k in 0..pts.len() {
                p.add(egui::Shape::convex_polygon(vec![c, pts[k], pts[(k + 1) % pts.len()]], col, egui::Stroke::NONE));
            }
        }
        "Speech bubble" => {
            let b = Rect::from_center_size(c - Vec2::new(0.0, rad * 0.15), Vec2::new(rad * 1.8, rad * 1.2));
            p.rect_filled(b, CornerRadius::same((rad * 0.35) as u8), col);
            p.add(egui::Shape::convex_polygon(vec![b.left_bottom() + Vec2::new(rad * 0.4, -1.0), b.left_bottom() + Vec2::new(rad * 0.9, -1.0), b.left_bottom() + Vec2::new(rad * 0.3, rad * 0.45)], col, egui::Stroke::NONE));
        }
        "Heart" => {
            let s = rad * 0.5;
            p.circle_filled(c + Vec2::new(-s * 0.55, -s * 0.3), s * 0.65, col);
            p.circle_filled(c + Vec2::new(s * 0.55, -s * 0.3), s * 0.65, col);
            p.add(egui::Shape::convex_polygon(vec![c + Vec2::new(-s * 1.15, -s * 0.05), c + Vec2::new(s * 1.15, -s * 0.05), c + Vec2::new(0.0, s * 1.35)], col, egui::Stroke::NONE));
        }
        "Underline" => {
            let pts: Vec<Pos2> = (0..=24).map(|k| {
                let u = k as f32 / 24.0;
                c + Vec2::new((u - 0.5) * rad * 1.8, (u * 9.0 + time * 4.0).sin() * rad * 0.12)
            }).collect();
            p.add(egui::Shape::line(pts, stroke));
        }
        "Badge" => {
            p.circle_filled(c, rad * 0.8, col);
            p.circle_stroke(c, rad * 0.62, egui::Stroke::new(rad * 0.05, Color32::WHITE));
            icons::paint(p, Rect::from_center_size(c, Vec2::splat(rad * 0.7)), Icon::Star, rad * 0.7, Color32::WHITE);
        }
        "Ring" => {
            p.circle_stroke(c, rad * 0.75, stroke);
        }
        _ => {
            // Tag.
            let b = Rect::from_center_size(c, Vec2::new(rad * 1.7, rad * 0.9));
            p.rect_filled(b, CornerRadius::same((rad * 0.2) as u8), col);
            p.circle_filled(b.left_center() + Vec2::new(rad * 0.28, 0.0), rad * 0.1, t.sunken);
        }
    }
}

/// The shared AI runner (one per app).
pub fn ai_runner() -> &'static std::sync::Mutex<crate::services::ai_run::AiRunner> {
    static R: std::sync::OnceLock<std::sync::Mutex<crate::services::ai_run::AiRunner>> = std::sync::OnceLock::new();
    R.get_or_init(|| std::sync::Mutex::new(crate::services::ai_run::AiRunner::default()))
}

/// The `studio-ai` feature behind an AI panel tile.
fn ai_feature(name: &str) -> Option<studio_ai::features::Feature> {
    use studio_ai::features::Feature as F;
    Some(match name {
        "Auto captions" => F::Captions,
        "Remove background" => F::RemoveBackground,
        "Voice cleanup" => F::VoiceCleanup,
        "Trim silences" => F::SilenceRemoval,
        "Smart reframe" => F::AutoReframe,
        "Scene cuts" => F::SceneCuts,
        "Beat markers" => F::BeatSync,
        "Smooth slow motion" => F::SmoothSlowMotion,
        _ => return None,
    })
}

fn ai_list(app: &mut StudioApp, ui: &mut egui::Ui, items: &[usize]) {
    use crate::services::ai_run::Action;
    // Pictures from words (Codex).
    super::generate::card(app, ui);
    ui.add_space(6.0);
    let t = theme::tokens(ui.ctx());
    let sel_clip = app.editor.selection.first().copied();
    for &i in items {
        let tool = &view::AI_TOOLS[i];
        let action = Action::for_tool(tool.name);
        // Remove background is the Inspector's cut-out, on the selected (or
        // first) video clip.
        let cut = cfg!(feature = "render") && matches!(tool.name, "Remove background" | "Smooth slow motion");
        let row = ai_feature(tool.name).map(crate::services::models::feature);
        let ready = row.as_ref().is_some_and(|r| r.ready);
        let w = ui.available_width();
        let (r, _) = ui.allocate_exact_size(Vec2::new(w, 76.0), Sense::hover());
        let p = ui.painter();
        p.rect_filled(r, CornerRadius::same(theme::radius::CARD), t.raised);
        let icon_r = Rect::from_min_size(r.min + Vec2::new(12.0, 14.0), Vec2::splat(32.0));
        p.rect_filled(icon_r, CornerRadius::same(8), t.accent_soft);
        icons::paint(p, icon_r, Icon::Sparkle, 16.0, t.accent);
        let tx = icon_r.right() + 12.0;
        let btn_w = 92.0;
        kit::paint_text_clipped(p, egui::pos2(tx, r.top() + 22.0), Align2::LEFT_CENTER, tool.name, theme::control_medium(), t.text, r.right() - tx - btn_w - 16.0);
        let running = action.is_some_and(|a| ai_runner().lock().unwrap_or_else(|e| e.into_inner()).is_running(a));
        #[cfg(feature = "render")]
        let running = running || (cut && (crate::editor::cutout::busy() || crate::editor::slowmo::busy()));
        let note = match (&row, action) {
            (Some(r), None) if cut && !r.ready => format!("{} {}", tool.blurb, r.note),
            (_, None) if cut => format!("{} {}", tool.blurb, if tool.name == "Remove background" { "People stay, the rest goes; Inspector ▸ Cut out picks objects." } else { "On the selected slowed clip (speed under 1, or a ramp)." }),
            (_, None) => format!("{} Coming to this panel soon.", tool.blurb),
            (Some(r), Some(_)) if !r.ready => format!("{} {}", tool.blurb, r.note),
            _ => tool.blurb.to_owned(),
        };
        let galley = p.layout(note, theme::body(), t.text_2, r.right() - tx - btn_w - 16.0);
        p.galley(egui::pos2(tx, r.top() + 34.0), galley, t.text_2);
        let br = Rect::from_min_size(egui::pos2(r.right() - btn_w - 12.0, r.center().y - 14.0), Vec2::new(btn_w, 28.0));
        let mut bui = ui.new_child(egui::UiBuilder::new().max_rect(br));
        kit::on_card(&mut bui);
        let enabled = (action.is_some() || cut) && ready && !running;
        bui.add_enabled_ui(enabled, |bui| {
            let label = if running { "Running" } else if action.is_none() && !cut { "Soon" } else if ready { "Run" } else { "Needs model" };
            let b = Button::secondary(label).icon(Icon::PlayFill).busy(running).min_width(btn_w).show(bui);
            let tip = if !ready && (action.is_some() || cut) { "Install the model in Settings ▸ Models".to_owned() } else if cut { crate::i18n::tf(format!("Run on the {}", if sel_clip.is_some() { "selected clip" } else { "first video clip" })) } else { crate::i18n::tf(format!("Run on the {}", if sel_clip.is_some() { "selected clip" } else { "first clip with sound" })) };
            let b = kit::tip(b, &tip, None);
            #[cfg(feature = "render")]
            if b.clicked() && cut {
                let target = sel_clip.filter(|id| app.doc.project.clip(*id).is_some_and(|c| matches!(c.kind, view::ClipKind::Video))).or_else(|| app.doc.project.clips.iter().filter(|c| matches!(c.kind, view::ClipKind::Video)).min_by_key(|c| (c.start, c.lane)).map(|c| c.id));
                let ctx = bui.ctx().clone();
                match target.ok_or_else(|| "there is no video clip to work on".to_owned()).and_then(|id| if tool.name == "Remove background" { crate::editor::cutout::start_people(app, &ctx, id) } else { crate::editor::slowmo::start(app, &ctx, id, crate::services::slowmo::Method::Flow) }) {
                    Ok(msg) => {
                        kit::push(&ctx, msg, None, Some(Icon::Sparkle));
                    }
                    Err(e) => {
                        kit::push(&ctx, e, None, Some(Icon::Warning));
                    }
                }
            }
            if b.clicked() && !cut {
                if let Some(a) = action {
                    let sel = app.editor.selection.clone();
                    let res = ai_runner().lock().unwrap_or_else(|e| e.into_inner()).start(&app.doc, a, &sel);
                    match res {
                        Ok(msg) => {
                            kit::push(bui.ctx(), msg, None, Some(Icon::Sparkle));
                        }
                        Err(e) => {
                            kit::push(bui.ctx(), e.0, None, Some(Icon::Warning));
                        }
                    }
                }
            }
        });
        ui.add_space(4.0);
    }
    // Installed and available plugins (and OpenFX).
    let chip = app.editor.library_chip[RailItem::Ai.index()];
    let q = app.editor.library_query.to_lowercase();
    super::library_plugins::show(app, ui, chip, &q);
}

/// Apply finished AI jobs (called every frame from the editor).
pub fn poll_ai(app: &mut StudioApp, ctx: &egui::Context) {
    let results = ai_runner().lock().unwrap_or_else(|e| e.into_inner()).poll(&mut app.doc);
    let busy = !ai_runner().lock().unwrap_or_else(|e| e.into_inner()).jobs().is_empty();
    for r in results {
        match r {
            Ok(o) => {
                if !o.select.is_empty() {
                    app.editor.selection = o.select.clone();
                }
                kit::push(ctx, o.message, None, Some(Icon::Sparkle));
            }
            Err(e) => {
                kit::push(ctx, e.0, None, Some(Icon::Warning));
            }
        }
    }
    if busy {
        ctx.request_repaint_after(std::time::Duration::from_millis(200));
    }
}

// --- applying items ------------------------------------------------------

impl StudioApp {
    /// 0..1 while a just-imported media item is still "importing".
    pub fn editor_import_progress(&self, media: u32, now: f64) -> Option<f32> {
        let started = self.imports.iter().find(|(id, _)| *id == media)?.1;
        let k = ((now - started) / 1.2) as f32;
        (k < 1.0).then_some(k.max(0.02))
    }
}

/// Add the generated sample footage to the bin (File → Add sample media).
pub fn import_sample_media(app: &mut StudioApp, ctx: &egui::Context) {
    // Until a file picker and the media probe are wired up, importing adds
    // the sample footage so the flow can be tried end to end.
    let now = ctx.input(|i| i.time);
    let existing: Vec<String> = app.doc.project.media.iter().map(|m| m.name.clone()).collect();
    let mut new: Vec<view::MediaItem> = view::demo_media().into_iter().filter(|m| !existing.contains(&m.name)).collect();
    for mi in &mut new {
        mi.id = app.doc.alloc_id() as u32;
    }
    if new.is_empty() {
        kit::push(ctx, "All sample media is already in this project", None, Some(Icon::Info));
        return;
    }
    let n = new.len();
    for m in &new {
        app.imports.push((m.id, now));
    }
    app.doc.edit("Import media", |p| p.media.extend(new));
    kit::push(ctx, crate::i18n::tf(format!("Imported {n} items")), None, Some(Icon::DownloadSimple));
}

/// Apply a library item to the selection (click) or to `target`.
fn apply(app: &mut StudioApp, ctx: &egui::Context, rail: RailItem, i: usize, target: Option<u32>) {
    if !matches!(rail, RailItem::Effects | RailItem::Looks | RailItem::Motions | RailItem::Cuts) {
        add(app, ctx, rail, i, None, None);
        return;
    }
    let targets: Vec<u32> = match target {
        Some(id) => vec![id],
        None => app.editor.selection.clone(),
    };
    let panel = crate::services::library::Panel::ALL[rail.index()];
    match crate::services::library::apply(&mut app.doc, panel, i, &targets) {
        Ok(out) => app.editor.undo_toast = Some(kit::push(ctx, out.message, Some("Undo"), Some(Icon::Check))),
        Err(r) => {
            kit::push(ctx, r.0, None, Some(Icon::Info));
        }
    }
}

/// Add a library item to the timeline at `at` (or the playhead) on `lane`
/// (or the natural lane for its kind).
/// How long a media item is when it goes on the timeline: stills 4 s,
/// video and sound their whole length.
pub fn media_clip_len(kind: MediaKind, dur: Time) -> Time {
    if kind == MediaKind::Image { 4 * SEC } else { dur.max(SEC / 10) }
}

pub fn add(app: &mut StudioApp, ctx: &egui::Context, rail: RailItem, i: usize, at: Option<Time>, lane: Option<u32>) {
    if matches!(rail, RailItem::Assets | RailItem::Audio) && crate::content::get().is_some() {
        super::library_content::add_content(app, ctx, i, at, lane);
        return;
    }
    let t = at.unwrap_or(app.editor.playhead);
    let p = &app.doc.project;
    let lane_of = |kind: LaneKind, prefer_last: bool| {
        let mut ls = p.lanes.iter().filter(|l| l.kind == kind);
        if prefer_last { ls.last().map(|l| l.id) } else { ls.next().map(|l| l.id) }
    };
    let mut clip = match rail {
        RailItem::Media => {
            let mi = p.media[i].clone();
            let kind = if mi.kind == MediaKind::Audio { ClipKind::Audio } else { ClipKind::Video };
            let lane = lane.or_else(|| if kind == ClipKind::Audio { lane_of(LaneKind::Audio, false) } else { p.spine() });
            new_clip(lane, t, media_clip_len(mi.kind, mi.dur), kind, mi.name.rsplit_once('.').map_or(mi.name.as_str(), |(a, _)| a), Some(mi.id))
        }
        RailItem::Text => {
            let preset = &studio_text::styles::catalogue()[i];
            let mut c = new_clip(lane.or_else(|| lane_of(LaneKind::Visual, false)), t, 4 * SEC, ClipKind::Title, &preset.name, None);
            c.text = if preset.sample.chars().count() <= 24 { preset.sample.clone() } else { "Your title".into() };
            c.text_style = Some(preset.id.clone());
            c.extra.text_size = preset.style.size.clamp(5.0, 300.0);
            c
        }
        RailItem::Assets => {
            let mut c = new_clip(lane.or_else(|| lane_of(LaneKind::Visual, true)), t, 3 * SEC, ClipKind::Title, view::ASSETS[i], None);
            c.transform.scale = 30.0;
            c
        }
        RailItem::Audio => {
            let d = (view::SOUNDS[i].1 as f64 * SEC as f64) as Time;
            // Sounds are generated; until the content library ships audio,
            // they use the sample music item if the project has one.
            let media = p.media.iter().find(|m| m.kind == MediaKind::Audio).map(|m| m.id);
            let Some(media) = media else {
                kit::push(ctx, "Import an audio file first", None, Some(Icon::Info));
                return;
            };
            new_clip(lane.or_else(|| lane_of(LaneKind::Audio, true)), t, d.min(30 * SEC), ClipKind::Audio, view::SOUNDS[i].0, Some(media))
        }
        RailItem::Effects | RailItem::Looks => {
            let item = if rail == RailItem::Effects { catalogue::effects()[i] } else { catalogue::looks()[i] };
            let mut c = new_clip(lane.or_else(|| lane_of(LaneKind::Visual, true)), t, 4 * SEC, ClipKind::Fx, item.name, None);
            if rail == RailItem::Effects {
                c.effects = vec![view::Effect::new(item.id)];
            } else {
                c.look = Some(item.id.into());
            }
            c
        }
        RailItem::Story => return,
        RailItem::Motions | RailItem::Cuts | RailItem::Ai => {
            apply(app, ctx, rail, i, None);
            return;
        }
    };
    let Some(lane) = clip.lane_opt() else { return };
    clip.lane = lane;
    let name = clip.name.clone();
    let spine = p.spine() == Some(lane);
    // On the spine, insert at the nearest cut; later clips move along.
    let cut = p.clips.iter().filter(|c| c.lane == lane).flat_map(|c| [c.start, c.end()]).min_by_key(|x| (x - t).abs()).unwrap_or(0);
    let new_id = app.doc.alloc_id() as u32;
    clip.id = new_id;
    let ok = if spine {
        clip.start = cut;
        let mc = crate::bridge::new_clip(&app.doc.model, &clip);
        let seq = app.doc.seq();
        app.doc.apply("Add clip", m::Edit::Insert { seq, lane: Some(m::LaneId(lane as u64)), at: cut, clip: Box::new(mc) })
    } else {
        app.doc.edit("Add clip", |p| {
            p.clips.push(clip);
            // Free lanes do not overlap: find room, adding a lane if needed.
            p.resolve_overlap(new_id);
        })
    };
    if ok {
        app.editor.selection = vec![new_id];
        app.editor.undo_toast = Some(kit::push(ctx, crate::i18n::tf(format!("Added {name}")), Some("Undo"), Some(Icon::Plus)));
    }
}

/// An empty clip of `kind` on `lane`.
pub fn blank_clip(lane: u32, start: Time, dur: Time, kind: ClipKind, name: &str, media: Option<u32>) -> view::Clip {
    new_clip(Some(lane), start, dur, kind, name, media)
}

fn new_clip(lane: Option<u32>, start: Time, dur: Time, kind: ClipKind, name: &str, media: Option<u32>) -> view::Clip {
    let mut c = view::Clip {
        id: 0,
        lane: lane.unwrap_or(u32::MAX),
        start,
        dur,
        src_in: 0,
        kind,
        name: name.into(),
        media,
        text: String::new(),
        text_colour: Color32::WHITE,
        transform: Default::default(),
        grade: Default::default(),
        audio: Default::default(),
        speed: 100.0,
        effects: vec![],
        motion_in: None,
        look: None,
        keys: vec![],
        text_style: None,
        extra: Default::default(),
    };
    if kind == ClipKind::Title {
        c.transform.y = 0.0;
    }
    c
}

trait LaneOpt {
    fn lane_opt(&self) -> Option<u32>;
}
impl LaneOpt for view::Clip {
    fn lane_opt(&self) -> Option<u32> {
        (self.lane != u32::MAX).then_some(self.lane)
    }
}

/// Fill used for clips of this rail's items (for drag ghosts on the
/// timeline).
pub fn drag_kind(rail: RailItem, media: &[view::MediaItem], i: usize) -> ClipKind {
    if matches!(rail, RailItem::Assets | RailItem::Audio) && crate::content::get().is_some() {
        return super::library_content::drag_kind(i);
    }
    match rail {
        RailItem::Media => if media.get(i).is_some_and(|m| m.kind == MediaKind::Audio) { ClipKind::Audio } else { ClipKind::Video },
        RailItem::Audio => ClipKind::Audio,
        RailItem::Text | RailItem::Assets => ClipKind::Title,
        _ => ClipKind::Fx,
    }
}

pub fn drag_fill(t: &theme::Tokens, rail: RailItem, media: &[view::MediaItem], i: usize) -> Color32 {
    clip_fill(t, drag_kind(rail, media, i))
}

pub fn drag_dur(rail: RailItem, media: &[view::MediaItem], i: usize) -> Time {
    if matches!(rail, RailItem::Assets | RailItem::Audio) && crate::content::get().is_some() {
        return super::library_content::drag_dur(i);
    }
    match rail {
        RailItem::Media => media.get(i).map_or(4 * SEC, |m| media_clip_len(m.kind, m.dur)),
        RailItem::Audio => ((view::SOUNDS[i].1 as f64 * SEC as f64) as Time).min(30 * SEC),
        RailItem::Assets => 3 * SEC,
        _ => 4 * SEC,
    }
}

/// An adjustment clip carrying an effect or look, over `start..start+dur`.
pub fn add_adjustment(app: &mut StudioApp, ctx: &egui::Context, d: LibDrag, start: Time, dur: Time, lane: Option<u32>) {
    let item = if d.rail == RailItem::Effects { catalogue::effects()[d.index] } else { catalogue::looks()[d.index] };
    let lane = lane.or_else(|| app.doc.project.lanes.iter().find(|l| l.kind == LaneKind::Visual).map(|l| l.id));
    let mut c = new_clip(lane, start, dur, ClipKind::Fx, item.name, None);
    if d.rail == RailItem::Effects {
        c.effects = vec![view::Effect::new(item.id)];
    } else {
        c.look = Some(item.id.into());
    }
    let Some(lane) = c.lane_opt() else { return };
    c.lane = lane;
    let id = app.doc.alloc_id() as u32;
    c.id = id;
    if app.doc.edit("Add adjustment", |p| {
        p.clips.push(c);
        p.resolve_overlap(id);
    }) {
        app.editor.selection = vec![id];
        app.editor.undo_toast = Some(kit::push(ctx, format!("{} added as an adjustment clip", item.name), Some("Undo"), Some(Icon::MagicWand)));
    }
}

pub fn apply_to(app: &mut StudioApp, ctx: &egui::Context, d: LibDrag, clip: u32) {
    apply(app, ctx, d.rail, d.index, Some(clip));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_media_goes_in_whole() {
        // A 1:30 video and a 3-minute song keep their full length.
        assert_eq!(media_clip_len(MediaKind::Video, 90 * SEC), 90 * SEC);
        assert_eq!(media_clip_len(MediaKind::Audio, 180 * SEC), 180 * SEC);
        assert_eq!(media_clip_len(MediaKind::Image, 0), 4 * SEC);
    }
}
