// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `wear` and `hold`: dressing a rigged figure and putting things in its
//! hands. They work on any character, from any body the `model` op builds to
//! a rigged model of your own.
//!
//! * `wear`: clothing, hats, shoes, gloves, glasses, scarves, belts, bags and
//!   armour made to fit the body (see `studio_mesh::wear`): real meshes in
//!   fabric materials, skinned from the body's own weights so they follow
//!   it, hard things rigid on one bone. `{"op": "wear", "clip": 3, "items":
//!   [{"kind": "coat", "style": "trench", "colour": "#b49a6a"}, {"kind":
//!   "hat", "style": "fedora"}]}`; `remove`, `undress` and `list` take them
//!   off and say what is worn. Loose parts can be left to the cloth
//!   simulation (`loose: true`).
//! * `hold`: any prop (a kit prop by name, or any model clip) in a hand: it
//!   rides on the hand, the fingers close round it by how thick it is, and a
//!   flame (a general fire element: emissive layers and a flickering point
//!   light that follows the hand) can burn on it.

use super::*;
use studio_mesh::hand::{self, Frame, Grip};
use studio_mesh::prop;
use studio_mesh::rig::Xf;
use studio_mesh::wear::{self, Skin, Spec};
use studio_mesh::{Mesh, V3, add};

/// A colour from a word, "#rrggbb" or [r, g, b] (0-255).
fn rgb(v: &Value) -> Result<[u8; 3], String> {
    if let Some(a) = v.as_array() {
        let c: Vec<u8> = a.iter().filter_map(Value::as_f64).map(|x| x.clamp(0.0, 255.0) as u8).collect();
        if c.len() >= 3 {
            return Ok([c[0], c[1], c[2]]);
        }
    }
    let s = v.as_str().ok_or("a colour is \"#rrggbb\", a basic name or [r, g, b]")?;
    super::craft::colour(s).map(|c| [c.r(), c.g(), c.b()]).ok_or_else(|| format!("colour \"{s}\" is not #rrggbb or a basic name"))
}

fn opt_rgb(item: &Value, keys: &[&str]) -> Result<Option<[u8; 3]>, String> {
    for k in keys {
        if let Some(v) = item.get(*k).filter(|v| !v.is_null()) {
            return rgb(v).map(Some).map_err(|e| format!("{k}: {e}"));
        }
    }
    Ok(None)
}

fn num(v: &Value, k: &str) -> Option<f32> {
    v.get(k).and_then(Value::as_f64).map(|x| x as f32)
}

fn point(v: &Value) -> Option<V3> {
    let a = v.as_array()?;
    (a.len() == 3).then(|| [a[0].as_f64().unwrap_or(0.0) as f32, a[1].as_f64().unwrap_or(0.0) as f32, a[2].as_f64().unwrap_or(0.0) as f32])
}

/// A garment spec from an item of a `wear` op.
fn spec_of(item: &Value) -> Result<Spec, String> {
    let kind = ["kind", "item", "type", "what", "wear", "garment"].iter().find_map(|k| item.get(*k).and_then(Value::as_str)).ok_or_else(|| format!("each item needs \"kind\": {}", wear::KINDS))?;
    let mut s = Spec { kind: kind.to_string(), ..Spec::default() };
    if let Some(v) = item.get("style").and_then(Value::as_str) {
        s.style = v.to_string();
    }
    s.colour = opt_rgb(item, &["colour", "color"])?;
    s.colour2 = opt_rgb(item, &["colour2", "color2", "trim", "trim_colour", "second_colour", "lining"])?;
    s.material = ["material", "fabric", "cloth"].iter().find_map(|k| item.get(*k).and_then(Value::as_str)).map(str::to_string);
    if let Some(m) = &s.material
        && studio_mesh::fabric::material(m, None, None, studio_mesh::fabric::Pattern::None, 0).is_none()
    {
        return Err(format!("material \"{m}\": {}", studio_mesh::fabric::NAMES));
    }
    if let Some(p) = item.get("pattern").and_then(Value::as_str) {
        s.pattern = studio_mesh::fabric::Pattern::parse(p).ok_or_else(|| format!("pattern \"{p}\": plain, stripes, pinstripe, check, plaid, dots"))?;
    }
    s.fit = match item.get("fit") {
        Some(Value::String(w)) => match w.to_lowercase().as_str() {
            "tight" | "skin" | "skintight" | "snug" => 0.5,
            "fitted" | "slim" => 0.8,
            "regular" | "normal" | "standard" => 1.0,
            "relaxed" | "loose" => 1.5,
            "baggy" | "oversized" | "wide" | "voluminous" => 2.2,
            other => return Err(format!("fit \"{other}\": tight, fitted, regular, loose, baggy (or a number: 1 is usual)")),
        },
        Some(v) => v.as_f64().unwrap_or(1.0) as f32,
        None => 1.0,
    };
    s.length = item.get("length").map(|v| v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string()));
    s.sleeves = item.get("sleeves").and_then(Value::as_str).map(str::to_string);
    s.scale = num(item, "scale").or_else(|| num(item, "size")).unwrap_or(1.0);
    s.open = item.get("open").and_then(Value::as_bool);
    s.skin = match item.get("skin").and_then(Value::as_str) {
        None => Skin::Auto,
        Some(k) => match k.to_lowercase().as_str() {
            "smooth" | "soft" | "body" | "auto" => Skin::Smooth,
            "rigid" | "hard" | "bone" | "piece" => Skin::Rigid,
            "stiff" | "plate" => Skin::Stiff,
            other => return Err(format!("skin \"{other}\": smooth (bends with the body), rigid (whole on one bone, for hard things) or stiff (nearly rigid)")),
        },
    };
    s.name = item.get("name").and_then(Value::as_str).map(str::to_string);
    s.detail = num(item, "detail").unwrap_or(1.0).clamp(0.2, 3.0);
    s.trim = item.get("trim").and_then(Value::as_bool).unwrap_or(true);
    s.side = item.get("side").or_else(|| item.get("hand")).and_then(Value::as_str).and_then(|x| match x.to_lowercase().as_str() {
        "l" | "left" => Some('L'),
        "r" | "right" => Some('R'),
        _ => None,
    });
    s.loose = ["loose", "physics", "simulate", "cloth"].iter().any(|k| item.get(*k).and_then(Value::as_bool) == Some(true));
    Ok(s)
}

