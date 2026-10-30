// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Headless interface tests: the real app runs frames in an egui context
//! with no window, and the tests read what it drew and what a screen reader
//! would be told (the AccessKit tree).

use crate::app::{Overlay, StudioApp};
use crate::cli::Cli;
use eframe::egui::{self, accesskit, Event, Key, Modifiers, RawInput, Rect};
use std::collections::BTreeMap;

/// Language and accessibility modes are process-wide, so the tests that
/// change them run one at a time.
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

thread_local! {
    static HELD: std::cell::RefCell<Option<std::sync::MutexGuard<'static, ()>>> = const { std::cell::RefCell::new(None) };
}

/// Holds the process-wide settings still for the rest of this test: the
/// turn lasts until the test's thread ends, so a helper that builds the app
/// keeps it for the test that called it. Taking it again on the same thread
/// does nothing.
pub fn serial() -> Serial {
    HELD.with(|h| {
        let mut h = h.borrow_mut();
        if h.is_none() {
            *h = Some(SERIAL.lock().unwrap_or_else(|e| e.into_inner()));
        }
    });
    Serial
}

/// This thread's turn at the process-wide settings (see [`serial`]).
pub struct Serial;

pub struct Harness {
    _serial: Serial,
    pub app: StudioApp,
    pub ctx: egui::Context,
    frame: eframe::Frame,
    time: f64,
    pub last: Option<egui::FullOutput>,
    /// The window's size in points.
    pub size: egui::Vec2,
    /// Textures kept as a window's backend keeps them, for [`Harness::shot`].
    textures: crate::ui_shot::Textures,
}

impl Harness {
    pub fn new(args: &[&str]) -> Self {
        let serial = serial();
        crate::i18n::set_lang(studio_i18n::Lang::En);
        crate::access::set(false, crate::prefs::ColourVision::Standard);
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut argv: Vec<String> = vec!["--fresh".into(), "--simple-preview".into()];
        argv.extend(args.iter().map(|s| s.to_string()));
        let cli = Cli::parse(argv.into_iter());
        let mut app = StudioApp::new(&cc, cli);
        // Tests are not first-run: no tour over the screen unless asked.
        app.prefs.tour_done = true;
        app.prefs.tips_on = false;
        Harness { _serial: serial, app, ctx, frame: eframe::Frame::_new_kittest(), time: 0.0, last: None, size: egui::vec2(1440.0, 900.0), textures: Default::default() }
    }

    pub fn with_events(&mut self, events: Vec<Event>) -> &egui::FullOutput {
        self.time += 1.0 / 60.0;
        let raw = RawInput {
            screen_rect: Some(Rect::from_min_size(egui::pos2(0.0, 0.0), self.size)),
            time: Some(self.time),
            events,
            ..Default::default()
        };
        let app = &mut self.app;
        let frame = &mut self.frame;
        let mut out = self.ctx.run_ui(raw, |ui| eframe::App::ui(app, ui, frame));
        // Taken as a renderer takes them; dropping them unhandled would panic.
        self.textures.apply(&out.textures_delta);
        out.textures_delta.clear();
        self.last = Some(out);
        self.last.as_ref().unwrap()
    }

    /// A window of `w`×`h` points from the next frame on.
    pub fn sized(mut self, w: f32, h: f32) -> Self {
        self.size = egui::vec2(w, h);
        self
    }

    /// The last frame as a PNG at 1 pixel per point.
    pub fn shot(&self, path: impl AsRef<std::path::Path>) {
        let out = self.last.as_ref().expect("a frame to picture");
        let prims = self.ctx.tessellate(out.shapes.clone(), 1.0);
        let size = [self.size.x as usize, self.size.y as usize];
        let bg = crate::theme::tokens(&self.ctx).ground;
        let rgba = crate::ui_shot::render(&prims, &self.textures, size, 1.0, bg);
        crate::ui_shot::save_png(path.as_ref(), size, &rgba).expect("write the picture");
    }

    /// Text in the last frame cut off with no way to see the rest.
    pub fn cut_text(&self) -> Vec<crate::ui_shot::Cut> {
        let out = self.last.as_ref().expect("a frame");
        let mut cuts = crate::ui_shot::cut_text(&self.ctx, out, &[], 1.0);
        // Timeline clip names run past their clip: editor/timeline.rs (listed
        // in the R3-4 report, not this package's file).
        let named: Vec<(String, Rect)> = out.platform_output.accesskit_update.as_ref().map(|u| u.nodes.iter().filter_map(|(_, n)| n.bounds().map(|b| (n.label().unwrap_or("").to_owned(), Rect::from_min_max(egui::pos2(b.x0 as f32, b.y0 as f32), egui::pos2(b.x1 as f32, b.y1 as f32))))).collect()).unwrap_or_default();
        let clips: Vec<Rect> = out.platform_output.accesskit_update.as_ref().map(|u| u.nodes.iter().filter(|(_, n)| n.label().is_some_and(|l| l.contains(" clip on "))).filter_map(|(_, n)| n.bounds()).map(|b| Rect::from_min_max(egui::pos2(b.x0 as f32, b.y0 as f32), egui::pos2(b.x1 as f32, b.y1 as f32))).collect()).unwrap_or_default();
        let close = |a: Rect, b: Rect| (a.min - b.min).abs().max_elem() < 2.0 && (a.max - b.max).abs().max_elem() < 2.0;
        cuts.extend(crate::ui_shot::spilled_text(&self.ctx, out, &named, 1.5).into_iter().filter(|c| !clips.iter().any(|r| close(*r, c.clip))));
        cuts
    }

    /// Widgets in the last frame drawn over each other.
    pub fn overlaps(&self) -> Vec<crate::ui_shot::Overlap> {
        let names = self.last.as_ref().and_then(|o| o.platform_output.accesskit_update.as_ref()).map(|u| u.nodes.iter().map(|(id, n)| (id.0, n.label().or(n.value()).unwrap_or("").to_owned())).collect()).unwrap_or_default();
        // By design: egui's window resize handles lie over the window, and
        // a transition sits across the cut between two clips.
        let by_design = |a: &str, b: &str| {
            let cut = |x: &str| x.contains(" · ") && x.ends_with('s') && !x.contains(" clip on ");
            a.starts_with("Resize window") || b.starts_with("Resize window") || (a.contains(" clip on ") && cut(b)) || (b.contains(" clip on ") && cut(a))
        };
        crate::ui_shot::overlaps(&self.ctx, &names, 0.2).into_iter().filter(|o| !by_design(&o.a, &o.b)).collect()
    }

    /// A frame in which the window's close button was pressed; the commands
    /// the app sent back (`CancelClose` when it wants to ask first).
    pub fn close_requested(&mut self) -> Vec<egui::ViewportCommand> {
        self.time += 1.0 / 60.0;
        let rect = Rect::from_min_size(egui::pos2(0.0, 0.0), self.size);
        let mut raw = RawInput { screen_rect: Some(rect), time: Some(self.time), ..Default::default() };
        raw.viewports.entry(egui::ViewportId::ROOT).or_default().events.push(egui::ViewportEvent::Close);
        let (app, frame) = (&mut self.app, &mut self.frame);
        let mut out = self.ctx.run_ui(raw, |ui| eframe::App::ui(app, ui, frame));
        self.textures.apply(&out.textures_delta);
        out.textures_delta.clear();
        let cmds = out.viewport_output.get(&egui::ViewportId::ROOT).map(|v| v.commands.clone()).unwrap_or_default();
        self.last = Some(out);
        cmds
    }

