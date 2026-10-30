// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The command palette (⌥⌘P): one search box over everything the app can do.
//! It lists the registry (`register`, called from each feature's own file) and
//! every row of the menus, read through the same provider the menu bar uses, so
//! a menu row picked here is queued as the same `Do` and runs the way it does
//! from the menu bar. A command that cannot run now stays in the list, greyed,
//! with the reason ("Select a clip first").
//!
//! Drawn once a frame from `macmenu::frame`, so it needs nothing in `app.rs`.
//! Its keys are read before the window's own: Esc closes, ↑ ↓ move, Enter runs,
//! and nothing reaches the editor while it is open.

use crate::app::{Overlay, StudioApp};
use crate::dock::Panel;
use crate::editor::Action;
use crate::icons::Icon;
use crate::macmenu::{self, Do, Row};
use crate::theme;
use eframe::egui::{self, Align2, Id, Key, Modifiers, Order, Sense, Vec2};
use std::sync::{Arc, Mutex, Once};

/// Why a command cannot run now, or that it can.
pub type Enabled = fn(&StudioApp) -> Result<(), &'static str>;
/// What a command does when picked.
pub type Run = Arc<dyn Fn(&mut StudioApp, &egui::Context) + Send + Sync>;

/// One thing the palette can run.
#[derive(Clone)]
pub struct Command {
    pub id: String,
    pub label: String,
    /// Other words that should find it ("preferences" for Settings).
    pub keywords: String,
    /// The keymap action whose keys are shown on the right, if it has one.
    pub keys: Option<&'static str>,
    pub enabled: Enabled,
    pub run: Run,
}

impl Command {
    pub fn new(id: &str, label: &str, keywords: &str, enabled: Enabled, run: impl Fn(&mut StudioApp, &egui::Context) + Send + Sync + 'static) -> Self {
        Command { id: id.to_owned(), label: label.to_owned(), keywords: keywords.to_owned(), keys: None, enabled, run: Arc::new(run) }
    }

    /// Shows the keys of the keymap action `id` beside the command.
    pub fn keys(mut self, id: &'static str) -> Self {
        self.keys = Some(id);
        self
    }
}

static REGISTRY: Mutex<Vec<Command>> = Mutex::new(Vec::new());

/// Adds a command (a command with the same id is replaced). Call it from the
/// file that owns the feature; it shows up in the palette on the next frame.
pub fn register(cmd: Command) {
    let mut all = REGISTRY.lock().unwrap_or_else(|e| e.into_inner());
    match all.iter_mut().find(|c| c.id == cmd.id) {
        Some(old) => *old = cmd,
        None => all.push(cmd),
    }
}

/// Always runnable.
pub fn always(_: &StudioApp) -> Result<(), &'static str> {
    Ok(())
}

/// Runnable on the screens that have the editor's panels and layout.
pub fn in_editor(app: &StudioApp) -> Result<(), &'static str> {
    use crate::cli::Screen;
    if matches!(app.screen, Screen::Editor | Screen::Export | Screen::Settings) {
        Ok(())
    } else {
        Err("Open a project first")
    }
}

/// The commands the palette brings itself: the workspaces, the panels, Settings,
/// Help search and Reset layout. Each one queues the menu row's own `Do`.
fn register_builtins() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        register(Command::new("settings", "Open Settings", "preferences options", always, |_, _| macmenu::push(Do::Act(|| Some(Action::Settings)))).keys("settings"));
        register(Command::new("help", "Search help", "docs manual how to", always, |_, _| macmenu::push(Do::Act(|| Some(Action::Help)))));
        register(Command::new("layout-reset", "Reset layout", "panels default arrange", in_editor, |_, _| macmenu::push(Do::ResetLayout)));
        for w in crate::workspaces::builtin() {
            let name = w.name.clone();
            register(Command::new(&format!("workspace:{name}"), &format!("Open {name} workspace"), "layout switch", in_editor, move |_, _| macmenu::push(Do::Space(name.clone()))));
        }
        for p in Panel::ALL.into_iter().filter(|p| p.closable()) {
            register(Command::new(&format!("panel:{}", p.key()), &format!("Show {} panel", p.name()), "open window toggle", in_editor, move |_, _| macmenu::push(Do::Panel(p))));
        }
    });
}

