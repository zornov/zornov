// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Content that runs out of room, checked headless at small and large
//! window sizes: menus and popups fit the window and scroll, and no text is
//! cut off with no way to see the rest (it wraps, scrolls, or ends in an
//! ellipsis with the whole of it on hover).
//!
//! `STUDIO_UI_SHOTS=<dir> cargo test -p studio-app --bin studio ui_overflow -- --ignored`
//! also saves a picture of every surface at every size, with a report of
//! the text it found cut, for looking at by eye.

use crate::app::Overlay;
use crate::ui_tests::Harness;
use eframe::egui::{self, accesskit, Event, Key, Modifiers, PointerButton, Rect};

/// Window sizes checked: small laptop to a large display.
pub const SIZES: [(f32, f32); 5] = [(960.0, 600.0), (1024.0, 640.0), (1280.0, 720.0), (1440.0, 900.0), (2000.0, 1300.0)];

impl Harness {
    /// Where the control a screen reader calls `label` is (exact name first,
    /// then the first name that starts with it).
    pub fn find(&self, role: Option<accesskit::Role>, label: &str) -> Option<Rect> {
        let update = self.last.as_ref()?.platform_output.accesskit_update.as_ref()?;
        let nodes: Vec<_> = update.nodes.iter().filter(|(_, n)| role.is_none_or(|r| n.role() == r)).filter(|(id, _)| self.on_top(id.0)).filter_map(|(_, n)| Some((n.label().or(n.value()).unwrap_or("").to_owned(), n.bounds()?))).collect();
        let hit = nodes.iter().find(|(l, _)| l == label)?;
        let b = hit.1;
        Some(Rect::from_min_max(egui::pos2(b.x0 as f32, b.y0 as f32), egui::pos2(b.x1 as f32, b.y1 as f32)))
    }

    /// Whether the widget with this AccessKit id is on the topmost layer at
    /// its middle (not behind a dialog or sheet), so a click reaches it.
    pub fn on_top(&self, node: u64) -> bool {
        let found = self.ctx.viewport(|v| v.this_pass.widgets.layers().flat_map(|(_, l)| l.iter()).find(|w| w.id.value() == node).map(|w| (w.layer_id, w.interact_rect)));
        match found {
            Some((layer, r)) if r.width() > 0.0 && r.height() > 0.0 => self.ctx.layer_id_at(r.center()) == Some(layer),
            Some(_) => false,
            // Not a widget egui placed itself (a container): let it be.
            None => true,
        }
    }

    /// A click in the middle of `at`, then a few frames for it to settle.
    pub fn click_at(&mut self, at: Rect) {
        let pos = at.center();
        self.with_events(vec![Event::PointerMoved(pos)]);
        self.with_events(vec![Event::PointerButton { pos, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE }]);
        self.with_events(vec![Event::PointerButton { pos, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE }]);
        self.frames(12);
    }

    /// Where text reading exactly `text` was painted last (on top).
    pub fn find_text(&self, text: &str) -> Option<Rect> {
        fn walk(shape: &egui::epaint::Shape, text: &str, hit: &mut Option<Rect>) {
            match shape {
                egui::epaint::Shape::Text(t) if t.galley.text() == text => *hit = Some(t.galley.rect.translate(t.pos.to_vec2())),
                egui::epaint::Shape::Vec(v) => v.iter().for_each(|s| walk(s, text, hit)),
                _ => {}
            }
        }
        let mut hit = None;
        for cs in &self.last.as_ref()?.shapes {
            walk(&cs.shape, text, &mut hit);
        }
        // Not text behind a dialog or sheet: their backdrops cover the
        // whole window.
        let screen = self.ctx.content_rect();
        hit.filter(|r: &Rect| {
            self.ctx.layer_id_at(r.center()).is_some_and(|l| l.order == egui::Order::Background || !self.ctx.memory(|m| m.area_rect(l.id)).is_some_and(|a| a.expand(1.0).contains_rect(screen)))
        })
    }

    /// Click the control named `label` (or, with no such name, the text
    /// that reads `label`). False when there is none.
    pub fn click(&mut self, label: &str) -> bool {
        match self.find(None, label).or_else(|| self.find_text(label)) {
            Some(r) => {
                self.click_at(r);
                true
            }
            None => false,
        }
    }

    /// The wheel turned over `at` (positive `dy` scrolls the content up).
    pub fn wheel(&mut self, at: egui::Pos2, dy: f32) {
        self.with_events(vec![Event::PointerMoved(at)]);
        for _ in 0..30 {
            self.with_events(vec![Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: egui::vec2(0.0, -dy), phase: egui::TouchPhase::Move, modifiers: Modifiers::NONE, source: egui::MouseWheelSource::Wheel }]);
        }
        self.frames(20);
    }

    pub fn escape(&mut self) {
        self.key(Key::Escape, Modifiers::NONE);
        self.frames(12);
    }
}

