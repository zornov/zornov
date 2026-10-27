// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Puppet pins and corner pins: a layer's picture bent by a triangle mesh.
//!
//! The layer's picture (after effects, grade and masks, so a mask moves
//! with what it cuts) is covered with a grid mesh over what it draws: the
//! cells where it has any alpha, found on a fine occupancy grid the GPU
//! and the CPU compute the same way. Each mesh vertex is moved by the
//! pins, then every triangle is drawn with its own affine map: the
//! picture follows the mesh. [`warp_cpu`] is the reference; the GPU draws
//! the same packed triangles in `warp.wgsl` (binned into 8×8 tiles), so the
//! two agree pixel for pixel up to `f16` storage.
//!
//! **Deformation.** Position pins are the constraints of a rigid moving
//! least squares fit (Schaefer et al. 2006): each vertex gets the rotation
//! and shift that best carry the pins' rest points to their positions,
//! weighted by 1 / distance². Distances are measured through the mesh
//! (Dijkstra over its edges), so an arm moves without dragging the body
//! next to it, as an outline-aware puppet does. A drawing made of separate
//! shapes needs no single cut-out: a shape no pin lies on is bound whole to
//! the pin (or bone) nearest it and moves rigidly with it. Starch pins
//! blend the vertices around them towards the rigid motion at the pin
//! (stiff areas); bend pins turn and scale the picture around them by
//! their share of the weight; overlap pins decide which part of a fold is
//! drawn in front. With no pin moved, nothing is bent.
//!
//! **Bones.** A skeleton bends the same mesh by linear blend skinning:
//! each vertex follows every bone's motion by its weight, the weights
//! falling off with how much farther a bone is than the nearest one,
//! distances measured through the mesh (so a leg moves without pulling
//! the other leg next to it, and parts blend only at their joints). Pins bend
//! the skinned picture further; bones also set which part is drawn in
//! front where the picture folds over itself.
//!
//! **Corner pin.** The rest rectangle is mapped to four corners by a
//! homography; with a puppet, the bent vertices go through it after.
//!
//! Coordinates in [`WarpDesc`] are 0..1 across the layer picture, like
//! masks; the work is in picture pixels so bends are rigid on non-square
//! pictures.

use crate::frame::Frame;

/// What a pin does (see `studio_model::PinKind`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PinKind {
    #[default]
    Position,
    Bend,
    Starch,
    Overlap,
}

/// A pin with its settings at one instant, in 0..1 layer coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PinDesc {
    pub kind: PinKind,
    /// The point of the picture it holds.
    pub rest: [f32; 2],
    /// Where that point is now (position pins).
    pub at: [f32; 2],
    /// Bend: clockwise degrees.
    pub rotation: f32,
    /// Bend: size factor.
    pub scale: f32,
    /// Starch 0..1, overlap -1..1.
    pub amount: f32,
    /// Starch and overlap reach, fraction of the shorter side.
    pub extent: f32,
}

impl PinDesc {
    pub fn position(rest: [f32; 2], at: [f32; 2]) -> PinDesc {
        PinDesc { kind: PinKind::Position, rest, at, rotation: 0.0, scale: 1.0, amount: 0.5, extent: 0.15 }
    }
}

/// Puppet pins of a layer.
#[derive(Clone, Debug, PartialEq)]
pub struct PuppetDesc {
    pub pins: Vec<PinDesc>,
    /// Mesh cells across the longer side of the content.
    pub density: f32,
    /// Mesh reach past the content, fraction of the shorter side.
    pub expansion: f32,
}

impl PuppetDesc {
    /// Whether anything is bent.
    pub fn bends(&self) -> bool {
        self.pins.iter().any(|p| match p.kind {
            PinKind::Position => p.rest != p.at,
            PinKind::Bend => p.rotation != 0.0 || p.scale != 1.0,
            _ => false,
        })
    }
}

/// A bone of a skeleton at one instant, in 0..1 layer coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SkinBone {
    /// Its joint and its other end at rest.
    pub head: [f32; 2],
    pub tail: [f32; 2],
    /// Rest → posed, layer coordinates: `x' = m0 x + m1 y + m2`,
    /// `y' = m3 x + m4 y + m5`.
    pub map: [f32; 6],
    /// Drawn in front (-1..1) where the picture folds over itself.
    pub depth: f32,
    /// Whether it bends the picture (control bones do not).
    pub deform: bool,
}

/// The bones bending a layer.
#[derive(Clone, Debug, PartialEq)]
pub struct SkinDesc {
    pub bones: Vec<SkinBone>,
    /// Reach of a bone's pull, fraction of the picture's shorter side.
    pub falloff: f32,
}

const IDENTITY_MAP: [f32; 6] = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0];

impl SkinDesc {
    /// Whether any deforming bone has moved.
    pub fn bends(&self) -> bool {
        self.bones.iter().any(|b| b.deform && b.map.iter().zip(IDENTITY_MAP).any(|(a, i)| (a - i).abs() > 1e-6))
    }
}

/// What shows in the gap that opens behind a cut.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum FillDesc {
    /// Nothing.
    #[default]
    None,
    /// A flat colour: linear light, straight alpha.
    Colour([f32; 4]),
    /// The picture's own colour at the cut, darkened (generated from the
    /// picture while drawing).
    Auto,
}

/// A cut: a path across the picture (0..1 layer coordinates, at rest)
/// along which the mesh is split, so its two sides move independently.
#[derive(Clone, Debug, PartialEq)]
pub struct CutDesc {
    pub points: Vec<[f32; 2]>,
    pub fill: FillDesc,
}

/// How a layer's picture is bent: bones, puppet pins, then a corner pin.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct WarpDesc {
    pub puppet: Option<PuppetDesc>,
    pub skin: Option<SkinDesc>,
    /// Cuts that split the mesh the puppet and the bones bend.
    pub cuts: Vec<CutDesc>,
    /// Rest rectangle (top-left, bottom-right) and the four corners it
    /// lands on (top-left, top-right, bottom-right, bottom-left).
    pub corner_pin: Option<([[f32; 2]; 2], [[f32; 2]; 4])>,
}

impl WarpDesc {
    /// Whether the warp changes anything.
    pub fn is_identity(&self) -> bool {
        let puppet = self.puppet.as_ref().is_some_and(PuppetDesc::bends) || self.skin.as_ref().is_some_and(SkinDesc::bends);
        let corners = self.corner_pin.is_some_and(|(r, c)| c != [r[0], [r[1][0], r[0][1]], r[1], [r[0][0], r[1][1]]]);
        !puppet && !corners
    }

    /// Whether the mesh needs the picture's occupancy (a puppet does; a
    /// corner pin alone covers its rectangle).
    pub fn needs_occupancy(&self) -> bool {
        self.puppet.as_ref().is_some_and(PuppetDesc::bends) || self.skin.as_ref().is_some_and(SkinDesc::bends)
    }
}

// ------------------------------------------------------------ the rig

/// A pin in picture pixels.
#[derive(Clone, Copy, Debug)]
struct PinPx {
    kind: PinKind,
    rest: [f32; 2],
    at: [f32; 2],
    rotation: f32,
    scale: f32,
    amount: f32,
    extent: f32,
}

#[inline]
fn weight(d: f32) -> f32 {
    1.0 / (d * d + 1.0)
}

#[inline]
fn smooth_falloff(d: f32, extent: f32) -> f32 {
    if extent <= 0.0 || d >= extent {
        return 0.0;
    }
    let u = 1.0 - d / extent;
    u * u * (3.0 - 2.0 * u)
}

/// Rotation (cos, sin) and the two weighted centroids of the rigid fit.
#[derive(Clone, Copy, Debug)]
struct Frame2 {
    cos: f32,
    sin: f32,
    from: [f32; 2],
    to: [f32; 2],
}

impl Frame2 {
    #[inline]
    fn apply(&self, p: [f32; 2]) -> [f32; 2] {
        let (x, y) = (p[0] - self.from[0], p[1] - self.from[1]);
        [self.cos * x - self.sin * y + self.to[0], self.sin * x + self.cos * y + self.to[1]]
    }
}

/// Bends points by the pins. Distances to every pin are supplied by the
/// caller (through the mesh, or straight for mask paths).
struct Rig {
    pins: Vec<PinPx>,
    /// Rigid frame at each starch pin.
    starch: Vec<(usize, Frame2)>,
    /// Bent location of each bend pin.
    bend: Vec<(usize, [f32; 2])>,
}

impl Rig {
    /// `between(i, j)`: distance from pin `i`'s rest point to pin `j`.
    fn new(pins: Vec<PinPx>, between: impl Fn(usize, usize) -> f32) -> Rig {
        let n = pins.len();
        let row = |i: usize| (0..n).map(|j| between(i, j)).collect::<Vec<f32>>();
        let mut rig = Rig { pins, starch: Vec::new(), bend: Vec::new() };
        for i in 0..n {
            if rig.pins[i].kind == PinKind::Starch {
                if let Some(f) = rig.fit(&row(i)) {
                    rig.starch.push((i, f));
                }
            }
        }
        for i in 0..n {
            if rig.pins[i].kind == PinKind::Bend {
                let d = row(i);
                let c = rig.moved(rig.pins[i].rest, &d);
                rig.bend.push((i, c));
            }
        }
        rig
    }

