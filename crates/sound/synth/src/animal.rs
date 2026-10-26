// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Animal voices by source and filter: a band-limited pulse train for the
//! vocal folds (a pitch that glides, shakes and roughens) with breath noise,
//! run through resonances that move like a mouth opening and closing. Each
//! call is a handful of syllables with a pitch contour and three formants.
//! The formant numbers are typical of the species, not measurements of one
//! animal.

use crate::dsp::*;

/// One syllable of a call.
#[derive(Clone, Copy, Debug)]
pub struct Syl {
    /// Start in seconds.
    pub at: f32,
    /// Length in seconds.
    pub dur: f32,
    /// Pitch at the start, middle and end (Hz).
    pub f0: [f32; 3],
    /// First, second and third formant: start and end (Hz).
    pub fm: [[f32; 2]; 3],
    /// Loudness 0..1.
    pub amp: f32,
    /// Roughness: rate (Hz) and depth (0..1) of an amplitude flutter, as in a growl.
    pub rough: (f32, f32),
    /// Breath noise 0..1.
    pub noise: f32,
    /// Pitch wobble: rate (Hz) and depth (cents).
    pub vib: (f32, f32),
    /// Amplitude warble: rate (Hz) and depth 0..1 (a sheep's bleat).
    pub warble: (f32, f32),
}

#[allow(clippy::too_many_arguments)]
const fn syl(at: f32, dur: f32, f0: [f32; 3], f1: [f32; 2], f2: [f32; 2], f3: [f32; 2], amp: f32, rough: (f32, f32), noise: f32) -> Syl {
    Syl { at, dur, f0, fm: [f1, f2, f3], amp, rough, noise, vib: (0.0, 0.0), warble: (0.0, 0.0) }
}

const fn wob(mut s: Syl, vib: (f32, f32), warble: (f32, f32)) -> Syl {
    s.vib = vib;
    s.warble = warble;
    s
}

/// One kind of call.
#[derive(Clone, Copy, Debug)]
pub struct Call {
    /// Name ("bark").
    pub name: &'static str,
    /// Words that ask for it.
    pub words: &'static [&'static str],
    /// The syllables.
    pub syls: &'static [Syl],
}

/// A species.
#[derive(Clone, Copy, Debug)]
pub struct Species {
    /// Name.
    pub name: &'static str,
    /// Words that mean it.
    pub words: &'static [&'static str],
    /// Its calls; the first is the usual one.
    pub calls: &'static [Call],
}

const NONE: (f32, f32) = (0.0, 0.0);

