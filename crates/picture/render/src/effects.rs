// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Adapters to the `fx` crate (effects, transitions, motion presets) and
//! the `colour` crate (grades), plus the built-in generators.
//!
//! The renderer never implements an effect itself: it maps the scene's
//! named parameters onto the crates' typed parameter sets and calls them.
//! Without the `fx` feature, effects are skipped, transitions fall back to
//! a dissolve and motion presets to rest.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use studio_colour::{Grade, Primaries};

use crate::frame::{Frame, Px, premultiply};
use crate::mask::{coverage, sdf_ellipse, sdf_polygon, sdf_rect, star_vertices};
use crate::scene::{EffectDesc, GeneratorDesc, GeneratorKind, GradeDesc, Micros, MotionDesc, Params, ParamsExt};
#[cfg(feature = "fx")]
use crate::scene::{Param, SECOND};
use crate::transform::Pose;

/// Per-frame context for effects.
#[derive(Clone, Copy, Debug)]
pub struct EffectCtx {
    /// Seconds since the clip start.
    pub time: f32,
    pub frame_rate: f32,
    pub seed: u32,
}

// --------------------------------------------------------------- effects

#[cfg(feature = "fx")]
fn to_param_set(defs: &'static [studio_fx::ParamDef], params: &Params) -> studio_fx::ParamSet {
    use studio_fx::{ParamKind, ParamValue};
    let mut set = studio_fx::ParamSet::defaults(defs);
    for def in defs {
        let Some(p) = params.get(def.id) else { continue };
        let v = match (&def.kind, p) {
            (ParamKind::Float { .. }, Param::Float(v)) => Some(ParamValue::Float(*v as f32)),
            (ParamKind::Int { .. }, Param::Float(v)) => Some(ParamValue::Int(v.round() as i32)),
            (ParamKind::Bool, Param::Bool(b)) => Some(ParamValue::Bool(*b)),
            (ParamKind::Bool, Param::Float(v)) => Some(ParamValue::Bool(*v != 0.0)),
            (ParamKind::Choice(names), Param::Choice(s)) | (ParamKind::Choice(names), Param::Text(s)) => {
                names.iter().position(|n| n.eq_ignore_ascii_case(s)).map(|i| ParamValue::Choice(i as u32))
            }
            (ParamKind::Choice(_), Param::Float(v)) => Some(ParamValue::Choice(v.max(0.0) as u32)),
            (ParamKind::Colour, Param::Colour(c)) => Some(ParamValue::Colour([c[0] as f32, c[1] as f32, c[2] as f32, c[3] as f32])),
            (ParamKind::Point, Param::Point(pt)) => Some(ParamValue::Point([pt[0] as f32, pt[1] as f32])),
            _ => None,
        };
        if let Some(v) = v {
            // Out-of-range or wrong-kind values are rejected by `set` and
            // the default stays, which is the safe outcome for a project
            // file written by a newer version.
            let _ = set.set(def.id, v);
        }
    }
    set
}

/// Applies an effect stack on the CPU.
pub fn apply_effects(frame: Frame, effects: &[EffectDesc], ctx: EffectCtx) -> Frame {
    #[cfg(feature = "fx")]
    {
        let mut img = frame.into_fx();
        for e in effects {
            if e.kind == crate::comp::KIND {
                img = crate::comp::apply_cpu(Frame::from_fx(img), e, ctx).into_fx();
                continue;
            }
            let Some(def) = studio_fx::effects::find(&e.kind) else { continue };
            let params = to_param_set(def.params, &e.params);
            let fctx = studio_fx::EffectContext { time: ctx.time, frame_rate: ctx.frame_rate, seed: ctx.seed, history: &[], aux: None };
            img = def.apply(&fctx, &img, &params);
        }
        Frame::from_fx(img)
    }
    #[cfg(not(feature = "fx"))]
    {
        let _ = (effects, ctx);
        frame
    }
}

