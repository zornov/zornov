// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The ops the AI editor sends to a picture.
//!
//! A call is a list of ops, applied in order; when one fails, the ones
//! before it are taken back, so a call either happens or not. Each op is
//! `{"op": name, …}`. Studio's ops (below) speak in document pixels,
//! percentages 0..100 for opacity / flow / hardness / strength, and
//! `#rrggbb` colours; any engine command (`commands` lists all of them
//! with their parameters) runs as `{"op": "<id>", …params}` or
//! `{"cmd": "<id>", …params}`.
//!
//! * `new`, `open`, `picture` (switch), `save`, `export`, `place` (a file
//!   as a smart object that keeps its source), `close`
//! * `select` — `what`: subject, people, sky, background, object (with
//!   `point`, `points`, `exclude`, `box`), or rect / ellipse / lasso /
//!   wand / colour range / all / none / invert; `mode`, `feather`,
//!   `refine` (edge refinement for hair), `to: "mask"` (a layer mask)
//! * `paint` — `tool` (brush, pixel, eraser, mixer, smudge, blur, sharpen,
//!   dodge, burn, sponge, clone, heal, spot_heal, remove, history,
//!   pattern, replace_colour), `feel` or `preset` (pencil, ink, airbrush,
//!   watercolour, oil, gouache, chalk, charcoal, pastel, crayon, …), the
//!   stroke (`points`, `path` with SVG path data, or `shape`), `size`,
//!   `color`, `opacity`, `flow`, `hardness`, `spacing`, `taper`,
//!   `pressure`, `symmetry`, and the tool's own settings
//! * `fill`, `gradient`
//! * `remove` — take something out and fill the hole from its
//!   surroundings (`what` / `point` / `box`, or the selection)
//! * `adjust` — `kind` (levels, curves, hue_saturation, vibrance,
//!   selective_colour, colour_balance, exposure, brightness_contrast,
//!   black_white, photo_filter, channel_mixer, gradient_map, …), as a
//!   non-destructive adjustment layer unless `layer: false`
//! * `layer` — `action`: new, duplicate, delete, group, select, set,
//!   mask, clip, smart, merge, front, back, up, down
//! * `frequency_separation`
//! * `animate`, `frame`, `flipbook` — see [`crate::flipbook`]
//! * `undo`, `redo`, `snapshot`

use serde_json::{Map, Value, json};

use crate::flipbook::Flipbook;
use crate::strokes;
use crate::vision::{self, Target, Vision};
use crate::Canvas;

/// Apply `ops` to the canvas, all or nothing. Returns one result per op.
pub fn apply(c: &mut Canvas, ops: &[Value], v: &mut dyn Vision) -> Result<Value, String> {
    let start_doc = c.engine.active().map(|d| (d.doc.id, d.history.past_len()));
    let open_before = c.engine.documents().len();
    let mut out = Vec::with_capacity(ops.len());
    for (i, op) in ops.iter().enumerate() {
        match one(c, op, v) {
            Ok(r) => out.push(r),
            Err(e) => {
                rollback(c, start_doc, open_before);
                let name = op.get("op").or_else(|| op.get("cmd")).and_then(Value::as_str).unwrap_or("?");
                return Err(if ops.len() > 1 { format!("op {} ({name}): {e}; nothing was changed", i + 1) } else { format!("{name}: {e}") });
            }
        }
    }
    let mut v = json!({"results": out});
    if let Some(st) = c.engine.active() {
        v["picture"] = json!(st.doc.name);
        v["revision"] = json!(st.revision);
    }
    Ok(v)
}

/// Undo what this call did to the picture it started on, and close what it opened.
fn rollback(c: &mut Canvas, start: Option<(studio_pixel_doc::DocId, usize)>, open_before: usize) {
    while c.engine.documents().len() > open_before {
        let last = c.engine.documents().len() - 1;
        c.engine.close(last);
    }
    if let Some((id, past)) = start {
        if let Some(i) = c.engine.documents().iter().position(|d| d.doc.id == id) {
            c.engine.set_active(i);
            let mut guard = 0;
            while c.engine.active().is_some_and(|d| d.history.past_len() > past) && guard < 10_000 {
                if !c.engine.undo() {
                    break;
                }
                guard += 1;
            }
            if let Some(st) = c.engine.active_mut() {
                st.history.clear_redo();
            }
        }
    }
}

fn s<'a>(op: &'a Value, k: &str) -> Option<&'a str> {
    op.get(k).and_then(Value::as_str)
}
fn n(op: &Value, k: &str) -> Option<f64> {
    op.get(k).and_then(Value::as_f64)
}
fn b(op: &Value, k: &str) -> Option<bool> {
    op.get(k).and_then(Value::as_bool)
}

/// Params for an engine command: the op without Studio's own keys.
fn rest(op: &Value, drop: &[&str]) -> Value {
    let mut m = op.as_object().cloned().unwrap_or_default();
    for k in ["op", "cmd"].iter().chain(drop) {
        m.remove(*k);
    }
    Value::Object(m)
}

/// A percentage (0..100) as 0..1, also taking 0..1 when given as a fraction below 1.
fn unit(v: f64) -> f64 {
    if v > 1.0 { (v / 100.0).clamp(0.0, 1.0) } else { v.clamp(0.0, 1.0) }
}

