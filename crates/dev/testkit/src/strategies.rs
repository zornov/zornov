// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `proptest` strategies for Studio's data: projects built through edits
//! (so every generated project satisfies the model's invariants), clips,
//! keyframe tracks, and effect parameter sets from the `fx` catalogue.
//!
//! Strategies produce plain plans ([`ProjectPlan`], [`ClipPlan`]) that
//! `build` into model objects, so shrinking stays meaningful and failures
//! print a readable description.

use proptest::prelude::*;
use studio_fx::{ParamDef, ParamKind, ParamSet, ParamValue};
use studio_model::*;

/// One second in microseconds.
pub const S: Micros = SECOND;

/// A media item to put in the bin.
#[derive(Clone, Debug)]
pub enum MediaPlan {
    /// Video with sound, `duration` µs, at 1920×1080 30 fps.
    Video { duration: Micros },
    /// Stereo sound, `duration` µs.
    Audio { duration: Micros },
    /// A still image (unbounded duration).
    Image,
}

impl MediaPlan {
    pub fn info(&self) -> MediaInfo {
        match self {
            MediaPlan::Video { duration } => MediaInfo {
                kind: MediaKind::Video,
                container: "mp4".into(),
                duration: Some(*duration),
                video: Some(VideoInfo { codec: "h264".into(), width: 1920, height: 1080, frame_rate: Some(FrameRate::FPS_30), ..Default::default() }),
                audio: Some(AudioInfo { codec: "aac".into(), sample_rate: 48_000, channels: 2, layout: "stereo".into() }),
                extra: Default::default(),
            },
            MediaPlan::Audio { duration } => MediaInfo {
                kind: MediaKind::Audio,
                container: "wav".into(),
                duration: Some(*duration),
                audio: Some(AudioInfo { codec: "pcm_s16le".into(), sample_rate: 48_000, channels: 2, layout: "stereo".into() }),
                ..Default::default()
            },
            MediaPlan::Image => MediaInfo { kind: MediaKind::Image, container: "png".into(), video: Some(VideoInfo { codec: "png".into(), width: 800, height: 600, ..Default::default() }), ..Default::default() },
        }
    }

    pub fn duration(&self) -> Option<Micros> {
        match self {
            MediaPlan::Video { duration } | MediaPlan::Audio { duration } => Some(*duration),
            MediaPlan::Image => None,
        }
    }
}

/// A clip to place: which media (index into the plan's media list), where
/// in the source, how long, with what attributes.
#[derive(Clone, Debug)]
pub struct ClipPlan {
    pub media: usize,
    /// Fraction of the media duration where the clip starts in the source.
    pub source_in_frac: f64,
    /// Fraction of the remaining media the clip covers (0 < f ≤ 1).
    pub length_frac: f64,
    pub opacity: f64,
    pub scale: f64,
    pub rotation: f64,
    pub position: (f64, f64),
    pub volume_db: f64,
    pub speed: f64,
    pub reversed: bool,
    pub enabled: bool,
    pub blend: BlendMode,
    /// Keyframes on `transform.opacity` as fractions of the clip length and values.
    pub opacity_keys: Vec<(f64, f64, Interp)>,
}

/// A whole project.
#[derive(Clone, Debug)]
pub struct ProjectPlan {
    pub frame_rate: FrameRate,
    pub media: Vec<MediaPlan>,
    /// Spine clips in order.
    pub spine: Vec<ClipPlan>,
    /// Connected clips: (plan, lane index 0..visual_lanes, start fraction of the spine length).
    pub connected: Vec<(ClipPlan, usize, f64)>,
    pub visual_lanes: usize,
    pub audio_lanes: usize,
    /// Audio clips: (plan, lane index, start fraction).
    pub audio: Vec<(ClipPlan, usize, f64)>,
    /// Transitions after spine clip `i`: (index, duration fraction of the shorter neighbour).
    pub transitions: Vec<(usize, f64)>,
    pub markers: Vec<f64>,
}

/// What `ProjectPlan::build` produced.
pub struct Built {
    pub project: Project,
    pub seq: SequenceId,
    pub spine: LaneId,
    pub visual: Vec<LaneId>,
    pub audio: Vec<LaneId>,
    pub media: Vec<MediaId>,
    pub spine_clips: Vec<ClipId>,
    pub connected_clips: Vec<ClipId>,
    pub audio_clips: Vec<ClipId>,
    pub transitions: Vec<TransitionId>,
}

