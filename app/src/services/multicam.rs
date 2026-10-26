// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Multicam: several cameras of one take as one clip that cuts between
//! them.
//!
//! * [`sync`] finds where each angle starts relative to the first: by
//!   source timecode, or by sound — the onsets of each file's sound
//!   (rises of a 100 Hz loudness envelope) are cross-correlated with an
//!   FFT, the best lag refined to a millisecond on a 1 kHz envelope.
//! * [`build`] makes the multicam sequence: angle 1 on its main lane,
//!   every other angle on a lane above it, pictures only, plus the sound of
//!   one angle (the audio angle) on a sound lane; and a clip of it for the
//!   timeline, showing angle 1 (`meta.multicam = {angle, angles, names}`,
//!   the angle being the lane id the renderer draws alone).
//! * [`switch`] cuts a multicam clip at a time and shows another angle
//!   after the cut; [`set_angle`] changes a whole clip's angle.

use studio_model as m;

/// How to line the angles up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncBy {
    Sound,
    Timecode,
}

/// Start of each angle (in `ids` order) relative to the earliest, and a
/// confidence 0..1 per angle (1 for timecode).
pub fn sync(p: &m::Project, ids: &[m::MediaId], by: SyncBy) -> Result<Vec<(m::Micros, f32)>, String> {
    if ids.len() < 2 {
        return Err("multicam needs at least two angles".into());
    }
    let items: Vec<&m::MediaItem> = ids.iter().map(|id| p.media_item(*id).ok_or_else(|| format!("no media {}", id.0))).collect::<Result<_, _>>()?;
    let starts: Vec<(m::Micros, f32)> = match by {
        SyncBy::Timecode => {
            let missing: Vec<&str> = items.iter().filter(|i| i.timecode.is_none()).map(|i| i.name.as_str()).collect();
            if !missing.is_empty() {
                return Err(format!("no source timecode on {}; sync by sound instead", missing.join(", ")));
            }
            items.iter().map(|i| (i.timecode_start(p.settings.frame_rate), 1.0)).collect()
        }
        SyncBy::Sound => {
            let env: Vec<(Vec<f32>, Vec<f32>)> = items
                .iter()
                .map(|i| {
                    let d = crate::sound_bridge::decoded(&i.path).ok_or_else(|| format!("{} has no sound to sync by", i.name))?;
                    Ok((envelope(&d, 100), envelope(&d, 1000)))
                })
                .collect::<Result<_, String>>()?;
            let mut out = vec![(0, 1.0)];
            for (e, fine) in env.iter().skip(1) {
                let (lag, conf) = best_lag(&onsets(&env[0].0), &onsets(e));
                // Refine on the 1 kHz envelope around the coarse lag.
                let coarse = lag * 10;
                let mut best = (coarse, f32::MIN);
                let a = onsets(&env[0].1);
                let b = onsets(fine);
                for d in coarse - 15..=coarse + 15 {
                    let s = dot_at(&a, &b, d);
                    if s > best.1 {
                        best = (d, s);
                    }
                }
                // Angle i's own time t plays at reference time t - lag:
                // it starts `lag` milliseconds after the reference.
                out.push((-(best.0) * 1000, conf));
            }
            out
        }
    };
    let min = starts.iter().map(|s| s.0).min().unwrap_or(0);
    Ok(starts.into_iter().map(|(s, c)| (s - min, c)).collect())
}

/// Mean absolute level of the mono mix in windows of 1 / `rate` s.
fn envelope(d: &crate::sound_bridge::Decoded, rate: u32) -> Vec<f32> {
    let win = (d.rate / rate).max(1) as usize;
    let n = d.left.len() / win;
    (0..n)
        .map(|i| {
            let s = &d.left[i * win..(i + 1) * win];
            let r = &d.right[i * win..((i + 1) * win).min(d.right.len())];
            (s.iter().map(|v| v.abs()).sum::<f32>() + r.iter().map(|v| v.abs()).sum::<f32>()) / (2 * win) as f32
        })
        .collect()
}

/// Rises of an envelope (positive differences), zero-mean, unit length.
fn onsets(e: &[f32]) -> Vec<f32> {
    let mut o: Vec<f32> = e.windows(2).map(|w| (w[1] - w[0]).max(0.0)).collect();
    let mean = o.iter().sum::<f32>() / o.len().max(1) as f32;
    for v in &mut o {
        *v -= mean;
    }
    let norm = o.iter().map(|v| v * v).sum::<f32>().sqrt().max(1e-12);
    for v in &mut o {
        *v /= norm;
    }
    o
}

