// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Real media files: probing on import and decoding frames for thumbnails
//! and the viewer, with Studio's own `mp4`, `h264`, `hevc` and `media`
//! crates. This is the simple path used until `render` takes over
//! playback: one decoder per file, kept warm on a worker thread so frames
//! played in order decode forwards without seeking again.

use eframe::egui::ColorImage;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use studio_model as m;

/// What import learns about a file.
pub struct Probe {
    pub kind: m::MediaKind,
    pub duration: Option<m::Micros>,
    pub video: Option<m::VideoInfo>,
    pub audio: Option<m::AudioInfo>,
    pub container: String,
}

fn ext(path: &Path) -> String {
    path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase()
}

/// Lottie animations (`.json`, `.lottie`): vector motion graphics with
/// transparency, played by `studio-lottie`. Read once per file and kept.
pub const LOTTIE_EXT: &[&str] = &["json", "lottie"];

/// The animation in `path`, when it is one.
pub fn lottie(path: &Path) -> Option<std::sync::Arc<studio_lottie::Animation>> {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex, OnceLock};
    if !LOTTIE_EXT.contains(&ext(path).as_str()) {
        return None;
    }
    static CACHE: OnceLock<Mutex<HashMap<std::path::PathBuf, Arc<studio_lottie::Animation>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(a) = cache.lock().unwrap_or_else(|e| e.into_inner()).get(path) {
        return Some(a.clone());
    }
    let bytes = std::fs::read(path).ok()?;
    if !studio_lottie::sniff(&bytes) {
        return None;
    }
    let a = Arc::new(studio_lottie::Animation::from_bytes(&bytes, path.parent()).ok()?);
    let mut c = cache.lock().unwrap_or_else(|e| e.into_inner());
    if c.len() > 32 {
        c.clear();
    }
    c.insert(path.to_path_buf(), a.clone());
    Some(a)
}

pub const VIDEO_EXT: &[&str] = &["mp4", "mov", "m4v"];
pub const AUDIO_EXT: &[&str] = &["m4a", "wav", "mp3", "aac", "flac", "ogg", "aif", "aiff"];
pub const IMAGE_EXT: &[&str] = &["png", "jpg", "jpeg"];

