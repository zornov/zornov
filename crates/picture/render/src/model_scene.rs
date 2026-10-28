// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Adapter from a `studio_model::Project` to the renderer's [`Scene`].
//!
//! The model owns keyframe evaluation (`Clip::transform_at` and friends);
//! this adapter calls it for the requested time and translates the model's
//! types into the renderer's. Media keys are the model's media ids;
//! register each media item's path with the [`SourceRegistry`] (see
//! [`register_media`]).

use std::sync::Arc;

use studio_model::{BlendMode as MBlend, Clip, ClipSource, GeneratorKind as MGen, LaneKind, MaskMode as MMask, MaskShape as MShape, Micros, Project, Sequence, SequenceId, Value};

use crate::blend::BlendMode;
use crate::mask::{MaskMode, MaskShape, MaskSpec, PathPoint};
use crate::output::OutputSpace;
use crate::puppet::{CutDesc, FillDesc, PinDesc, PinKind, PuppetDesc, SkinBone, SkinDesc, WarpDesc};
use crate::scene::{Canvas, ClipDesc, EffectDesc, GeneratorDesc, GeneratorKind, GradeDesc, MediaKey, MotionDesc, MotionRef, Param, Params, ParamsExt, Scene, SourceDesc, TextRun, TransitionDesc};
use crate::colour_space::InputColour;
use crate::source::SourceRegistry;
use crate::view::ViewTransform;
use crate::space::{CameraDesc, Dof, LightDesc, LightKind, MaterialDesc, Space};
use crate::transform::Placement;
use studio_model::space as sp;

/// A project's sequence as a scene.
pub struct ModelScene {
    project: Arc<Project>,
    sequence: SequenceId,
    /// Drawn inside another clip (a compound): transparent where nothing
    /// is, so it lays over what is below like any clip.
    nested: bool,
    /// A view transform for this viewer instead of the project's.
    view_override: Option<ViewTransform>,
    /// Draw only this lane (a multicam clip showing one angle).
    only_lane: Option<studio_model::LaneId>,
}

impl ModelScene {
    /// The project's active sequence.
    pub fn new(project: Arc<Project>) -> ModelScene {
        let sequence = project.active_sequence;
        ModelScene { project, sequence, nested: false, view_override: None, only_lane: None }
    }

    pub fn for_sequence(project: Arc<Project>, sequence: SequenceId) -> ModelScene {
        ModelScene { project, sequence, nested: false, view_override: None, only_lane: None }
    }

    /// A sequence drawn inside a clip of another.
    pub fn nested(project: Arc<Project>, sequence: SequenceId) -> ModelScene {
        ModelScene { project, sequence, nested: true, view_override: None, only_lane: None }
    }

    /// Draws only `lane` (a multicam angle).
    pub fn only_lane(mut self, lane: Option<studio_model::LaneId>) -> ModelScene {
        self.only_lane = lane;
        self
    }

    /// Shows this scene through `view` instead of the project's view
    /// transform (a per-viewer display setting).
    pub fn with_view(mut self, view: Option<ViewTransform>) -> ModelScene {
        self.view_override = view;
        self
    }

    fn sequence(&self) -> Option<&Sequence> {
        self.project.sequence(self.sequence)
    }

    /// Stacking order of a lane: visual lanes above the spine count up from
    /// 1, the spine is 0; audio lanes have no pictures.
    fn order_of(&self, seq: &Sequence, clip: &Clip) -> Option<i32> {
        let lane = seq.lane(clip.lane)?;
        if lane.hidden || lane.kind == LaneKind::Audio {
            return None;
        }
        seq.lane_position(clip.lane)
    }

