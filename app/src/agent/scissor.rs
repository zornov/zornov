// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Scissoring for agents: cut lines that split a character's deformation
//! mesh so legs, arms and lips move apart instead of stretching together
//! (the same cuts the canvas's scissor tool draws; see `studio_model::Cut`
//! and `editor::pins`).
//!
//! * `scissor`: `cuts` draws cuts (`{"name": "legs", "path": [[x, y], …],
//!   "fill": "auto" | "#601010" | "none"}`); `auto` finds them (`true`,
//!   or `legs`, `arms`, `mouth`, `gaps`, `parts`, `all`); `cut` + `path` /
//!   `to` / `by` / `fill` / `name` / `enabled` changes one; `remove`
//!   deletes (`"all"`, an id or a name); `list` says what is there.
//! * `puppet` and `skeleton` take `cuts` and `auto_cut` too, and rigging a
//!   character (`rig`, `build`) cuts its legs, arms and mouth unless
//!   `auto_cut` is false.
//!
//! Points are canvas pixels from the centre, a name of the layer's box or
//! `{"u", "v"}`, like pins; paths are read through the layer's placement at
//! `at`. The cuts themselves live in the layer's 0..1 coordinates.

use super::Session;
use crate::editor::pins::LayerMap;
use crate::services::{character as ch, rigging as rg};
use crate::view;
use serde_json::{Value, json};
use studio_model as m;
use studio_rig::scissor as sc;

/// Marks the cuts a finder made, so finding again replaces them and leaves
/// the ones somebody drew.
const AUTO: &str = "auto";

/// A cut a finder proposes.
struct Found {
    cut: m::Cut,
    /// What it separates, for the report.
    what: String,
    /// Whether rigging applies it without being asked.
    by_default: bool,
}

/// Which finders an `auto` asks for.
#[derive(Clone, Copy, Default)]
struct Which {
    legs: bool,
    arms: bool,
    mouth: bool,
    gaps: bool,
    parts: bool,
}

impl Which {
    /// What rigging a `kind` of character cuts without being asked.
    fn rigging(kind: &str) -> Option<Which> {
        match rg::kind_of(kind)? {
            "biped" => Some(Which { legs: true, arms: true, mouth: true, gaps: false, parts: true }),
            "quadruped" => Some(Which { legs: true, parts: true, ..Which::default() }),
            _ => None,
        }
    }

    fn parse(v: &Value, kind: Option<&str>) -> Result<Which, String> {
        let words: Vec<String> = match v {
            Value::Bool(true) => {
                return Ok(kind.and_then(Which::rigging).unwrap_or(Which { gaps: true, parts: true, ..Which::default() }));
            }
            Value::String(s) => vec![s.to_lowercase()],
            Value::Array(a) => a.iter().filter_map(Value::as_str).map(str::to_lowercase).collect(),
            _ => return Err(AUTO_HELP.into()),
        };
        let mut w = Which::default();
        for word in &words {
            match word.as_str() {
                "legs" => w.legs = true,
                "arms" => w.arms = true,
                "mouth" | "lips" => w.mouth = true,
                "gaps" | "notches" => w.gaps = true,
                "parts" | "limbs" => w.parts = true,
                "all" => w = Which { legs: true, arms: true, mouth: true, gaps: true, parts: true },
                other => return Err(format!("auto \"{other}\": {AUTO_HELP}")),
            }
        }
        if words.is_empty() {
            return Err(AUTO_HELP.into());
        }
        Ok(w)
    }

    fn any_gaps(self) -> bool {
        self.legs || self.arms || self.gaps
    }
}

const AUTO_HELP: &str = "auto is true (the usual cuts for the kind of character), or legs, arms, mouth, gaps (any narrow gap or notch), parts (borders of the skeleton's parts), all; or a list of them";

