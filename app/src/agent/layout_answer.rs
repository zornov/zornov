// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The `layout` tool's answers, and the short summaries other replies
//! carry (`inspect`, `see`). Compact by default: one short object a thing,
//! front to back; detail on request.

use super::grid::{self, Rect};
use super::{Item, Kind, Layout, Opts, Role, SUMMARY_GRID_W, sec};
use crate::agent::guides;
use crate::agent::{Session, f, micros, secs};
use crate::view::{self, SEC};
use serde_json::{Value, json};

/// Most things named in a compact whole-frame answer.
const COMPACT_ITEMS: usize = 24;
/// Overlapping pairs named.
const COMPACT_PAIRS: usize = 8;

fn ri(v: f32) -> i64 {
    v.round() as i64
}

fn box_json(b: &Rect) -> Value {
    json!([ri(b[0]), ri(b[1]), ri(b[2]), ri(b[3])])
}

fn pct(v: f32) -> f64 {
    (v as f64 * 100.0).round() / 100.0
}

/// Pixels in from each side of the frame that title-safe and action-safe
/// leave (the viewer's guides).
fn safe_inset(frac: f32) -> f32 {
    ((1.0 - frac) / 2.0).max(0.0)
}

/// Two items that meet.
#[derive(Clone, Debug)]
pub(super) struct Pair {
    /// Lower and upper (draw order).
    pub below: usize,
    pub above: usize,
    /// Share of the smaller one the other overlaps.
    pub share: f32,
}

/// Which layers belong together, so their meeting is not a problem: a rig
/// and its parts, parts of one rig, objects of one world.
fn kin(a: &Item, b: &Item) -> bool {
    a.of.is_some() && (a.of == b.of || a.of == Some(b.id)) || b.of.is_some() && b.of == Some(a.id) || (a.object.is_some() && b.object.is_some() && a.id == b.id) || (a.kind == Kind::WorldObject || b.kind == Kind::WorldObject)
}

impl Layout {
    pub(super) fn frame(&self) -> Rect {
        [-self.w / 2.0, -self.h / 2.0, self.w / 2.0, self.h / 2.0]
    }

    pub(super) fn index_of(&self, id: u64, object: Option<usize>) -> Option<usize> {
        self.items.iter().position(|i| i.id == id && i.object == object)
    }

    fn label(&self, i: usize) -> String {
        let it = &self.items[i];
        match it.object {
            Some(o) => format!("{} (object {o} of world {})", it.name, it.id),
            None => format!("clip {} (\"{}\")", it.id, it.name),
        }
    }

    /// Items that meet by what is drawn, among things (not backdrops).
    pub(super) fn pairs(&self, min_share: f32) -> Vec<Pair> {
        let mut out = Vec::new();
        for (i, a) in self.items.iter().enumerate() {
            let Some(ga) = a.grid.as_ref().filter(|_| a.role != Role::Backdrop) else { continue };
            for (j, b) in self.items.iter().enumerate().skip(i + 1) {
                let Some(gb) = b.grid.as_ref().filter(|_| b.role != Role::Backdrop) else { continue };
                if kin(a, b) {
                    continue;
                }
                let share = grid::overlap_share(ga, gb);
                if share >= min_share {
                    out.push(Pair { below: i, above: j, share });
                }
            }
        }
        out.sort_by(|a, b| b.share.total_cmp(&a.share));
        out
    }

    /// What is wrong or notable about one item, in words.
    pub(super) fn flags(&self, i: usize) -> Vec<String> {
        let it = &self.items[i];
        let mut out = Vec::new();
        if it.role == Role::Backdrop || it.kind == Kind::WorldObject {
            return out;
        }
        let frame = self.frame();
        if it.bounds.is_none() && it.opacity > 0.02 {
            if it.on_frame <= 0.0 {
                out.push("off_screen".to_owned());
            }
            return out;
        }
        let Some(b) = it.bounds else { return out };
        let sides = ["left", "top", "right", "bottom"];
        let cut: Vec<&str> = (0..4).filter(|&s| it.touches[s] && it.spill[s] > 1.0).map(|s| sides[s]).collect();
        for s in &cut {
            out.push(format!("cut_{s}"));
        }
        if cut.is_empty() && it.on_frame < 0.98 && it.on_frame > 0.0 && it.from_alpha {
            // The box leaves the frame but the picture inside it does not.
        }
        if !it.from_alpha {
            // Only the box is known: nothing to say about what covers it.
        } else if it.visible < 0.15 && it.opacity > 0.2 {
            out.push("hidden".to_owned());
        } else if it.visible < 0.6 {
            out.push("partly_hidden".to_owned());
        }
        let (inner, name) = if it.role == Role::Text { (grid::inset(&frame, safe_inset(guides::TITLE_SAFE)), "outside_title_safe") } else { (grid::inset(&frame, safe_inset(guides::ACTION_SAFE)), "outside_action_safe") };
        if cut.is_empty() && it.role != Role::Part && (b[0] < inner[0] - 1.0 || b[1] < inner[1] - 1.0 || b[2] > inner[2] + 1.0 || b[3] > inner[3] + 1.0) {
            out.push(name.to_owned());
        }
        out
    }