pub fn probe(path: &Path) -> Result<Probe, String> {
    // ProRes, EXR / DPX / TIFF / 16-bit PNG and frame sequences.
    if let Some(pr) = crate::services::media_meta::probe(path) {
        return pr;
    }
    let e = ext(path);
    if let Some(anim) = lottie(path) {
        let (w, h) = anim.picture_size();
        return Ok(Probe {
            kind: m::MediaKind::Video,
            duration: Some((anim.duration() * 1e6).round() as m::Micros),
            video: Some(m::VideoInfo { codec: "lottie".into(), width: w, height: h, frame_rate: Some({ let fps = anim.fps; if (fps - fps.round()).abs() < 0.02 { m::FrameRate { num: fps.round() as u32, den: 1 } } else { m::FrameRate { num: (fps * 1001.0).round() as u32, den: 1001 } } }), rotation: 0, has_alpha: true, colour: Default::default() }),
            audio: None,
            container: "lottie".into(),
        });
    }
    if IMAGE_EXT.contains(&e.as_str()) {
        let img = read_image(path)?;
        return Ok(Probe {
            kind: m::MediaKind::Image,
            duration: None,
            video: Some(m::VideoInfo { codec: e.clone(), width: img.size[0] as u32, height: img.size[1] as u32, frame_rate: None, rotation: 0, has_alpha: true, colour: Default::default() }),
            audio: None,
            container: e,
        });
    }
    if matches!(e.as_str(), "wav" | "wave" | "mp3" | "ogg" | "oga" | "flac" | "aif" | "aiff" | "aifc") {
        // Decoded once and cached (playback and export reuse it), so the
        // probe reports the true length, rate and channel count.
        let path_str = path.to_str().ok_or("The file name is not valid UTF-8")?;
        let d = crate::sound_bridge::decoded(path_str).ok_or_else(|| format!("Could not read the sound in {}", path.display()))?;
        let frames = d.left.len() as f64;
        let channels = if d.mono { 1 } else { 2 };
        return Ok(Probe {
            kind: m::MediaKind::Audio,
            duration: Some((frames / d.rate as f64 * 1e6).round() as m::Micros),
            video: None,
            audio: Some(m::AudioInfo { codec: if e == "mp3" { "mp3".into() } else if e.starts_with("og") { "vorbis".into() } else if e == "flac" { "flac".into() } else { "pcm".into() }, sample_rate: d.rate, channels, layout: if channels == 1 { "mono".into() } else { "stereo".into() } }),
            container: e,
        });
    }
    let dm = studio_mp4::open(path).map_err(|err| format!("{err}"))?;
    let movie = dm.movie();
    let secs = movie.duration_secs;
    let mut video = None;
    let mut audio = None;
    for t in &movie.tracks {
        match t.kind {
            studio_mp4::demux::TrackKind::Video if video.is_none() => {
                let codec = match &t.codec {
                    studio_mp4::demux::Codec::Avc { .. } => "h264",
                    studio_mp4::demux::Codec::Hevc { .. } => "hevc",
                    _ => "other",
                };
                let fps = if t.duration_secs > 0.0 { t.samples.len() as f64 / t.duration_secs } else { 30.0 };
                let rate = if (fps - fps.round()).abs() < 0.02 { m::FrameRate { num: fps.round() as u32, den: 1 } } else { m::FrameRate { num: (fps * 1001.0).round() as u32, den: 1001 } };
                video = Some(m::VideoInfo { codec: codec.into(), width: t.width, height: t.height, frame_rate: Some(rate), rotation: t.rotation as u32, has_alpha: false, colour: Default::default() });
            }
            studio_mp4::demux::TrackKind::Audio if audio.is_none() => {
                audio = Some(m::AudioInfo { codec: "aac".into(), sample_rate: t.sample_rate, channels: t.channels, layout: if t.channels == 1 { "mono".into() } else { "stereo".into() } });
            }
            _ => {}
        }
    }
    let kind = if video.is_some() { m::MediaKind::Video } else { m::MediaKind::Audio };
    Ok(Probe { kind, duration: Some((secs * 1e6) as m::Micros), video, audio, container: e })
}

pub fn read_image(path: &Path) -> Result<ColorImage, String> {
    let img = match ext(path).as_str() {
        "png" => studio_media::png::read(path)?,
        _ => studio_media::jpeg::read(path)?,
    };
    Ok(ColorImage::from_rgba_unmultiplied([img.width as usize, img.height as usize], &img.pixels))
}

enum Dec {
    Avc(studio_h264::Decoder),
    Hevc(studio_hevc::Decoder),
}

/// One open file with a decoder positioned somewhere in its video track.
struct Session {
    demuxer: studio_mp4::Demuxer<std::io::BufReader<std::fs::File>>,
    track: usize,
    dec: Dec,
    /// Index of the next sample to feed, in decode order.
    next: usize,
    /// Last decoded picture and its presentation time in seconds.
    last: Option<(f64, ColorImage)>,
    /// Presentation times of the samples fed since the decoder started and
    /// not yet out, smallest first: pictures come out in presentation order,
    /// so each one takes the smallest.
    pending: std::collections::BTreeSet<i64>,
}

impl Session {
    fn open(path: &Path) -> Result<Self, String> {
        let demuxer = studio_mp4::open(path).map_err(|e| format!("{e}"))?;
        let (track, t) = demuxer
            .movie()
            .tracks
            .iter()
            .enumerate()
            .find(|(_, t)| t.kind == studio_mp4::demux::TrackKind::Video)
            .ok_or("no video track")?;
        let dec = match &t.codec {
            studio_mp4::demux::Codec::Avc { config } => {
                let sps: Vec<&[u8]> = config.sps.iter().map(|s| s.as_slice()).collect();
                let pps: Vec<&[u8]> = config.pps.iter().map(|s| s.as_slice()).collect();
                Dec::Avc(studio_h264::Decoder::new(&sps, &pps, config.nal_length_size as usize).map_err(|e| format!("{e:?}"))?)
            }
            studio_mp4::demux::Codec::Hevc { config } => {
                let sets: Vec<&[u8]> = config.vps().into_iter().chain(config.sps()).chain(config.pps()).collect();
                Dec::Hevc(studio_hevc::Decoder::new(&sets, config.nal_length_size as usize).map_err(|e| format!("{e:?}"))?)
            }
            _ => return Err("unsupported video codec".into()),
        };
        Ok(Self { demuxer, track, dec, next: usize::MAX, last: None, pending: Default::default() })
    }

