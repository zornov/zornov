// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `capture`: recording and live production for the agent.
//!
//! Cameras, microphones, screens and windows (with their permissions),
//! recording several inputs at once, voice-over straight onto the
//! timeline, the teleprompter, the vision mixer with titles, program and
//! ISO recording, RTMP / SRT streaming and the replay buffer, card ingest
//! with verified copies to two drives, and syncing separately recorded
//! sound. The engine is the `studio-live` crate; this turns its results
//! into timeline clips. Real devices open only when the person has asked
//! (`user_approved`) and the system has given access; `test:` sources
//! need neither, which is how everything here is tested.

use super::{Content, Session, f};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use studio_live::device;
use studio_live::ingest;
use studio_live::production::{LiveProduction, Production, ProductionSpec, SourceSpec};
use studio_live::prompter::{Mode, PromptLine, Prompter, Script};
use studio_live::record::{InputSpec, LiveRecording, RecordSpec, Recorder, demo_cursor};
use studio_live::source::{Consent, Kind, open_audio, parse_id};
use studio_live::switcher::{Corner, Layout, TitleKind, Transition};
use studio_live::vo::{LiveVo, VoSpec};

enum ProdMode {
    Sim(Box<Production>),
    Live(LiveProduction),
}

struct ProdSession {
    mode: ProdMode,
    dir: PathBuf,
    name: String,
}

struct VoSession {
    vo: LiveVo,
    start_s: f64,
    lane: String,
    dir: PathBuf,
}

struct RecSession {
    rec: LiveRecording,
    at: f64,
}

#[derive(Default)]
struct State {
    next: u32,
    recs: BTreeMap<u32, RecSession>,
    prods: BTreeMap<u32, ProdSession>,
    vos: BTreeMap<u32, VoSession>,
    prompter: Option<Prompter>,
}

fn state() -> &'static Mutex<State> {
    static S: OnceLock<Mutex<State>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(State { next: 1, ..Default::default() }))
}

fn lock() -> Result<std::sync::MutexGuard<'static, State>, String> {
    state().lock().map_err(|_| "the capture state is poisoned".to_string())
}

fn s_of<'a>(a: &'a Value, k: &str) -> Option<&'a str> {
    a.get(k).and_then(Value::as_str)
}

fn b_of(a: &Value, k: &str) -> bool {
    a.get(k).and_then(Value::as_bool).unwrap_or(false)
}

fn kind_of(id: &str) -> Kind {
    parse_id(id).map(|(k, _)| k).unwrap_or(Kind::Test)
}

fn is_test(id: &str) -> bool {
    kind_of(id) == Kind::Test
}

