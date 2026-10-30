// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The editor's dock: every panel lives in a tree of splits and tab stacks,
//! the one source of truth for where things are.
//!
//! - A **split** lays its children side by side (`Row`) or stacked
//!   (`Column`). Each child has a [`Size`] along the split: fixed points, a
//!   ratio of the split, or `Fill` (shares what is left). That is how the
//!   default tree keeps today's layout exactly: the Library and Inspector
//!   keep their width as the window grows, the Viewer flexes, the Timeline
//!   is a share of the height until someone drags it.
//! - A **tab stack** holds one or more panels; one is showing.
//! - Any node can be **collapsed**: it keeps its place and size and takes no
//!   room (double-click a splitter, or close the only tab in a stack).
//!
//! [`Tree::layout`] turns the tree into tile and splitter rectangles for a
//! window, honouring each panel's minimum size; the edits ([`Tree::dock`],
//! [`Tree::move_tab`], [`Tree::close`], [`Tree::open`], [`Tree::drag`],
//! [`Tree::toggle`]) are pure and tested here. Drawing, dragging and the
//! menus are in `editor::dock_ui`.

use eframe::egui::{Rect, Vec2, pos2};
use serde::{Deserialize, Serialize};

/// A view that can be docked.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Panel {
    Library,
    Viewer,
    Inspector,
    Timeline,
    /// The mixer, delivery, dialogue and subtitles (`editor::mixer`).
    Sound,
    Scopes,
    Review,
    Record,
    /// The cutting room's lists: timeline index, checks, takes, sequences, script.
    Editorial,
    /// The Assistant: its conversations, steps and composer.
    Ai,
}

impl Panel {
    pub const ALL: [Panel; 10] = [Panel::Library, Panel::Viewer, Panel::Inspector, Panel::Timeline, Panel::Sound, Panel::Scopes, Panel::Review, Panel::Record, Panel::Editorial, Panel::Ai];

    /// The name people see (English; translated where it is shown).
    pub fn name(self) -> &'static str {
        match self {
            Panel::Library => "Library",
            Panel::Viewer => "Viewer",
            Panel::Inspector => "Inspector",
            Panel::Timeline => "Timeline",
            Panel::Sound => "Sound",
            Panel::Scopes => "Scopes",
            Panel::Review => "Review",
            Panel::Record => "Record",
            Panel::Editorial => "Editorial",
            Panel::Ai => "Assistant",
        }
    }

    /// The id used in saved layouts and by the AI's tools.
    pub fn key(self) -> &'static str {
        match self {
            Panel::Library => "library",
            Panel::Viewer => "viewer",
            Panel::Inspector => "inspector",
            Panel::Timeline => "timeline",
            Panel::Sound => "sound",
            Panel::Scopes => "scopes",
            Panel::Review => "review",
            Panel::Record => "record",
            Panel::Editorial => "editorial",
            Panel::Ai => "ai",
        }
    }

    /// A panel by its key or name, in any case (`mixer` is the Sound panel).
    pub fn parse(s: &str) -> Option<Panel> {
        let s = s.trim();
        if s.eq_ignore_ascii_case("mixer") {
            return Some(Panel::Sound);
        }
        if s.eq_ignore_ascii_case("agent") || s.eq_ignore_ascii_case("ai_editor") {
            return Some(Panel::Ai);
        }
        Panel::ALL.into_iter().find(|p| p.key().eq_ignore_ascii_case(s) || p.name().eq_ignore_ascii_case(s))
    }

    /// The smallest the panel can be and still work.
    pub fn min_size(self) -> Vec2 {
        use crate::editor::{INSPECTOR_MIN, LIBRARY_MIN, TIMELINE_MIN, TOP_MIN, VIEWER_MIN};
        match self {
            Panel::Library => Vec2::new(LIBRARY_MIN, 200.0),
            Panel::Viewer => Vec2::new(VIEWER_MIN, TOP_MIN),
            Panel::Inspector => Vec2::new(INSPECTOR_MIN, 200.0),
            Panel::Timeline => Vec2::new(320.0, TIMELINE_MIN),
            Panel::Sound => Vec2::new(360.0, TIMELINE_MIN),
            Panel::Scopes => Vec2::new(240.0, 180.0),
            Panel::Review => Vec2::new(300.0, 200.0),
            Panel::Record => Vec2::new(320.0, 220.0),
            Panel::Editorial => Vec2::new(320.0, 220.0),
            Panel::Ai => Vec2::new(300.0, 320.0),
        }
    }

    /// The size the panel asks for when it is docked beside something
    /// (it gets at most half of what it docks against).
    pub fn preferred_size(self) -> Vec2 {
        use crate::editor::{INSPECTOR_DEFAULT, LIBRARY_DEFAULT};
        match self {
            Panel::Library => Vec2::new(LIBRARY_DEFAULT, 320.0),
            Panel::Viewer => Vec2::new(800.0, 480.0),
            Panel::Inspector => Vec2::new(INSPECTOR_DEFAULT, 320.0),
            Panel::Timeline | Panel::Sound => Vec2::new(800.0, 320.0),
            Panel::Scopes => Vec2::new(320.0, 260.0),
            Panel::Review => Vec2::new(380.0, 320.0),
            Panel::Record => Vec2::new(420.0, 360.0),
            Panel::Editorial => Vec2::new(460.0, 520.0),
            Panel::Ai => Vec2::new(420.0, 640.0),
        }
    }

    /// The Viewer always stays: it is the picture, and toasts sit on it.
    pub fn closable(self) -> bool {
        self != Panel::Viewer
    }

    /// Panels that can also be floated in a window of their own, by choice
    /// ("Float in a window"): they open docked, never over the editor.
    pub fn floats(self) -> bool {
        matches!(self, Panel::Review | Panel::Record | Panel::Editorial)
    }

    /// Panels that can pop out into a window of the system's own (another
    /// screen, say). The Viewer stays: it is the picture the editor works on.
    pub fn pops_out(self) -> bool {
        self != Panel::Viewer
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Axis {
    /// Children side by side, left to right.
    Row,
    /// Children stacked, top to bottom.
    Column,
}

impl Axis {
    fn of(self, v: Vec2) -> f32 {
        match self {
            Axis::Row => v.x,
            Axis::Column => v.y,
        }
    }
    fn span(self, r: Rect) -> f32 {
        self.of(r.size())
    }
    fn start(self, r: Rect) -> f32 {
        match self {
            Axis::Row => r.left(),
            Axis::Column => r.top(),
        }
    }
    fn end(self, r: Rect) -> f32 {
        match self {
            Axis::Row => r.right(),
            Axis::Column => r.bottom(),
        }
    }
    /// The part of `r` from `a` to `b` along this axis.
    fn slice(self, r: Rect, a: f32, b: f32) -> Rect {
        match self {
            Axis::Row => Rect::from_min_max(pos2(a, r.top()), pos2(b, r.bottom())),
            Axis::Column => Rect::from_min_max(pos2(r.left(), a), pos2(r.right(), b)),
        }
    }
}

/// A child's length along its split.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Size {
    /// Points; stays as the window changes.
    Px(f32),
    /// A share of the split's whole length.
    Ratio(f32),
    /// What the fixed and ratio children leave (shared between fills).
    #[default]
    Fill,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Tabs {
        panels: Vec<Panel>,
        #[serde(default)]
        active: usize,
    },
    Split {
        axis: Axis,
        children: Vec<Node>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Node {
    #[serde(default)]
    pub size: Size,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub collapsed: bool,
    pub kind: Kind,
}

impl Node {
    pub fn tabs(panels: &[Panel], size: Size) -> Node {
        Node { size, collapsed: false, kind: Kind::Tabs { panels: panels.to_vec(), active: 0 } }
    }
    pub fn split(axis: Axis, size: Size, children: Vec<Node>) -> Node {
        Node { size, collapsed: false, kind: Kind::Split { axis, children } }
    }
    pub fn collapsed(mut self, c: bool) -> Node {
        self.collapsed = c;
        self
    }
    fn panels(&self, out: &mut Vec<Panel>) {
        match &self.kind {
            Kind::Tabs { panels, .. } => out.extend(panels),
            Kind::Split { children, .. } => children.iter().for_each(|c| c.panels(out)),
        }
    }
    fn at(&self, path: &[usize]) -> Option<&Node> {
        match path.split_first() {
            None => Some(self),
            Some((i, rest)) => match &self.kind {
                Kind::Split { children, .. } => children.get(*i)?.at(rest),
                Kind::Tabs { .. } => None,
            },
        }
    }
    fn at_mut(&mut self, path: &[usize]) -> Option<&mut Node> {
        match path.split_first() {
            None => Some(self),
            Some((i, rest)) => match &mut self.kind {
                Kind::Split { children, .. } => children.get_mut(*i)?.at_mut(rest),
                Kind::Tabs { .. } => None,
            },
        }
    }
    fn find(&self, p: Panel, path: &mut Vec<usize>) -> bool {
        match &self.kind {
            Kind::Tabs { panels, .. } => panels.contains(&p),
            Kind::Split { children, .. } => {
                for (i, c) in children.iter().enumerate() {
                    path.push(i);
                    if c.find(p, path) {
                        return true;
                    }
                    path.pop();
                }
                false
            }
        }
    }
    /// Whether anything in the node takes room (`hide` hides panels for now,
    /// such as the Inspector in a narrow window).
    fn visible(&self, hide: &dyn Fn(Panel) -> bool) -> bool {
        !self.collapsed
            && match &self.kind {
                Kind::Tabs { panels, .. } => panels.iter().any(|p| !hide(*p)),
                Kind::Split { children, .. } => children.iter().any(|c| c.visible(hide)),
            }
    }
    fn min_size(&self, m: &Metrics, hide: &dyn Fn(Panel) -> bool) -> Vec2 {
        match &self.kind {
            Kind::Tabs { panels, .. } => {
                let shown: Vec<Panel> = panels.iter().copied().filter(|p| !hide(*p)).collect();
                let mut v = shown.iter().fold(Vec2::ZERO, |a, p| a.max(p.min_size()));
                if shown.len() > 1 {
                    v.y += m.tab_h;
                }
                v
            }
            Kind::Split { axis, children } => {
                let vis: Vec<Vec2> = children.iter().filter(|c| c.visible(hide)).map(|c| c.min_size(m, hide)).collect();
                let gaps = vis.len().saturating_sub(1) as f32 * m.gap;
                match axis {
                    Axis::Row => Vec2::new(vis.iter().map(|v| v.x).sum::<f32>() + gaps, vis.iter().fold(0.0, |a, v| a.max(v.y))),
                    Axis::Column => Vec2::new(vis.iter().fold(0.0, |a, v| a.max(v.x)), vis.iter().map(|v| v.y).sum::<f32>() + gaps),
                }
            }
        }
    }
    /// Drops empty stacks and one-child splits, keeps each panel once and
    /// the active tab in range.
    fn normalize(&mut self, seen: &mut Vec<Panel>) {
        match &mut self.kind {
            Kind::Tabs { panels, active } => {
                let current = panels.get(*active).copied();
                panels.retain(|p| {
                    let fresh = !seen.contains(p);
                    seen.push(*p);
                    fresh
                });
                *active = current.and_then(|c| panels.iter().position(|p| *p == c)).unwrap_or(0).min(panels.len().saturating_sub(1));
            }
            Kind::Split { children, .. } => {
                for c in children.iter_mut() {
                    c.normalize(seen);
                }
                children.retain(|c| !c.is_empty());
                if children.len() == 1 {
                    let mut only = children.pop().expect("one child");
                    only.size = self.size;
                    only.collapsed |= self.collapsed;
                    *self = only;
                }
            }
        }
    }
    fn is_empty(&self) -> bool {
        match &self.kind {
            Kind::Tabs { panels, .. } => panels.is_empty(),
            Kind::Split { children, .. } => children.is_empty(),
        }
    }
}

/// Spacing the layout uses.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Metrics {
    /// Between tiles (the look's `GAP`).
    pub gap: f32,
    /// A tab strip's height, over stacks of two or more.
    pub tab_h: f32,
}

impl Default for Metrics {
    fn default() -> Self {
        Metrics { gap: crate::editor::GAP, tab_h: 0.0 }
    }
}

/// Where a dragged panel lands, relative to a target.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Zone {
    Left,
    Right,
    Top,
    Bottom,
    /// Into the target's stack, as a tab.
    Tab,
}

