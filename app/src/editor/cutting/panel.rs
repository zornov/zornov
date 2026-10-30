// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The Editorial window (⌥E or the list button in the timeline bar): the
//! things an editor looks up while cutting a long film.
//!
//! - **Index**: every clip, marker, transition and effect in time order;
//!   a row jumps there and selects the clip.
//! - **Checks**: the project check (the Export screen's, with Fix and
//!   Accept as intended), gaps and the same footage used twice.
//! - **Takes**: scene, take, camera, circled and rating per file, read
//!   from names; string-outs of a scene or of the circled takes.
//! - **Sequences**: every cut, switch, duplicate, new, delete.
//! - **Script**: a line of the script finds every take that says it (one
//!   opens in the source monitor marked); the clip under the playhead as
//!   its transcript, where clicking words marks them and Cut takes them out
//!   of picture and sound.

use super::{open_source, toast};
use crate::agent::editorial::{Line, TakeHit, clip_lines, take_hits};
use crate::app::StudioApp;
use crate::kit::{self, ShowNoted};
use crate::theme;
use crate::view::{SEC, Time};
use eframe::egui::{self, RichText};
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use studio_model as m;
use studio_model::editorial as ed;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Index,
    Checks,
    Takes,
    Sequences,
    Script,
}

/// Work done off the interface thread (speech is heard the first time).
type Pending<T> = Arc<Mutex<Option<Result<T, String>>>>;

#[derive(Default)]
pub struct Panel {
    pub open: bool,
    pub tab: Tab,
    filter: String,
    scene: String,
    line: String,
    hits: Option<Result<Vec<TakeHit>, String>>,
    finding: Option<Pending<Vec<TakeHit>>>,
    /// The clip whose transcript is shown, its lines, and the words
    /// marked to cut (line, word).
    words_of: Option<u32>,
    lines: Option<Result<Vec<Line>, String>>,
    hearing: Option<Pending<Vec<Line>>>,
    marked: BTreeSet<(usize, usize)>,
}

fn secs(t: Time) -> String {
    format!("{:.2}", t as f64 / SEC as f64)
}

fn take<T>(p: &mut Option<Pending<T>>) -> Option<Result<T, String>> {
    let got = p.as_ref()?.lock().unwrap_or_else(|e| e.into_inner()).take();
    if got.is_some() {
        *p = None;
    }
    got
}

/// Draws the window when it is open.
pub fn show(app: &mut StudioApp, ctx: &egui::Context) {
    if crate::keymap::hit(ctx, &app.prefs.clone(), "editorial-panel") && !ctx.text_edit_focused() {
        app.editor.cutting.panel.open = !app.editor.cutting.panel.open;
    }
    if !app.editor.cutting.panel.open {
        return;
    }
    // Results that came in.
    let p = &mut app.editor.cutting.panel;
    if let Some(r) = take(&mut p.finding) {
        p.hits = Some(r);
    }
    if let Some(r) = take(&mut p.hearing) {
        p.lines = Some(r);
    }
    if p.finding.is_some() || p.hearing.is_some() {
        ctx.request_repaint_after(std::time::Duration::from_millis(200));
    }
    // Docked (or in a window of its own), the dock draws it.
    if super::super::dock_ui::docked(ctx, crate::dock::Panel::Editorial) {
        return;
    }
    let mut open = true;
    egui::Window::new("Editorial").id(egui::Id::unique("cut-editorial-panel")).open(&mut open).default_size([460.0, 520.0]).resizable(true).show(ctx, |ui| body(app, ui, true));
    if !open {
        app.editor.cutting.panel.open = false;
    }
}

/// The panel docked in the editor, in `r`.
pub fn docked(app: &mut StudioApp, ui: &mut egui::Ui, r: egui::Rect) {
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r.shrink(10.0)).layout(egui::Layout::top_down(egui::Align::Min)).id_salt("editorial-docked"));
    child.set_clip_rect(r.intersect(ui.clip_rect()));
    body(app, &mut child, false);
}