    /// The canvas the things leave empty: the `k` biggest free boxes
    /// (canvas px), their margin kept, leaving out `ignore`.
    pub(super) fn free_boxes(&self, ignore: &[u64], margin: f32, min: (f32, f32), k: usize) -> Vec<Rect> {
        let free = self.free_cells(ignore, margin);
        let (mw, mh) = (((min.0 / self.cell).ceil() as usize).max(1), ((min.1 / self.cell).ceil() as usize).max(1));
        grid::free_rects(&free, self.gw, self.gh, mw, mh, k).into_iter().map(|r| self.cells_to_canvas(r)).collect()
    }

    fn cells_to_canvas(&self, r: [usize; 4]) -> Rect {
        [r[0] as f32 * self.cell - self.w / 2.0, r[1] as f32 * self.cell - self.h / 2.0, r[2] as f32 * self.cell - self.w / 2.0, r[3] as f32 * self.cell - self.h / 2.0]
    }

    /// Cells nothing stands on (backdrops do not count as standing).
    pub(super) fn free_cells(&self, ignore: &[u64], margin: f32) -> Vec<bool> {
        let mut occ = vec![false; self.gw * self.gh];
        for it in self.items.iter().filter(|i| i.role != Role::Backdrop && i.opacity > 0.05 && !ignore.contains(&i.id)) {
            let Some(g) = &it.grid else { continue };
            for (o, a) in occ.iter_mut().zip(&g.a) {
                if *a >= grid::SOLID / 2 {
                    *o = true;
                }
            }
        }
        grid::empty_cells(&occ, self.gw, self.gh, (margin / self.cell).ceil() as usize)
    }

    /// The weight of what stands on the backdrop.
    pub(super) fn balance(&self) -> Option<grid::Balance> {
        let grids: Vec<&grid::Grid> = self.items.iter().filter(|i| i.role != Role::Backdrop && i.opacity > 0.05).filter_map(|i| i.grid.as_ref()).collect();
        grid::balance(&grids, self.gw, self.gh)
    }

    pub(super) fn balance_json(&self) -> Option<Value> {
        let b = self.balance()?;
        let (cx, cy) = (b.centre.0 * self.cell - self.w / 2.0, b.centre.1 * self.cell - self.h / 2.0);
        let mut lean = Vec::new();
        if (cx / self.w).abs() > 0.08 {
            lean.push(if cx < 0.0 { "left-heavy" } else { "right-heavy" });
        }
        if (cy / self.h).abs() > 0.08 {
            lean.push(if cy < 0.0 { "top-heavy" } else { "bottom-heavy" });
        }
        Some(json!({"centre": [ri(cx), ri(cy)], "left": pct(b.left), "right": pct(b.right), "top": pct(b.top), "bottom": pct(b.bottom), "fills": pct(b.weight), "lean": if lean.is_empty() { "balanced".to_owned() } else { lean.join(", ") }}))
    }

