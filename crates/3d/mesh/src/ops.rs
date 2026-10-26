// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Modeling operations. Each takes a mesh and changes it in place; vertex
//! seams (where a position has several normals or texture coordinates)
//! survive, because geometry is worked on welded positions and the
//! seam vertices follow.

use crate::region::Region;
use crate::{Group, Mesh, V3, add, cross, dot, length, lerp, normalise, scale, sub};
use std::collections::{HashMap, HashSet};

/// Smooth normals: each vertex the area-weighted average of the faces
/// around its position (seams in texture coordinates do not show).
pub fn smooth_normals_in_place(m: &mut Mesh) {
    let (ids, unique) = m.position_ids();
    let mut acc = vec![[0.0f32; 3]; unique.len()];
    for t in 0..m.triangles() {
        let [a, b, c] = m.tri(t);
        let n = cross(sub(m.positions[b], m.positions[a]), sub(m.positions[c], m.positions[a]));
        for v in [a, b, c] {
            acc[ids[v] as usize] = add(acc[ids[v] as usize], n);
        }
    }
    m.normals = ids.iter().map(|&i| normalise(acc[i as usize])).collect();
}

/// Normals again: smooth across edges whose faces meet at less than
/// `angle` degrees, sharp (split) where they meet at more. 0 = every face
/// flat; 180 = all smooth.
pub fn recompute_normals(m: &mut Mesh, angle: f32) {
    let cos = angle.clamp(0.0, 180.0).to_radians().cos();
    let (ids, _) = m.position_ids();
    let n = m.triangles();
    let face: Vec<V3> = (0..n).map(|t| cross(sub(m.positions[m.tri(t)[1]], m.positions[m.tri(t)[0]]), sub(m.positions[m.tri(t)[2]], m.positions[m.tri(t)[0]]))).collect();
    let mut around: HashMap<u32, Vec<usize>> = HashMap::new();
    for t in 0..n {
        for v in m.tri(t) {
            around.entry(ids[v]).or_default().push(t);
        }
    }
    // A new vertex per (vertex, normal), shared by corners that agree.
    let mut out = Mesh { materials: m.materials.clone(), groups: m.groups.clone(), ..Default::default() };
    let mut made: HashMap<(usize, [i32; 3]), u32> = HashMap::new();
    for t in 0..n {
        let fu = normalise(face[t]);
        for v in m.tri(t) {
            let mut acc = [0.0f32; 3];
            for &o in &around[&ids[v]] {
                if dot(normalise(face[o]), fu) >= cos - 1e-6 {
                    acc = add(acc, face[o]);
                }
            }
            let nn = normalise(acc);
            let key = (v, [(nn[0] * 1000.0) as i32, (nn[1] * 1000.0) as i32, (nn[2] * 1000.0) as i32]);
            let i = *made.entry(key).or_insert_with(|| {
                out.positions.push(m.positions[v]);
                out.normals.push(nn);
                out.uvs.push(m.uvs.get(v).copied().unwrap_or([0.0; 2]));
                (out.positions.len() - 1) as u32
            });
            out.indices.push(i);
        }
    }
    *m = out;
}

/// Turns every triangle inside out.
pub fn flip(m: &mut Mesh) {
    for t in 0..m.triangles() {
        m.indices.swap(3 * t + 1, 3 * t + 2);
    }
    for n in &mut m.normals {
        *n = scale(*n, -1.0);
    }
}

/// Rotation matrix (rows) from degrees about x, then y, then z.
pub fn rotation(deg: V3) -> [V3; 3] {
    let (a, b, c) = (deg[0].to_radians(), deg[1].to_radians(), deg[2].to_radians());
    let rx = [[1.0, 0.0, 0.0], [0.0, a.cos(), -a.sin()], [0.0, a.sin(), a.cos()]];
    let ry = [[b.cos(), 0.0, b.sin()], [0.0, 1.0, 0.0], [-b.sin(), 0.0, b.cos()]];
    let rz = [[c.cos(), -c.sin(), 0.0], [c.sin(), c.cos(), 0.0], [0.0, 0.0, 1.0]];
    mul(&rz, &mul(&ry, &rx))
}

fn mul(a: &[V3; 3], b: &[V3; 3]) -> [V3; 3] {
    std::array::from_fn(|i| std::array::from_fn(|j| a[i][0] * b[0][j] + a[i][1] * b[1][j] + a[i][2] * b[2][j]))
}

fn apply(r: &[V3; 3], v: V3) -> V3 {
    [dot(r[0], v), dot(r[1], v), dot(r[2], v)]
}

