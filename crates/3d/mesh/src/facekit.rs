// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The parts of a face that are not skin, each its own geometry:
//!
//! * eyelids, an upper and a lower for each eye (`Upper lid`, `Lower lid`
//!   materials): shells round the eyeball that close over it without
//!   touching it, half following the eyeball's gaze;
//! * eyelashes (`Lashes`) on each lid's margin, moving with it;
//! * corneas (`Cornea`): a clear, wet dome over each iris, riding on the
//!   eye bone;
//! * teeth (`Teeth`), gums (`Gums`) and a tongue (`Tongue`) in the mouth
//!   (`Mouth` is the dark inside), the lower row and the tongue following
//!   the jaw.
//!
//! Their controls are shape keys, named for lip-sync and face capture tools:
//! `eyeBlinkLeft/Right`, `eyeSquintLeft/Right`, `eyeWideLeft/Right` (the
//! lids and lashes); `pupilDilate`, `pupilConstrict` (the pupil's size);
//! `tongueOut`, `tongueUp`, `tongueDown`, `tongueCurl`, `tongueTH` (a tip
//! between the teeth). The jaw's own keys (`jawOpen`, visemes) already move
//! the lower teeth and the tongue.
//!
//! Coordinates are the head base's: units of its size from the frame's
//! origin (the middle of the eyes at the front of the eyeballs), x to the
//! character's left, y down, the face towards -z.

use crate::face::HeadFrame;
use crate::loft;
use crate::rig::ShapeKey;
use crate::{Material, Mesh, V3, add, scale, srgb_to_linear, sub};

/// Where each part's vertices are in the model.
#[derive(Clone, Debug, Default)]
pub struct Kit {
    /// Per eye (left = +x first): upper lid, lower lid, upper lashes, lower lashes.
    pub lids: [[std::ops::Range<usize>; 4]; 2],
    /// Per eye: the cornea.
    pub cornea: [std::ops::Range<usize>; 2],
    /// Teeth, gums, tongue (inside the head's own range).
    pub teeth: std::ops::Range<usize>,
    pub gums: std::ops::Range<usize>,
    pub tongue: std::ops::Range<usize>,
}

fn lin(c: [u8; 3]) -> [f32; 4] {
    [srgb_to_linear(c[0]), srgb_to_linear(c[1]), srgb_to_linear(c[2]), 1.0]
}

fn push(mesh: &mut Mesh, part: &Mesh, mat: u32) -> std::ops::Range<usize> {
    let mut mats = mesh.tri_materials();
    mats.extend(std::iter::repeat_n(mat, part.triangles()));
    let start = mesh.positions.len();
    let base = start as u32;
    mesh.positions.extend_from_slice(&part.positions);
    mesh.normals.extend_from_slice(&part.normals);
    mesh.uvs.extend_from_slice(&part.uvs);
    mesh.indices.extend(part.indices.iter().map(|i| i + base));
    mesh.regroup(&mats);
    start..mesh.positions.len()
}

fn material(mesh: &mut Mesh, m: Material) -> u32 {
    if let Some(i) = mesh.materials.iter().position(|x| x.name == m.name) {
        return i as u32;
    }
    mesh.materials.push(m);
    (mesh.materials.len() - 1) as u32
}