impl Zone {
    pub const ALL: [Zone; 5] = [Zone::Left, Zone::Right, Zone::Top, Zone::Bottom, Zone::Tab];
    pub fn parse(s: &str) -> Option<Zone> {
        match s.trim().to_ascii_lowercase().as_str() {
            "left" => Some(Zone::Left),
            "right" => Some(Zone::Right),
            "top" | "above" => Some(Zone::Top),
            "bottom" | "below" => Some(Zone::Bottom),
            "tab" | "center" | "centre" => Some(Zone::Tab),
            _ => None,
        }
    }
    /// Left and right swapped, for a right-to-left window (the tree keeps
    /// reading order; the screen shows it mirrored).
    pub fn mirrored(self) -> Zone {
        match self {
            Zone::Left => Zone::Right,
            Zone::Right => Zone::Left,
            z => z,
        }
    }
    fn axis(self) -> Option<(Axis, bool)> {
        match self {
            Zone::Left => Some((Axis::Row, true)),
            Zone::Right => Some((Axis::Row, false)),
            Zone::Top => Some((Axis::Column, true)),
            Zone::Bottom => Some((Axis::Column, false)),
            Zone::Tab => None,
        }
    }
}

/// One tab stack on screen.
#[derive(Clone, Debug, PartialEq)]
pub struct Tile {
    pub path: Vec<usize>,
    pub rect: Rect,
    /// The stack's panels that are showing tabs (all but hidden ones).
    pub panels: Vec<Panel>,
    pub active: Panel,
}

/// A splitter on screen: the gap between two children of a split, or the
/// edge where collapsed children wait to be dragged or double-clicked open.
#[derive(Clone, Debug, PartialEq)]
pub struct Splitter {
    /// The split.
    pub path: Vec<usize>,
    pub axis: Axis,
    pub rect: Rect,
    /// The visible children either side (`None` at an edge).
    pub before: Option<usize>,
    pub after: Option<usize>,
    /// Collapsed children between them (or past the edge).
    pub collapsed: Vec<usize>,
}

/// The tree laid out in a rectangle.
#[derive(Clone, Debug)]
pub struct Layout {
    pub tiles: Vec<Tile>,
    pub splitters: Vec<Splitter>,
    /// Every visible node's rectangle, by path.
    pub rects: Vec<(Vec<usize>, Rect)>,
    pub body: Rect,
    pub metrics: Metrics,
}

impl Layout {
    /// Nothing laid out yet (edits then size things from `body`).
    pub fn empty(body: Rect, metrics: Metrics) -> Layout {
        Layout { tiles: Vec::new(), splitters: Vec::new(), rects: Vec::new(), body, metrics }
    }
    pub fn rect(&self, path: &[usize]) -> Option<Rect> {
        self.rects.iter().find(|(p, _)| p == path).map(|(_, r)| *r)
    }
    pub fn tile_of(&self, p: Panel) -> Option<&Tile> {
        self.tiles.iter().find(|t| t.panels.contains(&p))
    }
    /// The panel's rectangle, if it is the one showing in its tile.
    pub fn panel_rect(&self, p: Panel) -> Option<Rect> {
        self.tiles.iter().find(|t| t.active == p).map(|t| t.rect)
    }
}

/// The lengths of a split's visible children: fixed and ratio sizes first
/// (never under their minimum), fills share the rest; when that leaves a
/// fill under its minimum the others give way, last first, down to theirs
/// and the fill takes what is left; when even their minimums do not fit,
/// everything shrinks in proportion.
pub fn distribute(span: f32, gap: f32, items: &[(Size, f32)]) -> Vec<f32> {
    let n = items.len();
    if n == 0 {
        return Vec::new();
    }
    let room = (span - gap * (n - 1) as f32).max(0.0);
    let mut fills: Vec<bool> = items.iter().map(|(s, _)| matches!(s, Size::Fill)).collect();
    if !fills.iter().any(|f| *f) {
        fills[n - 1] = true;
    }
    let mut len: Vec<f32> = items
        .iter()
        .zip(&fills)
        .map(|((s, min), fill)| match (fill, s) {
            (true, _) => 0.0,
            (false, Size::Px(p)) => p.max(*min),
            (false, Size::Ratio(r)) => (r * span).max(*min),
            (false, Size::Fill) => *min,
        })
        .collect();
    let fill_min: f32 = items.iter().zip(&fills).filter(|(_, f)| **f).map(|((_, m), _)| *m).sum();
    let fixed: f32 = len.iter().sum();
    let mut deficit = fill_min - (room - fixed);
    for i in (0..n).rev() {
        if deficit <= 0.0 {
            break;
        }
        if fills[i] {
            continue;
        }
        let give = (len[i] - items[i].1).max(0.0).min(deficit);
        len[i] -= give;
        deficit -= give;
    }
    let fixed: f32 = len.iter().sum();
    let free = room - fixed;
    let nfill = fills.iter().filter(|f| **f).count() as f32;
    if free >= fill_min {
        // Fills get their minimum and share the rest equally.
        let extra = free - fill_min;
        for i in 0..n {
            if fills[i] {
                len[i] = items[i].1 + extra / nfill;
            }
        }
    } else if free >= 0.0 {
        // The others are at their minimums: the fills take what is left,
        // under their own minimum (as the Viewer always did).
        for i in 0..n {
            if fills[i] {
                len[i] = if fill_min > 0.0 { items[i].1 / fill_min * free } else { free / nfill };
            }
        }
    } else {
        // Not even the others' minimums fit: everything shrinks in proportion.
        let total_min: f32 = items.iter().map(|(_, m)| *m).sum::<f32>().max(1e-3);
        for i in 0..n {
            len[i] = items[i].1 / total_min * room;
        }
    }
    len
}

