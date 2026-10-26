// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Every operation in the table, run on a mesh it suits: the result is a
//! valid mesh (faces wound one way, nothing out of range), closed and
//! facing out where it should be, with the counts it should have, and it
//! survives saving and loading unchanged.

use super::*;
use crate::prim;
use crate::select::Faces;
use serde_json::json;
use std::collections::BTreeSet;

fn args(v: Value) -> Map<String, Value> {
    v.as_object().cloned().unwrap_or_default()
}

/// Faces of three or more distinct corners in range; every directed edge
/// used once (one winding); per-face lists the right length.
fn valid(p: &Poly) -> Result<(), String> {
    let nv = p.positions.len() as u32;
    let mut directed = HashSet::new();
    for f in 0..p.faces() {
        let c = p.face(f);
        if c.len() < 3 {
            return Err(format!("face {f} has {} corners", c.len()));
        }
        let distinct: HashSet<u32> = c.iter().copied().collect();
        if distinct.len() != c.len() {
            return Err(format!("face {f} repeats a corner: {c:?}"));
        }
        for i in 0..c.len() {
            if c[i] >= nv {
                return Err(format!("face {f} uses missing vertex {}", c[i]));
            }
            if !directed.insert((c[i], c[(i + 1) % c.len()])) {
                return Err(format!("edge {}->{} is run the same way by two faces (face {f})", c[i], c[(i + 1) % c.len()]));
            }
        }
    }
    if p.face_material.len() != p.faces() || p.face_set.len() != p.faces() || p.face_smooth.len() != p.faces() {
        return Err("per-face lists don't match the faces".into());
    }
    if !p.uvs.is_empty() && p.uvs.len() != p.corners.len() {
        return Err("texture coordinates don't match the corners".into());
    }
    if p.positions.iter().any(|q| q.iter().any(|x| !x.is_finite())) {
        return Err("a vertex is not a number".into());
    }
    if p.face_set.iter().any(|s| s & (1 << 30) != 0) {
        return Err("a tool left its face tag behind".into());
    }
    Ok(())
}

fn closed(p: &Poly) -> bool {
    let t = Topology::of(p);
    t.edge_faces.values().all(|f| f.len() == 2)
}

fn outward(p: &Poly) -> bool {
    let mut q = p.clone();
    q.orient_outward();
    q.corners == p.corners
}

fn volume(p: &Poly) -> f32 {
    crate::ops::volume(p)
}

fn saves(p: &Poly) {
    let back = crate::asset::poly_from_bytes(&crate::asset::poly_bytes(p)).unwrap();
    assert_eq!(back.positions, p.positions);
    assert_eq!(back.corners, p.corners);
    assert_eq!(back.face_material, p.face_material);
    assert_eq!(back.face_smooth, p.face_smooth);
    assert_eq!(back.seams, p.seams);
    assert_eq!(back.sharp, p.sharp);
    assert_eq!(back.creases, p.creases);
    assert_eq!(back.has_uvs(), p.has_uvs());
}

struct Case {
    p: Poly,
    s: Selection,
}

impl Case {
    fn new(p: Poly, mode: Mode) -> Case {
        let s = Selection::new(&p, mode);
        Case { p, s }
    }
    fn faces(mut self, f: &Faces) -> Case {
        let l = f.list(&self.p);
        self.s.with_faces(&self.p, &l);
        self
    }
    fn verts(mut self, test: impl Fn(V3) -> bool) -> Case {
        let l: Vec<u32> = (0..self.p.positions.len() as u32).filter(|&v| test(self.p.positions[v as usize])).collect();
        self.s.with_vertices(&self.p, &l);
        self
    }
    fn edges(mut self, l: &[Edge]) -> Case {
        self.s.with_edges(&self.p, l);
        self
    }
    fn go(&mut self, id: &str, a: Value) -> Done {
        let ctx = Ctx::for_poly(&self.p);
        let d = run(&mut self.p, &mut self.s, id, &args(a), &ctx).unwrap_or_else(|e| panic!("{id}: {e}"));
        valid(&self.p).unwrap_or_else(|e| panic!("{id}: {e}"));
        saves(&self.p);
        // The pick stays in range and in step.
        assert_eq!(self.s.verts.len(), self.p.positions.len(), "{id}: pick out of step");
        assert_eq!(self.s.faces.len(), self.p.faces(), "{id}: pick out of step");
        d
    }
    fn go_at(&mut self, id: &str, a: Value, ctx: Ctx) -> Done {
        let d = run(&mut self.p, &mut self.s, id, &args(a), &ctx).unwrap_or_else(|e| panic!("{id}: {e}"));
        valid(&self.p).unwrap_or_else(|e| panic!("{id}: {e}"));
        saves(&self.p);
        d
    }
    fn solid(&self, id: &str) {
        assert!(closed(&self.p), "{id}: has holes");
        assert!(outward(&self.p), "{id}: faces turned in");
    }
}

