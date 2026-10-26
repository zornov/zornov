// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Things to hold: a small kit of props (a stick, a cup, a glass, a bottle,
//! a phone, a book, a handgun, a sword, a torch, a lantern, a candle, a
//! lighter, a briefcase, a handbag, an umbrella, a ball, a tray) and a flame,
//! any of which a hand can take (see [`crate::hand::Grip`]).
//!
//! Every prop is modelled upright (y grows downward, so the top is -y and
//! the base +y), its front towards -z, with its origin at the place a hand
//! holds it, and carries how it is held and how thick it is across the
//! fist. Sizes are the prop's longest side in px; pass a size that suits
//! the figure (a cup is about a ninth of a person's height).
//!
//! A flame is a general fire element, not part of any prop: a teardrop of
//! emissive layers that any prop (a candle, a torch, a lighter, a stick, a
//! cigarette's tip) can carry at a point of its own, with the strength of the
//! light it gives.

use crate::fabric::{self, Pattern};
use crate::hand::Grip;
use crate::loft;
use crate::sdf::{Part, Shape};
use crate::{Material, Mesh, V3, add, srgb_to_linear};

/// Every kit prop.
pub const NAMES: &str = "stick, cup, glass, bottle, phone, book, pistol, sword, torch, lantern, candle, lighter, briefcase, handbag, umbrella, ball, tray";

/// A place on a prop where a flame burns.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FlameSpot {
    /// Where the flame's base is, in the prop's own frame.
    pub at: V3,
    /// Its height (px).
    pub height: f32,
}

/// A prop ready to hold.
#[derive(Clone, Debug)]
pub struct Prop {
    pub name: String,
    pub mesh: Mesh,
    pub grip: Grip,
    /// How thick it is across the fist (px).
    pub thickness: f32,
    /// Where it burns, when it does.
    pub flame: Option<FlameSpot>,
}

fn lin(c: [u8; 3]) -> [f32; 4] {
    [srgb_to_linear(c[0]), srgb_to_linear(c[1]), srgb_to_linear(c[2]), 1.0]
}

fn mat(name: &str, kind: &str, colour: [u8; 3]) -> Material {
    let mut m = fabric::material(kind, Some(colour), None, Pattern::None, 0).unwrap_or_default();
    m.name = name.into();
    if m.texture.is_none() {
        m.base_colour = lin(colour);
    }
    m
}

fn join(parts: Vec<(Mesh, u32)>, materials: Vec<Material>) -> Mesh {
    let mut out = Mesh { materials, ..Default::default() };
    let mut mats: Vec<u32> = Vec::new();
    for (m, k) in parts {
        let base = out.positions.len() as u32;
        out.positions.extend_from_slice(&m.positions);
        out.normals.extend_from_slice(&m.normals);
        out.uvs.extend_from_slice(&m.uvs);
        out.indices.extend(m.indices.iter().map(|i| i + base));
        mats.extend(std::iter::repeat_n(k, m.triangles()));
    }
    out.regroup(&mats);
    out
}

fn moved(mut m: Mesh, by: V3) -> Mesh {
    for p in &mut m.positions {
        *p = add(*p, by);
    }
    m
}

/// A rounded box centred at `at`.
fn rbox(at: V3, size: V3, round: f32, cell: f32) -> Mesh {
    let mut p = Part::new(Shape::Box, at, size);
    p.round = round.min(size[0].min(size[1]).min(size[2]) * 0.5);
    crate::sdf::build(&[p], vec![Material::default()], cell)
}

