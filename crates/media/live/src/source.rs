// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Where pictures and sound come from.
//!
//! A source is pulled: the recorder or the mixer asks for the picture at a
//! time, or for the next `n` samples, so every consumer runs on its own
//! clock and tests run faster than real time. Test sources (`test:...`)
//! need no device and no permission; they are what the headless agent and
//! every test use. Real devices (`camera:`, `mic:`, `screen:`, `window:`)
//! open only when the person has acted (see [`Consent`]) and the system
//! has given permission.

use crate::frame::Frame;

/// A picture source.
pub trait VideoSource: Send {
    fn label(&self) -> String;
    fn size(&self) -> (u32, u32);
    /// The picture for time `pts_us` (the latest one the device has when
    /// it is a live device); `None` while the device has nothing yet.
    fn frame_at(&mut self, pts_us: i64) -> Option<Frame>;
}

/// A sound source (mono float samples).
pub trait AudioSource: Send {
    fn label(&self) -> String;
    fn rate(&self) -> u32;
    /// The next `n` samples (silence for what the device has not delivered).
    fn read(&mut self, n: usize) -> Vec<f32>;
}

/// Whether the person has asked for real devices to be used. Nothing
/// outside `test:` sources opens without it: the app sets it from a
/// button the person pressed, and the agent from the person's words.
#[derive(Clone, Copy, Debug, Default)]
pub struct Consent {
    pub user_action: bool,
}

impl Consent {
    pub const GIVEN: Consent = Consent { user_action: true };
}

/// What kind of thing a source id names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Test,
    Camera,
    Mic,
    Screen,
    Window,
}

/// The kind and the rest of an id like `camera:0x1234`.
pub fn parse_id(id: &str) -> Result<(Kind, &str), String> {
    let (k, rest) = id.split_once(':').unwrap_or((id, ""));
    let kind = match k {
        "test" => Kind::Test,
        "camera" => Kind::Camera,
        "mic" => Kind::Mic,
        "screen" => Kind::Screen,
        "window" => Kind::Window,
        _ => return Err(format!("unknown source \"{id}\"; ids look like test:bars, camera:<id>, mic:<id>, screen:<id>, window:<id> (capture devices lists them)")),
    };
    Ok((kind, rest))
}

/// Opens a picture source by id at about `w × h`.
pub fn open_video(id: &str, w: u32, h: u32, fps: f64, consent: Consent) -> Result<Box<dyn VideoSource>, String> {
    let (kind, rest) = parse_id(id)?;
    match kind {
        Kind::Test => Ok(Box::new(TestVideo::from_spec(rest, w, h, fps)?)),
        Kind::Camera | Kind::Screen | Kind::Window => {
            if !consent.user_action {
                return Err(format!("{id} is a real device: it opens only when the person asks for it (their button press in the app, or their word to the agent). Use a test: source otherwise"));
            }
            crate::device::open_video(kind, rest, w, h, fps)
        }
        Kind::Mic => Err(format!("{id} is a sound source")),
    }
}

/// Opens a sound source by id at `rate`.
pub fn open_audio(id: &str, rate: u32, consent: Consent) -> Result<Box<dyn AudioSource>, String> {
    let (kind, rest) = parse_id(id)?;
    match kind {
        Kind::Test => Ok(Box::new(TestAudio::from_spec(rest, rate)?)),
        Kind::Mic => {
            if !consent.user_action {
                return Err(format!("{id} is a real microphone: it opens only when the person asks for it. Use a test: source otherwise"));
            }
            crate::device::open_audio(rest, rate)
        }
        _ => Err(format!("{id} is a picture source")),
    }
}

// ───────────────────────── test pictures ─────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pattern {
    Bars,
    Counter,
    Ramp,
    Colour([u8; 3]),
}

/// A picture made on the spot: colour bars, a counter (the frame number
/// coded in blocks along the top, with a box that moves), a ramp or a
/// flat colour.
pub struct TestVideo {
    pattern: Pattern,
    seed: u8,
    w: u32,
    h: u32,
    fps: f64,
}

/// Number of blocks coding the frame number along the top of `counter`.
pub const COUNTER_BITS: u32 = 16;

