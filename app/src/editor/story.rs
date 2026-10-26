// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The Story section of the library: the shot list as a storyboard (scene
//! by scene, a card a shot), the script (written and edited here, line by
//! line with speakers, times and notes, or as one piece of text; imported
//! from files or dropped on the panel), the places, the character bible and what the plan gets wrong.
//! Built shots follow the timeline (trim a clip and its shot follows);
//! clicking a shot takes the playhead there and selects its clips.

use crate::app::StudioApp;
use crate::icons::Icon;
use crate::kit::{self, Button, IconButton};
use crate::theme;
use eframe::egui::{self, Color32, CornerRadius, Rect, RichText, Stroke};
use serde_json::{Value, json};
use studio_model as m;
use studio_model::story::{self as st, fountain, import, times};
use std::path::Path;
use crate::kit::ShowNoted;

/// The categories of the section.
pub const GROUPS: &[&str] = &["Shots", "Script", "Places", "Cast", "Check"];

fn secs(t: m::Micros) -> f64 {
    t as f64 / m::SECOND as f64
}

/// The story with built shots read back from the timeline.
fn now(app: &StudioApp) -> st::Story {
    let mut s = app.doc.model.story.clone();
    if let Some(seq) = app.doc.model.sequence(app.doc.seq()) {
        s.sync(seq);
    }
    s
}

/// Runs a `story` call of the agent on a copy of the project and plays
/// what it changed onto the window's document as one undo step.
fn run(app: &mut StudioApp, ctx: &egui::Context, args: Value, label: &str) {
    let mut doc = crate::doc::Doc::new(app.doc.model.clone());
    doc.folder = app.doc.folder.clone();
    let mut s = crate::agent::Session::new(doc);
    let n = s.doc.history.snapshot().undo.len();
    let r = s.call("story", &args);
    let msg = match r {
        Err(e) => Some(e),
        Ok(_) => match crate::agent::live::forward(&mut s.doc, n) {
            None => Some("the change could not be applied".into()),
            Some(edits) if edits.is_empty() => None,
            Some(edits) => {
                app.doc.begin_batch(label);
                let mut err = None;
                for (l, e) in edits {
                    if let Err(x) = app.doc.apply_replayed(&l, e) {
                        err = Some(x);
                        break;
                    }
                }
                if err.is_some() {
                    app.doc.rollback_batch();
                } else {
                    app.doc.commit_batch();
                }
                err
            }
        },
    };
    let text = match msg {
        Some(e) => e,
        None => format!("{label} done"),
    };
    kit::push(ctx, text, None, None);
}

fn set_story(app: &mut StudioApp, story: st::Story, label: &str) {
    if story != app.doc.model.story {
        app.doc.apply(label, m::Edit::SetStory { story: Box::new(story) });
    }
}

/// Edits one shot as a gesture (typing or dragging is one undo step).
fn edit_shot(app: &mut StudioApp, id: &str, f: impl FnOnce(&mut st::Shot)) {
    let mut s = now(app);
    if let Some((si, i)) = s.find_shot(id) {
        f(&mut s.scenes[si].shots[i]);
        if s != app.doc.model.story {
            app.doc.apply_gesture(&format!("story-shot-{id}"), "Shot", m::Edit::SetStory { story: Box::new(s) });
        }
    }
}

fn kind_colour(k: st::ShotKind, t: &theme::Tokens) -> Color32 {
    match k.tightness() {
        0 => Color32::from_rgb(70, 130, 180),
        1 => Color32::from_rgb(80, 160, 140),
        2 | 3 => Color32::from_rgb(170, 140, 60),
        4 => Color32::from_rgb(190, 90, 70),
        _ => t.danger,
    }
}

fn toolbar(app: &mut StudioApp, ui: &mut egui::Ui, has: bool) {
    let ctx = ui.ctx().clone();
    ui.horizontal_wrapped(|ui| {
        if Button::secondary("Import script").icon(Icon::DownloadSimple).compact().show(ui).clicked() {
            import_dialog(app, &ctx);
        }
        if has {
            if Button::secondary("Plan shots").icon(Icon::FilmSlate).compact().show(ui).clicked() {
                let mut s = now(app);
                let done = s.plan(false);
                if done.is_empty() {
                    kit::push(&ctx, "Every scene already has shots".to_owned(), None, None);
                } else {
                    set_story(app, s, "Plan shots");
                }
            }
            if Button::primary("Build timeline").compact().show(ui).clicked() {
                run(app, &ctx, json!({"do": "build"}), "Build from story");
            }
            if Button::ghost("Export").compact().show(ui).clicked() {
                if let Some(path) = rfd::FileDialog::new().set_title("Export the script").set_file_name(format!("{}.fountain", app.doc.project.name)).add_filter("Fountain", &["fountain"]).add_filter("Text", &["txt"]).add_filter("Subtitles", &["srt"]).save_file() {
                    run(app, &ctx, json!({"do": "export", "path": path.display().to_string()}), "Export script");
                }
            }
        }
    });
}

