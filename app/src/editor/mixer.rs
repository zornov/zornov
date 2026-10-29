// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The Sound panel: the film mix in the timeline's place (the Faders
//! button in the timeline bar, or ⇧X).
//!
//! - Mixer: a channel strip per lane (role, bus, fader with its meter,
//!   pan, reverb send, mute and solo, and a surround pad in 5.1 / 7.1),
//!   a strip per bus, and the master with per-channel meters and live
//!   momentary, short-term and integrated loudness.
//! - Delivery: the loudness spec, measure, one-step normalise, stems and
//!   ADM export.
//! - Dialogue & ADR: level or match the selected clips, the cue list with
//!   recording over picture (a 3-2-1 streamer, the take synced, matched and
//!   laid on the ADR lane), the cue sheet.
//! - Subtitles & dubs: tracks per language from the dialogue, import and
//!   export, the timing check, translation, burn-in, character voices and
//!   dubbing.
//!
//! Everything goes through `sound_desk`, the same functions the AI's
//! tools use; slow work (renders, speech, translation) runs on a worker
//! thread and lands as one undo step when done.

use crate::app::StudioApp;
use crate::icons::Icon;
use crate::kit::{self, Button, IconButton, Segment};
use crate::sound_desk::{self as desk, Desk, LaneMix};
use crate::theme;
use eframe::egui::{self, Align2, CornerRadius, Rect, Sense, Stroke, Vec2};
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, channel};
use studio_model as m;
use studio_sound::loudness::LoudnessTarget;
use studio_sound::mixer::BusKind;
use studio_sound::surround::Layout;
use crate::kit::ShowNoted;

fn on_id() -> egui::Id {
    egui::Id::unique("sound-panel-on")
}

/// Whether the Sound panel shows in the timeline's place.
pub fn is_on(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp::<bool>(on_id())).unwrap_or(false)
}

/// Shows or hides the Sound panel (in the Timeline's place, or docked).
pub fn set_on(ctx: &egui::Context, on: bool) {
    ctx.data_mut(|d| d.insert_temp(on_id(), on));
}

/// Shows or hides the Sound panel.
pub fn toggle(ctx: &egui::Context) {
    let on = is_on(ctx);
    ctx.data_mut(|d| d.insert_temp(on_id(), !on));
}

/// Work finished on a worker thread, applied on the next frame.
enum Done {
    Measured(LoudnessTarget, desk::render::Reading),
    Normalised(LoudnessTarget, f32, desk::render::Reading),
    Stems(Result<Vec<desk::render::Written>, String>, Option<Result<(usize, usize), String>>),
    Track(Result<desk::SubTrack, String>),
    Dub(String, Result<(Vec<desk::dub::Made>, Vec<desk::dub::LineReport>), String>),
}

struct Job {
    label: String,
    rx: Receiver<Done>,
}

fn jobs() -> &'static Mutex<Vec<Job>> {
    static J: std::sync::OnceLock<Mutex<Vec<Job>>> = std::sync::OnceLock::new();
    J.get_or_init(|| Mutex::new(Vec::new()))
}

fn spawn(label: &str, f: impl FnOnce() -> Done + Send + 'static) {
    let (tx, rx) = channel();
    std::thread::Builder::new()
        .name("studio-sound-job".into())
        .spawn(move || {
            let _ = tx.send(f());
        })
        .ok();
    jobs().lock().unwrap_or_else(|e| e.into_inner()).push(Job { label: label.into(), rx });
}

fn busy() -> Vec<String> {
    jobs().lock().unwrap_or_else(|e| e.into_inner()).iter().map(|j| j.label.clone()).collect()
}

/// The tabs of the Sound panel, in order. Every export (loudness, stems, the ADR
/// cue sheet, subtitle files) lives under `DELIVER`; the other tabs link to it.
const MIXER: usize = 0;
const DIALOGUE: usize = 1;
const CAPTIONS: usize = 2;
const DELIVER: usize = 3;

/// Panel state kept between frames.
#[derive(Clone, Default)]
struct PanelState {
    tab: usize,
    target: usize,
    reading: Option<(LoudnessTarget, desk::render::Reading)>,
    separate_bg: bool,
    adm: bool,
    level_target: f32,
    sub_lang: String,
    translate_to: String,
    selected_track: Option<String>,
    issues: Vec<String>,
    message: Option<String>,
}

fn state(ctx: &egui::Context) -> PanelState {
    ctx.data(|d| d.get_temp::<PanelState>(egui::Id::unique("sound-panel-state"))).unwrap_or_else(|| PanelState { level_target: -24.0, translate_to: "es".into(), ..Default::default() })
}

fn save(ctx: &egui::Context, s: PanelState) {
    ctx.data_mut(|d| d.insert_temp(egui::Id::unique("sound-panel-state"), s));
}

fn home(app: &StudioApp) -> Option<std::path::PathBuf> {
    app.doc.folder.as_ref().map(|f| f.root().to_path_buf())
}

fn db(p: f32) -> f32 {
    if p <= 0.0 { -120.0 } else { 20.0 * p.log10() }
}

fn fmt_lufs(v: f64) -> String {
    if v.is_finite() { format!("{v:.1}") } else { "—".into() }
}

