// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Instruments, made by synthesis: plucked strings by physical modelling
//! (a tuned delay loop; electric guitars through a pickup, an amp and, for
//! the distorted one, two clipping stages and a speaker cabinet), a
//! struck-string additive piano, detuned saw stacks, chip squares, FM electric piano,
//! drawbar organ, source-and-filter winds, brass and bowed strings, analogue
//! style synths, modal bells and mallets, and a drum kit. Every note is
//! rendered mono and peak-normalised, then scaled by its velocity, so the
//! arrangement controls the balance.

use crate::dsp::*;
use std::f32::consts::TAU;

/// The pitched instruments.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Inst {
    /// Grand piano.
    Piano,
    /// Tine electric piano.
    ElectricPiano,
    /// Drawbar organ.
    Organ,
    /// Steel-string acoustic guitar.
    Guitar,
    /// Nylon-string guitar.
    NylonGuitar,
    /// Harp.
    Harp,
    /// Double bass, plucked.
    UprightBass,
    /// Electric bass guitar.
    ElectricBass,
    /// Synth bass.
    SynthBass,
    /// Tenor saxophone.
    Sax,
    /// Trumpet.
    Trumpet,
    /// Trombone.
    Trombone,
    /// French horn.
    Horn,
    /// Flute.
    Flute,
    /// Clarinet.
    Clarinet,
    /// Violin (also the fiddle).
    Violin,
    /// Cello.
    Cello,
    /// String section.
    Strings,
    /// Warm synth pad.
    Pad,
    /// Vowel choir.
    Choir,
    /// Lead synth.
    SynthLead,
    /// Short plucked synth, for arpeggios.
    SynthPluck,
    /// Bell and glockenspiel.
    Bell,
    /// Marimba.
    Marimba,
    /// Vibraphone.
    Vibraphone,
    /// Electric guitar, clean through an amp.
    ElectricGuitar,
    /// Electric guitar through overdrive into a speaker cabinet.
    DistortedGuitar,
    /// A stack of detuned saws (trance, synthwave, big-room leads).
    SuperSaw,
    /// An 8-bit square-wave voice.
    Chip,
}

impl Inst {
    /// Every instrument.
    pub const ALL: [Inst; 29] = [
        Inst::Piano, Inst::ElectricPiano, Inst::Organ, Inst::Guitar, Inst::NylonGuitar, Inst::Harp, Inst::UprightBass, Inst::ElectricBass, Inst::SynthBass, Inst::Sax, Inst::Trumpet, Inst::Trombone, Inst::Horn, Inst::Flute, Inst::Clarinet, Inst::Violin, Inst::Cello, Inst::Strings, Inst::Pad, Inst::Choir, Inst::SynthLead, Inst::SynthPluck, Inst::Bell, Inst::Marimba, Inst::Vibraphone, Inst::ElectricGuitar, Inst::DistortedGuitar, Inst::SuperSaw, Inst::Chip,
    ];

    /// The name the AI and the replies use.
    pub fn name(self) -> &'static str {
        match self {
            Inst::Piano => "piano",
            Inst::ElectricPiano => "electric piano",
            Inst::Organ => "organ",
            Inst::Guitar => "acoustic guitar",
            Inst::NylonGuitar => "nylon guitar",
            Inst::Harp => "harp",
            Inst::UprightBass => "double bass",
            Inst::ElectricBass => "electric bass",
            Inst::SynthBass => "synth bass",
            Inst::Sax => "saxophone",
            Inst::Trumpet => "trumpet",
            Inst::Trombone => "trombone",
            Inst::Horn => "french horn",
            Inst::Flute => "flute",
            Inst::Clarinet => "clarinet",
            Inst::Violin => "violin",
            Inst::Cello => "cello",
            Inst::Strings => "strings",
            Inst::Pad => "pad",
            Inst::Choir => "choir",
            Inst::SynthLead => "synth lead",
            Inst::SynthPluck => "synth pluck",
            Inst::Bell => "bells",
            Inst::Marimba => "marimba",
            Inst::Vibraphone => "vibraphone",
            Inst::ElectricGuitar => "electric guitar",
            Inst::DistortedGuitar => "distorted electric guitar",
            Inst::SuperSaw => "supersaw",
            Inst::Chip => "chip square",
        }
    }

    /// An instrument from words ("sax", "double bass", "fiddle"…).
    pub fn parse(text: &str) -> Option<Inst> {
        let t = format!(" {} ", text.to_lowercase().replace(['-', ',', '.'], " "));
        // A word or its plural ("guitars", "synths").
        let has = |w: &str| t.contains(&format!(" {w} ")) || t.contains(&format!(" {w}s "));
        let guitarish = has("guitar") || has("riff") || has("lead guitar") || has("rhythm guitar") || has("axe");
        let dirty = ["distorted", "distortion", "overdrive", "overdriven", "fuzz", "fuzzy", "crunchy", "crunch", "heavy", "gritty", "dirty", "shred", "shredding", "metal", "chugging", "chug", "high gain"].iter().any(|w| has(w));
        Some(if (guitarish && dirty) || has("power chord") || has("palm muted") || has("palm mute") || has("shred") {
            Inst::DistortedGuitar
        } else if has("electric guitar") || has("clean guitar") || has("strat") || has("stratocaster") || has("telecaster") || has("les paul") || has("jangly guitar") || has("surf guitar") || (guitarish && ["electric", "clean", "jangly", "twangy", "surf", "funky"].iter().any(|w| has(w))) {
            Inst::ElectricGuitar
        } else if has("supersaw") || has("super saw") || has("saw synth") || has("saw lead") || has("detuned saw") || has("trance synth") {
            Inst::SuperSaw
        } else if has("8 bit") || has("chiptune") || has("chip tune") || has("square wave") || has("nes") || has("gameboy") || has("game boy") || has("retro game") {
            Inst::Chip
        } else if has("sax") || has("saxophone") || has("sax solo") {
            Inst::Sax
        } else if has("double bass") || has("upright bass") || has("upright") || has("contrabass") || has("stand up bass") {
            Inst::UprightBass
        } else if has("electric bass") || has("bass guitar") {
            Inst::ElectricBass
        } else if has("synth bass") || has("sub bass") || has("808") {
            Inst::SynthBass
        } else if has("electric piano") || has("rhodes") || has("keys") || has("wurlitzer") {
            Inst::ElectricPiano
        } else if has("piano") {
            Inst::Piano
        } else if has("organ") {
            Inst::Organ
        } else if has("nylon") || has("classical guitar") || has("spanish guitar") {
            Inst::NylonGuitar
        } else if has("guitar") || has("acoustic") || has("banjo") || has("mandolin") || has("ukulele") {
            Inst::Guitar
        } else if has("harp") {
            Inst::Harp
        } else if has("trumpet") || has("cornet") || has("brass") {
            Inst::Trumpet
        } else if has("trombone") || has("tuba") {
            Inst::Trombone
        } else if has("horn") || has("horns") || has("french horn") {
            Inst::Horn
        } else if has("flute") || has("piccolo") || has("recorder") || has("whistle") {
            Inst::Flute
        } else if has("clarinet") || has("oboe") || has("woodwind") || has("woodwinds") {
            Inst::Clarinet
        } else if has("violin") || has("fiddle") {
            Inst::Violin
        } else if has("cello") || has("viola") {
            Inst::Cello
        } else if has("strings") || has("orchestra") || has("orchestral") || has("string section") {
            Inst::Strings
        } else if has("choir") || has("voices") || has("vocals") || has("ooh") || has("aah") {
            Inst::Choir
        } else if has("pad") || has("pads") || has("drone") || has("ambient pad") {
            Inst::Pad
        } else if has("arp") || has("arpeggio") || has("pluck") {
            Inst::SynthPluck
        } else if has("lead") || has("synth") || has("synths") || has("synthesizer") || has("synthesiser") {
            Inst::SynthLead
        } else if has("bell") || has("bells") || has("glockenspiel") || has("chimes") || has("celesta") || has("music box") {
            Inst::Bell
        } else if has("marimba") || has("xylophone") {
            Inst::Marimba
        } else if has("vibraphone") || has("vibes") {
            Inst::Vibraphone
        } else {
            return None;
        })
    }

    /// The comfortable range as MIDI notes.
    pub fn range(self) -> (i32, i32) {
        match self {
            Inst::Piano | Inst::ElectricPiano | Inst::Organ | Inst::Harp => (36, 96),
            Inst::Guitar | Inst::NylonGuitar => (40, 84),
            Inst::UprightBass | Inst::ElectricBass | Inst::SynthBass => (28, 55),
            Inst::Sax => (49, 81),
            Inst::Trumpet => (55, 82),
            Inst::Trombone => (40, 72),
            Inst::Horn => (41, 77),
            Inst::Flute => (60, 96),
            Inst::Clarinet => (50, 91),
            Inst::Violin => (55, 96),
            Inst::Cello => (36, 76),
            Inst::Strings | Inst::Pad | Inst::Choir => (36, 88),
            Inst::SynthLead | Inst::SynthPluck => (48, 96),
            Inst::Bell | Inst::Marimba | Inst::Vibraphone => (48, 96),
            Inst::ElectricGuitar | Inst::DistortedGuitar => (40, 88),
            Inst::SuperSaw => (36, 96),
            Inst::Chip => (48, 96),
        }
    }
}