/// Moves, turns (degrees about x, y, z, around the origin) and scales
/// (per axis) the whole model, or only `region`.
pub fn transform(m: &mut Mesh, region: &Region, translate: V3, rotate: V3, scale_by: V3, pivot: V3) {
    let r = rotation(rotate);
    let sel = region.vertices(m);
    let flips = scale_by.iter().filter(|s| **s < 0.0).count() % 2 == 1;
    for (i, p) in m.positions.iter_mut().enumerate() {
        if !sel[i] {
            continue;
        }
        let local = sub(*p, pivot);
        let s = [local[0] * scale_by[0], local[1] * scale_by[1], local[2] * scale_by[2]];
        *p = add(add(apply(&r, s), pivot), translate);
    }
    for (i, n) in m.normals.iter_mut().enumerate() {
        if sel[i] {
            // Normals take the inverse scale.
            let inv = [n[0] / non_zero(scale_by[0]), n[1] / non_zero(scale_by[1]), n[2] / non_zero(scale_by[2])];
            *n = normalise(apply(&r, inv));
        }
    }
    if flips && matches!(region, Region::All) {
        for t in 0..m.triangles() {
            m.indices.swap(3 * t + 1, 3 * t + 2);
        }
    }
    if matches!(region, Region::All)
        && let Some(rig) = m.rig.as_mut()
    {
        let k = (scale_by[0] * scale_by[1] * scale_by[2]).abs().cbrt();
        std::sync::Arc::make_mut(rig).map_points(
            |p| {
                let l = sub(p, pivot);
                add(add(apply(&r, [l[0] * scale_by[0], l[1] * scale_by[1], l[2] * scale_by[2]]), pivot), translate)
            },
            |d| apply(&r, [d[0] * scale_by[0], d[1] * scale_by[1], d[2] * scale_by[2]]),
            k,
        );
    }
}

fn non_zero(v: f32) -> f32 {
    if v.abs() < 1e-6 { 1e-6 } else { v }
}

/// Centres the model on the origin and scales it so its largest side is
/// `size` pixels (0 keeps the size).
pub fn centre_and_fit(m: &mut Mesh, size: f32) {
    let (lo, hi) = m.bounds();
    let c = scale(add(lo, hi), 0.5);
    let big = (hi[0] - lo[0]).max(hi[1] - lo[1]).max(hi[2] - lo[2]);
    let k = if size > 0.0 && big > 1e-6 { size / big } else { 1.0 };
    for p in &mut m.positions {
        *p = scale(sub(*p, c), k);
    }
    if let Some(rig) = m.rig.as_mut() {
        std::sync::Arc::make_mut(rig).map_points(|p| scale(sub(p, c), k), |d| scale(d, k), k);
    }
}

/// Neighbouring positions of each welded position.
fn neighbours(m: &Mesh, ids: &[u32], count: usize) -> Vec<Vec<u32>> {
    let mut nb: Vec<HashSet<u32>> = vec![HashSet::new(); count];
    for t in 0..m.triangles() {
        let [a, b, c] = m.tri(t).map(|v| ids[v]);
        for (x, y) in [(a, b), (b, c), (c, a)] {
            if x != y {
                nb[x as usize].insert(y);
                nb[y as usize].insert(x);
            }
        }
    }
    nb.into_iter().map(|s| s.into_iter().collect()).collect()
}

/// Edges on the boundary (used by one triangle), by welded ids.
fn boundary_edges(m: &Mesh, ids: &[u32]) -> HashSet<(u32, u32)> {
    let mut count: HashMap<(u32, u32), u32> = HashMap::new();
    for t in 0..m.triangles() {
        let [a, b, c] = m.tri(t).map(|v| ids[v]);
        for (x, y) in [(a, b), (b, c), (c, a)] {
            *count.entry((x.min(y), x.max(y))).or_default() += 1;
        }
    }
    count.into_iter().filter(|(_, n)| *n == 1).map(|(e, _)| e).collect()
}

/// Loop subdivision, `levels` times: every triangle becomes four and the
/// surface rounds off (boundaries keep their curve). Texture coordinates
/// follow; each level multiplies the triangles by four (capped at about
/// two million).
pub fn subdivide(m: &mut Mesh, levels: usize) {
    for _ in 0..levels {
        if m.triangles() * 4 > 2_000_000 {
            break;
        }
        subdivide_once(m);
    }
    smooth_normals_in_place(m);
}

