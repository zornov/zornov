// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Editing text where it is drawn in the Player.
//!
//! Double-click a title, one of a template's text controls (a lower
//! third's name, its role, …) or a 3D text, and a text cursor opens right
//! there, at the spot clicked. Enter or a click outside keeps the words as
//! one undo step; Esc puts everything back.
//!
//! A template's text control is edited where its words are drawn and
//! written back to that control's value, as the Inspector's Template
//! section writes it.

use super::viewer::Placed;
use crate::app::StudioApp;
use crate::kit;
use crate::theme;
use eframe::egui::{self, Align2, CornerRadius, Key, Modifiers, Pos2, Rect, Vec2};
use serde_json::{Map, Value, json};
use studio_model as m;

/// What the words belong to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// A title clip's own text.
    Title,
    /// A template's text control, by its id.
    Template(&'static str),
    /// A 3D text.
    Solid3d,
}

/// A text being edited on the canvas.
#[derive(Clone, Debug)]
pub struct Editing {
    pub clip: u32,
    pub target: Target,
    pub text: String,
    original: String,
    /// Where the click landed, to put the cursor there on the first frame.
    click: Option<Pos2>,
    placed_caret: bool,
}

impl Editing {
    pub fn target(&self) -> (u32, Target) {
        (self.clip, self.target)
    }
}

/// Words on the canvas that can be edited, in screen points.
#[derive(Clone, Debug)]
pub struct Spot {
    pub clip: u32,
    pub target: Target,
    pub text: String,
    pub centre: Pos2,
    pub size: Vec2,
    /// The font size on screen.
    pub px: f32,
    pub angle: f32,
}

impl Spot {
    fn contains(&self, p: Pos2) -> bool {
        let d = p - self.centre;
        let (s, c) = (-self.angle).sin_cos();
        let local = Vec2::new(d.x * c - d.y * s, d.x * s + d.y * c);
        local.x.abs() <= self.size.x / 2.0 && local.y.abs() <= self.size.y / 2.0
    }
    fn rect(&self) -> Rect {
        Rect::from_center_size(self.centre, self.size)
    }
}

/// Every editable text under the playhead, topmost first. `placed` is
/// where the viewer put each layer.
pub fn spots(app: &StudioApp, canvas: Rect, scale: f32, placed: &[Placed]) -> Vec<Spot> {
    let proj = &app.doc.project;
    let time = app.editor.playhead;
    let seq = app.doc.model.active();
    let mut out = Vec::new();
    for c in proj.visual_stack_at(time) {
        let tr = c.transform;
        let k = tr.scale / 100.0;
        let angle = tr.rotation.to_radians();
        let model = seq.clip(m::ClipId(c.id as u64));
        let meta = model.map(|mc| &mc.meta);
        if let Some(t) = meta.and_then(|x| x.get("template")) {
            let Some(tpl) = t.get("id").and_then(Value::as_str).and_then(studio_mograph::find) else { continue };
            let vals: Map<String, Value> = t.get("controls").and_then(Value::as_object).cloned().unwrap_or_default();
            let dur = t.get("duration").and_then(Value::as_f64).unwrap_or(c.dur as f64 / 1e6);
            let local = (time - c.start) as f64 / 1e6;
            let (w, h) = (proj.width as f64, proj.height as f64);
            let (s, co) = angle.sin_cos();
            for sp in crate::agent::text_spots(tpl, &vals, w, h, dur) {
                if local < sp.start || local >= sp.start + sp.duration {
                    continue;
                }
                let off = Vec2::new(sp.centre[0] as f32, sp.centre[1] as f32) * k;
                let off = Vec2::new(off.x * co - off.y * s, off.x * s + off.y * co);
                let centre = canvas.center() + (Vec2::new(tr.x, tr.y) + off) * scale;
                let size = Vec2::new(sp.width as f32 * k, sp.size as f32 * 1.25 * k) * scale;
                out.push(Spot { clip: c.id, target: Target::Template(sp.control), text: sp.text, centre, size: size + Vec2::splat(8.0), px: sp.size as f32 * k * scale, angle });
            }
            continue;
        }
        if let Some(words) = meta.and_then(|x| x.get("text_3d")).and_then(|t| t.get("text")).and_then(Value::as_str) {
            if let Some(pl) = placed.iter().find(|p| p.id == c.id) {
                out.push(Spot { clip: c.id, target: Target::Solid3d, text: words.to_owned(), centre: pl.center, size: pl.size, px: 28.0, angle: pl.angle });
            }
            continue;
        }
        if c.kind == crate::view::ClipKind::Title && !c.text.is_empty() {
            if let Some(pl) = placed.iter().find(|p| p.id == c.id) {
                let px = c.extra.text_size * k * scale * proj.height as f32 / 1080.0;
                out.push(Spot { clip: c.id, target: Target::Title, text: c.text.clone(), centre: pl.center, size: pl.size, px, angle: pl.angle });
            }
        }
    }
    out
}

