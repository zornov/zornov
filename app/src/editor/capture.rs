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
use crate::icons::Icon;
use crate::kit;
use crate::kit::IconButton;
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

/// The viewer toolbar's button.
pub fn toolbar_button(ui: &mut egui::Ui) {
    let Ok(mut p) = panel().lock() else { return };
    if IconButton::new(Icon::Microphone, "Record, voice-over, prompter and live").active(p.open).show(ui).clicked() {
        p.open = !p.open;
    }
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

fn access_label(a: Access) -> &'static str {
    match a {
        Access::Authorized => "allowed",
        Access::NotDetermined => "not asked yet",
        Access::Denied => "turned off",
        Access::Restricted => "blocked",
        Access::Unavailable => "not available",
    }
}

fn pick(ui: &mut egui::Ui, id: &str, label: &str, items: &[(String, String)], cur: &mut Option<String>) {
    let name = cur.as_ref().and_then(|c| items.iter().find(|(i, _)| i == c)).map(|(_, n)| n.clone()).unwrap_or_else(|| "None".into());
    egui::ComboBox::from_id_salt(id).selected_text(format!("{label}: {name}")).width(280.0).show_ui(ui, |ui| {
        crate::kit::note_list(ui);
        ui.selectable_value(cur, None, "None");
        for (i, n) in items {
            ui.selectable_value(cur, Some(i.clone()), n);
        }
    });
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
        for (i, t) in ["Record", "Voice-over", "Prompter", "Live", "Cards"].iter().enumerate() {
            if ui.selectable_label(p.tab == i, *t).clicked() {
                p.tab = i;
            }
        }
    });
    ui.separator();
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
            ui.add_space(6.0);
            ui.label(egui::RichText::new(p.message.clone()).small());
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
    egui::Window::new("Record").open(&mut open).default_width(420.0).resizable(true).show(ctx, |ui| {
        let mut grip = |app: &mut StudioApp, ui: &mut egui::Ui| super::dock_ui::float_grip(app, ui, crate::dock::Panel::Record);
        body(app, ui, ctx, p, Some(&mut grip));
    });
    p.open = open;
}

fn access_row(ui: &mut egui::Ui, p: &mut Panel, kind: Kind, what: &str, request: &'static str) {
    let a = device::access(kind);
    ui.horizontal_wrapped(|ui| {
        ui.label(format!("{what}: {}", access_label(a)));
        if a == Access::NotDetermined && ui.small_button(format!("Allow {what}…")).clicked() {
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
            ui.label(egui::RichText::new(device::explain(kind, a)).small());
        }
    });
}

fn tab_record(app: &mut StudioApp, ui: &mut egui::Ui, p: &mut Panel) {
    let Some(d) = p.devices.clone() else { return };
    access_row(ui, p, Kind::Camera, "Camera", "The camera");
    access_row(ui, p, Kind::Mic, "Microphone", "The microphone");
    access_row(ui, p, Kind::Screen, "Screen recording", "Screen recording");
    ui.separator();
    pick(ui, "rec-cam", "Camera", &items(&d.cameras), &mut p.cam);
    pick(ui, "rec-mic", "Microphone", &items(&d.mics), &mut p.mic);
    let mut screens = items(&d.screens);
    screens.extend(items(&d.windows));
    pick(ui, "rec-screen", "Screen or window", &screens, &mut p.screen);
    if ui.small_button("Refresh devices").clicked() {
        p.devices = None;
    }
    ui.add_space(6.0);
    if let Some(id) = p.rec {
        let secs = p.last_status.as_ref().and_then(|s| s["recordings"].as_array()).and_then(|a| a.iter().find(|x| x["session"] == id)).map(|x| x["progress"]["seconds"].as_f64().unwrap_or(0.0)).unwrap_or(0.0);
        ui.label(format!("Recording {secs:.1} s"));
        if let Some(levels) = p.last_status.as_ref().and_then(|s| s["recordings"].as_array()).and_then(|a| a.iter().find(|x| x["session"] == id)).and_then(|x| x["progress"]["levels"].as_array()) {
            for l in levels {
                let db = l["peak_db"].as_f64().unwrap_or(-120.0);
                ui.add(egui::ProgressBar::new(((db + 60.0) / 60.0).clamp(0.0, 1.0) as f32).text(format!("{} {db:.0} dB{}", l["label"].as_str().unwrap_or(""), if l["clipped"] == true { "  CLIPPED" } else { "" })));
            }
        }
        if ui.button("■  Stop and add to timeline").clicked() {
            let at = app.editor.playhead as f64 / 1e6;
            match call(app, json!({"action": "stop", "session": id, "to_timeline": true, "at": at})) {
                Ok(v) => p.message = format!("Added {} recording(s) to the timeline.", v["recorded"].as_array().map_or(0, Vec::len)),
                Err(e) => p.message = e,
            }
            p.rec = None;
        }
    } else if ui.add_enabled(p.cam.is_some() || p.mic.is_some() || p.screen.is_some(), egui::Button::new("●  Record")).clicked() {
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
            Err(e) => p.message = e,
        }
    }
}