/// The percussion.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Drum {
    /// Acoustic kick.
    Kick,
    /// Long electronic kick.
    Kick808,
    /// Snare.
    Snare,
    /// Brushed snare sweep.
    Brush,
    /// Side stick.
    Rim,
    /// Hand clap.
    Clap,
    /// Closed hi-hat.
    HatClosed,
    /// Open hi-hat.
    HatOpen,
    /// Hi-hat pedal chick.
    HatPedal,
    /// Ride cymbal.
    Ride,
    /// Crash cymbal.
    Crash,
    /// Low tom.
    TomLow,
    /// Mid tom.
    TomMid,
    /// High tom.
    TomHigh,
    /// Shaker.
    Shaker,
    /// Tambourine.
    Tambourine,
    /// Timpani (pitched by `pitch`).
    Timpani,
    /// Taiko, a deep drum with a long boom.
    Taiko,
}

fn finish(mut x: Vec<f32>, vel: f32) -> Vec<f32> {
    // Peak-normalise, then velocity: a soft note is quieter, not just darker.
    let p = peak(&x).max(1e-6);
    let g = (0.25 + 0.75 * vel.clamp(0.0, 1.0)) * 0.7 / p;
    x.iter_mut().for_each(|v| *v *= g);
    x
}

/// One note of `inst` at MIDI pitch `midi`, held `hold` seconds, with
/// velocity `vel` (0..1). The result includes the release.
pub fn render_note(inst: Inst, midi: f32, hold: f32, vel: f32, rng: &mut Rng) -> Vec<f32> {
    let hz = midi_hz(midi);
    let hold = hold.clamp(0.03, 40.0);
    let x = match inst {
        Inst::Piano => piano(hz, hold, vel, rng),
        Inst::ElectricPiano => electric_piano(hz, hold, vel),
        Inst::Organ => organ(hz, hold, rng),
        Inst::Guitar => string(hz, hold, vel, rng, &STEEL),
        Inst::NylonGuitar => string(hz, hold, vel, rng, &NYLON),
        Inst::Harp => string(hz, hold, vel, rng, &HARP),
        Inst::UprightBass => string(hz, hold, vel, rng, &UPRIGHT),
        Inst::ElectricBass => string(hz, hold, vel, rng, &EBASS),
        Inst::SynthBass => synth_bass(hz, hold, vel),
        Inst::Sax => sax(hz, hold, vel, rng),
        Inst::Trumpet => brass(hz, hold, vel, rng, 0),
        Inst::Trombone => brass(hz, hold, vel, rng, 1),
        Inst::Horn => brass(hz, hold, vel, rng, 2),
        Inst::Flute => flute(hz, hold, vel, rng),
        Inst::Clarinet => clarinet(hz, hold, vel, rng),
        Inst::Violin => bowed(hz, hold, rng, &VIOLIN),
        Inst::Cello => bowed(hz, hold, rng, &CELLO),
        Inst::Strings => bowed(hz, hold, rng, &SECTION),
        Inst::Pad => bowed(hz, hold, rng, &PAD),
        Inst::Choir => bowed(hz, hold, rng, &CHOIR),
        Inst::SynthLead => synth_lead(hz, hold, vel, rng),
        Inst::SynthPluck => synth_pluck(hz, hold, vel),
        Inst::Bell => modal(hz, hold, &[(1.0, 1.0, 2.2), (2.76, 0.6, 1.4), (5.4, 0.4, 0.8), (8.93, 0.25, 0.4)], 0.0, 0.0),
        Inst::Marimba => modal(hz, hold, &[(1.0, 1.0, 0.45), (3.93, 0.35, 0.09), (9.2, 0.12, 0.03)], 0.0, 0.07),
        Inst::Vibraphone => modal(hz, hold, &[(1.0, 1.0, 3.0), (4.0, 0.3, 1.0), (10.0, 0.1, 0.3)], 4.5, 0.25),
        Inst::ElectricGuitar => electric(hz, hold, vel, rng, false),
        Inst::DistortedGuitar => electric(hz, hold, vel, rng, true),
        Inst::SuperSaw => supersaw(hz, hold, vel, rng),
        Inst::Chip => chip(hz, hold, vel),
    };
    finish(x, vel)
}

// ----- plucked strings: a tuned delay loop ----------------------------------

