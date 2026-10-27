// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Editing art: drawing paths (pen, curvature, pencil), pathfinder and
//! shape builder, live paint, offset, outline stroke, simplify and smooth,
//! scissors, knife and eraser, blob brush, the width tool, expanding live
//! art and type into plain paths, image trace, and arranging objects
//! (align, distribute, order, group).

use kurbo::{Affine, BezPath, Point, Rect, Vec2};
use studio_vector_geom::{Anchor, PathData, SubPath};
use studio_vector_pathops as ops;
use studio_vector_raster::{Cap, Color, Join, Paint, StrokeStyle, WidthPoint};

use crate::adapted::distort::catmull_rom;
use crate::appearance::{Appearance, Fill, Item, Stroke};
use crate::build::{self, BuildOptions};
use crate::doc::{Document, Node, NodeId, NodeKind};
use crate::FillRule;

pub fn geom_rule(r: FillRule) -> studio_vector_geom::FillRule {
    match r {
        FillRule::NonZero => studio_vector_geom::FillRule::NonZero,
        FillRule::EvenOdd => studio_vector_geom::FillRule::EvenOdd,
    }
}

fn kjoin(j: Join) -> ops::Join {
    match j {
        Join::Miter => ops::Join::Miter,
        Join::Round => ops::Join::Round,
        Join::Bevel => ops::Join::Bevel,
    }
}

fn kcap(c: Cap) -> ops::Cap {
    match c {
        Cap::Butt => ops::Cap::Butt,
        Cap::Round => ops::Cap::Round,
        Cap::Square => ops::Cap::Square,
    }
}

// ------------------------------------------------------------------ drawing

/// A pen path: anchors with optional handles (`out` handle; `in` mirrors
/// it for smooth points).
#[derive(Clone, Debug, serde::Deserialize)]
pub struct PenPoint {
    pub at: Point,
    #[serde(default)]
    pub out: Option<Point>,
    #[serde(default, rename = "in")]
    pub h_in: Option<Point>,
}

pub fn pen(points: &[PenPoint], closed: bool) -> PathData {
    let anchors = points
        .iter()
        .map(|p| {
            let out = p.out.unwrap_or(p.at);
            let h_in = p.h_in.unwrap_or_else(|| if p.out.is_some() { p.at - (out - p.at) } else { p.at });
            Anchor::with_handles(p.at, h_in, out)
        })
        .collect();
    PathData::single(SubPath::new(anchors, closed))
}

/// The curvature tool: a smooth curve through every point (corners where
/// `corners` says so).
pub fn curvature(points: &[Point], closed: bool, corners: &[usize]) -> PathData {
    let mut sp = catmull_rom(points, closed, 1.0);
    for &i in corners {
        if let Some(a) = sp.anchors.get_mut(i) {
            *a = Anchor::corner(a.p);
        }
    }
    PathData::single(sp)
}

/// The pencil: a freehand stroke fitted with few smooth curves, `fidelity`
/// 0 (loose, smooth) .. 1 (tight, accurate).
pub fn pencil(points: &[Point], closed: bool, fidelity: f64) -> PathData {
    if points.len() < 2 {
        return PathData::default();
    }
    let size = points.iter().fold(Rect::from_points(points[0], points[0]), |r, p| r.union_pt(*p));
    let tol = (size.width().max(size.height()) * (0.02 * (1.0 - fidelity.clamp(0.0, 1.0)) + 0.002)).max(0.05);
    let poly = PathData::single(SubPath::polyline(points, closed));
    ops::simplify(&poly, tol)
}

/// Smooth tool: relaxes a path, `amount` 0..1.
pub fn smooth(path: &PathData, amount: f64) -> PathData {
    ops::smooth(path, amount.clamp(0.0, 1.0) * 100.0)
}

pub fn simplify(path: &PathData, tolerance: f64) -> PathData {
    ops::simplify(path, tolerance.max(1e-4))
}

/// Shaper tool: a rough freehand drawing recognised as a clean shape
/// (rectangle, ellipse, triangle, polygon, line).
pub fn recognize(points: &[Point]) -> Option<crate::shapes::LiveShape> {
    use crate::shapes::LiveShape;
    use studio_vector_geom::recognize::{Recognized, recognize};
    Some(match recognize(points)? {
        Recognized::Line { a, b } => LiveShape::Line { from: a, to: b },
        Recognized::Rectangle { rect, .. } => LiveShape::Rect { rect, radii: [0.0; 4], corners: None },
        Recognized::Ellipse { rect, .. } => LiveShape::Ellipse { rect },
        Recognized::Polygon { center, radius, sides, rotation } => LiveShape::Polygon { center, radius, sides, rotation },
        Recognized::Scribble(_) => return None,
    })
}

// ------------------------------------------------------------------ geometry of nodes

/// A node's outline in document space (for booleans and cutting): its
/// fills' area, or its stroke's outline when it has only strokes.
pub fn outline_of(doc: &Document, id: NodeId) -> Option<(PathData, FillRule)> {
    let n = doc.find(id)?;
    let parent = doc.parent_of(id).and_then(|p| doc.world_transform(p)).unwrap_or(Affine::IDENTITY);
    let m = parent * n.transform;
    let rule = match &n.kind {
        NodeKind::Path { rule, .. } => *rule,
        _ => FillRule::NonZero,
    };
    let pieces: Vec<PathData> = match &n.kind {
        NodeKind::Path { .. } | NodeKind::Shape { .. } | NodeKind::Text(_) | NodeKind::Image(_) => build::geometry(doc, n).into_iter().map(|p| p.path).collect(),
        _ => {
            // Groups and live art: everything they draw.
            let s = build::scene(&Document { layers: vec![{ let mut k = n.clone(); k.transform = Affine::IDENTITY; k }], ..doc.clone() }, &BuildOptions::default());
            let mut b = BezPath::new();
            collect_outlines(&s, &mut b);
            vec![PathData::from_bezpath(&b)]
        }
    };
    let mut path = PathData::new(pieces.into_iter().flat_map(|p| p.subpaths).collect());
    let has_fill = n.appearance.items.iter().any(|i| matches!(i, Item::Fill(_))) || matches!(n.kind, NodeKind::Text(_) | NodeKind::Image(_)) || n.children().is_some();
    if !has_fill
        && let Some(s) = n.appearance.top_stroke()
    {
        path = ops::outline_stroke(&path, s.style.width, kcap(s.style.start_cap), kjoin(s.style.join), s.style.miter_limit);
        path.transform(m);
        return Some((path, FillRule::NonZero));
    }
    path.transform(m);
    Some((path, rule))
}

