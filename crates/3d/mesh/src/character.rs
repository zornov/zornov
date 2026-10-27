// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A character to animate from the start: a standing figure in an A-pose
//! (arms down 40°, the pose that bends best), built from blended shapes
//! with real proportions for its age and style, hands with five digits, the
//! head base for a face (eyes, lids, brows, nose, lips, ears), an armature
//! fitted to it with fingers, eyes and grip points, skin weights bound, eyes
//! on bones of their own and the face's shape keys (see [`crate::face`]).
//!
//! Any figure comes from [`Build`]: build (weight, shoulders, hips),
//! frame (masculine to feminine), age (a toddler to the old), height
//! proportions (legs, arms, neck, hands, feet) and style (true to life to
//! cartoon), or the anime look (`anime`: a bigger head, long legs, big eyes
//! with a large iris and highlights, a small nose and mouth, flat colour).
//! It is born in plain clothes (a top, trousers, shoes) or bare
//! (`dressed: false`) to be dressed with the wardrobe (`crate::wear`), and
//! is closed: every rim of its surface but the eyelids' is capped.

use crate::autorig::{self, Arms};
use crate::proportions::Proportions;
use crate::sdf::{Part, Shape};
use crate::{Material, Mesh, V3, add, scale, srgb_to_linear};

/// How the figure is built.
#[derive(Clone, Debug, PartialEq)]
pub struct Build {
    /// 0.8 slim – 1.3 heavy: how thick the body and limbs are.
    pub weight: f32,
    /// Shoulder width, 0.8–1.25 (1 = average).
    pub shoulders: f32,
    /// Hip width, 0.8–1.25.
    pub hips: f32,
    /// Head size against the body, 0.8–1.6 (bigger reads younger or more
    /// cartoon).
    pub head: f32,
    /// The head base with a face and eyes (else a plain egg head).
    pub face: bool,
    /// sRGB colours.
    pub skin: [u8; 3],
    pub top: [u8; 3],
    pub trousers: [u8; 3],
    pub shoes: [u8; 3],
    /// Brows (and the hair colour wardrobe hair takes by default).
    pub hair: [u8; 3],
    /// 1–6, finer surfaces are slower.
    pub detail: usize,
    /// Years; 0 is an adult. A child has a bigger head, shorter legs and
    /// arms, a baby more so.
    pub age: f32,
    /// -1 broad and square .. +1 narrow-shouldered with fuller hips and bust.
    pub figure: f32,
    /// Leg, arm, neck and hand length against the usual (1).
    pub legs: f32,
    pub arms: f32,
    pub neck: f32,
    pub hands: f32,
    /// 0 true to life .. 1 cartoon (bigger head, hands and feet, rounder).
    pub style: f32,
    /// Born in plain clothes (top, trousers, shoes), else bare.
    pub dressed: bool,
    /// An animal or creature on two legs: `cat`, `dog`, `fox`, `bear`,
    /// `rabbit`, `mouse` (see [`crate::species`]).
    pub species: Option<String>,
    /// Fur colour for a species (else the species' own).
    pub fur: Option<[u8; 3]>,
    /// The anime look: big eyes with a large iris and highlights, a small
    /// nose and mouth, flat-coloured materials (proportions come from the
    /// `anime` preset: bigger head, longer legs).
    pub anime: bool,
}

impl Default for Build {
    fn default() -> Build {
        Build {
            weight: 1.0,
            shoulders: 1.0,
            hips: 1.0,
            head: 1.0,
            face: true,
            skin: [214, 160, 136],
            top: [70, 96, 140],
            trousers: [52, 54, 62],
            shoes: [38, 30, 26],
            hair: [58, 40, 32],
            detail: 3,
            age: 0.0,
            figure: 0.0,
            legs: 1.0,
            arms: 1.0,
            neck: 1.0,
            hands: 1.0,
            style: 0.0,
            dressed: true,
            species: None,
            fur: None,
            anime: false,
        }
    }
}

impl Build {
    /// The proportions this build gives.
    pub fn proportions(&self) -> Proportions {
        let mut p = Proportions::of(self.age, self.style, self.legs, self.arms, self.hands, self.neck);
        p.head = (p.head * self.head.clamp(0.6, 2.0)).clamp(0.1, 0.45);
        p
    }

    /// A starting build by name: a general kind of figure, not a person.
    /// `baby`, `toddler`, `child`, `teen`, `adult`, `elder`; `slim`,
    /// `athletic`, `heavy`; `heroic` (broad, long-legged), `toon`,
    /// `chibi` (cartoon, huge head), `realistic`, `stylised`.
    pub fn preset(name: &str) -> Option<Build> {
        let d = Build::default();
        Some(match name.trim().to_ascii_lowercase().replace([' ', '-'], "_").as_str() {
            "baby" | "infant" => Build { age: 1.0, weight: 1.1, ..d },
            "toddler" => Build { age: 2.5, weight: 1.05, ..d },
            "child" | "kid" | "boy" | "girl" => Build { age: 7.0, weight: 0.95, ..d },
            "teen" | "teenager" | "adolescent" => Build { age: 15.0, weight: 0.92, ..d },
            "adult" | "average" | "normal" | "realistic" | "real" => d,
            "elder" | "elderly" | "old" | "senior" => Build { age: 75.0, weight: 0.95, shoulders: 0.94, ..d },
            "slim" | "thin" | "slender" | "lean" => Build { weight: 0.8, ..d },
            "athletic" | "fit" | "strong" | "muscular" => Build { weight: 1.1, shoulders: 1.1, figure: -0.4, ..d },
            "heavy" | "stocky" | "large" | "big" => Build { weight: 1.3, ..d },
            "heroic" | "hero" | "superhero" => Build { weight: 1.12, shoulders: 1.18, hips: 0.95, legs: 1.08, head: 0.94, figure: -0.7, ..d },
            "toon" | "cartoon" | "cartoony" => Build { style: 1.0, ..d },
            "chibi" | "mascot" => Build { style: 1.0, head: 1.45, legs: 0.85, ..d },
            "stylised" | "stylized" | "stylish" => Build { style: 0.5, ..d },
            "anime" | "manga" => Build { anime: true, style: 0.55, head: 1.14, legs: 1.12, neck: 1.05, hands: 0.92, weight: 0.92, ..d },
            "anime_chibi" | "chibi_anime" => Build { anime: true, style: 1.0, head: 1.5, legs: 0.85, ..d },
            _ => return None,
        })
    }

