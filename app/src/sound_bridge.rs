// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Timeline sound: decodes the audio of every clip, builds a `studio-sound`
//! mix plan from the model, plays it live in step with the playhead, and
//! mixes it offline for export.
//!
//! Decoded audio is cached per file (shared, read-only), so rebuilding the
//! plan after an edit is cheap: clip sources are views into the cache.

use std::collections::HashMap;
use std::path::Path;
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use studio_model as m;
use studio_sound::automation::{Curve, FadeShape};
use studio_sound::mixer::{AudioSource, ClipSpec, EffectSpec, MixPlan, Mixer, SpeedSpec};
use studio_sound::resample::Quality;

/// Output rate of the live player and of exported sound.
pub const RATE: u32 = studio_audio::RATE;

/// A file's audio, decoded once: planar, at its own rate.
pub struct Decoded {
    pub rate: u32,
    pub mono: bool,
    pub left: Arc<[f32]>,
    pub right: Arc<[f32]>,
}

fn cache() -> &'static Mutex<HashMap<String, Option<Arc<Decoded>>>> {
    static C: OnceLock<Mutex<HashMap<String, Option<Arc<Decoded>>>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The decoded audio of `path`, or `None` if it has none we can read.
/// Decodes on first use (the caller's thread) and caches the result.
pub fn decoded(path: &str) -> Option<Arc<Decoded>> {
    if let Some(hit) = cache().lock().unwrap_or_else(|e| e.into_inner()).get(path) {
        return hit.clone();
    }
    let d = decode_file(Path::new(path)).map(Arc::new);
    cache().lock().unwrap_or_else(|e| e.into_inner()).insert(path.to_string(), d.clone());
    d
}

/// Decodes a file without keeping it in the playback cache (for one-off
/// analysis of many files).
pub fn decode_uncached(path: &Path) -> Option<Decoded> {
    decode_file(path)
}

fn decode_file(path: &Path) -> Option<Decoded> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let wav = match ext.as_str() {
        "wav" | "wave" => studio_media::wav::read(path).ok()?,
        "mp3" => studio_media::mp3::read(path).ok()?,
        "ogg" | "oga" => studio_media::vorbis::read(path).ok()?,
        "flac" => {
            let f = studio_media::flac::decode(&std::fs::read(path).ok()?).ok()?;
            let scale = 1.0 / (1u64 << (f.bits_per_sample.clamp(4, 32) - 1)) as f32;
            studio_media::wav::Wav { sample_rate: f.sample_rate, channels: f.channels, samples: f.samples.iter().map(|&v| v as f32 * scale).collect() }
        }
        "aif" | "aiff" | "aifc" => {
            let a = studio_media::aiff::parse(&std::fs::read(path).ok()?).ok()?;
            studio_media::wav::Wav { sample_rate: a.sample_rate.round() as u32, channels: a.channels, samples: a.samples }
        }
        "mp4" | "mov" | "m4a" | "m4v" | "aac" => decode_mp4_aac(path).or_else(|| crate::services::media_meta::pcm(path))?,
        _ => return None,
    };
    from_interleaved(wav.sample_rate, wav.channels as usize, &wav.samples)
}

fn from_interleaved(rate: u32, channels: usize, samples: &[f32]) -> Option<Decoded> {
    if rate == 0 || channels == 0 || samples.is_empty() {
        return None;
    }
    let frames = samples.len() / channels;
    let mut left = Vec::with_capacity(frames);
    let mut right = Vec::with_capacity(frames);
    for f in samples.chunks_exact(channels) {
        left.push(f[0]);
        right.push(if channels > 1 { f[1] } else { f[0] });
    }
    Some(Decoded { rate, mono: channels == 1, left: left.into(), right: right.into() })
}

/// Decodes the first AAC track of an MP4/MOV, dropping the encoder priming
/// the container's edit list signals.
fn decode_mp4_aac(path: &Path) -> Option<studio_media::wav::Wav> {
    let mut dmx = studio_mp4::open(path).ok()?;
    let track = dmx.movie().tracks.iter().find(|t| matches!(t.codec, studio_mp4::Codec::Aac { .. }))?.clone();
    let studio_mp4::Codec::Aac { asc, .. } = &track.codec else { return None };
    let mut dec = studio_aac::Decoder::new(asc).ok()?;
    let mut out: Vec<f32> = Vec::new();
    for s in &track.samples {
        let Ok(data) = dmx.read_sample(s) else { continue };
        if let Ok(pcm) = dec.decode_frame(&data) {
            out.extend_from_slice(pcm);
        }
    }
    let channels = dec.channels().max(1);
    let rate = dec.sample_rate().max(1);
    // Priming: the edit list's media time is in track timescale units.
    let skip_media = track.edits.first().map(|e| e.media_time.max(0)).unwrap_or(0) as u64;
    let skip_frames = if track.timescale > 0 { skip_media * rate as u64 / track.timescale as u64 } else { 0 } as usize;
    let skip = (skip_frames * channels).min(out.len());
    Some(studio_media::wav::Wav { sample_rate: rate, channels: channels as u16, samples: out[skip..].to_vec() })
}

