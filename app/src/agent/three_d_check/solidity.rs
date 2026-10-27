// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Is a model a real solid? Measured from its triangles:
//!
//! - flat: its thinnest side against its largest (a card, a billboard);
//! - an extruded cut-out: most of its surface is two parallel faces, a
//!   front and a back, with an outline that is not a plain rectangle (a
//!   picture pushed back into a slab, not a form);
//! - open: edges with only one face (a shell you can see into, a single
//!   sided plane, holes), not counting sheets that are thin by nature
//!   (eyelids, lashes, corneas, hair cards);
//!
//! and from the scene: things floating above what is under them, sunk into
//! it, or standing where they would tip over.

use super::Problem;
use studio_mesh::{Mesh, V3, add, cross, dot, length, scale, sub};

/// What the solid measures say about one model.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Solidity {
    /// Thinnest side over largest.
    pub thin: f32,
    /// Edges with one face, and all edges.
    pub open_edges: usize,
    pub edges: usize,
    /// When it is a slab: the share of its surface in the front and back
    /// faces, and how much of its box's face the outline fills.
    pub slab: Option<(f32, f32)>,
}

impl Solidity {
    pub fn of(mesh: &Mesh) -> Solidity {
        let (lo, hi) = mesh.bounds();
        let mut size = [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]];
        size.sort_by(|a, b| b.total_cmp(a));
        let thin = if size[0] > 1e-6 { size[2] / size[0] } else { 0.0 };
        // Open edges, with corners welded; sheets that are thin by nature
        // (eyelids, lashes, corneas, hair cards) are left out.
        let (ids, _) = mesh.position_ids();
        let mats = mesh.tri_materials();
        let sheet: Vec<bool> = mesh.materials.iter().map(|m| studio_mesh::seal::thin_by_nature(&m.name)).collect();
        let mut count: std::collections::HashMap<(u32, u32), u32> = std::collections::HashMap::new();
        for t in 0..mesh.triangles() {
            if sheet.get(mats[t] as usize).copied().unwrap_or(false) {
                continue;
            }
            let c = mesh.tri(t);
            let v = [ids[c[0]], ids[c[1]], ids[c[2]]];
            for (a, b) in [(v[0], v[1]), (v[1], v[2]), (v[2], v[0])] {
                if a != b {
                    *count.entry((a.min(b), a.max(b))).or_insert(0) += 1;
                }
            }
        }
        let open_edges = count.values().filter(|&&n| n == 1).count();
        Solidity { thin, open_edges, edges: count.len(), slab: slab(mesh) }
    }

    pub fn is_open(&self) -> bool {
        self.open_edges > 8 && self.open_edges as f32 > self.edges as f32 * 0.002
    }
}

