// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Cut-out characters sitting in their backdrop, shared by the Inspector's
//! "In the scene" section and the AI editor (`shadow`, `light_match`,
//! `foreground`). Everything is read from the backdrop picture with the
//! characters left out:
//!
//! * its **light**: whether the light itself shows (a sun, a glowing sky:
//!   lit from behind), which side is brighter, how hard it is (how far
//!   apart lit and shaded ground are), the colour of the light and of the
//!   scene's own shadows;
//! * a **shadow** for a character from that light (`meta["shadow"]`, drawn
//!   by the renderer, `studio_render::shadow`);
//! * a **light match**: the character's brightness and colour moved
//!   towards the light around where it stands (a gentle shot match in its
//!   grade, `match_*`) and a `rim_light` on the edges facing the light;
//! * a **foreground piece**: a rock, trunk or grass cut out of the backdrop
//!   (SAM and the depth map) and laid over the characters standing behind
//!   it.

use eframe::egui::Vec2;
use serde_json::{Value, json};
use studio_ai::features::segment::Prompt;
use studio_colour::auto::{ColourStats, linear709_to_oklab};
use studio_media::rgba::RgbaImage;
use studio_model as m;

use crate::doc::Doc;
use crate::editor::xform;
use crate::services::colour_match as cm;
use crate::services::cutout;
use crate::view::{self, Time};