/// The Story section's body.
pub fn panel(app: &mut StudioApp, ui: &mut egui::Ui, chip: usize, body: Rect) {
    let t = theme::tokens(ui.ctx());
    let ctx = ui.ctx().clone();
    take_dropped_script(app, &ctx, body);
    let mut bui = ui.new_child(egui::UiBuilder::new().max_rect(body));
    bui.set_clip_rect(body.intersect(ui.clip_rect()));
    let story = now(app);
    if story.scenes.is_empty() && story.cast.is_empty() && story.places.is_empty() {
        let pad = theme::space::PANEL_PAD;
        egui::ScrollArea::vertical().id_salt(("story-empty", chip)).auto_shrink([false, false]).show_v(&mut bui, |ui| {
            egui::Frame::new().inner_margin(egui::Margin { left: pad as i8, right: pad as i8, top: 6, bottom: 8 }).show(ui, |ui| {
                ui.label(RichText::new("Write or paste your story or narration below, or bring in a file (text, Markdown, Fountain, SRT, VTT, Word) or drop it here. Zornov breaks it into scenes and lines you can time, plan and build the timeline from.").font(theme::body()).color(t.text_2));
                ui.add_space(4.0);
                if Button::secondary("Import script").icon(Icon::DownloadSimple).compact().show(ui).clicked() {
                    import_dialog(app, &ctx);
                }
                ui.add_space(8.0);
                script_text(app, ui, &story, &t, &ctx);
            });
        });
        drop_hint(ui, &t, body);
        return;
    }
    let pad = theme::space::PANEL_PAD;
    egui::ScrollArea::vertical().id_salt(("story-scroll", chip)).auto_shrink([false, false]).show_v(&mut bui, |ui| {
        egui::Frame::new().inner_margin(egui::Margin { left: pad as i8, right: pad as i8, top: 2, bottom: 8 }).show(ui, |ui| {
            toolbar(app, ui, !story.scenes.is_empty());
            ui.add_space(4.0);
            let head = format!("{} · {} scenes · {} shots · {:.0}s planned", if story.title.is_empty() { "Story" } else { &story.title }, story.scenes.len(), story.timings().len(), secs(story.duration()));
            ui.label(RichText::new(head).font(theme::body_medium()).color(t.text));
            ui.add_space(4.0);
            match chip {
                0 => shots(app, ui, &story, &t),
                1 => script(app, ui, &story, &t, &ctx),
                2 => places(app, ui, &story, &t),
                3 => cast(app, ui, &story, &t),
                _ => check(app, ui, &story, &t),
            }
        });
    });
    drop_hint(ui, &t, body);
}

fn shots(app: &mut StudioApp, ui: &mut egui::Ui, story: &st::Story, t: &theme::Tokens) {
    let times = story.timings();
    let playhead = app.editor.playhead as m::Micros;
    let issues = story.check();
    for (si, sc) in story.scenes.iter().enumerate() {
        ui.add_space(6.0);
        let place = story.place_of(sc).map(|p| format!(" · {}{}", p.ambience.as_deref().map(|a| format!("{a} · ")).unwrap_or_default(), p.surface.as_deref().unwrap_or(""))).unwrap_or_default();
        ui.label(RichText::new(format!("{}  {}", sc.id, fountain::slug_line(story, sc).trim_start_matches('.'))).font(theme::body_medium()).color(t.text));
        if !sc.summary.is_empty() || !place.is_empty() {
            ui.label(RichText::new(format!("{}{}", sc.summary, place)).font(theme::caption()).color(t.text_2));
        }
        if sc.shots.is_empty() {
            ui.label(RichText::new("No shots yet: Plan shots").font(theme::caption()).color(t.warn));
            continue;
        }
        for (i, sh) in sc.shots.iter().enumerate() {
            let tm = times.iter().find(|x| x.scene == si && x.shot == i).copied();
            let start = tm.map(|x| x.start).unwrap_or(0);
            let here = playhead >= start && playhead < start + sh.duration;
            let problem = issues.iter().any(|x| x.shot.as_deref() == Some(sh.id.as_str()) && x.severity >= st::Severity::Warning);
            let frame = egui::Frame::new()
                .fill(if here { t.accent_soft } else { t.raised })
                .stroke(Stroke::new(1.0, if problem { t.warn } else { t.line }))
                .corner_radius(CornerRadius::same(theme::radius::CONTROL))
                .inner_margin(egui::Margin::same(6));
            let r = frame.show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal_wrapped(|ui| {
                    let k = kind_colour(sh.kind, t);
                    ui.label(RichText::new(format!(" {} ", sh.kind.abbrev())).font(theme::caption()).color(Color32::WHITE).background_color(k));
                    ui.label(RichText::new(&sh.id).font(theme::body_medium()).color(t.text));
                    let mut kind = sh.kind;
                    egui::ComboBox::from_id_salt(("shot-kind", &sh.id)).selected_text(kind.name().replace('_', " ")).width(110.0).show_ui(ui, |ui| {
                        crate::kit::note_list(ui);
                        for k in st::ShotKind::ALL {
                            ui.selectable_value(&mut kind, k, k.name().replace('_', " "));
                        }
                    });
                    if kind != sh.kind {
                        edit_shot(app, &sh.id, |x| x.kind = kind);
                        app.doc.end_gesture();
                    }
                    let mut cam = sh.camera;
                    egui::ComboBox::from_id_salt(("shot-cam", &sh.id)).selected_text(cam.name().replace('_', " ")).width(80.0).show_ui(ui, |ui| {
                        crate::kit::note_list(ui);
                        for c in st::CameraMove::ALL {
                            ui.selectable_value(&mut cam, c, c.name().replace('_', " "));
                        }
                    });
                    if cam != sh.camera {
                        edit_shot(app, &sh.id, |x| x.camera = cam);
                        app.doc.end_gesture();
                    }
                    let mut d = secs(sh.duration);
                    let mut d32 = d as f32;
                    let dr = kit::Slider::new("Duration", &mut d32, 0.1..=600.0).default(3.0).unit("s").decimals(1).logarithmic(true).with_value().width(160.0).show(ui);
                    d = d32 as f64;
                    if dr.changed() && sh.clips.is_empty() {
                        edit_shot(app, &sh.id, |x| x.duration = (d * m::SECOND as f64) as m::Micros);
                    }
                    if dr.drag_stopped() || dr.lost_focus() {
                        app.doc.end_gesture();
                    }
                    if !sh.clips.is_empty() {
                        ui.label(RichText::new("on timeline").font(theme::caption()).color(t.accent));
                    }
                });
                let who = if sh.subjects.is_empty() { String::new() } else { format!("{} · ", sh.subjects.join(" + ")) };
                let lines: Vec<String> = sh.lines.iter().filter_map(|l| sc.lines.get(*l)).map(|l| l.text.chars().take(90).collect()).collect();
                ui.label(RichText::new(format!("{who}{:.1}s{}", secs(start), lines.first().map(|l| format!(" · “{l}”")).unwrap_or_default())).font(theme::caption()).color(t.text_2));
                let mut action = sh.action.clone();
                let ar = ui.add(egui::TextEdit::singleline(&mut action).hint_text("What happens in the shot").desired_width(f32::INFINITY));
                if ar.changed() {
                    edit_shot(app, &sh.id, |x| x.action = action.clone());
                }
                if ar.lost_focus() {
                    app.doc.end_gesture();
                }
            });
            let click = ui.interact(r.response.rect, egui::Id::unique(("shot-card", &sh.id)), egui::Sense::click());
            if click.clicked() {
                app.editor.playhead = start as crate::view::Time;
                app.editor.selection = sh.clips.iter().map(|c| c.0 as u32).collect();
            }
            ui.add_space(3.0);
        }
    }
}

