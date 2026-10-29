// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `sfx`: the sounds edits are made of, made to measure. A riser that
//! peaks exactly on the drop, a whoosh as long as the transition, a boom
//! under the cut to black, a hit on the beat: filtered noise and swept
//! tones, written as a WAV beside the project and placed in one op. Foley,
//! animals and anything described in words come from `studio-synth`'s
//! physical models (see `local_sound`).

use super::{Session, f, micros, secs};
use serde_json::{Value, json};
use std::f32::consts::PI;
use studio_model as m;

pub(super) const RATE: u32 = 48_000;

pub(super) struct Noise(pub(super) u64);

impl Noise {
    pub(super) fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 40) as f32 / (1u64 << 23) as f32 * 2.0 - 1.0
    }
}

/// A state-variable band-pass whose centre can move every sample.
pub(super) struct Band {
    low: f32,
    band: f32,
}

impl Band {
    pub(super) fn new() -> Self {
        Self { low: 0.0, band: 0.0 }
    }
    pub(super) fn run(&mut self, x: f32, hz: f32, q: f32) -> (f32, f32) {
        let fc = (2.0 * (PI * hz.min(RATE as f32 * 0.24) / RATE as f32).sin()).min(1.0);
        let high = x - self.low - q * self.band;
        self.band += fc * high;
        self.low += fc * self.band;
        (self.band, self.low)
    }
}

/// The beds a place sounds like, made here (no file or key needed).
pub const AMBIENCES: &str = "stream, waterfall, rain, wind, birds, crickets, fire, waves, forest (wind, leaves and birds), night (crickets and a low wind)";

pub fn is_ambience(kind: &str) -> bool {
    matches!(kind, "stream" | "brook" | "creek" | "river" | "waterfall" | "rain" | "wind" | "birds" | "crickets" | "fire" | "campfire" | "waves" | "sea" | "forest" | "night")
}

impl Noise {
    /// Uniform in 0..1.
    pub(super) fn unit(&mut self) -> f32 {
        self.next();
        (self.0 >> 40) as f32 / (1u64 << 24) as f32
    }
}

/// A short tone event: start sample, length, start and end pitch, level
/// and pan (0 left … 1 right).
struct Note {
    at: usize,
    len: usize,
    from: f32,
    to: f32,
    gain: f32,
    pan: f32,
}

/// Bird phrases: a few chirps swept up or down, a phrase every second or
/// three, from different places in the stereo field.
fn birds(n: usize, rng: &mut Noise) -> Vec<Note> {
    let mut out = Vec::new();
    let mut t = (rng.unit() * 0.8 * RATE as f32) as usize;
    while t < n {
        let pan = rng.unit();
        let base = 2600.0 + rng.unit() * 3000.0;
        let notes = 2 + (rng.unit() * 5.0) as usize;
        let gain = 0.25 + rng.unit() * 0.6;
        let mut at = t;
        for _ in 0..notes {
            let len = ((0.04 + rng.unit() * 0.11) * RATE as f32) as usize;
            let up = rng.unit() > 0.4;
            let span = base * (0.15 + rng.unit() * 0.5);
            out.push(Note { at, len, from: if up { base } else { base + span }, to: if up { base + span } else { base }, gain, pan });
            at += len + ((0.02 + rng.unit() * 0.08) * RATE as f32) as usize;
        }
        t = at + ((0.6 + rng.unit() * 2.6) * RATE as f32) as usize;
    }
    out
}

/// Cricket trains: a 4–5 kHz tone pulsed in groups, each cricket at its
/// own rate and place.
fn crickets(n: usize, rng: &mut Noise) -> Vec<Note> {
    let mut out = Vec::new();
    for _ in 0..3 {
        let hz = 4200.0 + rng.unit() * 900.0;
        let pan = rng.unit();
        let gain = 0.2 + rng.unit() * 0.35;
        let every = 0.35 + rng.unit() * 0.5;
        let mut t = rng.unit() * every;
        while ((t * RATE as f32) as usize) < n {
            for k in 0..3 {
                let at = ((t + k as f32 * 0.03) * RATE as f32) as usize;
                out.push(Note { at, len: (0.018 * RATE as f32) as usize, from: hz, to: hz * 0.995, gain, pan });
            }
            t += every * (0.9 + rng.unit() * 0.2);
        }
    }
    out
}

