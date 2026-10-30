// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Home: a side rail (Home, Projects, Templates, Learn, and the other ways
//! in), then the page for the chosen tab. The Home page opens with the last
//! project large and a column of canvases for a new one, then recent
//! projects, templates and lessons as picture cards. It is built from the
//! editor's own surfaces, accent and spacing.

use crate::app::{Overlay, StudioApp};
use crate::icons::{self, Icon};
use crate::kit::{self, Button, IconButton};
use crate::templates::{self, ALL, GUIDED};
use crate::{files, theme, view};
use crate::kit::ShowNoted;
use eframe::egui::{self, Align2, Color32, CornerRadius, Rect, Role, Sense, Vec2, WidgetInfo};
use std::path::PathBuf;

/// The pages of Home, in the order of the side rail.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum Tab {
    #[default]
    Home,
    Projects,
    Templates,
    Learn,
}

impl Tab {
    pub const ALL: [Tab; 4] = [Tab::Home, Tab::Projects, Tab::Templates, Tab::Learn];

    pub fn name(self) -> &'static str {
        match self {
            Tab::Home => "Home",
            Tab::Projects => "Projects",
            Tab::Templates => "Templates",
            Tab::Learn => "Learn",
        }
    }

    fn icon(self) -> Icon {
        match self {
            Tab::Home => Icon::House,
            Tab::Projects => Icon::Stack,
            Tab::Templates => Icon::SquaresFour,
            Tab::Learn => Icon::Sparkle,
        }
    }
}

/// What a template looks like on its card: its lanes and where its markers
/// sit along the timeline.
pub struct TplPreview {
    lanes: Vec<(view::LaneKind, Color32)>,
    marks: Vec<f32>,
}

#[derive(Default)]
pub struct StartState {
    pub search: String,
    pub custom_w: f32,
    pub custom_h: f32,
    pub custom_fps: usize,
    /// Each recent project's cover picture (see `files::cover_of`), looked
    /// up once.
    pub covers: std::collections::HashMap<std::path::PathBuf, Option<(std::path::PathBuf, f64)>>,
    pub tab: Tab,
    /// The project card the arrow keys are on.
    pub sel: Option<usize>,
    /// Columns of the project cards in the last frame (for Up and Down).
    pub cols: usize,
    /// Projects taken off the list this session, so the shared list does not
    /// bring them back.
    pub hidden: std::collections::HashSet<PathBuf>,
    previews: Vec<TplPreview>,
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

/// Recent projects shown before "Show all" on the Projects page.
const RECENT_TILES: usize = 12;

/// Height of the strip at the top kept clear for the window buttons.
pub const TITLE_BAR_H: f32 = 52.0;

/// Width of the side rail.
const NAV_W: f32 = 216.0;
/// Windows narrower than this swap the rail for a strip under the header.
const NARROW: f32 = 860.0;
/// Space between cards.
const GAP: f32 = 12.0;
/// Narrowest project, template or lesson card.
const CARD_MIN_W: f32 = 200.0;

/// Where the parts of the screen sit in a window of `full`.
pub struct Layout {
    /// The side rail, in a wide window.
    pub nav: Option<Rect>,
    pub main: Rect,
    /// The centred column the page is laid out in.
    pub column: Rect,
    pub search: Rect,
    /// Settings, as an icon beside the search in a narrow window.
    pub gear: Rect,
    /// The four tabs: down the rail, or in a row.
    pub tabs: [Rect; 4],
    /// Open project, Import timeline, Demo project.
    pub actions: [Rect; 3],
    /// Settings, at the foot of the rail.
    pub settings: Rect,
    /// Where the page starts.
    pub body_top: f32,
    pub narrow: bool,
}

pub fn layout(full: Rect) -> Layout {
    let rtl = crate::i18n::is_rtl();
    let narrow = full.width() < NARROW;
    let flip = |r: Rect| if rtl { crate::i18n::mirror(r, full) } else { r };
    let nav = (!narrow).then(|| flip(Rect::from_min_size(full.min, Vec2::new(NAV_W, full.height()))));
    let main = if narrow {
        full
    } else if rtl {
        Rect::from_min_max(full.min, egui::pos2(full.right() - NAV_W, full.bottom()))
    } else {
        Rect::from_min_max(egui::pos2(full.left() + NAV_W, full.top()), full.max)
    };
    let pad = if narrow { 16.0 } else { 32.0 };
    let content_w = (main.width() - 2.0 * pad).clamp(280.0, 1240.0);
    let cy = main.top() + if narrow { TITLE_BAR_H + 18.0 } else { 34.0 };
    let column = Rect::from_min_max(egui::pos2(main.center().x - content_w / 2.0, cy), egui::pos2(main.center().x + content_w / 2.0, main.bottom()));
    let search_w = if narrow { (content_w - 100.0).clamp(120.0, 260.0) } else { 280.0_f32.min(content_w * 0.5) };
    let search = if narrow {
        // After the wordmark and before the gear, ending at the far side.
        Rect::from_min_size(egui::pos2(column.right() - 36.0 - search_w, cy - 14.0), Vec2::new(search_w, 28.0))
    } else {
        Rect::from_min_size(egui::pos2(column.right() - search_w, cy - 14.0), Vec2::new(search_w, 28.0))
    };
    let gear = Rect::from_center_size(egui::pos2(column.right() - 14.0, cy), Vec2::splat(28.0));
    let mut tabs = [Rect::NOTHING; 4];
    let mut actions = [Rect::NOTHING; 3];
    let mut settings = Rect::NOTHING;
    let body_top;
    if let Some(n) = nav {
        let x = n.left() + 8.0;
        let w = n.width() - 16.0;
        let top = n.top() + TITLE_BAR_H + 52.0;
        for (i, r) in tabs.iter_mut().enumerate() {
            *r = Rect::from_min_size(egui::pos2(x, top + i as f32 * 34.0), Vec2::new(w, 32.0));
        }
        let atop = top + 4.0 * 34.0 + 30.0;
        for (i, r) in actions.iter_mut().enumerate() {
            *r = Rect::from_min_size(egui::pos2(x, atop + i as f32 * 34.0), Vec2::new(w, 32.0));
        }
        settings = Rect::from_min_size(egui::pos2(x, n.bottom() - 44.0), Vec2::new(w, 32.0));
        body_top = cy + 28.0;
    } else {
        let tw = (column.width() - 3.0 * 6.0) / 4.0;
        let ty = cy + 28.0;
        for (i, r) in tabs.iter_mut().enumerate() {
            *r = Rect::from_min_size(egui::pos2(column.left() + i as f32 * (tw + 6.0), ty), Vec2::new(tw, 32.0));
        }
        let aw = (column.width() - 2.0 * 6.0) / 3.0;
        let ay = ty + 38.0;
        for (i, r) in actions.iter_mut().enumerate() {
            *r = Rect::from_min_size(egui::pos2(column.left() + i as f32 * (aw + 6.0), ay), Vec2::new(aw, 32.0));
        }
        body_top = ay + 32.0 + 16.0;
    }
    Layout { nav, main, column, search, gear, tabs, actions, settings, body_top, narrow }
}

/// What the person asked for this frame; done after drawing.
enum Act {
    Tab(Tab),
    NewPreset(usize),
    ToggleCustom,
    Open(PathBuf),
    Reveal(PathBuf),
    Forget(PathBuf),
    Sample(&'static view::Recent),
    Template(usize),
    Guided(usize),
    OpenFolder,
    Import,
    Demo,
    Settings,
    ClearSearch,
}

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

