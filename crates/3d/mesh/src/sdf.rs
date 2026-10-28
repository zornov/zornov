// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Modeling with blended shapes, the way clay is pushed together: each
//! part (ball, rounded box, capsule, cylinder, torus) melts into the ones
//! before it over a blend distance, or carves them away. The result is a
//! distance field, turned into triangles by surface nets; normals come
//! from the field, so the surface is smooth where parts meet. Organic
//! forms (heads, bodies, creatures, soft products) come out whole and
//! seamless, ready for sculpting.
//!
//! A part can also be any distance field ([`Shape::Field`] with a
//! [`FieldFn`]): a formula, a minimal surface, a fractal (see
//! [`crate::math`]), blending and carving like the rest.

use crate::{Group, Material, Mesh, V3, add, dot, length, normalise, scale, sub};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Shape {
    /// A ball stretched to `size` (diameters).
    Ellipsoid,
    /// A box `size` across with edges rounded by `round` px.
    Box,
    /// A pill from `a` to `b` (part space) with radius `size[0] / 2`.
    Capsule,
    /// An upright cylinder (along y), `size[0]` across, `size[1]` tall.
    Cylinder,
    /// A ring in the x/z plane: `size[0]` across, tube `size[1]` thick.
    Torus,
    /// A tapering pill from `a` (radius `size[0] / 2`) to `b` (radius
    /// `size[1] / 2`): beaks, horns, claws, tails, legs, fingers.
    Cone,
    /// A box `size` across cut on the diagonal from its bottom at −x to its
    /// top at +x: a ramp, a wedge, a doorstop, a roof.
    Wedge,
    /// Any field filling the box `size` across: the part's `field`.
    Field,
}

/// A distance field a part can be: worked out in the part's own space
/// (pixels from its centre, before its turn), the part `size` px across.
/// Negative inside; near the surface about the distance in pixels.
pub trait FieldFn: Send + Sync {
    fn distance(&self, q: V3, size: V3) -> f32;
}

/// A shared [`FieldFn`] on a part: parts holding the same one are equal.
#[derive(Clone)]
pub struct FieldRef(pub std::sync::Arc<dyn FieldFn>);

impl PartialEq for FieldRef {
    fn eq(&self, other: &Self) -> bool {
        std::sync::Arc::ptr_eq(&self.0, &other.0)
    }
}

impl std::fmt::Debug for FieldRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FieldRef")
    }
}

