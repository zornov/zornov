// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Inspector (right): section cards for the selection. Every numeric
//! control is a field row (slider + field + reset + keyframe). With nothing
//! selected it shows the sequence settings, never a blank panel.

use super::library::RailItem;
use crate::app::StudioApp;
use crate::icons::{self, Icon};
use crate::kit::{self, Button, FieldRow, IconButton, Section, Segment};
use crate::view::{self, Clip, ClipKind};
use crate::{catalogue, theme};
use studio_model as m;
use eframe::egui::{self, Align2, Color32, CornerRadius, Rect, Sense, Vec2};
use crate::kit::ShowNoted;

pub fn show(app: &mut StudioApp, ui: &mut egui::Ui, r: Rect) {
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r).id_salt("inspector"));
    child.set_clip_rect(r.intersect(ui.clip_rect()));
    body(app, &mut child, r, None);
}

/// Narrow windows: the Inspector floats as a sheet from the right.
pub fn sheet(app: &mut StudioApp, ui: &mut egui::Ui, over: Rect) {
    let ctx = ui.ctx().clone();
    let id = egui::Id::unique("inspector-sheet");
    let k = kit::panel_t(&ctx, id, app.editor.sheet_open);
    if k <= 0.001 {
        return;
    }
    let t = theme::tokens(&ctx);
    let w = super::INSPECTOR_DEFAULT.min(over.width() - 24.0);
    let r = Rect::from_min_size(egui::pos2(over.right() - w * k, over.top()), Vec2::new(w, over.height()));
    egui::Area::new(id.with("area")).order(egui::Order::Foreground).fixed_pos(r.min).show(&ctx, |ui| {
        ui.set_opacity(k.min(1.0));
        let (rr, _) = ui.allocate_exact_size(r.size(), Sense::click());
        let p = ui.painter();
        p.add(theme::popover_shadow().as_shape(rr, CornerRadius::same(theme::radius::PANEL)));
        p.rect(rr, CornerRadius::same(theme::radius::PANEL), t.panel, egui::Stroke::new(1.0, t.line), egui::StrokeKind::Inside);
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rr));
        child.set_clip_rect(rr);
        let close = body(app, &mut child, rr, Some(()));
        if close {
            app.editor.sheet_open = false;
        }
    });
}

/// Returns true when the sheet's close button was clicked.
fn body(app: &mut StudioApp, ui: &mut egui::Ui, r: Rect, sheet: Option<()>) -> bool {
    let t = theme::tokens(ui.ctx());
    let pad = theme::space::PANEL_PAD;
    let sel: Vec<u32> = app.editor.selection.iter().copied().filter(|id| app.doc.project.clip(*id).is_some()).collect();
    let first = sel.first().and_then(|id| app.doc.project.clip(*id)).cloned();

    // Header.
    let header = Rect::from_min_size(r.min + Vec2::splat(pad), Vec2::new(r.width() - pad * 2.0, 28.0));
    let cut = app.editor.selected_cut.and_then(|id| app.doc.project.transitions.iter().find(|t| t.id == id)).cloned();
    let (title, sub) = match (&first, sel.len()) {
        (None, _) if cut.is_some() => {
            let c = cut.as_ref().unwrap();
            (catalogue::cut_name(&c.kind).to_owned(), crate::i18n::tf(format!("Cut · {}", view::format_short(c.dur))))
        }
        (None, _) => ("Details".to_owned(), String::new()),
        (Some(c), 1) => (c.name.clone(), format!("{} · {}", kind_name(c.kind), view::format_short(c.dur))),
        (Some(_), n) => (format!("{n} clips"), "Changes apply to all of them".to_owned()),
    };
    let mut close = false;
    let right_pad = if sheet.is_some() { 32.0 } else { 0.0 };
    if let Some(c) = &first {
        let chip = Rect::from_min_size(header.left_center() - Vec2::new(0.0, 5.0), Vec2::splat(10.0));
        ui.painter().rect_filled(chip, CornerRadius::same(3), super::clip_fill(&t, c.kind));
    }
    let tx = header.left() + if first.is_some() { 18.0 } else { 0.0 };
    let title_c = if first.is_none() && cut.is_none() { t.accent } else { t.text };
    let truncated = kit::paint_text_clipped(ui.painter(), egui::pos2(tx, header.center().y), Align2::LEFT_CENTER, &title, theme::section_title(), title_c, header.right() - tx - right_pad);
    if truncated {
        ui.interact(header, egui::Id::unique("insp-title"), Sense::hover()).on_hover_text(&title);
    }
    if sheet.is_some() {
        let br = Rect::from_center_size(egui::pos2(header.right() - 12.0, header.center().y), Vec2::splat(24.0));
        close = IconButton::new(Icon::X, "Close inspector").shortcut("Esc").size(24.0, 14.0).show_at(ui, br).clicked();
    }
    let body_top = if sub.is_empty() {
        header.bottom() + 6.0
    } else {
        let cut = kit::paint_text_clipped(ui.painter(), egui::pos2(header.left(), header.bottom() + 4.0), Align2::LEFT_TOP, &sub, theme::body(), t.text_2, header.width());
        if cut {
            ui.interact(Rect::from_min_size(egui::pos2(header.left(), header.bottom() + 2.0), Vec2::new(header.width(), 20.0)), egui::Id::unique("insp-sub"), Sense::hover()).on_hover_text(&sub);
        }
        header.bottom() + 26.0
    };
    ui.painter().hline(r.x_range(), body_top - 0.5, egui::Stroke::new(1.0, t.line));
    let body_r = Rect::from_min_max(egui::pos2(r.left(), body_top + 8.0), r.max - Vec2::new(0.0, 4.0));
    let mut bui = ui.new_child(egui::UiBuilder::new().max_rect(body_r));
    bui.set_clip_rect(body_r.intersect(ui.clip_rect()));
    egui::ScrollArea::vertical().id_salt("insp-scroll").auto_shrink([false, false]).show_v(&mut bui, |ui| {
        egui::Frame::new().inner_margin(egui::Margin { left: pad as i8, right: pad as i8, top: 0, bottom: 12 }).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = theme::space::SECTION_GAP;
            match (first, app.editor.selected_cut) {
                (None, Some(cut)) => cut_section(app, ui, cut),
                (None, None) => sequence(app, ui),
                (Some(c), _) => clip_sections(app, ui, &sel, &c),
            }
        });
    });
    close
}

