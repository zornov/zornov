// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The wardrobe: clothing, hats, shoes, gloves, glasses, scarves, belts,
//! bags and armour made to fit a body, whoever it is.
//!
//! A garment is a real mesh fitted to the body it is for, not a picture of
//! one:
//!
//! * **Shells** (shirts, coats, trousers, gloves, shoes, plate armour) are
//!   cut from the body's own surface where the garment covers it, lifted off
//!   it by the garment's ease and given thickness, so they take the shape
//!   of whatever build, age or style the body has and never sink into it.
//!   Where they end (a hem, a cuff, a collar line) is a cut across a bone,
//!   so it is straight and in the right place for a child and a giant alike.
//! * **Lofts** (skirts, coat tails, capes, hats, scarves, belts) are surfaces
//!   built round the body from its measured size.
//! * **Skin**: a shell or loft takes the skin weights of the body under it
//!   (cloth follows the body, so it never needs a rig of its own); hard
//!   things (hats, glasses, armour plates, buckles) are bound whole to one
//!   bone and do not warp.
//!
//! Everything is found from the rig's bones (by role, so `UpperArm.l` and
//! `upper_arm.L` are the same) and the body's measurements; nothing is
//! tuned to one model. A body without the bones a garment needs is told
//! which are missing.

use crate::bvh::Bvh;
use crate::fabric::{self, Pattern};
use crate::loft;
use crate::rig::{PointGrid, Rig};
use crate::{Material, Mesh, V3, add, cross, dot, length, lerp, normalise, scale, sub};

/// What can be worn, for the error that lists it.
pub const KINDS: &str = "shirt (tee, blouse, sweater, hoodie, tank, vest), jacket (blazer, bomber, leather, denim), coat (trench, overcoat, long, peacoat, parka), trousers (jeans, slacks, shorts, leggings), skirt (mini, midi, long, pleated), dress (sundress, gown), cape (cloak), hat (fedora, cap, beanie, helmet, top hat, cowboy, beret, bowler, hood, crown), shoes (sneakers, boots, dress, sandals, heels), gloves (mittens, gauntlets), glasses (round, square, aviator, sunglasses), scarf, belt, bag (backpack, satchel), armour (breastplate, pauldrons, bracers, greaves, gauntlets, helmet), hair (short, buzz, crew, bob, long, wavy, curly, afro, ponytail, bun), beard (goatee, stubble), moustache, brows";

/// How a worn thing is skinned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Skin {
    /// The kind's usual way.
    Auto,
    /// The weights of the body under it: it bends with the body.
    Smooth,
    /// Whole pieces rigid on one bone each.
    Rigid,
    /// The body's weights sharpened so it hardly bends (plates).
    Stiff,
}

/// A thing to wear.
#[derive(Clone, Debug)]
pub struct Spec {
    pub kind: String,
    pub style: String,
    pub colour: Option<[u8; 3]>,
    /// Trim, lining, the second colour of a pattern.
    pub colour2: Option<[u8; 3]>,
    pub material: Option<String>,
    pub pattern: Pattern,
    /// 0.5 close .. 1 usual .. 2 loose.
    pub fit: f32,
    /// A length word (`short`, `hip`, `thigh`, `knee`, `calf`, `ankle`, `floor`,
    /// or sleeve `sleeveless`, `short`, `elbow`, `long`) or a fraction of the
    /// way down.
    pub length: Option<String>,
    pub sleeves: Option<String>,
    pub scale: f32,
    pub open: Option<bool>,
    pub skin: Skin,
    pub name: Option<String>,
    /// More triangles (2) or fewer (0.5).
    pub detail: f32,
    /// Trim details (collar, buttons, belt, cuffs) on or off.
    pub trim: bool,
    /// Which side (`L`, `R`) for a single glove or shoe.
    pub side: Option<char>,
    /// Make the hanging part (coat tails, skirt, cape) a separate piece
    /// for cloth simulation instead of skinning it to the body.
    pub loose: bool,
}

impl Default for Spec {
    fn default() -> Spec {
        Spec { kind: String::new(), style: String::new(), colour: None, colour2: None, material: None, pattern: Pattern::None, fit: 1.0, length: None, sleeves: None, scale: 1.0, open: None, skin: Skin::Auto, name: None, detail: 1.0, trim: true, side: None, loose: false }
    }
}

/// A made garment, ready to join the body.
#[derive(Clone, Debug)]
pub struct Garment {
    pub name: String,
    pub kind: String,
    /// Positions, normals, uvs, triangles, its own materials.
    pub mesh: Mesh,
    pub joints: Vec<[u16; 4]>,
    pub weights: Vec<[f32; 4]>,
    /// Notes for the report.
    pub notes: Vec<String>,
    /// A hanging part left out for simulation: its mesh, the points along its
    /// top that stay on the body.
    pub loose: Option<Mesh>,
    /// How far (px) the skin under it is drawn in when it is put on, so a
    /// tight garment never shows skin through it in a pose; 0 leaves the
    /// body as it is.
    pub tuck: f32,
}

impl Garment {
    pub fn new(name: &str, kind: &str) -> Garment {
        Garment { name: name.into(), kind: kind.into(), mesh: Mesh::default(), joints: Vec::new(), weights: Vec::new(), notes: Vec::new(), loose: None, tuck: 0.0 }
    }

    pub fn material(&mut self, m: Material) -> u32 {
        self.mesh.materials.push(m);
        (self.mesh.materials.len() - 1) as u32
    }

    /// Adds a piece with its skin weights and a material.
    pub fn push(&mut self, piece: &Mesh, joints: Vec<[u16; 4]>, weights: Vec<[f32; 4]>, mat: u32) {
        let mut mats = self.mesh.tri_materials();
        mats.extend(std::iter::repeat_n(mat, piece.triangles()));
        let base = self.mesh.positions.len() as u32;
        self.mesh.positions.extend_from_slice(&piece.positions);
        self.mesh.normals.extend_from_slice(&piece.normals);
        self.mesh.uvs.extend_from_slice(&piece.uvs);
        self.mesh.indices.extend(piece.indices.iter().map(|i| i + base));
        self.joints.extend(joints);
        self.weights.extend(weights);
        self.mesh.regroup(&mats);
    }
}

// ------------------------------------------------------------------ fit

/// A cut across the body: keep what is on the `dir` side's back (where
/// `dot(p - point, dir) <= 0`), for vertices nearest the given bones (all
/// when empty).
#[derive(Clone, Debug)]
struct Cut {
    point: V3,
    dir: V3,
    bones: Vec<usize>,
}

struct ShellSpec {
    bones: Vec<usize>,
    cuts: Vec<Cut>,
    ease: f32,
    thick: f32,
    /// Fold depth against the ease, and their scale (px).
    folds: f32,
    fold_size: f32,
    budget: usize,
}