    // Projects made elsewhere (a terminal, an agent) join the list; looked
    // for every few seconds.
    {
        static LAST: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);
        let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
        // Tests never read this computer's real list of projects.
        if !cfg!(test) && last.is_none_or(|t| t.elapsed() > std::time::Duration::from_secs(3)) {
            *last = Some(std::time::Instant::now());
            files::merge_shared(&mut app.prefs.recents);
        }
    }
    if !app.start.hidden.is_empty() {
        let hidden = &app.start.hidden;
        app.prefs.recents.retain(|r| !hidden.contains(&r.path));
    }
    if app.start.previews.is_empty() {
        let lang = crate::i18n::lang();
        app.start.previews = ALL
            .iter()
            .map(|tpl| {
                let p = tpl.build(lang);
                let end = p.markers.iter().map(|m| m.time).max().unwrap_or(1).max(1) as f32 * 1.08;
                TplPreview { lanes: p.lanes.iter().map(|l| (l.kind, l.colour)).collect(), marks: p.markers.iter().map(|m| m.time as f32 / end).collect() }
            })
            .collect();
    }

    let lay = layout(full);
    let rtl = crate::i18n::is_rtl();
    let mut act: Option<Act> = None;
    let tab = app.start.tab;

    // The rail (wide), or the wordmark and tab strip (narrow).
    if let Some(nav) = lay.nav {
        ui.painter().rect_filled(nav, 0, t.panel);
        let edge = if rtl { nav.left() } else { nav.right() - 1.0 };
        ui.painter().rect_filled(Rect::from_min_size(egui::pos2(edge, nav.top()), Vec2::new(1.0, nav.height())), 0, t.line);
        let hy = nav.top() + TITLE_BAR_H + 4.0;
        let wx = if rtl { nav.right() - 24.0 - 110.0 } else { nav.left() + 12.0 };
        wordmark(ui.painter(), egui::pos2(wx, hy), &t);
        ui.painter().text(egui::pos2(if rtl { nav.right() - 20.0 } else { nav.left() + 20.0 }, lay.actions[0].top() - 16.0), if rtl { Align2::RIGHT_CENTER } else { Align2::LEFT_CENTER }, crate::i18n::shown("Start"), theme::micro(), t.text_3);
    } else {
        wordmark(ui.painter(), egui::pos2(lay.column.left(), lay.column.top()), &t);
    }
    for (i, tb) in Tab::ALL.into_iter().enumerate() {
        if nav_item(ui, lay.tabs[i], tb.icon(), tb.name(), "", tab == tb, lay.narrow).clicked() {
            act = Some(Act::Tab(tb));
        }
    }
    let rows: [(Icon, &str, &str, Act); 3] = [
        (Icon::FolderOpen, "Open project", "⌘O", Act::OpenFolder),
        (Icon::DownloadSimple, "Import timeline", "", Act::Import),
        (Icon::Play, "Demo project", "", Act::Demo),
    ];
    for (i, (icon, label, hint, a)) in rows.into_iter().enumerate() {
        if nav_item(ui, lay.actions[i], icon, label, hint, false, lay.narrow).clicked() {
            act = Some(a);
        }
    }
    let settings_keys = crate::keymap::hint(&app.prefs, "settings");
    if lay.narrow {
        if IconButton::new(Icon::GearSix, "Settings").shortcut(&settings_keys).show_at(ui, lay.gear).clicked() {
            act = Some(Act::Settings);
        }
    } else if nav_item(ui, lay.settings, Icon::GearSix, "Settings", &settings_keys, false, false).clicked() {
        act = Some(Act::Settings);
    }

    // Header: the page's name, and the search.
    let q = app.start.search.to_lowercase();
    if !lay.narrow {
        let title = if q.is_empty() { tab.name() } else { "Search" };
        ui.painter().text(if rtl { lay.column.right_center() } else { lay.column.left_center() }, if rtl { Align2::RIGHT_CENTER } else { Align2::LEFT_CENTER }, crate::i18n::shown(title), theme::dialog_title(), t.text);
    }
    let mut sui = ui.new_child(egui::UiBuilder::new().max_rect(lay.search));
    kit::text_field(&mut sui, &mut app.start.search, "Search projects", Some(Icon::MagnifyingGlass), lay.search.width(), None);

    // The page.
    let recents: Vec<files::Recent> = if app.cli.flag("no-recents") { vec![] } else { app.prefs.recents.iter().filter(|r| q.is_empty() || r.name.to_lowercase().contains(&q)).cloned().collect() };
    let samples: Vec<&'static view::Recent> = if app.cli.flag("no-samples") { vec![] } else { view::RECENTS.iter().filter(|r| q.is_empty() || r.name.to_lowercase().contains(&q)).collect() };
    let lang = crate::i18n::lang();
    let tpl_names: Vec<String> = ALL.iter().map(|x| x.name(lang)).collect();
    let tpl_hits: Vec<usize> = (0..ALL.len()).filter(|&i| q.is_empty() || tpl_names[i].to_lowercase().contains(&q)).collect();
    let page = Rect::from_min_max(egui::pos2(lay.main.left(), lay.body_top), lay.main.max);
    let mut gui = ui.new_child(egui::UiBuilder::new().max_rect(page));
    let content_w = lay.column.width();
    let hits = Hits { recents: &recents, samples: &samples, tpl_names: &tpl_names, tpl_hits: &tpl_hits };
    egui::ScrollArea::vertical().auto_shrink([false, false]).show_v(&mut gui, |ui| {
        let pad = ((ui.available_width() - content_w) / 2.0).max(0.0);
        ui.horizontal_top(|ui| {
            ui.add_space(pad);
            ui.vertical(|ui| {
                ui.set_width(content_w);
                ui.add_space(theme::space::SM);
                body(app, ui, &lay, &hits, &mut act);
                ui.add_space(theme::space::XXL);
            });
        });
    });

    let ctx = ui.ctx().clone();
    match act {
        None => {}
        Some(Act::Tab(tb)) => {
            app.start.tab = tb;
            app.start.sel = None;
        }
        Some(Act::NewPreset(i)) => new_project(app, &ctx, &PRESETS[i]),
        Some(Act::ToggleCustom) => {
            kit::toggle_open(&ctx, egui::Id::unique("start-custom"));
            if app.start.custom_w == 0.0 {
                app.start.custom_w = 2560.0;
                app.start.custom_h = 1440.0;
                app.start.custom_fps = 1;
            }
        }
        Some(Act::Open(path)) => app.open_folder(&ctx, Some(path)),
        Some(Act::Reveal(path)) => {
            if crate::services::export::reveal(&path).is_err() {
                kit::push_error(&ctx, "Could not show the project in Finder");
            }
        }
        Some(Act::Forget(path)) => forget(app, &ctx, &path),
        Some(Act::Sample(rec)) => {
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
            app.replace(&ctx, files::Replace::Sample(p));
        }
        Some(Act::Template(i)) => templates::open(app, &ctx, ALL[i].build(lang)),
        Some(Act::Guided(i)) => templates::open(app, &ctx, templates::build_guided(GUIDED[i].0, lang)),
        Some(Act::OpenFolder) if dialogs => app.open_folder(&ctx, None),
        Some(Act::Import) if dialogs => app.import_timeline(&ctx),
        Some(Act::Demo) => app.replace(&ctx, files::Replace::Sample(view::demo())),
        Some(Act::Settings) => app.overlay = Overlay::Settings,
        Some(Act::ClearSearch) => app.start.search.clear(),
        Some(Act::OpenFolder | Act::Import) => {}
    }
}

/// Take a project off the list. The project itself is left alone.
fn forget(app: &mut StudioApp, ctx: &egui::Context, path: &std::path::Path) {
    let name = app.prefs.recents.iter().find(|r| r.path == path).map(|r| r.name.clone()).unwrap_or_default();
    app.prefs.recents.retain(|r| r.path != path);
    app.start.hidden.insert(path.to_path_buf());
    app.start.sel = None;
    kit::push(ctx, crate::i18n::tf(format!("Removed {name} from Home")), None, Some(Icon::CheckCircle));
}

struct Hits<'a> {
    recents: &'a [files::Recent],
    samples: &'a [&'static view::Recent],
    tpl_names: &'a [String],
    tpl_hits: &'a [usize],
}

