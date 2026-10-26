// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `audio`: the sound side of a clip, as an editor works it: effects (EQ,
//! compressor, limiter, gate, de-esser, reverb, delay, pitch shift), fades
//! with their curve, and whether a speed change keeps the pitch. Volume
//! and pan are keyed with `animate` (`volume` in dB, `pan`).

use super::{Session, f};
use serde_json::Value;
use studio_model as m;

/// The sound effects a clip can carry, with their settings.
pub const EFFECTS: &str = "eq (bass, low_mid, mid, presence, air: dB ±24; high_pass, low_pass: Hz), compressor (threshold dB, ratio, attack ms, release ms, makeup dB), limiter (ceiling dB), gate (threshold dB, range dB), de_esser (threshold dB, freq Hz), reverb (room: booth, car, small_room, living_room, bathroom, office, corridor, hall, church, warehouse, cave, outdoor, plate; size, decay, mix 0-1, pre_delay ms), delay (time ms, feedback 0-0.95, mix 0-1), pitch_shift (semitones ±24)";

const KINDS: [&str; 8] = ["eq", "compressor", "limiter", "gate", "de_esser", "reverb", "delay", "pitch_shift"];

/// The setting a name means for a sound effect, taking the spellings
/// editors and other tools use (`highpass_hz`, `low_gain_db`,
/// `threshold_db`, `attack_ms`, `wet`…). `None`: not a setting it has.
fn param_name(kind: &str, key: &str) -> Option<&'static str> {
    let mut k = key.trim().to_ascii_lowercase().replace(['-', ' '], "_");
    for unit in ["_khz", "_hz", "_db", "_ms", "_s", "_pct", "_percent"] {
        if let Some(stripped) = k.strip_suffix(unit) {
            k = stripped.to_owned();
            break;
        }
    }
    let k = k.strip_suffix("_gain").map(str::to_owned).unwrap_or(k);
    Some(match (kind, k.as_str()) {
        ("eq", "bass" | "low" | "low_shelf" | "lows") => "bass",
        ("eq", "low_mid" | "lowmid" | "low_mids" | "mud") => "low_mid",
        ("eq", "mid" | "mids" | "middle") => "mid",
        ("eq", "presence" | "high_mid" | "upper_mid" | "highmid") => "presence",
        ("eq", "air" | "high" | "highs" | "treble" | "high_shelf") => "air",
        ("eq", "high_pass" | "highpass" | "hpf" | "hp" | "low_cut" | "lowcut") => "high_pass",
        ("eq", "low_pass" | "lowpass" | "lpf" | "lp" | "high_cut" | "highcut") => "low_pass",
        ("compressor" | "gate" | "de_esser", "threshold" | "thresh") => "threshold",
        ("compressor", "ratio") => "ratio",
        ("compressor", "attack") => "attack",
        ("compressor", "release") => "release",
        ("compressor", "makeup" | "make_up" | "output" | "gain") => "makeup",
        ("limiter", "ceiling" | "threshold" | "max" | "output") => "ceiling",
        ("gate", "range" | "reduction" | "floor") => "range",
        ("de_esser", "freq" | "frequency") => "freq",
        ("reverb", "size" | "room_size" | "room") => "size",
        ("reverb", "decay" | "time" | "length") => "decay",
        ("reverb" | "delay", "mix" | "wet" | "amount" | "dry_wet") => "mix",
        ("reverb", "pre_delay" | "predelay") => "pre_delay",
        ("reverb", "room" | "preset" | "space" | "type") => "room",
        ("delay", "time" | "delay") => "time",
        ("delay", "feedback" | "repeats") => "feedback",
        ("pitch_shift", "semitones" | "pitch" | "shift" | "amount") => "semitones",
        _ => return None,
    })
}

/// The settings of one effect, for the error that lists them.
fn settings_of(kind: &str) -> &'static str {
    match kind {
        "eq" => "bass, low_mid, mid, presence, air (dB, ±24), high_pass, low_pass (Hz)",
        "compressor" => "threshold (dB), ratio, attack (ms), release (ms), makeup (dB)",
        "limiter" => "ceiling (dB)",
        "gate" => "threshold (dB), range (dB)",
        "de_esser" => "threshold (dB), freq (Hz)",
        "reverb" => "room (booth, car, small_room, living_room, bathroom, office, corridor, hall, church, warehouse, cave, outdoor, plate), size, decay, mix (0-1), pre_delay (ms)",
        "delay" => "time (ms), feedback (0-0.95), mix (0-1)",
        _ => "semitones (±24)",
    }
}

fn curve(name: &str) -> Option<m::FadeCurve> {
    Some(match name {
        "linear" => m::FadeCurve::Linear,
        "equal_power" | "constant_power" | "crossfade" => m::FadeCurve::EqualPower,
        "s_curve" | "smooth" => m::FadeCurve::SCurve,
        "slow" | "exponential" => m::FadeCurve::Exponential,
        "fast" | "logarithmic" => m::FadeCurve::Logarithmic,
        _ => return None,
    })
}