/// Water bubbles: tiny rising sine blips, dozens a second.
fn bubbles(n: usize, rng: &mut Noise, per_sec: f32) -> Vec<Note> {
    let mut out = Vec::new();
    let mut t = 0.0f32;
    while ((t * RATE as f32) as usize) < n {
        let hz = 500.0 + rng.unit() * 1400.0;
        out.push(Note { at: (t * RATE as f32) as usize, len: ((0.01 + rng.unit() * 0.03) * RATE as f32) as usize, from: hz, to: hz * (1.4 + rng.unit() * 0.6), gain: 0.15 + rng.unit() * 0.3, pan: rng.unit() });
        t += rng.unit() * 2.0 / per_sec;
    }
    out
}

fn add_notes(l: &mut [f32], r: &mut [f32], notes: &[Note], level: f32) {
    for nt in notes {
        let mut ph = 0f32;
        for k in 0..nt.len {
            let i = nt.at + k;
            if i >= l.len() {
                break;
            }
            let p = k as f32 / nt.len.max(1) as f32;
            ph += 2.0 * PI * (nt.from + (nt.to - nt.from) * p) / RATE as f32;
            let v = ph.sin() * (PI * p).sin().max(0.0).powf(1.5) * nt.gain * level;
            l[i] += v * (1.0 - nt.pan).sqrt();
            r[i] += v * nt.pan.sqrt();
        }
    }
}

/// A place's sound bed of `kind`, `dur` seconds, stereo, peaking near
/// −12 dBFS so it sits under voices and music.
pub fn ambience(kind: &str, dur: f32, seed: u64) -> Result<(Vec<f32>, Vec<f32>), String> {
    let n = (dur * RATE as f32) as usize;
    let mut rng = Noise(seed | 1);
    let (mut l, mut r) = (vec![0f32; n], vec![0f32; n]);
    let kind = match kind {
        "brook" | "creek" | "river" => "stream",
        "campfire" => "fire",
        "sea" => "waves",
        k => k,
    };
    // Noise beds: (centre Hz, q, level) per channel, with slow movement.
    let (mut a1, mut b1, mut a2, mut b2) = (Band::new(), Band::new(), Band::new(), Band::new());
    let wind = matches!(kind, "wind" | "forest" | "night");
    let gust = rng.unit() * 10.0;
    for i in 0..n {
        let t = i as f32 / RATE as f32;
        let (x, y) = (rng.next(), rng.next());
        let (mut vl, mut vr) = (0.0, 0.0);
        match kind {
            "stream" => {
                let m = 1.0 + 0.15 * (t * 0.7).sin();
                vl += a1.run(x, 1800.0 * m, 0.9).0 * 0.5 + a2.run(x, 5200.0, 1.2).0 * 0.15;
                vr += b1.run(y, 1650.0 * m, 0.9).0 * 0.5 + b2.run(y, 5000.0, 1.2).0 * 0.15;
            }
            "waterfall" => {
                vl += a1.run(x, 900.0, 0.6).0 * 0.8 + a2.run(x, 3500.0, 0.8).0 * 0.4;
                vr += b1.run(y, 950.0, 0.6).0 * 0.8 + b2.run(y, 3300.0, 0.8).0 * 0.4;
            }
            "rain" => {
                vl += a1.run(x, 6000.0, 1.0).0 * 0.25 + a2.run(x, 700.0, 0.7).1 * 0.15;
                vr += b1.run(y, 5800.0, 1.0).0 * 0.25 + b2.run(y, 650.0, 0.7).1 * 0.15;
                // Drops: rare sharp ticks.
                if rng.unit() < 0.0012 {
                    let v = rng.next() * 0.9;
                    if rng.unit() < 0.5 { vl += v } else { vr += v }
                }
            }
            "waves" => {
                let swell = (0.5 + 0.5 * (2.0 * PI * t / 7.5).sin()).powf(2.0);
                let swell2 = (0.5 + 0.5 * (2.0 * PI * t / 7.5 + 0.6).sin()).powf(2.0);
                vl += a1.run(x, 400.0 + 1600.0 * swell, 0.7).0 * (0.15 + swell);
                vr += b1.run(y, 400.0 + 1600.0 * swell2, 0.7).0 * (0.15 + swell2);
            }
            "fire" => {
                vl += a1.run(x, 180.0, 0.8).1 * 0.5;
                vr += b1.run(y, 190.0, 0.8).1 * 0.5;
                if rng.unit() < 0.0009 {
                    let v = rng.next() * 1.2;
                    vl += v * rng.unit();
                    vr += v * rng.unit();
                }
            }
            _ => {}
        }
        if wind {
            let g = 0.35 + 0.65 * (0.5 + 0.5 * (t * 0.23 + gust).sin() * (t * 0.071).cos()).powf(2.0);
            let hz = 250.0 + 450.0 * g;
            let level = if kind == "night" { 0.25 } else { 0.7 };
            vl += a2.run(x, hz, 0.35).0 * g * level;
            vr += b2.run(y, hz * 1.07, 0.35).0 * g * level;
            if kind == "forest" {
                // Leaves: a brighter rustle that follows the gusts.
                vl += a1.run(x, 4200.0, 1.4).0 * g * g * 0.2;
                vr += b1.run(y, 4400.0, 1.4).0 * g * g * 0.2;
            }
        }
        l[i] = vl;
        r[i] = vr;
    }
    match kind {
        "birds" | "forest" => add_notes(&mut l, &mut r, &birds(n, &mut rng), if kind == "birds" { 1.0 } else { 0.6 }),
        "crickets" | "night" => add_notes(&mut l, &mut r, &crickets(n, &mut rng), 0.5),
        "stream" => add_notes(&mut l, &mut r, &bubbles(n, &mut rng, 25.0), 0.35),
        "waterfall" | "rain" | "wind" | "waves" | "fire" => {}
        other => return Err(format!("ambience \"{other}\": {AMBIENCES}")),
    }
    let fade = (0.6 * RATE as f32) as usize;
    for i in 0..n {
        let e = (i.min(n - 1 - i) as f32 / fade as f32).min(1.0);
        l[i] *= e;
        r[i] *= e;
    }
    let peak = l.iter().chain(r.iter()).fold(0f32, |m, v| m.max(v.abs())).max(1e-6);
    let gain = 0.25 / peak; // about −12 dBFS
    l.iter_mut().chain(r.iter_mut()).for_each(|v| *v *= gain);
    Ok((l, r))
}

