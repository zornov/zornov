// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Sound made here, from nothing but words: original music in a genre
//! (`music`), foley and effects from physical models (`sfx`), and a sung
//! voice from notes and syllables (`sing`). All of it is synthesised by
//! `studio-synth`, so it needs no file, no service and no network; the
//! replies say what was made and how, and where it is limited.

use super::{Session, f, secs};
use crate::view::{ClipKind, SEC};
use serde_json::{Value, json};
use studio_synth::dsp::{RATE, Stereo};
use studio_synth::music::{Cue, CueKind, MusicSpec};
use studio_synth::sing::{SungNote, Voice, VoiceKind};
use studio_synth::theory::parse_pitch;

/// A file-name-safe stem from words.
pub(super) fn slug(text: &str, max: usize) -> String {
    let s: String = text.to_lowercase().chars().map(|c| if c.is_alphanumeric() { c } else { ' ' }).collect::<String>().split_whitespace().collect::<Vec<_>>().join("-");
    s.chars().take(max).collect::<String>().trim_matches('-').to_owned()
}

fn number_or_word(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(true) => Some("yes".into()),
        _ => None,
    }
}

/// The sound a set of `sfx` fields describes, in words the models read:
/// `kind: "footsteps", surface: "gravel", count: 8, distance: "far"` is
/// "distant 8 footsteps on gravel".
pub(super) fn sfx_words(kind: &str, op: &Value) -> String {
    let mut pre: Vec<String> = Vec::new();
    let mut post: Vec<String> = Vec::new();
    let text = |k: &str| op.get(k).and_then(number_or_word);
    if let Some(d) = op.get("distance") {
        match d {
            Value::Number(n) => pre.push(if n.as_f64().unwrap_or(0.0) > 0.5 { "distant".into() } else { "close".into() }),
            Value::String(s) => pre.push(s.clone()),
            _ => {}
        }
    }
    if op.get("period").and_then(Value::as_bool).unwrap_or(false) {
        pre.push("period".into());
    } else if let Some(e) = text("era") {
        pre.push(format!("{e} period"));
    }
    for k in ["size", "speed", "intensity", "material", "shoe"] {
        if let Some(v) = text(k) {
            if k == "shoe" {
                post.push(format!("in {v}"));
            } else {
                pre.push(v);
            }
        }
    }
    if let Some(c) = text("count") {
        pre.push(c);
    }
    let species = text("species").or_else(|| text("animal"));
    let mut core = match species {
        Some(sp) => format!("{sp} {}", text("call").unwrap_or_default()),
        None => kind.to_owned(),
    };
    if let Some(s) = text("surface") {
        core.push_str(&format!(" on {s}"));
    }
    if let Some(t) = op.get("tail") {
        if !matches!(t, Value::Bool(false) | Value::Null) {
            core.push_str(" with a tail");
        }
    }
    format!("{} {} {}", pre.join(" "), core.trim(), post.join(" ")).split_whitespace().collect::<Vec<_>>().join(" ")
}

