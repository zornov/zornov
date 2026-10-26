// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Visemes: the mouth shapes of speech, and the channels (jaw, lip
//! closure, rounding, stretch, tongue…) each is made of. Jaw opening is
//! its own channel, apart from the lip shapes, so a rig can drive them
//! separately.

use crate::phoneme::Ph;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Viseme {
    Rest,
    /// M, B, P: lips pressed shut.
    MBP,
    /// F, V: lower lip to the upper teeth.
    FV,
    /// TH, DH: tongue between the teeth.
    TH,
    L,
    R,
    /// W and OO: rounded lips.
    WOO,
    /// CH, SH, J, ZH.
    CHSHJ,
    /// S, Z.
    SZ,
    AA,
    AE,
    AH,
    EE,
    IH,
    OH,
    UH,
    ER,
    /// T, D, N, K, G, H, Y: the tongue works, the lips follow their neighbours.
    Etc,
}

pub const VISEMES: [Viseme; 18] = [
    Viseme::Rest, Viseme::MBP, Viseme::FV, Viseme::TH, Viseme::L, Viseme::R, Viseme::WOO, Viseme::CHSHJ, Viseme::SZ, Viseme::AA, Viseme::AE, Viseme::AH, Viseme::EE, Viseme::IH, Viseme::OH, Viseme::UH, Viseme::ER, Viseme::Etc,
];

impl Viseme {
    pub fn index(self) -> usize {
        VISEMES.iter().position(|v| *v == self).unwrap_or(0)
    }

    pub fn name(self) -> &'static str {
        match self {
            Viseme::Rest => "rest",
            Viseme::MBP => "MBP",
            Viseme::FV => "FV",
            Viseme::TH => "TH",
            Viseme::L => "L",
            Viseme::R => "R",
            Viseme::WOO => "WOO",
            Viseme::CHSHJ => "CHSHJ",
            Viseme::SZ => "SZ",
            Viseme::AA => "AA",
            Viseme::AE => "AE",
            Viseme::AH => "AH",
            Viseme::EE => "EE",
            Viseme::IH => "IH",
            Viseme::OH => "OH",
            Viseme::UH => "UH",
            Viseme::ER => "ER",
            Viseme::Etc => "etc",
        }
    }

    /// From what a mouth chart's drawings are called: our names, the
    /// classic animation chart letters, the older four-shape names.
    pub fn named(n: &str) -> Option<Viseme> {
        let k: String = n.to_lowercase().chars().filter(|c| c.is_ascii_alphanumeric()).collect();
        Some(match k.as_str() {
            "rest" | "closed" | "shut" | "x" | "silence" | "sil" | "neutral" => Viseme::Rest,
            "mbp" | "m" | "b" | "p" | "pp" => Viseme::MBP,
            "fv" | "f" | "v" | "ff" => Viseme::FV,
            "th" | "dh" => Viseme::TH,
            "l" | "lt" | "tongue" => Viseme::L,
            "r" | "rr" => Viseme::R,
            "woo" | "oo" | "w" | "wq" | "u" | "q" | "wu" => Viseme::WOO,
            "chshj" | "ch" | "sh" | "j" | "jh" | "zh" => Viseme::CHSHJ,
            "sz" | "s" | "z" | "ss" => Viseme::SZ,
            "aa" | "ah" | "a" | "open" | "ai" | "aaa" => Viseme::AA,
            "ae" | "eh" | "wide" => Viseme::AE,
            "ee" | "e" | "i" | "ii" | "smile" | "teeth" => Viseme::EE,
            "ih" => Viseme::IH,
            "oh" | "o" | "round" | "ao" => Viseme::OH,
            "uh" => Viseme::UH,
            "er" => Viseme::ER,
            "etc" | "dd" | "nn" | "kk" | "tdnkg" | "cons" | "consonant" | "other" => Viseme::Etc,
            _ => return None,
        })
    }

    /// Visemes to try, nearest first, when a chart lacks this one.
    pub fn fallbacks(self) -> &'static [Viseme] {
        use Viseme::*;
        match self {
            Rest => &[Rest, MBP],
            MBP => &[MBP, Rest],
            FV => &[FV, EE, IH, Etc, AH],
            TH => &[TH, L, Etc, IH, AH],
            L => &[L, Etc, IH, AH, EE],
            R => &[R, ER, UH, WOO, AH],
            WOO => &[WOO, UH, OH, R, AH],
            CHSHJ => &[CHSHJ, WOO, UH, SZ, IH, AH],
            SZ => &[SZ, EE, IH, Etc, AH],
            AA => &[AA, AH, AE, OH],
            AE => &[AE, AH, AA, EE, IH],
            AH => &[AH, AA, AE, UH, IH],
            EE => &[EE, IH, SZ, AE, AH],
            IH => &[IH, EE, AE, AH],
            OH => &[OH, UH, WOO, AA, AH],
            UH => &[UH, OH, WOO, AH],
            ER => &[ER, R, UH, AH],
            Etc => &[Etc, IH, AH, L, Rest],
        }
    }
}