/// Every species.
pub const SPECIES: &[Species] = &[
    Species {
        name: "pig",
        words: &["pig", "hog", "piglet", "swine", "boar", "oink", "sow"],
        calls: &[
            Call { name: "oink", words: &["oink", "grunt", "snort"], syls: &[
                syl(0.0, 0.2, [110.0, 150.0, 130.0], [430.0, 520.0], [900.0, 1100.0], [2400.0, 2400.0], 0.9, (45.0, 0.5), 0.12),
                syl(0.2, 0.17, [130.0, 120.0, 100.0], [1000.0, 1250.0], [2200.0, 2600.0], [3300.0, 3300.0], 0.7, (60.0, 0.4), 0.55),
            ] },
            Call { name: "squeal", words: &["squeal", "scream", "shriek"], syls: &[
                syl(0.0, 0.85, [650.0, 1500.0, 900.0], [900.0, 1100.0], [2300.0, 2700.0], [3800.0, 3800.0], 0.9, (80.0, 0.2), 0.1),
            ] },
        ],
    },
    Species {
        name: "horse",
        words: &["horse", "stallion", "mare", "pony", "foal", "neigh", "whinny"],
        calls: &[
            Call { name: "neigh", words: &["neigh", "whinny", "call"], syls: &[
                wob(syl(0.0, 0.32, [800.0, 1300.0, 1500.0], [750.0, 900.0], [1700.0, 2000.0], [3200.0, 3300.0], 0.85, NONE, 0.2), (6.0, 25.0), NONE),
                wob(syl(0.3, 0.9, [1450.0, 900.0, 420.0], [900.0, 700.0], [2000.0, 1600.0], [3300.0, 2800.0], 0.9, (70.0, 0.35), 0.22), (5.5, 30.0), NONE),
            ] },
            Call { name: "snort", words: &["snort", "huff", "blow"], syls: &[
                syl(0.0, 0.35, [180.0, 150.0, 120.0], [600.0, 500.0], [1500.0, 1200.0], [2800.0, 2500.0], 0.7, (30.0, 0.7), 0.95),
            ] },
        ],
    },
    Species {
        name: "hen",
        words: &["hen", "chicken", "chook", "fowl", "cluck", "poultry", "chick"],
        calls: &[
            Call { name: "cluck", words: &["cluck", "bawk", "call"], syls: &[
                syl(0.0, 0.11, [720.0, 700.0, 520.0], [850.0, 950.0], [2100.0, 2300.0], [3400.0, 3400.0], 0.85, (110.0, 0.3), 0.15),
                syl(0.2, 0.1, [760.0, 720.0, 560.0], [900.0, 1000.0], [2200.0, 2300.0], [3500.0, 3500.0], 0.8, (110.0, 0.3), 0.15),
                syl(0.38, 0.12, [700.0, 680.0, 500.0], [850.0, 900.0], [2100.0, 2200.0], [3400.0, 3400.0], 0.75, (110.0, 0.3), 0.15),
                syl(0.62, 0.13, [740.0, 800.0, 560.0], [900.0, 1050.0], [2200.0, 2400.0], [3500.0, 3500.0], 0.8, (110.0, 0.3), 0.15),
            ] },
            Call { name: "cackle", words: &["cackle", "squawk", "panic"], syls: &[
                syl(0.0, 0.09, [800.0, 900.0, 700.0], [900.0, 1000.0], [2200.0, 2400.0], [3500.0, 3500.0], 0.9, (120.0, 0.35), 0.2),
                syl(0.12, 0.09, [850.0, 950.0, 720.0], [900.0, 1000.0], [2200.0, 2400.0], [3500.0, 3500.0], 0.9, (120.0, 0.35), 0.2),
                syl(0.23, 0.09, [900.0, 1000.0, 760.0], [950.0, 1050.0], [2300.0, 2500.0], [3500.0, 3500.0], 0.9, (120.0, 0.35), 0.2),
                syl(0.33, 0.09, [950.0, 1050.0, 780.0], [950.0, 1050.0], [2300.0, 2500.0], [3500.0, 3500.0], 0.9, (120.0, 0.35), 0.2),
                syl(0.42, 0.09, [900.0, 1000.0, 740.0], [950.0, 1050.0], [2300.0, 2500.0], [3500.0, 3500.0], 0.85, (120.0, 0.35), 0.2),
                syl(0.5, 0.35, [800.0, 700.0, 450.0], [900.0, 800.0], [2200.0, 1900.0], [3500.0, 3000.0], 0.8, (100.0, 0.4), 0.25),
            ] },
        ],
    },
    Species {
        name: "sheep",
        words: &["sheep", "lamb", "ewe", "ram", "baa", "bleat", "flock", "goat"],
        calls: &[
            Call { name: "baa", words: &["baa", "bleat", "call"], syls: &[
                wob(syl(0.0, 0.95, [300.0, 350.0, 310.0], [760.0, 700.0], [1350.0, 1250.0], [2800.0, 2700.0], 0.9, NONE, 0.1), (5.5, 18.0), (34.0, 0.5)),
            ] },
            Call { name: "lamb", words: &["lamb", "baby", "mew"], syls: &[
                wob(syl(0.0, 0.6, [480.0, 560.0, 470.0], [850.0, 780.0], [1500.0, 1400.0], [3000.0, 2900.0], 0.85, NONE, 0.08), (6.0, 20.0), (38.0, 0.45)),
            ] },
        ],
    },
    Species {
        name: "dog",
        words: &["dog", "puppy", "hound", "bark", "woof", "growl", "pup"],
        calls: &[
            Call { name: "bark", words: &["bark", "woof", "arf", "yap"], syls: &[
                syl(0.0, 0.2, [520.0, 430.0, 330.0], [700.0, 620.0], [1250.0, 1150.0], [2700.0, 2500.0], 0.95, (90.0, 0.3), 0.35),
            ] },
            Call { name: "growl", words: &["growl", "snarl", "grumble"], syls: &[
                syl(0.0, 1.3, [78.0, 85.0, 70.0], [500.0, 560.0], [1000.0, 1100.0], [2400.0, 2300.0], 0.9, (28.0, 0.8), 0.3),
            ] },
            Call { name: "whine", words: &["whine", "whimper", "cry"], syls: &[
                wob(syl(0.0, 0.8, [600.0, 1000.0, 700.0], [600.0, 700.0], [2300.0, 2500.0], [3300.0, 3300.0], 0.7, NONE, 0.08), (6.0, 40.0), NONE),
            ] },
            Call { name: "howl", words: &["howl", "wolf"], syls: &[
                wob(syl(0.0, 2.4, [380.0, 560.0, 440.0], [650.0, 800.0], [1100.0, 1200.0], [2600.0, 2600.0], 0.85, NONE, 0.1), (5.0, 35.0), NONE),
            ] },
        ],
    },
    Species {
        name: "cat",
        words: &["cat", "kitten", "kitty", "meow", "miaow", "purr", "tomcat"],
        calls: &[
            Call { name: "meow", words: &["meow", "miaow", "mew", "call"], syls: &[
                wob(syl(0.0, 0.75, [450.0, 720.0, 380.0], [700.0, 850.0], [1100.0, 2300.0], [2700.0, 3300.0], 0.85, NONE, 0.08), (5.5, 22.0), NONE),
            ] },
            Call { name: "purr", words: &["purr", "contented"], syls: &[
                syl(0.0, 1.8, [28.0, 30.0, 26.0], [250.0, 300.0], [800.0, 900.0], [2200.0, 2200.0], 0.7, (26.0, 0.9), 0.45),
            ] },
        ],
    },
    Species {
        name: "cow",
        words: &["cow", "bull", "calf", "cattle", "moo", "ox"],
        calls: &[Call { name: "moo", words: &["moo", "low", "call"], syls: &[
            wob(syl(0.0, 1.6, [115.0, 135.0, 118.0], [450.0, 320.0], [900.0, 820.0], [2300.0, 2200.0], 0.9, NONE, 0.12), (5.0, 12.0), NONE),
        ] }],
    },
    Species {
        name: "duck",
        words: &["duck", "mallard", "duckling", "quack", "goose"],
        calls: &[Call { name: "quack", words: &["quack", "call"], syls: &[
            syl(0.0, 0.22, [420.0, 400.0, 330.0], [1100.0, 1000.0], [2300.0, 2100.0], [3300.0, 3200.0], 0.9, (140.0, 0.55), 0.25),
            syl(0.33, 0.2, [400.0, 380.0, 320.0], [1050.0, 950.0], [2200.0, 2100.0], [3300.0, 3200.0], 0.8, (140.0, 0.55), 0.25),
        ] }],
    },
    Species {
        name: "rooster",
        words: &["rooster", "cock", "cockerel", "crow", "cock-a-doodle-doo"],
        calls: &[Call { name: "crow", words: &["crow", "cock-a-doodle-doo", "call"], syls: &[
            syl(0.0, 0.24, [600.0, 800.0, 750.0], [850.0, 950.0], [1700.0, 1900.0], [3000.0, 3000.0], 0.85, (90.0, 0.2), 0.1),
            syl(0.28, 0.2, [900.0, 950.0, 850.0], [900.0, 950.0], [1900.0, 2000.0], [3100.0, 3100.0], 0.85, (90.0, 0.2), 0.1),
            syl(0.52, 0.28, [1000.0, 1050.0, 900.0], [950.0, 1000.0], [2000.0, 2100.0], [3200.0, 3200.0], 0.9, (90.0, 0.2), 0.1),
            wob(syl(0.85, 0.9, [950.0, 800.0, 520.0], [1000.0, 800.0], [2100.0, 1700.0], [3200.0, 2900.0], 0.9, (100.0, 0.3), 0.15), (6.0, 30.0), NONE),
        ] }],
    },
    Species {
        name: "crow",
        words: &["crow", "raven", "rook", "caw", "jackdaw", "magpie"],
        calls: &[Call { name: "caw", words: &["caw", "call", "croak"], syls: &[
            syl(0.0, 0.4, [620.0, 560.0, 380.0], [900.0, 800.0], [1900.0, 1700.0], [3100.0, 3000.0], 0.9, (95.0, 0.5), 0.3),
            syl(0.55, 0.38, [600.0, 540.0, 370.0], [900.0, 800.0], [1900.0, 1700.0], [3100.0, 3000.0], 0.85, (95.0, 0.5), 0.3),
        ] }],
    },
    Species {
        name: "owl",
        words: &["owl", "hoot", "tawny"],
        calls: &[Call { name: "hoot", words: &["hoot", "call"], syls: &[
            wob(syl(0.0, 0.5, [390.0, 410.0, 380.0], [420.0, 400.0], [800.0, 780.0], [2300.0, 2300.0], 0.8, NONE, 0.06), (4.5, 10.0), NONE),
            wob(syl(0.9, 0.6, [380.0, 400.0, 340.0], [420.0, 400.0], [800.0, 760.0], [2300.0, 2300.0], 0.8, NONE, 0.06), (4.5, 10.0), NONE),
        ] }],
    },
    Species {
        name: "frog",
        words: &["frog", "toad", "ribbit", "croak"],
        calls: &[Call { name: "ribbit", words: &["ribbit", "croak", "call"], syls: &[
            syl(0.0, 0.14, [140.0, 170.0, 130.0], [500.0, 450.0], [1500.0, 1300.0], [2600.0, 2400.0], 0.9, (45.0, 0.8), 0.15),
            syl(0.2, 0.22, [150.0, 200.0, 120.0], [600.0, 480.0], [1700.0, 1400.0], [2700.0, 2500.0], 0.95, (50.0, 0.8), 0.15),
        ] }],
    },
    Species {
        name: "lion",
        words: &["lion", "tiger", "big cat", "roar", "lioness", "jaguar"],
        calls: &[Call { name: "roar", words: &["roar", "call"], syls: &[
            syl(0.0, 2.2, [70.0, 110.0, 55.0], [600.0, 450.0], [1100.0, 900.0], [2300.0, 2000.0], 0.95, (24.0, 0.7), 0.45),
        ] }],
    },
    Species {
        name: "elephant",
        words: &["elephant", "trumpet", "mammoth"],
        calls: &[Call { name: "trumpet", words: &["trumpet", "call", "roar"], syls: &[
            wob(syl(0.0, 1.5, [380.0, 900.0, 650.0], [700.0, 1000.0], [1700.0, 2300.0], [3300.0, 3500.0], 0.95, (60.0, 0.4), 0.3), (6.0, 25.0), NONE),
        ] }],
    },
    Species {
        name: "donkey",
        words: &["donkey", "ass", "mule", "hee-haw", "bray"],
        calls: &[Call { name: "bray", words: &["bray", "hee-haw", "call"], syls: &[
            syl(0.0, 0.5, [450.0, 700.0, 650.0], [800.0, 950.0], [1800.0, 2100.0], [3200.0, 3200.0], 0.85, (70.0, 0.35), 0.2),
            syl(0.55, 0.7, [320.0, 230.0, 180.0], [650.0, 550.0], [1300.0, 1100.0], [2800.0, 2600.0], 0.95, (65.0, 0.45), 0.25),
            syl(1.35, 0.4, [480.0, 720.0, 660.0], [800.0, 950.0], [1800.0, 2100.0], [3200.0, 3200.0], 0.8, (70.0, 0.35), 0.2),
            syl(1.8, 0.8, [300.0, 220.0, 170.0], [650.0, 550.0], [1300.0, 1100.0], [2800.0, 2600.0], 0.9, (65.0, 0.45), 0.25),
        ] }],
    },
    Species {
        name: "mouse",
        words: &["mouse", "rat", "squeak", "hamster", "rodent"],
        calls: &[Call { name: "squeak", words: &["squeak", "call"], syls: &[
            syl(0.0, 0.12, [3800.0, 5200.0, 4300.0], [3500.0, 4200.0], [6500.0, 7000.0], [9000.0, 9000.0], 0.7, NONE, 0.05),
            syl(0.2, 0.1, [4000.0, 5400.0, 4400.0], [3500.0, 4200.0], [6500.0, 7000.0], [9000.0, 9000.0], 0.6, NONE, 0.05),
        ] }],
    },
];

