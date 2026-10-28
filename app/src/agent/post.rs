// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Post-production tools for agents:
//!
//! * `colour`: colour management (input colour spaces per media, view
//!   transform, output space, OCIO configs), the full grade (wheels,
//!   curves, HSL, ASC CDL, LUTs), grade nodes with qualifiers and tracked
//!   windows, LUT export, HDR measurements and the stills gallery.
//! * `conform`: EDL / FCPXML / OTIO import and export, relinking offline
//!   media by name, reel and timecode.
//! * `media_manage`: source timecode and reels, proxies, consolidate,
//!   multicam (sync, build, cut to an angle).

use super::{Content, Session, f, micros, secs};
use crate::services;
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};
use studio_model as m;

fn text(v: Value) -> Result<Vec<Content>, String> {
    Ok(vec![Content::Text(v.to_string())])
}

fn s<'a>(a: &'a Value, k: &str) -> Option<&'a str> {
    a.get(k).and_then(Value::as_str)
}

/// JSON number / string / list / object → model parameter value.
fn to_value(v: &Value) -> Option<m::Value> {
    Some(match v {
        Value::Bool(b) => m::Value::Bool(*b),
        Value::Number(n) => m::Value::Scalar(n.as_f64()?),
        Value::String(t) => m::Value::Text(t.clone()),
        Value::Array(a) if a.iter().all(|p| p.is_array()) => m::Value::Curve(a.iter().filter_map(|p| Some([p.get(0)?.as_f64()?, p.get(1)?.as_f64()?])).collect()),
        Value::Array(a) if a.len() == 2 => m::Value::Point(m::Vec2::new(a[0].as_f64()?, a[1].as_f64()?)),
        _ => return None,
    })
}

fn from_value(v: &m::Value) -> Value {
    match v {
        m::Value::Bool(b) => json!(b),
        m::Value::Scalar(x) | m::Value::Angle(x) => json!((x * 1000.0).round() / 1000.0),
        m::Value::Point(p) => json!([p.x, p.y]),
        m::Value::Text(t) | m::Value::Choice(t) => json!(t),
        m::Value::Curve(c) => json!(c),
        m::Value::Colour(c) => json!([c.r, c.g, c.b, c.a]),
    }
}

fn params_json(p: &m::ParamMap) -> Value {
    Value::Object(p.iter().map(|(k, v)| (k.clone(), from_value(v))).collect())
}

/// Grade parameter names this tool accepts (besides `cdl_*`, curves and
/// `hsl_*` patterns).
const GRADE_KEYS: &[&str] = &[
    "exposure", "temperature", "tint", "contrast", "highlights", "shadows", "whites", "blacks", "saturation", "vibrance", "clarity", "dehaze", "fade", "sharpen", "vignette", "grain",
    "wheel_mode", "wheel_intensity", "look", "look_intensity", "lut_intensity", "lut_skin_protect",
    "curve_master", "curve_red", "curve_green", "curve_blue", "hue_vs_hue", "hue_vs_sat", "hue_vs_lum", "lum_vs_sat", "sat_vs_sat",
];

fn grade_key_ok(k: &str) -> bool {
    GRADE_KEYS.contains(&k)
        || k.starts_with("cdl_")
        || k.starts_with("hsl_")
        || ["lift", "gamma", "gain", "offset", "shadows", "midtones", "highlights"].iter().any(|w| k.starts_with(&format!("{w}_")) && ["hue", "amount", "luma"].iter().any(|x| k.ends_with(x)))
}

/// Other names for grade parameters agents reach for (`curve_r` failed a
/// whole colour task).
fn grade_alias(k: &str) -> Option<&'static str> {
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

