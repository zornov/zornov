// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Rigs and motions for `puppet`: pins placed from a layer's silhouette
//! (`rig`) and life baked into them as keys (`act`).
//!
//! Everything here works in the layer's own 0..1 coordinates (what the
//! puppet stores), so a motion follows the layer when it is moved, scaled
//! or flipped. "Front" is the side the character faces in its picture.

use serde_json::{Value, json};
use std::f64::consts::TAU;

/// Which pixels of the picture are the character.
pub(super) struct Shape {
    w: usize,
    h: usize,
    on: Vec<bool>,
    /// How the shape was found, for the report.
    pub from: &'static str,
}

impl Shape {
    /// A shape from 8-bit coverage (alpha or a matte), on above half.
    pub fn from_coverage(w: usize, h: usize, cover: &[u8], invert: bool, from: &'static str) -> Option<Shape> {
        if w == 0 || h == 0 || cover.len() < w * h {
            return None;
        }
        // Work at about 256 px across: plenty for pin places.
        let step = (w.max(h) / 256).max(1);
        let (sw, sh) = (w.div_ceil(step), h.div_ceil(step));
        let mut on = Vec::with_capacity(sw * sh);
        for y in (0..h).step_by(step) {
            for x in (0..w).step_by(step) {
                on.push((cover[y * w + x] >= 128) != invert);
            }
        }
        let n = on.iter().filter(|&&b| b).count();
        (n > 16 && n < sw * sh * 98 / 100).then_some(Shape { w: sw, h: sh, on, from })
    }

    /// A picture with no transparency: the character is what differs from
    /// the colour around its border (a cast sheet on a plain backdrop).
    pub fn keyed(w: usize, h: usize, rgb: &[[u8; 3]]) -> Option<Shape> {
        if w < 4 || h < 4 || rgb.len() < w * h {
            return None;
        }
        let mut border: Vec<[u8; 3]> = Vec::new();
        for x in 0..w {
            border.push(rgb[x]);
            border.push(rgb[(h - 1) * w + x]);
        }
        for y in 0..h {
            border.push(rgb[y * w]);
            border.push(rgb[y * w + w - 1]);
        }
        let mut bg = [0u8; 3];
        for c in 0..3 {
            let mut v: Vec<u8> = border.iter().map(|p| p[c]).collect();
            v.sort_unstable();
            bg[c] = v[v.len() / 2];
        }
        let far = |p: [u8; 3]| (0..3).map(|c| (p[c] as i32 - bg[c] as i32).pow(2)).sum::<i32>() > 40 * 40;
        let plain = border.iter().filter(|p| !far(**p)).count();
        if plain * 10 < border.len() * 8 {
            return None; // no plain backdrop to key
        }
        let cover: Vec<u8> = rgb[..w * h].iter().map(|p| if far(*p) { 255 } else { 0 }).collect();
        Shape::from_coverage(w, h, &cover, false, "the picture's backdrop colour")
    }

