// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! 3D models for the agent: `model` adds one (a shape to sculpt from, or a
//! glTF/OBJ/STL file) and `mesh` works on it (subdivide, sculpt, smooth,
//! extrude, booleans, materials, textures). Every change writes a new
//! model file named by its content into the project's `Models` folder and
//! points the clip at it, so undo goes back to the file before.

use super::*;
use studio_mesh::region::Region;
use studio_mesh::{Mesh, V3, ops};

/// Most triangles a sculpted model keeps (more is slow to draw and save).
const MAX_TRIANGLES: usize = 1_500_000;

pub(super) fn v3(v: &Value) -> Option<V3> {
    let a = v.as_array()?;
    if a.len() != 3 {
        return None;
    }
    Some([a[0].as_f64()? as f32, a[1].as_f64()? as f32, a[2].as_f64()? as f32])
}

pub(super) fn field_v3(op: &Value, k: &str) -> Result<Option<V3>, String> {
    match op.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v3(v).map(Some).ok_or_else(|| format!("\"{k}\" is [x, y, z] in the model's pixels")),
    }
}

/// A number field, read as `f` in the agent reads them (a quoted number,
/// or a switch for a strength: `fire: true` is 1).
pub(super) fn num(op: &Value, k: &str) -> Option<f32> {
    super::f(op, k).map(|v| v as f32)
}

/// A colour in words ("#rrggbb" or a basic name) as linear RGBA.
pub(super) fn linear_colour(v: &Value) -> Result<[f32; 4], String> {
    if let Some(a) = v.as_array() {
        let c: Vec<f32> = a.iter().filter_map(Value::as_f64).map(|x| x as f32).collect();
        if c.len() >= 3 {
            return Ok([c[0], c[1], c[2], c.get(3).copied().unwrap_or(1.0)]);
        }
    }
    let s = v.as_str().ok_or("a colour is \"#rrggbb\", a basic name, or [r, g, b] in linear light")?;
    let k = super::craft::colour(s).ok_or_else(|| format!("colour \"{s}\" is not #rrggbb or a basic name"))?;
    Ok([studio_mesh::srgb_to_linear(k.r()), studio_mesh::srgb_to_linear(k.g()), studio_mesh::srgb_to_linear(k.b()), k.a() as f32 / 255.0])
}

/// A region in JSON: "all", "top", "front"…, or {"sphere": {"at", "radius"}},
/// {"box": {"min", "max"}}, {"side": {"axis", "above"|"below"}},
/// {"facing": {"direction", "degrees"}}, {"material": n}.
pub fn region(v: Option<&Value>) -> Result<Region, String> {
    let Some(v) = v else { return Ok(Region::All) };
    if let Some(s) = v.as_str() {
        return Region::named(s).ok_or_else(|| format!("region \"{s}\": all, top, bottom, left, right, front or back, or an object (sphere, box, side, facing, material)"));
    }
    let bad = || "region is a word (all, top, front…) or one of {\"sphere\": {\"at\": [x,y,z], \"radius\": r}}, {\"box\": {\"min\": [..], \"max\": [..]}}, {\"side\": {\"axis\": \"x\", \"above\": 0}}, {\"facing\": {\"direction\": [x,y,z], \"degrees\": 45}}, {\"material\": n}".to_string();
    let o = v.as_object().ok_or_else(bad)?;
    if let Some(s) = o.get("sphere") {
        return Ok(Region::Sphere { centre: field_v3(s, "at")?.ok_or_else(bad)?, radius: num(s, "radius").ok_or_else(bad)? });
    }
    if let Some(b) = o.get("box") {
        return Ok(Region::Box { min: field_v3(b, "min")?.ok_or_else(bad)?, max: field_v3(b, "max")?.ok_or_else(bad)? });
    }
    if let Some(s) = o.get("side") {
        let axis = match s.get("axis").and_then(Value::as_str).unwrap_or("x") {
            "x" => 0,
            "y" => 1,
            "z" => 2,
            _ => return Err(bad()),
        };
        let (at, above) = match (num(s, "above"), num(s, "below")) {
            (Some(a), _) => (a, true),
            (None, Some(b)) => (b, false),
            _ => (0.0, true),
        };
        return Ok(Region::Side { axis, at, above });
    }
    if let Some(f) = o.get("facing") {
        return Ok(Region::Facing { direction: field_v3(f, "direction")?.ok_or_else(bad)?, degrees: num(f, "degrees").unwrap_or(45.0) });
    }
    if let Some(m) = o.get("material").and_then(Value::as_u64) {
        return Ok(Region::Material { index: m as u32 });
    }
    Err(bad())
}

/// A primitive from {"shape", "size", "detail"}; size is one number (the
/// largest side) or [x, y, z] to stretch it.
fn primitive(op: &Value, default_size: f32) -> Result<Mesh, String> {
    let shape = op.get("shape").and_then(Value::as_str).unwrap_or("sphere");
    let size = op.get("size");
    let (s, stretch) = match size {
        Some(v) if v.is_array() => {
            let d = v3(v).ok_or("\"size\" is a number or [x, y, z]")?;
            let big = d[0].max(d[1]).max(d[2]).max(1.0);
            (big, Some([d[0] / big, d[1] / big, d[2] / big]))
        }
        Some(v) => (v.as_f64().ok_or("\"size\" is a number or [x, y, z]")? as f32, None),
        None => (default_size, None),
    };
    let detail = op.get("detail").and_then(Value::as_u64).unwrap_or(4).min(7) as usize;
    if is_character(shape) {
        return character(op, s, detail);
    }
    if matches!(shape, "head" | "human_head" | "face" | "bust") {
        return Ok(studio_mesh::sdf::head(s.max(1.0), detail.max(1)));
    }
    let mut m = studio_mesh::prim::by_name(shape, s.max(1.0), detail).ok_or_else(|| format!("shape \"{shape}\": {}, head", studio_mesh::prim::NAMES.join(", ")))?;
    if let Some(k) = stretch {
        ops::transform(&mut m, &Region::All, [0.0; 3], [0.0; 3], k, [0.0; 3]);
    }
    Ok(m)
}

/// How a step's new geometry is skinned on a rigged model: `bind` is
/// `smooth` (the body's weights), `piece` (each piece rigid on the bone that
/// carries the most under it) or `bone` (rigid on `bone`); an added part
/// defaults to `piece`, so a hat or a badge stays hard on its bone, and any
/// other step to `smooth`.
fn bind_of(before: &Mesh, step: &Value, adds: bool) -> Result<studio_mesh::rigfit::Bind, String> {
    let bone = match step.get("bone").and_then(Value::as_str) {
        Some(b) => Some(before.rig.as_ref().ok_or("no rig")?.resolve_bone(b)?),
        None => None,
    };
    let word = step.get("bind").and_then(Value::as_str).unwrap_or(if bone.is_some() { "bone" } else if adds { "piece" } else { "smooth" });
    studio_mesh::rigfit::Bind::parse(word, bone)
}

fn is_character(shape: &str) -> bool {
    matches!(shape, "character" | "person" | "human" | "mannequin" | "figure" | "body" | "humanoid" | "baby" | "toddler" | "child" | "kid" | "teen" | "elder" | "toon" | "chibi" | "heroic" | "anime" | "manga" | "anime_chibi")
}

/// Whether a model op asks for the anime or toon look of a character.
fn wants_toon_look(op: &Value) -> bool {
    let word = |k: &str| op.get(k).and_then(Value::as_str).map(str::to_ascii_lowercase);
    is_character(op.get("shape").and_then(Value::as_str).unwrap_or("")) && [word("style"), word("preset"), word("shape")].iter().flatten().any(|w| matches!(w.as_str(), "anime" | "manga" | "anime_chibi" | "toon" | "cartoon" | "cel"))
}

/// The toon shading a model clip is drawn with, as its generator params:
/// `shading` ("toon", "cel", "anime", "flat", or "physical" to turn it off;
/// or `{"style", "bands", "outline", "outline_colour", "shadow_colour",
/// "rim", "rim_colour", "highlights"}`), and the same keys flat on the op
/// (`outline` is the ink line width in px, or false for none).
pub(super) fn shading_params(op: &Value, default_toon: bool) -> Result<Vec<(String, m::Value)>, String> {
    let colour = |v: &Value, k: &str| -> Result<m::Value, String> {
        let s = v.as_str().ok_or_else(|| format!("{k} is a colour (#rrggbb or a name)"))?;
        let c = super::craft::colour(s).ok_or_else(|| format!("{k}: colour \"{s}\" is not #rrggbb or a basic name"))?;
        Ok(m::Value::Colour(m::Rgba::new(c.r() as f64 / 255.0, c.g() as f64 / 255.0, c.b() as f64 / 255.0, 1.0)))
    };
    let obj = op.get("shading").and_then(Value::as_object);
    let get = |k: &str| obj.and_then(|o| o.get(k)).or_else(|| op.get(k));
    let style = match op.get("shading") {
        Some(Value::String(s)) => Some(s.to_ascii_lowercase()),
        Some(Value::Object(o)) => Some(o.get("style").and_then(Value::as_str).unwrap_or("toon").to_ascii_lowercase()),
        Some(Value::Bool(true)) => Some("toon".into()),
        Some(Value::Bool(false)) => Some("physical".into()),
        Some(_) => return Err("shading is \"toon\" (also cel, anime, flat) or \"physical\", or {\"style\": \"toon\", \"bands\": 2, \"outline\": 2, \"outline_colour\": \"#1a1418\", \"shadow_colour\": \"#8c80b0\", \"rim\": 0.35}".into()),
        None if default_toon => Some("toon".into()),
        None => None,
    };
    let Some(style) = style else { return Ok(Vec::new()) };
    let mut out = Vec::new();
    match style.as_str() {
        "toon" | "cel" | "anime" | "cartoon" | "flat" => out.push(("shading".to_string(), m::Value::Text(if style == "flat" { "flat".into() } else { "toon".into() }))),
        "physical" | "pbr" | "realistic" | "off" | "none" => {
            out.push(("shading".to_string(), m::Value::Text(String::new())));
            return Ok(out);
        }
        other => return Err(format!("shading \"{other}\": toon, cel, anime, flat or physical")),
    }
    if let Some(b) = get("bands").or_else(|| get("toon_bands")) {
        let n = b.as_f64().ok_or("bands is 2 or 3")?;
        out.push(("toon_bands".into(), m::Value::Scalar(n.clamp(2.0, 3.0))));
    }
    match get("outline") {
        Some(Value::Bool(false)) => out.push(("outline".into(), m::Value::Scalar(0.0))),
        Some(Value::Bool(true)) | None => {}
        Some(v) => out.push(("outline".into(), m::Value::Scalar(v.as_f64().ok_or("outline is the ink line width in px (0 or false for none)")?.clamp(0.0, 40.0)))),
    }
    for k in ["outline_colour", "shadow_colour", "rim_colour"] {
        if let Some(v) = get(k).or_else(|| get(k.replace("colour", "color").as_str())) {
            out.push((k.to_string(), colour(v, k)?));
        }
    }
    if let Some(v) = get("rim") {
        out.push(("rim".into(), m::Value::Scalar(v.as_f64().or(v.as_bool().map(|b| if b { 0.35 } else { 0.0 })).ok_or("rim is 0 to 2 (or false)")?)));
    }
    if let Some(v) = get("highlights").and_then(Value::as_bool) {
        out.push(("highlights".into(), m::Value::Bool(v)));
    }
    if let Some(v) = get("threshold").and_then(Value::as_f64) {
        out.push(("toon_threshold".into(), m::Value::Scalar(v.clamp(0.0, 1.0))));
    }
    Ok(out)
}

