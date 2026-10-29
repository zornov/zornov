// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A short two-note chime when a long task finishes (exports end
//! with a chime). Synthesised here and played through `studio-audio`.

/// Play the chime unless the person has turned sounds off.
pub fn play_unless(quiet: bool) {
    if !quiet {
        play();
    }
}

/// The output device, opened once and kept for the next chime.
static OUT: std::sync::Mutex<Option<studio_audio::AudioOut>> = std::sync::Mutex::new(None);

/// Play the chime on a background thread; silent if there is no output.
pub fn play() {
    std::thread::spawn(|| {
        let mut out = OUT.lock().unwrap_or_else(|e| e.into_inner());
        if out.is_none() {
            *out = studio_audio::AudioOut::open().ok();
        }
        if let Some(o) = out.as_ref() {
            o.push(&samples());
            // Hold the device until the notes have played, so two chimes do not overlap.
            std::thread::sleep(std::time::Duration::from_millis(900));
        }
    });
}

/// Two soft sine notes a fifth apart with quick attack and gentle decay,
/// 48 kHz interleaved stereo.
pub fn samples() -> Vec<f32> {
    const RATE: f32 = 48_000.0;
    let notes = [(0.0f32, 880.0f32), (0.12, 1318.5)];
    let len = (RATE * 0.8) as usize;
    let mut out = vec![0.0f32; len * 2];
    for (start, freq) in notes {
        let s0 = (start * RATE) as usize;
        for i in s0..len {
            let t = (i - s0) as f32 / RATE;
            let env = (t / 0.008).min(1.0) * (-t * 6.0).exp();
            // A touch of the octave makes it bell-like.
            let v = 0.16 * env * ((std::f32::consts::TAU * freq * t).sin() + 0.25 * (std::f32::consts::TAU * freq * 2.0 * t).sin());
            out[i * 2] += v;
            out[i * 2 + 1] += v;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn chime_is_quiet_and_finite() {
        let s = super::samples();
        assert!(s.iter().all(|v| v.is_finite() && v.abs() < 0.5));
        assert!(s.iter().any(|v| v.abs() > 0.05));
    }
}
