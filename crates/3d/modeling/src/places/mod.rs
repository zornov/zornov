// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Places and architecture, built as one model with named parts.
//!
//! A person modelling a street does not place a cube per window frame: they
//! build a facade whose bays, sills, lintels, pilasters and cornice are
//! made together, then a street of them. These generators do that for any
//! style or era (classical, art deco, brutalist, modern glass, medieval,
//! oriental, futuristic, rustic) and any wear, and return an [`Asset`]: named
//! parts (walls, window frames, window glass, cornice, kerbs, ...), each
//! with its own material, ready to become ONE model clip.
//!
//! Kinds: `building`, `facade`, `street`, `interior`, `bridge`, `fence`,
//! `stairs`. Sizes are Studio pixels (100 to the metre); y points down.

mod facade;
pub mod geom;
mod interior;
mod mass;
mod street;

use crate::asset::{Asset, Part};
use crate::groom::Rng;
use crate::material::{self, Lobes, MaterialDef, Shading, Surface};
use crate::poly::Poly;
use studio_mesh::{V3, srgb_to_linear};

/// What can be built.
pub const KINDS: &[(&str, &str)] = &[
    ("building", "a whole building: dressed front, plainer sides, roof, entrance (size = height)"),
    ("facade", "one dressed front wall with windows, doors, cornice, pilasters, balconies (size = height)"),
    ("street", "a street between two rows of buildings: road, kerbs, sidewalks, lamps, hydrants, fire escapes (size = length)"),
    ("interior", "a room: floor, walls, skirting, ceiling, windows, door, furniture (size = width)"),
    ("bridge", "a bridge: deck, parapets, piers or arches or cables (size = length)"),
    ("fence", "a fence with posts, rails and infill and a gate (size = length)"),
    ("stairs", "a flight of stairs with a balustrade and a landing (size = height)"),
];

/// The architectural styles.
pub const STYLES: &[(&str, &str)] = &[
    ("classical", "stone, pilasters, cornices, balustrades, pediments"),
    ("art_deco", "stepped setbacks, vertical fins, gilt spandrels, a spire"),
    ("brutalist", "raw concrete, deep-set windows, heavy slabs and ribs"),
    ("modern_glass", "curtain wall, slim mullions, slab edges, rooftop plant"),
    ("medieval", "half-timber, jettied floors, steep roofs, pointed windows"),
    ("oriental", "red columns, tiered tiled roofs with deep eaves, lattice"),
    ("futuristic", "smooth tapering towers, glowing bands, rings, a spire"),
    ("rustic", "rough stone and timber, shingles, shutters, a porch, a chimney"),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    Classical,
    ArtDeco,
    Brutalist,
    ModernGlass,
    Medieval,
    Oriental,
    Futuristic,
    Rustic,
}

impl Style {
    pub fn parse(s: &str) -> Option<Style> {
        let k = s.trim().to_lowercase().replace(['-', ' '], "_");
        Some(match k.as_str() {
            "classical" | "classic" | "neoclassical" | "georgian" | "baroque" | "victorian" | "haussmann" | "parisian" | "european" | "renaissance" => Style::Classical,
            "art_deco" | "deco" | "artdeco" | "1920s" | "1930s" | "new_york" | "skyscraper" => Style::ArtDeco,
            "brutalist" | "brutalism" | "concrete" | "socialist" | "soviet" => Style::Brutalist,
            "modern_glass" | "modern" | "glass" | "contemporary" | "corporate" | "curtain_wall" | "office" => Style::ModernGlass,
            "medieval" | "gothic" | "tudor" | "half_timber" | "timber" | "old_town" | "historic" => Style::Medieval,
            "oriental" | "asian" | "chinese" | "japanese" | "pagoda" | "east_asian" | "temple" => Style::Oriental,
            "futuristic" | "scifi" | "sci_fi" | "cyberpunk" | "future" | "space" => Style::Futuristic,
            "rustic" | "village" | "cottage" | "farm" | "country" | "rural" | "vernacular" => Style::Rustic,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Style::Classical => "classical",
            Style::ArtDeco => "art_deco",
            Style::Brutalist => "brutalist",
            Style::ModernGlass => "modern_glass",
            Style::Medieval => "medieval",
            Style::Oriental => "oriental",
            Style::Futuristic => "futuristic",
            Style::Rustic => "rustic",
        }
    }

    pub fn all() -> [Style; 8] {
        [Style::Classical, Style::ArtDeco, Style::Brutalist, Style::ModernGlass, Style::Medieval, Style::Oriental, Style::Futuristic, Style::Rustic]
    }
}

/// A surface for one part (sRGB colour, kept simple so it can be tinted by
/// wear and turned into a smart material when textures are wanted).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mat {
    pub rgb: [u8; 3],
    pub metallic: f32,
    pub rough: f32,
    pub glow: Option<[f32; 3]>,
    pub glass: bool,
    pub smart: &'static str,
}

