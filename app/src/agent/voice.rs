// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Narration: a line spoken by the system's voices, placed on the voice
//! lane, with its words timed by Studio's own speech recognition. The
//! words are what everything else follows: a scene lasts as long as its
//! line, a picture arrives when its word is said, captions show the words
//! being spoken, and the music steps back while someone talks.

use super::{Session, f, micros, secs};
use crate::view::{self, ClipKind, SEC};
use serde_json::{Value, json};

/// One spoken word, in timeline seconds.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Said {
    pub text: String,
    pub start: f64,
    pub end: f64,
}

/// The words of a line, measured on its own audio (seconds from its start).
pub(super) fn words_of(samples: &[f32], rate: u32) -> Vec<Said> {
    let store = studio_ai::store::ModelStore::default();
    let Ok(w) = studio_ai::features::transcribe::Whisper::load_best(&store) else { return Vec::new() };
    let opts = studio_ai::features::transcribe::TranscribeOptions { language: Some("en".into()), ..Default::default() };
    match w.transcribe(samples, rate, &opts, &studio_ai::jobs::JobCtx::detached()) {
        Ok(t) => t.words().map(|w| Said { text: w.text.trim().to_owned(), start: w.start, end: w.end }).filter(|w| !w.text.is_empty()).collect(),
        Err(_) => Vec::new(),
    }
}

/// Spreads a line's words evenly over its length (when recognition is not
/// available): good enough to pace captions and arrivals.
fn even_words(text: &str, len: f64) -> Vec<Said> {
    let ws: Vec<&str> = text.split_whitespace().collect();
    let total: usize = ws.iter().map(|w| w.chars().count() + 1).sum::<usize>().max(1);
    let mut t = 0.0;
    ws.iter()
        .map(|w| {
            let d = len * (w.chars().count() + 1) as f64 / total as f64;
            let s = Said { text: (*w).to_owned(), start: t, end: t + d * 0.9 };
            t += d;
            s
        })
        .collect()
}

/// The default voice: the most natural one installed for the language.
pub fn default_voice() -> Option<String> {
    let all = studio_hw::speech::voices();
    let en = |v: &&studio_hw::speech::Voice| v.locale.starts_with("en");
    // Premium and enhanced voices first (the list is sorted that way);
    // among the compact ones Samantha reads most naturally.
    all.iter().filter(en).find(|v| v.quality != "compact").or_else(|| all.iter().filter(en).find(|v| v.name.starts_with("Samantha"))).or_else(|| all.iter().find(en)).map(|v| v.id.clone())
}

// ------------------------------------------------- speaking and timing a line

/// A line asked for: what is said, in which voice, how fast.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Want {
    pub text: String,
    pub voice: Option<String>,
    pub wpm: Option<f32>,
}

/// A line spoken, trimmed, at speech level, and its words timed (seconds
/// from the start of the line).
pub(super) struct Spoken {
    pub samples: Vec<f32>,
    pub rate: u32,
    pub by: String,
    pub words: Vec<Said>,
    /// Whether the word times were heard (speech recognition) rather than spread evenly.
    pub heard: bool,
}

/// A line as the voice made it, before its words are timed.
struct Raw {
    samples: Vec<f32>,
    rate: u32,
    by: String,
}

/// What speaks a line and what times its words (the real ones, or fakes in
/// tests).
#[derive(Clone, Copy)]
pub(super) struct Maker {
    speak: fn(&super::voice_service::VoiceSettings, &str, Option<&str>, Option<f32>) -> Result<super::voice_service::Line, String>,
    words: fn(&[f32], u32) -> Vec<Said>,
    /// Gets the recogniser ready; run beside the first line being spoken, so
    /// its load (most of a second, once) is not waited for after it.
    warm: fn(),
}

fn warm_recogniser() {
    let _ = studio_ai::features::transcribe::Whisper::load_best(&studio_ai::store::ModelStore::default());
}

impl Maker {
    pub fn real() -> Maker {
        Maker { speak: super::voice_service::speak_with, words: words_of, warm: warm_recogniser }
    }
}