/// A rigged character `size` px tall. Any figure: `preset` (baby, toddler,
/// child, teen, adult, elder, slim, athletic, heavy, heroic, realistic,
/// stylised, toon, chibi) and then any of `age` (years or a word), `style`
/// (realistic, stylised, toon, or 0 to 1), `figure` (-1 broad .. 1 narrow
/// shouldered, full hipped; or masculine, feminine), `build` (slim … heavy or a number),
/// `shoulders`, `hips`, `legs`, `arms`, `neck`, `hands`, `head_size`, `face`,
/// `dressed` (false: bare, to be dressed with `wear`), the colours `skin`,
/// `top`, `trousers`, `shoes`, `detail`. A figure is born with sculpted hair
/// that is its own, not the outfit's (`dressed: false` keeps it): `hair` is
/// its colour (and the brows'), a style word, `false` for none, or
/// `{"style", "colour", "strands", "length"}`; `hairstyle` picks the style.
fn character(op: &Value, size: f32, detail: usize) -> Result<Mesh, String> {
    use studio_mesh::character::Build;
    let shape = op.get("shape").and_then(Value::as_str).unwrap_or("");
    // `style: "anime"` with no preset starts from the anime proportions.
    let anime_style = matches!(op.get("style").and_then(Value::as_str).map(str::to_ascii_lowercase).as_deref(), Some("anime" | "manga"));
    let preset = op.get("preset").and_then(Value::as_str).or(Build::preset(shape).map(|_| shape)).or(anime_style.then_some("anime"));
    let mut b = match preset {
        Some(p) => Build::preset(p).ok_or_else(|| format!("preset \"{p}\": {}", Build::PRESETS))?,
        None => Build::default(),
    };
    b.detail = detail.clamp(1, 5);
    let srgb = |k: &str| -> Result<Option<[u8; 3]>, String> {
        match op.get(k).and_then(Value::as_str) {
            Some(s) => super::craft::colour(s).map(|c| Some([c.r(), c.g(), c.b()])).ok_or_else(|| format!("{k}: colour \"{s}\" is not #rrggbb or a basic name")),
            None => Ok(None),
        }
    };
    if let Some(c) = srgb("skin")?.or(srgb("colour")?) {
        b.skin = c;
    }
    for (k, slot) in [("top", &mut b.top), ("trousers", &mut b.trousers), ("shoes", &mut b.shoes)] {
        if let Some(c) = srgb(k)? {
            *slot = c;
        }
    }
    // Hair: a colour, a style, false, or both in an object.
    let mut hair_style = ["hairstyle", "hair_style"].iter().find_map(|k| op.get(*k).and_then(Value::as_str)).map(str::to_string);
    let mut hair_strands = false;
    let mut hair_length = 1.0f32;
    let mut hair_on = true;
    let hair_colour = |w: &str| super::craft::colour(w).map(|c| [c.r(), c.g(), c.b()]);
    match op.get("hair") {
        Some(Value::Bool(on)) => hair_on = *on,
        Some(Value::String(w)) => match hair_colour(w.as_str()) {
            Some(c) => b.hair = c,
            None if studio_modeling::hairdo::knows(w) || matches!(w.as_str(), "bald" | "none") => hair_style = Some(w.clone()),
            None => return Err(format!("hair \"{w}\": a colour (#rrggbb or a name), a style ({}), or false", studio_modeling::hairdo::STYLES)),
        },
        Some(Value::Object(o)) => {
            if let Some(w) = o.get("colour").or_else(|| o.get("color")).and_then(Value::as_str) {
                b.hair = hair_colour(w).ok_or_else(|| format!("hair colour \"{w}\" is not #rrggbb or a basic name"))?;
            }
            if let Some(w) = o.get("style").and_then(Value::as_str) {
                hair_style = Some(w.to_string());
            }
            hair_strands = o.get("strands").and_then(Value::as_bool).unwrap_or(false);
            hair_length = o.get("length").and_then(Value::as_f64).map_or(1.0, |x| x as f32);
        }
        _ => {}
    }
    if matches!(hair_style.as_deref(), Some("bald" | "none")) {
        hair_on = false;
    }
    match op.get("build") {
        Some(Value::String(w)) => {
            b.weight = match w.as_str() {
                "slim" | "thin" | "slender" => 0.8,
                "average" | "normal" => 1.0,
                "athletic" | "strong" => 1.1,
                "heavy" | "large" | "stocky" => 1.3,
                other => return Err(format!("build \"{other}\": slim, average, athletic, heavy, or a number 0.8–1.3 (or use \"preset\": {})", Build::PRESETS)),
            }
        }
        Some(v) => b.weight = v.as_f64().unwrap_or(1.0) as f32,
        None => {}
    }
    b.shoulders = num(op, "shoulders").unwrap_or(b.shoulders);
    b.hips = num(op, "hips").unwrap_or(b.hips);
    b.head = num(op, "head_size").unwrap_or(b.head);
    b.legs = num(op, "legs").unwrap_or(b.legs);
    b.arms = num(op, "arms").unwrap_or(b.arms);
    b.neck = num(op, "neck").unwrap_or(b.neck);
    b.hands = num(op, "hands").unwrap_or(b.hands);
    b.face = op.get("face").and_then(Value::as_bool).unwrap_or(true);
    if let Some(d) = op.get("dressed").and_then(Value::as_bool) {
        b.dressed = d;
    }
    if let Some(sp) = op.get("species").and_then(Value::as_str) {
        if !studio_mesh::species::known(sp) {
            return Err(format!("species \"{sp}\": {} (an animal that walks on two legs, with the same body, face and bones as any figure)", studio_mesh::species::NAMES));
        }
        b.species = Some(sp.to_string());
    }
    b.fur = srgb("fur")?;
    match op.get("age") {
        Some(Value::String(w)) => {
            b.age = match w.to_lowercase().as_str() {
                "baby" | "infant" => 1.0,
                "toddler" => 2.5,
                "child" | "kid" => 7.0,
                "teen" | "teenager" => 15.0,
                "adult" | "young adult" => 0.0,
                "middle-aged" | "middle_aged" | "middle aged" => 50.0,
                "elder" | "elderly" | "old" | "senior" => 75.0,
                other => other.parse::<f32>().map_err(|_| format!("age \"{other}\": a number of years, or baby, toddler, child, teen, adult, elder"))?,
            }
        }
        Some(v) => b.age = v.as_f64().unwrap_or(0.0) as f32,
        None => {}
    }
    match op.get("style") {
        Some(Value::String(w)) => {
            b.style = match w.to_lowercase().as_str() {
                "realistic" | "real" | "lifelike" => 0.0,
                "stylised" | "stylized" | "stylish" => 0.5,
                "toon" | "cartoon" | "cartoony" | "chibi" | "cel" => 1.0,
                "anime" | "manga" => {
                    b.anime = true;
                    b.style.max(0.55)
                }
                other => other.parse::<f32>().map_err(|_| format!("style \"{other}\": realistic, stylised, toon, anime, or a number 0 to 1"))?,
            }
        }
        Some(v) => b.style = v.as_f64().unwrap_or(0.0) as f32,
        None => {}
    }
    match op.get("figure") {
        Some(Value::String(w)) => {
            b.figure = match w.to_lowercase().as_str() {
                "masculine" | "male" | "man" | "boy" | "broad" | "square" => -0.8,
                "neutral" | "androgynous" => 0.0,
                "feminine" | "female" | "woman" | "girl" | "curvy" => 0.8,
                other => other.parse::<f32>().map_err(|_| format!("figure \"{other}\": masculine, neutral, feminine, or a number -1 to 1"))?,
            }
        }
        Some(v) => b.figure = v.as_f64().unwrap_or(0.0) as f32,
        None => {}
    }
    let mut mesh = studio_mesh::character::character(size.max(20.0), &b);
    if hair_on && b.face && b.species.is_none() {
        let style = hair_style.unwrap_or_else(|| default_hairstyle(&b).to_string());
        let spec = studio_modeling::hair::HairSpec { style, colour: Some(b.hair), strands: hair_strands, length: hair_length, anime: b.anime || b.style >= 0.99, ..Default::default() };
        match studio_modeling::hair::fit(&mesh, &spec) {
            Ok(g) => {
                studio_mesh::wear::attach(&mut mesh, &g)?;
            }
            Err(e) if op.get("hair").is_some() || op.get("hairstyle").is_some() => return Err(format!("hair: {e}")),
            Err(_) => {}
        }
    }
    Ok(mesh)
}