// ---------------------------------------------------------------- the script

/// Brings a script file in as the project's story (one undo step) and
/// shows the Script tab. Text, Markdown, Fountain, SRT, VTT and Word.
pub fn import_file(app: &mut StudioApp, ctx: &egui::Context, path: &Path) {
    run(app, ctx, json!({"do": "import", "path": path.display().to_string(), "replace": true}), "Import script");
    if !app.doc.model.story.scenes.is_empty() {
        app.editor.rail = super::library::RailItem::Story;
        app.editor.library_chip[super::library::RailItem::Story.index()] = 1;
    }
}

/// File ▸ Import script and the panel's button.
pub fn import_dialog(app: &mut StudioApp, ctx: &egui::Context) {
    if let Some(path) = rfd::FileDialog::new().set_title("Import a script").add_filter("Script", import::Format::EXTENSIONS).pick_file() {
        import_file(app, ctx, &path);
    }
}

/// A script file dropped on the panel is imported; anything else says so.
fn take_dropped_script(app: &mut StudioApp, ctx: &egui::Context, body: Rect) {
    let (files, pos) = ctx.input(|i| (i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect::<Vec<_>>(), i.pointer.hover_pos()));
    // A drag from outside the window carries no pointer position.
    if files.is_empty() || pos.is_some_and(|p| !body.contains(p)) {
        return;
    }
    match files.iter().find(|p| crate::script_import::is_script_file(p)) {
        Some(p) => import_file(app, ctx, p),
        None => {
            kit::push(ctx, "That is not a script file: drop text, Markdown, Fountain, SRT, VTT or Word", None, Some(Icon::Warning));
        }
    }
}

/// An outline and a word while a file is dragged over the panel.
fn drop_hint(ui: &egui::Ui, t: &theme::Tokens, body: Rect) {
    if ui.ctx().input(|i| i.raw.hovered_files.is_empty()) {
        return;
    }
    let p = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::unique("story-drop")));
    p.rect_filled(body, CornerRadius::same(theme::radius::CONTROL), t.accent_soft.gamma_multiply(0.6));
    p.rect_stroke(body.shrink(2.0), CornerRadius::same(theme::radius::CONTROL), Stroke::new(2.0, t.accent), egui::StrokeKind::Inside);
    p.text(body.center(), egui::Align2::CENTER_CENTER, "Drop a script to import it", theme::body_medium(), t.accent);
}

/// A time as typed: seconds or a timecode.
fn parse_point(s: &str) -> Option<m::Micros> {
    match s.trim().parse::<f64>() {
        Ok(x) if x >= 0.0 && x.is_finite() => Some((x * m::SECOND as f64).round() as m::Micros),
        Ok(_) => None,
        Err(_) => times::parse_time(s),
    }
}

/// A length as typed: seconds, `4s`, `1m30s` or a timecode.
fn parse_len(s: &str) -> Option<m::Micros> {
    parse_point(s).filter(|d| *d > 0).or_else(|| times::parse_duration(s))
}

/// A small text box that keeps what is typed until it loses focus, then
/// returns it if it changed. `shown` is what it shows when not being edited.
fn committed(ui: &mut egui::Ui, id: egui::Id, shown: &str, width: f32, hint: &str, colour: Color32, mono: bool) -> Option<String> {
    let focused = ui.memory(|m| m.has_focus(id));
    let mut buf = if focused { ui.data(|d| d.get_temp::<String>(id)).unwrap_or_else(|| shown.to_owned()) } else { shown.to_owned() };
    let mut edit = egui::TextEdit::singleline(&mut buf).id(id).hint_text(hint).desired_width(width).text_color(colour);
    if mono {
        edit = edit.font(theme::mono_small());
    }
    let r = ui.add(edit);
    if r.has_focus() {
        ui.data_mut(|d| d.insert_temp(id, buf.clone()));
    }
    if r.lost_focus() {
        ui.data_mut(|d| d.remove_temp::<String>(id));
        if buf.trim() != shown.trim() {
            return Some(buf.trim().to_owned());
        }
    }
    None
}

/// A time box: blank clears, anything else must read as a time.
fn time_box(ui: &mut egui::Ui, ctx: &egui::Context, id: egui::Id, shown: &str, said: bool, hint: &str, t: &theme::Tokens, parse: fn(&str) -> Option<m::Micros>) -> Option<Option<m::Micros>> {
    let text = committed(ui, id, shown, 54.0, hint, if said { t.text } else { t.text_2 }, true)?;
    if text.is_empty() {
        return Some(None);
    }
    match parse(&text) {
        Some(v) => Some(Some(v)),
        None => {
            kit::push(ctx, "That is not a time: try 0:12, 1:02:03 or 12", None, Some(Icon::Warning));
            None
        }
    }
}

/// Edits the story as a gesture (typing is one undo step until focus goes).
fn edit_script(app: &mut StudioApp, key: String, label: &str, f: impl FnOnce(&mut st::Story)) {
    let mut s = now(app);
    f(&mut s);
    if s != app.doc.model.story {
        app.doc.apply_gesture(&key, label, m::Edit::SetStory { story: Box::new(s) });
    }
}

/// A change of shape (add, remove, move): its own undo step.
fn reshape(app: &mut StudioApp, label: &str, f: impl FnOnce(&mut st::Story)) {
    app.doc.end_gesture();
    let mut s = now(app);
    f(&mut s);
    set_story(app, s, label);
}

enum Act {
    /// A new line after line `.1` of scene `.0` (focus goes to it).
    LineAfter(usize, usize),
    AddLine(usize),
    RemoveLine(usize, usize),
    MoveLine(usize, usize, usize),
    AddScene(Option<usize>),
    RemoveScene(usize),
    Jump(m::Micros),
}

