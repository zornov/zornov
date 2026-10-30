// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The grade: validating and applying a clip's colour parameters, nodes,
//! LUTs, CDLs and the project's colour management. The agent's `colour`
//! tool and the Grade panel both call these, so a change means the same
//! thing from either.

use crate::doc::Doc;
use serde_json::{Map, Value, json};
use std::path::Path;
use studio_model as m;

/// JSON number / string / list / object → model parameter value.
pub fn to_value(v: &Value) -> Option<m::Value> {
    Some(match v {
        Value::Bool(b) => m::Value::Bool(*b),
        Value::Number(n) => m::Value::Scalar(n.as_f64()?),
        Value::String(t) => m::Value::Text(t.clone()),
        Value::Array(a) if a.iter().all(|p| p.is_array()) => m::Value::Curve(a.iter().filter_map(|p| Some([p.get(0)?.as_f64()?, p.get(1)?.as_f64()?])).collect()),
        Value::Array(a) if a.len() == 2 => m::Value::Point(m::Vec2::new(a[0].as_f64()?, a[1].as_f64()?)),
        _ => return None,
    })
}

pub fn from_value(v: &m::Value) -> Value {
    match v {
        m::Value::Bool(b) => json!(b),
        m::Value::Scalar(x) | m::Value::Angle(x) => json!((x * 1000.0).round() / 1000.0),
        m::Value::Point(p) => json!([p.x, p.y]),
        m::Value::Text(t) | m::Value::Choice(t) => json!(t),
        m::Value::Curve(c) => json!(c),
        m::Value::Colour(c) => json!([c.r, c.g, c.b, c.a]),
    }
}

pub fn params_json(p: &m::ParamMap) -> Value {
    Value::Object(p.iter().map(|(k, v)| (k.clone(), from_value(v))).collect())
}

/// Grade parameter names this tool accepts (besides `cdl_*`, curves and
/// `hsl_*` patterns).
pub const GRADE_KEYS: &[&str] = &[
    "exposure", "temperature", "tint", "contrast", "highlights", "shadows", "whites", "blacks", "saturation", "vibrance", "clarity", "dehaze", "fade", "sharpen", "vignette", "grain",
    "wheel_mode", "wheel_intensity", "look", "look_intensity", "lut_intensity", "lut_skin_protect",
    "curve_master", "curve_red", "curve_green", "curve_blue", "hue_vs_hue", "hue_vs_sat", "hue_vs_lum", "lum_vs_sat", "sat_vs_sat",
];

pub fn grade_key_ok(k: &str) -> bool {
    GRADE_KEYS.contains(&k)
        || k.starts_with("cdl_")
        || k.starts_with("hsl_")
        || ["lift", "gamma", "gain", "offset", "shadows", "midtones", "highlights"].iter().any(|w| k.starts_with(&format!("{w}_")) && ["hue", "amount", "luma"].iter().any(|x| k.ends_with(x)))
}

/// Other names for grade parameters agents reach for (`curve_r` failed a
/// whole colour task).
pub fn grade_alias(k: &str) -> Option<&'static str> {
    Some(match k {
        "curve" | "curves" | "curve_rgb" | "curve_luma" | "curve_y" | "master_curve" => "curve_master",
        "curve_r" | "red_curve" => "curve_red",
        "curve_g" | "green_curve" => "curve_green",
        "curve_b" | "blue_curve" => "curve_blue",
        "white_balance" | "warmth" => "temperature",
        "sat" => "saturation",
        _ => return None,
    })
}

