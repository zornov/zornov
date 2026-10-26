// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Voice styles: ready-made chains of the crate's processors that turn a
//! plain voice recording into a character or a place. Every style is built
//! from the units in this crate (pitch shift, EQ, dynamics, reverb, delay)
//! plus the small colour effects defined here (ring modulation, saturation,
//! tremolo, vibrato, chorus, comb, decimation, noise, whisper). Names,
//! groupings and settings are our own.
//!
//! Build a chain with [`VoiceStyle::build`]; it is an [`Effect`] like any
//! other and runs allocation-free. [`VoiceStyle::ALL`] lists the catalogue
//! for a picker, grouped by [`VoiceGroup`].

use crate::buffer::{Effect, db_to_gain};
use crate::delay::{DelayParams, StereoDelay};
use crate::dynamics::{Compressor, CompressorParams, Gate, GateParams};
use crate::eq::{Band, BandKind, Biquad, ParametricEq};
use crate::pitch::PitchShifter;
use crate::reverb::{Reverb, ReverbParams};

/// A group in the voice picker.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum VoiceGroup {
    /// Bigger, smaller, older, younger: pitch and body changes.
    Character,
    /// Rooms and halls.
    Place,
    /// Phones, radios, speakers and tape.
    Device,
    /// Robots, aliens and machines.
    Synthetic,
    /// Dreamy, scary, playful treatments.
    Mood,
    /// Clean enhancement for narration and podcasts.
    Polish,
}

impl VoiceGroup {
    /// Picker label.
    pub fn label(self) -> &'static str {
        match self {
            VoiceGroup::Character => "Characters",
            VoiceGroup::Place => "Places",
            VoiceGroup::Device => "Devices",
            VoiceGroup::Synthetic => "Synthetic",
            VoiceGroup::Mood => "Moods",
            VoiceGroup::Polish => "Polish",
        }
    }
}

/// The catalogue of voice styles.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[allow(missing_docs)]
pub enum VoiceStyle {
    // Characters
    Squirrel,
    Pixie,
    Youngster,
    Lumberjack,
    Giant,
    Villain,
    Elder,
    Twins,
    // Places
    Stairwell,
    Chapel,
    Cathedral,
    CaveMouth,
    Arena,
    Tunnel,
    Bathroom,
    // Devices
    CrackleLine,
    TinCanRadio,
    Walkie,
    Bullhorn,
    TinySpeaker,
    Cassette,
    VinylBooth,
    // Synthetic
    Clockwork,
    ToyRobot,
    Satellite,
    HiveMind,
    Glitch,
    // Moods
    DreamHaze,
    Phantom,
    DeepSea,
    WhisperWind,
    Nightline,
    // Polish
    Newsroom,
    PodcastWarm,
    StadiumAnnouncer,
}

impl VoiceStyle {
    /// Every style in picker order.
    pub const ALL: [VoiceStyle; 35] = [
        VoiceStyle::Squirrel,
        VoiceStyle::Pixie,
        VoiceStyle::Youngster,
        VoiceStyle::Lumberjack,
        VoiceStyle::Giant,
        VoiceStyle::Villain,
        VoiceStyle::Elder,
        VoiceStyle::Twins,
        VoiceStyle::Stairwell,
        VoiceStyle::Chapel,
        VoiceStyle::Cathedral,
        VoiceStyle::CaveMouth,
        VoiceStyle::Arena,
        VoiceStyle::Tunnel,
        VoiceStyle::Bathroom,
        VoiceStyle::CrackleLine,
        VoiceStyle::TinCanRadio,
        VoiceStyle::Walkie,
        VoiceStyle::Bullhorn,
        VoiceStyle::TinySpeaker,
        VoiceStyle::Cassette,
        VoiceStyle::VinylBooth,
        VoiceStyle::Clockwork,
        VoiceStyle::ToyRobot,
        VoiceStyle::Satellite,
        VoiceStyle::HiveMind,
        VoiceStyle::Glitch,
        VoiceStyle::DreamHaze,
        VoiceStyle::Phantom,
        VoiceStyle::DeepSea,
        VoiceStyle::WhisperWind,
        VoiceStyle::Nightline,
        VoiceStyle::Newsroom,
        VoiceStyle::PodcastWarm,
        VoiceStyle::StadiumAnnouncer,
    ];

