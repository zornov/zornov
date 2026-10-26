// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Repairs that keep a model's own faces (when it should not be rebuilt):
//! degenerate and doubled faces dropped, loose specks removed, every
//! face of a piece wound the same way and outwards, and holes closed
//! (each boundary loop filled by ear clipping in its own best plane).

use crate::geo::{triangle_area, weld};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use studio_mesh::{Mesh, V3, add, cross, dot, length, normalise, scale, sub};

#[derive(Clone, Debug, Default, Serialize)]
pub struct Repaired {
    pub degenerate: usize,
    pub duplicates: usize,
    pub specks: usize,
    pub flipped: usize,
    pub holes: usize,
}

impl Repaired {
    pub fn any(&self) -> bool {
        self.degenerate + self.duplicates + self.specks + self.flipped + self.holes > 0
    }
    pub fn describe(&self) -> String {
        let mut v = Vec::new();
        for (n, what) in [(self.degenerate, "degenerate faces dropped"), (self.duplicates, "doubled faces dropped"), (self.specks, "loose specks removed"), (self.flipped, "faces turned the right way"), (self.holes, "holes closed")] {
            if n > 0 {
                v.push(format!("{n} {what}"));
            }
        }
        if v.is_empty() { "nothing to repair".into() } else { v.join(", ") }
    }
}

fn keep_faces(m: &mut Mesh, keep: &[bool]) {
    let mats = m.tri_materials();
    let mut idx = Vec::new();
    let mut km = Vec::new();
    for t in 0..m.triangles() {
        if keep[t] {
            idx.extend_from_slice(&m.indices[3 * t..3 * t + 3]);
            km.push(mats[t]);
        }
    }
    m.indices = idx;
    m.regroup(&km);
    studio_mesh::ops::compact(m);
}

