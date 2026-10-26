// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Hit-testing: which layer is under a canvas point at a time.
//!
//! Every visible layer is selectable: the test walks the resolved draw
//! list from the top down and maps the point back through each layer's
//! real transform (crop, scale, rotation, flip, motion pose) to its visible
//! source rectangle. Masks and pixel alpha are deliberately ignored, so a
//! layer stays selectable where it is masked out or transparent (as in
//! every editor; the bounds are what the handles show).

use crate::compositor::layer_geometry;
use crate::resolve::{Node, resolve};
use crate::scene::{Micros, Scene};
use crate::source::Sources;

/// Nodes bottom to top as drawn: 3D runs far to near.
fn order(list: &crate::resolve::RenderList, sources: &mut Sources) -> Vec<usize> {
    let (canvas, camera) = (list.canvas, list.camera());
    crate::space::draw_order(list, |clip, pose| layer_geometry(sources, clip, pose, &canvas, &camera, 1.0).and_then(|g| g.depth))
}

/// One selectable layer and its canvas-space outline.
#[derive(Clone, Debug, PartialEq)]
pub struct LayerBounds {
    pub clip: u64,
    /// Corners in canvas pixels: top-left, top-right, bottom-right,
    /// bottom-left of the visible source rectangle.
    pub quad: [[f32; 2]; 4],
    /// Stacking order (higher is on top).
    pub order: i32,
}

/// Outlines of every visible layer at `t`, bottom to top. Transition
/// clips are both included; adjustment clips have no picture and are
/// left out.
pub fn layer_bounds(scene: &dyn Scene, sources: &mut Sources, t: Micros) -> Vec<LayerBounds> {
    let list = resolve(scene, t);
    let canvas = list.canvas;
    let camera = list.camera();
    let mut out = Vec::new();
    for i in order(&list, sources) {
        let node = &list.nodes[i];
        let items: Vec<(&crate::scene::ClipDesc, &crate::transform::Pose)> = match node {
            Node::Layer { clip, pose } => vec![(clip, pose)],
            Node::Transition { desc, left_pose, right_pose } => vec![(&desc.left, left_pose), (&desc.right, right_pose)],
            Node::Adjustment { .. } => vec![],
        };
        for (clip, pose) in items {
            if let crate::scene::SourceDesc::Mesh(m) = &clip.source {
                for (id, r) in crate::mesh_render::screen_rects(m, &canvas) {
                    out.push(LayerBounds { clip: id, quad: [[r[0], r[1]], [r[2], r[1]], [r[2], r[3]], [r[0], r[3]]], order: clip.order });
                }
                continue;
            }
            if let Some(g) = layer_geometry(sources, clip, pose, &canvas, &camera, 1.0) {
                out.push(LayerBounds { clip: clip.id, quad: g.quad(), order: clip.order });
            }
        }
    }
    out
}

/// The topmost layer whose transformed bounds contain `point` (canvas
/// pixels) at `t`, or `None` over the background.
pub fn layer_at(scene: &dyn Scene, sources: &mut Sources, point: [f32; 2], t: Micros) -> Option<u64> {
    let list = resolve(scene, t);
    let canvas = list.canvas;
    let camera = list.camera();
    for i in order(&list, sources).into_iter().rev() {
        let node = &list.nodes[i];
        let items: Vec<(&crate::scene::ClipDesc, &crate::transform::Pose)> = match node {
            Node::Layer { clip, pose } => vec![(clip, pose)],
            // The incoming clip of a transition is drawn over the outgoing one.
            Node::Transition { desc, left_pose, right_pose } => vec![(&desc.right, right_pose), (&desc.left, left_pose)],
            Node::Adjustment { .. } => vec![],
        };
        for (clip, pose) in items {
            if let crate::scene::SourceDesc::Mesh(m) = &clip.source {
                if let Some((id, _)) = crate::mesh_render::screen_rects(m, &canvas).into_iter().rev().find(|(_, r)| point[0] >= r[0] && point[0] <= r[2] && point[1] >= r[1] && point[1] <= r[3]) {
                    return Some(id);
                }
                continue;
            }
            if let Some(g) = layer_geometry(sources, clip, pose, &canvas, &camera, 1.0)
                && g.hit(point)
            {
                return Some(clip.id);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{Canvas, ClipDesc, GeneratorDesc, GeneratorKind, Params, SECOND, SourceDesc, StaticScene};
    use crate::source::{DecodeCache, SourceRegistry};

    fn solid(id: u64, order: i32) -> ClipDesc {
        ClipDesc::new(id, order, 0, SECOND, SourceDesc::Generator(GeneratorDesc { kind: GeneratorKind::Solid, params: Params::new(), text: vec![] }))
    }

    #[test]
    fn topmost_wins_and_rotation_counts() {
        let mut top = solid(2, 1);
        top.placement.scale = [0.5, 0.5];
        top.placement.rotation = 45.0;
        let scene = StaticScene::new(Canvas::new(200, 200, 30.0), SECOND).with_clip(solid(1, 0)).with_clip(top);
        let mut s = Sources::new(SourceRegistry::new(), DecodeCache::new(1 << 20));
        assert_eq!(layer_at(&scene, &mut s, [100.0, 100.0], 0), Some(2));
        // The rotated square's corner reaches 70 px from the centre on the
        // axes; 60 px out is inside, 80 px out is the bottom layer.
        assert_eq!(layer_at(&scene, &mut s, [160.0, 100.0], 0), Some(2));
        assert_eq!(layer_at(&scene, &mut s, [180.0, 100.0], 0), Some(1));
        // The unrotated square's corner would have been hit.
        assert_eq!(layer_at(&scene, &mut s, [145.0, 145.0], 0), Some(1));
        assert_eq!(layer_at(&scene, &mut s, [-5.0, 100.0], 0), None);
        let b = layer_bounds(&scene, &mut s, 0);
        assert_eq!(b.len(), 2);
        assert_eq!(b[1].clip, 2);
    }
}