/// The tabs and their lists (`grip`: the dock handle before the tabs, in
/// the floating window).
fn body(app: &mut StudioApp, ui: &mut egui::Ui, grip: bool) {
    if grip {
        ui.horizontal(|ui| super::super::dock_ui::float_grip(app, ui, crate::dock::Panel::Editorial));
    }
    let tabs = [Tab::Index, Tab::Checks, Tab::Takes, Tab::Sequences, Tab::Script];
    let mut at = tabs.iter().position(|t| *t == app.editor.cutting.panel.tab).unwrap_or(0);
    if crate::kit::tab_bar(ui, "editorial-tabs", &mut at, &["Index", "Checks", "Takes", "Sequences", "Script"]) {
        app.editor.cutting.panel.tab = tabs[at];
    }
    egui::ScrollArea::vertical().id_salt("editorial-lists").auto_shrink([false, false]).show_v(ui, |ui| match app.editor.cutting.panel.tab {
        Tab::Index => index(app, ui),
        Tab::Checks => checks(app, ui),
        Tab::Takes => takes(app, ui),
        Tab::Sequences => sequences(app, ui),
        Tab::Script => script(app, ui),
    });
}

fn jump(app: &mut StudioApp, t: Time, clip: Option<m::ClipId>) {
    app.editor.playhead = t.max(0);
    app.editor.playing = false;
    if let Some(c) = clip {
        app.editor.selection = vec![c.0 as u32];
    }
}

fn index(app: &mut StudioApp, ui: &mut egui::Ui) {
    let p = &mut app.editor.cutting.panel;
    ui.horizontal_wrapped(|ui| {
        ui.label("Find");
        ui.text_edit_singleline(&mut p.filter);
    });
    let want = p.filter.trim().to_lowercase();
    let rows = ed::timeline_index(&app.doc.model, app.doc.model.active());
    let mut go = None;
    // Two lines a row, so the list fits the panel at any width: the time,
    // kind and name, then the detail under them.
    for r in rows.iter().filter(|r| want.is_empty() || r.name.to_lowercase().contains(&want) || r.detail.to_lowercase().contains(&want) || r.kind.contains(&want)) {
        ui.horizontal_wrapped(|ui| {
            if ui.link(secs(r.start)).clicked() {
                go = Some((r.start, r.clip));
            }
            ui.label(RichText::new(r.kind).font(theme::caption()));
            ui.label(&r.name);
        });
        ui.label(RichText::new(&r.detail).font(theme::caption()));
        ui.add_space(2.0);
    }
    if let Some((t, c)) = go {
        jump(app, t, c);
    }
}