fn collect_outlines(s: &studio_vector_raster::Scene, out: &mut BezPath) {
    for it in &s.items {
        match it {
            studio_vector_raster::Item::Fill { path, transform, .. } => out.extend(*transform * (**path).clone()),
            studio_vector_raster::Item::Stroke { path, transform, style, .. } => out.extend(*transform * studio_vector_raster::stroke_outline(path, style, 0.05)),
            studio_vector_raster::Item::Layer { scene, .. } => collect_outlines(scene, out),
        }
    }
}

/// Replaces `ids` (removed) with `nodes` placed where the topmost was.
fn replace_with(doc: &mut Document, ids: &[NodeId], nodes: Vec<Node>) -> Vec<NodeId> {
    let top = ids.iter().copied().max_by_key(|id| order_key(doc, *id));
    let parent = top.and_then(|t| doc.parent_of(t));
    let index = top.and_then(|t| doc.siblings_mut(t).map(|(_, i)| i));
    for id in ids {
        doc.remove(*id);
    }
    let mut out = Vec::new();
    let mut at = index.unwrap_or(usize::MAX);
    for mut n in nodes {
        n.id = doc.new_id();
        out.push(n.id);
        let list: &mut Vec<Node> = match parent.and_then(|p| doc.find_mut(p)).and_then(Node::children_mut) {
            Some(l) => l,
            None => {
                let l = doc.active_layer();
                match doc.find_mut(l).and_then(Node::children_mut) {
                    Some(l) => l,
                    None => continue,
                }
            }
        };
        let i = at.min(list.len());
        list.insert(i, n);
        at = i + 1;
    }
    out
}

/// Stacking order key: depth-first index (higher is in front).
fn order_key(doc: &Document, id: NodeId) -> usize {
    let mut i = 0;
    let mut out = 0;
    doc.walk(&mut |n| {
        if n.id == id {
            out = i;
        }
        i += 1;
    });
    out
}

/// Ids sorted back to front.
pub fn back_to_front(doc: &Document, ids: &[NodeId]) -> Vec<NodeId> {
    let mut v = ids.to_vec();
    v.sort_by_key(|id| order_key(doc, *id));
    v.dedup();
    v
}

fn path_node(path: PathData, appearance: Appearance, rule: FillRule) -> Node {
    let mut n = Node::new(0, NodeKind::Path { path, rule });
    n.appearance = appearance;
    n
}

// ------------------------------------------------------------------ pathfinder

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pathfinder {
    Unite,
    MinusFront,
    MinusBack,
    Intersect,
    Exclude,
    Divide,
    Trim,
    Merge,
    Crop,
    Outline,
}

impl Pathfinder {
    pub fn parse(s: &str) -> Option<Pathfinder> {
        Some(match s.to_ascii_lowercase().replace(['-', ' '], "_").as_str() {
            "unite" | "union" | "add" => Pathfinder::Unite,
            "minus_front" | "subtract" | "minus" | "difference" => Pathfinder::MinusFront,
            "minus_back" => Pathfinder::MinusBack,
            "intersect" | "intersection" => Pathfinder::Intersect,
            "exclude" | "xor" => Pathfinder::Exclude,
            "divide" => Pathfinder::Divide,
            "trim" => Pathfinder::Trim,
            "merge" => Pathfinder::Merge,
            "crop" => Pathfinder::Crop,
            "outline" => Pathfinder::Outline,
            _ => return None,
        })
    }

    fn op(self) -> ops::PathfinderOp {
        match self {
            Pathfinder::Unite => ops::PathfinderOp::Unite,
            Pathfinder::MinusFront => ops::PathfinderOp::MinusFront,
            Pathfinder::MinusBack => ops::PathfinderOp::MinusBack,
            Pathfinder::Intersect => ops::PathfinderOp::Intersect,
            Pathfinder::Exclude => ops::PathfinderOp::Exclude,
            Pathfinder::Divide => ops::PathfinderOp::Divide,
            Pathfinder::Trim => ops::PathfinderOp::Trim,
            Pathfinder::Merge => ops::PathfinderOp::Merge,
            Pathfinder::Crop => ops::PathfinderOp::Crop,
            Pathfinder::Outline => ops::PathfinderOp::Outline,
        }
    }
}

/// Runs a pathfinder operation on `ids`; the results replace them, each
/// piece keeping the appearance of the shape it came from (the front
/// shape's for unite and intersect, the back's for minus front).
pub fn pathfinder(doc: &mut Document, ids: &[NodeId], op: Pathfinder) -> Result<Vec<NodeId>, String> {
    let ids = back_to_front(doc, ids);
    if ids.len() < if matches!(op, Pathfinder::Divide | Pathfinder::Outline | Pathfinder::Unite) { 1 } else { 2 } {
        return Err("select at least two shapes".into());
    }
    let mut shapes = Vec::new();
    let mut looks = Vec::new();
    for (k, id) in ids.iter().enumerate() {
        let (p, rule) = outline_of(doc, *id).ok_or_else(|| format!("#{id} is not a shape"))?;
        shapes.push(ops::Shape::new(p, geom_rule(rule), k as u64));
        let n = doc.find(*id).cloned().ok_or("unknown node")?;
        looks.push(if n.appearance.is_empty() { Appearance::fill(Color::rgb(0.5, 0.5, 0.5)) } else { n.appearance.clone() });
    }
    let res = ops::pathfinder(op.op(), &shapes);
    let nodes: Vec<Node> = res
        .into_iter()
        .filter(|s| !s.path.is_empty())
        .map(|s| {
            let look = if op == Pathfinder::Outline {
                // Outline gives stroked edges in the colour of their shape's fill.
                let c = looks.get(s.key as usize).and_then(|a| a.top_fill()).map(|f| f.paint.clone()).unwrap_or(Paint::solid(Color::BLACK));
                Appearance { items: vec![Item::Stroke(Stroke::new(c, 1.0))], effects: Vec::new() }
            } else {
                looks.get(s.key as usize).cloned().unwrap_or_default()
            };
            path_node(s.path, look, FillRule::NonZero)
        })
        .collect();
    let out = replace_with(doc, &ids, nodes);
    // Divide, trim and merge keep their pieces together in a group.
    if matches!(op, Pathfinder::Divide | Pathfinder::Trim | Pathfinder::Merge | Pathfinder::Outline) && out.len() > 1 {
        let g = group(doc, &out)?;
        return Ok(vec![g]);
    }
    Ok(out)
}