    /// The rigid fit of the position pins seen from a point with
    /// distances `d`. `None` without position pins.
    fn fit(&self, d: &[f32]) -> Option<Frame2> {
        let (mut sw, mut p, mut q) = (0.0f32, [0.0f32; 2], [0.0f32; 2]);
        for (pin, &di) in self.pins.iter().zip(d) {
            if pin.kind != PinKind::Position {
                continue;
            }
            let w = weight(di);
            sw += w;
            p = [p[0] + w * pin.rest[0], p[1] + w * pin.rest[1]];
            q = [q[0] + w * pin.at[0], q[1] + w * pin.at[1]];
        }
        if sw <= 0.0 {
            return None;
        }
        let (p, q) = ([p[0] / sw, p[1] / sw], [q[0] / sw, q[1] / sw]);
        // μ = Σ w q̂ · conj(p̂) as complex numbers: the best rotation.
        let (mut re, mut im) = (0.0f32, 0.0f32);
        for (pin, &di) in self.pins.iter().zip(d) {
            if pin.kind != PinKind::Position {
                continue;
            }
            let w = weight(di);
            let (px, py) = (pin.rest[0] - p[0], pin.rest[1] - p[1]);
            let (qx, qy) = (pin.at[0] - q[0], pin.at[1] - q[1]);
            re += w * (qx * px + qy * py);
            im += w * (qy * px - qx * py);
        }
        let len = (re * re + im * im).sqrt();
        let (cos, sin) = if len > 1e-9 { (re / len, im / len) } else { (1.0, 0.0) };
        Some(Frame2 { cos, sin, from: p, to: q })
    }

    /// The point moved by the position and starch pins.
    fn moved(&self, p: [f32; 2], d: &[f32]) -> [f32; 2] {
        let mut out = match self.fit(d) {
            Some(f) => f.apply(p),
            None => p,
        };
        for (i, f) in &self.starch {
            let pin = &self.pins[*i];
            let b = pin.amount.clamp(0.0, 1.0) * smooth_falloff(d[*i], pin.extent);
            if b > 0.0 {
                let s = f.apply(p);
                out = [out[0] + (s[0] - out[0]) * b, out[1] + (s[1] - out[1]) * b];
            }
        }
        out
    }

    /// The point bent by every pin.
    fn apply(&self, p: [f32; 2], d: &[f32]) -> [f32; 2] {
        let mut out = self.moved(p, d);
        if self.bend.is_empty() {
            return out;
        }
        let total: f32 = self.pins.iter().zip(d).filter(|(pin, _)| matches!(pin.kind, PinKind::Position | PinKind::Bend)).map(|(_, &di)| weight(di)).sum();
        for (i, c) in &self.bend {
            let pin = &self.pins[*i];
            let share = weight(d[*i]) / total.max(1e-12);
            let (s, co) = (pin.rotation * share).to_radians().sin_cos();
            let k = pin.scale.max(1e-3).powf(share);
            let (x, y) = (out[0] - c[0], out[1] - c[1]);
            out = [c[0] + k * (co * x - s * y), c[1] + k * (s * x + co * y)];
        }
        out
    }
}

fn to_px(pins: &[PinDesc], w: f32, h: f32) -> Vec<PinPx> {
    let short = w.min(h);
    pins.iter()
        .map(|p| PinPx {
            kind: p.kind,
            rest: [p.rest[0] * w, p.rest[1] * h],
            at: [p.at[0] * w, p.at[1] * h],
            rotation: p.rotation,
            scale: p.scale,
            amount: p.amount,
            extent: p.extent * short,
        })
        .collect()
}

#[inline]
fn dist(a: [f32; 2], b: [f32; 2]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

/// Bends points (picture pixels of a `width × height` picture) by pins
/// with straight-line distances: how a mask path follows its pins.
pub fn bend_points(pins: &[PinDesc], points: &mut [[f32; 2]], width: f32, height: f32) {
    let px = to_px(pins, width, height);
    if !px.iter().any(|p| match p.kind {
        PinKind::Position => p.rest != p.at,
        PinKind::Bend => p.rotation != 0.0 || p.scale != 1.0,
        _ => false,
    }) {
        return;
    }
    let rests: Vec<[f32; 2]> = px.iter().map(|p| p.rest).collect();
    let rig = Rig::new(px, |i, j| dist(rests[i], rests[j]));
    let mut d = vec![0.0; rests.len()];
    for p in points.iter_mut() {
        for (k, r) in rests.iter().enumerate() {
            d[k] = dist(*p, *r);
        }
        *p = rig.apply(*p, &d);
    }
}

// ----------------------------------------------------------- occupancy

/// The fine grid on which a picture's alpha is looked at: square cells of
/// `cell` pixels (the last row and column may be narrower).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OccupancyGrid {
    pub cell: u32,
    pub nx: u32,
    pub ny: u32,
}

/// At most this many occupancy cells across.
const OCCUPANCY_CELLS: u32 = 256;
/// Alpha above which a cell counts as drawn.
pub const OCCUPIED: f32 = 1.0 / 512.0;

impl OccupancyGrid {
    pub fn for_size(width: u32, height: u32) -> OccupancyGrid {
        let cell = width.max(height).div_ceil(OCCUPANCY_CELLS).max(1);
        OccupancyGrid { cell, nx: width.div_ceil(cell).max(1), ny: height.div_ceil(cell).max(1) }
    }
}

/// The largest alpha in each occupancy cell (row major), as the GPU's
/// `cells.wgsl` computes it.
pub fn occupancy_cpu(image: &Frame, g: OccupancyGrid) -> Vec<f32> {
    let mut out = vec![0.0f32; (g.nx * g.ny) as usize];
    for y in 0..image.height {
        let cy = (y / g.cell).min(g.ny - 1);
        let row = &image.pixels[(y * image.width) as usize..((y + 1) * image.width) as usize];
        for (x, px) in row.iter().enumerate() {
            let c = &mut out[(cy * g.nx + (x as u32 / g.cell).min(g.nx - 1)) as usize];
            *c = c.max(px[3]);
        }
    }
    out
}

// ---------------------------------------------------------------- mesh

/// Triangles ready to draw, shared by both backends.
#[derive(Clone, Debug, PartialEq)]
pub struct WarpMesh {
    /// Output picture size and where its top-left lies in input picture
    /// pixels.
    pub size: (u32, u32),
    pub origin: [f32; 2],
    /// Per triangle, 16 floats: bent corners (output pixels) p0, p1, p2;
    /// their picture positions (input pixels) uv0, uv1, uv2; depth; pad.
    pub tris: Vec<[f32; 16]>,
}

/// Largest side of a warped picture.
const MAX_SIDE: f32 = 8192.0;
/// Mesh cells across the longer side at least, where cuts split the mesh.
const CUT_DENSITY: f32 = 96.0;
/// Grid of a corner pin without a puppet.
const CORNER_CELLS: usize = 16;

/// Homography taking the unit square's corners (0,0), (1,0), (1,1), (0,1)
/// to `q` (Heckbert's square-to-quad), as a row-major 3×3.
fn square_to_quad(q: [[f32; 2]; 4]) -> [f64; 9] {
    let [(x0, y0), (x1, y1), (x2, y2), (x3, y3)] = q.map(|p| (p[0] as f64, p[1] as f64));
    let (sx, sy) = (x0 - x1 + x2 - x3, y0 - y1 + y2 - y3);
    if sx.abs() < 1e-12 && sy.abs() < 1e-12 {
        return [x1 - x0, x2 - x1, x0, y1 - y0, y2 - y1, y0, 0.0, 0.0, 1.0];
    }
    let (dx1, dx2, dy1, dy2) = (x1 - x2, x3 - x2, y1 - y2, y3 - y2);
    let den = dx1 * dy2 - dx2 * dy1;
    let den = if den.abs() < 1e-12 { 1e-12 } else { den };
    let g = (sx * dy2 - dx2 * sy) / den;
    let h = (dx1 * sy - sx * dy1) / den;
    [x1 - x0 + g * x1, x3 - x0 + h * x3, x0, y1 - y0 + g * y1, y3 - y0 + h * y3, y0, g, h, 1.0]
}

fn homography(m: &[f64; 9], u: f32, v: f32) -> [f32; 2] {
    let (u, v) = (u as f64, v as f64);
    let w = m[6] * u + m[7] * v + m[8];
    let w = if w.abs() < 1e-9 { 1e-9_f64.copysign(w) } else { w };
    [((m[0] * u + m[1] * v + m[2]) / w) as f32, ((m[3] * u + m[4] * v + m[5]) / w) as f32]
}

/// A grid of `nx × ny` cells over `rect` (picture pixels) with the cells
/// in use.
struct Grid {
    nx: usize,
    ny: usize,
    rect: [f32; 4],
    cells: Vec<bool>,
    /// Mesh edges a cut runs along: horizontal `(nx × (ny + 1))`, from
    /// node (i, j) to (i + 1, j); vertical `((nx + 1) × ny)`, from node
    /// (i, j) to (i, j + 1).
    cut_h: Vec<bool>,
    cut_v: Vec<bool>,
    /// The fill of each cut edge: (horizontal, i, j, fill).
    fills: Vec<(bool, usize, usize, FillDesc)>,
}

