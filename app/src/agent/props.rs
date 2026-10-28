// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! One vocabulary for every clip property: what an agent can read it can
//! write and animate, under the same name and in the same units.
//!
//! Names: `x`, `y` (canvas pixels from the centre, y down), `scale`,
//! `scale_x`, `scale_y`, `opacity`, `crop_left|right|top|bottom` (percent),
//! `rotation` (degrees), `anchor_x`, `anchor_y`, `volume` (dB), `pan`
//! (-100…100), `exposure` (stops), `contrast`, `saturation`, `temperature`,
//! `tint` (-100…100), any other grade setting as `grade.<name>`, an effect
//! setting as `<effect>.<setting>` (`zoom_blur.amount`; the second of two
//! alike as `zoom_blur#2.amount`), a mask setting as `mask1.<setting>`, a
//! title or shape setting as `generator.<setting>`. 3D: `z` (pixels, away
//! from the viewer), `rotation_x`, `rotation_y`, `rotation_z`,
//! `orientation_x|y|z` (degrees), `scale_z` (percent), `anchor_z`, and a
//! material setting as `material.<setting>`; on a camera `zoom` (pixels),
//! `focal_length` (mm), `fov` (degrees), `focus_distance`, `aperture`,
//! `blur_level` (percent), `poi_x|y|z`; on a light `intensity` (percent),
//! `colour`, `cone_angle`, `cone_feather`, `radius`, `falloff`,
//! `shadow_darkness` (percent), `poi_x|y|z`.

use serde_json::{Value, json};
use studio_model as m;

/// How a friendly value maps onto a model property.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Conv {
    /// Percent ↔ factor.
    Percent,
    /// One axis of a point.
    PointX,
    PointY,
    /// One axis of a drawn box's size: canvas pixels ↔ a fraction of
    /// the canvas.
    BoxX,
    BoxY,
    /// A length: canvas pixels ↔ a fraction of the canvas's shorter side
    /// (a drawn shape's outline width).
    Short,
    /// Degrees as an angle.
    Degrees,
    /// The same number, through the Inspector's unit for that grade path.
    Grade,
    /// The value as the model holds it (numbers, booleans, colours, text).
    Raw,
    /// A camera's zoom (pixels) shown as a focal length in mm or a field
    /// of view in degrees.
    FocalLength,
    Fov,
}

/// A friendly property resolved on one clip: the model paths it writes
/// (scale writes both axes) and the conversion.
#[derive(Clone, Debug)]
pub struct Prop {
    pub name: String,
    pub paths: Vec<String>,
    pub conv: Conv,
}

fn norm(s: &str) -> String {
    s.to_lowercase().replace([' ', '-'], "_")
}

/// The friendly name for an effect kind: `zoom_blur`.
pub fn effect_name(kind: &str) -> String {
    norm(kind.rsplit('.').next().unwrap_or(kind))
}

thread_local! {
    static CANVAS: std::cell::Cell<(f64, f64)> = const { std::cell::Cell::new((1920.0, 1080.0)) };
}

/// The canvas a box's pixel size is measured against.
pub fn set_canvas(width: f64, height: f64) {
    CANVAS.with(|c| c.set((width.max(1.0), height.max(1.0))));
}