fn lin(c: u8) -> f32 {
    let v = c as f32 / 255.0;
    if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

fn luma(p: &[u8]) -> f32 {
    0.2126 * lin(p[0]) + 0.7152 * lin(p[1]) + 0.0722 * lin(p[2])
}

/// The light of a backdrop, as far as a picture tells it.
#[derive(Clone, Debug, PartialEq)]
pub struct Light {
    /// Where the light comes from, 0..1 of the frame, when it shows (the
    /// sun, a glowing sky): the scene is lit from behind.
    pub source: Option<[f32; 2]>,
    /// -1 lit from the left .. 1 from the right.
    pub side: f32,
    /// 0 overcast (soft, faint shadows) .. 1 hard sun.
    pub hardness: f32,
    /// The light's colour (its brightest parts), sRGB 0..1.
    pub key: [f32; 3],
    /// The colour of the scene's own shadows, sRGB 0..1.
    pub shade: [f32; 3],
}

/// Reads the light of a backdrop picture (sRGB, characters left out).
pub fn read_light(img: &RgbaImage) -> Light {
    let (w, h) = (img.width as usize, img.height as usize);
    let px = |x: usize, y: usize| &img.pixels[(y * w + x) * 4..(y * w + x) * 4 + 4];
    // A coarse grid of brightness.
    let (gw, gh) = (24usize, 14usize);
    let mut cells = vec![0f32; gw * gh];
    // How many near-white pixels each cell has: the sun is white hot.
    let mut white = vec![0usize; gw * gh];
    for gy in 0..gh {
        for gx in 0..gw {
            let (x0, x1, y0, y1) = (gx * w / gw, ((gx + 1) * w / gw).max(gx * w / gw + 1), gy * h / gh, ((gy + 1) * h / gh).max(gy * h / gh + 1));
            let mut s = 0.0;
            for y in y0..y1.min(h) {
                for x in x0..x1.min(w) {
                    let l = luma(px(x, y));
                    s += l;
                    white[gy * gw + gx] += (l > 0.88) as usize;
                }
            }
            cells[gy * gw + gx] = s / ((y1 - y0) * (x1 - x0)) as f32;
        }
    }
    let mut sorted = cells.clone();
    sorted.sort_by(f32::total_cmp);
    let median = sorted[sorted.len() / 2].max(1e-3);
    // The brightest cell in the top three quarters, and how much it stands
    // out: a sun or a glowing patch of sky is the light itself.
    let (mut best, mut at) = (0f32, (0, 0));
    for gy in 0..gh * 3 / 4 {
        for gx in 0..gw {
            if cells[gy * gw + gx] > best {
                (best, at) = (cells[gy * gw + gx], (gx, gy));
            }
        }
    }
    // ... and stands out from the sky beside it too, white hot in good part:
    // a bright but even sky (or a lit cloud) is daylight, not the sun.
    let beside = {
        let (mut s, mut n) = (0.0, 0.0);
        for gy in at.1.saturating_sub(1)..(at.1 + 2).min(gh) {
            for gx in 0..gw {
                if gx.abs_diff(at.0) >= 3 && cells[gy * gw + gx] > 0.6 * median.max(0.05) {
                    s += cells[gy * gw + gx];
                    n += 1.0;
                }
            }
        }
        if n > 0.0 { s / n } else { 0.0 }
    };
    let cell_px = (w / gw) * (h / gh);
    let source = (white[at.1 * gw + at.0] * 6 > cell_px && best > 2.4 * median && best > 1.25 * beside).then(|| [(at.0 as f32 + 0.5) / gw as f32, (at.1 as f32 + 0.5) / gh as f32]);
    // Lit side: the brighter third of the lower part (where faces of
    // things catch the light).
    let third = |a: usize, b: usize| {
        let mut s = 0.0;
        let mut n = 0.0;
        for gy in gh / 3..gh {
            for gx in a..b {
                s += cells[gy * gw + gx];
                n += 1.0;
            }
        }
        s / n
    };
    let (l, r) = (third(0, gw / 3), third(gw - gw / 3, gw));
    let side = match source {
        Some(s) => ((s[0] - 0.5) * 2.5).clamp(-1.0, 1.0),
        None => ((r - l) / (r + l).max(1e-3) * 3.0).clamp(-1.0, 1.0),
    };
    // Hardness: how far apart lit and shaded ground are.
    let mut ground: Vec<(f32, [u8; 3])> = Vec::new();
    let step = (w / 160).max(1);
    for y in (h / 2..h).step_by(step) {
        for x in (0..w).step_by(step) {
            let p = px(x, y);
            ground.push((luma(p), [p[0], p[1], p[2]]));
        }
    }
    ground.sort_by(|a, b| a.0.total_cmp(&b.0));
    let q = |f: f32| ground[((ground.len() - 1) as f32 * f) as usize].0.max(1e-4);
    let ratio = (q(0.9) / q(0.1)).log2();
    let hardness = (((ratio - 1.5) / 3.5).clamp(0.0, 1.0) + if source.is_some() { 0.15 } else { 0.0 }).min(1.0);
    let mean = |v: &[(f32, [u8; 3])]| {
        let n = v.len().max(1) as f32;
        [0, 1, 2].map(|i| v.iter().map(|p| p.1[i] as f32 / 255.0).sum::<f32>() / n)
    };
    let shade = mean(&ground[..(ground.len() / 10).max(1)]);
    let mut all: Vec<(f32, [u8; 3])> = Vec::new();
    for y in (0..h).step_by(step) {
        for x in (0..w).step_by(step) {
            let p = px(x, y);
            all.push((luma(p), [p[0], p[1], p[2]]));
        }
    }
    all.sort_by(|a, b| b.0.total_cmp(&a.0));
    let key = mean(&all[..(all.len() / 30).max(1)]);
    Light { source, side, hardness, key, shade }
}

/// A shadow for a character whose feet are at `feet` (0..1 of the frame),
/// in `meta["shadow"]`'s terms: angle, length, softness, opacity, contact,
/// colour. `aspect` is the frame's width over its height.
pub fn shadow_settings(light: &Light, feet: [f32; 2], horizon: f32, aspect: f32) -> serde_json::Map<String, Value> {
    let hard = light.hardness;
    let (angle, length) = match light.source {
        // Lit from behind: shadows come towards the camera, spreading out
        // from below the light; a low light throws them long.
        Some(s) => {
            let a = (feet[1] - s[1]).atan2((feet[0] - s[0]) * aspect).to_degrees();
            let low = (s[1] / horizon.max(0.05)).clamp(0.0, 1.0);
            (a, 0.35 + 0.6 * low)
        }
        // Lit from the camera's side: they fall away into the picture,
        // and across it away from the brighter side.
        None => {
            let across = if light.side.abs() < 0.15 { 0.0 } else { -light.side };
            ((-0.55f32).atan2(across).to_degrees(), 0.3 + 0.35 * light.side.abs())
        }
    };
    let mix = |soft: f32, sharp: f32| soft + (sharp - soft) * hard;
    let shade = light.shade.map(|c| (c * 0.55).clamp(0.0, 1.0));
    let r = |v: f32, k: f32| (v * k).round() / k;
    let mut o = serde_json::Map::new();
    o.insert("angle".into(), json!(r(angle.rem_euclid(360.0), 1.0)));
    o.insert("length".into(), json!(r(length * mix(0.55, 1.0), 100.0)));
    o.insert("softness".into(), json!(r(mix(0.11, 0.025), 1000.0)));
    o.insert("opacity".into(), json!(r(mix(0.3, 0.55), 100.0)));
    o.insert("contact".into(), json!(r(mix(0.65, 0.55), 100.0)));
    o.insert("colour".into(), json!(shade.map(|c| r(c, 1000.0))));
    o
}

/// Where light from a backdrop catches a character centred at `centre`
/// (0..1 of the frame): the angle it comes from on screen, clockwise from
/// pointing right, and how strongly.
pub fn rim_settings(light: &Light, centre: [f32; 2], aspect: f32) -> (f32, f32) {
    match light.source {
        Some(s) => ((s[1] - centre[1]).atan2((s[0] - centre[0]) * aspect).to_degrees().rem_euclid(360.0), 0.45 + 0.4 * light.hardness),
        None => ((-0.6f32).atan2(if light.side.abs() < 0.15 { 0.0 } else { light.side }).to_degrees().rem_euclid(360.0), (0.12 + 0.35 * light.hardness * light.side.abs()).min(0.5)),
    }
}

/// Oklab statistics of the pixels of `img` inside `bbox` (0..1 of the frame).
pub fn region_stats(img: &RgbaImage, bbox: [f32; 4]) -> ColourStats {
    let (w, h) = (img.width as usize, img.height as usize);
    let (x0, y0) = ((bbox[0].clamp(0.0, 1.0) * w as f32) as usize, (bbox[1].clamp(0.0, 1.0) * h as f32) as usize);
    let (x1, y1) = (((bbox[2].clamp(0.0, 1.0) * w as f32) as usize).max(x0 + 1).min(w), ((bbox[3].clamp(0.0, 1.0) * h as f32) as usize).max(y0 + 1).min(h));
    let mut labs = Vec::new();
    for y in y0..y1 {
        for x in x0..x1 {
            let p = &img.pixels[(y * w + x) * 4..(y * w + x) * 4 + 3];
            labs.push(linear709_to_oklab([lin(p[0]), lin(p[1]), lin(p[2])]));
        }
    }
    stats(&labs)
}

fn stats(labs: &[[f32; 3]]) -> ColourStats {
    let n = labs.len().max(1) as f32;
    let mean = [0, 1, 2].map(|i| labs.iter().map(|l| l[i]).sum::<f32>() / n);
    let std = [0, 1, 2].map(|i| (labs.iter().map(|l| (l[i] - mean[i]).powi(2)).sum::<f32>() / n).sqrt());
    ColourStats { mean, std }
}

/// Oklab statistics of a picture's solid pixels (linear, premultiplied).
pub fn picture_stats(pic: &studio_render::frame::Frame) -> ColourStats {
    let labs: Vec<[f32; 3]> = pic.pixels.iter().filter(|p| p[3] > 0.5).map(|p| linear709_to_oklab([p[0] / p[3], p[1] / p[3], p[2] / p[3]])).collect();
    stats(&labs)
}

/// What a character's own colours should become in light like `around`:
/// darker in shade, brighter in sun, tinted by the light and the bounce of
/// the place, a little flatter in haze. Its own colours stay its own.
pub fn lit_like(have: &ColourStats, around: &ColourStats, amount: f32) -> ColourStats {
    let k = amount.clamp(0.0, 2.0);
    // A daylight scene averages about 0.62 in Oklab lightness.
    let dl = (0.6 * (around.mean[0] - 0.62)).clamp(-0.25, 0.08) * k;
    let da = (0.25 * around.mean[1]).clamp(-0.03, 0.03) * k;
    let db = (0.25 * around.mean[2]).clamp(-0.03, 0.03) * k;
    let flat = 1.0 + (((around.std[0] / 0.18).max(1e-3).powf(0.25)).clamp(0.85, 1.05) - 1.0) * k;
    ColourStats { mean: [have.mean[0] + dl, have.mean[1] + da, have.mean[2] + db], std: [have.std[0] * flat, have.std[1], have.std[2]] }
}

/// Where a cut-out stands on its own picture: the middle of the lowest
/// solid row of its body, 0..1 across and down. The body runs down from
/// its widest row without a gap, so a stray bit lower in the picture (a
/// neighbour on a character sheet) is not taken for the feet.
pub fn feet_of(pic: &studio_render::frame::Frame) -> [f32; 2] {
    let (w, h) = (pic.width as usize, pic.height as usize);
    let solid = |x: usize, y: usize| pic.pixels[y * w + x][3] > 0.5;
    let count: Vec<usize> = (0..h).map(|y| (0..w).filter(|&x| solid(x, y)).count()).collect();
    let Some(widest) = (0..h).max_by_key(|&y| count[y]).filter(|&y| count[y] > 0) else { return [0.5, 0.97] };
    let mut bottom = widest;
    while bottom + 1 < h && count[bottom + 1] > 0 {
        bottom += 1;
    }
    let band = (h / 25).max(2);
    let (mut sx, mut n) = (0.0, 0.0);
    for y in bottom.saturating_sub(band)..=bottom {
        for x in 0..w {
            if solid(x, y) {
                sx += x as f32 + 0.5;
                n += 1.0;
            }
        }
    }
    [if n > 0.0 { sx / n / w as f32 } else { 0.5 }, (bottom as f32 + 0.8) / h as f32]
}


/// Whether a clip is a piece of a backdrop laid over the characters.
pub fn is_piece(mc: &m::Clip) -> bool {
    mc.meta.contains_key("foreground")
}

/// The pictures above the main lane at `at` (characters, titles), pieces
/// of the backdrop excepted: what to leave out to see the backdrop.
pub fn above_backdrop(doc: &Doc, at: Time) -> Vec<u32> {
    let p = &doc.project;
    let spine = p.spine();
    p.clips
        .iter()
        .filter(|c| Some(c.lane) != spine && c.start <= at && at < c.end())
        .filter(|c| !doc.model.active().clip(m::ClipId(c.id as u64)).is_some_and(is_piece))
        .map(|c| c.id)
        .collect()
}

/// The characters (pictures over the main lane) showing at `at`.
pub fn characters_at(doc: &Doc, at: Time) -> Vec<u32> {
    let p = &doc.project;
    above_backdrop(doc, at).into_iter().filter(|id| p.clip(*id).is_some_and(|c| c.kind == view::ClipKind::Video && c.media.is_some())).collect()
}

/// The backdrop at `at`, `width` pixels wide, with everything above the
/// main lane left out (drawn on the CPU, for the Inspector; the AI editor
/// uses its own renderer).
pub fn backdrop_picture(doc: &Doc, at: Time, width: u32) -> Result<RgbaImage, String> {
    let leave = above_backdrop(doc, at);
    let mut model = doc.model.clone();
    for seq in model.sequences.iter_mut() {
        seq.clips.retain(|c| !leave.contains(&(c.id.0 as u32)));
    }
    let registry = studio_render::source::SourceRegistry::new();
    crate::editor::render_view::register_sources(doc, &registry);
    let mut r = studio_render::CpuCompositor::new(studio_render::Sources::new(registry, studio_render::DecodeCache::new(64 << 20)));
    let scene = studio_render::model_scene::ModelScene::new(std::sync::Arc::new(model));
    let (pw, ph) = (doc.project.width.max(1), doc.project.height.max(1));
    let w = width.min(pw).max(16);
    let h = ((w as f64 * ph as f64 / pw as f64).round() as u32).max(16);
    let e = studio_render::FrameRenderer::render_encoded(&mut r, &scene, at.max(0), Some((w, h)), &studio_render::OutputSettings { premultiplied: false, ..Default::default() }, studio_render::OutputDepth::Eight, 0);
    let pixels = match e.pixels {
        studio_render::EncodedPixels::Rgba8(v) => v,
        studio_render::EncodedPixels::Rgba16(v) => v.iter().map(|x| (x >> 8) as u8).collect(),
    };
    Ok(RgbaImage { width: e.width, height: e.height, pixels })
}

/// Where the far land ends in a backdrop (0..1 from the top), from its
/// depth when the depth model is installed, else 0.4.
pub fn horizon(img: &RgbaImage) -> f32 {
    let store = studio_ai::store::ModelStore::default();
    if !store.is_installed(studio_ai::registry::ModelId::DepthAnythingV2Small) {
        return 0.4;
    }
    let Ok(est) = studio_ai::features::depth::DepthEstimator::load(&store) else { return 0.4 };
    let Ok(frame) = studio_ai::frame::Frame::new(img.width as usize, img.height as usize, img.pixels.clone()) else { return 0.4 };
    let Ok(d) = est.estimate(&frame) else { return 0.4 };
    crate::agent::sight::ground::DepthGrid::shrink(d.width, d.height, &d.data, 96).horizon().clamp(0.05, 0.9)
}

/// A media clip's picture at `at` (after its crop, before its pins),
/// linear and premultiplied.
pub fn layer_source(doc: &Doc, id: u32, at: Time) -> Result<studio_render::frame::Frame, String> {
    let mc = doc.model.active().clip(m::ClipId(id as u64)).ok_or_else(|| format!("no clip {id}"))?;
    let m::ClipSource::Media(r) = &mc.source else { return Err(format!("clip {id} is not a picture or a video (a title or shape has no ground to stand on)")) };
    let mi = doc.project.media(r.media.0 as u32).ok_or("the clip's media is gone")?;
    let path = mi.path.clone().ok_or("sample media has no file")?;
    let mut src = studio_render::source::open_path(&path)?;
    let idx = src.frame_index(mc.source_time_at(at));
    let mut got = None;
    src.decode_to(idx, &mut |i, pic| {
        if i == idx {
            got = Some(pic.to_frame(true));
        }
    });
    let full = got.ok_or("the clip's picture could not be decoded")?;
    let c = mc.transform_at(at - mc.start).crop;
    let (fw, fh) = (full.width as f64, full.height as f64);
    let (x, y) = (((c.left * fw).floor() as u32).min(full.width - 1), ((c.top * fh).floor() as u32).min(full.height - 1));
    let (w, h) = ((((1.0 - c.left - c.right) * fw).round() as u32).clamp(1, full.width - x), (((1.0 - c.top - c.bottom) * fh).round() as u32).clamp(1, full.height - y));
    Ok(if (x, y, w, h) == (0, 0, full.width, full.height) { full } else { full.crop(x, y, w, h) })
}

/// A layer point (0..1 of its picture after crop) on the frame at `at`,
/// as 0..1 of the frame.
pub fn layer_on_frame(doc: &Doc, id: u32, at: Time, uv: [f32; 2]) -> Option<[f32; 2]> {
    let map = crate::editor::pins::LayerMap::at(&doc.project, &doc.model, id, at)?;
    let c = map.flat_canvas(m::Vec2::new(uv[0] as f64, uv[1] as f64));
    let (w, h) = (doc.project.width as f32, doc.project.height as f32);
    Some([c.x / w + 0.5, c.y / h + 0.5])
}

/// The layer's box on the frame (0..1), from its picture's corners.
pub fn layer_box(doc: &Doc, id: u32, at: Time) -> Option<[f32; 4]> {
    let pts: Vec<[f32; 2]> = [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]].iter().filter_map(|uv| layer_on_frame(doc, id, at, *uv)).collect();
    (pts.len() == 4).then(|| [pts.iter().map(|p| p[0]).fold(f32::MAX, f32::min), pts.iter().map(|p| p[1]).fold(f32::MAX, f32::min), pts.iter().map(|p| p[0]).fold(f32::MIN, f32::max), pts.iter().map(|p| p[1]).fold(f32::MIN, f32::max)])
}