/// The project check of the Export screen, here too: the same service, the same Fix and Accept.
fn project_checks(app: &mut StudioApp, ui: &mut egui::Ui) {
    use crate::services::export as svc;
    let id = egui::Id::new("editorial-project-check");
    let mut out: Option<svc::CheckOut> = ui.data(|d| d.get_temp(id));
    ui.horizontal(|ui| {
        ui.label(RichText::new("Project checks").strong());
        if crate::kit::Button::secondary("Check project").compact().show(ui).clicked() {
            let o = svc::check_project(&mut app.doc, false, false);
            toast(ui.ctx(), if o.is_clear() { "No problems found".to_owned() } else { format!("Found {} to look at", o.problems.len()) });
            out = Some(o);
        }
    });
    let Some(cur) = out.clone() else {
        ui.add_space(8.0);
        return;
    };
    if cur.is_clear() {
        ui.label(RichText::new("No problems found").font(theme::caption()));
    }
    let mut act: Option<(usize, bool)> = None;
    let mut go: Option<f64> = None;
    for (i, p) in cur.problems.iter().enumerate() {
        ui.horizontal_wrapped(|ui| {
            if let Some(at) = p.at {
                if ui.link(format!("{at:.2}")).clicked() {
                    go = Some(at);
                }
            }
            ui.label(RichText::new(&p.message).font(theme::caption()).color(if p.severity == svc::Severity::Fail { theme::tokens(ui.ctx()).danger } else { theme::tokens(ui.ctx()).text }));
            if p.fixable && crate::kit::Button::secondary("Fix").compact().show(ui).clicked() {
                act = Some((i, true));
            }
            if crate::kit::Button::ghost("Accept as intended").compact().show(ui).clicked() {
                act = Some((i, false));
            }
        });
    }
    if let Some(at) = go {
        jump(app, (at * SEC as f64) as Time, None);
    }
    if let Some((i, fix)) = act {
        let p = cur.problems[i].clone();
        if fix {
            let n = svc::fix_problem(&mut app.doc, &p.kind, p.clip).len();
            toast(ui.ctx(), if n == 0 { "Nothing could be fixed automatically".to_owned() } else { "Fixed 1 problem".to_owned() });
        } else if app.doc.folder.is_none() {
            toast(ui.ctx(), "Save the project to a folder first: accepted findings are kept with the project".to_owned());
        } else {
            match svc::accept(&mut app.doc, &p.kind, p.clip) {
                Ok(()) => toast(ui.ctx(), "Accepted as intended".to_owned()),
                Err(e) => toast(ui.ctx(), format!("Could not accept it: {e}")),
            }
        }
        out = Some(svc::check_project(&mut app.doc, false, false));
    }
    if let Some(o) = out {
        ui.data_mut(|d| d.insert_temp(id, o));
    }
    ui.add_space(8.0);
}

fn checks(app: &mut StudioApp, ui: &mut egui::Ui) {
    project_checks(app, ui);
    let s = app.doc.model.active().clone();
    let gaps = ed::find_gaps(&s, 0);
    let dups = ed::find_duplicates(&s, app.editor.frame_dur(app.doc.project.fps));
    ui.label(RichText::new(format!("Gaps ({})", gaps.len())).strong());
    let mut go = None;
    let mut close = None;
    for g in &gaps {
        ui.horizontal_wrapped(|ui| {
            let lane = s.lane(g.lane).map(|l| l.name.clone()).unwrap_or_default();
            if ui.link(format!("{lane} {}–{} s", secs(g.start), secs(g.end))).clicked() {
                go = Some((g.start, None));
            }
            if crate::kit::Button::ghost("Close").compact().show(ui).on_hover_text("Extract the gap: everything after moves up").clicked() {
                close = Some((g.lane, g.start, g.end));
            }
        });
    }
    ui.add_space(8.0);
    ui.label(RichText::new(format!("Footage used twice ({})", dups.len())).strong());
    for d in &dups {
        let name = app.doc.model.media_item(d.media).map(|i| i.name.clone()).unwrap_or_default();
        if ui.link(format!("{name} {}–{} s: clips {} and {}", secs(d.source.start), secs(d.source.end()), d.a.0, d.b.0)).clicked() {
            let at = s.clip(d.a).map_or(0, |c| c.start);
            go = Some((at, Some(d.a)));
        }
    }
    if let Some((t, c)) = go {
        jump(app, t, c);
        if let Some(b) = dups.iter().find(|d| Some(d.a) == c) {
            app.editor.selection.push(b.b.0 as u32);
        }
    }
    if let Some((lane, a, b)) = close {
        let seq = app.doc.seq();
        let r = ed::extract(&mut app.doc.model, seq, a, b, Some(&[lane]));
        match r {
            Ok(e) => {
                app.doc.apply("Close gap", e);
            }
            Err(e) => toast(ui.ctx(), e.to_string()),
        }
    }
}

