// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Letter-to-sound rules for words the dictionary lacks: names, new
//! words, typos. Longest letter group first, vowels by what follows them
//! (a vowel, consonant, then a final "e" makes it long).

use crate::phoneme::Ph::{self, *};

fn is_v(c: u8) -> bool {
    matches!(c, b'a' | b'e' | b'i' | b'o' | b'u')
}

/// Spells a whole number in words ("42" -> "forty two").
pub fn number_words(digits: &str) -> String {
    let n: u64 = digits.parse().unwrap_or(0);
    if digits.len() > 12 {
        return digits.chars().map(|c| ones(c as u64 - '0' as u64)).collect::<Vec<_>>().join(" ");
    }
    if n == 0 {
        return "zero".into();
    }
    fn ones(n: u64) -> &'static str {
        ["zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten", "eleven", "twelve", "thirteen", "fourteen", "fifteen", "sixteen", "seventeen", "eighteen", "nineteen"][n as usize]
    }
    fn below_1000(n: u64, out: &mut Vec<String>) {
        if n >= 100 {
            out.push(ones(n / 100).into());
            out.push("hundred".into());
        }
        let r = n % 100;
        if r >= 20 {
            out.push(["", "", "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety"][(r / 10) as usize].into());
            if r % 10 > 0 {
                out.push(ones(r % 10).into());
            }
        } else if r > 0 {
            out.push(ones(r).into());
        }
    }
    let mut out = Vec::new();
    for (div, name) in [(1_000_000_000, "billion"), (1_000_000, "million"), (1_000, "thousand")] {
        if n / div % 1000 > 0 {
            below_1000(n / div % 1000, &mut out);
            out.push(name.into());
        }
    }
    below_1000(n % 1000, &mut out);
    out.join(" ")
}

