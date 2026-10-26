// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Living places. `see` with `regions` says what a backdrop's parts are
//! (sky, water, ground, path, trees, rocks, mountains, buildings, people,
//! animals: a scene-parsing model, checked against the depth map) and
//! where; `mask` with `region` turns one into masks on any layer;
//! `particles` adds a particle layer (rain, snow, leaves, mist...) by
//! preset, into a region if asked; `living` lays whole recipes: water
//! that flows and glints, foliage that sways, drifting clouds, light
//! rays, day turning to night, weather coming in.

use super::{Session, f, micros, secs};
use crate::view::{self, ClipKind, SEC};
use serde_json::{Value, json};
use studio_ai::features::regions::{self, Region};
use studio_model as m;

/// One region of a backdrop, in fractions of the frame.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct RegionInfo {
    pub name: String,
    pub share: f32,
    /// `[x0, y0, x1, y1]`.
    pub bbox: [f32; 4],
    pub centre: [f32; 2],
    /// Mean depth over the region, 1 = near.
    pub depth: f32,
    /// Outlines: whether each is a hole, and its points.
    pub outlines: Vec<(bool, Vec<[f32; 2]>)>,
}

/// What a backdrop is made of at one moment.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct SceneRegions {
    pub backdrop: Option<u32>,
    /// Where the far land ends (fraction of the height), from the depth.
    pub horizon: f32,
    /// The brightest place in the sky (for light rays).
    pub light: [f32; 2],
    pub regions: Vec<RegionInfo>,
}

impl SceneRegions {
    pub fn get(&self, r: Region) -> Option<&RegionInfo> {
        self.regions.iter().find(|x| x.name == r.name())
    }
}

fn place_word(x: f32, y: f32) -> String {
    let hx = if x < 0.38 { "left" } else if x > 0.62 { "right" } else { "centre" };
    let vy = if y < 0.36 { "top " } else if y > 0.64 { "bottom " } else { "" };
    if vy.is_empty() && hx == "centre" { "centre".into() } else { format!("{vy}{hx}") }
}

fn depth_word(d: f32) -> &'static str {
    if d > 0.6 {
        "near"
    } else if d > 0.3 {
        "middle distance"
    } else {
        "far"
    }
}

/// Fetches a model the person's request needs (as `see` with `place`
/// does for depth), saying what it is when it cannot.
fn ensure(store: &studio_ai::store::ModelStore, id: studio_ai::registry::ModelId) -> Result<(), String> {
    if !store.is_installed(id) {
        let spec = id.spec();
        store
            .download(id, &studio_ai::jobs::JobCtx::detached())
            .map_err(|e| format!("could not download {} ({}, {}): {e}; get it in Settings → AI models", spec.name, studio_ai::registry::human_size(spec.download_size()), spec.licence))?;
    }
    Ok(())
}

impl Session {
    /// The picture clip a scene is set in at `at`: the lowest picture
    /// layer showing then.
    pub(super) fn backdrop_at(&self, at: view::Time) -> Option<u32> {
        let p = &self.doc.project;
        let idx = |l: u32| p.lanes.iter().position(|x| x.id == l).unwrap_or(0);
        p.clips.iter().filter(|c| c.kind == ClipKind::Video && c.start <= at && at < c.end() && !p.lanes.iter().any(|l| l.id == c.lane && l.hidden)).max_by_key(|c| idx(c.lane)).map(|c| c.id)
    }

