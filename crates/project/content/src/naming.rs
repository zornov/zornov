// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Human naming rules.
//!
//! Every user-visible string in the library is short and human: "Set name
//! 01", never a file name, hash, codepoint or generator prompt. This module
//! turns raw upstream titles into such names and checks the result.

/// Something wrong with a user-visible name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NameProblem {
    Empty,
    /// Longer than [`MAX_NAME_LEN`] characters.
    TooLong(usize),
    /// Contains a path separator or a file extension.
    LooksLikeFileName,
    /// A long run of hex digits or a mixed letter/digit token: a hash or id.
    LooksLikeHash(String),
    /// A Unicode codepoint such as `1F600` or `1F468-200D-1F469`.
    LooksLikeCodepoint(String),
    /// Generator prompt leftovers (`--ar 1:1`, "midjourney", "prompt").
    LooksLikePrompt(String),
    /// Underscores, double spaces or stray whitespace.
    Untidy,
}

/// Longest allowed user-visible name.
pub const MAX_NAME_LEN: usize = 48;

/// Checks a user-visible name against the naming rules.
pub fn check_name(name: &str) -> Vec<NameProblem> {
    let mut out = Vec::new();
    if name.trim().is_empty() {
        out.push(NameProblem::Empty);
        return out;
    }
    let n = name.chars().count();
    if n > MAX_NAME_LEN {
        out.push(NameProblem::TooLong(n));
    }
    if name.contains('/') || name.contains('\\') || has_extension(name) {
        out.push(NameProblem::LooksLikeFileName);
    }
    if name.contains('_') || name.contains("  ") || name != name.trim() {
        out.push(NameProblem::Untidy);
    }
    let lower = name.to_ascii_lowercase();
    for marker in ["--ar", "--v ", "--style", "--niji", "midjourney", "prompt:", "::", "--s ", "--q ", "--chaos", "upscaled", "dall-e", "stable diffusion"] {
        if lower.contains(marker) {
            out.push(NameProblem::LooksLikePrompt(marker.trim().into()));
            break;
        }
    }
    if name.split_whitespace().count() > 9 {
        out.push(NameProblem::LooksLikePrompt("too many words".into()));
    }
    for tok in name.split(|c: char| c.is_whitespace() || c == '-' || c == '(' || c == ')' || c == ',') {
        if tok.is_empty() {
            continue;
        }
        if is_codepoint_token(tok) {
            out.push(NameProblem::LooksLikeCodepoint(tok.into()));
            continue;
        }
        if is_hash_token(tok) {
            out.push(NameProblem::LooksLikeHash(tok.into()));
        }
    }
    out
}

fn has_extension(s: &str) -> bool {
    let lower = s.to_ascii_lowercase();
    [".png", ".jpg", ".jpeg", ".svg", ".mp4", ".mov", ".ogg", ".wav", ".mp3", ".flac", ".ttf", ".otf", ".webp", ".gif"]
        .iter()
        .any(|e| lower.ends_with(e))
}

/// `1F600`, `1F468-200D-1F469`, `FE0F`: four or five upper-case hex digits
/// with a digit and, for four-digit tokens, at least two hex letters (so a
/// product name like `B612` passes).
fn is_codepoint_token(tok: &str) -> bool {
    let letters = tok.chars().filter(|c| ('A'..='F').contains(c)).count();
    (4..=5).contains(&tok.len())
        && tok.chars().all(|c| c.is_ascii_digit() || ('A'..='F').contains(&c))
        && tok.chars().any(|c| c.is_ascii_digit())
        && letters >= 1
        && (tok.len() == 5 || letters >= 2)
}

/// Eight or more hex digits, or a longer token mixing letters and digits
/// without a plain-word shape ("abc123def").
fn is_hash_token(tok: &str) -> bool {
    let len = tok.len();
    if len >= 8 && tok.chars().all(|c| c.is_ascii_hexdigit()) {
        return true;
    }
    if len >= 10 {
        let digits = tok.chars().filter(|c| c.is_ascii_digit()).count();
        let letters = tok.chars().filter(|c| c.is_ascii_alphabetic()).count();
        // Words with a trailing number ("Sticker01") are fine; interleaved
        // letters and digits are not.
        let transitions = tok
            .chars()
            .zip(tok.chars().skip(1))
            .filter(|(a, b)| a.is_ascii_digit() != b.is_ascii_digit())
            .count();
        return digits >= 3 && letters >= 3 && transitions >= 3;
    }
    false
}