fn body(app: &mut StudioApp, ui: &mut egui::Ui, lay: &Layout, hits: &Hits, act: &mut Option<Act>) {
    let t = theme::tokens(ui.ctx());
    let searching = !app.start.search.is_empty();
    let w = ui.available_width();
    let cols = cols_for(w, CARD_MIN_W);

    if searching {
        if hits.recents.is_empty() && hits.samples.is_empty() && hits.tpl_hits.is_empty() {
            let r = ui.allocate_exact_size(Vec2::new(w, 240.0), Sense::hover()).0;
            ui.painter().rect_filled(r, CornerRadius::same(theme::radius::PANEL), t.panel);
            let mut eui = ui.new_child(egui::UiBuilder::new().max_rect(r));
            if kit::empty_state(&mut eui, Icon::FilmSlate, "No projects match your search.", Some(("Clear search", Icon::X))) {
                *act = Some(Act::ClearSearch);
            }
            return;
        }
        if !hits.recents.is_empty() {
            heading(ui, "Recent projects", None);
            recents_grid(app, ui, hits.recents, hits.recents.len(), act);
        }
        if !hits.tpl_hits.is_empty() {
            section_gap(ui);
            heading(ui, &crate::i18n::tr("home-templates"), None);
            templates_grid(app, ui, hits, hits.tpl_hits.len(), act);
        }
        if !hits.samples.is_empty() {
            section_gap(ui);
            samples_grid(app, ui, hits.samples, act);
        }
        return;
    }

    match app.start.tab {
        Tab::Home => {
            hero(app, ui, lay, hits.recents, act);
            if !hits.recents.is_empty() {
                section_gap(ui);
                let shown = cols.max(4).min(hits.recents.len());
                let more = (hits.recents.len() > shown).then(|| crate::i18n::tf(format!("Show all {} projects", hits.recents.len())));
                if heading(ui, "Recent projects", more.as_deref()) {
                    *act = Some(Act::Tab(Tab::Projects));
                }
                recents_grid(app, ui, hits.recents, shown, act);
            }
            section_gap(ui);
            let shown = cols.min(ALL.len());
            let more = (shown < ALL.len()).then_some("See all");
            if heading(ui, &crate::i18n::tr("home-templates"), more) {
                *act = Some(Act::Tab(Tab::Templates));
            }
            templates_grid(app, ui, hits, shown, act);
            section_gap(ui);
            let shown = cols.min(GUIDED.len());
            let more = (shown < GUIDED.len()).then_some("See all");
            if heading(ui, &crate::i18n::tr("home-learn"), more) {
                *act = Some(Act::Tab(Tab::Learn));
            }
            guided_grid(app, ui, shown, act);
        }
        Tab::Projects => {
            if hits.recents.is_empty() {
                let r = ui.allocate_exact_size(Vec2::new(w, 260.0), Sense::hover()).0;
                ui.painter().rect_filled(r, CornerRadius::same(theme::radius::PANEL), t.panel);
                let mut eui = ui.new_child(egui::UiBuilder::new().max_rect(r));
                if kit::empty_state(&mut eui, Icon::FilmSlate, "Projects you open or create will appear here.", Some(("New landscape project", Icon::Plus))) {
                    *act = Some(Act::NewPreset(0));
                }
            } else {
                let show_all_id = egui::Id::unique("start-show-all-recents");
                let show_all = ui.ctx().data(|d| d.get_temp::<bool>(show_all_id).unwrap_or(false));
                let hidden = if show_all { 0 } else { hits.recents.len().saturating_sub(RECENT_TILES) };
                heading(ui, "Recent projects", None);
                recents_grid(app, ui, hits.recents, hits.recents.len() - hidden, act);
                if hidden > 0 || show_all {
                    ui.add_space(theme::space::MD);
                    let label = if hidden > 0 { crate::i18n::tf(format!("Show all {} projects", hits.recents.len())) } else { crate::i18n::tf("Show fewer".to_owned()) };
                    if Button::ghost(&label).compact().show(ui).clicked() {
                        ui.ctx().data_mut(|d| d.insert_temp(show_all_id, hidden > 0));
                    }
                }
            }
        }
        Tab::Templates => {
            heading(ui, &crate::i18n::tr("home-templates"), None);
            templates_grid(app, ui, hits, ALL.len(), act);
        }
        Tab::Learn => {
            heading(ui, &crate::i18n::tr("home-learn"), None);
            guided_grid(app, ui, GUIDED.len(), act);
            if !hits.samples.is_empty() {
                section_gap(ui);
                samples_grid(app, ui, hits.samples, act);
            }
        }
    }
}

fn section_gap(ui: &mut egui::Ui) {
    ui.add_space(theme::space::XL);
}

/// A section's title, with a quiet link at the far end. True when the link
/// was clicked.
fn heading(ui: &mut egui::Ui, title: &str, more: Option<&str>) -> bool {
    let t = theme::tokens(ui.ctx());
    let mut clicked = false;
    ui.horizontal(|ui| {
        ui.set_min_height(theme::space::CONTROL_H_LARGE);
        ui.label(egui::RichText::new(crate::i18n::shown(title)).font(theme::section_title()).color(t.text));
        if let Some(more) = more {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                clicked = Button::ghost(more).compact().trailing(Icon::CaretRight).show(ui).clicked();
            });
        }
    });
    ui.add_space(theme::space::SM);
    clicked
}

fn cols_for(avail: f32, min_w: f32) -> usize {
    (((avail + GAP) / (min_w + GAP)).floor() as usize).max(1)
}

// --- the hero ---------------------------------------------------------------

/// The last project large, beside the canvases for a new one (above it in a
/// narrow window).
fn hero(app: &mut StudioApp, ui: &mut egui::Ui, lay: &Layout, recents: &[files::Recent], act: &mut Option<Act>) {
    let w = ui.available_width();
    let side_by_side = w >= 760.0;
    let new_w = 360.0_f32.min(w * 0.4);
    let new_h = 52.0 + 5.0 * 36.0;
    let cont_h = if side_by_side { new_h } else { 200.0 };
    let total_h = if side_by_side { new_h } else { cont_h + GAP + new_h };
    let area = ui.allocate_exact_size(Vec2::new(w, total_h), Sense::hover()).0;
    let (cont, newc) = if side_by_side {
        (Rect::from_min_size(area.min, Vec2::new(w - new_w - GAP, new_h)), Rect::from_min_size(area.min + Vec2::new(w - new_w, 0.0), Vec2::new(new_w, new_h)))
    } else {
        (Rect::from_min_size(area.min, Vec2::new(w, cont_h)), Rect::from_min_size(area.min + Vec2::new(0.0, cont_h + GAP), Vec2::new(w, new_h)))
    };
    let (cont, newc) = if crate::i18n::is_rtl() && side_by_side { (crate::i18n::mirror(cont, area), crate::i18n::mirror(newc, area)) } else { (cont, newc) };
    match recents.first() {
        Some(rec) => continue_card(app, ui, cont, rec, act),
        None => welcome_card(ui, cont, act),
    }
    new_card(app, ui, newc, act);
    let _ = lay;
}

