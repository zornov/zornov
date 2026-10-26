// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Fake movies with a ground truth.
//!
//! Every generated frame carries a **burnt-in frame counter**: a band of
//! 16 large blocks along the top, white for a set bit and black for a clear
//! bit, most significant bit on the left. The band survives lossy coding,
//! scaling and colour conversion, so [`read_counter`] recovers the frame
//! index from any decoded picture and a test can say "frame 37 was shown"
//! exactly. Below the band are SMPTE-style 75 % colour bars (pattern
//! [`Pattern::Bars`]) or a disc moving on a known path
//! ([`Pattern::Shapes`]). The audio is a 440 Hz tone with a click every
//! second ([`crate::audio::ticking_tone`]).
//!
//! Files are written with our own `encode` crate (H.264 + AAC in MP4 or
//! MOV). The ground truth ([`MovieTruth`]) is stored next to the fixture.

use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};
use studio_encode::{aac, h264, mux, yuv};

use crate::audio;
use crate::fixtures;
use crate::sha256;

/// Number of counter blocks (bits) in the band.
pub const COUNTER_BITS: usize = 16;

/// Picture content below the counter band.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Pattern {
    /// 75 % colour bars.
    Bars,
    /// A bright disc moving on a Lissajous path over a dark gradient.
    Shapes,
    /// Flat mid grey (cheap to encode; for very large sizes).
    Flat,
}

/// Container to write.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Container {
    Mp4,
    Mov,
}

/// What to generate.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MovieSpec {
    pub width: u32,
    pub height: u32,
    pub fps_num: u32,
    pub fps_den: u32,
    pub frames: u32,
    pub pattern: Pattern,
    pub container: Container,
    /// Audio sample rate and channel count; `None` for a silent movie.
    pub audio: Option<(u32, u16)>,
    /// Display rotation written into the track matrix (0, 90, 180, 270).
    pub rotation: u16,
    /// H.264 profile.
    pub main_profile: bool,
    /// IDR interval in frames (0 = first frame only).
    pub gop: u32,
    /// Target bit rate in kbit/s (0 = encoder's recommendation).
    pub kbps: u32,
}

impl MovieSpec {
    /// 320×180 at 30 fps, 2 s, bars, MP4 with stereo 48 kHz audio.
    pub fn small() -> MovieSpec {
        MovieSpec { width: 320, height: 180, fps_num: 30, fps_den: 1, frames: 60, pattern: Pattern::Bars, container: Container::Mp4, audio: Some((48_000, 2)), rotation: 0, main_profile: true, gop: 30, kbps: 0 }
    }

    pub fn size(mut self, w: u32, h: u32) -> MovieSpec {
        self.width = w;
        self.height = h;
        self
    }
    pub fn fps(mut self, num: u32, den: u32) -> MovieSpec {
        self.fps_num = num;
        self.fps_den = den;
        self
    }
    pub fn frames(mut self, n: u32) -> MovieSpec {
        self.frames = n;
        self
    }
    pub fn pattern(mut self, p: Pattern) -> MovieSpec {
        self.pattern = p;
        self
    }
    pub fn container(mut self, c: Container) -> MovieSpec {
        self.container = c;
        self
    }
    pub fn silent(mut self) -> MovieSpec {
        self.audio = None;
        self
    }
    pub fn audio(mut self, rate: u32, channels: u16) -> MovieSpec {
        self.audio = Some((rate, channels));
        self
    }
    pub fn rotation(mut self, deg: u16) -> MovieSpec {
        self.rotation = deg;
        self
    }
    pub fn baseline(mut self) -> MovieSpec {
        self.main_profile = false;
        self
    }
    pub fn gop(mut self, g: u32) -> MovieSpec {
        self.gop = g;
        self
    }
    pub fn kbps(mut self, k: u32) -> MovieSpec {
        self.kbps = k;
        self
    }

    pub fn fps_f64(&self) -> f64 {
        self.fps_num as f64 / self.fps_den.max(1) as f64
    }