const KINDS: [(st::LineKind, &str); 3] = [(st::LineKind::Action, "Action"), (st::LineKind::Narration, "Narration"), (st::LineKind::Dialogue, "Dialogue")];

/// A new line for scene `si` that follows `prev`: the same kind, and in a
/// conversation the other speaker.
fn next_line(s: &st::Story, si: usize, prev: Option<&st::Line>) -> st::Line {
    let kind = prev.map_or(st::LineKind::Narration, |p| p.kind);
    let who = match kind {
        st::LineKind::Action => None,
        st::LineKind::Narration => prev.and_then(|p| p.who.clone()).or(Some("narrator".into())),
        st::LineKind::Dialogue => s.scenes[si].cast.iter().find(|c| Some(c.as_str()) != prev.and_then(|p| p.who.as_deref())).cloned(),
    };
    st::Line { kind, who, ..Default::default() }
}

/// The script: written and edited here line by line, or as one text.
fn script(app: &mut StudioApp, ui: &mut egui::Ui, story: &st::Story, t: &theme::Tokens, ctx: &egui::Context) {
    let mode_id = egui::Id::unique("story-script-text-mode");
    let mut as_text = ui.data(|d| d.get_temp::<bool>(mode_id)).unwrap_or(false);
    ui.horizontal_wrapped(|ui| {
        ui.selectable_value(&mut as_text, false, "Lines");
        ui.selectable_value(&mut as_text, true, "Text");
        ui.label(RichText::new("Put a time in front of the words, like [0:12-0:18]. Lines without one follow their shot, or are paced by reading speed.").font(theme::caption()).color(t.text_2));
    });
    ui.data_mut(|d| d.insert_temp(mode_id, as_text));
    ui.add_space(4.0);
    if as_text {
        script_text(app, ui, story, t, ctx);
    } else {
        script_lines(app, ui, story, t, ctx);
    }
}

/// The whole script as one text to type or paste into; applied when the
/// box loses focus or Apply is pressed. Shots, places and the cast stay.
fn script_text(app: &mut StudioApp, ui: &mut egui::Ui, story: &st::Story, t: &theme::Tokens, ctx: &egui::Context) {
    #[derive(Clone)]
    struct Buf {
        text: String,
        base: String,
    }
    let id = egui::Id::unique("story-script-text");
    let current = fountain::write_script(story);
    let mut b = ui.data(|d| d.get_temp::<Buf>(id)).unwrap_or_else(|| Buf { text: current.clone(), base: current.clone() });
    if b.base != current {
        // The story changed elsewhere: follow it unless there is typing in the box.
        if b.text == b.base {
            b.text = current.clone();
        }
        b.base = current.clone();
    }
    let dirty = b.text != b.base;
    let r = ui.add(egui::TextEdit::multiline(&mut b.text).id_salt("story-script-box").font(theme::mono_small()).desired_rows(if story.scenes.is_empty() { 10 } else { 22 }).desired_width(f32::INFINITY).hint_text("Type or paste the story or narration here.\n\nScenes start with a heading line (EXT. MEADOW - DAWN) or # Heading.\nNAME: words is dialogue; other lines are narration.\n[0:12-0:18] in front of a line times it."));
    let mut apply = r.lost_focus() && b.text != b.base;
    ui.horizontal(|ui| {
        if dirty && Button::primary("Apply").compact().show(ui).clicked() {
            apply = true;
        }
        if dirty && Button::ghost("Revert").compact().show(ui).clicked() {
            b.text = b.base.clone();
        }
        let bytes = b.text.split_whitespace().count();
        ui.label(RichText::new(if dirty { format!("{bytes} words, not applied yet") } else { format!("{bytes} words") }).font(theme::caption()).color(if dirty { t.warn } else { t.text_2 }));
    });
    if apply {
        if b.text.trim().is_empty() {
            kit::push(ctx, "Nothing to apply: the text is empty", None, Some(Icon::Info));
            b.text = b.base.clone();
        } else {
            let new = import::read_script(&b.text, None);
            if new.scenes.is_empty() {
                kit::push(ctx, "No scenes found in that text", None, Some(Icon::Warning));
            } else {
                let old = now(app);
                let was_empty = old.scenes.is_empty();
                let mut merged = import::merge_script(&old, new);
                if merged.title.is_empty() {
                    merged.title = app.doc.project.name.clone();
                }
                merged.retime_shots();
                let text = fountain::write_script(&merged);
                app.doc.end_gesture();
                set_story(app, merged, if was_empty { "Write script" } else { "Edit script" });
                b = Buf { text: text.clone(), base: text };
                if was_empty {
                    app.editor.library_chip[super::library::RailItem::Story.index()] = 1;
                }
            }
        }
    }
    ui.data_mut(|d| d.insert_temp(id, b));
}

