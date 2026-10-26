// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Clips and everything attached to them: transform, audio, effects,
//! grade, masks, motion presets and keyframe tracks.

use crate::id::{ClipId, EffectId, GroupId, LaneId, LinkId, MaskId, MediaId, SequenceId};
use crate::keyframe::Track;
use crate::puppet::{CornerPin, Cut, Pin, Puppet};
use crate::skeleton::{BoneLink, Skeleton};
use crate::sequence::Marker;
use crate::space::{AutoOrient, Material};
use crate::time::{Micros, SECOND, TimeRange};
use crate::value::{Extra, ParamMap, Rgba, Value, Vec2};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A piece of media or generated content placed on a lane.
///
/// Timing is stored as `start`, `duration` (both on the sequence) and
/// `source_in` (in the source); the source out point is derived from the
/// speed with [`Clip::source_out`]. Storing only one of "duration" and
/// "source out" means the two can never disagree.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Clip {
    /// Unique id.
    pub id: ClipId,
    /// The lane the clip sits on.
    pub lane: LaneId,
    /// Position on the sequence.
    pub start: Micros,
    /// Length on the sequence; always positive.
    pub duration: Micros,
    /// First source instant used (for media and nested sequences).
    #[serde(default)]
    pub source_in: Micros,
    /// What the clip shows or plays.
    pub source: ClipSource,
    /// Display name.
    #[serde(default)]
    pub name: String,
    /// Disabled clips stay on the timeline but are not rendered.
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub enabled: bool,
    /// Playback speed.
    #[serde(default, skip_serializing_if = "Speed::is_normal")]
    pub speed: Speed,
    /// Play the source backwards.
    #[serde(default, skip_serializing_if = "is_false")]
    pub reversed: bool,
    /// Sound settings.
    #[serde(default)]
    pub audio: AudioProps,
    /// Picture placement.
    #[serde(default)]
    pub transform: Transform,
    /// How the clip combines with lanes below it.
    #[serde(default, skip_serializing_if = "BlendMode::is_normal")]
    pub blend: BlendMode,
    /// Video effects, applied in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effects: Vec<Effect>,
    /// Colour grade, applied after the effects.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grade: Option<Grade>,
    /// Masks, combined in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub masks: Vec<Mask>,
    /// Puppet pins bending the picture (after masks).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub puppet: Option<Puppet>,
    /// Cuts that split the puppet's and skeleton's deformation mesh, so
    /// parts (legs, lips, arms) move apart instead of stretching together.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cuts: Vec<Cut>,
    /// Corner pin (perspective and skew), after the puppet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub corner_pin: Option<CornerPin>,
    /// Bones bending the picture and carrying part layers (with the
    /// puppet's mesh); keyframe paths `bone.<id>.<setting>`,
    /// `ik.<id>.<setting>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skeleton: Option<Skeleton>,
    /// How a 3D layer takes light, casts shadows and extrudes (`None` =
    /// the defaults); keyframe paths `material.<field>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub material: Option<Material>,
    /// Keyframe tracks keyed by property path (see [`Clip::get_property`]).
    /// A property with a track ignores its static value.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub keyframes: BTreeMap<String, Track>,
    /// Properties worked out from other properties, keyed by property
    /// path (see [`crate::driver`]); a driven property ignores its keys
    /// except as the formula's `value`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub drivers: BTreeMap<String, crate::driver::Driver>,
    /// Entry, exit and loop motion presets.
    #[serde(default, skip_serializing_if = "Motion::is_empty")]
    pub motion: Motion,
    /// Markers; times are relative to the clip start.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub markers: Vec<Marker>,
    /// Clips sharing a link id move and trim together (e.g. picture and its
    /// detached sound).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<LinkId>,
    /// Clips sharing a group id are selected together.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<GroupId>,
    /// Follows another clip's position, scale and rotation (parenting).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<Parent>,
    /// Label colour shown on the timeline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<Rgba>,
    /// Free-form data for tools and plug-ins; the model never interprets it.
    #[serde(default, skip_serializing_if = "Extra::is_empty")]
    pub meta: Extra,
    /// Fields this version does not know about.
    #[serde(flatten)]
    pub extra: Extra,
}

fn yes() -> bool {
    true
}
fn is_true(b: &bool) -> bool {
    *b
}
fn is_false(b: &bool) -> bool {
    !*b
}

impl Clip {
    /// A clip with default settings.
    pub fn new(id: ClipId, lane: LaneId, start: Micros, duration: Micros, source: ClipSource) -> Self {
        Self {
            id,
            lane,
            start,
            duration,
            source_in: 0,
            source,
            name: String::new(),
            enabled: true,
            speed: Speed::default(),
            reversed: false,
            audio: AudioProps::default(),
            transform: Transform::default(),
            blend: BlendMode::Normal,
            effects: Vec::new(),
            grade: None,
            masks: Vec::new(),
            puppet: None,
            cuts: Vec::new(),
            corner_pin: None,
            skeleton: None,
            material: None,
            keyframes: BTreeMap::new(),
            drivers: BTreeMap::new(),
            motion: Motion::default(),
            markers: Vec::new(),
            link: None,
            group: None,
            parent: None,
            label: None,
            meta: Extra::new(),
            extra: Extra::new(),
        }
    }

    /// A clip playing `media` from `source_in` for `duration`.
    pub fn media(id: ClipId, lane: LaneId, start: Micros, media: MediaId, source_in: Micros, duration: Micros) -> Self {
        let mut c = Self::new(id, lane, start, duration, ClipSource::Media(MediaRef::new(media)));
        c.source_in = source_in;
        c
    }

    /// An empty spacer clip for the spine.
    pub fn gap(id: ClipId, lane: LaneId, start: Micros, duration: Micros) -> Self {
        Self::new(id, lane, start, duration, ClipSource::Gap)
    }

    /// The clip's span on the sequence.
    pub fn range(&self) -> TimeRange {
        TimeRange::new(self.start, self.duration)
    }

    /// First instant after the clip.
    pub fn end(&self) -> Micros {
        self.start + self.duration
    }

    /// Whether the clip is a spine spacer.
    pub fn is_gap(&self) -> bool {
        matches!(self.source, ClipSource::Gap)
    }

    /// The media item the clip plays, if any.
    pub fn media_id(&self) -> Option<MediaId> {
        match &self.source {
            ClipSource::Media(m) => Some(m.media),
            _ => None,
        }
    }

    /// The nested sequence the clip plays, if any.
    pub fn sequence_id(&self) -> Option<SequenceId> {
        match &self.source {
            ClipSource::Sequence(s) => Some(s.sequence),
            _ => None,
        }
    }

    /// Whether the source has a timeline of its own (media or a nested
    /// sequence), so source in/out and handles matter.
    pub fn has_source_time(&self) -> bool {
        matches!(self.source, ClipSource::Media(_) | ClipSource::Sequence(_))
    }

    /// Source time consumed by the first `t` microseconds of the clip,
    /// ignoring direction.
    pub fn source_offset(&self, t: Micros) -> f64 {
        self.speed.integrate(t)
    }

    /// Source length covered by the whole clip.
    pub fn source_len(&self) -> Micros {
        self.source_offset(self.duration).round() as Micros
    }

    /// One past the last source instant used.
    pub fn source_out(&self) -> Micros {
        self.source_in + self.source_len()
    }

