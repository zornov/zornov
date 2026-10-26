// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Character rigging on bones, shared by the Inspector, the canvas bone
//! tool and the AI: skeletons built from a character's silhouette
//! ([`template`]), what each bone is for ([`role`]), motions baked into
//! bone keys ([`act_keys`]: walk and run cycles, gestures, tails, swims),
//! and the split of a character picture into part pictures, one per bone
//! ([`split`]).
//!
//! Everything works in the layer's own 0..1 coordinates, like the
//! skeleton (`studio_model::Skeleton`), so it follows the layer when it is
//! moved, scaled or flipped. "Front" is the side the character faces in
//! its picture; a front view counts as facing right.

use std::collections::BTreeMap;
use std::f64::consts::TAU;
use studio_model as m;

// ------------------------------------------------------------ silhouette

/// Which pixels of a picture are the character, on a grid of at most
/// about 256 cells across.
#[derive(Clone, Debug)]
pub struct Silhouette {
    pub w: usize,
    pub h: usize,
    on: Vec<bool>,
    /// How it was found, for reports.
    pub from: &'static str,
}

impl Silhouette {
    /// From 8-bit coverage (alpha or a matte), on above half.
    pub fn from_coverage(w: usize, h: usize, cover: &[u8], invert: bool, from: &'static str) -> Option<Silhouette> {
        if w == 0 || h == 0 || cover.len() < w * h {
            return None;
        }
        let step = (w.max(h) / 256).max(1);
        let (sw, sh) = (w.div_ceil(step), h.div_ceil(step));
        let mut on = Vec::with_capacity(sw * sh);
        for y in (0..h).step_by(step) {
            for x in (0..w).step_by(step) {
                on.push((cover[y * w + x] >= 128) != invert);
            }
        }
        let n = on.iter().filter(|&&b| b).count();
        (n > 16 && n < sw * sh * 98 / 100).then_some(Silhouette { w: sw, h: sh, on, from })
    }

    /// A picture's transparency, or what differs from the plain colour
    /// around its border when it has none (a character on a backdrop).
    pub fn of_rgba(w: usize, h: usize, rgba: &[u8]) -> Option<Silhouette> {
        if rgba.len() < w * h * 4 {
            return None;
        }
        let alpha: Vec<u8> = rgba.chunks_exact(4).map(|p| p[3]).collect();
        if alpha.iter().any(|&a| a < 128) {
            return Silhouette::from_coverage(w, h, &alpha, false, "the picture's transparency");
        }
        let at = |x: usize, y: usize| -> [u8; 3] {
            let i = (y * w + x) * 4;
            [rgba[i], rgba[i + 1], rgba[i + 2]]
        };
        let mut border = Vec::new();
        for x in 0..w {
            border.push(at(x, 0));
            border.push(at(x, h - 1));
        }
        for y in 0..h {
            border.push(at(0, y));
            border.push(at(w - 1, y));
        }
        let mut bg = [0u8; 3];
        for c in 0..3 {
            let mut v: Vec<u8> = border.iter().map(|p| p[c]).collect();
            v.sort_unstable();
            bg[c] = v[v.len() / 2];
        }
        let far = |p: [u8; 3]| (0..3).map(|c| (p[c] as i32 - bg[c] as i32).pow(2)).sum::<i32>() > 40 * 40;
        if border.iter().filter(|p| !far(**p)).count() * 10 < border.len() * 8 {
            return None;
        }
        let cover: Vec<u8> = (0..w * h).map(|i| if far(at(i % w, i / w)) { 255 } else { 0 }).collect();
        Silhouette::from_coverage(w, h, &cover, false, "the picture's backdrop colour")
    }

    /// The part a crop leaves (fractions cut from each side): what the
    /// layer's 0..1 coordinates span.
    pub fn cropped(self, l: f64, t: f64, r: f64, b: f64) -> Option<Silhouette> {
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
        Silhouette::from_coverage(x1 - x0, y1 - y0, &cover, false, self.from)
    }

    pub fn at(&self, x: usize, y: usize) -> bool {
        self.on[y * self.w + x]
    }

    fn uv(&self, x: f64, y: f64) -> m::Vec2 {
        m::Vec2::new((x + 0.5) / self.w as f64, (y + 0.5) / self.h as f64)
    }

    /// The box the character fills, in cells.
    pub fn bounds(&self) -> (usize, usize, usize, usize) {
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
        let (mut sx, mut sy, mut n) = (0.0, 0.0, 0.0);
        for y in 0..self.h {
            for x in 0..self.w {
                if self.at(x, y) {
                    sx += x as f64;
                    sy += y as f64;
                    n += 1.0;
                }
            }
        }
        (sx / f64::max(n, 1.0), sy / f64::max(n, 1.0))
    }

    /// The cell of the shape nearest `(x, y)`.
    fn snap(&self, x: f64, y: f64) -> (f64, f64) {
        let (cx, cy) = (x.round().clamp(0.0, self.w as f64 - 1.0) as usize, y.round().clamp(0.0, self.h as f64 - 1.0) as usize);
        if self.at(cx, cy) {
            return (cx as f64, cy as f64);
        }
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
        (best.0 as f64, best.1 as f64)
    }

    /// The cell furthest along `dir` among columns `cols` and rows `rows`
    /// (fractions of the box), moved back inside by `inset` of the box.
    fn extreme(&self, cols: (f64, f64), rows: (f64, f64), dir: (f64, f64), inset: f64) -> Option<(f64, f64)> {
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
        let i = inset * bw.max(bh);
        Some(self.snap(x as f64 - dir.0 * i, y as f64 - dir.1 * i))
    }

    /// Cells of row `y` that are on, as runs `(start, end)`.
    fn runs(&self, y: usize) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        let mut start = None;
        for x in 0..=self.w {
            let on = x < self.w && self.at(x, y);
            match (on, start) {
                (true, None) => start = Some(x),
                (false, Some(s)) => {
                    out.push((s, x));
                    start = None;
                }
                _ => {}
            }
        }
        out
    }

    /// Which way a side view faces (heads lead); `None` for a front view.
    pub fn facing(&self) -> Option<bool> {
        let (x0, y0, x1, y1) = self.bounds();
        let (cx, _) = self.centroid();
        let band = y0 + ((y1 - y0) as f64 * 0.3) as usize;
        let (mut s, mut n) = (0.0, 0.0);
        for y in y0..=band {
            for x in x0..=x1 {
                if self.at(x, y) {
                    s += x as f64;
                    n += 1.0;
                }
            }
        }
        let lean = (s / f64::max(n, 1.0) - cx) / ((x1 - x0) as f64).max(1.0);
        (lean.abs() > 0.04).then_some(lean > 0.0)
    }

    /// Distance (cells) from each on cell to the nearest off cell.
    fn depth_map(&self) -> Vec<f32> {
        let mut d = vec![f32::INFINITY; self.w * self.h];
        for y in 0..self.h {
            for x in 0..self.w {
                if !self.at(x, y) {
                    d[y * self.w + x] = 0.0;
                }
            }
        }
        // Two chamfer passes.
        for pass in 0..2 {
            let ys: Vec<usize> = if pass == 0 { (0..self.h).collect() } else { (0..self.h).rev().collect() };
            for &y in &ys {
                let xs: Vec<usize> = if pass == 0 { (0..self.w).collect() } else { (0..self.w).rev().collect() };
                for &x in &xs {
                    let i = y * self.w + x;
                    let mut v = d[i];
                    let sgn: i64 = if pass == 0 { -1 } else { 1 };
                    for (dx, dy, c) in [(sgn, 0, 1.0f32), (0, sgn, 1.0), (sgn, sgn, 1.414), (-sgn, sgn, 1.414)] {
                        let (nx, ny) = (x as i64 + dx, y as i64 + dy);
                        let n = if nx < 0 || ny < 0 || nx >= self.w as i64 || ny >= self.h as i64 { 0.0 } else { d[ny as usize * self.w + nx as usize] };
                        v = v.min(n + c);
                    }
                    d[i] = v;
                }
            }
        }
        d
    }
}

// ------------------------------------------------------------ templates

pub const KINDS: &str = "biped (person, standing animal), quadruped (dog, cat, horse), bird, fish, snake (tail, tentacle, worm), plant (tree, flame, flower), object";

/// The kind a word names, if it is one.
pub fn kind_of(word: &str) -> Option<&'static str> {
    Some(match word.to_lowercase().as_str() {
        "biped" | "person" | "human" | "character" | "man" | "woman" | "child" | "robot" => "biped",
        "quadruped" | "animal" | "dog" | "cat" | "horse" | "cow" | "fox" | "wolf" | "lion" | "deer" => "quadruped",
        "bird" | "rooster" | "chicken" | "duck" | "owl" => "bird",
        "fish" | "shark" | "whale" | "dolphin" => "fish",
        "snake" | "tail" | "tentacle" | "worm" | "eel" | "rope" => "snake",
        "plant" | "tree" | "flame" | "flower" | "grass" | "fire" => "plant",
        "object" | "thing" | "prop" => "object",
        _ => return None,
    })
}