/// The sound side of a clip as `inspect` reports it: its sound effects with
/// their settings, fade curves and pitch keeping, in the `audio` op's words.
pub(super) fn describe(c: &m::Clip) -> Value {
    let name = |k: m::FadeCurve| match k {
        m::FadeCurve::Linear => "linear",
        m::FadeCurve::EqualPower => "equal_power",
        m::FadeCurve::SCurve => "s_curve",
        m::FadeCurve::Exponential => "slow",
        m::FadeCurve::Logarithmic => "fast",
        #[allow(unreachable_patterns)]
        _ => "linear",
    };
    let effects: Vec<Value> = c
        .audio
        .effects
        .iter()
        .map(|e| {
            let set: serde_json::Map<String, Value> = e.params.iter().filter_map(|(k, v)| Some((k.clone(), serde_json::json!(v.as_f64()?)))).collect();
            serde_json::json!({"effect": e.kind, "enabled": e.enabled, "settings": set, "takes": settings_of(&e.kind)})
        })
        .collect();
    serde_json::json!({"effects": effects, "fade_in_curve": name(c.audio.fade_in.curve), "fade_out_curve": name(c.audio.fade_out.curve), "preserve_pitch": c.audio.preserve_pitch})
}

/// Adds a sound effect to a stack (or updates the one of that kind) with
/// the settings given; `new_id` is used when it is added. Clips, lanes,
/// buses and the master share this. Returns what the effect now holds.
pub(super) fn set_effect(effects: &mut Vec<m::Effect>, kind: &str, params: Option<&Value>, new_id: u64) -> Result<String, String> {
    if !KINDS.contains(&kind) {
        return Err(format!("sound effect \"{kind}\": {EFFECTS}"));
    }
    // One of each kind: settings given update it.
    let idx = match effects.iter().position(|e| e.kind == kind) {
        Some(i) => i,
        None => {
            effects.push(m::Effect::new(m::EffectId(new_id), kind));
            effects.len() - 1
        }
    };
    if let Some(Value::Object(ps)) = params {
        for (k, v) in ps {
            // A setting the mixer does not know was stored and silently
            // did nothing: refuse it instead.
            let name = param_name(kind, k).ok_or_else(|| format!("{kind} has no setting \"{k}\"; its settings: {}", settings_of(kind)))?;
            if name == "room" {
                let r = v.as_str().ok_or("reverb room is a name")?;
                if studio_sound::reverb::ReverbParams::room(r).is_none() {
                    return Err(format!("no room \"{r}\"; rooms: {}", studio_sound::reverb::ROOMS.iter().map(|x| x.0).collect::<Vec<_>>().join(", ")));
                }
                effects[idx].params.insert("room".into(), m::Value::Choice(r.trim().to_ascii_lowercase().replace([' ', '-'], "_")));
                continue;
            }
            let mut x = v.as_f64().ok_or_else(|| format!("{kind} {k} is a number"))?;
            if name == "mix" && x > 1.0 {
                x /= 100.0; // given in percent
            }
            effects[idx].params.insert(name.to_owned(), m::Value::Scalar(x));
        }
    }
    let held: Vec<String> = effects[idx].params.iter().filter_map(|(k, v)| match v {
        m::Value::Choice(c) => Some(format!("{k} {c}")),
        _ => v.as_f64().map(|x| format!("{k} {x}")),
    }).collect();
    Ok(if held.is_empty() { format!("{kind} at its defaults") } else { format!("{kind}: {}", held.join(", ")) })
}

/// The settings of a stack, in the words `set_effect` takes.
pub(super) fn describe_effects(effects: &[m::Effect]) -> Vec<Value> {
    effects
        .iter()
        .map(|e| {
            let set: serde_json::Map<String, Value> = e.params.iter().filter_map(|(k, v)| Some((k.clone(), match v {
                m::Value::Choice(c) => serde_json::json!(c),
                _ => serde_json::json!(v.as_f64()?),
            }))).collect();
            serde_json::json!({"effect": e.kind, "enabled": e.enabled, "settings": set})
        })
        .collect()
}

