// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Closing a surface: the open edges of a mesh found as loops, and each
//! loop filled with a fan of triangles from its own vertices (no new
//! vertices, so skin weights and shape keys stay in step and the cap
//! follows the rim when it moves). A cap is wound to face the way the
//! surface round it does.
//!
//! Some surfaces are thin by nature and stay open: eyelids, lashes, the
//! cornea and hair cards ([`thin_by_nature`]); a check for holes leaves
//! them out.

use crate::{Mesh, add, length, scale, sub};

/// Materials that are single sheets on purpose: the eyelids, lashes and
/// corneas of a face and hair drawn as cards.
pub fn thin_by_nature(material: &str) -> bool {
    let n = material.trim().to_ascii_lowercase();
    matches!(n.as_str(), "upper lid" | "lower lid" | "lashes" | "cornea") || n.ends_with(" cards") || n.starts_with("hair:")
}

/// The open loops of a mesh, as vertex lists in the order the edges run in
/// their triangles. Corners at the same place count as one.
pub fn open_loops(m: &Mesh) -> Vec<Vec<usize>> {
    let (ids, _) = m.position_ids();
    let mut count: std::collections::HashMap<(u32, u32), u32> = Default::default();
    for t in 0..m.triangles() {
        let c = m.tri(t).map(|v| ids[v]);
        for (a, b) in [(c[0], c[1]), (c[1], c[2]), (c[2], c[0])] {
            if a != b {
                *count.entry((a.min(b), a.max(b))).or_default() += 1;
            }
        }
    }
    // Each open edge, directed as its triangle runs it: welded id → (next id, the raw vertex).
    let mut next: std::collections::HashMap<u32, (u32, usize)> = Default::default();
    let mut forked: std::collections::HashSet<u32> = Default::default();
    for t in 0..m.triangles() {
        let c = m.tri(t);
        for k in 0..3 {
            let (a, b) = (c[k], c[(k + 1) % 3]);
            let (ia, ib) = (ids[a], ids[b]);
            if ia == ib || count[&(ia.min(ib), ia.max(ib))] != 1 {
                continue;
            }
            if next.insert(ia, (ib, a)).is_some() {
                forked.insert(ia);
            }
        }
    }
    let mut loops = Vec::new();
    let mut seen: std::collections::HashSet<u32> = Default::default();
    let mut starts: Vec<u32> = next.keys().copied().collect();
    starts.sort_unstable();
    for s in starts {
        if seen.contains(&s) || forked.contains(&s) {
            continue;
        }
        let mut lp = Vec::new();
        let mut at = s;
        let closed = loop {
            if !seen.insert(at) || forked.contains(&at) {
                break at == s && !lp.is_empty();
            }
            let Some(&(to, raw)) = next.get(&at) else { break false };
            lp.push(raw);
            at = to;
            if lp.len() > 100_000 {
                break false;
            }
        };
        if closed && lp.len() >= 3 {
            loops.push(lp);
        }
    }
    loops
}

fn span(m: &Mesh, lp: &[usize]) -> f32 {
    let c = scale(lp.iter().fold([0.0; 3], |s, &v| add(s, m.positions[v])), 1.0 / lp.len() as f32);
    lp.iter().map(|&v| length(sub(m.positions[v], c))).fold(0.0, f32::max) * 2.0
}

/// Fills every open loop of the mesh that `keep` allows (by the material
/// of the triangle on its rim) and that is at most `max_span` across.
/// Returns how many loops were closed.
pub fn close_holes(m: &mut Mesh, max_span: f32, keep: impl Fn(&str) -> bool) -> usize {
    let loops = open_loops(m);
    if loops.is_empty() {
        return 0;
    }
    let mut mats = m.tri_materials();
    // The material on the rim of each loop: the triangle using its first
    // edge, found by welded corners (a loop's vertices come from different
    // triangles where corners are split).
    let (ids, _) = m.position_ids();
    let mut owner: std::collections::HashMap<(u32, u32), u32> = Default::default();
    for t in 0..m.triangles() {
        let c = m.tri(t);
        for k in 0..3 {
            owner.insert((ids[c[k]], ids[c[(k + 1) % 3]]), mats[t]);
        }
    }
    let mut closed = 0;
    for lp in loops {
        let Some(&mat) = owner.get(&(ids[lp[0]], ids[lp[1]])) else { continue };
        let name = m.materials.get(mat as usize).map_or("", |x| x.name.as_str());
        if thin_by_nature(name) || !keep(name) || span(m, &lp) > max_span {
            continue;
        }
        // The rim runs a → b in its triangles, so the cap runs b → a.
        for i in 1..lp.len() - 1 {
            m.indices.extend_from_slice(&[lp[0] as u32, lp[i + 1] as u32, lp[i] as u32]);
            mats.push(mat);
        }
        closed += 1;
    }
    if closed > 0 {
        m.regroup(&mats);
    }
    closed
}

/// Open edges left once the thin-by-nature sheets are set aside.
pub fn open_edges(m: &Mesh) -> usize {
    let (ids, _) = m.position_ids();
    let mats = m.tri_materials();
    let thin: Vec<bool> = m.materials.iter().map(|x| thin_by_nature(&x.name)).collect();
    let mut count: std::collections::HashMap<(u32, u32), u32> = Default::default();
    for t in 0..m.triangles() {
        if thin.get(mats[t] as usize).copied().unwrap_or(false) {
            continue;
        }
        let c = m.tri(t).map(|v| ids[v]);
        for (a, b) in [(c[0], c[1]), (c[1], c[2]), (c[2], c[0])] {
            if a != b {
                *count.entry((a.min(b), a.max(b))).or_default() += 1;
            }
        }
    }
    count.values().filter(|&&n| n == 1).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_open_box_is_closed() {
        let mut m = crate::prim::cube([10.0; 3]);
        // Take one face (two triangles) off.
        m.indices.truncate(m.indices.len() - 6);
        m.single_group();
        assert!(open_edges(&m) > 0);
        assert_eq!(close_holes(&mut m, 100.0, |_| true), 1);
        assert_eq!(open_edges(&m), 0);
    }
}
