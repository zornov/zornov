// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! QC of a finished MP4: decode every picture and all the sound and run
//! [`crate::qc`] over them.

use crate::qc::{self, AudioQc, Facts, Picture, Profile, Report, VideoQc};
use std::path::Path;

/// What the caller knows that the file cannot say.
#[derive(Default, Clone)]
pub struct Extra {
    /// Speech spans in seconds, for dialogue-gated loudness.
    pub speech: Vec<(f64, f64)>,
    pub caption_cues: Option<usize>,
    pub speech_secs: Option<f64>,
    /// Stop after this many seconds of programme (long films, quick look).
    pub max_secs: Option<f64>,
}

enum Dec {
    Avc(studio_h264::Decoder),
    Hevc(studio_hevc::Decoder),
}

fn feed_avc(vq: &mut VideoQc, frames: Vec<studio_h264::Frame>, full: bool) {
    for f in frames {
        vq.push(&Picture::Planes { w: f.width as usize, h: f.height as usize, y: &f.y, u: &f.u, v: &f.v, cw: f.chroma_width as usize, ch: f.chroma_height as usize, full_range: full });
    }
}

fn plane8(p: &studio_hevc::PlaneData, shift: u32) -> std::borrow::Cow<'_, [u8]> {
    match p {
        studio_hevc::PlaneData::U8(v) => std::borrow::Cow::Borrowed(v.as_slice()),
        studio_hevc::PlaneData::U16(v) => std::borrow::Cow::Owned(v.iter().map(|&x| (x >> shift).min(255) as u8).collect()),
    }
}

fn feed_hevc(vq: &mut VideoQc, frames: Vec<studio_hevc::Frame>, full: bool) {
    for f in frames {
        // Deeper pictures are read at eight bits (the top eight).
        let shift = f.bit_depth.saturating_sub(8) as u32;
        let (y, u, v) = (plane8(&f.y, shift), plane8(&f.u, shift), plane8(&f.v, shift));
        vq.push(&Picture::Planes { w: f.width, h: f.height, y: &y, u: &u, v: &v, cw: f.chroma_width, ch: f.chroma_height, full_range: full });
    }
}

