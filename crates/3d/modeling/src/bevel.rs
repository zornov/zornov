// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Beveling edges: each picked edge becomes a band `width` wide, cut flat
//! (a chamfer) or rounded over `segments` rows with a `profile` (0 flat,
//! 0.5 a quarter circle, toward 1 a squarer shoulder). Where bevels meet
//! they mitre or close with a rounded corner patch. Real objects have no
//! razor edges; a small bevel is what makes a hard-surface model catch the
//! light like a made thing.

use crate::poly::{Edge, Poly, Topology, ekey};
use std::collections::{HashMap, HashSet};
use studio_mesh::{V3, add, cross, dot, length, lerp, normalise, scale, sub};

/// The faces around a vertex in order, and the edge (other vertex) between
/// each pair: face `j` comes in along `spokes[j]` and leaves along
/// `spokes[j + 1]` (wrapping when the fan is closed).
struct Fan {
    faces: Vec<u32>,
    spokes: Vec<u32>,
    closed: bool,
}

impl Fan {
    fn m(&self) -> usize {
        self.faces.len()
    }
    /// Spoke number, wrapped for closed fans.
    fn spoke(&self, j: isize) -> Option<usize> {
        let n = if self.closed { self.m() } else { self.m() + 1 };
        if self.closed {
            Some(j.rem_euclid(n as isize) as usize)
        } else if j >= 0 && (j as usize) < n {
            Some(j as usize)
        } else {
            None
        }
    }
    fn face(&self, j: isize) -> Option<u32> {
        if self.closed {
            Some(self.faces[j.rem_euclid(self.m() as isize) as usize])
        } else if j >= 0 && (j as usize) < self.m() {
            Some(self.faces[j as usize])
        } else {
            None
        }
    }
}

fn fan(p: &Poly, t: &Topology, v: u32) -> Option<Fan> {
    let vf = &t.vertex_faces[v as usize];
    if vf.is_empty() {
        return None;
    }
    // prev and next at v in each face.
    let mut by_prev: HashMap<u32, u32> = HashMap::new();
    let mut nexts: HashMap<u32, u32> = HashMap::new();
    for &f in vf {
        let c = p.face(f as usize);
        let n = c.len();
        let i = c.iter().position(|&x| x == v)?;
        let (prev, next) = (c[(i + n - 1) % n], c[(i + 1) % n]);
        if by_prev.insert(prev, f).is_some() {
            return None;
        }
        nexts.insert(f, next);
    }
    // An open fan starts at the face whose incoming edge is open.
    let start = vf.iter().copied().find(|&f| {
        let c = p.face(f as usize);
        let i = c.iter().position(|&x| x == v).unwrap_or(0);
        let prev = c[(i + c.len() - 1) % c.len()];
        t.is_boundary(ekey(v, prev))
    });
    let closed = start.is_none();
    let first = start.unwrap_or(vf[0]);
    let mut faces = vec![first];
    let prev_of = |f: u32| {
        let c = p.face(f as usize);
        let i = c.iter().position(|&x| x == v).unwrap_or(0);
        c[(i + c.len() - 1) % c.len()]
    };
    let mut spokes = vec![prev_of(first)];
    let mut f = first;
    loop {
        let nx = nexts[&f];
        spokes.push(nx);
        match by_prev.get(&nx) {
            Some(&g) if g == first => break,
            Some(&g) => {
                if faces.contains(&g) {
                    return None;
                }
                faces.push(g);
                f = g;
            }
            None => break,
        }
    }
    if faces.len() != vf.len() {
        return None;
    }
    if closed {
        spokes.pop();
    }
    Some(Fan { faces, spokes, closed })
}