/// A clip's view into a cached decode.
struct SharedSource(Arc<Decoded>);

impl AudioSource for SharedSource {
    fn rate(&self) -> u32 {
        self.0.rate
    }
    fn channels(&self) -> usize {
        if self.0.mono { 1 } else { 2 }
    }
    fn frames(&self) -> Option<u64> {
        Some(self.0.left.len() as u64)
    }
    fn read(&mut self, frame: u64, left: &mut [f32], right: &mut [f32]) -> usize {
        let total = self.0.left.len() as u64;
        if frame >= total {
            return 0;
        }
        let n = (left.len() as u64).min(total - frame) as usize;
        let a = frame as usize;
        left[..n].copy_from_slice(&self.0.left[a..a + n]);
        right[..n].copy_from_slice(&self.0.right[a..a + n]);
        n
    }
}

/// The ramp put on hard sound edges so cuts do not click.
const DECLICK: m::Micros = 4_000;

/// Whether `clip` runs straight on from (`after` false) or into (`after`
/// true) a neighbour on its lane playing the next moment of the same file.
fn continues(seq: &m::Sequence, clip: &m::Clip, after: bool) -> bool {
    let m::ClipSource::Media(r) = &clip.source else { return false };
    seq.clips.iter().any(|o| {
        let m::ClipSource::Media(or) = &o.source else { return false };
        if o.id == clip.id || o.lane != clip.lane || or.media != r.media || o.reversed != clip.reversed {
            return false;
        }
        let (a, b) = if after { (clip, o) } else { (o, clip) };
        a.end() == b.start && (a.source_out() - b.source_in).abs() < 2_000
    })
}

/// A clip property's keyframes as a mixer curve over clip time: eased
/// stretches sampled every 10 ms, so they sound as they look; straight
/// ones are the mixer's own straight line between the two keys (`map` is
/// a straight scaling), and a held key steps. A ducked bed under a long
/// film is thousands of keys, made into a curve on every edit.
fn keyed(clip: &m::Clip, path: &str, rest: f32, map: impl Fn(f64) -> f32) -> Curve {
    let Some(track) = clip.keyframes.get(path).filter(|t| t.keys.len() >= 2) else {
        return Curve::constant(rest);
    };
    let (first, last) = (track.keys[0].time, track.keys[track.keys.len() - 1].time);
    let mut keys = Vec::new();
    let mut push = |t: m::Micros, v: Option<m::Value>| {
        if let Some(v) = v.and_then(|v| v.as_f64()) {
            keys.push(studio_sound::automation::Key::new(t, map(v)));
        }
    };
    if !track.modifiers.is_empty() {
        let mut t = first;
        while t < last {
            push(t, track.eval(t));
            t += 10_000;
        }
    } else {
        for w in track.keys.windows(2) {
            let (k0, k1) = (&w[0], &w[1]);
            push(k0.time, Some(k0.value.clone()));
            match k0.interp {
                m::Interp::Linear => {}
                m::Interp::Hold => push(k1.time - 1, Some(k0.value.clone())),
                _ => {
                    let mut t = k0.time + 10_000;
                    while t < k1.time {
                        push(t, track.eval(t));
                        t += 10_000;
                    }
                }
            }
        }
    }
    push(last, track.eval(last));
    Curve::new(keys, rest)
}