impl TestVideo {
    /// `bars`, `counter`, `counter:3` (a seed colouring the background),
    /// `ramp`, `red`, `green`, `blue`, `black`, `white`, `grey`.
    pub fn from_spec(spec: &str, w: u32, h: u32, fps: f64) -> Result<TestVideo, String> {
        let (name, arg) = spec.split_once(':').unwrap_or((spec, ""));
        let seed = arg.parse::<u8>().unwrap_or(0);
        let pattern = match name {
            "bars" | "" => Pattern::Bars,
            "counter" => Pattern::Counter,
            "ramp" => Pattern::Ramp,
            "red" => Pattern::Colour([220, 30, 30]),
            "green" => Pattern::Colour([30, 200, 60]),
            "blue" => Pattern::Colour([30, 60, 220]),
            "black" => Pattern::Colour([0, 0, 0]),
            "white" => Pattern::Colour([255, 255, 255]),
            "grey" | "gray" => Pattern::Colour([128, 128, 128]),
            other => return Err(format!("no test picture \"{other}\"; try bars, counter, ramp, red, green, blue, black, white, grey")),
        };
        Ok(TestVideo { pattern, seed, w: w.max(16), h: h.max(16), fps: fps.max(1.0) })
    }

    /// The frame number a `counter` picture was made for.
    pub fn read_counter(f: &Frame) -> u32 {
        let cell = (f.w / COUNTER_BITS).max(1);
        let mut n = 0;
        for b in 0..COUNTER_BITS {
            let p = f.px(b * cell + cell / 2, 2.min(f.h - 1));
            if p[0] > 128 {
                n |= 1 << b;
            }
        }
        n
    }
}

const BARS: [[u8; 3]; 7] = [[191, 191, 191], [191, 191, 0], [0, 191, 191], [0, 191, 0], [191, 0, 191], [191, 0, 0], [0, 0, 191]];

impl VideoSource for TestVideo {
    fn label(&self) -> String {
        match self.pattern {
            Pattern::Bars => "test bars".into(),
            Pattern::Counter => format!("test counter {}", self.seed),
            Pattern::Ramp => "test ramp".into(),
            Pattern::Colour(_) => "test colour".into(),
        }
    }

    fn size(&self) -> (u32, u32) {
        (self.w, self.h)
    }

    fn frame_at(&mut self, pts_us: i64) -> Option<Frame> {
        let (w, h) = (self.w, self.h);
        let mut f = Frame::new(w, h, [0, 0, 0, 255]);
        f.pts_us = pts_us;
        match self.pattern {
            Pattern::Colour(c) => f.fill_rect(0, 0, w as i32, h as i32, [c[0], c[1], c[2], 255]),
            Pattern::Bars => {
                for x in 0..w {
                    let c = BARS[(x * 7 / w) as usize];
                    for y in 0..h {
                        f.set(x, y, [c[0], c[1], c[2], 255]);
                    }
                }
            }
            Pattern::Ramp => {
                for x in 0..w {
                    let v = (x * 255 / (w - 1)) as u8;
                    for y in 0..h {
                        f.set(x, y, [v, v, v, 255]);
                    }
                }
            }
            Pattern::Counter => {
                let n = (pts_us as f64 * self.fps / 1e6).round().max(0.0) as u32;
                let bg = [(40 + self.seed.wrapping_mul(53) % 120), (40 + self.seed.wrapping_mul(97) % 120), (40 + self.seed.wrapping_mul(29) % 120), 255];
                f.fill_rect(0, 0, w as i32, h as i32, bg);
                let cell = (w / COUNTER_BITS).max(1);
                for b in 0..COUNTER_BITS {
                    let on = n >> b & 1 == 1;
                    let c = if on { [255, 255, 255, 255] } else { [0, 0, 0, 255] };
                    f.fill_rect((b * cell) as i32, 0, cell as i32, (h / 8).max(4) as i32, c);
                }
                // A box sweeping across, so motion is visible.
                let bw = (w / 8).max(4);
                let x = ((n as u64 * 7) % (w - bw) as u64) as i32;
                f.fill_rect(x, (h / 2) as i32, bw as i32, (h / 6).max(2) as i32, [250, 220, 40, 255]);
            }
        }
        Some(f)
    }
}

// ───────────────────────── test sound ─────────────────────────

#[derive(Clone, Copy, Debug)]
enum Wave {
    Sine(f32),
    /// Syllable-like bursts of a voice-ish tone.
    Voice,
    /// Deterministic noise in bursts: unique enough to line up by ear.
    Noise(u32),
    Silence,
}

/// A sound made on the spot.
pub struct TestAudio {
    wave: Wave,
    rate: u32,
    pos: u64,
    rng: u32,
    gain: f32,
}

