// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What a rig is made of, found by bone name: legs, arms, spine, head,
//! eyes, tail and fingers of any character (biped, quadruped, creature).
//!
//! Names are read case-insensitively and in any common spelling:
//! `upper_arm.L`, `UpperArm_L`, `LeftUpArm`, `thigh.R`, `leg_front.L.0`,
//! `paw_back.R`, `thumb.2.L`, `Index_Proximal_R` … Bones that are not
//! found are simply absent; whoever asks for a motion that needs them is
//! told which are missing and what to name them.

use crate::rig::{Rig, Xf};
use crate::{V3, add, cross, dot, length, normalise, scale, sub};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Two legs and arms: people, robots, birds on their feet.
    Biped,
    /// Four legs (front and back pairs): animals.
    Quadruped,
    /// Some legs, no clear pairing: spiders, insects, anything else.
    Creature,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    L,
    R,
    Mid,
}

impl Side {
    pub fn name(self) -> &'static str {
        match self {
            Side::L => "L",
            Side::R => "R",
            Side::Mid => "",
        }
    }
    pub fn sign(self) -> f32 {
        match self {
            Side::L => 1.0,
            Side::R => -1.0,
            Side::Mid => 0.0,
        }
    }
}

/// A leg or an arm: two bones that bend and an end (foot, paw, hand).
#[derive(Clone, Debug, PartialEq)]
pub struct Limb {
    /// `leg.L`, `arm.R`, `leg_front.L` …
    pub name: String,
    pub side: Side,
    /// The front pair of an animal (false for a person's legs).
    pub front: bool,
    pub upper: usize,
    pub lower: usize,
    /// The foot, paw or hand: its head is the ankle or wrist.
    pub end: usize,
    /// The toe bone of a foot, when there is one.
    pub tip: Option<usize>,
}