    /// Stable identifier for project files.
    pub fn id(self) -> &'static str {
        match self {
            VoiceStyle::Squirrel => "squirrel",
            VoiceStyle::Pixie => "pixie",
            VoiceStyle::Youngster => "youngster",
            VoiceStyle::Lumberjack => "lumberjack",
            VoiceStyle::Giant => "giant",
            VoiceStyle::Villain => "villain",
            VoiceStyle::Elder => "elder",
            VoiceStyle::Twins => "twins",
            VoiceStyle::Stairwell => "stairwell",
            VoiceStyle::Chapel => "chapel",
            VoiceStyle::Cathedral => "cathedral",
            VoiceStyle::CaveMouth => "cave_mouth",
            VoiceStyle::Arena => "arena",
            VoiceStyle::Tunnel => "tunnel",
            VoiceStyle::Bathroom => "bathroom",
            VoiceStyle::CrackleLine => "crackle_line",
            VoiceStyle::TinCanRadio => "tin_can_radio",
            VoiceStyle::Walkie => "walkie",
            VoiceStyle::Bullhorn => "bullhorn",
            VoiceStyle::TinySpeaker => "tiny_speaker",
            VoiceStyle::Cassette => "cassette",
            VoiceStyle::VinylBooth => "vinyl_booth",
            VoiceStyle::Clockwork => "clockwork",
            VoiceStyle::ToyRobot => "toy_robot",
            VoiceStyle::Satellite => "satellite",
            VoiceStyle::HiveMind => "hive_mind",
            VoiceStyle::Glitch => "glitch",
            VoiceStyle::DreamHaze => "dream_haze",
            VoiceStyle::Phantom => "phantom",
            VoiceStyle::DeepSea => "deep_sea",
            VoiceStyle::WhisperWind => "whisper_wind",
            VoiceStyle::Nightline => "nightline",
            VoiceStyle::Newsroom => "newsroom",
            VoiceStyle::PodcastWarm => "podcast_warm",
            VoiceStyle::StadiumAnnouncer => "stadium_announcer",
        }
    }

    /// Looks a style up by its [`id`](VoiceStyle::id).
    pub fn from_id(id: &str) -> Option<VoiceStyle> {
        VoiceStyle::ALL.iter().copied().find(|s| s.id() == id)
    }

    /// Picker name.
    pub fn name(self) -> &'static str {
        match self {
            VoiceStyle::Squirrel => "Squirrel",
            VoiceStyle::Pixie => "Pixie",
            VoiceStyle::Youngster => "Youngster",
            VoiceStyle::Lumberjack => "Lumberjack",
            VoiceStyle::Giant => "Giant",
            VoiceStyle::Villain => "Villain",
            VoiceStyle::Elder => "Elder",
            VoiceStyle::Twins => "Twins",
            VoiceStyle::Stairwell => "Stairwell",
            VoiceStyle::Chapel => "Chapel",
            VoiceStyle::Cathedral => "Cathedral",
            VoiceStyle::CaveMouth => "Cave mouth",
            VoiceStyle::Arena => "Arena",
            VoiceStyle::Tunnel => "Tunnel",
            VoiceStyle::Bathroom => "Bathroom",
            VoiceStyle::CrackleLine => "Crackle line",
            VoiceStyle::TinCanRadio => "Tin can radio",
            VoiceStyle::Walkie => "Walkie",
            VoiceStyle::Bullhorn => "Bullhorn",
            VoiceStyle::TinySpeaker => "Tiny speaker",
            VoiceStyle::Cassette => "Cassette",
            VoiceStyle::VinylBooth => "Vinyl booth",
            VoiceStyle::Clockwork => "Clockwork",
            VoiceStyle::ToyRobot => "Toy robot",
            VoiceStyle::Satellite => "Satellite",
            VoiceStyle::HiveMind => "Hive mind",
            VoiceStyle::Glitch => "Glitch",
            VoiceStyle::DreamHaze => "Dream haze",
            VoiceStyle::Phantom => "Phantom",
            VoiceStyle::DeepSea => "Deep sea",
            VoiceStyle::WhisperWind => "Whisper wind",
            VoiceStyle::Nightline => "Nightline",
            VoiceStyle::Newsroom => "Newsroom",
            VoiceStyle::PodcastWarm => "Podcast warm",
            VoiceStyle::StadiumAnnouncer => "Stadium announcer",
        }
    }

    /// One-line description for the picker.
    pub fn description(self) -> &'static str {
        match self {
            VoiceStyle::Squirrel => "Tiny and fast-talking, an octave up",
            VoiceStyle::Pixie => "Bright and sparkly, a fifth up with a shimmer",
            VoiceStyle::Youngster => "A few years younger: slightly higher and lighter",
            VoiceStyle::Lumberjack => "Big chest voice, a fourth down",
            VoiceStyle::Giant => "An octave down with a slow, heavy room",
            VoiceStyle::Villain => "Lower, darker, with a rasp and a cold hall",
            VoiceStyle::Elder => "Thinner and a little unsteady",
            VoiceStyle::Twins => "Two of you, slightly detuned",
            VoiceStyle::Stairwell => "Hard concrete echoes",
            VoiceStyle::Chapel => "A warm wooden room with a short tail",
            VoiceStyle::Cathedral => "Long stone reverb",
            VoiceStyle::CaveMouth => "Dark, dripping, distant echoes",
            VoiceStyle::Arena => "Stadium slap-back and crowd-sized space",
            VoiceStyle::Tunnel => "Metallic resonance, close and tight",
            VoiceStyle::Bathroom => "Tiled, bright and small",
            VoiceStyle::CrackleLine => "Old telephone: narrow band and line crackle",
            VoiceStyle::TinCanRadio => "AM radio through a tiny speaker",
            VoiceStyle::Walkie => "Handheld radio with squelch and clipping",
            VoiceStyle::Bullhorn => "Loud, honky and distorted",
            VoiceStyle::TinySpeaker => "Phone speaker: no bass, little top",
            VoiceStyle::Cassette => "Tape warmth with hiss and wobble",
            VoiceStyle::VinylBooth => "Dusty record with crackle and a soft top",
            VoiceStyle::Clockwork => "Classic metallic robot",
            VoiceStyle::ToyRobot => "Low-bit, low-rate toy voice box",
            VoiceStyle::Satellite => "Thin, flanged and drifting from orbit",
            VoiceStyle::HiveMind => "Many voices speaking as one",
            VoiceStyle::Glitch => "Stuttering digital breakup",
            VoiceStyle::DreamHaze => "Soft, wide and washed out",
            VoiceStyle::Phantom => "Ghostly tremolo in a cold space",
            VoiceStyle::DeepSea => "Muffled and slow, as if underwater",
            VoiceStyle::WhisperWind => "Breathy whisper with air around it",
            VoiceStyle::Nightline => "Late-night radio host: close and smooth",
            VoiceStyle::Newsroom => "Clean, present and consistent",
            VoiceStyle::PodcastWarm => "Rich low-mids and gentle levelling",
            VoiceStyle::StadiumAnnouncer => "Punchy, bright and huge",
        }
    }

    /// The style's group.
    pub fn group(self) -> VoiceGroup {
        use VoiceStyle::*;
        match self {
            Squirrel | Pixie | Youngster | Lumberjack | Giant | Villain | Elder | Twins => VoiceGroup::Character,
            Stairwell | Chapel | Cathedral | CaveMouth | Arena | Tunnel | Bathroom => VoiceGroup::Place,
            CrackleLine | TinCanRadio | Walkie | Bullhorn | TinySpeaker | Cassette | VinylBooth => VoiceGroup::Device,
            Clockwork | ToyRobot | Satellite | HiveMind | Glitch => VoiceGroup::Synthetic,
            DreamHaze | Phantom | DeepSea | WhisperWind | Nightline => VoiceGroup::Mood,
            Newsroom | PodcastWarm | StadiumAnnouncer => VoiceGroup::Polish,
        }
    }

    /// Builds the processing chain for `rate` and blocks up to `max_block`.
    /// Allocates here; the chain itself runs allocation-free.
    pub fn build(self, rate: u32, max_block: usize) -> VoiceChain {
        let mut c = VoiceChain::new();
        use VoiceStyle::*;
        match self {
            Squirrel => {
                c.push(PitchShifter::new(rate, 12.0, max_block));
                c.push(eq(rate, &[Band::new(BandKind::HighPass, 250.0, 0.707, 0.0), Band::bell(3500.0, 1.0, 3.0)]));
            }
            Pixie => {
                c.push(PitchShifter::new(rate, 7.0, max_block));
                c.push(Chorus::new(rate, 0.8, 4.0, 0.35));
                c.push(eq(rate, &[Band::new(BandKind::HighShelf, 6000.0, 0.707, 4.0)]));
                c.push(Reverb::new(rate, ReverbParams { mix: 0.15, size: 0.8, decay: 0.45, ..ReverbParams::PLATE }));
            }
            Youngster => {
                c.push(PitchShifter::new(rate, 3.0, max_block));
                c.push(eq(rate, &[Band::new(BandKind::LowShelf, 250.0, 0.707, -3.0), Band::bell(2500.0, 1.0, 2.0)]));
            }
            Lumberjack => {
                c.push(PitchShifter::new(rate, -5.0, max_block));
                c.push(eq(rate, &[Band::new(BandKind::LowShelf, 180.0, 0.707, 4.0), Band::bell(2000.0, 1.2, -2.0)]));
                c.push(Compressor::new(rate, CompressorParams { threshold_db: -18.0, ratio: 3.0, ..CompressorParams::VOICE }));
            }
            Giant => {
                c.push(PitchShifter::new(rate, -12.0, max_block));
                c.push(eq(rate, &[Band::new(BandKind::LowShelf, 150.0, 0.707, 5.0), Band::new(BandKind::LowPass, 6000.0, 0.707, 0.0)]));
                c.push(Reverb::new(rate, ReverbParams { pre_delay_ms: 40.0, size: 1.8, decay: 0.7, damping: 0.6, mix: 0.25, ..ReverbParams::HALL }));
            }
            Villain => {
                c.push(PitchShifter::new(rate, -4.0, max_block));
                c.push(Saturation::new(rate, 2.5, 5000.0));
                c.push(eq(rate, &[Band::new(BandKind::LowShelf, 200.0, 0.707, 3.0), Band::bell(800.0, 1.5, -3.0)]));
                c.push(Reverb::new(rate, ReverbParams { pre_delay_ms: 30.0, size: 1.5, decay: 0.72, damping: 0.5, low_cut_hz: 200.0, high_cut_hz: 6000.0, mix: 0.22, ..ReverbParams::HALL }));
            }
            Elder => {
                c.push(PitchShifter::new(rate, -1.0, max_block));
                c.push(Vibrato::new(rate, 5.5, 0.6, 1.0));
                c.push(eq(rate, &[Band::new(BandKind::HighPass, 300.0, 0.707, 0.0), Band::bell(1800.0, 1.0, 3.0), Band::new(BandKind::HighShelf, 5000.0, 0.707, -4.0)]));
            }
            Twins => {
                c.push(Harmony::new(rate, max_block, &[(0.25, 0.0), (-0.25, 0.0)], 0.5));
            }
            Stairwell => {
                c.push(StereoDelay::new(rate, DelayParams { time_left_ms: 90.0, time_right_ms: 130.0, feedback: 0.45, ping_pong: true, high_cut_hz: 5000.0, low_cut_hz: 200.0, mix: 0.35 }));
                c.push(Reverb::new(rate, ReverbParams { pre_delay_ms: 5.0, size: 0.7, decay: 0.5, damping: 0.2, mix: 0.2, ..ReverbParams::ROOM }));
            }
            Chapel => {
                c.push(Reverb::new(rate, ReverbParams { pre_delay_ms: 15.0, size: 1.1, decay: 0.6, damping: 0.5, low_cut_hz: 150.0, high_cut_hz: 8000.0, mix: 0.3, ..ReverbParams::HALL }));
            }
            Cathedral => {
                c.push(Reverb::new(rate, ReverbParams { pre_delay_ms: 40.0, size: 2.0, decay: 0.85, damping: 0.3, low_cut_hz: 120.0, high_cut_hz: 9000.0, mix: 0.4, ..ReverbParams::HALL }));
            }
            CaveMouth => {
                c.push(eq(rate, &[Band::new(BandKind::LowPass, 3500.0, 0.707, 0.0)]));
                c.push(StereoDelay::new(rate, DelayParams { time_left_ms: 320.0, time_right_ms: 470.0, feedback: 0.5, ping_pong: false, high_cut_hz: 2500.0, low_cut_hz: 100.0, mix: 0.4 }));
                c.push(Reverb::new(rate, ReverbParams { pre_delay_ms: 60.0, size: 1.9, decay: 0.8, damping: 0.7, high_cut_hz: 4000.0, mix: 0.35, ..ReverbParams::HALL }));
            }
            Arena => {
                c.push(eq(rate, &[Band::new(BandKind::HighPass, 150.0, 0.707, 0.0), Band::bell(2500.0, 1.0, 3.0)]));
                c.push(StereoDelay::new(rate, DelayParams { time_left_ms: 240.0, time_right_ms: 240.0, feedback: 0.3, ping_pong: false, high_cut_hz: 4000.0, low_cut_hz: 200.0, mix: 0.3 }));
                c.push(Reverb::new(rate, ReverbParams { pre_delay_ms: 80.0, size: 2.0, decay: 0.75, damping: 0.5, mix: 0.3, ..ReverbParams::HALL }));
            }
            Tunnel => {
                c.push(Comb::new(rate, 110.0, 0.6, 0.5));
                c.push(Reverb::new(rate, ReverbParams { pre_delay_ms: 8.0, size: 0.6, decay: 0.55, damping: 0.1, mix: 0.25, ..ReverbParams::ROOM }));
            }
            Bathroom => {
                c.push(eq(rate, &[Band::bell(3000.0, 1.0, 3.0)]));
                c.push(Reverb::new(rate, ReverbParams { pre_delay_ms: 3.0, size: 0.4, decay: 0.45, damping: 0.05, diffusion: 0.5, mix: 0.3, ..ReverbParams::ROOM }));
            }
            CrackleLine => {
                c.push(eq(rate, &[Band::new(BandKind::HighPass, 320.0, 1.0, 0.0), Band::new(BandKind::LowPass, 3300.0, 1.0, 0.0), Band::bell(1500.0, 1.0, 4.0)]));
                c.push(Saturation::new(rate, 2.0, 4000.0));
                c.push(Noise::new(rate, NoiseKind::Crackle, -32.0, 500.0));
                c.push(eq(rate, &[Band::new(BandKind::HighPass, 320.0, 1.0, 0.0), Band::new(BandKind::LowPass, 3300.0, 1.0, 0.0)]));
            }
            TinCanRadio => {
                c.push(eq(rate, &[Band::new(BandKind::HighPass, 400.0, 1.2, 0.0), Band::new(BandKind::LowPass, 4500.0, 1.2, 0.0), Band::bell(1200.0, 2.0, 6.0)]));
                c.push(Saturation::new(rate, 3.0, 4500.0));
                c.push(Noise::new(rate, NoiseKind::Hiss, -40.0, 1000.0));
                c.push(Compressor::new(rate, CompressorParams { threshold_db: -24.0, ratio: 6.0, attack_ms: 2.0, release_ms: 80.0, auto_makeup: true, ..Default::default() }));
            }
            Walkie => {
                c.push(Gate::new(rate, GateParams { threshold_db: -38.0, range_db: 80.0, attack_ms: 0.5, hold_ms: 60.0, release_ms: 30.0, ..Default::default() }));
                c.push(eq(rate, &[Band::new(BandKind::HighPass, 500.0, 1.0, 0.0), Band::new(BandKind::LowPass, 3000.0, 1.0, 0.0)]));
                c.push(Saturation::new(rate, 5.0, 3500.0));
                c.push(Noise::new(rate, NoiseKind::Hiss, -36.0, 800.0));
            }
            Bullhorn => {
                c.push(eq(rate, &[Band::new(BandKind::HighPass, 450.0, 1.0, 0.0), Band::bell(1800.0, 1.5, 8.0), Band::new(BandKind::LowPass, 5000.0, 1.0, 0.0)]));
                c.push(Saturation::new(rate, 6.0, 5000.0));
                c.push(Compressor::new(rate, CompressorParams { threshold_db: -20.0, ratio: 8.0, attack_ms: 1.0, release_ms: 60.0, ..Default::default() }));
            }
            TinySpeaker => {
                c.push(eq(rate, &[Band::new(BandKind::HighPass, 600.0, 0.9, 0.0), Band::bell(2200.0, 1.5, 4.0), Band::new(BandKind::LowPass, 7000.0, 0.8, 0.0)]));
                c.push(Saturation::new(rate, 1.5, 8000.0));
            }
            Cassette => {
                c.push(Vibrato::new(rate, 0.6, 0.35, 1.0));
                c.push(Saturation::new(rate, 1.6, 9000.0));
                c.push(eq(rate, &[Band::new(BandKind::LowShelf, 120.0, 0.707, 2.0), Band::new(BandKind::HighShelf, 7000.0, 0.707, -6.0)]));
                c.push(Noise::new(rate, NoiseKind::Hiss, -46.0, 2000.0));
            }
            VinylBooth => {
                c.push(eq(rate, &[Band::new(BandKind::HighPass, 80.0, 0.707, 0.0), Band::new(BandKind::HighShelf, 6000.0, 0.707, -8.0)]));
                c.push(Noise::new(rate, NoiseKind::Crackle, -34.0, 300.0));
                c.push(Reverb::new(rate, ReverbParams { size: 0.4, decay: 0.3, mix: 0.12, ..ReverbParams::ROOM }));
            }
            Clockwork => {
                c.push(RingMod::new(rate, 55.0, 0.9));
                c.push(Comb::new(rate, 220.0, 0.75, 0.6));
                c.push(eq(rate, &[Band::new(BandKind::HighPass, 200.0, 0.707, 0.0), Band::bell(2000.0, 1.0, 3.0)]));
            }
            ToyRobot => {
                c.push(Decimate::new(rate, 8000.0, 8));
                c.push(RingMod::new(rate, 110.0, 0.6));
                c.push(eq(rate, &[Band::new(BandKind::LowPass, 4000.0, 1.0, 0.0)]));
            }
            Satellite => {
                c.push(PitchShifter::new(rate, 2.0, max_block));
                c.push(Chorus::new(rate, 0.15, 1.5, 0.9));
                c.push(eq(rate, &[Band::new(BandKind::HighPass, 500.0, 0.707, 0.0), Band::new(BandKind::LowPass, 6000.0, 0.707, 0.0)]));
                c.push(Noise::new(rate, NoiseKind::Hiss, -42.0, 3000.0));
            }
            HiveMind => {
                c.push(Harmony::new(rate, max_block, &[(0.0, 0.0), (-12.0, -3.0), (7.0, -4.0), (-5.0, -4.0)], 1.0));
                c.push(Chorus::new(rate, 0.3, 6.0, 0.4));
                c.push(Reverb::new(rate, ReverbParams { size: 1.3, decay: 0.6, mix: 0.25, ..ReverbParams::HALL }));
            }
            Glitch => {
                c.push(Stutter::new(rate, 45.0, 0.35, 4));
                c.push(Decimate::new(rate, 16_000.0, 10));
            }
            DreamHaze => {
                c.push(Chorus::new(rate, 0.4, 8.0, 0.5));
                c.push(eq(rate, &[Band::new(BandKind::HighShelf, 4000.0, 0.707, -5.0), Band::new(BandKind::HighPass, 150.0, 0.707, 0.0)]));
                c.push(Reverb::new(rate, ReverbParams { pre_delay_ms: 30.0, size: 1.7, decay: 0.8, damping: 0.6, high_cut_hz: 5000.0, mix: 0.45, ..ReverbParams::HALL }));
            }
            Phantom => {
                c.push(Tremolo::new(rate, 6.5, 0.7));
                c.push(PitchShifter::new(rate, -2.0, max_block));
                c.push(eq(rate, &[Band::new(BandKind::HighPass, 250.0, 0.707, 0.0)]));
                c.push(Reverb::new(rate, ReverbParams { pre_delay_ms: 50.0, size: 1.8, decay: 0.82, damping: 0.4, low_cut_hz: 250.0, mix: 0.4, ..ReverbParams::HALL }));
            }
            DeepSea => {
                c.push(eq(rate, &[Band::new(BandKind::LowPass, 900.0, 0.8, 0.0), Band::new(BandKind::LowShelf, 200.0, 0.707, 3.0)]));
                c.push(Vibrato::new(rate, 0.5, 2.0, 1.0));
                c.push(Reverb::new(rate, ReverbParams { size: 1.2, decay: 0.6, damping: 0.9, high_cut_hz: 1500.0, mix: 0.35, ..ReverbParams::HALL }));
            }
            WhisperWind => {
                c.push(Whisper::new(rate, 0.85));
                c.push(eq(rate, &[Band::new(BandKind::HighPass, 500.0, 0.707, 0.0), Band::bell(4000.0, 1.0, 3.0)]));
                c.push(Reverb::new(rate, ReverbParams { size: 1.0, decay: 0.55, mix: 0.2, ..ReverbParams::PLATE }));
            }
            Nightline => {
                c.push(eq(rate, &[Band::new(BandKind::HighPass, 70.0, 0.707, 0.0), Band::new(BandKind::LowShelf, 180.0, 0.707, 4.0), Band::bell(3000.0, 1.0, 2.0), Band::new(BandKind::HighShelf, 9000.0, 0.707, -2.0)]));
                c.push(Compressor::new(rate, CompressorParams { threshold_db: -22.0, ratio: 4.0, attack_ms: 5.0, release_ms: 120.0, auto_makeup: true, ..CompressorParams::VOICE }));
            }
            Newsroom => {
                c.push(eq(rate, &[Band::new(BandKind::HighPass, 90.0, 0.707, 0.0), Band::bell(250.0, 1.0, -2.0), Band::bell(3500.0, 1.0, 3.0), Band::new(BandKind::HighShelf, 10_000.0, 0.707, 2.0)]));
                c.push(Compressor::new(rate, CompressorParams { threshold_db: -20.0, ratio: 3.0, auto_makeup: true, ..CompressorParams::VOICE }));
            }
            PodcastWarm => {
                c.push(eq(rate, &[Band::new(BandKind::HighPass, 60.0, 0.707, 0.0), Band::new(BandKind::LowShelf, 200.0, 0.707, 3.0), Band::bell(400.0, 1.2, -1.5), Band::bell(5000.0, 1.0, 1.5)]));
                c.push(Compressor::new(rate, CompressorParams { threshold_db: -22.0, ratio: 2.5, attack_ms: 10.0, release_ms: 200.0, auto_makeup: true, ..CompressorParams::VOICE }));
            }
            StadiumAnnouncer => {
                c.push(eq(rate, &[Band::new(BandKind::HighPass, 120.0, 0.707, 0.0), Band::bell(1500.0, 1.0, 3.0), Band::new(BandKind::HighShelf, 5000.0, 0.707, 4.0)]));
                c.push(Compressor::new(rate, CompressorParams { threshold_db: -18.0, ratio: 6.0, attack_ms: 3.0, release_ms: 100.0, auto_makeup: true, ..Default::default() }));
                c.push(StereoDelay::new(rate, DelayParams { time_left_ms: 180.0, time_right_ms: 180.0, feedback: 0.25, ping_pong: false, high_cut_hz: 5000.0, low_cut_hz: 300.0, mix: 0.2 }));
                c.push(Reverb::new(rate, ReverbParams { pre_delay_ms: 60.0, size: 2.0, decay: 0.7, mix: 0.25, ..ReverbParams::HALL }));
            }
        }
        c
    }
}

