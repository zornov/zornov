// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shapes and motion from mathematics, for the agent: the `math` field of
//! `model` (and of a blended part, and of a `mesh` `add` step), the warp
//! steps of `mesh` (twist, bend, taper, warp, repeat), and `animate` along
//! a `curve` or by a `formula`. The maths lives in `studio_mesh::math` and
//! `studio_vector::curve`; this reads the agent's JSON, says in words what
//! is wrong, and bakes shapes that change with `t` so they play.
//!
//! A `math` is a preset name (`"gyroid"`) or an object naming its kind by
//! one field: `implicit` (f(x, y, z) ≤ 0), `parametric` (`[x, y, z]` of u
//! and v), `tube` (a curve of u, and `radius`), `height` (z of x and y),
//! `lsystem`, `phyllotaxis` or `attractor`; or `preset` with the kind's
//! settings by name. Any other number in it is a setting, bound by name in
//! front of the formula.

use super::models::{field_v3, num, v3};
use super::*;
use serde_json::Map;
use std::sync::Arc;
use studio_mesh::math::{self, Fit, Program, deform, field, growth, surface};
use studio_mesh::sdf::Part;
use studio_mesh::{Mesh, V3};

/// The most frames a moving shape is baked to.
const MAX_FRAMES: usize = 240;
/// The most vertex positions (vertices × frames) one bake keeps.
const MAX_BAKED: usize = 30_000_000;
/// The most keys a curve or formula animation writes.
const MAX_KEYS: usize = 600;

/// A shape that changes as `t` runs: its points at a time (from where
/// they were the frame before), and what a frame costs in evaluations.
/// Points at a time, from the points a frame before.
pub(super) type Follow = Box<dyn Fn(f64, &[V3]) -> Vec<V3>>;

pub(super) struct Moving {
    pub at: Follow,
    pub cost: u64,
}

/// What a `math` made.
pub(super) struct Made {
    pub mesh: Mesh,
    pub name: String,
    pub said: Value,
    pub moving: Option<Moving>,
    /// An implicit solid's recipe: what the renderer needs to draw it
    /// straight from its formula.
    pub recipe: Option<field::Recipe>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Implicit,
    Parametric,
    Tube,
    Height,
    Plant,
    Phyllo,
    Attractor,
}

impl Kind {
    fn word(self) -> &'static str {
        match self {
            Kind::Implicit => "implicit",
            Kind::Parametric => "parametric",
            Kind::Tube => "tube",
            Kind::Height => "height",
            Kind::Plant => "lsystem",
            Kind::Phyllo => "phyllotaxis",
            Kind::Attractor => "attractor",
        }
    }

    /// The fields of this kind (every other number is a setting).
    fn fields(self) -> &'static [&'static str] {
        match self {
            Kind::Implicit => &["implicit", "preset", "bounds", "clip", "weight", "morph", "t", "bake"],
            Kind::Parametric => &["parametric", "preset", "coords", "u", "v", "resolution", "thickness", "cut", "morph", "t", "bake"],
            Kind::Tube => &["tube", "preset", "u", "radius", "sides", "resolution", "t", "bake"],
            Kind::Height => &["height", "preset", "bounds", "resolution", "depth", "t", "bake"],
            Kind::Plant => &["lsystem", "preset", "axiom", "rules", "iterations", "angle", "width", "taper", "shrink", "droop", "seed", "sides", "leaves", "leaf_colour", "radii", "pipe"],
            Kind::Phyllo => &["phyllotaxis", "preset", "count", "angle", "seed_size", "core", "seed_colour", "core_colour"],
            Kind::Attractor => &["attractor", "preset", "steps", "dt", "start", "skip", "radius", "sides", "every"],
        }
    }
}

/// Phyllotaxis by the plant it makes.
fn spread_of(s: &str) -> Option<growth::Spread> {
    growth::Spread::parse(s).or(match s {
        "sunflower_head" | "seeds" => Some(growth::Spread::Disc),
        "dandelion" | "pollen" => Some(growth::Spread::Ball),
        "pine_cone" | "artichoke" => Some(growth::Spread::Cone),
        "cauliflower" | "succulent" => Some(growth::Spread::Dome),
        _ => None,
    })
}

/// Every preset by kind, for messages and the reply to a bad name.
pub(super) fn presets() -> String {
    let names = |v: Vec<&str>| v.join(", ");
    format!(
        "implicit solids: {}; parametric surfaces: {}; tubes: {}; height fields: {}; L-systems: {}; attractors: {}; phyllotaxis: disc (sunflower), ball, cone (pinecone), dome (cactus)",
        names(field::PRESETS.iter().map(|p| p.name).collect()),
        names(surface::PRESETS.iter().map(|p| p.name).collect()),
        names(surface::TUBES.iter().map(|p| p.name).collect()),
        names(surface::HEIGHTS.iter().map(|p| p.name).collect()),
        names(growth::PLANTS.iter().map(|p| p.name).collect()),
        names(growth::ATTRACTORS.iter().map(|p| p.name).collect()),
    )
}

/// The kind a preset name is, if it is one.
fn preset_kind(name: &str) -> Option<Kind> {
    if field::preset(name).is_some() {
        Some(Kind::Implicit)
    } else if surface::preset(name).is_some() {
        Some(Kind::Parametric)
    } else if surface::tube_preset(name).is_some() {
        Some(Kind::Tube)
    } else if surface::height_preset(name).is_some() {
        Some(Kind::Height)
    } else if growth::plant(name).is_some() {
        Some(Kind::Plant)
    } else if growth::attractor(name).is_some() {
        Some(Kind::Attractor)
    } else if matches!(name, "sunflower" | "pinecone" | "cactus" | "dandelion") {
        Some(Kind::Phyllo)
    } else {
        None
    }
}

/// A `math` as an object with its kind (and preset name, if it is one).
/// A `math` as read: its fields, its kind, its preset's name.
type Read = (Map<String, Value>, Kind, Option<String>);

fn read(spec: &Value) -> Result<Read, String> {
    let o = match spec {
        Value::String(s) => {
            let mut o = Map::new();
            o.insert("preset".into(), Value::String(s.clone()));
            o
        }
        Value::Object(o) => o.clone(),
        _ => return Err(format!("math is a preset name or an object ({{\"implicit\": \"x^2 + y^2 + z^2 - 1\"}}, {{\"preset\": \"gyroid\", \"cells\": 3}}…). Presets: {}", presets())),
    };
    let keyed = [("implicit", Kind::Implicit), ("parametric", Kind::Parametric), ("surface", Kind::Parametric), ("tube", Kind::Tube), ("height", Kind::Height), ("lsystem", Kind::Plant), ("phyllotaxis", Kind::Phyllo), ("attractor", Kind::Attractor)];
    let mut o = o;
    if let Some(s) = o.remove("surface") {
        o.insert("parametric".into(), s);
    }
    // A preset named is what it is.
    if let Some(name) = o.get("preset").and_then(Value::as_str).map(str::to_owned)
        && let Some(kind) = preset_kind(&name).or(spread_of(&name).map(|_| Kind::Phyllo))
    {
        return Ok((o, kind, Some(name)));
    }
    for (k, kind) in keyed {
        if let Some(v) = o.get(k) {
            // `{"tube": "torus_knot"}` is the preset by its kind.
            let preset = v.as_str().filter(|s| preset_kind(s) == Some(kind) || (kind == Kind::Phyllo && spread_of(s).is_some())).map(str::to_owned);
            return Ok((o, kind, preset));
        }
    }
    let name = o.get("preset").and_then(Value::as_str).ok_or_else(|| format!("math needs a \"preset\" or one of implicit, parametric, tube, height, lsystem, phyllotaxis, attractor. Presets: {}", presets()))?.to_owned();
    let kind = preset_kind(&name).ok_or_else(|| format!("math preset \"{name}\" is not one; {}", presets()))?;
    Ok((o, kind, Some(name)))
}

/// The settings given in `o` (numbers that are not the kind's fields).
fn settings(o: &Map<String, Value>, kind: Kind) -> Result<Vec<(String, f64)>, String> {
    let mut out = Vec::new();
    for (k, v) in o {
        if kind.fields().contains(&k.as_str()) {
            continue;
        }
        match v {
            Value::Number(n) => out.push((k.to_lowercase(), n.as_f64().unwrap_or(0.0))),
            Value::Bool(b) => out.push((k.to_lowercase(), *b as u8 as f64)),
            _ => return Err(format!("math ({}): \"{k}\" is neither one of its fields ({}) nor a setting (a number)", kind.word(), kind.fields().join(", "))),
        }
    }
    Ok(out)
}

/// Settings bound in front of `body`: a preset's (checked by name) or, for
/// a formula of one's own, whatever numbers were given.
fn bound(what: &str, params: Option<&[(&str, f64)]>, given: &[(String, f64)], body: &str) -> Result<String, String> {
    match params {
        Some(p) => field::bind(what, p, given, body),
        None => {
            let own: Vec<(&str, f64)> = given.iter().map(|(k, v)| (k.as_str(), *v)).collect();
            field::bind(what, &own, given, body)
        }
    }
}

/// A number or a formula of the settings (`"tau"`, `"turns * 2 * pi"`).
fn constant(v: &Value, params: Option<&[(&str, f64)]>, given: &[(String, f64)]) -> Result<f64, String> {
    match v {
        Value::Number(n) => Ok(n.as_f64().unwrap_or(0.0)),
        Value::String(s) => Ok(math::compile("range", &bound("range", params, given, s)?, &[])?.eval(&[])),
        other => Err(format!("{other} is not a number or a formula")),
    }
}

fn range(v: Option<&Value>, default: [&str; 2], params: Option<&[(&str, f64)]>, given: &[(String, f64)]) -> Result<[f64; 2], String> {
    let pair = |a: &Value, b: &Value| -> Result<[f64; 2], String> { Ok([constant(a, params, given)?, constant(b, params, given)?]) };
    match v {
        Some(Value::Array(a)) if a.len() == 2 => pair(&a[0], &a[1]),
        Some(other) => Err(format!("a range is [from, to] (numbers or formulas such as \"tau\"), not {other}")),
        None => pair(&Value::from(default[0]), &Value::from(default[1])),
    }
}

/// One formula or a list of them.
fn formulas(v: &Value, what: &str) -> Result<Vec<String>, String> {
    match v {
        Value::String(s) => Ok(vec![s.clone()]),
        Value::Array(a) if !a.is_empty() && a.iter().all(Value::is_string) => Ok(a.iter().filter_map(Value::as_str).map(str::to_owned).collect()),
        Value::Object(o) if o.contains_key("x") => ["x", "y", "z"].iter().map(|k| o.get(*k).and_then(Value::as_str).map(str::to_owned).ok_or_else(|| format!("{what} {{\"x\", \"y\", \"z\"}} needs all three formulas"))).collect(),
        _ => Err(format!("{what} is a formula, or a list of formulas")),
    }
}