fn continue_card(app: &mut StudioApp, ui: &mut egui::Ui, r: Rect, rec: &files::Recent, act: &mut Option<Act>) {
    let t = theme::tokens(ui.ctx());
    let rtl = crate::i18n::is_rtl();
    theme::surface(ui.painter(), r, theme::radius::CARD, t.panel, &t);
    let pad = 14.0;
    let media_w = (r.width() * 0.46).clamp(160.0, 400.0);
    let media = Rect::from_min_size(egui::pos2(if rtl { r.right() - pad - media_w } else { r.left() + pad }, r.top() + pad), Vec2::new(media_w, r.height() - 2.0 * pad));
    // The cover, looked up a few projects per frame so a long list never
    // stalls a frame.
    if !app.start.covers.contains_key(&rec.path) {
        app.start.covers.insert(rec.path.clone(), files::cover_of(&rec.path));
        ui.ctx().request_repaint();
    }
    let seed = app.start.covers.get(&rec.path).cloned().flatten().map(|(p, secs)| app.thumbs.file_seed(&p, secs)).unwrap_or_else(|| files::seed_of(&rec.path));
    let resp = ui.interact(media, egui::Id::unique(("home-continue", &rec.path)), Sense::click());
    resp.widget_info(|| WidgetInfo::labeled(Role::Button, true, format!("Continue {}", rec.name)));
    let st = kit::State::from_response(ui, &resp, None);
    let p = ui.painter().with_clip_rect(ui.clip_rect());
    let cr = CornerRadius::same(theme::radius::CARD);
    p.rect_filled(media, cr, t.sunken);
    letterboxed(&p.with_clip_rect(media), &mut app.thumbs, media, (rec.width.max(1), rec.height.max(1)), seed, st.hover);
    kit::round_corners(&p, media, theme::radius::CARD as f32, t.panel);
    // A play disc comes up over the cover on hover.
    if st.hover > 0.01 {
        let c = media.center();
        p.circle_filled(c, 22.0, theme::alpha(Color32::BLACK, 0.55 * st.hover));
        icons::paint(&p, Rect::from_center_size(c, Vec2::splat(20.0)), Icon::PlayFill, 20.0, theme::alpha(Color32::WHITE, st.hover));
        p.rect_stroke(media, cr, egui::Stroke::new(1.0, theme::alpha(t.accent, st.hover)), egui::StrokeKind::Inside);
    }
    if st.focused {
        kit::focus_ring(&p, media, cr, &t);
    }
    if resp.clicked() {
        *act = Some(Act::Open(rec.path.clone()));
    }
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    let _ = resp;

    let tx = if rtl { r.right() - pad - media_w - 20.0 } else { media.right() + 20.0 };
    let tw = r.width() - 2.0 * pad - media_w - 20.0;
    let (align_top, align_c) = if rtl { (Align2::RIGHT_TOP, Align2::RIGHT_CENTER) } else { (Align2::LEFT_TOP, Align2::LEFT_CENTER) };
    let _ = align_c;
    let mut y = r.top() + pad + 2.0;
    kit::paint_text_clipped(&p, egui::pos2(tx, y), align_top, "Continue where you left off", theme::caption(), t.accent, tw);
    y += 22.0;
    let cut = kit::paint_text_clipped(&p, egui::pos2(tx, y), align_top, &rec.name, theme::dialog_title(), t.text, tw);
    y += 32.0;
    let ago = crate::i18n::tf(format!("Edited {}", files::edited_label(rec.edited).to_lowercase()));
    kit::paint_text_clipped(&p, egui::pos2(tx, y), align_top, &ago, theme::body(), t.text_2, tw);
    y += 20.0;
    kit::paint_text_clipped(&p, egui::pos2(tx, y), align_top, &project_meta(rec), theme::body(), t.text_2, tw);
    y += 24.0;
    kit::paint_text_clipped(&p, egui::pos2(tx, y), align_top, &studio_paths::tilde(&rec.path), theme::caption(), t.text_3, tw);
    let _ = cut;
    // Open it, or show it in the Finder.
    let btn = Rect::from_min_size(egui::pos2(if rtl { r.right() - pad - media_w - 20.0 - tw } else { tx }, r.bottom() - pad - theme::space::CONTROL_H_LARGE), Vec2::new(tw, theme::space::CONTROL_H_LARGE));
    let mut row = ui.new_child(egui::UiBuilder::new().max_rect(btn).layout(crate::i18n::row_layout()));
    row.spacing_mut().item_spacing.x = theme::space::SM;
    if Button::primary("Continue").icon(Icon::PlayFill).show(&mut row).clicked() {
        *act = Some(Act::Open(rec.path.clone()));
    }
    if Button::ghost("Show in Finder").icon(Icon::FolderOpen).show(&mut row).clicked() {
        *act = Some(Act::Reveal(rec.path.clone()));
    }
}

/// First run: say where to begin.
fn welcome_card(ui: &mut egui::Ui, r: Rect, act: &mut Option<Act>) {
    let t = theme::tokens(ui.ctx());
    let rtl = crate::i18n::is_rtl();
    theme::surface(ui.painter(), r, theme::radius::CARD, t.panel, &t);
    let pad = 24.0;
    let art_w = (r.width() * 0.3).clamp(0.0, 200.0);
    let art = Rect::from_min_size(egui::pos2(if rtl { r.right() - pad - art_w } else { r.left() + pad }, r.top() + pad), Vec2::new(art_w, r.height() - 2.0 * pad));
    if art_w > 80.0 {
        ui.painter().rect_filled(art, CornerRadius::same(theme::radius::CARD), t.sunken);
        kit::illustration(ui.painter(), art.center(), Icon::FilmStrip, t.text_2);
    }
    let tx = if rtl { r.right() - pad - art_w - 20.0 } else { art.right() + 20.0 };
    let tx = if art_w > 80.0 { tx } else if rtl { r.right() - pad } else { r.left() + pad };
    let tw = if art_w > 80.0 { r.width() - 2.0 * pad - art_w - 20.0 } else { r.width() - 2.0 * pad };
    let (align_top, _) = if rtl { (Align2::RIGHT_TOP, ()) } else { (Align2::LEFT_TOP, ()) };
    let p = ui.painter().clone();
    let mut y = r.top() + pad + 2.0;
    kit::paint_text_clipped(&p, egui::pos2(tx, y), align_top, "Welcome to Zornov", theme::dialog_title(), t.text, tw);
    ui.interact(Rect::from_min_size(egui::pos2(if rtl { tx - tw } else { tx }, y), Vec2::new(tw, 28.0)), egui::Id::unique("home-welcome"), Sense::hover()).widget_info(|| WidgetInfo::labeled(Role::Label, true, "Welcome to Zornov"));
    y += 34.0;
    let sentence = "Pick a canvas to start a project, open one you have, or try the demo.";
    let galley = p.layout(crate::i18n::shown(sentence).into_owned(), theme::body(), t.text_2, tw);
    let gh = galley.size().y;
    p.galley(egui::pos2(if rtl { tx - galley.size().x } else { tx }, y), galley, t.text_2);
    y += gh + 4.0;
    let _ = y;
    let btn = Rect::from_min_size(egui::pos2(if rtl { tx - tw } else { tx }, r.bottom() - pad - theme::space::CONTROL_H_LARGE), Vec2::new(tw, theme::space::CONTROL_H_LARGE));
    let mut row = ui.new_child(egui::UiBuilder::new().max_rect(btn).layout(crate::i18n::row_layout()));
    row.spacing_mut().item_spacing.x = theme::space::SM;
    if Button::primary("New landscape project").icon(Icon::Plus).show(&mut row).clicked() {
        *act = Some(Act::NewPreset(0));
    }
    if Button::secondary("Try the demo project").show(&mut row).clicked() {
        *act = Some(Act::Demo);
    }
}