/// Applies finished jobs.
fn poll(app: &mut StudioApp, ctx: &egui::Context, s: &mut PanelState) {
    let mut done = Vec::new();
    {
        let mut j = jobs().lock().unwrap_or_else(|e| e.into_inner());
        j.retain(|job| match job.rx.try_recv() {
            Ok(d) => {
                done.push(d);
                false
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => true,
            Err(_) => false,
        });
    }
    for d in done {
        let msg = match d {
            Done::Measured(t, r) => {
                let (ok, why) = desk::render::verdict(t, &r);
                s.reading = Some((t, r));
                if ok { format!("{} passes", t.label()) } else { why.join("; ") }
            }
            Done::Normalised(t, gain, r) => match desk::render::store_normalised(&mut app.doc, t, gain, &r) {
                Ok(()) => {
                    let m = crate::i18n::tf(format!("Normalised to {}: {} LUFS, true peak {:.1} dBTP (master {gain:+.1} dB)", t.label(), fmt_lufs(r.lufs), r.report.true_peak_dbtp));
                    s.reading = Some((t, r));
                    m
                }
                Err(e) => e,
            },
            Done::Stems(r, adm) => match r {
                Ok(w) => {
                    let mut m = crate::i18n::tf(format!("Wrote {} stems in {}", w.len(), w.first().and_then(|x| x.path.parent()).map(|p| p.display().to_string()).unwrap_or_default()));
                    if let Some(a) = adm {
                        m.push_str(&match a {
                            Ok((b, o)) => format!("; ADM: {b}-channel bed, {o} objects"),
                            Err(e) => format!("; ADM failed: {e}"),
                        });
                    }
                    m
                }
                Err(e) => e,
            },
            Done::Track(r) => match r {
                Ok(t) => {
                    let n = t.cues.len();
                    let lang = t.lang.clone();
                    let r = desk::edit_desk(&mut app.doc, "Subtitles", |d| {
                        if d.subtitles.is_empty() {
                            d.original_language = lang.clone();
                        }
                        d.subtitles.retain(|x| !(x.lang == t.lang && x.kind == t.kind));
                        d.subtitles.push(t);
                        Ok(String::new())
                    });
                    s.selected_track = Some(lang.clone());
                    match r {
                        Ok(_) => format!("{n} subtitles ({lang})"),
                        Err(e) => e,
                    }
                }
                Err(e) => e,
            },
            Done::Dub(lang, r) => match r.and_then(|(made, reports)| desk::dub::lay_down(&mut app.doc, &lang, &made).map(|_| reports)) {
                Ok(reports) => {
                    let long = reports.iter().filter(|r| r.long_by.is_some()).count();
                    crate::i18n::tf(format!("Dubbed {} lines into {}{}", reports.len(), studio_text::subtitles::language_name(&lang), if long > 0 { format!("; {long} too long to fit: shorten them") } else { String::new() }))
                }
                Err(e) => e,
            },
        };
        s.message = Some(msg.clone());
        kit::push(ctx, msg, None, Some(Icon::Faders));
    }
}

/// Draws the panel in `area` (the timeline's place).
pub fn show(app: &mut StudioApp, ui: &mut egui::Ui, area: Rect) {
    let ctx = ui.ctx().clone();
    let t = theme::tokens(&ctx);
    let mut s = state(&ctx);
    poll(app, &ctx, &mut s);
    recording(app, &ctx);
    let inner = area.shrink(8.0);
    let bar = Rect::from_min_size(inner.min, Vec2::new(inner.width(), 30.0));
    let body = Rect::from_min_max(egui::pos2(inner.left(), bar.bottom() + 6.0), inner.max);
    let mut right = ui.new_child(egui::UiBuilder::new().max_rect(bar).layout(egui::Layout::right_to_left(egui::Align::Center)));
    if IconButton::new(Icon::X, "Back to the timeline").shortcut("⇧X").show(&mut right).clicked()
        || (!ctx.egui_wants_keyboard_input() && ctx.input_mut(|i| i.consume_shortcut(&egui::KeyboardShortcut::new(egui::Modifiers::SHIFT, egui::Key::X))))
    {
        toggle(&ctx);
    }
    let mut d = Desk::of(&app.doc.model);
    let mut layout = Layout::ALL.iter().position(|l| *l == d.layout()).unwrap_or(0);
    if kit::segmented(&mut right, "sound-layout", &mut layout, &[Segment::text("Stereo"), Segment::text("5.1"), Segment::text("7.1")], None, None).changed() {
        let l = Layout::ALL[layout];
        let _ = desk::edit_desk(&mut app.doc, "Mix layout", |d| {
            d.layout = l.id().into();
            Ok(String::new())
        });
        d = Desk::of(&app.doc.model);
    }
    // Left: the title, the tabs and what is running, in the room the right
    // side leaves; a narrow panel scrolls them sideways.
    let left = Rect::from_min_max(bar.min, egui::pos2((right.min_rect().left() - 8.0).max(bar.left()), bar.bottom()));
    let mut top = ui.new_child(egui::UiBuilder::new().max_rect(left).layout(egui::Layout::left_to_right(egui::Align::Center)));
    top.set_clip_rect(left.intersect(ui.clip_rect()));
    kit::scroll_strip(&mut top, "sound-top", |top| {
        top.spacing_mut().item_spacing.x = 6.0;
        top.label(egui::RichText::new(crate::i18n::shown("Sound")).font(theme::body_medium()).color(t.text));
        kit::tab_bar(top, "sound-tabs", &mut s.tab, &["Mixer", "Dialogue", "Captions", "Deliver"]);
        let jobs_now = busy();
        if !jobs_now.is_empty() {
            top.add(egui::Spinner::new());
            top.label(egui::RichText::new(jobs_now.join(", ")).font(theme::caption()).color(t.text_2));
        }
    });
    let mut b = ui.new_child(egui::UiBuilder::new().max_rect(body).layout(egui::Layout::top_down(egui::Align::Min)));
    b.set_clip_rect(body.intersect(ui.clip_rect()));
    // The mixer's strips scroll sideways; the other tabs scroll down when
    // the panel is shorter than they are.
    match s.tab {
        MIXER => mixer_tab(app, &mut b, &d, &t),
        DIALOGUE => kit::scroll_panel(&mut b, "sound-dialogue", |b| dialogue_tab(app, b, &d, &mut s, &t)),
        CAPTIONS => kit::scroll_panel(&mut b, "sound-captions", |b| subtitles_tab(app, b, &d, &mut s, &t)),
        _ => kit::scroll_panel(&mut b, "sound-delivery", |b| delivery_tab(app, b, &d, &mut s, &t)),
    }
    save(&ctx, s);
    if !busy().is_empty() || app.editor.playing {
        ctx.request_repaint_after(std::time::Duration::from_millis(50));
    }
}

const STRIP_W: f32 = 92.0;

/// A vertical fader with a meter beside it; returns the new dB when dragged.
fn fader(ui: &mut egui::Ui, id: egui::Id, rect: Rect, value_db: f32, peak: f32, t: &theme::Tokens) -> Option<f32> {
    // Fader travel: +12 dB at the top to −60 at the bottom, 0 dB at 20 %.
    let to_y = |v: f32| {
        let v = v.clamp(-60.0, 12.0);
        let k = if v >= 0.0 { 0.2 * (1.0 - v / 12.0) } else { 0.2 + 0.8 * (-v / 60.0).sqrt() };
        rect.top() + k * rect.height()
    };
    let from_y = |y: f32| {
        let k = ((y - rect.top()) / rect.height()).clamp(0.0, 1.0);
        if k <= 0.2 { 12.0 * (1.0 - k / 0.2) } else { -60.0 * ((k - 0.2) / 0.8).powi(2) }
    };
    let p = ui.painter();
    let track = Rect::from_center_size(egui::pos2(rect.left() + 10.0, rect.center().y), Vec2::new(4.0, rect.height()));
    p.rect_filled(track, 2, t.sunken);
    p.hline(track.left() - 4.0..=track.right() + 4.0, to_y(0.0), Stroke::new(1.0, t.line));
    // Meter.
    let meter = Rect::from_min_max(egui::pos2(rect.left() + 24.0, rect.top()), egui::pos2(rect.left() + 30.0, rect.bottom()));
    p.rect_filled(meter, 1, t.sunken);
    let lvl = db(peak);
    if lvl > -60.0 {
        let y = to_y(lvl);
        let col = if lvl > -1.0 { t.danger } else if lvl > -9.0 { t.warn } else { t.accent };
        p.rect_filled(Rect::from_min_max(egui::pos2(meter.left(), y), meter.max), 1, col);
    }
    let knob = Rect::from_center_size(egui::pos2(track.center().x, to_y(value_db)), Vec2::new(18.0, 10.0));
    let resp = ui.interact(rect.with_max_x(rect.left() + 22.0), id, Sense::click_and_drag());
    p.rect_filled(knob, 3, if resp.hovered() || resp.dragged() { t.text } else { t.text_2 });
    if resp.double_clicked() {
        return Some(0.0);
    }
    if resp.dragged()
        && let Some(pos) = resp.interact_pointer_pos()
    {
        return Some((from_y(pos.y) * 10.0).round() / 10.0);
    }
    None
}

fn set_lane_gesture(app: &mut StudioApp, key: &str, lane: u64, f: impl FnOnce(&mut LaneMix, &mut m::Lane)) {
    let Some(mut l) = desk::model_lane(&app.doc, lane) else { return };
    let mut mix = LaneMix::of(&l);
    f(&mut mix, &mut l);
    mix.store(&mut l);
    let seq = app.doc.model.active_sequence;
    app.doc.apply_gesture(key, "Mix", m::Edit::SetLane { seq, lane: l });
}

fn set_desk_gesture(app: &mut StudioApp, key: &str, f: impl FnOnce(&mut Desk)) {
    let mut d = Desk::of(&app.doc.model);
    f(&mut d);
    let settings = d.stored(&app.doc.model.settings);
    if settings != app.doc.model.settings {
        app.doc.apply_gesture(key, "Mix", m::Edit::SetSettings { settings: Box::new(settings) });
    }
}

fn mixer_tab(app: &mut StudioApp, ui: &mut egui::Ui, d: &Desk, t: &theme::Tokens) {
    let meters = crate::sound_bridge::live_meters().unwrap_or_default();
    let Some(seq) = app.doc.model.sequence(app.doc.model.active_sequence).cloned() else { return };
    if d.buses.is_empty() {
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new(crate::i18n::shown("Lanes play straight into the master. Film mixes group them in buses.")).font(theme::caption()).color(t.text_2));
            if Button::secondary("Add film buses").icon(Icon::Plus).compact().show(ui).clicked() {
                let _ = desk::edit_desk(&mut app.doc, "Film buses", |d| {
                    d.add_film_buses();
                    Ok(String::new())
                });
            }
        });
    }
    let aux = d.buses.iter().find(|b| b.kind() == BusKind::Aux).map(|b| b.id);
    let surround = d.layout() != Layout::Stereo;
    // Strips keep their size: a narrow panel scrolls them sideways, a short
    // one down.
    let view_h = ui.available_height().max(180.0);
    let strips = egui::ScrollArea::both().id_salt("mixer-strips").show_h(ui, |ui| {
        ui.set_max_height(view_h);
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            for (i, lane) in seq.lanes.iter().enumerate() {
                let peak = meters.lane_ids.iter().position(|x| *x == lane.id.0).and_then(|k| meters.lane_peaks.get(k)).copied().unwrap_or(0.0);
                lane_strip(app, ui, lane, d, aux, surround, peak, i, t);
            }
            ui.separator();
            for b in &d.buses {
                let peak = meters.bus_ids.iter().position(|x| *x == b.id).and_then(|k| meters.bus_peaks.get(k)).copied().unwrap_or(0.0);
                bus_strip(app, ui, b, peak, t);
            }
            ui.separator();
            master_strip(app, ui, d, &meters, t);
        });
    });
    kit::note_scroll(ui.ctx(), strips.inner_rect, false);
}

