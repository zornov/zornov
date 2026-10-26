// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Animal and creature features for a standing figure: ears, a muzzle and
//! nose, whiskers and a tail on the same body, rig and face. The result is
//! an animal that walks on two legs (a character), with every body part,
//! bone and shape key of any other figure, so it is dressed, posed and
//! lip-synced the same way.
//!
//! Species: `cat`, `dog`, `fox`, `bear`, `rabbit`, `mouse`. The skin
//! takes the fur colour; the ears, muzzle and tail are separate surfaces
//! on the head and hips, and the tail has bones (`tail.1`…`tail.3`).

use crate::face::HeadFrame;
use crate::loft;
use crate::rig::{Bone, Rig};
use crate::{Material, Mesh, V3, add, scale, srgb_to_linear};

/// Every species.
pub const NAMES: &str = "cat, dog, fox, bear, rabbit, mouse";

struct Spec {
    /// Ear height and width (head units), how far apart, tilt out, droop.
    ear: (f32, f32, f32, f32, f32),
    muzzle: (f32, f32),
    tail: (f32, f32),
    whiskers: bool,
    fur: [u8; 3],
    nose: [u8; 3],
}

fn spec(name: &str) -> Option<Spec> {
    Some(match name.trim().to_ascii_lowercase().as_str() {
        "cat" | "kitten" => Spec { ear: (0.36, 0.26, 0.2, 14.0, 0.0), muzzle: (0.1, 0.1), tail: (0.9, 0.035), whiskers: true, fur: [196, 150, 100], nose: [210, 120, 130] },
        "dog" | "puppy" | "wolf" => Spec { ear: (0.34, 0.2, 0.28, 10.0, 1.0), muzzle: (0.2, 0.14), tail: (0.55, 0.04), whiskers: false, fur: [170, 120, 80], nose: [30, 24, 24] },
        "fox" => Spec { ear: (0.46, 0.24, 0.2, 10.0, 0.0), muzzle: (0.22, 0.1), tail: (0.85, 0.075), whiskers: true, fur: [210, 110, 50], nose: [24, 20, 20] },
        "bear" => Spec { ear: (0.17, 0.2, 0.3, 20.0, 0.0), muzzle: (0.14, 0.18), tail: (0.12, 0.06), whiskers: false, fur: [110, 76, 50], nose: [24, 20, 20] },
        "rabbit" | "bunny" | "hare" => Spec { ear: (0.78, 0.17, 0.14, 8.0, 0.0), muzzle: (0.06, 0.09), tail: (0.14, 0.07), whiskers: true, fur: [226, 220, 212], nose: [230, 150, 160] },
        "mouse" | "rat" => Spec { ear: (0.26, 0.3, 0.3, 24.0, 0.0), muzzle: (0.15, 0.08), tail: (1.0, 0.012), whiskers: true, fur: [170, 160, 150], nose: [226, 150, 160] },
        _ => return None,
    })
}

