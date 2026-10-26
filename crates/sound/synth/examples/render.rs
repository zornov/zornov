// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Writes short example renders to a folder: `cargo run -p studio-synth --example render -- <folder>`.

use studio_synth::dsp::{Stereo, frames};
use studio_synth::music::{Cue, CueKind, MusicSpec, compose};
use studio_synth::sfx;
use studio_synth::sing::{SungNote, Voice, VoiceKind, parse_part, sing};

fn save(dir: &std::path::Path, name: &str, s: &Stereo) {
    let path = dir.join(format!("{name}.wav"));
    std::fs::write(&path, s.wav_bytes()).expect("write wav");
    println!("{} ({:.1}s, peak {:.2})", path.display(), s.seconds(), s.peak());
}

fn main() {
    let dir = std::path::PathBuf::from(std::env::args().nth(1).expect("a folder to write into"));
    std::fs::create_dir_all(&dir).expect("create folder");
    let music: &[(&str, &str, f32, Option<f32>, Vec<Cue>)] = &[
        ("music-noir-jazz", "noir jazz with sax and double bass, rainy night", 24.0, None, vec![]),
        ("music-hopeful-folk", "hopeful acoustic folk", 24.0, None, vec![]),
        ("music-dark-score-cues", "dark slow score", 20.0, None, vec![Cue { at: 2.0, kind: CueKind::Hit }, Cue { at: 14.0, kind: CueKind::Resolve }]),
        ("music-minimal-electronic-120", "minimal electronic on a beat grid", 20.0, Some(120.0), vec![]),
        ("music-cinematic-riser", "cinematic strings and brass", 16.0, None, vec![Cue { at: 9.0, kind: CueKind::Riser }]),
        ("music-lofi", "lofi chill hip hop", 20.0, None, vec![]),
        ("music-playful", "playful quirky comedy", 16.0, None, vec![]),
    ];
    for (name, desc, secs, bpm, cues) in music {
        let mut spec = MusicSpec::new(desc, *secs);
        spec.bpm = *bpm;
        spec.cues = cues.clone();
        let piece = compose(&spec).expect("compose");
        println!("  {}", piece.plan.describe());
        save(&dir, name, &piece.audio);
    }
    for (name, desc, secs) in [
        ("sfx-match-strike", "match strike", None),
        ("sfx-riser-with-tail", "synth riser with a tail", Some(5.0)),
        ("sfx-distant-period-traffic", "distant period traffic", Some(8.0)),
        ("sfx-cafe-murmur", "café murmur", Some(8.0)),
        ("sfx-footsteps-gravel", "footsteps on gravel", Some(4.0)),
        ("sfx-passage", "footsteps on gravel, distant period traffic, café murmur and a match strike", Some(10.0)),
        ("sfx-pig", "pig oinking", None),
        ("sfx-horse", "horse neighing", None),
        ("sfx-hen", "hen clucking", None),
        ("sfx-sheep", "sheep bleating", None),
        ("sfx-dog-bark", "dog barking", Some(2.5)),
        ("sfx-dog-growl", "dog growling", None),
        ("sfx-door-knock", "three knocks on a door", None),
        ("sfx-fire", "fire crackling", Some(5.0)),
        ("sfx-rain-and-thunder", "rain and distant thunder", Some(7.0)),
    ] {
        let made = sfx::make(desc, secs, 0).expect("sfx");
        println!("  {} -> {}", desc, made.model);
        save(&dir, name, &made.audio);
    }
    // A scat duet with timed syllables.
    let beat = 0.5;
    let a = Voice { name: "soprano".into(), kind: VoiceKind::Soprano, notes: parse_part("E5:0.5b:doo G5:0.5b:bee C6:1b:bap r:0.5b A5:0.5b:da G5:0.5b:doo E5:1b:wah", beat).unwrap(), pan: -0.3, vibrato: 0.35, breath: 0.1 };
    let b = Voice { name: "tenor".into(), kind: VoiceKind::Tenor, notes: parse_part("r:1b C4:0.5b:doo E4:0.5b:bee G4:1b:bop r:0.5b F4:0.5b:da E4:0.5b:doo C4:1b:wah", beat).unwrap(), pan: 0.3, vibrato: 0.25, breath: 0.15 };
    let stems = sing(&[a, b], 1);
    let n = frames(6.0);
    let mut duet = Stereo::silent(n);
    for st in &stems {
        duet.add_mono(&st.audio, 0, 0.7, st.pan);
        let mut one = Stereo::silent(n);
        one.add_mono(&st.audio, 0, 0.9, 0.0);
        save(&dir, &format!("sing-stem-{}", st.name), &one);
    }
    save(&dir, "sing-scat-duet", &duet);
    let _ = SungNote { midi: 0.0, start: 0.0, dur: 0.0, syllable: String::new(), vel: 0.0 };
}