/// The hair a figure is born with when none is named.
fn default_hairstyle(b: &studio_mesh::character::Build) -> &'static str {
    if b.age > 0.0 && b.age < 3.0 {
        "buzz"
    } else if b.anime {
        if b.figure > 0.3 { "bangs" } else { "spiky" }
    } else if b.figure > 0.3 {
        "long"
    } else {
        "short"
    }
}

/// Material fields of an op or step; `named`: its "name" names the
/// material (in mesh steps, not on the model op where it names the clip).
/// A model from blended parts (`parts`): each melts into the ones before
/// it (`blend` px) or carves them (`subtract`); a part's own colour and
/// finish make a material of their own.
fn blended(op: &Value) -> Result<(Mesh, Option<super::math_shapes::Moving>), String> {
    use studio_mesh::sdf::{Join, Part, Shape};
    // Parts that are formulas, by their place in `parts`.
    let mut fields: Vec<(usize, std::sync::Arc<studio_mesh::math::field::Field>)> = Vec::new();
    let list = op.get("parts").and_then(Value::as_array).ok_or("\"parts\" is a list of {\"shape\", \"at\", \"size\", …}")?;
    if list.is_empty() || list.len() > 400 {
        return Err("\"parts\" holds 1 to 400 parts".into());
    }
    let base = surface_of(op, false)?.unwrap_or_default();
    let mut materials = vec![base.clone()];
    let mut parts = Vec::new();
    for (i, v) in list.iter().enumerate() {
        let bad = |what: &str| format!("part {}: {what}", i + 1);
        let shape_name = v.get("shape").and_then(Value::as_str).unwrap_or("ellipsoid");
        // A tube along a path: tapering pills from point to point.
        if let Some(Value::Array(path)) = v.get("path") {
            let pts: Vec<[f32; 3]> = path.iter().filter_map(v3).collect();
            if pts.len() < 2 {
                return Err(bad("a path needs two or more points [x, y, z]"));
            }
            let r0 = num(v, "radius").unwrap_or(20.0).max(0.1);
            let r1 = num(v, "radius_to").unwrap_or(r0).max(0.1);
            let mat = match surface_of(v, true)? {
                Some(mat) => {
                    materials.push(mat);
                    materials.len() as u32 - 1
                }
                None => 0,
            };
            let n = pts.len() - 1;
            for k in 0..n {
                let ra = r0 + (r1 - r0) * k as f32 / n as f32;
                let rb = r0 + (r1 - r0) * (k + 1) as f32 / n as f32;
                let mut p = Part::new(Shape::Cone, [0.0; 3], [ra * 2.0, rb * 2.0, 0.0]);
                (p.a, p.b) = (pts[k], pts[k + 1]);
                p.blend = num(v, "blend").unwrap_or(if k == 0 { 0.0 } else { ra.min(rb) * 0.3 }).max(0.0);
                p.material = mat;
                parts.push(p);
            }
            continue;
        }
        // A solid from a formula: an implicit `math` filling `size`.
        if let Some(m) = v.get("math") {
            let (f, sides, _) = super::math_shapes::field_of(m).map_err(|e| bad(&e))?;
            let size = super::math_shapes::field_size(sides, super::math_shapes::fit_of(v, 200.0).map_err(|e| bad(&e))?);
            fields.push((parts.len(), f.clone()));
            let mut p = Part::field(f, field_v3(v, "at")?.unwrap_or([0.0; 3]), size);
            p.turn = field_v3(v, "turn")?.unwrap_or([0.0; 3]);
            p.blend = num(v, "blend").unwrap_or(0.0).max(0.0);
            p.join = match (v.get("subtract").and_then(Value::as_bool), v.get("intersect").and_then(Value::as_bool)) {
                (Some(true), _) => Join::Subtract,
                (_, Some(true)) => Join::Intersect,
                _ => Join::Add,
            };
            if let Some(mat) = surface_of(v, true)? {
                materials.push(mat);
                p.material = materials.len() as u32 - 1;
            }
            parts.push(p);
            continue;
        }
        let shape = Shape::parse(shape_name).ok_or_else(|| bad("shape is ellipsoid, box, capsule, cone (tapering, from/to with radius and radius_to), cylinder, torus or wedge; or a \"path\" of points for a tube; or a \"math\" (an implicit formula or preset)"))?;
        let size = match v.get("size") {
            Some(s) if s.is_number() => [s.as_f64().unwrap_or(0.0) as f32; 3],
            Some(s) => v3(s).ok_or_else(|| bad("size is a number or [x, y, z] (px)"))?,
            None => [num(v, "radius").map_or(50.0, |r| r * 2.0); 3],
        };
        let mut p = Part::new(shape, field_v3(v, "at")?.unwrap_or([0.0; 3]), size);
        if shape == Shape::Capsule {
            let (a, b) = (field_v3(v, "from")?.ok_or_else(|| bad("a capsule needs \"from\" and \"to\" ([x, y, z]) and \"radius\""))?, field_v3(v, "to")?.ok_or_else(|| bad("a capsule needs \"to\""))?);
            // Ends are where they are; the part sits at the origin.
            p.at = [0.0; 3];
            (p.a, p.b) = (a, b);
            let r = num(v, "radius").unwrap_or(size[0] * 0.5);
            p.size = [r * 2.0; 3];
        }
        if shape == Shape::Cone {
            let (a, b) = (field_v3(v, "from")?.ok_or_else(|| bad("a cone needs \"from\" and \"to\" ([x, y, z]), \"radius\" (at from) and \"radius_to\""))?, field_v3(v, "to")?.ok_or_else(|| bad("a cone needs \"to\""))?);
            p.at = [0.0; 3];
            (p.a, p.b) = (a, b);
            let r = num(v, "radius").unwrap_or(size[0] * 0.5);
            let r2 = num(v, "radius_to").unwrap_or(r * 0.15);
            p.size = [r * 2.0, r2.max(0.05) * 2.0, 0.0];
        }
        p.turn = field_v3(v, "turn")?.unwrap_or([0.0; 3]);
        p.round = num(v, "round").unwrap_or(0.0);
        p.blend = num(v, "blend").unwrap_or(0.0).max(0.0);
        p.join = match (v.get("subtract").and_then(Value::as_bool), v.get("intersect").and_then(Value::as_bool)) {
            (Some(true), _) => Join::Subtract,
            (_, Some(true)) => Join::Intersect,
            _ => Join::Add,
        };
        if let Some(mat) = surface_of(v, true)? {
            // Same look, same material.
            let k = materials.iter().position(|m| m.base_colour == mat.base_colour && m.metallic == mat.metallic && m.roughness == mat.roughness && m.emissive == mat.emissive && m.subsurface == mat.subsurface).unwrap_or_else(|| {
                materials.push(mat.clone());
                materials.len() - 1
            });
            p.material = k as u32;
        }
        parts.push(p);
    }
    let extent = parts.iter().filter(|p| p.join == Join::Add).map(|p| studio_mesh::length(p.at) + p.size[0].max(p.size[1]).max(p.size[2]) + studio_mesh::length(p.a).max(studio_mesh::length(p.b))).fold(1.0f32, f32::max);
    if parts.is_empty() {
        return Err("\"parts\" made nothing".into());
    }
    let detail = op.get("detail").and_then(Value::as_u64).unwrap_or(4).clamp(1, 6) as usize;
    let mut cell = extent * 2.0 / [60.0, 90.0, 130.0, 170.0, 220.0, 280.0][detail - 1];
    // Fine enough that the thinnest part (a comb, a feather, an ear) keeps
    // three cells of thickness instead of vanishing or breaking up.
    let thin = parts.iter().filter(|p| p.join == Join::Add).map(studio_mesh::sdf::thinnest).fold(f32::INFINITY, f32::min);
    if thin.is_finite() && thin > 0.0 {
        cell = cell.min(thin / 3.0).max(extent * 2.0 / 360.0);
    }
    // Formulas cost more per sample: the grid stays under the cap.
    if !fields.is_empty() {
        let cost = fields.iter().map(|(_, f)| f.cost()).sum::<u64>() + 1;
        cell = cell.max(studio_mesh::math::field::cell_for([extent * 2.0; 3], detail, cost));
    }
    let m = studio_mesh::sdf::build(&parts, materials, cell);
    if m.triangles() == 0 {
        return Err("the parts make no surface (all carved away, or no part adds)".into());
    }
    let moving = super::math_shapes::moving_parts(parts, fields, cell, m.positions.len());
    Ok((m, moving))
}

/// How strong a glow is: a multiplier of the `emissive` colour, above 1
/// brighter than white (a neon tube 4-8, a lit window 2-4, a bulb 6-10), so
/// it blooms and lights the air round it.
pub(super) fn glow_strength(op: &Value) -> Result<Option<f32>, String> {
    match op.get("emissive_strength") {
        None | Some(Value::Null) => Ok(None),
        Some(v) => {
            let x = v.as_f64().filter(|x| x.is_finite()).ok_or_else(|| format!("emissive_strength is a number, a multiplier of the glow colour (1 as drawn, a neon tube 4 to 8, a lit window 2 to 4, a bulb 6 to 10); you gave {v}"))?;
            if !(0.0..=100.0).contains(&x) {
                return Err(format!("emissive_strength {x} is out of range: it multiplies the glow colour, write 0 (off) to 100; a neon tube 4 to 8, a lit window 2 to 4, a bulb 6 to 10"));
            }
            Ok(Some(x as f32))
        }
    }
}

