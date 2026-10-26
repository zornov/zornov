// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! AI actions that change the edit: auto captions, voice cleanup and beat
//! markers. Each runs as a `studio-ai` job off the UI thread; `poll` applies
//! finished results to the document as one undo step each.

use std::path::PathBuf;

use studio_ai::features::denoise::{CleanupSettings, VoiceCleanup};
use studio_ai::features::transcribe::{TranscribeOptions, Transcript, Whisper};
use studio_ai::{JobHandle, Jobs, ModelStore};

use super::{Outcome, Refusal};
use crate::doc::Doc;
use crate::view::{self, ClipKind, LaneKind, Time, SEC};

/// What the AI panel's Run button does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Captions,
    VoiceCleanup,
    BeatMarkers,
}

impl Action {
    /// The action behind an AI panel tile, by its name.
    pub fn for_tool(name: &str) -> Option<Action> {
        match name {
            "Auto captions" => Some(Action::Captions),
            "Voice cleanup" => Some(Action::VoiceCleanup),
            "Beat markers" => Some(Action::BeatMarkers),
            _ => None,
        }
    }
    pub fn job_name(self) -> &'static str {
        match self {
            Action::Captions => "Auto captions",
            Action::VoiceCleanup => "Voice cleanup",
            Action::BeatMarkers => "Beat markers",
        }
    }
}

/// The clip an action works on, captured when it starts so later edits do
/// not change where results land.
#[derive(Clone, Debug)]
struct Target {
    clip: u32,
    path: String,
    start: Time,
    dur: Time,
    src_in: Time,
    speed: f64,
}

enum Running {
    Captions(Target, JobHandle<Transcript>),
    Cleanup(Target, JobHandle<PathBuf>),
    Beats(Target, JobHandle<Vec<i64>>),
}

impl Running {
    fn name(&self) -> &'static str {
        match self {
            Running::Captions(..) => Action::Captions.job_name(),
            Running::Cleanup(..) => Action::VoiceCleanup.job_name(),
            Running::Beats(..) => Action::BeatMarkers.job_name(),
        }
    }
    fn progress(&self) -> f32 {
        match self {
            Running::Captions(_, h) => h.progress(),
            Running::Cleanup(_, h) => h.progress(),
            Running::Beats(_, h) => h.progress(),
        }
    }
    fn finished(&self) -> bool {
        match self {
            Running::Captions(_, h) => h.is_finished(),
            Running::Cleanup(_, h) => h.is_finished(),
            Running::Beats(_, h) => h.is_finished(),
        }
    }
    fn cancel(&self) {
        match self {
            Running::Captions(_, h) => h.cancel(),
            Running::Cleanup(_, h) => h.cancel(),
            Running::Beats(_, h) => h.cancel(),
        }
    }
}

/// Owns the AI job workers and the jobs in flight.
pub struct AiRunner {
    store: ModelStore,
    jobs: Jobs,
    running: Vec<Running>,
}

impl Default for AiRunner {
    fn default() -> Self {
        Self::new(ModelStore::default())
    }
}

/// A running job as the UI shows it.
pub struct JobView {
    pub name: &'static str,
    pub progress: f32,
}

impl AiRunner {
    pub fn new(store: ModelStore) -> Self {
        Self { store, jobs: Jobs::new(1), running: Vec::new() }
    }

    pub fn is_running(&self, a: Action) -> bool {
        self.running.iter().any(|r| r.name() == a.job_name())
    }

    pub fn jobs(&self) -> Vec<JobView> {
        self.running.iter().map(|r| JobView { name: r.name(), progress: r.progress() }).collect()
    }

    pub fn cancel_all(&mut self) {
        for r in &self.running {
            r.cancel();
        }
    }