/// Where a sheet's samples go in Studio's world: centred, scaled to `fit`.
fn placer(lo: [f64; 3], hi: [f64; 3], fit: Fit) -> (impl Fn([f64; 3]) -> V3 + Copy, f32) {
    let c = [(lo[0] + hi[0]) * 0.5, (lo[1] + hi[1]) * 0.5, (lo[2] + hi[2]) * 0.5];
    let e = math::to_studio([hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]]).map(f32::abs);
    let k = fit.scales(e);
    let place = move |p: [f64; 3]| {
        let q = math::to_studio([p[0] - c[0], p[1] - c[1], p[2] - c[2]]);
        [q[0] * k[0], q[1] * k[1], q[2] * k[2]]
    };
    (place, k[0].min(k[1]).min(k[2]))
}

fn titled(s: &str) -> String {
    let mut n = s.replace('_', " ");
    if let Some(c) = n.get_mut(0..1) {
        c.make_ascii_uppercase();
    }
    n
}

/// How fine to sample at `detail` 1 to 6.
fn fineness(detail: usize, table: [usize; 6]) -> usize {
    table[detail.clamp(1, 6) - 1]
}

/// Builds what `spec` describes, `fit` its size, at `detail` (1 to 6).
pub(super) fn make(spec: &Value, fit: Fit, detail: usize) -> Result<Made, String> {
    let (o, kind, preset) = read(spec)?;
    let given = settings(&o, kind)?;
    match kind {
        Kind::Implicit => implicit(&o, preset.as_deref(), &given, fit, detail),
        Kind::Parametric => parametric(&o, preset.as_deref(), &given, fit, detail),
        Kind::Tube => tube(&o, preset.as_deref(), &given, fit, detail),
        Kind::Height => height(&o, preset.as_deref(), &given, fit, detail),
        Kind::Plant => plant(&o, preset.as_deref(), &given, fit, detail),
        Kind::Phyllo => phyllo(&o, preset.as_deref(), &given, fit, detail),
        Kind::Attractor => attractor(&o, preset.as_deref(), &given, fit, detail),
    }
}

/// An implicit `math` as a field, its domain's sides (formula units, Studio
/// axes) and the formulas as bound.
pub(super) fn field_of(spec: &Value) -> Result<(Arc<field::Field>, V3, Vec<String>), String> {
    recipe_of(spec).map(|(f, s, src, _)| (f, s, src))
}

/// [`field_of`] with the field's recipe (its size still to be set).
pub(super) fn recipe_of(spec: &Value) -> Result<(Arc<field::Field>, V3, Vec<String>, field::Recipe), String> {
    let (o, kind, preset) = read(spec)?;
    if kind != Kind::Implicit {
        return Err(format!("a blended part's math is an implicit solid (f(x, y, z) ≤ 0, or a preset: {}); a {} shape is a model of its own (add it with mesh {{\"do\": \"add\", \"math\": …}})", field::PRESETS.iter().map(|p| p.name).collect::<Vec<_>>().join(", "), kind.word()));
    }
    let given = settings(&o, kind)?;
    let p = preset.as_deref().and_then(field::preset);
    let mut sources = match (p, o.get("implicit")) {
        (Some(p), _) => vec![bound(p.name, Some(p.params), &given, p.formula)?],
        (None, Some(v)) => formulas(v, "implicit")?.iter().map(|f| bound("implicit", None, &given, f)).collect::<Result<Vec<_>, _>>()?,
        (None, None) => return Err("math needs \"implicit\": a formula f(x, y, z), solid where it is 0 or less".into()),
    };
    // Morphing to another solid as t runs.
    let mut weight = o.get("weight").and_then(Value::as_str).map(str::to_owned);
    if let Some(m) = o.get("morph") {
        let to = m.get("to").ok_or("morph is {\"to\": another implicit math or preset, \"over\": [from, to] seconds}")?;
        let (to_o, to_kind, to_preset) = read(to)?;
        if to_kind != Kind::Implicit {
            return Err("an implicit solid morphs to another implicit solid".into());
        }
        let to_given = settings(&to_o, Kind::Implicit)?;
        let src = match to_preset.as_deref().and_then(field::preset) {
            Some(tp) => bound(tp.name, Some(tp.params), &to_given, tp.formula)?,
            None => bound("implicit", None, &to_given, &formulas(to_o.get("implicit").ok_or("morph \"to\" needs an implicit formula or preset")?, "implicit")?[0])?,
        };
        sources.push(src);
        let over = m.get("over").and_then(Value::as_array).map(|a| (a.first().and_then(Value::as_f64).unwrap_or(0.0), a.get(1).and_then(Value::as_f64).unwrap_or(2.0))).unwrap_or((0.0, 2.0));
        weight = Some(m.get("weight").and_then(Value::as_str).map(str::to_owned).unwrap_or(format!("smoothstep({:?}, {:?}, t)", over.0, over.1)));
    }
    if sources.len() > 1 && weight.is_none() {
        // A list of formulas with no weight: one after another, a second each.
        weight = Some("t".into());
    }
    let progs = sources.iter().map(|s| math::compile("implicit", s, field::NAMES)).collect::<Result<Vec<_>, _>>()?;
    let weight_src = weight.clone();
    let weight = weight.map(|w| math::compile("weight", &w, field::NAMES)).transpose()?;
    let half = match o.get("bounds") {
        None => [p.map_or(1.0, |p| p.bounds); 3],
        Some(Value::Number(n)) => [n.as_f64().unwrap_or(1.0).abs(); 3],
        Some(v) => v3(v).map(|s| s.map(|x| x.abs() as f64)).ok_or("bounds is a number (half the domain's side) or [x, y, z] half sides, in the formula's units")?,
    };
    if half.iter().any(|h| h.is_nan() || *h <= 0.0) {
        return Err("bounds must be more than 0".into());
    }
    let clip = match o.get("clip").and_then(Value::as_str) {
        None => p.map_or(field::Clip::Box, |p| p.clip),
        Some(c) => field::Clip::parse(c).ok_or("clip is box (the solid is cut to its domain) or ball (to the ball inside it)")?,
    };
    let t = o.get("t").and_then(Value::as_f64).unwrap_or(0.0);
    let f = field::Field::new(progs, weight, half.map(|h| -h), half, clip, t)?;
    let sides = math::to_studio([half[0] * 2.0, half[1] * 2.0, half[2] * 2.0]).map(f32::abs);
    let recipe = field::Recipe { formulas: sources.clone(), weight: weight_src, lo: half.map(|h| -h), hi: half, clip, size: sides };
    Ok((Arc::new(f), sides, sources, recipe))
}

/// A field part's size in px from a fit and its domain's sides.
pub(super) fn field_size(sides: V3, fit: Fit) -> V3 {
    let k = fit.scales(sides);
    [sides[0] * k[0], sides[1] * k[1], sides[2] * k[2]]
}

/// Parts with fields that move: the same triangles following the surface.
pub(super) fn moving_parts(parts: Vec<Part>, fields: Vec<(usize, Arc<field::Field>)>, cell: f32, verts: usize) -> Option<Moving> {
    if !fields.iter().any(|(_, f)| f.moves()) {
        return None;
    }
    let cost = fields.iter().map(|(_, f)| f.cost()).sum::<u64>() * 4 * 3 * verts as u64;
    Some(Moving {
        at: Box::new(move |t, prev| {
            let mut now = parts.clone();
            for (k, f) in &fields {
                let p = &parts[*k];
                now[*k] = Part { field: Some(studio_mesh::sdf::FieldRef(Arc::new(f.at_time(t)))), ..p.clone() };
            }
            let sample = studio_mesh::sdf::sampler(&now);
            field::track(prev, &sample, cell * 1.5, 3)
        }),
        cost,
    })
}

fn implicit(o: &Map<String, Value>, preset: Option<&str>, _given: &[(String, f64)], fit: Fit, detail: usize) -> Result<Made, String> {
    let (f, sides, sources, mut recipe) = recipe_of(&Value::Object(o.clone()))?;
    let size = field_size(sides, fit);
    recipe.size = size;
    let cell = field::cell_for(size, detail, f.cost());
    let parts = vec![Part::field(f.clone(), [0.0; 3], size)];
    let (mesh, cost) = studio_mesh::sdf::build_counted(&parts, vec![studio_mesh::Material::default()], cell);
    if mesh.triangles() == 0 {
        return Err(format!("the formula has no surface in its domain (bounds ±{:.3}): it is above 0 everywhere, or below 0 everywhere (then the cut to the box is all there is); check its sign (solid where it is 0 or less) and its scale", sides[0] / 2.0));
    }
    let (holes, _) = math::open_edges(&mesh);
    let moving = moving_parts(parts, vec![(0, f.clone())], cost.cell, mesh.positions.len());
    let said = json!({
        "kind": "implicit", "preset": preset, "formula": sources.first(), "morph_to": sources.get(1),
        "evals": cost.samples * f.cost(), "grid": cost.grid, "cell_px": (cost.cell * 100.0).round() / 100.0,
        "closed": holes == 0, "moves": f.moves(),
    });
    Ok(Made { mesh, name: titled(preset.unwrap_or("Implicit shape")), said, moving, recipe: Some(recipe) })
}

/// A surface's formulas: one `[x, y, z]` or three.
fn surface_formulas(v: &Value, coords: surface::Coords, params: Option<&[(&str, f64)]>, given: &[(String, f64)]) -> Result<surface::Formulas, String> {
    let list = formulas(v, "parametric")?;
    if list.len() != 1 && list.len() != 3 {
        return Err("parametric is one formula giving [x, y, z] (of u, v and t) or three formulas".into());
    }
    let parts = list.iter().map(|s| math::compile("parametric", &bound("parametric", params, given, s)?, surface::NAMES)).collect::<Result<Vec<_>, _>>()?;
    Ok(surface::Formulas { parts, coords })
}

struct SurfaceSpec {
    f: surface::Formulas,
    u: [f64; 2],
    v: [f64; 2],
    source: String,
    thickness: Option<f64>,
}

fn surface_spec(o: &Map<String, Value>, preset: Option<&str>, given: &[(String, f64)]) -> Result<SurfaceSpec, String> {
    let p = preset.and_then(surface::preset);
    let coords = match o.get("coords").and_then(Value::as_str) {
        Some(c) => surface::Coords::parse(c).ok_or("coords is cartesian (x, y, z), cylindrical (r, theta, z) or spherical (r, theta, phi)")?,
        None => p.map_or(surface::Coords::Cartesian, |p| p.coords),
    };
    let params = p.map(|p| p.params);
    let (f, source) = match p {
        Some(p) => (surface_formulas(&Value::from(p.formula), coords, params, given)?, p.formula.to_owned()),
        None => {
            let v = o.get("parametric").ok_or("a parametric surface needs \"parametric\": \"[x, y, z]\" of u and v (or a preset)")?;
            (surface_formulas(v, coords, None, given)?, v.to_string())
        }
    };
    let u = range(o.get("u"), p.map_or(["0", "tau"], |p| p.u), params, given)?;
    let v = range(o.get("v"), p.map_or(["0", "tau"], |p| p.v), params, given)?;
    Ok(SurfaceSpec { f, u, v, source, thickness: p.map(|p| p.thickness) })
}