fn subdivide_once(m: &mut Mesh) {
    let (ids, unique) = m.position_ids();
    let nb = neighbours(m, &ids, unique.len());
    let border = boundary_edges(m, &ids);
    let on_border: HashSet<u32> = border.iter().flat_map(|(a, b)| [*a, *b]).collect();
    // Opposite corners of each edge (for the edge points' weights).
    let mut opposite: HashMap<(u32, u32), Vec<u32>> = HashMap::new();
    for t in 0..m.triangles() {
        let [a, b, c] = m.tri(t).map(|v| ids[v]);
        for (x, y, o) in [(a, b, c), (b, c, a), (c, a, b)] {
            opposite.entry((x.min(y), x.max(y))).or_default().push(o);
        }
    }
    // New positions of the old points.
    let moved: Vec<V3> = (0..unique.len())
        .map(|i| {
            let p = unique[i];
            if on_border.contains(&(i as u32)) {
                let ends: Vec<u32> = nb[i].iter().copied().filter(|&o| border.contains(&((i as u32).min(o), (i as u32).max(o)))).collect();
                if ends.len() == 2 {
                    return add(scale(p, 0.75), scale(add(unique[ends[0] as usize], unique[ends[1] as usize]), 0.125));
                }
                return p;
            }
            let n = nb[i].len().max(1) as f32;
            let beta = if n == 3.0 { 3.0 / 16.0 } else { 3.0 / (8.0 * n) };
            let sum = nb[i].iter().fold([0.0; 3], |s, &o| add(s, unique[o as usize]));
            add(scale(p, 1.0 - n * beta), scale(sum, beta))
        })
        .collect();
    let edge_point = |a: u32, b: u32| -> V3 {
        let k = (a.min(b), a.max(b));
        let (pa, pb) = (unique[a as usize], unique[b as usize]);
        match opposite.get(&k) {
            Some(o) if o.len() == 2 && !border.contains(&k) => add(scale(add(pa, pb), 0.375), scale(add(unique[o[0] as usize], unique[o[1] as usize]), 0.125)),
            _ => scale(add(pa, pb), 0.5),
        }
    };
    let mut out = Mesh { materials: m.materials.clone(), ..Default::default() };
    for v in 0..m.positions.len() {
        out.positions.push(moved[ids[v] as usize]);
        out.normals.push(m.normals.get(v).copied().unwrap_or([0.0; 3]));
        out.uvs.push(m.uvs.get(v).copied().unwrap_or([0.0; 2]));
    }
    // One new vertex per (vertex pair) so texture seams stay apart.
    let mut mid: HashMap<(u32, u32), u32> = HashMap::new();
    let mats = m.tri_materials();
    let mut new_mats = Vec::with_capacity(mats.len() * 4);
    for t in 0..m.triangles() {
        let [a, b, c] = m.tri(t).map(|v| v as u32);
        let mut half = |x: u32, y: u32, out: &mut Mesh| -> u32 {
            *mid.entry((x.min(y), x.max(y))).or_insert_with(|| {
                out.positions.push(edge_point(ids[x as usize], ids[y as usize]));
                out.normals.push(normalise(add(m.normals[x as usize], m.normals[y as usize])));
                let (ux, uy) = (m.uvs[x as usize], m.uvs[y as usize]);
                out.uvs.push([(ux[0] + uy[0]) / 2.0, (ux[1] + uy[1]) / 2.0]);
                (out.positions.len() - 1) as u32
            })
        };
        let (ab, bc, ca) = (half(a, b, &mut out), half(b, c, &mut out), half(c, a, &mut out));
        out.indices.extend_from_slice(&[a, ab, ca, b, bc, ab, c, ca, bc, ab, bc, ca]);
        new_mats.extend_from_slice(&[mats[t]; 4]);
    }
    out.regroup(&new_mats);
    *m = out;
}

/// Taubin smoothing (no shrinking): `iterations` rounds, `strength` 0..1.
pub fn smooth(m: &mut Mesh, region: &Region, iterations: usize, strength: f32) {
    let (ids, mut unique) = m.position_ids();
    let nb = neighbours(m, &ids, unique.len());
    let sel = region.positions(m, &ids, unique.len());
    let border: HashSet<u32> = boundary_edges(m, &ids).into_iter().flat_map(|(a, b)| [a, b]).collect();
    let (lambda, mu) = (0.6307 * strength.clamp(0.0, 1.0), -0.6732 * strength.clamp(0.0, 1.0));
    for _ in 0..iterations.min(200) {
        for k in [lambda, mu] {
            let cur = unique.clone();
            for i in 0..cur.len() {
                if !sel[i] || border.contains(&(i as u32)) || nb[i].is_empty() {
                    continue;
                }
                let avg = scale(nb[i].iter().fold([0.0; 3], |s, &o| add(s, cur[o as usize])), 1.0 / nb[i].len() as f32);
                unique[i] = add(cur[i], scale(sub(avg, cur[i]), k));
            }
        }
    }
    for (v, p) in m.positions.iter_mut().enumerate() {
        *p = unique[ids[v] as usize];
    }
    smooth_normals_in_place(m);
}

/// Sculpt brushes, as a sculptor's strokes are: each pushes the surface
/// near `centre` within `radius` (smooth falloff), by `strength` pixels
/// (or 0..1 for smooth and flatten).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Brush {
    /// Out along the surface (negative: in).
    Inflate,
    /// The surface near the centre moves by `by`.
    Grab { by: V3 },
    /// Evens out bumps.
    Smooth,
    /// Presses towards the average plane under the brush.
    Flatten,
    /// Pulls towards the brush centre along the surface (sharpens ridges).
    Pinch,
    /// Cuts a groove (negative: a ridge) with pinched edges.
    Crease,
}

