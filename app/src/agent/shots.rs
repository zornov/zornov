// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shots a cinematographer names, for agents: "slow push in on Pip",
//! "low angle close-up on Bo", "orbit the group". The `shot` op frames
//! its subjects at a shot size (extreme wide to extreme close-up, two
//! shot, over the shoulder, insert, POV), from an angle, on a lens, then
//! moves the camera; the eyes land on the upper third line and a figure
//! looking one way gets its lead room on that side. A 2.5D camera on
//! stills must never show past the pictures: backdrops grow to cover, and
//! a move that would still open the empty canvas is cut back until it
//! does not.
//!
//! Also here: where figures sit on screen (for `see`, `frame` and the
//! framing check), previs (stand-ins and a camera per shot from a shot
//! list), the animatic sheet, and camera options compared side by side.

use super::guides::{self, Subject};
use super::space::{add, len, mul, r1, sub};
use super::{Content, Session, f, micros, secs};
use crate::view::{self, SEC};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use studio_model as m;
use studio_model::space as sp;

/// A picture's silhouette, from its transparency: its box (fractions of
/// the picture: left, top, right, bottom), the head's x (the top band's
/// middle), the middle of all of it, and whether it has transparency.
#[derive(Clone, Copy, Debug)]
pub struct Silhouette {
    pub bbox: [f64; 4],
    pub head_u: f64,
    pub centre_u: f64,
    pub alpha: bool,
}

static SILHOUETTES: Mutex<Option<HashMap<PathBuf, Option<Silhouette>>>> = Mutex::new(None);

/// The silhouette of a PNG (others are opaque rectangles: `None`).
pub fn silhouette(path: &Path) -> Option<Silhouette> {
    if !path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("png")) {
        return None;
    }
    if let Some(s) = SILHOUETTES.lock().ok().and_then(|m| m.as_ref().and_then(|m| m.get(path).copied())) {
        return s;
    }
    let s = read_silhouette(path);
    if let Ok(mut m) = SILHOUETTES.lock() {
        m.get_or_insert_with(HashMap::new).insert(path.to_path_buf(), s);
    }
    s
}

/// Whether a picture file has see-through parts.
pub fn has_alpha(path: &Path) -> bool {
    silhouette(path).is_some_and(|s| s.alpha)
}

fn read_silhouette(path: &Path) -> Option<Silhouette> {
    let mut d = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).ok()?));
    d.set_transformations(png::Transformations::normalize_to_color8());
    let mut r = d.read_info().ok()?;
    let mut buf = vec![0; r.output_buffer_size()?];
    let info = r.next_frame(&mut buf).ok()?;
    let (w, h) = (info.width as usize, info.height as usize);
    let chans = match info.color_type {
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Rgba => 4,
        _ => return Some(Silhouette { bbox: [0.0, 0.0, 1.0, 1.0], head_u: 0.5, centre_u: 0.5, alpha: false }),
    };
    let step = (w.max(h) / 300).max(1);
    let a = |x: usize, y: usize| buf[(y * w + x) * chans + chans - 1];
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
    let mut see_through = false;
    let (mut sx, mut n) = (0.0f64, 0.0f64);
    for y in (0..h).step_by(step) {
        for x in (0..w).step_by(step) {
            let v = a(x, y);
            if v < 250 {
                see_through = true;
            }
            if v >= 128 {
                (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
                sx += x as f64;
                n += 1.0;
            }
        }
    }
    if n == 0.0 {
        return Some(Silhouette { bbox: [0.0, 0.0, 1.0, 1.0], head_u: 0.5, centre_u: 0.5, alpha: true });
    }
    let band = y0 + (y1 - y0) / 4;
    let (mut hx, mut hn) = (0.0f64, 0.0f64);
    for y in (y0..=band).step_by(step) {
        for x in (x0..=x1).step_by(step) {
            if a(x, y) >= 128 {
                hx += x as f64;
                hn += 1.0;
            }
        }
    }
    let (wf, hf) = (w as f64, h as f64);
    Some(Silhouette {
        bbox: [x0 as f64 / wf, y0 as f64 / hf, (x1 + step).min(w) as f64 / wf, (y1 + step).min(h) as f64 / hf],
        head_u: if hn > 0.0 { hx / hn / wf } else { (x0 + x1) as f64 / 2.0 / wf },
        centre_u: sx / n / wf,
        alpha: see_through,
    })
}

/// A subject in the world: where its head, eyes and feet are.
#[derive(Clone, Debug)]
pub struct Body {
    pub id: u32,
    pub name: String,
    pub top: [f64; 3],
    pub bottom: [f64; 3],
    pub eyes: [f64; 3],
    pub left: f64,
    pub right: f64,
    pub height: f64,
    pub facing: i8,
    /// A flat picture (a card in space), not a model with volume.
    pub flat: bool,
}

/// Shot sizes: frame height in subject heights, whether the eyes go on
/// the upper third (else the figure is centred), and the usual lens.
struct Size {
    name: &'static str,
    words: &'static str,
    frame: f64,
    eyes: bool,
    mm: f64,
}

const SIZES: [Size; 11] = [
    Size { name: "extreme_wide", words: "extreme wide shot", frame: 5.0, eyes: false, mm: 24.0 },
    Size { name: "wide", words: "wide shot", frame: 1.3, eyes: false, mm: 28.0 },
    Size { name: "medium_wide", words: "medium wide (cowboy) shot", frame: 0.78, eyes: true, mm: 32.0 },
    Size { name: "medium", words: "medium shot", frame: 0.6, eyes: true, mm: 40.0 },
    Size { name: "medium_close_up", words: "medium close-up", frame: 0.42, eyes: true, mm: 50.0 },
    Size { name: "close_up", words: "close-up", frame: 0.28, eyes: true, mm: 65.0 },
    Size { name: "extreme_close_up", words: "extreme close-up", frame: 0.12, eyes: true, mm: 100.0 },
    Size { name: "two_shot", words: "two shot", frame: 0.75, eyes: true, mm: 35.0 },
    Size { name: "over_the_shoulder", words: "over-the-shoulder", frame: 0.42, eyes: true, mm: 50.0 },
    Size { name: "insert", words: "insert", frame: 0.18, eyes: false, mm: 85.0 },
    Size { name: "pov", words: "point of view", frame: 0.6, eyes: true, mm: 32.0 },
];

fn size(name: &str) -> Option<&'static Size> {
    let k = name.trim().to_lowercase().replace(['-', ' ', '.'], "_");
    let k = match k.as_str() {
        "establishing" | "est" | "ews" | "els" | "extreme_long" | "extreme_wide_shot" | "master" => "extreme_wide",
        "ws" | "ls" | "long" | "full" | "fs" | "wide_shot" | "long_shot" | "full_shot" => "wide",
        "mws" | "mls" | "cowboy" | "medium_long" | "medium_wide_shot" | "american" => "medium_wide",
        "ms" | "mid" | "medium_shot" | "mid_shot" | "waist" => "medium",
        "mcu" | "medium_closeup" => "medium_close_up",
        "cu" | "close" | "closeup" | "close_up_shot" | "reaction" => "close_up",
        "ecu" | "xcu" | "extreme_closeup" => "extreme_close_up",
        "2s" | "two" | "2_shot" | "twoshot" | "group" => "two_shot",
        "ots" | "over_shoulder" | "o_s" => "over_the_shoulder",
        "ins" | "detail" | "cutaway" => "insert",
        "point_of_view" => "pov",
        other => other,
    };
    SIZES.iter().find(|s| s.name == k)
}

/// Words that name each thing, longest first so "extreme close-up" wins
/// over "close-up".
const SIZE_WORDS: [(&str, &str); 30] = [
    ("extreme close up", "extreme_close_up"),
    ("extreme close-up", "extreme_close_up"),
    ("extreme closeup", "extreme_close_up"),
    ("medium close up", "medium_close_up"),
    ("medium close-up", "medium_close_up"),
    ("over the shoulder", "over_the_shoulder"),
    ("over-the-shoulder", "over_the_shoulder"),
    ("extreme wide", "extreme_wide"),
    ("extreme long", "extreme_wide"),
    ("establishing", "extreme_wide"),
    ("medium wide", "medium_wide"),
    ("medium long", "medium_wide"),
    ("point of view", "pov"),
    ("close up", "close_up"),
    ("close-up", "close_up"),
    ("closeup", "close_up"),
    ("two shot", "two_shot"),
    ("two-shot", "two_shot"),
    ("full shot", "wide"),
    ("long shot", "wide"),
    ("full body", "wide"),
    ("wide shot", "wide"),
    ("cowboy", "medium_wide"),
    ("medium", "medium"),
    ("mid shot", "medium"),
    ("insert", "insert"),
    ("detail", "insert"),
    ("wide", "wide"),
    ("ecu", "extreme_close_up"),
    ("pov", "pov"),
];

/// Camera heights: elevation (degrees; positive looks down).
fn angle(word: &str) -> Option<(f64, &'static str)> {
    Some(match word {
        "eye_level" | "eye level" | "level" => (0.0, "eye level"),
        "low" | "low_angle" | "low angle" | "from below" => (-15.0, "low angle"),
        "worm" | "worms_eye" | "worm's eye" | "worms eye" => (-40.0, "worm's-eye view"),
        "high" | "high_angle" | "high angle" | "from above" => (22.0, "high angle"),
        "birds_eye" | "bird's eye" | "birds eye" | "top down" | "top_down" | "overhead" => (80.0, "bird's-eye view"),
        _ => return None,
    })
}

const ANGLE_WORDS: [&str; 12] = ["worm's eye", "worms eye", "bird's eye", "birds eye", "top down", "overhead", "low angle", "from below", "high angle", "from above", "eye level", "dutch"];

/// One camera move named in words.
struct Named {
    kind: &'static str,
    sign: f64,
    extra: &'static str,
}