/// A skeleton for a kind of character, from its silhouette, facing right
/// when `right`. `aspect` is the picture's width over its height (pixels,
/// after crop).
pub fn template(kind: &str, s: &Silhouette, right: bool, aspect: f64) -> Result<m::Skeleton, String> {
    let kind = kind_of(kind).ok_or_else(|| format!("skeleton kind \"{kind}\": {KINDS}"))?;
    let (x0, y0, x1, y1) = s.bounds();
    let (bw, bh) = ((x1 - x0) as f64, (y1 - y0) as f64);
    if bw < 4.0 || bh < 4.0 {
        return Err("the silhouette is too small to rig".into());
    }
    // Box fractions → cells, mirrored for a character facing left.
    let fx = |a: f64| if right { a } else { 1.0 - a };
    let front = |a: f64, b: f64| if right { (1.0 - b, 1.0 - a) } else { (a, b) };
    let fwd = if right { 1.0 } else { -1.0 };
    let place = |u: f64, v: f64| s.snap(x0 as f64 + fx(u) * bw, y0 as f64 + v * bh);
    let uv = |p: (f64, f64)| s.uv(p.0, p.1);
    let mid = |a: (f64, f64), b: (f64, f64), k: f64| s.snap(a.0 + (b.0 - a.0) * k, a.1 + (b.1 - a.1) * k);
    let mut sk = m::Skeleton { aspect: aspect.max(0.05), ..m::Skeleton::default() };
    let add = |sk: &mut m::Skeleton, name: &str, parent: Option<u32>, a: (f64, f64), b: (f64, f64)| -> u32 {
        let (mut ha, hb) = (uv(a), uv(b));
        if (ha.x - hb.x).abs() + (ha.y - hb.y).abs() < 1e-3 {
            ha.y += 0.01; // never a bone of no length
        }
        sk.add(name, parent, ha, hb)
    };
    match kind {
        "biped" => {
            let top = s.extreme((0.25, 0.75), (0.0, 0.3), (0.0, -1.0), 0.02).ok_or("no head at the top of the silhouette")?;
            // The neck: the narrowest row in the upper body.
            let width = |y: usize| s.runs(y).iter().map(|r| r.1 - r.0).max().unwrap_or(0);
            let typical = {
                let mut v: Vec<usize> = ((y0 + (0.4 * bh) as usize)..=(y0 + (0.6 * bh) as usize).min(s.h - 1)).map(width).collect();
                v.sort_unstable();
                v.get(v.len() / 2).copied().unwrap_or(1)
            };
            // The head starts at the first row about half the body's
            // width (ears, horns and hair above it are narrower still).
            let head_top = (y0..=y1).find(|&y| width(y) * 2 >= typical).unwrap_or(y0);
            // The neck is the narrowest row below the head's widest one (a
            // round head is narrow at its crown too).
            let cheeks = (head_top..=(head_top + (0.25 * bh) as usize).min(s.h - 1)).max_by_key(|&y| width(y)).unwrap_or(head_top);
            let (ya, yb) = (cheeks + 1, y0 + (0.5 * bh) as usize);
            let rows: Vec<usize> = (ya..=yb.min(s.h - 1)).collect();
            let narrow = rows.iter().copied().min_by_key(|&y| width(y));
            let neck_v = match narrow {
                Some(y) if (width(y) as f64) < 0.8 * typical as f64 => (y - y0) as f64 / bh,
                _ => 0.22,
            };
            // The hips: just above where the legs part.
            let (cx, _) = s.centroid();
            let mut crotch = None;
            for y in ((y0 + (0.4 * bh) as usize)..=(y0 + (0.92 * bh) as usize).min(s.h - 1)).rev() {
                let runs = s.runs(y);
                let parted = runs.len() >= 2 && runs.windows(2).any(|w| (w[0].1 as f64) <= cx + 1.0 && (w[1].0 as f64) >= cx - 1.0);
                if !parted {
                    crotch = Some(y);
                    break;
                }
            }
            let hip_v = crotch.map_or(0.55, |y| ((y - y0) as f64 / bh - 0.04).clamp(0.4, 0.75));
            // The body's axis: the median centre of the run nearest the
            // centre of mass, row by row from neck to hips. Arms held out
            // widen the box but not this, so the spine stays in the torso.
            let axis = {
                let (ya, yb) = ((y0 as f64 + neck_v * bh) as usize, (y0 as f64 + hip_v * bh) as usize);
                let mut centres: Vec<f64> = (ya..=yb.min(s.h - 1))
                    .filter_map(|y| s.runs(y).into_iter().min_by(|a, b| (((a.0 + a.1) as f64 / 2.0 - cx).abs()).total_cmp(&(((b.0 + b.1) as f64 / 2.0 - cx).abs()))))
                    .map(|r| (r.0 + r.1) as f64 / 2.0)
                    .collect();
                centres.sort_by(f64::total_cmp);
                centres.get(centres.len() / 2).copied().unwrap_or(x0 as f64 + 0.5 * bw)
            };
            let neck = s.snap(axis, y0 as f64 + neck_v * bh);
            let pelvis = s.snap(axis, y0 as f64 + hip_v * bh);
            let shoulder_v = (neck_v + 0.07).min(hip_v - 0.1);
            let hand_f = s.extreme(front(0.0, 0.3), (0.25, 0.75), (fwd, 0.0), 0.03).unwrap_or(place(0.8, 0.5));
            let hand_b = s.extreme(front(0.7, 1.0), (0.25, 0.75), (-fwd, 0.0), 0.03).unwrap_or(place(0.2, 0.5));
            let foot_f = s.extreme(front(0.0, 0.5), (0.8, 1.0), (0.0, 1.0), 0.03).unwrap_or(place(0.6, 0.97));
            let foot_b = s.extreme(front(0.5, 1.0), (0.8, 1.0), (0.0, 1.0), 0.03).unwrap_or(place(0.4, 0.97));
            let hips = add(&mut sk, "hips", None, pelvis, mid(pelvis, neck, 0.12));
            let spine = add(&mut sk, "spine", Some(hips), pelvis, neck);
            add(&mut sk, "head", Some(spine), neck, top);
            for (side, hand, foot, depth) in [("front", hand_f, foot_f, 0.5), ("back", hand_b, foot_b, -0.5)] {
                let shoulder = s.snap(neck.0 + (hand.0 - neck.0) * 0.25, y0 as f64 + shoulder_v * bh);
                let elbow = mid(shoulder, hand, 0.5);
                let up = add(&mut sk, &format!("upper_arm.{side}"), Some(spine), shoulder, elbow);
                let fore = add(&mut sk, &format!("forearm.{side}"), Some(up), elbow, hand);
                let hip = s.snap(pelvis.0 + (foot.0 - pelvis.0) * 0.6, pelvis.1);
                let knee = mid(hip, foot, 0.5);
                let th = add(&mut sk, &format!("thigh.{side}"), Some(hips), hip, knee);
                let sh = add(&mut sk, &format!("shin.{side}"), Some(th), knee, foot);
                for id in [up, fore, th, sh] {
                    if let Some(b) = sk.bone_mut(id) {
                        b.depth = depth;
                    }
                }
                handle(&mut sk, &format!("hand.{side}"), fore);
                handle(&mut sk, &format!("foot.{side}"), sh);
            }
        }
        "quadruped" => {
            let nose = s.extreme(front(0.0, 0.3), (0.0, 0.65), (fwd, 0.0), 0.02).ok_or("no head at the front of the silhouette")?;
            let crown = s.extreme(front(0.0, 0.4), (0.0, 0.6), (0.0, -1.0), 0.02).unwrap_or(nose);
            let tail = s.extreme(front(0.82, 1.0), (0.0, 0.75), (-fwd, 0.0), 0.02);
            let withers = place(0.68, 0.4);
            let rump = place(0.22, 0.4);
            let head_base = mid(withers, crown, 0.65);
            let hips = add(&mut sk, "hips", None, rump, mid(rump, withers, 0.15));
            let spine = add(&mut sk, "spine", Some(hips), rump, withers);
            let neck = add(&mut sk, "neck", Some(spine), withers, head_base);
            add(&mut sk, "head", Some(neck), head_base, nose);
            if let Some(t) = tail {
                let t1 = mid(rump, t, 0.5);
                let a = add(&mut sk, "tail.1", Some(hips), rump, t1);
                add(&mut sk, "tail.2", Some(a), t1, t);
            }
            let ff = s.extreme(front(0.0, 0.42), (0.8, 1.0), (0.0, 1.0), 0.03).unwrap_or(place(0.7, 0.97));
            let bf = s.extreme(front(0.58, 1.0), (0.8, 1.0), (0.0, 1.0), 0.03).unwrap_or(place(0.25, 0.97));
            for (leg, top, foot) in [("front_leg", withers, ff), ("back_leg", rump, bf)] {
                let shoulder = s.snap(foot.0 + (top.0 - foot.0) * 0.2, top.1 + (foot.1 - top.1) * 0.25);
                let knee = mid(shoulder, foot, 0.55);
                let parent = if leg == "front_leg" { spine } else { hips };
                let up = add(&mut sk, &format!("{leg}.upper"), Some(parent), shoulder, knee);
                let low = add(&mut sk, &format!("{leg}.lower"), Some(up), knee, foot);
                handle(&mut sk, &format!("{leg} foot"), low);
            }
        }
        "bird" => {
            let (cx, cy) = s.centroid();
            let body = s.snap(cx, cy);
            let top = s.extreme(front(0.0, 0.5), (0.0, 0.45), (0.0, -1.0), 0.02).ok_or("no head at the top of the silhouette")?;
            let tail = s.extreme(front(0.75, 1.0), (0.0, 0.8), (-fwd, 0.0), 0.02);
            let feet = s.extreme((0.25, 0.75), (0.85, 1.0), (0.0, 1.0), 0.02);
            let neck_at = mid(body, top, 0.5);
            let hips = add(&mut sk, "hips", None, body, mid(body, neck_at, 0.3));
            let neck = add(&mut sk, "neck", Some(hips), body, neck_at);
            add(&mut sk, "head", Some(neck), neck_at, top);
            if let Some(t) = tail {
                add(&mut sk, "tail.1", Some(hips), body, t);
            }
            if let Some(f) = feet {
                let hip = s.snap(f.0, body.1 + (f.1 - body.1) * 0.35);
                let knee = mid(hip, f, 0.5);
                let th = add(&mut sk, "thigh.front", Some(hips), hip, knee);
                let sh = add(&mut sk, "shin.front", Some(th), knee, f);
                handle(&mut sk, "foot.front", sh);
            }
        }
        "fish" => {
            let (cx, cy) = s.centroid();
            let body = s.snap(cx, cy);
            let head = s.extreme(front(0.0, 0.3), (0.15, 0.85), (fwd, 0.0), 0.02).ok_or("no head at the front")?;
            let tail = s.extreme(front(0.75, 1.0), (0.05, 0.95), (-fwd, 0.0), 0.02).ok_or("no tail at the back")?;
            let root = add(&mut sk, "body", None, body, mid(body, head, 0.4));
            add(&mut sk, "head", Some(root), mid(body, head, 0.4), head);
            let mut parent = root;
            let mut at = body;
            for i in 1..=3 {
                let next = mid(body, tail, i as f64 / 3.0);
                parent = add(&mut sk, &format!("tail.{i}"), Some(parent), at, next);
                at = next;
            }
        }
        "snake" | "plant" | "object" => {
            // A chain along the shape's long axis, from its base (the bottom
            // for plants and objects, the back for a snake).
            let n = match kind {
                "snake" => 6,
                "plant" => 4,
                _ => 2,
            };
            let along_x = kind == "snake" && bw > bh;
            let mut pts = Vec::new();
            for i in 0..=n {
                let k = i as f64 / n as f64;
                let p = if along_x {
                    let x = x0 as f64 + bw * if right { k } else { 1.0 - k };
                    let col: Vec<usize> = (y0..=y1).filter(|&y| s.at((x.round() as usize).min(s.w - 1), y)).collect();
                    let y = if col.is_empty() { (y0 + y1) as f64 / 2.0 } else { col.iter().sum::<usize>() as f64 / col.len() as f64 };
                    s.snap(x, y)
                } else {
                    let y = y1 as f64 - bh * k;
                    let runs = s.runs((y.round() as usize).min(s.h - 1));
                    let x = runs.iter().max_by_key(|r| r.1 - r.0).map_or((x0 + x1) as f64 / 2.0, |r| (r.0 + r.1) as f64 / 2.0);
                    s.snap(x, y)
                };
                pts.push(p);
            }
            let name = match kind {
                "snake" => "spine",
                "plant" => "trunk",
                _ => "body",
            };
            let mut parent = None;
            for i in 0..n {
                parent = Some(add(&mut sk, &format!("{name}.{}", i + 1), parent, pts[i], pts[i + 1]));
            }
            if kind == "snake" {
                if let Some(end) = parent {
                    let mut h = handle_of(&sk, "tip", end);
                    h.chain = n as u32;
                    sk.ik.push(h);
                }
            }
        }
        _ => unreachable!(),
    }
    limits(&mut sk, right);
    // Reach of each bone's pull: about a limb's half-width.
    let d = s.depth_map();
    let mut th: Vec<f32> = d.iter().copied().filter(|v| v.is_finite() && *v > 0.0).collect();
    th.sort_by(f32::total_cmp);
    let typical = th.get(th.len() * 3 / 4).copied().unwrap_or(4.0) as f64;
    sk.falloff = (typical / s.w.min(s.h).max(1) as f64 * 0.6).clamp(0.015, 0.12);
    Ok(sk)
}