impl Brush {
    pub fn parse(name: &str, by: Option<V3>) -> Option<Brush> {
        Some(match name {
            "inflate" | "draw" | "push_out" | "bulge" => Brush::Inflate,
            "grab" | "move" | "pull" => Brush::Grab { by: by.unwrap_or([0.0; 3]) },
            "smooth" | "relax" => Brush::Smooth,
            "flatten" | "plane" => Brush::Flatten,
            "pinch" | "sharpen" => Brush::Pinch,
            "crease" | "groove" | "carve" => Brush::Crease,
            _ => return None,
        })
    }
}

/// Smooth falloff: 1 at the centre, 0 at the edge.
fn falloff(d: f32, radius: f32) -> f32 {
    let x = (d / radius.max(1e-6)).clamp(0.0, 1.0);
    let t = 1.0 - x * x;
    t * t * t
}

/// One brush stroke at `centre`. With `mirror_x`, the same stroke also
/// happens mirrored across x = 0 (symmetric sculpting).
pub fn sculpt(m: &mut Mesh, brush: Brush, centre: V3, radius: f32, strength: f32, mirror_x: bool) {
    let mut centres = vec![(centre, brush)];
    if mirror_x && centre[0].abs() > 1e-3 {
        let b = match brush {
            Brush::Grab { by } => Brush::Grab { by: [-by[0], by[1], by[2]] },
            other => other,
        };
        centres.push(([-centre[0], centre[1], centre[2]], b));
    }
    for (c, b) in centres {
        stroke(m, b, c, radius, strength);
    }
    smooth_normals_in_place(m);
}

fn stroke(m: &mut Mesh, brush: Brush, centre: V3, radius: f32, strength: f32) {
    if m.normals.len() != m.positions.len() {
        smooth_normals_in_place(m);
    }
    let (ids, unique) = m.position_ids();
    // A normal per welded position.
    let mut pn = vec![[0.0f32; 3]; unique.len()];
    for (v, &i) in ids.iter().enumerate() {
        pn[i as usize] = add(pn[i as usize], m.normals[v]);
    }
    let pn: Vec<V3> = pn.into_iter().map(normalise).collect();
    let inside: Vec<(usize, f32)> = unique.iter().enumerate().filter_map(|(i, p)| {
        let d = length(sub(*p, centre));
        (d < radius).then(|| (i, falloff(d, radius)))
    }).collect();
    if inside.is_empty() {
        return;
    }
    let total: f32 = inside.iter().map(|(_, w)| w).sum::<f32>().max(1e-6);
    let avg_n = normalise(inside.iter().fold([0.0; 3], |s, (i, w)| add(s, scale(pn[*i], *w))));
    let avg_p = scale(inside.iter().fold([0.0; 3], |s, (i, w)| add(s, scale(unique[*i], *w))), 1.0 / total);
    let nb = matches!(brush, Brush::Smooth).then(|| neighbours(m, &ids, unique.len()));
    let mut out = unique.clone();
    for &(i, w) in &inside {
        let p = unique[i];
        out[i] = match brush {
            Brush::Inflate => add(p, scale(pn[i], strength * w)),
            Brush::Grab { by } => add(p, scale(by, w)),
            Brush::Smooth => {
                let n = &nb.as_ref().expect("made")[i];
                if n.is_empty() {
                    p
                } else {
                    let avg = scale(n.iter().fold([0.0; 3], |s, &o| add(s, unique[o as usize])), 1.0 / n.len() as f32);
                    lerp(p, avg, (strength.clamp(0.0, 1.0)) * w)
                }
            }
            Brush::Flatten => {
                let off = dot(sub(p, avg_p), avg_n);
                sub(p, scale(avg_n, off * strength.clamp(0.0, 1.0) * w))
            }
            Brush::Pinch => {
                let to = sub(centre, p);
                let along = sub(to, scale(pn[i], dot(to, pn[i])));
                add(p, scale(along, (strength / radius.max(1.0)).clamp(-1.0, 1.0) * w))
            }
            Brush::Crease => {
                let to = sub(centre, p);
                let along = sub(to, scale(pn[i], dot(to, pn[i])));
                add(add(p, scale(pn[i], -strength * w)), scale(along, 0.3 * w))
            }
        };
    }
    for (v, p) in m.positions.iter_mut().enumerate() {
        *p = out[ids[v] as usize];
    }
}