/// One op.
pub fn one(c: &mut Canvas, op: &Value, v: &mut dyn Vision) -> Result<Value, String> {
    let name = s(op, "op").or_else(|| s(op, "cmd")).ok_or("each op needs `op` (a Studio op or any engine command id)")?;
    if op.get("op").is_none() || name.contains('.') {
        // An engine command.
        return c.run(name, rest(op, &[]));
    }
    match name {
        "new" => {
            let mut p = rest(op, &["name"]);
            if p.get("width").is_none() {
                p["width"] = json!(1920);
            }
            if p.get("height").is_none() {
                p["height"] = json!(1080);
            }
            c.new_picture(s(op, "name").unwrap_or("Painting"), p)
        }
        "open" => c.open(std::path::Path::new(s(op, "path").ok_or("`open` needs `path`")?)),
        "picture" | "switch" => {
            c.activate(s(op, "name").or_else(|| s(op, "picture")).ok_or("`picture` needs `name`")?)?;
            c.summary()
        }
        "close" => {
            let i = c.engine.active_index().ok_or("no picture open")?;
            c.engine.close(i);
            Ok(json!({"closed": true, "open": c.names()}))
        }
        "save" => c.save(),
        "export" => {
            let p = s(op, "path").ok_or("`export` needs `path` (its extension picks the format: psd, tif, png, jpg, webp, exr, …)")?;
            c.export(std::path::Path::new(p), s(op, "format"), op)
        }
        "place" => {
            s(op, "path").ok_or("`place` needs `path`")?;
            let mut params = rest(op, &[]);
            if params.get("smartObject").is_none() {
                params["smartObject"] = json!(true);
            }
            c.run("file.placeEmbedded", params)
        }
        "select" => select(c, op, v),
        "paint" | "stroke" => paint(c, op),
        "fill" => {
            let mut p = rest(op, &[]);
            if p.get("contents").is_none() {
                p["contents"] = json!(if p.get("color").is_some() { "color" } else { "foreground" });
            }
            if let Some(o) = n(op, "opacity") {
                p["opacity"] = json!(unit(o) * 100.0);
            }
            c.run("edit.fill", p)
        }
        "gradient" => c.run("paint.gradient", rest(op, &[])),
        "remove" => remove(c, op, v),
        "adjust" => adjust(c, op),
        "layer" => layer(c, op),
        "frequency_separation" => frequency_separation(c, op),
        "animate" => Flipbook::start(c, n(op, "drawings").unwrap_or(12.0) as usize, n(op, "fps").unwrap_or(12.0), n(op, "hold").unwrap_or(2.0) as u32),
        "frame" => frame(c, op),
        "flipbook" => Flipbook::set(c, op),
        "undo" => Ok(json!({"undone": c.engine.undo()})),
        "redo" => Ok(json!({"redone": c.engine.redo()})),
        "snapshot" => c.run("history.newSnapshot", json!({"name": s(op, "name").unwrap_or("Snapshot")})),
        "summary" | "inspect" => c.summary(),
        other => Err(format!(
            "unknown op \"{other}\"; Studio ops: new, open, picture, save, export, place, close, select, paint, fill, gradient, remove, adjust, layer, frequency_separation, animate, frame, flipbook, undo, redo, snapshot; or any engine command id (see `commands`)"
        )),
    }
}

// ---------------------------------------------------------------- select

fn select(c: &mut Canvas, op: &Value, v: &mut dyn Vision) -> Result<Value, String> {
    let what = s(op, "what").unwrap_or(if op.get("point").is_some() || op.get("box").is_some() || op.get("points").is_some() { "object" } else { "subject" });
    let mode = s(op, "mode").unwrap_or("replace").to_owned();
    let feather = n(op, "feather").unwrap_or(0.0) as f32;
    let simple = |c: &mut Canvas, id: &str, p: Value| c.run(id, p);
    let mut r = match what.to_lowercase().as_str() {
        "all" => simple(c, "select.all", json!({}))?,
        "none" | "nothing" | "deselect" => simple(c, "select.deselect", json!({}))?,
        "invert" | "inverse" => simple(c, "select.inverse", json!({}))?,
        "rect" | "rectangle" => {
            let r = op.get("rect").and_then(Value::as_array).ok_or("`rect` is [x, y, width, height]")?;
            let g = |i: usize| r.get(i).and_then(Value::as_f64).unwrap_or(0.0);
            simple(c, "select.rect", json!({"x": g(0), "y": g(1), "width": g(2), "height": g(3), "mode": mode, "feather": feather}))?
        }
        "ellipse" | "oval" => {
            let r = op.get("rect").and_then(Value::as_array).ok_or("`rect` is [x, y, width, height]")?;
            let g = |i: usize| r.get(i).and_then(Value::as_f64).unwrap_or(0.0);
            simple(c, "select.ellipse", json!({"x": g(0), "y": g(1), "width": g(2), "height": g(3), "mode": mode, "feather": feather}))?
        }
        "lasso" | "polygon" => {
            let pts = match (op.get("points"), s(op, "path")) {
                (Some(p), _) => p.clone(),
                (None, Some(d)) => json!(strokes::parse_path(d)?.into_iter().flatten().map(|q| [q.x, q.y]).collect::<Vec<_>>()),
                _ => return Err("a lasso needs `points` or `path`".into()),
            };
            simple(c, "select.lasso", json!({"points": pts, "mode": mode, "feather": feather}))?
        }
        "wand" | "magic_wand" | "colour" | "color" => {
            let p = op.get("point").and_then(Value::as_array).ok_or("the magic wand needs `point` [x, y]")?;
            let mut q = rest(op, &["what", "point", "feather"]);
            q["x"] = p[0].clone();
            q["y"] = p.get(1).cloned().unwrap_or(json!(0));
            simple(c, "select.magicWand", q)?
        }
        "colour_range" | "color_range" | "range" => simple(c, "select.colorRange", rest(op, &["what"]))?,
        "quick" => simple(c, "select.quick", rest(op, &["what"]))?,
        other => {
            let target = Target::parse(other)?;
            ai_select(c, op, v, target, &mode, feather)?
        }
    };
    if let Some(rf) = op.get("refine") {
        let p = match rf {
            Value::Bool(true) => json!({"radius": 6, "smartRadius": true, "decontaminate": false}),
            Value::Object(_) => rf.clone(),
            _ => Value::Null,
        };
        if !p.is_null() {
            r["refined"] = c.run("select.refineEdge", p)?;
        }
    }
    if s(op, "to") == Some("mask") {
        r["mask"] = c.run("layer.layerMask.revealSelection", json!({}))?;
    }
    Ok(r)
}