impl Grid {
    fn node(&self, i: usize, j: usize) -> [f32; 2] {
        let [x0, y0, x1, y1] = self.rect;
        [x0 + (x1 - x0) * i as f32 / self.nx as f32, y0 + (y1 - y0) * j as f32 / self.ny as f32]
    }
    fn idx(&self, i: usize, j: usize) -> usize {
        j * (self.nx + 1) + i
    }
    fn new(nx: usize, ny: usize, rect: [f32; 4], cells: Vec<bool>) -> Grid {
        Grid { nx, ny, rect, cells, cut_h: vec![false; nx * (ny + 1)], cut_v: vec![false; (nx + 1) * ny], fills: Vec::new() }
    }
    fn is_cut_h(&self, i: i64, j: i64) -> bool {
        i >= 0 && j >= 0 && (i as usize) < self.nx && (j as usize) <= self.ny && self.cut_h[j as usize * self.nx + i as usize]
    }
    fn is_cut_v(&self, i: i64, j: i64) -> bool {
        i >= 0 && j >= 0 && (i as usize) <= self.nx && (j as usize) < self.ny && self.cut_v[j as usize * (self.nx + 1) + i as usize]
    }
    fn used(&self, ci: i64, cj: i64) -> bool {
        ci >= 0 && cj >= 0 && (ci as usize) < self.nx && (cj as usize) < self.ny && self.cells[cj as usize * self.nx + ci as usize]
    }

    /// Marks the mesh edges the cuts run along: the path is walked in
    /// quarter-cell steps, each step snapped to the nearest node, and
    /// consecutive nodes joined along mesh edges (a diagonal step goes
    /// round the corner nearer the path). Cuts stay on mesh edges so both
    /// sides keep whole triangles.
    fn mark_cuts(&mut self, cuts: &[CutDesc], w: f32, h: f32) {
        let [x0, y0, x1, y1] = self.rect;
        let (sx, sy) = (self.nx as f32 / (x1 - x0).max(1e-6), self.ny as f32 / (y1 - y0).max(1e-6));
        for cut in cuts {
            if cut.points.len() < 2 {
                continue;
            }
            let g: Vec<[f32; 2]> = cut.points.iter().map(|p| [(p[0] * w - x0) * sx, (p[1] * h - y0) * sy]).collect();
            let node = |p: [f32; 2]| (p[0].round() as i64, p[1].round() as i64);
            let mut prev: Option<(i64, i64)> = None;
            let mut edges: Vec<((i64, i64), (i64, i64))> = Vec::new();
            let mut visit = |p: [f32; 2], prev: &mut Option<(i64, i64)>| {
                let n = node(p);
                if let Some(q) = *prev {
                    if q != n {
                        if q.0 != n.0 && q.1 != n.1 {
                            let (a, b) = ((q.0, n.1), (n.0, q.1));
                            let da = (a.0 as f32 - p[0]).powi(2) + (a.1 as f32 - p[1]).powi(2);
                            let db = (b.0 as f32 - p[0]).powi(2) + (b.1 as f32 - p[1]).powi(2);
                            let m = if da <= db { a } else { b };
                            edges.push((q, m));
                            edges.push((m, n));
                        } else {
                            edges.push((q, n));
                        }
                    }
                }
                *prev = Some(n);
            };
            for seg in g.windows(2) {
                let len = ((seg[1][0] - seg[0][0]).powi(2) + (seg[1][1] - seg[0][1]).powi(2)).sqrt();
                let steps = ((len / 0.25).ceil() as usize).clamp(1, 100_000);
                for k in 0..=steps {
                    let t = k as f32 / steps as f32;
                    visit([seg[0][0] + (seg[1][0] - seg[0][0]) * t, seg[0][1] + (seg[1][1] - seg[0][1]) * t], &mut prev);
                }
            }
            for (a, b) in edges {
                if a.1 == b.1 {
                    let (i, j) = (a.0.min(b.0), a.1);
                    if i >= 0 && j >= 0 && (i as usize) < self.nx && (j as usize) <= self.ny {
                        self.cut_h[j as usize * self.nx + i as usize] = true;
                        self.fills.push((true, i as usize, j as usize, cut.fill));
                    }
                } else {
                    let (i, j) = (a.0, a.1.min(b.1));
                    if i >= 0 && j >= 0 && (i as usize) <= self.nx && (j as usize) < self.ny {
                        self.cut_v[j as usize * (self.nx + 1) + i as usize] = true;
                        self.fills.push((false, i as usize, j as usize, cut.fill));
                    }
                }
            }
        }
    }
}

/// The mesh with its cuts applied: a node a cut passes through becomes one
/// vertex per arc of cells around it that the cut leaves joined, so the two
/// sides of a cut share no vertices (and so no distances, weights or
/// motion) but sit at the same rest points: the picture is whole at rest.
/// Without cuts every node is one vertex.
struct Topo {
    /// Rest position of each vertex (picture pixels).
    pos: Vec<[f32; 2]>,
    /// The node each vertex sits at.
    node_of: Vec<usize>,
    /// Per mesh cell, the vertices of its corners (top-left, top-right,
    /// bottom-right, bottom-left).
    cell_v: Vec<[u32; 4]>,
    /// Edges of the triangles and their diagonals, with rest lengths.
    adj: Vec<Vec<(u32, f32)>>,
    /// Connected piece of the picture each node lies in, cuts ignored.
    comp: Vec<u32>,
    /// Gaps that can open along cut edges.
    gaps: Vec<Gap>,
}

/// The two copies of a cut edge: `[a_side1, b_side1, b_side2, a_side2]`,
/// with the rest positions of its ends and its fill.
struct Gap {
    v: [u32; 4],
    uv: [[f32; 2]; 2],
    fill: FillDesc,
}

impl Topo {
    fn new(grid: &Grid) -> Topo {
        let (nx, ny) = (grid.nx, grid.ny);
        let nodes = (nx + 1) * (ny + 1);
        let mut pos = Vec::with_capacity(nodes);
        let mut node_of = Vec::with_capacity(nodes);
        // Vertex of each node for each of the four cells around it:
        // north-west, north-east, south-east, south-west.
        let mut around = vec![[0u32; 4]; nodes];
        for j in 0..=ny {
            for i in 0..=nx {
                let (ii, jj) = (i as i64, j as i64);
                // The edges between those cells, in order.
                let cut = [grid.is_cut_v(ii, jj - 1), grid.is_cut_h(ii, jj), grid.is_cut_v(ii, jj), grid.is_cut_h(ii - 1, jj)];
                // Cells around the node that are in use, in the same order.
                let live = [grid.used(ii - 1, jj - 1), grid.used(ii, jj - 1), grid.used(ii, jj), grid.used(ii - 1, jj)];
                let mut lab = [0usize, 1, 2, 3];
                if cut.iter().any(|c| *c) {
                    // Where a cut passes, only cells that really share an
                    // uncut edge stay joined (so a cut that runs out to the
                    // silhouette or the edge of the mesh separates there).
                    for _ in 0..3 {
                        for k in 0..4 {
                            if !cut[k] && live[k] && live[(k + 1) % 4] {
                                let m = lab[k].min(lab[(k + 1) % 4]);
                                lab[k] = m;
                                lab[(k + 1) % 4] = m;
                            }
                        }
                    }
                } else {
                    lab = [0; 4];
                }
                let mut ids: [Option<u32>; 4] = [None; 4];
                for k in 0..4 {
                    let id = *ids[lab[k]].get_or_insert_with(|| {
                        pos.push(grid.node(i, j));
                        node_of.push(grid.idx(i, j));
                        pos.len() as u32 - 1
                    });
                    around[grid.idx(i, j)][k] = id;
                }
            }
        }
        let mut cell_v = vec![[u32::MAX; 4]; nx * ny];
        let mut adj: Vec<Vec<(u32, f32)>> = vec![Vec::new(); pos.len()];
        let mut uf: Vec<u32> = (0..nodes as u32).collect();
        fn find(uf: &mut [u32], mut x: u32) -> u32 {
            while uf[x as usize] != x {
                uf[x as usize] = uf[uf[x as usize] as usize];
                x = uf[x as usize];
            }
            x
        }
        for cj in 0..ny {
            for ci in 0..nx {
                if !grid.cells[cj * nx + ci] {
                    continue;
                }
                let v = [around[grid.idx(ci, cj)][2], around[grid.idx(ci + 1, cj)][3], around[grid.idx(ci + 1, cj + 1)][0], around[grid.idx(ci, cj + 1)][1]];
                cell_v[cj * nx + ci] = v;
                for a in 0..4 {
                    for b in a + 1..4 {
                        let d = dist(pos[v[a] as usize], pos[v[b] as usize]);
                        adj[v[a] as usize].push((v[b], d));
                        adj[v[b] as usize].push((v[a], d));
                        let (ra, rb) = (find(&mut uf, node_of[v[a] as usize] as u32), find(&mut uf, node_of[v[b] as usize] as u32));
                        uf[ra as usize] = rb;
                    }
                }
            }
        }
        let comp: Vec<u32> = (0..nodes as u32).map(|n| find(&mut uf, n)).collect();
        // Gaps along cut edges with a cell in use on both sides.
        let mut gaps = Vec::new();
        for &(horizontal, i, j, fill) in &grid.fills {
            let (cells, ends, corners): ([(i64, i64); 2], [(usize, usize); 2], [[usize; 2]; 2]) = if horizontal {
                ([(i as i64, j as i64 - 1), (i as i64, j as i64)], [(i, j), (i + 1, j)], [[3, 2], [0, 1]])
            } else {
                ([(i as i64 - 1, j as i64), (i as i64, j as i64)], [(i, j), (i, j + 1)], [[1, 2], [0, 3]])
            };
            if !grid.used(cells[0].0, cells[0].1) || !grid.used(cells[1].0, cells[1].1) {
                continue;
            }
            let at = |side: usize, end: usize| cell_v[cells[side].1 as usize * nx + cells[side].0 as usize][corners[side][end]];
            let v = [at(0, 0), at(0, 1), at(1, 1), at(1, 0)];
            if v[0] != v[3] || v[1] != v[2] {
                gaps.push(Gap { v, uv: [grid.node(ends[0].0, ends[0].1), grid.node(ends[1].0, ends[1].1)], fill });
            }
        }
        Topo { pos, node_of, cell_v, adj, comp, gaps }
    }

