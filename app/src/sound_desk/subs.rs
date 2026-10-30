// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Subtitles and captions per language: made from what is said (Whisper
//! word timings on the dialogue stem, segmented and timed by the delivery
//! rules), from caption clips already on the timeline, or read from SRT /
//! VTT; checked and conformed to a specification; translated with the
//! timing kept; written as SRT / VTT; burnt into the picture as styled
//! caption clips; and muxed into exports as subtitle tracks.

use super::{Desk, SubCue, SubTrack};
use crate::doc::Doc;
use serde_json::{Value, json};
use studio_model as m;
use studio_text::subtitles::{SubtitleSpec, base_lang, language_name};
use studio_text::{CaptionStyle, CaptionTrack, Cue, Word};

/// A desk track as a caption track (speakers by name).
pub fn caption_track(t: &SubTrack) -> CaptionTrack {
    let mut ct = CaptionTrack::new(CaptionStyle::default());
    ct.language = t.lang.clone();
    for c in &t.cues {
        let mut cue = Cue::from_text(&c.text.replace('\n', " "), c.start, c.end);
        if let Some(name) = &c.speaker {
            let i = match ct.speakers.iter().position(|s| &s.name == name) {
                Some(i) => i,
                None => ct.add_speaker(name, studio_text::Rgba::WHITE),
            };
            cue.speaker = Some(i);
        }
        ct.cues.push(cue);
    }
    ct
}

/// A caption track as a desk track.
pub fn sub_track(ct: &CaptionTrack, lang: &str, kind: &str) -> SubTrack {
    SubTrack {
        lang: lang.to_owned(),
        name: language_name(lang),
        kind: kind.to_owned(),
        cues: ct.cues.iter().map(|c| SubCue { start: c.start, end: c.end, text: c.display_text(ct.style.max_chars_per_line, ct.style.max_lines), speaker: c.speaker.and_then(|s| ct.speakers.get(s)).map(|s| s.name.clone()) }).collect(),
    }
}

/// The frame rate of the project.
pub fn fps(doc: &Doc) -> f32 {
    doc.project.fps.max(1.0)
}

/// A track from timed words, segmented and conformed to `spec`.
pub fn from_words(words: Vec<Word>, lang: &str, spec: &SubtitleSpec, shots: &[f32]) -> SubTrack {
    let style = CaptionStyle { max_chars_per_line: spec.max_chars_per_line, max_lines: spec.max_lines, reading_speed_cps: spec.max_cps, ..CaptionStyle::default() };
    let mut ct = CaptionTrack::from_words(words, style, |_| None);
    studio_text::subtitles::conform(&mut ct, spec, shots);
    sub_track(&ct, lang, "subtitles")
}

/// Caption clips on the timeline (a lane named "Captions" or "Subtitles…")
/// as a track.
pub fn from_caption_clips(doc: &Doc, lang: &str) -> Option<SubTrack> {
    let p = &doc.project;
    let lane = p.lanes.iter().find(|l| {
        let n = l.name.to_ascii_lowercase();
        n == "captions" || n.starts_with("subtitles") || n.starts_with("captions")
    })?;
    let mut cues: Vec<SubCue> = p
        .clips
        .iter()
        .filter(|c| c.lane == lane.id && !c.text.trim().is_empty())
        .map(|c| SubCue { start: c.start as f32 / 1e6, end: (c.start + c.dur) as f32 / 1e6, text: c.text.trim().to_owned(), speaker: None })
        .collect();
    cues.sort_by(|a, b| a.start.total_cmp(&b.start));
    (!cues.is_empty()).then(|| SubTrack { lang: lang.into(), name: language_name(lang), kind: "subtitles".into(), cues })
}