fn cue_list(op: &Value, offset: f64) -> Result<Vec<Cue>, String> {
    let mut out = Vec::new();
    let mut push = |at: f64, kind: &str| -> Result<(), String> {
        let k = CueKind::parse(kind).ok_or_else(|| format!("cue \"{kind}\": hit, resolve, riser, drop or swell"))?;
        if at - offset > 0.05 {
            out.push(Cue { at: (at - offset) as f32, kind: k });
        }
        Ok(())
    };
    let one = |v: &Value, push: &mut dyn FnMut(f64, &str) -> Result<(), String>| -> Result<(), String> {
        match v {
            Value::Object(o) => {
                let at = o.get("at").or_else(|| o.get("time")).or_else(|| o.get("t")).and_then(Value::as_f64).ok_or("a cue needs \"at\" (seconds)")?;
                let kind = o.get("kind").or_else(|| o.get("type")).or_else(|| o.get("cue")).and_then(Value::as_str).unwrap_or("hit");
                push(at, kind)
            }
            Value::Array(a) if a.len() == 2 => {
                let (t, k) = match (a[0].as_f64(), a[1].as_f64()) {
                    (Some(t), _) => (t, a[1].as_str().unwrap_or("hit")),
                    (_, Some(t)) => (t, a[0].as_str().unwrap_or("hit")),
                    _ => return Err("a cue is [seconds, kind]".into()),
                };
                push(t, k)
            }
            Value::String(s) => {
                let norm = s.replace('@', " ").replace(" at ", " ");
                let mut kind = "hit";
                let mut at = None;
                for w in norm.split_whitespace() {
                    match w.trim_end_matches('s').parse::<f64>() {
                        Ok(t) => at = Some(t),
                        Err(_) => kind = w,
                    }
                }
                push(at.ok_or_else(|| format!("cue \"{s}\" has no time: \"hit@2.0\""))?, kind)
            }
            _ => Err("a cue is {\"at\": seconds, \"kind\": hit|resolve|riser|drop|swell}".into()),
        }
    };
    match op.get("cues") {
        Some(Value::Array(a)) => {
            for c in a {
                one(c, &mut push)?;
            }
        }
        Some(Value::Object(o)) => {
            for (k, v) in o {
                match v {
                    Value::Array(ts) => {
                        for t in ts.iter().filter_map(Value::as_f64) {
                            push(t, k)?;
                        }
                    }
                    other => {
                        if let Some(t) = other.as_f64() {
                            push(t, k)?;
                        }
                    }
                }
            }
        }
        _ => {}
    }
    // Shorthand: "hit": 2.0, "resolve": 14.5, "riser": [8, 20].
    for name in ["hit", "hits", "resolve", "riser", "drop", "swell"] {
        match op.get(name) {
            Some(Value::Array(ts)) => {
                for t in ts.iter().filter_map(Value::as_f64) {
                    push(t, name.trim_end_matches('s'))?;
                }
            }
            Some(v) => {
                if let Some(t) = v.as_f64() {
                    push(t, name.trim_end_matches('s'))?;
                }
            }
            None => {}
        }
    }
    out.sort_by(|a, b| a.at.total_cmp(&b.at));
    Ok(out)
}

impl Session {
    fn made_dir(&self, sub: &str) -> std::path::PathBuf {
        self.home.clone().map(|h| h.join(sub)).unwrap_or_else(|| studio_paths::cache().join(format!("made-{sub}")))
    }

    /// Imports a made WAV and returns its media id.
    fn import_made(&mut self, dir: &std::path::Path, stem: &str, audio: &Stereo) -> Result<u32, String> {
        let path = crate::files::write_once(dir, stem, "wav", &audio.wav_bytes()).map_err(|e| format!("could not write the sound: {e}"))?;
        let report = crate::services::project::import_media(&mut self.doc, &[path]);
        report.ids.first().copied().ok_or_else(|| "the made sound could not be imported".to_owned())
    }

    /// Runs an op and returns the clips it made.
    fn new_clips_of(&mut self, op: &Value) -> Result<Vec<u32>, String> {
        let before: std::collections::BTreeSet<u32> = self.doc.project.clips.iter().map(|c| c.id).collect();
        self.op(op)?;
        let mut new: Vec<u32> = self.doc.project.clips.iter().map(|c| c.id).filter(|id| !before.contains(id)).collect();
        new.sort();
        Ok(new)
    }

