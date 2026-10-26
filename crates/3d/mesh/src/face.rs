// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Faces that act: shape keys for Studio's head base (and the characters
//! built on it), named as face capture and avatar tools name them, so
//! captured faces and lip-sync drive them as they are:
//!
//! - expressions (ARKit names): `jawOpen`, `eyeBlinkLeft`/`Right`,
//!   `mouthSmileLeft`/`Right`, `mouthFrownLeft`/`Right`,
//!   `mouthStretchLeft`/`Right`, `mouthPucker`, `mouthClose`,
//!   `browInnerUp`, `browDownLeft`/`Right`, `browOuterUpLeft`/`Right`,
//!   `cheekPuff`, `noseSneerLeft`/`Right`, `mouthRollLower`;
//! - visemes (the mouth shapes of speech, Oculus names): `viseme_PP`,
//!   `viseme_FF`, `viseme_TH`, `viseme_DD`, `viseme_kk`, `viseme_CH`,
//!   `viseme_SS`, `viseme_nn`, `viseme_RR`, `viseme_aa`, `viseme_E`,
//!   `viseme_I`, `viseme_O`, `viseme_U`.
//!
//! "Left" is the character's own left (+x, the right of the picture when
//! it faces the camera). Each shape is an offset field over the head base's
//! layout (where its lips, lids and brows are), so it suits that head and
//! sculpts of it; other heads get keys too but may need them sculpted.

use crate::rig::ShapeKey;
use crate::{Mesh, V3, add, length, scale, sub};

/// Where a head base sits in a model: its origin and its size (chin to
/// the top of the skull, px).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HeadFrame {
    pub origin: V3,
    pub size: f32,
}

/// Finds a head base in a model by its eyes (the pupils' material).
pub fn find_head(mesh: &Mesh) -> Option<HeadFrame> {
    let pupil = mesh.materials.iter().position(|m| m.name == "Pupil")? as u32;
    let mats = mesh.tri_materials();
    let mut left = ([0.0f32; 3], 0.0f32);
    let mut right = ([0.0f32; 3], 0.0f32);
    for t in 0..mesh.triangles() {
        if mats[t] != pupil {
            continue;
        }
        let c = mesh.centroid(t);
        let side = if c[0] >= 0.0 { &mut left } else { &mut right };
        side.0 = add(side.0, c);
        side.1 += 1.0;
    }
    if left.1 < 1.0 || right.1 < 1.0 {
        return None;
    }
    let (l, r) = (scale(left.0, 1.0 / left.1), scale(right.0, 1.0 / right.1));
    let size = length(sub(l, r)) / 0.24;
    let mid = scale(add(l, r), 0.5);
    // The pupils sit at the front of eyes 0.0525 across from their centres.
    Some(HeadFrame { origin: [mid[0], mid[1], mid[2] + 0.3575 * size], size })
}