    /// The source time shown at sequence time `t` (which should lie inside
    /// the clip), accounting for speed, speed curves, reverse and freeze.
    pub fn source_time_at(&self, t: Micros) -> Micros {
        let rel = (t - self.start).clamp(0, self.duration);
        // Time remapping: a keyed (or driven) track saying which source
        // second shows at each moment of the clip.
        if let Some(s) = self.keyframes.get(TIME_REMAP).and_then(|tr| tr.eval(rel)).and_then(|v| v.as_f64()) {
            return ((s * SECOND as f64).round() as Micros).max(0);
        }
        let off = self.source_offset(rel);
        if self.reversed {
            // The last source instant is one microsecond before `source_out`.
            let len = self.source_offset(self.duration);
            (self.source_in as f64 + (len - off - 1.0).max(0.0)).round() as Micros
        } else {
            (self.source_in as f64 + off).round() as Micros
        }
    }

    /// Like [`Clip::source_time_at`], but times outside the clip continue
    /// into the unused source at the edge speed. Renderers use this for the
    /// part of a transition that runs past a clip's edge (the handles).
    pub fn source_time_extended(&self, t: Micros) -> Micros {
        let (dir, over) = if t < self.start {
            (-1.0, (self.start - t) as f64 * self.speed.rate_at(0))
        } else if t >= self.end() {
            (1.0, (t - self.end()) as f64 * self.speed.rate_at(self.duration))
        } else {
            return self.source_time_at(t);
        };
        // Forward clips extend before the in point and after the out point;
        // reversed clips run the other way through their source.
        let dir = if self.reversed { -dir } else { dir };
        let edge = if dir > 0.0 { self.source_out() as f64 } else { self.source_in as f64 };
        (edge + dir * over).round() as Micros
    }

    /// Unused source available before the in point and after the out
    /// point, converted to sequence time at the clip's edge speeds.
    /// `None` means unlimited (stills, generators, freeze frames, merged
    /// clips).
    /// `source_duration` is the media or nested-sequence length.
    pub fn handles(&self, source_duration: Option<Micros>) -> (Option<Micros>, Option<Micros>) {
        // A merged clip has no footage to run out of: past its content its
        // first and last pictures hold (renderers clamp to the content), so
        // a transition never needs the parts padded first.
        // A flattened merged clip (its render standing in for it) keeps
        // that: its first and last frames hold.
        if matches!(self.source, ClipSource::Sequence(_)) || self.extra.contains_key("flattened_from") {
            return (None, None);
        }
        let Some(total) = source_duration.filter(|_| self.has_source_time()) else {
            return (None, None);
        };
        let head_src = self.source_in.max(0);
        let tail_src = (total - self.source_out()).max(0);
        // Reversed clips run through the source backwards, so the source
        // tail sits before the clip's start on the timeline.
        let (before_src, after_src) = if self.reversed { (tail_src, head_src) } else { (head_src, tail_src) };
        let to_seq = |src: Micros, rate: f64| if rate <= 0.0 { None } else { Some((src as f64 / rate).floor() as Micros) };
        (to_seq(before_src, self.speed.rate_at(0)), to_seq(after_src, self.speed.rate_at(self.duration)))
    }

    /// Reads a property's static value by path.
    ///
    /// Paths: `transform.position`, `transform.scale_x`, `transform.scale_y`,
    /// `transform.rotation`, `transform.anchor`, `transform.opacity`,
    /// 3D: `transform.position_z`, `transform.rotation_x`,
    /// `transform.rotation_y`, `transform.orientation_x|y|z`,
    /// `transform.scale_z`, `transform.anchor_z`, `material.<field>`;
    /// `transform.crop_left|right|top|bottom`, `audio.volume_db`,
    /// `audio.pan`, `effect.<id>.<param>`, `audio_effect.<id>.<param>`,
    /// `mask.<id>.<param>`, `grade.<param>`, `generator.<param>`;
    /// `puppet.<pin>.<setting>`, `corner_pin.<corner>`,
    /// `mask.<id>.point.<n>` (a path point, its handles with it) and
    /// `mask.<id>.pin.<pin>.<setting>`; `bone.<id>.rotation|offset`,
    /// `ik.<id>.target|blend`.
    pub fn get_property(&self, path: &str) -> Option<Value> {
        if let Some(v) = self.pin_property(path) {
            return v;
        }
        if let Some(key) = path.strip_prefix("material.") {
            return self.material.clone().unwrap_or_default().get(key);
        }
        if let Some(v) = transform_value(&self.transform, path) {
            return Some(v);
        }
        Some(match path {
            "audio.volume_db" => Value::Scalar(self.audio.volume_db),
            "audio.pan" => Value::Scalar(self.audio.pan),
            // Unkeyed, the source second the clip opens on.
            TIME_REMAP => Value::Scalar(self.source_in as f64 / SECOND as f64),
            _ => return self.param_slot(path).and_then(|(map, key)| map.get(key).cloned()),
        })
    }

    /// Writes a property's static value by path. Parameters of effects,
    /// masks, grades and generators may be set even if not stored yet (they
    /// then override the implementation's default). Returns `false` for an
    /// unknown path or a value of a different kind than the stored one.
    pub fn set_property(&mut self, path: &str, value: Value) -> bool {
        if let Some(old) = self.get_property(path)
            && !old.same_kind(&value)
        {
            return false;
        }
        if let Some(done) = self.set_pin_property(path, &value) {
            return done;
        }
        if let Some(key) = path.strip_prefix("material.") {
            let mut m = self.material.clone().unwrap_or_default();
            if !m.set(key, &value) {
                return false;
            }
            self.material = Some(m);
            return true;
        }
        if set_transform_value(&mut self.transform, path, &value) {
            return true;
        }
        let scalar = value.as_f64();
        match path {
            "audio.volume_db" => self.audio.volume_db = scalar.unwrap_or(self.audio.volume_db),
            "audio.pan" => self.audio.pan = scalar.unwrap_or(self.audio.pan),
            _ => match self.param_slot_mut(path) {
                Some((map, key)) => {
                    map.insert(key.to_string(), value);
                }
                None => return false,
            },
        }
        true
    }

    /// The value of a property at clip-relative time `t`: the keyframe
    /// track's value if the property has one, otherwise the static value.
    pub fn property_at(&self, path: &str, t: Micros) -> Option<Value> {
        match self.keyframes.get(path).and_then(|tr| tr.eval(t)) {
            Some(v) => Some(v),
            None => self.get_property(path),
        }
    }

    /// The transform evaluated at clip-relative time `t` (keyframes applied;
    /// motion presets are applied by the renderer on top).
    pub fn transform_at(&self, t: Micros) -> Transform {
        // Keyed values written straight into the transform: copying the
        // whole clip (a puppet's thousands of keys) for every look at a
        // moving layer is what the renderer, the checks and the timeline
        // did many times a frame.
        let mut out = self.transform.clone();
        let mut other = false;
        for (path, tr) in self.keyframes.iter().filter(|(k, _)| k.starts_with("transform.")) {
            let Some(v) = tr.eval(t) else { continue };
            match transform_value(&out, path) {
                Some(old) if old.same_kind(&v) => {
                    set_transform_value(&mut out, path, &v);
                }
                Some(_) => {}
                None => other = true,
            }
        }
        // A keyed path the transform does not hold itself: the long way.
        if other {
            let mut probe = self.clone();
            for path in self.keyframes.keys().filter(|k| k.starts_with("transform.")) {
                if let Some(v) = self.property_at(path, t) {
                    probe.set_property(path, v);
                }
            }
            out = probe.transform;
        }
        out
    }

    /// A generator's parameters at clip-relative time `t`, keyframes
    /// applied (empty for other clips).
    pub fn generator_params_at(&self, t: Micros) -> ParamMap {
        let ClipSource::Generator(g) = &self.source else { return ParamMap::new() };
        let mut p = g.params.clone();
        for (path, tr) in &self.keyframes {
            if let Some(k) = path.strip_prefix("generator.")
                && let Some(v) = tr.eval(t)
            {
                p.insert(k.to_string(), v);
            }
        }
        p
    }