fn parametric(o: &Map<String, Value>, preset: Option<&str>, given: &[(String, f64)], fit: Fit, detail: usize) -> Result<Made, String> {
    let a = surface_spec(o, preset, given)?;
    // Morphing to another surface: both over their own ranges, mixed.
    let b = match o.get("morph") {
        Some(m) => {
            let to = m.get("to").ok_or("morph is {\"to\": another parametric math or preset, \"over\": [from, to] seconds}")?;
            let (to_o, kind, to_preset) = read(to)?;
            if kind != Kind::Parametric {
                return Err("a parametric surface morphs to another parametric surface".into());
            }
            let over = m.get("over").and_then(Value::as_array).map(|a| [a.first().and_then(Value::as_f64).unwrap_or(0.0), a.get(1).and_then(Value::as_f64).unwrap_or(2.0)]).unwrap_or([0.0, 2.0]);
            Some((surface_spec(&to_o, to_preset.as_deref(), &settings(&to_o, Kind::Parametric)?)?, over))
        }
        None => None,
    };
    let base = fineness(detail, [40, 64, 96, 140, 200, 280]);
    let (nu, nv) = match o.get("resolution") {
        Some(Value::Number(n)) => {
            let n = n.as_f64().unwrap_or(96.0) as usize;
            (n, n)
        }
        Some(Value::Array(r)) if r.len() == 2 => (r[0].as_f64().unwrap_or(96.0) as usize, r[1].as_f64().unwrap_or(96.0) as usize),
        Some(_) => return Err("resolution is a number of samples along u and v, or [along u, along v]".into()),
        None => {
            // Long ranges (a shell's turns) get more samples along them.
            let turns = ((a.u[1] - a.u[0]).abs() / std::f64::consts::TAU).clamp(1.0, 4.0);
            ((base as f64 * turns.sqrt()) as usize, base)
        }
    };
    let (nu, nv) = (nu.clamp(4, 2048), nv.clamp(4, 2048));
    let mut notes = Vec::new();
    let (nu, nv) = if nu * nv > surface::MAX_SAMPLES {
        let k = (surface::MAX_SAMPLES as f64 / (nu * nv) as f64).sqrt();
        notes.push(format!("resolution lowered to stay under {} samples", surface::MAX_SAMPLES));
        (((nu as f64 * k) as usize).max(4), ((nv as f64 * k) as usize).max(4))
    } else {
        (nu, nv)
    };
    let t0 = o.get("t").and_then(Value::as_f64).unwrap_or(0.0);
    let moves = b.is_some() || a.f.moves();
    let (au, av, source, preset_thickness) = (a.u, a.v, a.source.clone(), a.thickness);
    let point = {
        let (a, b) = (Arc::new(a), b.map(|(s, over)| (Arc::new(s), over)));
        move |fu: f64, fv: f64, t: f64| -> [f64; 3] {
            let pa = a.f.point(a.u[0] + (a.u[1] - a.u[0]) * fu, a.v[0] + (a.v[1] - a.v[0]) * fv, t);
            match &b {
                Some((b, over)) => {
                    let w = ((t - over[0]) / (over[1] - over[0]).max(1e-9)).clamp(0.0, 1.0);
                    let w = w * w * (3.0 - 2.0 * w);
                    let pb = b.f.point(b.u[0] + (b.u[1] - b.u[0]) * fu, b.v[0] + (b.v[1] - b.v[0]) * fv, t);
                    [0, 1, 2].map(|i| pa[i] + (pb[i] - pa[i]) * w)
                }
                None => pa,
            }
        }
    };
    let point = Arc::new(point);
    let p0 = point.clone();
    let mut sheet = surface::Sheet::sample(&move |fu, fv| p0(fu, fv, t0), [0.0, 1.0], [0.0, 1.0], nu, nv);
    let cut = match o.get("cut").and_then(Value::as_str) {
        Some(c) => {
            let prog = math::compile("cut", &bound("cut", preset.and_then(surface::preset).map(|p| p.params), given, c)?, surface::CUT_NAMES)?;
            let n = sheet.cut(&prog, au, av, t0);
            if n == sheet.keep.len() {
                return Err(format!("cut \"{c}\" removes every cell (it is true everywhere; holes go where it is true)"));
            }
            Some((c.to_owned(), n))
        }
        None => None,
    };
    let (lo, hi) = sheet.bounds();
    if !(0..3).any(|i| hi[i] - lo[i] > 1e-12) {
        return Err("the surface's points are all in one place (or undefined): check its formulas and the ranges of u and v".into());
    }
    let (place, _) = placer(lo, hi, fit);
    let size_px = match fit {
        Fit::Largest(s) => s,
        Fit::Sides(s) => s[0].max(s[1]).max(s[2]),
    };
    let given_thick = match o.get("thickness") {
        Some(v) => Some(v.as_f64().ok_or("thickness is px (0: a sheet with no thickness)")? as f32),
        None => None,
    };
    let mut thick = given_thick.unwrap_or_else(|| preset_thickness.map_or(0.0, |t| t as f32 * size_px));
    let mut built = surface::build(&sheet, &place, thick, None);
    // An open sheet becomes a thin shell, a solid, unless told not to.
    if !built.closed && given_thick.is_none() && thick == 0.0 {
        thick = size_px * 0.015;
        built = surface::build(&sheet, &place, thick, None);
        notes.push(format!("an open sheet, so made a shell {thick:.1} px thick (\"thickness\": 0 keeps it a sheet)"));
    }
    let evals = (nu * nv) as u64 * 3 + cut.as_ref().map_or(0, |_| ((nu - 1) * (nv - 1)) as u64);
    let moving = moves.then(|| {
        let built = Arc::new(surface::build(&sheet, &place, thick, None));
        let point = point.clone();
        let keep = sheet.keep.clone();
        Moving {
            at: Box::new(move |t, _| {
                let mut s = surface::Sheet::sample(&|fu, fv| point(fu, fv, t), [0.0, 1.0], [0.0, 1.0], nu, nv);
                s.keep = keep.clone();
                built.follow(&s, &place, None)
            }),
            cost: (nu * nv) as u64 * 3,
        }
    });
    let mesh = built.mesh;
    let said = json!({
        "kind": "parametric", "preset": preset, "formula": source, "u": au, "v": av, "grid": [nu, nv],
        "evals": evals, "thickness_px": (thick * 10.0).round() / 10.0, "cut": cut.as_ref().map(|c| json!({"formula": c.0, "cells_removed": c.1})),
        "closed": built.closed, "moves": moves, "notes": notes,
    });
    Ok(Made { mesh, name: titled(preset.unwrap_or("Surface")), said, moving, recipe: None })
}

fn tube(o: &Map<String, Value>, preset: Option<&str>, given: &[(String, f64)], fit: Fit, detail: usize) -> Result<Made, String> {
    let p = preset.and_then(surface::tube_preset);
    let params = p.map(|p| p.params);
    let src = match p {
        Some(p) => vec![p.formula.to_owned()],
        None => formulas(o.get("tube").ok_or("a tube needs \"tube\": \"[x, y, z]\" of u (a curve) or a preset")?, "tube")?,
    };
    if src.len() != 1 && src.len() != 3 {
        return Err("tube is one formula giving [x, y, z] of u, or three".into());
    }
    let progs = src.iter().map(|s| math::compile("tube", &bound("tube", params, given, s)?, surface::NAMES)).collect::<Result<Vec<_>, _>>()?;
    let f = surface::Formulas { parts: progs, coords: surface::Coords::Cartesian };
    let u = range(o.get("u"), p.map_or(["0", "tau"], |p| p.u), params, given)?;
    let n = o.get("resolution").and_then(Value::as_f64).map_or(fineness(detail, [100, 160, 240, 360, 500, 700]), |r| r as usize).clamp(8, 8000);
    let sides = o.get("sides").and_then(Value::as_f64).map_or(fineness(detail, [6, 8, 10, 14, 18, 24]), |s| s as usize).clamp(3, 64);
    let radius_prog = match o.get("radius") {
        Some(Value::String(s)) => Some(math::compile("radius", &bound("radius", params, given, s)?, surface::NAMES)?),
        _ => None,
    };
    let radius_num = o.get("radius").and_then(Value::as_f64);
    let t0 = o.get("t").and_then(Value::as_f64).unwrap_or(0.0);
    let curve_at = |t: f64| -> Vec<[f64; 3]> { (0..n).map(|k| f.point(u[0] + (u[1] - u[0]) * k as f64 / (n - 1) as f64, 0.0, t)).collect() };
    let curve = curve_at(t0);
    let (lo, hi) = bounds3(&curve);
    let span = (0..3).map(|i| hi[i] - lo[i]).fold(0.0f64, f64::max);
    if span.is_nan() || span <= 0.0 {
        return Err("the tube's curve stays in one place: check its formula and the range of u".into());
    }
    let r_default = radius_num.unwrap_or(p.map_or(span * 0.04, |p| p.radius));
    let radii = |t: f64| -> Vec<f64> {
        match &radius_prog {
            Some(r) => (0..n).map(|k| r.eval(&[u[0] + (u[1] - u[0]) * k as f64 / (n - 1) as f64, 0.0, t, 0.0])).collect(),
            None => vec![r_default],
        }
    };
    let sheet = surface::tube(&curve, &radii(t0), sides);
    let (slo, shi) = sheet.bounds();
    let (place, _) = placer(slo, shi, fit);
    let built = surface::build(&sheet, &place, 0.0, None);
    let moves = f.moves() || radius_prog.as_ref().is_some_and(|r| r.uses(2));
    let said = json!({"kind": "tube", "preset": preset, "formula": src, "u": u, "points": n, "sides": sides, "evals": n as u64 * 3, "closed": built.closed, "moves": moves});
    let moving = if moves {
        let built = Arc::new(surface::build(&sheet, &place, 0.0, None));
        let len = sheet.points.len();
        let f = Arc::new(f);
        let radius_prog = radius_prog.clone();
        Some(Moving {
            at: Box::new(move |t, prev| {
                let curve: Vec<[f64; 3]> = (0..n).map(|k| f.point(u[0] + (u[1] - u[0]) * k as f64 / (n - 1) as f64, 0.0, t)).collect();
                let r: Vec<f64> = match &radius_prog {
                    Some(r) => (0..n).map(|k| r.eval(&[u[0] + (u[1] - u[0]) * k as f64 / (n - 1) as f64, 0.0, t, 0.0])).collect(),
                    None => vec![r_default],
                };
                let s = surface::tube(&curve, &r, sides);
                // A curve that opens or closes changes its rings: hold.
                if s.points.len() != len { prev.to_vec() } else { built.follow(&s, &place, None) }
            }),
            cost: n as u64 * 3,
        })
    } else {
        None
    };
    Ok(Made { mesh: built.mesh, name: titled(preset.unwrap_or("Tube")), said, moving, recipe: None })
}

