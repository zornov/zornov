// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Media understanding for agents: shot changes, pauses and beats, found in
//! the source files and reported in timeline seconds for each clip that
//! uses them. Results are cached per file, so asking again is free.

use crate::view::{self, SEC};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use studio_ai::features::{beats, scenes, silence};

#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct MediaFacts {
    /// Shot changes, source seconds.
    pub scenes: Option<Vec<f64>>,
    /// Pauses, source seconds.
    pub silences: Option<Vec<(f64, f64)>>,
    pub beats: Option<Vec<f64>>,
    pub tempo: Option<f64>,
}

#[derive(Default)]
pub struct Analysis {
    cache: HashMap<(PathBuf, u64), MediaFacts>,
}

/// Decoded video frames, small, for shot detection.
pub(super) struct FileFrames {
    pub(super) src: Box<dyn studio_render::source::MediaSource>,
    pub(super) count: usize,
    pub(super) fps: f64,
    /// Width of the decoded thumbnails.
    pub(super) width: usize,
}

impl studio_ai::frame::FrameSource for FileFrames {
    fn frame_count(&self) -> usize {
        self.count
    }
    fn fps(&self) -> f64 {
        self.fps
    }
    fn frame(&mut self, index: usize) -> studio_ai::error::Result<studio_ai::frame::Frame> {
        let mut got = None;
        self.src.decode_to(index as u64, &mut |i, pic| {
            if i == index as u64 {
                got = Some(pic);
            }
        });
        let pic = got.ok_or_else(|| studio_ai::error::Error::Io(format!("frame {index} could not be decoded")))?;
        Ok(small_rgba(&pic, self.width))
    }
}

/// A `w`-wide RGBA thumbnail of a decoded picture (BT.709, nearest).
pub(super) fn small_rgba(pic: &studio_render::yuv::DecodedPicture, w: usize) -> studio_ai::frame::Frame {
    use studio_render::yuv::{PixelData, Planes};
    let (pw, ph) = (pic.width.max(1) as usize, pic.height.max(1) as usize);
    let h = (w * ph / pw).max(1);
    let mut out = vec![0u8; w * h * 4];
    let sample = |p: &Planes, i: usize| -> f32 {
        match p {
            Planes::U8(v) => v.get(i).copied().unwrap_or(0) as f32 / 255.0,
            Planes::U16(v) => v.get(i).copied().unwrap_or(0) as f32 / ((1u32 << pic.colour.bit_depth.clamp(8, 16)) - 1) as f32,
        }
    };
    for y in 0..h {
        for x in 0..w {
            let (sx, sy) = (x * pw / w, y * ph / h);
            let (r, g, b) = match &pic.data {
                PixelData::Yuv { y: yp, u, v, chroma_width, chroma_height } => {
                    let (cw, ch) = (*chroma_width as usize, *chroma_height as usize);
                    let (cx, cy) = (sx * cw / pw, sy * ch / ph);
                    let yy = sample(yp, sy * pw + sx);
                    let (cb, cr) = (sample(u, cy * cw + cx) - 0.5, sample(v, cy * cw + cx) - 0.5);
                    (yy + 1.5748 * cr, yy - 0.1873 * cb - 0.4681 * cr, yy + 1.8556 * cb)
                }
                PixelData::Rgba8(px) => {
                    let o = (sy * pw + sx) * 4;
                    (px[o] as f32 / 255.0, px[o + 1] as f32 / 255.0, px[o + 2] as f32 / 255.0)
                }
                #[allow(unreachable_patterns)]
                _ => (0.0, 0.0, 0.0),
            };
            let o = (y * w + x) * 4;
            out[o] = (r.clamp(0.0, 1.0) * 255.0) as u8;
            out[o + 1] = (g.clamp(0.0, 1.0) * 255.0) as u8;
            out[o + 2] = (b.clamp(0.0, 1.0) * 255.0) as u8;
            out[o + 3] = 255;
        }
    }
    studio_ai::frame::Frame { width: w, height: h, rgba: out }
}

