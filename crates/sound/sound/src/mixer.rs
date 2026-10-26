// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The clip → lane → master graph.
//!
//! A [`MixPlan`] describes everything audible: each clip's source, placement,
//! speed, volume and pan curves, fades and effects; each lane's role, gain,
//! mute/solo and effects; the master bus with its limiter and ducking rule.
//! [`Mixer::new`] turns the plan into a graph of processors sized for a
//! maximum block length, and [`Mixer::render`] then produces the timeline's
//! audio block by block, allocation-free, from wherever [`Mixer::seek`] put
//! the playhead. `render` and `app` use the same graph: `render` runs it
//! faster than real time into the encoder, `app` runs it on the audio thread
//! into the output ring buffer.
//!
//! Per clip the signal path is: source (its own rate) → speed and rate
//! conversion (windowed-sinc resampler, plus WSOLA when the pitch is kept)
//! → volume keyframes, fades, mute (sample-accurate) → clip effects → pan →
//! lane sum. Per lane: effects → lane gain → ducking → master sum. Master:
//! effects → true-peak limiter → loudness meter → output.
//!
//! Effects that add latency (the pitch shifter, the limiter) are compensated
//! per clip: on (re)start a clip's chain is primed with the frames before
//! the playhead and that many output frames are discarded, so the audible
//! clip sits exactly where the timeline says.

use crate::automation::{Curve, FadeShape, Fades};
use crate::buffer::{Effect, StereoBuf, db_to_gain, pan_gains};
use crate::delay::{DelayParams, StereoDelay};
use crate::ducking::{Ducker, DuckingParams};
use crate::dynamics::{Compressor, CompressorParams, DeEsser, DeEsserParams, Gate, GateParams, Limiter, LimiterParams};
use crate::eq::{Band, EQ_BANDS, ParametricEq};
use crate::loudness::LoudnessMeter;
use crate::pitch::{PitchShifter, Stretcher};
use crate::resample::{Quality, Resampler};
use crate::reverb::{Reverb, ReverbParams};
use crate::vad::{Vad, VadParams};
use crate::voice::{VoiceChain, VoiceStyle};
use crate::multi::{LfeFilter, MultiBuf, MultiFx, MultiLimiter};
use crate::surround::{Layout, MAX_CHANNELS, PanShape, Position, source_gains};
use crate::{Micros, frames_to_micros, micros_to_frames};

// ---------------------------------------------------------------------------
// Sources
// ---------------------------------------------------------------------------

/// Decoded audio the mixer can read at random positions, at the source's own
/// sample rate. `render` implements this over its decoders (with whatever
/// caching they need); [`MemorySource`] holds a whole clip in memory.
pub trait AudioSource: Send {
    /// Sample rate of the source in Hz.
    fn rate(&self) -> u32;
    /// 1 for mono content (both output channels receive it), 2 for stereo.
    /// Decides the pan law: mono sources are panned with constant power,
    /// stereo sources are balanced.
    fn channels(&self) -> usize;
    /// Length in frames, if known.
    fn frames(&self) -> Option<u64>;
    /// Writes frames `frame..frame + left.len()` into `left`/`right` and
    /// returns how many were available (fewer at the end; 0 past it). Mono
    /// sources write the same samples to both. The mixer zero-fills the rest.
    fn read(&mut self, frame: u64, left: &mut [f32], right: &mut [f32]) -> usize;
}

/// An in-memory planar stereo (or mono) source.
#[derive(Clone, Debug)]
pub struct MemorySource {
    left: Vec<f32>,
    right: Vec<f32>,
    rate: u32,
    channels: usize,
}

impl MemorySource {
    /// A stereo source from planar channels (lengths are truncated to match).
    pub fn stereo(left: Vec<f32>, right: Vec<f32>, rate: u32) -> Self {
        let n = left.len().min(right.len());
        let mut left = left;
        let mut right = right;
        left.truncate(n);
        right.truncate(n);
        Self { left, right, rate, channels: 2 }
    }

    /// A mono source.
    pub fn mono(samples: Vec<f32>, rate: u32) -> Self {
        Self { left: samples, right: Vec::new(), rate, channels: 1 }
    }

    /// A source from interleaved stereo samples.
    pub fn interleaved(stereo: &[f32], rate: u32) -> Self {
        let n = stereo.len() / 2;
        let left = (0..n).map(|i| stereo[2 * i]).collect();
        let right = (0..n).map(|i| stereo[2 * i + 1]).collect();
        Self { left, right, rate, channels: 2 }
    }

    /// Length in frames.
    pub fn len(&self) -> usize {
        self.left.len()
    }

    /// True for an empty source.
    pub fn is_empty(&self) -> bool {
        self.left.is_empty()
    }
}

impl AudioSource for MemorySource {
    fn rate(&self) -> u32 {
        self.rate
    }

    fn channels(&self) -> usize {
        self.channels
    }

    fn frames(&self) -> Option<u64> {
        Some(self.left.len() as u64)
    }

    fn read(&mut self, frame: u64, left: &mut [f32], right: &mut [f32]) -> usize {
        let start = (frame as usize).min(self.left.len());
        let n = left.len().min(right.len()).min(self.left.len() - start);
        left[..n].copy_from_slice(&self.left[start..start + n]);
        if self.channels == 1 {
            right[..n].copy_from_slice(&self.left[start..start + n]);
        } else {
            right[..n].copy_from_slice(&self.right[start..start + n]);
        }
        n
    }
}

// ---------------------------------------------------------------------------
// Plan
// ---------------------------------------------------------------------------

/// An effect as data, so plans can be compared and stored. [`EffectSpec::build`]
/// makes the processor.
#[derive(Clone, Debug, PartialEq)]
pub enum EffectSpec {
    /// Five-band parametric EQ with optional high- and low-pass.
    Eq {
        /// The bands, in order.
        bands: [Band; EQ_BANDS],
        /// High-pass corner in Hz, if any.
        high_pass: Option<f32>,
        /// Low-pass corner in Hz, if any.
        low_pass: Option<f32>,
    },
    /// Compressor.
    Compressor(CompressorParams),
    /// True-peak limiter.
    Limiter(LimiterParams),
    /// Noise gate.
    Gate(GateParams),
    /// De-esser.
    DeEsser(DeEsserParams),
    /// Algorithmic reverb.
    Reverb(ReverbParams),
    /// Stereo delay.
    Delay(DelayParams),
    /// Pitch shift in semitones (±24), length unchanged.
    PitchShift {
        /// Semitones up (positive) or down.
        semitones: f32,
    },
    /// A voice style with a blend amount (0 dry .. 1 full).
    Voice {
        /// The style.
        style: VoiceStyle,
        /// Blend between the untouched voice and the style.
        amount: f32,
    },
}

impl EffectSpec {
    /// A flat EQ (all bands at 0 dB), handy as a starting point.
    pub fn flat_eq() -> Self {
        EffectSpec::Eq { bands: [Band::flat(80.0), Band::flat(250.0), Band::flat(1000.0), Band::flat(3500.0), Band::flat(10_000.0)], high_pass: None, low_pass: None }
    }

    /// Stable identifier for project files.
    pub fn kind_id(&self) -> &'static str {
        match self {
            EffectSpec::Eq { .. } => "eq",
            EffectSpec::Compressor(_) => "compressor",
            EffectSpec::Limiter(_) => "limiter",
            EffectSpec::Gate(_) => "gate",
            EffectSpec::DeEsser(_) => "de_esser",
            EffectSpec::Reverb(_) => "reverb",
            EffectSpec::Delay(_) => "delay",
            EffectSpec::PitchShift { .. } => "pitch_shift",
            EffectSpec::Voice { .. } => "voice",
        }
    }

    /// Builds the processor for `rate` and blocks up to `max_block`. Allocates.
    pub fn build(&self, rate: u32, max_block: usize) -> Box<dyn Effect> {
        match self {
            EffectSpec::Eq { bands, high_pass, low_pass } => {
                let mut eq = ParametricEq::new(rate);
                for (i, b) in bands.iter().enumerate() {
                    eq.set_band(i, *b);
                }
                eq.set_high_pass(*high_pass);
                eq.set_low_pass(*low_pass);
                Box::new(eq)
            }
            EffectSpec::Compressor(p) => Box::new(Compressor::new(rate, *p)),
            EffectSpec::Limiter(p) => Box::new(Limiter::new(rate, *p)),
            EffectSpec::Gate(p) => Box::new(Gate::new(rate, *p)),
            EffectSpec::DeEsser(p) => Box::new(DeEsser::new(rate, *p)),
            EffectSpec::Reverb(p) => Box::new(Reverb::new(rate, *p)),
            EffectSpec::Delay(p) => Box::new(StereoDelay::new(rate, *p)),
            EffectSpec::PitchShift { semitones } => Box::new(PitchShifter::new(rate, *semitones, max_block)),
            EffectSpec::Voice { style, amount } => {
                let mut chain: VoiceChain = style.build(rate, max_block);
                chain.amount = amount.clamp(0.0, 1.0);
                chain.prepare(max_block);
                Box::new(chain)
            }
        }
    }
}

/// Playback speed of a clip.
#[derive(Clone, Debug, PartialEq)]
pub enum SpeedSpec {
    /// One speed for the whole clip (1 = normal; 0 freezes, which is silence).
    Constant(f64),
    /// Speed as a curve over clip time (keys in clip-relative microseconds).
    Varying(Curve),
}

impl Default for SpeedSpec {
    fn default() -> Self {
        SpeedSpec::Constant(1.0)
    }
}

impl SpeedSpec {
    /// Speed at clip time `t`.
    pub fn at(&self, t: Micros) -> f64 {
        match self {
            SpeedSpec::Constant(s) => *s,
            SpeedSpec::Varying(c) => c.eval(t) as f64,
        }
    }

    /// The largest speed the clip reaches (sizes buffers).
    pub fn max(&self) -> f64 {
        match self {
            SpeedSpec::Constant(s) => s.abs(),
            SpeedSpec::Varying(c) => c.keys().iter().fold(c.eval(0).abs() as f64, |m, k| m.max(k.value.abs() as f64)),
        }
    }

    /// Source time elapsed after `t` of clip time: the integral of the speed.
    /// Segments are integrated with Simpson's rule, exact for the polynomial
    /// eases the curves use.
    pub fn integrate(&self, t: Micros) -> Micros {
        match self {
            SpeedSpec::Constant(s) => (t as f64 * s).round() as Micros,
            SpeedSpec::Varying(c) => {
                let keys = c.keys();
                if keys.len() <= 1 {
                    return (t as f64 * c.eval(0) as f64).round() as Micros;
                }
                let mut acc = 0.0f64;
                let mut from = 0i64;
                // Before the first key the curve is flat.
                let first = keys[0].time.max(0);
                if t <= first {
                    return (t as f64 * keys[0].value as f64).round() as Micros;
                }
                acc += first as f64 * keys[0].value as f64;
                from = from.max(first);
                for w in keys.windows(2) {
                    let (a, b) = (w[0].time.max(from), w[1].time.min(t));
                    if b <= a {
                        if w[1].time >= t {
                            break;
                        }
                        continue;
                    }
                    acc += simpson(|x| c.eval(x) as f64, a, b, 16);
                    from = b;
                    if w[1].time >= t {
                        break;
                    }
                }
                if t > from {
                    acc += (t - from) as f64 * c.eval(t) as f64;
                }
                acc.round() as Micros
            }
        }
    }
}

fn simpson(f: impl Fn(Micros) -> f64, a: Micros, b: Micros, n: usize) -> f64 {
    let n = n.max(2) & !1;
    let h = (b - a) as f64 / n as f64;
    let mut s = f(a) + f(b);
    for i in 1..n {
        let x = a as f64 + i as f64 * h;
        let w = if i % 2 == 1 { 4.0 } else { 2.0 };
        s += w * f(x.round() as Micros);
    }
    s * h / 3.0
}

/// One clip in the plan. Times are timeline microseconds except where noted.
pub struct ClipSpec {
    /// Identity, stable across plan updates (the model's clip id).
    pub id: u64,
    /// The lane it sits on (a [`LaneSpec::id`]).
    pub lane: u64,
    /// Where the clip starts on the timeline.
    pub start: Micros,
    /// How long it plays on the timeline.
    pub duration: Micros,
    /// Source position of the clip's first output frame. For reversed clips
    /// playback runs backwards from here.
    pub source_in: Micros,
    /// Playback speed.
    pub speed: SpeedSpec,
    /// Keep the pitch when the speed is not 1 (WSOLA); otherwise varispeed.
    pub preserve_pitch: bool,
    /// Play the source backwards.
    pub reversed: bool,
    /// Volume in dB over clip time (keys in clip-relative microseconds).
    pub volume_db: Curve,
    /// Pan −1..1 over clip time.
    pub pan: Curve,
    /// Silence the clip (its effects keep running so tails are unaffected).
    pub muted: bool,
    /// Fade-in length.
    pub fade_in: Micros,
    /// Fade-out length.
    pub fade_out: Micros,
    /// Shape of both fades.
    pub fade_shape: FadeShape,
    /// Extra gain in dB applied with the volume, e.g. from loudness
    /// normalisation ([`crate::loudness::normalise_gain_db`]).
    pub gain_offset_db: f32,
    /// Effects, applied in order after the volume and before the pan.
    pub effects: Vec<EffectSpec>,
    /// The audio.
    pub source: Box<dyn AudioSource>,
}

impl ClipSpec {
    /// A clip with unity volume, centre pan, no fades or effects.
    pub fn new(id: u64, lane: u64, start: Micros, duration: Micros, source: Box<dyn AudioSource>) -> Self {
        Self {
            id,
            lane,
            start,
            duration,
            source_in: 0,
            speed: SpeedSpec::default(),
            preserve_pitch: true,
            reversed: false,
            volume_db: Curve::constant(0.0),
            pan: Curve::constant(0.0),
            muted: false,
            fade_in: 0,
            fade_out: 0,
            fade_shape: FadeShape::Linear,
            gain_offset_db: 0.0,
            effects: Vec::new(),
            source,
        }
    }