/// Makes the model symmetric across x = 0, keeping the side `keep` (+1 or
/// -1) and mirroring it to the other: what a sculptor does after working
/// one half of a face.
pub fn symmetrize(m: &mut Mesh, keep: f32) {
    let sign = if keep < 0.0 { -1.0 } else { 1.0 };
    let mats = m.tri_materials();
    let mut kept = Mesh { materials: m.materials.clone(), ..Default::default() };
    let mut map: HashMap<usize, u32> = HashMap::new();
    let mut kept_mats = Vec::new();
    for t in 0..m.triangles() {
        if m.centroid(t)[0] * sign < -1e-4 {
            continue;
        }
        let mut tri = [0u32; 3];
        for (k, v) in m.tri(t).into_iter().enumerate() {
            tri[k] = *map.entry(v).or_insert_with(|| {
                let mut p = m.positions[v];
                // Points near the middle land exactly on it, so the halves weld.
                if p[0] * sign < 1e-3 * (1.0 + p[0].abs()) && p[0] * sign < 0.5 {
                    p[0] = 0.0;
                }
                kept.positions.push(p);
                kept.normals.push(m.normals[v]);
                kept.uvs.push(m.uvs.get(v).copied().unwrap_or([0.0; 2]));
                (kept.positions.len() - 1) as u32
            });
        }
        kept.indices.extend_from_slice(&tri);
        kept_mats.push(mats[t]);
    }
    let n = kept.positions.len() as u32;
    let nt = kept.triangles();
    for v in 0..n as usize {
        let p = kept.positions[v];
        let nn = kept.normals[v];
        kept.positions.push([-p[0], p[1], p[2]]);
        kept.normals.push([-nn[0], nn[1], nn[2]]);
        kept.uvs.push(kept.uvs[v]);
    }
    for t in 0..nt {
        let [a, b, c] = kept.tri(t).map(|v| v as u32);
        kept.indices.extend_from_slice(&[a + n, c + n, b + n]);
        kept_mats.push(kept_mats[t]);
    }
    kept.regroup(&kept_mats);
    *m = kept;
    weld(m);
    smooth_normals_in_place(m);
}

/// A mirrored copy added across the plane `axis` = 0 (0 x, 1 y, 2 z).
pub fn mirror(m: &mut Mesh, axis: usize) {
    let a = axis.min(2);
    let mut copy = m.clone();
    for p in &mut copy.positions {
        p[a] = -p[a];
    }
    for n in &mut copy.normals {
        n[a] = -n[a];
    }
    for t in 0..copy.triangles() {
        copy.indices.swap(3 * t + 1, 3 * t + 2);
    }
    merge(m, &copy);
    weld(m);
}

/// Joins vertices that share position, normal and texture coordinates.
pub fn weld(m: &mut Mesh) {
    let mut map: HashMap<[i64; 8], u32> = HashMap::new();
    let mut out = Mesh { materials: m.materials.clone(), groups: m.groups.clone(), ..Default::default() };
    let q = |x: f32, k: f32| (x * k).round() as i64;
    let mut remap = Vec::with_capacity(m.positions.len());
    for v in 0..m.positions.len() {
        let (p, n, uv) = (m.positions[v], m.normals.get(v).copied().unwrap_or([0.0; 3]), m.uvs.get(v).copied().unwrap_or([0.0; 2]));
        let key = [q(p[0], 1000.0), q(p[1], 1000.0), q(p[2], 1000.0), q(n[0], 100.0), q(n[1], 100.0), q(n[2], 100.0), q(uv[0], 10000.0), q(uv[1], 10000.0)];
        let i = *map.entry(key).or_insert_with(|| {
            out.positions.push(p);
            out.normals.push(n);
            out.uvs.push(uv);
            (out.positions.len() - 1) as u32
        });
        remap.push(i);
    }
    out.indices = m.indices.iter().map(|&i| remap[i as usize]).collect();
    *m = out;
}

/// Pushes the faces in `region` out along their average normal by
/// `distance` pixels, with new sides joining them to the rest (an arm out
/// of a body, a nose out of a face); negative pushes in.
pub fn extrude(m: &mut Mesh, region: &Region, distance: f32) -> usize {
    let faces: Vec<usize> = region.faces(m);
    if faces.is_empty() {
        return 0;
    }
    let (ids, _) = m.position_ids();
    let dir = normalise(faces.iter().fold([0.0; 3], |s, &t| add(s, m.face_normal(t))));
    let face_set: HashSet<usize> = faces.iter().copied().collect();
    // Edges (welded) used by exactly one chosen face are the rim.
    let mut edge_count: HashMap<(u32, u32), (u32, (usize, usize))> = HashMap::new();
    for &t in &faces {
        let tri = m.tri(t);
        for (x, y) in [(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])] {
            let k = (ids[x].min(ids[y]), ids[x].max(ids[y]));
            let e = edge_count.entry(k).or_insert((0, (x, y)));
            e.0 += 1;
            e.1 = (x, y);
        }
    }
    let mats = m.tri_materials();
    let mut new_mats = mats.clone();
    // New copies of every vertex the chosen faces use.
    let mut copy: HashMap<usize, u32> = HashMap::new();
    for &t in &faces {
        for v in m.tri(t) {
            copy.entry(v).or_insert_with(|| {
                m.positions.push(add(m.positions[v], scale(dir, distance)));
                m.normals.push(m.normals[v]);
                m.uvs.push(m.uvs.get(v).copied().unwrap_or([0.0; 2]));
                (m.positions.len() - 1) as u32
            });
        }
    }
    // Rim walls: old edge to new edge, wound like the face that owned it.
    let mut walls = Vec::new();
    for (_, (n, (x, y))) in &edge_count {
        if *n == 1 {
            let (nx, ny) = (copy[x], copy[y]);
            walls.extend_from_slice(&[*x as u32, *y as u32, ny, *x as u32, ny, nx]);
        }
    }
    for &t in &faces {
        for k in 0..3 {
            let v = m.indices[3 * t + k] as usize;
            m.indices[3 * t + k] = copy[&v];
        }
    }
    let wall_mat = faces.first().map_or(0, |&t| mats[t]);
    let n_walls = walls.len() / 3;
    if distance < 0.0 {
        // Pushed in: the walls face the other way.
        for w in walls.chunks_mut(3) {
            w.swap(1, 2);
        }
    }
    m.indices.extend_from_slice(&walls);
    new_mats.extend(std::iter::repeat_n(wall_mat, n_walls));
    let _ = face_set;
    m.regroup(&new_mats);
    recompute_normals(m, 40.0);
    faces.len()
}

