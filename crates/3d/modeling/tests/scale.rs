// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Sculpting at production density: about two million triangles.

use studio_modeling::edit::Falloff;
use studio_modeling::noise::Pattern;
use studio_modeling::prim;
use studio_modeling::sculpt::{Alpha, Brush, Dyntopo, Kind, SculptMesh, Stroke};

#[test]
fn two_million_triangles_sculpt_interactively() {
    let base = prim::quad_sphere(500.0, 400);
    let t0 = std::time::Instant::now();
    let mut s = SculptMesh::from_poly(&base);
    let setup = t0.elapsed();
    assert!(s.live_triangles() > 1_900_000, "{}", s.live_triangles());
    let brush = Brush { kind: Kind::ClayStrips, radius: 60.0, strength: 0.6, falloff: Falloff::Smooth, invert: false, alpha: Some(Alpha { pattern: Pattern::Pores, size: 3.0, depth: 0.4, seed: 2 }), front_only: true, autosmooth: 0.1, face_set: 0 };
    let points: Vec<[f32; 3]> = (0..20).map(|i| {
        let a = i as f32 * 0.05;
        [500.0 * a.sin(), -500.0 * a.cos(), 0.0]
    }).collect();
    let t1 = std::time::Instant::now();
    let rep = s.stroke(&brush, &Stroke { points, spacing: 0.15, symmetry: [true, false, false], dyntopo: None });
    let stroke = t1.elapsed();
    eprintln!("setup {setup:?}, stroke of {} dabs moving {} points: {stroke:?}", rep.dabs, rep.moved);
    assert!(rep.moved > 1000);
    // Dynamic detail at this density stays local.
    let t2 = std::time::Instant::now();
    s.stroke(&Brush { kind: Kind::Draw, radius: 20.0, ..brush.clone() }, &Stroke { points: vec![[0.0, -500.0, 0.0]], spacing: 0.2, symmetry: [false; 3], dyntopo: Some(Dyntopo { detail: 0.8 }) });
    eprintln!("dyntopo dab {:?}", t2.elapsed());
}
