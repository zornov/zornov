// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `zornov eval`: the AI editor on real jobs, scored, so a change to it
//! is measured rather than guessed.
//!
//! ```text
//! zornov eval --out DIR [--provider codex|claude] [--model M] [--solo]
//!             [--tasks story_scene,music_video,...] [--minutes 30]
//! ```
//!
//! Each task builds a fresh project under `DIR` (its own footage, written
//! here; never the person's projects), runs the request exactly as the AI
//! panel does (the same gate, crew, plan, corrections and taste; Codex or
//! Claude Code through `zornov mcp --attach`), headless, then scores what
//! is on the timeline: checks with weights, 0 to 100 a task. Results go to
//! `DIR/results.json` and a line is added to `DIR/history.jsonl`, and the
//! table shows the change since the last run. Each task's calls are also
//! recorded (`DIR/TASK/agent-trace/TASK.jsonl`) and measured for friction:
//! calls to goal, retry, silent-drop and undo rates, reply tokens and wall
//! time go into both files, so a commit is measured on what agents go
//! through, not just the score.

use super::Session;
use super::brain::{AgentEvent, Answer, Gate};
use super::crew::{self, CrewBrain};
use super::live::Live;
use crate::view::{ClipKind, SEC};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// One job.
pub struct Task {
    pub name: &'static str,
    /// What the person asks.
    pub brief: &'static str,
    /// A correction sent after this many tool calls, if any.
    pub steer: Option<(usize, &'static str)>,
    setup: fn(&mut Session, &Path) -> Result<(), String>,
    score: fn(&mut Session, &Run) -> Vec<Check>,
}

/// What happened while it ran.
#[derive(Default)]
pub struct Run {
    pub said: String,
    pub tools: Vec<String>,
    pub notes: Vec<String>,
    pub errors: Vec<String>,
    pub tokens: u64,
    pub seconds: f32,
    pub plan: Option<crew::Plan>,
    /// The correction reached the agent.
    pub steered: bool,
    pub timed_out: bool,
    /// The friction the agent met, from its trace ([`super::trace_report::metrics`]).
    pub friction: Value,
}

/// One scored check.
#[derive(Clone, Debug)]
pub struct Check {
    pub name: String,
    pub pass: bool,
    pub weight: f32,
    pub detail: String,
}

fn check(name: &str, pass: bool, weight: f32, detail: impl Into<String>) -> Check {
    Check { name: name.into(), pass, weight, detail: detail.into() }
}

pub fn score_of(checks: &[Check]) -> f32 {
    let total: f32 = checks.iter().map(|c| c.weight).sum();
    if total <= 0.0 { 0.0 } else { 100.0 * checks.iter().filter(|c| c.pass).map(|c| c.weight).sum::<f32>() / total }
}

pub fn tasks() -> Vec<Task> {
    vec![
        Task {
            name: "story_scene",
            brief: "Stage a short story scene, about 10 seconds: a fox and a rabbit meet in a forest clearing at dusk. The fox walks in from the left and waves; the rabbit hops twice and runs off to the right. The characters must move and act, with forest ambience and a soft music bed.",
            steer: None,
            setup: |_, _| Ok(()),
            score: score_story,
        },
        Task {
            name: "music_video",
            brief: "Cut a 20 second music video from the clips in the media bin to the song on the timeline: cut on the beat, every clip used at least once, the most energetic moments on the loud parts.",
            steer: None,
            setup: setup_music_video,
            score: score_music_video,
        },
        Task {
            name: "fix_colour",
            brief: "The shot on the timeline has a nasty blue cast and looks too dark. Fix its colour so it looks natural: neutral greys, healthy exposure.",
            steer: None,
            setup: setup_fix_colour,
            score: score_fix_colour,
        },
        Task {
            name: "honest_animation",
            brief: "Make the cat on the timeline come alive like a cartoon character: it waves hello and bounces for 4 seconds.",
            steer: None,
            setup: setup_cat,
            score: score_cat,
        },
        Task {
            name: "steer_title",
            brief: "Make a 6 second title sequence: the words NIGHT RIDE fly in over a city backdrop, with a synth riser under it.",
            steer: Some((3, "Change of plan: the title should read DAWN RIDE, and the mood is a warm sunrise, not night.")),
            setup: |_, _| Ok(()),
            score: score_steer,
        },
    ]
}

// ---------------------------------------------------------------- footage

/// A clip of `secs` seconds written with Studio's own encoder: `look`
/// picks the picture (0..4: moving shapes in different colours; 9: a
/// daylight scene with a grey card, tinted blue and dark).
#[cfg(feature = "render")]
fn write_clip(path: &Path, secs: f32, look: u32) -> Result<(), String> {
    use studio_encode::{h264, mux, yuv};
    const W: u32 = 640;
    const H: u32 = 360;
    const FPS: u32 = 30;
    let n = (secs * FPS as f32) as u32;
    let mut cfg = h264::Config::new(W, H, FPS, 1);
    cfg.max_slices = 1;
    cfg.bitrate_kbps = 3000;
    let mut enc = h264::Encoder::new(cfg).map_err(|e| format!("{e:?}"))?;
    let mut muxer = mux::Muxer::create(path, mux::Options::mp4()).map_err(|e| e.to_string())?;
    let v = muxer.add_video(enc.track_info()).map_err(|e| e.to_string())?;
    let hues: [[f32; 3]; 5] = [[0.95, 0.35, 0.2], [0.2, 0.7, 0.95], [0.3, 0.9, 0.4], [0.95, 0.85, 0.2], [0.8, 0.3, 0.9]];
    for i in 0..n {
        let t = i as f32 / FPS as f32;
        let mut rgba = vec![0u8; (W * H * 4) as usize];
        for y in 0..H {
            for x in 0..W {
                let (fx, fy) = (x as f32 / W as f32, y as f32 / H as f32);
                let rgb: [f32; 3] = if look == 9 {
                    // Sky, ground, a grey card in the middle; then the cast.
                    let base = if (0.4..0.6).contains(&fx) && (0.35..0.65).contains(&fy) {
                        [0.5, 0.5, 0.5]
                    } else if fy < 0.5 {
                        [0.55 + 0.2 * fy, 0.7, 0.9]
                    } else {
                        [0.45, 0.4 + 0.1 * (fx * 20.0 + t).sin().abs(), 0.25]
                    };
                    [base[0] * 0.45, base[1] * 0.5, (base[2] * 0.6 + 0.25).min(1.0)]
                } else {
                    let c = hues[look as usize % hues.len()];
                    let (cx, cy) = (0.5 + 0.3 * (t * 1.3 + look as f32).sin(), 0.5 + 0.25 * (t * 1.7).cos());
                    let d = ((fx - cx) * 1.8).hypot(fy - cy);
                    let glow = (1.0 - d * 3.0).clamp(0.0, 1.0);
                    let bg = 0.12 + 0.1 * ((fx * 8.0 + t * 2.0).sin() * (fy * 6.0).cos()).abs();
                    [bg + glow * c[0], bg + glow * c[1], bg + glow * c[2]]
                };
                let k = ((y * W + x) * 4) as usize;
                rgba[k..k + 4].copy_from_slice(&[(rgb[0].clamp(0.0, 1.0) * 255.0) as u8, (rgb[1].clamp(0.0, 1.0) * 255.0) as u8, (rgb[2].clamp(0.0, 1.0) * 255.0) as u8, 255]);
            }
        }
        let pic = yuv::Yuv420::from_rgba(&rgba, W, H);
        let f = enc.encode(&pic);
        muxer.write_sample(v, &f.data, f.keyframe).map_err(|e| e.to_string())?;
    }
    muxer.finish().map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(not(feature = "render"))]
fn write_clip(_: &Path, _: f32, _: u32) -> Result<(), String> {
    Err("footage needs the render feature".into())
}

fn call(s: &mut Session, tool: &str, args: Value) -> Result<String, String> {
    let parts = s.run(tool, &args)?;
    Ok(parts.into_iter().filter_map(|p| if let super::Content::Text(t) = p { Some(t) } else { None }).collect::<Vec<_>>().join("\n"))
}

fn setup_music_video(s: &mut Session, dir: &Path) -> Result<(), String> {
    let media = dir.join("Footage");
    std::fs::create_dir_all(&media).map_err(|e| e.to_string())?;
    let mut paths = Vec::new();
    for i in 0..5 {
        let p = media.join(format!("shot-{}.mp4", i + 1));
        write_clip(&p, 6.0, i)?;
        paths.push(p.display().to_string());
    }
    call(s, "import", json!({"paths": paths}))?;
    // The song, 20 seconds of it (the `music` op scores pictures, and
    // there are none yet).
    call(s, "edit", json!({"ops": [{"op": "add", "item": "Upbeat Forever", "panel": "audio", "at": 0, "duration": 20}]}))?;
    Ok(())
}

fn setup_fix_colour(s: &mut Session, dir: &Path) -> Result<(), String> {
    let p = dir.join("Footage").join("blue-shot.mp4");
    std::fs::create_dir_all(p.parent().expect("dir")).map_err(|e| e.to_string())?;
    write_clip(&p, 4.0, 9)?;
    call(s, "import", json!({"paths": [p.display().to_string()]}))?;
    call(s, "edit", json!({"ops": [{"op": "place", "media": "blue-shot", "at": 0}]}))?;
    Ok(())
}

fn setup_cat(s: &mut Session, _: &Path) -> Result<(), String> {
    call(s, "edit", json!({"ops": [{"op": "card", "colour": "#bfe3ff", "at": 0, "duration": 4}]}))?;
    if call(s, "edit", json!({"ops": [{"op": "add", "item": "Cat", "panel": "assets", "at": 0, "duration": 4}]})).is_err() {
        call(s, "edit", json!({"ops": [{"op": "text", "text": "🐱", "at": 0, "duration": 4, "size": 300}]}))?;
    }
    Ok(())
}

// ---------------------------------------------------------------- scoring

fn secs(t: crate::view::Time) -> f64 {
    t as f64 / SEC as f64
}

fn looked_after_last_edit(run: &Run) -> bool {
    let last_edit = run.tools.iter().rposition(|t| t == "edit");
    let last_look = run.tools.iter().rposition(|t| matches!(t.as_str(), "see" | "frame" | "contact_sheet" | "review" | "check" | "listen"));
    match (last_edit, last_look) {
        (Some(e), Some(l)) => l > e,
        (None, _) => true,
        _ => false,
    }
}

fn honest(run: &Run) -> Check {
    let claimed = crew::claims_cannot(&run.said);
    let checked = run.tools.iter().any(|t| t == "capabilities");
    check("honest", !claimed || checked, 2.0, if claimed { "it said something cannot be done" } else { "no false 'impossible'" })
}

/// Clips with motion: keys, a motion preset, a rig or a puppet.
fn animated_clips(s: &mut Session) -> Vec<u32> {
    let ids: Vec<u32> = s.doc.project.clips.iter().filter(|c| matches!(c.kind, ClipKind::Video | ClipKind::Title)).map(|c| c.id).collect();
    ids.into_iter()
        .filter(|id| {
            let Some(c) = s.doc.project.clip(*id) else { return false };
            if !c.keys.is_empty() || c.motion_in.is_some() {
                return true;
            }
            call(s, "inspect", json!({"clip": id})).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).is_some_and(|v| {
                v["keyframes"].as_object().is_some_and(|k| !k.is_empty()) || v["keyframes"].as_array().is_some_and(|k| !k.is_empty()) || !v["pins"].is_null() || !v["skeleton"].is_null()
            })
        })
        .collect()
}

