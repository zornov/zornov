// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Models that move by themselves: a rigged model posed for the moment
//! being drawn (its action, bone turns, shape keys, IK and look-at
//! targets), and a baked simulation played over a model (cloth, soft
//! bodies, hair, shattered pieces, particles).
//!
//! What a model layer's parameters say (all animatable where numbers):
//! - `action`, `action_start` (clip seconds it starts at), `action_speed`,
//!   `action_offset` (seconds into it), `action_loop`; `blend_action` and
//!   `blend` (0..1) mix a second one over it;
//! - `bone.<bone>.x|y|z`: degrees on top of the action (x swings forward,
//!   y twists, z swings sideways); `bone.<bone>.lx|ly|lz`: a shift (px);
//! - `shape.<key>`: a shape key's weight;
//! - `ik.<name>.x|y|z`: where an IK chain reaches (model px) and
//!   `ik.<name>.on` how much (0..1); `look.x|y|z` and `look.on`: where the
//!   head (and eyes) look;
//! - `cache` (a `.scache` file), `cache_start`, `cache_speed`: a baked
//!   simulation; `smoke_colour`, `smoke_density`, `fire` for smoke.
//!
//! Posed models are kept for a while by what posed them, so scrubbing
//! back and forth does not pose them again.

use crate::scene::{Param, Params, ParamsExt};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use studio_mesh::Mesh;
use studio_mesh::cache::{Cache, Frames};
use studio_mesh::rig::PoseInput;

type Lru<T> = Mutex<(HashMap<String, Arc<T>>, Vec<String>)>;

fn posed_cache() -> &'static Lru<Mesh> {
    static C: OnceLock<Lru<Mesh>> = OnceLock::new();
    C.get_or_init(|| Mutex::new((HashMap::new(), Vec::new())))
}

fn sim_cache() -> &'static Lru<Cache> {
    static C: OnceLock<Lru<Cache>> = OnceLock::new();
    C.get_or_init(|| Mutex::new((HashMap::new(), Vec::new())))
}

fn get<T>(lru: &Lru<T>, key: &str) -> Option<Arc<T>> {
    let mut g = lru.lock().unwrap_or_else(|e| e.into_inner());
    let hit = g.0.get(key).cloned();
    if hit.is_some() {
        // Most recent last.
        if let Some(i) = g.1.iter().position(|k| k == key) {
            let k = g.1.remove(i);
            g.1.push(k);
        }
    }
    hit
}

fn put<T>(lru: &Lru<T>, key: String, v: Arc<T>, cap: usize) {
    let mut g = lru.lock().unwrap_or_else(|e| e.into_inner());
    if g.0.insert(key.clone(), v).is_none() {
        g.1.push(key);
    }
    while g.1.len() > cap {
        let old = g.1.remove(0);
        g.0.remove(&old);
    }
}

/// A baked simulation by file (read once, kept while in use).
pub fn load_cache(path: &str) -> Option<Arc<Cache>> {
    if let Some(c) = get(sim_cache(), path) {
        return Some(c);
    }
    let c = Arc::new(Cache::load(std::path::Path::new(path)).ok()?);
    put(sim_cache(), path.to_string(), c.clone(), 8);
    Some(c)
}

/// What poses the rig at `t` seconds into the clip.
pub fn pose_input(params: &Params, t: f32) -> PoseInput {
    let mut input = PoseInput::default();
    let action = params.text("action", "");
    if !action.is_empty() {
        let at = (t - params.f("action_start", 0.0)) * params.f("action_speed", 1.0) + params.f("action_offset", 0.0);
        input.action = Some((action.to_string(), at, params.flag("action_loop", true)));
    }
    let blend = params.text("blend_action", "");
    let k = params.f("blend", 0.0);
    if !blend.is_empty() && k > 0.0 {
        let at = (t - params.f("action_start", 0.0)) * params.f("blend_speed", params.f("action_speed", 1.0)) + params.f("blend_offset", 0.0);
        input.blend = Some((blend.to_string(), at, params.flag("action_loop", true), k.min(1.0)));
    }
    let comp = |c: &str| match c {
        "x" | "lx" => Some(0),
        "y" | "ly" => Some(1),
        "z" | "lz" => Some(2),
        _ => None,
    };
    let mut look: Option<[f32; 3]> = None;
    let mut look_on: Option<f32> = None;
    for (key, v) in params {
        let Some(x) = v.as_f32() else { continue };
        if let Some(rest) = key.strip_prefix("bone.") {
            if let Some((bone, c)) = rest.rsplit_once('.')
                && let Some(i) = comp(c)
            {
                let slot = if c.starts_with('l') { input.offsets.entry(bone.to_string()).or_insert([0.0; 3]) } else { input.rotations.entry(bone.to_string()).or_insert([0.0; 3]) };
                slot[i] = x;
            }
        } else if let Some(name) = key.strip_prefix("shape.") {
            input.shapes.insert(name.to_string(), x);
        } else if let Some(rest) = key.strip_prefix("ik.") {
            if let Some((name, c)) = rest.rsplit_once('.') {
                if c == "on" {
                    input.influences.insert(name.to_string(), x.clamp(0.0, 1.0));
                } else if let Some(i) = comp(c) {
                    input.targets.entry(name.to_string()).or_insert([0.0; 3])[i] = x;
                }
            }
        } else if let Some(c) = key.strip_prefix("look.") {
            if c == "on" {
                look_on = Some(x.clamp(0.0, 1.0));
            } else if let Some(i) = comp(c) {
                look.get_or_insert([0.0; 3])[i] = x;
            }
        }
    }
    if let Some(p) = look {
        for name in ["look", "eyes.L", "eyes.R"] {
            input.targets.insert(name.into(), p);
            input.influences.insert(name.into(), look_on.unwrap_or(1.0));
        }
    } else if let Some(on) = look_on {
        for name in ["look", "eyes.L", "eyes.R"] {
            input.influences.insert(name.into(), on);
        }
    }
    input
}