    /// Timeline end.
    pub fn end(&self) -> Micros {
        self.start + self.duration
    }

}

/// What a lane carries, for ducking and for routing to a bus of that kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum LaneRole {
    /// Speech: drives the ducking detector; goes to the dialogue bus.
    Dialogue,
    /// Music: is ducked under dialogue; goes to the music bus.
    Music,
    /// Room tone, backgrounds, atmospheres: the ambience bus.
    Ambience,
    /// Sound effects, foley and anything else: the effects bus.
    #[default]
    Other,
}

impl LaneRole {
    /// The kind of bus lanes of this role go to by default.
    pub fn bus_kind(self) -> BusKind {
        match self {
            LaneRole::Dialogue => BusKind::Dialogue,
            LaneRole::Music => BusKind::Music,
            LaneRole::Ambience => BusKind::Ambience,
            LaneRole::Other => BusKind::Effects,
        }
    }
}

/// What a bus carries. Stems are cut along these kinds: DX (dialogue), MX
/// (music), FX (effects), and M&E is music + effects + ambience.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum BusKind {
    /// Dialogue (DX).
    Dialogue,
    /// Music (MX).
    Music,
    /// Sound effects and foley (FX).
    #[default]
    Effects,
    /// Backgrounds and room tone (part of FX in a three-stem delivery).
    Ambience,
    /// An effect return fed by sends (a shared reverb, say). Solo-safe: it
    /// keeps playing when other buses are soloed.
    Aux,
}

impl BusKind {
    /// Every kind, in strip order.
    pub const ALL: [BusKind; 5] = [BusKind::Dialogue, BusKind::Music, BusKind::Effects, BusKind::Ambience, BusKind::Aux];

    /// Stable id: `dialogue`, `music`, `effects`, `ambience`, `aux`.
    pub fn id(self) -> &'static str {
        match self {
            BusKind::Dialogue => "dialogue",
            BusKind::Music => "music",
            BusKind::Effects => "effects",
            BusKind::Ambience => "ambience",
            BusKind::Aux => "aux",
        }
    }

    /// Parses an id or a stem abbreviation (`dx`, `mx`, `fx`, `bg`, `amb`).
    pub fn from_id(s: &str) -> Option<BusKind> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "dialogue" | "dialog" | "dx" | "voice" | "vo" | "speech" => BusKind::Dialogue,
            "music" | "mx" | "score" => BusKind::Music,
            "effects" | "fx" | "sfx" | "foley" => BusKind::Effects,
            "ambience" | "amb" | "bg" | "backgrounds" | "atmos" | "room_tone" => BusKind::Ambience,
            "aux" | "return" | "reverb" | "send" => BusKind::Aux,
            _ => return None,
        })
    }

    /// The stem abbreviation: DX, MX, FX, BG, AUX.
    pub fn stem(self) -> &'static str {
        match self {
            BusKind::Dialogue => "DX",
            BusKind::Music => "MX",
            BusKind::Effects => "FX",
            BusKind::Ambience => "BG",
            BusKind::Aux => "AUX",
        }
    }
}

/// A send from a lane to an aux bus.
#[derive(Clone, Debug, PartialEq)]
pub struct SendSpec {
    /// The bus it feeds (a [`BusSpec::id`]).
    pub bus: u64,
    /// Send level in dB over timeline time.
    pub level_db: Curve,
    /// Take the signal before the lane fader (level independent of the
    /// fader) instead of after it.
    pub pre_fader: bool,
}

/// A lane's place in a surround mix.
#[derive(Clone, Debug, PartialEq)]
pub struct SurroundSpec {
    /// −1 left .. 1 right, over timeline time.
    pub x: Curve,
    /// −1 back .. 1 front.
    pub y: Curve,
    /// 0 ear level .. 1 overhead (object export only).
    pub z: Curve,
    /// Width, centre use and LFE send.
    pub shape: PanShape,
}

impl Default for SurroundSpec {
    fn default() -> Self {
        Self { x: Curve::constant(0.0), y: Curve::constant(1.0), z: Curve::constant(0.0), shape: PanShape::default() }
    }
}

impl SurroundSpec {
    /// The position at `t`.
    pub fn at(&self, t: Micros) -> Position {
        Position { x: self.x.eval(t).clamp(-1.0, 1.0), y: self.y.eval(t).clamp(-1.0, 1.0), z: self.z.eval(t).clamp(0.0, 1.0) }
    }
}

/// One lane (track) in the plan.
#[derive(Clone, Debug, PartialEq)]
pub struct LaneSpec {
    /// Identity (the model's lane id).
    pub id: u64,
    /// Role for ducking and default bus routing.
    pub role: LaneRole,
    /// Lane gain in dB over timeline time (the fader and its automation).
    pub volume_db: Curve,
    /// Balance −1..1 (stereo mixes).
    pub pan: f32,
    /// Balance automation over timeline time; replaces `pan` when set.
    pub pan_curve: Option<Curve>,
    /// Silence the lane.
    pub muted: bool,
    /// When any lane is soloed, only soloed lanes sound.
    pub solo: bool,
    /// Lane effects, before the lane gain.
    pub effects: Vec<EffectSpec>,
    /// The bus it plays into; `None` = the bus of its role's kind when the
    /// plan has one, else the master.
    pub bus: Option<u64>,
    /// Sends to aux buses.
    pub sends: Vec<SendSpec>,
    /// Where it sits in a surround mix.
    pub surround: SurroundSpec,
}

impl LaneSpec {
    /// An unmuted lane at 0 dB.
    pub fn new(id: u64, role: LaneRole) -> Self {
        Self { id, role, volume_db: Curve::constant(0.0), pan: 0.0, pan_curve: None, muted: false, solo: false, effects: Vec::new(), bus: None, sends: Vec::new(), surround: SurroundSpec::default() }
    }
}

/// A bus: a group of lanes (or sends) with its own processing and fader.
#[derive(Clone, Debug, PartialEq)]
pub struct BusSpec {
    /// Identity.
    pub id: u64,
    /// What it carries.
    pub kind: BusKind,
    /// Display name.
    pub name: String,
    /// Bus gain in dB over timeline time.
    pub volume_db: Curve,
    /// Bus effects (EQ, compression, de-essing, reverb…), before the gain.
    pub effects: Vec<EffectSpec>,
    /// Silence the bus.
    pub muted: bool,
    /// When any bus is soloed, only soloed buses (and aux returns) sound.
    pub solo: bool,
}

impl BusSpec {
    /// A unity-gain bus with no effects.
    pub fn new(id: u64, kind: BusKind, name: impl Into<String>) -> Self {
        Self { id, kind, name: name.into(), volume_db: Curve::constant(0.0), effects: Vec::new(), muted: false, solo: false }
    }
}

/// The master bus.
#[derive(Clone, Debug, PartialEq)]
pub struct MasterSpec {
    /// Master gain in dB.
    pub volume_db: f32,
    /// Master effects, before the limiter.
    pub effects: Vec<EffectSpec>,
    /// The output limiter; `None` leaves peaks alone (export of stems).
    pub limiter: Option<LimiterParams>,
    /// Duck music lanes under dialogue lanes.
    pub ducking: Option<DuckingParams>,
    /// Detector settings for the ducking.
    pub vad: VadParams,
}

impl Default for MasterSpec {
    fn default() -> Self {
        Self { volume_db: 0.0, effects: Vec::new(), limiter: Some(LimiterParams::default()), ducking: None, vad: VadParams::default() }
    }
}

/// Everything the mixer plays.
#[derive(Default)]
pub struct MixPlan {
    /// The clips, any order.
    pub clips: Vec<ClipSpec>,
    /// The lanes, in display order (meters follow this order).
    pub lanes: Vec<LaneSpec>,
    /// The buses between lanes and the master, in strip order. None: every
    /// lane plays straight into the master.
    pub buses: Vec<BusSpec>,
    /// The master bus.
    pub master: MasterSpec,
    /// The speaker layout rendered.
    pub layout: Layout,
}

impl MixPlan {
    /// Timeline time after which nothing sounds (the latest clip end), or 0.
    pub fn end(&self) -> Micros {
        self.clips.iter().map(|c| c.end()).max().unwrap_or(0)
    }

    /// The kind of bus a lane ends up in (its own bus's kind, or its role's
    /// kind); this is the stem it belongs to.
    pub fn lane_kind(&self, lane: &LaneSpec) -> BusKind {
        match lane.bus.and_then(|b| self.buses.iter().find(|x| x.id == b)) {
            Some(b) if b.kind != BusKind::Aux => b.kind,
            _ => lane.role.bus_kind(),
        }
    }

    /// Keeps only the lanes whose stem is one of `kinds` (with their sends,
    /// so a dialogue reverb stays in the dialogue stem), and takes the
    /// limiter off so the stems add up to the mix. A three-stem delivery
    /// asks for `[Effects, Ambience]` as its FX stem.
    pub fn stem(&mut self, kinds: &[BusKind]) {
        let keep: Vec<bool> = self.lanes.iter().map(|l| kinds.contains(&self.lane_kind(l))).collect();
        let any_solo = self.lanes.iter().any(|l| l.solo);
        for (l, k) in self.lanes.iter_mut().zip(keep) {
            if any_solo && !l.solo {
                l.muted = true;
            }
            if !k {
                l.muted = true;
            }
            l.solo = false;
        }
        self.master.limiter = None;
    }
}

// ---------------------------------------------------------------------------
// Clip engine
// ---------------------------------------------------------------------------

/// Varispeed above this goes through the stretcher as well, so the resampler
/// ratio stays inside its range and buffers stay bounded.
const MAX_VARISPEED: f64 = 32.0;
/// The stretcher's own factor range (speed 0.1×..10×).
const MAX_STRETCH_SPEED: f64 = 10.0;
const MIN_STRETCH_SPEED: f64 = 0.1;

struct ClipEngine {
    id: u64,
    lane: usize,
    start_f: i64,
    len_f: i64,
    source_in: Micros,
    src_rate: u32,
    mono: bool,
    speed: SpeedSpec,
    preserve_pitch: bool,
    reversed: bool,
    volume_db: Curve,
    pan: Curve,
    muted: bool,
    fades: Fades,
    gain_offset: f32,
    source: Box<dyn AudioSource>,
    resampler: Resampler,
    stretcher: Option<Stretcher>,
    /// Same rate, speed 1: read the source straight through (bit-exact).
    direct: bool,
    spec_effects: Vec<EffectSpec>,
    effects: Vec<Box<dyn Effect>>,
    /// Delay added by the effects, compensated on seek.
    latency: usize,
    tail: usize,
    rate: u32,
    // Scratch.
    src: [Vec<f32>; 2],
    rs: [Vec<f32>; 2],
    gains: Vec<f32>,
    pans: Vec<f32>,
    // Running state.
    src_next: i64,
    in_frame: i64,
    active: bool,
}

impl ClipEngine {
    fn new(spec: &mut ClipSpec, lane: usize, rate: u32, max_block: usize, quality: Quality, placeholder: Box<dyn AudioSource>) -> Self {
        let source = std::mem::replace(&mut spec.source, placeholder);
        let src_rate = source.rate().max(1);
        let max_speed = spec.speed.max().max(1e-3);
        let speed_changes = match &spec.speed {
            SpeedSpec::Constant(s) => (*s - 1.0).abs() > 1e-9,
            SpeedSpec::Varying(_) => true,
        };
        let needs_stretch = (spec.preserve_pitch && speed_changes) || max_speed > MAX_VARISPEED;
        let direct = !speed_changes && src_rate == rate;
        let varispeed_max = if needs_stretch {
            if spec.preserve_pitch { (max_speed / MAX_STRETCH_SPEED).max(1.0) } else { (max_speed / MAX_STRETCH_SPEED).max(1.0).min(MAX_VARISPEED) }
        } else {
            max_speed.min(MAX_VARISPEED)
        };
        let ratio_max = (src_rate as f64 / rate as f64 * varispeed_max).clamp(1.0 / 64.0, 64.0);
        let rs_cap = max_block * 2 + 4096;
        let src_cap = (rs_cap as f64 * ratio_max).ceil() as usize + 16;
        let resampler = Resampler::with_ratio(src_rate as f64 / rate as f64, quality, src_cap);
        let stretcher = needs_stretch.then(|| Stretcher::new(rate, 1.0, rs_cap));
        let effects: Vec<Box<dyn Effect>> = spec.effects.iter().map(|e| e.build(rate, max_block)).collect();
        // The stretcher adds no nominal delay (output frame j comes from the
        // grain around input j / factor); only its fade-in needs pre-roll.
        let latency = effects.iter().map(|e| e.latency()).sum::<usize>();
        let tail = effects.iter().map(|e| e.tail()).max().unwrap_or(0);
        let mut e = Self {
            id: spec.id,
            lane,
            start_f: 0,
            len_f: 0,
            source_in: 0,
            src_rate,
            mono: source.channels() == 1,
            speed: spec.speed.clone(),
            preserve_pitch: spec.preserve_pitch,
            reversed: spec.reversed,
            volume_db: Curve::constant(0.0),
            pan: Curve::constant(0.0),
            muted: false,
            fades: Fades::default(),
            gain_offset: 1.0,
            source,
            resampler,
            stretcher,
            direct,
            spec_effects: spec.effects.clone(),
            effects,
            latency,
            tail,
            rate,
            src: [vec![0.0; src_cap], vec![0.0; src_cap]],
            rs: [vec![0.0; rs_cap], vec![0.0; rs_cap]],
            gains: vec![0.0; max_block],
            pans: vec![0.0; max_block],
            src_next: 0,
            in_frame: 0,
            active: false,
        };
        e.update_params(spec);
        e
    }