    /// The picture's own transparency (a PNG), or its plain backdrop.
    pub fn of_png(path: &std::path::Path) -> Option<Shape> {
        let mut d = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).ok()?));
        d.set_transformations(png::Transformations::normalize_to_color8());
        let mut r = d.read_info().ok()?;
        let mut buf = vec![0; r.output_buffer_size()?];
        let info = r.next_frame(&mut buf).ok()?;
        let (w, h) = (info.width as usize, info.height as usize);
        let px = &buf[..info.buffer_size()];
        let step = (w.max(h) / 256).max(1);
        let (sw, sh) = (w.div_ceil(step), h.div_ceil(step));
        let chans = match info.color_type {
            png::ColorType::Grayscale => 1,
            png::ColorType::GrayscaleAlpha => 2,
            png::ColorType::Rgb => 3,
            png::ColorType::Rgba => 4,
            png::ColorType::Indexed => return None,
        };
        let at = |x: usize, y: usize| &px[(y * w + x) * chans..(y * w + x + 1) * chans];
        let mut alpha = Vec::with_capacity(sw * sh);
        let mut rgb = Vec::with_capacity(sw * sh);
        for y in (0..h).step_by(step) {
            for x in (0..w).step_by(step) {
                let p = at(x, y);
                let (c, a) = match chans {
                    1 => ([p[0]; 3], 255),
                    2 => ([p[0]; 3], p[1]),
                    3 => ([p[0], p[1], p[2]], 255),
                    _ => ([p[0], p[1], p[2]], p[3]),
                };
                alpha.push(a);
                rgb.push(c);
            }
        }
        if alpha.iter().any(|&a| a < 128) {
            Shape::from_coverage(sw, sh, &alpha, false, "the picture's transparency")
        } else {
            Shape::keyed(sw, sh, &rgb)
        }
    }

    /// The part of the picture a crop leaves (fractions cut from the
    /// left, top, right and bottom), which is what the layer's 0..1
    /// coordinates span.
    pub fn cropped(self, l: f64, t: f64, r: f64, b: f64) -> Option<Shape> {
        if l + t + r + b <= 0.0 {
            return Some(self);
        }
        let x0 = (l.clamp(0.0, 1.0) * self.w as f64).round() as usize;
        let y0 = (t.clamp(0.0, 1.0) * self.h as f64).round() as usize;
        let x1 = ((1.0 - r.clamp(0.0, 1.0)) * self.w as f64).round() as usize;
        let y1 = ((1.0 - b.clamp(0.0, 1.0)) * self.h as f64).round() as usize;
        if x1 <= x0 + 2 || y1 <= y0 + 2 {
            return None;
        }
        let mut cover = Vec::with_capacity((x1 - x0) * (y1 - y0));
        for y in y0..y1 {
            for x in x0..x1 {
                cover.push(if self.at(x, y) { 255 } else { 0 });
            }
        }
        Shape::from_coverage(x1 - x0, y1 - y0, &cover, false, self.from)
    }

    fn at(&self, x: usize, y: usize) -> bool {
        self.on[y * self.w + x]
    }

    /// The silhouette grid: width, height and cells (row-major, true =
    /// the character).
    pub fn grid(&self) -> (usize, usize, &[bool]) {
        (self.w, self.h, &self.on)
    }

    fn uv(&self, x: usize, y: usize) -> (f64, f64) {
        ((x as f64 + 0.5) / self.w as f64, (y as f64 + 0.5) / self.h as f64)
    }

    /// The box the character fills, in columns and rows.
    fn bounds(&self) -> (usize, usize, usize, usize) {
        let (mut x0, mut y0, mut x1, mut y1) = (self.w, self.h, 0, 0);
        for y in 0..self.h {
            for x in 0..self.w {
                if self.at(x, y) {
                    x0 = x0.min(x);
                    x1 = x1.max(x);
                    y0 = y0.min(y);
                    y1 = y1.max(y);
                }
            }
        }
        (x0, y0, x1.max(x0), y1.max(y0))
    }

    fn centroid(&self) -> (f64, f64) {
        let (mut sx, mut sy, mut n) = (0.0f64, 0.0f64, 0.0f64);
        for y in 0..self.h {
            for x in 0..self.w {
                if self.at(x, y) {
                    sx += x as f64;
                    sy += y as f64;
                    n += 1.0;
                }
            }
        }
        (sx / n.max(1.0), sy / n.max(1.0))
    }

    /// The pixel of the shape nearest to column `x`, row `y`.
    fn snap(&self, x: f64, y: f64) -> (usize, usize) {
        let (cx, cy) = (x.round().clamp(0.0, self.w as f64 - 1.0) as usize, y.round().clamp(0.0, self.h as f64 - 1.0) as usize);
        let mut best = (cx, cy, usize::MAX);
        for yy in 0..self.h {
            for xx in 0..self.w {
                if self.at(xx, yy) {
                    let d = xx.abs_diff(cx).pow(2) + yy.abs_diff(cy).pow(2);
                    if d < best.2 {
                        best = (xx, yy, d);
                    }
                }
            }
        }
        (best.0, best.1)
    }

    /// The shape's pixel furthest along `dir` (`(dx, dy)`, a unit step)
    /// among columns `cols` and rows `rows` (fractions of the box), moved
    /// back inside by a little so the pin holds flesh, not the outline.
    fn extreme(&self, cols: (f64, f64), rows: (f64, f64), dir: (f64, f64)) -> Option<(usize, usize)> {
        let (x0, y0, x1, y1) = self.bounds();
        let (bw, bh) = ((x1 - x0) as f64, (y1 - y0) as f64);
        let (ca, cb) = (x0 as f64 + cols.0 * bw, x0 as f64 + cols.1 * bw);
        let (ra, rb) = (y0 as f64 + rows.0 * bh, y0 as f64 + rows.1 * bh);
        let mut best: Option<(usize, usize, f64)> = None;
        for y in (ra.floor().max(0.0) as usize)..=(rb.ceil() as usize).min(self.h - 1) {
            for x in (ca.floor().max(0.0) as usize)..=(cb.ceil() as usize).min(self.w - 1) {
                if self.at(x, y) {
                    let s = x as f64 * dir.0 + y as f64 * dir.1;
                    if best.is_none_or(|b| s > b.2) {
                        best = Some((x, y, s));
                    }
                }
            }
        }
        let (x, y, _) = best?;
        let inset = 0.04 * bw.max(bh);
        Some(self.snap(x as f64 - dir.0 * inset, y as f64 - dir.1 * inset))
    }

    /// Which way a side view faces, guessed from where the shape's top
    /// sits against its middle (heads lead; tails and backs trail). `None`
    /// when it reads as facing the camera.
    pub fn facing(&self) -> Option<bool> {
        let (x0, y0, x1, y1) = self.bounds();
        let (cx, _) = self.centroid();
        let band = y0 + ((y1 - y0) as f64 * 0.3) as usize;
        let (mut s, mut n) = (0.0f64, 0.0f64);
        for y in y0..=band {
            for x in x0..=x1 {
                if self.at(x, y) {
                    s += x as f64;
                    n += 1.0;
                }
            }
        }
        let lean = (s / n.max(1.0) - cx) / ((x1 - x0) as f64).max(1.0);
        (lean.abs() > 0.04).then_some(lean > 0.0)
    }
}

pub(super) const KINDS: &str = "biped (person, standing animal), quadruped, bird, fish, plant (or tree, flame), object, backdrop (a whole background picture)";