    /// One item as the compact object: who and where, in front or behind,
    /// and what is wrong.
    pub(super) fn item_json(&self, i: usize, detail: bool) -> Value {
        let it = &self.items[i];
        let mut o = json!({"id": it.id, "name": it.name, "is": it.kind.word(), "z": it.z});
        if let Some(n) = it.object {
            o["object"] = json!(n);
        }
        match it.bounds {
            Some(b) => {
                o["box"] = box_json(&b);
                if !it.from_alpha {
                    o["box_from"] = json!("transform");
                }
            }
            None => o["box"] = Value::Null,
        }
        if it.from_alpha {
            o["vis"] = json!(pct(it.visible));
        }
        if let Some(d) = it.depth {
            o["depth"] = json!(ri(d));
        }
        if it.rotation.abs() > 0.5 {
            o["rot"] = json!(ri(it.rotation));
        }
        if it.opacity < 0.99 {
            o["opacity"] = json!(pct(it.opacity));
        }
        if it.role == Role::Text {
            o["text"] = json!(it.what.chars().take(40).collect::<String>());
            if !it.lines.is_empty() {
                o["lines"] = if detail { json!(it.lines.iter().map(box_json).collect::<Vec<_>>()) } else { json!(it.lines.len()) };
            }
        }
        if it.of.is_some() {
            o["of"] = json!(it.of);
        }
        if let Some(fc) = it.facing {
            o["faces"] = json!(fc);
        }
        // A figure's head and hands are what titles and eyes care about.
        let wanted: &[&str] = if detail { &[] } else { &["head", "mouth"] };
        let parts: Vec<Value> = it.parts.iter().filter(|p| wanted.is_empty() || wanted.contains(&p.name.as_str())).map(|p| json!({"part": p.name, "at": [ri(p.at.0), ri(p.at.1)]})).collect();
        if !parts.is_empty() {
            o["parts"] = json!(parts);
        }
        let over: Vec<Value> = it.covers.iter().map(|&(j, s)| if detail { json!({"id": self.items[j].id, "hides": pct(s)}) } else { json!(self.items[j].id) }).collect();
        let under: Vec<Value> = it.covered_by.iter().map(|&(j, s)| if detail { json!({"id": self.items[j].id, "hides": pct(s)}) } else { json!(self.items[j].id) }).collect();
        if !over.is_empty() {
            o["over"] = json!(over);
        }
        if !under.is_empty() {
            o["under"] = json!(under);
        }
        let flags = self.flags(i);
        if !flags.is_empty() {
            o["flags"] = json!(flags);
        }
        if detail {
            o["corners"] = json!(it.quad.iter().map(|p| [ri(p.0), ri(p.1)]).collect::<Vec<_>>());
            if let Some(r) = it.reach {
                o["reach"] = box_json(&r);
            }
            if let Some((rx, ry)) = it.tilt {
                o["tilt_3d"] = json!([ri(rx), ri(ry)]);
            }
            if it.on_frame < 0.999 {
                o["box_in_frame"] = json!(pct(it.on_frame));
            }
            if it.transition {
                o["in_transition"] = json!(true);
            }
        }
        if !it.labels.is_empty() {
            o["labels"] = json!(it.labels.iter().take(if detail { 20 } else { 4 }).map(|l| json!({"label": l.name, "box": box_json(&l.canvas), "score": pct(l.score)})).collect::<Vec<_>>());
        }
        o
    }

    fn pairs_json(&self, pairs: &[Pair], limit: usize) -> Vec<Value> {
        pairs.iter().take(limit).map(|p| json!({"front": self.items[p.above].id, "back": self.items[p.below].id, "names": [self.items[p.above].name, self.items[p.below].name], "share": pct(p.share)})).collect()
    }

    /// The whole frame, compactly.
    pub(super) fn frame_json(&self, detail: bool) -> Value {
        // Front to back, so what is in front reads first.
        let mut order: Vec<usize> = (0..self.items.len()).rev().filter(|&i| self.items[i].role != Role::Backdrop && (detail || self.items[i].role != Role::Part)).collect();
        let total = order.len();
        let limit = if detail { 80 } else { COMPACT_ITEMS };
        order.truncate(limit);
        let backdrops: Vec<Value> = self.items.iter().filter(|i| i.role == Role::Backdrop).map(|i| json!({"id": i.id, "name": i.name, "is": i.kind.word()})).collect();
        let pairs = self.pairs(0.05);
        let mut out = json!({
            "at": sec(self.t),
            "frame": [ri(self.w), ri(self.h)],
            "items": order.iter().map(|&i| self.item_json(i, detail)).collect::<Vec<_>>(),
        });
        if total > limit {
            out["more"] = json!(format!("{} more things not listed: ask canvas_layout with clip, view: \"overlaps\" or a smaller range", total - limit));
        }
        if !backdrops.is_empty() {
            out["backdrops"] = json!(backdrops);
        }
        if !pairs.is_empty() {
            out["overlaps"] = json!(self.pairs_json(&pairs, COMPACT_PAIRS));
        }
        let free = self.free_boxes(&[], self.w * 0.015, (self.w * 0.12, self.h * 0.12), 3);
        if !free.is_empty() {
            out["free"] = json!(free.iter().map(|r| json!({"box": box_json(r), "size": [ri(r[2] - r[0]), ri(r[3] - r[1])]})).collect::<Vec<_>>());
        }
        if let Some(b) = self.balance_json() {
            out["balance"] = b;
        }
        if let Some(c) = &self.camera {
            out["camera"] = c.clone();
        }
        out
    }