/// An sRGB colour from `"#rrggbb"`, a name, `[r, g, b(, a)]` in 0..1 or
/// 0..255.
fn colour_of(v: &Value) -> Result<m::Rgba, String> {
    match v {
        Value::String(s) => {
            let c = super::craft::colour(s).ok_or_else(|| format!("colour \"{s}\": #rrggbb or a name (red, black…)"))?;
            Ok(m::Rgba::new(c.r() as f64 / 255.0, c.g() as f64 / 255.0, c.b() as f64 / 255.0, c.a() as f64 / 255.0))
        }
        Value::Array(a) if a.len() == 3 || a.len() == 4 => {
            let n: Vec<f64> = a.iter().filter_map(Value::as_f64).collect();
            if n.len() != a.len() {
                return Err("a colour is numbers".into());
            }
            let k = if n.iter().any(|x| *x > 1.0) { 255.0 } else { 1.0 };
            Ok(m::Rgba::new(n[0] / k, n[1] / k, n[2] / k, n.get(3).map_or(1.0, |a| a / if n.iter().any(|x| *x > 1.0) && n[3] > 1.0 { 255.0 } else { 1.0 })))
        }
        _ => Err("a colour is \"#rrggbb\", a name or [r, g, b]".into()),
    }
}

/// A fill from `"none"`, `"auto"` (generated from the picture), `"mouth"`
/// (a mouth's dark inside) or a colour.
fn fill_of(v: &Value) -> Result<m::CutFill, String> {
    match v {
        Value::Null | Value::Bool(false) => Ok(m::CutFill::None),
        Value::Bool(true) => Ok(m::CutFill::Auto),
        Value::String(s) => match s.to_lowercase().as_str() {
            "none" | "off" | "" => Ok(m::CutFill::None),
            "auto" | "generated" | "shade" => Ok(m::CutFill::Auto),
            "mouth" | "inside" => Ok(mouth_fill()),
            _ => colour_of(v).map(m::CutFill::Colour),
        },
        _ => colour_of(v).map(m::CutFill::Colour),
    }
}

fn mouth_fill() -> m::CutFill {
    let [r, g, b, a] = sc::MOUTH_INSIDE;
    m::CutFill::Colour(m::Rgba::new(r, g, b, a))
}

fn fill_said(fill: &m::CutFill) -> Value {
    match fill {
        m::CutFill::None => json!("none"),
        m::CutFill::Auto => json!("auto"),
        m::CutFill::Colour(c) => json!(format!("#{:02x}{:02x}{:02x}", (c.r * 255.0).round() as u8, (c.g * 255.0).round() as u8, (c.b * 255.0).round() as u8)),
    }
}

fn is_auto(c: &m::Cut) -> bool {
    c.extra.get(AUTO).and_then(Value::as_bool).unwrap_or(false)
}

/// The finder's cut as a model cut (id given when it is added).
fn model_cut(s: &sc::Suggestion, name: &str) -> m::Cut {
    let mut c = m::Cut::new(0, s.points.iter().map(|p| m::Vec2::new(p[0], p[1])).collect());
    c.name = name.to_owned();
    c.fill = match s.fill {
        Some([r, g, b, a]) => m::CutFill::Colour(m::Rgba::new(r, g, b, a)),
        None => m::CutFill::None,
    };
    c.extra.insert(AUTO.into(), json!(true));
    c
}

/// How far (layer fractions) a cut keeps from another before the two
/// count as the same line.
const SAME_LINE: f64 = 0.04;

fn mostly_on(c: &m::Cut, other: &[m::Cut]) -> bool {
    let near = c.points.iter().filter(|p| other.iter().any(|o| o.nearest(**p).is_some_and(|(_, d)| d < SAME_LINE))).count();
    near * 10 >= c.points.len() * 6
}

impl Session {
    fn scissor_map(&self, id: u32, t: view::Time) -> Result<LayerMap, String> {
        LayerMap::at(&self.doc.project, &self.doc.model, id, t).ok_or_else(|| format!("clip {id} draws nothing on the canvas at {:.2}s", super::secs(t)))
    }

    /// A cut from a spec: `{"name", "path" | "points", "fill", "enabled"}`
    /// or just a list of points.
    fn cut_from(&self, map: &LayerMap, spec: &Value) -> Result<m::Cut, String> {
        let (path, name, fill, enabled) = match spec {
            Value::Array(_) => (spec, "", None, true),
            Value::Object(o) => (
                o.get("path").or_else(|| o.get("points")).ok_or("each cut needs \"path\": [[x, y], …] (at least two points, canvas pixels from the centre, or {\"u\", \"v\"})")?,
                o.get("name").and_then(Value::as_str).unwrap_or(""),
                o.get("fill"),
                o.get("enabled").and_then(Value::as_bool).unwrap_or(true),
            ),
            _ => return Err("a cut is {\"name\", \"path\": [[x, y], …], \"fill\"} or a list of points".into()),
        };
        let Value::Array(pts) = path else { return Err("a path is a list of points: [[x, y], [x, y], …]".into()) };
        // A pair of numbers is one point; a list of pairs is the path.
        let points: Vec<m::Vec2> = pts.iter().map(|p| Self::layer_point(map, p, false)).collect::<Result<_, _>>()?;
        if points.len() < 2 {
            return Err("a cut needs at least two points".into());
        }
        let mut c = m::Cut::new(0, points);
        c.name = name.to_owned();
        c.enabled = enabled;
        if let Some(v) = fill {
            c.fill = fill_of(v)?;
        }
        if !c.is_active() && enabled {
            return Err("a cut's points are all in one place".into());
        }
        Ok(c)
    }

