// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Mastering exports, written from 16-bit pictures:
//!
//! * ProRes 422 HQ / 4444 / 422 Proxy in QuickTime (`studio-mov`), with
//!   the mix as 24-bit PCM and a timecode track starting at 01:00:00:00
//!   (drop-frame at 29.97 / 59.94), tagged with the project's colour space.
//! * OpenEXR sequences in ACES2065-1: the frame is rendered scene-referred
//!   (ACEScct-encoded 16-bit, no view transform), decoded to linear AP1
//!   and converted to AP0, written as half floats with the ACES container
//!   flag, chromaticities, timecode and frame rate.
//! * DPX (10-bit), TIFF (16-bit) and PNG (16-bit) sequences in the
//!   project's display encoding; DPX carries timecode and frame rate.
//! * DNxHR LB / SQ / HQ / HQX / 444 (SMPTE VC-3, `studio-dnxhr`) in QuickTime
//!   with the same sound, timecode and colour tags.
//!
//! Sequences are named `<folder name>.<frame>.ext`, numbered by their
//! record timecode frame (01:00:00:00 is frame 86400 at 24 fps).

use crate::formats::Format;
use std::path::{Path, PathBuf};
use studio_model as m;
use studio_render::output::OutputSpace;

/// The output encoding to render a pro format in.
pub fn output_space(format: Format, project: &m::Project) -> OutputSpace {
    if format == Format::Exr {
        return OutputSpace::AcesCct;
    }
    match project.settings.colour_space {
        m::ColourSpace::Rec709 => OutputSpace::Rec709,
        m::ColourSpace::DisplayP3 => OutputSpace::DisplayP3,
        m::ColourSpace::Rec2020Pq => OutputSpace::Rec2020Pq,
        m::ColourSpace::Rec2020Hlg => OutputSpace::Rec2020Hlg,
    }
}

/// H.273 primaries, transfer and matrix of an output space.
fn h273(space: OutputSpace) -> (u8, u8, u8) {
    match space {
        OutputSpace::Rec709 | OutputSpace::SrgbDisplay => (1, 1, 1),
        OutputSpace::DisplayP3 => (12, 13, 1),
        OutputSpace::Rec2020Pq => (9, 16, 9),
        OutputSpace::Rec2020Hlg => (9, 18, 9),
        OutputSpace::AcesCct => (2, 2, 2),
    }
}

/// Record start as a frame count of timecode labels.
fn start_frame(fps_num: u32, fps_den: u32) -> (u32, m::Timecode) {
    let rate = m::FrameRate::new(fps_num, fps_den);
    let tc = m::Timecode::for_rate(rate);
    (tc.label_frames(3600 * m::SECOND) as u32, tc)
}

enum MovEncoder {
    ProRes(studio_mov::prores::ProResEncoder),
    Dnx(studio_mov::dnxhr::DnxEncoder),
}

enum Inner {
    Mov { mov: studio_mov::mov::MovWriter, enc: MovEncoder },
    Seq { dir: PathBuf, stem: String, next: u64, format: Format, tc: m::Timecode },
}

/// A running pro export.
pub struct ProWriter {
    inner: Inner,
    space: OutputSpace,
    fps: (u32, u32),
    /// The mix, interleaved stereo, and how much of it has been written.
    pcm: Vec<f32>,
    pcm_at: usize,
    frames_written: u64,
}

