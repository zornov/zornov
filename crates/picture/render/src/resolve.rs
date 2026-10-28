// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The sequence resolver: from a scene at time `t` to an ordered list of
//! things to draw.
//!
//! Resolution decides stacking (lower `order` first, the spine at the
//! bottom), replaces the two clips of a running transition with one
//! transition node at the spine's position, separates adjustment clips
//! (which process everything below them) from picture layers, and
//! evaluates each clip's motion-preset pose for `t`. Nested scenes stay as
//! references: the compositor renders them recursively.

use crate::scene::{Canvas, ClipDesc, Micros, Scene, SourceDesc, TransitionDesc};
use crate::space::{CameraDesc, LightDesc, MAX_LIGHTS};
use crate::transform::Pose;

/// One item of the draw list.
#[derive(Clone, Debug)]
pub enum Node {
    /// A picture layer with its momentary motion pose.
    Layer { clip: ClipDesc, pose: Pose },
    /// A transition between two spine clips, each with its own pose.
    Transition { desc: TransitionDesc, left_pose: Pose, right_pose: Pose },
    /// An adjustment clip: effects, grade and masks applied to the canvas
    /// so far.
    Adjustment { clip: ClipDesc },
}

impl Node {
    fn order(&self) -> (i32, Micros, u64) {
        match self {
            Node::Layer { clip, .. } | Node::Adjustment { clip } => (clip.order, clip.start, clip.id),
            Node::Transition { desc, .. } => (desc.left.order, desc.left.start, desc.left.id),
        }
    }

    /// The clip ids this node draws (two for a transition).
    pub fn clip_ids(&self) -> Vec<u64> {
        match self {
            Node::Layer { clip, .. } | Node::Adjustment { clip } => vec![clip.id],
            Node::Transition { desc, .. } => vec![desc.left.id, desc.right.id],
        }
    }
}

/// Everything to draw for one output frame, bottom to top.
#[derive(Clone, Debug)]
pub struct RenderList {
    pub canvas: Canvas,
    pub time: Micros,
    pub nodes: Vec<Node>,
    /// The camera looking at the 3D layers (the topmost camera clip), if
    /// any; otherwise the default camera.
    pub camera: Option<CameraDesc>,
    /// Lights on the 3D layers.
    pub lights: Vec<LightDesc>,
}

impl RenderList {
    /// The camera the 3D layers are seen through.
    pub fn camera(&self) -> CameraDesc {
        self.camera.unwrap_or_else(|| CameraDesc::default_for(&self.canvas))
    }

    /// Layers in top-to-bottom order (for hit-testing).
    pub fn top_down(&self) -> impl Iterator<Item = &Node> {
        self.nodes.iter().rev()
    }
}

/// Pose of a clip at scene time `t`.
pub fn pose_at(clip: &ClipDesc, t: Micros) -> Pose {
    crate::effects::motion_pose(&clip.motion, t - clip.start, clip.duration)
}

/// Resolves the scene at `t`.
pub fn resolve(scene: &dyn Scene, t: Micros) -> RenderList {
    let canvas = scene.canvas();
    let mut nodes: Vec<Node> = Vec::new();
    let mut camera: Option<(i32, Micros, u64, CameraDesc)> = None;
    let mut lights: Vec<LightDesc> = Vec::new();
    for clip in scene.clips_at(t) {
        match &clip.source {
            SourceDesc::Camera(c) => {
                let key = (clip.order, clip.start, clip.id);
                if camera.is_none_or(|(o, s, i, _)| key > (o, s, i)) {
                    camera = Some((key.0, key.1, key.2, *c));
                }
                continue;
            }
            SourceDesc::Light(l) => {
                if lights.len() < MAX_LIGHTS {
                    lights.push(*l);
                }
                continue;
            }
            _ => {}
        }
        if clip.placement.opacity <= 0.0 && !matches!(clip.source, SourceDesc::Adjustment) {
            continue;
        }
        match clip.source {
            SourceDesc::Adjustment => nodes.push(Node::Adjustment { clip }),
            _ => {
                let pose = pose_at(&clip, t);
                if pose.opacity <= 0.0 {
                    continue;
                }
                nodes.push(Node::Layer { clip, pose });
            }
        }
    }
    if let Some(desc) = scene.transition_at(t) {
        let left_pose = pose_at(&desc.left, t);
        let right_pose = pose_at(&desc.right, t);
        nodes.push(Node::Transition { desc, left_pose, right_pose });
    }
    nodes.sort_by_key(|n| n.order());
    let camera = camera.map(|c| c.3);
    let seen = camera.unwrap_or_else(|| CameraDesc::default_for(&canvas));
    for n in &mut nodes {
        match n {
            Node::Layer { clip, pose } => settle_mesh(clip, pose, &canvas, &seen, &lights),
            Node::Transition { desc, left_pose, right_pose } => {
                settle_mesh(&mut desc.left, left_pose, &canvas, &seen, &lights);
                settle_mesh(&mut desc.right, right_pose, &canvas, &seen, &lights);
            }
            Node::Adjustment { .. } => {}
        }
    }
    let nodes = share_world(merge_meshes(nodes));
    RenderList { canvas, time: t, nodes, camera, lights }
}

