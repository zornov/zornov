// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The text tokenizer the open-vocabulary detector and the image-text
//! embedding model share: a lower-casing, byte-level BPE with a `</w>`
//! end-of-word marker (the CLIP vocabulary of 49 408 tokens), read from the
//! model's `tokenizer.json`.
//!
//! Radford et al., "Learning Transferable Visual Models From Natural
//! Language Supervision" (2021). Vocabulary and merges: MIT / Apache-2.0
//! (they ship with the models that use them).

use crate::error::{Error, Result};
use std::collections::HashMap;
use studio_nn::json::{self, Json};

pub const START: i64 = 49406;
pub const END: i64 = 49407;

/// A loaded vocabulary.
pub struct ClipTokenizer {
    vocab: HashMap<String, i64>,
    ranks: HashMap<(String, String), usize>,
    to_char: [char; 256],
}

/// GPT-2's reversible byte to printable-character map.
fn byte_chars() -> [char; 256] {
    let mut bs: Vec<u32> = (b'!' as u32..=b'~' as u32).chain(0xA1..=0xAC).chain(0xAE..=0xFF).collect();
    let mut cs = bs.clone();
    let mut n = 0;
    for b in 0..256u32 {
        if !bs.contains(&b) {
            bs.push(b);
            cs.push(256 + n);
            n += 1;
        }
    }
    let mut out = ['\0'; 256];
    for (b, c) in bs.into_iter().zip(cs) {
        out[b as usize] = char::from_u32(c).expect("valid char");
    }
    out
}

impl ClipTokenizer {
    /// Parses a Hugging Face `tokenizer.json` with a BPE model.
    pub fn parse(text: &str) -> Result<ClipTokenizer> {
        let bad = || Error::Inference("tokenizer.json: unexpected layout".into());
        let j = json::parse(text)?;
        let model = j.get("model").ok_or_else(bad)?;
        let Some(Json::Obj(v)) = model.get("vocab") else { return Err(bad()) };
        let vocab: HashMap<String, i64> = v.iter().filter_map(|(k, id)| Some((k.clone(), id.as_f64()? as i64))).collect();
        let merges = model.get("merges").and_then(|m| m.as_arr()).ok_or_else(bad)?;
        let mut ranks = HashMap::with_capacity(merges.len());
        for (i, m) in merges.iter().enumerate() {
            let pair = match m {
                Json::Str(s) => s.split_once(' ').map(|(a, b)| (a.to_string(), b.to_string())),
                Json::Arr(a) if a.len() == 2 => Some((a[0].as_str().ok_or_else(bad)?.to_string(), a[1].as_str().ok_or_else(bad)?.to_string())),
                _ => None,
            };
            if let Some(p) = pair {
                ranks.insert(p, i);
            }
        }
        if vocab.get("<|startoftext|>") != Some(&START) || vocab.get("<|endoftext|>") != Some(&END) {
            return Err(bad());
        }
        Ok(ClipTokenizer { vocab, ranks, to_char: byte_chars() })
    }

    /// Splits like CLIP's pattern: words of letters, single digits, runs of
    /// other symbols, and the English contractions.
    fn pieces(text: &str) -> Vec<String> {
        let cs: Vec<char> = text.chars().collect();
        let mut out = Vec::new();
        let mut i = 0;
        while i < cs.len() {
            let c = cs[i];
            if c.is_whitespace() {
                i += 1;
                continue;
            }
            if c == '\'' {
                let rest: String = cs[i..(i + 3).min(cs.len())].iter().collect();
                if let Some(m) = ["'re", "'ve", "'ll", "'s", "'t", "'m", "'d"].iter().find(|m| rest.starts_with(**m)) {
                    out.push((*m).to_string());
                    i += m.chars().count();
                    continue;
                }
            }
            let start = i;
            if c.is_alphabetic() {
                while i < cs.len() && cs[i].is_alphabetic() {
                    i += 1;
                }
            } else if c.is_numeric() {
                i += 1;
            } else {
                while i < cs.len() && !cs[i].is_whitespace() && !cs[i].is_alphanumeric() {
                    i += 1;
                }
            }
            out.push(cs[start..i].iter().collect());
        }
        out
    }

