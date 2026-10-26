// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Scenes shared by the agreement and golden tests.

#![allow(dead_code)]

use std::sync::Arc;

use studio_render::mask::{MaskMode, MaskShape, MaskSpec, PathPoint};
use studio_render::puppet::{PinDesc, PinKind, PuppetDesc, WarpDesc};
use studio_render::scene::{Canvas, ClipDesc, EffectDesc, GeneratorDesc, GeneratorKind, GradeDesc, Micros, MotionDesc, MotionRef, Param, Params, SECOND, Scene, SourceDesc, StaticScene, TransitionDesc};
use studio_render::source::{DecodeCache, ProceduralSource, SourceRegistry, SourceSpec, Sources};
use studio_render::space::{CameraDesc, Dof, LightDesc, LightKind, Space};
use studio_render::{BlendMode, MediaKey};

/// A scene whose media clips play their source from the clip start, so
/// the test pattern moves with time.
pub struct Moving(pub StaticScene);

impl Scene for Moving {
    fn canvas(&self) -> Canvas {
        self.0.canvas()
    }
    fn duration(&self) -> Micros {
        self.0.duration()
    }
    fn clips_at(&self, t: Micros) -> Vec<ClipDesc> {
        let mut v = self.0.clips_at(t);
        for c in &mut v {
            if let SourceDesc::Media { time, .. } = &mut c.source {
                *time = t - c.start;
            }
        }
        v
    }
    fn transition_at(&self, t: Micros) -> Option<TransitionDesc> {
        let mut tr = self.0.transition_at(t)?;
        for c in [&mut tr.left, &mut tr.right] {
            if let SourceDesc::Media { time, .. } = &mut c.source {
                *time = t - c.start;
            }
        }
        Some(tr)
    }
}

pub fn params(kv: &[(&str, Param)]) -> Params {
    kv.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()
}

pub fn solid(id: u64, order: i32, start: Micros, dur: Micros, rgba: [f64; 4]) -> ClipDesc {
    ClipDesc::new(id, order, start, dur, SourceDesc::Generator(GeneratorDesc { kind: GeneratorKind::Solid, params: params(&[("colour", Param::Colour(rgba))]), text: vec![] }))
}

pub fn gradient(id: u64, order: i32, start: Micros, dur: Micros, a: [f64; 4], b: [f64; 4], angle: f64) -> ClipDesc {
    ClipDesc::new(
        id,
        order,
        start,
        dur,
        SourceDesc::Generator(GeneratorDesc {
            kind: GeneratorKind::Gradient,
            params: params(&[("start", Param::Colour(a)), ("end", Param::Colour(b)), ("angle", Param::Float(angle))]),
            text: vec![],
        }),
    )
}

pub fn shape(id: u64, order: i32, start: Micros, dur: Micros, kind: &str, fill: [f64; 4]) -> ClipDesc {
    ClipDesc::new(
        id,
        order,
        start,
        dur,
        SourceDesc::Generator(GeneratorDesc {
            kind: GeneratorKind::Shape,
            params: params(&[("shape", Param::Choice(kind.into())), ("fill", Param::Colour(fill)), ("size", Param::Point([0.6, 0.6]))]),
            text: vec![],
        }),
    )
}

pub fn media(id: u64, order: i32, start: Micros, dur: Micros, key: MediaKey) -> ClipDesc {
    ClipDesc::new(id, order, start, dur, SourceDesc::Media { media: key, time: 0 })
}

pub fn effect(kind: &str, kv: &[(&str, Param)]) -> EffectDesc {
    EffectDesc { kind: kind.into(), params: params(kv) }
}

pub fn grade(kv: &[(&str, Param)]) -> GradeDesc {
    GradeDesc { params: params(kv), lut: None }
}

pub fn mask(shape: MaskShape, feather: f32) -> MaskSpec {
    MaskSpec { shape, feather, size: [0.6, 0.6], ..Default::default() }
}

pub fn path_mask() -> MaskShape {
    // A rounded blob: four points with symmetric handles.
    let h = 0.12;
    MaskShape::Path(vec![
        PathPoint { point: [0.5, 0.15], handle_in: [-h, 0.0], handle_out: [h, 0.0] },
        PathPoint { point: [0.85, 0.5], handle_in: [0.0, -h], handle_out: [0.0, h] },
        PathPoint { point: [0.5, 0.85], handle_in: [h, 0.0], handle_out: [-h, 0.0] },
        PathPoint { point: [0.2, 0.5], handle_in: [0.0, h], handle_out: [0.0, -h] },
    ])
}