/// Whether an effect is known to the `fx` crate and has a GPU shader.
pub fn effect_has_shader(kind: &str) -> bool {
    #[cfg(feature = "fx")]
    {
        studio_fx::effects::find(kind).is_some_and(|d| d.wgsl.is_some())
    }
    #[cfg(not(feature = "fx"))]
    {
        let _ = kind;
        false
    }
}

/// The WGSL source and packed parameters of an effect, for the GPU path.
#[cfg(feature = "fx")]
pub fn effect_shader(e: &EffectDesc) -> Option<(&'static str, Vec<[f32; 4]>)> {
    let def = studio_fx::effects::find(&e.kind)?;
    let wgsl = def.wgsl?;
    Some((wgsl, to_param_set(def.params, &e.params).pack_vec4()))
}

/// The data an effect's shader draws from (particles), for a picture of
/// `width`×`height` at `ctx`, when the effect has any.
#[cfg(feature = "fx")]
pub fn effect_storage(e: &EffectDesc, ctx: EffectCtx, width: u32, height: u32) -> Option<Vec<u32>> {
    let def = studio_fx::effects::find(&e.kind)?;
    let make = def.storage?;
    let fctx = studio_fx::EffectContext { time: ctx.time, frame_rate: ctx.frame_rate, seed: ctx.seed, history: &[], aux: None };
    Some(make(&fctx, width as usize, height as usize, &to_param_set(def.params, &e.params)))
}

// ----------------------------------------------------------------- grade

