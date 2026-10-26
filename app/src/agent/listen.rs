// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `listen`: hearing for an agent. A song, a sound effect or a clip's
//! sound described as what an editor hears: how loud and where, where it
//! builds, drops and breaks, the beat, and for short sounds their shape
//! (a hard hit, a soft swell, a long tail), with an energy line to read.

use super::Session;
use crate::view::{self, SEC};
use serde_json::{Value, json};
use studio_ai::features::beats;

fn db(x: f64) -> f64 {
    20.0 * x.max(1e-6).log10()
}

fn r(v: f64, p: i32) -> f64 {
    let k = 10f64.powi(p);
    (v * k).round() / k
}

/// RMS level (dB) of `mono` in windows of `win` samples.
fn levels(mono: &[f32], win: usize) -> Vec<f64> {
    mono.chunks(win.max(1)).map(|c| db((c.iter().map(|&x| (x as f64) * (x as f64)).sum::<f64>() / c.len().max(1) as f64).sqrt())).collect()
}

/// A line of block characters, one per point, quiet to loud.
fn sparkline(v: &[f64]) -> String {
    const B: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let (lo, hi) = (v.iter().cloned().fold(f64::INFINITY, f64::min).max(-60.0), v.iter().cloned().fold(f64::NEG_INFINITY, f64::max));
    v.iter().map(|&x| B[(((x.max(lo) - lo) / (hi - lo).max(1e-6)) * 7.0).round().clamp(0.0, 7.0) as usize]).collect()
}

/// What a short sound is, by its shape: "hit" (short and hard), "boom"
/// (hard with a long tail), "swell" (builds slowly: risers), "swoosh"
/// (a soft start, quick or sustained: whooshes, swishes).
pub fn sound_kind(mono: &[f32], rate: u32) -> &'static str {
    let lv = levels(mono, (rate as f64 * 0.01) as usize);
    let (pk_i, pk) = lv.iter().cloned().enumerate().fold((0, f64::NEG_INFINITY), |a, (i, v)| if v > a.1 { (i, v) } else { a });
    let start_i = lv.iter().position(|&v| v > pk - 30.0).unwrap_or(0);
    let attack = (pk_i - start_i) as f64 * 0.01;
    let tail = lv.iter().rposition(|&v| v > pk - 30.0).unwrap_or(pk_i).saturating_sub(pk_i) as f64 * 0.01;
    let dur = mono.len() as f64 / rate.max(1) as f64;
    let zc = mono.windows(2).filter(|w| (w[0] >= 0.0) != (w[1] >= 0.0)).count() as f64 / dur.max(1e-3);
    let dark = zc < 800.0;
    // Measured on the library: impacts peak within 10 ms, swishes take
    // 30-40 ms, booms are dark with a long tail, risers take most of
    // their length to arrive.
    if attack > 0.4 {
        "swell"
    } else if attack < 0.012 {
        if tail < 0.25 { "hit" } else { "boom" }
    } else if dark && tail > 0.35 {
        "boom"
    } else {
        "swoosh"
    }
}