/// (left, right) samples for a sound of `kind` lasting `dur` seconds.
pub fn make(kind: &str, dur: f32, seed: u64) -> Result<(Vec<f32>, Vec<f32>), String> {
    let n = (dur * RATE as f32) as usize;
    let mut noise = Noise(seed | 1);
    let (mut l, mut r) = (vec![0f32; n], vec![0f32; n]);
    let (mut bl, mut br) = (Band::new(), Band::new());
    let mut phase = 0f32;
    for i in 0..n {
        let t = i as f32 / RATE as f32;
        let p = t / dur.max(1e-3);
        let (a, b) = (noise.next(), noise.next());
        let (x, y) = match kind {
            // Noise climbing from low to bright with a rising tone, getting
            // louder all the way to the end (place it so the end is the drop).
            "riser" => {
                let hz = 250.0 * 48f32.powf(p);
                let amp = p.powf(2.0);
                phase += 2.0 * PI * (110.0 * 8f32.powf(p)) / RATE as f32;
                let tone = phase.sin() * 0.25;
                (bl.run(a, hz, 0.5).0 * amp + tone * amp, br.run(b, hz * 1.03, 0.5).0 * amp + tone * amp)
            }
            // A band of air that swells and passes from left to right.
            "whoosh" => {
                let env = (PI * p).sin().powf(2.0);
                let hz = 500.0 + 5000.0 * (PI * p).sin().powf(1.5);
                let pan = p * PI / 2.0;
                let (sa, sb) = (bl.run(a, hz, 0.7).0, br.run(b, hz, 0.7).0);
                (sa * env * pan.cos() * 1.4, sb * env * pan.sin() * 1.4)
            }
            // A sub drop with a burst on top, ringing out.
            "boom" => {
                let hz = 35.0 + 60.0 * (-t / 0.35).exp();
                phase += 2.0 * PI * hz / RATE as f32;
                let sub = phase.sin() * (-t * 3.5 / dur.max(0.3)).exp();
                let burst = bl.run(a, 900.0, 1.2).1 * (-t * 20.0).exp() * 0.8;
                let v = (1.6 * (sub + burst)).tanh();
                (v, v)
            }
            // A short knock: a click of noise and a thump.
            "hit" => {
                let hz = 60.0 + 80.0 * (-t / 0.03).exp();
                phase += 2.0 * PI * hz / RATE as f32;
                let thump = phase.sin() * (-t * 18.0).exp();
                let click = bl.run(a, 2200.0, 1.0).0 * (-t * 45.0).exp() * 1.5;
                let v = (1.4 * (thump + click)).tanh();
                (v, v)
            }
            other => return Err(format!("sfx kind \"{other}\": riser, whoosh, boom or hit")),
        };
        // 5 ms fades so nothing clicks.
        let edge = (t / 0.005).min((dur - t) / 0.005).clamp(0.0, 1.0);
        l[i] = x * edge;
        r[i] = y * edge;
    }
    let peak = l.iter().chain(r.iter()).fold(0f32, |m, v| m.max(v.abs())).max(1e-6);
    let gain = 0.89 / peak; // −1 dBFS
    l.iter_mut().chain(r.iter_mut()).for_each(|v| *v *= gain);
    Ok((l, r))
}

