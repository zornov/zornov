// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Composition guides: the viewer's overlays (rule of thirds, golden
//! ratio, centre, action and title safe, aspect guides), in one place for
//! the viewer that draws them and the agent that reads the picture
//! against them.
//!
//! * The viewer says which guides are on ([`set_viewer`]); `see`, `frame`
//!   and the framing check use those when the agent names none.
//! * [`draw`] paints guides onto a rendered frame, so the agent sees the
//!   lines the person sees.
//! * [`Subject`] is a figure on screen (its silhouette's box and where its
//!   eyes are) and [`report`] says where it sits against the guides;
//!   [`problems`] finds poor framing: headroom, lead room, limbs cut at a
//!   joint, tangents.
//!
//! Positions are fractions of the frame: x from the left, y from the top.

use serde_json::{Value, json};
use std::sync::Mutex;
use studio_media::rgba::RgbaImage;

/// One overlay.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Guide {
    Thirds,
    Golden,
    Centre,
    Safe,
    /// An aspect frame (width / height) inside the canvas, e.g. 2.39.
    Aspect(f32),
}

/// Aspect guides the viewer offers, with their names.
pub const ASPECTS: [(&str, f32); 7] = [("2.39:1", 2.39), ("2:1", 2.0), ("1.85:1", 1.85), ("16:9", 16.0 / 9.0), ("4:3", 4.0 / 3.0), ("1:1", 1.0), ("9:16", 9.0 / 16.0)];

/// The golden section, as a fraction of the frame.
const PHI: f32 = 0.381_966;

/// Title safe and action safe, as fractions of the frame (the viewer's).
pub const TITLE_SAFE: f32 = 0.9;
pub const ACTION_SAFE: f32 = 0.95;

impl Guide {
    pub fn name(self) -> String {
        match self {
            Guide::Thirds => "thirds".into(),
            Guide::Golden => "golden".into(),
            Guide::Centre => "centre".into(),
            Guide::Safe => "safe".into(),
            Guide::Aspect(a) => ASPECTS.iter().find(|(_, v)| (v - a).abs() < 0.005).map_or_else(|| format!("{a:.2}:1"), |(n, _)| (*n).to_owned()),
        }
    }

    /// Reads `thirds`, `golden` (`phi`, `golden_ratio`), `centre`
    /// (`center`, `cross`), `safe` (`title_safe`, `action_safe`), or an
    /// aspect (`2.39`, `2.39:1`, `scope`, `4:3`, `9:16`, `square`).
    pub fn parse(s: &str) -> Option<Guide> {
        let k = s.trim().to_lowercase().replace([' ', '-'], "_");
        Some(match k.as_str() {
            "thirds" | "rule_of_thirds" | "third" | "grid" => Guide::Thirds,
            "golden" | "phi" | "golden_ratio" | "golden_section" | "phi_grid" => Guide::Golden,
            "centre" | "center" | "cross" | "centre_cross" | "center_cross" | "middle" => Guide::Centre,
            "safe" | "safe_areas" | "title_safe" | "action_safe" | "safe_area" => Guide::Safe,
            "scope" | "cinemascope" | "anamorphic" => Guide::Aspect(2.39),
            "flat" => Guide::Aspect(1.85),
            "square" => Guide::Aspect(1.0),
            "vertical" | "portrait" | "reel" | "story" => Guide::Aspect(9.0 / 16.0),
            other => {
                let other = other.trim_end_matches("_aspect").trim_end_matches("_guide");
                let v = match other.split_once(':') {
                    Some((a, b)) => a.parse::<f32>().ok()? / b.parse::<f32>().ok().filter(|b| *b > 0.0)?,
                    None => other.parse::<f32>().ok()?,
                };
                if !(0.3..=4.0).contains(&v) {
                    return None;
                }
                Guide::Aspect(v)
            }
        })
    }

    /// The aspect frame on a `w` × `h` canvas: left, top, right, bottom
    /// as fractions.
    fn aspect_rect(a: f32, w: f32, h: f32) -> [f32; 4] {
        let canvas = w / h.max(1.0);
        if a >= canvas {
            let k = canvas / a;
            [0.0, (1.0 - k) / 2.0, 1.0, (1.0 + k) / 2.0]
        } else {
            let k = a / canvas;
            [(1.0 - k) / 2.0, 0.0, (1.0 + k) / 2.0, 1.0]
        }
    }
}