    /// Takes the parameters that do not need a rebuild.
    fn update_params(&mut self, spec: &ClipSpec) {
        self.start_f = micros_to_frames(spec.start, self.rate);
        self.len_f = micros_to_frames(spec.duration, self.rate).max(0);
        self.source_in = spec.source_in;
        self.volume_db = spec.volume_db.clone();
        self.pan = spec.pan.clone();
        self.muted = spec.muted;
        self.fades = Fades {
            fade_in: micros_to_frames(spec.fade_in, self.rate).max(0) as u64,
            fade_out: micros_to_frames(spec.fade_out, self.rate).max(0) as u64,
            shape: spec.fade_shape,
        };
        self.gain_offset = db_to_gain(spec.gain_offset_db);
    }

    /// Timeline frames during which the clip produces output (tails included).
    fn span(&self) -> (i64, i64) {
        (self.start_f, self.start_f + self.len_f + self.tail as i64)
    }

    fn clip_time(&self, frame: i64) -> Micros {
        frames_to_micros(frame, self.rate)
    }

    /// Positions the engine so the next rendered frame is clip frame `c0`.
    ///
    /// Output frame `j` of the chain is the effects' response to gain-stage
    /// frame `j - latency`, so the gain stage is started `pre_roll` frames
    /// before `c0` and `pre_roll + latency` output frames are discarded. The
    /// pre-roll fills the resampler's history with real signal (instead of
    /// the zeros a reset leaves) and covers the stretcher's fade-in, so the
    /// first kept frame is already steady. Allocation-free.
    fn seek(&mut self, c0: i64, scratch: &mut StereoBuf) {
        let pre_roll = if self.direct { 0 } else { (self.resampler.latency() as f64 / self.resampler.ratio().max(1.0 / 64.0)).ceil() as usize }
            + self.stretcher.as_ref().map(|s| 2 * s.latency()).unwrap_or(0)
            + 64;
        self.in_frame = c0 - pre_roll as i64;
        let t_clip = self.clip_time(self.in_frame);
        let elapsed = self.speed.integrate(t_clip.max(0)) + (t_clip.min(0) as f64 * self.speed.at(0)).round() as Micros;
        let src_t = if self.reversed { self.source_in - elapsed } else { self.source_in + elapsed };
        self.src_next = micros_to_frames(src_t, self.src_rate);
        self.resampler.reset();
        if let Some(s) = &mut self.stretcher {
            s.reset();
        }
        for e in &mut self.effects {
            e.reset();
        }
        self.active = true;
        let mut left = pre_roll + self.latency;
        let cap = scratch.capacity();
        while left > 0 {
            let n = left.min(cap);
            scratch.set_frames(n);
            let (l, r) = scratch.channels_mut();
            self.produce(l, r);
            left -= n;
        }
    }

    /// Reads `n` source frames (forward or backward) into `self.src`,
    /// zero-filling beyond the source. Advances the source position.
    fn read_source(&mut self, n: usize) {
        let [src_l, src_r] = &mut self.src;
        let (l, r) = (&mut src_l[..n], &mut src_r[..n]);
        if self.reversed {
            let end = self.src_next;
            let start = end - n as i64;
            let got = if end <= 0 {
                0
            } else {
                let from = start.max(0);
                let want = (end - from) as usize;
                // Right-align so the sample at `end - 1` lands at index n-1.
                let off = n - want;
                let k = self.source.read(from as u64, &mut l[off..off + want], &mut r[off..off + want]);
                l[off + k..].fill(0.0);
                r[off + k..].fill(0.0);
                l[..off].fill(0.0);
                r[..off].fill(0.0);
                want
            };
            if got == 0 {
                l.fill(0.0);
                r.fill(0.0);
            }
            l.reverse();
            r.reverse();
            self.src_next -= n as i64;
        } else {
            let got = if self.src_next < 0 {
                let skip = (-self.src_next as usize).min(n);
                l[..skip].fill(0.0);
                r[..skip].fill(0.0);
                let k = if skip < n { self.source.read(0, &mut l[skip..], &mut r[skip..]) } else { 0 };
                skip + k
            } else {
                self.source.read(self.src_next as u64, l, r)
            };
            l[got..].fill(0.0);
            r[got..].fill(0.0);
            self.src_next += n as i64;
        }
    }

    /// Pulls `out.len()` frames at the mixer rate from the resampler, feeding
    /// it source frames as needed.
    fn pull_resampled(&mut self, out_l: &mut [f32], out_r: &mut [f32]) {
        let n = out_l.len();
        let mut filled = 0;
        // Drain what the filter already holds.
        filled += self.resampler.process(&[], &[], out_l, out_r);
        let mut stalls = 0;
        while filled < n {
            let remaining = n - filled;
            let want = ((remaining as f64 * self.resampler.ratio()).ceil() as usize + 2).min(self.resampler.space()).min(self.src[0].len());
            if want == 0 {
                stalls += 1;
                if stalls > 4 {
                    break;
                }
            } else {
                self.read_source(want);
            }
            let (sl, sr) = (&self.src[0][..want], &self.src[1][..want]);
            let k = self.resampler.process(sl, sr, &mut out_l[filled..], &mut out_r[filled..]);
            filled += k;
        }
        out_l[filled..].fill(0.0);
        out_r[filled..].fill(0.0);
    }

    /// Fills `out` with the speed-converted source for input frames starting
    /// at `self.in_frame`.
    fn pull_raw(&mut self, out_l: &mut [f32], out_r: &mut [f32]) {
        let n = out_l.len();
        let speed = self.speed.at(self.clip_time(self.in_frame).max(0)).abs();
        if speed < 1e-6 {
            out_l.fill(0.0);
            out_r.fill(0.0);
            return;
        }
        if self.direct {
            self.read_source(n);
            out_l.copy_from_slice(&self.src[0][..n]);
            out_r.copy_from_slice(&self.src[1][..n]);
            return;
        }
        let base = self.src_rate as f64 / self.rate as f64;
        match &mut self.stretcher {
            None => {
                self.resampler.set_ratio(base * speed.min(MAX_VARISPEED));
                self.pull_resampled(out_l, out_r);
            }
            Some(_) => {
                // Split the speed between the stretcher (keeps pitch) and the
                // resampler (varispeed) so each stays within its range.
                let (stretch_speed, vari) = if self.preserve_pitch {
                    let s = speed.clamp(MIN_STRETCH_SPEED, MAX_STRETCH_SPEED);
                    (s, speed / s)
                } else {
                    let v = speed.min(MAX_VARISPEED);
                    (speed / v, v)
                };
                self.resampler.set_ratio(base * vari);
                let mut produced = 0;
                let mut stalls = 0;
                while produced < n {
                    let st = self.stretcher.as_mut().expect("stretcher present");
                    st.set_factor(1.0 / stretch_speed);
                    let got = st.pull(&mut out_l[produced..], &mut out_r[produced..]);
                    produced += got;
                    if produced >= n {
                        break;
                    }
                    let need = st.needed_input().max(1).min(st.space()).min(self.rs[0].len());
                    if need == 0 {
                        stalls += 1;
                        if stalls > 4 {
                            break;
                        }
                        continue;
                    }
                    // Move the scratch out while `pull_resampled` borrows
                    // `self` (a swap, not an allocation).
                    let mut tmp_l = std::mem::take(&mut self.rs[0]);
                    let mut tmp_r = std::mem::take(&mut self.rs[1]);
                    self.pull_resampled(&mut tmp_l[..need], &mut tmp_r[..need]);
                    let st = self.stretcher.as_mut().expect("stretcher present");
                    st.push(&tmp_l[..need], &tmp_r[..need]);
                    self.rs[0] = tmp_l;
                    self.rs[1] = tmp_r;
                }
                out_l[produced..].fill(0.0);
                out_r[produced..].fill(0.0);
            }
        }
    }

    /// Produces one block of clip output into `out` (frames of the mixer).
    fn produce(&mut self, out_l: &mut [f32], out_r: &mut [f32]) {
        let n = out_l.len();
        self.pull_raw(out_l, out_r);
        // Volume keyframes, fades, mute and the clip's extent, at input time.
        let in_t = self.clip_time(self.in_frame);
        let gains = &mut self.gains[..n];
        if self.muted {
            gains.fill(0.0);
        } else {
            self.volume_db.fill(in_t, self.rate, gains);
            for g in gains.iter_mut() {
                *g = db_to_gain(*g) * self.gain_offset;
            }
            let first = self.in_frame;
            // Frames outside 0..len are silent; fades apply inside.
            let lo = (-first).clamp(0, n as i64) as usize;
            let hi = (self.len_f - first).clamp(0, n as i64) as usize;
            gains[..lo].fill(0.0);
            gains[hi..].fill(0.0);
            if hi > lo {
                self.fades.apply((first + lo as i64) as u64, self.len_f as u64, &mut gains[lo..hi]);
            }
        }
        for i in 0..n {
            out_l[i] *= gains[i];
            out_r[i] *= gains[i];
        }
        for e in &mut self.effects {
            e.process(out_l, out_r);
        }
        // Pan at output time (the effects' latency is already accounted for).
        let out_frame = self.in_frame - self.latency as i64;
        if self.pan.is_constant() {
            let p = self.pan.eval(0);
            if p.abs() > 1e-6 || self.mono {
                let (gl, gr) = self.pan_pair(p);
                for i in 0..n {
                    out_l[i] *= gl;
                    out_r[i] *= gr;
                }
            }
        } else {
            let t = self.clip_time(out_frame);
            let mut pans = std::mem::take(&mut self.pans);
            self.pan.fill(t, self.rate, &mut pans[..n]);
            for i in 0..n {
                let (gl, gr) = self.pan_pair(pans[i]);
                out_l[i] *= gl;
                out_r[i] *= gr;
            }
            self.pans = pans;
        }
        self.in_frame += n as i64;
    }

    /// Mono sources use the constant-power law (centre = unity, so a mono
    /// clip at 0 dB plays at its own level); stereo sources use a balance law.
    #[inline]
    fn pan_pair(&self, p: f32) -> (f32, f32) {
        if self.mono {
            let (l, r) = pan_gains(p);
            (l * std::f32::consts::SQRT_2, r * std::f32::consts::SQRT_2)
        } else {
            let p = p.clamp(-1.0, 1.0);
            (1.0f32.min(1.0 - p), 1.0f32.min(1.0 + p))
        }
    }

    /// Renders the clip's contribution for timeline frames `f0..f0 + n` into
    /// `out`, seeking first when the playhead is not where the engine left off.
    fn render(&mut self, f0: i64, out_l: &mut [f32], out_r: &mut [f32], scratch: &mut StereoBuf) {
        let c0 = f0 - self.start_f;
        if !self.active || self.in_frame - self.latency as i64 != c0 {
            self.seek(c0, scratch);
        }
        self.produce(out_l, out_r);
    }
}

// ---------------------------------------------------------------------------
// Lanes, buses, master, mixer
// ---------------------------------------------------------------------------

/// Where a lane's output goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Dest {
    Bus(usize),
    Master,
}

struct LaneEngine {
    id: u64,
    role: LaneRole,
    volume_db: Curve,
    pan: f32,
    pan_curve: Option<Curve>,
    muted: bool,
    solo: bool,
    spec_effects: Vec<EffectSpec>,
    effects: Vec<Box<dyn Effect>>,
    bus_id: Option<u64>,
    dest: Dest,
    sends: Vec<SendSpec>,
    /// Bus index per send (`None`: the bus is gone).
    send_dest: Vec<Option<usize>>,
    surround: SurroundSpec,
    buf: StereoBuf,
    /// Pre-fader copy for pre-fader sends.
    pre: StereoBuf,
    gains: Vec<f32>,
    pans: Vec<f32>,
}

impl LaneEngine {
    fn new(spec: &LaneSpec, rate: u32, max_block: usize) -> Self {
        Self {
            id: spec.id,
            role: spec.role,
            volume_db: spec.volume_db.clone(),
            pan: spec.pan,
            pan_curve: spec.pan_curve.clone(),
            muted: spec.muted,
            solo: spec.solo,
            spec_effects: spec.effects.clone(),
            effects: spec.effects.iter().map(|e| e.build(rate, max_block)).collect(),
            bus_id: spec.bus,
            dest: Dest::Master,
            sends: spec.sends.clone(),
            send_dest: Vec::new(),
            surround: spec.surround.clone(),
            buf: StereoBuf::new(max_block),
            pre: StereoBuf::new(max_block),
            gains: vec![0.0; max_block],
            pans: vec![0.0; max_block],
        }
    }

    fn update(&mut self, spec: &LaneSpec, rate: u32, max_block: usize) {
        self.role = spec.role;
        self.volume_db = spec.volume_db.clone();
        self.pan = spec.pan;
        self.pan_curve = spec.pan_curve.clone();
        self.muted = spec.muted;
        self.solo = spec.solo;
        self.bus_id = spec.bus;
        self.sends = spec.sends.clone();
        self.surround = spec.surround.clone();
        if self.spec_effects != spec.effects {
            self.spec_effects = spec.effects.clone();
            self.effects = spec.effects.iter().map(|e| e.build(rate, max_block)).collect();
        }
    }
}

/// A delay line over N channels, lining a bus up with the slowest bus.
struct DelayComp {
    lines: Vec<Vec<f32>>,
    pos: usize,
}

impl DelayComp {
    fn new(channels: usize, frames: usize) -> Self {
        Self { lines: vec![vec![0.0; frames]; if frames == 0 { 0 } else { channels }], pos: 0 }
    }