/// Merges `{name: value | null}` into a parameter map. `closest` names the
/// nearest known parameter for a misspelt one.
pub fn merge_params(into: &mut m::ParamMap, given: &Value, check: bool, closest: impl Fn(&str) -> Option<String>) -> Result<Vec<String>, String> {
    let obj = given.as_object().ok_or("params is an object of name: value")?;
    let mut changed = Vec::new();
    for (k, v) in obj {
        let k = &grade_alias(k).map_or_else(|| k.clone(), str::to_owned);
        if check && !grade_key_ok(k) {
            let hint = closest(k).map(|c| format!(" (did you mean \"{c}\"?)")).unwrap_or_default();
            return Err(format!(
                "no grade parameter \"{k}\"{hint}; it takes: {}, cdl_slope / cdl_offset / cdl_power / cdl_sat, hsl_<colour>_<hue|sat|lum>, and wheels as lift_ / gamma_ / gain_ / offset_ / shadows_ / midtones_ / highlights_ + hue | amount | luma. Curves are lists of [in, out] points 0..1, e.g. {{\"curve_red\": [[0, 0], [0.5, 0.45], [1, 1]]}}",
                GRADE_KEYS.join(", ")
            ));
        }
        if v.is_null() {
            into.remove(k);
        } else {
            into.insert(k.clone(), to_value(v).ok_or_else(|| format!("\"{k}\": a number, text, [x, y] or a list of [x, y] points"))?);
        }
        changed.push(k.clone());
    }
    Ok(changed)
}

/// The model's clip (all of it: masks, keys, effects by id).
pub fn model_clip(doc: &Doc, id: u32) -> Option<m::Clip> {
    doc.model.sequences.iter().find_map(|q| q.clip(m::ClipId(id as u64))).cloned()
}

/// The sequence holding clip `clip`.
pub fn seq_of(doc: &Doc, clip: u32) -> Option<m::SequenceId> {
    doc.model.sequences.iter().find(|s| s.clip(m::ClipId(clip as u64)).is_some()).map(|s| s.id)
}

/// A clip with its sequence and its grade (an enabled empty one when it has none).
pub fn clip_grade(doc: &Doc, id: u32) -> Result<(m::SequenceId, m::Clip, m::Grade), String> {
    let c = model_clip(doc, id).ok_or_else(|| format!("no clip {id}"))?;
    let seq = seq_of(doc, id).ok_or("clip not in a sequence")?;
    let g = c.grade.clone().unwrap_or(m::Grade { enabled: true, ..Default::default() });
    Ok((seq, c, g))
}

/// Applies `params`, `lut`, `reset` and `enabled` of `a` to `g`. True when
/// `a` asked for any change.
pub fn grade_args(g: &mut m::Grade, a: &Value, closest: impl Fn(&str) -> Option<String>) -> Result<bool, String> {
    if let Some(p) = a.get("params") {
        merge_params(&mut g.params, p, true, closest)?;
    }
    if let Some(l) = a.get("lut") {
        g.lut = l
            .as_str()
            .filter(|x| !x.is_empty())
            .map(|x| {
                let p = Path::new(x);
                studio_colour::read_lut(p).map(|_| x.to_string())
            })
            .transpose()?;
    }
    if a.get("reset").and_then(Value::as_bool) == Some(true) {
        g.params.clear();
        g.lut = None;
        g.set_nodes(&[]);
    }
    if let Some(e) = a.get("enabled").and_then(Value::as_bool) {
        g.enabled = e;
    }
    Ok(a.get("params").is_some() || a.get("lut").is_some() || a.get("reset").is_some() || a.get("enabled").is_some())
}

fn num(a: &Value, k: &str) -> Option<f64> {
    a.get(k).and_then(Value::as_f64)
}