/// Smooth value noise in 3D (deterministic from `seed`).
fn noise(p: V3, seed: u32) -> f32 {
    let h = |x: i32, y: i32, z: i32| -> f32 {
        let mut n = (x as u32).wrapping_mul(0x8da6_b343) ^ (y as u32).wrapping_mul(0xd816_3841) ^ (z as u32).wrapping_mul(0xcb1a_b31f) ^ seed.wrapping_mul(0x9e37_79b9);
        n ^= n >> 13;
        n = n.wrapping_mul(0x5bd1_e995);
        n ^= n >> 15;
        (n & 0xffff) as f32 / 32767.5 - 1.0
    };
    let f = [p[0].floor(), p[1].floor(), p[2].floor()];
    let t: V3 = std::array::from_fn(|i| {
        let x = p[i] - f[i];
        x * x * (3.0 - 2.0 * x)
    });
    let (x, y, z) = (f[0] as i32, f[1] as i32, f[2] as i32);
    let l = |a: f32, b: f32, t: f32| a + (b - a) * t;
    l(
        l(l(h(x, y, z), h(x + 1, y, z), t[0]), l(h(x, y + 1, z), h(x + 1, y + 1, z), t[0]), t[1]),
        l(l(h(x, y, z + 1), h(x + 1, y, z + 1), t[0]), l(h(x, y + 1, z + 1), h(x + 1, y + 1, z + 1), t[0]), t[1]),
        t[2],
    )
}

/// Layers of noise: rough, natural detail.
pub fn fbm(p: V3, octaves: usize, seed: u32) -> f32 {
    let (mut sum, mut amp, mut freq) = (0.0, 0.5, 1.0);
    for o in 0..octaves.clamp(1, 8) {
        sum += amp * noise(scale(p, freq), seed.wrapping_add(o as u32 * 101));
        amp *= 0.5;
        freq *= 2.0;
    }
    sum
}

/// Pushes the surface in and out along its normals by noise: `amount`
/// pixels, features about `size` pixels across (rock, bark, skin pores
/// at small sizes).
pub fn displace(m: &mut Mesh, region: &Region, amount: f32, size: f32, octaves: usize, seed: u32) {
    if m.normals.len() != m.positions.len() {
        smooth_normals_in_place(m);
    }
    let (ids, unique) = m.position_ids();
    let sel = region.positions(m, &ids, unique.len());
    let mut pn = vec![[0.0f32; 3]; unique.len()];
    for (v, &i) in ids.iter().enumerate() {
        pn[i as usize] = add(pn[i as usize], m.normals[v]);
    }
    let moved: Vec<V3> = unique.iter().enumerate().map(|(i, p)| if sel[i] { add(*p, scale(normalise(pn[i]), amount * fbm(scale(*p, 1.0 / size.max(1.0)), octaves, seed))) } else { *p }).collect();
    for (v, p) in m.positions.iter_mut().enumerate() {
        *p = moved[ids[v] as usize];
    }
    smooth_normals_in_place(m);
}

/// Adds `other` into `m` (materials appended).
pub fn merge(m: &mut Mesh, other: &Mesh) {
    let base = m.positions.len() as u32;
    let mat_base = m.materials.len() as u32;
    let mut mats = m.tri_materials();
    m.positions.extend_from_slice(&other.positions);
    m.normals.extend_from_slice(&other.normals);
    m.uvs.extend_from_slice(&other.uvs);
    m.indices.extend(other.indices.iter().map(|i| i + base));
    m.materials.extend(other.materials.iter().cloned());
    mats.extend(other.tri_materials().into_iter().map(|k| k + mat_base));
    m.regroup(&mats);
}

