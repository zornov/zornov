// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What bone names say: which side a bone is on, its mirror twin's name,
//! and its role in a shared vocabulary, so poses, motions, mocap and
//! templates move between rigs that name bones differently and between
//! 2D and 3D.
//!
//! Roles are Studio's names: `hips`, `spine`, `chest`, `neck`, `head`,
//! `shoulder.L`, `upper_arm.L`, `forearm.L`, `hand.L`, `thigh.L`,
//! `shin.L`, `foot.L`, `toe.L`, `eye.L` (and `.R`); four-legged animals
//! `leg_front.L.0` (upper), `leg_front.L.1` (lower), `paw_front.L`,
//! `leg_back…`; chains `tail.1`, `tail.2`…. Names from Mixamo
//! (`mixamorig:LeftUpLeg`), CMU and other BVH libraries (`lThigh`,
//! `LHipJoint`), Unreal and Unity (`thigh_l`, `LeftUpperLeg`), Blender
//! (`thigh.L`) and Studio's 2D skeletons (`thigh.front`,
//! `front_leg.upper`) all come to these.
//!
//! A 2D side view names limbs by the side nearer the camera (`front`) and
//! the far one (`back`); which of the character's own sides that is
//! depends on the way it faces ([`near_side`]).

/// A side of the body.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
    /// Nearer the camera, in a 2D side view.
    Front,
    /// Further from it.
    Back,
}

/// Name pieces that say a side, and the piece for the other side.
const SWAPS: &[(&str, &str)] = &[(".L", ".R"), ("_L", "_R"), (".l", ".r"), ("_l", "_r"), ("-L", "-R"), (" L", " R"), (".front", ".back"), ("_front", "_back")];
const WORDS: &[(&str, &str)] = &[("Left", "Right"), ("left", "right"), ("LEFT", "RIGHT"), ("front", "back"), ("Front", "Back")];

/// Which side a bone is on, by its name.
pub fn side(name: &str) -> Option<Side> {
    let lower = name.to_ascii_lowercase();
    let raw = lower.rsplit([':', '|']).next().unwrap_or(&lower);
    let has = |s: &str| raw.ends_with(s) || raw.contains(&format!("{s}.")) || raw.contains(&format!("{s}_"));
    if raw.ends_with(".front") || raw.ends_with("_front") || raw.starts_with("front_") {
        Some(Side::Front)
    } else if raw.ends_with(".back") || raw.ends_with("_back") || raw.starts_with("back_") || raw.starts_with("hind_") {
        Some(Side::Back)
    } else if raw.starts_with("left") || has(".l") || has("_l") || raw.starts_with("l_") || side_letter(raw) == Some('l') {
        Some(Side::Left)
    } else if raw.starts_with("right") || has(".r") || has("_r") || raw.starts_with("r_") || side_letter(raw) == Some('r') {
        Some(Side::Right)
    } else if raw.contains("front") {
        Some(Side::Front)
    } else if raw.contains("back") && !raw.contains("lowerback") {
        Some(Side::Back)
    } else {
        None
    }
}

/// `lThigh`, `rFoot`: a bare side letter before a body part.
fn side_letter(lower: &str) -> Option<char> {
    let c = lower.chars().next()?;
    ((c == 'l' || c == 'r') && lower.len() > 2 && SIDED.contains(&&lower[1..])).then_some(c)
}

/// The name of a bone's mirror twin (`upper_arm.L` ↔ `upper_arm.R`,
/// `LeftHand` ↔ `RightHand`, `thigh.front` ↔ `thigh.back`); `None` for a
/// bone on the middle line.
pub fn mirror(name: &str) -> Option<String> {
    for (a, b) in SWAPS {
        for (x, y) in [(a, b), (b, a)] {
            if let Some(stem) = name.strip_suffix(x) {
                return Some(format!("{stem}{y}"));
            }
            if let Some(i) = name.find(&format!("{x}.")) {
                return Some(format!("{}{y}{}", &name[..i], &name[i + x.len()..]));
            }
        }
    }
    for (a, b) in WORDS {
        for (x, y) in [(a, b), (b, a)] {
            if let Some(i) = name.find(x) {
                return Some(format!("{}{y}{}", &name[..i], &name[i + x.len()..]));
            }
        }
    }
    let raw = name.rsplit([':', '|']).next().unwrap_or(name);
    let pre = &name[..name.len() - raw.len()];
    let lower = raw.to_ascii_lowercase();
    match side_letter(&lower) {
        Some('l') => Some(format!("{pre}{}{}", if raw.starts_with('L') { 'R' } else { 'r' }, &raw[1..])),
        Some('r') => Some(format!("{pre}{}{}", if raw.starts_with('R') { 'L' } else { 'l' }, &raw[1..])),
        _ => None,
    }
}