    fn process(&mut self, buf: &mut MultiBuf) {
        let Some(len) = self.lines.first().map(Vec::len) else { return };
        if len == 0 {
            return;
        }
        let start = self.pos;
        for (c, line) in self.lines.iter_mut().enumerate() {
            let x = buf.chan_mut(c);
            let mut p = start;
            for v in x.iter_mut() {
                let out = line[p];
                line[p] = *v;
                *v = out;
                p = (p + 1) % len;
            }
        }
        self.pos = (start + buf.frames()) % len;
    }

    fn reset(&mut self) {
        for l in &mut self.lines {
            l.fill(0.0);
        }
        self.pos = 0;
    }
}

struct BusEngine {
    spec: BusSpec,
    fx: MultiFx,
    comp: DelayComp,
    buf: MultiBuf,
    gains: Vec<f32>,
}

impl BusEngine {
    fn new(spec: &BusSpec, layout: Layout, rate: u32, max_block: usize) -> Self {
        Self { spec: spec.clone(), fx: MultiFx::new(&spec.effects, layout, rate, max_block), comp: DelayComp::new(0, 0), buf: MultiBuf::new(layout.channels(), max_block), gains: vec![0.0; max_block] }
    }
}

/// Level readings after the latest block, for the app's meters.
#[derive(Clone, Debug, Default)]
pub struct Meters {
    /// Lane ids in meter order (the plan's lane order).
    pub lane_ids: Vec<u64>,
    /// Peak absolute sample per lane after its gain and ducking.
    pub lane_peaks: Vec<f32>,
    /// Bus ids in meter order (the plan's bus order).
    pub bus_ids: Vec<u64>,
    /// Peak absolute sample per bus after its gain.
    pub bus_peaks: Vec<f32>,
    /// Peak absolute sample of the master output.
    pub master_peak: f32,
    /// Peak per master channel (layout order).
    pub channel_peaks: Vec<f32>,
    /// Limiter gain reduction in dB, as a positive amount (0 when idle or absent).
    pub limiter_reduction_db: f32,
    /// Momentary (400 ms) loudness of the output in LUFS.
    pub momentary_lufs: f64,
    /// Short-term (3 s) loudness in LUFS.
    pub short_term_lufs: f64,
    /// Integrated loudness since the last reset, LUFS.
    pub integrated_lufs: f64,
    /// Highest true peak since the last reset, dBTP.
    pub true_peak_dbtp: f64,
    /// Current ducking gain in dB (0 when not ducking).
    pub ducking_db: f32,
    /// Whether the dialogue lanes currently carry speech.
    pub speech: bool,
}

/// The output limiter: the stereo one, or the linked N-channel one.
enum MasterLimiter {
    Stereo(Limiter),
    Multi(MultiLimiter),
}

impl MasterLimiter {
    fn new(rate: u32, layout: Layout, p: LimiterParams) -> Self {
        if layout == Layout::Stereo { MasterLimiter::Stereo(Limiter::new(rate, p)) } else { MasterLimiter::Multi(MultiLimiter::new(rate, layout.channels(), p)) }
    }
    fn latency(&self) -> usize {
        match self {
            MasterLimiter::Stereo(l) => l.latency(),
            MasterLimiter::Multi(l) => l.latency(),
        }
    }
    fn reset(&mut self) {
        match self {
            MasterLimiter::Stereo(l) => l.reset(),
            MasterLimiter::Multi(l) => l.reset(),
        }
    }
    fn set_params(&mut self, p: LimiterParams, rate: u32) {
        match self {
            MasterLimiter::Stereo(l) => l.set_params(p, rate),
            MasterLimiter::Multi(l) => l.set_params(p, rate),
        }
    }
    fn gain_reduction_db(&self) -> f32 {
        match self {
            MasterLimiter::Stereo(l) => l.gain_reduction_db(),
            MasterLimiter::Multi(l) => l.gain_reduction_db(),
        }
    }
    fn process(&mut self, buf: &mut MultiBuf) {
        match self {
            MasterLimiter::Stereo(l) => {
                let (a, b) = buf.pair_mut(0, 1);
                l.process(a, b);
            }
            MasterLimiter::Multi(l) => l.process(buf),
        }
    }
}

/// The mixer: a built [`MixPlan`] with a playhead.
pub struct Mixer {
    rate: u32,
    max_block: usize,
    quality: Quality,
    layout: Layout,
    clips: Vec<ClipEngine>,
    lanes: Vec<LaneEngine>,
    buses: Vec<BusEngine>,
    /// Lane indices, dialogue first so the ducking detector runs before music.
    lane_order: Vec<usize>,
    master: MasterSpec,
    master_fx: MultiFx,
    limiter: Option<MasterLimiter>,
    lfe: LfeFilter,
    vad: Vad,
    ducker: Ducker,
    meter: LoudnessMeter,
    meters: Meters,
    frame: i64,
    clip_buf: StereoBuf,
    scratch: StereoBuf,
    dialogue: StereoBuf,
    master_buf: MultiBuf,
    duck_gains: Vec<f32>,
    send_gains: Vec<f32>,
    /// Extra latency of the slowest bus, compensated on the others.
    bus_latency: usize,
}

/// A source used while a plan's sources are moved into the engines.
struct Silence;

impl AudioSource for Silence {
    fn rate(&self) -> u32 {
        48_000
    }
    fn channels(&self) -> usize {
        2
    }
    fn frames(&self) -> Option<u64> {
        Some(0)
    }
    fn read(&mut self, _: u64, _: &mut [f32], _: &mut [f32]) -> usize {
        0
    }
}

/// Adds a stereo block into the first two channels of `dst`, scaled.
fn add_stereo(dst: &mut MultiBuf, src: &StereoBuf, gain: f32) {
    let n = src.frames().min(dst.frames());
    let (l, r) = dst.pair_mut(0, 1);
    for i in 0..n {
        l[i] += src.left()[i] * gain;
        r[i] += src.right()[i] * gain;
    }
}

/// Pans a stereo block into a surround `dst` with gains moving linearly from
/// `g0` to `g1` across the block, scaled by `gains` per frame (or 1).
fn add_panned(dst: &mut MultiBuf, src: &StereoBuf, g0: &[[f32; MAX_CHANNELS]; 2], g1: &[[f32; MAX_CHANNELS]; 2], gains: Option<&[f32]>) {
    let n = src.frames().min(dst.frames());
    let inv = 1.0 / n.max(1) as f32;
    for c in 0..dst.channels() {
        let (a0, b0, a1, b1) = (g0[0][c], g0[1][c], g1[0][c], g1[1][c]);
        if a0 == 0.0 && b0 == 0.0 && a1 == 0.0 && b1 == 0.0 {
            continue;
        }
        let out = dst.chan_mut(c);
        for i in 0..n {
            let t = i as f32 * inv;
            let ga = a0 + (a1 - a0) * t;
            let gb = b0 + (b1 - b0) * t;
            let g = gains.map_or(1.0, |g| g[i]);
            out[i] += (src.left()[i] * ga + src.right()[i] * gb) * g;
        }
    }
}

impl Mixer {
    /// Builds the graph for `plan` at `rate` Hz with blocks of at most
    /// `max_block` frames, resampling at `quality`. Allocates; call on a
    /// control thread. The playhead starts at 0.
    pub fn new(rate: u32, max_block: usize, quality: Quality, mut plan: MixPlan) -> Self {
        let max_block = max_block.clamp(32, 1 << 16);
        let rate = rate.max(1);
        let layout = plan.layout;
        let lanes: Vec<LaneEngine> = plan.lanes.iter().map(|l| LaneEngine::new(l, rate, max_block)).collect();
        let lane_index = |lanes: &[LaneEngine], id: u64| lanes.iter().position(|l| l.id == id);
        let mut clips = Vec::with_capacity(plan.clips.len());
        for c in &mut plan.clips {
            if let Some(li) = lane_index(&lanes, c.lane) {
                clips.push(ClipEngine::new(c, li, rate, max_block, quality, Box::new(Silence)));
            }
        }
        let buses = plan.buses.iter().map(|b| BusEngine::new(b, layout, rate, max_block)).collect();
        let master = plan.master.clone();
        let master_fx = MultiFx::new(&master.effects, layout, rate, max_block);
        let limiter = master.limiter.map(|p| MasterLimiter::new(rate, layout, p));
        let ducker = Ducker::new(rate, master.ducking.unwrap_or_default());
        let mut m = Self {
            rate,
            max_block,
            quality,
            layout,
            clips,
            lanes,
            buses,
            lane_order: Vec::new(),
            master,
            master_fx,
            limiter,
            lfe: LfeFilter::new(rate, 120.0),
            vad: Vad::new(rate, plan.master.vad),
            ducker,
            meter: if layout == Layout::Stereo { LoudnessMeter::new(rate) } else { LoudnessMeter::with_layout(rate, layout) },
            meters: Meters::default(),
            frame: 0,
            clip_buf: StereoBuf::new(max_block),
            scratch: StereoBuf::new(max_block),
            dialogue: StereoBuf::new(max_block),
            master_buf: MultiBuf::new(layout.channels(), max_block),
            duck_gains: vec![1.0; max_block],
            send_gains: vec![0.0; max_block],
            bus_latency: 0,
        };
        m.rebuild_order();
        m
    }

    fn rebuild_order(&mut self) {
        let mut order: Vec<usize> = (0..self.lanes.len()).collect();
        order.sort_by_key(|&i| match self.lanes[i].role {
            LaneRole::Dialogue => 0,
            LaneRole::Other | LaneRole::Ambience => 1,
            LaneRole::Music => 2,
        });
        self.lane_order = order;
        // Routing: an explicit bus, else the first bus of the role's kind.
        for l in &mut self.lanes {
            let explicit = l.bus_id.and_then(|id| self.buses.iter().position(|b| b.spec.id == id));
            let by_role = || self.buses.iter().position(|b| b.spec.kind == l.role.bus_kind());
            l.dest = match explicit.or_else(by_role) {
                Some(i) => Dest::Bus(i),
                None => Dest::Master,
            };
            l.send_dest = l.sends.iter().map(|s| self.buses.iter().position(|b| b.spec.id == s.bus)).collect();
        }
        // Line the buses up with the slowest one.
        let lat: Vec<usize> = self.buses.iter().map(|b| b.fx.latency()).collect();
        let max = lat.iter().copied().max().unwrap_or(0);
        if max != self.bus_latency || self.buses.iter().zip(&lat).any(|(b, l)| b.comp.lines.first().map_or(0, Vec::len) != max - l) {
            let ch = self.layout.channels();
            for (b, l) in self.buses.iter_mut().zip(&lat) {
                b.comp = DelayComp::new(ch, max - l);
            }
        }
        self.bus_latency = max;
        self.meters.lane_ids = self.lanes.iter().map(|l| l.id).collect();
        self.meters.lane_peaks = vec![0.0; self.lanes.len()];
        self.meters.bus_ids = self.buses.iter().map(|b| b.spec.id).collect();
        self.meters.bus_peaks = vec![0.0; self.buses.len()];
        self.meters.channel_peaks = vec![0.0; self.layout.channels()];
    }

    /// Sample rate.
    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// The speaker layout rendered.
    pub fn layout(&self) -> Layout {
        self.layout
    }

    /// Largest block one `render` call processes at once (larger requests
    /// are split).
    pub fn max_block(&self) -> usize {
        self.max_block
    }

    /// Playhead in timeline microseconds.
    pub fn position(&self) -> Micros {
        frames_to_micros(self.frame, self.rate)
    }

    /// Playhead in frames.
    pub fn frame(&self) -> i64 {
        self.frame
    }

    /// Timeline time after which the output is silent: the latest clip end
    /// plus effect tails and the master latency.
    pub fn end(&self) -> Micros {
        let last = self.clips.iter().map(|c| c.span().1).max().unwrap_or(0);
        let bus_tail = self.buses.iter().map(|b| b.fx.tail()).max().unwrap_or(0) + self.bus_latency;
        let master_tail = self.master_fx.tail() + self.limiter.as_ref().map(|l| l.latency()).unwrap_or(0);
        frames_to_micros(last + (bus_tail + master_tail) as i64, self.rate)
    }

    /// Latency of the master bus in frames (bus and master effects plus the
    /// limiter look-ahead). Live playback schedules audio this much earlier;
    /// offline renders drop these frames from the start.
    pub fn master_latency(&self) -> usize {
        self.bus_latency + self.master_fx.latency() + self.limiter.as_ref().map(|l| l.latency()).unwrap_or(0)
    }

    /// Moves the playhead. Effect tails are cut; the next block starts clean.
    pub fn seek(&mut self, t: Micros) {
        self.seek_frame(micros_to_frames(t, self.rate));
    }

    /// Moves the playhead to a frame.
    pub fn seek_frame(&mut self, frame: i64) {
        self.frame = frame;
        for c in &mut self.clips {
            c.active = false;
        }
        for l in &mut self.lanes {
            for e in &mut l.effects {
                e.reset();
            }
        }
        for b in &mut self.buses {
            b.fx.reset();
            b.comp.reset();
        }
        self.master_fx.reset();
        self.lfe.reset();
        if let Some(l) = &mut self.limiter {
            l.reset();
        }
        self.vad.reset();
        self.ducker.reset();
    }

    /// Resets the loudness meter (at the start of an export).
    pub fn reset_loudness(&mut self) {
        self.meter.reset();
    }

    /// The running loudness meter over everything rendered since the last
    /// reset; `report()` gives the export's integrated loudness.
    pub fn loudness(&self) -> &LoudnessMeter {
        &self.meter
    }

    /// Level readings after the latest block.
    pub fn meters(&self) -> &Meters {
        &self.meters
    }