/// Hair, a beard, a moustache or brows from an item.
fn hair_spec_of(item: &Value, kind: &str) -> Result<studio_modeling::hair::HairSpec, String> {
    let mut h = studio_modeling::hair::HairSpec { kind: kind.to_string(), ..Default::default() };
    if let Some(v) = item.get("style").and_then(Value::as_str) {
        h.style = v.to_string();
    }
    h.colour = opt_rgb(item, &["colour", "color"])?;
    h.melanin = num(item, "melanin").unwrap_or(h.melanin);
    h.redness = num(item, "redness").unwrap_or(h.redness);
    h.seed = item.get("seed").and_then(Value::as_u64).unwrap_or(1);
    h.density = num(item, "density").unwrap_or(1.0);
    h.length = num(item, "length").unwrap_or(1.0);
    h.name = item.get("name").and_then(Value::as_str).map(str::to_string);
    Ok(h)
}

/// The cloth fabric a material is simulated as.
fn sim_fabric(material: &str) -> &'static str {
    match material.to_lowercase().as_str() {
        "silk" | "satin" | "nylon" | "linen" => "silk",
        "denim" | "wool" | "tweed" | "flannel" | "knit" => "denim",
        "leather" | "suede" => "leather",
        "canvas" => "canvas",
        _ => "cotton",
    }
}

impl Session {
    /// The model clip an op names (`clip`: id or name).
    fn figure_of(&self, op: &Value, what: &str) -> Result<u32, String> {
        let v = op.get("clip").ok_or_else(|| format!("{what} needs \"clip\": the model clip of the figure (id or name)"))?;
        super::sim3d::id_of(&self.doc, v).ok_or_else(|| format!("no clip \"{v}\""))
    }

    fn put_model(&mut self, mut mc: m::Clip, mesh: &Mesh, what: &str) -> Result<(), String> {
        let file = crate::mesh_store::save(&self.doc, mesh)?;
        crate::mesh_store::set_file(&mut mc, file);
        let seq = self.doc.seq();
        let ok = self.doc.apply(what, m::Edit::SetClip { seq, clip: Box::new(mc) });
        self.check(ok, "").map(|_| ())
    }