    /// `sfx` made here: foley and effects from a description.
    pub(super) fn sfx_local(&mut self, desc: &str, op: &Value, why: &str) -> Result<String, String> {
        let want = f(op, "duration").map(|d| d as f32);
        let stem = format!("sfx-{}", slug(desc, 30));
        let variant = op.get("variant").and_then(Value::as_u64).unwrap_or_else(|| {
            let prefix = format!("{stem}-");
            self.doc.project.media.iter().filter(|m| m.name.starts_with(&prefix)).count() as u64
        });
        let made = studio_synth::sfx::make(desc, want, variant)?;
        let dur = made.audio.seconds() as f64;
        let anchored = made.anchor.is_some();
        let at = match (f(op, "at"), f(op, "ends_at")) {
            (Some(a), _) => a,
            (None, Some(e)) => e - made.anchor.map_or(dur, |a| a as f64),
            _ => return Err("sfx needs \"at\" (where it starts) or \"ends_at\" (where it peaks or ends)".into()),
        }
        .max(0.0);
        let dir = self.made_dir("sfx");
        let media = self.import_made(&dir, &format!("{stem}-{variant}"), &made.audio)?;
        let mut place = json!({"op": "place", "media": media, "at": at, "lane": op.get("lane").cloned().unwrap_or(json!("sound"))});
        if let Some(v) = f(op, "volume") {
            place["volume"] = json!(v);
        }
        let note = self.place(&place)?;
        let moment = match (made.anchor, op.get("ends_at")) {
            (Some(a), Some(_)) => format!(", its peak at {:.2}s", at + a as f64),
            (Some(a), None) => format!(", its main moment {:.2}s in", a),
            _ => String::new(),
        };
        let _ = anchored;
        Ok(format!("made {:.2}s of \"{}\" here by physical models ({why}; take {variant}, `variant` makes another){moment}: {note}", dur, made.model))
    }