    /// The nearest things to item `i`: gap in pixels and which side.
    pub(super) fn near_json(&self, i: usize, k: usize) -> Vec<Value> {
        let Some(a) = self.items[i].bounds else { return Vec::new() };
        let mut near: Vec<(f32, usize, &'static str)> = Vec::new();
        for (j, o) in self.items.iter().enumerate() {
            if j == i || o.role == Role::Backdrop || o.kind == Kind::WorldObject || kin(&self.items[i], o) {
                continue;
            }
            let Some(b) = o.bounds else { continue };
            let (gx, gy) = ((b[0] - a[2]).max(a[0] - b[2]).max(0.0), (b[1] - a[3]).max(a[1] - b[3]).max(0.0));
            let side = if gx >= gy { if b[0] >= a[2] { "right" } else { "left" } } else if b[1] >= a[3] { "below" } else { "above" };
            let gap = if gx == 0.0 && gy == 0.0 { 0.0 } else { (gx * gx + gy * gy).sqrt() };
            near.push((gap, j, side));
        }
        near.sort_by(|x, y| x.0.total_cmp(&y.0));
        near.into_iter().take(k).map(|(gap, j, side)| json!({"id": self.items[j].id, "name": self.items[j].name, "gap": ri(gap), "side": if gap == 0.0 { "touching" } else { side }})).collect()
    }
}

/// Parses `[x, y]` or `{"x":, "y":}`.
fn point(v: &Value) -> Option<(f32, f32)> {
    match v {
        Value::Array(a) if a.len() == 2 => Some((a[0].as_f64()? as f32, a[1].as_f64()? as f32)),
        Value::Object(_) => Some((v.get("x")?.as_f64()? as f32, v.get("y")?.as_f64()? as f32)),
        _ => None,
    }
}

impl Session {
    /// A moment worth looking at when none was named: the one with most
    /// things on screen.
    fn layout_default_time(&self) -> view::Time {
        let p = &self.doc.project;
        let mut best = (0usize, 0 as view::Time);
        for c in p.clips.iter().filter(|c| c.kind != view::ClipKind::Audio) {
            let t = c.start + c.dur / 2;
            let n = p.visual_stack_at(t).len();
            if n > best.0 {
                best = (n, t);
            }
        }
        best.1
    }

    /// An item named by id or by name.
    fn layout_pick(&self, lay: &Layout, v: &Value, object: Option<usize>) -> Result<usize, String> {
        let want: Option<u64> = v.as_u64().or_else(|| v.as_str().and_then(|s| s.trim().parse::<u64>().ok()));
        if let Some(id) = want {
            return lay.index_of(id, object).or_else(|| lay.items.iter().position(|i| i.id == id)).ok_or_else(|| format!("clip {id} draws nothing at {:.2}s (not on screen then: ask at another moment, or inspect it)", secs(lay.t)));
        }
        let name = v.as_str().ok_or("clip is an id or a name")?.to_lowercase();
        lay.items.iter().rposition(|i| i.name.to_lowercase() == name).or_else(|| lay.items.iter().rposition(|i| i.name.to_lowercase().contains(&name))).ok_or_else(|| format!("nothing called \"{name}\" is on screen at {:.2}s", secs(lay.t)))
    }