impl Mat {
    pub const fn new(rgb: [u8; 3], rough: f32, smart: &'static str) -> Mat {
        Mat { rgb, metallic: 0.0, rough, glow: None, glass: false, smart }
    }
    pub const fn metal(rgb: [u8; 3], rough: f32) -> Mat {
        Mat { rgb, metallic: 1.0, rough, glow: None, glass: false, smart: "steel" }
    }
    pub const fn glass(rgb: [u8; 3]) -> Mat {
        Mat { rgb, metallic: 0.0, rough: 0.05, glow: None, glass: true, smart: "glass" }
    }
    pub const fn glowing(rgb: [u8; 3], strength: f32) -> Mat {
        let k = strength;
        Mat { rgb, metallic: 0.0, rough: 0.4, glow: Some([rgb[0] as f32 / 255.0 * k, rgb[1] as f32 / 255.0 * k, rgb[2] as f32 / 255.0 * k]), glass: false, smart: "emissive" }
    }
}

/// One named part being built.
pub(crate) struct Slot {
    pub name: String,
    pub mat: Mat,
    pub poly: Poly,
}

#[derive(Default)]
pub(crate) struct Parts {
    pub slots: Vec<Slot>,
}

impl Parts {
    /// The polygons of the part called `name` (made with `mat` on first use).
    pub fn slot(&mut self, name: &str, mat: Mat) -> &mut Poly {
        if let Some(i) = self.slots.iter().position(|s| s.name == name) {
            return &mut self.slots[i].poly;
        }
        self.slots.push(Slot { name: name.into(), mat, poly: Poly::new() });
        &mut self.slots.last_mut().unwrap().poly
    }

    /// Another builder's parts added into these, turned `yaw` degrees about
    /// the vertical axis and moved by `by`.
    pub fn absorb(&mut self, other: Parts, yaw: f32, by: V3) {
        let (s, c) = yaw.to_radians().sin_cos();
        for slot in other.slots {
            let mut poly = slot.poly;
            poly.map_positions(|v| [v[0] * c + v[2] * s + by[0], v[1] + by[1], -v[0] * s + v[2] * c + by[2]]);
            self.slot(&slot.name, slot.mat).merge(&poly);
        }
    }

    pub fn triangles(&self) -> usize {
        self.slots.iter().map(|s| (0..s.poly.faces()).map(|f| s.poly.face(f).len().saturating_sub(2)).sum::<usize>()).sum()
    }
}

/// A fitting main size for a kind in a style (px): a medieval house is three
/// storeys, a classical block five, a street three buildings long.
pub fn default_size(kind: &str, style: Style) -> f32 {
    match kind {
        "street" | "road" | "avenue" | "alley" | "city_street" => 3600.0,
        "interior" | "room" | "hall" | "lobby" | "flat" => 650.0,
        "bridge" => 2400.0,
        "fence" | "wall" | "railing" | "gate" => 900.0,
        "stairs" | "staircase" | "steps" => 320.0,
        _ => match style {
            Style::Medieval | Style::Rustic => 950.0,
            Style::Oriental => 1500.0,
            Style::Classical => 2000.0,
            Style::Brutalist => 2100.0,
            Style::ArtDeco | Style::ModernGlass | Style::Futuristic => 3200.0,
        },
    }
}

/// What to build.
#[derive(Clone, Debug)]
pub struct Spec {
    pub kind: String,
    pub style: Style,
    /// 0 pristine .. 1 ruined: grime, stains, broken glass, missing pieces.
    pub wear: f32,
    /// The main length: height for a building, length for a street.
    pub size: f32,
    pub floors: Option<usize>,
    pub width: Option<f32>,
    pub depth: Option<f32>,
    pub seed: u64,
    /// 1 simple, 2 normal, 3 rich (muntins, dentils, balusters).
    pub detail: u8,
    /// Share of windows lit from inside (0..1).
    pub lit: f32,
    /// Smart layered materials baked into textures instead of flat colours.
    pub textured: bool,
    /// Kind specific: street `rows` of buildings per side, bridge `arch`/`beam`/`suspension`,
    /// stairs `straight`/`turn`/`spiral`, fence `picket`/`rail`/`panel`, interior `room`.
    pub variant: Option<String>,
}

impl Spec {
    pub fn new(kind: &str, style: Style) -> Spec {
        Spec { kind: kind.into(), style, wear: 0.2, size: default_size(kind, style), floors: None, width: None, depth: None, seed: 1, detail: 2, lit: 0.0, textured: false, variant: None }
    }
}

/// Everything a builder needs while it works.
pub(crate) struct Ctx {
    pub rng: Rng,
    pub look: facade::Look,
    pub wear: f32,
    pub detail: u8,
    pub lit: f32,
    pub parts: Parts,
}