    /// Renders `out.frames()` frames from the playhead and advances it.
    /// Allocation-free; blocks longer than `max_block` are processed in
    /// parts. A surround mix comes out folded down to two channels (for
    /// monitoring); [`Mixer::render_multi`] gives every channel.
    pub fn render(&mut self, out: &mut StereoBuf) {
        let total = out.frames();
        let mut done = 0;
        while done < total {
            let n = (total - done).min(self.max_block);
            self.render_block(n);
            let (ol, or) = out.channels_mut();
            self.fold_down(&mut ol[done..done + n], &mut or[done..done + n]);
            done += n;
        }
    }

    fn fold_down(&self, l: &mut [f32], r: &mut [f32]) {
        if self.layout == Layout::Stereo {
            l.copy_from_slice(&self.master_buf.chan(0)[..l.len()]);
            r.copy_from_slice(&self.master_buf.chan(1)[..r.len()]);
            return;
        }
        l.fill(0.0);
        r.fill(0.0);
        for (c, (gl, gr)) in self.layout.downmix().iter().enumerate() {
            let x = self.master_buf.chan(c);
            for i in 0..l.len() {
                l[i] += x[i] * gl;
                r[i] += x[i] * gr;
            }
        }
    }

    /// Renders into interleaved stereo (`out.len() / 2` frames; surround is
    /// folded down).
    pub fn render_interleaved(&mut self, out: &mut [f32]) {
        let total = out.len() / 2;
        let mut done = 0;
        let mut l = [0.0f32; 256];
        let mut r = [0.0f32; 256];
        while done < total {
            let n = (total - done).min(self.max_block);
            self.render_block(n);
            let mut k = 0;
            while k < n {
                let m = (n - k).min(256);
                if self.layout == Layout::Stereo {
                    l[..m].copy_from_slice(&self.master_buf.chan(0)[k..k + m]);
                    r[..m].copy_from_slice(&self.master_buf.chan(1)[k..k + m]);
                } else {
                    l[..m].fill(0.0);
                    r[..m].fill(0.0);
                    for (c, (gl, gr)) in self.layout.downmix().iter().enumerate() {
                        let x = &self.master_buf.chan(c)[k..k + m];
                        for i in 0..m {
                            l[i] += x[i] * gl;
                            r[i] += x[i] * gr;
                        }
                    }
                }
                for i in 0..m {
                    out[2 * (done + k + i)] = l[i];
                    out[2 * (done + k + i) + 1] = r[i];
                }
                k += m;
            }
            done += n;
        }
    }

    /// Renders every channel of the layout into `out` (which must have the
    /// layout's channel count) and advances the playhead.
    pub fn render_multi(&mut self, out: &mut MultiBuf) {
        let total = out.frames();
        let mut done = 0;
        while done < total {
            let n = (total - done).min(self.max_block);
            self.render_block(n);
            for c in 0..out.channels().min(self.master_buf.channels()) {
                out.chan_mut(c)[done..done + n].copy_from_slice(self.master_buf.chan(c));
            }
            done += n;
        }
    }

    fn render_block(&mut self, n: usize) {
        let f0 = self.frame;
        let f1 = f0 + n as i64;
        let any_solo = self.lanes.iter().any(|l| l.solo);
        let surround = self.layout != Layout::Stereo;
        // Clips into lanes.
        for l in &mut self.lanes {
            l.buf.set_frames(n);
            l.buf.clear();
        }
        self.clip_buf.set_frames(n);
        for c in &mut self.clips {
            let (s, e) = c.span();
            let lane = &self.lanes[c.lane];
            let audible = !lane.muted && (!any_solo || lane.solo);
            if f1 <= s || f0 >= e || !audible {
                c.active = false;
                continue;
            }
            let (cl, cr) = self.clip_buf.channels_mut();
            c.render(f0, cl, cr, &mut self.scratch);
            self.lanes[c.lane].buf.add(&self.clip_buf);
        }
        // Lanes into buses (or the master), dialogue first.
        self.master_buf.set_frames(n);
        self.master_buf.clear();
        for b in &mut self.buses {
            b.buf.set_frames(n);
            b.buf.clear();
        }
        self.dialogue.set_frames(n);
        self.dialogue.clear();
        let t0 = frames_to_micros(f0, self.rate);
        let t1 = frames_to_micros(f1, self.rate);
        let ducking = self.master.ducking.is_some();
        let mut speech_decided = false;
        for &li in &self.lane_order {
            let lane = &mut self.lanes[li];
            let audible = !lane.muted && (!any_solo || lane.solo);
            if !audible {
                self.meters.lane_peaks[li] = 0.0;
                continue;
            }
            {
                let (bl, br) = lane.buf.channels_mut();
                for e in &mut lane.effects {
                    e.process(bl, br);
                }
            }
            if lane.sends.iter().any(|s| s.pre_fader) {
                lane.pre.copy_from(&lane.buf);
            }
            let (bl, br) = lane.buf.channels_mut();
            let gains = &mut lane.gains[..n];
            lane.volume_db.fill(t0, self.rate, gains);
            if lane.volume_db.is_constant() {
                let g = db_to_gain(gains[0]);
                gains.fill(g);
            } else {
                for g in gains.iter_mut() {
                    *g = db_to_gain(*g);
                }
            }
            if surround {
                // Balance is the panner's job in a surround mix.
                for i in 0..n {
                    bl[i] *= gains[i];
                    br[i] *= gains[i];
                }
            } else if let Some(pc) = lane.pan_curve.as_ref().filter(|c| !c.is_constant()) {
                let pans = &mut lane.pans[..n];
                pc.fill(t0, self.rate, pans);
                for i in 0..n {
                    let p = pans[i].clamp(-1.0, 1.0);
                    bl[i] *= gains[i] * 1.0f32.min(1.0 - p);
                    br[i] *= gains[i] * 1.0f32.min(1.0 + p);
                }
            } else {
                let pan = lane.pan_curve.as_ref().map_or(lane.pan, |c| c.eval(t0)).clamp(-1.0, 1.0);
                let (pl, pr) = (1.0f32.min(1.0 - pan), 1.0f32.min(1.0 + pan));
                for i in 0..n {
                    bl[i] *= gains[i] * pl;
                    br[i] *= gains[i] * pr;
                }
            }
            if ducking {
                match lane.role {
                    LaneRole::Dialogue => self.dialogue.add(&lane.buf),
                    LaneRole::Music => {
                        if !speech_decided {
                            self.vad.process_stereo(self.dialogue.left(), self.dialogue.right());
                            speech_decided = true;
                        }
                        self.ducker.fill(self.vad.is_speech(), &mut self.duck_gains[..n]);
                        lane.buf.apply_gain(&self.duck_gains[..n]);
                    }
                    LaneRole::Other | LaneRole::Ambience => {}
                }
            }
            self.meters.lane_peaks[li] = lane.buf.peak();
            // Route.
            let (g0, g1) = if surround {
                let p0 = lane.surround.at(t0);
                let p1 = lane.surround.at(t1);
                (source_gains(self.layout, p0, lane.surround.shape, 2), source_gains(self.layout, p1, lane.surround.shape, 2))
            } else {
                ([[0.0; MAX_CHANNELS]; 2], [[0.0; MAX_CHANNELS]; 2])
            };
            let target = match lane.dest {
                Dest::Bus(b) => &mut self.buses[b].buf,
                Dest::Master => &mut self.master_buf,
            };
            if surround {
                add_panned(target, &lane.buf, &g0, &g1, None);
            } else {
                add_stereo(target, &lane.buf, 1.0);
            }
            // Sends.
            for (s, dest) in lane.sends.iter().zip(&lane.send_dest) {
                let Some(b) = *dest else { continue };
                let sg = &mut self.send_gains[..n];
                s.level_db.fill(t0, self.rate, sg);
                for g in sg.iter_mut() {
                    *g = db_to_gain(*g);
                }
                let src = if s.pre_fader { &lane.pre } else { &lane.buf };
                let target = &mut self.buses[b].buf;
                if surround {
                    add_panned(target, src, &g0, &g1, Some(sg));
                } else {
                    let (l, r) = target.pair_mut(0, 1);
                    for i in 0..n {
                        l[i] += src.left()[i] * sg[i];
                        r[i] += src.right()[i] * sg[i];
                    }
                }
            }
        }
        if ducking && !speech_decided {
            // No music lane this block: keep the detector and ramp running.
            self.vad.process_stereo(self.dialogue.left(), self.dialogue.right());
            self.ducker.fill(self.vad.is_speech(), &mut self.duck_gains[..n]);
        }
        // Buses into the master.
        let bus_solo = self.buses.iter().any(|b| b.spec.solo && b.spec.kind != BusKind::Aux);
        for (bi, b) in self.buses.iter_mut().enumerate() {
            b.fx.process(&mut b.buf);
            b.comp.process(&mut b.buf);
            let gains = &mut b.gains[..n];
            b.spec.volume_db.fill(t0, self.rate, gains);
            if b.spec.volume_db.is_constant() {
                let g = db_to_gain(gains[0]);
                if (g - 1.0).abs() > 1e-9 {
                    b.buf.scale(g);
                }
            } else {
                for g in gains.iter_mut() {
                    *g = db_to_gain(*g);
                }
                b.buf.apply_gain(gains);
            }
            let audible = !b.spec.muted && (!bus_solo || b.spec.solo || b.spec.kind == BusKind::Aux);
            if !audible {
                self.meters.bus_peaks[bi] = 0.0;
                continue;
            }
            self.meters.bus_peaks[bi] = b.buf.peak();
            self.master_buf.add(&b.buf);
        }
        // Master.
        let g = db_to_gain(self.master.volume_db);
        if (g - 1.0).abs() > 1e-9 {
            self.master_buf.scale(g);
        }
        self.master_fx.process(&mut self.master_buf);
        if surround {
            self.lfe.process(&mut self.master_buf);
        }
        if let Some(l) = &mut self.limiter {
            l.process(&mut self.master_buf);
        }
        if surround {
            self.meter.process_multi(&self.master_buf);
        } else {
            self.meter.process(self.master_buf.chan(0), self.master_buf.chan(1));
        }
        self.meters.master_peak = self.master_buf.peak();
        for c in 0..self.master_buf.channels() {
            self.meters.channel_peaks[c] = self.master_buf.chan_peak(c);
        }
        self.meters.limiter_reduction_db = self.limiter.as_ref().map(|l| l.gain_reduction_db()).unwrap_or(0.0);
        self.meters.momentary_lufs = self.meter.momentary_lufs();
        self.meters.short_term_lufs = self.meter.short_term_lufs();
        self.meters.integrated_lufs = self.meter.integrated_lufs();
        self.meters.true_peak_dbtp = self.meter.true_peak_dbtp();
        self.meters.ducking_db = if ducking { self.ducker.gain_db() } else { 0.0 };
        self.meters.speech = ducking && self.vad.is_speech();
        self.frame = f1;
    }

    /// Replaces the plan, keeping the signal state of everything that did not
    /// change structurally (so a volume tweak during playback does not cut a
    /// reverb tail). Clips whose source, speed, direction or effects changed,
    /// and new clips, get fresh engines. A new layout rebuilds the buses and
    /// master. Allocates; call from the control thread while holding the
    /// same lock the audio thread renders under.
    pub fn update(&mut self, mut plan: MixPlan) {
        let relayout = plan.layout != self.layout;
        self.layout = plan.layout;
        // Lanes: match by id, keep order from the plan.
        let old_lanes = std::mem::take(&mut self.lanes);
        let mut old_lanes: Vec<Option<LaneEngine>> = old_lanes.into_iter().map(Some).collect();
        for spec in &plan.lanes {
            let found = old_lanes.iter_mut().find(|l| l.as_ref().is_some_and(|l| l.id == spec.id)).and_then(Option::take);
            let lane = match found {
                Some(mut l) => {
                    l.update(spec, self.rate, self.max_block);
                    l
                }
                None => LaneEngine::new(spec, self.rate, self.max_block),
            };
            self.lanes.push(lane);
        }
        // Buses: keep engines whose effects did not change.
        let old_buses = std::mem::take(&mut self.buses);
        let mut old_buses: Vec<Option<BusEngine>> = old_buses.into_iter().map(Some).collect();
        for spec in &plan.buses {
            let found = if relayout { None } else { old_buses.iter_mut().find(|b| b.as_ref().is_some_and(|b| b.spec.id == spec.id && b.spec.effects == spec.effects)).and_then(Option::take) };
            let bus = match found {
                Some(mut b) => {
                    b.spec = spec.clone();
                    b
                }
                None => BusEngine::new(spec, self.layout, self.rate, self.max_block),
            };
            self.buses.push(bus);
        }
        // Clips.
        let old_clips = std::mem::take(&mut self.clips);
        let mut old_clips: Vec<Option<ClipEngine>> = old_clips.into_iter().map(Some).collect();
        for spec in &mut plan.clips {
            let Some(li) = self.lanes.iter().position(|l| l.id == spec.lane) else { continue };
            let reuse = old_clips.iter_mut().find(|c| c.as_ref().is_some_and(|c| c.id == spec.id && Self::same_structure(c, spec))).and_then(Option::take);
            let engine = match reuse {
                Some(mut c) => {
                    c.lane = li;
                    c.update_params(spec);
                    c.source = std::mem::replace(&mut spec.source, Box::new(Silence));
                    c
                }
                None => ClipEngine::new(spec, li, self.rate, self.max_block, self.quality, Box::new(Silence)),
            };
            self.clips.push(engine);
        }
        // Master.
        if relayout || self.master.effects != plan.master.effects {
            self.master_fx = MultiFx::new(&plan.master.effects, self.layout, self.rate, self.max_block);
        }
        if relayout {
            self.master_buf = MultiBuf::new(self.layout.channels(), self.max_block);
            self.meter = if self.layout == Layout::Stereo { LoudnessMeter::new(self.rate) } else { LoudnessMeter::with_layout(self.rate, self.layout) };
            self.limiter = None;
        }
        match plan.master.limiter {
            Some(p) => match &mut self.limiter {
                Some(l) => l.set_params(p, self.rate),
                None => self.limiter = Some(MasterLimiter::new(self.rate, self.layout, p)),
            },
            None => self.limiter = None,
        }
        self.ducker.set_params(plan.master.ducking.unwrap_or_default());
        self.vad.set_params(plan.master.vad);
        self.master = plan.master.clone();
        self.rebuild_order();
    }

