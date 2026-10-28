// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Export jobs: presets, the request a preset makes for a project, and a
//! running job with progress events, ETA, cancel and "Reveal in Finder".
//!
//! Rendering and encoding run on their own thread (`export_run`); the UI
//! polls `ExportJob::poll` each frame and gets the events since the last
//! poll. Needs the `render` feature (the default).

use crate::doc::Doc;
use crate::export_run::{self, Request};
use crate::formats::Format;
use crate::view::Time;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

/// How a preset picks the output size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Size {
    /// Exactly this size.
    Fixed(u32, u32),
    /// Scale the project so its short side is this many pixels, keeping its
    /// shape: "1080p" is 1920 × 1080 for a landscape project and
    /// 1080 × 1920 for a vertical one.
    ShortSide(u32),
    /// The project's own size.
    Project,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Preset {
    pub name: &'static str,
    pub sub: &'static str,
    pub format: Format,
    pub size: Size,
    /// 0..100.
    pub quality: f32,
    /// False until the format can be written (audio-only waits for the
    /// audio mix).
    pub available: bool,
}

pub const PRESETS: &[Preset] = &[
    Preset { name: "Social 1080p", sub: "H.264 · 1080p in the project's shape", format: Format::H264, size: Size::ShortSide(1080), quality: 70.0, available: true },
    Preset { name: "4K master", sub: "H.264 · 2160p", format: Format::H264, size: Size::ShortSide(2160), quality: 90.0, available: true },
    Preset { name: "Vertical", sub: "H.264 · 1080 × 1920", format: Format::H264, size: Size::Fixed(1080, 1920), quality: 70.0, available: true },
    Preset { name: "GIF", sub: "Looping · 720p", format: Format::Gif, size: Size::ShortSide(720), quality: 50.0, available: true },
    Preset { name: "Intermediate", sub: "Zornov codec · CLV", format: Format::Clv, size: Size::Project, quality: 95.0, available: true },
    Preset { name: "PNG sequence", sub: "Lossless stills", format: Format::Png, size: Size::Project, quality: 100.0, available: true },
    Preset { name: "HEVC 10-bit", sub: "HEVC Main 10 · smooth gradients", format: Format::Hevc10, size: Size::ShortSide(2160), quality: 90.0, available: true },
    Preset { name: "Audio only", sub: "AAC · M4A", format: Format::M4a, size: Size::Project, quality: 80.0, available: true },
    Preset { name: "ProRes 422 HQ", sub: "Master · 10-bit 4:2:2 · PCM · timecode", format: Format::ProRes422Hq, size: Size::Project, quality: 100.0, available: true },
    Preset { name: "ProRes 4444", sub: "Master · 12-bit 4:4:4 · PCM · timecode", format: Format::ProRes4444, size: Size::Project, quality: 100.0, available: true },
    Preset { name: "ProRes Proxy", sub: "Review / offline · 1080p", format: Format::ProResProxy, size: Size::ShortSide(1080), quality: 60.0, available: true },
    Preset { name: "EXR sequence (ACES)", sub: "ACES2065-1 half float · VFX and colour", format: Format::Exr, size: Size::Project, quality: 100.0, available: true },
    Preset { name: "DPX sequence", sub: "10-bit · film and finishing", format: Format::Dpx, size: Size::Project, quality: 100.0, available: true },
    Preset { name: "TIFF sequence", sub: "16-bit", format: Format::Tiff16, size: Size::Project, quality: 100.0, available: true },
    Preset { name: "PNG 16-bit sequence", sub: "16-bit lossless", format: Format::Png16, size: Size::Project, quality: 100.0, available: true },
    Preset { name: "DNxHR LB", sub: "Offline · 8-bit 4:2:2 · PCM · timecode", format: Format::DnxHrLb, size: Size::Project, quality: 100.0, available: true },
    Preset { name: "DNxHR SQ", sub: "Edit · 8-bit 4:2:2 · PCM · timecode", format: Format::DnxHrSq, size: Size::Project, quality: 100.0, available: true },
    Preset { name: "DNxHR HQ", sub: "Finishing · 8-bit 4:2:2 · PCM · timecode", format: Format::DnxHrHq, size: Size::Project, quality: 100.0, available: true },
    Preset { name: "DNxHR HQX", sub: "Master · 10-bit 4:2:2 · PCM · timecode", format: Format::DnxHr, size: Size::Project, quality: 100.0, available: true },
    Preset { name: "DNxHR 444", sub: "Master · 10-bit 4:4:4 · PCM · timecode", format: Format::DnxHr444, size: Size::Project, quality: 100.0, available: true },
];

pub fn preset(name: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|p| p.name == name)
}

