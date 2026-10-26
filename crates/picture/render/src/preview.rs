// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The preview driver: renders the newest request on a worker thread.
//!
//! Rules (from the product requirements): the viewer asks for a time and
//! its own pixel size; the preview renders at that size (capped at the
//! canvas) and never lowers it on its own. When requests arrive faster
//! than frames render, only the newest is kept and the rest are dropped,
//! so the picture never lags behind the playhead. The user's quality pick
//! (full / half / quarter) is a divisor the viewer applies to the size it
//! asks for; this module does not touch it.

use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::compositor::FrameRenderer;
use crate::output::{EncodedPixels, OutputDepth, OutputSettings};
use crate::scene::{Micros, Scene};

/// How long a preview frame waits for its video pictures before the
/// nearest decoded ones stand in.
const PREVIEW_WAIT_MS: u64 = 40;

/// What the viewer wants rendered.
#[derive(Clone, Debug)]
pub struct PreviewRequest {
    pub time: Micros,
    /// Viewer pixel size; the renderer keeps the canvas aspect and never
    /// exceeds the canvas.
    pub size: (u32, u32),
    /// Monotonic sequence number from the viewer; results carry it back.
    pub sequence: u64,
    pub output: OutputSettings,
    /// Scrubbing: show the nearest key frame of video sources rather than
    /// decoding up to the exact frame (see `source::set_fast_seek`).
    pub fast: bool,
}

/// A rendered preview frame.
#[derive(Clone, Debug)]
pub struct PreviewFrame {
    pub sequence: u64,
    pub time: Micros,
    pub width: u32,
    pub height: u32,
    /// Encoded RGBA8 in the requested output settings.
    pub rgba: Vec<u8>,
    /// Wall time the render took.
    pub render_time: Duration,
}

struct Shared {
    pending: Mutex<State>,
    wake: Condvar,
}

struct State {
    request: Option<PreviewRequest>,
    scene: Option<Arc<dyn Scene>>,
    latest: Option<PreviewFrame>,
    quit: bool,
    dropped: u64,
    rendered: u64,
}

/// Owns the render thread.
pub struct Previewer {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl Previewer {
    /// Starts the render thread with the given renderer.
    pub fn new(mut renderer: Box<dyn FrameRenderer>) -> Previewer {
        let shared = Arc::new(Shared {
            pending: Mutex::new(State { request: None, scene: None, latest: None, quit: false, dropped: 0, rendered: 0 }),
            wake: Condvar::new(),
        });
        let worker = shared.clone();
        let thread = std::thread::Builder::new()
            .name("studio-preview".into())
            .spawn(move || {
                // Never block the preview on decoding for long: past this,
                // the nearest decoded picture stands in and the frame is
                // drawn again once the exact one is ready.
                crate::source::set_wait_budget(Some(Duration::from_millis(PREVIEW_WAIT_MS)));
                loop {
                    let (req, scene) = {
                        let mut g = worker.pending.lock().unwrap_or_else(|e| e.into_inner());
                        loop {
                            if g.quit {
                                return;
                            }
                            if let (Some(r), Some(s)) = (g.request.take(), g.scene.clone()) {
                                break (r, s);
                            }
                            g = worker.wake.wait(g).unwrap_or_else(|e| e.into_inner());
                        }
                    };
                    let start = Instant::now();
                    crate::source::set_fast_seek(req.fast);
                    crate::source::set_wait_budget(Some(Duration::from_millis(if req.fast { 0 } else { PREVIEW_WAIT_MS })));
                    let _ = crate::source::take_provisional();
                    let frame_index = (req.time.max(0) as f64 / 1e6 * scene.canvas().frame_rate + 1e-3).floor() as u32;
                    // A frame that panics (a damaged file, a decoder bug) is
                    // skipped; the preview thread keeps serving requests.
                    let Ok(e) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| renderer.render_encoded(scene.as_ref(), req.time, Some(req.size), &req.output, OutputDepth::Eight, frame_index))) else {
                        crate::source::set_fast_seek(false);
                        continue;
                    };
                    let rgba = match e.pixels {
                        EncodedPixels::Rgba8(v) => v,
                        EncodedPixels::Rgba16(v) => v.iter().map(|x| (x >> 8) as u8).collect(),
                    };
                    let out = PreviewFrame { sequence: req.sequence, time: req.time, width: e.width, height: e.height, rgba, render_time: start.elapsed() };
                    let provisional = crate::source::take_provisional() && !req.fast;
                    let mut g = worker.pending.lock().unwrap_or_else(|e| e.into_inner());
                    g.rendered += 1;
                    g.latest = Some(out);
                    // A stand-in was drawn: draw this request again shortly
                    // (unless something newer is already waiting).
                    if provisional && g.request.is_none() {
                        drop(g);
                        std::thread::sleep(Duration::from_millis(6));
                        let mut g = worker.pending.lock().unwrap_or_else(|e| e.into_inner());
                        if g.request.is_none() {
                            g.request = Some(req);
                        }
                    }
                }
            })
            .expect("spawn preview thread");
        Previewer { shared, thread: Some(thread) }
    }

    /// Sets the scene to render. A new scene replaces the old one for the
    /// next request.
    pub fn set_scene(&self, scene: Arc<dyn Scene>) {
        let mut g = self.shared.pending.lock().unwrap_or_else(|e| e.into_inner());
        g.scene = Some(scene);
        self.shared.wake.notify_one();
    }

    /// Asks for a frame. A request still waiting is replaced (and counted
    /// as dropped).
    pub fn request(&self, req: PreviewRequest) {
        let mut g = self.shared.pending.lock().unwrap_or_else(|e| e.into_inner());
        if g.request.replace(req).is_some() {
            g.dropped += 1;
        }
        self.shared.wake.notify_one();
    }

    /// Takes the most recently finished frame, if a new one is ready.
    pub fn poll(&self) -> Option<PreviewFrame> {
        self.shared.pending.lock().unwrap_or_else(|e| e.into_inner()).latest.take()
    }

    /// Waits up to `timeout` for a finished frame.
    pub fn wait(&self, timeout: Duration) -> Option<PreviewFrame> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(f) = self.poll() {
                return Some(f);
            }
            if Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// Requests dropped because a newer one arrived first, and frames
    /// rendered. The viewer shows the drop count as information only.
    pub fn stats(&self) -> (u64, u64) {
        let g = self.shared.pending.lock().unwrap_or_else(|e| e.into_inner());
        (g.dropped, g.rendered)
    }
}