    pub fn frames(&mut self, n: usize) {
        for _ in 0..n {
            self.with_events(vec![]);
        }
    }

    pub fn key(&mut self, key: Key, modifiers: Modifiers) {
        self.with_events(vec![Event::Key { key, physical_key: Some(key), pressed: true, repeat: false, modifiers }]);
        self.with_events(vec![Event::Key { key, physical_key: Some(key), pressed: false, repeat: false, modifiers }]);
    }

    /// Every node of the accessibility tree from the last frame.
    pub fn nodes(&self) -> Vec<(accesskit::Role, String)> {
        let Some(update) = self.last.as_ref().and_then(|o| o.platform_output.accesskit_update.as_ref()) else { return vec![] };
        update.nodes.iter().map(|(_, n)| (n.role(), n.label().or(n.value()).unwrap_or("").to_owned())).collect()
    }

    pub fn labels(&self) -> Vec<String> {
        self.nodes().into_iter().map(|(_, l)| l).filter(|l| !l.is_empty()).collect()
    }

    /// Interactive controls that a screen reader would announce with no name.
    pub fn unlabelled(&self) -> Vec<accesskit::Role> {
        use accesskit::Role::*;
        self.nodes().into_iter().filter(|(r, l)| matches!(r, Button | CheckBox | RadioButton | ComboBox | Slider | TextInput | ListBoxOption | Switch | Tab | MenuItem | SpinButton) && l.trim().is_empty()).map(|(r, _)| r).collect()
    }
}

#[test]
fn start_screen_runs_in_every_language() {
    for l in studio_i18n::Lang::ALL {
        let mut h = Harness::new(&[]);
        h.app.prefs.language = l.tag().into();
        h.frames(3);
        assert_eq!(crate::i18n::lang(), l);
        assert!(h.last.as_ref().unwrap().shapes.len() > 5, "{}: something was drawn", l.tag());
        // The language reaches the screen reader too.
        assert!(!h.labels().is_empty(), "{}: the tree has labels", l.tag());
    }
    crate::i18n::set_lang(studio_i18n::Lang::En);
}

#[test]
fn start_screen_controls_are_named() {
    let mut h = Harness::new(&[]);
    h.frames(3);
    let bad = h.unlabelled();
    assert!(bad.is_empty(), "controls without a name on the start screen: {bad:?}");
}

#[test]
fn editor_controls_are_named() {
    let mut h = Harness::new(&["--screen", "editor"]);
    h.frames(4);
    let bad = h.unlabelled();
    assert!(bad.is_empty(), "controls without a name in the editor: {bad:?}");
    let labels = h.labels();
    for want in ["Export", "Jobs", "Edit", "Colour", "Audio", "Effects", "Motion", "Review", "Paint"] {
        assert!(labels.iter().any(|l| l == want), "{want} is in the accessibility tree: {labels:?}");
    }
}

#[test]
fn timeline_clips_are_named_with_their_details() {
    let mut h = Harness::new(&["--screen", "editor"]);
    h.frames(4);
    let options: Vec<_> = h.nodes().into_iter().filter(|(r, _)| *r == accesskit::Role::ListBoxOption).collect();
    assert!(!options.is_empty(), "clip nodes are in the tree");
    let labels = h.labels();
    let clip = labels.iter().find(|l| l.contains("Lake at dawn") && l.contains("lasts")).unwrap_or_else(|| panic!("a clip label: {labels:?}"));
    assert!(clip.contains("Video clip") && clip.contains("lasts"), "{clip}");
}

#[test]
fn settings_and_export_and_help_are_named() {
    for (overlay, name) in [(Overlay::Settings, "settings"), (Overlay::Export, "export"), (Overlay::Help, "help")] {
        let mut h = Harness::new(&["--screen", "editor"]);
        h.app.overlay = overlay;
        h.frames(4);
        let bad = h.unlabelled();
        assert!(bad.is_empty(), "{name}: controls without a name: {bad:?}");
    }
}