fn takes(app: &mut StudioApp, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    ui.horizontal_wrapped(|ui| {
        if crate::kit::Button::secondary("Read scene and take from names").compact().show(ui).clicked() {
            let mut edits = Vec::new();
            for mi in &app.doc.model.media {
                if let Some((scene, take, camera)) = ed::parse_take(&mi.name).filter(|_| !mi.extra.contains_key("scene")) {
                    let info = ed::TakeInfo { scene: Some(scene), take, camera, ..ed::take_info(mi) };
                    if let Ok(e) = ed::set_take_info(&app.doc.model, mi.id, &info) {
                        edits.push(e);
                    }
                }
            }
            if edits.is_empty() {
                toast(&ctx, "No file names carry scene and take");
            } else {
                app.doc.apply("Scene and take", m::Edit::Batch { edits });
            }
        }
    });
    ui.horizontal_wrapped(|ui| {
        ui.label("Scene");
        ui.add(egui::TextEdit::singleline(&mut app.editor.cutting.panel.scene).desired_width(60.0));
        let scene = app.editor.cutting.panel.scene.trim().to_uppercase();
        for (label, circled) in [("String out", false), ("Circled takes", true)] {
            if ui.button(label).on_hover_text("Every take end to end in a new sequence, in scene and take order").clicked() {
                string_out(app, &ctx, (!scene.is_empty()).then_some(scene.clone()), circled);
            }
        }
    });
    ui.separator();
    let mut rows: Vec<(ed::TakeInfo, m::MediaItem)> = app.doc.model.media.iter().map(|mi| (ed::take_info(mi), mi.clone())).collect();
    rows.sort_by_key(|(i, mi)| (num(&i.scene), num(&i.take), mi.id.0));
    let mut change = None;
    let mut open = None;
    // A table keeps its columns: wider than the window, it scrolls sideways.
    kit::scroll_wide(ui, "cut-takes-scroll", |ui| egui::Grid::new("cut-takes").striped(true).num_columns(5).show(ui, |ui| {
        for (info, mi) in &rows {
            if ui.link(&mi.name).on_hover_text("Open in the source monitor").clicked() {
                open = Some(mi.id);
            }
            ui.label(format!("{} {}", info.scene.as_deref().map_or(String::new(), |s| format!("Sc {s}")), info.take.as_deref().map_or(String::new(), |t| format!("Tk {t}"))));
            let mut c = info.circled;
            if ui.checkbox(&mut c, "circled").changed() {
                change = Some((mi.id, ed::TakeInfo { circled: c, ..info.clone() }));
            }
            let mut r = info.rating;
            ui.horizontal(|ui| {
                for k in 1..=5u8 {
                    let on = r >= k;
                    if ui.small_button(RichText::new(if on { "●" } else { "○" }).color({ let t = theme::tokens(ui.ctx()); if on { t.warn } else { t.text_2 } })).clicked() {
                        r = if r == k { 0 } else { k };
                        change = Some((mi.id, ed::TakeInfo { rating: r, ..info.clone() }));
                    }
                }
            });
            ui.label(RichText::new(info.note.clone().unwrap_or_default()).font(theme::caption()));
            ui.end_row();
        }
    }));
    if let Some((id, info)) = change
        && let Ok(e) = ed::set_take_info(&app.doc.model, id, &info)
    {
        app.doc.apply("Take", e);
    }
    if let Some(id) = open {
        open_source(app, ed::Source::Media(id), 0, false);
    }
}

fn num(s: &Option<String>) -> (u64, String) {
    let v = s.clone().unwrap_or_default();
    (v.chars().take_while(char::is_ascii_digit).collect::<String>().parse().unwrap_or(u64::MAX), v)
}