/// A flame `height` px tall: a teardrop of emissive layers (a hot core inside
/// a cooler, partly clear body), base at the origin, tip up (-y).
pub fn flame(height: f32, colour: Option<[u8; 3]>) -> Mesh {
    let h = height.max(1.0);
    let outer = colour.unwrap_or([255, 140, 40]);
    let teardrop = |scale_r: f32, scale_h: f32| -> Mesh {
        // From the tip (top) to the base: a teardrop profile.
        let profile: Vec<(f32, f32)> = (0..=14)
            .map(|i| {
                let t = i as f32 / 14.0;
                // Radius: 0 at the tip, widest low down, rounded at the base.
                let r = (t.powf(0.8) * (1.0 - t).powf(0.35) * 1.5).max(0.0) * 0.5 * scale_r * h * 0.5;
                (r, -h * scale_h * (1.0 - t))
            })
            .collect();
        loft::lathe([0.0; 3], &profile, 14)
    };
    let lerp_c = |a: [f32; 3], b: [f32; 3], t: f32| [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t];
    let hot = lerp_c([1.0, 0.95, 0.6], [srgb_to_linear(outer[0]), srgb_to_linear(outer[1]), srgb_to_linear(outer[2])], 0.1);
    let mut body = Material { name: "Flame".into(), base_colour: [srgb_to_linear(outer[0]), srgb_to_linear(outer[1]) * 0.8, srgb_to_linear(outer[2]) * 0.4, 0.62], roughness: 1.0, ..Material::default() };
    body.emissive = [srgb_to_linear(outer[0]) * 4.0, srgb_to_linear(outer[1]) * 2.4, srgb_to_linear(outer[2]) * 0.8];
    let core = Material { name: "Flame core".into(), base_colour: [hot[0], hot[1], hot[2], 1.0], roughness: 1.0, emissive: [hot[0] * 9.0, hot[1] * 8.0, hot[2] * 5.0], ..Material::default() };
    join(vec![(teardrop(1.0, 1.0), 0), (teardrop(0.5, 0.62), 1)], vec![body, core])
}

/// A kit prop by name, about `size` px along its longest side.
pub fn kit(name: &str, size: f32, colour: Option<[u8; 3]>, material: Option<&str>, tip: Option<[u8; 3]>) -> Result<Prop, String> {
    let s = size.max(4.0);
    let cell = (s / 90.0).max(0.2);
    let n = name.trim().to_ascii_lowercase().replace([' ', '-'], "_");
    let pick = |default: [u8; 3]| colour.unwrap_or(default);
    let made = match n.as_str() {
        "stick" | "rod" | "wand" | "baton" | "pencil" | "pen" | "cigarette" | "cigar" | "chopstick" | "straw" => stick(s, colour, tip, &n),
        "cup" | "mug" | "coffee" | "teacup" => cup(s, pick([230, 230, 225]), material.unwrap_or("plastic")),
        "glass" | "tumbler" | "wine_glass" => glass(s, colour),
        "bottle" | "flask" => bottle(s, pick([40, 90, 50]), cell),
        "phone" | "smartphone" | "tablet" | "device" => phone(s, pick([30, 30, 34]), cell),
        "book" | "notebook" | "journal" => book(s, pick([110, 40, 40]), cell),
        "pistol" | "gun" | "handgun" | "revolver" => pistol(s, pick([40, 40, 44]), cell),
        "sword" | "sabre" | "saber" | "blade" | "knife" | "dagger" => sword(s, pick([190, 195, 205]), material.unwrap_or("steel"), n.as_str()),
        "torch" | "brand" => torch(s, pick([90, 60, 40])),
        "lantern" | "lamp" => lantern(s, pick([40, 40, 44]), cell),
        "candle" | "taper" => candle(s, pick([235, 225, 195])),
        "lighter" | "zippo" => lighter(s, pick([170, 175, 185]), cell),
        "briefcase" | "case" | "suitcase" | "attache" => briefcase(s, pick([60, 40, 30]), cell),
        "handbag" | "bag" | "purse" | "pouch" => handbag(s, pick([150, 60, 70]), cell),
        "umbrella" | "parasol" => umbrella(s, pick([30, 40, 70])),
        "ball" | "orb" | "apple" | "sphere" | "globe" => ball(s, pick([200, 60, 50])),
        "tray" | "plate" | "dish" | "platter" => tray(s, pick([190, 190, 195])),
        other => return Err(format!("prop \"{other}\": {NAMES}; or hold any model clip by its id with \"clip\"")),
    };
    Ok(made)
}