fn eq(rate: u32, bands: &[Band]) -> ParametricEq {
    let mut e = ParametricEq::new(rate);
    for i in 0..crate::eq::EQ_BANDS {
        match bands.get(i) {
            Some(b) => e.set_band(i, *b),
            None => e.set_band(i, Band { enabled: false, ..Band::flat(1000.0) }),
        }
    }
    e
}

/// A chain of effects run in order, with a final wet/dry mix.
pub struct VoiceChain {
    stages: Vec<Box<dyn Effect>>,
    /// Blend between the untouched voice (0) and the full style (1).
    pub amount: f32,
    dry: [Vec<f32>; 2],
    delay_line: [Vec<f32>; 2],
    delay_pos: usize,
}

impl VoiceChain {
    /// An empty chain (pass-through).
    pub fn new() -> Self {
        Self { stages: Vec::new(), amount: 1.0, dry: [Vec::new(), Vec::new()], delay_line: [Vec::new(), Vec::new()], delay_pos: 0 }
    }

    /// Appends a stage.
    pub fn push<E: Effect + 'static>(&mut self, e: E) {
        self.stages.push(Box::new(e));
        let lat = self.latency();
        if lat > 0 {
            // Dry path delay for latency-matched blending.
            self.delay_line = [vec![0.0; lat], vec![0.0; lat]];
            self.delay_pos = 0;
        }
    }

    /// Number of stages.
    pub fn len(&self) -> usize {
        self.stages.len()
    }

    /// True for a pass-through chain.
    pub fn is_empty(&self) -> bool {
        self.stages.is_empty()
    }

    /// Makes room for blending blocks up to `max_block` frames (call once
    /// after building when `amount` will be under 1).
    pub fn prepare(&mut self, max_block: usize) {
        self.dry = [vec![0.0; max_block], vec![0.0; max_block]];
    }
}