/// A surface to look at: how to get there from a fresh app.
pub struct Surface {
    pub name: &'static str,
    pub args: &'static [&'static str],
    pub setup: fn(&mut Harness),
    /// Controls clicked in turn after the setup (by name or by their text).
    pub clicks: &'static [&'static str],
}

fn nothing(_: &mut Harness) {}

fn select(h: &mut Harness, kind: crate::view::ClipKind) {
    if let Some(c) = h.app.doc.project.clips.iter().find(|c| c.kind == kind) {
        h.app.editor.selection = vec![c.id];
    }
    // Narrow windows show the Inspector as a sheet.
    h.app.editor.sheet_open = true;
}

fn select_first_clip(h: &mut Harness) {
    select(h, crate::view::ClipKind::Video);
}

/// The project saved in a folder of its own, so Safety and Rights have
/// something to show.
pub fn saved_project(h: &mut Harness) {
    let dir = std::env::temp_dir().join(format!("studio-overflow-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let f = studio_model::file::ProjectFolder::create(&dir).expect("a project folder");
    f.save(&h.app.doc.model).expect("saved");
    h.app.doc.folder = Some(f);
}

/// A short screenplay with long scene headings and names, broken down,
/// scheduled and with tasks, so every Production tab has rows to show.
pub fn production_script(h: &mut Harness) {
    saved_project(h);
    let script = "INT. THE VERY LONG CORRIDOR OUTSIDE THE ORBITAL STATION'S HYDROPONICS DECK - NIGHT\n\nMARGUERITE DELACROIX-WHITFIELD walks with a lantern, a battered suitcase and a stack of letters.\n\nMARGUERITE DELACROIX-WHITFIELD\nWe should have left before the storm.\n\nEXT. RAILWAY PLATFORM ON THE EDGE OF THE SALT FLATS AT THE END OF THE WORLD - DAY\n\nBARTHOLOMEW OKONKWO-HARRINGTON waits beside a vintage motorcycle with a sidecar and two dogs.\n\nBARTHOLOMEW OKONKWO-HARRINGTON\nYou came back.\n";
    let placeholder = crate::doc::Doc::new(h.app.doc.model.clone());
    let mut session = crate::agent::Session::new(std::mem::replace(&mut h.app.doc, placeholder));
    session.home = session.doc.folder.as_ref().map(|f| f.root().to_path_buf());
    let calls = [
        ("story", serde_json::json!({"do": "import", "text": script, "format": "fountain"})),
        ("production", serde_json::json!({"do": "breakdown"})),
        ("production", serde_json::json!({"do": "schedule", "start": "2026-11-02"})),
        ("production", serde_json::json!({"do": "asset_tasks"})),
    ];
    for (tool, args) in calls {
        if let Err(e) = session.call(tool, &args) {
            eprintln!("survey setup: {tool} {args}: {e}");
        }
    }
    h.app.doc = session.doc;
    h.app.overlay = Overlay::Production;
}

/// Review, opened as a panel (View > Panels, or the Review workspace).
fn open_review(h: &mut Harness) {
    let ctx = h.ctx.clone();
    crate::editor::dock_ui::open_panel(&mut h.app, &ctx, crate::dock::Panel::Review, false);
}

/// Paint with a picture open and every side section unfolded.
pub fn paint_picture(h: &mut Harness) {
    let _ = crate::paint::apply(&[serde_json::json!({"op": "new", "name": "Overflow", "width": 683, "height": 1024, "background": "white"})]);
    for _ in 0..200 {
        h.frames(1);
        if crate::screens::paint::idle() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    h.frames(4);
    for section in ["Adjustments", "Animation", "History"] {
        h.click(section);
    }
}

/// Every surface the survey visits: screens, overlays, panels.
pub fn surfaces() -> Vec<Surface> {
    use crate::view::ClipKind;
    const ED: &[&str] = &["--screen", "editor"];
    let s = |name, args, setup: fn(&mut Harness), clicks| Surface { name, args, setup, clicks };
    vec![
        s("start", &[], nothing, &[]),
        s("editor", ED, nothing, &[]),
        s("editor-clip", ED, select_first_clip, &[]),
        s("insp-video-audio", ED, select_first_clip, &["Audio"]),
        s("insp-video-speed", ED, select_first_clip, &["Speed"]),
        s("insp-video-animation", ED, select_first_clip, &["Animation"]),
        s("insp-video-adjust", ED, select_first_clip, &["Adjust"]),
        s("insp-title", ED, |h| select(h, ClipKind::Title), &[]),
        s("insp-title-animation", ED, |h| select(h, ClipKind::Title), &["Animation"]),
        s("insp-audio", ED, |h| select(h, ClipKind::Audio), &[]),
        s("insp-audio-speed", ED, |h| select(h, ClipKind::Audio), &["Speed"]),
        s("insp-fx", ED, |h| select(h, ClipKind::Fx), &[]),
        s("sound", ED, nothing, &["More", "Sound"]),
        s("review", ED, open_review, &[]),
        s("review-saved", ED, |h| {
            saved_project(h);
            open_review(h);
        }, &[]),
        s("compositor", ED, select_first_clip, &["Nodes"]),
        s("graph", ED, select_first_clip, &["Curves"]),
        s("vector", ED, nothing, &["Vector art: draw logos, icons, illustrations and infographics"]),
        s("capture", ED, nothing, &["Record, voice-over, prompter and live"]),
        s("viewport-3d", ED, nothing, &["3D workspace"]),
        s("scopes", ED, nothing, &["Scopes"]),
        s("mograph", ED, nothing, &["Motion graphics: templates, shapes, charts, 3D text"]),
        s("annotate", ED, nothing, &["Annotate for the AI"]),
        s("editorial", ED, nothing, &["More", "Editorial"]),
        s("library", ED, |h| h.app.overlay = Overlay::Library, &[]),
        s("library-saved", ED, |h| {
            saved_project(h);
            h.app.overlay = Overlay::Library;
        }, &[]),
        s("export", ED, |h| h.app.overlay = Overlay::Export, &[]),
        s("settings", ED, |h| h.app.overlay = Overlay::Settings, &[]),
        s("production", ED, |h| h.app.overlay = Overlay::Production, &[]),
        s("production-saved", ED, |h| {
            saved_project(h);
            h.app.overlay = Overlay::Production;
        }, &[]),
        s("production-script", ED, production_script, &[]),
        s("help", ED, |h| h.app.overlay = Overlay::Help, &[]),
        s("gallery", &["--screen", "gallery"], nothing, &[]),
        s("paint", &["--screen", "paint"], nothing, &[]),
        s("paint-picture", &["--screen", "paint"], paint_picture, &[]),
        s("paint-commands", &["--screen", "paint"], paint_picture, &["Commands…"]),
    ]
}

fn slug(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect::<String>().split('-').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("-")
}

/// One look: the picture (when asked for) and the text found cut.
fn look(h: &Harness, dir: Option<&std::path::Path>, name: &str, report: &mut Vec<String>) {
    let (w, hh) = (h.size.x as u32, h.size.y as u32);
    if let Some(dir) = dir {
        h.shot(dir.join(format!("{w}x{hh}")).join(format!("{name}.png")));
    }
    for o in h.overlaps() {
        report.push(format!("{w}x{hh}\t{name}\tOVERLAP {:?} / {:?}\t{:.0},{:.0}-{:.0},{:.0}\t{:.0},{:.0}-{:.0},{:.0}", o.a, o.b, o.ra.min.x, o.ra.min.y, o.ra.max.x, o.ra.max.y, o.rb.min.x, o.rb.min.y, o.rb.max.x, o.rb.max.y));
    }
    for c in h.cut_text() {
        report.push(format!("{w}x{hh}\t{name}\t{:?}\ttext {:.0},{:.0}-{:.0},{:.0}\tclip {:.0},{:.0}-{:.0},{:.0}", c.text.chars().take(70).collect::<String>(), c.rect.min.x, c.rect.min.y, c.rect.max.x, c.rect.max.y, c.clip.min.x, c.clip.min.y, c.clip.max.x, c.clip.max.y));
    }
}

/// The controls that open something (tabs, menus, drop-downs) on a surface.
fn openers(h: &Harness) -> Vec<(accesskit::Role, String)> {
    use accesskit::Role::*;
    let mut seen = std::collections::BTreeSet::new();
    let Some(update) = h.last.as_ref().and_then(|o| o.platform_output.accesskit_update.as_ref()) else { return vec![] };
    update
        .nodes
        .iter()
        .filter(|(id, _)| h.on_top(id.0))
        .filter(|(_, n)| matches!(n.role(), Tab | ComboBox) || (n.role() == Button && (n.is_selected().is_some() || n.toggled().is_some())))
        .map(|(_, n)| (n.role(), n.label().or(n.value()).unwrap_or("").to_owned()))
        .filter(|(_, l)| !l.is_empty())
        .filter(|x| seen.insert(x.clone()))
        .collect()
}

/// Named buttons that open a menu or popover.
const MENU_BUTTONS: &[&str] = &["File", "Edit", "Clip", "View", "Help", "Layout", "Background jobs", "Overlays", "Add", "More", "Safety", "Rights"];

/// Visit every surface at every size, open each tab, drop-down and menu on
/// it, and collect what is cut off.
pub fn survey(dir: Option<&std::path::Path>, only: Option<&str>) -> Vec<String> {
    let mut report = vec![];
    for (w, hh) in SIZES {
        for s in surfaces() {
            if only.is_some_and(|o| !s.name.starts_with(o)) {
                continue;
            }
            let _paint = s.name.starts_with("paint").then(crate::paint::test_lock);
            let _viewport = crate::editor::viewport::test_lock();
            let mut h = Harness::new(s.args).sized(w, hh);
            (s.setup)(&mut h);
            h.frames(8);
            for c in s.clicks {
                h.click(c);
            }
            look(&h, dir, s.name, &mut report);
            for (role, label) in openers(&h) {
                let Some(r) = h.find(Some(role), &label) else { continue };
                h.click_at(r);
                look(&h, dir, &format!("{}--{}", s.name, slug(&label)), &mut report);
                // A second click closes a drop-down (Esc would also close
                // a sheet under it).
                if role == accesskit::Role::ComboBox {
                    h.click_at(r);
                }
            }
            for m in MENU_BUTTONS {
                if h.click(m) {
                    look(&h, dir, &format!("{}--menu-{}", s.name, slug(m)), &mut report);
                    // Menus close with a second click; tabs (Safety,
                    // Rights) stay.
                    if crate::kit::any_popover_open(&h.ctx) {
                        h.click(m);
                    }
                }
            }
            if s.name.starts_with("paint") {
                let _ = crate::paint::apply(&[serde_json::json!({"op": "close"})]);
            }
            // Panels that keep their open state between windows (Record,
            // Vector art) close again with the button that opened them.
            for c in s.clicks.iter().rev() {
                h.click(c);
            }
            if let Some(dir) = dir {
                let _ = std::fs::create_dir_all(dir);
                let _ = std::fs::write(dir.join("cut-text.tsv"), report.join("\n"));
            }
        }
    }
    report
}

#[test]
#[ignore = "pictures of every surface at every size: STUDIO_UI_SHOTS=<dir>"]
fn ui_overflow_survey() {
    // Never the person's own asset library: a scratch one unless told.
    if std::env::var_os("STUDIO_ASSETS_DB").is_none() {
        // SAFETY: an ignored test run on its own, before the library opens.
        unsafe { std::env::set_var("STUDIO_ASSETS_DB", std::env::temp_dir().join(format!("studio-overflow-assets-{}.db", std::process::id()))) };
    }
    let dir = std::env::var_os("STUDIO_UI_SHOTS").map(std::path::PathBuf::from);
    let only = std::env::var("STUDIO_UI_ONLY").ok();
    let report = survey(dir.as_deref(), only.as_deref());
    if let Some(dir) = &dir {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("cut-text.tsv"), report.join("\n")).unwrap();
    }
    eprintln!("{} cut pieces of text", report.len());
}

/// Surfaces held to "nothing cut off, nothing drawn over" in the fast
/// tests. (Library ▸ Rights reads the shared asset database, which one
/// other test in this binary owns; the survey checks it on a scratch copy.)
const CHECKED: &[&str] = &["start", "editor", "editor-clip", "insp-video-animation", "insp-title-animation", "insp-audio", "export", "settings", "sound", "review-saved", "production-script", "help", "paint-picture", "editorial"];

#[test]
fn nothing_is_cut_off_or_drawn_over_at_small_and_large_sizes() {
    let mut visited = 0;
    for (w, hh) in [(960.0, 600.0), (1440.0, 900.0)] {
        for s in surfaces().into_iter().filter(|s| CHECKED.contains(&s.name)) {
            visited += 1;
            // A thread of its own for each, so what one surface leaves in
            // thread-local state (the Review panel's, say) does not reach
            // the next.
            std::thread::scope(|scope| {
                scope
                    .spawn(|| {
                        let _paint = s.name.starts_with("paint").then(crate::paint::test_lock);
                        let _viewport = crate::editor::viewport::test_lock();
                        let mut h = Harness::new(s.args).sized(w, hh);
                        (s.setup)(&mut h);
                        h.frames(8);
                        for c in s.clicks {
                            assert!(h.click(c), "{}: {c} can be clicked", s.name);
                        }
                        let (cuts, over) = (h.cut_text(), h.overlaps());
                        if s.name.starts_with("paint") {
                            let _ = crate::paint::apply(&[serde_json::json!({"op": "close"})]);
                        }
                        for c in s.clicks.iter().rev() {
                            h.click(c);
                        }
                        assert!(cuts.is_empty(), "{} at {w}×{hh}: text cut off with no way to see it: {cuts:#?}", s.name);
                        assert!(over.is_empty(), "{} at {w}×{hh}: drawn over each other: {over:#?}", s.name);
                    })
                    .join()
                    .unwrap_or_else(|e| std::panic::resume_unwind(e));
            });
        }
    }
    assert_eq!(visited, 2 * CHECKED.len(), "every listed surface was looked at");
}

/// The viewer's Add menu (screenshot case: its 3D menu ran past the window's
/// bottom): it stays inside the window and scrolls to its last item.
#[test]
fn a_menu_taller_than_the_window_fits_it_and_scrolls_to_the_end() {
    let last = "Add a note";
    for (w, hh) in [(960.0, 600.0), (1440.0, 900.0), (2000.0, 1300.0)] {
        let _viewport = crate::editor::viewport::test_lock();
        let mut h = Harness::new(&["--screen", "editor"]).sized(w, hh);
        h.frames(6);
        let add = egui::Id::unique("viewer-add");
        crate::kit::set_open(&h.ctx, add, true);
        h.frames(6);
        let area = h.ctx.memory(|m| m.area_rect(add.with("area"))).expect("the Add menu's area");
        let screen = Rect::from_min_size(egui::Pos2::ZERO, h.size);
        assert!(screen.contains_rect(area), "the menu {area:?} is inside the window {screen:?}");
        let visible = |h: &Harness| h.find_text(last).is_some_and(|r| area.contains_rect(r));
        // Longer than its room, so the end starts out of view.
        assert!(!visible(&h), "{last} starts out of view at {w}×{hh}");
        h.wheel(area.center(), 400.0);
        h.wheel(area.center(), 400.0);
        assert!(visible(&h), "wheeling brings {last} into view at {w}×{hh}");
        assert!(h.cut_text().is_empty(), "{:#?}", h.cut_text());
    }
}

/// egui's own menus and sub-menus fit the window the same way.
#[test]
fn popup_body_caps_to_the_window() {
    let ctx = egui::Context::default();
    let screen = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 500.0));
    let mut seen = 0.0;
    for frame in 0..6 {
        let input = egui::RawInput { screen_rect: Some(screen), time: Some(frame as f64 / 60.0), ..Default::default() };
        crate::editor::test_frame(&ctx, input, |ui| {
            egui::Area::new(egui::Id::unique("popup-body-test")).fixed_pos(egui::pos2(10.0, 100.0)).show(ui.ctx(), |ui| {
                crate::kit::popup_body(ui, |ui| {
                    for i in 0..80 {
                        ui.label(format!("item {i}"));
                    }
                });
                seen = ui.min_rect().height();
            });
        });
    }
    assert!(seen <= 500.0 - 100.0 - crate::kit::EDGE + 1.0, "capped to the room below: {seen}");
    assert!(seen > 300.0, "and uses it: {seen}");
}