fn stick(s: f32, colour: Option<[u8; 3]>, tip: Option<[u8; 3]>, kind: &str) -> Prop {
    // A slim rod along y, held about a third from the base, with an optional
    // head of its own colour (a cigarette's end, a pen's nib, a bulb).
    let r = (s * 0.018).max(0.5);
    let body_c = colour.unwrap_or(match kind {
        "cigarette" | "cigar" => [240, 238, 230],
        "pencil" | "pen" => [220, 190, 60],
        "wand" | "baton" => [70, 45, 30],
        _ => [150, 110, 70],
    });
    let mut materials = vec![mat("Stick", if matches!(kind, "wand" | "baton" | "stick" | "rod") { "leather" } else { "plastic" }, body_c)];
    materials[0].roughness = 0.6;
    let top = -s * 0.64;
    let bot = s * 0.36;
    let body = loft::tube(&[[0.0, bot, 0.0], [0.0, 0.0, 0.0], [0.0, top, 0.0]], &[r, r, r], 8);
    let mut parts = vec![(body, 0)];
    let head_c = tip.or(match kind {
        "cigarette" | "cigar" => Some([200, 120, 60]),
        "pencil" | "pen" => Some([40, 40, 40]),
        _ => None,
    });
    if let Some(c) = head_c {
        materials.push(mat("Stick tip", "plastic", c));
        let ball = moved(crate::prim::uv_sphere(r * if tip.is_some() { 1.8 } else { 1.05 }, 12, 8), [0.0, top, 0.0]);
        parts.push((ball, 1));
    }
    let grip = if matches!(kind, "wand" | "baton" | "stick" | "rod") { Grip::Handle } else { Grip::Pinch };
    Prop { name: kind.into(), mesh: join(parts, materials), grip, thickness: r * 2.0, flame: Some(FlameSpot { at: [0.0, top - r * 1.4, 0.0], height: s * 0.2 }) }
}

fn cup(s: f32, colour: [u8; 3], kind: &str) -> Prop {
    let (h, r) = (s, s * 0.42);
    // A tapered cup, hollow, with a handle: lathe from the rim down the inside and back up the outside.
    let top = -h * 0.5;
    let profile = [(r * 0.94, top), (r * 0.9, top + h * 0.04), (r * 0.78, h * 0.42), (r * 0.0, h * 0.46), (r * 0.0, h * 0.5), (r * 0.8, h * 0.5), (r * 0.98, top + h * 0.06), (r * 1.0, top)];
    let body = loft::lathe([0.0; 3], &profile, 32);
    let handle: Vec<V3> = (0..9).map(|i| { let a = -1.2 + 2.4 * i as f32 / 8.0; [r * 1.0 + r * 0.55 * a.cos().abs() * 0.0 + r * 0.6 * (1.0 - (a * 0.8).cos()).max(0.0) + r * 0.45 * (a.cos() - 0.3), h * 0.05 + a * h * 0.28, 0.0] }).collect();
    let handle = loft::tube(&handle, &vec![r * 0.1; 9], 8);
    let mut m = mat("Cup", kind, colour);
    m.roughness = 0.35;
    Prop { name: "cup".into(), mesh: join(vec![(body, 0), (handle, 0)], vec![m]), grip: Grip::Handle, thickness: r * 2.0, flame: None }
}

fn glass(s: f32, colour: Option<[u8; 3]>) -> Prop {
    let (h, r) = (s, s * 0.22);
    let top = -h * 0.5;
    let profile = [(r, top), (r * 0.98, top + h * 0.02), (r * 0.8, h * 0.46), (0.0, h * 0.48), (0.0, h * 0.5), (r * 0.82, h * 0.5), (r * 1.0, top + h * 0.03)];
    let body = loft::lathe([0.0; 3], &profile, 28);
    let glass = Material { name: "Glass".into(), base_colour: [0.9, 0.95, 1.0, 1.0], roughness: 0.03, transmission: 0.95, ior: 1.5, ..Material::default() };
    let mut parts = vec![(body, 0)];
    let mut materials = vec![glass];
    if let Some(c) = colour {
        // A drink in it, up to two thirds.
        let liquid = loft::lathe([0.0; 3], &[(0.0, top + h * 0.28), (r * 0.9, top + h * 0.28), (r * 0.78, h * 0.4), (0.0, h * 0.42)], 24);
        materials.push(Material { name: "Drink".into(), base_colour: lin(c), roughness: 0.1, transmission: 0.5, ..Material::default() });
        parts.push((liquid, 1));
    }
    Prop { name: "glass".into(), mesh: join(parts, materials), grip: Grip::Handle, thickness: r * 2.0, flame: None }
}