/// The feet of a clip's picture (0..1 of it), from its lowest solid row.
pub fn feet(doc: &Doc, id: u32, at: Time) -> Result<[f32; 2], String> {
    let ft = feet_of(&layer_source(doc, id, at)?);
    Ok([(ft[0] * 1000.0).round() / 1000.0, (ft[1] * 1000.0).round() / 1000.0])
}

/// Sets `mc`'s shadow from a backdrop's light (keeping its feet and ground
/// line), and says what it read.
pub fn auto_shadow(doc: &Doc, mc: &mut m::Clip, at: Time, light: &Light, horizon: f32) -> Result<String, String> {
    let id = mc.id.0 as u32;
    let mut o = mc.meta.get("shadow").and_then(Value::as_object).cloned().unwrap_or_default();
    o.remove("enabled");
    if !o.contains_key("feet") {
        o.insert("feet".into(), json!(feet(doc, id, at)?));
    }
    let ft = o.get("feet").and_then(Value::as_array).map(|a| [a[0].as_f64().unwrap_or(0.5) as f32, a.get(1).and_then(Value::as_f64).unwrap_or(0.97) as f32]).unwrap_or([0.5, 0.97]);
    let on_frame = layer_on_frame(doc, id, at, ft).unwrap_or([0.5, 0.8]);
    let aspect = doc.project.width as f32 / doc.project.height.max(1) as f32;
    o.extend(shadow_settings(light, on_frame, horizon, aspect));
    let angle = o["angle"].as_f64().unwrap_or(0.0) as f32;
    let said = format!(
        "{}; its shadow falls {} ({angle:.0}°), {:.2} of its height long, {}",
        describe_light(light),
        fall_words(angle),
        o["length"].as_f64().unwrap_or(0.0),
        if light.hardness > 0.6 { "crisp" } else if light.hardness > 0.3 { "soft" } else { "very soft and faint" }
    );
    mc.meta.insert("shadow".into(), Value::Object(o));
    Ok(said)
}