    /// Duration in microseconds (frames × exact frame duration).
    pub fn duration_us(&self) -> i64 {
        ((self.frames as f64 * self.fps_den as f64 * 1e6) / self.fps_num as f64).round() as i64
    }

    /// A file name that encodes the spec, for the fixture store.
    pub fn file_name(&self) -> String {
        let ext = match self.container {
            Container::Mp4 => "mp4",
            Container::Mov => "mov",
        };
        let audio = match self.audio {
            Some((r, c)) => format!("a{r}x{c}"),
            None => "silent".into(),
        };
        let pat = match self.pattern {
            Pattern::Bars => "bars",
            Pattern::Shapes => "shapes",
            Pattern::Flat => "flat",
        };
        format!(
            "movie_{}x{}_{}-{}fps_{}f_{pat}_{audio}_rot{}_{}_gop{}_{}k.{ext}",
            self.width,
            self.height,
            self.fps_num,
            self.fps_den,
            self.frames,
            self.rotation,
            if self.main_profile { "main" } else { "base" },
            self.gop,
            self.kbps
        )
    }
}

/// Ground truth of one frame.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FrameTruth {
    pub index: u32,
    /// Presentation time in microseconds.
    pub pts_us: i64,
    /// SHA-256 of the RGBA source picture before encoding.
    pub source_sha256: String,
    /// Mean linear-ish RGB (0..255) of the whole frame.
    pub mean_rgb: [f32; 3],
    /// Disc centre for [`Pattern::Shapes`] (pixels).
    pub disc: Option<[f32; 2]>,
}

/// Ground truth of the audio track.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AudioTruth {
    pub rate: u32,
    pub channels: u16,
    pub frames: u64,
    pub rms_per_second: Vec<f32>,
    /// Sample positions of the clicks.
    pub click_frames: Vec<u64>,
    pub tone_hz: f64,
    pub tone_db: f32,
}

/// Ground truth of a generated movie.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MovieTruth {
    pub spec: MovieSpec,
    pub duration_us: i64,
    pub frames: Vec<FrameTruth>,
    pub audio: Option<AudioTruth>,
}

/// Height of the counter band in pixels.
pub fn band_height(height: u32) -> u32 {
    (height / 6).max(2)
}

/// Draws frame `index` of `spec` as straight-alpha RGBA8.
pub fn draw_frame(spec: &MovieSpec, index: u32) -> Vec<u8> {
    let (w, h) = (spec.width as usize, spec.height as usize);
    let mut px = vec![0u8; w * h * 4];
    let band = band_height(spec.height) as usize;
    let disc = disc_centre(spec, index);
    for y in 0..h {
        for x in 0..w {
            let rgb = if y < band {
                let bit = x * COUNTER_BITS / w.max(1);
                let set = (index >> (COUNTER_BITS - 1 - bit)) & 1 == 1;
                // A thin dark separator between blocks helps a human read it.
                let edge = (x * COUNTER_BITS) % w.max(1) < COUNTER_BITS;
                if edge { [32, 32, 32] } else if set { [235, 235, 235] } else { [16, 16, 16] }
            } else {
                match spec.pattern {
                    Pattern::Bars => bar_colour(x * 8 / w.max(1)),
                    Pattern::Flat => [118, 118, 118],
                    Pattern::Shapes => {
                        let (cx, cy) = (disc[0], disc[1]);
                        let r = (h as f32 * 0.12).max(2.0);
                        let d = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
                        if d < r {
                            [240, 200, 60]
                        } else {
                            let g = 40 + (y * 60 / h.max(1)) as u8;
                            [g / 2, g, g + 20]
                        }
                    }
                }
            };
            let i = (y * w + x) * 4;
            px[i..i + 3].copy_from_slice(&rgb);
            px[i + 3] = 255;
        }
    }
    px
}