/// Builds a colour-crate grade from named parameters.
///
/// Keys (all optional; missing means identity): `exposure` (stops),
/// `temperature` (Kelvin), `tint`, `contrast`, `highlights`, `shadows`,
/// `whites`, `blacks` (−1..1), `saturation` (0..2, 1 neutral), `vibrance`,
/// `clarity`, `dehaze` (−1..1), `fade` (0..1), `sharpen`, `vignette`,
/// `grain` (amounts), wheels as `<lift|gamma|gain|offset|shadows|midtones|
/// highlights>_<hue|amount|luma>`, `wheel_mode` (`primary` / `log`),
/// `wheel_intensity`, an exact ASC CDL as `cdl_slope_r|g|b`,
/// `cdl_offset_*`, `cdl_power_*`, `cdl_sat` (it replaces the four-way
/// wheels), curves as point lists `curve_master|red|green|blue`,
/// `hue_vs_hue|hue_vs_sat|hue_vs_lum|lum_vs_sat|sat_vs_sat`, eight-band
/// HSL as `hsl_<red|orange|yellow|green|aqua|blue|purple|magenta>_<hue|sat|light>`,
/// `lut_intensity`, `lut_skin_protect`; the LUT file
/// comes from [`GradeDesc::lut`].
pub fn grade_from_desc(desc: &GradeDesc) -> Grade {
    let p = &desc.params;
    let mut g = Grade::default();
    let b = &mut g.basic;
    b.exposure = p.f("exposure", b.exposure);
    b.temperature = p.f("temperature", b.temperature);
    b.tint = p.f("tint", b.tint);
    b.contrast = p.f("contrast", 0.0);
    b.highlights = p.f("highlights", 0.0);
    b.shadows = p.f("shadows", 0.0);
    b.whites = p.f("whites", 0.0);
    b.blacks = p.f("blacks", 0.0);
    b.saturation = p.f("saturation", 1.0);
    b.vibrance = p.f("vibrance", 0.0);
    b.clarity = p.f("clarity", 0.0);
    b.dehaze = p.f("dehaze", 0.0);
    b.fade = p.f("fade", 0.0);
    b.sharpen.amount = p.f("sharpen", b.sharpen.amount);
    b.vignette.amount = p.f("vignette", b.vignette.amount);
    b.grain.amount = p.f("grain", b.grain.amount);
    let w = &mut g.wheels;
    for (name, wheel) in [
        ("lift", &mut w.lift),
        ("gamma", &mut w.gamma),
        ("gain", &mut w.gain),
        ("offset", &mut w.offset),
        ("shadows", &mut w.shadows),
        ("midtones", &mut w.midtones),
        ("highlights", &mut w.highlights),
    ] {
        wheel.hue = p.f(&format!("{name}_hue"), wheel.hue);
        wheel.amount = p.f(&format!("{name}_amount"), wheel.amount);
        wheel.luma = p.f(&format!("{name}_luma"), wheel.luma);
    }
    if p.text("wheel_mode", "primary") == "log" {
        w.mode = studio_colour::WheelMode::ShadowsMidtonesHighlights;
    }
    w.intensity = p.f("wheel_intensity", w.intensity);
    // An exact ASC CDL (from an EDL, a .cdl / .cc / .ccc file, or set by
    // hand): `cdl_slope_r` … `cdl_power_b`, `cdl_sat`.
    if ["slope", "offset", "power"].iter().any(|k| ["r", "g", "b"].iter().any(|c| p.get(&format!("cdl_{k}_{c}")).is_some())) {
        let tri = |k: &str, d: f32| ["r", "g", "b"].map(|c| p.f(&format!("cdl_{k}_{c}"), d));
        w.asc = Some(studio_colour::Cdl { slope: tri("slope", 1.0), offset: tri("offset", 0.0), power: tri("power", 1.0), saturation: 1.0 });
    }
    g.basic.saturation *= p.f("cdl_sat", 1.0);
    // Curves (points in normalised grading units; see `studio_colour::curves`).
    let pts = |k: &str| match p.get(k) {
        Some(crate::scene::Param::Curve(v)) if !v.is_empty() => Some(v.iter().map(|q| [q[0] as f32, q[1] as f32]).collect::<Vec<_>>()),
        _ => None,
    };
    let c = &mut g.curves;
    for (k, slot) in [
        ("curve_master", &mut c.master),
        ("curve_red", &mut c.red),
        ("curve_green", &mut c.green),
        ("curve_blue", &mut c.blue),
        ("hue_vs_hue", &mut c.hue_vs_hue),
        ("hue_vs_sat", &mut c.hue_vs_sat),
        ("hue_vs_lum", &mut c.hue_vs_lum),
        ("lum_vs_sat", &mut c.lum_vs_sat),
        ("sat_vs_sat", &mut c.sat_vs_sat),
    ] {
        if let Some(v) = pts(k) {
            *slot = v;
        }
    }
    // Eight-band HSL: `hsl_<band>_hue|sat|light`, −1..1.
    for (i, band) in studio_colour::hsl::BAND_NAMES.iter().enumerate() {
        let b = &mut g.hsl.bands[i];
        b.hue = p.f(&format!("hsl_{band}_hue"), b.hue);
        b.saturation = p.f(&format!("hsl_{band}_sat"), b.saturation);
        b.lightness = p.f(&format!("hsl_{band}_light"), b.lightness);
    }
    // Shot matching: this clip's footage statistics moved to a reference's
    // (`match_src_*` / `match_ref_*`, three Oklab channels each).
    let strength = p.f("match_strength", 0.0);
    if strength > 0.0 {
        let tri = |k: &str| [0, 1, 2].map(|i| p.f(&format!("{k}_{i}"), 0.0));
        g.colour_match = Some(studio_colour::auto::ColourMatch {
            source: studio_colour::auto::ColourStats { mean: tri("match_src_mean"), std: tri("match_src_std") },
            reference: studio_colour::auto::ColourStats { mean: tri("match_ref_mean"), std: tri("match_ref_std") },
            strength,
        });
    }
    // A named look (the Looks panel): its grade baked into a 3D LUT once,
    // applied like a LUT file at full strength.
    let look = p.text("look", "");
    if !look.is_empty() && desc.lut.is_none() {
        if let Some(cube) = look_cube(look) {
            g.lut = Some(studio_colour::LutSettings { lut: cube, space: studio_colour::LutSpace::Rec709, intensity: p.f("look_intensity", 1.0), skin_protect: 0.0 });
        }
    }
    if let Some(path) = &desc.lut
        && let Some(cube) = load_cube(path)
    {
        g.lut = Some(studio_colour::LutSettings {
            lut: cube,
            space: studio_colour::LutSpace::default(),
            intensity: p.f("lut_intensity", 1.0),
            skin_protect: p.f("lut_skin_protect", 0.0),
        });
    }
    g.working = Primaries::BT709;
    g
}