#[allow(clippy::too_many_arguments)]
fn lane_strip(app: &mut StudioApp, ui: &mut egui::Ui, lane: &m::Lane, d: &Desk, aux: Option<u64>, surround: bool, peak: f32, index: usize, t: &theme::Tokens) {
    let mix = LaneMix::of(lane);
    let role = desk::lane_role(lane);
    let h = ui.available_height().max(180.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(STRIP_W, h), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(6), t.raised);
    let mut c = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink(5.0)).layout(egui::Layout::top_down(egui::Align::Min)));
    c.spacing_mut().item_spacing.y = 3.0;
    let name = if lane.name.is_empty() { crate::i18n::tf(format!("Lane {}", index + 1)) } else { lane.name.clone() };
    kit::label_trunc(&mut c, egui::RichText::new(name).font(theme::caption()).color(t.text).strong());
    // Role: click to cycle.
    let roles = ["dialogue", "music", "effects", "ambience"];
    let rid = desk::role_id(role);
    let tag = match rid {
        "dialogue" => "DX",
        "music" => "MX",
        "ambience" => "BG",
        _ => "FX",
    };
    let mut extra = String::new();
    if let Some(l) = &mix.dub {
        extra = format!(" · dub {l}");
    } else if mix.adr {
        extra = " · ADR".into();
    }
    if kit::chip(&mut c, &format!("{tag}{extra}"), false, None).on_hover_text(crate::i18n::shown("Role: click for the next (dialogue, music, effects, ambience)")).clicked() {
        let next = roles[(roles.iter().position(|r| *r == rid).unwrap_or(0) + 1) % roles.len()];
        let _ = desk::edit_lane(&mut app.doc, lane.id.0, "Track role", |mix, _| {
            mix.role = Some(next.into());
            Ok(String::new())
        });
    }
    // Bus.
    if !d.buses.is_empty() {
        let current = mix.bus.and_then(|b| d.bus(b)).map(|b| b.name.clone()).unwrap_or_else(|| d.buses.iter().find(|b| b.kind() == role.bus_kind()).map(|b| format!("{} (auto)", b.name)).unwrap_or_else(|| "Master".into()));
        egui::ComboBox::from_id_salt(("lane-bus", lane.id.0)).selected_text(egui::RichText::new(current).font(theme::caption())).width(STRIP_W - 14.0).show_ui(&mut c, |ui| {
            crate::kit::note_list(ui);
            if ui.selectable_label(mix.bus.is_none(), "By role").clicked() {
                let _ = desk::edit_lane(&mut app.doc, lane.id.0, "Track bus", |mix, _| {
                    mix.bus = None;
                    Ok(String::new())
                });
            }
            for b in d.buses.iter().filter(|b| b.kind() != BusKind::Aux) {
                if ui.selectable_label(mix.bus == Some(b.id), &b.name).clicked() {
                    let id = b.id;
                    let _ = desk::edit_lane(&mut app.doc, lane.id.0, "Track bus", |mix, _| {
                        mix.bus = Some(id);
                        Ok(String::new())
                    });
                }
            }
        });
    }
    // Send to the reverb return.
    if let Some(aux) = aux {
        let mut lvl = mix.sends.iter().find(|s| s.bus == aux).map(|s| s.level_db).unwrap_or(-90.0);
        let r = kit::Slider::new("verb", &mut lvl, -90.0..=6.0).default(-90.0).unit("dB").with_label().with_value().show(&mut c).on_hover_text(crate::i18n::shown("Send to the reverb return"));
        if r.changed() {
            set_lane_gesture(app, &format!("send-{}", lane.id.0), lane.id.0, |mix, _| {
                mix.sends.retain(|s| s.bus != aux);
                if lvl > -89.0 {
                    mix.sends.push(desk::Send { bus: aux, level_db: lvl, keys: Vec::new(), pre: false });
                }
            });
        }
    }
    // Pan or the surround pad.
    if surround {
        let pad = c.allocate_exact_size(Vec2::splat(STRIP_W - 14.0), Sense::click_and_drag());
        let (r, resp) = pad;
        let p = c.painter();
        p.rect_filled(r, 4, t.sunken);
        p.vline(r.center().x, r.y_range(), Stroke::new(1.0, t.line));
        p.hline(r.x_range(), r.center().y, Stroke::new(1.0, t.line));
        let spec = desk::mix::lane_spec(lane, false);
        let pos = spec.surround.at(app.editor.playhead);
        let dot = egui::pos2(r.center().x + pos.x * r.width() * 0.5, r.center().y - pos.y * r.height() * 0.5);
        p.circle_filled(dot, 5.0, t.accent);
        p.text(r.center_top() + Vec2::new(0.0, 2.0), Align2::CENTER_TOP, crate::i18n::shown("front"), theme::caption(), t.text_2);
        if resp.dragged()
            && let Some(at) = resp.interact_pointer_pos()
        {
            let x = ((at.x - r.center().x) / (r.width() * 0.5)).clamp(-1.0, 1.0);
            let y = (-(at.y - r.center().y) / (r.height() * 0.5)).clamp(-1.0, 1.0);
            set_lane_gesture(app, &format!("pad-{}", lane.id.0), lane.id.0, |mix, _| {
                mix.surround.x = (x * 100.0).round() / 100.0;
                mix.surround.y = (y * 100.0).round() / 100.0;
                mix.surround.x_keys.clear();
                mix.surround.y_keys.clear();
                mix.surround.placed = true;
            });
        }
        resp.on_hover_text(crate::i18n::shown("Where the lane sits in the room: drag (front is up)"));
    } else {
        let mut pan = lane.pan as f32;
        // The slider and its word fit the strip.
        let word = kit::text_width(&c, "pan", egui::TextStyle::Body.resolve(c.style())) + c.spacing().item_spacing.x;
        let _ = word;
        if kit::Slider::new("pan", &mut pan, -1.0..=1.0).default(0.0).decimals(2).with_label().show(&mut c).changed() {
            set_lane_gesture(app, &format!("pan-{}", lane.id.0), lane.id.0, |mix, l| {
                l.pan = pan as f64;
                mix.pan_keys.clear();
            });
        }
    }
    // Mute, solo.
    c.horizontal(|ui| {
        let mut mute = lane.muted;
        let mut solo = lane.solo;
        if kit::chip(ui, "M", mute, None).on_hover_text(crate::i18n::shown("Mute")).clicked() {
            mute = !mute;
            let _ = desk::edit_lane(&mut app.doc, lane.id.0, "Mute track", |_, l| {
                l.muted = mute;
                Ok(String::new())
            });
        }
        if kit::chip(ui, "S", solo, None).on_hover_text(crate::i18n::shown("Solo")).clicked() {
            solo = !solo;
            let _ = desk::edit_lane(&mut app.doc, lane.id.0, "Solo track", |_, l| {
                l.solo = solo;
                Ok(String::new())
            });
        }
    });
    // Fader.
    let auto = !mix.volume_keys.is_empty();
    let shown = if auto { desk::mix::curve(&mix.volume_keys, lane.volume_db as f32).eval(app.editor.playhead) } else { lane.volume_db as f32 };
    let fr = Rect::from_min_max(egui::pos2(c.cursor().left(), c.cursor().top() + 4.0), egui::pos2(c.cursor().left() + 34.0, rect.bottom() - 22.0));
    if fr.height() > 30.0 {
        if let Some(v) = fader(&mut c, egui::Id::unique(("lane-fader", lane.id.0)), fr, shown, peak, t) {
            set_lane_gesture(app, &format!("fader-{}", lane.id.0), lane.id.0, |mix, l| {
                l.volume_db = v as f64;
                mix.volume_keys.clear();
            });
        }
        c.painter().text(egui::pos2(rect.center().x, rect.bottom() - 6.0), Align2::CENTER_BOTTOM, format!("{shown:+.1} dB{}", if auto { " A" } else { "" }), theme::caption(), t.text_2);
    }
}