impl ClipPlan {
    fn make(&self, id: ClipId, lane: LaneId, media: MediaId, media_dur: Option<Micros>, rate: FrameRate) -> Clip {
        let (source_in, duration) = match media_dur {
            Some(d) => {
                let d = d.max(2 * rate.frame_duration());
                let sin = rate.snap_floor((d as f64 * self.source_in_frac * 0.8) as Micros);
                let remaining = d - sin;
                let len = rate.snap_floor((remaining as f64 * self.length_frac) as Micros).max(rate.frame_duration());
                (sin, len.min(remaining).max(rate.frame_duration()))
            }
            None => (0, rate.snap((2.0 * S as f64 * self.length_frac.max(0.1)) as Micros).max(rate.frame_duration())),
        };
        let mut c = Clip::media(id, lane, 0, media, source_in, duration);
        c.name = format!("clip{}", id.0);
        c.transform.opacity = self.opacity;
        c.transform.scale_x = self.scale;
        c.transform.scale_y = self.scale;
        c.transform.rotation = self.rotation;
        c.transform.position = Vec2::new(self.position.0, self.position.1);
        c.audio.volume_db = self.volume_db;
        c.enabled = self.enabled;
        c.blend = self.blend;
        if !self.opacity_keys.is_empty() {
            let keys: Vec<Keyframe> = self.opacity_keys.iter().map(|(f, v, i)| Keyframe::new(((duration - 1) as f64 * f) as Micros, Value::Scalar(*v), *i)).collect();
            let mut t = Track::new(keys);
            t.keys.dedup_by_key(|k| k.time);
            c.keyframes.insert("transform.opacity".into(), t);
        }
        c
    }
}

impl ProjectPlan {
    /// Builds the project through edits. Every edit is checked by the model,
    /// so a plan that cannot be realised panics here (a strategy bug) rather
    /// than yielding an invalid project.
    pub fn build(&self) -> Built {
        let mut p = Project::new(ProjectSettings { frame_rate: self.frame_rate, ..Default::default() });
        let seq = p.active_sequence;
        let spine = p.active().spine_id();
        let rate = self.frame_rate;
        let mut media = Vec::new();
        for (i, m) in self.media.iter().enumerate() {
            let id = MediaId(p.alloc_id());
            let ext = match m {
                MediaPlan::Video { .. } => "mp4",
                MediaPlan::Audio { .. } => "wav",
                MediaPlan::Image => "png",
            };
            p.apply(Edit::InsertMedia { item: Box::new(MediaItem::new(id, format!("/media/item{i}.{ext}"), m.info())), index: None }).unwrap();
            media.push(id);
        }
        let mut visual = Vec::new();
        for i in 0..self.visual_lanes {
            let id = LaneId(p.alloc_id());
            p.apply(Edit::AddLane { seq, lane: Lane::new(id, LaneKind::Visual, format!("V{}", i + 1)), index: None }).unwrap();
            visual.push(id);
        }
        let mut audio = Vec::new();
        for i in 0..self.audio_lanes {
            let id = LaneId(p.alloc_id());
            p.apply(Edit::AddLane { seq, lane: Lane::new(id, LaneKind::Audio, format!("A{}", i + 1)), index: None }).unwrap();
            audio.push(id);
        }
        let mut spine_clips = Vec::new();
        for cp in &self.spine {
            let mi = cp.media % media.len().max(1);
            let id = ClipId(p.alloc_id());
            let clip = cp.make(id, spine, media[mi], self.media[mi].duration(), rate);
            p.apply(Edit::Append { seq, lane: None, clip: Box::new(clip) }).unwrap();
            spine_clips.push(id);
        }
        let spine_len = p.active().spine_end().max(S);
        let mut connected_clips = Vec::new();
        for (cp, lane_i, frac) in &self.connected {
            if visual.is_empty() {
                break;
            }
            let lane = visual[*lane_i % visual.len()];
            let mi = cp.media % media.len().max(1);
            let id = ClipId(p.alloc_id());
            let clip = cp.make(id, lane, media[mi], self.media[mi].duration(), rate);
            let at = rate.snap((spine_len as f64 * frac) as Micros);
            // Overwrite replaces anything under it, so the lane never overlaps.
            p.apply(Edit::Overwrite { seq, lane: Some(lane), at, clip: Box::new(clip) }).unwrap();
            connected_clips.push(id);
        }
        let mut audio_clips = Vec::new();
        for (cp, lane_i, frac) in &self.audio {
            if audio.is_empty() {
                break;
            }
            let lane = audio[*lane_i % audio.len()];
            let mi = cp.media % media.len().max(1);
            let id = ClipId(p.alloc_id());
            let mut clip = cp.make(id, lane, media[mi], self.media[mi].duration(), rate);
            if let ClipSource::Media(m) = &mut clip.source {
                m.video = false;
                m.audio = true;
            }
            let at = rate.snap((spine_len as f64 * frac) as Micros);
            p.apply(Edit::Overwrite { seq, lane: Some(lane), at, clip: Box::new(clip) }).unwrap();
            audio_clips.push(id);
        }
        let mut transitions = Vec::new();
        for (i, frac) in &self.transitions {
            if spine_clips.len() < 2 {
                break;
            }
            let i = *i % (spine_clips.len() - 1);
            let left = spine_clips[i];
            if transitions.iter().any(|(l, _)| *l == left) {
                continue;
            }
            let max = p.max_transition_duration(seq, left, TransitionAlign::Centre).unwrap_or(0);
            let dur = rate.snap_floor((max as f64 * frac) as Micros);
            if dur < rate.frame_duration() * 2 || dur > max {
                continue;
            }
            let id = TransitionId(p.alloc_id());
            let t = Transition { id, left, right: ClipId(0), kind: "dissolve".into(), duration: dur, align: TransitionAlign::Centre, params: Default::default(), extra: Default::default() };
            if p.apply(Edit::AddTransition { seq, transition: t }).is_ok() {
                transitions.push((left, id));
            }
        }
        for f in &self.markers {
            let id = MarkerId(p.alloc_id());
            let m = Marker::new(id, rate.snap((spine_len as f64 * f) as Micros), "mark");
            p.apply(Edit::AddMarker { seq, clip: None, marker: m }).unwrap();
        }
        p.check_invariants().expect("built project satisfies invariants");
        Built { project: p, seq, spine, visual, audio, media, spine_clips, connected_clips, audio_clips, transitions: transitions.into_iter().map(|(_, t)| t).collect() }
    }
}