    /// Start `action` on the selected clip (or the first clip with sound).
    pub fn start(&mut self, doc: &Doc, action: Action, selection: &[u32]) -> Result<String, Refusal> {
        if self.is_running(action) {
            return Err(Refusal(format!("{} is already running", action.job_name())));
        }
        // Beats with no sound clip chosen: the whole mix, as it plays. (A
        // beat built from many short samples has no beat in any one clip;
        // the first clip with sound was one bark, and "No beats were found".)
        if action == Action::BeatMarkers && !selection.iter().filter_map(|id| doc.project.clip(*id)).any(|c| clip_has_sound(doc, c)) {
            let end = doc.project.clips.iter().map(|c| c.start + c.dur).max().unwrap_or(0);
            if end <= 0 {
                return Err(Refusal("Nothing on the timeline makes a sound".into()));
            }
            let model = doc.model.clone();
            let h = self.jobs.spawn(action.job_name(), move |_ctx| {
                let mix = crate::sound_bridge::mix_for_export(&model, 0, end);
                let a = studio_sound::beats::analyse_interleaved(&mix, crate::sound_bridge::RATE);
                Ok(a.beats.into_iter().filter(|&b| b < end).collect())
            });
            let whole = Target { clip: 0, path: String::new(), start: 0, dur: end, src_in: 0, speed: 1.0 };
            self.running.push(Running::Beats(whole, h));
            return Ok(format!("{} started on the whole mix", action.job_name()));
        }
        let target = target(doc, selection, action)?;
        let what = doc.project.clip(target.clip).map(|c| c.name.clone()).unwrap_or_default();
        match action {
            Action::Captions => {
                let whisper = Whisper::load_best(&self.store).map_err(|e| Refusal(e.to_string()))?;
                let t = target.clone();
                let h = self.jobs.spawn(action.job_name(), move |ctx| {
                    let (mono, rate) = mono_window(&t)?;
                    whisper.transcribe(&mono, rate, &TranscribeOptions::default(), ctx)
                });
                self.running.push(Running::Captions(target, h));
            }
            Action::VoiceCleanup => {
                let engine = VoiceCleanup::load(&self.store).unwrap_or_else(|_| VoiceCleanup::fallback());
                let t = target.clone();
                let h = self.jobs.spawn(action.job_name(), move |ctx| {
                    let (inter, rate) = stereo_window(&t)?;
                    let clean = engine.process(&inter, rate, 2, &CleanupSettings::default(), ctx)?;
                    let path = cleanup_path(&t);
                    if let Some(dir) = path.parent() {
                        std::fs::create_dir_all(dir).map_err(|e| studio_ai::Error::Io(e.to_string()))?;
                    }
                    let wav = studio_media::wav::Wav { sample_rate: rate, channels: 2, samples: clean };
                    std::fs::write(&path, studio_media::wav::encode_pcm16(&wav)).map_err(|e| studio_ai::Error::Io(e.to_string()))?;
                    Ok(path)
                });
                self.running.push(Running::Cleanup(target, h));
            }
            Action::BeatMarkers => {
                let t = target.clone();
                let h = self.jobs.spawn(action.job_name(), move |_ctx| {
                    let (inter, rate) = stereo_window(&t)?;
                    let a = studio_sound::beats::analyse_interleaved(&inter, rate);
                    // Clip-relative source times → sequence times.
                    Ok(a.beats.iter().map(|b| t.start + (*b as f64 / t.speed) as i64).filter(|x| *x < t.start + t.dur).collect())
                });
                self.running.push(Running::Beats(target, h));
            }
        }
        Ok(format!("{} started on {what}", action.job_name()))
    }

    /// Apply finished jobs to `doc`. Returns one message per finished job.
    pub fn poll(&mut self, doc: &mut Doc) -> Vec<Result<Outcome, Refusal>> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < self.running.len() {
            if !self.running[i].finished() {
                i += 1;
                continue;
            }
            let r = self.running.remove(i);
            out.push(match r {
                Running::Captions(t, h) => match h.take_result() {
                    Some(Ok(tr)) => add_captions(doc, &t, &tr),
                    Some(Err(e)) => Err(Refusal(e.to_string())),
                    None => Err(Refusal("Auto captions was cancelled".into())),
                },
                Running::Cleanup(t, h) => match h.take_result() {
                    Some(Ok(path)) => add_cleaned(doc, &t, path),
                    Some(Err(e)) => Err(Refusal(e.to_string())),
                    None => Err(Refusal("Voice cleanup was cancelled".into())),
                },
                Running::Beats(t, h) => match h.take_result() {
                    Some(Ok(beats)) => add_beats(doc, &t, &beats),
                    Some(Err(e)) => Err(Refusal(e.to_string())),
                    None => Err(Refusal("Beat markers was cancelled".into())),
                },
            });
        }
        out
    }
}

