// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Characters that act in a world: a rigged model posed for a clip at a
//! moment (walk, run, idle, wave, talk, dance...), in metres, y up, standing
//! on the ground.

use crate::asset::{Asset, mesh_from_studio};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use studio_mesh::Mesh;
use studio_mesh::rig::PoseInput;

fn with_clip() -> &'static Mutex<HashMap<(u64, String), Arc<Mesh>>> {
    static C: OnceLock<Mutex<HashMap<(u64, String), Arc<Mesh>>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The rest model with the clip's action made for its rig.
fn prepared(asset: &Asset, clip: &str) -> Option<Arc<Mesh>> {
    let (rest, _, _) = asset.rigged.as_ref()?;
    let key = (asset.key, clip.to_string());
    if let Some(m) = with_clip().lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return Some(m.clone());
    }
    let rig = rest.rig.as_ref()?;
    let mesh = if rig.action(clip).is_some() {
        rest.clone()
    } else {
        let action = studio_mesh::motion::make(rig, clip, &studio_mesh::motion::Style::default()).ok()?;
        let mut r = (**rig).clone();
        r.actions.push(action);
        let mut m = (**rest).clone();
        m.rig = Some(Arc::new(r));
        Arc::new(m)
    };
    with_clip().lock().unwrap_or_else(|e| e.into_inner()).insert(key, mesh.clone());
    Some(mesh)
}

/// Whether a clip can be played on this character.
pub fn can_play(asset: &Asset, clip: &str) -> bool {
    prepared(asset, clip).is_some()
}

/// The character posed `t` seconds into `clip` (looping), in metres with
/// its feet on y = 0; `None` for an unknown clip or a model with no rig.
pub fn posed(asset: &Asset, clip: &str, t: f32) -> Option<Mesh> {
    let m = prepared(asset, clip)?;
    let (_, k, lift) = asset.rigged.as_ref()?;
    let input = PoseInput { action: Some((clip.to_string(), t.max(0.0), true)), ..PoseInput::default() };
    let rig = m.rig.as_ref()?;
    let pm = rig.posed_mesh(&m, &input);
    let mut out = mesh_from_studio(&pm, *k);
    for p in out.positions.iter_mut() {
        p[1] -= *lift;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::{CharJ, NoLibrary, WorldSpec, build};
    use crate::world::ObjectKind;

    fn world_with_walker() -> crate::world::World {
        let mut s = WorldSpec { preset: "meadow".into(), size: Some(100.0), quality: "fast".into(), seed: Some(2), ..WorldSpec::default() };
        s.scatter = vec![crate::build::ScatterJ { kind: "bush".into(), count: 3, size: 1.0, ..Default::default() }];
        s.characters = vec![CharJ { name: "Ana".into(), at: [0.0, 0.0], yaw: 0.0, clip: "walk".into(), walk_to: vec![[0.0, 12.0], [10.0, 12.0]], ..CharJ::default() }];
        build(&s, &NoLibrary).unwrap()
    }

    #[test]
    fn a_character_walks_its_route_turns_at_the_corner_and_idles_when_it_arrives() {
        let w = world_with_walker();
        let i = w.objects.iter().position(|o| o.kind == ObjectKind::Character).unwrap();
        assert!(w.is_performer(&w.objects[i]));
        let start = w.performer_state(i, 0.0).unwrap();
        assert!((start.inst.pos[2]).abs() < 1e-3 && start.clip == "walk");
        // 1.4 m/s: after 5 s it is 7 m along, heading +z.
        let mid = w.performer_state(i, 5.0).unwrap();
        assert!((mid.inst.pos[2] - 7.0).abs() < 0.05 && mid.inst.yaw.abs() < 1.0, "{:?}", mid.inst);
        // After the corner it heads +x (yaw 90).
        let after = w.performer_state(i, 12.0).unwrap();
        assert!((after.inst.yaw - 90.0).abs() < 1.0 && after.inst.pos[0] > 0.5, "{:?}", after.inst);
        let done = w.performer_state(i, 60.0).unwrap();
        assert_eq!(done.clip, "idle");
        assert!((done.inst.pos[0] - 10.0).abs() < 1e-3 && (done.inst.pos[2] - 12.0).abs() < 1e-3);
        // Feet on the ground all along.
        for t in [0.0, 3.0, 9.0, 14.0] {
            let p = w.performer_state(i, t).unwrap();
            assert!((p.inst.pos[1] - w.ground_y(p.inst.pos[0], p.inst.pos[2])).abs() < 0.05);
        }
    }

    #[test]
    fn posing_moves_the_limbs_and_keeps_the_feet_on_the_ground() {
        let w = world_with_walker();
        let o = w.objects.iter().find(|o| o.kind == ObjectKind::Character).unwrap();
        let a = &w.assets[o.asset];
        assert!(can_play(a, "walk") && can_play(a, "wave") && !can_play(a, "moonwalk"));
        let m0 = posed(a, "walk", 0.0).unwrap();
        let m1 = posed(a, "walk", 0.3).unwrap();
        assert_eq!(m0.positions.len(), m1.positions.len());
        let moved = m0.positions.iter().zip(&m1.positions).filter(|(p, q)| crate::math::dist(**p, **q) > 0.02).count();
        assert!(moved > m0.positions.len() / 20, "{moved} vertices moved");
        let b = crate::asset::mesh_bounds(&m0);
        assert!(b.min[1].abs() < 0.12 && b.max[1] > 1.5 && b.max[1] < 2.0, "{b:?}");
        // Faces point out (a closed body: more out than in).
        let c = b.centre();
        let outward = (0..m0.triangles()).filter(|t| {
            let [p, q, r] = m0.tri(*t).map(|i| m0.positions[i]);
            let n = crate::math::cross(crate::math::sub(q, p), crate::math::sub(r, p));
            crate::math::dot(n, crate::math::sub(p, c)) > 0.0
        }).count();
        assert!(outward * 10 > m0.triangles() * 5, "{outward} of {}", m0.triangles());
    }
}
