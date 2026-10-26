// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The phonemes of English (ARPAbet, as CMUdict writes them) and what
//! each is acoustically: the classes the aligner listens for.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Ph {
    AA, AE, AH, AO, AW, AY, EH, ER, EY, IH, IY, OW, OY, UH, UW,
    B, CH, D, DH, F, G, HH, JH, K, L, M, N, NG, P, R, S, SH, T, TH, V, W, Y, Z, ZH,
    /// Silence between words or phrases.
    Sil,
}

/// What a phoneme sounds like, for the aligner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    Vowel,
    /// L, R, W, Y: voiced and vowel-like but weaker.
    Glide,
    Nasal,
    VoicedFric,
    UnvoicedFric,
    /// A stop's closure then burst; voiced or not.
    VoicedStop,
    UnvoicedStop,
    Affricate,
    Silence,
}

const ALL: [(&str, Ph); 40] = [
    ("AA", Ph::AA), ("AE", Ph::AE), ("AH", Ph::AH), ("AO", Ph::AO), ("AW", Ph::AW), ("AY", Ph::AY), ("EH", Ph::EH), ("ER", Ph::ER), ("EY", Ph::EY), ("IH", Ph::IH), ("IY", Ph::IY), ("OW", Ph::OW), ("OY", Ph::OY), ("UH", Ph::UH), ("UW", Ph::UW),
    ("B", Ph::B), ("CH", Ph::CH), ("D", Ph::D), ("DH", Ph::DH), ("F", Ph::F), ("G", Ph::G), ("HH", Ph::HH), ("JH", Ph::JH), ("K", Ph::K), ("L", Ph::L), ("M", Ph::M), ("N", Ph::N), ("NG", Ph::NG), ("P", Ph::P), ("R", Ph::R), ("S", Ph::S), ("SH", Ph::SH), ("T", Ph::T), ("TH", Ph::TH), ("V", Ph::V), ("W", Ph::W), ("Y", Ph::Y), ("Z", Ph::Z), ("ZH", Ph::ZH), ("SIL", Ph::Sil),
];

impl Ph {
    /// From an ARPAbet symbol, with an optional stress digit ("AH0").
    /// Returns the phoneme and its stress (0 none, 1 primary, 2 secondary).
    pub fn parse(s: &str) -> Option<(Ph, u8)> {
        let t = s.trim();
        let (body, stress) = match t.as_bytes().last() {
            Some(d @ b'0'..=b'2') => (&t[..t.len() - 1], d - b'0'),
            _ => (t, 0),
        };
        let up = body.to_ascii_uppercase();
        ALL.iter().find(|(n, _)| *n == up).map(|&(_, p)| (p, stress))
    }

    pub fn name(self) -> &'static str {
        ALL.iter().find(|(_, p)| *p == self).map_or("?", |(n, _)| n)
    }

    pub fn is_vowel(self) -> bool {
        self.class() == Class::Vowel
    }

    pub fn class(self) -> Class {
        use Ph::*;
        match self {
            AA | AE | AH | AO | AW | AY | EH | ER | EY | IH | IY | OW | OY | UH | UW => Class::Vowel,
            L | R | W | Y => Class::Glide,
            M | N | NG => Class::Nasal,
            DH | V | Z | ZH => Class::VoicedFric,
            F | S | SH | TH | HH => Class::UnvoicedFric,
            B | D | G => Class::VoicedStop,
            P | T | K => Class::UnvoicedStop,
            CH | JH => Class::Affricate,
            Sil => Class::Silence,
        }
    }

    /// Plosives and nasal stops where the lips close: M, B, P.
    pub fn is_bilabial_stop(self) -> bool {
        matches!(self, Ph::M | Ph::B | Ph::P)
    }

    /// Typical length in seconds at an ordinary speaking rate.
    pub fn typical(self) -> f64 {
        use Ph::*;
        match self {
            AA | AE | AO | AW | AY | OY | ER | EY | IY | OW | UW => 0.12,
            AH | EH | IH | UH => 0.075,
            M | N | NG => 0.07,
            L | R | W | Y => 0.065,
            S | SH | Z | ZH | F | TH => 0.10,
            V | DH => 0.05,
            HH => 0.06,
            B | D | G => 0.07,
            P | T | K => 0.09,
            CH | JH => 0.11,
            Sil => 0.12,
        }
    }
}

/// A phoneme placed in time (seconds from the start of the audio).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Timed {
    pub ph: Ph,
    pub stress: u8,
    pub start: f64,
    pub end: f64,
    /// Index of its word in the words given (None for silence).
    pub word: Option<usize>,
}

impl Timed {
    pub fn dur(&self) -> f64 {
        self.end - self.start
    }
}