/// How wet a model's surface is, 0 dry to 1 soaked: darker and richer in
/// colour, glossy, reflecting the lights and what is round it, with puddles
/// where it faces up.
pub(super) fn wetness_of(op: &Value) -> Result<Option<f64>, String> {
    match op.get("wetness") {
        None | Some(Value::Null) => Ok(None),
        Some(v) => {
            let x = v.as_f64().filter(|x| x.is_finite()).ok_or_else(|| format!("wetness is a number from 0 (dry) to 1 (soaked); you gave {v}"))?;
            if !(0.0..=1.0).contains(&x) {
                return Err(format!("wetness {x} is out of range: write 0 (dry) to 1 (soaked); 0.3 damp, 0.7 rain-wet street, 1 puddles on every flat surface"));
            }
            Ok(Some(x))
        }
    }
}

fn surface_of(op: &Value, named: bool) -> Result<Option<studio_mesh::Material>, String> {
    let keys = ["colour", "metallic", "roughness", "emissive", "opacity", "subsurface", "transmission", "ior"];
    let strength = glow_strength(op)?;
    if strength.is_some() && op.get("emissive").is_none() && keys.iter().any(|k| op.get(*k).is_some()) {
        return Err("emissive_strength multiplies the glow colour, so it comes with emissive in the same call: add \"emissive\": \"#ff9a3c\" (the colour of the light it gives off)".into());
    }
    if !keys.iter().any(|k| op.get(*k).is_some()) {
        return Ok(None);
    }
    let mut mat = studio_mesh::Material::default();
    if let Some(c) = op.get("colour") {
        mat.base_colour = linear_colour(c)?;
    }
    if let Some(v) = num(op, "metallic") {
        mat.metallic = v.clamp(0.0, 1.0);
    }
    if let Some(v) = num(op, "roughness") {
        mat.roughness = v.clamp(0.0, 1.0);
    }
    if let Some(c) = op.get("emissive") {
        let e = linear_colour(c)?;
        let k = strength.unwrap_or(1.0);
        mat.emissive = [e[0] * k, e[1] * k, e[2] * k];
    }
    if let Some(v) = num(op, "opacity") {
        mat.base_colour[3] = v.clamp(0.0, 1.0);
    }
    if let Some(v) = num(op, "subsurface") {
        mat.subsurface = v.clamp(0.0, 1.0);
    }
    if let Some(v) = num(op, "transmission") {
        mat.transmission = v.clamp(0.0, 1.0);
    }
    if let Some(v) = num(op, "ior") {
        mat.ior = v.clamp(1.0, 4.0);
    }
    if let Some(n) = op.get("name").and_then(Value::as_str).filter(|_| named) {
        mat.name = n.to_owned();
    }
    Ok(Some(mat))
}

pub(super) fn read_picture(path: &str) -> Result<studio_mesh::Texture, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    let img = if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        studio_media::png::decode(&bytes).map_err(|e| format!("{path}: {e:?}"))?
    } else if bytes.starts_with(&[0xff, 0xd8]) {
        studio_media::jpeg::decode(&bytes).map_err(|e| format!("{path}: {e}"))?
    } else {
        return Err(format!("{path}: textures are PNG or JPEG"));
    };
    Ok(studio_mesh::Texture { width: img.width, height: img.height, rgba: img.pixels })
}

/// Mirrors texture coordinates: "v" turns a picture upside down on the
/// surface (files made for programs that read pictures bottom up), "u"
/// left to right.
fn flip_uvs(mesh: &mut Mesh, which: &str) -> Result<(), String> {
    let (u, v) = match which {
        "v" | "vertical" | "y" => (false, true),
        "u" | "horizontal" | "x" => (true, false),
        "both" | "uv" => (true, true),
        _ => return Err("\"flip\" is v (upside down), u (left to right) or both".into()),
    };
    for t in &mut mesh.uvs {
        if u {
            t[0] = 1.0 - t[0];
        }
        if v {
            t[1] = 1.0 - t[1];
        }
    }
    Ok(())
}

/// The materials of a model. Few: all of them. Many (a model built of a
/// hundred parts has a hundred): the ones that are textured or named, and
/// how many are left out, so the reply is not a hundred identical lines.
fn materials_said(m: &Mesh) -> Value {
    let line = |(i, mat): (usize, &studio_mesh::Material)| json!({"index": i, "name": mat.name, "textured": mat.texture.is_some()});
    if m.materials.len() <= 6 {
        return Value::Array(m.materials.iter().enumerate().map(line).collect());
    }
    let plain = |mat: &studio_mesh::Material| mat.texture.is_none() && mat.name == "Default";
    let mut shown: Vec<Value> = m.materials.iter().enumerate().filter(|(_, mat)| !plain(mat)).map(line).collect();
    let left_out = m.materials.len() - shown.len();
    shown.push(json!({"plain_default_materials_not_listed": left_out, "total": m.materials.len()}));
    Value::Array(shown)
}

/// What a model is made of, by name: each name that carries triangles with
/// their count (same-named parts add up), the first sixteen and how many
/// more, so a model built of a hundred parts is not a hundred lines.
fn parts_said(m: &Mesh) -> Value {
    let mut per_material = vec![0usize; m.materials.len()];
    for k in m.tri_materials() {
        if let Some(n) = per_material.get_mut(k as usize) {
            *n += 1;
        }
    }
    let mut named: Vec<(&str, usize)> = Vec::new();
    for (mt, n) in m.materials.iter().zip(per_material).filter(|(_, n)| *n > 0) {
        match named.iter_mut().find(|(name, _)| *name == mt.name) {
            Some(entry) => entry.1 += n,
            None => named.push((&mt.name, n)),
        }
    }
    let mut shown: Vec<Value> = named.iter().take(16).map(|(name, n)| json!({"name": name, "triangles": n})).collect();
    if named.len() > 16 {
        shown.push(json!({"more_parts": named.len() - 16}));
    }
    Value::Array(shown)
}

/// Words about a model for the agent: size, place of its box, triangles.
pub(super) fn describe(m: &Mesh) -> Value {
    let (lo, hi) = m.bounds();
    let r = |v: f32| (v * 10.0).round() / 10.0;
    let worn = if m.rig.is_some() { Some(studio_mesh::wear::worn(m)) } else { None };
    json!({
        "triangles": m.triangles(),
        "vertices": m.positions.len(),
        "materials": materials_said(m),
        "box_min": [r(lo[0]), r(lo[1]), r(lo[2])],
        "box_max": [r(hi[0]), r(hi[1]), r(hi[2])],
        "rig": m.rig.as_deref().map(super::anim3d::describe_rig),
        "parts": parts_said(m),
        "worn": worn,
    })
}

impl Session {
    /// Saves a model by its content; returns the file's path.
    pub(super) fn save_model(&self, mesh: &Mesh) -> Result<String, String> {
        crate::mesh_store::save(&self.doc, mesh)
    }

    /// A model clip's mesh and its clip.
    pub(super) fn model_of(&self, id: u32) -> Result<(m::Clip, Mesh), String> {
        let mc = self.model_clip(id).ok_or_else(|| format!("no clip {id}"))?;
        if crate::mesh_store::file_of(&mc).is_none() {
            return Err(format!("clip {id} is not a 3D model (add one with the model op)"));
        }
        let mesh = crate::mesh_store::load(&mc)?;
        Ok((mc, mesh))
    }

    /// The file a tool argument names: a web link is downloaded into the
    /// project's Downloads first; a name, a relative path, `~/…` or a
    /// `file://` link is found in the project's folders. The error says
    /// where it looked.
    pub(super) fn input_file(&mut self, given: &str) -> Result<std::path::PathBuf, String> {
        let home = self.home.clone().or_else(|| self.doc.folder.as_ref().map(|f| f.root().to_path_buf()));
        if download::is_url(given) {
            let dir = download::folder_for_kind(home.as_deref(), m::file::MediaKind::Models3d);
            return download::fetch_pick(given, &dir, None, &download::Pick::default()).map(|f| f.file).map_err(|e| format!("could not download {given}: {e}"));
        }
        download::resolve_local(home.as_deref(), given)
    }

