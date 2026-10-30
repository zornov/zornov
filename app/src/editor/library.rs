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
    /// The name shown on the tab (the glossary's); `name` stays the key
    /// that saved workspaces use.
    pub fn label(self) -> &'static str {
        match self {
            RailItem::Motions => "Animations",
            RailItem::Cuts => "Transitions",
            RailItem::Ai => "Tools",
            other => other.name(),
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
            RailItem::Ai => &["All tools", "Installed", "Available"],
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

/// Width of the overflow cell.
const MORE_W: f32 = 36.0;

/// How many of the tabs fit across `avail` (all of them when they do, else
/// as many as leave room for the overflow cell).
fn tabs_that_fit(widths: &[f32], avail: f32) -> usize {
    let all: f32 = 6.0 + widths.iter().map(|w| w + 2.0).sum::<f32>();
    if all <= avail {
        return widths.len();
    }
    let mut x = 6.0 + MORE_W + 2.0;
    let mut n = 0;
    for w in widths {
        x += w + 2.0;
        if x > avail {
            break;
        }
        n += 1;
    }
    n.max(1)
}

/// The section tabs: a row of icon-over-label tabs, the active one in the
/// accent. Number keys 1-9 and 0 switch sections; clicking the active one
/// collapses the library. The tabs that do not fit are in a "…" menu, so
/// none is ever out of reach.
pub fn rail(app: &mut StudioApp, ui: &mut egui::Ui, r: Rect) {
    let t = theme::tokens(ui.ctx());
    let widths: Vec<f32> = RailItem::ALL.iter().map(|item| (ui.fonts_mut(|f| f.layout_no_wrap(item.label().to_string(), theme::caption(), t.text).size().x) + 14.0).max(40.0)).collect();
    let fit = tabs_that_fit(&widths, r.width());
    rail_cells(app, ui, r, &widths, fit);
    ui.painter().hline(r.x_range(), r.bottom() - 0.5, Stroke::new(1.0, t.line));
}

fn rail_cells(app: &mut StudioApp, ui: &mut egui::Ui, r: Rect, widths: &[f32], fit: usize) {
    let t = theme::tokens(ui.ctx());
    let mut open_tool: Option<RailItem> = None;
    let mut x = r.left() + 6.0;
    for (item, w) in RailItem::ALL.into_iter().zip(widths.iter().copied()) {
        // Keys reach every section, shown as a tab or not.
        let key_hit = !ui.ctx().egui_wants_keyboard_input()
            && ui.input_mut(|i| i.consume_shortcut(&egui::KeyboardShortcut::new(egui::Modifiers::NONE, [egui::Key::Num1, egui::Key::Num2, egui::Key::Num3, egui::Key::Num4, egui::Key::Num5, egui::Key::Num6, egui::Key::Num7, egui::Key::Num8, egui::Key::Num9, egui::Key::Num0][item.index()])));
        if key_hit {
            open_tool = Some(item);
        }
        if item.index() >= fit {
            continue;
        }
        let name = item.label();
        let cell = Rect::from_min_size(egui::pos2(x, r.top() + 4.0), Vec2::new(w, r.height() - 8.0));
        let resp = ui.interact(cell, egui::Id::unique(("rail", item.index())), Sense::click());
        let st = kit::State::from_response(ui, &resp, None);
        let active = app.editor.rail == item && app.editor.dock.is_open(crate::dock::Panel::Library);
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
        if resp.clicked() {
            open_tool = Some(item);
        }
        x += w + 2.0;
    }
    if fit < RailItem::ALL.len() {
        let cell = Rect::from_min_size(egui::pos2(x, r.top() + 4.0), Vec2::new(MORE_W, r.height() - 8.0));
        let resp = ui.interact(cell, egui::Id::new("rail-more"), Sense::click());
        let st = kit::State::from_response(ui, &resp, None);
        let hidden_active = app.editor.rail.index() >= fit && app.editor.dock.is_open(crate::dock::Panel::Library);
        let fg = if hidden_active { t.accent } else { theme::mix(t.text, t.text, st.hover) };
        icons::paint(ui.painter(), Rect::from_center_size(cell.center(), Vec2::splat(16.0)), Icon::DotsThree, 16.0, fg);
        if st.focused {
            kit::focus_ring(ui.painter(), cell, CornerRadius::same(theme::radius::CONTROL), &t);
        }
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "More sections"));
        let resp = kit::tip(resp, "More sections", None);
        let id = egui::Id::new("rail-more-menu");
        if resp.clicked() {
            kit::toggle_open(ui.ctx(), id);
        }
        kit::popover(ui.ctx(), id, cell, kit::Side::BelowRight, 180.0, |ui| {
            for item in RailItem::ALL.into_iter().filter(|i| i.index() >= fit) {
                if kit::MenuItem::new(item.label()).icon(item.icon()).shortcut(item.shortcut()).checked(app.editor.rail == item).show(ui).clicked() {
                    open_tool = Some(item);
                    kit::set_open(ui.ctx(), id, false);
                }
            }
        });
    }
    if let Some(item) = open_tool {
        app.editor.rail = item;
        app.editor.open_panel(crate::dock::Panel::Library);
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
    // The tools keep their old chip numbers (1 All tools, 2 Installed, 3
    // Available; 0 was "AI editor", now only the assistant panel), so the
    // saved chip and the assistant's own jump to "All tools" still agree.
    let off = usize::from(rail == RailItem::Ai);
    let mut chip = app.editor.library_chip[rail.index()].saturating_sub(off).min(rail.groups().len().saturating_sub(1));
    categories(ui, col, rail.groups(), &mut chip);
    ui.painter().vline(col.right() - 0.5, col.y_range(), Stroke::new(1.0, t.line));
    if rail == RailItem::Story {
        // The production plan: storyboard, script, places, cast, check.
        app.editor.library_chip[rail.index()] = chip + off;
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
    let hint = crate::i18n::tf(format!("Search {}", rail.label().to_lowercase()));
    kit::text_field(&mut sui, &mut app.editor.library_query, if rail == RailItem::Ai { "Search tools" } else { &hint }, Some(Icon::MagnifyingGlass), search_w, None);
    let chips_r = Rect::from_min_max(egui::pos2(inner.left(), search.bottom()), egui::pos2(inner.right(), search.bottom()));
    let r = Rect::from_min_max(egui::pos2(inner.left() - pad, r.top()), r.max);
    app.editor.library_chip[rail.index()] = chip + off;

    let body = Rect::from_min_max(egui::pos2(r.left(), chips_r.bottom() + 12.0), r.max - Vec2::new(0.0, 4.0));
    let mut bui = ui.new_child(egui::UiBuilder::new().max_rect(body));
    bui.set_clip_rect(body.intersect(ui.clip_rect()));
    let query = app.editor.library_query.to_lowercase();

    if matches!(rail, RailItem::Assets | RailItem::Audio) && super::library_content::panel(app, ui, rail, chip, &query, body) {
        return;
    }
    let sort = ui.ctx().data(|d| d.get_temp::<crate::services::library::Sort>(egui::Id::new("media-sort"))).unwrap_or_default();
    let items = items_for(app, rail, chip, &query, sort);
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
                if rail == RailItem::Media {
                    offline_banner(app, ui);
                    sort_row(app, ui, sort);
                }
                grid(app, ui, rail, &items);
            }
        });
    });
}