struct StringSpec {
    bright: f32,
    t60: f32,
    pos: f32,
    /// Body resonances: (Hz, Q, gain).
    body: &'static [(f32, f32, f32)],
    /// Thump at the start: (Hz, seconds, gain).
    thump: (f32, f32, f32),
    damp: f32,
}

const STEEL: StringSpec = StringSpec { bright: 0.62, t60: 3.0, pos: 0.16, body: &[(98.0, 6.0, 0.5), (205.0, 7.0, 0.35), (410.0, 5.0, 0.2)], thump: (150.0, 0.012, 0.15), damp: 0.09 };
const NYLON: StringSpec = StringSpec { bright: 0.38, t60: 2.4, pos: 0.2, body: &[(110.0, 6.0, 0.6), (215.0, 6.0, 0.3)], thump: (140.0, 0.01, 0.12), damp: 0.1 };
const HARP: StringSpec = StringSpec { bright: 0.7, t60: 4.5, pos: 0.12, body: &[(180.0, 5.0, 0.3)], thump: (200.0, 0.006, 0.05), damp: 0.25 };
const UPRIGHT: StringSpec = StringSpec { bright: 0.22, t60: 1.6, pos: 0.17, body: &[(68.0, 5.0, 1.2), (112.0, 6.0, 0.7), (190.0, 5.0, 0.3)], thump: (95.0, 0.03, 0.45), damp: 0.07 };
const ELECTRIC: StringSpec = StringSpec { bright: 0.58, t60: 4.5, pos: 0.11, body: &[(1400.0, 1.6, 0.12)], thump: (120.0, 0.006, 0.03), damp: 0.07 };
const EBASS: StringSpec = StringSpec { bright: 0.42, t60: 2.8, pos: 0.26, body: &[(80.0, 4.0, 0.4)], thump: (100.0, 0.01, 0.12), damp: 0.06 };

fn string(hz: f32, hold: f32, vel: f32, rng: &mut Rng, s: &StringSpec) -> Vec<f32> {
    // Lower notes ring longer, higher notes die quicker.
    let t60 = s.t60 * (220.0 / hz).clamp(0.35, 1.8).powf(0.5);
    let len = frames((hold + s.damp * 7.0).min(t60 * 1.2).max(0.1));
    let period = (SR / hz) as usize;
    let ex = pluck_excite(rng, period, s.bright * (0.55 + 0.6 * vel), s.pos);
    let mut ks = Ks::new(hz, s.bright * (0.7 + 0.5 * vel), t60, &ex);
    let body: Vec<(Svf, SvfCoef, f32)> = s.body.iter().map(|(f, q, g)| (Svf::new(), SvfCoef::new(*f, *q), *g)).collect();
    let mut body = body;
    let mut out = vec![0.0; len];
    let mut thump = Svf::new();
    let tc = SvfCoef::new(s.thump.0, 1.2);
    for (i, o) in out.iter_mut().enumerate() {
        let t = i as f32 / SR;
        let x = ks.tick();
        let mut y = x;
        for (f, c, g) in body.iter_mut() {
            y += f.tick(c, x).bp * *g;
        }
        if t < s.thump.1 * 4.0 {
            y += thump.tick(&tc, rng.signed()).bp * s.thump.2 * (-t / s.thump.1).exp() * 3.0;
        }
        let rel = if t > hold { (-(t - hold) / s.damp).exp() } else { 1.0 };
        *o = y * rel;
    }
    out
}

/// An electric guitar: the string heard through a magnetic pickup (no
/// wood, a low cut), then a clean amp; or, `drive`, two clipping stages
/// (a soft overdrive, then a harder asymmetric one for the crunch), a mid
/// push and a speaker cabinet's band, which also sustains the note.
fn electric(hz: f32, hold: f32, vel: f32, rng: &mut Rng, drive: bool) -> Vec<f32> {
    let x = high_pass(&string(hz, hold, vel, rng, &ELECTRIC), 85.0, 0.7);
    if !drive {
        return low_pass(&x, 6000.0, 0.7);
    }
    let g = 10.0 + 16.0 * vel;
    let bias = 0.12f32;
    let y: Vec<f32> = x.iter().map(|v| {
        let a = (v * g).tanh();
        (a * 2.2 + bias).tanh() - bias.tanh()
    }).collect();
    let mid = band_pass(&y, 900.0, 0.9);
    let mut cab = low_pass(&high_pass(&y, 100.0, 0.7), 4300.0, 0.9);
    mix_into(&mut cab, &mid, 0, 0.35);
    cab
}

/// Seven saws a few cents apart through a filter that opens with the note.
fn supersaw(hz: f32, hold: f32, vel: f32, rng: &mut Rng) -> Vec<f32> {
    let rel = 0.25;
    let len = frames(hold + release_len(rel));
    let detune = [0.0f32, 7.0, -7.0, 15.0, -15.0, 24.0, -24.0];
    let mut saws: Vec<Saw> = detune.iter().map(|_| Saw::at(rng.unit())).collect();
    let mut f = Svf::new();
    let mut out = vec![0.0; len];
    for (i, o) in out.iter_mut().enumerate() {
        let t = i as f32 / SR;
        let env = asr(t, hold, 0.012, rel);
        let src: f32 = saws.iter_mut().zip(detune).map(|(s, c)| s.tick(hz * cents(c))).sum::<f32>() / 3.0;
        let cut = hz * 3.0 + (2500.0 + 5000.0 * vel) * (0.4 + 0.6 * (-t / 0.6).exp());
        *o = f.at(src, cut.min(16_000.0), 0.9).lp * env;
    }
    out
}

/// A square wave stepped to sixteen levels, as an 8-bit sound chip makes it.
fn chip(hz: f32, hold: f32, vel: f32) -> Vec<f32> {
    let rel = 0.05;
    let len = frames(hold + release_len(rel));
    let mut p = Pulse::at(0.0);
    let mut out = vec![0.0; len];
    for (i, o) in out.iter_mut().enumerate() {
        let t = i as f32 / SR;
        let env = asr(t, hold, 0.002, rel) * (0.75 + 0.25 * (-t / 0.2).exp());
        let x = p.tick(hz * cents(vibrato(t, 6.0, 15.0, 0.25)), if vel > 0.7 { 0.25 } else { 0.5 });
        *o = ((x * env * 8.0).round() / 8.0) * 0.8;
    }
    out
}

// ----- piano ------------------------------------------------------------------