/// Whether `name` is a species (or an alias of one).
pub fn known(name: &str) -> bool {
    spec(name).is_some()
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

/// Gives the figure the species' features. `fur` overrides the usual colour.
/// Run before the rig is skinned to the final surface: the new surfaces are
/// skinned here (ears, muzzle, whiskers on the head; the tail on its bones).
pub fn apply(mesh: &mut Mesh, rig: &mut Rig, species: &str, fur: Option<[u8; 3]>, frame: HeadFrame) -> Result<(), String> {
    let sp = spec(species).ok_or_else(|| format!("species \"{species}\": {NAMES}"))?;
    let fur = fur.unwrap_or(sp.fur);
    let (o, s) = (frame.origin, frame.size);
    let w = |p: V3| add(o, scale(p, s));
    let head = rig.bone("head").ok_or("no head bone")?;
    // Fur: the body and the head take it.
    if let Some(skin) = mesh.materials.first_mut() {
        skin.base_colour = lin(fur);
        skin.roughness = 0.9;
        skin.subsurface = 0.1;
    }
    let fur_m = {
        mesh.materials.push(Material { name: "Fur".into(), base_colour: lin(fur), roughness: 0.92, subsurface: 0.1, ..Material::default() });
        (mesh.materials.len() - 1) as u32
    };
    let inner_m = {
        mesh.materials.push(Material { name: "Inner ear".into(), base_colour: lin([226, 160, 160]), roughness: 0.7, subsurface: 0.3, ..Material::default() });
        (mesh.materials.len() - 1) as u32
    };
    let nose_m = {
        mesh.materials.push(Material { name: "Nose".into(), base_colour: lin(sp.nose), roughness: 0.35, ..Material::default() });
        (mesh.materials.len() - 1) as u32
    };
    let n0 = mesh.positions.len();
    // Ears: a leaf of fur with a paler inside, tilted out, drooping for floppy ones.
    let (eh, ew, spread, tilt, droop) = sp.ear;
    for sx in [1.0f32, -1.0] {
        let base = w([sx * spread, -0.42, 0.05]);
        let leaf = loft::surface(
            9,
            9,
            false,
            |u, v| {
                // Across (-1..1) and along (0 base, 1 tip), widest low down.
                let a = (u - 0.5) * 2.0;
                let width = ew * s * 0.5 * (1.0 - v).powf(0.7) * (0.3 + 0.7 * (v * 3.14).sin().max(0.2));
                let along = eh * s * v;
                let out = (tilt.to_radians().sin() * along + droop * 0.3 * s * v * v) * sx;
                let up = -(tilt.to_radians().cos() * along) * (1.0 - 0.8 * droop * v);
                [base[0] + out + a * width * sx * 0.0 + a * width, base[1] + up + droop * 0.28 * s * v * v, base[2] + a * 0.0 + 0.02 * s * (1.0 - a * a)]
            },
            |p| [0.0, 0.0, -1.0 + 0.0 * p[0]],
        );
        let r = push(mesh, &leaf, fur_m);
        let _ = r;
        let inner = {
            let mut m = leaf.clone();
            for p in &mut m.positions {
                p[2] -= 0.006 * s;
                p[0] = base[0] + (p[0] - base[0]) * 0.6;
            }
            m
        };
        push(mesh, &inner, inner_m);
    }
    // Muzzle: the bridge and the nose, on the face above the mouth.
    let (ml, mw) = sp.muzzle;
    let mut muzzle = crate::prim::uv_sphere(1.0, 20, 12);
    crate::ops::transform(&mut muzzle, &crate::region::Region::All, [0.0; 3], [0.0; 3], [mw * s, 0.1 * s + 0.04 * s * ml * 4.0, (0.1 + ml) * s], [0.0; 3]);
    for p in &mut muzzle.positions {
        *p = add(*p, w([0.0, 0.15, -0.42 - ml * 0.5]));
    }
    push(mesh, &muzzle, fur_m);
    let mut nose = crate::prim::uv_sphere(0.045 * s, 14, 9);
    for p in &mut nose.positions {
        *p = add(*p, w([0.0, 0.155, -0.48 - ml]));
    }
    push(mesh, &nose, nose_m);
    // Whiskers: three fine hairs each side.
    let whisk_m = {
        mesh.materials.push(Material { name: "Whiskers".into(), base_colour: [0.9, 0.9, 0.88, 1.0], roughness: 0.5, ..Material::default() });
        (mesh.materials.len() - 1) as u32
    };
    if sp.whiskers {
        for sx in [1.0f32, -1.0] {
            for k in 0..3 {
                let y = 0.2 + 0.03 * k as f32;
                let a = w([sx * 0.05, y, -0.46 - ml * 0.6]);
                let b = w([sx * 0.4, y - 0.04 + 0.04 * k as f32, -0.4 - ml * 0.3]);
                let t = loft::tube(&[a, crate::lerp(a, b, 0.5), b], &[0.0035 * s, 0.0028 * s, 0.001 * s], 4);
                push(mesh, &t, whisk_m);
            }
        }
    }
    // Everything above is on the head, rigid.
    let n1 = mesh.positions.len();
    rig.joints.resize(n1, [head as u16, 0, 0, 0]);
    rig.weights.resize(n1, [1.0, 0.0, 0.0, 0.0]);
    for v in n0..n1 {
        rig.joints[v] = [head as u16, 0, 0, 0];
        rig.weights[v] = [1.0, 0.0, 0.0, 0.0];
    }
    // The tail: a tapering tube from the base of the spine, on three bones.
    let hips = rig.bone("hips").ok_or("no hips bone")?;
    let hb = rig.bones[hips].head;
    let (len, rad) = sp.tail;
    let height = (mesh.bounds().1[1] - mesh.bounds().0[1]).max(1.0);
    let l = len * 0.3 * height;
    let r0 = rad * height * 0.6;
    let pts: Vec<V3> = (0..9)
        .map(|i| {
            let t = i as f32 / 8.0;
            [hb[0], hb[1] - 0.02 * height + 0.05 * height * t - 0.1 * l * t.powi(2), hb[2] + 0.06 * height + l * t]
        })
        .collect();
    let radii: Vec<f32> = (0..9).map(|i| r0 * (1.0 - 0.45 * i as f32 / 8.0)).collect();
    let tail = loft::tube(&pts, &radii, 10);
    let tr = push(mesh, &tail, fur_m);
    let b0 = rig.bones.len();
    let mut parent = hips;
    for k in 0..3 {
        let (a, b) = (pts[k * 3], pts[((k + 1) * 3).min(8)]);
        let mut bone = Bone::new(format!("tail.{}", k + 1), Some(parent), a, b);
        bone.deform = true;
        rig.bones.push(bone);
        parent = rig.bones.len() - 1;
    }
    rig.joints.resize(mesh.positions.len(), [0; 4]);
    rig.weights.resize(mesh.positions.len(), [0.0; 4]);
    for v in tr {
        // Along the tail, between the hips and the three tail bones.
        let t = ((mesh.positions[v][2] - pts[0][2]) / (pts[8][2] - pts[0][2]).max(1e-3)).clamp(0.0, 1.0);
        let seg = (t * 3.0).min(2.999);
        let k = seg as usize;
        let f = seg - k as f32;
        let (b_a, b_b) = (b0 + k, (b0 + k + 1).min(b0 + 2));
        rig.joints[v] = [b_a as u16, b_b as u16, 0, 0];
        rig.weights[v] = [1.0 - f, f, 0.0, 0.0];
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_species_adds_ears_a_muzzle_and_a_tail_on_bones() {
        for sp in NAMES.split(", ") {
            let mut m = crate::character::character(240.0, &crate::character::Build { detail: 1, dressed: false, species: Some(sp.into()), ..Default::default() });
            let rig = m.rig.clone().unwrap();
            assert!(rig.bone("tail.1").is_some() && rig.bone("tail.3").is_some(), "{sp}");
            let names: Vec<&str> = m.materials.iter().map(|x| x.name.as_str()).collect();
            assert!(names.contains(&"Fur") && names.contains(&"Nose") && names.contains(&"Inner ear"), "{sp}: {names:?}");
            assert!(m.rig_in_step(), "{sp}");
            // A fox can be dressed like anyone.
            let g = crate::wear::fit(&m, &crate::wear::Spec { kind: "shirt".into(), style: "tee".into(), detail: 0.3, ..Default::default() }).unwrap();
            crate::wear::attach(&mut m, &g[0]).unwrap();
        }
        assert!(crate::character::Build { species: Some("dragon".into()), ..Default::default() }.species.is_some());
    }
}