    fn find_cut(cuts: &[m::Cut], v: &Value) -> Result<u32, String> {
        let found = match v {
            Value::Number(n) => n.as_u64().and_then(|id| cuts.iter().find(|c| c.id as u64 == id)),
            Value::String(s) => cuts.iter().find(|c| c.name.eq_ignore_ascii_case(s)).or_else(|| s.parse::<u32>().ok().and_then(|id| cuts.iter().find(|c| c.id == id))),
            _ => None,
        };
        found.map(|c| c.id).ok_or_else(|| {
            let have: Vec<String> = cuts.iter().map(|c| if c.name.is_empty() { c.id.to_string() } else { format!("{} \"{}\"", c.id, c.name) }).collect();
            if have.is_empty() { "there are no cuts yet: draw some with \"cuts\" or find them with \"auto\"".to_owned() } else { format!("no cut {v}; cuts: {}", have.join(", ")) }
        })
    }

    /// The cuts as a line each, in canvas pixels at `t`.
    fn cuts_said(&self, id: u32, mc: &m::Clip, t: view::Time) -> Vec<Value> {
        let Ok(map) = self.scissor_map(id, t) else { return Vec::new() };
        mc.cuts
            .iter()
            .map(|c| {
                let path: Vec<Value> = c
                    .points
                    .iter()
                    .map(|p| {
                        let v = map.canvas(*p);
                        json!([(v.x as f64 * 10.0).round() / 10.0, (v.y as f64 * 10.0).round() / 10.0])
                    })
                    .collect();
                let mut o = json!({"cut": c.id, "path": path, "fill": fill_said(&c.fill)});
                if !c.name.is_empty() {
                    o["name"] = json!(c.name);
                }
                if !c.enabled {
                    o["enabled"] = json!(false);
                }
                if is_auto(c) {
                    o["found"] = json!("auto");
                }
                o
            })
            .collect()
    }

    /// The picture of a layer as lightness 0..1 over what the layer shows
    /// (after crop), at most about 512 across, for finding a drawn mouth.
    fn luma_of(&self, mc: &m::Clip) -> Option<(Vec<f32>, usize, usize)> {
        let path = ch::picture_path(&self.doc, mc).ok()?;
        let (w, h, px) = ch::picture(&path).ok()?;
        let c = &mc.transform.crop;
        let x0 = (c.left.clamp(0.0, 1.0) * w as f64).round() as usize;
        let y0 = (c.top.clamp(0.0, 1.0) * h as f64).round() as usize;
        let x1 = ((1.0 - c.right.clamp(0.0, 1.0)) * w as f64).round() as usize;
        let y1 = ((1.0 - c.bottom.clamp(0.0, 1.0)) * h as f64).round() as usize;
        if x1 <= x0 + 2 || y1 <= y0 + 2 {
            return None;
        }
        let step = ((x1 - x0).max(y1 - y0) / 512).max(1);
        let (cw, chh) = ((x1 - x0).div_ceil(step), (y1 - y0).div_ceil(step));
        let mut out = Vec::with_capacity(cw * chh);
        for y in (y0..y1).step_by(step) {
            for x in (x0..x1).step_by(step) {
                let i = (y * w + x) * 4;
                out.push(if px[i + 3] < 128 { 1.0 } else { (0.2126 * px[i] as f32 + 0.7152 * px[i + 1] as f32 + 0.0722 * px[i + 2] as f32) / 255.0 });
            }
        }
        Some((out, cw, chh))
    }

