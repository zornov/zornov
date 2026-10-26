// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Multiresolution sculpting: a clean base mesh with levels of
//! subdivision stacked on it. Big forms are shaped at a low level and fine
//! detail at a high one; a change at a low level carries the detail above
//! it along (the change is subdivided up, so wrinkles ride on a moved
//! jaw). The base stays a light, animatable cage; the top level is the
//! detail that gets baked into normal and displacement maps.

use crate::poly::Poly;
use crate::sculpt::{Brush, Report, SculptMesh, Stroke};
use studio_mesh::{V3, add, sub};

#[derive(Clone, Debug)]
pub struct Multires {
    /// levels[0] is the base; each next level is the one before
    /// subdivided once, with its own sculpted positions.
    pub levels: Vec<Poly>,
}

impl Multires {
    pub fn new(base: Poly) -> Multires {
        Multires { levels: vec![base] }
    }

    pub fn top(&self) -> usize {
        self.levels.len() - 1
    }

    /// Adds a level of subdivision on top (smooth, creases respected).
    pub fn subdivide(&mut self) {
        let next = crate::subd::once(&self.levels[self.top()]);
        self.levels.push(next);
    }

    /// Sets level `l`'s positions; the change rides up through every
    /// level above.
    pub fn set_positions(&mut self, l: usize, positions: Vec<V3>) {
        let l = l.min(self.top());
        let delta: Vec<V3> = positions.iter().zip(&self.levels[l].positions).map(|(a, b)| sub(*a, *b)).collect();
        self.levels[l].positions = positions;
        let mut d = delta;
        for k in l..self.top() {
            // Catmull–Clark is linear in the points: subdividing the
            // change gives the smooth change one level up.
            let mut carrier = self.levels[k].clone();
            carrier.positions = d;
            let up = crate::subd::once(&carrier);
            let next = &mut self.levels[k + 1];
            for (p, dd) in next.positions.iter_mut().zip(&up.positions) {
                *p = add(*p, *dd);
            }
            d = up.positions;
        }
    }

    /// Sculpts at level `l`.
    pub fn stroke(&mut self, l: usize, brush: &Brush, stroke: &Stroke) -> Report {
        let l = l.min(self.top());
        let mut s = SculptMesh::from_poly(&self.levels[l]);
        let rep = s.stroke(brush, stroke);
        self.set_positions(l, s.pos[..self.levels[l].positions.len()].to_vec());
        rep
    }

    /// Throws away the detail above level `l`.
    pub fn delete_above(&mut self, l: usize) {
        self.levels.truncate(l + 1);
    }

    /// Rebuilds a lower level from the top (after the top was replaced,
    /// say by a voxel sculpt projected back): each level becomes the
    /// level above averaged down to its vertices.
    pub fn rebuild_lower(&mut self) {
        for k in (0..self.top()).rev() {
            let nv = self.levels[k].positions.len();
            // Vertex points of a subdivision keep their numbers; the level
            // above's points at those numbers are the limit of the sculpt
            // there. A few rounds of "fit" pull level k so its subdivision
            // lands on them.
            for _ in 0..4 {
                let up = crate::subd::once(&self.levels[k]);
                let target = self.levels[k + 1].positions.clone();
                for v in 0..nv {
                    let err = sub(target[v], up.positions[v]);
                    self.levels[k].positions[v] = add(self.levels[k].positions[v], err);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit::Falloff;
    use crate::prim;
    use crate::sculpt::Kind;
    use studio_mesh::length;

    #[test]
    fn low_level_moves_carry_high_level_detail() {
        let mut m = Multires::new(prim::quad_sphere(100.0, 4));
        m.subdivide();
        m.subdivide();
        assert_eq!(m.levels[2].faces(), 6 * 16 * 16);
        let start = m.levels[2].positions.iter().map(|p| -p[1]).fold(f32::MIN, f32::max);
        // Fine detail on top.
        let brush = Brush { kind: Kind::Draw, radius: 15.0, strength: 1.0, falloff: Falloff::Smooth, invert: false, alpha: None, front_only: true, autosmooth: 0.0, face_set: 0 };
        let st = Stroke { points: vec![[0.0, -100.0, 0.0]], spacing: 0.2, symmetry: [false; 3], dyntopo: None };
        for _ in 0..5 {
            m.stroke(2, &brush, &st);
        }
        let bump = |m: &Multires| m.levels[2].positions.iter().map(|p| -p[1]).fold(f32::MIN, f32::max);
        let b0 = bump(&m);
        assert!(b0 > start + 3.0, "{start} -> {b0}");
        // Move the whole top half up at the base level.
        let moved: Vec<V3> = m.levels[0].positions.iter().map(|p| if p[1] < 0.0 { add(*p, [0.0, -30.0, 0.0]) } else { *p }).collect();
        m.set_positions(0, moved);
        let b1 = bump(&m);
        assert!((b1 - b0 - 30.0).abs() < 3.0, "the detail rode up: {b0} -> {b1}");
        let base_r = length(m.levels[0].positions[0]);
        assert!(base_r > 50.0);
    }
}