/// Lights `mc` like the place it stands in at `at` (`img`: the backdrop):
/// a match in its grade and, when the light has a side, a rim light
/// (effect id `rim_id`). `amount` and `rim` scale them (1 = as read).
pub fn light_match(doc: &Doc, mc: &mut m::Clip, at: Time, img: &RgbaImage, amount: f32, rim: f32, rim_id: u64) -> Result<String, String> {
    let id = mc.id.0 as u32;
    let light = read_light(img);
    let bx = layer_box(doc, id, at).ok_or_else(|| format!("clip {id} is not on screen then"))?;
    // Around where it stands: its box, wider, and the ground at its feet.
    let (bw, bh) = (bx[2] - bx[0], bx[3] - bx[1]);
    let around = region_stats(img, [bx[0] - 0.4 * bw, bx[1] - 0.1 * bh, bx[2] + 0.4 * bw, bx[3] + 0.15 * bh]);
    let have = picture_stats(&layer_source(doc, id, at)?);
    let want = lit_like(&have, &around, amount);
    cm::clear(mc);
    cm::set(mc, &have, &want, 1.0);
    let r3 = |v: f32| ((v * 1000.0).round() / 1000.0) as f64;
    mc.meta.insert("light_match".into(), json!({"amount": r3(amount), "at": at as f64 / 1e6, "around": [r3(around.mean[0]), r3(around.mean[1]), r3(around.mean[2])]}));
    mc.effects.retain(|e| !(e.kind == "rim_light" && e.extra.contains_key("light_match")));
    let aspect = doc.project.width as f32 / doc.project.height.max(1) as f32;
    let (from, strength) = rim_settings(&light, [(bx[0] + bx[2]) / 2.0, (bx[1] + bx[3]) / 2.0], aspect);
    let strength = strength * rim.clamp(0.0, 2.0);
    if strength > 0.02 {
        // The effect works on the picture itself: the screen angle turned
        // into the picture's own (flipped, rotated).
        let tr = mc.transform_at(at - mc.start);
        let mut a = from - tr.rotation as f32;
        if (tr.scale_x < 0.0) != tr.flip_x {
            a = 180.0 - a;
        }
        if (tr.scale_y < 0.0) != tr.flip_y {
            a = -a;
        }
        let mut e = m::Effect::new(m::EffectId(rim_id), "rim_light");
        e.params.insert("angle".into(), m::Value::Scalar(a.rem_euclid(360.0).round() as f64));
        e.params.insert("intensity".into(), m::Value::Scalar(r3(strength)));
        e.params.insert("colour".into(), m::Value::Colour(m::Rgba { r: light.key[0] as f64, g: light.key[1] as f64, b: light.key[2] as f64, a: 1.0 }));
        e.params.insert("width".into(), m::Value::Scalar(if light.source.is_some() { 1.2 } else { 0.8 }));
        e.extra.insert("light_match".into(), json!(true));
        mc.effects.push(e);
    }
    let dl = want.mean[0] - have.mean[0];
    Ok(format!(
        "{}{}; rim light {}",
        if dl.abs() < 0.015 { "brightness kept".to_string() } else if dl < 0.0 { format!("darkened {:.0}% for the shade around it", -dl / have.mean[0].max(0.05) * 100.0) } else { format!("brightened {:.0}% for the light around it", dl / have.mean[0].max(0.05) * 100.0) },
        if (want.mean[2] - have.mean[2]).abs() > 0.005 || (want.mean[1] - have.mean[1]).abs() > 0.005 { format!(", tinted {}", tint_words(want.mean[1] - have.mean[1], want.mean[2] - have.mean[2])) } else { String::new() },
        if strength > 0.02 { format!("from {} at {:.0}%", from_words(from), strength * 100.0) } else { "left off (flat light)".into() }
    ))
}