/// Starts, shows and ends the on-canvas text editor. Call it after the
/// picture and its handles are drawn.
pub fn interact(app: &mut StudioApp, ui: &mut egui::Ui, canvas: Rect, scale: f32, placed: &[Placed]) {
    let ctx = ui.ctx().clone();
    #[cfg(test)]
    ctx.data_mut(|d| d.insert_temp(egui::Id::new("canvas-text-spots"), spots(app, canvas, scale, placed)));
    if app.editor.text_edit.is_none() {
        let hit = ui.input(|i| if i.pointer.button_double_clicked(egui::PointerButton::Primary) { i.pointer.interact_pos() } else { None });
        let Some(pos) = hit.filter(|p| canvas.contains(*p) && ctx.layer_id_at(*p) == Some(ui.layer_id())) else { return };
        let Some(spot) = spots(app, canvas, scale, placed).into_iter().find(|s| s.contains(pos)) else { return };
        app.editor.selection = vec![spot.clip];
        app.editor.playing = false;
        app.editor.text_edit = Some(Editing { clip: spot.clip, target: spot.target, original: spot.text.clone(), text: spot.text, click: Some(pos), placed_caret: false });
        ctx.request_repaint();
        return;
    }
    let Some(mut ed) = app.editor.text_edit.take() else { return };
    let Some(spot) = spots(app, canvas, scale, placed).into_iter().find(|s| (s.clip, s.target) == ed.target()) else {
        // The words moved off the picture (or the clip went): nothing to edit.
        return;
    };
    let t = theme::tokens(&ctx);
    let id = egui::Id::new(("canvas-text", ed.clip));
    let font = egui::FontId::new(spot.px.clamp(12.0, 160.0), egui::FontFamily::Name(theme::SEMIBOLD.into()));
    // Room for what is typed, at least what was there.
    let typed = ui.painter().layout_no_wrap(ed.text.clone(), font.clone(), t.text);
    let w = typed.size().x.max(spot.size.x - 8.0).max(80.0) + 20.0;
    let h = typed.size().y.max(spot.size.y - 8.0) + 10.0;
    let mut rect = Rect::from_center_size(spot.centre, Vec2::new(w, h));
    let area = canvas.expand(0.0);
    if rect.left() < area.left() {
        rect = rect.translate(Vec2::X * (area.left() - rect.left()));
    }
    if rect.right() > area.right() {
        rect = rect.translate(Vec2::X * (area.right() - rect.right()));
    }
    let radius = CornerRadius::same(theme::radius::CONTROL);
    ui.painter().rect_filled(rect, radius, theme::alpha(t.panel, 0.92));
    kit::focus_ring(ui.painter(), rect, radius, &t);
    // Esc: back as it was, nothing was written while typing. (egui lets go
    // of the field's focus on Esc before this runs, and other keys may have
    // taken the press, so losing focus without Enter or a click elsewhere
    // is a cancel too.)
    if ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
        ctx.memory_mut(|m| m.surrender_focus(id));
        return;
    }
    let enter = ui.input(|i| i.key_pressed(Key::Enter));
    let inner = rect.shrink2(Vec2::new(10.0, 5.0));
    let resp = ui.put(inner, egui::TextEdit::singleline(&mut ed.text).id(id).font(font.clone()).text_color(t.text).frame(egui::Frame::NONE).margin(egui::Margin::ZERO).desired_width(inner.width()).align(Align2::CENTER_CENTER));
    resp.widget_info({
        let words = ed.text.clone();
        move || egui::WidgetInfo::text_edit(true, words.clone(), words.clone(), "Edit text")
    });
    if !ed.placed_caret {
        ed.placed_caret = true;
        resp.request_focus();
        // The cursor goes where the click was.
        let at = ed.click.take().map(|p| {
            let g = ui.painter().layout_no_wrap(ed.text.clone(), font.clone(), t.text);
            let origin = egui::pos2(resp.rect.center().x - g.size().x / 2.0, resp.rect.top());
            g.cursor_from_pos(p - origin)
        });
        let mut state = egui::text_edit::TextEditState::load(&ctx, id).unwrap_or_default();
        state.cursor.set_char_range(Some(egui::text::CCursorRange::one(at.unwrap_or_else(|| egui::text::CCursor::new(egui::text::CharIndex(ed.text.chars().count()))))));
        state.store(&ctx, id);
        ctx.request_repaint();
        app.editor.text_edit = Some(ed);
        return;
    }
    let outside = ui.input(|i| i.pointer.any_pressed()) && ui.input(|i| i.pointer.interact_pos()).is_some_and(|p| !rect.contains(p));
    if outside || (resp.lost_focus() && enter) {
        commit(app, &ctx, &ed);
        return;
    }
    if resp.lost_focus() {
        return;
    }
    // Keep the field focused while it is up, and the frame coming.
    if !resp.has_focus() {
        resp.request_focus();
    }
    app.editor.text_edit = Some(ed);
}