/// Speaks the line and takes off the synthesizer's silence and levels it.
fn speak_raw(maker: Maker, service: &super::voice_service::VoiceSettings, want: &Want) -> Result<Raw, String> {
    let speech = (maker.speak)(service, &want.text, want.voice.as_deref(), want.wpm)?;
    if speech.samples.is_empty() {
        return Err("the voice produced no sound".into());
    }
    // Trim the synthesizer's leading and trailing silence.
    let thr = 0.004;
    let first = speech.samples.iter().position(|x| x.abs() > thr).unwrap_or(0);
    let last = speech.samples.iter().rposition(|x| x.abs() > thr).unwrap_or(speech.samples.len() - 1);
    let pad = (speech.rate as usize) / 50;
    let mut samples: Vec<f32> = speech.samples[first.saturating_sub(pad)..(last + pad).min(speech.samples.len())].to_vec();
    // Every voice at speech level, whichever service made it.
    super::voice_service::level_speech(&mut samples, speech.rate);
    Ok(Raw { samples, rate: speech.rate, by: speech.by })
}

/// Times the words of a spoken line.
fn time_words(maker: Maker, raw: Raw, text: &str) -> Spoken {
    let len = raw.samples.len() as f64 / raw.rate as f64;
    let mut words = (maker.words)(&raw.samples, raw.rate);
    let heard = !words.is_empty();
    if words.is_empty() {
        words = even_words(text, len);
    }
    // The script's own spelling and capitals, on the heard timings.
    align_to_script(&mut words, text);
    Spoken { samples: raw.samples, rate: raw.rate, by: raw.by, words, heard }
}

/// The line made, from nothing.
fn make_spoken(maker: Maker, service: &super::voice_service::VoiceSettings, want: &Want) -> Result<Spoken, String> {
    let raw = std::thread::scope(|sc| {
        sc.spawn(maker.warm);
        speak_raw(maker, service, want)
    })?;
    Ok(time_words(maker, raw, &want.text))
}

/// Lines made lately, by what was asked: the same line in the same voice is
/// made once. A line is made again when the film is rebuilt or a step is
/// tried a second way, and a hosted voice charges for every one.
#[derive(Default)]
struct Made {
    order: std::collections::VecDeque<String>,
    by_key: std::collections::HashMap<String, std::sync::Arc<Spoken>>,
    samples: usize,
}

/// Most samples kept (about 80 MB).
const KEEP_SAMPLES: usize = 20_000_000;

fn made() -> &'static std::sync::Mutex<Made> {
    static M: std::sync::OnceLock<std::sync::Mutex<Made>> = std::sync::OnceLock::new();
    M.get_or_init(Default::default)
}

fn key_of(service: &super::voice_service::VoiceSettings, want: &Want) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    format!("{service:?}").hash(&mut h);
    want.hash_parts(&mut h);
    format!("{:016x}", h.finish())
}

impl Want {
    fn hash_parts(&self, h: &mut impl std::hash::Hasher) {
        use std::hash::Hash;
        self.text.hash(h);
        self.voice.hash(h);
        self.wpm.map(f32::to_bits).hash(h);
    }
}

fn recall(key: &str) -> Option<std::sync::Arc<Spoken>> {
    made().lock().unwrap_or_else(|e| e.into_inner()).by_key.get(key).cloned()
}

fn remember(key: String, line: std::sync::Arc<Spoken>) {
    let mut m = made().lock().unwrap_or_else(|e| e.into_inner());
    if m.by_key.insert(key.clone(), line.clone()).is_none() {
        m.order.push_back(key);
        m.samples += line.samples.len();
    }
    while m.samples > KEEP_SAMPLES && m.order.len() > 1 {
        if let Some(old) = m.order.pop_front() {
            if let Some(gone) = m.by_key.remove(&old) {
                m.samples = m.samples.saturating_sub(gone.samples.len());
            }
        }
    }
}

