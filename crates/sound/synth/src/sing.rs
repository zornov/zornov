// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A singing voice by formant synthesis: a glottal pulse train at the sung
//! pitch (with portamento between notes and a vibrato that blooms after the
//! attack), run through four resonances that move from consonant to vowel,
//! with noise for fricatives and bursts. Notes carry syllables ("doo",
//! "bap", "la"); each syllable is read as a consonant, a vowel and a
//! consonant by spelling.
//!
//! It is a synthetic voice: clear in pitch and rhythm, buzzy and plainly
//! artificial in timbre, and it sings syllables, not words. Each voice is
//! its own mono stem, with the time of every syllable for lip sync.

use crate::dsp::*;
use crate::theory::parse_pitch;

/// A kind of voice: it sets the formant scale and the singer's range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VoiceKind {
    /// High female.
    Soprano,
    /// Low female.
    Alto,
    /// High male.
    Tenor,
    /// Low male.
    Bass,
}

impl VoiceKind {
    /// Reads "soprano", "alto", "tenor", "bass" and near words.
    pub fn parse(s: &str) -> Option<VoiceKind> {
        let t = s.trim().to_lowercase();
        Some(match t.as_str() {
            "soprano" | "female" | "woman" | "girl" | "high" | "treble" => VoiceKind::Soprano,
            "alto" | "contralto" | "mezzo" | "mezzo-soprano" => VoiceKind::Alto,
            "tenor" | "male" | "man" | "boy" | "baritone" => VoiceKind::Tenor,
            "bass" | "low" | "bass-baritone" | "basso" => VoiceKind::Bass,
            _ => return None,
        })
    }
    /// Name.
    pub fn name(self) -> &'static str {
        match self {
            VoiceKind::Soprano => "soprano",
            VoiceKind::Alto => "alto",
            VoiceKind::Tenor => "tenor",
            VoiceKind::Bass => "bass",
        }
    }
    /// The comfortable range as MIDI notes.
    pub fn range(self) -> (f32, f32) {
        match self {
            VoiceKind::Soprano => (60.0, 84.0),
            VoiceKind::Alto => (53.0, 77.0),
            VoiceKind::Tenor => (48.0, 72.0),
            VoiceKind::Bass => (40.0, 64.0),
        }
    }
    fn scale(self) -> f32 {
        match self {
            VoiceKind::Soprano => 1.18,
            VoiceKind::Alto => 1.12,
            VoiceKind::Tenor => 1.0,
            VoiceKind::Bass => 0.9,
        }
    }
    fn female(self) -> bool {
        matches!(self, VoiceKind::Soprano | VoiceKind::Alto)
    }
}

/// One sung note.
#[derive(Clone, Debug, PartialEq)]
pub struct SungNote {
    /// Pitch as a MIDI number (60 is middle C), fractions allowed.
    pub midi: f32,
    /// Start in seconds.
    pub start: f32,
    /// Length in seconds.
    pub dur: f32,
    /// What is sung on it: "doo", "bap", "la"…
    pub syllable: String,
    /// Loudness 0..1.
    pub vel: f32,
}

/// One voice and its part.
#[derive(Clone, Debug)]
pub struct Voice {
    /// A name for the stem.
    pub name: String,
    /// The kind of voice.
    pub kind: VoiceKind,
    /// The notes.
    pub notes: Vec<SungNote>,
    /// Stereo place -1..1.
    pub pan: f32,
    /// 0 straight, 1 a wide vibrato.
    pub vibrato: f32,
    /// 0 clear, 1 breathy.
    pub breath: f32,
}

/// When each syllable is sung, for lip sync.
#[derive(Clone, Debug, PartialEq)]
pub struct Sung {
    /// The syllable.
    pub text: String,
    /// Start in seconds.
    pub start: f32,
    /// End in seconds.
    pub end: f32,
    /// Pitch (MIDI).
    pub midi: f32,
    /// The vowel sung, for choosing a mouth shape.
    pub vowel: &'static str,
}

