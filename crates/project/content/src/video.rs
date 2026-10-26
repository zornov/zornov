// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A poster frame from an MP4 clip (H.264), used for motion-clip
//! thumbnails. The frame is taken a quarter of the way in, which avoids the
//! usual empty first frame of a looping effect.

use studio_media::rgba::RgbaImage;
use studio_mp4::Codec;
use std::path::Path;

/// Decodes the frame shown at `fraction` (0..1) of the clip's duration.
pub fn poster_frame(path: &Path, fraction: f64) -> Result<RgbaImage, String> {
    let mut demuxer = studio_mp4::demux::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let track = demuxer.movie().video().ok_or("no video track")?.clone();
    let (sps, pps, nal_len) = match &track.codec {
        Codec::Avc { config } => (config.sps.clone(), config.pps.clone(), config.nal_length_size as usize),
        Codec::Hevc { .. } => return Err("HEVC poster frames are not supported yet".into()),
        other => return Err(format!("unsupported video codec {other:?}")),
    };
    let sps_refs: Vec<&[u8]> = sps.iter().map(|v| v.as_slice()).collect();
    let pps_refs: Vec<&[u8]> = pps.iter().map(|v| v.as_slice()).collect();
    let mut decoder = studio_h264::Decoder::new(&sps_refs, &pps_refs, nal_len).map_err(|e| e.to_string())?;
    let secs = (track.duration_secs * fraction.clamp(0.0, 0.95)).max(0.0);
    let seek = track.sample_for_time(secs).or_else(|| track.sample_for_time(0.0)).ok_or("empty video track")?;
    let target_pts = track.samples.get(seek.target_index).map(|s| s.pts).unwrap_or(0);
    let mut best: Option<studio_h264::Frame> = None;
    let mut buf = Vec::new();
    let consider = |best: &mut Option<studio_h264::Frame>, f: studio_h264::Frame| {
        let pts = f.pts.unwrap_or(i64::MIN);
        let better = match best {
            None => true,
            Some(b) => {
                let bp = b.pts.unwrap_or(i64::MIN);
                // Prefer the latest frame at or before the target, else the earliest after it.
                if bp <= target_pts { pts <= target_pts && pts > bp } else { pts < bp }
            }
        };
        if better {
            *best = Some(f);
        }
    };
    for idx in seek.sync_index..=seek.target_index.min(track.samples.len().saturating_sub(1)) {
        let sample = track.samples[idx];
        demuxer.read_sample_into(&sample, &mut buf).map_err(|e| e.to_string())?;
        let frames = decoder.decode_with_pts(&buf, Some(sample.pts)).map_err(|e| e.to_string())?;
        for f in frames {
            consider(&mut best, f);
        }
    }
    for f in decoder.flush() {
        consider(&mut best, f);
    }
    let frame = best.ok_or("no frame decoded")?;
    Ok(RgbaImage { width: frame.width, height: frame.height, pixels: studio_h264::to_rgba(&frame) })
}

/// Duration in seconds from the container header.
pub fn duration_secs(path: &Path) -> Result<f64, String> {
    let demuxer = studio_mp4::demux::open(path).map_err(|e| e.to_string())?;
    Ok(demuxer.movie().duration_secs)
}