fn bus_strip(app: &mut StudioApp, ui: &mut egui::Ui, b: &desk::Bus, peak: f32, t: &theme::Tokens) {
    let h = ui.available_height().max(180.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(STRIP_W, h), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(6), theme::alpha(t.accent, 0.08));
    let mut c = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink(5.0)).layout(egui::Layout::top_down(egui::Align::Min)));
    kit::label_trunc(&mut c, egui::RichText::new(&b.name).font(theme::caption()).color(t.text).strong());
    c.label(egui::RichText::new(format!("{} bus{}", b.kind().stem(), if b.effects.is_empty() { String::new() } else { format!(" · {}", b.effects.iter().map(|e| e.kind.as_str()).collect::<Vec<_>>().join(", ")) })).font(theme::caption()).color(t.text_2));
    let id = b.id;
    c.horizontal(|ui| {
        if kit::chip(ui, "M", b.muted, None).on_hover_text(crate::i18n::shown("Mute")).clicked() {
            let v = !b.muted;
            let _ = desk::edit_desk(&mut app.doc, "Mute bus", |d| {
                if let Some(x) = d.buses.iter_mut().find(|x| x.id == id) {
                    x.muted = v;
                }
                Ok(String::new())
            });
        }
        if kit::chip(ui, "S", b.solo, None).on_hover_text(crate::i18n::shown("Solo")).clicked() {
            let v = !b.solo;
            let _ = desk::edit_desk(&mut app.doc, "Solo bus", |d| {
                if let Some(x) = d.buses.iter_mut().find(|x| x.id == id) {
                    x.solo = v;
                }
                Ok(String::new())
            });
        }
    });
    let fr = Rect::from_min_max(egui::pos2(c.cursor().left(), c.cursor().top() + 4.0), egui::pos2(c.cursor().left() + 34.0, rect.bottom() - 22.0));
    if fr.height() > 30.0 {
        if let Some(v) = fader(&mut c, egui::Id::unique(("bus-fader", id)), fr, b.volume_db, peak, t) {
            set_desk_gesture(app, &format!("bus-{id}"), |d| {
                if let Some(x) = d.buses.iter_mut().find(|x| x.id == id) {
                    x.volume_db = v;
                    x.keys.clear();
                }
            });
        }
        c.painter().text(egui::pos2(rect.center().x, rect.bottom() - 6.0), Align2::CENTER_BOTTOM, format!("{:+.1} dB", b.volume_db), theme::caption(), t.text_2);
    }
}