const MOVE_WORDS: [(&str, Named); 46] = [
    ("dolly zoom", Named { kind: "dolly_zoom", sign: 1.0, extra: "" }),
    ("vertigo", Named { kind: "dolly_zoom", sign: 1.0, extra: "" }),
    ("zolly", Named { kind: "dolly_zoom", sign: 1.0, extra: "" }),
    ("rack focus", Named { kind: "rack_focus", sign: 1.0, extra: "" }),
    ("pull focus", Named { kind: "rack_focus", sign: 1.0, extra: "" }),
    ("shift focus", Named { kind: "rack_focus", sign: 1.0, extra: "" }),
    ("follow focus", Named { kind: "follow_focus", sign: 1.0, extra: "" }),
    ("push in", Named { kind: "push_in", sign: 1.0, extra: "" }),
    ("push-in", Named { kind: "push_in", sign: 1.0, extra: "" }),
    ("dolly in", Named { kind: "push_in", sign: 1.0, extra: "" }),
    ("move in", Named { kind: "push_in", sign: 1.0, extra: "" }),
    ("creep in", Named { kind: "push_in", sign: 1.0, extra: "" }),
    ("pull out", Named { kind: "pull_out", sign: 1.0, extra: "" }),
    ("pull back", Named { kind: "pull_out", sign: 1.0, extra: "" }),
    ("dolly out", Named { kind: "pull_out", sign: 1.0, extra: "" }),
    ("pull away", Named { kind: "pull_out", sign: 1.0, extra: "" }),
    ("orbit", Named { kind: "orbit", sign: 1.0, extra: "" }),
    ("circle", Named { kind: "orbit", sign: 1.0, extra: "" }),
    ("arc around", Named { kind: "orbit", sign: 1.0, extra: "" }),
    ("go around", Named { kind: "orbit", sign: 1.0, extra: "" }),
    ("truck left", Named { kind: "truck", sign: -1.0, extra: "" }),
    ("track left", Named { kind: "truck", sign: -1.0, extra: "" }),
    ("crab left", Named { kind: "truck", sign: -1.0, extra: "" }),
    ("truck right", Named { kind: "truck", sign: 1.0, extra: "" }),
    ("track right", Named { kind: "truck", sign: 1.0, extra: "" }),
    ("crab right", Named { kind: "truck", sign: 1.0, extra: "" }),
    ("crane down", Named { kind: "crane", sign: -1.0, extra: "" }),
    ("boom down", Named { kind: "crane", sign: -1.0, extra: "" }),
    ("crane up", Named { kind: "crane", sign: 1.0, extra: "" }),
    ("boom up", Named { kind: "crane", sign: 1.0, extra: "" }),
    ("jib up", Named { kind: "crane", sign: 1.0, extra: "" }),
    ("crane", Named { kind: "crane", sign: 1.0, extra: "" }),
    ("tilt up", Named { kind: "tilt", sign: 1.0, extra: "" }),
    ("tilt down", Named { kind: "tilt", sign: -1.0, extra: "" }),
    ("pan left", Named { kind: "pan", sign: -1.0, extra: "" }),
    ("pan right", Named { kind: "pan", sign: 1.0, extra: "" }),
    ("crash zoom", Named { kind: "zoom", sign: 1.0, extra: "crash" }),
    ("zoom in", Named { kind: "zoom", sign: 1.0, extra: "" }),
    ("zoom out", Named { kind: "zoom", sign: -1.0, extra: "" }),
    ("steadicam", Named { kind: "steadicam", sign: 1.0, extra: "" }),
    ("walk with", Named { kind: "steadicam", sign: 1.0, extra: "" }),
    ("handheld", Named { kind: "handheld", sign: 1.0, extra: "" }),
    ("hand-held", Named { kind: "handheld", sign: 1.0, extra: "" }),
    ("drone", Named { kind: "drone", sign: 1.0, extra: "" }),
    ("follow", Named { kind: "follow", sign: 1.0, extra: "" }),
    ("tracking", Named { kind: "follow", sign: 1.0, extra: "" }),
];

/// Names in a shot's words: capitalised words that are not shot words
/// ("wide on Ana and Ben" names Ana and Ben).
fn names_in(say: &str) -> Vec<String> {
    const SKIP: [&str; 24] = ["on", "of", "the", "a", "an", "and", "to", "at", "with", "from", "in", "slow", "slowly", "fast", "quick", "very", "shot", "angle", "lens", "focus", "group", "everyone", "both", "them"];
    let vocab = |w: &str| SKIP.contains(&w) || SIZE_WORDS.iter().any(|(v, _)| v.split([' ', '-']).any(|x| x == w)) || MOVE_WORDS.iter().any(|(v, _)| v.split([' ', '-']).any(|x| x == w)) || ANGLE_WORDS.iter().any(|v| v.split([' ', '\'']).any(|x| x == w));
    let mut out: Vec<String> = Vec::new();
    for w in say.split(|c: char| !(c.is_alphanumeric() || c == '-' || c == '\'')) {
        let w = w.trim_matches(['\'', '-']);
        if w.chars().next().is_some_and(char::is_uppercase) && !vocab(&w.to_lowercase()) && !out.iter().any(|o| o == w) {
            out.push(w.to_owned());
        }
    }
    out
}

/// A shot as asked for.
#[derive(Default)]
struct Spec {
    size: Option<&'static Size>,
    subjects: Vec<u32>,
    elevation: f64,
    angle: &'static str,
    yaw: f64,
    dutch: f64,
    moves: Vec<(String, f64, String)>,
    speed: f64,
    mm: Option<f64>,
    shallow: bool,
    focus_to: Option<u32>,
    drone_style: Option<&'static str>,
    profile: Option<&'static str>,
}

/// Whole-word containment (names, "push in").
fn has(text: &str, w: &str) -> Option<usize> {
    let mut from = 0;
    while let Some(i) = text[from..].find(w) {
        let at = from + i;
        let before = text[..at].chars().next_back();
        let after = text[at + w.len()..].chars().next();
        if before.is_none_or(|c| !c.is_alphanumeric()) && after.is_none_or(|c| !c.is_alphanumeric()) {
            return Some(at);
        }
        from = at + w.len().max(1);
        if from >= text.len() {
            break;
        }
    }
    None
}

impl Session {
    /// Which way a figure looks on screen at `t`: its pins (head ahead of
    /// body), its silhouette's lean, else the way it moves.
    fn facing_on_screen(&self, mc: &m::Clip, t: view::Time, sil: Option<&Silhouette>) -> i8 {
        let local = (t - mc.start).clamp(0, mc.duration);
        let flip = mc.transform_at(local).flip_x;
        let flipped = |right: bool| if right != flip { 1 } else { -1 };
        if let Some(pp) = mc.puppet.as_ref().filter(|p| p.enabled) {
            let named = |want: &[&str]| pp.pins.iter().find(|p| want.iter().any(|w| p.name.to_lowercase().contains(w))).map(|p| p.rest.x);
            if let (Some(head), Some(body)) = (named(&["head", "beak", "face"]), named(&["body", "chest", "back", "hips", "torso"])) {
                if (head - body).abs() > 0.03 {
                    return flipped(head > body);
                }
            }
        }
        if let Some(s) = sil {
            let lean = (s.head_u - s.centre_u) / (s.bbox[2] - s.bbox[0]).max(1e-3);
            if lean.abs() > 0.05 {
                return flipped(lean > 0.0);
            }
        }
        // Moving: it faces where it goes.
        let seq = self.doc.model.sequence(self.doc.seq());
        if let Some(seq) = seq {
            let at = |t: view::Time| seq.placed_transform(mc, t.clamp(mc.start, mc.end() - 1)).position.x;
            let d = at(t + SEC / 3) - at(t - SEC / 3);
            if d.abs() > 0.02 * self.doc.project.width as f64 {
                return if d > 0.0 { 1 } else { -1 };
            }
        }
        0
    }

    fn silhouette_of(&self, mc: &m::Clip) -> Option<Silhouette> {
        let m::ClipSource::Media(r) = &mc.source else { return None };
        silhouette(&self.doc.project.media(r.media.0 as u32)?.path.clone()?)
    }

    /// Whether clip `id` is a figure at `t`: a picture above the main lane
    /// that is see-through (a cut-out character or prop), a model, or one
    /// that does not fill the frame. However close the camera comes, a
    /// cut-out stays a figure.
    pub(super) fn is_figure(&self, c: &view::Clip, t: view::Time) -> bool {
        let p = &self.doc.project;
        if c.kind != view::ClipKind::Video || t < c.start || t >= c.start + c.dur || Some(c.lane) == p.spine() || p.lane(c.lane).is_none_or(|l| l.hidden) {
            return false;
        }
        let Some(mc) = self.model_clip(c.id) else { return false };
        if mc.is_camera_or_light() || matches!(&mc.source, m::ClipSource::Generator(g) if g.kind == m::GeneratorKind::Adjustment) || !mc.enabled {
            return false;
        }
        if super::space::rig_kind(&mc) == Some("model") || self.silhouette_of(&mc).is_some_and(|s| s.alpha) {
            return true;
        }
        !self.covers_much(&mc, t)
    }

    /// Figures on screen at `t` (characters and props, not the backdrop):
    /// their silhouette's box and eyes as fractions of the frame.
    pub(super) fn subjects_at(&self, t: view::Time) -> Vec<Subject> {
        let p = &self.doc.project;
        let (w, h) = (p.width as f32, p.height as f32);
        let mut out = Vec::new();
        for c in p.clips.iter().filter(|c| self.is_figure(c, t)) {
            let Some(mc) = self.model_clip(c.id) else { continue };
            let Some(map) = crate::editor::pins::LayerMap::at(p, &self.doc.model, c.id, t) else { continue };
            let sil = self.silhouette_of(&mc);
            let s = sil.unwrap_or(Silhouette { bbox: [0.0, 0.0, 1.0, 1.0], head_u: 0.5, centre_u: 0.5, alpha: false });
            let cr = &mc.transform.crop;
            let (kw, kh) = ((1.0 - cr.left - cr.right).max(1e-3), (1.0 - cr.top - cr.bottom).max(1e-3));
            let to = |u: f64, v: f64| {
                let q = map.canvas(m::Vec2::new((u - cr.left) / kw, (v - cr.top) / kh));
                [(q.x + w / 2.0) / w, (q.y + h / 2.0) / h]
            };
            let b = s.bbox;
            let corners = [to(b[0], b[1]), to(b[2], b[1]), to(b[2], b[3]), to(b[0], b[3])];
            let bbox = [
                corners.iter().map(|c| c[0]).fold(f32::MAX, f32::min),
                corners.iter().map(|c| c[1]).fold(f32::MAX, f32::min),
                corners.iter().map(|c| c[0]).fold(f32::MIN, f32::max),
                corners.iter().map(|c| c[1]).fold(f32::MIN, f32::max),
            ];
            let eyes = to(s.head_u, b[1] + 0.12 * (b[3] - b[1]));
            out.push(Subject { id: c.id, name: c.name.clone(), bbox, eyes, facing: self.facing_on_screen(&mc, t, sil.as_ref()) });
        }
        out
    }

