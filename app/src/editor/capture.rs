// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The Record window: cameras, microphones and screens (asking the system
//! for access only when a button is pressed), recording, voice-over that
//! plays the timeline and counts you in, the teleprompter, the live
//! mixer with streaming, and copying a card to two drives. Every button
//! here is the person's own action, so real devices are used as asked and
//! never otherwise. It drives the same `capture` tool the AI editor uses.

use crate::agent::{Content, Session};
use crate::app::StudioApp;
use super::library_plugins::plain_error;
use crate::icons::Icon;
use crate::kit;
use eframe::egui;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use studio_live::device::{self, Access, Devices};
use studio_live::source::Kind;

#[derive(Default)]
struct Panel {
    open: bool,
    tab: usize,
    devices: Option<Devices>,
    cam: Option<String>,
    mic: Option<String>,
    screen: Option<String>,
    rec: Option<u32>,
    vo: Option<u32>,
    vo_was_recording: bool,
    count_in: u32,
    bpm: f32,
    gain: f32,
    message: String,
    script: String,
    prompter_on: bool,
    wpm: f32,
    mirror: bool,
    follow: bool,
    tex: Option<egui::TextureHandle>,
    live: Option<u32>,
    live_tex: Option<egui::TextureHandle>,
    title: String,
    sub: String,
    stream_url: String,
    transition: usize,
    from: String,
    backup: String,
    ask: Arc<Mutex<Option<String>>>,
    last_status: Option<Value>,
    last_poll: f64,
}

fn panel() -> &'static Mutex<Panel> {
    static P: std::sync::OnceLock<Mutex<Panel>> = std::sync::OnceLock::new();
    P.get_or_init(|| Mutex::new(Panel { count_in: 3, bpm: 100.0, gain: 1.0, wpm: 150.0, ..Default::default() }))
}

/// Runs a `capture` call on the open project. The window's buttons are the
/// person's own action, so real devices are allowed in what it sends.
fn call(app: &mut StudioApp, args: Value) -> Result<Value, String> {
    let doc = std::mem::replace(&mut app.doc, Session::default().doc);
    let mut s = Session::new(doc);
    let r = s.call("capture", &args);
    app.doc = s.doc;
    let parts = r?;
    parts.iter().find_map(|p| if let Content::Text(t) = p { serde_json::from_str(t).ok() } else { None }).ok_or_else(|| "no answer".to_string())
}

/// Shows a failed call in the panel's message line, in words.
fn check(p: &mut Panel, what: &str, r: Result<Value, String>) {
    if let Err(e) = r {
        p.message = plain_error(what, &e);
    }
}

fn access_label(a: Access) -> &'static str {
    match a {
        Access::Authorized => "allowed",
        Access::NotDetermined => "not asked yet",
        Access::Denied => "turned off",
        Access::Restricted => "blocked",
        Access::Unavailable => "not available",
    }
}

/// A labelled device menu: "None" first, then each device.
fn pick(ui: &mut egui::Ui, id: &str, label: &str, items: &[(String, String)], cur: &mut Option<String>) {
    let selected = cur.as_ref().and_then(|c| items.iter().position(|(i, _)| i == c)).map_or(0, |i| i + 1);
    let mut names: Vec<&str> = vec!["None"];
    names.extend(items.iter().map(|(_, n)| n.as_str()));
    kit::labeled_row(ui, label, |ui| {
        // TODO(kit): menu width is a local constant until the kit has a fill-width menu button.
        let width = (ui.available_width() - 6.0).clamp(140.0, 300.0);
        if let Some(i) = kit::menu_button(ui, id, selected, &names, width, None) {
            *cur = if i == 0 { None } else { Some(items[i - 1].0.clone()) };
        }
    });
}

/// An on/off switch with its label after it.
fn switch(ui: &mut egui::Ui, on: &mut bool, label: &str) -> egui::Response {
    let t = crate::theme::tokens(ui.ctx());
    ui.horizontal(|ui| {
        let r = kit::toggle(ui, on, label, None);
        ui.label(egui::RichText::new(kit::shown(label)).font(crate::theme::body()).color(t.text));
        r
    })
    .inner
}