/// Menu rows the registry already lists (with better names): left out of the
/// menu part of the list so each thing appears once.
fn covered(what: &Do) -> bool {
    match what {
        Do::Space(_) | Do::Panel(_) | Do::ResetLayout => true,
        Do::Act(f) => matches!(f(), Some(Action::Settings | Action::Help)),
        _ => false,
    }
}

// --- the list ---------------------------------------------------------------------------------

enum Act {
    Menu(Do),
    Cmd(Run),
}

/// One row of the palette.
pub struct Entry {
    pub id: String,
    /// What is shown ("Save").
    pub label: String,
    /// Where it lives ("File"), empty for registry commands.
    pub menu: String,
    /// Search text: menu title, label and keywords.
    haystack: String,
    pub keys: String,
    pub state: Result<(), String>,
    act: Option<Act>,
}

impl Entry {
    /// "File ▸ Save", or just the label.
    pub fn title(&self) -> String {
        if self.menu.is_empty() {
            self.label.clone()
        } else {
            format!("{} ▸ {}", self.menu, self.label)
        }
    }

    pub fn enabled(&self) -> bool {
        self.state.is_ok()
    }

    #[cfg(test)]
    fn plain(id: &str, label: &str, menu: &str, keywords: &str, enabled: bool) -> Entry {
        let haystack = format!("{menu} {label} {keywords}").to_lowercase();
        Entry { id: id.into(), label: label.into(), menu: menu.into(), haystack, keys: String::new(), state: if enabled { Ok(()) } else { Err("No".into()) }, act: None }
    }
}

/// Every command now: the registry, then the menu rows.
fn entries(app: &StudioApp, ctx: &egui::Context) -> Vec<Entry> {
    register_builtins();
    let mut out = Vec::new();
    let reg = REGISTRY.lock().unwrap_or_else(|e| e.into_inner()).clone();
    for c in reg {
        let keys = match (c.keys, c.id.strip_prefix("panel:")) {
            (Some(id), _) => crate::keymap::hint(&app.prefs, id),
            (None, Some(p)) => crate::keymap::hint(&app.prefs, &format!("panel-{p}")),
            _ => String::new(),
        };
        let haystack = format!("{} {}", c.label, c.keywords).to_lowercase();
        let state = (c.enabled)(app).map_err(str::to_owned);
        out.push(Entry { id: c.id, label: c.label, menu: String::new(), haystack, keys, state, act: Some(Act::Cmd(c.run)) });
    }
    for m in macmenu::menus(app, ctx) {
        for r in &m.rows {
            let Row::Item(i) = r else { continue };
            if covered(&i.what) {
                continue;
            }
            let haystack = format!("{} ▸ {}", m.title, i.label).to_lowercase();
            let state = macmenu::why_not(app, i).map_err(str::to_owned);
            out.push(Entry { id: format!("menu:{}:{}", m.title, i.label), label: i.label.clone(), menu: m.title.to_owned(), haystack, keys: i.shortcut.clone(), state, act: Some(Act::Menu(i.what.clone())) });
        }
    }
    out
}

// --- matching and ranking ---------------------------------------------------------------------

/// How well one word matches: a run of it in the text beats letters in order.
fn word_score(word: &str, hay: &str) -> Option<i32> {
    if let Some(at) = hay.find(word) {
        let starts_word = hay[..at].chars().next_back().is_none_or(|c| !c.is_alphanumeric());
        return Some(200 + if starts_word { 60 } else { 0 } - at.min(100) as i32);
    }
    let mut chars = hay.chars().enumerate();
    let mut last: Option<usize> = None;
    let mut score = 0;
    for c in word.chars() {
        let (i, _) = chars.find(|(_, h)| *h == c)?;
        score += 10 + if last.is_some_and(|l| i == l + 1) { 8 } else { 0 };
        last = Some(i);
    }
    Some(score)
}