/// The line spoken and timed: from what was made lately when it was.
pub(super) fn say_line(maker: Maker, service: &super::voice_service::VoiceSettings, want: &Want) -> Result<std::sync::Arc<Spoken>, String> {
    let key = key_of(service, want);
    if let Some(line) = recall(&key) {
        return Ok(line);
    }
    let line = std::sync::Arc::new(make_spoken(maker, service, want)?);
    remember(key, line.clone());
    Ok(line)
}

/// Makes several lines at once, so the `narrate` ops that follow find them
/// ready. A hosted voice speaks four lines at a time; the Mac's synthesizer
/// speaks one at a time, and the lines already spoken are timed meanwhile.
/// The lines are the same as when each is made on its turn; a line that
/// fails here fails again, with its own reason, when its turn comes.
pub(super) fn prepare_lines(maker: Maker, service: &super::voice_service::VoiceSettings, wants: &[Want]) {
    let mut todo: Vec<(String, &Want)> = Vec::new();
    for w in wants {
        let key = key_of(service, w);
        if recall(&key).is_none() && !todo.iter().any(|(k, _)| *k == key) {
            todo.push((key, w));
        }
    }
    if todo.len() < 2 {
        return;
    }
    let speakers = if service.provider == super::voice_service::Provider::Mac { 1 } else { todo.len().min(4) };
    let timers = todo.len().min(3);
    let next = std::sync::atomic::AtomicUsize::new(0);
    let (tx, rx) = std::sync::mpsc::channel::<(String, &Want, Raw)>();
    let rx = std::sync::Mutex::new(rx);
    std::thread::scope(|sc| {
        sc.spawn(maker.warm);
        for _ in 0..speakers {
            let tx = tx.clone();
            let (todo, next) = (&todo, &next);
            sc.spawn(move || {
                loop {
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    let Some((key, want)) = todo.get(i) else { break };
                    if let Ok(raw) = speak_raw(maker, service, want) {
                        let _ = tx.send((key.clone(), want, raw));
                    }
                }
            });
        }
        drop(tx);
        for _ in 0..timers {
            let rx = &rx;
            sc.spawn(move || {
                loop {
                    let item = rx.lock().unwrap_or_else(|e| e.into_inner()).recv();
                    let Ok((key, want, raw)) = item else { break };
                    remember(key, std::sync::Arc::new(time_words(maker, raw, &want.text)));
                }
            });
        }
    });
}

/// Forgets every line made lately (tests).
#[cfg(test)]
fn forget_all() {
    *made().lock().unwrap_or_else(|e| e.into_inner()) = Made::default();
}

