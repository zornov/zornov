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

pub struct Harness {
    _serial: std::sync::MutexGuard<'static, ()>,
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
        let serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
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
        crate::ui_shot::cut_text(&self.ctx, self.last.as_ref().expect("a frame"), &[], 1.0)
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
    for want in ["Export", "Settings", "Layout"] {
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
    assert_eq!(h.app.prefs.workspace, "Production");
    assert!(!h.app.editor.panel_open(crate::dock::Panel::Inspector));
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
    assert!(said.contains("Welcome"), "{said}");
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