/// A voice rendered.
#[derive(Clone, Debug)]
pub struct Stem {
    /// The voice's name.
    pub name: String,
    /// Mono audio from time zero.
    pub audio: Vec<f32>,
    /// The syllables with their times.
    pub syllables: Vec<Sung>,
    /// Stereo place.
    pub pan: f32,
    /// Notes outside the voice's comfortable range.
    pub strained: usize,
}

// ----- syllables ---------------------------------------------------------------

/// Vowels: (name, F1, F2, F3).
const VOWELS: &[(&str, f32, f32, f32)] = &[
    ("ee", 270.0, 2290.0, 3010.0),
    ("ih", 390.0, 1990.0, 2550.0),
    ("eh", 530.0, 1840.0, 2480.0),
    ("ae", 660.0, 1720.0, 2410.0),
    ("ah", 730.0, 1090.0, 2440.0),
    ("aw", 570.0, 840.0, 2410.0),
    ("oh", 450.0, 800.0, 2830.0),
    ("oo", 300.0, 870.0, 2240.0),
    ("uh", 640.0, 1190.0, 2390.0),
    ("er", 490.0, 1350.0, 1690.0),
];

fn vowel(name: &str) -> (f32, f32, f32) {
    VOWELS.iter().find(|v| v.0 == name).map_or((730.0, 1090.0, 2440.0), |v| (v.1, v.2, v.3))
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Cons {
    /// Voiced or unvoiced stop: burst Hz, voiced.
    Stop(f32, bool),
    /// Fricative: noise centre Hz, Q, voiced.
    Fric(f32, f32, bool),
    /// Nasal with its second formant.
    Nasal(f32),
    /// Glide or liquid: F1, F2, F3.
    Glide(f32, f32, f32),
    /// Breath, shaped by the vowel.
    Aspirate,
}

impl Cons {
    fn len(self) -> f32 {
        match self {
            Cons::Stop(..) => 0.045,
            Cons::Fric(..) => 0.085,
            Cons::Nasal(_) => 0.065,
            Cons::Glide(..) => 0.06,
            Cons::Aspirate => 0.07,
        }
    }
}

fn cons_of(c: &str) -> Option<Cons> {
    Some(match c {
        "p" => Cons::Stop(900.0, false),
        "b" => Cons::Stop(700.0, true),
        "t" => Cons::Stop(4200.0, false),
        "d" => Cons::Stop(3200.0, true),
        "k" | "c" | "q" | "ck" | "x" => Cons::Stop(1900.0, false),
        "g" => Cons::Stop(1500.0, true),
        "s" => Cons::Fric(6800.0, 1.6, false),
        "z" => Cons::Fric(6200.0, 1.6, true),
        "sh" | "ch" | "j" => Cons::Fric(3600.0, 1.3, c == "j"),
        "f" | "ph" => Cons::Fric(5200.0, 0.5, false),
        "v" => Cons::Fric(4800.0, 0.5, true),
        "th" => Cons::Fric(5800.0, 0.5, false),
        "h" => Cons::Aspirate,
        "m" => Cons::Nasal(1000.0),
        "n" | "ng" => Cons::Nasal(1500.0),
        "l" => Cons::Glide(360.0, 1100.0, 2800.0),
        "r" => Cons::Glide(420.0, 1300.0, 1700.0),
        "w" => Cons::Glide(300.0, 700.0, 2200.0),
        "y" => Cons::Glide(280.0, 2200.0, 3000.0),
        _ => return None,
    })
}

struct Parsed {
    onset: Vec<Cons>,
    vowel: &'static str,
    coda: Vec<Cons>,
    /// A diphthong's second vowel.
    glide_to: Option<&'static str>,
}

fn is_vowel_letter(c: char) -> bool {
    matches!(c, 'a' | 'e' | 'i' | 'o' | 'u')
}

fn consonants(s: &str) -> Vec<Cons> {
    let mut out = Vec::new();
    let b: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < b.len() {
        let two: String = b[i..(i + 2).min(b.len())].iter().collect();
        if two.len() == 2 && matches!(two.as_str(), "sh" | "ch" | "th" | "ph" | "ng" | "ck") {
            out.extend(cons_of(&two));
            i += 2;
        } else {
            out.extend(cons_of(&b[i].to_string()));
            i += 1;
        }
    }
    out
}

/// Reads a syllable by its spelling.
fn parse_syllable(text: &str) -> Parsed {
    let t: String = text.to_lowercase().chars().filter(|c| c.is_ascii_alphabetic()).collect();
    if t.is_empty() {
        return Parsed { onset: vec![], vowel: "ah", coda: vec![], glide_to: None };
    }
    let chars: Vec<char> = t.chars().collect();
    let first_v = chars.iter().position(|c| is_vowel_letter(*c)).unwrap_or(chars.len());
    // A leading y is a glide ("ya"), a trailing one a vowel ("ly").
    let (onset_s, rest): (String, String) = (chars[..first_v].iter().collect(), chars[first_v..].iter().collect());
    let mut vend = rest.chars().position(|c| !is_vowel_letter(c)).unwrap_or(rest.len());
    // A y or w closing the word belongs to the vowel: "day", "dow".
    if vend > 0 && rest.len() == vend + 1 && matches!(rest.as_bytes()[vend], b'y' | b'w' | b'h') {
        vend += 1;
    }
    let (vg, coda_s) = rest.split_at(vend);
    let open = coda_s.is_empty();
    let mut glide_to = None;
    let v: &'static str = match vg {
        "" => {
            // No vowel letter: "ly", "my" → ee; "mm" → a hum; otherwise a schwa.
            if t.ends_with('y') { "ee" } else { "uh" }
        }
        "ee" | "ea" | "ie" => "ee",
        "i" => if open { "ee" } else { "ih" },
        "e" => if open { "eh" } else { "eh" },
        "a" => if open { "ah" } else { "ae" },
        "ah" | "aa" => "ah",
        "ai" | "ay" | "ei" | "ey" => {
            glide_to = Some("ih");
            "eh"
        }
        "o" => if open { "oh" } else { "aw" },
        "oh" => "oh",
        "oo" | "ou" | "ue" | "ew" => "oo",
        "ow" | "au" => {
            glide_to = Some("oo");
            "ah"
        }
        "aw" | "oa" => "aw",
        "u" => if open { "oo" } else { "uh" },
        "uh" => "uh",
        _ => "ah",
    };
    let coda_s = if coda_s == "y" { "" } else { coda_s };
    let mut onset = consonants(&onset_s);
    if onset_s.is_empty() && chars[0] == 'y' {
        onset = vec![Cons::Glide(280.0, 2200.0, 3000.0)];
    }
    onset.truncate(2);
    let mut coda = consonants(coda_s);
    coda.truncate(2);
    Parsed { onset, vowel: v, coda, glide_to }
}