    /// `model`: add a 3D model (a shape, or a file), or export one.
    pub(super) fn model(&mut self, op: &Value) -> Result<String, String> {
        if let Some(out) = op.get("export").and_then(Value::as_str) {
            let id = op.get("clip").and_then(Value::as_u64).ok_or("export needs \"clip\"")? as u32;
            let (mc, mesh) = self.model_of(id)?;
            crate::mesh_store::export_model(&mc, &mesh, std::path::Path::new(out))?;
            return Ok(format!("clip {id} exported to {out}"));
        }
        if op.get("clip").is_some() {
            return Err("model adds a new model (shape or file) or exports one (\"clip\" + \"export\"); to change a model use the mesh op".into());
        }
        let short = self.doc.project.width.min(self.doc.project.height) as f32;
        let mut moving: Option<super::math_shapes::Moving> = None;
        let mut math_said: Option<Value> = None;
        let mut native_frame: Option<super::anim3d::Frame> = None;
        let mut recipe: Option<studio_mesh::math::field::Recipe> = None;
        let direct = match op.get("render").and_then(Value::as_str) {
            None | Some("mesh") => false,
            Some("direct") => true,
            Some(other) => return Err(format!("render \"{other}\": mesh (triangles, the default) or direct (drawn straight from a math formula)")),
        };
        if direct && op.get("math").is_none() {
            return Err("render: direct draws a math shape from its formula; give \"math\" (an implicit formula or preset)".into());
        }
        let (mut mesh, what, notes) = match op.get("file").and_then(Value::as_str) {
            Some(file) => {
                let fit = num(op, "size").unwrap_or(short * 0.6);
                // A link is downloaded first; a name or relative path is found
                // in the project's folders.
                let path = self.input_file(file)?;
                let file = path.to_string_lossy().into_owned();
                let file = file.as_str();
                // Read at its own size first: the numbers a rig written in
                // the file's units needs are its centre and scale.
                let mut got = crate::mesh_store::read_model(&path, 0.0)?;
                {
                    let (lo, hi) = got.mesh.bounds();
                    let big = (hi[0] - lo[0]).max(hi[1] - lo[1]).max(hi[2] - lo[2]);
                    let k = if fit > 0.0 && big > 1e-6 { fit / big } else { 1.0 };
                    let c = studio_mesh::scale(studio_mesh::add(lo, hi), 0.5);
                    native_frame = Some(super::anim3d::Frame { formula: false, centre: c.map(f64::from), scale: [k; 3] });
                    if fit > 0.0 {
                        ops::centre_and_fit(&mut got.mesh, fit);
                    }
                }
                let n = op.get("name").and_then(Value::as_str).map(str::to_owned).unwrap_or_else(|| std::path::Path::new(file).file_stem().and_then(|s| s.to_str()).unwrap_or("Model").to_owned());
                if got.mesh.triangles() > MAX_TRIANGLES {
                    let mut m = got.mesh;
                    studio_mesh::decimate::decimate(&mut m, MAX_TRIANGLES);
                    (m, n, [got.notes, vec![format!("decimated to {MAX_TRIANGLES} triangles")]].concat())
                } else {
                    (got.mesh, n, got.notes)
                }
            }
            None if op.get("math").is_some() => {
                let fit = super::math_shapes::fit_of(op, short * 0.4)?;
                let detail = op.get("detail").and_then(Value::as_u64).unwrap_or(4).clamp(1, 6) as usize;
                // Drawn directly, the mesh is only a stand-in: a coarse one.
                let mut made = super::math_shapes::make(&op["math"], fit, if direct { detail.min(2) } else { detail })?;
                if made.mesh.triangles() > MAX_TRIANGLES {
                    // Too fine to keep: simplified (a lower detail is faster).
                    let had = made.mesh.triangles();
                    studio_mesh::decimate::decimate(&mut made.mesh, MAX_TRIANGLES);
                    made.said["notes"] = json!([format!("{had} triangles simplified to {} (the most a model keeps); a lower detail, or render: direct, keeps it lighter", made.mesh.triangles())]);
                    // Its vertices are not the bake's any more.
                    made.moving = None;
                }
                if direct && made.recipe.is_none() {
                    return Err("render: direct draws implicit solids and fractals (a distance field, sphere traced); parametric surfaces, tubes, height fields and growth are drawn as their meshes".into());
                }
                if op["math"].get("bake").and_then(Value::as_bool) != Some(false) && !direct {
                    moving = made.moving;
                }
                recipe = made.recipe;
                math_said = Some(made.said);
                (made.mesh, op.get("name").and_then(Value::as_str).map(str::to_owned).unwrap_or(made.name), Vec::new())
            }
            None if op.get("parts").is_some() => {
                let (m, mv) = blended(op)?;
                moving = mv;
                (m, op.get("name").and_then(Value::as_str).unwrap_or("Model").to_owned(), Vec::new())
            }
            None => {
                // No shape used to make a sphere and say ok.
                if op.get("shape").and_then(Value::as_str).is_none() {
                    return Err(format!("model needs \"shape\" ({}, head, or a character: person, child, toon, …), \"file\" (a model file) or \"parts\"", studio_mesh::prim::NAMES.join(", ")));
                }
                let m = primitive(op, if is_character(op.get("shape").and_then(Value::as_str).unwrap_or("")) { short * 0.75 } else { short * 0.4 })?;
                let shape = op.get("shape").and_then(Value::as_str).unwrap_or("sphere");
                let mut n = shape.to_owned();
                if let Some(c) = n.get_mut(0..1) {
                    c.make_ascii_uppercase();
                }
                (m, n, Vec::new())
            }
        };
        let shape_name = op.get("shape").and_then(Value::as_str).unwrap_or("");
        if glow_strength(op)?.is_some() && op.get("emissive").is_none() {
            return Err("emissive_strength multiplies the glow colour, so it comes with emissive in the same call: add \"emissive\": \"#ff9a3c\" (the colour of the light it gives off); to strengthen a model you already have, use mesh {do: \"material\", emissive_strength}".into());
        }
        wetness_of(op)?;
        if let Some(mat) = surface_of(op, false)?.filter(|_| op.get("parts").is_none() && !is_character(shape_name) && !matches!(shape_name, "head" | "human_head" | "face" | "bust")) {
            // On a file, the colour and finish go over every material.
            let keep_tex: Vec<_> = mesh.materials.iter().map(|m| m.texture.clone()).collect();
            if op.get("math").is_some() && mesh.materials.len() > 1 {
                // A plant's bark, not its leaves; a cone's seeds, not its core.
                let name = mesh.materials[0].name.clone();
                mesh.materials[0] = studio_mesh::Material { name, ..mat };
            } else if op.get("file").is_some() {
                for (i, m) in mesh.materials.iter_mut().enumerate() {
                    *m = studio_mesh::Material { texture: keep_tex[i].clone(), name: m.name.clone(), ..mat.clone() };
                }
            } else {
                mesh.materials = vec![mat];
            }
        }
        // A head's colour is its skin's; the eyes and lips keep theirs.
        if matches!(op.get("shape").and_then(Value::as_str), Some("head" | "human_head" | "face" | "bust"))
            && let Some(mat) = surface_of(op, false)?
            && let Some(skin) = mesh.materials.first_mut()
        {
            if op.get("colour").is_some() {
                skin.base_colour = mat.base_colour;
            }
            if op.get("roughness").is_some() {
                skin.roughness = mat.roughness;
            }
            if op.get("subsurface").is_some() {
                skin.subsurface = mat.subsurface;
            }
        }
        let mut said = self.place_model(op, &mesh, what, notes)?;
        let frame = native_frame.or_else(|| math_said.as_ref().and_then(|m| super::anim3d::Frame::from_json(&m["frame"])));
        if let Some(f) = frame {
            let id = self.doc.project.clips.iter().map(|c| c.id).max().unwrap_or(0) as u32;
            if let Some(mut mc) = self.model_clip(id) {
                super::anim3d::set_frame(&mut mc, &f);
                let seq = self.doc.seq();
                let ok = self.doc.apply("Model frame", m::Edit::SetClip { seq, clip: Box::new(mc) });
                self.check(ok, "")?;
            }
        }
        if let Some(m) = math_said {
            let id = self.doc.project.clips.iter().map(|c| c.id).max().unwrap_or(0) as u32;
            let how = self.keep_math(id, m["kind"].as_str().unwrap_or("math"), recipe.as_ref(), direct)?;
            said.push_str(&format!("; math: {m}"));
            if direct {
                said.push_str(&format!("; {how}"));
            }
        }
        if let Some(mv) = moving {
            let id = self.doc.project.clips.iter().map(|c| c.id).max().unwrap_or(0) as u32;
            said.push_str(&format!("; {}", self.bake_math(id, &mesh, &mv)?));
        }
        Ok(said)
    }

    /// Puts a finished model on the timeline as a model clip: `at`,
    /// `duration`, `lane`, `position`, `rotation` and `shadows` from `op`,
    /// `name` (else `what`) for the clip.
    pub(super) fn place_model(&mut self, op: &Value, mesh: &Mesh, what: String, notes: Vec<String>) -> Result<String, String> {
        self.place_model_with(op, mesh, what, notes, None)
    }

    /// As [`Self::place_model`], keeping an editable source with it (parts,
    /// modifiers, materials: what edit mode and `mesh` edit steps open).
    pub(super) fn place_model_with(&mut self, op: &Value, mesh: &Mesh, what: String, notes: Vec<String>, source: Option<&studio_modeling::asset::Asset>) -> Result<String, String> {
        let file = self.save_model(mesh)?;
        let source = match source {
            Some(a) => Some(crate::mesh_store::save_source(&self.doc, a)?),
            None => None,
        };
        let at = micros(f(op, "at").unwrap_or(0.0));
        let end = super::new_clip_end(self.doc.project.duration(), at);
        let dur = f(op, "duration").map(micros).unwrap_or(end - at).max(SEC / 10);
        let lane = match op.get("lane") {
            Some(v) => self.room(Some(super::craft::lane_of(&self.doc.project, v, None)?), at, dur, None)?,
            None => self.room(None, at, dur, None)?,
        };
        let id = self.doc.alloc_id() as u32;
        let name = op.get("name").and_then(Value::as_str).map(str::to_owned).unwrap_or(what);
        let mut c = crate::mesh_store::new_clip(id as u64, lane as u64, at, dur, file, &name);
        crate::mesh_store::set_source(&mut c, source);
        if let Some(p) = field_v3(op, "position")? {
            c.transform.position = m::Vec2::new(p[0] as f64, p[1] as f64);
            c.transform.position_z = p[2] as f64;
        }
        if let Some(r) = field_v3(op, "rotation")? {
            (c.transform.rotation_x, c.transform.rotation_y, c.transform.rotation) = (r[0] as f64, r[1] as f64, r[2] as f64);
        }
        if let Some(w) = wetness_of(op)?
            && let m::ClipSource::Generator(g) = &mut c.source
        {
            g.params.insert("wetness".into(), m::Value::Scalar(w));
        }
        // Toon shading: asked for, or the anime and toon look's own.
        let shading = shading_params(op, wants_toon_look(op))?;
        if let m::ClipSource::Generator(g) = &mut c.source {
            for (k, v) in &shading {
                g.params.insert(k.clone(), v.clone());
            }
        }
        // Models throw shadows unless told not to.
        if op.get("shadows").and_then(Value::as_bool) == Some(false) {
            c.material.get_or_insert_with(Default::default).casts_shadows = false;
        }
        let seq = self.doc.seq();
        let ok = self.doc.apply("Add model", m::Edit::InsertClip { seq, clip: Box::new(c) });
        self.check(ok, "")?;
        let mut said = format!("model clip {id}: {}", describe(mesh));
        if let Some(rest) = self.rest_placed(op, id) {
            said.push_str(&format!("; {rest}"));
        }
        if !notes.is_empty() {
            said.push_str(&format!("; {}", notes.join("; ")));
        }
        if let Some(kept) = self.kept_like(&name) {
            said.push_str(&format!("; {kept}"));
        }
        if shading.iter().any(|(k, v)| k == "shading" && matches!(v, m::Value::Text(t) if !t.is_empty())) {
            said.push_str("; drawn toon-shaded (cel bands, rim light, ink outline; change with the shading op)");
        }
        Ok(said)
    }