/// A slab: two parallel faces hold most of the surface.
fn slab(mesh: &Mesh) -> Option<(f32, f32)> {
    let n = mesh.triangles();
    if n < 8 {
        return None;
    }
    let mut total = 0.0f32;
    let mut cov = [[0.0f32; 3]; 3];
    let mut faces: Vec<(V3, f32, V3)> = Vec::with_capacity(n);
    for t in 0..n {
        let c = mesh.tri(t);
        let (a, b, d) = (mesh.positions[c[0]], mesh.positions[c[1]], mesh.positions[c[2]]);
        let cr = cross(sub(b, a), sub(d, a));
        let area = length(cr) * 0.5;
        if area <= 0.0 {
            continue;
        }
        let nrm = scale(cr, 0.5 / area);
        total += area;
        for i in 0..3 {
            for j in 0..3 {
                cov[i][j] += area * nrm[i] * nrm[j];
            }
        }
        faces.push((nrm, area, scale(add(add(a, b), d), 1.0 / 3.0)));
    }
    if total <= 0.0 {
        return None;
    }
    // The direction most of the surface faces (either way).
    let mut axis = [1.0f32, 1.0, 1.0];
    for _ in 0..24 {
        let next = [dot(cov[0], axis), dot(cov[1], axis), dot(cov[2], axis)];
        let l = length(next);
        if l < 1e-12 {
            return None;
        }
        axis = scale(next, 1.0 / l);
    }
    let (mut front, mut back) = (Vec::new(), Vec::new());
    let mut flat_area = 0.0;
    for (nrm, area, c) in &faces {
        let d = dot(*nrm, axis);
        if d.abs() > 0.97 {
            flat_area += area;
            if d > 0.0 { front.push((dot(*c, axis), *area)) } else { back.push((dot(*c, axis), *area)) }
        }
    }
    let share = flat_area / total;
    if share < 0.55 || front.is_empty() || back.is_empty() {
        return None;
    }
    // Both sides lie in one plane each.
    let (lo, hi) = mesh.bounds();
    let extent = length(sub(hi, lo)).max(1e-6);
    let tight = |v: &[(f32, f32)]| {
        let w: f32 = v.iter().map(|x| x.1).sum();
        let mean = v.iter().map(|x| x.0 * x.1).sum::<f32>() / w;
        (v.iter().map(|x| (x.0 - mean).powi(2) * x.1).sum::<f32>() / w).sqrt() < extent * 0.02
    };
    if !tight(&front) || !tight(&back) {
        return None;
    }
    // The outline's fill of its box seen along the axis: the front's area
    // over the box across the axis.
    let (u, v) = {
        let a = if axis[0].abs() < 0.6 { [1.0, 0.0, 0.0] } else { [0.0, 1.0, 0.0] };
        let u = cross(axis, a);
        let u = scale(u, 1.0 / length(u));
        (u, cross(axis, u))
    };
    let (mut ul, mut uh, mut vl, mut vh) = (f32::INFINITY, f32::NEG_INFINITY, f32::INFINITY, f32::NEG_INFINITY);
    for p in &mesh.positions {
        ul = ul.min(dot(*p, u));
        uh = uh.max(dot(*p, u));
        vl = vl.min(dot(*p, v));
        vh = vh.max(dot(*p, v));
    }
    let front_area: f32 = front.iter().map(|x| x.1).sum();
    let fill = front_area / ((uh - ul) * (vh - vl)).max(1e-9);
    Some((share, fill.min(1.0)))
}

/// Names of things that are flat, or open, by nature.
const FLAT_THINGS: &[&str] = &["card", "paper", "poster", "sign", "screen", "flag", "banner", "plane", "sheet", "page", "leaf", "coin", "ticket", "label", "photo", "picture", "painting", "mat", "rug", "carpet", "tile", "panel", "board", "plank", "door", "book", "phone", "tablet", "letter", "envelope", "note", "sticker", "decal", "billboard", "backdrop", "cloth", "curtain", "cape", "towel", "napkin", "plate", "tray", "lid", "shelf", "window", "glass", "mirror", "frame", "wall", "floor", "ground", "terrain", "road", "water", "ocean", "sea", "pond", "grass", "shadow", "halo", "ring", "disc", "disk"];

fn says(name: &str, words: &[&str]) -> bool {
    let n = name.to_ascii_lowercase();
    n.split(|c: char| !c.is_ascii_alphanumeric()).any(|w| words.iter().any(|x| w == *x || (w.len() == x.len() + 1 && w.starts_with(x) && w.ends_with('s'))))
}

/// Notes on one hero's solidity.
pub fn notes(id: u32, name: &str, at: f64, s: &Solidity) -> Vec<Problem> {
    let mut out = Vec::new();
    let label = format!("model \"{name}\" (clip {id})");
    let flat_by_nature = says(name, FLAT_THINGS);
    if s.thin < 0.06 && !flat_by_nature {
        out.push(Problem {
            kind: "3d_flat",
            at,
            clip: id,
            problem: format!("{label} is flat: its thinnest side is {:.0}% of its largest, a card or billboard rather than a form; from any angle but the front it reads as a sliver", s.thin * 100.0),
            fix: "build it with depth: `model` with blended `parts` (ellipsoids, capsules, cones, boxes with real thickness on every axis, melted together with `blend`), or `mesh` `extrude`/`inflate` the flat model; a `body` collider will then have real volume too",
        });
    } else if let Some((share, fill)) = s.slab
        && fill < 0.82
        && !flat_by_nature
    {
        out.push(Problem {
            kind: "3d_extruded",
            at,
            clip: id,
            problem: format!("{label} is an extruded cut-out: {:.0}% of its surface is one flat front and one flat back, around an outline that fills {:.0}% of its box: a picture pushed back into a slab, not a modelled form (the side and top views give it away)", share * 100.0, fill * 100.0),
            fix: "model the form instead of carving one picture: `model` with blended `parts` for each mass (body, head, limbs, tail, beak or nose as cones, ears and crests as thin ellipsoids), using the picture only as the reference for proportions; or carve from front, side and top views with depth",
        });
    }
    if s.is_open() && !flat_by_nature {
        out.push(Problem {
            kind: "3d_open",
            at,
            clip: id,
            problem: format!("{label} is not a closed solid: {} edges have only one face (holes, a shell with no inside or a single-sided surface), so it shows its inside, lights wrong from behind and has no volume for physics", s.open_edges),
            fix: "`mesh` with step `solid` (closes holes, thickens open sheets, turns every face outward); keep `thickness` about 2% of its size for shells",
        });
    }
    out
}