    /// Where the figures sit against `guides` at `t`, a line each.
    pub(super) fn framing_words(&self, t: view::Time, g: &[guides::Guide]) -> Vec<String> {
        let p = &self.doc.project;
        guides::report(&self.subjects_at(t), g, p.width as f32, p.height as f32)
    }

    /// `check`'s framing problems: each figure judged at a few moments of
    /// its time on screen, in the viewer's aspect guide when one is on.
    pub(super) fn framing_problems(&self) -> Vec<Value> {
        let p = &self.doc.project;
        let frame = guides::frame_of(&guides::viewer(), p.width as f32, p.height as f32);
        let mut seen: HashMap<(u32, String), Value> = HashMap::new();
        let figs: Vec<&view::Clip> = p.clips.iter().filter(|c| [1, 2, 3].iter().any(|k| self.is_figure(c, c.start + c.dur * k / 4))).collect();
        for c in &figs {
            for k in [1, 2, 3] {
                let t = c.start + c.dur * k / 4;
                let subs: Vec<Subject> = self.subjects_at(t).into_iter().filter(|s| s.id == c.id).collect();
                for mut v in guides::problems(&subs, frame) {
                    let kind = v["kind"].as_str().unwrap_or("").to_owned();
                    v["at"] = json!((secs(t) * 100.0).round() / 100.0);
                    seen.entry((c.id, kind)).or_insert(v);
                }
            }
        }
        // Figures touching each other: at the middle of each overlap.
        for (i, a) in figs.iter().enumerate() {
            for b in &figs[i + 1..] {
                let (s, e) = (a.start.max(b.start), (a.start + a.dur).min(b.start + b.dur));
                if e <= s {
                    continue;
                }
                let t = s + (e - s) / 2;
                let subs: Vec<Subject> = self.subjects_at(t).into_iter().filter(|x| x.id == a.id || x.id == b.id).collect();
                for mut v in guides::problems(&subs, frame).into_iter().filter(|v| v["problem"].as_str().is_some_and(|p| p.starts_with("clips "))) {
                    v["at"] = json!((secs(t) * 100.0).round() / 100.0);
                    seen.entry((b.id, format!("touch{}", a.id))).or_insert(v);
                }
            }
        }
        let mut out: Vec<Value> = seen.into_values().collect();
        out.sort_by(|a, b| a["at"].as_f64().unwrap_or(0.0).total_cmp(&b["at"].as_f64().unwrap_or(0.0)));
        out.truncate(12);
        out
    }

    /// A subject in the world at `t`.
    pub(super) fn body(&self, id: u32, t: view::Time) -> Result<Body, String> {
        let mc = self.model_clip(id).ok_or_else(|| format!("no clip {id}"))?;
        let vc = self.clip(id)?;
        let t = t.clamp(mc.start, mc.end() - 1);
        let seq = self.doc.model.sequence(self.doc.seq()).ok_or("no sequence")?;
        if super::space::rig_kind(&mc) == Some("model") {
            let (_, mesh) = self.model_of(id)?;
            let (lo, hi) = mesh.bounds();
            let tr = seq.placed_transform(&mc, t);
            let s = [tr.scale_x, tr.scale_y, tr.scale_z];
            let w = |p: [f32; 3]| [tr.position.x + p[0] as f64 * s[0], tr.position.y + p[1] as f64 * s[1], tr.position_z + p[2] as f64 * s[2]];
            let (a, b) = (w(lo), w(hi));
            let (cx, cz) = ((a[0] + b[0]) / 2.0, (a[2] + b[2]) / 2.0);
            let height = (b[1] - a[1]).abs();
            return Ok(Body { id, name: vc.name.clone(), top: [cx, a[1], cz], bottom: [cx, b[1], cz], eyes: [cx, a[1] + 0.12 * height, a[2].min(b[2])], left: a[0].min(b[0]), right: a[0].max(b[0]), height, facing: 0, flat: false });
        }
        if mc.is_camera_or_light() {
            return Err(format!("clip {id} is a camera or light, not something to frame"));
        }
        let Some((plane, source)) = self.plane_of(&mc, t) else {
            return Err(format!("clip {id} (\"{}\") is flat (2D): a camera only sees 3D layers; put it in space first (three_d)", vc.name));
        };
        let sil = self.silhouette_of(&mc);
        let s = sil.unwrap_or(Silhouette { bbox: [0.0, 0.0, 1.0, 1.0], head_u: 0.5, centre_u: 0.5, alpha: false });
        let at = |u: f64, v: f64| plane.at(u * source.x, v * source.y);
        let b = s.bbox;
        let (top, bottom) = (at(s.head_u, b[1]), at((b[0] + b[2]) / 2.0, b[3]));
        let (l, r) = (at(b[0], (b[1] + b[3]) / 2.0), at(b[2], (b[1] + b[3]) / 2.0));
        Ok(Body {
            id,
            name: vc.name.clone(),
            top,
            bottom,
            eyes: at(s.head_u, b[1] + 0.12 * (b[3] - b[1])),
            left: l[0].min(r[0]),
            right: l[0].max(r[0]),
            height: len(sub(bottom, top)),
            facing: self.facing_on_screen(&mc, t, sil.as_ref()),
            flat: true,
        })
    }

    /// Clips a shot can name: 3D layers and models, by name.
    fn nameable(&self) -> Vec<(u32, String)> {
        let Some(seq) = self.doc.model.sequence(self.doc.seq()) else { return Vec::new() };
        seq.clips
            .iter()
            .filter(|c| (c.transform.three_d || super::space::rig_kind(c) == Some("model")) && !c.is_camera_or_light() && !c.name.trim().is_empty())
            .map(|c| (c.id.0 as u32, c.name.to_lowercase()))
            .collect()
    }