/// A panel popped out of the editor into its own system window.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Popped {
    pub panel: Panel,
    /// The window's size in points (as the person left it).
    #[serde(default)]
    pub size: Option<[f32; 2]>,
    /// Where it docks back: beside (or with) this panel.
    #[serde(default)]
    pub back: Option<(Panel, Zone)>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Tree {
    pub root: Node,
    /// Panels closed out of a stack, and a panel they were stacked with, so
    /// reopening puts them back there.
    #[serde(default)]
    pub parked: Vec<(Panel, Panel)>,
    /// Panels in windows of their own.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub popped: Vec<Popped>,
}

impl Default for Tree {
    fn default() -> Self {
        Tree::legacy((true, crate::editor::LIBRARY_DEFAULT), (true, crate::editor::INSPECTOR_DEFAULT), (true, None))
    }
}

impl Tree {
    /// The fixed layout Studio had before docking: Library | Viewer |
    /// Inspector over the Timeline. Old saved layouts and workspaces become
    /// this tree with their widths, height and open flags.
    pub fn legacy(library: (bool, f32), inspector: (bool, f32), timeline: (bool, Option<f32>)) -> Tree {
        use crate::editor::TIMELINE_DEFAULT_FRAC;
        let top = Node::split(
            Axis::Row,
            Size::Fill,
            vec![
                Node::tabs(&[Panel::Library], Size::Px(library.1)).collapsed(!library.0),
                Node::tabs(&[Panel::Viewer], Size::Fill),
                Node::tabs(&[Panel::Inspector], Size::Px(inspector.1)).collapsed(!inspector.0),
            ],
        );
        let tl = timeline.1.map_or(Size::Ratio(TIMELINE_DEFAULT_FRAC), Size::Px);
        Tree { root: Node::split(Axis::Column, Size::Fill, vec![top, Node::tabs(&[Panel::Timeline], tl).collapsed(!timeline.0)]), parked: Vec::new(), popped: Vec::new() }
    }

    /// The tree for an old `Layout` pref (no tree saved yet).
    pub fn from_layout(l: &crate::prefs::Layout) -> Tree {
        use crate::editor::{INSPECTOR_DEFAULT, LIBRARY_DEFAULT};
        Tree::legacy(
            (l.library_open.unwrap_or(true), l.library_w.unwrap_or(LIBRARY_DEFAULT)),
            (l.inspector_open.unwrap_or(true), l.inspector_w.unwrap_or(INSPECTOR_DEFAULT)),
            (l.timeline_open.unwrap_or(true), l.timeline_h),
        )
    }

    /// A tree read from a file made sound: each panel once, no empty stacks,
    /// the Viewer present (or the default tree when it is not).
    pub fn sanitized(mut self) -> Tree {
        self.normalize();
        if self.find(Panel::Viewer).is_none() {
            return Tree::default();
        }
        self
    }

    fn normalize(&mut self) {
        let mut seen = Vec::new();
        self.root.normalize(&mut seen);
        self.root.collapsed = false;
        let present = self.panels();
        // Docked wins over popped out; each panel in one window.
        self.popped.retain(|w| !present.contains(&w.panel) && w.panel.pops_out());
        let mut out: Vec<Panel> = Vec::new();
        self.popped.retain(|w| {
            let fresh = !out.contains(&w.panel);
            out.push(w.panel);
            fresh
        });
        self.parked.retain(|(p, _)| !present.contains(p) && !out.contains(p));
        let mut keep: Vec<Panel> = Vec::new();
        self.parked.retain(|(p, _)| {
            let fresh = !keep.contains(p);
            keep.push(*p);
            fresh
        });
    }

    /// Every docked panel, in tree order.
    pub fn panels(&self) -> Vec<Panel> {
        let mut v = Vec::new();
        self.root.panels(&mut v);
        v
    }

    /// The path to the stack holding `p`.
    pub fn find(&self, p: Panel) -> Option<Vec<usize>> {
        let mut path = Vec::new();
        self.root.find(p, &mut path).then_some(path)
    }

    pub fn contains(&self, p: Panel) -> bool {
        self.find(p).is_some()
    }

    pub fn node(&self, path: &[usize]) -> Option<&Node> {
        self.root.at(path)
    }

    /// Docked and not collapsed (it may be a tab behind another).
    pub fn is_open(&self, p: Panel) -> bool {
        let Some(path) = self.find(p) else { return false };
        (0..=path.len()).all(|k| self.root.at(&path[..k]).is_some_and(|n| !n.collapsed))
    }

    /// Open and the tab showing in its stack.
    pub fn is_showing(&self, p: Panel) -> bool {
        self.is_open(p) && self.active_in_stack(p) == Some(p)
    }

    fn active_in_stack(&self, p: Panel) -> Option<Panel> {
        match &self.root.at(&self.find(p)?)?.kind {
            Kind::Tabs { panels, active } => panels.get(*active).copied(),
            Kind::Split { .. } => None,
        }
    }

    /// Shows `p`'s tab in its stack.
    pub fn activate(&mut self, p: Panel) -> bool {
        let Some(path) = self.find(p) else { return false };
        if let Some(Node { kind: Kind::Tabs { panels, active }, .. }) = self.root.at_mut(&path) {
            *active = panels.iter().position(|q| *q == p).unwrap_or(0);
        }
        true
    }

    /// Takes `p` out of the tree (its place closes up).
    pub fn remove(&mut self, p: Panel) -> bool {
        let Some(path) = self.find(p) else { return false };
        if let Some(Node { kind: Kind::Tabs { panels, active }, .. }) = self.root.at_mut(&path) {
            let current = panels.get(*active).copied();
            panels.retain(|q| *q != p);
            *active = current.and_then(|c| panels.iter().position(|q| *q == c)).unwrap_or(0);
        }
        self.normalize();
        true
    }

    /// Closes `p`: out of a stack with other tabs (remembered for reopening
    /// there), or its stack collapsed when it is alone. The Viewer stays.
    pub fn close(&mut self, p: Panel) -> bool {
        if !p.closable() {
            return false;
        }
        if let Some(w) = self.popped.iter().position(|w| w.panel == p) {
            // Its window closes; reopening docks it where it came from.
            let w = self.popped.remove(w);
            if let Some((n, Zone::Tab)) = w.back {
                self.parked.push((p, n));
            }
            return true;
        }
        let Some(path) = self.find(p) else { return false };
        let Some(node) = self.root.at_mut(&path) else { return false };
        let Kind::Tabs { panels, .. } = &node.kind else { return false };
        if panels.len() == 1 {
            if path.is_empty() {
                return false;
            }
            node.collapsed = true;
            return true;
        }
        let neighbour = panels.iter().copied().find(|q| *q != p);
        self.remove(p);
        if let Some(n) = neighbour {
            self.parked.push((p, n));
        }
        true
    }

    /// Opens `p`: uncollapses it where it is; back in the stack it was
    /// closed out of; or docked where it belongs by default. `lay` is the
    /// tree's current layout (for sizes).
    pub fn open(&mut self, p: Panel, lay: &Layout) -> bool {
        if self.is_popped(p) {
            return true;
        }
        if let Some(path) = self.find(p) {
            for k in 0..=path.len() {
                if let Some(n) = self.root.at_mut(&path[..k]) {
                    n.collapsed = false;
                }
            }
            return self.activate(p);
        }
        let parked = self.parked.iter().find(|(q, _)| *q == p).map(|(_, n)| *n);
        self.parked.retain(|(q, _)| *q != p);
        if let Some(n) = parked.filter(|n| self.is_open(*n)) {
            return self.dock(p, Some(n), Zone::Tab, lay);
        }
        let (target, zone) = match p {
            Panel::Library => (Some(Panel::Viewer), Zone::Left),
            Panel::Timeline => (None, Zone::Bottom),
            // The mixer beside the timeline, never in its place: levels are
            // set while the clips go by.
            Panel::Sound if self.is_open(Panel::Timeline) => (Some(Panel::Timeline), Zone::Right),
            Panel::Sound => (None, Zone::Bottom),
            // The browser zone holds the tools that bring things in: the AI
            // editor, the cutting room's lists and Record (the AI editor used
            // to be a Library section).
            Panel::Ai | Panel::Editorial | Panel::Record if self.is_open(Panel::Library) => (Some(Panel::Library), Zone::Tab),
            Panel::Ai | Panel::Editorial | Panel::Record => (Some(Panel::Viewer), Zone::Left),
            // Review notes, versions and the team join the Inspector's zone.
            Panel::Review if self.is_open(Panel::Inspector) => (Some(Panel::Inspector), Zone::Tab),
            // Scopes beside the picture (the Viewer keeps its minimum width).
            _ => (Some(Panel::Viewer), Zone::Right),
        };
        self.dock(p, target, zone, lay)
    }