/// Transcribes the dialogue (the dialogue stem of the whole programme, or
/// the full mix when nothing is marked as dialogue) with the installed
/// Whisper model. `translate`: Whisper's own translation into English.
pub fn transcribe(project: &m::Project, language: Option<&str>, translate: bool) -> Result<(Vec<Word>, String), String> {
    use studio_ai::features::transcribe::{Task, TranscribeOptions, Whisper};
    let end = super::render::end_of(project);
    if end <= 0 {
        return Err("The timeline is empty: nothing to transcribe".into());
    }
    let stereo = studio_sound::surround::Layout::Stereo;
    let req = super::mix::MixRequest { layout: Some(stereo), stem: Some(vec![studio_sound::BusKind::Dialogue]), normalise: false, limiter: false, ..Default::default() };
    let mut chans = super::render::render(project, &req, 0, end);
    if chans[0].iter().all(|v| v.abs() < 1e-5) {
        chans = super::render::render(project, &super::mix::MixRequest { layout: Some(stereo), normalise: false, limiter: false, ..Default::default() }, 0, end);
    }
    let mono: Vec<f32> = chans[0].iter().zip(&chans[1]).map(|(a, b)| 0.5 * (a + b)).collect();
    let store = studio_ai::store::ModelStore::default();
    let whisper = Whisper::load_best(&store).map_err(|e| format!("{e} (Settings ▸ Models installs Whisper)"))?;
    let opts = TranscribeOptions { language: language.map(base_lang).map(str::to_owned), task: if translate { Task::Translate } else { Task::Transcribe }, word_timestamps: !translate };
    let tr = whisper.transcribe(&mono, crate::sound_bridge::RATE, &opts, &studio_ai::jobs::JobCtx::detached()).map_err(|e| e.to_string())?;
    let mut words: Vec<Word> = tr.words().map(|w| Word::new(w.text.trim(), w.start as f32, w.end as f32)).filter(|w| !w.text.is_empty()).collect();
    if words.is_empty() {
        // Segments without word timings (translation): spread the words.
        for s in &tr.segments {
            for w in Cue::from_text(s.text.trim(), s.start as f32, s.end as f32).words {
                words.push(w);
            }
        }
    }
    if words.is_empty() {
        return Err("No speech was found".into());
    }
    Ok((words, if translate { "en".into() } else { tr.language }))
}

/// SRT or VTT text of a track.
pub fn write(t: &SubTrack, vtt: bool) -> String {
    let ct = caption_track(t);
    if vtt { ct.to_vtt(false) } else { ct.to_srt() }
}

/// Reads SRT or VTT.
pub fn read(text: &str, lang: &str) -> Result<SubTrack, String> {
    let ct = CaptionTrack::parse(text, CaptionStyle::default())?;
    Ok(sub_track(&ct, lang, "subtitles"))
}

/// Conforms a track to `spec` (and shot changes); returns how many cues moved.
pub fn conform(t: &mut SubTrack, spec: &SubtitleSpec, shots: &[f32]) -> usize {
    let mut ct = caption_track(t);
    let n = studio_text::subtitles::conform(&mut ct, spec, shots);
    let kind = t.kind.clone();
    *t = sub_track(&ct, &t.lang, &kind);
    n
}

/// What breaks the rules, as `{cue, at, text, problem}` lines.
pub fn check(t: &SubTrack, spec: &SubtitleSpec) -> Vec<Value> {
    let ct = caption_track(t);
    studio_text::subtitles::check(&ct, spec)
        .into_iter()
        .map(|i| {
            let c = &t.cues[i.cue];
            json!({"cue": i.cue + 1, "at": (c.start * 100.0).round() / 100.0, "text": c.text, "problem": i.problem})
        })
        .collect()
}

/// A translation of `source` into `lang` with the same timing: from
/// `texts` (one per cue, the AI's or a translator's) or, without them,
/// from the OpenAI API with the person's key.
pub fn translate(source: &SubTrack, lang: &str, texts: Option<Vec<String>>) -> Result<SubTrack, String> {
    let texts = match texts {
        Some(t) => {
            if t.len() != source.cues.len() {
                return Err(format!("{} texts for {} subtitles: give one translation per subtitle, in order", t.len(), source.cues.len()));
            }
            t
        }
        None => machine_translate(&source.cues.iter().map(|c| c.text.replace('\n', " ")).collect::<Vec<_>>(), &source.lang, lang)?,
    };
    let ct = studio_text::subtitles::translated(&caption_track(source), &texts, lang);
    let mut t = sub_track(&ct, lang, &source.kind);
    t.name = language_name(lang);
    Ok(t)
}

/// The service address; tests point it at a stand-in on this machine with
/// `STUDIO_TRANSLATE_TEST_BASE` (only a local address counts, so a key
/// can never be sent elsewhere this way).
fn base() -> (String, bool) {
    match std::env::var("STUDIO_TRANSLATE_TEST_BASE") {
        Ok(b) if b.starts_with("http://127.0.0.1:") => (b.trim_end_matches('/').to_owned(), true),
        _ => ("https://api.openai.com".to_owned(), false),
    }
}