    /// `wear`: put clothes on a figure, take them off, say what is worn.
    pub(super) fn wear(&mut self, op: &Value) -> Result<String, String> {
        let id = self.figure_of(op, "wear")?;
        let (mc, mut mesh) = self.model_of(id)?;
        if op.get("list").and_then(Value::as_bool) == Some(true) || op.as_object().is_some_and(|o| o.keys().filter(|k| !super::args::is_note(k) && k.as_str() != "clip").count() == 0) {
            let worn = wear::worn(&mesh);
            return Ok(format!("clip {id} wears: {}; kinds: {}; fabrics: {}; patterns: plain, stripes, pinstripe, check, plaid, dots", if worn.is_empty() { "nothing but its own skin".into() } else { worn.join(", ") }, wear::KINDS, studio_mesh::fabric::NAMES));
        }
        let mut said: Vec<String> = Vec::new();
        // Off first: by name, or everything.
        let mut off: Vec<String> = match op.get("remove") {
            Some(Value::String(s)) => vec![s.clone()],
            Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).map(str::to_string).collect(),
            _ => Vec::new(),
        };
        if op.get("undress").and_then(Value::as_bool) == Some(true) {
            off.extend(wear::worn(&mesh));
        }
        for name in &off {
            match wear::remove(&mut mesh, name) {
                Ok(n) => said.push(format!("took off {name} ({n} triangles)")),
                Err(e) if op.get("undress").and_then(Value::as_bool) == Some(true) => {
                    let _ = e;
                }
                Err(e) => return Err(e),
            }
        }
        let items: Vec<Value> = match op.get("items") {
            Some(Value::Array(a)) => a.clone(),
            Some(_) => return Err("\"items\" is a list of {\"kind\": …} items".into()),
            None if ["kind", "item", "type", "what", "wear", "garment"].iter().any(|k| op.get(*k).is_some()) => vec![op.clone()],
            None => Vec::new(),
        };
        let mut loose: Vec<(String, Mesh, String)> = Vec::new();
        for (i, item) in items.iter().enumerate() {
            let kind_word = ["kind", "item", "type", "what", "wear", "garment"].iter().find_map(|k| item.get(*k).and_then(Value::as_str)).unwrap_or("");
            if studio_modeling::hair::is_hair(kind_word) {
                let hs = hair_spec_of(item, kind_word)?;
                let g = studio_modeling::hair::fit(&mesh, &hs).map_err(|e| format!("item {} ({kind_word}): {e}", i + 1))?;
                if wear::worn(&mesh).iter().any(|n| n.eq_ignore_ascii_case(&g.name)) {
                    let _ = wear::remove(&mut mesh, &g.name);
                }
                wear::attach(&mut mesh, &g)?;
                said.push(format!("{} ({}): {} triangles{}", g.name, g.kind, g.mesh.triangles(), g.notes.first().map(|n| format!("; {n}")).unwrap_or_default()));
                continue;
            }
            let spec = spec_of(item).map_err(|e| format!("item {}: {e}", i + 1))?;
            let garments = wear::fit(&mesh, &spec).map_err(|e| format!("item {} ({}): {e}", i + 1, spec.kind))?;
            for g in garments {
                // The same thing again replaces it.
                if wear::worn(&mesh).iter().any(|n| n.eq_ignore_ascii_case(&g.name)) {
                    let _ = wear::remove(&mut mesh, &g.name);
                }
                let range = wear::attach(&mut mesh, &g)?;
                let mut line = format!("{} ({}): {} triangles", g.name, g.kind, g.mesh.triangles());
                for n in &g.notes {
                    line.push_str(&format!("; {n}"));
                }
                if let Some(l) = &g.loose {
                    let main = g.mesh.materials.first().cloned().unwrap_or_default();
                    loose.push((g.name.clone(), l.clone(), main.name.clone()));
                    // The tails take the main material.
                    let mut tails = l.clone();
                    tails.materials = vec![main];
                    tails.single_group();
                    loose.last_mut().expect("just pushed").1 = tails;
                }
                let _ = range;
                said.push(line);
                strip_base(&mut mesh, &g.kind);
            }
        }
        if items.is_empty() && off.is_empty() {
            return Err(format!("wear: give \"items\" ([{{\"kind\": \"coat\", \"style\": \"trench\", \"colour\": \"#b49a6a\"}}]), \"remove\", \"undress\": true or \"list\": true; kinds: {}", wear::KINDS));
        }
        mesh.tidy();
        if !mesh.rig_in_step() {
            return Err("internal: the rig fell out of step with the clothes".into());
        }
        self.put_model(mc.clone(), &mesh, "Wear")?;
        // Loose parts become cloth that is simulated on the figure.
        for (name, tails, material) in loose {
            let (mc, _) = self.model_of(id)?;
            let dur = (mc.duration as f64 / 1e6).clamp(0.5, 60.0);
            let cid = self.add_model_clip(&tails, &name, mc.start, mc.duration, Some(&mc))?;
            let r = self.simulate(&json!({"kind": "cloth", "clip": cid, "on": id, "pin": "top", "duration": dur, "fabric": sim_fabric(&material)}));
            match r {
                Ok(s) => said.push(format!("{name} (loose part) is clip {cid}, simulated as cloth: {s}")),
                Err(e) => said.push(format!("{name} (loose part) is clip {cid}; the simulation did not run: {e}")),
            }
        }
        Ok(format!("clip {id}: {}", said.join("; ")))
    }
}

/// Plain clothes painted on at birth go when a real garment covers their
/// place: that material turns to skin, so a skirt over painted trousers
/// shows legs.
fn strip_base(mesh: &mut Mesh, kind: &str) {
    let zones: &[&str] = match kind {
        "shirt" | "jacket" | "coat" => &["Top"],
        "trousers" | "skirt" => &["Trousers"],
        "dress" => &["Top", "Trousers"],
        "shoes" => &["Shoes"],
        _ => &[],
    };
    let Some(skin) = mesh.materials.iter().find(|m| m.name == "Skin").cloned() else { return };
    for m in &mut mesh.materials {
        if zones.contains(&m.name.as_str()) {
            m.base_colour = skin.base_colour;
            m.roughness = skin.roughness;
            m.subsurface = skin.subsurface;
        }
    }
}

// ------------------------------------------------------------------ hold