// ----- the voice ---------------------------------------------------------------

#[derive(Clone, Copy)]
enum Seg {
    Cons(Cons),
    Vowel,
}

struct Plan {
    segs: Vec<(f32, f32, Seg)>,
    vowel: &'static str,
    glide_to: Option<&'static str>,
}

fn plan_note(n: &SungNote) -> Plan {
    let p = parse_syllable(&n.syllable);
    let mut onset_len: f32 = p.onset.iter().map(|c| c.len()).sum();
    let mut coda_len: f32 = p.coda.iter().map(|c| c.len()).sum();
    // Consonants take a share of the note, never most of it.
    let limit = 0.45 * n.dur;
    let k = if onset_len + coda_len > limit { limit / (onset_len + coda_len) } else { 1.0 };
    onset_len *= k;
    coda_len *= k;
    let mut segs = Vec::new();
    let mut t = n.start;
    for c in &p.onset {
        let l = c.len() * k;
        segs.push((t, t + l, Seg::Cons(*c)));
        t += l;
    }
    let vend = n.start + n.dur - coda_len;
    segs.push((t, vend.max(t + 0.01), Seg::Vowel));
    let mut t = vend;
    for c in &p.coda {
        let l = c.len() * k;
        segs.push((t, t + l, Seg::Cons(*c)));
        t += l;
    }
    let _ = onset_len;
    Plan { segs, vowel: p.vowel, glide_to: p.glide_to }
}