fn score_story(s: &mut Session, run: &Run) -> Vec<Check> {
    let dur = secs(s.doc.project.duration());
    let animated = animated_clips(s);
    let clips = &s.doc.project.clips;
    let audio = clips.iter().filter(|c| c.kind == ClipKind::Audio).count();
    vec![
        check("length", (7.0..=14.0).contains(&dur), 1.0, format!("{dur:.1}s")),
        check("characters act", animated.len() >= 2, 4.0, format!("{} animated clips", animated.len())),
        check("a place", clips.iter().any(|c| matches!(c.kind, ClipKind::Video | ClipKind::Title) && c.dur as f64 >= 0.6 * dur * SEC as f64), 1.0, "a backdrop under most of it"),
        check("sound", audio >= 2, 2.0, format!("{audio} audio clips")),
        honest(run),
        check("looked last", looked_after_last_edit(run), 1.0, "looked after its last edit"),
    ]
}

fn score_music_video(s: &mut Session, run: &Run) -> Vec<Check> {
    let clips = s.doc.project.clips.clone();
    let video: Vec<_> = clips.iter().filter(|c| c.kind == ClipKind::Video).collect();
    let used: std::collections::HashSet<_> = video.iter().filter_map(|c| c.media).collect();
    let end = video.iter().map(|c| c.start + c.dur).max().unwrap_or(0);
    let song = clips.iter().find(|c| c.kind == ClipKind::Audio);
    // Beats of the song, on the timeline.
    let beats: Vec<f64> = song
        .and_then(|c| call(s, "listen", json!({"clip": c.id})).ok())
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .map(|v| v["beats"].as_array().cloned().unwrap_or_default().iter().filter_map(Value::as_f64).collect())
        .unwrap_or_default();
    let cuts: Vec<f64> = video.iter().map(|c| secs(c.start)).filter(|t| *t > 0.05).collect();
    let on_beat = cuts.iter().filter(|t| beats.iter().any(|b| (b - *t).abs() < 0.09)).count();
    let share = if cuts.is_empty() { 0.0 } else { on_beat as f64 / cuts.len() as f64 };
    vec![
        check("length", (15.0..=25.0).contains(&secs(end)), 1.0, format!("{:.1}s of picture", secs(end))),
        check("enough cuts", video.len() >= 8, 2.0, format!("{} shots", video.len())),
        check("every clip used", used.len() >= 5, 1.0, format!("{} of 5 clips", used.len())),
        check("on the beat", !beats.is_empty() && share >= 0.6, 3.0, format!("{on_beat} of {} cuts within 90 ms of a beat ({} beats heard)", cuts.len(), beats.len())),
        check("music under it", song.is_some_and(|c| c.start + c.dur >= end.saturating_sub(SEC / 2)), 1.0, "the song runs to the end"),
        honest(run),
        check("looked last", looked_after_last_edit(run), 1.0, "looked after its last edit"),
    ]
}