/// The canvases for a new project, one row each.
fn new_card(app: &mut StudioApp, ui: &mut egui::Ui, r: Rect, act: &mut Option<Act>) {
    let t = theme::tokens(ui.ctx());
    let rtl = crate::i18n::is_rtl();
    theme::surface(ui.painter(), r, theme::radius::CARD, t.panel, &t);
    let pad = 14.0;
    let (left_c, right_c) = if rtl { (Align2::RIGHT_CENTER, Align2::LEFT_CENTER) } else { (Align2::LEFT_CENTER, Align2::RIGHT_CENTER) };
    let (start_x, end_x) = if rtl { (r.right() - pad - 4.0, r.left() + pad + 4.0) } else { (r.left() + pad + 4.0, r.right() - pad - 4.0) };
    let cy = r.top() + 26.0;
    ui.painter().text(egui::pos2(start_x, cy), left_c, crate::i18n::shown("New project"), theme::section_title(), t.text);
    let keys = crate::keymap::hint(&app.prefs, "new-project");
    let keys = if keys.is_empty() { "⌘N".to_owned() } else { keys };
    ui.painter().text(egui::pos2(end_x, cy), right_c, keys, theme::caption(), t.text_3);
    let top = r.top() + 48.0;
    let row_h = 36.0;
    let mut custom = Rect::NOTHING;
    for i in 0..5 {
        let rr = Rect::from_min_size(egui::pos2(r.left() + 6.0, top + i as f32 * row_h), Vec2::new(r.width() - 12.0, row_h - 2.0));
        let (name, sub, aspect, hint) = match PRESETS.get(i) {
            Some(pr) => (pr.name, format!("{}×{}", pr.w, pr.h), Some((pr.w, pr.h)), format!("{} · {}", pr.ratio, pr.hint)),
            None => ("Custom", crate::i18n::tf("Any size".to_owned()), None, "Pick any width, height and frame rate".to_owned()),
        };
        let resp = ui.interact(rr, egui::Id::unique(("home-preset", name)), Sense::click());
        resp.widget_info(|| WidgetInfo::labeled(Role::Button, true, name));
        let st = kit::State::from_response(ui, &resp, None);
        let p = ui.painter();
        let cr = CornerRadius::same(theme::radius::CONTROL);
        if st.hover > 0.0 {
            p.rect_filled(rr, cr, theme::alpha(t.alt, st.hover));
        }
        let gx = if rtl { rr.right() - 22.0 } else { rr.left() + 22.0 };
        aspect_glyph(p, Rect::from_center_size(egui::pos2(gx, rr.center().y), Vec2::splat(24.0)), aspect, theme::mix(t.text_2, t.accent, st.hover));
        let tx = if rtl { rr.right() - 46.0 } else { rr.left() + 46.0 };
        kit::paint_text_clipped(p, egui::pos2(tx, rr.center().y), left_c, name, theme::control_medium(), t.text, rr.width() - 46.0 - 100.0);
        let ex = if rtl { rr.left() + 12.0 } else { rr.right() - 12.0 };
        kit::paint_text_clipped(p, egui::pos2(ex, rr.center().y), right_c, &sub, theme::caption(), t.text_2, 96.0);
        if st.focused {
            kit::focus_ring(p, rr, cr, &t);
        }
        if resp.clicked() {
            *act = Some(if aspect.is_some() { Act::NewPreset(i) } else { Act::ToggleCustom });
        }
        if aspect.is_none() {
            custom = rr;
        }
        resp.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(hint);
    }
    custom_popover(app, ui, custom, act);
}

