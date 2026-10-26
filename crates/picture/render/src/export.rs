// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Export rendering: every frame of a range, in order, from a pool of
//! workers with back-pressure.
//!
//! Frames are handed out to workers in contiguous chunks (so each worker
//! decodes a run of consecutive pictures and the decode cache is not
//! thrashed), rendered in parallel, and delivered to the sink strictly in
//! order through a reorder buffer. The number of chunks in flight is
//! bounded: when the sink (the encoder) is slower than the renderers, the
//! workers wait instead of piling up frames in memory.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::compositor::FrameRenderer;
use crate::output::{EncodedPixels, OutputDepth, OutputSettings};
use crate::scene::{Micros, Scene};

/// Pixel format of exported frames.
pub type ExportDepth = OutputDepth;

/// Export settings.
#[derive(Clone, Debug)]
pub struct ExportSettings {
    /// First frame index (inclusive) and one past the last.
    pub frames: std::ops::Range<u64>,
    pub output: OutputSettings,
    pub depth: ExportDepth,
    /// Workers; 0 picks one per available core (at most 8).
    pub workers: usize,
    /// Frames per chunk handed to a worker.
    pub chunk: u64,
    /// Chunks that may be rendered ahead of the sink.
    pub ahead: usize,
    /// Output size; `None` = canvas.
    pub size: Option<(u32, u32)>,
}

impl Default for ExportSettings {
    fn default() -> Self {
        ExportSettings { frames: 0..0, output: OutputSettings::default(), depth: ExportDepth::Eight, workers: 0, chunk: 16, ahead: 4, size: None }
    }
}

/// Pixels of one exported frame.
pub type ExportPixels = EncodedPixels;

/// One exported frame with its index.
#[derive(Clone, Debug)]
pub struct ExportFrame {
    pub index: u64,
    pub time: Micros,
    pub width: u32,
    pub height: u32,
    pub pixels: ExportPixels,
}

/// Progress and cancellation handle shared with the UI.
#[derive(Default)]
pub struct ExportProgress {
    done: AtomicU64,
    total: AtomicU64,
    cancel: AtomicBool,
}

impl ExportProgress {
    pub fn new() -> Arc<ExportProgress> {
        Arc::new(ExportProgress::default())
    }