/// "Sort: Date added" with the other orders in a menu.
fn sort_row(app: &StudioApp, ui: &mut egui::Ui, sort: crate::services::library::Sort) {
    if app.doc.project.media.len() < 2 {
        return;
    }
    let mut pick = None;
    kit::menu_group_button(ui, "media-sort", &crate::i18n::tf(format!("Sort: {}", sort.label())), Some(Icon::List), |ui| {
        for s in crate::services::library::Sort::ALL {
            if kit::MenuItem::new(s.label()).checked(s == sort).show(ui).clicked() {
                pick = Some(s);
            }
        }
    });
    if let Some(s) = pick {
        ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("media-sort"), s));
    }
    ui.add_space(6.0);
}

/// "N files are offline", with the way to find them again, above the bin.
fn offline_banner(app: &mut StudioApp, ui: &mut egui::Ui) {
    let n = crate::services::library::offline_ids(&app.doc.project.media).len();
    if n == 0 {
        return;
    }
    ui.horizontal(|ui| {
        let t = theme::tokens(ui.ctx());
        icons::paint(ui.painter(), Rect::from_min_size(ui.cursor().min + Vec2::new(0.0, 2.0), Vec2::splat(16.0)), Icon::Warning, 14.0, t.danger);
        ui.add_space(20.0);
        ui.label(egui::RichText::new(crate::i18n::tf(format!("{n} file{} offline", if n == 1 { " is" } else { "s are" }))).font(theme::body()).color(t.text));
        if Button::secondary("Find in folder").icon(Icon::FolderOpen).compact().show(ui).clicked() {
            relink_from_folder(app, ui.ctx());
        }
    });
    ui.add_space(6.0);
}

/// Pick a folder and point every offline file found there (by name) back at it.
fn relink_from_folder(app: &mut StudioApp, ctx: &egui::Context) {
    let Some(dir) = rfd::FileDialog::new().set_title("Find offline files in").pick_folder() else { return };
    let res = crate::services::library::relink_in(&mut app.doc, &[dir]);
    kit::push(ctx, res.message(), None, Some(if res.done.is_empty() { Icon::Info } else { Icon::Check }));
}

/// Open the media menu for `media` where the pointer is now. The place is
/// kept, so the menu stays put while the pointer moves.
fn open_media_menu(ctx: &egui::Context, media: u32) {
    let at = ctx.input(|i| i.pointer.latest_pos()).unwrap_or_default();
    ctx.data_mut(|d| {
        d.insert_temp(egui::Id::new("media-menu-for"), media);
        d.insert_temp(egui::Id::new("media-menu-at"), at);
        d.remove::<(String, String)>(egui::Id::new("media-menu-fields"));
    });
    kit::set_open(ctx, egui::Id::new("media-menu"), true);
}