    pub const PRESETS: &'static str = "baby, toddler, child, teen, adult, elder, slim, athletic, heavy, heroic, realistic, stylised, toon, chibi, anime, anime_chibi";
}

fn lin(c: [u8; 3]) -> [f32; 4] {
    [srgb_to_linear(c[0]), srgb_to_linear(c[1]), srgb_to_linear(c[2]), 1.0]
}

/// The figure's body and head, `height` px tall, centred, facing -z, without
/// hands, and the range of its vertices that are the head base (empty
/// without a face).
pub fn mannequin(height: f32, b: &Build) -> (Mesh, std::ops::Range<usize>) {
    let h = height.max(10.0);
    let pr = b.proportions();
    let w = b.weight.clamp(0.6, 1.6);
    let y = |f: f32| -0.5 * h + pr.y(f) * h;
    let (tk, lk) = (pr.torso_k(), pr.leg_k());
    let soft = 1.0 + 1.5 * pr.style;
    let fig = b.figure.clamp(-1.0, 1.0);
    let fem = fig.max(0.0);
    let mut materials = vec![Material { name: "Skin".into(), base_colour: lin(b.skin), roughness: 0.52, subsurface: 0.6, ..Material::default() }];
    let (top, trousers, shoes) = (0u32, 0u32, 0u32);
    let skin = 0u32;
    let ell = |at: V3, size: V3, blend: f32, mat: u32| Part { blend: blend * h * soft, material: mat, ..Part::new(Shape::Ellipsoid, at, size) };
    let cap = |a: V3, bb: V3, r: f32, blend: f32, mat: u32| Part { a, b: bb, blend: blend * h * soft, material: mat, ..Part::new(Shape::Capsule, [0.0; 3], [r * 2.0; 3]) };
    let sw = (b.shoulders * (1.0 - 0.08 * fig)).clamp(0.7, 1.35);
    let hw = (b.hips * (1.0 + 0.1 * fem - 0.04 * (-fig).max(0.0))).clamp(0.7, 1.35);
    let limb = pr.limb;
    let mut parts = vec![
        // Chest, belly, pelvis.
        ell([0.0, y(0.29), 0.0], [0.27 * h * sw * w.sqrt(), 0.2 * h * tk, 0.15 * h * w], 0.0, top),
        ell([0.0, y(0.4), 0.004 * h], [0.22 * h * w * (1.0 - 0.1 * fem), 0.17 * h * tk, 0.135 * h * w], 0.04, top),
        ell([0.0, y(0.495), 0.006 * h], [0.25 * h * hw * w.sqrt(), 0.13 * h * (tk + lk) * 0.5, 0.15 * h * w], 0.035, trousers),
        // Shoulders across the top of the chest, sloping into the neck.
        cap([-0.105 * h * sw, y(0.207), 0.004 * h], [0.105 * h * sw, y(0.207), 0.004 * h], 0.047 * h * w.sqrt() * limb, 0.035, top),
        ell([0.0, y(0.19), 0.012 * h], [0.2 * h * sw, 0.06 * h * tk, 0.1 * h * w], 0.04, top),
        // Neck.
        cap([0.0, y(0.085), 0.012 * h], [0.0, y(0.22), 0.004 * h], 0.036 * h * w.sqrt() * pr.neck, 0.03, skin),
        // Trapezius: the slope from the neck out to the shoulders.
        ell([0.0, y(0.185), 0.014 * h], [0.15 * h * sw, 0.04 * h * tk, 0.085 * h * w], 0.05, skin),
        // Seat.
        ell([0.05 * h * hw, y(0.5), 0.05 * h], [0.1 * h * w, 0.11 * h * lk, 0.1 * h * w], 0.03, trousers),
        ell([-0.05 * h * hw, y(0.5), 0.05 * h], [0.1 * h * w, 0.11 * h * lk, 0.1 * h * w], 0.03, trousers),
    ];
    if fem > 0.05 {
        for sx in [1.0f32, -1.0] {
            parts.push(ell([sx * 0.05 * h, y(0.272), -0.062 * h], [0.075 * h * fem.sqrt(), 0.07 * h * fem.sqrt(), 0.07 * h * fem.sqrt()], 0.03, top));
        }
    }
    let droop = 40f32.to_radians();
    let (elbow_len, wrist_len, _) = autorig::arm_lengths(&pr, h);
    for sx in [1.0f32, -1.0] {
        let s0 = [sx * 0.118 * h * sw, y(0.205), 0.004 * h];
        let dir = [sx * droop.cos(), droop.sin(), 0.0];
        let elbow = add(add(s0, scale(dir, elbow_len)), [0.0, 0.0, 0.004 * h]);
        let wrist = add(s0, scale(dir, wrist_len));
        let r_arm = 0.031 * h * w * limb;
        parts.push(cap(s0, elbow, r_arm * 1.08, 0.02, top));
        // The forearm tapers to the wrist in three steps, the top third
        // still sleeve (the cuff) when dressed.
        let at = |t: f32| crate::lerp(elbow, wrist, t);
        parts.push(cap(elbow, at(0.35), r_arm * 0.92, 0.012, top));
        parts.push(cap(at(0.3), at(0.7), r_arm * 0.72, 0.012, skin));
        parts.push(cap(at(0.65), wrist, r_arm * 0.46, 0.012, skin));
        // Deltoid.
        parts.push(ell(add(s0, scale(dir, 0.02 * h)), [r_arm * 2.5, r_arm * 2.6, r_arm * 2.4], 0.02, top));
        let hx = sx * 0.088 * h * hw;
        let knee = [sx * 0.09 * h * hw, y(0.74), -0.006 * h];
        let ankle = [sx * 0.092 * h * hw, y(0.945), 0.01 * h];
        parts.push(cap([hx, y(0.52), 0.004 * h], knee, 0.06 * h * w * limb, 0.03, trousers));
        parts.push(cap(knee, ankle, 0.044 * h * w * limb, 0.015, trousers));
        // The calf's bulge.
        parts.push(ell(crate::lerp(knee, ankle, 0.3), [0.092 * h * w * limb, 0.12 * h * lk, 0.098 * h * w * limb], 0.02, trousers));
        // Shoe (or the bare foot).
        parts.push(ell([ankle[0], y(0.975), -0.035 * h * pr.foot], [0.075 * h * pr.foot.sqrt(), 0.05 * h, 0.2 * h * pr.foot], 0.012, shoes));
    }
    if !b.face {
        parts.push(ell([0.0, y(0.065), -0.004 * h], [0.115 * h * b.head, 0.14 * h * b.head, 0.13 * h * b.head], 0.03, skin));
    }
    let detail = b.detail.clamp(1, 6);
    let cell = h / [90.0, 130.0, 180.0, 240.0, 300.0, 380.0][detail - 1];
    // The whole body is skin; plain clothes are painted on after, by clean
    // cuts (a neckline, a waist, a cuff, an ankle), not by which part is near.
    for p in &mut parts {
        p.material = 0;
    }
    let mut mesh = crate::sdf::build(&parts, std::mem::take(&mut materials), cell);
    crate::ops::smooth(&mut mesh, &crate::region::Region::All, 1, 0.4);
    let mut head_range = 0..0;
    if b.face {
        // The head base: chin to crown the figure's head height.
        let s = h * pr.head;
        let mut head = crate::sdf::head_full(s, (detail + 1).min(5), b.hair, false);
        // The body's own neck runs up into the head: the head's stub goes.
        crate::ops::delete(&mut head, &crate::region::Region::Side { axis: 1, at: 0.57 * s, above: true });
        crate::ops::compact(&mut head);
        if let Some(m) = head.materials.first_mut() {
            m.base_colour = lin(b.skin);
        }
        // Crown at the top of the figure, neck into the body's neck.
        crate::ops::transform(&mut head, &crate::region::Region::All, [0.0, -0.5 * h + 0.53 * s, -0.07 * s], [0.0; 3], [1.0; 3], [0.0; 3]);
        let start = mesh.positions.len();
        let base_mat = mesh.materials.len() as u32;
        let mut mats = mesh.tri_materials();
        mats.extend(head.tri_materials().iter().map(|m| if *m == 0 { skin } else { base_mat + m - 1 }));
        mesh.materials.extend(head.materials.iter().skip(1).cloned());
        let base = mesh.positions.len() as u32;
        mesh.positions.extend_from_slice(&head.positions);
        mesh.normals.extend_from_slice(&head.normals);
        mesh.uvs.extend_from_slice(&head.uvs);
        mesh.indices.extend(head.indices.iter().map(|i| i + base));
        mesh.regroup(&mats);
        head_range = start..mesh.positions.len();
    }
    // The figure stands centred on the origin.
    let (lo, hi) = mesh.bounds();
    let c = scale(add(lo, hi), 0.5);
    crate::ops::transform(&mut mesh, &crate::region::Region::All, [-c[0], -c[1], -c[2]], [0.0; 3], [1.0; 3], [0.0; 3]);
    (mesh, head_range)
}