fn ai_select(c: &mut Canvas, op: &Value, v: &mut dyn Vision, target: Target, mode: &str, feather: f32) -> Result<Value, String> {
    let forced = b(op, "model") == Some(true);
    // Subject, sky and background: the engine's segmentation first (it
    // finds every main object and keeps distant hills out of the sky);
    // the models when it finds nothing, or with `model: true`.
    if !forced && matches!(target, Target::Subject | Target::Sky | Target::Background) {
        let id = if target == Target::Sky { "select.sky" } else { "select.subject" };
        let engine_mode = if target == Target::Background { "replace" } else { mode };
        if let Ok(mut r) = c.run(id, json!({"mode": engine_mode, "sampleAllLayers": true})) {
            let found = c.doc().and_then(|d| d.selection.as_ref()).is_some_and(|s| !s.content_bounds().is_empty());
            let enough = r.get("coverage").and_then(Value::as_f64).is_none_or(|cv| cv > 0.002);
            if found && enough {
                if target == Target::Background {
                    c.run("select.inverse", json!({}))?;
                }
                if feather > 0.0 {
                    c.run("select.modify.feather", json!({"radius": feather}))?;
                }
                r["by"] = json!("analysis");
                return Ok(r);
            }
        }
    }
    let img = vision::work_image(c)?;
    let doc_w = c.doc().map(|d| d.size.width).ok_or("no picture open")?;
    let prompts = vision::scale_prompts(&vision::prompts_from(op)?, doc_w, img.width);
    let label = match target {
        Target::Subject => "Select Subject",
        Target::People => "Select People",
        Target::Sky => "Select Sky",
        Target::Background => "Select Background",
        Target::Object => "Object Selection",
    };
    match vision::mask_for(v, &img, target, &prompts) {
        Ok(mask) => {
            let mut r = vision::select_mask(c, &mask, img.width, img.height, mode, feather, label)?;
            r["by"] = json!("model");
            Ok(r)
        }
        // Without the models, the engine's own analysis.
        Err(e) if !forced => {
            let fallback = match target {
                Target::Object => op
                    .get("box")
                    .and_then(Value::as_array)
                    .map(|bx| {
                        let g = |i: usize| bx.get(i).and_then(Value::as_f64).unwrap_or(0.0);
                        ("select.object", json!({"rect": [g(0), g(1), g(2) - g(0), g(3) - g(1)], "mode": mode}))
                    })
                    .or_else(|| op.get("point").map(|p| ("select.object", json!({"point": p, "mode": mode})))),
                _ => Some(("select.subject", json!({"mode": mode}))),
            };
            let (id, p) = fallback.ok_or(e.clone())?;
            let mut r = c.run(id, p)?;
            if feather > 0.0 {
                c.run("select.modify.feather", json!({"radius": feather}))?;
            }
            r["by"] = json!("analysis");
            r["note"] = json!(e);
            Ok(r)
        }
        Err(e) => Err(e),
    }
}

/// Make the topmost pixel layer the active one when the active layer has
/// no pixels (an adjustment, fill, text or group layer), so retouching
/// lands on the photograph under the adjustments.
fn to_pixels(c: &mut Canvas) -> Result<Option<u64>, String> {
    fn top_raster(ls: &[studio_pixel_doc::Layer]) -> Option<u64> {
        for l in ls.iter().rev() {
            if let Some(k) = l.children() {
                if let Some(id) = top_raster(k) {
                    return Some(id);
                }
            } else if matches!(l.content, studio_pixel_doc::LayerContent::Raster(_)) && l.visible {
                return Some(l.id.0);
            }
        }
        None
    }
    let st = c.engine.active().ok_or("no picture open")?;
    let active_is_pixels = st.active_layer.and_then(|id| st.doc.layer(id)).is_some_and(|l| matches!(l.content, studio_pixel_doc::LayerContent::Raster(_)));
    if active_is_pixels {
        return Ok(None);
    }
    let Some(id) = top_raster(&st.doc.layers) else { return Ok(None) };
    c.run("layer.select", json!({"layer": id}))?;
    Ok(Some(id))
}

