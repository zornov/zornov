// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Captions: speech recognition with word timestamps (Whisper).
//!
//! The audio is cut into 30 s windows. Each window is encoded once, then
//! decoded greedily with Whisper's timestamp tokens, which split it into
//! segments; the next window starts where the last complete segment ended.
//! Word times come from the decoder's cross-attention: a few attention heads
//! (listed per model in its `generation_config.json`) follow the audio
//! closely, and dynamic time warping over their attention weights finds the
//! audio frame where each token starts.
//!
//! Whisper paper: Radford et al., "Robust Speech Recognition via Large-Scale
//! Weak Supervision" (2022). Model weights: MIT licence.

use crate::cache::{Cacheable, Reader, Writer};
use crate::engine::{self, Tensor};
use crate::error::{invalid, Error, Result};
use crate::jobs::JobCtx;
use crate::registry::ModelId;
use crate::store::ModelStore;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use studio_nn::audio::{resample, whisper_log_mel, WHISPER_CHUNK_SAMPLES, WHISPER_FRAMES, WHISPER_HOP, WHISPER_SR};
use studio_nn::json::{self, Json};
use studio_nn::Session;

/// One recognised word. Times are seconds from the start of the audio.
#[derive(Clone, Debug, PartialEq)]
pub struct Word {
    /// The word as spoken, with its leading space if it had one.
    pub text: String,
    pub start: f64,
    pub end: f64,
    /// Mean token probability (0..1); low values are worth a second look.
    pub probability: f32,
}

/// A caption-sized piece of text.
#[derive(Clone, Debug, PartialEq)]
pub struct Segment {
    pub start: f64,
    pub end: f64,
    pub text: String,
    /// Empty when word timestamps were not requested (or for translation).
    pub words: Vec<Word>,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Transcript {
    /// ISO 639-1 code ("en", "de", ...), detected or as requested.
    pub language: String,
    pub segments: Vec<Segment>,
}

impl Transcript {
    /// All words in order.
    pub fn words(&self) -> impl Iterator<Item = &Word> {
        self.segments.iter().flat_map(|s| s.words.iter())
    }

    /// Plain text.
    pub fn text(&self) -> String {
        self.segments.iter().map(|s| s.text.trim()).collect::<Vec<_>>().join(" ")
    }