/// Adds, edits or removes a node of `g` as the `colour node` arguments
/// say; windows live in `c.masks`. `alloc` hands out mask ids. Returns the
/// nodes after the change (also stored in `g`).
pub fn node_args(c: &mut m::Clip, g: &mut m::Grade, a: &Value, id: u32, alloc: &mut dyn FnMut() -> u64, closest: impl Fn(&str) -> Option<String>) -> Result<Vec<m::GradeNode>, String> {
    let mut nodes = g.nodes();
    let index = a.get("node").and_then(Value::as_u64).map(|n| n as usize);
    if a.get("remove").and_then(Value::as_bool) == Some(true) {
        let i = index.ok_or("remove needs \"node\" (1-based)")?;
        if i == 0 || i > nodes.len() {
            return Err(format!("no node {i}; the clip has {}", nodes.len()));
        }
        let n = nodes.remove(i - 1);
        if let Some(w) = n.window {
            c.masks.retain(|mk| mk.id != w);
        }
    } else if ["params", "qualifier", "window", "mix", "name", "key_blur", "enabled", "lut", "window_invert"].iter().any(|k| a.get(*k).is_some()) {
        let i = match index {
            Some(i) if i >= 1 && i <= nodes.len() => i - 1,
            Some(i) => return Err(format!("no node {i}; the clip has {} (leave \"node\" out to add one)", nodes.len())),
            None => {
                nodes.push(m::GradeNode::default());
                nodes.len() - 1
            }
        };
        let n = &mut nodes[i];
        if let Some(p) = a.get("params") {
            merge_params(&mut n.params, p, true, &closest)?;
        }
        match a.get("qualifier") {
            Some(Value::String(preset)) => set_qualifier_preset(n, preset)?,
            Some(Value::Null) => n.qualifier.clear(),
            Some(q @ Value::Object(_)) => {
                merge_params(&mut n.qualifier, q, false, &closest)?;
            }
            _ => {}
        }
        if let Some(name) = a.get("name").and_then(Value::as_str) {
            n.name = name.to_string();
        }
        if let Some(v) = num(a, "mix") {
            n.mix = if v > 1.0 { v / 100.0 } else { v }.clamp(0.0, 1.0);
        }
        if let Some(v) = num(a, "key_blur") {
            n.key_blur = v.max(0.0);
        }
        if let Some(v) = a.get("enabled").and_then(Value::as_bool) {
            n.enabled = v;
        }
        if let Some(v) = a.get("window_invert").and_then(Value::as_bool) {
            n.window_invert = v;
        }
        if let Some(l) = a.get("lut") {
            n.lut = l.as_str().filter(|x| !x.is_empty()).map(str::to_owned);
        }
        match a.get("window") {
            Some(Value::Null) => {
                if let Some(w) = n.window.take() {
                    c.masks.retain(|mk| mk.id != w);
                }
            }
            Some(Value::Number(mid)) => {
                let mid = m::MaskId(mid.as_u64().unwrap_or(0));
                if !c.masks.iter().any(|mk| mk.id == mid) {
                    return Err(format!("clip {id} has no mask {}", mid.0));
                }
                n.window = Some(mid);
            }
            Some(Value::Object(o)) => {
                let shape = match o.get("shape").and_then(Value::as_str).unwrap_or("ellipse") {
                    "rectangle" | "rect" | "box" => m::MaskShape::Rectangle,
                    "ellipse" | "circle" | "oval" => m::MaskShape::Ellipse,
                    "gradient" | "line" => m::MaskShape::Line,
                    o => return Err(format!("window shape \"{o}\": ellipse, rectangle or gradient (or a mask id)")),
                };
                let mut mask = match n.window.and_then(|w| c.masks.iter().find(|mk| mk.id == w).cloned()) {
                    Some(mk) => mk,
                    None => m::Mask::new(m::MaskId(alloc()), shape),
                };
                mask.shape = shape;
                let g = |k: &str| o.get(k).and_then(Value::as_f64);
                if let (Some(x), Some(y)) = (g("x"), g("y")) {
                    mask.params.insert("position".into(), m::Value::Point(m::Vec2::new(x, y)));
                }
                if let (Some(w), Some(h)) = (g("width"), g("height")) {
                    mask.params.insert("size".into(), m::Value::Point(m::Vec2::new(w, h)));
                }
                for k in ["feather", "rotation", "roundness", "expansion"] {
                    if let Some(v) = g(k) {
                        mask.params.insert(k.into(), m::Value::Scalar(v));
                    }
                }
                mask.extra.insert("role".into(), json!("grade_window"));
                let mid = mask.id;
                c.masks.retain(|mk| mk.id != mid);
                c.masks.push(mask);
                n.window = Some(mid);
            }
            _ => {}
        }
    }
    g.set_nodes(&nodes);
    Ok(nodes)
}

/// Keys a node on a qualifier preset (`skin`, `sky`, …).
pub fn set_qualifier_preset(n: &mut m::GradeNode, preset: &str) -> Result<(), String> {
    studio_colour::Qualifier::preset(preset).ok_or_else(|| format!("no qualifier preset \"{preset}\"; skin, sky, foliage, red, orange, yellow, green, cyan, blue, magenta, highlights, midtones, shadows"))?;
    n.qualifier.clear();
    n.qualifier.insert("preset".into(), m::Value::Text(preset.to_string()));
    Ok(())
}

