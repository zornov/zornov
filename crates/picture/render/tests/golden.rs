// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Golden images: small canvases rendered by the CPU reference and compared
//! with the PNGs in `tests/golden/`. One per blend mode, mask shape and
//! combine rule, transform case, transition, generator and text hook.
//!
//! Comparison is in 8-bit sRGB (as stored): at most 3/255 per channel on
//! 99.5 % of pixels. Regenerate after a deliberate look change with
//! `RENDER_BLESS=1 cargo test -p studio-render --test golden` and review
//! the PNG diffs.

mod common;

use std::path::PathBuf;
use std::sync::Arc;

use studio_render::compositor::FrameRenderer;
use studio_render::mask::{MaskMode, MaskShape, MaskSpec};
use studio_render::scene::{Canvas, ClipDesc, GeneratorDesc, GeneratorKind, MotionRef, Param, Params, SECOND, SourceDesc, StaticScene, TextRun};
use studio_render::text::{TextRenderer, set_text_renderer};
use studio_render::{BlendMode, CpuCompositor, Frame, MediaKey, OutputSettings};

const W: u32 = 96;
const H: u32 = 54;

fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

fn rgba8(f: &Frame) -> Vec<u8> {
    studio_render::output::convert_rgba8(f, &OutputSettings { dither: false, ..Default::default() }, 0)
}

/// Compares against (or writes) `name.png`. Returns a message on mismatch.
fn check(name: &str, frame: &Frame) -> Option<String> {
    assert!(frame.is_finite(), "{name}: non-finite pixels");
    let path = golden_dir().join(format!("{name}.png"));
    if std::env::var_os("RENDER_BLESS").is_some() {
        std::fs::create_dir_all(golden_dir()).unwrap();
        frame.save_png(&path).unwrap();
        return None;
    }
    let Ok(expected) = Frame::load_png(&path) else {
        return Some(format!("{name}: missing golden {} (run with RENDER_BLESS=1)", path.display()));
    };
    if (expected.width, expected.height) != (frame.width, frame.height) {
        return Some(format!("{name}: size {}x{} differs from golden {}x{}", frame.width, frame.height, expected.width, expected.height));
    }
    let (a, b) = (rgba8(frame), rgba8(&expected));
    // An (almost) invisible pixel's colour does not count: stored
    // unpremultiplied, it is noise divided by a tiny alpha.
    let differs = |x: u8, y: u8| (x as i32 - y as i32).abs() > 3;
    let bad = a
        .chunks_exact(4)
        .zip(b.chunks_exact(4))
        .filter(|(p, q)| if p[3] <= 3 && q[3] <= 3 { differs(p[3], q[3]) } else { p.iter().zip(*q).any(|(x, y)| differs(*x, *y)) })
        .count();
    let frac = bad as f64 / (frame.width * frame.height) as f64;
    if frac > 0.005 {
        let out = golden_dir().join(format!("{name}.actual.png"));
        frame.save_png(&out).unwrap();
        return Some(format!("{name}: {:.2}% of pixels differ from the golden (actual written to {})", frac * 100.0, out.display()));
    }
    None
}

fn compositor() -> CpuCompositor {
    CpuCompositor::new(common::sources(&common::registry()))
}

fn canvas() -> Canvas {
    let mut c = Canvas::new(W, H, 30.0);
    c.background = [0.05, 0.05, 0.06, 1.0];
    c
}

/// Backdrop: a horizontal gradient with a media picture in the middle.
fn backdrop() -> Vec<ClipDesc> {
    let mut pic = common::media(1, 0, 0, SECOND, MediaKey(1));
    pic.placement.scale = [0.75, 0.75];
    vec![common::gradient(10, -1, 0, SECOND, [0.9, 0.2, 0.1, 1.0], [0.1, 0.3, 0.9, 1.0], 0.0), pic]
}

/// Foreground: a vertical gradient fading out towards the bottom, with an
/// HDR highlight at the top.
fn foreground(id: u64, order: i32) -> ClipDesc {
    let mut c = common::gradient(id, order, 0, SECOND, [1.6, 1.4, 0.6, 1.0], [0.2, 0.9, 0.4, 0.0], 90.0);
    c.placement.scale = [0.8, 0.8];
    c.placement.rotation = -8.0;
    c
}