    fn decode_sample(&mut self, i: usize) -> Result<Vec<(f64, ColorImage)>, String> {
        let t = &self.demuxer.movie().tracks[self.track];
        let sample = t.samples[i];
        let data = self.demuxer.read_sample(&sample).map_err(|e| format!("{e}"))?;
        self.pending.insert(sample.pts);
        let frames: Vec<ColorImage> = match &mut self.dec {
            Dec::Avc(d) => d.decode(&data).map_err(|e| format!("{e:?}"))?.iter().map(|f| ColorImage::from_rgba_unmultiplied([f.width as usize, f.height as usize], &studio_h264::to_rgba(f))).collect(),
            Dec::Hevc(d) => d.decode(&data).map_err(|e| format!("{e:?}"))?.iter().map(|f| ColorImage::from_rgba_unmultiplied([f.width, f.height], &f.to_rgba())).collect(),
        };
        Ok(self.timed(frames))
    }

    /// Pictures (out in presentation order) with their own presentation
    /// times. (Not the time of the sample that released them: with B-frames
    /// that is a later frame's, and the wrong picture was shown.)
    fn timed(&mut self, frames: Vec<ColorImage>) -> Vec<(f64, ColorImage)> {
        let t = &self.demuxer.movie().tracks[self.track];
        let scale = t.timescale.max(1) as f64;
        frames
            .into_iter()
            .map(|f| {
                let pts = self.pending.pop_first().unwrap_or(i64::MAX);
                let probe = studio_mp4::demux::Sample { pts, ..t.samples[0] };
                // Before the edit list starts: earlier than anything shown.
                let secs = if pts == i64::MAX { f64::MAX } else { t.presentation_secs(&probe).unwrap_or(-1.0 - pts as f64 / scale) };
                (secs, f)
            })
            .collect()
    }

    /// The picture shown at `secs`. Decodes forwards from the current
    /// position when it can, otherwise from the nearest sync sample.
    fn frame_at(&mut self, secs: f64, keyframe_only: bool) -> Result<ColorImage, String> {
        let track = self.demuxer.movie().tracks[self.track].clone();
        let seek = track.sample_for_time(secs).ok_or("empty track")?;
        if keyframe_only {
            self.reset()?;
            let mut out = self.decode_sample(seek.sync_index)?;
            if out.is_empty() {
                out = self.flush();
            }
            self.next = usize::MAX;
            return out.pop().map(|(_, f)| f).ok_or_else(|| "no picture".into());
        }
        let want = track.presentation_secs(&track.samples[seek.target_index]).unwrap_or(secs);
        if let Some((t, f)) = &self.last {
            if *t == want {
                return Ok(f.clone());
            }
        }
        let forward = self.next != usize::MAX && seek.sync_index < self.next && self.last.as_ref().is_none_or(|(t, _)| *t < want) && seek.target_index < self.next + 90;
        if !forward {
            self.reset()?;
            self.next = seek.sync_index;
        }
        // Feed until the target picture is out (B-frames hold it back
        // behind later samples), flushing at the end of the stream.
        let mut got = false;
        while !got {
            let out = if self.next < track.samples.len() {
                let out = self.decode_sample(self.next)?;
                self.next += 1;
                out
            } else {
                let out = self.flush();
                self.next = usize::MAX;
                if out.is_empty() {
                    break;
                }
                out
            };
            for (t, f) in out {
                if t <= want + 1e-9 || self.last.is_none() {
                    got |= t >= want - 1e-9;
                    self.last = Some((t, f));
                }
            }
        }
        self.last.as_ref().map(|(_, f)| f.clone()).ok_or_else(|| "no picture".into())
    }