fn handle_of(sk: &m::Skeleton, name: &str, bone: u32) -> m::IkHandle {
    let target = sk.bone(bone).map_or(m::Vec2::ZERO, |b| b.tail);
    m::IkHandle { id: sk.next_ik_id(), name: name.into(), bone, chain: 2, target, blend: 1.0, flip: false, enabled: false, ..Default::default() }
}

/// An IK handle on `bone`'s tail, off until it is used.
fn handle(sk: &mut m::Skeleton, name: &str, bone: u32) {
    let h = handle_of(sk, name, bone);
    sk.ik.push(h);
}

/// Joint limits by role, the way the character faces.
fn limits(sk: &mut m::Skeleton, right: bool) {
    let rest: Vec<(u32, m::Vec2, m::Vec2)> = sk.bones.iter().map(|b| (b.id, b.head, b.tail)).collect();
    let aspect = sk.aspect;
    for b in &mut sk.bones {
        let (_, h, t) = rest.iter().copied().find(|r| r.0 == b.id).unwrap_or((0, b.head, b.tail));
        let dir = ((t.x - h.x) * aspect, t.y - h.y);
        // The sign that swings the tail toward the back (knees) or the
        // front (elbows).
        let back = toward(dir, if right { (-1.0, 0.0) } else { (1.0, 0.0) });
        let (lo, hi): (f64, f64) = match role(&b.name) {
            Role::Shin(_) => (-5.0, 150.0),
            Role::Forearm(_) => (-150.0, 5.0),
            Role::Head => (-50.0, 50.0),
            Role::Neck => (-60.0, 60.0),
            Role::Spine => (-45.0, 45.0),
            _ => continue,
        };
        // Shins bend back; forearms bend forward (the other way from back).
        let s = if back >= 0.0 { 1.0 } else { -1.0 };
        let (a, z) = (lo * s, hi * s);
        b.min = Some(a.min(z));
        b.max = Some(a.max(z));
    }
}

/// Which way a clockwise turn (positive degrees) moves a bone's tail
/// against direction `want` (`> 0`: toward it). `dir` is the bone in
/// pixels.
fn toward(dir: (f64, f64), want: (f64, f64)) -> f64 {
    // A clockwise turn moves the tail along (-dy, dx) (y down).
    let v = (-dir.1, dir.0);
    let len = (v.0 * v.0 + v.1 * v.1).sqrt().max(1e-9);
    (v.0 * want.0 + v.1 * want.1) / len
}

// ------------------------------------------------------------ roles

/// Front (near, the side the character faces) or back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Front,
    Back,
}

/// What a bone is for, from its name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Root,
    Spine,
    Neck,
    Head,
    UpperArm(Side),
    Forearm(Side),
    Thigh(Side),
    Shin(Side),
    /// A tail, counted from the body (1, 2, …).
    Tail(u32),
    /// A trunk, snake or object chain, counted from the base.
    Chain(u32),
    Other,
}

fn side(n: &str) -> Side {
    if n.contains("back") || n.contains("left") || n.contains(".l") || n.contains("far") { Side::Back } else { Side::Front }
}

fn index(n: &str) -> u32 {
    n.rsplit(|c: char| !c.is_ascii_digit()).next().and_then(|d| d.parse().ok()).unwrap_or(1)
}

/// A bone's role from its name (`thigh.front`, `front_leg.upper`,
/// `tail.2`…).
pub fn role(name: &str) -> Role {
    let n = name.to_lowercase();
    if n.starts_with("front_leg") || n.starts_with("foreleg") {
        return if n.contains("lower") || n.contains("2") { Role::Forearm(Side::Front) } else { Role::UpperArm(Side::Front) };
    }
    if n.starts_with("back_leg") || n.starts_with("hind_leg") || n.starts_with("hindleg") {
        return if n.contains("lower") || n.contains("2") { Role::Shin(Side::Front) } else { Role::Thigh(Side::Front) };
    }
    if n.contains("forearm") || n.contains("lower_arm") || n.contains("hand") || n.contains("wing.2") {
        Role::Forearm(side(&n))
    } else if n.contains("upper_arm") || n.contains("arm") || n.contains("wing") {
        Role::UpperArm(side(&n))
    } else if n.contains("shin") || n.contains("calf") || n.contains("lower_leg") || n.contains("foot") {
        Role::Shin(side(&n))
    } else if n.contains("thigh") || n.contains("upper_leg") || n.contains("leg") {
        Role::Thigh(side(&n))
    } else if n.contains("tail") || n.contains("fin") {
        Role::Tail(index(&n))
    } else if n.contains("head") || n.contains("skull") {
        Role::Head
    } else if n.contains("neck") {
        Role::Neck
    } else if n.contains("trunk") || n.starts_with("spine.") || n.starts_with("body.") || n.contains("stem") || n.contains("segment") {
        Role::Chain(index(&n))
    } else if n.contains("spine") || n.contains("chest") || n.contains("torso") {
        Role::Spine
    } else if n.contains("hips") || n.contains("pelvis") || n.contains("root") || n == "body" {
        Role::Root
    } else {
        Role::Other
    }
}

