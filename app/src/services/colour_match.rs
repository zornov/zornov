// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shot matching: one clip's colour and light moved towards another's, so
//! shots from different cameras, times of day or white balances cut
//! together. The statistics of each shot (mean and spread per Oklab
//! channel, over a few frames through it) go into the clip's grade
//! (`match_*` parameters) and the grade moves the clip's own footage from
//! its statistics to the reference's look (`studio_colour::ColourMatch`,
//! the same on the CPU and the GPU), before the rest of the clip's grade.

use studio_colour::auto::{colour_stats, ColourStats};
use studio_colour::{Image, Primaries};
use studio_model as m;

use crate::view::{self, SEC};

/// Frames sampled through a shot.
const SAMPLES: usize = 5;
/// Width the frames are measured at.
const WIDTH: u32 = 320;

/// A picture in the working space, small, from a decoded picture.
fn small(pic: &studio_render::yuv::DecodedPicture) -> studio_render::frame::Frame {
    let f = pic.to_frame(true);
    let k = (f.width / WIDTH).max(1);
    let (w, h) = (f.width / k, f.height / k);
    let mut px = Vec::with_capacity((w * h) as usize);
    for y in 0..h {
        for x in 0..w {
            px.push(f.pixels[((y * k) * f.width + x * k) as usize]);
        }
    }
    studio_render::frame::Frame { width: w, height: h, pixels: px }
}

/// The source frames a clip shows at a few moments, small, with each one's
/// moment (clip time).
fn frames(proj: &view::Project, mc: &m::Clip) -> Result<Vec<(studio_render::frame::Frame, m::Micros)>, String> {
    let m::ClipSource::Media(r) = &mc.source else { return Err("only footage and pictures can be matched (not titles or shapes)".into()) };
    let mi = proj.media(r.media.0 as u32).ok_or("the clip's media is gone")?;
    if matches!(mi.kind, view::MediaKind::Audio) {
        return Err("a sound has no colour to match".into());
    }
    let path = mi.path.clone().ok_or("sample media has no file to measure")?;
    let mut src = studio_render::source::open_path(&path)?;
    let mut out = Vec::new();
    for k in 0..SAMPLES {
        let local = mc.duration * (2 * k as i64 + 1) / (2 * SAMPLES as i64);
        let idx = src.frame_index(mc.source_time_at(mc.start + local));
        let mut got = None;
        src.decode_to(idx, &mut |i, pic| {
            if i == idx {
                got = Some(small(&pic));
            }
        });
        if let Some(f) = got {
            out.push((f, local));
        }
    }
    if out.is_empty() {
        return Err("no frame of the clip could be decoded".into());
    }
    Ok(out)
}

fn stack(fs: Vec<studio_render::frame::Frame>) -> Image {
    let w = fs[0].width;
    let mut pixels = Vec::new();
    let mut h = 0;
    for f in fs.into_iter().filter(|f| f.width == w) {
        h += f.height;
        pixels.extend(f.pixels);
    }
    Image { width: w, height: h, pixels }
}

/// A clip's footage as shot (`look: false`) or as it looks with its own
/// grade (`look: true`), as statistics.
pub fn stats(proj: &view::Project, mc: &m::Clip, look: bool) -> Result<ColourStats, String> {
    let fs = frames(proj, mc)?;
    let graded: Vec<studio_render::frame::Frame> = fs
        .into_iter()
        .map(|(f, local)| match (&mc.grade, look) {
            (Some(g), true) if g.enabled => {
                let desc = studio_render::scene::GradeDesc { params: studio_render::model_scene::convert_params(&mc.grade_params_at(local)), lut: g.lut.clone() };
                studio_render::effects::apply_grade(f, &desc, 0)
            }
            _ => f,
        })
        .collect();
    Ok(colour_stats(&stack(graded), &Primaries::BT709))
}

const KEYS: [&str; 4] = ["match_src_mean", "match_src_std", "match_ref_mean", "match_ref_std"];

/// Writes a match into the clip's grade (made if it has none).
pub fn set(mc: &mut m::Clip, source: &ColourStats, reference: &ColourStats, strength: f64) {
    let g = mc.grade.get_or_insert_with(|| m::Grade { enabled: true, params: m::ParamMap::new(), lut: None, extra: m::Extra::new() });
    for (k, v) in KEYS.iter().zip([source.mean, source.std, reference.mean, reference.std]) {
        for (i, x) in v.iter().enumerate() {
            g.params.insert(format!("{k}_{i}"), m::Value::Scalar(*x as f64));
        }
    }
    g.params.insert("match_strength".into(), m::Value::Scalar(strength.clamp(0.0, 1.0)));
}

/// Takes a match out of the clip's grade.
pub fn clear(mc: &mut m::Clip) -> bool {
    let Some(g) = &mut mc.grade else { return false };
    let n = g.params.len();
    g.params.retain(|k, _| !k.starts_with("match_"));
    mc.keyframes.retain(|k, _| !k.starts_with("grade.match_"));
    n != g.params.len()
}

/// The strength of the clip's match, if it has one.
pub fn strength(mc: &m::Clip) -> Option<f64> {
    mc.grade.as_ref()?.params.get("match_strength")?.as_f64()
}

/// How far apart two shots are, in words (for the AI and the toast).
pub fn difference(a: &ColourStats, b: &ColourStats) -> String {
    let dl = (b.mean[0] - a.mean[0]) as f64;
    let (da, db) = ((b.mean[1] - a.mean[1]) as f64, (b.mean[2] - a.mean[2]) as f64);
    let ds = (b.std[0] - a.std[0]) as f64;
    let mut parts = Vec::new();
    if dl.abs() > 0.02 {
        parts.push(format!("{} brighter", if dl > 0.0 { "the reference is" } else { "this shot is" }));
    }
    if db.abs() > 0.008 {
        parts.push(format!("the reference is {}", if db > 0.0 { "warmer (more yellow)" } else { "cooler (more blue)" }));
    }
    if da.abs() > 0.008 {
        parts.push(format!("the reference is {}", if da > 0.0 { "more magenta" } else { "greener" }));
    }
    if ds.abs() > 0.02 {
        parts.push(format!("the reference has {} contrast", if ds > 0.0 { "more" } else { "less" }));
    }
    if parts.is_empty() { "the two shots already look alike".into() } else { parts.join("; ") }
}

/// Seconds, for messages.
pub fn secs(t: m::Micros) -> f64 {
    t as f64 / SEC as f64
}