fn scene(clips: Vec<ClipDesc>) -> StaticScene {
    let mut s = StaticScene::new(canvas(), SECOND);
    s.clips = clips;
    s
}

fn report(failures: Vec<String>) {
    assert!(failures.is_empty(), "golden mismatches:\n  {}", failures.join("\n  "));
}

#[test]
fn blend_modes() {
    let mut c = compositor();
    let mut failures = Vec::new();
    for mode in BlendMode::ALL {
        let mut clips = backdrop();
        let mut fg = foreground(2, 1);
        fg.blend = mode;
        fg.placement.opacity = 0.9;
        clips.push(fg);
        let f = c.render(&scene(clips), 0, None);
        failures.extend(check(&format!("blend_{}", mode.id()), &f));
    }
    report(failures);
}

#[test]
fn masks() {
    let mut c = compositor();
    let mut failures = Vec::new();
    let shapes = [MaskShape::Rectangle, MaskShape::Ellipse, MaskShape::Line, MaskShape::Mirror, MaskShape::Star, MaskShape::Heart, common::path_mask()];
    for shape in shapes {
        let name = shape.id();
        let mut clips = backdrop();
        let mut fg = foreground(2, 1);
        fg.masks.push(MaskSpec { shape: shape.clone(), feather: 0.08, size: [0.6, 0.7], rotation: 15.0, roundness: 0.4, ..Default::default() });
        clips.push(fg);
        failures.extend(check(&format!("mask_{name}"), &c.render(&scene(clips), 0, None)));
    }
    // Hard edge, inverted, expanded, and the three combine rules.
    let variants: Vec<(&str, Vec<MaskSpec>)> = vec![
        ("mask_hard_edge", vec![MaskSpec { shape: MaskShape::Ellipse, size: [0.7, 0.7], ..Default::default() }]),
        ("mask_invert", vec![MaskSpec { shape: MaskShape::Ellipse, invert: true, feather: 0.1, size: [0.5, 0.5], ..Default::default() }]),
        ("mask_expand", vec![MaskSpec { shape: MaskShape::Star, expansion: 0.08, size: [0.5, 0.5], ..Default::default() }]),
        ("mask_opacity", vec![MaskSpec { shape: MaskShape::Rectangle, opacity: 0.5, size: [0.7, 0.5], ..Default::default() }]),
        ("mask_add", vec![
            MaskSpec { shape: MaskShape::Ellipse, size: [0.5, 0.5], centre: [0.35, 0.5], feather: 0.05, ..Default::default() },
            MaskSpec { shape: MaskShape::Rectangle, size: [0.4, 0.4], centre: [0.65, 0.5], ..Default::default() },
        ]),
        ("mask_subtract", vec![
            MaskSpec { shape: MaskShape::Ellipse, size: [0.8, 0.8], feather: 0.05, ..Default::default() },
            MaskSpec { shape: MaskShape::Star, mode: MaskMode::Subtract, size: [0.5, 0.5], ..Default::default() },
        ]),
        ("mask_intersect", vec![
            MaskSpec { shape: MaskShape::Ellipse, size: [0.8, 0.8], feather: 0.05, ..Default::default() },
            MaskSpec { shape: MaskShape::Rectangle, mode: MaskMode::Intersect, size: [0.9, 0.4], rotation: 30.0, ..Default::default() },
        ]),
        ("mask_subtract_first", vec![MaskSpec { shape: MaskShape::Heart, mode: MaskMode::Subtract, size: [0.5, 0.5], feather: 0.05, ..Default::default() }]),
    ];
    for (name, masks) in variants {
        let mut clips = backdrop();
        let mut fg = foreground(2, 1);
        fg.masks = masks;
        clips.push(fg);
        failures.extend(check(name, &c.render(&scene(clips), 0, None)));
    }
    report(failures);
}