    /// Opens or closes `p`.
    pub fn set_open(&mut self, p: Panel, open: bool, lay: &Layout) -> bool {
        if open { self.open(p, lay) } else { self.close(p) }
    }

    /// How long a panel docked at `zone` of something `target_len` long is:
    /// half of it at most, the panel's preferred size when that is less, and
    /// never under its minimum.
    pub fn edge_len(p: Panel, axis: Axis, target_len: f32, target_min: f32, gap: f32) -> f32 {
        let half = ((target_len - gap) / 2.0).max(0.0);
        let spare = (target_len - gap - target_min).max(0.0);
        axis.of(p.preferred_size()).min(half).min(spare).max(axis.of(p.min_size()).min(half))
    }

    /// Where `p` would land if dropped at `zone` of `target`: the tree
    /// docked on a copy and laid out the way `lay` was, so the preview is
    /// exactly the result (others giving way included).
    pub fn preview(&self, p: Panel, target: Option<Panel>, zone: Zone, lay: &Layout, hide: &dyn Fn(Panel) -> bool) -> Option<Rect> {
        let mut t = self.clone();
        if !t.dock(p, target, zone, lay) {
            return None;
        }
        t.layout(lay.body, lay.metrics, hide).tile_of(p).map(|tile| tile.rect)
    }

    /// Docks `p` at `zone` of the stack holding `target` (or of the whole
    /// window when `target` is `None`), taking it from wherever it was.
    pub fn dock(&mut self, p: Panel, target: Option<Panel>, zone: Zone, lay: &Layout) -> bool {
        if target == Some(p) {
            // Onto its own stack: only meaningful when there are others in it.
            let Some(path) = self.find(p) else { return false };
            let others = match &self.root.at(&path).map(|n| &n.kind) {
                Some(Kind::Tabs { panels, .. }) => panels.iter().copied().find(|q| *q != p),
                _ => None,
            };
            return match others {
                Some(o) if zone != Zone::Tab => self.dock(p, Some(o), zone, lay),
                _ => false,
            };
        }
        let gap = lay.metrics.gap;
        // Sizes come from the layout before anything moves.
        let target_path = match target {
            Some(t) => match self.find(t) {
                Some(path) => path,
                None => return false,
            },
            None => Vec::new(),
        };
        let target_rect = lay.rect(&target_path).unwrap_or(lay.body);
        let Some((axis, before)) = zone.axis() else {
            let Some(t) = target else { return false };
            self.remove(p);
            self.popped.retain(|w| w.panel != p);
            let Some(path) = self.find(t) else { return false };
            if let Some(Node { kind: Kind::Tabs { panels, active }, .. }) = self.root.at_mut(&path) {
                panels.push(p);
                *active = panels.len() - 1;
            }
            self.parked.retain(|(q, _)| *q != p);
            return true;
        };
        let parent_rect = target_path.split_last().and_then(|(_, up)| lay.rect(up));
        let target_min = match target {
            Some(_) => self.root.at(&target_path).map_or(0.0, |n| axis.of(n.min_size(&lay.metrics, &|_| false))),
            None => 0.0,
        };
        let len = Tree::edge_len(p, axis, axis.span(target_rect), target_min, gap);
        self.remove(p);
        self.parked.retain(|(q, _)| *q != p);
        self.popped.retain(|w| w.panel != p);
        // The target's path again: removing `p` may have moved it.
        let path = match target {
            Some(t) => match self.find(t) {
                Some(path) => path,
                None => return false,
            },
            None => Vec::new(),
        };
        let parent_axis = path.split_last().and_then(|(_, up)| match &self.root.at(up)?.kind {
            Kind::Split { axis, .. } => Some(*axis),
            Kind::Tabs { .. } => None,
        });
        let fresh = |size| Node::tabs(&[p], size);
        let root_same_axis = target.is_none() && matches!(&self.root.kind, Kind::Split { axis: a, .. } if *a == axis);
        if parent_axis == Some(axis) || root_same_axis {
            // A sibling in the same split.
            let (split_path, index) = if root_same_axis { (Vec::new(), None) } else { (path[..path.len() - 1].to_vec(), Some(path[path.len() - 1])) };
            let span = parent_rect.filter(|_| !root_same_axis).map_or(axis.span(target_rect), |r| axis.span(r)).max(1.0);
            let target_len = axis.span(target_rect);
            let Some(split) = self.root.at_mut(&split_path) else { return false };
            let Kind::Split { children, .. } = &mut split.kind else { return false };
            let new_size = match index.map(|i| children[i].size) {
                Some(Size::Px(_)) => Size::Px(len),
                _ => Size::Ratio(len / span),
            };
            if let Some(i) = index {
                // The target gives up what the new panel takes.
                children[i].size = match children[i].size {
                    Size::Px(_) => Size::Px((target_len - len - gap).max(0.0)),
                    Size::Ratio(_) => Size::Ratio(((target_len - len - gap) / span).max(0.0)),
                    Size::Fill => Size::Fill,
                };
            }
            let at = match (index, before) {
                (Some(i), true) => i,
                (Some(i), false) => i + 1,
                (None, true) => 0,
                (None, false) => children.len(),
            };
            children.insert(at, fresh(new_size));
        } else {
            // Wrap the target in a new split across the other way.
            let Some(slot) = self.root.at_mut(&path) else { return false };
            let mut old = std::mem::replace(slot, Node::tabs(&[], Size::Fill));
            let size = old.size;
            old.size = Size::Fill;
            old.collapsed = false;
            let new = fresh(Size::Ratio(len / axis.span(target_rect).max(1.0)));
            let children = if before { vec![new, old] } else { vec![old, new] };
            *slot = Node::split(axis, size, children);
        }
        self.normalize();
        true
    }

    /// Moves `p` to `index` among the tabs of the stack holding `target`
    /// (reordering when it is already there), and shows it.
    pub fn move_tab(&mut self, p: Panel, target: Panel, index: usize) -> bool {
        let Some(target_path) = self.find(target) else { return false };
        let mut index = index;
        if self.find(p).as_ref() == Some(&target_path) {
            // A reorder within the stack.
            if let Some(Node { kind: Kind::Tabs { panels, active }, .. }) = self.root.at_mut(&target_path) {
                let from = panels.iter().position(|q| *q == p).unwrap_or(0);
                if from < index {
                    index -= 1;
                }
                panels.remove(from);
                let i = index.min(panels.len());
                panels.insert(i, p);
                *active = i;
            }
            return true;
        }
        self.remove(p);
        self.parked.retain(|(q, _)| *q != p);
        self.popped.retain(|w| w.panel != p);
        let Some(path) = self.find(target) else { return false };
        if let Some(Node { kind: Kind::Tabs { panels, active }, .. }) = self.root.at_mut(&path) {
            let i = index.min(panels.len());
            panels.insert(i, p);
            *active = i;
        }
        self.normalize();
        true
    }

    /// Lays the tree out in `body`. `hide` keeps panels out for now (the
    /// Inspector becomes a sheet in a narrow window).
    pub fn layout(&self, body: Rect, m: Metrics, hide: &dyn Fn(Panel) -> bool) -> Layout {
        let mut out = Layout::empty(body, m);
        let mut path = Vec::new();
        if self.root.visible(hide) {
            place(&self.root, &mut path, body, m, hide, &mut out);
        }
        out
    }

    /// A splitter dragged by `delta` points (along its axis, toward the
    /// child after it). Children keep the kind of size they had.
    pub fn drag(&mut self, s: &Splitter, delta: f32, lay: &Layout, hide: &dyn Fn(Panel) -> bool) -> bool {
        let m = lay.metrics;
        let Some(split_rect) = lay.rect(&s.path) else { return false };
        let span = s.axis.span(split_rect).max(1.0);
        let child_rect = |i: usize| {
            let mut p = s.path.clone();
            p.push(i);
            lay.rect(&p)
        };
        let Some(split) = self.root.at_mut(&s.path) else { return false };
        let Kind::Split { children, .. } = &mut split.kind else { return false };
        let set = |n: &mut Node, len: f32| {
            n.size = match n.size {
                Size::Px(_) => Size::Px(len),
                Size::Ratio(_) => Size::Ratio(len / span),
                Size::Fill => Size::Fill,
            }
        };
        match (s.before, s.after) {
            (Some(a), Some(b)) => {
                let (Some(ra), Some(rb)) = (child_rect(a), child_rect(b)) else { return false };
                let (la, lb) = (s.axis.span(ra), s.axis.span(rb));
                let (ma, mb) = (s.axis.of(children[a].min_size(&m, hide)), s.axis.of(children[b].min_size(&m, hide)));
                let d = delta.clamp(-(la - ma).max(0.0), (lb - mb).max(0.0));
                if d == 0.0 {
                    return false;
                }
                if matches!(children[a].size, Size::Fill) && matches!(children[b].size, Size::Fill) {
                    children[a].size = Size::Ratio((la + d) / span);
                } else {
                    set(&mut children[a], la + d);
                    set(&mut children[b], lb - d);
                }
                true
            }
            _ => {
                // An edge: dragging opens what is collapsed there, at its minimum.
                if delta == 0.0 {
                    return false;
                }
                for &i in &s.collapsed {
                    children[i].collapsed = false;
                    let min = s.axis.of(children[i].min_size(&m, hide));
                    set(&mut children[i], min);
                }
                true
            }
        }
    }

