// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Procedural geometry as a graph of nodes: each node makes or changes a
//! mesh from its inputs and settings, so a model is a recipe that can be
//! run again with other numbers (more windows, a taller tower, another
//! seed). A part can keep a graph instead of a fixed mesh.
//!
//! Nodes (`op`): `shape`, `curve` (sweep), `loft`, `spin`, `outline`,
//! `transform`, `join`, `array`, `radial`, `mirror`, `boolean`, `subdivide`,
//! `bevel`, `extrude`, `inset`, `solidify`, `displace`, `smooth`,
//! `instance` (copies of one input at the points of another), `remesh`,
//! `material` (sets the slot).

use crate::modifiers::{self, Modifier, NoParts};
use crate::poly::Poly;
use crate::select::{Edges, Faces};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use studio_mesh::{V3, add};

/// One node.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub id: String,
    pub op: String,
    /// Ids of the nodes it reads.
    #[serde(default)]
    pub inputs: Vec<String>,
    /// Its settings (as the op takes them).
    #[serde(default)]
    pub with: Value,
}

/// A graph: nodes in any order; `output` names the result.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Graph {
    pub nodes: Vec<Node>,
    pub output: String,
    /// Named numbers the nodes can use as "$name" (the knobs).
    #[serde(default)]
    pub params: HashMap<String, f64>,
}

fn resolve(v: &Value, params: &HashMap<String, f64>) -> Value {
    match v {
        Value::String(s) if s.starts_with('$') => params.get(&s[1..]).map(|x| serde_json::json!(x)).unwrap_or(v.clone()),
        Value::Array(a) => Value::Array(a.iter().map(|x| resolve(x, params)).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, x)| (k.clone(), resolve(x, params))).collect()),
        _ => v.clone(),
    }
}

fn num(v: &Value, k: &str, d: f32) -> f32 {
    v.get(k).and_then(Value::as_f64).map_or(d, |x| x as f32)
}

fn v3(v: &Value, k: &str) -> Option<V3> {
    let a = v.get(k)?.as_array()?;
    (a.len() == 3).then(|| [a[0].as_f64().unwrap_or(0.0) as f32, a[1].as_f64().unwrap_or(0.0) as f32, a[2].as_f64().unwrap_or(0.0) as f32])
}

fn faces(v: &Value) -> Result<Faces, String> {
    match v.get("faces") {
        None => Ok(Faces::All),
        Some(Value::String(s)) => Faces::named(s).ok_or_else(|| format!("faces \"{s}\"")),
        Some(Value::Object(o)) if o.contains_key("facing") => Ok(Faces::Facing { direction: v3(&v["faces"], "facing").unwrap_or([0.0, -1.0, 0.0]), degrees: num(&v["faces"], "degrees", 45.0) }),
        Some(_) => Err("faces in a node: a word (top, front…) or {\"facing\": [x,y,z], \"degrees\"}".into()),
    }
}

impl Graph {
    /// Runs the graph.
    pub fn eval(&self) -> Result<Poly, String> {
        let mut done: HashMap<String, Poly> = HashMap::new();
        let mut stack: Vec<String> = Vec::new();
        self.node(&self.output, &mut done, &mut stack)
    }