impl Session {
    /// `narrate`: speaks `say` at `at` (default: after the last line) and
    /// places it on the voice lane. Returns the clip and its words.
    pub(super) fn narrate(&mut self, op: &Value) -> Result<(String, u32, Vec<Said>), String> {
        let text = op.get("say").or_else(|| op.get("text")).and_then(Value::as_str).ok_or("narrate needs \"say\": the words to speak")?.trim().to_owned();
        if text.is_empty() {
            return Err("narrate needs words to say".into());
        }
        // The service chosen in Settings speaks; the theme's and the
        // default voice are Mac voices, used only when the Mac speaks.
        let service = super::voice_service::VoiceSettings::load();
        let want = self.narration_want(op, &text, &service);
        let spoken = say_line(Maker::real(), &service, &want)?;
        let (samples, speech_rate, by, len) = (&spoken.samples, spoken.rate, spoken.by.clone(), spoken.samples.len() as f64 / spoken.rate as f64);
        let words = spoken.words.clone();
        // Stored once per distinct line and voice, words beside it.
        let wav = studio_media::wav::Wav { sample_rate: speech_rate, channels: 2, samples: samples.iter().flat_map(|s| [*s, *s]).collect() };
        let dir = self.home.clone().map(|h| h.join("voice")).unwrap_or_else(|| studio_paths::cache().join("made-voice"));
        let path = crate::files::write_once(&dir, "line", "wav", &studio_media::wav::encode_pcm16(&wav)).map_err(|e| format!("could not write the narration: {e}"))?;
        let _ = std::fs::write(path.with_extension("words.json"), serde_json::to_vec(&words).unwrap_or_default());
        // Its phonemes, timed on the sound (word times fence them in only
        // when they were heard), kept for any mouth that says the line.
        let wins: Vec<studio_speech::WordIn> = words.iter().map(|w| if spoken.heard { studio_speech::WordIn::timed(&w.text, w.start, w.end) } else { studio_speech::WordIn::plain(&w.text) }).collect();
        if let Ok(al) = studio_speech::align(samples, speech_rate, &wins) {
            let _ = std::fs::write(path.with_extension("phonemes.tsv"), al.to_tsv());
        }
        let report = crate::services::project::import_media(&mut self.doc, &[path.clone()]);
        let media = *report.ids.first().ok_or("the narration could not be imported")?;
        let p = &self.doc.project;
        let at = f(op, "at").unwrap_or_else(|| secs(p.clips.iter().filter(|c| c.kind == ClipKind::Audio && c.name.starts_with("line-")).map(|c| c.end()).max().unwrap_or(0)));
        let before: std::collections::BTreeSet<u32> = p.clips.iter().map(|c| c.id).collect();
        let mut place = json!({"op": "place", "media": media, "at": at, "lane": "voice"});
        if let Some(v) = f(op, "volume") {
            place["volume"] = json!(v);
        }
        self.op(&place)?;
        let id = self.doc.project.clips.iter().map(|c| c.id).find(|id| !before.contains(id)).ok_or("the narration was not placed")?;
        let words: Vec<Said> = words.into_iter().map(|w| Said { text: w.text, start: w.start + at, end: w.end + at }).collect();
        // The character that says it moves its mouth to it.
        let mut by = by;
        if let Some(sp) = op.get("speaker").and_then(Value::as_u64) {
            let mut l = json!({"op": "lip_sync", "clip": sp, "audio": id});
            for k in ["intensity", "emotion"] {
                if let Some(v) = op.get(k) {
                    l[k] = v.clone();
                }
            }
            match self.lip_sync(&l) {
                Ok(note) => by = format!("{by}; {note}"),
                Err(e) => by = format!("{by}; the mouth of clip {sp} was not synced: {e}"),
            }
        }
        // The music steps back under every line (unless asked not to).
        let ducked = if op.get("duck").and_then(Value::as_bool).unwrap_or(true) { self.duck_music(f(op, "duck_db").unwrap_or(12.0).clamp(3.0, 24.0))? } else { 0 };
        Ok((format!("narration {:.2}–{:.2}s in {} ({} words, at speech level{})", at, at + len, by, words.len(), if ducked > 0 { "; the music dips under it" } else { "" }), id, words))
    }

    /// What a `narrate` op asks the voice for: its words, voice and speed.
    pub(super) fn narration_want(&self, op: &Value, text: &str, service: &super::voice_service::VoiceSettings) -> Want {
        let mac = service.provider == super::voice_service::Provider::Mac;
        let voice = op.get("voice").and_then(Value::as_str).map(str::to_owned).or_else(|| if mac { self.theme_voice().or_else(default_voice) } else { None });
        Want { text: text.to_owned(), voice, wpm: f(op, "rate").map(|r| r as f32) }
    }

    /// Makes ahead the narration an edit's ops will ask for (`narrate`, and
    /// `scene` with words to say), several lines at a time.
    pub(super) fn prepare_narration(&self, ops: &[Value]) {
        let service = super::voice_service::VoiceSettings::load();
        let mut wants = Vec::new();
        for op in ops {
            let op = super::args::unwrap_op(op);
            match op.get("op").and_then(Value::as_str) {
                Some("narrate") => {
                    let text = op.get("say").or_else(|| op.get("text")).and_then(Value::as_str).unwrap_or("").trim();
                    if !text.is_empty() {
                        wants.push(self.narration_want(&op, text, &service));
                    }
                }
                Some("scene") if op.get("voice") != Some(&Value::Bool(false)) => {
                    let text = op.get("say").and_then(Value::as_str).unwrap_or("").trim();
                    if !text.is_empty() {
                        let mut n = json!({});
                        if let Some(v) = op.get("voice").filter(|v| v.is_string()) {
                            n["voice"] = v.clone();
                        }
                        wants.push(self.narration_want(&n, text, &service));
                    }
                }
                _ => {}
            }
        }
        prepare_lines(Maker::real(), &service, &wants);
    }

