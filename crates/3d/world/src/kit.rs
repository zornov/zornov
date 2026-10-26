// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Modular kits: walls with doorways and windows, floors with stairwells,
//! stairs, roofs, towers, shops, warehouses, rooms and sci-fi halls, put
//! together from boxes with real thickness, each coming with the solids a
//! walker bumps into. Everything is in metres, y up, standing on y = 0 with
//! its middle at the origin; the front (the door side) faces -z.

use crate::collide::Solid;
use crate::math::V3;
use crate::noise::Rng;
use studio_mesh::{Material, Mesh};

/// A box-by-box mesh builder with materials.
#[derive(Default)]
pub struct MeshBuilder {
    pos: Vec<V3>,
    nrm: Vec<V3>,
    uv: Vec<[f32; 2]>,
    idx: Vec<u32>,
    tri_mat: Vec<u32>,
    mats: Vec<Material>,
}

fn rot(p: V3, yaw_deg: f32) -> V3 {
    let (s, c) = yaw_deg.to_radians().sin_cos();
    [p[0] * c + p[2] * s, p[1], -p[0] * s + p[2] * c]
}

/// A plain material in linear RGB.
pub fn mat(name: &str, rgb: V3, rough: f32, metal: f32) -> Material {
    Material { name: name.into(), base_colour: [rgb[0], rgb[1], rgb[2], 1.0], roughness: rough, metallic: metal, ..Material::default() }
}
pub fn glow(name: &str, rgb: V3, strength: f32) -> Material {
    Material { name: name.into(), base_colour: [rgb[0] * 0.2, rgb[1] * 0.2, rgb[2] * 0.2, 1.0], roughness: 0.4, emissive: [rgb[0] * strength, rgb[1] * strength, rgb[2] * strength], ..Material::default() }
}

impl MeshBuilder {
    /// Takes a finished mesh in as it is.
    pub fn adopt(&mut self, m: Mesh) {
        self.tri_mat = m.tri_materials();
        self.pos = m.positions;
        self.nrm = m.normals;
        self.uv = m.uvs;
        self.idx = m.indices;
        self.mats = m.materials;
    }

    pub fn material(&mut self, m: Material) -> u32 {
        if let Some(i) = self.mats.iter().position(|x| x.name == m.name) {
            return i as u32;
        }
        self.mats.push(m);
        (self.mats.len() - 1) as u32
    }

    /// A quad with corners in order round its front (counter-clockwise
    /// seen from the side it faces); `uv_scale` is metres per texture
    /// repeat.
    pub fn quad(&mut self, p: [V3; 4], m: u32) {
        use crate::math::{cross, norm, sub};
        let n = norm(cross(sub(p[1], p[0]), sub(p[3], p[0])));
        let base = self.pos.len() as u32;
        let (u, v) = (crate::math::len(sub(p[1], p[0])), crate::math::len(sub(p[3], p[0])));
        let uvs = [[0.0, 0.0], [u, 0.0], [u, v], [0.0, v]];
        for k in 0..4 {
            self.pos.push(p[k]);
            self.nrm.push(n);
            self.uv.push(uvs[k]);
        }
        self.idx.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        self.tri_mat.extend_from_slice(&[m, m]);
    }

    /// A triangle (counter-clockwise seen from its front).
    pub fn tri(&mut self, p: [V3; 3], m: u32) {
        use crate::math::{cross, norm, sub};
        let n = norm(cross(sub(p[1], p[0]), sub(p[2], p[0])));
        let base = self.pos.len() as u32;
        for k in 0..3 {
            self.pos.push(p[k]);
            self.nrm.push(n);
            self.uv.push([p[k][0], p[k][2]]);
        }
        self.idx.extend_from_slice(&[base, base + 1, base + 2]);
        self.tri_mat.push(m);
    }

    /// A box with a material per face group: sides, top, bottom.
    pub fn cuboid3(&mut self, c: V3, half: V3, yaw: f32, sides: u32, top: u32, bottom: u32) {
        for a in 0..3 {
            let (u, v) = ((a + 1) % 3, (a + 2) % 3);
            for sign in [1.0f32, -1.0] {
                let m = if a == 1 { if sign > 0.0 { top } else { bottom } } else { sides };
                let corner = |su: f32, sv: f32| {
                    let mut o = [0.0f32; 3];
                    o[a] = sign * half[a];
                    o[u] = su * half[u];
                    o[v] = sv * half[v];
                    let r = rot(o, yaw);
                    [c[0] + r[0], c[1] + r[1], c[2] + r[2]]
                };
                let q = if sign > 0.0 { [corner(-1.0, -1.0), corner(1.0, -1.0), corner(1.0, 1.0), corner(-1.0, 1.0)] } else { [corner(-1.0, -1.0), corner(-1.0, 1.0), corner(1.0, 1.0), corner(1.0, -1.0)] };
                self.quad(q, m);
            }
        }
    }

    pub fn cuboid(&mut self, c: V3, half: V3, yaw: f32, m: u32) {
        self.cuboid3(c, half, yaw, m, m, m);
    }

    /// An upright cylinder with smooth sides and flat caps.
    pub fn cylinder(&mut self, base: V3, radius: f32, height: f32, segs: usize, m: u32) {
        let segs = segs.max(3);
        let ring = |k: usize| {
            let a = k as f32 / segs as f32 * std::f32::consts::TAU;
            (a.sin(), a.cos())
        };
        let b0 = self.pos.len() as u32;
        for k in 0..=segs {
            let (s, c) = ring(k % segs);
            for (y, v) in [(0.0, 0.0), (height, 1.0)] {
                self.pos.push([base[0] + s * radius, base[1] + y, base[2] + c * radius]);
                self.nrm.push([s, 0.0, c]);
                self.uv.push([k as f32 / segs as f32 * radius * std::f32::consts::TAU, v * height]);
            }
        }
        for k in 0..segs as u32 {
            let (a, b, c, d) = (b0 + 2 * k, b0 + 2 * k + 1, b0 + 2 * k + 2, b0 + 2 * k + 3);
            // Outward: counter-clockwise seen from outside.
            self.idx.extend_from_slice(&[a, c, d, a, d, b]);
            self.tri_mat.extend_from_slice(&[m, m]);
        }
        for (y, up) in [(height, true), (0.0, false)] {
            for k in 0..segs {
                let (s0, c0) = ring(k);
                let (s1, c1) = ring(k + 1);
                let p = |s: f32, c: f32| [base[0] + s * radius, base[1] + y, base[2] + c * radius];
                let ctr = [base[0], base[1] + y, base[2]];
                if up {
                    self.tri([ctr, p(s1, c1), p(s0, c0)], m);
                } else {
                    self.tri([ctr, p(s0, c0), p(s1, c1)], m);
                }
            }
        }
    }