/// Centre of the moving disc for [`Pattern::Shapes`].
pub fn disc_centre(spec: &MovieSpec, index: u32) -> [f32; 2] {
    let (w, h) = (spec.width as f32, spec.height as f32);
    let band = band_height(spec.height) as f32;
    let t = index as f32 / spec.frames.max(1) as f32 * std::f32::consts::TAU;
    let cx = w * 0.5 + (w * 0.3) * t.sin();
    let cy = band + (h - band) * 0.5 + ((h - band) * 0.3) * (2.0 * t).cos();
    [cx, cy]
}

/// 75 % colour bars (left to right): grey, yellow, cyan, green, magenta, red, blue, black.
pub fn bar_colour(bar: usize) -> [u8; 3] {
    const C: u8 = 191;
    match bar {
        0 => [C, C, C],
        1 => [C, C, 0],
        2 => [0, C, C],
        3 => [0, C, 0],
        4 => [C, 0, C],
        5 => [C, 0, 0],
        6 => [0, 0, C],
        _ => [16, 16, 16],
    }
}

/// Reads the frame counter from a decoded RGBA8 picture of any size, by
/// sampling the centre of each block in the band and thresholding luma.
pub fn read_counter(rgba: &[u8], width: u32, height: u32) -> u32 {
    read_counter_with(width, height, |x, y| {
        let i = ((y * width + x) * 4) as usize;
        [rgba[i], rgba[i + 1], rgba[i + 2]]
    })
}

/// Same as [`read_counter`] over a pixel accessor (for planar pictures).
pub fn read_counter_with(width: u32, height: u32, rgb_at: impl Fn(u32, u32) -> [u8; 3]) -> u32 {
    let band = band_height(height);
    let y0 = band / 4;
    let y1 = band * 3 / 4;
    let mut value = 0u32;
    for bit in 0..COUNTER_BITS as u32 {
        // Average a few samples in the middle of the block.
        let x0 = (bit * width) / COUNTER_BITS as u32 + width / (COUNTER_BITS as u32 * 4);
        let x1 = ((bit + 1) * width) / COUNTER_BITS as u32 - width / (COUNTER_BITS as u32 * 4);
        let mut sum = 0u64;
        let mut n = 0u64;
        for y in y0..=y1.max(y0) {
            for x in x0..=x1.max(x0) {
                let p = rgb_at(x.min(width - 1), y.min(height - 1));
                sum += (p[0] as u64 * 54 + p[1] as u64 * 183 + p[2] as u64 * 19) >> 8;
                n += 1;
            }
        }
        let luma = sum / n.max(1);
        if luma > 120 {
            value |= 1 << (COUNTER_BITS as u32 - 1 - bit);
        }
    }
    value
}

/// Reads the counter from an I420 picture.
pub fn read_counter_i420(y: &[u8], width: u32, height: u32) -> u32 {
    read_counter_with(width, height, |x, yy| {
        let l = y[(yy * width + x) as usize];
        [l, l, l]
    })
}

fn mean_rgb(rgba: &[u8]) -> [f32; 3] {
    let n = (rgba.len() / 4).max(1) as f64;
    let mut s = [0f64; 3];
    for p in rgba.chunks_exact(4) {
        for c in 0..3 {
            s[c] += p[c] as f64;
        }
    }
    [(s[0] / n) as f32, (s[1] / n) as f32, (s[2] / n) as f32]
}

/// Mono audio of the movie (the ticking tone).
pub fn movie_audio_mono(spec: &MovieSpec) -> Option<(Vec<f32>, u32, u16)> {
    let (rate, channels) = spec.audio?;
    let secs = spec.duration_us() as f64 / 1e6;
    Some((audio::ticking_tone(rate, secs, -12.0), rate, channels))
}

