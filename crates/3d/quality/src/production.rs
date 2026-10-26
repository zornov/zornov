// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The finish every AI-made model goes through before anyone sees it:
//! repaired, rebuilt as clean quads at its class's budget, unwrapped,
//! given crease-aware normals and its textures carried over, then judged.
//! Several versions are made (budgets and chart sizes), each scored by the
//! critics, and the best kept; while a bar is missed the finish tries
//! again with the setting that bar answers to (more faces for a missed
//! silhouette, smaller charts for stretch, larger for too many islands).

use crate::score::{Class, ModelReport, critique_model};
use crate::{remesh, repair, transfer, unwrap};
use serde::Serialize;
use studio_mesh::Mesh;

#[derive(Clone, Copy, Debug)]
pub struct Options {
    pub class: Class,
    /// Quads to aim for (the class's budget when None).
    pub quads: Option<usize>,
    /// Texture size for carried-over textures.
    pub texture: u32,
    /// Versions to make and choose from.
    pub candidates: usize,
    /// Extra rounds while bars are missed.
    pub retries: usize,
}

impl Options {
    pub fn new(class: Class) -> Options {
        Options { class, quads: None, texture: 2048, candidates: 2, retries: 2 }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Attempt {
    pub quads: usize,
    pub cone: f32,
    pub score: f32,
    pub passes: bool,
}

pub struct Finished {
    pub mesh: Mesh,
    pub report: ModelReport,
    pub repaired: repair::Repaired,
    pub attempts: Vec<Attempt>,
}

#[derive(Clone, Copy, Debug)]
struct Setting {
    quads: usize,
    cone: f32,
}

fn make(src: &Mesh, colours: Option<&[[f32; 3]]>, s: Setting, texture: u32) -> Mesh {
    let mut m = remesh::remesh(src, &remesh::Params { quads: s.quads, ..Default::default() });
    unwrap::unwrap(&mut m, &unwrap::Params { cone: s.cone, texture, ..Default::default() });
    studio_mesh::ops::recompute_normals(&mut m, 50.0);
    match colours {
        Some(c) => {
            transfer::bake_colours(src, c, &mut m, texture);
        }
        None => {
            transfer::bake(src, &mut m, texture);
        }
    }
    // Names a person can pick materials by.
    for (i, mat) in m.materials.iter_mut().enumerate() {
        if mat.name.trim().is_empty() || mat.name == "Default" {
            mat.name = if i == 0 { "Surface".into() } else { format!("Surface {}", i + 1) };
        }
    }
    m
}

/// Finishes `src`. `reference` is what it should look like (the
/// original itself when there is nothing better).
pub fn finish(src: &Mesh, reference: Option<&Mesh>, o: &Options) -> Finished {
    finish_with(src, None, reference, o)
}

/// Like [`finish`] for a surface whose colour is given per vertex (a scan,
/// or what image-to-3D makes): the colours are baked into the texture.
/// The surface is used as it is (repair would renumber its vertices); it
/// should already be closed.
pub fn finish_with(src: &Mesh, colours: Option<&[[f32; 3]]>, reference: Option<&Mesh>, o: &Options) -> Finished {
    let mut fixed = src.clone();
    let repaired = if colours.is_some() { repair::Repaired::default() } else { repair::repair(&mut fixed) };
    let reference = reference.unwrap_or(src);
    let budget = o.quads.unwrap_or_else(|| o.class.budget());
    let mut queue: Vec<Setting> = vec![Setting { quads: (budget as f32 * 0.8) as usize, cone: 66.0 }];
    if o.candidates > 1 {
        queue.push(Setting { quads: (budget as f32 * 0.55) as usize, cone: 58.0 });
    }
    for k in 2..o.candidates {
        queue.push(Setting { quads: (budget as f32 * (0.35 + 0.1 * k as f32)).max(200.0) as usize, cone: 72.0 });
    }
    let mut attempts = Vec::new();
    let mut best: Option<(Mesh, ModelReport)> = None;
    let mut retries = o.retries;
    while let Some(s) = queue.pop() {
        let m = make(&fixed, colours, s, o.texture);
        let r = critique_model(&m, o.class, Some(reference));
        attempts.push(Attempt { quads: r.quads, cone: s.cone, score: r.scores.overall, passes: r.passes() });
        let better = match &best {
            None => true,
            Some((_, b)) => (r.passes(), r.scores.overall) > (b.passes(), b.scores.overall),
        };
        // What the misses ask for next.
        if !r.passes() && retries > 0 && queue.is_empty() {
            retries -= 1;
            let missed = |n: &str| r.bars.iter().any(|b| b.name == n && !b.pass);
            let mut next = s;
            if missed("silhouette overlap") || missed("surface error") || missed("worst surface error") || missed("torn or rippled surface") || missed("manifold") || missed("watertight") {
                next.quads = ((s.quads as f32 * 1.3) as usize).min(budget);
            }
            if missed("UV stretch") {
                next.cone = (s.cone - 10.0).max(35.0);
            }
            if missed("UV islands") || missed("UV packing") {
                next.cone = (s.cone + 8.0).min(80.0);
            }
            if missed("faces over budget") {
                next.quads = (s.quads as f32 * 0.75) as usize;
            }
            if next.quads != s.quads || next.cone != s.cone {
                queue.push(next);
            }
        }
        if better {
            best = Some((m, r));
        }
    }
    let (mesh, report) = best.unwrap_or_else(|| (fixed.clone(), critique_model(&fixed, o.class, Some(reference))));
    Finished { mesh, report, repaired, attempts }
}