/// Merges `{name: value | null}` into a parameter map.
fn merge_params(into: &mut m::ParamMap, given: &Value, check: bool) -> Result<Vec<String>, String> {
    let obj = given.as_object().ok_or("params is an object of name: value")?;
    let mut changed = Vec::new();
    for (k, v) in obj {
        let k = &grade_alias(k).map_or_else(|| k.clone(), str::to_owned);
        if check && !grade_key_ok(k) {
            let hint = super::args::closest(k, GRADE_KEYS).map(|c| format!(" (did you mean \"{c}\"?)")).unwrap_or_default();
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

impl Session {
    fn media_ids(&self, v: Option<&Value>) -> Result<Vec<m::MediaId>, String> {
        match v {
            None => Err("say which media (an id, a name, a list, or \"all\")".into()),
            Some(Value::String(a)) if a == "all" => Ok(self.doc.model.media.iter().map(|i| i.id).collect()),
            Some(Value::Array(list)) => list.iter().map(|x| super::craft::media_of(&self.doc.project, x).map(|i| m::MediaId(i as u64))).collect(),
            Some(x) => Ok(vec![m::MediaId(super::craft::media_of(&self.doc.project, x)? as u64)]),
        }
    }

    fn clip_ids(&self, a: &Value) -> Vec<u32> {
        let mut v = super::ids(a, "clips");
        if let Some(c) = a.get("clip").and_then(Value::as_u64) {
            v.push(c as u32);
        }
        v
    }

    fn seq_of(&self, clip: u32) -> Option<m::SequenceId> {
        self.doc.model.sequences.iter().find(|s| s.clip(m::ClipId(clip as u64)).is_some()).map(|s| s.id)
    }

    fn ocio_path(&self) -> Option<PathBuf> {
        self.doc.model.settings.colour.ocio_config.as_deref().map(PathBuf::from)
    }

    fn apply_edit(&mut self, label: &str, edit: m::Edit) -> Result<(), String> {
        let ok = self.doc.apply(label, edit);
        self.check(ok, "").map(|_| ())
    }

    // ------------------------------------------------------------ colour

    pub(super) fn colour_tool(&mut self, a: &Value) -> Result<Vec<Content>, String> {
        let action = s(a, "action").unwrap_or("spaces");
        match action {
            "spaces" | "info" => self.colour_info(),
            "input" => {
                let ids = self.media_ids(a.get("media"))?;
                let space = s(a, "space").ok_or("input needs \"space\" (see action spaces), or \"auto\" for what the file says")?;
                let cfg = self.ocio_path();
                studio_render::colour_space::InputColour::parse(space, cfg.as_deref())?;
                let auto = matches!(space.trim().to_ascii_lowercase().as_str(), "" | "auto" | "file" | "default");
                let edits = ids
                    .iter()
                    .filter_map(|id| self.doc.model.media_item(*id))
                    .map(|i| {
                        let mut it = i.clone();
                        it.input_colour = (!auto).then(|| space.trim().to_string());
                        m::Edit::SetMedia { item: Box::new(it) }
                    })
                    .collect();
                self.apply_edit("Input colour space", m::Edit::Batch { edits })?;
                let mut note = String::new();
                if self.doc.model.settings.colour.view.is_empty() && studio_colour::CameraSpace::parse(space).is_some() {
                    note = " Log footage is scene-linear now: set a view transform (colour action view, view \"aces\") or grade it, or highlights clip.".into();
                }
                text(json!({ "media": ids.iter().map(|i| i.0).collect::<Vec<_>>(), "input": if auto { "from the file".to_string() } else { space.to_string() }, "note": note.trim() }))
            }
            "view" => {
                let view = s(a, "view").or_else(|| s(a, "space")).ok_or("view needs \"view\": none, aces, aces:<peak nits> or ocio:<display>/<view>")?;
                let mut st = self.doc.model.settings.clone();
                if let Some(p) = f(a, "hdr_peak") {
                    st.colour.hdr_peak = Some(p.round() as u32);
                }
                let cfg = self.ocio_path();
                studio_render::view::ViewTransform::parse(view, cfg.as_deref(), st.colour.hdr_peak)?;
                st.colour.view = if view.eq_ignore_ascii_case("none") { String::new() } else { view.trim().to_string() };
                self.apply_edit("View transform", m::Edit::SetSettings { settings: Box::new(st) })?;
                text(json!({ "view": view, "hdr_peak": self.doc.model.settings.colour.hdr_peak.unwrap_or(1000) }))
            }
            "output" => {
                let space = s(a, "space").ok_or("output needs \"space\": rec709, p3, rec2020_pq or rec2020_hlg")?;
                let cs = match space.to_ascii_lowercase().replace(['.', ' ', '-'], "_").as_str() {
                    "rec709" | "rec_709" | "bt709" | "sdr" => m::ColourSpace::Rec709,
                    "p3" | "displayp3" | "display_p3" | "p3_d65" => m::ColourSpace::DisplayP3,
                    "rec2020_pq" | "pq" | "hdr10" | "rec2100_pq" => m::ColourSpace::Rec2020Pq,
                    "rec2020_hlg" | "hlg" | "rec2100_hlg" => m::ColourSpace::Rec2020Hlg,
                    o => return Err(format!("no output space \"{o}\"; rec709, p3, rec2020_pq, rec2020_hlg")),
                };
                let mut st = self.doc.model.settings.clone();
                st.colour_space = cs;
                if let Some(p) = f(a, "hdr_peak") {
                    st.colour.hdr_peak = Some(p.round() as u32);
                }
                self.apply_edit("Output colour space", m::Edit::SetSettings { settings: Box::new(st) })?;
                text(json!({ "output": format!("{cs:?}") }))
            }
            "ocio" => {
                let path = s(a, "config").or_else(|| s(a, "path")).unwrap_or("");
                let mut st = self.doc.model.settings.clone();
                if path.is_empty() {
                    st.colour.ocio_config = None;
                    self.apply_edit("OCIO config", m::Edit::SetSettings { settings: Box::new(st) })?;
                    return text(json!({ "ocio": null }));
                }
                let cfg = studio_render::colour_space::ocio_config(Path::new(path))?;
                st.colour.ocio_config = Some(path.to_string());
                self.apply_edit("OCIO config", m::Edit::SetSettings { settings: Box::new(st) })?;
                let unusable: Vec<String> = cfg.spaces.iter().filter(|c| cfg.to_working(&c.name).is_err()).map(|c| c.name.clone()).collect();
                text(json!({
                    "config": cfg.name,
                    "colour_spaces": cfg.spaces.iter().filter(|c| !c.display_referred).map(|c| c.name.clone()).collect::<Vec<_>>(),
                    "displays": cfg.displays.iter().map(|d| json!({ "display": d.name, "views": d.views.iter().map(|v| v.name.clone()).collect::<Vec<_>>() })).collect::<Vec<_>>(),
                    "roles": cfg.roles.iter().map(|(r, c)| format!("{r}: {c}")).collect::<Vec<_>>(),
                    "working_space": cfg.working_space().map(|w| w.name.clone()),
                    "cannot_use": unusable,
                    "how": "input colour per media: space \"ocio:<colour space>\"; view: view \"ocio:<display>/<view>\"",
                }))
            }
            "grade" => self.grade_set(a),
            "node" | "nodes" => self.grade_node(a),
            "lut" => self.lut_tool(a),
            "cdl" => self.cdl_tool(a),
            "hdr" => self.hdr_tool(a),
            "stills" | "still" => self.stills_tool(a),
            o => Err(format!("colour has no action \"{o}\"; actions: spaces, input, view, output, ocio, grade, node, lut, cdl, hdr, stills")),
        }
    }

    fn colour_info(&mut self) -> Result<Vec<Content>, String> {
        let st = &self.doc.model.settings;
        let media: Vec<Value> = self
            .doc
            .model
            .media
            .iter()
            .filter(|i| i.has_video())
            .map(|i| json!({ "id": i.id.0, "name": i.name, "input": i.input_colour.clone().unwrap_or_else(|| "from the file".into()), "file_says": [i.info.video.as_ref().map(|v| v.colour.primaries.clone()), i.info.video.as_ref().map(|v| v.colour.transfer.clone())] }))
            .collect();
        let cameras: Vec<Value> = studio_colour::CameraSpace::presets().into_iter().map(|c| json!([c.slug(), c.name()])).collect();
        text(json!({
            "working_space": "scene-linear, BT.709 primaries, unbounded (wider gamuts kept as negative values); grading runs in ACEScct / AP1",
            "view": if st.colour.view.is_empty() { "none (display-referred)".to_string() } else { st.colour.view.clone() },
            "output": format!("{:?}", st.colour_space),
            "hdr_peak": st.colour.hdr_peak.unwrap_or(1000),
            "ocio_config": st.colour.ocio_config,
            "media": media,
            "input_spaces": { "camera": cameras, "other": ["rec709", "srgb", "linear", "acescg", "aces2065-1", "acescct", "rec2020", "rec2020_pq", "rec2020_hlg", "p3d65", "displayp3", "cineon", "ocio:<colour space>"] },
            "views": ["none", "aces", "aces:<peak nits>", "ocio:<display>/<view>"],
            "outputs": ["rec709", "p3", "rec2020_pq", "rec2020_hlg"],
        }))
    }

    /// The clip's model grade, made if missing.
    fn model_grade(&self, id: u32) -> Result<(m::SequenceId, m::Clip, m::Grade), String> {
        let c = self.model_clip(id).ok_or_else(|| format!("no clip {id}"))?;
        let seq = self.seq_of(id).ok_or("clip not in a sequence")?;
        let g = c.grade.clone().unwrap_or(m::Grade { enabled: true, ..Default::default() });
        Ok((seq, c, g))
    }

    fn grade_set(&mut self, a: &Value) -> Result<Vec<Content>, String> {
        let ids = self.clip_ids(a);
        if ids.is_empty() {
            // Read the grade of nothing: list what can be set.
            return text(json!({ "params": GRADE_KEYS, "patterns": ["cdl_slope_r|g|b", "cdl_offset_*", "cdl_power_*", "cdl_sat", "<lift|gamma|gain|offset|shadows|midtones|highlights>_<hue|amount|luma>", "hsl_<band>_<hue|sat|light>"] }));
        }
        let mut edits = Vec::new();
        let mut out = Vec::new();
        for id in &ids {
            let (seq, mut c, mut g) = self.model_grade(*id)?;
            if let Some(p) = a.get("params") {
                merge_params(&mut g.params, p, true)?;
            }
            if let Some(l) = a.get("lut") {
                g.lut = l.as_str().filter(|x| !x.is_empty()).map(|x| {
                    let p = Path::new(x);
                    studio_colour::read_lut(p).map(|_| x.to_string())
                }).transpose()?;
            }
            if a.get("reset").and_then(Value::as_bool) == Some(true) {
                g.params.clear();
                g.lut = None;
                g.set_nodes(&[]);
            }
            if let Some(e) = a.get("enabled").and_then(Value::as_bool) {
                g.enabled = e;
            }
            out.push(json!({ "clip": id, "params": params_json(&g.params), "lut": g.lut, "nodes": g.nodes().len(), "enabled": g.enabled }));
            c.grade = Some(g);
            edits.push(m::Edit::SetClip { seq, clip: Box::new(c) });
        }
        if a.get("params").is_some() || a.get("lut").is_some() || a.get("reset").is_some() || a.get("enabled").is_some() {
            self.apply_edit("Grade", m::Edit::Batch { edits })?;
        }
        text(json!(out))
    }

    fn grade_node(&mut self, a: &Value) -> Result<Vec<Content>, String> {
        let id = a.get("clip").and_then(Value::as_u64).ok_or("node needs \"clip\"")? as u32;
        let (seq, mut c, mut g) = self.model_grade(id)?;
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
        } else if a.get("params").is_some() || a.get("qualifier").is_some() || a.get("window").is_some() || a.get("mix").is_some() || a.get("name").is_some() || a.get("key_blur").is_some() || a.get("enabled").is_some() || a.get("lut").is_some() || a.get("window_invert").is_some() {
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
                merge_params(&mut n.params, p, true)?;
            }
            match a.get("qualifier") {
                Some(Value::String(preset)) => {
                    studio_colour::Qualifier::preset(preset).ok_or_else(|| format!("no qualifier preset \"{preset}\"; skin, sky, foliage, red, orange, yellow, green, cyan, blue, magenta, highlights, midtones, shadows"))?;
                    n.qualifier.clear();
                    n.qualifier.insert("preset".into(), m::Value::Text(preset.clone()));
                }
                Some(Value::Null) => n.qualifier.clear(),
                Some(q @ Value::Object(_)) => {
                    merge_params(&mut n.qualifier, q, false)?;
                }
                _ => {}
            }
            if let Some(name) = s(a, "name") {
                n.name = name.to_string();
            }
            if let Some(v) = f(a, "mix") {
                n.mix = if v > 1.0 { v / 100.0 } else { v }.clamp(0.0, 1.0);
            }
            if let Some(v) = f(a, "key_blur") {
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
                        None => m::Mask::new(m::MaskId(self.doc.alloc_id()), shape),
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
        c.grade = Some(g);
        let windows: Vec<Value> = nodes.iter().enumerate().filter_map(|(i, n)| n.window.map(|w| json!({ "node": i + 1, "mask": w.0 }))).collect();
        let list: Vec<Value> = nodes
            .iter()
            .enumerate()
            .map(|(i, n)| json!({ "node": i + 1, "name": n.name, "enabled": n.enabled, "params": params_json(&n.params), "qualifier": params_json(&n.qualifier), "window": n.window.map(|w| w.0), "window_invert": n.window_invert, "mix": n.mix, "key_blur": n.key_blur, "lut": n.lut }))
            .collect();
        self.apply_edit("Grade node", m::Edit::SetClip { seq, clip: Box::new(c) })?;
        text(json!({ "clip": id, "nodes": list, "track_windows": if windows.is_empty() { Value::Null } else { json!({ "windows": windows, "how": "make a window follow a subject: track {clip, mask: <mask id>, point: [x, y]}" }) } }))
    }

    fn lut_tool(&mut self, a: &Value) -> Result<Vec<Content>, String> {
        if let Some(path) = s(a, "export") {
            let id = a.get("clip").and_then(Value::as_u64).ok_or("lut export needs \"clip\": the grade to bake")? as u32;
            let size = a.get("size").and_then(Value::as_u64).unwrap_or(33).clamp(2, 129) as usize;
            let c = self.model_clip(id).ok_or_else(|| format!("no clip {id}"))?;
            let g = c.grade.clone().unwrap_or_default();
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
            let skipped = g.nodes().iter().filter(|n| n.window.is_some()).count();
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
            studio_colour::write_lut(Path::new(path), &lut, &format!("Zornov clip {id}"))?;
            let spatial = grade.basic.clarity != 0.0 || grade.basic.sharpen.amount != 0.0 || grade.basic.vignette.amount != 0.0 || grade.basic.grain.amount != 0.0 || grade.basic.dehaze != 0.0;
            return text(json!({ "lut": path, "size": size, "input_output": "Rec.709 / BT.1886 video levels in and out", "not_in_lut": { "spatial_tools": spatial, "window_nodes": skipped } }));
        }
        let ids = self.clip_ids(a);
        let path = s(a, "path").or_else(|| s(a, "import")).ok_or("lut takes \"path\" + \"clip(s)\" to apply a .cube / .3dl, or \"export\": path + \"clip\" to bake a grade")?;
        studio_colour::read_lut(Path::new(path))?;
        let mut params = Map::new();
        params.insert("lut_intensity".into(), json!(f(a, "intensity").map(|v| if v > 1.0 { v / 100.0 } else { v }).unwrap_or(1.0)));
        self.grade_set(&json!({ "clips": ids, "lut": path, "params": params }))
    }

    fn cdl_tool(&mut self, a: &Value) -> Result<Vec<Content>, String> {
        if let Some(path) = s(a, "export") {
            let ids = self.clip_ids(a);
            let mut xml = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<ColorDecisionList xmlns=\"urn:ASC:CDL:v1.01\">\n");
            for id in &ids {
                let c = self.model_clip(*id).ok_or_else(|| format!("no clip {id}"))?;
                let p = c.grade.map(|g| g.params).unwrap_or_default();
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
            std::fs::write(path, xml).map_err(|e| e.to_string())?;
            return text(json!({ "cdl": path, "clips": ids }));
        }
        let ids = self.clip_ids(a);
        if ids.is_empty() {
            return Err("cdl needs \"clip\" or \"clips\"".into());
        }
        let mut p = Map::new();
        if let Some(path) = s(a, "path") {
            let x = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
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
        }
        for (k, d) in [("slope", 1.0), ("offset", 0.0), ("power", 1.0)] {
            if let Some(v) = a.get(k) {
                let tri: Vec<f64> = match v {
                    Value::Array(x) => x.iter().filter_map(Value::as_f64).collect(),
                    Value::Number(n) => vec![n.as_f64().unwrap_or(d); 3],
                    _ => Vec::new(),
                };
                if tri.len() != 3 {
                    return Err(format!("\"{k}\" is a number or [r, g, b]"));
                }
                for (c, x) in ["r", "g", "b"].iter().zip(tri) {
                    p.insert(format!("cdl_{k}_{c}"), json!(x));
                }
            }
        }
        if let Some(v) = f(a, "sat") {
            p.insert("cdl_sat".into(), json!(v));
        }
        self.grade_set(&json!({ "clips": ids, "params": p }))
    }

    /// Linear frames of the edit as the output sees them.
    fn linear_frames(&mut self, times: &[f64], width: u32) -> Result<Vec<studio_render::Frame>, String> {
        let registry = studio_render::source::SourceRegistry::new();
        crate::editor::render_view::register_sources(&self.doc, &registry);
        crate::editor::render_view::install_text_renderer(self.doc.project.height);
        let mut r = studio_render::best_renderer(studio_render::Sources::new(registry, studio_render::DecodeCache::new(128 << 20)));
        let scene = studio_render::model_scene::ModelScene::new(std::sync::Arc::new(self.doc.model.clone()));
        let w = width.min(self.doc.project.width.max(1));
        let h = (w as u64 * self.doc.project.height as u64 / self.doc.project.width.max(1) as u64).max(1) as u32;
        Ok(times.iter().map(|t| r.render(&scene, micros(*t), Some((w, h)))).collect())
    }

    fn hdr_tool(&mut self, a: &Value) -> Result<Vec<Content>, String> {
        let dur = secs(super::super::services::transport::duration(&self.doc));
        let times: Vec<f64> = match f(a, "t") {
            Some(t) => vec![t],
            None => {
                let n = a.get("count").and_then(Value::as_u64).unwrap_or(12).clamp(1, 120) as usize;
                let (t0, t1) = (f(a, "from").unwrap_or(0.0), f(a, "to").unwrap_or(dur));
                (0..n).map(|i| t0 + (t1 - t0) * (i as f64 + 0.5) / n as f64).collect()
            }
        };
        let frames = self.linear_frames(&times, 640)?;
        let mut ll = studio_colour::hdr::LightLevels::default();
        let mut per = Vec::new();
        for (t, fr) in times.iter().zip(&frames) {
            let img = fr.clone().into_colour();
            let st = studio_colour::hdr::measure(&img, 1);
            ll.add(&st);
            per.push(json!({ "t": t, "max_nits": st.max_cll.round(), "average_nits": st.frame_average.round(), "median_nits": st.median_nits.round(), "p99_nits": st.p99_nits.round(), "above_203": (st.above_203 * 1000.0).round() / 10.0, "above_1000": (st.above_1000 * 1000.0).round() / 10.0, "outside_709": (st.outside_709 * 1000.0).round() / 10.0, "outside_p3": (st.outside_p3 * 1000.0).round() / 10.0 }));
        }
        let hdr = matches!(self.doc.model.settings.colour_space, m::ColourSpace::Rec2020Pq | m::ColourSpace::Rec2020Hlg);
        text(json!({
            "output": format!("{:?}", self.doc.model.settings.colour_space),
            "scale": "cd/m², 1.0 working = 203 (graphics white); percentages of pixels",
            "max_cll": ll.max_cll.round(),
            "max_fall": ll.max_fall.round(),
            "note": if hdr { "MaxCLL / MaxFALL over the sampled frames (an estimate for the HDR10 metadata; sample more with count)" } else { "the output is SDR: values above 203 cd/m² clip at export; set output rec2020_pq for HDR" },
            "frames": per,
        }))
    }

    fn stills_tool(&mut self, a: &Value) -> Result<Vec<Content>, String> {
        if let Some(name) = s(a, "save") {
            let id = a.get("clip").and_then(Value::as_u64).ok_or("save needs \"clip\": the shot whose grade to keep")? as u32;
            let c = self.model_clip(id).ok_or_else(|| format!("no clip {id}"))?;
            let g = c.grade.clone().unwrap_or_default();
            let t = f(a, "t").unwrap_or(secs(c.start + c.duration / 2));
            let jpeg = self.frames.rgba(&self.doc, micros(t), 480).ok().map(|img| studio_content::jpeg::encode(&img, 85));
            let st = services::grade_stills::save(&self.doc, name, &g, jpeg.as_deref(), Some(id as u64), Some(t))?;
            return text(json!({ "still": st.name, "picture": st.picture, "from_clip": id, "at": t }));
        }
        if let Some(name) = s(a, "apply") {
            let st = services::grade_stills::get(&self.doc, name).ok_or_else(|| format!("no still \"{name}\"; see stills list"))?;
            let ids = self.clip_ids(a);
            if ids.is_empty() {
                return Err("apply needs \"clips\"".into());
            }
            let g = services::grade_stills::grade_for(&st);
            let mut edits = Vec::new();
            for id in &ids {
                let mut c = self.model_clip(*id).ok_or_else(|| format!("no clip {id}"))?;
                let seq = self.seq_of(*id).ok_or("clip not in a sequence")?;
                // Keep this clip's own shot match.
                let mut ng = g.clone();
                if let Some(old) = &c.grade {
                    for (k, v) in old.params.iter().filter(|(k, _)| k.starts_with("match_")) {
                        ng.params.insert(k.clone(), v.clone());
                    }
                }
                c.grade = Some(ng);
                edits.push(m::Edit::SetClip { seq, clip: Box::new(c) });
            }
            self.apply_edit("Apply still", m::Edit::Batch { edits })?;
            return text(json!({ "applied": st.name, "clips": ids }));
        }
        if let Some(name) = s(a, "remove") {
            services::grade_stills::remove(&self.doc, name)?;
            return text(json!({ "removed": name }));
        }
        let list: Vec<Value> = services::grade_stills::list(&self.doc).iter().map(|s| json!({ "name": s.name, "picture": s.picture, "from_clip": s.from_clip, "at": s.at, "params": s.grade.params.len(), "nodes": s.grade.nodes().len(), "lut": s.grade.lut })).collect();
        text(json!({ "stills": list, "folder": services::grade_stills::folder(&self.doc) }))
    }

    // ----------------------------------------------------------- conform

    pub(super) fn conform_tool(&mut self, a: &Value) -> Result<Vec<Content>, String> {
        let action = s(a, "action").unwrap_or("offline");
        let fmt_of = |p: &str| -> String {
            match Path::new(p).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase().as_str() {
                "edl" => "edl".into(),
                "fcpxml" | "xml" => "fcpxml".into(),
                "otio" => "otio".into(),
                o => o.to_string(),
            }
        };
        match action {
            "import" => {
                let path = s(a, "path").ok_or("import needs \"path\" (.edl, .fcpxml, .otio)")?;
                let text_in = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
                let fmt = s(a, "format").map(str::to_owned).unwrap_or_else(|| fmt_of(path));
                let mut p = self.doc.model.clone();
                let imp = match fmt.as_str() {
                    "edl" => {
                        let rate = s(a, "rate").and_then(studio_model::timecode::rate_from_name);
                        m::interchange::edl::import(&mut p, &text_in, m::interchange::edl::EdlImport { rate })
                    }
                    "fcpxml" => m::interchange::fcpxml::import(&mut p, &text_in),
                    "otio" => m::interchange::otio::import(&mut p, &text_in),
                    o => return Err(format!("no importer for \"{o}\"; edl, fcpxml, otio (AAF is not supported: export AAF as EDL, FCPXML or OTIO from the other system)")),
                }
                .map_err(|e| e.to_string())?;
                // Ids were taken from the copy: take them for the document too.
                while self.doc.model.next_id < p.next_id {
                    self.doc.alloc_id();
                }
                let seq = imp.sequence;
                self.apply_edit(&format!("Import {}", fmt.to_uppercase()), imp.edit)?;
                if a.get("open").and_then(Value::as_bool) != Some(false) {
                    self.apply_edit("Open sequence", m::Edit::SetActiveSequence { id: seq })?;
                }
                let offline: Vec<Value> = self.doc.model.media.iter().filter(|m| m.path.starts_with("offline:")).map(|m| json!({ "id": m.id.0, "reel": m.reel, "name": m.name })).collect();
                let name = self.doc.model.sequence(seq).map(|s| s.name.clone()).unwrap_or_default();
                text(json!({ "sequence": seq.0, "name": name, "report": imp.report.issues.iter().map(|i| format!("{}: {}", i.item, i.message)).collect::<Vec<_>>(), "offline_media": offline, "next": if offline.is_empty() { Value::Null } else { json!("conform action relink with folder: where the camera files are") } }))
            }
            "export" => {
                let path = s(a, "path").ok_or("export needs \"path\" (.edl, .fcpxml, .otio)")?;
                let fmt = s(a, "format").map(str::to_owned).unwrap_or_else(|| fmt_of(path));
                let seq = self.doc.seq();
                let tc = s(a, "start_timecode").unwrap_or("01:00:00:00");
                let rate = self.doc.model.settings.frame_rate;
                let tcf = m::Timecode::for_rate(rate);
                let start_frames = tcf.parse_lenient(tc).ok_or_else(|| format!("\"{tc}\" is not a timecode"))?;
                // As a time of day (HH:MM:SS + frames of label).
                let (h, mi, sc, ff) = tcf.label(start_frames);
                let start = ((h * 3600 + mi * 60 + sc) * m::SECOND) + ff * m::SECOND / tcf.nominal_fps();
                let (out, report) = match fmt.as_str() {
                    "edl" => m::interchange::edl::export(&self.doc.model, seq, m::interchange::edl::EdlOptions { record_start: start, drop_frame: true }),
                    "fcpxml" | "xml" => m::interchange::fcpxml_write::export(&self.doc.model, seq, m::interchange::fcpxml_write::FcpxmlOptions { start_timecode: start }),
                    "otio" => m::interchange::otio::export(&self.doc.model, seq),
                    o => return Err(format!("no exporter for \"{o}\"; edl, fcpxml, otio (AAF is not written)")),
                }
                .map_err(|e| e.to_string())?;
                std::fs::write(path, &out).map_err(|e| format!("{path}: {e}"))?;
                text(json!({ "written": path, "format": fmt, "bytes": out.len(), "not_carried_over": report.issues.iter().map(|i| format!("{}: {}", i.item, i.message)).collect::<Vec<_>>() }))
            }
            "relink" => {
                let folder = s(a, "folder").ok_or("relink needs \"folder\": where to look (searched with its subfolders)")?;
                let want: Vec<m::MediaId> = match a.get("media") {
                    Some(v) => self.media_ids(Some(v))?,
                    None => self.doc.model.media.iter().filter(|i| m::interchange::conform::needs_file(i, &|p| Path::new(p).exists())).map(|i| i.id).collect(),
                };
                if want.is_empty() {
                    return text(json!({ "relinked": [], "note": "no media is offline or missing" }));
                }
                let mut candidates = Vec::new();
                let mut stack = vec![PathBuf::from(folder)];
                let mut seen = 0;
                while let Some(d) = stack.pop() {
                    let Ok(rd) = std::fs::read_dir(&d) else { continue };
                    for e in rd.flatten() {
                        let p = e.path();
                        if p.is_dir() {
                            stack.push(p);
                            continue;
                        }
                        seen += 1;
                        if seen > 20_000 {
                            break;
                        }
                        let Ok(pr) = crate::media_io::probe(&p) else { continue };
                        let mut item = m::MediaItem::new(m::MediaId(0), p.display().to_string(), m::MediaInfo { kind: pr.kind, container: pr.container, duration: pr.duration, video: pr.video.clone(), audio: pr.audio, extra: Default::default() });
                        services::media_meta::enrich(&mut item, &p);
                        candidates.push(m::interchange::conform::Candidate { path: item.path.clone(), reel: item.reel.clone(), timecode: item.timecode.clone(), duration: pr.duration, frame_rate: pr.video.and_then(|v| v.frame_rate) });
                    }
                }
                let by_tc = a.get("by_timecode").and_then(Value::as_bool).unwrap_or(true);
                let (relinks, report) = m::interchange::conform::plan(&self.doc.model, &want, &candidates, by_tc);
                let edit = m::interchange::conform::apply(&self.doc.model, &relinks);
                if !relinks.is_empty() {
                    self.apply_edit("Relink media", edit)?;
                }
                text(json!({
                    "relinked": relinks.iter().map(|r| json!({ "media": r.media.0, "file": r.path, "how": r.how, "timecode": r.timecode, "reel": r.reel })).collect::<Vec<_>>(),
                    "not_found": report.issues.iter().map(|i| format!("{}: {}", i.item, i.message)).collect::<Vec<_>>(),
                    "files_looked_at": candidates.len(),
                }))
            }
            "offline" | "list" => {
                let list: Vec<Value> = self.doc.model.media.iter().filter(|i| m::interchange::conform::needs_file(i, &|p| Path::new(p).exists()) && !i.path.starts_with("sample:")).map(|i| json!({ "id": i.id.0, "name": i.name, "path": i.path, "reel": i.reel, "timecode": i.timecode })).collect();
                text(json!({ "offline_or_missing": list }))
            }
            o => Err(format!("conform has no action \"{o}\"; import, export, relink, offline")),
        }
    }

    // ------------------------------------------------------ media manage

    pub(super) fn media_manage(&mut self, a: &Value) -> Result<Vec<Content>, String> {
        let action = s(a, "action").unwrap_or("info");
        match action {
            "info" => {
                let ids = match a.get("media") {
                    Some(v) => self.media_ids(Some(v))?,
                    None => self.doc.model.media.iter().map(|i| i.id).collect(),
                };
                let list: Vec<Value> = ids
                    .iter()
                    .filter_map(|id| self.doc.model.media_item(*id))
                    .map(|i| {
                        json!({
                            "id": i.id.0, "name": i.name, "codec": i.info.video.as_ref().map(|v| v.codec.clone()), "size": i.info.video.as_ref().map(|v| [v.width, v.height]),
                            "fps": i.info.video.as_ref().and_then(|v| v.frame_rate).map(|r| (r.as_f64() * 1000.0).round() / 1000.0),
                            "timecode": i.timecode, "reel": i.reel, "input_colour": i.input_colour,
                            "proxy": i.proxy_path, "offline": i.path.starts_with("offline:") || !Path::new(&i.path).exists(),
                        })
                    })
                    .collect();
                text(json!({ "media": list, "use_proxies": services::proxies::enabled(&self.doc.model) }))
            }
            "set" => {
                let ids = self.media_ids(a.get("media"))?;
                let mut edits = Vec::new();
                for id in &ids {
                    let mut it = self.doc.model.media_item(*id).ok_or("no media")?.clone();
                    if let Some(tc) = a.get("timecode") {
                        it.timecode = match tc.as_str().filter(|t| !t.is_empty()) {
                            Some(t) => {
                                let rate = it.timecode_rate(self.doc.model.settings.frame_rate);
                                m::Timecode::new(rate, t.contains(';')).parse_lenient(t).ok_or_else(|| format!("\"{t}\" is not a timecode at {:.3} fps", rate.as_f64()))?;
                                Some(t.to_string())
                            }
                            None => None,
                        };
                    }
                    if let Some(r) = a.get("reel") {
                        it.reel = r.as_str().filter(|t| !t.is_empty()).map(str::to_owned);
                    }
                    edits.push(m::Edit::SetMedia { item: Box::new(it) });
                }
                self.apply_edit("Media timecode / reel", m::Edit::Batch { edits })?;
                self.media_manage(&json!({ "action": "info", "media": ids.iter().map(|i| i.0).collect::<Vec<_>>() }))
            }
            "proxies" | "make_proxies" => {
                let ids = match a.get("media") {
                    Some(v) => self.media_ids(Some(v))?,
                    None => self.doc.model.media.iter().filter(|i| i.info.kind == m::MediaKind::Video).map(|i| i.id).collect(),
                };
                let scale = a.get("scale").and_then(Value::as_u64).unwrap_or(2) as u32;
                let (made, failed) = services::proxies::make(&self.doc, &ids, scale, a.get("again").and_then(Value::as_bool) == Some(true), &mut |_, _, _| true);
                if !made.is_empty() {
                    let edit = services::proxies::record(&self.doc.model, &made);
                    self.apply_edit("Make proxies", edit)?;
                }
                if a.get("use").and_then(Value::as_bool) != Some(false) && !made.is_empty() {
                    let e = services::proxies::set_enabled(&self.doc.model, true);
                    self.apply_edit("Use proxies", e)?;
                }
                text(json!({ "made": made.iter().map(|(m, _)| json!({ "media": m.media.0, "file": m.path, "size": [m.width, m.height] })).collect::<Vec<_>>(), "failed": failed, "use_proxies": services::proxies::enabled(&self.doc.model), "codec": "Apple ProRes 422 LT" }))
            }
            "use_proxies" => {
                let on = a.get("on").and_then(Value::as_bool).unwrap_or(true);
                let e = services::proxies::set_enabled(&self.doc.model, on);
                self.apply_edit(if on { "Use proxies" } else { "Use originals" }, e)?;
                text(json!({ "use_proxies": on, "note": "export always reads the original files" }))
            }
            "forget_proxies" => {
                let ids = self.media_ids(a.get("media").or(Some(&json!("all"))))?;
                let e = services::proxies::forget(&self.doc.model, &ids);
                self.apply_edit("Forget proxies", e)?;
                text(json!({ "forgot": ids.len() }))
            }
            "consolidate" | "collect" => {
                let folder = s(a, "folder").ok_or("consolidate needs \"folder\": where to collect the media")?;
                let (edit, r) = services::consolidate::consolidate(&self.doc.model, Path::new(folder), a.get("all").and_then(Value::as_bool) == Some(true))?;
                self.apply_edit("Consolidate media", edit)?;
                text(json!({ "folder": folder, "copied_files": r.copied, "megabytes": (r.bytes as f64 / 1e6).round(), "skipped": r.skipped }))
            }
            "multicam" => {
                let ids = self.media_ids(a.get("media"))?;
                let by = match s(a, "sync").unwrap_or("sound") {
                    "timecode" | "tc" => services::multicam::SyncBy::Timecode,
                    _ => services::multicam::SyncBy::Sound,
                };
                let synced = services::multicam::sync(&self.doc.model, &ids, by)?;
                let starts: Vec<m::Micros> = synced.iter().map(|s| s.0).collect();
                let audio = a.get("audio").and_then(Value::as_u64).map(|n| (n as usize).saturating_sub(1)).unwrap_or(0);
                let name = s(a, "name").unwrap_or("Multicam").to_string();
                let mut p = self.doc.model.clone();
                let lane = self.doc.model.active().spine_id();
                let built = services::multicam::build(&mut p, &name, &ids, &starts, audio, lane)?;
                while self.doc.model.next_id < p.next_id {
                    self.doc.alloc_id();
                }
                let seq = self.doc.seq();
                let at = f(a, "at").map(micros);
                let mut edits = vec![built.edit];
                if a.get("place").and_then(Value::as_bool) != Some(false) {
                    let clip = Box::new(built.clip.clone());
                    edits.push(match at {
                        Some(t) => m::Edit::Overwrite { seq, lane: Some(lane), at: t, clip },
                        None => m::Edit::Append { seq, lane: Some(lane), clip },
                    });
                }
                self.apply_edit("Multicam", m::Edit::Batch { edits })?;
                let placed = self.doc.model.active().clips.iter().find(|c| c.sequence_id() == Some(built.sequence)).map(|c| c.id.0);
                text(json!({
                    "multicam_sequence": built.sequence.0,
                    "clip": placed,
                    "angles": ids.iter().zip(&synced).enumerate().map(|(i, (id, (s, c)))| json!({ "angle": i + 1, "media": id.0, "starts_at": secs(*s), "confidence": (c * 100.0).round() / 100.0 })).collect::<Vec<_>>(),
                    "synced_by": format!("{by:?}").to_lowercase(),
                    "next": "cut to an angle: media_manage {action: angle, clip, at, angle}",
                }))
            }
            "angle" | "switch" => {
                let clip = a.get("clip").and_then(Value::as_u64).ok_or("angle needs \"clip\": the multicam clip")?;
                let n = a.get("angle").and_then(Value::as_u64).ok_or("angle needs \"angle\" (1-based)")? as usize;
                let seq = self.seq_of(clip as u32).ok_or("clip not in a sequence")?;
                let edit = match f(a, "at") {
                    Some(t) => {
                        let q = services::multicam::switch(&self.doc.model, seq, m::ClipId(clip), micros(t), n)?;
                        // Replay the result as edits: the split, then the angle.
                        let mut edits = vec![m::Edit::Split { seq, at: self.doc.model.settings.frame_rate.snap(micros(t)), clips: Some(vec![m::ClipId(clip)]) }];
                        let after = q.sequence(seq).and_then(|s| s.clips.iter().find(|c| c.start == self.doc.model.settings.frame_rate.snap(micros(t)) && c.sequence_id().is_some()).cloned()).ok_or("no clip after the cut")?;
                        edits.push(m::Edit::SetClip { seq, clip: Box::new(after) });
                        m::Edit::Batch { edits }
                    }
                    None => {
                        let c = self.model_clip(clip as u32).ok_or("no clip")?;
                        services::multicam::set_angle(seq, &c, n)?
                    }
                };
                self.apply_edit("Cut to angle", edit)?;
                text(json!({ "angle": n, "clip": clip, "at": f(a, "at") }))
            }
            o => Err(format!("media_manage has no action \"{o}\"; info, set, proxies, use_proxies, forget_proxies, consolidate, multicam, angle")),
        }
    }
}