    /// The material with its animated fields evaluated at clip-relative
    /// time `t`.
    pub fn material_at(&self, t: Micros) -> Material {
        let mut m = self.material.clone().unwrap_or_default();
        for key in Material::ANIMATED {
            if let Some(v) = self.keyframes.get(&format!("material.{key}")).and_then(|tr| tr.eval(t)) {
                m.set(key, &v);
            }
        }
        m
    }

    /// Whether the clip is a camera or a light (no picture of its own).
    pub fn is_camera_or_light(&self) -> bool {
        matches!(&self.source, ClipSource::Generator(g) if matches!(g.kind, GeneratorKind::Camera | GeneratorKind::Light))
    }

    /// The puppet with its animated settings evaluated at clip-relative
    /// time `t`.
    pub fn puppet_at(&self, t: Micros) -> Option<Puppet> {
        let mut p = self.puppet.clone()?;
        for pin in &mut p.pins {
            self.pin_at(&format!("puppet.{}.", pin.id), pin, t);
        }
        Some(p)
    }

    /// The skeleton with its animated settings evaluated at clip-relative
    /// time `t`, before IK (see [`Skeleton::solve`]).
    pub fn skeleton_keys_at(&self, t: Micros) -> Option<Skeleton> {
        let mut s = self.skeleton.clone()?;
        if self.keyframes.keys().any(|k| k.starts_with("bone.") || k.starts_with("ik.")) {
            for b in &mut s.bones {
                for key in crate::skeleton::Bone::ANIMATED {
                    if let Some(v) = self.keyframes.get(&format!("bone.{}.{key}", b.id)).and_then(|tr| tr.eval(t)) {
                        b.set(key, &v);
                    }
                }
            }
            for h in &mut s.ik {
                for key in crate::skeleton::IkHandle::ANIMATED {
                    if let Some(v) = self.keyframes.get(&format!("ik.{}.{key}", h.id)).and_then(|tr| tr.eval(t)) {
                        h.set(key, &v);
                    }
                }
            }
        }
        Some(s)
    }

    /// The skeleton posed at clip-relative time `t`: keys evaluated, IK
    /// solved (the bones' rotations are the solved ones).
    pub fn skeleton_at(&self, t: Micros) -> Option<Skeleton> {
        let mut s = self.skeleton_keys_at(t)?;
        s.solve();
        Some(s)
    }

    /// The corner pin with its corners evaluated at clip-relative time `t`.
    pub fn corner_pin_at(&self, t: Micros) -> Option<CornerPin> {
        let mut c = self.corner_pin.clone()?;
        for name in CornerPin::CORNERS {
            if let Some(Value::Point(p)) = self.keyframes.get(&format!("corner_pin.{name}")).and_then(|tr| tr.eval(t))
                && let Some(slot) = c.corner_mut(name)
            {
                *slot = p;
            }
        }
        Some(c)
    }

    /// A mask with its animated path points and pins evaluated at
    /// clip-relative time `t` (its parameters: [`Clip::mask_params_at`]).
    pub fn mask_at(&self, mask: &Mask, t: Micros) -> Mask {
        let mut m = mask.clone();
        let prefix = format!("mask.{}.", mask.id.0);
        for (i, pt) in m.path.iter_mut().enumerate() {
            if let Some(Value::Point(p)) = self.keyframes.get(&format!("{prefix}point.{i}")).and_then(|tr| tr.eval(t)) {
                pt.point = p;
            }
        }
        for pin in &mut m.pins {
            self.pin_at(&format!("{prefix}pin.{}.", pin.id), pin, t);
        }
        m
    }

    fn pin_at(&self, prefix: &str, pin: &mut Pin, t: Micros) {
        for key in Pin::ANIMATED {
            if let Some(v) = self.keyframes.get(&format!("{prefix}{key}")).and_then(|tr| tr.eval(t)) {
                pin.set(key, &v);
            }
        }
    }

    /// Splits a pin, corner or mask-point path: the pin (or corner, or
    /// point) and the setting. `None` when `path` is not one of those.
    fn pin_slot<'p>(path: &'p str) -> Option<PinSlot<'p>> {
        if let Some(rest) = path.strip_prefix("puppet.") {
            let (id, key) = rest.split_once('.')?;
            return Some(PinSlot::Puppet(id.parse().ok()?, key));
        }
        if let Some(corner) = path.strip_prefix("corner_pin.") {
            return Some(PinSlot::Corner(corner));
        }
        if let Some(rest) = path.strip_prefix("bone.") {
            let (id, key) = rest.split_once('.')?;
            return Some(PinSlot::Bone(id.parse().ok()?, key));
        }
        if let Some(rest) = path.strip_prefix("ik.") {
            let (id, key) = rest.split_once('.')?;
            return Some(PinSlot::Ik(id.parse().ok()?, key));
        }
        let rest = path.strip_prefix("mask.")?;
        let (mask, rest) = rest.split_once('.')?;
        let mask: u64 = mask.parse().ok()?;
        if let Some(n) = rest.strip_prefix("point.") {
            return Some(PinSlot::MaskPoint(mask, n.parse().ok()?));
        }
        let (id, key) = rest.strip_prefix("pin.")?.split_once('.')?;
        Some(PinSlot::MaskPin(mask, id.parse().ok()?, key))
    }

    /// `Some(None)`: a pin path that names nothing; `None`: not a pin path.
    fn pin_property(&self, path: &str) -> Option<Option<Value>> {
        let slot = Self::pin_slot(path)?;
        Some(match slot {
            PinSlot::Puppet(id, key) => self.puppet.as_ref().and_then(|p| p.pin(id)).and_then(|p| p.get(key)),
            PinSlot::Corner(name) => self.corner_pin.clone().and_then(|mut c| c.corner_mut(name).map(|p| Value::Point(*p))),
            PinSlot::MaskPoint(m, i) => self.masks.iter().find(|k| k.id.0 == m).and_then(|k| k.path.get(i)).map(|p| Value::Point(p.point)),
            PinSlot::MaskPin(m, id, key) => self.masks.iter().find(|k| k.id.0 == m).and_then(|k| k.pins.iter().find(|p| p.id == id)).and_then(|p| p.get(key)),
            PinSlot::Bone(id, key) => self.skeleton.as_ref().and_then(|s| s.bone(id)).and_then(|b| b.get(key)),
            PinSlot::Ik(id, key) => self.skeleton.as_ref().and_then(|s| s.handle(id)).and_then(|h| h.get(key)),
        })
    }

    fn set_pin_property(&mut self, path: &str, v: &Value) -> Option<bool> {
        let slot = Self::pin_slot(path)?;
        Some(match slot {
            PinSlot::Puppet(id, key) => self.puppet.as_mut().and_then(|p| p.pin_mut(id)).is_some_and(|p| p.set(key, v)),
            PinSlot::Corner(name) => match (self.corner_pin.as_mut().and_then(|c| c.corner_mut(name)), v) {
                (Some(slot), Value::Point(p)) => {
                    *slot = *p;
                    true
                }
                _ => false,
            },
            PinSlot::MaskPoint(m, i) => match (self.masks.iter_mut().find(|k| k.id.0 == m).and_then(|k| k.path.get_mut(i)), v) {
                (Some(pt), Value::Point(p)) => {
                    pt.point = *p;
                    true
                }
                _ => false,
            },
            PinSlot::MaskPin(m, id, key) => self.masks.iter_mut().find(|k| k.id.0 == m).and_then(|k| k.pins.iter_mut().find(|p| p.id == id)).is_some_and(|p| p.set(key, v)),
            PinSlot::Bone(id, key) => self.skeleton.as_mut().and_then(|s| s.bone_mut(id)).is_some_and(|b| b.set(key, v)),
            PinSlot::Ik(id, key) => self.skeleton.as_mut().and_then(|s| s.handle_mut(id)).is_some_and(|h| h.set(key, v)),
        })
    }

    /// Effect parameters evaluated at clip-relative time `t`.
    pub fn effect_params_at(&self, effect: &Effect, t: Micros) -> ParamMap {
        self.params_at(&format!("effect.{}.", effect.id.0), &effect.params, t)
    }

    /// Mask parameters evaluated at clip-relative time `t`.
    pub fn mask_params_at(&self, mask: &Mask, t: Micros) -> ParamMap {
        self.params_at(&format!("mask.{}.", mask.id.0), &mask.params, t)
    }

    /// Grade parameters evaluated at clip-relative time `t`.
    pub fn grade_params_at(&self, t: Micros) -> ParamMap {
        match &self.grade {
            Some(g) => self.params_at("grade.", &g.params, t),
            None => ParamMap::new(),
        }
    }

    fn params_at(&self, prefix: &str, params: &ParamMap, t: Micros) -> ParamMap {
        params
            .iter()
            .map(|(k, v)| {
                let path = format!("{prefix}{k}");
                let v = self.keyframes.get(&path).and_then(|tr| tr.eval(t)).unwrap_or_else(|| v.clone());
                (k.clone(), v)
            })
            .collect()
    }

    /// Splits `effect.<id>.<param>` style paths into the map and key.
    fn param_slot<'a, 'p>(&'a self, path: &'p str) -> Option<(&'a ParamMap, &'p str)> {
        let (head, rest) = path.split_once('.')?;
        match head {
            "grade" => Some((&self.grade.as_ref()?.params, rest)),
            "generator" => match &self.source {
                ClipSource::Generator(g) => Some((&g.params, rest)),
                _ => None,
            },
            "effect" | "audio_effect" | "mask" => {
                let (id, key) = rest.split_once('.')?;
                let id: u64 = id.parse().ok()?;
                let map = match head {
                    "effect" => &self.effects.iter().find(|e| e.id.0 == id)?.params,
                    "audio_effect" => &self.audio.effects.iter().find(|e| e.id.0 == id)?.params,
                    _ => &self.masks.iter().find(|m| m.id.0 == id)?.params,
                };
                Some((map, key))
            }
            _ => None,
        }
    }

    fn param_slot_mut<'a, 'p>(&'a mut self, path: &'p str) -> Option<(&'a mut ParamMap, &'p str)> {
        let (head, rest) = path.split_once('.')?;
        match head {
            "grade" => Some((&mut self.grade.as_mut()?.params, rest)),
            "generator" => match &mut self.source {
                ClipSource::Generator(g) => Some((&mut g.params, rest)),
                _ => None,
            },
            "effect" | "audio_effect" | "mask" => {
                let (id, key) = rest.split_once('.')?;
                let id: u64 = id.parse().ok()?;
                let map = match head {
                    "effect" => &mut self.effects.iter_mut().find(|e| e.id.0 == id)?.params,
                    "audio_effect" => &mut self.audio.effects.iter_mut().find(|e| e.id.0 == id)?.params,
                    _ => &mut self.masks.iter_mut().find(|m| m.id.0 == id)?.params,
                };
                Some((map, key))
            }
            _ => None,
        }
    }

    /// Removes keyframe tracks whose path starts with `prefix` (used when
    /// an effect or mask is deleted).
    pub(crate) fn drop_tracks_with_prefix(&mut self, prefix: &str) {
        self.keyframes.retain(|k, _| !k.starts_with(prefix));
    }
}

