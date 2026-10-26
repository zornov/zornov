// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Every edit-mode operation in one table: its name, menu, shortcut, what
//! it does, and its settings with their ranges. The same table builds the
//! menus, the operation search, the right-click menus, the panel that
//! adjusts the last operation, tooltips, and what the AI can call; and
//! [`run`] carries any of them out on a mesh and its [`Selection`].
//!
//! Distances are in the model's own pixels; settings marked as lengths
//! default to a share of the model's size, so a default bevel suits a
//! bolt and a building alike.

use crate::edit::{self, Falloff, Plane};
use crate::ops;
use crate::poly::{Edge, Poly, Topology, ekey};
use crate::selection::{Combine, Elem, Mode, Selection, Similar};
use serde_json::{Map, Value};
use std::collections::HashSet;
use studio_mesh::{V3, add, length, normalise, scale, sub};

/// Which menu an operation lives in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Menu {
    Select,
    Add,
    Mesh,
    Vertex,
    Edge,
    Face,
    Normals,
    Clean,
    Uv,
}

impl Menu {
    pub const ALL: [Menu; 9] = [Menu::Select, Menu::Add, Menu::Mesh, Menu::Vertex, Menu::Edge, Menu::Face, Menu::Normals, Menu::Clean, Menu::Uv];

    pub fn label(self) -> &'static str {
        match self {
            Menu::Select => "Select",
            Menu::Add => "Add",
            Menu::Mesh => "Mesh",
            Menu::Vertex => "Vertex",
            Menu::Edge => "Edge",
            Menu::Face => "Face",
            Menu::Normals => "Normals",
            Menu::Clean => "Clean up",
            Menu::Uv => "UV",
        }
    }
}

/// A setting's kind and range.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    /// Model pixels; the default is `default` × the model's size.
    Length { default: f32, min: f32, max: f32 },
    /// A plain number.
    Number { default: f32, min: f32, max: f32 },
    /// Degrees.
    Angle { default: f32, min: f32, max: f32 },
    /// Percent.
    Percent { default: f32 },
    Count { default: i64, min: i64, max: i64 },
    Toggle { default: bool },
    Choice { options: &'static [&'static str], default: usize },
    /// A direction or point (model space).
    Vector { default: V3 },
    /// An axis (x, y or z).
    Axis { default: usize },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Param {
    pub key: &'static str,
    pub label: &'static str,
    pub kind: Kind,
}

/// How the pointer carries on an operation after it runs (in the app; the
/// AI gives the numbers).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Follow {
    /// Nothing more: done.
    None,
    /// The picked geometry moves with the pointer (extrude, duplicate).
    Move,
    /// Moves along the picked faces' normal (extruding faces).
    MoveNormal,
    /// The named setting follows the pointer's distance from the pivot.
    Drag(&'static str),
}

/// One operation.
#[derive(Clone, Copy, Debug)]
pub struct Tool {
    pub id: &'static str,
    pub label: &'static str,
    pub menu: Menu,
    /// The shortcut as shown ("E", "Cmd+B", "Shift+D"; "" for none).
    pub keys: &'static str,
    pub tip: &'static str,
    pub params: &'static [Param],
    /// The select modes whose right-click menu lists it (none: menus only).
    pub context: &'static [Mode],
    pub follow: Follow,
}

impl Tool {
    pub fn param(&self, key: &str) -> Option<&Param> {
        self.params.iter().find(|p| p.key == key)
    }
}

const V: &[Mode] = &[Mode::Vertex];
const E: &[Mode] = &[Mode::Edge];
const F: &[Mode] = &[Mode::Face];
const VE: &[Mode] = &[Mode::Vertex, Mode::Edge];
const EF: &[Mode] = &[Mode::Edge, Mode::Face];
const ALL: &[Mode] = &[Mode::Vertex, Mode::Edge, Mode::Face];
const NONE: &[Mode] = &[];

const fn len(key: &'static str, label: &'static str, default: f32) -> Param {
    Param { key, label, kind: Kind::Length { default, min: 0.0, max: 1e6 } }
}
const fn slen(key: &'static str, label: &'static str, default: f32) -> Param {
    Param { key, label, kind: Kind::Length { default, min: -1e6, max: 1e6 } }
}
const fn count(key: &'static str, label: &'static str, default: i64, min: i64, max: i64) -> Param {
    Param { key, label, kind: Kind::Count { default, min, max } }
}
const fn num(key: &'static str, label: &'static str, default: f32, min: f32, max: f32) -> Param {
    Param { key, label, kind: Kind::Number { default, min, max } }
}
const fn angle(key: &'static str, label: &'static str, default: f32, min: f32, max: f32) -> Param {
    Param { key, label, kind: Kind::Angle { default, min, max } }
}
const fn toggle(key: &'static str, label: &'static str, default: bool) -> Param {
    Param { key, label, kind: Kind::Toggle { default } }
}
const fn choice(key: &'static str, label: &'static str, options: &'static [&'static str], default: usize) -> Param {
    Param { key, label, kind: Kind::Choice { options, default } }
}
const fn axis(key: &'static str, label: &'static str, default: usize) -> Param {
    Param { key, label, kind: Kind::Axis { default } }
}
const fn vector(key: &'static str, label: &'static str, default: V3) -> Param {
    Param { key, label, kind: Kind::Vector { default } }
}

const fn tool(id: &'static str, label: &'static str, menu: Menu, keys: &'static str, tip: &'static str, params: &'static [Param], context: &'static [Mode], follow: Follow) -> Tool {
    Tool { id, label, menu, keys, tip, params, context, follow }
}

const SIMILAR: &[&str] = &["normal", "area", "material", "sides", "coplanar", "smooth", "face_set", "length", "direction", "face_angle", "seam", "sharp", "crease", "edges"];
const MERGE_AT: &[&str] = &["centre", "cursor", "first", "last", "collapse", "distance"];
const DELETE: &[&str] = &["vertices", "edges", "faces", "edge_loops"];
const DISSOLVE: &[&str] = &["auto", "vertices", "edges", "faces"];
const COMPARE: &[&str] = &["equal", "more", "fewer"];
const SEPARATE: &[&str] = &["selection", "material", "loose_parts"];
const CLEAR: &[&str] = &["none", "front", "back"];
const UNWRAP: &[&str] = &["seams", "angle", "smart"];
const PROJECT: &[&str] = &["box", "planar", "front", "cylinder", "sphere", "view"];
const CAP: &[&str] = &["ngon", "fan", "quads", "none"];
const CIRCLE_FILL: &[&str] = &["ngon", "fan", "quads"];
const SIDE: &[&str] = &["positive_to_negative", "negative_to_positive"];