impl Drop for Previewer {
    fn drop(&mut self) {
        {
            let mut g = self.shared.pending.lock().unwrap_or_else(|e| e.into_inner());
            g.quit = true;
        }
        self.shared.wake.notify_all();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compositor::CpuCompositor;
    use crate::scene::{Canvas, ClipDesc, GeneratorDesc, GeneratorKind, Param, Params, SECOND, SourceDesc, StaticScene};
    use crate::source::{DecodeCache, SourceRegistry, Sources};

    #[test]
    fn newest_request_wins_and_size_is_honoured() {
        let mut params = Params::new();
        params.insert("colour".into(), Param::Colour([1.0, 0.0, 0.0, 1.0]));
        let scene = StaticScene::new(Canvas::new(640, 360, 30.0), 10 * SECOND)
            .with_clip(ClipDesc::new(1, 0, 0, 10 * SECOND, SourceDesc::Generator(GeneratorDesc { kind: GeneratorKind::Solid, params, text: vec![] })));
        let renderer = CpuCompositor::new(Sources::new(SourceRegistry::new(), DecodeCache::new(1 << 20)));
        let p = Previewer::new(Box::new(renderer));
        // Requests before a scene is set wait.
        for i in 0..50 {
            p.request(PreviewRequest { time: i * SECOND / 10, size: (320, 180), sequence: i as u64, output: OutputSettings { dither: false, ..Default::default() }, fast: false });
        }
        p.set_scene(Arc::new(scene));
        let f = p.wait(Duration::from_secs(10)).expect("a frame");
        assert_eq!(f.sequence, 49, "only the newest request renders");
        assert_eq!((f.width, f.height), (320, 180));
        assert_eq!(&f.rgba[..4], &[255, 0, 0, 255]);
        let (dropped, rendered) = p.stats();
        assert_eq!(dropped, 49);
        assert_eq!(rendered, 1);
        // Larger than the canvas is capped, never upscaled.
        p.request(PreviewRequest { time: 0, size: (5000, 5000), sequence: 100, output: OutputSettings::default(), fast: false });
        let f = p.wait(Duration::from_secs(10)).unwrap();
        assert_eq!((f.width, f.height), (640, 360));
    }
}