fn master_strip(app: &mut StudioApp, ui: &mut egui::Ui, d: &Desk, meters: &studio_sound::Meters, t: &theme::Tokens) {
    let layout = d.layout();
    let w = 120.0 + 10.0 * layout.channels() as f32;
    let h = ui.available_height().max(180.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, h), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(6), t.raised);
    let mut c = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink(5.0)).layout(egui::Layout::top_down(egui::Align::Min)));
    c.label(egui::RichText::new(crate::i18n::shown("Master")).font(theme::caption()).color(t.text).strong());
    let gain = d.delivery.as_ref().map_or(0.0, |x| x.gain_db);
    let lufs = |v: f64| fmt_lufs(v);
    c.label(egui::RichText::new(crate::i18n::tf(format!("M {}  S {}\nI {}  TP {}", lufs(meters.momentary_lufs), lufs(meters.short_term_lufs), lufs(meters.integrated_lufs), lufs(meters.true_peak_dbtp)))).font(theme::caption()).color(t.text_2)).on_hover_text(crate::i18n::shown("Momentary, short-term and integrated loudness (LUFS) and true peak (dBTP) since playback started"));
    if gain != 0.0 {
        c.label(egui::RichText::new(format!("delivery {gain:+.1} dB")).font(theme::caption()).color(t.text_2));
    }
    let fr = Rect::from_min_max(egui::pos2(c.cursor().left(), c.cursor().top() + 4.0), egui::pos2(c.cursor().left() + 34.0, rect.bottom() - 22.0));
    if fr.height() > 30.0 {
        if let Some(v) = fader(&mut c, egui::Id::unique("master-fader"), fr, d.master.volume_db, meters.master_peak, t) {
            set_desk_gesture(app, "master", |d| d.master.volume_db = v);
        }
        // A meter per channel.
        let p = c.painter();
        for (i, name) in layout.names().iter().enumerate() {
            let x = fr.right() + 8.0 + i as f32 * 10.0;
            let m = Rect::from_min_max(egui::pos2(x, fr.top()), egui::pos2(x + 6.0, fr.bottom()));
            p.rect_filled(m, 1, t.sunken);
            let lvl = db(meters.channel_peaks.get(i).copied().unwrap_or(0.0));
            if lvl > -60.0 {
                let k = 1.0 - (lvl + 60.0) / 60.0;
                let col = if lvl > -1.0 { t.danger } else if lvl > -9.0 { t.warn } else { t.accent };
                p.rect_filled(Rect::from_min_max(egui::pos2(m.left(), m.top() + k.clamp(0.0, 1.0) * m.height()), m.max), 1, col);
            }
            p.text(egui::pos2(m.center().x, fr.bottom() + 2.0), Align2::CENTER_TOP, *name, egui::FontId::proportional(8.0), t.text_2);
        }
        c.painter().text(egui::pos2(rect.left() + 22.0, rect.bottom() - 6.0), Align2::CENTER_BOTTOM, format!("{:+.1}", d.master.volume_db), theme::caption(), t.text_2);
    }
}

fn delivery_tab(app: &mut StudioApp, ui: &mut egui::Ui, d: &Desk, s: &mut PanelState, t: &theme::Tokens) {
    if let Some(x) = d.delivery.as_ref().and_then(|x| LoudnessTarget::from_id(&x.target)) {
        if s.reading.is_none() {
            s.target = LoudnessTarget::ALL.iter().position(|y| *y == x).unwrap_or(0);
        }
    }
    ui.horizontal_wrapped(|ui| {
        ui.label(egui::RichText::new(crate::i18n::shown("Spec")).font(theme::caption()).color(t.text_2));
        let target = LoudnessTarget::ALL[s.target.min(LoudnessTarget::ALL.len() - 1)];
        egui::ComboBox::from_id_salt("loudness-target").selected_text(target.label()).show_ui(ui, |ui| {
            crate::kit::note_list(ui);
            for (i, x) in LoudnessTarget::ALL.iter().enumerate() {
                ui.selectable_value(&mut s.target, i, x.label());
            }
        });
        let target = LoudnessTarget::ALL[s.target.min(LoudnessTarget::ALL.len() - 1)];
        let free = busy().is_empty();
        if Button::secondary("Measure").icon(Icon::Gauge).compact().show(ui).clicked() && free {
            let model = app.doc.model.clone();
            let layout = d.layout();
            spawn("Measuring", move || Done::Measured(target, desk::render::measure(&model, target, layout)));
        }
        if Button::primary("Normalise").icon(Icon::MagicWand).compact().show(ui).clicked() && free {
            let model = app.doc.model.clone();
            spawn("Normalising", move || match desk::render::normalise_gain(&model, target) {
                Ok((g, r)) => Done::Normalised(target, g, r),
                Err(e) => Done::Stems(Err(e), None),
            });
        }
    });
    if let Some((target, r)) = &s.reading {
        let (ok, why) = desk::render::verdict(*target, r);
        let col = if ok { t.accent } else { t.warn };
        ui.label(egui::RichText::new(if ok { crate::i18n::tf(format!("Passes {}", target.label())) } else { why.join("; ") }).color(col));
        egui::Grid::new("loudness-grid").num_columns(2).spacing([16.0, 4.0]).show(ui, |ui| {
            let row = |ui: &mut egui::Ui, k: &str, v: String| {
                ui.label(egui::RichText::new(k).font(theme::caption()).color(t.text_2));
                ui.label(egui::RichText::new(v).font(theme::caption()).color(t.text));
                ui.end_row();
            };
            row(ui, "Integrated", format!("{} LUFS", fmt_lufs(r.report.integrated_lufs)));
            if r.dialogue_gated {
                row(ui, "Dialogue-gated", format!("{} LUFS ({:.0}% speech)", fmt_lufs(r.lufs), r.speech_share * 100.0));
            }
            row(ui, "Short-term max", format!("{} LUFS", fmt_lufs(r.report.short_term_max_lufs)));
            row(ui, "Momentary max", format!("{} LUFS", fmt_lufs(r.report.momentary_max_lufs)));
            row(ui, "Loudness range", format!("{:.1} LU", r.report.range_lu));
            row(ui, "True peak", format!("{:.1} dBTP", r.report.true_peak_dbtp));
        });
    }
    ui.separator();
    ui.horizontal_wrapped(|ui| {
        ui.label(egui::RichText::new(crate::i18n::tf(format!("Stems ({}): DX, MX, FX, M&E, mix{}", d.layout().id(), if d.languages.is_empty() { "" } else { ", and each dub" }))).font(theme::caption()).color(t.text_2));
        ui.checkbox(&mut s.separate_bg, "BG apart");
        ui.checkbox(&mut s.adm, "ADM objects");
        if Button::secondary("Export stems…").icon(Icon::Export).compact().show(ui).clicked() && busy().is_empty() {
            if let Some(dir) = rfd::FileDialog::new().set_title("Folder for the stems").pick_folder() {
                let model = app.doc.model.clone();
                let (layout, bg, adm) = (d.layout(), s.separate_bg, s.adm);
                let title = app.doc.project.name.clone();
                spawn("Writing stems", move || {
                    let w = desk::render::export_stems(&model, &dir, layout, bg, &title);
                    let a = adm.then(|| desk::render::export_adm(&model, &dir.join(format!("{} ADM.wav", if title.is_empty() { "Zornov" } else { &title })), layout, &title));
                    Done::Stems(w, a)
                });
            }
        }
    });
    ui.separator();
    ui.horizontal_wrapped(|ui| {
        ui.label(egui::RichText::new(crate::i18n::tf(format!("ADR cue sheet ({} cues)", d.adr.len()))).font(theme::caption()).color(t.text_2));
        if Button::secondary("Export cue sheet…").icon(Icon::Export).compact().show(ui).clicked()
            && let Some(p) = rfd::FileDialog::new().set_file_name("ADR cue sheet.csv").save_file()
        {
            let _ = std::fs::write(&p, desk::adr::cue_sheet(d, app.doc.project.fps, &app.doc.project.name));
        }
    });
    ui.horizontal_wrapped(|ui| {
        let selected = s.selected_track.clone().or_else(|| d.subtitles.first().map(|t| t.lang.clone()));
        let track = selected.as_deref().and_then(|l| d.subtitles_of(l)).cloned();
        let name = track.as_ref().map(|t| t.name.clone()).unwrap_or_else(|| crate::i18n::shown("no subtitles yet").into_owned());
        ui.label(egui::RichText::new(crate::i18n::tf(format!("Subtitles: {name}"))).font(theme::caption()).color(t.text_2));
        let enabled = track.is_some();
        ui.add_enabled_ui(enabled, |ui| {
            if Button::secondary("Export subtitles…").icon(Icon::Export).compact().show(ui).on_hover_text(crate::i18n::shown("Writes the track chosen in Captions as SRT or VTT")).clicked()
                && let Some(tr) = &track
                && let Some(p) = rfd::FileDialog::new().add_filter("SubRip", &["srt"]).add_filter("WebVTT", &["vtt"]).set_file_name(format!("{}.srt", tr.lang)).save_file()
            {
                let vtt = p.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("vtt"));
                let _ = std::fs::write(&p, desk::subs::write(tr, vtt));
            }
        });
    });
    if let Some(m) = &s.message {
        ui.label(egui::RichText::new(m).font(theme::caption()).color(t.text_2));
    }
}

