// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Loudness, limiting and the reverb bus, built on `studio-sound`.

use crate::dsp::*;
use studio_sound::buffer::Effect;
use studio_sound::dynamics::{Limiter, LimiterParams};
use studio_sound::loudness;
use studio_sound::reverb::{Reverb, ReverbParams};

/// Integrated loudness in LUFS (negative infinity for silence).
pub fn lufs(s: &Stereo) -> f64 {
    loudness::measure(&s.l, &s.r, RATE).integrated_lufs
}

/// Runs a true-peak limiter over the signal, keeping its length.
pub fn limit(s: &mut Stereo, ceiling_db: f32) {
    let mut lim = Limiter::new(RATE, LimiterParams { ceiling_db, release_ms: 120.0, true_peak: true });
    let lat = lim.latency();
    let n = s.len();
    s.l.extend(std::iter::repeat_n(0.0, lat));
    s.r.extend(std::iter::repeat_n(0.0, lat));
    for (l, r) in s.l.chunks_mut(4096).zip(s.r.chunks_mut(4096)) {
        lim.process(l, r);
    }
    s.l.drain(..lat);
    s.r.drain(..lat);
    s.fit(n);
}

/// Brings the mix to `target` LUFS with nothing over −1.5 dBFS. Returns
/// the loudness reached and the sample peak.
pub fn master(s: &mut Stereo, target: f64) -> (f64, f32) {
    for _ in 0..4 {
        let m = lufs(s);
        if !m.is_finite() {
            return (m, s.peak());
        }
        let diff = target - m;
        if diff.abs() < 0.15 && s.peak() <= db(-1.4) {
            break;
        }
        s.scale(db(diff as f32));
        limit(s, -1.5);
    }
    (lufs(s), s.peak())
}

/// Rings a mono-compatible send through the room named `room`
/// (`studio_sound::reverb::ROOMS`), wet only, same length.
pub fn reverb(send: &Stereo, room: &str) -> Stereo {
    let mut p = ReverbParams::room(room).unwrap_or(ReverbParams::HALL);
    p.mix = 1.0;
    let mut rv = Reverb::new(RATE, p);
    let mut out = send.clone();
    for (l, r) in out.l.chunks_mut(4096).zip(out.r.chunks_mut(4096)) {
        rv.process(l, r);
    }
    out
}

/// Removes anything below `hz` (rumble) in place.
pub fn high_pass(s: &mut Stereo, hz: f32) {
    let c = SvfCoef::new(hz, 0.7);
    let (mut a, mut b) = (Svf::new(), Svf::new());
    for v in s.l.iter_mut() {
        *v = a.tick(&c, *v).hp;
    }
    for v in s.r.iter_mut() {
        *v = b.tick(&c, *v).hp;
    }
}