/// Puppet pins, corner pins and pinned masks.
#[test]
fn pins() {
    let mut c = compositor();
    let mut failures = Vec::new();
    let mut both = common::puppet_warp();
    both.corner_pin = common::corner_warp().corner_pin;
    let cases: Vec<(&str, Box<dyn Fn(&mut ClipDesc)>)> = vec![
        ("pins_puppet", Box::new(|fg| fg.warp = Some(common::puppet_warp()))),
        ("pins_corner_pin", Box::new(|fg| fg.warp = Some(common::corner_warp()))),
        ("pins_puppet_corner_pin", Box::new(move |fg| fg.warp = Some(both.clone()))),
        ("pins_mask", Box::new(|fg| fg.masks.push(common::pinned_mask()))),
        ("pins_puppet_masked", Box::new(|fg| {
            fg.masks.push(MaskSpec { shape: MaskShape::Ellipse, feather: 0.05, size: [0.7, 0.8], ..Default::default() });
            fg.warp = Some(common::puppet_warp());
        })),
    ];
    for (name, f) in cases {
        let mut clips = backdrop();
        let mut fg = foreground(2, 1);
        f(&mut fg);
        clips.push(fg);
        failures.extend(check(name, &c.render(&scene(clips), 0, None)));
    }
    // A puppet on a shape layer (the mesh covers only the star).
    let mut clips = backdrop();
    let mut star = common::shape(3, 1, 0, SECOND, "star", [0.95, 0.9, 0.3, 1.0]);
    star.warp = Some(common::puppet_warp());
    clips.push(star);
    failures.extend(check("pins_puppet_shape", &c.render(&scene(clips), 0, None)));
    report(failures);
}

#[test]
fn transforms() {
    let mut c = compositor();
    let mut failures = Vec::new();
    let cases: Vec<(&str, Box<dyn Fn(&mut ClipDesc)>)> = vec![
        ("transform_fit", Box::new(|_| {})),
        ("transform_scale", Box::new(|c| c.placement.scale = [0.5, 0.8])),
        ("transform_rotate", Box::new(|c| c.placement.rotation = 30.0)),
        ("transform_position", Box::new(|c| c.placement.position = [20.0, -10.0])),
        ("transform_anchor", Box::new(|c| {
            c.placement.anchor = [64.0, 36.0];
            c.placement.rotation = 45.0;
        })),
        ("transform_crop", Box::new(|c| c.placement.crop = [0.2, 0.1, 0.1, 0.3])),
        ("transform_flip", Box::new(|c| {
            c.placement.flip_x = true;
            c.placement.flip_y = true;
        })),
        ("transform_minify", Box::new(|c| c.placement.scale = [0.15, 0.15])),
        ("transform_magnify", Box::new(|c| {
            c.placement.scale = [3.0, 3.0];
            c.placement.position = [10.0, 5.0];
        })),
        ("transform_opacity", Box::new(|c| c.placement.opacity = 0.4)),
        ("transform_all", Box::new(|c| {
            c.placement.scale = [0.7, 0.5];
            c.placement.rotation = -20.0;
            c.placement.position = [-12.0, 6.0];
            c.placement.anchor = [-30.0, 0.0];
            c.placement.crop = [0.0, 0.2, 0.1, 0.0];
            c.placement.flip_x = true;
        })),
        ("transform_portrait_source", Box::new(|c| c.source = SourceDesc::Media { media: MediaKey(2), time: 0 })),
    ];
    for (name, f) in cases {
        let mut clip = common::media(1, 0, 0, SECOND, MediaKey(1));
        f(&mut clip);
        let clips = vec![common::gradient(10, -1, 0, SECOND, [0.2, 0.2, 0.25, 1.0], [0.35, 0.3, 0.4, 1.0], 45.0), clip];
        failures.extend(check(name, &c.render(&scene(clips), 0, None)));
    }
    // Motion presets: entry at a third of its length, exit at two thirds,
    // and a loop, through the resolver's pose evaluation.
    let presets: Vec<&studio_fx::MotionDef> = studio_fx::motion::all().iter().collect();
    let entry = presets.iter().find(|p| p.kind == studio_fx::motion::MotionKind::InOut).expect("an in/out preset");
    let looping = presets.iter().find(|p| p.kind == studio_fx::motion::MotionKind::Loop).expect("a loop preset");
    let mut clip = common::media(1, 0, 0, 3 * SECOND, MediaKey(1));
    clip.placement.scale = [0.6, 0.6];
    clip.motion.entry = Some(MotionRef { kind: entry.id.into(), duration: SECOND, params: Params::new() });
    clip.motion.exit = Some(MotionRef { kind: entry.id.into(), duration: SECOND, params: Params::new() });
    let mut s = scene(vec![clip.clone()]);
    s.duration = 3 * SECOND;
    failures.extend(check("transform_motion_entry", &c.render(&s, SECOND / 3, None)));
    failures.extend(check("transform_motion_exit", &c.render(&s, 3 * SECOND - SECOND / 3, None)));
    failures.extend(check("transform_motion_rest", &c.render(&s, 3 * SECOND / 2, None)));
    clip.motion = Default::default();
    clip.motion.looping = Some(MotionRef { kind: looping.id.into(), duration: 2 * SECOND, params: Params::new() });
    let mut s = scene(vec![clip]);
    s.duration = 3 * SECOND;
    failures.extend(check("transform_motion_loop", &c.render(&s, SECOND / 2, None)));
    report(failures);
}