/// How a phoneme is made: its viseme, and for diphthongs the viseme it
/// glides to.
pub fn of_phoneme(p: Ph) -> (Viseme, Option<Viseme>) {
    use Ph::*;
    use Viseme as V;
    match p {
        AA => (V::AA, None),
        AE | EH => (V::AE, None),
        AH => (V::AH, None),
        AO => (V::OH, None),
        AW => (V::AA, Some(V::WOO)),
        AY => (V::AA, Some(V::IH)),
        ER => (V::ER, None),
        EY => (V::AE, Some(V::EE)),
        IH => (V::IH, None),
        IY => (V::EE, None),
        OW => (V::OH, Some(V::WOO)),
        OY => (V::OH, Some(V::EE)),
        UH => (V::UH, None),
        UW => (V::WOO, None),
        B | M | P => (V::MBP, None),
        F | V => (V::FV, None),
        TH | DH => (V::TH, None),
        L => (V::L, None),
        R => (V::R, None),
        W => (V::WOO, None),
        CH | JH | SH | ZH => (V::CHSHJ, None),
        S | Z => (V::SZ, None),
        T | D | N | K | G | NG | HH => (V::Etc, None),
        Y => (V::IH, None),
        Sil => (V::Rest, None),
    }
}

/// The things a mouth does, each 0..1 (the smile, frown and brows 0..1 as
/// well; eye gaze -1..1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Channel {
    /// The jaw drops.
    Jaw,
    /// The lips press together (works with the jaw open).
    Close,
    /// Lips pushed forward and rounded.
    Pucker,
    /// Lip corners drawn wide.
    Stretch,
    /// Lower lip rolled in under the upper teeth.
    Roll,
    /// Upper teeth showing.
    Teeth,
    TongueUp,
    TongueDown,
    TongueFwd,
    TongueCurl,
    Smile,
    Frown,
    BrowUp,
    BrowDown,
    Blink,
    EyeX,
    EyeY,
}

pub const CHANNELS: [Channel; 17] = [
    Channel::Jaw, Channel::Close, Channel::Pucker, Channel::Stretch, Channel::Roll, Channel::Teeth, Channel::TongueUp, Channel::TongueDown, Channel::TongueFwd, Channel::TongueCurl, Channel::Smile, Channel::Frown, Channel::BrowUp, Channel::BrowDown, Channel::Blink, Channel::EyeX, Channel::EyeY,
];

/// The channels visemes themselves make (the rest come from emotion,
/// blinks and gaze).
pub const MOUTH_CHANNELS: [Channel; 10] = [
    Channel::Jaw, Channel::Close, Channel::Pucker, Channel::Stretch, Channel::Roll, Channel::Teeth, Channel::TongueUp, Channel::TongueDown, Channel::TongueFwd, Channel::TongueCurl,
];

impl Channel {
    pub fn index(self) -> usize {
        CHANNELS.iter().position(|c| *c == self).unwrap_or(0)
    }
    pub fn name(self) -> &'static str {
        match self {
            Channel::Jaw => "jaw",
            Channel::Close => "close",
            Channel::Pucker => "pucker",
            Channel::Stretch => "stretch",
            Channel::Roll => "roll",
            Channel::Teeth => "teeth",
            Channel::TongueUp => "tongue_up",
            Channel::TongueDown => "tongue_down",
            Channel::TongueFwd => "tongue_forward",
            Channel::TongueCurl => "tongue_curl",
            Channel::Smile => "smile",
            Channel::Frown => "frown",
            Channel::BrowUp => "brow_up",
            Channel::BrowDown => "brow_down",
            Channel::Blink => "blink",
            Channel::EyeX => "eye_x",
            Channel::EyeY => "eye_y",
        }
    }
}