fn kind_name(k: ClipKind) -> &'static str {
    match k {
        ClipKind::Video => "Video",
        ClipKind::Audio => "Audio",
        ClipKind::Title => "Text",
        ClipKind::Fx => "Adjustment",
    }
}

fn sequence(app: &mut StudioApp, ui: &mut egui::Ui) {
    let t = theme::tokens(ui.ctx());
    let p = app.doc.project.clone();
    ui.spacing_mut().item_spacing.y = 10.0;
    let row = |ui: &mut egui::Ui, k: &str, v: &str| {
        ui.horizontal_top(|ui| {
            let (lr, lresp) = ui.allocate_exact_size(Vec2::new(96.0, 16.0), Sense::hover());
            let cut = kit::paint_text_clipped(ui.painter(), lr.left_center(), Align2::LEFT_CENTER, k, theme::caption(), t.text_2, 92.0);
            kit::tip_if_cut(lresp, cut, k);
            ui.add(egui::Label::new(egui::RichText::new(v).font(theme::caption()).color(t.text)).wrap());
        });
    };
    let gcd = |mut a: u32, mut b: u32| {
        while b != 0 {
            (a, b) = (b, a % b);
        }
        a.max(1)
    };
    let g = gcd(p.width, p.height);
    let place = app.doc.folder.as_ref().map(|f| f.root().display().to_string()).unwrap_or_else(|| "Not saved yet".into());
    row(ui, "Name:", &p.name);
    row(ui, "Location:", &place);
    row(ui, "Imported media:", &format!("{} items", p.media.len()));
    let (x0, x1) = (ui.max_rect().left(), ui.max_rect().right());
    kit::hline(ui, x0..=x1, ui.cursor().top());
    row(ui, "Aspect ratio:", &format!("{}:{}", p.width / g, p.height / g));
    row(ui, "Resolution:", &format!("{}×{}", p.width, p.height));
    row(ui, "Frame rate:", &format!("{:.2}fps", p.fps));
    row(ui, "Length:", &view::format_duration(p.duration()));

    // Canvas settings open from "Modify".
    let open_id = egui::Id::unique("details-modify");
    let mut open = ui.data(|d| d.get_temp::<bool>(open_id)).unwrap_or(false);
    ui.add_space(4.0);
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
        if Button::secondary(if open { "Done" } else { "Modify" }).compact().show(ui).clicked() {
            open = !open;
        }
    });
    ui.data_mut(|d| d.insert_temp(open_id, open));
    if open {
        let mut w = p.width as f32;
        let mut h = p.height as f32;
        let a = FieldRow::new("Width", &mut w, 16.0..=8192.0).default(1920.0).unit("px").no_slider().step(2.0).scrub(2.0).show(ui);
        let b = FieldRow::new("Height", &mut h, 16.0..=8192.0).default(1080.0).unit("px").no_slider().step(2.0).scrub(2.0).show(ui);
        if a.changed || b.changed {
            app.doc.edit_gesture("canvas-size", "Canvas size", |p| {
                p.width = w as u32;
                p.height = h as u32;
            });
        }
        kit::labeled_row(ui, "Frame rate", |ui| {
            let rates = [24.0, 25.0, 30.0, 50.0, 60.0];
            let names = ["24 fps", "25 fps", "30 fps", "50 fps", "60 fps"];
            let cur = rates.iter().position(|r| *r == p.fps).unwrap_or(2);
            if let Some(i) = kit::menu_button(ui, "seq-fps", cur, &names, ui.available_width() - 40.0, None) {
                app.doc.edit("Frame rate", |p| p.fps = rates[i]);
            }
        });
        kit::labeled_row(ui, "Background", |ui| {
            let mut c = p.background;
            if kit::colour_well(ui, "seq-bg", &mut c, None).changed() {
                app.doc.edit_gesture("seq-bg", "Background", |p| p.background = c);
            }
            ui.label(egui::RichText::new(kit::to_hex(c)).font(theme::body()).color(t.text_2));
        });
    }
}

/// A selected transition: its kind and length (0.1 s up to what the
/// clips' handles allow).
fn cut_section(app: &mut StudioApp, ui: &mut egui::Ui, id: u64) {
    let Some(tr) = app.doc.project.transitions.iter().find(|t| t.id == id).cloned() else {
        app.editor.selected_cut = None;
        return;
    };
    let seq = app.doc.seq();
    let limit = app.doc.model.max_transition_duration(seq, m::ClipId(tr.left as u64), m::TransitionAlign::default()).unwrap_or(tr.dur).max(view::SEC / 10);
    Section::new("cut", "Cut").icon(Icon::Intersect).show(ui, |ui| {
        let items = catalogue::cuts();
        let names: Vec<&str> = items.iter().map(|i| i.name).collect();
        let cur = items.iter().position(|i| i.id == tr.kind).unwrap_or(0);
        kit::labeled_row(ui, "Style", |ui| {
            if let Some(i) = kit::menu_button(ui, "cut-kind", cur, &names, ui.available_width() - 40.0, None) {
                let kind = items[i].id.to_owned();
                app.doc.edit("Change cut", |p| {
                    if let Some(t) = p.transitions.iter_mut().find(|t| t.id == id) {
                        t.kind = kind;
                    }
                });
            }
        });
        let mut secs = tr.dur as f32 / view::SEC as f32;
        let max = limit as f32 / view::SEC as f32;
        let row = FieldRow::new("Duration", &mut secs, 0.1..=max.max(0.1)).default(0.5f32.min(max)).unit("s").decimals(1).step(0.1).scrub(0.01).show(ui);
        if row.changed {
            let dur = (secs as f64 * view::SEC as f64) as view::Time;
            app.doc.edit_gesture(&format!("cut-dur-{id}"), "Cut duration", |p| {
                if let Some(t) = p.transitions.iter_mut().find(|t| t.id == id) {
                    t.dur = dur;
                }
            });
        }
        ui.horizontal(|ui| {
            ui.add_space(kit::LABEL_W);
            if Button::ghost("Remove cut").icon(Icon::Trash).compact().show(ui).clicked() {
                if app.doc.apply("Remove cut", m::Edit::RemoveTransition { seq, id: m::TransitionId(id) }) {
                    app.editor.selected_cut = None;
                }
            }
        });
    });
    hint(ui, Icon::Info, "Longer cuts need more media past the clips' edges.");
}