/// Pins for a kind of character, facing right when `right`, as `pins`
/// entries of the puppet op (`{"name", "at": {"u", "v"}}`).
pub(super) fn rig(kind: &str, s: &Shape, right: bool) -> Result<Vec<Value>, String> {
    let f = |a: f64| if right { a } else { 1.0 - a };
    // Front and back column ranges of the box.
    let front = |a: f64, b: f64| if right { (1.0 - b, 1.0 - a) } else { (a, b) };
    let fwd = if right { 1.0 } else { -1.0 };
    let (x0, y0, x1, y1) = s.bounds();
    let (bw, bh) = ((x1 - x0) as f64, (y1 - y0) as f64);
    let place = |u: f64, v: f64| s.snap(x0 as f64 + u * bw, y0 as f64 + v * bh);
    let (cx, cy) = s.centroid();
    let body = s.snap(cx, cy);
    let up = (0.0, -1.0);
    let down = (0.0, 1.0);
    let mut pins: Vec<(&str, Option<(usize, usize)>)> = Vec::new();
    let mut joints: Vec<(String, (usize, usize))> = Vec::new();
    match kind {
        "biped" | "person" | "human" | "character" => {
            pins.push(("head", s.extreme((0.3, 0.7), (0.0, 0.3), up)));
            pins.push(("chest", Some(place(0.5, 0.38))));
            pins.push(("hips", Some(place(0.5, 0.62))));
            pins.push(("back_hand", s.extreme(front(0.75, 1.0), (0.3, 0.7), (-fwd, 0.0))));
            pins.push(("front_hand", s.extreme(front(0.0, 0.25), (0.3, 0.7), (fwd, 0.0))));
            pins.push(("back_foot", s.extreme(front(0.5, 1.0), (0.8, 1.0), down)));
            pins.push(("front_foot", s.extreme(front(0.0, 0.5), (0.8, 1.0), down)));
        }
        "quadruped" | "animal" | "dog" | "cat" | "horse" => {
            pins.push(("head", s.extreme(front(0.0, 0.35), (0.0, 0.6), up)));
            pins.push(("back", Some(place(f(0.5), 0.35))));
            pins.push(("body", Some(body)));
            pins.push(("tail", s.extreme(front(0.8, 1.0), (0.0, 0.7), (-fwd, 0.0))));
            pins.push(("front_foot", s.extreme(front(0.0, 0.4), (0.8, 1.0), down)));
            pins.push(("back_foot", s.extreme(front(0.6, 1.0), (0.8, 1.0), down)));
        }
        "bird" | "rooster" | "chicken" => {
            pins.push(("head", s.extreme(front(0.0, 0.5), (0.0, 0.45), up)));
            pins.push(("body", Some(body)));
            pins.push(("tail", s.extreme(front(0.75, 1.0), (0.0, 0.75), (-fwd, 0.0))));
            pins.push(("feet", s.extreme((0.25, 0.75), (0.85, 1.0), down)));
        }
        "fish" => {
            pins.push(("head", s.extreme(front(0.0, 0.3), (0.2, 0.8), (fwd, 0.0))));
            pins.push(("body", Some(body)));
            pins.push(("tail", s.extreme(front(0.75, 1.0), (0.1, 0.9), (-fwd, 0.0))));
        }
        "plant" | "tree" | "flame" | "flower" | "grass" => {
            pins.push(("base", s.extreme((0.3, 0.7), (0.85, 1.0), down)));
            pins.push(("middle", Some(place(0.5, 0.5))));
            pins.push(("top", s.extreme((0.2, 0.8), (0.0, 0.2), up)));
        }
        "object" | "thing" | "prop" => {
            pins.push(("base", s.extreme((0.3, 0.7), (0.85, 1.0), down)));
            pins.push(("top", s.extreme((0.3, 0.7), (0.0, 0.15), up)));
        }
        other => return Err(format!("rig \"{other}\": {KINDS}")),
    }
    // Legs get a hip and a knee above each foot, so a step bends the leg
    // instead of dragging the body with it.
    let hip = match kind {
        "biped" | "person" | "human" | "character" => Some(0.58),
        "quadruped" | "animal" | "dog" | "cat" | "horse" => Some(0.55),
        "bird" | "rooster" | "chicken" => Some(0.72),
        _ => None,
    };
    // Arms get a shoulder and an elbow above each hand, so a swing turns
    // the arm at the shoulder and bends it at the elbow.
    if matches!(kind, "biped" | "person" | "human" | "character") {
        let hands: Vec<(String, (usize, usize))> = pins.iter().filter(|(n, _)| n.contains("hand")).filter_map(|(n, p)| p.map(|p| (n.replace("_hand", ""), p))).collect();
        let sy = y0 as f64 + 0.27 * bh;
        for (side, (hx, hy)) in hands {
            let sx = hx as f64 + (cx - hx as f64) * 0.55;
            joints.push((format!("{side}_shoulder"), s.snap(sx, sy)));
            joints.push((format!("{side}_elbow"), s.snap((sx + hx as f64) / 2.0, (sy + hy as f64) / 2.0)));
        }
    }
    if let Some(hv) = hip {
        let feet: Vec<(String, (usize, usize))> = pins.iter().filter(|(n, _)| n.contains("foot") || n.contains("feet")).filter_map(|(n, p)| p.map(|p| (n.replace("_foot", "").replace("feet", ""), p))).collect();
        let hy = y0 as f64 + hv * bh;
        for (side, (fx, fy)) in feet {
            let pre = if side.is_empty() { String::new() } else { format!("{side}_") };
            joints.push((format!("{pre}hip"), s.snap(fx as f64, hy)));
            joints.push((format!("{pre}knee"), s.snap(fx as f64, (hy + fy as f64) / 2.0)));
        }
    }
    let mut out = Vec::new();
    let all = pins.into_iter().filter_map(|(n, p)| p.map(|p| (n.to_owned(), p))).chain(joints);
    for (name, (x, y)) in all {
        if out.iter().any(|q: &(f64, f64, String)| (q.0 - x as f64).abs() + (q.1 - y as f64).abs() < 3.0) {
            continue; // two parts landed on one spot (a tiny or odd shape)
        }
        out.push((x as f64, y as f64, name));
    }
    if out.len() < 2 {
        return Err("the silhouette is too small to rig: place pins yourself with \"pins\"".into());
    }
    Ok(out
        .into_iter()
        .map(|(x, y, name)| {
            let (u, v) = s.uv(x as usize, y as usize);
            json!({"name": name, "at": {"u": (u * 1000.0).round() / 1000.0, "v": (v * 1000.0).round() / 1000.0}})
        })
        .collect())
}