/// Where the media menu opened.
fn media_menu_anchor(ctx: &egui::Context) -> egui::Pos2 {
    ctx.data(|d| d.get_temp::<egui::Pos2>(egui::Id::new("media-menu-at"))).unwrap_or_default()
}

/// The Name and Folder fields of the media menu, each under a caption (the
/// placeholder hint is gone once a field holds text). Returns whether Enter
/// was pressed in the name / the folder.
fn media_menu_fields(ui: &mut egui::Ui, name: &mut String, folder: &mut String) -> (bool, bool) {
    let t = theme::tokens(ui.ctx());
    let caption = |ui: &mut egui::Ui, text: &str| {
        ui.add_space(2.0);
        ui.label(egui::RichText::new(crate::i18n::tf(text.to_owned())).font(theme::caption()).color(t.text_2));
    };
    caption(ui, "Name");
    let renamed = kit::text_field(ui, name, "Name", Some(Icon::FilmStrip), 200.0, None).lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
    caption(ui, "Folder");
    let filed = kit::text_field(ui, folder, "No folder", Some(Icon::Folder), 200.0, None).lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
    (renamed, filed)
}

/// Right-click on a bin item: add it, or (when its file is gone) relink it.
fn media_menu(app: &mut StudioApp, ctx: &egui::Context, offline: &std::collections::HashSet<u32>) {
    let id = egui::Id::new("media-menu");
    if !kit::is_open(ctx, id) {
        return;
    }
    let Some(media) = ctx.data(|d| d.get_temp::<u32>(egui::Id::new("media-menu-for"))) else { return };
    let Some(index) = app.doc.project.media.iter().position(|x| x.id == media) else {
        kit::set_open(ctx, id, false);
        return;
    };
    let at = media_menu_anchor(ctx);
    let (mut add_it, mut relink_it, mut proxy_it, mut cancel_it) = (false, false, false, false);
    let is_video = app.doc.project.media[index].kind == MediaKind::Video;
    let making = crate::services::proxies::progress_of(media).is_some();
    let (mut name, mut folder) = ctx.data(|d| d.get_temp::<(String, String)>(egui::Id::new("media-menu-fields"))).unwrap_or_else(|| (app.doc.project.media[index].name.clone(), crate::services::library::folder_of(&app.doc, media).unwrap_or_default()));
    let (mut renamed, mut filed) = (false, false);
    kit::popover(ctx, id, Rect::from_min_size(at, Vec2::ZERO), kit::Side::Below, 220.0, |ui| {
        add_it = kit::MenuItem::new("Add to timeline").show(ui).clicked();
        if offline.contains(&media) {
            relink_it = kit::MenuItem::new("Relink file").show(ui).clicked();
        } else if is_video && making {
            cancel_it = kit::MenuItem::new("Cancel proxy").show(ui).clicked();
        } else if is_video {
            proxy_it = kit::MenuItem::new("Make proxy").show(ui).clicked();
        }
        kit::menu_separator(ui);
        (renamed, filed) = media_menu_fields(ui, &mut name, &mut folder);
    });
    ctx.data_mut(|d| d.insert_temp(egui::Id::new("media-menu-fields"), (name.clone(), folder.clone())));
    if renamed || filed {
        kit::set_open(ctx, id, false);
        ctx.data_mut(|d| d.remove::<(String, String)>(egui::Id::new("media-menu-fields")));
        let res = if renamed { crate::services::library::rename(&mut app.doc, media, &name) } else { crate::services::library::set_folder(&mut app.doc, media, &folder) };
        match res {
            Ok(msg) => {
                kit::push(ctx, msg, None, Some(Icon::Check));
            }
            Err(e) => {
                kit::push(ctx, e.0, None, Some(Icon::Warning));
            }
        }
    }
    if proxy_it {
        kit::set_open(ctx, id, false);
        crate::services::proxies::start(&app.doc, vec![m::MediaId(media as u64)], 2);
        kit::push(ctx, crate::i18n::tf(format!("Making a proxy of {}", app.doc.project.media[index].name)), None, Some(Icon::Info));
    }
    if cancel_it {
        kit::set_open(ctx, id, false);
        crate::services::proxies::cancel(media);
    }
    if add_it {
        kit::set_open(ctx, id, false);
        add(app, ctx, RailItem::Media, index, None, None);
    }
    if relink_it {
        kit::set_open(ctx, id, false);
        let name = app.doc.project.media[index].name.clone();
        if let Some(to) = rfd::FileDialog::new().set_title(format!("Locate {name}")).pick_file() {
            match crate::services::library::relink(&mut app.doc, media, &to) {
                Ok(msg) => {
                    kit::push(ctx, msg, None, Some(Icon::Check));
                }
                Err(e) => {
                    kit::push(ctx, e.0, None, Some(Icon::Warning));
                }
            }
        }
    }
}