/// A sound effect for the mixer, from its settings (any left out keep the
/// effect's defaults). Clip, lane, bus and master effects share these.
pub(crate) fn audio_effect(e: &m::Effect) -> Option<EffectSpec> {
    let f = |k: &str| e.params.get(k).and_then(|v| v.as_f64()).map(|v| v as f32);
    use studio_sound::dynamics::{CompressorParams, DeEsserParams, GateParams, LimiterParams};
    Some(match e.kind.rsplit('.').next().unwrap_or(&e.kind) {
        "eq" => {
            let EffectSpec::Eq { mut bands, .. } = EffectSpec::flat_eq() else { return None };
            bands[0].kind = studio_sound::eq::BandKind::LowShelf;
            bands[4].kind = studio_sound::eq::BandKind::HighShelf;
            for (i, k) in ["bass", "low_mid", "mid", "presence", "air"].iter().enumerate() {
                if let Some(g) = f(k) {
                    bands[i].gain_db = g.clamp(-24.0, 24.0);
                }
            }
            EffectSpec::Eq { bands, high_pass: f("high_pass").filter(|h| *h > 0.0), low_pass: f("low_pass").filter(|h| *h > 0.0) }
        }
        "compressor" => {
            let mut p = CompressorParams::default();
            p.threshold_db = f("threshold").unwrap_or(p.threshold_db);
            p.ratio = f("ratio").unwrap_or(p.ratio).max(1.0);
            p.attack_ms = f("attack").unwrap_or(p.attack_ms);
            p.release_ms = f("release").unwrap_or(p.release_ms);
            p.makeup_db = f("makeup").unwrap_or(p.makeup_db);
            EffectSpec::Compressor(p)
        }
        "limiter" => {
            let mut p = LimiterParams::default();
            p.ceiling_db = f("ceiling").unwrap_or(p.ceiling_db).min(0.0);
            EffectSpec::Limiter(p)
        }
        "gate" => {
            let mut p = GateParams::default();
            p.threshold_db = f("threshold").unwrap_or(p.threshold_db);
            p.range_db = f("range").unwrap_or(p.range_db);
            EffectSpec::Gate(p)
        }
        "de_esser" => {
            let mut p = DeEsserParams::default();
            p.threshold_db = f("threshold").unwrap_or(p.threshold_db);
            p.freq_hz = f("freq").unwrap_or(p.freq_hz);
            EffectSpec::DeEsser(p)
        }
        "reverb" => {
            // A room preset as the starting point, then any settings given.
            let room = match e.params.get("room") {
                Some(m::Value::Choice(r) | m::Value::Text(r)) => studio_sound::reverb::ReverbParams::room(r),
                _ => None,
            };
            let mut p = room.unwrap_or_default();
            p.size = f("size").unwrap_or(p.size);
            p.decay = f("decay").unwrap_or(p.decay);
            p.mix = f("mix").unwrap_or(p.mix).clamp(0.0, 1.0);
            p.pre_delay_ms = f("pre_delay").unwrap_or(p.pre_delay_ms);
            EffectSpec::Reverb(p)
        }
        "delay" => {
            let mut p = studio_sound::delay::DelayParams::default();
            if let Some(t) = f("time") {
                p.time_left_ms = t;
                p.time_right_ms = t;
            }
            p.feedback = f("feedback").unwrap_or(p.feedback).clamp(0.0, 0.95);
            p.mix = f("mix").unwrap_or(p.mix).clamp(0.0, 1.0);
            EffectSpec::Delay(p)
        }
        "pitch_shift" => EffectSpec::PitchShift { semitones: f("semitones").unwrap_or(0.0).clamp(-24.0, 24.0) },
        _ => return None,
    })
}

fn fade_shape(c: m::FadeCurve) -> FadeShape {
    match c {
        m::FadeCurve::Linear => FadeShape::Linear,
        m::FadeCurve::EqualPower => FadeShape::EqualPower,
        m::FadeCurve::SCurve => FadeShape::SCurve,
        m::FadeCurve::Exponential => FadeShape::Slow,
        m::FadeCurve::Logarithmic => FadeShape::Fast,
    }
}

/// The mix plan of the project's active sequence as the desk sets it up
/// (buses, sends, automation, layout, delivery gain). Clips whose media has
/// no readable audio are left out (they are silent, not errors).
pub fn plan(project: &m::Project) -> MixPlan {
    plan_for(project, &crate::sound_desk::mix::MixRequest::default())
}