/// The score of a query against a lower-cased haystack: every word of the query
/// must match; None when one does not.
pub fn score(query: &str, hay: &str) -> Option<i32> {
    let q = query.to_lowercase();
    q.split_whitespace().try_fold(0, |sum, w| Some(sum + word_score(w, hay)?))
}

/// The entries that match `query`, best first (indexes into `entries`). Those
/// that can run come before those that cannot; recently used ones lead among
/// equals, and with no query the order is recent first, then as listed.
pub fn rank(entries: &[Entry], query: &str, recent: &[String]) -> Vec<usize> {
    let mut hits: Vec<(usize, i32)> = entries
        .iter()
        .enumerate()
        .filter_map(|(i, e)| {
            let s = score(query, &e.haystack)?;
            let bonus = recent.iter().position(|r| *r == e.id).map_or(0, |p| 150 - 10 * p.min(10) as i32);
            Some((i, s + bonus))
        })
        .collect();
    hits.sort_by(|a, b| entries[b.0].enabled().cmp(&entries[a.0].enabled()).then(b.1.cmp(&a.1)));
    hits.into_iter().map(|(i, _)| i).collect()
}

// --- state and window -------------------------------------------------------------------------

#[derive(Clone, Default)]
struct State {
    open: bool,
    query: String,
    sel: usize,
    /// The selection moved by key: scroll to it.
    moved: bool,
    recent: Vec<String>,
}

fn state_id() -> Id {
    Id::new("palette-state")
}

fn state(ctx: &egui::Context) -> State {
    ctx.data(|d| d.get_temp(state_id())).unwrap_or_default()
}

fn set(ctx: &egui::Context, s: State) {
    ctx.data_mut(|d| d.insert_temp(state_id(), s));
}

/// Opens the palette with an empty search.
pub fn open(ctx: &egui::Context) {
    let s = state(ctx);
    set(ctx, State { open: true, query: String::new(), sel: 0, moved: false, recent: s.recent });
    ctx.request_repaint();
}

#[allow(dead_code)] // the top bar and tests ask
pub fn is_open(ctx: &egui::Context) -> bool {
    state(ctx).open
}

fn close(ctx: &egui::Context) {
    let mut s = state(ctx);
    s.open = false;
    set(ctx, s);
}

const MAX_RECENT: usize = 8;
const ROWS_SHOWN: usize = 10;
const ROW_H: f32 = 30.0;