/// Sides round a tube `radius_px` thick on screen: about one every 4 px of
/// its rim, 5 for a twig up to 24 for a trunk.
fn tube_sides(radius_px: f64) -> usize {
    ((std::f64::consts::TAU * radius_px / 4.0).ceil() as usize).clamp(5, 24)
}

fn bounds3(p: &[[f64; 3]]) -> ([f64; 3], [f64; 3]) {
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for q in p {
        for i in 0..3 {
            lo[i] = lo[i].min(q[i]);
            hi[i] = hi[i].max(q[i]);
        }
    }
    (lo, hi)
}

fn height(o: &Map<String, Value>, preset: Option<&str>, given: &[(String, f64)], fit: Fit, detail: usize) -> Result<Made, String> {
    let p = preset.and_then(surface::height_preset);
    let params = p.map(|p| p.params);
    let src = match p {
        Some(p) => p.formula.to_owned(),
        None => o.get("height").and_then(Value::as_str).ok_or("a height field needs \"height\": a formula of x and y (the height up), or a preset")?.to_owned(),
    };
    let prog = Arc::new(math::compile("height", &bound("height", params, given, &src)?, surface::HEIGHT_NAMES)?);
    let half = match o.get("bounds") {
        None => [1.0, 1.0],
        Some(Value::Number(n)) => [n.as_f64().unwrap_or(1.0).abs(); 2],
        Some(Value::Array(a)) if a.len() == 2 => [a[0].as_f64().unwrap_or(1.0).abs(), a[1].as_f64().unwrap_or(1.0).abs()],
        Some(_) => return Err("bounds is a number (half the side) or [x, y] half sides".into()),
    };
    let n = match o.get("resolution") {
        Some(v) => v.as_f64().ok_or("resolution is samples along a side")? as usize,
        None => fineness(detail, [48, 72, 110, 160, 240, 360]),
    }
    .clamp(4, 512);
    let (nx, ny) = if half[0] >= half[1] { (n, ((n as f64 * half[1] / half[0]) as usize).max(4)) } else { (((n as f64 * half[0] / half[1]) as usize).max(4), n) };
    let t0 = o.get("t").and_then(Value::as_f64).unwrap_or(0.0);
    let h = |prog: &Program, x: f64, y: f64, t: f64| {
        let z = prog.eval(&[x, y, t, x.hypot(y)]);
        if z.is_finite() { z } else { 0.0 }
    };
    let pr = prog.clone();
    let (sheet, _) = surface::height(&move |x, y| h(&pr, x, y, t0), [-half[0], -half[1]], [half[0], half[1]], nx, ny, 1e-9);
    let (lo, hi) = sheet.bounds();
    let size_px = match fit {
        Fit::Largest(s) => s,
        Fit::Sides(s) => s[0].max(s[2]),
    };
    // Scale by the ground's sides, not the hills, so heights keep their
    // proportion; the floor `depth` px under the lowest point.
    let (place, k) = placer([lo[0], lo[1], 0.0], [hi[0], hi[1], 0.0], match fit {
        Fit::Largest(s) => Fit::Largest(s),
        Fit::Sides(s) => Fit::Sides([s[0], 1.0, s[2]]),
    });
    let place = move |q: [f64; 3]| {
        let mut p = place([q[0], q[1], 0.0]);
        p[1] = -(q[2] as f32) * k;
        p
    };
    let depth = o.get("depth").and_then(Value::as_f64).map_or(size_px * 0.04, |d| d as f32).max(0.5);
    let floor_z = lo[2] as f32 * k - depth;
    let floor: Vec<V3> = sheet.points.iter().map(|q| {
        let mut p = place(*q);
        p[1] = -floor_z;
        p
    }).collect();
    let built = surface::build(&sheet, &place, 0.0, Some(&floor));
    let moves = prog.uses(2);
    let said = json!({"kind": "height", "preset": preset, "formula": src, "grid": [nx, ny], "evals": (nx * ny) as u64, "closed": built.closed, "moves": moves, "heights": [lo[2], hi[2]]});
    let moving = moves.then(|| {
        let built = Arc::new(surface::build(&sheet, &place, 0.0, Some(&floor)));
        let floor = floor.clone();
        Moving {
            at: Box::new(move |t, _| {
                let pr = prog.clone();
                let (s, _) = surface::height(&move |x, y| h(&pr, x, y, t), [-half[0], -half[1]], [half[0], half[1]], nx, ny, 1e-9);
                built.follow(&s, &place, Some(&floor))
            }),
            cost: (nx * ny) as u64,
        }
    });
    Ok(Made { mesh: built.mesh, name: titled(preset.unwrap_or("Height field")), said, moving, recipe: None })
}

fn material(name: &str, colour: [u8; 3], roughness: f32) -> studio_mesh::Material {
    let l = studio_mesh::srgb_to_linear;
    studio_mesh::Material { name: name.into(), base_colour: [l(colour[0]), l(colour[1]), l(colour[2]), 1.0], roughness, metallic: 0.0, ..Default::default() }
}

/// A material's colour from an op field, if given.
fn colour_field(o: &Map<String, Value>, k: &str) -> Result<Option<[f32; 4]>, String> {
    o.get(k).map(super::models::linear_colour).transpose()
}

fn plant(o: &Map<String, Value>, preset: Option<&str>, given: &[(String, f64)], fit: Fit, _detail: usize) -> Result<Made, String> {
    if let Some((k, _)) = given.first() {
        return Err(format!("an L-system has no setting \"{k}\"; its fields are {}", Kind::Plant.fields().join(", ")));
    }
    let base = preset.and_then(growth::plant);
    let spec = o.get("lsystem").filter(|v| v.is_object());
    let mut sys = match (base, spec) {
        (_, Some(s)) => growth::System {
            axiom: s.get("axiom").or(o.get("axiom")).and_then(Value::as_str).map(str::to_owned).or(base.map(|b| b.axiom.to_owned())).ok_or("an lsystem needs \"axiom\" (the start, e.g. \"F\")")?,
            rules: Vec::new(),
        },
        (Some(b), None) => b.system(),
        (None, None) => growth::System { axiom: o.get("axiom").and_then(Value::as_str).ok_or("an lsystem needs a preset or \"axiom\" and \"rules\"")?.to_owned(), rules: Vec::new() },
    };
    if let Some(r) = spec.and_then(|s| s.get("rules")).or(o.get("rules")) {
        let r = r.as_object().ok_or("rules is {\"F\": \"F[+F]F[-F]F\", …}: a symbol to what it becomes (or to a list of choices, picked at random)")?;
        for (k, v) in r {
            let mut cs = k.chars();
            let (Some(c), None) = (cs.next(), cs.next()) else { return Err(format!("a rule's key is one symbol, not \"{k}\"")) };
            let choices: Vec<String> = match v {
                Value::String(s) => vec![s.clone()],
                Value::Array(a) => a.iter().filter_map(Value::as_str).map(str::to_owned).collect(),
                _ => return Err(format!("rule {k} is a string or a list of strings")),
            };
            sys.rules.retain(|(x, _)| *x != c);
            sys.rules.push((c, choices));
        }
    }
    if sys.rules.is_empty() {
        return Err("an lsystem needs \"rules\"".into());
    }
    let pick = |k: &str| spec.and_then(|s| s.get(k)).or(o.get(k)).and_then(Value::as_f64);
    let mut how = base.map_or(growth::Turtle::default(), |b| b.turtle);
    how.angle = pick("angle").unwrap_or(how.angle);
    how.width = pick("width").unwrap_or(how.width);
    how.taper = pick("taper").unwrap_or(how.taper).clamp(0.05, 1.0);
    how.shrink = pick("shrink").unwrap_or(how.shrink).clamp(0.05, 1.5);
    how.droop = pick("droop").unwrap_or(how.droop);
    let iterations = pick("iterations").map_or(base.map_or(4, |b| b.iterations), |v| v as usize).min(12);
    let seed = pick("seed").unwrap_or(1.0) as u32;
    let (s, capped) = growth::expand(&sys, iterations, seed);
    let grown = growth::turtle(&s, &how);
    if grown.branches.is_empty() {
        return Err("the L-system drew nothing (no F or G steps in what it grew)".into());
    }
    // Radii: the pipe model by default (one smooth taper per branch, a
    // fork's section shared by its children); "radii": "turtle" keeps the
    // turtle's own (thinned by `taper` at every `!` and `[`).
    let radii: Vec<Vec<f64>> = match o.get("radii").and_then(Value::as_str).unwrap_or("pipe") {
        "pipe" | "da_vinci" => growth::pipe_radii(&grown, how.width, pick("pipe").unwrap_or(2.0)),
        "turtle" => grown.branches.iter().map(|b| b.radii.clone()).collect(),
        other => return Err(format!("radii is \"pipe\" (smooth taper, forks share the parent's section) or \"turtle\" (thinned at every ! and [), not \"{other}\"")),
    };
    let fixed_sides = pick("sides").map(|v| (v as usize).clamp(3, 32));
    // Every branch a tube; then leaves.
    let all: Vec<[f64; 3]> = grown.branches.iter().flat_map(|b| b.points.iter().copied()).collect();
    let (lo, hi) = bounds3(&all);
    let (place, px_per_unit) = placer(lo, hi, fit);
    let mut mesh = Mesh::default();
    let bark = material("Bark", [96, 70, 48], 0.85);
    for (b, r) in grown.branches.iter().zip(&radii) {
        let sides = fixed_sides.unwrap_or_else(|| tube_sides(r.first().copied().unwrap_or(0.0) * px_per_unit as f64));
        let sheet = surface::tube(&b.points, r, sides);
        let built = surface::build(&sheet, &place, 0.0, None);
        studio_mesh::ops::merge(&mut mesh, &built.mesh);
    }
    mesh.materials = vec![bark];
    mesh.single_group();
    let leaves = o.get("leaves").and_then(Value::as_bool).unwrap_or(true) && !grown.leaves.is_empty();
    if leaves {
        let mut leaf = material("Leaf", [74, 128, 52], 0.6);
        leaf.subsurface = 0.3;
        if let Some(c) = colour_field(o, "leaf_colour")? {
            leaf.base_colour = c;
        }
        let one = studio_mesh::prim::uv_sphere(1.0, 8, 5);
        let mut leaves_mesh = Mesh::default();
        for l in grown.leaves.iter().take(20_000) {
            let s = l.size * 0.6;
            let up = [l.heading[1] * l.side[2] - l.heading[2] * l.side[1], l.heading[2] * l.side[0] - l.heading[0] * l.side[2], l.heading[0] * l.side[1] - l.heading[1] * l.side[0]];
            let mut m = one.clone();
            for p in &mut m.positions {
                let q = [p[0] as f64, p[1] as f64, p[2] as f64];
                // Long along the heading, wide along the side, thin.
                let w = [0, 1, 2].map(|i| l.at[i] + l.heading[i] * (q[1] + 1.0) * s + l.side[i] * q[0] * s * 0.4 + up[i] * q[2] * s * 0.05);
                *p = place(w);
            }
            studio_mesh::ops::smooth_normals_in_place(&mut m);
            studio_mesh::ops::merge(&mut leaves_mesh, &m);
        }
        leaves_mesh.materials = vec![leaf];
        leaves_mesh.single_group();
        studio_mesh::ops::merge(&mut mesh, &leaves_mesh);
    }
    // Fitted again, leaves and bark thickness and all.
    math::fit(&mut mesh, fit);
    let said = json!({"kind": "lsystem", "preset": preset, "radii": o.get("radii").and_then(Value::as_str).unwrap_or("pipe"), "symbols": s.len(), "capped": capped || grown.capped, "branches": grown.branches.len(), "leaves": if leaves { grown.leaves.len().min(20_000) } else { 0 }, "iterations": iterations, "angle": how.angle});
    Ok(Made { mesh, name: titled(preset.unwrap_or("Plant")), said, moving: None, recipe: None })
}