/// Sentence case: first letter upper, the rest as written but lowercased
/// unless the word is an initialism (`UI`, `3D`, `CJK`) or a single letter.
pub fn sentence_case(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for (i, word) in s.split_whitespace().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        let keep = word.chars().filter(|c| c.is_alphabetic()).count() <= 3
            && word.chars().any(|c| c.is_ascii_uppercase())
            && word.chars().all(|c| !c.is_lowercase());
        if i == 0 {
            let mut chars = word.chars();
            if let Some(first) = chars.next() {
                out.extend(first.to_uppercase());
                if keep {
                    out.push_str(chars.as_str());
                } else {
                    out.push_str(&chars.as_str().to_lowercase());
                }
            }
        } else if keep || word.chars().next().is_some_and(|c| !c.is_alphabetic()) {
            out.push_str(word);
        } else {
            out.push_str(&word.to_lowercase());
        }
    }
    out
}

/// Gentle case for upstream titles: capitalises the first letter, keeps
/// capitals the author wrote ("Juhani Junkala"), and only lowercases
/// shouting words (all caps, more than three letters).
pub fn gentle_case(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for (i, word) in s.split_whitespace().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        let letters = word.chars().filter(|c| c.is_alphabetic()).count();
        let shouting = letters > 3 && word.chars().all(|c| !c.is_lowercase());
        let w = if shouting { word.to_lowercase() } else { word.to_string() };
        if i == 0 {
            let mut chars = w.chars();
            if let Some(first) = chars.next() {
                out.extend(first.to_uppercase());
                out.push_str(chars.as_str());
            }
        } else {
            out.push_str(&w);
        }
    }
    out
}

/// Title case: every word capitalised (used for set names).
pub fn title_case(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for (i, word) in s.split_whitespace().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        let mut chars = word.chars();
        if let Some(first) = chars.next() {
            out.extend(first.to_uppercase());
            let rest = chars.as_str();
            if rest.chars().all(|c| c.is_uppercase() || !c.is_alphabetic()) && rest.len() <= 3 {
                out.push_str(rest);
            } else {
                out.push_str(&rest.to_lowercase());
            }
        }
    }
    out
}

/// Turns an upstream identifier-ish title into a readable name:
/// `cards-pack-open-1` → "Cards pack open 1", `coin1` → "Coin 1",
/// `emote__` → "Emote", `Juhani [Chiptune] 1. Stage 1` → "Juhani 1. Stage 1".
pub fn humanise(raw: &str) -> String {
    // Drop bracketed asides and anything that reads like a file name.
    let mut s = String::with_capacity(raw.len());
    let mut depth = 0i32;
    for c in raw.chars() {
        match c {
            '[' | '{' => depth += 1,
            ']' | '}' => depth -= 1,
            _ if depth > 0 => {}
            '_' | '-' | '.' | '/' | '\\' => s.push(' '),
            _ => s.push(c),
        }
    }
    if let Some(dot) = raw.rfind('.') {
        let ext = &raw[dot + 1..];
        if (2..=4).contains(&ext.len()) && ext.chars().all(|c| c.is_ascii_alphanumeric()) && has_extension(raw) {
            s = humanise(&raw[..dot]);
        }
    }
    // Split "coin1" into "coin 1" and "8bit" stays "8bit".
    let mut spaced = String::with_capacity(s.len() + 4);
    let chars: Vec<char> = s.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        if i > 0 && c.is_ascii_digit() && chars[i - 1].is_ascii_lowercase() {
            spaced.push(' ');
        }
        spaced.push(c);
    }
    let words: Vec<&str> = spaced.split_whitespace().collect();
    let mut name = gentle_case(&words.join(" "));
    if name.chars().count() > MAX_NAME_LEN {
        let mut cut = String::new();
        for w in name.split_whitespace() {
            if cut.chars().count() + w.chars().count() + 1 > MAX_NAME_LEN {
                break;
            }
            if !cut.is_empty() {
                cut.push(' ');
            }
            cut.push_str(w);
        }
        if !cut.is_empty() {
            name = cut;
        }
    }
    name
}

