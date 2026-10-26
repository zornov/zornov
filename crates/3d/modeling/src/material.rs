// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Layered, physically based materials and smart materials. A material is
//! a base surface with layers over it (paint over metal, rust in patches,
//! dust on top, dirt in the crevices, worn edges), each shown where its
//! mask says — masks read the model's own shape (curvature, occlusion,
//! facing, height, thickness) and procedural patterns. Baked onto an
//! unwrapped model this gives full texture sets: base colour, occlusion /
//! roughness / metallic, normal and height, with detail that follows the
//! form instead of sitting on it like a decal.
//!
//! The shading model behind each preset (skin, hair, glass, car paint,
//! cloth, fur…) is named in [`Shading`] so a renderer can pick the right
//! lobes.

use crate::bake::{Baked, Map, Texel};
use crate::noise::Pattern;
use serde::{Deserialize, Serialize};
use studio_mesh::{V3, dot, lerp, normalise, scale};

/// The kind of light response a material needs from the renderer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Shading {
    #[default]
    Standard,
    /// Subsurface scattering with a red-shifted mean free path, oily sheen.
    Skin,
    /// Strand shading (two specular lobes, one coloured, along the fibre).
    Hair,
    /// Clear transmission with refraction.
    Glass,
    /// Velvety rim (sheen), soft highlights.
    Cloth,
    /// Base coat with metallic flakes under a clear coat.
    CarPaint,
    /// Wet cornea over an iris.
    Eye,
    /// Thin, back-lit leaves.
    Foliage,
    /// Flat bands of colour (cartoon).
    Toon,
}

/// Extra light response beyond base colour / metal / roughness.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct Lobes {
    #[serde(default)]
    pub clearcoat: f32,
    #[serde(default)]
    pub clearcoat_roughness: f32,
    #[serde(default)]
    pub sheen: f32,
    #[serde(default)]
    pub sheen_colour: Option<V3>,
    #[serde(default)]
    pub transmission: f32,
    #[serde(default)]
    pub ior: Option<f32>,
    #[serde(default)]
    pub subsurface: f32,
    #[serde(default)]
    pub subsurface_colour: Option<V3>,
    /// How far light travels under the surface (px).
    #[serde(default)]
    pub subsurface_radius: f32,
    #[serde(default)]
    pub anisotropy: f32,
    /// Metallic flake sparkle (car paint), 0–1.
    #[serde(default)]
    pub flakes: f32,
    #[serde(default)]
    pub emissive: Option<V3>,
}

/// A surface: what one layer looks like.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Surface {
    /// Linear RGB.
    pub colour: V3,
    #[serde(default)]
    pub metallic: f32,
    #[serde(default = "half")]
    pub roughness: f32,
    /// A pattern that varies the colour, roughness and height.
    #[serde(default)]
    pub pattern: Option<Variation>,
    /// How proud of the layer below it stands (px; chips of paint are
    /// thick, rust swells, scratches cut in).
    #[serde(default)]
    pub height: f32,
}

fn half() -> f32 {
    0.5
}

/// Variation within a surface by a pattern.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Variation {
    pub pattern: Pattern,
    /// Feature size (px on the model).
    pub size: f32,
    /// Colour change across the pattern (0 none – 1 a lot).
    #[serde(default = "tenth")]
    pub colour: f32,
    /// Roughness change.
    #[serde(default = "tenth")]
    pub roughness: f32,
    /// Bump height (px).
    #[serde(default)]
    pub bump: f32,
    /// A second colour to blend toward (else lighter/darker).
    #[serde(default)]
    pub to: Option<V3>,
    #[serde(default)]
    pub seed: u32,
}

fn tenth() -> f32 {
    0.1
}

