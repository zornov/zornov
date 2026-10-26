// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The 3D workspace's view of a scene: the same clips seen through an
//! editor camera instead of the scene's own, for moving through the
//! world as in a 3D application.
//!
//! [`ViewScene`] wraps any [`Scene`] and changes only what the workspace
//! asks for, so every compositor draws it unchanged:
//!
//! - the scene's cameras are dropped and the editor camera put on top
//!   (the topmost camera is the one the 3D layers are seen through);
//! - 2D layers become cards in the world, each at the depth the workspace
//!   gives it (all at the canvas plane, or spread out by stacking order
//!   for arranging stills and video in space); they stay unlit, as they
//!   drew in 2D;
//! - clips hidden in the viewport are left out;
//! - the shading mode decides the light: the scene's own (rendered), a
//!   light that follows the view (material), or that light on plain grey
//!   models (solid);
//! - the background can be transparent, so the workspace's floor grid
//!   shows behind the world.

use crate::mesh_render::MaterialOverride;
use crate::scene::{Canvas, ClipDesc, Micros, Scene, SourceDesc, TransitionDesc};
use crate::space::{CameraDesc, LightDesc, LightKind, MaterialDesc, Space, normalise};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// How the viewport draws the world.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Shading {
    /// The scene's own lights, materials and effects: the final picture.
    #[default]
    Rendered,
    /// Materials and pictures under a light that follows the view.
    Material,
    /// Plain grey models under the view light; pictures stay.
    Solid,
    /// Only outlines (drawn by the workspace; the picture is not needed).
    Wireframe,
    /// The material picture seen through, outlines on top.
    Xray,
}

impl Shading {
    pub const ALL: [Shading; 5] = [Shading::Solid, Shading::Material, Shading::Rendered, Shading::Wireframe, Shading::Xray];

    pub fn name(self) -> &'static str {
        match self {
            Shading::Rendered => "rendered",
            Shading::Material => "material",
            Shading::Solid => "solid",
            Shading::Wireframe => "wireframe",
            Shading::Xray => "xray",
        }
    }

    pub fn parse(s: &str) -> Option<Shading> {
        Some(match s.trim().to_lowercase().replace(['-', ' ', '_'], "").as_str() {
            "rendered" | "render" | "final" => Shading::Rendered,
            "material" | "materialpreview" | "lookdev" => Shading::Material,
            "solid" | "clay" => Shading::Solid,
            "wireframe" | "wire" => Shading::Wireframe,
            "xray" | "x" | "seethrough" => Shading::Xray,
            _ => return None,
        })
    }

    /// Whether the picture is rendered at all in this mode.
    pub fn draws_picture(self) -> bool {
        self != Shading::Wireframe
    }
}

/// What the workspace changes about the scene.
#[derive(Clone, Debug, PartialEq)]
pub struct ViewOptions {
    /// The editor camera (canvas pixels, as scene cameras are).
    pub camera: CameraDesc,
    pub shading: Shading,
    /// 2D layers become cards in the world (otherwise they stay flat on
    /// the screen, as in the final picture).
    pub cards: bool,
    /// World pixels between neighbouring 2D layers in stacking order: the
    /// lowest goes furthest back, the stack centred on the canvas plane.
    /// 0 puts every card at the canvas plane.
    pub spread: f32,
    /// Clips left out of the viewport.
    pub hidden: HashSet<u64>,
    /// Draw nothing where nothing is (the workspace paints its own floor).
    pub transparent: bool,
}

impl ViewOptions {
    pub fn new(camera: CameraDesc) -> ViewOptions {
        ViewOptions { camera, shading: Shading::Rendered, cards: true, spread: 0.0, hidden: HashSet::new(), transparent: false }
    }
}

/// Id of the editor camera's clip (never a model id).
pub const EDITOR_CAMERA_ID: u64 = u64::MAX - 7;

/// A scene seen from the 3D workspace.
pub struct ViewScene {
    pub inner: Arc<dyn Scene>,
    pub options: ViewOptions,
}

impl ViewScene {
    pub fn new(inner: Arc<dyn Scene>, options: ViewOptions) -> ViewScene {
        ViewScene { inner, options }
    }