/// Writes the movie described by `spec` to `path` (and its truth next to
/// it); returns the truth.
pub fn write_movie(spec: &MovieSpec, path: &Path) -> io::Result<MovieTruth> {
    let mut cfg = h264::Config::new(spec.width, spec.height, spec.fps_num, spec.fps_den);
    cfg.profile = if spec.main_profile { h264::Profile::Main } else { h264::Profile::Baseline };
    cfg.gop = spec.gop;
    // One slice: the output must not depend on the core count.
    cfg.max_slices = 1;
    if spec.kbps > 0 {
        cfg.bitrate_kbps = spec.kbps;
    }
    let mut venc = h264::Encoder::new(cfg).map_err(io::Error::other)?;
    let opts = match spec.container {
        Container::Mp4 => mux::Options::mp4(),
        Container::Mov => mux::Options::mov(),
    };
    let mut muxer = mux::Muxer::create(path, opts)?;
    let v = muxer.add_video(venc.track_info())?;
    let mut audio_track = None;
    let mut aenc = None;
    let mut audio_truth = None;
    let mut audio_samples = Vec::new();
    if let Some((mono, rate, channels)) = movie_audio_mono(spec) {
        let enc = aac::Encoder::new(aac::Config { sample_rate: rate, channels, bitrate: 96_000 * channels as u32 }).map_err(io::Error::other)?;
        audio_track = Some(muxer.add_audio(enc.track_info())?);
        aenc = Some(enc);
        audio_truth = Some(AudioTruth {
            rate,
            channels,
            frames: mono.len() as u64,
            rms_per_second: audio::rms_per_second(&mono, rate),
            click_frames: audio::find_clicks(&mono, 0.9, rate as usize / 2).into_iter().map(|c| c as u64).collect(),
            tone_hz: 440.0,
            tone_db: -12.0,
        });
        audio_samples = audio::interleave(&mono, channels as usize);
    }
    let mut frames = Vec::with_capacity(spec.frames as usize);
    let mut audio_pos = 0usize;
    let ch = spec.audio.map(|(_, c)| c as usize).unwrap_or(0);
    for i in 0..spec.frames {
        let rgba = draw_frame(spec, i);
        let pic = yuv::Yuv420::from_rgba(&rgba, spec.width, spec.height);
        let f = venc.encode(&pic);
        muxer.write_sample(v, &f.data, f.keyframe)?;
        frames.push(FrameTruth {
            index: i,
            pts_us: ((i as f64 * spec.fps_den as f64 * 1e6) / spec.fps_num as f64).round() as i64,
            source_sha256: sha256::hex_digest(&rgba),
            mean_rgb: mean_rgb(&rgba),
            disc: (spec.pattern == Pattern::Shapes).then(|| disc_centre(spec, i)),
        });
        // Interleave audio roughly a frame at a time, as an export would.
        if let (Some(enc), Some(t)) = (aenc.as_mut(), audio_track) {
            let want = ((i as u64 + 1) * audio_samples.len() as u64 / spec.frames.max(1) as u64) as usize;
            let want = want - want % ch.max(1);
            for au in enc.push(&audio_samples[audio_pos..want]) {
                muxer.write_sample(t, &au, true)?;
            }
            audio_pos = want;
        }
    }
    if let (Some(enc), Some(t)) = (aenc.as_mut(), audio_track) {
        for au in enc.push(&audio_samples[audio_pos..]) {
            muxer.write_sample(t, &au, true)?;
        }
        for au in enc.flush() {
            muxer.write_sample(t, &au, true)?;
        }
    }
    muxer.finish()?;
    // The muxer stamps the wall clock into the headers; fixtures must be
    // byte-identical across runs, so the stamps are zeroed.
    let mut bytes = std::fs::read(path)?;
    zero_timestamps(&mut bytes);
    if spec.rotation != 0 {
        patch_rotation(&mut bytes, spec.rotation).map_err(io::Error::other)?;
    }
    std::fs::write(path, bytes)?;
    let truth = MovieTruth { spec: spec.clone(), duration_us: spec.duration_us(), frames, audio: audio_truth };
    fixtures::write_truth(path, &truth)?;
    Ok(truth)
}