/// Σ a[i] · b[i + lag].
fn dot_at(a: &[f32], b: &[f32], lag: i64) -> f32 {
    let mut s = 0f32;
    for (i, x) in a.iter().enumerate() {
        let j = i as i64 + lag;
        if j >= 0 && (j as usize) < b.len() {
            s += x * b[j as usize];
        }
    }
    s
}

/// The lag (in samples) maximising Σ a[i]·b[i + lag], with the peak's
/// height as a confidence, by FFT cross-correlation.
fn best_lag(a: &[f32], b: &[f32]) -> (i64, f32) {
    let n = (a.len() + b.len()).next_power_of_two().max(2);
    let mut fa: Vec<(f64, f64)> = a.iter().map(|v| (*v as f64, 0.0)).chain(std::iter::repeat((0.0, 0.0))).take(n).collect();
    let mut fb: Vec<(f64, f64)> = b.iter().map(|v| (*v as f64, 0.0)).chain(std::iter::repeat((0.0, 0.0))).take(n).collect();
    fft(&mut fa, false);
    fft(&mut fb, false);
    // corr[lag] = Σ a[i] b[i+lag] = IFFT(conj(A) · B).
    let mut c: Vec<(f64, f64)> = fa.iter().zip(&fb).map(|(x, y)| (x.0 * y.0 + x.1 * y.1, x.0 * y.1 - x.1 * y.0)).collect();
    fft(&mut c, true);
    let mut best = (0i64, f64::MIN);
    for (i, v) in c.iter().enumerate() {
        let lag = if i < n / 2 { i as i64 } else { i as i64 - n as i64 };
        if v.0 > best.1 {
            best = (lag, v.0);
        }
    }
    (best.0, best.1.clamp(0.0, 1.0) as f32)
}

/// In-place radix-2 FFT (`inverse` scales by 1/n).
fn fft(x: &mut [(f64, f64)], inverse: bool) {
    let n = x.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            x.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let ang = 2.0 * std::f64::consts::PI / len as f64 * if inverse { 1.0 } else { -1.0 };
        let (wr, wi) = (ang.cos(), ang.sin());
        for start in (0..n).step_by(len) {
            let (mut cr, mut ci) = (1.0, 0.0);
            for k in 0..len / 2 {
                let (ur, ui) = x[start + k];
                let (vr0, vi0) = x[start + k + len / 2];
                let (vr, vi) = (vr0 * cr - vi0 * ci, vr0 * ci + vi0 * cr);
                x[start + k] = (ur + vr, ui + vi);
                x[start + k + len / 2] = (ur - vr, ui - vi);
                let nc = cr * wr - ci * wi;
                ci = cr * wi + ci * wr;
                cr = nc;
            }
        }
        len <<= 1;
    }
    if inverse {
        for v in x.iter_mut() {
            v.0 /= n as f64;
            v.1 /= n as f64;
        }
    }
}

/// What [`build`] made.
#[derive(Clone, Debug)]
pub struct Built {
    pub edit: m::Edit,
    pub sequence: m::SequenceId,
    /// The clip to place on the timeline (not yet placed).
    pub clip: m::Clip,
    pub angles: Vec<m::LaneId>,
}