/// The script line by line: scenes with their heading, start and notes;
/// each line with its kind, speaker, words, start / end / length and note.
fn script_lines(app: &mut StudioApp, ui: &mut egui::Ui, story: &st::Story, t: &theme::Tokens, ctx: &egui::Context) {
    let all = story.line_times();
    let playhead = app.editor.playhead as m::Micros;
    let mut act: Option<Act> = None;
    let focus_id = egui::Id::unique("story-focus-next");
    let want_focus = ui.data(|d| d.get_temp::<(usize, usize)>(focus_id));
    for (si, sc) in story.scenes.iter().enumerate() {
        ui.add_space(8.0);
        let hkey = format!("script-h-{si}");
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(format!("Scene {}", sc.id)).font(theme::body_medium()).color(t.text));
            let shown = fountain::slug_line(story, sc).trim_start_matches('.').to_owned();
            let mut h = shown.clone();
            let r = ui.add(egui::TextEdit::singleline(&mut h).id_salt(("scene-heading", si)).hint_text("EXT. PLACE - TIME").desired_width(210.0));
            if r.changed() {
                let text = h.clone();
                edit_script(app, hkey.clone(), "Scene heading", |s| s.scenes[si].heading = text);
            }
            if r.lost_focus() {
                if h != shown {
                    let text = h.clone();
                    edit_script(app, hkey.clone(), "Scene heading", |s| fountain::set_heading(s, si, &text));
                }
                app.doc.end_gesture();
            }
            let start = sc.start.map(times::format_time).unwrap_or_default();
            if let Some(v) = time_box(ui, ctx, egui::Id::unique(("scene-start", si)), &start, sc.start.is_some(), "start", t, parse_point) {
                reshape(app, "Scene start", |s| s.scenes[si].start = v);
            }
            if IconButton::new(Icon::Plus, "Add a line").compact().show(ui).clicked() {
                act = Some(Act::AddLine(si));
            }
            if IconButton::new(Icon::Trash, "Delete the scene").compact().show(ui).clicked() {
                act = Some(Act::RemoveScene(si));
            }
        });
        let mut notes = sc.notes.clone();
        let r = ui.add(egui::TextEdit::singleline(&mut notes).id_salt(("scene-notes", si)).hint_text("Scene notes for the crew").desired_width(f32::INFINITY));
        if r.changed() {
            edit_script(app, format!("script-n-{si}"), "Scene notes", |s| s.scenes[si].notes = notes.clone());
        }
        if r.lost_focus() {
            app.doc.end_gesture();
        }
        if sc.lines.is_empty() {
            ui.label(RichText::new("No lines yet: press + to add one").font(theme::caption()).color(t.text_2));
        }
        for (li, l) in sc.lines.iter().enumerate() {
            let lt = all.iter().find(|x| x.scene == si && x.line == li).copied();
            let here = lt.is_some_and(|x| playhead >= x.start && playhead < x.end);
            let shot = sc.shots.iter().find(|x| x.lines.contains(&li)).map(|x| x.id.clone());
            let key = format!("script-{si}-{li}");
            let frame = egui::Frame::new()
                .fill(if here { t.accent_soft } else { t.raised })
                .stroke(Stroke::new(1.0, t.line))
                .corner_radius(CornerRadius::same(theme::radius::CONTROL))
                .inner_margin(egui::Margin::same(6));
            frame.show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal_wrapped(|ui| {
                    let n = ui.add(egui::Label::new(RichText::new(format!("{:>2}", li + 1)).font(theme::mono_small()).color(t.text_2)).sense(egui::Sense::click()));
                    if n.on_hover_text("Go to this line").clicked() {
                        if let Some(x) = lt {
                            act = Some(Act::Jump(x.start));
                        }
                    }
                    let mut kind = l.kind;
                    let name = KINDS.iter().find(|k| k.0 == kind).map_or("", |k| k.1);
                    egui::ComboBox::from_id_salt(("line-kind", si, li)).selected_text(name).width(88.0).show_ui(ui, |ui| {
                        kit::note_list(ui);
                        for (k, name) in KINDS {
                            ui.selectable_value(&mut kind, k, name);
                        }
                    });
                    if kind != l.kind {
                        edit_script(app, key.clone(), "Line kind", |s| {
                            let line = &mut s.scenes[si].lines[li];
                            line.kind = kind;
                            match kind {
                                st::LineKind::Action => line.who = None,
                                st::LineKind::Narration => line.who = Some(line.who.clone().filter(|w| w != "narrator").unwrap_or_else(|| "narrator".into())),
                                st::LineKind::Dialogue => {
                                    if line.who.as_deref() == Some("narrator") {
                                        line.who = None;
                                    }
                                }
                            }
                        });
                        app.doc.end_gesture();
                    }
                    if kind != st::LineKind::Action {
                        let who = l.who.clone().unwrap_or_default();
                        let shown = story.character(&who).map(|c| if c.name.is_empty() { c.id.clone() } else { c.name.clone() }).unwrap_or_else(|| who.replace('_', " "));
                        let hint = if kind == st::LineKind::Narration { "narrator" } else { "speaker" };
                        if let Some(text) = committed(ui, egui::Id::unique(("line-who", si, li)), &shown, 96.0, hint, t.text, false) {
                            reshape(app, "Speaker", |s| {
                                let id = if text.is_empty() {
                                    (kind == st::LineKind::Narration).then(|| "narrator".to_owned())
                                } else {
                                    Some(s.character(&text).map(|c| c.id.clone()).unwrap_or_else(|| fountain::slug(&text)))
                                };
                                if let Some(w) = &id {
                                    s.know_speaker(si, w, kind);
                                }
                                s.scenes[si].lines[li].who = id;
                            });
                        }
                    }
                    match &shot {
                        Some(s) => ui.label(RichText::new(format!("shot {s}")).font(theme::caption()).color(t.accent)),
                        None => ui.label(RichText::new("no shot").font(theme::caption()).color(t.text_2)),
                    };
                    if IconButton::new(Icon::CaretUp, "Move up").compact().show(ui).clicked() && li > 0 {
                        act = Some(Act::MoveLine(si, li, li - 1));
                    }
                    if IconButton::new(Icon::CaretDown, "Move down").compact().show(ui).clicked() && li + 1 < sc.lines.len() {
                        act = Some(Act::MoveLine(si, li, li + 1));
                    }
                    if IconButton::new(Icon::Plus, "Add a line below").compact().show(ui).clicked() {
                        act = Some(Act::LineAfter(si, li));
                    }
                    if IconButton::new(Icon::Trash, "Delete the line").compact().show(ui).clicked() {
                        act = Some(Act::RemoveLine(si, li));
                    }
                });
                // The words. Enter makes the next line; a pasted block becomes lines.
                let tid = egui::Id::unique(("line-text", si, li));
                let enter = ui.memory(|m| m.has_focus(tid)) && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
                let mut text = l.text.clone();
                let r = ui.add(egui::TextEdit::multiline(&mut text).id(tid).desired_rows(1).desired_width(f32::INFINITY).hint_text(if l.kind == st::LineKind::Action { "What we see" } else { "What is said" }));
                if want_focus == Some((si, li)) {
                    r.request_focus();
                    ui.data_mut(|d| d.remove_temp::<(usize, usize)>(focus_id));
                }
                if r.changed() {
                    if text.contains('\n') {
                        let parts: Vec<String> = text.split('\n').map(str::to_owned).collect();
                        reshape(app, "Paste script", |s| {
                            s.scenes[si].lines[li].text = parts[0].trim().to_owned();
                            let kind = s.scenes[si].lines[li].kind;
                            for (k, p) in parts.iter().skip(1).filter(|p| !p.trim().is_empty()).enumerate() {
                                let nl = import::line_from_text(p, kind);
                                if let Some(w) = nl.who.clone() {
                                    s.know_speaker(si, &w, nl.kind);
                                }
                                s.insert_line(si, li + 1 + k, nl);
                            }
                        });
                    } else {
                        edit_script(app, key.clone(), "Script line", |s| s.scenes[si].lines[li].text = text.clone());
                    }
                }
                if r.lost_focus() {
                    app.doc.end_gesture();
                }
                if enter {
                    act = Some(Act::LineAfter(si, li));
                }
                // Times and notes.
                ui.horizontal_wrapped(|ui| {
                    let (a, b, d) = lt.map(|x| (times::format_time(x.start), times::format_time(x.end), times::format_duration(x.end - x.start))).unwrap_or_default();
                    let said_start = l.start.is_some();
                    let said_len = l.duration.is_some();
                    let from = lt.map_or(0, |x| x.start);
                    if let Some(v) = time_box(ui, ctx, egui::Id::unique(("line-start", si, li)), &a, said_start, "start", t, parse_point) {
                        reshape(app, "Line start", |s| s.scenes[si].lines[li].start = v);
                    }
                    ui.label(RichText::new("–").font(theme::caption()).color(t.text_2));
                    if let Some(v) = time_box(ui, ctx, egui::Id::unique(("line-end", si, li)), &b, said_start && said_len, "end", t, parse_point) {
                        match v {
                            Some(e) if e > from => reshape(app, "Line end", |s| {
                                let line = &mut s.scenes[si].lines[li];
                                // An end pins the start too: both are the person's times.
                                let a = line.start.unwrap_or(from);
                                line.start = Some(a);
                                line.duration = Some(e - a);
                            }),
                            Some(_) => {
                                kit::push(ctx, "The end must be after the start", None, Some(Icon::Warning));
                            }
                            None => reshape(app, "Line end", |s| s.scenes[si].lines[li].duration = None),
                        }
                    }
                    if let Some(v) = time_box(ui, ctx, egui::Id::unique(("line-len", si, li)), &d, said_len, "len", t, parse_len) {
                        reshape(app, "Line length", |s| s.scenes[si].lines[li].duration = v);
                    }
                    if (said_start || said_len) && IconButton::new(Icon::X, "Follow the shot again").compact().show(ui).clicked() {
                        reshape(app, "Line time", |s| {
                            let line = &mut s.scenes[si].lines[li];
                            line.start = None;
                            line.duration = None;
                        });
                    }
                    if l.kind == st::LineKind::Dialogue {
                        if let Some(text) = committed(ui, egui::Id::unique(("line-how", si, li)), l.how.as_deref().unwrap_or(""), 90.0, "how", t.text, false) {
                            reshape(app, "Line delivery", |s| s.scenes[si].lines[li].how = (!text.is_empty()).then_some(text));
                        }
                    }
                    if let Some(text) = committed(ui, egui::Id::unique(("line-note", si, li)), &l.note, 140.0, "note", t.text, false) {
                        reshape(app, "Line note", |s| s.scenes[si].lines[li].note = text);
                    }
                });
            });
            ui.add_space(3.0);
        }
    }
    ui.add_space(8.0);
    if Button::secondary("Add scene").icon(Icon::Plus).compact().show(ui).clicked() {
        act = Some(Act::AddScene(story.scenes.len().checked_sub(1)));
    }
    match act {
        None => {}
        Some(Act::Jump(at)) => app.editor.playhead = at as crate::view::Time,
        Some(Act::LineAfter(si, li)) => {
            reshape(app, "Add line", |s| {
                let nl = next_line(s, si, s.scenes[si].lines.get(li));
                s.insert_line(si, li + 1, nl);
            });
            ui.data_mut(|d| d.insert_temp(focus_id, (si, li + 1)));
        }
        Some(Act::AddLine(si)) => {
            let n = story.scenes[si].lines.len();
            reshape(app, "Add line", |s| {
                let nl = next_line(s, si, s.scenes[si].lines.last());
                s.insert_line(si, n, nl);
            });
            ui.data_mut(|d| d.insert_temp(focus_id, (si, n)));
        }
        Some(Act::RemoveLine(si, li)) => reshape(app, "Delete line", |s| {
            s.remove_line(si, li);
        }),
        Some(Act::MoveLine(si, from, to)) => reshape(app, "Move line", |s| {
            s.move_line(si, from, to);
        }),
        Some(Act::AddScene(after)) => {
            let mut new_index = 0;
            reshape(app, "Add scene", |s| {
                new_index = s.add_scene(after);
                s.insert_line(new_index, 0, st::Line { kind: st::LineKind::Narration, who: Some("narrator".into()), ..Default::default() });
            });
            ui.data_mut(|d| d.insert_temp(focus_id, (new_index, 0usize)));
        }
        Some(Act::RemoveScene(si)) => reshape(app, "Delete scene", |s| {
            s.scenes.remove(si);
        }),
    }
}