impl Session {
    /// `sfx`: make a riser / whoosh / boom / hit of a given length and put
    /// it on a sound lane, at `at` or ending at `ends_at`.
    ///
    /// Beds (`stream`, `birds`, `forest`…) are the sound of a place: lay one
    /// under each location for as long as the scene stays there (up to
    /// 120 s a piece; place another to go on).
    pub(super) fn sfx(&mut self, op: &Value) -> Result<String, String> {
        if let Some(d) = op.get("describe").and_then(Value::as_str) {
            return self.described_sfx(d, op);
        }
        let kind = op.get("kind").and_then(Value::as_str).ok_or_else(|| format!("sfx needs \"kind\": riser, whoosh, boom or hit; a place's bed: {AMBIENCES}; or foley and effects such as footsteps, match, traffic, murmur, door, fire, an animal (settings: surface, count, distance, species…); or \"describe\": any sound in words. All of it is made here: {}", studio_synth::sfx::catalogue()))?;
        // Anything beyond the four classic sounds and the beds is made by the physical models.
        let classic = matches!(kind, "riser" | "whoosh" | "boom" | "hit") || is_ambience(kind);
        let settings = ["surface", "shoe", "count", "species", "animal", "call", "distance", "size", "speed", "material", "period", "era", "tail", "intensity"].iter().any(|k| op.get(*k).is_some());
        if !classic || settings {
            let words = super::local_sound::sfx_words(kind, op);
            return self.sfx_local(&words, op, "from the kind and settings");
        }
        let bed = is_ambience(kind);
        let default = match kind {
            "riser" => 2.4,
            "whoosh" => 0.8,
            "boom" => 2.0,
            _ if bed => 20.0,
            _ => 0.35,
        };
        let dur = f(op, "duration").unwrap_or(default).clamp(0.05, if bed { 120.0 } else { 30.0 }) as f32;
        let at = match (f(op, "at"), f(op, "ends_at")) {
            (Some(a), _) => a,
            (None, Some(e)) => e - dur as f64,
            _ => return Err("sfx needs \"at\" (where it starts) or \"ends_at\" (where it peaks; for a riser, the drop)".into()),
        }
        .max(0.0);
        // The nth sound of its kind and length in this project is a new variant,
        // not the same file again (`variant` says which, to repeat one on purpose).
        let variant = op.get("variant").and_then(Value::as_u64).unwrap_or_else(|| {
            let prefix = format!("{kind}-{dur:.2}s-");
            self.doc.project.media.iter().filter(|m| m.name.starts_with(&prefix)).count() as u64
        });
        let seed = (dur * 977.0) as u64 ^ kind.len() as u64 * 7919 ^ variant.wrapping_mul(0x9e37_79b9);
        let (l, r) = if bed { ambience(kind, dur, seed)? } else { make(kind, dur, seed)? };
        let samples: Vec<f32> = l.iter().zip(r.iter()).flat_map(|(a, b)| [*a, *b]).collect();
        let wav = studio_media::wav::Wav { sample_rate: RATE, channels: 2, samples };
        let dir = self.home.clone().map(|h| m::file::ProjectFolder::new(h).generated_dir(m::file::MadeKind::Sounds)).unwrap_or_else(|| studio_paths::cache().join("made-sfx"));
        let path = crate::files::write_once(&dir, &format!("{kind}-{dur:.2}s"), "wav", &studio_media::wav::encode_pcm16(&wav)).map_err(|e| format!("could not write the sound: {e}"))?;
        let report = crate::services::project::import_media(&mut self.doc, &[path.clone()]);
        let media = *report.ids.first().ok_or("the made sound could not be imported")?;
        let mut place = json!({"op": "place", "media": media, "at": at, "lane": op.get("lane").cloned().unwrap_or(json!("sound"))});
        if let Some(v) = f(op, "volume") {
            place["volume"] = json!(v);
        }
        let note = self.place(&place)?;
        Ok(format!("made a {:.2}s {kind} ({}): {note}", dur, if op.get("ends_at").is_some() { format!("peaks at {:.2}s", at + dur as f64) } else { format!("from {:.2}s", secs(micros(at))) }))
    }