/// Turns `p` about the x axis through `c` by `deg` (positive: the front
/// goes down and back, a lid closing).
fn turn_x(p: V3, c: V3, deg: f32) -> V3 {
    let (s, k) = deg.to_radians().sin_cos();
    let d = sub(p, c);
    [p[0], c[1] + d[1] * k - d[2] * s, c[2] + d[1] * s + d[2] * k]
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The direction from an eye's centre at azimuth `phi` (across, to the
/// character's left positive) and elevation `theta` (up positive), degrees.
fn dir(phi: f32, theta: f32) -> V3 {
    let (p, t) = (phi.to_radians(), theta.to_radians());
    [p.sin() * t.cos(), -t.sin(), -p.cos() * t.cos()]
}

/// Adds the face parts to a model whose head base is at `frame`, with the
/// head's vertices `head` (extended to take the teeth and tongue). `eye_white`
/// style materials are found by name in the model.
pub fn build(mesh: &mut Mesh, head: &mut std::ops::Range<usize>, frame: HeadFrame, lash_colour: [u8; 3]) -> Kit {
    let s = frame.size;
    let o = frame.origin;
    let w = |p: V3| add(o, scale(p, s));
    let mut kit = Kit::default();
    // ---- the lips: an upper and a lower tube round the mouth's opening,
    // meeting at the corners, the upper with its bow, the lower fuller.
    let lips_m = material(mesh, Material { name: "Lips".into(), base_colour: lin([178, 112, 102]), roughness: 0.42, subsurface: 0.7, ..Material::default() });
    let lip = |upper: bool| -> Mesh {
        let n = 33;
        let path: Vec<V3> = (0..n)
            .map(|i| {
                let x = -0.09 + 0.18 * i as f32 / (n - 1) as f32;
                let u = (x / 0.09).abs().min(1.0);
                let arc = (1.0 - u * u).max(0.0);
                let y = if upper { 0.289 - 0.025 * arc.powf(0.8) + 0.003 * (1.0 - (x.abs() / 0.018)).max(0.0) } else { 0.288 + 0.017 * arc.powf(0.9) };
                [x, y, -0.358 + 0.3 * x * x + if upper { 0.0 } else { 0.004 * arc }]
            })
            .map(w)
            .collect();
        let radii: Vec<f32> = (0..n)
            .map(|i| {
                let x = -0.09 + 0.18 * i as f32 / (n - 1) as f32;
                let u = (x / 0.09).abs().min(1.0);
                s * (if upper { 0.0125 } else { 0.0155 }) * (1.0 - 0.55 * u.powi(3))
            })
            .collect();
        loft::tube(&path, &radii, 10)
    };
    let mut lips = lip(true);
    let lower = lip(false);
    let base = lips.positions.len() as u32;
    lips.positions.extend_from_slice(&lower.positions);
    lips.normals.extend_from_slice(&lower.normals);
    lips.uvs.extend_from_slice(&lower.uvs);
    lips.indices.extend(lower.indices.iter().map(|i| i + base));
    // The corners: a small round of lip at each end.
    for sx in [1.0f32, -1.0] {
        let mut c = crate::prim::uv_sphere(0.0085 * s, 10, 6);
        let at = w([0.09 * sx, 0.288, -0.349]);
        for p in &mut c.positions {
            *p = add(*p, at);
        }
        let b = lips.positions.len() as u32;
        lips.positions.extend_from_slice(&c.positions);
        lips.normals.extend_from_slice(&c.normals);
        lips.uvs.extend_from_slice(&c.uvs);
        lips.indices.extend(c.indices.iter().map(|i| i + b));
    }
    push(mesh, &lips, lips_m);
    // ---- the mouth: teeth, gums, tongue (inside the head's range)
    let teeth_m = material(mesh, Material { name: "Teeth".into(), base_colour: lin([238, 232, 216]), roughness: 0.3, subsurface: 0.2, ..Material::default() });
    let gums_m = material(mesh, Material { name: "Gums".into(), base_colour: lin([178, 84, 92]), roughness: 0.45, subsurface: 0.5, ..Material::default() });
    let tongue_m = material(mesh, Material { name: "Tongue".into(), base_colour: lin([192, 88, 98]), roughness: 0.4, subsurface: 0.6, ..Material::default() });
    let arch = |y: f32, back: f32, n: usize| -> Vec<V3> { (0..n).map(|i| { let x = -0.066 + 0.132 * i as f32 / (n - 1) as f32; [x, y, -0.306 + back + 15.0 * x * x] }).collect() };
    let row = |y: f32, upper: bool| -> Mesh {
        let n = 37;
        let path: Vec<V3> = arch(y, 0.0, n).into_iter().map(w).collect();
        // A bulge for every tooth along the row.
        let radii: Vec<f32> = (0..n).map(|i| s * 0.0105 * (0.8 + 0.2 * (i as f32 / (n - 1) as f32 * 6.0 * std::f32::consts::PI).cos().powi(2))).collect();
        let mut m = loft::tube(&path, &radii, 8);
        let cy = w([0.0, y, 0.0])[1];
        // Taller than deep; the upper row's edge reaches down, the lower's up.
        loft::deform(&mut m, |p| [p[0], cy + (p[1] - cy) * 1.6 + if upper { s * 0.004 } else { -s * 0.004 }, p[2]]);
        m
    };
    let up = row(0.2705, true);
    let low = row(0.3045, false);
    let mut teeth = up;
    let base = teeth.positions.len() as u32;
    teeth.positions.extend_from_slice(&low.positions);
    teeth.normals.extend_from_slice(&low.normals);
    teeth.uvs.extend_from_slice(&low.uvs);
    teeth.indices.extend(low.indices.iter().map(|i| i + base));
    kit.teeth = push(mesh, &teeth, teeth_m);
    let gum = |y: f32| -> Mesh {
        let path: Vec<V3> = arch(y, 0.012, 25).into_iter().map(w).collect();
        loft::tube(&path, &vec![s * 0.0085; 25], 8)
    };
    let mut gums = gum(0.255);
    let g2 = gum(0.32);
    let base = gums.positions.len() as u32;
    gums.positions.extend_from_slice(&g2.positions);
    gums.normals.extend_from_slice(&g2.normals);
    gums.uvs.extend_from_slice(&g2.uvs);
    gums.indices.extend(g2.indices.iter().map(|i| i + base));
    kit.gums = push(mesh, &gums, gums_m);
    // The tongue: a flattened tube from the throat to the tip, resting on the lower teeth.
    let path: Vec<V3> = (0..9).map(|i| { let t = i as f32 / 8.0; w([0.0, 0.318 - 0.006 * t, -0.19 - 0.115 * t]) }).collect();
    let radii: Vec<f32> = (0..9).map(|i| s * (0.03 - 0.012 * (i as f32 / 8.0).powi(2))).collect();
    let mut tongue = loft::tube(&path, &radii, 10);
    let cy = w([0.0, 0.318, 0.0])[1];
    loft::deform(&mut tongue, |p| [o[0] + (p[0] - o[0]) * 1.2, cy + (p[1] - cy) * 0.42, p[2]]);
    kit.tongue = push(mesh, &tongue, tongue_m);
    head.end = mesh.positions.len();
    // ---- eyes: lids, lashes, corneas
    let lid_m = [
        material(mesh, Material { name: "Upper lid".into(), base_colour: mesh.materials.first().map_or([0.7, 0.5, 0.4, 1.0], |m| m.base_colour), roughness: 0.52, subsurface: 0.6, ..Material::default() }),
        material(mesh, Material { name: "Lower lid".into(), base_colour: mesh.materials.first().map_or([0.7, 0.5, 0.4, 1.0], |m| m.base_colour), roughness: 0.52, subsurface: 0.6, ..Material::default() }),
    ];
    let lash_m = material(mesh, Material { name: "Lashes".into(), base_colour: lin(lash_colour), roughness: 0.6, ..Material::default() });
    let cornea_m = material(mesh, Material { name: "Cornea".into(), base_colour: [0.95, 0.97, 1.0, 0.1], roughness: 0.012, transmission: 0.97, ior: 1.38, ..Material::default() });
    let re = 0.0525 * s;
    for (k, sx) in [1.0f32, -1.0].into_iter().enumerate() {
        let c = add(o, [0.12 * s * sx, 0.0, -0.305 * s]);
        let rl = re * 1.1;
        // Margins: the upper lid's edge lies above the pupil's centre, the lower below.
        let up_margin = |phi: f32| 24.0 * (phi.to_radians() * 1.15).cos() - 4.0;
        let low_margin = |phi: f32| -19.0 * (phi.to_radians() * 1.1).cos() - 1.0;
        let upper = loft::surface(
            28,
            9,
            false,
            |u, v| {
                let phi = -72.0 + 144.0 * u;
                let theta = up_margin(phi) + (74.0 - up_margin(phi)) * v;
                add(c, scale(dir(phi, theta), rl))
            },
            |p| sub(p, c),
        );
        let lower = loft::surface(
            28,
            6,
            false,
            |u, v| {
                let phi = -70.0 + 140.0 * u;
                let theta = low_margin(phi) - (60.0 + low_margin(phi)) * v;
                add(c, scale(dir(phi, theta), rl))
            },
            |p| sub(p, c),
        );
        kit.lids[k][0] = push(mesh, &upper, lid_m[0]);
        kit.lids[k][1] = push(mesh, &lower, lid_m[1]);
        // Lashes: curved hairs fanned along each lid's margin.
        let lash = |margin: &dyn Fn(f32) -> f32, count: usize, len: f32, up_sign: f32| -> Mesh {
            let mut out = Mesh::default();
            for i in 0..count {
                let phi = -62.0 + 124.0 * i as f32 / (count - 1) as f32;
                let theta = margin(phi);
                let d = dir(phi, theta);
                let base = add(c, scale(d, rl));
                // Out along the surface normal, bending away from the eye.
                let l = len * (0.65 + 0.35 * (1.0 - (phi / 62.0).abs().powi(2)));
                let tip_dir = [d[0] * 0.9 + 0.25 * phi.signum() * (phi.abs() / 62.0), d[1] + up_sign * 0.55, d[2] * 0.9];
                let tip_dir = crate::normalise(tip_dir);
                let mid = add(base, scale(add(d, scale(tip_dir, 0.5)), l * 0.5));
                let tip = add(base, scale(tip_dir, l));
                let t = loft::tube(&[base, mid, tip], &[s * 0.0021, s * 0.0016, s * 0.0007], 4);
                let b = out.positions.len() as u32;
                out.positions.extend_from_slice(&t.positions);
                out.normals.extend_from_slice(&t.normals);
                out.uvs.extend_from_slice(&t.uvs);
                out.indices.extend(t.indices.iter().map(|x| x + b));
            }
            out
        };
        kit.lids[k][2] = push(mesh, &lash(&up_margin, 20, 0.036 * s, -1.0), lash_m);
        kit.lids[k][3] = push(mesh, &lash(&low_margin, 12, 0.02 * s, 1.0), lash_m);
        // The cornea: a clear dome, a little proud at its middle.
        let cornea = loft::surface(
            24,
            6,
            true,
            |u, v| {
                let a = u * std::f32::consts::TAU;
                let ang = 38.0f32.to_radians() * v;
                let r = re * (1.02 + 0.06 * (1.0 - v * v));
                add(c, [r * ang.sin() * a.cos(), -r * ang.sin() * a.sin(), -r * ang.cos()])
            },
            |p| sub(p, c),
        );
        kit.cornea[k] = push(mesh, &cornea, cornea_m);
    }
    kit
}

/// The shape keys of the parts (see the module notes) for a model built
/// with [`build`].
pub fn keys(mesh: &Mesh, kit: &Kit, frame: HeadFrame) -> Vec<ShapeKey> {
    let n = mesh.positions.len();
    let s = frame.size;
    let o = frame.origin;
    let mut out = Vec::new();
    let sides = [("Left", 1.0f32), ("Right", -1.0)];
    // Lids: turn the upper (and lower) lid and its lashes about the eye's centre.
    let lid_key = |name: &str, k: usize, up_deg: f32, low_deg: f32| -> ShapeKey {
        let sx = if k == 0 { 1.0 } else { -1.0 };
        let c = add(o, [0.12 * s * sx, 0.0, -0.305 * s]);
        let mut d = vec![[0.0f32; 3]; n];
        for (parts, deg) in [(&[kit.lids[k][0].clone(), kit.lids[k][2].clone()], up_deg), (&[kit.lids[k][1].clone(), kit.lids[k][3].clone()], low_deg)] {
            for r in parts {
                for v in r.clone() {
                    d[v] = sub(turn_x(mesh.positions[v], c, deg), mesh.positions[v]);
                }
            }
        }
        ShapeKey::from_dense(name, &d)
    };
    for (k, (side, _)) in sides.iter().enumerate() {
        // Closed: the lids meet; the upper does most of the travel.
        out.push(lid_key(&format!("eyeBlink{side}"), k, 34.0, -8.0));
        out.push(lid_key(&format!("eyeSquint{side}"), k, 8.0, -6.0));
        out.push(lid_key(&format!("eyeWide{side}"), k, -9.0, 3.0));
    }
    // Pupils: along the sphere, wider or narrower round the gaze.
    let pupil = mesh.materials.iter().position(|m| m.name == "Pupil").map(|i| i as u32);
    if let Some(pm) = pupil {
        let mats = mesh.tri_materials();
        let mut verts: Vec<usize> = Vec::new();
        for t in 0..mesh.triangles() {
            if mats[t] == pm {
                verts.extend(mesh.tri(t));
            }
        }
        verts.sort_unstable();
        verts.dedup();
        for (name, k) in [("pupilDilate", 1.7f32), ("pupilConstrict", 0.55)] {
            let mut d = vec![[0.0f32; 3]; n];
            for &v in &verts {
                let sx = if mesh.positions[v][0] >= o[0] { 1.0 } else { -1.0 };
                let c = add(o, [0.12 * s * sx, 0.0, -0.305 * s]);
                let rel = sub(mesh.positions[v], c);
                let r = crate::length(rel);
                if r < 1e-4 {
                    continue;
                }
                // Angle from the gaze (-z); scaled round it.
                let a = (-rel[2] / r).clamp(-1.0, 1.0).acos();
                let planar = [rel[0], rel[1]];
                let pl = (planar[0] * planar[0] + planar[1] * planar[1]).sqrt().max(1e-6);
                let a2 = a * k;
                let np = [c[0] + r * a2.sin() * planar[0] / pl, c[1] + r * a2.sin() * planar[1] / pl, c[2] - r * a2.cos()];
                d[v] = sub(np, mesh.positions[v]);
            }
            out.push(ShapeKey::from_dense(name, &d));
        }
    }
    // The tongue: its tip moves most.
    let tr = kit.tongue.clone();
    let (mut zmax, mut zmin) = (f32::NEG_INFINITY, f32::INFINITY);
    for v in tr.clone() {
        zmax = zmax.max(mesh.positions[v][2]);
        zmin = zmin.min(mesh.positions[v][2]);
    }
    let tip_w = |v: usize| smoothstep(0.1, 1.0, (zmax - mesh.positions[v][2]) / (zmax - zmin).max(1e-4));
    let tk = |name: &str, f: &dyn Fn(f32) -> V3| -> ShapeKey {
        let mut d = vec![[0.0f32; 3]; n];
        for v in tr.clone() {
            d[v] = scale(f(tip_w(v)), s);
        }
        ShapeKey::from_dense(name, &d)
    };
    out.push(tk("tongueOut", &|w| [0.0, -0.004 * w, -0.085 * w]));
    out.push(tk("tongueUp", &|w| [0.0, -0.045 * w, -0.01 * w]));
    out.push(tk("tongueDown", &|w| [0.0, 0.03 * w, -0.01 * w]));
    out.push(tk("tongueCurl", &|w| [0.0, -0.04 * w * w, 0.035 * w * w]));
    // TH: the tip forward between the teeth.
    out.push(tk("tongueTH", &|w| [0.0, -0.008 * w, -0.05 * w]));
    out
}

/// Every key name [`keys`] makes.
pub const NAMES: &[&str] = &["eyeBlinkLeft", "eyeBlinkRight", "eyeSquintLeft", "eyeSquintRight", "eyeWideLeft", "eyeWideRight", "pupilDilate", "pupilConstrict", "tongueOut", "tongueUp", "tongueDown", "tongueCurl", "tongueTH"];
