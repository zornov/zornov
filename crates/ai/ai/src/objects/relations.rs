// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! How things in a frame relate: left or right of, in front of or behind,
//! on, holding, near. Computed from the boxes, the distances and (for
//! people) the hands at one moment; nothing here looks at pixels.

use super::{BBox, Kind, ObjectMap};

/// One thing at one moment.
#[derive(Clone, Debug, PartialEq)]
pub struct Spot {
    pub id: u32,
    pub kind: Kind,
    pub label: String,
    pub bbox: BBox,
    /// Metres from the lens.
    pub z: Option<f32>,
    /// Hands, if the body is known.
    pub wrists: Vec<[f32; 2]>,
}

/// A relation of `a` to `b` ("`a` is on `b`").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rel {
    Holding,
    On,
    InFrontOf,
    Behind,
    LeftOf,
    RightOf,
    Near,
    /// The other thing is on this one.
    Under,
    /// The other thing is holding this one.
    HeldBy,
}

impl Rel {
    pub fn words(self) -> &'static str {
        match self {
            Rel::Holding => "holding",
            Rel::On => "on",
            Rel::InFrontOf => "in front of",
            Rel::Behind => "behind",
            Rel::LeftOf => "left of",
            Rel::RightOf => "right of",
            Rel::Near => "near",
            Rel::Under => "under",
            Rel::HeldBy => "held by",
        }
    }

    /// The relation seen from the other side.
    pub fn flipped(self) -> Option<Rel> {
        match self {
            Rel::InFrontOf => Some(Rel::Behind),
            Rel::Behind => Some(Rel::InFrontOf),
            Rel::LeftOf => Some(Rel::RightOf),
            Rel::RightOf => Some(Rel::LeftOf),
            Rel::Near => Some(Rel::Near),
            Rel::Holding => Some(Rel::HeldBy),
            Rel::HeldBy => Some(Rel::Holding),
            Rel::On => Some(Rel::Under),
            Rel::Under => Some(Rel::On),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Relation {
    pub a: u32,
    pub rel: Rel,
    pub b: u32,
}

/// Everything on screen at `t`.
pub fn snapshot(map: &ObjectMap, t: f32) -> Vec<Spot> {
    map.objects
        .iter()
        .filter_map(|o| {
            let ob = o.at(t)?;
            Some(Spot { id: o.id, kind: o.kind, label: o.label.clone(), bbox: ob.bbox, z: ob.z, wrists: ob.body.map(|b| b.wrists()).unwrap_or_default() })
        })
        .collect()
}

/// Whether the two boxes overlap enough for one to hide part of the other.
fn overlapping(a: &BBox, b: &BBox) -> bool {
    a.inter(b) > 0.15 * a.area().min(b.area())
}

/// Depth gap that counts as "in front": a handful of centimetres is noise
/// in a depth map, so the gap must be a real share of the distance.
fn depth_gap(a: Option<f32>, b: Option<f32>) -> Option<f32> {
    let (a, b) = (a?, b?);
    let need = (0.12 * a.min(b)).max(0.4);
    ((a - b).abs() >= need).then_some(b - a)
}

/// `a` holds `b`: one of a person's hands is on the object, which is small
/// next to the person and about as far away.
fn holding(a: &Spot, b: &Spot) -> bool {
    a.kind == Kind::Person && !matches!(b.kind, Kind::Person | Kind::Scenery) && b.bbox.area() < 0.3 * a.bbox.area() && {
        let reach = b.bbox.grown(1.35);
        a.wrists.iter().any(|w| reach.contains(w[0], w[1])) && depth_gap(a.z, b.z).is_none_or(|g| g.abs() < 1.0)
    }
}

/// `a` rests on `b`: its bottom is at `b`'s top surface (or just inside the
/// top of `b`, seen from above), over `b`, and smaller.
fn on(a: &Spot, b: &Spot) -> bool {
    if a.kind == Kind::Scenery || b.kind == Kind::Scenery || a.bbox.area() >= 0.6 * b.bbox.area() {
        return false;
    }
    let (ab, bb) = (&a.bbox, &b.bbox);
    let over = (ab.right().min(bb.right()) - ab.x.max(bb.x)).max(0.0) / ab.w.max(1e-6);
    let top = bb.y - 0.12 * ab.h;
    let seated = ab.bottom() >= top && ab.bottom() <= bb.y + 0.5 * bb.h && ab.cy() < bb.cy();
    // A person on a seat or a vehicle sits low in it; a thing on a table
    // stands at its far edge. Either way a's middle is above b's middle.
    over >= 0.6 && seated && depth_gap(a.z, b.z).is_none_or(|g| g.abs() < 1.2)
}

fn near(a: &Spot, b: &Spot) -> bool {
    if a.kind == Kind::Scenery || b.kind == Kind::Scenery {
        return false;
    }
    let gx = (a.bbox.x.max(b.bbox.x) - a.bbox.right().min(b.bbox.right())).max(0.0);
    let gy = (a.bbox.y.max(b.bbox.y) - a.bbox.bottom().min(b.bbox.bottom())).max(0.0);
    let gap = (gx * gx + gy * gy).sqrt();
    let size = a.bbox.w.max(a.bbox.h).max(b.bbox.w.max(b.bbox.h));
    gap < 0.45 * size && depth_gap(a.z, b.z).is_none_or(|g| g.abs() < (0.25 * a.z.unwrap_or(4.0)).max(1.0))
}

/// The strongest relations between the pair, in the direction "`a` rel
/// `b`" (a pair can have two: in front of and left of).
pub fn between(a: &Spot, b: &Spot) -> Vec<Rel> {
    let mut out = Vec::new();
    if holding(a, b) {
        out.push(Rel::Holding);
    }
    if on(a, b) {
        out.push(Rel::On);
    }
    if overlapping(&a.bbox, &b.bbox)
        && !out.contains(&Rel::On)
        && !out.contains(&Rel::Holding)
        && let Some(gap) = depth_gap(a.z, b.z)
    {
        out.push(if gap > 0.0 { Rel::InFrontOf } else { Rel::Behind });
    }
    // Side by side: little overlap across, clearly apart.
    let across = (a.bbox.right().min(b.bbox.right()) - a.bbox.x.max(b.bbox.x)).max(0.0) / a.bbox.w.min(b.bbox.w).max(1e-6);
    if across < 0.3 && (a.bbox.cx() - b.bbox.cx()).abs() > 0.5 * (a.bbox.w + b.bbox.w) * 0.6 && out.is_empty() {
        out.push(if a.bbox.cx() < b.bbox.cx() { Rel::LeftOf } else { Rel::RightOf });
    }
    if out.is_empty() && near(a, b) {
        out.push(Rel::Near);
    }
    out
}

/// The relations worth saying at `t`: each pair once with its strongest
/// relation (holding, on, in front of or behind, near); left and right are
/// only given on request ([`around`]).
pub fn all(map: &ObjectMap, t: f32) -> Vec<Relation> {
    let spots = snapshot(map, t);
    let mut out = Vec::new();
    for (i, a) in spots.iter().enumerate() {
        for b in spots.iter().skip(i + 1) {
            let (ab, ba) = (between(a, b), between(b, a));
            let strong = |from: &Spot, to: &Spot, rels: &[Rel]| rels.iter().find(|r| matches!(r, Rel::Holding | Rel::On)).map(|&rel| Relation { a: from.id, rel, b: to.id });
            let pick = strong(a, b, &ab)
                .or_else(|| strong(b, a, &ba))
                .or_else(|| ab.contains(&Rel::InFrontOf).then_some(Relation { a: a.id, rel: Rel::InFrontOf, b: b.id }))
                .or_else(|| ab.contains(&Rel::Behind).then_some(Relation { a: b.id, rel: Rel::InFrontOf, b: a.id }))
                .or_else(|| ab.contains(&Rel::Near).then_some(Relation { a: a.id, rel: Rel::Near, b: b.id }));
            out.extend(pick);
        }
    }
    out
}

/// Everything related to object `id` at `t`, as (other object, how `id`
/// relates to it), strongest first, including left and right.
pub fn around(map: &ObjectMap, t: f32, id: u32) -> Vec<(u32, Rel)> {
    let spots = snapshot(map, t);
    let Some(me) = spots.iter().find(|s| s.id == id) else { return Vec::new() };
    let mut out: Vec<(u32, Rel, f32)> = Vec::new();
    for other in spots.iter().filter(|s| s.id != id) {
        let dist = ((me.bbox.cx() - other.bbox.cx()).powi(2) + (me.bbox.cy() - other.bbox.cy()).powi(2)).sqrt();
        for rel in between(me, other) {
            out.push((other.id, rel, dist));
        }
        // What the other thing does to me, said from my side.
        for rel in between(other, me).into_iter().filter_map(Rel::flipped) {
            if !out.iter().any(|x| x.0 == other.id && x.1 == rel) && !(rel == Rel::Near && out.iter().any(|x| x.0 == other.id)) {
                out.push((other.id, rel, dist));
            }
        }
    }
    out.sort_by(|a, b| rank(a.1).cmp(&rank(b.1)).then(a.2.total_cmp(&b.2)));
    out.into_iter().map(|(i, r, _)| (i, r)).collect()
}

fn rank(r: Rel) -> u8 {
    match r {
        Rel::Holding | Rel::HeldBy => 0,
        Rel::On | Rel::Under => 1,
        Rel::InFrontOf | Rel::Behind => 2,
        Rel::Near => 3,
        Rel::LeftOf | Rel::RightOf => 4,
    }
}

/// Objects in front of (`front` true) or behind `id` at `t`: those that
/// overlap it on screen and are clearly nearer or farther, nearest first
/// for "in front" and farthest last for "behind".
pub fn depth_neighbours(map: &ObjectMap, t: f32, id: u32, front: bool) -> Vec<u32> {
    let spots = snapshot(map, t);
    let Some(me) = spots.iter().find(|s| s.id == id) else { return Vec::new() };
    let mut v: Vec<(&Spot, f32)> = spots
        .iter()
        .filter(|s| s.id != id && s.kind != Kind::Scenery && overlapping(&me.bbox, &s.bbox))
        .filter_map(|s| depth_gap(me.z, s.z).filter(|g| (*g < 0.0) == front).map(|_| (s, s.z.unwrap_or(0.0))))
        .collect();
    v.sort_by(|a, b| a.1.total_cmp(&b.1));
    v.into_iter().map(|(s, _)| s.id).collect()
}

#[cfg(test)]
mod tests {
    use super::super::testing::*;
    use super::*;

    fn spot(id: u32, kind: Kind, label: &str, b: [f32; 4], z: Option<f32>) -> Spot {
        Spot { id, kind, label: label.into(), bbox: BBox::new(b[0], b[1], b[2], b[3]), z, wrists: Vec::new() }
    }

    #[test]
    fn a_cup_stands_on_a_table() {
        let table = spot(1, Kind::Thing, "table", [0.2, 0.55, 0.6, 0.3], Some(3.0));
        let cup = spot(2, Kind::Thing, "cup", [0.45, 0.45, 0.06, 0.12], Some(2.9));
        assert!(between(&cup, &table).contains(&Rel::On));
        assert!(!between(&table, &cup).contains(&Rel::On));
        // A cup far behind the table in depth is not on it.
        let far = spot(3, Kind::Thing, "cup", [0.45, 0.45, 0.06, 0.12], Some(7.0));
        assert!(!between(&far, &table).contains(&Rel::On));
    }

    #[test]
    fn a_person_holds_what_the_hand_touches() {
        let mut p = spot(1, Kind::Person, "person", [0.3, 0.1, 0.25, 0.8], Some(2.5));
        p.wrists = vec![[0.52, 0.55]];
        let phone = spot(2, Kind::Thing, "phone", [0.5, 0.52, 0.05, 0.08], Some(2.4));
        assert!(between(&p, &phone).contains(&Rel::Holding));
        let lamp = spot(3, Kind::Thing, "lamp", [0.8, 0.2, 0.05, 0.1], Some(2.5));
        assert!(!between(&p, &lamp).contains(&Rel::Holding));
        // Too big to be held.
        let sofa = spot(4, Kind::Thing, "sofa", [0.2, 0.3, 0.6, 0.6], Some(2.5));
        assert!(!between(&p, &sofa).contains(&Rel::Holding));
    }

    #[test]
    fn nearer_overlapping_things_are_in_front() {
        let a = spot(1, Kind::Person, "person", [0.3, 0.2, 0.2, 0.6], Some(2.0));
        let b = spot(2, Kind::Thing, "chair", [0.4, 0.4, 0.3, 0.4], Some(4.0));
        assert!(between(&a, &b).contains(&Rel::InFrontOf));
        assert!(between(&b, &a).contains(&Rel::Behind));
        // No clear depth gap: neither.
        let c = spot(3, Kind::Thing, "chair", [0.4, 0.4, 0.3, 0.4], Some(2.2));
        assert!(!between(&a, &c).contains(&Rel::InFrontOf));
    }

    #[test]
    fn side_by_side_is_left_and_right() {
        let a = spot(1, Kind::Thing, "lamp", [0.05, 0.3, 0.1, 0.2], Some(3.0));
        let b = spot(2, Kind::Thing, "clock", [0.8, 0.3, 0.1, 0.2], Some(3.0));
        assert_eq!(between(&a, &b), vec![Rel::LeftOf]);
        assert_eq!(between(&b, &a), vec![Rel::RightOf]);
    }

    #[test]
    fn close_neighbours_are_near_and_far_ones_are_not() {
        let a = spot(1, Kind::Animal, "dog", [0.40, 0.5, 0.15, 0.2], Some(3.0));
        let b = spot(2, Kind::Person, "person", [0.58, 0.2, 0.15, 0.6], Some(3.2));
        assert!(near(&a, &b));
        let c = spot(3, Kind::Person, "person", [0.58, 0.2, 0.15, 0.6], Some(9.0));
        assert!(!near(&a, &c));
    }

    #[test]
    fn the_map_level_answers_use_the_moment() {
        let cup = object(2, "cup", Kind::Thing, vec![obs(0.0, 0.45, 0.45, 0.06, 0.12), obs(4.0, 0.45, 0.45, 0.06, 0.12)]);
        let table = object(1, "table", Kind::Thing, vec![obs(0.0, 0.2, 0.55, 0.6, 0.3), obs(4.0, 0.2, 0.55, 0.6, 0.3)]);
        let late = object(3, "dog", Kind::Animal, vec![obs(3.0, 0.1, 0.6, 0.2, 0.2), obs(4.0, 0.1, 0.6, 0.2, 0.2)]);
        let m = map(vec![table, cup, late]);
        let r = all(&m, 1.0);
        assert!(r.contains(&Relation { a: 2, rel: Rel::On, b: 1 }), "{r:?}");
        assert!(all(&m, 1.0).iter().all(|x| x.a != 3 && x.b != 3), "the dog is not there yet");
        let near_cup = around(&m, 1.0, 2);
        assert_eq!(near_cup.first(), Some(&(1, Rel::On)));
    }

    #[test]
    fn front_and_behind_lists_are_ordered() {
        let mut o1 = object(1, "person", Kind::Person, vec![obs(0.0, 0.3, 0.2, 0.2, 0.6)]);
        let mut o2 = object(2, "tree", Kind::Thing, vec![obs(0.0, 0.2, 0.1, 0.5, 0.8)]);
        let mut o3 = object(3, "fence", Kind::Thing, vec![obs(0.0, 0.25, 0.4, 0.5, 0.3)]);
        let mut o4 = object(4, "car", Kind::Vehicle, vec![obs(0.0, 0.3, 0.5, 0.3, 0.2)]);
        o1.obs[0].z = Some(2.0);
        o2.obs[0].z = Some(9.0);
        o3.obs[0].z = Some(5.0);
        o4.obs[0].z = Some(1.0);
        let m = map(vec![o1, o2, o3, o4]);
        assert_eq!(depth_neighbours(&m, 0.0, 1, false), vec![3, 2], "behind the person, nearest first");
        assert_eq!(depth_neighbours(&m, 0.0, 1, true), vec![4]);
    }
}