    /// The light that follows the view: from above the lens and to its
    /// left, with some ambient so nothing goes black.
    pub fn view_lights(camera: &CameraDesc) -> [LightDesc; 2] {
        let a = camera.axes;
        // In camera space: right, down, forward; towards down-right-ahead.
        let local = normalise([0.35, 0.55, 1.0]);
        let dir = normalise([
            a[0][0] * local[0] + a[1][0] * local[1] + a[2][0] * local[2],
            a[0][1] * local[0] + a[1][1] * local[1] + a[2][1] * local[2],
            a[0][2] * local[0] + a[1][2] * local[1] + a[2][2] * local[2],
        ]);
        let light = |kind, colour: f32| LightDesc { kind, position: camera.position, direction: dir, colour: [colour; 3], cone: [0.0, 1.0], radius: 0.0, falloff: 0.0, casts_shadows: false, shadow_darkness: 0.0, sky: None, look: None };
        [light(LightKind::Ambient, 0.38), light(LightKind::Parallel, 0.78)]
    }

    fn solid_override() -> MaterialOverride {
        MaterialOverride { base_colour: Some([0.62, 0.62, 0.62, 1.0]), metallic: Some(0.0), roughness: Some(0.55), emissive: Some([0.0; 3]), opacity: None, subsurface: Some(0.0), wetness: None, emissive_strength: None }
    }

    /// Whether a clip is a 2D picture layer (what becomes a card).
    fn flat_picture(c: &ClipDesc) -> bool {
        c.space.is_none() && !matches!(c.source, SourceDesc::Camera(_) | SourceDesc::Light(_) | SourceDesc::Adjustment | SourceDesc::Mesh(_) | SourceDesc::World(_))
    }

    /// The depth of each 2D layer's card among `clips` (the workspace uses
    /// the same numbers to draw and pick them).
    pub fn depths_of(clips: &[ClipDesc], spread: f32) -> HashMap<u64, f32> {
        let mut flat: Vec<(i32, Micros, u64)> = clips.iter().filter(|c| Self::flat_picture(c)).map(|c| (c.order, c.start, c.id)).collect();
        flat.sort();
        let mid = (flat.len().max(1) - 1) as f32 * 0.5;
        flat.iter().enumerate().map(|(rank, k)| (k.2, (mid - rank as f32) * spread)).collect()
    }

    /// The card depths at `t`.
    pub fn card_depths(&self, t: Micros) -> HashMap<u64, f32> {
        let clips: Vec<ClipDesc> = self.inner.clips_at(t).into_iter().filter(|c| !self.options.hidden.contains(&c.id)).collect();
        Self::depths_of(&clips, self.options.spread)
    }

    fn adjust(&self, mut clip: ClipDesc, studio: bool, depths: &HashMap<u64, f32>) -> Option<ClipDesc> {
        let o = &self.options;
        match &mut clip.source {
            SourceDesc::Camera(_) => return None,
            SourceDesc::Light(_) if studio => return None,
            SourceDesc::Light(_) | SourceDesc::Adjustment => {}
            SourceDesc::Mesh(m) => {
                if o.shading == Shading::Solid {
                    for inst in &mut m.instances {
                        inst.overrides = Self::solid_override();
                        inst.accepts_lights = true;
                    }
                }
            }
            _ => {
                if clip.space.is_none() && o.cards {
                    // A 2D layer becomes an unlit card at its depth.
                    let z = depths.get(&clip.id).copied().unwrap_or(0.0);
                    clip.space = Some(Space { z, material: MaterialDesc { accepts_lights: false, casts_shadows: false, accepts_shadows: false, ..MaterialDesc::default() }, ..Space::default() });
                }
            }
        }
        Some(clip)
    }
}

impl Scene for ViewScene {
    fn canvas(&self) -> Canvas {
        let mut c = self.inner.canvas();
        if self.options.transparent {
            c.background = [0.0; 4];
        }
        c
    }

    fn duration(&self) -> Micros {
        self.inner.duration()
    }