/// Shape builder: of the regions the shapes `ids` make, merges those
/// containing any of `merge_at` into one shape and deletes those
/// containing `delete_at`; the rest stay as separate pieces.
pub fn shape_builder(doc: &mut Document, ids: &[NodeId], merge_at: &[Point], delete_at: &[Point]) -> Result<Vec<NodeId>, String> {
    let ids = back_to_front(doc, ids);
    let mut shapes = Vec::new();
    let mut looks = Vec::new();
    for (k, id) in ids.iter().enumerate() {
        let (p, rule) = outline_of(doc, *id).ok_or_else(|| format!("#{id} is not a shape"))?;
        shapes.push(ops::Shape::new(p, geom_rule(rule), k as u64));
        looks.push(doc.find(*id).map(|n| n.appearance.clone()).unwrap_or_default());
    }
    let arr = ops::shape_builder(&shapes, false);
    let hit = |r: &ops::Region, pts: &[Point]| pts.iter().any(|p| studio_vector_geom::hit::fill_contains(&r.path.to_bezpath(), studio_vector_geom::FillRule::NonZero, *p));
    let merged: Vec<&ops::Region> = arr.regions.iter().filter(|r| hit(r, merge_at)).collect();
    let mut nodes = Vec::new();
    if !merged.is_empty() {
        let p = ops::merge_regions(&merged);
        let look = merged[0].top().and_then(|k| looks.get(k).cloned()).unwrap_or_default();
        nodes.push(path_node(p, look, FillRule::NonZero));
    }
    for r in &arr.regions {
        if hit(r, merge_at) || hit(r, delete_at) {
            continue;
        }
        let look = r.top().and_then(|k| looks.get(k).cloned()).unwrap_or_default();
        nodes.push(path_node(r.path.clone(), look, FillRule::NonZero));
    }
    Ok(replace_with(doc, &ids, nodes))
}

/// Live paint: the regions the outlines of `ids` enclose become separate
/// faces that can each be filled; `fills` colours the faces containing
/// each point. Returns the new group.
pub fn live_paint(doc: &mut Document, ids: &[NodeId], fills: &[(Point, Paint)]) -> Result<NodeId, String> {
    let ids = back_to_front(doc, ids);
    let mut shapes = Vec::new();
    for (k, id) in ids.iter().enumerate() {
        let n = doc.find(*id).ok_or("unknown node")?;
        let parent = doc.parent_of(*id).and_then(|p| doc.world_transform(p)).unwrap_or(Affine::IDENTITY);
        let pieces = build::geometry(doc, n);
        let mut p = PathData::new(pieces.into_iter().flat_map(|p| p.path.subpaths).collect());
        p.transform(parent * n.transform);
        shapes.push(ops::Shape::new(p, studio_vector_geom::FillRule::NonZero, k as u64));
    }
    let (regions, edges) = ops::live_paint(&shapes);
    let mut kids = Vec::new();
    for r in &regions {
        let paint = fills.iter().find(|(p, _)| studio_vector_geom::hit::fill_contains(&r.path.to_bezpath(), studio_vector_geom::FillRule::NonZero, *p)).map(|f| f.1.clone());
        let ap = match paint {
            Some(p) => Appearance::fill(p),
            None => Appearance::fill(Color::rgba(1.0, 1.0, 1.0, 0.0)),
        };
        let mut n = path_node(r.path.clone(), ap, FillRule::NonZero);
        n.name = "Face".into();
        kids.push(n);
    }
    for e in edges {
        let mut n = path_node(e.path, Appearance::stroke(Color::BLACK, 1.0), FillRule::NonZero);
        n.name = "Edge".into();
        kids.push(n);
    }
    let mut g = Node::new(0, NodeKind::Group { children: kids, clip: false, layer: false });
    g.name = "Live paint".into();
    let new = replace_with(doc, &ids, vec![g]);
    let id = new.first().copied().ok_or("nothing to paint")?;
    let mut kids = doc.find(id).and_then(|n| n.children().cloned()).unwrap_or_default();
    doc.reid(&mut kids);
    if let Some(c) = doc.find_mut(id).and_then(Node::children_mut) {
        *c = kids;
    }
    Ok(id)
}

// ------------------------------------------------------------------ path edits

/// Applies `f` to a path node's (or live shape's, which becomes a path)
/// geometry in place.
pub fn edit_path(doc: &mut Document, id: NodeId, f: impl FnOnce(&PathData) -> PathData) -> Result<(), String> {
    let n = doc.find(id).ok_or("unknown node")?;
    let path = match &n.kind {
        NodeKind::Path { path, .. } => path.clone(),
        NodeKind::Shape { shape } => shape.to_path(),
        _ => return Err(format!("#{id} is a {}, not a path", n.kind_name())),
    };
    let rule = match &n.kind {
        NodeKind::Path { rule, .. } => *rule,
        _ => FillRule::NonZero,
    };
    let out = f(&path);
    if let Some(n) = doc.find_mut(id) {
        n.kind = NodeKind::Path { path: out, rule };
    }
    Ok(())
}