/// Takes a light match off: its grade match and its rim light.
pub fn clear_light_match(mc: &mut m::Clip) -> bool {
    let had = mc.meta.remove("light_match").is_some();
    let n = mc.effects.len();
    mc.effects.retain(|e| !(e.kind == "rim_light" && e.extra.contains_key("light_match")));
    if had {
        cm::clear(mc);
    }
    had || n != mc.effects.len()
}

/// A piece of a backdrop to lay over the characters: what to pick on the
/// backdrop at  (canvas px from the centre), the ground behind it (0..1
/// of the frame, when known: the cut keeps what stands nearer than halfway
/// from there), what to call it, and which characters it goes over (else
/// those it overlaps then).
pub struct PieceCut {
    pub backdrop: u32,
    pub at: Time,
    pub pick: cutout::Pick,
    pub behind: Option<[f32; 2]>,
    pub kind: String,
    pub hides: Option<Vec<u32>>,
}

fn applied(doc: &mut Doc, _ok: bool) -> Result<(), String> {
    match doc.error.take() {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

/// Cuts a piece out of a backdrop (SAM on the pick, kept to what the depth
/// map says stands nearer than the ground behind it) into a matte, and
/// lays a copy of the backdrop showing only that piece on a new lane over
/// the characters it hides. Returns the new clip and what was done.
pub fn lay_piece(doc: &mut Doc, cut: &PieceCut) -> Result<(u32, String), String> {
    let p = doc.project.clone();
    let spine = p.spine().ok_or("there is no main lane")?;
    let (backdrop, at, behind, kind) = (cut.backdrop, cut.at, cut.behind, cut.kind.as_str());
    let (w, h) = (p.width as f32, p.height as f32);
    let to_canvas = |q: [f32; 2]| Vec2::new((q[0] - 0.5) * w, (q[1] - 0.5) * h);
    let store = studio_ai::ModelStore::default();
    let depth_id = studio_ai::registry::ModelId::DepthAnythingV2Small;
    if !store.is_installed(depth_id) {
        store.download(depth_id, &studio_ai::jobs::JobCtx::detached()).map_err(|e| format!("could not download the depth model ({e}); get it in Settings → AI models"))?;
    }
    let job = cutout::plan(doc, backdrop, Some((&cut.pick, at)), 0.0, 1)?;
    cutout::models_ready(&job)?;
    let cutout::Subject::Object { at: frame_at, prompts } = &job.subject else { return Err("the backdrop could not be picked".into()) };
    let frame = job.frame(*frame_at)?;
    let depth = {
        let est = studio_ai::features::depth::DepthEstimator::load(&store).map_err(|e| format!("the depth model is not ready ({e})"))?;
        est.estimate(&frame).map_err(|e| e.to_string())?
    };
    let (fw, fh) = (frame.width, frame.height);
    let seed = prompts
        .iter()
        .find_map(|p| match p {
            Prompt::Point { x, y, positive: true } => Some((*x, *y)),
            _ => None,
        })
        .unwrap_or((fw as f32 / 2.0, fh as f32 / 2.0));
    let at_px = |x: f32, y: f32| (y.clamp(0.0, fh as f32 - 1.0) as usize) * fw + x.clamp(0.0, fw as f32 - 1.0) as usize;
    // The piece's depth (around the pick), and the cut: what stands nearer
    // than halfway from the land behind it to the piece.
    let mut near_seed: Vec<f32> = Vec::new();
    for dy in -4..=4 {
        for dx in -4..=4 {
            near_seed.push(depth.data[at_px(seed.0 + dx as f32 * 2.0, seed.1 + dy as f32 * 2.0)]);
        }
    }
    near_seed.sort_by(f32::total_cmp);
    let d_piece = near_seed[near_seed.len() / 2];
    let d_behind = match behind {
        Some(bq) => {
            // The land behind, on the backdrop's own picture.
            let c = to_canvas(bq);
            let geo = xform::geo(&p, &doc.model, backdrop, at).ok_or("the backdrop is not on screen")?;
            let s = geo.to_source(c).ok_or("that ground is off the backdrop")?;
            let src = geo.source();
            depth.data[at_px(s.x / src.x * fw as f32, s.y / src.y * fh as f32)]
        }
        None => d_piece - 0.2,
    };
    let cut_at = if d_piece - d_behind > 0.04 { d_behind + 0.5 * (d_piece - d_behind) } else { d_piece - 0.1 };
    // What the depth says the piece is: everything nearer than the cut
    // joined to the pick.
    let mut near = studio_ai::frame::Mask::zeros(fw, fh);
    for i in 0..fw * fh {
        near.data[i] = (depth.data[i] >= cut_at) as u8 as f32;
    }
    let region = joined(&near, seed);
    let region_ok = region.coverage() > 0.0005 && region.coverage() < 0.4;
    // SAM, prompted with that region's box, gives clean edges where it
    // agrees with the depth; where it picked only a part, the depth wins.
    let sam = studio_ai::features::segment::ObjectCutout::load(&store).map_err(|e| e.to_string())?;
    let mut ask: Vec<Prompt> = prompts.iter().filter(|p| !matches!(p, Prompt::Box { .. })).copied().collect();
    match region.bbox().filter(|_| region_ok) {
        Some((x0, y0, x1, y1)) => ask.push(Prompt::Box { x0: x0 as f32, y0: y0 as f32, x1: x1 as f32, y1: y1 as f32 }),
        None => ask.extend(prompts.iter().filter(|p| matches!(p, Prompt::Box { .. })).copied()),
    }
    let sel = sam.cut_out(&frame, &ask).map_err(|e| e.to_string())?;
    let sam_mask = sel.mask.resized(fw, fh);
    let both = (0..fw * fh).filter(|&i| sam_mask.data[i] > 0.5 && region.data[i] > 0.5).count() as f32;
    let sam_ok = sel.present && both > 0.6 * region.coverage() * (fw * fh) as f32;
    let mut keep = studio_ai::frame::Mask::zeros(fw, fh);
    if region_ok {
        // SAM's edge, within a few pixels of what the depth says.
        let grown = grow(&region, (fw.max(fh) / 150).max(2));
        for i in 0..fw * fh {
            keep.data[i] = if sam_ok { (sam_mask.data[i] > 0.5 && grown.data[i] > 0.5) as u8 as f32 } else { region.data[i] };
        }
    } else if sel.present {
        // The depth did not single it out (a piece at the depth of the land
        // around it): SAM's pick, nearer than the cut.
        for i in 0..fw * fh {
            keep.data[i] = (sam_mask.data[i] > 0.5 && depth.data[i] >= cut_at - 0.05) as u8 as f32;
        }
    }
    if keep.coverage() < 0.0003 {
        return Err("the cut came out empty: pick the piece with a point well inside it, or a box around it".into());
    }
    let keep = studio_ai::features::matting::refine_edges(&frame, &keep);
    let cover = keep.coverage();
    let mut wr = studio_ai::features::matte_file::Writer::create(&job.file, fw as u32, fh as u32, job.range.start).map_err(|e| e.to_string())?;
    for _ in job.range.clone() {
        wr.push(&keep).map_err(|e| e.to_string())?;
    }
    wr.finish().map_err(|e| e.to_string())?;
    // Which characters it goes over: `hides`, else those showing then
    // that stand where it is.
    let pbox = {
        let km = keep.bbox().unwrap_or((0, 0, fw, fh));
        let geo = xform::geo(&p, &doc.model, backdrop, at).ok_or("the backdrop is not on screen")?;
        let src = geo.source();
        let a = geo.to_canvas(Vec2::new(km.0 as f32 / fw as f32 * src.x, km.1 as f32 / fh as f32 * src.y));
        let b = geo.to_canvas(Vec2::new(km.2 as f32 / fw as f32 * src.x, km.3 as f32 / fh as f32 * src.y));
        [a.x.min(b.x) / w + 0.5, a.y.min(b.y) / h + 0.5, a.x.max(b.x) / w + 0.5, a.y.max(b.y) / h + 0.5]
    };
    let overlaps = |doc: &Doc, c: u32| layer_box(doc, c, at).is_some_and(|b| b[0] < pbox[2] && pbox[0] < b[2] && b[1] < pbox[3] && pbox[1] < b[3]);
    let hides: Vec<u32> = match &cut.hides {
        Some(v) => v.clone(),
        None => characters_at(doc, at).into_iter().filter(|c| overlaps(doc, *c)).collect(),
    };
    let lanes = &p.lanes;
    let idx = |l: u32| lanes.iter().position(|x| x.id == l);
    let spine_i = idx(spine).unwrap_or(lanes.len());
    let top = hides.iter().filter_map(|c| p.clip(*c)).filter_map(|c| idx(c.lane)).min().unwrap_or(spine_i);
    let seq = doc.seq();
    let lid = doc.alloc_id();
    let ok = doc.apply("Add lane", m::Edit::AddLane { seq, lane: m::Lane::new(m::LaneId(lid), m::LaneKind::Visual, "Foreground"), index: Some(top) });
    applied(doc, ok)?;
    let mut mc = doc.model.active().clip(m::ClipId(backdrop as u64)).cloned().ok_or("the backdrop is gone")?;
    let pid = doc.alloc_id();
    mc.id = m::ClipId(pid);
    mc.lane = m::LaneId(lid);
    mc.name = format!("{kind} from {}", mc.name);
    mc.name[..1].make_ascii_uppercase();
    mc.link = None;
    mc.group = None;
    mc.meta.remove("shadow");
    mc.meta.remove("light_match");
    let r = cutout::CutResult { frames: (job.range.end - job.range.start) as usize, coverage: cover, empty: 0 };
    cutout::store_matte(&mut mc, &job, &r, false);
    mc.meta.insert("foreground".into(), json!({"from": backdrop, "hides": hides, "depth": d_piece, "cut": cut_at}));
    let ok = doc.apply("Foreground piece", m::Edit::InsertClip { seq, clip: Box::new(mc) });
    applied(doc, ok)?;
    let front: Vec<u32> = characters_at(doc, at).into_iter().filter(|c| !hides.contains(c) && *c as u64 != pid && p.clip(*c).and_then(|k| idx(k.lane)).is_some_and(|i| i >= top) && overlaps(doc, *c)).collect();
    let (cx0, cy0) = ((pbox[0] - 0.5) * w, (pbox[1] - 0.5) * h);
    let (cx1, cy1) = ((pbox[2] - 0.5) * w, (pbox[3] - 0.5) * h);
    let mut note = format!(
        "clip {pid}: the {kind} cut from clip {backdrop} ({:.1}% of it, box [{:.0}, {:.0}]-[{:.0}, {:.0}]) on a new lane over {}; it moves with the backdrop (same transform, keys and pins). A character goes behind it with its feet on the ground behind (higher up the frame than the piece's base) and smaller for the distance",
        cover * 100.0,
        cx0,
        cy0,
        cx1,
        cy1,
        if hides.is_empty() { "nothing yet".to_string() } else { format!("clip(s) {hides:?}") }
    );
    if !front.is_empty() {
        note += &format!(". Clip(s) {front:?} overlap it but are under it now: restack them in_front_of {pid} if they stand in front");
    }
    if !sam_ok {
        note += ". The cut follows the depth (SAM saw the piece differently): frame it to check its edges";
    }
    Ok((pid as u32, note))
}

/// `m` grown by `r` pixels (a square neighbourhood, in two passes).
fn grow(m: &studio_ai::frame::Mask, r: usize) -> studio_ai::frame::Mask {
    let (w, h) = (m.width, m.height);
    let mut a = studio_ai::frame::Mask::zeros(w, h);
    for y in 0..h {
        for x in 0..w {
            a.data[y * w + x] = (x.saturating_sub(r)..(x + r + 1).min(w)).any(|xx| m.data[y * w + xx] > 0.5) as u8 as f32;
        }
    }
    let mut b = studio_ai::frame::Mask::zeros(w, h);
    for y in 0..h {
        for x in 0..w {
            b.data[y * w + x] = (y.saturating_sub(r)..(y + r + 1).min(h)).any(|yy| a.data[yy * w + x] > 0.5) as u8 as f32;
        }
    }
    b
}

/// The part of `m` joined to `seed` (pixels).
fn joined(m: &studio_ai::frame::Mask, seed: (f32, f32)) -> studio_ai::frame::Mask {
    let (w, h) = (m.width, m.height);
    let mut out = studio_ai::frame::Mask::zeros(w, h);
    let (sx, sy) = ((seed.0.max(0.0) as usize).min(w - 1), (seed.1.max(0.0) as usize).min(h - 1));
    if m.data[sy * w + sx] < 0.5 {
        return out;
    }
    let mut stack = vec![(sx, sy)];
    out.data[sy * w + sx] = 1.0;
    while let Some((x, y)) = stack.pop() {
        for (dx, dy) in [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)] {
            let (nx, ny) = (x as i32 + dx, y as i32 + dy);
            if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
                continue;
            }
            let (nx, ny) = (nx as usize, ny as usize);
            let i = ny * w + nx;
            if out.data[i] == 0.0 && m.data[i] > 0.5 {
                out.data[i] = 1.0;
                stack.push((nx, ny));
            }
        }
    }
    out
}