fn piano(hz: f32, hold: f32, vel: f32, rng: &mut Rng) -> Vec<f32> {
    let nyq = SR * 0.45;
    let b = (2.0e-4 * (hz / 261.6).powf(1.5)).clamp(4e-5, 2.2e-3);
    let tau0 = (3.2 * (180.0 / hz).clamp(0.12, 2.6)).max(0.25);
    let damp = (0.35 * (220.0 / hz).clamp(0.12, 1.5)).max(0.05);
    let total = (hold + damp * 5.0).min(tau0 * 4.5).max(0.12);
    let len = frames(total);
    let mut parts = Vec::new();
    for n in 1..=18 {
        let f = n as f32 * hz * (1.0 + b * (n * n) as f32).sqrt();
        if f > nyq * 0.95 {
            break;
        }
        // The hammer strikes about an eighth along the string; it also
        // softens the highs when played gently.
        let strike = (n as f32 * std::f32::consts::PI / 8.0).sin().abs().max(0.15);
        let amp = strike / (n as f32).powf(0.85) * (-(n as f32) * (1.0 - vel) * 0.22).exp();
        let tau = tau0 / (1.0 + 0.55 * (n as f32 - 1.0));
        // Two strings per note, a hair apart: the slow beating of a real piano.
        let beat = 0.25 + rng.unit() * 0.5;
        parts.push((Rot::new(f, rng.unit() * TAU), Rot::new(f + beat * (1.0 + n as f32 * 0.15), rng.unit() * TAU), amp, tau));
    }
    let mut out = vec![0.0; len];
    let mut hammer = Svf::new();
    let hc = SvfCoef::new(1400.0 + 2400.0 * vel, 0.8);
    for (i, o) in out.iter_mut().enumerate() {
        let t = i as f32 / SR;
        let mut s = 0.0;
        for (a, b2, amp, tau) in parts.iter_mut() {
            let e = 0.72 * (-t / *tau).exp() + 0.28 * (-t / (*tau * 4.0)).exp();
            s += (a.tick() + b2.tick()) * 0.5 * *amp * e;
        }
        if t < 0.03 {
            s += hammer.tick(&hc, rng.signed()).lp * (-t / 0.006).exp() * 0.5 * vel;
        }
        let rel = if t > hold { (-(t - hold) / damp).exp() } else { 1.0 };
        *o = s * rel * smooth(t, 0.002);
    }
    out
}

fn electric_piano(hz: f32, hold: f32, vel: f32) -> Vec<f32> {
    let tau = (1.9 * (220.0 / hz).clamp(0.3, 2.0)).max(0.3);
    let damp = 0.18;
    let len = frames((hold + damp * 6.0).min(tau * 4.0).max(0.15));
    let (mut pm, mut pc, mut pt) = (0.0f32, 0.0f32, 0.0f32);
    let mut out = vec![0.0; len];
    for (i, o) in out.iter_mut().enumerate() {
        let t = i as f32 / SR;
        pm += TAU * hz / SR;
        pc += TAU * hz / SR;
        pt += TAU * hz * 14.0 / SR;
        let index = (0.5 + 2.4 * vel) * (-t / 0.45).exp() + 0.25;
        let tine = (pt).sin() * 0.5 * (-t / 0.03).exp() * vel;
        let y = (pc + index * pm.sin()).sin() + tine;
        let trem = 1.0 + 0.12 * (TAU * 4.6 * t).sin() * smooth(t - 0.2, 0.5);
        let rel = if t > hold { (-(t - hold) / damp).exp() } else { 1.0 };
        *o = y * (-t / tau).exp() * trem * rel * smooth(t, 0.002);
    }
    out
}

fn organ(hz: f32, hold: f32, rng: &mut Rng) -> Vec<f32> {
    let rel = 0.06;
    let len = frames(hold + release_len(rel));
    let bars = [(0.5, 0.8), (1.0, 1.0), (1.5, 0.3), (2.0, 0.7), (3.0, 0.25), (4.0, 0.4), (5.0, 0.1), (6.0, 0.1), (8.0, 0.15)];
    let mut oscs: Vec<(Rot, f32)> = bars.iter().filter(|(r, _)| hz * r < SR * 0.45).map(|(r, g)| (Rot::new(hz * r, rng.unit() * TAU), *g)).collect();
    let mut out = vec![0.0; len];
    for (i, o) in out.iter_mut().enumerate() {
        let t = i as f32 / SR;
        let mut s = 0.0;
        for (r, g) in oscs.iter_mut() {
            s += r.tick() * *g;
        }
        let click = if t < 0.004 { rng.signed() * 0.4 } else { 0.0 };
        let trem = 1.0 + 0.07 * (TAU * 6.2 * t).sin();
        *o = (s * 0.3 * trem + click) * asr(t, hold, 0.012, rel);
    }
    out
}

// ----- winds and brass --------------------------------------------------------

fn sax(hz: f32, hold: f32, vel: f32, rng: &mut Rng) -> Vec<f32> {
    let rel = 0.10;
    let len = frames(hold + release_len(rel));
    let (mut saw, mut sq) = (Saw::at(rng.unit()), Pulse::at(rng.unit()));
    let (mut f1, mut f2, mut fb, mut fl) = (Svf::new(), Svf::new(), Svf::new(), Svf::new());
    let (c1, c2, cb) = (SvfCoef::new(680.0, 3.0), SvfCoef::new(1450.0, 3.5), SvfCoef::new(2600.0, 0.9));
    let mut out = vec![0.0; len];
    for (i, o) in out.iter_mut().enumerate() {
        let t = i as f32 / SR;
        let env = asr(t, hold, 0.035, rel) * (1.0 + 0.06 * (TAU * 0.8 * t).sin());
        let c = -70.0 * (-t / 0.05).exp() + vibrato(t, 5.2, 26.0, 0.25);
        let f = hz * cents(c);
        let src = 0.7 * saw.tick(f) + 0.5 * sq.tick(f, 0.38);
        let body = fl.at(src, 2200.0 + 4000.0 * vel * env, 0.7).lp;
        let a = f1.tick(&c1, src).bp;
        let b = f2.tick(&c2, src).bp;
        let breath = fb.tick(&cb, rng.signed()).bp * (0.05 + 0.10 * (1.0 - smooth(t, 0.15)));
        *o = (0.55 * body + 0.5 * a + 0.35 * b + breath) * env;
    }
    out
}

fn brass(hz: f32, hold: f32, vel: f32, rng: &mut Rng, kind: u8) -> Vec<f32> {
    // (attack, release, formant Hz, brightness, scoop cents)
    let (attack, rel, form, bright, scoop) = match kind {
        0 => (0.04, 0.09, 1250.0, 9.0, 35.0),
        1 => (0.06, 0.12, 620.0, 6.0, 25.0),
        _ => (0.09, 0.16, 480.0, 3.5, 15.0),
    };
    let len = frames(hold + release_len(rel));
    let (mut saw, mut pul) = (Saw::at(rng.unit()), Pulse::at(rng.unit()));
    let (mut flt, mut fm, mut fn_) = (Svf::new(), Svf::new(), Svf::new());
    let cf = SvfCoef::new(form, 2.5);
    let cn = SvfCoef::new(3000.0, 0.8);
    let mut out = vec![0.0; len];
    for (i, o) in out.iter_mut().enumerate() {
        let t = i as f32 / SR;
        let env = asr(t, hold, attack, rel);
        let f = hz * cents(-scoop * (-t / 0.04).exp() + vibrato(t, 5.0, 14.0, 0.35));
        let src = 0.8 * saw.tick(f) + 0.3 * pul.tick(f, 0.28);
        let cut = (hz * (1.4 + bright * vel * env.powf(1.5))).clamp(300.0, 9000.0);
        let lp = flt.at(src, cut, 1.0).lp;
        let fo = fm.tick(&cf, lp).bp;
        let lip = fn_.tick(&cn, rng.signed()).bp * 0.04 * (-(t) / 0.06).exp();
        *o = (lp * 0.7 + fo * 0.5 + lip) * env;
    }
    out
}