/// One finger: its bones from the knuckle out.
#[derive(Clone, Debug, PartialEq)]
pub struct Finger {
    /// thumb, index, middle, ring, pinky.
    pub name: &'static str,
    pub side: Side,
    pub joints: Vec<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Body {
    pub kind: Kind,
    /// The bone nothing hangs from; moving it moves the whole figure.
    pub root: usize,
    pub hips: usize,
    pub spine: Vec<usize>,
    pub chest: Option<usize>,
    pub neck: Option<usize>,
    pub head: Option<usize>,
    pub jaw: Option<usize>,
    pub eyes: Vec<usize>,
    pub shoulders: Vec<(Side, usize)>,
    pub legs: Vec<Limb>,
    pub arms: Vec<Limb>,
    pub tail: Vec<usize>,
    pub fingers: Vec<Finger>,
    /// Which way it faces and which way is up, in the model (unit).
    pub fwd: V3,
    pub up: V3,
}

/// The pieces of a bone name: lower-case words and numbers, camelCase and
/// separators split (`LeftUpArm` → left, up, arm).
pub fn tokens(name: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut prev: Option<char> = None;
    let flush = |cur: &mut String, out: &mut Vec<String>| {
        if !cur.is_empty() {
            out.push(cur.to_lowercase());
            cur.clear();
        }
    };
    for c in name.chars() {
        if !c.is_alphanumeric() {
            flush(&mut cur, &mut out);
            prev = None;
            continue;
        }
        if let Some(p) = prev
            && ((p.is_lowercase() && c.is_uppercase()) || (p.is_alphabetic() != c.is_alphabetic()))
        {
            flush(&mut cur, &mut out);
        }
        cur.push(c);
        prev = Some(c);
    }
    flush(&mut cur, &mut out);
    out
}

fn has(t: &[String], words: &[&str]) -> bool {
    t.iter().any(|x| words.contains(&x.as_str()))
}

fn side_of(t: &[String]) -> Side {
    if has(t, &["l", "left"]) {
        Side::L
    } else if has(t, &["r", "right"]) {
        Side::R
    } else {
        Side::Mid
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    Hips,
    Spine,
    Chest,
    Neck,
    Head,
    Jaw,
    Eye,
    Shoulder,
    UpperArm,
    Forearm,
    Hand,
    Thigh,
    Shin,
    Foot,
    Toe,
    Tail,
    Finger(&'static str),
}

fn role_of(t: &[String]) -> Option<Role> {
    for (w, n) in [("thumb", "thumb"), ("index", "index"), ("middle", "middle"), ("ring", "ring"), ("pinky", "pinky"), ("pinkie", "pinky"), ("little", "pinky")] {
        if has(t, &[w]) {
            return Some(Role::Finger(n));
        }
    }
    if t.iter().any(|x| x.starts_with("eye") && !["eyelid", "eyelids", "eyelash", "eyelashes", "eyebrow", "eyebrows"].contains(&x.as_str())) && !has(t, &["lid", "lash", "brow"]) {
        return Some(Role::Eye);
    }
    if has(t, &["jaw", "chin"]) {
        return Some(Role::Jaw);
    }
    if has(t, &["tail"]) {
        return Some(Role::Tail);
    }
    if has(t, &["head"]) {
        return Some(Role::Head);
    }
    if has(t, &["neck"]) {
        return Some(Role::Neck);
    }
    if has(t, &["hips", "hip", "pelvis"]) && !has(t, &["thigh"]) {
        return Some(Role::Hips);
    }
    if has(t, &["chest", "thorax", "torso", "ribs"]) {
        return Some(Role::Chest);
    }
    if has(t, &["spine", "back", "waist", "abdomen"]) && !has(t, &["leg", "paw", "hoof", "foot"]) {
        return Some(Role::Spine);
    }
    if has(t, &["shoulder", "clavicle", "collar"]) {
        return Some(Role::Shoulder);
    }
    let up = has(t, &["upper", "up"]);
    let low = has(t, &["lower", "low", "down"]);
    if has(t, &["forearm", "elbow"]) || (low && has(t, &["arm"])) {
        return Some(Role::Forearm);
    }
    if has(t, &["upperarm", "uparm", "humerus"]) || has(t, &["arm"]) {
        return Some(Role::UpperArm);
    }
    if has(t, &["hand", "wrist", "palm"]) {
        return Some(Role::Hand);
    }
    if has(t, &["thigh", "upleg", "upperleg", "femur"]) || (up && has(t, &["leg"])) {
        return Some(Role::Thigh);
    }
    if has(t, &["shin", "calf", "tibia", "lowerleg"]) || (low && has(t, &["leg"])) {
        return Some(Role::Shin);
    }
    if has(t, &["foot", "ankle", "paw", "hoof"]) {
        return Some(Role::Foot);
    }
    if has(t, &["toe", "toes", "ball"]) {
        return Some(Role::Toe);
    }
    // An animal's `leg_front.L.0` / `.1`: upper then lower.
    if has(t, &["leg"]) {
        let n = t.iter().rev().find_map(|x| x.parse::<u32>().ok());
        return Some(if n == Some(1) { Role::Shin } else { Role::Thigh });
    }
    None
}

fn is_front(t: &[String]) -> Option<bool> {
    if has(t, &["front", "fore", "forelimb"]) {
        Some(true)
    } else if has(t, &["back", "hind", "rear"]) {
        Some(false)
    } else {
        None
    }
}

impl Body {
    /// The roles of a rig's bones. Always returns something; whatever is
    /// not there is empty.
    pub fn of(rig: &Rig) -> Body {
        let n = rig.bones.len();
        let toks: Vec<Vec<String>> = rig.bones.iter().map(|b| tokens(&b.name)).collect();
        let roles: Vec<Option<Role>> = toks.iter().map(|t| role_of(t)).collect();
        let desc = |i: usize| -> usize {
            let mut c = 0;
            for j in 0..n {
                let mut k = Some(j);
                while let Some(x) = k {
                    if x == i {
                        c += 1;
                        break;
                    }
                    k = rig.bones[x].parent;
                }
            }
            c
        };
        let root = (0..n).filter(|&i| rig.bones[i].parent.is_none()).max_by_key(|&i| desc(i)).unwrap_or(0);
        let first = |r: Role| (0..n).find(|&i| roles[i] == Some(r));
        let hips = first(Role::Hips).unwrap_or(root);
        let mut spine: Vec<usize> = (0..n).filter(|&i| roles[i] == Some(Role::Spine)).collect();
        spine.sort_unstable();
        let tail: Vec<usize> = (0..n).filter(|&i| roles[i] == Some(Role::Tail)).collect();
        let eyes: Vec<usize> = (0..n).filter(|&i| roles[i] == Some(Role::Eye)).collect();
        let shoulders: Vec<(Side, usize)> = (0..n).filter(|&i| roles[i] == Some(Role::Shoulder)).map(|i| (side_of(&toks[i]), i)).collect();
        // Limbs: group the bones of each role by side (and front or back).
        let mut legs: Vec<Limb> = Vec::new();
        let mut arms: Vec<Limb> = Vec::new();
        let pick = |role: Role, side: Side, front: Option<bool>, among: &[usize]| -> Option<usize> { among.iter().copied().find(|&i| roles[i] == Some(role) && side_of(&toks[i]) == side && (front.is_none() || is_front(&toks[i]) == front || is_front(&toks[i]).is_none())) };
        let all: Vec<usize> = (0..n).collect();
        // Legs: every thigh starts one.
        for i in (0..n).filter(|&i| roles[i] == Some(Role::Thigh)) {
            let side = side_of(&toks[i]);
            let front = is_front(&toks[i]);
            // The shin hangs from the thigh; the foot from the shin.
            let shin = (0..n).find(|&j| roles[j] == Some(Role::Shin) && rig.bones[j].parent == Some(i)).or_else(|| pick(Role::Shin, side, front, &all));
            let Some(shin) = shin else { continue };
            let foot = (0..n).find(|&j| roles[j] == Some(Role::Foot) && rig.bones[j].parent == Some(shin)).or_else(|| pick(Role::Foot, side, front, &all));
            let Some(foot) = foot else { continue };
            let toe = (0..n).find(|&j| roles[j] == Some(Role::Toe) && rig.bones[j].parent == Some(foot));
            legs.push(Limb { name: format!("leg{}{}", front.map_or("", |f| if f { "_front" } else { "_back" }), if side == Side::Mid { String::new() } else { format!(".{}", side.name()) }), side, front: front == Some(true), upper: i, lower: shin, end: foot, tip: toe });
        }
        for i in (0..n).filter(|&i| roles[i] == Some(Role::UpperArm)) {
            let side = side_of(&toks[i]);
            let fore = (0..n).find(|&j| roles[j] == Some(Role::Forearm) && rig.bones[j].parent == Some(i)).or_else(|| pick(Role::Forearm, side, None, &all));
            let Some(fore) = fore else { continue };
            let hand = (0..n).find(|&j| roles[j] == Some(Role::Hand) && rig.bones[j].parent == Some(fore)).or_else(|| pick(Role::Hand, side, None, &all));
            let Some(hand) = hand else { continue };
            arms.push(Limb { name: format!("arm.{}", side.name()), side, front: true, upper: i, lower: fore, end: hand, tip: None });
        }
        // Fingers: the bones of one finger of one hand, from the knuckle out.
        let mut fingers: Vec<Finger> = Vec::new();
        for name in ["thumb", "index", "middle", "ring", "pinky"] {
            for side in [Side::L, Side::R] {
                let mut joints: Vec<usize> = (0..n).filter(|&i| roles[i] == Some(Role::Finger(name)) && side_of(&toks[i]) == side).collect();
                // Order by depth in the hierarchy, then by number.
                joints.sort_by_key(|&i| {
                    let mut d = 0;
                    let mut k = rig.bones[i].parent;
                    while let Some(x) = k {
                        d += 1;
                        k = rig.bones[x].parent;
                    }
                    d
                });
                if !joints.is_empty() {
                    fingers.push(Finger { name, side, joints });
                }
            }
        }
        let kind = if legs.iter().any(|l| l.front) && legs.iter().any(|l| !l.front) && legs.len() >= 4 || (legs.len() >= 4 && arms.is_empty()) {
            Kind::Quadruped
        } else if legs.len() == 2 {
            Kind::Biped
        } else {
            Kind::Creature
        };
        let mut body = Body {
            kind,
            root,
            hips,
            spine,
            chest: first(Role::Chest),
            neck: first(Role::Neck),
            head: first(Role::Head),
            jaw: first(Role::Jaw),
            eyes,
            shoulders,
            legs,
            arms,
            tail,
            fingers,
            fwd: [0.0, 0.0, -1.0],
            up: [0.0, -1.0, 0.0],
        };
        body.orient(rig);
        body
    }

    /// Up and forward from the rest pose (feet to head; toes ahead of
    /// heels, or head ahead of hips), snapped to an axis when close.
    fn orient(&mut self, rig: &Rig) {
        let tail_of = |i: usize| rig.bones[i].tail;
        let head_of = |i: usize| rig.bones[i].head;
        let mut up = None;
        if !self.legs.is_empty() {
            let feet = self.legs.iter().map(|l| tail_of(l.tip.unwrap_or(l.end))).fold([0.0; 3], add);
            let feet = scale(feet, 1.0 / self.legs.len() as f32);
            // From the soles to the hip joints: straight up even for an animal whose head is ahead.
            let hips = scale(self.legs.iter().map(|l| head_of(l.upper)).fold([0.0; 3], add), 1.0 / self.legs.len() as f32);
            let u = normalise(sub(hips, feet));
            if u != [0.0; 3] {
                up = Some(u);
            }
        }
        let mut u = up.unwrap_or([0.0, -1.0, 0.0]);
        u = snap(u);
        let mut fwd = [0.0; 3];
        match self.kind {
            Kind::Biped => {
                for l in &self.legs {
                    if let Some(t) = l.tip {
                        fwd = add(fwd, sub(tail_of(t), head_of(l.end)));
                    }
                }
            }
            _ => {
                if let Some(h) = self.head {
                    fwd = sub(tail_of(h), head_of(self.hips));
                }
            }
        }
        fwd = sub(fwd, scale(u, dot(fwd, u)));
        let f = normalise(fwd);
        let f = if f == [0.0; 3] { snap(sub([0.0, 0.0, -1.0], scale(u, u[2] * -1.0))) } else { snap(f) };
        self.up = u;
        self.fwd = f;
    }

    /// The character's left (toward `.L` bones) in the model.
    pub fn left(&self) -> V3 {
        cross(self.up, self.fwd)
    }

    pub fn leg(&self, side: Side, front: bool) -> Option<&Limb> {
        self.legs.iter().find(|l| l.side == side && (l.front == front || self.kind == Kind::Biped))
    }

    pub fn arm(&self, side: Side) -> Option<&Limb> {
        self.arms.iter().find(|l| l.side == side)
    }

    pub fn finger(&self, name: &str, side: Side) -> Option<&Finger> {
        self.fingers.iter().find(|f| f.name == name && f.side == side)
    }

    /// Bones the body lacks for a kind of motion, in words.
    pub fn missing_for_legs(&self, rig: &Rig) -> Option<String> {
        if !self.legs.is_empty() {
            return None;
        }
        Some(format!(
            "no legs found in the rig (bones: {}); name them like thigh.L / shin.L / foot.L (or upper_leg, calf, ankle), or leg_front.L.0 / leg_front.L.1 / paw_front.L for an animal",
            rig.bones.iter().map(|b| b.name.as_str()).collect::<Vec<_>>().join(", ")
        ))
    }

    /// The model's vertical axis must be one of x, y, z for the motion
    /// maths; characters stand along y.
    pub fn upright(&self) -> bool {
        self.up[1].abs() > 0.99
    }

    /// Length of a limb's two bones.
    pub fn limb_length(rig: &Rig, l: &Limb) -> f32 {
        rig.bones[l.upper].length() + rig.bones[l.lower].length()
    }

    /// Rest-pose frames (bones at rest).
    pub fn rest(rig: &Rig) -> Vec<Xf> {
        rig.rest_world()
    }

    /// The point a limb's end (ankle, wrist) is at rest.
    pub fn rest_end(rig: &Rig, l: &Limb) -> V3 {
        rig.bones[l.end].head
    }

    /// How tall the figure is along `up` at rest (crown to soles).
    pub fn height(&self, rig: &Rig) -> f32 {
        let mut lo = f32::INFINITY;
        let mut hi = f32::NEG_INFINITY;
        for b in &rig.bones {
            for p in [b.head, b.tail] {
                let h = dot(p, self.up);
                lo = lo.min(h);
                hi = hi.max(h);
            }
        }
        if lo.is_finite() { (hi - lo).max(1.0) } else { 100.0 }
    }

    /// Where the ground is at rest: the lowest sole, as a height along up.
    pub fn sole_height(&self, rig: &Rig) -> f32 {
        let mut lo = f32::INFINITY;
        for l in &self.legs {
            for i in [Some(l.end), l.tip].into_iter().flatten() {
                for p in [rig.bones[i].head, rig.bones[i].tail] {
                    lo = lo.min(dot(p, self.up));
                }
            }
        }
        if lo.is_finite() { lo } else { 0.0 }
    }

    pub fn describe(&self, rig: &Rig) -> String {
        let name = |i: usize| rig.bones[i].name.as_str();
        format!(
            "{:?}: {} legs, {} arms, {} spine bones, head {}, {} eyes, {} tail bones, {} finger bones",
            self.kind,
            self.legs.len(),
            self.arms.len(),
            self.spine.len() + usize::from(self.chest.is_some()),
            self.head.map_or("-", name),
            self.eyes.len(),
            self.tail.len(),
            self.fingers.iter().map(|f| f.joints.len()).sum::<usize>()
        )
    }
}

fn snap(v: V3) -> V3 {
    for (i, a) in [[1.0f32, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]].iter().enumerate() {
        let d = v[i];
        if d.abs() > 0.9 {
            return scale(*a, d.signum());
        }
    }
    let _ = length(v);
    normalise(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::autorig;
    use crate::prim;

    #[test]
    fn names_in_any_spelling_find_their_roles() {
        assert_eq!(tokens("LeftUpArm"), ["left", "up", "arm"]);
        assert_eq!(tokens("upper_arm.L"), ["upper", "arm", "l"]);
        assert_eq!(tokens("Thumb_Proximal_R"), ["thumb", "proximal", "r"]);
        assert_eq!(tokens("index.2.L"), ["index", "2", "l"]);
        assert_eq!(role_of(&tokens("mixamorig:LeftUpLeg")), Some(Role::Thigh));
        assert_eq!(role_of(&tokens("calf_r")), Some(Role::Shin));
        assert_eq!(role_of(&tokens("leg_front.L.1")), Some(Role::Shin));
        assert_eq!(role_of(&tokens("EyeBrow.L")), None);
        assert_eq!(role_of(&tokens("EYE.L")), Some(Role::Eye));
        assert_eq!(role_of(&tokens("Pinkie_3_L")), Some(Role::Finger("pinky")));
    }

    #[test]
    fn a_person_and_an_animal_are_read_from_their_rigs() {
        let m = crate::character::character(400.0, &crate::character::Build { detail: 1, ..Default::default() });
        let rig = m.rig.clone().unwrap();
        let b = Body::of(&rig);
        assert_eq!(b.kind, Kind::Biped);
        assert_eq!((b.legs.len(), b.arms.len(), b.eyes.len()), (2, 2, 2));
        assert!(b.upright() && b.up[1] < -0.99, "{:?}", b.up);
        assert!(b.fwd[2] < -0.99, "faces -z: {:?}", b.fwd);
        assert!(b.left()[0] > 0.99, "left is +x: {:?}", b.left());
        assert!(b.head.is_some() && b.neck.is_some() && b.chest.is_some() && !b.spine.is_empty());
        // A quadruped along x, head toward -x.
        let body = prim::cube([1.0, 1.0, 1.0]);
        let mut mesh = body;
        for p in &mut mesh.positions {
            *p = [p[0] * 300.0, p[1] * 160.0, p[2] * 80.0];
        }
        let q = autorig::quadruped(&mesh);
        let qb = Body::of(&q);
        assert_eq!(qb.kind, Kind::Quadruped);
        assert_eq!(qb.legs.len(), 4);
        assert!(qb.fwd[0] < -0.99, "{:?}", qb.fwd);
        assert!(qb.legs.iter().filter(|l| l.front).count() == 2);
        assert_eq!(qb.tail.len(), 3);
    }
}