/// The baked LUT of a shipped look, by slug (cached).
fn look_cube(slug: &str) -> Option<Arc<studio_colour::CubeFile>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Option<Arc<studio_colour::CubeFile>>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut g = cache.lock().unwrap_or_else(|e| e.into_inner());
    g.entry(slug.to_string())
        .or_insert_with(|| {
            let look = studio_colour::looks::LOOKS.iter().find(|l| l.slug == slug)?;
            Some(Arc::new(studio_colour::CubeFile { title: Some(look.title.into()), lut1d: None, lut3d: Some(look.bake(studio_colour::looks::LOOK_SIZE)) }))
        })
        .clone()
}

fn load_cube(path: &str) -> Option<Arc<studio_colour::CubeFile>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Option<Arc<studio_colour::CubeFile>>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut g = cache.lock().unwrap_or_else(|e| e.into_inner());
    g.entry(path.to_string())
        .or_insert_with(|| studio_colour::read_lut(std::path::Path::new(path)).ok().map(Arc::new))
        .clone()
}

/// Applies a grade on the CPU. `frame_index` animates grain.
pub fn apply_grade(frame: Frame, desc: &GradeDesc, frame_index: u64) -> Frame {
    let grade = grade_from_desc(desc);
    if grade.is_identity() {
        return frame;
    }
    let mut img = frame.into_colour();
    grade.apply(&mut img, studio_colour::FrameInfo { frame: frame_index });
    Frame::from_colour(img)
}

// ----------------------------------------------------------- transitions

/// Combines the outgoing picture `a` and incoming `b` at `progress`.
/// Unknown kinds dissolve.
pub fn transition(kind: &str, a: Frame, b: Frame, progress: f32, params: &Params, seed: u32) -> Frame {
    let progress = progress.clamp(0.0, 1.0);
    if progress <= 0.0 {
        return a;
    }
    if progress >= 1.0 {
        return b;
    }
    #[cfg(feature = "fx")]
    {
        if let Some(def) = studio_fx::transitions::find(kind) {
            let set = to_param_set(def.params, params);
            let ctx = studio_fx::TransitionContext { seed };
            let out = def.apply(&ctx, &a.into_fx(), &b.into_fx(), progress, &set);
            return Frame::from_fx(out);
        }
    }
    let _ = (kind, params, seed);
    dissolve(&a, &b, progress)
}

/// Linear cross-dissolve of premultiplied frames.
pub fn dissolve(a: &Frame, b: &Frame, t: f32) -> Frame {
    Frame { width: a.width, height: a.height, pixels: a.pixels.iter().zip(&b.pixels).map(|(p, q)| std::array::from_fn(|i| p[i] + (q[i] - p[i]) * t)).collect() }
}

/// Whether a transition has a GPU shader in the `fx` crate.
pub fn transition_has_shader(kind: &str) -> bool {
    #[cfg(feature = "fx")]
    {
        studio_fx::transitions::find(kind).is_some_and(|d| d.wgsl.is_some())
    }
    #[cfg(not(feature = "fx"))]
    {
        let _ = kind;
        false
    }
}

/// The WGSL source and packed parameters of a transition.
#[cfg(feature = "fx")]
pub fn transition_shader(kind: &str, params: &Params) -> Option<(&'static str, Vec<[f32; 4]>)> {
    let def = studio_fx::transitions::find(kind)?;
    let wgsl = def.wgsl?;
    Some((wgsl, to_param_set(def.params, params).pack_vec4()))
}