/// The species and calls on offer, for replies.
pub fn catalogue() -> String {
    SPECIES.iter().map(|s| format!("{} ({})", s.name, s.calls.iter().map(|c| c.name).collect::<Vec<_>>().join(", "))).collect::<Vec<_>>().join("; ")
}

/// A species and call from words ("a dog barking", "pig squeal", "hen").
pub fn find(text: &str) -> Option<(&'static Species, &'static Call)> {
    let t = format!(" {} ", text.to_lowercase().replace([',', '.', ';', ':'], " "));
    // The earliest species word wins; the call is a call word that species has.
    let mut best: Option<(usize, &Species)> = None;
    for s in SPECIES {
        for w in s.words {
            if let Some(p) = t.find(&format!(" {w} ")).or_else(|| t.find(&format!(" {w}s ")).or_else(|| t.find(&format!(" {w}ing ")))) {
                if best.is_none_or(|(bp, _)| p < bp) {
                    best = Some((p, s));
                }
            }
        }
    }
    let (_, sp) = best?;
    let call = sp
        .calls
        .iter()
        .find(|c| c.words.iter().any(|w| *w != "call" && (t.contains(&format!(" {w} ")) || t.contains(&format!(" {w}s ")) || t.contains(&format!(" {w}ing ")) || t.contains(&format!(" {w}ed ")))))
        .unwrap_or(&sp.calls[0]);
    Some((sp, call))
}