impl Session {
    pub(super) fn audio(&mut self, op: &Value) -> Result<String, String> {
        let mut ids: Vec<u64> = op.get("clip").and_then(Value::as_u64).into_iter().collect();
        if let Some(Value::Array(cs)) = op.get("clips") {
            ids.extend(cs.iter().filter_map(Value::as_u64));
        }
        if ids.is_empty() {
            return Err("audio needs \"clip\" (or \"clips\")".into());
        }
        let mut said = Vec::new();
        for id in ids {
            let mut c = self.model_clip(id as u32).ok_or_else(|| format!("no clip {id}"))?;
            let shape = match op.get("curve").and_then(Value::as_str) {
                Some(s) => Some(curve(s).ok_or_else(|| format!("curve \"{s}\": linear, equal_power, s_curve, slow or fast"))?),
                None => None,
            };
            for (key, fade) in [("fade_in", &mut c.audio.fade_in), ("fade_out", &mut c.audio.fade_out)] {
                if let Some(s) = f(op, key) {
                    fade.duration = super::micros(s.clamp(0.0, 30.0));
                    said.push(format!("{key} {s:.2}s"));
                }
                if let Some(k) = shape {
                    fade.curve = k;
                }
            }
            if let Some(b) = op.get("preserve_pitch").and_then(Value::as_bool) {
                c.audio.preserve_pitch = b;
                said.push(if b { "keeps its pitch when sped up".into() } else { "pitch follows speed".into() });
            }
            match op.get("remove") {
                Some(Value::Bool(true)) => {
                    c.audio.effects.clear();
                    said.push("no sound effects".into());
                }
                Some(Value::String(k)) => {
                    c.audio.effects.retain(|e| e.kind != *k);
                    said.push(format!("no {k}"));
                }
                _ => {}
            }
            if let Some(kind) = op.get("effect").and_then(Value::as_str) {
                let id = self.doc.alloc_id();
                said.push(set_effect(&mut c.audio.effects, kind, op.get("params"), id)?);
            }
            let seq = self.sequence_of(id).unwrap_or(self.doc.seq());
            let ok = self.doc.apply("Sound", m::Edit::SetClip { seq, clip: Box::new(c) });
            self.check(ok, "")?;
        }
        Ok(said.join("; "))
    }
}

#[cfg(test)]
mod tests {
    use super::param_name;
    use super::super::Session;
    use serde_json::json;

    /// Footage merged with something else keeps its sound, at the right
    /// time through a trim and a move (it was silent once merged).
    #[test]
    fn merged_clips_keep_their_sound_in_time() {
        let dir = std::env::temp_dir().join(format!("studio-merged-sound-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let wav = dir.join("click.wav");
        let rate = 48_000u32;
        let samples: Vec<f32> = (0..rate * 3).flat_map(|i| if (48_000..48_096).contains(&i) { [0.9, 0.9] } else { [0.0, 0.0] }).collect();
        std::fs::write(&wav, studio_media::wav::encode_pcm16(&studio_media::wav::Wav { sample_rate: rate, channels: 2, samples })).unwrap();
        let mut doc = crate::services::project::new_project(crate::services::project::canvas("Landscape").unwrap());
        crate::services::project::import_to_timeline(&mut doc, &[wav]).unwrap();
        let mut s = Session::new(doc);
        let sound = s.doc.project.clips.iter().find(|c| c.media.is_some()).unwrap().id;
        s.edit(&[json!({"op": "card", "colour": "white", "at": 0.0, "duration": 3.0, "width": 100, "height": 100}), json!({"op": "merge", "clips": [sound, "$1"]})], "x", false).unwrap();
        let merged = s.doc.project.clips.iter().find(|c| c.name == "Merged").unwrap().id;
        s.edit(&[json!({"op": "trim", "clip": merged, "start": 0.5}), json!({"op": "move", "clips": [merged], "to": 2.0})], "x", false).unwrap();
        let mix = crate::sound_bridge::mix_for_export(&s.doc.model, 0, 5 * crate::view::SEC);
        let loudest = (0..mix.len() / 2).max_by(|&a, &b| mix[2 * a].abs().total_cmp(&mix[2 * b].abs())).unwrap();
        assert!(mix[2 * loudest].abs() > 0.3, "the merged clip is heard");
        // 1.0 s into the sound, 0.5 s trimmed off, the clip at 2.0 s: 2.5 s.
        assert!((loudest as f64 / rate as f64 - 2.5).abs() < 0.003, "click at {:.4}s", loudest as f64 / rate as f64);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn settings_take_common_spellings_and_refuse_unknown_ones() {
        assert_eq!(param_name("eq", "highpass_hz"), Some("high_pass"));
        assert_eq!(param_name("eq", "low_gain_db"), Some("bass"));
        assert_eq!(param_name("eq", "high_gain_db"), Some("air"));
        assert_eq!(param_name("eq", "mid_gain_db"), Some("mid"));
        assert_eq!(param_name("compressor", "threshold_db"), Some("threshold"));
        assert_eq!(param_name("compressor", "attack_ms"), Some("attack"));
        assert_eq!(param_name("compressor", "makeup_db"), Some("makeup"));
        assert_eq!(param_name("reverb", "wet"), Some("mix"));
        assert_eq!(param_name("de_esser", "frequency_hz"), Some("freq"));
        assert_eq!(param_name("eq", "sparkle"), None);
        assert_eq!(param_name("limiter", "ratio"), None);
    }
}