/// A property path that names a pin's setting, a corner or a mask point.
enum PinSlot<'p> {
    Puppet(u32, &'p str),
    Corner(&'p str),
    MaskPoint(u64, usize),
    MaskPin(u64, u32, &'p str),
    Bone(u32, &'p str),
    Ik(u32, &'p str),
}

/// What a clip shows or plays.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClipSource {
    /// A media bin item.
    Media(MediaRef),
    /// Another sequence of the same project (a compound clip).
    Sequence(SequenceRef),
    /// Generated content (solid, gradient, text, shape, adjustment).
    Generator(Generator),
    /// Empty space on the spine.
    Gap,
}

/// Reference to a media item and which of its streams the clip uses.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MediaRef {
    /// The media item.
    pub media: MediaId,
    /// Use the picture.
    #[serde(default = "yes")]
    pub video: bool,
    /// Use the sound.
    #[serde(default = "yes")]
    pub audio: bool,
    /// Fields this version does not know about.
    #[serde(flatten)]
    pub extra: Extra,
}

impl MediaRef {
    /// Uses both picture and sound.
    pub fn new(media: MediaId) -> Self {
        Self { media, video: true, audio: true, extra: Extra::new() }
    }
}

/// Reference to a nested sequence.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SequenceRef {
    /// The nested sequence.
    pub sequence: SequenceId,
    /// Fields this version does not know about.
    #[serde(flatten)]
    pub extra: Extra,
}

/// Generated content.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Generator {
    /// What to generate.
    pub kind: GeneratorKind,
    /// Parameters defined by the implementing crate (colour, gradient
    /// stops, shape size, font, …).
    #[serde(default, skip_serializing_if = "ParamMap::is_empty")]
    pub params: ParamMap,
    /// Styled text runs, for text generators.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub text: Vec<TextRun>,
    /// Fields this version does not know about.
    #[serde(flatten)]
    pub extra: Extra,
}

impl Generator {
    /// A generator with no parameters set.
    pub fn new(kind: GeneratorKind) -> Self {
        Self { kind, params: ParamMap::new(), text: Vec::new(), extra: Extra::new() }
    }

    /// A text generator showing `text` in the default style.
    pub fn text(text: impl Into<String>) -> Self {
        let mut g = Self::new(GeneratorKind::Text);
        g.text.push(TextRun { text: text.into(), style: ParamMap::new() });
        g
    }

    /// All text runs joined.
    pub fn plain_text(&self) -> String {
        self.text.iter().map(|r| r.text.as_str()).collect()
    }
}

/// Kinds of generated content.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GeneratorKind {
    /// A flat colour.
    Solid,
    /// A colour gradient.
    Gradient,
    /// Styled text.
    Text,
    /// A vector shape.
    Shape,
    /// No picture of its own: its effects and grade apply to the lanes below.
    Adjustment,
    /// A 3D camera: where it stands and what it looks at decide how 3D
    /// layers are seen (see [`crate::space::CameraSettings`]).
    Camera,
    /// A 3D light (see [`crate::space::LightSettings`]).
    Light,
    /// A 3D model: a `.smesh` file in the project's `Models` folder
    /// (parameter `file`), lit by the scene's lights and seen through its
    /// camera. Parameters `base_colour`, `metallic`, `roughness`,
    /// `emissive` and `opacity` override its materials; `smooth` (0..1)
    /// rounds its shading.
    Model,
}

/// A run of text sharing one style.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TextRun {
    /// The characters.
    pub text: String,
    /// Style overrides for this run (font, size, fill, …), by name.
    #[serde(default, skip_serializing_if = "ParamMap::is_empty")]
    pub style: ParamMap,
}

/// The keyframe path of time remapping: source seconds over clip time
/// (see [`Clip::source_time_at`]).
pub const TIME_REMAP: &str = "time_remap";

/// Playback speed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Speed {
    /// A fixed multiplier; 1 is normal, 0 freezes on the in point.
    Constant(f64),
    /// A multiplier that varies over the clip: a scalar track whose times
    /// are relative to the clip start. Values below zero count as zero.
    Curve(Track),
}

impl Default for Speed {
    fn default() -> Self {
        Speed::Constant(1.0)
    }
}