/// A timeline too narrow for its tools scrolls them sideways (the wheel
/// brings the last one into reach) instead of clipping them.
#[test]
fn a_narrow_timeline_toolbar_scrolls_instead_of_clipping() {
    let _viewport = crate::editor::viewport::test_lock();
    let mut h = Harness::new(&["--screen", "editor"]).sized(520.0, 700.0);
    h.frames(8);
    let first = h.find(None, "Select (drag empty space to box-select)").expect("the first tool");
    let strips: Vec<_> = crate::kit::scrolled_rects(&h.ctx).into_iter().filter(|(r, horiz)| *horiz && r.contains(first.center())).collect();
    assert!(!strips.is_empty(), "the tools sit in a sideways scroll strip");
    let strip = strips[0].0;
    h.with_events(vec![Event::PointerMoved(strip.center())]);
    for _ in 0..10 {
        h.with_events(vec![Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: egui::vec2(-60.0, 0.0), phase: egui::TouchPhase::Move, modifiers: Modifiers::NONE, source: egui::MouseWheelSource::Wheel }]);
    }
    h.frames(20);
    // Scrolled out of the strip's left edge, or at least moved along.
    let moved = h.find(None, "Select (drag empty space to box-select)");
    assert!(moved.is_none_or(|m| m.left() < first.left() - 20.0), "wheeling scrolls the tools: {first:?} -> {moved:?}");
}
