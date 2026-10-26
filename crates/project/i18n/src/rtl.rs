// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Right-to-left text for egui.
//!
//! egui shapes each run of text with HarfBuzz (letters join, ligatures form,
//! marks attach, and the glyphs of a word come out right to left) but it
//! lays the runs of a line out left to right, in the order they are written,
//! with no bidirectional reordering. For Arabic and Persian that means every
//! word is right but a sentence reads from its first word on the *left*, and
//! numbers and Latin names sit on the wrong side.
//!
//! [`display`] fixes the order of the words: it applies the Unicode
//! bidirectional algorithm to find which words are right to left, and puts
//! the *words* (never the letters inside a word) in visual order. The text
//! it returns is still made of ordinary logical words, so HarfBuzz keeps
//! doing the shaping and no presentation-form substitution is needed.
//!
//! Brackets that end up in a right-to-left run are swapped (`(` and `)`), as
//! UAX #9 rule L4 asks, because their position was reversed.
//!
//! The result is for drawing only. Text given to a screen reader must be the
//! logical text, so widgets pass the untransformed string to AccessKit.

use std::borrow::Cow;
use unicode_bidi::{BidiInfo, Level};

/// Is `c` in the Arabic script blocks?
pub fn is_arabic(c: char) -> bool {
    matches!(c as u32, 0x0600..=0x06FF | 0x0750..=0x077F | 0x08A0..=0x08FF | 0xFB50..=0xFDFF | 0xFE70..=0xFEFF)
}

/// Does the text contain any character that is written right to left?
pub fn has_rtl(s: &str) -> bool {
    s.chars().any(|c| is_arabic(c) || matches!(c as u32, 0x0590..=0x05FF | 0x200F | 0x202B | 0x202E | 0x2067))
}

fn mirror(c: char) -> char {
    match c {
        '(' => ')',
        ')' => '(',
        '[' => ']',
        ']' => '[',
        '{' => '}',
        '}' => '{',
        '<' => '>',
        '>' => '<',
        '«' => '»',
        '»' => '«',
        '‹' => '›',
        '›' => '‹',
        c => c,
    }
}

/// Characters that belong inside a word: letters, digits, combining marks and
/// the joiners Persian writes inside words.
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c as u32, 0x200C | 0x200D | 0x0300..=0x036F | 0x064B..=0x065F | 0x0670 | 0x06D6..=0x06ED | 0x0640)
}

/// One word, or one other character (a space, a bracket…), with its level.
struct Token {
    text: String,
    level: u8,
}

fn tokens(line: &str, levels: &[Level]) -> Vec<Token> {
    // `levels` is per byte of `line`.
    let mut out: Vec<Token> = Vec::new();
    let mut word = false;
    for (i, c) in line.char_indices() {
        let level = levels[i].number();
        // A word ends where its direction level changes (`abc123`).
        let join = word && is_word_char(c) && out.last().is_some_and(|t| t.level == level);
        if join {
            out.last_mut().unwrap().text.push(c);
        } else {
            out.push(Token { text: c.to_string(), level });
        }
        word = is_word_char(c);
    }
    out
}

/// UAX #9 rule L2 on tokens: from the highest level down to the lowest odd
/// one, reverse every maximal run at or above that level.
fn reorder(tokens: &mut [Token]) {
    let max = tokens.iter().map(|t| t.level).max().unwrap_or(0);
    let min_odd = tokens.iter().map(|t| t.level).filter(|l| l % 2 == 1).min();
    let Some(min_odd) = min_odd else { return };
    let mut level = max;
    while level >= min_odd {
        let mut i = 0;
        while i < tokens.len() {
            if tokens[i].level >= level {
                let mut j = i;
                while j < tokens.len() && tokens[j].level >= level {
                    j += 1;
                }
                tokens[i..j].reverse();
                i = j;
            } else {
                i += 1;
            }
        }
        if level == 0 {
            break;
        }
        level -= 1;
    }
}

/// What to hand to egui for `s`. `rtl_ui` is the interface direction: it
/// sets the paragraph direction, so numbers and punctuation in a text with
/// no strong characters of its own, and Latin names inside Arabic, land on
/// the right side. Text with no right-to-left characters is returned as is.
pub fn display(s: &str, rtl_ui: bool) -> Cow<'_, str> {
    if !has_rtl(s) {
        return Cow::Borrowed(s);
    }
    let base = if rtl_ui { Level::rtl() } else { Level::ltr() };
    let mut out = String::with_capacity(s.len());
    for (n, line) in s.split('\n').enumerate() {
        if n > 0 {
            out.push('\n');
        }
        if !has_rtl(line) {
            out.push_str(line);
            continue;
        }
        let info = BidiInfo::new(line, Some(base));
        let Some(para) = info.paragraphs.first() else {
            out.push_str(line);
            continue;
        };
        let levels = &info.levels[para.range.clone()];
        let mut toks = tokens(line, levels);
        reorder(&mut toks);
        for t in toks {
            if t.level % 2 == 1 && t.text.chars().count() == 1 {
                out.extend(t.text.chars().map(mirror));
            } else {
                out.push_str(&t.text);
            }
        }
    }
    Cow::Owned(out)
}