/// Roughly where a call's energy sits: the mean of its three formants over
/// its syllables, weighted by length, and its mean pitch.
pub fn mean_formants(call: &Call) -> ([f32; 3], f32) {
    let mut f = [0.0f32; 3];
    let mut p = 0.0;
    let mut w = 0.0;
    for s in call.syls {
        for (i, fm) in s.fm.iter().enumerate() {
            f[i] += (fm[0] + fm[1]) * 0.5 * s.dur;
        }
        p += (s.f0[0] + s.f0[1] + s.f0[2]) / 3.0 * s.dur;
        w += s.dur;
    }
    ([f[0] / w, f[1] / w, f[2] / w], p / w)
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn render_syl(s: &Syl, scale: f32, form: f32, rng: &mut Rng) -> Vec<f32> {
    let total = s.dur + 0.04;
    let len = frames(total);
    let mut saw = Saw::at(rng.unit());
    let mut pul = Pulse::at(rng.unit());
    let mut filters = [Svf::new(), Svf::new(), Svf::new()];
    let gains = [1.0f32, 0.75, 0.4];
    let mut walk = 0.0f32;
    let mut lp = Lp1::default();
    let mut out = vec![0.0; len];
    let att = (s.dur * 0.12).clamp(0.006, 0.06);
    let rel = (s.dur * 0.2).clamp(0.01, 0.12);
    for (i, o) in out.iter_mut().enumerate() {
        let t = i as f32 / SR;
        let p = (t / s.dur).clamp(0.0, 1.0);
        let f0 = if p < 0.5 { lerp(s.f0[0], s.f0[1], p * 2.0) } else { lerp(s.f0[1], s.f0[2], (p - 0.5) * 2.0) } * scale;
        walk += (rng.signed() - walk * 40.0 / SR * 20.0) * 0.00018;
        let f = f0 * cents(s.vib.1 * (std::f32::consts::TAU * s.vib.0 * t).sin()) * (1.0 + walk);
        let glottal = 0.65 * saw.tick(f) + 0.45 * pul.tick(f, 0.22);
        let n = rng.signed();
        let src = lp.tick(glottal, f * 12.0 + 2000.0) * (1.0 - 0.5 * s.noise) + n * s.noise;
        let mut y = 0.0;
        for (k, flt) in filters.iter_mut().enumerate() {
            let fc = lerp(s.fm[k][0], s.fm[k][1], p) * form;
            let q = [5.0, 7.0, 8.0][k];
            y += flt.tick(&SvfCoef::new(fc, q), src).bp * gains[k];
        }
        // Keep a little of the raw source so the pitch stays clear.
        y = y * 1.6 + src * 0.12;
        let rough = if s.rough.1 > 0.0 { 1.0 - s.rough.1 * 0.5 * (1.0 + (std::f32::consts::TAU * s.rough.0 * t).sin()) } else { 1.0 };
        let warble = if s.warble.1 > 0.0 { 1.0 - s.warble.1 * 0.5 * (1.0 + (std::f32::consts::TAU * s.warble.0 * t).sin()) } else { 1.0 };
        let env = smooth(t, att) * smooth(total - t, rel);
        *o = y * env * rough * warble * s.amp;
    }
    out
}

/// One call from a species, `variant` changing the individual (pitch, vowel
/// and timing differ a little), `size` (0.5 small .. 2 large) scaling the
/// pitch down for big animals and up for young ones, `intensity` 0..1.
pub fn render(sp: &Species, call: &Call, variant: u64, size: f32, intensity: f32) -> Vec<f32> {
    let mut rng = Rng::new(variant.wrapping_mul(0x9E37_79B9) ^ sp.name.len() as u64 * 131 ^ call.name.len() as u64 * 17);
    let pitch = rng.range(0.93, 1.07) / size.clamp(0.5, 2.0).powf(0.6);
    let form = rng.range(0.94, 1.06) / size.clamp(0.5, 2.0).powf(0.25);
    let end = call.syls.iter().map(|s| s.at + s.dur).fold(0.0, f32::max) * rng.range(0.94, 1.08);
    let mut out = vec![0.0; frames(end + 0.2)];
    for s in call.syls {
        let mut s2 = *s;
        s2.at *= rng.range(0.96, 1.05);
        s2.amp *= (0.55 + 0.45 * intensity.clamp(0.0, 1.0)) * rng.range(0.9, 1.0);
        s2.rough.1 = (s2.rough.1 * (0.7 + 0.6 * intensity)).min(0.95);
        let x = render_syl(&s2, pitch, form, &mut rng);
        mix_into(&mut out, &x, frames(s2.at), 1.0);
    }
    let p = peak(&out).max(1e-6);
    out.iter_mut().for_each(|v| *v *= 0.85 / p);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_sound::fft::Fft;

    /// Smoothed power spectrum of a whole sound: (Hz, power) per bin.
    fn spectrum(x: &[f32]) -> Vec<(f32, f32)> {
        let n = 8192;
        let fft = Fft::new(n);
        let mut acc = vec![0.0f32; n / 2];
        let mut count = 0;
        let mut pos = 0;
        while pos + n <= x.len() {
            let mut re: Vec<f32> = x[pos..pos + n].to_vec();
            let mut win = vec![0.0; n];
            studio_sound::fft::hann(&mut win);
            re.iter_mut().zip(&win).for_each(|(a, w)| *a *= w);
            let mut im = vec![0.0; n];
            fft.forward(&mut re, &mut im);
            for k in 0..n / 2 {
                acc[k] += re[k] * re[k] + im[k] * im[k];
            }
            count += 1;
            pos += n / 2;
        }
        assert!(count > 0, "sound too short");
        acc.iter().enumerate().map(|(k, p)| (k as f32 * SR / n as f32, *p / count as f32)).collect()
    }

    #[test]
    fn every_call_is_audible_and_differs_per_individual() {
        for sp in SPECIES {
            for c in sp.calls {
                let a = render(sp, c, 1, 1.0, 0.8);
                let b = render(sp, c, 2, 1.0, 0.8);
                assert!(peak(&a) > 0.5 && a.iter().all(|v| v.is_finite()), "{} {}", sp.name, c.name);
                assert_ne!(a, b, "{} {}: two animals the same", sp.name, c.name);
            }
        }
    }

    #[test]
    fn declared_formants_are_plausible_for_a_vocal_tract() {
        for sp in SPECIES {
            for c in sp.calls {
                let (f, p) = mean_formants(c);
                // Small animals have higher formants than any mammal larger than a rat: allow the mouse.
                let small = sp.name == "mouse";
                let (lo1, hi1) = if small { (3000.0, 5000.0) } else if c.name == "purr" { (200.0, 400.0) } else { (280.0, 1200.0) };
                assert!((lo1..=hi1).contains(&f[0]), "{} {}: F1 {}", sp.name, c.name, f[0]);
                assert!(f[1] > f[0] * 1.15 && f[2] > f[1] * 1.1, "{} {}: formants out of order {f:?}", sp.name, c.name);
                assert!(f[1] < if small { 8000.0 } else { 2700.0 } && f[2] < if small { 10_000.0 } else { 4200.0 }, "{} {}: {f:?}", sp.name, c.name);
                assert!(p > 20.0 && p < 6000.0, "{} {}: f0 {p}", sp.name, c.name);
            }
        }
    }

    #[test]
    fn the_sound_has_its_energy_around_its_formants() {
        for sp in SPECIES {
            for c in sp.calls.iter().filter(|c| c.name != "purr") {
                let x = render(sp, c, 3, 1.0, 0.9);
                let mut sx = x.clone();
                if sx.len() < 8192 * 2 {
                    sx.resize(8192 * 2, 0.0);
                }
                let spec = spectrum(&sx);
                // Smooth over ~300 Hz and find the strongest region above the pitch.
                let (f, p) = mean_formants(c);
                let bin = SR / 8192.0;
                let w = (150.0 / bin) as usize;
                let smooth_at = |k: usize| spec[k.saturating_sub(w)..(k + w).min(spec.len())].iter().map(|s| s.1).sum::<f32>();
                let from = ((p * 1.3).max(150.0) / bin) as usize;
                let to = (9000.0 / bin) as usize;
                let (best, _) = (from..to).map(|k| (k, smooth_at(k))).fold((0, 0.0f32), |a, b| if b.1 > a.1 { b } else { a });
                let peak_hz = best as f32 * bin;
                assert!(peak_hz >= f[0] * 0.55 && peak_hz <= f[2] * 1.2, "{} {}: energy peaks at {peak_hz:.0} Hz, formants {f:?}", sp.name, c.name);
            }
        }
    }

    #[test]
    fn size_lowers_the_voice() {
        let (sp, c) = find("a dog barking").unwrap();
        let big = render(sp, c, 1, 2.0, 0.8);
        let small = render(sp, c, 1, 0.6, 0.8);
        let zc = |x: &[f32]| x.windows(2).filter(|w| (w[0] < 0.0) != (w[1] < 0.0)).count() as f32 / x.len() as f32;
        assert!(zc(&small) > zc(&big));
    }

    #[test]
    fn words_find_species_and_calls() {
        assert_eq!(find("a pig oinking").map(|(s, c)| (s.name, c.name)), Some(("pig", "oink")));
        assert_eq!(find("horse neighing").map(|(s, _)| s.name), Some("horse"));
        assert_eq!(find("hen clucking").map(|(s, c)| (s.name, c.name)), Some(("hen", "cluck")));
        assert_eq!(find("sheep bleating").map(|(s, _)| s.name), Some("sheep"));
        assert_eq!(find("dog bark").map(|(s, c)| (s.name, c.name)), Some(("dog", "bark")));
        assert_eq!(find("a dog growling in the dark").map(|(_, c)| c.name), Some("growl"));
        assert!(find("a spaceship").is_none());
    }
}