/// Check `path` against `profile`.
pub fn check_file(path: &Path, profile: &Profile, extra: &Extra) -> Result<Report, String> {
    let mut dmx = studio_mp4::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let movie = dmx.movie().clone();
    let bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let mut facts = Facts { file: path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), bytes, caption_cues: extra.caption_cues, speech_secs: extra.speech_secs, ..Default::default() };
    let mut vq = None;
    if let Some(t) = movie.video() {
        let t = t.clone();
        let full = matches!(&t.color, Some(studio_mp4::ColorInfo::Nclx { full_range: true, .. }));
        let dur = t.samples.first().map_or(0, |s| s.duration as u64).max(1);
        let fps = t.timescale as f64 / dur as f64;
        facts.width = t.coded_width;
        facts.height = t.coded_height;
        facts.fps = fps;
        facts.video_secs = t.samples.len() as f64 * dur as f64 / t.timescale.max(1) as f64;
        let mut dec = match &t.codec {
            studio_mp4::Codec::Avc { config } => {
                let sps: Vec<&[u8]> = config.sps.iter().map(|s| s.as_slice()).collect();
                let pps: Vec<&[u8]> = config.pps.iter().map(|s| s.as_slice()).collect();
                facts.video_codec = "h264".into();
                Dec::Avc(studio_h264::Decoder::new(&sps, &pps, config.nal_length_size as usize).map_err(|e| format!("{e:?}"))?)
            }
            studio_mp4::Codec::Hevc { config } => {
                let sets: Vec<&[u8]> = config.vps().into_iter().chain(config.sps()).chain(config.pps()).collect();
                facts.video_codec = "hevc".into();
                Dec::Hevc(studio_hevc::Decoder::new(&sets, config.nal_length_size as usize).map_err(|e| format!("{e:?}"))?)
            }
            _ => return Err("QC reads H.264 and HEVC pictures".into()),
        };
        let mut q = VideoQc::new(fps);
        let limit = extra.max_secs.map(|s| (s * fps) as usize).unwrap_or(usize::MAX);
        let mut buf = Vec::new();
        'pictures: for s in &t.samples {
            dmx.read_sample_into(s, &mut buf).map_err(|e| e.to_string())?;
            match &mut dec {
                Dec::Avc(d) => feed_avc(&mut q, d.decode(&buf).map_err(|e| format!("{e:?}"))?, full),
                Dec::Hevc(d) => feed_hevc(&mut q, d.decode(&buf).map_err(|e| format!("{e:?}"))?, full),
            }
            if q.frames() as usize >= limit {
                break 'pictures;
            }
        }
        match &mut dec {
            Dec::Avc(d) => feed_avc(&mut q, d.flush(), full),
            Dec::Hevc(d) => feed_hevc(&mut q, d.flush(), full),
        }
        vq = Some(q);
    }
    let mut aq = None;
    if let Some(t) = movie.audio() {
        let t = t.clone();
        if let studio_mp4::Codec::Aac { asc, .. } = &t.codec {
            let mut dec = studio_aac::Decoder::new(asc).map_err(|e| format!("{e:?}"))?;
            let (mut l, mut r) = (Vec::<f32>::new(), Vec::<f32>::new());
            let mut rate;
            let mut a = None::<AudioQc>;
            let mut buf = Vec::new();
            let skip_media = t.edits.first().map(|e| e.media_time.max(0)).unwrap_or(0) as u64;
            let mut skip: Option<usize> = None;
            let limit = extra.max_secs.map(|s| (s * t.sample_rate as f64) as usize).unwrap_or(usize::MAX);
            let mut total = 0usize;
            for s in &t.samples {
                dmx.read_sample_into(s, &mut buf).map_err(|e| e.to_string())?;
                let (ch, r_) = (dec.channels().max(1), dec.sample_rate().max(1));
                let Ok(pcm) = dec.decode_frame(&buf) else { continue };
                rate = r_;
                let skip_frames = *skip.get_or_insert_with(|| if t.timescale > 0 { (skip_media * rate as u64 / t.timescale as u64) as usize } else { 0 });
                l.clear();
                r.clear();
                for f in pcm.chunks_exact(ch) {
                    l.push(f[0]);
                    r.push(if ch > 1 { f[1] } else { f[0] });
                }
                let (from, take) = if total < skip_frames { (skip_frames - total, l.len()) } else { (0, l.len()) };
                total += l.len();
                if from >= l.len() {
                    continue;
                }
                let aq = a.get_or_insert_with(|| {
                    let q = AudioQc::new(rate);
                    if profile.dialogue_gated && !extra.speech.is_empty() { q.with_speech(extra.speech.clone()) } else { q }
                });
                let _ = take;
                aq.push(&l[from..], &r[from..]);
                if aq.seconds() * rate as f64 >= limit as f64 {
                    break;
                }
            }
            if let Some(q) = a {
                facts.audio_secs = Some(q.seconds());
                aq = Some(q);
            }
        } else {
            // A sound track we do not decode: it counts as present.
            facts.audio_secs = Some(facts.video_secs);
        }
    }
    Ok(qc::report(profile, &facts, vq.as_ref(), aq.as_ref()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_real_mp4_is_decoded_and_judged() {
        let dir = std::env::temp_dir().join(format!("studio-filecheck-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("a.mp4");
        crate::testutil::piece(&p, 64, 48, 0, 30);
        // A moving gradient with no sound.
        let r = check_file(&p, &Profile::default(), &Extra::default()).unwrap();
        assert!(r.passed, "{}", r.to_text());
        assert_eq!(r.measurements["size"], serde_json::json!([64, 48]));
        assert_eq!(r.measurements["frames"], 30);
        // The same file against a spec that wants sound and a bigger picture fails on both.
        let strict = Profile { expect_audio: true, expect_size: Some((1920, 1080)), ..Profile::default() };
        let r = check_file(&p, &strict, &Extra::default()).unwrap();
        assert!(!r.passed);
        assert!(r.findings.iter().any(|f| f.check == "audio_missing") && r.findings.iter().any(|f| f.check == "size"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
