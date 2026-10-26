// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `polygon_mask` and `parts_check`: drawing an outline by hand when the
//! automatic one is not right, and checking a split into parts.
//!
//! * `polygon_mask`: a mask on a clip from a polygon whose points are
//!   **pixels of the clip's own picture** (the source file, from its top
//!   left corner), not the canvas. It follows the clip's crop, scale and
//!   moves like any mask. `smooth` rounds the corners, `feather` softens
//!   the edge (picture pixels), `invert` keeps the outside; the reply gives
//!   the box and the share of the picture it covers, and refuses a polygon
//!   that crosses itself.
//! * `parts_check`: the picture a rig was split from, its parts stacked
//!   and where the stack differs from the picture, side by side (picture,
//!   parts, error), with the counts.

use super::{Content, Session, f};
use serde_json::{Value, json};
use studio_model as m;

/// Two segments cross (not merely share an end).
fn crosses(a: ([f64; 2], [f64; 2]), b: ([f64; 2], [f64; 2])) -> bool {
    let o = |p: [f64; 2], q: [f64; 2], r: [f64; 2]| (q[0] - p[0]) * (r[1] - p[1]) - (q[1] - p[1]) * (r[0] - p[0]);
    let (d1, d2, d3, d4) = (o(b.0, b.1, a.0), o(b.0, b.1, a.1), o(a.0, a.1, b.0), o(a.0, a.1, b.1));
    ((d1 > 0.0) != (d2 > 0.0)) && ((d3 > 0.0) != (d4 > 0.0)) && d1 != 0.0 && d2 != 0.0 && d3 != 0.0 && d4 != 0.0
}

/// The first pair of edges of the closed polygon that cross.
pub fn self_crossing(pts: &[[f64; 2]]) -> Option<(usize, usize)> {
    let n = pts.len();
    for i in 0..n {
        let a = (pts[i], pts[(i + 1) % n]);
        for j in i + 2..n {
            if i == 0 && j == n - 1 {
                continue;
            }
            if crosses(a, (pts[j], pts[(j + 1) % n])) {
                return Some((i, j));
            }
        }
    }
    None
}

/// Polygon area (shoelace), unsigned.
pub fn area(pts: &[[f64; 2]]) -> f64 {
    let n = pts.len();
    (0..n).map(|i| pts[i][0] * pts[(i + 1) % n][1] - pts[(i + 1) % n][0] * pts[i][1]).sum::<f64>().abs() / 2.0
}

fn point(v: &Value) -> Option<[f64; 2]> {
    match v {
        Value::Array(a) if a.len() == 2 => Some([a[0].as_f64()?, a[1].as_f64()?]),
        Value::Object(o) => Some([o.get("x")?.as_f64()?, o.get("y")?.as_f64()?]),
        _ => None,
    }
}