/// A model layer made ready to draw: its transform and pose become the
/// model's place in the world, and the layer itself becomes a flat
/// full-canvas picture (it keeps its opacity and blur) seen through the
/// scene's camera and lit by its lights.
fn settle_mesh(clip: &mut ClipDesc, pose: &mut Pose, canvas: &Canvas, camera: &CameraDesc, lights: &[LightDesc]) {
    if let SourceDesc::World(w) = &mut clip.source {
        // A world is seen through the scene camera as it is; it keeps its opacity.
        w.camera = Some(*camera);
        // The scene's air and look, from the project's light clips.
        w.carrier = lights.iter().find_map(|l| l.look.and_then(|k| k.atmos));
        clip.placement = crate::transform::Placement { opacity: clip.placement.opacity, ..Default::default() };
        clip.space = None;
        *pose = Pose { opacity: pose.opacity, blur: pose.blur, ..Pose::REST };
        return;
    }
    let SourceDesc::Mesh(m) = &mut clip.source else { return };
    let space = clip.space.unwrap_or_default();
    for inst in &mut m.instances {
        inst.world = crate::mesh_render::model_matrix(canvas.size(), &clip.placement, &space, pose);
        // Where it will be when the shutter closes (motion blur).
        if let Some(m) = inst.motion.as_mut() {
            let m = std::sync::Arc::make_mut(m);
            m.world = crate::mesh_render::model_matrix(canvas.size(), &m.placement, &m.space, pose);
        }
        inst.accepts_lights = space.material.accepts_lights;
        inst.casts_shadows = space.material.casts_shadows;
        inst.accepts_shadows = space.material.accepts_shadows;
    }
    m.camera = Some(*camera);
    m.lights = lights.to_vec();
    clip.placement = crate::transform::Placement { opacity: clip.placement.opacity, ..Default::default() };
    clip.space = None;
    *pose = Pose { opacity: pose.opacity, blur: pose.blur, ..Pose::REST };
}

/// Neighbouring plain model layers (nothing on them but the model) drawn
/// in one pass, so nearer models hide further ones wherever they meet.
fn merge_meshes(nodes: Vec<Node>) -> Vec<Node> {
    let plain = |n: &Node| match n {
        Node::Layer { clip, pose } => {
            matches!(clip.source, SourceDesc::Mesh(_))
                && clip.effects.is_empty()
                && clip.grade.is_none()
                && clip.masks.is_empty()
                && clip.warp.is_none()
                && clip.matte.is_none()
                && clip.blend == crate::blend::BlendMode::Normal
                && clip.placement.opacity >= 1.0
                && pose.opacity >= 1.0
                && pose.blur == 0.0
        }
        _ => false,
    };
    let mut out: Vec<Node> = Vec::with_capacity(nodes.len());
    for n in nodes {
        if plain(&n)
            && let Some(last) = out.last_mut()
            && plain(last)
            && let (Node::Layer { clip: into, .. }, Node::Layer { clip: from, .. }) = (last, &n)
            && let (SourceDesc::Mesh(a), SourceDesc::Mesh(b)) = (&mut into.source, &from.source)
        {
            a.instances.extend(b.instances.iter().cloned());
            continue;
        }
        out.push(n);
    }
    out
}