/// Any of the common frame rates.
pub fn arb_frame_rate() -> impl Strategy<Value = FrameRate> {
    prop_oneof![Just(FrameRate::FPS_24), Just(FrameRate::FPS_25), Just(FrameRate::FPS_30), Just(FrameRate::FPS_29_97), Just(FrameRate::FPS_60), Just(FrameRate::new(60_000, 1001)), Just(FrameRate::new(50, 1))]
}

pub fn arb_blend() -> impl Strategy<Value = BlendMode> {
    prop_oneof![Just(BlendMode::Normal), Just(BlendMode::Multiply), Just(BlendMode::Screen), Just(BlendMode::Overlay), Just(BlendMode::Add), Just(BlendMode::Difference)]
}

pub fn arb_interp() -> impl Strategy<Value = Interp> {
    prop_oneof![Just(Interp::Hold), Just(Interp::Linear), Just(Interp::Ease), Just(Interp::Auto)]
}

pub fn arb_media() -> impl Strategy<Value = MediaPlan> {
    prop_oneof![
        (1 * S..30 * S).prop_map(|d| MediaPlan::Video { duration: d }),
        (1 * S..60 * S).prop_map(|d| MediaPlan::Audio { duration: d }),
        Just(MediaPlan::Image),
    ]
}

/// A clip plan referring to one of `media_count` media items.
pub fn arb_clip(media_count: usize) -> impl Strategy<Value = ClipPlan> {
    let timing = (0..media_count.max(1), 0.0..1.0f64, 0.05..1.0f64, 0.0..=1.0f64, 0.05..4.0f64, -360.0..360.0f64, (-1000.0..1000.0f64, -600.0..600.0f64));
    let rest = (-30.0..12.0f64, prop_oneof![Just(1.0), 0.25..4.0f64], any::<bool>(), prop::bool::weighted(0.9), arb_blend(), prop::collection::vec((0.0..=1.0f64, 0.0..=1.0f64, arb_interp()), 0..4));
    (timing, rest).prop_map(|((media, source_in_frac, length_frac, opacity, scale, rotation, position), (volume_db, speed, reversed, enabled, blend, opacity_keys))| ClipPlan {
        media,
        source_in_frac,
        length_frac,
        opacity,
        scale,
        rotation,
        position,
        volume_db,
        speed,
        reversed,
        enabled,
        blend,
        opacity_keys,
    })
}

/// A whole project plan with up to `max_clips` spine clips.
pub fn arb_project(max_clips: usize) -> impl Strategy<Value = ProjectPlan> {
    (arb_frame_rate(), prop::collection::vec(arb_media(), 1..5), 0usize..3, 0usize..3).prop_flat_map(move |(frame_rate, media, visual_lanes, audio_lanes)| {
        let n = media.len();
        (
            Just(frame_rate),
            Just(media),
            prop::collection::vec(arb_clip(n), 1..=max_clips.max(1)),
            prop::collection::vec((arb_clip(n), 0usize..3, 0.0..0.9f64), 0..4),
            Just(visual_lanes),
            Just(audio_lanes),
            prop::collection::vec((arb_clip(n), 0usize..3, 0.0..0.9f64), 0..3),
            prop::collection::vec((0usize..8, 0.1..1.0f64), 0..3),
            prop::collection::vec(0.0..1.0f64, 0..4),
        )
            .prop_map(|(frame_rate, media, spine, connected, visual_lanes, audio_lanes, audio, transitions, markers)| ProjectPlan { frame_rate, media, spine, connected, visual_lanes, audio_lanes, audio, transitions, markers })
    })
}