/// Output size for a project, rounded to even numbers (H.264 needs them).
pub fn output_size(size: Size, project: (u32, u32)) -> (u32, u32) {
    let even = |v: f64| ((v / 2.0).round() as u32 * 2).max(2);
    let (pw, ph) = (project.0.max(2) as f64, project.1.max(2) as f64);
    match size {
        Size::Fixed(w, h) => (w, h),
        Size::Project => (even(pw), even(ph)),
        Size::ShortSide(s) => {
            let k = s as f64 / pw.min(ph);
            (even(pw * k), even(ph * k))
        }
    }
}

/// Suggested H.264 bitrate in kb/s for a size and rate (by resolution).
pub fn suggested_kbps(w: u32, h: u32, fps: f64) -> u32 {
    let px = w as f64 * h as f64;
    let base = if px <= 1280.0 * 720.0 { 6.0 } else if px <= 1920.0 * 1080.0 { 12.0 } else if px <= 2560.0 * 1440.0 { 20.0 } else { 45.0 };
    ((if fps > 40.0 { base * 1.5 } else { base }) * 1000.0) as u32
}

/// Frames an export of `range` at `fps` writes (the exporter renders
/// `floor(start·fps) .. ceil(end·fps)`).
pub fn frame_count(range: (Time, Time), fps: f64) -> u64 {
    let first = (range.0 as f64 / 1e6 * fps).floor() as u64;
    let last = ((range.1 as f64 / 1e6 * fps).ceil() as u64).max(first + 1);
    last - first
}

/// The request a preset makes for the whole sequence (or `range`).
pub fn request(doc: &Doc, p: &Preset, path: PathBuf, range: Option<(Time, Time)>) -> Request {
    let proj = &doc.project;
    let fps = proj.fps as f64;
    let size = output_size(p.size, (proj.width, proj.height));
    let range = range.unwrap_or((0, super::transport::duration(doc)));
    Request { path, format: p.format, size, fps, range, quality: p.quality, bitrate_kbps: suggested_kbps(size.0, size.1, fps) }
}

/// What happened since the last poll.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Progress { fraction: f32, eta_secs: Option<f32> },
    Finished { path: PathBuf },
    Failed(String),
    Cancelled,
}

/// A running (or finished) export.
pub struct ExportJob {
    pub task: Arc<export_run::Task>,
    pub request: Request,
    started: Instant,
    last: f32,
    ended: bool,
}

impl ExportJob {
    /// Start exporting on a background thread.
    pub fn start(doc: &Doc, request: Request) -> Self {
        let task = export_run::start(doc, request.clone());
        Self { task, request, started: Instant::now(), last: -1.0, ended: false }
    }

    pub fn fraction(&self) -> f32 {
        self.task.fraction()
    }

    /// Seconds left, from the rate so far (none until 2 % is done).
    pub fn eta_secs(&self) -> Option<f32> {
        let f = self.fraction();
        (f > 0.02 && f < 1.0).then(|| self.started.elapsed().as_secs_f32() * (1.0 - f) / f)
    }

    pub fn is_finished(&self) -> bool {
        self.task.finished.load(std::sync::atomic::Ordering::Acquire)
    }

    pub fn cancel(&self) {
        self.task.progress.cancel();
    }

    /// How the job ended, or `None` while it runs; unlike [`poll`](Self::poll)
    /// it can be asked again.
    pub fn outcome(&self) -> Option<Event> {
        if !self.is_finished() {
            return None;
        }
        Some(if let Some(e) = self.task.error.lock().unwrap_or_else(|e| e.into_inner()).clone() {
            Event::Failed(e)
        } else if self.task.progress.is_cancelled() {
            Event::Cancelled
        } else {
            Event::Finished { path: self.request.path.clone() }
        })
    }

    /// Events since the last call: progress when it moved, then exactly one
    /// of Finished / Failed / Cancelled when the job ends.
    pub fn poll(&mut self) -> Vec<Event> {
        let mut out = Vec::new();
        if self.ended {
            return out;
        }
        let f = self.fraction();
        if f > self.last {
            self.last = f;
            out.push(Event::Progress { fraction: f, eta_secs: self.eta_secs() });
        }
        if let Some(e) = self.outcome() {
            self.ended = true;
            out.push(e);
        }
        out
    }