    /// Builds a clip description at sequence time `t`. `source_time`
    /// overrides the clip's own mapping (for transition handles).
    fn describe(&self, seq: &Sequence, clip: &Clip, order: i32, t: Micros, source_time: Option<Micros>) -> Option<ClipDesc> {
        // Its drivers worked out for this moment.
        let driven = seq.driven(clip, t);
        let clip: &Clip = &driven;
        let rel = t - clip.start;
        let source = match &clip.source {
            ClipSource::Gap => return None,
            ClipSource::Media(m) => {
                if !m.video {
                    return None;
                }
                let item = self.project.media_item(m.media)?;
                if !item.has_video() {
                    return None;
                }
                SourceDesc::Media { media: MediaKey(m.media.0), time: source_time.unwrap_or_else(|| clip.source_time_at(t)) }
            }
            ClipSource::Sequence(s) => {
                // A multicam clip shows the lane of its chosen angle.
                let angle = clip.multicam_angle();
                let nested: Arc<dyn Scene> = Arc::new(ModelScene::nested(self.project.clone(), s.sequence).only_lane(angle));
                SourceDesc::Nested { scene: nested, time: source_time.unwrap_or_else(|| clip.source_time_at(t)) }
            }
            ClipSource::Generator(g) => match g.kind {
                MGen::Adjustment => SourceDesc::Adjustment,
                MGen::Camera => SourceDesc::Camera(self.camera(seq, clip, t)),
                MGen::Light => SourceDesc::Light(self.light(seq, clip, t)),
                MGen::Model => {
                    let mut params = convert_params(&g.params);
                    for (k, v) in clip.keyframes.keys().filter_map(|k| k.strip_prefix("generator.").map(|p| (p.to_string(), k))) {
                        if let Some(val) = clip.property_at(v, rel) {
                            params.insert(k, convert_value(&val));
                        }
                    }
                    // A walkable world: drawn through the scene camera.
                    let world_file = params.text("world", "").to_string();
                    if !world_file.is_empty() {
                        let world = crate::world_scene::load(&world_file)?;
                        let q = match params.text("quality", "balanced") {
                            "fast" => crate::realtime::Quality::Fast,
                            "high" => crate::realtime::Quality::High,
                            _ => crate::realtime::Quality::Balanced,
                        };
                        let tod = params.get("time_of_day").and_then(Param::as_f32);
                        SourceDesc::World(crate::world_scene::WorldDesc { world, camera: None, time: rel as f32 / 1e6, time_of_day: tod, metres_per_px: params.f("metres_per_px", crate::world_scene::DEFAULT_METRES_PER_PX), quality: q, carrier: None, toon: toon_of(&params) })
                    } else {
                    let file = params.text("file", "").to_string();
                    let mesh = crate::mesh_render::load(&file)?;
                    // Posed by its rig, or played from a baked simulation.
                    let base = mesh.clone();
                    let (mesh, key) = crate::model_anim::animate(mesh, &file, &params, rel as f32 / 1e6);
                    // Where it is a moment later, for motion blur.
                    let motion = self.model_motion(seq, clip, &g.params, &base, &file, t);
                    let overrides = crate::mesh_render::MaterialOverride {
                        base_colour: params.get("base_colour").and_then(Param::as_colour),
                        metallic: params.get("metallic").and_then(Param::as_f32),
                        roughness: params.get("roughness").and_then(Param::as_f32),
                        emissive: params.get("emissive").and_then(Param::as_colour).map(|c| [c[0], c[1], c[2]]),
                        opacity: params.get("opacity").and_then(Param::as_f32),
                        subsurface: params.get("subsurface").and_then(Param::as_f32),
                        wetness: params.get("wetness").and_then(Param::as_f32),
                        // A glow can be brighter than white: it blooms.
                        emissive_strength: params.get("emissive_strength").and_then(Param::as_f32),
                        toon: toon_of(&params),
                    };
                    // Placed, lit and seen when the frame is resolved.
                    SourceDesc::Mesh(crate::mesh_render::MeshDesc {
                        instances: vec![crate::mesh_render::MeshInstance {
                            id: clip.id.0,
                            mesh,
                            key,
                            world: [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]],
                            overrides,
                            smooth: params.f("smooth", 1.0),
                            accepts_lights: true,
                            casts_shadows: true,
                            accepts_shadows: true,
                            motion,
                            // A math shape drawn from its formula (`math_recipe`), when asked.
                            direct: if params.text("math_render", "mesh") == "direct" { crate::direct::from_recipe(params.text("math_recipe", ""), rel as f32 / 1e6) } else { None },
                        }],
                        camera: None,
                        lights: Vec::new(),
                        ..Default::default()
                    })
                    }
                }
                kind => {
                    let mut params = convert_params(&g.params);
                    for (k, v) in clip.keyframes.keys().filter_map(|k| k.strip_prefix("generator.").map(|p| (p.to_string(), k))) {
                        if let Some(val) = clip.property_at(v, rel) {
                            params.insert(k, convert_value(&val));
                        }
                    }
                    // Animated text and shape layers need to know where in
                    // the clip they are; shape layers also the canvas their
                    // units are pixels of.
                    if params.contains_key("motion") || params.contains_key("vector") || params.contains_key("art") {
                        params.insert("clip_time".into(), Param::Float(rel as f64 / 1e6));
                        params.insert("clip_duration".into(), Param::Float(clip.duration as f64 / 1e6));
                    }
                    if params.contains_key("vector") || params.contains_key("art") {
                        params.insert("canvas_width".into(), Param::Float(self.project.settings.width as f64));
                        params.insert("canvas_height".into(), Param::Float(self.project.settings.height as f64));
                    }
                    SourceDesc::Generator(GeneratorDesc {
                        kind: match kind {
                            MGen::Solid => GeneratorKind::Solid,
                            MGen::Gradient => GeneratorKind::Gradient,
                            MGen::Text => GeneratorKind::Text,
                            _ => GeneratorKind::Shape,
                        },
                        params,
                        text: g.text.iter().map(|r| TextRun { text: r.text.clone(), style: convert_params(&r.style) }).collect(),
                    })
                }
            },
        };
        // Its own transform, carried by its parents, turned by auto-orient.
        let tr = sp::oriented_transform(seq, clip, t, self.project.settings.width as f64);
        let placement = Placement {
            position: [tr.position.x as f32, tr.position.y as f32],
            scale: [tr.scale_x as f32, tr.scale_y as f32],
            rotation: tr.rotation as f32,
            anchor: [tr.anchor.x as f32, tr.anchor.y as f32],
            opacity: tr.opacity as f32,
            crop: [tr.crop.left as f32, tr.crop.top as f32, tr.crop.right as f32, tr.crop.bottom as f32],
            flip_x: tr.flip_x,
            flip_y: tr.flip_y,
        };
        let effects = clip
            .effects
            .iter()
            .filter(|e| e.enabled)
            .map(|e| EffectDesc { kind: e.kind.clone(), params: convert_params(&clip.effect_params_at(e, rel)) })
            .collect();
        let grade = clip.grade.as_ref().filter(|g| g.enabled).map(|g| GradeDesc { params: convert_params(&clip.grade_params_at(rel)), lut: g.lut.clone() });
        // Masks a grade node uses as its window shape that node only.
        let windows = clip.grade.as_ref().map(|g| g.window_masks()).unwrap_or_default();
        let mask_spec = |m: &studio_model::Mask| convert_mask(&clip.mask_at(m, rel), &convert_params(&clip.mask_params_at(m, rel)));
        let masks = clip.masks.iter().filter(|m| !windows.contains(&m.id)).map(mask_spec).collect();
        let grade_nodes = clip
            .grade
            .as_ref()
            .filter(|g| g.enabled)
            .map(|g| g.nodes())
            .unwrap_or_default()
            .into_iter()
            .filter(|n| n.enabled && n.mix > 0.0)
            .map(|n| crate::nodes::GradeNodeDesc {
                grade: GradeDesc { params: convert_params(&n.params), lut: n.lut.clone() },
                qualifier: crate::nodes::qualifier_from(&convert_params(&n.qualifier)),
                window: n.window.and_then(|id| clip.masks.iter().find(|m| m.id == id)).map(mask_spec),
                window_invert: n.window_invert,
                mix: n.mix.clamp(0.0, 1.0) as f32,
                key_blur: n.key_blur.max(0.0) as f32,
            })
            .collect();
        let warp = convert_warp(clip, rel);
        let motion = MotionDesc {
            entry: clip.motion.entry.as_ref().map(convert_motion),
            exit: clip.motion.exit.as_ref().map(convert_motion),
            looping: clip.motion.looping.as_ref().map(convert_motion),
        };
        Some(ClipDesc {
            id: clip.id.0,
            order,
            start: clip.start,
            duration: clip.duration,
            source,
            placement,
            blend: convert_blend(clip.blend),
            effects,
            grade,
            grade_nodes,
            masks,
            warp,
            matte: match &clip.source {
                ClipSource::Media(m) => matte_of(clip, m.media.0),
                _ => None,
            },
            shadow: shadow_of(clip, rel, self.project.settings.height as f32),
            // A model is always in space.
            space: (tr.three_d || matches!(&clip.source, ClipSource::Generator(g) if g.kind == MGen::Model)).then(|| space_of(&tr, &clip.material_at(rel))),
            motion,
            seed: clip.id.0 as u32,
        })
    }

    /// The camera clip's view at `t`.
    fn camera(&self, seq: &Sequence, clip: &Clip, t: Micros) -> CameraDesc {
        let width = self.project.settings.width as f64;
        let v = sp::camera_view(seq, clip, t, width);
        let set = sp::CameraSettings::read(&clip.generator_params_at((t - clip.start).clamp(0, clip.duration)), width);
        CameraDesc {
            position: v.position.map(|x| x as f32),
            axes: v.axes.map(|r| r.map(|x| x as f32)),
            zoom: v.zoom as f32,
            dof: set.dof.then_some(Dof { focus: set.focus_distance as f32, aperture: set.aperture as f32, blur_level: set.blur_level as f32 }),
            trace: trace_of(&clip.generator_params_at((t - clip.start).clamp(0, clip.duration))),
        }
    }

    /// The light clip at `t`.
    fn light(&self, seq: &Sequence, clip: &Clip, t: Micros) -> LightDesc {
        let tr = seq.placed_transform(clip, t);
        let gp = clip.generator_params_at((t - clip.start).clamp(0, clip.duration));
        let set = sp::LightSettings::read(&gp);
        let pos = [tr.position.x, tr.position.y, tr.position_z];
        // Aimed at its point of interest; one sitting on it aims ahead.
        let d = [set.poi[0] - pos[0], set.poi[1] - pos[1], set.poi[2] - pos[2]];
        let n = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        let dir = if n > 1e-6 { d.map(|v| v / n) } else { sp::apply(&sp::rotation(tr.orientation, tr.rotation_x, tr.rotation_y, tr.rotation), [0.0, 0.0, 1.0]) };
        let c = set.colour.to_linear();
        let half = set.cone_angle.to_radians() * 0.5;
        LightDesc {
            kind: match set.kind {
                sp::LightKind::Parallel => LightKind::Parallel,
                sp::LightKind::Spot => LightKind::Spot,
                sp::LightKind::Point => LightKind::Point,
                sp::LightKind::Ambient => LightKind::Ambient,
            },
            position: pos.map(|v| v as f32),
            direction: dir.map(|v| v as f32),
            colour: [(c[0] * set.intensity) as f32, (c[1] * set.intensity) as f32, (c[2] * set.intensity) as f32],
            cone: [half.cos() as f32, (half * (1.0 - set.cone_feather)).cos().min(1.0) as f32],
            radius: set.radius as f32,
            falloff: set.falloff as f32,
            casts_shadows: set.casts_shadows,
            shadow_darkness: set.shadow_darkness as f32,
            sky: None,
            look: None,
        }
        .with_sky(&gp, set.intensity as f32)
        .with_look(&gp, clip.id.0, t as f32 / 1e6)
    }

    /// A model layer a moment after `t` (a fraction of a frame given by the
    /// active camera's shutter), when the camera path-traces with motion
    /// blur.
    fn model_motion(&self, seq: &Sequence, clip: &Clip, params: &studio_model::ParamMap, base: &Arc<studio_mesh::Mesh>, file: &str, t: Micros) -> Option<Arc<crate::mesh_render::Motion>> {
        let cam = sp::active_camera(seq, t)?;
        let shutter = trace_of(&cam.generator_params_at((t - cam.start).clamp(0, cam.duration)))?.shutter;
        if shutter <= 0.0 {
            return None;
        }
        let fps = self.project.settings.frame_rate.as_f64().max(1.0);
        let t1 = t + (shutter as f64 / fps * 1e6).round().max(1.0) as Micros;
        let rel = t1 - clip.start;
        let mut p = convert_params(params);
        for (k, v) in clip.keyframes.keys().filter_map(|k| k.strip_prefix("generator.").map(|p| (p.to_string(), k))) {
            if let Some(val) = clip.property_at(v, rel) {
                p.insert(k, convert_value(&val));
            }
        }
        let (mesh, key) = crate::model_anim::animate(base.clone(), file, &p, rel as f32 / 1e6);
        let tr = sp::oriented_transform(seq, clip, t1, self.project.settings.width as f64);
        let placement = Placement {
            position: [tr.position.x as f32, tr.position.y as f32],
            scale: [tr.scale_x as f32, tr.scale_y as f32],
            rotation: tr.rotation as f32,
            anchor: [tr.anchor.x as f32, tr.anchor.y as f32],
            opacity: tr.opacity as f32,
            crop: [tr.crop.left as f32, tr.crop.top as f32, tr.crop.right as f32, tr.crop.bottom as f32],
            flip_x: tr.flip_x,
            flip_y: tr.flip_y,
        };
        Some(Arc::new(crate::mesh_render::Motion { mesh, key, placement, space: space_of(&tr, &clip.material_at(rel)), world: [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]] }))
    }

    /// Source time for a transition's handles: footage runs on past the
    /// clip's edge; a merged clip runs on through its contents and holds
    /// its first or last picture beyond them.
    fn handle_time(&self, clip: &Clip, t: Micros) -> Micros {
        let st = Self::extended_source_time(clip, t);
        match &clip.source {
            ClipSource::Sequence(s) => match self.project.sequence(s.sequence) {
                Some(inner) => st.clamp(0, (inner.duration() - 1).max(0)),
                None => st,
            },
            _ => st,
        }
    }

    /// Source time of a clip at sequence time `t`, allowing `t` outside the
    /// clip (transition handles): the speed curve is integrated past the
    /// clip's ends.
    fn extended_source_time(clip: &Clip, t: Micros) -> Micros {
        let rel = t - clip.start;
        if rel >= 0 && rel <= clip.duration {
            return clip.source_time_at(t);
        }
        let len = clip.speed.integrate(clip.duration);
        let off = if rel < 0 { -clip.speed.integrate(-rel) } else { clip.speed.integrate(rel) };
        if clip.reversed {
            (clip.source_in as f64 + (len - off - 1.0)).round() as Micros
        } else {
            (clip.source_in as f64 + off).round() as Micros
        }
    }
}