/// Resolves a friendly property name on `clip`.
pub fn resolve(clip: &m::Clip, name: &str) -> Result<Prop, String> {
    let n = norm(name.trim());
    // Names agents reach for from other editors: `transform.scale`,
    // `position.x`, `alpha`. Each is the friendly name it means.
    if let Some(real) = other_name(&n) {
        return resolve(clip, real).map(|mut p| {
            p.name = real.to_owned();
            p
        });
    }
    if n == "transform.position" {
        return Err(format!("position is two properties on clip {}: x and y (canvas px from the centre, y down); animate also takes \"property\": \"position\" with [t, x, y] keys", clip.id.0));
    }
    let p = |paths: &[&str], conv| Ok(Prop { name: n.clone(), paths: paths.iter().map(|s| (*s).to_owned()).collect(), conv });
    // A shape layer's inputs by their own names.
    if let m::ClipSource::Generator(g) = &clip.source
        && g.kind == m::GeneratorKind::Shape
        && g.params.contains_key("vector")
        && g.params.contains_key(n.as_str())
        && !n.contains('.')
    {
        return Ok(Prop { name: n.clone(), paths: vec![format!("generator.{n}")], conv: Conv::Raw });
    }
    // A colour card or a drawn box: its colour and its size in pixels.
    if let m::ClipSource::Generator(g) = &clip.source {
        match (g.kind, n.as_str()) {
            (m::GeneratorKind::Solid, "colour" | "color" | "fill") => return p(&["generator.colour"], Conv::Raw),
            (m::GeneratorKind::Shape, "colour" | "color" | "fill") => return p(&["generator.fill"], Conv::Raw),
            (m::GeneratorKind::Shape, "width" | "box_width") => return p(&["generator.size"], Conv::BoxX),
            (m::GeneratorKind::Shape, "height" | "box_height") => return p(&["generator.size"], Conv::BoxY),
            (m::GeneratorKind::Shape, "roundness") => return p(&["generator.roundness"], Conv::Raw),
            (m::GeneratorKind::Shape, "stroke" | "stroke_colour" | "stroke_color" | "outline") => return p(&["generator.stroke"], Conv::Raw),
            (m::GeneratorKind::Shape, "stroke_width" | "outline_width") => return p(&["generator.stroke_width"], Conv::Short),
            (m::GeneratorKind::Camera, "zoom") => return p(&["generator.zoom"], Conv::Raw),
            (m::GeneratorKind::Camera, "focal_length" | "lens") => return p(&["generator.zoom"], Conv::FocalLength),
            (m::GeneratorKind::Camera, "fov" | "field_of_view" | "angle_of_view") => return p(&["generator.zoom"], Conv::Fov),
            (m::GeneratorKind::Camera, "focus" | "focus_distance") => return p(&["generator.focus_distance"], Conv::Raw),
            (m::GeneratorKind::Camera, "aperture") => return p(&["generator.aperture"], Conv::Raw),
            (m::GeneratorKind::Camera, "blur_level") => return p(&["generator.blur_level"], Conv::Percent),
            (m::GeneratorKind::Light, "intensity") => return p(&["generator.intensity"], Conv::Percent),
            (m::GeneratorKind::Light, "colour" | "color") => return p(&["generator.colour"], Conv::Raw),
            (m::GeneratorKind::Light, "cone_angle" | "cone") => return p(&["generator.cone_angle"], Conv::Degrees),
            (m::GeneratorKind::Light, "cone_feather" | "feather") => return p(&["generator.cone_feather"], Conv::Percent),
            (m::GeneratorKind::Light, "radius" | "falloff") => return Ok(Prop { name: n.clone(), paths: vec![format!("generator.{n}")], conv: Conv::Raw }),
            (m::GeneratorKind::Light, "glow" | "flare" | "beam" | "source_size" | "flicker" | "flicker_speed" | "flame_height" | "flame_width" | "flame_tongues") => return Ok(Prop { name: n.clone(), paths: vec![format!("generator.{n}")], conv: Conv::Raw }),
            (m::GeneratorKind::Light, "shadow_darkness") => return p(&["generator.shadow_darkness"], Conv::Percent),
            (m::GeneratorKind::Model, "colour" | "color" | "base_colour") => return p(&["generator.base_colour"], Conv::Raw),
            (m::GeneratorKind::Model, "emissive" | "glow") => return p(&["generator.emissive"], Conv::Raw),
            // Opacity in percent like any clip's (it scales the material's own).
            (m::GeneratorKind::Model, "opacity") => return p(&["generator.opacity"], Conv::Percent),
            (m::GeneratorKind::Model, "metallic" | "roughness" | "smooth" | "subsurface" | "wetness" | "emissive_strength") => return Ok(Prop { name: n.clone(), paths: vec![format!("generator.{n}")], conv: Conv::Raw }),
            (m::GeneratorKind::Camera | m::GeneratorKind::Light, "poi_x" | "target_x") => return p(&["generator.poi"], Conv::PointX),
            (m::GeneratorKind::Camera | m::GeneratorKind::Light, "poi_y" | "target_y") => return p(&["generator.poi"], Conv::PointY),
            (m::GeneratorKind::Camera | m::GeneratorKind::Light, "poi_z" | "target_z") => return p(&["generator.poi_z"], Conv::Raw),
            _ => {}
        }
    }
    match n.as_str() {
        "scale" | "zoom" => return p(&["transform.scale_x", "transform.scale_y"], Conv::Percent),
        "scale_x" | "width" => return p(&["transform.scale_x"], Conv::Percent),
        "scale_y" | "height" => return p(&["transform.scale_y"], Conv::Percent),
        "x" => return p(&["transform.position"], Conv::PointX),
        "y" => return p(&["transform.position"], Conv::PointY),
        "anchor_x" => return p(&["transform.anchor"], Conv::PointX),
        "anchor_y" => return p(&["transform.anchor"], Conv::PointY),
        "rotation" | "rotate" | "rotation_z" => return p(&["transform.rotation"], Conv::Degrees),
        "z" | "position_z" | "depth" => return p(&["transform.position_z"], Conv::Raw),
        "rotation_x" | "tilt_x" => return p(&["transform.rotation_x"], Conv::Degrees),
        "rotation_y" | "turn_y" => return p(&["transform.rotation_y"], Conv::Degrees),
        "orientation_x" | "orientation_y" | "orientation_z" => return Ok(Prop { name: n.clone(), paths: vec![format!("transform.{n}")], conv: Conv::Degrees }),
        "scale_z" => return p(&["transform.scale_z"], Conv::Percent),
        "anchor_z" => return p(&["transform.anchor_z"], Conv::Raw),
        "opacity" => return p(&["transform.opacity"], Conv::Percent),
        "flip" | "flip_x" | "mirror" => return p(&["transform.flip_x"], Conv::Raw),
        "flip_y" => return p(&["transform.flip_y"], Conv::Raw),
        "crop_left" | "crop_right" | "crop_top" | "crop_bottom" => {
            let path = format!("transform.{n}");
            return Ok(Prop { name: n.clone(), paths: vec![path], conv: Conv::Percent });
        }
        "volume" => return p(&["audio.volume_db"], Conv::Raw),
        "pan" => return p(&["audio.pan"], Conv::Percent),
        "exposure" => return p(&["grade.exposure"], Conv::Raw),
        // How much of the clip's look shows (0-100 %), animatable.
        "look_strength" | "look_amount" => return p(&["grade.look_intensity"], Conv::Percent),
        "contrast" | "saturation" | "temperature" | "tint" => {
            return Ok(Prop { name: n.clone(), paths: vec![format!("grade.{n}")], conv: Conv::Grade });
        }
        _ => {}
    }
    let (head, setting) = n.split_once('.').ok_or_else(|| unknown(clip, name))?;
    match head {
        "grade" | "generator" => return Ok(Prop { name: n.clone(), paths: vec![n.clone()], conv: Conv::Raw }),
        "material" if m::Material::default().get(setting).is_some() => return Ok(Prop { name: n.clone(), paths: vec![n.clone()], conv: Conv::Raw }),
        "material" => return Err(format!("no material setting \"{setting}\"; material.accepts_lights, casts_shadows, accepts_shadows, ambient, diffuse, specular, shininess, extrude, side_shade")),
        _ => {}
    }
    if let Some(k) = head.strip_prefix("mask") {
        let i: usize = k.trim_start_matches('_').parse().unwrap_or(1).max(1);
        let mk = clip.masks.get(i - 1).ok_or_else(|| format!("the clip has {} mask(s); \"{head}\" is not one", clip.masks.len()))?;
        return Ok(Prop { name: n.clone(), paths: vec![format!("mask.{}.{setting}", mk.id.0)], conv: Conv::Raw });
    }
    // An effect: by its name, the n-th of that name with #n, or by id.
    let (fx, nth) = match head.split_once('#') {
        Some((a, b)) => (a, b.parse::<usize>().unwrap_or(1).max(1)),
        None => (head, 1),
    };
    let found = if let Ok(id) = fx.parse::<u64>() {
        clip.effects.iter().find(|e| e.id.0 == id)
    } else {
        let want = fx.replace('_', "");
        clip.effects.iter().filter(|e| effect_name(&e.kind).replace('_', "").contains(&want)).nth(nth - 1)
    };
    let e = found.ok_or_else(|| unknown(clip, name))?;
    Ok(Prop { name: n.clone(), paths: vec![format!("effect.{}.{setting}", e.id.0)], conv: Conv::Raw })
}