/// What a viseme asks of the lips and jaw: `(channel, target, strength)`
/// for the mouth channels. Strength is how firmly it holds its target
/// against its neighbours' (closures hold hardest; the consonants that
/// let the lips follow their neighbours barely hold at all).
pub fn lip_targets(v: Viseme) -> [(f32, f32); 6] {
    // jaw, close, pucker, stretch, roll, teeth
    use Viseme::*;
    let s = |a: [f32; 6], w: [f32; 6]| -> [(f32, f32); 6] { std::array::from_fn(|i| (a[i], w[i])) };
    match v {
        Rest => s([0.0, 0.0, 0.0, 0.0, 0.0, 0.0], [0.7, 0.5, 0.7, 0.7, 0.7, 0.7]),
        MBP => s([0.04, 1.0, 0.0, 0.0, 0.0, 0.0], [0.5, 6.0, 1.0, 1.0, 1.0, 1.0]),
        FV => s([0.12, 0.0, 0.0, 0.1, 1.0, 0.6], [0.8, 1.5, 1.2, 0.5, 8.0, 2.0]),
        TH => s([0.22, 0.0, 0.0, 0.15, 0.0, 0.5], [0.7, 1.5, 0.8, 0.5, 1.0, 1.0]),
        L => s([0.3, 0.0, 0.0, 0.2, 0.0, 0.3], [0.45, 1.0, 0.4, 0.4, 1.0, 0.4]),
        R => s([0.25, 0.0, 0.5, 0.0, 0.0, 0.1], [0.5, 1.0, 1.4, 0.5, 1.0, 0.5]),
        WOO => s([0.22, 0.0, 1.0, 0.0, 0.0, 0.0], [0.7, 1.0, 2.5, 1.5, 1.0, 1.0]),
        CHSHJ => s([0.2, 0.0, 0.65, 0.0, 0.0, 0.5], [0.7, 1.0, 2.0, 1.5, 1.0, 1.0]),
        SZ => s([0.12, 0.0, 0.0, 0.45, 0.0, 0.8], [1.0, 1.0, 1.5, 1.5, 1.0, 2.0]),
        AA => s([1.0, 0.0, 0.0, 0.1, 0.0, 0.3], [1.3, 1.0, 0.8, 0.7, 1.0, 0.6]),
        AE => s([0.7, 0.0, 0.0, 0.55, 0.0, 0.5], [1.1, 1.0, 0.8, 1.0, 1.0, 0.8]),
        AH => s([0.6, 0.0, 0.05, 0.05, 0.0, 0.2], [1.0, 1.0, 0.6, 0.6, 1.0, 0.5]),
        EE => s([0.22, 0.0, 0.0, 1.0, 0.0, 0.7], [0.9, 1.0, 1.0, 1.6, 1.0, 1.2]),
        IH => s([0.3, 0.0, 0.0, 0.55, 0.0, 0.4], [0.8, 1.0, 0.8, 1.0, 1.0, 0.8]),
        OH => s([0.65, 0.0, 0.75, 0.0, 0.0, 0.0], [1.1, 1.0, 1.8, 1.2, 1.0, 1.0]),
        UH => s([0.35, 0.0, 0.45, 0.0, 0.0, 0.0], [0.8, 1.0, 1.2, 0.8, 1.0, 1.0]),
        ER => s([0.3, 0.0, 0.4, 0.1, 0.0, 0.1], [0.8, 1.0, 1.0, 0.6, 1.0, 0.6]),
        // Lips follow the neighbours: tiny strengths.
        Etc => s([0.2, 0.0, 0.0, 0.2, 0.0, 0.2], [0.15, 0.4, 0.1, 0.1, 0.4, 0.1]),
    }
}

/// The tongue a phoneme asks for: `(up, down, forward, curl)`, and how
/// firmly.
pub fn tongue_target(p: Ph) -> ([f32; 4], f32) {
    use Ph::*;
    match p {
        T | D | N => ([1.0, 0.0, 0.0, 0.0], 2.0),
        L => ([1.0, 0.0, 0.2, 0.0], 2.0),
        K | G | NG => ([0.6, 0.0, 0.0, 0.0], 1.5),
        TH | DH => ([0.0, 0.0, 1.0, 0.0], 3.0),
        R | ER => ([0.2, 0.0, 0.0, 1.0], 2.0),
        S | Z | SH | ZH | CH | JH => ([0.45, 0.0, 0.0, 0.0], 1.2),
        AA | AE | AH | EH | AY | AW => ([0.0, 0.6, 0.0, 0.0], 0.8),
        IY | IH | EY | Y => ([0.35, 0.0, 0.0, 0.0], 0.6),
        _ => ([0.0, 0.1, 0.0, 0.0], 0.3),
    }
}