/// A valid scalar keyframe track inside `[0, duration)`: sorted, unique
/// times, same kind.
pub fn arb_scalar_track(duration: Micros) -> impl Strategy<Value = Track> {
    prop::collection::btree_map(0..duration.max(2), (-100.0..100.0f64, arb_interp()), 1..8).prop_map(|m| Track::new(m.into_iter().map(|(t, (v, i))| Keyframe::new(t, Value::Scalar(v), i)).collect()))
}

/// A colour track (for linear-light interpolation tests).
pub fn arb_colour_track(duration: Micros) -> impl Strategy<Value = Track> {
    prop::collection::btree_map(0..duration.max(2), ((0.0..=1.0f64, 0.0..=1.0f64, 0.0..=1.0f64, 0.0..=1.0f64), arb_interp()), 1..6)
        .prop_map(|m| Track::new(m.into_iter().map(|(t, ((r, g, b, a), i))| Keyframe::new(t, Value::Colour(Rgba::new(r, g, b, a)), i)).collect()))
}

/// A random in-range value for one `fx` parameter.
pub fn arb_param_value(def: &'static ParamDef) -> BoxedStrategy<ParamValue> {
    match &def.kind {
        ParamKind::Float { min, max } => (*min..=*max).prop_map(ParamValue::Float).boxed(),
        ParamKind::Int { min, max } => (*min..=*max).prop_map(ParamValue::Int).boxed(),
        ParamKind::Bool => any::<bool>().prop_map(ParamValue::Bool).boxed(),
        ParamKind::Choice(options) => (0..options.len() as u32).prop_map(ParamValue::Choice).boxed(),
        ParamKind::Colour => (0.0..=1.0f32, 0.0..=1.0f32, 0.0..=1.0f32, 0.0..=1.0f32).prop_map(|(r, g, b, a)| ParamValue::Colour([r, g, b, a])).boxed(),
        ParamKind::Point => (-0.5..=1.5f32, -0.5..=1.5f32).prop_map(|(x, y)| ParamValue::Point([x, y])).boxed(),
    }
}

/// A random, valid parameter set for an effect / transition / motion
/// parameter list: every parameter in range (clamping is the `ParamSet`'s
/// job; this drives it with legal values).
pub fn arb_param_set(defs: &'static [ParamDef]) -> BoxedStrategy<ParamSet> {
    let per: Vec<BoxedStrategy<ParamValue>> = defs.iter().map(arb_param_value).collect();
    per.prop_map(move |values| {
        let mut set = ParamSet::defaults(defs);
        for (d, v) in defs.iter().zip(values) {
            let _ = set.set(d.id, v);
        }
        set
    })
    .boxed()
}

/// Extreme values for a parameter: both ends of its range, zero, and a
/// few out-of-range and non-finite numbers that `ParamSet::set` must
/// reject or clamp.
pub fn extreme_param_values(def: &ParamDef) -> Vec<ParamValue> {
    match &def.kind {
        ParamKind::Float { min, max } => vec![ParamValue::Float(*min), ParamValue::Float(*max), ParamValue::Float(0.0), ParamValue::Float(*min - 1e6), ParamValue::Float(*max + 1e6), ParamValue::Float(f32::NAN), ParamValue::Float(f32::INFINITY)],
        ParamKind::Int { min, max } => vec![ParamValue::Int(*min), ParamValue::Int(*max), ParamValue::Int(i32::MIN), ParamValue::Int(i32::MAX)],
        ParamKind::Bool => vec![ParamValue::Bool(true), ParamValue::Bool(false)],
        ParamKind::Choice(options) => (0..options.len() as u32).map(ParamValue::Choice).chain([ParamValue::Choice(u32::MAX)]).collect(),
        ParamKind::Colour => vec![ParamValue::Colour([0.0; 4]), ParamValue::Colour([1.0; 4]), ParamValue::Colour([100.0, -5.0, f32::NAN, 1.0])],
        ParamKind::Point => vec![ParamValue::Point([0.0, 0.0]), ParamValue::Point([1.0, 1.0]), ParamValue::Point([-1e9, 1e9]), ParamValue::Point([f32::NAN, 0.5])],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    proptest! {
        #![proptest_config(ProptestConfig { cases: 24, ..ProptestConfig::default() })]
        #[test]
        fn generated_projects_satisfy_invariants(plan in arb_project(6)) {
            let b = plan.build();
            prop_assert!(b.project.check_invariants().is_ok());
            prop_assert_eq!(b.spine_clips.len(), plan.spine.len());
        }

        #[test]
        fn generated_tracks_validate(track in arb_scalar_track(5 * S)) {
            prop_assert!(track.validate().is_ok());
        }
    }
}