fn convert_blend(b: MBlend) -> BlendMode {
    match b {
        MBlend::Normal => BlendMode::Normal,
        MBlend::Add => BlendMode::Add,
        MBlend::Multiply => BlendMode::Multiply,
        MBlend::Screen => BlendMode::Screen,
        MBlend::Overlay => BlendMode::Overlay,
        MBlend::SoftLight => BlendMode::SoftLight,
        MBlend::HardLight => BlendMode::HardLight,
        MBlend::ColourDodge => BlendMode::ColourDodge,
        MBlend::ColourBurn => BlendMode::ColourBurn,
        MBlend::Darken => BlendMode::Darken,
        MBlend::Lighten => BlendMode::Lighten,
        MBlend::Difference => BlendMode::Difference,
        MBlend::Exclusion => BlendMode::Exclusion,
        MBlend::Hue => BlendMode::Hue,
        MBlend::Saturation => BlendMode::Saturation,
        MBlend::Colour => BlendMode::Colour,
        MBlend::Luminosity => BlendMode::Luminosity,
        MBlend::StencilAlpha => BlendMode::StencilAlpha,
        MBlend::SilhouetteAlpha => BlendMode::SilhouetteAlpha,
        MBlend::StencilLuma => BlendMode::StencilLuma,
        MBlend::SilhouetteLuma => BlendMode::SilhouetteLuma,
    }
}