fn tab_voiceover(app: &mut StudioApp, ui: &mut egui::Ui, p: &mut Panel) {
    let Some(d) = p.devices.clone() else { return };
    access_row(ui, p, Kind::Mic, "Microphone", "The microphone");
    pick(ui, "vo-mic", "Microphone", &items(&d.mics), &mut p.mic);
    ui.horizontal_wrapped(|ui| {
        ui.label("Count-in");
        for b in [0u32, 3, 4] {
            ui.selectable_value(&mut p.count_in, b, if b == 0 { "none".to_owned() } else { format!("{b} beats") });
        }
    });
    kit::Slider::new("tempo", &mut p.bpm, 60.0..=180.0).default(120.0).step(1.0).with_label().with_value().show(ui);
    kit::Slider::new("input level", &mut p.gain, 0.0..=2.0).default(1.0).decimals(2).with_label().with_value().show(ui);
    ui.label(egui::RichText::new("Wear headphones: the timeline plays while you speak.").small());
    if let Some(id) = p.vo {
        let st = p.last_status.as_ref().and_then(|s| s["voiceovers"].as_array()).and_then(|a| a.iter().find(|x| x["session"] == id)).cloned();
        if let Some(st) = &st {
            let db = st["peak_db"].as_f64().unwrap_or(-120.0);
            let db = if db.is_finite() { db } else { -120.0 };
            ui.add(egui::ProgressBar::new(((db + 60.0) / 60.0).clamp(0.0, 1.0) as f32).text(format!("{} {:.1} s{}", st["state"].as_str().unwrap_or(""), st["seconds_recorded"].as_f64().unwrap_or(0.0), if st["clipped"] == true { "  CLIPPED" } else { "" })));
        }
        if ui.button("■  Stop and place on the timeline").clicked() {
            app.editor.playing = false;
            match call(app, json!({"action": "stop", "session": id})) {
                Ok(v) => p.message = format!("Voice-over placed at {:.1} s ({:.1} s). {}", v["starts_at"].as_f64().unwrap_or(0.0), v["seconds"].as_f64().unwrap_or(0.0), v["advice"].as_str().unwrap_or("")),
                Err(e) => p.message = e,
            }
            p.vo = None;
            p.vo_was_recording = false;
        }
    } else if ui.add_enabled(p.mic.is_some(), egui::Button::new("●  Record voice-over at the playhead")).clicked() {
        let at = app.editor.playhead as f64 / 1e6;
        match call(app, json!({"action": "voiceover", "mic": p.mic, "at": at, "count_in": p.count_in, "bpm": p.bpm, "gain": p.gain, "click": true, "user_approved": true})) {
            Ok(v) => {
                p.vo = v["session"].as_u64().map(|x| x as u32);
                p.message = if p.count_in > 0 { "Counting in…".into() } else { String::new() };
            }
            Err(e) => p.message = e,
        }
    }
}