/// What the mix is made from, as one number: the settings (the sound
/// desk), the media, every sequence's lanes and transitions, and every clip
/// that can sound (nested sequences are heard through their clip). A clip
/// with its sound off counts only by its place (a neighbour's declick
/// looks at it), not its keys. Edits to pictures alone leave it the same.
fn sound_key(project: &m::Project) -> u64 {
    use std::hash::Hasher;
    struct Hash(std::collections::hash_map::DefaultHasher);
    impl std::io::Write for Hash {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.write(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut h = Hash(Default::default());
    let _ = serde_json::to_writer(&mut h, &(project.active_sequence, &project.settings, &project.media));
    for seq in &project.sequences {
        let _ = serde_json::to_writer(&mut h, &(seq.id, &seq.lanes, &seq.transitions));
        for c in &seq.clips {
            let _ = match &c.source {
                m::ClipSource::Media(r) if !r.audio => serde_json::to_writer(&mut h, &(c.id, c.lane, c.start, c.duration, c.source_in, &c.source)),
                m::ClipSource::Media(_) | m::ClipSource::Sequence(_) => serde_json::to_writer(&mut h, c),
                _ => Ok(()),
            };
        }
    }
    h.0.finish()
}

/// The mix plan for one variant: a layout, a language's mix, a stem.
pub fn plan_for(project: &m::Project, req: &crate::sound_desk::mix::MixRequest) -> MixPlan {
    let mut plan = MixPlan::default();
    let Some(seq) = project.sequence(project.active_sequence) else { return plan };
    crate::sound_desk::mix::apply(project, &mut plan, req);
    for clip in &seq.clips {
        if !clip.enabled || clip.audio.muted {
            continue;
        }
        // Merged clips and nested sequences: their sounds, through the
        // outer clip (they were silent in the mix and in exports).
        if let m::ClipSource::Sequence(r) = &clip.source {
            let speed = average_speed(clip);
            let w = Window {
                id: clip.id.0,
                lane: clip.lane.0,
                at: clip.start,
                from: clip.source_in,
                to: clip.source_in + (clip.duration as f64 * speed) as m::Micros,
                speed,
                volume: (keyed(clip, "audio.volume_db", clip.audio.volume_db as f32, |v| v as f32), clip.start),
                effects: clip.audio.effects.iter().filter(|e| e.enabled).filter_map(audio_effect).collect(),
                fades: (clip.audio.fade_in.duration, clip.audio.fade_out.duration),
            };
            nested(project, &mut plan, r.sequence, &w, 0);
            continue;
        }
        let Some(spec) = media_spec(project, seq, clip) else { continue };
        plan.clips.push(spec);
    }
    plan
}

/// A clip's average speed (source time per timeline time).
fn average_speed(clip: &m::Clip) -> f64 {
    match &clip.speed {
        m::Speed::Constant(s) => *s,
        m::Speed::Curve(_) => (clip.speed.integrate(clip.duration) / clip.duration.max(1) as f64).max(0.01),
    }
}

/// Where a nested sequence's sound lands: the part `from..to` of it plays
/// from `at` on the outer timeline, `speed` times as fast, on the outer
/// clip's lane, with its volume (a curve over outer clip time, and that
/// clip's start), sound effects and fades on top.
struct Window {
    id: u64,
    lane: u64,
    at: m::Micros,
    from: m::Micros,
    to: m::Micros,
    speed: f64,
    volume: (Curve, m::Micros),
    effects: Vec<EffectSpec>,
    fades: (m::Micros, m::Micros),
}

/// The inner volume (a curve over inner clip time, starting `skip` in)
/// plus the outer one, over the piece's own time (outer units).
fn summed(inner: &Curve, skip: m::Micros, w: &Window, start: m::Micros, dur: m::Micros) -> Curve {
    let outer_at = |t: m::Micros| w.volume.0.eval(start + t - w.volume.1);
    if inner.is_constant() && w.volume.0.is_constant() {
        return Curve::constant(inner.eval(0) + outer_at(0));
    }
    let mut keys = Vec::new();
    let mut t = 0;
    while t <= dur {
        keys.push(studio_sound::automation::Key::new(t, inner.eval(skip + (t as f64 * w.speed) as m::Micros) + outer_at(t)));
        t += 10_000;
    }
    Curve::new(keys, inner.eval(0) + outer_at(0))
}

fn nested(project: &m::Project, plan: &mut MixPlan, seq_id: m::SequenceId, w: &Window, depth: u32) {
    let Some(seq) = project.sequence(seq_id).filter(|_| depth < 8) else { return };
    for clip in &seq.clips {
        let lane_off = seq.lanes.iter().find(|l| l.id == clip.lane).is_some_and(|l| l.muted);
        if !clip.enabled || clip.audio.muted || lane_off {
            continue;
        }
        let (a, b) = (clip.start.max(w.from), clip.end().min(w.to));
        if b <= a {
            continue;
        }
        let start = w.at + ((a - w.from) as f64 / w.speed).round() as m::Micros;
        let dur = (((b - a) as f64) / w.speed).round() as m::Micros;
        let speed = average_speed(clip);
        let id = w.id.wrapping_mul(1_000_003) ^ clip.id.0;
        match &clip.source {
            m::ClipSource::Sequence(r) => {
                let from = clip.source_in + ((a - clip.start) as f64 * speed) as m::Micros;
                let inner = keyed(clip, "audio.volume_db", clip.audio.volume_db as f32, |v| v as f32);
                let mut effects: Vec<EffectSpec> = clip.audio.effects.iter().filter(|e| e.enabled).filter_map(audio_effect).collect();
                effects.extend(w.effects.iter().cloned());
                let inner_w = Window {
                    id,
                    lane: w.lane,
                    at: start,
                    from,
                    to: from + ((b - a) as f64 * speed) as m::Micros,
                    speed: w.speed * speed,
                    volume: (summed(&inner, a - clip.start, w, start, dur), start),
                    effects,
                    fades: (if a == w.from { w.fades.0 } else { 0 }, if b == w.to { w.fades.1 } else { 0 }),
                };
                nested(project, plan, r.sequence, &inner_w, depth + 1);
            }
            m::ClipSource::Media(_) => {
                let Some(mut spec) = media_spec(project, seq, clip) else { continue };
                spec.id = id;
                spec.lane = w.lane;
                spec.start = start;
                spec.duration = dur;
                if !clip.reversed {
                    spec.source_in = clip.source_in + ((a - clip.start) as f64 * speed) as m::Micros;
                }
                spec.speed = SpeedSpec::Constant(speed * w.speed);
                spec.volume_db = summed(&spec.volume_db, a - clip.start, w, start, dur);
                spec.pan.shift(-(((a - clip.start) as f64 / w.speed) as m::Micros));
                // Cut by the outer clip's edges: declick there, or its fade.
                spec.fade_in = if a > clip.start { DECLICK } else { (spec.fade_in as f64 / w.speed) as m::Micros };
                spec.fade_out = if b < clip.end() { DECLICK } else { (spec.fade_out as f64 / w.speed) as m::Micros };
                if a == w.from && w.fades.0 > 0 {
                    spec.fade_in = spec.fade_in.max(w.fades.0);
                }
                if b == w.to && w.fades.1 > 0 {
                    spec.fade_out = spec.fade_out.max(w.fades.1);
                }
                spec.effects.extend(w.effects.iter().cloned());
                plan.clips.push(spec);
            }
            _ => {}
        }
    }
}

/// One media clip's sound for the mixer, as it sits in `seq`.
fn media_spec(project: &m::Project, seq: &m::Sequence, clip: &m::Clip) -> Option<ClipSpec> {
    {
        let m::ClipSource::Media(mref) = &clip.source else { return None };
        if !mref.audio {
            return None;
        }
        let item = project.media_item(mref.media)?;
        let dec = decoded(&item.path)?;
        let mut spec = ClipSpec::new(clip.id.0, clip.lane.0, clip.start, clip.duration, Box::new(SharedSource(dec)));
        spec.source_in = if clip.reversed { clip.source_out() } else { clip.source_in };
        spec.reversed = clip.reversed;
        spec.speed = match &clip.speed {
            m::Speed::Constant(s) => SpeedSpec::Constant(*s),
            // Speed curves: the average rate (source used / clip length)
            // keeps sound and picture exactly the same length.
            m::Speed::Curve(_) => SpeedSpec::Constant((clip.speed.integrate(clip.duration) / clip.duration.max(1) as f64).max(0.01)),
        };
        spec.preserve_pitch = clip.audio.preserve_pitch;
        // Volume and pan follow their keyframes (ducking, rides, swells).
        spec.volume_db = keyed(clip, "audio.volume_db", clip.audio.volume_db as f32, |v| v as f32);
        spec.pan = keyed(clip, "audio.pan", clip.audio.pan as f32, |v| v as f32);
        spec.fade_in = clip.audio.fade_in.duration;
        spec.fade_out = clip.audio.fade_out.duration;
        // One shape for both ramps: the fade-in's, or the fade-out's when
        // only that is set.
        spec.fade_shape = fade_shape(if clip.audio.fade_in.duration > 0 { clip.audio.fade_in.curve } else { clip.audio.fade_out.curve });
        // A hard edge in the middle of a sound clicks: a few milliseconds of
        // ramp at every edge that has no fade, unless the clip simply
        // continues into (or out of) the next piece of the same recording.
        if spec.fade_in == 0 && !continues(seq, clip, false) {
            spec.fade_in = DECLICK;
        }
        if spec.fade_out == 0 && !continues(seq, clip, true) {
            spec.fade_out = DECLICK;
        }
        spec.effects = clip.audio.effects.iter().filter(|e| e.enabled).filter_map(audio_effect).collect();
        Some(spec)
    }
}

/// Mixes `[start, end)` of the project for export: interleaved stereo at
/// [`RATE`] (a stereo mix even when the desk is set up for surround).
pub fn mix_for_export(project: &m::Project, start: m::Micros, end: m::Micros) -> Vec<f32> {
    let req = crate::sound_desk::mix::MixRequest::delivered(studio_sound::surround::Layout::Stereo);
    let (l, r) = studio_sound::mixer::mix_offline(plan_for(project, &req), RATE, start, end, Quality::Best);
    let mut out = Vec::with_capacity(l.len() * 2);
    for (a, b) in l.iter().zip(r.iter()) {
        out.push(*a);
        out.push(*b);
    }
    out
}

/// The live mixer's latest meters and when they were read.
fn live() -> &'static Mutex<Option<(studio_sound::Meters, std::time::Instant)>> {
    static M: OnceLock<Mutex<Option<(studio_sound::Meters, std::time::Instant)>>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(None))
}