#[test]
fn right_to_left_mirrors_the_panels() {
    let region = |h: &Harness, name: &str| -> Rect {
        h.ctx.data(|d| d.get_temp::<Vec<(&'static str, Rect)>>(egui::Id::new("onboarding-targets"))).unwrap().into_iter().find(|(n, _)| *n == name).map(|(_, r)| r).unwrap()
    };
    let mut ltr = Harness::new(&["--screen", "editor"]);
    ltr.app.prefs.language = "en".into();
    ltr.frames(4);
    assert!(region(&ltr, "Library").center().x < region(&ltr, "Viewer").center().x);
    assert!(region(&ltr, "Inspector").center().x > region(&ltr, "Viewer").center().x);
    let ltr_timeline = region(&ltr, "Timeline");
    // One harness at a time: they share the language setting.
    drop(ltr);

    let mut rtl = Harness::new(&["--screen", "editor"]);
    rtl.app.prefs.language = "fa".into();
    rtl.frames(4);
    assert!(region(&rtl, "Library").center().x > region(&rtl, "Viewer").center().x, "library on the right in Persian");
    assert!(region(&rtl, "Inspector").center().x < region(&rtl, "Viewer").center().x, "inspector on the left in Persian");
    // Time still runs left to right: the timeline spans the window as before.
    assert_eq!(region(&rtl, "Timeline"), ltr_timeline);
    crate::i18n::set_lang(studio_i18n::Lang::En);
}

#[test]
fn keyboard_moves_between_regions() {
    let mut h = Harness::new(&["--screen", "editor"]);
    h.frames(3);
    h.key(Key::F6, Modifiers::NONE);
    h.frames(2);
    assert!(crate::a11y::focused_region(&h.ctx).is_some(), "F6 focuses a region");
    let first = crate::a11y::focused_region(&h.ctx);
    h.key(Key::F6, Modifiers::NONE);
    h.frames(2);
    assert_ne!(crate::a11y::focused_region(&h.ctx), first);
    assert!(!crate::a11y::last_announcement(&h.ctx).is_empty(), "the region is announced");
}

#[test]
fn keyboard_walks_the_timeline_and_says_what_it_finds() {
    let mut h = Harness::new(&["--screen", "editor"]);
    h.frames(3);
    h.app.editor.playhead = 0;
    h.app.editor.selection.clear();
    h.key(Key::CloseBracket, Modifiers::NONE);
    h.frames(2);
    assert_eq!(h.app.editor.selection.len(), 1, "] selects a clip");
    let first = h.app.editor.selection[0];
    assert!(crate::a11y::last_announcement(&h.ctx).contains("clip"), "{}", crate::a11y::last_announcement(&h.ctx));
    h.key(Key::CloseBracket, Modifiers::NONE);
    h.frames(2);
    assert_ne!(h.app.editor.selection[0], first, "] moves to the next clip");
    h.key(Key::OpenBracket, Modifiers::NONE);
    h.frames(2);
    assert_eq!(h.app.editor.selection[0], first, "[ goes back");
    // Edit points.
    h.app.editor.playhead = 0;
    h.key(Key::ArrowDown, Modifiers::NONE);
    h.frames(2);
    assert!(h.app.editor.playhead > 0, "Down jumps to the next edit point");
}

#[test]
fn nudging_moves_the_selected_clip_one_frame() {
    let mut h = Harness::new(&["--screen", "editor"]);
    h.frames(3);
    h.app.editor.playhead = 0;
    h.key(Key::CloseBracket, Modifiers::NONE);
    h.frames(2);
    let id = h.app.editor.selection[0];
    let before = h.app.doc.project.clip(id).unwrap().start;
    // Pick a clip that can move later: the sample's overlay titles.
    h.app.editor.selection = vec![h.app.doc.project.clips.iter().find(|c| c.kind == crate::view::ClipKind::Title).unwrap().id];
    let id = h.app.editor.selection[0];
    let before_title = h.app.doc.project.clip(id).unwrap().start;
    h.key(Key::ArrowRight, Modifiers::ALT);
    h.frames(2);
    let after = h.app.doc.project.clip(id).unwrap().start;
    assert!(after > before_title, "Alt+Right nudged later ({before_title} to {after})");
    let _ = before;
    h.app.doc.undo();
    h.app.doc.undo();
}

#[test]
fn workspaces_switch_with_the_keyboard() {
    let mut h = Harness::new(&["--screen", "editor"]);
    h.frames(3);
    h.key(Key::Num2, Modifiers::ALT);
    h.frames(2);
    assert_eq!(h.app.prefs.workspace, "Colour");
    assert_eq!(h.app.editor.rail, crate::editor::RailItem::Looks);
    h.key(Key::Num6, Modifiers::ALT);
    h.frames(2);
    assert_eq!(h.app.prefs.workspace, "Review");
    assert!(h.app.editor.dock.is_showing(crate::dock::Panel::Review));
    h.key(Key::Num1, Modifiers::ALT);
    h.frames(2);
    assert!(h.app.editor.panel_open(crate::dock::Panel::Inspector));
    assert!(crate::a11y::last_announcement(&h.ctx).contains("Edit"));
}

#[test]
fn toasts_are_announced() {
    let mut h = Harness::new(&["--screen", "editor"]);
    h.frames(2);
    crate::kit::push(&h.ctx, "Saved my project", None, None);
    h.frames(2);
    assert_eq!(crate::a11y::last_announcement(&h.ctx), "Saved my project");
    // The live region is in the tree.
    let live: Vec<_> = h.last.as_ref().unwrap().platform_output.accesskit_update.as_ref().unwrap().nodes.iter().filter(|(_, n)| n.live().is_some()).collect();
    assert!(!live.is_empty(), "a live region exists");
}

#[test]
fn announcements_follow_the_language() {
    let mut h = Harness::new(&["--screen", "editor"]);
    h.app.prefs.language = "de".into();
    h.frames(2);
    crate::kit::push(&h.ctx, "Cut removed", None, None);
    h.frames(2);
    let said = crate::a11y::last_announcement(&h.ctx);
    assert_ne!(said, "Cut removed", "translated");
    assert!(!said.is_empty());
    crate::i18n::set_lang(studio_i18n::Lang::En);
}

#[test]
fn high_contrast_and_colour_vision_change_the_colours() {
    let mut h = Harness::new(&["--screen", "editor"]);
    h.frames(2);
    let base = crate::theme::tokens(&h.ctx);
    h.app.prefs.high_contrast = true;
    h.frames(2);
    let hc = crate::theme::tokens(&h.ctx);
    assert_ne!(base.text_2, hc.text_2);
    assert!(crate::theme::contrast(hc.text_2, hc.panel) >= 7.0);
    h.app.prefs.high_contrast = false;
    h.app.prefs.colour_vision = crate::prefs::ColourVision::RedGreen;
    h.frames(2);
    assert_ne!(crate::theme::tokens(&h.ctx).audio, base.audio);
    h.app.prefs.colour_vision = crate::prefs::ColourVision::Standard;
    h.frames(2);
    assert_eq!(crate::theme::tokens(&h.ctx), base);
}

#[test]
fn interface_scales_up_to_200_percent() {
    let mut h = Harness::new(&["--screen", "editor"]);
    h.app.prefs.ui_scale = 2.0;
    h.frames(3);
    assert!((h.ctx.zoom_factor() - 2.0).abs() < 0.01);
    assert!(h.unlabelled().is_empty());
    h.app.prefs.ui_scale = 1.0;
}

#[test]
fn reduced_motion_is_respected() {
    let mut h = Harness::new(&["--screen", "editor"]);
    h.app.prefs.reduce_motion = true;
    h.frames(2);
    assert!(crate::kit::reduce_motion(&h.ctx));
}

#[test]
fn the_tour_runs_once_and_can_be_replayed() {
    let mut h = Harness::new(&["--screen", "editor"]);
    h.app.prefs.tour_done = false;
    h.frames(3);
    assert!(crate::onboarding::tour_running(&h.app));
    let said = crate::a11y::last_announcement(&h.ctx);
    assert!(said.starts_with("Library"), "the tour opens on the Library: {said}");
    // Walk to the end with the keyboard.
    for _ in 0..crate::onboarding::STEPS.len() + 1 {
        h.key(Key::ArrowRight, Modifiers::NONE);
        h.frames(2);
    }
    assert!(h.app.prefs.tour_done, "finishing the tour is remembered");
    // Replay from Settings' button, or Help ▸ Welcome tour.
    crate::editor::run(&mut h.app, &h.ctx.clone(), crate::editor::Action::Tour);
    h.frames(2);
    assert!(crate::onboarding::tour_running(&h.app));
    h.key(Key::Escape, Modifiers::NONE);
    h.frames(2);
    assert!(h.app.prefs.tour_done, "Escape skips");
}

#[test]
fn tips_appear_once_and_can_be_turned_off() {
    let mut h = Harness::new(&["--screen", "editor", "--project", "empty"]);
    h.app.prefs.tips_on = true;
    // Time passes: the first applicable tip shows after a moment.
    for _ in 0..400 {
        h.with_events(vec![]);
    }
    assert!(crate::a11y::last_announcement(&h.ctx).contains("Drag media"), "{}", crate::a11y::last_announcement(&h.ctx));
    assert_eq!(crate::onboarding::next_tip(&h.app), Some(1));
    h.app.prefs.tips_seen.push("empty-timeline".into());
    assert_ne!(crate::onboarding::next_tip(&h.app), Some(1));
}

#[test]
fn f1_opens_help_and_search_finds_articles() {
    let mut h = Harness::new(&["--screen", "editor"]);
    h.frames(2);
    h.key(Key::F1, Modifiers::NONE);
    h.frames(3);
    assert_eq!(h.app.overlay, Overlay::Help);
    h.key(Key::F1, Modifiers::NONE);
    h.frames(2);
    assert_eq!(h.app.overlay, Overlay::None);
    // With the timeline focused, F1 opens its article.
    h.key(Key::F6, Modifiers::NONE);
    h.frames(2);
    while crate::a11y::focused_region(&h.ctx) != Some(crate::a11y::Region::Timeline) {
        h.key(Key::F6, Modifiers::NONE);
        h.frames(2);
    }
    h.key(Key::F1, Modifiers::NONE);
    h.frames(3);
    let labels = h.labels();
    assert!(labels.iter().any(|l| l == "The timeline"), "{labels:?}");
}

#[test]
fn start_screen_offers_the_seven_templates() {
    let mut h = Harness::new(&[]);
    h.frames(4);
    let labels = h.labels();
    for name in ["YouTube video", "Short film", "Music video", "Animated story", "Documentary", "Podcast", "Ad"] {
        assert!(labels.iter().any(|l| l == name), "template {name}: {labels:?}");
    }
}

#[test]
fn text_is_translated_on_screen() {
    let mut h = Harness::new(&["--screen", "editor"]);
    h.app.prefs.language = "fr".into();
    h.frames(3);
    let labels = h.labels();
    assert!(labels.iter().any(|l| l == "Exporter"), "{labels:?}");
    crate::i18n::set_lang(studio_i18n::Lang::En);
}

#[test]
fn nothing_on_screen_is_a_message_id() {
    // A missing message shows its id; ids look like `a-b-c` with a hyphen
    // and no spaces.
    for l in ["en", "ar", "hi", "ja"] {
        let mut h = Harness::new(&["--screen", "editor"]);
        h.app.prefs.language = l.into();
        h.frames(3);
        for label in h.labels() {
            let looks_like_id = !label.contains(' ') && label.contains('-') && label.chars().all(|c| c.is_ascii_lowercase() || c == '-' || c.is_ascii_digit()) && label.len() > 8;
            assert!(!looks_like_id, "{l}: `{label}` looks like a message id");
        }
    }
    crate::i18n::set_lang(studio_i18n::Lang::En);
}

#[test]
fn label_text_map_is_stable() {
    // The set of labelled roles in the editor, as a coarse regression check.
    let mut h = Harness::new(&["--screen", "editor"]);
    h.frames(3);
    let mut by_role: BTreeMap<String, usize> = BTreeMap::new();
    for (r, l) in h.nodes() {
        if !l.is_empty() {
            *by_role.entry(format!("{r:?}")).or_default() += 1;
        }
    }
    assert!(by_role.get("Button").copied().unwrap_or(0) >= 10, "{by_role:?}");
}

#[test]
fn the_canvas_can_be_worked_from_the_keyboard() {
    let mut h = Harness::new(&["--screen", "editor"]);
    h.frames(3);
    h.app.editor.selection.clear();
    h.app.editor.playhead = 2 * crate::view::SEC;
    let stack = crate::a11y::layers_at(&h.app.doc.project, h.app.editor.playhead);
    assert!(stack.len() >= 2, "the sample has stacked layers at 2 s: {stack:?}");
    // `.` picks the top layer, then the next one down.
    h.key(Key::Period, Modifiers::NONE);
    h.frames(2);
    assert_eq!(h.app.editor.selection, vec![stack[0]]);
    assert!(crate::a11y::last_announcement(&h.ctx).starts_with("Layer 1 of"), "{}", crate::a11y::last_announcement(&h.ctx));
    h.key(Key::Period, Modifiers::NONE);
    h.frames(2);
    assert_eq!(h.app.editor.selection, vec![stack[1]]);
    h.key(Key::Comma, Modifiers::NONE);
    h.frames(2);
    assert_eq!(h.app.editor.selection, vec![stack[0]]);
    // ⌘⌥→ moves it 1 % of the picture to the right and says so.
    let before = h.app.doc.project.clip(stack[0]).unwrap().transform.x;
    h.key(Key::ArrowRight, Modifiers::COMMAND | Modifiers::ALT);
    h.frames(2);
    let after = h.app.doc.project.clip(stack[0]).unwrap().transform.x;
    assert!((after - before - 19.2).abs() < 0.5, "moved by 1 % of 1920 px: {before} to {after}");
    assert!(crate::a11y::last_announcement(&h.ctx).starts_with("Moved to"), "{}", crate::a11y::last_announcement(&h.ctx));
    // ⌥, lists the layers.
    h.key(Key::Comma, Modifiers::ALT);
    h.frames(2);
    assert!(crate::a11y::last_announcement(&h.ctx).contains("layers at the playhead"), "{}", crate::a11y::last_announcement(&h.ctx));
    h.app.doc.undo();
}

#[test]
fn alt_period_says_where_you_are() {
    let mut h = Harness::new(&["--screen", "editor"]);
    h.frames(3);
    h.key(Key::Period, Modifiers::ALT);
    h.frames(2);
    assert!(crate::a11y::last_announcement(&h.ctx).starts_with("Playhead at"), "{}", crate::a11y::last_announcement(&h.ctx));
}

// --- glyph coverage ------------------------------------------------------
//
// Icons are drawn as textures (`icons.rs`), never as font characters, so no
// private-use character should reach a font. These tests fail when one does,
// and when any character in the UI would draw as an empty box.

fn is_private_use(c: char) -> bool {
    matches!(c as u32, 0xE000..=0xF8FF | 0xF_0000..=0xF_FFFD | 0x10_0000..=0x10_FFFD)
}

fn walk_text(shape: &egui::epaint::Shape, f: &mut impl FnMut(&egui::epaint::TextShape)) {
    match shape {
        egui::epaint::Shape::Text(t) => f(t),
        egui::epaint::Shape::Vec(v) => v.iter().for_each(|s| walk_text(s, f)),
        _ => {}
    }
}

/// Every character a galley drew, with the font its section asked for, that
/// that font (and its fallbacks) has no glyph for.
fn missing_glyphs(h: &Harness) -> Vec<String> {
    let mut missing = vec![];
    for cs in &h.last.as_ref().expect("a frame").shapes {
        walk_text(&cs.shape, &mut |t| {
            for sec in &t.galley.job.sections {
                let text = &t.galley.job.text[sec.byte_range.start.0 as usize..sec.byte_range.end.0 as usize];
                for c in text.chars().filter(|c| !c.is_control() && !c.is_whitespace()) {
                    if !h.ctx.fonts_mut(|f| f.has_glyph(&sec.format.font_id, c)) {
                        missing.push(format!("U+{:04X} in {:?}", c as u32, text));
                    }
                }
            }
        });
    }
    missing.sort();
    missing.dedup();
    missing
}

#[test]
fn drawn_text_has_glyphs_and_no_private_use_characters() {
    let mut bad = vec![];
    for screen in ["start", "editor", "settings", "export", "gallery", "paint"] {
        let mut h = Harness::new(&["--screen", screen]);
        h.frames(4);
        bad.extend(missing_glyphs(&h).into_iter().map(|m| format!("{screen}: {m}")));
        for l in h.labels() {
            assert!(!l.chars().any(is_private_use), "{screen}: accessibility label has an icon character: {l:?}");
        }
    }
    assert!(bad.is_empty(), "text drawn with a font that has no glyph: {bad:?}");
}

#[test]
fn ui_strings_have_glyphs_and_no_private_use_characters() {
    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }
    let mut files = vec![];
    walk(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut files);
    let h = Harness::new(&[]);
    for _ in 0..2 {
        h.ctx.run_ui(RawInput::default(), |_| {}).textures_delta.clear();
    }
    let mut private = vec![];
    let mut missing: BTreeMap<char, String> = BTreeMap::new();
    for f in files {
        let src = std::fs::read_to_string(&f).unwrap();
        for (n, line) in src.lines().enumerate() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            // Text drawn by the app: any non-ASCII character in a string.
            for c in line.chars().filter(|c| !c.is_ascii()) {
                let name = f.file_name().unwrap().to_string_lossy();
                // Letters are sample text; the terminal shell, the script
                // log and the eval prompts are not drawn by egui.
                if (c.is_alphabetic() || matches!(&*name, "shell.rs" | "script.rs" | "eval.rs" | "ui_tests.rs")) && !is_private_use(c) {
                    continue;
                }
                if is_private_use(c) {
                    private.push(format!("{name}:{} U+{:04X}", n + 1, c as u32));
                } else if !h.ctx.fonts_mut(|fo| fo.has_glyph(&egui::FontId::proportional(14.0), c)) && !c.is_whitespace() {
                    missing.entry(c).or_insert_with(|| format!("{name}:{}", n + 1));
                }
            }
        }
    }
    assert!(private.is_empty(), "private-use characters in UI source: {private:?}");
    assert!(missing.is_empty(), "characters no UI font can draw (would show as boxes): {missing:?}");
}

