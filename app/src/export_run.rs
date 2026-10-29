// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Export: frames from `studio-render`'s exporter, written by Studio's own
//! encoders (`encode` for H.264 in MP4, `media` for CLV, PNG and GIF). Runs
//! on a background thread; the jobs popover shows progress and can cancel.
//!
//! MP4 exports carry the sequence's sound, mixed by `studio-sound` and
//! encoded as AAC-LC: a stereo track (192 kb/s), a 5.1 track too when the
//! sound desk mixes in surround (384 kb/s), and the same again for every
//! dubbed language (its M&E with the dub), each tagged with its language;
//! every subtitle track goes in as a timed-text track and beside the file
//! as `.srt` and `.vtt` (`sound_desk::render::tracks`, `sound_desk::subs`).

use crate::doc::Doc;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use studio_render::export::{ExportDepth, ExportFrame, ExportPixels, ExportProgress, ExportSettings, Exporter};
use studio_render::scene::{Canvas, ClipDesc, Micros, Scene, TransitionDesc};

pub use crate::formats::Format;

/// What the dialog asks for.
#[derive(Clone, Debug)]
pub struct Request {
    pub path: PathBuf,
    pub format: Format,
    pub size: (u32, u32),
    pub fps: f64,
    /// Sequence range in microseconds.
    pub range: (Micros, Micros),
    /// 0..100 (CLV quality).
    pub quality: f32,
    /// H.264 bitrate.
    pub bitrate_kbps: u32,
}

/// Shared state of a running export.
pub struct Task {
    pub progress: Arc<ExportProgress>,
    pub finished: AtomicBool,
    pub error: Mutex<Option<String>>,
    pub path: PathBuf,
}

impl crate::app::JobTask for Task {
    fn fraction(&self) -> f32 {
        Task::fraction(self)
    }
    fn finished(&self) -> bool {
        self.finished.load(Ordering::Acquire)
    }
    fn cancel(&self) {
        self.progress.cancel();
    }
    fn cancelled(&self) -> bool {
        self.progress.is_cancelled()
    }
    fn take_error(&self) -> Option<String> {
        self.error.lock().unwrap_or_else(|e| e.into_inner()).take()
    }
}

impl std::fmt::Debug for Task {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Task({} at {:.0}%)", self.path.display(), self.fraction() * 100.0)
    }
}

impl Task {
    /// 0..1.
    pub fn fraction(&self) -> f32 {
        let (done, total) = self.progress.progress();
        if total == 0 { 0.0 } else { done as f32 / total as f32 }
    }
}

/// The project's scene at a different frame rate (export can change it).
struct RateScene {
    inner: Arc<dyn Scene>,
    fps: f64,
}

impl Scene for RateScene {
    fn canvas(&self) -> Canvas {
        Canvas { frame_rate: self.fps, ..self.inner.canvas() }
    }
    fn duration(&self) -> Micros {
        self.inner.duration()
    }
    fn clips_at(&self, t: Micros) -> Vec<ClipDesc> {
        self.inner.clips_at(t)
    }
    fn transition_at(&self, t: Micros) -> Option<TransitionDesc> {
        self.inner.transition_at(t)
    }
}

fn even(v: u32) -> u32 {
    (v / 2 * 2).max(2)
}

/// `src` centred on a black `dw x dh` picture: a render whose shape is not
/// the output's (a landscape project in a vertical preset) gets bars rather
/// than a refusal from the encoder.
fn pad_rgba(src: &[u8], sw: u32, sh: u32, dw: u32, dh: u32) -> Vec<u8> {
    let (sw, sh, dw, dh) = (sw as usize, sh as usize, dw as usize, dh as usize);
    let mut out = vec![0u8; dw * dh * 4];
    for px in out.chunks_exact_mut(4) {
        px[3] = 255;
    }
    let (ox, oy) = (dw.saturating_sub(sw) / 2, dh.saturating_sub(sh) / 2);
    for y in 0..sh.min(dh) {
        let n = sw.min(dw) * 4;
        let d = ((y + oy) * dw + ox) * 4;
        out[d..d + n].copy_from_slice(&src[y * sw * 4..y * sw * 4 + n]);
    }
    out
}

/// A receiver of finished pictures (RGBA, width, height) instead of a file:
/// the DCP path encodes each one to JPEG 2000.
pub type Sink = Box<dyn FnMut(&[u8], u32, u32) -> Result<(), String> + Send>;

/// How a render differs from a plain export: render pieces for the queue
/// and for render nodes draw video only, with one encoder everywhere.
pub struct RunOptions {
    /// Mix and encode the sequence's sound (off for pieces: the sound is mixed once for the whole).
    pub audio: bool,
    /// Use Studio's own H.264 encoder even when the machine has hardware.
    pub software: bool,
    /// Hand pictures to this instead of writing `req.path`.
    pub sink: Option<Sink>,
}