/// A 3D layer's settings.
fn space_of(tr: &studio_model::Transform, m: &studio_model::Material) -> Space {
    Space {
        z: tr.position_z as f32,
        anchor_z: tr.anchor_z as f32,
        scale_z: tr.scale_z as f32,
        rotation_x: tr.rotation_x as f32,
        rotation_y: tr.rotation_y as f32,
        orientation: tr.orientation.map(|v| v as f32),
        material: MaterialDesc {
            accepts_lights: m.accepts_lights,
            casts_shadows: m.casts_shadows,
            accepts_shadows: m.accepts_shadows,
            ambient: m.ambient as f32,
            diffuse: m.diffuse as f32,
            specular: m.specular as f32,
            shininess: m.shininess as f32,
            extrude: (m.extrude * tr.scale_z) as f32,
            side_shade: m.side_shade as f32,
        },
    }
}

/// Converts a model value.
pub fn convert_value(v: &Value) -> Param {
    match v {
        Value::Bool(b) => Param::Bool(*b),
        Value::Scalar(x) | Value::Angle(x) => Param::Float(*x),
        Value::Point(p) => Param::Point([p.x, p.y]),
        Value::Colour(c) => Param::Colour(c.to_linear()),
        Value::Text(s) => Param::Text(s.clone()),
        Value::Choice(s) => Param::Choice(s.clone()),
        Value::Curve(c) => Param::Curve(c.clone()),
    }
}