/// Offset path: a new path `amount` outside (inside when negative) the node.
pub fn offset(doc: &mut Document, id: NodeId, amount: f64, join: Join) -> Result<NodeId, String> {
    let (p, _) = outline_of(doc, id).ok_or("not a shape")?;
    let out = ops::offset_path(&p, amount, kjoin(join), 4.0);
    let look = doc.find(id).map(|n| n.appearance.clone()).unwrap_or_default();
    let mut n = path_node(out, look, FillRule::NonZero);
    n.name = "Offset".into();
    let parent = doc.parent_of(id);
    let inv = parent.and_then(|p| doc.world_transform(p)).unwrap_or(Affine::IDENTITY).inverse();
    if let NodeKind::Path { path, .. } = &mut n.kind {
        path.transform(inv);
    }
    Ok(doc.add(parent, n))
}

/// Outline stroke: the node's stroke becomes a filled path in its colour.
pub fn outline_stroke(doc: &mut Document, id: NodeId) -> Result<NodeId, String> {
    let n = doc.find(id).cloned().ok_or("unknown node")?;
    let s = n.appearance.top_stroke().cloned().ok_or("it has no stroke")?;
    let pieces = build::geometry(doc, &n);
    let path = PathData::new(pieces.into_iter().flat_map(|p| p.path.subpaths).collect());
    let out = if s.style.profile.is_empty() && s.style.dashes.is_empty() {
        ops::outline_stroke(&path, s.style.width, kcap(s.style.start_cap), kjoin(s.style.join), s.style.miter_limit)
    } else {
        // Variable widths and dashes: the rasterizer's outline, united.
        let raw = studio_vector_raster::stroke_outline(&path.to_bezpath(), &s.style, 0.02);
        ops::normalize(&PathData::from_bezpath(&raw), studio_vector_geom::FillRule::NonZero)
    };
    let mut fill = Fill::new(s.paint.clone());
    fill.opacity = s.opacity;
    let mut node = path_node(out, Appearance { items: vec![Item::Fill(fill)], effects: Vec::new() }, FillRule::NonZero);
    node.transform = n.transform;
    node.name = if n.name.is_empty() { "Outlined stroke".into() } else { n.name.clone() };
    let out = replace_with(doc, &[id], vec![node]);
    out.first().copied().ok_or_else(|| "nothing to outline".into())
}

/// Scissors: cuts a path at the point nearest `at` (closed paths open
/// there; open paths become two).
pub fn scissors(doc: &mut Document, id: NodeId, at: Point) -> Result<Vec<NodeId>, String> {
    let n = doc.find(id).cloned().ok_or("unknown node")?;
    let path = match &n.kind {
        NodeKind::Path { path, .. } => path.clone(),
        NodeKind::Shape { shape } => shape.to_path(),
        _ => return Err("only paths can be cut".into()),
    };
    let local = n.transform.inverse() * at;
    let (si, seg, t, _, _) = path.nearest(local).ok_or("the path is empty")?;
    let mut sp = path.subpaths[si].clone();
    let k = sp.insert_anchor(seg, t);
    let mut pieces = Vec::new();
    if sp.closed {
        sp.anchors.rotate_left(k);
        let first = sp.anchors[0];
        sp.anchors.push(first);
        sp.closed = false;
        pieces.push(sp);
    } else {
        let a = SubPath::new(sp.anchors[..=k].to_vec(), false);
        let b = SubPath::new(sp.anchors[k..].to_vec(), false);
        pieces.push(a);
        pieces.push(b);
    }
    let mut others = path.subpaths.clone();
    others.remove(si);
    let mut nodes = Vec::new();
    for (i, p) in pieces.into_iter().enumerate() {
        let mut subs = vec![p];
        if i == 0 {
            subs.extend(others.iter().cloned());
        }
        let mut m = n.clone();
        m.kind = NodeKind::Path { path: PathData::new(subs), rule: FillRule::NonZero };
        nodes.push(m);
    }
    Ok(replace_with(doc, &[id], nodes))
}

/// Knife: a freehand line through shapes splits each into the pieces on
/// either side.
pub fn knife(doc: &mut Document, ids: &[NodeId], line: &[Point]) -> Result<Vec<NodeId>, String> {
    if line.len() < 2 {
        return Err("the cut needs at least two points".into());
    }
    // The line is an open path: the shape builder treats it as a cutting edge.
    let cut = PathData::single(SubPath::polyline(line, false));
    let mut out = Vec::new();
    for id in back_to_front(doc, ids) {
        let Some((p, rule)) = outline_of(doc, id) else { continue };
        let look = doc.find(id).map(|n| n.appearance.clone()).unwrap_or_default();
        let shapes = [ops::Shape::new(p, geom_rule(rule), 0), ops::Shape::new(cut.clone(), studio_vector_geom::FillRule::NonZero, 1)];
        let arr = ops::shape_builder(&shapes, false);
        let pieces: Vec<Node> = arr.regions.into_iter().filter(|r| r.sources.contains(&0) && !r.path.is_empty()).map(|r| path_node(r.path, look.clone(), FillRule::NonZero)).collect();
        // Shapes inside a group keep their parent's space.
        let inv = doc.parent_of(id).and_then(|p| doc.world_transform(p)).unwrap_or(Affine::IDENTITY).inverse();
        let pieces = pieces
            .into_iter()
            .map(|mut n| {
                if let NodeKind::Path { path, .. } = &mut n.kind {
                    path.transform(inv);
                }
                n
            })
            .collect();
        out.extend(replace_with(doc, &[id], pieces));
    }
    Ok(out)
}

/// Eraser: rubs out the area a stroke of `width` along `line` covers.
pub fn eraser(doc: &mut Document, ids: &[NodeId], line: &[Point], width: f64) -> Result<Vec<NodeId>, String> {
    let rub = PathData::single(SubPath::polyline(line, false));
    let area = ops::outline_stroke(&rub, width.max(0.1), ops::Cap::Round, ops::Join::Round, 4.0);
    let mut out = Vec::new();
    for id in back_to_front(doc, ids) {
        let Some((p, rule)) = outline_of(doc, id) else { continue };
        let look = doc.find(id).map(|n| n.appearance.clone()).unwrap_or_default();
        let left = ops::boolean(&p, geom_rule(rule), &area, studio_vector_geom::FillRule::NonZero, ops::BoolOp::Difference);
        let inv = doc.parent_of(id).and_then(|p| doc.world_transform(p)).unwrap_or(Affine::IDENTITY).inverse();
        let nodes = if left.is_empty() { Vec::new() } else { vec![path_node(left.transformed(inv), look, FillRule::NonZero)] };
        out.extend(replace_with(doc, &[id], nodes));
    }
    Ok(out)
}