// --- keyboard focus ---------------------------------------------------------------

/// An editor with its first clip selected, and what the timeline holds.
fn keyed_editor() -> (Harness, Vec<u32>, i64) {
    let mut h = Harness::new(&["--screen", "editor"]);
    h.frames(4);
    let first = h.app.doc.project.clips.first().expect("a sample clip").id;
    h.app.editor.selection = vec![first];
    h.frames(2);
    let clips = h.app.doc.project.clips.iter().map(|c| c.id).collect();
    let playhead = h.app.editor.playhead;
    (h, clips, playhead)
}

/// ⌘ as the window system reports it (the shortcut table reads both flags).
fn cmd() -> Modifiers {
    Modifiers { command: true, mac_cmd: cfg!(target_os = "macos"), ctrl: !cfg!(target_os = "macos"), ..Modifiers::NONE }
}

/// Everything the keyboard can do to a timeline: type, Delete, ⌘A, split,
/// duplicate, trim, shuttle. `owner` claims the keyboard before every key, as a
/// panel does each frame it is focused.
fn hammer_keys(h: &mut Harness, owner: Option<crate::keymap::KeyOwner>) {
    let claim = |h: &Harness| owner.iter().for_each(|o| crate::keymap::claim(&h.ctx, *o));
    claim(h);
    h.with_events(vec![Event::Text("hello world".into())]);
    for key in [Key::H, Key::E, Key::L, Key::O, Key::S, Key::B, Key::M, Key::I, Key::Q, Key::W, Key::Delete, Key::Backspace, Key::ArrowLeft, Key::Home] {
        claim(h);
        h.key(key, Modifiers::NONE);
    }
    for key in [Key::A, Key::D] {
        claim(h);
        h.key(key, cmd());
    }
}

