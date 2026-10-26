// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Genres and moods as data: tempo, feel, chord progressions, who plays
//! what, and how the piece builds. The composition engine reads these;
//! adding a genre is adding a row.

use crate::instrument::Inst;
use crate::theory::{Mode, Q};

/// How a layer plays.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Role {
    /// The tune.
    Lead,
    /// The tune a third below.
    Harmony,
    /// Chords with a rhythm.
    Comp(Comp),
    /// The bass line.
    Bass(Bass),
    /// Held chords.
    Pad,
    /// Broken chords on a steady grid.
    Arp,
    /// A repeated pulse on one chord tone.
    Ostinato,
    /// A low held note and its fifth, under everything.
    Drone,
    /// Rare, ringing notes with space around them.
    Sparse,
}

/// Rhythms for chord instruments.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Comp {
    /// Syncopated jazz voicings.
    Jazz,
    /// Down and up strokes of a guitar.
    Strum,
    /// Fingerpicked arpeggio.
    Picking,
    /// Piano ballad: bass note and broken chord.
    Block,
    /// Short offbeat stabs.
    Stabs,
    /// Oom-pah: low note, then chord.
    OomPah,
}

/// Bass lines.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Bass {
    /// Four to the bar, walking between chords.
    Walking,
    /// Root and fifth.
    RootFifth,
    /// One long root a bar.
    Pedal,
    /// Steady eighths with octave jumps.
    Pulse,
    /// Root on the offbeats.
    Offbeat,
    /// Behind-the-beat hip-hop pattern.
    Lazy,
    /// Driving eighth roots.
    Rock,
}

/// Drum patterns.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Beat {
    /// No drums.
    None,
    /// Ride swing with feathered kick.
    Jazz,
    /// Slow brush swirls.
    Brushes,
    /// Soft kick, shaker, tambourine.
    Folk,
    /// Timpani and taiko with cymbal swells.
    Cinematic,
    /// A heartbeat and a distant drum.
    Dark,
    /// Four on the floor, a tight grid.
    Minimal,
    /// Lazy boom-bap.
    Lofi,
    /// Kick, snare, eighth hats.
    Rock,
    /// Pop backbeat with a clap.
    Pop,
    /// Oom-pah with a brush snare.
    Playful,
}

/// A part in the arrangement.
#[derive(Clone, Copy, Debug)]
pub struct Layer {
    /// Who plays.
    pub inst: Inst,
    /// What they play.
    pub role: Role,
    /// Comes in when the energy reaches this (0..1).
    pub enter: f32,
    /// Level in dB.
    pub db: f32,
    /// Stereo place (-1..1).
    pub pan: f32,
    /// How much goes to the reverb (0..1).
    pub send: f32,
}

const fn layer(inst: Inst, role: Role, enter: f32, db: f32, pan: f32, send: f32) -> Layer {
    Layer { inst, role, enter, db, pan, send }
}

/// A chord progression: `(semitones above the tonic, quality)`, each chord
/// lasting `per` bars.
#[derive(Clone, Copy, Debug)]
pub struct Prog {
    /// Bars per chord.
    pub per: u32,
    /// The chords.
    pub chords: &'static [(i32, Q)],
}

/// How the energy moves over the piece.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Arc {
    /// Builds from quiet to full.
    Rise,
    /// Stays about even.
    Steady,
    /// Starts full and thins.
    Fall,
    /// Up and back down.
    Wave,
}