pub(in crate::agent) const ACTS: &str = "breathe, idle, talk, listen, nod, shake_head, look, think, point, wave, surprise, laugh, scared, sad, celebrate, peck, wag (tail), hop, walk, run, sneak, swim, sway; on a backdrop: breeze";

/// Pins for a whole backdrop (rig "backdrop"): the ground and the lower
/// half held, the top and sides free to move, so canopies, grass and
/// water sway while the land stays put.
pub(super) fn backdrop() -> Vec<Value> {
    [
        ("base left", 0.1, 0.95),
        ("base centre", 0.5, 0.95),
        ("base right", 0.9, 0.95),
        ("ground left", 0.2, 0.62),
        ("ground centre", 0.5, 0.6),
        ("ground right", 0.8, 0.62),
        ("top left", 0.12, 0.1),
        ("top centre", 0.5, 0.06),
        ("top right", 0.88, 0.1),
        ("edge left", 0.05, 0.38),
        ("edge right", 0.95, 0.38),
    ]
    .iter()
    .map(|(n, u, v)| json!({"name": n, "at": {"u": u, "v": v}}))
    .collect()
}

/// What a pin is, from its name.
#[derive(Clone, Copy, PartialEq)]
enum Part {
    Head,
    Body,
    Hand(bool),
    Elbow(bool),
    Hip(bool),
    Knee(bool),
    Foot(bool),
    Tail,
    Top,
    Edge,
    Base,
    Other,
}

fn part(name: &str) -> Part {
    let n = name.to_lowercase();
    let front = n.contains("front") || n.contains("right");
    if n.contains("head") || n.contains("face") || n.contains("beak") {
        Part::Head
    } else if n.contains("elbow") || n.contains("forearm") {
        Part::Elbow(front)
    } else if n.contains("hand") || n.contains("arm") || n.contains("wing") {
        Part::Hand(front)
    } else if n.contains("knee") || n.contains("shin") {
        Part::Knee(front)
    } else if n.contains("hip") && !n.contains("hips") || n.contains("thigh") {
        Part::Hip(front)
    } else if n.contains("foot") || n.contains("feet") || n.contains("leg") || n.contains("paw") {
        Part::Foot(front)
    } else if n.contains("tail") || n.contains("fin") {
        Part::Tail
    } else if n.contains("top") || n.contains("tip") || n.contains("canopy") {
        Part::Top
    } else if n.contains("edge") {
        Part::Edge
    } else if n.contains("base") || n.contains("root") || n.contains("ground") {
        Part::Base
    } else if ["body", "chest", "back", "hips", "middle", "torso", "belly", "shoulder", "neck"].iter().any(|b| n.contains(b)) {
        Part::Body
    } else {
        Part::Other
    }
}

pub(in crate::agent) fn known_act(a: &str) -> bool {
    matches!(
        a,
        "breathe" | "idle" | "talk" | "listen" | "nod" | "shake_head" | "look" | "think" | "point" | "wave" | "surprise" | "laugh" | "scared" | "shiver" | "sad" | "celebrate" | "peck" | "wag" | "tail_sway" | "hop" | "walk" | "run" | "sneak" | "swim" | "sway" | "breeze"
    )
}

/// Acts that happen once at the start of their window (not a loop).
fn once(act: &str) -> bool {
    matches!(act, "surprise")
}

/// A step cycle. Each foot is planted for 60% of the stride, sliding back
/// under the body at an even pace (so on a travelling layer it stays put on
/// the ground), then swings forward for 40% in a lifted arc and lands; the
/// knee leads the swing forward and up; the hands swing against the feet,
/// the elbows half as far; the body is lowest as a foot lands and highest
/// as the legs pass, twice a stride, and the head and shoulders ride it.
fn gait(p: Part, t: f64, period: f64, reach: f64, lift: f64, bob: f64, crouch: f64) -> (f64, f64) {
    let pi = std::f64::consts::PI;
    // (forward, up, how far into the swing 0..1..0)
    let step = |front: bool| -> (f64, f64, f64) {
        let u = (t / period + if front { 0.0 } else { 0.5 }).rem_euclid(1.0);
        if u < 0.6 {
            (reach * (1.0 - 2.0 * u / 0.6), 0.0, 0.0)
        } else {
            let s = (u - 0.6) / 0.4;
            let e = 0.5 - 0.5 * (pi * s).cos();
            (reach * (2.0 * e - 1.0), lift * (pi * s).sin(), (pi * s).sin())
        }
    };
    let b = -bob * (TAU * 2.0 * t / period).cos() - crouch;
    // A hand is forward when the other side's foot is.
    let swing = |front: bool| 0.75 * reach * (TAU * (t / period + if front { 0.5 } else { 0.0 })).cos();
    match p {
        Part::Foot(front) => {
            let (f, u, _) = step(front);
            (f, u)
        }
        Part::Knee(front) => {
            let (f, u, sw) = step(front);
            (0.5 * f + 0.3 * reach * sw, 0.55 * u + 0.25 * lift * sw + 0.5 * b)
        }
        Part::Hand(front) => {
            let x = swing(front);
            (x, b + 0.12 * x.abs())
        }
        Part::Elbow(front) => (0.45 * swing(front), b + 0.04 * swing(front).abs()),
        Part::Head => (0.1 * reach + 0.04 * reach * (TAU * 2.0 * t / period).sin(), 0.85 * b + 0.006 - 0.5 * crouch),
        Part::Tail => (-0.2 * reach * (TAU * t / period).sin(), b),
        Part::Base | Part::Edge | Part::Top => (0.0, 0.0),
        _ => (0.0, b),
    }
}