/// The Custom row's width, height and frame rate.
fn custom_popover(app: &mut StudioApp, ui: &mut egui::Ui, anchor: Rect, act: &mut Option<Act>) {
    let t = theme::tokens(ui.ctx());
    let custom_id = egui::Id::unique("start-custom");
    let _ = act;
    let mut create = None;
    kit::popover(ui.ctx(), custom_id, anchor, kit::Side::BelowRight, 260.0, |ui| {
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
    }
}

/// A canvas shape in a small box: its ratio, or a dashed frame for Custom.
fn aspect_glyph(p: &egui::Painter, boxr: Rect, aspect: Option<(u32, u32)>, c: Color32) {
    match aspect {
        Some((w, h)) => {
            let s = (boxr.width() / w as f32).min(boxr.height() / h as f32);
            let fr = Rect::from_center_size(boxr.center(), Vec2::new(w as f32 * s, h as f32 * s));
            p.rect(fr, CornerRadius::same(3), theme::alpha(c, 0.14), egui::Stroke::new(1.5, c), egui::StrokeKind::Inside);
        }
        None => {
            let fr = boxr.shrink2(Vec2::new(1.0, 3.0));
            dashed_rect(p, fr, c);
            icons::paint(p, fr, Icon::Plus, 12.0, c);
        }
    }
}

// --- the cards --------------------------------------------------------------

/// What one card says.
struct CardInfo {
    label: String,
    sub: String,
    /// Up to two lines under the sub line.
    desc: Option<String>,
    badge: Option<String>,
    tip: Option<String>,
    /// Open, show in Finder and remove from the list, on hover.
    actions: bool,
}

#[derive(Default)]
struct CardOut {
    clicked: Option<usize>,
    reveal: Option<usize>,
    forget: Option<usize>,
    cols: usize,
}

/// A grid of cards: a picture in 16:9, then the name and a line about it.
#[allow(clippy::too_many_arguments)]
fn card_grid(ui: &mut egui::Ui, grid_key: &str, n: usize, sel: Option<usize>, info: &dyn Fn(usize) -> CardInfo, paint: &mut dyn FnMut(&egui::Painter, Rect, usize, f32)) -> CardOut {
    let t = theme::tokens(ui.ctx());
    let avail = ui.available_width();
    let cols = cols_for(avail, CARD_MIN_W);
    let tile_w = ((avail - GAP * (cols as f32 - 1.0)) / cols as f32).floor();
    let thumb_h = (tile_w * 9.0 / 16.0).round();
    let has_desc = n > 0 && info(0).desc.is_some();
    let label_h = if has_desc { 82.0 } else { 50.0 };
    let tile_h = thumb_h + label_h;
    let rows = n.div_ceil(cols);
    let total_h = rows as f32 * (tile_h + GAP) - GAP;
    let area = ui.allocate_exact_size(Vec2::new(avail, total_h.max(0.0)), Sense::hover()).0;
    let clip = ui.clip_rect();
    let rtl = crate::i18n::is_rtl();
    let mut out = CardOut { cols, ..Default::default() };
    let cr = CornerRadius::same(theme::radius::CARD);
    for i in 0..n {
        let (row, col) = (i / cols, i % cols);
        let col = if rtl { cols - 1 - col } else { col };
        let rect = Rect::from_min_size(area.min + Vec2::new(col as f32 * (tile_w + GAP), row as f32 * (tile_h + GAP)), Vec2::new(tile_w, tile_h));
        if !clip.intersects(rect) {
            continue;
        }
        let ti = info(i);
        let id = ui.scope_id().with(("home-card", grid_key, i));
        let resp = ui.interact(rect, id, Sense::click());
        let selected = sel == Some(i);
        resp.widget_info(|| WidgetInfo::selected(Role::Button, true, selected, &ti.label));
        let st = kit::State::from_response(ui, &resp, None);
        let over = ui.rect_contains_pointer(rect) && clip.contains(ui.ctx().pointer_hover_pos().unwrap_or(rect.center()));
        let thumb = Rect::from_min_size(rect.min, Vec2::new(tile_w, thumb_h));
        let p = ui.painter().with_clip_rect(clip);
        p.rect_filled(thumb, cr, t.sunken);
        paint(&p.with_clip_rect(thumb.intersect(clip)), thumb, i, st.hover);
        kit::round_corners(&p, thumb, theme::radius::CARD as f32, t.ground);
        if let Some(b) = &ti.badge {
            let g = p.layout_no_wrap(crate::i18n::shown(b).into_owned(), theme::caption(), Color32::WHITE);
            let br = Rect::from_min_size(thumb.right_bottom() - g.size() - Vec2::new(14.0, 12.0), g.size() + Vec2::new(10.0, 4.0));
            p.rect_filled(br, CornerRadius::same(4), Color32::from_black_alpha(170));
            p.galley(br.center() - g.size() / 2.0, g, Color32::WHITE);
        }
        if selected {
            p.rect_stroke(thumb, cr, egui::Stroke::new(2.0, t.accent), egui::StrokeKind::Inside);
        } else if st.hover > 0.0 {
            p.rect_stroke(thumb, cr, egui::Stroke::new(1.0, theme::alpha(t.accent, st.hover)), egui::StrokeKind::Inside);
        }
        if st.focused {
            kit::focus_ring(&p, thumb, cr, &t);
        }
        let color = if selected { t.text } else { theme::mix(t.text, t.text, st.hover) };
        let lx = if rtl { rect.right() - 2.0 } else { rect.left() + 2.0 };
        let al = if rtl { Align2::RIGHT_TOP } else { Align2::LEFT_TOP };
        let cut = kit::paint_text_clipped(&p, egui::pos2(lx, thumb.bottom() + 9.0), al, &ti.label, theme::body_medium(), color, tile_w - 4.0);
        kit::paint_text_clipped(&p, egui::pos2(lx, thumb.bottom() + 28.0), al, &ti.sub, theme::caption(), t.text_2, tile_w - 4.0);
        if let Some(d) = &ti.desc {
            let mut job = egui::text::LayoutJob::simple(crate::i18n::shown(d).into_owned(), theme::caption(), t.text_3, tile_w - 4.0);
            job.wrap.max_rows = 2;
            job.wrap.break_anywhere = false;
            job.wrap.overflow_character = Some('…');
            let g = p.layout_job(job);
            p.galley(egui::pos2(if rtl { lx - g.size().x } else { lx }, thumb.bottom() + 47.0), g, t.text_3);
        }
        // Hover actions: open, show in Finder, remove from the list.
        let mut action_hit = false;
        if ti.actions && (over || selected) {
            let defs = [(Icon::Play, "Open"), (Icon::FolderOpen, "Show in Finder"), (Icon::X, "Remove from list")];
            for (k, (icon, name)) in defs.into_iter().enumerate() {
                let x = if rtl { thumb.left() + 6.0 + k as f32 * 28.0 } else { thumb.right() - 6.0 - 24.0 - (2 - k) as f32 * 28.0 };
                let br = Rect::from_min_size(egui::pos2(x, thumb.top() + 6.0), Vec2::splat(24.0));
                let r = ui.interact(br, id.with(("action", k)), Sense::click());
                r.widget_info(|| WidgetInfo::labeled(Role::Button, true, name));
                let hv = r.hovered();
                p.circle_filled(br.center(), 12.0, if hv { t.accent_fill } else { Color32::from_black_alpha(165) });
                icons::paint(&p, br, icon, 14.0, if hv { t.on_accent() } else { Color32::WHITE });
                if r.clicked() {
                    action_hit = true;
                    match k {
                        0 => out.clicked = Some(i),
                        1 => out.reveal = Some(i),
                        _ => out.forget = Some(i),
                    }
                }
                kit::tip(r, name, None);
            }
        }
        if resp.clicked() && !action_hit {
            out.clicked = Some(i);
        }
        match (&ti.tip, cut) {
            (Some(tip), _) if !tip.is_empty() => {
                let (label, tip) = (ti.label.clone(), tip.clone());
                resp.on_hover_ui(move |ui| {
                    ui.set_max_width(260.0);
                    ui.label(egui::RichText::new(label).font(theme::body_medium()));
                    ui.label(egui::RichText::new(tip).font(theme::body()).color(theme::tokens(ui.ctx()).text_2));
                });
            }
            (_, true) => {
                resp.on_hover_text(&ti.label);
            }
            _ => {}
        }
    }
    out
}

/// "1920×1080 · 16:9 · 2:14", without the length when there is none.
fn project_meta(rec: &files::Recent) -> String {
    let mut s = format!("{}×{} · {}", rec.width, rec.height, ratio_of(rec.width, rec.height));
    if rec.length > 0 {
        s.push_str(" · ");
        s.push_str(&view::format_duration(rec.length));
    }
    s
}

/// "16:9", "9:16"; a frame that does not reduce to small numbers is "2.39:1".
fn ratio_of(w: u32, h: u32) -> String {
    fn gcd(a: u32, b: u32) -> u32 {
        if b == 0 { a } else { gcd(b, a % b) }
    }
    let g = gcd(w.max(1), h.max(1)).max(1);
    let (a, b) = (w / g, h / g);
    if a <= 21 && b <= 21 {
        format!("{a}:{b}")
    } else if w >= h {
        format!("{:.2}:1", w as f32 / h.max(1) as f32)
    } else {
        format!("1:{:.2}", h as f32 / w.max(1) as f32)
    }
}

/// Arrow keys move over the project cards and Enter opens the one chosen.
fn move_selection(ui: &egui::Ui, sel: &mut Option<usize>, n: usize, cols: usize) -> Option<usize> {
    if n == 0 || ui.ctx().egui_wants_keyboard_input() {
        *sel = sel.filter(|&i| i < n);
        return None;
    }
    let none = egui::Modifiers::NONE;
    let key = |k| ui.input_mut(|i| i.consume_key(none, k));
    let cols = cols.max(1);
    let mut next = *sel;
    let step = |cur: Option<usize>, d: isize| match cur {
        None => Some(0),
        Some(i) => Some((i as isize + d).clamp(0, n as isize - 1) as usize),
    };
    if key(egui::Key::ArrowRight) {
        next = step(next, 1);
    }
    if key(egui::Key::ArrowLeft) {
        next = step(next, -1);
    }
    if key(egui::Key::ArrowDown) {
        next = step(next, cols as isize);
    }
    if key(egui::Key::ArrowUp) {
        next = step(next, -(cols as isize));
    }
    *sel = next.filter(|&i| i < n);
    let nothing_focused = ui.ctx().memory(|m| m.focused().is_none());
    if nothing_focused && sel.is_some() && key(egui::Key::Enter) { *sel } else { None }
}

fn recents_grid(app: &mut StudioApp, ui: &mut egui::Ui, all: &[files::Recent], shown: usize, act: &mut Option<Act>) {
    let recents = &all[..shown.min(all.len())];
    // Each project shows a picture from its own media, looked up a few
    // projects per frame so a long list never stalls a frame.
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
    let seeds: Vec<u32> = recents.iter().map(|r| app.start.covers.get(&r.path).cloned().flatten().map(|(p, secs)| app.thumbs.file_seed(&p, secs)).unwrap_or_else(|| files::seed_of(&r.path))).collect();
    if let Some(i) = move_selection(ui, &mut app.start.sel, recents.len(), app.start.cols) {
        *act = Some(Act::Open(recents[i].path.clone()));
    }
    let thumbs = &mut app.thumbs;
    let out = card_grid(
        ui,
        "recents",
        recents.len(),
        app.start.sel,
        &|i| {
            let r = &recents[i];
            CardInfo {
                label: r.name.clone(),
                sub: crate::i18n::tf(format!("{} · {}×{}", files::edited_label(r.edited), r.width, r.height)),
                desc: None,
                badge: (r.length > 0).then(|| view::format_duration(r.length)),
                // Where it lives, on hover (same names stay apart).
                tip: Some(studio_paths::tilde(&r.path)),
                actions: true,
            }
        },
        &mut |p, r, i, hover| {
            let rec = &recents[i];
            letterboxed(p, thumbs, r, (rec.width.max(1), rec.height.max(1)), seeds[i], hover);
        },
    );
    app.start.cols = out.cols;
    if let Some(i) = out.clicked {
        *act = Some(Act::Open(recents[i].path.clone()));
    }
    if let Some(i) = out.reveal {
        *act = Some(Act::Reveal(recents[i].path.clone()));
    }
    if let Some(i) = out.forget {
        *act = Some(Act::Forget(recents[i].path.clone()));
    }
}

/// Templates as a miniature of the timeline each one sets up.
fn templates_grid(app: &mut StudioApp, ui: &mut egui::Ui, hits: &Hits, shown: usize, act: &mut Option<Act>) {
    let t = theme::tokens(ui.ctx());
    let lang = crate::i18n::lang();
    let idx: Vec<usize> = hits.tpl_hits.iter().copied().take(shown).collect();
    let previews = &app.start.previews;
    let out = card_grid(
        ui,
        "templates",
        idx.len(),
        None,
        &|i| {
            let tpl = &ALL[idx[i]];
            CardInfo { label: hits.tpl_names[idx[i]].clone(), sub: format!("{} · {}×{} · {} fps", tpl.ratio(), tpl.width, tpl.height, tpl.fps), desc: None, badge: None, tip: Some(tpl.blurb(lang)), actions: false }
        },
        &mut |p, r, i, hover| {
            let k = idx[i];
            let tpl = &ALL[k];
            let ink = |c: Color32| if t.dark { c } else { theme::mix(c, Color32::BLACK, 0.22) };
            p.rect_filled(r, 0, theme::mix(t.sunken, tpl.tint.0, 0.10 + 0.05 * hover));
            // A soft glow from the corner in the template's own colour.
            p.circle_filled(r.left_top() + Vec2::new(r.width() * 0.15, 0.0), r.width() * 0.5, theme::alpha(tpl.tint.0, 0.07 + 0.04 * hover));
            let area = r.shrink2(Vec2::new(14.0, 12.0));
            let chip = Rect::from_min_size(area.min, Vec2::splat(22.0));
            p.rect_filled(chip, CornerRadius::same(theme::radius::CONTROL), theme::alpha(Color32::BLACK, if t.dark { 0.3 } else { 0.08 }));
            icons::paint(p, chip, tpl.icon, 14.0, t.text);
            let Some(pv) = previews.get(k) else { return };
            let body = Rect::from_min_max(egui::pos2(area.left(), area.top() + 34.0), egui::pos2(area.right(), area.bottom() - 2.0));
            let n = pv.lanes.len().max(1) as f32;
            let lane_h = ((body.height() - (n - 1.0) * 3.0) / n).clamp(2.0, 9.0);
            let top = body.bottom() - n * lane_h - (n - 1.0) * 3.0;
            for &at in &pv.marks {
                let x = body.left() + at.clamp(0.0, 1.0) * body.width();
                p.rect_filled(Rect::from_center_size(egui::pos2(x, top - 6.0), Vec2::new(2.0, 6.0)), 1, ink(t.marker));
            }
            for (li, (kind, colour)) in pv.lanes.iter().enumerate() {
                let y = top + li as f32 * (lane_h + 3.0);
                // Clip cuts from a small hash of the template and lane, so
                // each card keeps its own steady rhythm.
                let mut h = (k as u32 + 1).wrapping_mul(2654435761) ^ (li as u32 + 7).wrapping_mul(40503);
                let mut x = body.left() + if *kind == view::LaneKind::Visual { ((h >> 3) % 40) as f32 } else { 0.0 };
                while x < body.right() - 6.0 {
                    h = h.wrapping_mul(1664525).wrapping_add(1013904223);
                    let w = (18 + (h >> 24) % 60) as f32;
                    let x1 = (x + w).min(body.right());
                    p.rect_filled(Rect::from_min_max(egui::pos2(x, y), egui::pos2(x1, y + lane_h)), 2, ink(*colour));
                    x = x1 + if *kind == view::LaneKind::Visual { (4 + (h >> 20) % 30) as f32 } else { 2.0 };
                }
            }
        },
    );
    if let Some(i) = out.clicked {
        *act = Some(Act::Template(idx[i]));
    }
}

/// Lessons, each over a frame of the sample footage it practises on.
fn guided_grid(app: &mut StudioApp, ui: &mut egui::Ui, shown: usize, act: &mut Option<Act>) {
    let t = theme::tokens(ui.ctx());
    let lang = crate::i18n::lang();
    let names: Vec<String> = GUIDED.iter().map(|(g, _)| studio_i18n::tr_in(lang, &format!("guide-{g}-name"), &[])).collect();
    let blurbs: Vec<String> = GUIDED.iter().map(|(g, _)| studio_i18n::tr_in(lang, &format!("guide-{g}-blurb"), &[])).collect();
    let thumbs = &mut app.thumbs;
    let out = card_grid(
        ui,
        "guided",
        shown.min(GUIDED.len()),
        None,
        &|i| CardInfo { label: names[i].clone(), sub: crate::i18n::tf("Practice on sample footage".to_owned()), desc: Some(blurbs[i].clone()), badge: None, tip: None, actions: false },
        &mut |p, r, i, hover| {
            let seed = view::RECENTS[i % view::RECENTS.len()].seed;
            letterboxed(p, thumbs, r, (16, 9), seed, hover);
            // A scrim, then the lesson's icon in an accent disc.
            p.rect_filled(r, 0, theme::alpha(Color32::BLACK, 0.35 - 0.12 * hover));
            let c = r.center();
            p.circle_filled(c, 22.0, theme::mix(t.accent_fill, t.accent, hover));
            icons::paint(p, Rect::from_center_size(c, Vec2::splat(22.0)), GUIDED[i].1, 22.0, t.on_accent());
        },
    );
    if let Some(i) = out.clicked {
        *act = Some(Act::Guided(i));
    }
}

fn samples_grid(app: &mut StudioApp, ui: &mut egui::Ui, samples: &[&'static view::Recent], act: &mut Option<Act>) {
    heading(ui, "Samples", None);
    let thumbs = &mut app.thumbs;
    let out = card_grid(
        ui,
        "samples",
        samples.len(),
        None,
        &|i| CardInfo { label: samples[i].name.to_owned(), sub: samples[i].edited.to_owned(), desc: None, badge: Some(view::format_duration(samples[i].seconds as i64 * view::SEC)), tip: None, actions: false },
        &mut |p, r, i, hover| letterboxed(p, thumbs, r, samples[i].aspect, samples[i].seed, hover),
    );
    if let Some(i) = out.clicked {
        *act = Some(Act::Sample(samples[i]));
    }
}

// --- the rail ---------------------------------------------------------------

/// A row of the rail (or a pill of the strip): an icon, a name and, for the
/// actions, a shortcut.
fn nav_item(ui: &mut egui::Ui, r: Rect, icon: Icon, label: &str, hint: &str, selected: bool, compact: bool) -> egui::Response {
    let t = theme::tokens(ui.ctx());
    let resp = ui.interact(r, egui::Id::unique(("home-nav", label)), Sense::click());
    resp.widget_info(|| WidgetInfo::selected(Role::Button, true, selected, label));
    let st = kit::State::from_response(ui, &resp, None);
    let p = ui.painter();
    let cr = CornerRadius::same(theme::radius::CONTROL);
    if selected {
        p.rect_filled(r, cr, t.accent_soft);
    } else if st.hover > 0.0 {
        p.rect_filled(r, cr, theme::alpha(t.alt, st.hover));
    }
    let rtl = crate::i18n::is_rtl();
    let ink = if selected { t.text } else { theme::mix(t.text_2, t.text, st.hover) };
    let icon_c = if selected { t.accent } else { theme::mix(t.text_2, t.accent, st.hover) };
    let (ix, tx, align) = if rtl { (r.right() - 18.0, r.right() - 36.0, Align2::RIGHT_CENTER) } else { (r.left() + 18.0, r.left() + 36.0, Align2::LEFT_CENTER) };
    icons::paint(p, Rect::from_center_size(egui::pos2(ix, r.center().y), Vec2::splat(16.0)), icon, 16.0, icon_c);
    let hint_w = if hint.is_empty() || compact { 0.0 } else { 34.0 };
    let cut = kit::paint_text_clipped(p, egui::pos2(tx, r.center().y), align, label, theme::control_medium(), ink, r.width() - 40.0 - hint_w);
    if hint_w > 0.0 {
        let (hx, ha) = if rtl { (r.left() + 10.0, Align2::LEFT_CENTER) } else { (r.right() - 10.0, Align2::RIGHT_CENTER) };
        p.text(egui::pos2(hx, r.center().y), ha, hint, theme::caption(), t.text_3);
    }
    if st.focused {
        kit::focus_ring(p, r, cr, &t);
    }
    kit::tip_if_cut(resp, cut, label).on_hover_cursor(egui::CursorIcon::PointingHand)
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
    use super::Tab;
    use crate::ui_tests::Harness;
    use eframe::egui::{self, Event, PointerButton};

    fn recent(i: usize) -> crate::files::Recent {
        crate::files::Recent { path: format!("/tmp/zq-test/P{i}").into(), name: format!("Zq list {i}"), edited: 0, length: 134_000_000, width: 1920, height: 1080 }
    }

    fn click(h: &mut Harness, at: egui::Pos2) {
        let press = |down| Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: down, modifiers: egui::Modifiers::NONE };
        h.with_events(vec![Event::PointerMoved(at)]);
        h.with_events(vec![press(true)]);
        h.with_events(vec![press(false)]);
    }

    fn window(h: &Harness) -> egui::Rect {
        egui::Rect::from_min_size(egui::pos2(0.0, 0.0), h.size)
    }

    /// Typing in the search field narrows the projects and samples.
    #[test]
    fn typing_in_search_filters_the_projects() {
        let mut h = Harness::new(&[]);
        h.app.prefs.recents = vec![crate::files::Recent { path: "/tmp/zq-test/Zq project".into(), name: "Zq project".into(), edited: 0, length: 0, width: 1920, height: 1080 }];
        h.frames(3);
        let before = h.labels();
        assert!(before.iter().any(|l| l.contains("Zq project")), "a recent project is listed first: {before:?}");
        let at = super::layout(window(&h)).search.center();
        click(&mut h, at);
        h.with_events(vec![Event::Text("zzqx".into())]);
        h.frames(2);
        assert_eq!(h.app.start.search, "zzqx", "the typed text reaches the field");
        let after = h.labels();
        assert!(!after.iter().any(|l| l.contains("Zq project")), "projects are filtered out: {after:?}");
        assert!(after.iter().any(|l| l == "Clear search"), "the empty search offers to clear it: {after:?}");
    }

    /// Recent projects come before the templates, then the lessons.
    #[test]
    fn recent_projects_come_first() {
        let mut h = Harness::new(&[]);
        h.app.prefs.recents = vec![recent(0)];
        h.frames(3);
        let nodes = &h.last.as_ref().unwrap().platform_output.accesskit_update.as_ref().unwrap().nodes;
        let top = |name: &str| nodes.iter().filter(|(_, n)| n.label().or(n.value()) == Some(name)).filter_map(|(_, n)| n.bounds()).map(|b| b.y0).fold(f64::INFINITY, f64::min);
        let (recent, templates, learn) = (top("Recent projects"), top("Start from a template"), top("Learn by doing"));
        assert!(recent.is_finite() && templates.is_finite() && learn.is_finite(), "all three headings are drawn");
        assert!(recent < templates && templates < learn, "recent ({recent}) above templates ({templates}) above lessons ({learn})");
    }

    /// The Projects page shows the latest projects as covers, and the rest
    /// behind "Show all"; search still finds every one.
    #[test]
    fn a_long_history_shows_the_latest_and_the_rest_on_request() {
        let mut h = Harness::new(&[]);
        h.app.prefs.recents = (0..20).map(recent).collect();
        h.app.start.tab = Tab::Projects;
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

    /// Every way in is on the screen, with a name for a screen reader.
    #[test]
    fn every_entry_point_is_present() {
        for (w, hgt) in [(1000.0, 700.0), (1800.0, 1000.0), (700.0, 800.0)] {
            let mut h = Harness::new(&[]).sized(w, hgt);
            h.app.prefs.recents = vec![recent(0), recent(1)];
            h.frames(3);
            let l = h.labels();
            for want in ["Home", "Projects", "Templates", "Learn", "Open project", "Import timeline", "Demo project", "Settings", "Landscape", "Vertical", "Square", "Portrait", "Custom", "Continue", "Show in Finder", "Zq list 0", "Start from a template", "Learn by doing"] {
                assert!(l.iter().any(|s| s == want || s.starts_with(want)), "{w}: {want} is on Home: {l:?}");
            }
            assert!(h.unlabelled().is_empty(), "{w}: every control is named: {:?}", h.unlabelled());
        }
    }

    /// First run: a welcome with the two ways to begin.
    #[test]
    fn first_run_says_where_to_begin() {
        let mut h = Harness::new(&[]);
        h.app.prefs.recents.clear();
        h.frames(3);
        let l = h.labels();
        for want in ["Welcome to Zornov", "New landscape project", "Try the demo project"] {
            assert!(l.iter().any(|s| s == want), "{want}: {l:?}");
        }
        assert!(!l.iter().any(|s| s == "Recent projects"), "no empty list of recent projects: {l:?}");
    }

    /// Nothing is drawn over anything else, with and without projects, from
    /// a narrow window to a wide one, on every page.
    #[test]
    fn no_overlap_at_any_width() {
        for (w, hgt) in [(700.0, 800.0), (1000.0, 700.0), (1440.0, 900.0), (1800.0, 1000.0)] {
            for tab in Tab::ALL {
                for n in [0, 9] {
                    let mut h = Harness::new(&[]).sized(w, hgt);
                    h.app.prefs.recents = (0..n).map(recent).collect();
                    h.app.start.tab = tab;
                    h.frames(3);
                    let o = h.overlaps();
                    assert!(o.is_empty(), "{w}px {tab:?} with {n} projects: {o:?}");
                    if let Ok(dir) = std::env::var("HOME_SHOTS") {
                        h.shot(format!("{dir}/home-{w}-{tab:?}-{n}.png"));
                    }
                }
            }
        }
    }

    /// The rail switches pages.
    #[test]
    fn the_rail_switches_pages() {
        let mut h = Harness::new(&[]);
        h.app.prefs.recents = vec![recent(0)];
        h.frames(3);
        let lay = super::layout(window(&h));
        click(&mut h, lay.tabs[2].center());
        h.frames(2);
        assert_eq!(h.app.start.tab, Tab::Templates);
        let l = h.labels();
        assert!(l.iter().any(|s| s == "Start from a template"), "{l:?}");
        assert!(!l.iter().any(|s| s == "Recent projects"), "{l:?}");
        click(&mut h, lay.tabs[3].center());
        h.frames(2);
        assert_eq!(h.app.start.tab, Tab::Learn);
        assert!(h.labels().iter().any(|s| s == "Samples"));
    }

    /// Arrow keys move over the project cards.
    #[test]
    fn arrow_keys_move_over_the_projects() {
        let mut h = Harness::new(&[]);
        h.app.prefs.recents = (0..6).map(recent).collect();
        h.app.start.tab = Tab::Projects;
        h.frames(3);
        assert_eq!(h.app.start.sel, None);
        h.key(egui::Key::ArrowRight, egui::Modifiers::NONE);
        h.frames(1);
        assert_eq!(h.app.start.sel, Some(0));
        h.key(egui::Key::ArrowRight, egui::Modifiers::NONE);
        assert_eq!(h.app.start.sel, Some(1));
        h.key(egui::Key::ArrowLeft, egui::Modifiers::NONE);
        assert_eq!(h.app.start.sel, Some(0));
        let cols = h.app.start.cols;
        assert!(cols >= 1);
        h.key(egui::Key::ArrowDown, egui::Modifiers::NONE);
        assert_eq!(h.app.start.sel, Some(cols.min(5)));
    }

    /// Removing a project from the list leaves the project, says so, and
    /// keeps it off the list this session.
    #[test]
    fn removing_a_project_takes_it_off_the_list() {
        let mut h = Harness::new(&[]);
        h.app.prefs.recents = (0..3).map(recent).collect();
        h.frames(2);
        let ctx = h.ctx.clone();
        super::forget(&mut h.app, &ctx, std::path::Path::new("/tmp/zq-test/P1"));
        h.frames(3);
        assert_eq!(h.app.prefs.recents.len(), 2);
        let l = h.labels();
        assert!(!l.iter().any(|s| s == "Zq list 1"), "{l:?}");
        assert!(h.app.start.hidden.contains(std::path::Path::new("/tmp/zq-test/P1")));
    }

    #[test]
    fn ratios_read_as_people_say_them() {
        assert_eq!(super::ratio_of(1920, 1080), "16:9");
        assert_eq!(super::ratio_of(1080, 1920), "9:16");
        assert_eq!(super::ratio_of(1080, 1350), "4:5");
        assert_eq!(super::ratio_of(1920, 804), "2.39:1");
    }
}