fn flute(hz: f32, hold: f32, vel: f32, rng: &mut Rng) -> Vec<f32> {
    let rel = 0.09;
    let len = frames(hold + release_len(rel));
    let mut ph = 0.0f32;
    let mut fb = Svf::new();
    let cb = SvfCoef::new((hz * 3.0).clamp(1200.0, 7000.0), 1.4);
    let mut out = vec![0.0; len];
    for (i, o) in out.iter_mut().enumerate() {
        let t = i as f32 / SR;
        let env = asr(t, hold, 0.06, rel);
        let f = hz * cents(vibrato(t, 5.0, 14.0, 0.3));
        ph += TAU * f / SR;
        let tone = ph.sin() + 0.22 * (2.0 * ph).sin() * vel + 0.07 * (3.0 * ph).sin();
        let breath = fb.tick(&cb, rng.signed()).bp * (0.07 + 0.1 * (-t / 0.08).exp());
        *o = (tone * 0.6 + breath) * env;
    }
    out
}

fn clarinet(hz: f32, hold: f32, vel: f32, rng: &mut Rng) -> Vec<f32> {
    let rel = 0.07;
    let len = frames(hold + release_len(rel));
    let mut pul = Pulse::at(rng.unit());
    let (mut lp, mut fm, mut fb) = (Svf::new(), Svf::new(), Svf::new());
    let cf = SvfCoef::new(1500.0, 2.0);
    let cb = SvfCoef::new(3200.0, 1.0);
    let mut out = vec![0.0; len];
    for (i, o) in out.iter_mut().enumerate() {
        let t = i as f32 / SR;
        let env = asr(t, hold, 0.03, rel);
        let f = hz * cents(vibrato(t, 4.8, 9.0, 0.4));
        let src = pul.tick(f, 0.47);
        let body = lp.at(src, hz * (3.0 + 4.0 * vel * env), 0.8).lp;
        let f2 = fm.tick(&cf, src).bp;
        let breath = fb.tick(&cb, rng.signed()).bp * 0.03;
        *o = (body * 0.7 + f2 * 0.2 + breath) * env;
    }
    out
}

// ----- bowed strings, pads and voices ----------------------------------------

struct Bowed {
    voices: usize,
    spread: f32,
    attack: f32,
    rel: f32,
    vib: (f32, f32, f32),
    /// Body resonances: (Hz, Q, gain).
    body: &'static [(f32, f32, f32)],
    /// Low-pass at this many times the pitch (0: fixed `fixed_lp`).
    lp_mult: f32,
    fixed_lp: f32,
    noise: f32,
    sweep: f32,
}

const VIOLIN: Bowed = Bowed { voices: 3, spread: 4.0, attack: 0.07, rel: 0.12, vib: (5.6, 20.0, 0.2), body: &[(300.0, 4.0, 0.4), (800.0, 5.0, 0.5), (2800.0, 4.0, 0.35)], lp_mult: 9.0, fixed_lp: 0.0, noise: 0.03, sweep: 0.0 };
const CELLO: Bowed = Bowed { voices: 3, spread: 5.0, attack: 0.10, rel: 0.16, vib: (5.0, 17.0, 0.3), body: &[(105.0, 5.0, 0.6), (230.0, 5.0, 0.5), (520.0, 4.0, 0.35)], lp_mult: 7.0, fixed_lp: 0.0, noise: 0.035, sweep: 0.0 };
const SECTION: Bowed = Bowed { voices: 6, spread: 11.0, attack: 0.20, rel: 0.4, vib: (5.2, 12.0, 0.5), body: &[(350.0, 3.0, 0.4), (900.0, 3.0, 0.4), (2500.0, 3.0, 0.2)], lp_mult: 6.0, fixed_lp: 0.0, noise: 0.015, sweep: 0.0 };
const PAD: Bowed = Bowed { voices: 5, spread: 9.0, attack: 0.7, rel: 0.9, vib: (0.0, 0.0, 0.0), body: &[(500.0, 1.0, 0.2)], lp_mult: 0.0, fixed_lp: 1400.0, noise: 0.0, sweep: 0.5 };
const CHOIR: Bowed = Bowed { voices: 5, spread: 14.0, attack: 0.3, rel: 0.45, vib: (5.0, 24.0, 0.3), body: &[(780.0, 6.0, 0.9), (1150.0, 7.0, 0.6), (2900.0, 8.0, 0.25)], lp_mult: 0.0, fixed_lp: 3800.0, noise: 0.03, sweep: 0.0 };

fn bowed(hz: f32, hold: f32, rng: &mut Rng, s: &Bowed) -> Vec<f32> {
    let len = frames(hold + release_len(s.rel));
    let n = s.voices;
    let mut saws: Vec<(Saw, f32)> = (0..n)
        .map(|k| {
            let spread = if n == 1 { 0.0 } else { (k as f32 / (n - 1) as f32 * 2.0 - 1.0) * s.spread };
            (Saw::at(rng.unit()), spread)
        })
        .collect();
    let mut bodies: Vec<(Svf, SvfCoef, f32)> = s.body.iter().map(|(f, q, g)| (Svf::new(), SvfCoef::new(*f, *q), *g)).collect();
    let mut lp = Svf::new();
    let mut nz = Svf::new();
    let cn = SvfCoef::new(3500.0, 0.8);
    let drift = rng.unit() * TAU;
    let mut out = vec![0.0; len];
    for (i, o) in out.iter_mut().enumerate() {
        let t = i as f32 / SR;
        let env = asr(t, hold, s.attack, s.rel);
        let vib = vibrato(t, s.vib.0, s.vib.1, s.vib.2);
        let mut src = 0.0;
        for (k, (saw, sp)) in saws.iter_mut().enumerate() {
            let slow = 3.0 * (TAU * 0.3 * t + drift + k as f32).sin();
            src += saw.tick(hz * cents(*sp + vib + slow));
        }
        src /= (n as f32).sqrt();
        let cut = if s.lp_mult > 0.0 { hz * s.lp_mult } else { s.fixed_lp * (1.0 + s.sweep * 0.5 * (TAU * 0.13 * t).sin()) };
        let base = lp.at(src, cut, 0.7).lp;
        let mut y = base * 0.5;
        for (f, c, g) in bodies.iter_mut() {
            y += f.tick(c, src).bp * *g;
        }
        y += nz.tick(&cn, rng.signed()).bp * s.noise;
        *o = y * env;
    }
    out
}