/// Every operation.
pub static TOOLS: &[Tool] = &[
    // ---------------------------------------------------------- select
    tool("select_all", "Select all", Menu::Select, "A", "Picks everything that is shown.", &[], NONE, Follow::None),
    tool("select_none", "Select none", Menu::Select, "Alt+A", "Clears the pick.", &[], NONE, Follow::None),
    tool("select_invert", "Invert selection", Menu::Select, "Cmd+I", "Picks what wasn't picked, and drops what was.", &[], NONE, Follow::None),
    tool("select_loop", "Select edge loop", Menu::Select, "Alt+Click", "The loop running through the active edge (in face mode, the loop of faces across it).", &[], ALL, Follow::None),
    tool("select_ring", "Select edge ring", Menu::Select, "Cmd+Alt+Click", "The ring of edges across the quads from the active edge.", &[], ALL, Follow::None),
    tool("select_linked", "Select linked", Menu::Select, "Cmd+L", "Everything joined to the pick.", &[toggle("seams", "Stop at seams", false)], ALL, Follow::None),
    tool("select_more", "Select more", Menu::Select, "Cmd+=", "Grows the pick by one ring of neighbours.", &[], NONE, Follow::None),
    tool("select_less", "Select less", Menu::Select, "Cmd+-", "Shrinks the pick by one ring.", &[], NONE, Follow::None),
    tool("select_similar", "Select similar", Menu::Select, "Shift+G", "Adds elements like the picked ones: faces by normal, area, material, sides, plane; edges by length, direction, angle, marks; vertices by normal or edge count.", &[choice("by", "Compare", SIMILAR, 0), num("threshold", "Threshold", 5.0, 0.0, 180.0)], ALL, Follow::None),
    tool("select_boundary", "Select boundary loop", Menu::Select, "", "The outline of the picked faces.", &[], F, Follow::None),
    tool("select_non_manifold", "Select non-manifold", Menu::Select, "Shift+Cmd+Alt+M", "Open edges, edges with more than two faces and pinched vertices: what keeps a model from being solid.", &[], VE, Follow::None),
    tool("select_sharp", "Select sharp edges", Menu::Select, "", "Edges where the faces meet at more than an angle.", &[angle("angle", "Sharper than", 30.0, 1.0, 180.0)], NONE, Follow::None),
    tool("select_random", "Select random", Menu::Select, "", "A random share of what is shown.", &[Param { key: "percent", label: "Share", kind: Kind::Percent { default: 50.0 } }, count("seed", "Seed", 1, 0, 100000)], NONE, Follow::None),
    tool("select_sides", "Select faces by sides", Menu::Select, "", "Faces with a number of corners (triangles, quads, n-gons).", &[count("sides", "Corners", 4, 3, 64), choice("compare", "Compare", COMPARE, 0)], NONE, Follow::None),
    tool("select_material", "Select by material", Menu::Select, "", "Faces of a material slot.", &[count("slot", "Slot", 0, 0, 255)], NONE, Follow::None),
    tool("select_mirror", "Select mirror", Menu::Select, "Shift+Cmd+M", "Picks the elements mirrored across an axis instead.", &[axis("axis", "Axis", 0), toggle("extend", "Keep the pick too", false)], NONE, Follow::None),
    tool("hide", "Hide", Menu::Select, "H", "Hides the pick.", &[], NONE, Follow::None),
    tool("hide_unselected", "Hide unselected", Menu::Select, "Shift+H", "Hides everything but the pick.", &[], NONE, Follow::None),
    tool("reveal", "Reveal hidden", Menu::Select, "Alt+H", "Shows what was hidden (it comes back picked).", &[], NONE, Follow::None),
    // ---------------------------------------------------------- add
    tool("add_cube", "Cube", Menu::Add, "", "A box of quads.", &[len("size", "Size", 0.25), count("segments", "Segments", 1, 1, 64)], NONE, Follow::None),
    tool("add_plane", "Plane", Menu::Add, "", "A flat square.", &[len("size", "Size", 0.25)], NONE, Follow::None),
    tool("add_grid", "Grid", Menu::Add, "", "A flat square of rows and columns of quads.", &[len("size", "Size", 0.25), count("x", "Columns", 10, 1, 512), count("y", "Rows", 10, 1, 512)], NONE, Follow::None),
    tool("add_circle", "Circle", Menu::Add, "", "A flat disc: one many-sided face, a fan of triangles, or quads round a smaller middle.", &[len("radius", "Radius", 0.125), count("vertices", "Vertices", 32, 3, 512), choice("fill", "Fill", CIRCLE_FILL, 0)], NONE, Follow::None),
    tool("add_uv_sphere", "UV sphere", Menu::Add, "", "A sphere of rings and segments.", &[len("radius", "Radius", 0.125), count("segments", "Segments", 32, 3, 256), count("rings", "Rings", 16, 2, 128)], NONE, Follow::None),
    tool("add_ico_sphere", "Ico sphere", Menu::Add, "", "A sphere of even triangles.", &[len("radius", "Radius", 0.125), count("subdivisions", "Subdivisions", 2, 1, 6)], NONE, Follow::None),
    tool("add_quad_sphere", "Quad sphere", Menu::Add, "", "A sphere of quads (a rounded cube): clean to subdivide.", &[len("radius", "Radius", 0.125), count("segments", "Segments", 4, 1, 64)], NONE, Follow::None),
    tool("add_cylinder", "Cylinder", Menu::Add, "", "A round post standing up.", &[len("radius", "Radius", 0.1), len("depth", "Depth", 0.25), count("vertices", "Vertices", 32, 3, 512), choice("cap", "Caps", CAP, 0)], NONE, Follow::None),
    tool("add_cone", "Cone", Menu::Add, "", "A cone or a truncated one.", &[len("radius1", "Base radius", 0.1), len("radius2", "Top radius", 0.0), len("depth", "Depth", 0.25), count("vertices", "Vertices", 32, 3, 512)], NONE, Follow::None),
    tool("add_torus", "Torus", Menu::Add, "", "A ring.", &[len("major", "Ring radius", 0.1), len("minor", "Tube radius", 0.03), count("segments", "Segments", 48, 3, 512), count("sides", "Sides", 12, 3, 256)], NONE, Follow::None),
    tool("add_head", "Head", Menu::Add, "", "A generic human head with eyes, to model or sculpt a character from.", &[len("size", "Size", 0.3), count("detail", "Detail", 2, 1, 4)], NONE, Follow::None),
    tool("add_curve", "Curve", Menu::Add, "", "A round tube along a curve (an S bend, an arc, a ring, a coil or a line).", &[choice("kind", "Shape", crate::prim::CURVES, 0), len("size", "Length", 0.3), len("radius", "Thickness", 0.01), count("resolution", "Resolution", 48, 4, 512)], NONE, Follow::None),
    // ---------------------------------------------------------- mesh
    tool("extrude", "Extrude", Menu::Mesh, "E", "Pulls the pick out: faces as one region along their normal, open edges into a new band.", &[slen("distance", "Distance", 0.0)], ALL, Follow::MoveNormal),
    tool("duplicate", "Duplicate", Menu::Mesh, "Shift+D", "Copies the picked faces; the copy follows the pointer.", &[], ALL, Follow::Move),
    tool("delete", "Delete", Menu::Mesh, "X", "Deletes vertices, edges, faces or edge loops (with the faces they leave without a corner).", &[choice("what", "Delete", DELETE, 0)], ALL, Follow::None),
    tool("dissolve", "Dissolve", Menu::Mesh, "Cmd+X", "Removes the pick but keeps the surface closed: faces round it join into one.", &[choice("what", "Dissolve", DISSOLVE, 0)], ALL, Follow::None),
    tool("merge", "Merge", Menu::Mesh, "M", "Joins picked vertices into one: at their middle, the 3D cursor, the first or last picked, each group at its own middle, or those closer than a distance.", &[choice("at", "At", MERGE_AT, 0), len("distance", "Distance", 0.001)], VE, Follow::None),
    tool("split", "Split", Menu::Mesh, "Y", "Disconnects the picked faces from the rest.", &[], F, Follow::None),
    tool("separate", "Separate", Menu::Mesh, "P", "Moves the picked faces (or each material, or each loose piece) into a new object.", &[choice("by", "By", SEPARATE, 0)], ALL, Follow::None),
    tool("mirror", "Mirror", Menu::Mesh, "Cmd+M", "Flips the pick across an axis through the pivot.", &[axis("axis", "Axis", 0)], NONE, Follow::None),
    tool("symmetrize", "Symmetrize", Menu::Mesh, "", "Makes the whole mesh symmetric: one half copied over the other.", &[axis("axis", "Axis", 0), choice("direction", "Direction", SIDE, 1)], NONE, Follow::None),
    tool("shrink_fatten", "Shrink/fatten", Menu::Mesh, "Alt+S", "Moves picked vertices along their normals.", &[slen("distance", "Distance", 0.0)], NONE, Follow::Drag("distance")),
    tool("to_sphere", "To sphere", Menu::Mesh, "Shift+Alt+S", "Pulls the pick toward a sphere round its middle.", &[Param { key: "factor", label: "Factor", kind: Kind::Number { default: 1.0, min: 0.0, max: 1.0 } }], NONE, Follow::Drag("factor")),
    tool("smooth_vertices", "Smooth vertices", Menu::Mesh, "", "Relaxes the picked vertices without shrinking the surface.", &[count("iterations", "Repeat", 1, 1, 100), num("factor", "Smoothing", 0.5, 0.0, 1.0)], V, Follow::None),
    tool("randomize", "Randomize", Menu::Mesh, "", "Moves picked vertices by a random amount.", &[len("amount", "Amount", 0.01), count("seed", "Seed", 0, 0, 100000)], NONE, Follow::None),
    tool("selection_to_cursor", "Selection to cursor", Menu::Mesh, "", "Moves the pick so its middle sits on the 3D cursor.", &[], NONE, Follow::None),
    tool("bisect", "Bisect", Menu::Mesh, "", "Cuts the picked faces (all when none are) along a plane, optionally clearing one side and closing the cut.", &[vector("point", "Point", [0.0; 3]), vector("normal", "Normal", [1.0, 0.0, 0.0]), choice("clear", "Clear", CLEAR, 0), toggle("fill", "Fill the cut", false)], NONE, Follow::None),
    tool("knife", "Knife", Menu::Mesh, "K", "Cuts new edges across the faces along a line you draw (a plane for the AI).", &[vector("point", "Point", [0.0; 3]), vector("normal", "Normal", [1.0, 0.0, 0.0])], ALL, Follow::None),
    tool("spin", "Spin", Menu::Mesh, "", "Turns the picked edges round an axis through the 3D cursor into a surface (a vase from a profile).", &[angle("degrees", "Angle", 360.0, -3600.0, 3600.0), count("steps", "Steps", 12, 3, 512), axis("axis", "Axis", 1)], VE, Follow::None),
    tool("screw", "Screw", Menu::Mesh, "", "Turns the picked edges round an axis through the 3D cursor while they climb: threads, springs, spiral stairs.", &[num("turns", "Turns", 2.0, -100.0, 100.0), count("steps", "Steps per turn", 16, 3, 256), slen("height", "Rise per turn", 0.1), axis("axis", "Axis", 1)], VE, Follow::None),
    tool("subdivide", "Subdivide", Menu::Mesh, "", "Splits the picked edges (the edges of picked faces) into pieces; faces split all round become grids.", &[count("cuts", "Cuts", 1, 1, 64), num("smooth", "Smoothness", 0.0, 0.0, 1.0)], EF, Follow::None),
    tool("triangulate", "Triangulate faces", Menu::Mesh, "Cmd+T", "Splits picked quads and n-gons into triangles.", &[], F, Follow::None),
    tool("tris_to_quads", "Triangles to quads", Menu::Mesh, "Alt+J", "Joins pairs of picked triangles that lie flat together into quads.", &[angle("angle", "Max angle", 40.0, 0.0, 180.0)], F, Follow::None),
    // ---------------------------------------------------------- vertex
    tool("bevel_vertices", "Bevel vertices", Menu::Vertex, "Shift+Cmd+B", "Cuts picked corners off (rounded with more segments).", &[len("width", "Width", 0.02), count("segments", "Segments", 1, 1, 32), num("profile", "Profile", 0.5, 0.0, 1.0)], V, Follow::Drag("width")),
    tool("connect", "Connect vertex path", Menu::Vertex, "J", "Splits faces between picked corners.", &[], V, Follow::None),
    tool("make_face", "New edge/face from vertices", Menu::Vertex, "F", "A face through the picked vertices, or filling the hole whose rim is picked.", &[], VE, Follow::None),
    tool("vertex_slide", "Slide vertices", Menu::Vertex, "Shift+V", "Slides picked vertices along an edge of theirs.", &[Param { key: "factor", label: "Factor", kind: Kind::Number { default: 0.5, min: 0.0, max: 1.0 } }, vector("toward", "Toward", [1.0, 0.0, 0.0])], V, Follow::Drag("factor")),
    // ---------------------------------------------------------- edge
    tool("bevel", "Bevel edges", Menu::Edge, "Cmd+B", "Rounds or chamfers the picked edges.", &[len("width", "Width", 0.02), count("segments", "Segments", 1, 1, 32), num("profile", "Profile", 0.5, 0.0, 1.0)], EF, Follow::Drag("width")),
    tool("loop_cut", "Loop cut and slide", Menu::Edge, "Cmd+R", "Cuts loops across the ring of quads through the active edge.", &[count("cuts", "Cuts", 1, 1, 64), num("slide", "Slide", 0.0, -1.0, 1.0)], ALL, Follow::Drag("slide")),
    tool("edge_slide", "Slide edges", Menu::Edge, "G G", "Slides picked edge loops along the edges beside them.", &[num("factor", "Factor", 0.0, -1.0, 1.0)], E, Follow::Drag("factor")),
    tool("bridge", "Bridge edge loops", Menu::Edge, "", "Joins two picked loops of open edges (or two picked face groups) with a band of faces.", &[count("segments", "Segments", 1, 1, 64), count("twist", "Twist", 0, -64, 64)], EF, Follow::None),
    tool("fill", "Fill", Menu::Edge, "Alt+F", "Closes holes whose rim is picked with one face each.", &[], E, Follow::None),
    tool("grid_fill", "Grid fill", Menu::Edge, "", "Closes a picked hole with a grid of quads (an even number of edges round it).", &[], E, Follow::None),
    tool("mark_seam", "Mark seam", Menu::Edge, "", "Marks picked edges where the texture layout is cut.", &[toggle("clear", "Clear instead", false)], E, Follow::None),
    tool("mark_sharp", "Mark sharp", Menu::Edge, "", "Marks picked edges to shade hard.", &[toggle("clear", "Clear instead", false)], E, Follow::None),
    tool("crease", "Edge crease", Menu::Edge, "Shift+E", "How sharp picked edges stay under subdivision (0 smooth, 1 sharp).", &[num("amount", "Crease", 1.0, 0.0, 1.0)], E, Follow::Drag("amount")),
    tool("mark_sharp_by_angle", "Mark sharp by angle", Menu::Edge, "", "Marks every edge sharper than an angle as sharp (and creased, and as a seam, if asked).", &[angle("angle", "Sharper than", 30.0, 1.0, 180.0), toggle("crease", "Crease too", false), toggle("seam", "Seam too", false)], NONE, Follow::None),
    // ---------------------------------------------------------- face
    tool("extrude_region", "Extrude faces", Menu::Face, "", "Pulls the picked faces out as one region along their average normal.", &[slen("distance", "Distance", 0.0)], F, Follow::Drag("distance")),
    tool("extrude_normals", "Extrude faces along normals", Menu::Face, "", "Pulls the picked faces out, each corner along its own normal (an even shell).", &[slen("distance", "Distance", 0.0)], F, Follow::Drag("distance")),
    tool("extrude_individual", "Extrude individual faces", Menu::Face, "Alt+E", "Pulls each picked face out on its own.", &[slen("distance", "Distance", 0.0)], F, Follow::Drag("distance")),
    tool("inset", "Inset faces", Menu::Face, "I", "A new rim inside the picked faces' outline (each face on its own, if asked).", &[len("thickness", "Thickness", 0.0), slen("depth", "Depth", 0.0), toggle("individual", "Each face", false)], F, Follow::Drag("thickness")),
    tool("poke", "Poke faces", Menu::Face, "", "A point in each picked face's middle, the face fanned to it.", &[slen("offset", "Offset", 0.0)], F, Follow::None),
    tool("solidify", "Solidify faces", Menu::Face, "", "Gives the picked faces thickness.", &[slen("thickness", "Thickness", 0.02)], F, Follow::Drag("thickness")),
    tool("assign_material", "Assign material", Menu::Face, "", "Gives the picked faces a material slot.", &[count("slot", "Slot", 0, 0, 255)], F, Follow::None),
    tool("shade_smooth", "Shade smooth", Menu::Face, "", "Picked faces shade smooth (all faces when none are picked).", &[], F, Follow::None),
    tool("shade_flat", "Shade flat", Menu::Face, "", "Picked faces show their facets (all faces when none are picked).", &[], F, Follow::None),
    // ---------------------------------------------------------- normals
    tool("recalc_outside", "Recalculate outside", Menu::Normals, "Shift+N", "Turns the picked faces to point outward, consistently.", &[], F, Follow::None),
    tool("recalc_inside", "Recalculate inside", Menu::Normals, "Shift+Cmd+N", "Turns the picked faces to point inward.", &[], NONE, Follow::None),
    tool("flip_normals", "Flip", Menu::Normals, "", "Turns the picked faces round.", &[], F, Follow::None),
    // ---------------------------------------------------------- clean up
    tool("merge_by_distance", "Merge by distance", Menu::Clean, "", "Welds vertices closer than a distance (picked ones, or all when none are).", &[len("distance", "Distance", 0.001)], NONE, Follow::None),
    tool("fill_holes", "Fill holes", Menu::Clean, "", "Closes every hole of up to a number of sides.", &[count("sides", "Sides", 4, 3, 100000)], NONE, Follow::None),
    tool("limited_dissolve", "Limited dissolve", Menu::Clean, "", "Joins faces that lie flat together into bigger ones.", &[angle("angle", "Max angle", 5.0, 0.0, 180.0)], NONE, Follow::None),
    tool("fix_t_junctions", "Stitch cracks", Menu::Clean, "", "Joins open edges that a vertex sits on (T-junctions).", &[len("distance", "Tolerance", 0.001)], NONE, Follow::None),
    tool("decimate", "Decimate", Menu::Clean, "", "Fewer faces: a share of what there is.", &[num("ratio", "Ratio", 0.5, 0.01, 1.0)], NONE, Follow::None),
    // ---------------------------------------------------------- uv
    tool("unwrap", "Unwrap", Menu::Uv, "U", "Flattens the surface into a texture layout: cut at marked seams, at sharp angles, or smart pieces.", &[choice("method", "Method", UNWRAP, 0), angle("angle", "Angle", 66.0, 1.0, 180.0), num("margin", "Margin", 0.006, 0.0, 0.2)], NONE, Follow::None),
    tool("project_uvs", "Project", Menu::Uv, "", "Lays the texture on by a simple projection.", &[choice("kind", "Projection", PROJECT, 0), len("size", "Texture size", 1.0)], NONE, Follow::None),
    tool("pack_uvs", "Pack islands", Menu::Uv, "", "Packs the layout's pieces into the square without overlap.", &[num("margin", "Margin", 0.01, 0.0, 0.2)], NONE, Follow::None),
    tool("reset_uvs", "Reset", Menu::Uv, "", "Gives each picked face the whole square.", &[], NONE, Follow::None),
    tool("clear_seams", "Clear seams", Menu::Uv, "", "Removes every seam mark.", &[], NONE, Follow::None),
];