/// The friendly name another editor's (or a guessed) name means.
fn other_name(n: &str) -> Option<&'static str> {
    Some(match n {
        "transform.scale" | "scale.xy" | "uniform_scale" => "scale",
        "transform.scale_x" | "scale.x" => "scale_x",
        "transform.scale_y" | "scale.y" => "scale_y",
        "transform.scale_z" | "scale.z" => "scale_z",
        "transform.opacity" | "alpha" => "opacity",
        "transform.rotation" | "rotation.z" => "rotation",
        "rotation.x" | "transform.rotation_x" => "rotation_x",
        "rotation.y" | "transform.rotation_y" => "rotation_y",
        "position.x" | "position_x" | "pos_x" | "transform.x" => "x",
        "position.y" | "position_y" | "pos_y" | "transform.y" => "y",
        "position.z" | "transform.position_z" | "transform.z" => "z",
        "anchor.x" | "transform.anchor_x" => "anchor_x",
        "anchor.y" | "transform.anchor_y" => "anchor_y",
        "volume_db" | "audio.volume" | "audio.volume_db" => "volume",
        "audio.pan" => "pan",
        _ => return None,
    })
}

/// The friendly names every clip has (for "did you mean").
const COMMON: &[&str] = &["x", "y", "z", "scale", "scale_x", "scale_y", "scale_z", "rotation", "rotation_x", "rotation_y", "opacity", "anchor_x", "anchor_y", "anchor_z", "crop_left", "crop_right", "crop_top", "crop_bottom", "volume", "pan", "exposure", "contrast", "saturation", "temperature", "tint", "look_strength"];