// ----- synths ------------------------------------------------------------------

fn synth_lead(hz: f32, hold: f32, vel: f32, rng: &mut Rng) -> Vec<f32> {
    let rel = 0.12;
    let len = frames(hold + release_len(rel));
    let (mut a, mut b, mut p) = (Saw::at(rng.unit()), Saw::at(rng.unit()), Pulse::at(0.0));
    let mut f = Svf::new();
    let mut out = vec![0.0; len];
    for (i, o) in out.iter_mut().enumerate() {
        let t = i as f32 / SR;
        let env = asr(t, hold, 0.008, rel);
        let fe = (-t / 0.35).exp();
        let h = hz * cents(vibrato(t, 5.5, 12.0, 0.2));
        let src = 0.5 * a.tick(h * cents(6.0)) + 0.5 * b.tick(h * cents(-6.0)) + 0.35 * p.tick(h * 0.5, 0.4);
        let cut = hz * (2.0 + 6.0 * vel * fe) + 600.0;
        let y = f.at(src, cut, 2.2).lp;
        *o = y * env;
    }
    out
}

fn synth_bass(hz: f32, hold: f32, vel: f32) -> Vec<f32> {
    let rel = 0.08;
    let len = frames(hold + release_len(rel));
    let (mut s, mut sub) = (Saw::at(0.0), 0.0f32);
    let mut f = Svf::new();
    let mut out = vec![0.0; len];
    for (i, o) in out.iter_mut().enumerate() {
        let t = i as f32 / SR;
        let env = asr(t, hold, 0.004, rel);
        sub += TAU * hz / SR;
        let cut = 160.0 + (2400.0 * vel + 400.0) * (-t / 0.16).exp();
        let y = f.at(s.tick(hz), cut, 2.5).lp * 0.6 + sub.sin() * 0.7;
        *o = (y * 1.2).tanh() * env;
    }
    out
}

fn synth_pluck(hz: f32, hold: f32, vel: f32) -> Vec<f32> {
    let tau = (hold * 0.8).clamp(0.12, 0.5);
    let len = frames(tau * 5.0);
    let (mut a, mut b) = (Saw::at(0.1), Saw::at(0.6));
    let mut f = Svf::new();
    let mut out = vec![0.0; len];
    for (i, o) in out.iter_mut().enumerate() {
        let t = i as f32 / SR;
        let src = a.tick(hz * cents(5.0)) + b.tick(hz * cents(-5.0));
        let cut = 350.0 + (hz * 6.0 + 2500.0 * vel) * (-t / 0.1).exp();
        *o = f.at(src, cut, 1.6).lp * (-t / tau).exp() * smooth(t, 0.002);
    }
    out
}

/// Bells and mallets: a sum of decaying sines at fixed ratios.
fn modal(hz: f32, hold: f32, parts: &[(f32, f32, f32)], tremolo: f32, damp: f32) -> Vec<f32> {
    let scale = (440.0 / hz).clamp(0.4, 2.2).powf(0.4);
    let tau_max = parts.iter().map(|p| p.2).fold(0.0, f32::max) * scale;
    let total = if damp > 0.0 { (hold + damp * 6.0).min(tau_max * 5.0) } else { tau_max * 5.0 };
    let len = frames(total.max(0.1));
    let mut oscs: Vec<(Rot, f32, f32)> = parts.iter().filter(|(r, _, _)| hz * r < SR * 0.45).map(|(r, g, t)| (Rot::new(hz * r, 0.0), *g, t * scale)).collect();
    let mut out = vec![0.0; len];
    for (i, o) in out.iter_mut().enumerate() {
        let t = i as f32 / SR;
        let mut s = 0.0;
        for (r, g, tau) in oscs.iter_mut() {
            s += r.tick() * *g * (-t / *tau).exp();
        }
        let trem = if tremolo > 0.0 { 1.0 + 0.25 * (TAU * tremolo * t).sin() } else { 1.0 };
        let rel = if damp > 0.0 && t > hold { (-(t - hold) / damp).exp() } else { 1.0 };
        *o = s * trem * rel * smooth(t, 0.001);
    }
    out
}

// ----- drums -------------------------------------------------------------------

/// One drum hit with velocity `vel`. `pitch` (MIDI) sets timpani only.
pub fn render_drum(d: Drum, vel: f32, pitch: f32, rng: &mut Rng) -> Vec<f32> {
    let x = match d {
        Drum::Kick => kick(vel, 118.0, 46.0, 0.04, 0.17, 0.5),
        Drum::Kick808 => kick(vel, 100.0, 40.0, 0.03, 0.55, 0.9),
        Drum::Snare => snare(vel, rng),
        Drum::Brush => noise_hit(rng, 0.32, 5200.0, 0.8, 0.03, 0.12, 0.0),
        Drum::Rim => rim(rng),
        Drum::Clap => clap(rng),
        Drum::HatClosed => hat(rng, 0.04, 0.12),
        Drum::HatOpen => hat(rng, 0.26, 0.9),
        Drum::HatPedal => hat(rng, 0.02, 0.08),
        Drum::Ride => ride(rng),
        Drum::Crash => crash(rng),
        Drum::TomLow => tom(rng, 88.0, 0.3),
        Drum::TomMid => tom(rng, 128.0, 0.25),
        Drum::TomHigh => tom(rng, 178.0, 0.2),
        Drum::Shaker => noise_hit(rng, 0.09, 7200.0, 1.0, 0.012, 0.03, 0.0),
        Drum::Tambourine => tambourine(rng),
        Drum::Timpani => timpani(midi_hz(pitch.clamp(34.0, 60.0)), 1.6),
        Drum::Taiko => taiko(rng),
    };
    finish(x, vel)
}

fn kick(vel: f32, f0: f32, f1: f32, sweep: f32, tau: f32, drive: f32) -> Vec<f32> {
    let len = frames(tau * 6.0);
    let mut ph = 0.0f32;
    let mut out = vec![0.0; len];
    for (i, o) in out.iter_mut().enumerate() {
        let t = i as f32 / SR;
        let f = f1 + (f0 * (0.7 + 0.5 * vel) - f1) * (-t / sweep).exp();
        ph += TAU * f / SR;
        let click = if t < 0.004 { (t * 3000.0).sin() * (1.0 - t / 0.004) * 0.4 } else { 0.0 };
        *o = ((ph.sin() * (-t / tau).exp() + click) * (1.0 + 2.0 * drive)).tanh();
    }
    out
}

