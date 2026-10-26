// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The object map: what is in a shot or a picture, where it is in every
//! frame, how far away and how big, what it looks like and how things
//! relate. One map per piece of footage (or picture), cached by content
//! hash, answers every question about objects without looking at the
//! footage again.
//!
//! * [`build`]: finds objects with the open-vocabulary detector on a few
//!   key frames, follows them between those with the motion tracker, reads
//!   depth, colours, text, faces and poses, and separates the camera's
//!   motion from the objects'.
//! * [`geometry`]: depth to metres, positions and sizes in 3D.
//! * [`relations`]: left of, in front of, on, holding, near.
//! * [`space`]: where the frame is free for titles and placed things.
//! * [`moments`]: "find the moment where..." by picture-text similarity.
//! * [`words`]: compact answers.
//!
//! Boxes are normalised to the frame (0..1, origin top-left) so a map does
//! not depend on the resolution it was read at. Times are seconds from the
//! start of the footage the map was made for.

pub mod assoc;
pub mod build;
pub mod colour;
pub mod geometry;
pub mod moments;
pub mod motion;
pub mod relations;
pub mod space;
pub mod words;

use crate::cache::{CacheKey, Cacheable, Reader, ResultCache, Writer};
use serde::{Deserialize, Serialize};

/// Bump when the meaning of the stored data changes.
pub const VERSION: u32 = 1;

/// A box, normalised to the frame.
#[derive(Clone, Copy, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct BBox {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl BBox {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> BBox {
        BBox { x, y, w, h }
    }
    pub fn from_pixels(r: &crate::features::faces::Rect, fw: f32, fh: f32) -> BBox {
        BBox { x: r.x / fw, y: r.y / fh, w: r.w / fw, h: r.h / fh }
    }
    pub fn to_pixels(&self, fw: f32, fh: f32) -> crate::features::faces::Rect {
        crate::features::faces::Rect { x: self.x * fw, y: self.y * fh, w: self.w * fw, h: self.h * fh }
    }
    pub fn right(&self) -> f32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
    pub fn cx(&self) -> f32 {
        self.x + self.w / 2.0
    }
    pub fn cy(&self) -> f32 {
        self.y + self.h / 2.0
    }
    pub fn area(&self) -> f32 {
        self.w.max(0.0) * self.h.max(0.0)
    }
    pub fn inter(&self, o: &BBox) -> f32 {
        let ix = self.right().min(o.right()) - self.x.max(o.x);
        let iy = self.bottom().min(o.bottom()) - self.y.max(o.y);
        ix.max(0.0) * iy.max(0.0)
    }
    pub fn iou(&self, o: &BBox) -> f32 {
        let i = self.inter(o);
        let u = self.area() + o.area() - i;
        if u > 1e-9 { i / u } else { 0.0 }
    }
    /// The share of this box that lies inside `o`.
    pub fn inside(&self, o: &BBox) -> f32 {
        self.inter(o) / self.area().max(1e-9)
    }
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x <= self.right() && y >= self.y && y <= self.bottom()
    }
    pub fn grown(&self, k: f32) -> BBox {
        BBox { x: self.x - self.w * (k - 1.0) / 2.0, y: self.y - self.h * (k - 1.0) / 2.0, w: self.w * k, h: self.h * k }
    }
    pub fn lerp(&self, o: &BBox, t: f32) -> BBox {
        let m = |a: f32, b: f32| a + (b - a) * t;
        BBox { x: m(self.x, o.x), y: m(self.y, o.y), w: m(self.w, o.w), h: m(self.h, o.h) }
    }
    /// Clipped to the frame.
    pub fn clamped(&self) -> BBox {
        let (x0, y0) = (self.x.clamp(0.0, 1.0), self.y.clamp(0.0, 1.0));
        let (x1, y1) = (self.right().clamp(0.0, 1.0), self.bottom().clamp(0.0, 1.0));
        BBox { x: x0, y: y0, w: x1 - x0, h: y1 - y0 }
    }
}

/// What sort of thing it is, which decides how it is described.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Kind {
    Person,
    Animal,
    Vehicle,
    Thing,
    /// Large fixed parts of the place: trees, mountains, buildings.
    Scenery,
}

/// Body points that matter for relations (normalised).
#[derive(Clone, Copy, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct Body {
    pub nose: Option<[f32; 2]>,
    pub left_eye: Option<[f32; 2]>,
    pub right_eye: Option<[f32; 2]>,
    pub left_shoulder: Option<[f32; 2]>,
    pub right_shoulder: Option<[f32; 2]>,
    pub left_wrist: Option<[f32; 2]>,
    pub right_wrist: Option<[f32; 2]>,
    pub left_hip: Option<[f32; 2]>,
    pub right_hip: Option<[f32; 2]>,
    pub left_ankle: Option<[f32; 2]>,
    pub right_ankle: Option<[f32; 2]>,
}