    /// Every spoken word on the timeline, in order (timeline seconds).
    pub(super) fn spoken_words(&self) -> Vec<Said> {
        let p = &self.doc.project;
        let mut out = Vec::new();
        for c in p.clips.iter().filter(|c| c.kind == ClipKind::Audio && !p.lane(c.lane).is_some_and(|l| l.muted)) {
            let Some(mi) = c.media.and_then(|m| p.media(m)) else { continue };
            let Some(path) = &mi.path else { continue };
            let Ok(bytes) = std::fs::read(path.with_extension("words.json")) else { continue };
            let Ok(ws) = serde_json::from_slice::<Vec<Said>>(&bytes) else { continue };
            let off = secs(c.start) - secs(c.src_in);
            for w in ws {
                let (s, e) = (w.start + off, w.end + off);
                if s >= secs(c.start) - 0.01 && s < secs(c.end()) {
                    out.push(Said { text: w.text, start: s, end: e.min(secs(c.end())) });
                }
            }
        }
        out.sort_by(|a, b| a.start.total_cmp(&b.start));
        out
    }

    /// The music steps back while someone speaks: its volume dips under
    /// every stretch of speech (gaps under 0.6 s joined), 0.15 s down,
    /// 0.4 s back up.
    pub(super) fn duck_music(&mut self, depth_db: f64) -> Result<usize, String> {
        let words = self.spoken_words();
        if words.is_empty() {
            return Ok(0);
        }
        let mut spans: Vec<(f64, f64)> = Vec::new();
        for w in &words {
            match spans.last_mut() {
                Some(s) if w.start - s.1 < 0.6 => s.1 = s.1.max(w.end),
                _ => spans.push((w.start, w.end)),
            }
        }
        let p = &self.doc.project;
        let music: Vec<(u32, f64, f64, f64)> = p
            .clips
            .iter()
            .filter(|c| c.kind == ClipKind::Audio && c.media.and_then(|m| p.media(m)).is_some_and(|mi| mi.dur >= 20 * SEC))
            .map(|c| (c.id, secs(c.start), secs(c.end()), c.audio.volume as f64))
            .collect();
        let mut n = 0;
        for (id, s, e, base) in music {
            let mut keys: Vec<Value> = vec![json!([0.0, base])];
            for &(a, b) in &spans {
                let (a, b) = (a - s, b - s);
                if b < 0.0 || a > e - s {
                    continue;
                }
                keys.push(json!([(a - 0.15).max(0.0), base]));
                keys.push(json!([a.max(0.0), base - depth_db]));
                keys.push(json!([b.min(e - s), base - depth_db]));
                keys.push(json!([(b + 0.4).min(e - s), base]));
            }
            keys.push(json!([e - s, base]));
            // Keys in time order, each moment once.
            let mut ks: Vec<(f64, f64)> = keys.iter().filter_map(|k| Some((k[0].as_f64()?, k[1].as_f64()?))).collect();
            ks.sort_by(|a, b| a.0.total_cmp(&b.0));
            ks.dedup_by(|b, a| (a.0 - b.0).abs() < 0.02);
            let keys: Vec<Value> = ks.iter().map(|(t, v)| json!([t, v])).collect();
            let _ = self.op(&json!({"op": "keys", "clip": id, "property": "volume", "remove": "all"}));
            self.op(&json!({"op": "animate", "clip": id, "property": "volume", "keys": keys, "ease": "smooth"}))?;
            n += 1;
        }
        Ok(n)
    }