/// Bevels `edges` (closed-surface edges between two faces). Returns how
/// many edges were beveled.
pub fn bevel(p: &mut Poly, edges: &[Edge], width: f32, segments: usize, profile: f32) -> usize {
    let n = segments.clamp(1, 64);
    let t = Topology::of(p);
    let mut bev: HashSet<Edge> = edges.iter().map(|e| ekey(e.0, e.1)).filter(|e| t.edge_faces.get(e).is_some_and(|f| f.len() == 2)).collect();
    let mut fans: HashMap<u32, Fan> = HashMap::new();
    loop {
        let ends: HashSet<u32> = bev.iter().flat_map(|e| [e.0, e.1]).collect();
        let mut bad = HashSet::new();
        for v in ends {
            if let std::collections::hash_map::Entry::Vacant(slot) = fans.entry(v) {
                match fan(p, &t, v) {
                    Some(f) => {
                        slot.insert(f);
                    }
                    None => {
                        bad.insert(v);
                    }
                }
            }
        }
        if bad.is_empty() {
            break;
        }
        bev.retain(|e| !bad.contains(&e.0) && !bad.contains(&e.1));
    }
    if bev.is_empty() {
        return 0;
    }
    let profile = profile.clamp(0.0, 0.999);
    let mut profiles: HashMap<(u32, u32, u32), Vec<u32>> = HashMap::new();
    let mut replace: HashMap<(u32, u32), Vec<u32>> = HashMap::new();
    // Left/right points of each bevel at each end: (v, edge) -> (left, right).
    let mut ends: HashMap<(u32, Edge), (u32, u32)> = HashMap::new();
    let mut caps: Vec<(Vec<u32>, u32, V3)> = Vec::new();
    let mut verts: Vec<u32> = fans.keys().copied().collect();
    verts.sort_unstable();
    for v in verts {
        let fan = &fans[&v];
        let pv = p.positions[v as usize];
        let dir = |p: &Poly, u: u32| normalise(sub(p.positions[u as usize], pv));
        let len = |p: &Poly, u: u32| length(sub(p.positions[u as usize], pv));
        let nsp = fan.spokes.len();
        let beveled: Vec<usize> = (0..nsp).filter(|&s| bev.contains(&ekey(v, fan.spokes[s]))).collect();
        let k = beveled.len();
        if k == 0 {
            continue;
        }
        // The outward normal here, for orienting caps.
        let mut vn = [0.0; 3];
        for &f in &fan.faces {
            vn = add(vn, p.face_normal(f as usize));
        }
        let vn = normalise(vn);
        // Slide along spoke `s` far enough to sit `width` off the bevelled
        // spoke `b`.
        let slide = |p: &mut Poly, s: usize, b: usize| -> u32 {
            let (ds, db) = (dir(p, fan.spokes[s]), dir(p, fan.spokes[b]));
            let sin = length(cross(ds, db)).max(0.05);
            let d = (width / sin).min(0.48 * len(p, fan.spokes[s]));
            p.add_vertex(add(pv, scale(ds, d)))
        };
        let mat = p.face_material[fan.faces[0] as usize];
        if k == 1 {
            let s = beveled[0] as isize;
            let (Some(sl), Some(sr)) = (fan.spoke(s - 1), fan.spoke(s + 1)) else { continue };
            let left = slide(p, sl, s as usize);
            let right = slide(p, sr, s as usize);
            let fl = fan.face(s - 1).unwrap();
            let fr = fan.face(s).unwrap();
            replace.insert((fl, v), vec![left]);
            replace.insert((fr, v), vec![right]);
            ends.insert((v, ekey(v, fan.spokes[s as usize])), (left, right));
            let absorb = (fan.closed && fan.m() == 3) || (!fan.closed && fan.m() == 2);
            if absorb {
                if fan.closed {
                    let fo = fan.face(s + 1).unwrap();
                    let pr = profile_points(p, &mut profiles, v, left, right, n, profile);
                    replace.insert((fo, v), pr.into_iter().rev().collect());
                }
            } else {
                if let Some(fo) = fan.face(s + 1).filter(|f| *f != fl) {
                    replace.insert((fo, v), vec![right, v]);
                }
                if let Some(fo) = fan.face(s - 2).filter(|f| *f != fr) {
                    replace.insert((fo, v), vec![v, left]);
                }
                let pr = profile_points(p, &mut profiles, v, left, right, n, profile);
                let mut ring = vec![v];
                ring.extend(pr.into_iter().rev());
                caps.push((ring, mat, vn));
            }
            continue;
        }
        // Sectors between consecutive bevelled spokes, each with one point.
        let mut sector_pt: HashMap<usize, u32> = HashMap::new(); // keyed by the bevelled spoke that starts it
        for (i, &sa) in beveled.iter().enumerate() {
            let sb = if i + 1 < k { beveled[i + 1] } else if fan.closed { beveled[0] + nsp } else { usize::MAX };
            if sb == usize::MAX {
                continue;
            }
            // Open fans: the ends (before the first and after the last
            // bevel) slide along the open edge.
            let interior: Vec<usize> = (sa + 1..sb).map(|j| j % nsp).collect();
            let pt = if interior.is_empty() {
                let f = fan.face(sa as isize).unwrap() as usize;
                let (da, db) = (dir(p, fan.spokes[sa % nsp]), dir(p, fan.spokes[sb % nsp]));
                let theta = dot(da, db).clamp(-1.0, 1.0).acos();
                let mut bis = add(da, db);
                if length(bis) < 1e-3 {
                    bis = cross(p.face_normal(f), da);
                    if dot(bis, sub(p.face_centre(f), pv)) < 0.0 {
                        bis = scale(bis, -1.0);
                    }
                }
                let bis = normalise(bis);
                let lim = 0.48 * len(p, fan.spokes[sa % nsp]).min(len(p, fan.spokes[sb % nsp]));
                let d = (width / (theta / 2.0).sin().max(0.05)).min(lim / (theta / 2.0).cos().max(0.05));
                p.add_vertex(add(pv, scale(bis, d)))
            } else {
                let mid = interior[interior.len() / 2];
                let da = dir(p, fan.spokes[sa % nsp]);
                let db = dir(p, fan.spokes[sb % nsp]);
                let dm = dir(p, fan.spokes[mid]);
                let sin = length(cross(dm, da)).min(length(cross(dm, db))).max(0.05);
                let d = (width / sin).min(0.48 * len(p, fan.spokes[mid]));
                p.add_vertex(add(pv, scale(dm, d)))
            };
            sector_pt.insert(sa, pt);
            for j in sa..sb {
                if let Some(f) = fan.face(j as isize) {
                    replace.insert((f, v), vec![pt]);
                }
            }
        }
        // Open fans: faces before the first bevel and after the last.
        if !fan.closed {
            let (first, last) = (beveled[0], beveled[k - 1]);
            let before = {
                let pt = slide(p, 0, first);
                for j in 0..first {
                    if let Some(f) = fan.face(j as isize) {
                        replace.insert((f, v), vec![pt]);
                    }
                }
                pt
            };
            let after = {
                let pt = slide(p, nsp - 1, last);
                for j in last..fan.m() {
                    if let Some(f) = fan.face(j as isize) {
                        replace.insert((f, v), vec![pt]);
                    }
                }
                pt
            };
            sector_pt.insert(usize::MAX, before);
            sector_pt.insert(usize::MAX - 1, after);
        }
        // Each bevel's ends: left in the face before it, right after.
        let mut ring: Vec<u32> = Vec::new();
        for (i, &s) in beveled.iter().enumerate() {
            let prev_sector = if i > 0 { beveled[i - 1] } else if fan.closed { beveled[k - 1] } else { usize::MAX };
            let left = sector_pt[&prev_sector];
            let right = if i + 1 < k || fan.closed { sector_pt[&s] } else { sector_pt[&(usize::MAX - 1)] };
            ends.insert((v, ekey(v, fan.spokes[s])), (left, right));
            let pr = profile_points(p, &mut profiles, v, left, right, n, profile);
            ring.extend_from_slice(&pr[..pr.len() - 1]);
            if i + 1 == k && !fan.closed {
                ring.push(*pr.last().unwrap());
            }
        }
        if k >= 3 && fan.closed {
            ring.reverse();
            caps.push((ring, mat, vn));
        }
    }
    // Bands along each bevelled edge.
    let mut strips: Vec<(Vec<u32>, u32)> = Vec::new();
    for e in &bev {
        let fs = &t.edge_faces[e];
        // The face that runs e.0 -> e.1.
        let runs = |f: u32| {
            let c = p.face(f as usize);
            (0..c.len()).any(|i| c[i] == e.0 && c[(i + 1) % c.len()] == e.1)
        };
        let (a, b) = if runs(fs[0]) || runs(fs[1]) { (e.0, e.1) } else { (e.1, e.0) };
        let mat = p.face_material[fs[0] as usize];
        let (Some(&(la, ra)), Some(&(lb, rb))) = (ends.get(&(a, *e)), ends.get(&(b, *e))) else { continue };
        let pa = profile_points(p, &mut profiles, a, la, ra, n, profile);
        let pb = profile_points(p, &mut profiles, b, lb, rb, n, profile);
        for j in 0..n {
            strips.push((vec![pa[j], pa[j + 1], pb[n - j - 1], pb[n - j]], mat));
        }
    }
    // Rebuild faces.
    let mut out = Poly { positions: std::mem::take(&mut p.positions), sharp: std::mem::take(&mut p.sharp), seams: std::mem::take(&mut p.seams), materials: std::mem::take(&mut p.materials), ..Poly::new() };
    out.creases = p.creases.iter().filter(|(e, _)| !bev.contains(e)).map(|(e, w)| (*e, *w)).collect();
    for f in 0..p.faces() {
        let c = p.face(f);
        let mut vs = Vec::with_capacity(c.len() + 4);
        for &v in c {
            match replace.get(&(f as u32, v)) {
                Some(l) => vs.extend_from_slice(l),
                None => vs.push(v),
            }
        }
        out.add_face_like(&vs, None, p.face_material[f], p.face_set[f], p.face_smooth[f]);
    }
    for (q, m) in strips {
        out.add_face_like(&q, None, m, 0, true);
    }
    for (ring, m, vn) in caps {
        add_cap(&mut out, ring, m, vn, profile);
    }
    out.uvs.clear();
    out.sharp.retain(|e| !bev.contains(e));
    out.seams.retain(|e| !bev.contains(e));
    out.clean();
    out.compact();
    *p = out;
    bev.len()
}

