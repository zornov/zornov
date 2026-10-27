// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Sculpted hairstyles: hair as one closed, solid mass shaped round the
//! head (the way a sculptor or an animated film models it) instead of
//! thousands of strands. The mass is built from blended shapes in the head
//! base's own units, carved clear of the face and ears, scored with locks
//! that run from the crown, pushed out of the body wherever it would sink
//! into the neck or shoulders, and skinned to the head (long hair gives to
//! the neck and chest). It is closed, so a check finds no holes, and reads
//! cleanly at any distance.
//!
//! Styles: `short`, `buzz`, `crew`, `pixie`, `bob`, `long`, `straight`,
//! `wavy`, `curly`, `afro`, `ponytail`, `bun`, `spiky`, `twintails`,
//! `bangs` (long with a full fringe). The anime look cuts the locks deep
//! and hard so they shade in flat bands.

use studio_mesh::bvh::Bvh;
use studio_mesh::sdf::{Join, Part, Shape};
use studio_mesh::wear::Scalp;
use studio_mesh::{Material, Mesh, V3, add, dot, normalise, scale, sub};

/// The sculpted styles, for the error that lists them.
pub const STYLES: &str = "short, buzz, crew, pixie, bob, long, straight, wavy, curly, afro, ponytail, bun, spiky, twintails, bangs";

/// Whether a style word is one this module sculpts.
pub fn knows(style: &str) -> bool {
    canonical(style).is_some()
}

fn canonical(style: &str) -> Option<&'static str> {
    Some(match style.trim().to_ascii_lowercase().replace([' ', '-'], "_").as_str() {
        "" | "short" | "cropped" | "hair" | "wig" | "default" => "short",
        "buzz" | "buzzcut" | "buzz_cut" | "shaved" => "buzz",
        "crew" | "crewcut" | "crew_cut" => "crew",
        "pixie" | "pixie_cut" => "pixie",
        "bob" | "lob" => "bob",
        "long" | "long_straight" | "straight" | "hime" => "long",
        "wavy" | "waves" => "wavy",
        "curly" | "curls" => "curly",
        "afro" | "coily" => "afro",
        "ponytail" | "tail" | "pony" => "ponytail",
        "bun" | "topknot" | "top_knot" => "bun",
        "spiky" | "spikes" | "spiked" | "anime_spiky" => "spiky",
        "twintails" | "twin_tails" | "pigtails" | "twin" => "twintails",
        "bangs" | "fringe" | "long_bangs" => "bangs",
        _ => return None,
    })
}

/// How the hair is made.
#[derive(Clone, Debug)]
pub struct Style {
    pub style: String,
    /// Linear RGB.
    pub colour: V3,
    /// Length against the style's usual (1).
    pub length: f32,
    /// Hard, deep locks and a flat finish, for the anime and toon look.
    pub anime: bool,
    pub seed: u32,
}

/// The cranium of the head base (centre, radii), in head units.
const SKULL: (V3, V3) = ([0.0, -0.13, 0.07], [0.34, 0.4, 0.43]);