/// The body a garment is made for: its mesh, rig and measurements.
struct Fit<'a> {
    mesh: &'a Mesh,
    rig: &'a Rig,
    h: f32,
    /// The deforming bone nearest each vertex.
    near: Vec<u32>,
    /// The bone carrying each vertex most.
    main: Vec<u32>,
    bvh: Bvh,
    /// The body without arms and hands (for measuring round the trunk and legs).
    trunk: Bvh,
    grid: PointGrid,
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Where `p` (on the triangle's plane) falls in triangle `t`, as weights of
/// its corners that sum to 1.
fn barycentric(p: V3, t: [V3; 3]) -> [f32; 3] {
    let (e0, e1, e2) = (sub(t[1], t[0]), sub(t[2], t[0]), sub(p, t[0]));
    let (d00, d01, d11, d20, d21) = (dot(e0, e0), dot(e0, e1), dot(e1, e1), dot(e2, e0), dot(e2, e1));
    let den = d00 * d11 - d01 * d01;
    if den.abs() < 1e-12 {
        return [1.0, 0.0, 0.0];
    }
    let v = ((d11 * d20 - d01 * d21) / den).clamp(0.0, 1.0);
    let w = ((d00 * d21 - d01 * d20) / den).clamp(0.0, 1.0 - v);
    [1.0 - v - w, v, w]
}

impl<'a> Fit<'a> {
    fn new(mesh: &'a Mesh) -> Result<Fit<'a>, String> {
        let rig = mesh.rig.as_deref().ok_or("this model has no rig: give it one first (for a person, add it with model shape \"character\"; for another model, rig it with a template)")?;
        if !rig.is_skinned() || !mesh.rig_in_step() {
            return Err("the model's rig has no skin that fits its surface yet: bind it with {\"op\": \"rig\", \"clip\": id, \"skinning\": \"smooth\"}".into());
        }
        let (lo, hi) = mesh.bounds();
        let h = (hi[1] - lo[1]).max(1.0);
        // The bones that carry the body's surface: not eyes or grips.
        let carry: Vec<usize> = (0..rig.bones.len()).filter(|&i| rig.bones[i].deform && !rig.bones[i].name.starts_with("eye.")).collect();
        // The bone nearest each vertex, and the bone that carries it most (by
        // its skin weights): a vertex belongs to a part of the body only when
        // both agree, so a jaw close to the shoulder bones is still the head's.
        let nearest: Vec<u32> = mesh
            .positions
            .iter()
            .map(|p| {
                carry
                    .iter()
                    .copied()
                    .min_by(|&a, &b| {
                        let d = |i: usize| length(sub(crate::autorig::closest_on_segment(rig.bones[i].head, rig.bones[i].tail, *p), *p));
                        d(a).total_cmp(&d(b))
                    })
                    .unwrap_or(0) as u32
            })
            .collect();
        let main: Vec<u32> = (0..mesh.positions.len()).map(|v| crate::rigfit::main_bone(rig, v).filter(|b| carry.contains(b)).map_or(nearest[v], |b| b as u32)).collect();
        let near = nearest.clone();
        let arm = |b: u32| {
            let n = rig.bones[b as usize].name.as_str();
            ["shoulder", "upper_arm", "forearm", "hand", "thumb", "index", "middle", "ring", "pinky", "grip"].iter().any(|p| n.starts_with(p))
        };
        let trunk = Bvh::new((0..mesh.triangles()).filter(|&t| mesh.tri(t).iter().filter(|&&v| !arm(near[v])).count() >= 2).map(|t| mesh.tri(t).map(|v| mesh.positions[v])).collect());
        Ok(Fit { mesh, rig, h, near, main, bvh: Bvh::of_mesh(mesh), trunk, grid: PointGrid::new(&mesh.positions) })
    }

    fn bone(&self, name: &str) -> Option<usize> {
        self.rig.bone(name)
    }

    /// A bone that must be there.
    fn need(&self, name: &str) -> Result<usize, String> {
        self.bone(name).ok_or_else(|| format!("this garment needs the bone \"{name}\" and the rig has none like it ({}); wear it on a humanoid, or choose a kind that suits this body", self.rig.bones.iter().map(|b| b.name.as_str()).collect::<Vec<_>>().join(", ")))
    }

    /// The bones named `base.L` and `base.R` (those that exist).
    fn both(&self, base: &str) -> Vec<usize> {
        ["L", "R"].iter().filter_map(|s| self.bone(&format!("{base}.{s}"))).collect()
    }

    /// Bones by role and every finger under them.
    fn hand_bones(&self, side: &str) -> Vec<usize> {
        let mut v: Vec<usize> = self.bone(&format!("hand.{side}")).into_iter().collect();
        v.extend((0..self.rig.bones.len()).filter(|&i| {
            let n = &self.rig.bones[i].name;
            n.ends_with(&format!(".{side}.0")) || n.ends_with(&format!(".{side}.1")) || n.ends_with(&format!(".{side}.2"))
        }));
        v
    }

    fn head_of(&self, b: usize) -> V3 {
        self.rig.bones[b].head
    }
    fn tail_of(&self, b: usize) -> V3 {
        self.rig.bones[b].tail
    }
    /// A point a fraction `t` along bone `b`.
    fn along(&self, b: usize, t: f32) -> V3 {
        lerp(self.head_of(b), self.tail_of(b), t)
    }
    fn axis(&self, b: usize) -> V3 {
        normalise(sub(self.tail_of(b), self.head_of(b)))
    }

    /// The middle of the body's slice at height `y` among vertices near these bones.
    fn centre_at(&self, y: f32, bones: &[usize]) -> V3 {
        let band = self.h * 0.012;
        let mut acc = [0.0f32; 3];
        let mut n = 0.0;
        for (i, p) in self.mesh.positions.iter().enumerate() {
            if (p[1] - y).abs() < band && bones.contains(&(self.near[i] as usize)) {
                acc = add(acc, *p);
                n += 1.0;
            }
        }
        if n < 4.0 { [self.rig.bones.first().map_or(0.0, |b| b.head[0]), y, 0.0] } else { scale(acc, 1.0 / n) }
    }

    /// Skin weights of the body at the point of its surface nearest each
    /// point: the three corners of that triangle blended by where the point
    /// falls in it, so cloth bends exactly as the skin under it does (the
    /// nearest vertex alone is used when no surface is close).
    fn weights_for(&self, positions: &[V3]) -> (Vec<[u16; 4]>, Vec<[f32; 4]>) {
        let rig = self.rig;
        let mut j = Vec::with_capacity(positions.len());
        let mut w = Vec::with_capacity(positions.len());
        for p in positions {
            if let Some((q, tri, _)) = self.bvh.nearest(*p, self.h * 0.15) {
                let c = self.mesh.tri(tri as usize);
                let bary = barycentric(q, [self.mesh.positions[c[0]], self.mesh.positions[c[1]], self.mesh.positions[c[2]]]);
                let mut acc: Vec<(u16, f32)> = Vec::with_capacity(12);
                for (k, &v) in c.iter().enumerate() {
                    for s in 0..4 {
                        let x = rig.weights[v][s] * bary[k];
                        if x <= 0.0 {
                            continue;
                        }
                        match acc.iter_mut().find(|e| e.0 == rig.joints[v][s]) {
                            Some(e) => e.1 += x,
                            None => acc.push((rig.joints[v][s], x)),
                        }
                    }
                }
                acc.sort_by(|a, b| b.1.total_cmp(&a.1));
                acc.truncate(4);
                let total: f32 = acc.iter().map(|e| e.1).sum();
                if total > 1e-6 {
                    let (mut jj, mut ww) = ([0u16; 4], [0.0f32; 4]);
                    for (k, (b, x)) in acc.iter().enumerate() {
                        jj[k] = *b;
                        ww[k] = x / total;
                    }
                    j.push(jj);
                    w.push(ww);
                    continue;
                }
            }
            let i = self.grid.nearest(&self.mesh.positions, *p);
            j.push(rig.joints[i]);
            w.push(rig.weights[i]);
        }
        (j, w)
    }

    /// A bone of an arm, a hand or a finger.
    fn on_arm(&self, bone: usize) -> bool {
        self.rig.bones.get(bone).is_some_and(|b| {
            let n = b.name.to_ascii_lowercase();
            n.contains("arm") || ["shoulder", "hand", "grip", "thumb", "index", "middle", "ring", "pinky"].iter().any(|k| n.starts_with(k))
        })
    }

    fn rigid_weights(&self, n: usize, bone: usize) -> (Vec<[u16; 4]>, Vec<[f32; 4]>) {
        (vec![[bone as u16, 0, 0, 0]; n], vec![[1.0, 0.0, 0.0, 0.0]; n])
    }

    /// Weights from the body, sharpened so one bone takes nearly all.
    fn stiff(w: &mut [[f32; 4]]) {
        for x in w.iter_mut() {
            let mut s = 0.0;
            for v in x.iter_mut() {
                *v = v.powi(4);
                s += *v;
            }
            if s > 0.0 {
                for v in x.iter_mut() {
                    *v /= s;
                }
            }
        }
    }

    // ----------------------------------------------------------- shells

    /// A shell cut from the body surface (see the module notes).
    fn shell(&self, s: &ShellSpec) -> Result<(Mesh, Vec<[u16; 4]>, Vec<[f32; 4]>), String> {
        self.shell_with(s, None)
    }

    /// [`Fit::shell`] with a round hole through it round a vertical axis
    /// (`(x, z, radius, lowest y it reaches)`: a neckline).
    fn shell_with(&self, s: &ShellSpec, hole: Option<(f32, f32, f32, f32)>) -> Result<(Mesh, Vec<[u16; 4]>, Vec<[f32; 4]>), String> {
        let m = self.mesh;
        let head_ok = self.bone("head").is_some_and(|h| s.bones.contains(&h));
        let chin = self.bone("head").map_or(f32::NEG_INFINITY, |h| self.head_of(h)[1] - 0.01 * self.h);
        let member = |v: usize| s.bones.contains(&(self.near[v] as usize)) && s.bones.contains(&(self.main[v] as usize)) && (head_ok || m.positions[v][1] >= chin);
        let passes = |p: V3, v: usize, c: &Cut, slack: f32| -> bool { (!c.bones.is_empty() && !c.bones.contains(&(self.near[v] as usize))) || dot(sub(p, c.point), c.dir) <= slack };
        let mut remap = vec![u32::MAX; m.positions.len()];
        let mut out = Mesh { materials: vec![Material::default()], ..Default::default() };
        let mut origin: Vec<usize> = Vec::new();
        for t in 0..m.triangles() {
            let c = m.tri(t);
            if c.iter().filter(|&&v| member(v)).count() < 2 {
                continue;
            }
            let centre = m.centroid(t);
            if let Some((hx, hz, hr, hy)) = hole
                && centre[1] < hy
                && ((centre[0] - hx).powi(2) + (centre[2] - hz).powi(2)).sqrt() < hr
            {
                continue;
            }
            if !s.cuts.iter().all(|cut| {
                let v = *c.iter().find(|&&v| cut.bones.is_empty() || cut.bones.contains(&(self.near[v] as usize))).unwrap_or(&c[0]);
                passes(centre, v, cut, 0.0)
            }) {
                continue;
            }
            for &v in &c {
                if remap[v] == u32::MAX {
                    remap[v] = out.positions.len() as u32;
                    out.positions.push(m.positions[v]);
                    out.normals.push(m.normals.get(v).copied().unwrap_or([0.0; 3]));
                    origin.push(v);
                }
                out.indices.push(remap[v]);
            }
        }
        if out.triangles() < 8 {
            return Err("nothing of the body is there to cover (the rig's bones and the surface do not meet where this garment goes)".into());
        }
        out.uvs = vec![[0.0; 2]; out.positions.len()];
        // Straight edges: what hangs over a cut is brought back to it.
        for (k, &v) in origin.iter().enumerate() {
            for cut in &s.cuts {
                if !cut.bones.is_empty() && !cut.bones.contains(&(self.near[v] as usize)) {
                    continue;
                }
                let d = dot(sub(out.positions[k], cut.point), cut.dir);
                if d > 0.0 {
                    out.positions[k] = sub(out.positions[k], scale(cut.dir, d));
                }
            }
        }
        // Fewer triangles before lifting off (cuts are held in place).
        if out.triangles() > s.budget {
            crate::decimate::decimate(&mut out, s.budget);
        }
        let n_orig = out.positions.len();
        let body_n: Vec<V3> = (0..n_orig).map(|k| self.body_normal(out.positions[k])).collect();
        // The cut edge: its vertices lifted a little less, so a hem does not flare.
        let boundary = open_vertices(&out);
        let ease = s.ease;
        let start = out.positions.clone();
        for k in 0..n_orig {
            let p = start[k];
            let fold = if s.folds > 0.0 {
                let f = fold_noise(p, s.fold_size);
                f * s.folds * ease
            } else {
                0.0
            };
            let edge = if boundary[k] { 0.8 } else { 1.0 };
            out.positions[k] = add(p, scale(body_n[k], (ease * edge + fold).max(ease * 0.4)));
        }
        // Soften, then keep a clear gap from every part of the body.
        for _ in 0..3 {
            laplacian(&mut out, &boundary, 0.35);
            smooth_edges(&mut out, &boundary, 0.5);
            self.keep_clear(&mut out, ease * 0.92);
        }
        crate::ops::smooth_normals_in_place(&mut out);
        let (mut j, mut w) = self.weights_for(&out.positions);
        smooth_weights(&out, &mut j, &mut w, 5);
        // Thickness: an inward flange along every open edge.
        let (mut out, j, w) = add_rim(out, j, w, s.thick);
        out.uvs = vec![[0.0; 2]; out.positions.len()];
        Ok((out, j, w))
    }

    /// The body's surface normal at a point near it.
    fn body_normal(&self, p: V3) -> V3 {
        match self.bvh.nearest(p, self.h) {
            Some((q, tri, d)) => {
                let t = tri as usize;
                let n = self.mesh.face_normal(t);
                let away = normalise(sub(p, q));
                let _ = d;
                // The vertex normal blended, so lifted shells stay smooth.
                let i = self.grid.nearest(&self.mesh.positions, p);
                let vn = self.mesh.normals.get(i).copied().unwrap_or(n);
                if dot(vn, vn) > 0.5 { vn } else if dot(away, n) > 0.0 { n } else { away }
            }
            None => [0.0, -1.0, 0.0],
        }
    }

    /// Pushes vertices out until they are at least `gap` from the body.
    fn keep_clear(&self, m: &mut Mesh, gap: f32) {
        for p in &mut m.positions {
            if let Some((q, tri, d)) = self.bvh.nearest(*p, gap * 3.0)
                && d < gap
            {
                let n = self.mesh.face_normal(tri as usize);
                let away = sub(*p, q);
                // Which side of the surface: the side the face looks to is outside.
                let outside = dot(away, n) >= 0.0 || d < 1e-4;
                let dir = if d > 1e-4 && outside { normalise(away) } else { n };
                *p = add(q, scale(dir, gap));
            }
        }
    }

    /// Texture coordinates tiled by `tile` px.
    fn uvs(&self, m: &mut Mesh, tile: f32) {
        let k = 1.0 / tile.max(0.5);
        for (i, p) in m.positions.iter().enumerate() {
            let n = m.normals.get(i).copied().unwrap_or([0.0, 1.0, 0.0]);
            let a = n.map(f32::abs);
            // Project along the face's main axis.
            let uv = if a[0] >= a[1] && a[0] >= a[2] { [p[2], p[1]] } else if a[1] >= a[2] { [p[0], p[2]] } else { [p[0], p[1]] };
            m.uvs[i] = [uv[0] * k, uv[1] * k];
        }
    }
}

/// Folds in cloth: noise stretched down the garment.
fn fold_noise(p: V3, size: f32) -> f32 {
    let s = size.max(1.0);
    let (x, y, z) = (p[0] / s, p[1] / (s * 3.0), p[2] / s);
    let h = |a: f32, b: f32, c: f32| {
        let mut v = (a * 12.9898 + b * 78.233 + c * 37.719).sin() * 43758.545;
        v -= v.floor();
        v
    };
    let (xi, yi, zi) = (x.floor(), y.floor(), z.floor());
    let (fx, fy, fz) = (x - xi, y - yi, z - zi);
    let (sx, sy, sz) = (fx * fx * (3.0 - 2.0 * fx), fy * fy * (3.0 - 2.0 * fy), fz * fz * (3.0 - 2.0 * fz));
    let l = |a: f32, b: f32, t: f32| a + (b - a) * t;
    let c = |dx: f32, dy: f32, dz: f32| h(xi + dx, yi + dy, zi + dz);
    let v = l(
        l(l(c(0.0, 0.0, 0.0), c(1.0, 0.0, 0.0), sx), l(c(0.0, 1.0, 0.0), c(1.0, 1.0, 0.0), sx), sy),
        l(l(c(0.0, 0.0, 1.0), c(1.0, 0.0, 1.0), sx), l(c(0.0, 1.0, 1.0), c(1.0, 1.0, 1.0), sx), sy),
        sz,
    );
    v - 0.5
}

/// Which vertices lie on an open edge (an edge of one triangle only).
fn open_vertices(m: &Mesh) -> Vec<bool> {
    let (ids, _) = m.position_ids();
    let mut count: std::collections::HashMap<(u32, u32), u32> = Default::default();
    for t in 0..m.triangles() {
        let c = m.tri(t).map(|v| ids[v]);
        for (a, b) in [(c[0], c[1]), (c[1], c[2]), (c[2], c[0])] {
            *count.entry((a.min(b), a.max(b))).or_default() += 1;
        }
    }
    let open: std::collections::HashSet<u32> = count.iter().filter(|(_, n)| **n == 1).flat_map(|(e, _)| [e.0, e.1]).collect();
    ids.iter().map(|i| open.contains(i)).collect()
}

/// Smooths vertices towards their neighbours; open-edge vertices only
/// slide along the edge (they are held).
fn laplacian(m: &mut Mesh, hold: &[bool], k: f32) {
    let (ids, unique) = m.position_ids();
    let mut sum = vec![[0.0f32; 3]; unique.len()];
    let mut cnt = vec![0.0f32; unique.len()];
    let mut seen: std::collections::HashSet<(u32, u32)> = Default::default();
    for t in 0..m.triangles() {
        let c = m.tri(t);
        for (a, b) in [(c[0], c[1]), (c[1], c[2]), (c[2], c[0])] {
            let (ia, ib) = (ids[a], ids[b]);
            if seen.insert((ia.min(ib), ia.max(ib))) {
                sum[ia as usize] = add(sum[ia as usize], m.positions[b]);
                cnt[ia as usize] += 1.0;
                sum[ib as usize] = add(sum[ib as usize], m.positions[a]);
                cnt[ib as usize] += 1.0;
            }
        }
    }
    let old = m.positions.clone();
    for (v, p) in m.positions.iter_mut().enumerate() {
        if hold[v] {
            continue;
        }
        let i = ids[v] as usize;
        if cnt[i] > 0.0 {
            *p = lerp(old[v], scale(sum[i], 1.0 / cnt[i]), k);
        }
    }
}

/// Evens out an open edge: each edge vertex moves towards the middle of its
/// two neighbours along the edge, so a cut across triangles is a clean line
/// and not a saw.
fn smooth_edges(m: &mut Mesh, open: &[bool], k: f32) {
    let (ids, unique) = m.position_ids();
    let mut count: std::collections::HashMap<(u32, u32), u32> = Default::default();
    for t in 0..m.triangles() {
        let c = m.tri(t).map(|v| ids[v]);
        for (a, b) in [(c[0], c[1]), (c[1], c[2]), (c[2], c[0])] {
            *count.entry((a.min(b), a.max(b))).or_default() += 1;
        }
    }
    let mut nb: Vec<Vec<u32>> = vec![Vec::new(); unique.len()];
    for (&(a, b), &n) in &count {
        if n == 1 {
            nb[a as usize].push(b);
            nb[b as usize].push(a);
        }
    }
    let mut pos: Vec<V3> = unique.clone();
    for _ in 0..4 {
        let old = pos.clone();
        for (i, p) in pos.iter_mut().enumerate() {
            if nb[i].len() == 2 {
                let mid = scale(add(old[nb[i][0] as usize], old[nb[i][1] as usize]), 0.5);
                *p = lerp(old[i], mid, k);
            }
        }
    }
    for (v, p) in m.positions.iter_mut().enumerate() {
        if open[v] {
            *p = pos[ids[v] as usize];
        }
    }
}

/// Softens skin weights over a surface: each vertex takes half its own and
/// half its neighbours' average, a few times, so where the body's weights
/// change quickly (an armpit, a crotch) the cloth across the gap does not
/// get pulled apart.
fn smooth_weights(m: &Mesh, j: &mut [[u16; 4]], w: &mut [[f32; 4]], iters: usize) {
    let (ids, unique) = m.position_ids();
    let mut nb: Vec<Vec<u32>> = vec![Vec::new(); unique.len()];
    for t in 0..m.triangles() {
        let c = m.tri(t).map(|v| ids[v]);
        for (a, b) in [(c[0], c[1]), (c[1], c[2]), (c[2], c[0])] {
            if a != b {
                if !nb[a as usize].contains(&b) {
                    nb[a as usize].push(b);
                }
                if !nb[b as usize].contains(&a) {
                    nb[b as usize].push(a);
                }
            }
        }
    }
    // Per welded position: the weights as (bone, weight) lists.
    let mut cur: Vec<Vec<(u16, f32)>> = vec![Vec::new(); unique.len()];
    for v in 0..m.positions.len() {
        let slot = &mut cur[ids[v] as usize];
        if slot.is_empty() {
            *slot = (0..4).filter(|&k| w[v][k] > 0.0).map(|k| (j[v][k], w[v][k])).collect();
        }
    }
    for _ in 0..iters {
        let prev = cur.clone();
        for (i, slot) in cur.iter_mut().enumerate() {
            if nb[i].is_empty() {
                continue;
            }
            let mut acc: Vec<(u16, f32)> = prev[i].iter().map(|&(b, x)| (b, x * 0.5)).collect();
            let k = 0.5 / nb[i].len() as f32;
            for &n in &nb[i] {
                for &(b, x) in &prev[n as usize] {
                    match acc.iter_mut().find(|e| e.0 == b) {
                        Some(e) => e.1 += x * k,
                        None => acc.push((b, x * k)),
                    }
                }
            }
            acc.sort_by(|a, b| b.1.total_cmp(&a.1));
            acc.truncate(4);
            let total: f32 = acc.iter().map(|e| e.1).sum();
            if total > 0.0 {
                for e in &mut acc {
                    e.1 /= total;
                }
            }
            *slot = acc;
        }
    }
    for v in 0..m.positions.len() {
        let slot = &cur[ids[v] as usize];
        let mut jj = [0u16; 4];
        let mut ww = [0.0f32; 4];
        for (k, &(b, x)) in slot.iter().take(4).enumerate() {
            jj[k] = b;
            ww[k] = x;
        }
        if ww.iter().sum::<f32>() <= 0.0 {
            ww[0] = 1.0;
        }
        j[v] = jj;
        w[v] = ww;
    }
}

/// Gives a sheet thickness: a second surface `thick` px inside it, facing
/// the other way, joined to it along every open edge, so the garment is a
/// closed solid (a hem, a cuff or a brim has a real edge and nothing shows
/// through from behind). The materials stay as they were.
fn add_rim(mut m: Mesh, mut j: Vec<[u16; 4]>, mut w: Vec<[f32; 4]>, thick: f32) -> (Mesh, Vec<[u16; 4]>, Vec<[f32; 4]>) {
    if thick <= 1e-4 || m.triangles() == 0 {
        return (m, j, w);
    }
    let (ids, _) = m.position_ids();
    let mut count: std::collections::HashMap<(u32, u32), u32> = Default::default();
    for t in 0..m.triangles() {
        let c = m.tri(t);
        for k in 0..3 {
            let (a, b) = (ids[c[k]], ids[c[(k + 1) % 3]]);
            *count.entry((a.min(b), a.max(b))).or_default() += 1;
        }
    }
    let n = m.positions.len();
    let tris = m.triangles();
    let mut mats = m.tri_materials();
    if m.normals.len() != n {
        m.normals.resize(n, [0.0; 3]);
        crate::ops::smooth_normals_in_place(&mut m);
    }
    if m.uvs.len() != n {
        m.uvs.resize(n, [0.0; 2]);
    }
    // The inner surface: every vertex again, moved in along its normal.
    for v in 0..n {
        let nv = m.normals[v];
        m.positions.push(sub(m.positions[v], scale(nv, thick)));
        m.normals.push(scale(nv, -1.0));
        m.uvs.push(m.uvs[v]);
        j.push(j[v]);
        w.push(w[v]);
    }
    let n32 = n as u32;
    for t in 0..tris {
        let c = m.tri(t);
        m.indices.extend_from_slice(&[c[0] as u32 + n32, c[2] as u32 + n32, c[1] as u32 + n32]);
        mats.push(mats[t]);
    }
    // The edge between them, along every open edge.
    for t in 0..tris {
        let c = m.tri(t);
        for k in 0..3 {
            let (a, b) = (c[k], c[(k + 1) % 3]);
            if ids[a] == ids[b] || count[&(ids[a].min(ids[b]), ids[a].max(ids[b]))] != 1 {
                continue;
            }
            let (a, b) = (a as u32, b as u32);
            m.indices.extend_from_slice(&[b, a, a + n32, b, a + n32, b + n32]);
            mats.push(mats[t]);
            mats.push(mats[t]);
        }
    }
    m.regroup(&mats);
    (m, j, w)
}

// ------------------------------------------------------------ recipes

fn lc(s: &str) -> String {
    s.trim().to_ascii_lowercase().replace([' ', '-'], "_")
}

/// The canonical kind of a word.
pub fn canonical_kind(word: &str) -> Option<&'static str> {
    Some(match lc(word).as_str() {
        "shirt" | "tee" | "t_shirt" | "tshirt" | "blouse" | "top" | "sweater" | "jumper" | "pullover" | "hoodie" | "tank" | "vest" | "singlet" | "polo" | "cardigan" | "sweatshirt" | "tunic" => "shirt",
        "jacket" | "blazer" | "bomber" | "windbreaker" | "suit_jacket" | "gilet" | "bolero" => "jacket",
        "coat" | "trench" | "trenchcoat" | "overcoat" | "raincoat" | "parka" | "peacoat" | "greatcoat" | "duster" | "robe" | "kimono" => "coat",
        "trousers" | "pants" | "jeans" | "slacks" | "shorts" | "leggings" | "chinos" | "breeches" | "bermuda" | "culottes" => "trousers",
        "skirt" | "kilt" | "tutu" => "skirt",
        "dress" | "gown" | "sundress" | "frock" | "jumpsuit" | "overalls" => "dress",
        "cape" | "cloak" | "mantle" | "poncho" | "shawl" => "cape",
        "hat" | "fedora" | "cap" | "beanie" | "helmet" | "top_hat" | "tophat" | "cowboy" | "beret" | "bowler" | "hood" | "crown" | "cowl" | "bonnet" | "turban" | "headband" | "visor" | "hardhat" | "helm" => "hat",
        "shoes" | "shoe" | "boots" | "boot" | "sneakers" | "sneaker" | "trainers" | "sandals" | "heels" | "loafers" | "slippers" | "footwear" => "shoes",
        "gloves" | "glove" | "mittens" | "gauntlets" | "gauntlet" | "mitts" => "gloves",
        "glasses" | "spectacles" | "sunglasses" | "goggles" | "shades" | "eyeglasses" | "monocle" => "glasses",
        "scarf" | "muffler" | "cravat" | "bandana" | "tie" | "necktie" | "bow_tie" | "bowtie" => "scarf",
        "belt" | "sash" | "cummerbund" | "harness" => "belt",
        "bag" | "backpack" | "satchel" | "rucksack" | "messenger_bag" | "purse" | "quiver" | "pack" => "bag",
        "armour" | "armor" | "breastplate" | "cuirass" | "pauldrons" | "pauldron" | "bracers" | "bracer" | "greaves" | "greave" | "plate" | "chainmail" | "mail" => "armour",
        _ => return None,
    })
}

/// Fits a garment (or, for armour, a set of plates) to the body.
pub fn fit(body: &Mesh, spec: &Spec) -> Result<Vec<Garment>, String> {
    let kind = canonical_kind(&spec.kind).ok_or_else(|| format!("kind \"{}\": {KINDS}", spec.kind))?;
    // A word like "fedora" or "trench" given as the kind is a style of it.
    let mut spec = spec.clone();
    if spec.style.is_empty() && lc(&spec.kind) != kind {
        spec.style = lc(&spec.kind);
    }
    let f = Fit::new(body)?;
    let name = spec.name.clone().unwrap_or_else(|| title(if spec.style.is_empty() { kind } else { &spec.style }));
    let mut g = Garment::new(&name, kind);
    match kind {
        "shirt" | "jacket" | "coat" => upper(&f, &spec, kind, &mut g)?,
        "trousers" => lower(&f, &spec, &mut g)?,
        "skirt" | "dress" => skirt_or_dress(&f, &spec, kind, &mut g)?,
        "cape" => cape(&f, &spec, &mut g)?,
        "shoes" => shoes(&f, &spec, &mut g)?,
        "gloves" => gloves(&f, &spec, &mut g)?,
        "hat" => hat(&f, &spec, &mut g)?,
        "glasses" => glasses(&f, &spec, &mut g)?,
        "scarf" => scarf(&f, &spec, &mut g)?,
        "belt" => belt(&f, &spec, &mut g)?,
        "bag" => bag(&f, &spec, &mut g)?,
        "armour" => return armour(&f, &spec),
        _ => return Err(format!("kind \"{kind}\": {KINDS}")),
    }
    if g.mesh.triangles() == 0 {
        return Err(format!("{kind} came out empty"));
    }
    finish(&f, &spec, &mut g);
    Ok(vec![g])
}

fn title(s: &str) -> String {
    let s = s.replace('_', " ");
    let mut c = s.chars();
    c.next().map_or(String::new(), |f| f.to_uppercase().collect::<String>() + c.as_str())
}

/// Makes a garment ready to join the body: every piece a closed solid
/// (small holes capped, open sheets given an inside), every vertex clear of
/// the skin, and how far a tight one draws the skin in under it.
fn finish(f: &Fit, spec: &Spec, g: &mut Garment) {
    crate::ops::smooth_normals_in_place(&mut g.mesh);
    if g.mesh.uvs.len() != g.mesh.positions.len() {
        g.mesh.uvs.resize(g.mesh.positions.len(), [0.0; 2]);
    }
    // Patterns laid on flat panels, not smeared where the projection turns.
    panel_uvs(g);
    // Buttons, buckles, the ends of a band: small holes are capped.
    crate::seal::close_holes(&mut g.mesh, 0.04 * f.h, |_| true);
    // A sheet still open (a brim, a cape) gets an inside of its own.
    if crate::seal::open_edges(&g.mesh) > 0 {
        let mesh = std::mem::take(&mut g.mesh);
        let (m, j, w) = add_rim(mesh, std::mem::take(&mut g.joints), std::mem::take(&mut g.weights), 0.0025 * f.h);
        g.mesh = m;
        g.joints = j;
        g.weights = w;
    }
    // Nothing under the skin: cloth that bends with the body is kept a
    // little off it everywhere (hard pieces ride where they were made).
    let bends = g.weights.iter().any(|w| w[1] > 0.01);
    if bends {
        f.keep_clear(&mut g.mesh, 0.0025 * f.h);
    }
    // Close-fitting cloth draws the skin under it in a little.
    let tight = spec.fit < 0.9 || matches!(lc(&spec.style).as_str(), "leggings" | "tights" | "tank" | "singlet" | "bodysuit" | "catsuit" | "wetsuit" | "gloves");
    if bends && matches!(g.kind.as_str(), "shirt" | "jacket" | "coat" | "trousers" | "dress" | "gloves" | "shoes" | "armour") {
        g.tuck = if tight { 0.004 * f.h } else { 0.0015 * f.h };
    }
}


// ------------------------------------------------------------ measuring

/// Landmarks of a standing figure, found from its bones (heights are y,
/// which grows downward).
#[allow(dead_code)]
struct Marks {
    crown: f32,
    chin: f32,
    neck: f32,
    shoulder: f32,
    waist: f32,
    crotch: f32,
    knee: f32,
    ankle: f32,
    sole: f32,
    cx: f32,
}

impl<'a> Fit<'a> {
    fn marks(&self) -> Result<Marks, String> {
        let hips = self.need("hips")?;
        let head = self.need("head")?;
        let neck = self.need("neck")?;
        let spine = self.need("spine")?;
        let shin = self.need("shin.L").or_else(|_| self.need("shin.R"))?;
        let (_, hi) = self.mesh.bounds();
        let shoulder = self.bone("upper_arm.L").map_or_else(|| self.head_of(neck)[1], |b| self.head_of(b)[1]);
        Ok(Marks {
            crown: self.tail_of(head)[1],
            chin: self.head_of(head)[1],
            neck: self.head_of(neck)[1],
            shoulder,
            waist: (self.head_of(spine)[1] + self.tail_of(spine)[1]) * 0.5,
            crotch: self.head_of(hips)[1],
            knee: self.head_of(shin)[1],
            ankle: self.tail_of(shin)[1],
            sole: hi[1],
            cx: self.head_of(hips)[0],
        })
    }

    /// How far the surface of `bvh` is from `centre` along `dir`, the
    /// outermost crossing.
    fn reach_in(bvh: &Bvh, centre: V3, dir: V3, far: f32) -> Option<f32> {
        let from = add(centre, scale(dir, far));
        bvh.ray(from, scale(dir, -1.0), 0.0, far * 2.0).map(|h| far - h.t)
    }

    /// The radius of the body's core round a vertical axis at height `y`: the
    /// first surface met going out from the axis, averaged round (a neck's
    /// radius, not the shoulders' reach).
    fn core_radius(&self, axis: V3, y: f32) -> f32 {
        let mut sum = 0.0;
        let mut n = 0.0;
        for i in 0..24 {
            let a = i as f32 * std::f32::consts::TAU / 24.0;
            if let Some(hit) = self.trunk.ray([axis[0], y, axis[2]], [a.cos(), 0.0, a.sin()], 0.0, self.h * 0.3) {
                sum += hit.t;
                n += 1.0;
            }
        }
        if n > 0.0 { sum / n } else { 0.02 * self.h }
    }

    /// How far the body is from a vertical axis at height `y`, towards `a`.
    fn body_reach(&self, axis: V3, y: f32, a: f32) -> Option<f32> {
        let dir = [a.cos(), 0.0, a.sin()];
        Self::reach_in(&self.trunk, [axis[0], y, axis[2]], dir, self.h * 0.8)
    }

    /// The radius of the body round bone `b` at `t` along it, towards the
    /// angle `a` in the plane across the bone.
    fn limb_radius(&self, b: usize, t: f32, a: f32) -> f32 {
        let axis = self.axis(b);
        let (e1, e2) = perp(axis);
        let dir = add(scale(e1, a.cos()), scale(e2, a.sin()));
        let c = self.along(b, t);
        // The first surface going out from the bone: the limb's own, not a
        // thigh or a hip the arm hangs beside.
        self.bvh.ray(c, dir, 0.0, self.h * 0.1).map_or_else(|| Self::reach_in(&self.bvh, c, dir, self.h * 0.06).unwrap_or(self.h * 0.02), |hit| hit.t)
    }

    /// The y of a hem named in words, or a fraction from the shoulders to the floor.
    fn hem(&self, mk: &Marks, word: &str) -> f32 {
        let h = self.h;
        match lc(word).as_str() {
            "waist" | "cropped" | "crop" => mk.waist,
            "hip" | "hips" | "short" => mk.crotch - 0.05 * h,
            "crotch" => mk.crotch,
            "thigh" | "mini" => lerp([0.0, mk.crotch, 0.0], [0.0, mk.knee, 0.0], 0.4)[1],
            "midthigh" | "mid_thigh" => lerp([0.0, mk.crotch, 0.0], [0.0, mk.knee, 0.0], 0.55)[1],
            "knee" | "midi" => mk.knee,
            "calf" | "midcalf" | "mid_calf" | "long" => lerp([0.0, mk.knee, 0.0], [0.0, mk.ankle, 0.0], 0.5)[1],
            "ankle" | "maxi" => mk.ankle - 0.02 * h,
            "floor" | "gown" | "full" => mk.sole - 0.012 * h,
            other => other.parse::<f32>().map_or(mk.knee, |f| lerp([0.0, mk.neck, 0.0], [0.0, mk.sole, 0.0], f.clamp(0.0, 1.0))[1]),
        }
    }
}

/// Two axes across `axis`.
fn perp(axis: V3) -> (V3, V3) {
    let r = if axis[2].abs() < 0.9 { [0.0, 0.0, -1.0] } else { [0.0, 1.0, 0.0] };
    let a = normalise(cross(axis, r));
    (a, normalise(cross(axis, a)))
}

/// A circular moving average over a ring of values.
fn blur_ring(v: &mut [f32], times: usize) {
    let n = v.len();
    for _ in 0..times {
        let old = v.to_vec();
        for i in 0..n {
            v[i] = 0.25 * old[(i + n - 1) % n] + 0.5 * old[i] + 0.25 * old[(i + 1) % n];
        }
    }
}

// ------------------------------------------------------------- materials

/// The fabric a garment is made of: its material (named for the garment)
/// and the texture's tile size.
fn cloth(f: &Fit, spec: &Spec, g_name: &str, default_material: &str, default_colour: [u8; 3]) -> (Material, f32) {
    let name = spec.material.clone().unwrap_or_else(|| default_material.to_string());
    let colour = spec.colour.or_else(|| fabric::metal_colour(&name)).unwrap_or(default_colour);
    let mut m = fabric::material(&name, Some(colour), spec.colour2, spec.pattern, 128)
        .or_else(|| fabric::material(default_material, Some(colour), spec.colour2, spec.pattern, 128))
        .unwrap_or_default();
    m.name = g_name.to_string();
    (m, fabric::repeat_px(&name, spec.pattern, f.h))
}

/// A plain material for trim.
fn trim_material(spec: &Spec, g_name: &str, what: &str, default_material: &str, default_colour: [u8; 3]) -> Material {
    let colour = spec.colour2.or_else(|| fabric::metal_colour(default_material)).unwrap_or(default_colour);
    let mut m = fabric::material(default_material, Some(colour), None, Pattern::None, 64).unwrap_or_default();
    m.name = format!("{g_name} {what}");
    m
}

// --------------------------------------------------------------- shells

struct UpperStyle {
    /// 0 sleeveless, 1 short, 2 elbow, 3 three-quarter, 4 wrist.
    sleeve: u8,
    hem: &'static str,
    ease: f32,
    thick: f32,
    material: &'static str,
    colour: [u8; 3],
    /// 0 none, 1 band, 2 turned-down with lapels.
    collar: u8,
    buttons: u8,
    belt: bool,
    flare: f32,
    folds: f32,
    cuffs: bool,
    neck: f32,
    open: bool,
}

fn upper_style(kind: &str, style: &str) -> UpperStyle {
    let base = |sleeve, hem, ease, thick, material, colour, collar, buttons| UpperStyle { sleeve, hem, ease, thick, material, colour, collar, buttons, belt: false, flare: 0.0, folds: 0.6, cuffs: false, neck: 0.0, open: false };
    match (kind, style) {
        ("shirt", "tee" | "t_shirt" | "tshirt") => base(1, "hip", 0.006, 0.0025, "cotton", [200, 200, 205], 0, 0),
        ("shirt", "polo") => base(1, "hip", 0.007, 0.003, "cotton", [60, 90, 150], 1, 2),
        ("shirt", "tank" | "vest" | "singlet") => base(0, "hip", 0.005, 0.002, "cotton", [225, 225, 225], 0, 0),
        ("shirt", "sweater" | "jumper" | "pullover" | "sweatshirt") => UpperStyle { cuffs: true, neck: 0.4, folds: 0.4, ..base(4, "hip", 0.012, 0.007, "knit", [130, 40, 50], 0, 0) },
        ("shirt", "hoodie") => UpperStyle { cuffs: true, neck: 0.5, ..base(4, "hip", 0.018, 0.008, "flannel", [90, 100, 110], 0, 0) },
        ("shirt", "cardigan") => UpperStyle { cuffs: true, open: true, ..base(4, "hip", 0.013, 0.007, "knit", [110, 90, 70], 0, 5) },
        ("shirt", "tunic") => UpperStyle { cuffs: true, ..base(4, "thigh", 0.011, 0.004, "linen", [200, 190, 170], 1, 0) },
        ("shirt", _) => UpperStyle { cuffs: true, neck: 0.15, ..base(4, "hip", 0.009, 0.003, "cotton", [225, 225, 230], 2, 6) },
        ("jacket", "bomber" | "windbreaker") => UpperStyle { cuffs: true, neck: 0.3, ..base(4, "waist", 0.017, 0.006, "nylon", [60, 80, 60], 1, 0) },
        ("jacket", "leather" | "biker") => UpperStyle { cuffs: true, ..base(4, "hip", 0.013, 0.005, "leather", [30, 28, 28], 2, 1) },
        ("jacket", "denim") => UpperStyle { cuffs: true, ..base(4, "hip", 0.013, 0.004, "denim", [60, 90, 140], 2, 6) },
        ("jacket", _) => UpperStyle { cuffs: true, ..base(4, "hip", 0.014, 0.005, "wool", [50, 55, 70], 2, 2) },
        ("coat", "trench" | "trenchcoat") => UpperStyle { belt: true, flare: 0.14, folds: 0.9, cuffs: true, ..base(4, "knee", 0.02, 0.006, "cotton", [170, 140, 100], 2, 10) },
        ("coat", "peacoat") => UpperStyle { flare: 0.06, cuffs: true, ..base(4, "thigh", 0.022, 0.007, "wool", [35, 45, 70], 2, 6) },
        ("coat", "parka") => UpperStyle { flare: 0.05, cuffs: true, neck: 0.5, ..base(4, "thigh", 0.03, 0.01, "canvas", [70, 80, 60], 1, 0) },
        ("coat", "robe" | "kimono") => UpperStyle { flare: 0.1, open: true, ..base(4, "ankle", 0.025, 0.005, "silk", [150, 40, 60], 0, 0) },
        ("coat", "long" | "overcoat" | "greatcoat" | "duster") => UpperStyle { flare: 0.11, cuffs: true, ..base(4, "calf", 0.024, 0.007, "wool", [50, 50, 55], 2, 6) },
        ("coat", _) => UpperStyle { flare: 0.1, cuffs: true, ..base(4, "knee", 0.022, 0.007, "wool", [60, 60, 70], 2, 6) },
        _ => base(4, "hip", 0.01, 0.003, "cotton", [200, 200, 200], 0, 0),
    }
}

fn sleeve_level(word: &str) -> Option<u8> {
    Some(match lc(word).as_str() {
        "none" | "sleeveless" | "no" => 0,
        "short" | "cap" => 1,
        "elbow" | "half" => 2,
        "three_quarter" | "3/4" | "threequarter" => 3,
        "long" | "full" | "wrist" => 4,
        _ => return None,
    })
}

/// Shirts, jackets and coats.
fn upper(f: &Fit, spec: &Spec, kind: &str, g: &mut Garment) -> Result<(), String> {
    let mk = f.marks()?;
    let h = f.h;
    let st = upper_style(kind, &lc(&spec.style));
    let torso: Vec<usize> = ["hips", "spine", "chest", "neck"].iter().filter_map(|n| f.bone(n)).collect();
    let shoulders = f.both("shoulder");
    let uarms = f.both("upper_arm");
    let farms = f.both("forearm");
    f.need("chest")?;
    let sleeve = spec.sleeves.as_deref().and_then(sleeve_level).unwrap_or(st.sleeve);
    let hem_word = spec.length.clone().unwrap_or_else(|| st.hem.to_string());
    let hem_y = f.hem(&mk, &hem_word);
    let ease = st.ease * h * spec.fit.clamp(0.3, 3.0);
    // Where the shell stops: the waist line of the hips for a short hem, or
    // above the leg split when a skirt carries on below.
    let shell_end = hem_y.min(mk.crotch - 0.03 * h);
    let high_neck = st.neck > 0.05;
    let mut bones: Vec<usize> = torso.iter().copied().filter(|&b| high_neck || Some(b) != f.bone("neck")).collect();
    bones.extend(&shoulders);
    let mut cuts = vec![Cut { point: [0.0, shell_end, 0.0], dir: [0.0, 1.0, 0.0], bones: torso.iter().copied().filter(|&b| Some(b) != f.bone("neck")).collect() }];
    // The neckline: across the neck, higher on a collared or high-necked piece.
    let neck_b = f.need("neck")?;
    let neck_y = lerp([0.0, mk.neck, 0.0], [0.0, f.tail_of(neck_b)[1], 0.0], st.neck.max(0.0))[1];
    // A neckline: a round opening for the neck, or, with a high neck, a cut across it.
    if high_neck {
        cuts.push(Cut { point: [0.0, neck_y, 0.0], dir: [0.0, -1.0, 0.0], bones: torso.iter().copied().chain(shoulders.iter().copied()).collect() });
    }
    let nc = f.centre_at(f.along(neck_b, 0.5)[1], &[neck_b]);
    let neck_r = f.core_radius([nc[0], 0.0, nc[2]], f.along(neck_b, 0.5)[1]);
    let hole = (!high_neck).then_some((nc[0], nc[2], neck_r * if kind == "shirt" { 1.1 } else { 1.05 }, mk.neck + 0.02 * h));
    if sleeve > 0 {
        bones.extend(&uarms);
        for &ua in &uarms {
            if sleeve == 1 {
                cuts.push(Cut { point: f.along(ua, 0.62), dir: f.axis(ua), bones: vec![ua] });
            }
        }
        if sleeve >= 2 {
            bones.extend(&farms);
            let t = [0.0, 0.0, 0.0, 0.6, 0.94][sleeve as usize];
            for &fa in &farms {
                cuts.push(Cut { point: f.along(fa, t), dir: f.axis(fa), bones: vec![fa] });
            }
        }
    }
    let shell = ShellSpec { bones, cuts, ease, thick: st.thick * h, folds: st.folds, fold_size: h * 0.05, budget: (14_000.0 * spec.detail) as usize };
    let (mut m, j, w) = f.shell_with(&shell, hole)?;
    // Open front (cardigan, robe, open coat): a slit down the middle.
    let open = spec.open.unwrap_or(st.open);
    if open {
        slit(f, &mut m, mk.cx, 0.013 * h, neck_y, shell_end);
    }
    let (fab, tile) = cloth(f, spec, &g.name.clone(), st.material, st.colour);
    f.uvs(&mut m, tile);
    let mat = g.material(fab);
    let mut w = w;
    let mut j = j;
    skin_mode(spec, Skin::Smooth, &mut w, &mut j);
    g.push(&m, j, w, mat);
    let shell_bvh = Bvh::of_mesh(&m);
    // A rib round the neckline hides its cut edge: a clean, rolled collar.
    if let (Some((hx, hz, hr, _)), true) = (hole, spec.trim) {
        let from = mk.neck - 0.3 * h;
        let pts: Vec<V3> = (0..=40)
            .filter_map(|i| {
                let a = i as f32 / 40.0 * std::f32::consts::TAU;
                let (x, z) = (hx + hr * 1.06 * a.cos(), hz + hr * 1.06 * a.sin());
                shell_bvh.ray([x, from, z], [0.0, 1.0, 0.0], 0.0, h * 1.2).map(|hit| [x, from + hit.t, z])
            })
            .collect();
        if pts.len() > 30 {
            let mut path = pts.clone();
            path.push(pts[0]);
            let rib = loft::tube(&path, &vec![0.0105 * h; path.len()], 8);
            let (rj, rw) = f.weights_for(&rib.positions);
            g.push(&rib, rj, rw, mat);
        }
    }
    // The part below the leg split: a skirt of the coat.
    if hem_y > shell_end + 0.02 * h {
        let flare = st.flare * if spec.fit > 1.2 { 1.2 } else { 1.0 };
        let front_gap = if open { 0.22 } else { 0.0 };
        let top = shell_end - 0.025 * h;
        let (mut sk, sj, sw) = f.skirt_loft(&SkirtSpec { top_y: top, hem_y, pad: ease * 1.05, flare, front_gap, back_vent: if kind == "coat" { 0.0 } else { 0.0 }, pleats: 0.0, thick: st.thick * h, wave: 0.01 * h, top_shell: Some(&shell_bvh), angles: None, segments: 72, long_follow: 0.38, shell_overlap: 0.9 }, &mk)?;
        f.uvs(&mut sk, tile);
        if spec.loose {
            g.loose = Some(sk);
            g.notes.push("the tails are left free for cloth simulation".into());
        } else {
            let mut sj = sj;
            let mut sw = sw;
            skin_mode(spec, Skin::Smooth, &mut sw, &mut sj);
            g.push(&sk, sj, sw, mat);
        }
    }
    if spec.trim {
        trims_upper(f, spec, &st, &mk, &shell_bvh, hem_y.min(shell_end), g, &farms, sleeve)?;
    }
    Ok(())
}

/// Removes triangles in a strip down the front (the garment hangs open).
fn slit(f: &Fit, m: &mut Mesh, cx: f32, half: f32, top_y: f32, bottom_y: f32) {
    let _ = f;
    // The front is -z of the middle.
    let (lo, hi) = m.bounds();
    let mid_z = (lo[2] + hi[2]) * 0.5;
    let keep: Vec<bool> = (0..m.triangles()).map(|t| {
        let c = m.centroid(t);
        !((c[0] - cx).abs() < half && c[2] < mid_z && c[1] > top_y && c[1] < bottom_y + 0.05 * f.h)
    }).collect();
    let mut idx = Vec::new();
    for (t, k) in keep.iter().enumerate() {
        if *k {
            idx.extend_from_slice(&m.indices[3 * t..3 * t + 3]);
        }
    }
    m.indices = idx;
    let mats = vec![0u32; m.triangles()];
    m.regroup(&mats);
}

fn skin_mode(spec: &Spec, auto: Skin, w: &mut [[f32; 4]], j: &mut [[u16; 4]]) {
    let mode = if spec.skin == Skin::Auto { auto } else { spec.skin };
    match mode {
        Skin::Stiff => Fit::stiff(w),
        Skin::Rigid => {
            // The bone that carries most of the piece, for every vertex.
            let mut sums: std::collections::HashMap<u16, f32> = Default::default();
            for (jj, ww) in j.iter().zip(w.iter()) {
                for k in 0..4 {
                    *sums.entry(jj[k]).or_default() += ww[k];
                }
            }
            let best = sums.iter().max_by(|a, b| a.1.total_cmp(b.1).then(b.0.cmp(a.0))).map_or(0, |(b, _)| *b);
            for (jj, ww) in j.iter_mut().zip(w.iter_mut()) {
                *jj = [best, 0, 0, 0];
                *ww = [1.0, 0.0, 0.0, 0.0];
            }
        }
        _ => {}
    }
}

/// Collar, lapels, buttons, belt, cuffs.
#[allow(clippy::too_many_arguments)]
fn trims_upper(f: &Fit, spec: &Spec, st: &UpperStyle, mk: &Marks, shell: &Bvh, shell_end: f32, g: &mut Garment, farms: &[usize], sleeve: u8) -> Result<(), String> {
    let h = f.h;
    let name = g.name.clone();
    let trim = g.material(trim_material(spec, &name, "trim", if st.material == "leather" || st.material == "cotton" || st.material == "wool" { "leather" } else { "wool" }, spec.colour2.unwrap_or_else(|| shade(spec.colour.unwrap_or(st.colour), 0.7))));
    let chest = f.need("chest")?;
    let neck = f.need("neck")?;
    let axis_z = f.centre_at(mk.neck, &[chest, neck])[2];
    let ax = [mk.cx, 0.0, axis_z];
    // Collar.
    if st.collar > 0 {
        let ny = f.along(neck, 0.4)[1];
        let r0 = f.core_radius([ax[0], 0.0, f.centre_at(ny, &[neck])[2]], ny).max(0.02 * h) + st.ease * h * 0.9;
        let rows = 6;
        let (hc, flare) = if st.collar == 2 { (0.05 * h, 0.5) } else { (0.045 * h, 0.12) };
        let y0 = mk.neck - 0.01 * h;
        let mut m = loft::surface(
            64,
            rows,
            true,
            |u, v| {
                let a = u * std::f32::consts::TAU;
                // Taller at the back (+z), open at the front for a turned collar.
                let back = (a.sin() * 0.5 + 0.5).powf(1.0);
                let height = hc * (0.55 + 0.9 * back);
                let r = r0 * (1.0 + flare * v * v) + 0.004 * h * v;
                [ax[0] + r * a.cos(), y0 - height * v, ax[2] + r * a.sin()]
            },
            |p| [p[0] - ax[0], 0.0, p[2] - ax[2]],
        );
        if st.collar == 2 {
            // Open at the front.
            let keep: Vec<u32> = m.indices.chunks(3).filter(|t| t.iter().all(|&i| { let p = m.positions[i as usize]; !((p[0] - ax[0]).abs() < 0.02 * h && p[2] < ax[2]) })).flatten().copied().collect();
            m.indices = keep;
        }
        let (j, w) = f.weights_for(&m.positions);
        let mats = vec![0u32; m.triangles()];
        m.regroup(&mats);
        let (m, j, w) = add_rim(m, j, w, 0.003 * h);
        g.push(&m, j, w, trim);
    }
    // Lapels: two panels on the chest, shaped like the front of a collar.
    if st.collar == 2 {
        for sx in [1.0f32, -1.0] {
            let mut pts: Vec<V3> = Vec::new();
            let (ny, my) = (mk.neck + 0.005 * h, lerp([0.0, mk.neck, 0.0], [0.0, mk.waist, 0.0], 0.8)[1]);
            let (cols, rows) = (4usize, 10usize);
            for r in 0..rows {
                let v = r as f32 / (rows - 1) as f32;
                let y = ny + (my - ny) * v;
                let inner = 0.012 * h;
                let width = (0.075 * h) * (1.0 - 0.45 * v) + 0.006 * h;
                for c in 0..cols {
                    let u = c as f32 / (cols - 1) as f32;
                    let x = mk.cx + sx * (inner + (width - inner * 0.0) * u);
                    // Down onto the garment's front.
                    let from = [x, y, axis_z - h * 0.4];
                    match shell.ray(from, [0.0, 0.0, 1.0], 0.0, h * 0.8) {
                        Some(hit) => pts.push([x, y, from[2] + hit.t - 0.004 * h]),
                        None => pts.push([x, y, axis_z - 0.1 * h]),
                    }
                }
            }
            let m = loft::surface(cols, rows, false, |u, v| pts[((v * (rows - 1) as f32).round() as usize) * cols + (u * (cols - 1) as f32).round() as usize], |_| [0.0, 0.0, -1.0]);
            let (j, w) = f.weights_for(&m.positions);
            let (m, j, w) = add_rim(m, j, w, 0.003 * h);
            g.push(&m, j, w, trim);
        }
    }
    // Buttons down the front.
    if st.buttons > 0 {
        let button = g.material(trim_material(spec, &name, "buttons", "plastic", shade(spec.colour.unwrap_or(st.colour), 0.45)));
        let double = st.buttons >= 6 && st.buttons != 5;
        let n_rows = if st.buttons == 5 { 6 } else { st.buttons as usize };
        let n = n_rows.clamp(1, 12);
        let cols: Vec<f32> = if st.buttons >= 6 && st.collar == 2 && st.buttons != 6 || st.buttons == 10 { vec![-0.03, 0.03] } else { vec![0.0] };
        let _ = double;
        for col in &cols {
            for i in 0..n {
                let v = (i as f32 + 0.5) / n as f32;
                let y = lerp([0.0, mk.neck + 0.04 * h, 0.0], [0.0, shell_end - 0.03 * h, 0.0], v)[1];
                let x = mk.cx + col * h;
                let from = [x, y, axis_z - h * 0.4];
                if let Some(hit) = shell.ray(from, [0.0, 0.0, 1.0], 0.0, h * 0.8) {
                    let p = [x, y, from[2] + hit.t - 0.003 * h];
                    let mut b = crate::prim::uv_sphere(0.0065 * h, 12, 8);
                    crate::ops::transform(&mut b, &crate::region::Region::All, [0.0; 3], [0.0; 3], [1.0, 1.0, 0.45], [0.0; 3]);
                    for q in &mut b.positions {
                        *q = add(*q, p);
                    }
                    let (j, w) = f.weights_for(&b.positions);
                    let mut j = j;
                    let mut w = w;
                    // Hard buttons ride whole on the bone under them.
                    skin_mode(&Spec { skin: Skin::Rigid, ..spec.clone() }, Skin::Rigid, &mut w, &mut j);
                    g.push(&b, j, w, button);
                }
            }
        }
    }
    // Belt.
    if st.belt {
        let y = mk.waist + 0.01 * h;
        let bm = g.material(trim_material(spec, &name, "belt", "leather", shade(spec.colour.unwrap_or(st.colour), 0.8)));
        let axis = [mk.cx, 0.0, f.centre_at(y, &[f.need("spine")?])[2]];
        let belt = loft::band(axis, y, 0.032 * h, 0.006 * h, |a| Fit::reach_in(shell, [axis[0], y, axis[2]], [a.cos(), 0.0, a.sin()], h * 0.8).unwrap_or(0.1 * h) + 0.002 * h, 64);
        let (j, w) = f.weights_for(&belt.positions);
        g.push(&belt, j, w, bm);
        // The buckle at the front (-z).
        let r = Fit::reach_in(shell, [axis[0], y, axis[2]], [0.0, 0.0, -1.0], h * 0.8).unwrap_or(0.1 * h);
        let mut buckle = crate::prim::cube([0.03 * h, 0.034 * h, 0.006 * h]);
        for q in &mut buckle.positions {
            *q = add(*q, [axis[0], y, axis[2] - r - 0.008 * h]);
        }
        let metal = g.material(trim_material(&Spec { colour2: Some([200, 170, 90]), ..spec.clone() }, &name, "buckle", "brass", [200, 170, 90]));
        let (mut j, mut w) = f.weights_for(&buckle.positions);
        skin_mode(&Spec { skin: Skin::Rigid, ..spec.clone() }, Skin::Rigid, &mut w, &mut j);
        g.push(&buckle, j, w, metal);
    }
    // Cuffs at the wrists.
    if st.cuffs && sleeve >= 3 {
        for &fa in farms {
            let t1 = if sleeve == 4 { 0.94 } else { 0.6 };
            if let Some(m) = f.ring(fa, t1 - 0.08, t1 + 0.02, st.ease * h * 1.15, 0.004 * h, 28) {
                let (j, w) = f.weights_for(&m.positions);
                g.push(&m, j, w, trim);
            }
        }
    }
    Ok(())
}

fn shade(c: [u8; 3], k: f32) -> [u8; 3] {
    [(c[0] as f32 * k) as u8, (c[1] as f32 * k) as u8, (c[2] as f32 * k) as u8]
}

impl<'a> Fit<'a> {
    /// A short tube round a limb bone from `t0` to `t1` along it, `pad` px off the body.
    fn ring(&self, bone: usize, t0: f32, t1: f32, pad: f32, thick: f32, segs: usize) -> Option<Mesh> {
        let axis = self.axis(bone);
        let (e1, e2) = perp(axis);
        let (a, b) = (self.along(bone, t0), self.along(bone, t1));
        let radii: Vec<f32> = (0..segs).map(|i| self.limb_radius(bone, (t0 + t1) * 0.5, i as f32 * std::f32::consts::TAU / segs as f32)).collect();
        if radii.iter().all(|r| *r <= 0.0) {
            return None;
        }
        let m = loft::surface(
            segs,
            4,
            true,
            |u, v| {
                let i = ((u * segs as f32).round() as usize) % segs;
                let ang = u * std::f32::consts::TAU;
                let (r_off, along) = [(0.0, 0.0), (thick, 0.0), (thick, 1.0), (0.0, 1.0)][((v * 3.0).round() as usize).min(3)];
                let r = radii[i] + pad + r_off;
                add(lerp(a, b, along), add(scale(e1, r * ang.cos()), scale(e2, r * ang.sin())))
            },
            |p| sub(p, lerp(a, b, 0.5)),
        );
        Some(m)
    }
}

// ------------------------------------------------------------- skirts

struct SkirtSpec<'a> {
    top_y: f32,
    hem_y: f32,
    pad: f32,
    /// Widening towards the hem (0.1 = 10% wider at the hem than at the top).
    flare: f32,
    /// Radians of opening at the front (0 closed).
    front_gap: f32,
    #[allow(dead_code)]
    back_vent: f32,
    pleats: f32,
    thick: f32,
    wave: f32,
    /// The shell above, to take the top ring's size from.
    top_shell: Option<&'a Bvh>,
    /// Only these angles (start, end in radians) rather than all the way round.
    angles: Option<(f32, f32)>,
    segments: usize,
    /// How much of the way the hem follows the legs (0..1).
    long_follow: f32,
    shell_overlap: f32,
}