fn click(h: &mut Harness, pos: egui::Pos2) {
    use egui::PointerButton::Primary;
    h.with_events(vec![Event::PointerMoved(pos)]);
    h.with_events(vec![Event::PointerButton { pos, button: Primary, pressed: true, modifiers: Modifiers::NONE }]);
    h.with_events(vec![Event::PointerButton { pos, button: Primary, pressed: false, modifiers: Modifiers::NONE }]);
}

/// The control: with the timeline in charge, the same keys do edit it.
#[test]
fn the_same_keys_edit_the_timeline_when_it_has_the_keyboard() {
    let (mut h, clips, _) = keyed_editor();
    h.key(Key::A, cmd());
    assert!(h.app.editor.selection.len() > 1 || clips.len() == 1, "⌘A selects the clips");
    h.app.editor.selection = vec![clips[0]];
    h.key(Key::Delete, Modifiers::NONE);
    h.frames(2);
    assert!(h.app.doc.project.clips.len() < clips.len(), "Delete removes the selected clip");
}

/// A click in a panel hands it the keyboard: typing, Delete and ⌘A leave the timeline alone.
#[test]
fn keys_in_a_focused_panel_do_not_reach_the_timeline() {
    for panel in [crate::dock::Panel::Library, crate::dock::Panel::Inspector] {
        let (mut h, clips, playhead) = keyed_editor();
        let before = h.app.editor.selection.clone();
        let steps = h.app.doc.history_len();
        let tile = h.app.editor.last_layout().tile_of(panel).unwrap_or_else(|| panic!("{panel:?} is docked")).rect;
        // The header strip: a click there is in the panel, on no text field.
        click(&mut h, tile.left_top() + egui::vec2(6.0, 4.0));
        h.frames(2);
        assert!(!h.ctx.text_edit_focused(), "{panel:?}: the click is not in a text field");
        assert!(crate::keymap::owner(&h.ctx).is_some(), "{panel:?} has claimed the keyboard");
        hammer_keys(&mut h, None);
        h.frames(2);
        let after: Vec<u32> = h.app.doc.project.clips.iter().map(|c| c.id).collect();
        assert_eq!(after, clips, "{panel:?}: the clips are unchanged");
        assert_eq!(h.app.editor.selection, before, "{panel:?}: the selection is unchanged");
        assert_eq!(h.app.editor.playhead, playhead, "{panel:?}: the playhead is unchanged");
        assert_eq!(h.app.doc.history_len(), steps, "{panel:?}: nothing was edited");
    }
}

/// An editor that claims the keyboard (the node graph, the vector tool) keeps
/// its typing; the claim lapses when it stops asking.
#[test]
fn a_claim_keeps_typing_away_from_the_timeline() {
    use crate::keymap::KeyOwner;
    let (mut h, clips, playhead) = keyed_editor();
    let steps = h.app.doc.history_len();
    for owner in [KeyOwner::Panel, KeyOwner::PanelUndo, KeyOwner::Text] {
        hammer_keys(&mut h, Some(owner));
        let after: Vec<u32> = h.app.doc.project.clips.iter().map(|c| c.id).collect();
        assert_eq!(after, clips, "{owner:?}: the clips are unchanged");
        assert_eq!(h.app.editor.playhead, playhead, "{owner:?}: the playhead is unchanged");
        assert_eq!(h.app.doc.history_len(), steps, "{owner:?}: nothing was edited");
    }
    h.frames(3);
    assert_eq!(crate::keymap::owner(&h.ctx), None, "the claim lapsed");
}

// --- the top bar -------------------------------------------------------------------

const MENUS: [&str; 6] = ["File", "Edit", "Clip", "Project", "View", "Help"];
const TABS: [&str; 7] = ["Edit", "Colour", "Audio", "Effects", "Motion", "Review", "Paint"];