/// A text field for one field of the bible; returns the new text when it
/// changed.
fn field(ui: &mut egui::Ui, label: &str, value: &str, id: String) -> (Option<String>, bool) {
    let mut v = value.to_owned();
    let mut out = None;
    let mut done = false;
    ui.horizontal(|ui| {
        ui.add_sized([72.0, 18.0], egui::Label::new(RichText::new(label).font(theme::caption())));
        let r = ui.add(egui::TextEdit::singleline(&mut v).id_salt(id).desired_width(f32::INFINITY));
        if r.changed() {
            out = Some(v.clone());
        }
        done = r.lost_focus();
    });
    (out, done)
}

fn places(app: &mut StudioApp, ui: &mut egui::Ui, story: &st::Story, t: &theme::Tokens) {
    if story.places.is_empty() {
        ui.label(RichText::new("No places yet: scene headings (EXT. MEADOW - DAWN) make them").font(theme::caption()).color(t.text_2));
    }
    for p in &story.places {
        ui.add_space(6.0);
        ui.label(RichText::new(format!("{}{}", if p.name.is_empty() { &p.id } else { &p.name }, if p.interior { " (inside)" } else { "" })).font(theme::body_medium()).color(t.text));
        let mut change: Option<(String, String)> = None;
        let mut done = false;
        for (k, v) in [("backdrop", p.backdrop.clone().unwrap_or_default()), ("ambience", p.ambience.clone().unwrap_or_default()), ("light", p.light.clone()), ("surface", p.surface.clone().unwrap_or_default()), ("notes", p.notes.clone())] {
            let (c, d) = field(ui, k, &v, format!("place-{}-{k}", p.id));
            if let Some(c) = c {
                change = Some((k.to_owned(), c));
            }
            done |= d;
        }
        if let Some((k, v)) = change {
            let mut s = now(app);
            if let Some(x) = s.places.iter_mut().find(|x| x.id == p.id) {
                let opt = |v: &str| (!v.trim().is_empty()).then(|| v.to_owned());
                match k.as_str() {
                    "backdrop" => x.backdrop = opt(&v),
                    "ambience" => x.ambience = opt(&v),
                    "surface" => x.surface = opt(&v),
                    "light" => x.light = v,
                    _ => x.notes = v,
                }
            }
            app.doc.apply_gesture(&format!("story-place-{}", p.id), "Place", m::Edit::SetStory { story: Box::new(s) });
        }
        if done {
            app.doc.end_gesture();
        }
    }
}

