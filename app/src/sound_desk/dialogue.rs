// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Dialogue editing on clips: leveling (phrase-by-phrase gain written as
//! the clip's volume keys, so it stays editable) and voice matching (EQ,
//! level and room put on a take so it sits with a reference).

use crate::doc::Doc;
use studio_model as m;
use studio_sound::dialogue::{LevelerParams, VoiceMatch, level_keys, match_voice, phrases, profile};

/// The clip and the sequence holding it.
pub fn find_clip(doc: &Doc, id: u64) -> Option<(m::SequenceId, m::Clip)> {
    doc.model.sequences.iter().find_map(|s| s.clip(m::ClipId(id)).map(|c| (s.id, c.clone())))
}

/// A clip's own sound (what its media plays over the clip, before any
/// volume or effect), mono at the file's rate.
pub fn clip_sound(project: &m::Project, clip: &m::Clip) -> Option<(Vec<f32>, u32)> {
    let m::ClipSource::Media(r) = &clip.source else { return None };
    if !r.audio {
        return None;
    }
    let item = project.media_item(r.media)?;
    let d = crate::sound_bridge::decoded(&item.path)?;
    let speed = match &clip.speed {
        m::Speed::Constant(s) => *s,
        m::Speed::Curve(_) => clip.speed.integrate(clip.duration) / clip.duration.max(1) as f64,
    };
    let a = studio_sound::micros_to_frames(clip.source_in, d.rate).max(0) as usize;
    let b = studio_sound::micros_to_frames(clip.source_in + (clip.duration as f64 * speed) as i64, d.rate).max(0) as usize;
    let (a, b) = (a.min(d.left.len()), b.min(d.left.len()));
    if b <= a {
        return None;
    }
    let mono: Vec<f32> = d.left[a..b].iter().zip(&d.right[a..b]).map(|(l, r)| 0.5 * (l + r)).collect();
    // Clip time runs `speed` times slower than the source: report a rate
    // that maps samples to clip time.
    Some((mono, ((d.rate as f64) / speed).round().max(1.0) as u32))
}

/// Levels a clip's phrases to `target_lufs`: replaces its volume keys with
/// one hold per phrase (on top of its current volume). Returns the phrases
/// found and the gain each got.
pub fn level(doc: &mut Doc, clip: u64, p: LevelerParams) -> Result<Vec<studio_sound::dialogue::Phrase>, String> {
    let (seq, mut c) = find_clip(doc, clip).ok_or_else(|| format!("no clip {clip}"))?;
    let (mono, rate) = clip_sound(&doc.model, &c).ok_or_else(|| format!("clip {clip} has no sound to level"))?;
    let ph = phrases(&mono, rate, p);
    if ph.is_empty() {
        return Err(format!("clip {clip}: no speech found to level"));
    }
    let keys = level_keys(&ph, c.audio.volume_db as f32);
    let track = m::Track::new(keys.iter().map(|&(t, v)| m::Keyframe::new(t.min(c.duration), m::Value::Scalar(v as f64), m::Interp::Linear)).collect());
    c.keyframes.insert("audio.volume_db".into(), track);
    if !doc.apply("Level dialogue", m::Edit::SetClip { seq, clip: Box::new(c) }) {
        return Err(doc.error.take().unwrap_or_else(|| "leveling was refused".into()));
    }
    Ok(ph)
}

/// Makes clip `take` sound like clip `reference`: an EQ with the five
/// band corrections, the level difference as clip gain, and a little room
/// when the reference is livelier. Earlier matches on the take are
/// replaced. Returns what was applied.
pub fn match_clip(doc: &mut Doc, take: u64, reference: u64) -> Result<VoiceMatch, String> {
    let (_, r) = find_clip(doc, reference).ok_or_else(|| format!("no clip {reference}"))?;
    let (seq, mut c) = find_clip(doc, take).ok_or_else(|| format!("no clip {take}"))?;
    let (rs, rrate) = clip_sound(&doc.model, &r).ok_or_else(|| format!("clip {reference} has no sound"))?;
    let (ts, trate) = clip_sound(&doc.model, &c).ok_or_else(|| format!("clip {take} has no sound"))?;
    let mut vm = match_voice(&profile(&rs, rrate), &profile(&ts, trate));
    // The reference's own volume counts too.
    vm.gain_db += (r.audio.volume_db - c.audio.volume_db) as f32;
    apply_match(&mut c, &vm);
    if !doc.apply("Match voice", m::Edit::SetClip { seq, clip: Box::new(c) }) {
        return Err(doc.error.take().unwrap_or_else(|| "matching was refused".into()));
    }
    Ok(vm)
}

/// Writes a match into a clip's sound settings.
pub fn apply_match(c: &mut m::Clip, vm: &VoiceMatch) {
    c.audio.effects.retain(|e| !e.extra.contains_key("voice_match"));
    let next_id = |c: &m::Clip| c.audio.effects.iter().map(|e| e.id.0).max().unwrap_or(0) + 1;
    let mut eq = super::effect("eq", &[("bass", vm.eq_db[0] as f64), ("low_mid", vm.eq_db[1] as f64), ("mid", vm.eq_db[2] as f64), ("presence", vm.eq_db[3] as f64), ("air", vm.eq_db[4] as f64)]);
    eq.id = m::EffectId(next_id(c));
    eq.extra.insert("voice_match".into(), serde_json::Value::Bool(true));
    c.audio.effects.push(eq);
    if vm.reverb_mix > 0.0 {
        let mut rv = super::effect("reverb", &[("mix", vm.reverb_mix as f64)]);
        rv.params.insert("room".into(), m::Value::Choice("small_room".into()));
        rv.id = m::EffectId(next_id(c));
        rv.extra.insert("voice_match".into(), serde_json::Value::Bool(true));
        c.audio.effects.push(rv);
    }
    c.audio.volume_db += vm.gain_db as f64;
}