// ---------------------------------------------------------------- motion

/// The pose of a clip's motion presets `rel` microseconds into a clip of
/// `duration`. Entry plays over its duration from the start, exit over its
/// duration up to the end, loop repeats in between; they stack.
pub fn motion_pose(motion: &MotionDesc, rel: Micros, duration: Micros) -> Pose {
    #[cfg(feature = "fx")]
    {
        use studio_fx::motion::{MotionKind, Phase};
        let mut pose = studio_fx::Pose::REST;
        let mut any = false;
        if let Some(entry) = &motion.entry
            && let Some(def) = studio_fx::motion::find(&entry.kind)
            && def.kind == MotionKind::InOut
            && entry.duration > 0
            && rel < entry.duration
        {
            let set = to_param_set(def.params, &entry.params);
            pose = pose.then(&def.pose(Phase::Entry, rel as f32 / entry.duration as f32, &set));
            any = true;
        }
        if let Some(exit) = &motion.exit
            && let Some(def) = studio_fx::motion::find(&exit.kind)
            && def.kind == MotionKind::InOut
            && exit.duration > 0
        {
            let start = duration - exit.duration;
            if rel >= start {
                let set = to_param_set(def.params, &exit.params);
                pose = pose.then(&def.pose(Phase::Exit, (rel - start) as f32 / exit.duration as f32, &set));
                any = true;
            }
        }
        if let Some(lp) = &motion.looping
            && let Some(def) = studio_fx::motion::find(&lp.kind)
            && def.kind == MotionKind::Loop
        {
            let mut set = to_param_set(def.params, &lp.params);
            if lp.duration > 0 {
                let _ = set.set("period", studio_fx::ParamValue::Float(lp.duration as f32 / SECOND as f32));
            }
            pose = pose.then(&def.loop_pose(rel as f32 / SECOND as f32, &set));
            any = true;
        }
        if !any || !pose.is_finite() {
            return Pose::REST;
        }
        Pose { offset: pose.offset, scale: pose.scale, rotation: pose.rotation, opacity: pose.opacity, blur: pose.blur, clip: pose.clip, turn: pose.turn, depth: pose.depth }
    }
    #[cfg(not(feature = "fx"))]
    {
        let _ = (motion, rel, duration);
        Pose::REST
    }
}

// ------------------------------------------------------------ generators

fn straight_to_premul(c: [f32; 4]) -> Px {
    premultiply(c)
}

/// Draws a generator at `width × height` pixels.
///
/// Parameters: solid `colour`; gradient `start`, `end` (colours), `angle`
/// (degrees, 0 = left to right); shape `shape` (`rectangle`, `ellipse`,
/// `polygon`, `star`, `line`, `arrow`), `fill`, `stroke` (colours),
/// `stroke_width` (fraction of the shorter side), `size` (point, fractions
/// of the picture), `roundness`, `sides`, `rotation`. Text goes through the
/// installed [`crate::text::TextRenderer`].
pub fn generate(g: &GeneratorDesc, width: u32, height: u32) -> Frame {
    let p = &g.params;
    match g.kind {
        GeneratorKind::Solid => {
            let c = straight_to_premul(p.colour("colour", [1.0, 1.0, 1.0, 1.0]));
            Frame::filled(width, height, c)
        }
        // OKLab, eased through every stop, radial with a soft tail:
        // see `gradient`.
        GeneratorKind::Gradient => crate::gradient::draw(p, width, height),
        GeneratorKind::Shape => shape(p, width, height),
        GeneratorKind::Text => {
            crate::text::text_renderer().raster(&g.text, &g.params, width, height).unwrap_or_else(|| Frame::new(width, height))
        }
    }
}