/// Blob brush: paints a filled shape along `line` (`width` wide) and
/// merges it with the shapes it touches that share its colour.
pub fn blob_brush(doc: &mut Document, line: &[Point], width: f64, color: Color, parent: Option<NodeId>) -> Result<NodeId, String> {
    let stroke = PathData::single(SubPath::polyline(line, false));
    let mut area = ops::outline_stroke(&stroke, width.max(0.1), ops::Cap::Round, ops::Join::Round, 4.0);
    // Shapes of the same flat colour it overlaps join it.
    let mut joined = Vec::new();
    let candidates: Vec<NodeId> = {
        let mut v = Vec::new();
        doc.walk(&mut |n| {
            if matches!(n.kind, NodeKind::Path { .. }) && n.appearance.items.len() == 1 && matches!(n.appearance.top_fill().map(|f| &f.paint), Some(Paint::Solid { color: c }) if c.to_hex() == color.to_hex()) {
                v.push(n.id);
            }
        });
        v
    };
    let ab = area.bounds();
    for id in candidates {
        let Some((p, rule)) = outline_of(doc, id) else { continue };
        if p.bounds().zip(ab).is_some_and(|(a, b)| a.intersect(b).area() > 0.0) {
            let both = ops::boolean(&p, geom_rule(rule), &area, studio_vector_geom::FillRule::NonZero, ops::BoolOp::Intersect);
            if !both.is_empty() {
                area = ops::boolean(&p, geom_rule(rule), &area, studio_vector_geom::FillRule::NonZero, ops::BoolOp::Union);
                joined.push(id);
            }
        }
    }
    for id in &joined {
        doc.remove(*id);
    }
    let mut n = path_node(area, Appearance::fill(color), FillRule::NonZero);
    n.name = "Blob".into();
    Ok(doc.add(parent, n))
}

/// The width tool: sets a stroke's width profile (points at 0..1 along
/// the path, widths as multiples of the stroke weight).
pub fn set_width_profile(doc: &mut Document, id: NodeId, profile: Vec<WidthPoint>) -> Result<(), String> {
    let n = doc.find_mut(id).ok_or("unknown node")?;
    let mut done = false;
    for it in n.appearance.items.iter_mut().rev() {
        if let Item::Stroke(s) = it {
            let mut p = profile.clone();
            p.sort_by(|a, b| a.at.total_cmp(&b.at));
            s.style.profile = p;
            done = true;
            break;
        }
    }
    if done { Ok(()) } else { Err("it has no stroke".into()) }
}

/// Named width profiles.
pub fn width_preset(name: &str) -> Option<Vec<WidthPoint>> {
    let w = |a: f64, b: f64| WidthPoint { at: a, width: b };
    Some(match name {
        "uniform" => Vec::new(),
        "taper" | "taper_end" => vec![w(0.0, 1.0), w(1.0, 0.0)],
        "taper_start" => vec![w(0.0, 0.0), w(1.0, 1.0)],
        "taper_both" | "leaf" => vec![w(0.0, 0.0), w(0.5, 1.0), w(1.0, 0.0)],
        "bulge" => vec![w(0.0, 0.4), w(0.5, 1.4), w(1.0, 0.4)],
        "pinch" => vec![w(0.0, 1.0), w(0.5, 0.2), w(1.0, 1.0)],
        "brush" => vec![w(0.0, 0.1), w(0.15, 1.0), w(0.7, 0.8), w(1.0, 0.05)],
        _ => return None,
    })
}

// ------------------------------------------------------------------ expand

/// Expands a node into plain paths: live shapes, type (outlines), live
/// effects and appearance stacks (each fill and stroke its own path),
/// symbols, repeats, blends and envelopes. Returns the new group or path.
pub fn expand(doc: &mut Document, id: NodeId) -> Result<NodeId, String> {
    let n = doc.find(id).cloned().ok_or("unknown node")?;
    let mut single = doc.clone();
    single.layers = vec![{
        let mut k = n.clone();
        k.transform = Affine::IDENTITY;
        k.opacity = 1.0;
        k.blend = Default::default();
        k.mask = None;
        k
    }];
    let s = build::scene(&single, &BuildOptions::default());
    let mut kids = Vec::new();
    scene_to_nodes(&s, &mut kids);
    let mut out = if kids.len() == 1 {
        kids.pop().unwrap_or_else(|| Node::new(0, NodeKind::Group { children: Vec::new(), clip: false, layer: false }))
    } else {
        Node::new(0, NodeKind::Group { children: kids, clip: false, layer: false })
    };
    out.transform = n.transform;
    out.opacity = n.opacity;
    out.blend = n.blend;
    out.mask = n.mask.clone();
    out.name = n.name.clone();
    let new = replace_with(doc, &[id], vec![out]);
    let new_id = new.first().copied().ok_or("nothing to expand")?;
    let mut kids = doc.find(new_id).and_then(|n| n.children().cloned()).unwrap_or_default();
    doc.reid(&mut kids);
    if let Some(c) = doc.find_mut(new_id).and_then(Node::children_mut) {
        *c = kids;
    }
    Ok(new_id)
}