    /// Who speaks, and the installed voices, best first.
    pub(super) fn voices(&self) -> Value {
        use super::voice_service::{Provider, VoiceSettings};
        let s = VoiceSettings::load();
        if s.provider != Provider::Mac || !s.mac_voice_allowed() {
            let (voice, how) = match s.provider {
                Provider::Fish => (s.fish_voice.clone(), "a Fish Audio voice model id (fish.audio); empty: Fish's default voice"),
                Provider::ElevenLabs => (s.elevenlabs_voice.clone(), "an ElevenLabs voice id"),
                Provider::OpenAi => (s.openai_voice.clone(), "an OpenAI voice name (alloy, ash, coral, echo, fable, nova, onyx, sage, shimmer)"),
                Provider::Mac => (String::new(), "the Mac voices are switched off: nothing can be spoken until a service is chosen in Settings"),
            };
            return json!({
                "speaking": s.provider.name(),
                "voice": voice,
                "voice_is": how,
                "mac_voices": if s.mac_voice_allowed() { "allowed" } else { "switched off: never used, even by name" },
                "note": "narrate's \"voice\" takes a voice of this service; leave it out for the one chosen in Settings",
            });
        }
        let v = studio_hw::speech::voices();
        let best = default_voice();
        json!({
            "default": best,
            "voices": v.iter().filter(|x| x.locale.starts_with("en")).map(|x| json!({"name": x.name, "id": x.id, "locale": x.locale, "quality": x.quality})).collect::<Vec<_>>(),
            "note": if v.iter().any(|x| x.quality != "compact") { "premium and enhanced voices sound most natural" } else { "only compact voices are installed; Premium voices (System Settings > Accessibility > Spoken Content > System voice > Manage Voices) sound far more natural" },
        })
    }

    fn theme_voice(&self) -> Option<String> {
        self.doc.model.settings.extra.get("theme").and_then(|t| t.get("voice")).and_then(Value::as_str).map(str::to_owned)
    }
}

/// Puts the script's words on the recognised ones (same order; where the
/// recogniser split or merged words, the script word goes to the nearest).
fn align_to_script(words: &mut [Said], script: &str) {
    let norm = |s: &str| s.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect::<String>();
    let tokens: Vec<&str> = script.split_whitespace().collect();
    if tokens.len() == words.len() {
        for (w, t) in words.iter_mut().zip(&tokens) {
            w.text = (*t).to_owned();
        }
        return;
    }
    // Walk both: a heard word takes the next script word that matches it.
    let mut j = 0;
    for w in words.iter_mut() {
        let h = norm(&w.text);
        if let Some(k) = (j..tokens.len().min(j + 4)).find(|&k| norm(tokens[k]) == h) {
            w.text = tokens[k].to_owned();
            j = k + 1;
        }
    }
}