impl Default for VoiceChain {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for VoiceChain {
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let blend = self.amount < 1.0 && self.dry[0].len() >= left.len();
        if blend {
            self.dry[0][..left.len()].copy_from_slice(left);
            self.dry[1][..right.len()].copy_from_slice(right);
        }
        for s in &mut self.stages {
            s.process(left, right);
        }
        if blend {
            let a = self.amount.max(0.0);
            let lat = self.delay_line[0].len();
            for i in 0..left.len() {
                let (dl, dr) = if lat > 0 {
                    let p = self.delay_pos;
                    let out = (self.delay_line[0][p], self.delay_line[1][p]);
                    self.delay_line[0][p] = self.dry[0][i];
                    self.delay_line[1][p] = self.dry[1][i];
                    self.delay_pos = (p + 1) % lat;
                    out
                } else {
                    (self.dry[0][i], self.dry[1][i])
                };
                left[i] = dl + (left[i] - dl) * a;
                right[i] = dr + (right[i] - dr) * a;
            }
        }
    }

    fn reset(&mut self) {
        for s in &mut self.stages {
            s.reset();
        }
        for d in &mut self.delay_line {
            d.fill(0.0);
        }
        self.delay_pos = 0;
    }

    fn latency(&self) -> usize {
        self.stages.iter().map(|s| s.latency()).sum()
    }