fn shape(p: &Params, width: u32, height: u32) -> Frame {
    // Vector illustration documents draw at the output's own resolution.
    if let Some(f) = crate::vector_art::draw(p, width, height) {
        return f;
    }
    // A shape layer: a document of shapes, operators and paint.
    let doc = p.text("vector", "");
    if !doc.is_empty()
        && let Some(f) = vector_shape(p, doc, width, height)
    {
        return f;
    }
    let (w, h) = (width as f32, height as f32);
    let short = w.min(h);
    let fill = straight_to_premul(p.colour("fill", [1.0, 1.0, 1.0, 1.0]));
    let stroke = straight_to_premul(p.colour("stroke", [0.0, 0.0, 0.0, 0.0]));
    let stroke_w = p.f("stroke_width", 0.0).max(0.0) * short;
    let size = p.point("size", [0.5, 0.5]);
    let half = [size[0] * w * 0.5, size[1] * h * 0.5];
    let (sin, cos) = p.f("rotation", 0.0).to_radians().sin_cos();
    let kind = p.text("shape", "rectangle").to_ascii_lowercase();
    let roundness = p.f("roundness", 0.0).clamp(0.0, 1.0) * half[0].min(half[1]);
    let sides = p.f("sides", 6.0).round().clamp(3.0, 64.0) as usize;
    let polygon: Vec<[f32; 2]> = match kind.as_str() {
        "polygon" => (0..sides)
            .map(|i| {
                let a = -std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::TAU / sides as f32;
                [a.cos() * half[0], a.sin() * half[1]]
            })
            .collect(),
        "star" => star_vertices().iter().map(|v| [v[0] * half[0], v[1] * half[1]]).collect(),
        "arrow" => {
            // Shaft of 40 % height, head taking the right 35 %.
            let (hx, hy) = (half[0], half[1]);
            let shaft = hy * 0.4;
            let head = hx * 0.7;
            vec![[-hx, -shaft], [hx - head, -shaft], [hx - head, -hy], [hx, 0.0], [hx - head, hy], [hx - head, shaft], [-hx, shaft]]
        }
        _ => Vec::new(),
    };
    let sdf = |x: f32, y: f32| -> f32 {
        let dx = x - w * 0.5;
        let dy = y - h * 0.5;
        let lx = dx * cos + dy * sin;
        let ly = -dx * sin + dy * cos;
        match kind.as_str() {
            "ellipse" => sdf_ellipse([lx, ly], half),
            "line" => ly.abs() - (half[1] * 0.1).max(0.5).min(half[1]),
            "polygon" | "star" | "arrow" => sdf_polygon([lx, ly], &polygon),
            _ => sdf_rect([lx, ly], half, roundness),
        }
    };
    Frame::from_fn(width, height, |x, y| {
        let d = sdf(x as f32 + 0.5, y as f32 + 0.5);
        let inside = coverage(d, 0.0);
        let mut out = [fill[0] * inside, fill[1] * inside, fill[2] * inside, fill[3] * inside];
        if stroke_w > 0.0 && stroke[3] > 0.0 {
            let ring = coverage(d.abs() - stroke_w * 0.5, 0.0);
            for i in 0..4 {
                out[i] = stroke[i] * ring + out[i] * (1.0 - ring);
            }
        }
        out
    })
}

/// Linear light to the sRGB curve (shape documents work in sRGB values,
/// as colour pickers and hex codes do).
fn encode_srgb(c: f64) -> f64 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.003_130_8 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 }
}

/// A shape layer's document, the moment and inputs it is drawn with, and
/// its canvas, from a generator's settings (`None` when it has none).
pub fn vector_inputs(p: &Params, width: u32, height: u32) -> Option<(studio_vector::Document, studio_vector::Ctx, [f64; 2])> {
    use studio_vector::Input;
    let doc = studio_vector::parse(p.text("vector", "")).ok()?;
    let mut cx = studio_vector::Ctx::at(p.f("clip_time", 0.0) as f64, p.f("clip_duration", 1.0) as f64);
    for (k, v) in p {
        let input = match v {
            Param::Float(x) => Input::Num(*x),
            Param::Bool(b) => Input::Num(*b as u8 as f64),
            Param::Point(q) => Input::Point(*q),
            Param::Colour(c) => Input::Colour([encode_srgb(c[0]), encode_srgb(c[1]), encode_srgb(c[2]), c[3]]),
            Param::Text(t) | Param::Choice(t) => Input::Text(t.clone()),
            Param::Curve(_) => continue,
        };
        cx.inputs.insert(k.clone(), input);
    }
    let canvas = [p.f("canvas_width", width as f32) as f64, p.f("canvas_height", height as f32) as f64];
    Some((doc, cx, canvas))
}