    /// A cone-ish frustum for trunks and arms: bottom and top radius.
    pub fn frustum(&mut self, base: V3, r0: f32, r1: f32, height: f32, segs: usize, m: u32) {
        let segs = segs.max(3);
        let b0 = self.pos.len() as u32;
        let slope = (r0 - r1) / height.max(1e-4);
        for k in 0..=segs {
            let a = (k % segs) as f32 / segs as f32 * std::f32::consts::TAU;
            let (s, c) = (a.sin(), a.cos());
            let n = crate::math::norm([s, slope, c]);
            for (y, r) in [(0.0, r0), (height, r1)] {
                self.pos.push([base[0] + s * r, base[1] + y, base[2] + c * r]);
                self.nrm.push(n);
                self.uv.push([k as f32 * 0.2, y]);
            }
        }
        for k in 0..segs as u32 {
            let (a, b, c, d) = (b0 + 2 * k, b0 + 2 * k + 1, b0 + 2 * k + 2, b0 + 2 * k + 3);
            self.idx.extend_from_slice(&[a, c, d, a, d, b]);
            self.tri_mat.extend_from_slice(&[m, m]);
        }
    }

    pub fn finish(self) -> Mesh {
        let mut m = Mesh { positions: self.pos, normals: self.nrm, uvs: self.uv, indices: self.idx, materials: self.mats, ..Default::default() };
        if m.materials.is_empty() {
            m.materials.push(Material::default());
        }
        let tm = self.tri_mat;
        m.regroup(&tm);
        m
    }
}

/// A made structure.
pub struct Built {
    pub b: MeshBuilder,
    pub solids: Vec<Solid>,
    /// Points that glow: local position, colour (linear), range in metres.
    pub lights: Vec<(V3, V3, f32)>,
    pub size: [f32; 3],
}

impl Built {
    fn new(size: [f32; 3]) -> Built {
        Built { b: MeshBuilder::default(), solids: Vec::new(), lights: Vec::new(), size }
    }

    /// A built structure around a finished mesh (no solids or lights yet).
    pub fn from_mesh(mesh: Mesh, size: [f32; 3]) -> Built {
        let mut b = MeshBuilder::default();
        b.adopt(mesh);
        Built { b, solids: Vec::new(), lights: Vec::new(), size }
    }

    fn solid_box(&mut self, c: V3, half: V3, yaw: f32) {
        self.solids.push(Solid::Box { centre: c, half, yaw });
    }

    /// A box that is both seen and bumped into.
    fn block(&mut self, c: V3, half: V3, yaw: f32, m: u32) {
        self.b.cuboid(c, half, yaw, m);
        self.solid_box(c, half, yaw);
    }

    pub fn mesh(self) -> (Mesh, Vec<Solid>, Vec<(V3, V3, f32)>) {
        (self.b.finish(), self.solids, self.lights)
    }

    /// The solids placed at `at`, turned `yaw` degrees.
    pub fn solids_placed(solids: &[Solid], at: V3, yaw: f32) -> Vec<Solid> {
        solids
            .iter()
            .map(|s| match s {
                Solid::Box { centre, half, yaw: y } => {
                    let r = rot(*centre, yaw);
                    Solid::Box { centre: [at[0] + r[0], at[1] + r[1], at[2] + r[2]], half: *half, yaw: y + yaw }
                }
                Solid::Cylinder { base, radius, height } => {
                    let r = rot(*base, yaw);
                    Solid::Cylinder { base: [at[0] + r[0], at[1] + r[1], at[2] + r[2]], radius: *radius, height: *height }
                }
                Solid::Mesh { bvh, at: a, yaw: y, scale } => {
                    let r = rot(*a, yaw);
                    Solid::Mesh { bvh: bvh.clone(), at: [at[0] + r[0], at[1] + r[1], at[2] + r[2]], yaw: y + yaw, scale: *scale }
                }
            })
            .collect()
    }
}

/// A doorway or window in a wall.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Opening {
    /// Centre, metres along the wall from its start.
    pub at: f32,
    pub width: f32,
    /// Bottom above the wall's base (0 for a doorway).
    pub sill: f32,
    pub head: f32,
}

impl Opening {
    pub fn door(at: f32) -> Opening {
        Opening { at, width: 1.3, sill: 0.0, head: 2.2 }
    }
    pub fn window(at: f32) -> Opening {
        Opening { at, width: 1.3, sill: 0.95, head: 2.0 }
    }
    pub fn is_door(&self) -> bool {
        self.sill <= 0.01
    }
}

/// Materials a structure is dressed in.
#[derive(Clone, Copy)]
pub struct Palette {
    pub wall: u32,
    pub trim: u32,
    pub floor: u32,
    pub roof: u32,
    pub glass: u32,
}

/// A straight wall from (x0, z0) to (x1, z1) standing on `base_y`, with
/// openings cut through it. Glass fills windows; door frames are added.
#[allow(clippy::too_many_arguments)]
fn wall(bt: &mut Built, from: [f32; 2], to: [f32; 2], base_y: f32, height: f32, thick: f32, openings: &[Opening], pal: &Palette) {
    let d = [to[0] - from[0], to[1] - from[1]];
    let l = (d[0] * d[0] + d[1] * d[1]).sqrt();
    if l < 1e-3 {
        return;
    }
    let dir = [d[0] / l, d[1] / l];
    let yaw = (-dir[1]).atan2(dir[0]).to_degrees();
    // A box along the wall from s0..s1 and y0..y1.
    let seg = |bt: &mut Built, s0: f32, s1: f32, y0: f32, y1: f32, m: u32, solid: bool| {
        if s1 - s0 < 1e-3 || y1 - y0 < 1e-3 {
            return;
        }
        let sm = (s0 + s1) * 0.5;
        let c = [from[0] + dir[0] * sm, base_y + (y0 + y1) * 0.5, from[1] + dir[1] * sm];
        let half = [(s1 - s0) * 0.5, (y1 - y0) * 0.5, thick * 0.5];
        if solid {
            bt.block(c, half, yaw, m);
        } else {
            bt.b.cuboid(c, half, yaw, m);
        }
    };
    let mut ops: Vec<Opening> = openings.iter().map(|o| Opening { at: o.at.clamp(o.width * 0.5 + 0.1, l - o.width * 0.5 - 0.1), ..*o }).collect();
    ops.sort_by(|a, b| a.at.total_cmp(&b.at));
    let mut s = 0.0;
    for o in &ops {
        let (o0, o1) = (o.at - o.width * 0.5, o.at + o.width * 0.5);
        seg(bt, s, o0, 0.0, height, pal.wall, true);
        if o.sill > 0.01 {
            seg(bt, o0, o1, 0.0, o.sill, pal.wall, true);
        }
        seg(bt, o0, o1, o.head, height, pal.wall, true);
        // Frame and glass.
        let ft = 0.06;
        seg(bt, o0, o0 + ft, o.sill, o.head, pal.trim, false);
        seg(bt, o1 - ft, o1, o.sill, o.head, pal.trim, false);
        seg(bt, o0, o1, o.head - ft, o.head, pal.trim, false);
        if !o.is_door() {
            seg(bt, o0, o1, o.sill, o.sill + ft, pal.trim, false);
            let sm = o.at;
            let c = [from[0] + dir[0] * sm, base_y + (o.sill + o.head) * 0.5, from[1] + dir[1] * sm];
            bt.b.cuboid(c, [o.width * 0.5 - ft, (o.head - o.sill) * 0.5 - ft, 0.02], yaw, pal.glass);
        }
        s = o1;
    }
    seg(bt, s, l, 0.0, height, pal.wall, true);
}

