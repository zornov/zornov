// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Hair, beards, moustaches and brows as real geometry on a figure: strands
//! grown by [`crate::groom`] from the scalp (or the jaw, the lip, the brow)
//! of the body, drawn as hair cards, in a hair material of their own,
//! skinned to the head (the ends of long hair give a little to the neck and
//! shoulders so it follows when the head turns), kept out of the head and
//! body, and removable by name like any other thing worn.
//!
//! Styles: `short`, `buzz`, `crew`, `bob`, `long`, `wavy`, `curly`, `afro`,
//! `ponytail`, `bun`, `bald` (nothing); facial hair `beard`, `goatee`,
//! `stubble`, `moustache`, `brows`.

use crate::groom::{self, Groom, Step};
use studio_mesh::bvh::Bvh;
use studio_mesh::wear::{self, Garment};
use studio_mesh::{Mesh, V3, add, dot, length, normalise, scale, sub};

/// What can be grown, for the error that lists it.
pub const STYLES: &str = "hair: short, buzz, crew, bob, long, wavy, curly, afro, ponytail, bun, bald; facial hair: beard, goatee, stubble, moustache, brows";

/// Whether `kind` is something this module grows.
pub fn is_hair(kind: &str) -> bool {
    matches!(kind.trim().to_ascii_lowercase().as_str(), "hair" | "hairstyle" | "wig" | "beard" | "goatee" | "stubble" | "moustache" | "mustache" | "brows" | "eyebrows")
}

/// What to grow.
#[derive(Clone, Debug)]
pub struct HairSpec {
    pub kind: String,
    pub style: String,
    /// sRGB, or the natural colour from `melanin` and `redness`.
    pub colour: Option<[u8; 3]>,
    pub melanin: f32,
    pub redness: f32,
    pub seed: u64,
    /// 0.5 sparse .. 1 usual .. 2 dense.
    pub density: f32,
    /// Length against the style's usual (1).
    pub length: f32,
    pub name: Option<String>,
}

impl Default for HairSpec {
    fn default() -> HairSpec {
        HairSpec { kind: "hair".into(), style: String::new(), colour: None, melanin: 0.6, redness: 0.1, seed: 1, density: 1.0, length: 1.0, name: None }
    }
}

fn lc(s: &str) -> String {
    s.trim().to_ascii_lowercase().replace([' ', '-'], "_")
}