// ------------------------------------------------------------ acts

pub const ACTS: &str = "idle, breathe, walk, run, sneak, hop, jump, celebrate, wave, point, nod, shake_head, look, talk, listen, think, sad, scared, surprise, laugh, peck, wag (a tail), swim, slither, sway, breeze";

const ACT_NAMES: [&str; 26] = ["idle", "breathe", "walk", "run", "sneak", "hop", "jump", "celebrate", "wave", "point", "nod", "shake_head", "look", "talk", "listen", "think", "sad", "scared", "surprise", "laugh", "peck", "wag", "swim", "slither", "sway", "breeze"];

pub fn known_act(a: &str) -> bool {
    ACT_NAMES.contains(&a)
}

/// One bone's turn at a moment of an act: `(degrees, toward)`: how far
/// its tail swings toward a direction (`(forward, down)` units).
type Swing = (f64, (f64, f64));

const FWD: (f64, f64) = (1.0, 0.0);
const UP: (f64, f64) = (0.0, -1.0);
const NOD: (f64, f64) = (0.7, 0.7);

/// A gait: legs swinging about the hips in turn, knees bending most as a
/// leg passes under the body, arms against the legs, the body bobbing
/// twice a stride and leaning into it.
#[allow(clippy::too_many_arguments)]
fn gait(r: Role, t: f64, period: f64, swing: f64, knee: f64, arm: f64, elbow: f64, lean: f64) -> Swing {
    let ph = |s: Side| TAU * t / period + if s == Side::Front { 0.0 } else { std::f64::consts::PI };
    match r {
        Role::Thigh(s) => (swing * ph(s).sin(), FWD),
        // The knee folds back through the swing (cos > 0: the leg coming
        // forward), a little in stance.
        Role::Shin(s) => (-(6.0 + knee * ph(s).cos().max(0.0).powi(2) + 6.0 * (-ph(s).cos()).max(0.0)), FWD),
        Role::UpperArm(s) => (-arm * ph(s).sin(), FWD),
        Role::Forearm(s) => (elbow * (1.0 + 0.4 * (-ph(s).sin()).max(0.0)), FWD),
        Role::Spine => (lean, FWD),
        Role::Head | Role::Neck => (-0.4 * lean + 2.0 * (2.0 * TAU * t / period).sin(), FWD),
        Role::Tail(i) => (8.0 * (TAU * t / period - 0.5 * i as f64).sin(), UP),
        _ => (0.0, FWD),
    }
}

/// Every bone's swing in `act` at `t` seconds (tempo `speed`), and the
/// root's shift (forward, up) in heights of the picture.
fn act_at(act: &str, r: Role, t: f64, speed: f64) -> (Swing, (f64, f64)) {
    let t = t * speed;
    let wave = |period: f64| (TAU * t / period).sin();
    let none = (0.0, FWD);
    let pi = std::f64::consts::PI;
    match act {
        "breathe" | "idle" => {
            let b = wave(3.4);
            let idle = act == "idle";
            let s = match r {
                Role::Spine => (1.2 * b, FWD),
                Role::Head | Role::Neck => (if idle { 2.5 * wave(5.3) } else { -0.8 * b }, NOD),
                Role::UpperArm(_) => (1.5 * b, FWD),
                Role::Tail(i) => (if idle { 6.0 * (TAU * t / 2.9 - 0.4 * i as f64).sin() } else { 0.0 }, UP),
                Role::Chain(i) => (0.8 * i as f64 * wave(4.1), FWD),
                _ => none,
            };
            (s, (0.0, 0.004 * b))
        }
        "walk" => (gait(r, t, 1.0, 24.0, 48.0, 18.0, 14.0, 4.0), (0.0, 0.012 * (TAU * t / 1.0).cos().powi(2))),
        "run" => (gait(r, t, 0.6, 38.0, 85.0, 38.0, 70.0, 12.0), (0.0, 0.035 * (TAU * t / 0.6).cos().powi(2) - 0.01)),
        "sneak" => {
            let (s, _) = (gait(r, t, 1.6, 16.0, 40.0, 8.0, 35.0, 16.0), ());
            let s = match r {
                Role::Shin(_) => (s.0 - 25.0, s.1),
                Role::Thigh(_) => (s.0 + 18.0, s.1),
                _ => s,
            };
            (s, (0.0, -0.03))
        }
        "hop" | "jump" | "celebrate" => {
            let period = if act == "jump" { 1.4 } else { 0.8 };
            let ph = (t / period).fract();
            let air = if ph < 0.6 { (pi * ph / 0.6).sin() } else { 0.0 };
            let squash = if ph >= 0.6 { (pi * (ph - 0.6) / 0.4).sin() } else { 0.0 };
            let height = if act == "jump" { 0.3 } else { 0.14 };
            let s = match r {
                Role::Thigh(_) => (30.0 * air + 25.0 * squash, FWD),
                Role::Shin(_) => (-55.0 * air - 45.0 * squash, FWD),
                Role::UpperArm(_) if act == "celebrate" => (160.0 + 10.0 * wave(0.4), FWD),
                Role::Forearm(_) if act == "celebrate" => (15.0, FWD),
                Role::UpperArm(_) => (60.0 * air - 20.0 * squash, FWD),
                Role::Spine => (8.0 * squash, FWD),
                Role::Tail(_) => (-20.0 * air, UP),
                _ => none,
            };
            (s, (0.0, height * air - 0.04 * squash))
        }
        "wave" => (
            match r {
                Role::UpperArm(Side::Front) => (150.0, FWD),
                Role::Forearm(Side::Front) => (25.0 + 30.0 * wave(0.6), FWD),
                Role::Head => (3.0 * wave(1.2), NOD),
                _ => none,
            },
            (0.0, 0.0),
        ),
        "point" => (
            match r {
                Role::UpperArm(Side::Front) => (85.0, FWD),
                Role::Forearm(Side::Front) => (5.0, FWD),
                Role::Head => (-4.0, NOD),
                Role::Spine => (3.0, FWD),
                _ => none,
            },
            (0.0, 0.0),
        ),
        "nod" => (if r == Role::Head { (14.0 * wave(0.9).max(0.0), NOD) } else { none }, (0.0, 0.0)),
        "shake_head" | "look" => {
            let s = if act == "look" { 12.0 * wave(4.0) } else { 9.0 * wave(0.45) };
            (if r == Role::Head { (s, FWD) } else { none }, (0.0, 0.0))
        }
        "talk" => {
            let syl = (TAU * t * 1.7).sin().abs();
            (
                match r {
                    Role::Head => (5.0 * syl + 2.0 * wave(2.3), NOD),
                    Role::UpperArm(Side::Front) => (20.0 + 10.0 * wave(1.9), FWD),
                    Role::Forearm(Side::Front) => (40.0 + 20.0 * wave(1.9).max(0.0), FWD),
                    Role::Spine => (1.5 * syl, FWD),
                    _ => none,
                },
                (0.0, 0.0),
            )
        }
        "listen" => (
            match r {
                Role::Head => (8.0 + 4.0 * wave(2.6).max(0.0), NOD),
                Role::Spine => (4.0, FWD),
                _ => none,
            },
            (0.0, 0.0),
        ),
        "think" => (
            match r {
                Role::Head => (-8.0 + 2.0 * wave(3.1), NOD),
                Role::UpperArm(Side::Front) => (35.0, FWD),
                Role::Forearm(Side::Front) => (120.0, FWD),
                _ => none,
            },
            (0.0, 0.0),
        ),
        "sad" => (
            match r {
                Role::Head => (25.0 + 2.0 * wave(5.0), NOD),
                Role::Neck => (15.0, NOD),
                Role::Spine => (10.0, FWD),
                Role::UpperArm(_) => (-4.0, FWD),
                Role::Tail(_) => (-25.0, UP),
                _ => none,
            },
            (0.0, -0.01),
        ),
        "scared" => {
            let tr = 1.5 * (TAU * t * 11.0).sin();
            (
                match r {
                    Role::Head => (-12.0 + tr, NOD),
                    Role::Spine => (-8.0 + tr, FWD),
                    Role::UpperArm(_) => (70.0, FWD),
                    Role::Forearm(_) => (100.0, FWD),
                    Role::Thigh(_) => (12.0, FWD),
                    Role::Shin(_) => (-24.0, FWD),
                    Role::Tail(_) => (-35.0, UP),
                    _ => none,
                },
                (0.0, -0.025),
            )
        }
        "surprise" => {
            let k = if t < 0.3 { (pi * t / 0.6).sin() } else { (-(t - 0.9).max(0.0) / 0.8).exp() };
            (
                match r {
                    Role::Head => (-15.0 * k, NOD),
                    Role::Spine => (-10.0 * k, FWD),
                    Role::UpperArm(_) => (110.0 * k, FWD),
                    Role::Forearm(_) => (40.0 * k, FWD),
                    Role::Tail(_) => (30.0 * k, UP),
                    _ => none,
                },
                (0.0, 0.04 * k),
            )
        }
        "laugh" => {
            let b = (TAU * t * 4.0).sin().abs();
            (
                match r {
                    Role::Head => (-12.0 - 4.0 * b, NOD),
                    Role::Spine => (-4.0 + 3.0 * b, FWD),
                    Role::UpperArm(_) => (20.0, FWD),
                    Role::Forearm(_) => (70.0, FWD),
                    _ => none,
                },
                (0.0, 0.01 * b),
            )
        }
        "peck" => {
            let ph = (t / 1.2).fract();
            let k = if (0.2..0.5).contains(&ph) { (pi * (ph - 0.2) / 0.3).sin() } else { 0.0 };
            (
                match r {
                    Role::Neck => (55.0 * k, NOD),
                    Role::Head => (25.0 * k, NOD),
                    Role::Tail(_) => (10.0 * k, UP),
                    _ => none,
                },
                (0.0, 0.0),
            )
        }
        "wag" => (
            match r {
                Role::Tail(i) => (22.0 * (TAU * t / 0.5 - 0.7 * i as f64).sin(), UP),
                _ => none,
            },
            (0.0, 0.0),
        ),
        "swim" | "slither" => {
            let period = if act == "swim" { 1.0 } else { 1.4 };
            (
                match r {
                    Role::Tail(i) => (16.0 * (TAU * t / period - 0.8 * i as f64).sin(), UP),
                    Role::Chain(i) => (14.0 * (TAU * t / period - 0.9 * i as f64).sin(), UP),
                    Role::Head => (-4.0 * (TAU * t / period).sin(), UP),
                    Role::Root => (3.0 * (TAU * t / period + 1.0).sin(), UP),
                    _ => none,
                },
                (0.0, 0.0),
            )
        }
        "sway" | "breeze" => {
            let (amp, period) = if act == "sway" { (5.0, 3.6) } else { (2.0, 4.7) };
            (
                match r {
                    Role::Chain(i) => (amp * (0.6 + 0.4 * i as f64) * (TAU * t / period - 0.45 * i as f64).sin() + if act == "breeze" { 0.8 * wave(1.9) } else { 0.0 }, FWD),
                    Role::Spine => (0.5 * amp * wave(period), FWD),
                    Role::Head => (0.4 * amp * wave(period * 1.1), NOD),
                    _ => none,
                },
                (0.0, 0.0),
            )
        }
        _ => (none, (0.0, 0.0)),
    }
}