/// Notes on what in the scene is not resting physically.
pub fn physical_notes(findings: &[crate::physics3d::rest::Finding], at: f64) -> Vec<Problem> {
    findings
        .iter()
        .map(|f| match f.kind {
            "floating" => Problem {
                kind: "3d_floating",
                at,
                clip: f.clip as u32,
                problem: format!("model \"{}\" (clip {}) floats {:.0} px above what is under it, with nothing holding it up", f.name, f.clip, f.amount),
                fix: "`settle` with its `clip` (it comes down onto what is under it and stays), or `drop` to see it fall; if it hangs or flies on purpose, `joint` it to what holds it or `body` with `type` none",
            },
            "sunk" => Problem {
                kind: "3d_sunk",
                at,
                clip: f.clip as u32,
                problem: format!("model \"{}\" (clip {}) is sunk {:.0} px into what it touches: two solids in one place", f.name, f.clip, f.amount),
                fix: "`settle` with its `clip` (pushed out and rested), or move it up by that much",
            },
            _ => Problem {
                kind: "3d_tips",
                at,
                clip: f.clip as u32,
                problem: format!("model \"{}\" (clip {}) stands where it cannot balance: dropped there it turns {:.0}° and falls over", f.name, f.clip, f.amount),
                fix: "`settle` with its `clip` (it ends up where it really rests), widen what it stands on, or `joint` it with kind fixed to what it leans on",
            },
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cut_out_slab_is_flagged_and_a_ball_is_not() {
        // A star outline pushed back into a slab.
        let star: Vec<[f32; 2]> = (0..10).map(|i| {
            let a = i as f32 * std::f32::consts::PI / 5.0;
            let r = if i % 2 == 0 { 100.0 } else { 45.0 };
            [r * a.cos(), r * a.sin()]
        }).collect();
        let mut m = Mesh::default();
        for (k, z) in [0.0f32, 30.0].iter().enumerate() {
            let c = m.positions.len() as u32;
            m.positions.push([0.0, 0.0, *z]);
            for p in &star {
                m.positions.push([p[0], p[1], *z]);
            }
            for i in 0..10u32 {
                let (a, b) = (c + 1 + i, c + 1 + (i + 1) % 10);
                if k == 0 { m.indices.extend([c, b, a]) } else { m.indices.extend([c, a, b]) }
            }
        }
        for i in 0..10u32 {
            let (a, b) = (1 + i, 1 + (i + 1) % 10);
            let (a2, b2) = (12 + i, 12 + (i + 1) % 10);
            m.indices.extend([a, b, b2, a, b2, a2]);
        }
        let s = Solidity::of(&m);
        let (share, fill) = s.slab.expect("a slab");
        assert!(share > 0.55 && fill < 0.82, "{share} {fill}");
        assert!(!notes(1, "Rooster", 0.0, &s).is_empty());
        let ball = Solidity::of(&studio_mesh::prim::ico_sphere(100.0, 3));
        assert!(ball.slab.is_none() && !ball.is_open() && ball.thin > 0.9);
        assert!(notes(2, "Ball", 0.0, &ball).is_empty());
        // A plane with one side is open and flat.
        let plane = Solidity::of(&studio_mesh::prim::by_name("plane", 100.0, 1).unwrap());
        assert!(notes(3, "Rooster", 0.0, &plane).iter().any(|p| p.kind == "3d_flat"));
        assert!(notes(3, "Poster", 0.0, &plane).is_empty(), "a poster is flat by nature");
    }
}