impl Speed {
    fn is_normal(&self) -> bool {
        *self == Speed::Constant(1.0)
    }

    /// Whether the clip freezes on one frame.
    pub fn is_freeze(&self) -> bool {
        *self == Speed::Constant(0.0)
    }

    /// The multiplier at clip-relative time `t`.
    pub fn rate_at(&self, t: Micros) -> f64 {
        match self {
            Speed::Constant(r) => r.max(0.0),
            Speed::Curve(tr) => tr.eval(t).and_then(|v| v.as_f64()).unwrap_or(1.0).max(0.0),
        }
    }

    /// Source time consumed by the first `t` microseconds.
    pub fn integrate(&self, t: Micros) -> f64 {
        match self {
            Speed::Constant(r) => r.max(0.0) * t as f64,
            Speed::Curve(_) => {
                if t <= 0 {
                    return 0.0;
                }
                // Composite Simpson's rule; one sample pair per millisecond
                // (bounded) keeps the error far below one microsecond for
                // the smooth curves keyframes produce.
                let n = ((t / 1000).clamp(8, 8192) as usize) * 2;
                let h = t as f64 / n as f64;
                let f = |i: usize| self.rate_at((i as f64 * h).round() as Micros);
                let mut sum = f(0) + f(n);
                for i in 1..n {
                    sum += if i % 2 == 1 { 4.0 } else { 2.0 } * f(i);
                }
                sum * h / 3.0
            }
        }
    }

    /// The sequence duration that consumes `source_len` of source, found by
    /// bisection for curves. `None` when the speed is zero everywhere.
    pub fn duration_for(&self, source_len: Micros) -> Option<Micros> {
        match self {
            Speed::Constant(r) if *r > 0.0 => Some((source_len as f64 / r).round() as Micros),
            Speed::Constant(_) => None,
            Speed::Curve(_) => {
                let (mut lo, mut hi) = (0i64, 1i64);
                while self.integrate(hi) < source_len as f64 {
                    hi *= 2;
                    if hi > 1 << 42 {
                        return None;
                    }
                }
                while hi - lo > 1 {
                    let mid = lo + (hi - lo) / 2;
                    if self.integrate(mid) < source_len as f64 { lo = mid } else { hi = mid }
                }
                Some(hi)
            }
        }
    }
}

/// Sound settings of a clip.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AudioProps {
    /// Gain in decibels (0 = unchanged); keyframable as `audio.volume_db`.
    #[serde(default)]
    pub volume_db: f64,
    /// Stereo position, −1 (left) to 1 (right); keyframable as `audio.pan`.
    #[serde(default)]
    pub pan: f64,
    /// Silence the clip.
    #[serde(default, skip_serializing_if = "is_false")]
    pub muted: bool,
    /// Gain ramp from silence at the clip start.
    #[serde(default, skip_serializing_if = "Fade::is_none")]
    pub fade_in: Fade,
    /// Gain ramp to silence at the clip end.
    #[serde(default, skip_serializing_if = "Fade::is_none")]
    pub fade_out: Fade,
    /// Keep the pitch when the speed changes.
    #[serde(default = "yes")]
    pub preserve_pitch: bool,
    /// Audio effects, applied in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effects: Vec<Effect>,
}

impl Default for AudioProps {
    fn default() -> Self {
        Self { volume_db: 0.0, pan: 0.0, muted: false, fade_in: Fade::default(), fade_out: Fade::default(), preserve_pitch: true, effects: Vec::new() }
    }
}

/// An audio gain ramp.
#[derive(Clone, Copy, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct Fade {
    /// Length of the ramp; zero means no fade.
    pub duration: Micros,
    /// Shape of the ramp.
    #[serde(default)]
    pub curve: FadeCurve,
}

impl Fade {
    fn is_none(&self) -> bool {
        self.duration == 0
    }
}

/// Shape of a fade.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FadeCurve {
    /// Gain changes linearly.
    #[default]
    Linear,
    /// Constant perceived loudness when crossfading (sine/cosine law).
    EqualPower,
    /// Slow start, fast finish.
    Exponential,
    /// Fast start, slow finish.
    Logarithmic,
    /// Slow at both ends.
    SCurve,
}

/// Picture placement on the canvas.
///
/// `position` is where the anchor lands, in canvas pixels measured from
/// the canvas centre. `anchor` is in source pixels from the source centre.
/// A scale of 1 shows the source fitted inside the canvas.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Transform {
    /// Anchor position relative to the canvas centre.
    #[serde(default)]
    pub position: Vec2,
    /// Horizontal scale.
    #[serde(default = "one")]
    pub scale_x: f64,
    /// Vertical scale.
    #[serde(default = "one")]
    pub scale_y: f64,
    /// Clockwise rotation in degrees.
    #[serde(default)]
    pub rotation: f64,
    /// Rotation and scale centre relative to the source centre.
    #[serde(default)]
    pub anchor: Vec2,
    /// 0 (invisible) to 1 (opaque).
    #[serde(default = "one")]
    pub opacity: f64,
    /// Fractions of the source removed from each edge.
    #[serde(default)]
    pub crop: Crop,
    /// Mirror left-right.
    #[serde(default, skip_serializing_if = "is_false")]
    pub flip_x: bool,
    /// Mirror top-bottom.
    #[serde(default, skip_serializing_if = "is_false")]
    pub flip_y: bool,
    /// 3D switch: the layer is a card in 3D space, seen through the camera
    /// and lit by lights (see [`crate::space`]).
    #[serde(default, skip_serializing_if = "is_false")]
    pub three_d: bool,
    /// Depth in pixels; positive is further away.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub position_z: f64,
    /// Tilt in degrees: positive tips the top edge away.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub rotation_x: f64,
    /// Turn in degrees: positive turns the right edge away.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub rotation_y: f64,
    /// Resting orientation (x, y, z degrees), applied after the rotations.
    #[serde(default, skip_serializing_if = "is_zero3")]
    pub orientation: [f64; 3],
    /// Depth scale (thickness of an extruded layer, distance of children).
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub scale_z: f64,
    /// Depth of the anchor (the centre a 3D layer turns about).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub anchor_z: f64,
    /// Turns by itself: along its motion path or towards the camera.
    #[serde(default, skip_serializing_if = "AutoOrient::is_off")]
    pub auto_orient: AutoOrient,
}

fn is_zero(v: &f64) -> bool {
    *v == 0.0
}
fn is_one(v: &f64) -> bool {
    *v == 1.0
}
fn is_zero3(v: &[f64; 3]) -> bool {
    *v == [0.0; 3]
}

fn one() -> f64 {
    1.0
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            position: Vec2::ZERO,
            scale_x: 1.0,
            scale_y: 1.0,
            rotation: 0.0,
            anchor: Vec2::ZERO,
            opacity: 1.0,
            crop: Crop::default(),
            flip_x: false,
            flip_y: false,
            three_d: false,
            position_z: 0.0,
            rotation_x: 0.0,
            rotation_y: 0.0,
            orientation: [0.0; 3],
            scale_z: 1.0,
            anchor_z: 0.0,
            auto_orient: AutoOrient::Off,
        }
    }
}

/// Edge crop as fractions (0..1) of the source size.
#[derive(Clone, Copy, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct Crop {
    /// Removed from the left.
    pub left: f64,
    /// Removed from the right.
    pub right: f64,
    /// Removed from the top.
    pub top: f64,
    /// Removed from the bottom.
    pub bottom: f64,
}