/// Guides from an agent's `guides` argument: a list or comma-separated
/// names, `true` (the viewer's), `false` / `[]` (none). Missing: the
/// viewer's.
pub fn from_arg(v: Option<&Value>) -> Result<Vec<Guide>, String> {
    let names: Vec<String> = match v {
        None | Some(Value::Null) | Some(Value::Bool(true)) => return Ok(viewer()),
        Some(Value::Bool(false)) => return Ok(Vec::new()),
        Some(Value::String(s)) if s == "viewer" || s == "on" => return Ok(viewer()),
        Some(Value::String(s)) => s.split(',').map(str::to_owned).collect(),
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).map(str::to_owned).collect(),
        _ => return Err(GUIDE_NAMES.into()),
    };
    names.iter().filter(|n| !n.trim().is_empty()).map(|n| Guide::parse(n).ok_or_else(|| format!("guide \"{n}\": {GUIDE_NAMES}"))).collect()
}

pub const GUIDE_NAMES: &str = "guides are thirds, golden, centre, safe, or an aspect (2.39, 1.85, 4:3, 1:1, 9:16)";

static VIEWER: Mutex<Vec<Guide>> = Mutex::new(Vec::new());

/// The viewer's overlays now on (the window calls this as they change).
pub fn set_viewer(g: &[Guide]) {
    if let Ok(mut v) = VIEWER.lock() {
        if v.as_slice() != g {
            *v = g.to_vec();
        }
    }
}

/// The overlays on in the viewer (none in a headless agent).
pub fn viewer() -> Vec<Guide> {
    VIEWER.lock().map(|v| v.clone()).unwrap_or_default()
}