/// A small, steady difference per character (from `seed`) so two
/// characters doing the same act never move in step.
fn variation(seed: u64) -> (f64, f64, f64) {
    let mut h = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ 0x2545_f491_4f6c_dd1d;
    let mut next = || {
        h ^= h << 13;
        h ^= h >> 7;
        h ^= h << 17;
        (h >> 11) as f64 / (1u64 << 53) as f64
    };
    (0.9 + 0.2 * next(), 0.88 + 0.24 * next(), 2.0 * next())
}

/// Keys an act gives a skeleton.
#[derive(Clone, Debug, Default)]
pub struct ActKeys {
    /// Bone id → `(seconds, degrees)` (clip time).
    pub rotations: BTreeMap<u32, Vec<(f64, f64)>>,
    /// The root's offsets, `(seconds, offset)`.
    pub offsets: BTreeMap<u32, Vec<(f64, m::Vec2)>>,
    /// For a gait: how far the body travels per second to keep the feet
    /// from sliding, in heights of the picture.
    pub stride: Option<f64>,
}

/// Keys for `acts` (summed) over `[from, from + dur]` seconds of clip time
/// on top of the pose `base` (the skeleton as it stands at `from`),
/// eased in and out of it at the ends.
pub fn act_keys(acts: &[String], base: &m::Skeleton, from: f64, dur: f64, right: bool, speed: f64, seed: u64) -> ActKeys {
    let rate = if dur * 15.0 > 1500.0 { 1500.0 / dur } else { 15.0 };
    let n = (dur * rate).ceil().max(2.0) as usize;
    let (tempo, size, phase) = variation(seed);
    let fwd = if right { 1.0 } else { -1.0 };
    let aspect = base.aspect.max(1e-6);
    let mut out = ActKeys::default();
    let root = base.bones.iter().find(|b| b.parent.is_none()).map(|b| b.id);
    let once = |a: &str| a == "surprise";
    // Seen from the front, limbs swing toward the camera: on the picture
    // that is a small sway, not a stride.
    let front_view = base.faces_right.is_none();
    for b in &base.bones {
        let r = role(&b.name);
        let limb = matches!(r, Role::Thigh(_) | Role::Shin(_) | Role::UpperArm(_) | Role::Forearm(_));
        let gaits = acts.iter().any(|a| matches!(a.as_str(), "walk" | "run" | "sneak"));
        let size = if front_view && limb && gaits { size * 0.3 } else { size };
        let dir = ((b.tail.x - b.head.x) * aspect, b.tail.y - b.head.y);
        let mut keys = Vec::with_capacity(n + 1);
        let mut moves = false;
        for i in 0..=n {
            let t = dur * i as f64 / n as f64;
            let ease = (t / 0.35).min((dur - t) / 0.35).clamp(0.0, 1.0);
            let ease = ease * ease * (3.0 - 2.0 * ease);
            let mut deg = 0.0;
            for a in acts {
                let at = if once(a) { t } else { t * tempo + phase };
                let ((d, want), _) = act_at(a, r, at, speed);
                if d != 0.0 {
                    let s = toward(dir, (want.0 * fwd, want.1));
                    // A bone at right angles to the swing has no clear way:
                    // fall back on forward.
                    let s = if s.abs() < 0.2 { toward(dir, (fwd, 0.0)).signum().max(-1.0) } else { s.signum() };
                    deg += d * size * s;
                }
            }
            moves |= deg.abs() > 1e-4;
            keys.push((from + t, b.limit(b.rotation + ease * deg)));
        }
        if moves {
            out.rotations.insert(b.id, keys);
        }
    }
    if let Some(root) = root {
        let rb = base.bone(root).expect("root");
        let mut keys = Vec::with_capacity(n + 1);
        let mut moves = false;
        for i in 0..=n {
            let t = dur * i as f64 / n as f64;
            let ease = (t / 0.35).min((dur - t) / 0.35).clamp(0.0, 1.0);
            let ease = ease * ease * (3.0 - 2.0 * ease);
            let (mut f, mut u) = (0.0, 0.0);
            for a in acts {
                let at = if once(a) { t } else { t * tempo + phase };
                let (_, (df, du)) = act_at(a, Role::Root, at, speed);
                f += df * size;
                u += du * size;
            }
            moves |= f.abs() + u.abs() > 1e-6;
            keys.push((from + t, m::Vec2::new(rb.offset.x + ease * fwd * f / aspect, rb.offset.y - ease * u)));
        }
        if moves {
            out.offsets.insert(root, keys);
        }
    }
    // Feet that stay put: the body travels two steps a stride.
    let leg = |s: Side| -> f64 {
        base.bones.iter().filter(|b| matches!(role(&b.name), Role::Thigh(x) | Role::Shin(x) if x == s)).map(|b| (((b.tail.x - b.head.x) * aspect).powi(2) + (b.tail.y - b.head.y).powi(2)).sqrt()).sum()
    };
    let l = leg(Side::Front);
    if l > 0.0 {
        let (period, swing) = if acts.iter().any(|a| a == "run") {
            (0.6, 38.0f64)
        } else if acts.iter().any(|a| a == "sneak") {
            (1.6, 16.0)
        } else if acts.iter().any(|a| a == "walk") {
            (1.0, 24.0)
        } else {
            (0.0, 0.0)
        };
        if period > 0.0 {
            let step = 2.0 * l * swing.to_radians().sin() * size;
            out.stride = Some(2.0 * step / (period / (speed * tempo)));
        }
    }
    out
}

// ------------------------------------------------------------ parts

/// One part of a split character picture.
#[derive(Clone, Debug)]
pub struct Part {
    /// The bone it hangs on.
    pub bone: u32,
    /// What it is ("head", "upper_arm.front"…).
    pub name: String,
    /// The part's pixels on a picture the size of the whole (RGBA, the
    /// rest transparent), so it lines up with the character.
    pub rgba: Vec<u8>,
    /// How many pixels it has.
    pub pixels: usize,
    /// Pixels painted in behind other parts (`split_clean`): where this
    /// part is hidden by another, so lifting that one shows picture.
    pub filled: usize,
}