/// Where `text` was painted in the top bar, the leftmost of them (a menu's
/// name can also be a workspace tab's).
fn bar_text(h: &Harness, text: &str) -> Option<egui::Rect> {
    fn walk(shape: &egui::epaint::Shape, text: &str, out: &mut Vec<egui::Rect>) {
        match shape {
            egui::epaint::Shape::Text(t) if t.galley.text() == text => out.push(t.galley.rect.translate(t.pos.to_vec2())),
            egui::epaint::Shape::Vec(v) => v.iter().for_each(|s| walk(s, text, out)),
            _ => {}
        }
    }
    let mut found = Vec::new();
    for cs in &h.last.as_ref()?.shapes {
        walk(&cs.shape, text, &mut found);
    }
    found.into_iter().filter(|r| r.bottom() <= crate::editor::TOPBAR_H).min_by(|a, b| a.left().total_cmp(&b.left()))
}

/// The labels of the buttons a menu adds when it opens.
fn open_menu_items(h: &mut Harness, menu: &str) -> Vec<String> {
    let buttons = |h: &Harness| h.nodes().into_iter().filter(|(r, _)| *r == accesskit::Role::Button).map(|(_, l)| l).collect::<Vec<_>>();
    let before = buttons(h);
    let at = bar_text(h, menu).unwrap_or_else(|| panic!("the {menu} menu"));
    h.click_at(at);
    let mut after = buttons(h);
    for b in &before {
        if let Some(i) = after.iter().position(|a| a == b) {
            after.remove(i);
        }
    }
    after
}

fn top_bar(w: f32) -> Harness {
    let mut h = Harness::new(&["--screen", "editor"]).sized(w, 900.0);
    h.frames(6);
    h
}

/// Menus | workspace tabs with names | Jobs | Export, left to right; Settings,
/// Layout, Library, Production, Review and Paint are no buttons of the bar.
#[test]
fn the_top_bar_is_menus_then_workspace_tabs_then_jobs_and_export() {
    let h = top_bar(1440.0);
    let buttons: Vec<String> = h.nodes().into_iter().filter(|(r, _)| *r == accesskit::Role::Button).map(|(_, l)| l).collect();
    for gone in ["Layout", "Settings", "Library", "Production", "Review", "Paint"] {
        assert!(!buttons.iter().any(|b| b == gone), "{gone} is not a top bar button: {buttons:?}");
    }
    let tab = |n: &str| h.find(Some(accesskit::Role::RadioButton), n).unwrap_or_else(|| panic!("a workspace tab named {n}"));
    let menus: Vec<egui::Rect> = MENUS.iter().map(|m| bar_text(&h, m).unwrap_or_else(|| panic!("the {m} menu"))).collect();
    let tabs: Vec<egui::Rect> = TABS.iter().map(|t| tab(t)).collect();
    let jobs = h.find(None, "Jobs").expect("the Jobs bell");
    let export = h.find(None, "Export").expect("Export");
    assert!(menus.windows(2).all(|w| w[0].right() <= w[1].left()), "menus in order: {menus:?}");
    assert!(tabs.windows(2).all(|w| w[0].right() <= w[1].left() + 1.0), "tabs in order: {tabs:?}");
    assert!(menus.last().unwrap().right() <= tabs[0].left(), "the menus come before the tabs");
    assert!(tabs.last().unwrap().right() <= jobs.left(), "the tabs come before Jobs");
    assert!(jobs.right() <= export.left(), "Jobs comes before Export");
    for r in menus.iter().chain(&tabs).chain([&jobs, &export]) {
        assert!(r.top() >= 0.0 && r.bottom() <= crate::editor::TOPBAR_H + 1.0, "in the bar: {r:?}");
    }
}

/// The tabs stay down to 1100 px with nothing cut off or drawn over; below
/// that one Workspace dropdown has the same list.
#[test]
fn the_workspace_tabs_reach_1100_px_and_become_a_dropdown_below() {
    let h = top_bar(1100.0);
    for t in TABS {
        assert!(h.find(Some(accesskit::Role::RadioButton), t).is_some(), "{t} is a tab at 1100 px");
    }
    let bar = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1100.0, crate::editor::TOPBAR_H));
    let cut: Vec<_> = h.cut_text().into_iter().filter(|c| bar.contains_rect(c.rect.shrink(1.0))).collect();
    assert!(cut.is_empty(), "text cut off in the top bar at 1100 px: {cut:#?}");
    let over: Vec<_> = h.overlaps().into_iter().filter(|o| bar.contains(o.ra.center()) && bar.contains(o.rb.center())).collect();
    assert!(over.is_empty(), "drawn over each other in the top bar at 1100 px: {over:#?}");

    let mut h = top_bar(900.0);
    assert!(h.find(Some(accesskit::Role::RadioButton), "Colour").is_none(), "no tabs at 900 px");
    assert!(h.find(None, "Export").is_some(), "Export stays");
    // The dropdown lists them all, and Paint.
    let dd = h.find_text("Edit").expect("the dropdown shows the workspace");
    // (At this width the menus and the dropdown both read Edit: the dropdown is the right one.)
    h.click_at(dd);
    let labels = h.labels();
    for t in TABS {
        assert!(labels.iter().any(|l| l == t), "{t} is in the dropdown: {labels:?}");
    }
    h.escape();
}

/// Every command has one home in the menus (the tabs, the panel and the
/// bar's own buttons are its contextual places).
#[test]
fn each_menu_command_has_one_home() {
    let mut seen: Vec<(String, &str)> = Vec::new();
    for m in MENUS {
        let mut h = top_bar(1440.0);
        let items = open_menu_items(&mut h, m);
        assert!(!items.is_empty(), "{m} opens with items");
        seen.extend(items.into_iter().map(|i| (i, m)));
    }
    let names: Vec<&str> = seen.iter().map(|(i, _)| i.as_str()).collect();
    // A workspace and its main panel may share a name (Review, Edit…): View
    // lists them under separate headings, Workspaces and Panels.
    for (i, m) in seen.iter().filter(|(i, _)| !TABS.contains(&i.as_str())) {
        assert_eq!(names.iter().filter(|n| *n == i).count(), 1, "\"{i}\" (in {m}) is offered in more than one place: {seen:?}");
    }
    let home = |item: &str| seen.iter().find(|(i, _)| i == item).map(|(_, m)| *m);
    for (item, menu) in [
        ("Project settings…", "File"),
        ("Settings…", "File"),
        ("Export…", "File"),
        ("Production…", "Project"),
        ("Project health…", "Project"),
        ("Reset layout", "View"),
        ("Maximise panel", "View"),
        ("Colour", "View"),
        ("Inspector", "View"),
        ("Review", "View"),
    ] {
        assert_eq!(home(item), Some(menu), "{item} lives in {menu}: {seen:?}");
    }
}