/// How a clip's picture combines with what is below it (W3C Compositing
/// and Blending Level 1 modes).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[allow(missing_docs)]
pub enum BlendMode {
    #[default]
    Normal,
    Add,
    Multiply,
    Screen,
    Overlay,
    SoftLight,
    HardLight,
    ColourDodge,
    ColourBurn,
    Darken,
    Lighten,
    Difference,
    Exclusion,
    Hue,
    Saturation,
    Colour,
    Luminosity,
    /// Track matte: the layers below show only where this one is opaque
    /// (this one itself is not drawn). Within a merged clip it cuts what
    /// that clip holds; on the timeline, everything below.
    StencilAlpha,
    /// The layers below show only where this one is transparent.
    SilhouetteAlpha,
    /// The layers below show as much as this one is bright.
    StencilLuma,
    /// The layers below show as much as this one is dark.
    SilhouetteLuma,
}

/// A clip's parent: the clip it follows. It moves, scales and turns by
/// however much the parent has moved, scaled and turned since they were
/// linked, about the parent's position, so linking never makes it jump.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Parent {
    /// The clip followed (on the same sequence).
    pub clip: ClipId,
    /// The parent's position when they were linked.
    pub rest_position: Vec2,
    /// The parent's scale (x, y) when they were linked.
    pub rest_scale: Vec2,
    /// The parent's rotation when they were linked (degrees).
    pub rest_rotation: f64,
    /// A 3D parent's depth when they were linked.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub rest_z: f64,
    /// A 3D parent's X rotation when they were linked (degrees).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub rest_rotation_x: f64,
    /// A 3D parent's Y rotation when they were linked (degrees).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub rest_rotation_y: f64,
    /// Follows one bone of the parent's skeleton as well (a part layer:
    /// an arm on the shoulder bone).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bone: Option<BoneLink>,
}

impl BlendMode {
    /// One of the track-matte modes.
    pub fn is_matte(self) -> bool {
        matches!(self, BlendMode::StencilAlpha | BlendMode::SilhouetteAlpha | BlendMode::StencilLuma | BlendMode::SilhouetteLuma)
    }

    fn is_normal(&self) -> bool {
        *self == BlendMode::Normal
    }
}

/// One effect in a clip's stack.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Effect {
    /// Unique id; keyframe paths use it (`effect.<id>.<param>`).
    pub id: EffectId,
    /// Effect name as registered by the `fx` crate, e.g. `"gaussian_blur"`.
    pub kind: String,
    /// Bypassed effects keep their settings but do nothing.
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Parameter values; missing ones take the effect's defaults.
    #[serde(default)]
    pub params: ParamMap,
    /// Fields this version does not know about.
    #[serde(flatten)]
    pub extra: Extra,
}

impl Effect {
    /// An enabled effect with default parameters.
    pub fn new(id: EffectId, kind: impl Into<String>) -> Self {
        Self { id, kind: kind.into(), enabled: true, params: ParamMap::new(), extra: Extra::new() }
    }
}

/// A clip's colour grade. Parameter names are defined by the `colour`
/// crate; keyframe paths are `grade.<param>`.
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct Grade {
    /// Bypass switch.
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Grade parameters (exposure, wheels, curves, HSL bands, …).
    #[serde(default)]
    pub params: ParamMap,
    /// Path of a LUT file applied as part of the grade.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lut: Option<String>,
    /// Fields this version does not know about.
    #[serde(flatten)]
    pub extra: Extra,
}

impl Clip {
    /// The angle a multicam clip shows: the lane of its multicam sequence
    /// drawn alone (`meta.multicam.angle`).
    pub fn multicam_angle(&self) -> Option<LaneId> {
        self.meta.get("multicam")?.get("angle")?.as_u64().map(LaneId)
    }

    /// The angle lanes of a multicam clip, in angle order.
    pub fn multicam_angles(&self) -> Option<Vec<LaneId>> {
        let a = self.meta.get("multicam")?.get("angles")?.as_array()?;
        Some(a.iter().filter_map(|v| v.as_u64()).map(LaneId).collect())
    }

    /// Makes this clip a multicam clip over `angles` (lanes of its
    /// sequence, with their names), showing `angle`.
    pub fn set_multicam(&mut self, angle: LaneId, angles: &[LaneId], names: &[String]) {
        self.meta.insert("multicam".into(), serde_json::json!({ "angle": angle.0, "angles": angles.iter().map(|l| l.0).collect::<Vec<_>>(), "names": names }));
    }

    /// Shows another angle of a multicam clip (no change for other clips).
    pub fn set_multicam_angle(&mut self, angle: LaneId) {
        if let Some(m) = self.meta.get_mut("multicam") {
            m["angle"] = serde_json::json!(angle.0);
        }
    }
}

/// A secondary correction ("node") applied after the clip's grade, in
/// order: its own grade, limited by an HSL qualifier and a window, mixed
/// in by `mix`. Kept in [`Grade::extra`] under `"nodes"` (see
/// [`Grade::nodes`]).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GradeNode {
    /// Label shown in the node list.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// Bypass switch.
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Grade parameters, as for the clip's grade (`exposure`, wheels,
    /// curves, HSL bands, `cdl_*`, `look`, …).
    #[serde(default, skip_serializing_if = "ParamMap::is_empty")]
    pub params: ParamMap,
    /// A LUT file applied in this node.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lut: Option<String>,
    /// HSL qualifier: `preset` (`skin`, `sky`, `foliage`, `highlights`,
    /// …) and/or `hue`, `hue_width`, `hue_soft`, `sat_low`, `sat_high`,
    /// `sat_soft`, `lum_low`, `lum_high`, `lum_soft`, `invert`. Empty:
    /// the whole picture.
    #[serde(default, skip_serializing_if = "ParamMap::is_empty")]
    pub qualifier: ParamMap,
    /// A mask of this clip used as the node's power window. Such a mask
    /// shapes the node only; it no longer limits where the clip shows.
    /// Track it like any mask to follow a subject.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<MaskId>,
    /// Grade outside the window instead of inside.
    #[serde(default, skip_serializing_if = "is_false")]
    pub window_invert: bool,
    /// Strength 0..1.
    #[serde(default = "one")]
    pub mix: f64,
    /// Softens the key (qualifier × window): blur radius in pixels of the
    /// clip's picture.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub key_blur: f64,
}

impl Default for GradeNode {
    fn default() -> Self {
        Self { name: String::new(), enabled: true, params: ParamMap::new(), lut: None, qualifier: ParamMap::new(), window: None, window_invert: false, mix: 1.0, key_blur: 0.0 }
    }
}

impl Grade {
    /// The secondary correction nodes, in order.
    pub fn nodes(&self) -> Vec<GradeNode> {
        self.extra.get("nodes").and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default()
    }

    /// Replaces the nodes (none removes the key).
    pub fn set_nodes(&mut self, nodes: &[GradeNode]) {
        if nodes.is_empty() {
            self.extra.remove("nodes");
        } else {
            self.extra.insert("nodes".into(), serde_json::to_value(nodes).unwrap_or_default());
        }
    }

    /// Masks used as node windows (they do not limit the clip's picture).
    pub fn window_masks(&self) -> Vec<MaskId> {
        self.nodes().iter().filter_map(|n| n.window).collect()
    }
}

/// A mask limiting where a clip is visible.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Mask {
    /// Unique id; keyframe paths use it (`mask.<id>.<param>`).
    pub id: MaskId,
    /// Shape.
    pub shape: MaskShape,
    /// How this mask combines with the ones before it.
    #[serde(default)]
    pub mode: MaskMode,
    /// Swap inside and outside.
    #[serde(default, skip_serializing_if = "is_false")]
    pub invert: bool,
    /// Shape parameters: `position`, `size`, `rotation`, `feather`,
    /// `expansion`, `opacity`, plus shape-specific ones such as `roundness`.
    #[serde(default)]
    pub params: ParamMap,
    /// Points of a free Bézier path (only for [`MaskShape::Path`]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub path: Vec<PathPoint>,
    /// Pins bending the path (path masks only), in the same 0..1 layer
    /// coordinates; keyframe paths `mask.<id>.pin.<pin>.<setting>`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pins: Vec<Pin>,
    /// Fields this version does not know about.
    #[serde(flatten)]
    pub extra: Extra,
}