impl Body {
    pub fn wrists(&self) -> Vec<[f32; 2]> {
        [self.left_wrist, self.right_wrist].into_iter().flatten().collect()
    }
    /// Both feet are in the picture (the whole body is visible).
    pub fn full_body(&self) -> bool {
        self.left_ankle.is_some() || self.right_ankle.is_some()
    }
}

/// One object at one moment.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Obs {
    pub t: f32,
    pub bbox: BBox,
    /// The detector found it here (otherwise the tracker carried it).
    pub detected: bool,
    /// Detector score, or the tracker's confidence.
    pub conf: f32,
    /// Relative inverse depth under the object, 0..1 (1 = nearest).
    pub depth: Option<f32>,
    /// Distance along the lens axis, metres.
    pub z: Option<f32>,
    /// Face box (people).
    pub face: Option<BBox>,
    /// Body points (people).
    pub body: Option<Body>,
    /// Head box (animals).
    pub head: Option<BBox>,
}

impl Obs {
    pub fn new(t: f32, bbox: BBox, detected: bool, conf: f32) -> Obs {
        Obs { t, bbox, detected, conf, depth: None, z: None, face: None, body: None, head: None }
    }
}

/// One object followed through the footage.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Object {
    /// Stable within the map: numbered by first appearance, left to right.
    pub id: u32,
    pub label: String,
    /// "woman", "child": what a more specific name said.
    pub kind: Kind,
    pub descriptor: Option<String>,
    /// Mean detector score over the detections.
    pub score: f32,
    /// Other names it was also called, with their best scores.
    pub also: Vec<(String, f32)>,
    pub obs: Vec<Obs>,
    pub enter: f32,
    pub leave: f32,
    /// Came into the picture after the start / left before the end.
    pub enters: bool,
    pub leaves: bool,
    pub colours: Vec<String>,
    pub texts: Vec<String>,
    /// Width and height in metres.
    pub size: Option<[f32; 2]>,
    /// Which way a person or animal faces.
    pub facing: Option<String>,
    /// Movement of the object itself, camera motion taken out.
    pub movement: Option<Movement>,
}

/// How an object moves through the place, apart from the camera.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Movement {
    /// Frame widths per second of the object's own travel (camera removed).
    pub speed: f32,
    /// Direction of its travel across the frame, degrees clockwise from
    /// "right" in picture space (0 = right, 90 = down).
    pub heading: f32,
    /// Metres per second toward (negative) or away from (positive) the lens.
    pub depth_speed: Option<f32>,
    /// Moves on screen only because the camera does.
    pub still: bool,
}

/// A line of text read in the picture.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TextItem {
    pub t: f32,
    pub text: String,
    pub bbox: BBox,
    pub score: f32,
    /// The object it is written on or held by.
    pub object: Option<u32>,
}

/// One sampled frame's camera pose relative to the first sample: shift as a
/// share of the frame width, turn, and log of scale.
#[derive(Clone, Copy, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct CamPose {
    pub t: f32,
    pub x: f32,
    pub y: f32,
    pub angle: f32,
    pub log_scale: f32,
}

/// What the camera does over the footage.
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct CameraMotion {
    pub path: Vec<CamPose>,
    /// "static", "pans right", "tilts up", "pushes in", "handheld", ...
    pub summary: String,
}

/// How metres were obtained.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Scale {
    /// How many known-size things the depth was calibrated on (0 = assumed).
    pub anchors: u32,
    pub hfov_deg: f32,
}

/// Everything known about the objects in one piece of footage.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ObjectMap {
    pub version: u32,
    /// Size the footage was read at.
    pub width: u32,
    pub height: u32,
    pub fps: f32,
    pub duration: f32,
    /// Times of the key frames (where the detector looked).
    pub keyframes: Vec<f32>,
    /// What was searched for.
    pub vocabulary: Vec<String>,
    pub objects: Vec<Object>,
    pub texts: Vec<TextItem>,
    pub camera: CameraMotion,
    pub scale: Scale,
    /// How busy the picture is per cell (16 x 9, 0..255) at each key frame.
    pub detail: Vec<Vec<u8>>,
}

impl ObjectMap {
    pub fn object(&self, id: u32) -> Option<&Object> {
        self.objects.iter().find(|o| o.id == id)
    }

