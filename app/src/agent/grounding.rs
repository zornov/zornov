// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Standing on the ground: where a 3D model's lowest point is against the
//! surface under it. A figure placed by numbers can stand inside the road,
//! or a hand's breadth above it, and the numbers look fine; the picture
//! shows legs cut off at the shins, or nothing at all. The edit's read-back
//! and `check` say so, with the height that would stand it there.
//!
//! The surface is the nearest one under the model's top among the other
//! models on screen that are not rigged and are wider than it (the street,
//! the floor, a table under a cup), found by casting down through each
//! one's triangles.

use super::Session;
use super::motion_check::Problem;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use studio_mesh::bvh::Bvh;
use studio_model as m;

/// How far into the surface (as a share of the model's height) counts as
/// buried rather than standing.
const OFF: f64 = 0.03;
/// How far over it counts as floating: more, since the box is the model at
/// rest and a walk lifts the body a little with each step.
const OVER: f64 = 0.06;

/// A model file as footing needs it: its box, whether it is rigged, and
/// (for what others stand on) its triangles to cast against.
struct Shape {
    lo: [f32; 3],
    hi: [f32; 3],
    rigged: bool,
    bvh: Option<Bvh>,
    /// (first triangle, count, material name) runs, to name what was hit.
    groups: Vec<(u32, u32, String)>,
}

impl Shape {
    /// The material name of triangle `tri`.
    fn material(&self, tri: u32) -> String {
        self.groups.iter().find(|(s, n, _)| tri >= *s && tri < s + n).map(|g| g.2.clone()).unwrap_or_default()
    }
}

/// The shape of a model file, read once.
fn shape(file: &str) -> Option<Arc<Shape>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Option<Arc<Shape>>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(s) = cache.lock().ok()?.get(file) {
        return s.clone();
    }
    let s = studio_mesh::io::load(std::path::Path::new(file)).ok().map(|mesh| {
        let (lo, hi) = mesh.bounds();
        let rigged = mesh.rig.as_ref().is_some_and(|r| !r.bones.is_empty());
        let groups = mesh.groups.iter().map(|gr| (gr.start, gr.count, mesh.materials.get(gr.material as usize).map(|m| m.name.clone()).unwrap_or_default())).collect();
        Arc::new(Shape { lo, hi, rigged, bvh: (!rigged).then(|| Bvh::of_mesh(&mesh)), groups })
    });
    let mut c = cache.lock().ok()?;
    if c.len() > 64 {
        c.clear();
    }
    c.insert(file.to_owned(), s.clone());
    s
}

/// Where a model stands at one moment.
pub(super) struct Footing {
    /// Px its lowest point is under the surface (negative: above it).
    pub gap: f64,
    /// The model it stands on (or in, or over).
    pub on: u32,
    /// Its height on screen, px.
    pub height: f64,
    /// Its `y` now, and the `y` that would stand it on the surface.
    pub y: f64,
    pub stand_y: f64,
    /// The material it stands on ("road", "floor boards"…).
    pub material: String,
}

impl Footing {
    /// Buried in the surface, standing, or floating over it, in words.
    pub fn said(&self, name: &str) -> Option<String> {
        let off = self.gap.abs();
        let allowed = if self.gap > 0.0 { OFF } else { OVER };
        if off <= allowed * self.height {
            return None;
        }
        let how = if self.gap > 0.0 { "under the top of" } else { "above" };
        let what = if self.gap > 0.0 { "buried in it" } else { "floating over it" };
        Some(format!("its lowest point is {off:.0} px {how} \"{name}\" (clip {}): {what}; y {:.0} stands it there", self.on, self.stand_y))
    }
}

impl Session {
    /// Where model clip `id` stands at project time `t`, if a surface is
    /// under it.
    pub(super) fn footing(&self, id: u32, t: m::Micros) -> Option<Footing> {
        let seq = self.doc.model.active();
        let mc = seq.clip(m::ClipId(id as u64))?;
        if t < mc.start || t >= mc.start + mc.duration {
            return None;
        }
        let me = shape(&crate::mesh_store::file_of(mc)?)?;
        let tr = mc.transform_at(t - mc.start);
        // Tipped over or lying down: it has no feet to stand on.
        if tr.rotation_x.abs() > 1.0 || tr.rotation.abs() > 1.0 {
            return None;
        }
        let k = tr.scale_x.abs().max(1e-3) as f32;
        let (x, y, z) = (tr.position.x as f32, tr.position.y as f32, tr.position_z as f32);
        let (top, low) = (y + k * me.lo[1], y + k * me.hi[1]);
        let height = low - top;
        if height <= 0.0 {
            return None;
        }
        let area = (me.hi[0] - me.lo[0]) * (me.hi[2] - me.lo[2]) * k * k;
        // Cast from a little over its top, so a ceiling above a head is not its floor.
        let from = top - 0.25 * height;
        let mut best: Option<(f32, u32, String)> = None;
        for oc in seq.clips.iter().filter(|c| c.id != mc.id && c.enabled && t >= c.start && t < c.start + c.duration) {
            let Some(s) = crate::mesh_store::file_of(oc).and_then(|f| shape(&f)) else { continue };
            let Some(bvh) = s.bvh.as_ref() else { continue };
            let o = oc.transform_at(t - oc.start);
            let ok = o.scale_x.abs().max(1e-3) as f32;
            // What stands on a thing is smaller across than it: a box on the
            // ground, not the ground on the box.
            if (s.hi[0] - s.lo[0]) * (s.hi[2] - s.lo[2]) * ok * ok <= area {
                continue;
            }
            let (sn, cs) = (o.rotation_y as f32).to_radians().sin_cos();
            let (dx, dz) = (x - o.position.x as f32, z - o.position_z as f32);
            let (lx, lz) = ((dx * cs + dz * sn) / ok, (-dx * sn + dz * cs) / ok);
            let oy = (from - o.position.y as f32) / ok;
            if let Some(hit) = bvh.ray([lx, oy, lz], [0.0, 1.0, 0.0], 0.0, 1e7) {
                let wy = o.position.y as f32 + ok * (oy + hit.t);
                if best.as_ref().is_none_or(|b| wy < b.0) {
                    best = Some((wy, oc.id.0 as u32, s.material(hit.tri)));
                }
            }
        }
        let (surface, on, material) = best?;
        let gap = (low - surface) as f64;
        Some(Footing { gap, on, height: height as f64, y: y as f64, stand_y: y as f64 - gap, material })
    }