/// Puppet pins bending a picture: one held, one dragged and one bent.
pub fn puppet_warp() -> WarpDesc {
    let mut bend = PinDesc::position([0.5, 0.35], [0.5, 0.35]);
    bend.kind = PinKind::Bend;
    bend.rotation = 25.0;
    let mut starch = PinDesc::position([0.3, 0.6], [0.3, 0.6]);
    starch.kind = PinKind::Starch;
    starch.amount = 0.8;
    WarpDesc {
        puppet: Some(PuppetDesc { pins: vec![PinDesc::position([0.25, 0.75], [0.25, 0.75]), PinDesc::position([0.8, 0.3], [0.9, 0.12]), bend, starch], density: 20.0, expansion: 0.0 }),
        ..WarpDesc::default()
    }
}

/// A corner pin with perspective.
pub fn corner_warp() -> WarpDesc {
    WarpDesc { corner_pin: Some(([[0.0, 0.0], [1.0, 1.0]], [[0.15, 0.05], [0.95, 0.15], [0.85, 0.95], [0.05, 0.8]])), ..WarpDesc::default() }
}

/// The path mask with a pin pulling its right side out.
pub fn pinned_mask() -> MaskSpec {
    MaskSpec { shape: path_mask(), feather: 0.05, pins: vec![PinDesc::position([0.2, 0.5], [0.2, 0.5]), PinDesc::position([0.85, 0.5], [0.95, 0.35])], ..Default::default() }
}

/// Registry with two moving test patterns: key 1 landscape, key 2 a small
/// portrait picture.
pub fn registry() -> Arc<SourceRegistry> {
    let reg = SourceRegistry::new();
    reg.register(MediaKey(1), SourceSpec::Procedural(ProceduralSource::test_pattern(128, 72, 30.0, 300)));
    reg.register(MediaKey(2), SourceSpec::Procedural(ProceduralSource::test_pattern(40, 64, 30.0, 300)));
    reg
}

pub fn sources(reg: &Arc<SourceRegistry>) -> Sources {
    Sources::new(reg.clone(), DecodeCache::new(64 << 20))
}

/// A busy scene that touches every stage of the pipeline: media with crop,
/// rotation, flip and minification; a feathered mask and a subtracted one;
/// a shader effect and a CPU-only effect; a grade; blend modes; a motion
/// preset; an adjustment layer with a mask; and a wipe transition on the
/// spine between 1.5 s and 2.5 s.
pub fn busy_scene() -> Moving {
    let mut canvas = Canvas::new(192, 108, 30.0);
    canvas.background = [0.02, 0.02, 0.05, 1.0];
    let mut a = media(1, 0, 0, 2 * SECOND, MediaKey(1));
    a.placement.crop = [0.1, 0.0, 0.0, 0.15];
    a.placement.scale = [1.1, 1.1];
    a.effects.push(effect("posterise", &[("levels", Param::Float(5.0))]));
    a.grade = Some(grade(&[("exposure", Param::Float(0.4)), ("saturation", Param::Float(1.3)), ("gain_luma", Param::Float(0.1))]));
    let mut b = media(2, 0, 2 * SECOND, 2 * SECOND, MediaKey(1));
    b.placement.flip_x = true;
    b.effects.push(effect("gaussian_blur", &[("radius", Param::Float(1.5))]));
    let mut small = media(3, 1, 0, 4 * SECOND, MediaKey(2));
    small.placement.scale = [0.6, 0.6];
    small.placement.rotation = 25.0;
    small.placement.position = [-40.0, 10.0];
    small.blend = BlendMode::Screen;
    small.masks.push(MaskSpec { shape: MaskShape::Ellipse, feather: 0.15, size: [0.9, 0.8], ..Default::default() });
    small.masks.push(MaskSpec { shape: MaskShape::Rectangle, mode: MaskMode::Subtract, size: [0.3, 0.3], centre: [0.7, 0.3], ..Default::default() });
    let mut tiny = media(4, 2, 0, 4 * SECOND, MediaKey(1));
    tiny.placement.scale = [0.2, 0.2];
    tiny.placement.position = [60.0, -30.0];
    tiny.placement.opacity = 0.8;
    tiny.blend = BlendMode::Add;
    tiny.motion = MotionDesc { entry: None, exit: None, looping: Some(MotionRef { kind: "float".into(), duration: 2 * SECOND, params: Params::new() }) };
    let mut adj = ClipDesc::new(5, 3, 0, 4 * SECOND, SourceDesc::Adjustment);
    adj.grade = Some(grade(&[("contrast", Param::Float(0.3)), ("temperature", Param::Float(5600.0))]));
    adj.masks.push(MaskSpec { shape: MaskShape::Line, rotation: -20.0, feather: 0.2, ..Default::default() });
    let mut txt = gradient(6, 4, 0, 4 * SECOND, [1.0, 0.8, 0.2, 1.0], [0.2, 0.4, 1.0, 0.0], 30.0);
    txt.placement.scale = [0.5, 0.15];
    txt.placement.position = [0.0, 40.0];
    txt.blend = BlendMode::Overlay;
    let scene = StaticScene::new(canvas, 4 * SECOND)
        .with_clip(a)
        .with_clip(b)
        .with_clip(small)
        .with_clip(tiny)
        .with_clip(adj)
        .with_clip(txt)
        .with_transition(1, 2, "wipe", 3 * SECOND / 2, SECOND, params(&[("softness", Param::Float(0.1))]));
    Moving(scene)
}