/// How fast (layer heights a second) a planted foot slides back under the
/// body in a gait at `speed`: the pace a layer travels at for its feet to
/// stay still on the ground.
pub(in crate::agent) fn planted_pace(act: &str, speed: f64) -> Option<f64> {
    let (period, reach) = match act {
        "walk" => (0.9, 0.12),
        "run" => (0.5, 0.16),
        "sneak" => (1.4, 0.06),
        _ => return None,
    };
    Some(2.0 * reach / (0.6 * period) * speed)
}

/// How far pin `name` sits from rest at `t` seconds into `act`, as
/// `(forward, up)` in heights of the layer. `speed` scales the tempo.
fn offset(act: &str, name: &str, t: f64, speed: f64) -> (f64, f64) {
    let p = part(name);
    // Outside a gait an elbow goes half as far as its hand.
    if let Part::Elbow(front) = p
        && !matches!(act, "walk" | "run" | "sneak")
    {
        let (f, u) = offset(act, if front { "front_hand" } else { "back_hand" }, t, speed);
        return (0.5 * f, 0.5 * u);
    }
    let t = t * speed;
    let wave = |period: f64| (TAU * t / period).sin();
    let pi = std::f64::consts::PI;
    match act {
        "breathe" => {
            let s = wave(3.4);
            match p {
                Part::Head => (0.0, 0.010 * s),
                Part::Body | Part::Hand(_) | Part::Top | Part::Hip(_) => (0.0, 0.008 * s),
                Part::Tail => (0.0, 0.004 * s),
                _ => (0.0, 0.0),
            }
        }
        "idle" => {
            let (f, u) = offset("breathe", name, t, 1.0);
            match p {
                Part::Head => (f + 0.006 * wave(5.3), u),
                Part::Tail => (f + 0.006 * wave(2.9), u),
                Part::Top => (f + 0.012 * wave(4.1), u),
                _ => (f, u),
            }
        }
        "talk" => {
            // A syllable-rate bob with a slow lean toward the listener.
            let syl = (TAU * t * 1.7).sin().abs();
            match p {
                Part::Head => (0.006 + 0.006 * wave(2.3), 0.018 * syl),
                Part::Body => (0.0, 0.005 * syl),
                Part::Hand(true) => (0.01 * wave(1.9), 0.02 * wave(1.9).max(0.0)),
                _ => (0.0, 0.0),
            }
        }
        "listen" => match p {
            // Leaning in, with small slow nods.
            Part::Head => (0.01, 0.004 - 0.008 * wave(2.6).max(0.0)),
            Part::Body => (0.004, 0.0),
            _ => (0.0, 0.0),
        },
        "nod" => match p {
            Part::Head => (0.004 * wave(0.9).max(0.0), -0.02 * wave(0.9).max(0.0)),
            _ => (0.0, 0.0),
        },
        "shake_head" => match p {
            Part::Head => (0.02 * wave(0.45), 0.0),
            _ => (0.0, 0.0),
        },
        "look" => match p {
            Part::Head => (0.025 * wave(4.0), 0.004 * wave(2.0)),
            _ => (0.0, 0.0),
        },
        "think" => match p {
            Part::Head => (-0.006 + 0.004 * wave(3.1), 0.012),
            Part::Hand(true) => (0.01, 0.08),
            _ => (0.0, 0.0),
        },
        "point" => match p {
            Part::Hand(true) => (0.09, 0.06),
            Part::Head => (0.008, 0.002),
            Part::Body => (0.006, 0.0),
            _ => (0.0, 0.0),
        },
        "wave" => match p {
            Part::Hand(true) => (0.03 * wave(0.6), 0.12 + 0.03 * wave(0.6)),
            Part::Head => (0.0, 0.004 * wave(1.2)),
            _ => (0.0, 0.0),
        },
        "surprise" => {
            // A start back and up in a third of a second, held, then settling.
            let k = if t < 0.3 { (pi * t / 0.6).sin() } else { (-(t - 0.9).max(0.0) / 0.8).exp() };
            match p {
                Part::Head => (-0.03 * k, 0.05 * k),
                Part::Body | Part::Hip(_) => (-0.015 * k, 0.03 * k),
                Part::Hand(_) => (-0.01 * k, 0.07 * k),
                Part::Tail => (0.0, 0.04 * k),
                Part::Knee(_) => (-0.005 * k, 0.012 * k),
                _ => (0.0, 0.0),
            }
        }
        "laugh" => {
            let b = (TAU * t * 4.0).sin().abs();
            match p {
                Part::Head => (-0.01, 0.016 * b),
                Part::Body | Part::Hip(_) => (0.0, 0.012 * b),
                Part::Hand(_) => (0.0, 0.012 * b),
                Part::Tail => (0.0, 0.01 * b),
                _ => (0.0, 0.0),
            }
        }
        "scared" | "shiver" => {
            let tr = 0.004 * (TAU * t * 11.0).sin();
            match p {
                Part::Head => (-0.012 + tr, -0.025),
                Part::Body | Part::Hip(_) => (-0.006 + tr, -0.02),
                Part::Hand(_) => (tr, 0.01),
                Part::Tail => (0.0, -0.02),
                Part::Knee(_) => (0.0, -0.01),
                _ => (0.0, 0.0),
            }
        }
        "sad" => {
            let s = 0.004 * wave(5.0);
            match p {
                Part::Head => (0.012, -0.035 + s),
                Part::Body | Part::Hip(_) => (0.0, -0.012 + s),
                Part::Tail => (0.0, -0.025),
                Part::Hand(_) => (0.0, -0.012),
                _ => (0.0, 0.0),
            }
        }
        "celebrate" => {
            let (f, u) = offset("hop", name, t, 1.0);
            match p {
                Part::Hand(_) => (f + 0.02 * wave(0.4), u + 0.14),
                Part::Tail => (f + 0.02 * wave(0.3), u),
                _ => (f, u),
            }
        }
        "peck" => {
            // Quick dips forward and down, about one a second.
            let ph = (t / 1.2).fract();
            let k = if (0.2..0.5).contains(&ph) { (pi * (ph - 0.2) / 0.3).sin() } else { 0.0 };
            match p {
                Part::Head => (0.05 * k, -0.10 * k),
                Part::Body => (0.01 * k, -0.012 * k),
                Part::Tail => (0.0, 0.012 * k),
                _ => (0.0, 0.0),
            }
        }
        "wag" | "tail_sway" => match p {
            Part::Tail => (0.03 * wave(0.7), 0.012 * wave(0.35)),
            _ => (0.0, 0.0),
        },
        "hop" => {
            let ph = (t / 0.8).fract();
            let k = if ph < 0.6 { (pi * ph / 0.6).sin() } else { 0.0 };
            let squash = if ph >= 0.6 { (pi * (ph - 0.6) / 0.4).sin() } else { 0.0 };
            let lift = 0.15 * k;
            match p {
                Part::Foot(_) | Part::Base => (0.0, lift),
                Part::Knee(_) => (0.0, lift - 0.02 * squash),
                Part::Head | Part::Top => (0.0, lift - 0.03 * squash),
                _ => (0.0, lift - 0.015 * squash),
            }
        }
        "walk" => gait(p, t, 0.9, 0.12, 0.07, 0.018, 0.0),
        "run" => gait(p, t, 0.5, 0.16, 0.11, 0.032, 0.015),
        "sneak" => gait(p, t, 1.4, 0.06, 0.035, 0.006, 0.035),
        "swim" => match p {
            Part::Tail => (0.0, 0.05 * wave(1.1)),
            Part::Body => (0.0, -0.012 * wave(1.1)),
            Part::Head => (0.0, 0.01 * wave(1.1)),
            _ => (0.0, 0.0),
        },
        "sway" => match p {
            Part::Top => (0.035 * wave(3.6), 0.004 * wave(1.8)),
            Part::Body | Part::Head => (0.014 * wave(3.6), 0.0),
            Part::Edge => (0.012 * wave(3.6), 0.0),
            _ => (0.0, 0.0),
        },
        "breeze" => match p {
            // Gentle and irregular: two slow waves out of step.
            Part::Top => (0.007 * wave(4.7) + 0.003 * wave(1.9), 0.0015 * wave(3.1)),
            Part::Edge => (0.004 * wave(5.3) + 0.0015 * wave(2.3), 0.0),
            Part::Head | Part::Body => (0.003 * wave(4.7), 0.0),
            _ => (0.0, 0.0),
        },
        _ => (0.0, 0.0),
    }
}