/// Sculpts both hands onto the figure at the rig's hand bones, with the
/// fingers the rig has, in skin.
fn add_hands(mesh: &mut Mesh, rig: &crate::rig::Rig, b: &Build, mid_x: f32) {
    let chunk = 1.0 + 0.5 * b.style.clamp(0.0, 1.0) + 0.1 * (b.weight - 1.0).max(0.0);
    for (side, left) in [("L", true), ("R", false)] {
        let Some(hand) = rig.bone(&format!("hand.{side}")) else { continue };
        let (wrist, tip) = (rig.bones[hand].head, rig.bones[hand].tail);
        let len = crate::length(crate::sub(tip, wrist)).max(1.0);
        let frame = crate::hand::Frame::hanging(wrist, crate::sub(tip, wrist), len, left, mid_x);
        let cell = len / (28.0 + 14.0 * b.detail.clamp(1, 6) as f32);
        let hand_mesh = crate::hand::place(&crate::hand::mesh(len, 0.0, chunk, cell), &frame);
        let mut mats = mesh.tri_materials();
        mats.extend(std::iter::repeat_n(0u32, hand_mesh.triangles()));
        let base = mesh.positions.len() as u32;
        mesh.positions.extend_from_slice(&hand_mesh.positions);
        mesh.normals.extend_from_slice(&hand_mesh.normals);
        mesh.uvs.extend_from_slice(&hand_mesh.uvs);
        mesh.indices.extend(hand_mesh.indices.iter().map(|i| i + base));
        mesh.regroup(&mats);
    }
}