    fn flush(&mut self) -> Vec<(f64, ColorImage)> {
        let frames = match &mut self.dec {
            Dec::Avc(d) => d.flush().iter().map(|f| ColorImage::from_rgba_unmultiplied([f.width as usize, f.height as usize], &studio_h264::to_rgba(f))).collect(),
            Dec::Hevc(d) => d.flush().iter().map(|f| ColorImage::from_rgba_unmultiplied([f.width, f.height], &f.to_rgba())).collect(),
        };
        self.timed(frames)
    }

    fn reset(&mut self) -> Result<(), String> {
        // A fresh decoder is the simplest correct seek.
        let path_free = Session::open_like(self)?;
        self.dec = path_free;
        self.last = None;
        self.pending.clear();
        Ok(())
    }

    fn open_like(s: &Session) -> Result<Dec, String> {
        let t = &s.demuxer.movie().tracks[s.track];
        Ok(match &t.codec {
            studio_mp4::demux::Codec::Avc { config } => {
                let sps: Vec<&[u8]> = config.sps.iter().map(|x| x.as_slice()).collect();
                let pps: Vec<&[u8]> = config.pps.iter().map(|x| x.as_slice()).collect();
                Dec::Avc(studio_h264::Decoder::new(&sps, &pps, config.nal_length_size as usize).map_err(|e| format!("{e:?}"))?)
            }
            studio_mp4::demux::Codec::Hevc { config } => {
                let sets: Vec<&[u8]> = config.vps().into_iter().chain(config.sps()).chain(config.pps()).collect();
                Dec::Hevc(studio_hevc::Decoder::new(&sets, config.nal_length_size as usize).map_err(|e| format!("{e:?}"))?)
            }
            _ => return Err("unsupported video codec".into()),
        })
    }
}

thread_local! {
    static SESSIONS: std::cell::RefCell<HashMap<(PathBuf, bool), Session>> = std::cell::RefCell::new(HashMap::new());
}

/// Decode the picture at `secs`, scaled to fit `w × h`. Thumbnails ask for
/// `keyframe_only`, which is much cheaper.
pub fn frame(path: &Path, secs: f64, w: usize, h: usize, keyframe_only: bool) -> Result<ColorImage, String> {
    let e = ext(path);
    #[cfg(feature = "render")]
    if let Some(img) = crate::services::media_meta::frame(path, secs) {
        return Ok(fit(&img?, w, h));
    }
    let full = if IMAGE_EXT.contains(&e.as_str()) {
        read_image(path)?
    } else if let Some(anim) = lottie(path) {
        let (lw, lh) = anim.picture_size();
        let f = anim.in_frame + (secs.max(0.0) * anim.fps).floor().min(anim.frame_count() as f64 - 1.0);
        ColorImage::from_rgba_unmultiplied([lw as usize, lh as usize], &anim.render_rgba(f, lw, lh))
    } else {
        SESSIONS.with(|s| {
            let mut s = s.borrow_mut();
            let key = (path.to_path_buf(), keyframe_only);
            if !s.contains_key(&key) {
                if s.len() > 8 {
                    s.clear();
                }
                s.insert(key.clone(), Session::open(path)?);
            }
            let session = s.get_mut(&key).unwrap();
            let t = &session.demuxer.movie().tracks[session.track];
            let (rotation, flip) = (t.rotation, t.flip);
            // Upright, as it is shown (phone footage is stored sideways).
            session.frame_at(secs, keyframe_only).map(|img| upright(img, rotation, flip))
        })?
    };
    Ok(fit(&full, w, h))
}