fn tab_prompter(app: &mut StudioApp, ui: &mut egui::Ui, ctx: &egui::Context, p: &mut Panel) {
    if !p.prompter_on {
        ui.label("Script (leave empty to read the story's dialogue and narration):");
        ui.add(egui::TextEdit::multiline(&mut p.script).desired_rows(5).desired_width(f32::INFINITY));
        ui.checkbox(&mut p.follow, "Follow my voice (needs the speech model and the microphone)");
        if p.follow {
            let d = p.devices.clone().unwrap_or_default();
            access_row(ui, p, Kind::Mic, "Microphone", "The microphone");
            pick(ui, "prompter-mic", "Microphone", &items(&d.mics), &mut p.mic);
        }
        kit::Slider::new("words a minute", &mut p.wpm, 60.0..=260.0).default(150.0).step(1.0).with_label().with_value().show(ui);
        ui.checkbox(&mut p.mirror, "Mirror (for a prompter glass)");
        if ui.button("Start prompter").clicked() {
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
                            p.message = e;
                        }
                    }
                }
                Err(e) => p.message = e,
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
        if ui.button("Play / pause").clicked() {
            let running = p.last_status.as_ref().and_then(|s| s["prompter"]["running"].as_bool()).unwrap_or(true);
            let _ = call(app, json!({"action": "prompter", "do": if running { "pause" } else { "play" }}));
        }
        if ui.button("Back").clicked() {
            let _ = call(app, json!({"action": "prompter", "do": "back"}));
        }
        if ui.checkbox(&mut p.mirror, "Mirror").changed() {
            let _ = call(app, json!({"action": "prompter", "do": "settings", "mirror": p.mirror}));
        }
        if kit::Slider::new("wpm", &mut p.wpm, 60.0..=260.0).default(150.0).step(1.0).with_label().with_value().show(ui).changed() {
            let _ = call(app, json!({"action": "prompter", "do": "settings", "wpm": p.wpm}));
        }
        if ui.button("Close").clicked() {
            crate::agent::ui_stop_follow();
            let _ = call(app, json!({"action": "prompter", "do": "unload"}));
            p.prompter_on = false;
        }
    });
    p.last_status = call(app, json!({"action": "status"})).ok();
}