fn string_out(app: &mut StudioApp, ctx: &egui::Context, scene: Option<String>, circled: bool) {
    let mut rows: Vec<(ed::TakeInfo, m::MediaId, String)> = app
        .doc
        .model
        .media
        .iter()
        .filter(|mi| mi.has_video())
        .map(|mi| (ed::take_info(mi), mi.id, mi.name.clone()))
        .filter(|(i, _, _)| scene.as_ref().is_none_or(|s| i.scene.as_deref().map(str::to_uppercase).as_ref() == Some(s)) && (!circled || i.circled))
        .collect();
    if rows.is_empty() {
        toast(ctx, "No takes match");
        return;
    }
    rows.sort_by_key(|(i, id, _)| (num(&i.scene), num(&i.take), id.0));
    let takes: Vec<ed::Take> = rows
        .iter()
        .map(|(i, id, _)| {
            let label = [i.scene.as_ref().map(|s| format!("Sc {s}")), i.take.as_ref().map(|t| format!("Tk {t}")), i.circled.then(|| "(circled)".to_owned())].into_iter().flatten().collect::<Vec<_>>().join(" ");
            ed::Take { media: *id, from: None, to: None, label }
        })
        .collect();
    let name = match (&scene, circled) {
        (Some(s), true) => format!("Scene {s} circled takes"),
        (Some(s), false) => format!("Scene {s} string-out"),
        (None, true) => "Circled takes".to_owned(),
        (None, false) => "String-out".to_owned(),
    };
    match ed::string_out(&mut app.doc.model, &name, &takes, 0) {
        Ok((e, id)) => {
            if app.doc.apply(&name, e) {
                app.doc.apply("Switch sequence", m::Edit::SetActiveSequence { id });
                app.editor.selection.clear();
            }
        }
        Err(e) => toast(ctx, e.to_string()),
    }
}

fn sequences(app: &mut StudioApp, ui: &mut egui::Ui) {
    let active = app.doc.seq();
    let list: Vec<(m::SequenceId, String, Time, usize)> = app.doc.model.sequences.iter().map(|s| (s.id, s.name.clone(), s.duration(), s.clips.iter().filter(|c| !c.is_gap()).count())).collect();
    let mut act: Option<(&str, m::SequenceId)> = None;
    // A table keeps its columns: wider than the window, it scrolls sideways.
    kit::scroll_wide(ui, "cut-seqs-scroll", |ui| egui::Grid::new("cut-seqs").striped(true).num_columns(4).show(ui, |ui| {
        for (id, name, dur, n) in &list {
            let on = *id == active;
            if ui.selectable_label(on, name).clicked() && !on {
                act = Some(("switch", *id));
            }
            ui.label(RichText::new(format!("{n} clips · {} s", secs(*dur))).font(theme::caption()));
            if crate::kit::Button::ghost("Duplicate").compact().show(ui).clicked() {
                act = Some(("duplicate", *id));
            }
            if ui.add_enabled_ui(!on, |ui| crate::kit::Button::ghost("Delete").compact().show(ui)).inner.clicked() {
                act = Some(("delete", *id));
            }
            ui.end_row();
        }
    }));
    if crate::kit::Button::secondary("New sequence").compact().show(ui).clicked() {
        let n = format!("Sequence {}", list.len() + 1);
        let (e, id) = ed::new_sequence(&mut app.doc.model, &n);
        if app.doc.apply("New sequence", e) {
            act = Some(("switch", id));
        }
    }
    match act {
        Some(("switch", id)) => {
            app.doc.apply("Switch sequence", m::Edit::SetActiveSequence { id });
            app.editor.selection.clear();
        }
        Some(("duplicate", id)) => {
            let name = format!("{} copy", app.doc.model.sequence(id).map(|s| s.name.as_str()).unwrap_or("Sequence"));
            if let Ok((e, new)) = ed::duplicate_sequence(&mut app.doc.model, id, &name)
                && app.doc.apply("Duplicate sequence", e)
            {
                app.doc.apply("Switch sequence", m::Edit::SetActiveSequence { id: new });
                app.editor.selection.clear();
            }
        }
        Some(("delete", id)) => {
            app.doc.apply("Delete sequence", m::Edit::RemoveSequence { id });
        }
        _ => {}
    }
}