/// A genre.
#[derive(Clone, Copy, Debug)]
pub struct Style {
    /// The name.
    pub name: &'static str,
    /// Words that mean it.
    pub words: &'static [&'static str],
    /// Default, lowest and highest tempo (beats a minute).
    pub bpm: (f32, f32, f32),
    /// Beats in a bar.
    pub meter: u32,
    /// Swing of the eighths: 0 straight, 0.33 triplet feel.
    pub swing: f32,
    /// Scale for a major feel.
    pub major_mode: Mode,
    /// Scale for a minor feel.
    pub minor_mode: Mode,
    /// Progressions for a major feel.
    pub major: &'static [Prog],
    /// Progressions for a minor feel.
    pub minor: &'static [Prog],
    /// The band, in order of coming in.
    pub layers: &'static [Layer],
    /// The drum pattern.
    pub beat: Beat,
    /// Drums come in at this energy.
    pub beat_enter: f32,
    /// Drum level in dB.
    pub beat_db: f32,
    /// Reverb room.
    pub room: &'static str,
    /// Reverb amount for the whole mix (0..1).
    pub wet: f32,
    /// How the piece moves.
    pub arc: Arc,
    /// Lead: smallest note step in beats, rest chance, long-note chance.
    pub lead: (f32, f32, f32),
    /// Timing looseness in milliseconds (0 on a strict grid).
    pub human_ms: f32,
    /// Cadence to the tonic through the dominant at the end.
    pub cadence: bool,
}

use Bass as B;
use Comp as C;
use Role as R;

const MAJ: Q = Q::Maj;
const MIN: Q = Q::Min;