/// Small explanatory text.
fn note(ui: &mut egui::Ui, text: &str) {
    let t = crate::theme::tokens(ui.ctx());
    ui.label(egui::RichText::new(text).font(crate::theme::caption()).color(t.text_2));
}

fn items(d: &[device::Device]) -> Vec<(String, String)> {
    d.iter().map(|x| (x.id.clone(), x.name.clone())).collect()
}

/// Whether the Record panel is open (docked or in its window).
pub fn is_open() -> bool {
    panel().lock().is_ok_and(|p| p.open)
}

pub fn set_open(open: bool) {
    if let Ok(mut p) = panel().lock() {
        p.open = open;
    }
}

/// The panel docked in the editor, in `r`.
pub fn docked(app: &mut StudioApp, ui: &mut egui::Ui, r: egui::Rect) {
    let Ok(mut guard) = panel().lock() else { return };
    let p = &mut *guard;
    let ctx = ui.ctx().clone();
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r.shrink(10.0)).layout(egui::Layout::top_down(egui::Align::Min)).id_salt("record-docked"));
    child.set_clip_rect(r.intersect(ui.clip_rect()));
    egui::ScrollArea::vertical().id_salt("record-docked-scroll").auto_shrink([false, false]).show(&mut child, |ui| body(app, ui, &ctx, p, None));
}

/// The tabs and their controls (in the window or docked); `grip` puts the
/// dock handle before the tabs.
fn body(app: &mut StudioApp, ui: &mut egui::Ui, ctx: &egui::Context, p: &mut Panel, grip: Option<&mut dyn FnMut(&mut StudioApp, &mut egui::Ui)>) {
    // One line of tabs: a narrow window scrolls them sideways.
    crate::kit::scroll_strip(ui, "record-tabs", |ui| {
        if let Some(g) = grip {
            g(app, ui);
        }
        let tabs = ["Record", "Voice-over", "Prompter", "Live", "Cards"].map(kit::Segment::text);
        kit::segmented(ui, "record-tabs-seg", &mut p.tab, &tabs, None, None);
    });
    ui.add_space(crate::theme::space::GAP);
    // Taller than the window or panel allows: the tab scrolls.
    crate::kit::scroll_panel(ui, ("record-body", p.tab), |ui| {
        match p.tab {
            0 => tab_record(app, ui, p),
            1 => tab_voiceover(app, ui, p),
            2 => tab_prompter(app, ui, ctx, p),
            3 => tab_live(app, ui, ctx, p),
            _ => tab_cards(app, ui, p),
        }
        if !p.message.is_empty() {
            ui.add_space(crate::theme::space::SECTION_GAP);
            note(ui, &p.message.clone());
        }
    });
}