/// The 3D passes of a frame (model layers split apart by 2D layers, or by
/// effects of their own) are one world: each pass draws its own models,
/// with the lower passes' models as holdouts (nearer ones hide it) and the
/// higher passes' as shadow casters. The world's sky and the air of empty
/// space are drawn once, by the lowest pass (or the world layer below);
/// the passes above are transparent wherever none of their models are, so
/// a 2D card between them stays seen.
fn share_world(mut nodes: Vec<Node>) -> Vec<Node> {
    let pass = |n: &Node| match n {
        Node::Layer { clip, .. } => matches!(clip.source, SourceDesc::Mesh(_)),
        _ => false,
    };
    let world_at = nodes.iter().position(|n| matches!(n, Node::Layer { clip, .. } if matches!(clip.source, SourceDesc::World(_))));
    let passes: Vec<usize> = nodes.iter().enumerate().filter(|(_, n)| pass(n)).map(|(i, _)| i).collect();
    if passes.len() < 2 && world_at.is_none_or(|w| passes.first().is_none_or(|p| *p < w)) {
        return nodes;
    }
    let own: Vec<Vec<crate::mesh_render::MeshInstance>> = passes
        .iter()
        .map(|&i| match &nodes[i] {
            Node::Layer { clip, .. } => match &clip.source {
                SourceDesc::Mesh(m) => m.instances.clone(),
                _ => Vec::new(),
            },
            _ => Vec::new(),
        })
        .collect();
    for (k, &i) in passes.iter().enumerate() {
        if let Node::Layer { clip, .. } = &mut nodes[i]
            && let SourceDesc::Mesh(m) = &mut clip.source
        {
            m.holdouts = own[..k].iter().flatten().cloned().collect();
            m.casters = own[k + 1..].iter().flatten().cloned().collect();
            m.layered = k > 0 || world_at.is_some_and(|w| w < i);
        }
    }
    nodes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{GeneratorDesc, GeneratorKind, Params, SECOND, StaticScene};

    fn solid(id: u64, order: i32, start: Micros, dur: Micros) -> ClipDesc {
        ClipDesc::new(id, order, start, dur, SourceDesc::Generator(GeneratorDesc { kind: GeneratorKind::Solid, params: Params::new(), text: vec![] }))
    }

    #[test]
    fn stacking_and_transitions() {
        let scene = StaticScene::new(Canvas::new(64, 36, 30.0), 4 * SECOND)
            .with_clip(solid(1, 0, 0, 2 * SECOND))
            .with_clip(solid(2, 0, 2 * SECOND, 2 * SECOND))
            .with_clip(solid(3, 2, SECOND, 2 * SECOND))
            .with_clip(solid(4, 1, 0, 4 * SECOND))
            .with_transition(1, 2, "dissolve", 3 * SECOND / 2, SECOND, Params::new());
        let list = resolve(&scene, SECOND / 2);
        let ids: Vec<Vec<u64>> = list.nodes.iter().map(|n| n.clip_ids()).collect();
        assert_eq!(ids, vec![vec![1], vec![4]]);
        let list = resolve(&scene, 7 * SECOND / 4);
        let ids: Vec<Vec<u64>> = list.nodes.iter().map(|n| n.clip_ids()).collect();
        assert_eq!(ids, vec![vec![1, 2], vec![4], vec![3]]);
        match &list.nodes[0] {
            Node::Transition { desc, .. } => assert!((desc.progress - 0.25).abs() < 1e-6),
            _ => panic!(),
        }
        // Invisible clips are dropped.
        let mut hidden = solid(9, 5, 0, SECOND);
        hidden.placement.opacity = 0.0;
        let scene = scene.with_clip(hidden);
        assert_eq!(resolve(&scene, 0).nodes.len(), 2);
    }
}
