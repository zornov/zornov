// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `inspect {clip, what: "rig"}`: a rigged 3D model's skeleton as an agent
//! needs it to pose, constrain or fix it: every bone with its parent, rest
//! head and tail and length, the constraints with what they aim at, how
//! the skin is bound (which bones carry how much of the body, which carry
//! none) and the shape keys and actions. `inspect` had no rig section, so
//! an agent rigging a character guessed bone names and frames.

use super::Session;
use serde_json::{Value, json};
use studio_mesh::rig::{Constraint, Rig, Target};

/// Most bones listed one by one (a rig with more lists the rest by name).
const BONES_LISTED: usize = 80;

fn r1(v: f32) -> f64 {
    (v as f64 * 10.0).round() / 10.0
}

fn p3(v: [f32; 3]) -> Value {
    json!([r1(v[0]), r1(v[1]), r1(v[2])])
}

/// The rig described (see the module).
pub(super) fn report(r: &Rig) -> Value {
    let name = |i: usize| r.bones.get(i).map_or_else(|| format!("#{i}"), |b| b.name.clone());
    // How many vertices each bone carries most of.
    let mut carried = vec![0usize; r.bones.len()];
    let mut loose = 0usize;
    for (j, w) in r.joints.iter().zip(&r.weights) {
        let (k, wmax) = w.iter().enumerate().fold((0, 0.0f32), |a, (k, x)| if *x > a.1 { (k, *x) } else { a });
        if wmax <= 0.0 {
            loose += 1;
        } else if let Some(c) = carried.get_mut(j[k] as usize) {
            *c += 1;
        }
    }
    let verts = r.joints.len();
    let bones: Vec<Value> = r
        .bones
        .iter()
        .enumerate()
        .take(BONES_LISTED)
        .map(|(i, b)| {
            let len = ((b.tail[0] - b.head[0]).powi(2) + (b.tail[1] - b.head[1]).powi(2) + (b.tail[2] - b.head[2]).powi(2)).sqrt();
            let mut o = json!({"name": b.name, "head": p3(b.head), "tail": p3(b.tail), "length": r1(len)});
            if let Some(p) = b.parent {
                o["parent"] = json!(name(p));
            }
            if !b.deform {
                o["deform"] = json!(false);
            } else if r.is_skinned() {
                o["carries"] = json!(format!("{:.0}%", carried[i] as f64 * 100.0 / verts.max(1) as f64));
            }
            o
        })
        .collect();
    let target = |t: &Target| match t {
        Target::Point(p) => json!({"point": p3(*p)}),
        Target::Bone(i) => json!({"bone": name(*i)}),
    };
    let constraints: Vec<Value> = r
        .constraints
        .iter()
        .map(|c| {
            let mut o = json!({"name": c.name(), "kind": c.kind(), "bone": name(c.bone())});
            match c {
                Constraint::Ik { chain, target: t, pole, influence, .. } => {
                    o["chain"] = json!(chain);
                    o["target"] = target(t);
                    if let Some(p) = pole {
                        o["pole"] = p3(*p);
                    }
                    o["influence"] = json!(r1(*influence));
                }
                Constraint::LookAt { target: t, influence, .. } => {
                    o["target"] = target(t);
                    o["influence"] = json!(r1(*influence));
                }
                Constraint::Limit { min, max, .. } => {
                    o["min_degrees"] = p3(*min);
                    o["max_degrees"] = p3(*max);
                }
                Constraint::CopyRotation { from, influence, .. } => {
                    o["from"] = json!(name(*from));
                    o["influence"] = json!(r1(*influence));
                }
            }
            o
        })
        .collect();
    let idle: Vec<String> = r.bones.iter().enumerate().filter(|(i, b)| b.deform && r.is_skinned() && carried[*i] == 0).map(|(_, b)| b.name.clone()).collect();
    let mut skin = json!({"bound": r.is_skinned(), "vertices": verts, "dual_quaternion": r.dual_quat});
    if !r.is_skinned() {
        skin["fix"] = json!("the skin is not bound: rig again with bones (or a template) to bind it");
    } else {
        if !idle.is_empty() {
            skin["bones_carrying_nothing"] = json!(idle);
            skin["note"] = json!("a deforming bone that carries no vertex moves nothing when posed: move its head and tail into the part it should move, or set deform false");
        }
        if loose > 0 {
            skin["vertices_unweighted"] = json!(loose);
        }
    }
    let mut v = json!({
        "space": "bone heads, tails, IK points and poles are in the model's own px before the clip's transform: centred, y down, the character facing -z (the camera)",
        "bones": bones,
        "constraints": constraints,
        "skin": skin,
        "shape_keys": r.shapes.iter().map(|k| k.name.clone()).collect::<Vec<_>>(),
        "actions": r.actions.iter().map(|a| json!({"name": a.name, "seconds": (a.duration as f64 * 100.0).round() / 100.0})).collect::<Vec<_>>(),
    });
    if r.bones.len() > BONES_LISTED {
        v["more_bones"] = json!(r.bones.iter().skip(BONES_LISTED).map(|b| b.name.clone()).collect::<Vec<_>>());
    }
    v
}

impl Session {
    /// The rig section of `inspect`: the report, or what the clip lacks.
    pub(super) fn rig_section(&self, id: u32) -> Value {
        match self.rig_of(id) {
            Ok((_, _, rig)) => report(&rig),
            Err(e) => json!({"none": e}),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services;

    #[test]
    fn a_rigged_model_reports_its_bones_constraints_and_skin() {
        let mut s = Session::new(services::project::new_custom("Rig", 640, 360, 30.0));
        s.edit(&[json!({"op": "model", "shape": "cube", "size": 100, "detail": 2, "duration": 2})], "m", false).unwrap();
        let id = s.doc.project.clips.iter().map(|c| c.id).max().unwrap();
        // Before a rig: the section says how to get one.
        let v = s.rig_section(id);
        assert!(v["none"].as_str().unwrap().contains("rig it first"), "{v}");
        s.edit(&[json!({"op": "rig", "clip": id, "template": "chain", "count": 3})], "rig", false).unwrap();
        let v = s.rig_section(id);
        let bones = v["bones"].as_array().unwrap();
        assert_eq!(bones.len(), 3, "{v}");
        assert!(bones[1]["parent"].is_string() && bones[0]["head"].as_array().unwrap().len() == 3, "{v}");
        assert_eq!(v["skin"]["bound"], true, "{v}");
        assert!(v["space"].as_str().unwrap().contains("model's own px"));
        // Through the tool.
        let r = s.run("inspect", &json!({"clip": id, "what": "rig"})).unwrap();
        let t: String = r.into_iter().filter_map(|c| if let super::super::Content::Text(t) = c { Some(t) } else { None }).collect();
        let v: Value = serde_json::from_str(&t).unwrap();
        assert!(v["rig"]["bones"].as_array().is_some_and(|b| b.len() == 3), "{v}");
    }
}