/// An ADR recording in progress.
struct Recording {
    cue: u32,
    stop_at: i64,
    input: studio_audio::AudioIn,
    rec: studio_sound::recorder::VoiceOverRecorder,
}

thread_local! {
    /// The recording, kept on the UI thread (the input device's handle
    /// belongs to the thread that opened it).
    static RECORDING: std::cell::RefCell<Option<Recording>> = const { std::cell::RefCell::new(None) };
}

fn is_recording() -> bool {
    RECORDING.with(|r| r.borrow().is_some())
}

fn start_recording(app: &mut StudioApp, cue: &desk::AdrCue) -> Result<(), String> {
    let input = studio_audio::AudioIn::open(None)?;
    let rate = studio_audio::RATE;
    let pre = 4_000_000i64;
    let mut rec = studio_sound::recorder::VoiceOverRecorder::new(rate, studio_sound::recorder::RecorderParams { count_in_beats: 0, start: (cue.start - pre).max(0), trim_silence_below_db: None, ..Default::default() });
    rec.start();
    app.editor.playhead = (cue.start - pre).max(0);
    app.editor.playing = true;
    RECORDING.with(|r| *r.borrow_mut() = Some(Recording { cue: cue.id, stop_at: cue.end + 1_000_000, input, rec }));
    Ok(())
}

/// Feeds the microphone while recording; at the cue's end, brings the take in.
fn recording(app: &mut StudioApp, ctx: &egui::Context) {
    let finished = RECORDING.with(|slot| {
        let mut slot = slot.borrow_mut();
        let r = slot.as_mut()?;
        studio_sound::recorder::DeviceFeed::new(&r.input).pump(&mut r.rec);
        if app.editor.playing && app.editor.playhead < r.stop_at {
            return None;
        }
        slot.take()
    });
    if is_recording() {
        ctx.request_repaint();
    }
    let Some(r) = finished else { return };
    let mut rec = r.rec;
    app.editor.playing = false;
    let Some(take) = rec.stop() else { return };
    let path = studio_paths::cache().join(format!("adr-take-{}.wav", r.cue));
    let wav = studio_media::wav::Wav { sample_rate: take.rate, channels: 1, samples: take.samples };
    if std::fs::write(&path, studio_media::wav::encode_pcm16(&wav)).is_err() {
        kit::push(ctx, "The take could not be written", None, Some(Icon::Warning));
        return;
    }
    let home = home(app);
    let msg = match desk::adr::take(&mut app.doc, home.as_deref(), r.cue, &path, false, true, true) {
        Ok(t) => crate::i18n::tf(format!("Take of cue {} in at {:.2}s (synced {:+.0} ms)", r.cue, t.start, t.shift * 1000.0)),
        Err(e) => e,
    };
    kit::push(ctx, msg, None, Some(Icon::Microphone));
}

/// The streamer during recording: 3, 2, 1 before the line, then the line.
fn streamer(app: &StudioApp, ui: &mut egui::Ui, d: &Desk, t: &theme::Tokens) {
    let Some(cue_id) = RECORDING.with(|r| r.borrow().as_ref().map(|r| r.cue)) else { return };
    let Some(cue) = d.adr.iter().find(|c| c.id == cue_id) else { return };
    let left = (cue.start - app.editor.playhead) as f64 / 1e6;
    let text = if left > 3.0 { "●  get ready".to_owned() } else if left > 0.0 { format!("●  {}", left.ceil() as i64) } else { format!("●  {}", cue.text) };
    ui.label(egui::RichText::new(text).font(egui::FontId::proportional(22.0)).color(t.danger));
}