    fn bpe(&self, word: &str) -> Vec<String> {
        let mut parts: Vec<String> = word.bytes().map(|b| self.to_char[b as usize].to_string()).collect();
        if let Some(l) = parts.last_mut() {
            l.push_str("</w>");
        }
        while parts.len() > 1 {
            let best = (0..parts.len() - 1).filter_map(|i| self.ranks.get(&(parts[i].clone(), parts[i + 1].clone())).map(|&r| (r, i))).min();
            let Some((_, i)) = best else { break };
            let merged = format!("{}{}", parts[i], parts[i + 1]);
            parts[i] = merged;
            parts.remove(i + 1);
        }
        parts
    }

    /// Token ids of `text` without the start and end markers.
    pub fn tokens(&self, text: &str) -> Vec<i64> {
        let text = text.to_lowercase();
        let mut ids = Vec::new();
        for piece in Self::pieces(&text) {
            for p in self.bpe(&piece) {
                if let Some(&id) = self.vocab.get(&p) {
                    ids.push(id);
                }
            }
        }
        ids
    }

    /// `[start, tokens.., end]`, cut to `len` (the end marker always kept)
    /// and padded with zeros; the second vector is the attention mask.
    pub fn encode(&self, text: &str, len: usize) -> (Vec<i64>, Vec<i64>) {
        let mut ids = vec![START];
        ids.extend(self.tokens(text));
        ids.truncate(len.saturating_sub(1).max(1));
        ids.push(END);
        let mut mask = vec![1i64; ids.len()];
        ids.resize(len, 0);
        mask.resize(len, 0);
        (ids, mask)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A toy vocabulary: single letters, a few merges, the markers.
    fn toy() -> ClipTokenizer {
        let chars = byte_chars();
        let mut vocab = HashMap::new();
        let mut next = 0;
        for b in b'a'..=b'z' {
            vocab.insert(chars[b as usize].to_string(), next);
            vocab.insert(format!("{}</w>", chars[b as usize]), next + 1000);
            next += 1;
        }
        for (i, w) in ["ca", "cat</w>", "at</w>", "t</w>"].iter().enumerate() {
            vocab.insert((*w).to_string(), 100 + i as i64);
        }
        vocab.insert("<|startoftext|>".into(), START);
        vocab.insert("<|endoftext|>".into(), END);
        let mut ranks = HashMap::new();
        ranks.insert(("c".to_string(), "a".to_string()), 0);
        ranks.insert(("a".to_string(), "t</w>".to_string()), 1);
        ranks.insert(("ca".to_string(), "t</w>".to_string()), 2);
        ClipTokenizer { vocab, ranks, to_char: chars }
    }

    #[test]
    fn merges_apply_in_rank_order() {
        let t = toy();
        assert_eq!(t.bpe("cat"), vec!["cat</w>"]);
        assert_eq!(t.tokens("A CAT"), vec![1000, 101]);
    }

    #[test]
    fn pieces_split_like_the_pattern() {
        assert_eq!(ClipTokenizer::pieces("a photo of dad's 2 dogs!!"), vec!["a", "photo", "of", "dad", "'s", "2", "dogs", "!!"]);
    }

    #[test]
    fn encode_pads_and_truncates() {
        let t = toy();
        let (ids, mask) = t.encode("cat", 6);
        assert_eq!(ids, vec![START, 101, END, 0, 0, 0]);
        assert_eq!(mask, vec![1, 1, 1, 0, 0, 0]);
        let (ids, _) = t.encode("cat cat cat cat cat", 4);
        assert_eq!(ids.len(), 4);
        assert_eq!(ids[3], END);
    }
}