impl Mask {
    /// A mask with default parameters.
    pub fn new(id: MaskId, shape: MaskShape) -> Self {
        Self { id, shape, mode: MaskMode::Add, invert: false, params: ParamMap::new(), path: Vec::new(), pins: Vec::new(), extra: Extra::new() }
    }

    /// The shape as a Bézier path in 0..1 layer coordinates (rectangles,
    /// ellipses, stars and hearts are traced; a path is itself; a line or
    /// mirror has no outline and gives `None`). Rotation is applied.
    pub fn outline(&self) -> Option<Vec<PathPoint>> {
        let num = |k: &str, d: f64| self.params.get(k).and_then(Value::as_f64).unwrap_or(d);
        let pt = |k: &str, d: Vec2| match self.params.get(k) {
            Some(Value::Point(p)) => *p,
            _ => d,
        };
        let c = pt("position", Vec2::new(0.5, 0.5));
        let size = pt("size", Vec2::new(0.5, 0.5));
        let (hx, hy) = (size.x / 2.0, size.y / 2.0);
        let (s, co) = num("rotation", 0.0).to_radians().sin_cos();
        let turn = |x: f64, y: f64| Vec2::new(c.x + x * co - y * s, c.y + x * s + y * co);
        let turn_v = |x: f64, y: f64| Vec2::new(x * co - y * s, x * s + y * co);
        let corner = |x: f64, y: f64| PathPoint { point: turn(x, y), handle_in: Vec2::ZERO, handle_out: Vec2::ZERO };
        Some(match self.shape {
            MaskShape::Path => self.path.clone(),
            MaskShape::Rectangle => vec![corner(-hx, -hy), corner(hx, -hy), corner(hx, hy), corner(-hx, hy)],
            MaskShape::Ellipse => {
                // Four arcs, each a cubic with the usual 0.5523 handles.
                let k = 0.552_284_75;
                vec![
                    PathPoint { point: turn(0.0, -hy), handle_in: turn_v(-hx * k, 0.0), handle_out: turn_v(hx * k, 0.0) },
                    PathPoint { point: turn(hx, 0.0), handle_in: turn_v(0.0, -hy * k), handle_out: turn_v(0.0, hy * k) },
                    PathPoint { point: turn(0.0, hy), handle_in: turn_v(hx * k, 0.0), handle_out: turn_v(-hx * k, 0.0) },
                    PathPoint { point: turn(-hx, 0.0), handle_in: turn_v(0.0, hy * k), handle_out: turn_v(0.0, -hy * k) },
                ]
            }
            MaskShape::Star => (0..10)
                .map(|i| {
                    let r = if i % 2 == 0 { 1.0 } else { 0.42 };
                    let a = -std::f64::consts::FRAC_PI_2 + i as f64 * std::f64::consts::PI / 5.0;
                    corner(r * a.cos() * hx, r * a.sin() * hy)
                })
                .collect(),
            MaskShape::Heart => [(0.0, -0.35), (0.5, -0.85), (1.0, -0.35), (0.0, 1.0), (-1.0, -0.35), (-0.5, -0.85)].iter().map(|&(x, y)| corner(x * hx, y * hy)).collect(),
            MaskShape::Line | MaskShape::Mirror => return None,
        })
    }
}

/// Mask shapes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[allow(missing_docs)]
pub enum MaskShape {
    Rectangle,
    Ellipse,
    /// Splits the frame along a line.
    Line,
    /// A band with mirrored edges.
    Mirror,
    Star,
    Heart,
    /// Free Bézier path from [`Mask::path`].
    Path,
}

/// How masks combine.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[allow(missing_docs)]
pub enum MaskMode {
    #[default]
    Add,
    Subtract,
    Intersect,
}

/// A point of a Bézier mask path, with handles relative to the point.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PathPoint {
    /// The point.
    pub point: Vec2,
    /// Incoming handle offset.
    #[serde(default)]
    pub handle_in: Vec2,
    /// Outgoing handle offset.
    #[serde(default)]
    pub handle_out: Vec2,
}

/// Entry, exit and loop motion presets of a clip.
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct Motion {
    /// Plays from the clip start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry: Option<MotionPreset>,
    /// Plays up to the clip end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit: Option<MotionPreset>,
    /// Repeats between entry and exit; `duration` is one cycle.
    #[serde(default, rename = "loop", skip_serializing_if = "Option::is_none")]
    pub looping: Option<MotionPreset>,
}

impl Motion {
    fn is_empty(&self) -> bool {
        self.entry.is_none() && self.exit.is_none() && self.looping.is_none()
    }
}