/// Points of the curve across a bevel at vertex `v` from `from` to `to`
/// (both ends included), shared by every face that uses them.
fn profile_points(p: &mut Poly, cache: &mut HashMap<(u32, u32, u32), Vec<u32>>, v: u32, from: u32, to: u32, n: usize, profile: f32) -> Vec<u32> {
    if let Some(l) = cache.get(&(v, from, to)) {
        return l.clone();
    }
    if let Some(l) = cache.get(&(v, to, from)) {
        return l.iter().rev().copied().collect();
    }
    let (a, c, b) = (p.positions[from as usize], p.positions[v as usize], p.positions[to as usize]);
    let phi = dot(normalise(sub(a, c)), normalise(sub(b, c))).clamp(-1.0, 1.0).acos();
    let w = (phi / 2.0).sin() * (profile * std::f32::consts::FRAC_PI_2).tan();
    let mut ids = vec![from];
    for j in 1..n {
        let t = j as f32 / n as f32;
        let (b0, b1, b2) = ((1.0 - t) * (1.0 - t), 2.0 * t * (1.0 - t) * w, t * t);
        let s = b0 + b1 + b2;
        let q = scale(add(add(scale(a, b0), scale(c, b1)), scale(b, b2)), 1.0 / s);
        ids.push(p.add_vertex(q));
    }
    ids.push(to);
    cache.insert((v, from, to), ids.clone());
    ids
}