/// Whether a clip plays sound from a file (not a picture).
fn clip_has_sound(doc: &Doc, c: &view::Clip) -> bool {
    c.media.and_then(|m| doc.project.media.iter().find(|mi| mi.id == m)).is_some_and(|mi| mi.kind != view::MediaKind::Image)
}

fn target(doc: &Doc, selection: &[u32], action: Action) -> Result<Target, Refusal> {
    let p = &doc.project;
    let has_sound = |c: &view::Clip| c.media.and_then(|m| p.media.iter().find(|mi| mi.id == m)).is_some_and(|mi| mi.kind != view::MediaKind::Image);
    let clip = selection
        .iter()
        .filter_map(|id| p.clip(*id))
        .find(|c| has_sound(c))
        .or_else(|| {
            // Beat markers want music; captions and cleanup want speech.
            let prefer = if action == Action::BeatMarkers { ClipKind::Audio } else { ClipKind::Video };
            p.clips.iter().filter(|c| has_sound(c)).min_by_key(|c| (c.kind != prefer, c.start))
        })
        .ok_or_else(|| Refusal("Select a clip with sound first".into()))?;
    let media = clip.media.and_then(|m| p.media.iter().find(|mi| mi.id == m)).ok_or_else(|| Refusal("That clip has no media".into()))?;
    let path = media.path.clone().ok_or_else(|| Refusal("That clip's media is missing".into()))?;
    Ok(Target { clip: clip.id, path: path.to_string_lossy().into_owned(), start: clip.start, dur: clip.dur, src_in: clip.src_in, speed: (clip.speed as f64 / 100.0).max(0.01) })
}

/// The clip's portion of its media's sound, interleaved stereo.
fn stereo_window(t: &Target) -> studio_ai::Result<(Vec<f32>, u32)> {
    let d = crate::sound_bridge::decoded(&t.path).ok_or_else(|| studio_ai::Error::Io("This clip has no sound we can read".into()))?;
    let a = (t.src_in.max(0) as f64 / 1e6 * d.rate as f64) as usize;
    let len = (t.dur as f64 * t.speed / 1e6 * d.rate as f64) as usize;
    let a = a.min(d.left.len());
    let b = (a + len).min(d.left.len());
    let mut v = Vec::with_capacity((b - a) * 2);
    for i in a..b {
        v.push(d.left[i]);
        v.push(d.right[i]);
    }
    Ok((v, d.rate))
}

fn mono_window(t: &Target) -> studio_ai::Result<(Vec<f32>, u32)> {
    let (s, rate) = stereo_window(t)?;
    Ok((s.chunks_exact(2).map(|f| 0.5 * (f[0] + f[1])).collect(), rate))
}

fn cleanup_path(t: &Target) -> PathBuf {

    let stem = std::path::Path::new(&t.path).file_stem().and_then(|s| s.to_str()).unwrap_or("clip");
    studio_paths::studio().join("Cache").join("voice").join(format!("{stem} (clean) {}.wav", t.clip))
}

/// Source seconds in the media → sequence time, for this clip.
fn seq_time(t: &Target, media_secs: f64) -> Time {
    // The transcript is relative to the window start (src_in).
    t.start + (media_secs * 1e6 / t.speed) as Time
}

fn add_captions(doc: &mut Doc, t: &Target, tr: &Transcript) -> Result<Outcome, Refusal> {
    let segs: Vec<_> = tr.segments.iter().filter(|s| !s.text.trim().is_empty()).collect();
    if segs.is_empty() {
        return Err(Refusal("No speech was found in that clip".into()));
    }
    let lane_id = doc.alloc_id() as u32;
    let mut ids = Vec::new();
    let mut clips = Vec::new();
    for s in segs {
        let start = seq_time(t, s.start).max(t.start);
        let end = seq_time(t, s.end).min(t.start + t.dur);
        if end - start < SEC / 10 {
            continue;
        }
        let mut c = super::library::new_clip(start, end - start, ClipKind::Title, "Caption", None);
        c.text = s.text.trim().to_string();
        c.lane = lane_id;
        c.id = doc.alloc_id() as u32;
        ids.push(c.id);
        clips.push(c);
    }
    let n = clips.len();
    let ok = doc.edit("Auto captions", |p| {
        let colour = p.lanes.iter().find(|l| l.kind == LaneKind::Visual).map(|l| l.colour).unwrap_or(eframe::egui::Color32::from_rgb(110, 90, 160));
        let height = p.lanes.iter().find(|l| l.kind == LaneKind::Visual).map(|l| l.height).unwrap_or(44.0);
        p.lanes.insert(0, view::Lane { id: lane_id, name: "Captions".into(), kind: LaneKind::Visual, locked: false, hidden: false, muted: false, solo: false, height, colour });
        p.clips.extend(clips);
    });
    super::library::commit(ok, doc, "Auto captions", format!("Added {n} captions ({})", tr.language), ids)
}