/// Sculpts the hair on a figure: a mesh in the body's space with its skin
/// weights, in one material named `name`.
pub fn sculpt(body: &Mesh, sc: &Scalp, st: &Style, name: &str) -> Result<(Mesh, Vec<[u16; 4]>, Vec<[f32; 4]>), String> {
    let style = canonical(&st.style).ok_or_else(|| format!("hair style \"{}\": {STYLES}", st.style))?;
    let (o, s) = (sc.frame.origin, sc.frame.size);
    let len = st.length.clamp(0.3, 2.5);
    let at = |p: V3| add(o, scale(p, s));
    let ell = |c: V3, d: V3, blend: f32| Part { blend: blend * s, ..Part::new(Shape::Ellipsoid, at(c), scale(d, s)) };
    let bx = |c: V3, d: V3, round: f32, blend: f32| Part { round: round * s, blend: blend * s, ..Part::new(Shape::Box, at(c), scale(d, s)) };
    let cone = |a: V3, b: V3, ra: f32, rb: f32, blend: f32| Part { a: scale(a, s), b: scale(b, s), blend: blend * s, ..Part::new(Shape::Cone, o, [ra * 2.0 * s, rb * 2.0 * s, 0.0]) };
    let cut = |p: Part, blend: f32| Part { join: Join::Subtract, blend: blend * s, material: u32::MAX, ..p };
    let (c0, r0) = SKULL;
    // The cap over the skull, `v` thick.
    let cap = |v: f32| ell([0.0, c0[1] - 0.012, c0[2]], [2.0 * (r0[0] + v), 2.0 * (r0[1] + v), 2.0 * (r0[2] + v)], 0.0);
    // The face clear of hair below the hairline; the sides and front of the
    // neck clear below the ears (short styles).
    let face = |wide: f32| cut(ell([0.0, 0.22, -0.5], [0.62 * wide, 1.04, 0.62], 0.0), 0.03);
    let sides = || cut(bx([0.0, 0.36, -0.06], [1.3, 0.64, 0.72], 0.06, 0.0), 0.04);
    let fringe = |n: usize, drop: f32| -> Vec<Part> {
        (0..n)
            .map(|i| {
                let x = -0.17 + 0.34 * i as f32 / (n.max(2) - 1) as f32;
                cone([x * 0.7, -0.45, -0.3], [x * 1.05, -0.2 + drop, -0.445], 0.075, 0.02, 0.03)
            })
            .collect()
    };
    let mut parts: Vec<Part> = Vec::new();
    let mut lumpy = 0.0f32;
    let mut waves = 0.0f32;
    match style {
        "buzz" => parts.extend([cap(0.008), face(1.0), sides()]),
        "crew" => parts.extend([cap(0.018), ell([0.0, -0.4, -0.05], [0.4, 0.16, 0.4], 0.04), face(1.0), sides()]),
        "short" => parts.extend([cap(0.032), ell([0.0, -0.42, -0.02], [0.5, 0.2, 0.55], 0.06), face(1.0), sides()]),
        "pixie" => {
            parts.extend([cap(0.03), face(1.0), sides()]);
            parts.extend(fringe(3, -0.04));
        }
        "spiky" => {
            parts.extend([cap(0.035), face(1.0), sides()]);
            // Spikes out from the crown and back, swept up and back.
            let n = 15;
            for i in 0..n {
                let a = i as f32 / n as f32 * std::f32::consts::TAU;
                let ring = if i % 3 == 0 { 0.35 } else { 0.7 };
                let (x, z) = (ring * a.sin(), ring * a.cos());
                let d = normalise([x, -1.0 + 0.3 * ring, z + 0.35]);
                if d[2] < -0.45 {
                    continue;
                }
                let base = add(c0, [d[0] * r0[0], d[1] * r0[1], d[2] * r0[2]]);
                let l = (0.22 + 0.08 * ((i * 7 % 5) as f32 / 4.0)) * len;
                let tip = add(base, scale(normalise(add(d, [0.0, -0.35, 0.25])), l));
                parts.push(cone(base, tip, 0.1, 0.012, 0.025));
            }
            parts.extend(fringe(3, -0.02));
        }
        "bob" => {
            let bottom = 0.3 + 0.08 * (len - 1.0);
            parts.extend([cap(0.045), ell([0.0, -0.04, 0.06], [0.9, 0.92, 0.98], 0.06), face(1.1)]);
            parts.push(cut(bx([0.0, bottom + 0.5, 0.0], [2.0, 1.0, 2.0], 0.0, 0.0), 0.02));
            parts.extend(fringe(5, 0.0));
        }
        "long" | "bangs" | "wavy" | "curly" => {
            let drop = 0.55 * len;
            parts.extend([cap(0.045), ell([0.0, -0.42, 0.0], [0.55, 0.2, 0.6], 0.06)]);
            // Down the back, and a curtain each side over the ears.
            parts.push(bx([0.0, -0.05 + drop * 0.5 + 0.25, 0.33], [0.74, 0.5 + drop, 0.22], 0.08, 0.08));
            for sx in [1.0f32, -1.0] {
                parts.push(bx([sx * 0.33, 0.12 + drop * 0.25, 0.06], [0.14, 0.55 + drop * 0.5, 0.34], 0.06, 0.06));
            }
            parts.push(face(1.0));
            parts.extend(fringe(if style == "bangs" { 6 } else { 4 }, if style == "bangs" { 0.03 } else { -0.03 }));
            if style == "wavy" {
                waves = 0.025;
            }
            if style == "curly" {
                lumpy = 0.035;
            }
        }
        "afro" => {
            parts.extend([ell([0.0, -0.24, 0.07], [1.1 * len.sqrt(), 1.0 * len.sqrt(), 1.15 * len.sqrt()], 0.0), cut(ell([0.0, 0.25, -0.56], [0.68, 1.1, 0.72], 0.0), 0.04)]);
            parts.push(cut(bx([0.0, 0.42, -0.1], [1.4, 0.6, 0.8], 0.06, 0.0), 0.04));
            lumpy = 0.05;
        }
        "ponytail" => {
            parts.extend([cap(0.025), face(1.0), sides()]);
            parts.push(ell([0.0, -0.24, 0.5], [0.13, 0.13, 0.11], 0.02));
            parts.push(cone([0.0, -0.23, 0.53], [0.0, -0.05 + 0.75 * len, 0.66], 0.11, 0.035, 0.04));
            parts.push(ell([0.0, 0.05, 0.6], [0.2, 0.32, 0.18], 0.06));
        }
        "bun" => {
            parts.extend([cap(0.026), face(1.0), sides()]);
            parts.push(ell([0.0, -0.47, 0.3], [0.34 * len.sqrt(), 0.3 * len.sqrt(), 0.34 * len.sqrt()], 0.03));
        }
        "twintails" => {
            parts.extend([cap(0.035), face(1.0), sides()]);
            for sx in [1.0f32, -1.0] {
                parts.push(ell([sx * 0.31, -0.33, 0.22], [0.12, 0.12, 0.12], 0.02));
                parts.push(cone([sx * 0.33, -0.31, 0.25], [sx * 0.5, -0.1 + 1.0 * len, 0.36], 0.13, 0.05, 0.05));
            }
            parts.extend(fringe(5, 0.0));
        }
        _ => unreachable!(),
    }
    let mat = Material { name: name.to_string(), base_colour: [st.colour[0], st.colour[1], st.colour[2], 1.0], roughness: if st.anime { 0.75 } else { 0.48 }, ..Material::default() };
    let cell = s / 64.0;
    let mut m = studio_mesh::sdf::build(&parts, vec![mat], cell);
    if m.triangles() == 0 {
        return Err(format!("the {style} hair came out empty"));
    }
    studio_mesh::ops::smooth(&mut m, &studio_mesh::region::Region::All, 1, 0.4);
    studio_mesh::ops::smooth_normals_in_place(&mut m);
    // Locks: grooves running down from the crown, cut deep and sharp for
    // the anime look; waves and curls on top.
    let (locks, depth, sharp) = if st.anime { (14.0f32, 0.022f32, 3.0f32) } else { (30.0, 0.01, 1.6) };
    let seed = st.seed;
    for v in 0..m.positions.len() {
        let p = scale(sub(m.positions[v], o), 1.0 / s);
        let n = m.normals[v];
        let phi = p[0].atan2(p[2] - 0.05);
        let jitter = 0.7 * (3.0 * phi + seed as f32).sin() + 0.4 * (p[1] * 6.0).sin();
        let g = (locks * 0.5 * phi + jitter).sin().abs();
        let fade = smoothstep(-0.52, -0.32, p[1]);
        let mut d = -depth * (1.0 - g).powf(sharp) * fade;
        if waves > 0.0 {
            d += waves * (p[1] * 18.0 + phi * 2.0).sin() * smoothstep(0.0, 0.3, p[1]);
        }
        if lumpy > 0.0 {
            d += lumpy * studio_mesh::ops::fbm(scale(p, 9.0), 3, seed);
        }
        m.positions[v] = add(m.positions[v], scale(n, d * s));
    }
    studio_mesh::ops::smooth_normals_in_place(&mut m);
    // Out of the body: the outer surface never sinks into the neck, the
    // shoulders or the back (the inside of the mass stays in the skull).
    let bvh = Bvh::of_mesh(body);
    let gap = 0.006 * s;
    for _ in 0..2 {
        for v in 0..m.positions.len() {
            let p = m.positions[v];
            let local = scale(sub(p, o), 1.0 / s);
            if local[1] < 0.15 && inside_skull(local) {
                continue;
            }
            if let Some((q, tri, dist)) = bvh.nearest(p, 0.25 * s) {
                let bn = body.face_normal(tri as usize);
                let below = dot(sub(p, q), bn) < 0.0;
                if (below || dist < gap) && dot(m.normals[v], bn) > -0.2 {
                    m.positions[v] = add(q, scale(bn, gap));
                }
            }
        }
        studio_mesh::ops::smooth_normals_in_place(&mut m);
    }
    m.uvs = m.positions.iter().map(|p| [(p[0] - o[0]) / s, (p[1] - o[1]) / s]).collect();
    m.single_group();
    // Skin: the head; hair below the nape gives to the neck and chest.
    let mut joints = Vec::with_capacity(m.positions.len());
    let mut weights = Vec::with_capacity(m.positions.len());
    for p in &m.positions {
        let y = (p[1] - o[1]) / s;
        let give = 0.7 * smoothstep(0.3, 1.2, y);
        match (sc.neck_bone, sc.chest_bone) {
            (Some(nb), Some(cb)) if give > 0.01 => {
                joints.push([sc.head_bone as u16, nb as u16, cb as u16, 0]);
                weights.push([1.0 - give, give * 0.45, give * 0.55, 0.0]);
            }
            _ => {
                joints.push([sc.head_bone as u16, 0, 0, 0]);
                weights.push([1.0, 0.0, 0.0, 0.0]);
            }
        }
    }
    Ok((m, joints, weights))
}

fn inside_skull(p: V3) -> bool {
    let (c, r) = SKULL;
    let d = sub(p, c);
    (d[0] / r[0]).powi(2) + (d[1] / r[1]).powi(2) + (d[2] / r[2]).powi(2) < 1.0
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}