fn bottle(s: f32, colour: [u8; 3], cell: f32) -> Prop {
    let (h, r) = (s, s * 0.14);
    let top = -h * 0.5;
    let profile = [(0.0, top), (r * 0.18, top), (r * 0.2, top + h * 0.02), (r * 0.2, top + h * 0.22), (r * 0.5, top + h * 0.3), (r * 0.98, top + h * 0.42), (r, top + h * 0.55), (r, h * 0.48), (r * 0.8, h * 0.5), (0.0, h * 0.5)];
    let body = loft::lathe([0.0; 3], &profile, 28);
    let _ = cell;
    let glass = Material { name: "Bottle".into(), base_colour: lin(colour), roughness: 0.08, transmission: 0.35, ior: 1.5, ..Material::default() };
    Prop { name: "bottle".into(), mesh: join(vec![(body, 0)], vec![glass]), grip: Grip::Handle, thickness: r * 2.0, flame: None }
}

fn phone(s: f32, colour: [u8; 3], cell: f32) -> Prop {
    let (h, w, d) = (s, s * 0.48, s * 0.05);
    let body = rbox([0.0, 0.0, 0.0], [w, h, d], d * 0.45, cell);
    let screen = rbox([0.0, 0.0, -d * 0.45], [w * 0.92, h * 0.94, d * 0.2], d * 0.1, cell);
    let mut a = mat("Phone", "plastic", colour);
    a.metallic = 0.6;
    a.roughness = 0.3;
    let b = Material { name: "Phone screen".into(), base_colour: [0.01, 0.012, 0.02, 1.0], roughness: 0.04, emissive: [0.05, 0.12, 0.22], ..Material::default() };
    Prop { name: "phone".into(), mesh: join(vec![(body, 0), (screen, 1)], vec![a, b]), grip: Grip::Handle, thickness: w, flame: None }
}

fn book(s: f32, colour: [u8; 3], cell: f32) -> Prop {
    let (h, w, d) = (s, s * 0.72, s * 0.16);
    let cover = rbox([0.0, 0.0, 0.0], [w, h, d], d * 0.2, cell);
    let pages = rbox([w * 0.02, 0.0, 0.0], [w * 0.94, h * 0.94, d * 0.8], d * 0.05, cell);
    let a = mat("Book cover", "leather", colour);
    let b = Material { name: "Book pages".into(), base_colour: lin([235, 228, 210]), roughness: 0.9, ..Material::default() };
    Prop { name: "book".into(), mesh: join(vec![(cover, 0), (pages, 1)], vec![a, b]), grip: Grip::Handle, thickness: d * 1.4, flame: None }
}

fn pistol(s: f32, colour: [u8; 3], cell: f32) -> Prop {
    // The grip is along y (held through the fist), the barrel runs towards -z.
    let l = s;
    let slide = rbox([0.0, -l * 0.12, -l * 0.28], [l * 0.1, l * 0.14, l * 0.62], l * 0.03, cell);
    let barrel = loft::tube(&[[0.0, -l * 0.115, -l * 0.58], [0.0, -l * 0.115, -l * 0.7]], &[l * 0.026, l * 0.026], 10);
    let grip = rbox([0.0, l * 0.1, -l * 0.02], [l * 0.095, l * 0.3, l * 0.16], l * 0.03, cell);
    let guard = loft::tube(&[[0.0, -l * 0.04, -l * 0.2], [0.0, l * 0.02, -l * 0.16], [0.0, -l * 0.01, -l * 0.08]], &[l * 0.012; 3], 6);
    let mut a = mat("Pistol", "steel", colour);
    a.roughness = 0.4;
    Prop { name: "pistol".into(), mesh: join(vec![(slide, 0), (barrel, 0), (grip, 0), (guard, 0)], vec![a]), grip: Grip::Handle, thickness: l * 0.1, flame: None }
}

