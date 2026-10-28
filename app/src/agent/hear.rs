// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Hearing for `listen`, beyond levels: what a sound is (speech, music,
//! applause, noise, silence, by the half second), what is wrong with it
//! (clipping, clicks, hum, rumble), what a voice is like, how loud it is
//! against delivery targets, and, over the timeline (`mix: true`), how the
//! voice and the music sit together: how far under the voice the music is
//! and where it masks the words. All on this Mac (`studio_ai::features::
//! hearing`).

use super::Session;
use crate::view::SEC;
use serde_json::{Value, json};
use studio_ai::features::hearing::{self, Heard};
use studio_model as m;

fn r1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

fn r2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

/// Loudness against where it is going, in words.
fn loudness_said(lufs: f64, peak: f64) -> String {
    let mut s = if !lufs.is_finite() || lufs < -70.0 {
        "almost silent".to_owned()
    } else {
        let web = lufs - (-14.0);
        let mut t = format!("{lufs:.1} LUFS: ");
        t += &if web.abs() <= 1.5 {
            "right for YouTube, Instagram and TikTok (-14)".to_owned()
        } else if web < 0.0 {
            format!("{:.0} dB quieter than the -14 LUFS social platforms play at (-16 suits podcasts and voice, -23 broadcast)", -web)
        } else {
            format!("{web:.0} dB louder than the -14 LUFS social platforms turn it down to")
        };
        t
    };
    if peak > -1.0 {
        s += &format!("; peaks reach {peak:.1} dBTP, above the -1 dBTP ceiling (it may distort once encoded)");
    }
    s
}

/// The `heard` part of a listen answer for `mono` at `rate`; `at` maps
/// seconds of `mono` to the times reported.
pub fn heard(mono: &[f32], rate: u32, at: &dyn Fn(f64) -> f64) -> Value {
    let h: Heard = hearing::analyse(mono, rate);
    let rep = studio_sound::loudness::measure_mono(mono, rate);
    let mut segs: Vec<Value> = h.segments.iter().map(|s| json!({"from": r2(at(s.from)), "to": r2(at(s.to)), "is": s.label})).collect();
    let more = segs.len().saturating_sub(40);
    segs.truncate(40);
    let mut o = json!({
        "sounds_like": hearing::describe(&h),
        "timeline": segs,
        "loudness": loudness_said(rep.integrated_lufs, rep.true_peak_dbtp),
        "lufs": r1(rep.integrated_lufs),
        "true_peak_db": r1(rep.true_peak_dbtp),
        "range_lu": r1(rep.range_lu),
    });
    if more > 0 {
        o["timeline_more"] = json!(format!("{more} more stretches: listen with from / to for the detail"));
    }
    let mut faults = serde_json::Map::new();
    if !h.clipping.is_empty() {
        faults.insert("clipping".into(), json!(h.clipping.iter().take(10).map(|t| r2(at(*t))).collect::<Vec<_>>()));
    }
    if !h.clicks.is_empty() {
        faults.insert("clicks".into(), json!(h.clicks.iter().take(10).map(|t| r2(at(*t))).collect::<Vec<_>>()));
    }
    if let Some((hz, p)) = h.hum {
        faults.insert("hum".into(), json!({"hz": hz, "db_above": p, "fix": "audio effect eq with a narrow cut at that frequency, or ai voice_cleanup"}));
    }
    if h.rumble_db > 0.0 {
        faults.insert("rumble".into(), json!({"db": h.rumble_db, "fix": "audio effect eq with a high-pass at 80 Hz"}));
    }
    if !faults.is_empty() {
        o["faults"] = Value::Object(faults);
    }
    if let Some((f0, spread)) = h.voice_pitch {
        let mut v = json!({"pitch_hz": f0, "range_semitones": spread});
        if let Some(sr) = h.syllable_rate {
            v["syllables_per_second"] = json!(sr);
        }
        if let Some(room) = h.room {
            v["room"] = json!(room);
        }
        o["voice"] = v;
    }
    o
}

/// A clip's role in the mix, from its lane's name or by ear.
fn role_of(name: &str, heard_speech: impl FnOnce() -> Option<bool>) -> Option<&'static str> {
    let n = name.to_lowercase();
    if ["voice", "narration", "dialogue", "vo", "speech", "interview"].iter().any(|k| n.contains(k)) {
        return Some("voice");
    }
    if ["music", "score", "song", "track", "bed"].iter().any(|k| n.contains(k)) {
        return Some("music");
    }
    if ["sfx", "sound effect", "effects", "foley"].iter().any(|k| n.contains(k)) {
        return None;
    }
    heard_speech().map(|s| if s { "voice" } else { "music" })
}