/// Break `s` into lines that each fit (`fits` measures a candidate line in
/// its display form), then convert every line. Use for wrapped paragraphs:
/// wrapping the already reordered text would break lines at the wrong end.
pub fn wrap(s: &str, rtl_ui: bool, fits: impl Fn(&str) -> bool) -> String {
    let mut lines: Vec<String> = Vec::new();
    for para in s.split('\n') {
        let mut cur = String::new();
        for word in para.split(' ') {
            let cand = if cur.is_empty() { word.to_owned() } else { format!("{cur} {word}") };
            if cur.is_empty() || fits(&display(&cand, rtl_ui)) {
                cur = cand;
            } else {
                lines.push(std::mem::replace(&mut cur, word.to_owned()));
            }
        }
        lines.push(cur);
    }
    lines.iter().map(|l| display(l, rtl_ui).into_owned()).collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(s: &str) -> Vec<&str> {
        s.split(' ').collect()
    }

    #[test]
    fn latin_is_untouched() {
        assert!(matches!(display("Export 3 clips", false), Cow::Borrowed(_)));
        assert!(matches!(display("Export 3 clips", true), Cow::Borrowed(_)));
    }

    #[test]
    fn words_of_an_rtl_sentence_are_reversed_but_not_their_letters() {
        // "salaam donya": the first word must end up on the right.
        let d = display("سلام دنیا", true);
        assert_eq!(words(&d), ["دنیا", "سلام"]);
        // Letters inside each word keep their logical order (HarfBuzz joins them).
        assert!(d.contains("سلام") && d.contains("دنیا"));
    }

    #[test]
    fn a_single_word_is_unchanged() {
        assert_eq!(display("سلام", true), "سلام");
        assert_eq!(display("سلام", false), "سلام");
    }

    #[test]
    fn numbers_stay_left_to_right_and_land_on_the_correct_side() {
        // Logical: file 123 → visual (left to right): 123 file.
        assert_eq!(display("ملف 123", true), "123 ملف");
        // A number in the middle of a sentence keeps its place among the words.
        assert_eq!(words(&display("ملف 123 ب", true)), ["ب", "123", "ملف"]);
    }

    #[test]
    fn latin_names_inside_arabic_read_left_to_right() {
        // "Studio" first in an RTL paragraph is rightmost; the Arabic goes to its left.
        assert_eq!(display("Studio سلام", true), "سلام Studio");
        assert_eq!(display("سلام Studio", true), "Studio سلام");
        // Two Latin words stay in reading order.
        assert_eq!(display("ملف Rough Cut", true), "Rough Cut ملف");
    }

    #[test]
    fn brackets_around_several_words_are_mirrored() {
        assert_eq!(display("(ب ج)", true), "(ج ب)");
        // A single bracketed word looks the same either way.
        assert_eq!(display("(ب)", true), "(ب)");
    }

    #[test]
    fn marks_and_joiners_stay_with_their_word() {
        let d = display("سِنْتِر پایه", true);
        assert_eq!(words(&d), ["پایه", "سِنْتِر"]);
        // Persian writes ZWNJ inside words.
        let d = display("سیب‌زمینی خوب", true);
        assert_eq!(words(&d), ["خوب", "سیب‌زمینی"]);
    }

    #[test]
    fn lines_are_handled_one_by_one() {
        let d = display("اب جد\nه و", true);
        let lines: Vec<_> = d.split('\n').collect();
        assert_eq!(lines, ["جد اب", "و ه"]);
    }

    #[test]
    fn an_ltr_interface_still_reorders_rtl_text() {
        // A paragraph that starts with Arabic is right to left on its own.
        assert_eq!(words(&display("سلام دنیا", false)), ["دنیا", "سلام"]);
    }

    #[test]
    fn wrap_breaks_logical_words_then_orders_each_line() {
        let text = "ابج ابج ابج";
        let w = wrap(text, true, |l| l.chars().count() <= 7);
        let lines: Vec<_> = w.split('\n').collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].chars().count(), 7, "two words on the first line");
        assert_eq!(lines[1].chars().count(), 3);
    }

    #[test]
    fn punctuation_and_digits_in_rtl_text() {
        // Arabic comma and a trailing full stop go with the sentence.
        let d = display("نعم، لا.", true);
        assert!(d.starts_with('.'), "the full stop ends the sentence, so it is leftmost: {d}");
    }
}