/// The search grid of a split: the picture at most 384 cells across, which
/// cells are the character, and the distance, through the character, from
/// each bone to every cell.
struct Grid {
    step: usize,
    gw: usize,
    gh: usize,
    on: Vec<bool>,
}

impl Grid {
    fn of(rgba: &[u8], w: usize, h: usize) -> Grid {
        let step = (w.max(h) / 384).max(1);
        let (gw, gh) = (w.div_ceil(step), h.div_ceil(step));
        let alpha = |x: usize, y: usize| rgba[(y * w + x) * 4 + 3];
        let on: Vec<bool> = (0..gw * gh).map(|i| alpha(((i % gw) * step).min(w - 1), ((i / gw) * step).min(h - 1)) >= 24).collect();
        Grid { step, gw, gh, on }
    }

    fn cell(&self, u: f64, v: f64) -> (usize, usize) {
        (((u * self.gw as f64).floor().max(0.0) as usize).min(self.gw - 1), ((v * self.gh as f64).floor().max(0.0) as usize).min(self.gh - 1))
    }

    /// Distance from bone `b` to each cell, through the character (off it
    /// costs a lot: gaps between legs are not shortcuts).
    fn field(&self, b: &m::Bone) -> Vec<f32> {
        use std::cmp::Reverse;
        use std::collections::BinaryHeap;
        let (gw, gh, on) = (self.gw, self.gh, &self.on);
        let mut d = vec![f32::INFINITY; gw * gh];
        let mut heap = BinaryHeap::new();
        let (a, z) = (self.cell(b.head.x, b.head.y), self.cell(b.tail.x, b.tail.y));
        let n = (a.0.abs_diff(z.0).max(a.1.abs_diff(z.1)) + 1).max(2);
        for k in 0..=n {
            let t = k as f64 / n as f64;
            let (x, y) = ((a.0 as f64 + (z.0 as f64 - a.0 as f64) * t).round() as usize, (a.1 as f64 + (z.1 as f64 - a.1 as f64) * t).round() as usize);
            let i = y.min(gh - 1) * gw + x.min(gw - 1);
            // Seeds off the character still start the search (thin limbs).
            d[i] = 0.0;
            heap.push(Reverse((0u32, i)));
        }
        while let Some(Reverse((du, u))) = heap.pop() {
            if du as f32 > d[u] {
                continue;
            }
            let (ux, uy) = ((u % gw) as i64, (u / gw) as i64);
            for (dx, dy, c) in [(1, 0, 10u32), (-1, 0, 10), (0, 1, 10), (0, -1, 10), (1, 1, 14), (1, -1, 14), (-1, 1, 14), (-1, -1, 14)] {
                let (vx, vy) = (ux + dx, uy + dy);
                if vx < 0 || vy < 0 || vx >= gw as i64 || vy >= gh as i64 {
                    continue;
                }
                let v = vy as usize * gw + vx as usize;
                let cost = if on[v] { c } else { c * 8 };
                let nd = du + cost;
                if (nd as f32) < d[v] {
                    d[v] = nd as f32;
                    heap.push(Reverse((nd, v)));
                }
            }
        }
        d
    }

    /// A field's value at pixel `(x, y)`, interpolated between cells so a
    /// boundary between two bones runs smoothly, not in cell-sized steps.
    fn at(&self, f: &[f32], x: usize, y: usize) -> f32 {
        let (gx, gy) = (x as f32 / self.step as f32, y as f32 / self.step as f32);
        let (x0, y0) = ((gx.floor() as usize).min(self.gw - 1), (gy.floor() as usize).min(self.gh - 1));
        let (x1, y1) = ((x0 + 1).min(self.gw - 1), (y0 + 1).min(self.gh - 1));
        let (fx, fy) = (gx - x0 as f32, gy - y0 as f32);
        let v = |cx: usize, cy: usize| f[cy * self.gw + cx].min(1.0e7);
        (v(x0, y0) * (1.0 - fx) + v(x1, y0) * fx) * (1.0 - fy) + (v(x0, y1) * (1.0 - fx) + v(x1, y1) * fx) * fy
    }
}

/// Which bone's part each bone's pixels end up in: bones with too few
/// pixels go to their parent, the hips to the spine.
fn merge_small(bones: &[&m::Bone], owner: &[usize], on: &[bool]) -> Vec<usize> {
    let count = |k: usize| owner.iter().zip(on).filter(|(o, on)| **on && **o == k).count();
    let total = on.iter().filter(|b| **b).count().max(1);
    let mut merged: Vec<usize> = (0..bones.len()).collect();
    let parent_of = |k: usize| bones[k].parent.and_then(|p| bones.iter().position(|b| b.id == p));
    for k in 0..bones.len() {
        let small = count(k) * 400 < total;
        let hips = role(&bones[k].name) == Role::Root && bones.iter().any(|b| role(&b.name) == Role::Spine && b.parent == Some(bones[k].id));
        if small || hips {
            // Hips into the spine (a child), anything else into its parent.
            let to = if hips { bones.iter().position(|b| role(&b.name) == Role::Spine && b.parent == Some(bones[k].id)) } else { parent_of(k) };
            if let Some(to) = to {
                merged[k] = to;
            }
        }
    }
    merged
}

fn root_of(merged: &[usize], mut k: usize) -> usize {
    for _ in 0..64 {
        if merged[k] == k {
            break;
        }
        k = merged[k];
    }
    k
}

/// Splits a `w × h` RGBA character picture into one picture per bone that
/// bends it: each pixel goes to the bone nearest to it through the
/// character (so an arm stays an arm where it crosses the body), and each
/// limb keeps a round cap of its parent's pixels at its joint so a turn
/// opens no gap. Bones with too few pixels go to their parent; the hips
/// join the spine as the torso.
pub fn split(rgba: &[u8], w: usize, h: usize, sk: &m::Skeleton) -> Result<Vec<Part>, String> {
    if rgba.len() < w * h * 4 || w < 2 || h < 2 {
        return Err("the picture is empty".into());
    }
    let bones: Vec<&m::Bone> = sk.bones.iter().filter(|b| b.deform).collect();
    if bones.is_empty() {
        return Err("the skeleton has no bones that bend the picture".into());
    }
    let grid = Grid::of(rgba, w, h);
    let (step, gw, gh, on) = (grid.step, grid.gw, grid.gh, grid.on.clone());
    let cell = |u: f64, v: f64| grid.cell(u, v);
    let field = |b: &m::Bone| grid.field(b);
    let fields: Vec<Vec<f32>> = bones.iter().map(|b| field(b)).collect();
    // Owner of each cell.
    let mut owner: Vec<usize> = (0..gw * gh).map(|i| (0..bones.len()).min_by(|&a, &b| fields[a][i].total_cmp(&fields[b][i])).unwrap_or(0)).collect();
    // Small parts and the hips go to their parents.
    let merged = merge_small(&bones, &owner, &on);
    let find = |k: usize| root_of(&merged, k);
    for o in &mut owner {
        *o = find(*o);
    }
    // Round caps: a limb also takes its parent's pixels within its
    // thickness around its joint.
    let sil = Silhouette { w: gw, h: gh, on: on.clone(), from: "" };
    let depth = sil.depth_map();
    let mut parts = Vec::new();
    for k in 0..bones.len() {
        if find(k) != k {
            continue;
        }
        let b = bones[k];
        let (jx, jy) = cell(b.head.x, b.head.y);
        let r = (depth[jy * gw + jx].min(gw.max(gh) as f32 / 8.0).max(2.0) * 1.3) as f64;
        let cap = |x: usize, y: usize| b.parent.is_some() && ((x as f64 - jx as f64).powi(2) + (y as f64 - jy as f64).powi(2)).sqrt() <= r;
        let mut px = vec![0u8; w * h * 4];
        let mut n = 0;
        for y in 0..h {
            for x in 0..w {
                let (cx, cy) = ((x / step).min(gw - 1), (y / step).min(gh - 1));
                let mine = owner[cy * gw + cx] == k || cap(cx, cy);
                let i = (y * w + x) * 4;
                if mine && rgba[i + 3] > 0 {
                    px[i..i + 4].copy_from_slice(&rgba[i..i + 4]);
                    n += 1;
                }
            }
        }
        if n == 0 {
            continue;
        }
        let name = if role(&b.name) == Role::Spine && merged.iter().enumerate().any(|(j, &to)| to == k && j != k && role(&bones[j].name) == Role::Root) { "torso".to_owned() } else { b.name.clone() };
        parts.push(Part { bone: b.id, name, rgba: px, pixels: n, filled: 0 });
    }
    Ok(parts)
}