/// Mean colour of the middle of the picture at `t` (0..1 RGB).
fn mean_middle(s: &mut Session, t: f64) -> Option<[f32; 3]> {
    let parts = s.run("frame", &json!({"t": t})).ok()?;
    let jpeg = parts.into_iter().find_map(|p| if let super::Content::Jpeg(b) = p { Some(b) } else { None })?;
    let img = studio_media::jpeg::decode(&jpeg).ok()?;
    let (w, h) = (img.width as usize, img.height as usize);
    let mut sum = [0f64; 3];
    let mut n = 0f64;
    for y in h * 2 / 5..h * 3 / 5 {
        for x in w * 9 / 20..w * 11 / 20 {
            let k = (y * w + x) * 4;
            for c in 0..3 {
                sum[c] += img.pixels[k + c] as f64 / 255.0;
            }
            n += 1.0;
        }
    }
    (n > 0.0).then(|| [(sum[0] / n) as f32, (sum[1] / n) as f32, (sum[2] / n) as f32])
}

fn score_fix_colour(s: &mut Session, run: &Run) -> Vec<Check> {
    let m = mean_middle(s, 1.0);
    let (cast, luma) = m.map_or((1.0, 0.0), |[r, g, b]| ((b - r).abs().max((b - g).abs()), 0.2126 * r + 0.7152 * g + 0.0722 * b));
    vec![
        check("grey card neutral", cast < 0.06, 4.0, format!("blue against red/green {cast:.3} (was about 0.25)")),
        check("exposure", (0.35..=0.65).contains(&luma), 2.0, format!("card luma {luma:.2} (was about 0.27)")),
        honest(run),
        check("looked last", looked_after_last_edit(run), 1.0, "looked after its last edit"),
    ]
}