/// Rewrites the first video `tkhd` matrix in an MP4/MOV file to a display
/// rotation (clockwise degrees: 90, 180, 270). The muxer always writes the
/// identity; cameras write these matrices, so decoders must honour them.
pub fn patch_rotation(bytes: &mut [u8], degrees: u16) -> Result<(), String> {
    let (a, b, c, d): (i32, i32, i32, i32) = match degrees % 360 {
        0 => (1 << 16, 0, 0, 1 << 16),
        90 => (0, 1 << 16, -(1 << 16), 0),
        180 => (-(1 << 16), 0, 0, -(1 << 16)),
        270 => (0, -(1 << 16), 1 << 16, 0),
        other => return Err(format!("rotation {other} is not a multiple of 90")),
    };
    let pos = find_video_tkhd(bytes).ok_or("no video tkhd box")?;
    let version = bytes[pos + 8];
    let matrix_at = pos + 8 + 4 + if version == 1 { 48 } else { 36 };
    let m: [i32; 9] = [a, b, 0, c, d, 0, 0, 0, 0x4000_0000];
    for (i, v) in m.iter().enumerate() {
        bytes[matrix_at + i * 4..matrix_at + i * 4 + 4].copy_from_slice(&v.to_be_bytes());
    }
    Ok(())
}

/// Zeroes the creation and modification times of every `mvhd`, `tkhd` and
/// `mdhd` box (version 0 and 1).
pub fn zero_timestamps(bytes: &mut [u8]) {
    fn walk(bytes: &mut [u8], start: usize, end: usize) {
        let mut pos = start;
        while pos + 8 <= end {
            let size = u32::from_be_bytes(bytes[pos..pos + 4].try_into().unwrap()) as usize;
            let size = if size == 0 { end - pos } else { size.max(8) };
            if pos + size > end {
                return;
            }
            let kind: [u8; 4] = bytes[pos + 4..pos + 8].try_into().unwrap();
            match &kind {
                b"moov" | b"trak" | b"mdia" => walk(bytes, pos + 8, pos + size),
                b"mvhd" | b"tkhd" | b"mdhd" => {
                    let version = bytes[pos + 8];
                    let n = if version == 1 { 16 } else { 8 };
                    let at = pos + 12;
                    if at + n <= end {
                        bytes[at..at + n].fill(0);
                    }
                }
                _ => {}
            }
            pos += size;
        }
    }
    let len = bytes.len();
    walk(bytes, 0, len);
}

/// Finds the `tkhd` of the first track whose `tkhd` has a non-zero width
/// (the video track). Returns the box start offset.
fn find_video_tkhd(bytes: &[u8]) -> Option<usize> {
    fn walk(bytes: &[u8], start: usize, end: usize, out: &mut Vec<usize>) {
        let mut pos = start;
        while pos + 8 <= end {
            let size = u32::from_be_bytes(bytes[pos..pos + 4].try_into().unwrap()) as usize;
            let kind = &bytes[pos + 4..pos + 8];
            let size = if size == 0 { end - pos } else { size.max(8) };
            if pos + size > end {
                return;
            }
            match kind {
                b"moov" | b"trak" => walk(bytes, pos + 8, pos + size, out),
                b"tkhd" => out.push(pos),
                _ => {}
            }
            pos += size;
        }
    }
    let mut found = Vec::new();
    walk(bytes, 0, bytes.len(), &mut found);
    found.into_iter().find(|&pos| {
        let version = bytes[pos + 8];
        let width_at = pos + 8 + 4 + if version == 1 { 48 } else { 36 } + 36;
        width_at + 4 <= bytes.len() && u32::from_be_bytes(bytes[width_at..width_at + 4].try_into().unwrap()) != 0
    })
}

/// The fixture for `spec`, generated on first use.
pub fn movie(spec: &MovieSpec) -> std::path::PathBuf {
    fixtures::fixture(&spec.file_name(), |p| write_movie(spec, p).map(|_| ()))
}

/// Ground truth of a movie fixture made with [`movie`].
pub fn truth(path: &Path) -> MovieTruth {
    fixtures::read_truth(path)
}

/// The standard fixtures every suite can rely on.
pub mod standard {
    use super::*;

