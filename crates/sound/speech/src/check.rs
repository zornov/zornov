// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The sync check: what a mouth does against what the voice does. It
//! flags a mouth that moves in silence, one that stays still while
//! someone speaks, lip closures (M, B, P) that never happen or are too
//! brief to see, and a mouth that is out of step with the sound by more
//! than two frames. Each flag comes with its fix.

use crate::features::Frames;
use crate::phoneme::{Ph, Timed};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    MovesInSilence,
    StillWhileSpeaking,
    MissedClosure,
    ShortClosure,
    OutOfSync,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::MovesInSilence => "mouth moves in silence",
            Kind::StillWhileSpeaking => "mouth still while speaking",
            Kind::MissedClosure => "missed M/B/P closure",
            Kind::ShortClosure => "M/B/P closure too short to see",
            Kind::OutOfSync => "out of sync",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Issue {
    pub kind: Kind,
    pub t0: f64,
    pub t1: f64,
    pub detail: String,
    pub fix: String,
    /// For `OutOfSync`: the shift to apply to the mouth, seconds (+ later).
    pub shift: Option<f64>,
}

#[derive(Clone, Debug, Default)]
pub struct Report {
    pub issues: Vec<Issue>,
    /// How far the mouth trails the sound, seconds (negative: it leads),
    /// when there was enough speech to measure.
    pub lag: Option<f64>,
}

/// What to measure against.
pub struct Input<'a> {
    /// How open the mouth looks, 100 samples a second (0 shut … 1 wide).
    pub open: &'a [f32],
    /// How firmly the lips are pressed, if the character has that (else
    /// a shut mouth counts as a closure).
    pub close: Option<&'a [f32]>,
    pub audio: &'a Frames,
    pub phones: &'a [Timed],
    /// Frame rate of the video the mouth is shown in.
    pub fps: f64,
    /// How far the mouth is meant to lead the sound, seconds.
    pub lead: f64,
}

fn at(v: &[f32], t: f64) -> f32 {
    v.get((t.max(0.0) * 100.0) as usize).copied().unwrap_or(0.0)
}

fn runs(flags: &[bool], min_len: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut s = None;
    for (i, &f) in flags.iter().enumerate().chain(std::iter::once((flags.len(), &false))) {
        match (f, s) {
            (true, None) => s = Some(i),
            (false, Some(a)) => {
                if i - a >= min_len {
                    out.push((a, i));
                }
                s = None;
            }
            _ => {}
        }
    }
    out
}