fn unknown(clip: &m::Clip, name: &str) -> String {
    let fx: Vec<String> = clip.effects.iter().map(|e| format!("{}.<setting>", effect_name(&e.kind))).collect();
    let hint = super::args::closest(&norm(name), COMMON).map(|c| format!(" (did you mean \"{c}\"?)")).unwrap_or_default();
    format!(
        "no property \"{name}\"{hint} on clip {}; units: x, y, z px from the centre (y down), scale and opacity in percent (100 = as is, 0..100), rotation in degrees, volume in dB; `inspect {{clip, what: \"properties\"}}` lists this clip's own. Properties: x, y, z, scale, scale_x, scale_y, scale_z, rotation, rotation_x, rotation_y, orientation_x/y/z, anchor_z, material.<setting>, opacity, crop_left/right/top/bottom, anchor_x, anchor_y, volume, pan, exposure, contrast, saturation, temperature, tint, grade.<name>, generator.<name>{}{}",
        clip.id.0,
        if fx.is_empty() { String::new() } else { format!(", {}", fx.join(", ")) },
        if clip.masks.is_empty() { String::new() } else { ", mask1.<setting>".into() }
    )
}

fn scalar(v: &m::Value) -> f64 {
    match v {
        m::Value::Scalar(x) | m::Value::Angle(x) => *x,
        m::Value::Bool(b) => *b as u8 as f64,
        _ => 0.0,
    }
}