    fn node(&self, id: &str, done: &mut HashMap<String, Poly>, stack: &mut Vec<String>) -> Result<Poly, String> {
        if let Some(p) = done.get(id) {
            return Ok(p.clone());
        }
        if stack.iter().any(|s| s == id) {
            return Err(format!("node \"{id}\" depends on itself"));
        }
        let n = self.nodes.iter().find(|n| n.id == id).ok_or_else(|| format!("no node \"{id}\""))?;
        stack.push(id.into());
        let inputs: Vec<Poly> = n.inputs.iter().map(|i| self.node(i, done, stack)).collect::<Result<_, _>>()?;
        stack.pop();
        let w = resolve(&n.with, &self.params);
        let first = || inputs.first().cloned().ok_or_else(|| format!("node \"{id}\" ({}) needs an input", n.op));
        let run = |p: Poly, m: Modifier| -> Result<Poly, String> { Ok(modifiers::apply(&p, &[m], &NoParts)?.poly) };
        let out = match n.op.as_str() {
            "shape" => {
                let name = w.get("shape").and_then(Value::as_str).unwrap_or("cube");
                let size = num(&w, "size", 100.0);
                let mut p = crate::prim::by_name(name, size, w.get("detail").and_then(Value::as_u64).unwrap_or(1) as usize).ok_or_else(|| format!("shape \"{name}\""))?;
                if let Some(s) = v3(&w, "scale") {
                    p.map_positions(|q| [q[0] * s[0], q[1] * s[1], q[2] * s[2]]);
                }
                p
            }
            "curve" => {
                let c: crate::curves::Curve = serde_json::from_value(w.get("curve").cloned().unwrap_or(Value::Null)).map_err(|e| format!("node \"{id}\": curve: {e}"))?;
                let pr: crate::curves::Profile = serde_json::from_value(w.get("profile").cloned().unwrap_or(Value::Null)).map_err(|e| format!("node \"{id}\": profile: {e}"))?;
                let along: crate::curves::Along = w.get("along").map(|a| serde_json::from_value(a.clone())).transpose().map_err(|e| format!("node \"{id}\": along: {e}"))?.unwrap_or_default();
                crate::curves::sweep(&c, &pr, &along, w.get("samples").and_then(Value::as_u64).unwrap_or(8) as usize)
            }
            "spin" => {
                let prof: Vec<V3> = w.get("profile").and_then(Value::as_array).map(|a| a.iter().filter_map(|q| q.as_array().map(|x| [x[0].as_f64().unwrap_or(0.0) as f32, x.get(1).and_then(Value::as_f64).unwrap_or(0.0) as f32, 0.0])).collect()).unwrap_or_default();
                crate::edit::spin(&prof, num(&w, "degrees", 360.0), w.get("steps").and_then(Value::as_u64).unwrap_or(32) as usize, 1)
            }
            "transform" => {
                let mut p = first()?;
                let all = vec![true; p.positions.len()];
                crate::edit::transform(&mut p, &all, v3(&w, "move").unwrap_or([0.0; 3]), v3(&w, "turn").unwrap_or([0.0; 3]), v3(&w, "scale").unwrap_or([1.0; 3]), v3(&w, "pivot").unwrap_or([0.0; 3]), 0.0, crate::edit::Falloff::Smooth);
                p
            }
            "join" => {
                let mut it = inputs.into_iter();
                let mut p = it.next().ok_or_else(|| format!("node \"{id}\" (join) needs inputs"))?;
                for q in it {
                    let mats = p.materials.len();
                    p.merge(&q);
                    let _ = mats;
                }
                p
            }
            "array" => run(first()?, Modifier::Array { count: num(&w, "count", 3.0) as usize, offset: v3(&w, "offset").unwrap_or([0.0; 3]), relative: v3(&w, "relative").unwrap_or([0.0; 3]), merge: w.get("merge").and_then(Value::as_bool).unwrap_or(false) })?,
            "radial" => run(first()?, Modifier::Radial { count: num(&w, "count", 6.0) as usize, axis: num(&w, "axis", 1.0) as usize, degrees: num(&w, "degrees", 360.0), centre: v3(&w, "centre").unwrap_or([0.0; 3]) })?,
            "mirror" => run(first()?, Modifier::Mirror { axes: [w.get("x").and_then(Value::as_bool).unwrap_or(true), w.get("y").and_then(Value::as_bool).unwrap_or(false), w.get("z").and_then(Value::as_bool).unwrap_or(false)], bisect: false, weld: num(&w, "weld", 0.0) })?,
            "boolean" => {
                if inputs.len() != 2 {
                    return Err(format!("node \"{id}\" (boolean) takes two inputs"));
                }
                let op = studio_mesh::csg::Op::parse(w.get("mode").and_then(Value::as_str).unwrap_or("subtract")).ok_or("boolean mode is union, subtract or intersect")?;
                modifiers::boolean(&inputs[0], &inputs[1], op)
            }
            "subdivide" => crate::subd::subdivide(&first()?, num(&w, "levels", 1.0).clamp(0.0, 5.0) as usize),
            "bevel" => {
                let mut p = first()?;
                let e = Edges::Sharper { degrees: num(&w, "angle", 30.0) }.list(&p);
                crate::bevel::bevel(&mut p, &e, num(&w, "width", 2.0), num(&w, "segments", 2.0) as usize, num(&w, "profile", 0.5));
                p
            }
            "extrude" | "inset" => {
                let mut p = first()?;
                let pick = faces(&w)?.mask(&p);
                if n.op == "extrude" {
                    crate::edit::extrude(&mut p, &pick, num(&w, "distance", 10.0), None, w.get("individual").and_then(Value::as_bool).unwrap_or(false));
                } else {
                    crate::edit::inset(&mut p, &pick, num(&w, "thickness", 5.0), num(&w, "depth", 0.0), w.get("individual").and_then(Value::as_bool).unwrap_or(false));
                }
                p.compact();
                p
            }
            "solidify" => run(first()?, Modifier::Solidify { thickness: num(&w, "thickness", 2.0), offset: num(&w, "offset", 0.0), rim: true })?,
            "displace" => run(first()?, Modifier::Displace { amount: num(&w, "amount", 5.0), size: num(&w, "size", 50.0), octaves: num(&w, "octaves", 4.0) as usize, seed: num(&w, "seed", 0.0) as u32 })?,
            "smooth" => run(first()?, Modifier::Smooth { iterations: num(&w, "iterations", 5.0) as usize, factor: num(&w, "factor", 0.5) })?,
            "remesh" => crate::remesh::voxel(&first()?, num(&w, "size", 4.0))?,
            "material" => {
                let mut p = first()?;
                let slot = num(&w, "slot", 0.0) as u32;
                for m in &mut p.face_material {
                    *m = slot;
                }
                p.ensure_materials();
                p
            }
            "instance" => {
                // Copies of input 0 at the vertices of input 1.
                if inputs.len() != 2 {
                    return Err(format!("node \"{id}\" (instance) takes the copy and the points"));
                }
                let mut out = Poly::new();
                let k = num(&w, "scale", 1.0);
                for q in &inputs[1].positions {
                    let mut c = inputs[0].clone();
                    c.map_positions(|v| add([v[0] * k, v[1] * k, v[2] * k], *q));
                    out.merge(&c);
                }
                out.materials = inputs[0].materials.clone();
                out.ensure_materials();
                out
            }
            other => return Err(format!("node \"{id}\": no op \"{other}\" (shape, curve, spin, transform, join, array, radial, mirror, boolean, subdivide, bevel, extrude, inset, solidify, displace, smooth, remesh, material, instance)")),
        };
        done.insert(id.into(), out.clone());
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_column_with_knobs() {
        let g: Graph = serde_json::from_value(json!({
            "output": "column",
            "params": {"height": 300, "flutes": 12},
            "nodes": [
                {"id": "shaft", "op": "shape", "with": {"shape": "cylinder", "size": 60, "scale": [1, 5, 1], "detail": 3}},
                {"id": "groove", "op": "shape", "with": {"shape": "cylinder", "size": 10, "scale": [1, 32, 1]}},
                {"id": "groove_out", "op": "transform", "inputs": ["groove"], "with": {"move": [30, 0, 0]}},
                {"id": "grooves", "op": "radial", "inputs": ["groove_out"], "with": {"count": "$flutes"}},
                {"id": "fluted", "op": "boolean", "inputs": ["shaft", "grooves"], "with": {"mode": "subtract"}},
                {"id": "column", "op": "bevel", "inputs": ["fluted"], "with": {"width": 1, "segments": 1, "angle": 60}}
            ]
        })).unwrap();
        let p = g.eval().unwrap();
        assert!(p.faces() > 50);
        let (lo, hi) = p.bounds();
        assert!(hi[1] - lo[1] > 250.0);
        let mut loop_g = g.clone();
        loop_g.nodes[0].inputs = vec!["column".into()];
        assert!(loop_g.eval().unwrap_err().contains("itself"));
    }
}