/// `count` copies in a row, each `offset` further on.
pub fn array(m: &mut Mesh, count: usize, offset: V3) {
    let one = m.clone();
    for k in 1..count.clamp(1, 256) {
        let mut c = one.clone();
        for p in &mut c.positions {
            *p = add(*p, scale(offset, k as f32));
        }
        let base = m.positions.len() as u32;
        let mut mats = m.tri_materials();
        m.positions.extend_from_slice(&c.positions);
        m.normals.extend_from_slice(&c.normals);
        m.uvs.extend_from_slice(&c.uvs);
        m.indices.extend(c.indices.iter().map(|i| i + base));
        mats.extend(c.tri_materials());
        m.regroup(&mats);
    }
}

/// Texture coordinates projected: `box` (each face from its main axis),
/// `sphere` or `cylinder` (around y), `planar` (onto x/y).
pub fn project_uvs(m: &mut Mesh, kind: &str) -> Result<(), String> {
    let (lo, hi) = m.bounds();
    let c = scale(add(lo, hi), 0.5);
    let size = [(hi[0] - lo[0]).max(1e-6), (hi[1] - lo[1]).max(1e-6), (hi[2] - lo[2]).max(1e-6)];
    let n = m.positions.len();
    if m.normals.len() != n {
        smooth_normals_in_place(m);
    }
    for v in 0..n {
        let p = sub(m.positions[v], c);
        m.uvs[v] = match kind {
            "box" | "cube" => {
                let a = m.normals[v].map(f32::abs);
                if a[0] >= a[1] && a[0] >= a[2] {
                    [p[2] / size[2] + 0.5, p[1] / size[1] + 0.5]
                } else if a[1] >= a[2] {
                    [p[0] / size[0] + 0.5, p[2] / size[2] + 0.5]
                } else {
                    [p[0] / size[0] + 0.5, p[1] / size[1] + 0.5]
                }
            }
            "sphere" | "spherical" => {
                let d = normalise(p);
                [0.5 + d[2].atan2(d[0]) / std::f32::consts::TAU, 0.5 + d[1].clamp(-1.0, 1.0).asin() / std::f32::consts::PI]
            }
            "cylinder" | "cylindrical" => [0.5 + p[2].atan2(p[0]) / std::f32::consts::TAU, p[1] / size[1] + 0.5],
            "planar" | "plane" | "front" => [p[0] / size[0] + 0.5, p[1] / size[1] + 0.5],
            other => return Err(format!("uv projection \"{other}\": box, sphere, cylinder or planar")),
        };
    }
    Ok(())
}

/// Removes triangles in `region` (and vertices no longer used).
pub fn delete(m: &mut Mesh, region: &Region) -> usize {
    let gone: HashSet<usize> = region.faces(m).into_iter().collect();
    let mats = m.tri_materials();
    let mut keep_mats = Vec::new();
    let mut idx = Vec::new();
    for t in 0..m.triangles() {
        if !gone.contains(&t) {
            idx.extend_from_slice(&m.indices[3 * t..3 * t + 3]);
            keep_mats.push(mats[t]);
        }
    }
    m.indices = idx;
    m.regroup(&keep_mats);
    compact(m);
    gone.len()
}

/// Drops vertices no triangle uses.
pub fn compact(m: &mut Mesh) {
    let mut map = vec![u32::MAX; m.positions.len()];
    let mut out = Mesh { materials: m.materials.clone(), groups: m.groups.clone(), ..Default::default() };
    for i in &mut m.indices {
        let v = *i as usize;
        if map[v] == u32::MAX {
            map[v] = out.positions.len() as u32;
            out.positions.push(m.positions[v]);
            out.normals.push(m.normals.get(v).copied().unwrap_or([0.0; 3]));
            out.uvs.push(m.uvs.get(v).copied().unwrap_or([0.0; 2]));
        }
        *i = map[v];
    }
    out.indices = std::mem::take(&mut m.indices);
    *m = out;
}

/// Gives the faces in `region` material `index` (made from `material` when
/// given), so one model can have skin, lips and eyes.
pub fn assign_material(m: &mut Mesh, region: &Region, material: Option<crate::Material>, index: Option<usize>) -> usize {
    let k = match (material, index) {
        (Some(mat), _) => {
            m.materials.push(mat);
            m.materials.len() - 1
        }
        (None, Some(i)) => i.min(m.materials.len().saturating_sub(1)),
        (None, None) => return 0,
    };
    let faces = region.faces(m);
    let mut mats = m.tri_materials();
    for &t in &faces {
        mats[t] = k as u32;
    }
    m.regroup(&mats);
    faces.len()
}

