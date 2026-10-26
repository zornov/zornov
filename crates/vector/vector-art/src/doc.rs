// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The document: artboards, a tree of layers, groups and art, and the
//! libraries art refers to (swatches, graphic styles, type styles,
//! symbols). Every node has a stable id; the JSON form is what files,
//! the AI and the undo history use.

use kurbo::{Affine, Point, Rect};
use serde::{Deserialize, Serialize};
use studio_vector_geom::PathData;
use studio_vector_raster::{BlendMode, Color, Image};

use crate::anim::Animation;
use crate::appearance::{Appearance, GraphicStyle};
use crate::color::Swatch;
use crate::shapes::LiveShape;
use crate::text::{CharStyle, ParaStyle, TextNode};

pub type NodeId = u64;

pub(crate) fn identity() -> Affine {
    Affine::IDENTITY
}

pub(crate) fn is_identity(a: &Affine) -> bool {
    *a == Affine::IDENTITY
}

fn one() -> f32 {
    1.0
}

fn yes() -> bool {
    true
}

fn is_true(b: &bool) -> bool {
    *b
}

fn is_false(b: &bool) -> bool {
    !*b
}

fn is_one(v: &f32) -> bool {
    *v == 1.0
}

fn is_normal(b: &BlendMode) -> bool {
    *b == BlendMode::Normal
}

/// A page of the document: the region exported, shown on the timeline,
/// or printed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Artboard {
    pub id: NodeId,
    #[serde(default)]
    pub name: String,
    pub rect: Rect,
    /// Painted under the art; `None` is transparent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<Color>,
}

/// A guide line: vertical at `x` or horizontal at `y`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Guide {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y: Option<f64>,
    #[serde(default)]
    pub locked: bool,
}

/// The document grid art can snap to.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Grid {
    /// Distance between major lines.
    pub spacing: f64,
    /// Minor lines per major cell.
    pub subdivisions: u32,
    #[serde(default)]
    pub visible: bool,
    #[serde(default)]
    pub snap: bool,
}

impl Default for Grid {
    fn default() -> Grid {
        Grid { spacing: 72.0, subdivisions: 8, visible: false, snap: false }
    }
}

/// An opacity mask on a node: the mask art's luminance (or alpha) decides
/// how much of the node shows.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OpacityMask {
    pub art: Vec<Node>,
    /// Use the mask's alpha instead of its brightness.
    #[serde(default)]
    pub alpha: bool,
    #[serde(default)]
    pub invert: bool,
}

/// How a repeat lays out copies of its children.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Repeat {
    /// Copies around `center`, each turned by 360° / `count`.
    Radial {
        count: u32,
        #[serde(default)]
        center: Option<Point>,
        /// Copies are pushed out from the centre by this much.
        #[serde(default)]
        radius: f64,
        /// Copies rotate to follow the circle (otherwise stay upright).
        #[serde(default = "yes")]
        rotate: bool,
    },
    /// Copies in rows and columns, `gap` apart (between bounding boxes).
    Grid {
        rows: u32,
        cols: u32,
        #[serde(default)]
        gap: (f64, f64),
        /// Every other row shifts by half a cell (brick layout).
        #[serde(default)]
        offset_rows: bool,
    },
    /// The art and its reflection across a line through `at` at `angle`
    /// degrees (0 = vertical mirror line).
    Mirror {
        #[serde(default)]
        at: Option<Point>,
        #[serde(default)]
        angle: f64,
    },
}

/// A blend between two shapes: steps of interpolated geometry and colour.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BlendSpec {
    /// Steps between the first and last child (0 = smooth: chosen from
    /// how far the colours and shapes differ).
    #[serde(default)]
    pub steps: u32,
    /// Steps follow this path (spine) instead of a straight line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spine: Option<PathData>,
    /// Steps turn to follow the spine.
    #[serde(default)]
    pub align_to_path: bool,
}

/// A warp style for envelopes (each a map of the art's bounding box).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WarpKind {
    #[default]
    Arc,
    ArcLower,
    ArcUpper,
    Arch,
    Bulge,
    ShellLower,
    ShellUpper,
    Flag,
    Wave,
    Fish,
    Rise,
    Fisheye,
    Inflate,
    Squeeze,
    Twist,
}