/// Every genre.
pub const STYLES: &[Style] = &[
    Style {
        name: "jazz",
        words: &["jazz", "jazzy", "swing", "bebop", "noir", "big band", "bossa", "smoky", "detective", "bar", "lounge", "blues"],
        bpm: (118.0, 60.0, 200.0),
        meter: 4,
        swing: 0.3,
        major_mode: Mode::Major,
        minor_mode: Mode::Dorian,
        major: &[
            Prog { per: 1, chords: &[(2, Q::Min7), (7, Q::Dom7), (0, Q::Maj7), (9, Q::Dom7)] },
            Prog { per: 1, chords: &[(0, Q::Maj7), (9, Q::Min7), (2, Q::Min7), (7, Q::Dom7)] },
            Prog { per: 1, chords: &[(0, Q::Maj7), (5, Q::Maj7), (4, Q::Min7), (9, Q::Dom7)] },
        ],
        minor: &[
            Prog { per: 1, chords: &[(0, Q::Min7), (5, Q::Min7), (2, Q::Hdim7), (7, Q::Dom7)] },
            Prog { per: 1, chords: &[(2, Q::Hdim7), (7, Q::Dom7), (0, Q::Min7), (8, Q::Maj7)] },
            Prog { per: 1, chords: &[(0, Q::Min6), (5, Q::Min7), (7, Q::Dom7), (0, Q::Min7)] },
        ],
        layers: &[
            layer(Inst::Piano, R::Comp(C::Jazz), 0.0, -4.0, -0.25, 0.25),
            layer(Inst::UprightBass, R::Bass(B::Walking), 0.1, -1.0, 0.0, 0.1),
            layer(Inst::Sax, R::Lead, 0.3, -2.0, 0.22, 0.4),
            layer(Inst::Vibraphone, R::Harmony, 0.85, -12.0, -0.4, 0.5),
        ],
        beat: Beat::Jazz,
        beat_enter: 0.18,
        beat_db: -9.0,
        room: "hall",
        wet: 0.55,
        arc: Arc::Wave,
        lead: (0.5, 0.35, 0.3),
        human_ms: 14.0,
        cadence: true,
    },
    Style {
        name: "folk",
        words: &["folk", "acoustic", "country", "singer-songwriter", "singer songwriter", "bluegrass", "americana", "campfire", "ballad", "indie folk", "pastoral", "countryside"],
        bpm: (98.0, 66.0, 140.0),
        meter: 4,
        swing: 0.0,
        major_mode: Mode::Major,
        minor_mode: Mode::Dorian,
        major: &[
            Prog { per: 1, chords: &[(0, MAJ), (7, MAJ), (9, MIN), (5, MAJ)] },
            Prog { per: 1, chords: &[(0, MAJ), (5, MAJ), (0, MAJ), (7, MAJ)] },
            Prog { per: 1, chords: &[(9, MIN), (5, MAJ), (0, MAJ), (7, MAJ)] },
        ],
        minor: &[
            Prog { per: 1, chords: &[(0, MIN), (8, MAJ), (3, MAJ), (10, MAJ)] },
            Prog { per: 1, chords: &[(0, MIN), (5, MIN), (8, MAJ), (7, MAJ)] },
            Prog { per: 1, chords: &[(0, MIN), (10, MAJ), (5, MIN), (0, MIN)] },
        ],
        layers: &[
            layer(Inst::Guitar, R::Comp(C::Strum), 0.0, -3.0, -0.3, 0.2),
            layer(Inst::UprightBass, R::Bass(B::RootFifth), 0.3, -3.0, 0.0, 0.1),
            layer(Inst::Violin, R::Lead, 0.35, -3.0, 0.3, 0.35),
            layer(Inst::Strings, R::Pad, 0.72, -10.0, 0.0, 0.4),
            layer(Inst::Flute, R::Harmony, 0.88, -10.0, 0.4, 0.4),
        ],
        beat: Beat::Folk,
        beat_enter: 0.4,
        beat_db: -12.0,
        room: "living_room",
        wet: 0.4,
        arc: Arc::Rise,
        lead: (0.5, 0.3, 0.3),
        human_ms: 10.0,
        cadence: true,
    },
    Style {
        name: "cinematic",
        words: &["cinematic", "film", "score", "orchestral", "orchestra", "epic", "trailer", "dramatic", "soundtrack", "heroic", "emotional", "inspiring"],
        bpm: (84.0, 54.0, 130.0),
        meter: 4,
        swing: 0.0,
        major_mode: Mode::Major,
        minor_mode: Mode::Minor,
        major: &[
            Prog { per: 2, chords: &[(0, MAJ), (7, MAJ), (9, MIN), (5, MAJ)] },
            Prog { per: 2, chords: &[(9, MIN), (5, MAJ), (0, MAJ), (7, MAJ)] },
        ],
        minor: &[
            Prog { per: 2, chords: &[(0, MIN), (8, MAJ), (3, MAJ), (10, MAJ)] },
            Prog { per: 2, chords: &[(0, MIN), (5, MIN), (8, MAJ), (7, Q::Dom7)] },
        ],
        layers: &[
            layer(Inst::Strings, R::Pad, 0.0, -4.0, 0.0, 0.5),
            layer(Inst::Cello, R::Bass(B::Pedal), 0.12, -3.0, -0.2, 0.3),
            layer(Inst::Piano, R::Comp(C::Picking), 0.2, -8.0, -0.3, 0.45),
            layer(Inst::Horn, R::Lead, 0.45, -5.0, 0.2, 0.5),
            layer(Inst::Violin, R::Ostinato, 0.6, -11.0, 0.35, 0.4),
            layer(Inst::Choir, R::Pad, 0.86, -12.0, 0.0, 0.6),
        ],
        beat: Beat::Cinematic,
        beat_enter: 0.5,
        beat_db: -8.0,
        room: "hall",
        wet: 0.7,
        arc: Arc::Rise,
        lead: (1.0, 0.3, 0.5),
        human_ms: 6.0,
        cadence: true,
    },
    Style {
        name: "dark",
        words: &["dark", "ominous", "horror", "eerie", "creepy", "suspense", "sinister", "dread", "haunting", "brooding", "menacing", "thriller", "tense", "tension", "slow score", "grim", "bleak"],
        bpm: (62.0, 40.0, 92.0),
        meter: 4,
        swing: 0.0,
        major_mode: Mode::Major,
        minor_mode: Mode::Phrygian,
        major: &[Prog { per: 2, chords: &[(0, Q::Sus2), (5, Q::Sus4), (0, Q::Sus2), (7, Q::Sus4)] }],
        minor: &[
            Prog { per: 2, chords: &[(0, MIN), (1, MAJ), (0, MIN), (6, Q::Dim)] },
            Prog { per: 2, chords: &[(0, MIN), (8, MAJ), (5, MIN), (0, MIN)] },
            Prog { per: 2, chords: &[(0, MIN), (0, MIN), (3, MAJ), (1, MAJ)] },
        ],
        layers: &[
            layer(Inst::Pad, R::Drone, 0.0, -3.0, 0.0, 0.5),
            layer(Inst::Cello, R::Bass(B::Pedal), 0.15, -4.0, -0.2, 0.5),
            layer(Inst::Piano, R::Sparse, 0.22, -7.0, 0.25, 0.6),
            layer(Inst::Bell, R::Sparse, 0.4, -14.0, -0.4, 0.7),
            layer(Inst::Strings, R::Ostinato, 0.55, -12.0, 0.2, 0.5),
            layer(Inst::Choir, R::Pad, 0.82, -12.0, 0.0, 0.7),
        ],
        beat: Beat::Dark,
        beat_enter: 0.45,
        beat_db: -7.0,
        room: "cave",
        wet: 0.8,
        arc: Arc::Rise,
        lead: (1.0, 0.55, 0.55),
        human_ms: 8.0,
        cadence: false,
    },
    Style {
        name: "electronic",
        words: &["electronic", "techno", "house", "edm", "synthwave", "minimal", "dance", "club", "electro", "synth", "idm", "trance", "beat", "grid", "pulse", "tech"],
        bpm: (122.0, 84.0, 160.0),
        meter: 4,
        swing: 0.0,
        major_mode: Mode::Major,
        minor_mode: Mode::Minor,
        major: &[Prog { per: 1, chords: &[(0, MAJ), (7, MAJ), (9, MIN), (5, MAJ)] }, Prog { per: 2, chords: &[(0, Q::Maj7), (5, Q::Maj7)] }],
        minor: &[
            Prog { per: 1, chords: &[(0, MIN), (10, MAJ), (8, MAJ), (10, MAJ)] },
            Prog { per: 2, chords: &[(0, MIN), (0, MIN), (3, MAJ), (10, MAJ)] },
            Prog { per: 2, chords: &[(0, Q::Min7), (8, Q::Maj7)] },
        ],
        layers: &[
            layer(Inst::SynthBass, R::Bass(B::Offbeat), 0.12, -4.0, 0.0, 0.05),
            layer(Inst::SynthPluck, R::Arp, 0.22, -8.0, -0.3, 0.35),
            layer(Inst::Pad, R::Pad, 0.35, -12.0, 0.0, 0.5),
            layer(Inst::SynthLead, R::Lead, 0.62, -8.0, 0.25, 0.4),
            layer(Inst::Bell, R::Sparse, 0.78, -14.0, 0.4, 0.6),
        ],
        beat: Beat::Minimal,
        beat_enter: 0.0,
        beat_db: -5.0,
        room: "plate",
        wet: 0.35,
        arc: Arc::Rise,
        lead: (0.5, 0.4, 0.15),
        human_ms: 0.0,
        cadence: false,
    },
    Style {
        name: "ambient",
        words: &["ambient", "calm", "peaceful", "meditation", "drone", "atmospheric", "ethereal", "dreamy", "floating", "relaxing", "serene", "spa", "soundscape", "new age", "gentle", "quiet"],
        bpm: (66.0, 44.0, 96.0),
        meter: 4,
        swing: 0.0,
        major_mode: Mode::Lydian,
        minor_mode: Mode::Dorian,
        major: &[Prog { per: 2, chords: &[(0, Q::Maj7), (5, Q::Maj7), (9, Q::Min7), (7, Q::Sus4)] }],
        minor: &[Prog { per: 2, chords: &[(0, Q::Min7), (8, Q::Maj7), (3, Q::Maj7), (10, Q::Dom7)] }],
        layers: &[
            layer(Inst::Pad, R::Pad, 0.0, -3.0, 0.0, 0.7),
            layer(Inst::Piano, R::Sparse, 0.25, -8.0, -0.25, 0.7),
            layer(Inst::Strings, R::Pad, 0.5, -10.0, 0.1, 0.7),
            layer(Inst::Flute, R::Lead, 0.6, -9.0, 0.3, 0.7),
            layer(Inst::Bell, R::Sparse, 0.7, -14.0, 0.4, 0.8),
        ],
        beat: Beat::None,
        beat_enter: 2.0,
        beat_db: -12.0,
        room: "hall",
        wet: 0.85,
        arc: Arc::Wave,
        lead: (1.0, 0.6, 0.6),
        human_ms: 12.0,
        cadence: false,
    },
    Style {
        name: "lofi",
        words: &["lofi", "lo-fi", "lo fi", "chill", "chillhop", "hip hop", "hip-hop", "study", "boom bap", "mellow", "downtempo", "trip hop", "cafe", "coffee shop"],
        bpm: (80.0, 60.0, 98.0),
        meter: 4,
        swing: 0.2,
        major_mode: Mode::Major,
        minor_mode: Mode::Dorian,
        major: &[Prog { per: 1, chords: &[(2, Q::Min7), (7, Q::Dom7), (0, Q::Maj7), (9, Q::Min7)] }, Prog { per: 1, chords: &[(0, Q::Maj7), (9, Q::Min7), (5, Q::Maj7), (7, Q::Dom7)] }],
        minor: &[Prog { per: 1, chords: &[(0, Q::Min7), (5, Q::Min7), (10, Q::Dom7), (3, Q::Maj7)] }, Prog { per: 1, chords: &[(0, Q::Min7), (8, Q::Maj7), (3, Q::Maj7), (7, Q::Dom7)] }],
        layers: &[
            layer(Inst::ElectricPiano, R::Comp(C::Jazz), 0.0, -3.0, -0.2, 0.3),
            layer(Inst::ElectricBass, R::Bass(B::Lazy), 0.15, -2.0, 0.0, 0.05),
            layer(Inst::Guitar, R::Lead, 0.45, -6.0, 0.3, 0.4),
            layer(Inst::Vibraphone, R::Harmony, 0.8, -12.0, -0.3, 0.5),
        ],
        beat: Beat::Lofi,
        beat_enter: 0.1,
        beat_db: -8.0,
        room: "small_room",
        wet: 0.35,
        arc: Arc::Steady,
        lead: (0.5, 0.45, 0.3),
        human_ms: 16.0,
        cadence: true,
    },
    Style {
        name: "rock",
        words: &["rock", "punk", "indie", "garage", "grunge", "alternative", "band", "driving", "guitar rock", "metal", "anthem"],
        bpm: (124.0, 84.0, 180.0),
        meter: 4,
        swing: 0.0,
        major_mode: Mode::Mixolydian,
        minor_mode: Mode::Minor,
        major: &[Prog { per: 1, chords: &[(0, MAJ), (5, MAJ), (7, MAJ), (5, MAJ)] }, Prog { per: 1, chords: &[(0, MAJ), (7, MAJ), (9, MIN), (5, MAJ)] }],
        minor: &[Prog { per: 1, chords: &[(0, MIN), (10, MAJ), (8, MAJ), (7, MAJ)] }, Prog { per: 1, chords: &[(0, MIN), (8, MAJ), (10, MAJ), (0, MIN)] }],
        layers: &[
            layer(Inst::Guitar, R::Comp(C::Strum), 0.0, -3.0, -0.4, 0.15),
            layer(Inst::ElectricBass, R::Bass(B::Rock), 0.1, -2.0, 0.0, 0.05),
            layer(Inst::Organ, R::Pad, 0.5, -12.0, 0.2, 0.3),
            layer(Inst::Guitar, R::Lead, 0.4, -4.0, 0.4, 0.3),
        ],
        beat: Beat::Rock,
        beat_enter: 0.08,
        beat_db: -5.0,
        room: "warehouse",
        wet: 0.3,
        arc: Arc::Rise,
        lead: (0.5, 0.3, 0.25),
        human_ms: 7.0,
        cadence: true,
    },
    Style {
        name: "piano",
        words: &["piano", "classical", "baroque", "romantic", "solo piano", "nocturne", "lullaby", "intimate", "sentimental", "tender", "wistful", "melancholy", "melancholic", "sad", "nostalgic", "reflective"],
        bpm: (76.0, 48.0, 116.0),
        meter: 4,
        swing: 0.0,
        major_mode: Mode::Major,
        minor_mode: Mode::HarmonicMinor,
        major: &[
            Prog { per: 1, chords: &[(0, MAJ), (7, MAJ), (9, MIN), (4, MIN), (5, MAJ), (0, MAJ), (5, MAJ), (7, Q::Dom7)] },
            Prog { per: 1, chords: &[(0, MAJ), (9, MIN), (5, MAJ), (7, MAJ)] },
        ],
        minor: &[
            Prog { per: 1, chords: &[(0, MIN), (10, MAJ), (8, MAJ), (7, Q::Dom7)] },
            Prog { per: 1, chords: &[(0, MIN), (5, MIN), (7, Q::Dom7), (0, MIN)] },
        ],
        layers: &[
            layer(Inst::Piano, R::Comp(C::Block), 0.0, -3.0, -0.1, 0.4),
            layer(Inst::Piano, R::Lead, 0.3, -2.0, 0.15, 0.45),
            layer(Inst::Cello, R::Bass(B::Pedal), 0.55, -7.0, -0.3, 0.4),
            layer(Inst::Strings, R::Pad, 0.72, -11.0, 0.0, 0.5),
        ],
        beat: Beat::None,
        beat_enter: 2.0,
        beat_db: -12.0,
        room: "hall",
        wet: 0.6,
        arc: Arc::Wave,
        lead: (0.5, 0.35, 0.4),
        human_ms: 12.0,
        cadence: true,
    },
    Style {
        name: "pop",
        words: &["pop", "upbeat", "catchy", "happy", "bright", "commercial", "radio", "uplifting", "feel good", "feel-good", "cheerful", "summer", "vlog", "corporate"],
        bpm: (112.0, 88.0, 138.0),
        meter: 4,
        swing: 0.0,
        major_mode: Mode::Major,
        minor_mode: Mode::Minor,
        major: &[Prog { per: 1, chords: &[(0, MAJ), (7, MAJ), (9, MIN), (5, MAJ)] }, Prog { per: 1, chords: &[(9, MIN), (5, MAJ), (0, MAJ), (7, MAJ)] }],
        minor: &[Prog { per: 1, chords: &[(0, MIN), (8, MAJ), (3, MAJ), (10, MAJ)] }, Prog { per: 1, chords: &[(0, MIN), (10, MAJ), (8, MAJ), (10, MAJ)] }],
        layers: &[
            layer(Inst::Piano, R::Comp(C::Block), 0.0, -4.0, -0.25, 0.25),
            layer(Inst::ElectricBass, R::Bass(B::Pulse), 0.15, -3.0, 0.0, 0.05),
            layer(Inst::Guitar, R::Comp(C::Strum), 0.3, -8.0, -0.4, 0.2),
            layer(Inst::SynthLead, R::Lead, 0.45, -6.0, 0.25, 0.35),
            layer(Inst::Strings, R::Pad, 0.75, -11.0, 0.0, 0.4),
            layer(Inst::Bell, R::Sparse, 0.7, -14.0, 0.4, 0.5),
        ],
        beat: Beat::Pop,
        beat_enter: 0.2,
        beat_db: -6.0,
        room: "plate",
        wet: 0.35,
        arc: Arc::Rise,
        lead: (0.5, 0.3, 0.2),
        human_ms: 4.0,
        cadence: true,
    },
    Style {
        name: "playful",
        words: &["playful", "quirky", "comedy", "comic", "funny", "whimsical", "circus", "cartoon", "ragtime", "silly", "bouncy", "march", "kids", "children", "toy", "sneaky", "mischievous", "caper"],
        bpm: (126.0, 90.0, 168.0),
        meter: 4,
        swing: 0.12,
        major_mode: Mode::Major,
        minor_mode: Mode::HarmonicMinor,
        major: &[
            Prog { per: 1, chords: &[(0, MAJ), (9, Q::Dom7), (2, Q::Dom7), (7, Q::Dom7)] },
            Prog { per: 1, chords: &[(0, MAJ), (5, MAJ), (7, Q::Dom7), (0, MAJ)] },
        ],
        minor: &[Prog { per: 1, chords: &[(0, MIN), (7, Q::Dom7), (0, MIN), (7, Q::Dom7)] }, Prog { per: 1, chords: &[(0, MIN), (5, MIN), (7, Q::Dom7), (0, MIN)] }],
        layers: &[
            layer(Inst::Piano, R::Comp(C::OomPah), 0.0, -4.0, -0.2, 0.15),
            layer(Inst::UprightBass, R::Bass(B::RootFifth), 0.1, -3.0, 0.0, 0.05),
            layer(Inst::Clarinet, R::Lead, 0.3, -3.0, 0.25, 0.25),
            layer(Inst::Marimba, R::Harmony, 0.6, -9.0, -0.35, 0.3),
            layer(Inst::Trombone, R::Pad, 0.8, -10.0, 0.0, 0.3),
        ],
        beat: Beat::Playful,
        beat_enter: 0.15,
        beat_db: -9.0,
        room: "small_room",
        wet: 0.25,
        arc: Arc::Wave,
        lead: (0.5, 0.25, 0.15),
        human_ms: 8.0,
        cadence: true,
    },
];