/// A light for the 3D scenes.
pub fn light(id: u64, kind: LightKind, position: [f32; 3], aim: [f32; 3], colour: [f32; 3], shadows: bool) -> ClipDesc {
    let d = [aim[0] - position[0], aim[1] - position[1], aim[2] - position[2]];
    let n = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-6);
    let half = 35f32.to_radians();
    ClipDesc::new(
        id,
        50,
        0,
        SECOND,
        SourceDesc::Light(LightDesc {
            kind,
            position,
            direction: d.map(|v| v / n),
            colour,
            cone: [half.cos(), (half * 0.5).cos()],
            radius: 0.0,
            falloff: 0.0,
            casts_shadows: shadows,
            shadow_darkness: 0.7,
            sky: None,
            look: None,
        }),
    )
}

/// A camera at `from` aimed at `to`.
pub fn camera(id: u64, canvas: &Canvas, from: [f32; 3], to: [f32; 3], dof: Option<Dof>) -> ClipDesc {
    let cam = CameraDesc { position: from, axes: studio_render::space::look_at(from, to, 0.0), zoom: studio_render::space::default_zoom(canvas.width as f32), dof, trace: None };
    ClipDesc::new(id, 60, 0, SECOND, SourceDesc::Camera(cam))
}

fn in_space(mut c: ClipDesc, f: impl FnOnce(&mut Space)) -> ClipDesc {
    let mut s = Space::default();
    f(&mut s);
    c.space = Some(s);
    c
}