/// What the playing mix measures now (peaks per lane, bus and channel,
/// momentary / short-term / integrated loudness); `None` when nothing has
/// played in the last half second.
pub fn live_meters() -> Option<studio_sound::Meters> {
    let g = live().lock().unwrap_or_else(|e| e.into_inner());
    g.as_ref().filter(|(_, at)| at.elapsed() < Duration::from_millis(500)).map(|(m, _)| m.clone())
}

enum Cmd {
    Plan(MixPlan),
    Play(m::Micros),
    Pause,
}

/// Live playback: a thread owning the mixer and the output device.
pub struct LivePlayer {
    tx: Sender<Cmd>,
    /// The project edit the plan was made from.
    edits: u64,
    /// What that plan was made from ([`sound_key`]).
    sound: Option<u64>,
    playing: bool,
    started_at: Option<(std::time::Instant, m::Micros, f32)>,
}

impl LivePlayer {
    pub fn new() -> Self {
        let (tx, rx) = channel();
        let _ = std::thread::Builder::new().name("studio-sound".into()).spawn(move || run(rx));
        Self::with_sender(tx)
    }

    /// A player whose commands go to `tx` (the sound thread, or a test).
    fn with_sender(tx: Sender<Cmd>) -> Self {
        Self { tx, edits: u64::MAX, sound: None, playing: false, started_at: None }
    }