/// Words a caption group can hold, by how fast they are spoken.
pub fn caption_groups(words: &[Said]) -> Vec<Vec<Said>> {
    let mut groups: Vec<Vec<Said>> = Vec::new();
    let mut cur: Vec<Said> = Vec::new();
    let mut cap = 4;
    for (i, w) in words.iter().enumerate() {
        let gap = cur.last().map_or(0.0, |l: &Said| w.start - l.end);
        if !cur.is_empty() && (gap > 0.18 || cur.len() >= cap) {
            groups.push(std::mem::take(&mut cur));
        }
        if cur.is_empty() {
            // Words in the next second set how many fit: fast speech, fewer.
            let dense = words[i..].iter().take_while(|x| x.start < w.start + 1.0).count();
            cap = if dense as f64 > 3.5 { 2 } else if dense as f64 > 2.5 { 3 } else { 4 };
        }
        cur.push(w.clone());
        if w.text.ends_with(['.', '?', '!', ',', ';', ':']) {
            groups.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        groups.push(cur);
    }
    groups
}

#[allow(dead_code)]
fn _unused(_: view::Time) {}

#[cfg(test)]
mod tests {
    use super::*;

    use super::super::voice_service::{Line, Provider, VoiceSettings};
    use std::sync::atomic::{AtomicUsize, Ordering};

    static SPOKEN: AtomicUsize = AtomicUsize::new(0);
    static NOW: AtomicUsize = AtomicUsize::new(0);
    static MOST: AtomicUsize = AtomicUsize::new(0);
    /// The made lines and the counters are shared: these tests take turns.
    static TURN: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// A voice that takes 60 ms a line and makes a tone as long as the text.
    fn fake_speak(_: &VoiceSettings, text: &str, voice: Option<&str>, wpm: Option<f32>) -> Result<Line, String> {
        SPOKEN.fetch_add(1, Ordering::SeqCst);
        let now = NOW.fetch_add(1, Ordering::SeqCst) + 1;
        MOST.fetch_max(now, Ordering::SeqCst);
        std::thread::sleep(std::time::Duration::from_millis(60));
        NOW.fetch_sub(1, Ordering::SeqCst);
        let n = 1600 * text.len().max(1) + voice.map_or(0, str::len) + wpm.map_or(0, |w| w as usize);
        Ok(Line { samples: (0..n).map(|i| 0.3 * ((i as f32) * 0.05).sin()).collect(), rate: 16_000, by: "fake".into() })
    }

    /// A recogniser that takes 60 ms and hears one word per second.
    fn fake_words(samples: &[f32], rate: u32) -> Vec<Said> {
        std::thread::sleep(std::time::Duration::from_millis(60));
        (0..(samples.len() as f64 / rate as f64) as usize).map(|i| Said { text: format!("w{i}"), start: i as f64, end: i as f64 + 0.8 }).collect()
    }

    fn fake() -> Maker {
        Maker { speak: fake_speak, words: fake_words, warm: || {} }
    }

    fn want(text: &str) -> Want {
        Want { text: text.into(), voice: None, wpm: None }
    }

    fn hosted() -> VoiceSettings {
        VoiceSettings { provider: Provider::OpenAi, ..Default::default() }
    }

    /// The same line in the same voice is made once.
    #[test]
    fn a_line_said_twice_is_made_once() {
        let _turn = TURN.lock().unwrap_or_else(|e| e.into_inner());
        forget_all();
        let service = hosted();
        let before = SPOKEN.load(Ordering::SeqCst);
        let a = say_line(fake(), &service, &want("Once upon a cached time.")).unwrap();
        let b = say_line(fake(), &service, &want("Once upon a cached time.")).unwrap();
        assert_eq!(SPOKEN.load(Ordering::SeqCst) - before, 1, "spoken once");
        assert!(std::sync::Arc::ptr_eq(&a, &b));
        // Another voice or speed is another line.
        let c = say_line(fake(), &service, &Want { voice: Some("alloy".into()), ..want("Once upon a cached time.") }).unwrap();
        assert_ne!(c.samples.len(), a.samples.len());
        assert_eq!(SPOKEN.load(Ordering::SeqCst) - before, 2);
    }

    /// Lines made ahead, several at once, are the lines that come from
    /// making each on its turn, and take less time.
    #[test]
    fn lines_made_ahead_are_the_same_lines_and_come_sooner() {
        let _turn = TURN.lock().unwrap_or_else(|e| e.into_inner());
        forget_all();
        let service = hosted();
        let texts = ["First of the ahead lines.", "Second of the ahead lines, a little longer.", "Third.", "Fourth of the ahead lines, the longest of all of them here."];
        let wants: Vec<Want> = texts.iter().map(|t| want(t)).collect();
        // Each on its turn, nothing remembered.
        let t = std::time::Instant::now();
        let alone: Vec<Spoken> = wants.iter().map(|w| make_spoken(fake(), &service, w).unwrap()).collect();
        let one_by_one = t.elapsed();
        // Ahead.
        let t = std::time::Instant::now();
        prepare_lines(fake(), &service, &wants);
        let ahead = t.elapsed();
        for (w, want_line) in wants.iter().zip(&alone) {
            let got = recall(&key_of(&service, w)).expect("made ahead");
            assert_eq!(got.samples, want_line.samples);
            assert_eq!(got.rate, want_line.rate);
            assert_eq!(got.by, want_line.by);
            assert_eq!(got.words.iter().map(|x| (x.text.clone(), x.start.to_bits(), x.end.to_bits())).collect::<Vec<_>>(), want_line.words.iter().map(|x| (x.text.clone(), x.start.to_bits(), x.end.to_bits())).collect::<Vec<_>>());
        }
        assert!(ahead < one_by_one.mul_f32(0.7), "ahead {ahead:?} against one by one {one_by_one:?}");
        // The turns then find them ready: nothing more is spoken.
        let before = SPOKEN.load(Ordering::SeqCst);
        for w in &wants {
            say_line(fake(), &service, w).unwrap();
        }
        assert_eq!(SPOKEN.load(Ordering::SeqCst), before);
    }

    /// The Mac's synthesizer speaks one line at a time; only the timing of
    /// the words overlaps.
    #[test]
    fn the_mac_voice_is_never_asked_for_two_lines_at_once() {
        let _turn = TURN.lock().unwrap_or_else(|e| e.into_inner());
        forget_all();
        let service = VoiceSettings { provider: Provider::Mac, ..Default::default() };
        MOST.store(0, Ordering::SeqCst);
        let wants: Vec<Want> = ["Mac one line here.", "Mac two lines here.", "Mac three lines here."].iter().map(|t| want(t)).collect();
        prepare_lines(fake(), &service, &wants);
        assert_eq!(MOST.load(Ordering::SeqCst), 1);
        assert!(wants.iter().all(|w| recall(&key_of(&service, w)).is_some()));
    }

    /// A line the voice cannot make is left for its own turn, which reports
    /// why; the others are made.
    #[test]
    fn a_failing_line_does_not_stop_the_others() {
        let _turn = TURN.lock().unwrap_or_else(|e| e.into_inner());
        fn picky(s: &VoiceSettings, text: &str, v: Option<&str>, w: Option<f32>) -> Result<Line, String> {
            if text.contains("refused") { Err("the voice refused".into()) } else { fake_speak(s, text, v, w) }
        }
        forget_all();
        let service = hosted();
        let maker = Maker { speak: picky, words: fake_words, warm: || {} };
        let wants = vec![want("A fine line."), want("A refused line."), want("Another fine line.")];
        prepare_lines(maker, &service, &wants);
        assert!(recall(&key_of(&service, &wants[0])).is_some() && recall(&key_of(&service, &wants[2])).is_some());
        assert!(recall(&key_of(&service, &wants[1])).is_none());
        assert_eq!(say_line(maker, &service, &wants[1]).err().as_deref(), Some("the voice refused"));
    }


    fn w(t: &str, s: f64, e: f64) -> Said {
        Said { text: t.into(), start: s, end: e }
    }

    #[test]
    fn captions_group_by_pace_pauses_and_sentences() {
        let ws = vec![w("Our", 0.0, 0.2), w("seagull", 0.25, 0.6), w("got", 0.65, 0.8), w("dumped.", 0.85, 1.2), w("He", 2.0, 2.1), w("is", 2.15, 2.25), w("so", 2.3, 2.45), w("sad", 2.5, 2.9)];
        let g = caption_groups(&ws);
        let texts: Vec<String> = g.iter().map(|g| g.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ")).collect();
        assert_eq!(texts[0..2], ["Our seagull".to_owned(), "got dumped.".to_owned()]);
        assert!(texts.last().unwrap().ends_with("sad"));
        // A pause always starts a new group.
        assert!(g.iter().all(|g| g.windows(2).all(|p| p[1].start - p[0].end <= 0.18)));
    }

    #[test]
    fn even_words_cover_the_line() {
        let ws = even_words("one two three", 3.0);
        assert_eq!(ws.len(), 3);
        assert!(ws[2].start < 3.0 && ws[0].start == 0.0);
    }
}
