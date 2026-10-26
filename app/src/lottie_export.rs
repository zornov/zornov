// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Exporting the timeline (or part of it) as a Lottie animation for the web
//! and apps: `.json`, or `.lottie` (dotLottie).
//!
//! Every picture clip becomes an image layer holding its picture, embedded:
//! a photo or graphic as its own file, a title, card, shape or gradient as
//! Studio draws it. Shape layers become Lottie shape layers instead, their
//! outlines and paint keyed frame by frame, so trims, morphs and repeaters
//! play as they do in Studio. Where it sits, how big, its turn and its opacity are
//! taken from Studio's own renderer at every frame, so keyframes, eases,
//! motion presets and parenting come out exactly as they play, then the
//! keys a straight line between their neighbours already gives are left
//! out. Blend modes carry over. What Lottie cannot hold (video, sound,
//! effects, grades, masks, transitions, 3D) is listed in the report and
//! left out.
//!
//! The document follows the lottie-spec 1.0 description of image layers,
//! transforms and keyframes; the dotLottie archive is written by
//! `studio-lottie`.

use crate::doc::Doc;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use studio_render::resolve::{Node, resolve};
use studio_render::scene::SourceDesc;
use studio_render::transform::layer_matrix;

/// What an export wrote and left out.
#[derive(Debug, Default)]
pub struct Report {
    pub layers: usize,
    pub frames: u64,
    pub bytes: u64,
    pub left_out: Vec<String>,
}