/// Keeps recordings going and draws the window when it is open and not
/// docked (called every frame from the editor).
pub fn show(app: &mut StudioApp, ctx: &egui::Context) {
    let Ok(mut guard) = panel().lock() else { return };
    let p = &mut *guard;
    if !p.open && p.rec.is_none() && p.vo.is_none() && p.live.is_none() {
        return;
    }
    // Answers from a permission question asked on another thread.
    if let Some(msg) = p.ask.lock().ok().and_then(|mut a| a.take()) {
        p.message = msg;
        p.devices = None;
    }
    let now = ctx.input(|i| i.time);
    let busy = p.rec.is_some() || p.vo.is_some() || p.live.is_some();
    if busy && now - p.last_poll > 0.1 {
        p.last_poll = now;
        p.last_status = call(app, json!({"action": "status"})).ok();
    }
    if busy || p.prompter_on {
        ctx.request_repaint_after(std::time::Duration::from_millis(33));
    }
    // Voice-over over picture: the timeline plays once the count-in is over.
    if let Some(v) = p.vo {
        let recording = p.last_status.as_ref().and_then(|s| s["voiceovers"].as_array()).and_then(|a| a.iter().find(|x| x["session"] == v)).map(|x| x["state"] == "Recording").unwrap_or(false);
        if recording && !p.vo_was_recording {
            app.editor.playing = true;
        }
        p.vo_was_recording = recording;
    }
    if p.devices.is_none() && p.open {
        p.devices = Some(device::list());
    }
    if super::dock_ui::docked(ctx, crate::dock::Panel::Record) {
        return;
    }
    let mut open = p.open;
    // TODO(kit): a floating-window helper (title bar, close, grip) until then the theme styles the egui window.
    egui::Window::new(crate::i18n::visual("Record").into_owned()).id(egui::Id::new("record-window")).open(&mut open).default_width(420.0).min_width(320.0).resizable(true).collapsible(false).show(ctx, |ui| {
        let mut grip = |app: &mut StudioApp, ui: &mut egui::Ui| super::dock_ui::float_grip(app, ui, crate::dock::Panel::Record);
        body(app, ui, ctx, p, Some(&mut grip));
    });
    p.open = open;
}

fn access_row(ui: &mut egui::Ui, p: &mut Panel, kind: Kind, what: &str, request: &'static str) {
    let a = device::access(kind);
    let t = crate::theme::tokens(ui.ctx());
    ui.horizontal_wrapped(|ui| {
        ui.label(egui::RichText::new(format!("{what}: {}", access_label(a))).font(crate::theme::body()).color(t.text));
        if a == Access::NotDetermined && kit::Button::secondary(&format!("Allow {}…", what.to_lowercase())).compact().show(ui).clicked() {
            // The system's own question; asked here because the person pressed this.
            let slot = p.ask.clone();
            std::thread::spawn(move || {
                let r = device::request(kind);
                let msg = match r {
                    Ok(a) if a.ok() => format!("{request} allowed."),
                    Ok(a) => device::explain(kind, a),
                    Err(e) => e,
                };
                if let Ok(mut s) = slot.lock() {
                    *s = Some(msg);
                }
            });
        } else if matches!(a, Access::Denied | Access::Restricted) {
            note(ui, &device::explain(kind, a));
        }
    });
}