fn cast(app: &mut StudioApp, ui: &mut egui::Ui, story: &st::Story, t: &theme::Tokens) {
    if story.cast.is_empty() {
        ui.label(RichText::new("No characters yet: the speakers of the script become the cast").font(theme::caption()).color(t.text_2));
    }
    for c in &story.cast {
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(if c.name.is_empty() { &c.id } else { &c.name }).font(theme::body_medium()).color(t.text));
            let mut size32 = c.size as f32;
            let r = kit::Slider::new("Size", &mut size32, 0.05..=20.0).default(1.0).unit("×").decimals(2).logarithmic(true).with_label().with_value().width(200.0).show(ui);
            if r.changed() {
                let mut s = now(app);
                if let Some(x) = s.cast.iter_mut().find(|x| x.id == c.id) {
                    x.size = size32 as _;
                }
                app.doc.apply_gesture(&format!("story-cast-{}", c.id), "Character", m::Edit::SetStory { story: Box::new(s) });
            }
            if r.drag_stopped() || r.lost_focus() {
                app.doc.end_gesture();
            }
        });
        let mut change: Option<(String, String)> = None;
        let mut done = false;
        for (k, v) in [("look", c.look.clone()), ("references", c.references.join(", ")), ("cast item", c.cast_item.clone().unwrap_or_default()), ("voice", c.voice.clone().unwrap_or_default()), ("personality", c.personality.clone()), ("moves", c.moves.join(", "))] {
            let (ch, d) = field(ui, k, &v, format!("cast-{}-{k}", c.id));
            if let Some(ch) = ch {
                change = Some((k.to_owned(), ch));
            }
            done |= d;
        }
        if let Some((k, v)) = change {
            let mut s = now(app);
            if let Some(x) = s.cast.iter_mut().find(|x| x.id == c.id) {
                let list = |v: &str| v.split(',').map(|w| w.trim().to_owned()).filter(|w| !w.is_empty()).collect::<Vec<_>>();
                let opt = |v: &str| (!v.trim().is_empty()).then(|| v.to_owned());
                match k.as_str() {
                    "look" => x.look = v,
                    "references" => x.references = list(&v),
                    "cast item" => x.cast_item = opt(&v),
                    "voice" => x.voice = opt(&v),
                    "personality" => x.personality = v,
                    _ => x.moves = list(&v),
                }
            }
            app.doc.apply_gesture(&format!("story-cast-{}", c.id), "Character", m::Edit::SetStory { story: Box::new(s) });
        }
        if done {
            app.doc.end_gesture();
        }
    }
}