    /// The footing of clip `id` at its start, said when it is off the
    /// surface (for the edit's read-back).
    pub(super) fn footing_said(&self, id: u32) -> Option<String> {
        let c = self.doc.project.clip(id)?;
        let f = self.footing(id, c.start)?;
        let name = self.doc.project.clip(f.on).map(|o| o.name.clone()).unwrap_or_default();
        f.said(&name)
    }

    /// Models buried in the surface they stand on, or floating over it,
    /// for most of their time on screen (a jump or a fall is a moment, not
    /// most of the shot).
    pub(super) fn footing_problems(&self) -> Vec<Problem> {
        let seq = self.doc.model.active();
        let mut out = Vec::new();
        for mc in seq.clips.iter().filter(|c| c.enabled && crate::mesh_store::file_of(c).is_some()) {
            let id = mc.id.0 as u32;
            let feet: Vec<(m::Micros, Footing)> = (0..5).map(|i| mc.start + mc.duration * (2 * i + 1) / 10).filter_map(|t| self.footing(id, t).map(|f| (t, f))).collect();
            if feet.len() < 3 {
                continue;
            }
            let buried: Vec<&(m::Micros, Footing)> = feet.iter().filter(|(_, f)| f.gap > OFF * f.height).collect();
            let floating: Vec<&(m::Micros, Footing)> = feet.iter().filter(|(_, f)| -f.gap > OVER * f.height).collect();
            let name = |i: u32| self.doc.project.clip(i).map(|c| c.name.clone()).unwrap_or_default();
            let (kind, list, fix) = if buried.len() * 2 >= feet.len() {
                ("3d_buried", buried, "raise it so its lowest point meets the surface (the y given), or walk it with act `ground: \"scene\"` so its feet follow what it stands on")
            } else if floating.len() * 5 >= feet.len() * 4 {
                ("3d_floating", floating, "lower it onto the surface (the y given), or say in the notes why it hovers (3d-ok: floating)")
            } else {
                continue;
            };
            let (t, f) = list[0];
            out.push(Problem { kind, at: (*t as f64 / 1e6 * 10.0).round() / 10.0, clip: id, problem: format!("model \"{}\" (clip {id}): {}", name(id), f.said(&name(f.on)).unwrap_or_default()), fix });
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_figure_in_the_road_or_over_it_is_said_with_the_height_that_stands_it() {
        let mut s = Session::default();
        // A slab whose top is at y 0 (it spans 0 to 40, y down), and a box 200 high on it.
        s.edit(&[json!({"op": "model", "shape": "cube", "size": [2000, 40, 2000], "position": [0, 20, 0], "name": "Ground", "duration": 4})], "t", false).unwrap();
        s.edit(&[json!({"op": "model", "shape": "cube", "size": [100, 200, 100], "position": [0, -100, 0], "name": "Box", "duration": 4})], "t", false).unwrap();
        let id = |s: &Session, n: &str| s.doc.project.clips.iter().find(|c| c.name == n).unwrap().id;
        let (ground, bx) = (id(&s, "Ground"), id(&s, "Box"));
        let f = s.footing(bx, 0).expect("the box stands on the ground");
        assert_eq!(f.on, ground);
        assert!(f.gap.abs() < 1.0, "standing: {}", f.gap);
        assert!(s.footing_said(bx).is_none() && s.footing_problems().is_empty());
        // Sunk 150 px into it: said in the read-back, with the y that stands it.
        let r = s.edit(&[json!({"op": "set", "clip": bx, "y": 50})], "t", false).unwrap();
        let line = r["read_back"]["clips"].as_array().unwrap().iter().find(|c| c["clip"] == bx).unwrap().clone();
        let said = line["ground"].as_str().unwrap_or_default().to_owned();
        assert!(said.contains("150 px under") && said.contains("buried") && said.contains("y -100"), "{line}");
        assert!(s.footing_problems().iter().any(|p| p.kind == "3d_buried" && p.clip == bx));
        // Lifted 300 px over it: floating.
        s.edit(&[json!({"op": "set", "clip": bx, "y": -400})], "t", false).unwrap();
        assert!(s.footing_said(bx).is_some_and(|t| t.contains("300 px above") && t.contains("floating")));
        assert!(s.footing_problems().iter().any(|p| p.kind == "3d_floating"));
        // The ground itself has nothing under it: nothing to say.
        assert!(s.footing(ground, 0).is_none());
    }
}