fn hint(ui: &mut egui::Ui, icon: Icon, text: &str) {
    let t = theme::tokens(ui.ctx());
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
        icons::paint(ui.painter(), r, icon, 14.0, t.text_2);
        ui.add(egui::Label::new(egui::RichText::new(text).font(theme::body()).color(t.text_2)).wrap());
    });
}

/// A field row bound to a clip property for every selected clip. `path`
/// is the model property path; its keyframe diamond adds or removes a key
/// at the playhead.
#[allow(clippy::too_many_arguments)]
fn prop(
    app: &mut StudioApp,
    ui: &mut egui::Ui,
    ids: &[u32],
    path: &str,
    label: &str,
    get: fn(&Clip) -> f32,
    set: fn(&mut Clip, f32),
    range: std::ops::RangeInclusive<f32>,
    default: f32,
    unit: &str,
    decimals: usize,
) {
    let Some(c) = app.doc.project.clip(ids[0]).cloned() else { return };
    let mut v = get(&c);
    let local = (app.editor.playhead - c.start).clamp(0, c.dur);
    let half_frame = app.editor.frame_dur(app.doc.project.fps) / 2;
    let key_here = c.key_times(path).and_then(|ts| ts.iter().copied().find(|t| (t - local).abs() <= half_frame));
    let mut keyed = key_here.is_some();
    // Only properties the model can animate get a keyframe diamond.
    let keyable = path.starts_with("transform.") || path.starts_with("grade.") || matches!(path, "audio.volume_db" | "audio.pan");
    let mut row = FieldRow::new(label, &mut v, range.clone())
        .default(default)
        .unit(unit)
        .decimals(decimals)
        .scrub(((range.end() - range.start()) / 400.0).max(0.01));
    if keyable {
        row = row.keyframe(&mut keyed);
    }
    let out = row.show(ui);
    if out.changed {
        let ids2 = ids.to_vec();
        app.doc.edit_gesture(&format!("{path}-{label}-{ids2:?}"), label, |p| {
            for id in &ids2 {
                if let Some(c) = p.clip_mut(*id) {
                    set(c, v);
                }
            }
        });
    }
    if out.keyframe_toggled {
        let seq = app.doc.seq();
        let mut edits = Vec::new();
        for id in ids {
            let Some(mc) = app.doc.model.active().clip(m::ClipId(*id as u64)) else { continue };
            let Some(vc) = app.doc.project.clip(*id) else { continue };
            let local = (app.editor.playhead - vc.start).clamp(0, vc.dur);
            match key_here {
                Some(t) if !keyed => edits.push(m::Edit::RemoveKeyframe { seq, clip: mc.id, property: path.into(), time: t }),
                _ if keyed => {
                    let value = mc.property_at(path, local).unwrap_or_else(|| crate::bridge::model_value(path, v));
                    edits.push(m::Edit::SetKeyframe { seq, clip: mc.id, property: path.into(), key: m::Keyframe::new(local, value, m::Interp::Ease) });
                    // Scale has two axes that move together.
                    if path == "transform.scale_x" {
                        let value = mc.property_at("transform.scale_y", local).unwrap_or(m::Value::Scalar(1.0));
                        edits.push(m::Edit::SetKeyframe { seq, clip: mc.id, property: "transform.scale_y".into(), key: m::Keyframe::new(local, value, m::Interp::Ease) });
                    }
                }
                _ => {}
            }
        }
        if !edits.is_empty() {
            app.doc.apply(if keyed { "Add keyframe" } else { "Remove keyframe" }, m::Edit::Batch { edits });
        }
    }
}

/// Parent and track matte rows for one clip.
fn links(app: &mut StudioApp, ui: &mut egui::Ui, id: u64) {
    use crate::services::layers;
    let Some(sq) = app.doc.model.sequence(app.doc.seq()) else { return };
    let Some(me) = sq.clip(studio_model::ClipId(id)).cloned() else { return };
    // Parent: any other picture clip on this timeline that overlaps it.
    let candidates: Vec<(u64, String)> = sq
        .clips
        .iter()
        .filter(|o| o.id != me.id && o.start < me.end() && o.end() > me.start && !matches!(o.source, studio_model::ClipSource::Gap))
        .filter(|o| sq.lanes.iter().find(|l| l.id == o.lane).is_some_and(|l| l.kind != studio_model::LaneKind::Audio))
        .map(|o| (o.id.0, if o.name.is_empty() { crate::i18n::tf(format!("Clip {}", o.id.0)) } else { o.name.clone() }))
        .collect();
    let mut names: Vec<String> = vec!["None".into()];
    names.extend(candidates.iter().map(|(_, n)| n.clone()));
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let current = me.parent.as_ref().and_then(|p| candidates.iter().position(|(c, _)| *c == p.clip.0)).map_or(0, |i| i + 1);
    let playhead = app.editor.playhead;
    let mut said: Option<Result<String, String>> = None;
    kit::labeled_row(ui, "Parent", |ui| {
        if let Some(i) = kit::menu_button(ui, "parent", current, &refs, ui.available_width() - 40.0, None) {
            said = Some(layers::set_parent(&mut app.doc, id, (i > 0).then(|| candidates[i - 1].0), playhead));
        }
    });
    // Track matte: shows through the clip on the lane above.
    let now = layers::matte_of(&app.doc, id);
    let modes: Vec<&str> = std::iter::once("None").chain(layers::MATTES.iter().map(|m| m.1)).collect();
    let cur = now.and_then(|(mode, _)| layers::MATTES.iter().position(|m| m.0 == mode)).map_or(0, |i| i + 1);
    kit::labeled_row(ui, "Track matte", |ui| {
        if let Some(i) = kit::menu_button(ui, "track-matte", cur, &modes, ui.available_width() - 40.0, None) {
            said = Some(set_matte(app, id, now, i));
        }
    });
    if let Some(Err(e)) = said {
        kit::push(ui.ctx(), e, None, None);
    }
}