/// Where a layer shows.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "by", rename_all = "snake_case")]
pub enum Mask {
    All,
    /// Convex edges (worn paint, polished corners): curvature above `from`.
    Edges { #[serde(default = "edge_from")] from: f32, #[serde(default = "soft")] softness: f32 },
    /// Crevices (dirt, grime, rust in seams): low curvature and occlusion.
    Crevices { #[serde(default = "cav")] amount: f32, #[serde(default = "soft")] softness: f32 },
    /// Faces looking a way (dust and snow on top, moss on the north side).
    Facing { direction: V3, #[serde(default = "deg")] degrees: f32, #[serde(default = "soft")] softness: f32 },
    /// Along an axis between two heights (water lines, mud splash low down).
    Height { axis: usize, from: f32, to: f32 },
    /// Patches by a pattern (rust spots, chips, moss, stains).
    Patches { pattern: Pattern, size: f32, #[serde(default = "halff")] coverage: f32, #[serde(default = "soft")] softness: f32, #[serde(default)] seed: u32 },
    /// Thin parts (ears, fingers glow red with back light).
    Thin { #[serde(default = "halff")] from: f32 },
    /// Both masks.
    Both { a: Box<Mask>, b: Box<Mask> },
    /// Either mask.
    Either { a: Box<Mask>, b: Box<Mask> },
    /// Everywhere the mask isn't.
    Not { mask: Box<Mask> },
    /// The mask broken up by a pattern (natural edges).
    Breakup { mask: Box<Mask>, pattern: Pattern, size: f32, #[serde(default = "halff")] amount: f32 },
}

fn edge_from() -> f32 {
    0.6
}
fn soft() -> f32 {
    0.08
}
fn cav() -> f32 {
    0.5
}
fn deg() -> f32 {
    50.0
}
fn halff() -> f32 {
    0.5
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0).max(1e-6)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// What a mask reads at a point.
pub struct Here {
    pub pos: V3,
    pub normal: V3,
    pub curvature: f32,
    pub ao: f32,
    pub thickness: f32,
}

impl Mask {
    pub fn eval(&self, h: &Here) -> f32 {
        match self {
            Mask::All => 1.0,
            Mask::Edges { from, softness } => smoothstep(from - softness, from + softness, h.curvature),
            Mask::Crevices { amount, softness } => {
                let cav = (0.5 - h.curvature).max(0.0) * 2.0 + (1.0 - h.ao);
                smoothstep(1.0 - amount - softness, 1.0 - amount + softness, cav)
            }
            Mask::Facing { direction, degrees, softness } => {
                let c = dot(h.normal, normalise(*direction));
                let edge = degrees.to_radians().cos();
                smoothstep(edge - softness, edge + softness, c)
            }
            Mask::Height { axis, from, to } => {
                let t = smoothstep(from.min(*to), from.max(*to), h.pos[(*axis).min(2)]);
                if from > to { 1.0 - t } else { t }
            }
            Mask::Patches { pattern, size, coverage, softness, seed } => {
                let v = 0.5 + 0.5 * pattern.height(h.pos, *size, *seed).clamp(-1.0, 1.0);
                smoothstep(1.0 - coverage - softness, 1.0 - coverage + softness, v)
            }
            Mask::Thin { from } => smoothstep(from - 0.1, from + 0.1, h.thickness),
            Mask::Both { a, b } => a.eval(h) * b.eval(h),
            Mask::Either { a, b } => {
                let (x, y) = (a.eval(h), b.eval(h));
                x + y - x * y
            }
            Mask::Not { mask } => 1.0 - mask.eval(h),
            Mask::Breakup { mask, pattern, size, amount } => {
                let m = mask.eval(h);
                let n = 0.5 + 0.5 * pattern.height(h.pos, *size, 9).clamp(-1.0, 1.0);
                (m - amount * (1.0 - n) * (1.0 - m) * 2.0 + amount * (n - 0.5) * m).clamp(0.0, 1.0) * smoothstep(0.0, 0.2, m)
            }
        }
    }
}

/// A layer over the base.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Layer {
    pub name: String,
    pub surface: Surface,
    pub mask: Mask,
    #[serde(default = "onef")]
    pub opacity: f32,
}

fn onef() -> f32 {
    1.0
}

/// A full material.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MaterialDef {
    pub name: String,
    pub base: Surface,
    #[serde(default)]
    pub layers: Vec<Layer>,
    #[serde(default)]
    pub shading: Shading,
    #[serde(default)]
    pub lobes: Lobes,
    /// Fine surface detail as a bump over everything (pores, grain,
    /// weave): pattern, size px, height px.
    #[serde(default)]
    pub micro: Option<(Pattern, f32, f32)>,
}

/// One point of a material, evaluated.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sample {
    pub colour: V3,
    pub metallic: f32,
    pub roughness: f32,
    pub height: f32,
}

fn surface_at(s: &Surface, p: V3) -> Sample {
    let mut out = Sample { colour: s.colour, metallic: s.metallic, roughness: s.roughness, height: s.height };
    if let Some(v) = &s.pattern {
        let n = v.pattern.height(p, v.size, v.seed).clamp(-1.5, 1.5);
        out.colour = match v.to {
            Some(to) => lerp(s.colour, to, (0.5 + 0.5 * n).clamp(0.0, 1.0) * v.colour.clamp(0.0, 1.0) * 2.0),
            None => scale(s.colour, (1.0 + v.colour * n).max(0.0)),
        };
        out.roughness = (s.roughness + v.roughness * n * 0.5).clamp(0.02, 1.0);
        out.height += v.bump * n;
    }
    out
}

impl MaterialDef {
    /// The material at a point.
    pub fn eval(&self, h: &Here) -> Sample {
        let mut s = surface_at(&self.base, h.pos);
        for l in &self.layers {
            let m = (l.mask.eval(h) * l.opacity).clamp(0.0, 1.0);
            if m <= 0.0 {
                continue;
            }
            let t = surface_at(&l.surface, h.pos);
            s.colour = lerp(s.colour, t.colour, m);
            s.metallic += (t.metallic - s.metallic) * m;
            s.roughness += (t.roughness - s.roughness) * m;
            s.height += t.height * m;
        }
        if let Some((p, size, amount)) = self.micro {
            s.height += amount * p.height(h.pos, size, 3);
        }
        s
    }

    /// One flat colour / finish for renderers without textures.
    pub fn average(&self) -> Sample {
        let mut acc = Sample { colour: [0.0; 3], metallic: 0.0, roughness: 0.0, height: 0.0 };
        let n = 64;
        for i in 0..n {
            let p = [i as f32 * 17.3, (i * 7 % 13) as f32 * 11.1, (i * 3 % 7) as f32 * 23.7];
            let h = Here { pos: p, normal: normalise([0.3, -0.8, 0.5]), curvature: 0.5, ao: 0.9, thickness: 0.2 };
            let s = self.eval(&h);
            acc.colour = studio_mesh::add(acc.colour, s.colour);
            acc.metallic += s.metallic;
            acc.roughness += s.roughness;
        }
        Sample { colour: scale(acc.colour, 1.0 / n as f32), metallic: acc.metallic / n as f32, roughness: acc.roughness / n as f32, height: 0.0 }
    }
}

/// Baked texture maps for one material on one model.
pub struct TextureSet {
    /// Linear RGB + alpha.
    pub base_colour: Map,
    /// Occlusion, roughness, metallic (glTF layout).
    pub orm: Map,
    /// Tangent-space normals (0–1).
    pub normal: Map,
    /// Height, 0.5 = the surface.
    pub height: Map,
}

/// Bakes a material over a model's texels, using its baked shape maps
/// (occlusion, curvature, thickness, and the dense normals if any).
pub fn bake_material(def: &MaterialDef, texels: &[Texel], shape: &Baked, size: u32, px_per_texel: f32) -> TextureSet {
    let mut base = Map::new(size, size, 4, 0.0);
    let mut orm = Map::new(size, size, 3, 0.0);
    let mut height = Map::new(size, size, 1, 0.5);
    let mut hraw = vec![0.0f32; (size * size) as usize];
    for t in texels {
        let i = (t.y * size + t.x) as usize;
        let h = Here { pos: t.pos, normal: t.normal, curvature: shape.curvature.data[i], ao: shape.ao.data[i], thickness: shape.thickness.data[i] };
        let s = def.eval(&h);
        base.set(t.x, t.y, &[s.colour[0], s.colour[1], s.colour[2], 1.0]);
        orm.set(t.x, t.y, &[shape.ao.data[i], s.roughness, s.metallic]);
        hraw[i] = s.height;
        base.covered[i] = true;
        orm.covered[i] = true;
        height.covered[i] = true;
    }
    // Height → normals (in texel steps; heights in px over px per texel).
    let mut normal = shape.normal.clone();
    let k = 1.0 / px_per_texel.max(1e-6);
    let at = |x: i64, y: i64| -> f32 {
        let (x, y) = (x.clamp(0, size as i64 - 1) as u32, y.clamp(0, size as i64 - 1) as u32);
        hraw[(y * size + x) as usize]
    };
    for t in texels {
        let (x, y) = (t.x as i64, t.y as i64);
        let i = (t.y * size + t.x) as usize;
        let cov = |x: i64, y: i64| x >= 0 && y >= 0 && x < size as i64 && y < size as i64 && base.covered[(y as u32 * size + x as u32) as usize];
        let hx = if cov(x + 1, y) && cov(x - 1, y) { (at(x + 1, y) - at(x - 1, y)) / 2.0 } else { 0.0 };
        let hy = if cov(x, y + 1) && cov(x, y - 1) { (at(x, y + 1) - at(x, y - 1)) / 2.0 } else { 0.0 };
        let detail = normalise([-hx * k, -hy * k, 1.0]);
        // Reoriented normal blending onto the baked normal.
        let b = [normal.data[i * 3] * 2.0 - 1.0, normal.data[i * 3 + 1] * 2.0 - 1.0, normal.data[i * 3 + 2] * 2.0 - 1.0];
        let tz = [b[0], b[1], b[2] + 1.0];
        let u = [-detail[0], -detail[1], detail[2]];
        let d = dot(tz, u) / tz[2].max(1e-6);
        let r = normalise([tz[0] * d - u[0], tz[1] * d - u[1], tz[2] * d - u[2]]);
        normal.data[i * 3] = r[0] * 0.5 + 0.5;
        normal.data[i * 3 + 1] = r[1] * 0.5 + 0.5;
        normal.data[i * 3 + 2] = r[2] * 0.5 + 0.5;
        height.set(t.x, t.y, &[(0.5 + hraw[i] / 8.0).clamp(0.0, 1.0)]);
    }
    for m in [&mut base, &mut orm, &mut height, &mut normal] {
        m.dilate(8);
    }
    TextureSet { base_colour: base, orm, normal, height }
}

fn c(r: u8, g: u8, b: u8) -> V3 {
    [studio_mesh::srgb_to_linear(r), studio_mesh::srgb_to_linear(g), studio_mesh::srgb_to_linear(b)]
}

fn surf(colour: V3, metallic: f32, roughness: f32) -> Surface {
    Surface { colour, metallic, roughness, pattern: None, height: 0.0 }
}

fn vary(s: Surface, pattern: Pattern, size: f32, colour: f32, roughness: f32, bump: f32) -> Surface {
    Surface { pattern: Some(Variation { pattern, size, colour, roughness, bump, to: None, seed: 0 }), ..s }
}

/// Smart materials by name; `scale` is the model's size in px (patterns
/// are sized to it). `colour` overrides the main colour where it makes
/// sense (paint, plastic, fabric, car paint).
pub fn smart(name: &str, scale_px: f32, colour: Option<V3>) -> Option<MaterialDef> {
    let s = scale_px.max(1.0);
    let def = |name: &str, base: Surface, layers: Vec<Layer>, shading: Shading, lobes: Lobes, micro: Option<(Pattern, f32, f32)>| MaterialDef { name: name.into(), base, layers, shading, lobes, micro };
    let layer = |name: &str, surface: Surface, mask: Mask| Layer { name: name.into(), surface, mask, opacity: 1.0 };
    let pick = |d: V3| colour.unwrap_or(d);
    let dirt = |amount: f32| layer("Dirt", surf(c(58, 48, 38), 0.0, 0.9), Mask::Breakup { mask: Box::new(Mask::Crevices { amount, softness: 0.15 }), pattern: Pattern::Noise, size: s * 0.05, amount: 0.5 });
    let dust = || layer("Dust", surf(c(160, 150, 135), 0.0, 0.95), Mask::Breakup { mask: Box::new(Mask::Facing { direction: [0.0, -1.0, 0.0], degrees: 35.0, softness: 0.2 }), pattern: Pattern::Noise, size: s * 0.03, amount: 0.6 });
    Some(match name {
        "painted_metal" | "painted_metal_worn" => def(
            name,
            vary(surf(pick(c(48, 92, 130)), 0.0, 0.45), Pattern::Noise, s * 0.08, 0.05, 0.1, 0.0),
            vec![
                layer("Bare metal", Surface { height: -0.6, ..vary(surf(c(160, 160, 165), 1.0, 0.35), Pattern::Wrinkles, s * 0.01, 0.08, 0.2, 0.0) }, Mask::Breakup { mask: Box::new(Mask::Edges { from: 0.58, softness: 0.05 }), pattern: Pattern::Cracks, size: s * 0.03, amount: 0.6 }),
                layer("Chips", Surface { height: -0.4, ..surf(c(150, 150, 155), 1.0, 0.4) }, Mask::Patches { pattern: Pattern::Cells, size: s * 0.012, coverage: 0.06, softness: 0.03, seed: 4 }),
                dirt(0.45),
                dust(),
            ],
            Shading::Standard,
            Lobes::default(),
            None,
        ),
        "rusty_iron" | "rust" => def(
            name,
            vary(surf(c(70, 68, 66), 1.0, 0.55), Pattern::Hammered, s * 0.02, 0.1, 0.2, 0.2),
            vec![
                layer("Rust", Surface { height: 0.5, ..Surface { pattern: Some(Variation { pattern: Pattern::Rock, size: s * 0.01, colour: 0.5, roughness: 0.1, bump: 0.6, to: Some(c(140, 70, 30)), seed: 2 }), ..surf(c(95, 45, 22), 0.0, 0.9) } }, Mask::Either { a: Box::new(Mask::Patches { pattern: Pattern::Noise, size: s * 0.15, coverage: 0.55, softness: 0.1, seed: 1 }), b: Box::new(Mask::Crevices { amount: 0.6, softness: 0.2 }) }),
                dirt(0.4),
            ],
            Shading::Standard,
            Lobes::default(),
            None,
        ),
        "brushed_aluminium" | "aluminium" | "brushed_metal" => def(name, vary(surf(c(200, 202, 205), 1.0, 0.3), Pattern::Wrinkles, s * 0.002, 0.03, 0.15, 0.02), vec![layer("Polished edges", surf(c(220, 222, 225), 1.0, 0.15), Mask::Edges { from: 0.62, softness: 0.05 })], Shading::Standard, Lobes { anisotropy: 0.7, ..Default::default() }, None),
        "chrome" => def(name, surf(c(240, 240, 242), 1.0, 0.04), vec![], Shading::Standard, Lobes::default(), None),
        "steel" | "stainless_steel" => def(name, vary(surf(c(170, 172, 176), 1.0, 0.28), Pattern::Noise, s * 0.05, 0.03, 0.1, 0.0), vec![dirt(0.55)], Shading::Standard, Lobes::default(), None),
        "gold" => def(name, vary(surf(c(255, 200, 90), 1.0, 0.2), Pattern::Noise, s * 0.05, 0.02, 0.08, 0.0), vec![layer("Tarnish", surf(c(150, 110, 50), 1.0, 0.45), Mask::Crevices { amount: 0.5, softness: 0.2 })], Shading::Standard, Lobes::default(), None),
        "copper" | "copper_patina" | "bronze" => def(
            name,
            vary(surf(if name == "bronze" { c(180, 120, 70) } else { c(220, 130, 90) }, 1.0, 0.3), Pattern::Hammered, s * 0.03, 0.06, 0.15, 0.1),
            vec![layer("Verdigris", surf(c(90, 160, 130), 0.0, 0.8), Mask::Breakup { mask: Box::new(Mask::Crevices { amount: 0.55, softness: 0.2 }), pattern: Pattern::Noise, size: s * 0.04, amount: 0.6 })],
            Shading::Standard,
            Lobes::default(),
            None,
        ),
        "car_paint" => def(name, surf(pick(c(160, 10, 18)), 0.3, 0.35), vec![], Shading::CarPaint, Lobes { clearcoat: 1.0, clearcoat_roughness: 0.03, flakes: 0.6, ..Default::default() }, Some((Pattern::Noise, s * 0.2, 0.02))),
        "plastic" | "plastic_glossy" => def(name, vary(surf(pick(c(220, 60, 40)), 0.0, 0.25), Pattern::Noise, s * 0.1, 0.02, 0.05, 0.0), vec![layer("Scuffs", surf(pick(c(220, 60, 40)), 0.0, 0.5), Mask::Edges { from: 0.62, softness: 0.06 }), dirt(0.6)], Shading::Standard, Lobes::default(), None),
        "plastic_matte" => def(name, surf(pick(c(40, 42, 45)), 0.0, 0.7), vec![dirt(0.6)], Shading::Standard, Lobes::default(), Some((Pattern::Pores, s * 0.003, 0.05))),
        "rubber" => def(name, surf(pick(c(25, 25, 27)), 0.0, 0.85), vec![dust()], Shading::Standard, Lobes::default(), Some((Pattern::Noise, s * 0.004, 0.05))),
        "leather" => def(name, vary(surf(pick(c(110, 62, 35)), 0.0, 0.55), Pattern::Noise, s * 0.06, 0.15, 0.1, 0.0), vec![layer("Worn", surf(c(150, 95, 60), 0.0, 0.45), Mask::Edges { from: 0.6, softness: 0.08 }), dirt(0.5)], Shading::Standard, Lobes { sheen: 0.2, ..Default::default() }, Some((Pattern::Pores, s * 0.004, 0.12))),
        "fabric" | "cotton" => def(name, vary(surf(pick(c(150, 150, 140)), 0.0, 0.9), Pattern::Noise, s * 0.08, 0.06, 0.05, 0.0), vec![dirt(0.6)], Shading::Cloth, Lobes { sheen: 0.6, ..Default::default() }, Some((Pattern::Weave, s * 0.004, 0.08))),
        "denim" => def(name, vary(surf(pick(c(40, 60, 100)), 0.0, 0.85), Pattern::Wrinkles, s * 0.002, 0.15, 0.05, 0.0), vec![layer("Faded", surf(c(110, 130, 160), 0.0, 0.9), Mask::Edges { from: 0.58, softness: 0.1 })], Shading::Cloth, Lobes { sheen: 0.4, ..Default::default() }, Some((Pattern::Weave, s * 0.003, 0.1))),
        "velvet" => def(name, surf(pick(c(120, 10, 40)), 0.0, 0.8), vec![], Shading::Cloth, Lobes { sheen: 1.0, sheen_colour: Some(c(255, 150, 180)), ..Default::default() }, None),
        "wood" | "wood_varnished" => def(name, Surface { pattern: Some(Variation { pattern: Pattern::Wood, size: s * 0.15, colour: 0.4, roughness: 0.1, bump: 0.05, to: Some(c(90, 50, 25)), seed: 1 }), ..surf(pick(c(150, 95, 55)), 0.0, if name == "wood" { 0.6 } else { 0.25 }) }, vec![dirt(0.6)], Shading::Standard, Lobes { clearcoat: if name == "wood_varnished" { 0.6 } else { 0.0 }, clearcoat_roughness: 0.1, ..Default::default() }, None),
        "concrete" => def(name, vary(surf(c(130, 128, 124), 0.0, 0.9), Pattern::Noise, s * 0.05, 0.1, 0.05, 0.1), vec![layer("Stains", surf(c(95, 92, 88), 0.0, 0.9), Mask::Patches { pattern: Pattern::Noise, size: s * 0.2, coverage: 0.3, softness: 0.2, seed: 5 }), dirt(0.5)], Shading::Standard, Lobes::default(), Some((Pattern::Pores, s * 0.006, 0.25))),
        "stone" | "rock" => def(name, vary(surf(c(120, 112, 100), 0.0, 0.85), Pattern::Rock, s * 0.08, 0.25, 0.1, 1.0), vec![layer("Moss", surf(c(60, 90, 30), 0.0, 0.9), Mask::Breakup { mask: Box::new(Mask::Both { a: Box::new(Mask::Facing { direction: [0.0, -1.0, 0.0], degrees: 45.0, softness: 0.2 }), b: Box::new(Mask::Crevices { amount: 0.4, softness: 0.3 }) }), pattern: Pattern::Noise, size: s * 0.05, amount: 0.6 })], Shading::Standard, Lobes::default(), Some((Pattern::Rock, s * 0.01, 0.3))),
        "marble" => def(name, Surface { pattern: Some(Variation { pattern: Pattern::Ridges, size: s * 0.2, colour: 0.6, roughness: 0.05, bump: 0.0, to: Some(c(120, 120, 125)), seed: 3 }), ..surf(c(235, 232, 228), 0.0, 0.12) }, vec![], Shading::Standard, Lobes { subsurface: 0.3, subsurface_radius: s * 0.01, ..Default::default() }, None),
        "ceramic" | "porcelain" => def(name, surf(pick(c(240, 238, 232)), 0.0, 0.08), vec![dirt(0.7)], Shading::Standard, Lobes { clearcoat: 0.8, clearcoat_roughness: 0.02, ..Default::default() }, None),
        "glass" | "glass_clear" => def(name, surf(c(245, 250, 250), 0.0, 0.02), vec![], Shading::Glass, Lobes { transmission: 1.0, ior: Some(1.5), ..Default::default() }, None),
        "glass_frosted" => def(name, surf(c(240, 245, 245), 0.0, 0.35), vec![], Shading::Glass, Lobes { transmission: 0.9, ior: Some(1.5), ..Default::default() }, None),
        "water" => def(name, surf(c(200, 225, 230), 0.0, 0.02), vec![], Shading::Glass, Lobes { transmission: 1.0, ior: Some(1.33), ..Default::default() }, None),
        "gem" | "diamond" => def(name, surf(pick(c(250, 250, 255)), 0.0, 0.0), vec![], Shading::Glass, Lobes { transmission: 1.0, ior: Some(2.4), ..Default::default() }, None),
        "skin" | "skin_realistic" => def(
            name,
            Surface { pattern: Some(Variation { pattern: Pattern::Noise, size: s * 0.03, colour: 0.12, roughness: 0.08, bump: 0.0, to: Some(c(190, 110, 95)), seed: 7 }), ..surf(pick(c(205, 150, 125)), 0.0, 0.5) },
            vec![
                layer("Flush", surf(c(200, 105, 95), 0.0, 0.45), Mask::Patches { pattern: Pattern::Noise, size: s * 0.12, coverage: 0.25, softness: 0.25, seed: 11 }),
                layer("Oily", surf(pick(c(205, 150, 125)), 0.0, 0.32), Mask::Edges { from: 0.56, softness: 0.1 }),
                layer("Creases", surf(c(150, 95, 80), 0.0, 0.6), Mask::Crevices { amount: 0.55, softness: 0.15 }),
            ],
            Shading::Skin,
            Lobes { subsurface: 0.7, subsurface_colour: Some(c(230, 90, 70)), subsurface_radius: s * 0.012, sheen: 0.1, ..Default::default() },
            Some((Pattern::Pores, s * 0.0025, 0.035)),
        ),
        "skin_stylized" | "skin_cartoon" => def(name, surf(pick(c(240, 180, 150)), 0.0, 0.6), vec![layer("Cheeks", surf(c(240, 140, 130), 0.0, 0.6), Mask::Patches { pattern: Pattern::Noise, size: s * 0.2, coverage: 0.15, softness: 0.3, seed: 2 })], Shading::Skin, Lobes { subsurface: 0.5, subsurface_colour: Some(c(240, 100, 80)), subsurface_radius: s * 0.02, ..Default::default() }, None),
        "creature_skin" | "scales" | "reptile" => def(name, Surface { pattern: Some(Variation { pattern: Pattern::Scales, size: s * 0.01, colour: 0.25, roughness: 0.15, bump: 0.6, to: Some(c(40, 60, 30)), seed: 4 }), ..surf(pick(c(90, 110, 60)), 0.0, 0.45) }, vec![layer("Belly", surf(c(190, 180, 130), 0.0, 0.5), Mask::Facing { direction: [0.0, 1.0, 0.0], degrees: 60.0, softness: 0.25 }), dirt(0.5)], Shading::Skin, Lobes { subsurface: 0.3, subsurface_radius: s * 0.005, ..Default::default() }, Some((Pattern::Scales, s * 0.01, 0.4))),
        "eye" => def(name, surf(c(245, 245, 240), 0.0, 0.05), vec![], Shading::Eye, Lobes { clearcoat: 1.0, clearcoat_roughness: 0.0, subsurface: 0.3, ..Default::default() }, None),
        "leaf" | "foliage" => def(name, vary(surf(pick(c(55, 105, 35)), 0.0, 0.5), Pattern::Ridges, s * 0.02, 0.15, 0.1, 0.0), vec![layer("Dry edges", surf(c(150, 120, 50), 0.0, 0.7), Mask::Edges { from: 0.62, softness: 0.1 })], Shading::Foliage, Lobes { subsurface: 0.5, subsurface_colour: Some(c(150, 200, 60)), transmission: 0.3, ..Default::default() }, None),
        "bark" => def(name, Surface { pattern: Some(Variation { pattern: Pattern::Wrinkles, size: s * 0.03, colour: 0.3, roughness: 0.1, bump: 1.5, to: Some(c(50, 38, 28)), seed: 5 }), ..surf(c(95, 75, 58), 0.0, 0.85) }, vec![layer("Moss", surf(c(70, 95, 35), 0.0, 0.9), Mask::Breakup { mask: Box::new(Mask::Crevices { amount: 0.4, softness: 0.2 }), pattern: Pattern::Noise, size: s * 0.04, amount: 0.6 })], Shading::Standard, Lobes::default(), Some((Pattern::Wrinkles, s * 0.01, 0.5))),
        "terrain" | "ground" => def(
            name,
            vary(surf(c(110, 90, 65), 0.0, 0.95), Pattern::Rock, s * 0.01, 0.2, 0.05, 0.5),
            vec![
                layer("Grass", vary(surf(c(70, 100, 40), 0.0, 0.9), Pattern::Noise, s * 0.01, 0.25, 0.05, 0.0), Mask::Breakup { mask: Box::new(Mask::Facing { direction: [0.0, -1.0, 0.0], degrees: 30.0, softness: 0.15 }), pattern: Pattern::Noise, size: s * 0.05, amount: 0.6 }),
                layer("Rock", vary(surf(c(120, 115, 105), 0.0, 0.8), Pattern::Rock, s * 0.02, 0.2, 0.1, 1.0), Mask::Not { mask: Box::new(Mask::Facing { direction: [0.0, -1.0, 0.0], degrees: 45.0, softness: 0.15 }) }),
                layer("Snow", surf(c(240, 245, 250), 0.0, 0.6), Mask::Both { a: Box::new(Mask::Facing { direction: [0.0, -1.0, 0.0], degrees: 40.0, softness: 0.1 }), b: Box::new(Mask::Height { axis: 1, from: -s * 0.15, to: -s * 0.3 }) }),
            ],
            Shading::Standard,
            Lobes::default(),
            None,
        ),
        "clay" => def(name, surf(pick(c(190, 120, 90)), 0.0, 0.7), vec![], Shading::Standard, Lobes { subsurface: 0.15, ..Default::default() }, Some((Pattern::Noise, s * 0.01, 0.05))),
        "toon" | "cartoon" => def(name, surf(pick(c(240, 120, 60)), 0.0, 0.6), vec![], Shading::Toon, Lobes::default(), None),
        "fur" => def(name, surf(pick(c(120, 90, 60)), 0.0, 0.8), vec![], Shading::Hair, Lobes::default(), None),
        "bone" | "ivory" => def(name, vary(surf(c(225, 215, 190), 0.0, 0.5), Pattern::Noise, s * 0.05, 0.08, 0.1, 0.0), vec![dirt(0.5)], Shading::Standard, Lobes { subsurface: 0.25, ..Default::default() }, Some((Pattern::Pores, s * 0.005, 0.05))),
        "emissive" | "light" | "neon" => def(name, surf(pick(c(255, 230, 180)), 0.0, 0.4), vec![], Shading::Standard, Lobes { emissive: Some(scale(pick(c(255, 230, 180)), 4.0)), ..Default::default() }, None),
        _ => return None,
    })
}

pub const SMART: &[&str] = &[
    "painted_metal", "rusty_iron", "brushed_aluminium", "chrome", "steel", "gold", "copper", "bronze", "car_paint", "plastic", "plastic_matte", "rubber", "leather", "fabric", "denim", "velvet", "wood", "wood_varnished", "concrete", "stone", "marble", "ceramic", "glass", "glass_frosted", "water", "gem", "skin", "skin_stylized", "creature_skin", "eye", "leaf", "bark", "terrain", "clay", "toon", "fur", "bone", "emissive",
];

#[cfg(test)]
mod tests {
    use super::*;

    fn here(curv: f32, ao: f32, n: V3) -> Here {
        Here { pos: [10.0, 20.0, 30.0], normal: n, curvature: curv, ao, thickness: 0.2 }
    }

    #[test]
    fn every_smart_material_evaluates_physically() {
        for name in SMART {
            let m = smart(name, 500.0, None).unwrap();
            for (curv, ao) in [(0.5, 1.0), (0.9, 1.0), (0.1, 0.3)] {
                let s = m.eval(&here(curv, ao, [0.0, -1.0, 0.0]));
                assert!(s.colour.iter().all(|c| c.is_finite() && *c >= 0.0 && *c <= 4.0), "{name}: {:?}", s.colour);
                assert!((0.0..=1.0).contains(&s.metallic), "{name}");
                assert!((0.0..=1.0).contains(&s.roughness), "{name}");
            }
            let json = serde_json::to_string(&m).unwrap();
            let back: MaterialDef = serde_json::from_str(&json).unwrap();
            assert_eq!(back, m, "{name} round-trips");
        }
        assert!(smart("unobtainium", 1.0, None).is_none());
    }

    #[test]
    fn worn_paint_shows_metal_on_edges_and_dirt_in_crevices() {
        let m = smart("painted_metal", 500.0, None).unwrap();
        let flat = m.eval(&here(0.5, 1.0, [1.0, 0.0, 0.0]));
        let edge = m.eval(&here(0.95, 1.0, [1.0, 0.0, 0.0]));
        let crevice = m.eval(&here(0.05, 0.2, [1.0, 0.0, 0.0]));
        assert!(edge.metallic > flat.metallic + 0.2, "{flat:?} vs {edge:?}");
        assert!(crevice.roughness > flat.roughness, "{flat:?} vs {crevice:?}");
        let lum = |s: &Sample| s.colour[0] + s.colour[1] + s.colour[2];
        assert!(lum(&crevice) < lum(&flat) * 1.1);
    }
}