/// Words that hint at a genre only weakly (they also describe moods or
/// settings), so a strong word elsewhere in the description wins.
const WEAK: &[&str] = &[
    "score", "beat", "band", "bar", "bright", "happy", "synth", "gentle", "quiet", "tense", "tension", "sad", "melancholy", "melancholic", "grim", "bleak", "cheerful", "upbeat", "catchy", "nostalgic", "reflective", "wistful", "emotional", "inspiring", "sentimental", "tender", "summer", "calm", "grid", "pulse", "tech", "kids", "children", "toy", "fun", "uplifting", "peaceful", "warm", "light", "dramatic", "intimate", "driving", "dance", "blues", "ballad", "heroic", "romantic", "minimal", "dreamy", "slow score", "suspense", "haunting", "brooding",
];

/// The genre named in some words: the earliest strong word wins, then the
/// earliest weak one.
pub fn find_style(text: &str) -> Option<&'static Style> {
    let t = format!(" {} ", text.to_lowercase().replace([',', '.', ';', ':', '(', ')', '/'], " "));
    let mut best: Option<(&Style, u8, usize)> = None;
    for s in STYLES {
        for w in s.words {
            let found = t.find(&format!(" {w} ")).or_else(|| (w.contains('-') || w.contains(' ')).then(|| t.find(*w)).flatten());
            if let Some(pos) = found {
                let strength = if WEAK.contains(w) { 0 } else { 1 };
                let better = match best {
                    None => true,
                    Some((_, st, p)) => strength > st || (strength == st && pos < p),
                };
                if better {
                    best = Some((s, strength, pos));
                }
            }
        }
    }
    best.map(|(s, _, _)| s)
}