/// Renders one voice.
pub fn render_voice(v: &Voice, seed: u64) -> Stem {
    let mut rng = Rng::new(seed ^ 0x51_6E6);
    let end = v.notes.iter().map(|n| n.start + n.dur).fold(0.0, f32::max) + 0.3;
    let n = frames(end);
    let (lo, hi) = v.kind.range();
    let strained = v.notes.iter().filter(|x| x.midi < lo - 3.0 || x.midi > hi + 3.0).count();
    let plans: Vec<Plan> = v.notes.iter().map(plan_note).collect();
    let mut out = vec![0.0f32; n];
    let mut saw = Saw::at(rng.unit());
    let mut pul = Pulse::at(0.0);
    let mut form = [Svf::new(); 6];
    let mut nz = Svf::new();
    let mut lp = Lp1::default();
    let mut tilt = Lp1::default();
    let mut prev_saw = 0.0f32;
    let scale = v.kind.scale();
    let mut f0 = 0.0f32;
    let mut amp = 0.0f32;
    let mut fm = [500.0f32, 1500.0, 2500.0, 3500.0, 4500.0];
    let mut noise_env = 0.0f32;
    let mut burst_env = 0.0f32;
    let mut cur_idx: Option<usize> = None;
    let mut last_end = -1.0f32;
    let mut syllables = Vec::new();
    for (i, nn) in v.notes.iter().enumerate() {
        let pl = &plans[i];
        let vs = pl.segs.iter().find(|s| matches!(s.2, Seg::Vowel)).map_or((nn.start, nn.start + nn.dur), |s| (s.0, s.1));
        syllables.push(Sung { text: nn.syllable.clone(), start: nn.start, end: nn.start + nn.dur, midi: nn.midi, vowel: pl.vowel });
        let _ = vs;
    }
    // Notes in time order.
    let mut order: Vec<usize> = (0..v.notes.len()).collect();
    order.sort_by(|a, b| v.notes[*a].start.total_cmp(&v.notes[*b].start));
    let mut oi = 0usize;
    let mut bursts_fired: Vec<(usize, usize)> = Vec::new();
    let vib_depth = 6.0 + 26.0 * v.vibrato.clamp(0.0, 1.0);
    for (s, o) in out.iter_mut().enumerate() {
        let t = s as f32 / SR;
        // Which note are we in?
        while oi < order.len() && t >= v.notes[order[oi]].start + v.notes[order[oi]].dur {
            last_end = v.notes[order[oi]].start + v.notes[order[oi]].dur;
            oi += 1;
        }
        let note = order.get(oi).copied().filter(|k| t >= v.notes[*k].start);
        if note != cur_idx {
            if let Some(k) = note {
                // A new note after a rest starts on its pitch; a joined one glides.
                if t - last_end > 0.06 || f0 == 0.0 {
                    f0 = midi_hz(v.notes[k].midi);
                }
            }
            cur_idx = note;
        }
        let (mut target_amp, mut voiced_gate, mut noise_target, mut noise_hz, mut noise_q) = (0.0f32, 1.0f32, 0.0f32, 4000.0f32, 1.0f32);
        let mut target_f = fm;
        let mut vib_on = 0.0;
        if let Some(k) = note {
            let nn = &v.notes[k];
            let pl = &plans[k];
            let hz = midi_hz(nn.midi);
            f0 += (hz - f0) * (1.0 - (-1.0 / (0.03 * SR)).exp());
            let seg = pl.segs.iter().find(|sg| t >= sg.0 && t < sg.1).or_else(|| pl.segs.last());
            let (vf1, vf2, vf3) = vowel(pl.vowel);
            let mut vt = (vf1, vf2, vf3);
            if let Some(g) = pl.glide_to {
                // Diphthongs slide to their second vowel over the last half.
                let (g1, g2, g3) = vowel(g);
                let vseg = pl.segs.iter().find(|sg| matches!(sg.2, Seg::Vowel)).unwrap();
                let u = ((t - vseg.0) / (vseg.1 - vseg.0).max(0.01) - 0.4).clamp(0.0, 0.6) / 0.6;
                vt = (vf1 + (g1 - vf1) * u, vf2 + (g2 - vf2) * u, vf3 + (g3 - vf3) * u);
            }
            let kscale = scale;
            let mut f1 = vt.0 * kscale;
            // Sopranos tune the first formant up to the pitch when they sing high.
            if v.kind.female() && hz * 1.05 > f1 {
                f1 = (hz * 1.05).min(1100.0);
            }
            let vform = [f1, vt.1 * kscale.powf(0.8), vt.2 * kscale.powf(0.6), 3500.0 * kscale.powf(0.4), 4700.0];
            target_f = vform;
            vib_on = smooth(t - nn.start - 0.22, 0.35);
            if let Some((a, b, sg)) = seg.map(|x| (x.0, x.1, x.2)) {
                let u = ((t - a) / (b - a).max(1e-3)).clamp(0.0, 1.0);
                match sg {
                    Seg::Vowel => {
                        target_amp = 0.5 + 0.5 * nn.vel;
                        // Release on the last 35 ms unless the next note follows at once.
                        let next_joined = order.get(oi + 1).is_some_and(|nk| v.notes[*nk].start - (nn.start + nn.dur) < 0.03);
                        let to_end = nn.start + nn.dur - t;
                        let has_coda = pl.segs.last().is_some_and(|l| !matches!(l.2, Seg::Vowel));
                        if !next_joined && !has_coda {
                            target_amp *= smooth(to_end, 0.04);
                        }
                    }
                    Seg::Cons(c) => match c {
                        Cons::Stop(hz_b, voiced) => {
                            // Closure, then a burst, then voicing begins.
                            target_amp = if voiced { 0.12 * (1.0 - u) } else { 0.0 };
                            voiced_gate = if u < 0.5 { 0.0 } else { 1.0 };
                            if u > 0.45 && u < 0.7 {
                                noise_target = if voiced { 0.5 } else { 1.0 };
                                noise_hz = hz_b;
                                noise_q = 1.2;
                            }
                            target_f = [250.0, 1200.0, 2300.0, 3500.0, 4700.0];
                            let key = (k, (a * 1000.0) as usize);
                            if u > 0.45 && !bursts_fired.contains(&key) {
                                bursts_fired.push(key);
                                burst_env = 1.0;
                            }
                        }
                        Cons::Fric(c_hz, q, voiced) => {
                            noise_target = 0.8 * smooth(u, 0.2) * smooth(1.0 - u, 0.3);
                            noise_hz = c_hz;
                            noise_q = q;
                            target_amp = if voiced { 0.3 } else { 0.0 };
                            voiced_gate = if voiced { 1.0 } else { 0.0 };
                        }
                        Cons::Nasal(f2) => {
                            target_amp = 0.45;
                            target_f = [260.0, f2, 2300.0, 3300.0, 4500.0];
                        }
                        Cons::Glide(a1, a2, a3) => {
                            // From the glide's own formants toward the vowel.
                            let w = smooth(u, 1.0);
                            target_f = [a1 + (vform[0] - a1) * w, a2 + (vform[1] - a2) * w, a3 + (vform[2] - a3) * w, 3500.0, 4700.0];
                            target_amp = 0.5 + 0.4 * w;
                        }
                        Cons::Aspirate => {
                            noise_target = 0.5;
                            noise_hz = vt.1;
                            noise_q = 0.8;
                            target_amp = 0.1;
                            voiced_gate = 0.2;
                        }
                    },
                }
            }
        } else {
            f0 = f0.max(100.0);
        }
        // Smooth everything that moves.
        let k_amp = 1.0 - (-1.0 / (0.006 * SR)).exp();
        let k_fm = 1.0 - (-1.0 / (0.014 * SR)).exp();
        amp += (target_amp * voiced_gate - amp) * k_amp;
        for j in 0..5 {
            fm[j] += (target_f[j] - fm[j]) * k_fm;
        }
        noise_env += (noise_target - noise_env) * (1.0 - (-1.0 / (0.004 * SR)).exp());
        burst_env *= 1.0 - 1.0 / (0.008 * SR);
        // The source: a pulse train with a spectral tilt, a breath of noise.
        let vibr = cents(vib_depth * vib_on * (std::f32::consts::TAU * 5.4 * t).sin()) * (1.0 + 0.004 * rng.signed());
        let fr = (f0.max(40.0) * vibr).min(2000.0);
        // A flat-spectrum pulse train, so the vowel's upper resonances carry weight.
        let sv = saw.tick(fr);
        let flat = (sv - prev_saw) * (SR / (std::f32::consts::TAU * fr)).min(30.0);
        prev_saw = sv;
        let src = tilt.tick(0.3 * sv + 0.7 * flat + 0.15 * pul.tick(fr, 0.28), 4800.0);
        let breath = v.breath.clamp(0.0, 1.0);
        let n_in = rng.signed();
        let excite = src * amp + n_in * (0.02 + 0.12 * breath) * amp;
        // The tract: parallel resonances plus a little of the direct source.
        let gains = [1.0f32, 0.5, 0.22, 0.07, 0.03];
        let qs = [7.0f32, 9.0, 11.0, 13.0, 14.0];
        let mut y = 0.0;
        for j in 0..5 {
            y += form[j].tick(&SvfCoef::new(fm[j], qs[j]), excite).bp * gains[j];
        }
        y += lp.tick(excite, 900.0) * 0.18;
        // The singer's formant: extra ring near 3 kHz in trained voices.
        y += form[5].tick(&SvfCoef::new(2900.0, 6.0), excite).bp * 0.05;
        let noise_part = nz.tick(&SvfCoef::new(noise_hz, noise_q), n_in).bp * (noise_env + 1.4 * burst_env);
        *o = y * 1.6 + noise_part * 0.6;
    }
    let p = peak(&out).max(1e-6);
    out.iter_mut().for_each(|x| *x *= 0.8 / p);
    Stem { name: v.name.clone(), audio: out, syllables, pan: v.pan, strained }
}

