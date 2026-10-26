// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A model's report card: every critic's measures, a 0–100 score per
//! area, and the bars a production-ready model clears. The bars are set
//! above what the best dedicated generators ship (dense triangle soup or
//! evenly spread quads, many-island atlases with uneven detail, light
//! baked into colour): Studio's results must clear every one.

use crate::{pbr, topo, uv, views};
use serde::Serialize;
use studio_mesh::Mesh;

/// What a model is for, which sets its face budget.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Class {
    Prop,
    Character,
    Creature,
    Vehicle,
    Architecture,
    Environment,
    Product,
}

impl Class {
    /// Quads (two triangles each) a hero asset of this kind gets.
    pub fn budget(self) -> usize {
        match self {
            Class::Prop | Class::Product => 6_000,
            Class::Character | Class::Creature => 12_000,
            Class::Vehicle => 10_000,
            Class::Architecture => 8_000,
            Class::Environment => 16_000,
        }
    }

    pub fn symmetric(self) -> bool {
        matches!(self, Class::Character | Class::Creature | Class::Vehicle)
    }

    pub fn parse(s: &str) -> Option<Class> {
        serde_json::from_value(serde_json::Value::String(s.to_owned())).ok()
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Bar {
    pub name: &'static str,
    pub value: f32,
    pub bar: f32,
    /// true: the value must be at least `bar`; false: at most.
    pub at_least: bool,
    pub pass: bool,
}

fn bar(name: &'static str, value: f32, at_least: bool, b: f32) -> Bar {
    let pass = if at_least { value >= b } else { value <= b };
    Bar { name, value, bar: b, at_least, pass }
}

#[derive(Clone, Debug, Serialize)]
pub struct Scores {
    pub topology: f32,
    pub uv: f32,
    pub materials: f32,
    pub fidelity: f32,
    pub efficiency: f32,
    pub overall: f32,
}

#[derive(Clone, Debug, Serialize)]
pub struct ModelReport {
    pub topology: topo::Topology,
    pub uv: uv::UvQuality,
    pub materials: pbr::Materials,
    pub fidelity: Option<views::Fidelity>,
    pub symmetry: f32,
    pub quads: usize,
    pub budget: usize,
    pub scores: Scores,
    pub bars: Vec<Bar>,
    /// Bars missed, in words.
    pub faults: Vec<String>,
}

impl ModelReport {
    pub fn passes(&self) -> bool {
        self.bars.iter().all(|b| b.pass)
    }

    /// One line for the AI and the Inspector.
    pub fn summary(&self) -> String {
        let s = &self.scores;
        let mut out = format!(
            "quality {:.0}/100 (topology {:.0}, UVs {:.0}, materials {:.0}, fidelity {:.0}, budget {:.0}); {} quads of {}, {}{}, {} UV island{}",
            s.overall,
            s.topology,
            s.uv,
            s.materials,
            s.fidelity,
            s.efficiency,
            self.quads,
            self.budget,
            if self.topology.watertight { "closed" } else { "has holes" },
            if self.topology.manifold { ", manifold" } else { ", not manifold" },
            self.uv.islands,
            if self.uv.islands == 1 { "" } else { "s" }
        );
        if self.faults.is_empty() {
            out.push_str("; clears every production bar");
        } else {
            out.push_str(&format!("; misses: {}", self.faults.join("; ")));
        }
        out
    }
}

fn lin(v: f32, bad: f32, good: f32) -> f32 {
    (((v - bad) / (good - bad)).clamp(0.0, 1.0)) * 100.0
}

/// Judges `m` as a `class` model, against `reference` when there is one.
pub fn critique_model(m: &Mesh, class: Class, reference: Option<&Mesh>) -> ModelReport {
    let t = topo::analyse(m);
    let u = uv::analyse(m);
    let p = pbr::analyse(m);
    let f = reference.map(|r| views::fidelity(m, r));
    let symmetry = if class.symmetric() { views::symmetry(m) } else { 1.0 };
    let quads = ((t.quad_ratio * t.triangles as f32) / 2.0).round() as usize + ((1.0 - t.quad_ratio) * t.triangles as f32).round() as usize;
    let budget = class.budget();
    let topology = {
        let closed = if t.watertight { 25.0 } else { 25.0 * (1.0 - (t.boundary_edges as f32 / 50.0).min(1.0)) * 0.5 };
        let manifold = if t.manifold { 20.0 } else { 0.0 };
        let clean = 10.0 * (1.0 - (t.self_intersections as f32 / 20.0).min(1.0)) * if t.specks == 0 && t.degenerate == 0 { 1.0 } else { 0.5 };
        closed + manifold + clean + 0.2 * lin(t.quad_ratio, 0.0, 0.98) + 0.15 * lin(t.pole_ratio, 0.4, 0.05) + 0.10 * lin(t.loop_score, 0.0, 0.5)
    };
    let uv_score = if m.uvs.len() != m.positions.len() || u.collapsed > 0.5 {
        0.0
    } else {
        let islands = lin((u.islands as f32).ln_1p(), (400.0f32).ln_1p(), (8.0f32).ln_1p());
        0.25 * lin(u.stretch, 2.0, 1.05) + 0.15 * lin(u.conformal, 3.0, 1.1) + 0.2 * lin(u.overlap, 0.3, 0.0) + 0.15 * lin(u.packing, 0.1, 0.65) + 0.15 * lin(u.density_cov, 1.0, 0.1) + 0.10 * islands - 30.0 * u.flipped - 30.0 * u.outside
    }
    .max(0.0);
    let materials = 0.3 * lin(p.albedo_out_of_range, 0.5, 0.0) + 0.2 * lin(p.half_metal, 0.5, 0.0) + 0.3 * lin(p.baked_light, 0.6, 0.1) + if p.named { 20.0 } else { 10.0 };
    let fidelity = match &f {
        Some(f) => 0.3 * lin(f.silhouette_iou, 0.7, 0.98) + 0.15 * lin(f.worst_view_iou, 0.6, 0.95) + 0.25 * lin(f.chamfer, 0.02, 0.002) + 0.1 * lin(f.hausdorff, 0.06, 0.008) + 0.2 * lin(f.torn, 0.15, 0.005),
        None => 0.0,
    } * if class.symmetric() { 0.85 + 0.15 * (lin(symmetry, 0.85, 0.99) / 100.0) } else { 1.0 };
    let ratio = quads as f32 / budget as f32;
    let efficiency = if ratio <= 1.0 { lin(ratio, 0.02, 0.4).min(100.0) } else { lin(ratio, 4.0, 1.0) };
    let overall = if f.is_some() { 0.3 * topology + 0.2 * uv_score + 0.15 * materials + 0.25 * fidelity + 0.1 * efficiency } else { (0.3 * topology + 0.2 * uv_score + 0.15 * materials + 0.1 * efficiency) / 0.75 };
    let mut bars = vec![
        bar("watertight", if t.watertight { 1.0 } else { 0.0 }, true, 1.0),
        bar("manifold", if t.manifold { 1.0 } else { 0.0 }, true, 1.0),
        bar("self-intersections", t.self_intersections as f32, false, 0.0),
        bar("quad ratio", t.quad_ratio, true, 0.95),
        bar("pole ratio", t.pole_ratio, false, 0.15),
        bar("faces over budget", ratio, false, 1.0),
        bar("UV stretch", u.stretch, false, 1.15),
        bar("UV overlap", u.overlap, false, 0.01),
        bar("UV packing", u.packing, true, 0.5),
        bar("texture detail spread", u.density_cov, false, 0.3),
        bar("UV islands", u.islands as f32, false, (budget as f32 / 150.0).max(24.0)),
        bar("albedo out of range", p.albedo_out_of_range, false, 0.02),
        bar("light baked into colour", p.baked_light, false, 0.3),
    ];
    if let Some(f) = &f {
        bars.push(bar("silhouette overlap", f.silhouette_iou, true, 0.95));
        bars.push(bar("surface error", f.chamfer, false, 0.005));
        bars.push(bar("worst surface error", f.hausdorff, false, 0.02));
        bars.push(bar("torn or rippled surface", f.torn, false, 0.03));
    }
    if class.symmetric() {
        bars.push(bar("symmetry", symmetry, true, 0.97));
    }
    let faults = bars
        .iter()
        .filter(|b| !b.pass)
        .map(|b| match b.name {
            "watertight" => format!("{} open edges (holes)", t.boundary_edges),
            "manifold" => format!("{} non-manifold edges, {} vertices, {} flipped", t.nonmanifold_edges, t.nonmanifold_vertices, t.flipped_edges),
            n => format!("{n} {:.3} (bar {} {:.3})", b.value, if b.at_least { "≥" } else { "≤" }, b.bar),
        })
        .collect();
    ModelReport { topology: t, uv: u, materials: p, fidelity: f, symmetry, quads, budget, scores: Scores { topology, uv: uv_score, materials, fidelity, efficiency, overall }, bars, faults }
}