/// Words for the editor context line: "rule of thirds, 2.39:1 guide".
pub fn describe(g: &[Guide]) -> String {
    g.iter()
        .map(|g| match g {
            Guide::Thirds => "rule of thirds".to_owned(),
            Guide::Golden => "golden ratio".to_owned(),
            Guide::Centre => "centre cross".to_owned(),
            Guide::Safe => "title/action safe".to_owned(),
            Guide::Aspect(_) => format!("{} aspect guide", g.name()),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Lines a guide draws: vertical x positions and horizontal y positions
/// (fractions), and rectangles (left, top, right, bottom), each with
/// whether it is dashed.
pub struct Lines {
    pub vertical: Vec<f32>,
    pub horizontal: Vec<f32>,
    pub rects: Vec<([f32; 4], bool, &'static str)>,
    /// The aspect frame: what lies outside it is dimmed.
    pub matte: Option<[f32; 4]>,
}

/// What the guides draw on a `w` × `h` canvas.
pub fn lines(guides: &[Guide], w: f32, h: f32) -> Lines {
    let mut out = Lines { vertical: Vec::new(), horizontal: Vec::new(), rects: Vec::new(), matte: None };
    // Thirds and golden lines fall inside the aspect frame when one is on.
    let frame = guides.iter().find_map(|g| if let Guide::Aspect(a) = g { Some(Guide::aspect_rect(*a, w, h)) } else { None }).unwrap_or([0.0, 0.0, 1.0, 1.0]);
    let fx = |k: f32| frame[0] + (frame[2] - frame[0]) * k;
    let fy = |k: f32| frame[1] + (frame[3] - frame[1]) * k;
    for g in guides {
        match *g {
            Guide::Thirds => {
                out.vertical.extend([fx(1.0 / 3.0), fx(2.0 / 3.0)]);
                out.horizontal.extend([fy(1.0 / 3.0), fy(2.0 / 3.0)]);
            }
            Guide::Golden => {
                out.vertical.extend([fx(PHI), fx(1.0 - PHI)]);
                out.horizontal.extend([fy(PHI), fy(1.0 - PHI)]);
            }
            Guide::Centre => {}
            Guide::Safe => {
                for (k, name) in [(TITLE_SAFE, "Title safe"), (ACTION_SAFE, "Action safe")] {
                    let m = (1.0 - k) / 2.0;
                    out.rects.push(([m, m, 1.0 - m, 1.0 - m], true, name));
                }
            }
            Guide::Aspect(a) => {
                let r = Guide::aspect_rect(a, w, h);
                out.rects.push((r, false, ""));
                out.matte = Some(r);
            }
        }
    }
    out
}

/// Paints the guides onto a frame: the dimmed outside of an aspect
/// guide, then the lines, light with a dark edge so they read on any
/// picture.
pub fn draw(img: &mut RgbaImage, guides: &[Guide]) {
    if guides.is_empty() {
        return;
    }
    let (w, h) = (img.width as usize, img.height as usize);
    let l = lines(guides, w as f32, h as f32);
    let px = |img: &mut RgbaImage, x: usize, y: usize, c: [u8; 3], a: f32| {
        if x < w && y < h {
            let i = (y * w + x) * 4;
            for k in 0..3 {
                img.pixels[i + k] = (img.pixels[i + k] as f32 * (1.0 - a) + c[k] as f32 * a).round() as u8;
            }
        }
    };
    if let Some(r) = l.matte {
        let (x0, y0, x1, y1) = ((r[0] * w as f32) as usize, (r[1] * h as f32) as usize, (r[2] * w as f32) as usize, (r[3] * h as f32) as usize);
        for y in 0..h {
            for x in 0..w {
                if x < x0 || x >= x1 || y < y0 || y >= y1 {
                    px(img, x, y, [0, 0, 0], 0.55);
                }
            }
        }
    }
    let thick = (w / 900).max(1);
    let vline = |img: &mut RgbaImage, x: usize, y0: usize, y1: usize, dashed: bool| {
        for y in y0..y1.min(h) {
            if dashed && (y / (6 * thick)) % 2 == 1 {
                continue;
            }
            for d in 0..thick {
                px(img, (x + d).saturating_sub(thick / 2), y, [255, 255, 255], 0.7);
            }
            px(img, x + thick, y, [0, 0, 0], 0.35);
        }
    };
    let hline = |img: &mut RgbaImage, y: usize, x0: usize, x1: usize, dashed: bool| {
        for x in x0..x1.min(w) {
            if dashed && (x / (6 * thick)) % 2 == 1 {
                continue;
            }
            for d in 0..thick {
                px(img, x, (y + d).saturating_sub(thick / 2), [255, 255, 255], 0.7);
            }
            px(img, x, y + thick, [0, 0, 0], 0.35);
        }
    };
    let at = |f: f32, n: usize| ((f * n as f32).round() as usize).min(n.saturating_sub(1));
    for &x in &l.vertical {
        vline(img, at(x, w), 0, h, false);
    }
    for &y in &l.horizontal {
        hline(img, at(y, h), 0, w, false);
    }
    for (r, dashed, _) in &l.rects {
        let (x0, y0, x1, y1) = (at(r[0], w), at(r[1], h), at(r[2], w), at(r[3], h));
        vline(img, x0, y0, y1 + 1, *dashed);
        vline(img, x1, y0, y1 + 1, *dashed);
        hline(img, y0, x0, x1 + 1, *dashed);
        hline(img, y1, x0, x1 + 1, *dashed);
    }
    if guides.contains(&Guide::Centre) {
        let (cx, cy, arm) = (w / 2, h / 2, w / 40);
        hline(img, cy, cx - arm, cx + arm + 1, false);
        vline(img, cx, cy - arm, cy + arm + 1, false);
    }
}

/// A figure on screen.
#[derive(Clone, Debug)]
pub struct Subject {
    pub id: u32,
    pub name: String,
    /// Its silhouette's box: left, top, right, bottom (fractions; may run
    /// past 0..1 when it is cut by the frame).
    pub bbox: [f32; 4],
    /// Where its eyes are (an estimate from the silhouette when no face
    /// was found).
    pub eyes: [f32; 2],
    /// Which way it looks or moves on screen: +1 right, -1 left, 0 at the
    /// camera or unknown.
    pub facing: i8,
}

impl Subject {
    fn width(&self) -> f32 {
        self.bbox[2] - self.bbox[0]
    }
    fn height(&self) -> f32 {
        self.bbox[3] - self.bbox[1]
    }
    fn centre(&self) -> [f32; 2] {
        [(self.bbox[0] + self.bbox[2]) / 2.0, (self.bbox[1] + self.bbox[3]) / 2.0]
    }
    /// Shown whole or nearly (not a close-up of part of it).
    fn whole(&self) -> bool {
        self.bbox[1] > -0.01 && self.bbox[3] < 1.01
    }
}

fn pct(v: f32) -> String {
    format!("{:.0}%", v * 100.0)
}

/// The nearest line among `xs` to `v`, and how far.
fn nearest(xs: &[f32], v: f32) -> Option<(f32, f32)> {
    xs.iter().map(|&x| (x, (x - v).abs())).min_by(|a, b| a.1.total_cmp(&b.1))
}

/// Where each subject sits against the guides, one line each.
pub fn report(subjects: &[Subject], guides: &[Guide], w: f32, h: f32) -> Vec<String> {
    let g: Vec<Guide> = if guides.is_empty() { vec![Guide::Thirds] } else { guides.to_vec() };
    let l = lines(&g, w, h);
    let frame = l.matte.unwrap_or([0.0, 0.0, 1.0, 1.0]);
    let mut out = Vec::new();
    for s in subjects {
        let mut said = Vec::new();
        let c = s.centre();
        let side = |x: f32, lines: &[f32]| -> Option<String> {
            let (lx, d) = nearest(lines, x)?;
            let name = if lx < 0.5 { "left" } else { "right" };
            (d < 0.06).then(|| if d < 0.02 { format!("on the {name} line") } else { format!("near the {name} line ({} off)", pct(d)) })
        };
        let kind = if g.contains(&Guide::Golden) && !g.contains(&Guide::Thirds) { "golden" } else { "third" };
        let vertical: Vec<f32> = l.vertical.clone();
        let horizontal: Vec<f32> = l.horizontal.clone();
        if !vertical.is_empty() {
            match side(c[0], &vertical) {
                Some(t) => said.push(format!("body {t} ({kind}s)")),
                None if (c[0] - 0.5).abs() < 0.04 => said.push("body dead centre".into()),
                None => said.push(format!("body at x {}", pct(c[0]))),
            }
            if let Some((ly, d)) = nearest(&horizontal, s.eyes[1]) {
                let name = if ly < 0.5 { "upper" } else { "lower" };
                said.push(if d < 0.025 { format!("eyes on the {name} {kind} line") } else { format!("eyes {} {} the {name} {kind} line", pct(d), if s.eyes[1] < ly { "above" } else { "below" }) });
            }
            // The strongest points: where the lines cross.
            let best = vertical.iter().flat_map(|&x| horizontal.iter().map(move |&y| (x, y))).map(|(x, y)| ((x - s.eyes[0]).hypot(y - s.eyes[1]), x, y)).min_by(|a, b| a.0.total_cmp(&b.0));
            if let Some((d, x, y)) = best.filter(|b| b.0 < 0.05) {
                said.push(format!("eyes at the {}{} power point ({} off)", if y < 0.5 { "top " } else { "bottom " }, if x < 0.5 { "left" } else { "right" }, pct(d)));
            }
        }
        if g.contains(&Guide::Centre) && (c[0] - 0.5).abs() < 0.03 && (c[1] - 0.5).abs() < 0.05 {
            said.push("on the centre cross".into());
        }
        if g.contains(&Guide::Safe) {
            let ts = (1.0 - TITLE_SAFE) / 2.0;
            let as_ = (1.0 - ACTION_SAFE) / 2.0;
            let b = s.bbox;
            if b[0] < as_ || b[1] < as_ || b[2] > 1.0 - as_ || b[3] > 1.0 - as_ {
                said.push("runs outside action safe".into());
            } else if b[0] < ts || b[1] < ts || b[2] > 1.0 - ts || b[3] > 1.0 - ts {
                said.push("inside action safe, outside title safe".into());
            } else {
                said.push("inside title safe".into());
            }
        }
        if l.matte.is_some() {
            let b = s.bbox;
            if b[1] < frame[1] - 0.005 && s.eyes[1] > frame[1] {
                said.push(format!("the {} frame cuts the top of the head", g.iter().find(|g| matches!(g, Guide::Aspect(_))).map(|g| g.name()).unwrap_or_default()));
            }
            if s.eyes[1] < frame[1] || s.eyes[1] > frame[3] || s.eyes[0] < frame[0] || s.eyes[0] > frame[2] {
                said.push("its eyes fall outside the aspect frame".into());
            }
        }
        // Room above the head and in front of the eyes.
        let headroom = s.bbox[1] - frame[1];
        said.push(if headroom < -0.005 { "top cut by the frame".to_owned() } else { format!("headroom {}", pct(headroom / (frame[3] - frame[1]))) });
        if s.facing != 0 {
            let ahead = if s.facing > 0 { frame[2] - s.eyes[0] } else { s.eyes[0] - frame[0] };
            said.push(format!("looks {} with {} of the frame ahead", if s.facing > 0 { "right" } else { "left" }, pct(ahead / (frame[2] - frame[0]))));
        }
        out.push(format!("#{} {}: {}", s.id, s.name, said.join(", ")));
    }
    out
}

/// Poor framing: each a `{kind, problem, fix, clip}` for `check`.
pub fn problems(subjects: &[Subject], frame: [f32; 4]) -> Vec<Value> {
    let mut out = Vec::new();
    let (fw, fh) = (frame[2] - frame[0], frame[3] - frame[1]);
    let push = |out: &mut Vec<Value>, kind: &str, s: &Subject, problem: String, fix: &str| {
        out.push(json!({"kind": kind, "problem": problem, "fix": fix, "clip": s.id}));
    };
    for s in subjects {
        let b = s.bbox;
        let (h, w) = (s.height() / fh, s.width() / fw);
        let name = format!("clip {} (\"{}\")", s.id, s.name);
        // Headroom: a head cut by the top edge while the figure is not a
        // close-up, or a big gap over a figure that fills the frame.
        let top = (b[1] - frame[1]) / fh;
        if top < -0.01 && h < 1.6 && s.eyes[1] > frame[1] + 0.02 {
            push(&mut out, "headroom", s, format!("{name}: the top of its head is cut by the frame ({} of its height off the top)", pct(-top * fh / s.height().max(1e-3))), "for a close-up cut across the forehead deliberately (the eyes on the upper third), otherwise lower it or pull back so the whole head shows with a little room");
        } else if h > 0.45 && top > 0.18 && s.whole() {
            push(&mut out, "headroom", s, format!("{name}: {} of empty frame over its head; it sinks in the frame", pct(top)), "raise it (or tilt down) so its eyes sit near the upper third line: headroom of 5-12% for a medium shot");
        }
        // Lead room: looking or moving toward the near edge.
        if s.facing != 0 && s.whole() {
            let ahead = if s.facing > 0 { frame[2] - s.eyes[0] } else { s.eyes[0] - frame[0] } / fw;
            let behind = 1.0 - ahead;
            if ahead < 0.4 && behind > ahead + 0.12 {
                push(&mut out, "lead_room", s, format!("{name} looks {} with only {} of the frame ahead of it and {} behind: it is boxed in against the edge", if s.facing > 0 { "right" } else { "left" }, pct(ahead), pct(behind)), "give it lead room: put it on the third line opposite the way it looks (looking right: the left third), so the space is ahead of it");
            }
        }
        // Limbs cut at a joint: the frame's bottom edge crossing the
        // figure at the ankles or knees, a side edge clipping a hand.
        if b[3] > frame[3] + 0.005 && b[1] > frame[1] - 0.005 {
            let kept = (frame[3] - b[1]) / s.height().max(1e-3);
            if (0.88..0.985).contains(&kept) {
                push(&mut out, "cut_limbs", s, format!("{name}: the bottom of the frame cuts it at the ankles (keeps {} of its height)", pct(kept)), "show the feet with a little floor under them, or cut higher, mid-thigh or at the waist, never at a joint");
            } else if (0.66..0.78).contains(&kept) {
                push(&mut out, "cut_limbs", s, format!("{name}: the bottom of the frame cuts it at the knees (keeps {} of its height)", pct(kept)), "cut mid-thigh (a cowboy shot) or show the whole figure, not at the knee");
            }
        }
        for (cut, edge) in [((frame[0] - b[0]) / s.width().max(1e-3), "left"), ((b[2] - frame[2]) / s.width().max(1e-3), "right")] {
            if (0.02..0.2).contains(&cut) && w < 0.8 {
                push(&mut out, "cut_limbs", s, format!("{name}: the {edge} edge clips {} of its width (a hand or a shoulder)", pct(cut)), "bring it fully in or crop well into it: a sliver cut off reads as a mistake");
            }
        }
        // Tangents: an edge just touching the frame.
        for (gap, edge) in [((b[0] - frame[0]) / fw, "left"), ((frame[2] - b[2]) / fw, "right"), ((b[1] - frame[1]) / fh, "top"), ((frame[3] - b[3]) / fh, "bottom")] {
            if (0.0..0.012).contains(&gap) {
                push(&mut out, "tangent", s, format!("{name} just touches the {edge} edge of the frame (a tangent: the eye snags on it)"), "move it clearly inside the frame or clearly across the edge");
            }
        }
    }
    // Two figures just touching.
    for (i, a) in subjects.iter().enumerate() {
        for b in &subjects[i + 1..] {
            let vert = a.bbox[1].max(b.bbox[1]) < a.bbox[3].min(b.bbox[3]);
            let gap = (b.bbox[0] - a.bbox[2]).max(a.bbox[0] - b.bbox[2]) / fw;
            if vert && (0.0..0.008).contains(&gap) {
                out.push(json!({"kind": "tangent", "problem": format!("clips {} and {} just touch edge to edge (a tangent: they read as one shape)", a.id, b.id), "fix": "overlap them clearly (one in front) or open a gap between them", "clip": b.id}));
            }
        }
    }
    out
}

/// The frame the subjects are judged in: the first aspect guide's, or
/// the whole canvas.
pub fn frame_of(guides: &[Guide], w: f32, h: f32) -> [f32; 4] {
    lines(guides, w, h).matte.unwrap_or([0.0, 0.0, 1.0, 1.0])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn subject(bbox: [f32; 4], facing: i8) -> Subject {
        let eyes = [(bbox[0] + bbox[2]) / 2.0 + facing as f32 * 0.02, bbox[1] + 0.1 * (bbox[3] - bbox[1])];
        Subject { id: 1, name: "Pip".into(), bbox, eyes, facing }
    }

    #[test]
    fn parses_guides() {
        assert_eq!(Guide::parse("rule of thirds"), Some(Guide::Thirds));
        assert_eq!(Guide::parse("2.39:1"), Some(Guide::Aspect(2.39)));
        assert_eq!(Guide::parse("4:3").map(|g| g.name()), Some("4:3".into()));
        assert_eq!(Guide::parse("golden-ratio"), Some(Guide::Golden));
        assert!(Guide::parse("banana").is_none());
        assert!(from_arg(Some(&json!(["thirds", "nope"]))).is_err());
        assert_eq!(from_arg(Some(&json!("thirds,centre"))).unwrap(), vec![Guide::Thirds, Guide::Centre]);
    }

    #[test]
    fn aspect_frames_letterbox_and_pillarbox() {
        let r = Guide::aspect_rect(2.39, 1920.0, 1080.0);
        assert!(r[0] == 0.0 && r[1] > 0.1 && (r[3] - r[1] - 1.7778 / 2.39).abs() < 1e-3);
        let r = Guide::aspect_rect(9.0 / 16.0, 1920.0, 1080.0);
        assert!(r[1] == 0.0 && (r[2] - r[0] - 0.3164).abs() < 1e-3);
    }

    #[test]
    fn report_reads_thirds_and_lead_room() {
        // Eyes at x 1/3 + 0.02, y 1/3: on the left third, looking right.
        let s = subject([0.25, 0.30, 0.38, 0.63], 1);
        let r = report(&[s], &[Guide::Thirds], 1920.0, 1080.0).join(" ");
        assert!(r.contains("eyes on the upper third line"), "{r}");
        assert!(r.contains("power point"), "{r}");
        assert!(r.contains("looks right with"), "{r}");
    }

    #[test]
    fn framing_problems_found() {
        // Facing right, pressed against the right edge.
        let boxed = subject([0.75, 0.2, 0.92, 0.9], 1);
        let p = problems(&[boxed], [0.0, 0.0, 1.0, 1.0]);
        assert!(p.iter().any(|v| v["kind"] == "lead_room"), "{p:?}");
        // Feet cut at the ankles.
        let ankles = subject([0.4, 0.1, 0.6, 1.05], 0);
        let p = problems(&[ankles], [0.0, 0.0, 1.0, 1.0]);
        assert!(p.iter().any(|v| v["kind"] == "cut_limbs"), "{p:?}");
        // Head cut in a wide shot.
        let head = subject([0.4, -0.03, 0.6, 0.7], 0);
        assert!(problems(&[head], [0.0, 0.0, 1.0, 1.0]).iter().any(|v| v["kind"] == "headroom"));
        // Just touching the left edge.
        let touch = subject([0.005, 0.3, 0.2, 0.8], 0);
        assert!(problems(&[touch], [0.0, 0.0, 1.0, 1.0]).iter().any(|v| v["kind"] == "tangent"));
        // Well framed: none.
        let good = subject([0.28, 0.12, 0.42, 0.9], 1);
        assert!(problems(&[good], [0.0, 0.0, 1.0, 1.0]).is_empty());
    }

    #[test]
    fn draws_lines_and_matte() {
        let mut img = RgbaImage { width: 300, height: 200, pixels: vec![128; 300 * 200 * 4] };
        draw(&mut img, &[Guide::Thirds, Guide::Aspect(2.39)]);
        // A third line is light; the letterbox above the 2.39 frame dark.
        assert!(img.pixels[(100 * 300 + 100) * 4] > 180);
        assert!(img.pixels[(2 * 300 + 150) * 4] < 70);
    }
}