fn add_cleaned(doc: &mut Doc, t: &Target, path: PathBuf) -> Result<Outcome, Refusal> {
    let report = super::project::import_media(doc, &[path]);
    let Some(&media) = report.ids.first() else { return Err(Refusal("The cleaned audio could not be imported".into())) };
    let Some(lane) = doc.project.lanes.iter().find(|l| l.kind == LaneKind::Audio).map(|l| l.id) else {
        return Err(Refusal("Add an audio lane for the cleaned voice".into()));
    };
    let mut c = super::library::new_clip(t.start, t.dur, ClipKind::Audio, "Voice (clean)", Some(media));
    c.lane = lane;
    c.id = doc.alloc_id() as u32;
    let new_id = c.id;
    let original = t.clip;
    let ok = doc.edit("Voice cleanup", |p| {
        if let Some(o) = p.clips.iter_mut().find(|c| c.id == original) {
            o.audio.enabled = false;
        }
        p.clips.push(c);
        p.resolve_overlap(new_id);
    });
    super::library::commit(ok, doc, "Voice cleanup", "Cleaned voice added; the original clip's sound is muted".into(), vec![new_id])
}

fn add_beats(doc: &mut Doc, _t: &Target, beats: &[i64]) -> Result<Outcome, Refusal> {
    if beats.is_empty() {
        return Err(Refusal("No beats were found".into()));
    }
    let ids: Vec<u64> = beats.iter().map(|_| doc.alloc_id()).collect();
    let n = beats.len();
    let ok = doc.edit("Beat markers", |p| {
        for (id, b) in ids.iter().zip(beats) {
            if !p.markers.iter().any(|m| (m.time - *b).abs() < SEC / 20) {
                p.markers.push(view::Marker { id: *id, time: *b, note: "Beat".into() });
            }
        }
        p.markers.sort_by_key(|m| m.time);
    });
    super::library::commit(ok, doc, "Beat markers", format!("Added {n} beat markers"), vec![])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("studio-ai-run-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d.join(name)
    }

    fn doc_with(path: &std::path::Path) -> Doc {
        let mut doc = crate::services::project::new_project(crate::services::project::canvas("Landscape").unwrap());
        crate::services::project::import_to_timeline(&mut doc, &[path.to_path_buf()]).unwrap();
        doc
    }

    fn run_to_end(r: &mut AiRunner, doc: &mut Doc) -> Result<Outcome, Refusal> {
        let t0 = Instant::now();
        loop {
            let mut out = r.poll(doc);
            if let Some(x) = out.pop() {
                return x;
            }
            assert!(t0.elapsed() < Duration::from_secs(300), "job timed out");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    #[test]
    fn beat_markers_land_on_a_click_track() {
        // 120 BPM clicks for 8 s.
        let rate = 48_000u32;
        let mut s = vec![0f32; rate as usize * 8 * 2];
        for k in 0..16 {
            let at = (k as f64 * 0.5 * rate as f64) as usize;
            for i in 0..400 {
                let v = (1.0 - i as f32 / 400.0) * (i as f32 * 0.7).sin();
                s[(at + i) * 2] = v;
                s[(at + i) * 2 + 1] = v;
            }
        }
        let p = tmp("clicks.wav");
        std::fs::write(&p, studio_media::wav::encode_pcm16(&studio_media::wav::Wav { sample_rate: rate, channels: 2, samples: s })).unwrap();
        let mut doc = doc_with(&p);
        let mut r = AiRunner::new(ModelStore::new(tmp("no-models")));
        r.start(&doc, Action::BeatMarkers, &[]).unwrap();
        let o = run_to_end(&mut r, &mut doc).expect("beats found");
        assert!(o.message.starts_with("Added"));
        let n = doc.project.markers.len();
        assert!((12..=17).contains(&n), "about one marker per click, got {n}");
        // Undo removes them in one step.
        doc.undo();
        assert_eq!(doc.project.markers.len(), 0);
        // A beat built from one short hit placed again and again (no clip
        // holds the beat; the mix does).
        let hit: Vec<f32> = (0..4800).flat_map(|i| { let v = (1.0 - i as f32 / 4800.0) * (i as f32 * 0.7).sin(); [v, v] }).collect();
        let hp = tmp("hit.wav");
        std::fs::write(&hp, studio_media::wav::encode_pcm16(&studio_media::wav::Wav { sample_rate: rate, channels: 2, samples: hit })).unwrap();
        let mut doc = crate::services::project::new_project(crate::services::project::canvas("Landscape").unwrap());
        crate::services::project::import_to_timeline(&mut doc, &[hp]).unwrap();
        let first = doc.project.clips[0].clone();
        let ids: Vec<u64> = (0..15).map(|_| doc.alloc_id()).collect();
        assert!(doc.edit("hits", |p| {
            for (k, id) in ids.iter().enumerate() {
                let mut c = first.clone();
                c.id = *id as u32;
                c.start = (k as i64 + 1) * 500_000;
                p.clips.push(c);
            }
        }));
        r.start(&doc, Action::BeatMarkers, &[]).unwrap();
        run_to_end(&mut r, &mut doc).expect("the mix has the beat");
        assert!(doc.project.markers.len() >= 10, "markers on the hits: {}", doc.project.markers.len());
    }

    #[test]
    fn voice_cleanup_adds_clean_audio_and_mutes_the_original() {
        let rate = 48_000u32;
        let mut seed = 1u32;
        let s: Vec<f32> = (0..rate * 3)
            .flat_map(|i| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let n = (seed >> 9) as f32 / (1u32 << 23) as f32 - 0.5;
                let v = 0.3 * (2.0 * std::f32::consts::PI * 220.0 * i as f32 / rate as f32).sin() + 0.05 * n;
                [v, v]
            })
            .collect();
        let p = tmp("noisy.wav");
        std::fs::write(&p, studio_media::wav::encode_pcm16(&studio_media::wav::Wav { sample_rate: rate, channels: 2, samples: s })).unwrap();
        let mut doc = doc_with(&p);
        let original = doc.project.clips[0].id;
        let lanes_audio = doc.project.lanes.iter().any(|l| l.kind == LaneKind::Audio);
        let mut r = AiRunner::new(ModelStore::new(tmp("no-models")));
        r.start(&doc, Action::VoiceCleanup, &[original]).unwrap();
        let res = run_to_end(&mut r, &mut doc);
        if !lanes_audio {
            assert!(res.is_err());
            return;
        }
        res.expect("cleanup applied");
        assert!(!doc.project.clip(original).unwrap().audio.enabled, "original muted");
        assert!(doc.project.clips.iter().any(|c| c.name == "Voice (clean)"));
    }

    /// Real Whisper run on speech made with macOS `say`; skipped when the
    /// model or `say` is not available.
    #[test]
    fn captions_from_real_speech() {
        let store = ModelStore::default();
        if Whisper::load_best(&store).is_err() {
            eprintln!("skipped: Whisper not installed");
            return;
        }
        let p = tmp("speech.wav");
        let ok = std::process::Command::new("say").args(["-o", p.to_str().unwrap(), "--data-format=LEI16@16000", "Hello and welcome to Studio. This is a caption test."]).status().map(|s| s.success()).unwrap_or(false);
        if !ok {
            eprintln!("skipped: say not available");
            return;
        }
        let mut doc = doc_with(&p);
        let mut r = AiRunner::new(store);
        r.start(&doc, Action::Captions, &[]).unwrap();
        let o = run_to_end(&mut r, &mut doc).expect("captions added");
        let caps: Vec<_> = doc.project.clips.iter().filter(|c| c.name == "Caption").collect();
        assert!(!caps.is_empty(), "{}", o.message);
        let text = caps.iter().map(|c| c.text.to_lowercase()).collect::<Vec<_>>().join(" ");
        assert!(text.contains("welcome") && text.contains("caption"), "transcript: {text}");
        assert!(doc.project.lanes.first().is_some_and(|l| l.name == "Captions"));
    }
}