/// Shot changes in a video file, decoding key-frame-aligned segments on
/// every core at once and joining their frame signatures.
fn shot_changes(path: &Path) -> Result<Vec<f64>, String> {
    let Ok(src) = studio_render::source::open_path(path) else { return Ok(Vec::new()) };
    let info = src.info();
    let fps = info.frame_rate.unwrap_or(30.0).max(1.0);
    let count = info.frame_count.unwrap_or(0) as usize;
    if count == 0 || info.width == 0 {
        return Ok(Vec::new());
    }
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(1, 16);
    // Segment starts on sync frames, so each decoder starts cleanly.
    let mut starts: Vec<usize> = (0..workers).map(|k| src.nearest_sync((count * k / workers) as u64) as usize).collect();
    starts.sort_unstable();
    starts.dedup();
    drop(src);
    let ranges: Vec<(usize, usize)> = starts.iter().enumerate().map(|(i, &a)| (a, starts.get(i + 1).copied().unwrap_or(count))).filter(|(a, b)| b > a).collect();
    let results: Vec<Result<Vec<(usize, scenes::FrameSignature)>, String>> = std::thread::scope(|sc| {
        let handles: Vec<_> = ranges
            .iter()
            .map(|&(a, b)| {
                sc.spawn(move || {
                    let src = studio_render::source::open_path(path).map_err(|e| e.to_string())?;
                    let mut ff = FileFrames { src, count, fps, width: 96 };
                    let mut sigs = Vec::with_capacity(b - a);
                    for i in a..b {
                        use studio_ai::frame::FrameSource;
                        match ff.frame(i) {
                            Ok(fr) => sigs.push((i, scenes::FrameSignature::of(&fr))),
                            Err(_) => continue,
                        }
                    }
                    Ok(sigs)
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap_or_else(|_| Err("a decode thread failed".into()))).collect()
    });
    let mut sigs = Vec::with_capacity(count);
    for r in results {
        sigs.extend(r?);
    }
    sigs.sort_by_key(|s| s.0);
    Ok(scenes::find_cuts(&sigs, fps, &scenes::SceneSettings::default()).iter().map(|c| c.time).collect())
}

/// Where the facts of a file are kept between sessions.
fn disk_path(p: &Path, modified: u64, min_silence: f64) -> PathBuf {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    p.hash(&mut h);
    modified.hash(&mut h);
    ((min_silence * 1000.0) as i64).hash(&mut h);
    studio_paths::cache().join("analysis").join(format!("{:016x}.json", h.finish()))
}

fn modified(p: &Path) -> u64 {
    std::fs::metadata(p).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs())
}

impl Analysis {
    fn facts(&mut self, path: &Path, want: &[&str], min_silence: f64) -> Result<MediaFacts, String> {
        let key = (path.to_path_buf(), modified(path));
        // Memory first, then the on-disk cache (survives sessions).
        let disk = disk_path(path, key.1, min_silence);
        let mut f = match self.cache.get(&key) {
            Some(f) => f.clone(),
            None => std::fs::read(&disk).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default(),
        };
        if want.contains(&"scenes") && f.scenes.is_none() {
            f.scenes = Some(shot_changes(path)?);
        }
        let need_audio = (want.contains(&"silences") && f.silences.is_none()) || (want.contains(&"beats") && f.beats.is_none());
        if need_audio {
            match crate::sound_bridge::decoded(&path.to_string_lossy()) {
                Some(d) => {
                    let mono: Vec<f32> = d.left.iter().zip(d.right.iter()).map(|(l, r)| 0.5 * (l + r)).collect();
                    if want.contains(&"silences") && f.silences.is_none() {
                        let s = silence::find_silences(&mono, d.rate, &silence::SilenceSettings { min_silence, ..Default::default() });
                        f.silences = Some(s.iter().map(|c| (c.start, c.end)).collect());
                    }
                    if want.contains(&"beats") && f.beats.is_none() {
                        let inter: Vec<f32> = d.left.iter().zip(d.right.iter()).flat_map(|(l, r)| [*l, *r]).collect();
                        let b = beats::beat_times(&inter, d.rate, 2);
                        f.tempo = beats::tempo(&b);
                        f.beats = Some(b);
                    }
                }
                None => {
                    f.silences.get_or_insert_with(Vec::new);
                    f.beats.get_or_insert_with(Vec::new);
                }
            }
        }
        if let Some(dir) = disk.parent() {
            let _ = std::fs::create_dir_all(dir);
            if let Ok(b) = serde_json::to_vec(&f) {
                let _ = std::fs::write(&disk, b);
            }
        }
        self.cache.insert(key, f.clone());
        Ok(f)
    }

    /// Facts for each clip (or the given ones), in timeline seconds.
    pub fn run(&mut self, p: &view::Project, clips: &[u32], want: &[&str], min_silence: f64) -> Result<Value, String> {
        let targets: Vec<&view::Clip> = p.clips.iter().filter(|c| c.media.is_some() && (clips.is_empty() || clips.contains(&c.id))).collect();
        if targets.is_empty() {
            return Err("no clips with media to analyse".into());
        }
        let mut out = Vec::new();
        for c in targets {
            let Some(mi) = c.media.and_then(|m| p.media(m)) else { continue };
            let Some(path) = &mi.path else {
                out.push(json!({"clip": c.id, "note": "sample media without a file"}));
                continue;
            };
            let f = self.facts(path, want, min_silence)?;
            // Source seconds -> timeline seconds, inside the clip.
            let speed = (c.speed as f64 / 100.0).max(0.01);
            let (s_in, s_out) = (c.src_in as f64 / SEC as f64, (c.src_in as f64 + c.dur as f64 * speed) / SEC as f64);
            let to_tl = |t: f64| ((c.start as f64 / SEC as f64 + (t - s_in) / speed) * 1000.0).round() / 1000.0;
            let inside = |t: f64| t >= s_in && t <= s_out;
            let mut o = serde_json::Map::new();
            o.insert("clip".into(), json!(c.id));
            if let Some(sc) = &f.scenes {
                o.insert("shot_changes".into(), json!(sc.iter().copied().filter(|t| inside(*t)).map(to_tl).collect::<Vec<_>>()));
            }
            if let Some(si) = &f.silences {
                o.insert("pauses".into(), json!(si.iter().filter(|(a, b)| *b > s_in && *a < s_out).map(|(a, b)| [to_tl(a.max(s_in)), to_tl(b.min(s_out))]).collect::<Vec<_>>()));
            }
            if let Some(b) = &f.beats {
                let bs: Vec<f64> = b.iter().copied().filter(|t| inside(*t)).map(to_tl).take(4000).collect();
                o.insert("beats".into(), json!(bs));
                if let Some(t) = f.tempo {
                    o.insert("bpm".into(), json!((t * 10.0).round() / 10.0));
                }
            }
            out.push(Value::Object(o));
        }
        Ok(json!({"clips": out}))
    }
}
