// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! ADR (automated dialogue replacement): the cue list, the cue sheet the
//! recording stage works from (timecode in and out, character, line,
//! reason), the three-beep guide an actor speaks to, and bringing a take
//! in: lined up with the production line (one shift, or word by word with
//! `warp`), matched to the production voice, laid on the ADR lane, with the
//! production line dipped under it when it replaces it.

use super::{AdrCue, Desk, LaneMix};
use crate::doc::Doc;
use serde_json::json;
use studio_model as m;
use studio_sound::align;

/// SMPTE timecode `HH:MM:SS:FF` (non-drop) of a timeline time.
pub fn timecode(t: m::Micros, fps: f32) -> String {
    let fps_i = fps.round().max(1.0) as i64;
    // The frame the time falls in.
    let frames = (t as f64 * fps as f64 / 1e6 + 1e-6).floor() as i64;
    let (f, s) = (frames % fps_i, frames / fps_i);
    format!("{:02}:{:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60, f)
}

/// The cue sheet as CSV (one line per cue, quoted where needed).
pub fn cue_sheet(desk: &Desk, fps: f32, title: &str) -> String {
    let q = |s: &str| if s.contains([',', '"', '\n']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s.to_owned() };
    let mut out = format!("# ADR cue sheet: {}\nCue,Character,In,Out,Duration (s),Line,Reason,Status,Notes\n", q(title));
    let mut cues: Vec<&AdrCue> = desk.adr.iter().collect();
    cues.sort_by_key(|c| (c.start, c.id));
    for c in cues {
        out.push_str(&format!(
            "{},{},{},{},{:.2},{},{},{},{}\n",
            c.id,
            q(&c.character),
            timecode(c.start, fps),
            timecode(c.end, fps),
            (c.end - c.start) as f64 / 1e6,
            q(&c.text),
            q(&c.reason),
            q(&c.status),
            q(&c.notes)
        ));
    }
    out
}

/// The guide track for recording a cue: three 1 kHz beeps a second apart
/// ending one second before the line (the silent fourth beep), each one
/// frame long, over `preroll` seconds of lead-in. Mono at `rate`.
pub fn beeps(rate: u32, preroll: f32, fps: f32) -> Vec<f32> {
    let n = (preroll.max(3.0) * rate as f32) as usize;
    let mut x = vec![0.0f32; n];
    let len = (rate as f32 / fps.max(1.0)) as usize;
    for k in 1..=3 {
        let at = n - k * rate as usize;
        for i in 0..len {
            let env = (std::f32::consts::PI * i as f32 / len as f32).sin().min(1.0);
            x[at + i] = 0.5 * env * (std::f32::consts::TAU * 1000.0 * i as f32 / rate as f32).sin();
        }
    }
    x
}

/// Adds a cue (or replaces the one with the same id). Returns its id.
pub fn add_cue(desk: &mut Desk, mut cue: AdrCue) -> u32 {
    if cue.id == 0 {
        cue.id = desk.adr.iter().map(|c| c.id).max().unwrap_or(0) + 1;
    }
    if cue.status.is_empty() {
        cue.status = "to do".into();
    }
    let id = cue.id;
    match desk.adr.iter_mut().find(|c| c.id == id) {
        Some(c) => *c = cue,
        None => desk.adr.push(cue),
    }
    desk.adr.sort_by_key(|c| (c.start, c.id));
    id
}

/// Cues from a subtitle track: every line in `[from, to)` (seconds), or of
/// the given characters.
pub fn cues_from_subtitles(desk: &Desk, lang: &str, from: f32, to: f32, characters: &[String], reason: &str) -> Vec<AdrCue> {
    let Some(t) = desk.subtitles_of(lang) else { return Vec::new() };
    t.cues
        .iter()
        .filter(|c| c.end > from && c.start < to)
        .filter(|c| characters.is_empty() || c.speaker.as_ref().is_some_and(|s| characters.iter().any(|x| x.eq_ignore_ascii_case(s))))
        .map(|c| AdrCue { character: c.speaker.clone().unwrap_or_default(), start: (c.start as f64 * 1e6) as i64, end: (c.end as f64 * 1e6) as i64, text: c.text.replace('\n', " "), reason: reason.to_owned(), ..Default::default() })
        .collect()
}

/// How a take came in.
#[derive(Clone, Debug)]
pub struct TakeReport {
    pub clip: u32,
    /// Where it sits (seconds).
    pub start: f64,
    /// The shift found against the production line, seconds.
    pub shift: f64,
    /// How clearly it matched (0..1).
    pub confidence: f32,
    pub warped: bool,
    pub matched: Option<studio_sound::dialogue::VoiceMatch>,
    pub replaced: bool,
}

/// Mono samples of a sound file at the mixing rate.
pub fn read_mono(path: &std::path::Path) -> Result<Vec<f32>, String> {
    let d = crate::sound_bridge::decode_uncached(path).ok_or_else(|| format!("{}: no sound Studio can read", path.display()))?;
    let mono: Vec<f32> = d.left.iter().zip(d.right.iter()).map(|(l, r)| 0.5 * (l + r)).collect();
    let rate = crate::sound_bridge::RATE;
    Ok(if d.rate == rate { mono } else { studio_sound::resample::resample(&mono, &mono, d.rate, rate, studio_sound::Quality::Best).0 })
}

/// Brings a recorded take of cue `id` in from `path`.
pub fn take(doc: &mut Doc, home: Option<&std::path::Path>, id: u32, path: &std::path::Path, warp: bool, voice_match: bool, replace: bool) -> Result<TakeReport, String> {
    let rate = crate::sound_bridge::RATE;
    let desk = Desk::of(&doc.model);
    let cue = desk.adr.iter().find(|c| c.id == id).cloned().ok_or_else(|| format!("no ADR cue {id}"))?;
    let take = read_mono(path)?;
    if take.len() < rate as usize / 10 {
        return Err("the take is too short".into());
    }
    // The production line under the cue, with a second either side.
    let lead = 1_000_000i64;
    let g0 = (cue.start - lead).max(0);
    let g1 = cue.end + lead;
    let stereo = studio_sound::surround::Layout::Stereo;
    let req = super::mix::MixRequest { layout: Some(stereo), stem: Some(vec![studio_sound::BusKind::Dialogue]), normalise: false, limiter: false, ..Default::default() };
    // Leave earlier ADR out of the guide: it is the production line we follow.
    let mut guide_project = doc.model.clone();
    if let Some(seq) = guide_project.sequences.iter_mut().find(|s| s.id == guide_project.active_sequence) {
        for l in &mut seq.lanes {
            if LaneMix::of(l).adr {
                l.muted = true;
            }
        }
    }
    let g = super::render::render(&guide_project, &req, g0, g1);
    let guide: Vec<f32> = g[0].iter().zip(&g[1]).map(|(a, b)| 0.5 * (a + b)).collect();
    let has_guide = guide.iter().any(|v| v.abs() > 1e-4);
    let (mut audio, mut start, shift, conf) = if has_guide {
        let max = (take.len() as f32 / rate as f32).max(2.0) + 1.0;
        let (shift, conf) = align::offset(&guide, &take, rate, max);
        // Take frame j plays at guide frame j + shift.
        (take.clone(), g0 as f64 / 1e6 + shift as f64 / rate as f64, shift as f64 / rate as f64, conf)
    } else {
        (take.clone(), cue.start as f64 / 1e6, 0.0, 0.0)
    };
    let mut warped = false;
    if warp && has_guide {
        // Speech of each, then a word-by-word map from the take onto the guide.
        let speech = |x: &[f32]| studio_sound::vad::speech_segments(x, rate, studio_sound::vad::VadParams::default());
        let (gs, ts) = (speech(&guide), speech(&take));
        if let (Some(gf), Some(gl), Some(tf), Some(tl)) = (gs.first(), gs.last(), ts.first(), ts.last()) {
            let pad = 150_000;
            let fr = |t: i64, n: usize| (studio_sound::micros_to_frames(t.max(0), rate) as usize).min(n);
            let (ga, gb) = (fr(gf.0 - pad, guide.len()), fr(gl.1 + pad, guide.len()));
            let (ta, tb) = (fr(tf.0 - pad, take.len()), fr(tl.1 + pad, take.len()));
            if gb > ga && tb > ta {
                let map = align::warp_map(&guide[ga..gb], &take[ta..tb], rate, 0.8, 0.08);
                audio = align::conform(&take[ta..tb], rate, &map, 2.0);
                start = g0 as f64 / 1e6 + ga as f64 / rate as f64;
                warped = true;
            }
        }
    }
    if start < 0.0 {
        let cut = ((-start) * rate as f64) as usize;
        audio.drain(..cut.min(audio.len()));
        start = 0.0;
    }
    let dir = home.map(|h| h.join("ADR")).unwrap_or_else(|| studio_paths::cache().join("adr"));
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let wav = studio_media::wav::Wav { sample_rate: rate, channels: 2, samples: audio.iter().flat_map(|s| [*s, *s]).collect() };
    let file = crate::files::write_once(&dir, &format!("cue{id:03}"), "wav", &studio_media::wav::encode_pcm16(&wav)).map_err(|e| format!("could not write the take: {e}"))?;
    let dur = (audio.len() as f64 / rate as f64 * 1e6) as i64;
    doc.begin_batch(&format!("ADR take, cue {id}"));
    let r = place(doc, &cue, &file, (start * 1e6) as i64, dur, replace);
    let clip = match r {
        Ok(c) => {
            doc.commit_batch();
            c
        }
        Err(e) => {
            doc.rollback_batch();
            return Err(e);
        }
    };
    let matched = if voice_match && has_guide {
        // Match the take to the production voice under it.
        let pseudo_ref = guide;
        let mut vm = studio_sound::dialogue::match_voice(&studio_sound::dialogue::profile(&pseudo_ref, rate), &studio_sound::dialogue::profile(&audio, rate));
        let (seq, mut c) = super::dialogue::find_clip(doc, clip as u64).ok_or("the take vanished")?;
        vm.gain_db = vm.gain_db.clamp(-18.0, 18.0);
        super::dialogue::apply_match(&mut c, &vm);
        if !doc.apply("Match ADR to production", m::Edit::SetClip { seq, clip: Box::new(c) }) {
            return Err(doc.error.take().unwrap_or_else(|| "matching was refused".into()));
        }
        Some(vm)
    } else {
        None
    };
    super::edit_desk(doc, "ADR take", |d| {
        if let Some(c) = d.adr.iter_mut().find(|c| c.id == id) {
            c.status = "recorded".into();
            c.take = Some(clip as u64);
        }
        Ok(String::new())
    })?;
    Ok(TakeReport { clip, start, shift, confidence: conf, warped, matched, replaced: replace })
}

fn place(doc: &mut Doc, cue: &AdrCue, file: &std::path::Path, start: i64, dur: i64, replace: bool) -> Result<u32, String> {
    use crate::view::ClipKind;
    let seq = doc.seq();
    // The ADR lane, made once.
    let lane = match doc.model.sequence(seq).and_then(|s| s.lanes.iter().find(|l| LaneMix::of(l).adr)).map(|l| l.id.0) {
        Some(id) => id,
        None => {
            let id = doc.alloc_id();
            let index = doc.model.sequence(seq).map(|s| s.lanes.len());
            let mut l = m::Lane::new(m::LaneId(id), m::LaneKind::Audio, "ADR");
            LaneMix { adr: true, role: Some("dialogue".into()), ..Default::default() }.store(&mut l);
            if !doc.apply("Add ADR lane", m::Edit::AddLane { seq, lane: l, index }) {
                return Err(doc.error.take().unwrap_or_else(|| "the ADR lane was refused".into()));
            }
            id
        }
    };
    let report = crate::services::project::import_media(doc, &[file.to_path_buf()]);
    let media = *report.ids.first().ok_or_else(|| format!("the take could not be imported: {}", report.failed.join("; ")))?;
    let mut c = crate::services::library::new_clip(start, dur, ClipKind::Audio, &format!("ADR {} {}", cue.id, cue.character), Some(media));
    c.lane = lane as u32;
    c.id = doc.alloc_id() as u32;
    let id = c.id;
    let lane32 = lane as u32;
    let prefix = format!("ADR {} ", cue.id);
    if !doc.edit("ADR take", |p| {
        // A new take of the cue replaces the last one on the lane.
        p.clips.retain(|x| !(x.lane == lane32 && x.name.starts_with(&prefix)));
        p.clips.push(c);
    }) {
        return Err(doc.error.take().unwrap_or_else(|| "the take was refused".into()));
    }
    if let Some((seq, mut mc)) = super::dialogue::find_clip(doc, id as u64) {
        mc.meta.insert("adr".into(), json!({"cue": cue.id, "text": cue.text, "character": cue.character}));
        doc.apply("ADR note", m::Edit::SetClip { seq, clip: Box::new(mc) });
    }
    if replace {
        dip_production(doc, cue.start, cue.end)?;
    }
    Ok(id)
}

/// Dips the production dialogue lanes (not ADR, not dubs) to silence over
/// `[a, b)` with 20 ms ramps, in their fader automation.
pub fn dip_production(doc: &mut Doc, a: i64, b: i64) -> Result<usize, String> {
    let seq = doc.seq();
    let lanes: Vec<m::Lane> = doc.model.sequence(seq).map(|s| s.lanes.clone()).unwrap_or_default();
    let mut n = 0;
    for lane in lanes {
        let mix = LaneMix::of(&lane);
        if super::lane_role(&lane) != studio_sound::mixer::LaneRole::Dialogue || mix.adr || mix.dub.is_some() {
            continue;
        }
        super::edit_lane(doc, lane.id.0, "Dip production line", |mix, l| {
            let base = super::mix::curve(&mix.volume_keys, l.volume_db as f32);
            let ramp = 20_000;
            let (va, vb) = (base.eval(a - ramp), base.eval(b + ramp));
            mix.volume_keys.retain(|(t, _)| *t < a - ramp || *t > b + ramp);
            mix.volume_keys.extend([(a - ramp, va), (a, -90.0), (b, -90.0), (b + ramp, vb)]);
            mix.volume_keys.sort_by_key(|k| k.0);
            Ok(String::new())
        })?;
        n += 1;
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timecode_and_cue_sheet() {
        assert_eq!(timecode(3_723_500_000, 25.0), "01:02:03:12");
        let mut d = Desk::default();
        add_cue(&mut d, AdrCue { character: "MAYA".into(), start: 12_000_000, end: 14_500_000, text: "Wait, \"stop\"!".into(), reason: "noise".into(), ..Default::default() });
        let id = add_cue(&mut d, AdrCue { character: "RAJ".into(), start: 2_000_000, end: 3_000_000, text: "Hello".into(), ..Default::default() });
        assert_eq!(id, 2);
        let csv = cue_sheet(&d, 24.0, "Reel 1");
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines[1], "Cue,Character,In,Out,Duration (s),Line,Reason,Status,Notes");
        assert!(lines[2].starts_with("2,RAJ,00:00:02:00,00:00:03:00,1.00,Hello,,to do,"), "{}", lines[2]);
        assert!(lines[3].contains("\"Wait, \"\"stop\"\"!\""), "{}", lines[3]);
    }

    #[test]
    fn three_beeps_a_second_apart_before_the_line() {
        let rate = 48_000;
        let b = beeps(rate, 4.0, 24.0);
        assert_eq!(b.len(), 4 * rate as usize);
        let on: Vec<usize> = (0..4).filter(|s| b[s * rate as usize..(s + 1) * rate as usize].iter().any(|v| v.abs() > 0.1)).collect();
        assert_eq!(on, vec![1, 2, 3], "beeps in seconds 1, 2, 3; the line starts at 4");
    }
}