/// Where the light comes from, in words.
pub fn describe_light(l: &Light) -> String {
    let q = if l.hardness > 0.6 {
        "hard light"
    } else if l.hardness > 0.3 {
        "soft light"
    } else {
        "flat, overcast light"
    };
    match l.source {
        Some(s) => format!("{q} from a bright source in the picture at {:.0}% across, {:.0}% down (the scene is lit from behind)", s[0] * 100.0, s[1] * 100.0),
        None if l.side.abs() < 0.15 => format!("{q} from in front"),
        None => format!("{q} from the {}", if l.side < 0.0 { "left" } else { "right" }),
    }
}

pub fn fall_words(angle: f32) -> &'static str {
    let a = angle.rem_euclid(360.0);
    match a {
        a if !(22.5..337.5).contains(&a) => "to the right",
        a if a < 67.5 => "to the right and towards the camera",
        a if a < 112.5 => "towards the camera",
        a if a < 157.5 => "to the left and towards the camera",
        a if a < 202.5 => "to the left",
        a if a < 247.5 => "to the left and back",
        a if a < 292.5 => "straight back",
        _ => "to the right and back",
    }
}

pub fn from_words(angle: f32) -> &'static str {
    let a = angle.rem_euclid(360.0);
    match a {
        a if !(22.5..337.5).contains(&a) => "the right",
        a if a < 67.5 => "the lower right",
        a if a < 112.5 => "below",
        a if a < 157.5 => "the lower left",
        a if a < 202.5 => "the left",
        a if a < 247.5 => "the upper left",
        a if a < 292.5 => "above",
        _ => "the upper right",
    }
}