    /// Call once per UI frame with the editor's transport state and the
    /// project's edit count (`Doc::edits`). Starts, stops, re-seeks after a
    /// jump, and swaps in a new plan after an edit. The new plan joins the
    /// sound already playing (the mixer keeps its place and its tails), so
    /// edits during playback, by hand or by an agent, never cut the sound.
    pub fn sync(&mut self, project: &m::Project, edits: u64, playing: bool, playhead: m::Micros, rate: f32) {
        if edits != self.edits {
            self.edits = edits;
            // A picture moved, a title typed: the sound is the same and its
            // plan (every volume ride sampled) is not made again.
            let key = sound_key(project);
            if self.sound != Some(key) {
                self.sound = Some(key);
                let _ = self.tx.send(Cmd::Plan(plan(project)));
            }
        }
        // Only normal-speed forward playback is heard (shuttle at other
        // rates stays silent rather than out of sync).
        let audible = playing && (rate - 1.0).abs() < 1e-3;
        if audible && !self.playing {
            self.playing = true;
            self.started_at = Some((std::time::Instant::now(), playhead, rate));
            let _ = self.tx.send(Cmd::Play(playhead));
        } else if !audible && self.playing {
            self.playing = false;
            self.started_at = None;
            let _ = self.tx.send(Cmd::Pause);
        } else if audible {
            // A scrub or loop jump while playing: re-seek when the playhead
            // is more than 80 ms away from where the audio clock says.
            if let Some((t0, p0, r)) = self.started_at {
                let expect = p0 + (t0.elapsed().as_secs_f64() * r as f64 * 1e6) as m::Micros;
                if (expect - playhead).abs() > 80_000 {
                    self.started_at = Some((std::time::Instant::now(), playhead, rate));
                    let _ = self.tx.send(Cmd::Play(playhead));
                }
            }
        }
    }
}

impl Default for LivePlayer {
    fn default() -> Self {
        Self::new()
    }
}