/// Toon shading a model clip asks for: `shading` "toon" (also "cel",
/// "anime", "flat"), with `toon_bands` (2 or 3), `shadow_colour`,
/// `toon_threshold`, `rim`, `rim_colour`, `highlights`, `outline` (canvas
/// px, 0 none) and `outline_colour`.
fn toon_of(params: &Params) -> Option<crate::mesh_render::Toon> {
    let d = crate::mesh_render::Toon::default();
    let rgb = |k: &str, or: [f32; 3]| params.get(k).and_then(Param::as_colour).map_or(or, |c| [c[0], c[1], c[2]]);
    let toon = match params.text("shading", "") {
        "toon" | "cel" | "anime" | "cartoon" => d,
        "flat" => crate::mesh_render::Toon { bands: 2, threshold: 0.0, rim: 0.0, highlights: false, ..d },
        _ => return None,
    };
    Some(crate::mesh_render::Toon {
        bands: params.f("toon_bands", toon.bands as f32).clamp(2.0, 3.0) as u8,
        shadow: rgb("shadow_colour", toon.shadow),
        threshold: params.f("toon_threshold", toon.threshold).clamp(0.0, 1.0),
        rim: params.f("rim", toon.rim).clamp(0.0, 2.0),
        rim_colour: rgb("rim_colour", toon.rim_colour),
        highlights: params.flag("highlights", toon.highlights),
        outline: params.f("outline", toon.outline).clamp(0.0, 40.0),
        outline_colour: rgb("outline_colour", toon.outline_colour),
    })
}

/// Converts a model parameter map.
pub fn convert_params(m: &studio_model::ParamMap) -> Params {
    m.iter().map(|(k, v)| (k.clone(), convert_value(v))).collect()
}

fn convert_motion(m: &studio_model::MotionPreset) -> MotionRef {
    MotionRef { kind: m.kind.clone(), duration: m.duration, params: convert_params(&m.params) }
}

fn convert_pin(p: &studio_model::Pin) -> PinDesc {
    PinDesc {
        kind: match p.kind {
            studio_model::PinKind::Position => PinKind::Position,
            studio_model::PinKind::Bend => PinKind::Bend,
            studio_model::PinKind::Starch => PinKind::Starch,
            studio_model::PinKind::Overlap => PinKind::Overlap,
        },
        rest: [p.rest.x as f32, p.rest.y as f32],
        at: [p.position.x as f32, p.position.y as f32],
        rotation: p.rotation as f32,
        scale: p.scale as f32,
        amount: p.amount as f32,
        extent: p.extent as f32,
    }
}

/// The clip's puppet and corner pin at clip time `rel`; `None` when they
/// change nothing.
fn convert_warp(clip: &Clip, rel: Micros) -> Option<WarpDesc> {
    let w = warp_desc(clip, rel);
    (!w.is_identity()).then_some(w)
}

/// The warp of a clip at clip time `rel` whether or not it bends anything
/// yet (the canvas shows the mesh of an unmoved rig).
pub fn warp_desc(clip: &Clip, rel: Micros) -> WarpDesc {
    let puppet = clip.puppet_at(rel).filter(|p| p.enabled && !p.pins.is_empty()).map(|p| PuppetDesc { pins: p.pins.iter().map(convert_pin).collect(), density: p.density as f32, expansion: p.expansion as f32 });
    let pt = |v: studio_model::Vec2| [v.x as f32, v.y as f32];
    let corner_pin = clip.corner_pin_at(rel).filter(|c| c.enabled).map(|c| ([pt(c.rest[0]), pt(c.rest[1])], c.corners().map(pt)));
    let skin = clip.skeleton_at(rel).filter(|s| s.enabled && !s.bones.is_empty()).map(|s| {
        let posed = s.forward();
        SkinDesc {
            bones: s
                .bones
                .iter()
                .zip(&posed.bones)
                .map(|(b, p)| SkinBone { head: pt(b.head), tail: pt(b.tail), map: p.map.0.map(|v| v as f32), depth: b.depth as f32, deform: b.deform })
                .collect(),
            falloff: s.falloff as f32,
        }
    });
    let cuts = clip
        .cuts
        .iter()
        .filter(|c| c.is_active())
        .map(|c| CutDesc {
            points: c.points.iter().map(|p| pt(*p)).collect(),
            fill: match c.fill {
                studio_model::CutFill::None => FillDesc::None,
                studio_model::CutFill::Colour(k) => FillDesc::Colour(k.to_linear().map(|v| v as f32)),
                studio_model::CutFill::Auto => FillDesc::Auto,
            },
        })
        .collect();
    WarpDesc { puppet, skin, cuts, corner_pin }
}

fn convert_mask(m: &studio_model::Mask, p: &Params) -> MaskSpec {
    let shape = match m.shape {
        MShape::Rectangle => MaskShape::Rectangle,
        MShape::Ellipse => MaskShape::Ellipse,
        MShape::Line => MaskShape::Line,
        MShape::Mirror => MaskShape::Mirror,
        MShape::Star => MaskShape::Star,
        MShape::Heart => MaskShape::Heart,
        MShape::Path => MaskShape::Path(
            m.path
                .iter()
                .map(|pt| PathPoint {
                    point: [pt.point.x as f32, pt.point.y as f32],
                    handle_in: [pt.handle_in.x as f32, pt.handle_in.y as f32],
                    handle_out: [pt.handle_out.x as f32, pt.handle_out.y as f32],
                })
                .collect(),
        ),
    };
    let d = MaskSpec::default();
    MaskSpec {
        shape,
        mode: match m.mode {
            MMask::Add => MaskMode::Add,
            MMask::Subtract => MaskMode::Subtract,
            MMask::Intersect => MaskMode::Intersect,
        },
        invert: m.invert,
        centre: p.point("position", d.centre),
        size: p.point("size", d.size),
        rotation: p.f("rotation", 0.0),
        feather: p.f("feather", 0.0),
        expansion: p.f("expansion", 0.0),
        opacity: p.f("opacity", 1.0),
        roundness: p.f("roundness", 0.0),
        pins: if matches!(m.shape, MShape::Path) { m.pins.iter().map(convert_pin).collect() } else { Vec::new() },
    }
}