/// Once a frame, after the menu bar's picks: opens, closes and draws the palette.
pub fn frame(app: &mut StudioApp, ctx: &egui::Context) {
    let mut st = state(ctx);
    let busy = app.overlay != Overlay::None || crate::onboarding::tour_running(app);
    if !st.open {
        // Never while another field has the keys: a person typing is not opening anything.
        if !busy && !ctx.text_edit_focused() && crate::keymap::hit(ctx, &app.prefs, "palette") {
            open(ctx);
        }
        return;
    }
    if busy || crate::keymap::hit(ctx, &app.prefs, "palette") {
        close(ctx);
        return;
    }
    // The palette's keys come first, and nothing else sees them.
    let (esc, down, up, enter) = ctx.input_mut(|i| {
        let none = Modifiers::NONE;
        (i.consume_key(none, Key::Escape), i.consume_key(none, Key::ArrowDown), i.consume_key(none, Key::ArrowUp), i.consume_key(none, Key::Enter))
    });
    if esc {
        close(ctx);
        return;
    }
    crate::keymap::claim(ctx, crate::keymap::KeyOwner::Text);

    let all = entries(app, ctx);
    let before = st.query.clone();
    let shown = rank(&all, &st.query, &st.recent);
    let last = shown.len().saturating_sub(1);
    if down {
        st.sel = (st.sel + 1).min(last);
        st.moved = true;
    }
    if up {
        st.sel = st.sel.saturating_sub(1);
        st.moved = true;
    }
    st.sel = st.sel.min(last);

    let mut pick: Option<usize> = None;
    if enter {
        pick = shown.get(st.sel).copied();
    }
    let t = theme::tokens(ctx);
    let screen = ctx.content_rect();
    let width = 560.0_f32.min(screen.width() - 32.0);
    let pos = egui::pos2(screen.center().x - width / 2.0, screen.top() + (screen.height() * 0.14).max(24.0));
    let area = egui::Area::new(Id::new("palette-window")).order(Order::Foreground).fixed_pos(pos).show(ctx, |ui| {
        let (frame_rect, _) = ui.allocate_exact_size(Vec2::new(width, ROW_H + 2.0 * theme::space::SM + ROWS_SHOWN.min(shown.len().max(1)) as f32 * ROW_H + theme::space::SM), Sense::hover());
        let p = ui.painter();
        p.add(theme::popover_shadow().as_shape(frame_rect, egui::CornerRadius::same(theme::radius::PANEL)));
        p.rect(frame_rect, egui::CornerRadius::same(theme::radius::PANEL), t.panel, egui::Stroke::new(1.0, t.line), egui::StrokeKind::Inside);
        let inner = frame_rect.shrink(theme::space::SM);
        ui.scope_builder(egui::UiBuilder::new().max_rect(inner), |ui| {
            let r = crate::kit::text_field(ui, &mut st.query, "Search commands", Some(Icon::MagnifyingGlass), inner.width(), None);
            if !r.has_focus() {
                r.request_focus();
            }
            ui.add_space(theme::space::SM);
            list(ui, &all, &shown, &mut st, &mut pick, inner.width());
        });
    });
    // A click outside closes it.
    let outside = ctx.input(|i| i.pointer.any_pressed() && i.pointer.interact_pos().is_some_and(|p| !area.response.rect.contains(p)));
    if st.query != before {
        st.sel = 0;
        st.moved = true;
    }
    if outside {
        st.open = false;
    }
    if let Some(i) = pick {
        let e = &all[i];
        if e.enabled() {
            st.open = false;
            st.recent.retain(|r| *r != e.id);
            st.recent.insert(0, e.id.clone());
            st.recent.truncate(MAX_RECENT);
            set(ctx, st);
            match &e.act {
                Some(Act::Menu(what)) => macmenu::push(what.clone()),
                Some(Act::Cmd(run)) => run(app, ctx),
                None => {}
            }
            return;
        }
    }
    set(ctx, st);
}