/// Indices into the catalogue for this rail item, after chip and search
/// filters.
fn items_for(app: &StudioApp, rail: RailItem, chip: usize, q: &str, sort: crate::services::library::Sort) -> Vec<usize> {
    let names: Vec<String> = match rail {
        // A folder's name finds what is in it.
        RailItem::Media => app.doc.project.media.iter().map(|m| match crate::services::library::folder_of(&app.doc, m.id) { Some(f) => format!("{} {f}", m.name), None => m.name.clone() }).collect(),
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
    let mut found: Vec<usize> = names
        .iter()
        .enumerate()
        .filter(|(i, name)| {
            // Tools that cannot run yet are not listed.
            if rail == RailItem::Ai && !ai_runnable(name) {
                return false;
            }
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
                    RailItem::Ai => (chip == 1) == ai_feature(view::AI_TOOLS[*i].name).map(crate::services::models::feature).is_some_and(|r| r.ready),
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
        .collect();
    if rail == RailItem::Media {
        crate::services::library::sort_media(&app.doc, &mut found, sort);
    }
    found
}

fn grid(app: &mut StudioApp, ui: &mut egui::Ui, rail: RailItem, items: &[usize]) {
    let t = theme::tokens(ui.ctx());
    if rail == RailItem::Media {
        // Proxies made in the background land here, with their result.
        for (msg, bad) in crate::services::proxies::collect(&mut app.doc) {
            kit::push(ui.ctx(), msg, None, Some(if bad { Icon::Warning } else { Icon::Check }));
        }
        if crate::services::proxies::busy() {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(200));
        }
    }
    let names: Vec<String> = items.iter().map(|&i| item_name(app, rail, i)).collect();
    let offline = if rail == RailItem::Media { crate::services::library::offline_ids(&app.doc.project.media) } else { Default::default() };
    // A file that cannot be read says so (with the reason on hover) instead of a grey tile.
    let unreadable: Vec<Option<String>> = items.iter().map(|&i| if rail == RailItem::Media && app.doc.project.media[i].kind != MediaKind::Audio { app.doc.project.media[i].path.as_deref().and_then(thumbs::read_failure) } else { None }).collect();
    let badges: Vec<Option<String>> = items
        .iter()
        .enumerate()
        .map(|(k, &i)| {
            if rail == RailItem::Media && offline.contains(&app.doc.project.media[i].id) {
                Some("Offline".to_string())
            } else if unreadable[k].is_some() {
                Some("Can't read".to_string())
            } else {
                badge(app, rail, i)
            }
        })
        .collect();
    let selected = app.editor.library_selected;
    let now = ui.input(|i| i.time);
    let progress: Vec<Option<f32>> = items
        .iter()
        .map(|&i| {
            if rail != RailItem::Media {
                return None;
            }
            let id = app.doc.project.media[i].id;
            app.editor_import_progress(id, now).or_else(|| crate::services::proxies::progress_of(id))
        })
        .collect();
    let folders: Vec<Option<String>> = items.iter().map(|&i| if rail == RailItem::Media { crate::services::library::folder_of(&app.doc, app.doc.project.media[i].id) } else { None }).collect();
    let tips: Vec<Option<String>> = items
        .iter()
        .enumerate()
        .map(|(k, &i)| match rail {
            RailItem::Effects => Some(catalogue::effects()[i].description.to_string()),
            RailItem::Cuts => Some(catalogue::cuts()[i].description.to_string()),
            RailItem::Motions => Some(catalogue::motions()[i].description.to_string()),
            RailItem::Media if offline.contains(&app.doc.project.media[i].id) => Some(crate::i18n::tf(format!("{} is not at {}. Right-click to relink it.", app.doc.project.media[i].name, app.doc.project.media[i].path.as_ref().map_or_else(String::new, |p| p.display().to_string())))),
            RailItem::Media => unreadable[k].as_ref().map(|e| crate::i18n::tf(format!("Could not read this file: {e}"))),
            _ => None,
        })
        .collect();
    // Media already on the timeline gets the "added" badge.
    let used: std::collections::HashSet<u32> = app.doc.project.clips.iter().filter_map(|c| c.media).collect();
    let added: Vec<bool> = items.iter().map(|&i| rail == RailItem::Media && used.contains(&app.doc.project.media[i].id)).collect();
    let info = |k: usize| TileInfo {
        label: names[k].as_str(),
        badge: badges[k].as_deref(),
        sub: folders[k].as_deref(),
        selected: selected == Some((rail, items[k])),
        addable: rail != RailItem::Ai,
        progress: progress[k],
        tip: tips[k].as_deref(),
        added: added[k],
    };
    let media = app.doc.project.media.clone();
    let thumbs = &mut app.thumbs;
    let mut paint = |p: &Painter, r: Rect, k: usize, hover: f32, live: bool| {
        paint_thumb(thumbs, &media, rail, items[k], p, r, hover, live, &t);
        if rail == RailItem::Media && offline.contains(&media[items[k]].id) {
            // Gone from disk: the picture dims and says so.
            p.rect_filled(r, 0, t.sunken.gamma_multiply(0.75));
            icons::paint(p, Rect::from_center_size(r.center(), Vec2::splat(20.0)), Icon::Warning, 18.0, t.danger);
        }
    };
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
    if rail == RailItem::Media {
        if let Some(k) = out.secondary {
            open_media_menu(&ctx, app.doc.project.media[items[k]].id);
        }
        media_menu(app, &ctx, &offline);
    }
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
                MediaKind::Audio => Some(audio_badge(m.dur, m.path.as_deref().and_then(crate::services::library::wave).map(|w| w.secs))),
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
                let wave = m.path.as_deref().and_then(crate::services::library::wave);
                peaks_tile(p, r, wave.as_deref(), live.then_some(clock), hover, t);
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
        RailItem::Audio => {
            // Built-in sounds are names only: no waveform to show.
            p.rect_filled(r, 0, theme::mix(t.sunken, t.audio, if t.dark { 0.35 } else { 0.55 }));
            icons::paint(p, Rect::from_center_size(r.center(), Vec2::splat(22.0)), Icon::SpeakerHigh, 20.0, Color32::from_white_alpha(200));
        }
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

/// Column pitch and width of a tile's waveform, in points.
const WAVE_PITCH: f32 = 3.0;
const WAVE_BAR: f32 = 2.0;

/// Per column of a tile `width` wide, the (peak, body) half-heights as 0..1
/// of the tile's waveform room: the peak edge and the RMS body, on a decibel
/// scale. Always `columns` entries, read from the real bars.
fn tile_columns(w: &crate::services::library::Wave, columns: usize) -> Vec<(f32, f32)> {
    use crate::services::library::wave_height;
    (0..columns)
        .map(|k| {
            let (pk, rms) = w.range(k as f32 / columns as f32, (k + 1) as f32 / columns as f32);
            let peak = wave_height(pk);
            (peak, wave_height(rms).min(peak))
        })
        .collect()
}

/// A sound's real waveform, centred and symmetric: a pale peak edge round a
/// stronger RMS body, so dynamics show. `None` (still being read, or
/// unreadable) is a quiet dotted centre line, not an invented wave. The tile
/// is a quiet tint of the audio colour; hovering brightens it and sweeps a
/// playhead, the part already played drawn full strength.
fn peaks_tile(p: &Painter, r: Rect, wave: Option<&crate::services::library::Wave>, clock: Option<f64>, hover: f32, t: &theme::Tokens) {
    let bg = theme::mix(t.sunken, t.audio, if t.dark { 0.16 } else { 0.22 });
    p.rect_filled(r, 0, theme::mix(bg, Color32::WHITE, 0.05 * hover));
    let mid = r.center().y;
    let ink = theme::mix(t.audio, Color32::WHITE, if t.dark { 0.62 } else { 0.35 });
    let Some(wave) = wave else {
        let mut x = r.left() + 8.0;
        while x < r.right() - 8.0 {
            p.circle_filled(egui::pos2(x, mid), 0.8, theme::mix(bg, ink, 0.5));
            x += 6.0;
        }
        return;
    };
    let cols = (((r.width() - 4.0) / WAVE_PITCH) as usize).max(1);
    let room = r.height() * 0.4;
    let play = clock.map(|c| ((c * 0.3) % 1.0) as f32);
    for (k, (peak, body)) in tile_columns(wave, cols).into_iter().enumerate() {
        let x = r.left() + 2.0 + k as f32 * WAVE_PITCH;
        let played = play.is_none_or(|pl| (k as f32 / cols as f32) < pl);
        let k_ink = if played { 1.0 } else { 0.55 };
        let edge = Rect::from_center_size(egui::pos2(x + WAVE_BAR / 2.0, mid), Vec2::new(WAVE_BAR, (peak * room * 2.0).max(2.0)));
        let core = Rect::from_center_size(edge.center(), Vec2::new(WAVE_BAR, (body * room * 2.0).max(1.0).min(edge.height())));
        p.rect_filled(edge, 1, ink.gamma_multiply(0.38 * k_ink));
        p.rect_filled(core, 1, ink.gamma_multiply(k_ink));
    }
    if let Some(pl) = play {
        p.vline(egui::lerp(r.left()..=r.right(), pl), r.y_range(), egui::Stroke::new(1.0, Color32::from_white_alpha(200)));
    }
}

/// Width a Tools card gives its Run button: the label, the icon and the
/// button's padding (so "Needs model" is never cut at the card edge), at
/// least 92 and at most half the card.
fn run_button_width(p: &Painter, label: &str, card_w: f32) -> f32 {
    let text = p.layout_no_wrap(label.to_owned(), theme::control_medium(), Color32::WHITE).size().x;
    (text + 2.0 * 12.0 + icons::SIZE + 6.0).max(92.0).min(card_w * 0.5)
}

/// The duration badge of a sound: the project's length for it, else the
/// length read from the file, else a dash. Never missing.
fn audio_badge(dur: Time, read_secs: Option<f64>) -> String {
    if dur > 0 {
        view::format_short(dur)
    } else if let Some(s) = read_secs.filter(|s| *s > 0.0) {
        view::format_short((s * SEC as f64) as Time)
    } else {
        "–".into()
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
/// Whether the tool does something today: it has an action here, or it is
/// one of the two Inspector tools run from this list.
fn ai_runnable(name: &str) -> bool {
    crate::services::ai_run::Action::for_tool(name).is_some() || (cfg!(feature = "render") && matches!(name, "Remove background" | "Smooth slow motion"))
}

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
        let running = action.is_some_and(|a| ai_runner().lock().unwrap_or_else(|e| e.into_inner()).is_running(a));
        #[cfg(feature = "render")]
        let running = running || (cut && (crate::editor::cutout::busy() || crate::editor::slowmo::busy()));
        let label = if running { "Running" } else if ready { "Run" } else { "Needs model" };
        let btn_w = run_button_width(p, label, r.width());
        kit::paint_text_clipped(p, egui::pos2(tx, r.top() + 22.0), Align2::LEFT_CENTER, tool.name, theme::control_medium(), t.text, r.right() - tx - btn_w - 16.0);
        let note = match (&row, action) {
            (Some(r), None) if cut && !r.ready => format!("{} {}", tool.blurb, r.note),
            (_, None) if cut => format!("{} {}", tool.blurb, if tool.name == "Remove background" { "People stay, the rest goes; Inspector ▸ Cut out picks objects." } else { "On the selected slowed clip (speed under 1, or a ramp)." }),
            (_, None) => tool.blurb.to_owned(),
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
        Ok(out) => super::toast_undo(&mut app.editor, ctx, out.message, Icon::Check),
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
    match rail {
        RailItem::Story => return,
        RailItem::Motions | RailItem::Cuts | RailItem::Ai => {
            apply(app, ctx, rail, i, None);
            return;
        }
        _ => {}
    }
    // One implementation of "add this item to the timeline", shared with the agent.
    let panel = crate::services::library::Panel::ALL[rail.index()];
    match crate::services::library::add(&mut app.doc, panel, i, t, lane) {
        Ok(out) => {
            app.editor.selection = out.select;
            super::toast_undo(&mut app.editor, ctx, out.message, Icon::Plus);
        }
        Err(r) => {
            kit::push(ctx, r.0, None, Some(Icon::Info));
        }
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
        super::toast_undo(&mut app.editor, ctx, format!("{} added as an adjustment clip", item.name), Icon::MagicWand);
    }
}

pub fn apply_to(app: &mut StudioApp, ctx: &egui::Context, d: LibDrag, clip: u32) {
    apply(app, ctx, d.rail, d.index, Some(clip));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_menu_name_and_folder_rows_are_labelled() {
        let ctx = egui::Context::default();
        let raw = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(300.0, 200.0))), ..Default::default() };
        let (mut name, mut folder) = ("clip_b.mp4".to_owned(), String::new());
        let mut out = ctx.run_ui(raw, |ui| {
            media_menu_fields(ui, &mut name, &mut folder);
        });
        out.textures_delta.clear();
        let mut texts = vec![];
        fn walk(s: &egui::epaint::Shape, out: &mut Vec<String>) {
            match s {
                egui::epaint::Shape::Text(t) => out.push(t.galley.text().to_owned()),
                egui::epaint::Shape::Vec(v) => v.iter().for_each(|s| walk(s, out)),
                _ => {}
            }
        }
        out.shapes.iter().for_each(|c| walk(&c.shape, &mut texts));
        assert!(texts.iter().any(|t| t == "Name"), "a Name caption: {texts:?}");
        assert!(texts.iter().any(|t| t == "Folder"), "a Folder caption: {texts:?}");
        assert!(texts.iter().any(|t| t == "clip_b.mp4"), "the name is the field's text");
        assert!(texts.iter().any(|t| t == "No folder"), "an empty folder says so: {texts:?}");
    }

    /// A tile drawn on the CPU, no window: RGBA of `w`×`h` points.
    fn tile_frame(wave: Option<&crate::services::library::Wave>, clock: Option<f64>, hover: f32, dark: bool) -> (Vec<u8>, usize, usize, theme::Tokens) {
        let ctx = egui::Context::default();
        ctx.set_visuals(if dark { egui::Visuals::dark() } else { egui::Visuals::light() });
        let (w, h) = (180usize, 100usize);
        let mut textures = crate::ui_shot::Textures::default();
        let raw = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(w as f32, h as f32))), ..Default::default() };
        let mut tokens = None;
        let mut out = ctx.run_ui(raw, |ui| {
            let t = theme::tokens(ui.ctx());
            peaks_tile(ui.painter(), Rect::from_min_size(Pos2::ZERO, egui::vec2(w as f32, h as f32)), wave, clock, hover, &t);
            tokens = Some(t);
        });
        textures.apply(&out.textures_delta);
        out.textures_delta.clear();
        let prims = ctx.tessellate(out.shapes.clone(), 1.0);
        let t = tokens.unwrap();
        (crate::ui_shot::render(&prims, &textures, [w, h], 1.0, Color32::BLACK), w, h, t)
    }

    fn lum(px: &[u8], w: usize, x: usize, y: usize) -> f32 {
        let i = (y * w + x) * 4;
        0.2126 * px[i] as f32 + 0.7152 * px[i + 1] as f32 + 0.0722 * px[i + 2] as f32
    }

    /// Mostly quiet speech with two loud hits and a pause.
    fn dynamic_wave() -> crate::services::library::Wave {
        let n = 48000usize;
        let x: Vec<f32> = (0..n)
            .map(|i| {
                let u = i as f32 / n as f32;
                let env = if (0.40..0.50).contains(&u) { 0.0 } else if (0.25..0.30).contains(&u) || (0.70..0.75).contains(&u) { 0.9 } else { 0.08 };
                (i as f32 * 0.21).sin() * env
            })
            .collect();
        crate::services::library::wave_of(&x, &[], 48000)
    }

    #[test]
    fn tile_columns_keep_dynamics_instead_of_clipped_blocks() {
        let cols = tile_columns(&dynamic_wave(), 56);
        assert_eq!(cols.len(), 56);
        assert!(cols.iter().all(|&(p, b)| (0.0..=1.0).contains(&p) && b <= p));
        let tallest = cols.iter().map(|c| c.0).fold(0.0, f32::max);
        let at_top = cols.iter().filter(|c| c.0 > 0.95 * tallest).count();
        assert!(at_top < cols.len() / 4, "only the loud hits reach the top ({at_top} of {})", cols.len());
        assert!(cols.iter().any(|c| c.0 > 0.5 && c.0 < 0.9), "quiet speech has a visible, shorter body");
        assert!(cols.iter().any(|c| c.0 == 0.0), "the pause is the centre line");
    }

    #[test]
    fn audio_tile_frame_is_a_symmetric_waveform_on_a_quiet_tile() {
        let wave = dynamic_wave();
        for dark in [true, false] {
            let (px, w, h, t) = tile_frame(Some(&wave), None, 0.0, dark);
            let mid = h / 2;
            // Quiet tile: the corner is close to the neutral sunken colour, far from a saturated fill.
            let corner = [px[0], px[1], px[2]];
            let sunken = [t.sunken.r(), t.sunken.g(), t.sunken.b()];
            let tint = corner.iter().zip(sunken).map(|(a, b)| (*a as i32 - b as i32).abs()).max().unwrap();
            assert!(tint < 60, "tile tint stays quiet (dark={dark}): {corner:?} vs {sunken:?}");
            let bg = lum(&px, w, 1, 1);
            // The waveform is symmetric about the centre line, row by row.
            let mut ink_rows = vec![];
            for d in 0..mid {
                let up: usize = (0..w).filter(|&x| (lum(&px, w, x, mid - 1 - d) - bg).abs() > 14.0).count();
                let down: usize = (0..w).filter(|&x| (lum(&px, w, x, mid + d) - bg).abs() > 14.0).count();
                assert!((up as i32 - down as i32).abs() <= 2, "row {d} off the centre: {up} above, {down} below (dark={dark})");
                ink_rows.push(up);
            }
            assert!(ink_rows[0] > 20, "there is a body at the centre line");
            // Not a solid block: the wave thins out going up, and the very top rows are nearly empty.
            let near_top = ink_rows[(mid as f32 * 0.9) as usize];
            assert!(near_top < ink_rows[0] / 2, "only the loud hits reach the top: {near_top} vs {}", ink_rows[0]);
            assert!(ink_rows.windows(2).all(|p| p[1] <= p[0] + 2), "rows never fill in higher up");
            // The body is stronger than its peak edge: at a loud column the centre is brighter than the outer rows.
            let loud_x = (w as f32 * 0.28) as usize;
            let centre = (loud_x - 3..loud_x + 3).map(|x| (lum(&px, w, x, mid) - bg).abs()).fold(0.0, f32::max);
            let edge_row = (mid as f32 * 0.25) as usize;
            let edge = (loud_x - 3..loud_x + 3).map(|x| (lum(&px, w, x, edge_row) - bg).abs()).fold(0.0, f32::max);
            assert!(centre > edge, "RMS body {centre} stronger than the peak edge {edge}");
        }
    }

    #[test]
    fn audio_tile_without_a_read_is_a_quiet_line_and_hover_plays() {
        let (px, w, h, _) = tile_frame(None, None, 0.0, true);
        let bg = lum(&px, w, 1, 1);
        let off_centre = (0..h).filter(|y| (*y as i32 - (h / 2) as i32).abs() > 3).any(|y| (0..w).any(|x| (lum(&px, w, x, y) - bg).abs() > 8.0));
        assert!(!off_centre, "no wave is invented while the sound is being read");
        assert!((0..w).any(|x| (lum(&px, w, x, h / 2) - bg).abs() > 8.0), "a dotted centre line says it is waiting");
        // Hovering brightens the tile and puts a playhead on it.
        let wave = dynamic_wave();
        let (rest, ..) = tile_frame(Some(&wave), None, 0.0, true);
        let (live, ..) = tile_frame(Some(&wave), Some(1.0), 1.0, true);
        assert!(lum(&live, w, 1, 1) > lum(&rest, w, 1, 1), "hover lifts the tile");
        let head = ((1.0f64 * 0.3) % 1.0) as f32 * w as f32;
        assert!(lum(&live, w, head as usize, 2) > lum(&live, w, (head as usize + 20).min(w - 1), 2) + 40.0, "the playhead shows");
    }

    #[test]
    fn audio_badge_is_never_missing() {
        assert_eq!(audio_badge(2 * SEC, None), "2.0s");
        assert_eq!(audio_badge(0, Some(75.0)), view::format_short(75 * SEC));
        assert_eq!(audio_badge(0, None), "–");
        assert_eq!(audio_badge(0, Some(0.0)), "–");
    }

    #[test]
    fn tools_run_button_is_given_the_room_its_label_needs() {
        let ctx = egui::Context::default();
        let raw = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(420.0, 200.0))), ..Default::default() };
        let mut checked = 0;
        ctx.run_ui(raw, |ui| {
            for label in ["Run", "Running", "Needs model"] {
                let card_w = 360.0;
                let w = run_button_width(ui.painter(), label, card_w);
                assert!(w >= 92.0 && w <= card_w * 0.5);
                let r = Button::secondary(label).icon(Icon::PlayFill).min_width(w).show(ui);
                assert!(r.rect.width() <= w + 0.5, "{label}: button {} wider than its slot {w}", r.rect.width());
                checked += 1;
            }
        })
        .textures_delta
        .clear();
        assert_eq!(checked, 3);
    }

    #[test]
    fn media_menu_stays_where_it_opened_while_the_pointer_moves() {
        let ctx = egui::Context::default();
        let at = |x: f32, y: f32| egui::RawInput { events: vec![egui::Event::PointerMoved(egui::pos2(x, y))], ..Default::default() };
        ctx.run_ui(at(120.0, 80.0), |ui| open_media_menu(ui.ctx(), 7)).textures_delta.clear();
        ctx.run_ui(at(400.0, 300.0), |_| {}).textures_delta.clear();
        ctx.run_ui(at(410.0, 320.0), |_| {}).textures_delta.clear();
        assert_eq!(media_menu_anchor(&ctx), egui::pos2(120.0, 80.0));
        assert!(kit::is_open(&ctx, egui::Id::new("media-menu")));
    }

    #[test]
    fn long_media_goes_in_whole() {
        // A 1:30 video and a 3-minute song keep their full length.
        assert_eq!(media_clip_len(MediaKind::Video, 90 * SEC), 90 * SEC);
        assert_eq!(media_clip_len(MediaKind::Audio, 180 * SEC), 180 * SEC);
        assert_eq!(media_clip_len(MediaKind::Image, 0), 4 * SEC);
    }

    #[test]
    fn rail_tabs_that_do_not_fit_go_to_the_overflow_menu() {
        let widths = vec![50.0; RailItem::ALL.len()];
        // Wide enough: every tab shows and there is no "…".
        assert_eq!(tabs_that_fit(&widths, 700.0), RailItem::ALL.len());
        // The default library width: some tabs, never none, never all, and
        // the tabs plus the "…" cell fit across.
        let n = tabs_that_fit(&widths, 340.0);
        assert!((1..RailItem::ALL.len()).contains(&n), "{n}");
        assert!(6.0 + MORE_W + 2.0 + n as f32 * 52.0 <= 340.0);
        // Hopelessly narrow: still one tab and the menu.
        assert_eq!(tabs_that_fit(&widths, 20.0), 1);
    }

    #[test]
    fn tool_labels_follow_the_glossary_and_keys_stay() {
        assert_eq!(RailItem::Cuts.label(), "Transitions");
        assert_eq!(RailItem::Motions.label(), "Animations");
        assert_eq!(RailItem::Ai.label(), "Tools");
        // Saved workspaces keep finding their section by the old key.
        assert_eq!(RailItem::Ai.name(), "AI");
        assert!(!RailItem::Ai.groups().iter().any(|g| g.contains("AI editor")));
    }

    #[test]
    fn tools_list_has_no_unfinished_entries() {
        for t in view::AI_TOOLS.iter().filter(|t| !ai_runnable(t.name)) {
            assert!(matches!(t.name, "Trim silences" | "Smart reframe" | "Scene cuts" | "Remove background" | "Smooth slow motion"), "{}", t.name);
        }
        assert!(ai_runnable("Auto captions") && !ai_runnable("Scene cuts"));
    }

    /// Library ▸ Tools ▸ Available crashed the signed app: listing the
    /// bundled plugins compiled WebAssembly, which hardened runtime kills
    /// without the JIT entitlement. Draw every Tools chip headless, in a
    /// narrow library, and keep the sections reachable.
    #[test]
    fn every_tools_chip_draws_headless() {
        use crate::ui_tests::Harness;
        let mut h = Harness::new(&["--screen", "editor"]);
        h.frames(3);
        for chip in 1..=3 {
            h.app.editor.rail = RailItem::Ai;
            h.app.editor.library_chip[RailItem::Ai.index()] = chip;
            h.app.editor.open_panel(crate::dock::Panel::Library);
            h.frames(3);
            assert_eq!(h.app.editor.rail, RailItem::Ai);
            assert_eq!(h.app.editor.library_chip[RailItem::Ai.index()], chip, "chip {chip} kept");
        }
    }
}