impl Scene for ModelScene {
    fn canvas(&self) -> Canvas {
        let s = &self.project.settings;
        let bg = s.background.to_linear();
        Canvas {
            width: s.width,
            height: s.height,
            frame_rate: s.frame_rate.as_f64(),
            background: if self.nested { [0.0; 4] } else { [(bg[0] * bg[3]) as f32, (bg[1] * bg[3]) as f32, (bg[2] * bg[3]) as f32, bg[3] as f32] },
            output: match s.colour_space {
                studio_model::ColourSpace::Rec709 => OutputSpace::Rec709,
                studio_model::ColourSpace::DisplayP3 => OutputSpace::DisplayP3,
                studio_model::ColourSpace::Rec2020Pq => OutputSpace::Rec2020Pq,
                studio_model::ColourSpace::Rec2020Hlg => OutputSpace::Rec2020Hlg,
            },
            // A compound clip's frame is part of a bigger picture: the view
            // is applied once, to the whole frame.
            view: if self.nested { ViewTransform::None } else { self.view_override.unwrap_or_else(|| project_view(&s.colour)) },
        }
    }

    fn duration(&self) -> Micros {
        self.sequence().map(|s| s.duration()).unwrap_or(0)
    }

    fn clips_at(&self, t: Micros) -> Vec<ClipDesc> {
        let Some(seq) = self.sequence() else { return Vec::new() };
        let in_transition: Vec<_> = seq
            .transitions
            .iter()
            .filter(|tr| transition_window(seq, tr).is_some_and(|(s, e)| t >= s && t < e))
            .flat_map(|tr| [tr.left, tr.right])
            .collect();
        seq.clips
            .iter()
            .filter(|c| c.enabled && t >= c.start && t < c.end() && !in_transition.contains(&c.id))
            .filter(|c| self.only_lane.is_none_or(|l| c.lane == l))
            .filter_map(|c| {
                let order = self.order_of(seq, c)?;
                self.describe(seq, c, order, t, None)
            })
            .collect()
    }

    fn transition_at(&self, t: Micros) -> Option<TransitionDesc> {
        let seq = self.sequence()?;
        for tr in &seq.transitions {
            let Some((start, end)) = transition_window(seq, tr) else { continue };
            if t < start || t >= end {
                continue;
            }
            let left = seq.clip(tr.left)?;
            let right = seq.clip(tr.right)?;
            let order = self.order_of(seq, left)?;
            let l = self.describe(seq, left, order, t, Some(self.handle_time(left, t)))?;
            let r = self.describe(seq, right, order, t, Some(self.handle_time(right, t)))?;
            return Some(TransitionDesc {
                kind: tr.kind.clone(),
                progress: ((t - start) as f64 / (end - start).max(1) as f64) as f32,
                params: convert_params(&tr.params),
                left: l,
                right: r,
                seed: tr.id.0 as u32,
            });
        }
        None
    }
}

/// The sequence-time window a transition covers.
fn transition_window(seq: &Sequence, tr: &studio_model::Transition) -> Option<(Micros, Micros)> {
    let left = seq.clip(tr.left)?;
    let cut = left.end();
    let (pre, post) = tr.split();
    Some((cut - pre, cut + post))
}

/// A clip's cut-out matte (`meta["matte"]`: `file`, the `media` it was made
/// for, `invert`, `enabled`), when it still shows that media.
pub fn matte_of(clip: &Clip, media: u64) -> Option<crate::scene::MatteDesc> {
    let o = clip.meta.get("matte")?.as_object()?;
    if o.get("media")?.as_u64()? != media || o.get("enabled").and_then(|v| v.as_bool()) == Some(false) {
        return None;
    }
    Some(crate::scene::MatteDesc { media: crate::scene::matte_key(o.get("file")?.as_str()?), invert: o.get("invert").and_then(|v| v.as_bool()).unwrap_or(false) })
}

/// A clip's ground shadow (`meta["shadow"]`: `angle`, `length`,
/// `opacity`, `softness`, `contact`, `colour` as sRGB 0..1, `feet` as
/// layer coordinates, `ground` as canvas px from the centre, `enabled`).
/// Without `feet`, the feet are the lowest of its puppet's foot pins (at
/// rest), or the bottom middle of its picture.
pub fn shadow_of(clip: &Clip, rel: Micros, canvas_height: f32) -> Option<crate::shadow::ShadowDesc> {
    let o = clip.meta.get("shadow")?.as_object()?;
    if o.get("enabled").and_then(|v| v.as_bool()) == Some(false) {
        return None;
    }
    let num = |k: &str, d: f32| o.get(k).and_then(|v| v.as_f64()).map_or(d, |v| v as f32);
    let pair = |k: &str| -> Option<[f32; 2]> {
        let a = o.get(k)?.as_array()?;
        Some([a.first()?.as_f64()? as f32, a.get(1)?.as_f64()? as f32])
    };
    let d = crate::shadow::ShadowDesc::default();
    let feet = pair("feet").or_else(|| {
        let p = clip.puppet_at(rel)?;
        let low: Vec<[f32; 2]> = p
            .pins
            .iter()
            .filter(|pin| ["foot", "feet", "toe", "paw", "hoof"].iter().any(|w| pin.name.to_lowercase().contains(w)))
            .map(|pin| [pin.rest.x as f32, pin.rest.y as f32])
            .collect();
        let bottom = low.iter().map(|q| q[1]).fold(f32::MIN, f32::max);
        let near: Vec<&[f32; 2]> = low.iter().filter(|q| q[1] > bottom - 0.06).collect();
        (!near.is_empty()).then(|| [near.iter().map(|q| q[0]).sum::<f32>() / near.len() as f32, bottom])
    });
    let colour = o.get("colour").and_then(|v| v.as_array()).filter(|a| a.len() >= 3).map(|a| crate::shadow::srgb_to_linear([0, 1, 2].map(|i| a[i].as_f64().unwrap_or(0.0) as f32)));
    Some(crate::shadow::ShadowDesc {
        feet: feet.unwrap_or(d.feet),
        ground: o.get("ground").and_then(|v| v.as_f64()).map(|g| g as f32 + canvas_height * 0.5),
        angle: num("angle", d.angle),
        length: num("length", d.length).max(0.0),
        opacity: num("opacity", d.opacity).clamp(0.0, 1.0),
        softness: num("softness", d.softness).max(0.0),
        contact: num("contact", d.contact).clamp(0.0, 1.0),
        colour: colour.unwrap_or(d.colour),
    })
}