/// Repairs `m` in place.
pub fn repair(m: &mut Mesh) -> Repaired {
    let mut r = Repaired::default();
    if m.triangles() == 0 {
        return r;
    }
    let diag = crate::geo::diagonal(m);
    // Degenerate and doubled faces.
    let w = weld(m);
    let mut seen: HashSet<[u32; 3]> = HashSet::new();
    let mut keep = vec![true; m.triangles()];
    for t in 0..m.triangles() {
        let c = w.tris[t];
        if c[0] == c[1] || c[1] == c[2] || c[0] == c[2] || triangle_area(m, t) <= diag * diag * 1e-12 {
            keep[t] = false;
            r.degenerate += 1;
            continue;
        }
        let mut k = c;
        k.sort();
        if !seen.insert(k) {
            keep[t] = false;
            r.duplicates += 1;
        }
    }
    if r.degenerate + r.duplicates > 0 {
        keep_faces(m, &keep);
    }
    // Specks: pieces under 0.5% of the surface, when a bigger piece exists.
    let w = weld(m);
    let n = m.triangles();
    let mut by_edge: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
    for (t, c) in w.tris.iter().enumerate() {
        for k in 0..3 {
            let (a, b) = (c[k], c[(k + 1) % 3]);
            by_edge.entry((a.min(b), a.max(b))).or_default().push(t);
        }
    }
    let mut comp = vec![usize::MAX; n];
    let mut comps: Vec<Vec<usize>> = Vec::new();
    for s in 0..n {
        if comp[s] != usize::MAX {
            continue;
        }
        let id = comps.len();
        let mut list = vec![];
        let mut st = vec![s];
        comp[s] = id;
        while let Some(t) = st.pop() {
            list.push(t);
            let c = w.tris[t];
            for k in 0..3 {
                let (a, b) = (c[k], c[(k + 1) % 3]);
                for &o in &by_edge[&(a.min(b), a.max(b))] {
                    if comp[o] == usize::MAX {
                        comp[o] = id;
                        st.push(o);
                    }
                }
            }
        }
        comps.push(list);
    }
    let areas: Vec<f32> = comps.iter().map(|c| c.iter().map(|&t| triangle_area(m, t)).sum()).collect();
    let total: f32 = areas.iter().sum();
    if comps.len() > 1 {
        let mut keep = vec![true; n];
        for (c, list) in comps.iter().enumerate() {
            if areas[c] < total * 0.005 {
                r.specks += 1;
                for &t in list {
                    keep[t] = false;
                }
            }
        }
        if r.specks > 0 {
            keep_faces(m, &keep);
        }
    }
    // Consistent winding per piece (flood from a seed), then outwards
    // (positive enclosed volume).
    let w = weld(m);
    let n = m.triangles();
    let mut by_edge: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
    for (t, c) in w.tris.iter().enumerate() {
        for k in 0..3 {
            let (a, b) = (c[k], c[(k + 1) % 3]);
            by_edge.entry((a.min(b), a.max(b))).or_default().push(t);
        }
    }
    let runs = |c: [u32; 3], a: u32, b: u32| (0..3).any(|k| c[k] == a && c[(k + 1) % 3] == b);
    let mut flip = vec![false; n];
    let mut done = vec![false; n];
    for s in 0..n {
        if done[s] {
            continue;
        }
        let mut piece = vec![];
        let mut st = vec![s];
        done[s] = true;
        while let Some(t) = st.pop() {
            piece.push(t);
            let mut c = w.tris[t];
            if flip[t] {
                c.swap(1, 2);
            }
            for k in 0..3 {
                let (a, b) = (c[k], c[(k + 1) % 3]);
                let fs = &by_edge[&(a.min(b), a.max(b))];
                if fs.len() != 2 {
                    continue;
                }
                let o = if fs[0] == t { fs[1] } else { fs[0] };
                if done[o] {
                    continue;
                }
                // A neighbour wound the same way runs the shared edge b → a.
                flip[o] = runs(w.tris[o], a, b);
                done[o] = true;
                st.push(o);
            }
        }
        // Volume of the piece as wound.
        let mut vol = 0.0f64;
        for &t in &piece {
            let mut c = w.tris[t];
            if flip[t] {
                c.swap(1, 2);
            }
            let [a, b, cc] = c.map(|i| w.points[i as usize]);
            vol += dot(a, cross(b, cc)) as f64;
        }
        if vol < 0.0 {
            for &t in &piece {
                flip[t] = !flip[t];
            }
        }
    }
    for t in 0..n {
        if flip[t] {
            m.indices.swap(3 * t + 1, 3 * t + 2);
            r.flipped += 1;
        }
    }
    // Holes: boundary loops, filled.
    let w = weld(m);
    let mut next: HashMap<u32, u32> = HashMap::new();
    let mut count: HashMap<(u32, u32), usize> = HashMap::new();
    for c in &w.tris {
        for k in 0..3 {
            let (a, b) = (c[k], c[(k + 1) % 3]);
            *count.entry((a.min(b), a.max(b))).or_default() += 1;
        }
    }
    for c in &w.tris {
        for k in 0..3 {
            let (a, b) = (c[k], c[(k + 1) % 3]);
            if count[&(a.min(b), a.max(b))] == 1 {
                // The hole runs the other way round.
                next.insert(b, a);
            }
        }
    }
    let mut used: HashSet<u32> = HashSet::new();
    let mut fills: Vec<[u32; 3]> = Vec::new();
    let starts: Vec<u32> = next.keys().copied().collect();
    for s in starts {
        if used.contains(&s) {
            continue;
        }
        let mut lp = vec![s];
        used.insert(s);
        let mut cur = s;
        let mut ok = false;
        while let Some(&nx) = next.get(&cur) {
            if nx == s {
                ok = true;
                break;
            }
            if !used.insert(nx) || lp.len() > 20_000 {
                break;
            }
            lp.push(nx);
            cur = nx;
        }
        if ok && lp.len() >= 3 {
            fills.extend(ear_clip(&lp, &w.points));
            r.holes += 1;
        }
    }
    if !fills.is_empty() {
        // New faces reuse a vertex at each welded position.
        let mut any: HashMap<u32, u32> = HashMap::new();
        for (i, &pid) in w.ids.iter().enumerate() {
            any.entry(pid).or_insert(i as u32);
        }
        let mut mats = m.tri_materials();
        for f in fills {
            m.indices.extend(f.map(|p| any[&p]));
            mats.push(0);
        }
        m.regroup(&mats);
    }
    if r.any() {
        studio_mesh::ops::smooth_normals_in_place(m);
    }
    r
}