/// Like [`split`], for a photograph of a real person or object: parts with
/// clean edges, every visible pixel in exactly one part (no round caps of
/// the parent's pixels copied into the limb), the part boundaries smooth
/// (the distance from each bone is interpolated between the search cells,
/// not stepped), stray holes and speckle repaired, and what a part hides
/// behind it painted in on the part behind (see
/// `studio_ai::features::parts`). `hints` are optional masks per bone id
/// (SAM 2.1 outlines of a limb, picture-sized, 0..1): a pixel inside a
/// limb's mask counts as nearer to that limb, so a boundary follows the
/// outline the model found.
pub fn split_clean(rgba: &[u8], w: usize, h: usize, sk: &m::Skeleton, hints: &[(u32, Vec<f32>)], opts: &studio_ai::features::parts::Options) -> Result<(Vec<Part>, studio_ai::features::parts::Report), String> {
    use studio_ai::features::parts as pt;
    if rgba.len() < w * h * 4 || w < 2 || h < 2 {
        return Err("the picture is empty".into());
    }
    let bones: Vec<&m::Bone> = sk.bones.iter().filter(|b| b.deform).collect();
    if bones.is_empty() {
        return Err("the skeleton has no bones that bend the picture".into());
    }
    let grid = Grid::of(rgba, w, h);
    let fields: Vec<Vec<f32>> = bones.iter().map(|b| grid.field(b)).collect();
    let owner: Vec<usize> = (0..grid.gw * grid.gh).map(|i| (0..bones.len()).min_by(|&a, &b| fields[a][i].total_cmp(&fields[b][i])).unwrap_or(0)).collect();
    let merged = merge_small(&bones, &owner, &grid.on);
    let roots: Vec<usize> = (0..bones.len()).filter(|&k| root_of(&merged, k) == k).collect();
    let spec_of: Vec<usize> = (0..bones.len()).map(|k| roots.iter().position(|&r| r == root_of(&merged, k)).unwrap_or(0)).collect();
    let masks: Vec<Option<&Vec<f32>>> = bones.iter().map(|b| hints.iter().find(|(id, _)| *id == b.id).map(|(_, m)| m).filter(|m| m.len() == w * h)).collect();
    // A label per pixel: inside a cell whose four corners agree, that
    // owner; along a boundary, the nearest bone by the interpolated field.
    let owner_at = |cx: usize, cy: usize| spec_of[owner[cy.min(grid.gh - 1) * grid.gw + cx.min(grid.gw - 1)]];
    let mut labels = vec![0u16; w * h];
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            if rgba[i * 4 + 3] < 8 {
                continue;
            }
            let (cx, cy) = (x / grid.step, y / grid.step);
            let o = owner_at(cx, cy);
            let sure = o == owner_at(cx + 1, cy) && o == owner_at(cx, cy + 1) && o == owner_at(cx + 1, cy + 1) && !masks.iter().any(Option::is_some);
            labels[i] = if sure {
                o as u16 + 1
            } else {
                let best = (0..bones.len())
                    .map(|k| {
                        let d = grid.at(&fields[k], x, y);
                        (k, masks[k].map_or(d, |mk| d * (1.0 - 0.45 * mk[i].clamp(0.0, 1.0))))
                    })
                    .min_by(|a, b| a.1.total_cmp(&b.1))
                    .map_or(0, |(k, _)| k);
                spec_of[best] as u16 + 1
            };
        }
    }
    let specs: Vec<pt::PartSpec> = roots
        .iter()
        .map(|&r| {
            let b = bones[r];
            let name = if role(&b.name) == Role::Spine && merged.iter().enumerate().any(|(j, &to)| to == r && j != r && role(&bones[j].name) == Role::Root) { "torso".to_owned() } else { b.name.clone() };
            let parent = b.parent.and_then(|p| bones.iter().position(|q| q.id == p)).map(|p| spec_of[p]).filter(|&p| p != spec_of[r]);
            pt::PartSpec { name, parent, joint: [(b.head.x * w as f64) as f32, (b.head.y * h as f64) as f32], depth: b.depth as f32 + if role(&b.name) == Role::Head { 0.25 } else { 0.0 } }
        })
        .collect();
    let frame = studio_ai::Frame::new(w, h, rgba.to_vec()).map_err(|e| e.to_string())?;
    let cleaned = pt::separate(&frame, &labels, &specs, opts);
    let parts = cleaned.parts.into_iter().zip(&roots).filter(|(p, _)| p.pixels > 0).map(|(p, &r)| Part { bone: bones[r].id, name: p.name, rgba: p.rgba, pixels: p.pixels, filled: p.filled }).collect();
    Ok((parts, cleaned.report))
}

/// Fills the transparent-or-cut hole `hole` (true = refill) of an RGBA
/// picture from the pixels around it (a smooth patch, as when an eye is
/// lifted onto its own layer and the face under it must not show a gap).
pub fn patch(rgba: &mut [u8], w: usize, h: usize, hole: &[bool]) {
    let mut known: Vec<bool> = hole.iter().map(|b| !b).collect();
    let mut left: Vec<usize> = (0..w * h).filter(|&i| hole[i]).collect();
    for _ in 0..(w.max(h)) {
        if left.is_empty() {
            break;
        }
        let mut next = Vec::new();
        let mut fill = Vec::new();
        for &i in &left {
            let (x, y) = ((i % w) as i64, (i / w) as i64);
            let mut acc = [0u32; 4];
            let mut n = 0;
            for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                let (nx, ny) = (x + dx, y + dy);
                if nx < 0 || ny < 0 || nx >= w as i64 || ny >= h as i64 {
                    continue;
                }
                let j = ny as usize * w + nx as usize;
                if known[j] && rgba[j * 4 + 3] > 0 {
                    for c in 0..4 {
                        acc[c] += rgba[j * 4 + c] as u32;
                    }
                    n += 1;
                }
            }
            if n > 0 {
                fill.push((i, acc.map(|a| (a / n) as u8)));
            } else {
                next.push(i);
            }
        }
        if fill.is_empty() {
            break;
        }
        for (i, c) in fill {
            rgba[i * 4..i * 4 + 4].copy_from_slice(&c);
            known[i] = true;
        }
        left = next;
    }
}

/// Where a face's eyes and mouth most likely are on a head whose pixels
/// fill the box `lo`..`hi` (0..1 picture coordinates): boxes `(name,
/// centre, half size)`, for a front view or a side view facing `right`.
pub fn face_boxes(lo: m::Vec2, hi: m::Vec2, side_view: bool, right: bool) -> Vec<(String, m::Vec2, m::Vec2)> {
    let (w, h) = (hi.x - lo.x, hi.y - lo.y);
    let at = |u: f64, v: f64| m::Vec2::new(lo.x + w * if right { u } else { 1.0 - u }, lo.y + h * v);
    let half = |a: f64, b: f64| m::Vec2::new(a * w, b * h);
    if side_view {
        vec![("eyes".into(), at(0.66, 0.38), half(0.12, 0.13)), ("mouth".into(), at(0.74, 0.72), half(0.2, 0.12))]
    } else {
        vec![("eye.front".into(), at(0.66, 0.42), half(0.13, 0.12)), ("eye.back".into(), at(0.34, 0.42), half(0.13, 0.12)), ("mouth".into(), at(0.5, 0.74), half(0.22, 0.1))]
    }
}

// ------------------------------------------------------------ swaps

/// A layer's place in a swap set, from `meta.swap`: `(rig clip, set,
/// option)`.
pub fn swap_tag(mc: &m::Clip) -> Option<(u64, String, String)> {
    let s = mc.meta.get("swap")?;
    Some((s.get("rig")?.as_u64()?, s.get("set")?.as_str()?.to_owned(), s.get("option")?.as_str()?.to_owned()))
}

/// Tags `mc` as option `option` of swap set `set` of rig clip `rig`.
pub fn set_swap_tag(mc: &mut m::Clip, rig: u64, set: &str, option: &str) {
    mc.meta.insert("swap".into(), serde_json::json!({"rig": rig, "set": set, "option": option}));
}