// ---------------------------------------------------------------- paint

/// Brush presets for the feel of a medium.
pub const FEELS: &[(&str, &str)] = &[
    ("pencil", "Soft Pencil"),
    ("hard_pencil", "Hard Pencil"),
    ("graphite", "Soft Pencil"),
    ("ink", "Ink Pen"),
    ("pen", "Ink Pen"),
    ("calligraphy", "Calligraphy Flat"),
    ("airbrush", "Airbrush Soft"),
    ("spray", "Airbrush Pressure Flow"),
    ("watercolour", "Watercolor Wet Edges"),
    ("watercolor", "Watercolor Wet Edges"),
    ("wash", "Watercolor Wash"),
    ("ink_wash", "Ink Wash Sponge"),
    ("oil", "Oil Bristle"),
    ("gouache", "Gouache Flat"),
    ("acrylic", "Gouache Flat"),
    ("dry_brush", "Dry Bristle"),
    ("chalk", "Chalk"),
    ("charcoal", "Charcoal"),
    ("pastel", "Pastel"),
    ("crayon", "Wax Crayon"),
    ("marker", "Hard Round Pressure Opacity"),
    ("soft", "Soft Round Pressure Size"),
    ("hard", "Hard Round Pressure Size"),
    ("spatter", "Spatter"),
    ("stipple", "Stipple"),
    ("grass", "Grass"),
    ("foliage", "Foliage"),
    ("sparkle", "Sparkle"),
    ("texture", "Canvas Texture"),
    ("sponge", "Sponge Texture"),
    ("rake", "Rake"),
];

fn tool_id(tool: &str) -> Result<&'static str, String> {
    Ok(match tool.trim().to_lowercase().replace([' ', '-'], "_").as_str() {
        "brush" | "paint" | "eraser" | "erase" => "paint.stroke",
        "pixel" | "pencil_tool" | "aliased" => "paint.pencil",
        "mixer" | "wet" | "mix" => "paint.mixerBrush",
        "smudge" | "finger" => "paint.smudge",
        "blur" => "paint.blur",
        "sharpen" => "paint.sharpen",
        "dodge" | "lighten" => "paint.dodge",
        "burn" | "darken" => "paint.burn",
        "sponge" | "saturate" | "desaturate" => "paint.sponge",
        "clone" | "clone_stamp" | "stamp" => "paint.cloneStamp",
        "heal" | "healing" | "healing_brush" => "paint.healingBrush",
        "spot_heal" | "spot" | "spot_healing" | "blemish" => "paint.spotHealing",
        "remove" | "remove_tool" => "paint.remove",
        "history" | "history_brush" | "restore" => "paint.historyBrush",
        "pattern" | "pattern_stamp" => "paint.patternStamp",
        "replace_colour" | "replace_color" | "colour_replacement" | "color_replacement" => "paint.colorReplacement",
        "red_eye" => "paint.redEye",
        other => return Err(format!("unknown paint tool \"{other}\": brush, pixel, eraser, mixer, smudge, blur, sharpen, dodge, burn, sponge, clone, heal, spot_heal, remove, history, pattern, replace_colour")),
    })
}