    /// Objects whose label (or a name it was also called, or its
    /// descriptor) matches `name`, ignoring case and a plural "s".
    pub fn named(&self, name: &str) -> Vec<&Object> {
        let want = singular(&name.trim().to_lowercase());
        self.objects.iter().filter(|o| matches_name(o, &want)).collect()
    }

    /// The cache key of a map for footage hash `media` read with `params`.
    pub fn cache_key(media: &str, params: &str) -> CacheKey {
        CacheKey::new(media, format!("objects-v{VERSION}"), params)
    }

    pub fn load(cache: &ResultCache, key: &CacheKey) -> Option<ObjectMap> {
        cache.get::<ObjectMap>(key)
    }

    pub fn store(&self, cache: &ResultCache, key: &CacheKey) -> std::io::Result<()> {
        cache.put(key, self)
    }
}

fn singular(s: &str) -> String {
    let s = s.trim_start_matches("a ").trim_start_matches("an ").trim_start_matches("the ");
    if s.len() > 3 && s.ends_with("ies") {
        format!("{}y", &s[..s.len() - 3])
    } else if s.len() > 3 && s.ends_with("es") && (s.ends_with("ses") || s.ends_with("xes") || s.ends_with("ches") || s.ends_with("shes")) {
        s[..s.len() - 2].to_string()
    } else if s.len() > 2 && s.ends_with('s') && !s.ends_with("ss") {
        s[..s.len() - 1].to_string()
    } else if s == "people" {
        "person".to_string()
    } else {
        s.to_string()
    }
}

fn matches_name(o: &Object, want: &str) -> bool {
    let is = |s: &str| singular(&s.to_lowercase()) == want;
    // The whole name, or one word of it ("box" for "cardboard box").
    let word = |s: &str| s.split_whitespace().any(|w| singular(&w.to_lowercase()) == want);
    is(&o.label) || o.descriptor.as_deref().is_some_and(is) || o.also.iter().any(|(n, _)| is(n)) || word(&o.label)
}

impl Object {
    /// The observation at `t`, interpolated between neighbours; `None`
    /// outside the object's time on screen.
    pub fn at(&self, t: f32) -> Option<Obs> {
        let first = self.obs.first()?;
        let last = self.obs.last()?;
        if t < first.t - 1e-4 || t > last.t + 1e-4 {
            return None;
        }
        let i = self.obs.partition_point(|o| o.t <= t);
        if i == 0 {
            return Some(first.clone());
        }
        if i >= self.obs.len() {
            return Some(last.clone());
        }
        let (a, b) = (&self.obs[i - 1], &self.obs[i]);
        let k = if b.t > a.t { ((t - a.t) / (b.t - a.t)).clamp(0.0, 1.0) } else { 0.0 };
        let near = if k < 0.5 { a } else { b };
        let mix = |p: Option<f32>, q: Option<f32>| match (p, q) {
            (Some(p), Some(q)) => Some(p + (q - p) * k),
            (p, q) => p.or(q),
        };
        Some(Obs { t, bbox: a.bbox.lerp(&b.bbox, k), detected: false, conf: a.conf.min(b.conf), depth: mix(a.depth, b.depth), z: mix(a.z, b.z), face: near.face, body: near.body, head: near.head })
    }

    /// Whether it is on screen at `t`.
    pub fn visible(&self, t: f32) -> bool {
        t >= self.enter - 1e-4 && t <= self.leave + 1e-4
    }

    /// How much of the frame it fills on average.
    pub fn prominence(&self) -> f32 {
        let n = self.obs.len().max(1) as f32;
        let area: f32 = self.obs.iter().map(|o| o.bbox.area()).sum::<f32>() / n;
        let span = (self.leave - self.enter).max(0.04);
        // Sizeable, sure and present for long things first.
        (area.sqrt() * 0.7 + 0.3) * self.score * (0.5 + span.min(8.0) / 16.0)
    }
}