fn round(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

/// A model value as the agent sees it.
pub fn to_friendly(prop: &Prop, path: &str, v: &m::Value) -> Value {
    match prop.conv {
        Conv::Percent => json!(round(scalar(v) * 100.0)),
        Conv::PointX => match v { m::Value::Point(p) => json!(round(p.x)), _ => Value::Null },
        Conv::PointY => match v { m::Value::Point(p) => json!(round(p.y)), _ => Value::Null },
        Conv::BoxX => match v { m::Value::Point(p) => json!(round(p.x * CANVAS.with(|c| c.get().0))), _ => Value::Null },
        Conv::BoxY => match v { m::Value::Point(p) => json!(round(p.y * CANVAS.with(|c| c.get().1))), _ => Value::Null },
        Conv::Short => json!(round(scalar(v) * CANVAS.with(|c| { let (w, h) = c.get(); w.min(h) }))),
        Conv::Degrees => json!(round(scalar(v))),
        Conv::Grade => json!(round(crate::bridge::ui_value(path, v) as f64)),
        Conv::Raw => raw_out(v),
        Conv::FocalLength => json!(round(m::space::focal_length_from_zoom(scalar(v), CANVAS.with(|c| c.get().0)))),
        Conv::Fov => json!(round(m::space::fov_from_zoom(scalar(v), CANVAS.with(|c| c.get().0)))),
    }
}

/// Any model value as JSON (colours as `#rrggbb`).
pub fn raw_out(v: &m::Value) -> Value {
    match v {
        m::Value::Scalar(x) | m::Value::Angle(x) => json!(round(*x)),
        m::Value::Bool(b) => json!(b),
        m::Value::Point(p) => json!([round(p.x), round(p.y)]),
        m::Value::Colour(c) => json!(format!("#{:02x}{:02x}{:02x}", (c.r.clamp(0.0, 1.0) * 255.0).round() as u8, (c.g.clamp(0.0, 1.0) * 255.0).round() as u8, (c.b.clamp(0.0, 1.0) * 255.0).round() as u8)),
        m::Value::Text(t) | m::Value::Choice(t) => json!(t),
        m::Value::Curve(c) => json!(c),
    }
}

/// A friendly value as a model value for `path`; `current` is the value
/// there now (a point keeps its other axis; a raw value keeps its kind).
pub fn to_model(prop: &Prop, path: &str, v: &Value, current: Option<&m::Value>) -> Result<m::Value, String> {
    let num = || v.as_f64().ok_or_else(|| format!("{} takes a number", prop.name));
    Ok(match prop.conv {
        Conv::Percent => m::Value::Scalar(num()? / 100.0),
        Conv::Short => m::Value::Scalar((num()? / CANVAS.with(|c| { let (w, h) = c.get(); w.min(h) })).max(0.0)),
        Conv::Degrees => m::Value::Angle(num()?),
        Conv::FocalLength => m::Value::Scalar(m::space::zoom_from_focal_length(num()?, CANVAS.with(|c| c.get().0))),
        Conv::Fov => m::Value::Scalar(m::space::zoom_from_fov(num()?, CANVAS.with(|c| c.get().0))),
        Conv::Grade => crate::bridge::model_value(path, num()? as f32),
        Conv::PointX | Conv::PointY => {
            let mut p = match current {
                Some(m::Value::Point(p)) => *p,
                _ => m::Vec2 { x: 0.0, y: 0.0 },
            };
            if prop.conv == Conv::PointX { p.x = num()? } else { p.y = num()? }
            m::Value::Point(p)
        }
        Conv::BoxX | Conv::BoxY => {
            let mut p = match current {
                Some(m::Value::Point(p)) => *p,
                _ => m::Vec2 { x: 1.0, y: 1.0 },
            };
            let (w, h) = CANVAS.with(|c| c.get());
            if prop.conv == Conv::BoxX { p.x = (num()? / w).max(0.0) } else { p.y = (num()? / h).max(0.0) }
            m::Value::Point(p)
        }
        Conv::Raw => match (current, v) {
            (Some(m::Value::Bool(_)), _) | (None, Value::Bool(_)) => m::Value::Bool(v.as_bool().or_else(|| v.as_f64().map(|x| x != 0.0)).ok_or_else(|| format!("{} takes true or false", prop.name))?),
            (Some(m::Value::Angle(_)), _) => m::Value::Angle(num()?),
            (Some(m::Value::Point(_)), Value::Array(a)) | (None, Value::Array(a)) if a.len() == 2 => m::Value::Point(m::Vec2 { x: a[0].as_f64().unwrap_or(0.0), y: a[1].as_f64().unwrap_or(0.0) }),
            (Some(m::Value::Colour(_)), Value::String(s)) | (None, Value::String(s)) if s.starts_with('#') || matches!(current, Some(m::Value::Colour(_))) => {
                let c = super::craft::colour(s).ok_or_else(|| format!("\"{s}\" is not #rrggbb or a basic name"))?;
                m::Value::Colour(m::Rgba { r: c.r() as f64 / 255.0, g: c.g() as f64 / 255.0, b: c.b() as f64 / 255.0, a: 1.0 })
            }
            (Some(m::Value::Choice(_)), Value::String(s)) => m::Value::Choice(s.clone()),
            (Some(m::Value::Text(_)), Value::String(s)) => m::Value::Text(s.clone()),
            (_, Value::String(s)) => m::Value::Choice(s.clone()),
            _ => m::Value::Scalar(num()?),
        },
    })
}

/// A property's value now, or its default when the clip has not stored
/// it yet (effect settings keep only what differs from their default).
pub fn value_or_default(clip: &m::Clip, path: &str) -> Option<m::Value> {
    if let Some(v) = clip.get_property(path) {
        return Some(v);
    }
    // A grade keeps only what was changed; the rest is neutral.
    if path == "grade.look_intensity" {
        return Some(m::Value::Scalar(1.0));
    }
    if path.starts_with("grade.") {
        return Some(crate::bridge::model_value(path, 0.0));
    }
    // A drawn box with no outline yet.
    if let m::ClipSource::Generator(g) = &clip.source
        && g.kind == m::GeneratorKind::Shape
    {
        match path {
            "generator.stroke" => return Some(m::Value::Colour(m::Rgba { r: 0.0, g: 0.0, b: 0.0, a: 0.0 })),
            "generator.stroke_width" => return Some(m::Value::Scalar(0.0)),
            _ => {}
        }
    }
    // How a lamp shows itself: nothing until asked for.
    if let m::ClipSource::Generator(g) = &clip.source
        && g.kind == m::GeneratorKind::Light
        && let Some(key) = path.strip_prefix("generator.")
        && !g.params.contains_key(key)
    {
        match key {
            "glow" | "flare" | "beam" | "source_size" | "flicker" => return Some(m::Value::Scalar(0.0)),
            "flicker_speed" => return Some(m::Value::Scalar(7.0)),
            _ => {}
        }
    }
    // A 3D model's material as the model file has it until changed
    // (opacity multiplies it, so 1 leaves it as it is).
    if let m::ClipSource::Generator(g) = &clip.source
        && g.kind == m::GeneratorKind::Model
        && let Some(key) = path.strip_prefix("generator.")
    {
        let material = || {
            #[cfg(feature = "render")]
            let first = match g.params.get("file") {
                Some(m::Value::Text(f)) => studio_render::mesh_render::load(f).and_then(|mesh| mesh.materials.first().cloned()),
                _ => None,
            };
            #[cfg(not(feature = "render"))]
            let first: Option<studio_mesh::Material> = None;
            first.unwrap_or_default()
        };
        return match key {
            "opacity" | "smooth" | "emissive_strength" => Some(m::Value::Scalar(1.0)),
            "wetness" => Some(m::Value::Scalar(0.0)),
            "metallic" => Some(m::Value::Scalar(material().metallic as f64)),
            "roughness" => Some(m::Value::Scalar(material().roughness as f64)),
            "subsurface" => Some(m::Value::Scalar(material().subsurface as f64)),
            _ => None,
        };
    }
    let rest = path.strip_prefix("effect.")?;
    let (id, key) = rest.split_once('.')?;
    let e = clip.effects.iter().find(|e| e.id.0.to_string() == id)?;
    let def = studio_fx::effects::find(&e.kind)?;
    let p = def.params.iter().find(|p| p.id == key)?;
    use studio_fx::params::ParamValue as P;
    Some(match p.default {
        P::Float(x) => m::Value::Scalar(x as f64),
        P::Int(x) => m::Value::Scalar(x as f64),
        P::Bool(b) => m::Value::Bool(b),
        P::Choice(c) => m::Value::Scalar(c as f64),
        P::Colour(c) => m::Value::Colour(m::Rgba { r: c[0] as f64, g: c[1] as f64, b: c[2] as f64, a: c[3] as f64 }),
        P::Point(q) => m::Value::Point(m::Vec2 { x: q[0] as f64, y: q[1] as f64 }),
    })
}

fn interp_name(i: m::Interp) -> &'static str {
    match i {
        m::Interp::Hold => "hold",
        m::Interp::Linear => "linear",
        m::Interp::Ease => "ease",
        m::Interp::Auto => "smooth",
        m::Interp::Curve => "curve",
    }
}