fn tab_live(app: &mut StudioApp, ui: &mut egui::Ui, ctx: &egui::Context, p: &mut Panel) {
    let Some(d) = p.devices.clone() else { return };
    let Some(id) = p.live else {
        ui.label("Pick the cameras and screens to switch between, then open the mixer.");
        pick(ui, "live-cam", "Camera", &items(&d.cameras), &mut p.cam);
        let mut screens = items(&d.screens);
        screens.extend(items(&d.windows));
        pick(ui, "live-screen", "Screen or window", &screens, &mut p.screen);
        pick(ui, "live-mic", "Microphone", &items(&d.mics), &mut p.mic);
        if ui.add_enabled(p.cam.is_some() || p.screen.is_some(), egui::Button::new("Open the mixer")).clicked() {
            let mut sources = Vec::new();
            if let Some(c) = &p.cam {
                sources.push(json!({"name": "Camera", "video": c, "audio": p.mic}));
            }
            if let Some(s) = &p.screen {
                sources.push(json!({"name": "Screen", "video": s}));
            }
            match call(app, json!({"action": "live", "do": "open", "sources": sources, "user_approved": true, "realtime": true, "replay_secs": 30, "record_program": true, "iso": true, "width": 1280, "height": 720})) {
                Ok(v) => p.live = v["session"].as_u64().map(|x| x as u32),
                Err(e) => p.message = e,
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
    ui.horizontal_wrapped(|ui| {
        ui.label("Take");
        for (i, name) in sources.iter().enumerate() {
            if ui.button(name).clicked() {
                let t = TRANS[p.transition];
                let _ = call(app, json!({"action": "live", "do": "take", "session": id, "to": i, "transition": t}));
            }
        }
        egui::ComboBox::from_id_salt("live-trans").selected_text(TRANS[p.transition]).show_ui(ui, |ui| {
            crate::kit::note_list(ui);
            for (i, t) in TRANS.iter().enumerate() {
                ui.selectable_value(&mut p.transition, i, *t);
            }
        });
    });
    ui.horizontal_wrapped(|ui| {
        ui.add(egui::TextEdit::singleline(&mut p.title).hint_text("Name").desired_width(120.0));
        ui.add(egui::TextEdit::singleline(&mut p.sub).hint_text("Role").desired_width(120.0));
        if ui.button("Title on").clicked() {
            let _ = call(app, json!({"action": "live", "do": "title", "session": id, "kind": "lower", "text": p.title, "sub": p.sub}));
        }
        if ui.button("Off").clicked() {
            let _ = call(app, json!({"action": "live", "do": "title_off", "session": id}));
        }
    });
    ui.horizontal_wrapped(|ui| {
        ui.add(egui::TextEdit::singleline(&mut p.stream_url).password(true).hint_text("rtmp://… or srt://…").desired_width(220.0));
        if ui.button("Go live").clicked() {
            match call(app, json!({"action": "live", "do": "stream", "session": id, "url": p.stream_url, "user_approved": true})) {
                Ok(_) => p.message = "Streaming.".into(),
                Err(e) => p.message = e,
            }
        }
        if ui.button("Stop stream").clicked() {
            let _ = call(app, json!({"action": "live", "do": "stop_stream", "session": id}));
        }
    });
    if let Some(streams) = p.last_status.as_ref().and_then(|s| s["productions"].as_array()).and_then(|a| a.iter().find(|x| x["session"] == id)).and_then(|x| x["streams"].as_array()) {
        for s in streams {
            ui.label(format!("{}: {} ({} sent, {} dropped)", s["target"].as_str().unwrap_or(""), s["state"].as_str().unwrap_or(""), s["sent"], s["dropped"]));
        }
    }
    ui.horizontal_wrapped(|ui| {
        if ui.button("Save last 20 s").clicked() {
            match call(app, json!({"action": "live", "do": "replay_save", "session": id, "seconds": 20})) {
                Ok(v) => p.message = format!("Replay saved: {}", v["saved"].as_str().unwrap_or("")),
                Err(e) => p.message = e,
            }
        }
        if ui.button("Replay at half speed").clicked() {
            match call(app, json!({"action": "live", "do": "replay_play", "session": id, "seconds": 10, "speed": 0.5})) {
                Ok(_) => p.message = "Replay is a new source: take it to program.".into(),
                Err(e) => p.message = e,
            }
        }
        if ui.button("■  Close and add to timeline").clicked() {
            match call(app, json!({"action": "live", "do": "close", "session": id, "to_timeline": true, "at": app.editor.playhead as f64 / 1e6})) {
                Ok(v) => p.message = format!("{} file(s) added.", v["files"].as_array().map_or(0, Vec::len)),
                Err(e) => p.message = e,
            }
            p.live = None;
            p.live_tex = None;
        }
    });
}

fn tab_cards(app: &mut StudioApp, ui: &mut egui::Ui, p: &mut Panel) {
    if ui.button("Look for cards").clicked() {
        match call(app, json!({"action": "cards"})) {
            Ok(v) => {
                let first = v["cards"].as_array().and_then(|a| a.first()).cloned();
                p.message = format!("{} card(s) found.", v["cards"].as_array().map_or(0, Vec::len));
                if let Some(c) = first {
                    p.from = c["path"].as_str().unwrap_or("").to_owned();
                }
            }
            Err(e) => p.message = e,
        }
    }
    ui.label("From (card or folder)");
    ui.text_edit_singleline(&mut p.from);
    ui.label("Second copy on another drive");
    ui.text_edit_singleline(&mut p.backup);
    if ui.add_enabled(!p.from.is_empty() && !p.backup.is_empty(), egui::Button::new("Copy, check and add to project")).clicked() {
        match call(app, json!({"action": "ingest", "from": p.from, "backup": p.backup, "import": true})) {
            Ok(v) => p.message = format!("{} file(s), {} MB copied; {} verified{}", v["files"], v["megabytes"], v["verified"], if v["ok"] == true { ". Every copy matches the card." } else { ". PROBLEMS: see the AI editor's report; do not wipe the card." }),
            Err(e) => p.message = e,
        }
    }
}