impl Cacheable for ObjectMap {
    const TAG: &'static str = "objectmap";
    fn encode(&self, w: &mut Writer) {
        w.str(&serde_json::to_string(self).unwrap_or_default());
    }
    fn decode(r: &mut Reader) -> Option<Self> {
        let m: ObjectMap = serde_json::from_str(&r.str()?).ok()?;
        (m.version == VERSION).then_some(m)
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use super::*;

    pub fn obs(t: f32, x: f32, y: f32, w: f32, h: f32) -> Obs {
        Obs::new(t, BBox::new(x, y, w, h), true, 0.9)
    }

    pub fn object(id: u32, label: &str, kind: Kind, obs: Vec<Obs>) -> Object {
        let (enter, leave) = (obs.first().map_or(0.0, |o| o.t), obs.last().map_or(0.0, |o| o.t));
        Object { id, label: label.into(), kind, descriptor: None, score: 0.8, also: Vec::new(), obs, enter, leave, enters: false, leaves: false, colours: Vec::new(), texts: Vec::new(), size: None, facing: None, movement: None }
    }

    pub fn map(objects: Vec<Object>) -> ObjectMap {
        let duration = objects.iter().map(|o| o.leave).fold(0.0, f32::max);
        ObjectMap { version: VERSION, width: 1920, height: 1080, fps: 30.0, duration, keyframes: vec![0.0], vocabulary: vec!["person".into()], objects, texts: Vec::new(), camera: CameraMotion::default(), scale: Scale { anchors: 0, hfov_deg: 63.0 }, detail: Vec::new() }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;

    #[test]
    fn boxes_overlap_sensibly() {
        let a = BBox::new(0.0, 0.0, 0.5, 0.5);
        let b = BBox::new(0.25, 0.25, 0.5, 0.5);
        assert!((a.iou(&b) - 0.0625 / 0.4375).abs() < 1e-5);
        assert!((a.inside(&BBox::new(0.0, 0.0, 1.0, 1.0)) - 1.0).abs() < 1e-6);
        assert_eq!(a.iou(&BBox::new(0.6, 0.6, 0.2, 0.2)), 0.0);
        let g = a.grown(2.0);
        assert!((g.w - 1.0).abs() < 1e-6 && (g.x + 0.25).abs() < 1e-6);
        assert_eq!(g.clamped(), BBox::new(0.0, 0.0, 0.75, 0.75));
    }

    #[test]
    fn an_object_is_interpolated_between_observations() {
        let o = object(1, "dog", Kind::Animal, vec![obs(0.0, 0.0, 0.2, 0.2, 0.2), obs(2.0, 0.6, 0.2, 0.2, 0.2)]);
        let m = o.at(1.0).unwrap();
        assert!((m.bbox.x - 0.3).abs() < 1e-6);
        assert!(o.at(2.5).is_none() && o.at(-0.1).is_none());
        assert!(o.visible(2.0) && !o.visible(2.1));
    }

    #[test]
    fn names_match_plurals_and_alternatives() {
        let mut dog = object(1, "dog", Kind::Animal, vec![obs(0.0, 0.0, 0.0, 0.1, 0.1)]);
        dog.also.push(("puppy".into(), 0.3));
        let mut woman = object(2, "person", Kind::Person, vec![obs(0.0, 0.5, 0.0, 0.1, 0.1)]);
        woman.descriptor = Some("woman".into());
        let m = map(vec![dog, woman]);
        assert_eq!(m.named("Dogs").len(), 1);
        assert_eq!(m.named("the puppy")[0].id, 1);
        assert_eq!(m.named("woman")[0].id, 2);
        assert_eq!(m.named("people")[0].id, 2);
        let mut boxy = object(3, "cardboard box", Kind::Thing, vec![obs(0.0, 0.1, 0.1, 0.1, 0.1)]);
        boxy.also.clear();
        let m2 = map(vec![boxy]);
        assert_eq!(m2.named("box").len(), 1);
        assert_eq!(m2.named("boxes").len(), 1);
        assert!(m2.named("card").is_empty(), "a part of a word is not a match");
        assert!(m.named("cat").is_empty());
    }

    #[test]
    fn the_map_survives_the_cache() {
        let dir = std::env::temp_dir().join(format!("studio-objects-cache-{}", std::process::id()));
        let cache = ResultCache::new(&dir);
        let mut o = object(1, "dog", Kind::Animal, vec![obs(0.0, 0.1, 0.2, 0.3, 0.4)]);
        o.colours = vec!["brown".into()];
        let m = map(vec![o]);
        let key = ObjectMap::cache_key("hash", "k=3");
        assert!(ObjectMap::load(&cache, &key).is_none());
        m.store(&cache, &key).unwrap();
        assert_eq!(ObjectMap::load(&cache, &key), Some(m.clone()));
        // Other parameters or media are other entries.
        assert!(ObjectMap::load(&cache, &ObjectMap::cache_key("hash", "k=4")).is_none());
        assert!(ObjectMap::load(&cache, &ObjectMap::cache_key("other", "k=3")).is_none());
        let _ = std::fs::remove_dir_all(dir);
    }
}
