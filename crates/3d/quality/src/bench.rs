// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The quality benchmark: a fixed, varied set of cases (characters
//! realistic and stylised, creatures, props, vehicles, architecture,
//! environments, products for 3D; illustrations, photos, posters, logos
//! for pictures and vector art), each made the way Studio makes it and
//! judged by the critics, with the scores kept over time.
//!
//! A 3D case is a recipe in the same words the AI uses for the `model` op
//! (blended parts, or a primitive or the head base), plus what kind of
//! asset it is. Its reference is the same design built very finely: the
//! shape that was asked for. Each case is run through
//! - `raw`: the model as it comes out of the recipe today (what the AI's
//!   `model` op adds), and
//! - `finished`: the production finish ([`crate::production`]),
//!
//! and, for any case with a file of the same name in an outside folder
//! (a model made elsewhere, from the same prompt), `outside`, judged the
//! same way against the same reference.
//!
//! Picture and vector cases are judged from files: the folder given holds
//! `<case id>.png` and `<case id>.svg` from whoever made them.

use crate::score::{Class, critique_model};
use serde::Serialize;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use studio_mesh::{Material, Mesh, V3};

/// The cases, kept with the crate so every run judges the same set.
pub const CASES: &str = include_str!("../bench/cases.json");

fn v3(v: &Value) -> Option<V3> {
    let a = v.as_array()?;
    if a.len() != 3 {
        return None;
    }
    Some([a[0].as_f64()? as f32, a[1].as_f64()? as f32, a[2].as_f64()? as f32])
}

fn colour(v: &Value) -> Option<[f32; 4]> {
    if let Some(a) = v.as_array() {
        let c: Vec<f32> = a.iter().filter_map(Value::as_f64).map(|x| x as f32).collect();
        return (c.len() >= 3).then(|| [c[0], c[1], c[2], c.get(3).copied().unwrap_or(1.0)]);
    }
    let s = v.as_str()?.strip_prefix('#')?;
    let h = |i: usize| u8::from_str_radix(s.get(i..i + 2)?, 16).ok();
    Some([studio_mesh::srgb_to_linear(h(0)?), studio_mesh::srgb_to_linear(h(2)?), studio_mesh::srgb_to_linear(h(4)?), 1.0])
}

fn surface(v: &Value) -> Option<Material> {
    if !["colour", "metallic", "roughness", "emissive", "subsurface"].iter().any(|k| v.get(*k).is_some()) {
        return None;
    }
    let mut m = Material::default();
    if let Some(c) = v.get("colour").and_then(colour) {
        m.base_colour = c;
    }
    if let Some(x) = v.get("metallic").and_then(Value::as_f64) {
        m.metallic = x as f32;
    }
    if let Some(x) = v.get("roughness").and_then(Value::as_f64) {
        m.roughness = x as f32;
    }
    if let Some(x) = v.get("subsurface").and_then(Value::as_f64) {
        m.subsurface = x as f32;
    }
    if let Some(n) = v.get("name").and_then(Value::as_str) {
        m.name = n.into();
    }
    Some(m)
}