/// A floor slab with an optional rectangular hole (a stairwell):
/// (x0, z0, x1, z1) of the hole.
fn slab(bt: &mut Built, min: [f32; 2], max: [f32; 2], y_top: f32, thick: f32, hole: Option<[f32; 4]>, m: u32) {
    let y = y_top - thick * 0.5;
    let rect = |bt: &mut Built, a: [f32; 2], b: [f32; 2]| {
        if b[0] - a[0] < 1e-3 || b[1] - a[1] < 1e-3 {
            return;
        }
        bt.block([(a[0] + b[0]) * 0.5, y, (a[1] + b[1]) * 0.5], [(b[0] - a[0]) * 0.5, thick * 0.5, (b[1] - a[1]) * 0.5], 0.0, m);
    };
    match hole {
        None => rect(bt, min, max),
        Some(h) => {
            rect(bt, min, [max[0], h[1]]);
            rect(bt, [min[0], h[3]], max);
            rect(bt, [min[0], h[1]], [h[0], h[3]]);
            rect(bt, [h[2], h[1]], [max[0], h[3]]);
        }
    }
}

/// A straight stair rising along +z from (x, z0), `width` wide, up
/// `rise` metres in steps no taller than `max_step`.
fn stairs(bt: &mut Built, x: f32, z0: f32, width: f32, base_y: f32, rise: f32, tread: f32, m: u32) -> f32 {
    let n = (rise / 0.18).ceil().max(1.0) as usize;
    let h = rise / n as f32;
    for k in 0..n {
        let top = base_y + h * (k + 1) as f32;
        let zc = z0 + tread * (k as f32 + 0.5);
        bt.block([x, (base_y + top) * 0.5, zc], [width * 0.5, (top - base_y) * 0.5, tread * 0.5], 0.0, m);
    }
    z0 + tread * n as f32
}

fn pal_for(bt: &mut Built, rng: &mut Rng, kind: &str) -> Palette {
    let pick = |rng: &mut Rng, list: &[V3]| list[rng.index(list.len())];
    let (wall, roof, trim, floor) = match kind {
        "house" => (pick(rng, &[[0.62, 0.55, 0.43], [0.7, 0.66, 0.58], [0.5, 0.34, 0.26], [0.74, 0.7, 0.55], [0.38, 0.42, 0.4]]), pick(rng, &[[0.28, 0.1, 0.07], [0.13, 0.14, 0.16], [0.3, 0.17, 0.1]]), [0.8, 0.78, 0.72], [0.28, 0.17, 0.1]),
        "tower" => (pick(rng, &[[0.32, 0.34, 0.37], [0.5, 0.5, 0.48], [0.25, 0.3, 0.38], [0.4, 0.33, 0.28]]), [0.12, 0.12, 0.13], [0.14, 0.14, 0.16], [0.3, 0.3, 0.32]),
        "scifi" => ([0.62, 0.65, 0.7], [0.18, 0.2, 0.24], [0.08, 0.09, 0.11], [0.2, 0.22, 0.26]),
        _ => (pick(rng, &[[0.55, 0.5, 0.42], [0.6, 0.58, 0.55], [0.45, 0.4, 0.34]]), [0.2, 0.2, 0.22], [0.2, 0.2, 0.2], [0.35, 0.32, 0.28]),
    };
    let metal = if kind == "scifi" { 0.6 } else { 0.0 };
    Palette {
        wall: bt.b.material(mat(&format!("{kind} wall"), wall, if kind == "scifi" { 0.35 } else { 0.9 }, metal)),
        trim: bt.b.material(mat(&format!("{kind} trim"), trim, 0.6, if kind == "scifi" { 0.8 } else { 0.0 })),
        floor: bt.b.material(mat(&format!("{kind} floor"), floor, 0.7, 0.0)),
        roof: bt.b.material(mat(&format!("{kind} roof"), roof, 0.8, 0.0)),
        glass: bt.b.material(Material { name: "Glass".into(), base_colour: [0.04, 0.07, 0.1, 1.0], roughness: 0.04, metallic: 0.3, ..Material::default() }),
    }
}

/// What kinds of structure the kit makes.
pub const STRUCTURES: &[&str] = &["house", "cottage", "apartment", "tower", "skyscraper", "shop", "warehouse", "room", "office", "hall", "corridor", "hab"];

/// A structure by name: `w` x `d` metres on the ground, `floors` high
/// where that applies.
pub fn structure(kind: &str, w: f32, d: f32, floors: usize, seed: u32) -> Option<Built> {
    Some(match kind {
        "house" | "cottage" => house(w, d, floors.clamp(1, 3), seed),
        "apartment" | "tower" | "skyscraper" | "office" => tower(w, d, floors.clamp(2, 60), seed),
        "shop" => shop(w, d, seed),
        "warehouse" => warehouse(w, d, seed),
        "room" => room(w, d, 2.8, seed, false),
        "hall" | "corridor" | "hab" => room(w, d, 3.0, seed, true),
        _ => return None,
    })
}

