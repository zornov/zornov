// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Motion capture onto any rig: BVH files read into an armature and an
//! action, and actions moved from one rig to another (retargeting) by
//! bone role, so a capture from a suit, a library or another character
//! drives Studio's characters.
//!
//! Bones are matched by role: names from Mixamo (`mixamorig:LeftUpLeg`),
//! CMU and other BVH libraries (`LeftUpLeg`, `lThigh`, `LHipJoint`),
//! Unreal and Unity humanoids (`thigh_l`, `LeftUpperLeg`), Blender
//! (`thigh.L`) and Studio's own all come to the same roles. Retargeting
//! copies each bone's turn in the world from its rest pose, corrected for
//! rigs whose rest poses differ (a T-pose capture onto an A-pose
//! character), and scales the body's travel by the hip heights.

use crate::rig::{Action, Bone, BonePose, Quat, Rig, Xf, q_conj, q_from_to, q_mul, q_normalise};
use crate::{V3, add, scale, sub};

/// The Studio role of a bone name, if it has one (`thigh.L`, `head`…).
pub fn role(name: &str) -> Option<String> {
    let raw = name.rsplit([':', '|']).next().unwrap_or(name);
    let lower = raw.to_ascii_lowercase().replace([' ', '-'], "_");
    let lower = lower.trim_start_matches("def_").trim_start_matches("bip01_").trim_start_matches("bip001_").trim_start_matches("b_").to_string();
    // Which side, and the name without it.
    let (side, part): (Option<&str>, String) = {
        let l = lower.as_str();
        if let Some(r) = l.strip_prefix("left") {
            (Some("L"), r.trim_start_matches('_').to_string())
        } else if let Some(r) = l.strip_prefix("right") {
            (Some("R"), r.trim_start_matches('_').to_string())
        } else if let Some(r) = l.strip_suffix(".l").or_else(|| l.strip_suffix("_l")) {
            (Some("L"), r.to_string())
        } else if let Some(r) = l.strip_suffix(".r").or_else(|| l.strip_suffix("_r")) {
            (Some("R"), r.to_string())
        } else if let Some(r) = l.strip_prefix("l_") {
            (Some("L"), r.to_string())
        } else if let Some(r) = l.strip_prefix("r_") {
            (Some("R"), r.to_string())
        } else if l.len() > 2 && (l.starts_with('l') || l.starts_with('r')) && SIDED.iter().any(|p| &l[1..] == *p) {
            (Some(if l.starts_with('l') { "L" } else { "R" }), l[1..].to_string())
        } else {
            (None, l.to_string())
        }
    };
    let part = part.trim_matches('_');
    let sided = |base: &str| side.map(|s| format!("{base}.{s}"));
    match part {
        "hips" | "hip" | "pelvis" | "root_hips" => side.is_none().then(|| "hips".into()),
        "spine" | "lowerback" | "abdomen" | "spine01" | "spine_01" | "torso" => side.is_none().then(|| "spine".into()),
        "spine1" | "spine2" | "chest" | "upperchest" | "upper_chest" | "spine02" | "spine_02" | "spine_03" | "thorax" | "chest1" => side.is_none().then(|| "chest".into()),
        "neck" | "neck1" | "neck_01" | "neck01" => side.is_none().then(|| "neck".into()),
        "head" => side.is_none().then(|| "head".into()),
        "shoulder" | "clavicle" | "collar" | "shldr_clavicle" => sided("shoulder"),
        "arm" | "upperarm" | "upper_arm" | "shldr" | "humerus" => sided("upper_arm"),
        "forearm" | "lowerarm" | "lower_arm" | "fore_arm" | "elbow" | "radius" => sided("forearm"),
        "hand" | "wrist" => sided("hand"),
        "upleg" | "up_leg" | "thigh" | "upperleg" | "upper_leg" | "hipjoint_thigh" | "femur" => sided("thigh"),
        "leg" | "shin" | "calf" | "lowerleg" | "lower_leg" | "knee" | "tibia" => sided("shin"),
        "foot" | "ankle" => sided("foot"),
        "toebase" | "toe" | "toes" | "ball" | "toe_base" => sided("toe"),
        "eye" => sided("eye"),
        _ => None,
    }
}