/// Builds a recipe the way the `model` op does; `fineness` scales the
/// grid (1 = the op's own detail, larger = finer, for references).
pub fn build_recipe(r: &Value, fineness: f32) -> Result<Mesh, String> {
    use studio_mesh::sdf::{Join, Part, Shape};
    if let Some(shape) = r.get("shape").and_then(Value::as_str) {
        let size = r.get("size").and_then(Value::as_f64).unwrap_or(300.0) as f32;
        let detail = ((r.get("detail").and_then(Value::as_u64).unwrap_or(4) as f32) + (fineness.log2().max(0.0))).round() as usize;
        if shape == "head" {
            return Ok(studio_mesh::sdf::head(size, detail.clamp(1, 7)));
        }
        return studio_mesh::prim::by_name(shape, size, detail.min(7)).ok_or_else(|| format!("shape {shape}"));
    }
    let list = r.get("parts").and_then(Value::as_array).ok_or("a recipe has \"parts\" or \"shape\"")?;
    let base = surface(r).unwrap_or_default();
    let mut materials = vec![base];
    let mut parts = Vec::new();
    for v in list {
        let shape = Shape::parse(v.get("shape").and_then(Value::as_str).unwrap_or("ellipsoid")).ok_or("part shape")?;
        let size = match v.get("size") {
            Some(s) if s.is_number() => [s.as_f64().unwrap_or(0.0) as f32; 3],
            Some(s) => v3(s).ok_or("part size")?,
            None => [v.get("radius").and_then(Value::as_f64).map_or(50.0, |r| r as f32 * 2.0); 3],
        };
        let mut p = Part::new(shape, v.get("at").and_then(v3).unwrap_or([0.0; 3]), size);
        if shape == Shape::Capsule {
            p.at = [0.0; 3];
            p.a = v.get("from").and_then(v3).ok_or("capsule from")?;
            p.b = v.get("to").and_then(v3).ok_or("capsule to")?;
            let r = v.get("radius").and_then(Value::as_f64).map_or(size[0] * 0.5, |r| r as f32);
            p.size = [r * 2.0; 3];
        }
        p.turn = v.get("turn").and_then(v3).unwrap_or([0.0; 3]);
        p.round = v.get("round").and_then(Value::as_f64).unwrap_or(0.0) as f32;
        p.blend = v.get("blend").and_then(Value::as_f64).unwrap_or(0.0).max(0.0) as f32;
        p.join = if v.get("subtract").and_then(Value::as_bool) == Some(true) {
            Join::Subtract
        } else if v.get("intersect").and_then(Value::as_bool) == Some(true) {
            Join::Intersect
        } else {
            Join::Add
        };
        if let Some(mat) = surface(v) {
            let k = materials.iter().position(|m| m.base_colour == mat.base_colour && m.metallic == mat.metallic && m.roughness == mat.roughness).unwrap_or_else(|| {
                materials.push(mat.clone());
                materials.len() - 1
            });
            p.material = k as u32;
        }
        parts.push(p);
    }
    let extent = parts.iter().filter(|p| p.join == Join::Add).map(|p| studio_mesh::length(p.at) + p.size[0].max(p.size[1]).max(p.size[2]) + studio_mesh::length(p.a).max(studio_mesh::length(p.b))).fold(1.0f32, f32::max);
    let detail = r.get("detail").and_then(Value::as_u64).unwrap_or(4).clamp(1, 6) as usize;
    let cell = extent * 2.0 / ([60.0, 90.0, 130.0, 170.0, 220.0, 280.0][detail - 1] * fineness);
    let m = studio_mesh::sdf::build(&parts, materials, cell);
    if m.triangles() == 0 {
        return Err("the parts make no surface".into());
    }
    Ok(m)
}