/// How an envelope bends the art inside it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Envelope {
    /// A preset warp: `bend` and the two distortions are -1..1.
    Warp {
        style: WarpKind,
        #[serde(default = "half")]
        bend: f64,
        #[serde(default)]
        horizontal: f64,
        #[serde(default)]
        vertical: f64,
        /// Bend vertically instead of horizontally.
        #[serde(default)]
        vertical_axis: bool,
    },
    /// A mesh: `points` is a (rows+1)×(cols+1) grid over the art's box,
    /// row by row, moved to where each grid point should go.
    Mesh { rows: u32, cols: u32, points: Vec<Point> },
    /// The four corners of the art's box moved to `corners` (top-left,
    /// top-right, bottom-right, bottom-left): a perspective-free distort.
    Free { corners: [Point; 4] },
    /// The art's box mapped onto the outline of a shape (top, right,
    /// bottom, left edges from the shape's box corners).
    Shape { shape: PathData },
}

fn half() -> f64 {
    0.5
}

/// A plane of the perspective grid.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Plane {
    #[default]
    Left,
    Right,
    Floor,
}

/// A two-point perspective grid: a horizon at `horizon_y` with vanishing
/// points at its ends, and the ground line where the planes meet the viewer.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Perspective {
    pub left_vp: Point,
    pub right_vp: Point,
    /// Where the two walls meet the floor, nearest the viewer.
    pub origin: Point,
    /// Height of the walls' reference square at the origin.
    pub cell: f64,
}

impl Default for Perspective {
    fn default() -> Perspective {
        Perspective { left_vp: Point::new(-600.0, 300.0), right_vp: Point::new(2520.0, 300.0), origin: Point::new(960.0, 800.0), cell: 100.0 }
    }
}

/// An image embedded in, or linked from, the document.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ImageNode {
    /// The pixels (embedded) or…
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<Image>,
    /// …a file next to the document, read when drawn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub href: Option<String>,
    /// Size in document units (the image is stretched to it).
    pub width: f64,
    pub height: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NodeKind {
    Group {
        #[serde(default)]
        children: Vec<Node>,
        /// A clipping group: the first child's outline clips the rest.
        #[serde(default, skip_serializing_if = "is_false")]
        clip: bool,
        /// A top-level layer (shown in the layers list as such).
        #[serde(default, skip_serializing_if = "is_false")]
        layer: bool,
    },
    Path {
        path: PathData,
        #[serde(default)]
        rule: crate::FillRule,
    },
    /// A shape that keeps its parameters (corner radius, sides, points…).
    Shape { shape: LiveShape },
    Text(TextNode),
    Image(ImageNode),
    /// An instance of a symbol: the symbol's art drawn through this node.
    Instance {
        symbol: String,
        /// Per-instance overrides of the symbol's art, by JSON pointer into
        /// its node list.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        overrides: Vec<(String, serde_json::Value)>,
    },
    /// Copies of the children laid out radially, in a grid or mirrored.
    Repeat {
        repeat: Repeat,
        #[serde(default)]
        children: Vec<Node>,
    },
    /// Steps interpolated between the first and last child.
    Blend {
        blend: BlendSpec,
        #[serde(default)]
        children: Vec<Node>,
    },
    /// The children bent by a warp, mesh or shape.
    Envelope {
        envelope: Envelope,
        #[serde(default)]
        children: Vec<Node>,
    },
    /// The children drawn on a plane of the perspective grid: their
    /// coordinates are grid units on that plane.
    Perspective {
        plane: Plane,
        #[serde(default)]
        children: Vec<Node>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub id: NodeId,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(flatten)]
    pub kind: NodeKind,
    #[serde(default = "identity", skip_serializing_if = "is_identity")]
    pub transform: Affine,
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub opacity: f32,
    #[serde(default, skip_serializing_if = "is_normal")]
    pub blend: BlendMode,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub visible: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub locked: bool,
    #[serde(default, skip_serializing_if = "Appearance::is_empty")]
    pub appearance: Appearance,
    /// A graphic style this node follows (its appearance is the style's).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mask: Option<Box<OpacityMask>>,
    /// Area type flows around this node, kept this far away.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wrap: Option<f64>,
}