    /// `shading`: the toon look on model clips, or back to physical
    /// shading: `clip`/`clips` (default every model clip in the sequence),
    /// `shading`, `bands`, `outline`, `outline_colour`, `shadow_colour`,
    /// `rim`, `rim_colour`, `highlights`, `threshold`.
    pub(super) fn shading_op(&mut self, op: &Value) -> Result<String, String> {
        let mut ids: Vec<u32> = op.get("clip").and_then(Value::as_u64).map(|c| c as u32).into_iter().collect();
        ids.extend(super::ids(op, "clips"));
        if ids.is_empty() {
            ids = self.doc.model.sequence(self.doc.seq()).map(|q| q.clips.iter().filter(|c| matches!(&c.source, m::ClipSource::Generator(g) if g.kind == m::GeneratorKind::Model)).map(|c| c.id.0 as u32).collect()).unwrap_or_default();
        }
        if ids.is_empty() {
            return Err("shading: no model clips here to shade".into());
        }
        let params = shading_params(op, true)?;
        let toon = params.iter().any(|(k, v)| k == "shading" && matches!(v, m::Value::Text(t) if !t.is_empty()));
        for &id in &ids {
            let mut mc = self.model_clip(id).ok_or_else(|| format!("no clip {id}"))?;
            match &mut mc.source {
                m::ClipSource::Generator(g) if g.kind == m::GeneratorKind::Model => {
                    if !toon {
                        for k in ["toon_bands", "outline", "outline_colour", "shadow_colour", "rim", "rim_colour", "highlights", "toon_threshold"] {
                            g.params.remove(k);
                        }
                    }
                    for (k, v) in &params {
                        g.params.insert(k.clone(), v.clone());
                    }
                }
                _ => return Err(format!("clip {id} is not a model clip: shading is for 3D models and worlds")),
            }
            let seq = self.doc.seq();
            let ok = self.doc.apply("Shading", m::Edit::SetClip { seq, clip: Box::new(mc) });
            self.check(ok, "")?;
        }
        Ok(format!("{} model clip{} {}", ids.len(), if ids.len() == 1 { "" } else { "s" }, if toon { "toon-shaded: cel bands, a shadow colour, rim light and ink outlines" } else { "back to physical shading" }))
    }

    /// `mesh`: change a model, one step or a list of steps in order (one
    /// undo step for all of them).
    pub(super) fn mesh(&mut self, op: &Value) -> Result<String, String> {
        let id = op.get("clip").and_then(Value::as_u64).ok_or("mesh needs \"clip\" (a model clip)")? as u32;
        let (mut mc, mut mesh) = self.model_of(id)?;
        let steps: Vec<Value> = match op.get("steps") {
            Some(Value::Array(a)) => a.clone(),
            Some(_) => return Err("\"steps\" is a list of {\"do\": …} steps".into()),
            // wetness and emissive_strength on their own are a material step.
            None if op.get("do").is_none() && (op.get("wetness").is_some() || op.get("emissive_strength").is_some()) => {
                let mut s = op.clone();
                s["do"] = Value::String("material".into());
                vec![s]
            }
            None => vec![op.clone()],
        };
        if steps.is_empty() {
            return Err("no steps".into());
        }
        if super::mesh_edit::is_edit(op, &steps) {
            return self.mesh_edit(op, &steps);
        }
        let mut said = Vec::new();
        let before = mesh.clone();
        for (i, step) in steps.iter().enumerate() {
            let what = step.get("do").and_then(Value::as_str).ok_or_else(|| format!("step {}: needs \"do\" ({})", i + 1, STEPS.join(", ")))?;
            // A rigged model is kept whole by every step: what the step adds
            // is skinned (a part added rigid to the bone under it, or as
            // asked with "bind"), what it rebuilds keeps the old weights.
            let snap = mesh.rig.is_some().then(|| mesh.clone());
            let r = self.mesh_step(&mut mesh, &mut mc, what, step).map_err(|e| format!("step {} ({what}): {e}. Nothing was changed.", i + 1))?;
            said.push(r);
            if let Some(snap) = snap {
                let bind = bind_of(&snap, step, matches!(what, "add" | "merge")).map_err(|e| format!("step {} ({what}): {e}. Nothing was changed.", i + 1))?;
                if let Some(n) = mesh.sync_rig(&snap, &bind) {
                    said.push(n);
                }
            }
            if mesh.triangles() > MAX_TRIANGLES {
                return Err(format!("step {} ({what}) made {} triangles, over {MAX_TRIANGLES}; subdivide less or decimate first. Nothing was changed.", i + 1, mesh.triangles()));
            }
            if mesh.triangles() == 0 {
                return Err(format!("step {} ({what}) left no triangles. Nothing was changed.", i + 1));
            }
        }
        mesh.tidy();
        if let Some(n) = mesh.sync_rig(&before, &studio_mesh::rigfit::Bind::Smooth) {
            said.push(n);
        }
        let file = self.save_model(&mesh)?;
        crate::mesh_store::set_file(&mut mc, file);
        // The triangles changed under any editable source: it no longer
        // says what the model is (edit mode makes a new one from these).
        crate::mesh_store::set_source(&mut mc, None);
        let seq = self.doc.seq();
        let ok = self.doc.apply("Model", m::Edit::SetClip { seq, clip: Box::new(mc) });
        self.check(ok, "")?;
        Ok(format!("{}; now {}", said.join("; "), describe(&mesh)))
    }