impl ProWriter {
    pub fn create(format: Format, path: &Path, project: &m::Project, w: u32, h: u32, fps: (u32, u32), pcm: Vec<f32>) -> Result<ProWriter, String> {
        let space = output_space(format, project);
        let (start, tc) = start_frame(fps.0, fps.1);
        let inner = match format {
            Format::ProRes422Hq | Format::ProRes4444 | Format::ProResProxy | Format::DnxHrLb | Format::DnxHrSq | Format::DnxHrHq | Format::DnxHr | Format::DnxHr444 => {
                let profile = match format {
                    Format::ProRes422Hq => studio_prores::Profile::Hq,
                    Format::ProRes4444 => studio_prores::Profile::P4444,
                    Format::ProResProxy => studio_prores::Profile::Proxy,
                    // Unused for DNxHR files (the profile is set below).
                    _ => studio_prores::Profile::Hq,
                };
                let target = path.to_path_buf();
                let (p, t, mm) = h273(space);
                let video = studio_mov::mov::VideoTrack { profile, width: w, height: h, rate_num: fps.0, rate_den: fps.1, colour: (p as u16, t as u16, mm as u16), alpha: false };
                let audio = (!pcm.is_empty()).then_some(studio_mov::mov::AudioTrack { sample_rate: crate::sound_bridge::RATE, channels: 2 });
                let timecode = studio_mov::StartTimecode { frame: start, fps: tc.nominal_fps() as u32, drop_frame: tc.is_drop_frame(), reel: Some(m::interchange::edl::reel_name(&project.active().name)) };
                if let Some(dnx) = format.dnxhr() {
                    let enc = studio_mov::dnxhr::DnxEncoder::new(dnx, w, h, (p, t, mm)).map_err(|e| format!("{} cannot write a {w} x {h} picture: {e}", format.label()))?;
                    let mov = studio_mov::mov::MovWriter::create_dnxhr(&target, dnx, video, audio, Some(timecode)).map_err(|e| e.to_string())?;
                    Inner::Mov { mov, enc: MovEncoder::Dnx(enc) }
                } else {
                    let mov = studio_mov::mov::MovWriter::create(&target, video, audio, Some(timecode)).map_err(|e| e.to_string())?;
                    let enc = studio_mov::prores::ProResEncoder::new(profile, w, h, (p, t, mm), false);
                    Inner::Mov { mov, enc: MovEncoder::ProRes(enc) }
                }
            }
            _ => {
                std::fs::create_dir_all(path).map_err(|e| format!("{}: {e}", path.display()))?;
                let stem = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "frames".into());
                Inner::Seq { dir: path.to_path_buf(), stem, next: start as u64, format, tc }
            }
        };
        Ok(ProWriter { inner, space, fps, pcm, pcm_at: 0, frames_written: 0 })
    }

    /// Writes one frame of RGBA16 pixels.
    pub fn push(&mut self, rgba16: &[u16], w: u32, h: u32) -> Result<(), String> {
        self.frames_written += 1;
        match &mut self.inner {
            Inner::Mov { mov, enc, .. } => {
                let bytes = match enc {
                    MovEncoder::ProRes(e) => e.encode(rgba16, w, h)?,
                    MovEncoder::Dnx(e) => e.encode(rgba16, w, h)?,
                };
                mov.write_video(&bytes).map_err(|e| e.to_string())?;
                // The mix up to the end of this frame.
                let end = ((self.frames_written as f64 * self.fps.1 as f64 / self.fps.0 as f64 * crate::sound_bridge::RATE as f64).round() as usize * 2).min(self.pcm.len());
                if end > self.pcm_at {
                    mov.write_audio(&self.pcm[self.pcm_at..end]).map_err(|e| e.to_string())?;
                    self.pcm_at = end;
                }
                Ok(())
            }
            Inner::Seq { dir, stem, next, format, tc } => {
                let n = *next;
                *next += 1;
                let px: Vec<[f32; 4]> = rgba16.chunks_exact(4).map(|c| [c[0] as f32 / 65535.0, c[1] as f32 / 65535.0, c[2] as f32 / 65535.0, c[3] as f32 / 65535.0]).collect();
                let label = tc.label(n as i64);
                let timecode = studio_imageio::TimeCode { hours: label.0 as u8, minutes: label.1 as u8, seconds: label.2 as u8, frames: label.3 as u8, drop_frame: tc.is_drop_frame() };
                let (bytes, ext) = match format {
                    Format::Exr => {
                        // ACEScct-encoded AP1 → linear AP1 → ACES2065-1.
                        let m = studio_colour::primaries::rgb_to_rgb(&studio_colour::Primaries::ACES_AP1, &studio_colour::Primaries::ACES_AP0);
                        let lin: Vec<[f32; 4]> = px
                            .iter()
                            .map(|p| {
                                let c = m.apply(studio_colour::Transfer::AcesCct.decode_rgb([p[0], p[1], p[2]]));
                                [c[0], c[1], c[2], p[3]]
                            })
                            .collect();
                        let rate = ((self.fps.0) as i32, self.fps.1);
                        (studio_imageio::exr::encode(w, h, &lin, &studio_imageio::exr::WriteOptions { aces: true, timecode: Some(timecode), frame_rate: Some(rate), ..Default::default() }), "exr")
                    }
                    Format::Dpx => {
                        let transfer = match self.space {
                            OutputSpace::Rec709 | OutputSpace::SrgbDisplay => studio_imageio::Transfer::Video,
                            _ => studio_imageio::Transfer::Other(0),
                        };
                        let opts = studio_imageio::dpx::WriteOptions { bits: 10, transfer, timecode: Some(timecode), frame_rate: Some(self.fps.0 as f64 / self.fps.1 as f64), reel: None };
                        (studio_imageio::dpx::encode(w, h, &px, &opts), "dpx")
                    }
                    Format::Tiff16 => (studio_imageio::tiff::encode(w, h, &px, false), "tif"),
                    _ => (studio_imageio::png16::encode(w, h, &px, false), "png"),
                };
                std::fs::write(dir.join(format!("{stem}.{n:07}.{ext}")), bytes).map_err(|e| e.to_string())
            }
        }
    }

    /// Finishes the file.
    pub fn finish(mut self, out: &Path) -> Result<(), String> {
        match self.inner {
            Inner::Mov { mut mov, .. } => {
                if self.pcm_at < self.pcm.len() {
                    mov.write_audio(&self.pcm[self.pcm_at..]).map_err(|e| e.to_string())?;
                    self.pcm_at = self.pcm.len();
                }
                mov.finish().map_err(|e| e.to_string())?;
                Ok(())
            }
            Inner::Seq { .. } => Ok(()),
        }
    }

    /// Removes what a cancelled export left.
    pub fn abandon(self, out: &Path) {
        if let Inner::Mov { .. } = &self.inner {
            let _ = std::fs::remove_file(out);
        }
    }
}