/// Describes `mono` (at `rate`) as heard; `offset` / `pace` map file
/// seconds to the times reported (timeline seconds for a clip).
pub fn describe(mono: &[f32], rate: u32, stereo_width: Option<f64>, offset: f64, pace: f64) -> Value {
    let rate_f = rate as f64;
    let dur = mono.len() as f64 / rate_f;
    let at = |file_secs: f64| r(offset + file_secs / pace, 3);
    let peak = mono.iter().fold(0f32, |m, &x| m.max(x.abs())) as f64;
    let rms = (mono.iter().map(|&x| (x as f64) * (x as f64)).sum::<f64>() / mono.len().max(1) as f64).sqrt();
    // Brightness: zero crossings per second (hiss and cymbals high, bass low).
    let zc = mono.windows(2).filter(|w| (w[0] >= 0.0) != (w[1] >= 0.0)).count() as f64 / dur.max(1e-3);
    let brightness = if zc < 800.0 { "dark, bass-heavy" } else if zc < 2500.0 { "warm, mid-range" } else if zc < 6000.0 { "bright" } else { "very bright, hissy" };
    let mut out = json!({
        "duration": r(dur, 3),
        "peak_db": r(db(peak), 1),
        "average_db": r(db(rms), 1),
        "brightness": brightness,
    });
    if let Some(w) = stereo_width {
        out["stereo"] = json!(if w < 0.05 { "mono" } else if w < 0.3 { "narrow stereo" } else { "wide stereo" });
    }
    if dur < 8.0 {
        // A sound effect: its shape.
        let win = (rate_f * 0.01) as usize; // 10 ms
        let lv = levels(mono, win);
        let (pk_i, pk) = lv.iter().cloned().enumerate().fold((0, f64::NEG_INFINITY), |a, (i, v)| if v > a.1 { (i, v) } else { a });
        let start_i = lv.iter().position(|&v| v > pk - 30.0).unwrap_or(0);
        let attack = (pk_i - start_i) as f64 * 0.01;
        let tail_i = lv.iter().rposition(|&v| v > pk - 30.0).unwrap_or(pk_i);
        let tail = (tail_i.saturating_sub(pk_i)) as f64 * 0.01;
        let kind = sound_kind(mono, rate);
        let shape = match kind {
            "hit" => "a short, hard hit (punch, click, knock)",
            "boom" => "a hard hit with a long tail (boom, explosion, impact with reverb)",
            "swell" => "a slow swell that builds (riser)",
            _ if tail > 1.0 => "a sound with a soft start that sustains (drone, long swoosh)",
            _ => "a quick sound with a soft start (whoosh, swish)",
        };
        out["shape"] = json!(shape);
        out["kind"] = json!(kind);
        out["attack"] = json!(r(attack, 3));
        out["tail"] = json!(r(tail, 2));
        out["loudest_at"] = json!(at(pk_i as f64 * 0.01));
        out["sounds_from"] = json!(at(start_i as f64 * 0.01));
        let step = (lv.len() / 48).max(1);
        let line: Vec<f64> = lv.chunks(step).map(|c| c.iter().cloned().fold(f64::NEG_INFINITY, f64::max)).collect();
        out["energy"] = json!(sparkline(&line));
        return out;
    }
    // A song or a long sound: where it is loud, where it builds and drops.
    let half = (rate_f * 0.5) as usize;
    let lv = levels(mono, half); // one value per half second
    let smooth = |i: usize, n: usize| {
        let a = i.saturating_sub(n / 2);
        let b = (i + n / 2 + 1).min(lv.len());
        lv[a..b].iter().sum::<f64>() / (b - a).max(1) as f64
    };
    let sm: Vec<f64> = (0..lv.len()).map(|i| smooth(i, 4)).collect(); // ~2 s
    let lo = sm.iter().cloned().fold(f64::INFINITY, f64::min).max(-60.0);
    let hi = sm.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let _level = |v: f64| {
        let x = (v - lo) / (hi - lo).max(1e-6);
        if x < 0.4 { "quiet" } else if x < 0.75 { "medium" } else { "loud" }
    };
    // Drops: the level jumps up by 6 dB or more against the 4 s before;
    // breaks: falls by as much.
    let mut drops = Vec::new();
    let mut breaks = Vec::new();
    let mut cuts: Vec<usize> = vec![0];
    let mut last_event = -100i64;
    for i in 8..lv.len().saturating_sub(2) {
        let before = lv[i - 8..i].iter().sum::<f64>() / 8.0;
        let after = lv[i..(i + 4).min(lv.len())].iter().sum::<f64>() / 4.0;
        if (i as i64) - last_event < 8 {
            continue;
        }
        if after - before >= 6.0 {
            drops.push(json!({"at": at(i as f64 * 0.5), "rise_db": r(after - before, 1)}));
            last_event = i as i64;
            cuts.push(i);
        } else if before - after >= 6.0 {
            breaks.push(json!({"at": at(i as f64 * 0.5), "fall_db": r(before - after, 1)}));
            last_event = i as i64;
            cuts.push(i);
        }
    }
    cuts.push(lv.len());
    // Sections between those moments, each marked against the song's
    // usual level.
    let mut sorted = lv.clone();
    sorted.sort_by(f64::total_cmp);
    let usual = sorted[sorted.len() / 2];
    let sections: Vec<Value> = cuts
        .windows(2)
        .filter(|w| w[1] > w[0])
        .map(|w| {
            let avg = lv[w[0]..w[1]].iter().sum::<f64>() / (w[1] - w[0]) as f64;
            let feel = if avg >= usual + 1.5 { "peak" } else if avg <= usual - 6.0 { "quiet" } else if avg <= usual - 2.5 { "low" } else { "main" };
            json!({"from": at(w[0] as f64 * 0.5), "to": at(w[1] as f64 * 0.5), "feel": feel, "average_db": r(avg, 1)})
        })
        .collect();
    let _ = (smooth, lo, hi);
    let inter: Vec<f32> = mono.iter().flat_map(|&x| [x, x]).collect();
    let bt = beats::beat_times(&inter, rate, 2);
    let bpm = beats::tempo(&bt);
    let step = (sm.len() / 64).max(1);
    let line: Vec<f64> = sm.iter().step_by(step).cloned().collect();
    out["energy"] = json!(sparkline(&line));
    out["energy_step"] = json!(r(step as f64 * 0.5 / pace, 2));
    out["sections"] = json!(sections);
    out["drops"] = json!(drops);
    out["breaks"] = json!(breaks);
    out["bpm"] = json!(bpm.map(|b| r(b, 1)));
    out["beats"] = json!(bt.iter().map(|&t| at(t)).collect::<Vec<_>>());
    out["bar"] = json!(bpm.map(|b| r(4.0 * 60.0 / b / pace, 3)));
    out
}