/// The opacity keys that show `choice` of a swap set at each time (hold
/// keys, so the switch is a cut on the frame): for each member clip, its
/// new `transform.opacity` track. `keys` are `(timeline seconds, option)`
/// in order; `members` are `(clip, its timeline start, option)`. Keys of
/// a member outside `[first key, last]` are kept.
pub fn swap_keys(members: &[(&m::Clip, f64, String)], keys: &[(f64, String)]) -> Vec<(u64, m::Track)> {
    let mut out = Vec::new();
    let (Some(first), Some(last)) = (keys.first(), keys.last()) else { return out };
    for (mc, start, option) in members {
        let mut track = mc.keyframes.get("transform.opacity").cloned().unwrap_or_else(|| m::Track::new(Vec::new()));
        let local = |s: f64| ((s - start) * m::SECOND as f64).round() as m::Micros;
        let (lo, hi) = (local(first.0), local(last.0));
        track.keys.retain(|k| k.time < lo || k.time > hi);
        for (t, choice) in keys {
            let v = if choice.eq_ignore_ascii_case(option) { 1.0 } else { 0.0 };
            track.set_key(m::Keyframe::new(local(*t).max(0), v, m::Interp::Hold));
        }
        out.push((mc.id.0, track));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stick figure facing right on a 100 × 140 picture: a round head, a
    /// body, an arm reaching forward and two legs.
    fn figure() -> (usize, usize, Vec<u8>) {
        let (w, h) = (100, 140);
        let mut px = vec![0u8; w * h * 4];
        let mut fill = |x0: usize, y0: usize, x1: usize, y1: usize, c: [u8; 3]| {
            for y in y0..y1 {
                for x in x0..x1 {
                    let i = (y * w + x) * 4;
                    px[i..i + 4].copy_from_slice(&[c[0], c[1], c[2], 255]);
                }
            }
        };
        fill(40, 4, 62, 26, [240, 200, 160]); // head
        fill(46, 26, 56, 32, [240, 200, 160]); // neck
        fill(38, 32, 64, 80, [40, 90, 200]); // body
        fill(64, 40, 92, 48, [40, 90, 200]); // front arm, reaching
        fill(30, 40, 38, 74, [40, 90, 200]); // back arm, hanging
        fill(40, 80, 49, 136, [30, 30, 30]); // back leg
        fill(54, 80, 63, 136, [30, 30, 30]); // front leg
        (w, h, px)
    }

    #[test]
    fn a_biped_skeleton_lands_on_its_silhouette() {
        let (w, h, px) = figure();
        let s = Silhouette::of_rgba(w, h, &px).unwrap();
        let sk = template("person", &s, true, w as f64 / h as f64).unwrap();
        let b = |n: &str| sk.named(n).unwrap_or_else(|| panic!("no {n}: {:?}", sk.bones.iter().map(|b| &b.name).collect::<Vec<_>>()));
        assert!(b("head").tail.y < 0.1, "head at the top: {:?}", b("head").tail);
        assert!(b("shin.front").tail.y > 0.9 && b("shin.back").tail.y > 0.9);
        assert!(b("forearm.front").tail.x > 0.8, "the front hand reaches right: {:?}", b("forearm.front").tail);
        assert_eq!(b("forearm.front").parent, Some(b("upper_arm.front").id));
        assert_eq!(b("thigh.front").parent, Some(b("hips").id));
        // Knees bend back, elbows forward: facing right, a shin may turn
        // clockwise only (its foot goes back).
        assert!(b("shin.front").max.unwrap() > 100.0 && b("shin.front").min.unwrap() > -10.0);
        assert!(sk.ik.iter().any(|h| h.name == "foot.front" && !h.enabled));
        for kind in ["dog", "bird", "fish", "snake", "tree", "object"] {
            assert!(template(kind, &s, true, 0.7).is_ok(), "{kind}");
        }
        assert!(template("cloud", &s, true, 1.0).is_err());
    }

    #[test]
    fn a_walk_swings_legs_in_turn_and_travels() {
        let (w, h, px) = figure();
        let s = Silhouette::of_rgba(w, h, &px).unwrap();
        let mut sk = template("biped", &s, true, w as f64 / h as f64).unwrap();
        sk.faces_right = Some(true);
        let k = act_keys(&["walk".into()], &sk, 1.0, 3.0, true, 1.0, 7);
        let th_f = &k.rotations[&sk.named("thigh.front").unwrap().id];
        let th_b = &k.rotations[&sk.named("thigh.back").unwrap().id];
        assert_eq!(th_f.first().unwrap().0, 1.0);
        assert!(th_f.first().unwrap().1.abs() < 1e-9 && th_f.last().unwrap().1.abs() < 1e-9, "starts and ends at rest");
        assert!(th_f.iter().zip(th_b).any(|(a, b)| a.1 * b.1 < -50.0), "legs swing opposite");
        // Facing right a thigh swinging forward turns counter-clockwise.
        assert!(th_f.iter().any(|p| p.1 < -15.0) && th_f.iter().any(|p| p.1 > 15.0));
        let shin = &k.rotations[&sk.named("shin.front").unwrap().id];
        assert!(shin.iter().all(|p| p.1 >= -5.0 - 1e-9), "knees only bend back: {:?}", shin.iter().map(|p| p.1).fold(f64::MAX, f64::min));
        assert!(k.stride.unwrap() > 0.05);
        assert!(k.offsets.values().next().unwrap().iter().any(|p| p.1.y < -0.005), "the body bobs up");
        // Facing left mirrors; seen from the front, a walk sways rather than
        // strides.
        let left = act_keys(&["walk".into()], &sk, 1.0, 3.0, false, 1.0, 7);
        assert!(left.rotations.values().next().is_some());
        let mut front = sk.clone();
        front.faces_right = None;
        let f = act_keys(&["walk".into()], &front, 1.0, 3.0, true, 1.0, 7);
        let reach = |k: &ActKeys, id: u32| k.rotations[&id].iter().map(|p| p.1.abs()).fold(0.0, f64::max);
        let th = sk.named("thigh.front").unwrap().id;
        assert!(reach(&f, th) < 0.5 * reach(&k, th));
        for a in ACT_NAMES {
            let k = act_keys(&[a.to_owned()], &sk, 0.0, 2.0, true, 1.0, 3);
            let tails = ["wag", "swim", "slither", "sway", "breeze"];
            assert!(!k.rotations.is_empty() || tails.contains(&a), "{a} moves no bone");
        }
        assert!(!known_act("fly"));
    }

    #[test]
    fn a_split_gives_each_bone_its_pixels_with_joint_caps() {
        let (w, h, px) = figure();
        let s = Silhouette::of_rgba(w, h, &px).unwrap();
        let sk = template("biped", &s, true, w as f64 / h as f64).unwrap();
        let parts = split(&px, w, h, &sk).unwrap();
        let names: Vec<&str> = parts.iter().map(|p| p.name.as_str()).collect();
        assert!(names.contains(&"torso") && names.contains(&"head"), "{names:?}");
        assert!(!names.contains(&"hips"));
        let part = |n: &str| parts.iter().find(|p| p.name == n).unwrap();
        let has = |p: &Part, x: usize, y: usize| p.rgba[(y * w + x) * 4 + 3] > 0;
        assert!(has(part("head"), 50, 10) && !has(part("torso"), 50, 10));
        // The front foot is the front shin's, not the back one's.
        assert!(has(part("shin.front"), 58, 130) && !has(part("shin.back"), 58, 130));
        // Every pixel is in some part.
        for y in 0..h {
            for x in 0..w {
                if px[(y * w + x) * 4 + 3] > 0 {
                    assert!(parts.iter().any(|p| has(p, x, y)), "({x}, {y}) is in no part");
                }
            }
        }
        // A limb overlaps its parent at the joint.
        let th = sk.named("thigh.front").unwrap();
        let (jx, jy) = ((th.head.x * w as f64) as usize, (th.head.y * h as f64) as usize);
        assert!(has(part("thigh.front"), jx, jy.saturating_sub(3)) || has(part("thigh.front"), jx, jy));
    }

    #[test]
    fn patches_fill_holes_and_swaps_cut_on_the_frame() {
        let (w, h) = (8, 8);
        let mut px = vec![200u8; w * h * 4];
        let hole: Vec<bool> = (0..w * h).map(|i| (3..5).contains(&(i % w)) && (3..5).contains(&(i / w))).collect();
        for (i, &b) in hole.iter().enumerate() {
            if b {
                px[i * 4..i * 4 + 4].copy_from_slice(&[0, 0, 0, 255]);
            }
        }
        patch(&mut px, w, h, &hole);
        assert_eq!(&px[(3 * w + 3) * 4..(3 * w + 3) * 4 + 4], &[200, 200, 200, 200]);
        let mk = |id: u64| m::Clip::new(m::ClipId(id), m::LaneId(1), 0, 10 * m::SECOND, m::ClipSource::Generator(m::Generator::new(m::GeneratorKind::Solid)));
        let (a, b) = (mk(5), mk(6));
        let members = vec![(&a, 0.0, "A".to_owned()), (&b, 0.0, "O".to_owned())];
        let out = swap_keys(&members, &[(1.0, "A".into()), (2.0, "O".into()), (3.0, "A".into())]);
        let ta = &out[0].1;
        assert_eq!(ta.eval(m::SECOND + m::SECOND / 2), Some(m::Value::Scalar(1.0)));
        assert_eq!(ta.eval(2 * m::SECOND + m::SECOND / 2), Some(m::Value::Scalar(0.0)));
        assert_eq!(out[1].1.eval(2 * m::SECOND + 1), Some(m::Value::Scalar(1.0)));
        let mut c = mk(9);
        set_swap_tag(&mut c, 3, "mouth", "A");
        assert_eq!(swap_tag(&c), Some((3, "mouth".into(), "A".into())));
        let boxes = face_boxes(m::Vec2::new(0.4, 0.1), m::Vec2::new(0.6, 0.4), false, true);
        assert_eq!(boxes.len(), 3);
        assert!(boxes[2].1.y > boxes[0].1.y, "the mouth is below the eyes");
        assert!(boxes[0].1.x > boxes[1].1.x);
        assert_eq!(role("front_leg.lower"), Role::Forearm(Side::Front));
        assert_eq!(role("tail.3"), Role::Tail(3));
        assert_eq!(role("trunk.2"), Role::Chain(2));
        assert_eq!(role("hips"), Role::Root);
    }
}