/// The node's qualifier preset, if it is keyed on one.
pub fn qualifier_preset(n: &m::GradeNode) -> Option<String> {
    match n.qualifier.get("preset") {
        Some(m::Value::Text(t)) => Some(t.clone()),
        _ => None,
    }
}

/// Qualifier presets the panel offers, in order.
pub const QUALIFIER_PRESETS: &[&str] = &["skin", "sky", "foliage", "red", "orange", "yellow", "green", "cyan", "blue", "magenta", "highlights", "midtones", "shadows"];

/// Writes the grade's CDL (`cdl_*`) of each `(clip, params)` as ASC CDL XML.
pub fn cdl_xml(grades: &[(u32, m::ParamMap)]) -> String {
    let mut xml = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<ColorDecisionList xmlns=\"urn:ASC:CDL:v1.01\">\n");
    for (id, p) in grades {
        let n = |k: &str, d: f64| match p.get(k) {
            Some(m::Value::Scalar(v)) => *v,
            _ => d,
        };
        let tri = |k: &str, d: f64| format!("{:.6} {:.6} {:.6}", n(&format!("cdl_{k}_r"), d), n(&format!("cdl_{k}_g"), d), n(&format!("cdl_{k}_b"), d));
        xml.push_str(&format!(
            "  <ColorDecision>\n    <ColorCorrection id=\"clip_{id}\">\n      <SOPNode>\n        <Slope>{}</Slope>\n        <Offset>{}</Offset>\n        <Power>{}</Power>\n      </SOPNode>\n      <SatNode>\n        <Saturation>{:.6}</Saturation>\n      </SatNode>\n    </ColorCorrection>\n  </ColorDecision>\n",
            tri("slope", 1.0),
            tri("offset", 0.0),
            tri("power", 1.0),
            n("cdl_sat", 1.0)
        ));
    }
    xml.push_str("</ColorDecisionList>\n");
    xml
}

/// The `cdl_*` values of an ASC CDL file's text; an error naming `path`
/// when it has none.
pub fn cdl_from_xml(x: &str, path: &str) -> Result<Map<String, Value>, String> {
    let mut p = Map::new();
    let tag = |t: &str| -> Option<Vec<f64>> {
        let i = x.find(&format!("<{t}>"))? + t.len() + 2;
        let j = x[i..].find(&format!("</{t}>"))? + i;
        Some(x[i..j].split_whitespace().filter_map(|v| v.parse().ok()).collect())
    };
    for (k, t) in [("slope", "Slope"), ("offset", "Offset"), ("power", "Power")] {
        if let Some(v) = tag(t).filter(|v| v.len() == 3) {
            for (c, x) in ["r", "g", "b"].iter().zip(v) {
                p.insert(format!("cdl_{k}_{c}"), json!(x));
            }
        }
    }
    if let Some(v) = tag("Saturation").and_then(|v| v.first().copied()) {
        p.insert("cdl_sat".into(), json!(v));
    }
    if p.is_empty() {
        return Err(format!("{path}: no ASC CDL (Slope / Offset / Power) found"));
    }
    Ok(p)
}

/// What a baked LUT could not carry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LutBake {
    /// The grade has spatial tools (clarity, sharpen, vignette, grain, dehaze).
    pub spatial: bool,
    /// Nodes with a window, skipped.
    pub windows: usize,
}