    /// The backdrop at `t` (seconds) read into regions, cached by the
    /// backdrop clip and its moment.
    pub(super) fn regions_at(&mut self, t: f64, backdrop: Option<u32>) -> Result<SceneRegions, String> {
        let at = (t * SEC as f64) as view::Time;
        let back = backdrop.or_else(|| self.backdrop_at(at));
        // Everything else showing then is left out of the picture read.
        let leave: Vec<u32> = self.doc.project.clips.iter().filter(|c| c.kind != ClipKind::Audio && Some(c.id) != back && c.start <= at && at < c.end()).map(|c| c.id).collect();
        let key = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            "regions1".hash(&mut h);
            (self.doc.project.width, self.doc.project.height).hash(&mut h);
            match back.and_then(|b| self.model_clip(b)) {
                Some(c) => {
                    serde_json::to_string(&c).unwrap_or_default().hash(&mut h);
                    (at - c.start).hash(&mut h);
                }
                None => at.hash(&mut h),
            }
            format!("regions:{:016x}", h.finish())
        };
        if let Some(v) = self.memo_pub().get(&key).cloned()
            && let Ok(r) = serde_json::from_value::<SceneRegions>(v)
        {
            return Ok(r);
        }
        let img = self.frames.rgba_without(&self.doc, &leave, at, 912)?;
        // The person asked to read the place: fetch the models once.
        let store = studio_ai::store::ModelStore::default();
        ensure(&store, studio_ai::registry::ModelId::DepthAnythingV2Small)?;
        ensure(&store, studio_ai::registry::ModelId::UperNetConvNextTiny)?;
        let frame = studio_ai::frame::Frame::new(img.width as usize, img.height as usize, img.pixels.clone()).map_err(|e| e.to_string())?;
        let depth = studio_ai::features::depth::DepthEstimator::load(&store).map_err(|e| format!("the depth model is not ready: {e}"))?.estimate(&frame).map_err(|e| format!("could not read the depth: {e}"))?;
        let parser = regions::SceneParser::load(&store).map_err(|e| format!("the scene model is not ready: {e}"))?;
        let mut map = parser.parse(&frame, 256).map_err(|e| format!("could not read the scene: {e}"))?;
        map.refine_with_depth(&depth);
        let horizon = super::sight::ground::DepthGrid::shrink(depth.width, depth.height, &depth.data, 96).horizon();
        let d = depth.resized(map.width, map.height);
        let (w, h) = (map.width, map.height);
        let mut out = Vec::new();
        for r in Region::ALL {
            let hard = map.hard(r);
            let n = hard.iter().filter(|&&v| v > 0.5).count();
            let share = n as f32 / (w * h) as f32;
            if share < 0.01 {
                continue;
            }
            let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
            let (mut sx, mut sy, mut sd) = (0f32, 0f32, 0f32);
            for y in 0..h {
                for x in 0..w {
                    if hard[y * w + x] > 0.5 {
                        x0 = x0.min(x);
                        y0 = y0.min(y);
                        x1 = x1.max(x + 1);
                        y1 = y1.max(y + 1);
                        sx += x as f32 + 0.5;
                        sy += y as f32 + 0.5;
                        sd += d.data[y * w + x];
                    }
                }
            }
            let outlines = regions::outlines(&hard, w, h, 0.5, 0.002, 0.8, 160).into_iter().map(|o| (o.hole, o.points)).collect();
            out.push(RegionInfo {
                name: r.name().into(),
                share,
                bbox: [x0 as f32 / w as f32, y0 as f32 / h as f32, x1 as f32 / w as f32, y1 as f32 / h as f32],
                centre: [sx / n as f32 / w as f32, sy / n as f32 / h as f32],
                depth: sd / n as f32,
                outlines,
            });
        }
        // The light: the brightest patch of sky (or of the top half).
        let sky = map.hard(Region::Sky);
        let (iw, ih) = (img.width as usize, img.height as usize);
        let mut best = (0.0f32, [0.5f32, 0.15f32]);
        for gy in 0..h {
            for gx in 0..w {
                if sky.iter().any(|&v| v > 0.5) && sky[gy * w + gx] < 0.5 {
                    continue;
                }
                if !sky.iter().any(|&v| v > 0.5) && gy > h / 2 {
                    continue;
                }
                let (px, py) = (gx * iw / w, gy * ih / h);
                let q = &img.pixels[(py * iw + px) * 4..][..3];
                let l = 0.2126 * q[0] as f32 + 0.7152 * q[1] as f32 + 0.0722 * q[2] as f32;
                if l > best.0 {
                    best = (l, [(gx as f32 + 0.5) / w as f32, (gy as f32 + 0.5) / h as f32]);
                }
            }
        }
        let result = SceneRegions { backdrop: back, horizon, light: best.1, regions: out };
        let v = serde_json::to_value(&result).unwrap_or(Value::Null);
        self.memo_pub().insert(key, v);
        self.memo_save_pub();
        Ok(result)
    }

    /// `see` with `regions`: what the backdrop's parts are and where.
    pub(super) fn see_regions(&mut self, a: &Value) -> Result<String, String> {
        let t = a.get("t").or(a.get("at")).and_then(Value::as_f64).ok_or("see with regions needs \"t\": the moment to read (seconds on the timeline)")?;
        let back = a.get("backdrop").and_then(Value::as_u64).map(|x| x as u32);
        let r = self.regions_at(t, back)?;
        Ok(self.regions_words(t, &r))
    }

    pub(super) fn regions_words(&self, t: f64, r: &SceneRegions) -> String {
        let (w, h) = (self.doc.project.width as f32, self.doc.project.height as f32);
        let px = |u: f32, v: f32| format!("[{}, {}]", ((u - 0.5) * w).round(), ((v - 0.5) * h).round());
        let mut out = vec![format!(
            "the backdrop{} at {t:.2}s is made of (canvas px from the centre):",
            r.backdrop.map(|b| format!(" (clip {b})")).unwrap_or_default()
        )];
        let mut rs = r.regions.clone();
        rs.sort_by(|a, b| b.share.total_cmp(&a.share));
        for x in &rs {
            let parts = x.outlines.iter().filter(|o| !o.0).count();
            out.push(format!(
                "  {:10} {:3.0}%  {}, {}{}  box {} to {}",
                x.name,
                x.share * 100.0,
                place_word(x.centre[0], x.centre[1]),
                depth_word(x.depth),
                if parts > 1 { format!(", {parts} parts") } else { String::new() },
                px(x.bbox[0], x.bbox[1]),
                px(x.bbox[2], x.bbox[3]),
            ));
        }
        if rs.is_empty() {
            out.push("  nothing the scene model knows (an abstract or graphic picture?)".into());
        }
        out.push(format!("the far land ends at y = {}; the brightest light is at {}.", ((r.horizon - 0.5) * h).round(), px(r.light[0], r.light[1])));
        out.push("Put an effect inside a region with `mask` {\"clip\", \"region\": \"water\"} (on an adjustment layer: ripples, sway, a grade); particles into one with `particles` {\"preset\", \"region\"}; or bring the whole place to life with `living` {\"recipes\": [\"water\", \"foliage\", \"clouds\", \"rays\"]}.".into());
        out.join("\n")
    }

    /// Masks on clip `id` that let it show only in a region of the
    /// backdrop (or everywhere but, with `invert`). Returns how many.
    pub(super) fn region_masks(&mut self, id: u32, info: &RegionInfo, at: view::Time, feather: f64, expand: f64, invert: bool) -> Result<usize, String> {
        let seq = self.doc.seq();
        let cid = m::ClipId(id as u64);
        let (w, h) = (self.doc.project.width as f32, self.doc.project.height as f32);
        let map = crate::editor::pins::LayerMap::at(&self.doc.project, &self.doc.model, id, at);
        // Frame fractions to the layer's own 0..1 coordinates.
        let to_layer = |p: [f32; 2]| -> m::Vec2 {
            match &map {
                Some(mp) => mp.layer(eframe::egui::Vec2::new((p[0] - 0.5) * w, (p[1] - 0.5) * h)).unwrap_or(m::Vec2::new(p[0] as f64, p[1] as f64)),
                None => m::Vec2::new(p[0] as f64, p[1] as f64),
            }
        };
        let mut masks = Vec::new();
        if invert {
            let mut all = m::Mask::new(m::MaskId(self.doc.alloc_id()), m::MaskShape::Rectangle);
            all.params.insert("position".into(), m::Value::Point(m::Vec2::new(0.5, 0.5)));
            all.params.insert("size".into(), m::Value::Point(m::Vec2::new(1.5, 1.5)));
            masks.push(all);
        }
        // Outer outlines first, then holes cut out of them.
        let mut order: Vec<&(bool, Vec<[f32; 2]>)> = info.outlines.iter().filter(|o| !o.0).collect();
        order.extend(info.outlines.iter().filter(|o| o.0));
        for (hole, pts) in order {
            let mut mk = m::Mask::new(m::MaskId(self.doc.alloc_id()), m::MaskShape::Path);
            mk.mode = if *hole != invert { m::MaskMode::Subtract } else { m::MaskMode::Add };
            mk.path = pts.iter().map(|&p| m::PathPoint { point: to_layer(p), handle_in: m::Vec2::new(0.0, 0.0), handle_out: m::Vec2::new(0.0, 0.0) }).collect();
            mk.params.insert("feather".into(), m::Value::Scalar(feather));
            if expand != 0.0 {
                mk.params.insert("expansion".into(), m::Value::Scalar(expand));
            }
            masks.push(mk);
        }
        let n = masks.len();
        for mk in masks {
            let ok = self.doc.apply("Region mask", m::Edit::AddMask { seq, clip: cid, mask: mk, index: None });
            self.check(ok, "")?;
        }
        Ok(n)
    }

    /// `mask` with `region`: the clip shows only inside that part of the
    /// backdrop.
    pub(super) fn mask_region(&mut self, op: &Value) -> Result<String, String> {
        let id = op.get("clip").and_then(Value::as_u64).ok_or("mask needs \"clip\"")? as u32;
        let name = op.get("region").and_then(Value::as_str).ok_or("region is a name: sky, water, ground, path, trees, rocks, mountains, buildings, people, animals")?;
        let region = Region::from_name(name).ok_or_else(|| format!("region \"{name}\": sky, water, ground, path, trees, rocks, mountains, buildings, people or animals"))?;
        let c = self.clip(id)?;
        let t = f(op, "t").unwrap_or(secs(c.start + c.dur.min(SEC / 10)));
        let at = micros(t);
        let back = op.get("backdrop").and_then(Value::as_u64).map(|x| x as u32).or_else(|| self.backdrop_at(at).filter(|&b| b != id));
        let r = self.regions_at(t, back)?;
        let info = r.get(region).cloned().ok_or_else(|| format!("the backdrop at {t:.2}s has no {} (it has {}); see with regions", region.name(), r.regions.iter().map(|x| x.name.as_str()).collect::<Vec<_>>().join(", ")))?;
        let n = self.region_masks(id, &info, at, f(op, "feather").unwrap_or(0.01), f(op, "expand").unwrap_or(0.0), op.get("invert").and_then(Value::as_bool).unwrap_or(false))?;
        Ok(format!("clip {id} now shows {} the {} ({:.0}% of the frame, {n} mask(s): soften with feather, grow with expand)", if op.get("invert").and_then(Value::as_bool).unwrap_or(false) { "everywhere but" } else { "only in" }, region.name(), info.share * 100.0))
    }

    /// A lane directly above `lane` that is free over `at..at + dur`
    /// (one is added when the lane above is taken).
    fn lane_above(&mut self, lane: u32, at: view::Time, dur: view::Time) -> Result<u32, String> {
        let p = &self.doc.project;
        let idx = p.lanes.iter().position(|l| l.id == lane).ok_or("that lane is gone")?;
        if idx > 0 {
            let above = &p.lanes[idx - 1];
            if above.kind == view::LaneKind::Visual && !above.locked && !p.clips.iter().any(|c| c.lane == above.id && c.start < at + dur && at < c.end()) {
                return Ok(above.id);
            }
        }
        let id = self.doc.alloc_id();
        let seq = self.doc.seq();
        let ok = self.doc.apply("Add lane", m::Edit::AddLane { seq, lane: m::Lane::new(m::LaneId(id), m::LaneKind::Visual, "Living"), index: Some(idx) });
        self.check(ok, "")?;
        Ok(id as u32)
    }

    /// Inserts a clip of `source` with `effects` on `lane`.
    fn insert_layer(&mut self, lane: u32, at: view::Time, dur: view::Time, name: &str, source: m::ClipSource, effects: Vec<m::Effect>, blend: m::BlendMode) -> Result<u32, String> {
        let id = self.doc.alloc_id() as u32;
        let mut c = m::Clip::new(m::ClipId(id as u64), m::LaneId(lane as u64), at, dur, source);
        c.name = name.into();
        c.effects = effects;
        c.blend = blend;
        let seq = self.doc.seq();
        let ok = self.doc.apply(name, m::Edit::InsertClip { seq, clip: Box::new(c) });
        self.check(ok, "")?;
        Ok(id)
    }

    fn effect_of(&mut self, kind: &str, params: &[(&str, m::Value)]) -> m::Effect {
        let mut e = m::Effect::new(m::EffectId(self.doc.alloc_id()), kind);
        for (k, v) in params {
            e.params.insert((*k).into(), v.clone());
        }
        e
    }

    /// A clear full-frame layer carrying a particle system.
    #[allow(clippy::too_many_arguments)]
    fn particle_layer(&mut self, preset: &'static studio_fx::particles::Preset, lane: u32, at: view::Time, dur: view::Time, region: Option<&RegionInfo>, scene: Option<&SceneRegions>, extra: &[(String, m::Value)]) -> Result<u32, String> {
        let s = |x: f64| m::Value::Scalar(x);
        let mut ps: Vec<(&str, m::Value)> = vec![("preset", m::Value::Choice(preset.name.into()))];
        if let Some(r) = region {
            // Inside the region: falling things fall across its width,
            // the rest start anywhere in it.
            let falling = matches!(preset.emitter, studio_fx::particles::Emitter::Top);
            let rising = matches!(preset.emitter, studio_fx::particles::Emitter::Bottom);
            ps.push(("emitter", m::Value::Choice(if falling { "Top edge" } else if rising { "Bottom edge" } else { "Area" }.into())));
            ps.push(("area_a", m::Value::Point(m::Vec2::new(r.bbox[0] as f64, r.bbox[1] as f64))));
            ps.push(("area_b", m::Value::Point(m::Vec2::new(r.bbox[2] as f64, r.bbox[3] as f64))));
        }
        if let Some(sc) = scene {
            ps.push(("horizon", s(sc.horizon as f64)));
        }
        let mut e = self.effect_of("particles", &ps);
        for (k, v) in extra {
            e.params.insert(k.clone(), v.clone());
        }
        let mut g = m::Generator::new(m::GeneratorKind::Solid);
        g.params.insert("colour".into(), m::Value::Colour(m::Rgba { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }));
        let blend = if preset.glow { m::BlendMode::Screen } else { m::BlendMode::Normal };
        let id = self.insert_layer(lane, at, dur, &format!("{} (particles)", preset.name), m::ClipSource::Generator(g), vec![e], blend)?;
        if let Some(r) = region {
            self.region_masks(id, r, at, 0.02, 0.0, false)?;
        }
        Ok(id)
    }

    /// `particles`: a particle layer by preset, over the whole frame or
    /// into a region of the backdrop.
    pub(super) fn particles(&mut self, op: &Value) -> Result<String, String> {
        let name = op.get("preset").and_then(Value::as_str).ok_or_else(|| format!("particles needs \"preset\": {}", studio_fx::particles::PRESETS.iter().map(|p| p.id).collect::<Vec<_>>().join(", ")))?;
        let (_, preset) = studio_fx::particles::preset(name).ok_or_else(|| format!("no particle preset \"{name}\"; presets: {}", studio_fx::particles::PRESETS.iter().map(|p| p.id).collect::<Vec<_>>().join(", ")))?;
        let at_s = f(op, "at").unwrap_or(0.0);
        let at = micros(at_s);
        let end = self.doc.project.clips.iter().map(|c| c.end()).max().unwrap_or(SEC * 5);
        let dur = f(op, "duration").map(micros).unwrap_or_else(|| (end - at).max(SEC)).max(SEC / 10);
        let region = op.get("region").and_then(Value::as_str);
        let (scene, info) = match region {
            Some(rn) => {
                let r = Region::from_name(rn).ok_or_else(|| format!("region \"{rn}\": sky, water, ground, path, trees, rocks, mountains, buildings, people or animals"))?;
                let sc = self.regions_at(f(op, "t").unwrap_or(at_s + 0.05), None)?;
                let i = sc.get(r).cloned().ok_or_else(|| format!("the backdrop has no {} (it has {})", r.name(), sc.regions.iter().map(|x| x.name.as_str()).collect::<Vec<_>>().join(", ")))?;
                (Some(sc), Some(i))
            }
            None => (None, None),
        };
        // In front of everything, or behind a clip's layer.
        let lane = match op.get("behind").and_then(Value::as_u64) {
            Some(b) => {
                let l = self.clip(b as u32)?.lane;
                self.lane_above_below(l, at, dur)?
            }
            None => match op.get("lane") {
                Some(v) => {
                    let want = super::craft::lane_of(&self.doc.project, v, None)?;
                    self.room(Some(want), at, dur, None)?
                }
                None => self.room(None, at, dur, None)?,
            },
        };
        let mut extra = Vec::new();
        if let Some(Value::Object(o)) = op.get("params") {
            for (k, v) in o {
                let val = match v {
                    Value::Number(n) => m::Value::Scalar(n.as_f64().unwrap_or(0.0)),
                    Value::Bool(b) => m::Value::Bool(*b),
                    Value::String(s) if s.starts_with('#') => {
                        let c = super::craft::colour(s).ok_or_else(|| format!("{k}: \"{s}\" is not #rrggbb"))?;
                        m::Value::Colour(m::Rgba { r: c.r() as f64 / 255.0, g: c.g() as f64 / 255.0, b: c.b() as f64 / 255.0, a: 1.0 })
                    }
                    Value::String(s) => m::Value::Choice(s.clone()),
                    Value::Array(a) if a.len() == 2 => m::Value::Point(m::Vec2::new(a[0].as_f64().unwrap_or(0.0), a[1].as_f64().unwrap_or(0.0))),
                    _ => return Err(format!("particle setting {k}: a number, true/false, a choice name, #rrggbb or [x, y]")),
                };
                extra.push((k.clone(), val));
            }
        }
        let id = self.particle_layer(preset, lane, at, dur, info.as_ref(), scene.as_ref(), &extra)?;
        Ok(format!(
            "{} as clip {id} ({:.1}s from {at_s:.2}s{}); its settings are particles.<setting>: amount (animates 0..2 smoothly), size, speed, direction, gravity, wind, turbulence, colour, opacity, ground, horizon, on_ground, depth, look, emitter, area_a, area_b",
            preset.name,
            secs(dur),
            info.as_ref().map(|i| format!(", inside the {}", i.name)).unwrap_or_default()
        ))
    }

    /// A free lane just under `lane` (so a layer goes behind that clip).
    fn lane_above_below(&mut self, lane: u32, at: view::Time, dur: view::Time) -> Result<u32, String> {
        let p = &self.doc.project;
        let idx = p.lanes.iter().position(|l| l.id == lane).ok_or("that lane is gone")?;
        if self.doc.project.spine() == Some(lane) {
            return Err("that clip is on the main lane, the bottom of the picture: nothing goes behind it".into());
        }
        match p.lanes.get(idx + 1) {
            Some(below) if below.kind == view::LaneKind::Visual => self.lane_above(below.id, at, dur),
            _ => self.lane_above(lane, at, dur),
        }
    }

    /// `living`: recipes that bring a still backdrop to life.
    pub(super) fn living(&mut self, op: &Value) -> Result<String, String> {
        let recipes: Vec<String> = match op.get("recipes").or(op.get("recipe")) {
            Some(Value::Array(v)) => v.iter().filter_map(Value::as_str).map(str::to_lowercase).collect(),
            Some(Value::String(s)) => vec![s.to_lowercase()],
            _ => vec!["water".into(), "foliage".into(), "clouds".into()],
        };
        const KNOWN: &[&str] = &["water", "foliage", "clouds", "rays", "dusk", "night", "day_to_night", "night_to_day", "rain", "storm", "snow", "mist", "fog", "leaves", "fireflies", "dust", "embers", "magic"];
        if let Some(bad) = recipes.iter().find(|r| !KNOWN.contains(&r.as_str())) {
            return Err(format!("no recipe \"{bad}\"; recipes: {}", KNOWN.join(", ")));
        }
        // The backdrop and the stretch of time it is on screen.
        let t0 = f(op, "t").or(f(op, "at"));
        let back = match op.get("backdrop").and_then(Value::as_u64) {
            Some(b) => b as u32,
            None => self.backdrop_at(micros(t0.unwrap_or(0.0))).ok_or("no picture on screen there to bring to life; give \"backdrop\" (its clip id)")?,
        };
        let bc = self.clip(back)?;
        let at = f(op, "at").map(micros).unwrap_or(bc.start).max(bc.start);
        let dur = f(op, "duration").map(micros).unwrap_or(bc.end() - at).max(SEC / 10);
        let ramp = op.get("ramp").and_then(Value::as_array).and_then(|a| Some((a.first()?.as_f64()?, a.get(1)?.as_f64()?)));
        let needs_regions = recipes.iter().any(|r| ["water", "foliage", "clouds", "rays", "night", "day_to_night", "night_to_day", "dusk", "mist", "fog", "fireflies"].contains(&r.as_str()));
        let scene = if needs_regions { Some(self.regions_at(secs(at) + 0.05, Some(back))?) } else { None };
        let reg = |r: Region| scene.as_ref().and_then(|s| s.get(r).cloned());
        let s = |x: f64| m::Value::Scalar(x);
        let mut said: Vec<String> = Vec::new();
        let mut skipped: Vec<String> = Vec::new();
        let (dur_s, len) = (secs(dur), dur);
        // A grade layer over everything (characters too: light changes
        // for all), made once and shared by the light recipes.
        let mut grade_layer: Option<u32> = None;
        let mut grade = |me: &mut Session, keys: &[(&str, Vec<[f64; 2]>)]| -> Result<u32, String> {
            let id = match grade_layer {
                Some(id) => id,
                None => {
                    let lane = me.room(None, at, len, None)?;
                    let id = me.insert_layer(lane, at, len, "Light (living)", m::ClipSource::Generator(m::Generator::new(m::GeneratorKind::Adjustment)), vec![], m::BlendMode::Normal)?;
                    grade_layer = Some(id);
                    id
                }
            };
            for (prop, k) in keys {
                me.animate(&json!({"clip": id, "property": prop, "keys": k, "ease": "ease"}))?;
            }
            Ok(id)
        };
        // Light first (top), then the backdrop's own layers, each added
        // directly over the backdrop (so later ones sit lower).
        for r in &recipes {
            let (a, b) = ramp.unwrap_or((0.0, dur_s.min(4.0)));
            match r.as_str() {
                "dusk" | "night" | "day_to_night" | "night_to_day" => {
                    let (from, to) = match r.as_str() {
                        "night" => (1.0, 1.0),
                        "night_to_day" => (1.0, 0.0),
                        "dusk" => (0.0, 0.55),
                        _ => (0.0, 1.0),
                    };
                    let k = |v0: f64, v1: f64| if r == "night" { vec![[0.0, v1]] } else { vec![[a, v0], [b, v1]] };
                    let id = grade(self, &[("exposure", k(-2.1 * from, -2.1 * to)), ("saturation", k(-50.0 * from, -50.0 * to)), ("temperature", k(-40.0 * from, -40.0 * to))])?;
                    let mut stars = None;
                    if to > 0.9 || from > 0.9 {
                        if let Some(sky) = reg(Region::Sky) {
                            let lane = self.lane_above(bc.lane, at, dur)?;
                            let (_, p) = studio_fx::particles::preset("stars").expect("stars preset");
                            let sid = self.particle_layer(p, lane, at, dur, Some(&sky), scene.as_ref(), &[])?;
                            if r != "night" {
                                self.animate(&json!({"clip": sid, "property": "particles.amount", "keys": [[a, from], [b, to]], "ease": "ease"}))?;
                            }
                            stars = Some(sid);
                        }
                    }
                    said.push(format!("{} on light layer {id}{}", r.replace('_', " "), stars.map(|s| format!(", stars in the sky as clip {s}")).unwrap_or_default()));
                }
                "rain" | "storm" | "snow" | "leaves" | "dust" | "embers" | "magic" | "fireflies" => {
                    let preset = match r.as_str() {
                        "storm" => "rain",
                        x => x,
                    };
                    let (_, p) = studio_fx::particles::preset(preset).expect("weather preset");
                    let lane = if r == "fireflies" { self.lane_above(bc.lane, at, dur)? } else { self.room(None, at, dur, None)? };
                    let mut extra: Vec<(String, m::Value)> = Vec::new();
                    if r == "storm" {
                        extra.push(("amount".into(), s(1.6)));
                        extra.push(("wind".into(), s(-25.0)));
                    }
                    let area = if r == "fireflies" { reg(Region::Ground).or(reg(Region::Trees)) } else { None };
                    let id = self.particle_layer(p, lane, at, dur, area.as_ref(), scene.as_ref(), &extra)?;
                    if let Some((a, b)) = ramp {
                        let top = if r == "storm" { 1.6 } else { 1.0 };
                        self.animate(&json!({"clip": id, "property": "particles.amount", "keys": [[a, 0.0], [b, top]], "ease": "ease"}))?;
                    }
                    let mut note = format!("{} as clip {id}", p.name.to_lowercase());
                    if r == "rain" || r == "storm" {
                        let (dark, sat) = if r == "storm" { (-0.8, -35.0) } else { (-0.35, -20.0) };
                        let keys = |v: f64| match ramp {
                            Some((a, b)) => vec![[a, 0.0], [b, v]],
                            None => vec![[0.0, v]],
                        };
                        let gid = grade(self, &[("exposure", keys(dark)), ("saturation", keys(sat))])?;
                        note.push_str(&format!(", the light dimmed on layer {gid}"));
                        if r == "storm" {
                            // Lightning: two quick flashes, well apart.
                            let flash = self.room(None, at, dur, None)?;
                            let fid = self.insert_layer(flash, at, dur, "Lightning (living)", m::ClipSource::Generator(m::Generator::new(m::GeneratorKind::Adjustment)), vec![], m::BlendMode::Normal)?;
                            let start = ramp.map(|(_, b)| b).unwrap_or(0.0) + 1.0;
                            let mut keys = vec![[0.0, 0.0]];
                            let mut tt = start;
                            while tt + 0.4 < dur_s {
                                keys.extend([[tt - 0.02, 0.0], [tt, 1.6], [tt + 0.06, 0.2], [tt + 0.12, 1.2], [tt + 0.35, 0.0]]);
                                tt += 4.5;
                            }
                            self.animate(&json!({"clip": fid, "property": "exposure", "keys": keys, "ease": "linear"}))?;
                            note.push_str(&format!(", lightning on {fid}"));
                        }
                    }
                    said.push(note);
                }
                _ => {}
            }
        }
        for r in &recipes {
            match r.as_str() {
                "rays" => {
                    let light = scene.as_ref().map(|s| s.light).unwrap_or([0.5, 0.15]);
                    let lane = self.lane_above(bc.lane, at, dur)?;
                    let e = self.effect_of("god_rays", &[("center", m::Value::Point(m::Vec2::new(light[0] as f64, light[1] as f64))), ("intensity", s(0.7)), ("length", s(0.5)), ("threshold", s(0.55))]);
                    let id = self.insert_layer(lane, at, dur, "Light rays (living)", m::ClipSource::Generator(m::Generator::new(m::GeneratorKind::Adjustment)), vec![e], m::BlendMode::Normal)?;
                    let half = dur_s / 2.0;
                    self.animate(&json!({"clip": id, "property": "god_rays.intensity", "keys": [[0.0, 0.55], [half, 0.85], [dur_s, 0.55]], "ease": "ease"}))?;
                    said.push(format!("light rays from the bright sky as clip {id}"));
                }
                "clouds" => {
                    let Some(sky) = reg(Region::Sky) else {
                        skipped.push("clouds (no sky in the picture)".into());
                        continue;
                    };
                    let lane = self.lane_above(bc.lane, at, dur)?;
                    // Wisps drifting with the wind, inside the sky only.
                    let e = self.effect_of(
                        "noise",
                        &[
                            ("scale", s(28.0)),
                            ("octaves", s(5.0)),
                            ("speed", s(0.04)),
                            ("contrast", s(1.6)),
                            ("colour_a", m::Value::Colour(m::Rgba { r: 1.0, g: 1.0, b: 1.0, a: 0.0 })),
                            ("colour_b", m::Value::Colour(m::Rgba { r: 1.0, g: 1.0, b: 1.0, a: 0.55 })),
                            ("drift", m::Value::Point(m::Vec2::new(1.2, 0.0))),
                            ("mix", s(0.6)),
                        ],
                    );
                    let id = self.insert_layer(lane, at, dur, "Drifting clouds (living)", m::ClipSource::Generator(m::Generator::new(m::GeneratorKind::Adjustment)), vec![e], m::BlendMode::Normal)?;
                    self.region_masks(id, &sky, at, 0.03, -0.004, false)?;
                    said.push(format!("clouds drifting in the sky as clip {id}"));
                }
                "water" => {
                    let Some(water) = reg(Region::Water) else {
                        skipped.push("water (no water in the picture)".into());
                        continue;
                    };
                    let lane = self.lane_above(bc.lane, at, dur)?;
                    let (_, gl) = studio_fx::particles::preset("glints").expect("glints preset");
                    let gid = self.particle_layer(gl, lane, at, dur, Some(&water), scene.as_ref(), &[("amount".into(), s(0.7))])?;
                    // A stream that runs at the camera flows down the
                    // picture; a lake or the sea drifts sideways, slower.
                    let (bw, bh) = (water.bbox[2] - water.bbox[0], water.bbox[3] - water.bbox[1]);
                    let stream = bh > 0.6 * bw || (water.bbox[3] > 0.97 && water.depth > 0.5 && bh > 0.35);
                    let (dir, speed) = if stream { (90.0, 6.0) } else { (0.0, 1.5) };
                    let lane = self.lane_above(bc.lane, at, dur)?;
                    let e = self.effect_of("water", &[("direction", s(dir)), ("speed", s(speed)), ("ripple", s(if stream { 0.5 } else { 0.3 })), ("scale", s(if stream { 3.0 } else { 5.0 }))]);
                    let id = self.insert_layer(lane, at, dur, "Living water", m::ClipSource::Generator(m::Generator::new(m::GeneratorKind::Adjustment)), vec![e], m::BlendMode::Normal)?;
                    self.region_masks(id, &water, at, 0.008, 0.0, false)?;
                    said.push(format!("water {} as clip {id}, glints on it as clip {gid}", if stream { "flowing towards the camera" } else { "rippling" }));
                }
                "foliage" => {
                    let Some(trees) = reg(Region::Trees) else {
                        skipped.push("foliage (no trees or plants in the picture)".into());
                        continue;
                    };
                    let lane = self.lane_above(bc.lane, at, dur)?;
                    let sway = self.effect_of("noise_displace", &[("amount", s(0.35)), ("scale", s(9.0)), ("speed", s(0.3)), ("octaves", s(2.0))]);
                    let gust = self.effect_of("wave", &[("amplitude", s(0.12)), ("wavelength", s(45.0)), ("angle", s(0.0)), ("speed", s(0.15))]);
                    let id = self.insert_layer(lane, at, dur, "Swaying foliage (living)", m::ClipSource::Generator(m::Generator::new(m::GeneratorKind::Adjustment)), vec![sway, gust], m::BlendMode::Normal)?;
                    self.region_masks(id, &trees, at, 0.01, 0.003, false)?;
                    said.push(format!("foliage swaying as clip {id}"));
                }
                "mist" | "fog" => {
                    let lane = if op.get("front").and_then(Value::as_bool).unwrap_or(false) { self.room(None, at, dur, None)? } else { self.lane_above(bc.lane, at, dur)? };
                    let (_, p) = studio_fx::particles::preset("mist").expect("mist preset");
                    // Over the water if there is any, else low over the ground.
                    let area = reg(Region::Water).or(reg(Region::Ground)).or(reg(Region::Path));
                    let extra: Vec<(String, m::Value)> = if r == "fog" { vec![("amount".into(), s(1.8)), ("size".into(), s(1.4))] } else { vec![] };
                    let id = match &area {
                        Some(ar) => {
                            let mut grown = ar.clone();
                            grown.bbox[1] = (grown.bbox[1] - 0.08).max(0.0);
                            let id = self.particle_layer(p, lane, at, dur, None, scene.as_ref(), &extra)?;
                            // The puffs start over the region; the layer is
                            // not masked, so mist spills softly past it.
                            let e = self.model_clip(id).and_then(|c| c.effects.first().cloned());
                            if let Some(mut e) = e {
                                e.params.insert("emitter".into(), m::Value::Choice("Area".into()));
                                e.params.insert("area_a".into(), m::Value::Point(m::Vec2::new(grown.bbox[0] as f64, grown.bbox[1] as f64)));
                                e.params.insert("area_b".into(), m::Value::Point(m::Vec2::new(grown.bbox[2] as f64, grown.bbox[3] as f64)));
                                let seq = self.doc.seq();
                                let ok = self.doc.apply("Mist", m::Edit::SetEffect { seq, clip: m::ClipId(id as u64), effect: e });
                                self.check(ok, "")?;
                            }
                            id
                        }
                        None => self.particle_layer(p, lane, at, dur, None, scene.as_ref(), &extra)?,
                    };
                    if let Some((a, b)) = ramp {
                        self.animate(&json!({"clip": id, "property": "particles.amount", "keys": [[a, 0.0], [b, if r == "fog" { 1.8 } else { 1.0 }]], "ease": "ease"}))?;
                    }
                    said.push(format!("{r} as clip {id}{}", area.map(|a| format!(" over the {}", a.name)).unwrap_or_default()));
                }
                _ => {}
            }
        }
        if said.is_empty() {
            return Err(format!("nothing to bring to life: {}", skipped.join("; ")));
        }
        Ok(format!(
            "backdrop {back} brought to life over {:.1}s: {}{}. Each is its own layer: tune its effect settings, or remove it. Frame a moment to look, and play a second or two to see it move.",
            dur_s,
            said.join("; "),
            if skipped.is_empty() { String::new() } else { format!(" (skipped: {})", skipped.join("; ")) }
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::Doc;

    fn session() -> Session {
        Session::new(Doc::from_view(&view::demo()))
    }

    fn added(r: &Value) -> Vec<u32> {
        r["changed"]["added"].as_array().map(|v| v.iter().filter_map(|c| c["id"].as_u64()).map(|x| x as u32).collect()).unwrap_or_default()
    }

    #[test]
    fn particles_by_preset_make_a_clear_layer_with_the_preset() {
        let mut s = session();
        let r = s.edit(&[json!({"op": "particles", "preset": "snow", "at": 1.0, "duration": 4.0, "params": {"amount": 1.5, "wind": 10}})], "snow", false).unwrap();
        let id = added(&r)[0];
        let mc = s.model_clip(id).unwrap();
        assert_eq!(mc.duration, 4 * SEC);
        let e = &mc.effects[0];
        assert_eq!(e.kind, "particles");
        assert_eq!(e.params.get("preset"), Some(&m::Value::Choice("Snow".into())));
        assert_eq!(e.params.get("amount"), Some(&m::Value::Scalar(1.5)));
        // Glows screen onto what is below; snow covers.
        assert_eq!(mc.blend, m::BlendMode::Normal);
        let r = s.edit(&[json!({"op": "particles", "preset": "fireflies"})], "ff", false).unwrap();
        assert_eq!(s.model_clip(added(&r)[0]).unwrap().blend, m::BlendMode::Screen);
        let e = s.edit(&[json!({"op": "particles", "preset": "lava lamp"})], "x", false).unwrap_err();
        assert!(e.contains("rain") && e.contains("embers"), "{e}");
    }

    #[test]
    fn a_storm_comes_in_with_dim_light_and_lightning() {
        let mut s = session();
        let back = s.backdrop_at(SEC).expect("the demo has pictures");
        let r = s.edit(&[json!({"op": "living", "recipes": ["storm"], "backdrop": back, "ramp": [0.5, 2.0]})], "storm", false).unwrap();
        let ids = added(&r);
        assert_eq!(ids.len(), 3, "rain, a light layer and lightning: {r}");
        let rain = ids.iter().filter_map(|&i| s.model_clip(i)).find(|c| c.effects.iter().any(|e| e.kind == "particles")).unwrap();
        let path = format!("effect.{}.amount", rain.effects[0].id.0);
        assert_eq!(rain.property_at(&path, 0), Some(m::Value::Scalar(0.0)), "no rain before the ramp");
        assert!(matches!(rain.property_at(&path, micros(3.0)), Some(m::Value::Scalar(v)) if (v - 1.6).abs() < 1e-6));
        let light = ids.iter().filter_map(|&i| s.model_clip(i)).find(|c| c.name.starts_with("Light")).unwrap();
        assert!(light.grade.as_ref().is_some_and(|g| g.enabled), "the light layer's grade is on");
        assert!(matches!(light.property_at("grade.exposure", micros(3.0)), Some(m::Value::Scalar(v)) if v < -0.5));
        let e = s.edit(&[json!({"op": "living", "recipes": ["volcano"], "backdrop": back})], "x", false).unwrap_err();
        assert!(e.contains("recipes:"), "{e}");
    }

    #[test]
    fn region_masks_follow_the_outlines_and_cut_holes() {
        let mut s = session();
        let r = s.edit(&[json!({"op": "particles", "preset": "mist", "at": 0.0, "duration": 2.0})], "mist", false).unwrap();
        let id = added(&r)[0];
        let square = vec![[0.2, 0.5], [0.8, 0.5], [0.8, 0.9], [0.2, 0.9]];
        let hole = vec![[0.4, 0.6], [0.6, 0.6], [0.6, 0.7], [0.4, 0.7]];
        let info = RegionInfo { name: "water".into(), share: 0.2, bbox: [0.2, 0.5, 0.8, 0.9], centre: [0.5, 0.7], depth: 0.7, outlines: vec![(true, hole), (false, square)] };
        let n = s.region_masks(id, &info, SEC / 2, 0.01, 0.0, false).unwrap();
        assert_eq!(n, 2);
        let mc = s.model_clip(id).unwrap();
        assert_eq!(mc.masks[0].mode, m::MaskMode::Add, "the outer outline first");
        assert_eq!(mc.masks[1].mode, m::MaskMode::Subtract, "then the hole");
        // A full-frame layer: frame fractions are layer fractions.
        let p = mc.masks[0].path[1].point;
        assert!((p.x - 0.8).abs() < 1e-3 && (p.y - 0.5).abs() < 1e-3, "{p:?}");
        // Inverted: everything but the region.
        s.region_masks(id, &info, SEC / 2, 0.01, 0.0, true).unwrap();
        let mc = s.model_clip(id).unwrap();
        assert_eq!(mc.masks[2].shape, m::MaskShape::Rectangle);
        assert_eq!(mc.masks[3].mode, m::MaskMode::Subtract);
        assert_eq!(mc.masks[4].mode, m::MaskMode::Add);
    }
}