/// When feet (or a body) meet the ground during `act` over a window of
/// `dur` seconds: `(seconds into the window, how hard 0..1)`, from the
/// same tempo and timing [`keys`] moves the pins with, so a sound placed
/// there lands on the frame the foot does. Skips the eased first and last
/// moments where the act settles.
pub(in crate::agent) fn contacts(act: &str, dur: f64, speed: f64, seed: u64) -> Vec<(f64, f32)> {
    let (tempo, _, phase) = variation(seed, "front_foot");
    let speed = speed.max(0.05);
    // Contacts at x = (t·tempo + phase)·speed / period ∈ k·step + at.
    let (period, step, at, hard): (f64, f64, f64, f32) = match act {
        "walk" => (0.9, 0.5, 0.0, 0.6),
        "run" => (0.5, 0.5, 0.0, 1.0),
        "sneak" => (1.4, 0.5, 0.0, 0.25),
        "hop" | "celebrate" => (0.8, 1.0, 0.6, 0.9),
        "swim" => (1.1, 1.0, 0.25, 0.5),
        "peck" => (1.2, 1.0, 0.35, 0.3),
        _ => return Vec::new(),
    };
    let x0 = phase * speed / period;
    let x1 = (dur * tempo + phase) * speed / period;
    let mut out = Vec::new();
    let mut k = ((x0 - at) / step).ceil() as i64;
    loop {
        let x = k as f64 * step + at;
        if x > x1 {
            break;
        }
        let t = (x * period / speed - phase) / tempo;
        if t >= 0.2 && t <= dur - 0.1 {
            // Every other footfall a touch softer, as feet differ.
            let w = if step < 1.0 && k % 2 != 0 { hard * 0.85 } else { hard };
            out.push((t, w));
        }
        k += 1;
    }
    out
}