fn scene_to_nodes(s: &studio_vector_raster::Scene, out: &mut Vec<Node>) {
    use studio_vector_raster::Item as SI;
    for it in &s.items {
        match it {
            SI::Fill { path, transform, rule, paint, blend, opacity } => {
                let mut f = Fill::new(paint.clone());
                f.blend = *blend;
                f.opacity = *opacity;
                let mut n = path_node(PathData::from_bezpath(path), Appearance { items: vec![Item::Fill(f)], effects: Vec::new() }, *rule);
                n.transform = *transform;
                out.push(n);
            }
            SI::Stroke { path, transform, style, paint, blend, opacity } => {
                let o = studio_vector_raster::stroke_outline(path, style, 0.02);
                let mut f = Fill::new(paint.clone());
                f.blend = *blend;
                f.opacity = *opacity;
                let mut n = path_node(PathData::from_bezpath(&o), Appearance { items: vec![Item::Fill(f)], effects: Vec::new() }, FillRule::NonZero);
                n.transform = *transform;
                out.push(n);
            }
            SI::Layer { layer, scene } => {
                let mut kids = Vec::new();
                scene_to_nodes(scene, &mut kids);
                let mut g = Node::new(0, NodeKind::Group { children: kids, clip: false, layer: false });
                g.opacity = layer.opacity;
                g.blend = layer.blend;
                if let Some(c) = &layer.clip {
                    let cn = path_node(PathData::from_bezpath(&(c.transform * (*c.path).clone())), Appearance::default(), c.rule);
                    if let Some(k) = g.children_mut() {
                        k.insert(0, cn);
                    }
                    if let NodeKind::Group { clip, .. } = &mut g.kind {
                        *clip = true;
                    }
                }
                out.push(g);
            }
        }
    }
}

/// Create outlines: type becomes compound paths with the type's appearance.
pub fn outline_text(doc: &mut Document, id: NodeId) -> Result<NodeId, String> {
    let n = doc.find(id).cloned().ok_or("unknown node")?;
    if !matches!(n.kind, NodeKind::Text(_)) {
        return Err("not type".into());
    }
    let pieces = build::geometry(doc, &n);
    let ap = if n.appearance.is_empty() { Appearance::fill(Color::BLACK) } else { n.appearance.clone() };
    let mut kids: Vec<Node> = pieces
        .into_iter()
        .map(|p| {
            let look = match p.color {
                Some(c) => Appearance::fill(c),
                None => ap.clone(),
            };
            path_node(p.path, look, FillRule::NonZero)
        })
        .collect();
    let mut out = if kids.len() == 1 { kids.remove(0) } else { Node::new(0, NodeKind::Group { children: kids, clip: false, layer: false }) };
    out.transform = n.transform;
    out.name = if let NodeKind::Text(t) = &n.kind { t.plain() } else { String::new() };
    let new = replace_with(doc, &[id], vec![out]);
    new.first().copied().ok_or_else(|| "nothing to outline".into())
}

// ------------------------------------------------------------------ trace

/// Traces a raster image into vector shapes with a preset (`"6 colors"`,
/// `"black and white logo"`, `"line art"`, `"high fidelity photo"`,
/// `"flat logo"`, …). The result replaces an image node, or is added for
/// image files. Returns the group of traced shapes.
pub fn trace(doc: &mut Document, image: &studio_vector_raster::Image, preset: &str, at: Rect, replace: Option<NodeId>) -> Result<NodeId, String> {
    let params = studio_vector_trace::preset(preset).ok_or_else(|| format!("no trace preset \"{preset}\"; try {}", studio_vector_trace::PRESET_NAMES.join(", ")))?;
    let straight = studio_vector_raster::Pixmap { width: image.width, height: image.height, data: image.data.to_vec() }.to_straight();
    let raster = studio_vector_trace::Raster::new(image.width, image.height, straight).map_err(|e| e.to_string())?;
    let res = studio_vector_trace::trace_within(&raster, &params, 2_000_000).map_err(|e| e.to_string())?;
    // Traced coordinates are image pixels: map them onto `at`.
    let m = Affine::translate((at.x0, at.y0)) * Affine::scale_non_uniform(at.width() / image.width as f64, at.height() / image.height as f64);
    let mut kids = Vec::new();
    for t in res.paths {
        let c = Color::from_rgba8(t.color[0], t.color[1], t.color[2], 255);
        let ap = match t.stroke {
            Some(w) => Appearance::stroke(c, w * m.as_coeffs()[0]),
            None => Appearance::fill(c),
        };
        kids.push(path_node(t.path.transformed(m), ap, FillRule::NonZero));
    }
    let mut g = Node::new(0, NodeKind::Group { children: kids, clip: false, layer: false });
    g.name = format!("Traced ({preset})");
    let id = match replace {
        Some(r) => {
            let inv = doc.parent_of(r).and_then(|p| doc.world_transform(p)).unwrap_or(Affine::IDENTITY).inverse();
            g.transform = inv;
            replace_with(doc, &[r], vec![g]).first().copied().ok_or("trace made nothing")?
        }
        None => doc.add(None, g),
    };
    let mut kids = doc.find(id).and_then(|n| n.children().cloned()).unwrap_or_default();
    doc.reid(&mut kids);
    if let Some(c) = doc.find_mut(id).and_then(Node::children_mut) {
        *c = kids;
    }
    Ok(id)
}

// ------------------------------------------------------------------ arrange

/// Groups `ids` (in their stacking order) where the topmost was.
pub fn group(doc: &mut Document, ids: &[NodeId]) -> Result<NodeId, String> {
    let ids = back_to_front(doc, ids);
    let top = *ids.last().ok_or("nothing to group")?;
    let parent = doc.parent_of(top);
    let index = doc.siblings_mut(top).map(|(_, i)| i).unwrap_or(0);
    // The group takes the top member's place in the stack as it is once
    // the members are out: counted before they are removed, the index
    // was too high by the members below it, and the group rose above
    // things it had sat under (labels went behind their own boxes).
    let below = ids.iter().filter(|id| **id != top && doc.parent_of(**id) == parent && doc.siblings_mut(**id).is_some_and(|(_, i)| i < index)).count();
    let index = index - below;
    let parent_m = parent.and_then(|p| doc.world_transform(p)).unwrap_or(Affine::IDENTITY);
    let mut kids = Vec::new();
    for id in &ids {
        // Keep where each child appears: move it into the group's space.
        let world = doc.world_transform(*id).unwrap_or(Affine::IDENTITY);
        if let Some(mut n) = doc.remove(*id) {
            n.transform = parent_m.inverse() * world;
            kids.push(n);
        }
    }
    let id = doc.new_id();
    let g = Node::new(id, NodeKind::Group { children: kids, clip: false, layer: false });
    let list = match parent.and_then(|p| doc.find_mut(p)).and_then(Node::children_mut) {
        Some(l) => l,
        None => &mut doc.layers,
    };
    let i = index.min(list.len());
    list.insert(i, g);
    Ok(id)
}