fn check(app: &mut StudioApp, ui: &mut egui::Ui, story: &st::Story, t: &theme::Tokens) {
    let issues = story.check();
    if issues.is_empty() {
        ui.label(RichText::new("The plan holds: every line is shown, the coverage varies and the line is kept.").font(theme::body()).color(t.accent));
        return;
    }
    let times = story.timings();
    for i in &issues {
        let colour = match i.severity {
            st::Severity::Error => t.danger,
            st::Severity::Warning => t.warn,
            st::Severity::Note => t.text_2,
        };
        let r = ui.vertical(|ui| {
            ui.label(RichText::new(&i.problem).font(theme::body()).color(colour));
            ui.label(RichText::new(&i.fix).font(theme::caption()).color(t.text_2));
        });
        if let Some(sid) = &i.shot {
            if ui.interact(r.response.rect, egui::Id::unique(("issue", sid, i.kind)), egui::Sense::click()).clicked() {
                if let Some((si, k)) = story.find_shot(sid) {
                    if let Some(x) = times.iter().find(|x| x.scene == si && x.shot == k) {
                        app.editor.playhead = x.start as crate::view::Time;
                    }
                }
            }
        }
        ui.add_space(4.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The panel drawn headless: every tab with a planned story, a shot
    /// edited from its card as one undo step, and Build from the panel.
    #[test]
    fn the_story_panel_shows_edits_and_builds() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = StudioApp::new(&cc, crate::cli::Cli { screen: Some(crate::cli::Screen::Editor), fresh: true, ..Default::default() });
        let mut s = crate::agent::Session::new(crate::services::project::new_custom("Panel", 640, 360, 30.0));
        let script = "EXT. MEADOW - DAY\n\nNARRATOR (V.O.)\nThe fox walked out into the sun.\n\nFOX\nHello?\n\nOWL\nUp here!\n";
        s.call("story", &json!({"do": "import", "text": script, "plan": true})).unwrap();
        app.doc = s.doc;
        let area = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(420.0, 900.0));
        let mut t = 0.0;
        let mut draw = |app: &mut StudioApp, chip: usize| {
            t += 1.0 / 60.0;
            let input = egui::RawInput { screen_rect: Some(area), time: Some(t), ..Default::default() };
            let mut out = ctx.run_ui(input, |ui| panel(app, ui, chip, area));
            let _ = ctx.tessellate(std::mem::take(&mut out.shapes), out.pixels_per_point);
            out.textures_delta.clear();
        };
        for chip in 0..GROUPS.len() {
            draw(&mut app, chip);
            draw(&mut app, chip);
        }
        let undo = app.doc.history.undo_len();
        let first = app.doc.model.story.scenes[0].shots[0].id.clone();
        edit_shot(&mut app, &first, |x| x.action = "the m".into());
        edit_shot(&mut app, &first, |x| x.action = "the meadow in sun".into());
        app.doc.end_gesture();
        assert_eq!(app.doc.model.story.scenes[0].shots[0].action, "the meadow in sun");
        assert_eq!(app.doc.history.undo_len(), undo + 1, "typing is one undo step");
        let ectx = ctx.clone();
        run(&mut app, &ectx, json!({"do": "build"}), "Build from story");
        assert_eq!(app.doc.history.undo_len(), undo + 2, "the build is one undo step");
        assert!(now(&app).scenes[0].shots.iter().all(|sh| !sh.clips.is_empty()), "every shot is on the timeline");
        draw(&mut app, 0);
        app.doc.undo();
        assert!(now(&app).scenes[0].shots.iter().all(|sh| sh.clips.is_empty()));
    }

    #[derive(Debug)]
    struct Dropped(std::path::PathBuf);
    impl egui::DroppedFile for Dropped {
        fn path(&self) -> &Path {
            &self.0
        }
        fn bytes(&self) -> Result<Vec<u8>, String> {
            std::fs::read(&self.0).map_err(|e| e.to_string())
        }
    }

    /// The script panel from nothing: the empty state takes a dropped file
    /// (SRT keeps its times), both views draw, and edits keep shots and
    /// times in step and undo.
    #[test]
    fn a_script_is_dropped_on_the_panel_then_edited() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = StudioApp::new(&cc, crate::cli::Cli { screen: Some(crate::cli::Screen::Editor), fresh: true, ..Default::default() });
        app.doc = crate::agent::Session::new(crate::services::project::new_custom("Drop", 640, 360, 30.0)).doc;
        let area = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(420.0, 900.0));
        let mut t = 0.0;
        let mut draw = |app: &mut StudioApp, files: Vec<std::path::PathBuf>| {
            t += 1.0 / 60.0;
            let dropped_files = files.into_iter().map(|p| std::sync::Arc::new(Dropped(p)) as std::sync::Arc<dyn egui::DroppedFile + Send + Sync>).collect();
            let input = egui::RawInput { screen_rect: Some(area), time: Some(t), dropped_files, ..Default::default() };
            let mut out = ctx.run_ui(input, |ui| panel(app, ui, 1, area));
            let _ = ctx.tessellate(std::mem::take(&mut out.shapes), out.pixels_per_point);
            out.textures_delta.clear();
        };
        draw(&mut app, Vec::new());
        assert!(app.doc.model.story.is_empty());
        let dir = std::env::temp_dir().join(format!("studio-script-drop-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // Something that is not a script is refused.
        let video = dir.join("clip.mp4");
        std::fs::write(&video, b"x").unwrap();
        draw(&mut app, vec![video]);
        assert!(app.doc.model.story.is_empty());
        let srt = dir.join("lines.srt");
        std::fs::write(&srt, "1\n00:00:01,000 --> 00:00:03,000\nThe fox woke early.\n\n2\n00:00:04,000 --> 00:00:06,500\nFERN: Is anyone there?\n").unwrap();
        draw(&mut app, vec![srt]);
        let lines = &app.doc.model.story.scenes[0].lines;
        assert_eq!((lines.len(), lines[0].start, lines[1].duration), (2, Some(1_000_000), Some(2_500_000)));
        assert_eq!(app.editor.library_chip[crate::editor::library::RailItem::Story.index()], 1);
        let _ = std::fs::remove_dir_all(&dir);
        // Both views draw.
        draw(&mut app, Vec::new());
        ctx.data_mut(|d| d.insert_temp(egui::Id::unique("story-script-text-mode"), true));
        draw(&mut app, Vec::new());
        draw(&mut app, Vec::new());
        ctx.data_mut(|d| d.insert_temp(egui::Id::unique("story-script-text-mode"), false));
        // Edits: a line's words (one undo step), a time set by hand, a line added.
        let undo = app.doc.history.undo_len();
        edit_script(&mut app, "k".into(), "Script line", |s| s.scenes[0].lines[0].text = "The fox".into());
        edit_script(&mut app, "k".into(), "Script line", |s| s.scenes[0].lines[0].text = "The fox woke.".into());
        app.doc.end_gesture();
        assert_eq!(app.doc.history.undo_len(), undo + 1);
        reshape(&mut app, "Line start", |s| s.scenes[0].lines[1].start = Some(9_000_000));
        assert_eq!(now(&app).line_times()[1].start, 9_000_000);
        reshape(&mut app, "Add line", |s| {
            let nl = next_line(s, 0, s.scenes[0].lines.get(1));
            s.insert_line(0, 2, nl);
        });
        assert_eq!(app.doc.model.story.scenes[0].lines.len(), 3);
        assert_eq!(app.doc.model.story.scenes[0].lines[2].kind, st::LineKind::Dialogue);
        draw(&mut app, Vec::new());
        app.doc.undo();
        assert_eq!(app.doc.model.story.scenes[0].lines.len(), 2);
        assert!(parse_point("0:12") == Some(12_000_000) && parse_point("12") == Some(12_000_000) && parse_point("-3").is_none() && parse_point("soon").is_none());
        assert_eq!(parse_len("4s"), Some(4_000_000));
    }
}