/// The surfaces that were buttons are reached from the menus, the tabs or a panel.
#[test]
fn library_production_review_and_paint_still_open() {
    use crate::app::Overlay;
    let open = |menu: &str, item: &str| {
        let mut h = top_bar(1440.0);
        open_menu_items(&mut h, menu);
        let r = h.find(Some(accesskit::Role::Button), item).unwrap_or_else(|| panic!("{item} in {menu}"));
        h.click_at(r);
        h
    };
    assert_eq!(open("Project", "Production…").app.overlay, Overlay::Production);
    assert_eq!(open("Project", "Project health…").app.overlay, Overlay::Library);
    assert_eq!(open("File", "Settings…").app.overlay, Overlay::Settings);
    assert!(open("View", "Review").app.editor.dock.is_showing(crate::dock::Panel::Review), "View ▸ Review shows the panel");
    // Project settings: the Inspector's Details with the canvas editor open.
    let mut h = open("File", "Project settings…");
    h.frames(3);
    assert!(h.app.editor.selection.is_empty() && h.app.editor.dock.is_open(crate::dock::Panel::Inspector));
    let buttons: Vec<String> = h.nodes().into_iter().filter(|(r, _)| *r == accesskit::Role::Button).map(|(_, l)| l).collect();
    assert!(buttons.iter().any(|b| b == "Done") && !buttons.iter().any(|b| b == "Modify"), "the canvas editor is open: {buttons:?}");
    // Paint is the last workspace tab.
    let mut h = top_bar(1440.0);
    let paint = h.find(Some(accesskit::Role::RadioButton), "Paint").unwrap();
    h.click_at(paint);
    assert_eq!(h.app.screen, crate::cli::Screen::Paint);
}

// --- scripted headless runs (`zornov app --ui-script`) --------------------

mod scripted {
    use super::*;
    use crate::ui_script::{self, Session, Step};

    fn scratch(name: &str, clip: bool) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("zornov-ui-script-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut doc = crate::services::project::new_project(&crate::services::project::CANVASES[0]);
        if clip {
            let media = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../testdata/media/h264_baseline_cavlc.mp4");
            crate::services::project::import_to_timeline(&mut doc, &[media]).expect("a clip on the timeline");
        }
        crate::services::project::save(&mut doc, Some(&dir)).expect("a scratch project");
        dir
    }

    fn session(out: &std::path::Path, extra: &[&str]) -> Session {
        let mut argv: Vec<String> = vec!["--ui-script".into(), "x".into(), "--ui-out".into(), out.display().to_string()];
        argv.extend(extra.iter().map(|s| s.to_string()));
        Session::new(crate::cli::Cli::parse(argv.into_iter()), true)
    }

    fn png(path: &std::path::Path) -> (u32, u32, Vec<u8>) {
        let dec = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).expect("the picture")));
        let mut r = dec.read_info().unwrap();
        let mut buf = vec![0; r.output_buffer_size().unwrap()];
        let info = r.next_frame(&mut buf).unwrap();
        buf.truncate(info.buffer_size());
        (info.width, info.height, buf)
    }

    fn colours(px: &[u8]) -> usize {
        px.chunks_exact(4).step_by(7).map(|p| [p[0], p[1], p[2]]).collect::<std::collections::HashSet<_>>().len()
    }

    #[test]
    fn the_grammar_reads_every_step_and_names_what_is_wrong() {
        let p = |l: &str| ui_script::parse_line(l);
        assert_eq!(p("  # a comment").unwrap(), None);
        assert_eq!(p("size 1800 1028").unwrap(), Some(Step::Size(1800.0, 1028.0)));
        assert_eq!(p("home").unwrap(), Some(Step::Home));
        assert_eq!(p("open \"/tmp/a b\"").unwrap(), Some(Step::Open("/tmp/a b".into())));
        assert!(matches!(p("dclick 10 20 cmd shift").unwrap(), Some(Step::Click { count: 2, mods, .. }) if mods.command && mods.shift));
        assert!(matches!(p("rclick 1 2").unwrap(), Some(Step::Click { button: egui::PointerButton::Secondary, .. })));
        assert!(matches!(p("click \"Save as\"#2 cmd").unwrap(), Some(Step::Click { at: ui_script::Target::Label(l, 2), mods, .. }) if l == "Save as" && mods.command));
        assert!(matches!(p("move \"File\"").unwrap(), Some(Step::Move { at: ui_script::Target::Label(_, 1) })));
        assert!(p("click \"File").is_err() && p("click \"File\"#0").is_err() && p("move \"File\" cmd").is_err());
        assert!(matches!(p("drag 1 2 30 40").unwrap(), Some(Step::Drag { hold: false, .. })));
        assert!(matches!(p("drag 1 2 30 40 shift hold").unwrap(), Some(Step::Drag { hold: true, mods, .. }) if mods.shift));
        assert!(matches!(p("drag \"Clip\"#2 30 40").unwrap(), Some(Step::Drag { from: ui_script::Target::Label(_, 2), to: (30.0, 40.0), .. })));
        assert_eq!(p("release").unwrap(), Some(Step::Release));
        assert!(matches!(p("scroll 5 5 -40 alt").unwrap(), Some(Step::Scroll { dy, .. }) if dy == -40.0));
        assert!(matches!(p("key Enter").unwrap(), Some(Step::Key { key: Key::Enter, .. })));
        assert!(matches!(p("key esc").unwrap(), Some(Step::Key { key: Key::Escape, .. })));
        assert_eq!(p("text \"a \\\"b\\\" c\"").unwrap(), Some(Step::Text("a \"b\" c".into())));
        assert_eq!(p("wait 0.5").unwrap(), Some(Step::Wait(0.5)));
        assert_eq!(p("shot home").unwrap(), Some(Step::Shot("home".into())));
        for bad in ["click 1", "click a b", "key Nonsense", "click 1 2 hyper", "shot ../x", "jump 3", "text \"open", "home now"] {
            assert!(p(bad).is_err(), "{bad} should be refused");
        }
    }

    #[test]
    fn a_script_opens_a_project_clicks_a_menu_and_the_picture_is_2x_and_not_blank() {
        let _s = serial();
        let dir = scratch("menu", false);
        let out = std::env::temp_dir().join(format!("zornov-ui-script-{}-menu-out", std::process::id()));
        // Where the File menu is, from the same layout at the same size.
        let mut h = top_bar(1440.0);
        let at = bar_text(&mut h, "File").expect("the File menu");
        let mut s = session(&out, &["--size", "1440x900"]);
        let script = format!("open \"{}\"\nframes 6\nshot before\nclick {} {}\nframes 4\nshot after\nhome\nshot home\n", dir.display(), at.center().x, at.center().y);
        let report = ui_script::run_text(&mut s, &script, &out, "test");
        assert!(report.failures.is_empty(), "{:?}", report.failures);
        assert_eq!(report.shots.len(), 3);
        let (bw, bh, before) = png(&out.join("before.png"));
        assert_eq!((bw, bh), (2880, 1800), "1440x900 points at 2x");
        let (_, _, after) = png(&out.join("after.png"));
        let (_, _, home) = png(&out.join("home.png"));
        for (name, px) in [("before", &before), ("after", &after), ("home", &home)] {
            assert!(colours(px) > 12, "{name} is not blank: {} colours", colours(px));
        }
        assert_ne!(before, after, "the menu opened and is in the picture");
        assert_ne!(before, home, "home is another screen");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&out);
    }

    #[test]
    fn a_label_is_clicked_like_its_coordinates_and_a_miss_is_logged() {
        let _s = serial();
        let out = std::env::temp_dir().join(format!("zornov-ui-script-{}-label-out", std::process::id()));
        let mut s = session(&out, &["--size", "1440x900", "--screen", "editor"]);
        let before = s.app.overlay;
        let report = ui_script::run_text(&mut s, "click \"File\"\nframes 3\nclick \"Settings…\"\nframes 3\nclick \"No such thing\"\nclick \"File\"#9\nshot after\n", &out, "l.ui");
        assert_eq!(s.app.overlay, crate::app::Overlay::Settings, "File ▸ Settings… opened by name (was {before:?})");
        assert_eq!(report.failures.len(), 2, "{:?}", report.failures);
        assert!(report.log.iter().filter(|l| l.starts_with("failed")).count() == 2 && report.log.last().unwrap().starts_with("ok") && report.shots.len() == 1, "{:?}", report.log);
        let _ = std::fs::remove_dir_all(&out);
    }

    #[test]
    fn a_held_drag_keeps_the_button_down_until_the_shot_or_a_release() {
        let _s = serial();
        let out = std::env::temp_dir().join(format!("zornov-ui-script-{}-hold-out", std::process::id()));
        let mut s = session(&out, &["--size", "800x600"]);
        let down = |s: &Session| s.ctx.input(|i| i.pointer.primary_down());
        let r = ui_script::run_text(&mut s, "drag 100 100 300 200 hold\n", &out, "h");
        assert!(r.failures.is_empty() && down(&s), "{:?}", r.failures);
        let r = ui_script::run_text(&mut s, "shot held\n", &out, "h");
        assert!(r.failures.is_empty() && !down(&s), "the shot is taken, then the button is let go");
        ui_script::run_text(&mut s, "drag 100 100 300 200 hold\nrelease\n", &out, "h");
        assert!(!down(&s));
        let r = ui_script::run_text(&mut s, "release\n", &out, "h");
        assert_eq!(r.failures.len(), 1, "nothing is held");
        let _ = std::fs::remove_dir_all(&out);
    }

    #[test]
    fn a_failed_step_is_reported_with_its_line_and_the_run_goes_on() {
        let _s = serial();
        let out = std::env::temp_dir().join(format!("zornov-ui-script-{}-fail-out", std::process::id()));
        let mut s = session(&out, &["--size", "800x600"]);
        let report = ui_script::run_text(&mut s, "open /nonexistent/zornov\nclick 5000 5 \nfrobnicate\nshot after\n", &out, "f.ui");
        assert_eq!(report.failures.len(), 3, "{:?}", report.failures);
        assert!(report.failures[0].starts_with("f.ui:1:") && report.failures[2].starts_with("f.ui:3:"));
        assert_eq!(report.shots.len(), 1, "the last step still ran");
        let _ = std::fs::remove_dir_all(&out);
    }

    #[test]
    fn the_viewer_shows_a_frame_the_gpu_rendered_when_a_clip_is_on_the_timeline() {
        let _s = serial();
        let dir = scratch("viewer", true);
        let out = std::env::temp_dir().join(format!("zornov-ui-script-{}-viewer-out", std::process::id()));
        let mut s = session(&out, &["--size", "1440x900"]);
        if !s.on_gpu() {
            eprintln!("no GPU here: {:?}", s.gpu_note);
            return;
        }
        let report = ui_script::run_text(&mut s, &format!("open \"{}\"\nshot clip\n", dir.display()), &out, "test");
        assert!(report.failures.is_empty(), "{:?}", report.failures);
        assert!(!s.app.doc.project.clips.is_empty(), "a clip is on the timeline");
        assert!(s.viewer_frames() > 0, "the preview renderer sent the viewer a frame");
        if let Some(keep) = std::env::var_os("ZORNOV_KEEP_SHOTS") {
            let _ = std::fs::copy(out.join("clip.png"), std::path::Path::new(&keep).join("clip.png"));
        }
        let (w, h, px) = png(&out.join("clip.png"));
        // The middle of the window is the viewer: a picture there has many colours.
        let (x0, x1, y0, y1) = (w as usize * 3 / 10, w as usize * 7 / 10, h as usize * 2 / 10, h as usize * 5 / 10);
        let mut seen = std::collections::HashSet::new();
        for y in (y0..y1).step_by(3) {
            for x in (x0..x1).step_by(3) {
                let i = (y * w as usize + x) * 4;
                seen.insert([px[i] / 4, px[i + 1] / 4, px[i + 2] / 4]);
            }
        }
        assert!(seen.len() > 24, "the viewer area holds a rendered picture: {} colours", seen.len());
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&out);
    }
}

