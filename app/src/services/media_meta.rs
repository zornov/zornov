// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Pro media Studio plays natively, without converting: Apple ProRes in
//! QuickTime (with PCM sound), OpenEXR, DPX, TIFF and 16-bit PNG stills,
//! and numbered frame sequences (`plate.####.exr`, or any frame of one).
//! Probing, thumbnails, sound, and the source timecode / reel name that
//! EDLs and conform need.

use crate::media_io::Probe;
use studio_model as m;
use std::path::{Path, PathBuf};

fn rate_of(fps: f64) -> m::FrameRate {
    studio_model::timecode::rate_from_name(&format!("{fps:.3}")).unwrap_or(m::FrameRate::FPS_24)
}

fn is_still_format(path: &Path) -> bool {
    let mut head = [0u8; 32];
    let n = std::fs::File::open(path).and_then(|mut f| std::io::Read::read(&mut f, &mut head)).unwrap_or(0);
    let head = &head[..n];
    matches!(studio_imageio::Format::sniff(head), Some(studio_imageio::Format::Exr | studio_imageio::Format::Dpx | studio_imageio::Format::Tiff))
        || (head.len() >= 26 && head.starts_with(b"\x89PNG\r\n\x1a\n") && head[24] == 16)
}

fn is_prores(path: &Path) -> bool {
    let Ok(dm) = studio_mp4::open(path) else { return false };
    dm.movie().video().is_some_and(|t| studio_prores::Profile::from_fourcc(&t.fourcc).is_some())
}

/// Whether Studio decodes `path` itself through these formats.
pub fn native(path: &Path) -> bool {
    studio_imageio::sequence::Sequence::is_pattern(path) || (path.is_file() && (is_still_format(path) || is_prores(path)))
}

/// The probe of a native pro file, or `None` for anything else.
pub fn probe(path: &Path) -> Option<Result<Probe, String>> {
    if studio_imageio::sequence::Sequence::is_pattern(path) {
        return Some(probe_sequence(path));
    }
    if !path.is_file() {
        return None;
    }
    if is_still_format(path) {
        return Some(studio_imageio::read_with_format(path).map(|(img, f)| Probe {
            kind: m::MediaKind::Image,
            duration: None,
            video: Some(m::VideoInfo { codec: format!("{f:?}").to_lowercase(), width: img.width, height: img.height, frame_rate: None, rotation: 0, has_alpha: img.has_alpha, colour: Default::default() }),
            audio: None,
            container: format!("{f:?}").to_lowercase(),
        }));
    }
    if is_prores(path) {
        return Some(probe_prores(path));
    }
    None
}

fn probe_sequence(path: &Path) -> Result<Probe, String> {
    let seq = studio_imageio::sequence::Sequence::from_pattern(path)?;
    let first = (seq.first..=seq.last).map(|n| seq.frame(n)).find(|p| p.exists()).ok_or("the sequence has no frames")?;
    let (img, f) = studio_imageio::read_with_format(&first)?;
    let fps = img.meta.frame_rate.unwrap_or(24.0);
    let frames = seq.len() as f64;
    Ok(Probe {
        kind: m::MediaKind::Video,
        duration: Some((frames / fps * 1e6).round() as m::Micros),
        video: Some(m::VideoInfo { codec: format!("{f:?} sequence").to_lowercase(), width: img.width, height: img.height, frame_rate: Some(rate_of(fps)), rotation: 0, has_alpha: img.has_alpha, colour: Default::default() }),
        audio: None,
        container: "sequence".into(),
    })
}

fn probe_prores(path: &Path) -> Result<Probe, String> {
    let dm = studio_mp4::open(path).map_err(|e| format!("{e}"))?;
    let movie = dm.movie();
    let t = movie.video().ok_or("no video track")?;
    let profile = studio_prores::Profile::from_fourcc(&t.fourcc).ok_or("not ProRes")?;
    let fps = if t.duration_secs > 0.0 { t.samples.len() as f64 / t.duration_secs } else { 24.0 };
    let audio = studio_mov::probe::pcm_format(path).map(|(rate, ch)| m::AudioInfo { codec: "pcm".into(), sample_rate: rate, channels: ch, layout: if ch == 1 { "mono".into() } else { "stereo".into() } });
    Ok(Probe {
        kind: m::MediaKind::Video,
        duration: Some((t.duration_secs * 1e6).round() as m::Micros),
        video: Some(m::VideoInfo {
            codec: format!("prores_{}", String::from_utf8_lossy(&profile.fourcc())),
            width: t.width,
            height: t.height,
            frame_rate: Some(rate_of(fps)),
            rotation: t.rotation as u32,
            has_alpha: false,
            colour: Default::default(),
        }),
        audio,
        container: "mov".into(),
    })
}

/// Fills in what the file says about its source: timecode and reel name
/// (QuickTime `tmcd` track, DPX / EXR headers).
pub fn enrich(item: &mut m::MediaItem, path: &Path) {
    if let Some(tc) = studio_mov::probe::timecode(path) {
        let rate = item.info.video.as_ref().and_then(|v| v.frame_rate).unwrap_or_else(|| m::FrameRate::new(tc.fps.max(1), 1));
        let t = m::Timecode::new(rate, tc.drop_frame);
        item.timecode = Some(t.format_frames(tc.frame as i64));
        if tc.reel.is_some() {
            item.reel = tc.reel;
        }
        return;
    }
    let first: Option<PathBuf> = if studio_imageio::sequence::Sequence::is_pattern(path) {
        studio_imageio::sequence::Sequence::from_pattern(path).ok().and_then(|s| (s.first..=s.last).map(|n| s.frame(n)).find(|p| p.exists()))
    } else if is_still_format(path) {
        Some(path.to_path_buf())
    } else {
        None
    };
    if let Some(f) = first
        && let Ok(img) = studio_imageio::read(&f)
    {
        if let Some(tc) = img.meta.timecode {
            item.timecode = Some(tc.to_string());
        }
        if img.meta.reel.is_some() {
            item.reel = img.meta.reel;
        }
    }
}

/// A frame of a native pro file as an egui image at full size
/// (thumbnails and the bin fit it).
#[cfg(feature = "render")]
pub fn frame(path: &Path, secs: f64) -> Option<Result<eframe::egui::ColorImage, String>> {
    if !native(path) {
        return None;
    }
    Some((|| {
        let mut src = studio_render::source::open_path(path)?;
        let info = src.info();
        let index = src.frame_index((secs * 1e6) as i64).min(info.frame_count.unwrap_or(1).saturating_sub(1));
        let mut got = None;
        if !src.decode_to(index, &mut |i, p| {
            if i == index {
                got = Some(p);
            }
        }) {
            return Err(format!("{}: frame {index} cannot be decoded", path.display()));
        }
        let pic = got.ok_or("no picture")?;
        let frame = pic.to_frame(true);
        let rgba = studio_render::output::convert_rgba8(&frame, &studio_render::OutputSettings::default(), 0);
        Ok(eframe::egui::ColorImage::from_rgba_unmultiplied([frame.width as usize, frame.height as usize], &rgba))
    })())
}

/// PCM sound of a QuickTime file, interleaved, for the sound mixer.
pub fn pcm(path: &Path) -> Option<studio_media::wav::Wav> {
    let (rate, channels, samples) = studio_mov::probe::pcm(path)?;
    Some(studio_media::wav::Wav { sample_rate: rate, channels, samples })
}