fn paint(c: &mut Canvas, op: &Value) -> Result<Value, String> {
    let tool = s(op, "tool").unwrap_or("brush");
    let id = tool_id(tool)?;
    let erase = matches!(tool, "eraser" | "erase") || b(op, "erase") == Some(true);
    let size = n(op, "size").unwrap_or(24.0).clamp(0.5, 5000.0);
    // Dabs closer than a quarter of the size are invisible as dabs.
    let subpaths = strokes::sample(op, (size * 0.12).clamp(0.5, 8.0))?;
    if let Some(sym) = op.get("symmetry") {
        match sym {
            Value::Bool(false) | Value::Null => {
                c.run("paint.symmetryDisable", json!({}))?;
            }
            Value::String(m) => {
                c.run("paint.setSymmetry", json!({"mode": m}))?;
            }
            other => {
                c.run("paint.setSymmetry", other.clone())?;
            }
        }
    }
    let mut p: Map<String, Value> = rest(op, &["tool", "feel", "points", "path", "shape", "taper", "pressure", "symmetry", "erase"]).as_object().cloned().unwrap_or_default();
    p.insert("size".into(), json!(size));
    let is_brush = id == "paint.stroke" || id == "paint.pencil";
    // Units: brushes take 0..1, the retouch tools 1..100.
    for k in ["opacity", "flow", "hardness"] {
        if let Some(x) = n(op, k) {
            p.insert(k.into(), json!(if is_brush { unit(x) } else { unit(x) * 100.0 }));
        }
    }
    if let Some(feel) = s(op, "feel") {
        let key = feel.trim().to_lowercase().replace([' ', '-'], "_");
        let preset = FEELS.iter().find(|(k, _)| *k == key).map(|(_, v)| *v).ok_or_else(|| format!("unknown feel \"{feel}\": {}", FEELS.iter().map(|(k, _)| *k).collect::<Vec<_>>().join(", ")))?;
        p.insert("preset".into(), json!(preset));
    }
    if erase {
        p.insert("erase".into(), json!(true));
    }
    // A stroke with varying pressure (a taper, pressure ramps, a tablet)
    // shapes size and opacity, as a pen does.
    let varies = subpaths.iter().any(|sp| sp.iter().any(|q| (q.p - sp[0].p).abs() > 0.02));
    if is_brush && varies && p.get("brush").is_none() {
        p.insert("brush".into(), json!({"pressureSize": true}));
    }
    // Strokes need pixels: with an adjustment, fill, text or group layer
    // active (and no mask asked for), they go on a new layer above it.
    let mut new_layer = None;
    if op.get("target").is_none() && op.get("layer").is_none() {
        let not_pixels = c.engine.active().and_then(|d| d.active_layer.and_then(|id| d.doc.layer(id))).is_some_and(|l| !matches!(l.content, studio_pixel_doc::LayerContent::Raster(_)));
        if not_pixels {
            new_layer = Some(c.run("layer.new.layer", json!({"name": if is_brush { "Paint" } else { "Retouch" }}))?);
            // Retouching on an empty layer samples what is under it.
            match id {
                "paint.cloneStamp" | "paint.healingBrush" => {
                    p.entry("sampleLayer").or_insert(json!("all"));
                }
                "paint.spotHealing" | "paint.remove" | "paint.smudge" | "paint.mixerBrush" => {
                    p.entry("sampleAllLayers").or_insert(json!(true));
                }
                _ => {}
            }
        }
    }
    let mut results = Vec::new();
    for sp in &subpaths {
        let pts: Vec<Value> = sp.iter().map(|q| json!([round2(q.x), round2(q.y), round2(q.p)])).collect();
        let mut q = p.clone();
        q.insert("points".into(), Value::Array(pts));
        let r = c.run(id, Value::Object(q))?;
        results.push(r);
    }
    let mut v = json!({"tool": id, "strokes": subpaths.len(), "points": subpaths.iter().map(Vec::len).sum::<usize>(), "results": results});
    if let Some(l) = new_layer {
        v["new_layer"] = l;
        v["note"] = json!("the active layer had no pixels to paint, so the strokes went on a new layer \"Paint\" above it");
    }
    Ok(v)
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

// ---------------------------------------------------------------- remove

fn remove(c: &mut Canvas, op: &Value, v: &mut dyn Vision) -> Result<Value, String> {
    let has_target = op.get("what").is_some() || op.get("point").is_some() || op.get("box").is_some() || op.get("points").is_some();
    let picked = if has_target {
        let mut sel = op.clone();
        sel["op"] = json!("select");
        sel["mode"] = json!("replace");
        if let Some(o) = sel.as_object_mut() {
            o.remove("expand");
        }
        Some(select(c, &sel, v)?)
    } else if c.doc().is_some_and(|d| d.selection.is_none()) {
        return Err("say what to remove (`what`: people / subject / sky, or `point` / `box` on it), or select it first".into());
    } else {
        None
    };
    // A little past the edge so no outline of it stays.
    let grow = n(op, "expand").unwrap_or(6.0);
    if grow > 0.0 {
        c.run("select.modify.expand", json!({"radius": grow.round().clamp(1.0, 500.0)}))?;
    }
    let switched = to_pixels(c)?;
    let fill = c.run("edit.contentAwareFill", json!({"sampling": "auto", "colorAdaptation": "high", "output": if b(op, "new_layer") == Some(true) { "new" } else { "current" }}))?;
    if b(op, "keep_selection") != Some(true) {
        c.run("select.deselect", json!({}))?;
    }
    let mut r = json!({"selected": picked, "filled": fill});
    if let Some(id) = switched {
        r["layer"] = json!(id);
        r["note"] = json!("filled on the topmost pixel layer, under the adjustments");
    }
    Ok(r)
}

// ---------------------------------------------------------------- adjust

fn adjustment_id(kind: &str) -> Result<&'static str, String> {
    Ok(match kind.trim().to_lowercase().replace([' ', '-', '/'], "_").as_str() {
        "levels" => "levels",
        "curves" => "curves",
        "exposure" => "exposure",
        "vibrance" => "vibrance",
        "hue_saturation" | "hue" | "saturation" | "huesaturation" => "hueSaturation",
        "colour_balance" | "color_balance" => "colorBalance",
        "brightness_contrast" | "brightness" | "contrast" => "brightnessContrast",
        "black_white" | "black_and_white" | "mono" => "blackWhite",
        "photo_filter" | "warm" | "cool" => "photoFilter",
        "channel_mixer" => "channelMixer",
        "invert" => "invert",
        "posterize" => "posterize",
        "threshold" => "threshold",
        "gradient_map" => "gradientMap",
        "selective_colour" | "selective_color" => "selectiveColor",
        "colour_lookup" | "color_lookup" | "lut" => "colorLookup",
        other => return Err(format!("unknown adjustment \"{other}\": levels, curves, exposure, vibrance, hue_saturation, colour_balance, brightness_contrast, black_white, photo_filter, channel_mixer, invert, posterize, threshold, gradient_map, selective_colour, colour_lookup")),
    })
}