/// Ungroups: the children take the group's place (and transform).
pub fn ungroup(doc: &mut Document, id: NodeId) -> Result<Vec<NodeId>, String> {
    let g = doc.find(id).cloned().ok_or("unknown node")?;
    let NodeKind::Group { children, clip, .. } = &g.kind else { return Err("not a group".into()) };
    let mut kids = children.clone();
    if *clip && !kids.is_empty() {
        // The clip path itself goes (releasing the clipping mask keeps it).
        kids.remove(0);
    }
    for k in &mut kids {
        k.transform = g.transform * k.transform;
        k.opacity *= g.opacity;
    }
    let ids: Vec<NodeId> = kids.iter().map(|k| k.id).collect();
    let parent = doc.parent_of(id);
    let Some((list, i)) = doc.siblings_mut(id) else { return Err("unknown node".into()) };
    list.remove(i);
    for (k, n) in kids.into_iter().enumerate() {
        list.insert(i + k, n);
    }
    let _ = parent;
    Ok(ids)
}

/// Makes a clipping group: the frontmost of `ids` clips the others.
pub fn clip(doc: &mut Document, ids: &[NodeId]) -> Result<NodeId, String> {
    let ids = back_to_front(doc, ids);
    let g = group(doc, &ids)?;
    if let Some(n) = doc.find_mut(g)
        && let NodeKind::Group { children, clip, .. } = &mut n.kind
    {
        if let Some(top) = children.pop() {
            children.insert(0, top);
        }
        *clip = true;
    }
    Ok(g)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    HCenter,
    Right,
    Top,
    VCenter,
    Bottom,
}

impl Align {
    pub fn parse(s: &str) -> Option<Align> {
        Some(match s.to_ascii_lowercase().as_str() {
            "left" => Align::Left,
            "center" | "centre" | "hcenter" | "horizontal_center" => Align::HCenter,
            "right" => Align::Right,
            "top" => Align::Top,
            "middle" | "vcenter" | "vertical_center" => Align::VCenter,
            "bottom" => Align::Bottom,
            _ => return None,
        })
    }
}

fn move_by(doc: &mut Document, id: NodeId, d: Vec2) {
    // Movement in document space, applied in the node's parent space.
    let pm = doc.parent_of(id).and_then(|p| doc.world_transform(p)).unwrap_or(Affine::IDENTITY);
    let c = pm.inverse().as_coeffs();
    let local = Vec2::new(c[0] * d.x + c[2] * d.y, c[1] * d.x + c[3] * d.y);
    if let Some(n) = doc.find_mut(id) {
        n.transform = Affine::translate(local) * n.transform;
    }
}

/// Aligns `ids` to each other, or to `to` (an artboard's or a key object's box).
pub fn align(doc: &mut Document, ids: &[NodeId], how: Align, to: Option<Rect>) -> Result<(), String> {
    let boxes: Vec<(NodeId, Rect)> = ids.iter().filter_map(|id| build::world_bounds(doc, *id).map(|b| (*id, b))).collect();
    let all = to.or_else(|| boxes.iter().map(|b| b.1).reduce(|a, b| a.union(b))).ok_or("nothing to align")?;
    for (id, b) in boxes {
        let d = match how {
            Align::Left => Vec2::new(all.x0 - b.x0, 0.0),
            Align::HCenter => Vec2::new(all.center().x - b.center().x, 0.0),
            Align::Right => Vec2::new(all.x1 - b.x1, 0.0),
            Align::Top => Vec2::new(0.0, all.y0 - b.y0),
            Align::VCenter => Vec2::new(0.0, all.center().y - b.center().y),
            Align::Bottom => Vec2::new(0.0, all.y1 - b.y1),
        };
        move_by(doc, id, d);
    }
    Ok(())
}

/// Distributes `ids` evenly: `horizontal` or vertical, by `spacing` between
/// boxes when given, otherwise evenly between the outermost two.
pub fn distribute(doc: &mut Document, ids: &[NodeId], horizontal: bool, spacing: Option<f64>) -> Result<(), String> {
    let mut boxes: Vec<(NodeId, Rect)> = ids.iter().filter_map(|id| build::world_bounds(doc, *id).map(|b| (*id, b))).collect();
    if boxes.len() < 2 {
        return Err("distribute needs two or more objects".into());
    }
    let key = |r: &Rect| if horizontal { r.x0 } else { r.y0 };
    boxes.sort_by(|a, b| key(&a.1).total_cmp(&key(&b.1)));
    let size = |r: &Rect| if horizontal { r.width() } else { r.height() };
    let first = boxes[0].1;
    let last = boxes[boxes.len() - 1].1;
    let total: f64 = boxes.iter().map(|b| size(&b.1)).sum();
    let gap = spacing.unwrap_or_else(|| {
        let span = if horizontal { last.x1 - first.x0 } else { last.y1 - first.y0 };
        (span - total) / (boxes.len() - 1) as f64
    });
    let mut pos = if horizontal { first.x0 } else { first.y0 };
    for (id, b) in boxes {
        let d = pos - key(&b);
        move_by(doc, id, if horizontal { Vec2::new(d, 0.0) } else { Vec2::new(0.0, d) });
        pos += size(&b) + gap;
    }
    Ok(())
}

/// Moves a node within its list: `"front"`, `"back"`, `"forward"`, `"backward"`.
pub fn arrange(doc: &mut Document, id: NodeId, how: &str) -> Result<(), String> {
    let (list, i) = doc.siblings_mut(id).ok_or("unknown node")?;
    let n = list.remove(i);
    let j = match how {
        "front" | "bring_to_front" => list.len(),
        "back" | "send_to_back" => 0,
        "forward" | "bring_forward" => (i + 1).min(list.len()),
        "backward" | "send_backward" => i.saturating_sub(1),
        other => {
            list.insert(i, n);
            return Err(format!("arrange how? \"{other}\" (front, back, forward, backward)"));
        }
    };
    list.insert(j, n);
    Ok(())
}