pub fn find(id: &str) -> Option<&'static Tool> {
    TOOLS.iter().find(|t| t.id == id)
}

/// Operations whose words match a search ("bev" finds the bevels).
pub fn search(query: &str) -> Vec<&'static Tool> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return TOOLS.iter().collect();
    }
    let words: Vec<&str> = q.split_whitespace().collect();
    let mut hits: Vec<(usize, &Tool)> = TOOLS
        .iter()
        .filter_map(|t| {
            let hay = format!("{} {} {} {}", t.label.to_lowercase(), t.id.replace('_', " "), t.menu.label().to_lowercase(), t.tip.to_lowercase());
            if !words.iter().all(|w| hay.contains(w)) {
                return None;
            }
            // Label starts best, then words of the label, then the tip.
            let l = t.label.to_lowercase();
            let rank = if l.starts_with(&q) { 0 } else if l.split_whitespace().any(|w| w.starts_with(words[0])) { 1 } else if l.contains(&q) { 2 } else { 3 };
            Some((rank, t))
        })
        .collect();
    hits.sort_by_key(|(r, t)| (*r, t.label));
    hits.into_iter().map(|(_, t)| t).collect()
}

/// Where an operation happens: the 3D cursor and the eye's direction (in
/// the model's own space), and the model's size for default lengths.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ctx {
    pub cursor: V3,
    /// From the eye into the scene.
    pub view: V3,
    pub size: f32,
    /// The pivot the app turns and mirrors about (else the pick's middle).
    pub pivot: Option<V3>,
}

impl Default for Ctx {
    fn default() -> Ctx {
        Ctx { cursor: [0.0; 3], view: [0.0, 0.0, 1.0], size: 100.0, pivot: None }
    }
}