/// The character's own side nearer the camera in a 2D side view: facing
/// screen right, the camera sees its right side.
pub fn near_side(faces_right: bool) -> char {
    if faces_right { 'R' } else { 'L' }
}

/// The shared role of a bone (see the module notes); for a 2D side view's
/// `front` / `back` bones, `faces_right` says which of the character's
/// sides they are (unknown: front is taken as right).
pub fn role_facing(name: &str, faces_right: Option<bool>) -> Option<String> {
    let raw = name.rsplit([':', '|']).next().unwrap_or(name);
    let lower = raw.to_ascii_lowercase().replace([' ', '-'], "_");
    let lower = lower.trim_start_matches("def_").trim_start_matches("bip01_").trim_start_matches("bip001_").trim_start_matches("b_").to_string();
    let near = near_side(faces_right.unwrap_or(true));
    let far = if near == 'R' { 'L' } else { 'R' };
    // Studio's 2D names: `upper_arm.front`, `front_leg.upper`, `tail.2`.
    if let Some(stem) = lower.strip_suffix(".front") {
        return role_facing(&format!("{stem}.{near}"), None);
    }
    if let Some(stem) = lower.strip_suffix(".back") {
        return role_facing(&format!("{stem}.{far}"), None);
    }
    for (prefix, end) in [("front_leg", "front"), ("foreleg", "front"), ("back_leg", "back"), ("hind_leg", "back"), ("hindleg", "back")] {
        if let Some(rest) = lower.strip_prefix(prefix) {
            let low = rest.contains("lower") || rest.contains('2') || rest.contains('1') && !rest.contains("upper");
            return Some(format!("leg_{end}.{near}.{}", if low { 1 } else { 0 }));
        }
    }
    if let Some(rest) = lower.strip_prefix("leg_front.").or_else(|| lower.strip_prefix("leg_back.")) {
        let end = if lower.starts_with("leg_front") { "front" } else { "back" };
        let (s, i) = rest.split_once('.').unwrap_or((rest, "0"));
        return Some(format!("leg_{end}.{}.{i}", s.to_ascii_uppercase()));
    }
    if let Some(rest) = lower.strip_prefix("paw_front.").or_else(|| lower.strip_prefix("paw_back.")) {
        let end = if lower.starts_with("paw_front") { "front" } else { "back" };
        return Some(format!("paw_{end}.{}", rest.to_ascii_uppercase()));
    }
    for chain in ["tail", "trunk", "tentacle"] {
        if let Some(rest) = lower.strip_prefix(chain) {
            let n: String = rest.chars().filter(|c| c.is_ascii_digit()).collect();
            return Some(format!("{chain}.{}", n.parse::<u32>().unwrap_or(1)));
        }
    }
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
        } else if let Some(c) = side_letter(l) {
            (Some(if c == 'l' { "L" } else { "R" }), l[1..].to_string())
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
        "jaw" => side.is_none().then(|| "jaw".into()),
        "shoulder" | "clavicle" | "collar" | "shldr_clavicle" => sided("shoulder"),
        "arm" | "upperarm" | "upper_arm" | "shldr" | "humerus" | "uparm" => sided("upper_arm"),
        "forearm" | "lowerarm" | "lower_arm" | "fore_arm" | "elbow" | "radius" | "loarm" => sided("forearm"),
        "hand" | "wrist" => sided("hand"),
        "upleg" | "up_leg" | "thigh" | "upperleg" | "upper_leg" | "hipjoint_thigh" | "femur" => sided("thigh"),
        "leg" | "shin" | "calf" | "lowerleg" | "lower_leg" | "knee" | "tibia" => sided("shin"),
        "foot" | "ankle" => sided("foot"),
        "toebase" | "toe" | "toes" | "ball" | "toe_base" => sided("toe"),
        "eye" => sided("eye"),
        _ => None,
    }
}