    /// A splitter double-clicked: opens what is collapsed at it, or else
    /// collapses the neighbour that is not the one filling the space.
    pub fn toggle(&mut self, s: &Splitter) -> bool {
        let Some(split) = self.root.at_mut(&s.path) else { return false };
        let Kind::Split { children, .. } = &mut split.kind else { return false };
        if !s.collapsed.is_empty() {
            for &i in &s.collapsed {
                children[i].collapsed = false;
            }
            return true;
        }
        let (Some(a), Some(b)) = (s.before, s.after) else { return false };
        let pick = if matches!(children[b].size, Size::Fill) && !matches!(children[a].size, Size::Fill) { a } else { b };
        // The Viewer's stack never collapses.
        let mut ps = Vec::new();
        children[pick].panels(&mut ps);
        if ps.contains(&Panel::Viewer) {
            return false;
        }
        children[pick].collapsed = true;
        true
    }

    /// Whether `p` is in a window of its own.
    pub fn is_popped(&self, p: Panel) -> bool {
        self.popped.iter().any(|w| w.panel == p)
    }

    /// Open anywhere: docked and not collapsed, or popped out.
    pub fn is_shown_anywhere(&self, p: Panel) -> bool {
        self.is_open(p) || self.is_popped(p)
    }

    /// Where `p` would dock back to if it left: with the others in its stack,
    /// or beside its neighbour in the split.
    fn anchor(&self, p: Panel) -> Option<(Panel, Zone)> {
        let path = self.find(p)?;
        if let Kind::Tabs { panels, .. } = &self.root.at(&path)?.kind
            && let Some(o) = panels.iter().copied().find(|q| *q != p)
        {
            return Some((o, Zone::Tab));
        }
        let (i, up) = path.split_last()?;
        let Kind::Split { axis, children } = &self.root.at(up)?.kind else { return None };
        let first = |n: &Node| {
            let mut v = Vec::new();
            n.panels(&mut v);
            v.first().copied()
        };
        let (after, before) = match axis {
            Axis::Row => (Zone::Right, Zone::Left),
            Axis::Column => (Zone::Bottom, Zone::Top),
        };
        if *i > 0 {
            return first(&children[i - 1]).map(|q| (q, after));
        }
        children.get(1).and_then(first).map(|q| (q, before))
    }

    /// Takes `p` out of the editor into a window of its own, as big as it
    /// was on screen (at least its minimum), remembering where it was.
    pub fn pop_out(&mut self, p: Panel, lay: &Layout) -> bool {
        if !p.pops_out() || self.is_popped(p) {
            return false;
        }
        let size = lay.tile_of(p).map(|t| t.rect.size()).unwrap_or_else(|| p.preferred_size()).max(p.min_size());
        let back = self.anchor(p);
        self.remove(p);
        self.parked.retain(|(q, _)| *q != p);
        self.popped.push(Popped { panel: p, size: Some([size.x, size.y]), back });
        true
    }

    /// Brings a popped-out panel back into the editor where it came from
    /// (or where it goes by default when that is gone).
    pub fn dock_back(&mut self, p: Panel, lay: &Layout) -> bool {
        let Some(i) = self.popped.iter().position(|w| w.panel == p) else { return false };
        let w = self.popped.remove(i);
        if let Some((n, z)) = w.back.filter(|(n, _)| self.is_open(*n))
            && self.dock(p, Some(n), z, lay)
        {
            return true;
        }
        self.open(p, lay)
    }

    /// Remembers a popped-out window's size (when the person resizes it).
    pub fn set_popped_size(&mut self, p: Panel, size: Vec2) {
        if let Some(w) = self.popped.iter_mut().find(|w| w.panel == p) {
            let old = w.size.map_or(Vec2::ZERO, |s| Vec2::new(s[0], s[1]));
            if (old - size).length() > 1.0 {
                w.size = Some([size.x, size.y]);
            }
        }
    }

    /// The old fields a workspace file kept (for older Studio versions
    /// reading it): open flags and sizes of the three side panels.
    pub fn legacy_fields(&self) -> ((bool, f32), (bool, f32), (bool, Option<f32>)) {
        use crate::editor::{INSPECTOR_DEFAULT, LIBRARY_DEFAULT};
        let px = |p: Panel| {
            self.find(p).and_then(|path| match self.root.at(&path)?.size {
                Size::Px(v) => Some(v),
                _ => None,
            })
        };
        (
            (self.is_open(Panel::Library), px(Panel::Library).unwrap_or(LIBRARY_DEFAULT)),
            (self.is_open(Panel::Inspector), px(Panel::Inspector).unwrap_or(INSPECTOR_DEFAULT)),
            (self.is_open(Panel::Timeline), px(Panel::Timeline)),
        )
    }
}