    /// The `shot` op's fields and words, read.
    fn read_spec(&self, op: &Value) -> Result<Spec, String> {
        let mut s = Spec { speed: 1.0, angle: "eye level", ..Default::default() };
        let say = op.get("say").or(op.get("describe")).and_then(Value::as_str).unwrap_or("").to_lowercase();
        // Subjects: by id, by name, or named in the words.
        let named = self.nameable();
        let id_of = |v: &Value| -> Result<u32, String> {
            if let Some(id) = v.as_u64().or_else(|| v.get("clip").and_then(Value::as_u64)) {
                return Ok(id as u32);
            }
            let n = v.as_str().ok_or("subjects are clips: ids or names")?.trim().to_lowercase();
            named.iter().find(|(_, x)| *x == n).or_else(|| named.iter().find(|(_, x)| x.contains(&n) || has(&n, x).is_some())).map(|(id, _)| *id).ok_or_else(|| format!("no 3D layer or model named \"{n}\" (name it, or switch it into 3D with three_d); there are {}", named.iter().map(|(i, n)| format!("{i} \"{n}\"")).collect::<Vec<_>>().join(", ")))
        };
        match op.get("on").or(op.get("subject")).or(op.get("subjects")) {
            Some(Value::Array(a)) => {
                for v in a {
                    s.subjects.push(id_of(v)?);
                }
            }
            Some(v) => s.subjects.push(id_of(v)?),
            None => {}
        }
        // "rack focus to Bo": the name after the focus words is the focus
        // target, not a subject to frame.
        let focus_at = ["rack focus to ", "pull focus to ", "shift focus to ", "focus to ", "rack to "].iter().find_map(|w| say.find(w).map(|i| i + w.len()));
        if !say.is_empty() {
            let group = ["the group", "everyone", "everybody", "all of them", "them all", "the two", "both of them", "the pair", "the family", "the crowd"].iter().any(|w| has(&say, w).is_some());
            let mut found: Vec<(usize, u32)> = named.iter().filter_map(|(id, n)| has(&say, n).map(|at| (at, *id))).collect();
            found.sort();
            for (at, id) in found {
                if focus_at.is_some_and(|f| at >= f && at < f + 40) {
                    s.focus_to.get_or_insert(id);
                } else if !s.subjects.contains(&id) {
                    s.subjects.push(id);
                }
            }
            if group && s.subjects.len() < 2 {
                let t = micros(f(op, "at").unwrap_or(0.0));
                for c in self.doc.project.clips.iter().filter(|c| self.is_figure(c, t)) {
                    if !s.subjects.contains(&c.id) && self.body(c.id, t).is_ok() {
                        s.subjects.push(c.id);
                    }
                }
            }
            if let Some((_, k)) = SIZE_WORDS.iter().find(|(w, _)| has(&say, w).is_some()) {
                s.size = size(k);
            }
            for w in ANGLE_WORDS {
                if has(&say, w).is_some() {
                    if w == "dutch" {
                        s.dutch = 12.0;
                    } else if let Some((e, name)) = angle(w) {
                        (s.elevation, s.angle) = (e, name);
                    }
                }
            }
            if ["canted", "tilted frame", "dutch angle"].iter().any(|w| has(&say, w).is_some()) {
                s.dutch = 12.0;
            }
            if has(&say, "three quarter").is_some() || has(&say, "3/4").is_some() {
                s.yaw = 25.0;
            }
            if has(&say, "profile").is_some() {
                s.yaw = 60.0;
            }
            if has(&say, "from the left").is_some() {
                s.yaw = -s.yaw.abs().max(20.0);
            } else if has(&say, "from the right").is_some() {
                s.yaw = s.yaw.abs().max(20.0);
            }
            s.speed = if ["very slow", "glacial", "imperceptible"].iter().any(|w| has(&say, w).is_some()) {
                0.45
            } else if ["slow", "slowly", "gentle", "gently", "subtle", "creeping", "lazy"].iter().any(|w| has(&say, w).is_some()) {
                0.65
            } else if ["quick", "fast", "quickly", "snap", "whip", "sudden", "rapid"].iter().any(|w| has(&say, w).is_some()) {
                1.6
            } else {
                1.0
            };
            let mut used: Vec<(usize, usize)> = Vec::new();
            for (w, n) in &MOVE_WORDS {
                if let Some(at) = has(&say, w) {
                    if used.iter().any(|(a, b)| at < *b && at + w.len() > *a) {
                        continue;
                    }
                    used.push((at, at + w.len()));
                    s.moves.push((n.kind.to_owned(), n.sign, n.extra.to_owned()));
                }
            }
            for (w, k, sign) in [("pan", "pan", 1.0), ("tilt", "tilt", 1.0), ("zoom", "zoom", 1.0), ("reveal", "reveal", 1.0), ("rise", "crane", 1.0), ("aerial", "drone", 1.0), ("flyover", "drone", 1.0), ("fly over", "drone", 1.0)] {
                if has(&say, w).is_some_and(|at| !used.iter().any(|(a, b)| at >= *a && at < *b)) && !s.moves.iter().any(|m| m.0 == k) {
                    s.moves.push((k.to_owned(), sign, String::new()));
                }
            }
            if ["flyover", "fly over"].iter().any(|w| has(&say, w).is_some()) {
                s.drone_style = Some("flyover");
            } else if has(&say, "reveal").is_some() && s.moves.iter().any(|m| m.0 == "drone") {
                s.drone_style = Some("reveal");
            }
            for (w, p) in [("nervous", "nervous"), ("frantic", "nervous"), ("anxious", "nervous"), ("running", "running"), ("chase", "running"), ("documentary", "documentary"), ("shoulder", "shoulder"), ("calm", "calm"), ("breathing", "calm")] {
                if has(&say, w).is_some() {
                    s.profile = Some(p);
                    if !s.moves.iter().any(|m| m.0 == "handheld") {
                        s.moves.push(("handheld".into(), 1.0, String::new()));
                    }
                }
            }
            if ["shallow", "soft background", "bokeh", "shallow focus", "blurred background"].iter().any(|w| has(&say, w).is_some()) {
                s.shallow = true;
            }
            // "on a 85mm", "85 mm lens".
            for (i, _) in say.match_indices("mm") {
                let num: String = say[..i].trim_end().chars().rev().take_while(|c| c.is_ascii_digit() || *c == '.').collect::<Vec<_>>().into_iter().rev().collect();
                if let Ok(v) = num.parse::<f64>() {
                    s.mm = Some(v);
                }
            }
        }
        if let Some(z) = op.get("size").and_then(Value::as_str) {
            s.size = Some(size(z).ok_or_else(|| format!("size \"{z}\": {}", SIZES.iter().map(|s| s.name).collect::<Vec<_>>().join(", ")))?);
        }
        if let Some(a) = op.get("angle").and_then(Value::as_str) {
            if a == "dutch" {
                s.dutch = 12.0;
            } else {
                (s.elevation, s.angle) = angle(&a.to_lowercase().replace('-', " ")).ok_or("angle is eye_level, low, worms_eye, high, birds_eye or dutch")?;
            }
        }
        if let Some(d) = f(op, "dutch") {
            s.dutch = d;
        }
        if let Some(y) = f(op, "yaw") {
            s.yaw = y;
        }
        if let Some(mm) = f(op, "focal_length").or(f(op, "lens")) {
            s.mm = Some(mm);
        }
        if let Some(v) = op.get("shallow").or(op.get("dof")).and_then(Value::as_bool) {
            s.shallow = v;
        }
        if let Some(sp) = op.get("speed") {
            s.speed = match sp.as_str() {
                Some("very_slow" | "very slow") => 0.45,
                Some("slow") => 0.65,
                Some("normal" | "medium") => 1.0,
                Some("fast" | "quick") => 1.6,
                Some(o) => return Err(format!("speed \"{o}\": very_slow, slow, normal, fast (or a number, 1 = normal)")),
                None => sp.as_f64().ok_or("speed is a word or a number")?.clamp(0.2, 3.0),
            };
        }
        match op.get("move") {
            Some(Value::String(mv)) => {
                let low = mv.to_lowercase().replace('_', " ");
                let found = MOVE_WORDS.iter().find(|(w, _)| has(&low, w).is_some()).map(|(_, n)| (n.kind.to_owned(), n.sign, n.extra.to_owned()));
                s.moves = vec![found.unwrap_or((mv.to_lowercase(), 1.0, String::new()))];
                if s.moves[0].0 == "static" || s.moves[0].0 == "locked" || s.moves[0].0 == "none" {
                    s.moves.clear();
                }
            }
            Some(Value::Object(_)) => s.moves = vec![("raw".into(), 1.0, op["move"].to_string())],
            Some(_) => return Err("move is words (\"slow push in\", \"orbit\", \"crane up\") or a camera move object".into()),
            None => {}
        }
        if let Some(p) = op.get("profile").and_then(Value::as_str) {
            s.profile = ["calm", "documentary", "shoulder", "nervous", "running"].into_iter().find(|x| *x == p);
        }
        if let Some(fv) = op.get("focus_to") {
            s.focus_to = Some(id_of(fv)?);
        }
        if s.subjects.is_empty() {
            return Err(format!(
                "a shot needs a subject: \"on\" (a clip id or name) or a name in \"say\"; named 3D layers and models: {}",
                if named.is_empty() { "none (switch layers into 3D with three_d, or add a model)".to_owned() } else { named.iter().map(|(i, n)| format!("{i} \"{n}\"")).collect::<Vec<_>>().join(", ") }
            ));
        }
        if s.size.is_none() {
            s.size = size(if s.subjects.len() > 2 { "wide" } else if s.subjects.len() == 2 { "two_shot" } else { "medium" });
        }
        Ok(s)
    }