    /// Cuts for a character from its silhouette (narrow gaps and notches:
    /// the legs, arms), its face (the lips) and its skeleton's parts.
    fn find_cuts(&self, mc: &m::Clip, kind: Option<&str>, which: Which) -> Result<Vec<Found>, String> {
        // Asked for legs or arms without a kind: a person.
        let kind = kind.and_then(rg::kind_of).or(if which.legs || which.arms { Some("biped") } else { None });
        let (w, h, on, from, facing) = self.shape_grid(mc)?;
        let mut out: Vec<Found> = Vec::new();
        let (mut bx0, mut by0, mut bx1, mut by1) = (w, h, 0usize, 0usize);
        for (i, v) in on.iter().enumerate() {
            if *v {
                bx0 = bx0.min(i % w);
                bx1 = bx1.max(i % w);
                by0 = by0.min(i / w);
                by1 = by1.max(i / w);
            }
        }
        if bx0 > bx1 {
            return Err("the silhouette is empty".into());
        }
        let (bw, bh) = ((bx1 - bx0 + 1) as f64, (by1 - by0 + 1) as f64);
        if which.any_gaps() {
            let mut legs = 0;
            let mut arms = 0;
            for g in sc::gaps(sc::Mask { w, h, on: &on }, sc::GapOptions::default()) {
                let apex = g.points[0];
                let (rx, ry) = ((apex[0] * w as f64 - bx0 as f64) / bw, (apex[1] * h as f64 - by0 as f64) / bh);
                let (role, wanted) = match kind {
                    Some("biped") if ry >= 0.45 && (0.2..0.8).contains(&rx) => ("legs", which.legs),
                    Some("biped") if ry < 0.65 && !(0.35..0.65).contains(&rx) => ("arms", which.arms),
                    Some("quadruped") if ry >= 0.4 => ("legs", which.legs),
                    _ => ("gap", which.gaps),
                };
                if !wanted {
                    continue;
                }
                let name = match role {
                    "legs" => {
                        legs += 1;
                        if legs == 1 { "legs".to_owned() } else { format!("legs {legs}") }
                    }
                    "arms" => {
                        arms += 1;
                        format!("arm.{}", if rx < 0.5 { "left" } else { "right" })
                    }
                    _ => format!("gap {}", out.len() + 1),
                };
                let what = format!("{name}: a {} gap from ({:.2}, {:.2}) down to ({:.2}, {:.2})", if role == "gap" { "narrow" } else { "narrow" }, apex[0], apex[1], g.points.last().map_or(0.0, |p| p[0]), g.points.last().map_or(0.0, |p| p[1]));
                out.push(Found { cut: model_cut(&g, &name), what, by_default: true });
            }
            let _ = arms;
        }
        if which.mouth {
            // The head is the top of the silhouette; its face the middle of
            // the lower half of it.
            let head_h = (0.17 * bh).max(4.0);
            let top: Vec<usize> = (0..w * h).filter(|i| on[*i] && (i / w) < by0 + 4.max((head_h * 0.25) as usize)).map(|i| i % w).collect();
            if !top.is_empty() {
                let xc = top.iter().sum::<usize>() as f64 / top.len() as f64;
                let half = 0.5 * head_h * 0.95;
                let lo = m::Vec2::new(((xc - half).max(0.0)) / w as f64, by0 as f64 / h as f64);
                let hi = m::Vec2::new(((xc + half).min(w as f64)) / w as f64, (by0 as f64 + head_h) / h as f64);
                let side = facing.is_some();
                let boxes = rg::face_boxes(lo, hi, side, facing.unwrap_or(true));
                if let Some((_, centre, half)) = boxes.iter().find(|(n, _, _)| n == "mouth") {
                    let landmarks = sc::Mouth { corners: [[centre.x - half.x, centre.y], [centre.x + half.x, centre.y]], lips: None };
                    let luma = self.luma_of(mc);
                    let s = sc::mouth(&landmarks, luma.as_ref().map(|(p, w, h)| (p.as_slice(), *w, *h)));
                    let seen = s.confidence >= 0.6;
                    let what = if seen { "mouth: the lip line found in the picture".to_owned() } else { "mouth: the lips estimated from the head (no drawn mouth line found in the picture)".to_owned() };
                    out.push(Found { cut: model_cut(&s, "mouth"), what, by_default: seen });
                }
            }
        }
        if which.parts {
            if let Ok(parts) = self.part_cuts(mc) {
                let chosen: Vec<m::Cut> = out.iter().map(|f| f.cut.clone()).collect();
                for p in parts {
                    if !mostly_on(&p.cut, &chosen) {
                        out.push(p);
                    }
                }
            }
        }
        let _ = from;
        Ok(out)
    }

