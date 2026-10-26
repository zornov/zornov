// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Looking into the asset library's files so they can be searched by what
//! is in them: a few frames of every picture and clip are described (colour,
//! light, people, lettering, close-up or wide) and given a signature for
//! "find more like this"; faces are grouped into people; sounds get a
//! spectral signature; speech is written out with the local recogniser when
//! its model is installed. Everything runs on this computer.
//!
//! What it cannot do yet, said plainly: name objects and actions (no
//! licence-safe object model is installed), read text on screen (it only
//! notices that there is lettering), or tell who a face belongs to (faces
//! are grouped by how they look; the person names a group once and every
//! asset with it is labelled).

use crate::assets::{self, Asset, Store};
use std::path::Path;

/// What a pass found, for the caller to report.
#[derive(Debug, Default, Clone)]
pub struct Found {
    pub frames: usize,
    pub faces: usize,
    pub words: usize,
    pub notes: Vec<String>,
}

/// A picture's signature: where the colours are (a 4×4 grid), which hues
/// it holds, and how its light is spread. Centred, so cosine similarity
/// tells pictures apart instead of calling everything alike.
pub fn picture_signature(rgba: &[u8], w: usize, h: usize) -> Vec<f32> {
    let mut grid = [[0f32; 3]; 16];
    let mut count = [0f32; 16];
    let mut hue = [0f32; 12];
    let mut light = [0f32; 8];
    for y in 0..h {
        for x in 0..w {
            let o = (y * w + x) * 4;
            let (r, g, b) = (rgba[o] as f32 / 255.0, rgba[o + 1] as f32 / 255.0, rgba[o + 2] as f32 / 255.0);
            let cell = (y * 4 / h.max(1)).min(3) * 4 + (x * 4 / w.max(1)).min(3);
            grid[cell][0] += r;
            grid[cell][1] += g;
            grid[cell][2] += b;
            count[cell] += 1.0;
            let (mx, mn) = (r.max(g).max(b), r.min(g).min(b));
            let l = 0.2126 * r + 0.7152 * g + 0.0722 * b;
            light[((l * 8.0) as usize).min(7)] += 1.0;
            let sat = mx - mn;
            if sat > 0.12 {
                let hh = if mx == r { ((g - b) / sat).rem_euclid(6.0) } else if mx == g { (b - r) / sat + 2.0 } else { (r - g) / sat + 4.0 };
                hue[((hh * 2.0) as usize).min(11)] += sat;
            }
        }
    }
    let mut v = Vec::with_capacity(48 + 12 + 8);
    for (c, n) in grid.iter().zip(count) {
        for ch in c {
            v.push(ch / n.max(1.0) - 0.5);
        }
    }
    let ht: f32 = hue.iter().sum::<f32>().max(1e-6);
    v.extend(hue.iter().map(|x| x / ht - 1.0 / 12.0));
    let lt: f32 = light.iter().sum::<f32>().max(1e-6);
    v.extend(light.iter().map(|x| x / lt - 1.0 / 8.0));
    v
}

/// The words for what a frame looks like (see [`assets::describe_look`]).
pub fn describe_frame(rgba: &[u8], w: usize, h: usize, faces: &[studio_ai::features::faces::Face]) -> String {
    let img = studio_media::rgba::RgbaImage { width: w as u32, height: h as u32, pixels: rgba.to_vec() };
    let seen = super::sight::look_at(&img, None, faces);
    let n = (w * h).max(1) as f32;
    let mut mean = [0f32; 3];
    for px in rgba.chunks_exact(4) {
        for c in 0..3 {
            mean[c] += px[c] as f32 / 255.0;
        }
    }
    for m in &mut mean {
        *m /= n;
    }
    let biggest = seen.faces.iter().map(|x| x[2] as f32).fold(0.0, f32::max);
    let mut text = assets::describe_look(mean, seen.brightness as f32, seen.contrast as f32, seen.faces.len(), biggest, seen.lettering, w as f32 / h.max(1) as f32);
    for c in &seen.colours {
        text.push(' ');
        text.push_str(c);
    }
    text
}