fn tab_record(app: &mut StudioApp, ui: &mut egui::Ui, p: &mut Panel) {
    let Some(d) = p.devices.clone() else { return };
    kit::menu_heading(ui, "Access");
    access_row(ui, p, Kind::Camera, "Camera", "The camera");
    access_row(ui, p, Kind::Mic, "Microphone", "The microphone");
    access_row(ui, p, Kind::Screen, "Screen recording", "Screen recording");
    kit::menu_separator(ui);
    kit::menu_heading(ui, "Sources");
    pick(ui, "rec-cam", "Camera", &items(&d.cameras), &mut p.cam);
    pick(ui, "rec-mic", "Microphone", &items(&d.mics), &mut p.mic);
    let mut screens = items(&d.screens);
    screens.extend(items(&d.windows));
    pick(ui, "rec-screen", "Screen", &screens, &mut p.screen);
    if kit::Button::ghost("Refresh devices").icon(Icon::ArrowCounterClockwise).compact().show(ui).clicked() {
        p.devices = None;
    }
    kit::menu_separator(ui);
    if let Some(id) = p.rec {
        let secs = p.last_status.as_ref().and_then(|s| s["recordings"].as_array()).and_then(|a| a.iter().find(|x| x["session"] == id)).map(|x| x["progress"]["seconds"].as_f64().unwrap_or(0.0)).unwrap_or(0.0);
        kit::menu_heading(ui, &format!("Recording {secs:.1} s"));
        if let Some(levels) = p.last_status.as_ref().and_then(|s| s["recordings"].as_array()).and_then(|a| a.iter().find(|x| x["session"] == id)).and_then(|x| x["progress"]["levels"].as_array()) {
            for l in levels {
                let db = l["peak_db"].as_f64().unwrap_or(-120.0);
                ui.add(egui::ProgressBar::new(((db + 60.0) / 60.0).clamp(0.0, 1.0) as f32).text(format!("{} {db:.0} dB{}", l["label"].as_str().unwrap_or(""), if l["clipped"] == true { "  CLIPPED" } else { "" })));
            }
        }
        if kit::Button::primary("Stop and add to timeline").icon(Icon::Square).show(ui).clicked() {
            let at = app.editor.playhead as f64 / 1e6;
            match call(app, json!({"action": "stop", "session": id, "to_timeline": true, "at": at})) {
                Ok(v) => p.message = format!("Added {} recording(s) to the timeline.", v["recorded"].as_array().map_or(0, Vec::len)),
                Err(e) => p.message = plain_error("stop the recording", &e),
            }
            p.rec = None;
        }
    } else if ui.add_enabled_ui(p.cam.is_some() || p.mic.is_some() || p.screen.is_some(), |ui| kit::Button::primary("Record").icon(Icon::Circle).show(ui)).inner.clicked() {
        let mut inputs = Vec::new();
        if p.cam.is_some() {
            inputs.push(json!({"label": "camera", "video": p.cam, "audio": p.mic}));
        } else if p.mic.is_some() {
            inputs.push(json!({"label": "microphone", "audio": p.mic}));
        }
        if let Some(s) = &p.screen {
            inputs.push(json!({"label": "screen", "video": s, "cursor": true}));
        }
        match call(app, json!({"action": "record", "inputs": inputs, "user_approved": true, "name": "take"})) {
            Ok(v) => p.rec = v["session"].as_u64().map(|x| x as u32),
            Err(e) => p.message = plain_error("start recording", &e),
        }
    }
}

fn tab_voiceover(app: &mut StudioApp, ui: &mut egui::Ui, p: &mut Panel) {
    let Some(d) = p.devices.clone() else { return };
    access_row(ui, p, Kind::Mic, "Microphone", "The microphone");
    pick(ui, "vo-mic", "Microphone", &items(&d.mics), &mut p.mic);
    kit::labeled_row(ui, "Count-in", |ui| {
        const BEATS: [u32; 3] = [0, 3, 4];
        let mut sel = BEATS.iter().position(|b| *b == p.count_in).unwrap_or(0);
        let opts = ["None", "3 beats", "4 beats"].map(kit::Segment::text);
        if kit::segmented(ui, "vo-count-in", &mut sel, &opts, None, None).changed() {
            p.count_in = BEATS[sel];
        }
    });
    kit::Slider::new("tempo", &mut p.bpm, 60.0..=180.0).default(120.0).step(1.0).with_label().with_value().show(ui);
    kit::Slider::new("input level", &mut p.gain, 0.0..=2.0).default(1.0).decimals(2).with_label().with_value().show(ui);
    note(ui, "Wear headphones: the timeline plays while you speak.");
    if let Some(id) = p.vo {
        let st = p.last_status.as_ref().and_then(|s| s["voiceovers"].as_array()).and_then(|a| a.iter().find(|x| x["session"] == id)).cloned();
        if let Some(st) = &st {
            let db = st["peak_db"].as_f64().unwrap_or(-120.0);
            let db = if db.is_finite() { db } else { -120.0 };
            ui.add(egui::ProgressBar::new(((db + 60.0) / 60.0).clamp(0.0, 1.0) as f32).text(format!("{} {:.1} s{}", st["state"].as_str().unwrap_or(""), st["seconds_recorded"].as_f64().unwrap_or(0.0), if st["clipped"] == true { "  CLIPPED" } else { "" })));
        }
        if kit::Button::primary("Stop and place on the timeline").icon(Icon::Square).show(ui).clicked() {
            app.editor.playing = false;
            match call(app, json!({"action": "stop", "session": id})) {
                Ok(v) => p.message = format!("Voice-over placed at {:.1} s ({:.1} s). {}", v["starts_at"].as_f64().unwrap_or(0.0), v["seconds"].as_f64().unwrap_or(0.0), v["advice"].as_str().unwrap_or("")),
                Err(e) => p.message = plain_error("place the voice-over", &e),
            }
            p.vo = None;
            p.vo_was_recording = false;
        }
    } else if ui.add_enabled_ui(p.mic.is_some(), |ui| kit::Button::primary("Record voice-over at the playhead").icon(Icon::Circle).show(ui)).inner.clicked() {
        let at = app.editor.playhead as f64 / 1e6;
        match call(app, json!({"action": "voiceover", "mic": p.mic, "at": at, "count_in": p.count_in, "bpm": p.bpm, "gain": p.gain, "click": true, "user_approved": true})) {
            Ok(v) => {
                p.vo = v["session"].as_u64().map(|x| x as u32);
                p.message = if p.count_in > 0 { "Counting in…".into() } else { String::new() };
            }
            Err(e) => p.message = plain_error("start the voice-over", &e),
        }
    }
}

