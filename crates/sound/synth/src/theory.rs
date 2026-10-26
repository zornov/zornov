// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Keys, scales and chords.

/// A scale.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Ionian.
    Major,
    /// Natural minor (Aeolian).
    Minor,
    /// Minor with a raised sixth.
    Dorian,
    /// Major with a flat seventh.
    Mixolydian,
    /// Minor with a flat second: dark and Spanish.
    Phrygian,
    /// Major with a raised fourth: wonder.
    Lydian,
    /// Minor with a raised seventh: noir and tension.
    HarmonicMinor,
}

impl Mode {
    /// Semitones of the seven scale degrees above the tonic.
    pub fn steps(self) -> [i32; 7] {
        match self {
            Mode::Major => [0, 2, 4, 5, 7, 9, 11],
            Mode::Minor => [0, 2, 3, 5, 7, 8, 10],
            Mode::Dorian => [0, 2, 3, 5, 7, 9, 10],
            Mode::Mixolydian => [0, 2, 4, 5, 7, 9, 10],
            Mode::Phrygian => [0, 1, 3, 5, 7, 8, 10],
            Mode::Lydian => [0, 2, 4, 6, 7, 9, 11],
            Mode::HarmonicMinor => [0, 2, 3, 5, 7, 8, 11],
        }
    }
    /// True for the modes with a minor third.
    pub fn is_minor(self) -> bool {
        matches!(self, Mode::Minor | Mode::Dorian | Mode::Phrygian | Mode::HarmonicMinor)
    }
    /// The name.
    pub fn name(self) -> &'static str {
        match self {
            Mode::Major => "major",
            Mode::Minor => "minor",
            Mode::Dorian => "dorian",
            Mode::Mixolydian => "mixolydian",
            Mode::Phrygian => "phrygian",
            Mode::Lydian => "lydian",
            Mode::HarmonicMinor => "harmonic minor",
        }
    }
}

/// A key: a tonic (0 = C … 11 = B) and a mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Key {
    /// Pitch class of the tonic.
    pub root: i32,
    /// The scale.
    pub mode: Mode,
}

const NAMES: [&str; 12] = ["C", "C#", "D", "Eb", "E", "F", "F#", "G", "Ab", "A", "Bb", "B"];

/// The name of a pitch class.
pub fn pc_name(pc: i32) -> &'static str {
    NAMES[pc.rem_euclid(12) as usize]
}

/// A pitch class from a letter name ("c", "F#", "Bb", "e flat").
pub fn parse_pc(text: &str) -> Option<i32> {
    let t = text.trim().to_lowercase();
    let mut chars = t.chars();
    let base = match chars.next()? {
        'c' => 0,
        'd' => 2,
        'e' => 4,
        'f' => 5,
        'g' => 7,
        'a' => 9,
        'b' => 11,
        _ => return None,
    };
    let rest: String = chars.collect();
    let rest = rest.trim();
    let acc = if rest.starts_with('#') || rest.starts_with("sharp") {
        1
    } else if rest.starts_with('b') && !rest.starts_with("be") || rest.starts_with("flat") {
        -1
    } else {
        0
    };
    Some((base + acc as i32).rem_euclid(12))
}

impl Key {
    /// Reads "D minor", "F# major", "Bb", "a dorian", "C".
    pub fn parse(text: &str, default_mode: Mode) -> Option<Key> {
        let t = text.trim();
        if t.is_empty() {
            return None;
        }
        let mut parts = t.split_whitespace();
        let first = parts.next()?;
        let (letter, attached) = {
            let mut it = first.chars();
            let l = it.next()?;
            (l, it.collect::<String>())
        };
        let rest_words: Vec<String> = parts.map(str::to_lowercase).collect();
        // "Bb", "F#", "Ebm", "Am"
        let mut name = letter.to_string();
        let mut tail = attached.to_lowercase();
        if tail.starts_with('#') || tail.starts_with('b') {
            name.push(tail.remove(0));
        }
        let mut words = rest_words;
        if !tail.is_empty() {
            words.insert(0, tail);
        }
        let root = parse_pc(&name)?;
        let mut mode = default_mode;
        for w in &words {
            mode = match w.as_str() {
                "minor" | "min" | "m" | "aeolian" => Mode::Minor,
                "major" | "maj" | "ionian" => Mode::Major,
                "dorian" => Mode::Dorian,
                "mixolydian" => Mode::Mixolydian,
                "phrygian" => Mode::Phrygian,
                "lydian" => Mode::Lydian,
                "harmonic" => Mode::HarmonicMinor,
                _ => mode,
            };
        }
        Some(Key { root, mode })
    }

    /// "D minor".
    pub fn name(&self) -> String {
        format!("{} {}", pc_name(self.root), self.mode.name())
    }

    /// Pitch classes of the scale.
    pub fn scale(&self) -> Vec<i32> {
        self.mode.steps().iter().map(|s| (self.root + s).rem_euclid(12)).collect()
    }
}

/// A chord quality.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Q {
    /// Major triad.
    Maj,
    /// Minor triad.
    Min,
    /// Dominant seventh.
    Dom7,
    /// Major seventh.
    Maj7,
    /// Minor seventh.
    Min7,
    /// Diminished triad.
    Dim,
    /// Half-diminished seventh.
    Hdim7,
    /// Suspended fourth.
    Sus4,
    /// Suspended second.
    Sus2,
    /// Minor sixth.
    Min6,
    /// Open fifth.
    Power,
}