    /// Frames delivered to the sink and total frames.
    pub fn progress(&self) -> (u64, u64) {
        (self.done.load(Ordering::Relaxed), self.total.load(Ordering::Relaxed))
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

/// Result of an export run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExportOutcome {
    Finished,
    Cancelled,
}

/// Statistics of a finished export.
#[derive(Clone, Debug)]
pub struct ExportReport {
    pub outcome: ExportOutcome,
    pub frames: u64,
    pub elapsed: Duration,
}

struct Chunk {
    first: u64,
    frames: Vec<ExportFrame>,
}

/// Renders a range of frames, in order, to a sink.
pub struct Exporter;

impl Exporter {
    /// Runs the export on the calling thread, feeding `sink` with every
    /// frame in order. `make_renderer` is called once per worker (each
    /// worker needs its own decoders; a GPU renderer may share one device).
    pub fn run(
        scene: Arc<dyn Scene>,
        settings: ExportSettings,
        progress: Arc<ExportProgress>,
        make_renderer: impl Fn() -> Box<dyn FrameRenderer> + Send + Sync + 'static,
        mut sink: impl FnMut(ExportFrame),
    ) -> ExportReport {
        let start = Instant::now();
        let total = settings.frames.end.saturating_sub(settings.frames.start);
        progress.total.store(total, Ordering::Relaxed);
        if total == 0 {
            return ExportReport { outcome: ExportOutcome::Finished, frames: 0, elapsed: start.elapsed() };
        }
        let workers = if settings.workers == 0 { std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2).min(8) } else { settings.workers };
        let chunk = settings.chunk.max(1);
        let ahead = settings.ahead.max(1);
        let canvas = scene.canvas();
        let frame_us = 1e6 / canvas.frame_rate.max(1e-6);
        let next_chunk = Arc::new(Mutex::new(settings.frames.start));
        // Bounded channel: at most `ahead` finished chunks wait for the sink.
        let (tx, rx): (SyncSender<Chunk>, Receiver<Chunk>) = sync_channel(ahead);
        let make_renderer = Arc::new(make_renderer);
        let settings = Arc::new(settings);
        let mut handles = Vec::new();
        for _ in 0..workers {
            let scene = scene.clone();
            let settings = settings.clone();
            let progress = progress.clone();
            let next_chunk = next_chunk.clone();
            let tx = tx.clone();
            let make_renderer = make_renderer.clone();
            handles.push(std::thread::spawn(move || {
                let mut renderer = make_renderer();
                loop {
                    if progress.is_cancelled() {
                        return;
                    }
                    let first = {
                        let mut g = next_chunk.lock().unwrap_or_else(|e| e.into_inner());
                        let f = *g;
                        if f >= settings.frames.end {
                            return;
                        }
                        *g = (f + chunk).min(settings.frames.end);
                        f
                    };
                    let last = (first + chunk).min(settings.frames.end);
                    let mut frames = Vec::with_capacity((last - first) as usize);
                    for index in first..last {
                        if progress.is_cancelled() {
                            return;
                        }
                        let time = (index as f64 * frame_us).round() as Micros;
                        let e = renderer.render_encoded(scene.as_ref(), time, settings.size, &settings.output, settings.depth, index as u32);
                        frames.push(ExportFrame { index, time, width: e.width, height: e.height, pixels: e.pixels });
                    }
                    // Blocks when the sink is behind: back-pressure.
                    if tx.send(Chunk { first, frames }).is_err() {
                        return;
                    }
                }
            }));
        }
        drop(tx);
        // Reorder buffer: deliver chunks in index order.
        let mut expected = settings.frames.start;
        let mut waiting: BTreeMap<u64, Chunk> = BTreeMap::new();
        let mut delivered = 0u64;
        for c in rx {
            waiting.insert(c.first, c);
            while let Some(c) = waiting.remove(&expected) {
                let n = c.frames.len() as u64;
                for f in c.frames {
                    sink(f);
                }
                delivered += n;
                progress.done.store(delivered, Ordering::Relaxed);
                expected += n;
            }
            if progress.is_cancelled() {
                break;
            }
        }
        for h in handles {
            let _ = h.join();
        }
        let outcome = if progress.is_cancelled() && delivered < total { ExportOutcome::Cancelled } else { ExportOutcome::Finished };
        ExportReport { outcome, frames: delivered, elapsed: start.elapsed() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compositor::CpuCompositor;
    use crate::scene::{Canvas, ClipDesc, MediaKey, SECOND, SourceDesc, StaticScene};
    use crate::source::{DecodeCache, ProceduralSource, SourceRegistry, SourceSpec, Sources};

    fn scene_and_registry() -> (Arc<dyn Scene>, Arc<SourceRegistry>) {
        let reg = SourceRegistry::new();
        reg.register(MediaKey(1), SourceSpec::Procedural(ProceduralSource::test_pattern(64, 36, 30.0, 90)));
        let mut clip = ClipDesc::new(1, 0, 0, 3 * SECOND, SourceDesc::Media { media: MediaKey(1), time: 0 });
        clip.seed = 1;
        // A static scene cannot map time to source time; use a scene that does.
        struct Moving(StaticScene);
        impl Scene for Moving {
            fn canvas(&self) -> Canvas {
                self.0.canvas()
            }
            fn duration(&self) -> Micros {
                self.0.duration()
            }
            fn clips_at(&self, t: Micros) -> Vec<ClipDesc> {
                let mut v = self.0.clips_at(t);
                for c in &mut v {
                    if let SourceDesc::Media { time, .. } = &mut c.source {
                        *time = t - c.start;
                    }
                }
                v
            }
            fn transition_at(&self, _t: Micros) -> Option<crate::scene::TransitionDesc> {
                None
            }
        }
        let scene = Moving(StaticScene::new(Canvas::new(64, 36, 30.0), 3 * SECOND).with_clip(clip));
        (Arc::new(scene), reg)
    }

    #[test]
    fn frames_arrive_in_order_from_several_workers() {
        let (scene, reg) = scene_and_registry();
        let cache = DecodeCache::new(1 << 24);
        let settings = ExportSettings { frames: 0..45, workers: 3, chunk: 4, ahead: 2, ..Default::default() };
        let progress = ExportProgress::new();
        let mut got = Vec::new();
        let r = Exporter::run(
            scene,
            settings,
            progress.clone(),
            move || Box::new(CpuCompositor::new(Sources::new(reg.clone(), cache.clone()))),
            |f| got.push((f.index, f.pixels)),
        );
        assert_eq!(r.outcome, ExportOutcome::Finished);
        assert_eq!(r.frames, 45);
        assert_eq!(progress.progress(), (45, 45));
        // Frames arrive in index order and differ between frames (the test
        // pattern moves).
        let mut last: Option<&ExportPixels> = None;
        for (i, (index, px)) in got.iter().enumerate() {
            assert_eq!(*index, i as u64);
            if let (ExportPixels::Rgba8(a), Some(ExportPixels::Rgba8(b))) = (px, last) {
                assert_ne!(a, b, "frame {i} equals the previous one");
            }
            last = Some(px);
        }
    }

    #[test]
    fn cancel_stops_early() {
        let (scene, reg) = scene_and_registry();
        let cache = DecodeCache::new(1 << 24);
        let progress = ExportProgress::new();
        let p2 = progress.clone();
        let settings = ExportSettings { frames: 0..90, workers: 2, chunk: 2, ahead: 1, ..Default::default() };
        let mut n = 0;
        let r = Exporter::run(
            scene,
            settings,
            progress,
            move || Box::new(CpuCompositor::new(Sources::new(reg.clone(), cache.clone()))),
            |_| {
                n += 1;
                if n == 6 {
                    p2.cancel();
                }
            },
        );
        assert_eq!(r.outcome, ExportOutcome::Cancelled);
        assert!(r.frames < 90);
    }
}