fn gauss(p: V3, c: V3, r: f32) -> f32 {
    let d = sub(p, c);
    (-(d[0] * d[0] + d[1] * d[1] + d[2] * d[2]) / (r * r)).exp()
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Turns `p` about the x axis through `c` by `deg` (positive: points in
/// front and below go down and back).
fn turn_x(p: V3, c: V3, deg: f32) -> V3 {
    let (s, k) = deg.to_radians().sin_cos();
    let d = sub(p, c);
    [p[0], c[1] + d[1] * k - d[2] * s, c[2] + d[1] * s + d[2] * k]
}

/// How much of the jaw's turn a point of the head takes (0 the skull, 1 the
/// lower face), for the head's own units (`lip`: on the lips, which split at
/// the mouth line). The `jawOpen` key and the `jaw` bone use it alike.
pub fn jaw_weight(p: V3, lip: bool) -> f32 {
    let mouth_y = 0.287;
    let below = if lip { if p[1] > mouth_y { 1.0 } else { 0.0 } } else { smoothstep(mouth_y - 0.01, mouth_y + 0.03, p[1]) };
    let not_neck = 1.0 - smoothstep(0.43, 0.6, p[1] + (p[2] + 0.27).max(0.0) * 0.9);
    let front = 1.0 - smoothstep(0.02, 0.16, p[2]);
    below * not_neck * front
}

/// Bones of the face, for lip-sync and acting: `jaw` (turns about x, 20° is
/// `jawOpen` at 1), `tongue.0`…`tongue.2` (the tongue's root to its tip;
/// their z turns curl it, x lifts it), `eye.L`, `eye.R`.
pub const BONES: &[&str] = &["jaw", "tongue.0", "tongue.1", "tongue.2", "eye.L", "eye.R"];

/// The expression and viseme keys for the head at `frame`, over vertices
/// `verts` of `mesh` (others never move). Eyeballs (materials named Eye
/// white, Iris, Pupil) stay put; lids close over them.
pub fn head_shapes(mesh: &Mesh, verts: std::ops::Range<usize>, frame: HeadFrame) -> Vec<ShapeKey> {
    let s = frame.size.max(1e-3);
    let n = mesh.positions.len();
    // Each vertex's material (from any triangle using it).
    let mut vmat = vec![0u32; n];
    let mats = mesh.tri_materials();
    for t in 0..mesh.triangles() {
        for v in mesh.tri(t) {
            vmat[v] = mats[t];
        }
    }
    let eye_mats: Vec<u32> = mesh.materials.iter().enumerate().filter(|(_, m)| matches!(m.name.as_str(), "Eye white" | "Iris" | "Pupil")).map(|(i, _)| i as u32).collect();
    let lip_mat = mesh.materials.iter().position(|m| m.name == "Lips").map(|i| i as u32);
    // Head-local points in units of the head's size.
    let local: Vec<Option<V3>> = (0..n).map(|i| (verts.contains(&i) && !eye_mats.contains(&vmat[i])).then(|| scale(sub(mesh.positions[i], frame.origin), 1.0 / s))).collect();
    let field = |f: &dyn Fn(V3, bool) -> V3| -> Vec<V3> {
        local.iter().enumerate().map(|(i, p)| p.map_or([0.0; 3], |p| scale(f(p, Some(vmat[i]) == lip_mat), s))).collect()
    };
    let mouth_y = 0.287;
    let corner = |side: f32| [0.085 * side, mouth_y, -0.335];
    // Lower face weight for the jaw.
    let jaw_w = |p: V3, lip: bool| -> f32 {
        let below = if lip { if p[1] > mouth_y { 1.0 } else { 0.0 } } else { smoothstep(mouth_y - 0.01, mouth_y + 0.03, p[1]) };
        let not_neck = 1.0 - smoothstep(0.43, 0.6, p[1] + (p[2] + 0.27).max(0.0) * 0.9);
        let front = 1.0 - smoothstep(0.02, 0.16, p[2]);
        below * not_neck * front
    };
    let jaw = |p: V3, lip: bool| -> V3 { scale(sub(turn_x(p, [0.0, 0.12, 0.08], 20.0), p), jaw_w(p, lip)) };
    let smile = |side: f32| move |p: V3, _lip: bool| -> V3 {
        let c = corner(side);
        add(scale([0.024 * side, -0.03, 0.014], gauss(p, c, 0.055)), scale([0.004 * side, -0.014, -0.006], gauss(p, [0.15 * side, 0.12, -0.27], 0.07)))
    };
    let frown = |side: f32| move |p: V3, _lip: bool| -> V3 { scale([0.006 * side, 0.026, 0.004], gauss(p, corner(side), 0.05)) };
    let stretch = |side: f32| move |p: V3, _lip: bool| -> V3 { scale([0.026 * side, 0.004, 0.012], gauss(p, corner(side), 0.055)) };
    let pucker = |p: V3, _lip: bool| -> V3 {
        let w = gauss(p, [0.0, mouth_y, -0.35], 0.075);
        scale([-p[0] * 0.5, (mouth_y - p[1]) * 0.2, -0.035], w)
    };
    let close = |p: V3, lip: bool| -> V3 {
        let w = gauss(p, [0.0, mouth_y, -0.35], 0.07) * if lip { 1.0 } else { 0.4 };
        [0.0, if p[1] < mouth_y { 0.008 } else { -0.01 } * w, 0.004 * w]
    };
    let roll_lower = |p: V3, lip: bool| -> V3 {
        let w = if lip && p[1] > mouth_y { 1.0 } else { 0.0 } * gauss(p, [0.0, 0.31, -0.35], 0.08);
        [0.0, -0.018 * w, 0.022 * w]
    };
    let blink = |side: f32| move |p: V3, _lip: bool| -> V3 {
        let e = [0.12 * side, 0.0, -0.305];
        let d = sub(p, e);
        let near = 1.0 - smoothstep(0.075, 0.115, length(d));
        let in_front = smoothstep(-0.2, -0.33, p[2]);
        let across = 1.0 - smoothstep(0.07, 0.1, (p[0] - e[0]).abs());
        let w = near * in_front * across;
        if w <= 0.0 {
            return [0.0; 3];
        }
        if p[1] < 0.012 {
            // Upper lid: down over the eye, most at its edge.
            let k = 1.0 - smoothstep(-0.02, -0.1, p[1]);
            scale(sub(turn_x(p, e, 52.0), p), w * (0.25 + 0.75 * k))
        } else {
            let k = 1.0 - smoothstep(0.045, 0.1, p[1]);
            scale(sub(turn_x(p, e, -10.0), p), w * k)
        }
    };
    let brow_inner_up = |p: V3, _lip: bool| -> V3 { scale([0.0, -0.026, 0.0], gauss(p, [0.04, -0.09, -0.34], 0.05) + gauss(p, [-0.04, -0.09, -0.34], 0.05)) };
    let brow_down = |side: f32| move |p: V3, _lip: bool| -> V3 { scale([-0.006 * side, 0.02, -0.006], gauss(p, [0.09 * side, -0.085, -0.34], 0.055)) };
    let brow_outer_up = |side: f32| move |p: V3, _lip: bool| -> V3 { scale([0.0, -0.026, 0.0], gauss(p, [0.165 * side, -0.08, -0.31], 0.05)) };
    let puff = |p: V3, _lip: bool| -> V3 { scale([p[0].signum() * 0.028, 0.0, -0.01], gauss(p, [0.14 * p[0].signum(), 0.21, -0.25], 0.075)) };
    let sneer = |side: f32| move |p: V3, _lip: bool| -> V3 { scale([0.0, -0.012, -0.004], gauss(p, [0.042 * side, 0.17, -0.43], 0.04)) };
    let ff = |p: V3, lip: bool| -> V3 { add(scale(jaw(p, lip), 0.1), roll_lower(p, lip)) };
    let named: Vec<(&str, Vec<V3>)> = vec![
        ("jawOpen", field(&jaw)),
        ("eyeBlinkLeft", field(&blink(1.0))),
        ("eyeBlinkRight", field(&blink(-1.0))),
        ("mouthSmileLeft", field(&smile(1.0))),
        ("mouthSmileRight", field(&smile(-1.0))),
        ("mouthFrownLeft", field(&frown(1.0))),
        ("mouthFrownRight", field(&frown(-1.0))),
        ("mouthStretchLeft", field(&stretch(1.0))),
        ("mouthStretchRight", field(&stretch(-1.0))),
        ("mouthPucker", field(&pucker)),
        ("mouthClose", field(&close)),
        ("mouthRollLower", field(&roll_lower)),
        ("browInnerUp", field(&brow_inner_up)),
        ("browDownLeft", field(&brow_down(1.0))),
        ("browDownRight", field(&brow_down(-1.0))),
        ("browOuterUpLeft", field(&brow_outer_up(1.0))),
        ("browOuterUpRight", field(&brow_outer_up(-1.0))),
        ("cheekPuff", field(&puff)),
        ("noseSneerLeft", field(&sneer(1.0))),
        ("noseSneerRight", field(&sneer(-1.0))),
        ("viseme_FF", field(&ff)),
    ];
    let get = |name: &str| named.iter().find(|(k, _)| *k == name).map(|(_, v)| v.clone()).unwrap_or_default();
    let mix = |parts: &[(&str, f32)]| -> Vec<V3> {
        let mut out = vec![[0.0f32; 3]; n];
        for (name, w) in parts {
            for (o, d) in out.iter_mut().zip(get(name)) {
                *o = add(*o, scale(d, *w));
            }
        }
        out
    };
    let wide = |k: f32| [("mouthStretchLeft", k), ("mouthStretchRight", k)];
    let mut visemes: Vec<(&str, Vec<V3>)> = vec![
        ("viseme_PP", mix(&[("mouthClose", 1.0)])),
        ("viseme_TH", mix(&[("jawOpen", 0.2)])),
        ("viseme_DD", mix(&[&[("jawOpen", 0.25)][..], &wide(0.2)].concat())),
        ("viseme_kk", mix(&[&[("jawOpen", 0.3)][..], &wide(0.3)].concat())),
        ("viseme_CH", mix(&[("jawOpen", 0.2), ("mouthPucker", 0.5)])),
        ("viseme_SS", mix(&[&[("jawOpen", 0.1)][..], &wide(0.5)].concat())),
        ("viseme_nn", mix(&[("jawOpen", 0.2)])),
        ("viseme_RR", mix(&[("jawOpen", 0.2), ("mouthPucker", 0.4)])),
        ("viseme_aa", mix(&[&[("jawOpen", 0.75)][..], &wide(0.1)].concat())),
        ("viseme_E", mix(&[&[("jawOpen", 0.35)][..], &wide(0.6)].concat())),
        ("viseme_I", mix(&[&[("jawOpen", 0.25)][..], &wide(0.8)].concat())),
        ("viseme_O", mix(&[("jawOpen", 0.5), ("mouthPucker", 0.7)])),
        ("viseme_U", mix(&[("jawOpen", 0.25), ("mouthPucker", 1.0)])),
    ];
    let mut all = named;
    all.append(&mut visemes);
    all.into_iter().map(|(name, d)| ShapeKey::from_dense(name, &d)).collect()
}

/// Every key name [`head_shapes`] makes.
pub const NAMES: &[&str] = &[
    "jawOpen", "eyeBlinkLeft", "eyeBlinkRight", "mouthSmileLeft", "mouthSmileRight", "mouthFrownLeft", "mouthFrownRight", "mouthStretchLeft", "mouthStretchRight", "mouthPucker", "mouthClose", "mouthRollLower", "browInnerUp", "browDownLeft", "browDownRight",
    "browOuterUpLeft", "browOuterUpRight", "cheekPuff", "noseSneerLeft", "noseSneerRight", "viseme_FF", "viseme_PP", "viseme_TH", "viseme_DD", "viseme_kk", "viseme_CH", "viseme_SS", "viseme_nn", "viseme_RR", "viseme_aa", "viseme_E", "viseme_I", "viseme_O", "viseme_U",
];

/// Short names people and AIs reach for, to the key names.
pub fn alias(name: &str) -> &str {
    match name {
        "smile" => "mouthSmile",
        "blink" => "eyeBlink",
        "frown" => "mouthFrown",
        "jaw" | "open" | "mouth_open" => "jawOpen",
        "pucker" | "kiss" => "mouthPucker",
        "brows_up" | "surprise" => "browInnerUp",
        "aa" | "a" | "ah" => "viseme_aa",
        "ee" | "e" => "viseme_E",
        "ih" | "i" => "viseme_I",
        "oh" | "o" => "viseme_O",
        "oo" | "u" => "viseme_U",
        "mbp" | "m" | "b" | "p" => "viseme_PP",
        "fv" | "f" | "v" => "viseme_FF",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_head_base_gets_a_face_that_opens_blinks_and_smiles() {
        let m = crate::sdf::head(300.0, 2);
        let f = find_head(&m).unwrap();
        assert!((f.size - 300.0).abs() < 12.0 && length(f.origin) < 8.0, "{f:?}");
        let keys = head_shapes(&m, 0..m.positions.len(), f);
        assert_eq!(keys.len(), NAMES.len());
        let key = |n: &str| keys.iter().find(|k| k.name == n).unwrap();
        // The jaw drops the chin.
        let chin = (0..m.positions.len()).filter(|&i| m.positions[i][2] < -70.0).max_by(|&a, &b| m.positions[a][1].total_cmp(&m.positions[b][1])).unwrap();
        let j = key("jawOpen");
        let k = j.indices.iter().position(|&i| i as usize == chin).expect("the chin moves");
        assert!(j.deltas[k][1] > 10.0, "{:?}", j.deltas[k]);
        // Blinks are one-sided, smiles lift the corners.
        assert!(key("eyeBlinkLeft").indices.iter().all(|&i| m.positions[i as usize][0] > 0.0));
        let sm = key("mouthSmileLeft");
        let lift: f32 = sm.deltas.iter().map(|d| d[1]).sum::<f32>();
        assert!(lift < 0.0);
        // Eyeballs never move.
        let pupil = m.materials.iter().position(|x| x.name == "Pupil").unwrap() as u32;
        let mats = m.tri_materials();
        let pupil_verts: std::collections::HashSet<u32> = (0..m.triangles()).filter(|&t| mats[t] == pupil).flat_map(|t| m.tri(t).map(|v| v as u32)).collect();
        assert!(keys.iter().all(|k| k.indices.iter().all(|i| !pupil_verts.contains(i))));
    }
}