    /// `layout`: where everything is.
    #[cfg(feature = "render")]
    pub(in crate::agent) fn layout_tool(&mut self, a: &Value) -> Result<String, String> {
        let view = a.get("view").and_then(Value::as_str).unwrap_or("frame").to_lowercase();
        let detail = a.get("detail").and_then(Value::as_bool).unwrap_or(false);
        let grid = a.get("grid").and_then(Value::as_u64).map(|g| (g as usize).clamp(48, 640)).unwrap_or(0);
        let t = f(a, "t").or_else(|| f(a, "at")).map(micros).unwrap_or_else(|| self.layout_default_time());
        let range = match (f(a, "from"), f(a, "to")) {
            (Some(x), Some(y)) if y > x => Some((micros(x), micros(y))),
            _ => None,
        };
        if let Some((from, to)) = range {
            if !matches!(view.as_str(), "item" | "clip") {
                return self.layout_range(from, to, a, grid).map(|v| v.to_string());
            }
        }
        let object = a.get("object").and_then(Value::as_u64).map(|o| o as usize);
        let focus: Vec<u64> = match view.as_str() {
            "item" | "clip" => a.get("clip").and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))).into_iter().collect(),
            _ => Vec::new(),
        };
        let geometry_only = a.get("draw").and_then(Value::as_bool) == Some(false);
        let mut lay = self.layout_at(t, &Opts { grid, focus: focus.clone(), geometry_only })?;
        if lay.items.is_empty() {
            return Ok(json!({"at": sec(t), "frame": [ri(lay.w), ri(lay.h)], "items": [], "note": "nothing is drawn at this moment"}).to_string());
        }
        let out = match view.as_str() {
            "frame" | "all" | "" => lay.frame_json(detail),
            "item" | "clip" => {
                let v = a.get("clip").ok_or("view \"item\" needs \"clip\" (an id or a name)")?;
                let i = self.layout_pick(&lay, v, object)?;
                // A name: draw that item properly now it is known.
                if focus.is_empty() {
                    lay = self.layout_at(t, &Opts { grid, focus: vec![lay.items[i].id], geometry_only })?;
                }
                let i = self.layout_pick(&lay, v, object)?;
                let mut o = lay.item_json(i, true);
                o["at"] = json!(sec(t));
                o["near"] = json!(lay.near_json(i, 4));
                if lay.items[i].object.is_none() {
                    let (c0, c1) = self.doc.project.clip(lay.items[i].id as u32).map_or((t, t), |c| (c.start, c.start + c.dur));
                    let (from, to) = range.unwrap_or((c0.max(t - 2 * SEC), c1.min(t + 2 * SEC)));
                    if to > from {
                        if let Some(m) = self.layout_motion(from, to, 8).get(&(lay.items[i].id, lay.items[i].object)) {
                            o["motion"] = motion_json(m, from, to);
                        }
                    }
                }
                o
            }
            "overlaps" | "collisions" => {
                let pairs = lay.pairs(0.01);
                let flagged: Vec<Value> = (0..lay.items.len()).filter(|&i| !lay.flags(i).is_empty()).map(|i| json!({"id": lay.items[i].id, "name": lay.items[i].name, "flags": lay.flags(i)})).take(40).collect();
                json!({"at": sec(t), "overlaps": lay.pairs_json(&pairs, 40), "problems": flagged})
            }
            "free" | "space" | "balance" => {
                let margin = f(a, "margin").unwrap_or(lay.w as f64 * 0.015) as f32;
                let ignore: Vec<u64> = super::super::ids(a, "ignore").into_iter().map(u64::from).collect();
                let free = lay.free_boxes(&ignore, margin, (lay.w * 0.08, lay.h * 0.08), 6);
                json!({"at": sec(t), "frame": [ri(lay.w), ri(lay.h)], "free": free.iter().map(|r| json!({"box": box_json(r), "size": [ri(r[2] - r[0]), ri(r[3] - r[1])], "centre": [ri((r[0] + r[2]) / 2.0), ri((r[1] + r[3]) / 2.0)]})).collect::<Vec<_>>(), "balance": lay.balance_json(), "title_safe": box_json(&grid::inset(&lay.frame(), safe_inset(guides::TITLE_SAFE))), "action_safe": box_json(&grid::inset(&lay.frame(), safe_inset(guides::ACTION_SAFE)))})
            }
            "fit" | "where" => self.layout_fit(&lay, a)?,
            "safe" => {
                let flagged: Vec<Value> = (0..lay.items.len()).filter(|&i| lay.flags(i).iter().any(|f| f.starts_with("outside_") || f.starts_with("cut_") || f == "off_screen")).map(|i| json!({"id": lay.items[i].id, "name": lay.items[i].name, "box": lay.items[i].bounds.map(|b| box_json(&b)), "flags": lay.flags(i)})).collect();
                json!({"at": sec(t), "title_safe": box_json(&grid::inset(&lay.frame(), safe_inset(guides::TITLE_SAFE))), "action_safe": box_json(&grid::inset(&lay.frame(), safe_inset(guides::ACTION_SAFE))), "violations": flagged})
            }
            other => return Err(format!("canvas_layout view \"{other}\": frame (default), item, overlaps, free, fit, safe")),
        };
        Ok(out.to_string())
    }

    /// Where would a box of some size fit.
    fn layout_fit(&self, lay: &Layout, a: &Value) -> Result<Value, String> {
        let ignore: Vec<u64> = super::super::ids(a, "ignore").into_iter().map(u64::from).collect();
        // The size: given, or the same as another thing's.
        let mut size: Option<(f32, f32)> = a.get("size").and_then(point).or_else(|| Some((f(a, "width")? as f32, f(a, "height")? as f32)));
        if size.is_none() {
            if let Some(like) = a.get("like") {
                let i = self.layout_pick(lay, like, None)?;
                let b = lay.items[i].bounds.ok_or("that item shows nothing to take the size of")?;
                let k = f(a, "scale").unwrap_or(1.0) as f32;
                size = Some(((b[2] - b[0]) * k, (b[3] - b[1]) * k));
            }
        }
        let (nw, nh) = size.ok_or("fit needs \"size\": [width, height] in pixels, or \"like\": a clip to take the size of")?;
        let margin = f(a, "margin").unwrap_or(lay.w as f64 * 0.015) as f32;
        let region = match a.get("within").and_then(Value::as_str).unwrap_or("title_safe") {
            "frame" => lay.frame(),
            "action_safe" | "action" => grid::inset(&lay.frame(), safe_inset(guides::ACTION_SAFE)),
            _ => grid::inset(&lay.frame(), safe_inset(guides::TITLE_SAFE)),
        };
        // Where it would like to be.
        let frame = lay.frame();
        let third_w = (region[2] - region[0]) / 3.0;
        let third_h = (region[3] - region[1]) / 3.0;
        let (mut px, mut py) = ((region[0] + region[2]) / 2.0, (region[1] + region[3]) / 2.0);
        if let Some(n) = a.get("near") {
            if let Some(p) = point(n) {
                (px, py) = p;
            } else {
                let i = self.layout_pick(lay, n, None)?;
                (px, py) = lay.items[i].centre();
            }
        } else if let Some(w) = a.get("prefer").and_then(Value::as_str) {
            match w {
                "top" | "upper" | "upper_third" => py = region[1] + third_h / 2.0,
                "bottom" | "lower" | "lower_third" => py = region[3] - third_h / 2.0,
                "left" => px = region[0] + third_w / 2.0,
                "right" => px = region[2] - third_w / 2.0,
                "centre" | "center" | "middle" => {}
                other => return Err(format!("prefer \"{other}\": top, bottom, left, right, centre, upper_third, lower_third")),
            }
        }
        let free = lay.free_cells(&ignore, margin);
        let cell = lay.cell;
        let to_cell = |x: f32, y: f32| ((x + frame[2]) / cell, (y + frame[3]) / cell);
        let (c0, c1) = (to_cell(region[0], region[1]), to_cell(region[2], region[3]));
        let within = [c0.0.ceil().max(0.0) as usize, c0.1.ceil().max(0.0) as usize, (c1.0.floor() as usize).min(lay.gw), (c1.1.floor() as usize).min(lay.gh)];
        let need = ((nw / cell).ceil() as usize, (nh / cell).ceil() as usize);
        let spots = grid::fit(&free, lay.gw, lay.gh, need.0, need.1, within, to_cell(px, py), 3);
        if !spots.is_empty() {
            return Ok(json!({
                "at": sec(lay.t),
                "fits": true,
                "size": [ri(nw), ri(nh)],
                "spots": spots.iter().map(|r| {
                    let c = lay.cells_to_canvas(*r);
                    // The box asked for, centred in the cells found.
                    let (cx, cy) = ((c[0] + c[2]) / 2.0, (c[1] + c[3]) / 2.0);
                    json!({"box": box_json(&[cx - nw / 2.0, cy - nh / 2.0, cx + nw / 2.0, cy + nh / 2.0]), "centre": [ri(cx), ri(cy)]})
                }).collect::<Vec<_>>(),
            }));
        }
        // Nothing that size: the biggest gap, and how much smaller fits.
        let best = grid::free_rects(&free, lay.gw, lay.gh, 1, 1, 8).into_iter().map(|r| lay.cells_to_canvas(r)).map(|r| (r, ((r[2] - r[0]) / nw).min((r[3] - r[1]) / nh))).max_by(|x, y| x.1.total_cmp(&y.1));
        Ok(match best {
            Some((r, k)) => json!({"at": sec(lay.t), "fits": false, "size": [ri(nw), ri(nh)], "largest_free": {"box": box_json(&r), "size": [ri(r[2] - r[0]), ri(r[3] - r[1])]}, "fits_at_scale": pct(k.max(0.0)), "note": "the free space is smaller than that: scale to fits_at_scale, move something, or place it over a backdrop-only area"}),
            None => json!({"at": sec(lay.t), "fits": false, "size": [ri(nw), ri(nh)], "note": "no free space: everything is covered"}),
        })
    }

    /// Several moments across a range: who is there, how they move, and
    /// where they meet.
    #[cfg(feature = "render")]
    fn layout_range(&mut self, from: view::Time, to: view::Time, a: &Value, grid_w: usize) -> Result<Value, String> {
        let n = (f(a, "samples").unwrap_or(5.0) as usize).clamp(2, 12);
        let mut seen: Vec<(u64, Option<usize>, String, &'static str, Role, f32, usize)> = Vec::new();
        let mut meets: Vec<((u64, u64), f32, f64, String)> = Vec::new();
        let mut flagged: Vec<(u64, String, f64)> = Vec::new();
        let mut shown: Vec<Value> = Vec::new();
        for k in 0..n {
            let t = from + ((to - from) as f64 * k as f64 / (n - 1) as f64) as view::Time;
            let lay = self.layout_at(t.min(to - 1).max(0), &Opts { grid: if grid_w == 0 { SUMMARY_GRID_W } else { grid_w }, ..Opts::default() })?;
            for (i, it) in lay.items.iter().enumerate() {
                if it.role == Role::Backdrop {
                    continue;
                }
                match seen.iter_mut().find(|s| s.0 == it.id && s.1 == it.object) {
                    Some(s) => {
                        s.5 = s.5.min(it.visible);
                        s.6 += 1;
                    }
                    None => seen.push((it.id, it.object, it.name.clone(), it.kind.word(), it.role, it.visible, 1)),
                }
                for fl in lay.flags(i) {
                    if !flagged.iter().any(|f| f.0 == it.id && f.1 == fl) {
                        flagged.push((it.id, fl, secs(t)));
                    }
                }
            }
            for p in lay.pairs(0.05) {
                let key = (lay.items[p.above].id, lay.items[p.below].id);
                let names = format!("{} / {}", lay.items[p.above].name, lay.items[p.below].name);
                match meets.iter_mut().find(|m| m.0 == key) {
                    Some(m) => m.1 = m.1.max(p.share),
                    None => meets.push((key, p.share, secs(t), names)),
                }
            }
            shown.push(json!({"t": sec(t), "things": lay.items.iter().filter(|i| i.role != Role::Backdrop && i.role != Role::Part).count()}));
        }
        let motion = self.layout_motion(from, to, 8);
        let items: Vec<Value> = seen
            .iter()
            .filter(|s| s.4 != Role::Part)
            .take(40)
            .map(|s| {
                let mut o = json!({"id": s.0, "name": s.2, "is": s.3, "seen": format!("{}/{} moments", s.6, n), "least_visible": pct(s.5)});
                if let Some(m) = motion.get(&(s.0, s.1)) {
                    o["motion"] = motion_json(m, from, to);
                }
                o
            })
            .collect();
        Ok(json!({
            "from": sec(from),
            "to": sec(to),
            "moments": n,
            "items": items,
            "meet": meets.iter().take(12).map(|m| json!({"front": (m.0).0, "back": (m.0).1, "names": m.3, "worst_share": pct(m.1), "first_seen": m.2})).collect::<Vec<_>>(),
            "problems": flagged.iter().take(20).map(|f| json!({"id": f.0, "flag": f.1, "first_seen": f.2})).collect::<Vec<_>>(),
        }))
    }

    /// What `inspect` adds for one clip: where its pixels are and what it
    /// is under or over. A few dozen tokens.
    #[cfg(feature = "render")]
    pub(in crate::agent) fn layout_of(&mut self, id: u32, t: view::Time) -> Option<Value> {
        let lay = self.layout_at(t, &Opts { grid: SUMMARY_GRID_W, focus: vec![id as u64], ..Opts::default() }).ok()?;
        let i = lay.index_of(id as u64, None)?;
        let it = &lay.items[i];
        let mut o = json!({});
        if let Some(b) = it.bounds {
            o["box"] = box_json(&b);
            o["box_from"] = json!(if it.from_alpha { "alpha" } else { "transform" });
        } else {
            o["box"] = Value::Null;
        }
        if it.from_alpha {
            o["visible"] = json!(pct(it.visible));
        }
        o["z"] = json!(format!("{} of {}", it.z + 1, lay.items.len()));
        let named = |l: &[(usize, f32)]| l.iter().take(4).map(|&(j, s)| json!(format!("{} \"{}\" {:.0}%", lay.items[j].id, lay.items[j].name, s * 100.0))).collect::<Vec<_>>();
        if !it.covered_by.is_empty() {
            o["under"] = json!(named(&it.covered_by));
        }
        if !it.covers.is_empty() {
            o["over"] = json!(named(&it.covers));
        }
        if let Some(fc) = it.facing {
            o["faces"] = json!(fc);
        }
        if let Some(h) = it.parts.iter().find(|p| p.name == "head") {
            o["head"] = json!([ri(h.at.0), ri(h.at.1)]);
        }
        let flags = lay.flags(i);
        if !flags.is_empty() {
            o["flags"] = json!(flags);
        }
        if let Some(d) = it.depth {
            o["depth"] = json!(ri(d));
        }
        o["at"] = json!(sec(t));
        o["more"] = json!("canvas_layout {\"view\":\"item\", \"clip\": id} for parts, motion and neighbours");
        Some(o)
    }

    /// The line `see` adds for a moment: how many things, what meets, what
    /// is cut, where there is room.
    #[cfg(feature = "render")]
    pub(in crate::agent) fn layout_line(&mut self, t: view::Time) -> Option<String> {
        let lay = self.layout_at(t, &Opts { grid: SUMMARY_GRID_W, ..Opts::default() }).ok()?;
        let things: Vec<usize> = (0..lay.items.len()).filter(|&i| !matches!(lay.items[i].role, Role::Backdrop | Role::Part) && lay.items[i].kind != Kind::WorldObject).collect();
        if things.is_empty() {
            return None;
        }
        let mut parts = vec![format!("{} things", things.len())];
        let pairs = lay.pairs(0.1);
        if !pairs.is_empty() {
            parts.push(format!("overlap: {}", pairs.iter().take(3).map(|p| format!("{} over {} {:.0}%", lay.items[p.above].id, lay.items[p.below].id, p.share * 100.0)).collect::<Vec<_>>().join(", ")));
        }
        let flagged: Vec<String> = things.iter().filter_map(|&i| lay.flags(i).first().map(|f| format!("{} {f}", lay.items[i].id))).take(4).collect();
        if !flagged.is_empty() {
            parts.push(format!("flags: {}", flagged.join(", ")));
        }
        if let Some(r) = lay.free_boxes(&[], lay.w * 0.015, (lay.w * 0.12, lay.h * 0.12), 1).first() {
            parts.push(format!("free: x {}..{}, y {}..{}", ri(r[0]), ri(r[2]), ri(r[1]), ri(r[3])));
        }
        if let Some(b) = lay.balance_json() {
            parts.push(format!("balance: {}", b["lean"].as_str().unwrap_or("")));
        }
        Some(format!("layout {:.2}s (canvas px from centre, y down): {}. Ask canvas_layout for boxes", secs(t), parts.join("; ")))
    }
}

