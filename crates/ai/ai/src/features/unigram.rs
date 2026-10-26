// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The text tokenizer of the picture-text embedding model: a SentencePiece
//! unigram vocabulary of 32 000 pieces read from the model's
//! `tokenizer.json`. Text is lower-cased with punctuation removed (how the
//! model was trained), spaces become `▁`, and the most likely split into
//! pieces is found with the Viterbi algorithm (Kudo, "Subword
//! Regularization", 2018). The result ends with the end marker and is
//! padded with it to a fixed length, which is how the model reads text.

use crate::error::{Error, Result};
use std::collections::HashMap;
use studio_nn::json::{self, Json};

/// The id of the end marker, also the padding.
pub const END: i64 = 1;

pub struct UnigramTokenizer {
    pieces: HashMap<String, (i64, f32)>,
    unk: i64,
    unk_score: f32,
    max_chars: usize,
}

impl UnigramTokenizer {
    pub fn parse(text: &str) -> Result<UnigramTokenizer> {
        let bad = || Error::Inference("tokenizer.json: unexpected layout".into());
        let j = json::parse(text)?;
        let model = j.get("model").ok_or_else(bad)?;
        let vocab = model.get("vocab").and_then(|v| v.as_arr()).ok_or_else(bad)?;
        let unk = model.get("unk_id").and_then(|v| v.as_f64()).unwrap_or(2.0) as i64;
        let mut pieces = HashMap::with_capacity(vocab.len());
        let (mut min, mut max_chars) = (0.0f32, 1usize);
        for (id, entry) in vocab.iter().enumerate() {
            let Json::Arr(pair) = entry else { return Err(bad()) };
            let (Some(p), Some(s)) = (pair.first().and_then(|p| p.as_str()), pair.get(1).and_then(|s| s.as_f64())) else { return Err(bad()) };
            // Control pieces (`<pad>`, `</s>`, `<unk>`, byte pieces) are never
            // produced by the split.
            if p.starts_with('<') && p.ends_with('>') && p.chars().count() > 2 {
                continue;
            }
            min = min.min(s as f32);
            max_chars = max_chars.max(p.chars().count());
            pieces.insert(p.to_string(), (id as i64, s as f32));
        }
        if pieces.len() < 1000 {
            return Err(bad());
        }
        Ok(UnigramTokenizer { pieces, unk, unk_score: min - 10.0, max_chars })
    }

    /// Lower case, no punctuation, single spaces.
    pub fn canonical(text: &str) -> String {
        let cleaned: String = text.chars().filter(|c| !c.is_ascii_punctuation()).collect::<String>().to_lowercase();
        cleaned.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    /// The most likely pieces of `text` (no end marker).
    pub fn tokens(&self, text: &str) -> Vec<i64> {
        let norm = Self::canonical(text);
        if norm.is_empty() {
            return Vec::new();
        }
        let chars: Vec<char> = std::iter::once('\u{2581}').chain(norm.chars().map(|c| if c == ' ' { '\u{2581}' } else { c })).collect();
        let n = chars.len();
        // best[i] = (score of the best split of chars[..i], start of its last piece, id)
        let mut best: Vec<(f32, usize, i64)> = vec![(f32::NEG_INFINITY, 0, 0); n + 1];
        best[0].0 = 0.0;
        for i in 1..=n {
            for len in 1..=self.max_chars.min(i) {
                let start = i - len;
                if best[start].0 == f32::NEG_INFINITY {
                    continue;
                }
                let s: String = chars[start..i].iter().collect();
                let cand = match self.pieces.get(&s) {
                    Some(&(id, sc)) => Some((best[start].0 + sc, id)),
                    // A single character the vocabulary does not know.
                    None if len == 1 => Some((best[start].0 + self.unk_score, self.unk)),
                    None => None,
                };
                if let Some((sc, id)) = cand
                    && sc > best[i].0
                {
                    best[i] = (sc, start, id);
                }
            }
        }
        let mut out = Vec::new();
        let mut i = n;
        while i > 0 {
            let (_, start, id) = best[i];
            out.push(id);
            i = start;
        }
        out.reverse();
        // Runs of unknown characters are one unknown piece.
        out.dedup_by(|a, b| *a == self.unk && *b == self.unk);
        out
    }

    /// `len` ids: the pieces, the end marker, then the end marker as padding.
    pub fn encode(&self, text: &str, len: usize) -> Vec<i64> {
        let mut ids = self.tokens(text);
        ids.truncate(len.saturating_sub(1));
        ids.push(END);
        ids.resize(len, END);
        ids
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A toy vocabulary: a few words and letters.
    fn toy() -> UnigramTokenizer {
        let mut pieces = HashMap::new();
        let mut put = |p: &str, id: i64, s: f32| {
            pieces.insert(p.to_string(), (id, s));
        };
        put("\u{2581}", 10, -4.0);
        put("\u{2581}a", 11, -2.0);
        put("\u{2581}cat", 12, -3.0);
        put("\u{2581}c", 13, -3.5);
        put("at", 14, -3.0);
        put("a", 15, -4.5);
        put("t", 16, -4.5);
        put("\u{2581}dog", 17, -3.0);
        UnigramTokenizer { pieces, unk: 2, unk_score: -15.0, max_chars: 4 }
    }

    #[test]
    fn text_is_canonicalised_like_the_model_was_trained() {
        assert_eq!(UnigramTokenizer::canonical("  A Cat,   sleeping!! (in) the sun."), "a cat sleeping in the sun");
        assert_eq!(UnigramTokenizer::canonical("don't"), "dont");
        assert_eq!(UnigramTokenizer::canonical("..."), "");
    }

    #[test]
    fn the_likeliest_split_wins() {
        let t = toy();
        // "▁cat" (-3.0) beats "▁c" + "at" (-6.5).
        assert_eq!(t.tokens("a cat"), vec![11, 12]);
        assert_eq!(t.tokens("A DOG"), vec![11, 17]);
    }

    #[test]
    fn unknown_characters_are_one_unknown_piece() {
        let t = toy();
        assert_eq!(t.tokens("a zzz"), vec![11, 10, 2]);
    }

    #[test]
    fn encoding_ends_and_pads_with_the_end_marker() {
        let t = toy();
        let ids = t.encode("a cat", 6);
        assert_eq!(ids, vec![11, 12, 1, 1, 1, 1]);
        // Too long: cut, end marker kept.
        let ids = t.encode("a cat a cat a cat", 4);
        assert_eq!(ids.len(), 4);
        assert_eq!(ids[3], 1);
        assert_eq!(t.encode("", 3), vec![1, 1, 1]);
    }
}
