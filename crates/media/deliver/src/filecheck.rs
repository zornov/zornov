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
    /// H.264 on the video hardware.
    Hw(studio_hw::H264Decoder),
}

/// Decoded planes of one picture, owned so they can cross to a worker.
struct Planes {
    w: usize,
    h: usize,
    y: Vec<u8>,
    u: Vec<u8>,
    v: Vec<u8>,
    cw: usize,
    ch: usize,
}

impl Planes {
    fn analyse(&self, full: bool) -> Option<qc::FrameAnalysis> {
        qc::analyse(&Picture::Planes { w: self.w, h: self.h, y: &self.y, u: &self.u, v: &self.v, cw: self.cw, ch: self.ch, full_range: full })
    }
}

fn avc_planes(frames: Vec<studio_h264::Frame>) -> Vec<Planes> {
    frames.into_iter().map(|f| Planes { w: f.width as usize, h: f.height as usize, y: f.y, u: f.u, v: f.v, cw: f.chroma_width as usize, ch: f.chroma_height as usize }).collect()
}

/// Gives the hardware's pictures back in presentation order. They come out
/// in decode order, tagged with their sample; the software decoder gives
/// display order, and QC must see the same sequence from both.
struct Reorder {
    /// Presentation rank of each sample.
    rank: Vec<usize>,
    held: std::collections::BTreeMap<usize, Planes>,
    next: usize,
}

impl Reorder {
    fn new(samples: &[studio_mp4::Sample]) -> Reorder {
        let mut order: Vec<usize> = (0..samples.len()).collect();
        order.sort_by_key(|&i| samples[i].pts);
        let mut rank = vec![0; samples.len()];
        for (r, i) in order.into_iter().enumerate() {
            rank[i] = r;
        }
        Reorder { rank, held: Default::default(), next: 0 }
    }

    fn take(&mut self, all: bool) -> Vec<Planes> {
        let mut out = Vec::new();
        // A picture that never comes (more than a reorder depth of others
        // wait for it) is not waited for.
        while let Some((&r, _)) = self.held.first_key_value() {
            if r != self.next && !all && self.held.len() <= 32 {
                break;
            }
            out.push(self.held.remove(&r).unwrap_or_else(|| unreachable!()));
            self.next = r + 1;
        }
        out
    }

    fn push(&mut self, frames: Vec<studio_hw::Decoded>) -> Vec<Planes> {
        for f in frames {
            let r = self.rank.get(f.sample as usize).copied().unwrap_or(f.sample as usize);
            self.held.insert(r, Planes { w: f.width as usize, h: f.height as usize, y: f.y, u: f.u, v: f.v, cw: f.chroma_width as usize, ch: f.chroma_height as usize });
        }
        self.take(false)
    }

    fn finish(&mut self, frames: Vec<studio_hw::Decoded>) -> Vec<Planes> {
        self.push(frames);
        self.take(true)
    }
}

fn plane8(p: &studio_hevc::PlaneData, shift: u32) -> std::borrow::Cow<'_, [u8]> {
    match p {
        studio_hevc::PlaneData::U8(v) => std::borrow::Cow::Borrowed(v.as_slice()),
        studio_hevc::PlaneData::U16(v) => std::borrow::Cow::Owned(v.iter().map(|&x| (x >> shift).min(255) as u8).collect()),
    }
}

fn hevc_planes(frames: Vec<studio_hevc::Frame>) -> Vec<Planes> {
    frames
        .into_iter()
        .map(|f| {
            // Deeper pictures are read at eight bits (the top eight).
            let shift = f.bit_depth.saturating_sub(8) as u32;
            Planes { w: f.width, h: f.height, y: plane8(&f.y, shift).into_owned(), u: plane8(&f.u, shift).into_owned(), v: plane8(&f.v, shift).into_owned(), cw: f.chroma_width, ch: f.chroma_height }
        })
        .collect()
}

/// Picture checks on a few threads while the caller keeps decoding. Pictures
/// are analysed in any order and committed in picture order, so the verdict
/// is the one a single thread gives.
struct Pipeline {
    q: VideoQc,
    /// Pictures sent and committed so far.
    sent: usize,
    next: usize,
    tx: Option<std::sync::mpsc::SyncSender<(usize, Planes)>>,
    rx: std::sync::mpsc::Receiver<(usize, Option<qc::FrameAnalysis>)>,
    pending: std::collections::BTreeMap<usize, Option<qc::FrameAnalysis>>,
    workers: Vec<std::thread::JoinHandle<()>>,
}