/// Phonemes for a lower-case word by rule.
pub fn letters_to_sound(w: &str) -> Vec<(Ph, u8)> {
    let b: Vec<u8> = w.bytes().filter(|c| c.is_ascii_lowercase()).collect();
    let n = b.len();
    let mut out: Vec<Ph> = Vec::new();
    let at = |i: usize| -> u8 { if i < n { b[i] } else { 0 } };
    let starts = |i: usize, s: &str| b[i..].starts_with(s.as_bytes());
    // Whether the vowel at i is followed by consonant(s) then a final silent e.
    let magic = |i: usize| -> bool {
        let mut j = i + 1;
        let mut cons = 0;
        while j < n && !is_v(b[j]) && b[j] != b'y' {
            cons += 1;
            j += 1;
        }
        cons >= 1 && cons <= 2 && j == n - 1 && b[j] == b'e'
    };
    let mut i = 0;
    let mut vowels_seen = 0;
    while i < n {
        let c = b[i];
        let prev = if i > 0 { b[i - 1] } else { 0 };
        let next = at(i + 1);
        let (ph, adv): (Vec<Ph>, usize) = match c {
            b'a' => {
                vowels_seen += 1;
                if starts(i, "ai") || starts(i, "ay") {
                    (vec![EY], 2)
                } else if starts(i, "au") || starts(i, "aw") {
                    (vec![AO], 2)
                } else if starts(i, "ar") && i + 2 >= n || starts(i, "ar") && !is_v(at(i + 2)) {
                    (vec![AA, R], 2)
                } else if magic(i) {
                    (vec![EY], 1)
                } else if starts(i, "al") && !is_v(at(i + 2)) && at(i + 2) != b'l' {
                    (vec![AO, L], 2)
                } else if i == 0 && n > 2 && !is_v(next) { (vec![AH], 1) } else { (vec![AE], 1) }
            }
            b'e' => {
                if i == n - 1 && i > 0 && vowels_seen > 0 && !is_v(prev) {
                    (vec![], 1) // silent final e
                } else if starts(i, "ee") || starts(i, "ea") {
                    vowels_seen += 1;
                    (vec![IY], 2)
                } else if starts(i, "ei") || starts(i, "ey") {
                    vowels_seen += 1;
                    (vec![EY], 2)
                } else if starts(i, "ew") {
                    vowels_seen += 1;
                    (vec![UW], 2)
                } else if starts(i, "er") && (i + 2 >= n || !is_v(at(i + 2))) {
                    vowels_seen += 1;
                    (vec![ER], 2)
                } else if starts(i, "es") && i + 2 == n && i > 0 {
                    (vec![IH, Z], 2)
                } else if starts(i, "ed") && i + 2 == n && i > 0 && vowels_seen > 0 {
                    (if matches!(prev, b't' | b'd') { vec![IH, D] } else if matches!(prev, b'p' | b'k' | b'f' | b's' | b'x') { vec![T] } else { vec![D] }, 2)
                } else {
                    vowels_seen += 1;
                    if magic(i) { (vec![IY], 1) } else { (vec![EH], 1) }
                }
            }
            b'i' => {
                vowels_seen += 1;
                if starts(i, "igh") {
                    (vec![AY], 3)
                } else if starts(i, "ie") && i + 2 == n {
                    (vec![AY], 2)
                } else if starts(i, "ie") {
                    (vec![IY], 2)
                } else if starts(i, "ir") {
                    (vec![ER], 2)
                } else if starts(i, "ing") && i + 3 == n {
                    (vec![IH, NG], 3)
                } else if magic(i) || (i == n - 1) {
                    (vec![AY], 1)
                } else {
                    (vec![IH], 1)
                }
            }
            b'o' => {
                vowels_seen += 1;
                if starts(i, "oo") {
                    (if matches!(at(i + 2), b'k') { vec![UH] } else { vec![UW] }, 2)
                } else if starts(i, "oa") || starts(i, "oe") {
                    (vec![OW], 2)
                } else if starts(i, "ou") {
                    (vec![AW], 2)
                } else if starts(i, "ow") {
                    (if i + 2 >= n { vec![OW] } else { vec![AW] }, 2)
                } else if starts(i, "oi") || starts(i, "oy") {
                    (vec![OY], 2)
                } else if starts(i, "or") {
                    (vec![AO, R], 2)
                } else if magic(i) || i == n - 1 {
                    (vec![OW], 1)
                } else {
                    (vec![AA], 1)
                }
            }
            b'u' => {
                vowels_seen += 1;
                if starts(i, "ur") {
                    (vec![ER], 2)
                } else if magic(i) {
                    (vec![Y, UW], 1)
                } else if prev == b'q' || starts(i, "ue") && i + 2 == n {
                    (vec![UW], if starts(i, "ue") { 2 } else { 1 })
                } else {
                    (vec![AH], 1)
                }
            }
            b'y' => {
                if i == 0 {
                    (vec![Y], 1)
                } else if i == n - 1 {
                    (if vowels_seen <= 1 && n <= 3 { vec![AY] } else { vec![IY] }, 1)
                } else {
                    (vec![IH], 1)
                }
            }
            b'b' => (if next == b'b' { vec![B] } else if i == n - 1 && prev == b'm' { vec![] } else { vec![B] }, if next == b'b' { 2 } else { 1 }),
            b'c' => {
                if starts(i, "ch") {
                    (vec![CH], 2)
                } else if starts(i, "ck") {
                    (vec![K], 2)
                } else if matches!(next, b'e' | b'i' | b'y') {
                    (vec![S], 1)
                } else {
                    (vec![K], 1)
                }
            }
            b'd' => {
                if starts(i, "dge") { (vec![JH], 2) } else if next == b'd' { (vec![D], 2) } else { (vec![D], 1) }
            }
            b'f' => (vec![F], if next == b'f' { 2 } else { 1 }),
            b'g' => {
                if starts(i, "gh") {
                    (if i == 0 { vec![G] } else { vec![] }, 2)
                } else if starts(i, "gn") && i == 0 {
                    (vec![], 1)
                } else if matches!(next, b'e' | b'i' | b'y') && i + 1 < n {
                    (vec![JH], 1)
                } else if next == b'g' {
                    (vec![G], 2)
                } else {
                    (vec![G], 1)
                }
            }
            b'h' => (if matches!(prev, b'c' | b's' | b't' | b'p' | b'g' | b'w') { vec![] } else { vec![HH] }, 1),
            b'j' => (vec![JH], 1),
            b'k' => {
                if i == 0 && next == b'n' { (vec![], 1) } else { (vec![K], if next == b'k' { 2 } else { 1 }) }
            }
            b'l' => (vec![L], if next == b'l' { 2 } else { 1 }),
            b'm' => (vec![M], if next == b'm' { 2 } else { 1 }),
            b'n' => {
                if starts(i, "ng") { (vec![NG], 2) } else if starts(i, "nk") { (vec![NG, K], 2) } else { (vec![N], if next == b'n' { 2 } else { 1 }) }
            }
            b'p' => {
                if starts(i, "ph") { (vec![F], 2) } else { (vec![P], if next == b'p' { 2 } else { 1 }) }
            }
            b'q' => (vec![K, W], if next == b'u' { 2 } else { 1 }),
            b'r' => (vec![R], if next == b'r' { 2 } else { 1 }),
            b's' => {
                if starts(i, "sh") {
                    (vec![SH], 2)
                } else if starts(i, "ssion") || starts(i, "sion") {
                    (vec![ZH], 1)
                } else if next == b's' {
                    (vec![S], 2)
                } else if i > 0 && is_v(prev) && is_v(next) {
                    (vec![Z], 1)
                } else {
                    (vec![S], 1)
                }
            }
            b't' => {
                if starts(i, "th") {
                    (if i == 0 && matches!(w, "the" | "this" | "that" | "these" | "those" | "then" | "them" | "they" | "there") { vec![DH] } else { vec![TH] }, 2)
                } else if starts(i, "tion") || starts(i, "tial") {
                    (vec![SH], 1)
                } else if starts(i, "tch") {
                    (vec![CH], 3)
                } else {
                    (vec![T], if next == b't' { 2 } else { 1 })
                }
            }
            b'v' => (vec![V], 1),
            b'w' => {
                if starts(i, "wr") && i == 0 { (vec![], 1) } else if starts(i, "wh") { (vec![W], 2) } else { (vec![W], 1) }
            }
            b'x' => (if i == 0 { vec![Z] } else { vec![K, S] }, 1),
            b'z' => (vec![Z], if next == b'z' { 2 } else { 1 }),
            _ => (vec![], 1),
        };
        out.extend(ph);
        i += adv.max(1);
    }
    // First vowel of a longer word gets primary stress.
    let mut stressed = false;
    out.into_iter()
        .map(|p| {
            if p.is_vowel() && !stressed {
                stressed = true;
                (p, 1)
            } else {
                (p, 0)
            }
        })
        .collect()
}