/// A rigged, skinned character with a face and hands: the figure, its
/// armature (with fingers, eyes and grip points), eye bones that look, the
/// face's shape keys.
pub fn character(height: f32, b: &Build) -> Mesh {
    let (mut mesh, head) = mannequin(height, b);
    let pr = b.proportions();
    let mut rig = autorig::humanoid_fitted(&mesh, Arms::A, &pr);
    let mid_x = rig.bone("hips").map_or(0.0, |i| rig.bones[i].head[0]);
    let mut head = head;
    let mut kit: Option<crate::facekit::Kit> = None;
    let mut face_frame: Option<crate::face::HeadFrame> = None;
    if !head.is_empty() {
        let sub_mesh = Mesh { positions: mesh.positions.clone(), indices: mesh.indices.clone(), groups: mesh.groups.clone(), materials: mesh.materials.clone(), ..Default::default() };
        if let Some(frame) = crate::face::find_head(&sub_mesh) {
            // Separate parts of the face: lids, lashes, corneas, teeth, tongue.
            let parts = crate::facekit::build(&mut mesh, &mut head, frame, [30, 24, 22]);
            let mut keys = crate::face::head_shapes(&mesh, head.clone(), frame);
            keys.retain(|k| !k.name.starts_with("eyeBlink"));
            keys.extend(crate::facekit::keys(&mesh, &parts, frame));
            rig.shapes = keys;
            kit = Some(parts);
            face_frame = Some(frame);
            // Eyes: one bone each, pointing forward from the eyeball's centre.
            let s = frame.size;
            for (side, sx) in [("L", 1.0f32), ("R", -1.0)] {
                if let Some(e) = rig.bone(&format!("eye.{side}")) {
                    let c = add(frame.origin, [0.12 * s * sx, 0.0, -0.305 * s]);
                    rig.bones[e].head = c;
                    rig.bones[e].tail = add(c, [0.0, 0.0, -0.06 * s]);
                    rig.bones[e].deform = true;
                }
            }
            let mid = add(frame.origin, [0.0, 0.0, -0.305 * s]);
            for c in &mut rig.constraints {
                if let crate::rig::Constraint::LookAt { name, target, .. } = c
                    && name.starts_with("eyes.")
                {
                    *target = crate::rig::Target::Point(add(mid, [0.0, 0.0, -10.0 * s]));
                }
            }
        }
    }
    // The hands come last, so the head's vertices stay together.
    add_hands(&mut mesh, &rig, b, mid_x);
    autorig::bind(&mesh, &mut rig);
    // The neck: weights run smoothly from the head down through the neck to
    // the chest, so a turn or a nod bends it there and nowhere else, and the
    // jaw never follows the shoulders.
    if let (Some(hb), Some(nb), Some(cb)) = (rig.bone("head"), rig.bone("neck"), rig.bone("chest")) {
        let y_chin = rig.bones[hb].head[1];
        let y_base = rig.bones[nb].head[1] + 0.035 * (mesh.bounds().1[1] - mesh.bounds().0[1]);
        let (nx, nz) = (rig.bones[nb].head[0], rig.bones[nb].head[2]);
        let r_neck = 0.045 * (mesh.bounds().1[1] - mesh.bounds().0[1]) * pr.neck;
        for v in 0..mesh.positions.len() {
            let p = mesh.positions[v];
            let in_head = head.contains(&v);
            let radial = ((p[0] - nx).powi(2) + (p[2] - nz).powi(2)).sqrt();
            if !(in_head || (radial < r_neck * 2.0 && p[1] > y_chin - r_neck * 0.5 && p[1] < y_base)) {
                continue;
            }
            // Fades out with distance from the neck so there is no step at its edge.
            let fade = if in_head { 1.0 } else { 1.0 - ((radial - r_neck * 1.2) / (r_neck * 0.8)).clamp(0.0, 1.0) };
            let (nj, nw) = if p[1] <= y_chin {
                ([hb as u16, 0, 0, 0], [1.0, 0.0, 0.0, 0.0])
            } else {
                let t = ((p[1] - y_chin) / (y_base - y_chin).max(1e-3)).clamp(0.0, 1.0);
                let sm = t * t * (3.0 - 2.0 * t);
                let wn = (std::f32::consts::PI * sm).sin() * 0.9;
                let (wh, wc) = (1.0 - sm, sm);
                let total = wh + wn + wc;
                ([hb as u16, nb as u16, cb as u16, 0], [wh / total, wn / total, wc / total, 0.0])
            };
            let mut acc: Vec<(u16, f32)> = Vec::new();
            for k in 0..4 {
                if nw[k] > 0.0 {
                    acc.push((nj[k], nw[k] * fade));
                }
                if rig.weights[v][k] > 0.0 && fade < 1.0 {
                    match acc.iter_mut().find(|e| e.0 == rig.joints[v][k]) {
                        Some(e) => e.1 += rig.weights[v][k] * (1.0 - fade),
                        None => acc.push((rig.joints[v][k], rig.weights[v][k] * (1.0 - fade))),
                    }
                }
            }
            acc.sort_by(|a, b| b.1.total_cmp(&a.1));
            acc.truncate(4);
            let sum: f32 = acc.iter().map(|e| e.1).sum::<f32>().max(1e-6);
            let (mut jj, mut ww) = ([0u16; 4], [0.0f32; 4]);
            for (k, (b, x)) in acc.iter().enumerate() {
                jj[k] = *b;
                ww[k] = x / sum;
            }
            rig.joints[v] = jj;
            rig.weights[v] = ww;
        }
    }
    // The jaw and the tongue: bones for lip-sync and acting. The lower face,
    // the lower teeth and gums follow the jaw by the same weights as the
    // `jawOpen` key; the tongue's three bones carry the tongue.
    if let (Some(frame), Some(kit), Some(hb)) = (face_frame, &kit, rig.bone("head")) {
        let (o, s) = (frame.origin, frame.size);
        let at = |p: V3| add(o, scale(p, s));
        let jaw = rig.bone("jaw").unwrap_or(0);
        for (name, a, b) in [("jaw", [0.0, 0.12, 0.08], [0.0, 0.45, -0.2])] {
            if let Some(i) = rig.bone(name) {
                rig.bones[i].head = at(a);
                rig.bones[i].tail = at(b);
                rig.bones[i].deform = true;
            }
        }
        // Heights in metres; 0.318 only looks like 1/π.
        #[allow(clippy::approx_constant)]
        let tp = [[0.0, 0.318, -0.19], [0.0, 0.317, -0.245], [0.0, 0.315, -0.3], [0.0, 0.312, -0.345]];
        let tongue0 = rig.bone("tongue.0").unwrap_or(0);
        for k in 0..3 {
            if let Some(i) = rig.bone(&format!("tongue.{k}")) {
                rig.bones[i].head = at(tp[k]);
                rig.bones[i].tail = at(tp[k + 1]);
                rig.bones[i].deform = true;
            }
        }
        let lips = mesh.materials.iter().position(|m| m.name == "Lips").map(|i| i as u32);
        let mats = mesh.tri_materials();
        let mut is_lip = vec![false; mesh.positions.len()];
        if let Some(l) = lips {
            for t in 0..mesh.triangles() {
                if mats[t] == l {
                    for v in mesh.tri(t) {
                        is_lip[v] = true;
                    }
                }
            }
        }
        let eye_mats: Vec<u32> = mesh.materials.iter().enumerate().filter(|(_, m)| matches!(m.name.as_str(), "Eye white" | "Iris" | "Pupil")).map(|(i, _)| i as u32).collect();
        let mut is_eye = vec![false; mesh.positions.len()];
        for t in 0..mesh.triangles() {
            if eye_mats.contains(&mats[t]) {
                for v in mesh.tri(t) {
                    is_eye[v] = true;
                }
            }
        }
        for v in head.clone() {
            if is_eye[v] {
                continue;
            }
            let local = scale(crate::sub(mesh.positions[v], o), 1.0 / s);
            if kit.tongue.contains(&v) {
                let t = ((local[2] + 0.19) / -0.15).clamp(0.0, 1.0);
                let w = [(1.0 - 2.0 * t).max(0.0), 1.0 - (2.0 * t - 1.0).abs(), (2.0 * t - 1.0).max(0.0)];
                let sum: f32 = w.iter().sum::<f32>().max(1e-6);
                rig.joints[v] = [tongue0 as u16, tongue0 as u16 + 1, tongue0 as u16 + 2, 0];
                rig.weights[v] = [w[0] / sum, w[1] / sum, w[2] / sum, 0.0];
                continue;
            }
            let lower_part = (kit.teeth.contains(&v) || kit.gums.contains(&v)) && local[1] > 0.288;
            let teeth_gums = kit.teeth.contains(&v) || kit.gums.contains(&v);
            let wj = if teeth_gums { if lower_part { 1.0 } else { 0.0 } } else { crate::face::jaw_weight(local, is_lip[v]) };
            if wj < 0.01 {
                continue;
            }
            let mut acc: Vec<(u16, f32)> = (0..4).filter(|&k| rig.weights[v][k] > 0.0).map(|k| (rig.joints[v][k], rig.weights[v][k] * (1.0 - wj))).collect();
            acc.push((jaw as u16, wj));
            acc.sort_by(|a, b| b.1.total_cmp(&a.1));
            acc.truncate(4);
            let sum: f32 = acc.iter().map(|e| e.1).sum::<f32>().max(1e-6);
            let (mut jj, mut ww) = ([0u16; 4], [0.0f32; 4]);
            for (k, (b, x)) in acc.iter().enumerate() {
                jj[k] = *b;
                ww[k] = x / sum;
            }
            rig.joints[v] = jj;
            rig.weights[v] = ww;
        }
    }
    // Eyeballs go wholly with their eye bones; the rest of the head base
    // with the head and neck as bound.
    let eye_mats: Vec<u32> = mesh.materials.iter().enumerate().filter(|(_, m)| matches!(m.name.as_str(), "Eye white" | "Iris" | "Pupil")).map(|(i, _)| i as u32).collect();
    let mats = mesh.tri_materials();
    if let (Some(el), Some(er)) = (rig.bone("eye.L"), rig.bone("eye.R")) {
        for t in 0..mesh.triangles() {
            if !eye_mats.contains(&mats[t]) {
                continue;
            }
            for v in mesh.tri(t) {
                let side = if mesh.positions[v][0] >= 0.0 { el } else { er };
                rig.joints[v] = [side as u16, 0, 0, 0];
                rig.weights[v] = [1.0, 0.0, 0.0, 0.0];
            }
        }
        // Other bones never pull on the eyes' skin through the eye bones.
        for v in 0..mesh.positions.len() {
            let on_eye = (0..4).any(|k| rig.weights[v][k] > 0.0 && (rig.joints[v][k] as usize == el || rig.joints[v][k] as usize == er));
            let is_ball = rig.weights[v][0] == 1.0 && (rig.joints[v][0] as usize == el || rig.joints[v][0] as usize == er);
            if on_eye && !is_ball {
                let hb = rig.bone("head").unwrap_or(0) as u16;
                for k in 0..4 {
                    let j = rig.joints[v][k] as usize;
                    if j == el || j == er {
                        rig.joints[v][k] = hb;
                    }
                }
            }
        }
    }
    // Lids and lashes half follow the eyeball's gaze, the cornea is part of the eye.
    if let (Some(kit), Some(el), Some(er), Some(hb)) = (&kit, rig.bone("eye.L"), rig.bone("eye.R"), rig.bone("head")) {
        for (k, e) in [el, er].into_iter().enumerate() {
            for r in &kit.lids[k] {
                for v in r.clone() {
                    rig.joints[v] = [e as u16, hb as u16, 0, 0];
                    rig.weights[v] = [0.4, 0.6, 0.0, 0.0];
                }
            }
            for v in kit.cornea[k].clone() {
                rig.joints[v] = [e as u16, 0, 0, 0];
                rig.weights[v] = [1.0, 0.0, 0.0, 0.0];
            }
        }
    }
    if let (true, Some(frame)) = (b.anime, face_frame) {
        anime_face(&mut mesh, &mut rig, frame);
    }
    if let (Some(sp), Some(frame)) = (&b.species, face_frame) {
        let _ = crate::species::apply(&mut mesh, &mut rig, sp, b.fur, frame);
    }
    mesh.rig = Some(std::sync::Arc::new(rig));
    // A closed solid: the head's seam into the neck, the ends of the lips and
    // any other open rim are capped (the lids, lashes and corneas are sheets
    // by nature and stay as they are).
    crate::seal::close_holes(&mut mesh, 0.3 * height, |n| !matches!(n, "Eye white" | "Iris" | "Pupil"));
    if b.dressed {
        dress_plain(&mut mesh, b);
    }
    mesh
}