    /// Block until the job ends (tests and command-line use).
    pub fn wait(&mut self) -> Event {
        loop {
            if let Some(e) = self.poll().into_iter().find(|e| !matches!(e, Event::Progress { .. })) {
                return e;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    /// Like [`wait`](Self::wait), but gives up after `limit`: `None` while
    /// the render is still going (it carries on; poll it).
    pub fn wait_for(&mut self, limit: std::time::Duration) -> Option<Event> {
        let until = Instant::now() + limit;
        loop {
            if let Some(e) = self.poll().into_iter().find(|e| !matches!(e, Event::Progress { .. })) {
                return Some(e);
            }
            if Instant::now() >= until {
                return None;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
}

/// Show a file in the Finder.
pub fn reveal(path: &Path) -> Result<(), String> {
    std::process::Command::new("open").arg("-R").arg(path).status().map_err(|e| e.to_string()).and_then(|s| if s.success() { Ok(()) } else { Err(format!("open -R exited with {s}")) })
}

/// A file name that does not overwrite anything in `dir`: "Name.mp4",
/// "Name 2.mp4", …
pub fn free_path(dir: &Path, name: &str, format: Format) -> PathBuf {
    let ext = format.extension().trim_start_matches('.').trim_end_matches('/');
    let make = |n: u32| {
        let stem = if n == 1 { name.to_owned() } else { format!("{name} {n}") };
        if ext.is_empty() { dir.join(stem) } else { dir.join(format!("{stem}.{ext}")) }
    };
    (1..).map(make).find(|p| !p.exists()).unwrap_or_else(|| make(1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view;

    #[test]
    fn social_1080p_follows_the_project_shape() {
        let p = preset("Social 1080p").unwrap();
        assert_eq!(output_size(p.size, (1920, 1080)), (1920, 1080));
        assert_eq!(output_size(p.size, (1080, 1920)), (1080, 1920));
        assert_eq!(output_size(p.size, (1080, 1350)), (1080, 1350));
        assert_eq!(output_size(Size::ShortSide(720), (1001, 1001)), (720, 720));
    }

    #[test]
    fn frame_count_matches_the_exporter() {
        assert_eq!(frame_count((0, view::SEC), 30.0), 30);
        assert_eq!(frame_count((0, view::SEC / 2), 30.0), 15);
        assert_eq!(frame_count((0, 1_000_001), 30.0), 31);
    }

    #[test]
    fn export_job_reports_progress_then_finishes() {
        let doc = Doc::from_view(&view::demo());
        let dir = std::env::temp_dir().join(format!("studio-export-job-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = free_path(&dir, "Job test", Format::H264);
        let mut req = request(&doc, preset("Social 1080p").unwrap(), path.clone(), Some((0, view::SEC / 3)));
        req.size = (320, 180);
        let mut job = ExportJob::start(&doc, req);
        let mut events = Vec::new();
        loop {
            let e = job.poll();
            let end = e.iter().any(|e| !matches!(e, Event::Progress { .. }));
            events.extend(e);
            if end {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(events.iter().any(|e| matches!(e, Event::Progress { .. })));
        assert_eq!(events.last(), Some(&Event::Finished { path: path.clone() }));
        assert!(job.poll().is_empty(), "no events after the end");
        assert!(path.exists());
        assert_eq!(free_path(&dir, "Job test", Format::H264), dir.join("Job test 2.mp4"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dnxhr_presets_are_on_and_write_quicktime_files() {
        let names = ["DNxHR LB", "DNxHR SQ", "DNxHR HQ", "DNxHR HQX", "DNxHR 444"];
        let doc = Doc::from_view(&view::demo());
        let dir = std::env::temp_dir().join(format!("studio-export-dnx-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for name in names {
            let p = preset(name).unwrap();
            assert!(p.available, "{name}");
            for text in [p.name, p.sub, p.format.label()] {
                let l = text.to_lowercase();
                assert!(!l.contains("ffmpeg") && !l.contains("avid") && !l.contains("not available") && !l.contains("needs"), "{text}");
            }
            assert_eq!(p.format.extension(), ".mov");
            let path = free_path(&dir, name, p.format);
            let mut req = request(&doc, p, path.clone(), Some((0, view::SEC / 4)));
            req.size = (320, 180);
            let mut job = ExportJob::start(&doc, req);
            assert!(matches!(job.wait(), Event::Finished { .. }), "{name}");
            let bytes = std::fs::read(&path).unwrap();
            assert!(bytes.windows(4).any(|w| w == b"AVdh"), "{name}: sample entry");
            // The frames are DNxHR coding units: header prefix with version 3, then the compression ID.
            let at = bytes.windows(5).position(|w| w == [0, 0, 2, 0x80, 3]).unwrap_or_else(|| panic!("{name}: no frame header"));
            let cid = u32::from_be_bytes(bytes[at + 0x28..at + 0x2C].try_into().unwrap());
            let want = match name {
                "DNxHR LB" => 1274,
                "DNxHR SQ" => 1273,
                "DNxHR HQ" => 1272,
                "DNxHR HQX" => 1271,
                _ => 1270,
            };
            assert_eq!(cid, want, "{name}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cancelled_export_leaves_no_file() {
        let doc = Doc::from_view(&view::demo());
        let path = std::env::temp_dir().join(format!("studio-export-cancel-{}.mp4", std::process::id()));
        let mut req = request(&doc, preset("Social 1080p").unwrap(), path.clone(), None);
        req.size = (320, 180);
        let mut job = ExportJob::start(&doc, req);
        job.cancel();
        assert_eq!(job.wait(), Event::Cancelled);
        assert!(!path.exists());
    }
}