    /// 320×180 30 fps 2 s bars with stereo audio, MP4.
    pub fn small() -> std::path::PathBuf {
        movie(&MovieSpec::small())
    }
    /// 320×180 silent MP4.
    pub fn silent() -> std::path::PathBuf {
        movie(&MovieSpec::small().silent())
    }
    /// Odd dimensions (321×181) silent MP4.
    pub fn odd_size() -> std::path::PathBuf {
        movie(&MovieSpec::small().size(321, 181).silent().frames(20))
    }
    /// Tiny 16×16 picture.
    pub fn tiny() -> std::path::PathBuf {
        movie(&MovieSpec::small().size(16, 16).silent().frames(20))
    }
    /// 29.97 fps (30000/1001) bars, 90 frames.
    pub fn drop_frame() -> std::path::PathBuf {
        movie(&MovieSpec::small().fps(30_000, 1001).frames(90).silent())
    }
    /// Rotated 90° MOV.
    pub fn rotated_90() -> std::path::PathBuf {
        movie(&MovieSpec::small().container(Container::Mov).rotation(90).silent().frames(20))
    }
    /// Moving disc, 60 fps.
    pub fn shapes_60() -> std::path::PathBuf {
        movie(&MovieSpec::small().pattern(Pattern::Shapes).fps(60, 1).frames(120).silent())
    }
    /// Baseline profile (CAVLC), GOP 10.
    pub fn baseline() -> std::path::PathBuf {
        movie(&MovieSpec::small().baseline().gop(10).silent().frames(30))
    }
    /// Mono 44.1 kHz audio.
    pub fn mono_44k() -> std::path::PathBuf {
        movie(&MovieSpec::small().audio(44_100, 1).frames(30))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counter_round_trips_through_drawing() {
        let spec = MovieSpec::small().size(96, 54);
        for i in [0u32, 1, 2, 255, 1234, 65535] {
            let px = draw_frame(&spec, i);
            assert_eq!(read_counter(&px, 96, 54), i);
        }
    }

    #[test]
    fn counter_survives_half_size_and_blur() {
        let spec = MovieSpec::small().size(160, 90);
        let px = draw_frame(&spec, 0xA5C3);
        // Box downscale by 2.
        let (w, h) = (80u32, 45u32);
        let mut small = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                for c in 0..4 {
                    let mut s = 0u32;
                    for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                        s += px[(((y * 2 + dy) * 160 + x * 2 + dx) * 4 + c) as usize] as u32;
                    }
                    small[((y * w + x) * 4 + c) as usize] = (s / 4) as u8;
                }
            }
        }
        assert_eq!(read_counter(&small, w, h), 0xA5C3);
    }

    #[test]
    fn rotation_patch_targets_the_video_track() {
        // A fake tkhd pair: audio first (zero width), then video.
        let mut bytes = Vec::new();
        let tkhd = |width: u32| {
            let mut b = vec![0u8; 92];
            b[0..4].copy_from_slice(&92u32.to_be_bytes());
            b[4..8].copy_from_slice(b"tkhd");
            b[84..88].copy_from_slice(&(width << 16).to_be_bytes());
            b
        };
        let mut moov = Vec::new();
        for w in [0u32, 320] {
            let t = tkhd(w);
            let mut trak = vec![0u8; 8];
            trak[0..4].copy_from_slice(&((t.len() + 8) as u32).to_be_bytes());
            trak[4..8].copy_from_slice(b"trak");
            trak.extend(t);
            moov.extend(trak);
        }
        let mut hdr = vec![0u8; 8];
        hdr[0..4].copy_from_slice(&((moov.len() + 8) as u32).to_be_bytes());
        hdr[4..8].copy_from_slice(b"moov");
        bytes.extend(hdr);
        bytes.extend(moov);
        patch_rotation(&mut bytes, 90).unwrap();
        let video = find_video_tkhd(&bytes).unwrap();
        let m = &bytes[video + 48..video + 48 + 36];
        let read = |i: usize| i32::from_be_bytes(m[i * 4..i * 4 + 4].try_into().unwrap());
        let mat = [read(0), read(1), read(2), read(3), read(4), read(5), read(6), read(7), read(8)];
        assert_eq!(studio_mp4::matrix_rotation(&mat), (90, false));
    }
}