/// Puts on, changes or takes off a clip's track matte from the Inspector.
fn set_matte(app: &mut StudioApp, id: u64, now: Option<(&'static str, u64)>, choice: usize) -> Result<String, String> {
    use crate::services::layers;
    use studio_model as m;
    match (now, choice) {
        (None, 0) => Ok(String::new()),
        (None, i) => {
            let above = layers::layer_above(&app.doc, id).ok_or("Put the matte (a shape, title or picture) on the lane above this clip first.")?;
            let mut ids = vec![app.doc.alloc_id(), app.doc.alloc_id()];
            let mut next = move || ids.remove(0);
            layers::track_matte(&mut app.doc, id, above, layers::MATTES[i - 1].0, &mut next).map(|_| String::new())
        }
        (Some((_, matte)), i) => {
            let Some(c) = app.doc.model.sequences.iter().find_map(|s| s.clip(m::ClipId(id))).cloned() else { return Ok(String::new()) };
            let m::ClipSource::Sequence(r) = &c.source else { return Ok(String::new()) };
            let inner = r.sequence;
            let mut mc = app.doc.model.sequence(inner).and_then(|s| s.clip(m::ClipId(matte))).cloned().ok_or("the matte is gone")?;
            mc.blend = if i == 0 { m::BlendMode::Normal } else { layers::MATTES[i - 1].2 };
            app.doc.apply("Track matte", m::Edit::SetClip { seq: inner, clip: Box::new(mc) });
            if i == 0 {
                let seq = app.doc.seq();
                app.doc.apply("Remove matte", m::Edit::ExpandCompound { seq, clip: m::ClipId(id) });
            }
            Ok(String::new())
        }
    }
}

fn edit_all(app: &mut StudioApp, ids: &[u32], label: &str, f: impl Fn(&mut Clip)) {
    let ids = ids.to_vec();
    app.doc.edit(label, |p| {
        for id in &ids {
            if let Some(c) = p.clip_mut(*id) {
                f(c);
            }
        }
    });
}

/// The inspector's tabs for a clip and the section kinds each one shows.
/// `gen` is the generator kind of a generated layer (text, shape, model…),
/// which decides what its first tab is called and holds.
pub(crate) fn clip_tabs(kind: ClipKind, generated: Option<m::GeneratorKind>) -> Vec<(&'static str, Vec<&'static str>)> {
    use m::GeneratorKind as G;
    // Settings no tab above has room for sit in "Advanced", collapsed.
    let advanced = |extra: &[&'static str]| -> (&'static str, Vec<&'static str>) {
        let mut v = vec!["generator", "data"];
        v.extend_from_slice(extra);
        ("Advanced", v)
    };
    match kind {
        ClipKind::Audio => vec![("Audio", vec!["clip", "audio", "audio-fx"]), ("Speed", vec!["speed"]), advanced(&[])],
        ClipKind::Title => {
            let first = match generated {
                Some(G::Text) | None => "Text",
                Some(G::Model) => "Model",
                _ => "Shape",
            };
            // A model's own panel (parts, modifiers, materials, UVs,
            // shading) sits under the Model section; its rigid-body physics
            // with the other physics, under Animation.
            vec![
                (first, vec!["clip", "text", "shape", "mograph-template", "mograph-shape", "mograph-text", "model", "model-parts", "model-modifiers", "model-materials", "model-uv", "model-object", "world-spec", "sim-settings", "transform", "crop", "space", "mask", "pins"]),
                ("Animation", vec!["motion", "modifiers", "physics", "physics3d", "drivers", "skeleton", "character3d", "sim3d"]),
                ("Adjust", vec!["colour", "effects"]),
                advanced(&["world3d"]),
            ]
        }
        ClipKind::Fx => vec![("Adjust", vec!["clip", "colour", "effects"]), advanced(&[])],
        ClipKind::Video => vec![
            ("Video", vec!["clip", "mograph-template", "transform", "crop", "space", "mask", "pins", "cutout", "tracking"]),
            ("Audio", vec!["audio", "audio-fx"]),
            ("Speed", vec!["speed", "slow-motion"]),
            ("Animation", vec!["motion", "modifiers", "physics", "drivers", "skeleton"]),
            ("Adjust", vec!["colour", "effects", "match-colour", "in-scene"]),
            advanced(&[]),
        ],
    }
}

/// Every section kind some tab shows (what the tests check each section against).
pub(crate) fn all_tab_kinds() -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    for kind in [ClipKind::Video, ClipKind::Audio, ClipKind::Title, ClipKind::Fx] {
        for g in std::iter::once(None).chain(super::params::ALL_KINDS.iter().copied().map(Some)) {
            out.extend(clip_tabs(kind, g).into_iter().flat_map(|t| t.1));
        }
    }
    for g in [m::GeneratorKind::Camera, m::GeneratorKind::Light] {
        out.extend(super::three_d::camera_light_tabs(g).into_iter().flat_map(|t| t.1));
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// The tab row: labels with an accent underline under the active one.
fn tab_row(ui: &mut egui::Ui, names: &[&str], cur: &mut usize) {
    // Wider than the panel: the row scrolls sideways, one line.
    let full = ui.available_width();
    kit::scroll_strip(ui, "insp-tabs", |ui| tab_cells(ui, names, cur, full));
}

fn tab_cells(ui: &mut egui::Ui, names: &[&str], cur: &mut usize, full: f32) {
    let t = theme::tokens(ui.ctx());
    let natural: f32 = names.iter().map(|n| kit::text_width(ui, n, theme::body()) + 18.0).sum();
    let (r, _) = ui.allocate_exact_size(Vec2::new(natural.max(full), 30.0), Sense::hover());
    let mut x = r.left();
    for (i, n) in names.iter().enumerate() {
        let w = kit::text_width(ui, n, theme::body()) + 18.0;
        let cell = Rect::from_min_size(egui::pos2(x, r.top()), Vec2::new(w, r.height()));
        let resp = ui.interact(cell, egui::Id::unique(("insp-tab", *n)), Sense::click());
        let active = *cur == i;
        let c = if active { t.accent } else if resp.hovered() { t.text } else { t.text_2 };
        ui.painter().text(cell.center(), Align2::CENTER_CENTER, *n, theme::body(), c);
        if active {
            ui.painter().rect_filled(Rect::from_min_max(egui::pos2(cell.left() + 6.0, cell.bottom() - 2.0), egui::pos2(cell.right() - 6.0, cell.bottom())), CornerRadius::same(1), t.accent);
        }
        if resp.clicked() {
            *cur = i;
        }
        x += w;
    }
    ui.painter().hline(r.x_range(), r.bottom() - 0.5, egui::Stroke::new(1.0, t.line));
}

fn clip_sections(app: &mut StudioApp, ui: &mut egui::Ui, ids: &[u32], c: &Clip) {
    let rig = super::three_d::tabs_for(app, c);
    let key = egui::Id::unique(("insp-tab-of", if rig.is_some() { "rig" } else { kind_name(c.kind) }));
    let gk = app.doc.model.active().clip(m::ClipId(c.id as u64)).and_then(|mc| match &mc.source {
        m::ClipSource::Generator(g) => Some(g.kind),
        _ => None,
    });
    let tabs = rig.unwrap_or_else(|| clip_tabs(c.kind, gk));
    let mut cur = ui.data(|d| d.get_temp::<usize>(key)).unwrap_or(0).min(tabs.len() - 1);
    // A workspace can ask for a tab (Colour opens on Adjust, Audio on Audio).
    if let Some(want) = crate::workspaces::take_tab(ui.ctx()) {
        if let Some(i) = tabs.iter().position(|t| t.0 == want.as_str()) {
            cur = i;
            crate::workspaces::tab_shown(ui.ctx());
        }
    }
    if tabs.len() > 1 {
        let names: Vec<&str> = tabs.iter().map(|t| t.0).collect();
        tab_row(ui, &names, &mut cur);
        ui.data_mut(|d| d.insert_temp(key, cur));
    }
    let filter = egui::Id::new(kit::SECTION_FILTER);
    ui.data_mut(|d| d.insert_temp(filter, tabs[cur].1.clone()));
    clip_sections_inner(app, ui, ids, c);
    ui.data_mut(|d| d.remove::<Vec<&'static str>>(filter));
}

fn clip_sections_inner(app: &mut StudioApp, ui: &mut egui::Ui, ids: &[u32], c: &Clip) {
    let t = theme::tokens(ui.ctx());
    let kind = c.kind;
    let visual = c.is_visual() && kind != ClipKind::Fx;

    // Name, on/off and label; the fill of a solid, gradient or shape layer.
    super::params::top(app, ui, ids);

    let is_text = ids.first().and_then(|id| app.doc.model.active().clip(m::ClipId(*id as u64))).is_none_or(|mc| matches!(&mc.source, m::ClipSource::Generator(g) if g.kind == m::GeneratorKind::Text));
    if kind == ClipKind::Title && is_text {
        let out = Section::new("text", "Text").icon(Icon::TextT).resettable(true).show(ui, |ui| {
            let mut text = c.text.clone();
            let w = ui.available_width();
            let resp = kit::text_field(ui, &mut text, "Type your title", None, w, None);
            if resp.changed() {
                let ids2 = ids.to_vec();
                app.doc.edit_gesture(&format!("text-{ids2:?}"), "Edit text", |p| {
                    for id in &ids2 {
                        if let Some(c) = p.clip_mut(*id) {
                            c.text = text.clone();
                        }
                    }
                });
            }
            if resp.lost_focus() {
                app.doc.end_gesture();
            }
            kit::labeled_row(ui, "Colour", |ui| {
                let mut col = c.text_colour;
                if kit::colour_well(ui, "text-colour", &mut col, None).changed() {
                    let ids2 = ids.to_vec();
                    app.doc.edit_gesture(&format!("text-colour-{ids2:?}"), "Text colour", |p| {
                        for id in &ids2 {
                            if let Some(c) = p.clip_mut(*id) {
                                c.text_colour = col;
                            }
                        }
                    });
                }
                ui.label(egui::RichText::new(kit::to_hex(col)).font(theme::body()).color(t.text_2));
            });
            let mut size = c.extra.text_size;
            let row = FieldRow::new("Size", &mut size, 5.0..=300.0).default(40.0).unit("pt").step(1.0).scrub(0.5).show(ui);
            if row.changed {
                let ids2 = ids.to_vec();
                app.doc.edit_gesture(&format!("text-size-{ids2:?}"), "Text size", |p| {
                    for id in &ids2 {
                        if let Some(c) = p.clip_mut(*id) {
                            c.extra.text_size = size;
                        }
                    }
                });
            }
            let mut spacing = c.extra.letter_spacing;
            let row = FieldRow::new("Letter spacing", &mut spacing, -10.0..=100.0).default(0.0).step(1.0).scrub(0.2).show(ui);
            if row.changed {
                let ids2 = ids.to_vec();
                app.doc.edit_gesture(&format!("text-spacing-{ids2:?}"), "Letter spacing", |p| {
                    for id in &ids2 {
                        if let Some(c) = p.clip_mut(*id) {
                            c.extra.letter_spacing = spacing;
                        }
                    }
                });
            }
            let mut a = c.extra.text_align as usize;
            let w = ui.available_width();
            if kit::segmented(ui, "text-align", &mut a, &[Segment::text("Left"), Segment::text("Centre"), Segment::text("Right")], Some(w), None).changed() {
                edit_all(app, ids, "Text align", |c| c.extra.text_align = a as u8);
            }
            if let [_] = ids {
                super::params::text_style_rows(app, ui, ids);
            }
        });
        if out.reset {
            edit_all(app, ids, "Reset text", |c| {
                c.text_colour = Color32::WHITE;
                c.extra.text_align = 1;
            });
        }
    }

    if visual {
        let out = Section::new("transform", "Transform").icon(Icon::ArrowsOutCardinal).resettable(c.transform != view::Transform::default()).show(ui, |ui| {
            // Standard ranges: position ±canvas, scale 1–1000 %,
            // rotation ±360°.
            let (cw, ch) = (app.doc.project.width as f32, app.doc.project.height as f32);
            prop(app, ui, ids, "transform.position", "Position X", |c| c.transform.x, |c, v| c.transform.x = v, -cw..=cw, 0.0, "px", 0);
            prop(app, ui, ids, "transform.position", "Position Y", |c| c.transform.y, |c, v| c.transform.y = v, -ch..=ch, 0.0, "px", 0);
            prop(app, ui, ids, "transform.scale_x", "Scale", |c| c.transform.scale, |c, v| c.transform.scale = v, 1.0..=1000.0, 100.0, "%", 0);
            prop(app, ui, ids, "transform.rotation", "Rotation", |c| c.transform.rotation, |c, v| c.transform.rotation = v, -360.0..=360.0, 0.0, "°", 1);
            prop(app, ui, ids, "transform.opacity", "Opacity", |c| c.transform.opacity, |c, v| c.transform.opacity = v, 0.0..=100.0, 100.0, "%", 0);
            kit::labeled_row(ui, "Blend", |ui| {
                let names: Vec<&str> = crate::bridge::BLENDS.iter().map(|b| b.1).collect();
                if let Some(i) = kit::menu_button(ui, "blend", c.extra.blend as usize, &names, ui.available_width() - 40.0, None) {
                    edit_all(app, ids, "Blend mode", |c| c.extra.blend = i as u8);
                }
            });
            if let [id] = ids {
                links(app, ui, *id as u64);
            }
        });
        if out.reset {
            edit_all(app, ids, "Reset transform", |c| c.transform = view::Transform::default());
        }
    }
    // Crop and flip.
    if visual {
        super::params::crop(app, ui, ids);
    }
    // 3D (on picture clips), or a camera's or a light's settings.
    super::three_d::sections(app, ui, ids, c);
    // Rigs, motion, simulation, render settings and skies.
    super::anim3d::inspector(app, ui, ids, c);

    if kind == ClipKind::Video || kind == ClipKind::Fx {
        let mut on = c.grade.enabled;
        let out = Section::new("colour", "Colour").icon(Icon::DropHalf).enable(&mut on).resettable(c.grade != view::Grade::default() || c.look.is_some()).show(ui, |ui| {
            prop(app, ui, ids, "grade.exposure", "Exposure", |c| c.grade.exposure, |c, v| c.grade.exposure = v, -100.0..=100.0, 0.0, "", 0);
            prop(app, ui, ids, "grade.contrast", "Contrast", |c| c.grade.contrast, |c, v| c.grade.contrast = v, -100.0..=100.0, 0.0, "", 0);
            prop(app, ui, ids, "grade.saturation", "Saturation", |c| c.grade.saturation, |c, v| c.grade.saturation = v, -100.0..=100.0, 0.0, "", 0);
            prop(app, ui, ids, "grade.temperature", "Temperature", |c| c.grade.temperature, |c, v| c.grade.temperature = v, -100.0..=100.0, 0.0, "", 0);
            prop(app, ui, ids, "grade.tint", "Tint", |c| c.grade.tint, |c, v| c.grade.tint = v, -100.0..=100.0, 0.0, "", 0);
            kit::labeled_row(ui, "Look", |ui| match &c.look {
                Some(l) => {
                    let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width() - 34.0, 28.0), Sense::hover());
                    ui.painter().rect_filled(r, CornerRadius::same(6), kit::field_fill(ui));
                    icons::paint(ui.painter(), Rect::from_center_size(egui::pos2(r.left() + 16.0, r.center().y), Vec2::splat(16.0)), Icon::Palette, 14.0, t.accent);
                    kit::paint_text_clipped(ui.painter(), egui::pos2(r.left() + 30.0, r.center().y), Align2::LEFT_CENTER, catalogue::look_name(l), theme::control(), t.text, r.width() - 36.0);
                    if IconButton::new(Icon::X, "Remove look").compact().show(ui).clicked() {
                        edit_all(app, ids, "Remove look", |c| c.look = None);
                    }
                }
                None => {
                    if Button::ghost("Browse looks").icon(Icon::Palette).compact().show(ui).clicked() {
                        app.editor.rail = RailItem::Looks;
                        app.editor.open_panel(crate::dock::Panel::Library);
                    }
                }
            });
        });
        if out.enable_changed {
            edit_all(app, ids, if on { "Turn on colour" } else { "Turn off colour" }, |c| c.grade.enabled = on);
        }
        if out.reset {
            edit_all(app, ids, "Reset colour", |c| {
                c.grade = view::Grade::default();
                c.look = None;
            });
        }
    }

    if kind != ClipKind::Audio {
        Section::new("effects", "Effects").icon(Icon::MagicWand).show(ui, |ui| {
            if c.effects.is_empty() {
                let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 28.0), Sense::hover());
                ui.painter().text(r.left_center(), Align2::LEFT_CENTER, crate::i18n::shown("No effects on this clip."), theme::body(), t.text_2);
            }
            let mut remove = None;
            for (i, e) in c.effects.iter().enumerate() {
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width() - 64.0, 28.0), Sense::hover());
                    ui.painter().rect_filled(r, CornerRadius::same(6), kit::field_fill(ui));
                    icons::paint(ui.painter(), Rect::from_center_size(egui::pos2(r.left() + 16.0, r.center().y), Vec2::splat(16.0)), Icon::MagicWand, 14.0, if e.enabled { t.accent } else { t.text_2 });
                    kit::paint_text_clipped(ui.painter(), egui::pos2(r.left() + 30.0, r.center().y), Align2::LEFT_CENTER, catalogue::effect_name(&e.kind), theme::control(), if e.enabled { t.text } else { t.text_2 }, r.width() - 36.0);
                    let mut on = e.enabled;
                    let tip = if on { "Turn effect off" } else { "Turn effect on" };
                    if kit::toggle(ui, &mut on, tip, None).changed() {
                        let eid = e.id;
                        edit_all(app, &ids[..1], if on { "Turn on effect" } else { "Turn off effect" }, move |c| {
                            if let Some(x) = c.effects.iter_mut().find(|x| x.id == eid) {
                                x.enabled = on;
                            }
                        });
                    }
                    if IconButton::new(Icon::X, "Remove effect").compact().show(ui).clicked() {
                        remove = Some(i);
                    }
                });
                // Numeric parameters, built from the effect's definition.
                if let Some(def) = studio_fx::effects::find(&e.kind) {
                    for pd in def.params {
                        // Choices (a particle preset, a noise kind) as a menu,
                        // written straight to the model's effect.
                        if let studio_fx::ParamKind::Choice(options) = pd.kind {
                            let clip = m::ClipId(ids[0] as u64);
                            let me = app.doc.model.sequences.iter().find_map(|q| q.clip(clip)).and_then(|mc| mc.effects.iter().find(|x| x.id.0 == e.id).cloned());
                            let dflt = match pd.default {
                                studio_fx::ParamValue::Choice(i) => i as usize,
                                _ => 0,
                            };
                            let cur = me
                                .as_ref()
                                .and_then(|x| x.params.get(pd.id))
                                .and_then(|v| match v {
                                    m::Value::Choice(n) | m::Value::Text(n) => options.iter().position(|o| o.eq_ignore_ascii_case(n)),
                                    m::Value::Scalar(x) => Some((*x).max(0.0) as usize),
                                    _ => None,
                                })
                                .unwrap_or(dflt)
                                .min(options.len().saturating_sub(1));
                            let mut pick = None;
                            kit::labeled_row(ui, pd.label, |ui| pick = kit::menu_button(ui, &format!("fx-{}-{}", e.id, pd.id), cur, options, ui.available_width() - 40.0, None));
                            if let (Some(i), Some(mut me)) = (pick, me) {
                                me.params.insert(pd.id.into(), m::Value::Choice(options[i].into()));
                                let seq = app.doc.seq();
                                app.doc.apply(pd.label, m::Edit::SetEffect { seq, clip, effect: me });
                            }
                            continue;
                        }
                        let (lo, hi, int) = match pd.kind {
                            studio_fx::ParamKind::Float { min, max } => (min, max, false),
                            studio_fx::ParamKind::Int { min, max } => (min as f32, max as f32, true),
                            _ => continue,
                        };
                        let dflt = match pd.default {
                            studio_fx::ParamValue::Float(x) => x,
                            studio_fx::ParamValue::Int(x) => x as f32,
                            _ => 0.0,
                        };
                        let mut v = e.params.iter().find(|(k, _)| k == pd.id).map_or(dflt, |(_, x)| *x);
                        let row = FieldRow::new(pd.label, &mut v, lo..=hi).default(dflt).decimals(if int { 0 } else if hi - lo <= 2.0 { 2 } else { 1 }).step(if int { 1.0 } else { 0.0 }).scrub(((hi - lo) / 400.0).max(0.001)).show(ui);
                        if row.changed {
                            let (eid, pid) = (e.id, pd.id.to_owned());
                            let id0 = ids[0];
                            app.doc.edit_gesture(&format!("fx-{eid}-{pid}"), pd.label, |p| {
                                if let Some(x) = p.clip_mut(id0).and_then(|c| c.effects.iter_mut().find(|x| x.id == eid)) {
                                    x.params.retain(|(k, _)| *k != pid);
                                    x.params.push((pid.clone(), v));
                                }
                            });
                        }
                    }
                }
            }
            if let Some(i) = remove {
                edit_all(app, ids, "Remove effect", move |c| {
                    if i < c.effects.len() {
                        c.effects.remove(i);
                    }
                });
            }
            if Button::ghost("Add effect").icon(Icon::Plus).compact().show(ui).clicked() {
                app.editor.rail = RailItem::Effects;
                app.editor.open_panel(crate::dock::Panel::Library);
            }
        });
    }

    if kind == ClipKind::Audio || kind == ClipKind::Video {
        let mut on = c.audio.enabled;
        let out = Section::new("audio", "Audio").icon(Icon::SpeakerHigh).enable(&mut on).resettable(c.audio != view::AudioParams::default()).show(ui, |ui| {
            // Volume reads as 0–200 % (100 % is unchanged, 200 % is +12 dB).
            prop(app, ui, ids, "audio.volume_db", "Volume", |c| db_to_percent(c.audio.volume), |c, v| c.audio.volume = percent_to_db(v), 0.0..=200.0, 100.0, "%", 0);
            prop(app, ui, ids, "audio.pan", "Pan", |c| c.audio.pan, |c, v| c.audio.pan = v, -100.0..=100.0, 0.0, "", 0);
            prop(app, ui, ids, "audio.fade_in", "Fade in", |c| c.audio.fade_in, |c, v| c.audio.fade_in = v, 0.0..=10.0, 0.0, "s", 1);
            prop(app, ui, ids, "audio.fade_out", "Fade out", |c| c.audio.fade_out, |c, v| c.audio.fade_out = v, 0.0..=10.0, 0.0, "s", 1);
        });
        if out.enable_changed {
            edit_all(app, ids, if on { "Unmute clip" } else { "Mute clip" }, |c| c.audio.enabled = on);
        }
        if out.reset {
            edit_all(app, ids, "Reset audio", |c| c.audio = view::AudioParams::default());
        }
        super::audio_fx::section(app, ui, ids);
    }

    if kind == ClipKind::Video || kind == ClipKind::Audio {
        let out = Section::new("speed", "Speed").icon(Icon::Timer).resettable(c.speed != 100.0 || c.extra.reverse).show(ui, |ui| {
            // Speed as a multiplier, 0.1×–100×, with quick picks.
            let mut v = c.speed / 100.0;
            let o = FieldRow::new("Speed", &mut v, 0.1..=100.0).default(1.0).unit("×").decimals(2).step(0.01).scrub(0.01).no_slider().show(ui);
            let mut pick = None;
            ui.horizontal_wrapped(|ui| {
                ui.add_space(kit::LABEL_W);
                ui.spacing_mut().item_spacing.x = 6.0;
                for (label, k) in [("0.5×", 0.5), ("1×", 1.0), ("2×", 2.0), ("4×", 4.0)] {
                    if kit::chip(ui, label, (v - k).abs() < 1e-3, None).clicked() {
                        pick = Some(k);
                    }
                }
            });
            if o.changed || pick.is_some() {
                let v = pick.unwrap_or(v);
                // Constant speed changes the clip's length; the model
                // ripples the spine.
                let seq = app.doc.seq();
                let edits = ids.iter().map(|id| m::Edit::SetSpeed { seq, clip: m::ClipId(*id as u64), speed: m::Speed::Constant(v as f64) }).collect();
                if pick.is_some() {
                    app.doc.apply("Speed", m::Edit::Batch { edits });
                } else {
                    app.doc.apply_gesture(&format!("speed-{ids:?}"), "Speed", m::Edit::Batch { edits });
                }
            }
            kit::labeled_row(ui, "Reverse", |ui| {
                let mut rev = c.extra.reverse;
                if kit::toggle(ui, &mut rev, "Reverse", None).changed() {
                    edit_all(app, ids, "Reverse", |c| c.extra.reverse = rev);
                }
            });
        });
        if out.reset {
            let seq = app.doc.seq();
            let edits = ids.iter().map(|id| m::Edit::SetSpeed { seq, clip: m::ClipId(*id as u64), speed: m::Speed::Constant(1.0) }).collect();
            app.doc.apply("Reset speed", m::Edit::Batch { edits });
            edit_all(app, ids, "Reset speed", |c| c.extra.reverse = false);
        }
    }

    if visual {
        Section::new("motion", "Motion").icon(Icon::Path).show(ui, |ui| {
            kit::labeled_row(ui, "Entry", |ui| {
                let entries: Vec<&catalogue::Item> = catalogue::motions().iter().filter(|m| m.group == 1).collect();
                let mut names: Vec<&str> = vec!["None"];
                names.extend(entries.iter().map(|m| m.name));
                let cur = c.motion_in.as_deref().and_then(|id| entries.iter().position(|m| m.id == id)).map_or(0, |i| i + 1);
                if let Some(i) = kit::menu_button(ui, "motion-in", cur, &names, ui.available_width() - 40.0, None) {
                    let v = (i > 0).then(|| entries[i - 1].id.to_owned());
                    edit_all(app, ids, "Entry motion", move |c| c.motion_in = v.clone());
                }
            });
            if c.motion_in.is_some() {
                let mut d = c.extra.motion_dur;
                let max = (c.dur as f32 / view::SEC as f32).max(0.1);
                let row = FieldRow::new("Duration", &mut d, 0.1..=max).default(0.5).unit("s").decimals(1).step(0.1).scrub(0.01).show(ui);
                if row.changed {
                    let ids2 = ids.to_vec();
                    app.doc.edit_gesture(&format!("motion-dur-{ids2:?}"), "Motion duration", |p| {
                        for id in &ids2 {
                            if let Some(c) = p.clip_mut(*id) {
                                c.extra.motion_dur = d;
                            }
                        }
                    });
                }
            }
        });
        let out = Section::new("mask", "Mask").icon(Icon::Selection).resettable(c.extra.mask != 0).show(ui, |ui| {
            let mut m = c.extra.mask as usize;
            let w = ui.available_width();
            if kit::segmented(ui, "mask-shape", &mut m, &[Segment::text("None"), Segment::icon(Icon::Rectangle, "Rectangle"), Segment::icon(Icon::Circle, "Ellipse"), Segment::icon(Icon::Path, "Brush")], Some(w), None).changed() {
                edit_all(app, ids, "Mask shape", |c| c.extra.mask = m as u8);
            }
            if c.extra.mask != 0 {
                prop(app, ui, ids, "mask.feather", "Feather", |c| c.extra.feather, |c, v| c.extra.feather = v, 0.0..=100.0, 0.0, "%", 0);
                kit::labeled_row(ui, "Invert", |ui| {
                    let mut inv = c.extra.mask_invert;
                    if kit::toggle(ui, &mut inv, "Invert mask", None).changed() {
                        edit_all(app, ids, "Invert mask", |c| c.extra.mask_invert = inv);
                    }
                });
            }
        });
        if out.reset {
            edit_all(app, ids, "Remove mask", |c| {
                c.extra.mask = 0;
                c.extra.feather = 0.0;
                c.extra.mask_invert = false;
            });
        }
        // Puppet pins, corner pin and the mask's pins and points.
        super::pins::inspector(app, ui, ids[0]);
        // Bones, poses, motions, parts and swap sets.
        super::bones::inspector(app, ui, ids[0]);
        super::modifiers::inspector(app, ui, ids[0]);
        super::physics::inspector(app, ui, ids);
        super::drivers::inspector(app, ui, ids[0]);
        // Template controls, shape layer items, text animators.
        super::mograph::inspector(app, ui, ids[0]);
        // Motion tracks, followers and stabilizing.
        #[cfg(feature = "render")]
        super::track::inspector(app, ui, ids[0]);
        // Background removal, object cut-outs and roto.
        #[cfg(feature = "render")]
        super::cutout::inspector(app, ui, ids[0]);
        // Shot matching.
        #[cfg(feature = "render")]
        super::match_colour::inspector(app, ui, ids[0]);
        // Shadows, light and pieces that sit a character in its backdrop.
        #[cfg(feature = "render")]
        super::in_scene::inspector(app, ui, ids[0]);
        // In-between frames for slowed footage.
        #[cfg(feature = "render")]
        super::slowmo::inspector(app, ui, ids[0]);
    }
    // Every other setting of the layer, and what the tools keep on the clip.
    if let [id] = ids {
        super::body3d::section(app, ui, *id);
        super::world_edit::section(app, ui, *id);
        super::sim_edit::section(app, ui, *id);
    }
    super::params::rest(app, ui, ids);
}