#[derive(Clone, Debug, Serialize)]
pub struct CaseResult {
    pub id: String,
    pub category: String,
    pub kind: String,
    /// Pipeline → its score and bars.
    pub runs: Vec<Run>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Run {
    pub pipeline: String,
    pub overall: f32,
    pub passes: bool,
    pub bars_passed: usize,
    pub bars: usize,
    pub seconds: f32,
    pub detail: Value,
}

pub struct Settings {
    /// Where pictures of every result and the report go.
    pub out: PathBuf,
    /// Models made elsewhere from the same prompts (`<id>.glb|obj|stl`).
    pub outside_models: Option<PathBuf>,
    /// Pictures and SVGs to judge (`<id>.png`, `<id>.svg`).
    pub pictures: Option<PathBuf>,
    /// Only cases whose id contains this.
    pub only: Option<String>,
    /// Skip the production finish (score the raw pipeline only).
    pub raw_only: bool,
}

fn renders(m: &Mesh, path: &Path) {
    let (c, r) = crate::raster::frame(m);
    let views = [crate::raster::View { yaw: 25.0, pitch: 20.0 }, crate::raster::View { yaw: 115.0, pitch: 10.0 }, crate::raster::View { yaw: 205.0, pitch: 20.0 }, crate::raster::View { yaw: 0.0, pitch: 70.0 }];
    let pics: Vec<_> = views.iter().map(|&v| crate::raster::render(m, v, 256, c, r)).collect();
    let refs: Vec<_> = pics.iter().collect();
    let (w, h, rgba) = crate::raster::sheet(&refs, 4);
    let _ = crate::raster::write_png(path, w, h, &rgba);
}

fn model_run(name: &str, m: &Mesh, class: Class, reference: &Mesh, secs: f32, out: &Path, id: &str) -> Run {
    let r = critique_model(m, class, Some(reference));
    renders(m, &out.join(format!("{id}.{name}.png")));
    Run {
        pipeline: name.into(),
        overall: r.scores.overall,
        passes: r.passes(),
        bars_passed: r.bars.iter().filter(|b| b.pass).count(),
        bars: r.bars.len(),
        seconds: secs,
        detail: json!({ "summary": r.summary(), "report": r }),
    }
}

/// Runs the benchmark; returns every case's results.
pub fn run(s: &Settings) -> Result<Vec<CaseResult>, String> {
    let cases: Value = serde_json::from_str(CASES).map_err(|e| format!("cases.json: {e}"))?;
    std::fs::create_dir_all(&s.out).map_err(|e| format!("{}: {e}", s.out.display()))?;
    let mut results = Vec::new();
    for case in cases["models"].as_array().into_iter().flatten() {
        let id = case["id"].as_str().unwrap_or("case").to_owned();
        if s.only.as_ref().is_some_and(|o| !id.contains(o.as_str())) {
            continue;
        }
        let class = Class::parse(case["class"].as_str().unwrap_or("prop")).ok_or_else(|| format!("{id}: class"))?;
        let recipe = &case["recipe"];
        let t0 = std::time::Instant::now();
        let raw = build_recipe(recipe, 1.0).map_err(|e| format!("{id}: {e}"))?;
        let raw_secs = t0.elapsed().as_secs_f32();
        let reference = build_recipe(recipe, 2.2).map_err(|e| format!("{id}: {e}"))?;
        let mut runs = vec![model_run("raw", &raw, class, &reference, raw_secs, &s.out, &id)];
        if !s.raw_only {
            let t1 = std::time::Instant::now();
            let fin = crate::production::finish(&raw, Some(&reference), &crate::production::Options::new(class));
            let secs = raw_secs + t1.elapsed().as_secs_f32();
            let mut run = model_run("finished", &fin.mesh, class, &reference, secs, &s.out, &id);
            run.detail["attempts"] = serde_json::to_value(&fin.attempts).unwrap_or(Value::Null);
            runs.push(run);
        }
        if let Some(dir) = &s.outside_models {
            for ext in ["glb", "gltf", "obj", "stl"] {
                let p = dir.join(format!("{id}.{ext}"));
                if p.exists() {
                    let size = {
                        let (lo, hi) = reference.bounds();
                        (hi[0] - lo[0]).max(hi[1] - lo[1]).max(hi[2] - lo[2])
                    };
                    match studio_mesh::import::load(&p, size) {
                        Ok(got) => {
                            // Placed like the reference: centred on it.
                            let mut m = got.mesh;
                            let (rl, rh) = reference.bounds();
                            let (ml, mh) = m.bounds();
                            let shift: V3 = std::array::from_fn(|i| (rl[i] + rh[i]) * 0.5 - (ml[i] + mh[i]) * 0.5);
                            studio_mesh::ops::transform(&mut m, &studio_mesh::region::Region::All, shift, [0.0; 3], [1.0; 3], [0.0; 3]);
                            runs.push(model_run("outside", &m, class, &reference, 0.0, &s.out, &id));
                        }
                        Err(e) => eprintln!("{id}: {}: {e}", p.display()),
                    }
                    break;
                }
            }
        }
        eprintln!("{id}: {}", runs.iter().map(|r| format!("{} {:.1}{}", r.pipeline, r.overall, if r.passes { " ✓" } else { "" })).collect::<Vec<_>>().join(", "));
        results.push(CaseResult { id, category: case["category"].as_str().unwrap_or("").into(), kind: "model".into(), runs });
    }
    if let Some(dir) = &s.pictures {
        for case in cases["pictures"].as_array().into_iter().flatten() {
            let id = case["id"].as_str().unwrap_or("case").to_owned();
            if s.only.as_ref().is_some_and(|o| !id.contains(o.as_str())) {
                continue;
            }
            let mut runs = Vec::new();
            let png = dir.join(format!("{id}.png"));
            if png.exists() {
                let (w, h, rgba) = crate::raster::read_png(&png)?;
                let q = crate::image::analyse(w, h, &rgba);
                let faults = crate::image::faults(&q);
                let mut detail = json!({ "picture": q, "faults": faults });
                let mut overall = 100.0 - 12.0 * faults.len() as f32;
                // Text read back by whoever judged it (`<id>.txt`).
                if let Some(expected) = case["text"].as_str() {
                    let read = std::fs::read_to_string(dir.join(format!("{id}.txt"))).ok();
                    if let Some(read) = read {
                        let acc = crate::image::text_accuracy(expected, &read);
                        detail["text_accuracy"] = json!(acc);
                        overall -= (1.0 - acc) * 40.0;
                    }
                }
                runs.push(Run { pipeline: "picture".into(), overall: overall.max(0.0), passes: faults.is_empty(), bars_passed: 0, bars: 0, seconds: 0.0, detail });
            }
            let svg = dir.join(format!("{id}.svg"));
            if let Ok(doc) = std::fs::read_to_string(&svg) {
                let (shapes, view) = crate::vector::parse_svg(&doc);
                let mut q = crate::vector::analyse(&shapes, view);
                if png.exists() {
                    let (w, h, rgba) = crate::raster::read_png(&png)?;
                    let (iou, err) = crate::vector::compare(&shapes, view, w, h, &rgba);
                    q.fill_iou = Some(iou);
                    q.colour_error = Some(err);
                }
                let overall = (100.0 - 200.0 * q.kinks - 100.0 * q.redundant - 100.0 * q.tiny - 100.0 * q.wiggles - q.fill_iou.map_or(0.0, |i| (1.0 - i) * 100.0)).max(0.0);
                runs.push(Run { pipeline: "vector".into(), overall, passes: q.kinks < 0.05 && q.redundant < 0.02, bars_passed: 0, bars: 0, seconds: 0.0, detail: json!({ "vector": q }) });
            }
            if !runs.is_empty() {
                results.push(CaseResult { id, category: case["category"].as_str().unwrap_or("").into(), kind: "picture".into(), runs });
            }
        }
    }
    Ok(results)
}

/// Mean score and bars cleared per pipeline.
pub fn summary(results: &[CaseResult]) -> Value {
    let mut by: std::collections::BTreeMap<String, (f32, usize, usize, usize, usize)> = std::collections::BTreeMap::new();
    for c in results {
        for r in &c.runs {
            let e = by.entry(r.pipeline.clone()).or_default();
            e.0 += r.overall;
            e.1 += 1;
            e.2 += usize::from(r.passes);
            e.3 += r.bars_passed;
            e.4 += r.bars;
        }
    }
    Value::Object(by.into_iter().map(|(k, (s, n, p, bp, b))| (k, json!({ "cases": n, "mean_score": s / n.max(1) as f32, "cases_clearing_every_bar": p, "bars_cleared": format!("{bp}/{b}") }))).collect())
}

/// Appends one line per run to `history` (JSON lines), so scores can be
/// followed over time.
pub fn record(history: &Path, label: &str, results: &[CaseResult]) -> Result<(), String> {
    use std::io::Write;
    if let Some(dir) = history.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(history).map_err(|e| format!("{}: {e}", history.display()))?;
    let when = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let line = json!({ "time": when, "label": label, "summary": summary(results), "cases": results.iter().map(|c| json!({ "id": c.id, "runs": c.runs.iter().map(|r| json!({ "pipeline": r.pipeline, "score": r.overall, "passes": r.passes, "bars": format!("{}/{}", r.bars_passed, r.bars), "seconds": r.seconds })).collect::<Vec<_>>() })).collect::<Vec<_>>() });
    writeln!(f, "{line}").map_err(|e| e.to_string())
}

/// A readable report: a table per kind with every pipeline's score.
pub fn report_markdown(results: &[CaseResult]) -> String {
    let mut out = String::from("# Studio quality benchmark\n\n");
    out.push_str("| case | category | pipeline | score | bars | seconds | misses |\n|---|---|---|---:|---:|---:|---|\n");
    for c in results {
        for r in &c.runs {
            let misses = r.detail["report"]["faults"].as_array().map(|f| f.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("; ")).unwrap_or_default();
            out.push_str(&format!("| {} | {} | {} | {:.1} | {}/{} | {:.1} | {} |\n", c.id, c.category, r.pipeline, r.overall, r.bars_passed, r.bars, r.seconds, misses));
        }
    }
    out.push_str(&format!("\n## Summary\n\n```json\n{}\n```\n", serde_json::to_string_pretty(&summary(results)).unwrap_or_default()));
    out
}