/// A face's descriptor: its 8×8 light pattern with the average taken out,
/// so lighting and exposure matter little. Appearance only.
pub fn face_descriptor(rgba: &[u8], w: usize, h: usize, b: &studio_ai::features::faces::Rect) -> Option<Vec<f32>> {
    let (x0, y0) = (b.x.max(0.0) as usize, b.y.max(0.0) as usize);
    let (x1, y1) = (((b.x + b.w) as usize).min(w), ((b.y + b.h) as usize).min(h));
    if x1 <= x0 + 2 || y1 <= y0 + 2 {
        return None;
    }
    let mut cell = [0f32; 64];
    let mut n = [0f32; 64];
    for y in y0..y1 {
        for x in x0..x1 {
            let k = ((y - y0) * 8 / (y1 - y0)) * 8 + (x - x0) * 8 / (x1 - x0);
            let o = (y * w + x) * 4;
            cell[k] += 0.2126 * rgba[o] as f32 + 0.7152 * rgba[o + 1] as f32 + 0.0722 * rgba[o + 2] as f32;
            n[k] += 1.0;
        }
    }
    for (c, n) in cell.iter_mut().zip(n) {
        *c /= n.max(1.0);
    }
    let mean = cell.iter().sum::<f32>() / 64.0;
    Some(cell.iter().map(|c| c - mean).collect())
}

/// A sound's signature: the energy in sixteen bands from bass to treble
/// (Goertzel over a handful of windows), loudness left out so a quiet and a
/// loud take of the same thing match.
pub fn sound_signature(mono: &[f32], rate: u32) -> Vec<f32> {
    let win = (rate as usize / 4).max(256).min(mono.len().max(1));
    let windows = 8usize;
    let mut bands = vec![0f64; 16];
    let mut used = 0;
    for k in 0..windows {
        let at = if mono.len() > win { (mono.len() - win) * k / (windows - 1) } else { 0 };
        let seg = &mono[at..(at + win).min(mono.len())];
        if seg.len() < 64 {
            continue;
        }
        used += 1;
        for (i, e) in bands.iter_mut().enumerate() {
            let f = 80.0 * (7000.0f64 / 80.0).powf(i as f64 / 15.0);
            let w = 2.0 * std::f64::consts::PI * f / rate as f64;
            let c = 2.0 * w.cos();
            let (mut s1, mut s2) = (0.0f64, 0.0f64);
            for (j, x) in seg.iter().enumerate() {
                let hann = 0.5 - 0.5 * (2.0 * std::f64::consts::PI * j as f64 / seg.len() as f64).cos();
                let s0 = *x as f64 * hann + c * s1 - s2;
                s2 = s1;
                s1 = s0;
            }
            *e += (s1 * s1 + s2 * s2 - c * s1 * s2).max(0.0) / seg.len() as f64;
        }
    }
    if used == 0 {
        return Vec::new();
    }
    let raw: Vec<f64> = bands.iter().map(|e| (e / used as f64 + 1e-30).ln()).collect();
    // Nothing is told apart below 60 dB under the loudest band.
    let top = raw.iter().copied().fold(f64::MIN, f64::max);
    let logs: Vec<f64> = raw.iter().map(|l| l.max(top - 13.8)).collect();
    let mean = logs.iter().sum::<f64>() / logs.len() as f64;
    logs.iter().map(|l| (l - mean) as f32).collect()
}

fn is_image(p: &Path) -> bool {
    p.extension().and_then(|e| e.to_str()).is_some_and(|e| ["png", "jpg", "jpeg"].contains(&e.to_ascii_lowercase().as_str()))
}