impl Session {
    pub(super) fn polygon_mask(&mut self, op: &Value) -> Result<String, String> {
        let id = op.get("clip").and_then(Value::as_u64).ok_or("polygon_mask needs \"clip\"")? as u32;
        self.clip(id)?;
        let seq = self.doc.seq();
        let mut mc = self.model_clip(id).ok_or_else(|| format!("no clip {id}"))?;
        if let Some(rm) = op.get("remove") {
            let mid = rm.as_u64().ok_or("remove is the mask's id (from the reply that made it, or inspect)")?;
            let before = mc.masks.len();
            mc.masks.retain(|k| k.id.0 != mid);
            if mc.masks.len() == before {
                return Err(format!("clip {id} has no mask {mid}"));
            }
            let ok = self.doc.apply("Remove mask", m::Edit::SetClip { seq, clip: Box::new(mc) });
            self.check(ok, "")?;
            return Ok(format!("mask {mid} removed from clip {id}"));
        }
        let raw = op.get("points").and_then(Value::as_array).ok_or("polygon_mask needs \"points\": [[x, y], ...] in pixels of the clip's own picture (from its top left corner)")?;
        let mut pts: Vec<[f64; 2]> = Vec::new();
        for p in raw {
            pts.push(point(p).ok_or("each point is [x, y] in picture pixels")?);
        }
        pts.dedup();
        if pts.len() > 1 && pts.first() == pts.last() {
            pts.pop();
        }
        if pts.len() < 3 {
            return Err("a polygon needs three or more points".into());
        }
        // The picture the points are measured in.
        let m::ClipSource::Media(r) = &mc.source else { return Err(format!("clip {id} is not a picture or footage: its mask points have no source pixels to be measured in (use mask for a title or shape)")) };
        let mi = self.doc.project.media(r.media.0 as u32).ok_or("the clip's media is gone")?;
        let (sw, sh) = (mi.width.max(1) as f64, mi.height.max(1) as f64);
        let mut warnings = Vec::new();
        let outside = pts.iter().filter(|p| p[0] < 0.0 || p[1] < 0.0 || p[0] > sw || p[1] > sh).count();
        if outside > 0 {
            warnings.push(format!("{outside} point(s) lie outside the picture ({sw:.0}x{sh:.0}); they were kept on its edge"));
            for p in &mut pts {
                p[0] = p[0].clamp(0.0, sw);
                p[1] = p[1].clamp(0.0, sh);
            }
        }
        if let Some((i, j)) = self_crossing(&pts) {
            return Err(format!("the outline crosses itself between points {} to {} and {} to {}: a mask with a twist fills unevenly; reorder or remove a point", i + 1, (i + 1) % pts.len() + 1, j + 1, (j + 1) % pts.len() + 1));
        }
        let cover = area(&pts) / (sw * sh);
        let crop = mc.transform.crop.clone();
        let to_layer = |p: [f64; 2]| m::Vec2::new(((p[0] / sw) - crop.left) / (1.0 - crop.left - crop.right).max(1e-6), ((p[1] / sh) - crop.top) / (1.0 - crop.top - crop.bottom).max(1e-6));
        let layer: Vec<m::Vec2> = pts.iter().map(|p| to_layer(*p)).collect();
        // Corners rounded: Catmull-Rom tangents as bezier handles.
        let smooth = op.get("smooth").and_then(Value::as_bool).unwrap_or(false);
        let n = layer.len();
        let path: Vec<m::PathPoint> = (0..n)
            .map(|i| {
                let (prev, next) = (layer[(i + n - 1) % n], layer[(i + 1) % n]);
                let t = if smooth { m::Vec2::new((next.x - prev.x) / 6.0, (next.y - prev.y) / 6.0) } else { m::Vec2::ZERO };
                m::PathPoint { point: layer[i], handle_in: m::Vec2::new(-t.x, -t.y), handle_out: t }
            })
            .collect();
        let mid = mc.masks.iter().map(|k| k.id.0).max().map_or(1, |x| x + 1);
        let mut mask = m::Mask::new(m::MaskId(mid), m::MaskShape::Path);
        mask.path = path;
        mask.invert = op.get("invert").and_then(Value::as_bool).unwrap_or(false);
        // Feather in picture pixels, as roto does (a share of the short side).
        let feather = f(op, "feather").unwrap_or(1.0).max(0.0) * 2.0 / sw.min(sh);
        mask.params.insert("feather".into(), m::Value::Scalar(feather));
        mask.extra.insert("polygon".into(), json!(true));
        if let Some(name) = op.get("name").and_then(Value::as_str) {
            mask.extra.insert("name".into(), json!(name));
        }
        mc.masks.push(mask);
        let ok = self.doc.apply("Polygon mask", m::Edit::SetClip { seq, clip: Box::new(mc) });
        self.check(ok, "")?;
        let (x0, y0) = (pts.iter().map(|p| p[0]).fold(f64::MAX, f64::min), pts.iter().map(|p| p[1]).fold(f64::MAX, f64::min));
        let (x1, y1) = (pts.iter().map(|p| p[0]).fold(f64::MIN, f64::max), pts.iter().map(|p| p[1]).fold(f64::MIN, f64::max));
        Ok(format!(
            "clip {id}: mask {mid}, {n} points{} in picture pixels, box [{x0:.0}, {y0:.0}] to [{x1:.0}, {y1:.0}] of {sw:.0}x{sh:.0}, covering {:.1}% of the picture{}{}. Look at it with frame; remove it with polygon_mask {{\"clip\": {id}, \"remove\": {mid}}}",
            if smooth { " (corners rounded)" } else { "" },
            cover * 100.0,
            if op.get("invert").and_then(Value::as_bool).unwrap_or(false) { ", keeping the outside" } else { "" },
            if warnings.is_empty() { String::new() } else { format!(". Note: {}", warnings.join("; ")) },
        ))
    }