    /// `shot`: a camera framing subjects at a size, from an angle, then
    /// moving; one camera clip for the shot's time.
    pub(super) fn shot(&mut self, op: &Value) -> Result<String, String> {
        let spec = self.read_spec(op)?;
        let size = spec.size.expect("a size");
        let (w, h) = (self.width(), self.doc.project.height as f64);
        // When: the given time, else while the subjects are all on.
        let (s0, e0) = spec.subjects.iter().filter_map(|id| self.clip(*id).ok()).fold((0, i64::MAX), |(a, b), c| (a.max(c.start), b.min(c.start + c.dur)));
        let at = f(op, "at").map(micros).unwrap_or(s0);
        let end = f(op, "duration").map(|d| at + micros(d)).unwrap_or(if e0 > at && e0 != i64::MAX { e0 } else { self.doc.project.duration().max(at + 3 * SEC) });
        let dur = (end - at).max(SEC / 4);
        let bodies: Vec<Body> = spec.subjects.iter().map(|id| self.body(*id, at)).collect::<Result<_, _>>()?;
        // The lens: the camera already looking here gives its kit and
        // sensor to the new one.
        let base = self.camera_at(at).and_then(|c| self.model_clip(c));
        let mut params = sp::CameraSettings::defaults(w);
        if let Some(b) = &base {
            if let m::ClipSource::Generator(g) = &b.source {
                for k in ["sensor", "lens_kit", "squeeze", "breathing", "bokeh", "distortion", "fringe"] {
                    if let Some(v) = g.params.get(k) {
                        params.insert(k.into(), v.clone());
                    }
                }
            }
        }
        let lens = super::cine::lens_of(&params);
        let mm = lens.nearest(spec.mm.unwrap_or(size.mm));
        let zoom = lens.zoom(mm, w);
        // The frame round the subjects.
        let top = bodies.iter().map(|b| b.top[1]).fold(f64::MAX, f64::min);
        let bottom = bodies.iter().map(|b| b.bottom[1]).fold(f64::MIN, f64::max);
        let (left, right) = (bodies.iter().map(|b| b.left).fold(f64::MAX, f64::min), bodies.iter().map(|b| b.right).fold(f64::MIN, f64::max));
        let tall = (bottom - top).max(1.0);
        let eyes = { let n = bodies.len() as f64; bodies.iter().fold([0.0; 3], |a, b| add(a, mul(b.eyes, 1.0 / n))) };
        let lead = &bodies[0];
        let pov = size.name == "pov";
        let ots = size.name == "over_the_shoulder" && bodies.len() >= 2;
        let main = if ots { &bodies[0] } else { lead };
        let subject_h = if ots || pov { main.height } else { tall };
        let mut frame_h = size.frame * subject_h;
        if matches!(size.name, "wide" | "extreme_wide" | "two_shot") || bodies.len() > 1 && !ots {
            frame_h = frame_h.max((right - left) * if size.name == "two_shot" { 1.3 } else { 1.2 } * h / w);
        }
        let frame_w = frame_h * w / h;
        let mut aim = if ots { main.eyes } else { [(left + right) / 2.0, eyes[1], eyes[2]] };
        if size.eyes && !pov {
            // Eyes on the upper third.
            aim[1] = if ots { main.eyes[1] } else { eyes[1] } + frame_h / 6.0;
        } else if size.name == "extreme_wide" {
            aim[1] = (top + bottom) / 2.0 - frame_h / 6.0;
        } else if !size.eyes {
            aim[1] = (top + bottom) / 2.0;
        }
        if size.name == "insert" {
            aim = [(main.left + main.right) / 2.0, (main.top[1] + main.bottom[1]) / 2.0, main.eyes[2]];
        }
        // Lead room: one figure looking aside sits on the far third.
        let facing = if bodies.len() == 1 || ots { main.facing } else { 0 };
        if facing != 0 && !matches!(size.name, "extreme_close_up" | "insert") {
            aim[0] = main.eyes[0] + facing as f64 * frame_w / 6.0;
        }
        let depth = frame_h * zoom / h;
        let mut notes = Vec::new();
        let flat = bodies.iter().all(|b| b.flat);
        let mut yaw = spec.yaw;
        let mut elev = spec.elevation;
        if flat && yaw.abs() > 30.0 {
            notes.push(format!("the subjects are flat pictures: the side angle is held to 30° (at {:.0}° they turn to slivers)", yaw));
            yaw = 30.0 * yaw.signum();
        }
        if flat && elev.abs() > 35.0 {
            notes.push(format!("the subjects are flat pictures: the {} is held to 35° (from straight above a card disappears); use 3D models for a true top shot", spec.angle));
            elev = 35.0 * elev.signum();
        }
        let (ye, yy) = (elev.to_radians(), yaw.to_radians());
        let fwd = [yy.sin() * ye.cos(), ye.sin(), yy.cos() * ye.cos()];
        let (mut pos, mut poi) = (sub(aim, mul(fwd, depth)), aim);
        if ots {
            // Behind the near figure's shoulder, looking past it.
            let other = &bodies[1];
            let side = if other.eyes[0] < main.eyes[0] { -1.0 } else { 1.0 };
            let shoulder = [other.eyes[0] + side * 0.12 * other.height, other.eyes[1] + 0.1 * other.height, other.eyes[2] - 0.35 * depth];
            pos = add(shoulder, [side * 0.1 * other.height, 0.0, -0.25 * depth]);
            poi = aim;
            notes.push(format!("over {}'s shoulder (keep it close to the lens and out of focus: the eye goes to {})", other.name, main.name));
        }
        if pov {
            let ahead = if main.facing != 0 { [main.facing as f64, 0.0, 0.25] } else { [0.0, 0.0, 1.0] };
            pos = add(main.eyes, mul(ahead, 0.05 * main.height));
            poi = match bodies.get(1) {
                Some(b) => b.eyes,
                None => add(main.eyes, mul(ahead, depth)),
            };
            if flat {
                notes.push("a point of view from a flat picture looks along the plane: it works over a backdrop, or with 3D models".into());
            }
        }
        // The camera clip.
        params.insert("two_node".into(), m::Value::Bool(true));
        params.insert("zoom".into(), m::Value::Scalar(zoom));
        params.insert("poi".into(), m::Value::Point(m::Vec2::new(poi[0], poi[1])));
        params.insert("poi_z".into(), m::Value::Scalar(poi[2]));
        let focus_d = len(sub(main.eyes, pos)).max(1.0);
        params.insert("focus_distance".into(), m::Value::Scalar(focus_d));
        if spec.shallow || spec.focus_to.is_some() {
            params.insert("dof".into(), m::Value::Bool(true));
            params.insert("aperture".into(), m::Value::Scalar(0.035 * w));
        }
        let title = match op.get("say").and_then(Value::as_str) {
            Some(s) => format!("Shot: {s}"),
            None => format!("Shot: {} on {}", size.words, bodies.iter().map(|b| b.name.clone()).collect::<Vec<_>>().join(" and ")),
        };
        let id = match op.get("clip").and_then(Value::as_u64) {
            Some(c) => {
                let mut mc = self.model_clip(c as u32).ok_or_else(|| format!("no clip {c}"))?;
                if super::space::rig_kind(&mc) != Some("camera") {
                    return Err(format!("clip {c} is not a camera"));
                }
                if let m::ClipSource::Generator(g) = &mut mc.source {
                    g.params.extend(params);
                }
                mc.keyframes.retain(|k, _| !(k.starts_with("transform.") || k.starts_with("generator.")));
                mc.transform.position = m::Vec2::new(pos[0], pos[1]);
                mc.transform.position_z = pos[2];
                mc.transform.rotation = spec.dutch;
                self.apply_clip("Shot", mc)?;
                c as u32
            }
            None => {
                let id = self.add_rig(m::GeneratorKind::Camera, &json!({"at": secs(at), "duration": secs(dur)}), &title, params, pos)?;
                let mut mc = self.model_clip(id).ok_or("camera gone")?;
                mc.transform.rotation = spec.dutch;
                self.apply_clip("Shot", mc)?;
                id
            }
        };
        let vc = self.clip(id)?;
        let (t0, t1) = (vc.start, vc.start + vc.dur);
        // Moves, cut back if one would open the empty canvas.
        let cam_before = self.model_clip(id).ok_or("camera gone")?;
        let backdrops: Vec<m::Clip> = self
            .doc
            .model
            .sequence(self.doc.seq())
            .map(|q| q.clips.iter().filter(|c| c.transform.three_d && c.start < t1 && c.end() > t0 && matches!(c.source, m::ClipSource::Media(_) | m::ClipSource::Generator(_)) && !c.is_camera_or_light() && self.covers_much(c, t0)).cloned().collect())
            .unwrap_or_default();
        let mut said_moves = String::new();
        let mut cut = 1.0;
        for k in [1.0, 0.6, 0.35, 0.0] {
            if k < 1.0 {
                self.apply_clip("Shot", cam_before.clone())?;
                for b in &backdrops {
                    self.apply_clip("Shot", b.clone())?;
                }
            }
            let mvs = self.moves_for(&spec, k, depth, frame_w, frame_h, dur, &bodies)?;
            said_moves = if mvs.is_empty() { String::new() } else { self.camera(&json!({"op": "camera", "clip": id, "moves": mvs}))? };
            // The frame itself may run past a backdrop: grow it to cover.
            for b in &backdrops {
                if let Some(g) = self.cover(b.id.0 as u32, t0, t1).filter(|g| *g > 1.001) {
                    notes.push(format!("backdrop {} scaled up {g:.2}× to cover the frame", b.id.0));
                }
            }
            cut = k;
            if self.gaps(t0, t1).is_none() || spec.moves.is_empty() {
                break;
            }
        }
        if cut < 1.0 && !spec.moves.is_empty() {
            notes.push(if cut > 0.0 { format!("the move is cut to {:.0}% of its size: any further and the empty canvas past the pictures would show", cut * 100.0) } else { "the move is dropped: any move here shows the empty canvas past the pictures (generate a wider backdrop, or frame tighter)".into() });
        }
        if let Some((t, place)) = self.gaps(t0, t1) {
            notes.push(format!("warning: the empty canvas shows at {} at {:.2}s: the backdrop is smaller than this frame (frame tighter, or use a wider backdrop)", place, secs(t)));
        }
        let lens_now = super::cine::lens_of(&super::space::gen_params(&self.model_clip(id).ok_or("camera gone")?));
        let framing = self.framing_words(t0 + (t1 - t0) / 2, &guides::viewer());
        let mut out = format!(
            "{} on {} ({}{}, {}): camera clip {id}, {:.2}–{:.2}s, at [{}, {}, {}] looking at [{}, {}, {}], {} px from {}{}{}",
            size.words,
            bodies.iter().map(|b| format!("#{} {}", b.id, b.name)).collect::<Vec<_>>().join(" and "),
            spec.angle,
            if spec.dutch != 0.0 { ", dutch" } else { "" },
            lens_now.words(zoom, w),
            secs(t0),
            secs(t1),
            r1(pos[0]),
            r1(pos[1]),
            r1(pos[2]),
            r1(poi[0]),
            r1(poi[1]),
            r1(poi[2]),
            r1(depth),
            main.name,
            if spec.shallow { ", shallow focus on the eyes" } else { "" },
            if said_moves.is_empty() { String::new() } else { format!("; {said_moves}") }
        );
        if !notes.is_empty() {
            out.push_str(&format!("; {}", notes.join("; ")));
        }
        if !framing.is_empty() {
            out.push_str(&format!("; framing at the middle: {}", framing.join(" | ")));
        }
        Ok(out)
    }