/// Volume in dB to the 0–200 % control: 0 % is silence, 100 % unchanged,
/// 200 % is +12 dB.
pub fn db_to_percent(db: f32) -> f32 {
    if db <= -96.0 {
        0.0
    } else if db <= 0.0 {
        100.0 * 10f32.powf(db / 20.0)
    } else {
        100.0 + db / 12.0 * 100.0
    }
}

pub fn percent_to_db(p: f32) -> f32 {
    if p <= 0.05 {
        -96.0
    } else if p <= 100.0 {
        20.0 * (p / 100.0).log10()
    } else {
        (p - 100.0) / 100.0 * 12.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_mapping_round_trips() {
        for p in [0.0f32, 25.0, 50.0, 100.0, 150.0, 200.0] {
            let back = db_to_percent(percent_to_db(p));
            assert!((back - p).abs() < 0.2, "{p} -> {back}");
        }
        assert_eq!(percent_to_db(100.0), 0.0);
        assert!((percent_to_db(200.0) - 12.0).abs() < 1e-4);
    }

    /// The section kinds a source file draws, read from its `Section::new("…"` calls.
    fn section_kinds(src: &str) -> Vec<String> {
        let pat = "Section::new(\"";
        let mut out = Vec::new();
        let mut rest = src;
        while let Some(at) = rest.find(pat) {
            rest = &rest[at + pat.len()..];
            if let Some(end) = rest.find('"') {
                out.push(rest[..end].to_owned());
            }
        }
        out
    }

    /// Every section a 3D model clip can draw (its own model panel, physics,
    /// character and simulation) sits in one of the model clip's tabs, so
    /// the tab filter never hides it for good.
    #[test]
    fn every_model_clip_section_is_in_a_tab() {
        let tabs = clip_tabs(ClipKind::Title, Some(m::GeneratorKind::Model));
        let shown: Vec<&str> = tabs.iter().flat_map(|t| t.1.iter().copied()).collect();
        let mut kinds: Vec<String> = Vec::new();
        kinds.extend(section_kinds(include_str!("model_edit/panel.rs")));
        kinds.extend(section_kinds(include_str!("body3d.rs")));
        // The model's own section and the Model arm of the 3D animation sections.
        kinds.extend(["model", "character3d", "sim3d"].map(String::from));
        assert!(kinds.len() >= 9, "found only {kinds:?}");
        for k in &kinds {
            assert!(shown.contains(&k.as_str()), "model clip section \"{k}\" is in no tab of a model clip, so it never shows");
        }
    }
}