/// `img` mirrored (when `flip`), then turned `rotation` degrees clockwise.
fn upright(img: ColorImage, rotation: u16, flip: bool) -> ColorImage {
    let [w, h] = img.size;
    if (rotation % 360 == 0 && !flip) || w == 0 || h == 0 {
        return img;
    }
    let src = |x: usize, y: usize| img.pixels[y * w + if flip { w - 1 - x } else { x }];
    let (ow, oh) = if rotation % 180 == 90 { (h, w) } else { (w, h) };
    let mut out = Vec::with_capacity(ow * oh);
    for y in 0..oh {
        for x in 0..ow {
            out.push(match rotation % 360 {
                90 => src(y, h - 1 - x),
                180 => src(w - 1 - x, h - 1 - y),
                270 => src(w - 1 - y, x),
                _ => src(x, y),
            });
        }
    }
    ColorImage::new([ow, oh], out)
}

/// Box-filter `img` into a `w × h` picture, letterboxed to keep its aspect.
pub fn fit(img: &ColorImage, w: usize, h: usize) -> ColorImage {
    let [sw, sh] = img.size;
    if sw == 0 || sh == 0 || w == 0 || h == 0 {
        return ColorImage::new([w.max(1), h.max(1)], vec![eframe::egui::Color32::BLACK; w.max(1) * h.max(1)]);
    }
    let s = (w as f32 / sw as f32).min(h as f32 / sh as f32);
    let (dw, dh) = (((sw as f32 * s).round() as usize).clamp(1, w), ((sh as f32 * s).round() as usize).clamp(1, h));
    let (ox, oy) = ((w - dw) / 2, (h - dh) / 2);
    let mut out = vec![eframe::egui::Color32::BLACK; w * h];
    for y in 0..dh {
        let y0 = y * sh / dh;
        let y1 = ((y + 1) * sh / dh).max(y0 + 1).min(sh);
        for x in 0..dw {
            let x0 = x * sw / dw;
            let x1 = ((x + 1) * sw / dw).max(x0 + 1).min(sw);
            let (mut r, mut g, mut b, mut n) = (0u32, 0u32, 0u32, 0u32);
            // Sample a small grid inside the source box; plenty for previews.
            let sy = (y1 - y0).div_ceil(3).max(1);
            let sx = (x1 - x0).div_ceil(3).max(1);
            let mut yy = y0;
            while yy < y1 {
                let mut xx = x0;
                while xx < x1 {
                    let p = img.pixels[yy * sw + xx];
                    r += p.r() as u32;
                    g += p.g() as u32;
                    b += p.b() as u32;
                    n += 1;
                    xx += sx;
                }
                yy += sy;
            }
            let n = n.max(1);
            out[(oy + y) * w + ox + x] = eframe::egui::Color32::from_rgb((r / n) as u8, (g / n) as u8, (b / n) as u8);
        }
    }
    ColorImage::new([w, h], out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_letterboxes() {
        let img = ColorImage::new([4, 2], vec![eframe::egui::Color32::WHITE; 8]);
        let out = fit(&img, 4, 4);
        assert_eq!(out.size, [4, 4]);
        assert_eq!(out.pixels[0], eframe::egui::Color32::BLACK);
        assert_eq!(out.pixels[4 * 1 + 1], eframe::egui::Color32::WHITE);
    }

    fn testdata(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../testdata/media").join(name)
    }

    #[test]
    fn probes_and_decodes_h264_and_hevc() {
        for name in ["h264_high_cabac.mp4", "h264_main_cabac_bframes.mp4", "hevc_main.mp4"] {
            let path = testdata(name);
            if !path.exists() {
                continue;
            }
            let pr = probe(&path).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(pr.kind, m::MediaKind::Video, "{name}");
            let v = pr.video.unwrap();
            assert!(v.width > 0 && v.height > 0);
            let secs = pr.duration.unwrap() as f64 / 1e6;
            // A keyframe thumbnail and a frame a little way in.
            let thumb = frame(&path, 0.0, 64, 36, true).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(thumb.size, [64, 36]);
            let mid = frame(&path, secs * 0.4, 64, 36, false).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(mid.pixels.iter().any(|p| p.r() > 16 || p.g() > 16 || p.b() > 16), "{name} decoded black");
        }
    }

    #[test]
    fn unknown_files_are_reported() {
        assert!(probe(Path::new("/nonexistent/clip.mp4")).is_err());
    }
}