pub fn interp_of(s: &str) -> Option<m::Interp> {
    Some(match s {
        "linear" => m::Interp::Linear,
        "hold" | "step" => m::Interp::Hold,
        "ease" | "ease_in_out" | "bezier" => m::Interp::Ease,
        "smooth" | "auto" => m::Interp::Auto,
        _ => return None,
    })
}

/// Every friendly property of a clip with its value now, and its keys
/// (time from the clip start, value, curve, handles) in the same units.
pub fn describe(clip: &m::Clip) -> Value {
    let mut names: Vec<String> = ["x", "y", "scale_x", "scale_y", "rotation", "opacity", "anchor_x", "anchor_y", "crop_left", "crop_right", "crop_top", "crop_bottom", "volume", "pan"].iter().map(|s| (*s).to_owned()).collect();
    if clip.transform.three_d || clip.is_camera_or_light() {
        names.extend(["z", "rotation_x", "rotation_y", "orientation_x", "orientation_y", "orientation_z", "scale_z", "anchor_z"].iter().map(|s| (*s).to_owned()));
    }
    if clip.transform.three_d {
        names.extend(m::Material::ANIMATED.iter().filter(|k| **k != "shadow").map(|k| format!("material.{k}")));
    }
    if let Some(g) = &clip.grade {
        for k in g.params.keys() {
            names.push(match k.as_str() {
                "exposure" | "contrast" | "saturation" | "temperature" | "tint" => k.clone(),
                "look_intensity" => "look_strength".into(),
                _ => format!("grade.{k}"),
            });
        }
        if (g.params.contains_key("look") || clip.keyframes.contains_key("grade.look_intensity")) && !names.iter().any(|n| n == "look_strength") {
            names.push("look_strength".into());
        }
    }
    let mut seen: std::collections::HashMap<String, usize> = Default::default();
    for e in &clip.effects {
        let base = effect_name(&e.kind);
        let n = seen.entry(base.clone()).or_insert(0);
        *n += 1;
        let head = if *n > 1 { format!("{base}#{n}") } else { base };
        // Every setting the effect has, stored or at its default.
        let mut ks: Vec<String> = studio_fx::effects::find(&e.kind).map(|d| d.params.iter().map(|p| p.id.to_owned()).collect()).unwrap_or_default();
        for k in e.params.keys() {
            if !ks.contains(k) {
                ks.push(k.clone());
            }
        }
        for k in ks {
            names.push(format!("{head}.{k}"));
        }
    }
    for (i, mk) in clip.masks.iter().enumerate() {
        for k in mk.params.keys() {
            names.push(format!("mask{}.{k}", i + 1));
        }
    }
    if let m::ClipSource::Generator(g) = &clip.source {
        match g.kind {
            m::GeneratorKind::Camera => names.extend(["zoom", "focal_length", "fov", "focus_distance", "aperture", "blur_level", "poi_x", "poi_y", "poi_z"].iter().map(|s| (*s).to_owned())),
            m::GeneratorKind::Model => names.extend(["colour", "metallic", "roughness", "emissive", "emissive_strength", "wetness", "opacity", "smooth", "subsurface"].iter().map(|s| (*s).to_owned())),
            m::GeneratorKind::Light => names.extend(["intensity", "colour", "cone_angle", "cone_feather", "radius", "falloff", "shadow_darkness", "glow", "flare", "beam", "flicker", "poi_x", "poi_y", "poi_z"].iter().map(|s| (*s).to_owned())),
            _ => {}
        }
        for k in g.params.keys() {
            names.push(format!("generator.{k}"));
        }
    }
    let mut values = serde_json::Map::new();
    let mut keys = serde_json::Map::new();
    for name in names {
        let Ok(prop) = resolve(clip, &name) else { continue };
        let path = &prop.paths[0];
        if let Some(v) = value_or_default(clip, path) {
            values.insert(name.clone(), to_friendly(&prop, path, &v));
        }
        if let Some(track) = clip.keyframes.get(path) {
            // Each key with how it eases into the next, by name when it is
            // one of the named curves.
            let key_out = |i: usize, value: Value| {
                let k = &track.keys[i];
                let mut o = json!({"t": round(k.time as f64 / 1e6), "value": value, "curve": curve_name(k, track.keys.get(i + 1))});
                if k.interp == m::Interp::Ease {
                    o["out"] = json!([k.ease_out.x, k.ease_out.y]);
                }
                if let Some(p) = k.path {
                    o["path"] = json!({"in": p.inn, "out": p.out});
                }
                o["in"] = json!([k.ease_in.x, k.ease_in.y]);
                o
            };
            let ks: Vec<Value> = (0..track.keys.len()).map(|i| key_out(i, to_friendly(&prop, path, &track.keys[i].value))).collect();
            // x and y share one track: report it once, as position.
            let key_name = if matches!(prop.conv, Conv::PointX | Conv::PointY) && path == "transform.position" { "position".to_owned() } else { name.clone() };
            if track.keys.len() > 24 {
                // A key on every frame (a track followed, a stabilize): its
                // span and ends, not hundreds of keys.
                let (a, b) = (&track.keys[0], &track.keys[track.keys.len() - 1]);
                let is_pos = key_name == "position";
                let val = |v: &m::Value| if is_pos { raw_out(v) } else { to_friendly(&prop, path, v) };
                keys.insert(key_name, json!({"keys": track.keys.len(), "from": round(a.time as f64 / 1e6), "to": round(b.time as f64 / 1e6), "first": val(&a.value), "last": val(&b.value), "dense": "a key every frame (tracking or stabilize); keys / animate replace them"}));
            } else if key_name == "position" {
                let pk: Vec<Value> = (0..track.keys.len()).map(|i| key_out(i, raw_out(&track.keys[i].value))).collect();
                keys.insert(key_name, json!(pk));
            } else {
                keys.insert(key_name, json!(ks));
            }
        }
    }
    let mut out = json!({
        "values": values,
        "keyframes": keys,
        "blend": format!("{:?}", clip.blend).to_lowercase(),
        "reversed": clip.reversed,
        "enabled": clip.enabled,
        "masks": clip.masks.iter().enumerate().map(|(i, mk)| json!({"name": format!("mask{}", i + 1), "shape": format!("{:?}", mk.shape).to_lowercase(), "invert": mk.invert})).collect::<Vec<_>>(),
    });
    let mods = super::modifiers::listed(clip);
    if mods.as_object().is_some_and(|o| !o.is_empty()) {
        out["modifiers"] = mods;
    }
    if !clip.drivers.is_empty() {
        out["drivers"] = super::drivers::listed(clip);
    }
    out
}

/// The named curve a key leaves with (its next key's arriving handle
/// counts too), or its interpolation's name.
pub fn curve_name(k: &m::Keyframe, next: Option<&m::Keyframe>) -> String {
    if k.interp == m::Interp::Curve {
        return super::craft::curve_label(k.curve.unwrap_or_default());
    }
    let close = |a: m::Handle, b: Option<m::Handle>| b.is_none_or(|b| (a.x - b.x).abs() < 0.02 && (a.y - b.y).abs() < 0.02);
    ["ease_in_out", "ease_in", "ease_out", "overshoot", "anticipate", "snap", "whip"]
        .into_iter()
        .find(|n| super::craft::curve(n).is_some_and(|(interp, out, inn, _)| interp == k.interp && close(k.ease_out, out) && next.is_none_or(|nk| close(nk.ease_in, inn))))
        .map(str::to_owned)
        .unwrap_or_else(|| interp_name(k.interp).to_owned())
}