/// Bakes `g` (and its nodes keyed by colour only) into a `size`³ LUT file at
/// `path`: Rec.709 / BT.1886 video levels in and out.
pub fn export_lut(g: &m::Grade, path: &Path, size: usize, title: &str) -> Result<LutBake, String> {
    let desc = studio_render::scene::GradeDesc { params: studio_render::model_scene::convert_params(&g.params), lut: g.lut.clone() };
    let grade = studio_render::effects::grade_from_desc(&desc);
    let proc = grade.prepare();
    // Nodes keyed by colour only bake too; windows cannot.
    let nodes: Vec<(studio_colour::GradeProcessor, Option<studio_colour::Qualifier>, f32)> = g
        .nodes()
        .iter()
        .filter(|n| n.enabled && n.window.is_none())
        .map(|n| {
            let d = studio_render::scene::GradeDesc { params: studio_render::model_scene::convert_params(&n.params), lut: n.lut.clone() };
            (studio_render::effects::grade_from_desc(&d).prepare(), studio_render::nodes::qualifier_from(&studio_render::model_scene::convert_params(&n.qualifier)), n.mix as f32)
        })
        .collect();
    let windows = g.nodes().iter().filter(|n| n.window.is_some()).count();
    let bt = studio_colour::Transfer::Bt1886;
    let lut = studio_colour::lut::Lut3d::from_fn(size, |enc| {
        let mut lin = proc.apply_rgb(bt.decode_rgb(enc));
        for (p, q, mix) in &nodes {
            let k = q.as_ref().map_or(1.0, |q| q.key(lin)) * mix;
            let out = p.apply_rgb(lin);
            lin = [0, 1, 2].map(|i| lin[i] + k * (out[i] - lin[i]));
        }
        bt.encode_rgb(lin).map(|v| v.clamp(0.0, 1.0))
    });
    studio_colour::write_lut(path, &lut, title)?;
    let spatial = grade.basic.clarity != 0.0 || grade.basic.sharpen.amount != 0.0 || grade.basic.vignette.amount != 0.0 || grade.basic.grain.amount != 0.0 || grade.basic.dehaze != 0.0;
    Ok(LutBake { spatial, windows })
}

/// Output colour space by any of its names.
pub fn parse_output_space(space: &str) -> Result<m::ColourSpace, String> {
    Ok(match space.to_ascii_lowercase().replace(['.', ' ', '-'], "_").as_str() {
        "rec709" | "rec_709" | "bt709" | "sdr" => m::ColourSpace::Rec709,
        "p3" | "displayp3" | "display_p3" | "p3_d65" => m::ColourSpace::DisplayP3,
        "rec2020_pq" | "pq" | "hdr10" | "rec2100_pq" => m::ColourSpace::Rec2020Pq,
        "rec2020_hlg" | "hlg" | "rec2100_hlg" => m::ColourSpace::Rec2020Hlg,
        o => return Err(format!("no output space \"{o}\"; rec709, p3, rec2020_pq, rec2020_hlg")),
    })
}

/// Sets the view transform (and the HDR peak, when given) in `st` after
/// checking it parses against the project's OCIO config.
pub fn set_view(st: &mut m::ProjectSettings, view: &str, hdr_peak: Option<f64>) -> Result<(), String> {
    if let Some(p) = hdr_peak {
        st.colour.hdr_peak = Some(p.round() as u32);
    }
    let cfg = st.colour.ocio_config.as_deref().map(std::path::PathBuf::from);
    studio_render::view::ViewTransform::parse(view, cfg.as_deref(), st.colour.hdr_peak)?;
    st.colour.view = if view.eq_ignore_ascii_case("none") { String::new() } else { view.trim().to_string() };
    Ok(())
}

/// Sets the output colour space (and the HDR peak, when given) in `st`.
pub fn set_output(st: &mut m::ProjectSettings, space: &str, hdr_peak: Option<f64>) -> Result<m::ColourSpace, String> {
    let cs = parse_output_space(space)?;
    st.colour_space = cs;
    if let Some(p) = hdr_peak {
        st.colour.hdr_peak = Some(p.round() as u32);
    }
    Ok(cs)
}

/// Sets (or, for an empty `path`, clears) the OCIO config in `st`; the
/// loaded config when there is one.
pub fn set_ocio(st: &mut m::ProjectSettings, path: &str) -> Result<Option<std::sync::Arc<studio_colour::ocio::Config>>, String> {
    if path.is_empty() {
        st.colour.ocio_config = None;
        return Ok(None);
    }
    let cfg = studio_render::colour_space::ocio_config(Path::new(path))?;
    st.colour.ocio_config = Some(path.to_string());
    Ok(Some(cfg))
}