fn sword(s: f32, colour: [u8; 3], kind: &str, name: &str) -> Prop {
    let (blade_l, grip_l) = (s * 0.72, s * 0.2);
    let w = s * if name == "dagger" || name == "knife" { 0.05 } else { 0.035 };
    // Origin in the middle of the grip; the blade rises (-y).
    let blade_pts: Vec<V3> = (0..=10).map(|i| [0.0, -grip_l * 0.6 - blade_l * i as f32 / 10.0, 0.0]).collect();
    let radii: Vec<f32> = (0..=10).map(|i| w * 0.5 * (1.0 - 0.7 * (i as f32 / 10.0).powi(3))).collect();
    let mut blade = loft::tube(&blade_pts, &radii, 4);
    // Flatten the blade: thin across z.
    loft::deform(&mut blade, |p| [p[0], p[1], p[2] * 0.22]);
    let guard = loft::tube(&[[-w * 1.6, -grip_l * 0.55, 0.0], [w * 1.6, -grip_l * 0.55, 0.0]], &[w * 0.22; 2], 6);
    let grip = loft::tube(&[[0.0, grip_l * 0.5, 0.0], [0.0, -grip_l * 0.55, 0.0]], &[w * 0.28; 2], 8);
    let pommel = moved(crate::prim::uv_sphere(w * 0.4, 10, 8), [0.0, grip_l * 0.55, 0.0]);
    let steel = mat("Blade", kind, colour);
    let hilt = mat("Hilt", "leather", [60, 40, 30]);
    let metal = mat("Guard", "brass", [190, 160, 80]);
    Prop { name: name.into(), mesh: join(vec![(blade, 0), (guard, 2), (grip, 1), (pommel, 2)], vec![steel, hilt, metal]), grip: Grip::Handle, thickness: w * 0.56, flame: None }
}

fn torch(s: f32, colour: [u8; 3]) -> Prop {
    let (h, r) = (s, s * 0.05);
    let shaft = loft::tube(&[[0.0, h * 0.4, 0.0], [0.0, -h * 0.2, 0.0]], &[r, r * 1.1], 10);
    let head = loft::lathe([0.0; 3], &[(r * 1.1, -h * 0.2), (r * 1.7, -h * 0.3), (r * 1.9, -h * 0.4), (r * 1.4, -h * 0.47), (0.0, -h * 0.5)], 14);
    let wood = mat("Torch", "leather", colour);
    let wrap = mat("Torch head", "canvas", [70, 55, 40]);
    Prop { name: "torch".into(), mesh: join(vec![(shaft, 0), (head, 1)], vec![wood, wrap]), grip: Grip::Handle, thickness: r * 2.2, flame: Some(FlameSpot { at: [0.0, -h * 0.47, 0.0], height: h * 0.3 }) }
}

fn lantern(s: f32, colour: [u8; 3], cell: f32) -> Prop {
    // Held by the bail across the top: origin at the bail's middle, the body hanging below.
    let (h, r) = (s, s * 0.16);
    let frame = mat("Lantern frame", "iron", colour);
    let glass = Material { name: "Lantern glass".into(), base_colour: [1.0, 0.9, 0.7, 1.0], roughness: 0.05, transmission: 0.85, ..Material::default() };
    let bail = loft::tube(&(0..9).map(|i| { let a = std::f32::consts::PI * i as f32 / 8.0; [r * 0.9 * a.cos(), -r * 0.9 * a.sin() * 0.7, 0.0] }).collect::<Vec<_>>(), &[r * 0.06; 9], 6);
    let top_cap = loft::lathe([0.0; 3], &[(r * 0.15, 0.0), (r * 0.9, h * 0.08), (r * 1.0, h * 0.14)], 16);
    let body = loft::lathe([0.0; 3], &[(r * 0.95, h * 0.14), (r * 1.0, h * 0.4), (r * 0.9, h * 0.62), (r * 0.7, h * 0.66)], 16);
    let base = loft::lathe([0.0; 3], &[(r * 0.8, h * 0.62), (r * 0.95, h * 0.66), (r * 0.0, h * 0.68)], 16);
    let post = loft::tube(&[[0.0, h * 0.64, 0.0], [0.0, h * 0.4, 0.0]], &[r * 0.35; 2], 8);
    let _ = cell;
    Prop { name: "lantern".into(), mesh: join(vec![(bail, 0), (top_cap, 0), (body, 1), (base, 0), (post, 0)], vec![frame, glass]), grip: Grip::Handle, thickness: r * 0.12, flame: Some(FlameSpot { at: [0.0, h * 0.4, 0.0], height: h * 0.26 }) }
}