/// Closes the hole where three or more bevels meet.
fn add_cap(p: &mut Poly, mut ring: Vec<u32>, mat: u32, outward: V3, profile: f32) {
    ring.dedup();
    while ring.len() > 1 && ring.first() == ring.last() {
        ring.pop();
    }
    if ring.len() < 3 {
        return;
    }
    let tmp = Poly::from_faces(ring.iter().map(|&v| p.positions[v as usize]).collect(), &[(0..ring.len() as u32).collect()]);
    if dot(tmp.face_area_vector(0), outward) < 0.0 {
        ring.reverse();
    }
    if ring.len() <= 4 {
        p.add_face(&ring, mat);
        return;
    }
    // A fan to a centre lifted toward the old corner.
    let mut c = [0.0; 3];
    for &v in &ring {
        c = add(c, p.positions[v as usize]);
    }
    c = scale(c, 1.0 / ring.len() as f32);
    let lift = (0.25 * profile / 0.5).min(0.6);
    let r: f32 = ring.iter().map(|&v| length(sub(p.positions[v as usize], c))).sum::<f32>() / ring.len() as f32;
    let centre = p.add_vertex(add(c, scale(outward, r * lift)));
    for i in 0..ring.len() {
        p.add_face(&[ring[i], ring[(i + 1) % ring.len()], centre], mat);
    }
    let _ = lerp;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prim;
    use crate::select::Edges;

    fn closed(p: &Poly) -> bool {
        Topology::of(p).boundary_edges().is_empty()
    }

    fn volume(p: &Poly) -> f32 {
        let mut v = 0.0;
        for f in 0..p.faces() {
            let c = p.face(f);
            for t in crate::poly::triangulate(&p.positions, c) {
                let (a, b, d) = (p.positions[c[t[0]] as usize], p.positions[c[t[1]] as usize], p.positions[c[t[2]] as usize]);
                v += dot(a, cross(b, d)) / 6.0;
            }
        }
        v
    }

    #[test]
    fn one_edge_chamfers_the_box() {
        let mut c = prim::cube([2.0; 3], 1);
        let e = crate::select::nearest_edge(&c, [1.0, -1.0, 0.0]).unwrap();
        assert_eq!(bevel(&mut c, &[e], 0.2, 1, 0.5), 1);
        assert!(closed(&c), "{}", c.stats());
        assert_eq!(c.faces(), 7);
        assert!((volume(&c) - (8.0 - 0.04)).abs() < 1e-3, "{}", volume(&c));
    }

    #[test]
    fn every_edge_of_a_box_rounds_and_closes() {
        for segs in [1, 2, 4] {
            let mut c = prim::cube([2.0; 3], 1);
            let all = Edges::All.list(&c);
            bevel(&mut c, &all, 0.1, segs, 0.5);
            assert!(closed(&c), "segments {segs}: {}", c.stats());
            let v = volume(&c);
            assert!(v < 8.0 && v > 7.85, "segments {segs}: volume {v}");
            for f in 0..c.faces() {
                assert!(c.face_area(f) > 1e-6, "segments {segs}: face {f} is degenerate");
            }
        }
    }

    #[test]
    fn two_bevels_meeting_mitre() {
        let mut c = prim::cube([2.0; 3], 1);
        let e1 = crate::select::nearest_edge(&c, [1.0, -1.0, 0.0]).unwrap();
        let e2 = crate::select::nearest_edge(&c, [0.0, -1.0, 1.0]).unwrap();
        bevel(&mut c, &[e1, e2], 0.2, 3, 0.5);
        assert!(closed(&c), "{}", c.stats());
    }

    #[test]
    fn a_bevelled_cylinder_rim() {
        let mut c = prim::cylinder(1.0, 1.0, 2.0, 24, 1, prim::Cap::Ngon);
        let rim = Edges::Sharper { degrees: 45.0 }.list(&c);
        assert_eq!(rim.len(), 48);
        bevel(&mut c, &rim, 0.1, 3, 0.5);
        assert!(closed(&c), "{}", c.stats());
    }

    #[test]
    fn bevels_on_an_open_sheet() {
        // A bent sheet (an L of two quads): bevel the fold.
        let pts = vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 0.0, 1.0], [0.0, 0.0, 1.0], [0.0, -1.0, 1.0], [1.0, -1.0, 1.0]];
        let mut p = Poly::from_faces(pts, &[vec![0, 3, 2, 1], vec![3, 4, 5, 2]]);
        let e = ekey(2, 3);
        assert_eq!(bevel(&mut p, &[e], 0.2, 2, 0.5), 1);
        assert_eq!(p.faces(), 4);
        assert_eq!(Topology::of(&p).boundary_edges().len(), 4 + 2 * 3);
    }
}