impl Session {
    /// `listen` with `mix: true`: the timeline's mix from `from` to `to`
    /// as heard, and the voice against the music.
    pub(super) fn listen_mix(&mut self, a: &Value) -> Result<Value, String> {
        let end = self.doc.project.clips.iter().map(|c| c.start + c.dur).max().unwrap_or(0);
        let from = (a.get("from").and_then(Value::as_f64).unwrap_or(0.0).max(0.0) * SEC as f64) as m::Micros;
        let to = a.get("to").and_then(Value::as_f64).map_or(end, |t| (t * SEC as f64) as m::Micros).min(end);
        if to <= from {
            return Err("there is nothing on the timeline there to listen to".into());
        }
        if to - from > 15 * 60 * SEC {
            return Err("listen to at most 15 minutes of the mix at a time (from / to)".into());
        }
        let rate = crate::sound_bridge::RATE;
        let to_mono = |pcm: &[f32]| -> Vec<f32> { pcm.chunks_exact(2).map(|q| 0.5 * (q[0] + q[1])).collect() };
        let whole = to_mono(&crate::sound_bridge::mix_for_export(&self.doc.model, from, to));
        if whole.iter().all(|v| v.abs() < 1e-6) {
            return Err("the timeline is silent there".into());
        }
        let t0 = from as f64 / SEC as f64;
        let mut out = json!({"listened_to": format!("the mix from {:.2}s to {:.2}s (timeline times)", t0, to as f64 / SEC as f64)});
        out["heard"] = heard(&whole, rate, &|t| t0 + t);
        // Who is the voice and who the music: by ear first (how much of
        // each clip is speech, how much music), compared clip with clip; the
        // lane's name only where the ear cannot tell.
        let p = self.doc.project.clone();
        let mut heard_clips: Vec<(u32, String, Option<(f64, f64)>)> = Vec::new();
        for c in p.clips.iter().filter(|c| c.media.is_some() && c.start < to && c.start + c.dur > from) {
            let Some(mi) = c.media.and_then(|m| p.media(m)) else { continue };
            if matches!(mi.kind, crate::view::MediaKind::Image) || c.audio.volume <= -90.0 {
                continue;
            }
            let lane = p.lane(c.lane).map(|l| l.name.clone()).unwrap_or_default();
            let (src_in, dur, speed) = (c.src_in as f64 / SEC as f64, c.dur as f64 / SEC as f64, c.speed as f64 / 100.0);
            let by_ear = mi.path.clone().and_then(|path| {
                let d = crate::sound_bridge::decoded(&path.to_string_lossy())?;
                let (i0, i1) = ((src_in * d.rate as f64) as usize, (((src_in + dur.min(30.0) * speed) * d.rate as f64) as usize).min(d.left.len()));
                if i1 <= i0 + d.rate as usize {
                    return None;
                }
                let mono: Vec<f32> = d.left[i0..i1].iter().zip(&d.right[i0..i1]).map(|(l, r)| 0.5 * (l + r)).collect();
                let h = hearing::analyse(&mono, d.rate);
                let share = |l: &str| h.shares.iter().filter(|s| s.0 == l).map(|s| s.1).sum::<f64>() / h.duration.max(1e-6);
                Some((share("speech") + share("speech over music"), share("music")))
            });
            heard_clips.push((c.id, lane, by_ear));
        }
        let most_talk = heard_clips.iter().filter_map(|c| c.2.map(|e| e.0)).fold(0.0, f64::max);
        let roles: Vec<(u32, &'static str)> = heard_clips
            .iter()
            .filter_map(|(id, lane, ear)| {
                let r = match ear {
                    // Talk, and among the most talkative here: a narration, an
                    // interview, a film's own dialogue over its score.
                    Some((talk, _)) if *talk >= 0.08 && *talk >= most_talk * 0.5 => Some("voice"),
                    Some((talk, music)) if *music >= 0.3 || (*talk < 0.08 && *music > 0.1) => Some("music"),
                    _ => role_of(lane, || None),
                };
                r.map(|r| (*id, r))
            })
            .collect();
        let voice: Vec<u32> = roles.iter().filter(|r| r.1 == "voice").map(|r| r.0).collect();
        let music: Vec<u32> = roles.iter().filter(|r| r.1 == "music").map(|r| r.0).collect();
        out["voice_clips"] = json!(voice);
        out["music_clips"] = json!(music);
        if voice.is_empty() || music.is_empty() {
            out["balance"] = json!(if voice.is_empty() { "no voice under this stretch to balance against" } else { "no music under this stretch to balance against" });
            return Ok(out);
        }
        let stem = |keep: &[u32]| -> Vec<f32> {
            let mut model = self.doc.model.clone();
            for s in &mut model.sequences {
                for c in &mut s.clips {
                    if !keep.contains(&(c.id.0 as u32)) {
                        c.audio.muted = true;
                    }
                }
            }
            to_mono(&crate::sound_bridge::mix_for_export(&model, from, to))
        };
        let (v, mu) = (stem(&voice), stem(&music));
        let b = hearing::balance(&v, &mu, rate);
        let Some(gap) = b.voice_over_music_db else {
            out["balance"] = json!("the voice does not speak in this stretch");
            return Ok(out);
        };
        let verdict = if gap >= 15.0 {
            format!("the music sits {gap:.0} dB under the voice while it speaks: clear, words easy to follow")
        } else if gap >= 9.0 {
            format!("the music sits {gap:.0} dB under the voice: fine for a lively piece; for calm talk, duck it {:.0} dB more", 15.0 - gap)
        } else if gap >= 0.0 {
            format!("the music is only {gap:.0} dB under the voice: it fights the words; duck it about {:.0} dB while they speak (music under narration ducks itself; or animate its volume)", 15.0 - gap)
        } else {
            format!("the music is {:.0} dB louder than the voice: the words are lost; bring it down about {:.0} dB while they speak (music under narration ducks itself; or animate its volume)", -gap, 15.0 - gap)
        };
        out["balance"] = json!(verdict);
        out["voice_over_music_db"] = json!(gap);
        if !b.masked.is_empty() {
            out["masked"] = json!(b.masked.iter().take(12).map(|(a2, b2, band)| json!({"from": r2(t0 + a2), "to": r2(t0 + b2), "band": band})).collect::<Vec<_>>());
            out["masked_share"] = json!(r2(b.masked_share));
            out["masking"] = json!(format!(
                "the music covers the voice's consonants ({:.0}% of the speaking time): duck it there, or cut 2-4 dB around 2-3 kHz in the music with an eq so the words come through",
                b.masked_share * 100.0
            ));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_come_from_lane_names_first() {
        assert_eq!(role_of("Narration", || None), Some("voice"));
        assert_eq!(role_of("Music", || Some(true)), Some("music"));
        assert_eq!(role_of("SFX", || Some(true)), None);
        assert_eq!(role_of("A1", || Some(true)), Some("voice"));
        assert_eq!(role_of("A1", || None), None);
        assert!(loudness_said(-14.3, -2.0).contains("right for"));
        assert!(loudness_said(-20.0, -0.2).contains("6 dB quieter") && loudness_said(-20.0, -0.2).contains("ceiling"));
    }
}

#[cfg(test)]
mod listen_tests {
    use crate::agent::{Content, Session};
    use crate::services;
    use serde_json::{Value, json};

    fn wav(path: &std::path::Path, rate: u32, x: &[f32]) {
        let n = x.len() as u32;
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + n * 2).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        for v in [16u32.to_le_bytes().to_vec(), 1u16.to_le_bytes().to_vec(), 1u16.to_le_bytes().to_vec(), rate.to_le_bytes().to_vec(), (rate * 2).to_le_bytes().to_vec(), 2u16.to_le_bytes().to_vec(), 16u16.to_le_bytes().to_vec()] {
            out.extend_from_slice(&v);
        }
        out.extend_from_slice(b"data");
        out.extend_from_slice(&(n * 2).to_le_bytes());
        for &v in x {
            out.extend_from_slice(&((v.clamp(-1.0, 1.0) * 32000.0) as i16).to_le_bytes());
        }
        std::fs::write(path, out).unwrap();
    }

    /// Speech from the Mac's `say` (None where it cannot run).
    fn speech(dir: &std::path::Path) -> Option<std::path::PathBuf> {
        let p = dir.join("talk.wav");
        let ok = std::process::Command::new("say").args(["-v", "Samantha", "-o"]).arg(&p).args(["--data-format=LEI16@44100", "Hello and welcome back. Today we look at how to cut a short film so that every scene lands. First find the moment the story turns, then work backwards from it, and keep only what serves it."]).status().ok()?.success();
        ok.then_some(p)
    }

    /// A bright music bed: chords, a buzzy lead and a kick, 12 s.
    fn music(dir: &std::path::Path) -> std::path::PathBuf {
        let r = 44_100u32;
        let x: Vec<f32> = (0..r * 12)
            .map(|i| {
                let t = i as f64 / r as f64;
                let chord = [220.0, 277.2, 329.6, 440.0];
                let tone: f64 = chord.iter().map(|f| (2.0 * std::f64::consts::PI * f * t).sin()).sum::<f64>() * 0.08;
                let lead: f64 = (1..16).map(|k| (2.0 * std::f64::consts::PI * 293.7 * k as f64 * t).sin() / k as f64).sum::<f64>() * 0.08;
                let b = t % 0.5;
                let kick = (-b * 30.0).exp() * (2.0 * std::f64::consts::PI * 60.0 * b).sin() * 0.3;
                (tone + lead + kick) as f32
            })
            .collect();
        let p = dir.join("bed.wav");
        wav(&p, r, &x);
        p
    }

    fn text(v: Vec<Content>) -> Value {
        v.iter().find_map(|c| if let Content::Text(t) = c { serde_json::from_str(t).ok() } else { None }).unwrap()
    }

    /// `listen` on a clip hears the clip as it plays: its volume, its EQ
    /// and its effects, not the file underneath. `from` / `to` are timeline
    /// seconds, as the answer's times are.
    #[test]
    fn listen_to_a_clip_hears_its_volume_and_effects() {
        let dir = std::env::temp_dir().join(format!("studio-hear-fx-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let bed = music(&dir);
        let mut s = Session::new(services::project::new_custom("Hear", 640, 360, 30.0));
        let v = text(s.call("import", &json!({"paths": [bed.to_string_lossy()]})).unwrap());
        let media = v["media"][0].as_u64().unwrap();
        let r = s.edit(&[json!({"op": "place", "media": media, "at": 2.0})], "place", false).unwrap();
        let id = r["changed"]["added"][0]["id"].as_u64().unwrap();
        let raw = s.listen(&json!({"clip": id})).unwrap();
        // 20 dB down and the top cut away at 300 Hz.
        s.edit(&[json!({"op": "set", "clip": id, "volume": -20})], "down", false).unwrap();
        s.edit(&[json!({"op": "audio", "clip": id, "effect": "eq", "params": {"low_pass": 300}})], "dull", false).unwrap();
        let done = s.listen(&json!({"clip": id})).unwrap();
        let (a, b) = (raw["average_db"].as_f64().unwrap(), done["average_db"].as_f64().unwrap());
        assert!(a - b > 20.0, "average {a} dB → {b} dB");
        assert_eq!(done["brightness"], "dark, bass-heavy", "{done}");
        assert!(done["listened_to"].as_str().unwrap().contains("as it plays"), "{done}");
        // Timeline seconds: 4..6 s of the timeline is 2..4 s into the clip.
        let part = s.listen(&json!({"clip": id, "from": 4.0, "to": 6.0})).unwrap();
        assert!((part["duration"].as_f64().unwrap() - 2.0).abs() < 0.02, "{part}");
        assert!(part["loudest_at"].as_f64().is_some_and(|t| (4.0..=6.0).contains(&t)), "{part}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn listen_hears_speech_and_the_mix_balance() {
        let dir = std::env::temp_dir().join(format!("studio-hear-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let Some(talk) = speech(&dir) else {
            eprintln!("say is not available: skipped");
            return;
        };
        let bed = music(&dir);
        let mut s = Session::new(services::project::new_custom("Hear", 640, 360, 30.0));
        let v = text(s.call("import", &json!({"paths": [talk.to_string_lossy(), bed.to_string_lossy()]})).unwrap());
        let ids: Vec<u64> = v["media"].as_array().unwrap().iter().map(|m| m.as_u64().unwrap()).collect();
        // The voice alone, by ear.
        let h = s.listen(&json!({"media": ids[0]})).unwrap();
        let said = h["heard"]["sounds_like"].as_str().unwrap();
        assert!(said.contains("speech") && said.contains("Speech:"), "{said}");
        assert!(h["heard"]["lufs"].as_f64().unwrap() < -5.0);
        // The bed alone is music.
        let m = s.listen(&json!({"media": ids[1]})).unwrap();
        assert!(m["heard"]["sounds_like"].as_str().unwrap().contains("music"), "{}", m["heard"]);
        // Both on the timeline, the music as loud as the voice.
        s.edit(&[json!({"op": "place", "media": ids[0], "at": 0}), json!({"op": "place", "media": ids[1], "at": 0, "duration": 10})], "place", false).unwrap();
        let mix = s.listen(&json!({"mix": true, "from": 0, "to": 10})).unwrap();
        assert_eq!(mix["voice_clips"].as_array().unwrap().len(), 1, "{mix}");
        assert_eq!(mix["music_clips"].as_array().unwrap().len(), 1, "{mix}");
        let loud_gap = mix["voice_over_music_db"].as_f64().unwrap();
        assert!(mix["balance"].as_str().unwrap().contains("fights") || loud_gap < 15.0, "{mix}");
        // Music down 24 dB: clear.
        let bed_clip = mix["music_clips"][0].as_u64().unwrap();
        s.edit(&[json!({"op": "set", "clip": bed_clip, "volume": -24})], "duck", false).unwrap();
        let mix = s.listen(&json!({"mix": true, "from": 0, "to": 10})).unwrap();
        let gap = mix["voice_over_music_db"].as_f64().unwrap();
        assert!(gap > loud_gap + 15.0 && mix["balance"].as_str().unwrap().contains("clear"), "{loud_gap} → {mix}");
        let _ = std::fs::remove_dir_all(dir);
    }
}