fn dialogue_tab(app: &mut StudioApp, ui: &mut egui::Ui, d: &Desk, s: &mut PanelState, t: &theme::Tokens) {
    streamer(app, ui, d, t);
    let sel: Vec<u64> = app.editor.selection.iter().map(|x| *x as u64).collect();
    ui.horizontal_wrapped(|ui| {
        ui.label(egui::RichText::new(format!("{} selected", sel.len())).font(theme::caption()).color(t.text_2));
        kit::Slider::new("Target level", &mut s.level_target, -36.0..=-10.0).default(-23.0).unit("LUFS").with_value().width(160.0).show(ui);
        if Button::secondary("Level dialogue").compact().show(ui).clicked() {
            app.doc.begin_batch("Level dialogue");
            let mut out = Vec::new();
            for c in &sel {
                match desk::dialogue::level(&mut app.doc, *c, studio_sound::dialogue::LevelerParams { target_lufs: s.level_target, ..Default::default() }) {
                    Ok(p) => out.push(format!("clip {c}: {} phrases", p.len())),
                    Err(e) => out.push(e),
                }
            }
            app.doc.commit_batch();
            s.message = Some(out.join("; "));
        }
        if Button::secondary("Match to first selected").compact().show(ui).on_hover_text(crate::i18n::shown("Make the other selected clips sound like the first (EQ, level, room)")).clicked() && sel.len() >= 2 {
            let mut out = Vec::new();
            for c in &sel[1..] {
                match desk::dialogue::match_clip(&mut app.doc, *c, sel[0]) {
                    Ok(v) => out.push(format!("clip {c}: {:+.1} dB, EQ {:?}", v.gain_db, v.eq_db.map(|x| (x * 10.0).round() / 10.0))),
                    Err(e) => out.push(e),
                }
            }
            s.message = Some(out.join("; "));
        }
    });
    ui.separator();
    ui.horizontal_wrapped(|ui| {
        ui.label(egui::RichText::new(crate::i18n::shown("ADR cues")).font(theme::body_medium()).color(t.text));
        if Button::secondary("Add at playhead").icon(Icon::Plus).compact().show(ui).clicked() {
            let at = app.editor.playhead;
            let _ = desk::edit_desk(&mut app.doc, "ADR cue", |d| {
                desk::adr::add_cue(d, desk::AdrCue { start: at, end: at + 3_000_000, ..Default::default() });
                Ok(String::new())
            });
        }
        if Button::secondary("From subtitles").compact().show(ui).on_hover_text(crate::i18n::shown("A cue for every subtitle line of the original language")).clicked() {
            let cues = desk::adr::cues_from_subtitles(d, &d.original_language, 0.0, f32::MAX, &[], "");
            let _ = desk::edit_desk(&mut app.doc, "ADR cues", |d| {
                for c in cues {
                    desk::adr::add_cue(d, c);
                }
                Ok(String::new())
            });
        }
        if Button::ghost("Export cue sheet…").compact().show(ui).on_hover_text(crate::i18n::shown("Cue sheets are exported from Deliver")).clicked() {
            s.tab = DELIVER;
        }
    });
    let fps = app.doc.project.fps;
    egui::ScrollArea::vertical().id_salt("adr-cues").show_v(ui, |ui| {
        egui::Grid::new("adr-grid").num_columns(7).spacing([8.0, 4.0]).striped(true).show(ui, |ui| {
            for c in &d.adr {
                ui.label(egui::RichText::new(c.id.to_string()).font(theme::caption()));
                let mut who = c.character.clone();
                let mut line = c.text.clone();
                let a = ui.add(egui::TextEdit::singleline(&mut who).desired_width(70.0).hint_text("character"));
                ui.label(egui::RichText::new(format!("{} – {}", desk::adr::timecode(c.start, fps), desk::adr::timecode(c.end, fps))).font(theme::caption()).color(t.text_2));
                let b = ui.add(egui::TextEdit::singleline(&mut line).desired_width(220.0).hint_text("the line"));
                if (a.lost_focus() && who != c.character) || (b.lost_focus() && line != c.text) {
                    let id = c.id;
                    let _ = desk::edit_desk(&mut app.doc, "ADR cue", |d| {
                        if let Some(x) = d.adr.iter_mut().find(|x| x.id == id) {
                            x.character = who;
                            x.text = line;
                        }
                        Ok(String::new())
                    });
                }
                ui.label(egui::RichText::new(&c.status).font(theme::caption()).color(t.text_2));
                ui.horizontal_wrapped(|ui| {
                    let recording = is_recording();
                    if ui.add_enabled_ui(!recording, |ui| Button::secondary("Record").icon(Icon::Microphone).compact().show(ui)).inner.on_hover_text(crate::i18n::shown("Plays from 4 s before the line with a 3-2-1 streamer and records the microphone")).clicked() {
                        if let Err(e) = start_recording(app, c) {
                            s.message = Some(e);
                        }
                    }
                    if Button::secondary("Take…").compact().show(ui).on_hover_text(crate::i18n::shown("Bring in a recorded take: synced, matched, on the ADR lane")).clicked()
                        && let Some(p) = rfd::FileDialog::new().add_filter("Sound", &["wav", "aif", "aiff", "m4a", "mp3", "flac"]).pick_file()
                    {
                        let home = home(app);
                        s.message = Some(match desk::adr::take(&mut app.doc, home.as_deref(), c.id, &p, false, true, true) {
                            Ok(r) => crate::i18n::tf(format!("Take in at {:.2}s, synced {:+.0} ms", r.start, r.shift * 1000.0)),
                            Err(e) => e,
                        });
                    }
                    if ui.small_button("✓").on_hover_text(crate::i18n::shown("Approve the take")).clicked() {
                        let id = c.id;
                        let _ = desk::edit_desk(&mut app.doc, "ADR approved", |d| {
                            if let Some(x) = d.adr.iter_mut().find(|x| x.id == id) {
                                x.status = "approved".into();
                            }
                            Ok(String::new())
                        });
                    }
                });
                if ui.small_button("✕").on_hover_text(crate::i18n::shown("Remove the cue")).clicked() {
                    let id = c.id;
                    let _ = desk::edit_desk(&mut app.doc, "Remove ADR cue", |d| {
                        d.adr.retain(|x| x.id != id);
                        Ok(String::new())
                    });
                }
                ui.end_row();
            }
        });
    });
    if let Some(m) = &s.message {
        ui.label(egui::RichText::new(m).font(theme::caption()).color(t.text_2));
    }
}