/// Names that, after a bare `l`/`r`, are body parts (`lThigh`, `rFoot`).
const SIDED: &[&str] = &["thigh", "shin", "foot", "toe", "hand", "forearm", "shldr", "collar", "uparm", "loarm", "upleg", "leg", "arm", "clavicle", "hip", "knee", "ankle", "elbow", "wrist", "shoulder", "eye"];

/// Rig bone index for each role found in it.
fn roles(rig: &Rig) -> std::collections::HashMap<String, usize> {
    let mut out = std::collections::HashMap::new();
    for (i, b) in rig.bones.iter().enumerate() {
        if let Some(r) = role(&b.name) {
            // The first bone of a role wins (Spine before Spine1), except
            // the chest, where the last spine bone below the neck fits best.
            if r == "chest" || !out.contains_key(&r) {
                out.insert(r, i);
            }
        }
    }
    out
}

fn next<'a>(t: &mut impl Iterator<Item = &'a str>) -> Result<&'a str, String> {
    t.next().ok_or_else(|| "the BVH file ends early".to_string())
}

/// A BVH file: its armature (joints as bones, ends from children or end
/// sites) and its motion as an action. `fit` scales it so the figure is
/// that many px tall (0 keeps the file's units).
pub fn read_bvh(text: &str, fit: f32, name: &str) -> Result<(Rig, Action), String> {
    let mut tok = text.split_whitespace();
    let mut joints: Vec<(String, Option<usize>, V3, Vec<String>, Option<V3>)> = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    if next(&mut tok)? != "HIERARCHY" {
        return Err("not a BVH file (no HIERARCHY)".into());
    }
    let mut end_site = false;
    loop {
        let w = next(&mut tok)?;
        match w {
            "ROOT" | "JOINT" => {
                let n = next(&mut tok)?.to_string();
                joints.push((n, stack.last().copied(), [0.0; 3], Vec::new(), None));
                stack.push(joints.len() - 1);
            }
            "End" => {
                next(&mut tok)?;
                end_site = true;
            }
            "{" => {}
            "}" => {
                if end_site {
                    end_site = false;
                } else {
                    stack.pop();
                }
            }
            "OFFSET" => {
                let v: V3 = [next(&mut tok)?.parse().map_err(|_| "bad OFFSET")?, next(&mut tok)?.parse().map_err(|_| "bad OFFSET")?, next(&mut tok)?.parse().map_err(|_| "bad OFFSET")?];
                let j = *stack.last().ok_or("OFFSET outside a joint")?;
                if end_site {
                    joints[j].4 = Some(v);
                } else {
                    joints[j].2 = v;
                }
            }
            "CHANNELS" => {
                let n: usize = next(&mut tok)?.parse().map_err(|_| "bad CHANNELS")?;
                let j = *stack.last().ok_or("CHANNELS outside a joint")?;
                for _ in 0..n {
                    joints[j].3.push(next(&mut tok)?.to_ascii_lowercase());
                }
            }
            "MOTION" => break,
            other => return Err(format!("unexpected \"{other}\" in the BVH hierarchy")),
        }
        if joints.len() > 512 {
            return Err("too many joints".into());
        }
    }
    if joints.is_empty() {
        return Err("the BVH file has no joints".into());
    }
    if next(&mut tok)? != "Frames:" {
        return Err("no Frames: line".into());
    }
    let frames: usize = next(&mut tok)?.parse().map_err(|_| "bad frame count")?;
    if next(&mut tok)? != "Frame" || next(&mut tok)? != "Time:" {
        return Err("no Frame Time: line".into());
    }
    let dt: f32 = next(&mut tok)?.parse().map_err(|_| "bad frame time")?;
    let channels: usize = joints.iter().map(|j| j.3.len()).sum();
    let frames = frames.min(100_000);
    let mut data: Vec<f32> = Vec::with_capacity(frames * channels);
    for _ in 0..frames * channels {
        data.push(tok.next().and_then(|v| v.parse().ok()).unwrap_or(0.0));
    }
    // BVH is y up, facing +z; Studio is y down, facing -z: half a turn about x.
    let conv = |v: V3| [v[0], -v[1], -v[2]];
    let conv_q = |q: Quat| [q[0], -q[1], -q[2], q[3]];
    // Rest positions.
    let mut rest: Vec<V3> = Vec::with_capacity(joints.len());
    for j in &joints {
        let p = j.1.map_or([0.0; 3], |p| rest[p]);
        rest.push(add(p, j.2));
    }
    let lo = rest.iter().fold(f32::INFINITY, |a, p| a.min(p[1]));
    let hi = rest.iter().enumerate().fold(f32::NEG_INFINITY, |a, (i, p)| a.max(p[1]).max(joints[i].4.map_or(p[1], |e| p[1] + e[1])));
    let k = if fit > 0.0 && hi - lo > 1e-6 { fit / (hi - lo) } else { 1.0 };
    let mut rig = Rig::default();
    for (i, j) in joints.iter().enumerate() {
        let head = scale(conv(rest[i]), k);
        let kids: Vec<usize> = (0..joints.len()).filter(|&c| joints[c].1 == Some(i)).collect();
        let tail = if let Some(e) = j.4 {
            add(head, scale(conv(e), k))
        } else if !kids.is_empty() {
            // Towards the child furthest along (a hip's knee, not its other hip).
            let far = kids.iter().max_by(|a, b| crate::length(joints[**a].2).total_cmp(&crate::length(joints[**b].2))).copied().unwrap_or(kids[0]);
            scale(conv(rest[far]), k)
        } else {
            add(head, [0.0, -0.05 * fit.max(10.0), 0.0])
        };
        let tail = if crate::length(sub(tail, head)) < 1e-3 { add(head, [0.0, -1.0, 0.0]) } else { tail };
        rig.bones.push(Bone::new(j.0.clone(), j.1, head, tail));
    }
    // Frames: each joint's turn in the world (BVH frames are world-aligned
    // at rest), then into the rig's bones.
    let rest_world = rig.rest_world();
    let mut poses: Vec<Vec<BonePose>> = Vec::with_capacity(frames);
    for f in 0..frames {
        let row = &data[f * channels..(f + 1) * channels];
        let mut at = 0;
        let mut world: Vec<Quat> = Vec::with_capacity(joints.len());
        let mut root_pos = None;
        for j in &joints {
            let mut q = crate::rig::Q_ID;
            let mut pos = j.2;
            for c in &j.3 {
                let v = row.get(at).copied().unwrap_or(0.0);
                at += 1;
                match c.as_str() {
                    "xposition" => pos[0] = v,
                    "yposition" => pos[1] = v,
                    "zposition" => pos[2] = v,
                    "xrotation" => q = q_mul(q, crate::rig::q_axis_angle([1.0, 0.0, 0.0], v.to_radians())),
                    "yrotation" => q = q_mul(q, crate::rig::q_axis_angle([0.0, 1.0, 0.0], v.to_radians())),
                    "zrotation" => q = q_mul(q, crate::rig::q_axis_angle([0.0, 0.0, 1.0], v.to_radians())),
                    _ => {}
                }
            }
            let parent = j.1.map_or(crate::rig::Q_ID, |p| world[p]);
            world.push(q_normalise(q_mul(parent, q)));
            if j.1.is_none() && j.3.iter().any(|c| c.ends_with("position")) {
                root_pos = Some(scale(conv(pos), k));
            }
        }
        let bone_world: Vec<Quat> = world.iter().zip(&rest_world).map(|(w, r)| q_normalise(q_mul(conv_q(*w), r.rot))).collect();
        poses.push(rig.local_from_world(&bone_world, root_pos));
    }
    let action = Action::from_frames(name, 1.0 / dt.max(1e-4), &poses);
    Ok((rig, action))
}