fn tab_prompter(app: &mut StudioApp, ui: &mut egui::Ui, ctx: &egui::Context, p: &mut Panel) {
    if !p.prompter_on {
        kit::menu_heading(ui, "Script");
        note(ui, "Leave empty to read the story's dialogue and narration.");
        ui.add(egui::TextEdit::multiline(&mut p.script).desired_rows(5).desired_width(f32::INFINITY));
        switch(ui, &mut p.follow, "Follow my voice");
        note(ui, "Needs the speech model and the microphone.");
        if p.follow {
            let d = p.devices.clone().unwrap_or_default();
            access_row(ui, p, Kind::Mic, "Microphone", "The microphone");
            pick(ui, "prompter-mic", "Microphone", &items(&d.mics), &mut p.mic);
        }
        kit::Slider::new("words a minute", &mut p.wpm, 60.0..=260.0).default(150.0).step(1.0).with_label().with_value().show(ui);
        switch(ui, &mut p.mirror, "Mirror for a prompter glass");
        ui.add_space(crate::theme::space::GAP);
        if kit::Button::primary("Start prompter").icon(Icon::Play).show(ui).clicked() {
            let mut args = json!({"action": "prompter", "do": "load", "wpm": p.wpm, "mirror": p.mirror, "mode": if p.follow { "follow" } else { "pace" }});
            if p.script.trim().is_empty() {
                args["scene"] = json!("all");
            } else {
                args["text"] = json!(p.script);
            }
            match call(app, args).and_then(|_| call(app, json!({"action": "prompter", "do": "play"}))) {
                Ok(_) => {
                    p.prompter_on = true;
                    if p.follow {
                        let mic = p.mic.clone().and_then(|m| m.strip_prefix("mic:").map(str::to_owned)).unwrap_or_else(|| "default".into());
                        if let Err(e) = crate::agent::ui_start_follow(&mic) {
                            p.message = plain_error("follow your voice", &e);
                        }
                    }
                }
                Err(e) => p.message = plain_error("start the prompter", &e),
            }
        }
        return;
    }
    let dt = ctx.input(|i| i.stable_dt) as f64;
    let w = (ui.available_width().max(320.0)) as u32;
    if let Some(f) = crate::agent::ui_prompter_frame(dt, w, (w as f32 * 0.5625) as u32) {
        let img = egui::ColorImage::from_rgba_unmultiplied([f.w as usize, f.h as usize], &f.rgba);
        match &mut p.tex {
            Some(t) => t.set(img, egui::TextureOptions::LINEAR),
            None => p.tex = Some(ctx.load_texture("prompter", img, egui::TextureOptions::LINEAR)),
        }
    }
    if let Some(t) = &p.tex {
        ui.image((t.id(), egui::vec2(w as f32, w as f32 * 0.5625)));
    }
    ui.horizontal_wrapped(|ui| {
        if kit::Button::secondary("Play or pause").icon(Icon::Play).compact().show(ui).clicked() {
            let running = p.last_status.as_ref().and_then(|s| s["prompter"]["running"].as_bool()).unwrap_or(true);
            check(p, "play or pause the prompter", call(app, json!({"action": "prompter", "do": if running { "pause" } else { "play" }})));
        }
        if kit::Button::secondary("Back").icon(Icon::SkipBack).compact().show(ui).clicked() {
            check(p, "move the prompter back", call(app, json!({"action": "prompter", "do": "back"})));
        }
        if switch(ui, &mut p.mirror, "Mirror").changed() {
            check(p, "mirror the prompter", call(app, json!({"action": "prompter", "do": "settings", "mirror": p.mirror})));
        }
        if kit::Slider::new("wpm", &mut p.wpm, 60.0..=260.0).default(150.0).step(1.0).with_label().with_value().show(ui).changed() {
            check(p, "change the prompter speed", call(app, json!({"action": "prompter", "do": "settings", "wpm": p.wpm})));
        }
        if kit::Button::ghost("Close").icon(Icon::X).compact().show(ui).clicked() {
            crate::agent::ui_stop_follow();
            check(p, "close the prompter", call(app, json!({"action": "prompter", "do": "unload"})));
            p.prompter_on = false;
        }
    });
    p.last_status = call(app, json!({"action": "status"})).ok();
}