impl Default for RunOptions {
    fn default() -> Self {
        RunOptions { audio: true, software: false, sink: None }
    }
}

/// Start exporting `doc` as described by `req`.
pub fn start(doc: &Doc, req: Request) -> Arc<Task> {
    start_with(doc, req, RunOptions::default())
}

/// [`start`] with options.
pub fn start_with(doc: &Doc, req: Request, opts: RunOptions) -> Arc<Task> {
    let registry = studio_render::source::SourceRegistry::new();
    crate::editor::render_view::register_sources_export(doc, &registry);
    crate::editor::render_view::install_text_renderer(doc.project.height);
    let model = Arc::new(doc.model.clone());
    let scene: Arc<dyn Scene> = Arc::new(RateScene { inner: Arc::new(studio_render::model_scene::ModelScene::new(model.clone())), fps: req.fps });
    let task = Arc::new(Task { progress: ExportProgress::new(), finished: AtomicBool::new(false), error: Mutex::new(None), path: req.path.clone() });
    let t = task.clone();
    std::thread::Builder::new()
        .name("studio-export".into())
        .spawn(move || {
            if let Err(e) = run(scene, registry, &model, &req, &t, opts) {
                *t.error.lock().unwrap_or_else(|e| e.into_inner()) = Some(e);
            }
            t.finished.store(true, Ordering::Release);
        })
        .expect("spawn export thread");
    task
}

/// The sound, mixed by `mix` on its own thread so a cancel stops the
/// export at once: a long film's mix takes a while and cannot stop half
/// way. `None` when cancelled (the mix runs out unseen, its result
/// dropped; the muxer, dropped unfinished, removes its file).
fn mixed<T: Send + 'static>(task: &Task, mix: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new().name("studio-export-mix".into()).spawn(move || {
        let _ = tx.send(mix());
    }).ok()?;
    loop {
        match rx.recv_timeout(std::time::Duration::from_millis(50)) {
            Ok(sound) => return Some(sound),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) if !task.progress.is_cancelled() => {}
            Err(_) => return None,
        }
    }
}

enum Writer {
    H264 { enc: Video, mux: studio_encode::mux::Muxer, track: usize, audio: Vec<(studio_encode::aac::Encoder, usize, Vec<f32>, usize)>, subs: Vec<(usize, crate::sound_desk::SubTrack)>, t0: Micros, t1: Micros },
    Clv(studio_media::clv::ClvWriter),
    Png { dir: PathBuf, n: u64 },
    Gif { w: studio_media::gif::GifWriter<std::io::BufWriter<std::fs::File>>, delay: u16 },
    Sink(Sink),
    /// ProRes, EXR, DPX, TIFF, 16-bit PNG, DNxHR (`export_pro`).
    Pro(crate::export_pro::ProWriter),
}

/// The machine's video hardware when it has one; Studio's own encoder
/// otherwise (or when STUDIO_SOFTWARE_ENCODE is set).
enum Video {
    Hardware(studio_hw::H264Encoder),
    Software(Box<studio_encode::h264::Encoder>),
    /// 10-bit HEVC, fed 16-bit pictures.
    HardwareHevc(studio_hw::HevcEncoder),
    /// AV1 from `studio-av1-enc`; `n` counts the pictures pushed (the pts).
    Av1 { enc: Box<studio_av1_enc::Encoder>, n: i64 },
}