    /// SubRip subtitles.
    pub fn to_srt(&self) -> String {
        fn ts(t: f64) -> String {
            let ms = (t.max(0.0) * 1000.0).round() as u64;
            format!("{:02}:{:02}:{:02},{:03}", ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000)
        }
        let mut s = String::new();
        for (i, seg) in self.segments.iter().enumerate() {
            s.push_str(&format!("{}\n{} --> {}\n{}\n\n", i + 1, ts(seg.start), ts(seg.end), seg.text.trim()));
        }
        s
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Task {
    Transcribe,
    /// Speech in any language → English text.
    Translate,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TranscribeOptions {
    /// Spoken language ("en"); `None` detects it from the first 30 s.
    pub language: Option<String>,
    pub task: Task,
    pub word_timestamps: bool,
}

impl Default for TranscribeOptions {
    fn default() -> Self {
        TranscribeOptions { language: None, task: Task::Transcribe, word_timestamps: true }
    }
}

impl TranscribeOptions {
    /// Canonical text for the cache key.
    pub fn cache_params(&self) -> String {
        format!("v1 lang={} task={:?} words={}", self.language.as_deref().unwrap_or("auto"), self.task, self.word_timestamps)
    }
}

/// Whisper models in order of preference (largest installed wins).
pub const WHISPER_MODELS: [ModelId; 3] = [ModelId::WhisperSmall, ModelId::WhisperBase, ModelId::WhisperTiny];

// ------------------------------------------------------------------ tokenizer

/// Byte-level BPE vocabulary, used only for decoding ids to text.
struct Tokenizer {
    /// Token id → raw bytes (special tokens: their text).
    bytes: Vec<Vec<u8>>,
    special: HashMap<String, u32>,
}

/// GPT-2's reversible byte ↔ printable-character map, inverted.
fn byte_decoder() -> HashMap<char, u8> {
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
    bs.into_iter().zip(cs).map(|(b, c)| (char::from_u32(c).expect("valid char"), b as u8)).collect()
}

impl Tokenizer {
    fn parse(text: &str) -> Result<Tokenizer> {
        let j = json::parse(text)?;
        let bad = || Error::Inference("tokenizer.json: unexpected layout".into());
        let Some(Json::Obj(vocab)) = j.get("model").and_then(|m| m.get("vocab")) else { return Err(bad()) };
        let dec = byte_decoder();
        let mut bytes: Vec<Vec<u8>> = Vec::new();
        let mut put = |id: usize, b: Vec<u8>| {
            if bytes.len() <= id {
                bytes.resize(id + 1, Vec::new());
            }
            bytes[id] = b;
        };
        for (tok, id) in vocab {
            let id = id.as_f64().ok_or_else(bad)? as usize;
            put(id, tok.chars().map(|c| dec.get(&c).copied().unwrap_or(b'?')).collect());
        }
        let mut special = HashMap::new();
        for t in j.get("added_tokens").and_then(|a| a.as_arr()).ok_or_else(bad)? {
            let (Some(id), Some(content)) = (t.get("id").and_then(|v| v.as_f64()), t.get("content").and_then(|v| v.as_str())) else { continue };
            put(id as usize, content.as_bytes().to_vec());
            special.insert(content.to_string(), id as u32);
        }
        Ok(Tokenizer { bytes, special })
    }

    fn token_bytes(&self, id: u32) -> &[u8] {
        self.bytes.get(id as usize).map(|v| v.as_slice()).unwrap_or(&[])
    }

    fn decode(&self, ids: &[u32]) -> String {
        let b: Vec<u8> = ids.iter().flat_map(|&i| self.token_bytes(i).iter().copied()).collect();
        String::from_utf8_lossy(&b).into_owned()
    }
}

/// The parts of `generation_config.json` decoding needs.
struct GenConfig {
    sot: u32,
    eot: u32,
    no_timestamps: u32,
    ts_begin: u32,
    no_speech: Option<u32>,
    langs: Vec<(String, u32)>,
    transcribe: u32,
    translate: u32,
    suppress: Vec<u32>,
    begin_suppress: Vec<u32>,
    max_initial_ts: u32,
    alignment_heads: Vec<(usize, usize)>,
}

impl GenConfig {
    fn parse(text: &str, tok: &Tokenizer) -> Result<GenConfig> {
        let j = json::parse(text)?;
        let num = |k: &str| j.get(k).and_then(|v| v.as_f64()).map(|v| v as u32).ok_or_else(|| Error::Inference(format!("generation_config.json: missing {k}")));
        let ids = |k: &str| j.get(k).and_then(|v| v.as_arr()).map(|a| a.iter().filter_map(|v| v.as_f64()).map(|v| v as u32).collect::<Vec<_>>()).unwrap_or_default();
        let mut langs: Vec<(String, u32)> = match j.get("lang_to_id") {
            Some(Json::Obj(m)) => m.iter().filter_map(|(k, v)| Some((k.trim_start_matches("<|").trim_end_matches("|>").to_string(), v.as_f64()? as u32))).collect(),
            _ => Vec::new(),
        };
        langs.sort_by_key(|l| l.1);
        let task = |k: &str| j.get("task_to_id").and_then(|t| t.get(k)).and_then(|v| v.as_f64()).map(|v| v as u32);
        let no_timestamps = num("no_timestamps_token_id")?;
        let heads = j
            .get("alignment_heads")
            .and_then(|v| v.as_arr())
            .map(|a| a.iter().filter_map(|p| Some((p.as_arr()?.first()?.as_f64()? as usize, p.as_arr()?.get(1)?.as_f64()? as usize))).collect())
            .unwrap_or_default();
        Ok(GenConfig {
            sot: num("decoder_start_token_id")?,
            eot: num("eos_token_id")?,
            no_timestamps,
            ts_begin: no_timestamps + 1,
            no_speech: tok.special.get("<|nospeech|>").or_else(|| tok.special.get("<|nocaptions|>")).copied(),
            langs,
            transcribe: task("transcribe").unwrap_or(50359),
            translate: task("translate").unwrap_or(50358),
            suppress: ids("suppress_tokens"),
            begin_suppress: ids("begin_suppress_tokens"),
            max_initial_ts: num("max_initial_timestamp_index").unwrap_or(50),
            alignment_heads: heads,
        })
    }
}

// ------------------------------------------------------------------- model

/// A loaded Whisper model.
pub struct Whisper {
    pub model: ModelId,
    enc: Arc<Session>,
    dec: Arc<Session>,
    tok: Tokenizer,
    cfg: GenConfig,
}

/// Seconds per timestamp token, and encoder frames per second.
const TS_STEP: f64 = 0.02;
const ENC_FPS: f64 = 50.0;
const MEL_FPS: usize = 100;
const MAX_TOKENS: usize = 224;
const N_MELS: usize = 80;

impl Whisper {
    /// Loads a specific Whisper model.
    pub fn load(store: &ModelStore, id: ModelId) -> Result<Whisper> {
        let dir = store.require(id)?;
        Self::load_dir(&dir, id)
    }

    /// Loads the largest installed Whisper, or `ModelMissing(WhisperTiny)`
    /// (the smallest download) when there is none.
    pub fn load_best(store: &ModelStore) -> Result<Whisper> {
        match WHISPER_MODELS.iter().find(|&&id| store.is_installed(id)) {
            Some(&id) => Self::load(store, id),
            None => Err(Error::ModelMissing(ModelId::WhisperTiny)),
        }
    }

    fn load_dir(dir: &Path, id: ModelId) -> Result<Whisper> {
        let tok = Tokenizer::parse(&std::fs::read_to_string(dir.join("tokenizer.json"))?)?;
        let cfg = GenConfig::parse(&std::fs::read_to_string(dir.join("generation_config.json"))?, &tok)?;
        let enc = engine::session(&dir.join("encoder_model.onnx"))?;
        let dec = engine::session(&dir.join("decoder_model.onnx"))?;
        Ok(Whisper { model: id, enc, dec, tok, cfg })
    }

    /// Supported language codes.
    pub fn languages(&self) -> Vec<&str> {
        self.cfg.langs.iter().map(|l| l.0.as_str()).collect()
    }

    fn encode(&self, window: &[f32]) -> Result<Tensor> {
        let mel = whisper_log_mel(window, N_MELS);
        let mut o = engine::run(&self.enc, vec![("input_features", Tensor::f32(&[1, N_MELS, WHISPER_FRAMES], mel))])?;
        o.take("last_hidden_state")
    }

    /// Logits for every position of `tokens`, plus the cross-attentions when
    /// `attn` is set.
    fn decode(&self, enc: &Tensor, tokens: &[u32], attn: bool) -> Result<(Vec<f32>, usize, Vec<Tensor>)> {
        let ids = Tensor::i64(&[1, tokens.len()], tokens.iter().map(|&t| t as i64).collect());
        let mut o = engine::run(&self.dec, vec![("input_ids", ids), ("encoder_hidden_states", enc.clone())])?;
        let logits = o.take("logits")?;
        let vocab = *logits.shape.last().unwrap_or(&0);
        let mut att = Vec::new();
        if attn {
            for l in 0.. {
                match o.take(&format!("cross_attentions.{l}")) {
                    Ok(t) => att.push(t),
                    Err(_) => break,
                }
            }
        }
        Ok((logits.into_f32_vec(), vocab, att))
    }

    /// Spoken language of (up to) the first 30 s and its probability.
    pub fn detect_language(&self, samples_16k: &[f32]) -> Result<(String, f32)> {
        if self.cfg.langs.is_empty() {
            return Ok(("en".into(), 1.0));
        }
        let enc = self.encode(&samples_16k[..samples_16k.len().min(WHISPER_CHUNK_SAMPLES)])?;
        let (logits, _, _) = self.decode(&enc, &[self.cfg.sot], false)?;
        let l: Vec<f32> = self.cfg.langs.iter().map(|(_, id)| logits[*id as usize]).collect();
        let p = softmax(&l);
        let (i, &best) = p.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).expect("languages");
        Ok((self.cfg.langs[i].0.clone(), best))
    }

    /// Transcribes mono audio at `rate` Hz.
    pub fn transcribe(&self, mono: &[f32], rate: u32, opts: &TranscribeOptions, ctx: &JobCtx) -> Result<Transcript> {
        if rate == 0 {
            return invalid("sample rate 0");
        }
        ctx.set_message("Preparing audio");
        let audio = resample(mono, rate, WHISPER_SR as u32);
        let language = match &opts.language {
            Some(l) => {
                if !self.cfg.langs.is_empty() && !self.cfg.langs.iter().any(|(c, _)| c == l) {
                    return invalid(format!("language {l} is not supported by this model"));
                }
                l.clone()
            }
            None => self.detect_language(&audio)?.0,
        };
        let lang_tok = self.cfg.langs.iter().find(|(c, _)| *c == language).map(|l| l.1);
        let task_tok = if opts.task == Task::Translate { self.cfg.translate } else { self.cfg.transcribe };
        let mut prompt = vec![self.cfg.sot];
        prompt.extend(lang_tok);
        prompt.push(task_tok);
        let words_wanted = opts.word_timestamps && opts.task == Task::Transcribe && !self.cfg.alignment_heads.is_empty();
        let split_by_char = ["zh", "ja", "th", "lo", "my", "yue"].contains(&language.as_str());

        let total_frames = audio.len() / WHISPER_HOP;
        let mut seek = 0usize;
        let mut segments = Vec::new();
        while seek < total_frames {
            ctx.check()?;
            ctx.set_progress(seek as f32 / total_frames as f32);
            ctx.set_message(format!("Transcribing {} / {}", clock(seek as f64 / MEL_FPS as f64), clock(total_frames as f64 / MEL_FPS as f64)));
            let seg_frames = (total_frames - seek).min(WHISPER_FRAMES);
            let start_sample = seek * WHISPER_HOP;
            let window = &audio[start_sample..(start_sample + WHISPER_CHUNK_SAMPLES).min(audio.len())];
            let offset = seek as f64 / MEL_FPS as f64;
            let enc = self.encode(window)?;
            let dr = self.decode_window(&enc, &prompt, ctx)?;
            if dr.no_speech > 0.6 && dr.avg_logprob < -1.0 {
                seek += seg_frames;
                continue;
            }
            let (mut segs, advance) = self.split_segments(&dr.tokens, offset, seg_frames);
            if words_wanted && !segs.is_empty() {
                self.add_words(&enc, &prompt, &mut segs, offset, seg_frames, split_by_char)?;
            }
            segments.extend(segs.into_iter().filter(|s| !s.text.trim().is_empty()).map(|s| Segment { start: s.start, end: s.end, text: s.text, words: s.words }));
            seek += advance.max(1);
        }
        ctx.set_progress(1.0);
        Ok(Transcript { language, segments })
    }

    fn decode_window(&self, enc: &Tensor, prompt: &[u32], ctx: &JobCtx) -> Result<DecodeResult> {
        let c = &self.cfg;
        let mut tokens = prompt.to_vec();
        let mut sampled: Vec<u32> = Vec::new();
        let mut sum_logprob = 0.0f64;
        let mut no_speech = 0.0f32;
        for step in 0..MAX_TOKENS {
            ctx.check()?;
            let (logits, vocab, _) = self.decode(enc, &tokens, false)?;
            if step == 0
                && let Some(ns) = c.no_speech
            {
                // Probability of "no speech" at the start-of-transcript position.
                no_speech = softmax(&logits[..vocab])[ns as usize];
            }
            let mut l = logits[(tokens.len() - 1) * vocab..tokens.len() * vocab].to_vec();
            self.filter_logits(&mut l, &sampled);
            let lp = log_softmax(&l);
            let (next, &best) = lp.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).expect("vocab");
            let next = next as u32;
            if next == c.eot {
                break;
            }
            sum_logprob += best as f64;
            sampled.push(next);
            tokens.push(next);
            if repeating(&sampled) {
                break;
            }
        }
        let n = sampled.len().max(1) as f64;
        Ok(DecodeResult { tokens: sampled, avg_logprob: sum_logprob / n, no_speech })
    }

    /// Whisper's decoding rules: no special tokens, timestamps in pairs and
    /// never going backwards, an early first timestamp, and a timestamp
    /// whenever timestamps are jointly more likely than any text token.
    fn filter_logits(&self, l: &mut [f32], sampled: &[u32]) {
        let c = &self.cfg;
        let ninf = f32::NEG_INFINITY;
        let tsb = c.ts_begin as usize;
        let n = l.len();
        for &s in &c.suppress {
            if let Some(v) = l.get_mut(s as usize) {
                *v = ninf;
            }
        }
        // every special token between end-of-text and the first timestamp
        l[c.eot as usize + 1..tsb.min(n)].fill(ninf);
        if sampled.is_empty() {
            for &s in &c.begin_suppress {
                if let Some(v) = l.get_mut(s as usize) {
                    *v = ninf;
                }
            }
        }
        let is_ts = |t: u32| t >= c.ts_begin;
        let last = sampled.last().is_some_and(|&t| is_ts(t));
        let penult = sampled.len() < 2 || is_ts(sampled[sampled.len() - 2]);
        if last {
            if penult {
                l[tsb..].fill(ninf);
            } else {
                l[..c.eot as usize].fill(ninf);
            }
        }
        if let Some(&last_ts) = sampled.iter().rev().find(|&&t| is_ts(t)) {
            let floor = if last && !penult { last_ts } else { last_ts + 1 } as usize;
            l[tsb..floor.min(n)].fill(ninf);
        }
        if sampled.is_empty() {
            l[..tsb].fill(ninf);
            let max_init = tsb + c.max_initial_ts as usize + 1;
            if max_init < l.len() {
                l[max_init..].fill(ninf);
            }
        }
        let lp = log_softmax(l);
        let ts_mass = logsumexp(&lp[tsb..]);
        let text_max = lp[..tsb].iter().copied().fold(ninf, f32::max);
        if ts_mass > text_max {
            l[..tsb].fill(ninf);
        }
    }

    /// Splits sampled tokens at timestamp pairs; returns the segments and
    /// how many mel frames to advance.
    fn split_segments(&self, tokens: &[u32], offset: f64, seg_frames: usize) -> (Vec<RawSegment>, usize) {
        let tsb = self.cfg.ts_begin;
        let is_ts: Vec<bool> = tokens.iter().map(|&t| t >= tsb).collect();
        let single_end = is_ts.len() >= 2 && !is_ts[is_ts.len() - 2] && is_ts[is_ts.len() - 1];
        let mut cuts: Vec<usize> = (1..tokens.len()).filter(|&i| is_ts[i - 1] && is_ts[i]).collect();
        let mut out = Vec::new();
        let time = |t: u32| offset + (t - tsb) as f64 * TS_STEP;
        if !cuts.is_empty() {
            if single_end {
                cuts.push(tokens.len());
            }
            let mut last = 0;
            for &cut in &cuts {
                let s = &tokens[last..cut];
                out.push(self.raw_segment(s, time(s[0]), time(s[s.len() - 1])));
                last = cut;
            }
            let advance = if single_end { seg_frames } else { (tokens[last - 1] - tsb) as usize * 2 };
            (out, advance)
        } else {
            let mut dur = seg_frames as f64 / MEL_FPS as f64;
            if let Some(&t) = tokens.iter().rev().find(|&&t| t >= tsb)
                && t != tsb
            {
                dur = (t - tsb) as f64 * TS_STEP;
            }
            out.push(self.raw_segment(tokens, offset, offset + dur));
            (out, seg_frames)
        }
    }

    fn raw_segment(&self, tokens: &[u32], start: f64, end: f64) -> RawSegment {
        let text_tokens: Vec<u32> = tokens.iter().copied().filter(|&t| t < self.cfg.eot).collect();
        RawSegment { start, end, text: self.tok.decode(&text_tokens), text_tokens, words: Vec::new() }
    }

    /// Word timestamps for one window (see the module docs).
    fn add_words(&self, enc: &Tensor, prompt: &[u32], segs: &mut [RawSegment], offset: f64, seg_frames: usize, by_char: bool) -> Result<()> {
        let c = &self.cfg;
        let text: Vec<u32> = segs.iter().flat_map(|s| s.text_tokens.iter().copied()).collect();
        if text.is_empty() {
            return Ok(());
        }
        let mut tokens = prompt.to_vec();
        tokens.push(c.no_timestamps);
        tokens.extend(&text);
        tokens.push(c.eot);
        let (logits, vocab, att) = self.decode(enc, &tokens, true)?;
        // probability of each text token given what precedes it
        let first = prompt.len();
        let probs: Vec<f32> = text.iter().enumerate().map(|(i, &t)| softmax(&logits[(first + i) * vocab..(first + i + 1) * vocab])[t as usize]).collect();

        let n_tok = tokens.len();
        let frames = (seg_frames / 2).max(1);
        let rows = first..n_tok - 1; // from the no-timestamps token to the last text token
        let n = rows.len();
        let mut matrix = vec![0.0f32; n * frames];
        let mut used = 0;
        for &(layer, head) in &c.alignment_heads {
            let Some(a) = att.get(layer) else { continue };
            let (heads, len, enc_len) = (a.shape[1], a.shape[2], a.shape[3]);
            if head >= heads || len != n_tok || enc_len < frames {
                continue;
            }
            let v = a.f32s();
            let base = head * len * enc_len;
            let mut w: Vec<f32> = rows.clone().flat_map(|r| v[base + r * enc_len..base + r * enc_len + frames].to_vec()).collect();
            // normalise each frame column over tokens, then smooth over time
            for f in 0..frames {
                let col: Vec<f32> = (0..n).map(|r| w[r * frames + f]).collect();
                let mean = col.iter().sum::<f32>() / n as f32;
                let var = col.iter().map(|x| (x - mean) * (x - mean)).sum::<f32>() / (n as f32 - 1.0).max(1.0);
                let sd = var.sqrt().max(1e-8);
                for r in 0..n {
                    w[r * frames + f] = (w[r * frames + f] - mean) / sd;
                }
            }
            for r in 0..n {
                let row = median_filter(&w[r * frames..(r + 1) * frames], 7);
                for (m, x) in matrix[r * frames..(r + 1) * frames].iter_mut().zip(row) {
                    *m += x;
                }
            }
            used += 1;
        }
        if used == 0 {
            return Ok(());
        }
        let neg: Vec<f32> = matrix.iter().map(|x| -x / used as f32).collect();
        let (text_idx, time_idx) = dtw(&neg, n, frames);
        let mut jump_times = Vec::with_capacity(n);
        for i in 0..text_idx.len() {
            if i == 0 || text_idx[i] != text_idx[i - 1] {
                jump_times.push(time_idx[i] as f64 / ENC_FPS);
            }
        }

        // split each segment's tokens into words and assign the times
        let mut tok_pos = 0usize;
        for seg in segs.iter_mut() {
            let words = split_words(&self.tok, &seg.text_tokens, by_char);
            for (wtext, wtoks) in words {
                let (a, b) = (tok_pos, tok_pos + wtoks.len());
                tok_pos = b;
                let (Some(&ts), Some(&te)) = (jump_times.get(a), jump_times.get(b)) else { continue };
                let p = probs[a..b].iter().sum::<f32>() / (b - a).max(1) as f32;
                seg.words.push(Word { text: wtext, start: offset + ts, end: offset + te, probability: p });
            }
            merge_punctuation(&mut seg.words);
            // keep words inside their segment and in order
            let mut prev = seg.start;
            for w in &mut seg.words {
                w.start = w.start.clamp(prev, seg.end.max(prev));
                w.end = w.end.clamp(w.start, seg.end.max(w.start));
                prev = w.start;
            }
        }
        Ok(())
    }
}

struct DecodeResult {
    tokens: Vec<u32>,
    avg_logprob: f64,
    no_speech: f32,
}

struct RawSegment {
    start: f64,
    end: f64,
    text: String,
    text_tokens: Vec<u32>,
    words: Vec<Word>,
}

fn clock(t: f64) -> String {
    let s = t as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

/// True when the tail of `t` is one short phrase repeated (a decoding loop).
fn repeating(t: &[u32]) -> bool {
    for n in 1..=8 {
        let reps = if n <= 2 { 8 } else { 4 };
        if t.len() < n * reps {
            continue;
        }
        let tail = &t[t.len() - n..];
        if (1..reps).all(|k| &t[t.len() - (k + 1) * n..t.len() - k * n] == tail) {
            return true;
        }
    }
    false
}

pub(crate) fn softmax(x: &[f32]) -> Vec<f32> {
    let m = x.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let e: Vec<f32> = x.iter().map(|v| (v - m).exp()).collect();
    let s: f32 = e.iter().sum();
    e.into_iter().map(|v| v / s).collect()
}

fn logsumexp(x: &[f32]) -> f32 {
    let m = x.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    if m == f32::NEG_INFINITY {
        return m;
    }
    m + x.iter().map(|v| (v - m).exp()).sum::<f32>().ln()
}

fn log_softmax(x: &[f32]) -> Vec<f32> {
    let lse = logsumexp(x);
    x.iter().map(|v| v - lse).collect()
}

/// Median filter with odd `width`, edges reflected.
fn median_filter(x: &[f32], width: usize) -> Vec<f32> {
    let n = x.len();
    let h = width / 2;
    if n <= h {
        return x.to_vec();
    }
    let at = |i: isize| -> f32 {
        let i = if i < 0 { -i } else if i >= n as isize { 2 * (n as isize - 1) - i } else { i };
        x[i.clamp(0, n as isize - 1) as usize]
    };
    let mut buf = vec![0.0f32; width];
    (0..n as isize)
        .map(|i| {
            for (k, b) in buf.iter_mut().enumerate() {
                *b = at(i + k as isize - h as isize);
            }
            buf.sort_by(f32::total_cmp);
            buf[h]
        })
        .collect()
}

/// Dynamic time warping over an `n × m` cost matrix (row-major); returns the
/// row and column index of every step of the cheapest monotonic path from
/// (0, 0) to (n−1, m−1).
pub(crate) fn dtw(cost: &[f32], n: usize, m: usize) -> (Vec<usize>, Vec<usize>) {
    let w = m + 1;
    let mut acc = vec![f64::INFINITY; (n + 1) * w];
    let mut trace = vec![0u8; (n + 1) * w];
    acc[0] = 0.0;
    for j in 1..=m {
        for i in 1..=n {
            let c0 = acc[(i - 1) * w + j - 1];
            let c1 = acc[(i - 1) * w + j];
            let c2 = acc[i * w + j - 1];
            let (c, t) = if c0 <= c1 && c0 <= c2 { (c0, 0) } else if c1 <= c2 { (c1, 1) } else { (c2, 2) };
            acc[i * w + j] = cost[(i - 1) * m + j - 1] as f64 + c;
            trace[i * w + j] = t;
        }
    }
    let (mut i, mut j) = (n, m);
    let (mut ti, mut tj) = (Vec::new(), Vec::new());
    while i > 0 || j > 0 {
        ti.push(i.saturating_sub(1));
        tj.push(j.saturating_sub(1));
        let t = if i == 0 { 2 } else if j == 0 { 1 } else { trace[i * w + j] };
        match t {
            0 => {
                i -= 1;
                j -= 1;
            }
            1 => i -= 1,
            _ => j -= 1,
        }
    }
    ti.reverse();
    tj.reverse();
    (ti, tj)
}

const PUNCT: &str = "!\"#$%&'()*+,-./:;<=>?@[\\]^_`{|}~";

/// Splits tokens into words: tokens are first grouped into whole UTF-8
/// characters; then (unless `by_char`, for languages written without
/// spaces) a new word starts at a leading space or a punctuation mark.
fn split_words(tok: &Tokenizer, tokens: &[u32], by_char: bool) -> Vec<(String, Vec<u32>)> {
    let mut pieces: Vec<(String, Vec<u32>)> = Vec::new();
    let (mut bytes, mut ids) = (Vec::new(), Vec::new());
    for &t in tokens {
        bytes.extend_from_slice(tok.token_bytes(t));
        ids.push(t);
        if let Ok(s) = std::str::from_utf8(&bytes) {
            pieces.push((s.to_string(), std::mem::take(&mut ids)));
            bytes.clear();
        }
    }
    if !ids.is_empty() {
        pieces.push((String::from_utf8_lossy(&bytes).into_owned(), ids));
    }
    if by_char {
        return pieces;
    }
    let mut words: Vec<(String, Vec<u32>)> = Vec::new();
    for (s, ids) in pieces {
        let trimmed = s.trim();
        let punct = !trimmed.is_empty() && trimmed.chars().all(|c| PUNCT.contains(c));
        match words.last_mut() {
            Some(last) if !s.starts_with(' ') && !punct => {
                last.0.push_str(&s);
                last.1.extend(ids);
            }
            _ => words.push((s, ids)),
        }
    }
    words
}

/// Attaches opening punctuation to the following word and closing
/// punctuation to the preceding one, keeping their times.
fn merge_punctuation(words: &mut Vec<Word>) {
    const PREPEND: &str = "\"'“¿([{-";
    const APPEND: &str = "\"'.。,，!！?？:：”)]}、";
    let mut i = words.len().saturating_sub(1);
    while i > 0 {
        let t = words[i - 1].text.trim().to_string();
        if words[i - 1].text.starts_with(' ') && !t.is_empty() && t.chars().all(|c| PREPEND.contains(c)) {
            let w = words.remove(i - 1);
            words[i - 1].text = format!("{}{}", w.text, words[i - 1].text.trim_start());
            words[i - 1].start = w.start;
        }
        i -= 1;
    }
    let mut i = 1;
    while i < words.len() {
        let t = words[i].text.trim().to_string();
        if !words[i].text.starts_with(' ') && !t.is_empty() && t.chars().all(|c| APPEND.contains(c)) {
            let w = words.remove(i);
            words[i - 1].text.push_str(&w.text);
            words[i - 1].end = w.end;
        } else {
            i += 1;
        }
    }
}

// ------------------------------------------------------------------- cache

impl Cacheable for Word {
    const TAG: &'static str = "word";
    fn encode(&self, w: &mut Writer) {
        w.str(&self.text);
        w.f64(self.start);
        w.f64(self.end);
        w.f32(self.probability);
    }
    fn decode(r: &mut Reader) -> Option<Self> {
        Some(Word { text: r.str()?, start: r.f64()?, end: r.f64()?, probability: r.f32()? })
    }
}

impl Cacheable for Segment {
    const TAG: &'static str = "segment";
    fn encode(&self, w: &mut Writer) {
        w.f64(self.start);
        w.f64(self.end);
        w.str(&self.text);
        self.words.encode(w);
    }
    fn decode(r: &mut Reader) -> Option<Self> {
        Some(Segment { start: r.f64()?, end: r.f64()?, text: r.str()?, words: Vec::decode(r)? })
    }
}

impl Cacheable for Transcript {
    const TAG: &'static str = "transcript-v1";
    fn encode(&self, w: &mut Writer) {
        w.str(&self.language);
        self.segments.encode(w);
    }
    fn decode(r: &mut Reader) -> Option<Self> {
        Some(Transcript { language: r.str()?, segments: Vec::decode(r)? })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toy_tokenizer() -> Tokenizer {
        // "Ġ" is the byte-level form of a space.
        let text = r#"{"added_tokens":[{"id":10,"content":"<|endoftext|>"}],
            "model":{"vocab":{"Ġhello":0,"Ġwor":1,"ld":2,",":3,"Ġ\"":4,"hi":5,"Ã©":6}}}"#;
        Tokenizer::parse(text).unwrap()
    }

    #[test]
    fn byte_level_decoding() {
        let t = toy_tokenizer();
        assert_eq!(t.decode(&[0, 1, 2, 3]), " hello world,");
        assert_eq!(t.decode(&[6]), "é");
        assert_eq!(t.special["<|endoftext|>"], 10);
        assert_eq!(byte_decoder().len(), 256);
    }

    #[test]
    fn words_split_on_spaces_and_punctuation() {
        let t = toy_tokenizer();
        let w = split_words(&t, &[0, 1, 2, 3, 4, 5], false);
        let texts: Vec<&str> = w.iter().map(|x| x.0.as_str()).collect();
        assert_eq!(texts, [" hello", " world", ",", " \"hi"]);
        assert_eq!(w[1].1, [1, 2]);
        let w = split_words(&t, &[5, 6], true);
        assert_eq!(w.len(), 2);
    }

    #[test]
    fn punctuation_merges_into_words() {
        let mk = |t: &str, s: f64| Word { text: t.into(), start: s, end: s + 1.0, probability: 1.0 };
        let mut w = vec![mk(" hello", 0.0), mk(",", 1.0), mk(" \"", 2.0), mk("hi", 3.0)];
        // "hi" has no leading space so it would continue the quote word in
        // split_words; here the quote is a separate word that prepends.
        w[3].text = " hi".into();
        merge_punctuation(&mut w);
        let texts: Vec<&str> = w.iter().map(|x| x.text.as_str()).collect();
        assert_eq!(texts, [" hello,", " \"hi"]);
        assert_eq!((w[0].start, w[0].end), (0.0, 2.0));
        assert_eq!(w[1].start, 2.0);
    }

    #[test]
    fn dtw_follows_the_cheap_diagonal() {
        // 3 tokens over 6 frames; token k is cheap on frames 2k..2k+1.
        let (n, m) = (3, 6);
        let cost: Vec<f32> = (0..n * m).map(|i| if (i % m) / 2 == i / m { 0.0 } else { 1.0 }).collect();
        let (ti, tj) = dtw(&cost, n, m);
        assert_eq!(ti.first(), Some(&0));
        assert_eq!((ti.last(), tj.last()), (Some(&2), Some(&5)));
        for (&i, &j) in ti.iter().zip(&tj) {
            assert_eq!(j / 2, i, "path left the cheap band at ({i},{j})");
        }
    }

    #[test]
    fn median_and_softmax() {
        assert_eq!(median_filter(&[1.0, 9.0, 1.0, 1.0, 5.0, 5.0, 5.0], 3), vec![9.0, 1.0, 1.0, 1.0, 5.0, 5.0, 5.0]);
        let p = softmax(&[0.0, 0.0]);
        assert!((p[0] - 0.5).abs() < 1e-6);
        assert!((logsumexp(&[0.0, 0.0]) - 2f32.ln()).abs() < 1e-6);
    }

    #[test]
    fn repetition_guard() {
        assert!(repeating(&[5; 8]));
        assert!(!repeating(&[5; 7]));
        assert!(repeating(&[1, 2, 3, 1, 2, 3, 1, 2, 3, 1, 2, 3]));
        assert!(!repeating(&[1, 2, 3, 4, 5, 6, 7, 8, 9]));
    }

    #[test]
    fn srt_and_cache_round_trip() {
        let t = Transcript {
            language: "en".into(),
            segments: vec![Segment { start: 1.5, end: 62.25, text: " Hello there.".into(), words: vec![Word { text: " Hello".into(), start: 1.5, end: 2.0, probability: 0.9 }] }],
        };
        assert_eq!(t.to_srt(), "1\n00:00:01,500 --> 00:01:02,250\nHello there.\n\n");
        let mut w = Writer::default();
        t.encode(&mut w);
        assert_eq!(Transcript::decode(&mut Reader::new(&w.0)).unwrap(), t);
    }

    #[test]
    fn missing_model_is_reported() {
        let store = ModelStore::new(std::env::temp_dir().join("studio-ai-no-models"));
        assert_eq!(Whisper::load_best(&store).err(), Some(Error::ModelMissing(ModelId::WhisperTiny)));
    }
}