    /// `sing`: a sung part per voice, each its own clip for lip sync.
    pub(super) fn sing(&mut self, op: &Value) -> Result<String, String> {
        let bpm = f(op, "bpm").or_else(|| f(op, "tempo")).unwrap_or(120.0).clamp(20.0, 300.0);
        let beat = (60.0 / bpm) as f32;
        let voices: Vec<Value> = match op.get("voices") {
            Some(Value::Array(a)) if !a.is_empty() => a.clone(),
            _ => vec![op.clone()],
        };
        let mut parts: Vec<Voice> = Vec::new();
        for (i, v) in voices.iter().enumerate() {
            let notes_v = v.get("notes").or_else(|| v.get("part")).or_else(|| v.get("score")).ok_or("sing needs \"notes\": \"C4:0.5:doo E4:0.5:bee G4:1:bap\" (pitch:length:syllable, r:length for a rest) or a list of {pitch, start, dur, syl}")?;
            let notes: Vec<SungNote> = match notes_v {
                Value::String(s) => studio_synth::sing::parse_part(s, beat)?,
                Value::Array(a) => {
                    let mut out = Vec::new();
                    let mut t = 0.0f32;
                    for n in a {
                        if let Value::String(s) = n {
                            let mut part = studio_synth::sing::parse_part(s, beat)?;
                            for p in &mut part {
                                p.start += t;
                            }
                            t = part.last().map_or(t, |l| l.start + l.dur);
                            out.extend(part);
                            continue;
                        }
                        let pitch = n.get("pitch").or_else(|| n.get("note")).or_else(|| n.get("midi")).ok_or("a note needs \"pitch\" (C4, F#3 or a MIDI number)")?;
                        let midi = match pitch {
                            Value::Number(x) => x.as_f64().unwrap_or(60.0) as f32,
                            Value::String(s) => parse_pitch(s).ok_or_else(|| format!("\"{s}\" is not a pitch (C4, F#3, Bb2 or a MIDI number)"))?,
                            _ => return Err("a note's pitch is a name or a number".into()),
                        };
                        let start = n.get("start").or_else(|| n.get("at")).and_then(Value::as_f64).map(|x| x as f32).unwrap_or(t);
                        let dur = n.get("dur").or_else(|| n.get("duration")).or_else(|| n.get("length")).and_then(Value::as_f64).map(|x| x as f32).unwrap_or(0.5).max(0.05);
                        let syl = n.get("syl").or_else(|| n.get("syllable")).or_else(|| n.get("text")).and_then(Value::as_str).unwrap_or("la").to_owned();
                        let vel = n.get("vel").or_else(|| n.get("velocity")).and_then(Value::as_f64).unwrap_or(0.8) as f32;
                        t = start + dur;
                        out.push(SungNote { midi, start, dur, syllable: syl, vel });
                    }
                    out
                }
                _ => return Err("notes is a string or a list".into()),
            };
            if notes.is_empty() {
                return Err("sing: that voice has no notes".into());
            }
            let mut sorted: Vec<f32> = notes.iter().map(|n| n.midi).collect();
            sorted.sort_by(f32::total_cmp);
            let median = sorted[sorted.len() / 2];
            let kind = v.get("voice").or_else(|| v.get("kind")).or_else(|| v.get("type")).and_then(Value::as_str).and_then(VoiceKind::parse).unwrap_or(if median >= 69.0 { VoiceKind::Soprano } else if median >= 60.0 { VoiceKind::Alto } else if median >= 50.0 { VoiceKind::Tenor } else { VoiceKind::Bass });
            let name = v.get("name").and_then(Value::as_str).map(str::to_owned).unwrap_or_else(|| if voices.len() > 1 { format!("{} {}", kind.name(), i + 1) } else { kind.name().to_owned() });
            parts.push(Voice {
                name,
                kind,
                notes,
                pan: v.get("pan").and_then(Value::as_f64).unwrap_or(if voices.len() > 1 { -0.5 + i as f64 / (voices.len() - 1) as f64 } else { 0.0 }) as f32,
                vibrato: v.get("vibrato").and_then(Value::as_f64).unwrap_or(0.3) as f32,
                breath: v.get("breath").and_then(Value::as_f64).unwrap_or(0.12) as f32,
            });
        }
        let seed = op.get("seed").and_then(Value::as_u64).unwrap_or_else(|| self.doc.project.media.iter().filter(|m| m.name.starts_with("sung-")).count() as u64 + 1);
        let stems = studio_synth::sing::sing(&parts, seed);
        let at = f(op, "at").unwrap_or(0.0).max(0.0);
        let dir = self.made_dir("sung");
        let mut said = Vec::new();
        let mut warned = Vec::new();
        for (st, v) in stems.iter().zip(&parts) {
            let mut audio = Stereo::from_mono(&st.audio);
            audio.edges(0.01);
            let media = self.import_made(&dir, &format!("sung-{}-{seed}", slug(&st.name, 20)), &audio)?;
            // Each voice on its own lane, so they can overlap and be mixed or muted apart.
            let lane = match op.get("lane").and_then(Value::as_str) {
                Some(l) if voices.len() == 1 => l.to_owned(),
                _ => format!("sung {}", st.name),
            };
            if super::craft::lane_of(&self.doc.project, &json!(lane), Some(crate::view::MediaKind::Audio)).is_err() {
                self.op(&json!({"op": "lane", "action": "add", "kind": "sound", "name": lane}))?;
            }
            let mut place = json!({"op": "place", "media": media, "at": at, "lane": lane});
            if let Some(vol) = f(op, "volume") {
                place["volume"] = json!(vol);
            }
            let ids = self.new_clips_of(&place)?;
            let id = *ids.first().ok_or("the sung part was not placed")?;
            if v.pan.abs() > 0.01 {
                let _ = self.op(&json!({"op": "set", "clip": id, "pan": v.pan}));
            }
            let sy: Vec<String> = st.syllables.iter().map(|s| format!("{}@{:.2}", s.text, s.start as f64 + at)).collect();
            said.push(format!("{} ({}, {:.2}s) clip {id} on lane \"{lane}\": {}", st.name, v.kind.name(), st.audio.len() as f64 / RATE as f64, sy.join(" ")));
            if st.strained > 0 {
                let (lo, hi) = v.kind.range();
                warned.push(format!("{}: {} note(s) outside a {} range (MIDI {lo:.0}-{hi:.0}); use another voice type or move them", st.name, st.strained, v.kind.name()));
            }
        }
        let mut out = format!("sang {} part(s) here by formant synthesis: {}", stems.len(), said.join("; "));
        if !warned.is_empty() {
            out.push_str(&format!(". Careful: {}", warned.join("; ")));
        }
        out.push_str(". The voice is synthetic: the pitch and timing are exact, the tone is plainly artificial, and it sings syllables (doo, la, bap), not words; tell the person so. Each stem is its own clip, so a mouth can follow it (puppet `voice` with the clip).");
        Ok(out)
    }