fn subtitles_tab(app: &mut StudioApp, ui: &mut egui::Ui, d: &Desk, s: &mut PanelState, t: &theme::Tokens) {
    let fps = app.doc.project.fps;
    ui.horizontal_wrapped(|ui| {
        ui.label(egui::RichText::new(crate::i18n::shown("Language")).font(theme::caption()).color(t.text_2));
        ui.add(egui::TextEdit::singleline(&mut s.sub_lang).desired_width(50.0).hint_text("auto"));
        if Button::primary("From dialogue").icon(Icon::Subtitles).compact().show(ui).on_hover_text(crate::i18n::shown("Transcribe the dialogue and time subtitles by the Netflix rules")).clicked() && busy().is_empty() {
            let model = app.doc.model.clone();
            let lang = Some(s.sub_lang.trim().to_owned()).filter(|l| !l.is_empty());
            let shots: Vec<f32> = {
                let p = &app.doc.project;
                let spine = p.lanes.iter().find(|l| l.kind == crate::view::LaneKind::Spine).map(|l| l.id);
                p.clips.iter().filter(|c| Some(c.lane) == spine && c.start > 0).map(|c| c.start as f32 / 1e6).collect()
            };
            spawn("Transcribing", move || {
                Done::Track(desk::subs::transcribe(&model, lang.as_deref(), false).map(|(words, heard)| {
                    let l = lang.clone().unwrap_or(heard);
                    desk::subs::from_words(words, &l, &studio_text::subtitles::SubtitleSpec::netflix(&l, fps), &shots)
                }))
            });
        }
        if Button::secondary("Import…").compact().show(ui).clicked()
            && let Some(p) = rfd::FileDialog::new().add_filter("Subtitles", &["srt", "vtt"]).pick_file()
        {
            let lang = if s.sub_lang.trim().is_empty() { d.original_language.clone() } else { s.sub_lang.trim().to_owned() };
            let r = std::fs::read_to_string(&p).map_err(|e| e.to_string()).and_then(|text| desk::subs::read(&text, &lang));
            let (tx, rx) = channel();
            let _ = tx.send(Done::Track(r));
            jobs().lock().unwrap_or_else(|e| e.into_inner()).push(Job { label: "Importing".into(), rx });
        }
    });
    ui.separator();
    let selected = s.selected_track.clone().or_else(|| d.subtitles.first().map(|t| t.lang.clone()));
    ui.horizontal_wrapped(|ui| {
        for tr in &d.subtitles {
            let on = selected.as_deref() == Some(tr.lang.as_str());
            if ui.selectable_label(on, format!("{} ({}, {})", tr.name, tr.lang, tr.cues.len())).clicked() {
                s.selected_track = Some(tr.lang.clone());
                s.issues.clear();
            }
        }
    });
    let Some(tr) = selected.as_deref().and_then(|l| d.subtitles_of(l)).cloned() else {
        ui.label(egui::RichText::new(crate::i18n::shown("No subtitles yet: make them from the dialogue or import an SRT / VTT.")).font(theme::caption()).color(t.text_2));
        return;
    };
    let spec = studio_text::subtitles::SubtitleSpec::netflix(&tr.lang, fps);
    ui.horizontal_wrapped(|ui| {
        if Button::secondary("Check").compact().show(ui).clicked() {
            s.issues = desk::subs::check(&tr, &spec).iter().map(|v| format!("{} at {}s: {}", v["cue"], v["at"], v["problem"].as_str().unwrap_or(""))).collect();
            if s.issues.is_empty() {
                s.issues.push("Every subtitle passes".into());
            }
        }
        if Button::secondary("Conform").compact().show(ui).on_hover_text(crate::i18n::shown("Frame-accurate times, reading time, 5/6–7 s, 2-frame gaps, snapped to cuts")).clicked() {
            let mut t2 = tr.clone();
            desk::subs::conform(&mut t2, &spec, &[]);
            let _ = desk::edit_desk(&mut app.doc, "Conform subtitles", |d| {
                d.subtitles.retain(|x| !(x.lang == t2.lang && x.kind == t2.kind));
                d.subtitles.push(t2);
                Ok(String::new())
            });
        }
        if Button::ghost("Export subtitles…").compact().show(ui).on_hover_text(crate::i18n::shown("Subtitle files are exported from Deliver")).clicked() {
            s.tab = DELIVER;
        }
        if Button::secondary("Burn in").compact().show(ui).on_hover_text(crate::i18n::shown("Styled caption clips in the picture")).clicked() {
            s.message = Some(match desk::subs::burn_in(&mut app.doc, &tr, None) {
                Ok((_, n)) => format!("{n} subtitles burnt in"),
                Err(e) => e,
            });
        }
        ui.separator();
        ui.add(egui::TextEdit::singleline(&mut s.translate_to).desired_width(40.0));
        if Button::secondary("Translate").compact().show(ui).on_hover_text(crate::i18n::shown("With the OpenAI key; the AI editor can also translate them for you")).clicked() && busy().is_empty() {
            let (src, to) = (tr.clone(), s.translate_to.trim().to_owned());
            spawn("Translating", move || Done::Track(desk::subs::translate(&src, &to, None)));
        }
        let original = studio_text::subtitles::base_lang(&tr.lang) == studio_text::subtitles::base_lang(&d.original_language);
        if !original && Button::primary("Dub").icon(Icon::Microphone).compact().show(ui).on_hover_text(crate::i18n::shown("Speak every line in its character's voice, fitted to the original timing")).clicked() && busy().is_empty() {
            let model = app.doc.model.clone();
            let home = home(app);
            let lang = tr.lang.clone();
            spawn("Dubbing", move || Done::Dub(lang.clone(), desk::dub::prepare(&model, home.as_deref(), &lang, desk::dub::Fit::Phrases, &[], &[])));
        }
    });
    for i in s.issues.iter().take(8) {
        ui.label(egui::RichText::new(i).font(theme::caption()).color(t.warn));
    }
    // Characters and their voices for this language.
    let mut who: Vec<String> = tr.cues.iter().filter_map(|c| c.speaker.clone()).collect();
    who.sort();
    who.dedup();
    if !who.is_empty() {
        ui.label(egui::RichText::new(crate::i18n::tf(format!("Voices in {}", tr.name))).font(theme::caption()).color(t.text_2));
        egui::Grid::new("dub-voices").num_columns(2).show(ui, |ui| {
            for w in &who {
                ui.label(egui::RichText::new(w).font(theme::caption()));
                let mut v = d.characters.get(w).and_then(|c| c.voices.get(&tr.lang)).cloned().unwrap_or_default();
                let r = ui.add(egui::TextEdit::singleline(&mut v).desired_width(200.0).hint_text("voice id (Settings ▸ AI ▸ Voice & sound)"));
                if r.lost_focus() {
                    let (w, lang) = (w.clone(), tr.lang.clone());
                    let _ = desk::edit_desk(&mut app.doc, "Character voice", |d| {
                        let c = d.characters.entry(w).or_default();
                        if v.trim().is_empty() {
                            c.voices.remove(&lang);
                        } else {
                            c.voices.insert(lang, v.trim().to_owned());
                        }
                        Ok(String::new())
                    });
                }
                ui.end_row();
            }
        });
    }
    // The subtitles themselves.
    egui::ScrollArea::vertical().id_salt("sub-cues").show_v(ui, |ui| {
        for (i, c) in tr.cues.iter().enumerate() {
            let line = format!("{:>3}  {:>7.2} → {:>7.2}  {}{}", i + 1, c.start, c.end, c.speaker.as_ref().map(|s| format!("[{s}] ")).unwrap_or_default(), c.text.replace('\n', " / "));
            if ui.add(egui::Label::new(egui::RichText::new(line).font(egui::FontId::monospace(11.0)).color(t.text)).sense(Sense::click())).on_hover_text(crate::i18n::shown("Click: go there")).clicked() {
                app.editor.playhead = (c.start as f64 * 1e6) as i64;
            }
        }
    });
    if let Some(m) = &s.message {
        ui.label(egui::RichText::new(m).font(theme::caption()).color(t.text_2));
    }
}