fn script(app: &mut StudioApp, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    // A line of the script finds its takes.
    ui.label(RichText::new("Find the takes that say a line").strong());
    let mut find = false;
    ui.horizontal_wrapped(|ui| {
        let r = ui.add(egui::TextEdit::singleline(&mut app.editor.cutting.panel.line).hint_text("a line of the script").desired_width(300.0));
        find = (r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))) | crate::kit::Button::secondary("Find takes").compact().show(ui).clicked();
    });
    if find && app.editor.cutting.panel.finding.is_none() {
        let line = app.editor.cutting.panel.line.clone();
        let model = app.doc.model.clone();
        let media = app.doc.project.media.clone();
        let slot: Pending<Vec<TakeHit>> = Arc::default();
        let out = slot.clone();
        std::thread::spawn(move || *out.lock().unwrap_or_else(|e| e.into_inner()) = Some(take_hits(&model, &media, &line, 20)));
        app.editor.cutting.panel.finding = Some(slot);
    }
    if app.editor.cutting.panel.finding.is_some() {
        ui.label("Listening to the footage…");
    }
    let mut pick = None;
    match &app.editor.cutting.panel.hits {
        Some(Ok(hits)) if hits.is_empty() => {
            ui.label("No take says that line");
        }
        Some(Ok(hits)) => {
            for h in hits {
                let tag = if h.in_cut.is_empty() { String::new() } else { " · in the cut".to_owned() };
                if ui.link(format!("{} {} {:.2}–{:.2} s ({:.0}%{tag}): {}", h.take, h.name, h.from, h.to, h.score * 100.0, h.says)).on_hover_text("Open in the source monitor, marked").clicked() {
                    pick = Some(h.clone());
                }
            }
        }
        Some(Err(e)) => {
            ui.label(RichText::new(e).color(theme::tokens(ui.ctx()).danger));
        }
        None => {}
    }
    if let Some(h) = pick {
        open_source(app, ed::Source::Media(m::MediaId(h.media as u64)), (h.from * SEC as f64) as Time, true);
        if let Some(s) = &mut app.editor.cutting.source {
            s.mark_out = Some((h.to * SEC as f64) as Time);
        }
    }
    ui.separator();
    // The clip under the playhead, as words.
    let seq = app.doc.model.active();
    let clip = app.editor.selection.first().and_then(|id| seq.clip(m::ClipId(*id as u64))).or_else(|| seq.clip_at(seq.spine_id(), app.editor.playhead)).filter(|c| c.media_id().is_some()).cloned();
    let Some(clip) = clip else {
        ui.label("Put the playhead on a clip with speech to edit it by its words");
        return;
    };
    let p = &mut app.editor.cutting.panel;
    if p.words_of != Some(clip.id.0 as u32) {
        p.words_of = Some(clip.id.0 as u32);
        p.lines = None;
        p.marked.clear();
        let model = app.doc.model.clone();
        let c = clip.clone();
        let slot: Pending<Vec<Line>> = Arc::default();
        let out = slot.clone();
        std::thread::spawn(move || *out.lock().unwrap_or_else(|e| e.into_inner()) = Some(clip_lines(&model, &c)));
        p.hearing = Some(slot);
    }
    ui.label(RichText::new(format!("{}: click words to mark them, then Cut", clip.name)).strong());
    let (lo, hi) = (clip.source_in as f64 / 1e6, clip.source_out() as f64 / 1e6);
    let mut toggle = None;
    match &p.lines {
        None => {
            ui.label("Listening…");
        }
        Some(Err(e)) => {
            ui.label(RichText::new(e).color(theme::tokens(ui.ctx()).danger));
        }
        Some(Ok(lines)) => {
            for (li, l) in lines.iter().enumerate() {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 3.0;
                    let raw: Vec<&str> = l.text.split_whitespace().collect();
                    for (wi, w) in l.words.iter().enumerate() {
                        if w.to <= lo || w.from >= hi {
                            continue;
                        }
                        let text = raw.get(wi).copied().unwrap_or(&w.norm);
                        let marked = p.marked.contains(&(li, wi));
                        let rt = if marked { RichText::new(text).strikethrough().color(theme::tokens(ui.ctx()).danger) } else { RichText::new(text) };
                        if ui.add(egui::Label::new(rt).sense(egui::Sense::click())).clicked() {
                            toggle = Some((li, wi));
                        }
                    }
                });
            }
        }
    }
    if let Some(k) = toggle
        && !p.marked.remove(&k)
    {
        p.marked.insert(k);
    }
    let n = p.marked.len();
    if ui.add_enabled(n > 0, egui::Button::new(format!("Cut {n} word(s) from picture and sound"))).clicked() {
        let Some(Ok(lines)) = &p.lines else { return };
        let ranges: Vec<(Time, Time)> = p.marked.iter().filter_map(|(li, wi)| lines.get(*li)?.words.get(*wi)).map(|w| ((w.from * SEC as f64) as Time, (w.to * SEC as f64) as Time)).collect();
        let seq = app.doc.seq();
        match ed::remove_source_ranges(&mut app.doc.model, seq, clip.id, &ranges) {
            Ok(e) => {
                if app.doc.apply("Text cut", e) {
                    let p = &mut app.editor.cutting.panel;
                    p.marked.clear();
                    p.words_of = None;
                    app.editor.cutting.point = None;
                }
            }
            Err(e) => toast(&ctx, e.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::tracking::test_movie;
    use serde_json::json;

    #[test]
    fn every_tab_draws_and_takes_string_out() {
        let _guard = test_movie::LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let _serial = crate::ui_tests::serial();
        let mut app = StudioApp::new(&cc, crate::cli::Cli { screen: Some(crate::cli::Screen::Editor), fresh: true, ..Default::default() });
        let mut s = crate::agent::Session::new(crate::services::project::new_custom("Takes", 320, 180, 24.0));
        s.run("import", &json!({"paths": [test_movie::movie().to_string_lossy()]})).unwrap();
        let media = s.doc.model.media[0].id;
        s.edit(&[json!({"op": "overwrite", "media": media.0, "src_in": 0, "src_out": 0.5, "at": 0}), json!({"op": "take", "media": media.0, "scene": "12", "take": "3", "circled": true})], "cut", false).unwrap();
        app.doc = s.doc;
        let mut frame = eframe::Frame::_new_kittest();
        app.editor.cutting.panel.open = true;
        for (k, tab) in [Tab::Index, Tab::Checks, Tab::Takes, Tab::Sequences].into_iter().enumerate() {
            app.editor.cutting.panel.tab = tab;
            let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1600.0, 1000.0))), time: Some(k as f64 * 0.1), ..Default::default() };
            crate::editor::test_frame(&ctx, input, |ui| eframe::App::ui(&mut app, ui, &mut frame));
        }
        let before = app.doc.model.sequences.len();
        string_out(&mut app, &ctx, Some("12".into()), true);
        assert_eq!(app.doc.model.sequences.len(), before + 1);
        let s = app.doc.model.active();
        assert_eq!(s.name, "Scene 12 circled takes");
        assert_eq!(s.spine_clips()[0].markers[0].name, "Sc 12 Tk 3 (circled)");
    }

    #[test]
    fn the_checks_tab_offers_the_project_check() {
        let mut h = crate::ui_tests::Harness::new(&["--screen", "editor"]);
        h.app.editor.cutting.panel.open = true;
        h.app.editor.cutting.panel.tab = Tab::Checks;
        h.frames(4);
        let labels = h.labels();
        assert!(labels.iter().any(|l| l == "Check project"), "{labels:?}");
        assert!(labels.iter().any(|l| l.contains("Project checks")), "{labels:?}");
    }
}