fn cube1() -> Poly {
    prim::cube([100.0; 3], 1)
}
fn cube2() -> Poly {
    prim::cube([100.0; 3], 2)
}
fn top() -> Faces {
    Faces::named("top").unwrap()
}
fn open_cube() -> Poly {
    let mut c = cube1();
    let m = top().mask(&c);
    edit::delete_faces(&mut c, &m);
    c
}
fn edge_where(p: &Poly, test: impl Fn(V3, V3) -> bool) -> Edge {
    p.edges().into_iter().find(|e| test(p.positions[e.0 as usize], p.positions[e.1 as usize])).expect("an edge")
}

#[test]
fn every_operation_runs_and_leaves_a_sound_mesh() {
    let mut tested: BTreeSet<&str> = BTreeSet::new();
    let mut t = |id: &'static str| {
        assert!(find(id).is_some(), "{id} isn't in the table");
        tested.insert(id);
    };

    // ------------------------------------------------------ select
    let mut c = Case::new(cube2(), Mode::Face);
    t("select_all");
    c.go("select_all", json!({}));
    assert_eq!(c.s.count().2, 24);
    t("select_none");
    c.go("select_none", json!({}));
    assert_eq!(c.s.count().2, 0);
    let mut c = Case::new(cube2(), Mode::Face).faces(&top());
    t("select_invert");
    c.go("select_invert", json!({}));
    assert_eq!(c.s.count().2, 20);
    let mut c = Case::new(cube2(), Mode::Edge);
    let mid = edge_where(&c.p, |a, b| a[1].abs() < 1e-3 && b[1].abs() < 1e-3 && a[2] < -49.0 && b[2] < -49.0);
    t("select_loop");
    c.go("select_loop", json!({"edge": [mid.0, mid.1]}));
    assert_eq!(c.s.count().1, 8, "the loop round the middle");
    t("select_ring");
    c.go("select_ring", json!({"edge": [mid.0, mid.1]}));
    assert!(c.s.count().1 >= 2);
    let mut c = Case::new(cube2(), Mode::Face).faces(&Faces::List(vec![0]));
    t("select_linked");
    c.go("select_linked", json!({}));
    assert_eq!(c.s.count().2, 24);
    let mut c = Case::new(cube2(), Mode::Face).faces(&Faces::List(vec![0]));
    t("select_more");
    c.go("select_more", json!({}));
    assert!(c.s.count().2 > 1);
    let mut c = Case::new(cube2(), Mode::Face).faces(&top());
    t("select_less");
    c.go("select_less", json!({}));
    assert!(c.s.count().2 < 4);
    let first_top = top().list(&cube2())[0];
    let mut c = Case::new(cube2(), Mode::Face).faces(&Faces::List(vec![first_top]));
    t("select_similar");
    c.go("select_similar", json!({"by": "normal", "threshold": 1.0}));
    assert_eq!(c.s.count().2, 4);
    let mut c = Case::new(cube2(), Mode::Face).faces(&top());
    t("select_boundary");
    c.go("select_boundary", json!({}));
    assert_eq!(c.s.count().1, 8);
    let mut c = Case::new(open_cube(), Mode::Edge);
    t("select_non_manifold");
    c.go("select_non_manifold", json!({}));
    assert_eq!(c.s.count().1, 4);
    let mut c = Case::new(cube2(), Mode::Edge);
    t("select_sharp");
    c.go("select_sharp", json!({"angle": 30}));
    assert_eq!(c.s.count().1, 24);
    let mut c = Case::new(cube2(), Mode::Face);
    t("select_random");
    c.go("select_random", json!({"percent": 50, "seed": 3}));
    assert!(c.s.count().2 > 0 && c.s.count().2 < 24);
    let mut c = Case::new(cube2(), Mode::Face);
    t("select_sides");
    c.go("select_sides", json!({"sides": 4}));
    assert_eq!(c.s.count().2, 24);
    t("select_material");
    c.go("select_material", json!({"slot": 0}));
    assert_eq!(c.s.count().2, 24);
    let mut c = Case::new(cube2(), Mode::Vertex).verts(|q| q[0] > 1.0);
    let n = c.s.count().0;
    t("select_mirror");
    c.go("select_mirror", json!({"axis": "x"}));
    assert_eq!(c.s.count().0, n);
    assert!(c.s.vert_list().iter().all(|&v| c.p.positions[v as usize][0] < -1.0));
    let mut c = Case::new(cube2(), Mode::Face).faces(&top());
    t("hide");
    c.go("hide", json!({}));
    assert_eq!(c.s.visible(&c.p).faces.iter().filter(|b| **b).count(), 20);
    t("reveal");
    c.go("reveal", json!({}));
    assert!(c.s.visible(&c.p).faces.iter().all(|b| *b));
    c.go("select_none", json!({}));
    let mut c = Case::new(cube2(), Mode::Face).faces(&top());
    t("hide_unselected");
    c.go("hide_unselected", json!({}));
    assert_eq!(c.s.visible(&c.p).faces.iter().filter(|b| **b).count(), 4);

    // ------------------------------------------------------ add
    for (id, solid) in [("add_cube", true), ("add_plane", false), ("add_grid", false), ("add_circle", false), ("add_uv_sphere", true), ("add_ico_sphere", true), ("add_quad_sphere", true), ("add_cylinder", true), ("add_cone", true), ("add_torus", true), ("add_head", true), ("add_curve", true)] {
        t(id);
        let mut c = Case::new(cube1(), Mode::Face);
        let ctx = Ctx { cursor: [400.0, 0.0, 0.0], ..Ctx::for_poly(&c.p) };
        c.go_at(id, json!({}), ctx);
        let made = c.s.count().2;
        assert!(made > 0, "{id} picks what it made");
        assert_eq!(c.p.faces(), 6 + made, "{id}");
        let piece = c.s.face_pick();
        let mut alone = c.p.clone();
        alone.retain_faces(|f| piece[f]);
        alone.compact();
        let (lo, hi) = alone.bounds();
        assert!(((lo[0] + hi[0]) / 2.0 - 400.0).abs() < 60.0, "{id} sits at the cursor ({lo:?} {hi:?})");
        if solid {
            assert!(closed(&alone), "{id} has holes");
            if id != "add_torus" && id != "add_curve" {
                assert!(outward(&alone), "{id} faces in");
            }
        }
    }
    // Shapes on their own, for new objects, follow their settings.
    let ctx = Ctx::default();
    let g = primitive("add_grid", &args(json!({"x": 4, "y": 3, "size": 10})), &ctx).unwrap();
    assert_eq!(g.faces(), 12);
    let cyl = primitive("add_cylinder", &args(json!({"vertices": 12, "cap": "none"})), &ctx).unwrap();
    assert_eq!(cyl.faces(), 12);

    // ------------------------------------------------------ mesh
    let mut c = Case::new(cube1(), Mode::Face).faces(&top());
    t("extrude");
    c.go("extrude", json!({"distance": 50}));
    c.p.compact();
    assert_eq!(c.p.faces(), 10);
    c.solid("extrude");
    assert!((volume(&c.p) - 1.5e6).abs() < 10.0, "{}", volume(&c.p));
    let mut c = Case::new(prim::plane(100.0, 100.0, 1, 1), Mode::Edge);
    let e = c.p.edges()[0];
    c = c.edges(&[e]);
    c.go("extrude", json!({"distance": 30}));
    assert_eq!(c.p.faces(), 2);
    assert_eq!(c.s.count().1, 1, "the new edge is picked");

    let mut c = Case::new(cube1(), Mode::Face).faces(&top());
    t("duplicate");
    c.go("duplicate", json!({}));
    assert_eq!(c.p.faces(), 7);
    assert_eq!(c.s.count().2, 1);

    for (what, faces) in [("vertices", 3), ("edges", 4), ("faces", 5)] {
        t("delete");
        let mut c = Case::new(cube1(), Mode::Face).faces(&top());
        if what == "vertices" {
            c.s.mode = Mode::Vertex;
            c.s.with_vertices(&c.p, &[0]);
        }
        if what == "edges" {
            c.s.mode = Mode::Edge;
            let e = c.p.edges()[0];
            c.s.with_edges(&c.p, &[e]);
        }
        c.go("delete", json!({"what": what}));
        assert_eq!(c.p.faces(), faces, "delete {what}");
    }
    let mut c = Case::new(cube2(), Mode::Edge);
    let mid = edge_where(&c.p, |a, b| a[1].abs() < 1e-3 && b[1].abs() < 1e-3 && a[2] < -49.0 && b[2] < -49.0);
    c.go("select_loop", json!({"edge": [mid.0, mid.1]}));
    c.go("delete", json!({"what": "edge_loops"}));
    assert_eq!(c.p.faces(), 16);
    c.solid("delete edge loop");

    t("dissolve");
    let mut c = Case::new(cube2(), Mode::Vertex).verts(|q| q[1] < -49.0 && q[0].abs() < 1.0 && q[2].abs() < 1.0);
    c.go("dissolve", json!({}));
    assert_eq!(c.p.faces(), 21);
    c.solid("dissolve vertex");
    let mut c = Case::new(cube2(), Mode::Face).faces(&top());
    c.go("dissolve", json!({}));
    assert_eq!(c.p.faces(), 21);
    c.solid("dissolve faces");
    let mut c = Case::new(cube2(), Mode::Edge);
    let mid = edge_where(&c.p, |a, b| a[1].abs() < 1e-3 && b[1].abs() < 1e-3 && a[2] < -49.0 && b[2] < -49.0);
    c.go("select_loop", json!({"edge": [mid.0, mid.1]}));
    c.go("dissolve", json!({"what": "edges"}));
    assert_eq!(c.p.faces(), 16);
    c.solid("dissolve edges");

    t("merge");
    for at in ["centre", "cursor", "first", "last", "collapse"] {
        let mut c = Case::new(cube1(), Mode::Vertex).verts(|q| q[1] < 0.0);
        c.go("merge", json!({"at": at}));
        assert_eq!(c.p.faces(), 5, "merge at {at}: a pyramid");
        assert_eq!(c.p.positions.len(), 5, "merge at {at}");
        c.solid("merge");
        assert_eq!(c.s.count().0, 1, "the merged vertex stays picked");
    }
    let mut c = Case::new(cube1(), Mode::Vertex);
    let copy = c.p.clone();
    c.p.merge(&copy);
    c.s = Selection::new(&c.p, Mode::Vertex);
    c.go("select_all", json!({}));
    c.go("merge", json!({"at": "distance", "distance": 0.5}));
    assert_eq!(c.p.positions.len(), 8);

    let mut c = Case::new(cube1(), Mode::Face).faces(&top());
    t("split");
    c.go("split", json!({}));
    assert_eq!(Topology::of(&c.p).boundary_edges().len(), 8);
    assert_eq!(c.s.count().2, 1);

    let mut c = Case::new(cube2(), Mode::Face).faces(&top());
    t("separate");
    let d = c.go("separate", json!({}));
    assert_eq!(d.pieces.len(), 1);
    assert_eq!(d.pieces[0].faces(), 4);
    assert_eq!(c.p.faces(), 20);
    let mut two = cube1();
    let mut other = cube1();
    other.map_positions(|q| add(q, [300.0, 0.0, 0.0]));
    other.face_material = vec![1; 6];
    two.merge(&other);
    let mut c = Case::new(two.clone(), Mode::Face);
    let d = c.go("separate", json!({"by": "loose_parts"}));
    assert_eq!((d.pieces.len(), c.p.faces()), (1, 6));
    let mut c = Case::new(two, Mode::Face);
    let d = c.go("separate", json!({"by": "material"}));
    assert_eq!((d.pieces.len(), c.p.faces()), (1, 6));

    let mut c = Case::new(cube1(), Mode::Vertex);
    c.p.map_positions(|q| add(q, [30.0, 0.0, 0.0]));
    c.go("select_all", json!({}));
    t("mirror");
    c.go_at("mirror", json!({"axis": "x"}), Ctx { pivot: Some([0.0; 3]), ..Ctx::for_poly(&cube1()) });
    let (lo, hi) = c.p.bounds();
    assert!((lo[0] + 80.0).abs() < 1e-3 && (hi[0] - 20.0).abs() < 1e-3, "{lo:?} {hi:?}");
    c.solid("mirror");

    let mut c = Case::new(cube1(), Mode::Vertex);
    c.p.map_positions(|q| add(q, [30.0, 0.0, 0.0]));
    t("symmetrize");
    c.go("symmetrize", json!({"axis": "x", "direction": "positive_to_negative"}));
    let (lo, hi) = c.p.bounds();
    assert!((lo[0] + hi[0]).abs() < 1e-3, "{lo:?} {hi:?}");
    c.solid("symmetrize");

    let mut c = Case::new(cube1(), Mode::Vertex);
    c.go("select_all", json!({}));
    let v0 = volume(&c.p);
    t("shrink_fatten");
    c.go("shrink_fatten", json!({"distance": 10}));
    assert!(volume(&c.p) > v0);
    let mut c = Case::new(cube2(), Mode::Vertex);
    c.go("select_all", json!({}));
    t("to_sphere");
    c.go("to_sphere", json!({"factor": 1.0}));
    let r: Vec<f32> = c.p.positions.iter().map(|q| length(*q)).collect();
    let r0 = r[0];
    assert!(r.iter().all(|x| (x - r0).abs() < 1e-2), "{r:?}");
    let before = c.p.positions.clone();
    t("smooth_vertices");
    c.go("smooth_vertices", json!({"iterations": 2}));
    t("randomize");
    c.go("randomize", json!({"amount": 5, "seed": 2}));
    assert_ne!(before, c.p.positions);
    let mut c = Case::new(cube1(), Mode::Face).faces(&top());
    t("selection_to_cursor");
    c.go_at("selection_to_cursor", json!({}), Ctx { cursor: [0.0, -200.0, 0.0], ..Ctx::default() });
    assert!((c.s.median(&c.p).unwrap()[1] + 200.0).abs() < 1e-3);

    let mut c = Case::new(cube1(), Mode::Face);
    t("bisect");
    c.go("bisect", json!({"point": [0, 0, 0], "normal": [1, 0, 0], "clear": "front", "fill": true}));
    c.solid("bisect");
    assert!((volume(&c.p) - 5e5).abs() < 10.0);
    let mut c = Case::new(cube1(), Mode::Face);
    t("knife");
    c.go("knife", json!({"point": [0, 0, 0], "normal": [1, 0, 0]}));
    assert_eq!(c.p.faces(), 10);
    c.solid("knife");
    assert_eq!(c.s.count().1, 4, "the cut is picked");

    for id in ["spin", "screw"] {
        t(id);
        let mut c = Case::new(prim::plane(100.0, 100.0, 1, 1), Mode::Edge);
        c.p.map_positions(|q| add(q, [200.0, 0.0, 0.0]));
        let e = edge_where(&c.p, |a, b| (a[0] - b[0]).abs() < 1e-3 && a[0] > 240.0);
        c = c.edges(&[e]);
        let a = if id == "spin" { json!({"degrees": 180, "steps": 12, "axis": "y"}) } else { json!({"turns": 2, "steps": 12, "height": 50, "axis": "y"}) };
        c.go(id, a);
        assert_eq!(c.p.faces(), 1 + if id == "spin" { 12 } else { 24 }, "{id}");
    }

    let mut c = Case::new(cube1(), Mode::Edge);
    c.go("select_all", json!({}));
    t("subdivide");
    c.go("subdivide", json!({"cuts": 1}));
    assert_eq!(c.p.faces(), 24);
    c.solid("subdivide");
    let mut c = Case::new(cube1(), Mode::Face).faces(&top());
    c.go("subdivide", json!({"cuts": 2}));
    assert_eq!(c.s.count().2, 9, "the new grid stays picked");
    c.solid("subdivide faces");

    let mut c = Case::new(cube1(), Mode::Face);
    c.go("select_all", json!({}));
    t("triangulate");
    c.go("triangulate", json!({}));
    assert_eq!((c.p.faces(), c.s.count().2), (12, 12));
    c.solid("triangulate");
    t("tris_to_quads");
    c.go("tris_to_quads", json!({}));
    assert_eq!(c.p.faces(), 6);
    c.solid("tris_to_quads");

    // ------------------------------------------------------ vertex
    let mut c = Case::new(cube1(), Mode::Vertex);
    c.s.with_vertices(&c.p, &[0]);
    t("bevel_vertices");
    c.go("bevel_vertices", json!({"width": 20}));
    assert_eq!(c.p.faces(), 7);
    c.solid("bevel_vertices");
    let mut c = Case::new(prim::plane(100.0, 100.0, 1, 1), Mode::Vertex);
    let diagonal = [c.p.face(0)[0], c.p.face(0)[2]];
    c.s.with_vertices(&c.p, &diagonal);
    t("connect");
    c.go("connect", json!({}));
    assert_eq!(c.p.faces(), 2);
    let mut c = Case::new(open_cube(), Mode::Vertex).verts(|q| q[1] < 0.0);
    t("make_face");
    c.go("make_face", json!({}));
    c.solid("make_face");
    let mut c = Case::new(cube1(), Mode::Vertex);
    c.s.with_vertices(&c.p, &[0]);
    let was = c.p.positions[0];
    t("vertex_slide");
    c.go("vertex_slide", json!({"factor": 0.5, "toward": [1, 0, 0]}));
    assert!((length(sub(c.p.positions[0], was)) - 50.0).abs() < 1e-3);

    // ------------------------------------------------------ edge
    let mut c = Case::new(cube1(), Mode::Edge);
    let e = c.p.edges()[0];
    c = c.edges(&[e]);
    t("bevel");
    c.go("bevel", json!({"width": 10, "segments": 1}));
    assert_eq!(c.p.faces(), 7);
    c.solid("bevel");
    let mut c = Case::new(cube1(), Mode::Edge);
    let e = edge_where(&c.p, |a, b| (a[1] - b[1]).abs() > 1.0);
    t("loop_cut");
    c.go("loop_cut", json!({"edge": [e.0, e.1], "cuts": 1}));
    assert_eq!(c.p.faces(), 10);
    c.solid("loop_cut");
    assert_eq!(c.s.count().1, 4, "the new loop is picked");
    let ys: Vec<f32> = c.s.vert_list().iter().map(|&v| c.p.positions[v as usize][1]).collect();
    t("edge_slide");
    c.go("edge_slide", json!({"factor": 0.5}));
    let ys2: Vec<f32> = c.s.vert_list().iter().map(|&v| c.p.positions[v as usize][1]).collect();
    assert!(ys.iter().zip(&ys2).all(|(a, b)| (a - b).abs() > 10.0), "{ys:?} {ys2:?}");
    c.solid("edge_slide");

    let mut two = cube1();
    let mut other = cube1();
    other.map_positions(|q| add(q, [300.0, 0.0, 0.0]));
    two.merge(&other);
    let facing: Vec<bool> = (0..two.faces()).map(|f| {
        let ctr = two.face_centre(f)[0];
        (ctr - 50.0).abs() < 1.0 || (ctr - 250.0).abs() < 1.0
    }).collect();
    edit::delete_faces(&mut two, &facing);
    let rims = Topology::of(&two).boundary_edges();
    let mut c = Case::new(two, Mode::Edge).edges(&rims);
    t("bridge");
    c.go("bridge", json!({"segments": 2}));
    c.solid("bridge");
    let mut two = cube1();
    let mut other = cube1();
    other.map_positions(|q| add(q, [300.0, 0.0, 0.0]));
    two.merge(&other);
    let mut c = Case::new(two, Mode::Face);
    let facing: Vec<u32> = (0..c.p.faces() as u32).filter(|&f| {
        let ctr = c.p.face_centre(f as usize)[0];
        (ctr - 50.0).abs() < 1.0 || (ctr - 250.0).abs() < 1.0
    }).collect();
    c.s.with_faces(&c.p, &facing);
    c.go("bridge", json!({}));
    c.solid("bridge faces");

    let mut c = Case::new(open_cube(), Mode::Edge);
    let rim = Topology::of(&c.p).boundary_edges();
    c = c.edges(&rim);
    t("fill");
    c.go("fill", json!({}));
    c.solid("fill");
    let mut c = Case::new(prim::cylinder(50.0, 50.0, 100.0, 8, 1, prim::Cap::None), Mode::Edge);
    let rim: Vec<Edge> = Topology::of(&c.p).boundary_edges().into_iter().filter(|e| c.p.positions[e.0 as usize][1] < 0.0).collect();
    c = c.edges(&rim);
    t("grid_fill");
    c.go("grid_fill", json!({}));
    assert!(c.s.count().2 >= 2);
    let rim2 = Topology::of(&c.p).boundary_edges();
    c.s.with_edges(&c.p, &rim2);
    c.go("grid_fill", json!({}));
    c.solid("grid_fill");

    let mut c = Case::new(cube1(), Mode::Edge);
    c.go("select_all", json!({}));
    t("mark_seam");
    c.go("mark_seam", json!({}));
    assert_eq!(c.p.seams.len(), 12);
    c.go("mark_seam", json!({"clear": true}));
    assert!(c.p.seams.is_empty());
    t("mark_sharp");
    c.go("mark_sharp", json!({}));
    assert_eq!(c.p.sharp.len(), 12);
    t("crease");
    c.go("crease", json!({"amount": 0.5}));
    assert_eq!(c.p.creases.len(), 12);
    let mut c = Case::new(cube1(), Mode::Edge);
    t("mark_sharp_by_angle");
    c.go("mark_sharp_by_angle", json!({"angle": 30, "seam": true}));
    assert_eq!((c.p.sharp.len(), c.p.seams.len()), (12, 12));

    // ------------------------------------------------------ face
    for (id, faces) in [("extrude_region", 32), ("extrude_normals", 32), ("extrude_individual", 40)] {
        t(id);
        let mut c = Case::new(cube2(), Mode::Face).faces(&top());
        c.go(id, json!({"distance": 20}));
        c.p.compact();
        assert_eq!(c.p.faces(), faces, "{id}");
        c.solid(id);
        assert_eq!(c.s.count().2, 4, "{id}: the moved faces stay picked");
    }
    let mut c = Case::new(cube1(), Mode::Face).faces(&top());
    t("inset");
    c.go("inset", json!({"thickness": 10}));
    c.p.compact();
    assert_eq!(c.p.faces(), 10);
    c.solid("inset");
    let mut c = Case::new(cube1(), Mode::Face).faces(&top());
    t("poke");
    c.go("poke", json!({"offset": -10}));
    assert_eq!(c.p.faces(), 9);
    assert_eq!(c.s.count().2, 4);
    c.solid("poke");
    let mut c = Case::new(prim::plane(100.0, 100.0, 2, 2), Mode::Face);
    c.go("select_all", json!({}));
    t("solidify");
    c.go("solidify", json!({"thickness": 5}));
    c.solid("solidify");
    let mut c = Case::new(prim::plane(100.0, 100.0, 2, 2), Mode::Face);
    c.go("select_all", json!({}));
    c.go("solidify", json!({"thickness": -5}));
    c.solid("solidify inward");
    let mut c = Case::new(cube1(), Mode::Face).faces(&top());
    t("assign_material");
    c.go("assign_material", json!({"slot": 1}));
    assert!(c.p.materials.len() >= 2);
    assert_eq!(c.p.face_material.iter().filter(|m| **m == 1).count(), 1);
    t("shade_flat");
    c.go("shade_flat", json!({}));
    assert_eq!(c.p.face_smooth.iter().filter(|s| !**s).count(), 1);
    t("shade_smooth");
    c.go("shade_smooth", json!({}));
    assert!(c.p.face_smooth.iter().all(|s| *s));

    // ------------------------------------------------------ normals
    let mut c = Case::new(cube1(), Mode::Face);
    c.go("select_all", json!({}));
    t("flip_normals");
    c.go("flip_normals", json!({}));
    assert!(!outward(&c.p));
    t("recalc_outside");
    c.go("recalc_outside", json!({}));
    c.solid("recalc_outside");
    t("recalc_inside");
    c.go("recalc_inside", json!({}));
    assert!(volume(&c.p) < 0.0);

    // ------------------------------------------------------ clean up
    let mut c = Case::new(cube1(), Mode::Vertex);
    let copy = c.p.clone();
    c.p.merge(&copy);
    c.s = Selection::new(&c.p, Mode::Vertex);
    t("merge_by_distance");
    c.go("merge_by_distance", json!({"distance": 0.5}));
    assert_eq!(c.p.positions.len(), 8);
    let mut c = Case::new(open_cube(), Mode::Face);
    t("fill_holes");
    c.go("fill_holes", json!({}));
    c.solid("fill_holes");
    let mut c = Case::new(cube1(), Mode::Face);
    c.go("triangulate", json!({}));
    t("limited_dissolve");
    c.go("limited_dissolve", json!({"angle": 1}));
    assert_eq!(c.p.faces(), 6);
    c.solid("limited_dissolve");
    t("fix_t_junctions");
    c.go("fix_t_junctions", json!({}));
    let mut c = Case::new(prim::uv_sphere(50.0, 24, 12), Mode::Face);
    let n = c.p.faces();
    t("decimate");
    c.go("decimate", json!({"ratio": 0.5}));
    assert!(c.p.faces() < n);

    // ------------------------------------------------------ uv
    let mut c = Case::new(cube2(), Mode::Face);
    c.p.uvs.clear();
    c.go("mark_sharp_by_angle", json!({"angle": 30, "seam": true}));
    t("unwrap");
    c.go("unwrap", json!({"method": "seams"}));
    assert!(c.p.has_uvs());
    t("pack_uvs");
    c.go("pack_uvs", json!({}));
    assert!(c.p.uvs.iter().all(|u| u[0] >= -1e-3 && u[0] <= 1.001 && u[1] >= -1e-3 && u[1] <= 1.001));
    t("project_uvs");
    for kind in ["box", "cylinder", "sphere", "view"] {
        c.go("project_uvs", json!({"kind": kind}));
        assert!(c.p.has_uvs());
    }
    t("reset_uvs");
    c.go("select_all", json!({}));
    c.go("reset_uvs", json!({}));
    t("clear_seams");
    c.go("clear_seams", json!({}));
    assert!(c.p.seams.is_empty());

    let all: BTreeSet<&str> = TOOLS.iter().map(|t| t.id).collect();
    let missing: Vec<&&str> = all.difference(&tested).collect();
    assert!(missing.is_empty(), "operations without a test: {missing:?}");
}