    /// `music` made here: an original piece in a genre, fitted to a length
    /// and to cue points. `why` says why it was not taken from the library.
    pub(super) fn compose_music(&mut self, op: &Value, why: Option<&str>) -> Result<String, String> {
        let p = &self.doc.project;
        let video_end = p.clips.iter().filter(|c| c.kind != ClipKind::Audio).map(|c| c.end()).max().unwrap_or(0);
        let at = f(op, "at").unwrap_or(0.0).max(0.0);
        let len = f(op, "duration").or_else(|| f(op, "length")).or_else(|| f(op, "seconds")).or_else(|| (video_end > 0).then(|| (secs(video_end) - at).max(1.0))).ok_or("music needs a length: `duration` in seconds (or make the pictures first and it fits them)")?;
        if len < 0.8 {
            return Err("music: at least 0.8 seconds".into());
        }
        let mut words: Vec<String> = Vec::new();
        for k in ["describe", "style", "genre", "mood"] {
            if let Some(v) = op.get(k).and_then(Value::as_str) {
                words.push(v.to_owned());
            }
        }
        let insts: Vec<String> = match op.get("instruments") {
            Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).map(str::to_owned).collect(),
            Some(Value::String(s)) => s.split(|c| c == ',' || c == '+').map(|x| x.trim().to_owned()).filter(|x| !x.is_empty()).collect(),
            _ => Vec::new(),
        };
        if words.is_empty() && insts.is_empty() {
            words.push("calm".into());
        }
        let desc = words.join(", ");
        let mut spec = MusicSpec::new(&desc, len as f32);
        spec.instruments = insts.clone();
        spec.bpm = f(op, "bpm").or_else(|| f(op, "tempo")).map(|v| v as f32);
        spec.key = op.get("key").and_then(Value::as_str).map(str::to_owned);
        spec.tail = f(op, "tail").map(|v| v as f32);
        spec.cues = cue_list(op, at)?;
        let prefix = format!("music-{}", slug(&format!("{desc} {}", insts.join(" ")), 28));
        let take = op.get("variant").and_then(Value::as_u64).or_else(|| op.get("seed").and_then(Value::as_u64)).unwrap_or_else(|| self.doc.project.media.iter().filter(|m| m.name.starts_with(&format!("{prefix}-"))).count() as u64);
        spec.seed ^= take.wrapping_mul(0x9E37_79B9_7F4A_7C15);
        let piece = studio_synth::music::compose(&spec)?;
        let dir = self.made_dir("music");
        let media = self.import_made(&dir, &format!("{prefix}-{take}"), &piece.audio)?;
        // Made music for the whole film replaces the made music already under it.
        if at <= 0.01 && op.get("duration").is_none() && op.get("length").is_none() {
            let old: Vec<u32> = self.doc.project.clips.iter().filter(|c| c.kind == ClipKind::Audio && c.media.and_then(|m| self.doc.project.media(m)).is_some_and(|mi| mi.name.starts_with("music-") && mi.dur >= 20 * SEC)).map(|c| c.id).collect();
            if !old.is_empty() {
                self.op(&json!({"op": "delete", "clips": old}))?;
            }
        }
        let mut volume = f(op, "volume").unwrap_or(-10.0);
        let ids = self.new_clips_of(&json!({"op": "place", "media": media, "at": at, "lane": op.get("lane").cloned().unwrap_or(json!("music")), "volume": volume}))?;
        let narrated = !self.spoken_words().is_empty();
        let mut tail_note = String::new();
        if narrated {
            if f(op, "volume").is_none() {
                volume = -19.0;
                for id in &ids {
                    let _ = self.op(&json!({"op": "set", "clip": id, "volume": volume}));
                }
            }
            let n = self.duck_music(8.0)?;
            tail_note = format!("; ducking under the narration on {n} clip(s)");
        } else if f(op, "volume").is_none() {
            if let Some(l) = self.check_loudness() {
                volume = (volume + (-16.0 - l)).clamp(-30.0, 0.0);
                for id in &ids {
                    let _ = self.op(&json!({"op": "set", "clip": id, "volume": volume}));
                }
            }
        }
        let how = match why {
            Some(w) => format!("composed here because the library had nothing for it ({w})"),
            None => "composed here, original, no library track used".to_owned(),
        };
        Ok(format!("music {how}: {}; {:.2}-{:.2}s at {volume:.1} dB, clip {}{tail_note}. Same words with `variant: n` compose another take; `cues` ([{{\"at\": 2.0, \"kind\": \"hit\"}}], on the project timeline) land hits, a resolve, a riser, a drop or a swell exactly; `tail` sets the ring-out", piece.plan.describe(), at, at + len, ids.first().map_or(0, |i| *i)))
    }

    /// Whether a `music` op asks for composing rather than a library track.
    pub(super) fn music_wants_composing(&self, op: &Value) -> bool {
        let source = op.get("source").and_then(Value::as_str).unwrap_or("");
        if matches!(source, "library" | "catalogue") {
            return false;
        }
        if matches!(source, "made" | "compose" | "composed" | "local" | "original") {
            return true;
        }
        if op.get("item").is_some() {
            return false;
        }
        ["describe", "style", "genre", "instruments", "bpm", "tempo", "key", "cues", "tail", "seed", "variant", "length", "seconds", "at", "hit", "hits", "resolve", "riser", "drop", "swell"].iter().any(|k| op.get(*k).is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_become_words_the_models_read() {
        let op = json!({"kind": "footsteps", "surface": "gravel", "count": 8, "distance": "far"});
        assert_eq!(sfx_words("footsteps", &op), "far 8 footsteps on gravel");
        let op = json!({"species": "dog", "call": "growl", "size": "big"});
        assert_eq!(sfx_words("animal", &op), "big dog growl");
        assert_eq!(sfx_words("riser", &json!({"tail": true})), "riser with a tail");
        assert_eq!(sfx_words("traffic", &json!({"distance": 0.9, "period": true})), "distant period traffic");
    }

    #[test]
    fn cues_are_read_in_every_form() {
        let op = json!({"cues": [{"at": 2.0, "kind": "hit"}, [6.5, "resolve"], "riser@9"], "drop": 12.0, "hits": [3.0, 4.0]});
        let c = cue_list(&op, 0.0).unwrap();
        assert_eq!(c.len(), 6);
        assert_eq!(c[0], Cue { at: 2.0, kind: CueKind::Hit });
        assert!(c.iter().any(|x| x.kind == CueKind::Drop && x.at == 12.0));
        // Times are on the project timeline: music starting at 1.0 s sees them an offset earlier.
        let c = cue_list(&json!({"cues": {"hit": [3.0], "resolve": 7.0}}), 1.0).unwrap();
        assert_eq!(c, vec![Cue { at: 2.0, kind: CueKind::Hit }, Cue { at: 6.0, kind: CueKind::Resolve }]);
        assert!(cue_list(&json!({"cues": [{"at": 1.0, "kind": "explode"}]}), 0.0).unwrap_err().contains("hit, resolve"));
    }

    #[test]
    fn sung_music_and_effects_land_on_the_timeline() {
        let mut s = Session::default();
        let r = s.edit(&[json!({"op": "sfx", "kind": "footsteps", "surface": "gravel", "count": 4, "at": 1.0})], "steps", false).unwrap();
        assert!(r.to_string().contains("physical models"), "{r}");
        let r = s.edit(&[json!({"op": "music", "genre": "noir jazz with sax and double bass", "duration": 10, "bpm": 90, "cues": [{"at": 4.0, "kind": "hit"}]})], "score", false).unwrap();
        assert!(r.to_string().contains("composed here"), "{r}");
        assert!(s.doc.project.clips.iter().any(|c| c.name.starts_with("music-")), "the piece is on the timeline");
        let r = s.edit(&[json!({"op": "sing", "voices": [{"voice": "soprano", "notes": "E5:0.5:doo G5:0.5:bee"}, {"voice": "tenor", "notes": "C4:0.5:doo r:0.5 E4:0.5:bap"}]})], "scat", false).unwrap();
        assert!(r.to_string().contains("synthetic"), "{r}");
        assert!(s.doc.project.lanes.iter().any(|l| l.name.starts_with("sung soprano")));
    }
}