/// A stable key for a pose (rounded numbers, sorted names).
fn pose_key(input: &PoseInput) -> String {
    let r = |v: f32| (v * 1000.0).round() as i64;
    let mut parts: Vec<String> = Vec::new();
    if let Some((a, t, l)) = &input.action {
        parts.push(format!("a{a}@{}{}", r(*t), *l as u8));
    }
    if let Some((a, t, l, k)) = &input.blend {
        parts.push(format!("b{a}@{}{}x{}", r(*t), *l as u8, r(*k)));
    }
    let mut add_map = |tag: &str, m: &HashMap<String, [f32; 3]>| {
        let mut v: Vec<_> = m.iter().map(|(k, x)| format!("{tag}{k}={},{},{}", r(x[0]), r(x[1]), r(x[2]))).collect();
        v.sort();
        parts.extend(v);
    };
    add_map("r", &input.rotations);
    add_map("o", &input.offsets);
    add_map("t", &input.targets);
    let mut add_f = |tag: &str, m: &HashMap<String, f32>| {
        let mut v: Vec<_> = m.iter().map(|(k, x)| format!("{tag}{k}={}", r(*x))).collect();
        v.sort();
        parts.extend(v);
    };
    add_f("s", &input.shapes);
    add_f("i", &input.influences);
    parts.join(";")
}

/// The model as it is at `t` seconds into its clip, and a key naming that
/// state (for the layer cache): the model itself when nothing moves it.
pub fn animate(mesh: Arc<Mesh>, file: &str, params: &Params, t: f32) -> (Arc<Mesh>, String) {
    let cache_file = params.text("cache", "");
    let input = if mesh.rig.is_some() { pose_input(params, t) } else { PoseInput::default() };
    let sim_t = (t - params.f("cache_start", 0.0)) * params.f("cache_speed", 1.0);
    if cache_file.is_empty() && input.is_rest() {
        return (mesh, file.to_string());
    }
    let mut key = format!("{file}|{}", pose_key(&input));
    if !cache_file.is_empty() {
        key.push_str(&format!("|{cache_file}@{}", (sim_t * 1000.0).round() as i64));
    }
    if let Some(m) = get(posed_cache(), &key) {
        return (m, key);
    }
    let mut out: Mesh = match &mesh.rig {
        Some(rig) if !input.is_rest() => rig.posed_mesh(&mesh, &input),
        _ => (*mesh).clone(),
    };
    if !cache_file.is_empty()
        && let Some(cache) = load_cache(cache_file)
    {
        play(&mut out, &mesh, &cache, sim_t.max(0.0));
        if let Some(v) = out.volume.as_mut() {
            let v = Arc::make_mut(v);
            let c = params.colour("smoke_colour", [v.look.colour[0], v.look.colour[1], v.look.colour[2], 1.0]);
            v.look.colour = [c[0], c[1], c[2]];
            v.look.density = params.f("smoke_density", v.look.density);
            v.look.fire = params.f("fire", v.look.fire);
        }
    }
    let out = Arc::new(out);
    put(posed_cache(), key.clone(), out.clone(), 24);
    (out, key)
}