/// Kit props' sizes as a fraction of the figure's height.
fn kit_size(name: &str, h: f32) -> f32 {
    let f = match name {
        "cup" | "mug" | "coffee" | "teacup" => 0.075,
        "glass" | "tumbler" | "wine_glass" => 0.09,
        "bottle" | "flask" => 0.17,
        "phone" | "smartphone" | "tablet" | "device" => 0.085,
        "book" | "notebook" | "journal" => 0.12,
        "pistol" | "gun" | "handgun" | "revolver" => 0.1,
        "sword" | "sabre" | "saber" | "blade" => 0.5,
        "knife" | "dagger" => 0.16,
        "torch" | "brand" => 0.3,
        "lantern" | "lamp" => 0.15,
        "candle" | "taper" => 0.09,
        "lighter" | "zippo" => 0.038,
        "briefcase" | "case" | "suitcase" | "attache" => 0.2,
        "handbag" | "bag" | "purse" | "pouch" => 0.14,
        "umbrella" | "parasol" => 0.5,
        "ball" | "orb" | "apple" | "sphere" | "globe" => 0.08,
        "tray" | "plate" | "dish" | "platter" => 0.2,
        "cigarette" | "cigar" => 0.045,
        "pencil" | "pen" => 0.08,
        "chopstick" | "straw" => 0.13,
        _ => 0.16,
    };
    f * h
}

fn key_f(c: &mut m::Clip, path: &str, t: i64, v: f64) {
    if let Some(name) = path.strip_prefix("generator.")
        && let m::ClipSource::Generator(g) = &mut c.source
        && !g.params.get(name).is_some_and(|p| p.same_kind(&m::Value::Angle(0.0)))
    {
        g.params.insert(name.to_string(), m::Value::Angle(0.0));
    }
    let k = m::Keyframe::new(t, m::Value::Angle(v), m::Interp::Ease);
    match c.keyframes.get_mut(path) {
        Some(tr) => tr.set_key(k),
        None => {
            c.keyframes.insert(path.to_string(), m::Track::new(vec![k]));
        }
    }
}