impl Node {
    pub fn new(id: NodeId, kind: NodeKind) -> Node {
        Node {
            id,
            name: String::new(),
            kind,
            transform: Affine::IDENTITY,
            opacity: 1.0,
            blend: BlendMode::Normal,
            visible: true,
            locked: false,
            appearance: Appearance::default(),
            style: None,
            mask: None,
            wrap: None,
        }
    }

    pub fn children(&self) -> Option<&Vec<Node>> {
        match &self.kind {
            NodeKind::Group { children, .. }
            | NodeKind::Repeat { children, .. }
            | NodeKind::Blend { children, .. }
            | NodeKind::Envelope { children, .. }
            | NodeKind::Perspective { children, .. } => Some(children),
            _ => None,
        }
    }

    pub fn children_mut(&mut self) -> Option<&mut Vec<Node>> {
        match &mut self.kind {
            NodeKind::Group { children, .. }
            | NodeKind::Repeat { children, .. }
            | NodeKind::Blend { children, .. }
            | NodeKind::Envelope { children, .. }
            | NodeKind::Perspective { children, .. } => Some(children),
            _ => None,
        }
    }

    /// The kind's name as the JSON form spells it.
    pub fn kind_name(&self) -> &'static str {
        match &self.kind {
            NodeKind::Group { layer: true, .. } => "layer",
            NodeKind::Group { .. } => "group",
            NodeKind::Path { .. } => "path",
            NodeKind::Shape { .. } => "shape",
            NodeKind::Text(_) => "text",
            NodeKind::Image(_) => "image",
            NodeKind::Instance { .. } => "instance",
            NodeKind::Repeat { .. } => "repeat",
            NodeKind::Blend { .. } => "blend",
            NodeKind::Envelope { .. } => "envelope",
            NodeKind::Perspective { .. } => "perspective",
        }
    }
}

/// Reusable art: instances draw it wherever they are placed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Symbol {
    pub id: String,
    #[serde(default)]
    pub name: String,
    pub art: Vec<Node>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Document {
    #[serde(default = "version")]
    pub version: u32,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub artboards: Vec<Artboard>,
    /// Top-level layers, bottom first.
    #[serde(default)]
    pub layers: Vec<Node>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub swatches: Vec<Swatch>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub styles: Vec<GraphicStyle>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub char_styles: Vec<CharStyle>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub para_styles: Vec<ParaStyle>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub symbols: Vec<Symbol>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub guides: Vec<Guide>,
    #[serde(default)]
    pub grid: Grid,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub perspective: Option<Perspective>,
    #[serde(default, skip_serializing_if = "Animation::is_empty")]
    pub animation: Animation,
    /// The next id handed out.
    #[serde(default)]
    pub next_id: NodeId,
}

fn version() -> u32 {
    1
}

impl Default for Document {
    fn default() -> Document {
        Document::new(1920.0, 1080.0)
    }
}

impl Document {
    /// One artboard of `w` × `h` with one empty layer.
    pub fn new(w: f64, h: f64) -> Document {
        let mut d = Document {
            version: 1,
            name: String::new(),
            artboards: Vec::new(),
            layers: Vec::new(),
            swatches: Vec::new(),
            styles: Vec::new(),
            char_styles: Vec::new(),
            para_styles: Vec::new(),
            symbols: Vec::new(),
            guides: Vec::new(),
            grid: Grid::default(),
            perspective: None,
            animation: Animation::default(),
            next_id: 1,
        };
        let id = d.new_id();
        d.artboards.push(Artboard { id, name: "Artboard 1".into(), rect: Rect::new(0.0, 0.0, w, h), background: Some(Color::WHITE) });
        let id = d.new_id();
        let mut layer = Node::new(id, NodeKind::Group { children: Vec::new(), clip: false, layer: true });
        layer.name = "Layer 1".into();
        d.layers.push(layer);
        d
    }