/// Looks into one asset and records what it finds. `speech`: also write
/// out what is said (slow; needs the recogniser's model).
pub fn index_asset(store: &Store, a: &Asset, speech: bool) -> Result<Found, String> {
    let path = Path::new(&a.path);
    if !path.is_file() {
        return Err(format!("{} is not on disk", a.name));
    }
    let mut found = Found::default();
    let models = studio_ai::store::ModelStore::default();
    let detector = studio_ai::features::faces::FaceDetector::load(&models).ok();
    if detector.is_none() {
        found.notes.push("the face model is not installed, so people were not grouped (Settings ▸ Models)".into());
    }
    let mut sigs: Vec<(f64, Vec<f32>)> = Vec::new();
    let mut seen: Vec<(f64, f64, String)> = Vec::new();
    let mut faces: Vec<(f64, Vec<f32>)> = Vec::new();
    let mut frame = |t: f64, t_end: f64, rgba: &[u8], w: usize, h: usize| {
        found.frames += 1;
        let fr = studio_ai::frame::Frame { width: w, height: h, rgba: rgba.to_vec() };
        let boxes: Vec<studio_ai::features::faces::Face> = detector.as_ref().map(|d| d.detect(&fr).unwrap_or_default().into_iter().filter(|f| f.score > 0.6).collect()).unwrap_or_default();
        // Faces as fractions of the frame for the description.
        let norm: Vec<studio_ai::features::faces::Face> = boxes
            .iter()
            .map(|f| {
                let mut g = f.clone();
                g.bbox = studio_ai::features::faces::Rect { x: f.bbox.x / w as f32, y: f.bbox.y / h as f32, w: f.bbox.w / w as f32, h: f.bbox.h / h as f32 };
                g
            })
            .collect();
        sigs.push((t, picture_signature(rgba, w, h)));
        let text = describe_frame(rgba, w, h, &norm);
        match seen.last_mut() {
            Some(last) if last.2 == text => last.1 = t_end,
            _ => seen.push((t, t_end, text)),
        }
        for f in &boxes {
            if let Some(d) = face_descriptor(rgba, w, h, &f.bbox) {
                faces.push((t, d));
            }
        }
    };
    match a.kind.as_str() {
        "image" if is_image(path) => {
            let img = crate::media_io::read_image(path)?;
            let (w0, h0) = (img.size[0], img.size[1]);
            let w = 192.min(w0).max(1);
            let h = (w * h0 / w0.max(1)).max(1);
            let mut px = vec![0u8; w * h * 4];
            for y in 0..h {
                for x in 0..w {
                    let c = img.pixels[(y * h0 / h) * w0 + x * w0 / w];
                    let o = (y * w + x) * 4;
                    px[o..o + 4].copy_from_slice(&[c.r(), c.g(), c.b(), 255]);
                }
            }
            frame(0.0, 0.0, &px, w, h);
        }
        #[cfg(feature = "render")]
        "video" => {
            if let Ok(mut src) = studio_render::source::open_path(path) {
                let info = src.info();
                let fps = info.frame_rate.unwrap_or(30.0).max(1.0);
                let count = info.frame_count.unwrap_or(0);
                if count > 0 {
                    let secs = count as f64 / fps;
                    let n = ((secs / 3.0).ceil() as usize).clamp(1, 16);
                    for k in 0..n {
                        let t = secs * (k as f64 + 0.5) / n as f64;
                        let idx = ((t * fps) as u64).min(count - 1);
                        let mut got = None;
                        src.decode_to(idx, &mut |i, pic| {
                            if i == idx {
                                got = Some(super::analyse::small_rgba(&pic, 192));
                            }
                        });
                        if let Some(f) = got {
                            frame(secs * k as f64 / n as f64, secs * (k as f64 + 1.0) / n as f64, &f.rgba, f.width, f.height);
                        }
                    }
                }
            }
        }
        _ => {}
    }
    drop(frame);
    if !sigs.is_empty() {
        store.record_sigs(a.id, &sigs)?;
        store.record(a.id, "seen", &seen)?;
        store.record_faces(a.id, &faces)?;
        found.faces = faces.len();
    }
    // Sound: a signature for audio, speech for audio and video.
    if a.kind == "audio" || a.kind == "video" {
        if let Some(d) = crate::sound_bridge::decode_uncached(path) {
            let mono: Vec<f32> = d.left.iter().zip(d.right.iter()).map(|(l, r)| 0.5 * (l + r)).collect();
            if a.kind == "audio" {
                let s = sound_signature(&mono, d.rate);
                if !s.is_empty() {
                    store.record_sigs(a.id, &[(0.0, s)])?;
                }
            }
            if speech {
                let w = studio_ai::features::transcribe::Whisper::load_best(&models).ok();
                match w {
                    Some(w) => {
                        let opts = studio_ai::features::transcribe::TranscribeOptions { language: None, ..Default::default() };
                        if let Ok(t) = w.transcribe(&mono, d.rate, &opts, &studio_ai::jobs::JobCtx::detached()) {
                            let words: Vec<(String, f64, f64)> = t.words().map(|w| (w.text.trim().to_owned(), w.start, w.end)).filter(|w| !w.0.is_empty()).collect();
                            found.words = words.len();
                            let mut lines = Vec::new();
                            let mut cur: Vec<&(String, f64, f64)> = Vec::new();
                            for (i, w) in words.iter().enumerate() {
                                cur.push(w);
                                let gap = words.get(i + 1).map_or(f64::INFINITY, |n| n.1 - w.2);
                                if w.0.ends_with(['.', '?', '!']) || gap > 0.6 || cur.len() >= 24 {
                                    lines.push((cur[0].1, w.2, cur.iter().map(|w| w.0.as_str()).collect::<Vec<_>>().join(" ")));
                                    cur.clear();
                                }
                            }
                            store.record(a.id, "speech", &lines)?;
                        }
                    }
                    None => found.notes.push("the speech model is not installed, so what is said was not written out (Settings ▸ Models)".into()),
                }
            }
        }
    }
    store.mark_analysed(a.id, if speech { 2 } else { 1 })?;
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(w: usize, h: usize, rgb: [u8; 3]) -> Vec<u8> {
        (0..w * h).flat_map(|_| [rgb[0], rgb[1], rgb[2], 255]).collect()
    }

    #[test]
    fn signatures_tell_pictures_apart() {
        let red = picture_signature(&flat(32, 24, [200, 30, 30]), 32, 24);
        let red2 = picture_signature(&flat(32, 24, [190, 40, 35]), 32, 24);
        let blue = picture_signature(&flat(32, 24, [20, 40, 200]), 32, 24);
        assert!(assets::cosine(&red, &red2) > 0.98);
        assert!(assets::cosine(&red, &blue) < 0.5);
    }

    #[test]
    fn faces_of_one_person_group_and_others_do_not() {
        // Two lightings of the same pattern match; a different pattern does not.
        let w = 40;
        let make = |gain: f32, flip: bool| -> Vec<u8> {
            let mut px = vec![0u8; w * w * 4];
            for y in 0..w {
                for x in 0..w {
                    let xx = if flip { w - 1 - x } else { x };
                    let v = ((60.0 + (xx as f32 * 5.0) + ((y / 8) as f32 * 9.0)) * gain).min(255.0) as u8;
                    px[(y * w + x) * 4..(y * w + x) * 4 + 4].copy_from_slice(&[v, v, v, 255]);
                }
            }
            px
        };
        let r = studio_ai::features::faces::Rect { x: 4.0, y: 4.0, w: 32.0, h: 32.0 };
        let a = face_descriptor(&make(1.0, false), w, w, &r).unwrap();
        let b = face_descriptor(&make(0.8, false), w, w, &r).unwrap();
        let c = face_descriptor(&make(1.0, true), w, w, &r).unwrap();
        assert!(assets::cosine(&a, &b) > assets::FACE_SAME);
        assert!(assets::cosine(&a, &c) < assets::FACE_SAME);
    }

    #[test]
    fn sounds_are_told_apart_by_pitch_not_volume() {
        let tone = |f: f32, gain: f32| -> Vec<f32> { (0..16_000).map(|i| (i as f32 * f * std::f32::consts::TAU / 16_000.0).sin() * gain).collect() };
        let low = sound_signature(&tone(220.0, 0.5), 16_000);
        let low_quiet = sound_signature(&tone(220.0, 0.05), 16_000);
        let high = sound_signature(&tone(3000.0, 0.5), 16_000);
        assert!(assets::cosine(&low, &low_quiet) > 0.99);
        assert!(assets::cosine(&low, &high) < 0.5);
    }
}