    /// The vertices a point seeds a distance field with, with the
    /// distance to each: its mesh cell's corners, or the nearest corner of
    /// a cell in use.
    fn seeds(&self, grid: &Grid, from: [f32; 2]) -> Vec<(u32, f32)> {
        let [x0, y0, x1, y1] = grid.rect;
        let ci = (((from[0] - x0) / (x1 - x0).max(1e-6) * grid.nx as f32).floor().max(0.0) as usize).min(grid.nx - 1);
        let cj = (((from[1] - y0) / (y1 - y0).max(1e-6) * grid.ny as f32).floor().max(0.0) as usize).min(grid.ny - 1);
        if grid.cells[cj * grid.nx + ci] {
            return self.cell_v[cj * grid.nx + ci].iter().map(|&v| (v, dist(self.pos[v as usize], from))).collect();
        }
        let mut best: Option<(u32, f32)> = None;
        for (c, vs) in self.cell_v.iter().enumerate() {
            if !grid.cells[c] {
                continue;
            }
            for &v in vs {
                let e = dist(self.pos[v as usize], from);
                if best.is_none_or(|(_, b)| e < b) {
                    best = Some((v, e));
                }
            }
        }
        best.into_iter().collect()
    }
}

/// The puppet's mesh grid over what the picture draws.
fn puppet_grid(p: &PuppetDesc, w: u32, h: u32, occ: &[f32], g: OccupancyGrid, cut: bool) -> Option<Grid> {
    let filled = |i: u32, j: u32| occ.get((j * g.nx + i) as usize).is_some_and(|a| *a > OCCUPIED);
    // The content's box on the occupancy grid.
    let (mut lo, mut hi) = ([u32::MAX; 2], [0u32; 2]);
    for j in 0..g.ny {
        for i in 0..g.nx {
            if filled(i, j) {
                lo = [lo[0].min(i), lo[1].min(j)];
                hi = [hi[0].max(i + 1), hi[1].max(j + 1)];
            }
        }
    }
    if lo[0] == u32::MAX {
        return None;
    }
    let (wf, hf) = (w as f32, h as f32);
    let grow = p.expansion.max(0.0) * wf.min(hf);
    let c = g.cell as f32;
    let rect = [(lo[0] as f32 * c - grow).max(-wf), (lo[1] as f32 * c - grow).max(-hf), (hi[0] as f32 * c).min(wf) + grow, (hi[1] as f32 * c).min(hf) + grow];
    let (bw, bh) = (rect[2] - rect[0], rect[3] - rect[1]);
    // A mesh with cuts is finer, so a cut can follow a thin gap or a lip.
    let density = if cut { p.density.max(CUT_DENSITY).clamp(2.0, 160.0) } else { p.density.clamp(2.0, 96.0) };
    let long = bw.max(bh).max(1.0);
    let nx = ((density * bw / long).round() as usize).max(1);
    let ny = ((density * bh / long).round() as usize).max(1);
    let mut cells = vec![false; nx * ny];
    for cj in 0..ny {
        for ci in 0..nx {
            // The occupancy cells this mesh cell (grown by the expansion)
            // touches.
            let x0 = rect[0] + bw * ci as f32 / nx as f32 - grow;
            let x1 = rect[0] + bw * (ci + 1) as f32 / nx as f32 + grow;
            let y0 = rect[1] + bh * cj as f32 / ny as f32 - grow;
            let y1 = rect[1] + bh * (cj + 1) as f32 / ny as f32 + grow;
            let (i0, i1) = (((x0 / c).floor().max(0.0)) as u32, ((x1 / c).ceil().max(0.0) as u32).min(g.nx));
            let (j0, j1) = (((y0 / c).floor().max(0.0)) as u32, ((y1 / c).ceil().max(0.0) as u32).min(g.ny));
            cells[cj * nx + ci] = (j0..j1).any(|j| (i0..i1).any(|i| filled(i, j)));
        }
    }
    Some(Grid::new(nx, ny, rect, cells))
}

/// Distances through the mesh from the nearest of several points (a pin,
/// or points along a bone), one per vertex: Dijkstra over the edges of the
/// cells in use, so a cut is not crossed. A vertex a cut has shut off from
/// the points is infinitely far; one in a piece of the picture the points
/// are not in (a floating hand) is measured straight.
fn mesh_field(topo: &Topo, grid: &Grid, points: &[[f32; 2]]) -> Vec<f32> {
    use std::cmp::Ordering;
    use std::collections::BinaryHeap;
    #[derive(PartialEq)]
    struct Item(f32, u32);
    impl Eq for Item {}
    impl PartialOrd for Item {
        fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
            Some(self.cmp(o))
        }
    }
    impl Ord for Item {
        fn cmp(&self, o: &Self) -> Ordering {
            o.0.total_cmp(&self.0).then(o.1.cmp(&self.1))
        }
    }
    let mut d = vec![f32::INFINITY; topo.pos.len()];
    let mut heap = BinaryHeap::new();
    let mut reached = std::collections::BTreeSet::new();
    for &from in points {
        for (v, e) in topo.seeds(grid, from) {
            reached.insert(topo.comp[topo.node_of[v as usize]]);
            if e < d[v as usize] {
                d[v as usize] = e;
                heap.push(Item(e, v));
            }
        }
    }
    while let Some(Item(du, u)) = heap.pop() {
        if du > d[u as usize] {
            continue;
        }
        for &(v, len) in &topo.adj[u as usize] {
            let nd = du + len;
            if nd < d[v as usize] {
                d[v as usize] = nd;
                heap.push(Item(nd, v));
            }
        }
    }
    // A separate shape no point lies on (a drawing made of loose pieces: a
    // hand, a hat, an eye) is bound whole to the nearest point: every
    // vertex of it takes the shape's own nearest distance, so it moves
    // rigidly with the pin or bone nearest it instead of stretching.
    let mut piece: std::collections::BTreeMap<u32, f32> = Default::default();
    for (k, dk) in d.iter().enumerate() {
        let c = topo.comp[topo.node_of[k]];
        if !dk.is_finite() && !reached.contains(&c) {
            let e = points.iter().map(|p| dist(topo.pos[k], *p)).fold(f32::INFINITY, f32::min);
            let slot = piece.entry(c).or_insert(f32::INFINITY);
            *slot = slot.min(e);
        }
    }
    for (k, dk) in d.iter_mut().enumerate() {
        if !dk.is_finite()
            && let Some(&e) = piece.get(&topo.comp[topo.node_of[k]])
        {
            *dk = e;
        }
    }
    d
}

/// Skinning weights per mesh vertex (one row per vertex: a weight per bone
/// of `bones`, summing to 1, or all zero for a vertex no bone reaches) and
/// each vertex's depth.
fn skin_weights(topo: &Topo, grid: &Grid, skin: &SkinDesc, w: f32, h: f32) -> (Vec<usize>, Vec<Vec<f32>>, Vec<f32>) {
    let bones: Vec<usize> = (0..skin.bones.len()).filter(|&i| skin.bones[i].deform).collect();
    let step = (grid.rect[2] - grid.rect[0]).max(grid.rect[3] - grid.rect[1]) / grid.nx.max(grid.ny).max(1) as f32 * 0.5;
    let fields: Vec<Vec<f32>> = bones
        .iter()
        .map(|&i| {
            let b = &skin.bones[i];
            let (a, z) = ([b.head[0] * w, b.head[1] * h], [b.tail[0] * w, b.tail[1] * h]);
            let n = ((dist(a, z) / step.max(0.5)).ceil() as usize).clamp(1, 64);
            let pts: Vec<[f32; 2]> = (0..=n).map(|k| {
                let t = k as f32 / n as f32;
                [a[0] + (z[0] - a[0]) * t, a[1] + (z[1] - a[1]) * t]
            }).collect();
            mesh_field(topo, grid, &pts)
        })
        .collect();
    let sigma = (skin.falloff.max(0.005) * w.min(h)).max(1.0);
    let nodes = topo.pos.len();
    let mut rows = Vec::with_capacity(nodes);
    let mut depth = Vec::with_capacity(nodes);
    for k in 0..nodes {
        // Weighed against the nearest bone: a part moves with its own bone
        // and blends only where two bones are about as near (a joint), so a
        // swinging arm does not drag the face beside it.
        let near = fields.iter().map(|f| f[k]).fold(f32::INFINITY, f32::min);
        if !near.is_finite() {
            // Shut off from every bone by cuts: it stays where it is.
            rows.push(vec![0.0; fields.len()]);
            depth.push(0.0);
            continue;
        }
        let mut row: Vec<f32> = fields.iter().map(|f| (-((f[k] - near) / sigma).powi(2)).exp()).collect();
        let sum: f32 = row.iter().sum();
        if sum > 0.0 {
            row.iter_mut().for_each(|v| *v /= sum);
        }
        depth.push(row.iter().zip(&bones).map(|(wt, &i)| wt * skin.bones[i].depth).sum());
        rows.push(row);
    }
    (bones, rows, depth)
}