fn snare(vel: f32, rng: &mut Rng) -> Vec<f32> {
    let len = frames(0.45);
    let (mut f, mut g) = (Svf::new(), Svf::new());
    let (cn, ch) = (SvfCoef::new(2200.0, 0.6), SvfCoef::new(5500.0, 0.7));
    let (mut p1, mut p2) = (0.0f32, 0.0f32);
    let mut out = vec![0.0; len];
    for (i, o) in out.iter_mut().enumerate() {
        let t = i as f32 / SR;
        p1 += TAU * (185.0 + 60.0 * (-t / 0.012).exp()) / SR;
        p2 += TAU * 330.0 / SR;
        let n = rng.signed();
        let body = (p1.sin() * 0.8 + p2.sin() * 0.35) * (-t / 0.055).exp();
        let rattle = (f.tick(&cn, n).bp * 0.9 + g.tick(&ch, n).hp * 0.4) * (-t / (0.1 + 0.05 * vel)).exp();
        *o = body * 0.8 + rattle;
    }
    out
}

fn rim(rng: &mut Rng) -> Vec<f32> {
    let len = frames(0.12);
    let mut f = Svf::new();
    let c = SvfCoef::new(1700.0, 4.0);
    let mut ph = 0.0f32;
    let mut out = vec![0.0; len];
    for (i, o) in out.iter_mut().enumerate() {
        let t = i as f32 / SR;
        ph += TAU * 900.0 / SR;
        *o = (ph.sin() * 0.6 + f.tick(&c, rng.signed()).bp * 1.2) * (-t / 0.012).exp();
    }
    out
}

fn clap(rng: &mut Rng) -> Vec<f32> {
    let len = frames(0.35);
    let mut f = Svf::new();
    let c = SvfCoef::new(1250.0, 1.4);
    let mut out = vec![0.0; len];
    for (i, o) in out.iter_mut().enumerate() {
        let t = i as f32 / SR;
        let bursts = [0.0, 0.009, 0.019].iter().map(|b| if t >= *b { (-(t - b) / 0.004).exp() } else { 0.0 }).sum::<f32>();
        let env = bursts * 0.5 + (-(t - 0.02).max(0.0) / 0.09).exp() * if t > 0.02 { 0.5 } else { 0.0 };
        *o = f.tick(&c, rng.signed()).bp * env * 1.5;
    }
    out
}

fn noise_hit(rng: &mut Rng, total: f32, hz: f32, q: f32, attack: f32, tau: f32, _unused: f32) -> Vec<f32> {
    let len = frames(total);
    let mut f = Svf::new();
    let c = SvfCoef::new(hz, q);
    let mut out = vec![0.0; len];
    for (i, o) in out.iter_mut().enumerate() {
        let t = i as f32 / SR;
        let env = smooth(t, attack) * (-(t - attack).max(0.0) / tau).exp();
        *o = f.tick(&c, rng.signed()).bp * env;
    }
    out
}

fn hat(rng: &mut Rng, tau: f32, total: f32) -> Vec<f32> {
    let len = frames(total.max(tau * 5.0));
    let ratios = [2.0f32, 3.0, 4.16, 5.43, 6.79, 8.21];
    let mut pulses: Vec<Pulse> = ratios.iter().map(|_| Pulse::at(rng.unit())).collect();
    let (mut h1, mut h2) = (Svf::new(), Svf::new());
    let c = SvfCoef::new(7500.0, 0.7);
    let mut out = vec![0.0; len];
    for (i, o) in out.iter_mut().enumerate() {
        let t = i as f32 / SR;
        let mut m = 0.0;
        for (p, r) in pulses.iter_mut().zip(ratios) {
            m += p.tick(205.3 * r, 0.5);
        }
        let x = h1.tick(&c, m * 0.12 + rng.signed() * 0.5).hp;
        *o = h2.tick(&c, x).hp * (-t / tau).exp();
    }
    out
}

fn ride(rng: &mut Rng) -> Vec<f32> {
    let len = frames(1.6);
    let partials: [(f32, f32); 7] = [(3400.0, 1.0), (4570.0, 0.7), (5920.0, 0.6), (7210.0, 0.45), (8630.0, 0.3), (2700.0, 0.5), (1850.0, 0.4)];
    let mut oscs: Vec<(Rot, f32)> = partials.iter().map(|(f, g)| (Rot::new(*f, rng.unit() * TAU), *g)).collect();
    let mut f = Svf::new();
    let c = SvfCoef::new(6500.0, 0.7);
    let mut out = vec![0.0; len];
    for (i, o) in out.iter_mut().enumerate() {
        let t = i as f32 / SR;
        let mut s = 0.0;
        for (r, g) in oscs.iter_mut() {
            s += r.tick() * *g;
        }
        let ping = (-t / 0.03).exp();
        let wash = f.tick(&c, rng.signed()).hp * 0.5;
        *o = s * 0.25 * ((-t / 0.5).exp() * 0.6 + ping) + wash * (-t / 0.35).exp() * 0.6;
    }
    out
}

fn crash(rng: &mut Rng) -> Vec<f32> {
    let len = frames(3.2);
    let (mut f, mut g) = (Svf::new(), Svf::new());
    let c = SvfCoef::new(3200.0, 0.6);
    let c2 = SvfCoef::new(9000.0, 0.7);
    let mut out = vec![0.0; len];
    for (i, o) in out.iter_mut().enumerate() {
        let t = i as f32 / SR;
        let n = rng.signed();
        *o = (f.tick(&c, n).hp * 0.7 + g.tick(&c2, n).hp * 0.5) * (smooth(t, 0.004)) * ((-t / 0.35).exp() * 0.6 + (-t / 1.3).exp() * 0.4);
    }
    out
}

fn tom(rng: &mut Rng, hz: f32, tau: f32) -> Vec<f32> {
    let len = frames(tau * 6.0);
    let mut ph = 0.0f32;
    let mut f = Svf::new();
    let c = SvfCoef::new(2500.0, 1.0);
    let mut out = vec![0.0; len];
    for (i, o) in out.iter_mut().enumerate() {
        let t = i as f32 / SR;
        ph += TAU * hz * (1.0 + 0.5 * (-t / 0.05).exp()) / SR;
        *o = ph.sin() * (-t / tau).exp() + f.tick(&c, rng.signed()).bp * (-t / 0.01).exp() * 0.4;
    }
    out
}

fn tambourine(rng: &mut Rng) -> Vec<f32> {
    let len = frames(0.5);
    let mut oscs: Vec<Rot> = [5400.0f32, 6900.0, 8300.0, 9800.0].iter().map(|f| Rot::new(*f, rng.unit() * TAU)).collect();
    let mut f = Svf::new();
    let c = SvfCoef::new(8000.0, 0.8);
    let mut out = vec![0.0; len];
    for (i, o) in out.iter_mut().enumerate() {
        let t = i as f32 / SR;
        let s: f32 = oscs.iter_mut().map(|r| r.tick()).sum::<f32>() * 0.2;
        *o = (s + f.tick(&c, rng.signed()).hp * 0.6) * (-t / 0.11).exp() * smooth(t, 0.003);
    }
    out
}