/// Applies `f` to clip `id` and its grade as one undo step (or, with a
/// gesture key, merged with the other changes of that drag). False when
/// nothing changed or the edit was refused (`doc.error` says why).
pub fn edit_grade(doc: &mut Doc, id: u32, label: &str, gesture: Option<&str>, f: impl FnOnce(&mut m::Clip, &mut m::Grade)) -> bool {
    let Ok((seq, mut c, mut g)) = clip_grade(doc, id) else { return false };
    let before = c.clone();
    f(&mut c, &mut g);
    c.grade = Some(g);
    if c == before {
        return false;
    }
    let edit = m::Edit::SetClip { seq, clip: Box::new(c) };
    match gesture {
        Some(k) => doc.apply_gesture(k, label, edit),
        None => doc.apply(label, edit),
    }
}

/// Replaces clip `id` with `clip` (its grade and masks changed) as one undo
/// step, or merged into the drag `gesture`. False when nothing differs.
pub fn put_clip(doc: &mut Doc, id: u32, label: &str, gesture: Option<&str>, clip: m::Clip) -> bool {
    let Some(seq) = seq_of(doc, id) else { return false };
    if model_clip(doc, id).as_ref() == Some(&clip) {
        return false;
    }
    let edit = m::Edit::SetClip { seq, clip: Box::new(clip) };
    match gesture {
        Some(k) => doc.apply_gesture(k, label, edit),
        None => doc.apply(label, edit),
    }
}

/// Edits the project settings as one undo step.
pub fn edit_settings(doc: &mut Doc, label: &str, f: impl FnOnce(&mut m::ProjectSettings)) -> bool {
    let mut st = doc.model.settings.clone();
    f(&mut st);
    if st == doc.model.settings {
        return false;
    }
    doc.apply(label, m::Edit::SetSettings { settings: Box::new(st) })
}

// ------------------------------------------------------------ panel data

/// Wheels the panel shows, with their parameter prefix.
pub const WHEELS: &[(&str, &str)] = &[("lift", "Lift"), ("gamma", "Gamma"), ("gain", "Gain")];

/// A wheel's `[hue (degrees), amount 0..1, luma −1..1]`.
pub fn wheel(p: &m::ParamMap, name: &str) -> [f64; 3] {
    [scalar(p, &format!("{name}_hue"), 0.0), scalar(p, &format!("{name}_amount"), 0.0), scalar(p, &format!("{name}_luma"), 0.0)]
}

/// Sets a wheel; a neutral wheel leaves no parameters behind.
pub fn set_wheel(p: &mut m::ParamMap, name: &str, w: [f64; 3]) {
    let neutral = w[1] == 0.0 && w[2] == 0.0;
    for (k, v) in ["hue", "amount", "luma"].iter().zip(w) {
        let key = format!("{name}_{k}");
        if neutral {
            p.remove(&key);
        } else {
            p.insert(key, m::Value::Scalar(v));
        }
    }
}

/// Whether a wheel is off its neutral position.
pub fn wheel_set(p: &m::ParamMap, name: &str) -> bool {
    let w = wheel(p, name);
    w[1] != 0.0 || w[2] != 0.0
}

/// A scalar parameter, or `default`.
pub fn scalar(p: &m::ParamMap, key: &str, default: f64) -> f64 {
    match p.get(key) {
        Some(m::Value::Scalar(v)) | Some(m::Value::Angle(v)) => *v,
        _ => default,
    }
}

/// Sets a scalar parameter; the default value removes it.
pub fn set_scalar(p: &mut m::ParamMap, key: &str, v: f64, default: f64) {
    if (v - default).abs() < 1e-9 {
        p.remove(key);
    } else {
        p.insert(key.to_string(), m::Value::Scalar(v));
    }
}

/// The curves the panel offers: parameter name and label.
pub const CURVES: &[(&str, &str)] = &[("curve_master", "Master"), ("curve_red", "Red"), ("curve_green", "Green"), ("curve_blue", "Blue")];

/// A curve's control points (the straight line when it has none).
pub fn curve(p: &m::ParamMap, key: &str) -> Vec<[f64; 2]> {
    match p.get(key) {
        Some(m::Value::Curve(c)) if c.len() >= 2 => c.clone(),
        _ => vec![[0.0, 0.0], [1.0, 1.0]],
    }
}

