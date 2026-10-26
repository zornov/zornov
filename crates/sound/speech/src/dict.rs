// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Pronunciations: CMUdict (Carnegie Mellon University, BSD-style licence,
//! see ATTRIBUTIONS.md) bundled in the program, with letter-to-sound
//! rules for words it lacks.

use crate::phoneme::Ph;
use std::sync::OnceLock;

static RAW: &str = include_str!("../data/cmudict.dict");

fn map() -> &'static std::collections::HashMap<&'static str, &'static str> {
    static M: OnceLock<std::collections::HashMap<&'static str, &'static str>> = OnceLock::new();
    M.get_or_init(|| {
        let mut m = std::collections::HashMap::with_capacity(135_000);
        for l in RAW.lines().filter(|l| !l.is_empty() && !l.starts_with(';') && !l.starts_with('#')) {
            if let Some((k, v)) = l.split_once(' ') {
                // Variants ("word(2)") come after the first pronunciation.
                m.entry(k).or_insert(v);
            }
        }
        m
    })
}

fn parse(line: &str) -> Vec<(Ph, u8)> {
    line.split(' ').filter_map(Ph::parse).collect()
}

/// The first pronunciation of a word (letters and apostrophes, any case),
/// from the dictionary alone.
pub fn lookup(word: &str) -> Option<Vec<(Ph, u8)>> {
    let w = word.to_lowercase();
    map().get(w.as_str()).map(|l| parse(l)).filter(|p| !p.is_empty())
}

/// How many words the dictionary holds.
pub fn size() -> usize {
    map().len()
}

/// The pronunciation of a word: the dictionary, else a compound of two
/// dictionary words, else letter-to-sound rules. Numbers are spoken.
pub fn pronounce(word: &str) -> Vec<(Ph, u8)> {
    let clean: String = word.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '\'').collect::<String>().to_lowercase();
    let clean = clean.trim_matches('\'').to_owned();
    if clean.is_empty() {
        return Vec::new();
    }
    if clean.chars().all(|c| c.is_ascii_digit()) {
        let spoken = crate::lts::number_words(&clean);
        return spoken.split(' ').flat_map(pronounce).collect();
    }
    if let Some(p) = lookup(&clean) {
        return p;
    }
    // Plurals and possessives of known words.
    if let Some(stem) = clean.strip_suffix("'s").or_else(|| clean.strip_suffix('s')).filter(|s| s.len() >= 2)
        && let Some(mut p) = lookup(stem)
    {
        let last = p.last().map(|x| x.0);
        let tail = match last {
            Some(Ph::S | Ph::Z | Ph::SH | Ph::ZH | Ph::CH | Ph::JH) => vec![(Ph::IH, 0), (Ph::Z, 0)],
            Some(Ph::P | Ph::T | Ph::K | Ph::F | Ph::TH) => vec![(Ph::S, 0)],
            _ => vec![(Ph::Z, 0)],
        };
        p.extend(tail);
        return p;
    }
    // Two known words run together.
    if clean.len() >= 6 {
        for cut in (3..clean.len() - 2).rev() {
            if let (Some(mut a), Some(b)) = (lookup(&clean[..cut]), lookup(&clean[cut..])) {
                a.extend(b);
                return a;
            }
        }
    }
    crate::lts::letters_to_sound(&clean)
}