impl Ctx {
    /// A context for a mesh: its size from its bounds.
    pub fn for_poly(p: &Poly) -> Ctx {
        let (lo, hi) = p.bounds();
        let size = length(sub(hi, lo));
        Ctx { size: if size.is_finite() && size > 0.0 { size } else { 100.0 }, ..Ctx::default() }
    }
}

/// What an operation did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Done {
    pub said: String,
    /// Pieces taken out into objects of their own (separate).
    pub pieces: Vec<Poly>,
    /// The mesh changed shape or topology (otherwise only the pick did).
    pub changed: bool,
}

/// A setting's value from `args`, else its default.
pub fn value(t: &Tool, args: &Map<String, Value>, key: &str, ctx: &Ctx) -> Value {
    let Some(p) = t.param(key) else { return args.get(key).cloned().unwrap_or(Value::Null) };
    if let Some(v) = args.get(key) {
        return v.clone();
    }
    default_value(p, ctx)
}

/// A setting's default.
pub fn default_value(p: &Param, ctx: &Ctx) -> Value {
    match p.kind {
        Kind::Length { default, .. } => Value::from((default * ctx.size) as f64),
        Kind::Number { default, .. } | Kind::Angle { default, .. } | Kind::Percent { default } => Value::from(default as f64),
        Kind::Count { default, .. } => Value::from(default),
        Kind::Toggle { default } => Value::from(default),
        Kind::Choice { options, default } => Value::from(options[default]),
        Kind::Vector { default } => Value::from(default.iter().map(|x| *x as f64).collect::<Vec<_>>()),
        Kind::Axis { default } => Value::from(["x", "y", "z"][default]),
    }
}

/// Every setting at its default.
pub fn defaults(t: &Tool, ctx: &Ctx) -> Map<String, Value> {
    t.params.iter().map(|p| (p.key.to_string(), default_value(p, ctx))).collect()
}

struct Args<'a> {
    t: &'a Tool,
    a: &'a Map<String, Value>,
    ctx: &'a Ctx,
}

