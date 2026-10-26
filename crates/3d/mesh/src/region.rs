// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Part of a model, said in words: every face, the faces looking one way,
//! what lies inside a ball or a box, or one side of a plane.

use crate::{Mesh, V3, dot, length, normalise, sub};

#[derive(Clone, Debug, PartialEq, Default)]
pub enum Region {
    /// The whole model.
    #[default]
    All,
    /// Faces whose normal is within `degrees` of `direction`.
    Facing { direction: V3, degrees: f32 },
    /// Within `radius` of `centre`.
    Sphere { centre: V3, radius: f32 },
    /// Inside the box from `min` to `max`.
    Box { min: V3, max: V3 },
    /// Where coordinate `axis` (0 x, 1 y, 2 z) is above `at` (`above`) or
    /// below it.
    Side { axis: usize, at: f32, above: bool },
    /// Faces using material `index`.
    Material { index: u32 },
}

impl Region {
    fn holds(&self, p: V3) -> bool {
        match self {
            Region::All | Region::Facing { .. } | Region::Material { .. } => true,
            Region::Sphere { centre, radius } => length(sub(p, *centre)) <= *radius,
            Region::Box { min, max } => (0..3).all(|i| p[i] >= min[i].min(max[i]) && p[i] <= max[i].max(min[i])),
            Region::Side { axis, at, above } => {
                let v = p[(*axis).min(2)];
                if *above { v >= *at } else { v <= *at }
            }
        }
    }

    /// The triangles in the region.
    pub fn faces(&self, m: &Mesh) -> Vec<usize> {
        let mats = matches!(self, Region::Material { .. }).then(|| m.tri_materials());
        (0..m.triangles())
            .filter(|&t| match self {
                Region::All => true,
                Region::Facing { direction, degrees } => dot(m.face_normal(t), normalise(*direction)) >= degrees.clamp(0.0, 180.0).to_radians().cos() - 1e-5,
                Region::Material { index } => mats.as_ref().is_some_and(|k| k[t] == *index),
                _ => self.holds(m.centroid(t)),
            })
            .collect()
    }

    /// For each vertex, whether it is in the region.
    pub fn vertices(&self, m: &Mesh) -> Vec<bool> {
        match self {
            Region::All => vec![true; m.positions.len()],
            Region::Facing { .. } | Region::Material { .. } => {
                let mut sel = vec![false; m.positions.len()];
                for t in self.faces(m) {
                    for v in m.tri(t) {
                        sel[v] = true;
                    }
                }
                sel
            }
            _ => m.positions.iter().map(|p| self.holds(*p)).collect(),
        }
    }

    /// For each welded position (from [`Mesh::position_ids`]), whether it
    /// is in the region.
    pub fn positions(&self, m: &Mesh, ids: &[u32], count: usize) -> Vec<bool> {
        let mut sel = vec![false; count];
        for (v, on) in self.vertices(m).into_iter().enumerate() {
            if on {
                sel[ids[v] as usize] = true;
            }
        }
        sel
    }

    /// Reads a region from words: "all", "top"/"bottom"/"left"/"right"/
    /// "front"/"back" (faces looking that way), or the structured forms.
    pub fn named(name: &str) -> Option<Region> {
        let facing = |d: V3| Some(Region::Facing { direction: d, degrees: 45.0 });
        match name {
            "" | "all" | "everything" | "whole" => Some(Region::All),
            // y is down and z is away in Studio's world.
            "top" | "up" => facing([0.0, -1.0, 0.0]),
            "bottom" | "down" => facing([0.0, 1.0, 0.0]),
            "left" => facing([-1.0, 0.0, 0.0]),
            "right" => facing([1.0, 0.0, 0.0]),
            "front" | "toward_camera" => facing([0.0, 0.0, -1.0]),
            "back" | "away" => facing([0.0, 0.0, 1.0]),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prim;

    #[test]
    fn regions_pick_the_right_faces() {
        let m = prim::cube([100.0; 3]);
        assert_eq!(Region::All.faces(&m).len(), 12);
        assert_eq!(Region::named("top").unwrap().faces(&m).len(), 2);
        assert_eq!(Region::named("front").unwrap().faces(&m).len(), 2);
        let s = Region::Side { axis: 0, at: 0.0, above: true };
        assert!(s.vertices(&m).iter().filter(|b| **b).count() > 0);
        assert_eq!(Region::Sphere { centre: [500.0; 3], radius: 1.0 }.faces(&m).len(), 0);
    }
}