    fn mesh_step(&mut self, mesh: &mut Mesh, mc: &mut m::Clip, what: &str, s: &Value) -> Result<String, String> {
        let reg = || region(s.get("region"));
        Ok(match what {
            "subdivide" => {
                let levels = s.get("levels").and_then(Value::as_u64).unwrap_or(1).min(4) as usize;
                ops::subdivide(mesh, levels);
                format!("subdivided {levels}×")
            }
            "bevel" | "chamfer" => {
                use studio_modeling::{bevel, poly::{NormalMode, Poly}, select::Edges};
                let (lo, hi) = mesh.bounds();
                let biggest = (hi[0] - lo[0]).max(hi[1] - lo[1]).max(hi[2] - lo[2]).max(1.0);
                let width = num(s, "width").unwrap_or(biggest * 0.012).max(biggest * 0.0005);
                let segments = s.get("segments").and_then(Value::as_u64).unwrap_or(3).clamp(1, 8) as usize;
                let angle = num(s, "angle").unwrap_or(50.0).clamp(5.0, 175.0);
                let profile = num(s, "profile").unwrap_or(0.5).clamp(0.0, 1.0);
                let mut poly = Poly::from_mesh(mesh);
                // Flat triangles join back into the polygons they were cut from,
                // so a corner has the faces a modeler's bevel expects.
                studio_modeling::modifiers::limited_dissolve(&mut poly, 1.0);
                let edges = Edges::Sharper { degrees: angle }.list(&poly);
                if edges.is_empty() {
                    return Err(format!("no edges bend more than {angle} degrees, so there is nothing to bevel (lower \"angle\")"));
                }
                let n = bevel::bevel(&mut poly, &edges, width, segments, profile);
                if n == 0 {
                    return Err("none of those edges could be bevelled (open or non-manifold surface there); make the pieces closed shapes first".into());
                }
                *mesh = poly.to_mesh(40.0, NormalMode::Area);
                format!("bevelled {n} sharp edge{} {width:.1} px wide in {segments} segment{}", if n == 1 { "" } else { "s" }, if segments == 1 { "" } else { "s" })
            }
            "smooth" => {
                ops::smooth(mesh, &reg()?, s.get("iterations").and_then(Value::as_u64).unwrap_or(5) as usize, num(s, "strength").unwrap_or(1.0));
                "smoothed".into()
            }
            "sculpt" | "stroke" => {
                let brush_name = s.get("brush").and_then(Value::as_str).unwrap_or("inflate");
                let by = field_v3(s, "by")?;
                let brush = studio_mesh::ops::Brush::parse(brush_name, by).ok_or_else(|| format!("brush \"{brush_name}\": inflate, grab, smooth, flatten, pinch or crease"))?;
                let radius = num(s, "radius").ok_or("needs \"radius\" (pixels)")?;
                let strength = num(s, "strength").unwrap_or(match brush {
                    studio_mesh::ops::Brush::Smooth | studio_mesh::ops::Brush::Flatten => 0.5,
                    _ => radius * 0.15,
                });
                let symmetry = s.get("symmetry").and_then(Value::as_bool).unwrap_or(true);
                let points: Vec<V3> = match (s.get("path"), field_v3(s, "at")?) {
                    (Some(Value::Array(p)), _) => {
                        let pts: Vec<V3> = p.iter().filter_map(v3).collect();
                        if pts.len() != p.len() || pts.is_empty() {
                            return Err("\"path\" is a list of [x, y, z] points".into());
                        }
                        // Dabs a third of the radius apart along the path.
                        let mut dabs = vec![pts[0]];
                        for w in pts.windows(2) {
                            let len = studio_mesh::length(studio_mesh::sub(w[1], w[0]));
                            let n = (len / (radius / 3.0).max(0.5)).ceil().max(1.0) as usize;
                            for k in 1..=n {
                                dabs.push(studio_mesh::lerp(w[0], w[1], k as f32 / n as f32));
                            }
                        }
                        dabs
                    }
                    (_, Some(at)) => vec![at],
                    _ => return Err("needs \"at\": [x, y, z] (or \"path\": a list of points)".into()),
                };
                // A stroke spreads the brush over its dabs.
                let per = if points.len() > 1 && !matches!(brush, studio_mesh::ops::Brush::Smooth | studio_mesh::ops::Brush::Flatten) { strength / (points.len() as f32 / 3.0).max(1.0) } else { strength };
                for p in &points {
                    ops::sculpt(mesh, brush, *p, radius, per, symmetry);
                }
                format!("{brush_name} at {} point{}", points.len(), if points.len() == 1 { "" } else { "s" })
            }
            "symmetrize" | "symmetry" => {
                let keep = match s.get("keep").and_then(Value::as_str).unwrap_or("right") {
                    "right" | "+x" => 1.0,
                    "left" | "-x" => -1.0,
                    _ => return Err("\"keep\" is right or left".into()),
                };
                ops::symmetrize(mesh, keep);
                "made symmetric".into()
            }
            "mirror" => {
                let axis = match s.get("axis").and_then(Value::as_str).unwrap_or("x") {
                    "x" => 0,
                    "y" => 1,
                    "z" => 2,
                    _ => return Err("\"axis\" is x, y or z".into()),
                };
                ops::mirror(mesh, axis);
                "mirrored copy added".into()
            }
            "extrude" => {
                let d = num(s, "distance").ok_or("needs \"distance\" (pixels; negative pushes in)")?;
                let n = ops::extrude(mesh, &reg()?, d);
                if n == 0 {
                    return Err("the region holds no faces".into());
                }
                format!("{n} faces extruded {d} px")
            }
            "displace" | "noise" => {
                ops::displace(mesh, &reg()?, num(s, "amount").unwrap_or(5.0), num(s, "size").unwrap_or(40.0), s.get("octaves").and_then(Value::as_u64).unwrap_or(4) as usize, s.get("seed").and_then(Value::as_u64).unwrap_or(1) as u32);
                "displaced".into()
            }
            "move" | "transform" => {
                let pivot = field_v3(s, "pivot")?.unwrap_or([0.0; 3]);
                let scale_by = match s.get("scale") {
                    Some(v) if v.is_number() => [v.as_f64().unwrap_or(1.0) as f32; 3],
                    Some(v) => v3(v).ok_or("\"scale\" is a number or [x, y, z]")?,
                    None => [1.0; 3],
                };
                ops::transform(mesh, &reg()?, field_v3(s, "by")?.unwrap_or([0.0; 3]), field_v3(s, "turn")?.unwrap_or([0.0; 3]), scale_by, pivot);
                "moved".into()
            }
            "fit" => {
                ops::centre_and_fit(mesh, num(s, "size").unwrap_or(0.0));
                "centred".into()
            }
            "twist" | "bend" | "taper" | "warp" | "repeat" => super::math_shapes::warp_step(mesh, what, s)?,
            "render" => super::math_shapes::render_step(mesh, mc, s)?,
            "add" => {
                let mut part = match s.get("math") {
                    // A shape from mathematics, `size` px.
                    Some(m) => super::math_shapes::make(m, super::math_shapes::fit_of(s, 100.0)?, s.get("detail").and_then(Value::as_u64).unwrap_or(3).clamp(1, 6) as usize)?.mesh,
                    None => primitive(s, 100.0)?,
                };
                if let Some(mat) = surface_of(s, true)? {
                    part.materials = vec![mat];
                } else {
                    // Same material as the first, unless told otherwise.
                    part.materials = vec![mesh.materials.first().cloned().unwrap_or_default()];
                }
                if let Some(r) = field_v3(s, "turn")? {
                    ops::transform(&mut part, &Region::All, [0.0; 3], r, [1.0; 3], [0.0; 3]);
                }
                let at = field_v3(s, "at")?.unwrap_or([0.0; 3]);
                let mut twin = part.clone();
                ops::transform(&mut part, &Region::All, at, [0.0; 3], [1.0; 3], [0.0; 3]);
                ops::merge(mesh, &part);
                // Symmetric like the sculpt brushes: a pair when off-centre.
                let pair = s.get("symmetry").and_then(Value::as_bool).unwrap_or(true) && at[0].abs() > 0.5;
                if pair {
                    ops::transform(&mut twin, &Region::All, [0.0; 3], [0.0; 3], [-1.0, 1.0, 1.0], [0.0; 3]);
                    ops::transform(&mut twin, &Region::All, [-at[0], at[1], at[2]], [0.0; 3], [1.0; 3], [0.0; 3]);
                    ops::merge(mesh, &twin);
                }
                format!("{}{} added", if pair { "a pair of " } else { "" }, s.get("shape").and_then(Value::as_str).or(s.get("math").map(|_| "math shape")).unwrap_or("sphere"))
            }
            "boolean" | "union" | "subtract" | "intersect" => {
                let mode = s.get("mode").and_then(Value::as_str).unwrap_or(what);
                let kind = studio_mesh::csg::Op::parse(mode).ok_or("\"mode\" is union, subtract or intersect")?;
                let other = match s.get("with") {
                    Some(Value::Number(n)) => {
                        let oid = n.as_u64().unwrap_or(0) as u32;
                        let (oc, mut om) = self.model_of(oid)?;
                        let d = [(oc.transform.position.x - mc.transform.position.x) as f32, (oc.transform.position.y - mc.transform.position.y) as f32, (oc.transform.position_z - mc.transform.position_z) as f32];
                        ops::transform(&mut om, &Region::All, d, [0.0; 3], [1.0; 3], [0.0; 3]);
                        om
                    }
                    Some(w @ Value::Object(_)) => {
                        let mut part = primitive(w, 100.0)?;
                        if let Some(r) = field_v3(w, "turn")? {
                            ops::transform(&mut part, &Region::All, [0.0; 3], r, [1.0; 3], [0.0; 3]);
                        }
                        ops::transform(&mut part, &Region::All, field_v3(w, "at")?.unwrap_or([0.0; 3]), [0.0; 3], [1.0; 3], [0.0; 3]);
                        part
                    }
                    _ => return Err("\"with\" is a model clip id or a shape {\"shape\", \"size\", \"at\"}".into()),
                };
                if mesh.triangles() + other.triangles() > 120_000 {
                    return Err(format!("booleans work on up to 120000 triangles together ({} here); decimate first", mesh.triangles() + other.triangles()));
                }
                *mesh = studio_mesh::csg::boolean(mesh, &other, kind);
                format!("{mode} done")
            }
            "decimate" | "simplify" => {
                let t = s.get("triangles").and_then(Value::as_u64).ok_or("needs \"triangles\" (how many to keep)")? as usize;
                studio_mesh::decimate::decimate(mesh, t);
                format!("decimated to {}", mesh.triangles())
            }
            "solid" | "solidify" | "make_solid" | "close" | "thicken" | "watertight" => {
                // A real solid: holes closed, open sheets given thickness,
                // every face turned outward.
                use studio_modeling::poly::{NormalMode, Poly};
                let (lo, hi) = mesh.bounds();
                let size = studio_mesh::length(studio_mesh::sub(hi, lo)).max(1.0);
                let thickness = num(s, "thickness").unwrap_or(size * 0.02).max(size * 0.002);
                let before = studio_quality::topo::analyse(mesh);
                let mut p = Poly::from_mesh(mesh);
                let filled = studio_modeling::edit::fill_holes(&mut p, s.get("max_sides").and_then(Value::as_u64).unwrap_or(64) as usize);
                let open = studio_modeling::poly::Topology::of(&p).boundary_edges().len();
                if open > 0 {
                    studio_modeling::modifiers::solidify(&mut p, thickness, 0.0, true);
                }
                p.orient_outward();
                let mut out = p.to_mesh(40.0, NormalMode::Angle);
                if let Some(rig) = &mesh.rig {
                    out.rig = Some(std::sync::Arc::new(rig.transferred(mesh, &out)));
                }
                let after = studio_quality::topo::analyse(&out);
                *mesh = out;
                format!(
                    "made solid: {} open edges before, {filled} holes closed{}, {} open edges now{}",
                    before.boundary_edges,
                    if open > 0 { format!(", open sheets thickened to {thickness:.1} px") } else { String::new() },
                    after.boundary_edges,
                    if after.watertight { ", watertight" } else { "" }
                )
            }
            "normals" | "shading" => {
                ops::recompute_normals(mesh, num(s, "angle").unwrap_or(180.0));
                "normals recomputed".into()
            }
            "material" | "paint" => {
                let wet = wetness_of(s)?;
                let strength = glow_strength(s)?;
                if let Some(w) = wet
                    && let m::ClipSource::Generator(g) = &mut mc.source
                {
                    g.params.insert("wetness".into(), m::Value::Scalar(w));
                }
                // Only wetness, or only a stronger glow on the glow it has.
                if surface_of(s, true)?.is_none() {
                    let mut said = Vec::new();
                    if let Some(w) = wet {
                        said.push(format!("wetness {w} (the whole model: darker, glossy, reflecting lights and the scene; puddles on what faces up)"));
                    }
                    if let Some(k) = strength {
                        let which: Vec<usize> = match s.get("index").and_then(Value::as_u64) {
                            Some(i) => vec![i as usize],
                            None => (0..mesh.materials.len()).collect(),
                        };
                        let mut n = 0;
                        for i in which {
                            if let Some(m) = mesh.materials.get_mut(i).filter(|m| m.emissive.iter().any(|c| *c > 0.0)) {
                                m.emissive = m.emissive.map(|c| c * k);
                                n += 1;
                            }
                        }
                        if n == 0 {
                            return Err("emissive_strength scales a glow, and no material here glows: set emissive (the glow colour) with it, for example {\"do\": \"material\", \"emissive\": \"#ff9a3c\", \"emissive_strength\": 6}".into());
                        }
                        said.push(format!("glow x{k} on {n} material{}", if n == 1 { "" } else { "s" }));
                    }
                    if said.is_empty() {
                        return Err("give colour, metallic, roughness, emissive, emissive_strength, wetness or opacity".into());
                    }
                    return Ok(said.join("; "));
                }
                let mat = surface_of(s, true)?.ok_or("give colour, metallic, roughness, emissive or opacity")?;
                if s.get("region").is_some() {
                    // A new material on part of the model (lips, eyes).
                    let base = mesh.materials.first().cloned().unwrap_or_default();
                    let mut m2 = mat.clone();
                    if s.get("colour").is_none() {
                        m2.base_colour = base.base_colour;
                    }
                    let n = ops::assign_material(mesh, &reg()?, Some(m2), None);
                    if n == 0 {
                        return Err("the region holds no faces".into());
                    }
                    format!("{n} faces got material {}", mesh.materials.len() - 1)
                } else {
                    let index = s.get("index").or_else(|| s.get("slot")).and_then(Value::as_u64);
                    let which: Vec<usize> = match index {
                        Some(i) => vec![i as usize],
                        None => (0..mesh.materials.len()).collect(),
                    };
                    // A name belongs to one slot: given for every slot it
                    // turned 49 parts into "door knob", and `check` then
                    // found the parts missing.
                    let rename = s.get("name").is_some() && which.len() == 1;
                    let mut note = String::new();
                    if s.get("name").is_some() && !rename {
                        note = format!(" (name not applied: it names one slot, so give \"index\" too; slots: {})", mesh.materials.iter().enumerate().map(|(i, m)| format!("{i} {}", if m.name.is_empty() { "unnamed" } else { &m.name })).collect::<Vec<_>>().join(", "));
                    }
                    for i in which {
                        if let Some(m) = mesh.materials.get_mut(i) {
                            if s.get("colour").is_some() {
                                m.base_colour = mat.base_colour;
                            }
                            if s.get("opacity").is_some() {
                                m.base_colour[3] = mat.base_colour[3];
                            }
                            if s.get("metallic").is_some() {
                                m.metallic = mat.metallic;
                            }
                            if s.get("roughness").is_some() {
                                m.roughness = mat.roughness;
                            }
                            if s.get("emissive").is_some() {
                                m.emissive = mat.emissive;
                            }
                            if s.get("subsurface").is_some() {
                                m.subsurface = mat.subsurface;
                            }
                            if s.get("transmission").is_some() {
                                m.transmission = mat.transmission;
                            }
                            if s.get("ior").is_some() {
                                m.ior = mat.ior;
                            }
                            if rename {
                                m.name = mat.name.clone();
                            }
                        }
                    }
                    format!("material changed{note}")
                }
            }
            "texture" => {
                let path = s.get("image").and_then(Value::as_str).ok_or("needs \"image\" (a PNG or JPEG path)")?;
                let tex = read_picture(path)?;
                if let Some(p) = s.get("projection").and_then(Value::as_str) {
                    ops::project_uvs(mesh, p)?;
                }
                if let Some(f) = s.get("flip").and_then(Value::as_str) {
                    flip_uvs(mesh, f)?;
                }
                let which: Vec<usize> = match s.get("index").and_then(Value::as_u64) {
                    Some(i) => vec![i as usize],
                    None => (0..mesh.materials.len()).collect(),
                };
                for i in which {
                    if let Some(m) = mesh.materials.get_mut(i) {
                        m.texture = Some(tex.clone());
                        m.base_colour = [1.0, 1.0, 1.0, m.base_colour[3]];
                    }
                }
                format!("textured with {path}")
            }
            "uv" | "unwrap" => match (s.get("projection").and_then(Value::as_str), s.get("flip").and_then(Value::as_str)) {
                (None, Some(f)) => {
                    flip_uvs(mesh, f)?;
                    format!("texture flipped ({f})")
                }
                (p, f) => {
                    ops::project_uvs(mesh, p.unwrap_or("box"))?;
                    if let Some(f) = f {
                        flip_uvs(mesh, f)?;
                    }
                    "texture coordinates projected".into()
                }
            },
            "delete" => {
                let n = ops::delete(mesh, &reg()?);
                format!("{n} faces deleted")
            }
            "array" => {
                ops::array(mesh, s.get("count").and_then(Value::as_u64).unwrap_or(2) as usize, field_v3(s, "offset")?.ok_or("needs \"offset\": [x, y, z]")?);
                "copies added".into()
            }
            other => return Err(format!("unknown step \"{other}\"; steps are {}", STEPS.join(", "))),
        })
    }
}