impl Args<'_> {
    fn f(&self, k: &str) -> Result<f32, String> {
        let v = value(self.t, self.a, k, self.ctx);
        let x = v.as_f64().ok_or_else(|| format!("\"{k}\" is a number"))? as f32;
        if !x.is_finite() {
            return Err(format!("\"{k}\" is a number"));
        }
        Ok(match self.t.param(k).map(|p| p.kind) {
            Some(Kind::Length { min, max, .. } | Kind::Number { min, max, .. } | Kind::Angle { min, max, .. }) => x.clamp(min, max),
            Some(Kind::Percent { .. }) => x.clamp(0.0, 100.0),
            _ => x,
        })
    }
    fn n(&self, k: &str) -> Result<i64, String> {
        let v = value(self.t, self.a, k, self.ctx);
        let x = v.as_f64().ok_or_else(|| format!("\"{k}\" is a whole number"))?.round() as i64;
        Ok(match self.t.param(k).map(|p| p.kind) {
            Some(Kind::Count { min, max, .. }) => x.clamp(min, max),
            _ => x,
        })
    }
    fn b(&self, k: &str) -> bool {
        value(self.t, self.a, k, self.ctx).as_bool().unwrap_or(false)
    }
    fn choice(&self, k: &str) -> Result<&'static str, String> {
        let Some(Param { kind: Kind::Choice { options, .. }, .. }) = self.t.param(k) else { return Err(format!("no choice \"{k}\"")) };
        let v = value(self.t, self.a, k, self.ctx);
        match &v {
            Value::String(s) => options.iter().copied().find(|o| *o == s.as_str()).ok_or_else(|| format!("\"{k}\" is one of {}", options.join(", "))),
            Value::Number(n) => n.as_u64().and_then(|i| options.get(i as usize).copied()).ok_or_else(|| format!("\"{k}\" is one of {}", options.join(", "))),
            _ => Err(format!("\"{k}\" is one of {}", options.join(", "))),
        }
    }
    fn axis(&self, k: &str) -> Result<usize, String> {
        let v = value(self.t, self.a, k, self.ctx);
        match &v {
            Value::String(s) => match s.to_lowercase().as_str() {
                "x" => Ok(0),
                "y" => Ok(1),
                "z" => Ok(2),
                _ => Err(format!("\"{k}\" is x, y or z")),
            },
            Value::Number(n) => n.as_u64().filter(|i| *i < 3).map(|i| i as usize).ok_or_else(|| format!("\"{k}\" is x, y or z")),
            _ => Err(format!("\"{k}\" is x, y or z")),
        }
    }
    fn v3(&self, k: &str) -> Result<V3, String> {
        let v = value(self.t, self.a, k, self.ctx);
        let a = v.as_array().filter(|a| a.len() == 3).ok_or_else(|| format!("\"{k}\" is [x, y, z]"))?;
        let mut out = [0.0; 3];
        for i in 0..3 {
            out[i] = a[i].as_f64().ok_or_else(|| format!("\"{k}\" is [x, y, z]"))? as f32;
        }
        Ok(out)
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// The picked faces, or all of them when none are (for tools that work on
/// the whole mesh without a pick).
fn faces_or_all(p: &Poly, s: &Selection) -> Vec<bool> {
    let f = s.face_pick();
    if f.iter().any(|b| *b) { f } else { s.visible(p).faces }
}

fn verts_or_all(p: &Poly, s: &Selection) -> Vec<bool> {
    if s.verts.iter().any(|b| *b) { s.verts.clone() } else { s.visible(p).verts }
}

fn need_faces(s: &Selection) -> Result<Vec<bool>, String> {
    let f = s.face_pick();
    if !f.iter().any(|b| *b) {
        return Err("pick some faces first".into());
    }
    Ok(f)
}

fn need_edges(s: &Selection) -> Result<Vec<Edge>, String> {
    let e = s.edge_list();
    if e.is_empty() {
        return Err("pick some edges first".into());
    }
    Ok(e)
}

fn need_verts(s: &Selection) -> Result<Vec<bool>, String> {
    if !s.verts.iter().any(|b| *b) {
        return Err("pick some vertices first".into());
    }
    Ok(s.verts.clone())
}

/// A starting shape by an `add_…` operation's settings (centred on the
/// origin).
pub fn primitive(id: &str, args: &Map<String, Value>, ctx: &Ctx) -> Result<Poly, String> {
    let t = find(id).filter(|t| t.menu == Menu::Add).ok_or_else(|| format!("no shape \"{id}\""))?;
    let a = Args { t, a: args, ctx };
    use crate::prim;
    let pos = |x: f32| x.max(1e-3);
    Ok(match id {
        "add_cube" => {
            let s = pos(a.f("size")?);
            prim::cube([s; 3], a.n("segments")? as usize)
        }
        "add_plane" => {
            let s = pos(a.f("size")?);
            prim::plane(s, s, 1, 1)
        }
        "add_grid" => {
            let s = pos(a.f("size")?);
            prim::plane(s, s, a.n("x")? as usize, a.n("y")? as usize)
        }
        "add_circle" => {
            let (r, n) = (pos(a.f("radius")?), a.n("vertices")? as usize);
            match a.choice("fill")? {
                "fan" => {
                    let mut c = prim::circle(r, n);
                    edit::poke(&mut c, &[true], 0.0);
                    c
                }
                "quads" => {
                    let mut c = prim::cylinder(r, r, 0.0, n, 1, prim::Cap::Quads);
                    edit::merge_by_distance(&mut c, r * 1e-4);
                    let keep: Vec<bool> = (0..c.faces()).map(|f| c.face_normal(f)[1] < -0.5).collect();
                    c.retain_faces(|f| keep[f]);
                    c.compact();
                    c
                }
                _ => prim::circle(r, n),
            }
        }
        "add_uv_sphere" => prim::uv_sphere(pos(a.f("radius")?), a.n("segments")? as usize, a.n("rings")? as usize),
        "add_ico_sphere" => prim::ico_sphere(pos(a.f("radius")?), a.n("subdivisions")? as usize),
        "add_quad_sphere" => prim::quad_sphere(pos(a.f("radius")?), a.n("segments")? as usize),
        "add_cylinder" => {
            let r = pos(a.f("radius")?);
            let cap = match a.choice("cap")? {
                "fan" => prim::Cap::Fan,
                "quads" => prim::Cap::Quads,
                "none" => prim::Cap::None,
                _ => prim::Cap::Ngon,
            };
            prim::cylinder(r, r, pos(a.f("depth")?), a.n("vertices")? as usize, 1, cap)
        }
        "add_cone" => {
            let (r1, r2, d, n) = (pos(a.f("radius1")?), a.f("radius2")?.max(0.0), pos(a.f("depth")?), a.n("vertices")? as usize);
            if r2 <= 1e-4 { prim::cone(r1, d, n) } else { prim::cylinder(r2, r1, d, n, 1, prim::Cap::Ngon) }
        }
        "add_torus" => prim::torus(pos(a.f("major")?), pos(a.f("minor")?), a.n("segments")? as usize, a.n("sides")? as usize),
        "add_head" => prim::head(pos(a.f("size")?), a.n("detail")? as usize),
        "add_curve" => prim::curve(a.choice("kind")?, pos(a.f("size")?), pos(a.f("radius")?), a.n("resolution")? as usize).ok_or("no such curve")?,
        _ => return Err(format!("no shape \"{id}\"")),
    })
}

/// Runs operation `id` on `p` with the pick `s`. The pick follows what
/// the operation made (new faces after an extrude, the cut loops after a
/// loop cut). `args` may leave out any setting (its default is used).
pub fn run(p: &mut Poly, s: &mut Selection, id: &str, args: &Map<String, Value>, ctx: &Ctx) -> Result<Done, String> {
    let t = find(id).ok_or_else(|| format!("no operation \"{id}\"; operations: {}", TOOLS.iter().map(|t| t.id).collect::<Vec<_>>().join(", ")))?;
    for k in args.keys() {
        if t.param(k).is_none() && !matches!(k.as_str(), "edge" | "do" | "select" | "op" | "clip" | "part") {
            return Err(format!("{}: no setting \"{k}\" (it takes {})", t.id, if t.params.is_empty() { "none".to_string() } else { t.params.iter().map(|p| p.key).collect::<Vec<_>>().join(", ") }));
        }
    }
    s.sync(p);
    let a = Args { t, a: args, ctx };
    let mut done = Done { changed: true, ..Done::default() };
    let pivot = ctx.pivot.or_else(|| s.median(p)).unwrap_or(ctx.cursor);
    done.said = match id {
        // ------------------------------------------------------ select
        "select_all" | "select_none" | "select_invert" | "select_more" | "select_less" | "hide" | "hide_unselected" | "reveal" => {
            done.changed = false;
            match id {
                "select_all" => s.all(p),
                "select_none" => s.clear(),
                "select_invert" => s.invert(p),
                "select_more" => s.grow(p),
                "select_less" => s.shrink(p),
                "hide" => s.hide(p, false),
                "hide_unselected" => s.hide(p, true),
                _ => s.reveal(p),
            }
            picked_said(s)
        }
        "select_loop" | "select_ring" => {
            done.changed = false;
            let e = active_edge(p, s, args)?;
            let how = if args.get("extend").and_then(Value::as_bool).unwrap_or(false) { Combine::Add } else { Combine::Set };
            if id == "select_loop" { s.pick_loop(p, e, how) } else { s.pick_ring(p, e, how) }
            picked_said(s)
        }
        "select_linked" => {
            done.changed = false;
            if s.is_empty() {
                return Err("pick something to grow from".into());
            }
            s.linked(p, a.b("seams"));
            picked_said(s)
        }
        "select_similar" => {
            done.changed = false;
            let by = Similar::parse(a.choice("by")?).ok_or("unknown comparison")?;
            let n = s.similar(p, by, a.f("threshold")?)?;
            format!("{} more; {}", n, picked_said(s))
        }
        "select_boundary" => {
            done.changed = false;
            let faces = need_faces(s)?;
            let t = Topology::of(p);
            let rim: Vec<Edge> = t.edge_faces.iter().filter(|(_, fs)| {
                let n = fs.iter().filter(|&&f| faces[f as usize]).count();
                n >= 1 && (n < fs.len() || fs.len() == 1)
            }).map(|(e, _)| *e).collect();
            s.mode = Mode::Edge;
            s.with_edges(p, &rim);
            picked_said(s)
        }
        "select_non_manifold" => {
            done.changed = false;
            let (edges, verts) = crate::selection::non_manifold(p);
            if s.mode == Mode::Face {
                s.mode = Mode::Edge;
            }
            let mut all: Vec<u32> = edges.iter().flat_map(|e| [e.0, e.1]).collect();
            all.extend(verts.iter().copied());
            if s.mode == Mode::Edge {
                s.with_edges(p, &edges);
            } else {
                s.with_vertices(p, &all);
            }
            format!("{} and {}", plural(edges.len(), "open or shared edge", "open or shared edges"), plural(verts.len(), "pinched vertex", "pinched vertices"))
        }
        "select_sharp" => {
            done.changed = false;
            let edges = crate::select::Edges::Sharper { degrees: a.f("angle")? }.list(p);
            if s.mode == Mode::Face {
                s.mode = Mode::Edge;
            }
            s.with_edges(p, &edges);
            picked_said(s)
        }
        "select_random" => {
            done.changed = false;
            let share = a.f("percent")? / 100.0;
            let mut state = (a.n("seed")? as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
            let mut coin = move || {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                ((state >> 11) as f64 / (1u64 << 53) as f64) < share as f64
            };
            let vis = s.visible(p);
            match s.mode {
                Mode::Face => {
                    let list: Vec<u32> = (0..p.faces() as u32).filter(|&f| vis.faces[f as usize] && coin()).collect();
                    s.with_faces(p, &list);
                }
                Mode::Edge => {
                    let list: Vec<Edge> = p.edges().into_iter().filter(|e| vis.edge(*e) && coin()).collect();
                    s.with_edges(p, &list);
                }
                Mode::Vertex => {
                    let list: Vec<u32> = (0..p.positions.len() as u32).filter(|&v| vis.verts[v as usize] && coin()).collect();
                    s.with_vertices(p, &list);
                }
            }
            picked_said(s)
        }
        "select_sides" => {
            done.changed = false;
            let n = a.n("sides")? as usize;
            let cmp = a.choice("compare")?;
            let vis = s.visible(p);
            let list: Vec<u32> = (0..p.faces())
                .filter(|&f| vis.faces[f])
                .filter(|&f| {
                    let k = p.face(f).len();
                    match cmp {
                        "more" => k > n,
                        "fewer" => k < n,
                        _ => k == n,
                    }
                })
                .map(|f| f as u32)
                .collect();
            s.mode = Mode::Face;
            s.with_faces(p, &list);
            picked_said(s)
        }
        "select_material" => {
            done.changed = false;
            let slot = a.n("slot")? as u32;
            let list: Vec<u32> = (0..p.faces() as u32).filter(|&f| p.face_material[f as usize] == slot).collect();
            s.mode = Mode::Face;
            s.with_faces(p, &list);
            picked_said(s)
        }
        "select_mirror" => {
            done.changed = false;
            let ax = a.axis("axis")?;
            let (lo, hi) = p.bounds();
            let map = crate::selection::mirror_map(p, ax, length(sub(hi, lo)).max(1.0) * 1e-4);
            let mut vs: Vec<u32> = s.vert_list().iter().filter_map(|&v| map[v as usize]).collect();
            if a.b("extend") {
                vs.extend(s.vert_list());
            }
            let mode = s.mode;
            let faces: Vec<u32> = if mode == Mode::Face {
                let set: HashSet<u32> = vs.iter().copied().collect();
                (0..p.faces() as u32).filter(|&f| p.face(f as usize).iter().all(|v| set.contains(v)) && (a.b("extend") || !s.faces[f as usize] || p.face(f as usize).iter().all(|v| map[*v as usize] == Some(*v)))).collect()
            } else {
                Vec::new()
            };
            if mode == Mode::Face {
                s.with_faces(p, &faces);
            } else {
                s.mode = Mode::Vertex;
                s.with_vertices(p, &vs);
                s.mode = mode;
                s.sync(p);
            }
            picked_said(s)
        }
        // ------------------------------------------------------ add
        _ if t.menu == Menu::Add => {
            let mut shape = primitive(id, args, ctx)?;
            shape.map_positions(|q| add(q, ctx.cursor));
            let before = p.faces();
            merge_shape(p, &shape);
            s.fit(p);
            let made: Vec<u32> = (before as u32..p.faces() as u32).collect();
            let mode = s.mode;
            s.mode = Mode::Face;
            s.with_faces(p, &made);
            s.mode = mode;
            s.sync(p);
            format!("added a {} ({})", t.label.to_lowercase(), plural(made.len(), "face", "faces"))
        }
        // ------------------------------------------------------ mesh
        "extrude" => {
            let d = a.f("distance")?;
            if s.faces.iter().any(|b| *b) {
                let pick = s.face_pick();
                let dir = region_normal(p, &pick);
                let caps = edit::extrude(p, &pick, d, Some(dir), false);
                select_faces_after(p, s, &caps);
                format!("extruded {} {d:.1} px", plural(caps.len(), "face", "faces"))
            } else {
                let t2 = Topology::of(p);
                let open: Vec<Edge> = s.edge_list().into_iter().filter(|e| t2.is_boundary(*e)).collect();
                if open.is_empty() {
                    return Err("extrude needs picked faces, or picked open edges (on a rim)".into());
                }
                let dir = open_edges_out(p, &open);
                let made = edit::extrude_edges(p, &open, scale(dir, d));
                s.fit(p);
                let mode = s.mode;
                s.mode = Mode::Edge;
                s.with_edges(p, &made);
                s.mode = mode;
                s.sync(p);
                format!("extruded {} into a band", plural(open.len(), "edge", "edges"))
            }
        }
        "extrude_region" | "extrude_normals" | "extrude_individual" => {
            let pick = need_faces(s)?;
            let d = a.f("distance")?;
            let caps = match id {
                "extrude_region" => {
                    let dir = region_normal(p, &pick);
                    edit::extrude(p, &pick, d, Some(dir), false)
                }
                "extrude_normals" => edit::extrude(p, &pick, d, None, false),
                _ => individual_extrude(p, &pick, d),
            };
            select_faces_after(p, s, &caps);
            format!("extruded {} {d:.1} px", plural(caps.len(), "face", "faces"))
        }
        "duplicate" => {
            let pick = need_faces(s).map_err(|_| "duplicate copies faces: pick some (or the vertices round them)".to_string())?;
            let made = ops::duplicate(p, &pick);
            select_faces_after(p, s, &made);
            format!("copied {}", plural(made.len(), "face", "faces"))
        }
        "delete" => {
            let what = match a.choice("what")? {
                w if args.contains_key("what") => w,
                _ => match s.mode {
                    Mode::Vertex => "vertices",
                    Mode::Edge => "edges",
                    Mode::Face => "faces",
                },
            };
            let n = match what {
                "vertices" => ops::delete_vertices(p, &need_verts(s)?),
                "edges" => {
                    let e: HashSet<Edge> = need_edges(s)?.into_iter().collect();
                    ops::delete_edges(p, &e)
                }
                "edge_loops" => {
                    let e = need_edges(s)?;
                    ops::dissolve_edges_clean(p, &e)
                }
                _ => edit::delete_faces(p, &need_faces(s)?),
            };
            s.fit(p);
            s.clear();
            format!("deleted {}", plural(n, "face", "faces"))
        }
        "dissolve" => {
            let what = match a.choice("what")? {
                "auto" => match s.mode {
                    Mode::Vertex => "vertices",
                    Mode::Edge => "edges",
                    Mode::Face => "faces",
                },
                w => w,
            };
            let n = match what {
                "vertices" => ops::dissolve_vertices(p, &need_verts(s)?),
                "edges" => ops::dissolve_edges_clean(p, &need_edges(s)?),
                _ => ops::dissolve_faces(p, &need_faces(s)?),
            };
            s.fit(p);
            s.clear();
            format!("dissolved: {} fewer", plural(n, "face", "faces"))
        }
        "merge" => {
            let verts = need_verts(s)?;
            let list = s.vert_list();
            let at = a.choice("at")?;
            match at {
                "distance" => {
                    let n = ops::merge_by_distance_in(p, &verts, a.f("distance")?);
                    s.fit(p);
                    s.clear();
                    plural(n, "vertex merged", "vertices merged")
                }
                "collapse" => {
                    let groups: Vec<(Vec<u32>, V3)> = ops::islands(p, &verts).into_iter().map(|g| {
                        let c = ops::centre_of(p, &g);
                        (g, c)
                    }).collect();
                    let n = groups.len();
                    let kept = ops::merge_groups(p, &groups);
                    s.fit(p);
                    s.with_vertices(p, &kept);
                    format!("collapsed into {}", plural(n, "vertex", "vertices"))
                }
                _ => {
                    if list.len() < 2 {
                        return Err("pick two or more vertices to merge".into());
                    }
                    let point = match at {
                        "cursor" => ctx.cursor,
                        "first" => p.positions[*s.order.first().unwrap_or(&list[0]) as usize],
                        "last" => p.positions[*s.order.last().unwrap_or(&list[list.len() - 1]) as usize],
                        _ => ops::centre_of(p, &list),
                    };
                    let kept = ops::merge_groups(p, &[(list.clone(), point)]);
                    s.fit(p);
                    s.with_vertices(p, &kept);
                    format!("merged {} at the {at}", plural(list.len(), "vertex", "vertices"))
                }
            }
        }
        "split" => {
            let pick = need_faces(s)?;
            let n = ops::split(p, &pick);
            s.fit(p);
            let faces = s.face_list();
            s.with_faces(p, &faces);
            format!("split off ({} vertices doubled)", n)
        }
        "separate" => {
            let by = a.choice("by")?;
            let groups: Vec<Vec<bool>> = match by {
                "material" => {
                    let mats: Vec<u32> = {
                        let mut m: Vec<u32> = p.face_material.clone();
                        m.sort_unstable();
                        m.dedup();
                        m
                    };
                    if mats.len() < 2 {
                        return Err("every face has the same material; nothing to separate".into());
                    }
                    mats[1..].iter().map(|m| p.face_material.iter().map(|x| x == m).collect()).collect()
                }
                "loose_parts" => {
                    let parts = ops::loose_parts(p);
                    if parts.len() < 2 {
                        return Err("the mesh is one piece".into());
                    }
                    parts[1..].to_vec()
                }
                _ => {
                    let pick = need_faces(s)?;
                    if pick.iter().all(|b| *b) {
                        return Err("everything is picked: nothing would be left".into());
                    }
                    vec![pick]
                }
            };
            // Take pieces out from the last so earlier masks stay valid.
            let mut marks: Vec<Vec<bool>> = groups;
            let mut pieces = Vec::new();
            while let Some(m) = marks.pop() {
                let mut rest = marks.clone();
                let piece = {
                    let mut out = p.clone();
                    out.retain_faces(|f| m[f]);
                    out.compact();
                    out
                };
                // The remaining masks without the faces just taken.
                for r in &mut rest {
                    let kept: Vec<bool> = r.iter().zip(&m).filter(|(_, gone)| !**gone).map(|(b, _)| *b).collect();
                    *r = kept;
                }
                p.retain_faces(|f| !m[f]);
                p.compact();
                marks = rest;
                pieces.push(piece);
            }
            pieces.reverse();
            s.fit(p);
            s.clear();
            let n = pieces.len();
            done.pieces = pieces;
            format!("separated into {} more", plural(n, "object", "objects"))
        }
        "mirror" => {
            let verts = need_verts(s)?;
            let ax = a.axis("axis")?;
            ops::mirror_in_place(p, &verts, ax, pivot);
            format!("mirrored across {}", ["x", "y", "z"][ax])
        }
        "symmetrize" => {
            let ax = a.axis("axis")?;
            let positive = a.choice("direction")? == "positive_to_negative";
            let (lo, hi) = p.bounds();
            edit::symmetrize(p, ax, positive, length(sub(hi, lo)).max(1.0) * 1e-4);
            s.fit(p);
            s.clear();
            format!("symmetric across {}", ["x", "y", "z"][ax])
        }
        "shrink_fatten" => {
            let verts = need_verts(s)?;
            let d = a.f("distance")?;
            let normals = crate::modifiers::vertex_normals(p);
            for v in 0..p.positions.len() {
                if verts[v] {
                    p.positions[v] = add(p.positions[v], scale(normals[v], d));
                }
            }
            format!("moved along normals {d:.1} px")
        }
        "to_sphere" => {
            let verts = need_verts(s)?;
            let k = a.f("factor")?;
            let list = s.vert_list();
            let c = ops::centre_of(p, &list);
            let r = list.iter().map(|&v| length(sub(p.positions[v as usize], c))).sum::<f32>() / list.len().max(1) as f32;
            for v in 0..p.positions.len() {
                if verts[v] {
                    let q = p.positions[v];
                    p.positions[v] = studio_mesh::lerp(q, add(c, scale(normalise(sub(q, c)), r)), k);
                }
            }
            format!("{:.0} % to a sphere", k * 100.0)
        }
        "smooth_vertices" => {
            let verts = need_verts(s)?;
            crate::modifiers::smooth(p, a.n("iterations")? as usize, a.f("factor")?, &verts);
            "smoothed".into()
        }
        "randomize" => {
            let verts = need_verts(s)?;
            ops::randomize(p, &verts, a.f("amount")?, a.n("seed")? as u32);
            "randomized".into()
        }
        "selection_to_cursor" => {
            let verts = need_verts(s)?;
            let c = s.median(p).unwrap_or(ctx.cursor);
            let d = sub(ctx.cursor, c);
            for v in 0..p.positions.len() {
                if verts[v] {
                    p.positions[v] = add(p.positions[v], d);
                }
            }
            "moved to the 3D cursor".into()
        }
        "bisect" | "knife" => {
            let point = if args.contains_key("point") { a.v3("point")? } else { pivot };
            let normal = if args.contains_key("normal") { a.v3("normal")? } else { ctx.view_side() };
            if length(normal) < 1e-9 {
                return Err("\"normal\" can't be zero".into());
            }
            let pick = faces_or_all(p, s);
            let plane = Plane { at: point, normal };
            let n = if id == "knife" {
                let mut made = Vec::new();
                ops::keep_face_pick(p, &pick, |p| {
                    let m = ops::marked(p);
                    made = edit::knife(p, &m, plane, &|_| true);
                });
                s.fit(p);
                // The cut stays picked: as edges (face mode switches to edges).
                let mode = if s.mode == Mode::Vertex { Mode::Vertex } else { Mode::Edge };
                s.mode = Mode::Edge;
                s.with_edges(p, &made);
                s.mode = mode;
                s.sync(p);
                made.len()
            } else {
                let clear = match a.choice("clear")? {
                    "front" => Some(true),
                    "back" => Some(false),
                    _ => None,
                };
                let n = ops::bisect_in(p, &pick, plane, clear, a.b("fill"));
                s.fit(p);
                s.clear();
                n
            };
            format!("cut {}", plural(n, "new edge", "new edges"))
        }
        "spin" | "screw" => {
            let edges = need_edges(s)?;
            let ax = a.axis("axis")?;
            let chains = ops::edge_chains(&edges);
            let mut made = Vec::new();
            for (chain, closed) in &chains {
                let mut prof: Vec<V3> = chain.iter().map(|&v| p.positions[v as usize]).collect();
                if *closed {
                    prof.push(prof[0]);
                }
                let shape = if id == "spin" {
                    ops::spin_about(&prof, ctx.cursor, ax, a.f("degrees")?, a.n("steps")? as usize)
                } else {
                    ops::screw(&prof, ctx.cursor, ax, a.f("turns")?, a.n("steps")? as usize, a.f("height")?)
                };
                let onto: Vec<u32> = chain.clone();
                made.extend(ops::merge_welded(p, &shape, &onto));
            }
            select_faces_after(p, s, &made);
            format!("{} made {}", t.label.to_lowercase(), plural(made.len(), "face", "faces"))
        }
        "subdivide" => {
            let edges: HashSet<Edge> = need_edges(s)?.into_iter().collect();
            let cuts = a.n("cuts")? as usize;
            let picked_faces = s.face_pick();
            let mut new_verts = Vec::new();
            let after = ops::keep_face_pick(p, &picked_faces, |p| new_verts = ops::subdivide_edges(p, &edges, cuts));
            let faces_after: Vec<u32> = after.iter().enumerate().filter(|(_, b)| **b).map(|(i, _)| i as u32).collect();
            let k = a.f("smooth")?;
            if k > 0.0 {
                let mut pick = vec![false; p.positions.len()];
                for &v in &new_verts {
                    pick[v as usize] = true;
                }
                crate::modifiers::smooth(p, 2, k, &pick);
            }
            s.fit(p);
            if s.mode == Mode::Face {
                s.with_faces(p, &faces_after);
            } else {
                let mut vs = s.vert_list();
                vs.extend(new_verts.iter().copied());
                let mode = s.mode;
                s.mode = Mode::Vertex;
                s.with_vertices(p, &vs);
                s.mode = mode;
                s.sync(p);
            }
            format!("subdivided {} into {} each", plural(edges.len(), "edge", "edges"), cuts + 1)
        }
        "triangulate" => {
            let pick = faces_or_all(p, s);
            let after = ops::keep_face_pick(p, &pick, |p| {
                let m = ops::marked(p);
                edit::triangulate_faces(p, &m)
            });
            reselect(p, s, &after);
            "triangulated".into()
        }
        "tris_to_quads" => {
            let pick = faces_or_all(p, s);
            let deg = a.f("angle")?;
            // Joins only among picked faces: the others are set aside.
            let mut kept = p.clone();
            kept.retain_faces(|f| !pick[f]);
            let mut work = p.clone();
            work.retain_faces(|f| pick[f]);
            let n = edit::tris_to_quads(&mut work, deg);
            if work.has_uvs() != kept.has_uvs() {
                work.uvs.clear();
                kept.uvs.clear();
            }
            let first = kept.faces();
            let mut out = kept;
            out.positions = p.positions.clone();
            let mats = out.materials.clone();
            for f in 0..work.faces() {
                let uv = work.face_uvs(f).map(|u| u.to_vec());
                out.add_face_like(work.face(f), uv.as_deref(), work.face_material[f], work.face_set[f], work.face_smooth[f]);
            }
            out.materials = mats;
            *p = out;
            let after: Vec<bool> = (0..p.faces()).map(|f| f >= first).collect();
            reselect(p, s, &after);
            plural(n, "quad made", "quads made")
        }
        // ------------------------------------------------------ vertex
        "bevel_vertices" => {
            let verts = need_verts(s)?;
            let n = ops::bevel_vertices(p, &verts, a.f("width")?, a.n("segments")? as usize, a.f("profile")?);
            s.fit(p);
            s.clear();
            plural(n, "corner bevelled", "corners bevelled")
        }
        "connect" => {
            let verts = need_verts(s)?;
            let made = ops::connect_vertices(p, &verts);
            if made.is_empty() {
                return Err("no face has two picked corners that aren't neighbours".into());
            }
            s.fit(p);
            let mode = s.mode;
            s.mode = Mode::Edge;
            s.with_edges(p, &made);
            s.mode = mode;
            s.sync(p);
            plural(made.len(), "edge made", "edges made")
        }
        "make_face" => {
            let edges: HashSet<Edge> = s.edges.clone();
            let made = ops::make_face(p, &s.verts, &edges)?;
            s.fit(p);
            if !made.is_empty() {
                let mode = s.mode;
                s.mode = Mode::Face;
                s.with_faces(p, &made);
                s.mode = mode;
                s.sync(p);
            }
            if made.is_empty() { "face split".into() } else { plural(made.len(), "face made", "faces made") }
        }
        "vertex_slide" => {
            let verts = need_verts(s)?;
            let n = ops::vertex_slide(p, &verts, a.v3("toward")?, a.f("factor")?);
            plural(n, "vertex slid", "vertices slid")
        }
        // ------------------------------------------------------ edge
        "bevel" => {
            let edges = need_edges(s)?;
            let w = a.f("width")?;
            let n = crate::bevel::bevel(p, &edges, w, a.n("segments")? as usize, a.f("profile")?);
            if n == 0 {
                return Err("none of those edges could be bevelled (each needs a face on both sides)".into());
            }
            s.fit(p);
            s.clear();
            format!("bevelled {} {w:.1} px", plural(n, "edge", "edges"))
        }
        "loop_cut" => {
            let e = active_edge(p, s, args)?;
            let made = edit::loop_cut(p, e, a.n("cuts")? as usize, a.f("slide")?);
            if made.is_empty() {
                return Err("there is no ring of quads across that edge".into());
            }
            s.fit(p);
            let mode = s.mode;
            s.mode = Mode::Edge;
            s.with_edges(p, &made);
            s.mode = if mode == Mode::Face { Mode::Edge } else { mode };
            s.sync(p);
            plural(made.len(), "edge cut", "edges cut")
        }
        "edge_slide" => {
            let edges = need_edges(s)?;
            let n = ops::edge_slide(p, &edges, a.f("factor")?)?;
            plural(n, "vertex slid", "vertices slid")
        }
        "bridge" => {
            let made = if s.mode == Mode::Face && s.faces.iter().any(|b| *b) {
                let groups = face_groups(p, &s.face_pick());
                if groups.len() != 2 {
                    return Err(format!("bridging faces needs two separate groups picked; there are {}", groups.len()));
                }
                let before = p.faces();
                edit::bridge_faces(p, &groups[0], &groups[1], a.n("segments")? as usize, a.n("twist")? as i32)?;
                let _ = before;
                // The band's faces are those whose vertices all lie on the
                // two rims or between them; pick none, keep it simple.
                Vec::new()
            } else {
                let edges: HashSet<Edge> = need_edges(s)?.into_iter().collect();
                ops::bridge_loops(p, &edges, a.n("segments")? as usize, a.n("twist")? as i32)?
            };
            select_faces_after(p, s, &made);
            if made.is_empty() { "bridged".into() } else { format!("bridged with {}", plural(made.len(), "face", "faces")) }
        }
        "fill" | "grid_fill" => {
            let edges: HashSet<Edge> = need_edges(s)?.into_iter().collect();
            let made = ops::fill(p, &edges, id == "grid_fill")?;
            select_faces_after(p, s, &made);
            plural(made.len(), "face made", "faces made")
        }
        "mark_seam" | "mark_sharp" => {
            done.changed = true;
            let edges = need_edges(s)?;
            let clear = a.b("clear");
            let set = if id == "mark_seam" { &mut p.seams } else { &mut p.sharp };
            for e in &edges {
                if clear {
                    set.remove(e);
                } else {
                    set.insert(*e);
                }
            }
            format!("{} {}", if clear { "cleared" } else { "marked" }, plural(edges.len(), "edge", "edges"))
        }
        "crease" => {
            let edges = need_edges(s)?;
            let w = a.f("amount")?;
            for e in &edges {
                if w <= 0.0 {
                    p.creases.remove(e);
                } else {
                    p.creases.insert(*e, w);
                }
            }
            format!("crease {w:.2} on {}", plural(edges.len(), "edge", "edges"))
        }
        "mark_sharp_by_angle" => {
            let n = edit::mark_sharp(p, a.f("angle")?, a.b("crease").then_some(1.0), a.b("seam"));
            plural(n, "edge marked", "edges marked")
        }
        // ------------------------------------------------------ face
        "inset" => {
            let pick = need_faces(s)?;
            let caps = edit::inset(p, &pick, a.f("thickness")?, a.f("depth")?, a.b("individual"));
            select_faces_after(p, s, &caps);
            format!("inset {}", plural(caps.len(), "face", "faces"))
        }
        "poke" => {
            let pick = need_faces(s)?;
            let off = a.f("offset")?;
            let after = ops::keep_face_pick(p, &pick, |p| {
                let m = ops::marked(p);
                edit::poke(p, &m, off)
            });
            reselect(p, s, &after);
            "poked".into()
        }
        "solidify" => {
            let pick = need_faces(s)?;
            let caps = ops::solidify_faces(p, &pick, a.f("thickness")?);
            select_faces_after(p, s, &caps);
            "solidified".into()
        }
        "assign_material" => {
            let pick = need_faces(s)?;
            let slot = a.n("slot")? as u32;
            let mut n = 0;
            for f in 0..p.faces() {
                if pick[f] {
                    p.face_material[f] = slot;
                    n += 1;
                }
            }
            p.ensure_materials();
            format!("material slot {slot} on {}", plural(n, "face", "faces"))
        }
        "shade_smooth" | "shade_flat" => {
            let pick = faces_or_all(p, s);
            let on = id == "shade_smooth";
            let mut n = 0;
            for f in 0..p.faces() {
                if pick[f] {
                    p.face_smooth[f] = on;
                    n += 1;
                }
            }
            format!("{} shade {}", plural(n, "face", "faces"), if on { "smooth" } else { "flat" })
        }
        "recalc_outside" | "recalc_inside" | "flip_normals" => {
            let pick = faces_or_all(p, s);
            let n = match id {
                "flip_normals" => ops::flip(p, &pick),
                _ => ops::recalc_normals(p, &pick, id == "recalc_inside"),
            };
            plural(n, "face turned", "faces turned")
        }
        // ------------------------------------------------------ clean up
        "merge_by_distance" => {
            let verts = verts_or_all(p, s);
            let n = ops::merge_by_distance_in(p, &verts, a.f("distance")?);
            s.fit(p);
            s.clear();
            plural(n, "vertex merged", "vertices merged")
        }
        "fill_holes" => {
            let n = edit::fill_holes(p, a.n("sides")? as usize);
            s.fit(p);
            plural(n, "hole filled", "holes filled")
        }
        "limited_dissolve" => {
            let n = crate::modifiers::limited_dissolve(p, a.f("angle")?);
            s.fit(p);
            s.clear();
            plural(n, "face joined", "faces joined")
        }
        "fix_t_junctions" => {
            let n = edit::fix_t_junctions(p, a.f("distance")?);
            s.fit(p);
            plural(n, "crack stitched", "cracks stitched")
        }
        "decimate" => {
            let before = p.faces();
            let ev = crate::modifiers::apply(p, &[crate::modifiers::Modifier::Decimate { ratio: a.f("ratio")? }], &crate::modifiers::NoParts)?;
            *p = ev.poly;
            s.fit(p);
            s.clear();
            format!("{before} faces to {}", p.faces())
        }
        // ------------------------------------------------------ uv
        "unwrap" => {
            let seams = match a.choice("method")? {
                "angle" => crate::uv::Seams::Angle(a.f("angle")?),
                "smart" => crate::uv::Seams::Smart(a.f("angle")?),
                _ => crate::uv::Seams::Marked,
            };
            let q = crate::uv::unwrap(p, &crate::uv::Unwrap { seams, margin: a.f("margin")?, rotate: true });
            format!("{} pieces, {:.0} % of the square used, stretch {:.2}", q.charts, q.coverage * 100.0, q.stretch)
        }
        "project_uvs" => {
            let kind = a.choice("kind")?;
            if kind == "view" {
                let v = normalise(ctx.view);
                let up = if v[1].abs() < 0.9 { [0.0, -1.0, 0.0] } else { [0.0, 0.0, 1.0] };
                let x = normalise(studio_mesh::cross(up, v));
                let y = studio_mesh::cross(v, x);
                let size = a.f("size")?.max(1e-6);
                let mut uvs = Vec::with_capacity(p.corners.len());
                for &c in &p.corners {
                    let q = p.positions[c as usize];
                    uvs.push([studio_mesh::dot(q, x) / size, studio_mesh::dot(q, y) / size]);
                }
                p.uvs = uvs;
            } else {
                crate::uv::project(p, kind, a.f("size")?)?;
            }
            format!("projected ({kind})")
        }
        "pack_uvs" => {
            if !p.has_uvs() {
                return Err("the mesh has no texture layout yet; unwrap it first".into());
            }
            let n = ops::pack_uvs(p, a.f("margin")?);
            plural(n, "piece packed", "pieces packed")
        }
        "reset_uvs" => {
            let pick = faces_or_all(p, s);
            ops::reset_uvs(p, &pick);
            "layout reset".into()
        }
        "clear_seams" => {
            let n = p.seams.len();
            p.seams.clear();
            plural(n, "seam cleared", "seams cleared")
        }
        _ => return Err(format!("{} isn't something the mesh can do here", t.label)),
    };
    p.ensure_materials();
    s.sync(p);
    Ok(done)
}

impl Ctx {
    /// A plane normal across the view (for cuts made without one): the
    /// view's right.
    fn view_side(&self) -> V3 {
        let v = normalise(self.view);
        let up = if v[1].abs() < 0.9 { [0.0, -1.0, 0.0] } else { [0.0, 0.0, 1.0] };
        normalise(studio_mesh::cross(up, v))
    }
}

fn picked_said(s: &Selection) -> String {
    let (v, e, f) = s.count();
    format!("{} picked ({v} vertices, {e} edges, {f} faces)", match s.mode {
        Mode::Vertex => plural(v, "vertex", "vertices"),
        Mode::Edge => plural(e, "edge", "edges"),
        Mode::Face => plural(f, "face", "faces"),
    })
}

/// The edge a loop tool starts from: `args.edge` [a, b], else the active
/// edge, else the only picked edge.
fn active_edge(p: &Poly, s: &Selection, args: &Map<String, Value>) -> Result<Edge, String> {
    if let Some(e) = args.get("edge").and_then(Value::as_array).filter(|a| a.len() == 2) {
        let (a, b) = (e[0].as_u64().ok_or("\"edge\" is [vertex, vertex]")? as u32, e[1].as_u64().ok_or("\"edge\" is [vertex, vertex]")? as u32);
        let k = ekey(a, b);
        if !p.edges().contains(&k) {
            return Err(format!("vertices {a} and {b} don't share an edge"));
        }
        return Ok(k);
    }
    if let Some(Elem::Edge(e)) = s.active {
        return Ok(e);
    }
    let es = s.edge_list();
    es.first().copied().ok_or_else(|| "pick an edge (or give \"edge\": [vertex, vertex])".to_string())
}

fn region_normal(p: &Poly, pick: &[bool]) -> V3 {
    let mut n = [0.0; 3];
    for f in 0..p.faces() {
        if pick.get(f).copied().unwrap_or(false) {
            n = add(n, p.face_area_vector(f));
        }
    }
    if length(n) < 1e-12 { [0.0, -1.0, 0.0] } else { normalise(n) }
}

/// The average outward direction of open edges (away from their faces).
fn open_edges_out(p: &Poly, edges: &[Edge]) -> V3 {
    let t = Topology::of(p);
    let mut d = [0.0; 3];
    for e in edges {
        let Some(&f) = t.edge_faces.get(e).and_then(|f| f.first()) else { continue };
        let mid = studio_mesh::lerp(p.positions[e.0 as usize], p.positions[e.1 as usize], 0.5);
        d = add(d, normalise(sub(mid, p.face_centre(f as usize))));
    }
    if length(d) < 1e-9 { [0.0, -1.0, 0.0] } else { normalise(d) }
}

fn individual_extrude(p: &mut Poly, pick: &[bool], d: f32) -> Vec<u32> {
    // Each face along its own normal.
    let mut caps = Vec::new();
    let list: Vec<usize> = (0..p.faces()).filter(|&f| pick[f]).collect();
    for f in list {
        let mut one = vec![false; p.faces()];
        one[f] = true;
        let n = p.face_normal(f);
        caps.extend(edit::extrude(p, &one, d, Some(n), false));
    }
    caps
}

fn select_faces_after(p: &Poly, s: &mut Selection, faces: &[u32]) {
    s.fit(p);
    let mode = s.mode;
    s.mode = Mode::Face;
    s.with_faces(p, faces);
    s.mode = mode;
    if mode != Mode::Face {
        // Keep the smaller elements of the new faces picked.
        let vs = s.vert_list();
        let es = s.edge_list();
        s.clear();
        match mode {
            Mode::Vertex => s.with_vertices(p, &vs),
            _ => s.with_edges(p, &es),
        }
    }
    s.sync(p);
}

fn reselect(p: &Poly, s: &mut Selection, faces: &[bool]) {
    let list: Vec<u32> = faces.iter().enumerate().filter(|(_, b)| **b).map(|(i, _)| i as u32).collect();
    select_faces_after(p, s, &list);
}

/// Picked faces in connected groups.
fn face_groups(p: &Poly, pick: &[bool]) -> Vec<Vec<bool>> {
    let t = Topology::of(p);
    let mut seen = vec![false; p.faces()];
    let mut out = Vec::new();
    for seed in 0..p.faces() {
        if !pick[seed] || seen[seed] {
            continue;
        }
        let mut g = vec![false; p.faces()];
        let mut stack = vec![seed];
        seen[seed] = true;
        while let Some(f) = stack.pop() {
            g[f] = true;
            let c = p.face(f);
            for i in 0..c.len() {
                for &h in t.edge_faces.get(&ekey(c[i], c[(i + 1) % c.len()])).map(Vec::as_slice).unwrap_or(&[]) {
                    let h = h as usize;
                    if pick[h] && !seen[h] {
                        seen[h] = true;
                        stack.push(h);
                    }
                }
            }
        }
        out.push(g);
    }
    out
}

/// Adds a shape into a mesh, sharing its first material.
fn merge_shape(p: &mut Poly, shape: &Poly) {
    let mats = p.materials.clone();
    let mut s = shape.clone();
    s.materials.clear();
    for m in &mut s.face_material {
        *m = 0;
    }
    if p.faces() > 0 && !p.has_uvs() {
        s.uvs.clear();
    }
    let had_none = p.faces() == 0;
    p.merge(&s);
    p.materials = mats;
    if had_none && p.materials.is_empty() {
        p.materials = shape.materials.clone();
    }
    p.ensure_materials();
}

/// The falloff names proportional editing offers.
pub const FALLOFFS: &[(&str, Falloff)] = &[("smooth", Falloff::Smooth), ("sphere", Falloff::Sphere), ("root", Falloff::Root), ("sharp", Falloff::Sharp), ("linear", Falloff::Linear), ("constant", Falloff::Constant)];

#[cfg(test)]
mod tests;