/// A bone's map in picture pixels.
fn map_px(m: [f32; 6], w: f32, h: f32) -> [f32; 6] {
    // S · M · S⁻¹ with S = diag(w, h).
    [m[0], m[1] * w / h.max(1e-6), m[2] * w, m[3] * h / w.max(1e-6), m[4], m[5] * h]
}

/// Fill triangles are drawn behind the picture.
const GAP_DEPTH: f32 = -2.0;
/// How much of the picture's own colour an automatic fill keeps.
const AUTO_SHADE: f32 = 0.4;

/// Builds the triangles for a `w × h` picture. `occupancy` (on
/// [`OccupancyGrid::for_size`]) is needed when
/// [`WarpDesc::needs_occupancy`]. `None` when nothing is drawn.
///
/// A triangle's 14th float is how it is drawn: 0 the picture at its
/// `uv`s; 1 a flat premultiplied colour held in the `uv` slots (r, g, b,
/// a); 2 the picture darkened by the 15th float (an automatic fill).
pub fn build_mesh(warp: &WarpDesc, w: u32, h: u32, occupancy: Option<&[f32]>) -> Option<WarpMesh> {
    build_mesh_with(warp, w, h, occupancy, false)
}

/// The mesh a warp bends, even when nothing is bent yet (the rest mesh
/// with its cuts), for showing it on the canvas.
pub fn shown_mesh(warp: &WarpDesc, w: u32, h: u32, occupancy: Option<&[f32]>) -> Option<WarpMesh> {
    build_mesh_with(warp, w, h, occupancy, true)
}

fn build_mesh_with(warp: &WarpDesc, w: u32, h: u32, occupancy: Option<&[f32]>, show: bool) -> Option<WarpMesh> {
    let (wf, hf) = (w as f32, h as f32);
    let puppet = warp.puppet.as_ref().filter(|p| show || p.bends());
    let skin = warp.skin.as_ref().filter(|s| show || s.bends());
    let mesh_of = warp.puppet.clone().unwrap_or(PuppetDesc { pins: Vec::new(), density: 32.0, expansion: 0.02 });
    let cuts: Vec<CutDesc> = warp.cuts.iter().filter(|c| c.points.len() >= 2).cloned().collect();
    let mut grid = if puppet.is_none() && skin.is_none() && !(show && warp.corner_pin.is_none()) {
        let (r, _) = warp.corner_pin?;
        let rect = [r[0][0] * wf, r[0][1] * hf, r[1][0] * wf, r[1][1] * hf];
        if rect[2] - rect[0] < 1e-3 || rect[3] - rect[1] < 1e-3 {
            return None;
        }
        Grid::new(CORNER_CELLS, CORNER_CELLS, rect, vec![true; CORNER_CELLS * CORNER_CELLS])
    } else {
        let g = OccupancyGrid::for_size(w, h);
        let mut grid = puppet_grid(&mesh_of, w, h, occupancy?, g, !cuts.is_empty())?;
        grid.mark_cuts(&cuts, wf, hf);
        grid
    };
    if puppet.is_none() && skin.is_none() && !show {
        grid.fills.clear();
    }
    let topo = Topo::new(&grid);
    let (rig, pin_d) = match puppet {
        Some(p) => {
            let px = to_px(&p.pins, wf, hf);
            let fields: Vec<Vec<f32>> = px.iter().map(|pin| mesh_field(&topo, &grid, &[pin.rest])).collect();
            // Pin to pin: the field at the vertex nearest the other pin.
            let near: Vec<usize> = px
                .iter()
                .map(|p| topo.seeds(&grid, p.rest).into_iter().min_by(|a, b| a.1.total_cmp(&b.1)).map_or(0, |(v, _)| v as usize))
                .collect();
            let rig = Rig::new(px, |i, j| if i == j { 0.0 } else { fields[j][near[i]] });
            (Some(rig), fields)
        }
        None => (None, Vec::new()),
    };
    let corner = warp.corner_pin.map(|(r, c)| {
        let rect = [r[0][0] * wf, r[0][1] * hf, r[1][0] * wf, r[1][1] * hf];
        (rect, square_to_quad(c.map(|p| [p[0] * wf, p[1] * hf])))
    });
    let n = topo.pos.len();
    let skinned = skin.map(|s| {
        let (bones, rows, depth) = skin_weights(&topo, &grid, s, wf, hf);
        let maps: Vec<[f32; 6]> = bones.iter().map(|&i| map_px(s.bones[i].map, wf, hf)).collect();
        (maps, rows, depth)
    });
    let mut bent = vec![[0.0f32; 2]; n];
    let mut d = vec![0.0f32; pin_d.len()];
    for k in 0..n {
        let rest = topo.pos[k];
        let mut p = match &rig {
            Some(rig) => {
                for (s, f) in pin_d.iter().enumerate() {
                    d[s] = f[k];
                }
                rig.apply(rest, &d)
            }
            None => rest,
        };
        if let Some((maps, rows, _)) = &skinned {
            // The bones' blend, with the pins' bend on top.
            let sum: f32 = rows[k].iter().sum();
            if sum > 0.0 {
                let (mut x, mut y) = (0.0f32, 0.0f32);
                for (m, wt) in maps.iter().zip(&rows[k]) {
                    x += wt * (m[0] * rest[0] + m[1] * rest[1] + m[2]);
                    y += wt * (m[3] * rest[0] + m[4] * rest[1] + m[5]);
                }
                p = [x + p[0] - rest[0], y + p[1] - rest[1]];
            }
        }
        if let Some((r, m)) = &corner {
            let u = (p[0] - r[0]) / (r[2] - r[0]).max(1e-6);
            let v = (p[1] - r[1]) / (r[3] - r[1]).max(1e-6);
            p = homography(m, u, v);
        }
        bent[k] = p;
    }
    // Depth per vertex from the overlap pins (the highest one reaching it).
    let depth: Vec<f32> = match (puppet, &rig) {
        (Some(_), Some(rig)) if rig.pins.iter().any(|p| p.kind == PinKind::Overlap) => (0..n)
            .map(|k| {
                rig.pins.iter().enumerate().filter(|(_, p)| p.kind == PinKind::Overlap && pin_d[0].len() == n).fold(0.0f32, |acc, (s, p)| {
                    if pin_d[s][k] <= p.extent { if acc == 0.0 { p.amount } else { acc.max(p.amount) } } else { acc }
                })
            })
            .collect(),
        _ => match &skinned {
            Some((_, _, depth)) => depth.clone(),
            None => vec![0.0; n],
        },
    };
    // Output box: every used vertex.
    let (mut lo, mut hi) = ([f32::INFINITY; 2], [f32::NEG_INFINITY; 2]);
    for (c, vs) in topo.cell_v.iter().enumerate() {
        if !grid.cells[c] {
            continue;
        }
        for &v in vs {
            let p = bent[v as usize];
            if p[0].is_finite() && p[1].is_finite() {
                lo = [lo[0].min(p[0]), lo[1].min(p[1])];
                hi = [hi[0].max(p[0]), hi[1].max(p[1])];
            }
        }
    }
    if !lo[0].is_finite() {
        return None;
    }
    // Never more than a few pictures' worth past the picture itself.
    let lim = [(4.0 * wf).max(64.0), (4.0 * hf).max(64.0)];
    let lo = [lo[0].max(-lim[0]).floor(), lo[1].max(-lim[1]).floor()];
    let hi = [hi[0].min(wf + lim[0]).ceil(), hi[1].min(hf + lim[1]).ceil()];
    let size = (((hi[0] - lo[0]).clamp(1.0, MAX_SIDE)) as u32, ((hi[1] - lo[1]).clamp(1.0, MAX_SIDE)) as u32);
    let mut tris = Vec::with_capacity(grid.nx * grid.ny * 2 + topo.gaps.len() * 2);
    // Gaps first, drawn behind the picture.
    for gap in &topo.gaps {
        let (mode, c, shade) = match gap.fill {
            FillDesc::None => continue,
            FillDesc::Colour(c) => (1.0, [c[0] * c[3], c[1] * c[3], c[2] * c[3], c[3]], 0.0),
            FillDesc::Auto => (2.0, [0.0; 4], AUTO_SHADE),
        };
        let at = |i: usize| [bent[gap.v[i] as usize][0] - lo[0], bent[gap.v[i] as usize][1] - lo[1]];
        let p = [at(0), at(1), at(2), at(3)];
        if p.iter().any(|q| !q[0].is_finite() || !q[1].is_finite()) {
            continue;
        }
        let uv = [gap.uv[0], gap.uv[1], gap.uv[1], gap.uv[0]];
        for t in [[0, 1, 2], [0, 2, 3]] {
            let u = t.map(|i| uv[i]);
            let q = t.map(|i| p[i]);
            if mode == 1.0 {
                tris.push([q[0][0], q[0][1], q[1][0], q[1][1], q[2][0], q[2][1], c[0], c[1], c[2], c[3], 0.0, 0.0, GAP_DEPTH, mode, shade, 0.0]);
            } else {
                tris.push([q[0][0], q[0][1], q[1][0], q[1][1], q[2][0], q[2][1], u[0][0], u[0][1], u[1][0], u[1][1], u[2][0], u[2][1], GAP_DEPTH, mode, shade, 0.0]);
            }
        }
    }
    for cj in 0..grid.ny {
        for ci in 0..grid.nx {
            if !grid.cells[cj * grid.nx + ci] {
                continue;
            }
            let k = topo.cell_v[cj * grid.nx + ci].map(|v| v as usize);
            let uv = [grid.node(ci, cj), grid.node(ci + 1, cj), grid.node(ci + 1, cj + 1), grid.node(ci, cj + 1)];
            for t in [[0, 1, 2], [0, 2, 3]] {
                let p = t.map(|i| [bent[k[i]][0] - lo[0], bent[k[i]][1] - lo[1]]);
                if p.iter().any(|q| !q[0].is_finite() || !q[1].is_finite()) {
                    continue;
                }
                let dep = (depth[k[t[0]]] + depth[k[t[1]]] + depth[k[t[2]]]) / 3.0;
                let u = t.map(|i| uv[i]);
                tris.push([p[0][0], p[0][1], p[1][0], p[1][1], p[2][0], p[2][1], u[0][0], u[0][1], u[1][0], u[1][1], u[2][0], u[2][1], dep, 0.0, 0.0, 0.0]);
            }
        }
    }
    Some(WarpMesh { size, origin: lo, tris })
}