/// Sets a curve; the straight line removes it. Points stay sorted by input.
pub fn set_curve(p: &mut m::ParamMap, key: &str, mut pts: Vec<[f64; 2]>) {
    for q in &mut pts {
        q[0] = q[0].clamp(0.0, 1.0);
        q[1] = q[1].clamp(0.0, 1.0);
    }
    pts.sort_by(|a, b| a[0].total_cmp(&b[0]));
    pts.truncate(16);
    if pts.len() < 2 || pts == [[0.0, 0.0], [1.0, 1.0]] {
        p.remove(key);
    } else {
        p.insert(key.to_string(), m::Value::Curve(pts));
    }
}

/// The ASC CDL fields: slope, offset, power per channel, then saturation.
pub const CDL_FIELDS: &[(&str, f64)] = &[
    ("cdl_slope_r", 1.0), ("cdl_slope_g", 1.0), ("cdl_slope_b", 1.0),
    ("cdl_offset_r", 0.0), ("cdl_offset_g", 0.0), ("cdl_offset_b", 0.0),
    ("cdl_power_r", 1.0), ("cdl_power_g", 1.0), ("cdl_power_b", 1.0),
    ("cdl_sat", 1.0),
];

/// Whether any CDL field is off its default.
pub fn cdl_set(p: &m::ParamMap) -> bool {
    CDL_FIELDS.iter().any(|(k, d)| (scalar(p, k, *d) - d).abs() > 1e-9)
}

/// Window shapes a node offers: label and mask shape.
pub const WINDOW_SHAPES: &[(&str, m::MaskShape)] = &[("Ellipse", m::MaskShape::Ellipse), ("Rectangle", m::MaskShape::Rectangle), ("Gradient", m::MaskShape::Line)];

/// Gives node `index` a window of `shape`, centred and half the picture
/// (keeping the shape's position and size when it already has a window).
pub fn set_node_window(c: &mut m::Clip, nodes: &mut [m::GradeNode], index: usize, shape: Option<m::MaskShape>, alloc: &mut dyn FnMut() -> u64) {
    let Some(n) = nodes.get_mut(index) else { return };
    match shape {
        None => {
            if let Some(w) = n.window.take() {
                c.masks.retain(|mk| mk.id != w);
            }
        }
        Some(shape) => {
            let mut mask = match n.window.and_then(|w| c.masks.iter().find(|mk| mk.id == w).cloned()) {
                Some(mk) => mk,
                None => {
                    let mut mk = m::Mask::new(m::MaskId(alloc()), shape);
                    mk.params.insert("position".into(), m::Value::Point(m::Vec2::new(0.5, 0.5)));
                    mk.params.insert("size".into(), m::Value::Point(m::Vec2::new(0.5, 0.5)));
                    mk
                }
            };
            mask.shape = shape;
            mask.extra.insert("role".into(), json!("grade_window"));
            let id = mask.id;
            c.masks.retain(|mk| mk.id != id);
            c.masks.push(mask);
            n.window = Some(id);
        }
    }
}

/// The shape of node `index`'s window.
pub fn node_window_shape(c: &m::Clip, n: &m::GradeNode) -> Option<m::MaskShape> {
    let w = n.window?;
    c.masks.iter().find(|mk| mk.id == w).map(|mk| mk.shape)
}