/// Fills a loop (welded ids, in order) with triangles by ear clipping in
/// the loop's best plane.
fn ear_clip(lp: &[u32], points: &[V3]) -> Vec<[u32; 3]> {
    let pts: Vec<V3> = lp.iter().map(|&i| points[i as usize]).collect();
    let c = scale(pts.iter().fold([0.0; 3], |s, p| add(s, *p)), 1.0 / pts.len() as f32);
    // Newell's normal.
    let mut nrm = [0.0f32; 3];
    for k in 0..pts.len() {
        let (a, b) = (pts[k], pts[(k + 1) % pts.len()]);
        nrm[0] += (a[1] - b[1]) * (a[2] + b[2]);
        nrm[1] += (a[2] - b[2]) * (a[0] + b[0]);
        nrm[2] += (a[0] - b[0]) * (a[1] + b[1]);
    }
    let nrm = normalise(nrm);
    let helper = if nrm[0].abs() < 0.9 { [1.0, 0.0, 0.0] } else { [0.0, 1.0, 0.0] };
    let ax = normalise(cross(helper, nrm));
    let ay = cross(nrm, ax);
    let p2: Vec<[f32; 2]> = pts.iter().map(|p| [dot(sub(*p, c), ax), dot(sub(*p, c), ay)]).collect();
    let mut rest: Vec<usize> = (0..lp.len()).collect();
    let mut out = Vec::new();
    let area2 = |a: [f32; 2], b: [f32; 2], c: [f32; 2]| (b[0] - a[0]) * (c[1] - a[1]) - (c[0] - a[0]) * (b[1] - a[1]);
    let mut guard = 0;
    while rest.len() > 3 && guard < lp.len() * lp.len() {
        guard += 1;
        let k = rest.len();
        let mut clipped = false;
        for i in 0..k {
            let (a, b, cc) = (rest[(i + k - 1) % k], rest[i], rest[(i + 1) % k]);
            if area2(p2[a], p2[b], p2[cc]) <= 0.0 {
                continue;
            }
            let inside = rest.iter().any(|&o| {
                o != a && o != b && o != cc && area2(p2[a], p2[b], p2[o]) > 0.0 && area2(p2[b], p2[cc], p2[o]) > 0.0 && area2(p2[cc], p2[a], p2[o]) > 0.0
            });
            if inside {
                continue;
            }
            out.push([lp[a], lp[b], lp[cc]]);
            rest.remove(i);
            clipped = true;
            break;
        }
        if !clipped {
            // Not simple in this plane: a fan from the centre would need a new
            // point; clip the flattest corner instead.
            let i = 0;
            let (a, b, cc) = (rest[(i + k - 1) % k], rest[i], rest[(i + 1) % k]);
            out.push([lp[a], lp[b], lp[cc]]);
            rest.remove(i);
        }
    }
    if rest.len() == 3 {
        out.push([lp[rest[0]], lp[rest[1]], lp[rest[2]]]);
    }
    let _ = length;
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn holes_and_flips_are_mended() {
        let mut m = studio_mesh::prim::by_name("sphere", 100.0, 3).unwrap();
        // Two faces gone, one turned over.
        let n = m.indices.len();
        m.indices.drain(n - 6..);
        m.indices.swap(0, 1);
        m.single_group();
        let r = repair(&mut m);
        assert!(r.holes >= 1 && r.flipped >= 1, "{r:?}");
        let t = crate::topo::analyse(&m);
        assert!(t.watertight && t.manifold, "{t:?}");
    }
}