#[test]
fn transitions() {
    let mut c = compositor();
    let mut failures = Vec::new();
    let left = common::media(1, 0, 0, SECOND, MediaKey(1));
    let mut right = common::gradient(2, 0, SECOND, SECOND, [0.1, 0.6, 0.9, 1.0], [0.9, 0.9, 0.2, 1.0], 60.0);
    right.placement.scale = [0.9, 0.9];
    let mut kinds: Vec<String> = studio_fx::transitions::all().iter().map(|t| t.id.to_string()).collect();
    kinds.push("no_such_transition".into());
    for kind in kinds {
        let mut s = StaticScene::new(canvas(), 2 * SECOND).with_clip(left.clone()).with_clip(right.clone()).with_transition(1, 2, &kind, SECOND / 2, SECOND, Params::new());
        s.duration = 2 * SECOND;
        // A third of the way and two thirds, so asymmetric transitions show
        // their direction.
        for (tag, t) in [("a", SECOND / 2 + SECOND / 3), ("b", SECOND / 2 + 2 * SECOND / 3)] {
            failures.extend(check(&format!("transition_{kind}_{tag}"), &c.render(&s, t, None)));
        }
    }
    // Both sides exact at the ends.
    let s = StaticScene::new(canvas(), 2 * SECOND).with_clip(left.clone()).with_clip(right.clone()).with_transition(1, 2, "wipe", SECOND / 2, SECOND, Params::new());
    let before = c.render(&s, SECOND / 2 - 1, None);
    let start = c.render(&s, SECOND / 2, None);
    assert_eq!(rgba8(&before), rgba8(&start), "a transition starts exactly on the outgoing picture");
    report(failures);
}

#[test]
fn generators_and_adjustment() {
    let mut c = compositor();
    let mut failures = Vec::new();
    for kind in ["rectangle", "ellipse", "polygon", "star", "line", "arrow"] {
        let mut sh = common::shape(2, 1, 0, SECOND, kind, [0.95, 0.8, 0.2, 1.0]);
        let p = match &mut sh.source {
            SourceDesc::Generator(g) => &mut g.params,
            _ => unreachable!(),
        };
        p.insert("stroke".into(), Param::Colour([0.1, 0.1, 0.3, 1.0]));
        p.insert("stroke_width".into(), Param::Float(0.04));
        p.insert("roundness".into(), Param::Float(0.3));
        p.insert("rotation".into(), Param::Float(15.0));
        let mut clips = backdrop();
        clips.push(sh);
        failures.extend(check(&format!("generator_shape_{kind}"), &c.render(&scene(clips), 0, None)));
    }
    failures.extend(check("generator_gradient", &c.render(&scene(vec![common::gradient(1, 0, 0, SECOND, [0.0, 0.0, 0.0, 1.0], [1.0, 1.0, 1.0, 1.0], 20.0)]), 0, None)));
    failures.extend(check("generator_solid_alpha", &c.render(&scene(vec![common::solid(1, 0, 0, SECOND, [0.2, 0.9, 0.3, 0.5])]), 0, None)));
    // Adjustment clip: grade and effect on everything below, through a mask.
    let mut clips = backdrop();
    let mut adj = ClipDesc::new(3, 2, 0, SECOND, SourceDesc::Adjustment);
    adj.grade = Some(common::grade(&[("saturation", Param::Float(0.0)), ("exposure", Param::Float(0.5))]));
    adj.effects.push(common::effect("posterise", &[("levels", Param::Float(4.0))]));
    adj.masks.push(MaskSpec { shape: MaskShape::Line, rotation: 90.0, feather: 0.1, ..Default::default() });
    clips.push(adj);
    failures.extend(check("adjustment_masked", &c.render(&scene(clips), 0, None)));
    // Nested scene rendered as a layer.
    let inner = scene(backdrop());
    let mut nested = ClipDesc::new(4, 1, 0, SECOND, SourceDesc::Nested { scene: Arc::new(inner), time: 0 });
    nested.placement.scale = [0.6, 0.6];
    nested.placement.rotation = 12.0;
    failures.extend(check("nested_scene", &c.render(&scene(vec![common::solid(1, 0, 0, SECOND, [0.3, 0.3, 0.35, 1.0]), nested]), 0, None)));
    report(failures);
}