/// Replaces the clip's grade with a still's, keeping this clip's own shot
/// match (`match_*`).
pub fn apply_still_grade(c: &mut m::Clip, still: &m::Grade) {
    let mut ng = still.clone();
    if let Some(old) = &c.grade {
        for (k, v) in old.params.iter().filter(|(k, _)| k.starts_with("match_")) {
            ng.params.insert(k.clone(), v.clone());
        }
    }
    c.grade = Some(ng);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wheel_that_is_neutral_leaves_nothing_behind() {
        let mut p = m::ParamMap::new();
        set_wheel(&mut p, "lift", [200.0, 0.4, 0.1]);
        assert!(wheel_set(&p, "lift"));
        assert_eq!(wheel(&p, "lift"), [200.0, 0.4, 0.1]);
        set_wheel(&mut p, "lift", [200.0, 0.0, 0.0]);
        assert!(p.is_empty(), "{p:?}");
    }

    #[test]
    fn curves_stay_sorted_bounded_and_the_straight_line_is_no_curve() {
        let mut p = m::ParamMap::new();
        set_curve(&mut p, "curve_master", vec![[1.0, 1.0], [0.5, 0.4], [0.0, 0.0], [2.0, 3.0]]);
        let c = curve(&p, "curve_master");
        assert!(c.windows(2).all(|w| w[0][0] <= w[1][0]) && c.iter().all(|q| q.iter().all(|v| (0.0..=1.0).contains(v))), "{c:?}");
        set_curve(&mut p, "curve_master", vec![[0.0, 0.0], [1.0, 1.0]]);
        assert!(p.is_empty());
        set_curve(&mut p, "curve_red", (0..40).map(|i| [i as f64 / 40.0, 0.5]).collect());
        assert!(curve(&p, "curve_red").len() <= 16);
    }

    #[test]
    fn cdl_round_trips_through_xml() {
        let mut p = m::ParamMap::new();
        for (k, v) in [("cdl_slope_r", 1.2), ("cdl_offset_b", -0.05), ("cdl_power_g", 0.9), ("cdl_sat", 0.8)] {
            set_scalar(&mut p, k, v, CDL_FIELDS.iter().find(|(n, _)| *n == k).unwrap().1);
        }
        assert!(cdl_set(&p));
        let xml = cdl_xml(&[(7, p.clone())]);
        let back = cdl_from_xml(&xml, "x.cdl").unwrap();
        assert_eq!(back["cdl_slope_r"], json!(1.2));
        assert_eq!(back["cdl_sat"], json!(0.8));
        assert!(cdl_from_xml("nothing", "x.cdl").unwrap_err().contains("no ASC CDL"));
    }

    #[test]
    fn a_node_window_is_made_changed_and_removed_with_its_mask() {
        let mut c = m::Clip::new(m::ClipId(1), m::LaneId(1), 0, 1_000_000, m::ClipSource::Gap);
        let mut nodes = vec![m::GradeNode::default()];
        let mut next = 10;
        let mut alloc = || {
            next += 1;
            next
        };
        set_node_window(&mut c, &mut nodes, 0, Some(m::MaskShape::Ellipse), &mut alloc);
        assert_eq!(c.masks.len(), 1);
        assert_eq!(node_window_shape(&c, &nodes[0]), Some(m::MaskShape::Ellipse));
        set_node_window(&mut c, &mut nodes, 0, Some(m::MaskShape::Rectangle), &mut alloc);
        assert_eq!(c.masks.len(), 1, "the same mask changes shape");
        assert_eq!(node_window_shape(&c, &nodes[0]), Some(m::MaskShape::Rectangle));
        set_node_window(&mut c, &mut nodes, 0, None, &mut alloc);
        assert!(c.masks.is_empty() && nodes[0].window.is_none());
    }

    #[test]
    fn qualifier_presets_are_known_names() {
        let mut n = m::GradeNode::default();
        for p in QUALIFIER_PRESETS {
            set_qualifier_preset(&mut n, p).unwrap();
            assert_eq!(qualifier_preset(&n).as_deref(), Some(*p));
        }
        assert!(set_qualifier_preset(&mut n, "purple haze").unwrap_err().contains("no qualifier preset"));
    }

    #[test]
    fn output_spaces_take_their_common_names() {
        assert_eq!(parse_output_space("HDR10").unwrap(), m::ColourSpace::Rec2020Pq);
        assert_eq!(parse_output_space("rec.709").unwrap(), m::ColourSpace::Rec709);
        assert!(parse_output_space("xyz").is_err());
    }

    #[test]
    fn a_lut_baked_from_a_grade_reads_back() {
        let dir = std::env::temp_dir().join(format!("zornov-grade-lut-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("look.cube");
        let mut g = m::Grade { enabled: true, ..Default::default() };
        set_scalar(&mut g.params, "saturation", 0.5, 0.0);
        let bake = export_lut(&g, &path, 5, "Test").unwrap();
        assert_eq!(bake, LutBake { spatial: false, windows: 0 });
        let cube = studio_colour::read_lut(&path).unwrap();
        assert_eq!(cube.lut3d.as_ref().map(|l| l.size), Some(5));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