#[test]
fn the_table_is_well_formed_and_searchable() {
    let mut ids = HashSet::new();
    let mut keys = HashSet::new();
    for t in TOOLS {
        assert!(ids.insert(t.id), "{} twice", t.id);
        assert!(!t.label.is_empty() && !t.tip.is_empty(), "{} needs a label and a tip", t.id);
        assert!(t.tip.ends_with('.'), "{}'s tip is a sentence", t.id);
        if !t.keys.is_empty() && !t.keys.contains("Click") {
            assert!(keys.insert(t.keys), "{} and another share {}", t.id, t.keys);
        }
        if let Follow::Drag(k) = t.follow {
            assert!(t.param(k).is_some(), "{} drags a setting it doesn't have", t.id);
        }
        // Defaults are in range.
        let ctx = Ctx::default();
        for p in t.params {
            let v = default_value(p, &ctx);
            if let Kind::Choice { options, .. } = p.kind {
                assert!(options.contains(&v.as_str().unwrap()));
            }
        }
    }
    assert_eq!(search("bev")[0].id, "bevel");
    assert!(search("bevel").iter().any(|t| t.id == "bevel_vertices"));
    assert_eq!(search("loop cut")[0].id, "loop_cut");
    assert!(search("zzzz").is_empty());
    assert_eq!(search("").len(), TOOLS.len());
}