/// Registers every media item's file (and chosen input colour space)
/// with the source registry.
pub fn register_media(project: &Project, registry: &SourceRegistry) {
    for item in &project.media {
        registry.register_path(MediaKey(item.id.0), item.path.clone());
        registry.set_input(MediaKey(item.id.0), media_input(project, item));
    }
}

/// The input colour space of a media item, if one is chosen and valid.
pub fn media_input(project: &Project, item: &studio_model::MediaItem) -> Option<InputColour> {
    let spec = item.input_colour.as_deref()?;
    let cfg = project.settings.colour.ocio_config.as_deref().map(std::path::Path::new);
    InputColour::parse(spec, cfg).ok()
}

/// The project's view transform (`None` when unset or invalid).
pub fn project_view(c: &studio_model::ColourManagement) -> ViewTransform {
    let cfg = c.ocio_config.as_deref().map(std::path::Path::new);
    ViewTransform::parse(&c.view, cfg, c.hdr_peak).unwrap_or_default()
}

/// A camera's path-tracing settings: `render` "path" (or "path_traced"),
/// `samples`, `preview_samples`, `bounces`, `denoise`, `shutter`.
fn trace_of(p: &studio_model::ParamMap) -> Option<crate::trace::TraceSettings> {
    let mode = p.get("render").and_then(Value::as_str).unwrap_or("raster");
    if !matches!(mode, "path" | "path_traced" | "path_tracing" | "pathtrace") {
        return None;
    }
    let num = |k: &str| p.get(k).and_then(Value::as_f64);
    let d = crate::trace::TraceSettings::default();
    Some(crate::trace::TraceSettings {
        samples: num("samples").map_or(d.samples, |v| v.clamp(1.0, 16384.0) as u32),
        preview_samples: num("preview_samples").map_or(d.preview_samples, |v| v.clamp(1.0, 4096.0) as u32),
        bounces: num("bounces").map_or(d.bounces, |v| v.clamp(1.0, 32.0) as u32),
        denoise: !matches!(p.get("denoise"), Some(Value::Bool(false))),
        clamp: num("clamp").map_or(d.clamp, |v| v.max(1.0) as f32),
        shutter: num("shutter").map_or(0.0, |v| v.clamp(0.0, 1.0) as f32),
    })
}

impl LightDesc {
    /// How the light shows itself (glow, beam, flame), its flicker, and,
    /// on the light that carries the scene's air, the air and look (see
    /// [`crate::atmosphere::params`]).
    fn with_look(mut self, p: &studio_model::ParamMap, id: u64, secs: f32) -> LightDesc {
        let num = |k: &str| p.get(k).and_then(Value::as_f64).map(|v| v as f32);
        if let Some(amount) = num("flicker").filter(|a| *a > 0.0) {
            let k = crate::atmosphere::params::flicker(secs, id, amount, num("flicker_speed").unwrap_or(7.0));
            self.colour = crate::space::scale(self.colour, k);
        }
        self.look = crate::atmosphere::params::read(p, id, secs);
        self
    }