/// The 3D cases shared by the goldens and the agreement test: perspective
/// turns, depth sorting against timeline order, a moved camera, lights
/// with shadows, extrusion, depth of field, a 3D motion preset on a 2D
/// layer, a 3D track matte and a nested scene holding 3D layers.
pub fn space_scenes(canvas: Canvas) -> Vec<(&'static str, StaticScene, Micros)> {
    let w = canvas.width as f32;
    let floor = || {
        let g = gradient(1, 0, 0, SECOND, [0.15, 0.2, 0.35, 1.0], [0.6, 0.55, 0.5, 1.0], 90.0);
        in_space(g, |s| {
            s.rotation_x = -80.0;
            s.z = 0.2 * w;
        })
    };
    let card = |id: u64, order: i32, rgba: [f64; 4]| {
        let mut c = shape(id, order, 0, SECOND, "rectangle", rgba);
        c.placement.scale = [0.5, 0.5];
        c
    };
    let scene = |clips: Vec<ClipDesc>| {
        let mut s = StaticScene::new(canvas, SECOND);
        s.clips = clips;
        s
    };
    let back = || gradient(9, -1, 0, SECOND, [0.05, 0.05, 0.08, 1.0], [0.2, 0.1, 0.25, 1.0], 0.0);
    let mut out = Vec::new();

    let turned = in_space(card(2, 1, [0.9, 0.5, 0.1, 1.0]), |s| {
        s.rotation_y = 50.0;
        s.rotation_x = -20.0;
    });
    out.push(("space_turn", scene(vec![back(), turned]), 0));

    // The later lane is further away: it must draw underneath.
    let mut near = card(2, 1, [0.1, 0.8, 0.3, 1.0]);
    near.placement.position = [-0.1 * w, 0.0];
    let near = in_space(near, |s| s.z = -0.1 * w);
    let mut far = card(3, 2, [0.9, 0.2, 0.3, 1.0]);
    far.placement.position = [0.1 * w, 0.05 * w];
    let far = in_space(far, |s| s.z = 0.4 * w);
    out.push(("space_depth_sort", scene(vec![back(), near, far]), 0));

    let a = in_space(card(2, 1, [0.9, 0.8, 0.2, 1.0]), |_| {});
    let mut b = card(3, 2, [0.2, 0.6, 1.0, 1.0]);
    b.placement.position = [0.15 * w, 0.0];
    let b = in_space(b, |s| s.z = 0.3 * w);
    let cam = camera(8, &canvas, [0.8 * w, -0.4 * w, -1.0 * w], [0.0, 0.0, 0.1 * w], None);
    out.push(("space_camera", scene(vec![back(), floor(), a, b, cam]), 0));

    let mut caster = card(2, 1, [0.95, 0.95, 0.95, 1.0]);
    caster.placement.scale = [0.3, 0.3];
    let caster = in_space(caster, |s| {
        s.z = -0.05 * w;
        s.rotation_y = 25.0;
        s.material.casts_shadows = true;
        s.material.specular = 0.6;
    });
    let wall = in_space(gradient(3, 0, 0, SECOND, [0.7, 0.7, 0.75, 1.0], [0.5, 0.55, 0.6, 1.0], 90.0), |s| s.z = 0.15 * w);
    let spot = light(10, LightKind::Spot, [-0.4 * w, -0.3 * w, -0.8 * w], [0.0, 0.0, 0.15 * w], [1.4, 1.3, 1.1], true);
    let amb = light(11, LightKind::Ambient, [0.0; 3], [0.0, 0.0, 1.0], [0.15, 0.15, 0.2], false);
    out.push(("space_lights_shadows", scene(vec![wall, caster, spot, amb]), 0));

    let pt = light(10, LightKind::Point, [0.3 * w, -0.3 * w, -0.4 * w], [0.0; 3], [1.0, 0.9, 0.8], false);
    let sun = light(11, LightKind::Parallel, [0.0; 3], [0.3, 0.2, 1.0], [0.4, 0.45, 0.6], false);
    let mut solid = shape(2, 1, 0, SECOND, "star", [1.0, 0.75, 0.2, 1.0]);
    solid.placement.scale = [0.6, 0.6];
    let solid = in_space(solid, |s| {
        s.rotation_y = -35.0;
        s.rotation_x = 15.0;
        s.material.extrude = 0.06 * w;
    });
    out.push(("space_extrude", scene(vec![back(), solid, pt, sun]), 0));

    let mut sharp = card(2, 1, [0.9, 0.9, 0.2, 1.0]);
    sharp.placement.position = [-0.15 * w, 0.0];
    let sharp = in_space(sharp, |_| {});
    let mut soft = card(3, 2, [0.3, 0.9, 0.9, 1.0]);
    soft.placement.position = [0.2 * w, 0.0];
    let soft = in_space(soft, |s| s.z = 0.6 * w);
    let zoom = studio_render::space::default_zoom(w);
    let cam = camera(8, &canvas, [0.0, 0.0, -zoom], [0.0, 0.0, 0.0], Some(Dof { focus: zoom, aperture: 0.08 * w, blur_level: 1.0 }));
    out.push(("space_dof", scene(vec![back(), sharp, soft, cam]), 0));

    let mut posed = card(2, 1, [0.6, 0.3, 0.9, 1.0]);
    posed.motion = MotionDesc { entry: Some(MotionRef { kind: "turn_in".into(), duration: SECOND, params: Params::new() }), exit: None, looping: None };
    out.push(("pose_turn_in", scene(vec![back(), posed]), SECOND / 3));

    let mut matte = shape(3, 2, 0, SECOND, "ellipse", [1.0, 1.0, 1.0, 1.0]);
    matte.blend = BlendMode::StencilAlpha;
    let matte = in_space(matte, |s| s.rotation_y = 55.0);
    out.push(("space_matte", scene(vec![back(), card(2, 1, [0.9, 0.4, 0.2, 1.0]), matte]), 0));

    let inner = scene(vec![in_space(card(1, 0, [0.2, 0.9, 0.5, 1.0]), |s| s.rotation_y = 40.0)]);
    let nested = in_space(ClipDesc::new(4, 1, 0, SECOND, SourceDesc::Nested { scene: Arc::new(inner), time: 0 }), |s| s.rotation_x = 30.0);
    out.push(("space_nested", scene(vec![back(), nested]), 0));
    out
}