fn adjust(c: &mut Canvas, op: &Value) -> Result<Value, String> {
    let kind = s(op, "kind").ok_or("`adjust` needs `kind`")?;
    let params = rest(op, &["kind", "layer", "name", "opacity", "blend"]);
    // Destructive kinds the engine has only as a filter on pixels.
    match kind.trim().to_lowercase().replace([' ', '-', '/'], "_").as_str() {
        "shadows_highlights" => return c.run("image.adjustments.shadowsHighlights", params),
        "camera_raw" | "develop" | "raw" => return c.run("filter.cameraRaw", params),
        "hdr_toning" => return c.run("image.adjustments.hdrToning", params),
        "match_colour" | "match_color" => return c.run("image.adjustments.matchColor", params),
        "replace_colour" | "replace_color" => return c.run("image.adjustments.replaceColor", params),
        _ => {}
    }
    let id = adjustment_id(kind)?;
    if b(op, "layer") == Some(false) {
        return c.run(&format!("image.adjustments.{id}"), params);
    }
    let made = c.run(&format!("layer.newAdjustmentLayer.{id}"), json!({}))?;
    let set = if params.as_object().is_some_and(|m| !m.is_empty()) { Some(c.run("layer.setAdjustment", params)?) } else { None };
    let mut props = json!({});
    if let Some(name) = s(op, "name") {
        props["name"] = json!(name);
    }
    if let Some(o) = n(op, "opacity") {
        props["opacity"] = json!(unit(o));
    }
    if let Some(bl) = s(op, "blend") {
        props["blend"] = json!(bl);
    }
    if props.as_object().is_some_and(|m| !m.is_empty()) {
        c.run("layer.setProps", props)?;
    }
    Ok(json!({"layer": made, "set": set}))
}

// ---------------------------------------------------------------- layers

fn layer(c: &mut Canvas, op: &Value) -> Result<Value, String> {
    let action = s(op, "action").unwrap_or("new");
    let p = rest(op, &["action"]);
    let with_layer = |c: &mut Canvas| -> Result<(), String> {
        if let Some(l) = op.get("layer") {
            let sel = if l.is_u64() { json!({"layer": l}) } else { json!({"name": l}) };
            c.run("layer.select", sel)?;
        }
        Ok(())
    };
    match action {
        "new" | "add" => {
            let mut q = p.clone();
            if let Some(o) = n(op, "opacity") {
                q["opacity"] = json!(unit(o));
            }
            if let Some(bl) = s(op, "blend") {
                q["mode"] = json!(bl);
                if let Some(m) = q.as_object_mut() {
                    m.remove("blend");
                }
            }
            c.run("layer.new.layer", q)
        }
        "group" => c.run("layer.new.groupFromLayers", p),
        "select" => {
            let l = op.get("layer").or_else(|| op.get("name")).ok_or("`select` needs `layer` (id or name)")?;
            c.run("layer.select", if l.is_u64() { json!({"layer": l}) } else { json!({"name": l}) })
        }
        "duplicate" => {
            with_layer(c)?;
            c.run("layer.duplicate", json!({"name": op.get("name")}).as_object().filter(|_| op.get("name").is_some()).map(|m| Value::Object(m.clone())).unwrap_or(json!({})))
        }
        "delete" => {
            with_layer(c)?;
            c.run("layer.delete", json!({}))
        }
        "set" => {
            with_layer(c)?;
            let mut q = rest(op, &["action", "layer"]);
            if let Some(o) = n(op, "opacity") {
                q["opacity"] = json!(unit(o));
            }
            if let Some(o) = n(op, "fill") {
                q["fill"] = json!(unit(o));
            }
            c.run("layer.setProps", q)
        }
        "mask" => {
            with_layer(c)?;
            let id = match s(op, "from").unwrap_or("selection") {
                "selection" => "layer.layerMask.revealSelection",
                "hide_selection" => "layer.layerMask.hideSelection",
                "all" | "reveal" => "layer.layerMask.revealAll",
                "none" | "hide" => "layer.layerMask.hideAll",
                "transparency" => "layer.layerMask.fromTransparency",
                "delete" => "layer.layerMask.delete",
                "apply" => "layer.layerMask.apply",
                other => return Err(format!("unknown mask source \"{other}\": selection, hide_selection, all, none, transparency, delete, apply")),
            };
            c.run(id, json!({}))
        }
        "clip" => {
            with_layer(c)?;
            c.run("layer.createClippingMask", json!({}))
        }
        "smart" => {
            with_layer(c)?;
            c.run("layer.smartObjects.convertToSmartObject", json!({}))
        }
        "merge" => c.run(if b(op, "visible") == Some(true) { "layer.mergeVisible" } else { "layer.mergeDown" }, json!({})),
        "front" | "back" | "up" | "down" => {
            with_layer(c)?;
            c.run(
                match action {
                    "front" => "layer.arrange.bringToFront",
                    "back" => "layer.arrange.sendToBack",
                    "up" => "layer.arrange.bringForward",
                    _ => "layer.arrange.sendBackward",
                },
                json!({}),
            )
        }
        other => Err(format!("unknown layer action \"{other}\": new, group, select, duplicate, delete, set, mask, clip, smart, merge, front, back, up, down")),
    }
}