/// Renders all the voices of a piece: one stem each, the same seed so a
/// take can be remade.
pub fn sing(voices: &[Voice], seed: u64) -> Vec<Stem> {
    voices.iter().enumerate().map(|(i, v)| render_voice(v, seed.wrapping_add(i as u64 * 977))).collect()
}

/// Reads a part written as text: notes separated by spaces or commas,
/// each `pitch:length:syllable` (or `pitch:start:length:syllable`).
/// With no start given a note follows the one before; a rest is `r:length`
/// (or `rest:length`). Lengths are seconds, or beats when `beat` seconds
/// are given with an `b` suffix, as in `1b`.
pub fn parse_part(text: &str, beat: f32) -> Result<Vec<SungNote>, String> {
    let mut notes = Vec::new();
    let mut t = 0.0f32;
    for tok in text.split(|c: char| c == ',' || c.is_whitespace()).filter(|s| !s.is_empty()) {
        let parts: Vec<&str> = tok.split(':').collect();
        let secs = |s: &str| -> Result<f32, String> {
            let (num, mult) = match s.strip_suffix('b') {
                Some(n) => (n, beat),
                None => (s, 1.0),
            };
            num.parse::<f32>().map(|v| v * mult).map_err(|_| format!("\"{tok}\": \"{s}\" is not a length"))
        };
        if matches!(parts[0].to_lowercase().as_str(), "r" | "rest") {
            t += secs(parts.get(1).ok_or_else(|| format!("\"{tok}\": a rest needs a length (r:0.5)"))?)?;
            continue;
        }
        let midi = parse_pitch(parts[0]).ok_or_else(|| format!("\"{tok}\": \"{}\" is not a pitch (C4, F#3, Bb2 or a MIDI number)", parts[0]))?;
        let (start, dur, syl) = match parts.len() {
            1 => (t, 0.5, "la"),
            2 => (t, secs(parts[1])?, "la"),
            3 => (t, secs(parts[1])?, parts[2]),
            _ => (secs(parts[1])?, secs(parts[2])?, parts[3]),
        };
        notes.push(SungNote { midi, start, dur: dur.max(0.05), syllable: syl.to_owned(), vel: 0.8 });
        t = start + dur;
    }
    if notes.is_empty() {
        return Err("no notes: write them like C4:0.5:doo E4:0.5:bee G4:1:bap".into());
    }
    Ok(notes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_sound::pitch::dominant_frequency;

    fn voice(kind: VoiceKind, notes: Vec<SungNote>) -> Voice {
        Voice { name: "v".into(), kind, notes, pan: 0.0, vibrato: 0.3, breath: 0.1 }
    }

    fn n(midi: f32, start: f32, dur: f32, syl: &str) -> SungNote {
        SungNote { midi, start, dur, syllable: syl.into(), vel: 0.8 }
    }

    #[test]
    fn it_sings_the_pitches_it_is_given() {
        for kind in [VoiceKind::Soprano, VoiceKind::Alto, VoiceKind::Tenor, VoiceKind::Bass] {
            let (lo, hi) = kind.range();
            let pitches = [lo + 2.0, (lo + hi) / 2.0, hi - 3.0, lo + 7.0];
            let notes: Vec<SungNote> = pitches.iter().enumerate().map(|(i, p)| n(p.round(), i as f32 * 0.7, 0.65, ["la", "doo", "bee", "dah"][i])).collect();
            let stem = render_voice(&voice(kind, notes.clone()), 1);
            for nn in &notes {
                let hz = midi_hz(nn.midi);
                let a = frames(nn.start + 0.25);
                let seg = &stem.audio[a..a + frames(0.3)];
                let got = dominant_frequency(seg, RATE, hz * 0.7, hz * 1.4).unwrap_or(0.0);
                let cents_off = 1200.0 * (got / hz).log2();
                assert!(cents_off.abs() < 30.0, "{kind:?} {}: wanted {hz:.1} Hz, sang {got:.1} ({cents_off:.0} cents)", nn.midi);
            }
        }
    }

    #[test]
    fn syllables_are_read_by_spelling() {
        let p = parse_syllable("doo");
        assert_eq!((p.onset.len(), p.vowel, p.coda.len()), (1, "oo", 0));
        let p = parse_syllable("bap");
        assert_eq!((p.vowel, p.coda.len()), ("ae", 1));
        assert_eq!(parse_syllable("bee").vowel, "ee");
        assert_eq!(parse_syllable("la").vowel, "ah");
        assert_eq!(parse_syllable("shoo").onset.len(), 1);
        assert_eq!(parse_syllable("skee").onset.len(), 2);
        assert_eq!(parse_syllable("day").glide_to, Some("ih"));
        assert_eq!(parse_syllable("").vowel, "ah");
    }

    #[test]
    fn vowels_have_their_formants_and_voices_differ() {
        // At a low pitch the harmonics are close enough to show where the resonances sit.
        let peak_hz = |syl: &str, lo: f32, hi: f32| {
            let s = render_voice(&voice(VoiceKind::Tenor, vec![n(48.0, 0.0, 1.2, syl)]), 2);
            let seg: Vec<f32> = s.audio[frames(0.3)..frames(0.3) + 16384].to_vec();
            let fft = studio_sound::fft::Fft::new(16384);
            let mut win = vec![0.0; 16384];
            studio_sound::fft::hann(&mut win);
            let mut re: Vec<f32> = seg.iter().zip(&win).map(|(a, w)| a * w).collect();
            let mut im = vec![0.0; 16384];
            fft.forward(&mut re, &mut im);
            let bin = SR / 16384.0;
            let pow: Vec<f32> = (0..8192).map(|k| re[k] * re[k] + im[k] * im[k]).collect();
            let w = (120.0 / bin) as usize;
            let smoothed = |k: usize| pow[k - w..k + w].iter().sum::<f32>();
            let (best, _) = ((lo / bin) as usize..(hi / bin) as usize).map(|k| (k, smoothed(k))).fold((0, 0.0f32), |a, b| if b.1 > a.1 { b } else { a });
            best as f32 * bin
        };
        let ee = peak_hz("ee", 1000.0, 3200.0);
        let oo = peak_hz("oo", 600.0, 3200.0);
        let ah = peak_hz("ah", 600.0, 3200.0);
        assert!((1900.0..2700.0).contains(&ee), "ee's upper resonance at {ee}");
        assert!((600.0..1150.0).contains(&oo), "oo's strongest resonance at {oo}");
        assert!((650.0..1400.0).contains(&ah), "ah's resonances at {ah}");
        let tenor = render_voice(&voice(VoiceKind::Tenor, vec![n(60.0, 0.0, 0.8, "ah")]), 3);
        let bass = render_voice(&voice(VoiceKind::Bass, vec![n(60.0, 0.0, 0.8, "ah")]), 3);
        assert_ne!(tenor.audio, bass.audio);
    }

    #[test]
    fn consonants_make_noise_and_notes_keep_time() {
        let s = render_voice(&voice(VoiceKind::Alto, vec![n(64.0, 0.2, 0.5, "sa"), n(67.0, 0.8, 0.5, "ta")]), 4);
        // The s of "sa" is noise before the voice starts: energy before 0.28 s, high in frequency.
        let hiss = &s.audio[frames(0.2)..frames(0.27)];
        assert!(rms(hiss) > 0.01 && rms(&high_pass(hiss, 3000.0, 0.7)) > rms(hiss) * 0.4);
        // Silence before the first note and between notes after the release.
        assert!(rms(&s.audio[..frames(0.15)]) < 0.01);
        assert!(rms(&s.audio[frames(0.74)..frames(0.78)]) < 0.05);
        assert_eq!(s.syllables.len(), 2);
        assert!((s.syllables[1].start - 0.8).abs() < 1e-6);
        assert!(s.audio.iter().all(|v| v.is_finite()) && peak(&s.audio) <= 0.81);
    }

    #[test]
    fn a_duet_has_a_stem_per_voice() {
        let a = voice(VoiceKind::Soprano, parse_part("C5:0.5:doo E5:0.5:bee G5:1:bap", 0.5).unwrap());
        let mut b = voice(VoiceKind::Tenor, parse_part("C4:0.5:doo r:0.5 E4:1b:da", 0.5).unwrap());
        b.name = "tenor".into();
        let stems = sing(&[a, b], 9);
        assert_eq!(stems.len(), 2);
        assert_eq!(stems[1].syllables.len(), 2);
        assert_ne!(stems[0].audio, stems[1].audio);
        assert!((stems[1].syllables[1].start - 1.0).abs() < 1e-6, "{:?}", stems[1].syllables);
    }

    #[test]
    fn parts_parse_and_report_mistakes() {
        let p = parse_part("C4:0.5:doo, D4:0.25 E4", 0.5).unwrap();
        assert_eq!(p.len(), 3);
        assert_eq!(p[0].midi, 60.0);
        assert!((p[1].start - 0.5).abs() < 1e-6 && (p[2].start - 0.75).abs() < 1e-6);
        assert!(parse_part("H9:1", 0.5).unwrap_err().contains("not a pitch"));
        assert!(parse_part("", 0.5).is_err());
        let q = parse_part("C4:1b:la", 0.6).unwrap();
        assert!((q[0].dur - 0.6).abs() < 1e-6);
    }

    #[test]
    fn notes_outside_the_range_are_counted() {
        let s = render_voice(&voice(VoiceKind::Bass, vec![n(80.0, 0.0, 0.5, "la")]), 1);
        assert_eq!(s.strained, 1);
    }
}