/// A house: thick walls with doors and windows, floors with a stairwell
/// and stairs, a gabled roof and a chimney.
pub fn house(w: f32, d: f32, floors: usize, seed: u32) -> Built {
    let (w, d) = (w.max(4.0), d.max(4.0));
    let fh = 2.8;
    let total = fh * floors as f32 + 0.2;
    let mut bt = Built::new([w, total + w.min(d) * 0.35, d]);
    let mut rng = Rng::new(seed);
    let pal = pal_for(&mut bt, &mut rng, "house");
    let (hw, hd) = (w * 0.5, d * 0.5);
    let t = 0.25;
    // Foundation and ground floor.
    bt.block([0.0, 0.1, 0.0], [hw + 0.1, 0.1, hd + 0.1], 0.0, pal.trim);
    let floor_y = |f: usize| 0.2 + fh * f as f32;
    let door_x = rng.range(-w * 0.2, w * 0.2) + hw;
    for f in 0..floors {
        let y0 = floor_y(f);
        let win_count_w = ((w / 3.0) as usize).max(1);
        let win_count_d = ((d / 3.5) as usize).max(1);
        let spread = |len: usize, l: f32| -> Vec<f32> { (0..len).map(|i| l * (i as f32 + 0.5) / len as f32).collect() };
        // Front (-z): the door on the ground floor.
        let mut front: Vec<Opening> = spread(win_count_w, w).into_iter().map(Opening::window).collect();
        if f == 0 {
            front.retain(|o| (o.at - door_x).abs() > 1.6);
            front.push(Opening::door(door_x));
        }
        let back: Vec<Opening> = spread(win_count_w, w).into_iter().map(Opening::window).collect();
        let side: Vec<Opening> = spread(win_count_d, d).into_iter().map(Opening::window).collect();
        wall(&mut bt, [-hw, -hd], [hw, -hd], y0, fh, t, &front, &pal);
        wall(&mut bt, [hw, hd], [-hw, hd], y0, fh, t, &back.iter().map(|o| Opening { at: w - o.at, ..*o }).collect::<Vec<_>>(), &pal);
        wall(&mut bt, [-hw, hd], [-hw, -hd], y0, fh, t, &side.iter().map(|o| Opening { at: d - o.at, ..*o }).collect::<Vec<_>>(), &pal);
        wall(&mut bt, [hw, -hd], [hw, hd], y0, fh, t, &side, &pal);
        // Interior partition with a doorway on wide houses.
        if w > 7.0 {
            wall(&mut bt, [-hw + w * 0.4, -hd + t], [-hw + w * 0.4, hd - t], y0, fh, 0.15, &[Opening::door(d * 0.35)], &pal);
        }
        bt.lights.push(([0.0, y0 + fh - 0.4, 0.0], [1.0, 0.72, 0.42], (w.max(d) * 1.1).max(5.0)));
    }
    // Ground floor boards, upper floors with a stairwell on the left wall.
    let (sx, sz0) = (-hw + 0.9 + t, -hd + t + 0.4);
    let wide = 1.0;
    for f in 0..floors {
        let top = floor_y(f);
        if f == 0 {
            slab(&mut bt, [-hw, -hd], [hw, hd], top, 0.2, None, pal.floor);
        }
        if f + 1 < floors || floors > 1 {
            // Stairs from this floor to the next.
            if f + 1 < floors {
                let end = stairs(&mut bt, sx, sz0, wide, top, fh, 0.27, pal.floor);
                let hole = [sx - wide * 0.5 - 0.05, sz0 - 0.05, sx + wide * 0.5 + 0.05, end];
                slab(&mut bt, [-hw + t, -hd + t], [hw - t, hd - t], floor_y(f + 1), 0.25, Some(hole), pal.floor);
            }
        }
    }
    // Ceiling over the top floor.
    let top_y = floor_y(floors);
    slab(&mut bt, [-hw - 0.05, -hd - 0.05], [hw + 0.05, hd + 0.05], top_y + 0.15, 0.25, None, pal.trim);
    // Gabled roof along x: two slopes, gables closed, overhanging.
    let rise = d.min(w) * 0.32;
    let eave = 0.45;
    let y_e = top_y + 0.15;
    let (x0, x1) = (-hw - eave, hw + eave);
    let (zf, zb) = (-hd - eave, hd + eave);
    let ridge = [y_e + rise + 0.1, 0.0];
    let slope = |bt: &mut Built, zs: f32, flip: bool| {
        let (a, b, c, e) = ([x0, y_e, zs], [x1, y_e, zs], [x1, ridge[0], 0.0], [x0, ridge[0], 0.0]);
        let q = if flip { [b, a, e, c] } else { [a, b, c, e] };
        bt.b.quad(q, pal.roof);
        // Underside for thickness.
        let q2 = if flip { [a, b, c, e] } else { [b, a, e, c] };
        bt.b.quad(q2, pal.roof);
    };
    slope(&mut bt, zf, false);
    slope(&mut bt, zb, true);
    for (x, flip) in [(-hw, true), (hw, false)] {
        let (p0, p1, p2) = ([x, y_e, -hd], [x, y_e, hd], [x, y_e + rise * (1.0 - 0.0) * 0.0 + (rise * 0.78), 0.0]);
        let tri = if flip { [p1, p0, p2] } else { [p0, p1, p2] };
        bt.b.tri(tri, pal.wall);
    }
    // Chimney.
    let cx = hw * 0.4 * if rng.next() > 0.5 { 1.0 } else { -1.0 };
    bt.b.cuboid(cx_point(cx, y_e + rise * 0.7, hd * 0.3), [0.3, rise * 0.55, 0.3], 0.0, pal.trim);
    bt
}

fn cx_point(x: f32, y: f32, z: f32) -> V3 {
    [x, y, z]
}