    pub fn from_json(s: &str) -> Result<Document, String> {
        let mut d: Document = serde_json::from_str(s).map_err(|e| format!("not a vector document: {e}"))?;
        d.fix_ids();
        Ok(d)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    pub fn new_id(&mut self) -> NodeId {
        self.next_id = self.next_id.max(1);
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Gives every node and artboard a unique id (files edited by hand or
    /// written by other programs may repeat or omit them).
    pub fn fix_ids(&mut self) {
        let mut seen = std::collections::HashSet::new();
        let mut max = 0;
        self.walk(&mut |n| max = max.max(n.id));
        for a in &self.artboards {
            max = max.max(a.id);
        }
        self.next_id = self.next_id.max(max + 1);
        let mut next = self.next_id;
        for a in &mut self.artboards {
            if a.id == 0 || !seen.insert(a.id) {
                a.id = next;
                next += 1;
                seen.insert(a.id);
            }
        }
        fn fix(nodes: &mut [Node], seen: &mut std::collections::HashSet<NodeId>, next: &mut NodeId) {
            for n in nodes {
                if n.id == 0 || !seen.insert(n.id) {
                    n.id = *next;
                    *next += 1;
                    seen.insert(n.id);
                }
                if let Some(c) = n.children_mut() {
                    fix(c, seen, next);
                }
            }
        }
        fix(&mut self.layers, &mut seen, &mut next);
        self.next_id = next;
    }

    /// Visits every node (depth first, bottom to top).
    pub fn walk(&self, f: &mut dyn FnMut(&Node)) {
        fn go(nodes: &[Node], f: &mut dyn FnMut(&Node)) {
            for n in nodes {
                f(n);
                if let Some(c) = n.children() {
                    go(c, f);
                }
            }
        }
        go(&self.layers, f);
    }

    pub fn find(&self, id: NodeId) -> Option<&Node> {
        fn go(nodes: &[Node], id: NodeId) -> Option<&Node> {
            for n in nodes {
                if n.id == id {
                    return Some(n);
                }
                if let Some(c) = n.children()
                    && let Some(f) = go(c, id)
                {
                    return Some(f);
                }
            }
            None
        }
        go(&self.layers, id)
    }

    pub fn find_mut(&mut self, id: NodeId) -> Option<&mut Node> {
        fn go(nodes: &mut [Node], id: NodeId) -> Option<&mut Node> {
            for n in nodes {
                if n.id == id {
                    return Some(n);
                }
                if let Some(c) = n.children_mut()
                    && let Some(f) = go(c, id)
                {
                    return Some(f);
                }
            }
            None
        }
        go(&mut self.layers, id)
    }

    /// A node by id, or by name (the first match).
    pub fn lookup(&self, key: &str) -> Option<NodeId> {
        if let Ok(id) = key.trim_start_matches('#').parse::<NodeId>()
            && self.find(id).is_some()
        {
            return Some(id);
        }
        let mut hit = None;
        self.walk(&mut |n| {
            if hit.is_none() && n.name == key {
                hit = Some(n.id);
            }
        });
        hit
    }

    /// The id of the node's parent (`None` for top-level layers or unknown ids).
    pub fn parent_of(&self, id: NodeId) -> Option<NodeId> {
        let mut out = None;
        self.walk(&mut |n| {
            if let Some(c) = n.children()
                && c.iter().any(|k| k.id == id)
            {
                out = Some(n.id);
            }
        });
        out
    }

    /// The list holding `id` and its index there.
    pub fn siblings_mut(&mut self, id: NodeId) -> Option<(&mut Vec<Node>, usize)> {
        if let Some(i) = self.layers.iter().position(|n| n.id == id) {
            return Some((&mut self.layers, i));
        }
        let parent = self.parent_of(id)?;
        let list = self.find_mut(parent)?.children_mut()?;
        let i = list.iter().position(|n| n.id == id)?;
        Some((list, i))
    }

    /// Takes a node out of the tree.
    pub fn remove(&mut self, id: NodeId) -> Option<Node> {
        let (list, i) = self.siblings_mut(id)?;
        Some(list.remove(i))
    }

    /// The scene-space transform of a node: its own times every ancestor's.
    pub fn world_transform(&self, id: NodeId) -> Option<Affine> {
        fn go(nodes: &[Node], id: NodeId, m: Affine) -> Option<Affine> {
            for n in nodes {
                let mm = m * n.transform;
                if n.id == id {
                    return Some(mm);
                }
                if let Some(c) = n.children()
                    && let Some(f) = go(c, id, mm)
                {
                    return Some(f);
                }
            }
            None
        }
        go(&self.layers, id, Affine::IDENTITY)
    }

    /// The layer new art goes into: the topmost unlocked visible layer,
    /// made if there is none.
    pub fn active_layer(&mut self) -> NodeId {
        if let Some(l) = self.layers.iter().rev().find(|n| matches!(n.kind, NodeKind::Group { layer: true, .. }) && !n.locked && n.visible) {
            return l.id;
        }
        let id = self.new_id();
        let mut layer = Node::new(id, NodeKind::Group { children: Vec::new(), clip: false, layer: true });
        layer.name = format!("Layer {}", self.layers.len() + 1);
        self.layers.push(layer);
        id
    }

    /// Adds `node` on top of `parent`'s children (or the active layer).
    pub fn add(&mut self, parent: Option<NodeId>, mut node: Node) -> NodeId {
        if node.id == 0 || self.find(node.id).is_some() {
            node.id = self.new_id();
        }
        let id = node.id;
        self.next_id = self.next_id.max(id + 1);
        let parent = parent.filter(|p| self.find(*p).and_then(Node::children).is_some()).unwrap_or_else(|| self.active_layer());
        if let Some(c) = self.find_mut(parent).and_then(Node::children_mut) {
            c.push(node);
        }
        id
    }

    pub fn artboard(&self, key: Option<&str>) -> Option<&Artboard> {
        match key {
            None => self.artboards.first(),
            Some(k) => self
                .artboards
                .iter()
                .find(|a| a.name == k || a.id.to_string() == k.trim_start_matches('#'))
                .or_else(|| k.parse::<usize>().ok().and_then(|i| self.artboards.get(i.saturating_sub(1)))),
        }
    }

    /// The symbol with this id or name.
    pub fn symbol(&self, key: &str) -> Option<&Symbol> {
        self.symbols.iter().find(|s| s.id == key || s.name == key)
    }

    /// Re-ids every node in `nodes` (copies must not share ids with their originals).
    pub fn reid(&mut self, nodes: &mut [Node]) {
        for n in nodes {
            n.id = self.new_id();
            if let Some(c) = n.children_mut() {
                let mut taken = std::mem::take(c);
                self.reid(&mut taken);
                if let Some(c) = n.children_mut() {
                    *c = taken;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_document_has_an_artboard_and_a_layer() {
        let mut d = Document::new(800.0, 600.0);
        assert_eq!(d.artboards[0].rect, Rect::new(0.0, 0.0, 800.0, 600.0));
        let l = d.active_layer();
        assert_eq!(d.layers[0].id, l);
        let shape = Node::new(0, NodeKind::Shape { shape: LiveShape::Ellipse { rect: Rect::new(0.0, 0.0, 10.0, 10.0) } });
        let id = d.add(None, shape);
        assert_eq!(d.parent_of(id), Some(l));
        let back = Document::from_json(&d.to_json()).unwrap();
        assert_eq!(back, d);
        assert!(d.remove(id).is_some());
        assert!(d.find(id).is_none());
    }

    #[test]
    fn duplicate_ids_are_fixed() {
        let j = r#"{"layers":[{"id":5,"type":"group","layer":true,"children":[{"id":5,"type":"path","path":{"subpaths":[]}},{"id":0,"type":"path","path":{"subpaths":[]}}]}]}"#;
        let d = Document::from_json(j).unwrap();
        let mut ids = Vec::new();
        d.walk(&mut |n| ids.push(n.id));
        let mut u = ids.clone();
        u.sort();
        u.dedup();
        assert_eq!(u.len(), 3, "{ids:?}");
        assert!(d.next_id > *ids.iter().max().unwrap());
    }
}