fn tab_live(app: &mut StudioApp, ui: &mut egui::Ui, ctx: &egui::Context, p: &mut Panel) {
    let Some(d) = p.devices.clone() else { return };
    let Some(id) = p.live else {
        note(ui, "Pick the cameras and screens to switch between, then open the mixer.");
        pick(ui, "live-cam", "Camera", &items(&d.cameras), &mut p.cam);
        let mut screens = items(&d.screens);
        screens.extend(items(&d.windows));
        pick(ui, "live-screen", "Screen", &screens, &mut p.screen);
        pick(ui, "live-mic", "Microphone", &items(&d.mics), &mut p.mic);
        ui.add_space(crate::theme::space::GAP);
        if ui.add_enabled_ui(p.cam.is_some() || p.screen.is_some(), |ui| kit::Button::primary("Open the mixer").icon(Icon::Faders).show(ui)).inner.clicked() {
            let mut sources = Vec::new();
            if let Some(c) = &p.cam {
                sources.push(json!({"name": "Camera", "video": c, "audio": p.mic}));
            }
            if let Some(s) = &p.screen {
                sources.push(json!({"name": "Screen", "video": s}));
            }
            match call(app, json!({"action": "live", "do": "open", "sources": sources, "user_approved": true, "realtime": true, "replay_secs": 30, "record_program": true, "iso": true, "width": 1280, "height": 720})) {
                Ok(v) => p.live = v["session"].as_u64().map(|x| x as u32),
                Err(e) => p.message = plain_error("open the mixer", &e),
            }
        }
        return;
    };
    if let Some(f) = crate::agent::ui_live_frame(id, true) {
        let img = egui::ColorImage::from_rgba_unmultiplied([f.w as usize, f.h as usize], &f.rgba);
        match &mut p.live_tex {
            Some(t) => t.set(img, egui::TextureOptions::LINEAR),
            None => p.live_tex = Some(ctx.load_texture("live-multiview", img, egui::TextureOptions::LINEAR)),
        }
    }
    if let Some(t) = &p.live_tex {
        let w = ui.available_width().max(320.0);
        ui.image((t.id(), egui::vec2(w, w * 0.5625)));
    }
    let sources: Vec<String> = p.last_status.as_ref().and_then(|s| s["productions"].as_array()).and_then(|a| a.iter().find(|x| x["session"] == id)).and_then(|x| x["state"]["sources"].as_array()).map(|a| a.iter().filter_map(|s| s["name"].as_str().map(str::to_owned)).collect()).unwrap_or_default();
    const TRANS: [&str; 5] = ["cut", "mix", "dip", "wipe", "push"];
    const TRANS_NAMES: [&str; 5] = ["Cut", "Mix", "Dip", "Wipe", "Push"];
    kit::menu_heading(ui, "Take to program");
    ui.horizontal_wrapped(|ui| {
        for (i, name) in sources.iter().enumerate() {
            if kit::Button::secondary(name).compact().show(ui).clicked() {
                let t = TRANS[p.transition];
                check(p, "take that source to program", call(app, json!({"action": "live", "do": "take", "session": id, "to": i, "transition": t})));
            }
        }
        if let Some(i) = kit::menu_button(ui, "live-trans", p.transition, &TRANS_NAMES, 90.0, None) {
            p.transition = i;
        }
    });
    kit::menu_separator(ui);
    kit::menu_heading(ui, "Title");
    ui.horizontal_wrapped(|ui| {
        kit::text_field(ui, &mut p.title, "Name", None, 120.0, None);
        kit::text_field(ui, &mut p.sub, "Role", None, 120.0, None);
        if kit::Button::secondary("Show title").compact().show(ui).clicked() {
            check(p, "show the title", call(app, json!({"action": "live", "do": "title", "session": id, "kind": "lower", "text": p.title, "sub": p.sub})));
        }
        if kit::Button::ghost("Hide title").compact().show(ui).clicked() {
            check(p, "hide the title", call(app, json!({"action": "live", "do": "title_off", "session": id})));
        }
    });
    kit::menu_separator(ui);
    kit::menu_heading(ui, "Stream");
    ui.horizontal_wrapped(|ui| {
        // TODO(kit): the address is masked (stream keys), which kit::text_field cannot do yet.
        ui.add(egui::TextEdit::singleline(&mut p.stream_url).password(true).font(crate::theme::control()).hint_text("rtmp://… or srt://…").desired_width(220.0));
        if kit::Button::primary("Go live").compact().show(ui).clicked() {
            match call(app, json!({"action": "live", "do": "stream", "session": id, "url": p.stream_url, "user_approved": true})) {
                Ok(_) => p.message = "Streaming.".into(),
                Err(e) => p.message = plain_error("go live", &e),
            }
        }
        if kit::Button::secondary("Stop stream").compact().show(ui).clicked() {
            check(p, "stop the stream", call(app, json!({"action": "live", "do": "stop_stream", "session": id})));
        }
    });
    if let Some(streams) = p.last_status.as_ref().and_then(|s| s["productions"].as_array()).and_then(|a| a.iter().find(|x| x["session"] == id)).and_then(|x| x["streams"].as_array()) {
        for s in streams {
            note(ui, &format!("{}: {} ({} sent, {} dropped)", s["target"].as_str().unwrap_or(""), s["state"].as_str().unwrap_or(""), s["sent"], s["dropped"]));
        }
    }
    kit::menu_separator(ui);
    kit::menu_heading(ui, "Replay and finish");
    ui.horizontal_wrapped(|ui| {
        if kit::Button::secondary("Save last 20 s").compact().show(ui).clicked() {
            match call(app, json!({"action": "live", "do": "replay_save", "session": id, "seconds": 20})) {
                Ok(v) => p.message = format!("Replay saved: {}", v["saved"].as_str().unwrap_or("")),
                Err(e) => p.message = plain_error("save the replay", &e),
            }
        }
        if kit::Button::secondary("Replay at half speed").compact().show(ui).clicked() {
            match call(app, json!({"action": "live", "do": "replay_play", "session": id, "seconds": 10, "speed": 0.5})) {
                Ok(_) => p.message = "Replay is a new source: take it to program.".into(),
                Err(e) => p.message = plain_error("play the replay", &e),
            }
        }
        if kit::Button::primary("Close and add to timeline").icon(Icon::Square).compact().show(ui).clicked() {
            match call(app, json!({"action": "live", "do": "close", "session": id, "to_timeline": true, "at": app.editor.playhead as f64 / 1e6})) {
                Ok(v) => p.message = format!("{} file(s) added.", v["files"].as_array().map_or(0, Vec::len)),
                Err(e) => p.message = plain_error("close the live mixer", &e),
            }
            p.live = None;
            p.live_tex = None;
        }
    });
}