    /// A light that brings a sky (`sky` on, or a panorama `hdri`): its
    /// daylight from the time of day (`time` hours, `day` of the year,
    /// `latitude`, `heading` of north), `turbidity`, `sky_strength`,
    /// `ground` colour, `background`, `haze`; a parallel light becomes the
    /// sun, pointed and coloured by the sky.
    fn with_sky(mut self, p: &studio_model::ParamMap, intensity: f32) -> LightDesc {
        let on = matches!(p.get("sky"), Some(Value::Bool(true)));
        let hdri = p.get("hdri").and_then(Value::as_str).filter(|s| !s.is_empty());
        if !on && hdri.is_none() {
            return self;
        }
        let num = |k: &str, d: f64| p.get(k).and_then(Value::as_f64).unwrap_or(d) as f32;
        let mut sky = crate::sky::SkyDesc {
            turbidity: num("turbidity", 3.0).clamp(1.7, 12.0),
            strength: num("sky_strength", 1.0).max(0.0),
            background: !matches!(p.get("background"), Some(Value::Bool(false))),
            haze: num("haze", 0.0).max(0.0),
            hdri_turn: num("hdri_turn", 0.0),
            ..Default::default()
        };
        if let Some(c) = p.get("ground").and_then(Value::as_colour) {
            let l = c.to_linear();
            sky.ground = [l[0] as f32, l[1] as f32, l[2] as f32];
        }
        if let Some(path) = hdri {
            sky.hdri = crate::sky::load_hdri(path).unwrap_or(0);
        }
        if p.get("time").is_some() {
            sky.sun = crate::sky::sun_for_time(num("time", 12.0), num("day", 172.0), num("latitude", 40.0), num("heading", 0.0));
        } else {
            sky.sun = crate::space::normalise(self.direction);
        }
        if self.kind == LightKind::Parallel && sky.hdri == 0 {
            self.direction = sky.sun;
            self.colour = crate::space::scale(crate::sky::sun_colour(&sky), intensity);
        }
        if self.kind == LightKind::Ambient && sky.hdri != 0 {
            // Flat layers lit by an ambient light see the panorama's average.
            if let Some(h) = crate::sky::hdri(sky.hdri) {
                self.colour = crate::space::scale(h.average(), sky.strength);
            }
        }
        self.sky = Some(sky);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_model::{ClipId, Generator, LaneId, ProjectSettings, Rgba, Transition, TransitionAlign, TransitionId};

    #[test]
    fn project_round_trip() {
        let mut project = Project::new(ProjectSettings::default());
        let seq_id = project.active_sequence;
        let spine = project.active().spine_id();
        let mut g = Generator::new(MGen::Solid);
        g.params.insert("colour".into(), Value::Colour(Rgba::new(1.0, 0.0, 0.0, 1.0)));
        let a = Clip::new(ClipId(10), spine, 0, 2_000_000, ClipSource::Generator(g.clone()));
        let mut b = Clip::new(ClipId(11), spine, 2_000_000, 2_000_000, ClipSource::Generator(g));
        b.transform.rotation = 30.0;
        b.blend = MBlend::Screen;
        let seq = project.sequences.iter_mut().find(|s| s.id == seq_id).unwrap();
        seq.clips.push(a);
        seq.clips.push(b);
        seq.transitions.push(Transition {
            id: TransitionId(1),
            left: ClipId(10),
            right: ClipId(11),
            kind: "dissolve".into(),
            duration: 1_000_000,
            align: TransitionAlign::Centre,
            params: Default::default(),
            extra: Default::default(),
        });
        let scene = ModelScene::new(Arc::new(project));
        assert_eq!(scene.canvas().width, 1920);
        let clips = scene.clips_at(500_000);
        assert_eq!(clips.len(), 1);
        assert_eq!(clips[0].id, 10);
        assert!(scene.transition_at(500_000).is_none());
        let tr = scene.transition_at(2_250_000).expect("transition");
        assert!((tr.progress - 0.75).abs() < 1e-6);
        assert_eq!(tr.right.blend, BlendMode::Screen);
        assert_eq!(tr.right.placement.rotation, 30.0);
        assert!(scene.clips_at(2_250_000).is_empty());
        let _ = LaneId(0);
    }

    #[test]
    fn cameras_lights_and_3d_layers() {
        use studio_model::space as msp;
        let mut project = Project::new(ProjectSettings::default());
        let spine = project.active().spine_id();
        let mut card = Clip::new(ClipId(1), spine, 0, 2_000_000, ClipSource::Generator(Generator::new(MGen::Solid)));
        card.transform.three_d = true;
        card.transform.rotation_y = 30.0;
        card.transform.auto_orient = msp::AutoOrient::Camera;
        let mut cam_g = Generator::new(MGen::Camera);
        cam_g.params = msp::CameraSettings::defaults(1920.0);
        cam_g.params.insert("two_node".into(), Value::Bool(true));
        let mut cam = Clip::new(ClipId(2), spine, 0, 2_000_000, ClipSource::Generator(cam_g));
        cam.transform.position = studio_model::Vec2::new(800.0, 0.0);
        cam.transform.position_z = -2000.0;
        let mut light_g = Generator::new(MGen::Light);
        light_g.params = msp::LightSettings::defaults(msp::LightKind::Spot);
        let light = Clip::new(ClipId(3), spine, 0, 2_000_000, ClipSource::Generator(light_g));
        let seq = project.sequences.iter_mut().next().unwrap();
        let lane = studio_model::Lane::new(LaneId(77), studio_model::LaneKind::Visual, "Top");
        seq.lanes.insert(0, lane);
        seq.clips.push(card);
        cam.lane = LaneId(77);
        seq.clips.push(cam);
        let mut light = light;
        light.lane = LaneId(77);
        light.start = 0;
        seq.clips.push(light);
        let scene = ModelScene::new(Arc::new(project));
        let list = crate::resolve::resolve(&scene, 0);
        let cam = list.camera.expect("camera picked");
        assert_eq!(cam.position, [800.0, 0.0, -2000.0]);
        // Aimed at the centre: forward points back at the origin.
        assert!(cam.axes[2][0] < -0.3 && cam.axes[2][2] > 0.8, "{:?}", cam.axes);
        assert_eq!(list.lights.len(), 1);
        let layer = list.nodes.iter().find_map(|n| match n {
            crate::resolve::Node::Layer { clip, .. } => Some(clip.clone()),
            _ => None,
        });
        let layer = layer.expect("the card draws");
        let space = layer.space.expect("in 3D");
        // Faces the camera (off to the right: the right edge turns away by
        // atan(800 / 2000)), its own 30° on top.
        let facing = (800f32 / 2000.0).atan().to_degrees();
        assert!((space.rotation_y - (facing + 30.0)).abs() < 0.1, "{}", space.rotation_y);
    }
}