    /// Cuts along the borders of a skeleton's parts where an arm or a leg
    /// meets a neighbour, from the part splitter, left joined at the joints.
    fn part_cuts(&self, mc: &m::Clip) -> Result<Vec<Found>, String> {
        let sk = mc.skeleton.as_ref().ok_or("no skeleton")?;
        let path = ch::picture_path(&self.doc, mc)?;
        let (w, h, rgba) = ch::picture(&path)?;
        let parts = rg::split(&rgba, w, h, sk)?;
        let c = &mc.transform.crop;
        let x0 = (c.left.clamp(0.0, 1.0) * w as f64).round() as usize;
        let y0 = (c.top.clamp(0.0, 1.0) * h as f64).round() as usize;
        let x1 = ((1.0 - c.right.clamp(0.0, 1.0)) * w as f64).round() as usize;
        let y1 = ((1.0 - c.bottom.clamp(0.0, 1.0)) * h as f64).round() as usize;
        if x1 <= x0 + 2 || y1 <= y0 + 2 {
            return Err("the crop leaves nothing".into());
        }
        let step = ((x1 - x0).max(y1 - y0) / 192).max(1);
        let (gw, gh) = ((x1 - x0).div_ceil(step), (y1 - y0).div_ceil(step));
        let (cw, chh) = (gw, gh);
        let mut labels = vec![0u32; gw * gh];
        for gy in 0..gh {
            for gx in 0..gw {
                let (x, y) = (x0 + gx * step, y0 + gy * step);
                if let Some(k) = parts.iter().position(|p| p.rgba[(y * w + x) * 4 + 3] > 0) {
                    labels[gy * gw + gx] = k as u32 + 1;
                }
            }
        }
        let limb = |name: &str| match rg::role(name) {
            rg::Role::UpperArm(s) | rg::Role::Forearm(s) => Some((0, s)),
            rg::Role::Thigh(s) | rg::Role::Shin(s) => Some((1, s)),
            _ => None,
        };
        // Joints stay joined: where a limb hangs from its parent.
        let joints: Vec<([f64; 2], f64)> = sk.bones.iter().filter(|b| b.parent.is_some()).map(|b| ([b.head.x, b.head.y], 0.06)).collect();
        let mut out = Vec::new();
        for s in sc::boundaries(&labels, gw, gh, &joints, 0.08) {
            let Some((a, b)) = s.between else { continue };
            let (pa, pb) = (&parts[a as usize - 1], &parts[b as usize - 1]);
            let (la, lb) = (limb(&pa.name), limb(&pb.name));
            // Between two limbs or a limb and the body; not between the
            // segments of one limb (that is a joint).
            if (la.is_none() && lb.is_none()) || (la.is_some() && la == lb) {
                continue;
            }
            // Along a limb, not across it: the line where a limb joins its
            // parent is the joint, which stays joined.
            let (dx, dy) = ((s.points[s.points.len() - 1][0] - s.points[0][0]) * cw as f64, (s.points[s.points.len() - 1][1] - s.points[0][1]) * chh as f64);
            let along = |part: &rg::Part, limb: bool| -> bool {
                let Some(bone) = sk.bone(part.bone).filter(|_| limb) else { return false };
                let (bx, by) = ((bone.tail.x - bone.head.x) * cw as f64, (bone.tail.y - bone.head.y) * chh as f64);
                let (l1, l2) = (dx.hypot(dy), bx.hypot(by));
                l1 > 1e-9 && l2 > 1e-9 && ((dx * bx + dy * by) / (l1 * l2)).abs() >= 0.7
            };
            if !along(pa, la.is_some()) && !along(pb, lb.is_some()) {
                continue;
            }
            let name = format!("{}/{}", pa.name, pb.name);
            let what = format!("{name}: the border between those parts");
            out.push(Found { cut: model_cut(&s, &name), what, by_default: true });
        }
        Ok(out)
    }

    /// Adds `found` cuts, replacing the ones a finder made before.
    fn add_found(mc: &mut m::Clip, found: Vec<Found>, only_default: bool) -> (Vec<String>, Vec<String>) {
        mc.cuts.retain(|c| !is_auto(c));
        let (mut done, mut skipped) = (Vec::new(), Vec::new());
        for f in found {
            if only_default && !f.by_default {
                skipped.push(f.what);
                continue;
            }
            let mut c = f.cut;
            c.id = m::Cut::next_id(&mc.cuts);
            done.push(format!("{} (cut {})", f.what, c.id));
            mc.cuts.push(c);
        }
        (done, skipped)
    }