    /// `sfx` with `describe`: any sound in words, made by the models here (or
    /// by a hosted service when one is set up), placed like the made ones.
    fn described_sfx(&mut self, describe: &str, op: &Value) -> Result<String, String> {
        // A hosted service makes open-ended sounds when one is set up; otherwise,
        // and when it cannot, the physical models here make it.
        if !super::voice_service::sound_service_ready() {
            return self.sfx_local(describe, op, "from the description");
        }
        let want = f(op, "duration");
        let mono = match super::voice_service::described_sound(describe, want) {
            Ok(m) => m,
            Err(_) => return self.sfx_local(describe, op, "the hosted sound service did not answer, so the models here made it"),
        };
        let dur = mono.len() as f64 / 44_100.0;
        let at = match (f(op, "at"), f(op, "ends_at")) {
            (Some(a), _) => a,
            (None, Some(e)) => e - dur,
            _ => return Err("sfx needs \"at\" (where it starts) or \"ends_at\" (where it ends)".into()),
        }
        .max(0.0);
        let wav = studio_media::wav::Wav { sample_rate: 44_100, channels: 2, samples: mono.iter().flat_map(|s| [*s, *s]).collect() };
        let dir = self.home.clone().map(|h| m::file::ProjectFolder::new(h).generated_dir(m::file::MadeKind::Sounds)).unwrap_or_else(|| studio_paths::cache().join("made-sfx"));
        let name: String = describe.chars().filter(|c| c.is_alphanumeric() || *c == ' ').take(32).collect::<String>().trim().replace(' ', "-");
        let path = crate::files::write_once(&dir, &name, "wav", &studio_media::wav::encode_pcm16(&wav)).map_err(|e| format!("could not write the sound: {e}"))?;
        let report = crate::services::project::import_media(&mut self.doc, &[path.clone()]);
        let media = *report.ids.first().ok_or("the sound could not be imported")?;
        let mut place = json!({"op": "place", "media": media, "at": at, "lane": op.get("lane").cloned().unwrap_or(json!("sound"))});
        if let Some(v) = f(op, "volume") {
            place["volume"] = json!(v);
        }
        let note = self.place(&place)?;
        Ok(format!("\"{describe}\" ({dur:.2}s, made by the hosted sound service) from {at:.2}s: {note}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_sound_of_the_same_kind_is_a_new_variant() {
        let mut s = Session::default();
        let ops: Vec<Value> = [(1.0, None), (4.0, None), (7.0, Some(0))].iter().map(|(at, v)| {
            let mut o = json!({"op": "sfx", "kind": "whoosh", "at": at, "duration": 0.6});
            if let Some(v) = v {
                o["variant"] = json!(v);
            }
            o
        }).collect();
        s.edit(&ops, "whooshes", false).unwrap();
        let paths: std::collections::BTreeSet<_> = s.doc.project.media.iter().filter_map(|m| m.path.clone()).collect();
        assert_eq!(paths.len(), 2, "auto picks a new variant, an explicit variant 0 repeats the first: {paths:?}");
    }

    #[test]
    fn every_bed_is_made_quiet_and_stereo() {
        for kind in ["stream", "waterfall", "rain", "wind", "birds", "crickets", "fire", "waves", "forest", "night"] {
            let (l, r) = ambience(kind, 3.0, 7).unwrap();
            assert_eq!(l.len(), 3 * RATE as usize);
            let peak = l.iter().chain(r.iter()).fold(0f32, |m, v| m.max(v.abs()));
            assert!((0.2..=0.26).contains(&peak), "{kind}: {peak}");
            let diff: f32 = l.iter().zip(&r).map(|(a, b)| (a - b).abs()).sum();
            assert!(diff > 1.0, "{kind} is mono");
        }
        assert!(ambience("traffic", 1.0, 1).is_err());
    }
}