impl Shape {
    pub fn parse(s: &str) -> Option<Shape> {
        Some(match s {
            "ellipsoid" | "sphere" | "ball" | "egg" => Shape::Ellipsoid,
            "box" | "cube" | "rounded_box" => Shape::Box,
            "capsule" | "pill" | "limb" | "tube" => Shape::Capsule,
            "cylinder" => Shape::Cylinder,
            "torus" | "ring" | "donut" => Shape::Torus,
            "cone" | "taper" | "tapered_capsule" | "round_cone" | "horn" | "beak" | "claw" | "spike" | "tooth" | "fang" | "tail" => Shape::Cone,
            "wedge" | "ramp" | "prism" => Shape::Wedge,
            // Thin parts are flattened balls with real thickness.
            "sheet" | "fan" | "fin" | "wing" | "leaf" | "feather" | "petal" | "ear" | "comb" | "crest" | "blade" | "disc" | "disk" | "lens" => Shape::Ellipsoid,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Join {
    Add,
    Subtract,
    Intersect,
}

/// One shape in the mix.
#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    pub shape: Shape,
    pub at: V3,
    pub size: V3,
    /// Degrees about x, y, z.
    pub turn: V3,
    /// Capsule ends (relative to `at`); ignored by other shapes.
    pub a: V3,
    pub b: V3,
    pub round: f32,
    pub join: Join,
    /// How far it melts into the others (px); 0 = a hard edge.
    pub blend: f32,
    /// Index into the materials; the surface takes the material of the
    /// part it is nearest.
    pub material: u32,
    /// What a [`Shape::Field`] part is.
    pub field: Option<FieldRef>,
}

impl Part {
    pub fn new(shape: Shape, at: V3, size: V3) -> Part {
        Part { shape, at, size, turn: [0.0; 3], a: [0.0; 3], b: [0.0; 3], round: 0.0, join: Join::Add, blend: 0.0, material: 0, field: None }
    }

    /// A part that is the field `f`, filling the box `size` px across.
    pub fn field(f: std::sync::Arc<dyn FieldFn>, at: V3, size: V3) -> Part {
        Part { field: Some(FieldRef(f)), ..Part::new(Shape::Field, at, size) }
    }
}

fn smin(a: f32, b: f32, k: f32) -> f32 {
    if k <= 0.0 {
        return a.min(b);
    }
    let h = (0.5 + 0.5 * (b - a) / k).clamp(0.0, 1.0);
    b + (a - b) * h - k * h * (1.0 - h)
}

fn smax(a: f32, b: f32, k: f32) -> f32 {
    -smin(-a, -b, k)
}

/// A part ready to evaluate: world → part rotation (rows).
struct Prepared {
    part: Part,
    inv: [V3; 3],
}

fn prepare(p: &Part) -> Prepared {
    let r = crate::ops::rotation(p.turn);
    // The inverse of a rotation is its transpose.
    let inv = [[r[0][0], r[1][0], r[2][0]], [r[0][1], r[1][1], r[2][1]], [r[0][2], r[1][2], r[2][2]]];
    Prepared { part: p.clone(), inv }
}

fn shape_distance(pp: &Prepared, w: V3) -> f32 {
    let p = &pp.part;
    let d = sub(w, p.at);
    let q = [dot(pp.inv[0], d), dot(pp.inv[1], d), dot(pp.inv[2], d)];
    match p.shape {
        Shape::Ellipsoid => {
            // A good bound for ellipsoids (Quilez).
            let r = [(p.size[0] * 0.5).max(1e-3), (p.size[1] * 0.5).max(1e-3), (p.size[2] * 0.5).max(1e-3)];
            let k0 = length([q[0] / r[0], q[1] / r[1], q[2] / r[2]]);
            let k1 = length([q[0] / (r[0] * r[0]), q[1] / (r[1] * r[1]), q[2] / (r[2] * r[2])]);
            if k1 < 1e-9 { -r[0].min(r[1]).min(r[2]) } else { k0 * (k0 - 1.0) / k1 }
        }
        Shape::Box => {
            let rr = p.round.min(p.size[0].min(p.size[1]).min(p.size[2]) * 0.5).max(0.0);
            let h = [p.size[0] * 0.5 - rr, p.size[1] * 0.5 - rr, p.size[2] * 0.5 - rr];
            let e = [q[0].abs() - h[0], q[1].abs() - h[1], q[2].abs() - h[2]];
            length([e[0].max(0.0), e[1].max(0.0), e[2].max(0.0)]) + e[0].max(e[1]).max(e[2]).min(0.0) - rr
        }
        Shape::Capsule => {
            let ab = sub(p.b, p.a);
            let aq = sub(q, p.a);
            let t = (dot(aq, ab) / dot(ab, ab).max(1e-9)).clamp(0.0, 1.0);
            length(sub(aq, scale(ab, t))) - p.size[0] * 0.5
        }
        Shape::Cylinder => {
            let r = p.size[0] * 0.5;
            let h = p.size[1] * 0.5;
            let dx = length([q[0], 0.0, q[2]]) - r;
            let dy = q[1].abs() - h;
            dx.max(dy).min(0.0) + length([dx.max(0.0), dy.max(0.0), 0.0])
        }
        Shape::Torus => {
            let big = p.size[0] * 0.5 - p.size[1] * 0.5;
            let t = [length([q[0], 0.0, q[2]]) - big, q[1], 0.0];
            length(t) - p.size[1] * 0.5
        }
        Shape::Cone => round_cone(q, p.a, p.b, (p.size[0] * 0.5).max(1e-3), (p.size[1] * 0.5).max(1e-3)),
        Shape::Wedge => {
            let h = [p.size[0] * 0.5, p.size[1] * 0.5, p.size[2] * 0.5];
            let e = [q[0].abs() - h[0], q[1].abs() - h[1], q[2].abs() - h[2]];
            let boxd = length([e[0].max(0.0), e[1].max(0.0), e[2].max(0.0)]) + e[0].max(e[1]).max(e[2]).min(0.0);
            // Keep what is under the line from (−w/2, −h/2) to (w/2, h/2).
            let n = normalise([p.size[1], -p.size[0], 0.0]);
            boxd.max(-dot(n, [q[0], q[1], 0.0]))
        }
        Shape::Field => p.field.as_ref().map_or(f32::INFINITY, |f| f.0.distance(q, p.size)),
    }
}

/// Distance to a pill whose radius runs from `r1` at `a` to `r2` at `b`
/// (Quilez's round cone).
fn round_cone(p: V3, a: V3, b: V3, r1: f32, r2: f32) -> f32 {
    let ba = sub(b, a);
    let l2 = dot(ba, ba);
    if l2 < 1e-9 {
        return length(sub(p, a)) - r1.max(r2);
    }
    let rr = r1 - r2;
    let a2 = l2 - rr * rr;
    let il2 = 1.0 / l2;
    let pa = sub(p, a);
    let y = dot(pa, ba);
    let z = y - l2;
    let xv = sub(scale(pa, l2), scale(ba, y));
    let x2 = dot(xv, xv);
    let y2 = y * y * l2;
    let z2 = z * z * l2;
    let k = rr.signum() * rr * rr * x2;
    if z.signum() * a2 * z2 > k {
        return (x2 + z2).sqrt() * il2 - r2;
    }
    if y.signum() * a2 * y2 < k {
        return (x2 + y2).sqrt() * il2 - r1;
    }
    ((x2 * a2 * il2).max(0.0).sqrt() + y * rr) * il2 - r1
}

/// The thinnest dimension of a part that adds (px): what the grid must be
/// fine enough to keep.
pub fn thinnest(p: &Part) -> f32 {
    match p.shape {
        Shape::Ellipsoid | Shape::Box | Shape::Wedge | Shape::Field => p.size[0].min(p.size[1]).min(p.size[2]),
        Shape::Capsule => p.size[0],
        Shape::Cone => p.size[0].min(p.size[1]),
        Shape::Cylinder => p.size[0].min(p.size[1]),
        Shape::Torus => p.size[1],
    }
}

/// The whole field at `w`, and the material there: like paint, each
/// part colours the surface within `near` of itself (and a little into
/// its blend), later parts over earlier ones.
fn field(parts: &[Prepared], w: V3, near: f32) -> (f32, u32) {
    let mut d = f32::INFINITY;
    let mut mat = 0u32;
    for pp in parts {
        let s = shape_distance(pp, w);
        let p = &pp.part;
        match p.join {
            Join::Add => {
                d = if d.is_infinite() { s } else { smin(d, s, p.blend) };
                if s <= near + p.blend * 0.35 {
                    mat = p.material;
                }
            }
            Join::Subtract => d = smax(d, -s, p.blend),
            Join::Intersect => d = smax(d, s, p.blend),
        }
    }
    (d, mat)
}

/// Builds the surface of the parts. `cell` is the grid step in pixels
/// (smaller is finer and slower; about a hundredth of the size is good).
pub fn build(parts: &[Part], materials: Vec<Material>, cell: f32) -> Mesh {
    build_counted(parts, materials, cell).0
}

/// The most grid points along a side: a finer `cell` is made coarser.
pub const MAX_GRID: f32 = 400.0;

/// What a build cost: the grid it sampled and how many times it worked out
/// the field (each sample works out every part once).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Cost {
    pub grid: [usize; 3],
    pub samples: u64,
    pub cell: f32,
}

/// The parts' field as a function of a point, for following the surface
/// (projecting points onto it, say).
pub fn sampler(parts: &[Part]) -> impl Fn(V3) -> f32 + Sync + use<> {
    let prepared: Vec<Prepared> = parts.iter().map(prepare).collect();
    move |w| field(&prepared, w, 0.0).0
}

/// [`build`], saying what it cost.
pub fn build_counted(parts: &[Part], materials: Vec<Material>, cell: f32) -> (Mesh, Cost) {
    let prepared: Vec<Prepared> = parts.iter().map(prepare).collect();
    // Bounds of what is added, with room for the blends.
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    for p in parts.iter().filter(|p| p.join == Join::Add) {
        let reach = match p.shape {
            Shape::Capsule => length(p.a).max(length(p.b)) + p.size[0],
            Shape::Cone => length(p.a).max(length(p.b)) + p.size[0].max(p.size[1]),
            // A box's corner, however it is turned.
            Shape::Field => length(p.size) * 0.67,
            _ => p.size[0].max(p.size[1]).max(p.size[2]),
        } * 0.75
            + p.blend;
        // A field not turned fills its own box and no more.
        let half: V3 = if p.shape == Shape::Field && p.turn == [0.0; 3] { std::array::from_fn(|i| p.size[i] * 0.5 + p.blend) } else { [reach; 3] };
        for i in 0..3 {
            lo[i] = lo[i].min(p.at[i] - half[i]);
            hi[i] = hi[i].max(p.at[i] + half[i]);
        }
    }
    if !lo[0].is_finite() {
        return (Mesh::default(), Cost::default());
    }
    let cell = cell.max((hi[0] - lo[0]).max(hi[1] - lo[1]).max(hi[2] - lo[2]) / MAX_GRID).max(0.05);
    // A field cut flat by its box needs a cell of room past the cut.
    if parts.iter().any(|p| p.shape == Shape::Field) {
        for i in 0..3 {
            lo[i] -= cell;
            hi[i] += cell;
        }
    }
    let n: [usize; 3] = std::array::from_fn(|i| (((hi[i] - lo[i]) / cell).ceil() as usize + 2).max(2));
    let origin = [lo[0] - cell, lo[1] - cell, lo[2] - cell];
    let at = |x: usize, y: usize, z: usize| [origin[0] + x as f32 * cell, origin[1] + y as f32 * cell, origin[2] + z as f32 * cell];
    // The field at every grid point, slices in parallel.
    let mut values = vec![0f32; n[0] * n[1] * n[2]];
    {
        let threads = std::thread::available_parallelism().map_or(4, |t| t.get()).clamp(1, 16);
        let slice = n[0] * n[1];
        let next = std::sync::atomic::AtomicUsize::new(0);
        let chunks: Vec<std::sync::Mutex<&mut [f32]>> = values.chunks_mut(slice).map(std::sync::Mutex::new).collect();
        std::thread::scope(|s| {
            for _ in 0..threads {
                s.spawn(|| {
                    loop {
                        let z = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        if z >= n[2] {
                            break;
                        }
                        let mut out = chunks[z].lock().unwrap_or_else(|e| e.into_inner());
                        for y in 0..n[1] {
                            for x in 0..n[0] {
                                out[y * n[0] + x] = field(&prepared, at(x, y, z), 0.0).0;
                            }
                        }
                    }
                });
            }
        });
    }
    let v = |x: usize, y: usize, z: usize| values[(z * n[1] + y) * n[0] + x];
    // Surface nets: a vertex in every cell the surface passes through, at
    // the average of where it crosses the cell's edges. A slice of cells
    // at a time: a quad joins the cells round a grid edge, which lie in
    // this slice and the one before, so two slices of vertex numbers are
    // all that is kept (a whole grid of them was 4 bytes a grid point,
    // 240 MB at the evaluation cap).
    let slice = n[0] * n[1];
    let (mut before, mut here) = (vec![u32::MAX; slice], vec![u32::MAX; slice]);
    let mut mesh = Mesh { materials, ..Default::default() };
    const EDGES: [([usize; 3], [usize; 3]); 12] = [
        ([0, 0, 0], [1, 0, 0]),
        ([0, 1, 0], [1, 1, 0]),
        ([0, 0, 1], [1, 0, 1]),
        ([0, 1, 1], [1, 1, 1]),
        ([0, 0, 0], [0, 1, 0]),
        ([1, 0, 0], [1, 1, 0]),
        ([0, 0, 1], [0, 1, 1]),
        ([1, 0, 1], [1, 1, 1]),
        ([0, 0, 0], [0, 0, 1]),
        ([1, 0, 0], [1, 0, 1]),
        ([0, 1, 0], [0, 1, 1]),
        ([1, 1, 0], [1, 1, 1]),
    ];
    // A quad across every grid edge the surface crosses, wound outwards.
    let mut quads: Vec<[u32; 4]> = Vec::new();
    for z in 0..n[2] - 1 {
        std::mem::swap(&mut before, &mut here);
        here.fill(u32::MAX);
        for y in 0..n[1] - 1 {
            for x in 0..n[0] - 1 {
                let mut sum = [0.0f32; 3];
                let mut count = 0;
                for (a, b) in EDGES {
                    let va = v(x + a[0], y + a[1], z + a[2]);
                    let vb = v(x + b[0], y + b[1], z + b[2]);
                    if (va < 0.0) != (vb < 0.0) {
                        let t = va / (va - vb);
                        let pa = at(x + a[0], y + a[1], z + a[2]);
                        let pb = at(x + b[0], y + b[1], z + b[2]);
                        sum = add(sum, crate::lerp(pa, pb, t));
                        count += 1;
                    }
                }
                if count > 0 {
                    here[y * n[0] + x] = mesh.positions.len() as u32;
                    mesh.positions.push(scale(sum, 1.0 / count as f32));
                }
            }
        }
        // The grid edges from the points of layer z: their cells are in
        // slices z - 1 (`before`) and z (`here`).
        if z == 0 {
            continue;
        }
        let cell_id = |x: usize, y: usize, s: &[u32]| s[y * n[0] + x];
        for y in 1..n[1] - 1 {
            for x in 1..n[0] - 1 {
                let inside = v(x, y, z) < 0.0;
                for axis in 0..3 {
                    let (dx, dy, dz) = [(1, 0, 0), (0, 1, 0), (0, 0, 1)][axis];
                    if x + dx >= n[0] || y + dy >= n[1] || z + dz >= n[2] {
                        continue;
                    }
                    let there = v(x + dx, y + dy, z + dz) < 0.0;
                    if inside == there {
                        continue;
                    }
                    let c = match axis {
                        0 => [cell_id(x, y - 1, &before), cell_id(x, y, &before), cell_id(x, y, &here), cell_id(x, y - 1, &here)],
                        1 => [cell_id(x - 1, y, &before), cell_id(x - 1, y, &here), cell_id(x, y, &here), cell_id(x, y, &before)],
                        _ => [cell_id(x - 1, y - 1, &here), cell_id(x, y - 1, &here), cell_id(x, y, &here), cell_id(x - 1, y, &here)],
                    };
                    if c.contains(&u32::MAX) {
                        continue;
                    }
                    quads.push(if inside { c } else { [c[3], c[2], c[1], c[0]] });
                }
            }
        }
    }
    let mut mats = Vec::with_capacity(quads.len() * 2);
    // Normals from the field's gradient, material from the nearest part.
    let eps = cell * 0.5;
    let grad = |p: V3| {
        let f = |q: V3| field(&prepared, q, 0.0).0;
        normalise([f(add(p, [eps, 0.0, 0.0])) - f(sub(p, [eps, 0.0, 0.0])), f(add(p, [0.0, eps, 0.0])) - f(sub(p, [0.0, eps, 0.0])), f(add(p, [0.0, 0.0, eps])) - f(sub(p, [0.0, 0.0, eps]))])
    };
    mesh.normals = mesh.positions.iter().map(|p| grad(*p)).collect();
    let vmat: Vec<u32> = mesh.positions.iter().map(|p| field(&prepared, *p, cell * 0.75).1).collect();
    let cost = Cost { grid: n, samples: (n[0] * n[1] * n[2]) as u64 + mesh.positions.len() as u64 * 7, cell };
    for q in quads {
        // Split along the shorter diagonal.
        let d02 = length(sub(mesh.positions[q[0] as usize], mesh.positions[q[2] as usize]));
        let d13 = length(sub(mesh.positions[q[1] as usize], mesh.positions[q[3] as usize]));
        let tris = if d02 <= d13 { [[q[0], q[1], q[2]], [q[0], q[2], q[3]]] } else { [[q[0], q[1], q[3]], [q[1], q[2], q[3]]] };
        for t in tris {
            // The winding the field's normal agrees with.
            let (a, b, c) = (mesh.positions[t[0] as usize], mesh.positions[t[1] as usize], mesh.positions[t[2] as usize]);
            let face = crate::cross(sub(b, a), sub(c, a));
            let n = add(add(mesh.normals[t[0] as usize], mesh.normals[t[1] as usize]), mesh.normals[t[2] as usize]);
            if dot(face, n) < 0.0 {
                mesh.indices.extend_from_slice(&[t[0], t[2], t[1]]);
            } else {
                mesh.indices.extend_from_slice(&t);
            }
            // The corners' most common material.
            let m = [vmat[t[0] as usize], vmat[t[1] as usize], vmat[t[2] as usize]];
            mats.push(if m[1] == m[2] { m[1] } else { m[0] });
        }
    }
    mesh.uvs = vec![[0.0; 2]; mesh.positions.len()];
    if mesh.materials.is_empty() {
        mesh.materials.push(Material::default());
    }
    let top = mesh.materials.len() as u32 - 1;
    for m in &mut mats {
        *m = (*m).min(top);
    }
    mesh.regroup(&mats);
    let _ = Group { start: 0, count: 0, material: 0 };
    let _ = crate::ops::project_uvs(&mut mesh, "sphere");
    (mesh, cost)
}

/// An anatomical human head (and neck), `size` px from the chin to the
/// top of the skull, face towards -z: a realistic base to sculpt from.
/// Materials: 0 skin, 1 lips, 2 eye whites, 3 irises, 4 pupils, 5 brows.
pub fn head(size: f32, detail: usize) -> Mesh {
    head_with(size, detail, [58, 40, 32])
}

/// [`head`] with the colour of the brows (sRGB).
pub fn head_with(size: f32, detail: usize, brow: [u8; 3]) -> Mesh {
    head_full(size, detail, brow, true)
}

/// [`head_with`] choosing whether the lids are sculpted into the skin
/// (`true`: a bust that blinks by its shape keys) or left out for separate
/// lid parts (`false`, see [`crate::facekit`]).
pub fn head_full(size: f32, detail: usize, brow: [u8; 3], sculpt_lids: bool) -> Mesh {
    let s = size;
    let p = |shape: Shape, at: V3, sz: V3| Part::new(shape, [at[0] * s, at[1] * s, at[2] * s], [sz[0] * s, sz[1] * s, sz[2] * s]);
    let k = |part: Part, blend: f32| Part { blend: blend * s, ..part };
    let cap = |a: V3, b: V3, r: f32, blend: f32, mat: u32| Part { a: scale(a, s), b: scale(b, s), blend: blend * s, material: mat, ..Part::new(Shape::Capsule, [0.0; 3], [r * 2.0 * s; 3]) };
    let sub_ = |part: Part, blend: f32| Part { join: Join::Subtract, blend: blend * s, material: u32::MAX, ..part };
    let turned = |part: Part, turn: V3| Part { turn, ..part };
    let mat = |part: Part, m: u32| Part { material: m, ..part };
    let mut parts = vec![
        // Cranium: an egg wider at the back and top.
        p(Shape::Ellipsoid, [0.0, -0.13, 0.07], [0.68, 0.8, 0.86]),
        // Face mass and forehead.
        k(p(Shape::Ellipsoid, [0.0, 0.06, -0.12], [0.56, 0.74, 0.58]), 0.08),
        k(p(Shape::Ellipsoid, [0.0, -0.2, -0.21], [0.54, 0.36, 0.4]), 0.08),
        // Jaw and chin.
        k(turned(p(Shape::Ellipsoid, [0.0, 0.28, -0.06], [0.44, 0.26, 0.5]), [8.0, 0.0, 0.0]), 0.08),
        k(p(Shape::Ellipsoid, [0.0, 0.41, -0.265], [0.16, 0.12, 0.14]), 0.06),
        // The corners of the jaw below the ears.
        k(p(Shape::Ellipsoid, [0.19, 0.25, 0.0], [0.08, 0.12, 0.14]), 0.1),
        k(p(Shape::Ellipsoid, [-0.19, 0.25, 0.0], [0.08, 0.12, 0.14]), 0.1),
        // Cheekbones.
        k(p(Shape::Ellipsoid, [0.21, 0.085, -0.2], [0.15, 0.085, 0.14]), 0.06),
        k(p(Shape::Ellipsoid, [-0.21, 0.085, -0.2], [0.15, 0.085, 0.14]), 0.06),
        // Brow ridge.
        cap([-0.16, -0.075, -0.34], [0.16, -0.075, -0.34], 0.045, 0.05, 0),
        // Neck.
        cap([0.0, 0.3, 0.04], [0.0, 0.72, 0.1], 0.2, 0.1, 0),
        // Nose: bridge, tip, wings.
        cap([0.0, -0.04, -0.38], [0.0, 0.15, -0.47], 0.034, 0.04, 0),
        k(p(Shape::Ellipsoid, [0.0, 0.17, -0.47], [0.085, 0.08, 0.085]), 0.03),
        k(p(Shape::Ellipsoid, [0.045, 0.185, -0.425], [0.06, 0.055, 0.065]), 0.025),
        k(p(Shape::Ellipsoid, [-0.045, 0.185, -0.425], [0.06, 0.055, 0.065]), 0.025),
        // Lips.
        mat(k(p(Shape::Ellipsoid, [0.0, 0.27, -0.355], [0.13, 0.03, 0.05]), 0.025), 1),
        mat(k(p(Shape::Ellipsoid, [0.0, 0.302, -0.343], [0.12, 0.036, 0.05]), 0.025), 1),
        // Ears.
        k(turned(p(Shape::Ellipsoid, [0.345, 0.05, 0.06], [0.07, 0.24, 0.16]), [0.0, -18.0, 6.0]), 0.03),
        k(turned(p(Shape::Ellipsoid, [-0.345, 0.05, 0.06], [0.07, 0.24, 0.16]), [0.0, 18.0, -6.0]), 0.03),
    ];
    // With separate face parts the lips are made by `facekit`, not melted in.
    parts.retain(|pt| sculpt_lids || pt.material != 1);
    // Carved: eye sockets, nostrils, the mouth line, the bowls of the ears.
    for x in [0.12f32, -0.12] {
        parts.push(sub_(p(Shape::Ellipsoid, [x, 0.0, -0.37], [0.15, 0.1, 0.12]), 0.035));
    }
    for x in [0.026f32, -0.026] {
        parts.push(sub_(p(Shape::Ellipsoid, [x, 0.212, -0.445], [0.03, 0.022, 0.035]), 0.01));
    }
    parts.push(sub_(Part { a: scale([-0.085, 0.285, -0.4], s), b: scale([0.085, 0.285, -0.4], s), ..Part::new(Shape::Capsule, [0.0; 3], [0.012 * s; 3]) }, 0.008));
    for x in [0.37f32, -0.37] {
        parts.push(sub_(turned(p(Shape::Ellipsoid, [x, 0.05, 0.05], [0.04, 0.15, 0.1]), [0.0, if x > 0.0 { -18.0 } else { 18.0 }, 0.0]), 0.015));
    }
    // The mouth's inside: a cavity behind the lips for teeth and tongue.
    if !sculpt_lids {
        // A real mouth: an opening through the skin into a cavity, a groove
        // under the nose; the lips are tubes round its edge (see facekit).
        parts.push(sub_(p(Shape::Ellipsoid, [0.0, 0.289, -0.272], [0.2, 0.09, 0.17]), 0.012));
        parts.push(sub_(p(Shape::Ellipsoid, [0.0, 0.2885, -0.4], [0.178, 0.034, 0.14]), 0.01));
        parts.push(sub_(Part { a: scale([0.0, 0.228, -0.45], s), b: scale([0.0, 0.252, -0.43], s), ..Part::new(Shape::Capsule, [0.0; 3], [0.014 * s; 3]) }, 0.006));
    }
    // The neck ends flat, as a bust does.
    parts.push(Part { join: Join::Subtract, blend: 0.01 * s, material: u32::MAX, ..p(Shape::Box, [0.0, 0.95, 0.0], [2.0, 0.5, 2.0]) });
    // Lids over the eyes (the eyeballs are added as clean spheres below).
    for x in [0.12f32, -0.12] {
        if !sculpt_lids {
            continue;
        }
        parts.push(k(turned(p(Shape::Ellipsoid, [x, -0.043, -0.335], [0.135, 0.05, 0.08]), [14.0, 0.0, 0.0]), 0.012));
        parts.push(k(turned(p(Shape::Ellipsoid, [x, 0.052, -0.333], [0.125, 0.036, 0.062]), [-12.0, 0.0, 0.0]), 0.012));
    }
    let lin = crate::srgb_to_linear;
    let colour = |r: u8, g: u8, b: u8| [lin(r), lin(g), lin(b), 1.0];
    let materials = vec![
        Material { name: "Skin".into(), base_colour: colour(214, 160, 136), metallic: 0.0, roughness: 0.52, subsurface: 0.6, ..Material::default() },
        Material { name: "Lips".into(), base_colour: colour(178, 112, 102), metallic: 0.0, roughness: 0.42, subsurface: 0.7, ..Material::default() },
        Material { name: "Eye white".into(), base_colour: colour(236, 232, 224), metallic: 0.0, roughness: 0.08, ..Material::default() },
        Material { name: "Iris".into(), base_colour: colour(88, 104, 72), metallic: 0.0, roughness: 0.12, ..Material::default() },
        Material { name: "Pupil".into(), base_colour: colour(12, 12, 14), metallic: 0.0, roughness: 0.05, ..Material::default() },
        Material { name: "Brow".into(), base_colour: colour(brow[0], brow[1], brow[2]), metallic: 0.0, roughness: 0.7, ..Material::default() },
        Material { name: "Mouth".into(), base_colour: colour(54, 16, 22), metallic: 0.0, roughness: 0.85, subsurface: 0.0, ..Material::default() },
    ];
    let cell = s / [60.0, 90.0, 130.0, 170.0, 220.0, 280.0][detail.clamp(1, 6) - 1];
    let mut m = build(&parts, materials, cell);
    crate::ops::smooth(&mut m, &crate::region::Region::Material { index: 0 }, 2, 0.5);
    // Eyeballs: white, an iris and a pupil looking straight ahead.
    let mut mats = m.tri_materials();
    // The cavity's wall, inside the mouth, is dark.
    for t in 0..m.triangles() {
        if mats[t] != 0 && mats[t] != 1 {
            continue;
        }
        let c = m.centroid(t);
        let (x, y, z) = (c[0] / s, c[1] / s, c[2] / s);
        let q = (x / 0.1).powi(2) + ((y - 0.289) / 0.045).powi(2) + ((z + 0.272) / 0.085).powi(2);
        if q < 1.35 && m.face_normal(t)[2] > -0.2 {
            mats[t] = 6;
        }
    }
    // Brows: an arch of hair painted over each eye, thick at the nose and
    // thinning out to the temple.
    for t in 0..m.triangles() {
        if mats[t] != 0 {
            continue;
        }
        let c = m.centroid(t);
        let (x, y, z) = (c[0] / s, c[1] / s, c[2] / s);
        let ax = x.abs();
        if z > -0.27 || !(0.03..0.225).contains(&ax) {
            continue;
        }
        let u = (ax - 0.03) / 0.195;
        let centre = -0.088 - 0.03 * (std::f32::consts::PI * u).sin() + 0.012 * u;
        let half = 0.024 * (1.0 - 0.6 * u * u) + 0.004;
        if (y - centre).abs() < half {
            mats[t] = 5;
        }
    }
    for x in [0.12f32, -0.12] {
        let centre = [x * s, 0.0, -0.305 * s];
        let mut eye = crate::prim::uv_sphere(0.0525 * s, 48, 24);
        // The sphere's poles look forward, so the iris is round.
        crate::ops::transform(&mut eye, &crate::region::Region::All, centre, [90.0, 0.0, 0.0], [1.0; 3], [0.0; 3]);
        let base = m.positions.len() as u32;
        for t in 0..eye.triangles() {
            let d = dot(normalise(sub(eye.centroid(t), centre)), [0.0, 0.0, -1.0]);
            mats.push(if d > 13f32.to_radians().cos() { 4 } else if d > 30f32.to_radians().cos() { 3 } else { 2 });
        }
        m.positions.extend_from_slice(&eye.positions);
        m.normals.extend_from_slice(&eye.normals);
        m.uvs.extend_from_slice(&eye.uvs);
        m.indices.extend(eye.indices.iter().map(|i| i + base));
    }
    m.regroup(&mats);
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_balls_melt_into_one_closed_surface() {
        let parts = vec![Part::new(Shape::Ellipsoid, [-40.0, 0.0, 0.0], [100.0; 3]), Part { blend: 30.0, ..Part::new(Shape::Ellipsoid, [40.0, 0.0, 0.0], [100.0; 3]) }];
        let m = build(&parts, vec![], 3.0);
        assert!(m.triangles() > 1000);
        let (lo, hi) = m.bounds();
        assert!((lo[0] + 90.0).abs() < 4.0 && (hi[0] - 90.0).abs() < 4.0, "{lo:?} {hi:?}");
        // Watertight: every edge has two triangles.
        let mut edges = std::collections::HashMap::new();
        for t in 0..m.triangles() {
            let c = m.tri(t);
            for (a, b) in [(c[0], c[1]), (c[1], c[2]), (c[2], c[0])] {
                *edges.entry((a.min(b), a.max(b))).or_insert(0) += 1;
            }
        }
        assert!(edges.values().all(|n| *n == 2), "open edges");
        // Faces point outwards.
        let out = (0..m.triangles()).filter(|&t| dot(m.face_normal(t), normalise(m.centroid(t))) > 0.0).count();
        assert!(out as f32 > m.triangles() as f32 * 0.95);
    }

    /// A fingerprint of a built mix of parts, to compare meshing changes
    /// that must not change the mesh (run with `--ignored --nocapture`).
    #[test]
    #[ignore]
    fn fingerprint_of_a_build() {
        let parts = vec![
            Part::new(Shape::Ellipsoid, [-40.0, 0.0, 0.0], [100.0, 80.0, 90.0]),
            Part { blend: 30.0, material: 1, ..Part::new(Shape::Box, [40.0, 10.0, 0.0], [90.0; 3]) },
            Part { join: Join::Subtract, blend: 5.0, ..Part::new(Shape::Cylinder, [0.0, 0.0, 0.0], [40.0, 300.0, 40.0]) },
        ];
        let m = build(&parts, vec![Material::default(), Material::default()], 1.5);
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for p in &m.positions {
            for c in p {
                h = (h ^ u64::from(c.to_bits())).wrapping_mul(0x100_0000_01b3);
            }
        }
        for i in &m.indices {
            h = (h ^ u64::from(*i)).wrapping_mul(0x100_0000_01b3);
        }
        println!("build: {} vertices, {} triangles, fingerprint {h:016x}", m.positions.len(), m.triangles());
    }

    #[test]
    fn a_head_has_a_face() {
        let m = head(300.0, 2);
        let (lo, hi) = m.bounds();
        // The nose is the front-most point, near the middle.
        let front = m.positions.iter().copied().min_by(|a, b| a[2].total_cmp(&b[2])).unwrap();
        assert!(front[0].abs() < 15.0 && front[1] > 0.0 && front[1] < 90.0, "{front:?}");
        assert!(hi[1] - lo[1] > 300.0, "with the neck: {lo:?} {hi:?}");
        assert!(m.materials.len() == 7 && m.groups.len() >= 4, "{:?}", m.groups);
        // The brows exist as their own surface.
        let mats = m.tri_materials();
        assert!(mats.iter().filter(|&&k| k == 5).count() > 20, "brows");
    }

    #[test]
    fn a_tapered_cone_runs_from_one_radius_to_the_other() {
        let mut p = Part::new(Shape::Cone, [0.0; 3], [60.0, 10.0, 0.0]);
        p.a = [0.0, 0.0, 0.0];
        p.b = [100.0, 0.0, 0.0];
        let m = build(&[p], vec![], 1.5);
        let (lo, hi) = m.bounds();
        assert!((lo[0] + 30.0).abs() < 3.0 && (hi[0] - 105.0).abs() < 3.0, "{lo:?} {hi:?}");
        // Thick at a, thin at b.
        let near = |x: f32| m.positions.iter().filter(|p| (p[0] - x).abs() < 3.0).map(|p| p[1].abs()).fold(0.0f32, f32::max);
        assert!(near(0.0) > 25.0 && near(95.0) < 9.0, "{} {}", near(0.0), near(95.0));
    }

    #[test]
    fn a_wedge_is_full_height_at_one_end_and_thin_at_the_other() {
        let m = build(&[Part::new(Shape::Wedge, [0.0; 3], [100.0, 50.0, 40.0])], vec![], 1.0);
        let at = |x: f32| m.positions.iter().filter(|p| (p[0] - x).abs() < 2.0).map(|p| p[1]).fold((f32::INFINITY, f32::NEG_INFINITY), |(a, b), y| (a.min(y), b.max(y)));
        let (l0, h0) = at(-45.0);
        let (l1, h1) = at(45.0);
        // Thin at −x, the full height at +x.
        assert!(h1 - l1 > h0 - l0 + 25.0, "{l0} {h0} vs {l1} {h1}");
    }
}