/// The multicam sequence for `ids` started at `starts`, with the sound of
/// angle `audio` (index into `ids`), and a clip of it for lane `lane`.
pub fn build(p: &mut m::Project, name: &str, ids: &[m::MediaId], starts: &[m::Micros], audio: usize, lane: m::LaneId) -> Result<Built, String> {
    let seq_id = m::SequenceId(p.alloc_id());
    let spine = m::LaneId(p.alloc_id());
    let mut s = m::Sequence::new(seq_id, format!("{name} (multicam)"), spine);
    let mut angles = vec![spine];
    let mut names = Vec::new();
    for (i, (id, start)) in ids.iter().zip(starts).enumerate() {
        let item = p.media_item(*id).ok_or_else(|| format!("no media {}", id.0))?.clone();
        let dur = item.info.duration.ok_or_else(|| format!("{} has no length", item.name))?;
        let l = if i == 0 {
            spine
        } else {
            let l = m::LaneId(p.alloc_id());
            s.lanes.insert(0, m::Lane::new(l, m::LaneKind::Visual, format!("Angle {}", i + 1)));
            angles.push(l);
            l
        };
        names.push(item.name.clone());
        let mut c = m::Clip::media(m::ClipId(p.alloc_id()), l, *start, *id, 0, dur);
        if let m::ClipSource::Media(r) = &mut c.source {
            r.audio = false;
        }
        c.name = format!("Angle {}: {}", i + 1, item.name);
        if i == 0 {
            // The main lane cannot start with empty space.
            if *start > 0 {
                s.clips.push(m::Clip::gap(m::ClipId(p.alloc_id()), spine, 0, *start));
            }
        }
        s.clips.push(c);
        if i == audio && item.has_audio() {
            let al = m::LaneId(p.alloc_id());
            s.lanes.push(m::Lane::new(al, m::LaneKind::Audio, format!("Sound (angle {})", i + 1)));
            let mut a = m::Clip::media(m::ClipId(p.alloc_id()), al, *start, *id, 0, dur);
            if let m::ClipSource::Media(r) = &mut a.source {
                r.video = false;
            }
            a.name = format!("Sound: {}", item.name);
            s.clips.push(a);
        }
    }
    s.clips.sort_by_key(|c| (c.lane.0, c.start));
    let len = s.duration();
    let mut clip = m::Clip::new(m::ClipId(p.alloc_id()), lane, 0, len, m::ClipSource::Sequence(m::SequenceRef { sequence: seq_id, extra: Default::default() }));
    clip.name = name.to_string();
    clip.set_multicam(spine, &angles, &names);
    Ok(Built { edit: m::Edit::InsertSequence { sequence: Box::new(s), index: None }, sequence: seq_id, clip, angles })
}

/// The angle lanes of a multicam clip.
pub fn angles(c: &m::Clip) -> Option<Vec<m::LaneId>> {
    c.multicam_angles()
}

/// The edit showing angle `n` (1-based) for the whole clip.
pub fn set_angle(seq: m::SequenceId, c: &m::Clip, n: usize) -> Result<m::Edit, String> {
    let lanes = angles(c).ok_or("not a multicam clip")?;
    let lane = *lanes.get(n.wrapping_sub(1)).ok_or_else(|| format!("angle {n} does not exist; the clip has {}", lanes.len()))?;
    let mut nc = c.clone();
    nc.set_multicam_angle(lane);
    Ok(m::Edit::SetClip { seq, clip: Box::new(nc) })
}

/// Cuts the multicam clip at `at` (sequence time) and shows angle `n`
/// after the cut. Returns the edits to apply in order: the split, then the
/// angle change of the piece after it (found once the split is applied).
pub fn switch(p: &m::Project, seq: m::SequenceId, clip: m::ClipId, at: m::Micros, n: usize) -> Result<m::Project, String> {
    let mut q = p.clone();
    let c = q.sequence(seq).and_then(|s| s.clip(clip)).ok_or("no such clip")?.clone();
    angles(&c).ok_or("not a multicam clip")?;
    let at = q.settings.frame_rate.snap(at);
    let target = if at > c.start && at < c.end() {
        q.apply(m::Edit::Split { seq, at, clips: Some(vec![clip]) }).map_err(|e| e.to_string())?;
        let s = q.sequence(seq).ok_or("sequence")?;
        s.clips.iter().find(|x| x.lane == c.lane && x.start == at).cloned().ok_or("the split left no clip after the cut")?
    } else {
        c
    };
    q.apply(set_angle(seq, &target, n)?).map_err(|e| e.to_string())?;
    Ok(q)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fft_cross_correlation_finds_a_shift() {
        let a: Vec<f32> = (0..400).map(|i| if [30, 77, 150, 151, 260, 333].contains(&i) { 1.0 } else { 0.0 }).collect();
        let shift = 23;
        let b: Vec<f32> = (0..400).map(|i| if i >= shift { a[i - shift] } else { 0.0 }).collect();
        let (lag, conf) = best_lag(&onsets(&a), &onsets(&b));
        assert_eq!(lag, shift as i64);
        assert!(conf > 0.5);
        let (lag, _) = best_lag(&onsets(&b), &onsets(&a));
        assert_eq!(lag, -(shift as i64));
    }
}