fn candle(s: f32, colour: [u8; 3]) -> Prop {
    let (h, r) = (s, s * 0.07);
    let wax = loft::lathe([0.0; 3], &[(0.0, -h * 0.5), (r * 0.9, -h * 0.5), (r, -h * 0.47), (r, h * 0.5), (0.0, h * 0.5)], 18);
    let mut m = mat("Candle", "cotton", colour);
    m.roughness = 0.5;
    m.subsurface = 0.5;
    let wick = loft::tube(&[[0.0, -h * 0.5, 0.0], [0.0, -h * 0.55, 0.0]], &[r * 0.06; 2], 4);
    let black = mat("Wick", "plastic", [20, 20, 20]);
    Prop { name: "candle".into(), mesh: join(vec![(wax, 0), (wick, 1)], vec![m, black]), grip: Grip::Handle, thickness: r * 2.0, flame: Some(FlameSpot { at: [0.0, -h * 0.55, 0.0], height: h * 0.22 }) }
}

fn lighter(s: f32, colour: [u8; 3], cell: f32) -> Prop {
    let (h, w, d) = (s, s * 0.34, s * 0.18);
    let body = rbox([0.0, h * 0.1, 0.0], [w, h * 0.7, d], d * 0.25, cell);
    let lid = rbox([0.0, -h * 0.34, 0.0], [w, h * 0.25, d], d * 0.25, cell);
    let m = mat("Lighter", "steel", colour);
    Prop { name: "lighter".into(), mesh: join(vec![(body, 0), (lid, 0)], vec![m]), grip: Grip::Handle, thickness: w, flame: Some(FlameSpot { at: [0.0, -h * 0.48, 0.0], height: h * 0.22 }) }
}

fn briefcase(s: f32, colour: [u8; 3], cell: f32) -> Prop {
    // Held by its handle: origin at the middle of the handle, the case hanging below.
    let (w, h, d) = (s, s * 0.7, s * 0.22);
    let body = rbox([0.0, h * 0.5 + w * 0.04, 0.0], [w, h, d], d * 0.2, cell);
    let handle = loft::tube(&(0..9).map(|i| { let t = i as f32 / 8.0; [(t - 0.5) * w * 0.34, -w * 0.045 * (1.0 - (2.0 * t - 1.0).powi(2)) + w * 0.03, 0.0] }).collect::<Vec<_>>(), &[w * 0.014; 9], 6);
    let a = mat("Briefcase", "leather", colour);
    let b = mat("Briefcase clasp", "brass", [200, 170, 90]);
    let clasp = rbox([0.0, h * 0.1 + w * 0.04, -d * 0.52], [w * 0.1, h * 0.08, d * 0.05], d * 0.02, cell);
    Prop { name: "briefcase".into(), mesh: join(vec![(body, 0), (handle, 1), (clasp, 1)], vec![a, b]), grip: Grip::Handle, thickness: w * 0.03, flame: None }
}