/// A motion preset reference, implemented by the `fx` crate.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MotionPreset {
    /// Preset name, e.g. `"rise"`.
    pub kind: String,
    /// Length (one cycle for loops).
    pub duration: Micros,
    /// Preset parameters (easing, distance, …).
    #[serde(default, skip_serializing_if = "ParamMap::is_empty")]
    pub params: ParamMap,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keyframe::{Interp, Keyframe};

    fn clip() -> Clip {
        Clip::media(ClipId(1), LaneId(2), 1_000_000, MediaId(3), 500_000, 2_000_000)
    }

    #[test]
    fn source_mapping() {
        let mut c = clip();
        assert_eq!(c.source_out(), 2_500_000);
        assert_eq!(c.source_time_at(1_000_000), 500_000);
        assert_eq!(c.source_time_at(2_000_000), 1_500_000);
        c.speed = Speed::Constant(2.0);
        assert_eq!(c.source_out(), 4_500_000);
        assert_eq!(c.source_time_at(2_000_000), 2_500_000);
        c.reversed = true;
        assert_eq!(c.source_time_at(1_000_000), 4_499_999);
        c.speed = Speed::Constant(0.0);
        c.reversed = false;
        assert_eq!(c.source_time_at(2_500_000), 500_000);
    }

    #[test]
    fn extended_source_time() {
        let mut c = clip(); // 1–3 s on the timeline, source 0.5–2.5 s
        assert_eq!(c.source_time_extended(500_000), 0);
        assert_eq!(c.source_time_extended(3_250_000), 2_750_000);
        assert_eq!(c.source_time_extended(2_000_000), c.source_time_at(2_000_000));
        c.reversed = true;
        assert_eq!(c.source_time_extended(500_000), 3_000_000);
        assert_eq!(c.source_time_extended(3_250_000), 250_000);
    }

    #[test]
    fn handles() {
        let c = clip();
        assert_eq!(c.handles(Some(10_000_000)), (Some(500_000), Some(7_500_000)));
        let mut r = c.clone();
        r.reversed = true;
        assert_eq!(r.handles(Some(10_000_000)), (Some(7_500_000), Some(500_000)));
        let g = Clip::gap(ClipId(9), LaneId(2), 0, 10);
        assert_eq!(g.handles(Some(5)), (None, None));
        // Merged clips, and the render that stands in for one, hold their
        // edge pictures: a transition never runs out of frames.
        let mut merged = c.clone();
        merged.source = ClipSource::Sequence(SequenceRef { sequence: crate::SequenceId(4), extra: Default::default() });
        assert_eq!(merged.handles(Some(merged.duration)), (None, None));
        let mut flat = c.clone();
        flat.source_in = 0;
        flat.extra.insert("flattened_from".into(), serde_json::json!(4));
        assert_eq!(flat.handles(Some(flat.duration)), (None, None));
    }

    #[test]
    fn speed_curve_integrates() {
        let tr = Track::new(vec![Keyframe::new(0, 1.0, Interp::Linear), Keyframe::new(1_000_000, 3.0, Interp::Linear)]);
        let s = Speed::Curve(tr);
        // Average speed 2 over one second.
        assert!((s.integrate(1_000_000) - 2_000_000.0).abs() < 1.0);
        let d = s.duration_for(2_000_000).unwrap();
        assert!((d - 1_000_000).abs() <= 1, "{d}");
    }

    #[test]
    fn properties() {
        let mut c = clip();
        c.effects.push(Effect::new(EffectId(7), "blur"));
        assert!(c.set_property("effect.7.radius", Value::Scalar(4.0)));
        assert_eq!(c.get_property("effect.7.radius"), Some(Value::Scalar(4.0)));
        assert!(!c.set_property("transform.opacity", Value::Bool(true)));
        assert!(c.set_property("transform.opacity", Value::Scalar(0.5)));
        assert!(c.get_property("nope").is_none());
        c.keyframes.insert(
            "transform.opacity".into(),
            Track::new(vec![Keyframe::new(0, 0.0, Interp::Linear), Keyframe::new(100, 1.0, Interp::Linear)]),
        );
        assert_eq!(c.transform_at(50).opacity, 0.5);
        assert_eq!(c.transform.opacity, 0.5);
    }

    /// The transform at a time, read straight from the keys, is what
    /// writing each keyed value into a copy of the clip gives.
    #[test]
    fn transform_at_matches_keyed_properties() {
        let mut c = Clip::new(ClipId(1), LaneId(1), 0, 1000, ClipSource::Gap);
        let lin = |a: Value, b: Value| Track::new(vec![Keyframe::new(0, a, Interp::Linear), Keyframe::new(1000, b, Interp::Linear)]);
        c.keyframes.insert("transform.position".into(), lin(Value::Point(Vec2::new(0.0, 0.0)), Value::Point(Vec2::new(100.0, -50.0))));
        c.keyframes.insert("transform.rotation".into(), lin(Value::Angle(0.0), Value::Angle(90.0)));
        c.keyframes.insert("transform.scale_x".into(), lin(Value::Scalar(1.0), Value::Scalar(3.0)));
        c.keyframes.insert("transform.crop_left".into(), lin(Value::Scalar(0.0), Value::Scalar(0.2)));
        c.keyframes.insert("transform.position_z".into(), lin(Value::Scalar(0.0), Value::Scalar(-400.0)));
        c.keyframes.insert("transform.flip_x".into(), Track::new(vec![Keyframe::new(0, false, Interp::Hold), Keyframe::new(500, true, Interp::Hold)]));
        // A key of the wrong kind is skipped, as set_property refuses it.
        c.keyframes.insert("transform.opacity".into(), Track::new(vec![Keyframe::new(0, true, Interp::Hold)]));
        c.keyframes.insert("effect.1.radius".into(), lin(Value::Scalar(0.0), Value::Scalar(9.0)));
        for t in [0, 250, 500, 999] {
            let mut probe = c.clone();
            for path in c.keyframes.keys().filter(|k| k.starts_with("transform.")) {
                if let Some(v) = c.property_at(path, t) {
                    probe.set_property(path, v);
                }
            }
            assert_eq!(c.transform_at(t), probe.transform, "at {t}");
        }
        assert!(c.transform_at(999).flip_x && c.transform_at(999).position.x > 99.0);
    }

    #[test]
    fn json_round_trip_keeps_unknown_fields() {
        let mut c = clip();
        c.extra.insert("future".into(), serde_json::json!({"a": 1}));
        let s = serde_json::to_string(&c).unwrap();
        let mut v: serde_json::Value = serde_json::from_str(&s).unwrap();
        v["source"]["shiny"] = serde_json::json!(true);
        let back: Clip = serde_json::from_value(v).unwrap();
        assert_eq!(back.extra["future"], serde_json::json!({"a": 1}));
        let ClipSource::Media(m) = &back.source else { panic!() };
        assert_eq!(m.extra["shiny"], serde_json::json!(true));
    }
}

/// A `transform.*` property of `t` by path (`None` for any other path).
fn transform_value(t: &Transform, path: &str) -> Option<Value> {
    Some(match path {
        "transform.position_z" => Value::Scalar(t.position_z),
        "transform.rotation_x" => Value::Angle(t.rotation_x),
        "transform.rotation_y" => Value::Angle(t.rotation_y),
        "transform.orientation_x" => Value::Angle(t.orientation[0]),
        "transform.orientation_y" => Value::Angle(t.orientation[1]),
        "transform.orientation_z" => Value::Angle(t.orientation[2]),
        "transform.scale_z" => Value::Scalar(t.scale_z),
        "transform.anchor_z" => Value::Scalar(t.anchor_z),
        "transform.position" => Value::Point(t.position),
        "transform.scale_x" => Value::Scalar(t.scale_x),
        "transform.scale_y" => Value::Scalar(t.scale_y),
        "transform.rotation" => Value::Angle(t.rotation),
        "transform.anchor" => Value::Point(t.anchor),
        "transform.opacity" => Value::Scalar(t.opacity),
        "transform.flip_x" => Value::Bool(t.flip_x),
        "transform.flip_y" => Value::Bool(t.flip_y),
        "transform.crop_left" => Value::Scalar(t.crop.left),
        "transform.crop_right" => Value::Scalar(t.crop.right),
        "transform.crop_top" => Value::Scalar(t.crop.top),
        "transform.crop_bottom" => Value::Scalar(t.crop.bottom),
        _ => return None,
    })
}

/// Writes a `transform.*` property of `t` by path; `false` for any other
/// path.
fn set_transform_value(t: &mut Transform, path: &str, value: &Value) -> bool {
    let scalar = value.as_f64();
    match path {
        "transform.position_z" => t.position_z = scalar.unwrap_or(t.position_z),
        "transform.rotation_x" => t.rotation_x = scalar.unwrap_or(t.rotation_x),
        "transform.rotation_y" => t.rotation_y = scalar.unwrap_or(t.rotation_y),
        "transform.orientation_x" => t.orientation[0] = scalar.unwrap_or(t.orientation[0]),
        "transform.orientation_y" => t.orientation[1] = scalar.unwrap_or(t.orientation[1]),
        "transform.orientation_z" => t.orientation[2] = scalar.unwrap_or(t.orientation[2]),
        "transform.scale_z" => t.scale_z = scalar.unwrap_or(t.scale_z),
        "transform.anchor_z" => t.anchor_z = scalar.unwrap_or(t.anchor_z),
        "transform.position" => t.position = value.as_point().unwrap_or(t.position),
        "transform.anchor" => t.anchor = value.as_point().unwrap_or(t.anchor),
        "transform.rotation" => t.rotation = scalar.unwrap_or(t.rotation),
        "transform.scale_x" => t.scale_x = scalar.unwrap_or(t.scale_x),
        "transform.scale_y" => t.scale_y = scalar.unwrap_or(t.scale_y),
        "transform.opacity" => t.opacity = scalar.unwrap_or(t.opacity),
        "transform.flip_x" => t.flip_x = value.as_bool().unwrap_or(t.flip_x),
        "transform.flip_y" => t.flip_y = value.as_bool().unwrap_or(t.flip_y),
        "transform.crop_left" => t.crop.left = scalar.unwrap_or(t.crop.left),
        "transform.crop_right" => t.crop.right = scalar.unwrap_or(t.crop.right),
        "transform.crop_top" => t.crop.top = scalar.unwrap_or(t.crop.top),
        "transform.crop_bottom" => t.crop.bottom = scalar.unwrap_or(t.crop.bottom),
        _ => return false,
    }
    true
}