fn place(node: &Node, path: &mut Vec<usize>, r: Rect, m: Metrics, hide: &dyn Fn(Panel) -> bool, out: &mut Layout) {
    out.rects.push((path.clone(), r));
    match &node.kind {
        Kind::Tabs { panels, active } => {
            let shown: Vec<Panel> = panels.iter().copied().filter(|p| !hide(*p)).collect();
            let want = panels.get(*active).copied().filter(|p| !hide(*p));
            let Some(first) = shown.first().copied() else { return };
            out.tiles.push(Tile { path: path.clone(), rect: r, active: want.unwrap_or(first), panels: shown });
        }
        Kind::Split { axis, children } => {
            let axis = *axis;
            let vis: Vec<usize> = (0..children.len()).filter(|i| children[*i].visible(hide)).collect();
            let items: Vec<(Size, f32)> = vis.iter().map(|i| (children[*i].size, axis.of(children[*i].min_size(&m, hide)))).collect();
            let lens = distribute(axis.span(r), m.gap, &items);
            let mut at = axis.start(r);
            let mut rects: Vec<Rect> = Vec::with_capacity(vis.len());
            for (k, i) in vis.iter().enumerate() {
                let end = if k + 1 == vis.len() { axis.end(r) } else { at + lens[k] };
                let cr = axis.slice(r, at, end.max(at));
                rects.push(cr);
                path.push(*i);
                place(&children[*i], path, cr, m, hide, out);
                path.pop();
                at = end + m.gap;
            }
            // Splitters: between each visible pair (with any collapsed
            // children between them), and at either end where collapsed
            // children wait.
            let collapsed_in = |lo: usize, hi: usize| -> Vec<usize> { (lo..hi).filter(|i| !children[*i].visible(hide) && children[*i].collapsed).collect() };
            let line = |a: f32, b: f32| axis.slice(r, a, b);
            for k in 0..vis.len() {
                if k + 1 < vis.len() {
                    let (a, b) = (vis[k], vis[k + 1]);
                    out.splitters.push(Splitter {
                        path: path.clone(),
                        axis,
                        rect: line(axis.end(rects[k]), axis.start(rects[k + 1])),
                        before: Some(a),
                        after: Some(b),
                        collapsed: collapsed_in(a + 1, b),
                    });
                }
            }
            if let (Some(&first), Some(&last)) = (vis.first(), vis.last()) {
                let lead = collapsed_in(0, first);
                if !lead.is_empty() {
                    let s = axis.start(r);
                    out.splitters.push(Splitter { path: path.clone(), axis, rect: line(s - m.gap, s), before: None, after: Some(first), collapsed: lead });
                }
                let trail = collapsed_in(last + 1, children.len());
                if !trail.is_empty() {
                    let e = axis.end(r);
                    out.splitters.push(Splitter { path: path.clone(), axis, rect: line(e, e + m.gap), before: Some(last), after: None, collapsed: trail });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::{GAP, INSPECTOR_DEFAULT, INSPECTOR_MIN, LIBRARY_DEFAULT, LIBRARY_MIN, TIMELINE_DEFAULT_FRAC, TIMELINE_MIN, TOP_MIN, VIEWER_MIN};

    const M: Metrics = Metrics { gap: GAP, tab_h: 30.0 };
    fn none(_: Panel) -> bool {
        false
    }
    fn body(w: f32, h: f32) -> Rect {
        Rect::from_min_size(pos2(GAP, 40.0), Vec2::new(w, h))
    }
    fn lay(t: &Tree, w: f32, h: f32) -> Layout {
        t.layout(body(w, h), M, &none)
    }
    fn rect(l: &Layout, p: Panel) -> Rect {
        l.tile_of(p).unwrap_or_else(|| panic!("{p:?} on screen")).rect
    }
    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 0.01
    }

    /// The fixed layout before docking, computed the way the editor used to.
    fn old_regions(b: Rect, lib: Option<f32>, insp: Option<f32>, tl: Option<Option<f32>>) -> (Option<Rect>, Rect, Option<Rect>, Option<Rect>) {
        let main = b;
        let timeline = tl.map(|h| {
            let h = h.unwrap_or(main.height() * TIMELINE_DEFAULT_FRAC).clamp(TIMELINE_MIN, (main.height() - TOP_MIN - GAP).max(TIMELINE_MIN));
            Rect::from_min_max(pos2(main.left(), main.bottom() - h), main.max)
        });
        let top = Rect::from_min_max(main.min, pos2(main.right(), timeline.map_or(main.bottom(), |t| t.top() - GAP)));
        let mut lib_w = lib.map_or(0.0, |w| w.max(LIBRARY_MIN));
        let mut insp_w = insp.map_or(0.0, |w| w.max(INSPECTOR_MIN));
        let gaps = (lib_w > 0.0) as u8 as f32 * GAP + (insp_w > 0.0) as u8 as f32 * GAP;
        let over = lib_w + insp_w + gaps + VIEWER_MIN - top.width();
        if over > 0.0 {
            let ti = over.min((insp_w - INSPECTOR_MIN).max(0.0));
            insp_w -= ti;
            let tlb = (over - ti).min((lib_w - LIBRARY_MIN).max(0.0));
            lib_w -= tlb;
        }
        let library = (lib_w > 0.0).then(|| Rect::from_min_size(top.min, Vec2::new(lib_w, top.height())));
        let inspector = (insp_w > 0.0).then(|| Rect::from_min_max(pos2(top.right() - insp_w, top.top()), top.max));
        let vl = library.map_or(top.left(), |r| r.right() + GAP);
        let vr = inspector.map_or(top.right(), |r| r.left() - GAP);
        (library, Rect::from_min_max(pos2(vl, top.top()), pos2(vr.max(vl + 1.0), top.bottom())), inspector, timeline)
    }

    fn same(a: Rect, b: Rect) -> bool {
        close(a.min.x, b.min.x) && close(a.min.y, b.min.y) && close(a.max.x, b.max.x) && close(a.max.y, b.max.y)
    }

    #[test]
    fn the_default_tree_is_the_old_layout_at_every_size() {
        for (w, h) in [(1434.0, 857.0), (954.0, 557.0), (1914.0, 1037.0), (2554.0, 1397.0), (1200.0, 700.0), (1000.0, 640.0)] {
            let b = body(w, h);
            let (lib, viewer, insp, tl) = old_regions(b, Some(LIBRARY_DEFAULT), Some(INSPECTOR_DEFAULT), Some(None));
            let l = Tree::default().layout(b, M, &none);
            assert!(same(rect(&l, Panel::Library), lib.unwrap()), "library at {w}x{h}: {:?} vs {:?}", rect(&l, Panel::Library), lib);
            assert!(same(rect(&l, Panel::Viewer), viewer), "viewer at {w}x{h}: {:?} vs {viewer:?}", rect(&l, Panel::Viewer));
            assert!(same(rect(&l, Panel::Inspector), insp.unwrap()), "inspector at {w}x{h}");
            assert!(same(rect(&l, Panel::Timeline), tl.unwrap()), "timeline at {w}x{h}: {:?} vs {:?}", rect(&l, Panel::Timeline), tl);
        }
    }

    #[test]
    fn old_prefs_become_the_same_layout() {
        let old = crate::prefs::Layout { library_w: Some(400.0), inspector_w: Some(330.0), timeline_h: Some(260.0), library_open: Some(false), inspector_open: Some(true), timeline_open: Some(true) };
        let t = Tree::from_layout(&old);
        let b = body(1434.0, 857.0);
        let (lib, viewer, insp, tl) = old_regions(b, None, Some(330.0), Some(Some(260.0)));
        let l = t.layout(b, M, &none);
        assert!(lib.is_none() && l.tile_of(Panel::Library).is_none());
        assert!(same(rect(&l, Panel::Viewer), viewer));
        assert!(same(rect(&l, Panel::Inspector), insp.unwrap()));
        assert!(same(rect(&l, Panel::Timeline), tl.unwrap()));
        assert!(!t.is_open(Panel::Library) && t.is_open(Panel::Inspector));
        // And the old fields read back for an older Studio.
        assert_eq!(t.legacy_fields(), ((false, 400.0), (true, 330.0), (true, Some(260.0))));
    }

    #[test]
    fn distribute_keeps_minimums_and_gives_way_last_first() {
        // Room for everything: fixed sizes, the fill takes the rest.
        let v = distribute(1000.0, 3.0, &[(Size::Px(300.0), 260.0), (Size::Fill, 420.0), (Size::Px(200.0), 180.0)]);
        assert_eq!(v, vec![300.0, 494.0, 200.0]);
        // Too narrow: the last fixed child gives way to its minimum first.
        let v = distribute(900.0, 3.0, &[(Size::Px(300.0), 260.0), (Size::Fill, 420.0), (Size::Px(250.0), 180.0)]);
        assert_eq!(v, vec![294.0, 420.0, 180.0]);
        // The fill keeps what is left once the others are at their minimum;
        // when they do not fit either, everything shrinks in proportion.
        let v = distribute(503.0, 3.0, &[(Size::Px(300.0), 250.0), (Size::Fill, 250.0)]);
        assert_eq!(v, vec![250.0, 250.0]);
        let v = distribute(403.0, 3.0, &[(Size::Px(300.0), 250.0), (Size::Fill, 250.0)]);
        assert_eq!(v, vec![250.0, 150.0]);
        let v = distribute(203.0, 3.0, &[(Size::Px(300.0), 250.0), (Size::Fill, 250.0)]);
        assert_eq!(v, vec![100.0, 100.0]);
        // A ratio is of the whole span; no fill: the last child fills.
        let v = distribute(1000.0, 0.0, &[(Size::Ratio(0.25), 10.0), (Size::Px(100.0), 10.0)]);
        assert_eq!(v, vec![250.0, 750.0]);
        // Two fills share.
        let v = distribute(1003.0, 3.0, &[(Size::Fill, 100.0), (Size::Fill, 300.0)]);
        assert_eq!(v, vec![400.0, 600.0]);
    }

    #[test]
    fn docking_to_each_side_splits_the_target() {
        // A wide window, where there is room beside the Viewer.
        let (w, h) = (1914.0, 1037.0);
        let base = Tree::default();
        let l = lay(&base, w, h);
        let v = rect(&l, Panel::Viewer);
        for zone in [Zone::Left, Zone::Right, Zone::Top, Zone::Bottom] {
            let mut t = base.clone();
            let preview = t.preview(Panel::Scopes, Some(Panel::Viewer), zone, &l, &none).unwrap();
            assert!(t.dock(Panel::Scopes, Some(Panel::Viewer), zone, &l), "{zone:?}");
            let l2 = lay(&t, w, h);
            let s = rect(&l2, Panel::Scopes);
            let nv = rect(&l2, Panel::Viewer);
            match zone {
                Zone::Left => assert!(s.right() < nv.left() && close(s.left(), v.left())),
                Zone::Right => assert!(s.left() > nv.right() && close(s.right(), v.right())),
                Zone::Top => assert!(s.bottom() < nv.top() && close(s.top(), v.top())),
                Zone::Bottom => assert!(s.top() > nv.bottom() && close(s.bottom(), v.bottom())),
                Zone::Tab => unreachable!(),
            }
            assert!(same(s, preview), "{zone:?}: {s:?} vs preview {preview:?}");
            // Nobody else moved.
            assert!(same(rect(&l2, Panel::Library), rect(&l, Panel::Library)));
            assert!(same(rect(&l2, Panel::Timeline), rect(&l, Panel::Timeline)));
        }
        // A smaller window: the others give way, and the preview says so.
        let l = lay(&base, 1434.0, 857.0);
        let mut t = base.clone();
        let preview = t.preview(Panel::Scopes, Some(Panel::Viewer), Zone::Left, &l, &none).unwrap();
        t.dock(Panel::Scopes, Some(Panel::Viewer), Zone::Left, &l);
        let l2 = lay(&t, 1434.0, 857.0);
        assert!(same(rect(&l2, Panel::Scopes), preview));
        assert!(rect(&l2, Panel::Scopes).width() >= Panel::Scopes.min_size().x - 0.01);
    }

    #[test]
    fn docking_as_a_tab_stacks_and_shows_it() {
        let mut t = Tree::default();
        let l = lay(&t, 1434.0, 857.0);
        assert!(t.dock(Panel::Sound, Some(Panel::Timeline), Zone::Tab, &l));
        let l2 = lay(&t, 1434.0, 857.0);
        let tile = l2.tile_of(Panel::Timeline).unwrap();
        assert_eq!(tile.panels, vec![Panel::Timeline, Panel::Sound]);
        assert_eq!(tile.active, Panel::Sound);
        assert!(t.is_showing(Panel::Sound) && t.is_open(Panel::Timeline) && !t.is_showing(Panel::Timeline));
        assert!(same(tile.rect, rect(&l, Panel::Timeline)));
    }

    #[test]
    fn moving_a_panel_closes_its_old_place() {
        // The Inspector moves under the Library: the top row loses it and
        // the Viewer widens; the Library becomes a column.
        let mut t = Tree::default();
        let l = lay(&t, 1434.0, 857.0);
        assert!(t.dock(Panel::Inspector, Some(Panel::Library), Zone::Bottom, &l));
        let l2 = lay(&t, 1434.0, 857.0);
        let (lib, insp, v) = (rect(&l2, Panel::Library), rect(&l2, Panel::Inspector), rect(&l2, Panel::Viewer));
        assert!(close(lib.left(), insp.left()) && insp.top() > lib.bottom());
        assert!(close(v.right(), l2.body.right()), "the viewer takes the inspector's room");
        assert_eq!(t.panels().len(), 4);
        // And back: same shape as before.
        assert!(t.dock(Panel::Inspector, Some(Panel::Viewer), Zone::Right, &l2));
        assert!(matches!(&t.root.kind, Kind::Split { axis: Axis::Column, children } if children.len() == 2));
    }

    #[test]
    fn docking_to_the_window_edge() {
        let mut t = Tree::default();
        let l = lay(&t, 1434.0, 857.0);
        assert!(t.dock(Panel::Scopes, None, Zone::Bottom, &l));
        let l2 = lay(&t, 1434.0, 857.0);
        let s = rect(&l2, Panel::Scopes);
        assert!(close(s.bottom(), l2.body.bottom()) && close(s.width(), l2.body.width()));
        assert!(t.dock(Panel::Review, None, Zone::Left, &l2));
        let l3 = lay(&t, 1434.0, 857.0);
        let r = rect(&l3, Panel::Review);
        assert!(close(r.left(), l3.body.left()) && close(r.height(), l3.body.height()));
    }

    #[test]
    fn removing_merges_splits_back() {
        let mut t = Tree::default();
        let l = lay(&t, 1434.0, 857.0);
        t.dock(Panel::Scopes, Some(Panel::Inspector), Zone::Bottom, &l);
        let wrapped = t.find(Panel::Inspector).unwrap();
        assert_eq!(wrapped.len(), 3, "inspector wrapped in a column");
        assert!(t.remove(Panel::Scopes));
        assert_eq!(t, Tree::default(), "the column goes and the inspector keeps its width");
    }

    #[test]
    fn tabs_reorder_and_move_between_stacks() {
        let mut t = Tree::default();
        let l = lay(&t, 1434.0, 857.0);
        t.dock(Panel::Sound, Some(Panel::Timeline), Zone::Tab, &l);
        t.dock(Panel::Scopes, Some(Panel::Timeline), Zone::Tab, &l);
        let stack = |t: &Tree| match &t.node(&t.find(Panel::Timeline).unwrap()).unwrap().kind {
            Kind::Tabs { panels, active } => (panels.clone(), *active),
            _ => unreachable!(),
        };
        assert_eq!(stack(&t).0, vec![Panel::Timeline, Panel::Sound, Panel::Scopes]);
        assert!(t.move_tab(Panel::Scopes, Panel::Timeline, 0));
        assert_eq!(stack(&t), (vec![Panel::Scopes, Panel::Timeline, Panel::Sound], 0));
        assert!(t.move_tab(Panel::Scopes, Panel::Timeline, 3));
        assert_eq!(stack(&t), (vec![Panel::Timeline, Panel::Sound, Panel::Scopes], 2));
        assert!(t.move_tab(Panel::Timeline, Panel::Timeline, 2));
        assert_eq!(stack(&t), (vec![Panel::Sound, Panel::Timeline, Panel::Scopes], 1));
        // Into another stack.
        assert!(t.move_tab(Panel::Sound, Panel::Inspector, 0));
        assert_eq!(stack(&t).0, vec![Panel::Timeline, Panel::Scopes]);
        assert!(t.is_showing(Panel::Sound) && !t.is_showing(Panel::Inspector));
    }

    #[test]
    fn close_and_reopen() {
        let mut t = Tree::default();
        let l = lay(&t, 1434.0, 857.0);
        // Alone in its stack: collapses, keeps place and size.
        assert!(t.close(Panel::Library));
        assert!(t.contains(Panel::Library) && !t.is_open(Panel::Library));
        let l2 = lay(&t, 1434.0, 857.0);
        assert!(l2.tile_of(Panel::Library).is_none());
        assert!(close(rect(&l2, Panel::Viewer).left(), l2.body.left()));
        assert!(t.open(Panel::Library, &l2));
        assert_eq!(t, Tree::default());
        // A tab closes out of its stack and comes back to it.
        t.dock(Panel::Sound, Some(Panel::Timeline), Zone::Tab, &l);
        assert!(t.close(Panel::Sound));
        assert!(!t.contains(Panel::Sound) && t.parked == vec![(Panel::Sound, Panel::Timeline)]);
        assert!(t.open(Panel::Sound, &l));
        assert!(t.is_showing(Panel::Sound) && t.parked.is_empty());
        assert_eq!(t.find(Panel::Sound), t.find(Panel::Timeline));
        // The Viewer cannot close.
        assert!(!t.close(Panel::Viewer));
        // A panel never docked goes to its default place.
        assert!(t.open(Panel::Scopes, &l));
        let l3 = lay(&t, 1434.0, 857.0);
        assert!(rect(&l3, Panel::Scopes).left() > rect(&l3, Panel::Viewer).right());
    }

    #[test]
    fn splitters_drag_within_minimums_and_double_click_collapses() {
        let mut t = Tree::default();
        let l = lay(&t, 1434.0, 857.0);
        // The Library | Viewer splitter.
        let s = l.splitters.iter().find(|s| s.axis == Axis::Row && s.before == Some(0)).unwrap().clone();
        assert!(t.drag(&s, 100.0, &l, &none));
        let l2 = lay(&t, 1434.0, 857.0);
        assert!(close(rect(&l2, Panel::Library).width(), LIBRARY_DEFAULT + 100.0));
        // Far too far: stops where the Viewer reaches its minimum.
        let s = l2.splitters.iter().find(|s| s.axis == Axis::Row && s.before == Some(0)).unwrap().clone();
        t.drag(&s, 5000.0, &l2, &none);
        let l3 = lay(&t, 1434.0, 857.0);
        assert!(close(rect(&l3, Panel::Viewer).width(), VIEWER_MIN));
        // And the other way stops at the Library's.
        let s = l3.splitters.iter().find(|s| s.axis == Axis::Row && s.before == Some(0)).unwrap().clone();
        t.drag(&s, -5000.0, &l3, &none);
        let l4 = lay(&t, 1434.0, 857.0);
        assert!(close(rect(&l4, Panel::Library).width(), LIBRARY_MIN));
        // The timeline splitter: a ratio stays a ratio.
        let s = l4.splitters.iter().find(|s| s.axis == Axis::Column).unwrap().clone();
        t.drag(&s, -50.0, &l4, &none);
        let l5 = lay(&t, 1434.0, 857.0);
        assert!(close(rect(&l5, Panel::Timeline).height(), rect(&l4, Panel::Timeline).height() + 50.0));
        assert!(matches!(t.node(&t.find(Panel::Timeline).unwrap()).unwrap().size, Size::Ratio(_)));
        // Double-click collapses the Inspector (not the Viewer) and opens it again.
        let s = l5.splitters.iter().find(|s| s.axis == Axis::Row && s.after == Some(2)).unwrap().clone();
        assert!(t.toggle(&s));
        assert!(!t.is_open(Panel::Inspector));
        let l6 = lay(&t, 1434.0, 857.0);
        let edge = l6.splitters.iter().find(|s| s.collapsed == vec![2]).unwrap().clone();
        assert!(close(edge.rect.left(), l6.body.right()), "the edge splitter waits at the window edge");
        assert!(t.toggle(&edge));
        assert!(t.is_open(Panel::Inspector));
        // Dragging an edge opens what waits there at its minimum.
        t.close(Panel::Library);
        let l7 = lay(&t, 1434.0, 857.0);
        let edge = l7.splitters.iter().find(|s| s.collapsed == vec![0]).unwrap().clone();
        assert!(t.drag(&edge, 10.0, &l7, &none));
        let l8 = lay(&t, 1434.0, 857.0);
        assert!(close(rect(&l8, Panel::Library).width(), LIBRARY_MIN));
    }

    #[test]
    fn minimum_sizes_hold_in_small_windows_and_tabs_add_their_strip() {
        let mut t = Tree::default();
        let l = lay(&t, 954.0, 557.0);
        // The smallest window: the side panels at their minimums and the
        // Viewer the rest, as before docking.
        assert!(close(rect(&l, Panel::Library).width(), LIBRARY_MIN) && close(rect(&l, Panel::Inspector).width(), INSPECTOR_MIN));
        assert!(close(rect(&l, Panel::Viewer).width(), 954.0 - LIBRARY_MIN - INSPECTOR_MIN - 2.0 * GAP));
        assert!(rect(&l, Panel::Timeline).height() >= TIMELINE_MIN - 0.01);
        t.dock(Panel::Sound, Some(Panel::Timeline), Zone::Tab, &l);
        let min = t.root.min_size(&M, &none);
        assert!(close(min.y, TOP_MIN + GAP + TIMELINE_MIN + 30.0), "{min:?}");
        // Hidden panels (the narrow window's Inspector) take no room.
        let l = t.layout(body(954.0, 557.0), M, &|p| p == Panel::Inspector);
        assert!(l.tile_of(Panel::Inspector).is_none());
        assert!(close(rect(&l, Panel::Viewer).right(), l.body.right()));
    }

    #[test]
    fn layouts_round_trip_through_json_and_bad_files_are_mended() {
        let mut t = Tree::default();
        let l = lay(&t, 1434.0, 857.0);
        t.dock(Panel::Scopes, Some(Panel::Viewer), Zone::Right, &l);
        t.dock(Panel::Sound, Some(Panel::Timeline), Zone::Tab, &l);
        t.close(Panel::Library);
        let json = serde_json::to_string(&t).unwrap();
        let back: Tree = serde_json::from_str(&json).unwrap();
        assert_eq!(back, t);
        // Duplicates and empty stacks are dropped; no Viewer is the default.
        let dup: Tree = serde_json::from_str(r#"{"root":{"kind":{"split":{"axis":"row","children":[
            {"size":{"px":300.0},"kind":{"tabs":{"panels":["library","viewer"]}}},
            {"kind":{"tabs":{"panels":[]}}},
            {"kind":{"tabs":{"panels":["viewer","timeline"],"active":7}}}]}}}}"#)
        .unwrap();
        let dup = dup.sanitized();
        assert_eq!(dup.panels(), vec![Panel::Library, Panel::Viewer, Panel::Timeline]);
        let no_viewer: Tree = serde_json::from_str(r#"{"root":{"kind":{"tabs":{"panels":["library"]}}}}"#).unwrap();
        assert_eq!(no_viewer.sanitized(), Tree::default());
    }

    #[test]
    fn panels_pop_out_and_dock_back_where_they_were() {
        let base = {
            let mut t = Tree::default();
            let l = lay(&t, 1434.0, 857.0);
            t.dock(Panel::Ai, Some(Panel::Library), Zone::Tab, &l);
            t.dock(Panel::Scopes, Some(Panel::Viewer), Zone::Right, &l);
            t
        };
        let l = lay(&base, 1434.0, 857.0);
        // From a stack: back into that stack.
        let mut t = base.clone();
        let size = rect(&l, Panel::Library).size();
        assert!(t.pop_out(Panel::Ai, &l));
        assert!(!t.contains(Panel::Ai) && t.is_popped(Panel::Ai) && t.is_shown_anywhere(Panel::Ai) && !t.is_open(Panel::Ai));
        assert_eq!(t.popped[0].size, Some([size.x, size.y]), "as big as it was");
        assert_eq!(t.popped[0].back, Some((Panel::Library, Zone::Tab)));
        assert!(!t.pop_out(Panel::Ai, &l), "already out");
        assert!(t.open(Panel::Ai, &l) && t.is_popped(Panel::Ai), "opening a popped panel leaves it in its window");
        let l2 = lay(&t, 1434.0, 857.0);
        assert!(t.dock_back(Panel::Ai, &l2));
        assert_eq!(t.find(Panel::Ai), t.find(Panel::Library));
        assert!(t.popped.is_empty());
        // From a split: back beside its neighbour.
        let mut t = base.clone();
        assert!(t.pop_out(Panel::Scopes, &l));
        assert_eq!(t.popped[0].back, Some((Panel::Viewer, Zone::Right)));
        let l2 = lay(&t, 1434.0, 857.0);
        assert!(rect(&l2, Panel::Viewer).right() > rect(&l, Panel::Viewer).right(), "the viewer takes its room");
        t.dock_back(Panel::Scopes, &l2);
        let l3 = lay(&t, 1434.0, 857.0);
        assert!(rect(&l3, Panel::Scopes).left() > rect(&l3, Panel::Viewer).right());
        // The Viewer stays; closing a popped panel closes its window and
        // reopening docks it where it came from.
        assert!(!t.pop_out(Panel::Viewer, &l3));
        t.pop_out(Panel::Ai, &l3);
        assert!(t.close(Panel::Ai) && !t.is_shown_anywhere(Panel::Ai));
        assert!(t.open(Panel::Ai, &l3) && t.find(Panel::Ai) == t.find(Panel::Library));
        // Docking a popped panel anywhere takes it out of its window.
        t.pop_out(Panel::Ai, &l3);
        let l4 = lay(&t, 1434.0, 857.0);
        assert!(t.dock(Panel::Ai, Some(Panel::Timeline), Zone::Tab, &l4));
        assert!(!t.is_popped(Panel::Ai) && t.find(Panel::Ai) == t.find(Panel::Timeline));
        // Sizes are remembered as the person resizes the window.
        t.pop_out(Panel::Ai, &l4);
        t.set_popped_size(Panel::Ai, Vec2::new(500.0, 700.0));
        assert_eq!(t.popped[0].size, Some([500.0, 700.0]));
    }

    #[test]
    fn popped_panels_round_trip_and_bad_files_are_mended() {
        let mut t = Tree::default();
        let l = lay(&t, 1434.0, 857.0);
        t.dock(Panel::Ai, Some(Panel::Library), Zone::Tab, &l);
        t.pop_out(Panel::Ai, &l);
        let json = serde_json::to_string(&t).unwrap();
        assert!(json.contains("popped"));
        assert_eq!(serde_json::from_str::<Tree>(&json).unwrap(), t);
        // A tree saved before popping out existed has none.
        let old = serde_json::to_string(&Tree::default()).unwrap();
        assert!(!old.contains("popped") && serde_json::from_str::<Tree>(&old).unwrap().popped.is_empty());
        // Docked and popped at once, twice popped, the Viewer popped: mended.
        let mut bad = t.clone();
        bad.popped.push(Popped { panel: Panel::Ai, size: None, back: None });
        bad.popped.push(Popped { panel: Panel::Library, size: None, back: None });
        bad.popped.push(Popped { panel: Panel::Viewer, size: None, back: None });
        let bad = bad.sanitized();
        assert_eq!(bad.popped.iter().map(|w| w.panel).collect::<Vec<_>>(), vec![Panel::Ai]);
    }

    #[test]
    fn names_parse_both_ways() {
        for p in Panel::ALL {
            assert_eq!(Panel::parse(p.key()), Some(p));
            assert_eq!(Panel::parse(p.name()), Some(p));
        }
        assert_eq!(Panel::parse("Mixer"), Some(Panel::Sound));
        assert_eq!(Panel::parse("agent"), Some(Panel::Ai));
        assert_eq!(Zone::parse("below"), Some(Zone::Bottom));
        assert_eq!(Zone::Left.mirrored(), Zone::Right);
    }

    #[test]
    fn the_review_workspace_and_its_panel_share_one_name() {
        let spaces: Vec<String> = crate::workspaces::builtin().into_iter().map(|w| w.name.to_ascii_lowercase()).collect();
        // The Review workspace opens the Review panel: one name for both,
        // and no other panel is named like a workspace.
        for p in Panel::ALL {
            let shared = spaces.contains(&p.name().to_ascii_lowercase());
            assert_eq!(shared, p == Panel::Review, "{} is both a panel and a workspace", p.name());
        }
        assert_eq!(Panel::Review.name(), "Review");
        assert!(spaces.contains(&"review".to_owned()));
        assert_eq!(Panel::parse("review"), Some(Panel::Review), "the old name still reaches it");
    }
}