fn phyllo(o: &Map<String, Value>, preset: Option<&str>, given: &[(String, f64)], fit: Fit, detail: usize) -> Result<Made, String> {
    if let Some((k, _)) = given.first() {
        return Err(format!("phyllotaxis has no setting \"{k}\"; its fields are {}", Kind::Phyllo.fields().join(", ")));
    }
    let word = o.get("phyllotaxis").and_then(Value::as_str).or(preset).unwrap_or("disc");
    let spread = spread_of(word).ok_or_else(|| format!("phyllotaxis \"{word}\": disc (sunflower), ball (dandelion), cone (pinecone) or dome (cactus)"))?;
    let count = o.get("count").and_then(Value::as_f64).map_or(match spread {
        growth::Spread::Disc => 600,
        _ => 300,
    }, |c| c as usize);
    let angle = o.get("angle").and_then(Value::as_f64).unwrap_or(137.508);
    let seeds = growth::phyllotaxis(count, angle, spread);
    let n = seeds.len();
    // Seeds about as big as the gap between them.
    let seed = o.get("seed_size").and_then(Value::as_f64).unwrap_or(match spread {
        growth::Spread::Disc => 1.9 / (n as f64).sqrt(),
        _ => 3.2 / (n as f64).sqrt(),
    });
    let mut seed_mat = material("Seed", [70, 48, 30], 0.7);
    if let Some(c) = colour_field(o, "seed_colour")? {
        seed_mat.base_colour = c;
    }
    let segs = fineness(detail, [5, 6, 8, 10, 12, 14]);
    let one = studio_mesh::prim::uv_sphere(1.0, segs, segs / 2 + 1);
    let mut places: Vec<[f64; 3]> = Vec::new();
    let mut seeds_mesh = Mesh::default();
    let all: Vec<[f64; 3]> = seeds.iter().map(|s| s.at).collect();
    let (mut lo, mut hi) = bounds3(&all);
    for i in 0..3 {
        lo[i] -= seed;
        hi[i] += seed;
    }
    let (place, _) = placer(lo, hi, fit);
    for s in &seeds {
        let nn = s.normal;
        let pick = if nn[0].abs() < 0.9 { [1.0, 0.0, 0.0] } else { [0.0, 1.0, 0.0] };
        let a = {
            let c = [nn[1] * pick[2] - nn[2] * pick[1], nn[2] * pick[0] - nn[0] * pick[2], nn[0] * pick[1] - nn[1] * pick[0]];
            let l = (c[0] * c[0] + c[1] * c[1] + c[2] * c[2]).sqrt().max(1e-12);
            c.map(|x| x / l)
        };
        let b = [nn[1] * a[2] - nn[2] * a[1], nn[2] * a[0] - nn[0] * a[2], nn[0] * a[1] - nn[1] * a[0]];
        let r = seed * 0.5 * s.size;
        let mut m = one.clone();
        for p in &mut m.positions {
            let q = [p[0] as f64, p[1] as f64, p[2] as f64];
            let w = [0, 1, 2].map(|i| s.at[i] + (a[i] * q[0] + b[i] * q[2]) * r + nn[i] * q[1] * r * 0.6);
            places.push(w);
            *p = place(w);
        }
        studio_mesh::ops::smooth_normals_in_place(&mut m);
        studio_mesh::ops::merge(&mut seeds_mesh, &m);
    }
    seeds_mesh.materials = vec![seed_mat];
    seeds_mesh.single_group();
    let mut mesh = seeds_mesh;
    // A body under the seeds, so a cone or a ball is solid.
    let core = o.get("core").and_then(Value::as_bool).unwrap_or(spread != growth::Spread::Disc);
    if core {
        let mut body = material("Core", [120, 96, 60], 0.8);
        if let Some(c) = colour_field(o, "core_colour")? {
            body.base_colour = c;
        }
        let mut c = studio_mesh::prim::uv_sphere(1.0, 32, 16);
        for p in &mut c.positions {
            let q = [p[0] as f64, p[1] as f64, p[2] as f64];
            let w = match spread {
                growth::Spread::Cone => {
                    // The egg the seeds sit on, a little inside.
                    let z = q[2];
                    let r = 0.62 * (std::f64::consts::PI * (1.0 - z) / 2.0).sin().powf(0.8) * (1.15 - 0.3 * z) * 0.92;
                    let l = q[0].hypot(q[1]).max(1e-9);
                    [q[0] / l * r * (1.0 - z * z).sqrt(), q[1] / l * r * (1.0 - z * z).sqrt(), z * 0.97]
                }
                growth::Spread::Dome => [q[0] * 0.93, q[1] * 0.93, q[2].max(0.0) * 0.93],
                _ => [q[0] * 0.93, q[1] * 0.93, q[2] * 0.93],
            };
            *p = place(w);
        }
        studio_mesh::ops::smooth_normals_in_place(&mut c);
        c.materials = vec![body];
        c.single_group();
        studio_mesh::ops::merge(&mut mesh, &c);
    }
    math::fit(&mut mesh, fit);
    let said = json!({"kind": "phyllotaxis", "spread": word, "seeds": n, "angle": angle, "seed_size": seed, "core": core});
    Ok(Made { mesh, name: titled(match spread {
        growth::Spread::Disc => "Sunflower head",
        growth::Spread::Ball => "Seed ball",
        growth::Spread::Cone => "Pine cone",
        growth::Spread::Dome => "Dome of seeds",
    }), said, moving: None, recipe: None })
}