fn run(rx: Receiver<Cmd>) {
    const BLOCK: usize = 1024;
    const AHEAD: usize = RATE as usize / 10; // keep ~100 ms queued
    let out = studio_audio::AudioOut::open().ok();
    let mut mixer: Option<Mixer> = None;
    let mut playing = false;
    let mut buf = vec![0f32; BLOCK * 2];
    loop {
        let wait = if playing { Duration::from_millis(5) } else { Duration::from_millis(200) };
        match rx.recv_timeout(wait) {
            Ok(Cmd::Plan(p)) => match &mut mixer {
                Some(mx) => mx.update(p),
                None => mixer = Some(Mixer::new(RATE, BLOCK, Quality::High, p)),
            },
            Ok(Cmd::Play(t)) => {
                if let Some(o) = &out {
                    o.clear();
                }
                if let Some(mx) = &mut mixer {
                    // Start the mixer early by its look-ahead so sound and
                    // picture line up.
                    let lat = mx.master_latency() as i64 * 1_000_000 / RATE as i64;
                    mx.seek(t - lat);
                    // Integrated loudness counts from where playback starts.
                    mx.reset_loudness();
                }
                playing = true;
            }
            Ok(Cmd::Pause) => {
                playing = false;
                if let Some(o) = &out {
                    o.clear();
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        if !playing {
            continue;
        }
        let (Some(o), Some(mx)) = (&out, &mut mixer) else { continue };
        let mut rendered = false;
        while o.queued_frames() < AHEAD {
            mx.render_interleaved(&mut buf);
            o.push(&buf);
            rendered = true;
        }
        if rendered {
            *live().lock().unwrap_or_else(|e| e.into_inner()) = Some((mx.meters().clone(), std::time::Instant::now()));
        }
    }
}

/// Test helper: keeps `out` fed with silence the way [`run`] does, for
/// `d`; returns the underruns.
#[cfg(test)]
fn feed_for(out: &studio_audio::AudioOut, d: Duration) -> u64 {
    const AHEAD: usize = RATE as usize / 10;
    let buf = vec![0f32; 2048];
    let t0 = std::time::Instant::now();
    out.push(&buf);
    while t0.elapsed() < d {
        while out.queued_frames() < AHEAD {
            out.push(&buf);
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    out.underruns()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_keys_and_sound_effects_reach_the_mixer() {
        let mut c = m::Clip::new(m::ClipId(1), m::LaneId(1), 0, 4_000_000, m::ClipSource::Gap);
        c.keyframes.insert("audio.volume_db".into(), m::Track::new(vec![m::Keyframe::new(0, 0.0, m::Interp::Linear), m::Keyframe::new(1_000_000, -20.0, m::Interp::Linear)]));
        let k = keyed(&c, "audio.volume_db", 0.0, |v| v as f32);
        assert!((k.eval(500_000) + 10.0).abs() < 0.2, "{}", k.eval(500_000));
        assert!((k.eval(3_000_000) + 20.0).abs() < 1e-3, "holds after the last key");
        assert_eq!(keyed(&m::Clip::new(m::ClipId(2), m::LaneId(1), 0, 1, m::ClipSource::Gap), "audio.volume_db", -6.0, |v| v as f32).eval(0), -6.0);
        let mut e = m::Effect::new(m::EffectId(3), "compressor");
        e.params.insert("ratio".into(), m::Value::Scalar(4.0));
        assert!(matches!(audio_effect(&e), Some(EffectSpec::Compressor(p)) if p.ratio == 4.0));
        let mut eq = m::Effect::new(m::EffectId(4), "eq");
        eq.params.insert("bass".into(), m::Value::Scalar(-6.0));
        eq.params.insert("high_pass".into(), m::Value::Scalar(80.0));
        assert!(matches!(audio_effect(&eq), Some(EffectSpec::Eq { bands, high_pass: Some(h), .. }) if bands[0].gain_db == -6.0 && h == 80.0));
        assert!(audio_effect(&m::Effect::new(m::EffectId(5), "nonsense")).is_none());
    }

    /// Straight, held and eased keys all reach the mixer as the keys say,
    /// and a long straight ride stays a handful of points.
    #[test]
    fn volume_rides_keep_their_shape_in_few_points() {
        let mut c = m::Clip::new(m::ClipId(1), m::LaneId(1), 0, 10_000_000, m::ClipSource::Gap);
        let keys = vec![m::Keyframe::new(0, 0.0, m::Interp::Linear), m::Keyframe::new(2_000_000, -12.0, m::Interp::Hold), m::Keyframe::new(4_000_000, -3.0, m::Interp::Ease), m::Keyframe::new(6_000_000, -20.0, m::Interp::Linear), m::Keyframe::new(9_000_000, 0.0, m::Interp::Linear)];
        c.keyframes.insert("audio.volume_db".into(), m::Track::new(keys));
        let k = keyed(&c, "audio.volume_db", 0.0, |v| v as f32);
        let track = &c.keyframes["audio.volume_db"];
        for t in (0..10_000_000).step_by(37_000) {
            let want = track.eval(t).and_then(|v| v.as_f64()).unwrap() as f32;
            assert!((k.eval(t) - want).abs() < 0.2, "at {t}: {} vs {want}", k.eval(t));
        }
        // An hour of straight ducking: the keys themselves, not 360 000 samples.
        let ride: Vec<m::Keyframe> = (0..2000).map(|i| m::Keyframe::new(i * 1_800_000, if i % 2 == 0 { 0.0 } else { -10.0 }, m::Interp::Linear)).collect();
        c.keyframes.insert("audio.volume_db".into(), m::Track::new(ride));
        assert_eq!(keyed(&c, "audio.volume_db", 0.0, |v| v as f32).keys().len(), 2000);
    }

    fn commands(rx: &Receiver<Cmd>) -> Vec<&'static str> {
        rx.try_iter().map(|c| match c {
            Cmd::Plan(_) => "plan",
            Cmd::Play(_) => "play",
            Cmd::Pause => "pause",
        }).collect()
    }

    /// Plays silence through the real output for a few seconds, the way the
    /// sound thread feeds it, with every core busy, and counts the times
    /// the speaker ran dry:
    /// `cargo test -p studio-app -- --ignored feeding_survives_a_busy_machine --nocapture`
    #[test]
    #[ignore]
    fn feeding_survives_a_busy_machine() {
        let Ok(out) = studio_audio::AudioOut::open() else { return };
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let cores = std::thread::available_parallelism().map_or(8, |n| n.get());
        let hogs: Vec<_> = (0..cores * 2)
            .map(|_| {
                let stop = stop.clone();
                std::thread::spawn(move || {
                    let mut x = 1u64;
                    while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                        x = x.wrapping_mul(6364136223846793005).wrapping_add(1);
                    }
                    x
                })
            })
            .collect();
        let feeder = std::thread::spawn(move || feed_for(&out, Duration::from_secs(4)));
        let underruns = feeder.join().unwrap();
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        for h in hogs {
            let _ = h.join();
        }
        eprintln!("underruns with {} busy threads: {underruns}", cores * 2);
        assert_eq!(underruns, 0);
    }

    #[test]
    fn edits_while_playing_update_the_mix_without_cutting_the_sound() {
        let (tx, rx) = channel();
        let mut p = LivePlayer::with_sender(tx);
        let mut project = m::Project::default();
        let seq = project.active_sequence;
        let lane = project.sequence(seq).unwrap().lanes[0].id;
        let mut sound = m::Clip::new(m::ClipId(1), lane, 0, 4_000_000, m::ClipSource::Media(m::MediaRef::new(m::MediaId(9))));
        // Media the project does not list: only the plan's inputs matter here.
        project.sequences[0].clips.push(sound.clone());
        p.sync(&project, 1, true, 0, 1.0);
        assert_eq!(commands(&rx), ["plan", "play"]);
        // Ten edits to the sound land while it plays (an agent at work,
        // say); the playhead keeps time with the clock, as in the window.
        let t0 = std::time::Instant::now();
        for edit in 2..12 {
            sound.audio.volume_db = -(edit as f64);
            project.sequences[0].clips[0] = sound.clone();
            p.sync(&project, edit, true, t0.elapsed().as_micros() as i64, 1.0);
        }
        let cmds = commands(&rx);
        assert!(cmds.iter().all(|c| *c == "plan"), "no re-seek, so no gap in the sound: {cmds:?}");
        assert_eq!(cmds.len(), 10);
    }

    /// Moving a picture is an edit too, but the sound is the same: no new
    /// plan is made (a drag edits every frame).
    #[test]
    fn picture_edits_make_no_new_plan() {
        let (tx, rx) = channel();
        let mut p = LivePlayer::with_sender(tx);
        let mut project = m::Project::default();
        let seq = project.active_sequence;
        let lane = project.sequence(seq).unwrap().lanes[0].id;
        let mut card = m::Clip::new(m::ClipId(1), lane, 0, 4_000_000, m::ClipSource::Generator(m::Generator::new(m::GeneratorKind::Solid)));
        project.apply(m::Edit::InsertClip { seq, clip: Box::new(card.clone()) }).unwrap();
        p.sync(&project, 1, false, 0, 1.0);
        assert_eq!(commands(&rx), ["plan"]);
        for edit in 2..6 {
            card.transform.position.x = edit as f64 * 10.0;
            project.apply(m::Edit::SetClip { seq, clip: Box::new(card.clone()) }).unwrap();
            p.sync(&project, edit, false, 0, 1.0);
        }
        assert!(commands(&rx).is_empty());
        // A sound setting changes the plan.
        project.settings.extra.insert("sound".into(), serde_json::json!({"layout": "5.1"}));
        p.sync(&project, 6, false, 0, 1.0);
        assert_eq!(commands(&rx), ["plan"]);
    }

    #[test]
    fn interleaved_mono_and_stereo_split() {
        let d = from_interleaved(48_000, 2, &[0.1, 0.2, 0.3, 0.4]).unwrap();
        assert_eq!((&d.left[..], &d.right[..], d.mono), (&[0.1, 0.3][..], &[0.2, 0.4][..], false));
        let m = from_interleaved(22_050, 1, &[0.5, -0.5]).unwrap();
        assert_eq!((&m.left[..], &m.right[..], m.mono), (&[0.5, -0.5][..], &[0.5, -0.5][..], true));
        assert!(from_interleaved(0, 2, &[0.0, 0.0]).is_none());
    }

    #[test]
    fn source_reads_are_bounded() {
        let d = Arc::new(from_interleaved(48_000, 1, &[1.0, 2.0, 3.0]).unwrap());
        let mut s = SharedSource(d);
        let (mut l, mut r) = ([0f32; 4], [0f32; 4]);
        assert_eq!(s.read(1, &mut l, &mut r), 2);
        assert_eq!(&l[..2], &[2.0, 3.0]);
        assert_eq!(s.read(3, &mut l, &mut r), 0);
        assert_eq!(s.read(u64::MAX, &mut l, &mut r), 0);
    }

    #[test]
    fn empty_project_plans_nothing() {
        let p = plan(&m::Project::default());
        assert!(p.clips.is_empty());
    }
}