/// Groups that cover nothing are dropped (after deletions).
pub fn tidy_groups(m: &mut Mesh) {
    m.groups.retain(|g: &Group| g.count > 0);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prim;

    #[test]
    fn subdivision_rounds_a_cube_and_keeps_seams() {
        let mut m = prim::cube([100.0; 3]);
        let n0 = m.triangles();
        subdivide(&mut m, 2);
        assert_eq!(m.triangles(), n0 * 16);
        // Corners pulled in: the cube is rounder.
        let (lo, hi) = m.bounds();
        assert!(hi[0] - lo[0] < 100.0 && hi[0] - lo[0] > 60.0, "{lo:?} {hi:?}");
        assert_eq!(m.uvs.len(), m.positions.len());
    }

    #[test]
    fn sculpt_inflate_and_symmetry() {
        let mut m = prim::ico_sphere(100.0, 4);
        let before = m.bounds();
        sculpt(&mut m, Brush::Inflate, [100.0, 0.0, 0.0], 60.0, 20.0, true);
        let after = m.bounds();
        assert!(after.1[0] > before.1[0] + 15.0, "pushed out on +x");
        assert!(after.0[0] < before.0[0] - 15.0, "and mirrored on -x");
        assert!((after.1[1] - before.1[1]).abs() < 1.0, "the top untouched");
    }

    #[test]
    fn smoothing_does_not_shrink_much_and_symmetrize_mirrors() {
        let mut m = prim::ico_sphere(100.0, 3);
        displace(&mut m, &Region::All, 10.0, 30.0, 3, 7);
        smooth(&mut m, &Region::All, 10, 1.0);
        let (lo, hi) = m.bounds();
        assert!(hi[0] - lo[0] > 180.0, "Taubin keeps the size: {}", hi[0] - lo[0]);
        sculpt(&mut m, Brush::Inflate, [90.0, 0.0, 0.0], 40.0, 30.0, false);
        symmetrize(&mut m, 1.0);
        let (lo, hi) = m.bounds();
        assert!((hi[0] + lo[0]).abs() < 0.5, "symmetric: {lo:?} {hi:?}");
    }

    #[test]
    fn extrude_adds_walls() {
        let mut m = prim::cube([100.0; 3]);
        let n = extrude(&mut m, &Region::Facing { direction: [0.0, -1.0, 0.0], degrees: 10.0 }, 50.0);
        assert_eq!(n, 2);
        let (lo, _) = m.bounds();
        assert!((lo[1] + 100.0).abs() < 0.01, "the top went up 50: {lo:?}");
        assert_eq!(m.triangles(), 12 + 8);
    }
}

/// Particles as geometry: a copy of `shape` (scaled so its largest side is
/// each point's size) at every point, coloured by the point (similar
/// colours share a material; hot points glow).
pub fn instance_points(shape: &Mesh, points: &[crate::cache::Point]) -> Mesh {
    let (lo, hi) = shape.bounds();
    let c = scale(add(lo, hi), 0.5);
    let big = (hi[0] - lo[0]).max(hi[1] - lo[1]).max(hi[2] - lo[2]).max(1e-6);
    let base_mat = shape.materials.first().cloned().unwrap_or_default();
    let mut out = Mesh::default();
    let mut buckets: HashMap<[u8; 5], u32> = HashMap::new();
    let mut tri_mats: Vec<u32> = Vec::new();
    for p in points {
        if p.size <= 0.0 || p.rgba[3] == 0 {
            continue;
        }
        let key = [p.rgba[0] >> 4, p.rgba[1] >> 4, p.rgba[2] >> 4, p.rgba[3] >> 5, (p.heat.clamp(0.0, 1.0) * 6.0).round() as u8];
        let mat = *buckets.entry(key).or_insert_with(|| {
            let lin = |v: u8| crate::srgb_to_linear((v & 0xf0) | 0x08);
            let heat = key[4] as f32 / 6.0;
            // Hot: from deep red through orange to yellow-white.
            let glow = [1.0, (heat * 1.4 - 0.2).clamp(0.05, 1.0), (heat * 2.0 - 1.2).clamp(0.0, 1.0)];
            out.materials.push(crate::Material {
                name: "Particle".into(),
                base_colour: [lin(p.rgba[0]), lin(p.rgba[1]), lin(p.rgba[2]), ((key[3] as f32 + 0.5) / 8.0).min(1.0)],
                emissive: scale(glow, heat * heat * 6.0),
                ..base_mat.clone()
            });
            (out.materials.len() - 1) as u32
        });
        let k = p.size / big;
        let base = out.positions.len() as u32;
        out.positions.extend(shape.positions.iter().map(|v| add(scale(sub(*v, c), k), p.pos)));
        out.normals.extend_from_slice(&shape.normals);
        out.uvs.extend_from_slice(&shape.uvs);
        out.indices.extend(shape.indices.iter().map(|i| i + base));
        tri_mats.extend(std::iter::repeat_n(mat, shape.triangles()));
    }
    if out.materials.is_empty() {
        out.materials.push(base_mat);
    }
    out.regroup(&tri_mats);
    out
}