/// One clip's track through the export.
struct Track {
    order: i32,
    name: String,
    blend: u8,
    first: u64,
    last: u64,
    /// Per frame: position (x, y), scale (x, y) %, rotation °, opacity %.
    values: BTreeMap<u64, [f64; 6]>,
    /// The picture: (width, height, PNG or JPEG bytes, MIME type).
    picture: Option<(u32, u32, Vec<u8>, &'static str)>,
    /// A shape layer: its painted outlines at every frame, exported as
    /// Lottie shapes rather than a picture.
    shapes: Option<Vec<(u64, Vec<studio_vector::draw::Baked>)>>,
}

fn bm(b: studio_render::BlendMode) -> Option<u8> {
    use studio_render::BlendMode as B;
    Some(match b {
        B::Normal => 0,
        B::Multiply => 1,
        B::Screen => 2,
        B::Overlay => 3,
        B::Darken => 4,
        B::Lighten => 5,
        B::ColourDodge => 6,
        B::ColourBurn => 7,
        B::HardLight => 8,
        B::SoftLight => 9,
        B::Difference => 10,
        B::Exclusion => 11,
        B::Hue => 12,
        B::Saturation => 13,
        B::Colour => 14,
        B::Luminosity => 15,
        B::Add => 16,
        _ => return None,
    })
}

fn b64(bytes: &[u8]) -> String {
    crate::agent::mcp::base64(bytes)
}

/// A channel's slope (value per frame) at sample `k`.
fn slope(s: &[(u64, f64)], k: usize) -> f64 {
    let n = s.len();
    let d = |a: usize, b: usize| (s[b].1 - s[a].1) / (s[b].0 - s[a].0).max(1) as f64;
    // At the ends, a second-order one-sided difference (an ease starts
    // flat; the first step alone says otherwise).
    let end = |a: usize, b: usize, c: usize| (-3.0 * s[a].1 + 4.0 * s[b].1 - s[c].1) / (s[c].0 as f64 - s[a].0 as f64).abs().max(1.0);
    match n {
        0 | 1 => 0.0,
        2 => d(0, 1),
        _ if k == 0 => end(0, 1, 2),
        _ if k == n - 1 => -end(n - 1, n - 2, n - 3),
        _ => d(k - 1, k + 1),
    }
}

/// The cubic (Hermite) curve from sample `a` to sample `b` through their
/// values and slopes, at frame `f`.
fn hermite(s: &[(u64, f64)], a: usize, b: usize, f: f64) -> f64 {
    let dt = (s[b].0 - s[a].0) as f64;
    let u = (f - s[a].0 as f64) / dt;
    let (u2, u3) = (u * u, u * u * u);
    s[a].1 * (2.0 * u3 - 3.0 * u2 + 1.0) + slope(s, a) * dt * (u3 - 2.0 * u2 + u) + s[b].1 * (-2.0 * u3 + 3.0 * u2) + slope(s, b) * dt * (u3 - u2)
}

/// Whether a Lottie ease can draw that curve: it runs from one value to
/// another (an ease is a fraction of the change), or stands still.
fn expressible(s: &[(u64, f64)], a: usize, b: usize) -> bool {
    (s[b].1 - s[a].1).abs() > 1e-6 || (slope(s, a).abs() < 1e-9 && slope(s, b).abs() < 1e-9)
}

/// The samples to key: between two keys the curve through their values
/// and slopes stays within `tol` of every frame.
fn fit(s: &[(u64, f64)], tol: f64) -> Vec<usize> {
    let n = s.len();
    if n <= 2 {
        return (0..n).collect();
    }
    let fits = |a: usize, b: usize| expressible(s, a, b) && (a + 1..b).all(|k| (hermite(s, a, b, s[k].0 as f64) - s[k].1).abs() <= tol);
    let mut keys = vec![0];
    let mut a = 0;
    let mut b = 1;
    while b < n - 1 {
        if fits(a, b + 1) {
            b += 1;
        } else {
            keys.push(b);
            a = b;
            b = a + 1;
        }
    }
    keys.push(n - 1);
    keys
}

/// Ease handles from key `a` to key `b`: x at a third and two thirds (time
/// runs evenly), y from the slopes, which makes the ease that same curve.
fn handles(s: &[(u64, f64)], a: usize, b: usize) -> ((f64, f64), (f64, f64)) {
    let (dv, dt) = (s[b].1 - s[a].1, (s[b].0 - s[a].0) as f64);
    if dv.abs() <= 1e-6 {
        return ((0.0, 0.0), (1.0, 1.0));
    }
    ((1.0 / 3.0, slope(s, a) * dt / (3.0 * dv)), (2.0 / 3.0, 1.0 - slope(s, b) * dt / (3.0 * dv)))
}

/// A Lottie property from `dims` channels sampled per frame (frames counted
/// from `first`).
fn channels(chans: &[Vec<(u64, f64)>], first: u64, tol: f64) -> Value {
    let dims = chans.len();
    let still = chans.iter().all(|c| c.iter().all(|(_, v)| (v - c[0].1).abs() <= tol));
    if still {
        return json!({"a": 0, "k": if dims == 1 { json!(chans[0][0].1) } else { json!(chans.iter().map(|c| c[0].1).collect::<Vec<_>>()) }});
    }
    // A frame is keyed when any channel needs it.
    let mut keys: Vec<usize> = chans.iter().flat_map(|c| fit(c, tol)).collect();
    keys.sort_unstable();
    keys.dedup();
    let out: Vec<Value> = keys
        .iter()
        .enumerate()
        .map(|(j, &k)| {
            let mut key = json!({"t": chans[0][k].0 - first, "s": chans.iter().map(|c| c[k].1).collect::<Vec<_>>()});
            if let Some(&next) = keys.get(j + 1) {
                let h: Vec<_> = chans.iter().map(|c| handles(c, k, next)).collect();
                key["o"] = json!({"x": h.iter().map(|h| h.0.0).collect::<Vec<_>>(), "y": h.iter().map(|h| h.0.1).collect::<Vec<_>>()});
                key["i"] = json!({"x": h.iter().map(|h| h.1.0).collect::<Vec<_>>(), "y": h.iter().map(|h| h.1.1).collect::<Vec<_>>()});
            }
            key
        })
        .collect();
    json!({"a": 1, "k": out})
}

/// One channel of a track.
fn channel(track: &Track, i: usize) -> Vec<(u64, f64)> {
    track.values.iter().map(|(f, v)| (*f, v[i])).collect()
}

/// Writes the timeline from `start` to `end` (µs) to `path` (`.json` or
/// `.lottie`).
pub fn export(doc: &Doc, path: &Path, start: i64, end: i64) -> Result<Report, String> {
    let (cw, ch) = (doc.project.width, doc.project.height);
    let fps = doc.project.fps.max(1.0) as f64;
    if end <= start {
        return Err("nothing to export: the range is empty".into());
    }
    crate::editor::render_view::install_text_renderer(ch);
    let model = Arc::new(doc.model.clone());
    let scene = studio_render::model_scene::ModelScene::new(model.clone());
    let frames = (((end - start) as f64 / 1e6) * fps).ceil().max(1.0) as u64;
    let mut tracks: BTreeMap<u64, Track> = BTreeMap::new();
    let mut left_out: Vec<String> = Vec::new();
    let mut note = |s: String| {
        if !left_out.contains(&s) {
            left_out.push(s);
        }
    };
    let clip_name = |id: u64| model.sequences.iter().find_map(|q| q.clip(studio_model::ClipId(id))).map(|c| c.name.clone()).unwrap_or_else(|| format!("clip {id}"));
    for f in 0..frames {
        let t = start + (f as f64 / fps * 1e6).round() as i64;
        let list = resolve(&scene, t);
        for node in &list.nodes {
            let (clip, pose) = match node {
                Node::Layer { clip, pose } => (clip, pose),
                Node::Transition { .. } => {
                    note("transitions (the clips meet with a cut)".into());
                    continue;
                }
                Node::Adjustment { clip } => {
                    note(format!("adjustment layer \"{}\"", clip_name(clip.id)));
                    continue;
                }
            };
            if !tracks.contains_key(&clip.id) {
                let name = clip_name(clip.id);
                if !clip.effects.is_empty() || clip.grade.is_some() {
                    note(format!("effects and grade on \"{name}\""));
                }
                if !clip.masks.is_empty() || clip.warp.is_some() || clip.matte.is_some() {
                    note(format!("masks, pins and mattes on \"{name}\""));
                }
                if clip.space.is_some() {
                    note(format!("3D on \"{name}\" (exported flat)"));
                }
                let vector = matches!(&clip.source, SourceDesc::Generator(g) if g.params.contains_key("vector"));
                let picture = match &clip.source {
                    _ if vector => None,
                    SourceDesc::Generator(g) => {
                        let frame = studio_render::effects::generate(g, cw, ch);
                        Some((cw, ch, frame.to_srgb_png(), "image/png"))
                    }
                    SourceDesc::Media { .. } => {
                        let mc = model.sequences.iter().find_map(|q| q.clip(studio_model::ClipId(clip.id)));
                        let media = mc.and_then(|c| match &c.source {
                            studio_model::ClipSource::Media(r) => model.media.iter().find(|m| m.id == r.media),
                            _ => None,
                        });
                        match media {
                            Some(m) if m.info.kind == studio_model::MediaKind::Image => {
                                let p = Path::new(&m.path);
                                let mime = if crate::convert::sniff(p) == Some("jpg") { "image/jpeg" } else { "image/png" };
                                let v = m.info.video.as_ref();
                                match std::fs::read(p) {
                                    Ok(bytes) if mime == "image/jpeg" || bytes.starts_with(b"\x89PNG") => Some((v.map_or(0, |v| v.width), v.map_or(0, |v| v.height), bytes, mime)),
                                    _ => {
                                        note(format!("\"{name}\": its picture could not be read"));
                                        None
                                    }
                                }
                            }
                            _ => {
                                note(format!("video and sound (\"{name}\")"));
                                None
                            }
                        }
                    }
                    _ => {
                        note(format!("merged clips, cameras and lights (\"{name}\")"));
                        None
                    }
                };
                let blend = bm(clip.blend).unwrap_or_else(|| {
                    note(format!("track mattes (\"{name}\")"));
                    0
                });
                tracks.insert(clip.id, Track { order: clip.order, name, blend, first: f, last: f, values: BTreeMap::new(), picture, shapes: vector.then(Vec::new) });
            }
            let tr = tracks.get_mut(&clip.id).expect("inserted");
            if let (Some(frames), SourceDesc::Generator(g)) = (tr.shapes.as_mut(), &clip.source)
                && let Some((doc, cx, _)) = studio_render::effects::vector_inputs(&g.params, cw, ch)
            {
                frames.push((f, studio_vector::draw::baked(&doc, &cx)));
            }
            let (pw, ph) = match (tr.picture.as_ref(), tr.shapes.is_some()) {
                (Some((pw, ph, _, _)), _) => (*pw, *ph),
                (None, true) => (cw, ch),
                (None, false) => continue,
            };
            let m = layer_matrix([cw as f32, ch as f32], [pw as f32, ph as f32], &clip.placement, pose);
            // The picture's middle, and its axes.
            let mid = m.apply([pw as f32 / 2.0, ph as f32 / 2.0]);
            let o = m.apply([0.0, 0.0]);
            let (ex, ey) = (m.apply([1.0, 0.0]), m.apply([0.0, 1.0]));
            let (a, b, c, d) = ((ex[0] - o[0]) as f64, (ex[1] - o[1]) as f64, (ey[0] - o[0]) as f64, (ey[1] - o[1]) as f64);
            let sx = a.hypot(b);
            let sy = if sx > 0.0 { (a * d - b * c) / sx } else { 0.0 };
            if sx > 0.0 && (a * c + b * d).abs() / (sx * c.hypot(d).max(1e-9)) > 0.01 {
                note(format!("skew on \"{}\"", tr.name));
            }
            let mut rot = b.atan2(a).to_degrees();
            // Turns stay continuous (359° then 361°, not 359° then 1°).
            if let Some((_, prev)) = tr.values.range(..f).next_back() {
                while rot - prev[4] > 180.0 {
                    rot -= 360.0;
                }
                while rot - prev[4] < -180.0 {
                    rot += 360.0;
                }
            }
            let opacity = (clip.placement.opacity * pose.opacity).clamp(0.0, 1.0) as f64 * 100.0;
            tr.values.insert(f, [mid[0] as f64, mid[1] as f64, sx * 100.0, sy * 100.0, rot, opacity]);
            tr.last = f;
        }
    }
    // Top of the stack first, as Lottie lists layers.
    let mut order: Vec<&Track> = tracks.values().filter(|t| (t.picture.is_some() || t.shapes.is_some()) && !t.values.is_empty()).collect();
    order.sort_by_key(|t| (std::cmp::Reverse(t.order), t.first));
    let mut assets = Vec::new();
    let mut layers = Vec::new();
    for (k, tr) in order.iter().enumerate() {
        if let Some(frames) = &tr.shapes {
            // Drawn about the canvas middle, as Studio draws shape layers.
            let local: Vec<(u64, Vec<studio_vector::draw::Baked>)> = frames.iter().map(|(f, b)| (f - tr.first, b.clone())).collect();
            let ks = json!({
                "a": {"a": 0, "k": [0, 0]},
                "p": {"s": true, "x": channels(&[channel(tr, 0)], tr.first, 0.25), "y": channels(&[channel(tr, 1)], tr.first, 0.25)},
                "s": channels(&[channel(tr, 2), channel(tr, 3)], tr.first, 0.1),
                "r": channels(&[channel(tr, 4)], tr.first, 0.1),
                "o": channels(&[channel(tr, 5)], tr.first, 0.25),
            });
            layers.push(json!({"ddd": 0, "ind": k + 1, "ty": 4, "nm": tr.name, "sr": 1, "ks": ks, "ao": 0, "shapes": studio_vector::lottie::shapes(&local), "ip": tr.first, "op": tr.last + 1, "st": tr.first, "bm": tr.blend}));
            continue;
        }
        let (w, h, bytes, mime) = tr.picture.as_ref().expect("filtered");
        let id = format!("image_{k}");
        assets.push(json!({"id": id, "w": w, "h": h, "u": "", "p": format!("data:{mime};base64,{}", b64(bytes)), "e": 1}));
        // Anchored at the picture's middle, so a turn does not move the
        // position; x and y keyed apart, each with its own ease.
        let ks = json!({
            "a": {"a": 0, "k": [*w as f64 / 2.0, *h as f64 / 2.0]},
            "p": {"s": true, "x": channels(&[channel(tr, 0)], tr.first, 0.25), "y": channels(&[channel(tr, 1)], tr.first, 0.25)},
            "s": channels(&[channel(tr, 2), channel(tr, 3)], tr.first, 0.1),
            "r": channels(&[channel(tr, 4)], tr.first, 0.1),
            "o": channels(&[channel(tr, 5)], tr.first, 0.25),
        });
        layers.push(json!({"ddd": 0, "ind": k + 1, "ty": 2, "nm": tr.name, "refId": id, "sr": 1, "ks": ks, "ao": 0, "ip": tr.first, "op": tr.last + 1, "st": tr.first, "bm": tr.blend}));
    }
    if layers.is_empty() {
        return Err(format!("nothing in that range can go into a Lottie file (it holds pictures, titles, cards and shapes; left out: {})", if left_out.is_empty() { "nothing".into() } else { left_out.join("; ") }));
    }
    let name = doc.project.name.clone();
    let docv = json!({"v": "5.12.0", "nm": name, "fr": fps, "ip": 0, "op": frames, "w": cw, "h": ch, "ddd": 0, "assets": assets, "layers": layers, "markers": []});
    let json = serde_json::to_vec(&docv).map_err(|e| e.to_string())?;
    let bytes = if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("lottie")) {
        let id = "animation";
        let manifest = json!({"version": "1", "generator": "Studio", "animations": [{"id": id}]}).to_string();
        studio_lottie::write_dotlottie(&[("manifest.json", manifest.as_bytes()), (&format!("animations/{id}.json"), &json)])
    } else {
        json
    };
    studio_model::file::write_atomic(path, &bytes).map_err(|e| format!("Could not write {}: {e}", path.display()))?;
    Ok(Report { layers: layers.len(), frames, bytes: bytes.len() as u64, left_out })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eased_curves_fold_into_few_keys() {
        // A straight run: its two ends.
        let s: Vec<(u64, f64)> = (0..30).map(|f| (f, f as f64 * 2.0)).collect();
        assert_eq!(fit(&s, 0.01), vec![0, 29]);
        // Ease in and out (smoothstep) over 60 frames: one cubic, two keys,
        // handles that bring it back exactly.
        let s: Vec<(u64, f64)> = (0..=60).map(|f| { let u = f as f64 / 60.0; (f, 1200.0 * u * u * (3.0 - 2.0 * u)) }).collect();
        assert!(fit(&s, 0.25).len() <= 4, "{:?}", fit(&s, 0.25));
        let ((ox, oy), (ix, iy)) = handles(&s, 0, 60);
        for k in 1..60 {
            let x = k as f64 / 60.0;
            let y = studio_lottie::ease(ox, oy, ix, iy, x) * 1200.0;
            assert!((y - s[k as usize].1).abs() < 2.0, "{k}: {y} vs {}", s[k as usize].1);
        }
        // A hold then a jump: keyed at the jump.
        let s: Vec<(u64, f64)> = (0..10).map(|f| (f, if f < 5 { 0.0 } else { 10.0 })).collect();
        let k = fit(&s, 0.01);
        assert!(k.contains(&4) && k.contains(&5), "{k:?}");
    }
}
