// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Hover previews for sounds and music in the Library: the first three
//! seconds, faded out, through `studio-audio`. One preview at a time.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

static GENERATION: AtomicU64 = AtomicU64::new(0);
static CURRENT: Mutex<Option<PathBuf>> = Mutex::new(None);

const RATE: f32 = 48_000.0;
const SECONDS: f32 = 3.0;

/// Start previewing `path` (no-op if it is already playing).
pub fn play(path: &Path) {
    let mut cur = CURRENT.lock().unwrap_or_else(|e| e.into_inner());
    if cur.as_deref() == Some(path) {
        return;
    }
    *cur = Some(path.to_path_buf());
    let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    let path = path.to_path_buf();
    std::thread::spawn(move || {
        let Some(samples) = decode(&path) else { return };
        if GENERATION.load(Ordering::SeqCst) != generation {
            return;
        }
        let Ok(out) = studio_audio::AudioOut::open() else { return };
        out.push(&samples);
        let frames = samples.len() / 2;
        let start = std::time::Instant::now();
        while start.elapsed().as_secs_f32() < frames as f32 / RATE + 0.2 {
            if GENERATION.load(Ordering::SeqCst) != generation {
                out.clear();
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
    });
}

/// Stop whatever is previewing.
pub fn stop() {
    let mut cur = CURRENT.lock().unwrap_or_else(|e| e.into_inner());
    if cur.take().is_some() {
        GENERATION.fetch_add(1, Ordering::SeqCst);
    }
}

/// The first seconds of a file as 48 kHz interleaved stereo with a short
/// fade at the end.
fn decode(path: &Path) -> Option<Vec<f32>> {
    let ext = path.extension()?.to_string_lossy().to_ascii_lowercase();
    let w = match ext.as_str() {
        "ogg" => studio_media::ogg::read(path).ok()?,
        "wav" => studio_media::wav::read(path).ok()?,
        "mp3" => studio_media::mp3::read(path).ok()?,
        _ => return None,
    };
    let ch = w.channels.max(1) as usize;
    let src_frames = w.samples.len() / ch;
    let step = w.sample_rate as f32 / RATE;
    let n = ((SECONDS * RATE) as usize).min((src_frames as f32 / step) as usize);
    let fade = (0.15 * RATE) as usize;
    let mut out = Vec::with_capacity(n * 2);
    for i in 0..n {
        let s = i as f32 * step;
        let k = (s as usize).min(src_frames.saturating_sub(1));
        let l = w.samples[k * ch];
        let r = if ch > 1 { w.samples[k * ch + 1] } else { l };
        let g = 0.7 * if i + fade > n { (n - i) as f32 / fade as f32 } else { 1.0 };
        out.push(l * g);
        out.push(r * g);
    }
    Some(out)
}