    /// Whether an engine can keep running for a new spec of the same clip.
    fn same_structure(engine: &ClipEngine, spec: &ClipSpec) -> bool {
        engine.src_rate == spec.source.rate()
            && engine.mono == (spec.source.channels() == 1)
            && engine.preserve_pitch == spec.preserve_pitch
            && engine.reversed == spec.reversed
            && engine.speed == spec.speed
            && engine.spec_effects == spec.effects
    }
}

/// Renders `plan` from `start` to `end` offline at `rate` into planar stereo,
/// compensating the master latency so the output's first frame is timeline
/// time `start`. Allocates.
pub fn mix_offline(plan: MixPlan, rate: u32, start: Micros, end: Micros, quality: Quality) -> (Vec<f32>, Vec<f32>) {
    let mut m = Mixer::new(rate, 1024, quality, plan);
    let frames = (micros_to_frames(end, rate) - micros_to_frames(start, rate)).max(0) as usize;
    // The master's output lags its input by `latency` frames: render that
    // many extra and drop them from the front.
    let latency = m.master_latency();
    m.seek_frame(micros_to_frames(start, rate));
    let mut buf = StereoBuf::new(1024);
    let mut left = Vec::with_capacity(frames + latency);
    let mut right = Vec::with_capacity(frames + latency);
    while left.len() < frames + latency {
        let n = (frames + latency - left.len()).min(1024);
        buf.set_frames(n);
        m.render(&mut buf);
        left.extend_from_slice(buf.left());
        right.extend_from_slice(buf.right());
    }
    left.drain(..latency);
    right.drain(..latency);
    (left, right)
}

/// Renders `plan` offline in its own layout: one planar channel per
/// speaker (crate order, see [`crate::surround`]), latency compensated like
/// [`mix_offline`]. Allocates.
pub fn mix_offline_layout(plan: MixPlan, rate: u32, start: Micros, end: Micros, quality: Quality) -> Vec<Vec<f32>> {
    let ch = plan.layout.channels();
    let mut m = Mixer::new(rate, 1024, quality, plan);
    let frames = (micros_to_frames(end, rate) - micros_to_frames(start, rate)).max(0) as usize;
    let latency = m.master_latency();
    m.seek_frame(micros_to_frames(start, rate));
    let mut buf = MultiBuf::new(ch, 1024);
    let mut out: Vec<Vec<f32>> = (0..ch).map(|_| Vec::with_capacity(frames + latency)).collect();
    while out[0].len() < frames + latency {
        let n = (frames + latency - out[0].len()).min(1024);
        buf.set_frames(n);
        m.render_multi(&mut buf);
        for (c, o) in out.iter_mut().enumerate() {
            o.extend_from_slice(buf.chan(c));
        }
    }
    for o in &mut out {
        o.drain(..latency);
    }
    out
}