impl Session {
    /// `listen` to a media file, a clip (times on the timeline) or a
    /// library sound by name.
    pub(super) fn listen(&mut self, a: &Value) -> Result<Value, String> {
        if a.get("mix").and_then(Value::as_bool) == Some(true) {
            return self.listen_mix(a);
        }
        let p = self.doc.project.clone();
        // A clip with no file of its own (merged clips, nested sequences):
        // what it plays is the mix under it.
        if let Some(c) = a.get("clip").and_then(super::craft::as_id).and_then(|id| p.clip(id as u32)) {
            if c.media.is_none() {
                // `from` / `to` are timeline seconds, kept inside the clip.
                let sec = |v: f64| (v * SEC as f64) as view::Time;
                let t0 = a.get("from").and_then(Value::as_f64).map_or(c.start, |v| sec(v).clamp(c.start, c.end()));
                let t1 = a.get("to").and_then(Value::as_f64).map_or(c.end(), |v| sec(v).clamp(t0, c.end()));
                if t1 <= t0 {
                    return Err(format!("clip {} has nothing between {:.2}s and {:.2}s", c.id, t0 as f64 / SEC as f64, t1 as f64 / SEC as f64));
                }
                let pcm = crate::sound_bridge::mix_for_export(&self.doc.model, t0, t1);
                let (l, r): (Vec<f32>, Vec<f32>) = pcm.chunks_exact(2).map(|q| (q[0], q[1])).unzip();
                if l.iter().all(|v| v.abs() < 1e-6) {
                    return Err(format!("clip {} makes no sound (nothing audible under it)", c.id));
                }
                let mono: Vec<f32> = l.iter().zip(&r).map(|(a, b)| 0.5 * (a + b)).collect();
                let side: f64 = l.iter().zip(&r).map(|(a, b)| ((a - b) as f64).powi(2)).sum();
                let mid: f64 = mono.iter().map(|&m| (m as f64).powi(2) * 4.0).sum();
                let mut out = describe(&mono, crate::sound_bridge::RATE, Some((side / mid.max(1e-9)).sqrt()), t0 as f64 / SEC as f64, 1.0);
                out["heard"] = super::hear::heard(&mono, crate::sound_bridge::RATE, &|t| t0 as f64 / SEC as f64 + t);
                out["listened_to"] = json!(format!("clip {}: the mix it plays (times on the timeline)", c.id));
                return Ok(out);
            }
        }
        let (path, from, to, offset, pace, what) = if let Some(id) = a.get("clip").and_then(super::craft::as_id) {
            let c = p.clip(id as u32).ok_or_else(|| format!("no clip {id}"))?;
            let mi = c.media.and_then(|m| p.media(m)).ok_or("that clip has no media to listen to")?;
            let speed = c.speed as f64 / 100.0;
            let src_in = c.src_in as f64 / SEC as f64;
            (mi.path.clone().ok_or("that media has no file")?, src_in, src_in + c.dur as f64 / SEC as f64 * speed, c.start as f64 / SEC as f64 - src_in / speed, speed, format!("clip {id} (times on the timeline)"))
        } else if let Some(mv) = a.get("media") {
            let mi = self.media_item(mv)?;
            (mi.path.clone().ok_or("that media has no file")?, 0.0, f64::INFINITY, 0.0, 1.0, format!("{} (times in the file)", mi.name))
        } else if let Some(name) = a.get("item").and_then(Value::as_str) {
            let (panel, index, found) = self.resolve(name, &[crate::services::library::Panel::Audio])?;
            let item = crate::services::library::item(&self.doc, panel, index);
            (item.path.ok_or("that library sound has no file")?, 0.0, f64::INFINITY, 0.0, 1.0, format!("library sound \"{found}\""))
        } else {
            return Err("listen needs \"media\", \"clip\" or \"item\" (a library sound)".into());
        };
        let from = a.get("from").and_then(Value::as_f64).unwrap_or(from).max(0.0);
        let to = a.get("to").and_then(Value::as_f64).unwrap_or(to);
        let d = crate::sound_bridge::decoded(&path.to_string_lossy()).ok_or("that file has no sound Studio can decode")?;
        let rate = d.rate as f64;
        let (i0, i1) = ((from * rate) as usize, ((to * rate) as usize).min(d.left.len()));
        if i1 <= i0 {
            return Err("that range has no sound".into());
        }
        let mono: Vec<f32> = d.left[i0..i1].iter().zip(d.right[i0..i1].iter()).map(|(l, r)| 0.5 * (l + r)).collect();
        let side = d.left[i0..i1].iter().zip(d.right[i0..i1].iter()).map(|(l, r)| ((l - r) as f64).powi(2)).sum::<f64>();
        let mid = mono.iter().map(|&m| (m as f64).powi(2) * 4.0).sum::<f64>();
        let width = if d.mono { 0.0 } else { (side / mid.max(1e-9)).sqrt() };
        let mut out = describe(&mono, d.rate, Some(width), offset + from / pace, pace);
        out["heard"] = super::hear::heard(&mono, d.rate, &|t| offset + (from + t) / pace);
        out["listened_to"] = json!(what);
        let _: Option<view::Time> = None;
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::Doc;

    /// A merged clip has no file of its own: listening to it plays the mix
    /// under it, within `from` / `to`, and never says "no media".
    #[test]
    fn a_merged_clip_is_listened_to_by_its_mix() {
        let mut s = Session::new(Doc::from_view(&view::demo()));
        let ids: Vec<u32> = s.doc.project.clips.iter().filter(|c| c.media.is_some()).take(2).map(|c| c.id).collect();
        let r = s.edit(&[json!({"op": "merge", "clips": ids})], "merge", false);
        let r = r.expect("merge");
        let id = r["changed"]["added"][0]["id"].as_u64().expect("merged clip");
        let merged = s.doc.project.clip(id as u32).unwrap().clone();
        assert!(merged.media.is_none());
        let e = s.listen(&json!({"clip": id.to_string(), "from": 0.0, "to": 6.0})).err().unwrap_or_default();
        assert!(!e.contains("no media"), "{e}");
        let e = s.listen(&json!({"clip": id, "from": 9999.0, "to": 9999.5})).err().unwrap_or_default();
        assert!(e.contains("nothing between") || e.contains("no sound"), "range is honoured: {e}");
    }
}