pub const STEPS: &[&str] = &["subdivide", "bevel", "smooth", "sculpt", "stroke", "symmetrize", "mirror", "extrude", "displace", "move", "fit", "add", "boolean", "decimate", "normals", "material", "texture", "uv", "delete", "array", "twist", "bend", "taper", "warp", "repeat", "render"];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services;

    /// Zornov's own editable source, `.smodel`, loads as a model like any
    /// other model file.
    #[test]
    fn an_smodel_file_loads_as_a_model() {
        let dir = std::env::temp_dir().join(format!("studio-smodel-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut a = studio_modeling::asset::Asset::new("Box");
        a.parts.push(studio_modeling::asset::Part::new("body", studio_modeling::prim::cube([100.0, 60.0, 40.0], 1)));
        let path = dir.join("box.smodel");
        studio_modeling::asset::save(&a, &path).unwrap();
        let mut s = Session::new(services::project::new_custom("Models", 640, 360, 30.0));
        let r = s.edit(&[json!({"op": "model", "file": path.to_string_lossy(), "size": 200})], "add", false);
        assert!(r.is_ok(), "{r:?}");
        let id = s.doc.project.clips.iter().map(|c| c.id).max().unwrap();
        let (_, mesh) = s.model_of(id).unwrap();
        assert_eq!(mesh.triangles(), 12, "a box is 12 triangles");
        let (lo, hi) = mesh.bounds();
        let longest = (0..3).map(|k| hi[k] - lo[k]).fold(0.0f32, f32::max);
        assert!((longest - 200.0).abs() < 1.0, "fitted to size: {longest}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// USD text (.usda, and a .usdz holding one) loads as a model through
    /// the same path as any model file, by hand and by the agent.
    #[test]
    fn a_usd_file_loads_as_a_model() {
        let dir = std::env::temp_dir().join(format!("studio-usd-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut a = studio_modeling::asset::Asset::new("Box");
        a.parts.push(studio_modeling::asset::Part::new("body", studio_modeling::prim::cube([100.0, 60.0, 40.0], 1)));
        let usda = dir.join("box.usda");
        let usdz = dir.join("box.usdz");
        let mesh = a.to_mesh(false).unwrap();
        studio_modeling::formats::write(&mesh, &a, &usda).unwrap();
        studio_modeling::formats::write(&mesh, &a, &usdz).unwrap();
        for path in [&usda, &usdz] {
            assert!(crate::mesh_store::is_model_file(path), "{} is a model file", path.display());
            let got = crate::mesh_store::read_model(path, 200.0).unwrap();
            assert_eq!(got.mesh.triangles(), 12, "{}: a box is 12 triangles", path.display());
            let mut s = Session::new(services::project::new_custom("Models", 640, 360, 30.0));
            let r = s.edit(&[json!({"op": "model", "file": path.to_string_lossy(), "size": 200})], "add", false);
            assert!(r.is_ok(), "{r:?}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A model exports to USD (.usdz, .usda), FBX, STL and PLY as well as
    /// glTF and OBJ, and what is written reads back.
    #[test]
    fn a_model_exports_to_usd_fbx_stl_and_ply() {
        let dir = std::env::temp_dir().join(format!("studio-model-export-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut s = Session::new(services::project::new_custom("Models", 640, 360, 30.0));
        s.edit(&[json!({"op": "model", "shape": "cube", "size": 200})], "add", false).unwrap();
        let id = s.doc.project.clips.iter().map(|c| c.id).max().unwrap();
        for ext in ["usdz", "usda", "fbx", "stl", "ply", "glb", "obj"] {
            let out = dir.join(format!("cube.{ext}"));
            let r = s.edit(&[json!({"op": "model", "clip": id, "export": out.to_string_lossy()})], "export", false);
            assert!(r.is_ok(), "{ext}: {r:?}");
            assert!(out.is_file(), "{ext}: nothing written");
            let back = studio_modeling::formats::import(&out, 0.0).unwrap_or_else(|e| panic!("{ext}: {e}"));
            assert!(back.iter().map(|(_, p)| p.faces()).sum::<usize>() >= 6, "{ext}: the cube's faces come back");
        }
        assert!(crate::mesh_store::MODEL_EXPORTS.contains(&"usdz"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn add_sculpt_and_undo_a_model() {
        let mut s = Session::new(services::project::new_custom("Models", 640, 360, 30.0));
        s.edit(&[json!({"op": "model", "shape": "sphere", "size": 200, "detail": 4, "colour": "#d9a38a", "roughness": 0.6})], "add", false).unwrap();
        let id = s.doc.project.clips.iter().map(|c| c.id).max().unwrap();
        let (_, before) = s.model_of(id).unwrap();
        let r = s
            .edit(
                &[json!({"op": "mesh", "clip": id, "steps": [
                    {"do": "subdivide"},
                    {"do": "sculpt", "brush": "grab", "at": [0, 0, -100], "by": [0, 0, -40], "radius": 50},
                    {"do": "material", "region": {"sphere": {"at": [40, -20, -90], "radius": 20}}, "colour": "#202020", "roughness": 0.1},
                ]})],
                "sculpt",
                false,
            )
            .unwrap();
        assert!(r.to_string().contains("faces got material"), "{r}");
        let (_, after) = s.model_of(id).unwrap();
        assert_eq!(after.triangles(), before.triangles() * 4);
        assert!(after.bounds().0[2] < before.bounds().0[2] - 30.0, "nose pulled out");
        assert!(after.materials.len() == 2);
        // Undo goes back to the earlier file.
        s.doc.undo();
        let (_, back) = s.model_of(id).unwrap();
        assert_eq!(back.triangles(), before.triangles());
    }
}