fn tab_cards(app: &mut StudioApp, ui: &mut egui::Ui, p: &mut Panel) {
    if kit::Button::secondary("Look for cards").icon(Icon::MagnifyingGlass).show(ui).clicked() {
        match call(app, json!({"action": "cards"})) {
            Ok(v) => {
                let first = v["cards"].as_array().and_then(|a| a.first()).cloned();
                p.message = format!("{} card(s) found.", v["cards"].as_array().map_or(0, Vec::len));
                if let Some(c) = first {
                    p.from = c["path"].as_str().unwrap_or("").to_owned();
                }
            }
            Err(e) => p.message = plain_error("look for cards", &e),
        }
    }
    kit::menu_separator(ui);
    kit::menu_heading(ui, "From (card or folder)");
    let w = ui.available_width();
    kit::text_field(ui, &mut p.from, "Card or folder", Some(Icon::Folder), w, None);
    kit::menu_heading(ui, "Second copy on another drive");
    kit::text_field(ui, &mut p.backup, "Drive or folder", Some(Icon::HardDrives), w, None);
    ui.add_space(crate::theme::space::GAP);
    if ui.add_enabled_ui(!p.from.is_empty() && !p.backup.is_empty(), |ui| kit::Button::primary("Copy, check and add to project").icon(Icon::Copy).show(ui)).inner.clicked() {
        match call(app, json!({"action": "ingest", "from": p.from, "backup": p.backup, "import": true})) {
            Ok(v) => p.message = format!("{} file(s), {} MB copied; {} verified{}", v["files"], v["megabytes"], v["verified"], if v["ok"] == true { ". Every copy matches the card." } else { ". PROBLEMS: see the AI editor's report; do not wipe the card." }),
            Err(e) => p.message = plain_error("copy the card", &e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_tests::Harness;

    #[test]
    fn failed_calls_say_what_went_wrong_in_words() {
        let mut p = Panel::default();
        check(&mut p, "hide the title", Err("No such file or directory (os error 2)".into()));
        assert_eq!(p.message, "Couldn't hide the title: the file is not there");
        p.message.clear();
        check(&mut p, "hide the title", Ok(json!({})));
        assert!(p.message.is_empty(), "a good call leaves the line alone");
    }

    #[test]
    fn record_window_labels_fit_and_say_screen() {
        let mut h = Harness::new(&["--screen", "editor"]).sized(900.0, 700.0);
        {
            let mut p = panel().lock().unwrap();
            p.open = true;
            p.tab = 0;
            p.devices = Some(Devices::default());
        }
        h.frames(4);
        let mut labels = h.labels();
        let out = h.last.as_ref().expect("a frame");
        fn walk(s: &egui::epaint::Shape, out: &mut Vec<String>) {
            match s {
                egui::epaint::Shape::Text(t) => out.push(t.galley.text().to_owned()),
                egui::epaint::Shape::Vec(v) => v.iter().for_each(|s| walk(s, out)),
                _ => {}
            }
        }
        out.shapes.iter().for_each(|c| walk(&c.shape, &mut labels));
        let _ = panel().lock().map(|mut p| {
            p.open = false;
            p.devices = None;
        });
        assert!(labels.iter().any(|l| l == "Screen"), "{labels:?}");
        assert!(!labels.iter().any(|l| l.contains("Screen or window")), "the long label is gone");
        let cut: Vec<_> = h.cut_text().into_iter().filter(|c| ["Screen", "Camera", "Microphone", "Record"].contains(&c.text.as_str())).collect();
        assert!(cut.is_empty(), "{cut:?}");
    }
}