/// A tower of floors: a lobby you can enter, a grid of windows (some lit),
/// a parapet and rooftop plant.
pub fn tower(w: f32, d: f32, floors: usize, seed: u32) -> Built {
    let (w, d) = (w.max(6.0), d.max(6.0));
    let fh = 3.6;
    let total = fh * floors as f32;
    let mut bt = Built::new([w, total + 3.0, d]);
    let mut rng = Rng::new(seed);
    let pal = pal_for(&mut bt, &mut rng, "tower");
    let lit = bt.b.material(glow("Lit window", [1.0, 0.78, 0.5], 2.2));
    let (hw, hd) = (w * 0.5, d * 0.5);
    let t = 0.4;
    // Lobby: hollow, with a wide door and windows.
    let door = Opening { at: w * 0.5, width: 2.2, sill: 0.0, head: 2.6 };
    let cols = ((w / 3.2) as usize).max(2);
    let cs: Vec<f32> = (0..cols).map(|i| w * (i as f32 + 0.5) / cols as f32).collect();
    let lobby_h = fh + 0.8;
    let mut front: Vec<Opening> = cs.iter().filter(|c| (**c - w * 0.5).abs() > 2.0).map(|c| Opening { at: *c, width: 1.8, sill: 0.4, head: 3.0 }).collect();
    front.push(door);
    wall(&mut bt, [-hw, -hd], [hw, -hd], 0.0, lobby_h, t, &front, &pal);
    wall(&mut bt, [hw, hd], [-hw, hd], 0.0, lobby_h, t, &[], &pal);
    wall(&mut bt, [-hw, hd], [-hw, -hd], 0.0, lobby_h, t, &[], &pal);
    wall(&mut bt, [hw, -hd], [hw, hd], 0.0, lobby_h, t, &[], &pal);
    slab(&mut bt, [-hw, -hd], [hw, hd], 0.1, 0.1, None, pal.floor);
    bt.lights.push(([0.0, lobby_h - 0.5, 0.0], [1.0, 0.85, 0.65], w.max(d)));
    // Upper floors: solid block with a window grid on every face.
    let upper = total - lobby_h;
    let c = [0.0, lobby_h + upper * 0.5, 0.0];
    bt.block(c, [hw, upper * 0.5, hd], 0.0, pal.wall);
    let rows = ((upper / fh) as usize).max(1);
    let rh = upper / rows as f32;
    for face in 0..4 {
        let (n, along, len) = match face {
            0 => ([0.0, 0.0, -1.0], [1.0, 0.0, 0.0], w),
            1 => ([0.0, 0.0, 1.0], [-1.0, 0.0, 0.0], w),
            2 => ([-1.0, 0.0, 0.0], [0.0, 0.0, -1.0], d),
            _ => ([1.0, 0.0, 0.0], [0.0, 0.0, 1.0], d),
        };
        let off = if face < 2 { hd } else { hw };
        let cols = ((len / 2.4) as usize).max(2);
        for r in 0..rows {
            for k in 0..cols {
                let s = len * (k as f32 + 0.5) / cols as f32 - len * 0.5;
                let y = lobby_h + rh * (r as f32 + 0.5);
                let pos = [n[0] * (off + 0.03) + along[0] * s, y, n[2] * (off + 0.03) + along[2] * s];
                let half_w = (len / cols as f32) * 0.36;
                let m = if rng.next() < 0.18 { lit } else { pal.glass };
                let yaw = (-n[2]).atan2(n[0]).to_degrees() + 90.0;
                bt.b.cuboid(pos, [half_w, rh * 0.32, 0.03], yaw, m);
            }
        }
    }
    // Ledges and parapet.
    for r in 0..=rows {
        let y = lobby_h + rh * r as f32;
        bt.b.cuboid([0.0, y, 0.0], [hw + 0.12, 0.08, hd + 0.12], 0.0, pal.trim);
    }
    bt.block([0.0, total + 0.5, 0.0], [hw + 0.1, 0.5, hd + 0.1], 0.0, pal.trim);
    bt.b.cuboid([rng.range(-hw * 0.3, hw * 0.3), total + 1.5, rng.range(-hd * 0.3, hd * 0.3)], [hw * 0.25, 1.0, hd * 0.25], 0.0, pal.roof);
    bt
}

/// A shop: one big room with a glass front and a flat roof with a sign.
pub fn shop(w: f32, d: f32, seed: u32) -> Built {
    let (w, d) = (w.max(5.0), d.max(5.0));
    let h = 3.6;
    let mut bt = Built::new([w, h + 1.2, d]);
    let mut rng = Rng::new(seed);
    let pal = pal_for(&mut bt, &mut rng, "shop");
    let awning = bt.b.material(mat("Awning", [rng.range(0.2, 0.7), rng.range(0.1, 0.4), rng.range(0.1, 0.4)], 0.85, 0.0));
    let (hw, hd) = (w * 0.5, d * 0.5);
    let front = [Opening { at: w * 0.5, width: 1.4, sill: 0.0, head: 2.3 }, Opening { at: w * 0.2, width: w * 0.3, sill: 0.5, head: 2.8 }, Opening { at: w * 0.8, width: w * 0.3, sill: 0.5, head: 2.8 }];
    wall(&mut bt, [-hw, -hd], [hw, -hd], 0.0, h, 0.3, &front, &pal);
    wall(&mut bt, [hw, hd], [-hw, hd], 0.0, h, 0.3, &[], &pal);
    wall(&mut bt, [-hw, hd], [-hw, -hd], 0.0, h, 0.3, &[Opening::window(d * 0.5)], &pal);
    wall(&mut bt, [hw, -hd], [hw, hd], 0.0, h, 0.3, &[], &pal);
    slab(&mut bt, [-hw, -hd], [hw, hd], 0.1, 0.1, None, pal.floor);
    slab(&mut bt, [-hw - 0.1, -hd - 0.1], [hw + 0.1, hd + 0.1], h + 0.2, 0.3, None, pal.roof);
    bt.b.cuboid([0.0, h - 0.5, -hd - 0.5], [w * 0.5, 0.05, 0.6], 8.0 * 0.0, awning);
    bt.b.cuboid([0.0, h + 0.7, -hd + 0.2], [w * 0.3, 0.4, 0.08], 0.0, awning);
    bt.lights.push(([0.0, h - 0.4, 0.0], [1.0, 0.9, 0.75], w.max(d)));
    bt
}