/// A small, steady difference per character (and per pin) so two
/// characters doing the same act never move in step: its own tempo, size
/// and timing.
fn variation(seed: u64, name: &str) -> (f64, f64, f64) {
    let mut h = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ 0x2545_f491_4f6c_dd1d;
    let mut next = || {
        h ^= h << 13;
        h ^= h >> 7;
        h ^= h << 17;
        (h >> 11) as f64 / (1u64 << 53) as f64
    };
    let tempo = 0.88 + 0.24 * next();
    let size = 0.85 + 0.3 * next();
    let mut phase = 3.0 * next();
    // Pins of a backdrop each sway in their own time.
    let pin: u64 = name.bytes().fold(0u64, |a, b| a.wrapping_mul(31).wrapping_add(b as u64));
    if matches!(part(name), Part::Top | Part::Edge) {
        phase += (pin % 97) as f64 / 97.0 * 2.0;
    }
    (tempo, size, phase)
}

/// Keys for one pin over `[from, from + dur]` seconds (clip time): `rest`
/// moved by the summed acts, eased in and out of rest at the ends. `right`
/// is which way the picture faces; `aspect` is the layer's width over its
/// height; `seed` (the clip) sets the character's own tempo and timing.
/// Points are `{"u", "v"}`.
#[allow(clippy::too_many_arguments)]
pub(super) fn keys(acts: &[String], name: &str, rest: (f64, f64), from: f64, dur: f64, right: bool, aspect: f64, speed: f64, seed: u64) -> Option<Vec<(f64, f64, f64)>> {
    let rate = if dur * 12.0 > 1200.0 { 1200.0 / dur } else { 12.0 };
    let n = (dur * rate).ceil().max(2.0) as usize;
    let fwd = if right { 1.0 } else { -1.0 };
    let (tempo, size, phase) = variation(seed, name);
    let mut out = Vec::with_capacity(n + 1);
    let mut moves = false;
    for i in 0..=n {
        let t = dur * i as f64 / n as f64;
        let ease = (t / 0.35).min((dur - t) / 0.35).clamp(0.0, 1.0);
        let ease = ease * ease * (3.0 - 2.0 * ease);
        let (mut df, mut du) = (0.0, 0.0);
        for a in acts {
            let at = if once(a) { t } else { t * tempo + phase };
            let (f, u) = offset(a, name, at, speed);
            df += f * size;
            du += u * size;
        }
        moves |= df.abs() + du.abs() > 1e-6;
        out.push((from + t, rest.0 + ease * fwd * df / aspect.max(0.05), rest.1 - ease * du));
    }
    moves.then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A side-view "bird" facing right: a tall head at the right, a body,
    /// and a low tail at the left, on transparency.
    fn bird() -> Shape {
        let (w, h) = (60, 40);
        let mut a = vec![0u8; w * h];
        let mut fill = |x0: usize, y0: usize, x1: usize, y1: usize| {
            for y in y0..y1 {
                for x in x0..x1 {
                    a[y * w + x] = 255;
                }
            }
        };
        fill(15, 15, 45, 30); // body
        fill(38, 3, 50, 16); // head
        fill(4, 14, 16, 22); // tail
        fill(28, 30, 31, 38); // leg
        Shape::from_coverage(w, h, &a, false, "test").unwrap()
    }

    #[test]
    fn a_shape_tells_which_way_it_faces_and_rigs_onto_itself() {
        let s = bird();
        assert_eq!(s.facing(), Some(true));
        let pins = rig("bird", &s, true).unwrap();
        let at = |n: &str| {
            let p = pins.iter().find(|p| p["name"] == n).unwrap_or_else(|| panic!("no {n} in {pins:?}"));
            (p["at"]["u"].as_f64().unwrap(), p["at"]["v"].as_f64().unwrap())
        };
        assert!(at("head").0 > 0.6 && at("head").1 < 0.4, "{:?}", at("head"));
        assert!(at("tail").0 < 0.3, "{:?}", at("tail"));
        assert!(at("feet").1 > 0.8, "{:?}", at("feet"));
        // Facing left mirrors the rig: the head is looked for at the left,
        // and this bird has no head there.
        let left = rig("bird", &s, false).unwrap();
        assert!(left.iter().find(|p| p["name"] == "head").unwrap()["at"]["u"].as_f64().unwrap() < 0.5);
    }

    #[test]
    fn legs_get_hips_and_knees_and_characters_differ() {
        let pins = rig("bird", &bird(), true).unwrap();
        let names: Vec<&str> = pins.iter().map(|p| p["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"hip") && names.contains(&"knee"), "{names:?}");
        let v = |n: &str| pins.iter().find(|p| p["name"] == n).unwrap()["at"]["v"].as_f64().unwrap();
        assert!(v("hip") < v("knee") && v("knee") < v("feet"));
        // A step bends the knee less than the foot moves.
        let walk = vec!["walk".to_owned()];
        let foot = keys(&walk, "front_foot", (0.6, 0.95), 0.0, 2.0, true, 1.0, 1.0, 1).unwrap();
        let knee = keys(&walk, "front_knee", (0.6, 0.8), 0.0, 2.0, true, 1.0, 1.0, 1).unwrap();
        let reach = |k: &[(f64, f64, f64)], r: f64| k.iter().map(|p| (p.1 - r).abs()).fold(0.0, f64::max);
        assert!(reach(&knee, 0.6) < reach(&foot, 0.6) && reach(&knee, 0.6) > 0.0);
        // The same act on two characters is not in step.
        let a = keys(&walk, "head", (0.5, 0.2), 0.0, 2.0, true, 1.0, 1.0, 112).unwrap();
        let b = keys(&walk, "head", (0.5, 0.2), 0.0, 2.0, true, 1.0, 1.0, 147).unwrap();
        assert!(a.iter().zip(&b).any(|(x, y)| (x.2 - y.2).abs() > 1e-4));
        for act in ACTS.split(&[',', ';', ' '][..]).filter(|w| known_act(w)) {
            assert!(keys(&[act.to_owned()], "head", (0.5, 0.2), 0.0, 2.0, true, 1.0, 1.0, 5).is_some() || ["wag", "swim", "breeze", "point", "sway"].contains(&act) || act == "point", "{act} moves no head");
        }
        let bd = backdrop();
        assert!(keys(&["breeze".to_owned()], "top left", (0.12, 0.1), 0.0, 4.0, true, 1.7, 1.0, 9).is_some());
        assert!(keys(&["breeze".to_owned()], "base left", (0.1, 0.95), 0.0, 4.0, true, 1.7, 1.0, 9).is_none());
        assert_eq!(bd.len(), 11);
    }

    #[test]
    fn feet_meet_the_ground_where_their_keys_do() {
        let steps = contacts("walk", 6.0, 1.0, 9);
        // About two steps a 0.9 s stride.
        assert!((10..=16).contains(&steps.len()), "{steps:?}");
        let (tempo, _, phase) = variation(9, "front_foot");
        let lift = |name: &str, t: f64| offset("walk", name, t * tempo + phase, 1.0).1;
        for w in steps.windows(2) {
            let (a, b) = (w[0].0, w[1].0);
            // Down on the contact; between two, one foot is in the air.
            assert!(lift("front_foot", a).abs() < 1e-9 && lift("back_foot", a).abs() < 1e-9, "at {a}");
            let mid = 0.5 * (a + b);
            assert!(lift("front_foot", mid) + lift("back_foot", mid) > 0.02, "between {a} and {b}");
        }
        assert!(contacts("run", 6.0, 1.0, 9).len() > steps.len());
        assert!(contacts("wave", 6.0, 1.0, 9).is_empty());
        assert!(contacts("hop", 4.0, 1.0, 9).iter().all(|(_, w)| *w > 0.8));
    }

    #[test]
    fn a_crop_rigs_only_what_it_leaves() {
        // Two birds side by side; the crop keeps the right-hand one.
        let one = bird();
        let (w, h) = (one.w * 2, one.h);
        let mut a = vec![0u8; w * h];
        for y in 0..h {
            for x in 0..one.w {
                if one.at(x, y) {
                    a[y * w + x] = 255;
                    a[y * w + one.w + x] = 255;
                }
            }
        }
        let both = Shape::from_coverage(w, h, &a, false, "test").unwrap();
        let right = both.cropped(0.5, 0.0, 0.0, 0.0).unwrap();
        assert_eq!(right.bounds(), one.bounds());
        assert_eq!(right.facing(), Some(true));
    }

    #[test]
    fn a_plain_backdrop_is_keyed_out() {
        let (w, h) = (20, 20);
        let mut px = vec![[240u8, 240, 230]; w * h];
        for y in 5..15 {
            for x in 8..12 {
                px[y * w + x] = [200, 40, 20];
            }
        }
        let s = Shape::keyed(w, h, &px).unwrap();
        assert_eq!(s.bounds(), (8, 5, 11, 14));
    }

    #[test]
    fn acts_move_the_right_parts_toward_the_front_and_settle() {
        let acts = vec!["peck".to_owned()];
        let k = keys(&acts, "head", (0.8, 0.2), 1.0, 3.0, true, 1.5, 1.0, 3).unwrap();
        assert_eq!(k.first().unwrap().0, 1.0);
        assert!((k.first().unwrap().1 - 0.8).abs() < 1e-9 && (k.last().unwrap().2 - 0.2).abs() < 1e-9, "starts and ends at rest");
        assert!(k.iter().any(|p| p.1 > 0.81 && p.2 > 0.25), "pecks forward (right) and down");
        let l = keys(&acts, "head", (0.2, 0.2), 1.0, 3.0, false, 1.5, 1.0, 3).unwrap();
        assert!(l.iter().any(|p| p.1 < 0.19), "facing left pecks left");
        assert!(keys(&acts, "feet", (0.5, 0.9), 0.0, 3.0, true, 1.5, 1.0, 3).is_none(), "feet hold");
        let walk = vec!["walk".to_owned()];
        let a = keys(&walk, "front_foot", (0.6, 0.95), 0.0, 2.0, true, 1.0, 1.0, 3).unwrap();
        let b = keys(&walk, "back_foot", (0.4, 0.95), 0.0, 2.0, true, 1.0, 1.0, 3).unwrap();
        let lifted = |k: &[(f64, f64, f64)], i: usize| k[i].2 < 0.94;
        assert!((0..a.len()).any(|i| lifted(&a, i) && !lifted(&b, i)), "feet step in turn");
    }
}