fn motion_json(m: &super::Motion, from: view::Time, to: view::Time) -> Value {
    // A path of at most six points.
    let step = (m.path.len() as f32 / 6.0).ceil().max(1.0) as usize;
    let mut path: Vec<Value> = m.path.iter().step_by(step).map(|p| json!([p.0, ri(p.1), ri(p.2)])).collect();
    if let Some(last) = m.path.last() {
        if path.last() != Some(&json!([last.0, ri(last.1), ri(last.2)])) {
            path.push(json!([last.0, ri(last.1), ri(last.2)]));
        }
    }
    let mut o = json!({"from": sec(from), "to": sec(to), "path": path, "travel": ri(m.travel), "speed_avg": ri(m.avg), "speed_top": ri(m.top)});
    if m.travel < 2.0 {
        o["still"] = json!(true);
    }
    if let Some(e) = m.enters {
        o["enters_frame_at"] = json!(e);
    }
    if let Some(l) = m.leaves {
        o["leaves_frame_at"] = json!(l);
    }
    o
}

#[cfg(not(feature = "render"))]
impl Session {
    pub(in crate::agent) fn layout_of(&mut self, _id: u32, _t: view::Time) -> Option<Value> {
        None
    }
    pub(in crate::agent) fn layout_line(&mut self, _t: view::Time) -> Option<String> {
        None
    }
}