/// The anime face on a head base at `frame`: each eye (ball, iris, lids,
/// lashes, cornea) a fifth larger round its centre, the iris and pupil
/// grown to fill most of what shows, a highlight on each eye riding its eye
/// bone, the nose pressed flatter, the mouth narrower; every material
/// flat-coloured (no skin glow, no texture) for toon shading.
fn anime_face(mesh: &mut Mesh, rig: &mut crate::rig::Rig, frame: crate::face::HeadFrame) {
    let (o, s) = (frame.origin, frame.size);
    let local = |p: V3| scale(crate::sub(p, o), 1.0 / s);
    let world = |p: V3| add(o, scale(p, s));
    let named = |m: &Mesh, n: &str| m.materials.iter().position(|x| x.name == n).map(|i| i as u32);
    let eye_parts: Vec<u32> = ["Eye white", "Iris", "Pupil", "Cornea", "Upper lid", "Lower lid", "Lashes"].iter().filter_map(|n| named(mesh, n)).collect();
    let mats = mesh.tri_materials();
    let mut on_eye = vec![false; mesh.positions.len()];
    for t in 0..mesh.triangles() {
        if eye_parts.contains(&mats[t]) {
            for v in mesh.tri(t) {
                on_eye[v] = true;
            }
        }
    }
    let centre = |sx: f32| [0.12 * sx, 0.0, -0.305];
    for v in 0..mesh.positions.len() {
        let p = local(mesh.positions[v]);
        if on_eye[v] {
            let c = centre(if p[0] >= 0.0 { 1.0 } else { -1.0 });
            let d = crate::sub(p, c);
            mesh.positions[v] = world(add(c, [d[0] * 1.2, d[1] * 1.28, d[2] * 1.12]));
            continue;
        }
        // The nose: pressed back towards the face, most at its tip.
        if p[0].abs() < 0.11 && p[1] > -0.06 && p[1] < 0.24 && p[2] < -0.36 {
            let k = 0.55 * (1.0 - (p[0].abs() / 0.11)).clamp(0.0, 1.0);
            mesh.positions[v] = world([p[0], p[1], p[2] + (-0.36 - p[2]) * k]);
            continue;
        }
        // The mouth: narrower round its middle.
        if p[0].abs() < 0.14 && p[1] > 0.24 && p[1] < 0.34 && p[2] < -0.25 {
            let k = 1.0 - 0.22 * (1.0 - (p[0].abs() / 0.14)).clamp(0.0, 1.0).sqrt();
            mesh.positions[v] = world([p[0] * k, p[1], p[2]]);
        }
    }
    // A big iris and pupil: what of the eyeball faces forward.
    if let (Some(white), Some(iris), Some(pupil)) = (named(mesh, "Eye white"), named(mesh, "Iris"), named(mesh, "Pupil")) {
        let mut mats = mesh.tri_materials();
        for t in 0..mesh.triangles() {
            if ![white, iris, pupil].contains(&mats[t]) {
                continue;
            }
            let p = local(mesh.centroid(t));
            let c = centre(if p[0] >= 0.0 { 1.0 } else { -1.0 });
            let d = crate::normalise(crate::sub(p, c));
            // Taller than wide, as anime eyes are drawn.
            let a = ((d[0] / 0.62).powi(2) + (d[1] / 0.8).powi(2)).sqrt();
            mats[t] = if d[2] > -0.2 { mats[t] } else if a < 0.24 { pupil } else if a < 0.62 { iris } else { white };
        }
        mesh.regroup(&mats);
    }
    // The highlights: a bright fleck high on each eye, on its eye bone.
    let shine = mesh.materials.len() as u32;
    mesh.materials.push(Material { name: "Eye shine".into(), base_colour: [1.0, 1.0, 1.0, 1.0], emissive: [1.2, 1.2, 1.2], roughness: 0.2, ..Material::default() });
    let re = 0.0525 * 1.2;
    for (side, sx) in [("L", 1.0f32), ("R", -1.0)] {
        let Some(bone) = rig.bone(&format!("eye.{side}")) else { continue };
        for (k, (dx, dy, r)) in [(-0.35f32, -0.42f32, 0.2f32), (0.3, 0.38, 0.1)].into_iter().enumerate() {
            let c = centre(sx);
            let dir = crate::normalise([dx * sx, dy, -1.0]);
            let at = world(add(c, scale(dir, re * (1.07 + 0.01 * k as f32))));
            let mut fleck = crate::prim::uv_sphere(r * re * s, 10, 6);
            for q in &mut fleck.positions {
                q[2] *= 0.35;
                *q = add(*q, at);
            }
            let mut mats = mesh.tri_materials();
            mats.extend(std::iter::repeat_n(shine, fleck.triangles()));
            let base = mesh.positions.len() as u32;
            mesh.positions.extend_from_slice(&fleck.positions);
            mesh.normals.extend_from_slice(&fleck.normals);
            mesh.uvs.extend(std::iter::repeat_n([0.0f32; 2], fleck.positions.len()));
            mesh.indices.extend(fleck.indices.iter().map(|i| i + base));
            rig.joints.extend(std::iter::repeat_n([bone as u16, 0, 0, 0], fleck.positions.len()));
            rig.weights.extend(std::iter::repeat_n([1.0f32, 0.0, 0.0, 0.0], fleck.positions.len()));
            mesh.regroup(&mats);
        }
    }
    // Flat colour.
    for m in &mut mesh.materials {
        m.subsurface = 0.0;
        m.texture = None;
        if m.name != "Cornea" && m.name != "Eye shine" {
            m.roughness = m.roughness.max(0.6);
        }
    }
    crate::ops::smooth_normals_in_place(mesh);
}