fn handbag(s: f32, colour: [u8; 3], cell: f32) -> Prop {
    let (w, h, d) = (s, s * 0.7, s * 0.3);
    let body = rbox([0.0, h * 0.5 + w * 0.05, 0.0], [w, h, d], d * 0.45, cell);
    let handle = loft::tube(&(0..11).map(|i| { let a = std::f32::consts::PI * i as f32 / 10.0; [w * 0.2 * a.cos(), w * 0.07 - w * 0.2 * a.sin(), 0.0] }).collect::<Vec<_>>(), &[w * 0.014; 11], 6);
    let a = mat("Handbag", "leather", colour);
    Prop { name: "handbag".into(), mesh: join(vec![(body, 0), (handle, 0)], vec![a]), grip: Grip::Handle, thickness: w * 0.03, flame: None }
}

fn umbrella(s: f32, colour: [u8; 3]) -> Prop {
    // Handle in the fist; the shaft rises, the canopy opens at the top.
    let (h, r) = (s, s * 0.45);
    let shaft = loft::tube(&[[0.0, h * 0.08, 0.0], [0.0, -h * 0.9, 0.0]], &[h * 0.012; 2], 8);
    let hook: Vec<V3> = (0..9).map(|i| { let a = std::f32::consts::PI * i as f32 / 8.0; [-h * 0.05 + h * 0.05 * a.cos(), h * 0.08 + h * 0.05 * a.sin(), 0.0] }).collect();
    let hook = loft::tube(&hook, &vec![h * 0.014; 9], 8);
    let canopy = loft::lathe([0.0; 3], &[(0.0, -h * 1.02), (r * 0.45, -h * 0.98), (r * 0.8, -h * 0.9), (r, -h * 0.78), (r * 0.92, -h * 0.75)], 24);
    let m = mat("Umbrella canopy", "nylon", colour);
    let w = mat("Umbrella handle", "leather", [60, 40, 30]);
    Prop { name: "umbrella".into(), mesh: join(vec![(shaft, 1), (hook, 1), (canopy, 0)], vec![m, w]), grip: Grip::Handle, thickness: h * 0.03, flame: None }
}

fn ball(s: f32, colour: [u8; 3]) -> Prop {
    let m = mat("Ball", "rubber", colour);
    let mesh = join(vec![(crate::prim::uv_sphere(s * 0.5, 24, 16), 0)], vec![m]);
    Prop { name: "ball".into(), mesh, grip: Grip::Palm, thickness: s, flame: None }
}

fn tray(s: f32, colour: [u8; 3]) -> Prop {
    let r = s * 0.5;
    let top = -s * 0.02;
    let body = loft::lathe([0.0; 3], &[(0.0, s * 0.0), (r * 0.9, s * 0.0), (r, top), (r * 1.02, top - s * 0.02), (r * 0.98, top - s * 0.02), (r * 0.88, top + s * 0.01), (0.0, top + s * 0.01)], 32);
    let m = mat("Tray", "steel", colour);
    Prop { name: "tray".into(), mesh: join(vec![(body, 0)], vec![m]), grip: Grip::Palm, thickness: s * 0.04, flame: None }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kit_prop_is_a_surface_with_a_hold() {
        for n in NAMES.split(", ") {
            let p = kit(n, 60.0, None, None, None).unwrap_or_else(|e| panic!("{n}: {e}"));
            assert!(p.mesh.triangles() > 30, "{n}");
            assert!(p.mesh.materials.len() >= 1);
            let (lo, hi) = p.mesh.bounds();
            assert!(hi[0] - lo[0] < 120.0 && hi[1] - lo[1] < 120.0, "{n} is the size asked: {lo:?} {hi:?}");
        }
        assert!(kit("spaceship", 60.0, None, None, None).unwrap_err().contains("stick"));
    }

    #[test]
    fn a_flame_is_emissive_layers_with_its_tip_up() {
        let f = flame(30.0, None);
        assert!(f.materials.iter().all(|m| m.emissive.iter().any(|&e| e > 1.0)));
        let (lo, hi) = f.bounds();
        assert!(lo[1] < -25.0 && hi[1] <= 1.0, "tip up: {lo:?} {hi:?}");
        // The flame spots the kit gives.
        assert!(kit("candle", 60.0, None, None, None).unwrap().flame.is_some() && kit("cup", 60.0, None, None, None).unwrap().flame.is_none());
    }
}