impl Q {
    /// Semitones above the root.
    pub fn intervals(self) -> &'static [i32] {
        match self {
            Q::Maj => &[0, 4, 7],
            Q::Min => &[0, 3, 7],
            Q::Dom7 => &[0, 4, 7, 10],
            Q::Maj7 => &[0, 4, 7, 11],
            Q::Min7 => &[0, 3, 7, 10],
            Q::Dim => &[0, 3, 6],
            Q::Hdim7 => &[0, 3, 6, 10],
            Q::Sus4 => &[0, 5, 7],
            Q::Sus2 => &[0, 2, 7],
            Q::Min6 => &[0, 3, 7, 9],
            Q::Power => &[0, 7],
        }
    }
}

/// A chord: a root pitch class and a quality.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Chord {
    /// Root pitch class 0..11.
    pub root: i32,
    /// Quality.
    pub q: Q,
}

impl Chord {
    /// Pitch classes of the chord tones.
    pub fn pcs(&self) -> Vec<i32> {
        self.q.intervals().iter().map(|i| (self.root + i).rem_euclid(12)).collect()
    }
    /// The chord tone pitch classes, with the seventh (or sixth) added
    /// when the chord is a triad and `colour` is set.
    pub fn pcs_colour(&self, colour: bool) -> Vec<i32> {
        let mut v = self.pcs();
        if colour && v.len() == 3 {
            let seventh = match self.q {
                Q::Maj | Q::Sus4 | Q::Sus2 => 11,
                Q::Dim => 9,
                _ => 10,
            };
            v.push((self.root + seventh).rem_euclid(12));
        }
        v
    }
    /// The notes of the chord stacked upward from at or above `low`, as
    /// MIDI notes, `count` of them (cycling through the tones and octaves).
    pub fn stack(&self, low: i32, count: usize) -> Vec<i32> {
        let mut tones = self.pcs();
        tones.sort_by_key(|pc| (pc - low).rem_euclid(12));
        let mut out = Vec::new();
        let mut k = 0;
        while out.len() < count {
            let pc = tones[k % tones.len()];
            let base = low + (pc - low).rem_euclid(12);
            out.push(base + 12 * (k / tones.len()) as i32);
            k += 1;
        }
        out
    }
    /// The root as a MIDI note in the octave at or above `low`.
    pub fn root_at(&self, low: i32) -> i32 {
        low + (self.root - low).rem_euclid(12)
    }
}

/// The MIDI note with pitch class `pc` nearest to `near`.
pub fn nearest(pc: i32, near: i32) -> i32 {
    let base = near - (near - pc).rem_euclid(12);
    if near - base <= base + 12 - near { base } else { base + 12 }
}

/// A MIDI note from a name ("C4", "F#3", "Bb2", "60"): middle C is C4.
pub fn parse_pitch(text: &str) -> Option<f32> {
    let t = text.trim();
    if let Ok(v) = t.parse::<f32>() {
        return (0.0..=127.5).contains(&v).then_some(v);
    }
    let mut chars = t.chars().peekable();
    let letter = chars.next()?.to_ascii_lowercase();
    let base = match letter {
        'c' => 0,
        'd' => 2,
        'e' => 4,
        'f' => 5,
        'g' => 7,
        'a' => 9,
        'b' => 11,
        _ => return None,
    };
    let mut acc = 0;
    while let Some(&c) = chars.peek() {
        match c {
            '#' => acc += 1,
            'b' | '♭' => acc -= 1,
            '♯' => acc += 1,
            _ => break,
        }
        chars.next();
    }
    let rest: String = chars.collect();
    let octave: i32 = if rest.is_empty() { 4 } else { rest.parse().ok()? };
    Some(((octave + 1) * 12 + base + acc) as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_and_pitches_parse() {
        assert_eq!(Key::parse("D minor", Mode::Major), Some(Key { root: 2, mode: Mode::Minor }));
        assert_eq!(Key::parse("F# major", Mode::Minor), Some(Key { root: 6, mode: Mode::Major }));
        assert_eq!(Key::parse("Bb", Mode::Major).unwrap().root, 10);
        assert_eq!(Key::parse("Am", Mode::Major), Some(Key { root: 9, mode: Mode::Minor }));
        assert_eq!(Key::parse("E dorian", Mode::Major).unwrap().mode, Mode::Dorian);
        assert_eq!(parse_pitch("C4"), Some(60.0));
        assert_eq!(parse_pitch("A4"), Some(69.0));
        assert_eq!(parse_pitch("F#3"), Some(54.0));
        assert_eq!(parse_pitch("Bb2"), Some(46.0));
        assert_eq!(parse_pitch("67"), Some(67.0));
        assert_eq!(nearest(0, 61), 60);
        assert_eq!(nearest(11, 60), 59);
    }

    #[test]
    fn chords_stack_upward() {
        let c = Chord { root: 0, q: Q::Maj7 };
        assert_eq!(c.stack(48, 4), vec![48, 52, 55, 59]);
        assert_eq!(c.stack(52, 5), vec![52, 55, 59, 60, 64]);
    }
}