/// An attractor's trail as points (formula space), and what was used.
pub(super) fn attractor_points(o: &Map<String, Value>, preset: Option<&str>, given: &[(String, f64)]) -> Result<(Vec<[f64; 3]>, Value), String> {
    let p = preset.and_then(growth::attractor);
    type Equations<'a> = ([String; 3], Option<&'a [(&'a str, f64)]>);
    let (eqs, params): Equations = match (p, o.get("attractor")) {
        (Some(p), _) => (p.equations.map(str::to_owned), Some(p.params)),
        (None, Some(Value::Object(e))) => {
            let get = |k: &str| e.get(k).and_then(Value::as_str).map(str::to_owned).ok_or_else(|| format!("attractor {{\"dx\", \"dy\", \"dz\"}}: dx/dt, dy/dt, dz/dt as formulas of x, y, z (\"{k}\" is missing)"));
            ([get("dx")?, get("dy")?, get("dz")?], None)
        }
        _ => return Err(format!("attractor is a preset ({}) or {{\"dx\", \"dy\", \"dz\"}}: the equations as formulas of x, y, z", growth::ATTRACTORS.iter().map(|a| a.name).collect::<Vec<_>>().join(", "))),
    };
    let progs = [0, 1, 2].map(|i| bound("attractor", params, given, &eqs[i]).and_then(|s| math::compile(["dx", "dy", "dz"][i], &s, growth::ODE_NAMES)));
    let [a, b, c] = progs;
    let eq = [a?, b?, c?];
    let start = match o.get("start") {
        Some(v) => v3(v).map(|s| s.map(|x| x as f64)).ok_or("start is [x, y, z]")?,
        None => p.map_or([0.1, 0.0, 0.0], |p| p.start),
    };
    let dt = o.get("dt").and_then(Value::as_f64).unwrap_or(p.map_or(0.01, |p| p.dt)).clamp(1e-5, 1.0);
    let steps = o.get("steps").and_then(Value::as_f64).map_or(p.map_or(10_000, |p| p.steps), |s| s as usize);
    let skip = o.get("skip").and_then(Value::as_f64).map_or(200, |s| s as usize);
    let every = o.get("every").and_then(Value::as_f64).map_or((steps / 5000).max(1), |e| e as usize);
    let pts = growth::integrate(&eq, start, dt, steps, skip, every);
    if pts.len() < 2 {
        return Err("the attractor ran off to infinity at once (or skip ≥ steps): check its equations, start and dt".into());
    }
    let ran = steps.min(growth::MAX_STEPS);
    Ok((pts, json!({"steps": ran, "dt": dt, "evals": ran as u64 * 12, "points": ran.saturating_sub(skip) / every.max(1)})))
}

fn attractor(o: &Map<String, Value>, preset: Option<&str>, given: &[(String, f64)], fit: Fit, detail: usize) -> Result<Made, String> {
    let (pts, run) = attractor_points(o, preset, given)?;
    let (lo, hi) = bounds3(&pts);
    let span = (0..3).map(|i| hi[i] - lo[i]).fold(0.0f64, f64::max);
    let r = o.get("radius").and_then(Value::as_f64).unwrap_or(span * 0.004).max(span * 1e-5);
    let sides = o.get("sides").and_then(Value::as_f64).map_or(fineness(detail, [4, 5, 6, 8, 10, 12]), |s| s as usize).clamp(3, 32);
    let sheet = surface::tube(&pts, &[r], sides);
    let (slo, shi) = sheet.bounds();
    let (place, _) = placer(slo, shi, fit);
    let built = surface::build(&sheet, &place, 0.0, None);
    let mut said = json!({"kind": "attractor", "preset": preset, "closed": built.closed, "radius": r, "sides": sides});
    if let (Some(a), Some(b)) = (said.as_object_mut(), run.as_object()) {
        a.extend(b.clone());
    }
    Ok(Made { mesh: built.mesh, name: titled(preset.unwrap_or("Attractor")), said, moving: None, recipe: None })
}

/// The `fit` of a `size` field: a number (the largest side) or [x, y, z].
pub(super) fn fit_of(op: &Value, default: f32) -> Result<Fit, String> {
    match op.get("size") {
        None => Ok(Fit::Largest(default)),
        Some(Value::Number(n)) => Ok(Fit::Largest((n.as_f64().unwrap_or(default as f64) as f32).max(1.0))),
        Some(v) => v3(v).map(|s| Fit::Sides(s.map(|x| x.max(1.0)))).ok_or_else(|| "\"size\" is a number (the largest side, px) or [x, y, z]".into()),
    }
}

/// What drawing directly means, for the replies.
const DIRECT_SAID: &str = "drawn straight from its formula (sphere traced per pixel on the GPU, the CPU without one: exact detail at any size, moving with t live); its mesh is a coarse stand-in that casts its shadows and is what physics, rigs, booleans, mesh steps, exports and path-traced renders use";

impl Session {
    /// Sets how a math model is drawn: `direct` from its recipe, or `mesh`.
    pub(super) fn set_math_render(mc: &mut m::Clip, recipe: Option<&field::Recipe>, direct: bool) {
        if let m::ClipSource::Generator(g) = &mut mc.source {
            if let Some(r) = recipe {
                g.params.insert("math_recipe".into(), m::Value::Text(r.to_text()));
            }
            g.params.insert("math_render".into(), m::Value::Text(if direct { "direct" } else { "mesh" }.into()));
        }
    }

    /// Marks clip `id` as made from a `kind` of math, and keeps an implicit
    /// one's recipe and way of drawing.
    pub(super) fn keep_math(&mut self, id: u32, kind: &str, recipe: Option<&field::Recipe>, direct: bool) -> Result<String, String> {
        let mut mc = self.model_clip(id).ok_or("that clip is gone")?;
        if let m::ClipSource::Generator(g) = &mut mc.source {
            g.params.insert("math_kind".into(), m::Value::Text(kind.into()));
        }
        if recipe.is_some() {
            Self::set_math_render(&mut mc, recipe, direct);
        }
        let seq = self.doc.seq();
        let ok = self.doc.apply("Math shape", m::Edit::SetClip { seq, clip: Box::new(mc) });
        self.check(ok, "")?;
        Ok(if direct { DIRECT_SAID.into() } else { "drawn as its mesh (render: direct draws it from its formula instead)".into() })
    }

    /// Bakes a moving math shape on clip `id` over the clip's length into
    /// a played cache; says how.
    pub(super) fn bake_math(&mut self, id: u32, mesh: &Mesh, moving: &Moving) -> Result<String, String> {
        let mc = self.model_clip(id).ok_or("that clip is gone")?;
        let dur = secs(mc.duration).max(0.1);
        let mut fps = self.fps() as f64;
        let verts = mesh.positions.len().max(1);
        let mut frames = (dur * fps).ceil() as usize + 1;
        let mut notes = Vec::new();
        let most = MAX_FRAMES.min(MAX_BAKED / verts).min((math::MAX_EVALS / moving.cost.max(1)) as usize).max(2);
        if frames > most {
            frames = most;
            fps = (frames - 1) as f64 / dur;
            notes.push(format!("at {fps:.1} frames a second (played smoothly between)"));
        }
        let mut data = Vec::with_capacity(frames * verts);
        let mut prev = mesh.positions.clone();
        for f in 0..frames {
            let t = f as f64 / fps;
            let now = (moving.at)(t, &prev);
            if now.len() != verts {
                return Err("the moving shape changed its vertex count; nothing was baked".into());
            }
            data.extend_from_slice(&now);
            prev = now;
        }
        let cache = studio_mesh::cache::Cache { fps: fps as f32, frames, data: studio_mesh::cache::Frames::Vertices { count: verts, data } };
        let file = self.save_cache(&cache)?;
        self.play_cache(id, file, 0.0, &[], "Math motion")?;
        Ok(format!("moves with t: baked {frames} frames over {dur:.2} s{}", notes.iter().map(|n| format!(" {n}")).collect::<String>()))
    }

    /// `animate` along a `curve` (position keys) or by a `formula` (keys of
    /// any property).
    pub(super) fn animate_math(&mut self, op: &Value) -> Result<String, String> {
        let id = op.get("clip").and_then(Value::as_u64).ok_or("animate needs \"clip\"")? as u32;
        let c = self.clip(id)?;
        let clip_len = secs(c.dur);
        let fps = self.fps() as f64;
        if let Some(fm) = op.get("formula") {
            let (src, o) = match fm {
                Value::String(s) => (s.clone(), Map::new()),
                Value::Object(o) => (o.get("expr").and_then(Value::as_str).ok_or("formula {\"expr\": \"…\", \"start\", \"duration\", \"every\"}")?.to_owned(), o.clone()),
                _ => return Err("formula is a formula of t (seconds into the clip), e.g. \"360 * t / 4\", or {\"expr\", \"start\", \"duration\", \"every\"}".into()),
            };
            let prop = op.get("property").and_then(Value::as_str).ok_or("animate with a formula needs \"property\"")?;
            if prop == "position" {
                return Err("position takes a curve (\"curve\": {\"x\": …, \"y\": …} or a preset); a formula keys one number: x, y, z, rotation, scale, opacity…".into());
            }
            let prog = math::compile("formula", &src, &["t"])?;
            let start = o.get("start").and_then(Value::as_f64).unwrap_or(0.0).max(0.0);
            let dur = o.get("duration").and_then(Value::as_f64).unwrap_or(clip_len - start).clamp(0.0, clip_len - start);
            let every = o.get("every").and_then(Value::as_f64).unwrap_or(1.0 / fps).max(dur / MAX_KEYS as f64).max(1e-3);
            let n = ((dur / every).round() as usize + 1).clamp(2, MAX_KEYS);
            let samples: Vec<(f64, f64)> = (0..n).map(|k| {
                let t = start + dur * k as f64 / (n - 1) as f64;
                (t, prog.eval(&[t]))
            }).collect();
            let undefined: Vec<f64> = samples.iter().filter(|(_, v)| !v.is_finite()).map(|(t, _)| *t).collect();
            let keys: Vec<Value> = samples.iter().filter(|(_, v)| v.is_finite()).map(|(t, v)| json!([t, v])).collect();
            if keys.len() < 2 {
                return Err(format!("formula \"{src}\" is undefined over the clip"));
            }
            let said = self.animate(&json!({"clip": id, "property": prop, "keys": keys, "ease": "linear"}))?;
            // Say what was stored, and where the formula had no value: the
            // property holds still over those moments.
            let skipped = match undefined.as_slice() {
                [] => String::new(),
                [first, .., last] => format!(" ({} of {n} samples undefined, skipped, from {first:.2} s to {last:.2} s: the property holds still there)", undefined.len()),
                [only] => format!(" (1 of {n} samples undefined, skipped, at {only:.2} s)"),
            };
            return Ok(format!("{prop} follows {src} over {dur:.2} s: {said}{skipped}"));
        }
        let spec = op.get("curve").ok_or("animate needs \"keys\", \"curve\" or \"formula\"")?;
        let mut o = match spec {
            Value::String(s) => {
                let mut o = Map::new();
                o.insert("preset".into(), Value::String(s.clone()));
                o
            }
            Value::Object(o) => o.clone(),
            _ => return Err(format!("curve is a preset name ({}) or a curve {{\"x\", \"y\"}}, {{\"polar\"}}, {{\"attractor\"}}…", studio_vector::curve::names())),
        };
        let opt = |k: &str, o: &mut Map<String, Value>| o.remove(k);
        let start = opt("start", &mut o).and_then(|v| v.as_f64()).unwrap_or(0.0).max(0.0);
        let dur = opt("duration", &mut o).and_then(|v| v.as_f64()).unwrap_or(clip_len - start).clamp(0.05, (clip_len - start).max(0.05));
        let loops = opt("loops", &mut o).and_then(|v| v.as_f64()).unwrap_or(1.0).clamp(0.05, 100.0);
        let orient = opt("orient", &mut o).and_then(|v| v.as_bool()).unwrap_or(false);
        let centre = opt("centre", &mut o).or(opt("center", &mut o));
        let keys_n = opt("keys", &mut o).and_then(|v| v.as_f64()).map_or((dur * fps).ceil() as usize + 1, |k| k as usize).clamp(2, MAX_KEYS);
        let mc = self.model_clip(id).ok_or("that clip is gone")?;
        let here = [mc.transform.position.x, mc.transform.position.y, mc.transform.position_z];
        let base = match centre {
            Some(v) => {
                let a = v.as_array().ok_or("centre is [x, y] (or [x, y, z]) in canvas px")?;
                [a.first().and_then(Value::as_f64).unwrap_or(here[0]), a.get(1).and_then(Value::as_f64).unwrap_or(here[1]), a.get(2).and_then(Value::as_f64).unwrap_or(here[2])]
            }
            None => here,
        };
        // The path in canvas px about its centre: (x, y down, depth).
        let three_d = o.contains_key("attractor") || o.contains_key("z");
        let (path, closed, evals): (Vec<[f64; 3]>, bool, u64) = if three_d {
            let size = o.get("size").and_then(Value::as_f64).unwrap_or(400.0);
            let pts: Vec<[f64; 3]> = if o.contains_key("attractor") {
                let (kind_o, _, preset) = read(&Value::Object(o.clone()))?;
                let mut kind_o = kind_o;
                kind_o.remove("size");
                let given = settings(&kind_o, Kind::Attractor)?;
                attractor_points(&kind_o, preset.as_deref(), &given)?.0
            } else {
                let names = ["u", "t"];
                let get = |k: &str| o.get(k).and_then(Value::as_str).ok_or_else(|| format!("a 3D curve needs \"x\", \"y\" and \"z\" formulas of u (\"{k}\" is missing)")).and_then(|s| math::compile(k, s, &names));
                let (fx, fy, fz) = (get("x")?, get("y")?, get("z")?);
                let u = range(o.get("u"), ["0", "tau"], None, &[])?;
                (0..2000).map(|k| {
                    let uu = u[0] + (u[1] - u[0]) * k as f64 / 1999.0;
                    [fx.eval(&[uu, 0.0]), fy.eval(&[uu, 0.0]), fz.eval(&[uu, 0.0])]
                }).collect()
            };
            let (lo, hi) = bounds3(&pts);
            let span = (0..3).map(|i| hi[i] - lo[i]).fold(1e-12f64, f64::max);
            let c = [0, 1, 2].map(|i| (lo[i] + hi[i]) * 0.5);
            let k = size / span;
            // Formula space (z up, y away) to the canvas (y down) and depth.
            let px: Vec<[f64; 3]> = pts.iter().map(|p| [(p[0] - c[0]) * k, -(p[2] - c[2]) * k, (p[1] - c[1]) * k]).collect();
            (px, false, pts.len() as u64 * 3)
        } else {
            if !o.contains_key("size") && !o.contains_key("scale") {
                o.insert("size".into(), json!(300));
            }
            let l = studio_vector::curve::make(&o, 0.0, &|k, d| o.get(k).and_then(Value::as_f64).unwrap_or(d))?;
            let (pts, closed) = l.lines.into_iter().max_by_key(|(p, _)| p.len()).ok_or("the curve has no points")?;
            (pts.into_iter().map(|p| [p[0], p[1], 0.0]).collect(), closed, l.evals)
        };
        // Even speed: points spaced by length along the path.
        let mut ring = path.clone();
        if closed && let Some(f) = ring.first().copied() {
            ring.push(f);
        }
        let d3 = |a: [f64; 3], b: [f64; 3]| ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt();
        let mut acc = vec![0.0];
        for w in ring.windows(2) {
            acc.push(acc[acc.len() - 1] + d3(w[0], w[1]));
        }
        let total = acc[acc.len() - 1].max(1e-9);
        let at_len = |s: f64| {
            let s = s.rem_euclid(total + 1e-12).min(total);
            let k = acc.partition_point(|x| *x < s).clamp(1, acc.len() - 1);
            let u = ((s - acc[k - 1]) / (acc[k] - acc[k - 1]).max(1e-12)).clamp(0.0, 1.0);
            [0, 1, 2].map(|i| ring[k - 1][i] + (ring[k][i] - ring[k - 1][i]) * u)
        };
        let mut pos = Vec::with_capacity(keys_n);
        let mut zs = Vec::with_capacity(keys_n);
        let mut turns = Vec::with_capacity(keys_n);
        let mut last_angle: Option<f64> = None;
        for k in 0..keys_n {
            let f = k as f64 / (keys_n - 1) as f64;
            let t = start + dur * f;
            let s = total * loops * f;
            let p = at_len(if !closed && loops > 1.0 { let c = (s / total).floor(); let r = s - c * total; if c as i64 % 2 == 1 { total - r } else { r } } else { s.min(total * loops) });
            pos.push(json!([t, base[0] + p[0], base[1] + p[1]]));
            zs.push(json!([t, base[2] + p[2]]));
            if orient {
                let q = at_len((total * loops * f + total * 1e-3).min(total * loops));
                let mut a = (q[1] - p[1]).atan2(q[0] - p[0]).to_degrees();
                if let Some(prev) = last_angle {
                    while a - prev > 180.0 {
                        a -= 360.0;
                    }
                    while a - prev < -180.0 {
                        a += 360.0;
                    }
                }
                last_angle = Some(a);
                turns.push(json!([t, a]));
            }
        }
        let mut said = vec![self.animate(&json!({"clip": id, "property": "position", "keys": pos, "ease": "linear"}))?];
        if three_d {
            said.push(self.animate(&json!({"clip": id, "property": "z", "keys": zs, "ease": "linear"}))?);
        }
        if orient {
            said.push(self.animate(&json!({"clip": id, "property": "rotation", "keys": turns, "ease": "linear"}))?);
        }
        Ok(format!("clip {id} follows the curve ({}{} px long, {keys_n} keys over {dur:.2} s, {loops} time{}; {evals} formula evaluations): {}", if three_d { "3D, " } else { "" }, total.round(), if loops == 1.0 { "" } else { "s" }, said.join("; ")))
    }
}

/// The `mesh` step `render`: how a math model is drawn. `direct` from its
/// formula; `mesh` remeshes it from the formula at `detail`.
pub(super) fn render_step(mesh: &mut Mesh, mc: &mut m::Clip, s: &Value) -> Result<String, String> {
    let recipe = match &mc.source {
        m::ClipSource::Generator(g) => match g.params.get("math_recipe") {
            Some(m::Value::Text(t)) => field::Recipe::from_text(t)?,
            _ => return Err("only a model made from an implicit math (an implicit formula, a lattice, a fractal) can be drawn from its formula".into()),
        },
        _ => return Err("not a model".into()),
    };
    match s.get("mode").and_then(Value::as_str).unwrap_or("direct") {
        "direct" | "trace" | "formula" => {
            Session::set_math_render(mc, None, true);
            Ok(DIRECT_SAID.into())
        }
        "mesh" | "triangles" => {
            let detail = s.get("detail").and_then(Value::as_u64).unwrap_or(4).clamp(1, 6) as usize;
            let f = Arc::new(recipe.field(0.0)?);
            let cell = field::cell_for(recipe.size, detail, f.cost());
            let mats = std::mem::take(&mut mesh.materials);
            let (m2, cost) = studio_mesh::sdf::build_counted(&[Part::field(f.clone(), [0.0; 3], recipe.size)], if mats.is_empty() { vec![studio_mesh::Material::default()] } else { vec![mats[0].clone()] }, cell);
            if m2.triangles() == 0 {
                mesh.materials = mats;
                return Err("the formula made no surface".into());
            }
            *mesh = m2;
            Session::set_math_render(mc, None, false);
            Ok(format!("meshed from its formula at detail {detail}: {} triangles, {} evaluations on a {:?} grid", mesh.triangles(), cost.samples * f.cost(), cost.grid))
        }
        other => Err(format!("render mode \"{other}\": direct (from the formula) or mesh")),
    }
}

/// The `mesh` warp steps: twist, bend, taper, warp, repeat.
pub(super) fn warp_step(mesh: &mut Mesh, what: &str, s: &Value) -> Result<String, String> {
    let axis_of = |k: &str, d: &str| -> Result<usize, String> {
        let w = s.get(k).and_then(Value::as_str).unwrap_or(d);
        deform::axis(w).ok_or_else(|| format!("\"{k}\" is x, y (up and down) or z"))
    };
    let (lo, hi) = mesh.bounds();
    let size = (hi[0] - lo[0]).max(hi[1] - lo[1]).max(hi[2] - lo[2]).max(1.0);
    // A coarse model is split first so the warp has points to move (not
    // a rigged one: its weights are per vertex).
    let mut refined = 0;
    if what != "repeat" && mesh.rig.is_none() && s.get("refine").and_then(Value::as_bool).unwrap_or(true) {
        refined = deform::refine(mesh, size / 40.0, 400_000);
    }
    let r = if refined > 0 { format!(" (split {refined}× first so it bends smoothly)") } else { String::new() };
    Ok(match what {
        "twist" => {
            let axis = axis_of("axis", "y")?;
            let deg = num(s, "degrees").unwrap_or(90.0);
            deform::twist(mesh, axis, deg);
            format!("twisted {deg}° along {}{r}", ["x", "y", "z"][axis])
        }
        "bend" => {
            let axis = axis_of("axis", "y")?;
            let toward = axis_of("toward", if axis == 0 { "y" } else { "x" })?;
            if toward == axis {
                return Err("\"toward\" is another axis than \"axis\" (the one the bend curls towards)".into());
            }
            let deg = num(s, "degrees").unwrap_or(90.0);
            deform::bend(mesh, axis, toward, deg);
            format!("bent {deg}° along {} towards +{}{r}", ["x", "y", "z"][axis], ["x", "y", "z"][toward])
        }
        "taper" => {
            let axis = axis_of("axis", "y")?;
            let (from, to) = (num(s, "from").unwrap_or(1.0), num(s, "to").unwrap_or(0.4));
            deform::taper(mesh, axis, from, to);
            format!("tapered from ×{from} to ×{to} along {} (the low end: for y, the top){r}", ["x", "y", "z"][axis])
        }
        "warp" => match s.get("math") {
            Some(m) => {
                let list = formulas(m, "warp math")?;
                if list.len() != 1 && list.len() != 3 {
                    return Err("warp math is \"[dx, dy, dz]\" (one formula) or three, offsets in formula space (z up) with the model's half size as 1".into());
                }
                let progs = list.iter().map(|f| math::compile("warp", f, deform::NAMES)).collect::<Result<Vec<_>, _>>()?;
                deform::formula(mesh, &progs, 0.0);
                format!("warped by {}{r}", list.join(", "))
            }
            None => {
                let amount = num(s, "amount").unwrap_or(size * 0.05);
                let feature = num(s, "size").unwrap_or(size * 0.3);
                deform::noise(mesh, amount, feature, s.get("seed").and_then(Value::as_u64).unwrap_or(1) as u32);
                format!("warped by noise, about {amount:.1} px over features {feature:.0} px across{r}")
            }
        },
        "repeat" => {
            let around = s.get("around").and_then(Value::as_bool).unwrap_or(false) || s.get("radius").is_some();
            if around {
                let n = s.get("count").and_then(Value::as_u64).unwrap_or(6).clamp(1, 256) as usize;
                let radius = num(s, "radius").unwrap_or(size);
                let axis = axis_of("axis", "y")?;
                if mesh.triangles() * n > 1_500_000 {
                    return Err(format!("{n} copies would make {} triangles; decimate first or make fewer", mesh.triangles() * n));
                }
                deform::around(mesh, n, radius, axis);
                format!("{n} copies round {} at {radius:.0} px", ["x", "y", "z"][axis])
            } else {
                let counts = match s.get("count") {
                    Some(Value::Number(c)) => [c.as_u64().unwrap_or(2) as usize, 1, 1],
                    Some(v) => v3(v).map(|c| c.map(|x| x.max(1.0) as usize)).ok_or("count is a number (along x) or [x, y, z]")?,
                    None => [3, 1, 1],
                };
                let counts = counts.map(|c| c.clamp(1, 64));
                let total = counts[0] * counts[1] * counts[2];
                if mesh.triangles() * total > 1_500_000 || total > 4096 {
                    return Err(format!("{total} copies would make {} triangles; decimate first or make fewer", mesh.triangles() * total));
                }
                let gap = [(hi[0] - lo[0]) * 1.15, (hi[1] - lo[1]) * 1.15, (hi[2] - lo[2]) * 1.15];
                let spacing = match s.get("spacing") {
                    Some(Value::Number(n)) => [n.as_f64().unwrap_or(0.0) as f32; 3],
                    Some(_) => field_v3(s, "spacing")?.unwrap_or(gap),
                    None => gap,
                };
                deform::repeat(mesh, counts, spacing);
                format!("{total} copies, {}×{}×{} spaced {:.0}, {:.0}, {:.0} px", counts[0], counts[1], counts[2], spacing[0], spacing[1], spacing[2])
            }
        }
        _ => return Err(format!("no warp \"{what}\"")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services;

    fn session() -> Session {
        Session::new(services::project::new_custom("Math", 1280, 720, 30.0))
    }

    fn last(s: &Session) -> u32 {
        s.doc.project.clips.iter().map(|c| c.id).max().unwrap()
    }

    #[test]
    fn tubes_get_sides_by_their_size_on_screen() {
        assert_eq!(tube_sides(0.5), 5);
        assert_eq!(tube_sides(8.0), 13);
        assert_eq!(tube_sides(40.0), 24);
        // A tree's trunk is rounder than its twigs, and the pipe radii
        // keep a fork's children no thicker than the branch they leave.
        let tree = make(&json!({"lsystem": "tree", "iterations": 4}), Fit::Largest(400.0), 4).unwrap();
        assert_eq!(tree.said["radii"], "pipe");
        // At 400 px the trunk (0.22 of a 10-step tree) is near 9 px thick:
        // a dozen sides or more; the thinnest twigs get five.
        let tp = growth::plant("tree").unwrap();
        let (s, _) = growth::expand(&tp.system(), 4, 1);
        let g = growth::turtle(&s, &tp.turtle);
        let r = growth::pipe_radii(&g, tp.turtle.width, 2.0);
        let all: Vec<[f64; 3]> = g.branches.iter().flat_map(|b| b.points.iter().copied()).collect();
        let (lo, hi) = bounds3(&all);
        let (_, k) = placer(lo, hi, Fit::Largest(400.0));
        let trunk = tube_sides(r[0][0] * k as f64);
        let twig = r.iter().map(|x| tube_sides(x[0] * k as f64)).min().unwrap();
        println!("tree at 400 px: trunk {trunk} sides, thinnest twig {twig}");
        assert!(trunk >= 10 && twig <= 6 && trunk > twig, "trunk {trunk}, twig {twig}");
        assert!(make(&json!({"lsystem": "tree", "radii": "fat"}), Fit::Largest(400.0), 4).is_err());
    }

    #[test]
    fn every_kind_of_math_model_is_made_and_described() {
        for (math, closed) in [
            (json!("gyroid"), true),
            (json!({"implicit": "x^2 + y^2 + z^2 - 1", "bounds": 1.2}), true),
            (json!({"preset": "mandelbulb", "power": 6}), true),
            (json!({"preset": "torus_knot", "p": 3, "q": 5}), true),
            (json!({"parametric": "[cos(u) * (2 + cos(v)), sin(u) * (2 + cos(v)), sin(v)]"}), true),
            (json!({"preset": "mobius"}), true),
            (json!({"preset": "seashell"}), false),
            (json!({"preset": "swirl_torus"}), true),
            (json!({"preset": "sphere", "cut": "fract(u * 12) < 0.4", "thickness": 4}), true),
            (json!({"height": "0.2 * sin(4 * x) * cos(4 * y)"}), true),
            (json!({"preset": "lorenz"}), true),
            (json!({"lsystem": "tree", "iterations": 4}), false),
            (json!({"phyllotaxis": "sunflower", "count": 200}), false),
        ] {
            // A session each: models placed together would rest on each other.
            let mut s = session();
            let r = s.edit(&[json!({"op": "model", "math": math, "size": 300, "detail": 2, "duration": 2})], "math", false).unwrap_or_else(|e| panic!("{math}: {e}"));
            let id = last(&s);
            let (_, mesh) = s.model_of(id).unwrap();
            assert!(mesh.triangles() > 100, "{math}: {r}");
            assert!(r.to_string().contains("math: "), "{math}: {r}");
            if closed {
                assert_eq!(math::open_edges(&mesh).0, 0, "{math} has holes");
            }
            let (lo, hi) = mesh.bounds();
            let big = (hi[0] - lo[0]).max(hi[1] - lo[1]).max(hi[2] - lo[2]);
            assert!(big > 200.0 && big < 330.0, "{math}: {big}");
        }
    }

    #[test]
    fn bad_math_says_what_is_wrong() {
        let mut s = session();
        let e = |m: Value| s_err(&mut session(), m);
        fn s_err(s: &mut Session, m: Value) -> String {
            s.edit(&[json!({"op": "model", "math": m})], "math", false).unwrap_err()
        }
        assert!(e(json!("nonsense")).contains("gyroid"));
        assert!(e(json!({"preset": "gyroid", "bogus": 2})).contains("cells, thickness, solid"));
        assert!(e(json!({"implicit": "x^2 +"})).contains("implicit"));
        assert!(e(json!({"implicit": "x^2 + 5"})).contains("no surface"));
        assert!(e(json!({"implicit": "value_at(1)"})).contains("keys"));
        assert!(e(json!({"parametric": "[u, v]", "colour": "red"})).contains("colour"));
        let _ = &mut s;
    }

    #[test]
    fn a_shape_that_moves_with_t_is_baked_and_plays() {
        let mut s = session();
        s.edit(&[json!({"op": "model", "math": {"preset": "ripple_sphere", "speed": 3}, "size": 200, "detail": 1, "duration": 1})], "m", false).unwrap();
        let id = last(&s);
        let mc = s.model_clip(id).unwrap();
        let m::ClipSource::Generator(g) = &mc.source else { panic!() };
        let file = g.params.get("cache").and_then(|v| if let m::Value::Text(t) = v { Some(t.clone()) } else { None }).expect("a cache");
        let cache = studio_mesh::cache::Cache::load(std::path::Path::new(&file)).unwrap();
        let (_, mesh) = s.model_of(id).unwrap();
        assert!(cache.frames > 10 && cache.frames <= MAX_FRAMES);
        let a = cache.vertices_at(0.0).unwrap();
        let b = cache.vertices_at(0.5).unwrap();
        assert_eq!(a.len(), mesh.positions.len());
        assert!(a.iter().zip(&b).any(|(p, q)| studio_mesh::length(studio_mesh::sub(*p, *q)) > 0.5), "it moves");
        // A height field of waves moves too, and a parametric morph.
        s.edit(&[json!({"op": "model", "math": {"preset": "waves", "speed": 2}, "size": 300, "detail": 1, "duration": 1})], "m", false).unwrap();
        s.edit(&[json!({"op": "model", "math": {"preset": "sphere", "morph": {"to": "torus", "over": [0, 1]}}, "size": 300, "detail": 1, "duration": 1})], "m", false).unwrap();
        let mc = s.model_clip(last(&s)).unwrap();
        let m::ClipSource::Generator(g) = &mc.source else { panic!() };
        assert!(g.params.contains_key("cache"));
    }

    #[test]
    fn fields_blend_into_clay_parts_and_warps_work_on_any_model() {
        let mut s = session();
        let r = s.edit(&[json!({"op": "model", "parts": [
            {"shape": "ellipsoid", "size": 200},
            {"math": {"preset": "gyroid", "cells": 3, "solid": 1}, "size": 220, "intersect": true, "blend": 4},
        ], "detail": 2})], "p", false).unwrap();
        let id = last(&s);
        let (_, m) = s.model_of(id).unwrap();
        assert!(m.triangles() > 2000, "{r}");
        assert_eq!(math::open_edges(&m).0, 0);
        let v_before = math::volume(&m);
        s.edit(&[json!({"op": "mesh", "clip": id, "steps": [{"do": "twist", "degrees": 90}, {"do": "taper", "to": 0.6}, {"do": "bend", "degrees": 30}]})], "w", false).unwrap();
        let (_, w) = s.model_of(id).unwrap();
        assert!(math::volume(&w) < v_before, "tapered smaller");
        s.edit(&[json!({"op": "model", "shape": "cube", "size": 100, "detail": 1})], "c", false).unwrap();
        let c = last(&s);
        let r = s.edit(&[json!({"op": "mesh", "clip": c, "steps": [{"do": "warp", "math": "[0.2 * sin(4 * z), 0, 0]"}, {"do": "repeat", "count": [3, 1, 2]}]})], "w", false).unwrap();
        assert!(r.to_string().contains("6 copies"), "{r}");
        let e = s.edit(&[json!({"op": "model", "parts": [{"math": "torus_knot", "size": 100}]})], "p", false).unwrap_err();
        assert!(e.contains("implicit"), "{e}");
        // A math shape added to a model.
        s.edit(&[json!({"op": "mesh", "clip": c, "do": "add", "math": "trefoil", "size": 60, "at": [0, -120, 0]})], "a", false).unwrap();
    }

    #[test]
    fn a_math_model_can_be_drawn_from_its_formula_and_switched_back() {
        let mut s = session();
        let r = s.edit(&[json!({"op": "model", "math": {"preset": "gyroid", "cells": 3}, "render": "direct", "size": 200, "detail": 5})], "d", false).unwrap();
        assert!(r.to_string().contains("sphere traced"), "{r}");
        let id = last(&s);
        let param = |s: &Session, k: &str| match &s.model_clip(id).unwrap().source {
            m::ClipSource::Generator(g) => g.params.get(k).and_then(|v| if let m::Value::Text(t) = v { Some(t.clone()) } else { None }),
            _ => None,
        };
        assert_eq!(param(&s, "math_render").as_deref(), Some("direct"));
        let recipe = field::Recipe::from_text(&param(&s, "math_recipe").unwrap()).unwrap();
        assert!(recipe.formulas[0].contains("gyroid(") && (recipe.size[0] - 200.0).abs() < 1e-3);
        // The stand-in mesh is coarse; meshing it again from the formula is fine.
        let coarse = s.model_of(id).unwrap().1.triangles();
        s.edit(&[json!({"op": "mesh", "clip": id, "do": "render", "mode": "mesh", "detail": 4})], "m", false).unwrap();
        assert_eq!(param(&s, "math_render").as_deref(), Some("mesh"));
        assert!(s.model_of(id).unwrap().1.triangles() > coarse);
        s.edit(&[json!({"op": "mesh", "clip": id, "do": "render", "mode": "direct"})], "m", false).unwrap();
        assert_eq!(param(&s, "math_render").as_deref(), Some("direct"));
        // Only distance fields can be traced.
        assert!(s.edit(&[json!({"op": "model", "math": "torus_knot", "render": "direct"})], "d", false).unwrap_err().contains("implicit"));
        assert!(s.edit(&[json!({"op": "model", "shape": "cube", "render": "direct"})], "d", false).unwrap_err().contains("math"));
        s.edit(&[json!({"op": "model", "shape": "cube", "size": 50})], "c", false).unwrap();
        let c = last(&s);
        assert!(s.edit(&[json!({"op": "mesh", "clip": c, "do": "render", "mode": "direct"})], "r", false).unwrap_err().contains("implicit"));
    }

    #[test]
    fn layers_move_along_curves_and_by_formulas() {
        let mut s = session();
        s.edit(&[json!({"op": "model", "shape": "sphere", "size": 60, "detail": 1, "duration": 4})], "b", false).unwrap();
        let id = last(&s);
        let r = s.edit(&[json!({"op": "animate", "clip": id, "property": "position", "curve": {"preset": "lissajous", "size": 400, "orient": true}})], "c", false).unwrap();
        assert!(r.to_string().contains("follows the curve"), "{r}");
        let mc = s.model_clip(id).unwrap();
        let xs = &mc.keyframes["transform.position"].keys;
        assert!(xs.len() > 100);
        assert!(mc.keyframes.contains_key("transform.rotation"));
        s.edit(&[json!({"op": "animate", "clip": id, "property": "position", "curve": {"attractor": "lorenz", "size": 300, "duration": 4}})], "c", false).unwrap();
        let mc = s.model_clip(id).unwrap();
        assert!(mc.keyframes.contains_key("transform.position_z"));
        s.edit(&[json!({"op": "animate", "clip": id, "property": "rotation_y", "formula": "90 * sin(t * tau / 2)"})], "f", false).unwrap();
        let mc = s.model_clip(id).unwrap();
        assert!(mc.keyframes.keys().any(|k| k.contains("rotation_y")));
        assert!(s.edit(&[json!({"op": "animate", "clip": id, "property": "rotation_y", "formula": "prop(1, \"x\")"})], "f", false).unwrap_err().contains("prop"));
    }
}