#[test]
fn bad_settings_and_missing_picks_are_explained() {
    let mut p = cube1();
    let mut s = Selection::new(&p, Mode::Face);
    let ctx = Ctx::for_poly(&p);
    let e = run(&mut p, &mut s, "inset", &args(json!({})), &ctx).unwrap_err();
    assert!(e.contains("pick some faces"), "{e}");
    let e = run(&mut p, &mut s, "bevel", &args(json!({"wdith": 3})), &ctx).unwrap_err();
    assert!(e.contains("no setting \"wdith\"") && e.contains("width"), "{e}");
    let e = run(&mut p, &mut s, "nope", &args(json!({})), &ctx).unwrap_err();
    assert!(e.contains("no operation"), "{e}");
    s.with_faces(&p, &[0]);
    let e = run(&mut p, &mut s, "merge", &args(json!({"at": "middle"})), &ctx).unwrap_err();
    assert!(e.contains("one of"), "{e}");
    assert_eq!(p, cube1(), "a refused operation changes nothing");
}

#[test]
fn defaults_scale_with_the_model() {
    let small = Ctx::for_poly(&prim::cube([10.0; 3], 1));
    let big = Ctx::for_poly(&prim::cube([1000.0; 3], 1));
    let t = find("bevel").unwrap();
    let ws = value(t, &Map::new(), "width", &small).as_f64().unwrap();
    let wb = value(t, &Map::new(), "width", &big).as_f64().unwrap();
    assert!((wb / ws - 100.0).abs() < 1e-3);
}