impl Ctx {
    pub fn new(spec: &Spec) -> Ctx {
        Ctx { rng: Rng::new(spec.seed.wrapping_add(7)), look: facade::Look::of(spec.style), wear: spec.wear.clamp(0.0, 1.0), detail: spec.detail.clamp(1, 3), lit: spec.lit.clamp(0.0, 1.0), parts: Parts::default() }
    }
    /// A child builder with the same look that makes its own parts.
    pub fn child(&mut self) -> Ctx {
        Ctx { rng: Rng::new((self.rng.next() * 1e6) as u64 + 3), look: self.look.clone(), wear: self.wear, detail: self.detail, lit: self.lit, parts: Parts::default() }
    }
    pub fn chance(&mut self, p: f32) -> bool {
        self.rng.next() < p
    }
}

fn linear(rgb: [u8; 3]) -> V3 {
    [srgb_to_linear(rgb[0]), srgb_to_linear(rgb[1]), srgb_to_linear(rgb[2])]
}

/// The material of a part: wear darkens and roughens it.
fn definition(name: &str, mat: Mat, wear: f32, textured: bool, size: f32) -> MaterialDef {
    if textured && !mat.glass && mat.glow.is_none() && let Some(mut d) = material::smart(mat.smart, size, None) {
        if d.base.pattern.is_none() {
            d.base.colour = linear(mat.rgb);
        }
        d.name = name.into();
        return d;
    }
    let grime = [52.0, 47.0, 42.0];
    let k = if mat.glow.is_some() || mat.glass { 0.0 } else { wear * 0.4 };
    let rgb = [0, 1, 2].map(|i| (mat.rgb[i] as f32 * (1.0 - k) + grime[i] * k).round() as u8);
    let mut lobes = Lobes::default();
    if let Some(g) = mat.glow {
        lobes.emissive = Some(g);
    }
    if mat.glass {
        lobes.transmission = 0.9;
        lobes.ior = Some(1.5);
    }
    MaterialDef {
        name: name.into(),
        base: Surface { colour: linear(rgb), metallic: mat.metallic, roughness: (mat.rough + wear * 0.25 * (1.0 - mat.metallic * 0.5)).min(1.0), pattern: None, height: 0.0 },
        layers: Vec::new(),
        shading: if mat.glass { Shading::Glass } else { Shading::Standard },
        lobes,
        micro: None,
    }
}

/// Builds a place as an asset of named parts.
pub fn build(spec: &Spec) -> Result<Asset, String> {
    let mut cx = Ctx::new(spec);
    let kind = spec.kind.to_lowercase();
    // A street's thousands of small pieces keep plain edges unless asked for rich detail; everything else is bevelled to the last piece.
    let big = matches!(kind.as_str(), "street" | "road" | "avenue" | "alley" | "city_street");
    geom::bevel_everything(!big || spec.detail >= 3);
    match kind.as_str() {
        "building" | "house" | "tower" | "skyscraper" | "shop" | "temple" => mass::building(&mut cx, spec),
        "facade" | "front" | "frontage" => mass::facade_only(&mut cx, spec),
        "street" | "road" | "avenue" | "alley" | "city_street" => street::street(&mut cx, spec),
        "interior" | "room" | "hall" | "lobby" | "flat" => interior::room(&mut cx, spec),
        "bridge" => street::bridge(&mut cx, spec),
        "fence" | "wall" | "railing" | "gate" => street::fence(&mut cx, spec),
        "stairs" | "staircase" | "steps" => street::stairs(&mut cx, spec),
        other => {
            geom::bevel_everything(false);
            return Err(format!("place \"{other}\": kinds are {}", KINDS.iter().map(|k| k.0).collect::<Vec<_>>().join(", ")));
        }
    }
    geom::bevel_everything(false);
    let mut asset = Asset::new(&format!("{} {}", spec.style.name().replace('_', " "), kind));
    asset.texture_size = if spec.textured { 512 } else { 64 };
    let wear = cx.wear;
    for slot in cx.parts.slots {
        if slot.poly.faces() == 0 {
            continue;
        }
        let mut poly = slot.poly;
        poly.single_material();
        let def = definition(&slot.name, slot.mat, wear, spec.textured, spec.size);
        let mut part = Part::new(&slot.name, poly);
        part.materials = vec![def.name.clone()];
        // Soft bevels, flat faces: big faces decide the shading.
        part.smooth_angle = 50.0;
        part.modifiers = vec![crate::modifiers::Modifier::WeightedNormals];
        asset.materials.push(def);
        asset.parts.push(part);
    }
    if asset.parts.is_empty() {
        return Err("nothing was built".into());
    }
    Ok(asset)
}

/// The triangles of a place, flat-coloured (or baked when textured).
pub fn build_mesh(spec: &Spec) -> Result<(Asset, studio_mesh::Mesh), String> {
    let asset = build(spec)?;
    let mesh = asset.to_mesh(!spec.textured)?;
    Ok((asset, mesh))
}