/// A warehouse: a big hollow shed with a wide door, clerestory windows,
/// racks inside and a low-pitched roof.
pub fn warehouse(w: f32, d: f32, seed: u32) -> Built {
    let (w, d) = (w.max(10.0), d.max(10.0));
    let h = 7.0;
    let mut bt = Built::new([w, h + 1.5, d]);
    let mut rng = Rng::new(seed);
    let pal = pal_for(&mut bt, &mut rng, "warehouse");
    let (hw, hd) = (w * 0.5, d * 0.5);
    let high: Vec<Opening> = (0..((w / 6.0) as usize).max(2)).map(|i| Opening { at: w * (i as f32 + 0.5) / ((w / 6.0) as f32).max(2.0), width: 2.4, sill: 4.8, head: 6.2 }).collect();
    let mut front = high.clone();
    front.push(Opening { at: w * 0.5, width: 4.5, sill: 0.0, head: 4.5 });
    front.retain(|o| o.sill > 0.0 && (o.at - w * 0.5).abs() > 3.4 || o.sill == 0.0);
    wall(&mut bt, [-hw, -hd], [hw, -hd], 0.0, h, 0.3, &front, &pal);
    wall(&mut bt, [hw, hd], [-hw, hd], 0.0, h, 0.3, &[], &pal);
    wall(&mut bt, [-hw, hd], [-hw, -hd], 0.0, h, 0.3, &[Opening::door(d * 0.25)], &pal);
    wall(&mut bt, [hw, -hd], [hw, hd], 0.0, h, 0.3, &[], &pal);
    slab(&mut bt, [-hw, -hd], [hw, hd], 0.1, 0.1, None, pal.floor);
    // Low roof, sloping a little.
    let y_e = h;
    let ridge = h + 1.3;
    bt.b.quad([[-hw - 0.4, y_e, -hd - 0.4], [hw + 0.4, y_e, -hd - 0.4], [hw + 0.4, ridge, 0.0], [-hw - 0.4, ridge, 0.0]], pal.roof);
    bt.b.quad([[hw + 0.4, y_e, hd + 0.4], [-hw - 0.4, y_e, hd + 0.4], [-hw - 0.4, ridge, 0.0], [hw + 0.4, ridge, 0.0]], pal.roof);
    bt.b.quad([[-hw - 0.4, ridge, 0.0], [hw + 0.4, ridge, 0.0], [hw + 0.4, y_e, -hd - 0.4], [-hw - 0.4, y_e, -hd - 0.4]], pal.roof);
    bt.b.quad([[hw + 0.4, ridge, 0.0], [-hw - 0.4, ridge, 0.0], [-hw - 0.4, y_e, hd + 0.4], [hw + 0.4, y_e, hd + 0.4]], pal.roof);
    slab(&mut bt, [-hw, -hd], [hw, hd], h + 0.1, 0.2, None, pal.trim);
    // Racks and pallets.
    let rack = bt.b.material(mat("Rack", [0.15, 0.25, 0.5], 0.5, 0.5));
    let box_m = bt.b.material(mat("Cardboard", [0.45, 0.32, 0.18], 0.9, 0.0));
    let rows = ((d / 8.0) as usize).max(1);
    for r in 0..rows {
        let z = -hd * 0.4 + (r as f32) * 7.0;
        if z > hd - 3.0 {
            break;
        }
        bt.block([hw * 0.35, 1.8, z], [hw * 0.45, 1.8, 0.5], 0.0, rack);
        for k in 0..((hw * 0.9) as usize).max(1) {
            if rng.next() < 0.7 {
                bt.b.cuboid([hw * 0.35 - hw * 0.4 + k as f32 * 1.4, rng.range(0.7, 3.3), z - 0.1], [0.5, 0.4, 0.4], rng.range(-5.0, 5.0), box_m);
            }
        }
    }
    bt.lights.push(([0.0, h - 0.6, 0.0], [0.85, 0.9, 1.0], w.max(d) * 0.7));
    bt
}

/// A room (or sci-fi hall with glowing trim), walled with a doorway on the
/// front and the back and windows at the sides, floor and ceiling closed.
pub fn room(w: f32, d: f32, h: f32, seed: u32, scifi: bool) -> Built {
    let (w, d) = (w.max(2.2), d.max(2.2));
    let mut bt = Built::new([w, h + 0.3, d]);
    let mut rng = Rng::new(seed);
    let pal = pal_for(&mut bt, &mut rng, if scifi { "scifi" } else { "room" });
    let (hw, hd) = (w * 0.5, d * 0.5);
    let t = 0.2;
    let win = |len: f32| -> Vec<Opening> { if len > 4.0 && !scifi { vec![Opening::window(len * 0.5)] } else { vec![] } };
    wall(&mut bt, [-hw, -hd], [hw, -hd], 0.0, h, t, &[Opening::door(w * 0.5)], &pal);
    wall(&mut bt, [hw, hd], [-hw, hd], 0.0, h, t, &[Opening::door(w * 0.5)], &pal);
    wall(&mut bt, [-hw, hd], [-hw, -hd], 0.0, h, t, &win(d), &pal);
    wall(&mut bt, [hw, -hd], [hw, hd], 0.0, h, t, &win(d), &pal);
    slab(&mut bt, [-hw - t, -hd - t], [hw + t, hd + t], 0.12, 0.12, None, pal.floor);
    slab(&mut bt, [-hw - t, -hd - t], [hw + t, hd + t], h + 0.25, 0.25, None, pal.trim);
    if scifi {
        let cyan = bt.b.material(glow("Cyan strip", [0.2, 0.85, 1.0], 3.5));
        for (c, half, yaw) in [([0.0, 0.18, -hd + t * 0.6], [hw - t, 0.04, 0.03], 0.0), ([0.0, 0.18, hd - t * 0.6], [hw - t, 0.04, 0.03], 0.0), ([0.0, h - 0.15, -hd + t * 0.6], [hw - t, 0.05, 0.03], 0.0), ([0.0, h - 0.15, hd - t * 0.6], [hw - t, 0.05, 0.03], 0.0), ([-hw + t * 0.6, h - 0.15, 0.0], [hd - t, 0.05, 0.03], 90.0), ([hw - t * 0.6, h - 0.15, 0.0], [hd - t, 0.05, 0.03], 90.0)] {
            bt.b.cuboid(c, half, yaw, cyan);
        }
        // Wall panels.
        let panel = bt.b.material(mat("Panel", [0.1, 0.11, 0.14], 0.4, 0.7));
        let n = ((d / 1.6) as usize).max(1);
        for i in 0..n {
            let z = -hd + d * (i as f32 + 0.5) / n as f32;
            bt.b.cuboid([-hw + t * 0.5 + 0.03, h * 0.45, z], [0.03, h * 0.28, d / n as f32 * 0.38], 0.0, panel);
            bt.b.cuboid([hw - t * 0.5 - 0.03, h * 0.45, z], [0.03, h * 0.28, d / n as f32 * 0.38], 0.0, panel);
        }
        bt.lights.push(([0.0, h - 0.4, 0.0], [0.45, 0.85, 1.0], w.max(d) * 1.2));
    } else {
        bt.lights.push(([0.0, h - 0.35, 0.0], [1.0, 0.85, 0.62], w.max(d) * 1.4));
    }
    bt
}