impl TestAudio {
    /// `tone:440`, `voice`, `noise:7`, `silence`; an optional `@0.3` at
    /// the end sets the level (default 0.5).
    pub fn from_spec(spec: &str, rate: u32) -> Result<TestAudio, String> {
        // `>1.5` at the end starts the sound 1.5 s into itself: a recorder
        // that was switched on later than the camera.
        let (spec, start) = match spec.split_once('>') {
            Some((s, o)) => (s, o.parse::<f64>().map_err(|_| format!("bad start \"{o}\""))?),
            None => (spec, 0.0),
        };
        let (spec, gain) = match spec.split_once('@') {
            Some((s, g)) => (s, g.parse::<f32>().map_err(|_| format!("bad level \"{g}\""))?),
            None => (spec, 0.5),
        };
        let (name, arg) = spec.split_once(':').unwrap_or((spec, ""));
        let wave = match name {
            "tone" | "sine" => Wave::Sine(arg.parse().unwrap_or(440.0)),
            "voice" | "speech" => Wave::Voice,
            "noise" => Wave::Noise(arg.parse().unwrap_or(1)),
            "silence" | "" => Wave::Silence,
            other => return Err(format!("no test sound \"{other}\"; try tone:440, voice, noise:7, silence")),
        };
        let rng = if let Wave::Noise(s) = wave { s.wrapping_mul(2654435761).max(1) } else { 1 };
        let rate = rate.max(8000);
        let mut a = TestAudio { wave, rate, pos: 0, rng, gain: gain.clamp(0.0, 1.0) };
        if start > 0.0 {
            // Run the generator forward so the noise state matches.
            let n = (start * rate as f64) as usize;
            let _ = a.read(n);
        }
        Ok(a)
    }
}

impl AudioSource for TestAudio {
    fn label(&self) -> String {
        match self.wave {
            Wave::Sine(f) => format!("test tone {f} Hz"),
            Wave::Voice => "test voice".into(),
            Wave::Noise(s) => format!("test noise {s}"),
            Wave::Silence => "test silence".into(),
        }
    }

    fn rate(&self) -> u32 {
        self.rate
    }

    fn read(&mut self, n: usize) -> Vec<f32> {
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            let t = self.pos as f32 / self.rate as f32;
            let v = match self.wave {
                Wave::Sine(f) => (t * f * std::f32::consts::TAU).sin(),
                Wave::Voice => {
                    // Syllables at 4 per second, each a soft-edged burst.
                    let ph = (t * 4.0).fract();
                    let env = if ph < 0.7 { (ph / 0.7 * std::f32::consts::PI).sin() } else { 0.0 };
                    let f = 140.0;
                    env * ((t * f * std::f32::consts::TAU).sin() + 0.5 * (t * 2.0 * f * std::f32::consts::TAU).sin() + 0.25 * (t * 3.0 * f * std::f32::consts::TAU).sin()) / 1.75
                }
                Wave::Noise(_) => {
                    self.rng ^= self.rng << 13;
                    self.rng ^= self.rng >> 17;
                    self.rng ^= self.rng << 5;
                    // Bursts of about a third of a second, then a short gap.
                    let burst = ((t * 3.0).fract() < 0.8) as u8 as f32;
                    burst * ((self.rng as f32 / u32::MAX as f32) * 2.0 - 1.0)
                }
                Wave::Silence => 0.0,
            };
            out.push(v * self.gain);
            self.pos += 1;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counter_codes_the_frame_number() {
        let mut v = TestVideo::from_spec("counter:3", 320, 180, 30.0).unwrap();
        for n in [0u32, 1, 29, 300, 4095] {
            let f = v.frame_at((n as f64 / 30.0 * 1e6) as i64).unwrap();
            assert_eq!(TestVideo::read_counter(&f), n);
        }
    }

    #[test]
    fn real_devices_need_the_persons_action() {
        for id in ["camera:abc", "screen:main", "window:1"] {
            let e = open_video(id, 640, 360, 30.0, Consent::default()).err().unwrap();
            assert!(e.contains("real device"), "{e}");
        }
        assert!(open_audio("mic:default", 48000, Consent::default()).err().unwrap().contains("real microphone"));
        assert!(open_video("test:bars", 64, 36, 30.0, Consent::default()).is_ok());
        assert!(open_video("bogus:1", 64, 36, 30.0, Consent::default()).is_err());
    }

    #[test]
    fn a_later_start_is_the_same_sound_further_along() {
        let whole = TestAudio::from_spec("noise:5@0.5", 48000).unwrap().read(48000);
        let later = TestAudio::from_spec("noise:5@0.5>0.25", 48000).unwrap().read(12000);
        assert_eq!(&whole[12000..24000], &later[..]);
    }

    #[test]
    fn tones_have_their_pitch_and_noise_repeats_by_seed() {
        let mut t = TestAudio::from_spec("tone:1000@1", 48000).unwrap();
        let s = t.read(48000);
        let crossings = s.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count();
        assert!((crossings as i32 - 1000).abs() <= 2, "{crossings}");
        let a = TestAudio::from_spec("noise:5", 48000).unwrap().read(4000);
        let b = TestAudio::from_spec("noise:5", 48000).unwrap().read(4000);
        let c = TestAudio::from_spec("noise:6", 48000).unwrap().read(4000);
        assert_eq!(a, b);
        assert_ne!(a, c);
    }
}