    fn clips_at(&self, t: Micros) -> Vec<ClipDesc> {
        let studio = matches!(self.options.shading, Shading::Material | Shading::Solid | Shading::Xray);
        let clips: Vec<ClipDesc> = self.inner.clips_at(t).into_iter().filter(|c| !self.options.hidden.contains(&c.id)).collect();
        let depths = Self::depths_of(&clips, self.options.spread);
        let mut out: Vec<ClipDesc> = clips.into_iter().filter_map(|c| self.adjust(c, studio, &depths)).collect();
        let dur = self.inner.duration().max(1);
        out.push(ClipDesc::new(EDITOR_CAMERA_ID, i32::MAX, 0, dur.max(t + 1), SourceDesc::Camera(self.options.camera)));
        if studio {
            for (i, l) in Self::view_lights(&self.options.camera).into_iter().enumerate() {
                out.push(ClipDesc::new(EDITOR_CAMERA_ID - 1 - i as u64, i32::MAX - 1, 0, dur.max(t + 1), SourceDesc::Light(l)));
            }
        }
        out
    }

    fn transition_at(&self, t: Micros) -> Option<TransitionDesc> {
        let tr = self.inner.transition_at(t)?;
        if self.options.hidden.contains(&tr.left.id) || self.options.hidden.contains(&tr.right.id) {
            return None;
        }
        let studio = matches!(self.options.shading, Shading::Material | Shading::Solid | Shading::Xray);
        let mut tr = tr;
        let depths = HashMap::new();
        tr.left = self.adjust(tr.left, studio, &depths)?;
        tr.right = self.adjust(tr.right, studio, &depths)?;
        Some(tr)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolve::resolve;
    use crate::scene::{GeneratorDesc, GeneratorKind, Params, SECOND, StaticScene};

    fn solid(id: u64, order: i32) -> ClipDesc {
        ClipDesc::new(id, order, 0, SECOND, SourceDesc::Generator(GeneratorDesc { kind: GeneratorKind::Solid, params: Params::new(), text: vec![] }))
    }

    fn scene() -> Arc<dyn Scene> {
        let mut s = StaticScene::new(Canvas::new(320, 180, 30.0), SECOND);
        s.clips.push(solid(1, 0));
        s.clips.push(solid(2, 1));
        let mut cam = CameraDesc::default_for(&s.canvas);
        cam.position = [500.0, 0.0, -100.0];
        s.clips.push(ClipDesc::new(3, 5, 0, SECOND, SourceDesc::Camera(cam)));
        Arc::new(s)
    }

    #[test]
    fn the_editor_camera_replaces_the_scene_camera() {
        let inner = scene();
        let mut cam = CameraDesc::default_for(&inner.canvas());
        cam.position = [0.0, -900.0, -40.0];
        let v = ViewScene::new(inner, ViewOptions::new(cam));
        let list = resolve(&v, 0);
        assert_eq!(list.camera.expect("camera").position, [0.0, -900.0, -40.0]);
    }

    #[test]
    fn two_d_layers_become_cards_at_their_depth_and_hidden_ones_go() {
        let inner = scene();
        let cam = CameraDesc::default_for(&inner.canvas());
        let mut o = ViewOptions::new(cam);
        o.spread = 100.0;
        let v = ViewScene::new(inner.clone(), o.clone());
        let clips = v.clips_at(0);
        // Two layers 100 apart round the canvas plane, the lower further.
        let z = |id: u64| clips.iter().find(|c| c.id == id).and_then(|c| c.space).expect("a card").z;
        assert_eq!((z(1), z(2)), (50.0, -50.0));
        assert_eq!(v.card_depths(0)[&1], 50.0);
        assert!(!clips.iter().find(|c| c.id == 1).unwrap().space.unwrap().material.accepts_lights);
        o.hidden.insert(2);
        let v = ViewScene::new(inner.clone(), o);
        let clips = v.clips_at(0);
        assert!(clips.iter().all(|c| c.id != 2));
        assert_eq!(clips.iter().find(|c| c.id == 1).and_then(|c| c.space).unwrap().z, 0.0);
        // Flat: 2D stays 2D.
        let mut o = ViewOptions::new(cam);
        o.cards = false;
        let v = ViewScene::new(inner, o);
        assert!(v.clips_at(0).iter().find(|c| c.id == 1).unwrap().space.is_none());
    }

    /// Rendered through the editor's eye: a full-canvas 2D layer seen from
    /// the front fills the picture; from the side it is edge-on (a thin
    /// line), and with spread two layers stand apart in depth, so from the
    /// side both show as separate lines.
    #[test]
    fn cards_render_from_any_side() {
        use crate::compositor::{CpuCompositor, FrameRenderer};
        use crate::scene::Param;
        use crate::source::{DecodeCache, SourceRegistry, Sources};
        let canvas = Canvas::new(160, 90, 30.0);
        let mut s = StaticScene::new(canvas.clone(), SECOND);
        for (id, rgba) in [(1, [1.0, 0.0, 0.0, 1.0]), (2, [0.0, 0.0, 1.0, 1.0])] {
            let mut params = Params::new();
            params.insert("colour".into(), Param::Colour(rgba));
            let mut c = ClipDesc::new(id, id as i32, 0, SECOND, SourceDesc::Generator(GeneratorDesc { kind: GeneratorKind::Solid, params, text: vec![] }));
            c.placement.scale = [0.5, 0.5];
            s.clips.push(c);
        }
        let inner: Arc<dyn Scene> = Arc::new(s);
        let mut cpu = CpuCompositor::new(Sources::new(SourceRegistry::new(), DecodeCache::new(1 << 24)));
        let covered = |f: &crate::frame::Frame| {
            let mut n = 0;
            for y in 0..f.height {
                for x in 0..f.width {
                    if f.get(x, y)[3] > 0.5 {
                        n += 1;
                    }
                }
            }
            n as f32 / (f.width * f.height) as f32
        };
        let zoom = crate::space::default_zoom(160.0);
        let front = CameraDesc::default_for(&canvas);
        let mut o = ViewOptions::new(front);
        o.transparent = true;
        let f = cpu.render(&ViewScene::new(inner.clone(), o.clone()), 0, None);
        let a = covered(&f);
        assert!((a - 0.25).abs() < 0.05, "front: {a}");
        // From the right side (+x), looking -x, at the canvas plane.
        let side = CameraDesc { position: [zoom, 0.0, 0.0], axes: crate::space::look_at([zoom, 0.0, 0.0], [0.0, 0.0, 0.0], 0.0), zoom, dof: None, trace: None };
        o.camera = side;
        let f = cpu.render(&ViewScene::new(inner.clone(), o.clone()), 0, None);
        assert!(covered(&f) < 0.03, "edge-on: {}", covered(&f));
        // Spread apart, seen from above and in front: two cards, one
        // behind the other, cover more than one did.
        o.spread = 60.0;
        let above = [0.0, -zoom * 0.7, -zoom * 0.7];
        o.camera = CameraDesc { position: above, axes: crate::space::look_at(above, [0.0, 0.0, 0.0], 0.0), zoom, dof: None, trace: None };
        let spread = covered(&cpu.render(&ViewScene::new(inner.clone(), o.clone()), 0, None));
        o.spread = 0.0;
        let flat = covered(&cpu.render(&ViewScene::new(inner, o), 0, None));
        assert!(spread > flat * 1.2, "spread {spread} vs flat {flat}");
    }

    #[test]
    fn studio_shading_lights_from_the_view() {
        let inner = scene();
        let cam = CameraDesc::default_for(&inner.canvas());
        let mut o = ViewOptions::new(cam);
        o.shading = Shading::Solid;
        o.transparent = true;
        let v = ViewScene::new(inner, o);
        let list = resolve(&v, 0);
        assert_eq!(list.lights.len(), 2);
        assert_eq!(v.canvas().background, [0.0; 4]);
        // The parallel light shines into the view (positive forward).
        let p = list.lights.iter().find(|l| l.kind == LightKind::Parallel).unwrap();
        assert!(p.direction[2] > 0.5);
        for s in Shading::ALL {
            assert_eq!(Shading::parse(s.name()), Some(s));
        }
    }
}