/// Translates lines with the OpenAI Responses API, one JSON array of
/// strings in and out so lines never merge or split.
pub fn machine_translate(lines: &[String], from: &str, to: &str) -> Result<Vec<String>, String> {
    if lines.is_empty() {
        return Ok(Vec::new());
    }
    let (url, test) = base();
    let key = if test { std::env::var("STUDIO_TRANSLATE_TEST_KEY").ok() } else { crate::agent::keychain::api_key(crate::agent::keychain::OPENAI) }
        .ok_or("Translating needs an OpenAI key (Settings ▸ AI). Or pass the translations yourself: one text per subtitle")?;
    translate_with(&url, &key, lines, from, to)
}

/// [`machine_translate`] against a given service and key.
fn translate_with(url: &str, key: &str, lines: &[String], from: &str, to: &str) -> Result<Vec<String>, String> {
    let model = std::env::var("STUDIO_TRANSLATE_MODEL").unwrap_or_else(|_| crate::agent::openai::MODELS[0].0.to_owned());
    let prompt = format!(
        "Translate these film subtitles from {} to {}. Keep each line's meaning, register and length close to the original so it reads in the same time; keep names; no notes. Reply with only a JSON array of {} strings, one per input line, in order.\n{}",
        language_name(from),
        language_name(to),
        lines.len(),
        serde_json::to_string(lines).map_err(|e| e.to_string())?
    );
    let body = serde_json::to_vec(&json!({"model": model, "input": prompt})).map_err(|e| e.to_string())?;
    let auth = format!("Bearer {key}");
    let reply = studio_net::post(&format!("{url}/v1/responses"), &[("content-type", "application/json"), ("authorization", auth.as_str())], &body, std::time::Duration::from_secs(300))?;
    let status = reply.status;
    let bytes = reply.bytes(16 << 20).map_err(|e| format!("the reply broke off: {e}"))?;
    if !(200..300).contains(&(status as u32)) {
        return Err(format!("translation failed ({status})"));
    }
    let v: Value = serde_json::from_slice(&bytes).map_err(|e| format!("translation reply: {e}"))?;
    let text: String = v.get("output_text").and_then(Value::as_str).map(str::to_owned).unwrap_or_else(|| {
        v.get("output").and_then(Value::as_array).into_iter().flatten().flat_map(|o| o.get("content").and_then(Value::as_array).into_iter().flatten()).filter_map(|c| c.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join("")
    });
    let start = text.find('[').ok_or("the translation came back without the list of lines")?;
    let end = text.rfind(']').ok_or("the translation came back without the list of lines")?;
    let out: Vec<String> = serde_json::from_str(&text[start..=end]).map_err(|e| format!("the translation was not a list of lines: {e}"))?;
    if out.len() != lines.len() {
        return Err(format!("the translation has {} lines for {}", out.len(), lines.len()));
    }
    Ok(out)
}

/// Burns a track into the picture: a lane of styled caption clips (one per
/// subtitle) at the top of the stack. Replaces an earlier burn-in of the
/// same language. Returns the lane id and the clip count.
pub fn burn_in(doc: &mut Doc, t: &SubTrack, style: Option<&str>) -> Result<(u32, usize), String> {
    use crate::view::{ClipKind, LaneKind};
    let name = format!("Subtitles {}", t.lang);
    let old = doc.project.lanes.iter().find(|l| l.name == name).map(|l| l.id);
    let lane_id = old.unwrap_or_else(|| doc.alloc_id() as u32);
    let mut clips = Vec::new();
    for c in &t.cues {
        let start = (c.start as f64 * 1e6) as i64;
        let dur = ((c.end - c.start) as f64 * 1e6) as i64;
        if dur <= 0 {
            continue;
        }
        let mut clip = crate::services::library::new_clip(start, dur, ClipKind::Title, "Subtitle", None);
        clip.text = c.text.clone();
        clip.lane = lane_id;
        clip.id = doc.alloc_id() as u32;
        // Lower third, above the safe margin.
        clip.transform.y = doc.project.height as f32 * 0.36;
        // Readable over any picture: white with a soft shadow.
        clip.text_style = Some(style.map(str::to_owned).unwrap_or_else(|| "clean.ink-shadow".into()));
        clips.push(clip);
    }
    let n = clips.len();
    let ok = doc.edit("Burn in subtitles", |p| {
        if let Some(id) = old {
            p.clips.retain(|c| c.lane != id);
        } else {
            let colour = p.lanes.iter().find(|l| l.kind == LaneKind::Visual).map(|l| l.colour).unwrap_or(eframe::egui::Color32::from_rgb(110, 90, 160));
            let height = p.lanes.iter().find(|l| l.kind == LaneKind::Visual).map(|l| l.height).unwrap_or(44.0);
            p.lanes.insert(0, crate::view::Lane { id: lane_id, name: name.clone(), kind: LaneKind::Visual, locked: false, hidden: false, muted: false, solo: false, height, colour });
        }
        p.clips.extend(clips);
    });
    if !ok {
        return Err(doc.error.take().unwrap_or_else(|| "burn-in was refused".into()));
    }
    Ok((lane_id, n))
}

/// Writes every subtitle track next to an export of `[t0, t1)`
/// (`name.lang.srt` and `.vtt`, timed from the export's start); returns
/// the paths.
pub fn write_sidecars(project: &m::Project, video: &std::path::Path, t0: m::Micros, t1: m::Micros) -> Vec<std::path::PathBuf> {
    let desk = Desk::of(project);
    let mut out = Vec::new();
    let stem = video.file_stem().and_then(|s| s.to_str()).unwrap_or("export").to_owned();
    let dir = video.parent().map(std::path::Path::to_path_buf).unwrap_or_default();
    let (a, b) = (t0 as f32 / 1e6, t1 as f32 / 1e6);
    for t in &desk.subtitles {
        let mut t = t.clone();
        t.cues.retain(|c| c.end > a && c.start < b);
        for c in &mut t.cues {
            c.start = c.start.max(a) - a;
            c.end = c.end.min(b) - a;
        }
        let t = &t;
        let tag = if t.kind == "subtitles" || t.kind.is_empty() { t.lang.clone() } else { format!("{}.{}", t.lang, t.kind) };
        for (ext, vtt) in [("srt", false), ("vtt", true)] {
            let p = dir.join(format!("{stem}.{tag}.{ext}"));
            if std::fs::write(&p, write(t, vtt)).is_ok() {
                out.push(p);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track() -> SubTrack {
        SubTrack {
            lang: "en".into(),
            name: "English".into(),
            kind: "subtitles".into(),
            cues: vec![
                SubCue { start: 1.0, end: 2.5, text: "Good morning.".into(), speaker: Some("MAYA".into()) },
                SubCue { start: 3.0, end: 4.2, text: "Where were you last night?".into(), speaker: Some("RAJ".into()) },
            ],
        }
    }

    #[test]
    fn srt_round_trip_and_supplied_translation_keeps_timing_and_speakers() {
        let t = track();
        let srt = write(&t, false);
        assert!(srt.contains("00:00:01,000 --> 00:00:02,500"), "{srt}");
        let back = read(&srt, "en").unwrap();
        assert_eq!(back.cues.len(), 2);
        assert!((back.cues[1].start - 3.0).abs() < 1e-3);
        let es = translate(&t, "es", Some(vec!["Buenos días.".into(), "¿Dónde estabas anoche?".into()])).unwrap();
        assert_eq!(es.lang, "es");
        assert_eq!(es.name, "Español");
        assert_eq!((es.cues[1].start, es.cues[1].end), (3.0, 4.2));
        assert_eq!(es.cues[1].speaker.as_deref(), Some("RAJ"));
        assert!(translate(&t, "es", Some(vec!["uno".into()])).is_err(), "one text per subtitle");
        assert!(write(&es, true).starts_with("WEBVTT"));
    }

    #[test]
    fn machine_translation_through_a_local_stand_in() {
        // A one-shot local server answering like the Responses API.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            use std::io::{Read, Write};
            let (mut s, _) = listener.accept().unwrap();
            let mut buf = vec![0u8; 65536];
            let mut got = Vec::new();
            loop {
                let n = s.read(&mut buf).unwrap();
                got.extend_from_slice(&buf[..n]);
                let txt = String::from_utf8_lossy(&got);
                if let Some(h) = txt.find("\r\n\r\n") {
                    let len: usize = txt.lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse().unwrap())).unwrap_or(0);
                    if got.len() >= h + 4 + len {
                        break;
                    }
                }
            }
            let req = String::from_utf8_lossy(&got).to_string();
            let body = r#"{"output":[{"type":"message","content":[{"type":"output_text","text":"[\"Hola.\", \"Adiós.\"]"}]}]}"#;
            write!(s, "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}", body.len(), body).unwrap();
            req
        });
        let out = translate_with(&format!("http://127.0.0.1:{port}"), "test-key", &["Hello.".into(), "Goodbye.".into()], "en", "es").unwrap();
        assert_eq!(out, vec!["Hola.", "Adiós."]);
        let req = server.join().unwrap();
        assert!(req.contains("POST /v1/responses") && req.contains("Espa"), "{}", &req[..req.len().min(400)]);
    }
}