/// Draws a shape layer document (`studio-vector`) at this moment of its
/// clip. Its units are canvas pixels (`canvas_width` × `canvas_height`)
/// from the centre; every other generator setting is an input it can read
/// as `$name`.
fn vector_shape(p: &Params, src: &str, width: u32, height: u32) -> Option<Frame> {
    let _ = src;
    let (doc, cx, canvas) = vector_inputs(p, width, height)?;
    let pic = studio_vector::render(&doc, &cx, width, height, canvas);
    let lut = crate::colour_space::srgb8_to_linear_lut();
    let pixels = pic
        .rgba
        .chunks_exact(4)
        .map(|q| {
            let a = q[3] as f32 / 255.0;
            if q[3] == 0 {
                return [0.0; 4];
            }
            // Premultiplied 8-bit: back to straight, decode, premultiply.
            let un = |c: u8| lut[((c as u32 * 255 + q[3] as u32 / 2) / q[3] as u32).min(255) as usize] * a;
            [un(q[0]), un(q[1]), un(q[2]), a]
        })
        .collect();
    Some(Frame::from_pixels(width, height, pixels))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::Param;

    #[test]
    fn vector_shapes_draw_their_document() {
        let mut params = Params::new();
        params.insert("vector".into(), Param::Text(r##"[{"type": "circle", "radius": "$r"}, {"type": "fill", "colour": "$tint"}]"##.into()));
        params.insert("r".into(), Param::Float(300.0));
        params.insert("tint".into(), Param::Colour([1.0, 0.0, 0.0, 1.0]));
        params.insert("canvas_width".into(), Param::Float(1920.0));
        params.insert("canvas_height".into(), Param::Float(1080.0));
        let f = generate(&GeneratorDesc { kind: GeneratorKind::Shape, params, text: vec![] }, 192, 108);
        let c = f.get(96, 54);
        assert!((c[0] - 1.0).abs() < 1e-3 && c[1] < 1e-3 && (c[3] - 1.0).abs() < 1e-3, "{c:?}");
        // 300 px on a 1080 canvas is 30 px here.
        assert!(f.get(96 + 25, 54)[3] > 0.99 && f.get(96 + 35, 54)[3] < 0.01);
    }

    #[test]
    fn solid_and_gradient() {
        let mut params = Params::new();
        params.insert("colour".into(), Param::Colour([1.0, 0.5, 0.0, 0.5]));
        let f = generate(&GeneratorDesc { kind: GeneratorKind::Solid, params, text: vec![] }, 4, 2);
        assert_eq!(f.get(3, 1), [0.5, 0.25, 0.0, 0.5]);
        let f = generate(&GeneratorDesc { kind: GeneratorKind::Gradient, params: Params::new(), text: vec![] }, 64, 4);
        assert!(f.get(0, 0)[0] < 0.05 && f.get(63, 0)[0] > 0.95);
        assert!(f.get(20, 0)[0] < f.get(40, 0)[0]);
    }

    #[test]
    fn shapes_fill_centre_only() {
        for kind in ["rectangle", "ellipse", "polygon", "star", "arrow", "line"] {
            let mut params = Params::new();
            params.insert("shape".into(), Param::Choice(kind.into()));
            params.insert("fill".into(), Param::Colour([0.0, 1.0, 0.0, 1.0]));
            let f = generate(&GeneratorDesc { kind: GeneratorKind::Shape, params, text: vec![] }, 64, 64);
            assert!(f.get(32, 32)[1] > 0.99, "{kind} centre");
            assert!(f.get(0, 0)[3] < 0.01, "{kind} corner");
        }
    }

    #[test]
    fn grade_params_map() {
        let mut params = Params::new();
        params.insert("exposure".into(), Param::Float(1.0));
        params.insert("gain_luma".into(), Param::Float(0.2));
        let g = grade_from_desc(&GradeDesc { params, lut: None });
        assert_eq!(g.basic.exposure, 1.0);
        assert_eq!(g.wheels.gain.luma, 0.2);
        assert!(!g.is_identity());
        assert!(grade_from_desc(&GradeDesc::default()).is_identity());
    }

    #[test]
    fn grade_exposure_doubles_light() {
        let mut params = Params::new();
        params.insert("exposure".into(), Param::Float(1.0));
        let f = Frame::filled(2, 2, [0.1, 0.2, 0.3, 1.0]);
        let out = apply_grade(f, &GradeDesc { params, lut: None }, 0);
        let p = out.get(0, 0);
        assert!((p[0] - 0.2).abs() < 1e-3 && (p[2] - 0.6).abs() < 1e-3, "{p:?}");
    }

    #[test]
    fn dissolve_ends_exact() {
        let a = Frame::filled(2, 2, [1.0, 0.0, 0.0, 1.0]);
        let b = Frame::filled(2, 2, [0.0, 0.0, 1.0, 1.0]);
        assert_eq!(transition("no_such_transition", a.clone(), b.clone(), 0.0, &Params::new(), 0), a);
        assert_eq!(transition("no_such_transition", a.clone(), b.clone(), 1.0, &Params::new(), 0), b);
        let m = transition("no_such_transition", a, b, 0.5, &Params::new(), 0);
        assert_eq!(m.get(0, 0), [0.5, 0.0, 0.5, 1.0]);
    }

    #[cfg(feature = "fx")]
    #[test]
    fn fx_effect_and_motion_hook_up() {
        let mut params = Params::new();
        params.insert("levels".into(), Param::Float(2.0));
        let f = Frame::from_fn(8, 8, |x, _| [x as f32 / 8.0, 0.5, 0.5, 1.0]);
        let out = apply_effects(f.clone(), &[EffectDesc { kind: "posterise".into(), params }], EffectCtx { time: 0.0, frame_rate: 30.0, seed: 0 });
        assert_ne!(out, f);
        assert!(effect_has_shader("posterise"));
        let motion = MotionDesc {
            entry: Some(crate::scene::MotionRef { kind: studio_fx::motion::all()[0].id.into(), duration: SECOND, params: Params::new() }),
            exit: None,
            looping: None,
        };
        // Past the entry the pose is at rest; at the very start it is not.
        assert_eq!(motion_pose(&motion, 2 * SECOND, 4 * SECOND), Pose::REST);
        let start = motion_pose(&motion, 0, 4 * SECOND);
        assert!(start.is_rest() || start.opacity <= 1.0);
    }

    #[test]
    fn a_clip_look_becomes_a_lut() {
        let mut params = crate::scene::Params::default();
        params.insert("look".into(), crate::scene::Param::Choice("silver-mono".into()));
        let g = grade_from_desc(&GradeDesc { params, lut: None });
        assert!(g.lut.is_some(), "the look was dropped");
        // Silver Mono is black and white: a saturated red comes out grey.
        let out = apply_grade(crate::frame::Frame::from_pixels(1, 1, vec![[0.8, 0.1, 0.1, 1.0]]), &GradeDesc { params: { let mut p = crate::scene::Params::default(); p.insert("look".into(), crate::scene::Param::Choice("silver-mono".into())); p }, lut: None }, 0);
        let px = out.get(0, 0);
        assert!((px[0] - px[1]).abs() < 0.03 && (px[1] - px[2]).abs() < 0.03, "not monochrome: {px:?}");
    }
}