    /// The cut work of a `puppet`, `skeleton` or `scissor` op on `mc`:
    /// `cuts` drawn, `auto_cut` / `auto` found, and, when the op just rigged
    /// a character of `rig_kind`, the usual cuts for it unless `auto_cut` is
    /// false. Returns what was done.
    pub(super) fn cut_work(&self, op: &Value, id: u32, mc: &mut m::Clip, at: view::Time, rig_kind: Option<&str>) -> Result<Vec<String>, String> {
        let mut said = Vec::new();
        if let Some(list) = op.get("cuts") {
            let Value::Array(list) = list else { return Err("cuts is a list: [{\"name\": \"mouth\", \"path\": [[x, y], …], \"fill\": \"mouth\"}, …]".into()) };
            let map = self.scissor_map(id, at)?;
            for spec in list {
                let mut c = self.cut_from(&map, spec)?;
                // A cut with a name replaces the one of that name.
                if !c.name.is_empty() {
                    mc.cuts.retain(|o| !o.name.eq_ignore_ascii_case(&c.name));
                }
                c.id = m::Cut::next_id(&mc.cuts);
                said.push(format!("cut {}{}", c.id, if c.name.is_empty() { String::new() } else { format!(" \"{}\"", c.name) }));
                mc.cuts.push(c);
            }
        }
        let auto = op.get("auto").or_else(|| op.get("auto_cut"));
        let (which, explicit) = match auto {
            Some(Value::Bool(false)) => return Ok(said),
            Some(v) => (Some(Which::parse(v, rig_kind.or_else(|| op.get("build").or_else(|| op.get("kind")).and_then(Value::as_str)))?), true),
            None => (rig_kind.and_then(Which::rigging), false),
        };
        if let Some(which) = which {
            let kind = rig_kind.or_else(|| op.get("build").or_else(|| op.get("kind")).and_then(Value::as_str));
            let found = self.find_cuts(mc, kind, which)?;
            if found.is_empty() {
                if explicit {
                    said.push("auto cut found nothing to separate (no narrow gap or notch in the silhouette)".into());
                }
            } else {
                let (done, skipped) = Self::add_found(mc, found, !explicit);
                if !done.is_empty() {
                    said.push(format!("cut the mesh so parts move apart: {}", done.join("; ")));
                }
                if !skipped.is_empty() {
                    said.push(format!("proposed, not applied: {} (apply with \"auto\": [\"mouth\"])", skipped.join("; ")));
                }
            }
        }
        Ok(said)
    }