    /// Whether a 3D picture fills much of the frame at `t` (a backdrop).
    fn covers_much(&self, c: &m::Clip, t: m::Micros) -> bool {
        let p = &self.doc.project;
        let t = t.clamp(c.start, c.end() - 1);
        let Some(map) = crate::editor::pins::LayerMap::at(p, &self.doc.model, c.id.0 as u32, t) else { return false };
        let q: Vec<_> = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)].iter().map(|&(u, v)| map.canvas(m::Vec2::new(u, v))).collect();
        let (x0, x1) = (q.iter().map(|p| p.x).fold(f32::MAX, f32::min), q.iter().map(|p| p.x).fold(f32::MIN, f32::max));
        let (y0, y1) = (q.iter().map(|p| p.y).fold(f32::MAX, f32::min), q.iter().map(|p| p.y).fold(f32::MIN, f32::max));
        let (w, h) = (p.width as f32, p.height as f32);
        (x1.min(w / 2.0) - x0.max(-w / 2.0)).max(0.0) * (y1.min(h / 2.0) - y0.max(-h / 2.0)).max(0.0) > 0.6 * w * h
    }

    /// Camera moves for a shot, sized by `k` (1 = as asked).
    #[allow(clippy::too_many_arguments)]
    fn moves_for(&self, spec: &Spec, k: f64, depth: f64, frame_w: f64, frame_h: f64, dur: view::Time, bodies: &[Body]) -> Result<Vec<Value>, String> {
        if k <= 0.0 {
            return Ok(Vec::new());
        }
        let s = spec.speed;
        // Fast moves are short; slow ones take the whole shot and less ground.
        let len_s = if s > 1.2 { (secs(dur) * 0.4).clamp(0.4, 1.5) } else { secs(dur) };
        let amount = |base: f64| base * k * if s < 1.0 { s + 0.1 } else { s.min(1.4) };
        let flat = bodies.iter().all(|b| b.flat);
        let subject = json!({"clip": bodies[0].id});
        let mut out = Vec::new();
        for (kind, sign, extra) in &spec.moves {
            let mv = match kind.as_str() {
                "raw" => serde_json::from_str(extra).map_err(|e| e.to_string())?,
                "push_in" => json!({"type": "push_in", "by": r1(amount(0.25 * depth))}),
                "pull_out" => json!({"type": "pull_out", "by": r1(amount(0.3 * depth))}),
                "orbit" => {
                    let by = amount(35.0).min(if flat { 20.0 } else { 360.0 });
                    json!({"type": "orbit", "by": r1(by)})
                }
                "truck" => json!({"type": "truck", "by": r1(sign * amount(0.15 * frame_w))}),
                "crane" => json!({"type": "crane", "by": r1(sign * amount(22.0)), "arm": r1(0.7 * depth)}),
                "tilt" => json!({"type": "tilt", "by": r1(sign * amount(8.0))}),
                "pan" => json!({"type": "pan", "by": r1(sign * amount(15.0))}),
                "zoom" if extra == "crash" => json!({"type": "zoom", "by": 1.0 + 0.9 * k, "duration": 0.35}),
                "zoom" => json!({"type": "zoom", "by": if *sign > 0.0 { 1.0 + amount(0.3) } else { 1.0 / (1.0 + amount(0.3)) }}),
                "dolly_zoom" => json!({"type": "dolly_zoom", "by": r1(amount(0.35 * depth))}),
                "steadicam" => json!({"type": "steadicam", "by": r1(amount(0.15 * depth)), "subject": subject}),
                "handheld" => json!({"type": "handheld", "profile": spec.profile.unwrap_or(if s > 1.2 { "nervous" } else if s < 1.0 { "calm" } else { "documentary" }), "by": r1(100.0 * k)}),
                "drone" => json!({"type": "drone", "style": spec.drone_style.unwrap_or("rise"), "height": r1(amount(0.4 * frame_h * 2.0))}),
                "reveal" => json!({"type": "crane", "by": r1(amount(25.0)), "arm": r1(0.8 * depth)}),
                "follow" => json!({"type": "follow", "subject": subject}),
                "rack_focus" => match spec.focus_to {
                    Some(t) => json!({"type": "rack_focus", "to": {"clip": t}, "from": secs(dur) * 0.35, "duration": (secs(dur) * 0.3).clamp(0.4, 2.0)}),
                    None => return Err("rack focus to whom? name the subject after \"rack focus to\" (or give focus_to)".into()),
                },
                "follow_focus" => json!({"type": "follow_focus", "subject": subject}),
                other => return Err(format!("no shot move \"{other}\"")),
            };
            let mut mv = mv;
            if mv.get("duration").is_none() && !matches!(kind.as_str(), "rack_focus" | "handheld" | "follow" | "follow_focus" | "raw") {
                mv["duration"] = json!(r1(len_s * 100.0) / 100.0);
                if s > 1.2 {
                    mv["ease"] = json!("ease_out");
                }
            }
            out.push(mv);
        }
        Ok(out)
    }

    #[cfg(feature = "render")]
    /// `compare_cameras`: each option framed on the same moment, side by
    /// side, then taken back.
    pub(super) fn compare_cameras(&mut self, a: &Value) -> Result<Vec<Content>, String> {
        let opts = a.get("options").and_then(Value::as_array).ok_or("compare_cameras needs \"options\": two to six shots (words like \"low angle close-up on Pip\", or shot op fields)")?.clone();
        if !(2..=6).contains(&opts.len()) {
            return Err("compare two to six options".into());
        }
        if self.doc.in_batch() {
            return Err("compare_cameras cannot run inside an edit".into());
        }
        let g = guides::from_arg(a.get("guides"))?;
        let w = (a.get("width").and_then(Value::as_u64).unwrap_or(if opts.len() > 2 { 420 } else { 560 }) as u32).clamp(160, 960);
        let mut tiles = Vec::new();
        let mut lines = Vec::new();
        for (i, o) in opts.iter().enumerate() {
            let mut op = match o {
                Value::String(s) => json!({"say": s}),
                Value::Object(_) => o.clone(),
                _ => return Err("each option is words or shot fields".into()),
            };
            for k in ["at", "duration"] {
                if op.get(k).is_none() {
                    if let Some(v) = a.get(k) {
                        op[k] = v.clone();
                    }
                }
            }
            self.doc.begin_batch("Compare");
            let r = self.shot(&op).and_then(|said| {
                let id = self.doc.project.clips.iter().filter(|c| c.name.starts_with("Shot")).max_by_key(|c| c.id).map(|c| (c.start, c.dur));
                let t = f(a, "t").map(micros).or_else(|| id.map(|(s, d)| s + d / 2)).unwrap_or(0);
                let mut img = self.frames.rgba(&self.doc, t, w)?;
                guides::draw(&mut img, &g);
                let framing = self.framing_words(t, &g);
                Ok((img, said, framing, t))
            });
            self.doc.rollback_batch();
            let (img, said, framing, t) = r.map_err(|e| format!("option {}: {e}", i + 1))?;
            tiles.push(img);
            lines.push(json!({"option": i + 1, "t": secs(t), "shot": said, "framing": framing}));
        }
        self.frames.clear_prefetch();
        let cols = if tiles.len() <= 3 { tiles.len() } else { tiles.len().div_ceil(2) };
        let sheet = super::render::tiled(&tiles, cols, w);
        Ok(vec![
            Content::Jpeg(studio_content::jpeg::encode(&sheet, 80)),
            Content::Text(json!({"options": lines, "columns": cols, "note": "options read left to right, top to bottom; nothing was changed: make the one you choose with the shot op (the same words)"}).to_string()),
        ])
    }

    #[cfg(feature = "render")]
    /// `animatic`: every camera's shot in order, its first, middle and
    /// last moments in a row, and the shot list beside it.
    pub(super) fn animatic(&mut self, a: &Value) -> Result<Vec<Content>, String> {
        let w = self.width();
        let g = guides::from_arg(a.get("guides"))?;
        let Some(seq) = self.doc.model.sequence(self.doc.seq()) else { return Err("no sequence".into()) };
        let mut cams: Vec<m::Clip> = seq.clips.iter().filter(|c| c.enabled && super::space::rig_kind(c) == Some("camera")).cloned().collect();
        cams.sort_by_key(|c| (c.start, c.id.0));
        // Only the moments each camera is the one looking.
        let mut shots = Vec::new();
        for c in &cams {
            let (s, e) = (c.start, c.end());
            let live: Vec<m::Micros> = (0..=20).map(|k| s + (e - s - 1) * k / 20).filter(|t| sp::active_camera(seq, *t).is_some_and(|a| a.id == c.id)).collect();
            if let (Some(&a0), Some(&a1)) = (live.first(), live.last()) {
                shots.push((c.clone(), a0, a1));
            }
        }
        if shots.is_empty() {
            return Err("no cameras on the timeline: block the scene with previs or frame shots with the shot op first".into());
        }
        let tile = (a.get("width").and_then(Value::as_u64).unwrap_or(320) as u32).clamp(120, 640);
        let mut tiles = Vec::new();
        let mut list = Vec::new();
        for (i, (c, s, e)) in shots.iter().take(16).enumerate() {
            for t in [*s, s + (e - s) / 2, *e] {
                let mut img = self.frames.rgba(&self.doc, t, tile)?;
                guides::draw(&mut img, &g);
                tiles.push(img);
            }
            let set = sp::CameraSettings::read(&c.generator_params_at(0), w);
            let lens = super::cine::lens_of(&super::space::gen_params(c));
            let moved: Vec<&str> = ["transform.position", "transform.position_z", "generator.poi", "generator.zoom", "generator.focus_distance"].into_iter().filter(|p| c.keyframes.get(*p).is_some_and(|k| k.keys.len() > 1)).collect();
            list.push(format!(
                "{}. {:.2}–{:.2}s camera #{} \"{}\": {}{}{}",
                i + 1,
                secs(*s),
                secs(*e),
                c.id.0,
                c.name,
                lens.words(set.zoom, w),
                if moved.is_empty() { ", locked off".to_owned() } else { format!(", moving ({})", moved.iter().map(|p| p.split('.').nth(1).unwrap_or(p)).collect::<Vec<_>>().join(", ")) },
                if set.dof { format!(", focus at {:.0} px", set.focus_distance) } else { String::new() }
            ));
        }
        self.frames.clear_prefetch();
        let sheet = super::render::tiled(&tiles, 3, tile);
        Ok(vec![
            Content::Jpeg(studio_content::jpeg::encode(&sheet, 78)),
            Content::Text(json!({"shots": list, "note": "one row per shot: its first, middle and last moment; play or export the timeline for the moving animatic (export, any preset)"}).to_string()),
        ])
    }

    /// `previs`: stand-ins for the cast, a plain set if there is no
    /// backdrop, and a camera for each shot in turn.
    pub(super) fn previs(&mut self, op: &Value) -> Result<String, String> {
        let (w, h) = (self.width(), self.doc.project.height as f64);
        let at = micros(f(op, "at").unwrap_or(0.0));
        // Shots: given, else the project's story shot list.
        let mut shots: Vec<Value> = match op.get("shots") {
            Some(Value::Array(a)) => a.clone(),
            Some(_) => return Err("shots is a list of shots: words (\"wide on Pip and Bo\") or shot fields with duration".into()),
            None => Vec::new(),
        };
        let story = serde_json::to_value(&self.doc.model).ok().and_then(|v| v.get("story").cloned()).unwrap_or(Value::Null);
        let cast_names: HashMap<String, String> = story["cast"].as_array().map(|c| c.iter().filter_map(|x| Some((x["id"].as_str()?.to_lowercase(), x["name"].as_str().filter(|n| !n.is_empty()).or(x["id"].as_str())?.to_owned()))).collect()).unwrap_or_default();
        if shots.is_empty() {
            for sc in story["scenes"].as_array().into_iter().flatten() {
                for sh in sc["shots"].as_array().into_iter().flatten() {
                    let kind = sh["kind"].as_str().unwrap_or("medium");
                    let subjects: Vec<String> = sh["subjects"].as_array().into_iter().flatten().filter_map(Value::as_str).map(|s| cast_names.get(&s.to_lowercase()).cloned().unwrap_or_else(|| s.to_owned())).collect();
                    let mv = match sh["camera"].as_str().unwrap_or("static") {
                        "push_in" => "slow push in",
                        "pull_out" => "slow pull out",
                        "track" => "follow",
                        "crane" => "crane up",
                        "handheld" => "handheld",
                        "zoom" => "zoom in",
                        "pan" => "pan right",
                        "tilt" => "tilt up",
                        _ => "",
                    };
                    let size = match kind {
                        "establishing" => "extreme_wide",
                        "reaction" => "close_up",
                        k => k,
                    };
                    shots.push(json!({"size": size, "on": subjects, "move": mv, "duration": sh["duration"].as_f64().map_or(3.0, |d| d / SEC as f64), "framing": sh["framing"], "shot_id": sh["id"]}));
                }
            }
        }
        if shots.is_empty() {
            return Err("previs needs \"shots\" (or a story with a shot list: story-structure's shot list is read when there is one)".into());
        }
        let mut said = Vec::new();
        // The cast: every name a shot or the op mentions.
        let mut cast: Vec<String> = op.get("cast").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_owned).collect()).unwrap_or_default();
        for s in &shots {
            let mut ons: Vec<String> = match s.get("on") {
                Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).map(str::to_owned).collect(),
                Some(Value::String(x)) => vec![x.clone()],
                _ => Vec::new(),
            };
            // Names in the words: capitalised words that are not shot words.
            let say = s.as_str().or(s.get("say").and_then(Value::as_str)).unwrap_or("");
            ons.extend(names_in(say));
            for n in ons {
                if !cast.iter().any(|c| c.eq_ignore_ascii_case(&n)) {
                    cast.push(n);
                }
            }
        }
        let total: f64 = shots.iter().map(|s| f(s, "duration").unwrap_or(3.0)).sum();
        for s in shots.iter_mut() {
            if let Value::String(w) = s {
                *s = json!({"say": w.clone()});
            }
        }
        let named = self.nameable();
        let floor = 0.32 * h;
        let palette = ["#d9825b", "#5b8fd9", "#7bc47f", "#c98bd9", "#d9c45b", "#5bd0c9", "#d95b7a", "#9a9a9a"];
        let missing: Vec<String> = cast.iter().filter(|n| !named.iter().any(|(_, x)| x.eq_ignore_ascii_case(n))).cloned().collect();
        let n = missing.len().max(1) as f64;
        for (i, name) in missing.iter().enumerate() {
            let size = story["cast"].as_array().and_then(|c| c.iter().find(|x| x["name"].as_str().is_some_and(|v| v.eq_ignore_ascii_case(name)) || x["id"].as_str().is_some_and(|v| v.eq_ignore_ascii_case(name)))).and_then(|x| x["size"].as_f64()).unwrap_or(1.0);
            let tall = 0.5 * h * size.clamp(0.2, 3.0);
            let x = if n <= 1.0 { 0.0 } else { (i as f64 / (n - 1.0) - 0.5) * 0.55 * w };
            let r = |k: f64| (k * tall * 10.0).round() / 10.0;
            let parts = json!([
                {"shape": "ellipsoid", "at": [0, r(-0.42), 0], "size": [r(0.15), r(0.18), r(0.16)]},
                {"shape": "capsule", "from": [0, r(-0.3), 0], "to": [0, r(0.04), 0], "radius": r(0.1), "blend": r(0.03)},
                {"shape": "capsule", "from": [r(-0.05), r(0.06), 0], "to": [r(-0.06), r(0.47), 0], "radius": r(0.04), "blend": r(0.02)},
                {"shape": "capsule", "from": [r(0.05), r(0.06), 0], "to": [r(0.06), r(0.47), 0], "radius": r(0.04), "blend": r(0.02)},
                {"shape": "capsule", "from": [r(-0.12), r(-0.26), 0], "to": [r(-0.16), r(0.02), 0], "radius": r(0.03), "blend": r(0.02)},
                {"shape": "capsule", "from": [r(0.12), r(-0.26), 0], "to": [r(0.16), r(0.02), 0], "radius": r(0.03), "blend": r(0.02)},
                {"shape": "box", "at": [0, r(-0.42), r(-0.08)], "size": [r(0.05), r(0.03), r(0.05)]}
            ]);
            let m_op = json!({"op": "model", "name": name, "parts": parts, "colour": palette[i % palette.len()], "roughness": 0.8, "position": [r1(x), r1(floor - 0.5 * tall), 0], "at": secs(at), "duration": total, "detail": 2});
            self.op(&m_op).map_err(|e| format!("stand-in for {name}: {e}"))?;
            said.push(format!("stand-in \"{name}\" ({:.0} px tall)", tall));
        }
        // A plain set when nothing in 3D fills the frame: a back wall and
        // a floor, so camera moves read.
        let has_backdrop = self.doc.model.sequence(self.doc.seq()).is_some_and(|q| q.clips.iter().any(|c| c.transform.three_d && matches!(c.source, m::ClipSource::Media(_)) && self.covers_much(c, at)));
        if op.get("set").and_then(Value::as_bool).unwrap_or(!has_backdrop) {
            let zoom = sp::default_zoom(w);
            for (what, colour, extra) in [("Previs wall", "#4a4f57", json!({"z": r1(0.9 * zoom)})), ("Previs floor", "#3a3d42", json!({"rotation_x": 90, "y": r1(floor)}))] {
                let before: std::collections::BTreeSet<u32> = self.doc.project.clips.iter().map(|c| c.id).collect();
                self.op(&json!({"op": "card", "colour": colour, "at": secs(at), "duration": total}))?;
                let Some(id) = self.doc.project.clips.iter().map(|c| c.id).find(|i| !before.contains(i)) else { continue };
                let mut mc = self.model_clip(id).ok_or("set piece gone")?;
                mc.name = what.into();
                mc.transform.three_d = true;
                // Big enough that no camera here sees past it.
                (mc.transform.scale_x, mc.transform.scale_y) = (4.0, 4.0);
                if let Some(z) = extra.get("z").and_then(Value::as_f64) {
                    mc.transform.position_z = z;
                }
                if let Some(y) = extra.get("y").and_then(Value::as_f64) {
                    mc.transform.position = m::Vec2::new(0.0, y);
                    mc.transform.rotation_x = 90.0;
                }
                self.apply_clip("Previs set", mc)?;
                said.push(format!("{what} clip {id}"));
            }
        }
        // A camera per shot, one after another.
        let mut t = at;
        for (i, s) in shots.iter().enumerate() {
            let mut sop = s.clone();
            let d = f(s, "duration").unwrap_or(3.0);
            sop["at"] = json!(secs(t));
            sop["duration"] = json!(d);
            if let Some(o) = sop.as_object_mut() {
                o.remove("shot_id");
                o.remove("framing");
                if o.get("move").and_then(Value::as_str) == Some("") {
                    o.remove("move");
                }
            }
            let r = self.shot(&sop).map_err(|e| format!("shot {}: {e}", i + 1))?;
            said.push(format!("shot {} ({}): {}", i + 1, s.get("shot_id").and_then(Value::as_str).map(str::to_owned).unwrap_or_else(|| format!("{:.1}s", secs(t))), r.split("; framing").next().unwrap_or(&r)));
            t += micros(d);
        }
        if self.doc.project.duration() < t {
            said.push(format!("note: the shots run to {:.1}s; the stand-ins cover them all", secs(t)));
        }
        said.push("look at it with animatic (a row per shot), or play it".into());
        Ok(format!("previs: {}", said.join("; ")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services;

    /// A 640 × 360 scene: an opaque backdrop on the main lane and "Pip",
    /// a see-through figure facing right (its head ahead of its body),
    /// both in 3D.
    #[cfg(feature = "render")]
    fn scene(tag: &str) -> (Session, u32, u32) {
        let dir = std::env::temp_dir().join(format!("studio-cine-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (bg, fig) = (dir.join("hill.png"), dir.join("pip.png"));
        let mut px = Vec::new();
        for y in 0..360u32 {
            for x in 0..640u32 {
                // A bright window up on the left.
                let lit = x < 160 && y < 120;
                px.extend(if lit { [255, 214, 150, 255] } else { [(40 + x / 8) as u8, (70 + y / 6) as u8, 60, 255] });
            }
        }
        std::fs::write(&bg, studio_media::png::encode(&studio_media::rgba::RgbaImage { width: 640, height: 360, pixels: px })).unwrap();
        let (w, h) = (120u32, 240u32);
        let mut px = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                let (fx, fy) = (x as f64, y as f64);
                let head = (fx - 78.0).powi(2) + (fy - 34.0).powi(2) < 26.0f64.powi(2);
                let body = ((fx - 56.0) / 34.0).powi(2) + ((fy - 150.0) / 88.0).powi(2) < 1.0;
                if head || body {
                    let i = ((y * w + x) * 4) as usize;
                    px[i..i + 4].copy_from_slice(&[200, 120, 60, 255]);
                }
            }
        }
        std::fs::write(&fig, studio_media::png::encode(&studio_media::rgba::RgbaImage { width: w, height: h, pixels: px })).unwrap();
        let mut s = Session::new(services::project::new_custom("Cine", 640, 360, 30.0));
        s.run("import", &json!({"paths": [bg.to_string_lossy()], "to_timeline": true})).unwrap();
        let bg_id = s.doc.project.clips.iter().map(|c| c.id).max().unwrap();
        s.edit(&[json!({"op": "trim", "clip": bg_id, "end": 4.0})], "len", false).ok();
        let parts = s.run("import", &json!({"paths": [fig.to_string_lossy()]})).unwrap();
        let v: Value = parts.iter().find_map(|p| if let Content::Text(t) = p { serde_json::from_str(t).ok() } else { None }).unwrap();
        let media = v["media"][0].as_u64().unwrap();
        s.edit(&[json!({"op": "place", "media": media, "at": 0, "lane": "overlay", "duration": 4, "name": "Pip"})], "pip", false).unwrap();
        let pip = s.doc.project.clips.iter().map(|c| c.id).max().unwrap();
        s.edit(&[json!({"op": "set", "clip": pip, "scale": 50}), json!({"op": "three_d", "clips": [bg_id, pip]})], "3d", false).unwrap();
        (s, bg_id, pip)
    }

    #[cfg(feature = "render")]
    fn keys(s: &Session, id: u32, path: &str) -> Vec<f64> {
        s.model_clip(id).and_then(|c| c.keyframes.get(path).map(|t| t.keys.iter().filter_map(|k| match &k.value { m::Value::Point(p) => Some(p.y), v => v.as_f64() }).collect())).unwrap_or_default()
    }

    /// "slow push in on Pip": a medium shot with its eyes on the upper
    /// third and lead room ahead of where it faces, then a push in that
    /// never shows past the backdrop.
    #[cfg(feature = "render")]
    #[test]
    fn a_named_shot_frames_the_subject_on_the_thirds() {
        let (mut s, _, pip) = scene("named");
        assert_eq!(s.subjects_at(SEC).first().map(|x| x.facing), Some(1), "Pip faces right");
        let r = s.edit(&[json!({"op": "shot", "say": "slow push in on Pip"})], "shot", false).unwrap();
        let said = format!("{r:?}");
        assert!(said.contains("medium shot on #"), "{said}");
        let cam = s.doc.project.clips.iter().filter(|c| c.name.starts_with("Shot")).map(|c| c.id).max().unwrap();
        let z = keys(&s, cam, "transform.position_z");
        assert!(z.len() >= 2 && z.last().unwrap() > z.first().unwrap(), "pushed in: {z:?}");
        let start = s.clip(cam).unwrap().start;
        let pipped = s.subjects_at(start).into_iter().find(|x| x.id == pip).unwrap();
        assert!((pipped.eyes[1] - 1.0 / 3.0).abs() < 0.06, "eyes on the upper third: {:?}", pipped.eyes);
        assert!(pipped.eyes[0] < 0.45, "facing right, it stands on the left: {:?}", pipped.eyes);
        let c = s.clip(cam).unwrap();
        assert!(s.gaps(c.start, c.start + c.dur).is_none(), "no empty canvas: {said}");
        // A close-up is tighter; an unknown subject is refused with names.
        s.edit(&[json!({"op": "shot", "on": "Pip", "size": "close_up", "at": 0, "duration": 2})], "cu", false).unwrap();
        let e = s.edit(&[json!({"op": "shot", "say": "close-up on Bo"})], "x", false).unwrap_err();
        assert!(e.contains("Pip") || e.contains("pip"), "{e}");
    }

    /// A wide shot with a pull out runs past a backdrop that only just
    /// fills the frame: it grows or the move is cut back; either way the
    /// canvas never shows.
    #[cfg(feature = "render")]
    #[test]
    fn a_wide_move_never_shows_the_empty_canvas() {
        let (mut s, _, _) = scene("wide");
        let r = s.edit(&[json!({"op": "shot", "say": "wide shot on Pip, pull back", "at": 0, "duration": 3})], "wide", false).unwrap();
        let cam = s.doc.project.clips.iter().filter(|c| c.name.starts_with("Shot")).map(|c| c.id).max().unwrap();
        let c = s.clip(cam).unwrap();
        assert!(s.gaps(c.start, c.start + c.dur).is_none(), "{r:?}");
        // A camera move pushed far sideways with nothing to cover it is
        // reported.
        let r = s.edit(&[json!({"op": "camera", "clip": cam, "move": {"type": "truck", "by": 4000, "duration": 2}})], "far", false).unwrap();
        assert!(format!("{r:?}").contains("empty canvas") || s.gaps(c.start, c.start + c.dur).is_none(), "{r:?}");
    }

    #[cfg(feature = "render")]
    #[test]
    fn lenses_focus_pulls_and_rigs_key_the_camera() {
        let (mut s, bg, pip) = scene("rigs");
        let r = s.edit(&[json!({"op": "camera", "lens_kit": "anamorphic", "focal_length": 40, "sensor": "super35"})], "lens", false).unwrap();
        let cam = s.doc.project.clips.iter().filter(|c| super::super::space::rig_kind(&s.model_clip(c.id).unwrap()) == Some("camera")).map(|c| c.id).max().unwrap();
        let p = super::super::space::gen_params(&s.model_clip(cam).unwrap());
        let zoom = p.get("zoom").and_then(m::Value::as_f64).unwrap();
        assert!((zoom - 640.0 * 40.0 / (24.89 * 2.0)).abs() < 0.5, "40 mm on Super 35, 2× squeeze: {zoom}; {r:?}");
        assert!(format!("{r:?}").contains("Lens"), "a lens layer for the kit's character: {r:?}");
        let lens = s.doc.project.clips.iter().find(|c| c.name.starts_with("Lens (camera")).map(|c| c.id).unwrap();
        assert!(s.model_clip(lens).unwrap().effects.iter().any(|e| e.kind == "fisheye"));
        // Rack focus to Pip: focus keys, and the anamorphic lens breathes.
        s.edit(&[json!({"op": "camera", "clip": cam, "move": {"type": "rack_focus", "to": {"clip": pip}, "from": 1, "duration": 1}})], "rack", false).unwrap();
        assert_eq!(keys(&s, cam, "generator.focus_distance").len(), 2);
        assert_eq!(keys(&s, cam, "generator.zoom").len(), 2, "breathing keys the zoom");
        // The oval bokeh is baked on the layers as a lens blur.
        let bl = s.model_clip(bg).unwrap();
        assert!(bl.effects.iter().any(|e| e.kind == "lens_blur" && e.params.get("shape").and_then(|v| v.as_str()) == Some("Oval")), "oval bokeh on the backdrop");
        // Rigs: a crane rises, handheld shakes many keys, a drone climbs.
        s.edit(&[json!({"op": "camera", "clip": cam, "move": {"type": "crane", "by": 25, "duration": 2}})], "crane", false).unwrap();
        let y = keys(&s, cam, "transform.position");
        assert!(y.last().unwrap() < y.first().unwrap(), "crane up rises (y goes up the screen): {y:?}");
        let r = s.edit(&[json!({"op": "camera", "clip": cam, "move": {"type": "handheld", "profile": "nervous", "duration": 2}})], "hh", false).unwrap();
        assert!(keys(&s, cam, "generator.poi").len() > 15, "{r:?}");
        for mv in [json!({"type": "drone", "style": "reveal", "duration": 2}), json!({"type": "steadicam", "subject": {"clip": pip}}), json!({"type": "dolly_path", "path": [[100, 0, -400], [200, -50, -300]], "aim": "keep"}), json!({"type": "follow", "subject": {"clip": pip}})] {
            s.edit(&[json!({"op": "camera", "clip": cam, "move": mv})], "rig", false).unwrap_or_else(|e| panic!("{mv}: {e}"));
        }
        let e = s.edit(&[json!({"op": "camera", "clip": cam, "move": {"type": "handheld", "profile": "wobbly"}})], "bad", false).unwrap_err();
        assert!(e.contains("calm"), "{e}");
    }

    #[cfg(feature = "render")]
    #[test]
    fn lighting_setups_and_motivated_light() {
        let (mut s, _, pip) = scene("light");
        let r = s.edit(&[json!({"op": "lighting", "preset": "three_point", "on": pip})], "l", false).unwrap();
        let lights = |s: &Session| s.doc.model.sequence(s.doc.seq()).unwrap().clips.iter().filter(|c| c.enabled && super::super::space::rig_kind(c) == Some("light")).count();
        assert_eq!(lights(&s), 4, "key, fill, back, ambient: {r:?}");
        let r = s.edit(&[json!({"op": "lighting", "preset": "motivated", "on": "pip"})], "m", false).unwrap();
        let said = format!("{r:?}");
        assert!(said.contains("upper left"), "the window is up on the left: {said}");
        assert_eq!(lights(&s), 3, "the earlier set-up is switched off: key, fill, ambient");
        assert!(s.edit(&[json!({"op": "lighting", "mood": "tense"})], "t", false).is_ok());
    }

    #[cfg(feature = "render")]
    #[test]
    fn framing_is_read_against_the_guides_and_checked() {
        let (mut s, _, pip) = scene("frame");
        let parts = s.run("frame", &json!({"t": 1.0, "guides": ["thirds", "2.39"]})).unwrap();
        let text = parts.iter().find_map(|p| if let Content::Text(t) = p { Some(t.clone()) } else { None }).unwrap();
        assert!(text.contains("framing") && text.contains("Pip"), "{text}");
        // Pushed against the right edge facing right: no lead room.
        s.edit(&[json!({"op": "three_d", "clip": pip, "on": false}), json!({"op": "set", "clip": pip, "x": 250})], "edge", false).unwrap();
        let p = s.framing_problems();
        assert!(p.iter().any(|v| v["kind"] == "lead_room"), "{p:?}");
    }

    #[cfg(feature = "render")]
    #[test]
    fn previs_animatic_and_camera_options() {
        let mut s = Session::new(services::project::new_custom("Previs", 640, 360, 30.0));
        let r = s.edit(&[json!({"op": "previs", "shots": [{"say": "wide on Ana and Ben", "duration": 2}, {"say": "slow push in on Ana", "duration": 2}, {"size": "close_up", "on": "Ben", "angle": "low", "duration": 1.5}]})], "previs", false).unwrap();
        let said = format!("{r:?}");
        let names: Vec<String> = s.doc.project.clips.iter().map(|c| c.name.clone()).collect();
        assert!(names.iter().any(|n| n == "Ana") && names.iter().any(|n| n == "Ben"), "stand-ins: {names:?} {said}");
        assert_eq!(names.iter().filter(|n| n.starts_with("Shot")).count(), 3, "{said}");
        assert!(names.iter().any(|n| n == "Previs wall"), "{names:?}");
        let parts = s.run("animatic", &json!({})).unwrap();
        assert!(matches!(parts[0], Content::Jpeg(_)));
        let text = parts.iter().find_map(|p| if let Content::Text(t) = p { Some(t.clone()) } else { None }).unwrap();
        assert!(text.contains("3."), "three shots listed: {text}");
        let before = s.doc.project.clips.len();
        let parts = s.run("compare_cameras", &json!({"options": ["low angle close-up on Ana", "high angle wide on Ana and Ben"], "t": 1.0})).unwrap();
        assert!(matches!(parts[0], Content::Jpeg(_)));
        assert_eq!(s.doc.project.clips.len(), before, "comparing changes nothing");
    }

    #[test]
    fn words_find_sizes_and_moves() {
        assert!(has("slow push in on pip", "push in").is_some());
        assert!(has("pipe", "pip").is_none());
        assert_eq!(SIZE_WORDS.iter().find(|(w, _)| has("an extreme close-up of bo", w).is_some()).map(|x| x.1), Some("extreme_close_up"));
        assert_eq!(size("CU").map(|s| s.name), Some("close_up"));
        assert_eq!(size("cowboy").map(|s| s.name), Some("medium_wide"));
        assert!(angle("low angle").is_some_and(|a| a.0 < 0.0));
        assert_eq!(names_in("Wide on Ana and Ben, slow push in"), vec!["Ana", "Ben"]);
        assert!(names_in("close-up on the group").is_empty());
    }
}