/// A cactus, a dead tree, and other things the flora set lacks.
pub fn plant(kind: &str, size: f32, seed: u32) -> Option<Mesh> {
    let mut b = MeshBuilder::default();
    let mut rng = Rng::new(seed);
    match kind {
        "cactus" => {
            let m = b.material(mat("Cactus", [0.1, 0.25, 0.1], 0.7, 0.0));
            b.frustum([0.0, 0.0, 0.0], size * 0.07, size * 0.06, size * 0.85, 10, m);
            for s in [-1.0f32, 1.0] {
                if rng.next() < 0.8 {
                    let y = size * rng.range(0.3, 0.55);
                    b.cuboid([s * size * 0.13, y, 0.0], [size * 0.07, size * 0.015, size * 0.015], 0.0, m);
                    b.frustum([s * size * 0.2, y, 0.0], size * 0.045, size * 0.04, size * rng.range(0.25, 0.4), 8, m);
                }
            }
        }
        "dead_tree" | "deadwood" => {
            let m = b.material(mat("Deadwood", [0.2, 0.16, 0.12], 0.95, 0.0));
            b.frustum([0.0, 0.0, 0.0], size * 0.045, size * 0.02, size * 0.7, 7, m);
            for k in 0..5 {
                let y = size * (0.3 + 0.1 * k as f32);
                let a = rng.range(0.0, 360.0);
                let len = size * rng.range(0.15, 0.3);
                let (s, c) = a.to_radians().sin_cos();
                // A limb leaning out: a thin box tilted via its corners.
                let tip = [s * len, y + len * 0.7, c * len];
                let base = [0.0, y, 0.0];
                let side = crate::math::norm(crate::math::cross(crate::math::sub(tip, base), [0.0, 1.0, 0.0]));
                let w = size * 0.012;
                let q = |p: V3, o: f32| [p[0] + side[0] * o, p[1], p[2] + side[2] * o];
                b.quad([q(base, -w), q(base, w), q(tip, w * 0.3), q(tip, -w * 0.3)], m);
                b.quad([q(base, w), q(base, -w), q(tip, -w * 0.3), q(tip, w * 0.3)], m);
            }
        }
        "lamp" | "streetlight" => {
            let m = b.material(mat("Lamp post", [0.08, 0.09, 0.1], 0.5, 0.8));
            let g = b.material(glow("Lamp", [1.0, 0.85, 0.55], 6.0));
            b.frustum([0.0, 0.0, 0.0], size * 0.018, size * 0.012, size, 8, m);
            b.cuboid([size * 0.1, size, 0.0], [size * 0.11, size * 0.012, size * 0.012], 0.0, m);
            b.cuboid([size * 0.2, size - size * 0.02, 0.0], [size * 0.05, size * 0.015, size * 0.03], 0.0, g);
        }
        "fence" => {
            let m = b.material(mat("Wood fence", [0.35, 0.25, 0.15], 0.9, 0.0));
            for k in 0..5 {
                b.cuboid([k as f32 * size * 0.25 - size * 0.5, size * 0.3, 0.0], [size * 0.015, size * 0.3, size * 0.015], 0.0, m);
            }
            for y in [0.2, 0.45] {
                b.cuboid([0.0, size * y, 0.0], [size * 0.5, size * 0.02, size * 0.01], 0.0, m);
            }
        }
        "car" => {
            let body = b.material(mat("Car paint", [rng.range(0.1, 0.8), rng.range(0.1, 0.5), rng.range(0.1, 0.5)], 0.25, 0.4));
            let glass = b.material(mat("Car glass", [0.03, 0.05, 0.07], 0.05, 0.4));
            let tyre = b.material(mat("Tyre", [0.02, 0.02, 0.02], 0.9, 0.0));
            let l = size;
            b.cuboid([0.0, l * 0.17, 0.0], [l * 0.21, l * 0.07, l * 0.5], 0.0, body);
            b.cuboid([0.0, l * 0.3, -l * 0.04], [l * 0.19, l * 0.06, l * 0.25], 0.0, glass);
            for (x, z) in [(-1.0, -0.32), (1.0, -0.32), (-1.0, 0.32), (1.0, 0.32)] {
                b.cuboid([x * l * 0.2, l * 0.08, z * l], [l * 0.04, l * 0.08, l * 0.08], 0.0, tyre);
            }
        }
        "crate" => {
            let m = b.material(mat("Crate", [0.4, 0.28, 0.15], 0.9, 0.0));
            b.cuboid([0.0, size * 0.5, 0.0], [size * 0.5, size * 0.5, size * 0.5], 0.0, m);
        }
        _ => return None,
    }
    Some(b.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::walk::{Body, Input, Params};
    use crate::world::World;

    #[test]
    fn every_structure_is_a_real_closed_mesh_with_materials_and_solids() {
        for k in STRUCTURES {
            let b = structure(k, 9.0, 8.0, 3, 4).unwrap_or_else(|| panic!("{k}"));
            let (m, solids, lights) = b.mesh();
            assert!(m.triangles() > 40, "{k}");
            assert!(!solids.is_empty() && !lights.is_empty(), "{k}");
            assert_eq!(m.normals.len(), m.positions.len());
            assert!(m.groups.iter().map(|g| g.count as usize).sum::<usize>() == m.triangles(), "{k}");
            let bb = crate::asset::mesh_bounds(&m);
            let (lo, hi) = (bb.min, bb.max);
            assert!(lo[1] >= -0.01 && hi[1] > 2.0, "{k}: {lo:?} {hi:?}");
        }
        assert!(structure("castle", 5.0, 5.0, 1, 1).is_none());
    }

    #[test]
    fn faces_point_out_of_the_walls() {
        let (m, _, _) = house(8.0, 6.0, 1, 2).mesh();
        // The front wall's outer face (z = -3.125) must have a triangle
        // facing -z.
        let mut found = false;
        for t in 0..m.triangles() {
            let [a, b, c] = m.tri(t).map(|i| m.positions[i]);
            let n = crate::math::cross(crate::math::sub(b, a), crate::math::sub(c, a));
            if a[2] < -3.1 && b[2] < -3.1 && c[2] < -3.1 && n[2] < -1e-4 {
                found = true;
            }
        }
        assert!(found);
    }

    fn world_with(b: Built) -> World {
        let mut w = World::new("k");
        w.terrain = Some(crate::terrain::Terrain::flat([128.0, 128.0], 1.0));
        let (m, solids, _) = b.mesh();
        let _ = m;
        w.extra_solids = solids;
        w.finish();
        w
    }

    #[test]
    fn you_can_walk_in_the_front_door_but_not_through_the_wall_and_climb_the_stairs() {
        let w = world_with(house(8.0, 6.0, 2, 3));
        let p = Params::default();
        // Through a wall (side): blocked.
        let mut b = Body::at([-10.0, 0.0, 0.0]);
        b.settle(&w, &p);
        for _ in 0..360 {
            b.step(&w, &p, &Input { forward: 0.0, right: 1.0, ..Default::default() }, 0.0, 1.0 / 60.0);
        }
        assert!(b.pos[0] < -3.9, "stopped at the wall: {:?}", b.pos);
        // From the front, aimed at the door: gets inside. Find the door by
        // looking for a free corridor along z.
        let mut inside = false;
        for k in 0..70 {
            let x = -3.4 + k as f32 * 0.1;
            let mut c = Body::at([x, 0.0, -8.0]);
            c.settle(&w, &p);
            for _ in 0..300 {
                c.step(&w, &p, &Input { forward: 1.0, ..Default::default() }, 0.0, 1.0 / 60.0);
            }
            if c.pos[2] > -1.0 {
                inside = true;
            }
        }
        assert!(inside, "a doorway admits a walker");
    }

    #[test]
    fn the_stairs_of_a_two_storey_house_lead_up_to_the_first_floor() {
        let w = world_with(house(8.0, 6.0, 2, 3));
        let p = Params::default();
        // The stair rises along +z from its foot, near the left wall.
        let (sx, sz0) = (-4.0 + 0.9 + 0.25, -3.0 + 0.25 + 0.4);
        let mut b = Body::at([sx, 0.2, sz0 - 0.5]);
        b.settle(&w, &p);
        for _ in 0..240 {
            b.step(&w, &p, &Input { forward: 1.0, ..Default::default() }, 0.0, 1.0 / 60.0);
        }
        assert!((b.pos[1] - 3.0).abs() < 0.15 && b.grounded, "on the first floor: {:?}", b.pos);
        assert!(b.pos[2] > sz0 + 3.0);
    }

    #[test]
    fn plants_and_props_exist() {
        for k in ["cactus", "dead_tree", "lamp", "fence", "car", "crate"] {
            let m = plant(k, 3.0, 1).unwrap();
            assert!(m.triangles() > 6, "{k}");
        }
    }
}

/// Furniture and fittings: (mesh, box collider half sizes and centre).
pub fn furniture(kind: &str, size: f32, seed: u32) -> Option<Mesh> {
    let mut b = MeshBuilder::default();
    let mut rng = Rng::new(seed);
    let s = size;
    match kind {
        "sofa" => {
            let fabric = b.material(mat("Sofa fabric", [rng.range(0.05, 0.4), rng.range(0.05, 0.25), rng.range(0.05, 0.3)], 0.95, 0.0));
            b.cuboid([0.0, s * 0.14, 0.0], [s * 0.5, s * 0.14, s * 0.22], 0.0, fabric);
            b.cuboid([0.0, s * 0.33, s * 0.19], [s * 0.5, s * 0.17, s * 0.06], 0.0, fabric);
            for x in [-1.0f32, 1.0] {
                b.cuboid([x * s * 0.46, s * 0.26, 0.0], [s * 0.05, s * 0.12, s * 0.22], 0.0, fabric);
            }
        }
        "bed" => {
            let frame = b.material(mat("Bed frame", [0.25, 0.16, 0.1], 0.8, 0.0));
            let sheet = b.material(mat("Bedding", [0.75, 0.74, 0.7], 0.95, 0.0));
            b.cuboid([0.0, s * 0.1, 0.0], [s * 0.22, s * 0.1, s * 0.5], 0.0, frame);
            b.cuboid([0.0, s * 0.23, -s * 0.03], [s * 0.2, s * 0.05, s * 0.45], 0.0, sheet);
            b.cuboid([0.0, s * 0.3, -s * 0.52], [s * 0.22, s * 0.2, s * 0.02], 0.0, frame);
            b.cuboid([0.0, s * 0.3, -s * 0.38], [s * 0.12, s * 0.04, s * 0.07], 0.0, sheet);
        }
        "shelf" | "bookcase" => {
            let wood = b.material(mat("Shelf wood", [0.3, 0.2, 0.12], 0.85, 0.0));
            let book = b.material(mat("Books", [0.5, 0.15, 0.12], 0.8, 0.0));
            b.cuboid([-s * 0.24, s * 0.5, 0.0], [s * 0.015, s * 0.5, s * 0.08], 0.0, wood);
            b.cuboid([s * 0.24, s * 0.5, 0.0], [s * 0.015, s * 0.5, s * 0.08], 0.0, wood);
            for k in 0..5 {
                let y = s * (0.02 + 0.24 * k as f32);
                b.cuboid([0.0, y, 0.0], [s * 0.25, s * 0.012, s * 0.08], 0.0, wood);
                if k < 4 && rng.next() < 0.9 {
                    b.cuboid([rng.range(-0.1, 0.1) * s, y + s * 0.08, 0.0], [s * 0.12, s * 0.07, s * 0.05], 0.0, book);
                }
            }
        }
        "desk" | "counter" => {
            let wood = b.material(mat("Desk wood", [0.35, 0.25, 0.15], 0.7, 0.0));
            b.cuboid([0.0, s * 0.37, 0.0], [s * 0.5, s * 0.015, s * 0.25], 0.0, wood);
            for (x, z) in [(-1.0f32, -1.0f32), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
                b.cuboid([x * s * 0.46, s * 0.18, z * s * 0.21], [s * 0.015, s * 0.18, s * 0.015], 0.0, wood);
            }
        }
        "lamp_table" | "floor_lamp" => {
            let m = b.material(mat("Lamp body", [0.1, 0.1, 0.1], 0.5, 0.6));
            let g = b.material(glow("Lampshade", [1.0, 0.85, 0.6], 3.0));
            b.frustum([0.0, 0.0, 0.0], s * 0.03, s * 0.012, s * 0.8, 8, m);
            b.cuboid([0.0, s * 0.85, 0.0], [s * 0.09, s * 0.08, s * 0.09], 0.0, g);
        }
        "pot" | "plant_pot" => {
            let clay = b.material(mat("Clay", [0.45, 0.2, 0.12], 0.9, 0.0));
            let leaf = b.material(mat("Houseplant", [0.08, 0.28, 0.08], 0.7, 0.0));
            b.frustum([0.0, 0.0, 0.0], s * 0.12, s * 0.16, s * 0.25, 10, clay);
            for k in 0..6 {
                let a = k as f32 * 60.0;
                let (sa, ca) = a.to_radians().sin_cos();
                b.cuboid([sa * s * 0.08, s * 0.5, ca * s * 0.08], [s * 0.03, s * 0.22, s * 0.01], a, leaf);
            }
        }
        _ => return None,
    }
    Some(b.finish())
}