/// Grows the hair. A `bald` style (or a hairless kind) gives nothing: an
/// error that says so.
pub fn fit(body: &Mesh, spec: &HairSpec) -> Result<Garment, String> {
    let sc = wear::scalp(body)?;
    let kind = match lc(&spec.kind).as_str() {
        "hair" | "hairstyle" | "wig" => "hair",
        "beard" | "goatee" | "stubble" => "beard",
        "moustache" | "mustache" => "moustache",
        "brows" | "eyebrows" => "brows",
        other => return Err(format!("hair kind \"{other}\": {STYLES}")),
    };
    let style = {
        let s = lc(&spec.style);
        if s.is_empty() { match kind { "hair" => "short".to_string(), "beard" => lc(&spec.kind), other => other.to_string() } } else { s }
    };
    let style = if style == "hair" || style == "wig" { "short".to_string() } else { style };
    let s = sc.frame.size;
    let o = sc.frame.origin;
    let (faces, mut groom) = match (kind, style.as_str()) {
        ("hair", "bald" | "none") => return Err("bald: nothing to grow (take hair off with \"remove\": \"Hair\")".into()),
        ("hair", st) => {
            let base = |preset: &str, k: f32| Groom::preset(preset, 2.0 * s * spec.length.max(0.2) * k * if preset == "long_straight" || preset == "wavy" { 0.62 } else { 1.0 }).ok_or("preset");
            let parted = |mut g: Groom| {
                g.steps.insert(0, Step::Part { at: add(o, [0.0, -0.5 * s, 0.0]), along: [0.0, 0.0, 1.0], strength: 1.0 });
                g
            };
            let g = match st {
                "short" | "cropped" | "pixie" => base("short_hair", 1.0)?,
                "buzz" | "buzzcut" | "stubble" => Groom { length: 0.035 * s, ..base("short_hair", 1.0)? },
                "crew" | "crewcut" => Groom { length: 0.07 * s, ..base("short_hair", 1.0)? },
                "bob" => Groom { steps: vec![Step::Comb { direction: [0.0, 1.0, 0.1], strength: 0.8 }, Step::Gravity { amount: 0.7 }, Step::Cut { length: Some(0.5 * s * spec.length), fraction: None }, Step::Clump { amount: 0.4, shape: 1.2 }, Step::Frizz { amount: 0.02 }], ..base("long_straight", 1.0)? },
                "long" | "straight" => parted(base("long_straight", 1.0)?),
                "wavy" => parted(base("wavy", 1.0)?),
                "curly" => base("curly", 1.0)?,
                "afro" | "coily" => base("coily", 1.0)?,
                "ponytail" | "tail" => Groom { steps: vec![Step::Comb { direction: [0.0, 0.5, 1.0], strength: 0.95 }, Step::Gravity { amount: 0.5 }, Step::Clump { amount: 0.95, shape: 0.5 }, Step::Noise { amount: 0.05, size: 0.3 * s }], ..base("long_straight", 0.9)? },
                "bun" | "topknot" => Groom { steps: vec![Step::Comb { direction: [0.0, -0.4, 1.0], strength: 0.95 }, Step::Clump { amount: 0.9, shape: 0.4 }, Step::Curl { radius: 0.05 * s, turns: 2.0, random: 0.1 }], ..base("long_straight", 0.55)? },
                other => return Err(format!("hair style \"{other}\": {STYLES}")),
            };
            (sc.scalp.clone(), g)
        }
        ("beard", st) => {
            let g = Groom::preset("beard", 2.0 * s * spec.length.max(0.2)).ok_or("preset")?;
            let g = match st {
                "stubble" => Groom { length: 0.012 * s, ..g },
                "goatee" => Groom { length: 0.07 * s, ..g },
                _ => Groom { length: 0.11 * s * spec.length, ..g },
            };
            let faces: Vec<usize> = if st == "goatee" { sc.beard.iter().copied().filter(|&t| { let p = scale(sub(body.centroid(t), o), 1.0 / s); p[0].abs() < 0.12 && p[1] > 0.33 }).collect() } else { sc.beard.clone() };
            (faces, g)
        }
        ("moustache", _) => (sc.moustache.clone(), Groom { length: 0.07 * s * spec.length, ..Groom::preset("beard", 2.0 * s).ok_or("preset")? }),
        _ => (sc.brows.clone(), Groom { length: 0.045 * s, ..Groom::preset("eyebrows", 2.0 * s).ok_or("preset")? }),
    };
    if faces.is_empty() {
        return Err(format!("no surface for {kind} was found on this head"));
    }
    // Colour: given, or natural.
    if let Some(c) = spec.colour {
        groom.colour = Some([studio_mesh::srgb_to_linear(c[0]), studio_mesh::srgb_to_linear(c[1]), studio_mesh::srgb_to_linear(c[2])]);
    } else {
        groom.melanin = spec.melanin;
        groom.redness = spec.redness;
    }
    groom.seed = spec.seed;
    let surface = wear::scalp_mesh(body);
    let area: f32 = faces.iter().map(|&t| {
        let c = surface.tri(t);
        let (a, b, d) = (surface.positions[c[0]], surface.positions[c[1]], surface.positions[c[2]]);
        length(studio_mesh::cross(sub(b, a), sub(d, a))) * 0.5
    }).sum();
    // Cards: enough that they cover, wide enough to read as hair.
    let want = match kind {
        "hair" if matches!(style.as_str(), "buzz" | "crew" | "short" | "cropped" | "pixie") => 2600.0,
        "hair" => 2000.0,
        "brows" => 140.0,
        "moustache" => 260.0,
        _ => 1400.0,
    } * spec.density.clamp(0.3, 3.0);
    groom.count = want as usize;
    groom.guides = (want as usize / 8).clamp(40, 260);
    groom.segments = if matches!(style.as_str(), "afro" | "coily" | "curly") { 8 } else { 6 };
    let card = (area / want).sqrt() * 2.8;
    groom.root_width = card;
    groom.tip_width = card * 0.12;
    let mut strands = groom::grow(&surface, &faces, &groom);
    // Out of the head and the body: a point inside is pushed back out.
    let bvh = Bvh::of_mesh(body);
    let centre = add(o, scale([0.0, -0.13, 0.07], s));
    let radii = [0.36 * s, 0.43 * s, 0.45 * s];
    for p in &mut strands.points {
        let d = sub(*p, centre);
        let q = (d[0] / radii[0]).powi(2) + (d[1] / radii[1]).powi(2) + (d[2] / radii[2]).powi(2);
        if q < 1.0 && kind == "hair" && d[1] < 0.35 * s {
            let k = 1.0 / q.max(1e-4).sqrt();
            *p = add(centre, scale(d, k));
        }
        if let Some((c, tri, dist)) = bvh.nearest(*p, 0.02 * s * 2.0) {
            let n = body.face_normal(tri as usize);
            if dist < 0.25 && dot(sub(*p, c), n) <= 0.0 {
                *p = add(c, scale(n, 0.3));
            }
        }
    }
    let mut ribbons = groom::ribbons(&strands, &groom);
    // Skin: the head; the ends of long hair give to the neck and chest.
    let rig = body.rig.as_deref().ok_or("no rig")?;
    let per = strands.per;
    let mut joints = Vec::with_capacity(ribbons.positions.len());
    let mut weights = Vec::with_capacity(ribbons.positions.len());
    for vi in 0..ribbons.positions.len() {
        let within = vi % (4 * per);
        let row = (within % (2 * per)) / 2;
        let t = row as f32 / (per - 1) as f32;
        let long = kind == "hair" && matches!(style.as_str(), "long" | "straight" | "wavy" | "bob" | "ponytail" | "curly");
        let give = if long { 0.55 * t * t } else { 0.0 };
        match (sc.neck_bone, sc.chest_bone) {
            (Some(nb), Some(cb)) if give > 0.01 => {
                joints.push([sc.head_bone as u16, nb as u16, cb as u16, 0]);
                weights.push([1.0 - give, give * 0.6, give * 0.4, 0.0]);
            }
            _ => {
                joints.push([sc.head_bone as u16, 0, 0, 0]);
                weights.push([1.0, 0.0, 0.0, 0.0]);
            }
        }
    }
    let _ = rig;
    // A thin cap on the skin in the hair's colour, so no bald gaps show between cards.
    let cap = {
        let mut m = Mesh { materials: vec![Default::default()], ..Default::default() };
        let mut remap = std::collections::HashMap::new();
        for &t in &faces {
            for v in surface.tri(t) {
                let k = *remap.entry(v).or_insert_with(|| {
                    let n = surface.normals.get(v).copied().unwrap_or([0.0, -1.0, 0.0]);
                    m.positions.push(add(surface.positions[v], scale(n, 0.0035 * s)));
                    m.normals.push(n);
                    m.uvs.push([0.0, 0.0]);
                    (m.positions.len() - 1) as u32
                });
                m.indices.push(k);
            }
        }
        m.single_group();
        m
    };
    let name = spec.name.clone().unwrap_or_else(|| match kind { "hair" => "Hair".to_string(), "beard" => "Beard".to_string(), "moustache" => "Moustache".to_string(), _ => "Brows".to_string() });
    ribbons.materials[0].name = name.clone();
    // A bun: a ball of hair at the back of the head.
    let mut g = Garment::new(&name, kind);
    let mat = g.material(ribbons.materials[0].clone());
    g.push(&ribbons, joints, weights, mat);
    {
        let (rc, _) = groom.colours();
        let mut cap_mat = ribbons.materials[0].clone();
        cap_mat.texture = None;
        cap_mat.base_colour = [rc[0] * 0.75, rc[1] * 0.75, rc[2] * 0.75, 1.0];
        let cm = g.material(cap_mat);
        let n = cap.positions.len();
        g.push(&cap, vec![[sc.head_bone as u16, 0, 0, 0]; n], vec![[1.0, 0.0, 0.0, 0.0]; n], cm);
    }
    if kind == "hair" && style == "bun" {
        let mut ball = studio_mesh::prim::uv_sphere(0.2 * s, 24, 14);
        let c = add(o, [0.0, -0.42 * s, 0.34 * s]);
        for p in &mut ball.positions {
            *p = add(*p, c);
        }
        let n = ball.positions.len();
        g.push(&ball, vec![[sc.head_bone as u16, 0, 0, 0]; n], vec![[1.0, 0.0, 0.0, 0.0]; n], mat);
    }
    g.notes.push(format!("{} strands as hair cards", strands.count()));
    let _ = (normalise([1.0, 0.0, 0.0]), V3::default());
    Ok(g)
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_mesh::character::{Build, character};

    #[test]
    fn every_style_grows_on_a_head_and_stays_out_of_it() {
        let body = character(260.0, &Build { detail: 1, dressed: false, ..Build::default() });
        for (kind, style) in [("hair", "short"), ("hair", "long"), ("hair", "curly"), ("hair", "ponytail"), ("hair", "bun"), ("hair", "afro"), ("beard", "beard"), ("moustache", ""), ("brows", "")] {
            let g = fit(&body, &HairSpec { kind: kind.into(), style: style.into(), density: 0.3, ..HairSpec::default() }).unwrap_or_else(|e| panic!("{kind} {style}: {e}"));
            assert!(g.mesh.triangles() > 50, "{kind} {style}");
            assert_eq!(g.joints.len(), g.mesh.positions.len());
            let head = body.rig.as_ref().unwrap().bone("head").unwrap();
            assert!(g.joints.iter().all(|j| j[0] as usize == head));
        }
        assert!(fit(&body, &HairSpec { style: "bald".into(), ..HairSpec::default() }).unwrap_err().contains("bald"));
        assert!(fit(&body, &HairSpec { style: "mohawk".into(), ..HairSpec::default() }).unwrap_err().contains("short"));
    }
}