    fn tail(&self) -> usize {
        self.stages.iter().map(|s| s.tail()).max().unwrap_or(0)
    }
}

// ---------------------------------------------------------------------------
// Colour effects used by the styles. Each is a public, reusable Effect.
// ---------------------------------------------------------------------------

/// Soft saturation (tanh) with a tone low-pass after it.
pub struct Saturation {
    drive: f32,
    tone: Biquad,
    makeup: f32,
}

impl Saturation {
    /// `drive` ≥ 1 pushes harder; `tone_hz` rolls off the harmonics.
    pub fn new(rate: u32, drive: f32, tone_hz: f32) -> Self {
        let drive = drive.max(0.1);
        Self { drive, tone: Biquad::design(BandKind::LowPass, rate, tone_hz, 0.707, 0.0), makeup: 1.0 / drive.tanh() }
    }
}

impl Effect for Saturation {
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        for v in left.iter_mut().chain(right.iter_mut()) {
            *v = (*v * self.drive).tanh() * self.makeup;
        }
        self.tone.process(left, right);
    }
    fn reset(&mut self) {
        self.tone.reset();
    }
}

/// Ring modulation by a sine.
pub struct RingMod {
    step: f32,
    phase: f32,
    depth: f32,
}

impl RingMod {
    /// Carrier at `freq` Hz, `depth` 0..1 blends with the dry voice.
    pub fn new(rate: u32, freq: f32, depth: f32) -> Self {
        Self { step: freq / rate as f32, phase: 0.0, depth: depth.clamp(0.0, 1.0) }
    }
}