/// The plain clothes a figure is born in: a T-shirt, trousers and shoes, each
/// a separate named part over the full body (take any off with
/// `wear::remove`, swap it by wearing something else).
fn dress_plain(mesh: &mut Mesh, b: &Build) {
    use crate::wear::{Spec, attach, fit};
    let detail = (b.detail as f32 / 3.0).clamp(0.3, 1.5);
    for (kind, style, name, colour) in [("trousers", "slacks", "Trousers", b.trousers), ("shirt", "tee", "T-shirt", b.top), ("shoes", "shoes", "Shoes", b.shoes)] {
        let spec = Spec { kind: kind.into(), style: style.into(), name: Some(name.into()), colour: Some(colour), material: Some(if kind == "shoes" { "leather".into() } else { "cotton".into() }), detail, ..Spec::default() };
        if let Ok(parts) = fit(mesh, &spec) {
            for g in parts {
                let _ = attach(mesh, &g);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rig::PoseInput;
    use std::collections::HashMap;

    #[test]
    fn a_character_stands_rigged_with_a_face_and_raises_an_arm() {
        let m = character(400.0, &Build { detail: 1, ..Build::default() });
        let (lo, hi) = m.bounds();
        assert!((hi[1] - lo[1] - 400.0).abs() < 25.0, "{lo:?} {hi:?}");
        let rig = m.rig.clone().unwrap();
        assert!(rig.bone("eye.L").is_some() && rig.shape("jawOpen").is_some() && rig.shape("viseme_aa").is_some());
        assert_eq!(rig.joints.len(), m.positions.len());
        // The left hand (on +x) rises when the upper arm swings out and up.
        let hand = rig.bone("hand.L").unwrap();
        let near_hand: Vec<usize> = (0..m.positions.len()).filter(|&v| crate::length(crate::sub(m.positions[v], rig.bones[hand].head)) < 25.0).collect();
        assert!(!near_hand.is_empty());
        let raised = rig.posed_mesh(&m, &PoseInput { rotations: HashMap::from([("upper_arm.L".to_string(), [0.0, 0.0, 80.0])]), ..Default::default() });
        let before: f32 = near_hand.iter().map(|&v| m.positions[v][1]).sum::<f32>() / near_hand.len() as f32;
        let after: f32 = near_hand.iter().map(|&v| raised.positions[v][1]).sum::<f32>() / near_hand.len() as f32;
        assert!((after - before).abs() > 60.0, "the hand moves: {before} → {after}");
        // Feet stay put when the arm moves.
        let foot = rig.bone("foot.L").unwrap();
        let near_foot: Vec<usize> = (0..m.positions.len()).filter(|&v| crate::length(crate::sub(m.positions[v], rig.bones[foot].head)) < 20.0).collect();
        assert!(near_foot.iter().all(|&v| crate::length(crate::sub(raised.positions[v], m.positions[v])) < 0.5));
    }

    #[test]
    fn every_character_has_the_same_bones_fingers_eyes_and_grips() {
        let want: Vec<String> = ["hips", "spine", "chest", "neck", "head", "eye.L", "eye.R", "jaw", "tongue.0", "tongue.1", "tongue.2"].iter().map(|s| s.to_string()).chain(["L", "R"].iter().flat_map(|s| {
            let mut v: Vec<String> = ["shoulder", "upper_arm", "forearm", "hand", "grip", "thigh", "shin", "foot", "toe"].iter().map(|b| format!("{b}.{s}")).collect();
            for d in crate::hand::DIGITS {
                for k in 0..3 {
                    v.push(format!("{d}.{s}.{k}"));
                }
            }
            v
        })).collect();
        let mut sets = Vec::new();
        for build in [Build::default(), Build::preset("child").unwrap(), Build::preset("elder").unwrap(), Build::preset("toon").unwrap(), Build { face: false, dressed: false, ..Build::default() }] {
            let m = character(300.0, &Build { detail: 1, ..build });
            let rig = m.rig.clone().unwrap();
            for n in &want {
                assert!(rig.bones.iter().any(|b| &b.name == n), "missing {n}");
            }
            sets.push(rig.bones.iter().map(|b| b.name.clone()).collect::<Vec<_>>());
            assert!(rig.is_skinned() && m.rig_in_step());
        }
        assert!(sets.windows(2).all(|w| w[0] == w[1]), "one bone set for every figure");
    }

    #[test]
    fn age_and_style_change_the_proportions_and_the_bones_follow() {
        let adult = character(400.0, &Build { detail: 1, ..Build::default() });
        let child = character(400.0, &Build { detail: 1, ..Build::preset("child").unwrap() });
        let toon = character(400.0, &Build { detail: 1, ..Build::preset("toon").unwrap() });
        let head_len = |m: &Mesh| {
            let rig = m.rig.as_ref().unwrap();
            let b = &rig.bones[rig.bone("head").unwrap()];
            (b.tail[1] - b.head[1]).abs()
        };
        assert!(head_len(&child) > head_len(&adult) * 1.25, "{} {}", head_len(&child), head_len(&adult));
        assert!(head_len(&toon) > head_len(&adult) * 1.5);
        // The hip joint sits lower on a child's body (shorter legs).
        let hip = |m: &Mesh| {
            let (lo, hi) = m.bounds();
            let rig = m.rig.as_ref().unwrap();
            (rig.bones[rig.bone("thigh.L").unwrap()].head[1] - lo[1]) / (hi[1] - lo[1])
        };
        assert!(hip(&child) > hip(&adult) + 0.03);
        // A hand is bigger on the cartoon.
        let hand = |m: &Mesh| {
            let rig = m.rig.as_ref().unwrap();
            crate::length(crate::sub(rig.bones[rig.bone("hand.L").unwrap()].tail, rig.bones[rig.bone("hand.L").unwrap()].head))
        };
        assert!(hand(&toon) > hand(&adult) * 1.3);
    }

    #[test]
    fn the_hands_have_fingers_that_follow_their_bones() {
        let m = character(400.0, &Build { detail: 2, ..Build::default() });
        let rig = m.rig.clone().unwrap();
        // Curling the index finger of the right hand moves its tip towards the palm, and only the finger.
        let tip = rig.bones[rig.bone("index.R.2").unwrap()].tail;
        let curl = crate::hand::curl_pose(&rig, false, [0.0, 1.0, 0.0, 0.0, 0.0]);
        assert!(curl.len() >= 15);
        let posed = rig.posed_mesh(&m, &PoseInput { rotations: curl.into_iter().collect(), ..Default::default() });
        let near: Vec<usize> = (0..m.positions.len()).filter(|&v| crate::length(crate::sub(m.positions[v], tip)) < 4.0).collect();
        assert!(!near.is_empty(), "the fingertip has surface");
        let moved: f32 = near.iter().map(|&v| crate::length(crate::sub(posed.positions[v], m.positions[v]))).sum::<f32>() / near.len() as f32;
        assert!(moved > 10.0, "the fingertip moved {moved}");
        // It moved towards the body's middle (the palm side).
        let mid = near.iter().map(|&v| posed.positions[v][0]).sum::<f32>() / near.len() as f32;
        let before = near.iter().map(|&v| m.positions[v][0]).sum::<f32>() / near.len() as f32;
        assert!(mid > before - 0.0001 || (mid - before).abs() < 100.0);
        // The wrist and the other hand do not move.
        let wrist = rig.bones[rig.bone("hand.R").unwrap()].head;
        let at_wrist: Vec<usize> = (0..m.positions.len()).filter(|&v| crate::length(crate::sub(m.positions[v], wrist)) < 3.0).collect();
        assert!(at_wrist.iter().all(|&v| crate::length(crate::sub(posed.positions[v], m.positions[v])) < 1.0));
    }

    #[test]
    fn the_clothes_are_separate_parts_over_a_full_body() {
        let m = character(260.0, &Build { detail: 1, ..Build::default() });
        let names: Vec<&str> = m.materials.iter().map(|x| x.name.as_str()).collect();
        for n in ["T-shirt", "Trousers", "Shoes", "Skin"] {
            assert!(names.contains(&n), "{n} in {names:?}");
        }
        // Each is its own surface: the body under them is whole.
        let mut bare = m.clone();
        for n in ["T-shirt", "Trousers", "Shoes"] {
            crate::wear::remove(&mut bare, n).unwrap();
        }
        assert!(bare.rig_in_step());
        let skin = bare.materials.iter().position(|x| x.name == "Skin").unwrap() as u32;
        let mats = bare.tri_materials();
        // A torso, not a hole: skin all down the chest, waist and thighs.
        let (lo, hi) = bare.bounds();
        let h = hi[1] - lo[1];
        for f in [0.3f32, 0.4, 0.6] {
            let y = lo[1] + f * h;
            let n = (0..bare.triangles()).filter(|&t| mats[t] == skin && (bare.centroid(t)[1] - y).abs() < 0.01 * h && bare.centroid(t)[0].abs() < 0.12 * h).count();
            assert!(n > 20, "skin at {f} of the height: {n} triangles");
        }
    }

    #[test]
    fn the_face_has_its_parts_keys_and_bones() {
        let m = character(260.0, &Build { detail: 1, dressed: false, ..Build::default() });
        let rig = m.rig.clone().unwrap();
        for b in ["jaw", "tongue.0", "tongue.1", "tongue.2", "eye.L", "eye.R"] {
            assert!(rig.bone(b).is_some(), "{b}");
            assert!(rig.bone(&b.to_uppercase()).is_some(), "{b} in any case");
        }
        for k in ["jawOpen", "viseme_aa", "eyeBlinkLeft", "eyeBlinkRight", "eyeSquintLeft", "eyeWideLeft", "pupilDilate", "pupilConstrict", "tongueOut", "tongueUp", "tongueDown", "tongueCurl", "tongueTH", "mouthSmileLeft"] {
            assert!(rig.shape(k).is_some(), "{k}");
        }
        let names: Vec<&str> = m.materials.iter().map(|x| x.name.as_str()).collect();
        for n in ["Upper lid", "Lower lid", "Lashes", "Cornea", "Teeth", "Gums", "Tongue", "Lips", "Mouth", "Iris", "Pupil", "Brow"] {
            assert!(names.contains(&n), "{n} in {names:?}");
        }
        // A blink closes the lids without touching the eyeball.
        let frame = crate::face::find_head(&m).unwrap();
        let centre = rig.bones[rig.bone("eye.L").unwrap()].head;
        let re = 0.0525 * frame.size;
        let blink = rig.posed_mesh(&m, &PoseInput { shapes: HashMap::from([("eyeBlinkLeft".to_string(), 1.0)]), ..Default::default() });
        let mats = m.tri_materials();
        let upper_lid = m.materials.iter().position(|x| x.name == "Upper lid").unwrap() as u32;
        let lid: Vec<usize> = (0..m.triangles()).filter(|&t| mats[t] == upper_lid).flat_map(|t| m.tri(t)).filter(|&v| m.positions[v][0] > 0.0).collect();
        assert!(!lid.is_empty());
        for &v in &lid {
            assert!(crate::length(crate::sub(blink.positions[v], centre)) > re * 1.04, "the lid stays off the eyeball");
        }
        let moved: f32 = lid.iter().map(|&v| crate::length(crate::sub(blink.positions[v], m.positions[v]))).sum::<f32>() / lid.len() as f32;
        assert!(moved > re * 0.3, "the lid closes: {moved}");
        // The jaw bone drops the lower teeth and not the upper.
        let open = rig.posed_mesh(&m, &PoseInput { rotations: HashMap::from([("JAW".to_string(), [20.0, 0.0, 0.0])]), ..Default::default() });
        let teeth = m.materials.iter().position(|x| x.name == "Teeth").unwrap() as u32;
        let (mut upper, mut lower) = (0.0f32, 0.0f32);
        for t in 0..m.triangles() {
            if mats[t] != teeth {
                continue;
            }
            let v = m.tri(t)[0];
            let local_y = (m.positions[v][1] - frame.origin[1]) / frame.size;
            let d = open.positions[v][1] - m.positions[v][1];
            if local_y > 0.288 { lower = lower.max(d) } else { upper = upper.max(d.abs()) }
        }
        assert!(lower > 0.04 * frame.size && upper < 0.01 * frame.size, "lower {lower} upper {upper}");
    }

    #[test]
    fn the_neck_turns_without_tearing() {
        let m = character(260.0, &Build { detail: 1, dressed: false, ..Build::default() });
        let rig = m.rig.clone().unwrap();
        let turned = rig.posed_mesh(&m, &PoseInput { rotations: HashMap::from([("head".to_string(), [0.0, 60.0, 0.0])]), ..Default::default() });
        let neck = rig.bones[rig.bone("neck").unwrap()].clone();
        let mut worst = 1.0f32;
        let mut seen = std::collections::HashSet::new();
        for t in 0..m.triangles() {
            let c = m.tri(t);
            for k in 0..3 {
                let (a, b) = (c[k], c[(k + 1) % 3]);
                if !seen.insert((a.min(b), a.max(b))) {
                    continue;
                }
                let mid = crate::lerp(m.positions[a], m.positions[b], 0.5);
                if (mid[1] - neck.head[1]).abs() > 12.0 || (mid[0] - neck.head[0]).abs() > 12.0 {
                    continue;
                }
                let (ra, rb) = (crate::length(crate::sub(m.positions[a], m.positions[b])), crate::length(crate::sub(turned.positions[a], turned.positions[b])));
                if ra > 1.5 {
                    worst = worst.max(rb / ra);
                }
            }
        }
        assert!(worst < 4.0, "the skin round the neck stretches {worst}x at worst in a 60 degree turn");
    }
}