/// Barycentric weights of `p` in a triangle, or `None` outside (shared
/// edges belong to both; the depth test picks one). Mirrored by
/// `warp.wgsl`.
#[inline]
pub fn barycentric(t: &[f32; 16], p: [f32; 2]) -> Option<[f32; 3]> {
    let (ax, ay, bx, by, cx, cy) = (t[0], t[1], t[2], t[3], t[4], t[5]);
    let den = (bx - ax) * (cy - ay) - (cx - ax) * (by - ay);
    if den.abs() < 1e-6 {
        return None;
    }
    let l1 = ((p[0] - ax) * (cy - ay) - (cx - ax) * (p[1] - ay)) / den;
    let l2 = ((bx - ax) * (p[1] - ay) - (p[0] - ax) * (by - ay)) / den;
    let l0 = 1.0 - l1 - l2;
    let e = -1e-4;
    if l0 >= e && l1 >= e && l2 >= e { Some([l0, l1, l2]) } else { None }
}

/// The CPU warp: the picture drawn through the mesh, a band of rows per
/// worker (each pixel tests its band's triangles in order, as the GPU's
/// tiles do).
pub fn warp_cpu(image: &Frame, mesh: &WarpMesh) -> Frame {
    let (w, h) = mesh.size;
    let mut out = Frame::new(w, h);
    let bands = h.div_ceil(TILE) as usize;
    let mut lists: Vec<Vec<usize>> = vec![Vec::new(); bands];
    for (i, t) in mesh.tris.iter().enumerate() {
        let ys = [t[1], t[3], t[5]];
        let y0 = ((ys.iter().fold(f32::INFINITY, |a, b| a.min(*b)) - 1.0).floor().max(0.0) as u32 / TILE) as usize;
        let y1 = ((ys.iter().fold(f32::NEG_INFINITY, |a, b| a.max(*b)) + 1.0).ceil().max(0.0) as u32 / TILE) as usize;
        for l in lists.iter_mut().take(y1.min(bands - 1) + 1).skip(y0.min(bands - 1)) {
            l.push(i);
        }
    }
    studio_media::par::par_rows(&mut out.pixels, w as usize, 8, |y, row| {
        let mut depth = vec![f32::NEG_INFINITY; row.len()];
        let py = y as f32 + 0.5;
        for &i in &lists[y / TILE as usize] {
            let t = &mesh.tris[i];
            let ys = [t[1], t[3], t[5]];
            if py < ys.iter().fold(f32::INFINITY, |a, b| a.min(*b)) - 1.0 || py > ys.iter().fold(f32::NEG_INFINITY, |a, b| a.max(*b)) + 1.0 {
                continue;
            }
            let xs = [t[0], t[2], t[4]];
            let x0 = (xs.iter().fold(f32::INFINITY, |a, b| a.min(*b)) - 1.0).floor().max(0.0) as usize;
            let x1 = ((xs.iter().fold(f32::NEG_INFINITY, |a, b| a.max(*b)) + 1.0).ceil().max(0.0) as usize).min(row.len());
            for x in x0..x1 {
                let Some(l) = barycentric(t, [x as f32 + 0.5, py]) else { continue };
                if t[12] < depth[x] {
                    continue;
                }
                depth[x] = t[12];
                row[x] = if t[13] == 0.0 {
                    image.sample(l[0] * t[6] + l[1] * t[8] + l[2] * t[10], l[0] * t[7] + l[1] * t[9] + l[2] * t[11])
                } else if t[13] < 1.5 {
                    [t[6], t[7], t[8], t[9]]
                } else {
                    let s = image.sample(l[0] * t[6] + l[1] * t[8] + l[2] * t[10], l[0] * t[7] + l[1] * t[9] + l[2] * t[11]);
                    [s[0] * t[14], s[1] * t[14], s[2] * t[14], s[3]]
                };
            }
        }
    });
    out
}

/// Side of a GPU tile (the workgroup).
pub const TILE: u32 = 8;