impl Effect for RingMod {
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let m = (self.phase * std::f32::consts::TAU).sin();
            self.phase = (self.phase + self.step) % 1.0;
            let g = 1.0 - self.depth + self.depth * m;
            *l *= g;
            *r *= g;
        }
    }
    fn reset(&mut self) {
        self.phase = 0.0;
    }
}

/// Amplitude modulation by a sine.
pub struct Tremolo {
    step: f32,
    phase: f32,
    depth: f32,
}

impl Tremolo {
    /// `rate_hz` wobble speed, `depth` 0..1.
    pub fn new(rate: u32, rate_hz: f32, depth: f32) -> Self {
        Self { step: rate_hz / rate as f32, phase: 0.0, depth: depth.clamp(0.0, 1.0) }
    }
}

impl Effect for Tremolo {
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let m = 0.5 + 0.5 * (self.phase * std::f32::consts::TAU).sin();
            self.phase = (self.phase + self.step) % 1.0;
            let g = 1.0 - self.depth * m;
            *l *= g;
            *r *= g;
        }
    }
    fn reset(&mut self) {
        self.phase = 0.0;
    }
}

/// A short modulated delay line shared by vibrato and chorus.
struct ModDelay {
    buf: [Vec<f32>; 2],
    write: usize,
}

impl ModDelay {
    fn new(max_frames: usize) -> Self {
        Self { buf: [vec![0.0; max_frames + 2], vec![0.0; max_frames + 2]], write: 0 }
    }

    #[inline]
    fn read(&self, ch: usize, delay: f32) -> f32 {
        let n = self.buf[ch].len();
        let d = delay.clamp(1.0, (n - 2) as f32);
        let i = d as usize;
        let f = d - i as f32;
        let a = self.buf[ch][(self.write + n - i) % n];
        let b = self.buf[ch][(self.write + n - i - 1) % n];
        a + (b - a) * f
    }

    #[inline]
    fn push(&mut self, l: f32, r: f32) {
        self.buf[0][self.write] = l;
        self.buf[1][self.write] = r;
        self.write = (self.write + 1) % self.buf[0].len();
    }

    fn clear(&mut self) {
        for b in &mut self.buf {
            b.fill(0.0);
        }
        self.write = 0;
    }
}

/// Pitch wobble: a delay line whose length sweeps with a sine.
pub struct Vibrato {
    line: ModDelay,
    step: f32,
    phase: f32,
    centre: f32,
    depth: f32,
    mix: f32,
}

impl Vibrato {
    /// `rate_hz` wobble speed, `depth_ms` sweep amplitude, `mix` 0..1.
    pub fn new(rate: u32, rate_hz: f32, depth_ms: f32, mix: f32) -> Self {
        let depth = depth_ms * 0.001 * rate as f32;
        let centre = depth + 2.0;
        Self { line: ModDelay::new((centre + depth) as usize + 4), step: rate_hz / rate as f32, phase: 0.0, centre, depth, mix: mix.clamp(0.0, 1.0) }
    }
}

impl Effect for Vibrato {
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            self.line.push(*l, *r);
            let d = self.centre + self.depth * (self.phase * std::f32::consts::TAU).sin();
            self.phase = (self.phase + self.step) % 1.0;
            let wl = self.line.read(0, d);
            let wr = self.line.read(1, d);
            *l += (wl - *l) * self.mix;
            *r += (wr - *r) * self.mix;
        }
    }
    fn reset(&mut self) {
        self.line.clear();
        self.phase = 0.0;
    }
    fn latency(&self) -> usize {
        self.centre as usize
    }
}

/// Three-voice chorus with the voices spread across the stereo field.
pub struct Chorus {
    line: ModDelay,
    step: f32,
    phase: f32,
    centre: f32,
    depth: f32,
    mix: f32,
}

impl Chorus {
    /// `rate_hz` modulation speed, `depth_ms` sweep, `mix` 0..1.
    pub fn new(rate: u32, rate_hz: f32, depth_ms: f32, mix: f32) -> Self {
        let depth = depth_ms * 0.001 * rate as f32;
        let centre = 0.02 * rate as f32;
        Self { line: ModDelay::new((centre + depth) as usize + 4), step: rate_hz / rate as f32, phase: 0.0, centre, depth, mix: mix.clamp(0.0, 1.0) }
    }
}

impl Effect for Chorus {
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let third = 1.0 / 3.0;
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            self.line.push(*l, *r);
            let p = self.phase;
            self.phase = (self.phase + self.step) % 1.0;
            let d0 = self.centre + self.depth * (p * std::f32::consts::TAU).sin();
            let d1 = self.centre + self.depth * ((p + third) * std::f32::consts::TAU).sin();
            let d2 = self.centre + self.depth * ((p + 2.0 * third) * std::f32::consts::TAU).sin();
            let wl = (self.line.read(0, d0) + 0.7 * self.line.read(0, d1) + 0.3 * self.line.read(1, d2)) * 0.5;
            let wr = (self.line.read(1, d0) + 0.3 * self.line.read(0, d1) + 0.7 * self.line.read(1, d2)) * 0.5;
            *l += (wl - *l) * self.mix;
            *r += (wr - *r) * self.mix;
        }
    }
    fn reset(&mut self) {
        self.line.clear();
        self.phase = 0.0;
    }
}

/// Feedback comb filter tuned to a pitch: gives a metallic, fixed-pitch ring.
pub struct Comb {
    line: ModDelay,
    delay: f32,
    feedback: f32,
    mix: f32,
    fb_state: [f32; 2],
}

impl Comb {
    /// Resonance at `freq` Hz with `feedback` 0..0.95 and `mix` 0..1.
    pub fn new(rate: u32, freq: f32, feedback: f32, mix: f32) -> Self {
        let delay = rate as f32 / freq.max(20.0);
        Self { line: ModDelay::new(delay as usize + 4), delay, feedback: feedback.clamp(0.0, 0.95), mix: mix.clamp(0.0, 1.0), fb_state: [0.0; 2] }
    }
}