/// Split the active layer into a smooth colour layer (Low) and a detail
/// layer (High, Linear Light over it) that add back to the original, so
/// skin tone and texture retouch apart.
fn frequency_separation(c: &mut Canvas, op: &Value) -> Result<Value, String> {
    let radius = n(op, "radius").unwrap_or(6.0).clamp(0.5, 200.0);
    let src = c.engine.active().and_then(|d| d.active_layer).ok_or("select the layer to split")?;
    c.run("layer.duplicate", json!({"name": "Low frequency"}))?;
    let low = c.engine.active().and_then(|d| d.active_layer).ok_or("no layer")?;
    c.run("filter.blur.gaussianBlur", json!({"radius": radius}))?;
    c.run("layer.select", json!({"layer": src.0}))?;
    c.run("layer.duplicate", json!({"name": "High frequency"}))?;
    c.run("layer.arrange.bringForward", json!({}))?;
    let depth16 = c.doc().is_some_and(|d| !matches!(d.depth, studio_pixel_color::SampleType::U8));
    // 8-bit: subtract at half scale with a mid-grey offset; deeper: add the inverted low at half.
    let apply = if depth16 {
        json!({"source": {"layer": low.0, "channel": "composite", "invert": true}, "blending": "add", "opacity": 100, "scale": 2, "offset": 0})
    } else {
        json!({"source": {"layer": low.0, "channel": "composite"}, "blending": "subtract", "opacity": 100, "scale": 2, "offset": 128})
    };
    c.run("image.applyImage", apply)?;
    c.run("layer.setProps", json!({"blend": "LinearLight"}))?;
    Ok(json!({"low": low.0, "high": c.engine.active().and_then(|d| d.active_layer).map(|l| l.0), "radius": radius, "note": "paint on Low frequency to even tone, heal or clone on High frequency for texture"}))
}

// ---------------------------------------------------------------- frames