/// Lays a baked frame over the model.
fn play(out: &mut Mesh, rest: &Mesh, cache: &Cache, t: f32) {
    match &cache.data {
        Frames::Vertices { count, .. } if *count == out.positions.len() => {
            if let Some(p) = cache.vertices_at(t) {
                out.positions = p;
                studio_mesh::ops::smooth_normals_in_place(out);
            }
        }
        Frames::Pieces { ranges, .. } => {
            if let Some(xf) = cache.pieces_at(t) {
                for (r, (q, d)) in ranges.iter().zip(xf) {
                    let x = studio_mesh::rig::Xf::new(q, d);
                    for v in r.0 as usize..(r.0 + r.1) as usize {
                        if let (Some(p), Some(n)) = (rest.positions.get(v), rest.normals.get(v)) {
                            out.positions[v] = x.apply(*p);
                            out.normals[v] = x.apply_dir(*n);
                        }
                    }
                }
            }
        }
        Frames::Points(frames) => {
            let (i, _) = cache.frame_at(t);
            if let Some(points) = frames.get(i) {
                *out = studio_mesh::ops::instance_points(rest, points);
            }
        }
        Frames::Volume { dims, min, max, data } => {
            let (i, u) = cache.frame_at(t);
            if let Some(a) = data.get(i) {
                let b = data.get(i + 1).unwrap_or(a);
                let cells: Vec<[u8; 2]> = a.iter().zip(b).map(|(x, y)| [(x[0] as f32 + (y[0] as f32 - x[0] as f32) * u) as u8, (x[1] as f32 + (y[1] as f32 - x[1] as f32) * u) as u8]).collect();
                let look = out.volume.as_ref().map(|v| v.look).unwrap_or_default();
                out.volume = Some(Arc::new(studio_mesh::Volume { dims: *dims, min: *min, max: *max, cells, look }));
            }
        }
        _ => {}
    }
}

/// Whether a parameter name is one this module reads.
pub fn is_anim_param(name: &str) -> bool {
    matches!(name, "action" | "action_start" | "action_speed" | "action_offset" | "action_loop" | "blend_action" | "blend" | "blend_speed" | "blend_offset" | "cache" | "cache_start" | "cache_speed" | "smoke_colour" | "smoke_density" | "fire")
        || ["bone.", "shape.", "ik.", "look."].iter().any(|p| name.starts_with(p))
}

/// A number parameter (for tests and callers building params).
pub fn num(v: f64) -> Param {
    Param::Float(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rigged_model_poses_from_its_parameters() {
        let mut m = studio_mesh::character::character(300.0, &studio_mesh::character::Build { detail: 1, face: true, ..Default::default() });
        let rig = Arc::make_mut(m.rig.as_mut().unwrap());
        rig.actions.push(studio_mesh::motion::make(rig, "wave", &Default::default()).unwrap());
        let m = Arc::new(m);
        let mut p = Params::new();
        // At rest: the same model back.
        let (same, key) = animate(m.clone(), "f.smesh", &p, 0.5);
        assert!(Arc::ptr_eq(&same, &m) && key == "f.smesh");
        p.insert("action".into(), Param::Text("wave".into()));
        p.insert("bone.head.y".into(), num(30.0));
        p.insert("shape.jawOpen".into(), num(1.0));
        let (a, ka) = animate(m.clone(), "f.smesh", &p, 0.25);
        let (b, kb) = animate(m.clone(), "f.smesh", &p, 0.25);
        assert!(Arc::ptr_eq(&a, &b) && ka == kb, "kept for the same moment");
        let (c, kc) = animate(m.clone(), "f.smesh", &p, 0.75);
        assert_ne!(ka, kc);
        assert!(a.positions != c.positions && a.positions != m.positions);
        let input = pose_input(&p, 0.25);
        assert_eq!(input.rotations.get("head"), Some(&[0.0, 30.0, 0.0]));
    }

    #[test]
    fn a_baked_cloth_plays_over_its_model() {
        let m = Arc::new(studio_mesh::prim::plane(100.0, 100.0, 2));
        let n = m.positions.len();
        let frames: Vec<[f32; 3]> = (0..2).flat_map(|f| m.positions.iter().map(move |p| [p[0], p[1] + f as f32 * 50.0, p[2]])).collect();
        let cache = Cache { fps: 10.0, frames: 2, data: Frames::Vertices { count: n, data: frames } };
        let dir = std::env::temp_dir().join(format!("studio-anim-cache-{}", std::process::id()));
        let path = dir.join("cloth.scache");
        cache.save(&path).unwrap();
        let mut p = Params::new();
        p.insert("cache".into(), Param::Text(path.to_string_lossy().into_owned()));
        let (out, _) = animate(m.clone(), "plane", &p, 0.05);
        let _ = std::fs::remove_dir_all(&dir);
        assert!((out.positions[0][1] - m.positions[0][1] - 25.0).abs() < 1e-3);
    }
}