pub fn tint_words(da: f32, db: f32) -> &'static str {
    if db.abs() >= da.abs() {
        if db > 0.0 { "warmer" } else { "cooler" }
    } else if da > 0.0 {
        "towards magenta"
    } else {
        "towards green"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A backdrop: sky over ground, with a bright sun at `sun` (0..1) when
    /// given and the ground lit from the right otherwise.
    fn backdrop(sun: Option<[f32; 2]>) -> RgbaImage {
        let (w, h) = (192u32, 108u32);
        let mut px = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let (u, v) = (x as f32 / w as f32, y as f32 / h as f32);
                let mut c = if v < 0.45 { [120.0, 160.0, 210.0] } else { [70.0 + 60.0 * u, 110.0 + 40.0 * u, 50.0] };
                if v > 0.45 && sun.is_none() && (x / 12 + y / 9) % 3 == 0 {
                    c = c.map(|k| k * 0.3);
                }
                if let Some(s) = sun {
                    let d = ((u - s[0]).powi(2) + ((v - s[1]) * 0.56).powi(2)).sqrt();
                    if d < 0.08 {
                        c = [255.0, 245.0, 210.0];
                    }
                }
                px.extend([c[0] as u8, c[1] as u8, c[2] as u8, 255]);
            }
        }
        RgbaImage { width: w, height: h, pixels: px }
    }

    #[test]
    fn a_sun_in_the_picture_throws_shadows_towards_the_camera() {
        let l = read_light(&backdrop(Some([0.75, 0.2])));
        let s = l.source.expect("the sun is seen");
        assert!((s[0] - 0.75).abs() < 0.08 && (s[1] - 0.2).abs() < 0.1, "{s:?}");
        assert!(l.side > 0.3 && l.key[0] > 0.8);
        // A character on the left: its shadow falls down and to the left,
        // away from the sun.
        let o = shadow_settings(&l, [0.3, 0.8], 0.45, 16.0 / 9.0);
        let a = o["angle"].as_f64().unwrap();
        assert!((90.0..180.0).contains(&a), "{a}");
        let (from, k) = rim_settings(&l, [0.3, 0.6], 16.0 / 9.0);
        assert!((270.0..360.0).contains(&from) && k > 0.4, "{from} {k}");
    }

    #[test]
    fn light_from_the_side_throws_shadows_back_and_across() {
        let l = read_light(&backdrop(None));
        assert!(l.source.is_none());
        assert!(l.side > 0.15, "{}", l.side);
        let o = shadow_settings(&l, [0.5, 0.8], 0.45, 16.0 / 9.0);
        let a = o["angle"].as_f64().unwrap();
        // Lit from the right: falls to the left and back.
        assert!((180.0..270.0).contains(&a), "{a}");
        assert!(o["colour"].as_array().unwrap().iter().all(|c| c.as_f64().unwrap() < 0.3));
    }

    #[test]
    fn a_bright_even_sky_is_daylight_not_a_sun() {
        let mut img = backdrop(None);
        // White clouds over the top, brightest at the right.
        for y in 0..40usize {
            for x in 0..192usize {
                let k = 200.0 + 40.0 * x as f32 / 192.0;
                let i = (y * 192 + x) * 4;
                img.pixels[i..i + 3].copy_from_slice(&[k as u8, k as u8, (k + 10.0).min(255.0) as u8]);
            }
        }
        assert!(read_light(&img).source.is_none());
    }

    #[test]
    fn shade_darkens_and_tints_but_keeps_the_character() {
        let have = ColourStats { mean: [0.75, 0.0, 0.02], std: [0.12, 0.02, 0.03] };
        let shade = ColourStats { mean: [0.35, -0.04, 0.03], std: [0.08, 0.02, 0.02] };
        let sun = ColourStats { mean: [0.7, 0.0, 0.04], std: [0.2, 0.02, 0.02] };
        let a = lit_like(&have, &shade, 1.0);
        let b = lit_like(&have, &sun, 1.0);
        assert!(a.mean[0] < have.mean[0] - 0.1 && (b.mean[0] - have.mean[0]).abs() < 0.06);
        assert!(a.mean[1] < have.mean[1], "greener in a green shade");
        assert!(a.std[0] < have.std[0] && a.std[0] > have.std[0] * 0.8);
        let none = lit_like(&have, &shade, 0.0);
        assert_eq!(none.mean, have.mean);
    }

    #[test]
    fn feet_are_the_middle_of_the_lowest_solid_row() {
        let mut f = studio_render::frame::Frame::new(40, 80);
        // A body, and two feet at x 10..14 and 24..28 down to row 71.
        for y in 10..60 {
            for x in 8..32 {
                f.pixels[y * 40 + x] = [1.0; 4];
            }
        }
        for y in 60..72 {
            for x in (10..14).chain(24..28) {
                f.pixels[y * 40 + x] = [1.0; 4];
            }
        }
        // A stray bit of a neighbour on the sheet, below a gap.
        for y in 75..80 {
            for x in 30..38 {
                f.pixels[y * 40 + x] = [1.0; 4];
            }
        }
        let ft = feet_of(&f);
        assert!((ft[0] - 0.475).abs() < 0.03, "{ft:?}");
        assert!((ft[1] - 72.0 / 80.0).abs() < 0.02, "{ft:?}");
    }
}