/// `action` (made for `src`) moved onto `dst` by bone role. Bones of `dst`
/// with no counterpart keep their rest pose under their parents.
pub fn retarget(src: &Rig, action: &Action, dst: &Rig, name: &str) -> Result<Action, String> {
    let sr = roles(src);
    let dr = roles(dst);
    let pairs: Vec<(usize, usize)> = dr.iter().filter_map(|(r, &d)| sr.get(r).map(|&s| (s, d))).collect();
    if pairs.len() < 3 {
        return Err(format!("the rigs share too few bone roles ({}); name bones like hips, spine, thigh.L, upper_arm.R", pairs.len()));
    }
    let src_rest = src.rest_world();
    let dst_rest = dst.rest_world();
    let dst_rest_local = dst.rest_local();
    // Correction for differing rest poses: limbs (sided bones) point where
    // the source's point, so the turn from the source bone's rest direction
    // to the target's is taken out; the body's middle bones (hips, spine,
    // chest, neck, head), whose directions are arbitrary in many files,
    // turn as the source turns from its rest.
    let fix: Vec<(usize, usize, Quat)> = pairs
        .iter()
        .map(|&(s, d)| {
            let limb = role(&dst.bones[d].name).is_some_and(|r| r.contains('.'));
            let sd = sub(src.bones[s].tail, src.bones[s].head);
            let dd = sub(dst.bones[d].tail, dst.bones[d].head);
            (s, d, if limb { q_from_to(sd, dd) } else { crate::rig::Q_ID })
        })
        .collect();
    let height = |r: &Rig, i: Option<&usize>| i.map(|&i| r.bones[i].head);
    let (sh, dh) = (height(src, sr.get("hips")), height(dst, dr.get("hips")));
    let ground = |r: &Rig| r.bones.iter().fold(f32::NEG_INFINITY, |a, b| a.max(b.head[1]).max(b.tail[1]));
    let k = match (sh, dh) {
        (Some(s), Some(d)) => ((ground(dst) - d[1]) / (ground(src) - s[1]).max(1e-3)).abs(),
        _ => 1.0,
    };
    let fps = 30.0;
    let frames = ((action.duration * fps).round() as usize).max(1);
    let mut out: Vec<Vec<BonePose>> = Vec::with_capacity(frames + 1);
    for f in 0..=frames {
        let t = f as f32 / fps;
        let (pose, _) = action.sample(src.bones.len(), src.shapes.len(), t, false);
        let sw = src.forward(&pose);
        // Target frames worked out parent first.
        let mut world: Vec<Xf> = Vec::with_capacity(dst.bones.len());
        let mut rots: Vec<Quat> = Vec::with_capacity(dst.bones.len());
        for (i, b) in dst.bones.iter().enumerate() {
            let parent = b.parent.map_or(Xf::IDENTITY, |p| world[p]);
            let mut frame = parent.then(&dst_rest_local[i]);
            if let Some(&(s, _, c)) = fix.iter().find(|e| e.1 == i) {
                // Turn since rest, applied to the target's rest after lining
                // up the rest directions: R = Δ · C⁻¹ · B.
                let delta = q_mul(sw[s].rot, q_conj(src_rest[s].rot));
                frame.rot = q_normalise(q_mul(q_mul(delta, q_conj(c)), dst_rest[i].rot));
            }
            rots.push(frame.rot);
            world.push(frame);
        }
        let root = match (sr.get("hips"), dr.get("hips")) {
            (Some(&s), Some(&d)) => Some(add(dst.bones[d].head, scale(sub(sw[s].pos, src_rest[s].pos), k))),
            _ => None,
        };
        out.push(dst.local_from_world(&rots, root));
    }
    let mut a = Action::from_frames(name, fps, &out);
    a.duration = action.duration;
    Ok(a)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rig::PoseInput;

    const BVH: &str = "HIERARCHY
ROOT Hips
{
  OFFSET 0 90 0
  CHANNELS 6 Xposition Yposition Zposition Zrotation Xrotation Yrotation
  JOINT LeftUpLeg
  {
    OFFSET 10 0 0
    CHANNELS 3 Zrotation Xrotation Yrotation
    JOINT LeftLeg
    {
      OFFSET 0 -45 0
      CHANNELS 3 Zrotation Xrotation Yrotation
      End Site
      {
        OFFSET 0 -45 0
      }
    }
  }
  JOINT Spine
  {
    OFFSET 0 10 0
    CHANNELS 3 Zrotation Xrotation Yrotation
    JOINT Head
    {
      OFFSET 0 60 0
      CHANNELS 3 Zrotation Xrotation Yrotation
      End Site
      {
        OFFSET 0 20 0
      }
    }
  }
}
MOTION
Frames: 2
Frame Time: 0.5
0 90 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0
0 90 30 0 0 0 0 -60 0 0 0 0 0 0 0 0 0 0
";

    #[test]
    fn roles_from_many_naming_schemes() {
        for (n, r) in [("mixamorig:LeftUpLeg", "thigh.L"), ("RightForeArm", "forearm.R"), ("lThigh", "thigh.L"), ("thigh_r", "thigh.R"), ("upper_arm.L", "upper_arm.L"), ("Hips", "hips"), ("Spine1", "chest"), ("LeftToeBase", "toe.L"), ("calf_l", "shin.L"), ("Head", "head")] {
            assert_eq!(role(n).as_deref(), Some(r), "{n}");
        }
        assert_eq!(role("Tail3"), None);
    }

    #[test]
    fn a_bvh_reads_into_a_rig_and_a_motion() {
        let (rig, act) = read_bvh(BVH, 200.0, "test").unwrap();
        assert_eq!(rig.bones.len(), 5);
        // Studio's y is down: the head is above (smaller y than) the hips.
        let hips = rig.bone("Hips").unwrap();
        let head = rig.bone("Head").unwrap();
        assert!(rig.bones[head].head[1] < rig.bones[hips].head[1]);
        assert!((act.duration - 0.5).abs() < 1e-4);
        // Frame 2: the thigh swings -60° about BVH x: its foot comes
        // forward towards the viewer in BVH (+z), so to -z in Studio.
        let mut r = rig.clone();
        r.actions.push(act);
        let p = r.evaluate(&PoseInput { action: Some(("test".into(), 0.5, false)), ..Default::default() });
        let knee = r.bone("LeftLeg").unwrap();
        let foot = r.bone_lines(&p)[knee].1;
        let hip_at = r.bone_lines(&p)[r.bone("LeftUpLeg").unwrap()].0;
        assert!(foot[2] < hip_at[2] - 20.0, "{foot:?} vs {hip_at:?}");
    }

    #[test]
    fn retargeting_carries_a_motion_between_rigs() {
        let (src, act) = read_bvh(BVH, 200.0, "test").unwrap();
        let m = crate::character::character(300.0, &crate::character::Build { detail: 1, face: false, ..Default::default() });
        let dst = m.rig.clone().unwrap();
        let moved = retarget(&src, &act, &dst, "moved").unwrap();
        let mut r = (*dst).clone();
        r.actions.push(moved);
        let p = r.evaluate(&PoseInput { action: Some(("moved".into(), 0.5, false)), ..Default::default() });
        let shin = r.bone("shin.L").unwrap();
        let lines = r.bone_lines(&p);
        // The left leg swings forward on the character too.
        assert!(lines[shin].1[2] < lines[r.bone("thigh.L").unwrap()].0[2] - 15.0, "{:?}", lines[shin]);
        // The capture walks forward (its root moves 30 units towards the
        // viewer), and the right leg, not in it, hangs straight under the hips.
        let hips = r.bone("hips").unwrap();
        let moved_by = sub(lines[hips].0, r.bones[hips].head);
        assert!(moved_by[2] < -20.0, "{moved_by:?}");
        let rs = r.bone("shin.R").unwrap();
        let rel = sub(lines[rs].1, lines[hips].0);
        let rest = sub(r.bones[rs].tail, r.bones[hips].head);
        assert!(crate::length(sub(rel, rest)) < 5.0, "{rel:?} vs rest {rest:?}");
    }
}