/// Draws each run as a coloured box in a row, so the hook's placement and
/// pixel size are visible without a real shaper.
struct BoxText;

impl TextRenderer for BoxText {
    fn raster(&self, runs: &[TextRun], style: &Params, width: u32, height: u32) -> Option<Frame> {
        use studio_render::scene::ParamsExt;
        if runs.is_empty() {
            return None;
        }
        let colour = style.colour("colour", [1.0, 1.0, 1.0, 1.0]);
        let n = runs.len() as u32;
        Some(Frame::from_fn(width, height, |x, y| {
            let cell = width / n.max(1);
            let i = (x / cell.max(1)).min(n - 1);
            let inner = x % cell.max(1) > cell / 8 && x % cell.max(1) < cell - cell / 8 && y > height / 4 && y < height * 3 / 4;
            if inner {
                let shade = 0.5 + 0.5 * (i as f32 / n as f32);
                [colour[0] * shade * colour[3], colour[1] * shade * colour[3], colour[2] * colour[3], colour[3]]
            } else {
                [0.0; 4]
            }
        }))
    }
}

#[test]
fn space() {
    let mut c = compositor();
    let mut failures = Vec::new();
    let mut canvas = canvas();
    canvas.background = [0.0, 0.0, 0.0, 1.0];
    for (name, scene, t) in common::space_scenes(canvas) {
        failures.extend(check(name, &c.render(&scene, t, None)));
    }
    report(failures);
}

#[test]
fn text_hook() {
    let mut c = compositor();
    let mut failures = Vec::new();
    let runs = vec![TextRun { text: "Studio".into(), style: Params::new() }, TextRun { text: "render".into(), style: Params::new() }];
    let mut style = Params::new();
    style.insert("colour".into(), Param::Colour([1.0, 0.9, 0.3, 1.0]));
    let mut text = ClipDesc::new(2, 1, 0, SECOND, SourceDesc::Generator(GeneratorDesc { kind: GeneratorKind::Text, params: style, text: runs }));
    text.placement.scale = [0.7, 0.3];
    text.placement.rotation = -5.0;
    text.placement.position = [0.0, 8.0];
    let mut clips = backdrop();
    clips.push(text);
    let s = scene(clips);
    // Without a text renderer the layer draws nothing (the backdrop alone).
    let stub = c.render(&s, 0, None);
    failures.extend(check("text_stub", &stub));
    set_text_renderer(Arc::new(BoxText));
    let drawn = c.render(&s, 0, None);
    failures.extend(check("text_hook", &drawn));
    assert_ne!(rgba8(&stub), rgba8(&drawn), "the installed text renderer changes the picture");
    // Text is rasterised at the layer's pixel size, so a half-size preview
    // asks the hook for half-size pixels rather than shrinking a picture.
    let small = c.render(&s, 0, Some((W / 2, H / 2)));
    assert_eq!((small.width, small.height), (W / 2, H / 2));
    assert_ne!(rgba8(&small), rgba8(&c.render(&scene(backdrop()), 0, Some((W / 2, H / 2)))));
    report(failures);
}