/// Looks a genre up by its exact name.
pub fn style_named(name: &str) -> Option<&'static Style> {
    STYLES.iter().find(|s| s.name == name)
}

/// What a mood does to the music.
#[derive(Clone, Copy, Debug, Default)]
pub struct Mood {
    /// Minor (`Some(true)`) or major (`Some(false)`) feel.
    pub minor: Option<bool>,
    /// Multiplies the tempo.
    pub tempo: f32,
    /// The shape of the piece.
    pub arc: Option<Arc>,
    /// Ceiling on the energy (0..1).
    pub energy: f32,
    /// Thinner, with more space.
    pub sparse: bool,
    /// Swaps the drums for brushes (noir).
    pub brushes: bool,
}

/// Reads the mood words in a description.
pub fn mood_of(text: &str) -> Mood {
    let t = format!(" {} ", text.to_lowercase().replace([',', '.', ';', ':', '-'], " "));
    let has = |w: &str| t.contains(&format!(" {w} "));
    let any = |ws: &[&str]| ws.iter().any(|w| has(w));
    let mut m = Mood { minor: None, tempo: 1.0, arc: None, energy: 1.0, sparse: false, brushes: false };
    if any(&["dark", "sad", "melancholy", "melancholic", "ominous", "tense", "tension", "noir", "mysterious", "haunting", "eerie", "sombre", "somber", "gloomy", "brooding", "suspense", "suspenseful", "creepy", "grim", "bleak", "lonely", "sinister", "wistful", "sorrowful", "tragic"]) {
        m.minor = Some(true);
    }
    if any(&["hopeful", "happy", "bright", "cheerful", "uplifting", "joyful", "optimistic", "triumphant", "warm", "sunny", "playful", "peaceful", "fun", "positive", "heroic", "inspiring", "light"]) {
        m.minor = Some(false);
    }
    if any(&["slow", "slowly", "lazy", "relaxed", "relaxing", "sleepy", "languid", "unhurried", "slow burn"]) {
        m.tempo *= 0.74;
    }
    if any(&["calm", "gentle", "soft", "peaceful", "quiet"]) {
        m.tempo *= 0.88;
        m.energy = 0.75;
    }
    if any(&["fast", "quick", "frantic", "urgent", "racing", "energetic", "driving", "chase", "lively"]) {
        m.tempo *= 1.25;
    }
    if any(&["upbeat", "bouncy", "brisk"]) {
        m.tempo *= 1.1;
    }
    if any(&["noir", "smoky", "late night", "rainy", "detective"]) {
        m.tempo *= 0.65;
        m.brushes = true;
        m.minor = Some(true);
        m.energy = m.energy.min(0.8);
    }
    if any(&["build", "builds", "building", "rising", "growing", "crescendo", "rise", "hopeful", "tense", "suspense", "tension", "swelling"]) {
        m.arc = Some(Arc::Rise);
    }
    if any(&["fading", "fade", "winding", "outro", "dying", "settling"]) {
        m.arc = Some(Arc::Fall);
    }
    if any(&["steady", "constant", "loop", "background", "underscore", "bed"]) {
        m.arc = Some(Arc::Steady);
    }
    if any(&["sparse", "minimal", "minimalist", "spacious", "empty", "restrained", "subtle"]) {
        m.sparse = true;
    }
    if any(&["dark", "slow", "sparse", "subdued", "quiet", "restrained"]) {
        m.energy = m.energy.min(0.8);
    }
    if any(&["epic", "huge", "massive", "big", "powerful", "grand", "intense"]) {
        m.energy = 1.0;
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn genres_are_found_from_words() {
        assert_eq!(find_style("noir jazz with sax and double bass").unwrap().name, "jazz");
        assert_eq!(find_style("hopeful acoustic folk").unwrap().name, "folk");
        assert_eq!(find_style("dark slow score").unwrap().name, "dark");
        assert_eq!(find_style("minimal electronic on a beat grid").unwrap().name, "electronic");
        assert_eq!(find_style("a lofi hip hop beat").unwrap().name, "lofi");
        assert!(find_style("xyzzy").is_none());
    }

    #[test]
    fn moods_are_read() {
        let m = mood_of("hopeful acoustic, slow build");
        assert_eq!(m.minor, Some(false));
        assert!(m.tempo < 0.8);
        assert_eq!(m.arc, Some(Arc::Rise));
        assert_eq!(mood_of("dark slow score").minor, Some(true));
    }

    #[test]
    fn every_style_is_complete() {
        for s in STYLES {
            assert!(!s.major.is_empty() && !s.minor.is_empty() && !s.layers.is_empty(), "{}", s.name);
            assert!(s.bpm.1 <= s.bpm.0 && s.bpm.0 <= s.bpm.2);
            assert!(studio_sound::reverb::ReverbParams::room(s.room).is_some(), "{}: {}", s.name, s.room);
        }
    }
}