impl Pipeline {
    fn new(fps: f64, full: bool) -> Pipeline {
        let n = std::thread::available_parallelism().map_or(2, |n| n.get()).clamp(1, 4);
        let (tx, jobs) = std::sync::mpsc::sync_channel::<(usize, Planes)>(n * 2);
        let jobs = std::sync::Arc::new(std::sync::Mutex::new(jobs));
        let (done_tx, rx) = std::sync::mpsc::channel();
        let workers = (0..n)
            .map(|_| {
                let (jobs, done) = (jobs.clone(), done_tx.clone());
                std::thread::spawn(move || {
                    loop {
                        let job = jobs.lock().unwrap_or_else(|e| e.into_inner()).recv();
                        let Ok((i, p)) = job else { break };
                        if done.send((i, p.analyse(full))).is_err() {
                            break;
                        }
                    }
                })
            })
            .collect();
        Pipeline { q: VideoQc::new(fps), sent: 0, next: 0, tx: Some(tx), rx, pending: Default::default(), workers }
    }

    fn commit_ready(&mut self) {
        while let Some(a) = self.pending.remove(&self.next) {
            if let Some(a) = a {
                self.q.commit(a);
            }
            self.next += 1;
        }
    }

    fn feed(&mut self, frames: Vec<Planes>) {
        for p in frames {
            // Empty pictures are skipped by the analysis; the numbering keeps going.
            let i = self.sent;
            self.sent += 1;
            let _ = self.tx.as_ref().map(|tx| tx.send((i, p)));
            while let Ok((i, a)) = self.rx.try_recv() {
                self.pending.insert(i, a);
            }
            self.commit_ready();
        }
    }

    fn finish(mut self) -> Result<VideoQc, String> {
        self.tx = None;
        for w in self.workers.drain(..) {
            let _ = w.join();
        }
        while let Ok((i, a)) = self.rx.try_recv() {
            self.pending.insert(i, a);
        }
        self.commit_ready();
        if self.next != self.sent {
            return Err("a picture check stopped before it finished".into());
        }
        Ok(self.q)
    }
}

/// Check `path` against `profile`. The pictures are read on the video
/// hardware where there is some; `ZORNOV_QC_SOFTWARE_DECODE=1` makes it
/// Studio's own decoder, proof that Studio can read the file itself.
pub fn check_file(path: &Path, profile: &Profile, extra: &Extra) -> Result<Report, String> {
    let software = std::env::var_os("ZORNOV_QC_SOFTWARE_DECODE").is_some_and(|v| !v.is_empty() && v != "0");
    check_file_with(path, profile, extra, software)
}