fn score_cat(s: &mut Session, run: &Run) -> Vec<Check> {
    let animated = animated_clips(s);
    let refused = crew::claims_cannot(&run.said);
    vec![
        check("the cat moves", !animated.is_empty(), 5.0, format!("{} animated clips", animated.len())),
        check("no still workaround", !refused, 2.0, if refused { "it claimed it could not" } else { "it did not refuse" }),
        honest(run),
        check("looked last", looked_after_last_edit(run), 1.0, "looked after its last edit"),
    ]
}

fn score_steer(s: &mut Session, run: &Run) -> Vec<Check> {
    let texts: Vec<String> = s.doc.project.clips.iter().filter(|c| c.kind == ClipKind::Title).map(|c| c.text.to_uppercase()).collect();
    let dawn = texts.iter().any(|t| t.contains("DAWN"));
    let night = texts.iter().any(|t| t.contains("NIGHT"));
    vec![
        check("correction reached it", run.steered, 2.0, "the agent was told at its next step"),
        check("new title", dawn, 3.0, format!("titles: {texts:?}")),
        check("old title gone", !night, 2.0, "no NIGHT left"),
        honest(run),
    ]
}

// ---------------------------------------------------------------- running

/// Runs one task on a fresh project in `dir`: the request through the
/// same gate as the AI panel, `cast` making the brains (given the gate's
/// socket, for the command-line agents).
pub fn run_task(task: &Task, dir: &Path, cast: impl FnOnce(PathBuf) -> crew::Casting, crew_mode: bool, limit: Duration, log: &mut dyn FnMut(&str)) -> Result<(Vec<Check>, Run), String> {
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let proj = dir.join("project");
    let mut s = Session::at(&proj)?;
    (task.setup)(&mut s, dir)?;
    call(&mut s, "save", json!({}))?;
    // The agent's calls from here on are recorded, the scoring's are not.
    let trace_file = super::trace::begin_session(&dir.join("agent-trace"), task.name);
    let (calls_tx, calls_rx) = mpsc::channel();
    let (events_tx, events_rx) = mpsc::channel();
    let gate = Arc::new(Gate::new(calls_tx, events_tx, || {}));
    // Codex and Claude Code reach the gate through the socket.
    let socket = super::attach::serve(gate.clone()).map_err(|e| e.to_string())?;
    let cast = cast(socket);
    gate.set_project(Some(proj.clone()));
    let brief = format!("[editor: playhead 0.00s; nothing selected]{}\n{}", {
        let t = super::taste::brief(Some(&proj));
        if t.is_empty() { t } else { format!("\n{t}") }
    }, task.brief);
    let mut brain = if crew_mode { Box::new(CrewBrain::new(cast)) as Box<dyn super::brain::Brain> } else { cast(None)? };
    let g = gate.clone();
    let started = Instant::now();
    let turn = std::thread::spawn(move || {
        g.begin();
        g.board.approve_checkpoints.store(true, std::sync::atomic::Ordering::SeqCst);
        let r = if crew_mode { brain.turn(&brief, &g) } else { crew::honest_turn(brain.as_mut(), &brief, &g, crew::LEAD) };
        g.end();
        r
    });
    let mut live = Live::new(|| {});
    live.begin_turn(&format!("AI: {}", task.name));
    let mut run = Run::default();
    let mut steered = false;
    loop {
        let finished = turn.is_finished() && !live.busy();
        live.tick(&mut s.doc, &calls_rx);
        for e in events_rx.try_iter() {
            match e {
                AgentEvent::Text(t) => run.said.push_str(&t),
                AgentEvent::ToolStart { tool, brief, .. } => {
                    log(&format!("  · {tool} {brief}"));
                    run.tools.push(tool);
                }
                AgentEvent::ToolEnd { ok: false, brief, .. } => run.errors.push(brief),
                AgentEvent::ToolEnd { .. } => {}
                AgentEvent::Note(n) => {
                    if n.contains("picked up your correction") {
                        run.steered = true;
                    }
                    log(&format!("  ~ {n}"));
                    run.notes.push(n);
                }
                AgentEvent::Ask { answer, what, .. } => {
                    log(&format!("  ? {} → approved", super::brain::one_line(&what, 100)));
                    let _ = answer.send(Answer::Apply);
                }
                AgentEvent::Usage(u) => run.tokens += u.total(),
                AgentEvent::Error(e) => run.errors.push(e),
                AgentEvent::Plan(p) => run.plan = Some(p),
                AgentEvent::Done { .. } => {}
            }
        }
        if let Some((after, text)) = task.steer {
            if !steered && run.tools.len() >= after {
                steered = true;
                log(&format!("  ! correction: {text}"));
                gate.steer(text);
            }
        }
        if started.elapsed() > limit && !gate.stopped() {
            log("  ! out of time: stopping");
            run.timed_out = true;
            gate.halt();
        }
        // One more pass after the request ends, for its last words.
        if finished {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    live.end_turn(&mut s.doc);
    run.seconds = started.elapsed().as_secs_f32();
    super::trace::end_session();
    super::trace::force(Some(false));
    run.friction = super::trace_report::load(&trace_file).ok().and_then(|t| t.into_iter().next()).map_or_else(|| json!({"calls": 0}), |t| super::trace_report::metrics(&t));
    if let Err(e) = turn.join().unwrap_or_else(|p| Err(super::guard::describe("the request", p.as_ref()))) {
        run.errors.push(e);
    }
    let _ = call(&mut s, "save", json!({}));
    let mut checks = (task.score)(&mut s, &run);
    checks.push(check("finished in time", !run.timed_out, 1.0, format!("{:.0}s", run.seconds)));
    // An agent that never got to work passes nothing.
    if !run.tools.iter().any(|t| t == "edit") {
        for c in &mut checks {
            c.pass = false;
            c.detail = format!("no edit made ({})", run.errors.first().map_or("no error", |e| e.as_str()).chars().take(160).collect::<String>());
        }
    }
    Ok((checks, run))
}

/// `zornov eval …`
pub fn main(args: &[String]) -> i32 {
    let get = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    let Some(out) = get("--out").map(PathBuf::from) else {
        eprintln!("usage: zornov eval --out DIR [--provider codex|claude] [--model M] [--solo] [--tasks a,b] [--minutes 30]\ntasks: {}", tasks().iter().map(|t| t.name).collect::<Vec<_>>().join(", "));
        return 2;
    };
    let out = std::path::absolute(&out).unwrap_or(out);
    let provider = match get("--provider").as_deref() {
        Some("claude") | Some("claude-code") => 1,
        _ => 3,
    };
    let model = get("--model").unwrap_or_default();
    let crew_mode = !args.iter().any(|a| a == "--solo");
    let minutes: u64 = get("--minutes").and_then(|m| m.parse().ok()).unwrap_or(30);
    let want: Option<Vec<String>> = get("--tasks").map(|t| t.split(',').map(str::to_owned).collect());
    let _ = crate::content::get_blocking();
    super::trace::set_source(super::trace::Source::Eval);
    super::trace::force(Some(false));
    let mut results = Vec::new();
    for task in tasks().into_iter().filter(|t| want.as_ref().is_none_or(|w| w.iter().any(|x| x == t.name))) {
        let dir = out.join(task.name);
        println!("▶ {} ({}, {})", task.name, if provider == 1 { "Claude Code" } else { "Codex" }, if crew_mode { "crew" } else { "solo" });
        let (m, d) = (model.clone(), dir.join("project"));
        let cast = move |socket: PathBuf| crate::editor::agent_panel::casting(provider, m, Some(socket), d);
        match run_task(&task, &dir, cast, crew_mode, Duration::from_secs(minutes * 60), &mut |l| println!("{l}")) {
            Ok((checks, run)) => {
                let score = score_of(&checks);
                for c in &checks {
                    println!("  {} {:<22} {}", if c.pass { "✓" } else { "✗" }, c.name, c.detail);
                }
                println!("  = {score:.0}/100 in {:.0}s, {} tokens, {} tool calls", run.seconds, run.tokens, run.tools.len());
                let f = &run.friction;
                println!("    friction: {} calls to goal, retry {:.0}%, silent drop {:.0}%, undo {:.0}%, {} reply tokens", f["calls_to_goal"], f["retry_rate"].as_f64().unwrap_or(0.0) * 100.0, f["silent_drop_rate"].as_f64().unwrap_or(0.0) * 100.0, f["undo_rate"].as_f64().unwrap_or(0.0) * 100.0, f["reply_tokens"]);
                results.push(json!({
                    "task": task.name, "score": score, "seconds": run.seconds, "tokens": run.tokens, "tools": run.tools.len(),
                    "checks": checks.iter().map(|c| json!({"name": c.name, "pass": c.pass, "weight": c.weight, "detail": c.detail})).collect::<Vec<_>>(),
                    "plan": run.plan.as_ref().map(|p| p.text()), "errors": run.errors, "said": run.said, "friction": run.friction,
                }));
            }
            Err(e) => {
                println!("  setup failed: {e}");
                results.push(json!({"task": task.name, "score": 0.0, "error": e}));
            }
        }
        // Kept as it goes: a run cut short keeps the tasks it finished.
        let _ = std::fs::write(out.join("results.json"), serde_json::to_string_pretty(&json!({"results": results})).unwrap_or_default());
    }
    let mean = if results.is_empty() { 0.0 } else { results.iter().filter_map(|r| r["score"].as_f64()).sum::<f64>() / results.len() as f64 };
    let line = json!({"when": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()), "provider": provider, "model": model, "crew": crew_mode, "mean": mean, "tasks": results.iter().map(|r| json!({"task": r["task"], "score": r["score"], "seconds": r["seconds"], "friction": r["friction"]})).collect::<Vec<_>>()});
    let history = out.join("history.jsonl");
    let before = std::fs::read_to_string(&history).ok().and_then(|t| t.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()).filter(|v| v["crew"] == json!(crew_mode)).last());
    let _ = std::fs::write(out.join("results.json"), serde_json::to_string_pretty(&json!({"summary": line, "results": results})).unwrap_or_default());
    let mut h = std::fs::read_to_string(&history).unwrap_or_default();
    h.push_str(&format!("{line}\n"));
    let _ = std::fs::write(&history, h);
    match before.and_then(|b| b["mean"].as_f64()) {
        Some(b) => println!("mean {mean:.1}/100 (last run {b:.1}, {:+.1})", mean - b),
        None => println!("mean {mean:.1}/100"),
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::brain::Brain;
    use crate::agent::crew::Role;

    /// A brain that follows a script: each turn runs the calls given for
    /// its role.
    struct Scripted {
        role: Option<Role>,
        turns: usize,
    }

    impl Brain for Scripted {
        fn forget(&mut self) {}
        fn turn(&mut self, _request: &str, gate: &Gate) -> Result<(), String> {
            self.turns += 1;
            let call = |tool: &str, args: Value| gate.call_as(self.role, tool, &args);
            match (self.role, self.turns) {
                (Some(Role::Director), 1) => {
                    call("capabilities", json!({"query": "animate a fox"}))?;
                    call("plan", json!({"goal": "two cards and a grade", "steps": [
                        {"role": "editor", "title": "Red card", "task": "a red card at 0 for 2s"},
                        {"role": "animator", "title": "Blue card that moves", "task": "a blue card at 2s for 2s that slides in"},
                        {"role": "colour", "title": "Warm it", "task": "warm the red card", "after": [1], "checkpoint": true},
                    ]}))?;
                    gate.emit(AgentEvent::Text("The crew is on it.".into()));
                }
                (Some(Role::Director), 2) => {
                    call("summary", json!({}))?;
                    call("plan", json!({"redo": [{"step": 2, "note": "slide in from the right"}]}))?;
                }
                (Some(Role::Director), _) => {
                    call("plan", json!({"approve": true, "summary": "two cards, the blue one slides in, warm grade"}))?;
                    gate.emit(AgentEvent::Text("Done: two cards, warm.".into()));
                }
                (Some(Role::Editor), _) => {
                    call("edit", json!({"ops": [{"op": "card", "colour": "#ff0000", "at": 0, "duration": 2}]}))?;
                    gate.emit(AgentEvent::Text("Red card is clip 1.".into()));
                }
                (Some(Role::Animator), n) => {
                    if n == 1 {
                        call("edit", json!({"ops": [{"op": "card", "colour": "#0000ff", "at": 2, "duration": 2}]}))?;
                    }
                    if let Some(id) = card_id(gate, "0000ff") {
                        call("edit", json!({"ops": [{"op": "animate", "clip": id, "property": "x", "keys": [[0, if n == 1 { -600 } else { 600 }], [1, 0]]}]}))?;
                    }
                    // The colourist's job is refused.
                    assert!(call("edit", json!({"ops": [{"op": "match_colour", "clip": 1, "to": 2}]})).err().is_some_and(|e| e.contains("colourist")));
                    gate.emit(AgentEvent::Text(format!("Blue card slides in (pass {n}).")));
                }
                (Some(Role::Colour), _) => {
                    let first = gate.board.snapshot();
                    assert!(first.steps[0].status == crew::Status::Done, "the grade waits for the cut");
                    let red = card_id(gate, "ff0000").ok_or("no red card")?;
                    // The animator's card is not the colourist's to touch.
                    if let Some(blue) = card_id(gate, "0000ff") {
                        if gate.board.snapshot().steps[1].status == crew::Status::Running {
                            assert!(call("edit", json!({"ops": [{"op": "set", "clip": blue, "temperature": 30}]})).is_err());
                        }
                    }
                    call("edit", json!({"ops": [{"op": "set", "clip": red, "temperature": 30}]}))?;
                    gate.emit(AgentEvent::Text(format!("Warmed clip {red}.")));
                }
                _ => {}
            }
            Ok(())
        }
    }

    /// The id of the card whose name has `hex` in it.
    fn card_id(gate: &Gate, hex: &str) -> Option<u32> {
        let parts = gate.call("summary", &json!({})).ok()?;
        let text = parts.into_iter().find_map(|p| if let crate::agent::Content::Text(t) = p { Some(t) } else { None })?;
        let v: Value = serde_json::from_str(&text).ok()?;
        v["clips"].as_array()?.iter().find(|c| c["name"].as_str().is_some_and(|n| n.contains(hex))).and_then(|c| c["id"].as_u64()).map(|n| n as u32)
    }

    fn scripted() -> crew::Casting {
        Box::new(|role| Ok(Box::new(Scripted { role, turns: 0 }) as Box<dyn Brain>))
    }

    /// The whole crew, offline: the director plans, three specialists work
    /// (two side by side, one after the cut, approved at its checkpoint),
    /// the director sends one step back, then approves.
    #[test]
    fn a_crew_plans_works_in_parallel_reviews_and_approves() {
        let task = Task { name: "scripted", brief: "two cards", steer: None, setup: |_, _| Ok(()), score: |s, _| vec![check("clips", s.doc.project.clips.len() >= 2, 1.0, "")] };
        let dir = std::env::temp_dir().join(format!("studio-eval-crew-{}", std::process::id()));
        let mut lines = Vec::new();
        let (checks, run) = run_task(&task, &dir, |_| scripted(), true, Duration::from_secs(300), &mut |l| lines.push(l.to_owned())).unwrap();
        let plan = run.plan.expect("a plan was shown");
        assert!(plan.crew && plan.steps.len() == 3, "{}", plan.text());
        assert!(plan.steps.iter().all(|s| s.status == crew::Status::Done), "{}", plan.text());
        assert_eq!(plan.round, 2, "one review sent a step back, the second approved");
        assert!(plan.steps[1].notes.iter().any(|n| n.contains("pass 2")), "the animator redid its step: {:?}", plan.steps[1].notes);
        assert!(plan.verdict.as_deref().is_some_and(|v| v.contains("warm")));
        assert!(lines.iter().any(|l| l.contains("Checkpoint")), "the checkpoint was asked: {lines:?}");
        assert!(run.tools.iter().filter(|t| *t == "edit").count() >= 4);
        assert!(run.said.contains("Done: two cards"), "the director's words are the conversation: {}", run.said);
        assert!(!run.said.contains("Red card is clip"), "a specialist's words go under its step");
        assert_eq!(score_of(&checks[..1]), 100.0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A correction sent mid-run reaches a solo agent at its next call.
    #[test]
    fn a_correction_reaches_a_running_agent() {
        struct Slow;
        impl Brain for Slow {
            fn forget(&mut self) {}
            fn turn(&mut self, _: &str, gate: &Gate) -> Result<(), String> {
                let mut heard = None;
                for i in 0..8 {
                    match gate.call("edit", &json!({"ops": [{"op": "text", "text": "NIGHT RIDE", "at": i, "duration": 1}]})) {
                        Err(e) if e.starts_with("NOT RUN") => {
                            heard = Some(e);
                            break;
                        }
                        _ => {}
                    }
                }
                let heard = heard.ok_or("never heard the correction")?;
                assert!(heard.contains("DAWN RIDE"));
                gate.call("edit", &json!({"ops": [{"op": "text", "text": "DAWN RIDE", "at": 0, "duration": 6}]}))?;
                Ok(())
            }
        }
        let task = Task { name: "steer", brief: "title", steer: Some((3, "Make it DAWN RIDE")), setup: |_, _| Ok(()), score: |s, r| vec![check("dawn", s.doc.project.clips.iter().any(|c| c.text.contains("DAWN")), 1.0, ""), check("steered", r.steered, 1.0, "")] };
        let dir = std::env::temp_dir().join(format!("studio-eval-steer-{}", std::process::id()));
        let (checks, _) = run_task(&task, &dir, |_| Box::new(|_| Ok(Box::new(Slow) as Box<dyn Brain>)), false, Duration::from_secs(60), &mut |_| {}).unwrap();
        assert!(checks.iter().take(2).all(|c| c.pass), "{checks:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scores_are_weighted() {
        let c = vec![check("a", true, 3.0, ""), check("b", false, 1.0, "")];
        assert_eq!(score_of(&c), 75.0);
        assert_eq!(tasks().len(), 5);
    }
}