impl Effect for Comb {
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let dl = self.line.read(0, self.delay);
            let dr = self.line.read(1, self.delay);
            // Light damping in the loop keeps it from whistling.
            self.fb_state[0] = 0.5 * (dl + self.fb_state[0]);
            self.fb_state[1] = 0.5 * (dr + self.fb_state[1]);
            self.line.push(*l + self.fb_state[0] * self.feedback, *r + self.fb_state[1] * self.feedback);
            *l += (dl - *l) * self.mix;
            *r += (dr - *r) * self.mix;
        }
    }
    fn reset(&mut self) {
        self.line.clear();
        self.fb_state = [0.0; 2];
    }
}

/// Sample-rate and bit-depth reduction.
pub struct Decimate {
    step: f32,
    acc: f32,
    held: [f32; 2],
    levels: f32,
}

impl Decimate {
    /// Holds samples to emulate `target_rate` Hz and quantises to `bits`.
    pub fn new(rate: u32, target_rate: f32, bits: u32) -> Self {
        Self { step: (target_rate / rate as f32).clamp(0.01, 1.0), acc: 1.0, held: [0.0; 2], levels: (1u32 << bits.clamp(2, 16)) as f32 / 2.0 }
    }
}

impl Effect for Decimate {
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            self.acc += self.step;
            if self.acc >= 1.0 {
                self.acc -= 1.0;
                self.held = [(*l * self.levels).round() / self.levels, (*r * self.levels).round() / self.levels];
            }
            *l = self.held[0];
            *r = self.held[1];
        }
    }
    fn reset(&mut self) {
        self.acc = 1.0;
        self.held = [0.0; 2];
    }
}

/// What a [`Noise`] stage adds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoiseKind {
    /// Continuous hiss.
    Hiss,
    /// Sparse clicks and pops.
    Crackle,
}

/// Adds hiss or crackle, high-passed so it does not rumble. Deterministic
/// (xorshift) so renders are repeatable.
pub struct Noise {
    kind: NoiseKind,
    gain: f32,
    seed: u32,
    hp: Biquad,
    crackle_rate: f32,
    crackle_left: f32,
    crackle_amp: f32,
}

impl Noise {
    /// `level_db` sets the noise level; `hp_hz` the high-pass under it.
    pub fn new(rate: u32, kind: NoiseKind, level_db: f32, hp_hz: f32) -> Self {
        Self { kind, gain: db_to_gain(level_db), seed: 0x9E37_79B9, hp: Biquad::design(BandKind::HighPass, rate, hp_hz, 0.707, 0.0), crackle_rate: 40.0 / rate as f32, crackle_left: 0.0, crackle_amp: 0.0 }
    }

    #[inline]
    fn next(&mut self) -> f32 {
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 17;
        self.seed ^= self.seed << 5;
        (self.seed >> 8) as f32 / (1u32 << 23) as f32 - 1.0
    }
}

impl Effect for Noise {
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let n = match self.kind {
                NoiseKind::Hiss => self.next() * self.gain,
                NoiseKind::Crackle => {
                    let u = (self.next() + 1.0) * 0.5;
                    if self.crackle_left <= 0.0 && u < self.crackle_rate {
                        self.crackle_left = 20.0 + u * 2000.0;
                        self.crackle_amp = (self.next().abs() + 0.2) * self.gain * 6.0;
                    }
                    if self.crackle_left > 0.0 {
                        self.crackle_left -= 1.0;
                        self.crackle_amp *= 0.85;
                        self.next() * self.crackle_amp
                    } else {
                        0.0
                    }
                }
            };
            let n = self.hp.tick(0, n);
            *l += n;
            *r += n;
        }
    }
    fn reset(&mut self) {
        self.hp.reset();
        self.crackle_left = 0.0;
    }
}

/// Whisper: replaces the voiced part with noise shaped by the voice's own
/// envelope and spectrum (the noise is filtered by a fast band-tracking
/// filter bank fed from the signal), keeping the breath and consonants.
pub struct Whisper {
    amount: f32,
    seed: u32,
    bands: [Biquad; 4],
    env: [f32; 4],
    env_coeff: f32,
    out_bands: [Biquad; 4],
}

impl Whisper {
    /// `amount` 0..1 blends the whispered copy with the original.
    pub fn new(rate: u32, amount: f32) -> Self {
        let freqs = [500.0, 1200.0, 2500.0, 5000.0];
        let bands = freqs.map(|f| Biquad::design(BandKind::BandPass, rate, f, 1.5, 0.0));
        let out_bands = freqs.map(|f| Biquad::design(BandKind::BandPass, rate, f, 1.5, 0.0));
        Self { amount: amount.clamp(0.0, 1.0), seed: 0x1234_5678, bands, env: [0.0; 4], env_coeff: crate::dynamics::time_coeff(8.0, rate), out_bands }
    }
}

impl Effect for Whisper {
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let x = 0.5 * (*l + *r);
            self.seed ^= self.seed << 13;
            self.seed ^= self.seed >> 17;
            self.seed ^= self.seed << 5;
            let n = (self.seed >> 8) as f32 / (1u32 << 23) as f32 - 1.0;
            let mut w = 0.0;
            for b in 0..4 {
                let band = self.bands[b].tick(0, x).abs();
                self.env[b] = band + self.env_coeff * (self.env[b] - band);
                w += self.out_bands[b].tick(0, n) * self.env[b] * 10.0;
            }
            *l += (w - *l) * self.amount;
            *r += (w - *r) * self.amount;
        }
    }
    fn reset(&mut self) {
        for b in self.bands.iter_mut().chain(self.out_bands.iter_mut()) {
            b.reset();
        }
        self.env = [0.0; 4];
    }
}

/// Several pitch-shifted copies mixed together (for twins and choirs).
pub struct Harmony {
    voices: Vec<(PitchShifter, f32)>,
    scratch: [Vec<f32>; 2],
    acc: [Vec<f32>; 2],
    dry_gain: f32,
    delay: [Vec<f32>; 2],
    delay_pos: usize,
}