pub fn check(inp: &Input) -> Report {
    let mut rep = Report::default();
    let a = inp.audio;
    let n = a.len().min(inp.open.len().max(a.len()));
    let frame = 1.0 / inp.fps.max(1.0);
    // Speech: loud enough and either voiced or hissy; smoothed over 60 ms.
    let speech: Vec<bool> = (0..n)
        .map(|i| {
            let (lo, hi) = (i.saturating_sub(3), (i + 4).min(a.len()));
            let l = a.loud[lo..hi].iter().sum::<f32>() / (hi - lo) as f32;
            l > 0.12
        })
        .collect();
    // 1. Moving in silence.
    let silent: Vec<bool> = speech.iter().map(|s| !s).collect();
    let margin = ((inp.lead + 0.12) * 100.0) as usize;
    for (s, e) in runs(&silent, margin * 2 + 20) {
        let (s2, e2) = (s + 12, e.saturating_sub((inp.lead * 100.0) as usize + 6));
        if e2 <= s2 {
            continue;
        }
        let moving: Vec<bool> = (s2..e2).map(|i| at(inp.open, i as f64 / 100.0) > 0.2).collect();
        if let Some((ms, me)) = runs(&moving, 12).first().copied() {
            let (t0, t1) = ((s2 + ms) as f64 / 100.0, (s2 + me) as f64 / 100.0);
            rep.issues.push(Issue {
                kind: Kind::MovesInSilence,
                t0,
                t1,
                detail: format!("the mouth is open ({:.0}%) from {t0:.2}s to {t1:.2}s but the voice is silent", 100.0 * at(inp.open, (t0 + t1) / 2.0)),
                fix: format!("close the mouth between {t0:.2}s and {t1:.2}s: run lip_sync again on the line (it rests the mouth in pauses), or delete the mouth keys there"),
                shift: None,
            });
        }
    }
    // 2. Still while speaking.
    let voiced: Vec<bool> = (0..n).map(|i| speech[i] && a.loud[i.min(a.len() - 1)] > 0.2).collect();
    for (s, e) in runs(&voiced, 35) {
        let (s2, e2) = (s + 10, e.saturating_sub(10));
        if e2 <= s2 + 20 {
            continue;
        }
        let vals: Vec<f32> = (s2..e2).map(|i| at(inp.open, i as f64 / 100.0)).collect();
        let (lo, hi) = vals.iter().fold((f32::MAX, f32::MIN), |(l, h), v| (l.min(*v), h.max(*v)));
        if hi - lo < 0.08 {
            let (t0, t1) = (s as f64 / 100.0, e as f64 / 100.0);
            rep.issues.push(Issue {
                kind: Kind::StillWhileSpeaking,
                t0,
                t1,
                detail: format!("the mouth barely moves ({:.0}% to {:.0}%) while the voice speaks from {t0:.2}s to {t1:.2}s", 100.0 * lo, 100.0 * hi),
                fix: format!("run lip_sync on the line for {t0:.2}s to {t1:.2}s (raise intensity above 1 if the mouth moves too little)"),
                shift: None,
            });
        }
    }
    // 3. Closures on M, B, P.
    let closed = |t: f64| -> bool {
        match inp.close {
            Some(c) => at(c, t) > 0.5 || at(inp.open, t) < 0.06,
            None => at(inp.open, t) < 0.08,
        }
    };
    for p in inp.phones.iter().filter(|p| p.ph.is_bilabial_stop()) {
        // The mouth leads the sound, so look from before the sound begins.
        let (w0, w1) = (p.start - inp.lead - 0.06, p.end + 0.03);
        let mut shut = 0.0;
        let mut t = w0;
        while t <= w1 {
            if closed(t) {
                shut += 0.01;
            }
            t += 0.01;
        }
        // Word-initial or final M/B/P at the very start/end of a clip may be at rest anyway.
        if shut < 0.01 {
            rep.issues.push(Issue {
                kind: Kind::MissedClosure,
                t0: p.start,
                t1: p.end,
                detail: format!("the lips never close for {} at {:.2}s (open {:.0}%)", p.ph.name(), p.start, 100.0 * at(inp.open, (p.start + p.end) / 2.0)),
                fix: format!("close the lips at {:.2}s for at least {:.0} frames (run lip_sync again, which holds M/B/P closed)", p.start - inp.lead, 2.0),
                shift: None,
            });
        } else if shut + 1e-9 < 1.7 * frame && p.ph != Ph::M {
            rep.issues.push(Issue {
                kind: Kind::ShortClosure,
                t0: p.start,
                t1: p.end,
                detail: format!("the lips close for {:.0} ms at {:.2}s, under two frames at {} fps", shut * 1000.0, p.start, inp.fps),
                fix: "hold the closure for at least 2 frames".into(),
                shift: None,
            });
        }
    }
    // 4. Sync: the mouth's opening against the voice's loudness.
    // What the sound asks of the jaw: loudness, plus the opening each phoneme
    // calls for (irregular, so one answer fits where syllables alone repeat).
    let mut tmpl = vec![0.0f64; a.len()];
    for p in inp.phones {
        let jaw = crate::viseme::lip_targets(crate::viseme::of_phoneme(p.ph).0)[0].0 as f64;
        let (s, e) = (((p.start * 100.0) as usize).min(a.len()), ((p.end * 100.0) as usize).min(a.len()));
        for t in &mut tmpl[s..e] {
            *t = jaw;
        }
    }
    let loud = smooth(&a.loud.iter().zip(smooth(&tmpl, 2)).map(|(v, t)| *v as f64 + t).collect::<Vec<_>>(), 3);
    let mouth = smooth(&(0..a.len()).map(|i| at(inp.open, i as f64 / 100.0) as f64).collect::<Vec<_>>(), 4);
    let active = speech.iter().filter(|s| **s).count();
    if active >= 80 {
        let mut best = (f64::MIN, 0i32);
        let mut scores: Vec<(i32, f64)> = Vec::new();
        for lag in -30i32..=30 {
            // Mouth at i+lag against sound at i.
            let (mut sxy, mut sx, mut sy, mut sxx, mut syy, mut cnt) = (0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
            for i in 0..a.len() {
                let j = i as i32 + lag;
                if j < 0 || j as usize >= mouth.len() || !speech[i] {
                    continue;
                }
                let (x, y) = (loud[i], mouth[j as usize]);
                sx += x;
                sy += y;
                sxy += x * y;
                sxx += x * x;
                syy += y * y;
                cnt += 1.0;
            }
            if cnt < 40.0 {
                continue;
            }
            let (vx, vy) = (sxx / cnt - (sx / cnt).powi(2), syy / cnt - (sy / cnt).powi(2));
            if vx < 1e-4 || vy < 1e-5 {
                continue;
            }
            let r = (sxy / cnt - sx / cnt * sy / cnt) / (vx * vy).sqrt();
            scores.push((lag, r));
            if r > best.0 {
                best = (r, lag);
            }
        }
        // Only trust a clear peak that beats the lag the mouth should have:
        // speech repeats about five times a second, so a rival peak a syllable
        // away that is only a little better proves nothing.
        let expected = (-inp.lead * 100.0).round() as i32;
        let r_expected = scores.iter().find(|s| s.0 == expected).map_or(f64::MIN, |s| s.1);
        if best.0 > 0.25 && best.0 - r_expected > 0.1 {
            let lag = best.1 as f64 / 100.0;
            rep.lag = Some(lag);
            let err = lag + inp.lead;
            if err.abs() > 2.0 * frame {
                let shift = -err;
                rep.issues.push(Issue {
                    kind: Kind::OutOfSync,
                    t0: 0.0,
                    t1: a.len() as f64 / 100.0,
                    detail: format!(
                        "the mouth is {:.1} frames {} the voice (it should lead by about {:.0} ms; it {} by {:.0} ms)",
                        err.abs() / frame,
                        if err > 0.0 { "behind" } else { "ahead of" },
                        inp.lead * 1000.0,
                        if lag > 0.0 { "trails" } else { "leads" },
                        lag.abs() * 1000.0
                    ),
                    fix: format!("move the mouth keys {} by {:.3}s ({:.1} frames): lip_sync with \"offset\": {:.3}", if shift > 0.0 { "later" } else { "earlier" }, shift.abs(), shift.abs() / frame, shift),
                    shift: Some(shift),
                });
            }
        }
    }
    rep
}

fn smooth(v: &[f64], n: usize) -> Vec<f64> {
    (0..v.len())
        .map(|i| {
            let (a, b) = (i.saturating_sub(n), (i + n + 1).min(v.len()));
            v[a..b].iter().sum::<f64>() / (b - a) as f64
        })
        .collect()
}