/// Snaps a point to the grid, guides and other art's box edges and centres
/// within `tolerance`.
pub fn snap(doc: &Document, p: Point, tolerance: f64, exclude: &[NodeId]) -> Point {
    let mut xs: Vec<f64> = Vec::new();
    let mut ys: Vec<f64> = Vec::new();
    for g in &doc.guides {
        xs.extend(g.x);
        ys.extend(g.y);
    }
    for a in &doc.artboards {
        xs.extend([a.rect.x0, a.rect.center().x, a.rect.x1]);
        ys.extend([a.rect.y0, a.rect.center().y, a.rect.y1]);
    }
    let mut ids = Vec::new();
    doc.walk(&mut |n| {
        if n.children().is_none() && !exclude.contains(&n.id) {
            ids.push(n.id);
        }
    });
    for id in ids.into_iter().take(2000) {
        if let Some(b) = build::world_bounds(doc, id) {
            xs.extend([b.x0, b.center().x, b.x1]);
            ys.extend([b.y0, b.center().y, b.y1]);
        }
    }
    let mut out = p;
    if doc.grid.snap && doc.grid.spacing > 0.0 {
        let s = doc.grid.spacing / doc.grid.subdivisions.max(1) as f64;
        out = Point::new((p.x / s).round() * s, (p.y / s).round() * s);
    }
    if let Some((x, _)) = studio_vector_geom::snap::closest(p.x, xs, tolerance) {
        out.x = x;
    }
    if let Some((y, _)) = studio_vector_geom::snap::closest(p.y, ys, tolerance) {
        out.y = y;
    }
    out
}

/// A stroke style from the common knobs.
pub fn stroke_style(width: f64, join: Join, cap: Cap, dashes: Vec<f64>) -> StrokeStyle {
    StrokeStyle::new(width).with_join(join).with_caps(cap).with_dashes(dashes, 0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shapes::LiveShape;

    fn square(doc: &mut Document, x: f64, y: f64, s: f64, c: Color) -> NodeId {
        let mut n = Node::new(0, NodeKind::Shape { shape: LiveShape::Rect { rect: Rect::new(x, y, x + s, y + s), radii: [0.0; 4], corners: None } });
        n.appearance = Appearance::fill(c);
        doc.add(None, n)
    }

    #[test]
    fn pathfinder_operations_have_the_right_areas() {
        let cases = [(Pathfinder::Unite, 10000.0 + 10000.0 - 2500.0), (Pathfinder::MinusFront, 10000.0 - 2500.0), (Pathfinder::Intersect, 2500.0), (Pathfinder::Exclude, 20000.0 - 5000.0)];
        for (op, want) in cases {
            let mut d = Document::new(300.0, 300.0);
            let a = square(&mut d, 0.0, 0.0, 100.0, Color::rgb(1.0, 0.0, 0.0));
            let b = square(&mut d, 50.0, 50.0, 100.0, Color::rgb(0.0, 0.0, 1.0));
            let out = pathfinder(&mut d, &[a, b], op).unwrap();
            let area: f64 = out.iter().map(|id| outline_of(&d, *id).map(|(p, r)| studio_vector_pathops::area(&p, geom_rule(r)).abs()).unwrap_or(0.0)).sum();
            assert!((area - want).abs() < 1.0, "{op:?}: {area} vs {want}");
        }
        let mut d = Document::new(300.0, 300.0);
        let a = square(&mut d, 0.0, 0.0, 100.0, Color::rgb(1.0, 0.0, 0.0));
        let b = square(&mut d, 50.0, 50.0, 100.0, Color::rgb(0.0, 0.0, 1.0));
        let g = pathfinder(&mut d, &[a, b], Pathfinder::Divide).unwrap();
        assert_eq!(d.find(g[0]).and_then(Node::children).map(Vec::len), Some(3));
    }

    #[test]
    fn group_align_distribute_keep_positions() {
        let mut d = Document::new(300.0, 300.0);
        let a = square(&mut d, 0.0, 0.0, 10.0, Color::BLACK);
        let b = square(&mut d, 50.0, 20.0, 10.0, Color::BLACK);
        let c = square(&mut d, 200.0, 40.0, 10.0, Color::BLACK);
        align(&mut d, &[a, b, c], Align::Top, None).unwrap();
        for id in [a, b, c] {
            assert!((build::world_bounds(&d, id).unwrap().y0).abs() < 1e-6);
        }
        distribute(&mut d, &[a, b, c], true, None).unwrap();
        let bx = build::world_bounds(&d, b).unwrap().x0;
        assert!((bx - 100.0).abs() < 1e-6, "{bx}");
        let g = group(&mut d, &[a, b]).unwrap();
        assert!((build::world_bounds(&d, a).unwrap().x0).abs() < 1e-6);
        ungroup(&mut d, g).unwrap();
        assert!(d.find(g).is_none() && d.find(a).is_some());
    }

    #[test]
    fn cutting_tools() {
        let mut d = Document::new(300.0, 300.0);
        let a = square(&mut d, 0.0, 0.0, 100.0, Color::BLACK);
        let pieces = knife(&mut d, &[a], &[Point::new(-10.0, 50.0), Point::new(110.0, 50.0)]).unwrap();
        assert_eq!(pieces.len(), 2);
        let mut d = Document::new(300.0, 300.0);
        let a = square(&mut d, 0.0, 0.0, 100.0, Color::BLACK);
        let left = eraser(&mut d, &[a], &[Point::new(50.0, -10.0), Point::new(50.0, 110.0)], 20.0).unwrap();
        let area = studio_vector_pathops::area(&outline_of(&d, left[0]).unwrap().0, studio_vector_geom::FillRule::NonZero).abs();
        assert!((area - 8000.0).abs() < 30.0, "{area}");
        let p = pencil(&(0..50).map(|i| Point::new(i as f64 * 4.0, (i as f64 * 0.2).sin() * 30.0)).collect::<Vec<_>>(), false, 0.5);
        assert!(p.anchor_count() < 25 && p.anchor_count() >= 2, "{}", p.anchor_count());
    }
}