impl Harmony {
    /// `voices` are (semitones, level dB) pairs; `dry` is the original's gain
    /// (0..1), delayed to line up with the shifted copies.
    pub fn new(rate: u32, max_block: usize, voices: &[(f32, f32)], dry: f32) -> Self {
        let voices: Vec<(PitchShifter, f32)> = voices.iter().map(|(st, db)| (PitchShifter::new(rate, *st, max_block), db_to_gain(*db))).collect();
        let lat = voices.first().map(|v| v.0.latency()).unwrap_or(0);
        Self { voices, scratch: [vec![0.0; max_block], vec![0.0; max_block]], acc: [vec![0.0; max_block], vec![0.0; max_block]], dry_gain: dry.clamp(0.0, 1.0), delay: [vec![0.0; lat.max(1)], vec![0.0; lat.max(1)]], delay_pos: 0 }
    }
}

impl Effect for Harmony {
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let n = left.len().min(self.scratch[0].len());
        self.acc[0][..n].fill(0.0);
        self.acc[1][..n].fill(0.0);
        for (ps, g) in &mut self.voices {
            let [sl, sr] = &mut self.scratch;
            sl[..n].copy_from_slice(&left[..n]);
            sr[..n].copy_from_slice(&right[..n]);
            ps.process(&mut sl[..n], &mut sr[..n]);
            for i in 0..n {
                self.acc[0][i] += self.scratch[0][i] * *g;
                self.acc[1][i] += self.scratch[1][i] * *g;
            }
        }
        let lat = self.delay[0].len();
        let norm = 1.0 / (1.0 + self.voices.len() as f32 * 0.5);
        for i in 0..n {
            let p = self.delay_pos;
            let (dl, dr) = (self.delay[0][p], self.delay[1][p]);
            self.delay[0][p] = left[i];
            self.delay[1][p] = right[i];
            self.delay_pos = (p + 1) % lat;
            left[i] = (dl * self.dry_gain + self.acc[0][i]) * norm;
            right[i] = (dr * self.dry_gain + self.acc[1][i]) * norm;
        }
    }
    fn reset(&mut self) {
        for (ps, _) in &mut self.voices {
            ps.reset();
        }
        for d in &mut self.delay {
            d.fill(0.0);
        }
        self.delay_pos = 0;
    }
    fn latency(&self) -> usize {
        self.delay[0].len()
    }
}

/// Stutter: repeats short slices at random, like a dropped packet.
pub struct Stutter {
    buf: [Vec<f32>; 2],
    slice: usize,
    write: usize,
    repeat_left: usize,
    repeats: usize,
    read: usize,
    chance: f32,
    seed: u32,
}

impl Stutter {
    /// `slice_ms` slice length, `chance` 0..1 of stuttering each slice,
    /// `repeats` how many times a slice repeats.
    pub fn new(rate: u32, slice_ms: f32, chance: f32, repeats: usize) -> Self {
        let slice = ((slice_ms * 0.001 * rate as f32) as usize).max(32);
        Self { buf: [vec![0.0; slice], vec![0.0; slice]], slice, write: 0, repeat_left: 0, repeats: repeats.max(1), read: 0, chance: chance.clamp(0.0, 1.0), seed: 0xC0FF_EE11 }
    }
}

impl Effect for Stutter {
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            if self.repeat_left > 0 {
                *l = self.buf[0][self.read];
                *r = self.buf[1][self.read];
                self.read += 1;
                if self.read == self.slice {
                    self.read = 0;
                    self.repeat_left -= 1;
                }
            } else {
                self.buf[0][self.write] = *l;
                self.buf[1][self.write] = *r;
                self.write += 1;
                if self.write == self.slice {
                    self.write = 0;
                    self.seed ^= self.seed << 13;
                    self.seed ^= self.seed >> 17;
                    self.seed ^= self.seed << 5;
                    let u = (self.seed >> 8) as f32 / (1u32 << 24) as f32;
                    if u < self.chance {
                        self.repeat_left = self.repeats;
                        self.read = 0;
                    }
                }
            }
        }
    }
    fn reset(&mut self) {
        for b in &mut self.buf {
            b.fill(0.0);
        }
        self.write = 0;
        self.repeat_left = 0;
        self.read = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalogue_is_complete_and_unique() {
        assert!(VoiceStyle::ALL.len() >= 30);
        let mut ids: Vec<&str> = VoiceStyle::ALL.iter().map(|s| s.id()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), VoiceStyle::ALL.len());
        let mut names: Vec<&str> = VoiceStyle::ALL.iter().map(|s| s.name()).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), VoiceStyle::ALL.len());
        for s in VoiceStyle::ALL {
            assert_eq!(VoiceStyle::from_id(s.id()), Some(s));
            assert!(!s.description().is_empty());
        }
    }

    #[test]
    fn every_style_runs_in_blocks_and_stays_finite_and_sane() {
        let rate = 48_000;
        let n = rate as usize * 2;
        let voice: Vec<f32> = (0..n)
            .map(|i| {
                let t = i as f32 / rate as f32;
                let env = 0.5 + 0.5 * (std::f32::consts::TAU * 3.0 * t).sin();
                let mut v = 0.0;
                for h in 1..10 {
                    v += (std::f32::consts::TAU * 160.0 * h as f32 * t).sin() / h as f32;
                }
                0.25 * v * env
            })
            .collect();
        let rms_in = (voice.iter().map(|v| v * v).sum::<f32>() / n as f32).sqrt();
        for s in VoiceStyle::ALL {
            let mut chain = s.build(rate, 512);
            assert!(!chain.is_empty(), "{s:?}");
            let mut l = voice.clone();
            let mut r = voice.clone();
            for (a, b) in l.chunks_mut(512).zip(r.chunks_mut(512)) {
                chain.process(a, b);
            }
            assert!(l.iter().chain(&r).all(|v| v.is_finite()), "{s:?} produced NaN");
            let rms_out = (l[n / 2..].iter().map(|v| v * v).sum::<f32>() / (n / 2) as f32).sqrt();
            let ratio = crate::gain_to_db(rms_out / rms_in);
            assert!((-18.0..=12.0).contains(&ratio), "{s:?}: level changed by {ratio} dB");
            let peak = l.iter().fold(0.0f32, |m, v| m.max(v.abs()));
            assert!(peak < 4.0, "{s:?}: peak {peak}");
            chain.reset();
        }
    }

    #[test]
    fn blend_amount_zero_is_dry() {
        let rate = 48_000;
        let mut chain = VoiceStyle::Cathedral.build(rate, 256);
        chain.prepare(256);
        chain.amount = 0.0;
        let x: Vec<f32> = (0..256).map(|i| (i as f32 * 0.05).sin()).collect();
        let mut l = x.clone();
        let mut r = x.clone();
        chain.process(&mut l, &mut r);
        assert_eq!(l, x);
    }
}