// --- app.rs hooks: quit guard, toasts off the editor, export failures ------

mod hooks {
    use super::*;

    fn painted(h: &Harness, text: &str) -> usize {
        fn walk(shape: &egui::epaint::Shape, text: &str) -> usize {
            match shape {
                egui::epaint::Shape::Text(t) if t.galley.text() == text => 1,
                egui::epaint::Shape::Vec(v) => v.iter().map(|s| walk(s, text)).sum(),
                _ => 0,
            }
        }
        h.last.as_ref().map_or(0, |o| o.shapes.iter().map(|cs| walk(&cs.shape, text)).sum())
    }

    #[test]
    fn closing_the_window_with_unsaved_changes_asks_first() {
        let mut h = Harness::new(&["--screen", "editor"]).sized(1440.0, 900.0);
        h.frames(4);
        assert!(h.close_requested().is_empty() || !h.app.doc.dirty(), "a clean project closes without asking");
        h.app.doc.edit("Rename", |p| p.name = "Changed".into());
        assert!(h.app.doc.dirty());
        let cmds = h.close_requested();
        assert!(cmds.iter().any(|c| matches!(c, egui::ViewportCommand::CancelClose)), "closing is held back: {cmds:?}");
        h.frames(3);
        assert!(h.labels().iter().any(|l| l == "Don't save"), "the Save changes? dialog is up: {:?}", h.labels());
    }

    #[test]
    fn a_toast_shows_on_the_start_screen() {
        let mut h = Harness::new(&[]).sized(1440.0, 900.0);
        h.frames(3);
        assert_eq!(h.app.screen, crate::cli::Screen::Start);
        crate::kit::push(&h.ctx, "Toast on start", None, Some(crate::icons::Icon::Info));
        h.frames(3);
        assert_eq!(painted(&h, "Toast on start"), 1);
    }

    #[test]
    fn the_editor_draws_a_toast_once() {
        let mut h = Harness::new(&["--screen", "editor"]).sized(1440.0, 900.0);
        h.frames(4);
        crate::kit::push(&h.ctx, "Toast in editor", None, Some(crate::icons::Icon::Info));
        h.frames(3);
        assert_eq!(painted(&h, "Toast in editor"), 1, "one toast, not two");
    }

    #[test]
    fn a_failed_export_is_worded_as_couldnt_export() {
        let src = include_str!("app.rs");
        assert!(src.contains("Couldn't export {}: {e}") && !src.contains("\"{} failed: {e}\""));
    }
}
