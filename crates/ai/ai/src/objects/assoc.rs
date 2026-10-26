// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Matching what the detector finds on a key frame with the objects already
//! being followed, so an object keeps its id for as long as it is in the
//! shot: two dogs that cross keep their own ids, a person who is missed on
//! one key frame is still the same person on the next.

use super::{BBox, Kind};

/// An object being followed: where it is predicted to be now.
#[derive(Clone, Debug, PartialEq)]
pub struct Followed {
    pub label: String,
    pub kind: Kind,
    pub at: BBox,
}

/// A detection on the key frame.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    pub label: String,
    pub kind: Kind,
    pub score: f32,
    pub bbox: BBox,
}

/// Names that mean the same thing for matching.
fn same_family(a: &str, b: &str) -> bool {
    const FAMILIES: &[&[&str]] = &[&["television", "computer monitor", "screen"], &["cup", "glass"], &["sign", "road sign", "poster"], &["picture frame", "painting", "mirror"], &["plant", "flower"], &["house", "building"]];
    a == b || FAMILIES.iter().any(|f| f.contains(&a) && f.contains(&b))
}

/// How well a detection continues a followed object, or `None` if it does
/// not.
fn fit(f: &Followed, d: &Found) -> Option<f32> {
    let iou = f.at.iou(&d.bbox);
    let same = same_family(&f.label, &d.label);
    // A moving object may have travelled farther than its box between key
    // frames; allow a match by distance when the sizes agree.
    let size = (f.at.w * f.at.h).sqrt().max(1e-3);
    let dist = ((f.at.cx() - d.bbox.cx()).powi(2) + (f.at.cy() - d.bbox.cy()).powi(2)).sqrt();
    let ratio = (d.bbox.w * d.bbox.h).sqrt() / size;
    if same {
        if iou >= 0.2 {
            return Some(iou + 0.4);
        }
        if dist < 1.2 * size && (0.6..1.7).contains(&ratio) && f.kind != Kind::Scenery {
            return Some(0.2 - dist / (6.0 * size) + 0.1);
        }
        None
    } else if iou >= 0.6 && f.kind == d.kind {
        // The detector changed its mind about the name (chair / sofa).
        Some(iou)
    } else {
        None
    }
}

/// For each detection, the index of the followed object it continues (each
/// object at most once, best matches first), or `None` for a new object.
pub fn associate(followed: &[Followed], found: &[Found]) -> Vec<Option<usize>> {
    let mut pairs: Vec<(f32, usize, usize)> = Vec::new();
    for (i, f) in followed.iter().enumerate() {
        for (j, d) in found.iter().enumerate() {
            if let Some(s) = fit(f, d) {
                pairs.push((s + 0.05 * d.score, i, j));
            }
        }
    }
    pairs.sort_by(|a, b| b.0.total_cmp(&a.0));
    let mut out = vec![None; found.len()];
    let mut used = vec![false; followed.len()];
    for (_, i, j) in pairs {
        if !used[i] && out[j].is_none() {
            used[i] = true;
            out[j] = Some(i);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(label: &str, x: f32, y: f32, w: f32, h: f32) -> Followed {
        Followed { label: label.into(), kind: Kind::Thing, at: BBox::new(x, y, w, h) }
    }
    fn d(label: &str, x: f32, y: f32, w: f32, h: f32) -> Found {
        Found { label: label.into(), kind: Kind::Thing, score: 0.8, bbox: BBox::new(x, y, w, h) }
    }

    #[test]
    fn a_moved_object_keeps_its_identity() {
        let followed = [f("dog", 0.10, 0.5, 0.2, 0.2), f("dog", 0.60, 0.5, 0.2, 0.2)];
        // Found in the opposite order, each moved a little.
        let found = [d("dog", 0.64, 0.5, 0.2, 0.2), d("dog", 0.14, 0.5, 0.2, 0.2)];
        assert_eq!(associate(&followed, &found), vec![Some(1), Some(0)]);
    }

    #[test]
    fn crossing_objects_do_not_swap() {
        // Tracker predictions already carry each across; close but distinct.
        let followed = [f("person", 0.42, 0.2, 0.15, 0.6), f("person", 0.46, 0.2, 0.15, 0.6)];
        let found = [d("person", 0.40, 0.2, 0.15, 0.6), d("person", 0.48, 0.2, 0.15, 0.6)];
        assert_eq!(associate(&followed, &found), vec![Some(0), Some(1)]);
    }

    #[test]
    fn a_new_arrival_is_new_and_a_far_jump_is_not_a_match() {
        let followed = [f("cat", 0.1, 0.1, 0.1, 0.1)];
        let found = [d("cat", 0.8, 0.8, 0.1, 0.1), d("cat", 0.12, 0.1, 0.1, 0.1)];
        assert_eq!(associate(&followed, &found), vec![None, Some(0)]);
    }

    #[test]
    fn a_renamed_object_matches_on_overlap_only() {
        let followed = [f("chair", 0.2, 0.4, 0.3, 0.4)];
        assert_eq!(associate(&followed, &[d("sofa", 0.21, 0.4, 0.3, 0.4)]), vec![Some(0)]);
        assert_eq!(associate(&followed, &[d("lamp", 0.21, 0.4, 0.3, 0.4)]).len(), 1);
        assert_eq!(associate(&followed, &[d("sofa", 0.4, 0.45, 0.3, 0.4)]), vec![None]);
    }

    #[test]
    fn related_names_are_one_family() {
        let followed = [f("television", 0.2, 0.2, 0.3, 0.2)];
        assert_eq!(associate(&followed, &[d("computer monitor", 0.22, 0.2, 0.3, 0.2)]), vec![Some(0)]);
    }
}