fn run(scene: Arc<dyn Scene>, registry: Arc<studio_render::source::SourceRegistry>, model: &Arc<studio_model::Project>, req: &Request, task: &Task, mut opts: RunOptions) -> Result<(), String> {
    let started = std::time::Instant::now();
    let profile = std::env::var_os("STUDIO_EXPORT_PROFILE").is_some();
    let (mut t_yuv, mut t_enc, mut t_write) = (0f64, 0f64, 0f64);
    let mut t_mix = 0f64;
    let (w, h) = (even(req.size.0), even(req.size.1));
    let first = (req.range.0 as f64 / 1e6 * req.fps).floor() as u64;
    let last = ((req.range.1 as f64 / 1e6 * req.fps).ceil() as u64).max(first + 1);
    let rate = studio_model::time::FrameRate::from_f64(req.fps, 1e-3);
    let (fps_num, fps_den) = (rate.num, rate.den);
    let io = |e: std::io::Error| e.to_string();
    // An output folder that does not exist yet is made (the AI names
    // "Exports/AURA.mp4" before anything made "Exports").
    if let Some(parent) = req.path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|e| format!("could not make the output folder {}: {e}", parent.display()))?;
    }
    let mut writer = match req.format {
        _ if opts.sink.is_some() => Writer::Sink(opts.sink.take().expect("checked")),
        Format::H264 | Format::Hevc10 | Format::Av1 => {
            let mut cfg = studio_encode::h264::Config::new(w, h, fps_num, fps_den);
            cfg.bitrate_kbps = req.bitrate_kbps.clamp(1000, 200_000);
            let soft_info = |c: &studio_encode::h264::Config| studio_encode::h264::Encoder::new(c.clone()).map(|e| e.track_info());
            let hw = if matches!(req.format, Format::Hevc10 | Format::Av1) || opts.software || std::env::var_os("STUDIO_SOFTWARE_ENCODE").is_some() { None } else { studio_hw::H264Encoder::new(w, h, fps_num, fps_den, cfg.bitrate_kbps, cfg.gop) };
            let mux_opts = if req.format == Format::Av1 { studio_encode::mux::Options::mp4_av1() } else { studio_encode::mux::Options::mp4() };
            let mut mux = studio_encode::mux::Muxer::create(&req.path, mux_opts).map_err(io)?;
            let (enc, track) = match hw {
                _ if req.format == Format::Av1 => {
                    let mut acfg = studio_av1_enc::Config::new(w, h);
                    acfg.fps = (fps_num, fps_den);
                    acfg.rate = studio_av1_enc::RateMode::Cqp(Format::av1_base_q_idx(req.quality));
                    let enc = studio_av1_enc::Encoder::new(acfg).map_err(|e| format!("AV1 encoder: {e:?}"))?;
                    // BT.709 limited range, as the encoder's colour conversion writes it.
                    let info = studio_encode::mux::Av1TrackInfo { width: w, height: h, fps_num, fps_den, av1c: enc.av1c(), color: studio_encode::h264::ColorDesc { primaries: 1, transfer: 1, matrix: 1, full_range: false }, bitrate: req.bitrate_kbps.saturating_mul(1000) };
                    let track = mux.add_av1_video(info).map_err(io)?;
                    (Video::Av1 { enc: Box::new(enc), n: 0 }, track)
                }
                _ if req.format == Format::Hevc10 => {
                    let hw = studio_hw::HevcEncoder::new_main10(w, h, fps_num, fps_den, cfg.bitrate_kbps, cfg.gop).ok_or("This Mac's video hardware cannot write 10-bit HEVC; export as H.264 instead.")?;
                    let mut info = soft_info(&cfg).map_err(|e| format!("{e:?}"))?;
                    info.avcc = hw.hvcc();
                    info.hevc = true;
                    let track = mux.add_video(info).map_err(io)?;
                    (Video::HardwareHevc(hw), track)
                }
                Some(hw) => {
                    let mut info = soft_info(&cfg).map_err(|e| format!("{e:?}"))?;
                    info.avcc = hw.avcc();
                    let track = mux.add_video(info).map_err(io)?;
                    (Video::Hardware(hw), track)
                }
                None => {
                    let enc = studio_encode::h264::Encoder::new(cfg).map_err(|e| format!("{e:?}"))?;
                    let track = mux.add_video(enc.track_info()).map_err(io)?;
                    (Video::Software(Box::new(enc)), track)
                }
            };
            // Sound: mixed for the exact frame range so it matches the
            // picture's length; silent mixes get no audio track. One track
            // per language and layout, the original stereo first (it plays
            // by default; the rest are alternates in the player's menu).
            let t0 = (first as f64 * 1e6 / req.fps).round() as Micros;
            let t1 = (last as f64 * 1e6 / req.fps).round() as Micros;
            let m0 = std::time::Instant::now();
            let tracks = if opts.audio {
                let m = model.clone();
                let Some(t) = mixed(task, move || crate::sound_desk::render::tracks(&m, t0, t1)) else { return Ok(()) };
                t
            } else {
                Vec::new()
            };
            t_mix = m0.elapsed().as_secs_f64();
            let mut audio = Vec::new();
            for tr in tracks {
                if !tr.pcm.iter().any(|v| v.abs() > 1e-6) {
                    continue;
                }
                let ch = tr.layout.channels();
                let aenc = studio_encode::aac::Encoder::new(studio_encode::aac::Config { sample_rate: crate::sound_bridge::RATE, channels: ch as u16, bitrate: if ch > 2 { 384_000 } else { 192_000 } })?;
                let at = mux.add_audio(aenc.track_info()).map_err(io)?;
                mux.set_language(at, &tr.lang);
                mux.set_title(at, &tr.title);
                audio.push((aenc, at, tr.pcm, ch));
            }
            let mut subs = Vec::new();
            for t in if opts.audio { crate::sound_desk::Desk::of(model).subtitles } else { Vec::new() } {
                let st = mux.add_subtitles(studio_encode::mux::SubtitleInfo { width: w, height: h, font_size: (h / 18).clamp(12, 255) as u8 }).map_err(io)?;
                mux.set_language(st, studio_text::subtitles::iso639_2(&t.lang));
                mux.set_title(st, &if t.kind.is_empty() || t.kind == "subtitles" { t.name.clone() } else { format!("{} ({})", t.name, t.kind.to_uppercase()) });
                subs.push((st, t));
            }
            Writer::H264 { enc, mux, track, audio, subs, t0, t1 }
        }
        Format::Clv => Writer::Clv(studio_media::clv::ClvWriter::create(&req.path, w, h, fps_num, fps_den, 48_000, (req.quality as u8).clamp(1, 100)).map_err(io)?),
        Format::Png => {
            std::fs::create_dir_all(&req.path).map_err(io)?;
            Writer::Png { dir: req.path.clone(), n: 0 }
        }
        Format::Gif => {
            let file = std::io::BufWriter::new(std::fs::File::create(&req.path).map_err(io)?);
            Writer::Gif { w: studio_media::gif::GifWriter::new(file, w, h, 0).map_err(io)?, delay: (100.0 / req.fps).round().max(2.0) as u16 }
        }
        f @ (Format::ProRes422Hq | Format::ProRes4444 | Format::ProResProxy | Format::Exr | Format::Dpx | Format::Tiff16 | Format::Png16 | Format::DnxHrLb | Format::DnxHrSq | Format::DnxHrHq | Format::DnxHr | Format::DnxHr444) => {
            // Masters carry the mix as PCM; sequences have no sound.
            let pcm = if f.is_sequence() {
                Vec::new()
            } else {
                let t0 = (first as f64 * 1e6 / req.fps).round() as Micros;
                let t1 = (last as f64 * 1e6 / req.fps).round() as Micros;
                let pcm = crate::sound_bridge::mix_for_export(model, t0, t1);
                if pcm.iter().any(|v| v.abs() > 1e-6) { pcm } else { Vec::new() }
            };
            Writer::Pro(crate::export_pro::ProWriter::create(f, &req.path, model, w, h, (fps_num, fps_den), pcm)?)
        }
        // Sound only: the mix for the range, as AAC in M4A; no pictures drawn.
        Format::M4a => {
            let t0 = (first as f64 * 1e6 / req.fps).round() as Micros;
            let t1 = (last as f64 * 1e6 / req.fps).round() as Micros;
            let m = model.clone();
            let Some(pcm) = mixed(task, move || crate::sound_bridge::mix_for_export(&m, t0, t1)) else { return Ok(()) };
            let mut mux = studio_encode::mux::Muxer::create(&req.path, studio_encode::mux::Options::mp4()).map_err(io)?;
            let bitrate = if req.quality >= 90.0 { 256_000 } else { 192_000 };
            let mut aenc = studio_encode::aac::Encoder::new(studio_encode::aac::Config { sample_rate: crate::sound_bridge::RATE, channels: 2, bitrate })?;
            let at = mux.add_audio(aenc.track_info()).map_err(io)?;
            let mut units = aenc.push(&pcm);
            units.extend(aenc.flush());
            for u in &units {
                mux.write_sample(at, u, true).map_err(io)?;
            }
            mux.set_audio_samples(at, (pcm.len() / 2) as u64);
            mux.finish().map_err(io)?;
            return Ok(());
        }
    };
    // Diagnostics: STUDIO_EXPORT_WORKERS / _CHUNK / _CACHE_MB override the
    // defaults (to measure memory against speed).
    let env = |k: &str| std::env::var(k).ok().and_then(|v| v.parse::<u64>().ok());
    // Memory: frames being drawn and waiting are what grows (workers ×
    // chunk × frame size; a 4K frame is ~33 MB). Measured on a 14-core Mac:
    // past 3-4 workers export got no faster but much bigger, and a smaller
    // decode cache made it 2-3x slower. So: 3 workers, and chunks sized to
    // keep ~200 MB of frames in flight at any resolution.
    const WORKERS: usize = 3;
    const IN_FLIGHT_BYTES: u64 = 200 << 20;
    let frame_bytes = (w as u64 * h as u64 * 4).max(1);
    let chunk = (IN_FLIGHT_BYTES / (frame_bytes * WORKERS as u64)).clamp(2, 16);
    let cores = std::thread::available_parallelism().map_or(2, |n| n.get());
    let depth = if req.format == Format::Hevc10 || req.format.is_pro() { ExportDepth::Sixteen } else { ExportDepth::Eight };
    let mut settings = ExportSettings { frames: first..last, depth, size: Some((w, h)), workers: WORKERS.min(cores), chunk, ahead: 2, ..Default::default() };
    if req.format.is_pro() {
        // Masters are encoded for the project's display (EXR: scene-referred).
        settings.output.space = crate::export_pro::output_space(req.format, model);
        settings.output.dither = false;
    }
    // A 4K master of a 1080p project is drawn at 4K (the preview never
    // upscales; export may).
    settings.output.allow_upscale = true;
    if let Some(n) = env("STUDIO_EXPORT_WORKERS") {
        settings.workers = n as usize;
    }
    if let Some(n) = env("STUDIO_EXPORT_CHUNK") {
        settings.chunk = n;
    }
    let cache_bytes = env("STUDIO_EXPORT_CACHE_MB").map_or(256 << 20, |mb| (mb as usize) << 20);
    let mut failure: Option<String> = None;
    // STUDIO_EXPORT_SHARED_CACHE=1: one decode cache for every worker.
    let shared = std::env::var_os("STUDIO_EXPORT_SHARED_CACHE").is_some().then(|| studio_render::source::DecodeCache::new(cache_bytes));
    let make = move || studio_render::best_renderer(studio_render::source::Sources::new(registry.clone(), shared.clone().unwrap_or_else(|| studio_render::source::DecodeCache::new(cache_bytes))));
    let report = Exporter::run(scene, settings, task.progress.clone(), make, |f: ExportFrame| {
        if failure.is_some() {
            return;
        }
        // 10-bit HEVC is fed 16-bit pictures; everything else 8-bit.
        let (rgba, rgba16) = match f.pixels {
            ExportPixels::Rgba8(v) => (v, None),
            ExportPixels::Rgba16(v) => (Vec::new(), Some(v)),
        };
        // A picture of another shape than the output is centred on black (not for the DCP sink: it takes the picture as drawn).
        let (fw, fh, rgba) = if (f.width != w || f.height != h) && rgba16.is_none() && !matches!(writer, Writer::Sink(_)) { (w, h, pad_rgba(&rgba, f.width, f.height, w, h)) } else { (f.width, f.height, rgba) };
        let r = (|| -> Result<(), String> {
            match &mut writer {
                Writer::H264 { enc, mux, track, .. } => {
                    if std::env::var_os("STUDIO_EXPORT_RENDER_ONLY").is_some() {
                        return Ok(());
                    }
                    match enc {
                        Video::HardwareHevc(hw) => {
                            let b = std::time::Instant::now();
                            let outs = hw.encode_rgba16(rgba16.as_deref().ok_or("10-bit HEVC needs 16-bit pictures")?)?;
                            let c = std::time::Instant::now();
                            for o in outs {
                                mux.write_sample(*track, &o.data, o.keyframe).map_err(|e| e.to_string())?;
                            }
                            t_enc += (c - b).as_secs_f64();
                            t_write += c.elapsed().as_secs_f64();
                            Ok(())
                        }
                        Video::Av1 { enc, n } => {
                            let a = std::time::Instant::now();
                            let pic = studio_av1_enc::color::from_rgba8(&rgba, fw, fh, 8, false);
                            let b = std::time::Instant::now();
                            let pkts = enc.push_frame(&pic, *n).map_err(|e| format!("AV1 encoder: {e:?}"))?;
                            *n += 1;
                            let c = std::time::Instant::now();
                            for p in pkts {
                                mux.write_temporal_unit(*track, &p.data).map_err(|e| e.to_string())?;
                            }
                            t_yuv += (b - a).as_secs_f64();
                            t_enc += (c - b).as_secs_f64();
                            t_write += c.elapsed().as_secs_f64();
                            Ok(())
                        }
                        Video::Hardware(hw) => {
                            let b = std::time::Instant::now();
                            let outs = hw.encode_rgba(&rgba)?;
                            let c = std::time::Instant::now();
                            for o in outs {
                                mux.write_sample(*track, &o.data, o.keyframe).map_err(|e| e.to_string())?;
                            }
                            t_enc += (c - b).as_secs_f64();
                            t_write += c.elapsed().as_secs_f64();
                            Ok(())
                        }
                        Video::Software(enc) => {
                            let a = std::time::Instant::now();
                            let pic = studio_encode::yuv::Yuv420::from_rgba(&rgba, fw, fh);
                            let b = std::time::Instant::now();
                            let out = enc.encode(&pic);
                            let c = std::time::Instant::now();
                            let r = mux.write_sample(*track, &out.data, out.keyframe).map_err(|e| e.to_string());
                            t_yuv += (b - a).as_secs_f64();
                            t_enc += (c - b).as_secs_f64();
                            t_write += c.elapsed().as_secs_f64();
                            r
                        }
                    }
                }
                Writer::Clv(c) => c.push_frame(&studio_media::RgbaImage { width: fw, height: fh, pixels: rgba }).map_err(|e| e.to_string()),
                Writer::Png { dir, n } => {
                    let bytes = studio_media::png::encode(&studio_media::RgbaImage { width: fw, height: fh, pixels: rgba });
                    *n += 1;
                    std::fs::write(dir.join(format!("frame_{:05}.png", *n)), bytes).map_err(|e| e.to_string())
                }
                Writer::Gif { w, delay } => w.push(&studio_media::RgbaImage { width: fw, height: fh, pixels: rgba }, *delay, &Default::default()).map_err(|e| e.to_string()),
                Writer::Sink(sink) => sink(&rgba, f.width, f.height),
                Writer::Pro(p) => p.push(rgba16.as_deref().ok_or("masters need 16-bit pictures")?, f.width, f.height),
            }
        })();
        if let Err(e) = r {
            failure = Some(e);
            task.progress.cancel();
        }
    });
    if profile {
        let total = started.elapsed().as_secs_f64();
        eprintln!("export profile ({}): {} frames in {total:.1}s ({:.1} fps) | sound mix {t_mix:.1}s | rgb->yuv {t_yuv:.1}s | h264 encode {t_enc:.1}s | write {t_write:.1}s | rest (render, waiting) {:.1}s", if matches!(&writer, Writer::H264 { enc: Video::Hardware(_) | Video::HardwareHevc(_), .. }) { "hardware encode" } else { "software encode" }, last - first, (last - first) as f64 / (total - t_mix).max(1e-3), total - t_mix - t_yuv - t_enc - t_write);
    }
    if let Some(e) = failure {
        return Err(e);
    }
    if report.outcome == studio_render::export::ExportOutcome::Cancelled {
        // Leave nothing half-written behind.
        if let Writer::Pro(p) = writer {
            p.abandon(&req.path);
        }
        let _ = std::fs::remove_file(&req.path);
        return Ok(());
    }
    match writer {
        Writer::H264 { mut mux, audio, enc, track, subs, t0, t1 } => {
            // What the hardware still holds.
            let held = match enc {
                Video::Hardware(mut hw) => hw.finish()?,
                Video::HardwareHevc(mut hw) => hw.finish()?,
                Video::Software(_) => Vec::new(),
                Video::Av1 { mut enc, .. } => {
                    for p in enc.flush().map_err(|e| format!("AV1 encoder: {e:?}"))? {
                        mux.write_temporal_unit(track, &p.data).map_err(io)?;
                    }
                    Vec::new()
                }
            };
            for o in held {
                mux.write_sample(track, &o.data, o.keyframe).map_err(io)?;
            }
            for (mut aenc, at, pcm, ch) in audio {
                let frames = (pcm.len() / ch) as u64;
                let mut units = aenc.push(&pcm);
                units.extend(aenc.flush());
                for u in &units {
                    mux.write_sample(at, u, true).map_err(io)?;
                }
                mux.set_audio_samples(at, frames);
            }
            // Subtitles in the exported range, timed from its start.
            for (st, t) in &subs {
                let (a, b) = (t0 as f64 / 1e3, t1 as f64 / 1e3);
                for c in &t.cues {
                    let (s, e) = ((c.start as f64 * 1e3).max(a), (c.end as f64 * 1e3).min(b));
                    if e > s {
                        mux.write_subtitle(*st, (s - a).round() as u64, (e - a).round() as u64, &c.text).map_err(io)?;
                    }
                }
            }
            mux.finish().map_err(io)?;
            if opts.audio {
                crate::sound_desk::subs::write_sidecars(model, &req.path, t0, t1);
            }
        }
        Writer::Clv(c) => {
            c.finish().map_err(io)?;
        }
        Writer::Png { .. } => {}
        Writer::Gif { w, .. } => {
            w.finish().map_err(io)?;
        }
        Writer::Sink(_) => {}
        Writer::Pro(p) => p.finish(&req.path)?,
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view;

    fn wait(task: &Task) {
        let start = std::time::Instant::now();
        while !task.finished.load(Ordering::Acquire) {
            assert!(start.elapsed().as_secs() < 300, "export timed out");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(task.error.lock().unwrap_or_else(|e| e.into_inner()).is_none(), "{:?}", task.error.lock().unwrap_or_else(|e| e.into_inner()));
    }

    #[test]
    fn export_makes_a_missing_output_folder() {
        let doc = Doc::from_view(&view::demo());
        let root = std::env::temp_dir().join(format!("studio-export-newdir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let path = root.join("Exports").join("deeper").join("out.mp4");
        assert!(!path.parent().unwrap().exists());
        let task = start(&doc, Request { path: path.clone(), format: Format::H264, size: (320, 180), fps: 30.0, range: (0, view::SEC / 4), quality: 60.0, bitrate_kbps: 2000 });
        wait(&task);
        assert!(path.exists(), "the file is written into the folder that was made");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn exported_mp4_carries_the_mixed_sound() {
        // A 2-second 440 Hz tone at -6 dBFS on the timeline of a new project.
        let dir = std::env::temp_dir().join(format!("studio-export-audio-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let wav_path = dir.join("tone.wav");
        let rate = 48_000u32;
        let samples: Vec<f32> = (0..rate * 2).flat_map(|i| {
            let v = 0.5 * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / rate as f32).sin();
            [v, v]
        }).collect();
        let wav = studio_media::wav::Wav { sample_rate: rate, channels: 2, samples };
        std::fs::write(&wav_path, studio_media::wav::encode_pcm16(&wav)).unwrap();
        let mut doc = crate::services::project::new_project(crate::services::project::canvas("Landscape").unwrap());
        crate::services::project::import_to_timeline(&mut doc, &[wav_path.clone()]).unwrap();

        // The live plan sees the clip.
        let plan = crate::sound_bridge::plan(&doc.model);
        assert_eq!(plan.clips.len(), 1, "the tone becomes one mixer clip");

        let path = dir.join("with_sound.mp4");
        let task = start(&doc, Request { path: path.clone(), format: Format::H264, size: (320, 180), fps: 30.0, range: (0, 2 * view::SEC), quality: 60.0, bitrate_kbps: 2000 });
        wait(&task);

        // Read the exported audio back with our own demuxer + AAC decoder.
        let back = crate::sound_bridge::decoded(path.to_str().unwrap()).expect("exported file has an audio track");
        let secs = back.left.len() as f64 / back.rate as f64;
        assert!((secs - 2.0).abs() < 0.05, "audio length {secs:.3}s matches the 2 s export");
        let mid = &back.left[back.left.len() / 4..back.left.len() * 3 / 4];
        let rms = (mid.iter().map(|v| v * v).sum::<f32>() / mid.len() as f32).sqrt();
        // A 0.5-amplitude sine has RMS 0.354; allow for the limiter and codec.
        assert!((rms - 0.354).abs() < 0.06, "tone level survives the mix and AAC round trip (rms {rms:.3})");
    }

    #[test]
    fn cancelling_stops_soon_and_leaves_no_file() {
        let doc = Doc::from_view(&view::demo());
        let dir = std::env::temp_dir().join(format!("studio-export-cancel-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("cancelled.mp4");
        let task = start(&doc, Request { path: path.clone(), format: Format::H264, size: (1920, 1080), fps: 30.0, range: (0, 20 * view::SEC), quality: 80.0, bitrate_kbps: 12000 });
        std::thread::sleep(std::time::Duration::from_millis(400));
        task.progress.cancel();
        let asked = std::time::Instant::now();
        wait(&task);
        assert!(asked.elapsed().as_secs_f64() < 5.0, "stopped {:.1}s after cancel", asked.elapsed().as_secs_f64());
        assert!(task.fraction() < 0.9, "it was stopped, not finished");
        assert!(!path.exists(), "no half-written file is left");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn audio_only_export_is_the_mix_in_m4a() {
        let dir = std::env::temp_dir().join(format!("studio-export-m4a-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let wav_path = dir.join("tone.wav");
        let samples: Vec<f32> = (0..48_000 * 2).flat_map(|i| {
            let v = 0.5 * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 48_000.0).sin();
            [v, v]
        }).collect();
        std::fs::write(&wav_path, studio_media::wav::encode_pcm16(&studio_media::wav::Wav { sample_rate: 48_000, channels: 2, samples })).unwrap();
        let mut doc = crate::services::project::new_project(crate::services::project::canvas("Landscape").unwrap());
        crate::services::project::import_to_timeline(&mut doc, &[wav_path]).unwrap();
        let path = dir.join("sound.m4a");
        let task = start(&doc, Request { path: path.clone(), format: Format::M4a, size: (320, 180), fps: 30.0, range: (0, 2 * view::SEC), quality: 80.0, bitrate_kbps: 0 });
        wait(&task);
        let dm = studio_mp4::open(&path).unwrap();
        assert!(dm.movie().tracks.iter().all(|t| t.kind != studio_mp4::demux::TrackKind::Video), "sound only");
        let back = crate::sound_bridge::decoded(path.to_str().unwrap()).expect("an AAC track");
        assert!((back.left.len() as f64 / back.rate as f64 - 2.0).abs() < 0.05);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn exports_h264_that_our_decoder_reads_back() {
        let doc = Doc::from_view(&view::demo());
        let dir = std::env::temp_dir().join(format!("studio-export-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("out.mp4");
        let task = start(&doc, Request { path: path.clone(), format: Format::H264, size: (320, 180), fps: 30.0, range: (0, view::SEC / 2), quality: 60.0, bitrate_kbps: 2000 });
        wait(&task);
        assert!(task.fraction() > 0.99);
        let probe = crate::media_io::probe(&path).unwrap();
        let v = probe.video.unwrap();
        assert_eq!((v.width, v.height), (320, 180));
        let frame = crate::media_io::frame(&path, 0.2, 64, 36, false).unwrap();
        assert!(frame.pixels.iter().any(|p| p.r() > 20 || p.b() > 20));

        // PNG sequence: one file per frame.
        let seq = dir.join("frames");
        let task = start(&doc, Request { path: seq.clone(), format: Format::Png, size: (160, 90), fps: 10.0, range: (0, view::SEC / 2), quality: 100.0, bitrate_kbps: 0 });
        wait(&task);
        assert_eq!(std::fs::read_dir(&seq).unwrap().count(), 5);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A new project with a 2 s tone, then a gradient picture, on the timeline.
    fn picture_and_tone(dir: &std::path::Path) -> Doc {
        let (pw, ph) = (320u32, 180u32);
        let pixels: Vec<u8> = (0..pw * ph).flat_map(|i| { let (x, y) = (i % pw, i / pw); [(x * 255 / pw) as u8, (y * 255 / ph) as u8, 140, 255] }).collect();
        std::fs::write(dir.join("grad.png"), studio_media::png::encode(&studio_media::RgbaImage { width: pw, height: ph, pixels })).unwrap();
        let samples: Vec<f32> = (0..48_000 * 2).flat_map(|i| {
            let v = 0.5 * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 48_000.0).sin();
            [v, v]
        }).collect();
        std::fs::write(dir.join("tone.wav"), studio_media::wav::encode_pcm16(&studio_media::wav::Wav { sample_rate: 48_000, channels: 2, samples })).unwrap();
        let mut doc = crate::services::project::new_project(crate::services::project::canvas("Landscape").unwrap());
        crate::services::project::import_to_timeline(&mut doc, &[dir.join("tone.wav"), dir.join("grad.png")]).unwrap();
        doc
    }

    #[test]
    fn exports_av1_in_mp4_that_studio_av1_reads_back() {
        let dir = std::env::temp_dir().join(format!("studio-export-av1-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let doc = picture_and_tone(&dir);
        let path = dir.join("out.mp4");
        let task = start(&doc, Request { path: path.clone(), format: Format::Av1, size: (320, 180), fps: 24.0, range: (0, 4 * view::SEC), quality: 70.0, bitrate_kbps: 2000 });
        wait(&task);
        assert!(task.fraction() > 0.99);

        let mut dmx = studio_mp4::open(&path).unwrap();
        let movie = dmx.movie().clone();
        let v = movie.video().expect("a video track").clone();
        assert_eq!(&v.fourcc, b"av01");
        assert_eq!((v.coded_width, v.coded_height), (320, 180));
        assert_eq!(v.samples.len(), 96, "4 s at 24 fps");
        let secs = v.samples.iter().map(|s| s.duration as f64).sum::<f64>() / v.timescale as f64;
        assert!((secs - 4.0).abs() < 0.05, "video lasts {secs:.3} s");
        assert!(v.samples.iter().all(|s| s.sync), "key frames only");
        assert!(movie.audio().is_some(), "the sound is muxed like the other MP4 formats");

        // Decode every sample with studio-av1.
        let mut dec = studio_av1::Decoder::new();
        let mut pics = Vec::new();
        for s in &v.samples {
            let data = dmx.read_sample(s).unwrap();
            pics.extend(dec.decode(&data).unwrap());
        }
        // The decoder holds back pictures still in flight.
        pics.extend(dec.flush());
        assert_eq!(pics.len(), 96);
        assert!(pics.iter().all(|p| (p.width, p.height) == (320, 180)));
        let luma_mean = pics[80].planes[0].iter().map(|&y| y as f64).sum::<f64>() / pics[80].planes[0].len() as f64;
        assert!(luma_mean > 30.0, "the picture is not black (mean luma {luma_mean:.0})");

        // The sound reads back, and the import path accepts the file.
        let back = crate::sound_bridge::decoded(path.to_str().unwrap()).expect("an audio track");
        assert!((back.left.len() as f64 / back.rate as f64 - 4.0).abs() < 0.05, "the sound spans the exported range");
        let probe = crate::media_io::probe(&path).unwrap();
        let pv = probe.video.expect("probe sees the video");
        assert_eq!((pv.width, pv.height), (320, 180));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Export QC reads H.264 and HEVC only (`studio-deliver` filecheck); this passes
    /// once it has an AV1 branch. Run with `--ignored`.
    #[test]
    #[ignore = "needs an AV1 branch in studio-deliver's filecheck"]
    fn av1_export_passes_qc() {
        let dir = std::env::temp_dir().join(format!("studio-export-av1-qc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let doc = picture_and_tone(&dir);
        let path = dir.join("out.mp4");
        let task = start(&doc, Request { path: path.clone(), format: Format::Av1, size: (320, 180), fps: 24.0, range: (0, 2 * view::SEC), quality: 70.0, bitrate_kbps: 2000 });
        wait(&task);
        let report = studio_deliver::filecheck::check_file(&path, &studio_deliver::qc::Profile::default(), &Default::default()).expect("QC reads the file");
        assert!(report.passed, "{report:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `cargo test --release ... av1_export_timing -- --ignored --nocapture`
    #[test]
    #[ignore = "timing report"]
    fn av1_export_timing() {
        let dir = std::env::temp_dir().join(format!("studio-export-av1-time-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let doc = picture_and_tone(&dir);
        for (format, name) in [(Format::Av1, "av1.mp4"), (Format::H264, "h264.mp4")] {
            let path = dir.join(name);
            let t = std::time::Instant::now();
            let task = start(&doc, Request { path: path.clone(), format, size: (1920, 1080), fps: 30.0, range: (0, 5 * view::SEC), quality: 70.0, bitrate_kbps: 8000 });
            wait(&task);
            eprintln!("TIMING {format:?}: 5 s 1080p30 in {:.1} s, {} bytes", t.elapsed().as_secs_f64(), std::fs::metadata(&path).unwrap().len());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