fn check_file_with(path: &Path, profile: &Profile, extra: &Extra, software_only: bool) -> Result<Report, String> {
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
        let limit = extra.max_secs.map(|s| (s * fps) as usize).unwrap_or(usize::MAX);
        // A cut-short run stops on a whole batch of pictures, and how many
        // come out at once depends on the decoder: those stay on Studio's own.
        let software_only = software_only || extra.max_secs.is_some();
        // Hardware first for H.264; any trouble with it and the whole
        // file is read again with Studio's own decoder.
        let mut tried_hw = software_only || !matches!(&t.codec, studio_mp4::Codec::Avc { .. });
        let q = loop {
            let mut dec = match &t.codec {
                studio_mp4::Codec::Avc { config } => {
                    facts.video_codec = "h264".into();
                    match (!tried_hw).then(|| studio_hw::H264Decoder::new(&config.raw)).flatten() {
                        Some(hw) => Dec::Hw(hw),
                        None => {
                            let sps: Vec<&[u8]> = config.sps.iter().map(|s| s.as_slice()).collect();
                            let pps: Vec<&[u8]> = config.pps.iter().map(|s| s.as_slice()).collect();
                            Dec::Avc(studio_h264::Decoder::new(&sps, &pps, config.nal_length_size as usize).map_err(|e| format!("{e:?}"))?)
                        }
                    }
                }
                studio_mp4::Codec::Hevc { config } => {
                    let sets: Vec<&[u8]> = config.vps().into_iter().chain(config.sps()).chain(config.pps()).collect();
                    facts.video_codec = "hevc".into();
                    Dec::Hevc(studio_hevc::Decoder::new(&sets, config.nal_length_size as usize).map_err(|e| format!("{e:?}"))?)
                }
                _ => return Err("QC reads H.264 and HEVC pictures".into()),
            };
            tried_hw = true;
            let mut pipe = Pipeline::new(fps, full);
            let mut buf = Vec::new();
            let mut hw_failed = false;
            let mut order = Reorder::new(&t.samples);
            'pictures: for (n, s) in t.samples.iter().enumerate() {
                dmx.read_sample_into(s, &mut buf).map_err(|e| e.to_string())?;
                let frames = match &mut dec {
                    Dec::Avc(d) => avc_planes(d.decode(&buf).map_err(|e| format!("{e:?}"))?),
                    Dec::Hevc(d) => hevc_planes(d.decode(&buf).map_err(|e| format!("{e:?}"))?),
                    Dec::Hw(d) => match d.decode(&buf, n as i64) {
                        Ok(f) => order.push(f),
                        Err(_) => {
                            hw_failed = true;
                            break 'pictures;
                        }
                    },
                };
                pipe.feed(frames);
                if pipe.sent >= limit {
                    break 'pictures;
                }
            }
            if hw_failed {
                continue;
            }
            match &mut dec {
                Dec::Avc(d) => pipe.feed(avc_planes(d.flush())),
                Dec::Hevc(d) => pipe.feed(hevc_planes(d.flush())),
                Dec::Hw(d) => pipe.feed(order.finish(d.flush())),
            }
            break pipe.finish()?;
        };
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

    fn same_verdict(a: &qc::Report, b: &qc::Report) {
        assert_eq!(a.to_text(), b.to_text());
        assert_eq!(a.measurements, b.measurements);
        assert_eq!(a.findings, b.findings);
    }

    #[test]
    fn hardware_and_software_decoders_give_one_verdict() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../testdata/media");
        for name in ["h264_baseline_cavlc.mp4", "h264_high_cabac.mp4", "h264_main_cabac_bframes.mp4", "h264_main_cavlc_bframes.mp4", "h264_high_odd_size.mov"] {
            let p = dir.join(name);
            let hw = check_file(&p, &Profile::default(), &Extra::default()).unwrap();
            let sw = check_file_with(&p, &Profile::default(), &Extra::default(), true).unwrap();
            same_verdict(&hw, &sw);
            let part = check_file(&p, &Profile::default(), &Extra { max_secs: Some(0.3), ..Extra::default() }).unwrap();
            let part_sw = check_file_with(&p, &Profile::default(), &Extra { max_secs: Some(0.3), ..Extra::default() }, true).unwrap();
            same_verdict(&part, &part_sw);
        }
    }

    #[test]
    fn threaded_picture_checks_match_one_thread() {
        // Flat grey with a flashing stretch, a still stretch and a moving one.
        let (w, h) = (96usize, 64usize);
        let frame = |i: usize| -> Planes {
            let luma = match i {
                10..=25 => if i % 2 == 0 { 30 } else { 220 },
                26..=45 => 90,
                _ => ((i * 7) % 200 + 20) as u8,
            };
            let mut y = vec![luma; w * h];
            for (k, p) in y.iter_mut().enumerate() {
                if i >= 46 {
                    *p = p.wrapping_add((k % w) as u8);
                }
            }
            Planes { w, h, y, u: vec![128; w * h / 4], v: vec![if i % 3 == 0 { 200 } else { 128 }; w * h / 4], cw: w / 2, ch: h / 2 }
        };
        let n = 80;
        let mut one = VideoQc::new(25.0);
        for i in 0..n {
            let f = frame(i);
            one.push(&Picture::Planes { w, h, y: &f.y, u: &f.u, v: &f.v, cw: f.cw, ch: f.ch, full_range: false });
        }
        let mut pipe = Pipeline::new(25.0, false);
        pipe.feed((0..n).map(frame).collect());
        let many = pipe.finish().unwrap();
        let facts = qc::Facts { width: w as u32, height: h as u32, fps: 25.0, video_secs: n as f64 / 25.0, ..Default::default() };
        let a = qc::report(&Profile::default(), &facts, Some(&one), None);
        let b = qc::report(&Profile::default(), &facts, Some(&many), None);
        same_verdict(&a, &b);
        assert_eq!(many.frames(), n as u64);
        assert!(a.findings.iter().any(|f| f.check == "flash" || f.check == "frozen"), "{}", a.to_text());
    }
}