fn timpani(hz: f32, tau: f32) -> Vec<f32> {
    let len = frames(tau * 4.0);
    let modes = [(1.0f32, 1.0f32, 1.0f32), (1.5, 0.6, 0.8), (1.98, 0.45, 0.6), (2.44, 0.3, 0.45), (2.89, 0.2, 0.35)];
    let mut oscs: Vec<(Rot, f32, f32)> = modes.iter().map(|(r, g, t)| (Rot::new(hz * r * 0.98, 0.0), *g, tau * t)).collect();
    let mut out = vec![0.0; len];
    for (i, o) in out.iter_mut().enumerate() {
        let t = i as f32 / SR;
        let mut s = 0.0;
        for (r, g, ta) in oscs.iter_mut() {
            s += r.tick() * *g * (-t / *ta).exp();
        }
        *o = s * smooth(t, 0.003);
    }
    out
}

fn taiko(rng: &mut Rng) -> Vec<f32> {
    let mut x = tom(rng, 62.0, 0.55);
    let boom = timpani(48.0, 1.4);
    for (i, v) in boom.iter().enumerate() {
        if i < x.len() {
            x[i] += v * 0.7;
        }
    }
    x
}

/// A low impact: a sine falling from `from` to `to` Hz with a noise puff,
/// `tau` seconds to decay. Used under hits in a score.
pub fn boom(from: f32, to: f32, tau: f32, rng: &mut Rng) -> Vec<f32> {
    let len = frames(tau * 5.0);
    let mut ph = 0.0f32;
    let mut f = Svf::new();
    let mut out = vec![0.0; len];
    for (i, o) in out.iter_mut().enumerate() {
        let t = i as f32 / SR;
        let hz = to + (from - to) * (-t / (tau * 0.25)).exp();
        ph += TAU * hz / SR;
        let puff = f.at(rng.signed(), 700.0, 0.8).lp * (-t / 0.06).exp() * 0.6;
        *o = (ph.sin() * (-t / tau).exp() * 1.4 + puff).tanh();
    }
    out
}

/// A synth riser: filtered noise and a rising tone that climb and grow
/// for `seconds`, peaking at the very end.
pub fn riser(seconds: f32, rng: &mut Rng) -> Vec<f32> {
    let len = frames(seconds);
    let (mut f, mut g) = (Svf::new(), Svf::new());
    let mut ph = 0.0f32;
    let mut out = vec![0.0; len];
    for (i, o) in out.iter_mut().enumerate() {
        let p = i as f32 / len.max(1) as f32;
        let hz = 250.0 * 40f32.powf(p);
        let amp = p.powf(2.2);
        ph += TAU * 120.0 * 10f32.powf(p) / SR;
        let n = rng.signed();
        let air = f.at(n, hz, 0.7).bp * 1.3 + g.at(n, hz * 1.5, 1.2).bp * 0.5;
        *o = (air + ph.sin() * 0.3 + (2.0 * ph).sin() * 0.12) * amp;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_sound::pitch::dominant_frequency;

    #[test]
    fn every_instrument_sounds_at_its_pitch() {
        for inst in Inst::ALL {
            let (lo, hi) = inst.range();
            for midi in [lo + 4, (lo + hi) / 2] {
                let mut rng = Rng::new(9);
                let x = render_note(inst, midi as f32, 1.4, 0.8, &mut rng);
                assert!(x.iter().all(|v| v.is_finite()), "{inst:?}");
                assert!(peak(&x) > 0.05, "{inst:?} {midi} is silent");
                let hz = midi_hz(midi as f32);
                if inst == Inst::Bell {
                    continue; // inharmonic by design: no single period
                }
                // Mallets have partial-rich spectra: allow the octave.
                let seg = &x[frames(0.25)..(frames(0.25) + frames(0.35)).min(x.len())];
                let got = dominant_frequency(seg, RATE, hz * 0.6, hz * 1.7).unwrap_or(0.0);
                let ratio = got / hz;
                assert!((ratio - 1.0).abs() < 0.02 || (ratio - 2.0).abs() < 0.04 || (ratio - 0.5).abs() < 0.01, "{inst:?} {midi}: wanted {hz:.1} Hz, measured {got:.1}");
            }
        }
    }

    #[test]
    fn strings_and_keys_are_tuned_exactly() {
        for inst in [Inst::Guitar, Inst::UprightBass, Inst::Piano, Inst::Harp, Inst::Violin, Inst::Flute] {
            let (lo, hi) = inst.range();
            for midi in [lo + 5, (lo + hi) / 2 + 3] {
                let mut rng = Rng::new(2);
                let x = render_note(inst, midi as f32, 1.5, 0.7, &mut rng);
                let hz = midi_hz(midi as f32);
                let seg = &x[frames(0.3)..frames(0.8)];
                let got = dominant_frequency(seg, RATE, hz * 0.8, hz * 1.25).unwrap();
                assert!((got / hz - 1.0).abs() < 0.012, "{inst:?} {midi}: {hz:.1} vs {got:.1}");
            }
        }
    }

    #[test]
    fn velocity_is_loudness_and_notes_end() {
        let mut rng = Rng::new(1);
        let soft = render_note(Inst::Piano, 60.0, 1.0, 0.2, &mut rng);
        let hard = render_note(Inst::Piano, 60.0, 1.0, 1.0, &mut rng);
        assert!(peak(&hard) > peak(&soft) * 1.5);
        let tail = &hard[hard.len() - 200..];
        assert!(peak(tail) < 0.02, "the release must die away: {}", peak(tail));
    }

    #[test]
    fn drums_are_distinct_and_audible() {
        let kinds = [Drum::Kick, Drum::Kick808, Drum::Snare, Drum::Brush, Drum::Rim, Drum::Clap, Drum::HatClosed, Drum::HatOpen, Drum::HatPedal, Drum::Ride, Drum::Crash, Drum::TomLow, Drum::TomMid, Drum::TomHigh, Drum::Shaker, Drum::Tambourine, Drum::Timpani, Drum::Taiko];
        let mut lens = Vec::new();
        for d in kinds {
            let mut rng = Rng::new(4);
            let x = render_drum(d, 0.9, 45.0, &mut rng);
            assert!(peak(&x) > 0.1 && x.iter().all(|v| v.is_finite()), "{d:?}");
            lens.push(rms(&x));
        }
        // A kick is low, a hat is high: compare their centres.
        let mut rng = Rng::new(4);
        let k = render_drum(Drum::Kick, 1.0, 0.0, &mut rng);
        let h = render_drum(Drum::HatClosed, 1.0, 0.0, &mut rng);
        let zc = |x: &[f32]| x.windows(2).filter(|w| (w[0] < 0.0) != (w[1] < 0.0)).count() as f32 / x.len() as f32;
        assert!(zc(&h) > zc(&k) * 10.0);
    }
}