impl Session {
    /// `hold`: put a prop in a hand, close the fingers round it, light it.
    pub(super) fn hold(&mut self, op: &Value) -> Result<String, String> {
        let id = self.figure_of(op, "hold")?;
        let (mut mc, mut mesh) = self.model_of(id)?;
        let rig = mesh.rig.clone().ok_or("this model has no rig; hold needs a figure with hands (model shape \"character\")")?;
        let side = match op.get("hand").and_then(Value::as_str).unwrap_or("right").to_lowercase().as_str() {
            "r" | "right" => 'R',
            "l" | "left" => 'L',
            other => return Err(format!("hand \"{other}\": left or right")),
        };
        let left = side == 'L';
        let hand_b = rig.resolve_bone(&format!("hand.{side}")).map_err(|e| format!("{e} (hold needs a hand bone)"))?;
        let at = f(op, "at").unwrap_or(0.0);
        // Letting go: the prop comes off, the fingers open.
        if op.get("release").and_then(Value::as_bool) == Some(true) || op.get("drop").and_then(Value::as_bool) == Some(true) {
            for (bone, _) in hand::curl_pose(&rig, left, [0.0; 5]) {
                for c in ["x", "y", "z"] {
                    let path = format!("generator.bone.{bone}.{c}");
                    if mc.keyframes.contains_key(&path) {
                        key_f(&mut mc, &path, micros(at), 0.0);
                    }
                }
            }
            let name = op.get("name").and_then(Value::as_str).map(str::to_string);
            let removed = match name {
                Some(n) => wear::remove(&mut mesh, &n).map(|k| format!("; {n} removed ({k} triangles)")).unwrap_or_default(),
                None => String::new(),
            };
            self.put_model(mc, &mesh, "Let go")?;
            return Ok(format!("clip {id}: the {side} hand opens at {at:.2} s{removed}"));
        }
        let (lo, hi) = mesh.bounds();
        let figure = hi[1] - lo[1];
        // The prop: an item object, a kit name, or a model clip.
        let item = op.get("item").or_else(|| op.get("prop")).or_else(|| op.get("kit")).cloned().unwrap_or_else(|| op.clone());
        let spec = if item.is_object() { item.clone() } else { json!({}) };
        let pick = |k: &str| spec.get(k).or_else(|| op.get(k)).cloned();
        let kit_name = match &item {
            Value::String(s) => Some(s.clone()),
            Value::Object(_) => ["kit", "name", "prop", "item"].iter().find_map(|k| item.get(*k).and_then(Value::as_str)).map(str::to_string),
            _ => None,
        };
        let held_clip = match &item {
            Value::Number(n) => n.as_u64().map(|x| x as u32),
            Value::Object(_) => item.get("clip").and_then(|v| super::sim3d::id_of(&self.doc, v)),
            _ => None,
        };
        let colour = pick("colour").or_else(|| pick("color")).map(|v| rgb(&v)).transpose()?;
        let tip = pick("tip").map(|v| rgb(&v)).transpose()?;
        let material = pick("material").and_then(|v| v.as_str().map(str::to_string));
        let size = pick("size").and_then(|v| v.as_f64()).map(|x| x as f32);
        let (prop_mesh, name, natural_grip, thickness, spot, natural_lit) = if let Some(cid) = held_clip {
            let (_, mut pm) = self.model_of(cid)?;
            pm.rig = None;
            let (plo, phi) = pm.bounds();
            let thick = (phi[0] - plo[0]).min(phi[2] - plo[2]).max(1.0);
            let n = self.model_clip(cid).map(|c| c.name).unwrap_or_else(|| "Prop".into());
            // The standalone clip is now in the hand.
            let seq = self.doc.seq();
            let ok = self.doc.apply("Take prop in hand", m::Edit::RemoveClip { seq, clip: m::ClipId(cid as u64) });
            self.check(ok, "")?;
            (pm, n, Grip::Handle, thick, None, false)
        } else {
            let kn = kit_name.clone().ok_or_else(|| format!("hold needs what to hold: \"item\": a kit prop ({}) or {{\"kit\": …, \"size\", \"colour\"}}, or a model clip id", prop::NAMES))?;
            let canon = kn.trim().to_ascii_lowercase().replace([' ', '-'], "_");
            let sz = size.unwrap_or_else(|| kit_size(&canon, figure));
            let p = prop::kit(&kn, sz, colour, material.as_deref(), tip)?;
            let lit = matches!(canon.as_str(), "candle" | "taper" | "torch" | "brand" | "lantern" | "lamp");
            (p.mesh, p.name, p.grip, p.thickness, p.flame, lit)
        };
        // How it sits: the grip kind, the hand's own axes.
        let grip = match pick("grip").and_then(|v| v.as_str().map(str::to_string)) {
            Some(g) => Grip::parse(&g).ok_or_else(|| format!("grip \"{g}\": {}", Grip::NAMES))?,
            None => natural_grip,
        };
        let hb = &rig.bones[hand_b];
        let mid_x = rig.bone("hips").map_or(0.0, |i| rig.bones[i].head[0]);
        let len = studio_mesh::length(studio_mesh::sub(hb.tail, hb.head)).max(1.0);
        let frame = Frame::hanging(hb.head, studio_mesh::sub(hb.tail, hb.head), len, left, mid_x);
        let thumb = frame.dir_to_model([0.0, 0.0, 1.0]);
        let grip_bone = rig.bone(&format!("grip.{side}"));
        let anchor = grip_bone.map_or_else(|| frame.to_model(hand::grip_point(len)), |g| rig.bones[g].head);
        let mut q = grip.orientation(frame.along, frame.palm, thumb);
        if let Some(t) = pick("turn").as_ref().and_then(point) {
            q = studio_mesh::rig::q_mul(q, studio_mesh::rig::q_from_euler(t));
        }
        let offset = pick("offset").as_ref().and_then(point).unwrap_or([0.0; 3]);
        let xf = Xf::new(q, add(anchor, offset));
        let mut placed = prop_mesh.clone();
        for p in &mut placed.positions {
            *p = xf.apply(*p);
        }
        for n in &mut placed.normals {
            *n = xf.apply_dir(*n);
        }
        // Where a flame burns, in the model's frame.
        let flame_on = match pick("flame") {
            Some(Value::Bool(b)) => b,
            Some(Value::Object(_)) => true,
            _ => natural_lit,
        };
        let flame_spec = pick("flame").filter(Value::is_object).unwrap_or_else(|| json!({}));
        let mut flame_world: Option<V3> = None;
        let mut flame_height = 0.0;
        if flame_on {
            let (local, hgt) = match (flame_spec.get("at").and_then(point), spot) {
                (Some(a), s) => (a, num(&flame_spec, "size").or(s.map(|s| s.height)).unwrap_or(figure * 0.04)),
                (None, Some(s)) => (s.at, num(&flame_spec, "size").unwrap_or(s.height)),
                (None, None) => return Err("this prop has no flame spot: give \"flame\": {\"at\": [x, y, z] in the prop's own frame (y down), \"size\": px}".into()),
            };
            let colour = opt_rgb(&flame_spec, &["colour", "color"])?;
            let fl = prop::flame(hgt, colour);
            let base = xf.apply(local);
            let mut fp = fl.clone();
            // The flame burns upward whatever way the prop is held (up is -y).
            for p in &mut fp.positions {
                *p = add(*p, base);
            }
            let base_mat = placed.materials.len() as u32;
            let mut mats = placed.tri_materials();
            mats.extend(fl.tri_materials().into_iter().map(|k| k + base_mat));
            placed.materials.extend(fl.materials.iter().cloned());
            let off = placed.positions.len() as u32;
            placed.positions.extend_from_slice(&fp.positions);
            placed.normals.extend_from_slice(&fp.normals);
            placed.uvs.extend_from_slice(&fp.uvs);
            placed.indices.extend(fp.indices.iter().map(|i| i + off));
            placed.regroup(&mats);
            flame_world = Some(add(base, [0.0, -hgt * 0.4, 0.0]));
            flame_height = hgt;
        }
        // The prop joins the figure, whole on the hand.
        let label = op.get("name").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| name.clone());
        for (i, mt) in placed.materials.iter_mut().enumerate() {
            if !mt.name.to_lowercase().starts_with("flame") {
                mt.name = if i == 0 { label.clone() } else { format!("{label} {}", mt.name.to_lowercase()) };
            }
        }
        let first_new = mesh.positions.len();
        let mut rig2 = (*rig).clone();
        let n = placed.positions.len();
        let mut mats = mesh.tri_materials();
        let mat_base = mesh.materials.len() as u32;
        mats.extend(placed.tri_materials().into_iter().map(|k| k + mat_base));
        mesh.materials.extend(placed.materials.iter().cloned());
        let base = mesh.positions.len() as u32;
        mesh.positions.extend_from_slice(&placed.positions);
        mesh.normals.extend_from_slice(&placed.normals);
        mesh.uvs.extend_from_slice(&placed.uvs);
        mesh.indices.extend(placed.indices.iter().map(|i| i + base));
        rig2.joints.extend(std::iter::repeat_n([hand_b as u16, 0, 0, 0], n));
        rig2.weights.extend(std::iter::repeat_n([1.0, 0.0, 0.0, 0.0], n));
        mesh.regroup(&mats);
        mesh.rig = Some(std::sync::Arc::new(rig2));
        let _ = first_new;
        // The fingers close round it.
        let fingers = op.get("fingers").and_then(Value::as_bool).unwrap_or(true);
        let mut closed = 0;
        if fingers {
            let curls = grip.curls(thickness, len);
            for (bone, deg) in hand::curl_pose(&rig, left, curls) {
                for (c, x) in ["x", "y", "z"].iter().zip(deg) {
                    if x != 0.0 || mc.keyframes.contains_key(&format!("generator.bone.{bone}.{c}")) {
                        key_f(&mut mc, &format!("generator.bone.{bone}.{c}"), micros(at), x as f64);
                    }
                }
                closed += 1;
            }
        }
        mesh.tidy();
        self.put_model(mc.clone(), &mesh, "Hold")?;
        let mut said = format!("clip {id}: the {side} hand holds the {label} ({} grip, {} triangles, fingers {})", match grip { Grip::Handle => "handle", Grip::Pinch => "pinch", Grip::Palm => "palm" }, n, if closed > 0 { "closed round it" } else { "left as they are" });
        // A flame lights its surroundings: a point light that follows the hand and flickers.
        if let Some(fw) = flame_world.filter(|_| pick("light").and_then(|v| v.as_bool()).unwrap_or(true)) {
            match self.flame_light(id, hand_b, fw, flame_height, &spec, op) {
                Ok(s) => said.push_str(&format!("; {s}")),
                Err(e) => said.push_str(&format!("; the flame's light was not added: {e}")),
            }
        }
        Ok(said)
    }

    /// A point light at a place on a figure that follows it (the place
    /// carried by `bone`) over the clip, flickering.
    fn flame_light(&mut self, figure: u32, bone: usize, at: V3, height: f32, spec: &Value, op: &Value) -> Result<String, String> {
        #[cfg(not(feature = "render"))]
        {
            let _ = (figure, bone, at, height, spec, op);
            Err("lights that follow a hand need the renderer".into())
        }
        #[cfg(feature = "render")]
        {
            let (mc, mesh) = self.model_of(figure)?;
            let rig = mesh.rig.clone().ok_or("no rig")?;
            let colour = spec.get("light_colour").and_then(Value::as_str).unwrap_or("#ffa857").to_string();
            let intensity = f(spec, "intensity").or_else(|| f(op, "light_intensity")).unwrap_or(75.0);
            let h = self.doc.project.height as f64;
            let radius = f(spec, "radius").unwrap_or(h * 0.18);
            let falloff = f(spec, "falloff").unwrap_or(h * 1.1);
            // Where the place is as the figure moves: the rig posed at sample times, then the clip's own transform.
            let dur = mc.duration;
            let step = (SEC / 8).max(1);
            let rest_bone = rig.bones[bone].clone();
            let _ = rest_bone;
            let mut samples: Vec<(i64, V3)> = Vec::new();
            let mut t = 0i64;
            loop {
                let rel = t.min(dur.max(0));
                let params = studio_render::model_scene::convert_params(&mc.generator_params_at(rel));
                let input = studio_render::model_anim::pose_input(&params, rel as f32 / 1e6);
                let posed = rig.evaluate(&input);
                let local = posed.skin.get(bone).map_or(at, |x| x.apply(at));
                let world = super::sim3d::model_world(&mc.transform_at(rel));
                let w = super::sim3d::apply(&world, local);
                samples.push((rel, w));
                if rel >= dur {
                    break;
                }
                t += step;
            }
            let first = samples[0].1;
            let said = self.light(&json!({"type": "point", "colour": colour, "intensity": intensity, "position": [first[0], first[1], first[2]], "at": mc.start as f64 / 1e6, "duration": dur as f64 / 1e6, "radius": radius, "falloff": falloff, "shadows": false}))?;
            let lid = self.doc.project.clips.iter().map(|c| c.id).max().map(|c| c as u32).ok_or("no light")?;
            let mut lc = self.model_clip(lid).ok_or("the light is gone")?;
            let moves = samples.iter().any(|(_, p)| studio_mesh::length(studio_mesh::sub(*p, first)) > 0.5);
            if moves {
                lc.name = "Flame light".into();
                for (rel, p) in &samples {
                    let tt = *rel;
                    let k = m::Keyframe::new(tt, m::Value::Point(m::Vec2::new(p[0] as f64, p[1] as f64)), m::Interp::Linear);
                    match lc.keyframes.get_mut("transform.position") {
                        Some(tr) => tr.set_key(k),
                        None => {
                            lc.keyframes.insert("transform.position".into(), m::Track::new(vec![k]));
                        }
                    }
                    let kz = m::Keyframe::new(tt, m::Value::Scalar(p[2] as f64), m::Interp::Linear);
                    match lc.keyframes.get_mut("transform.position_z") {
                        Some(tr) => tr.set_key(kz),
                        None => {
                            lc.keyframes.insert("transform.position_z".into(), m::Track::new(vec![kz]));
                        }
                    }
                }
            } else {
                lc.name = "Flame light".into();
            }
            self.apply_clip("Flame light", lc)?;
            // A flicker: a wobble that never repeats, round the light's strength.
            let amount = (intensity * 0.3).max(5.0);
            let _ = self.modifier(&json!({"clip": lid, "property": "intensity", "add": "noise", "amount": amount, "per_second": 11.0, "octaves": 3, "seed": 7}));
            let _ = height;
            Ok(format!("a flickering point light (clip {lid}) follows it{}", if said.is_empty() { "" } else { "" }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services;

    fn figure(s: &mut Session, extra: Value) -> u32 {
        let mut op = json!({"op": "model", "shape": "character", "size": 220, "detail": 1, "duration": 4, "dressed": false});
        for (k, v) in extra.as_object().unwrap() {
            op[k] = v.clone();
        }
        s.edit(&[op], "add", false).unwrap();
        s.doc.project.clips.iter().map(|c| c.id).max().unwrap() as u32
    }

    #[test]
    fn a_figure_is_dressed_and_undressed_by_name() {
        let mut s = Session::new(services::project::new_custom("Wear", 640, 360, 30.0));
        let id = figure(&mut s, json!({"preset": "elder", "style": "stylised"}));
        let r = s.edit(&[json!({"op": "wear", "clip": id, "items": [{"kind": "coat", "style": "long", "colour": "#445566", "material": "tweed", "pattern": "check"}, {"kind": "hat", "style": "beret"}, {"kind": "shoes", "style": "boots"}]})], "dress", false).unwrap();
        let said = r.to_string();
        assert!(said.contains("Long") && said.contains("Beret") && said.contains("Boots"), "{said}");
        let (_, mesh) = s.model_of(id).unwrap();
        assert!(mesh.rig_in_step() && mesh.rig.as_ref().unwrap().is_skinned());
        let worn = wear::worn(&mesh);
        assert!(worn.iter().any(|n| n == "Long") && worn.iter().any(|n| n.starts_with("Beret")), "{worn:?}");
        // The coat has a fabric texture of its own.
        let coat = mesh.materials.iter().find(|m| m.name == "Long").unwrap();
        assert!(coat.texture.is_some());
        // What is worn is listed; one thing comes off.
        let l = s.edit(&[json!({"op": "wear", "clip": id, "list": true})], "list", false).unwrap().to_string();
        assert!(l.contains("Beret") && l.contains("kinds:"), "{l}");
        s.edit(&[json!({"op": "wear", "clip": id, "remove": "beret"})], "off", false).unwrap();
        let (_, mesh) = s.model_of(id).unwrap();
        assert!(!wear::worn(&mesh).iter().any(|n| n.starts_with("Beret")) && mesh.rig_in_step());
        // Bad words are met with what is possible.
        let e = s.edit(&[json!({"op": "wear", "clip": id, "items": [{"kind": "spacesuit"}]})], "x", false).unwrap_err();
        assert!(e.contains("shirt") && e.contains("armour"), "{e}");
        let e = s.edit(&[json!({"op": "wear", "clip": id, "items": [{"kind": "coat", "material": "unobtainium"}]})], "x", false).unwrap_err();
        assert!(e.contains("cotton") && e.contains("leather"), "{e}");
        let e = s.edit(&[json!({"op": "wear", "clip": id, "items": [{"kind": "coat", "fit": "snug-ish"}]})], "x", false).unwrap_err();
        assert!(e.contains("regular"), "{e}");
    }

    #[test]
    fn a_held_prop_follows_the_hand_and_the_fingers_close_round_it() {
        let mut s = Session::new(services::project::new_custom("Hold", 640, 360, 30.0));
        let id = figure(&mut s, json!({}));
        let (_, before) = s.model_of(id).unwrap();
        let r = s.edit(&[json!({"op": "hold", "clip": id, "hand": "left", "item": {"kit": "cup", "colour": "#cc3333"}})], "hold", false).unwrap().to_string();
        assert!(r.contains("holds the cup") && r.contains("closed round it"), "{r}");
        let (mc, mesh) = s.model_of(id).unwrap();
        assert!(mesh.rig_in_step());
        let rig = mesh.rig.clone().unwrap();
        let hand = rig.bone("hand.L").unwrap();
        let first = before.positions.len();
        assert!((first..mesh.positions.len()).all(|v| rig.joints[v][0] as usize == hand && rig.weights[v][0] == 1.0));
        // The cup sits in the fist: the grip bone is inside its extent.
        let grip = rig.bones[rig.bone("grip.L").unwrap()].head;
        let (lo, hi) = {
            let ps = &mesh.positions[first..];
            (ps.iter().fold([f32::INFINITY; 3], |a, p| [a[0].min(p[0]), a[1].min(p[1]), a[2].min(p[2])]), ps.iter().fold([f32::NEG_INFINITY; 3], |a, p| [a[0].max(p[0]), a[1].max(p[1]), a[2].max(p[2])]))
        };
        assert!((0..3).all(|i| grip[i] >= lo[i] - 1.0 && grip[i] <= hi[i] + 1.0), "grip {grip:?} in {lo:?}..{hi:?}");
        // Finger keys exist for the left hand only.
        assert!(mc.keyframes.keys().any(|k| k.starts_with("generator.bone.index.L.")) && !mc.keyframes.keys().any(|k| k.contains(".R.")));
        // Raise the forearm: the cup goes with the hand.
        let posed = rig.posed_mesh(&mesh, &studio_mesh::rig::PoseInput { rotations: [("forearm.L".to_string(), [90.0, 0.0, 0.0])].into_iter().collect(), ..Default::default() });
        let hand_moved = studio_mesh::length(studio_mesh::sub(posed.positions[first], mesh.positions[first]));
        assert!(hand_moved > 20.0, "the cup moves with the arm: {hand_moved}");
        // Letting go opens the fingers.
        s.edit(&[json!({"op": "hold", "clip": id, "hand": "left", "release": true, "at": 2})], "release", false).unwrap();
        let e = s.edit(&[json!({"op": "hold", "clip": id, "item": "spaceship"})], "x", false).unwrap_err();
        assert!(e.contains("cup") && e.contains("lantern"), "{e}");
    }

    #[test]
    fn a_lit_candle_gets_a_flickering_light_that_follows_the_hand() {
        let mut s = Session::new(services::project::new_custom("Flame", 640, 360, 30.0));
        let id = figure(&mut s, json!({}));
        let lights = |s: &Session| s.doc.model.sequence(s.doc.seq()).map_or(0, |q| q.clips.iter().filter(|c| matches!(&c.source, m::ClipSource::Generator(g) if g.kind == m::GeneratorKind::Light)).count());
        let before = lights(&s);
        let r = s.edit(&[json!({"op": "hold", "clip": id, "item": "candle"})], "hold", false).unwrap().to_string();
        assert!(r.contains("flickering point light"), "{r}");
        assert_eq!(lights(&s), before + 1);
        let (_, mesh) = s.model_of(id).unwrap();
        let flame = mesh.materials.iter().find(|m| m.name.starts_with("Flame")).expect("a flame");
        assert!(flame.emissive.iter().any(|&e| e > 1.0));
        // Not every held thing burns.
        let id2 = figure(&mut s, json!({}));
        s.edit(&[json!({"op": "hold", "clip": id2, "item": "phone"})], "phone", false).unwrap();
        assert_eq!(lights(&s), before + 1);
        // A flame is general: any prop can carry one where it says.
        let r = s.edit(&[json!({"op": "hold", "clip": id2, "hand": "left", "item": {"kit": "stick", "tip": "#aa3322", "flame": {"size": 12}}})], "stick", false).unwrap().to_string();
        assert!(r.contains("light"), "{r}");
    }

    #[test]
    fn hair_and_a_beard_are_worn_like_clothes_and_the_parts_are_listed() {
        let mut s = Session::new(services::project::new_custom("Hair", 640, 360, 30.0));
        let id = figure(&mut s, json!({}));
        let r = s.edit(&[json!({"op": "wear", "clip": id, "items": [{"kind": "hair", "style": "short", "colour": "#553322", "density": 0.3}, {"kind": "beard", "style": "stubble", "density": 0.3}]})], "hair", false).unwrap().to_string();
        assert!(r.contains("Hair") && r.contains("Beard"), "{r}");
        let (_, mesh) = s.model_of(id).unwrap();
        let worn = wear::worn(&mesh);
        assert!(worn.iter().any(|n| n == "Hair") && worn.iter().any(|n| n == "Beard"), "{worn:?}");
        assert!(mesh.rig_in_step());
        let d = super::super::models::describe(&mesh);
        assert!(d["parts"].as_array().unwrap().iter().any(|p| p["name"] == "Hair") && d["worn"].as_array().is_some());
        s.edit(&[json!({"op": "wear", "clip": id, "remove": "hair"})], "off", false).unwrap();
        let (_, mesh) = s.model_of(id).unwrap();
        assert!(!wear::worn(&mesh).iter().any(|n| n == "Hair"));
        // Bald is not a style to grow.
        let e = s.edit(&[json!({"op": "wear", "clip": id, "items": [{"kind": "hair", "style": "bald"}]})], "x", false).unwrap_err();
        assert!(e.contains("bald"), "{e}");
    }
}