fn r3(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

const TEST_SOURCES: &str = "pictures: test:bars, test:counter (test:counter:3 for another colour; the frame number is coded along the top), test:ramp, test:red/green/blue/black/white/grey; sound: test:tone:440, test:voice (speech-like), test:noise:7, test:silence (add @0.3 for the level, >1.5 to start 1.5 s into it)";

impl Session {
    /// Ops that add the lanes in `wanted` (name, kind: overlay or sound) that the project does not have yet.
    fn lane_adds(&self, wanted: &[(String, &str)]) -> Vec<Value> {
        let have: Vec<String> = self.doc.project.lanes.iter().map(|l| l.name.to_lowercase()).collect();
        let mut seen: Vec<String> = Vec::new();
        let mut out = Vec::new();
        for (name, kind) in wanted {
            let k = name.to_lowercase();
            if have.contains(&k) || seen.contains(&k) {
                continue;
            }
            seen.push(k);
            out.push(json!({"op": "lane", "action": "add", "name": name, "kind": kind}));
        }
        out
    }

    fn rec_dir(&self, sub: &str) -> PathBuf {
        self.home.clone().or_else(|| self.doc.folder.as_ref().map(|f| f.root().to_path_buf())).map(|h| h.join(sub)).unwrap_or_else(|| std::env::temp_dir().join("studio-recordings").join(sub))
    }

    /// The `capture` tool.
    pub(super) fn capture(&mut self, a: &Value) -> Result<Vec<Content>, String> {
        let action = s_of(a, "action").ok_or("capture needs an \"action\": devices, permissions, record, stop, status, voiceover, prompter, live, ingest, cards, verify, sync")?;
        let json_out = |v: Value| Ok(vec![Content::Text(v.to_string())]);
        match action {
            "devices" => json_out(self.cap_devices()),
            "permissions" => json_out(self.cap_permissions(a)?),
            "record" => json_out(self.cap_record(a)?),
            "stop" => json_out(self.cap_stop(a)?),
            "status" => json_out(self.cap_status()),
            "voiceover" | "vo" => json_out(self.cap_voiceover(a)?),
            "prompter" => self.cap_prompter(a),
            "live" => self.cap_live(a),
            "cards" => json_out(json!({"cards": ingest::cards_in(Path::new("/Volumes"))})),
            "ingest" => json_out(self.cap_ingest(a)?),
            "verify" => {
                let dir = s_of(a, "dir").ok_or("verify needs \"dir\": a folder made by ingest")?;
                let (n, bad) = ingest::verify_manifest(Path::new(dir))?;
                json_out(json!({"checked": n, "problems": bad, "ok": bad.is_empty()}))
            }
            "sync" => json_out(self.cap_sync(a)?),
            other => Err(format!("unknown capture action \"{other}\"")),
        }
    }

    // ───────────────────────── devices and permissions ─────────────────────────

    fn cap_devices(&self) -> Value {
        let d = device::list();
        let acc = |k: Kind| json!(device::access(k));
        json!({
            "test_sources": TEST_SOURCES,
            "cameras": d.cameras, "microphones": d.mics, "screens": d.screens, "windows": d.windows,
            "access": {"camera": acc(Kind::Camera), "microphone": acc(Kind::Mic), "screen": acc(Kind::Screen)},
            "note": "Real devices open only when the person asks (user_approved: true) and the system has given access. Window titles show only once screen recording is allowed."
        })
    }

    fn cap_permissions(&self, a: &Value) -> Result<Value, String> {
        let status = |k: Kind| {
            let ac = device::access(k);
            json!({"access": ac, "means": if ac.ok() { "allowed".to_owned() } else { device::explain(k, ac) }})
        };
        if let Some(what) = s_of(a, "request") {
            let k = match what {
                "camera" => Kind::Camera,
                "microphone" | "mic" => Kind::Mic,
                "screen" => Kind::Screen,
                o => return Err(format!("cannot ask for \"{o}\": camera, microphone or screen")),
            };
            if !b_of(a, "user_approved") {
                return Err(format!("asking for {what} access shows a system dialog to the person. Only do it when they asked to use the {what} (then pass user_approved: true)"));
            }
            if std::env::var_os("STUDIO_HEADLESS_NO_PROMPT").is_some() {
                return Err("permission questions are switched off in this environment".into());
            }
            let r = device::request(k)?;
            return Ok(json!({"asked": what, "result": r, "means": if r.ok() { "allowed".to_owned() } else { device::explain(k, r) }, "note": if k == Kind::Screen { "Screen recording access may need Zornov to be restarted before it counts." } else { "" }}));
        }
        Ok(json!({"camera": status(Kind::Camera), "microphone": status(Kind::Mic), "screen": status(Kind::Screen), "how_to_ask": "capture {action: permissions, request: camera|microphone|screen, user_approved: true}, only when the person asked to use it"}))
    }

    // ───────────────────────── recording ─────────────────────────

    fn cap_record(&mut self, a: &Value) -> Result<Value, String> {
        let consent = Consent { user_action: b_of(a, "user_approved") };
        let (w, h) = (f(a, "width").unwrap_or(1280.0) as u32, f(a, "height").unwrap_or(720.0) as u32);
        let fps = f(a, "fps").unwrap_or(30.0).clamp(1.0, 120.0);
        let inputs_json: Vec<Value> = match a.get("inputs").and_then(Value::as_array) {
            Some(v) if !v.is_empty() => v.clone(),
            _ => vec![json!({"label": s_of(a, "label").unwrap_or("input"), "video": a.get("video"), "audio": a.get("audio"), "cursor": a.get("cursor")})],
        };
        let mut any_real = false;
        let mut inputs = Vec::new();
        for (i, j) in inputs_json.iter().enumerate() {
            let video = s_of(j, "video").map(str::to_owned);
            let audio = s_of(j, "audio").map(str::to_owned);
            if video.is_none() && audio.is_none() {
                return Err(format!("input {i} needs \"video\" and/or \"audio\" (a source id from capture devices, or a test: source)"));
            }
            any_real |= video.as_deref().is_some_and(|v| !is_test(v)) || audio.as_deref().is_some_and(|v| !is_test(v));
            let label = s_of(j, "label").map(str::to_owned).or_else(|| video.clone()).or_else(|| audio.clone()).unwrap_or_default();
            // The pointer highlight: on for screens and windows, or when asked.
            let want_cursor = j.get("cursor").and_then(Value::as_bool).unwrap_or(video.as_deref().is_some_and(|v| matches!(kind_of(v), Kind::Screen | Kind::Window)));
            let cursor = match (&video, want_cursor) {
                (Some(v), true) if is_test(v) => Some(demo_cursor(1.0)),
                (Some(v), true) => device::system_cursor(v).map(|c| studio_live::record::CursorSetup { source: c, style: Default::default() }),
                _ => None,
            };
            inputs.push(InputSpec { label, video, audio, cursor });
        }
        let name = s_of(a, "name").unwrap_or("take").to_owned();
        let dir = s_of(a, "dir").map(PathBuf::from).unwrap_or_else(|| self.rec_dir("Recordings"));
        let rec = Recorder::open(RecordSpec { name: name.clone(), dir, width: w, height: h, fps, inputs, consent })?;
        let at = f(a, "at").unwrap_or(0.0);
        match f(a, "seconds") {
            // Test sources run on a simulated clock (a minute in a moment);
            // real devices (or realtime: true) run on the wall clock.
            Some(secs) if !any_real && !b_of(a, "realtime") => {
                let mut rec = rec;
                rec.run_for(secs.clamp(0.1, 3600.0))?;
                let levels = rec.progress().levels;
                let files = rec.finish()?;
                let mut out = json!({"recorded": files, "seconds": secs, "levels": levels, "clock": "simulated (test sources)"});
                if b_of(a, "to_timeline") || b_of(a, "import") {
                    out["timeline"] = self.cap_bring_in(&files, at, b_of(a, "to_timeline"))?;
                }
                Ok(out)
            }
            Some(secs) => {
                let live = LiveRecording::start(rec);
                std::thread::sleep(std::time::Duration::from_secs_f64(secs.clamp(0.1, 3600.0)));
                let levels = live.progress().levels;
                let files = live.stop()?;
                let mut out = json!({"recorded": files, "seconds": secs, "levels": levels, "clock": "real time"});
                if b_of(a, "to_timeline") || b_of(a, "import") {
                    out["timeline"] = self.cap_bring_in(&files, at, b_of(a, "to_timeline"))?;
                }
                Ok(out)
            }
            None => {
                let mut st = lock()?;
                let id = st.next;
                st.next += 1;
                st.recs.insert(id, RecSession { rec: LiveRecording::start(rec), at });
                Ok(json!({"session": id, "recording": true, "note": "Recording on the wall clock. capture {action: status} shows levels; capture {action: stop, session} ends it (to_timeline: true brings the files onto the timeline)."}))
            }
        }
    }

    fn cap_status(&self) -> Value {
        let Ok(st) = lock() else { return json!({"error": "state unavailable"}) };
        let recs: Vec<Value> = st.recs.iter().map(|(id, r)| json!({"session": id, "kind": "recording", "progress": r.rec.progress()})).collect();
        let vos: Vec<Value> = st
            .vos
            .iter()
            .map(|(id, v)| {
                let (state, meter, secs) = v.vo.status();
                json!({"session": id, "kind": "voiceover", "state": format!("{state:?}"), "seconds_recorded": r3(secs), "peak_db": meter.peak_db, "hold_db": meter.hold_db, "clipped": meter.clipped})
            })
            .collect();
        let prods: Vec<Value> = st
            .prods
            .iter()
            .map(|(id, p)| match &p.mode {
                ProdMode::Sim(pr) => json!({"session": id, "kind": "live production (simulated clock)", "seconds": r3(pr.seconds()), "state": pr.sw.state(), "streams": pr.stream_status()}),
                ProdMode::Live(lp) => lp.with(|pr| json!({"session": id, "kind": "live production (real time)", "seconds": r3(pr.seconds()), "behind_ms": pr.behind_ms, "state": pr.sw.state(), "streams": pr.stream_status()})).unwrap_or(json!({"session": id, "ended": true})),
            })
            .collect();
        json!({"recordings": recs, "voiceovers": vos, "productions": prods, "prompter": st.prompter.as_ref().map(|p| p.status())})
    }

    fn cap_stop(&mut self, a: &Value) -> Result<Value, String> {
        let id = f(a, "session").ok_or("stop needs \"session\"")? as u32;
        let mut st = lock()?;
        if let Some(r) = st.recs.remove(&id) {
            drop(st);
            let files = r.rec.stop()?;
            let mut out = json!({"recorded": files});
            if b_of(a, "to_timeline") || b_of(a, "import") {
                out["timeline"] = self.cap_bring_in(&files, f(a, "at").unwrap_or(r.at), b_of(a, "to_timeline"))?;
            }
            return Ok(out);
        }
        if let Some(v) = st.vos.remove(&id) {
            drop(st);
            return self.cap_finish_vo(v, a);
        }
        if st.prods.contains_key(&id) {
            drop(st);
            return self.cap_live_close(id, a);
        }
        Err(format!("no capture session {id} (capture status lists them)"))
    }

    /// Recorded files into the project: imported, and optionally placed.
    fn cap_bring_in(&mut self, files: &[studio_live::record::FileInfo], at: f64, place: bool) -> Result<Value, String> {
        let paths: Vec<PathBuf> = files.iter().map(|f| PathBuf::from(&f.path)).collect();
        let report = crate::services::project::import_media(&mut self.doc, &paths);
        let mut clips = Vec::new();
        if place {
            // A lone picture recorded to the start goes on the main lane (which keeps
            // clips end to end); anything else, or a given time, gets a lane of its own.
            let videos = files.iter().filter(|f| f.kind == "video").count();
            let main_ok = videos == 1 && at.abs() < 1e-9;
            let target = |f: &studio_live::record::FileInfo| -> (String, &'static str) {
                if f.kind == "audio" { (format!("audio {}", f.label), "sound") } else if main_ok { ("Main".to_owned(), "overlay") } else { (f.label.clone(), "overlay") }
            };
            let wanted: Vec<(String, &str)> = files.iter().map(|f| target(f)).collect();
            let mut ops = self.lane_adds(&wanted);
            for ((file, media), (lane, _)) in files.iter().zip(&report.ids).zip(&wanted) {
                let _ = file;
                ops.push(json!({"op": "place", "media": media, "at": at, "lane": lane}));
            }
            let before: std::collections::BTreeSet<u32> = self.doc.project.clips.iter().map(|c| c.id).collect();
            self.edit(&ops, "Place recordings", false)?;
            clips.extend(self.doc.project.clips.iter().map(|c| c.id).filter(|id| !before.contains(id)));
        }
        Ok(json!({"media": report.ids, "failed": report.failed, "clips": clips, "at": at}))
    }

    // ───────────────────────── voice-over ─────────────────────────

    fn vo_spec(&self, a: &Value) -> VoSpec {
        let d = VoSpec::default();
        VoSpec {
            start_s: f(a, "at").unwrap_or(0.0).max(0.0),
            count_in_beats: f(a, "count_in").map(|b| b as u32).unwrap_or(d.count_in_beats).min(8),
            bpm: f(a, "bpm").map(|b| b as f32).unwrap_or(d.bpm).clamp(30.0, 240.0),
            gain: f(a, "gain").map(|g| g as f32).unwrap_or(d.gain).clamp(0.0, 2.0),
            trim_db: if a.get("trim").and_then(Value::as_bool) == Some(false) { None } else { d.trim_db },
            latency_ms: f(a, "latency_ms").unwrap_or(0.0).clamp(0.0, 1000.0),
        }
    }

    fn cap_voiceover(&mut self, a: &Value) -> Result<Value, String> {
        let consent = Consent { user_action: b_of(a, "user_approved") };
        let mic_id = s_of(a, "mic").unwrap_or("test:voice");
        let spec = self.vo_spec(a);
        let lane = s_of(a, "lane").unwrap_or("voice-over").to_owned();
        let dir = self.rec_dir("Voice-over");
        match f(a, "seconds") {
            Some(secs) if is_test(mic_id) && !b_of(a, "realtime") => {
                let mut mic = open_audio(mic_id, 48_000, consent)?;
                let r = studio_live::vo::record(&mut *mic, &spec, secs.clamp(0.1, 3600.0), &mut |_| {})?;
                self.cap_place_take(r, &dir, &lane, a)
            }
            Some(secs) => {
                let mic = open_audio(mic_id, 48_000, consent)?;
                let vo = LiveVo::start(mic, spec.clone(), click_sink(b_of(a, "click")));
                std::thread::sleep(std::time::Duration::from_secs_f64(secs.clamp(0.1, 3600.0) + count_in_len(&spec)));
                let r = vo.stop()?;
                self.cap_place_take(r, &dir, &lane, a)
            }
            None => {
                let mic = open_audio(mic_id, 48_000, consent)?;
                let vo = LiveVo::start(mic, spec.clone(), click_sink(b_of(a, "click")));
                let mut st = lock()?;
                let id = st.next;
                st.next += 1;
                st.vos.insert(id, VoSession { vo, start_s: spec.start_s, lane, dir });
                Ok(json!({"session": id, "counting_in_beats": spec.count_in_beats, "take_starts_at": spec.start_s, "note": "Recording the microphone. capture {action: status} shows the level; capture {action: stop, session} ends the take and places it."}))
            }
        }
    }

    fn cap_finish_vo(&mut self, v: VoSession, a: &Value) -> Result<Value, String> {
        let r = v.vo.stop()?;
        let mut a2 = a.clone();
        a2["at"] = json!(v.start_s);
        self.cap_place_take(r, &v.dir, &v.lane, &a2)
    }

    /// The take written to the project folder and placed where it began.
    fn cap_place_take(&mut self, r: studio_live::vo::VoResult, dir: &Path, lane: &str, a: &Value) -> Result<Value, String> {
        std::fs::create_dir_all(dir).map_err(|e| format!("could not make {}: {e}", dir.display()))?;
        let n = (1..).find(|n| !dir.join(format!("vo-{n:03}.wav")).exists()).unwrap_or(1);
        let path = dir.join(format!("vo-{n:03}.wav"));
        let mut w = studio_live::wavout::WavWriter::create(&path, r.take.rate).map_err(|e| e.to_string())?;
        w.write(&r.take.samples).map_err(|e| e.to_string())?;
        w.finish().map_err(|e| e.to_string())?;
        let report = crate::services::project::import_media(&mut self.doc, std::slice::from_ref(&path));
        let media = *report.ids.first().ok_or("the take could not be imported")?;
        let start = r.take.start as f64 / 1e6;
        let before: std::collections::BTreeSet<u32> = self.doc.project.clips.iter().map(|c| c.id).collect();
        let mut place = json!({"op": "place", "media": media, "at": start, "lane": lane});
        if let Some(v) = f(a, "volume") {
            place["volume"] = json!(v);
        }
        let mut ops = self.lane_adds(&[(lane.to_owned(), "sound")]);
        ops.push(place);
        self.edit(&ops, "Place voice-over", false)?;
        let clip = self.doc.project.clips.iter().map(|c| c.id).find(|id| !before.contains(id));
        Ok(json!({
            "clip": clip, "media": media, "file": path.display().to_string(), "lane": lane,
            "starts_at": r3(start), "seconds": r3(r.take.duration as f64 / 1e6),
            "count_in_seconds": r3(r.count_in_s), "peak_db": (r.peak_db * 10.0).round() / 10.0, "clipped": r.clipped,
            "advice": if r.clipped { "The take clipped: record again with the microphone further away or gain lower." } else if r.peak_db < -30.0 { "The take is quiet (peak under -30 dB): record again closer, or with more gain." } else { "" }
        }))
    }

    // ───────────────────────── teleprompter ─────────────────────────

    fn prompt_lines_from_story(&self, scene: Option<&str>) -> Result<Vec<PromptLine>, String> {
        // Read through the saved form, so the prompter follows whatever the
        // story model holds (scenes with lines, a cast with names).
        let project = serde_json::to_value(&self.doc.model).map_err(|e| e.to_string())?;
        let story = project.get("story").cloned().unwrap_or(Value::Null);
        let scenes: Vec<Value> = story.get("scenes").and_then(Value::as_array).cloned().unwrap_or_default();
        let name_of = |id: &str| -> String { story.get("cast").and_then(Value::as_array).and_then(|c| c.iter().find(|c| c["id"] == id)).and_then(|c| c["name"].as_str()).map(str::to_owned).unwrap_or_else(|| id.to_owned()) };
        let chosen: Vec<&Value> = match scene {
            None | Some("all") => scenes.iter().collect(),
            Some(id) => vec![scenes.iter().find(|s| s["id"] == id).ok_or_else(|| format!("no scene \"{id}\" in the story (ids: {})", scenes.iter().filter_map(|s| s["id"].as_str()).collect::<Vec<_>>().join(", ")))?],
        };
        let mut out = Vec::new();
        for s in chosen {
            for l in s.get("lines").and_then(Value::as_array).into_iter().flatten() {
                let text = l["text"].as_str().unwrap_or("").to_owned();
                let kind = l["kind"].as_str().unwrap_or("action");
                let spoken = kind == "dialogue" || kind == "narration";
                out.push(PromptLine { who: l["who"].as_str().filter(|_| spoken).map(name_of), text, action: !spoken });
            }
        }
        if out.iter().all(|l| l.text.trim().is_empty()) {
            return Err("the story has no lines to read (the script is empty); give \"text\" instead".into());
        }
        Ok(out)
    }

    fn cap_prompter(&mut self, a: &Value) -> Result<Vec<Content>, String> {
        let op = s_of(a, "do").unwrap_or("status");
        let mut st = lock()?;
        let need = |st: &mut State| -> Result<(), String> { if st.prompter.is_none() { Err("no script loaded: capture {action: prompter, do: load, text | scene}".into()) } else { Ok(()) } };
        match op {
            "load" => {
                let script = if let Some(t) = s_of(a, "text") {
                    Script::from_text(t)
                } else {
                    drop(st);
                    let lines = self.prompt_lines_from_story(s_of(a, "scene"))?;
                    st = lock()?;
                    Script::from_lines(&lines)
                };
                if script.is_empty() {
                    return Err("the script is empty".into());
                }
                let wpm = f(a, "wpm").unwrap_or(150.0).clamp(40.0, 400.0);
                let mode = match s_of(a, "mode").unwrap_or("pace") {
                    "pace" => Mode::Pace(wpm),
                    "follow" | "voice" => Mode::Follow,
                    "manual" => Mode::Manual,
                    o => return Err(format!("mode \"{o}\": pace, follow or manual")),
                };
                studio_live::text::warm();
                let mut p = Prompter::new(script, mode);
                p.view.mirror = b_of(a, "mirror");
                if let Some(sz) = f(a, "size") {
                    p.view.size = sz as f32;
                }
                let status = p.status();
                st.prompter = Some(p);
                text_out(json!({"loaded": true, "words": status.of, "mode": s_of(a, "mode").unwrap_or("pace"), "reading_time_s": r3(status.seconds_left)}))
            }
            "play" | "start" => {
                need(&mut st)?;
                st.prompter.as_mut().unwrap().start();
                text_out(json!(st.prompter.as_ref().unwrap().status()))
            }
            "pause" | "stop" => {
                need(&mut st)?;
                st.prompter.as_mut().unwrap().pause();
                text_out(json!(st.prompter.as_ref().unwrap().status()))
            }
            "advance" | "tick" => {
                need(&mut st)?;
                let secs = f(a, "seconds").unwrap_or(1.0).clamp(0.0, 3600.0);
                let p = st.prompter.as_mut().unwrap();
                for _ in 0..(secs * 50.0).round() as u32 {
                    p.tick(0.02);
                }
                text_out(json!(p.status()))
            }
            "hear" => {
                need(&mut st)?;
                let words: Vec<String> = match (a.get("words").and_then(Value::as_array), s_of(a, "text")) {
                    (Some(w), _) => w.iter().filter_map(Value::as_str).map(str::to_owned).collect(),
                    (_, Some(t)) => t.split_whitespace().map(str::to_owned).collect(),
                    _ => return Err("hear needs \"words\" or \"text\": what the live transcription heard".into()),
                };
                let refs: Vec<&str> = words.iter().map(String::as_str).collect();
                let p = st.prompter.as_mut().unwrap();
                p.hear(&refs);
                text_out(json!(p.status()))
            }
            "jump" => {
                need(&mut st)?;
                let p = st.prompter.as_mut().unwrap();
                p.jump(f(a, "word").unwrap_or(0.0) as usize);
                text_out(json!(p.status()))
            }
            "back" => {
                need(&mut st)?;
                let p = st.prompter.as_mut().unwrap();
                p.back_paragraph();
                text_out(json!(p.status()))
            }
            "settings" => {
                need(&mut st)?;
                let p = st.prompter.as_mut().unwrap();
                if let Some(m) = a.get("mirror").and_then(Value::as_bool) {
                    p.view.mirror = m;
                }
                if let Some(sz) = f(a, "size") {
                    p.view.size = sz.clamp(12.0, 400.0) as f32;
                }
                if let Some(w) = f(a, "wpm") {
                    p.mode = Mode::Pace(w.clamp(40.0, 400.0));
                }
                text_out(json!({"mirror": p.view.mirror, "size": p.view.size, "status": p.status()}))
            }
            "frame" => {
                need(&mut st)?;
                let (w, h) = (f(a, "width").unwrap_or(960.0).clamp(160.0, 3840.0) as u32, f(a, "height").unwrap_or(540.0).clamp(90.0, 2160.0) as u32);
                st.prompter.as_mut().unwrap().settle();
                let p = st.prompter.as_ref().unwrap();
                let fr = p.render(w, h);
                let status = p.status();
                Ok(vec![Content::Jpeg(jpeg_of(&fr)), Content::Text(json!({"prompter": status, "mirrored": p.view.mirror}).to_string())])
            }
            "listen" => {
                need(&mut st)?;
                drop(st);
                self.cap_prompter_listen(a)
            }
            "status" => text_out(json!(st.prompter.as_ref().map(|p| p.status()))),
            "unload" => {
                st.prompter = None;
                text_out(json!({"unloaded": true}))
            }
            o => Err(format!("prompter do \"{o}\": load, play, pause, advance, hear, jump, back, settings, frame, status, unload")),
        }
    }

    /// Voice-follow: sound comes in (a media file played as if live, or the
    /// microphone), a rolling window of it is transcribed, and the new words
    /// move the prompter. Needs the speech model (Settings, AI models).
    fn cap_prompter_listen(&mut self, a: &Value) -> Result<Vec<Content>, String> {
        let consent = Consent { user_action: b_of(a, "user_approved") };
        let source: Box<dyn FnMut(usize) -> Vec<f32> + Send> = if let Some(m) = a.get("media") {
            let mi = self.media_item(m)?;
            let path = mi.path.clone().ok_or("that media has no file")?;
            let d = crate::sound_bridge::decoded(&path.to_string_lossy()).ok_or("that media has no sound Zornov can decode")?;
            let mono: Vec<f32> = d.left.iter().zip(d.right.iter()).map(|(l, r)| 0.5 * (l + r)).collect();
            let mono = if d.rate == 16_000 { mono } else { studio_live::sync::resample(&mono, d.rate, 16_000) };
            let mut pos = 0usize;
            Box::new(move |n| {
                let e = (pos + n).min(mono.len());
                let out = mono[pos.min(e)..e].to_vec();
                pos = e;
                out
            })
        } else {
            let mic = s_of(a, "mic").ok_or("listen needs \"media\" (a recording played as if live) or \"mic\"")?;
            let mut m = open_audio(mic, 48_000, consent)?;
            Box::new(move |n| {
                // Wait for the wall clock, then bring it to 16 kHz.
                std::thread::sleep(std::time::Duration::from_secs_f64(n as f64 / 16_000.0));
                let b = m.read(n * 3);
                studio_live::sync::resample(&b, 48_000, 16_000)
            })
        };
        let (fed, t) = follow_loop(source, f(a, "seconds").unwrap_or(f64::INFINITY), None)?;
        let st = lock()?;
        text_out(json!({"heard_words": fed, "listened_seconds": r3(t), "prompter": st.prompter.as_ref().map(|p| p.status())}))
    }

    // ───────────────────────── live production ─────────────────────────

    fn cap_live(&mut self, a: &Value) -> Result<Vec<Content>, String> {
        let op = s_of(a, "do").ok_or("live needs \"do\": open, take, cut, preview, scene, title, title_off, mute, gain, state, frame, multiview, run, stream, stop_stream, replay_save, replay_play, close")?;
        if op == "open" {
            return text_out(self.cap_live_open(a)?);
        }
        let id = f(a, "session").ok_or("live needs \"session\" (from live open)")? as u32;
        if op == "close" {
            let v = self.cap_live_close(id, a)?;
            return text_out(v);
        }
        let mut st = lock()?;
        let sess = st.prods.get_mut(&id).ok_or_else(|| format!("no live session {id}"))?;
        let dir = sess.dir.clone();
        let name = sess.name.clone();
        let call = |pr: &mut Production| -> Result<Vec<Content>, String> { live_command(pr, op, a, &dir, &name) };
        match &mut sess.mode {
            ProdMode::Sim(pr) => call(pr),
            ProdMode::Live(lp) => lp.with(|pr| call(pr))?,
        }
    }

    fn cap_live_open(&mut self, a: &Value) -> Result<Value, String> {
        let consent = Consent { user_action: b_of(a, "user_approved") };
        let srcs = a.get("sources").and_then(Value::as_array).ok_or("live open needs \"sources\": [{name, video, audio?}…]")?;
        let mut sources = Vec::new();
        let mut any_real = false;
        for (i, s) in srcs.iter().enumerate() {
            let video = s_of(s, "video").ok_or_else(|| format!("source {i} needs \"video\""))?.to_owned();
            let audio = s_of(s, "audio").map(str::to_owned);
            any_real |= !is_test(&video) || audio.as_deref().is_some_and(|x| !is_test(x));
            sources.push(SourceSpec { name: s_of(s, "name").map(str::to_owned).unwrap_or_else(|| format!("cam {}", i + 1)), video, audio });
        }
        let (w, h) = (f(a, "width").unwrap_or(1280.0) as u32, f(a, "height").unwrap_or(720.0) as u32);
        let fps = f(a, "fps").unwrap_or(30.0).clamp(1.0, 60.0);
        let mut pr = Production::open(ProductionSpec { width: w, height: h, fps, sources, consent, replay_secs: f(a, "replay_secs").unwrap_or(0.0).clamp(0.0, 300.0) })?;
        let name = s_of(a, "name").unwrap_or("show").to_owned();
        let dir = s_of(a, "dir").map(PathBuf::from).unwrap_or_else(|| self.rec_dir("Recordings"));
        let mut made = Vec::new();
        if b_of(a, "record_program") {
            made.push(pr.record_program(&dir, &name)?.display().to_string());
        }
        if b_of(a, "iso") {
            made.extend(pr.record_isos(&dir, &name)?.into_iter().map(|p| p.display().to_string()));
        }
        let urls: Vec<String> = match a.get("stream") {
            Some(Value::String(u)) => vec![u.clone()],
            Some(Value::Array(v)) => v.iter().filter_map(Value::as_str).map(str::to_owned).collect(),
            _ => Vec::new(),
        };
        if !urls.is_empty() && !b_of(a, "user_approved") {
            return Err("streaming sends the program to the internet: only when the person asked for it (user_approved: true)".into());
        }
        for u in &urls {
            pr.stream_to(u)?;
        }
        let realtime = a.get("realtime").and_then(Value::as_bool).unwrap_or(any_real || !urls.is_empty());
        let state = pr.sw.state();
        let mode = if realtime { ProdMode::Live(LiveProduction::start(pr)) } else { ProdMode::Sim(Box::new(pr)) };
        let mut st = lock()?;
        let id = st.next;
        st.next += 1;
        st.prods.insert(id, ProdSession { mode, dir, name });
        Ok(json!({"session": id, "clock": if realtime { "real time" } else { "simulated: advance it with do: run, seconds" }, "writing": made, "sources": state.sources, "next": "do: take / cut / title / run … then do: close"}))
    }

    fn cap_live_close(&mut self, id: u32, a: &Value) -> Result<Value, String> {
        let sess = lock()?.prods.remove(&id).ok_or_else(|| format!("no live session {id}"))?;
        let files = match sess.mode {
            ProdMode::Sim(pr) => pr.finish()?,
            ProdMode::Live(lp) => lp.stop()?,
        };
        let mut out = json!({"files": files});
        if b_of(a, "import") || b_of(a, "to_timeline") {
            let paths: Vec<PathBuf> = files.iter().map(|f| PathBuf::from(&f.path)).collect();
            let report = crate::services::project::import_media(&mut self.doc, &paths);
            let mut clips = Vec::new();
            if b_of(a, "to_timeline") {
                let lanes: Vec<(String, &str)> = files.iter().map(|file| (match &file.source { Some(s) => format!("iso {s}"), None => "program".to_owned() }, "overlay")).collect();
                let mut ops = self.lane_adds(&lanes);
                for ((_, media), (lane, _)) in files.iter().zip(&report.ids).zip(&lanes) {
                    ops.push(json!({"op": "place", "media": media, "at": f(a, "at").unwrap_or(0.0), "lane": lane}));
                }
                let before: std::collections::BTreeSet<u32> = self.doc.project.clips.iter().map(|c| c.id).collect();
                self.edit(&ops, "Place live recordings", false)?;
                clips.extend(self.doc.project.clips.iter().map(|c| c.id).filter(|i| !before.contains(i)));
            }
            out["timeline"] = json!({"media": report.ids, "clips": clips});
        }
        Ok(out)
    }

    // ───────────────────────── ingest and sync ─────────────────────────

    fn cap_ingest(&mut self, a: &Value) -> Result<Value, String> {
        let from = s_of(a, "from").ok_or("ingest needs \"from\": the card or folder (capture cards lists mounted cards)")?;
        let to = s_of(a, "to").map(PathBuf::from).unwrap_or_else(|| self.rec_dir("Footage"));
        let backups: Vec<PathBuf> = match a.get("backup") {
            Some(Value::String(s)) => vec![PathBuf::from(s)],
            Some(Value::Array(v)) => v.iter().filter_map(Value::as_str).map(PathBuf::from).collect(),
            _ => Vec::new(),
        };
        if backups.is_empty() && !b_of(a, "no_backup") {
            return Err("ingest wants a second copy on another drive: give \"backup\": a folder on a different drive (or \"no_backup\": true if the person says they do not want one)".into());
        }
        let plan = ingest::Plan { from: PathBuf::from(from), primary: to.clone(), backups, name: s_of(a, "name").map(str::to_owned), allow_same_drive: b_of(a, "allow_same_drive") };
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let rep = ingest::ingest(&plan, &cancel, &mut |_, _, _| {})?;
        let mut out = json!({
            "folder": rep.folder, "files": rep.files.len(), "megabytes": rep.bytes / (1 << 20), "verified": rep.verified,
            "ok": rep.ok(), "problems": rep.problems, "manifests": rep.manifests,
            "copies": rep.files.iter().map(|f| json!({"file": f.rel, "sha256": &f.sha256[..16], "kind": f.kind, "status": f.copies.iter().map(|c| c.status.as_str()).collect::<Vec<_>>()})).collect::<Vec<_>>(),
        });
        if b_of(a, "import") && rep.ok() {
            let paths: Vec<PathBuf> = rep.files.iter().filter(|f| f.kind == "video" || f.kind == "audio" || f.kind == "still").filter_map(|f| f.copies.first().map(|c| PathBuf::from(&c.path))).collect();
            let r = crate::services::project::import_media(&mut self.doc, &paths);
            out["imported"] = json!({"media": r.ids, "failed": r.failed});
        }
        Ok(out)
    }

    /// Lines separately recorded sound up with a camera clip.
    fn cap_sync(&mut self, a: &Value) -> Result<Value, String> {
        let p = self.doc.project.clone();
        let video_v = a.get("video").or_else(|| a.get("clip")).ok_or("sync needs \"video\": the camera clip (or its media) whose own sound is the guide")?;
        let audio_v = a.get("audio").ok_or("sync needs \"audio\": the separately recorded sound (a media id or name)")?;
        // The clip (if one was named) fixes where the camera media sits on the timeline.
        let clip = video_v.as_u64().and_then(|id| p.clip(id as u32)).cloned();
        let cam_media = match &clip {
            Some(c) => c.media.ok_or("that clip has no media")?,
            None => super::craft::media_of(&p, video_v)?,
        };
        let aud_media = super::craft::media_of(&p, audio_v)?;
        let load = |m: u32| -> Result<(Vec<f32>, u32), String> {
            let mi = p.media(m).ok_or("that media is gone")?;
            let path = mi.path.clone().ok_or("that media has no file")?;
            let d = crate::sound_bridge::decoded(&path.to_string_lossy()).ok_or_else(|| format!("{} has no sound Zornov can decode", mi.name))?;
            let mono: Vec<f32> = d.left.iter().zip(d.right.iter()).map(|(l, r)| 0.5 * (l + r)).collect();
            Ok((mono, d.rate))
        };
        let ((cam, cr), (ext, er)) = (load(cam_media)?, load(aud_media)?);
        let r = studio_live::sync::align(&cam, cr, &ext, er)?;
        let mut out = json!({"offset_s": r3(r.offset_s), "confidence": (r.confidence * 10.0).round() / 10.0, "drift_ppm": r.drift_ppm.map(|d| (d * 10.0).round() / 10.0), "meaning": "the recorder's file starts offset_s into the camera's media (negative: it started first)"});
        if r.confidence < 8.0 {
            out["warning"] = json!("low confidence: the two recordings may not share any sound (a clap or slate helps). Nothing was placed");
            return Ok(out);
        }
        if b_of(a, "apply") || b_of(a, "place") {
            // Where on the timeline the recorder's first sample belongs.
            let (cam_start, cam_src_in) = clip.as_ref().map(|c| (c.start as f64 / crate::view::SEC as f64, c.src_in as f64 / crate::view::SEC as f64)).unwrap_or((0.0, 0.0));
            let at = cam_start + r.offset_s - cam_src_in;
            let (at, source_in) = if at < 0.0 { (0.0, -at) } else { (at, 0.0) };
            let before: std::collections::BTreeSet<u32> = p.clips.iter().map(|c| c.id).collect();
            let mut place = json!({"op": "place", "media": aud_media, "at": at, "lane": s_of(a, "lane").unwrap_or("synced audio")});
            if source_in > 0.0 {
                place["source_in"] = json!(source_in);
            }
            if let Some(c) = &clip {
                // Only as long as the camera clip it belongs to.
                place["duration"] = json!(c.dur as f64 / crate::view::SEC as f64);
            }
            let lane_name = s_of(a, "lane").unwrap_or("synced audio").to_owned();
            let mut ops = self.lane_adds(&[(lane_name, "sound")]);
            ops.push(place);
            self.edit(&ops, "Place synced sound", false)?;
            let placed = self.doc.project.clips.iter().map(|c| c.id).find(|i| !before.contains(i));
            out["placed_clip"] = json!(placed);
            if let (Some(c), true) = (&clip, a.get("mute_camera").and_then(Value::as_bool).unwrap_or(true)) {
                self.edit(&[json!({"op": "set", "clips": [c.id], "muted": true})], "Mute camera sound", false)?;
                out["muted_camera_sound_of"] = json!(c.id);
            }
            if let (Some(c), Some(p)) = (&clip, placed) {
                let _ = self.edit(&[json!({"op": "link", "clips": [c.id, p]})], "Link synced sound", false);
            }
        }
        Ok(out)
    }
}

/// The words in `samples`, with their times in seconds from the start (the
/// speech model; none when it is not installed).
fn words_of(samples: &[f32], rate: u32) -> Vec<Said> {
    let store = studio_ai::store::ModelStore::default();
    let Ok(w) = studio_ai::features::transcribe::Whisper::load_best(&store) else { return Vec::new() };
    let opts = studio_ai::features::transcribe::TranscribeOptions { language: Some("en".into()), ..Default::default() };
    match w.transcribe(samples, rate, &opts, &studio_ai::jobs::JobCtx::detached()) {
        Ok(t) => t.words().map(|w| Said { text: w.text.trim().to_owned(), end: w.end }).filter(|w| !w.text.is_empty()).collect(),
        Err(_) => Vec::new(),
    }
}

struct Said {
    text: String,
    end: f64,
}

fn text_out(v: Value) -> Result<Vec<Content>, String> {
    Ok(vec![Content::Text(v.to_string())])
}

fn jpeg_of(f: &studio_live::frame::Frame) -> Vec<u8> {
    studio_content::jpeg::encode(&studio_media::RgbaImage { width: f.w, height: f.h, pixels: f.rgba.clone() }, 82)
}

fn count_in_len(s: &VoSpec) -> f64 {
    s.count_in_beats as f64 * 60.0 / s.bpm as f64
}

/// Count-in clicks to the speakers when asked for.
fn click_sink(on: bool) -> Box<dyn FnMut(&[f32]) + Send> {
    if !on {
        return Box::new(|_| {});
    }
    let out = studio_audio::AudioOut::open().ok();
    Box::new(move |c| {
        if let Some(o) = &out {
            o.push(c);
        }
    })
}

fn layout_of(pr: &Production, v: &Value) -> Result<Layout, String> {
    let sw = &pr.sw;
    let src = |x: &Value| -> Result<usize, String> {
        match x {
            Value::Number(n) => Ok(n.as_u64().unwrap_or(0) as usize),
            Value::String(s) => sw.find(s),
            _ => Err("a source is a number or a name".into()),
        }
    };
    let l = match v {
        Value::String(s) if s == "black" => Layout::Black,
        Value::String(s) => match sw.scene(s) {
            Ok(l) => l,
            Err(_) => Layout::Full(sw.find(s)?),
        },
        Value::Number(_) => Layout::Full(src(v)?),
        Value::Object(o) => {
            if let Some(p) = o.get("pip") {
                let corner = match p.get("corner").and_then(Value::as_str).unwrap_or("bottom_right") {
                    "top_left" => Corner::TopLeft,
                    "top_right" => Corner::TopRight,
                    "bottom_left" => Corner::BottomLeft,
                    _ => Corner::BottomRight,
                };
                Layout::Pip { main: src(p.get("main").ok_or("pip needs main")?)?, inset: src(p.get("inset").ok_or("pip needs inset")?)?, corner, scale: p.get("scale").and_then(Value::as_f64).unwrap_or(0.28) as f32 }
            } else if let Some(s) = o.get("split").and_then(Value::as_array) {
                if s.len() != 2 {
                    return Err("split takes two sources".into());
                }
                Layout::Split { left: src(&s[0])?, right: src(&s[1])? }
            } else if let Some(g) = o.get("grid").and_then(Value::as_array) {
                Layout::Grid(g.iter().map(src).collect::<Result<_, _>>()?)
            } else if let Some(x) = o.get("full") {
                Layout::Full(src(x)?)
            } else {
                return Err("a layout is a source (name or number), \"black\", a saved scene, or {pip:{main,inset,corner,scale}} / {split:[a,b]} / {grid:[…]}".into());
            }
        }
        _ => return Err("a layout is a source (name or number), \"black\", a saved scene, or an object".into()),
    };
    pr.sw.check(&l)?;
    Ok(l)
}

/// One command to a running production.
fn live_command(pr: &mut Production, op: &str, a: &Value, dir: &Path, name: &str) -> Result<Vec<Content>, String> {
    let ms = f(a, "ms").map(|m| m as i64);
    let trans = |default: &str| Transition::parse(s_of(a, "transition").unwrap_or(default));
    let ok = |extra: Value| -> Result<Vec<Content>, String> {
        let mut v = json!({"ok": true});
        if let (Value::Object(m), Value::Object(e)) = (&mut v, extra) {
            m.extend(e);
        }
        text_out(v)
    };
    match op {
        "state" => text_out(json!({"seconds": r3(pr.seconds()), "state": pr.sw.state(), "streams": pr.stream_status()})),
        "cut" => {
            let l = layout_of(pr, a.get("to").ok_or("cut needs \"to\": a source or layout")?)?;
            pr.sw.cut_to(l)?;
            ok(json!({"program": pr.sw.program()}))
        }
        "preview" => {
            let l = layout_of(pr, a.get("to").ok_or("preview needs \"to\"")?)?;
            pr.sw.set_preview(l)?;
            ok(json!({"preview": pr.sw.state().preview}))
        }
        "take" => {
            // With "to": straight to that layout; without: the preview goes to program.
            let t = trans("mix")?;
            let dur = ms.unwrap_or(if t == Transition::Cut { 0 } else { 500 });
            match a.get("to") {
                Some(to) => {
                    let l = layout_of(pr, to)?;
                    pr.sw.take_layout(l, t, dur);
                }
                None => pr.sw.take(t, dur),
            }
            ok(json!({"program": pr.sw.program(), "preview": pr.sw.state().preview, "transition_ms": dur}))
        }
        "scene" => {
            let n = s_of(a, "name").ok_or("scene needs \"name\"")?;
            let l = layout_of(pr, a.get("layout").ok_or("scene needs \"layout\"")?)?;
            pr.sw.save_scene(n, l)?;
            ok(json!({"saved": n}))
        }
        "title" => {
            let kind = match s_of(a, "kind").unwrap_or("lower") {
                "lower" | "lower_third" => TitleKind::Lower,
                "centre" | "center" | "full" => TitleKind::Centre,
                "bug" => TitleKind::Bug,
                o => return Err(format!("title kind \"{o}\": lower, centre or bug")),
            };
            let id = pr.sw.title_show(kind, s_of(a, "text").ok_or("title needs \"text\"")?, s_of(a, "sub").unwrap_or(""), ms.unwrap_or(300));
            ok(json!({"title": id}))
        }
        "title_off" => ok(json!({"hidden": pr.sw.title_hide(f(a, "title").map(|t| t as u32))})),
        "mute" | "unmute" => {
            let i = pr.sw.find(&a.get("source").map(|v| v.as_str().map(str::to_owned).unwrap_or_else(|| v.to_string())).ok_or("mute needs \"source\"")?)?;
            pr.sw.mute(i, op == "mute");
            ok(json!({"source": i, "muted": op == "mute"}))
        }
        "gain" => {
            let i = pr.sw.find(&a.get("source").map(|v| v.as_str().map(str::to_owned).unwrap_or_else(|| v.to_string())).ok_or("gain needs \"source\"")?)?;
            pr.sw.set_gain(i, f(a, "gain").unwrap_or(1.0) as f32);
            ok(json!({"source": i}))
        }
        "run" => {
            let secs = f(a, "seconds").ok_or("run needs \"seconds\" (the simulated clock; a real-time production runs by itself)")?.clamp(0.0, 3600.0);
            pr.run_for(secs)?;
            ok(json!({"seconds": r3(pr.seconds())}))
        }
        "frame" | "multiview" => {
            let fr = if op == "frame" { pr.last_program.clone().ok_or("nothing has been made yet: run a little first")? } else { pr.sw.multiview(pr.last_program.as_ref().ok_or("nothing has been made yet: run a little first")?) };
            Ok(vec![Content::Jpeg(jpeg_of(&fr)), Content::Text(json!({"seconds": r3(pr.seconds()), "state": pr.sw.state()}).to_string())])
        }
        "stream" => {
            if !b_of(a, "user_approved") {
                return Err("streaming sends the program to the internet: only when the person asked for it (user_approved: true)".into());
            }
            pr.stream_to(s_of(a, "url").ok_or("stream needs \"url\": rtmp://host/app/key or srt://host:port?streamid=…")?)?;
            ok(json!({"streams": pr.stream_status()}))
        }
        "stop_stream" => ok(json!({"stopped": pr.stop_streams()})),
        "record_program" => ok(json!({"writing": pr.record_program(dir, name)?.display().to_string()})),
        "replay_save" => {
            let (p, secs) = pr.save_replay(dir, name, f(a, "seconds").unwrap_or(10.0))?;
            ok(json!({"saved": p.display().to_string(), "seconds": r3(secs)}))
        }
        "replay_play" => {
            let i = pr.replay_as_source(s_of(a, "as").unwrap_or("replay"), f(a, "seconds").unwrap_or(10.0), f(a, "speed").unwrap_or(1.0))?;
            ok(json!({"source": i, "next": "take it to program like any source: do: take, to: replay"}))
        }
        o => Err(format!("live do \"{o}\": open, take, cut, preview, scene, title, title_off, mute, unmute, gain, state, frame, multiview, run, stream, stop_stream, record_program, replay_save, replay_play, close")),
    }
}

// ───────────────────────── for the Record window ─────────────────────────

/// The prompter's picture, after letting `dt` seconds pass (the window
/// calls this every frame it shows).
pub(crate) fn ui_prompter_frame(dt: f64, w: u32, h: u32) -> Option<studio_live::frame::Frame> {
    let mut st = lock().ok()?;
    let p = st.prompter.as_mut()?;
    let mut left = dt.clamp(0.0, 0.5);
    while left > 0.0 {
        let step = left.min(0.02);
        p.tick(step);
        left -= step;
    }
    Some(p.render(w, h))
}

/// The program (or the multiview) of a live session as it is now.
pub(crate) fn ui_live_frame(session: u32, multiview: bool) -> Option<studio_live::frame::Frame> {
    let mut st = lock().ok()?;
    let s = st.prods.get_mut(&session)?;
    let get = |pr: &mut Production| -> Option<studio_live::frame::Frame> {
        let last = pr.last_program.clone()?;
        Some(if multiview { pr.sw.multiview(&last) } else { last })
    };
    match &mut s.mode {
        ProdMode::Sim(pr) => get(pr),
        ProdMode::Live(lp) => lp.with(get).ok().flatten(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::SEC;

    fn session(tag: &str) -> Session {
        let dir = std::env::temp_dir().join(format!("studio-capture-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut s = Session::new(crate::services::project::new_custom("Capture", 640, 360, 30.0));
        s.home = Some(dir);
        s
    }

    fn cap(s: &mut Session, args: Value) -> Result<Value, String> {
        let parts = s.call("capture", &args)?;
        parts.iter().find_map(|p| if let Content::Text(t) = p { serde_json::from_str(t).ok() } else { None }).ok_or_else(|| "no text".to_string())
    }

    #[test]
    fn test_sources_record_to_files_and_land_on_the_timeline() {
        let mut s = session("rec");
        let r = cap(
            &mut s,
            json!({"action": "record", "inputs": [{"label": "cam a", "video": "test:counter:1", "audio": "test:tone:440"}, {"label": "lav", "audio": "test:voice"}], "seconds": 2, "width": 320, "height": 180, "to_timeline": true, "at": 1.0}),
        )
        .unwrap();
        assert_eq!(r["recorded"].as_array().unwrap().len(), 2);
        assert_eq!(r["timeline"]["clips"].as_array().unwrap().len(), 2, "{r}");
        let p = &s.doc.project;
        let video = p.clips.iter().find(|c| c.kind == crate::view::ClipKind::Video).expect("a video clip");
        assert_eq!(video.start, SEC);
        assert!((video.dur as f64 / SEC as f64 - 2.0).abs() < 0.1, "{}", video.dur);
        assert!(r["recorded"][0]["path"].as_str().unwrap().contains("Recordings"));
    }

    #[test]
    fn real_devices_streams_and_questions_need_the_persons_say() {
        let mut s = session("consent");
        for args in [
            json!({"action": "record", "video": "camera:default", "seconds": 1}),
            json!({"action": "record", "video": "screen:1", "seconds": 1}),
            json!({"action": "voiceover", "mic": "mic:default", "seconds": 1}),
            json!({"action": "permissions", "request": "camera"}),
            json!({"action": "live", "do": "open", "sources": [{"name": "a", "video": "test:bars"}], "stream": "rtmp://127.0.0.1:1/live/k"}),
        ] {
            let e = cap(&mut s, args.clone()).unwrap_err();
            assert!(e.contains("person") || e.contains("real device") || e.contains("real microphone"), "{args}: {e}");
        }
        // The listing answers without opening anything.
        let d = cap(&mut s, json!({"action": "devices"})).unwrap();
        assert!(d["test_sources"].as_str().unwrap().contains("test:counter"));
        assert!(d["access"]["camera"].is_string());
    }

    #[test]
    fn a_voice_over_is_counted_in_and_placed_where_it_began() {
        let mut s = session("vo");
        let r = cap(&mut s, json!({"action": "voiceover", "mic": "test:voice", "at": 4.0, "seconds": 2, "count_in": 4, "bpm": 120, "trim": false})).unwrap();
        assert!((r["count_in_seconds"].as_f64().unwrap() - 2.0).abs() < 0.05, "{r}");
        assert_eq!(r["starts_at"].as_f64().unwrap(), 4.0);
        assert_eq!(r["lane"], "voice-over");
        let c = s.doc.project.clip(r["clip"].as_u64().unwrap() as u32).unwrap();
        assert_eq!(c.start, 4 * SEC);
        assert!((c.dur as f64 / SEC as f64 - 2.0).abs() < 0.05);
        let e = cap(&mut s, json!({"action": "voiceover", "mic": "test:silence", "seconds": 1, "count_in": 0})).unwrap_err();
        assert!(e.contains("silence"), "{e}");
    }

    #[test]
    fn the_prompter_follows_words_and_draws() {
        let mut s = session("prompter");
        cap(&mut s, json!({"action": "prompter", "do": "load", "text": "Good evening and welcome.\n(pause)\nTonight we have a guest.", "mode": "follow", "mirror": true})).unwrap();
        let r = cap(&mut s, json!({"action": "prompter", "do": "hear", "text": "good evening and welcome tonight"})).unwrap();
        assert_eq!(r["word"], 6, "{r}");
        let parts = s.call("capture", &json!({"action": "prompter", "do": "frame", "width": 320, "height": 180})).unwrap();
        assert!(matches!(parts[0], Content::Jpeg(ref b) if b.len() > 1000));
        let e = cap(&mut s, json!({"action": "prompter", "do": "load", "scene": "9"})).unwrap_err();
        assert!(e.contains("story") || e.contains("scene"), "{e}");
    }

    #[test]
    fn a_live_show_is_switched_recorded_and_brought_into_the_project() {
        let mut s = session("live");
        let r = cap(&mut s, json!({"action": "live", "do": "open", "sources": [{"name": "wide", "video": "test:counter:1", "audio": "test:tone:300"}, {"name": "close", "video": "test:counter:2", "audio": "test:tone:700"}], "width": 320, "height": 180, "replay_secs": 6, "record_program": true, "iso": true})).unwrap();
        let id = r["session"].as_u64().unwrap();
        for args in [
            json!({"do": "cut", "to": "wide"}),
            json!({"do": "run", "seconds": 1}),
            json!({"do": "title", "kind": "lower", "text": "Priya", "sub": "Host"}),
            json!({"do": "take", "to": "close", "transition": "mix", "ms": 500}),
            json!({"do": "run", "seconds": 2}),
            json!({"do": "replay_save", "seconds": 2}),
        ] {
            let mut a = args.clone();
            a["action"] = json!("live");
            a["session"] = json!(id);
            cap(&mut s, a).unwrap_or_else(|e| panic!("{args}: {e}"));
        }
        let st = cap(&mut s, json!({"action": "live", "do": "state", "session": id})).unwrap();
        assert_eq!(st["state"]["program"]["full"], 1);
        let r = cap(&mut s, json!({"action": "live", "do": "close", "session": id, "to_timeline": true})).unwrap();
        assert_eq!(r["files"].as_array().unwrap().len(), 3, "{r}");
        assert_eq!(r["timeline"]["clips"].as_array().unwrap().len(), 3);
    }

    #[test]
    fn a_card_is_copied_twice_verified_and_imported() {
        let mut s = session("ingest");
        let root = s.home.clone().unwrap();
        let card = root.join("CARD/DCIM/100");
        std::fs::create_dir_all(&card).unwrap();
        // A real file the app can read: a recording of a test camera.
        let r = cap(&mut s, json!({"action": "record", "video": "test:bars", "audio": "test:tone:300", "label": "c", "seconds": 1, "width": 160, "height": 90})).unwrap();
        std::fs::copy(r["recorded"][0]["path"].as_str().unwrap(), card.join("C0001.MP4")).unwrap();
        let refused = cap(&mut s, json!({"action": "ingest", "from": root.join("CARD").display().to_string(), "to": root.join("work").display().to_string()})).unwrap_err();
        assert!(refused.contains("second copy"), "{refused}");
        let r = cap(&mut s, json!({"action": "ingest", "from": root.join("CARD").display().to_string(), "to": root.join("work").display().to_string(), "backup": root.join("backup").display().to_string(), "allow_same_drive": true, "name": "A001", "import": true})).unwrap();
        assert_eq!(r["ok"], true, "{r}");
        assert_eq!(r["verified"], 1);
        assert_eq!(r["imported"]["media"].as_array().unwrap().len(), 1);
        let v = cap(&mut s, json!({"action": "verify", "dir": root.join("backup/A001").display().to_string()})).unwrap();
        assert_eq!(v["ok"], true);
    }

    #[test]
    fn a_recorder_started_late_is_lined_up_with_the_camera() {
        let mut s = session("sync");
        let r = cap(
            &mut s,
            json!({"action": "record", "inputs": [{"label": "cam", "video": "test:bars", "audio": "test:noise:5@0.5"}, {"label": "recorder", "audio": "test:noise:5@0.5>1.5"}], "seconds": 6, "width": 160, "height": 90, "to_timeline": true, "at": 10.0}),
        )
        .unwrap();
        let clips = r["timeline"]["clips"].as_array().unwrap();
        let (cam_clip, rec_media) = (clips[0].as_u64().unwrap(), r["timeline"]["media"][1].as_u64().unwrap());
        let out = cap(&mut s, json!({"action": "sync", "video": cam_clip, "audio": rec_media, "apply": true})).unwrap();
        let off = out["offset_s"].as_f64().unwrap();
        assert!((off - 1.5).abs() < 0.01, "{out}");
        assert!(out["confidence"].as_f64().unwrap() > 15.0);
        let placed = s.doc.project.clip(out["placed_clip"].as_u64().unwrap() as u32).unwrap();
        assert!((placed.start as f64 / SEC as f64 - 11.5).abs() < 0.02, "the recorder's first sample belongs 1.5 s into the camera clip at 10 s: {}", placed.start);
    }
}

/// Sound in (16 kHz mono blocks), a rolling window of it transcribed, the
/// new words fed to the prompter, until the sound ends, `total` seconds
/// have passed, `stop` is set or the prompter is closed. Returns the words
/// fed and the seconds heard.
fn follow_loop(mut source: Box<dyn FnMut(usize) -> Vec<f32> + Send>, total: f64, stop: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>) -> Result<(usize, f64), String> {
    const WINDOW: f64 = 6.0;
    const HOP: f64 = 2.0;
    let rate = 16_000usize;
    let mut window: Vec<f32> = Vec::new();
    let mut t = 0.0f64;
    let mut emitted_until = 0.0f64;
    let mut fed = 0usize;
    loop {
        if stop.as_ref().is_some_and(|s| s.load(std::sync::atomic::Ordering::Relaxed)) || lock()?.prompter.is_none() {
            break;
        }
        let block = source((HOP * rate as f64) as usize);
        let ended = block.len() < (HOP * rate as f64) as usize || t + HOP >= total;
        window.extend_from_slice(&block);
        t += block.len() as f64 / rate as f64;
        let keep = (WINDOW * rate as f64) as usize;
        if window.len() > keep {
            window.drain(..window.len() - keep);
        }
        let win_start = t - window.len() as f64 / rate as f64;
        let words = words_of(&window, rate as u32);
        let len = window.len() as f64 / rate as f64;
        // Words near the edge of the window may be cut off: they wait for the next one.
        let ready: Vec<&Said> = words.iter().filter(|w| win_start + w.end > emitted_until + 0.05 && (ended || w.end < len - 0.4)).collect();
        if let Some(last) = ready.iter().map(|w| win_start + w.end).fold(None, |m: Option<f64>, x| Some(m.map_or(x, |m| m.max(x)))) {
            emitted_until = last;
        }
        if !ready.is_empty() {
            let refs: Vec<&str> = ready.iter().map(|w| w.text.as_str()).collect();
            if let Some(p) = lock()?.prompter.as_mut() {
                p.hear(&refs);
            }
            fed += refs.len();
        }
        if ended {
            break;
        }
    }
    Ok((fed, t))
}

fn follow_stop() -> &'static Mutex<Option<std::sync::Arc<std::sync::atomic::AtomicBool>>> {
    static S: OnceLock<Mutex<Option<std::sync::Arc<std::sync::atomic::AtomicBool>>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(None))
}

/// The Record window's "follow my voice": listens to the microphone on a
/// thread until [`ui_stop_follow`] (the person pressed the button, so the
/// microphone is theirs to use).
pub(crate) fn ui_start_follow(mic: &str) -> Result<(), String> {
    ui_stop_follow();
    let mut m = open_audio(mic, 48_000, Consent::GIVEN)?;
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    if let Ok(mut g) = follow_stop().lock() {
        *g = Some(stop.clone());
    }
    std::thread::spawn(move || {
        let source: Box<dyn FnMut(usize) -> Vec<f32> + Send> = Box::new(move |n| {
            std::thread::sleep(std::time::Duration::from_secs_f64(n as f64 / 16_000.0));
            let b = m.read(n * 3);
            studio_live::sync::resample(&b, 48_000, 16_000)
        });
        let _ = follow_loop(source, f64::INFINITY, Some(stop));
    });
    Ok(())
}

pub(crate) fn ui_stop_follow() {
    if let Some(s) = follow_stop().lock().ok().and_then(|mut g| g.take()) {
        s.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}