/// Writes the words back as one undo step, when they changed.
fn commit(app: &mut StudioApp, ctx: &egui::Context, ed: &Editing) {
    if ed.text == ed.original {
        return;
    }
    match ed.target {
        Target::Title => {
            let (id, text) = (ed.clip, ed.text.clone());
            app.doc.edit("Edit text", |p| {
                if let Some(c) = p.clip_mut(id) {
                    c.text = text;
                }
            });
        }
        Target::Template(control) => {
            let r = super::mograph::run(app, "Edit text", vec![json!({"op": "template", "clip": ed.clip, "controls": {control: ed.text}})]);
            if let Err(e) = r {
                kit::push(ctx, e, None, Some(crate::icons::Icon::Warning));
            }
        }
        Target::Solid3d => {
            let r = super::mograph::run(app, "Edit text", vec![json!({"op": "text_3d", "clip": ed.clip, "text": ed.text})]);
            if let Err(e) = r {
                kit::push(ctx, e, None, Some(crate::icons::Icon::Warning));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_tests::Harness;
    use eframe::egui::{Event, PointerButton, RawInput};

    struct Drive {
        h: Harness,
        frame: eframe::Frame,
        t: f64,
    }

    impl Drive {
        fn new() -> Drive {
            let h = Harness::new(&["--screen", "editor"]);
            let mut d = Drive { h, frame: eframe::Frame::_new_kittest(), t: 0.0 };
            d.frames(3);
            d
        }
        fn step(&mut self, events: Vec<Event>) {
            self.t += 1.0 / 60.0;
            let input = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1600.0, 1000.0))), time: Some(self.t), events, ..Default::default() };
            let app = &mut self.h.app;
            let frame = &mut self.frame;
            crate::editor::test_frame(&self.h.ctx, input, |ui| eframe::App::ui(app, ui, frame));
        }
        fn frames(&mut self, n: usize) {
            for _ in 0..n {
                self.step(vec![]);
            }
        }
        fn button(pos: Pos2, pressed: bool) -> Event {
            Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE }
        }
        fn key(key: Key, modifiers: Modifiers) -> Vec<Event> {
            vec![Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }, Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers }]
        }
        fn spot(&self, control: &str) -> Spot {
            let all: Vec<Spot> = self.h.ctx.data(|d| d.get_temp(egui::Id::new("canvas-text-spots"))).unwrap_or_default();
            all.into_iter().find(|s| matches!(s.target, Target::Template(c) if c == control)).unwrap_or_else(|| panic!("no spot for {control}"))
        }
        fn double_click(&mut self, at: Pos2) {
            self.step(vec![Event::PointerMoved(at)]);
            for _ in 0..2 {
                self.step(vec![Drive::button(at, true)]);
                self.step(vec![Drive::button(at, false)]);
            }
            self.frames(3);
        }
        /// The value of a placed template's control.
        fn control(&self, clip: u32, id: &str) -> Option<String> {
            let seq = self.h.app.doc.model.active();
            let mc = seq.clip(m::ClipId(clip as u64))?;
            mc.meta.get("template")?.get("controls")?.get(id)?.as_str().map(str::to_owned)
        }
        fn place_lower_third(&mut self) -> u32 {
            let r = super::super::mograph::run(&mut self.h.app, "Add template", vec![json!({"op": "template", "id": "lower_third.bar", "at": 0, "controls": {"name": "Jordan Rivers"}})]).expect("the template places");
            let id = r.iter().find_map(|n| n.split("clip ").nth(1)?.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()).expect("a clip id in the notes");
            // Two seconds in, when the words have all come on.
            self.h.app.editor.playhead = 2_000_000;
            self.h.app.editor.selection = vec![id];
            self.frames(3);
            id
        }
    }

    #[test]
    fn double_click_on_template_text_edits_it_and_enter_writes_the_control() {
        let mut d = Drive::new();
        let clip = d.place_lower_third();
        assert_eq!(d.control(clip, "name").as_deref(), Some("Jordan Rivers"));
        let at = d.spot("name").centre;
        d.double_click(at);
        let ed = d.h.app.editor.text_edit.as_ref().expect("double-click starts editing");
        assert_eq!(ed.target(), (clip, Target::Template("name")));
        // Type over all of it, then Enter.
        d.step(Drive::key(Key::A, Modifiers::COMMAND));
        d.step(vec![Event::Text("Ada Lovelace".into())]);
        d.frames(2);
        // Nothing is written while typing.
        assert_eq!(d.control(clip, "name").as_deref(), Some("Jordan Rivers"));
        d.step(Drive::key(Key::Enter, Modifiers::NONE));
        d.frames(3);
        assert!(d.h.app.editor.text_edit.is_none());
        assert_eq!(d.control(clip, "name").as_deref(), Some("Ada Lovelace"));
        // One undo step puts the old name back.
        let before = d.h.app.doc.can_undo();
        assert!(before);
        d.h.app.doc.undo();
        assert_eq!(d.control(clip, "name").as_deref(), Some("Jordan Rivers"));
    }

    #[test]
    fn escape_leaves_the_control_as_it_was() {
        let mut d = Drive::new();
        let clip = d.place_lower_third();
        let at = d.spot("role").centre;
        d.double_click(at);
        assert!(d.h.app.editor.text_edit.is_some());
        d.step(Drive::key(Key::A, Modifiers::COMMAND));
        d.step(vec![Event::Text("Gaffer".into())]);
        d.frames(2);
        d.step(Drive::key(Key::Escape, Modifiers::NONE));
        d.frames(3);
        assert!(d.h.app.editor.text_edit.is_none());
        let seq = d.h.app.doc.model.active();
        let saved = seq.clip(m::ClipId(clip as u64)).and_then(|c| c.meta.get("template")?.get("controls")?.get("role").cloned());
        assert!(saved.is_none(), "the role was never written: {saved:?}");
    }
}