    /// `scissor`: cut lines that split the mesh a puppet or skeleton bends.
    pub(super) fn scissor(&mut self, op: &Value) -> Result<String, String> {
        let (id, mut mc, _, at) = self.pin_target(op, "scissor")?;
        let mut said = Vec::new();
        if let Some(r) = op.get("remove") {
            if r.as_str() == Some("all") {
                mc.cuts.clear();
                said.push("removed every cut".to_owned());
            } else {
                let list = match r {
                    Value::Array(a) => a.clone(),
                    v => vec![v.clone()],
                };
                let ids: Vec<u32> = list.iter().map(|v| Self::find_cut(&mc.cuts, v)).collect::<Result<_, _>>()?;
                mc.cuts.retain(|c| !ids.contains(&c.id));
                said.push(format!("removed cut(s) {ids:?}"));
            }
        }
        said.extend(self.cut_work(op, id, &mut mc, at, None)?);
        if let Some(which) = op.get("cut") {
            let cid = Self::find_cut(&mc.cuts, which)?;
            let map = self.scissor_map(id, at)?;
            let new_path = match op.get("path") {
                Some(p) => Some(self.cut_from(&map, p)?.points),
                None => None,
            };
            let moved: Option<Vec<m::Vec2>> = match op.get("by").and_then(|b| b.as_array()).filter(|a| a.len() == 2) {
                Some(b) => {
                    let by = [b[0].as_f64().ok_or("by is [dx, dy] canvas pixels")?, b[1].as_f64().ok_or("by is [dx, dy] canvas pixels")?];
                    let cut = mc.cuts.iter().find(|c| c.id == cid).expect("found");
                    Some(cut.points.iter().map(|p| Self::moved_by(&map, *p, by, false)).collect::<Result<_, _>>()?)
                }
                None => None,
            };
            let cut = mc.cuts.iter_mut().find(|c| c.id == cid).expect("found");
            if let Some(p) = new_path.or(moved) {
                cut.points = p;
                said.push(format!("cut {cid} moved"));
            }
            if let Some(v) = op.get("fill") {
                cut.fill = fill_of(v)?;
                said.push(format!("cut {cid} fills its gap with {}", fill_said(&cut.fill)));
            }
            if let Some(n) = op.get("name").and_then(Value::as_str) {
                cut.name = n.to_owned();
            }
            if let Some(b) = op.get("enabled").and_then(Value::as_bool) {
                cut.enabled = b;
                said.push(if b { format!("cut {cid} on") } else { format!("cut {cid} off (kept, the mesh joined across it)") });
            }
        } else {
            for k in ["to", "by", "fill", "enabled"] {
                if op.get(k).is_some() && op.get("cuts").is_none() && op.get("auto").is_none() {
                    return Err(format!("\"{k}\" changes one cut: say which with \"cut\" (its id or name)"));
                }
            }
        }
        let listing = op.get("list").is_some();
        if said.is_empty() && !listing {
            return Err("scissor does nothing without cuts (draw), auto (find), cut + a change, remove or list".into());
        }
        if !said.is_empty() {
            let seq = self.doc.seq();
            let ok = self.doc.apply("Scissor", m::Edit::SetClip { seq, clip: Box::new(mc.clone()) });
            self.check(ok, "")?;
        }
        let mc = self.model_clip(id).ok_or("clip gone")?;
        let has_bend = mc.puppet.as_ref().is_some_and(|p| !p.pins.is_empty()) || mc.skeleton.is_some();
        Ok(format!(
            "{}{}cuts now (canvas px at {:.2}s): {}{}",
            said.join("; "),
            if said.is_empty() { "" } else { "; " },
            super::secs(at),
            json!(self.cuts_said(id, &mc, at)),
            if has_bend { "" } else { ". Cuts split the mesh a puppet or skeleton bends: add pins (puppet) or bones (skeleton) to move the sides apart" }
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> (Session, u32) {
        let (doc, id) = crate::services::character::tests::figure_doc();
        let mut s = Session::new(doc);
        s.home = Some(std::env::temp_dir().join(format!("studio-tests/scissor-session-{}-{id}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos())));
        (s, id)
    }

    fn run(s: &mut Session, op: Value) -> Result<String, String> {
        s.edit(&[op], "t", false).map(|v| v["notes"].as_array().map(|n| n.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("; ")).unwrap_or_default())
    }

    fn cuts(s: &Session, id: u32) -> Vec<m::Cut> {
        s.model_clip(id).unwrap().cuts
    }

    #[test]
    fn rigging_a_person_cuts_the_legs_and_the_mouth() {
        let (mut s, id) = session();
        let r = run(&mut s, json!({"op": "puppet", "clip": id, "rig": "biped", "facing": "right"})).unwrap();
        assert!(r.contains("cut the mesh so parts move apart") && r.contains("legs"), "{r}");
        let c = cuts(&s, id);
        let legs = c.iter().find(|c| c.name == "legs").expect("the gap between the legs");
        // The legs stand at columns 40..49 and 54..63 of 100: the gap is
        // the strip between them, from the crotch (row 80 of 140) down.
        assert!(legs.points.iter().all(|p| (p.x - 0.515).abs() < 0.04), "{:?}", legs.points);
        assert!((legs.points[0].y - 80.0 / 140.0).abs() < 0.06, "from the crotch: {:?}", legs.points[0]);
        assert!(legs.points.last().unwrap().y > 0.9);
        let mouth = c.iter().find(|c| c.name == "mouth").expect("the drawn mouth is found");
        assert!(matches!(mouth.fill, m::CutFill::Colour(_)), "a mouth fills its inside");
        assert!(mouth.points.iter().all(|p| (p.y - 20.0 / 140.0).abs() < 0.03), "{:?}", mouth.points);
        // Found cuts are tagged; finding again replaces them.
        assert!(c.iter().all(is_auto));
        let n = c.len();
        run(&mut s, json!({"op": "puppet", "clip": id, "rig": "biped", "facing": "right"})).unwrap();
        assert_eq!(cuts(&s, id).len(), n);
        // And it can be turned off.
        run(&mut s, json!({"op": "scissor", "clip": id, "remove": "all"})).unwrap();
        run(&mut s, json!({"op": "puppet", "clip": id, "rig": "biped", "facing": "right", "auto_cut": false})).unwrap();
        assert!(cuts(&s, id).is_empty());
    }

    #[test]
    fn a_skeleton_cuts_what_it_separates() {
        let (mut s, id) = session();
        let r = run(&mut s, json!({"op": "skeleton", "clip": id, "build": "person", "facing": "right"})).unwrap();
        assert!(r.contains("cut the mesh"), "{r}");
        assert!(cuts(&s, id).iter().any(|c| c.name == "legs"));
        // Parts' borders join the gaps: the legs' own border is the gap, so
        // it is not cut twice.
        let names: Vec<String> = cuts(&s, id).iter().map(|c| c.name.clone()).collect();
        assert!(names.iter().all(|n| !n.starts_with("torso/")), "a limb's joint stays joined: {names:?}");
    }

    #[test]
    fn cuts_are_drawn_changed_filled_and_removed() {
        let (mut s, id) = session();
        let r = run(&mut s, json!({"op": "scissor", "clip": id, "cuts": [{"name": "tail", "path": [{"u": 0.1, "v": 0.5}, {"u": 0.2, "v": 0.7}], "fill": "#400808"}]})).unwrap();
        assert!(r.contains("cut 1"), "{r}");
        let c = cuts(&s, id);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].points[1], m::Vec2::new(0.2, 0.7));
        assert!(matches!(c[0].fill, m::CutFill::Colour(k) if (k.r - 64.0 / 255.0).abs() < 1e-6));
        // Same name replaces; fill and enabled change one.
        run(&mut s, json!({"op": "scissor", "clip": id, "cuts": [{"name": "tail", "path": [{"u": 0.1, "v": 0.4}, {"u": 0.1, "v": 0.8}]}]})).unwrap();
        assert_eq!(cuts(&s, id).len(), 1);
        let cid = cuts(&s, id)[0].id;
        run(&mut s, json!({"op": "scissor", "clip": id, "cut": "tail", "fill": "auto", "name": "tail root"})).unwrap();
        assert_eq!(cuts(&s, id)[0].fill, m::CutFill::Auto);
        assert_eq!(cuts(&s, id)[0].name, "tail root");
        run(&mut s, json!({"op": "scissor", "clip": id, "cut": cid, "enabled": false})).unwrap();
        assert!(!cuts(&s, id)[0].enabled);
        let listed = run(&mut s, json!({"op": "scissor", "clip": id, "list": true})).unwrap();
        assert!(listed.contains("tail root"), "{listed}");
        // Mistakes say what to do.
        assert!(run(&mut s, json!({"op": "scissor", "clip": id, "cuts": [{"name": "x", "path": [{"u": 0.1, "v": 0.5}]}]})).is_err());
        assert!(run(&mut s, json!({"op": "scissor", "clip": id, "cut": "nobody", "fill": "auto"})).unwrap_err().contains("no cut"));
        assert!(run(&mut s, json!({"op": "scissor", "clip": id, "auto": "elbows"})).unwrap_err().contains("legs, arms"));
        assert!(run(&mut s, json!({"op": "scissor", "clip": id})).is_err());
        run(&mut s, json!({"op": "scissor", "clip": id, "remove": "tail root"})).unwrap();
        assert!(cuts(&s, id).is_empty());
        // Cuts ride along with the puppet op.
        run(&mut s, json!({"op": "puppet", "clip": id, "pins": [{"at": {"u": 0.5, "v": 0.2}}], "cuts": [[{"u": 0.1, "v": 0.5}, {"u": 0.9, "v": 0.5}]]})).unwrap();
        assert_eq!(cuts(&s, id).len(), 1);
    }

    #[test]
    fn auto_on_a_chosen_part_of_the_body() {
        let (mut s, id) = session();
        let r = run(&mut s, json!({"op": "scissor", "clip": id, "auto": ["legs"]})).unwrap();
        assert!(r.contains("legs"), "{r}");
        assert_eq!(cuts(&s, id).len(), 1);
        let r = run(&mut s, json!({"op": "scissor", "clip": id, "auto": "mouth"})).unwrap();
        assert!(r.contains("mouth"), "{r}");
        assert_eq!(cuts(&s, id).iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["mouth"]);
    }
}