/// The shared role of a bone name (see the module notes).
pub fn role(name: &str) -> Option<String> {
    role_facing(name, None)
}

/// Names that, after a bare `l`/`r`, are body parts (`lThigh`, `rFoot`).
const SIDED: &[&str] = &["thigh", "shin", "foot", "toe", "hand", "forearm", "shldr", "collar", "uparm", "loarm", "upleg", "leg", "arm", "clavicle", "hip", "knee", "ankle", "elbow", "wrist", "shoulder", "eye"];

/// For each bone of `dst`, the bone of `src` with the same role (by
/// [`role_facing`], each side's own facing): `(dst, src)` pairs.
pub fn pair_by_role(src: &[&str], src_facing: Option<bool>, dst: &[&str], dst_facing: Option<bool>) -> Vec<(usize, usize)> {
    let src_roles: Vec<Option<String>> = src.iter().map(|n| role_facing(n, src_facing)).collect();
    let mut out = Vec::new();
    for (d, name) in dst.iter().enumerate() {
        let Some(r) = role_facing(name, dst_facing) else {
            // No role: the same name.
            if let Some(s) = src.iter().position(|n| n.eq_ignore_ascii_case(name)) {
                out.push((d, s));
            }
            continue;
        };
        // The first bone of a role wins (Spine before Spine1), except the
        // chest, where the last spine bone below the neck fits best.
        let found = if r == "chest" { src_roles.iter().rposition(|x| x.as_deref() == Some("chest")) } else { src_roles.iter().position(|x| x.as_deref() == Some(r.as_str())) };
        if let Some(s) = found {
            out.push((d, s));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_from_many_naming_schemes_and_2d_sides() {
        for (n, r) in [("mixamorig:LeftUpLeg", "thigh.L"), ("RightForeArm", "forearm.R"), ("lThigh", "thigh.L"), ("thigh_r", "thigh.R"), ("upper_arm.L", "upper_arm.L"), ("Hips", "hips"), ("Spine1", "chest"), ("LeftToeBase", "toe.L"), ("calf_l", "shin.L"), ("Head", "head"), ("tail.2", "tail.2"), ("Tail3", "tail.3"), ("leg_front.L.1", "leg_front.L.1")] {
            assert_eq!(role(n).as_deref(), Some(r), "{n}");
        }
        // A 2D side view facing right shows its right side.
        assert_eq!(role_facing("upper_arm.front", Some(true)).as_deref(), Some("upper_arm.R"));
        assert_eq!(role_facing("shin.back", Some(true)).as_deref(), Some("shin.L"));
        assert_eq!(role_facing("thigh.front", Some(false)).as_deref(), Some("thigh.L"));
        assert_eq!(role_facing("front_leg.lower", Some(true)).as_deref(), Some("leg_front.R.1"));
        assert_eq!(role("banana"), None);
    }

    #[test]
    fn mirror_names_and_sides() {
        for (a, b) in [("upper_arm.L", "upper_arm.R"), ("thigh_r", "thigh_l"), ("LeftHand", "RightHand"), ("mixamorig:RightFoot", "mixamorig:LeftFoot"), ("thigh.front", "thigh.back"), ("lThigh", "rThigh"), ("leg_front.L.0", "leg_front.R.0"), ("eye left", "eye right")] {
            assert_eq!(mirror(a).as_deref(), Some(b), "{a}");
            assert_eq!(mirror(b).as_deref(), Some(a), "{b}");
        }
        assert_eq!(mirror("spine"), None);
        assert_eq!(mirror("head"), None);
        assert_eq!(side("upper_arm.L"), Some(Side::Left));
        assert_eq!(side("RightHand"), Some(Side::Right));
        assert_eq!(side("shin.back"), Some(Side::Back));
        assert_eq!(side("spine"), None);
        assert_eq!(side("leg_front.L.0"), Some(Side::Left));
        assert_eq!(side("LowerBack"), None);
    }

    #[test]
    fn rigs_pair_up_by_role() {
        let src = ["Hips", "Spine", "Spine1", "Spine2", "LeftUpLeg", "LeftLeg"];
        let dst = ["hips", "chest", "thigh.front", "shin.front", "tail.1"];
        // The 2D character faces left: its front is its left side.
        let p = pair_by_role(&src, None, &dst, Some(false));
        assert_eq!(p, vec![(0, 0), (1, 3), (2, 4), (3, 5)]);
    }
}