impl<'a> Fit<'a> {
    /// A skirt: a surface round the body from `top_y` down to the hem, as
    /// wide as the body there plus `pad` and flared, skinned between the hips
    /// and the two thighs so it swings with the legs without stretching.
    fn skirt_loft(&self, s: &SkirtSpec, mk: &Marks) -> Result<(Mesh, Vec<[u16; 4]>, Vec<[f32; 4]>), String> {
        let h = self.h;
        let hips = self.need("hips")?;
        let torso = [self.bone("spine"), self.bone("hips"), self.bone("chest")].into_iter().flatten().collect::<Vec<_>>();
        let c = self.centre_at(s.top_y, &torso);
        let axis = [mk.cx, 0.0, c[2]];
        let nu = s.segments.max(16);
        let nv = 16usize;
        let (a0, a1) = s.angles.unwrap_or((0.0, std::f32::consts::TAU));
        let full = s.angles.is_none();
        let cols = if full { nu } else { nu + 1 };
        let top_r: Vec<f32> = (0..cols)
            .map(|i| {
                let a = a0 + (a1 - a0) * i as f32 / if full { nu } else { nu - 1 } as f32;
                let body = self.body_reach(axis, s.top_y, a);
                let shell = s.top_shell.and_then(|b| Self::reach_in(b, [axis[0], s.top_y, axis[2]], [a.cos(), 0.0, a.sin()], h * 0.8));
                body.or(shell).unwrap_or(0.1 * h)
            })
            .collect();
        // Radii down the surface: the body's outermost reach (legs included),
        // the flare, then smoothed so the surface hangs like cloth.
        let mut radii = vec![vec![0.0f32; cols]; nv];
        let sw_r = top_r.iter().copied().fold(0.0f32, f32::max);
        for j in 0..nv {
            let t = j as f32 / (nv - 1) as f32;
            let y = s.top_y + (s.hem_y - s.top_y) * t;
            for i in 0..cols {
                let a = a0 + (a1 - a0) * i as f32 / if full { nu } else { nu - 1 } as f32;
                let body = self.body_reach(axis, y, a).unwrap_or(0.0);
                let hang = top_r[i] * (1.0 + s.flare * t.powf(1.2)) + s.pad * (1.0 + 0.6 * t);
                let leg_clear = body + s.pad * 1.1;
                radii[j][i] = hang.max(leg_clear);
            }
            if full {
                blur_ring(&mut radii[j], 3);
            }
        }
        for _ in 0..2 {
            for j in 1..nv - 1 {
                for i in 0..cols {
                    let v = 0.25 * radii[j - 1][i] + 0.5 * radii[j][i] + 0.25 * radii[j + 1][i];
                    radii[j][i] = v.max(0.0);
                }
            }
        }
        // The first row sits under the shell above, just inside it.
        for i in 0..cols {
            radii[0][i] = (top_r[i] + s.pad * 0.5 * s.shell_overlap).max(0.01 * h);
        }
        let hem_wave = s.wave;
        let front = -std::f32::consts::FRAC_PI_2;
        let mut m = loft::surface(
            nu,
            nv,
            full,
            |u, v| {
                let col = if full { (u * nu as f32).round() as usize % nu } else { (u * (cols - 1) as f32).round() as usize };
                let a = a0 + (a1 - a0) * u;
                let row = ((v * (nv - 1) as f32).round() as usize).min(nv - 1);
                let pleat = if s.pleats > 0.0 { 1.0 + s.pleats * 0.04 * (a * 24.0).sin() * v } else { 1.0 };
                let r = radii[row][col.min(cols - 1)] * pleat;
                // The hem rises a little at the front and falls at the sides.
                let y = s.top_y + (s.hem_y - s.top_y) * v + hem_wave * v * (a - front).cos();
                [axis[0] + r * a.cos(), y, axis[2] + r * a.sin()]
            },
            |p| [p[0] - axis[0], 0.0, p[2] - axis[2]],
        );
        // An opening down the front.
        if s.front_gap > 0.0 {
            let keep: Vec<u32> = m
                .indices
                .chunks(3)
                .filter(|t| {
                    let c = scale(add(add(m.positions[t[0] as usize], m.positions[t[1] as usize]), m.positions[t[2] as usize]), 1.0 / 3.0);
                    let a = (c[2] - axis[2]).atan2(c[0] - axis[0]);
                    let d = (a - front + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
                    d.abs() > s.front_gap
                })
                .flatten()
                .copied()
                .collect();
            m.indices = keep;
        }
        let _ = sw_r;
        let mats = vec![0u32; m.triangles()];
        m.regroup(&mats);
        // Skin: the hips at the top, the thighs towards the hem.
        let (tl, tr) = (self.bone("thigh.L"), self.bone("thigh.R"));
        let n = m.positions.len();
        let mut j = Vec::with_capacity(n);
        let mut w = Vec::with_capacity(n);
        let span = (s.hem_y - s.top_y).abs().max(1.0);
        for p in &m.positions {
            let t = ((p[1] - s.top_y) / span).clamp(0.0, 1.0);
            let follow = s.long_follow * t.powf(1.3);
            match (tl, tr) {
                (Some(l), Some(r)) => {
                    let side = smoothstep(-0.28 * 0.1 * h * 2.0, 0.28 * 0.1 * h * 2.0, p[0] - axis[0]);
                    j.push([hips as u16, l as u16, r as u16, 0]);
                    w.push([1.0 - follow, follow * side, follow * (1.0 - side), 0.0]);
                }
                _ => {
                    j.push([hips as u16, 0, 0, 0]);
                    w.push([1.0, 0.0, 0.0, 0.0]);
                }
            }
        }
        // The top rows take the body's own weights, as the shell above them
        // does, and give way to the skirt's own below: no seam pulls apart.
        let (bj, bw) = self.weights_for(&m.positions);
        let (mut j, mut w) = (j, w);
        for (k, p) in m.positions.iter().enumerate() {
            let t = ((p[1] - s.top_y) / span).clamp(0.0, 1.0);
            let a = smoothstep(0.0, 0.3, t);
            if a >= 1.0 {
                continue;
            }
            let mut acc: Vec<(u16, f32)> = Vec::new();
            for q in 0..4 {
                if w[k][q] > 0.0 {
                    acc.push((j[k][q], w[k][q] * a));
                }
                // A hand hanging by the hip is nearer than the hip itself:
                // a skirt never follows the arms.
                if bw[k][q] > 0.0 && !self.on_arm(bj[k][q] as usize) {
                    match acc.iter_mut().find(|e| e.0 == bj[k][q]) {
                        Some(e) => e.1 += bw[k][q] * (1.0 - a),
                        None => acc.push((bj[k][q], bw[k][q] * (1.0 - a))),
                    }
                }
            }
            acc.sort_by(|x, y| y.1.total_cmp(&x.1));
            acc.truncate(4);
            let tot: f32 = acc.iter().map(|e| e.1).sum::<f32>().max(1e-6);
            let mut jj = [0u16; 4];
            let mut ww = [0.0f32; 4];
            for (q, (b, x)) in acc.iter().enumerate() {
                jj[q] = *b;
                ww[q] = x / tot;
            }
            j[k] = jj;
            w[k] = ww;
        }
        smooth_weights(&m, &mut j, &mut w, 8);
        let (m, j, w) = add_rim(m, j, w, s.thick);
        Ok((m, j, w))
    }
}

fn skirt_or_dress(f: &Fit, spec: &Spec, kind: &str, g: &mut Garment) -> Result<(), String> {
    let mk = f.marks()?;
    let h = f.h;
    let style = lc(&spec.style);
    let (default_hem, flare, pleats, mat, col): (&str, f32, f32, &str, [u8; 3]) = match (kind, style.as_str()) {
        ("skirt", "mini") => ("thigh", 0.1, 0.0, "cotton", [150, 60, 80]),
        ("skirt", "pleated" | "kilt") => ("knee", 0.18, 1.0, "wool", [70, 70, 110]),
        ("skirt", "long" | "maxi") => ("ankle", 0.25, 0.0, "cotton", [90, 70, 120]),
        ("skirt", "tutu") => ("thigh", 0.6, 1.0, "nylon", [240, 180, 200]),
        ("skirt", _) => ("knee", 0.16, 0.0, "cotton", [90, 80, 100]),
        ("dress", "sundress") => ("knee", 0.2, 0.0, "cotton", [230, 190, 90]),
        ("dress", "gown") => ("floor", 0.4, 0.0, "satin", [140, 30, 60]),
        ("dress", "mini") => ("thigh", 0.14, 0.0, "cotton", [40, 40, 50]),
        ("dress", "jumpsuit" | "overalls") => ("ankle", 0.0, 0.0, "denim", [60, 90, 140]),
        _ => ("calf", 0.2, 0.0, "silk", [60, 90, 130]),
    };
    let hem_y = f.hem(&mk, spec.length.as_deref().unwrap_or(default_hem));
    let (fab, tile) = cloth(f, spec, &g.name.clone(), mat, col);
    let ease = 0.011 * h * spec.fit.clamp(0.3, 3.0);
    let mat_i = g.material(fab);
    let waist_y = mk.waist + 0.025 * h;
    let top_y = if kind == "skirt" { waist_y } else { mk.crotch - 0.03 * h };
    if kind == "dress" {
        // The bodice: a shell over the torso, strapless or with sleeves.
        let torso: Vec<usize> = ["hips", "spine", "chest"].iter().filter_map(|n| f.bone(n)).collect();
        let shoulders = f.both("shoulder");
        let sleeve = spec.sleeves.as_deref().and_then(sleeve_level).unwrap_or(if style == "sundress" || style == "gown" { 0 } else { 1 });
        let mut bones = torso.clone();
        let mut cuts = vec![Cut { point: [0.0, top_y, 0.0], dir: [0.0, 1.0, 0.0], bones: torso.clone() }];
        let neck = f.need("neck")?;
        let neck_y = if sleeve == 0 { lerp([0.0, mk.neck, 0.0], [0.0, mk.shoulder, 0.0], 0.55)[1] + 0.02 * h } else { mk.neck };
        cuts.push(Cut { point: [0.0, neck_y, 0.0], dir: [0.0, -1.0, 0.0], bones: torso.iter().copied().chain(shoulders.iter().copied()).collect() });
        if sleeve == 0 {
            // Strapless to thin straps: just the torso.
        } else {
            bones.extend(&shoulders);
            let ua = f.both("upper_arm");
            bones.extend(&ua);
            for &u in &ua {
                cuts.push(Cut { point: f.along(u, 0.55), dir: f.axis(u), bones: vec![u] });
            }
        }
        let _ = neck;
        let (m, j, w) = f.shell(&ShellSpec { bones, cuts, ease, thick: 0.003 * h, folds: 0.4, fold_size: h * 0.05, budget: (9000.0 * spec.detail) as usize })?;
        let mut m = m;
        f.uvs(&mut m, tile);
        g.push(&m, j, w, mat_i);
    }
    let bvh_top = None;
    let (mut sk, sj, sw) = f.skirt_loft(&SkirtSpec { top_y: top_y - 0.02 * h, hem_y, pad: ease, flare, front_gap: 0.0, back_vent: 0.0, pleats, thick: 0.003 * h, wave: 0.0, top_shell: bvh_top, angles: None, segments: 80, long_follow: if hem_y > mk.knee { 0.34 } else { 0.45 }, shell_overlap: 1.0 }, &mk)?;
    f.uvs(&mut sk, tile);
    if spec.loose {
        g.loose = Some(sk);
    } else {
        g.push(&sk, sj, sw, mat_i);
    }
    // A waistband.
    if spec.trim && kind == "skirt" {
        let band_m = g.material(trim_material(spec, &g.name.clone(), "band", "cotton", shade(spec.colour.unwrap_or(col), 0.7)));
        let axis = [mk.cx, 0.0, f.centre_at(top_y, &[f.need("hips")?])[2]];
        let band = loft::band(axis, top_y, 0.03 * h, 0.004 * h, |a| f.body_reach(axis, top_y, a).unwrap_or(0.1 * h) + ease * 1.2, 56);
        let (j, w) = f.weights_for(&band.positions);
        g.push(&band, j, w, band_m);
    }
    Ok(())
}

// ------------------------------------------------------------- trousers

fn lower(f: &Fit, spec: &Spec, g: &mut Garment) -> Result<(), String> {
    let mk = f.marks()?;
    let h = f.h;
    let style = lc(&spec.style);
    let (hem_default, ease_f, thick_f, mat, col): (&str, f32, f32, &str, [u8; 3]) = match style.as_str() {
        "shorts" | "bermuda" => ("thigh", 0.012, 0.004, "cotton", [150, 130, 100]),
        "jeans" => ("floor", 0.009, 0.0035, "denim", [60, 85, 130]),
        "leggings" => ("ankle", 0.003, 0.002, "nylon", [30, 30, 36]),
        "slacks" | "chinos" | "trousers" | "pants" | "" => ("floor", 0.011, 0.004, "wool", [55, 55, 62]),
        _ => ("floor", 0.011, 0.004, "cotton", [90, 90, 95]),
    };
    let hem_y = f.hem(&mk, spec.length.as_deref().unwrap_or(hem_default));
    let ease = ease_f * h * spec.fit.clamp(0.3, 3.0);
    let mut bones: Vec<usize> = f.both("thigh");
    bones.extend(f.both("shin"));
    if let Some(b) = f.bone("hips") {
        bones.push(b);
    }
    if bones.len() < 3 {
        return Err("trousers need thigh and shin bones (a humanoid)".into());
    }
    let waist = mk.waist + 0.03 * h;
    let mut cuts = vec![Cut { point: [0.0, waist, 0.0], dir: [0.0, -1.0, 0.0], bones: vec![] }];
    // The hem across each leg, square to the bone.
    for s in ["L", "R"] {
        let (Some(th), Some(sh)) = (f.bone(&format!("thigh.{s}")), f.bone(&format!("shin.{s}"))) else { continue };
        let _ = th;
        // Where the hem plane crosses the leg.
        let shin_top = f.head_of(sh)[1];
        let bone = if hem_y < shin_top { th } else { sh };
        let (a, b) = (f.head_of(bone), f.tail_of(bone));
        let t = ((hem_y - a[1]) / (b[1] - a[1]).abs().max(1.0) * (b[1] - a[1]).signum()).clamp(0.0, 1.0);
        let p = lerp(a, b, t);
        cuts.push(Cut { point: p, dir: f.axis(bone), bones: vec![bone] });
        if bone == th {
            cuts.push(Cut { point: p, dir: [0.0, 1.0, 0.0], bones: vec![sh] });
        }
    }
    let (m, j, w) = f.shell(&ShellSpec { bones, cuts, ease, thick: thick_f * h, folds: 0.7, fold_size: h * 0.045, budget: (14_000.0 * spec.detail) as usize })?;
    let (fab, tile) = cloth(f, spec, &g.name.clone(), mat, col);
    let mut m = m;
    f.uvs(&mut m, tile);
    let mi = g.material(fab);
    g.push(&m, j, w, mi);
    // A waistband and belt loops' line.
    if spec.trim {
        let band_m = g.material(trim_material(spec, &g.name.clone(), "waistband", mat, shade(spec.colour.unwrap_or(col), 0.8)));
        let axis = [mk.cx, 0.0, f.centre_at(waist, &[f.need("hips")?])[2]];
        let band = loft::band(axis, waist + 0.004 * h, 0.03 * h, 0.003 * h, |a| f.body_reach(axis, waist, a).unwrap_or(0.1 * h) + ease * 1.25, 56);
        let (j, w) = f.weights_for(&band.positions);
        g.push(&band, j, w, band_m);
    }
    Ok(())
}

// ----------------------------------------------------------------- cape

fn cape(f: &Fit, spec: &Spec, g: &mut Garment) -> Result<(), String> {
    let mk = f.marks()?;
    let h = f.h;
    let style = lc(&spec.style);
    let (hem_default, mat, col, around): (&str, &str, [u8; 3], f32) = match style.as_str() {
        "poncho" => ("hip", "wool", [140, 90, 60], 3.0),
        "shawl" => ("waist", "wool", [120, 100, 110], 2.6),
        "cloak" | "mantle" => ("ankle", "wool", [50, 40, 60], 2.8),
        _ => ("calf", "velvet", [140, 30, 40], 2.4),
    };
    let hem_y = f.hem(&mk, spec.length.as_deref().unwrap_or(hem_default));
    let (fab, tile) = cloth(f, spec, &g.name.clone(), mat, col);
    let mi = g.material(fab);
    let pad = 0.014 * h * spec.fit.clamp(0.5, 3.0);
    let top = mk.neck + 0.005 * h;
    let chest = f.need("chest")?;
    let spine = f.need("spine")?;
    let _ = (chest, spine);
    // From shoulder to shoulder round the back (+z).
    let back = std::f32::consts::FRAC_PI_2;
    let spread = around * 0.5 * 0.55;
    let (mut m, j, w) = {
        let (mut m, _, _) = f.skirt_loft(&SkirtSpec { top_y: top, hem_y, pad, flare: 0.5, front_gap: 0.0, back_vent: 0.0, pleats: 0.0, thick: 0.004 * h, wave: 0.0, top_shell: None, angles: Some((back - spread, back + spread)), segments: 48, long_follow: 0.1, shell_overlap: 1.0 }, &mk)?;
        // Skin from the chest at the shoulders to the spine and hips below.
        let hips = f.need("hips")?;
        let span = (hem_y - top).abs().max(1.0);
        let mut j = Vec::new();
        let mut w = Vec::new();
        for p in &m.positions {
            let t = ((p[1] - top) / span).clamp(0.0, 1.0);
            j.push([chest as u16, spine as u16, hips as u16, 0]);
            w.push([(1.0 - t * 1.6).max(0.0), (1.0 - (t - 0.35).abs() * 2.0).clamp(0.0, 1.0), (t * 1.4 - 0.4).clamp(0.0, 1.0), 0.0]);
        }
        for x in &mut w {
            let s: f32 = x.iter().sum();
            if s > 0.0 {
                for v in x.iter_mut() {
                    *v /= s;
                }
            } else {
                x[0] = 1.0;
            }
        }
        m.materials = vec![Material::default()];
        (m, j, w)
    };
    f.uvs(&mut m, tile);
    if spec.loose {
        g.loose = Some(m);
        g.notes.push("the cape is left free for cloth simulation".into());
    } else {
        g.push(&m, j, w, mi);
    }
    Ok(())
}

// ---------------------------------------------------------------- shoes

fn shoes(f: &Fit, spec: &Spec, g: &mut Garment) -> Result<(), String> {
    let h = f.h;
    let style = lc(&spec.style);
    // The shaft: how far up the shin it climbs (0 none).
    let (shaft, ease_f, mat, col, sole): (f32, f32, &str, [u8; 3], f32) = match style.as_str() {
        "boots" | "boot" => (0.55, 0.007, "leather", [70, 45, 30], 0.012),
        "sneakers" | "sneaker" | "trainers" => (0.06, 0.009, "canvas", [235, 235, 235], 0.016),
        "sandals" => (0.0, 0.004, "leather", [150, 100, 60], 0.01),
        "heels" => (0.04, 0.004, "leather", [30, 30, 30], 0.012),
        "slippers" => (0.0, 0.008, "felt", [150, 90, 100], 0.006),
        _ => (0.05, 0.006, "leather", [40, 28, 24], 0.012),
    };
    let sides: Vec<&str> = match spec.side {
        Some('L') => vec!["L"],
        Some('R') => vec!["R"],
        _ => vec!["L", "R"],
    };
    let ease = ease_f * h * spec.fit.clamp(0.3, 3.0);
    let (fab, tile) = cloth(f, spec, &g.name.clone(), mat, col);
    let mi = g.material(fab);
    let sole_m = g.material(trim_material(spec, &g.name.clone(), "sole", "rubber", [28, 26, 26]));
    let mut any = false;
    for s in sides {
        let (Some(foot), Some(shin)) = (f.bone(&format!("foot.{s}")), f.bone(&format!("shin.{s}"))) else { continue };
        let mut bones = vec![foot];
        bones.extend(f.bone(&format!("toe.{s}")));
        let mut cuts = Vec::new();
        // The top line: above the ankle for a shoe, up the shin for a boot.
        let top_t = (1.0 - shaft - 0.04).clamp(0.0, 1.0);
        bones.push(shin);
        cuts.push(Cut { point: f.along(shin, top_t), dir: scale(f.axis(shin), -1.0), bones: vec![shin] });
        let (mut m, mut j, mut w) = f.shell(&ShellSpec { bones, cuts, ease, thick: 0.004 * h, folds: 0.0, fold_size: h * 0.04, budget: (5000.0 * spec.detail) as usize })?;
        // A sole: the underside pushed down and given its own material.
        let (_, hi) = f.mesh.bounds();
        let mut mats = vec![0u32; m.triangles()];
        for p in m.positions.iter_mut() {
            if p[1] > hi[1] - 0.05 * h {
                let k = ((p[1] - (hi[1] - 0.05 * h)) / (0.05 * h)).clamp(0.0, 1.0);
                p[1] += sole * h * k.powf(2.0);
            }
        }
        crate::ops::smooth_normals_in_place(&mut m);
        for (t, mt) in mats.iter_mut().enumerate() {
            let c = m.centroid(t);
            if c[1] > hi[1] - 0.012 * h && m.face_normal(t)[1] > 0.5 {
                *mt = 1;
            }
        }
        m.materials = vec![Material::default(), Material::default()];
        m.regroup(&mats);
        f.uvs(&mut m, tile);
        // Shoes ride whole on the foot, boots follow the leg a little.
        skin_mode(spec, if shaft > 0.2 { Skin::Smooth } else { Skin::Rigid }, &mut w, &mut j);
        // Two materials in one piece: main and sole.
        let mut piece_mats = m.tri_materials();
        for x in &mut piece_mats {
            *x = if *x == 1 { sole_m } else { mi };
        }
        let base = g.mesh.positions.len() as u32;
        let mut all = g.mesh.tri_materials();
        all.extend(piece_mats);
        g.mesh.positions.extend_from_slice(&m.positions);
        g.mesh.normals.extend_from_slice(&m.normals);
        g.mesh.uvs.extend_from_slice(&m.uvs);
        g.mesh.indices.extend(m.indices.iter().map(|i| i + base));
        g.joints.extend(j);
        g.weights.extend(w);
        g.mesh.regroup(&all);
        any = true;
    }
    if !any {
        return Err("shoes need foot and shin bones".into());
    }
    Ok(())
}

// --------------------------------------------------------------- gloves

fn gloves(f: &Fit, spec: &Spec, g: &mut Garment) -> Result<(), String> {
    let h = f.h;
    let style = lc(&spec.style);
    let (cuff, ease_f, mat, col): (f32, f32, &str, [u8; 3]) = match style.as_str() {
        "gauntlets" | "gauntlet" => (0.45, 0.01, "leather", [60, 50, 45]),
        "mittens" | "mitts" => (0.12, 0.012, "wool", [180, 60, 60]),
        _ => (0.12, 0.004, "leather", [35, 30, 30]),
    };
    let ease = ease_f * h * spec.fit.clamp(0.3, 3.0);
    let (fab, tile) = cloth(f, spec, &g.name.clone(), mat, col);
    let mi = g.material(fab);
    let sides: Vec<&str> = match spec.side {
        Some('L') => vec!["L"],
        Some('R') => vec!["R"],
        _ => vec!["L", "R"],
    };
    let mut any = false;
    for s in sides {
        let Some(fa) = f.bone(&format!("forearm.{s}")) else { continue };
        let mut bones = f.hand_bones(s);
        if bones.is_empty() {
            continue;
        }
        bones.push(fa);
        let cuts = vec![Cut { point: f.along(fa, 1.0 - cuff), dir: scale(f.axis(fa), -1.0), bones: vec![fa] }];
        let (mut m, mut j, mut w) = f.shell(&ShellSpec { bones, cuts, ease, thick: 0.003 * h, folds: 0.0, fold_size: h * 0.04, budget: (7000.0 * spec.detail) as usize })?;
        f.uvs(&mut m, tile);
        skin_mode(spec, Skin::Smooth, &mut w, &mut j);
        g.push(&m, j, w, mi);
        any = true;
    }
    if !any {
        return Err("gloves need forearm and hand bones".into());
    }
    Ok(())
}

// ----------------------------------------------------------------- hats

/// The head as a hat sees it.
#[derive(Clone, Copy, Debug)]
struct Head {
    /// Top of the skull, middle.
    top: V3,
    /// Crown to chin.
    hh: f32,
    /// Half width and half depth at the brow, and the middle there.
    rx: f32,
    rz: f32,
    cx: f32,
    cz: f32,
}

impl<'a> Fit<'a> {
    fn head(&self) -> Result<Head, String> {
        let hb = self.need("head")?;
        let chin = self.head_of(hb)[1];
        let crown = self.tail_of(hb)[1];
        let hh = (chin - crown).abs().max(1.0);
        let verts: Vec<V3> = self.mesh.positions.iter().enumerate().filter(|(i, p)| self.near[*i] as usize == hb && p[1] < crown + hh * 0.6).map(|(_, p)| *p).collect();
        if verts.len() < 20 {
            return Err("the head has no surface to fit a hat to".into());
        }
        let top_y = verts.iter().map(|p| p[1]).fold(f32::INFINITY, f32::min);
        let slab: Vec<&V3> = verts.iter().filter(|p| (p[1] - (top_y + hh * 0.3)).abs() < hh * 0.05).collect();
        let (mut lo, mut hi) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
        for p in &slab {
            for i in 0..3 {
                lo[i] = lo[i].min(p[i]);
                hi[i] = hi[i].max(p[i]);
            }
        }
        if slab.is_empty() {
            return Err("the head has no surface to fit a hat to".into());
        }
        let cx = (lo[0] + hi[0]) / 2.0;
        let cz = (lo[2] + hi[2]) / 2.0;
        Ok(Head { top: [cx, top_y, cz], hh, rx: (hi[0] - lo[0]) / 2.0, rz: (hi[2] - lo[2]) / 2.0, cx, cz })
    }
}

/// A surface of revolution over an elliptical head: `profile` is
/// `(radius in head widths, height in head heights above the crown line)`;
/// up is negative (y grows down).
fn hat_lathe(hd: &Head, profile: &[(f32, f32)], seg: usize, dip: f32) -> Mesh {
    let n = profile.len();
    loft::surface(
        seg,
        n,
        true,
        |u, v| {
            let k = ((v * (n - 1) as f32).round() as usize).min(n - 1);
            let (r, y) = profile[k];
            let a = u * std::f32::consts::TAU;
            // A brim dips at the front (-z) and back, rises at the sides.
            let d = dip * (2.0 * a).cos() * r.max(0.0);
            [hd.cx + hd.rx * r * a.cos(), hd.top[1] + y * hd.hh + d * hd.hh * if r > 1.0 { 1.0 } else { 0.0 }, hd.cz + hd.rz * r * a.sin()]
        },
        |p| [p[0] - hd.cx, 0.0, p[2] - hd.cz],
    )
}

fn hat(f: &Fit, spec: &Spec, g: &mut Garment) -> Result<(), String> {
    let hd = f.head()?;
    let hb = f.need("head")?;
    let style = {
        let s = lc(&spec.style);
        if s.is_empty() { "fedora".to_string() } else { s }
    };
    let k = spec.scale.clamp(0.5, 2.0);
    let (mat, col): (&str, [u8; 3]) = match style.as_str() {
        "helmet" | "helm" | "hardhat" => ("steel", [0, 0, 0]),
        "beanie" => ("knit", [160, 50, 50]),
        "cap" | "baseball" => ("cotton", [50, 70, 120]),
        "beret" => ("wool", [120, 30, 40]),
        "cowboy" => ("suede", [140, 100, 60]),
        "crown" => ("gold", [0, 0, 0]),
        "top_hat" | "tophat" | "bowler" => ("wool", [25, 25, 28]),
        "hood" | "cowl" => ("wool", [60, 60, 70]),
        _ => ("wool", [110, 90, 70]),
    };
    let (fab, tile) = cloth(f, spec, &g.name.clone(), mat, col);
    let mi = g.material(fab);
    let band_m = g.material(trim_material(spec, &g.name.clone(), "band", "cotton", spec.colour2.unwrap_or(match style.as_str() {
        "fedora" => [30, 28, 28],
        "cowboy" => [60, 40, 30],
        _ => [200, 190, 170],
    })));
    // Heights: 0 is the crown line (top of the skull), 1 the chin; the band sits
    // about a third of the way down, above the brows and ears.
    let band_y = 0.3;
    let seg = 64usize;
    let mut parts: Vec<(Mesh, u32)> = Vec::new();
    match style.as_str() {
        "fedora" | "trilby" | "homburg" => {
            let up = -0.14;
            // Crown: pinched at the top, dented along the middle.
            let crown = [(0.0, up + 0.07), (0.25, up + 0.045), (0.55, up + 0.04), (0.8, up + 0.01), (1.0, up + 0.09), (1.1, band_y - 0.4), (1.1, band_y - 0.12), (1.12, band_y)];
            let crown: Vec<(f32, f32)> = crown.iter().map(|&(r, y)| (r * 1.0 + if r > 1.0 { 0.04 } else { 0.0 }, y)).collect();
            parts.push((hat_lathe(&hd, &crown, seg, 0.0), mi));
            // Brim: out and slightly up at the edge, dipping front and back.
            let brim = [(1.12, band_y), (1.45, band_y + 0.012), (1.8, band_y + 0.0), (2.0, band_y - 0.03), (2.0, band_y - 0.02), (1.8, band_y + 0.015), (1.4, band_y + 0.025), (1.12, band_y + 0.02)];
            parts.push((hat_lathe(&hd, &brim, seg, 0.04), mi));
            let band = [(1.1, band_y - 0.13), (1.13, band_y - 0.13), (1.13, band_y - 0.005), (1.1, band_y - 0.005)];
            let mut b = hat_lathe(&hd, &band, seg, 0.0);
            // Back to closed.
            let _ = &mut b;
            parts.push((b, band_m));
        }
        "top_hat" | "tophat" => {
            let crown = [(0.0, -0.58), (0.9, -0.58), (1.04, -0.55), (1.06, -0.2), (1.1, band_y - 0.03), (1.1, band_y)];
            parts.push((hat_lathe(&hd, &crown, seg, 0.0), mi));
            parts.push((hat_lathe(&hd, &[(1.1, band_y), (1.7, band_y - 0.01), (1.72, band_y - 0.03), (1.7, band_y + 0.01), (1.1, band_y + 0.02)], seg, 0.0), mi));
            parts.push((hat_lathe(&hd, &[(1.07, band_y - 0.13), (1.1, band_y - 0.13), (1.1, band_y - 0.01), (1.07, band_y - 0.01)], seg, 0.0), band_m));
        }
        "bowler" => {
            let crown = [(0.0, -0.3), (0.55, -0.28), (0.9, -0.2), (1.08, -0.05), (1.1, band_y - 0.03), (1.1, band_y)];
            parts.push((hat_lathe(&hd, &crown, seg, 0.0), mi));
            parts.push((hat_lathe(&hd, &[(1.1, band_y), (1.4, band_y - 0.01), (1.45, band_y - 0.04), (1.4, band_y + 0.0), (1.1, band_y + 0.02)], seg, 0.0), mi));
        }
        "cowboy" => {
            let crown = [(0.0, -0.22), (0.35, -0.3), (0.7, -0.27), (1.0, -0.12), (1.1, band_y - 0.03), (1.1, band_y)];
            parts.push((hat_lathe(&hd, &crown, seg, 0.0), mi));
            parts.push((hat_lathe(&hd, &[(1.1, band_y), (1.7, band_y + 0.01), (2.15, band_y - 0.03), (2.2, band_y - 0.1), (2.15, band_y - 0.05), (1.7, band_y + 0.03), (1.1, band_y + 0.03)], seg, 0.1), mi));
            parts.push((hat_lathe(&hd, &[(1.07, band_y - 0.1), (1.12, band_y - 0.1), (1.12, band_y - 0.01), (1.07, band_y - 0.01)], seg, 0.0), band_m));
        }
        "cap" | "baseball" => {
            let dome = [(0.0, -0.1), (0.5, -0.09), (0.85, -0.04), (1.05, 0.05), (1.08, band_y - 0.02), (1.08, band_y)];
            parts.push((hat_lathe(&hd, &dome, seg, 0.0), mi));
            // The peak: a curved plate at the front (-z).
            let peak = loft::surface(
                10,
                6,
                false,
                |u, v| {
                    let a = -std::f32::consts::FRAC_PI_2 + (u - 0.5) * 1.5;
                    let r = hd.rx * (1.08 + 1.0 * v * (1.0 - 0.25 * (u - 0.5).abs() * 2.0));
                    [hd.cx + r * a.cos(), hd.top[1] + (band_y - 0.01 + 0.07 * v) * hd.hh, hd.cz + hd.rz * (r / hd.rx) * a.sin()]
                },
                |_| [0.0, -1.0, 0.0],
            );
            let (j, w) = f.rigid_weights(peak.positions.len(), hb);
            let (peak, _j, _w) = add_rim(peak, j, w, 0.004 * f.h);
            parts.push((peak, mi));
        }
        "beanie" => {
            let dome = [(0.0, -0.12), (0.45, -0.115), (0.8, -0.06), (1.02, 0.05), (1.08, 0.2), (1.1, band_y + 0.02), (1.14, band_y + 0.02), (1.14, band_y - 0.1), (1.08, band_y - 0.1)];
            parts.push((hat_lathe(&hd, &dome, seg, 0.0), mi));
        }
        "beret" => {
            let dome = [(0.0, -0.04), (0.5, -0.045), (1.0, -0.03), (1.45, 0.05), (1.5, 0.12), (1.3, 0.18), (1.05, 0.2), (1.02, 0.18)];
            let mut b = hat_lathe(&hd, &dome, seg, 0.0);
            // Slumped to one side.
            loft::deform(&mut b, |p| [p[0] + (p[1] - hd.top[1]) * 0.0 - 0.0, p[1], p[2]]);
            parts.push((b, mi));
        }
        "helmet" | "helm" | "bascinet" => {
            // A dome down to the neck at the back and sides, open at the face.
            let dome = [(0.0, -0.14), (0.4, -0.13), (0.8, -0.08), (1.0, 0.04), (1.12, 0.3), (1.14, 0.55), (1.1, 0.75), (1.0, 0.95)];
            let mut m = hat_lathe(&hd, &dome, seg, 0.0);
            // The face opening: no surface in the front, below the brow.
            let keep: Vec<u32> = m
                .indices
                .chunks(3)
                .filter(|t| {
                    let c = scale(add(add(m.positions[t[0] as usize], m.positions[t[1] as usize]), m.positions[t[2] as usize]), 1.0 / 3.0);
                    let a = (c[2] - hd.cz).atan2(c[0] - hd.cx);
                    let front = (a + std::f32::consts::FRAC_PI_2).abs();
                    let y = (c[1] - hd.top[1]) / hd.hh;
                    !(front < 0.8 && y > 0.22)
                })
                .flatten()
                .copied()
                .collect();
            m.indices = keep;
            let mats = vec![0u32; m.triangles()];
            m.regroup(&mats);
            parts.push((m, mi));
            // A ridge along the top, front to back.
            let ridge: Vec<V3> = (0..16).map(|i| {
                let a = -std::f32::consts::FRAC_PI_2 + std::f32::consts::PI * i as f32 / 15.0 * 2.0 * 0.5 * 2.0;
                let _ = a;
                let t = i as f32 / 15.0;
                let z = hd.cz - hd.rz * 1.1 * (1.0 - 2.0 * t);
                let y = hd.top[1] - hd.hh * 0.17 * (std::f32::consts::PI * t).sin() + hd.hh * 0.0;
                [hd.cx, y, z]
            }).collect();
            parts.push((loft::tube(&ridge, &vec![hd.rx * 0.07; 16], 8), mi));
        }
        "hardhat" => {
            let dome = [(0.0, -0.2), (0.5, -0.19), (0.85, -0.12), (1.05, 0.0), (1.12, band_y - 0.02), (1.12, band_y)];
            parts.push((hat_lathe(&hd, &dome, seg, 0.0), mi));
            parts.push((hat_lathe(&hd, &[(1.12, band_y), (1.4, band_y + 0.01), (1.45, band_y), (1.1, band_y + 0.04)], seg, 0.0), mi));
        }
        "crown" => {
            // A band with points.
            let m = loft::surface(
                96,
                4,
                true,
                |u, v| {
                    let a = u * std::f32::consts::TAU;
                    let spikes = (a * 5.0).cos().max(0.0).powf(0.7) * 0.2;
                    let y = if v > 0.5 { -0.1 } else { -0.2 - spikes };
                    let r = 1.12 + if v == 0.0 || v == 1.0 { 0.0 } else { 0.0 } + 0.02 * ((v * 3.0).round() % 2.0);
                    [hd.cx + hd.rx * r * a.cos(), hd.top[1] + (y + 0.12 + 0.05 * (v * 3.0).round()) * hd.hh, hd.cz + hd.rz * r * a.sin()]
                },
                |p| [p[0] - hd.cx, 0.0, p[2] - hd.cz],
            );
            parts.push((m, mi));
        }
        "hood" | "cowl" => {
            // A shell over the head with the face left open.
            return hood(f, spec, g, mi, tile);
        }
        other => return Err(format!("hat style \"{other}\": fedora, top hat, bowler, cowboy, cap, beanie, beret, helmet, hardhat, crown, hood")),
    }
    // Scale about the head's middle, then bring it down so it rests on the skull.
    for (mut m, mat) in parts {
        if (k - 1.0).abs() > 1e-3 {
            loft::deform(&mut m, |p| [hd.cx + (p[0] - hd.cx) * k, hd.top[1] + (p[1] - hd.top[1]) * k, hd.cz + (p[2] - hd.cz) * k]);
        }
        loft::deform(&mut m, |p| p);
        let (mut j, mut w) = f.rigid_weights(m.positions.len(), hb);
        skin_mode(spec, Skin::Rigid, &mut w, &mut j);
        let (m, j, w) = add_rim(m, j, w, 0.004 * f.h);
        let mut m = m;
        f.uvs(&mut m, tile);
        g.push(&m, j, w, mat);
    }
    Ok(())
}

/// A hood: the head's surface lifted off it, open at the face, with a
/// soft fall over the shoulders.
fn hood(f: &Fit, spec: &Spec, g: &mut Garment, mi: u32, tile: f32) -> Result<(), String> {
    let hd = f.head()?;
    let hb = f.need("head")?;
    let neck = f.need("neck")?;
    let h = f.h;
    let cuts = vec![Cut { point: [hd.cx, hd.top[1] + hd.hh * 0.62, hd.cz - hd.rz * 0.2], dir: [0.0, -1.0, -0.0], bones: vec![] }];
    let (mut m, mut j, mut w) = f.shell(&ShellSpec { bones: vec![hb, neck], cuts, ease: 0.012 * h * spec.fit, thick: 0.005 * h, folds: 0.6, fold_size: h * 0.04, budget: 6000 })?;
    // The face: nothing in front of the brow line.
    let keep: Vec<u32> = m
        .indices
        .chunks(3)
        .filter(|t| {
            let c = scale(add(add(m.positions[t[0] as usize], m.positions[t[1] as usize]), m.positions[t[2] as usize]), 1.0 / 3.0);
            !(c[2] < hd.cz - hd.rz * 0.35 && (c[0] - hd.cx).abs() < hd.rx * 0.95 && c[1] > hd.top[1] + hd.hh * 0.18)
        })
        .flatten()
        .copied()
        .collect();
    m.indices = keep;
    let mats = vec![0u32; m.triangles()];
    m.regroup(&mats);
    f.uvs(&mut m, tile);
    skin_mode(spec, Skin::Smooth, &mut w, &mut j);
    g.push(&m, j, w, mi);
    Ok(())
}

// -------------------------------------------------------------- glasses

fn glasses(f: &Fit, spec: &Spec, g: &mut Garment) -> Result<(), String> {
    let hd = f.head()?;
    let hb = f.need("head")?;
    let style = lc(&spec.style);
    let k = spec.scale.clamp(0.6, 1.6);
    // Where the eyes are: their bones if there are any, else the head's measure.
    let (el, er) = match (f.bone("eye.L"), f.bone("eye.R")) {
        (Some(a), Some(b)) => (f.head_of(a), f.head_of(b)),
        _ => {
            let y = hd.top[1] + 0.46 * hd.hh;
            ([hd.cx + hd.rx * 0.38, y, hd.cz - hd.rz * 0.8], [hd.cx - hd.rx * 0.38, y, hd.cz - hd.rz * 0.8])
        }
    };
    let gap = length(sub(el, er));
    let z = el[2].min(er[2]) - 0.19 * gap;
    let r_lens = 0.4 * gap * k;
    let rim = 0.028 * gap * k;
    let frame_col = spec.colour.unwrap_or(match style.as_str() {
        "aviator" => [200, 170, 90],
        "sunglasses" | "shades" => [20, 20, 22],
        _ => [30, 30, 34],
    });
    let metal = matches!(style.as_str(), "aviator" | "round" | "monocle");
    let frame = g.material(if metal {
        fabric::material("brass", Some(frame_col), None, Pattern::None, 8).unwrap_or_default()
    } else {
        fabric::material("plastic", Some(frame_col), None, Pattern::None, 8).unwrap_or_default()
    });
    g.mesh.materials[frame as usize].name = format!("{} frame", g.name);
    let dark = matches!(style.as_str(), "sunglasses" | "shades" | "aviator");
    let lens = g.material(Material {
        name: format!("{} lens", g.name),
        base_colour: if dark { [0.02, 0.02, 0.025, 1.0] } else { [0.9, 0.95, 1.0, 1.0] },
        roughness: 0.05,
        transmission: if dark { 0.25 } else { 0.92 },
        ..Material::default()
    });
    let eye_dir = |s: f32| s;
    let sides = [(el, 1.0f32), (er, -1.0)];
    let mut pieces: Vec<(Mesh, u32)> = Vec::new();
    let circle = |c: V3, rx: f32, ry: f32, n: usize| -> Vec<V3> { (0..=n).map(|i| { let a = i as f32 / n as f32 * std::f32::consts::TAU; [c[0] + rx * a.cos(), c[1] + ry * a.sin(), z] }).collect() };
    for (e, s) in sides {
        let _ = eye_dir(s);
        let (rx, ry) = match style.as_str() {
            "square" | "rectangular" => (r_lens * 1.05, r_lens * 0.8),
            "aviator" => (r_lens * 1.0, r_lens * 1.1),
            "sunglasses" | "shades" => (r_lens * 1.1, r_lens * 0.9),
            _ => (r_lens, r_lens),
        };
        let c = [e[0], e[1], z];
        let ring = circle(c, rx, ry, 40);
        pieces.push((loft::tube(&ring, &vec![rim; ring.len()], 8), frame));
        // The lens: a flat disc.
        let disc = loft::surface(24, 3, true, |u, v| { let a = u * std::f32::consts::TAU; [c[0] + rx * v.min(1.0) * a.cos(), c[1] + ry * v * a.sin(), z] }, |_| [0.0, 0.0, -1.0]);
        pieces.push((disc, lens));
        // The temple: from the outer edge back along the head to the ear.
        let out = [c[0] + s * rx, c[1], z];
        let ear = [hd.cx + s * (hd.rx * 1.02), c[1] + 0.05 * hd.hh, hd.cz + hd.rz * 0.35];
        pieces.push((loft::tube(&[out, lerp(out, ear, 0.5), ear], &[rim * 0.8; 3], 6), frame));
    }
    // The bridge across the nose.
    let mid = lerp(el, er, 0.5);
    let bridge = [[el[0] - (el[0] - er[0]).signum() * 0.0 + (er[0] - el[0]) * 0.5 + (el[0] - er[0]) * 0.0, 0.0, 0.0]; 0];
    let _ = bridge;
    let b0 = [el[0] - r_lens, mid[1] - 0.1 * r_lens, z];
    let b1 = [er[0] + r_lens, mid[1] - 0.1 * r_lens, z];
    let bump = [mid[0], mid[1] - 0.2 * r_lens, z - 0.02 * gap];
    pieces.push((loft::tube(&[b0, lerp(b0, bump, 0.5), bump, lerp(bump, b1, 0.5), b1], &[rim * 0.8; 5], 6), frame));
    for (mut m, mat) in pieces {
        loft::deform(&mut m, |p| p);
        let (mut j, mut w) = f.rigid_weights(m.positions.len(), hb);
        skin_mode(spec, Skin::Rigid, &mut w, &mut j);
        g.push(&m, j, w, mat);
    }
    Ok(())
}

// --------------------------------------------------------------- scarf

fn scarf(f: &Fit, spec: &Spec, g: &mut Garment) -> Result<(), String> {
    let mk = f.marks()?;
    let h = f.h;
    let style = lc(&spec.style);
    let neck = f.need("neck")?;
    let chest = f.need("chest")?;
    let (fab, tile) = cloth(f, spec, &g.name.clone(), "knit", [150, 40, 40]);
    let mi = g.material(fab);
    let y = f.along(neck, 0.45)[1];
    let axis = [mk.cx, 0.0, f.centre_at(y, &[neck])[2]];
    let r0 = f.core_radius(axis, y).max(0.02 * h);
    let bow = style == "bow_tie" || style == "bowtie";
    let tie = style == "tie" || style == "necktie" || style == "cravat";
    let wide = if tie || bow { 0.018 * h } else { 0.055 * h };
    let mut wrap = loft::band(axis, y, wide * spec.scale, 0.02 * h * spec.fit.clamp(0.5, 2.0), |a| r0 + 0.012 * h * (1.0 + 0.2 * (a * 3.0).sin()), 48);
    f.uvs(&mut wrap, tile);
    let (mut j, mut w) = f.weights_for(&wrap.positions);
    skin_mode(spec, Skin::Smooth, &mut w, &mut j);
    g.push(&wrap, j, w, mi);
    // A tail down the front.
    let front_z = |yy: f32| f.body_reach(axis, yy, -std::f32::consts::FRAC_PI_2).map_or(axis[2] - 0.1 * h, |r| axis[2] - r);
    let len = if tie { 0.2 * h } else if bow { 0.0 } else { 0.24 * h };
    if len > 0.0 {
        let wtail = if tie { 0.03 * h } else { 0.055 * h };
        let x0 = mk.cx + if tie { 0.0 } else { 0.03 * h };
        let rows = 12usize;
        let mut tail = loft::surface(
            4,
            rows,
            false,
            |u, v| {
                let yy = y + 0.02 * h + len * v;
                let taper = if tie { 0.45 + 0.9 * (v * 3.0).min(1.0) * (1.0 - 0.1 * v) } else { 1.0 };
                let x = x0 + (u - 0.5) * wtail * taper;
                [x, yy, front_z(yy) - 0.016 * h - 0.003 * h * (u * std::f32::consts::TAU).sin()]
            },
            |_| [0.0, 0.0, -1.0],
        );
        f.uvs(&mut tail, tile);
        let (mut j, mut w) = f.weights_for(&tail.positions);
        skin_mode(spec, Skin::Smooth, &mut w, &mut j);
        let (m, j, w) = add_rim(tail, j, w, 0.006 * h);
        g.push(&m, j, w, mi);
    }
    let _ = chest;
    Ok(())
}

// ---------------------------------------------------------------- belt

fn belt(f: &Fit, spec: &Spec, g: &mut Garment) -> Result<(), String> {
    let mk = f.marks()?;
    let h = f.h;
    let (fab, tile) = cloth(f, spec, &g.name.clone(), "leather", [70, 45, 30]);
    let mi = g.material(fab);
    let spine = f.need("spine")?;
    let y = lerp([0.0, mk.waist, 0.0], [0.0, mk.crotch, 0.0], 0.3)[1];
    let axis = [mk.cx, 0.0, f.centre_at(y, &[spine, f.need("hips")?])[2]];
    let mut band = loft::band(axis, y, 0.034 * h * spec.scale, 0.006 * h, |a| f.body_reach(axis, y, a).unwrap_or(0.1 * h) + 0.004 * h * spec.fit, 64);
    f.uvs(&mut band, tile);
    let (mut j, mut w) = f.weights_for(&band.positions);
    skin_mode(spec, Skin::Smooth, &mut w, &mut j);
    g.push(&band, j, w, mi);
    let r = f.body_reach(axis, y, -std::f32::consts::FRAC_PI_2).unwrap_or(0.1 * h);
    let mut buckle = crate::prim::cube([0.036 * h, 0.04 * h, 0.007 * h]);
    for q in &mut buckle.positions {
        *q = add(*q, [axis[0], y, axis[2] - r - 0.012 * h]);
    }
    let metal = g.material(trim_material(&Spec { colour2: spec.colour2.or(Some([200, 170, 90])), ..spec.clone() }, &g.name.clone(), "buckle", "brass", [200, 170, 90]));
    let (mut j, mut w) = f.rigid_weights(buckle.positions.len(), spine);
    skin_mode(&Spec { skin: Skin::Rigid, ..spec.clone() }, Skin::Rigid, &mut w, &mut j);
    g.push(&buckle, j, w, metal);
    Ok(())
}

// ----------------------------------------------------------------- bags

fn bag(f: &Fit, spec: &Spec, g: &mut Garment) -> Result<(), String> {
    let mk = f.marks()?;
    let h = f.h;
    let style = lc(&spec.style);
    let chest = f.need("chest")?;
    let spine = f.need("spine")?;
    let (mat, col): (&str, [u8; 3]) = match style.as_str() {
        "satchel" | "messenger_bag" | "purse" => ("leather", [110, 70, 40]),
        "quiver" => ("leather", [90, 60, 40]),
        _ => ("canvas", [70, 90, 60]),
    };
    let (fab, tile) = cloth(f, spec, &g.name.clone(), mat, col);
    let mi = g.material(fab);
    let strap_m = g.material(trim_material(spec, &g.name.clone(), "strap", "leather", shade(spec.colour.unwrap_or(col), 0.7)));
    let c = f.centre_at(mk.waist - 0.08 * h, &[chest, spine]);
    let back = f.body_reach([mk.cx, 0.0, c[2]], c[1], std::f32::consts::FRAC_PI_2).unwrap_or(0.08 * h);
    let k = spec.scale.clamp(0.5, 2.0);
    let (size, at, rigid_to) = match style.as_str() {
        "satchel" | "messenger_bag" | "purse" => ([0.14 * h * k, 0.1 * h * k, 0.045 * h * k], [mk.cx - 0.1 * h, mk.crotch - 0.02 * h, c[2]], spine),
        "quiver" => ([0.05 * h * k, 0.22 * h * k, 0.05 * h * k], [mk.cx + 0.02 * h, mk.waist - 0.1 * h, c[2] + back + 0.03 * h], chest),
        _ => ([0.17 * h * k, 0.22 * h * k, 0.075 * h * k], [mk.cx, mk.waist - 0.07 * h, c[2] + back + 0.045 * h * k + 0.01 * h], chest),
    };
    // A soft rounded box, melted from one rounded-box part.
    let mut part = crate::sdf::Part::new(crate::sdf::Shape::Box, at, size);
    part.round = size[2].min(size[0]).min(size[1]) * 0.35;
    let mut m = crate::sdf::build(&[part], vec![Material::default()], (h / 260.0).max(0.4));
    f.uvs(&mut m, tile);
    let (mut j, mut w) = f.rigid_weights(m.positions.len(), rigid_to);
    skin_mode(spec, Skin::Rigid, &mut w, &mut j);
    g.push(&m, j, w, mi);
    // Straps over the shoulders (backpack) or across the chest (satchel).
    let shoulder = |s: f32| [mk.cx + s * 0.075 * h, mk.shoulder - 0.005 * h, c[2]];
    let sh_z = |p: V3| f.body_reach([mk.cx, 0.0, c[2]], p[1], std::f32::consts::FRAC_PI_2).unwrap_or(0.07 * h);
    let _ = sh_z;
    for s in [1.0f32, -1.0] {
        let path: Vec<V3> = if matches!(style.as_str(), "satchel" | "messenger_bag" | "purse") {
            if s < 0.0 {
                continue;
            }
            // From the bag, across the front of the chest to the opposite shoulder.
            let a = [at[0], at[1] - size[1] * 0.4, at[2] - size[2] * 0.4];
            let b = [mk.cx + 0.09 * h, mk.shoulder + 0.02 * h, c[2] - f.body_reach([mk.cx, 0.0, c[2]], mk.shoulder + 0.02 * h, -std::f32::consts::FRAC_PI_2).unwrap_or(0.07 * h) - 0.01 * h];
            (0..12).map(|i| { let t = i as f32 / 11.0; let p = lerp(a, b, t); let z = c[2] - f.body_reach([mk.cx, 0.0, c[2]], p[1], -std::f32::consts::FRAC_PI_2).unwrap_or(0.07 * h) - 0.012 * h; [p[0], p[1], p[2].min(z + 0.0).min(z)] }).collect()
        } else if style == "quiver" {
            continue;
        } else {
            // Over the shoulder and down the front to under the arm.
            let top = [shoulder(s)[0], mk.shoulder + 0.0 * h, c[2]];
            let back_bot = [top[0], at[1] - size[1] * 0.35, at[2] - size[2] * 0.5];
            let front = [top[0] * 0.9 + mk.cx * 0.1, mk.waist - 0.02 * h, c[2] - f.body_reach([mk.cx, 0.0, c[2]], mk.waist, -std::f32::consts::FRAC_PI_2).unwrap_or(0.07 * h) - 0.01 * h];
            vec![back_bot, lerp(back_bot, top, 0.6), top, lerp(top, front, 0.35), lerp(top, front, 0.7), front]
        };
        let strap = loft::tube(&path, &vec![0.011 * h; path.len()], 6);
        let (j, w) = f.weights_for(&strap.positions);
        g.push(&strap, j, w, strap_m);
    }
    Ok(())
}

// --------------------------------------------------------------- armour

fn armour(f: &Fit, spec: &Spec) -> Result<Vec<Garment>, String> {
    let style = lc(&spec.style);
    let h = f.h;
    let mk = f.marks()?;
    let base_name = spec.name.clone().unwrap_or_else(|| "Armour".into());
    let want = |piece: &str| -> bool {
        match style.as_str() {
            "" | "full" | "plate" | "knight" | "set" | "armour" | "armor" => true,
            s => s == piece || s == format!("{piece}s") || (piece == "breastplate" && matches!(s, "cuirass" | "chest")) || (piece == "greave" && s == "greaves") || (piece == "bracer" && s == "bracers") || (piece == "pauldron" && s == "pauldrons"),
        }
    };
    let steel_spec = Spec { material: spec.material.clone().or(Some("steel".into())), skin: Skin::Rigid, ..spec.clone() };
    let mut out = Vec::new();
    let ease = 0.016 * h * spec.fit.clamp(0.5, 2.0);
    let thick = 0.006 * h;
    let mut make = |name: &str, bones: Vec<usize>, cuts: Vec<Cut>, budget: usize| -> Result<(), String> {
        if bones.is_empty() {
            return Ok(());
        }
        let mut g = Garment::new(&format!("{base_name} {name}"), "armour");
        let (fab, tile) = cloth(f, &steel_spec, &g.name.clone(), "steel", [0, 0, 0]);
        let mi = g.material(fab);
        let (mut m, mut j, mut w) = f.shell(&ShellSpec { bones, cuts, ease, thick, folds: 0.0, fold_size: h * 0.05, budget: (budget as f32 * spec.detail) as usize })?;
        f.uvs(&mut m, tile);
        skin_mode(&steel_spec, Skin::Rigid, &mut w, &mut j);
        g.push(&m, j, w, mi);
        finish(f, spec, &mut g);
        out.push(g);
        Ok(())
    };
    let torso: Vec<usize> = ["hips", "spine", "chest"].iter().filter_map(|n| f.bone(n)).collect();
    if want("breastplate") && !torso.is_empty() {
        let mut bones = torso.clone();
        bones.extend(f.both("shoulder"));
        let cuts = vec![
            Cut { point: [0.0, mk.crotch - 0.07 * h, 0.0], dir: [0.0, 1.0, 0.0], bones: vec![] },
            Cut { point: [0.0, mk.neck + 0.008 * h, 0.0], dir: [0.0, -1.0, 0.0], bones: vec![] },
        ];
        make("breastplate", bones, cuts, 9000)?;
    }
    if want("pauldron") {
        for s in ["L", "R"] {
            if let (Some(ua), Some(sh)) = (f.bone(&format!("upper_arm.{s}")), f.bone(&format!("shoulder.{s}"))) {
                let cuts = vec![Cut { point: f.along(ua, 0.55), dir: f.axis(ua), bones: vec![ua] }];
                make(&format!("pauldron {s}"), vec![ua, sh], cuts, 4000)?;
            }
        }
    }
    if want("bracer") {
        for s in ["L", "R"] {
            if let Some(fa) = f.bone(&format!("forearm.{s}")) {
                let cuts = vec![Cut { point: f.along(fa, 0.15), dir: scale(f.axis(fa), -1.0), bones: vec![fa] }, Cut { point: f.along(fa, 0.97), dir: f.axis(fa), bones: vec![fa] }];
                make(&format!("bracer {s}"), vec![fa], cuts, 3000)?;
            }
        }
    }
    if want("greave") {
        for s in ["L", "R"] {
            if let Some(sh) = f.bone(&format!("shin.{s}")) {
                let cuts = vec![Cut { point: f.along(sh, 0.08), dir: scale(f.axis(sh), -1.0), bones: vec![sh] }, Cut { point: f.along(sh, 0.92), dir: f.axis(sh), bones: vec![sh] }];
                make(&format!("greave {s}"), vec![sh], cuts, 3500)?;
            }
        }
    }
    if want("cuisse") && style != "" && style != "full" {
        for s in ["L", "R"] {
            if let Some(th) = f.bone(&format!("thigh.{s}")) {
                let cuts = vec![Cut { point: f.along(th, 0.1), dir: scale(f.axis(th), -1.0), bones: vec![th] }, Cut { point: f.along(th, 0.95), dir: f.axis(th), bones: vec![th] }];
                make(&format!("cuisse {s}"), vec![th], cuts, 3500)?;
            }
        }
    }
    if matches!(style.as_str(), "" | "full" | "knight" | "set" | "helmet") {
        let mut hs = Spec { kind: "hat".into(), style: "helmet".into(), material: Some("steel".into()), skin: Skin::Rigid, name: Some(format!("{base_name} helmet")), ..spec.clone() };
        hs.material = spec.material.clone().or(Some("steel".into()));
        if let Ok(mut v) = fit_one(f, &hs) {
            out.append(&mut v);
        }
    }
    if matches!(style.as_str(), "" | "full" | "knight" | "set" | "gauntlets" | "gauntlet") {
        let gs = Spec { kind: "gloves".into(), style: "gauntlets".into(), material: spec.material.clone().or(Some("steel".into())), skin: Skin::Smooth, name: Some(format!("{base_name} gauntlets")), ..spec.clone() };
        if let Ok(mut v) = fit_one(f, &gs) {
            out.append(&mut v);
        }
    }
    if out.is_empty() {
        return Err("armour: pieces are breastplate, pauldrons, bracers, greaves, gauntlets, helmet (style \"full\" is the set)".into());
    }
    Ok(out)
}

/// Fits one garment with an already measured body.
fn fit_one(f: &Fit, spec: &Spec) -> Result<Vec<Garment>, String> {
    let kind = canonical_kind(&spec.kind).ok_or("kind")?;
    let name = spec.name.clone().unwrap_or_else(|| title(kind));
    let mut g = Garment::new(&name, kind);
    match kind {
        "hat" => hat(f, spec, &mut g)?,
        "gloves" => gloves(f, spec, &mut g)?,
        "shoes" => shoes(f, spec, &mut g)?,
        _ => return Err("kind".into()),
    }
    finish(f, spec, &mut g);
    Ok(vec![g])
}

// ------------------------------------------------------- head surfaces

/// Parts of a head's surface that hair grows on, and where the head is.
pub struct Scalp {
    /// Triangles of the body where scalp hair grows.
    pub scalp: Vec<usize>,
    /// Beard: cheeks, jaw, chin.
    pub beard: Vec<usize>,
    /// Moustache: between nose and lip.
    pub moustache: Vec<usize>,
    /// Brow ridge, each side.
    pub brows: Vec<usize>,
    /// The head base: its origin and size (see [`crate::face::HeadFrame`]).
    pub frame: crate::face::HeadFrame,
    pub head_bone: usize,
    pub neck_bone: Option<usize>,
    pub chest_bone: Option<usize>,
}

/// Finds where hair grows on a figure with a head base (see
/// [`crate::face::find_head`]): in the head's own units (x to the left, y
/// down, face towards -z, eyes at y 0, nose base about 0.22, mouth 0.29,
/// chin 0.53).
pub fn scalp(body: &Mesh) -> Result<Scalp, String> {
    let f = Fit::new(body)?;
    let frame = crate::face::find_head(&Mesh { positions: body.positions.clone(), indices: body.indices.clone(), groups: body.groups.clone(), materials: body.materials.clone(), ..Default::default() }).ok_or("hair needs a head base with a face (a figure made with model shape \"character\" or \"head\")")?;
    let head_bone = f.need("head")?;
    let s = frame.size;
    let mut out = Scalp { scalp: Vec::new(), beard: Vec::new(), moustache: Vec::new(), brows: Vec::new(), frame, head_bone, neck_bone: f.bone("neck"), chest_bone: f.bone("chest") };
    let eye_mats: Vec<u32> = body.materials.iter().enumerate().filter(|(_, m)| matches!(m.name.as_str(), "Eye white" | "Iris" | "Pupil" | "Cornea" | "Upper lid" | "Lower lid" | "Lashes" | "Teeth" | "Gums" | "Tongue")).map(|(i, _)| i as u32).collect();
    let mats = body.tri_materials();
    for t in 0..body.triangles() {
        if eye_mats.contains(&mats[t]) {
            continue;
        }
        let c = body.tri(t);
        if c.iter().filter(|&&v| f.main[v] as usize == head_bone).count() < 2 {
            continue;
        }
        let p = scale(sub(body.centroid(t), frame.origin), 1.0 / s);
        let n = body.face_normal(t);
        let front = p[2] < -0.12;
        // Hair grows: above the hairline in front, down to the nape behind.
        if ((front && p[1] < -0.3) || (!front && p[1] < if p[2] > 0.2 { 0.36 } else { 0.1 })) && n[1] < 0.6 {
            out.scalp.push(t);
        }
        if p[2] < 0.1 && p[0].abs() < 0.26 && p[1] > 0.3 && p[1] < 0.58 && !(p[1] < 0.35 && p[0].abs() < 0.11) {
            out.beard.push(t);
        } else if p[2] < 0.0 && p[0].abs() > 0.2 && p[0].abs() < 0.3 && p[1] > 0.08 && p[1] < 0.3 {
            out.beard.push(t);
        }
        if p[2] < -0.25 && p[0].abs() < 0.12 && p[1] > 0.215 && p[1] < 0.262 {
            out.moustache.push(t);
        }
        if p[2] < -0.25 && p[0].abs() > 0.03 && p[0].abs() < 0.22 && p[1] > -0.125 && p[1] < -0.055 {
            out.brows.push(t);
        }
    }
    Ok(out)
}

/// The body's surface for hair to grow on, as a mesh (positions, normals,
/// triangles).
pub fn scalp_mesh(body: &Mesh) -> Mesh {
    let mut m = Mesh { positions: body.positions.clone(), normals: body.normals.clone(), uvs: body.uvs.clone(), indices: body.indices.clone(), materials: body.materials.clone(), groups: body.groups.clone(), ..Default::default() };
    m.tidy();
    m
}

// ---------------------------------------------------------- putting on

/// Puts a garment on the model: its surface and skin weights join the
/// body's. Returns the range of new vertices. The garment's materials are
/// added after the body's.
pub fn attach(body: &mut Mesh, g: &Garment) -> Result<std::ops::Range<usize>, String> {
    let rig = body.rig.as_deref().ok_or("the model has no rig")?;
    let n0 = body.positions.len();
    if !body.rig_in_step() {
        return Err("the model's skin is out of step with its surface; repair it with {\"op\": \"rig\", \"clip\": id, \"skinning\": \"smooth\"}".into());
    }
    let mut rig = rig.clone();
    let mat_base = body.materials.len() as u32;
    let mut mats = body.tri_materials();
    mats.extend(g.mesh.tri_materials().into_iter().map(|m| m + mat_base));
    body.materials.extend(g.mesh.materials.iter().cloned());
    let base = n0 as u32;
    body.positions.extend_from_slice(&g.mesh.positions);
    body.normals.extend_from_slice(&g.mesh.normals);
    body.uvs.extend_from_slice(&g.mesh.uvs);
    body.indices.extend(g.mesh.indices.iter().map(|i| i + base));
    rig.joints.extend_from_slice(&g.joints);
    rig.weights.extend_from_slice(&g.weights);
    body.regroup(&mats);
    body.rig = Some(std::sync::Arc::new(rig));
    if g.tuck > 0.0 {
        tuck_skin(body, n0, g.tuck);
    }
    if g.kind == "hat" {
        tuck_hair(body);
    }
    Ok(n0..body.positions.len())
}

/// The axis a normal mostly faces along (0 x, 1 y, 2 z).
fn main_axis(n: V3) -> usize {
    let a = n.map(f32::abs);
    if a[0] >= a[1] && a[0] >= a[2] { 0 } else if a[1] >= a[2] { 1 } else { 2 }
}

fn project(p: V3, axis: usize) -> [f32; 2] {
    match axis {
        0 => [p[2], p[1]],
        1 => [p[0], p[2]],
        _ => [p[0], p[1]],
    }
}

/// Texture coordinates for a garment cut as panels, the way cloth is: each
/// triangle is projected along the axis its face looks along (smoothed so
/// panels are whole, not speckled), and a vertex where two panels meet is
/// split so each panel keeps its own coordinates. A per-vertex projection
/// smears a pattern across every triangle that straddles a turn; panels
/// meet at a clean seam instead. The scale is the one the garment was
/// given (by [`Fit::uvs`]).
fn panel_uvs(g: &mut Garment) {
    let m = &mut g.mesh;
    let n = m.positions.len();
    if n == 0 || m.uvs.len() != n || !m.materials.iter().any(|x| x.texture.is_some()) {
        return;
    }
    // The scale the coordinates already have.
    let mut ks: Vec<f32> = (0..n)
        .filter_map(|v| {
            let q = project(m.positions[v], main_axis(m.normals.get(v).copied().unwrap_or([0.0, 0.0, 1.0])));
            let (lq, lu) = ((q[0] * q[0] + q[1] * q[1]).sqrt(), (m.uvs[v][0].powi(2) + m.uvs[v][1].powi(2)).sqrt());
            (lq > 1.0 && lu > 1e-6).then_some(lu / lq)
        })
        .collect();
    if ks.len() < 8 {
        return;
    }
    ks.sort_by(f32::total_cmp);
    let k = ks[ks.len() / 2];
    // Each triangle's panel, then smoothed: a triangle takes the panel most
    // of its neighbours have.
    let tris = m.triangles();
    let mut axis: Vec<usize> = (0..tris).map(|t| main_axis(m.face_normal(t))).collect();
    let (ids, unique) = m.position_ids();
    let mut at: Vec<Vec<u32>> = vec![Vec::new(); unique.len()];
    for t in 0..tris {
        for v in m.tri(t) {
            at[ids[v] as usize].push(t as u32);
        }
    }
    for _ in 0..3 {
        let prev = axis.clone();
        for t in 0..tris {
            let mut votes = [0usize; 3];
            for v in m.tri(t) {
                for &o in &at[ids[v] as usize] {
                    votes[prev[o as usize]] += 1;
                }
            }
            let best = (0..3).max_by_key(|&a| votes[a]).unwrap_or(prev[t]);
            if votes[best] > votes[prev[t]] * 3 / 2 {
                axis[t] = best;
            }
        }
    }
    // Split corners shared by panels.
    let mut copy: std::collections::HashMap<(usize, usize), u32> = Default::default();
    let mut owner: Vec<Option<usize>> = vec![None; n];
    for t in 0..tris {
        for c in 0..3 {
            let v = m.indices[3 * t + c] as usize;
            let a = axis[t];
            let idx = match owner[v] {
                None => {
                    owner[v] = Some(a);
                    v as u32
                }
                Some(o) if o == a => v as u32,
                Some(_) => *copy.entry((v, a)).or_insert_with(|| {
                    m.positions.push(m.positions[v]);
                    m.normals.push(m.normals.get(v).copied().unwrap_or([0.0; 3]));
                    m.uvs.push([0.0; 2]);
                    g.joints.push(g.joints.get(v).copied().unwrap_or([0; 4]));
                    g.weights.push(g.weights.get(v).copied().unwrap_or([1.0, 0.0, 0.0, 0.0]));
                    (m.positions.len() - 1) as u32
                }),
            };
            m.indices[3 * t + c] = idx;
        }
    }
    for t in 0..tris {
        for v in m.tri(t) {
            let q = project(m.positions[v], axis[t]);
            m.uvs[v] = [q[0] * k, q[1] * k];
        }
    }
}

/// Draws the skin in by `by` px wherever the garment just put on (the
/// vertices from `from` on) lies over it, so the skin stays inside the
/// cloth when the body bends.
fn tuck_skin(body: &mut Mesh, from: usize, by: f32) {
    let Some(skin) = body.materials.iter().position(|m| m.name == "Skin") else { return };
    let skin = skin as u32;
    let mats = body.tri_materials();
    let mut on_skin = vec![false; from];
    let mut cloth = Vec::new();
    for t in 0..body.triangles() {
        let c = body.tri(t);
        if c.iter().all(|&v| v >= from) {
            cloth.push(c.map(|v| body.positions[v]));
        } else if mats[t] == skin {
            for v in c {
                if v < from {
                    on_skin[v] = true;
                }
            }
        }
    }
    if cloth.is_empty() {
        return;
    }
    let bvh = Bvh::new(cloth);
    let reach = by * 6.0 + 0.012 * (body.bounds().1[1] - body.bounds().0[1]);
    for v in 0..from {
        if !on_skin[v] {
            continue;
        }
        let (p, n) = (body.positions[v], body.normals.get(v).copied().unwrap_or([0.0; 3]));
        if dot(n, n) < 0.5 {
            continue;
        }
        // Only where the cloth is straight out from the skin (not past a hem).
        if let Some((q, _, d)) = bvh.nearest(p, reach)
            && d > 1e-4
            && dot(scale(sub(q, p), 1.0 / d), n) > 0.75
        {
            body.positions[v] = sub(p, scale(n, by));
        }
    }
}

/// Whether a material is hair (sculpted or cards) worn on the head.
pub fn is_hair_material(name: &str) -> bool {
    let n = name.trim().to_ascii_lowercase();
    ["hair", "fringe", "bangs", "ponytail", "pigtail", "twin tail", "bun", "locks", "wig"].iter().any(|w| n == *w || n.starts_with(&format!("{w} ")) || n.starts_with(&format!("{w}:")))
}

/// Where a worn hat comes down to on the head, in the head's own units (y
/// down, see [`scalp`]): the lowest point of the hard pieces riding on the
/// head bone above the brows. None when no hat is worn.
pub fn hat_line(body: &Mesh, sc: &Scalp) -> Option<f32> {
    let rig = body.rig.as_deref()?;
    let (o, s) = (sc.frame.origin, sc.frame.size);
    let mats = body.tri_materials();
    let own = |n: &str| matches!(n, "Skin" | "Lips" | "Eye white" | "Iris" | "Pupil" | "Brow" | "Cornea" | "Upper lid" | "Lower lid" | "Lashes" | "Teeth" | "Gums" | "Tongue" | "Mouth" | "Ears" | "Nose" | "Fur") || is_hair_material(n);
    let rigid_head = |v: usize| rig.joints.get(v).is_some_and(|j| j[0] as usize == sc.head_bone) && rig.weights.get(v).is_some_and(|w| w[0] > 0.98);
    let mut line: Option<f32> = None;
    let mut found = 0;
    for t in 0..body.triangles() {
        let name = body.materials.get(mats[t] as usize).map_or("", |m| m.name.as_str());
        if own(name) {
            continue;
        }
        let c = body.tri(t);
        if !c.iter().all(|&v| rigid_head(v)) {
            continue;
        }
        let p = scale(sub(body.centroid(t), o), 1.0 / s);
        // Above the brows and round the skull (glasses sit lower and in front).
        if p[1] > -0.12 || length(sub(p, [0.0, -0.13, 0.07])) > 0.9 {
            continue;
        }
        found += 1;
        line = Some(line.map_or(p[1], |l: f32| l.max(p[1])));
    }
    if found < 8 {
        return None;
    }
    line
}

/// Hair under a hat stays under it: what would come through the crown is
/// pressed down to the skull above the hat line (a soft band below it), so
/// a beanie or a fedora sits on the hair instead of being pierced by it.
/// Returns how many hair vertices moved.
pub fn tuck_hair(body: &mut Mesh) -> usize {
    let Ok(sc) = scalp(body) else { return 0 };
    let Some(line) = hat_line(body, &sc) else { return 0 };
    let (o, s) = (sc.frame.origin, sc.frame.size);
    let mats = body.tri_materials();
    let mut hair = vec![false; body.positions.len()];
    for t in 0..body.triangles() {
        if body.materials.get(mats[t] as usize).is_some_and(|m| is_hair_material(&m.name)) {
            for v in body.tri(t) {
                hair[v] = true;
            }
        }
    }
    // The skull: the head base's cranium, with a hair's breadth on it.
    let centre = [0.0, -0.13, 0.07];
    let radii = [0.355, 0.415, 0.445];
    let mut moved = 0;
    for v in 0..body.positions.len() {
        if !hair[v] {
            continue;
        }
        let p = scale(sub(body.positions[v], o), 1.0 / s);
        // Fully pressed above the line, fading out over a band below it.
        let k = 1.0 - smoothstep(line - 0.02, line + 0.05, p[1]);
        if k <= 0.0 {
            continue;
        }
        let d = sub(p, centre);
        let q = ((d[0] / radii[0]).powi(2) + (d[1] / radii[1]).powi(2) + (d[2] / radii[2]).powi(2)).sqrt();
        if q <= 1.0 {
            continue;
        }
        let onto = add(centre, scale(d, 1.0 / q));
        let to = lerp(p, onto, k);
        body.positions[v] = add(o, scale(to, s));
        moved += 1;
    }
    moved
}

/// Takes off what was put on under `name`: every material called that
/// (or starting with it and a space), and the surface with it. Returns how
/// many triangles went.
pub fn remove(body: &mut Mesh, name: &str) -> Result<usize, String> {
    let want = name.trim().to_ascii_lowercase();
    let which: Vec<u32> = body.materials.iter().enumerate().filter(|(_, m)| {
        let n = m.name.to_ascii_lowercase();
        n == want || n.starts_with(&format!("{want} "))
    }).map(|(i, _)| i as u32).collect();
    if which.is_empty() {
        let names: Vec<&str> = body.materials.iter().map(|m| m.name.as_str()).collect();
        return Err(format!("nothing called \"{name}\" is worn; the model's materials are {}", names.join(", ")));
    }
    let mats = body.tri_materials();
    let before = body.triangles();
    let mut idx = Vec::new();
    let mut keep_mats = Vec::new();
    for t in 0..before {
        if !which.contains(&mats[t]) {
            idx.extend_from_slice(&body.indices[3 * t..3 * t + 3]);
            keep_mats.push(mats[t]);
        }
    }
    body.indices = idx;
    // The taken-off materials go too; the others are renumbered.
    let mut new_of = vec![0u32; body.materials.len()];
    let mut kept = Vec::new();
    for (i, m) in body.materials.iter().enumerate() {
        if !which.contains(&(i as u32)) {
            new_of[i] = kept.len() as u32;
            kept.push(m.clone());
        }
    }
    body.materials = kept;
    let keep_mats: Vec<u32> = keep_mats.iter().map(|&m| new_of[m as usize]).collect();
    body.regroup(&keep_mats);
    // Vertices nothing uses any more go, with their skin weights.
    let mut rig = body.rig.as_deref().cloned();
    let mut used = vec![false; body.positions.len()];
    for &i in &body.indices {
        used[i as usize] = true;
    }
    let mut map = vec![u32::MAX; body.positions.len()];
    let (mut pos, mut nor, mut uv) = (Vec::new(), Vec::new(), Vec::new());
    let (mut jj, mut ww) = (Vec::new(), Vec::new());
    for v in 0..used.len() {
        if !used[v] {
            continue;
        }
        map[v] = pos.len() as u32;
        pos.push(body.positions[v]);
        nor.push(body.normals.get(v).copied().unwrap_or([0.0; 3]));
        uv.push(body.uvs.get(v).copied().unwrap_or([0.0; 2]));
        if let Some(r) = &rig {
            jj.push(r.joints.get(v).copied().unwrap_or([0; 4]));
            ww.push(r.weights.get(v).copied().unwrap_or([1.0, 0.0, 0.0, 0.0]));
        }
    }
    for i in &mut body.indices {
        *i = map[*i as usize];
    }
    if let Some(r) = rig.as_mut() {
        // Shape keys follow their vertices.
        for k in &mut r.shapes {
            let (mut idx, mut d) = (Vec::new(), Vec::new());
            for (i, delta) in k.indices.iter().zip(&k.deltas) {
                if let Some(&n) = map.get(*i as usize).filter(|n| **n != u32::MAX) {
                    idx.push(n);
                    d.push(*delta);
                }
            }
            k.indices = idx;
            k.deltas = d;
        }
        if r.is_skinned() {
            r.joints = jj;
            r.weights = ww;
        }
    }
    body.positions = pos;
    body.normals = nor;
    body.uvs = uv;
    body.rig = rig.map(std::sync::Arc::new);
    Ok(before - body.triangles())
}

/// What is worn: the material names that are not the body's own (the
/// first material, and the plain clothes painted on at birth).
pub fn worn(body: &Mesh) -> Vec<String> {
    body.materials.iter().map(|m| m.name.clone()).filter(|n| !matches!(n.as_str(), "Skin" | "Lips" | "Eye white" | "Iris" | "Pupil" | "Brow")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::character::{Build, character};
    use crate::rig::PoseInput;
    use std::collections::HashMap;

    fn bare(preset: &str) -> Mesh {
        let b = Build::preset(preset).unwrap();
        character(260.0, &Build { detail: 1, dressed: false, ..b })
    }

    fn spec(kind: &str, style: &str) -> Spec {
        Spec { kind: kind.into(), style: style.into(), detail: 0.35, ..Spec::default() }
    }

    fn dress(body: &Mesh, items: &[(&str, &str)]) -> (Mesh, Vec<Garment>) {
        let mut m = body.clone();
        let mut made = Vec::new();
        for (k, s) in items {
            for g in fit(&m, &spec(k, s)).unwrap_or_else(|e| panic!("{k} {s}: {e}")) {
                attach(&mut m, &g).unwrap();
                made.push(g);
            }
        }
        (m, made)
    }

    #[test]
    fn kinds_have_names() {
        assert_eq!(canonical_kind("Trench"), Some("coat"));
        assert_eq!(canonical_kind("fedora"), Some("hat"));
        assert_eq!(canonical_kind("jeans"), Some("trousers"));
        assert_eq!(canonical_kind("sneakers"), Some("shoes"));
        assert_eq!(canonical_kind("banana"), None);
    }

    /// Body vertices a garment covers must lie under its surface: on the
    /// inner side of the nearest cloth, never outside it. Vertices near the
    /// cloth's open edges (hems, cuffs) are not judged: the cloth ends there.
    /// Returns (vertices the garment covers, those that poke through).
    fn poked_through(body: &Mesh, g: &Garment, probe: f32) -> (usize, usize) {
        let bvh = Bvh::of_mesh(&g.mesh);
        let open = open_vertices(&g.mesh);
        let edge_pts: Vec<V3> = (0..g.mesh.positions.len()).filter(|&i| open[i]).map(|i| g.mesh.positions[i]).collect();
        let (mut covered, mut poked) = (0, 0);
        for (v, p) in body.positions.iter().enumerate() {
            let Some((q, tri, d)) = bvh.nearest(*p, probe) else { continue };
            if edge_pts.iter().any(|e| length(sub(*e, q)) < probe * 1.5) {
                continue;
            }
            covered += 1;
            let sd = dot(sub(*p, q), g.mesh.face_normal(tri as usize));
            // Outside the nearest cloth, and no cloth over it either.
            let covered_above = bvh.ray(*p, body.normals[v], 1e-3, probe * 2.0).is_some();
            if sd > 0.15 && d < probe && !covered_above {
                poked += 1;
                if poked < 6 {
                    eprintln!("poke at {p:?} signed gap {sd:.2}");
                }
            }
        }
        (covered, poked)
    }

    #[test]
    fn a_wardrobe_fits_without_poking_through() {
        let body = bare("adult");
        for (kind, style) in [("shirt", "sweater"), ("coat", "trench"), ("trousers", "jeans"), ("jacket", "blazer"), ("dress", "sundress"), ("shoes", "boots"), ("gloves", "leather")] {
            let gs = fit(&body, &spec(kind, style)).unwrap_or_else(|e| panic!("{kind}: {e}"));
            for g in gs {
                assert!(g.mesh.triangles() > 200, "{kind} has a surface");
                assert_eq!(g.joints.len(), g.mesh.positions.len());
                // The cloth stands off the body by about its ease.
                let (covered, poked) = poked_through(&body, &g, 0.03 * 260.0);
                assert!(covered > 50, "{kind} covers the body ({covered})");
                assert!((poked as f32) < covered as f32 * 0.01, "{kind}: {poked} of {covered} body vertices poke through");
            }
        }
    }

    #[test]
    fn it_fits_any_body_a_child_an_elder_and_a_cartoon() {
        for preset in ["child", "toon"] {
            let body = bare(preset);
            let (m, made) = dress(&body, &[("trousers", "slacks"), ("shirt", "tee"), ("coat", "long"), ("hat", "beanie"), ("shoes", "sneakers")]);
            assert!(m.rig_in_step(), "{preset}");
            assert!(made.len() >= 5);
            let (lo, hi) = body.bounds();
            let h = hi[1] - lo[1];
            // Everything worn stays near the figure.
            let (glo, ghi) = m.bounds();
            assert!(glo[1] > lo[1] - 0.1 * h && ghi[1] < hi[1] + 0.12 * h, "{preset}: {glo:?} {ghi:?}");
        }
    }

    #[test]
    fn a_coat_follows_a_walk_without_stretching() {
        let body = bare("adult");
        let (m, made) = dress(&body, &[("coat", "trench")]);
        let rig = m.rig.clone().unwrap();
        let coat = &made[0];
        let first = body.positions.len();
        let n = coat.mesh.positions.len();
        // A stride: one thigh forward, the other back, arms swinging.
        let pose = PoseInput { rotations: HashMap::from([("thigh.L".to_string(), [35.0, 0.0, 0.0]), ("thigh.R".to_string(), [-35.0, 0.0, 0.0]), ("upper_arm.L".to_string(), [-25.0, 0.0, 0.0]), ("upper_arm.R".to_string(), [25.0, 0.0, 0.0]), ("shin.R".to_string(), [40.0, 0.0, 0.0])]), ..Default::default() };
        let posed = rig.posed_mesh(&m, &pose);
        // No edge of the coat grows past a stretch limit (open edges excluded: they are cut, not seams).
        let mut worst = 1.0f32;
        let mut seen = std::collections::HashSet::new();
        for t in 0..coat.mesh.triangles() {
            let c = coat.mesh.tri(t);
            for k in 0..3 {
                let (a, b) = (c[k], c[(k + 1) % 3]);
                if !seen.insert((a.min(b), a.max(b))) {
                    continue;
                }
                let (ra, rb) = (length(sub(coat.mesh.positions[a], coat.mesh.positions[b])), length(sub(posed.positions[first + a], posed.positions[first + b])));
                if ra > 0.012 * 260.0 {
                    worst = worst.max(rb / ra);
                }
            }
        }
        assert!(n > 1000);
        if worst > 1.5 {
            // Where: the worst edges, for the report.
            let mut edges: Vec<(f32, usize, usize)> = Vec::new();
            let mut seen = std::collections::HashSet::new();
            for t in 0..coat.mesh.triangles() {
                let c = coat.mesh.tri(t);
                for k in 0..3 {
                    let (a, b) = (c[k], c[(k + 1) % 3]);
                    if !seen.insert((a.min(b), a.max(b))) {
                        continue;
                    }
                    let (ra, rb) = (length(sub(coat.mesh.positions[a], coat.mesh.positions[b])), length(sub(posed.positions[first + a], posed.positions[first + b])));
                    if ra > 0.012 * 260.0 {
                        edges.push((rb / ra, a, b));
                    }
                }
            }
            edges.sort_by(|x, y| y.0.total_cmp(&x.0));
            for (r, a, b) in edges.iter().take(5) {
                eprintln!("{r:.1}x edge {a}-{b} of {} verts, materials {:?}: at {:?} {:?} joints {:?} {:?} weights {:?} {:?}", coat.mesh.positions.len(), coat.mesh.materials.iter().map(|m| m.name.clone()).collect::<Vec<_>>(), coat.mesh.positions[*a], coat.mesh.positions[*b], coat.joints[*a], coat.joints[*b], coat.weights[*a], coat.weights[*b]);
            }
        }
        // No worse than the body's own skin under the same stride.
        let body_posed = rig.posed_mesh(&m, &pose);
        let mut body_worst = 1.0f32;
        let mut seen = std::collections::HashSet::new();
        for t in 0..body.triangles() {
            let c = body.tri(t);
            for k in 0..3 {
                let (a, b) = (c[k], c[(k + 1) % 3]);
                if !seen.insert((a.min(b), a.max(b))) {
                    continue;
                }
                let (ra, rb) = (length(sub(body.positions[a], body.positions[b])), length(sub(body_posed.positions[a], body_posed.positions[b])));
                if ra > 2.5 {
                    body_worst = body_worst.max(rb / ra);
                }
            }
        }
        assert!(worst < (1.5f32).max(body_worst * 1.25), "the coat stretches {worst}x at worst in a stride, the body {body_worst}x");
    }

    #[test]
    fn a_hat_stays_rigid_on_the_head_and_follows_it() {
        let body = bare("adult");
        let (m, made) = dress(&body, &[("hat", "fedora")]);
        let rig = m.rig.clone().unwrap();
        let first = body.positions.len();
        let head = rig.bone("head").unwrap();
        assert!(made.iter().all(|g| g.joints.iter().zip(&g.weights).all(|(j, w)| j[0] as usize == head && w[0] == 1.0)), "all on the head, whole");
        let posed = rig.posed_mesh(&m, &PoseInput { rotations: HashMap::from([("head".to_string(), [0.0, 40.0, 15.0])]), ..Default::default() });
        // Distances between hat vertices do not change.
        let n = m.positions.len();
        let mut worst = 0.0f32;
        for i in (first..n).step_by(37) {
            for j in (first..n).step_by(211) {
                let (a, b) = (length(sub(m.positions[i], m.positions[j])), length(sub(posed.positions[i], posed.positions[j])));
                worst = worst.max((a - b).abs());
            }
        }
        assert!(worst < 0.05, "rigid: {worst}");
        // And it moved with the head.
        let moved: f32 = (first..n).step_by(50).map(|i| length(sub(posed.positions[i], m.positions[i]))).sum::<f32>() / ((n - first) / 50) as f32;
        assert!(moved > 3.0, "{moved}");
        // It sits on the head: the crown of the hat is above the skull.
        let skull = body.positions.iter().map(|p| p[1]).fold(f32::INFINITY, f32::min);
        let h = body.bounds().1[1] - body.bounds().0[1];
        let hat_top = m.positions[first..].iter().map(|p| p[1]).fold(f32::INFINITY, f32::min);
        assert!(hat_top < skull + 2.0 && hat_top > skull - 0.12 * h, "{hat_top} {skull}");
    }

    #[test]
    fn what_is_worn_comes_off_by_name() {
        let body = bare("adult");
        let (mut m, _) = dress(&body, &[("shirt", "tee"), ("hat", "cap")]);
        let w = worn(&m);
        assert!(w.iter().any(|n| n.starts_with("Tee")) && w.iter().any(|n| n.starts_with("Cap")), "{w:?}");
        let before = m.triangles();
        let gone = remove(&mut m, "cap").unwrap();
        assert!(gone > 100 && m.triangles() == before - gone);
        assert!(m.rig_in_step());
        assert!(remove(&mut m, "cape").unwrap_err().contains("nothing called"));
    }

    #[test]
    fn a_body_without_the_bones_is_told_what_is_missing() {
        let mut m = crate::prim::cylinder(30.0, 30.0, 200.0, 24);
        crate::ops::subdivide(&mut m, 1);
        let mut rig = crate::autorig::chain(&m, 2, "top", "seg");
        crate::autorig::bind(&m, &mut rig);
        m.rig = Some(std::sync::Arc::new(rig));
        let err = fit(&m, &spec("coat", "trench")).unwrap_err();
        assert!(err.contains("needs the bone"), "{err}");
    }
}