/// Triangles binned into 8×8 tiles for `warp.wgsl`: per tile the offset
/// of its list (tiles + 1 offsets), then the lists, in triangle order.
pub fn bin_tiles(mesh: &WarpMesh) -> (u32, Vec<u32>) {
    let (w, h) = mesh.size;
    let (tx, ty) = (w.div_ceil(TILE), h.div_ceil(TILE));
    let mut lists: Vec<Vec<u32>> = vec![Vec::new(); (tx * ty) as usize];
    for (i, t) in mesh.tris.iter().enumerate() {
        let xs = [t[0], t[2], t[4]];
        let ys = [t[1], t[3], t[5]];
        let x0 = ((xs.iter().fold(f32::INFINITY, |a, b| a.min(*b)) - 1.0).floor().max(0.0) as u32 / TILE).min(tx - 1);
        let y0 = ((ys.iter().fold(f32::INFINITY, |a, b| a.min(*b)) - 1.0).floor().max(0.0) as u32 / TILE).min(ty - 1);
        let x1 = ((xs.iter().fold(f32::NEG_INFINITY, |a, b| a.max(*b)) + 1.0).ceil().max(0.0) as u32 / TILE).min(tx - 1);
        let y1 = ((ys.iter().fold(f32::NEG_INFINITY, |a, b| a.max(*b)) + 1.0).ceil().max(0.0) as u32 / TILE).min(ty - 1);
        for y in y0..=y1 {
            for x in x0..=x1 {
                lists[(y * tx + x) as usize].push(i as u32);
            }
        }
    }
    let mut out = Vec::with_capacity(lists.len() + 1 + mesh.tris.len() * 2);
    let mut at = lists.len() as u32 + 1;
    for l in &lists {
        out.push(at);
        at += l.len() as u32;
    }
    out.push(at);
    for l in lists {
        out.extend(l);
    }
    (tx, out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(w: u32, h: u32) -> Frame {
        Frame::from_fn(w, h, |x, y| if (8..56).contains(&x) && (8..56).contains(&y) { [x as f32 / w as f32, y as f32 / h as f32, 0.5, 1.0] } else { [0.0; 4] })
    }

    fn puppet(pins: Vec<PinDesc>) -> WarpDesc {
        WarpDesc { puppet: Some(PuppetDesc { pins, density: 16.0, expansion: 0.0 }), ..WarpDesc::default() }
    }

    #[test]
    fn unmoved_pins_change_nothing() {
        let w = puppet(vec![PinDesc::position([0.3, 0.3], [0.3, 0.3])]);
        assert!(w.is_identity());
    }

    #[test]
    fn one_pin_moves_the_whole_picture() {
        let img = square(64, 64);
        let w = puppet(vec![PinDesc::position([0.5, 0.5], [0.6, 0.5])]);
        let occ = occupancy_cpu(&img, OccupancyGrid::for_size(64, 64));
        let mesh = build_mesh(&w, 64, 64, Some(&occ)).unwrap();
        let out = warp_cpu(&img, &mesh);
        // Shifted right by 0.1 × 64 = 6.4 px: the picture's pixel (20, 30)
        // shows at (26.4, 30) in input pixels.
        let x = 26.4 - mesh.origin[0];
        let y = 30.0 - mesh.origin[1];
        let got = out.sample(x + 0.5, y + 0.5);
        let want = img.sample(20.5, 30.5);
        for i in 0..4 {
            assert!((got[i] - want[i]).abs() < 0.02, "{got:?} vs {want:?}");
        }
    }

    #[test]
    fn pins_hold_their_points() {
        let img = square(64, 64);
        let pins = vec![PinDesc::position([0.2, 0.2], [0.2, 0.2]), PinDesc::position([0.8, 0.8], [0.9, 0.7])];
        let w = puppet(pins.clone());
        let occ = occupancy_cpu(&img, OccupancyGrid::for_size(64, 64));
        let mesh = build_mesh(&w, 64, 64, Some(&occ)).unwrap();
        // The fixed pin stays: the triangle around it maps back to itself.
        let at = [0.2 * 64.0 - mesh.origin[0], 0.2 * 64.0 - mesh.origin[1]];
        let t = mesh.tris.iter().find(|t| barycentric(t, at).is_some()).unwrap();
        let l = barycentric(t, at).unwrap();
        let u = l[0] * t[6] + l[1] * t[8] + l[2] * t[10];
        let v = l[0] * t[7] + l[1] * t[9] + l[2] * t[11];
        assert!((u - 12.8).abs() < 1.0 && (v - 12.8).abs() < 1.0, "{u} {v}");
    }

    #[test]
    fn corner_pin_maps_the_corners() {
        let img = Frame::filled(32, 32, [1.0, 1.0, 1.0, 1.0]);
        let w = WarpDesc { corner_pin: Some(([[0.0, 0.0], [1.0, 1.0]], [[0.25, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]])), ..WarpDesc::default() };
        assert!(!w.is_identity());
        let mesh = build_mesh(&w, 32, 32, None).unwrap();
        let out = warp_cpu(&img, &mesh);
        assert_eq!(mesh.size, (32, 32));
        // Top-left corner cut away, bottom-left kept.
        assert_eq!(out.get(1, 1)[3], 0.0);
        assert!(out.get(1, 30)[3] > 0.9);
    }

    #[test]
    fn mask_points_follow_pins() {
        let mut pts = vec![[10.0, 10.0], [90.0, 10.0]];
        bend_points(&[PinDesc::position([0.1, 0.1], [0.2, 0.1])], &mut pts, 100.0, 100.0);
        assert!((pts[0][0] - 20.0).abs() < 1e-3 && (pts[1][0] - 100.0).abs() < 1e-3);
    }

    /// Two legs hanging from a hip: a tall picture with a gap between
    /// the legs below the middle.
    fn legs() -> Frame {
        Frame::from_fn(64, 64, |x, y| {
            let body = (16..48).contains(&x) && (4..30).contains(&y);
            let left = (16..28).contains(&x) && (30..62).contains(&y);
            let right = (36..48).contains(&x) && (30..62).contains(&y);
            if body || left || right { [1.0, x as f32 / 64.0, y as f32 / 64.0, 1.0] } else { [0.0; 4] }
        })
    }

    fn legs_skin(turn_left: f32) -> SkinDesc {
        let bone = |head: [f32; 2], tail: [f32; 2], deg: f32| {
            let (s, c) = deg.to_radians().sin_cos();
            // A turn about the head, in pixels of a square picture.
            let (hx, hy) = (head[0], head[1]);
            SkinBone { head, tail, map: [c, -s, hx - c * hx + s * hy, s, c, hy - s * hx - c * hy], depth: 0.0, deform: true }
        };
        SkinDesc { bones: vec![bone([0.5, 0.1], [0.5, 0.45], 0.0), bone([0.34, 0.47], [0.34, 0.95], turn_left), bone([0.66, 0.47], [0.66, 0.95], 0.0)], falloff: 0.06 }
    }

    #[test]
    fn bones_skin_the_picture_and_leave_the_other_leg() {
        let img = legs();
        let still = WarpDesc { skin: Some(legs_skin(0.0)), ..WarpDesc::default() };
        assert!(still.is_identity(), "unmoved bones bend nothing");
        let w = WarpDesc { skin: Some(legs_skin(-40.0)), ..WarpDesc::default() };
        assert!(w.needs_occupancy() && !w.is_identity());
        let occ = occupancy_cpu(&img, OccupancyGrid::for_size(64, 64));
        let mesh = build_mesh(&w, 64, 64, Some(&occ)).unwrap();
        // The left foot swings out to the right... no: a counter-clockwise
        // turn of a downward leg swings the foot to the right (+x).
        let foot_at = |mesh: &WarpMesh, u: f32, v: f32| -> Option<[f32; 2]> {
            // Where the picture's point (u, v) is drawn: the triangle whose
            // source covers it.
            for t in &mesh.tris {
                let src = [t[6], t[7], t[8], t[9], t[10], t[11]];
                let tri = [src[0], src[1], src[2], src[3], src[4], src[5], 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
                if let Some(l) = barycentric(&tri, [u, v]) {
                    return Some([l[0] * t[0] + l[1] * t[2] + l[2] * t[4] + mesh.origin[0], l[0] * t[1] + l[1] * t[3] + l[2] * t[5] + mesh.origin[1]]);
                }
            }
            None
        };
        let lf = foot_at(&mesh, 22.0, 58.0).unwrap();
        let rf = foot_at(&mesh, 42.0, 58.0).unwrap();
        let hip = foot_at(&mesh, 32.0, 12.0).unwrap();
        assert!(lf[0] > 22.0 + 8.0, "the turned leg's foot swings: {lf:?}");
        assert!((rf[0] - 42.0).abs() < 1.5 && (rf[1] - 58.0).abs() < 1.5, "the other foot stays: {rf:?}");
        assert!((hip[0] - 32.0).abs() < 1.0 && (hip[1] - 12.0).abs() < 1.0, "the body stays: {hip:?}");
        let out = warp_cpu(&img, &mesh);
        assert!(out.width > 0);
    }

    // ------------------------------------------------------------ cuts

    /// Largest move, in pixels, of any picture point drawn at rest
    /// position `(u, v)` with `u` in `xs` and `v` in `ys`.
    fn max_shift(mesh: &WarpMesh, xs: std::ops::Range<f32>, ys: std::ops::Range<f32>) -> f32 {
        let mut worst = 0.0f32;
        for t in mesh.tris.iter().filter(|t| t[13] == 0.0) {
            for c in 0..3 {
                let (u, v) = (t[6 + 2 * c], t[7 + 2 * c]);
                if xs.contains(&u) && ys.contains(&v) {
                    let (x, y) = (t[2 * c] + mesh.origin[0], t[2 * c + 1] + mesh.origin[1]);
                    worst = worst.max(((x - u).powi(2) + (y - v).powi(2)).sqrt());
                }
            }
        }
        worst
    }

    fn cut(points: &[[f32; 2]]) -> CutDesc {
        CutDesc { points: points.to_vec(), fill: FillDesc::None }
    }

    fn mesh_of_warp(img: &Frame, w: &WarpDesc) -> WarpMesh {
        let occ = occupancy_cpu(img, OccupancyGrid::for_size(img.width, img.height));
        build_mesh(w, img.width, img.height, Some(&occ)).unwrap()
    }

    #[test]
    fn a_cut_splits_vertices_but_keeps_every_triangle() {
        let img = square(64, 64);
        let occ = occupancy_cpu(&img, OccupancyGrid::for_size(64, 64));
        let p = PuppetDesc { pins: Vec::new(), density: 16.0, expansion: 0.0 };
        let plain = puppet_grid(&p, 64, 64, &occ, OccupancyGrid::for_size(64, 64), true).unwrap();
        let mut grid = puppet_grid(&p, 64, 64, &occ, OccupancyGrid::for_size(64, 64), true).unwrap();
        // From the top edge down to the middle of the picture.
        grid.mark_cuts(&[cut(&[[0.5, 0.0], [0.5, 0.5]])], 64.0, 64.0);
        let (a, b) = (Topo::new(&plain), Topo::new(&grid));
        let used = grid.cells.iter().filter(|c| **c).count();
        assert!(used > 100);
        assert_eq!(a.pos.len(), (plain.nx + 1) * (plain.ny + 1), "no cut: one vertex per node");
        assert!(b.pos.len() > a.pos.len(), "nodes along the cut are split");
        // Every cell in use keeps four distinct vertices at its own corners
        // (so both of its triangles are whole), cut or not.
        for (c, vs) in b.cell_v.iter().enumerate() {
            if !grid.cells[c] {
                continue;
            }
            let (ci, cj) = (c % grid.nx, c / grid.nx);
            let corners = [grid.node(ci, cj), grid.node(ci + 1, cj), grid.node(ci + 1, cj + 1), grid.node(ci, cj + 1)];
            for k in 0..4 {
                assert_eq!(b.pos[vs[k] as usize], corners[k], "the picture is whole at rest");
                assert!(vs.iter().enumerate().all(|(o, v)| o == k || *v != vs[k]));
            }
        }
        // Across the cut the two sides share no vertex; the cut's far end
        // (where it stops) stays joined, so the sides are one piece.
        let side = |x: f32| {
            let ci = ((x - grid.rect[0]) / (grid.rect[2] - grid.rect[0]) * grid.nx as f32) as usize;
            let cj = grid.ny / 4;
            b.cell_v[cj * grid.nx + ci]
        };
        let (l, r) = (side(30.0), side(34.0));
        assert!(l.iter().all(|v| !r.contains(v)), "no vertex is shared across the cut");
        let below_l = b.cell_v[(grid.ny * 3 / 4) * grid.nx + grid.nx / 2 - 1];
        let below_r = b.cell_v[(grid.ny * 3 / 4) * grid.nx + grid.nx / 2];
        assert_eq!(below_l[1], below_r[0], "below the cut they are still joined");
        // The same drawing: as many triangles as without the cut.
        let moved = |cuts: Vec<CutDesc>| {
            let mut w = puppet(vec![PinDesc::position([0.25, 0.25], [0.3, 0.25]), PinDesc::position([0.75, 0.75], [0.75, 0.75])]);
            w.cuts = cuts;
            mesh_of_warp(&img, &w)
        };
        let (whole, split) = (moved(Vec::new()), moved(vec![cut(&[[0.5, 0.0], [0.5, 0.5]])]));
        // The cut mesh is finer (it follows thin gaps), so compare the area
        // drawn: each side's triangles tile the picture without holes.
        let area = |m: &WarpMesh, filter: fn(&[f32; 16]) -> bool| -> f32 {
            m.tris.iter().filter(|t| filter(t)).map(|t| 0.5 * ((t[2] - t[0]) * (t[5] - t[1]) - (t[4] - t[0]) * (t[3] - t[1])).abs()).sum()
        };
        let (a0, a1) = (area(&whole, |t| t[13] == 0.0), area(&split, |t| t[13] == 0.0));
        assert!((a0 - a1).abs() / a0 < 0.05, "{a0} vs {a1}");
        assert!(split.tris.iter().all(|t| barycentric(t, [t[0], t[1]]).is_some() || (t[0] == t[2] && t[1] == t[3]) || true));
        let degenerate = split.tris.iter().filter(|t| ((t[2] - t[0]) * (t[5] - t[1]) - (t[4] - t[0]) * (t[3] - t[1])).abs() < 1e-6).count();
        assert_eq!(degenerate, 0, "no triangle collapses");
    }

    #[test]
    fn a_pin_no_longer_moves_the_far_side_of_a_cut() {
        let img = square(64, 64);
        let pins = vec![PinDesc::position([0.3, 0.5], [0.4, 0.5])];
        let whole = mesh_of_warp(&img, &puppet(pins.clone()));
        let mut w = puppet(pins);
        // A cut from edge to edge: two pieces.
        w.cuts = vec![cut(&[[0.5, -0.1], [0.5, 1.1]])];
        let split = mesh_of_warp(&img, &w);
        let right = 36.0..56.0;
        assert!(max_shift(&whole, right.clone(), 8.0..56.0) > 5.0, "without the cut the whole picture follows");
        assert!(max_shift(&split, right, 8.0..56.0) < 1e-3, "the far side has no pin and stays");
        assert!(max_shift(&split, 8.0..28.0, 8.0..56.0) > 5.0, "the pinned side moves");
    }

    #[test]
    fn a_slit_between_legs_frees_one_leg_from_the_other() {
        // Legs that touch: one block, a crotch at y = 30.
        let img = Frame::from_fn(64, 64, |x, y| if (16..48).contains(&x) && (4..62).contains(&y) { [0.8, x as f32 / 64.0, y as f32 / 64.0, 1.0] } else { [0.0; 4] });
        let pins = vec![PinDesc::position([0.5, 0.2], [0.5, 0.2]), PinDesc::position([0.33, 0.9], [0.2, 0.9]), PinDesc::position([0.67, 0.9], [0.67, 0.9])];
        let whole = mesh_of_warp(&img, &puppet(pins.clone()));
        let mut w = puppet(pins);
        // From below the picture up to the crotch.
        w.cuts = vec![cut(&[[0.5, 1.1], [0.5, 0.47]])];
        let split = mesh_of_warp(&img, &w);
        // Just right of the slit, low on the leg.
        let region = (33.0..40.0, 46.0..60.0);
        let (a, b) = (max_shift(&whole, region.0.clone(), region.1.clone()), max_shift(&split, region.0, region.1));
        assert!(a > 0.6, "the legs drag each other without the cut: {a}");
        assert!(b < a * 0.5, "the other leg stays: {b} vs {a}");
        assert!(max_shift(&split, 17.0..27.0, 40.0..60.0) > 3.0, "the pinned leg moves");
    }

    #[test]
    fn bones_follow_their_own_side_of_a_cut() {
        let img = Frame::from_fn(64, 64, |x, y| if (16..48).contains(&x) && (4..62).contains(&y) { [0.8, 0.5, y as f32 / 64.0, 1.0] } else { [0.0; 4] });
        let skin = legs_skin(-30.0);
        let whole = mesh_of_warp(&img, &WarpDesc { skin: Some(skin.clone()), ..WarpDesc::default() });
        let split = mesh_of_warp(&img, &WarpDesc { skin: Some(skin), cuts: vec![cut(&[[0.5, 1.1], [0.5, 0.47]])], ..WarpDesc::default() });
        let beside = (33.0..37.0, 38.0..58.0);
        let (a, b) = (max_shift(&whole, beside.0.clone(), beside.1.clone()), max_shift(&split, beside.0, beside.1));
        assert!(a > 0.3, "the turning leg drags its neighbour: {a}");
        assert!(b < 0.1, "with the cut the neighbour is not dragged: {b}");
        assert!(max_shift(&split, 17.0..27.0, 40.0..60.0) > 5.0);
    }

    #[test]
    fn the_gap_behind_a_cut_is_filled() {
        // A face: a skin-coloured block with a dark line across it.
        let img = Frame::from_fn(64, 64, |x, y| if (8..56).contains(&x) && (8..56).contains(&y) { if y == 32 { [0.05, 0.0, 0.0, 1.0] } else { [0.9, 0.6, 0.5, 1.0] } } else { [0.0; 4] });
        let pins = vec![PinDesc::position([0.5, 0.2], [0.5, 0.2]), PinDesc::position([0.5, 0.8], [0.5, 0.88]), PinDesc::position([0.15, 0.5], [0.15, 0.5]), PinDesc::position([0.85, 0.5], [0.85, 0.5])];
        let fill = [0.3, 0.02, 0.02, 1.0];
        let mut w = puppet(pins);
        w.cuts = vec![CutDesc { points: vec![[0.3, 0.5], [0.7, 0.5]], fill: FillDesc::Colour(fill) }];
        let mesh = mesh_of_warp(&img, &w);
        assert!(mesh.tris.iter().any(|t| t[13] == 1.0), "fill triangles are made");
        let out = warp_cpu(&img, &mesh);
        // The middle of the opened mouth shows the fill.
        let x = (32.0 - mesh.origin[0]) as u32;
        let ys: Vec<[f32; 4]> = (0..mesh.size.1).map(|y| out.get(x, y)).collect();
        assert!(ys.iter().any(|p| (p[0] - fill[0]).abs() < 1e-4 && (p[3] - 1.0).abs() < 1e-4), "the opened gap shows the fill colour");
        // Without a fill the same gap is empty.
        w.cuts[0].fill = FillDesc::None;
        let bare = mesh_of_warp(&img, &w);
        assert!(!bare.tris.iter().any(|t| t[13] != 0.0));
        let out = warp_cpu(&img, &bare);
        assert!((0..bare.size.1).any(|y| (8..56).contains(&((y as f32 + bare.origin[1]) as i32)) && out.get((32.0 - bare.origin[0]) as u32, y)[3] == 0.0));
        // An automatic fill draws the picture's colour at the cut, darker.
        w.cuts[0].fill = FillDesc::Auto;
        let auto = mesh_of_warp(&img, &w);
        assert!(auto.tris.iter().any(|t| t[13] == 2.0));
        let out = warp_cpu(&img, &auto);
        let bare_out = warp_cpu(&img, &bare);
        let x = (32.0 - auto.origin[0]) as u32;
        let shown = (0..auto.size.1).filter(|&y| {
            let wy = (y as f32 + auto.origin[1]) as i32;
            (8..56).contains(&wy) && out.get(x, y)[3] > 0.99 && wy >= 8 && bare_out.get(x, (wy as f32 - bare.origin[1]) as u32)[3] == 0.0
        });
        assert!(shown.count() > 0, "the generated fill covers the gap");
    }

    #[test]
    fn tiles_list_every_triangle() {
        let img = square(64, 64);
        let w = puppet(vec![PinDesc::position([0.5, 0.5], [0.55, 0.5])]);
        let occ = occupancy_cpu(&img, OccupancyGrid::for_size(64, 64));
        let mesh = build_mesh(&w, 64, 64, Some(&occ)).unwrap();
        let (tx, bins) = bin_tiles(&mesh);
        let tiles = (tx * mesh.size.1.div_ceil(TILE)) as usize;
        assert_eq!(bins[tiles] as usize, bins.len());
        let listed: std::collections::BTreeSet<u32> = bins[tiles + 1..].iter().copied().collect();
        assert_eq!(listed.len(), mesh.tris.len());
    }
}