/// The result rows: label on the left (the menu it lives in, dimmed), keys on
/// the right, the reason in place of keys when the row cannot run.
fn list(ui: &mut egui::Ui, all: &[Entry], shown: &[usize], st: &mut State, pick: &mut Option<usize>, width: f32) {
    let t = theme::tokens(ui.ctx());
    if shown.is_empty() {
        let (r, resp) = ui.allocate_exact_size(Vec2::new(width, ROW_H), Sense::hover());
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, "No matching commands"));
        ui.painter().text(egui::pos2(r.left() + theme::space::SM, r.center().y), Align2::LEFT_CENTER, "No matching commands", theme::body(), t.text_3);
        return;
    }
    egui::ScrollArea::vertical().id_salt("palette-list").max_height(ROWS_SHOWN as f32 * ROW_H).auto_shrink([false, true]).show(ui, |ui| {
        for (n, &i) in shown.iter().enumerate() {
            let e = &all[i];
            let (r, resp) = ui.allocate_exact_size(Vec2::new(width, ROW_H), Sense::click());
            let on = n == st.sel;
            let text = match &e.state {
                Ok(()) => e.title(),
                Err(why) => format!("{} (unavailable: {why})", e.title()),
            };
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, e.enabled(), &text));
            if resp.hovered() && ui.input(|i| i.pointer.delta() != Vec2::ZERO) {
                st.sel = n;
            }
            if on && st.moved {
                resp.scroll_to_me(None);
                st.moved = false;
            }
            if on {
                ui.painter().rect_filled(r, egui::CornerRadius::same(theme::radius::CONTROL), t.accent_soft);
            }
            if resp.clicked() {
                st.sel = n;
                *pick = Some(i);
            }
            let (main, dim) = if e.enabled() { (t.text, t.text_2) } else { (t.text_3, t.text_3) };
            let p = ui.painter();
            let mut x = r.left() + theme::space::SM;
            if !e.menu.is_empty() {
                let g = p.layout_no_wrap(format!("{} ▸ ", e.menu), theme::body(), dim);
                let w = g.size().x;
                p.galley(egui::pos2(x, r.center().y - g.size().y / 2.0), g, dim);
                x += w;
            }
            let g = p.layout_no_wrap(e.label.clone(), theme::body(), main);
            p.galley(egui::pos2(x, r.center().y - g.size().y / 2.0), g, main);
            let right = match &e.state {
                Ok(()) => e.keys.clone(),
                Err(why) => why.clone(),
            };
            if !right.is_empty() {
                p.text(egui::pos2(r.right() - theme::space::SM, r.center().y), Align2::RIGHT_CENTER, right, theme::caption(), dim);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_tests::Harness;

    fn sample() -> Vec<Entry> {
        vec![
            Entry::plain("a", "Save", "File", "", true),
            Entry::plain("b", "Save as…", "File", "", true),
            Entry::plain("c", "Open Settings", "", "preferences options", true),
            Entry::plain("d", "Delete", "Edit", "", false),
            Entry::plain("e", "Split at playhead", "Clip", "cut blade", true),
        ]
    }

    fn ids(e: &[Entry], q: &str, recent: &[&str]) -> Vec<String> {
        let recent: Vec<String> = recent.iter().map(|s| s.to_string()).collect();
        rank(e, q, &recent).into_iter().map(|i| e[i].id.clone()).collect()
    }

    #[test]
    fn matches_label_keywords_and_menu_title() {
        let e = sample();
        assert_eq!(ids(&e, "preferences", &[]), ["c"]);
        assert_eq!(ids(&e, "file save", &[]), ["a", "b"]);
        assert_eq!(ids(&e, "blade", &[]), ["e"]);
        // Letters in order find it too, and a word that is nowhere finds nothing.
        assert_eq!(ids(&e, "splt", &[]), ["e"]);
        assert!(ids(&e, "zzz", &[]).is_empty());
        assert!(ids(&e, "save zzz", &[]).is_empty());
    }

    #[test]
    fn ranks_closer_matches_first_and_disabled_rows_last() {
        let e = sample();
        assert_eq!(ids(&e, "save", &[]), ["a", "b"], "the shorter, earlier match leads");
        assert_eq!(ids(&e, "e", &[]).last().map(String::as_str), Some("d"), "a row that cannot run is last");
        let all = ids(&e, "", &[]);
        assert_eq!(all.len(), e.len());
        assert_eq!(all, ["a", "b", "c", "e", "d"]);
    }

    #[test]
    fn recent_commands_lead_the_list() {
        let e = sample();
        assert_eq!(ids(&e, "", &["e", "c"]), ["e", "c", "a", "b", "d"]);
        assert_eq!(ids(&e, "save", &["b"]), ["b", "a"]);
    }

    #[test]
    fn registering_twice_keeps_one_and_the_last() {
        register(Command::new("test:twice", "First", "", always, |_, _| {}));
        register(Command::new("test:twice", "Second", "", always, |_, _| {}));
        let all = REGISTRY.lock().unwrap();
        let mine: Vec<_> = all.iter().filter(|c| c.id == "test:twice").collect();
        assert_eq!(mine.len(), 1);
        assert_eq!(mine[0].label, "Second");
    }

    fn open_palette(h: &mut Harness) {
        h.with_events(vec![]);
        open(&h.ctx);
        h.with_events(vec![]);
        assert!(is_open(&h.ctx));
    }

    fn type_text(h: &mut Harness, s: &str) {
        h.with_events(vec![egui::Event::Text(s.into())]);
    }

    #[test]
    fn opens_with_its_keys_lists_rows_and_closes_on_esc() {
        let mut h = Harness::new(&["--screen", "editor"]);
        open_palette(&mut h);
        h.frames(2);
        let labels = h.labels();
        assert!(labels.iter().any(|l| l.contains("Search commands")), "{labels:?}");
        assert!(labels.iter().any(|l| l.contains("File ▸ Save")), "menu rows are listed with their menu");
        assert!(labels.iter().any(|l| l.contains("Open Edit workspace")), "registered commands are listed");
        assert!(h.unlabelled().is_empty(), "every control is named: {:?}", h.unlabelled());
        h.key(Key::Escape, Modifiers::NONE);
        assert!(!is_open(&h.ctx));
    }

    #[test]
    fn a_disabled_row_shows_its_reason_and_enter_does_not_run_it() {
        let mut h = Harness::new(&["--screen", "editor"]);
        open_palette(&mut h);
        type_text(&mut h, "edit ▸ delete");
        h.frames(2);
        let labels = h.labels();
        let row = labels.iter().find(|l| l.starts_with("Edit ▸ Delete")).unwrap_or_else(|| panic!("{labels:?}"));
        assert!(row.contains("Select a clip first"), "{row}");
        assert!(h.app.editor.selection.is_empty());
        h.key(Key::Enter, Modifiers::NONE);
        assert!(is_open(&h.ctx), "a row that cannot run leaves the palette open, showing why");
    }

    /// A menu row picked in the palette runs what the menu bar's pick of it runs.
    #[test]
    fn a_menu_row_picked_here_runs_like_the_menu_bar() {
        let mut h = Harness::new(&["--screen", "editor"]);
        open_palette(&mut h);
        let before = h.app.doc.project.markers.len();
        type_text(&mut h, "clip add marker");
        h.frames(1);
        h.key(Key::Enter, Modifiers::NONE);
        assert!(!is_open(&h.ctx), "a pick closes the palette");
        h.frames(2);
        assert_eq!(h.app.doc.project.markers.len(), before + 1, "the same Do the Clip menu queues");
        assert!(state(&h.ctx).recent.iter().any(|r| r == "menu:Clip:Add marker"), "it leads next time");
    }

    #[test]
    fn arrow_keys_move_and_a_registered_command_runs() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static RAN: AtomicUsize = AtomicUsize::new(0);
        register(Command::new("test:ran", "Zzqx probe command", "", always, |_, _| {
            RAN.fetch_add(1, Ordering::SeqCst);
        }));
        let mut h = Harness::new(&["--screen", "editor"]);
        open_palette(&mut h);
        type_text(&mut h, "zzqx");
        h.frames(1);
        assert_eq!(state(&h.ctx).sel, 0);
        h.key(Key::ArrowDown, Modifiers::NONE);
        assert_eq!(state(&h.ctx).sel, 0, "one match: nowhere to go");
        h.key(Key::Enter, Modifiers::NONE);
        assert_eq!(RAN.load(Ordering::SeqCst), 1);
        assert!(!is_open(&h.ctx));
    }

    #[test]
    fn it_closes_when_a_window_opens() {
        let mut h = Harness::new(&["--screen", "editor"]);
        h.with_events(vec![]);
        open(&h.ctx);
        h.app.overlay = Overlay::Help;
        h.with_events(vec![]);
        assert!(!is_open(&h.ctx), "an open window takes the keys: the palette closes");
    }

    /// The palette's own keys, as the window sees them (the harness sends no modifiers).
    #[test]
    fn its_keys_are_found_by_the_keymap() {
        let ctx = egui::Context::default();
        let m = Modifiers { alt: true, command: true, mac_cmd: cfg!(target_os = "macos"), ctrl: !cfg!(target_os = "macos"), ..Modifiers::NONE };
        let ev = egui::Event::Key { key: Key::P, physical_key: None, pressed: true, repeat: false, modifiers: m };
        let mut hit = false;
        let mut out = ctx.run_ui(egui::RawInput { events: vec![ev], ..Default::default() }, |ui| hit = crate::keymap::hit(ui.ctx(), &crate::prefs::Prefs::default(), "palette"));
        out.textures_delta.clear();
        assert!(hit);
    }

    #[test]
    fn the_open_palette_frame_has_no_overlaps_and_no_clipped_text() {
        let mut h = Harness::new(&["--screen", "editor"]);
        open_palette(&mut h);
        type_text(&mut h, "delete");
        h.frames(2);
        let cut: Vec<_> = h.cut_text().into_iter().collect();
        assert!(cut.is_empty(), "{} clipped", cut.len());
    }
}