fn frame(c: &mut Canvas, op: &Value) -> Result<Value, String> {
    let idx = |k: &str| n(op, k).map(|x| (x.max(1.0) as usize) - 1);
    if let Some(i) = idx("go").or_else(|| idx("show")) {
        return Flipbook::show(c, i);
    }
    match s(op, "action").unwrap_or("next") {
        "next" | "prev" | "previous" => {
            let fb = c.meta().and_then(|m| m.flipbook.clone()).ok_or("this picture is not an animation; start one with {\"op\": \"animate\"}")?;
            let n = fb.holds.len().max(1);
            let i = if s(op, "action") == Some("next") || s(op, "action").is_none() { (fb.current + 1) % n } else { (fb.current + n - 1) % n };
            Flipbook::show(c, i)
        }
        "add" | "new" => Flipbook::add(c, idx("after"), false, n(op, "hold").map(|h| h as u32)),
        "copy" | "duplicate" => Flipbook::add(c, idx("after"), true, n(op, "hold").map(|h| h as u32)),
        "delete" | "remove" => {
            let cur = c.meta().and_then(|m| m.flipbook.as_ref()).map(|f| f.current).unwrap_or(0);
            Flipbook::delete(c, idx("drawing").unwrap_or(cur))
        }
        "hold" | "timing" => Flipbook::set_hold(c, if b(op, "all") == Some(true) { None } else { idx("drawing").or_else(|| c.meta().and_then(|m| m.flipbook.as_ref()).map(|f| f.current)) }, n(op, "hold").ok_or("`hold` frames needed")? as u32),
        other => Err(format!("unknown frame action \"{other}\": next, prev, add, copy, delete, hold (or `go`: drawing number)")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vision::NoVision;

    fn canvas() -> Canvas {
        let mut c = Canvas::new(None);
        apply(&mut c, &[json!({"op": "new", "name": "Test", "width": 64, "height": 48, "background": "white"})], &mut NoVision).unwrap();
        c
    }

    fn px(c: &Canvas, x: u32, y: u32) -> [u8; 4] {
        let img = c.render(0).unwrap();
        let i = ((y * img.width + x) * 4) as usize;
        [img.pixels[i], img.pixels[i + 1], img.pixels[i + 2], img.pixels[i + 3]]
    }

    #[test]
    fn a_painted_path_lands_on_the_picture_and_undo_takes_it_back() {
        let mut c = canvas();
        apply(&mut c, &[json!({"op": "layer", "action": "new", "name": "Ink"}), json!({"op": "paint", "path": "M8 24 L56 24", "size": 6, "color": "#ff0000", "hardness": 100})], &mut NoVision).unwrap();
        let p = px(&c, 32, 24);
        assert!(p[0] > 200 && p[1] < 60, "red under the stroke: {p:?}");
        assert_eq!(px(&c, 32, 4), [255, 255, 255, 255], "white away from it");
        c.engine.undo();
        assert!(px(&c, 32, 24)[1] > 200, "undo took the stroke back");
    }

    #[test]
    fn a_tapered_stroke_is_thin_at_its_ends() {
        let mut c = canvas();
        apply(&mut c, &[json!({"op": "paint", "shape": {"kind": "line", "from": [4, 24], "to": [60, 24]}, "size": 14, "color": "#000000", "hardness": 100, "taper": 0.45})], &mut NoVision).unwrap();
        let dark = |x: u32| (0..48).filter(|&y| px(&c, x, y)[0] < 128).count();
        assert!(dark(32) > dark(8) + 3, "middle {} vs end {}", dark(32), dark(8));
    }

    #[test]
    fn painting_over_an_adjustment_layer_goes_on_a_new_layer() {
        let mut c = canvas();
        let r = apply(&mut c, &[json!({"op": "adjust", "kind": "vibrance", "vibrance": 20}), json!({"op": "paint", "points": [[5, 5], [40, 30]], "size": 4, "color": "#000000"})], &mut NoVision).unwrap();
        assert!(r["results"][1]["new_layer"].is_object() || !r["results"][1]["new_layer"].is_null(), "{r}");
        let sum = c.summary().unwrap();
        assert_eq!(sum["layers"][0]["name"], "Paint", "{sum}");
    }

    #[test]
    fn a_failing_op_takes_back_the_ones_before_it() {
        let mut c = canvas();
        let before = c.engine.active().unwrap().history.past_len();
        let e = apply(&mut c, &[json!({"op": "fill", "color": "#000000"}), json!({"op": "paint", "tool": "nonsense", "points": [[1, 1]]})], &mut NoVision).unwrap_err();
        assert!(e.contains("op 2") && e.contains("nothing was changed"), "{e}");
        assert_eq!(c.engine.active().unwrap().history.past_len(), before);
        assert_eq!(px(&c, 10, 10), [255, 255, 255, 255]);
    }

    #[test]
    fn engine_commands_pass_through_and_adjustments_make_layers() {
        let mut c = canvas();
        apply(&mut c, &[json!({"op": "fill", "color": "#808080"}), json!({"op": "adjust", "kind": "levels", "input": [0, 128, 128]}), json!({"op": "filter.blur.gaussianBlur", "radius": 2})], &mut NoVision).ok();
        let r = apply(&mut c, &[json!({"op": "adjust", "kind": "hue_saturation"})], &mut NoVision).unwrap();
        assert!(r["results"][0]["layer"].is_object() || r["results"][0]["layer"].is_number() || !r["results"][0]["layer"].is_null(), "{r}");
        let sum = c.summary().unwrap();
        assert!(sum["layers"].as_array().unwrap().iter().any(|l| l["kind"].as_str().unwrap_or("").to_lowercase().contains("adjust")), "{sum}");
    }

    #[test]
    fn remove_fills_a_selected_spot_from_its_surroundings() {
        let mut c = canvas();
        apply(&mut c, &[json!({"op": "fill", "color": "#3366cc"}), json!({"op": "select", "what": "rect", "rect": [28, 20, 8, 8]}), json!({"op": "fill", "color": "#ffff00"})], &mut NoVision).unwrap();
        assert!(px(&c, 32, 24)[0] > 200, "a yellow spot");
        apply(&mut c, &[json!({"op": "select", "what": "none"})], &mut NoVision).unwrap();
        assert!(apply(&mut c, &[json!({"op": "remove"})], &mut NoVision).unwrap_err().contains("say what"));
        apply(&mut c, &[json!({"op": "select", "what": "rect", "rect": [28, 20, 8, 8]}), json!({"op": "remove", "expand": 2})], &mut NoVision).unwrap();
        let p = px(&c, 32, 24);
        assert!(p[2] > 150 && p[0] < 120, "filled with the surrounding blue: {p:?}");
    }

    #[test]
    fn a_flipbook_has_drawings_timing_and_frames_to_render() {
        let mut c = canvas();
        let r = apply(&mut c, &[json!({"op": "animate", "drawings": 3, "fps": 12, "hold": 2})], &mut NoVision).unwrap();
        assert_eq!(r["results"][0]["drawings"], 3, "{r}");
        apply(&mut c, &[json!({"op": "paint", "points": [[10, 10], [20, 10]], "size": 4, "color": "#000000"}), json!({"op": "frame", "action": "copy"}), json!({"op": "frame", "action": "hold", "hold": 4})], &mut NoVision).unwrap();
        let fb = c.meta().unwrap().flipbook.clone().unwrap();
        assert_eq!(fb.holds, vec![2, 4, 2, 2]);
        assert_eq!(fb.current, 1);
        let t = fb.timing();
        assert!((t[1].0 - 2.0 / 12.0).abs() < 1e-9 && (t[1].1 - 4.0 / 12.0).abs() < 1e-9);
        let dir = std::env::temp_dir().join(format!("studio-canvas-frames-{}", std::process::id()));
        let files = fb.render_frames(&c, &dir).unwrap();
        assert_eq!(files.len(), 4);
        let _ = std::fs::remove_dir_all(&dir);
        let view = fb.onion_view(c.doc().unwrap(), 64);
        assert_eq!(view.width, 64);
    }

    #[test]
    fn frequency_separation_adds_back_to_the_original() {
        let mut c = canvas();
        apply(&mut c, &[json!({"op": "layer", "action": "new", "name": "Skin"}), json!({"op": "fill", "color": "#c08060"}), json!({"op": "paint", "points": [[10, 10], [50, 40]], "size": 3, "color": "#402010"})], &mut NoVision).unwrap();
        let before = c.render(0).unwrap();
        apply(&mut c, &[json!({"op": "frequency_separation", "radius": 4})], &mut NoVision).unwrap();
        let after = c.render(0).unwrap();
        let worst = before.pixels.iter().zip(&after.pixels).map(|(a, b)| a.abs_diff(*b)).max().unwrap();
        assert!(worst <= 6, "low + high reproduce the layer within rounding (worst {worst})");
    }
}