/// Measures a whole source's loudness (for "normalise clip" and the
/// inspector's LUFS readout). Allocates.
pub fn measure_source(source: &mut dyn AudioSource) -> crate::loudness::LoudnessReport {
    let rate = source.rate();
    let mut meter = LoudnessMeter::new(rate);
    let mut l = vec![0.0; 4096];
    let mut r = vec![0.0; 4096];
    let mut at = 0u64;
    loop {
        let k = source.read(at, &mut l, &mut r);
        if k == 0 {
            break;
        }
        meter.process(&l[..k], &r[..k]);
        at += k as u64;
    }
    meter.report()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automation::{Ease, Key};
    use crate::pitch::dominant_frequency;
    use crate::SECOND;

    const RATE: u32 = 48_000;

    fn sine(freq: f32, amp: f32, frames: usize, rate: u32) -> Vec<f32> {
        (0..frames).map(|i| amp * (std::f32::consts::TAU * freq * i as f32 / rate as f32).sin()).collect()
    }

    fn render(plan: MixPlan, frames: usize, block: usize) -> (Vec<f32>, Vec<f32>) {
        let mut m = Mixer::new(RATE, block, Quality::High, plan);
        let mut buf = StereoBuf::new(block);
        let (mut l, mut r) = (Vec::new(), Vec::new());
        while l.len() < frames {
            let n = (frames - l.len()).min(block);
            buf.set_frames(n);
            m.render(&mut buf);
            l.extend_from_slice(buf.left());
            r.extend_from_slice(buf.right());
        }
        (l, r)
    }

    fn no_limiter() -> MasterSpec {
        MasterSpec { limiter: None, ..Default::default() }
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
    }

    #[test]
    fn single_clip_is_sample_exact_and_starts_on_its_frame() {
        let src = sine(1000.0, 0.5, RATE as usize, RATE);
        let start_frame = 1234i64; // not block aligned
        let start = frames_to_micros(start_frame, RATE);
        let mut plan = MixPlan { lanes: vec![LaneSpec::new(1, LaneRole::Other)], master: no_limiter(), ..Default::default() };
        plan.clips.push(ClipSpec::new(7, 1, start, SECOND / 2, Box::new(MemorySource::stereo(src.clone(), src.clone(), RATE))));
        let (l, r) = render(plan, RATE as usize, 256);
        for i in 0..start_frame as usize {
            assert_eq!(l[i], 0.0, "silence before the clip at {i}");
        }
        let len = micros_to_frames(SECOND / 2, RATE) as usize;
        for i in 0..len {
            let expect = src[i];
            assert!((l[start_frame as usize + i] - expect).abs() < 1e-3, "frame {i}: {} vs {expect}", l[start_frame as usize + i]);
            assert_eq!(l[start_frame as usize + i], r[start_frame as usize + i]);
        }
        for i in start_frame as usize + len..l.len() {
            assert_eq!(l[i], 0.0, "silence after the clip at {i}");
        }
    }

    #[test]
    fn volume_keyframes_land_on_the_sample_and_fades_apply() {
        let src = vec![1.0f32; RATE as usize];
        let mut clip = ClipSpec::new(1, 1, 0, SECOND, Box::new(MemorySource::mono(src, RATE)));
        clip.volume_db = Curve::new(vec![Key::new(0, 0.0), Key::new(SECOND / 2, -12.0), Key::eased(3 * SECOND / 4, -6.0, Ease::Hold)], 0.0);
        clip.fade_in = SECOND / 10;
        clip.fade_out = SECOND / 10;
        clip.fade_shape = FadeShape::Linear;
        let plan = MixPlan { clips: vec![clip], lanes: vec![LaneSpec::new(1, LaneRole::Other)], master: no_limiter(), ..Default::default() };
        let (l, _) = render(plan, RATE as usize, 512);
        assert_eq!(l[0], 0.0, "fade-in starts at zero");
        assert!((l[2400] - db_to_gain(-1.2) * 0.5).abs() < 1e-3, "half-way through the fade: ramp 0.5 at the interpolated −1.2 dB, got {}", l[2400]);
        assert!((l[RATE as usize / 2] - db_to_gain(-12.0)).abs() < 1e-4, "the −12 dB key is exact on its sample");
        assert!((l[RATE as usize * 3 / 4 + 100] - db_to_gain(-6.0)).abs() < 1e-4, "held value after the hold key");
        assert!(l[RATE as usize - 1] < 1e-3, "fade-out reaches silence");
    }

    #[test]
    fn mute_solo_lane_gain_and_pan() {
        let src = vec![0.5f32; RATE as usize / 2];
        let mk = |id, lane| ClipSpec::new(id, lane, 0, SECOND / 2, Box::new(MemorySource::mono(src.clone(), RATE)) as Box<dyn AudioSource>);
        let mut la = LaneSpec::new(1, LaneRole::Other);
        la.volume_db = Curve::constant(-6.0);
        let mut lb = LaneSpec::new(2, LaneRole::Other);
        lb.muted = true;
        let plan = MixPlan { clips: vec![mk(1, 1), mk(2, 2)], lanes: vec![la.clone(), lb.clone()], master: no_limiter(), ..Default::default() };
        let (l, _) = render(plan, 1000, 256);
        assert!((l[500] - 0.5 * db_to_gain(-6.0)).abs() < 1e-4, "lane gain applies, muted lane contributes nothing");
        // Solo lane b: lane a goes quiet even though it is not muted.
        lb.muted = false;
        lb.solo = true;
        let plan = MixPlan { clips: vec![mk(1, 1), mk(2, 2)], lanes: vec![la, lb], master: no_limiter(), ..Default::default() };
        let (l, _) = render(plan, 1000, 256);
        assert!((l[500] - 0.5).abs() < 1e-4, "only the soloed lane sounds");
        // Pan a mono clip hard left: right is silent, left is +3 dB over centre.
        let mut c = mk(3, 1);
        c.pan = Curve::constant(-1.0);
        let plan = MixPlan { clips: vec![c], lanes: vec![LaneSpec::new(1, LaneRole::Other)], master: no_limiter(), ..Default::default() };
        let (l, r) = render(plan, 1000, 256);
        assert!((l[500] - 0.5 * std::f32::consts::SQRT_2).abs() < 1e-4);
        assert!(r[500].abs() < 1e-6);
    }

    #[test]
    fn overlapping_clips_sum_and_block_size_does_not_matter() {
        let a = sine(440.0, 0.3, RATE as usize, RATE);
        let b = sine(660.0, 0.2, RATE as usize, RATE);
        let plan = || {
            let mut p = MixPlan { lanes: vec![LaneSpec::new(1, LaneRole::Other), LaneSpec::new(2, LaneRole::Other)], master: no_limiter(), ..Default::default() };
            p.clips.push(ClipSpec::new(1, 1, 0, SECOND, Box::new(MemorySource::mono(a.clone(), RATE))));
            p.clips.push(ClipSpec::new(2, 2, SECOND / 4, SECOND / 2, Box::new(MemorySource::mono(b.clone(), RATE))));
            p
        };
        let (l1, _) = render(plan(), RATE as usize, 256);
        let (l2, _) = render(plan(), RATE as usize, 1024);
        let (l3, _) = render(plan(), RATE as usize, 333);
        for i in 0..RATE as usize {
            let in_b = i >= RATE as usize / 4 && i < RATE as usize * 3 / 4;
            let expect = a[i] + if in_b { b[i - RATE as usize / 4] } else { 0.0 };
            assert!((l1[i] - expect).abs() < 2e-3, "frame {i}");
            assert!((l1[i] - l2[i]).abs() < 1e-5 && (l1[i] - l3[i]).abs() < 1e-5, "block size changes the output at {i}");
        }
    }

    #[test]
    fn seeking_matches_a_continuous_render() {
        let a = sine(440.0, 0.3, RATE as usize, RATE);
        let plan = || {
            let mut p = MixPlan { lanes: vec![LaneSpec::new(1, LaneRole::Other)], master: no_limiter(), ..Default::default() };
            let mut c = ClipSpec::new(1, 1, SECOND / 10, SECOND / 2, Box::new(MemorySource::mono(a.clone(), RATE)));
            c.fade_in = SECOND / 20;
            p.clips.push(c);
            p
        };
        let (full, _) = render(plan(), RATE as usize, 512);
        let mut m = Mixer::new(RATE, 512, Quality::High, plan());
        let target = SECOND / 4 + 777;
        m.seek(target);
        let f = micros_to_frames(target, RATE) as usize;
        let mut buf = StereoBuf::new(512);
        m.render(&mut buf);
        for i in 0..512 {
            assert!((buf.left()[i] - full[f + i]).abs() < 1e-5, "frame {i} after seek");
        }
        assert_eq!(m.position(), frames_to_micros(f as i64 + 512, RATE));
    }

    #[test]
    fn other_sample_rate_keeps_frequency_and_level() {
        let src_rate = 44_100;
        let src = sine(1000.0, 0.5, src_rate as usize, src_rate);
        let mut p = MixPlan { lanes: vec![LaneSpec::new(1, LaneRole::Other)], master: no_limiter(), ..Default::default() };
        p.clips.push(ClipSpec::new(1, 1, 0, SECOND, Box::new(MemorySource::mono(src, src_rate))));
        let (l, _) = render(p, RATE as usize, 512);
        let mid = &l[RATE as usize / 4..RATE as usize * 3 / 4];
        let f = dominant_frequency(mid, RATE, 200.0, 4000.0).unwrap();
        assert!((f - 1000.0).abs() < 5.0, "frequency {f}");
        assert!((rms(mid) - 0.5 / std::f32::consts::SQRT_2).abs() < 0.01, "rms {}", rms(mid));
    }

    #[test]
    fn speed_without_pitch_preservation_is_varispeed() {
        let src = sine(440.0, 0.5, 2 * RATE as usize, RATE);
        let mut p = MixPlan { lanes: vec![LaneSpec::new(1, LaneRole::Other)], master: no_limiter(), ..Default::default() };
        let mut c = ClipSpec::new(1, 1, 0, SECOND, Box::new(MemorySource::mono(src, RATE)));
        c.speed = SpeedSpec::Constant(2.0);
        c.preserve_pitch = false;
        p.clips.push(c);
        let (l, _) = render(p, RATE as usize + 1000, 512);
        let f = dominant_frequency(&l[4800..RATE as usize - 4800], RATE, 200.0, 4000.0).unwrap();
        assert!((f - 880.0).abs() < 5.0, "doubled speed doubles the pitch: {f}");
        assert!(rms(&l[..RATE as usize]) > 0.3, "the clip plays for its whole timeline length");
        assert!(l[RATE as usize + 500].abs() < 1e-6, "and stops at its end");
    }

    #[test]
    fn speed_with_pitch_preservation_keeps_the_pitch() {
        let src = sine(440.0, 0.5, 2 * RATE as usize, RATE);
        let mut p = MixPlan { lanes: vec![LaneSpec::new(1, LaneRole::Other)], master: no_limiter(), ..Default::default() };
        let mut c = ClipSpec::new(1, 1, 0, SECOND, Box::new(MemorySource::mono(src, RATE)));
        c.speed = SpeedSpec::Constant(2.0);
        p.clips.push(c);
        let (l, _) = render(p, RATE as usize, 512);
        let f = dominant_frequency(&l[4800..RATE as usize - 4800], RATE, 200.0, 4000.0).unwrap();
        assert!((f - 440.0).abs() < 5.0, "pitch stays at 440: {f}");
        let level = rms(&l[4800..RATE as usize - 4800]);
        assert!((level - 0.5 / std::f32::consts::SQRT_2).abs() < 0.05, "level {level}");
        assert!(l[..200].iter().all(|v| v.abs() < 0.6), "no garbage at the start");
    }

    #[test]
    fn varying_speed_integrates_source_position() {
        // Speed ramps 1 → 3 over one second: the source advances by 2 s.
        let s = SpeedSpec::Varying(Curve::new(vec![Key::new(0, 1.0), Key::new(SECOND, 3.0)], 1.0));
        assert!((s.integrate(SECOND) - 2 * SECOND).abs() <= 2);
        assert!((s.integrate(SECOND / 2) - (SECOND / 2 + SECOND / 4)).abs() <= 2);
        assert!((s.integrate(2 * SECOND) - 5 * SECOND).abs() <= 2, "flat at 3 after the last key");
        assert_eq!(SpeedSpec::Constant(0.5).integrate(SECOND), SECOND / 2);
        // And the engine lands on the right sample after a seek: a ramp source
        // read at a speed that reaches the ramp value tells the position.
        let ramp: Vec<f32> = (0..4 * RATE as usize).map(|i| i as f32 / RATE as f32).collect();
        let mut p = MixPlan { lanes: vec![LaneSpec::new(1, LaneRole::Other)], master: no_limiter(), ..Default::default() };
        let mut c = ClipSpec::new(1, 1, 0, 2 * SECOND, Box::new(MemorySource::mono(ramp, RATE)));
        c.speed = s;
        c.preserve_pitch = false;
        p.clips.push(c);
        let mut m = Mixer::new(RATE, 512, Quality::High, p);
        m.seek(SECOND);
        let mut buf = StereoBuf::new(512);
        m.render(&mut buf);
        // Source time at clip time 1 s is 2 s (integral), so the ramp reads ~2.0.
        assert!((buf.left()[8] - 2.0).abs() < 0.01, "source position after seek: {}", buf.left()[8]);
    }

    #[test]
    fn reversed_clip_plays_backwards_from_source_in() {
        let ramp: Vec<f32> = (0..RATE as usize).map(|i| i as f32 / RATE as f32).collect();
        let mut p = MixPlan { lanes: vec![LaneSpec::new(1, LaneRole::Other)], master: no_limiter(), ..Default::default() };
        let mut c = ClipSpec::new(1, 1, 0, SECOND / 2, Box::new(MemorySource::mono(ramp, RATE)));
        c.reversed = true;
        c.source_in = SECOND; // start at the end of the source and run back
        p.clips.push(c);
        let (l, _) = render(p, RATE as usize / 2, 512);
        assert!((l[0] - (RATE as f32 - 1.0) / RATE as f32).abs() < 1e-3, "first frame is the last source sample: {}", l[0]);
        assert!(l[1000] < l[0] && l[20_000] < l[1000], "decreasing");
        assert!((l[24_000 - 1] - 0.5).abs() < 2e-3, "half a second in, the ramp reads 0.5: {}", l[23_999]);
    }

    #[test]
    fn latency_adding_effect_stays_aligned() {
        let src = sine(440.0, 0.5, RATE as usize, RATE);
        let plan = |effects: Vec<EffectSpec>| {
            let mut p = MixPlan { lanes: vec![LaneSpec::new(1, LaneRole::Other)], master: no_limiter(), ..Default::default() };
            let mut c = ClipSpec::new(1, 1, SECOND / 10, SECOND / 2, Box::new(MemorySource::mono(src.clone(), RATE)));
            c.effects = effects;
            p.clips.push(c);
            p
        };
        let (dry, _) = render(plan(vec![]), RATE as usize, 512);
        let (wet, _) = render(plan(vec![EffectSpec::PitchShift { semitones: 0.0 }]), RATE as usize, 512);
        let start = RATE as usize / 10;
        // WSOLA may pick a grain up to its search tolerance (8 ms) early, so
        // allow that much before the clip; nothing earlier.
        assert!(wet[..start - 400].iter().all(|v| v.abs() < 1e-6), "silent before the clip");
        // A zero shift is a pure delay inside the shifter; compensated, it lines up.
        for i in start + 2000..start + 20_000 {
            assert!((dry[i] - wet[i]).abs() < 0.08, "frame {i}: {} vs {}", dry[i], wet[i]);
        }
        assert!(rms(&wet[start + 100..start + 2000]) > 0.2, "the clip's attack is present");
    }

    #[test]
    fn master_limiter_holds_the_ceiling_and_meters_read() {
        let src = sine(100.0, 0.9, RATE as usize, RATE);
        let mut p = MixPlan { lanes: vec![LaneSpec::new(1, LaneRole::Other), LaneSpec::new(2, LaneRole::Other)], ..Default::default() };
        p.clips.push(ClipSpec::new(1, 1, 0, SECOND, Box::new(MemorySource::mono(src.clone(), RATE))));
        p.clips.push(ClipSpec::new(2, 2, 0, SECOND, Box::new(MemorySource::mono(src, RATE))));
        let mut m = Mixer::new(RATE, 512, Quality::High, p);
        let mut buf = StereoBuf::new(512);
        let ceiling = db_to_gain(-1.0);
        let mut peak = 0.0f32;
        for _ in 0..90 {
            m.render(&mut buf);
            peak = peak.max(buf.peak());
        }
        assert!(peak <= ceiling + 1e-3, "peak {peak} over ceiling {ceiling}");
        let meters = m.meters();
        assert_eq!(meters.lane_ids, vec![1, 2]);
        assert!(meters.lane_peaks[0] > 0.8 && meters.master_peak <= ceiling + 1e-3);
        assert!(meters.limiter_reduction_db > 3.0, "two 0.9 tones sum to 1.8: strong reduction, got {}", meters.limiter_reduction_db);
        assert!(meters.momentary_lufs > -10.0 && meters.momentary_lufs < 0.0);
    }

    #[test]
    fn integrated_loudness_of_a_mix_matches_the_tone() {
        let amp = db_to_gain(-23.0);
        let src = sine(1000.0, amp, 10 * RATE as usize, RATE);
        let mut p = MixPlan { lanes: vec![LaneSpec::new(1, LaneRole::Other)], master: no_limiter(), ..Default::default() };
        p.clips.push(ClipSpec::new(1, 1, 0, 10 * SECOND, Box::new(MemorySource::stereo(src.clone(), src, RATE))));
        let mut m = Mixer::new(RATE, 1024, Quality::High, p);
        let mut buf = StereoBuf::new(1024);
        for _ in 0..(10 * RATE as usize / 1024) {
            m.render(&mut buf);
        }
        let lufs = m.loudness().integrated_lufs();
        assert!((lufs + 23.0).abs() < 0.2, "{lufs}");
        assert!((m.end() - 10 * SECOND).abs() < 1000);
    }

    /// Speech-like burst: harmonics under a syllable-rate envelope.
    fn pseudo_speech(rate: u32, frames: usize) -> Vec<f32> {
        (0..frames)
            .map(|i| {
                let t = i as f32 / rate as f32;
                let syl = 0.6 + 0.4 * (std::f32::consts::TAU * 4.0 * t).sin();
                let mut v = 0.0;
                for h in 1..12 {
                    let f = 150.0 * h as f32 * (1.0 + 0.02 * (t * 3.0).sin());
                    let formant = (-(f - 700.0).powi(2) / 2.0e5).exp() + 0.6 * (-(f - 1500.0).powi(2) / 4.0e5).exp();
                    v += (std::f32::consts::TAU * f * t).sin() * formant / h as f32;
                }
                v * syl * 0.3
            })
            .collect()
    }

    #[test]
    fn music_ducks_under_dialogue_and_recovers() {
        let music = sine(220.0, 0.3, 6 * RATE as usize, RATE);
        let speech = pseudo_speech(RATE, 2 * RATE as usize);
        let mut p = MixPlan {
            lanes: vec![LaneSpec::new(1, LaneRole::Music), LaneSpec::new(2, LaneRole::Dialogue)],
            master: MasterSpec { limiter: None, ducking: Some(DuckingParams { amount_db: 12.0, attack_ms: 50.0, hold_ms: 100.0, release_ms: 200.0 }), ..Default::default() },
            ..Default::default()
        };
        p.clips.push(ClipSpec::new(1, 1, 0, 6 * SECOND, Box::new(MemorySource::mono(music, RATE))));
        // Speech from 2 s to 4 s; the ducking gain is read from the meters.
        p.clips.push(ClipSpec::new(2, 2, 2 * SECOND, 2 * SECOND, Box::new(MemorySource::mono(speech, RATE))));
        let mut m = Mixer::new(RATE, 512, Quality::High, p);
        let mut buf = StereoBuf::new(512);
        let mut duck_at = Vec::new();
        let mut speech_at = Vec::new();
        for _ in 0..(6 * RATE as usize / 512) {
            m.render(&mut buf);
            duck_at.push(m.meters().ducking_db);
            speech_at.push(m.meters().speech);
        }
        let blocks_per_s = RATE as usize / 512;
        assert!(duck_at[blocks_per_s..2 * blocks_per_s].iter().all(|d| *d > -0.01), "no ducking before speech");
        assert!(speech_at[3 * blocks_per_s..4 * blocks_per_s - 2].iter().filter(|s| **s).count() > blocks_per_s / 2, "speech detected while it plays");
        assert!(duck_at[3 * blocks_per_s..4 * blocks_per_s - 5].iter().all(|d| *d < -10.0), "fully ducked during speech: {:?}", &duck_at[3 * blocks_per_s..3 * blocks_per_s + 5]);
        assert!(duck_at[5 * blocks_per_s + 5..].iter().all(|d| *d > -0.5), "recovered after release: {:?}", &duck_at[5 * blocks_per_s + 5..5 * blocks_per_s + 10]);
    }

    #[test]
    fn update_keeps_engines_and_applies_new_gains() {
        let src = vec![0.5f32; RATE as usize];
        let plan = |db: f32| {
            let mut p = MixPlan { lanes: vec![LaneSpec::new(1, LaneRole::Other)], master: no_limiter(), ..Default::default() };
            let mut c = ClipSpec::new(1, 1, 0, SECOND, Box::new(MemorySource::mono(src.clone(), RATE)));
            c.volume_db = Curve::constant(db);
            p.clips.push(c);
            p
        };
        let mut m = Mixer::new(RATE, 512, Quality::High, plan(0.0));
        let mut buf = StereoBuf::new(512);
        m.render(&mut buf);
        assert!((buf.left()[100] - 0.5).abs() < 1e-5);
        m.update(plan(-6.0));
        m.render(&mut buf);
        assert!((buf.left()[100] - 0.5 * db_to_gain(-6.0)).abs() < 1e-5, "new gain applies without a seek");
        assert_eq!(m.frame(), 1024, "the playhead kept going");
    }

    #[test]
    fn offline_mix_compensates_master_latency() {
        let src = sine(500.0, 0.4, RATE as usize, RATE);
        let mut p = MixPlan { lanes: vec![LaneSpec::new(1, LaneRole::Other)], ..Default::default() };
        p.clips.push(ClipSpec::new(1, 1, SECOND / 10, SECOND / 2, Box::new(MemorySource::mono(src.clone(), RATE))));
        let (l, r) = mix_offline(p, RATE, 0, SECOND, Quality::Best);
        assert_eq!(l.len(), RATE as usize);
        assert_eq!(r.len(), RATE as usize);
        let start = RATE as usize / 10;
        assert!(l[..start].iter().all(|v| v.abs() < 1e-6));
        for i in 0..1000 {
            assert!((l[start + i] - src[i]).abs() < 2e-3, "frame {i}: {} vs {}", l[start + i], src[i]);
        }
        // measure_source and the normalisation helper work on a source.
        let mut s = MemorySource::mono(sine(1000.0, db_to_gain(-20.0), 5 * RATE as usize, RATE), RATE);
        let report = measure_source(&mut s);
        assert!((report.integrated_lufs + 20.0).abs() < 0.3, "{}", report.integrated_lufs);
    }

    #[test]
    fn render_path_does_not_allocate() {
        // A demanding plan: another sample rate, pitch-preserved varying
        // speed, a reversed clip, clip effects with latency and tails, lane
        // and master effects, ducking and the limiter.
        let src44 = sine(330.0, 0.3, 2 * 44_100, 44_100);
        let voice = pseudo_speech(RATE, 2 * RATE as usize);
        let music = sine(110.0, 0.3, 4 * RATE as usize, RATE);
        let mut p = MixPlan {
            lanes: vec![LaneSpec::new(1, LaneRole::Dialogue), LaneSpec::new(2, LaneRole::Music), LaneSpec::new(3, LaneRole::Other)],
            master: MasterSpec { ducking: Some(DuckingParams::default()), effects: vec![EffectSpec::flat_eq()], ..Default::default() },
            ..Default::default()
        };
        p.lanes[0].effects = vec![EffectSpec::Compressor(CompressorParams::VOICE), EffectSpec::DeEsser(DeEsserParams::default())];
        let mut c1 = ClipSpec::new(1, 1, SECOND / 3, 3 * SECOND / 2, Box::new(MemorySource::mono(voice, RATE)));
        c1.effects = vec![EffectSpec::Voice { style: VoiceStyle::Cathedral, amount: 0.7 }, EffectSpec::PitchShift { semitones: -2.0 }];
        c1.volume_db = Curve::new(vec![Key::new(0, -3.0), Key::new(SECOND, 0.0)], 0.0);
        c1.pan = Curve::new(vec![Key::new(0, -0.5), Key::new(SECOND, 0.5)], 0.0);
        c1.fade_in = SECOND / 10;
        let mut c2 = ClipSpec::new(2, 2, 0, 3 * SECOND, Box::new(MemorySource::mono(music, RATE)));
        c2.speed = SpeedSpec::Varying(Curve::new(vec![Key::new(0, 0.8), Key::new(2 * SECOND, 1.6)], 1.0));
        c2.effects = vec![EffectSpec::Reverb(ReverbParams::HALL), EffectSpec::Delay(DelayParams::default())];
        let mut c3 = ClipSpec::new(3, 3, SECOND / 2, SECOND, Box::new(MemorySource::mono(src44, 44_100)));
        c3.reversed = true;
        c3.source_in = SECOND;
        c3.speed = SpeedSpec::Constant(1.5);
        c3.preserve_pitch = false;
        c3.effects = vec![EffectSpec::Gate(GateParams::default())];
        p.clips.extend([c1, c2, c3]);
        let mut m = Mixer::new(RATE, 512, Quality::High, p);
        let mut buf = StereoBuf::new(512);
        let before = crate::alloc_count::count();
        m.seek(SECOND / 7); // a seek primes every clip's chain
        for _ in 0..(3 * RATE as usize / 512) {
            m.render(&mut buf);
            assert!(buf.left().iter().all(|v| v.is_finite()));
        }
        m.seek(2 * SECOND + 123);
        m.render(&mut buf);
        let _ = m.meters().master_peak;
        assert_eq!(crate::alloc_count::count() - before, 0, "render path allocated");
    }

    #[test]
    fn effect_specs_build_and_identify() {
        let specs = [
            EffectSpec::flat_eq(),
            EffectSpec::Compressor(CompressorParams::VOICE),
            EffectSpec::Limiter(LimiterParams::default()),
            EffectSpec::Gate(GateParams::default()),
            EffectSpec::DeEsser(DeEsserParams::default()),
            EffectSpec::Reverb(ReverbParams::HALL),
            EffectSpec::Delay(DelayParams::default()),
            EffectSpec::PitchShift { semitones: 3.0 },
            EffectSpec::Voice { style: VoiceStyle::Newsroom, amount: 0.5 },
        ];
        let mut ids = std::collections::HashSet::new();
        for s in &specs {
            assert!(ids.insert(s.kind_id()));
            let mut e = s.build(RATE, 256);
            let mut l = vec![0.1; 256];
            let mut r = vec![0.1; 256];
            e.process(&mut l, &mut r);
            assert!(l.iter().chain(&r).all(|v| v.is_finite()));
        }
    }

    fn tone_clip(id: u64, lane: u64, amp: f32) -> ClipSpec {
        let src = vec![amp; RATE as usize];
        ClipSpec::new(id, lane, 0, SECOND, Box::new(MemorySource::mono(src, RATE)))
    }

    #[test]
    fn lanes_route_to_buses_by_role_and_buses_have_gain_mute_and_solo() {
        let lanes = vec![LaneSpec::new(1, LaneRole::Dialogue), LaneSpec::new(2, LaneRole::Music)];
        let mut dx = BusSpec::new(10, BusKind::Dialogue, "DX");
        dx.volume_db = Curve::constant(-6.0);
        let mx = BusSpec::new(11, BusKind::Music, "MX");
        let plan = |dx: BusSpec, mx: BusSpec| MixPlan { clips: vec![tone_clip(1, 1, 0.2), tone_clip(2, 2, 0.1)], lanes: lanes.clone(), buses: vec![dx, mx], master: no_limiter(), ..Default::default() };
        let (l, _) = render(plan(dx.clone(), mx.clone()), 2000, 256);
        // Mono clips at centre play at full level on both sides.
        assert!((l[1000] - (0.2 * db_to_gain(-6.0) + 0.1)).abs() < 1e-4, "{}", l[1000]);
        let mut solo = mx.clone();
        solo.solo = true;
        let (l, _) = render(plan(dx.clone(), solo), 2000, 256);
        assert!((l[1000] - 0.1).abs() < 1e-4, "soloed music bus alone: {}", l[1000]);
        let mut muted = dx.clone();
        muted.muted = true;
        let mut m = Mixer::new(RATE, 256, Quality::High, plan(muted, mx.clone()));
        let mut b = StereoBuf::new(256);
        m.render(&mut b);
        assert_eq!(m.meters().bus_ids, vec![10, 11]);
        assert_eq!(m.meters().bus_peaks[0], 0.0);
        assert!(m.meters().bus_peaks[1] > 0.05);
    }

    #[test]
    fn sends_feed_an_aux_return_pre_or_post_fader() {
        let mut lane = LaneSpec::new(1, LaneRole::Other);
        lane.volume_db = Curve::constant(-60.0);
        lane.sends.push(SendSpec { bus: 20, level_db: Curve::constant(-6.0), pre_fader: true });
        let aux = BusSpec::new(20, BusKind::Aux, "Verb");
        let plan = |lane: LaneSpec| MixPlan { clips: vec![tone_clip(1, 1, 0.5)], lanes: vec![lane], buses: vec![aux.clone()], master: no_limiter(), ..Default::default() };
        let (l, _) = render(plan(lane.clone()), 2000, 256);
        // Pre-fader: the send ignores the −60 dB fader.
        let want = 0.5 * db_to_gain(-6.0) + 0.5 * db_to_gain(-60.0);
        assert!((l[1000] - want).abs() < 1e-4, "{} vs {want}", l[1000]);
        lane.sends[0].pre_fader = false;
        let (l, _) = render(plan(lane), 2000, 256);
        assert!(l[1000] < 0.001, "post-fader follows the fader: {}", l[1000]);
    }

    #[test]
    fn lane_pan_automation_moves_across_the_block() {
        let mut lane = LaneSpec::new(1, LaneRole::Other);
        lane.pan_curve = Some(Curve::new(vec![Key::new(0, -1.0), Key::new(SECOND / 2, 1.0)], 0.0));
        let plan = MixPlan { clips: vec![tone_clip(1, 1, 0.5)], lanes: vec![lane], master: no_limiter(), ..Default::default() };
        let (l, r) = render(plan, RATE as usize, 512);
        assert!(l[10] > 0.45 && r[10] < 0.01, "starts hard left {} {}", l[10], r[10]);
        assert!(r[RATE as usize / 2 + 10] > 0.45 && l[RATE as usize / 2 + 10] < 0.01, "ends hard right");
    }

    #[test]
    fn stems_add_up_to_the_unlimited_mix() {
        let lanes = vec![LaneSpec::new(1, LaneRole::Dialogue), LaneSpec::new(2, LaneRole::Music), LaneSpec::new(3, LaneRole::Other), LaneSpec::new(4, LaneRole::Ambience)];
        let mk = || MixPlan {
            clips: vec![tone_clip(1, 1, 0.1), tone_clip(2, 2, 0.05), tone_clip(3, 3, 0.03), tone_clip(4, 4, 0.02)],
            lanes: lanes.clone(),
            buses: vec![BusSpec::new(10, BusKind::Dialogue, "DX"), BusSpec::new(11, BusKind::Music, "MX"), BusSpec::new(12, BusKind::Effects, "FX"), BusSpec::new(13, BusKind::Ambience, "BG")],
            ..Default::default()
        };
        let mut full = mk();
        full.master.limiter = None;
        let (whole, _) = mix_offline(full, RATE, 0, SECOND / 2, Quality::High);
        let mut sum = vec![0.0f32; whole.len()];
        for kinds in [&[BusKind::Dialogue][..], &[BusKind::Music], &[BusKind::Effects, BusKind::Ambience]] {
            let mut p = mk();
            p.stem(kinds);
            assert!(p.master.limiter.is_none());
            let (s, _) = mix_offline(p, RATE, 0, SECOND / 2, Quality::High);
            for (a, b) in sum.iter_mut().zip(&s) {
                *a += b;
            }
        }
        for i in (0..whole.len()).step_by(997) {
            assert!((whole[i] - sum[i]).abs() < 1e-5, "frame {i}: {} vs {}", whole[i], sum[i]);
        }
        let mut me = mk();
        me.stem(&[BusKind::Music, BusKind::Effects, BusKind::Ambience]);
        let (m_and_e, _) = mix_offline(me, RATE, 0, SECOND / 2, Quality::High);
        assert!((m_and_e[1000] - (0.05 + 0.03 + 0.02)).abs() < 1e-4, "M&E has no dialogue: {}", m_and_e[1000]);
    }

    #[test]
    fn a_surround_mix_puts_dialogue_in_the_centre_and_music_in_left_and_right() {
        let mut dx = LaneSpec::new(1, LaneRole::Dialogue);
        dx.surround.shape.width = 0.0;
        let mut mx = LaneSpec::new(2, LaneRole::Music);
        mx.surround.shape.centre = false;
        mx.surround.shape.lfe_db = -6.0;
        let mut bg = LaneSpec::new(3, LaneRole::Ambience);
        bg.surround.y = Curve::constant(0.0);
        let plan = MixPlan {
            clips: vec![tone_clip(1, 1, 0.2), tone_clip(2, 2, 0.1), tone_clip(3, 3, 0.05)],
            lanes: vec![dx, mx, bg],
            master: no_limiter(),
            layout: Layout::Surround51,
            ..Default::default()
        };
        let solo = |i: usize| {
            let mut p = MixPlan { clips: Vec::new(), lanes: plan.lanes.clone(), master: no_limiter(), layout: Layout::Surround51, ..Default::default() };
            p.clips.push(tone_clip(i as u64 + 1, i as u64 + 1, [0.2, 0.1, 0.05][i]));
            mix_offline_layout(p, RATE, 0, SECOND / 4, Quality::High)
        };
        let d = solo(0);
        assert_eq!(d.len(), 6);
        assert!((d[2][4000] - 0.2 * std::f32::consts::SQRT_2).abs() < 1e-3 && d[0][4000].abs() < 1e-4 && d[4][4000].abs() < 1e-4, "dialogue in C: {:?}", d.iter().map(|c| c[4000]).collect::<Vec<_>>());
        let m = solo(1);
        assert!((m[0][4000] - 0.1).abs() < 1e-3 && (m[1][4000] - 0.1).abs() < 1e-3 && m[2][4000].abs() < 1e-4, "music L/R: {:?}", m.iter().map(|c| c[4000]).collect::<Vec<_>>());
        assert!(m[crate::surround::LFE][8000].abs() > 0.01, "music feeds the LFE");
        let b = solo(2);
        assert!(b[4][4000].abs() > 0.01 && b[5][4000].abs() > 0.01 && b[2][4000].abs() < 1e-4, "ambience wraps the room: {:?}", b.iter().map(|c| c[4000]).collect::<Vec<_>>());
        // The stereo fold-down of the same plan for monitoring.
        let (l, r) = mix_offline(MixPlan { clips: vec![tone_clip(1, 1, 0.2)], lanes: plan.lanes.clone(), master: no_limiter(), layout: Layout::Surround51, ..Default::default() }, RATE, 0, SECOND / 4, Quality::High);
        assert!((l[4000] - 0.2).abs() < 1e-3 && (r[4000] - 0.2).abs() < 1e-3, "fold-down keeps the level: {} {}", l[4000], r[4000]);
        // Loudness of a surround mix uses the layout's weights.
        let mut m = Mixer::new(RATE, 512, Quality::High, plan);
        let mut out = MultiBuf::new(6, 512);
        for _ in 0..200 {
            m.render_multi(&mut out);
        }
        assert!(m.meters().integrated_lufs.is_finite() && m.meters().channel_peaks.len() == 6);
    }

    #[test]
    fn a_bus_limiter_is_latency_compensated_against_the_other_buses() {
        let lanes = vec![LaneSpec::new(1, LaneRole::Dialogue), LaneSpec::new(2, LaneRole::Music)];
        let mut dx = BusSpec::new(10, BusKind::Dialogue, "DX");
        dx.effects.push(EffectSpec::Limiter(LimiterParams { ceiling_db: 0.0, ..Default::default() }));
        let mx = BusSpec::new(11, BusKind::Music, "MX");
        let mut c1 = ClipSpec::new(1, 1, SECOND / 10, SECOND / 10, Box::new(MemorySource::mono(vec![0.1; RATE as usize], RATE)));
        c1.fade_in = 0;
        let c2 = ClipSpec::new(2, 2, SECOND / 10, SECOND / 10, Box::new(MemorySource::mono(vec![0.1; RATE as usize], RATE)));
        let plan = MixPlan { clips: vec![c1, c2], lanes, buses: vec![dx, mx], master: no_limiter(), ..Default::default() };
        let (l, _) = mix_offline(plan, RATE, 0, SECOND / 2, Quality::High);
        let first = l.iter().position(|v| v.abs() > 1e-4).unwrap();
        let start = micros_to_frames(SECOND / 10, RATE) as usize;
        assert!(first.abs_diff(start) <= 1, "first sound at {first}, clip at {start}");
        assert!((l[start + 2000] - 0.2).abs() < 1e-3, "both buses arrive together: {}", l[start + 2000]);
    }
}