/// Zero-padded numbering as in "Set name 01": two digits up to 99, three
/// beyond.
pub fn numbered(set: &str, n: usize, total: usize) -> String {
    if total >= 100 { format!("{set} {n:03}") } else { format!("{set} {n:02}") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn good_names_pass() {
        for n in ["Golden Retriever 03", "Neon Frame 2", "Grinning face", "Cards pack open 1", "Animal Memes 001", "3D glasses", "A b 2"] {
            assert!(check_name(n).is_empty(), "{n}: {:?}", check_name(n));
        }
    }

    #[test]
    fn bad_names_fail() {
        assert!(check_name("1F600").contains(&NameProblem::LooksLikeCodepoint("1F600".into())));
        assert!(check_name("1F468-200D-1F469").iter().any(|p| matches!(p, NameProblem::LooksLikeCodepoint(_))));
        assert!(check_name("img 3f9a8c7d2b").iter().any(|p| matches!(p, NameProblem::LooksLikeHash(_))));
        assert!(check_name("cat sticker --ar 1:1 --v 6").iter().any(|p| matches!(p, NameProblem::LooksLikePrompt(_))));
        assert!(check_name("cute_cat").contains(&NameProblem::Untidy));
        assert!(check_name("cat.png").contains(&NameProblem::LooksLikeFileName));
        assert!(check_name("").contains(&NameProblem::Empty));
        assert!(check_name("a cute cat sitting on a sofa wearing a red hat at sunset").iter().any(|p| matches!(p, NameProblem::LooksLikePrompt(_))));
    }

    #[test]
    fn humanise_cleans_identifiers() {
        assert_eq!(humanise("cards-pack-open-1"), "Cards pack open 1");
        assert_eq!(humanise("coin1"), "Coin 1");
        assert_eq!(humanise("emote__"), "Emote");
        assert_eq!(humanise("emote  "), "Emote");
        assert_eq!(humanise("1st place medal"), "1st place medal");
        assert_eq!(humanise("75 CC0 breaking / falling / hit sfx"), "75 CC0 breaking falling hit sfx");
        assert_eq!(humanise("Fantozzi's Footsteps (Grass/Sand & Stone)"), "Fantozzi's Footsteps (Grass Sand & Stone)");
        assert!(check_name("B612 Mono").is_empty());
        assert!(check_name("Prompt").is_empty());
        assert!(check_name("Jumpland 44100").is_empty());
        assert_eq!(humanise("grinning face with big eyes"), "Grinning face with big eyes");
        assert_eq!(humanise("Juhani Junkala [Chiptune Adventures] 1. Stage 1"), "Juhani Junkala 1 Stage 1");
        assert_eq!(humanise("LASER SHOT big"), "Laser shot big");
        assert_eq!(gentle_case("the Ice Giants"), "The Ice Giants");
        assert_eq!(humanise("footstep_grass_000.ogg"), "Footstep grass 000");
        assert!(humanise("cute_cat").chars().count() <= MAX_NAME_LEN);
    }

    #[test]
    fn cases() {
        assert_eq!(sentence_case("Animal Memes"), "Animal memes");
        assert_eq!(sentence_case("UI & Clicks"), "UI & clicks");
        assert_eq!(sentence_case("fluent 3D"), "Fluent 3D");
        assert_eq!(title_case("animal memes"), "Animal Memes");
        assert_eq!(numbered("Neon Frame", 2, 12), "Neon Frame 02");
        assert_eq!(numbered("Neon Frame", 2, 120), "Neon Frame 002");
    }
}

/// Every item whose user-visible strings break the rules: `(id, text, problems)`.
pub fn audit(index: &crate::item::Index) -> Vec<(String, String, Vec<NameProblem>)> {
    let mut out = Vec::new();
    for it in &index.items {
        for text in [&it.name, &it.set, &it.category] {
            let problems = check_name(text);
            if !problems.is_empty() {
                out.push((it.id.clone(), text.clone(), problems));
            }
        }
    }
    out
}