    /// `parts_check`: the split of a rig, checked side by side.
    pub(super) fn parts_check(&mut self, a: &Value) -> Result<Vec<Content>, String> {
        let id = a.get("clip").and_then(Value::as_u64).ok_or("parts_check needs \"clip\": the rig that was split (parts)")? as u32;
        let crop = match a.get("crop") {
            None | Some(Value::Null) => None,
            Some(Value::Array(c)) if c.len() == 4 && c.iter().all(Value::is_number) => {
                let n: Vec<usize> = c.iter().filter_map(Value::as_f64).map(|v| v.max(0.0) as usize).collect();
                Some([n[0], n[1], n[2], n[3]])
            }
            Some(_) => return Err("crop is [x0, y0, x1, y1] in pixels of the picture the rig was split from".into()),
        };
        let panel = a.get("width").and_then(Value::as_u64).map_or(420, |w| w.clamp(120, 900) as usize);
        let d = crate::services::character::parts_check(&self.doc, id, crop, panel)?;
        let jpeg = studio_content::jpeg::encode(&studio_media::RgbaImage { width: d.image.width as u32, height: d.image.height as u32, pixels: d.image.rgba.clone() }, 85);
        let verdict = if d.holes == 0 && d.extras == 0 && d.mismatched < 8 { "clean: the parts stack back into the picture" } else { "not clean: see the red (hole), blue (extra) and yellow (colour differs) pixels in the right panel" };
        let report = json!({
            "verdict": verdict,
            "panels": "left to right: the picture | the parts (each tinted, dark lines at the seams, white hatching where more than one layer covers: fill painted in behind a part) | the error (red: the picture has a pixel the stack lacks; blue: the stack has one the picture lacks; yellow: colour differs)",
            "crop": d.crop,
            "holes": d.holes,
            "extras": d.extras,
            "mismatched": d.mismatched,
            "stacked": d.stacked,
            "parts": d.per_part.iter().map(|(n, p)| json!({"part": n, "pixels": p})).collect::<Vec<_>>(),
            "fix": "holes or a part in the wrong place: move that bone (pose / skeleton) and split again, or outline the part by hand with polygon_mask on its layer; an extra: a part picture was edited; crop zooms in (source pixels)",
        });
        Ok(vec![Content::Text(report.to_string()), Content::Jpeg(jpeg)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_twisted_outline_is_found() {
        let square = [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
        assert_eq!(self_crossing(&square), None);
        let bow = [[0.0, 0.0], [10.0, 10.0], [10.0, 0.0], [0.0, 10.0]];
        assert!(self_crossing(&bow).is_some());
        // Touching at a shared corner is not a crossing.
        let dart = [[0.0, 0.0], [10.0, 0.0], [5.0, 5.0], [10.0, 10.0], [0.0, 10.0]];
        assert_eq!(self_crossing(&dart), None);
        assert!((area(&square) - 100.0).abs() < 1e-9);
    }
}
