// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! From the desk to a `studio-sound` mix plan: lane roles, buses, sends,
//! automation, surround placement, the master and delivery gain, and the
//! variants exports need (a layout, a language's mix, one stem).

use super::{Desk, Keys, LaneMix, lane_role};
use studio_model as m;
use studio_sound::automation::{Curve, Key};
use studio_sound::dynamics::LimiterParams;
use studio_sound::loudness::LoudnessTarget;
use studio_sound::mixer::{BusKind, BusSpec, LaneRole, LaneSpec, MixPlan, SendSpec, SurroundSpec};
use studio_sound::surround::{Layout, PanShape};

/// Which mix to build.
#[derive(Clone, Debug, PartialEq)]
pub struct MixRequest {
    /// Speaker layout; `None` = the desk's.
    pub layout: Option<Layout>,
    /// The language whose dialogue plays; `None` = the original.
    pub language: Option<String>,
    /// Only these stems (lanes of these kinds), unlimited.
    pub stem: Option<Vec<BusKind>>,
    /// Apply the delivery normalisation gain.
    pub normalise: bool,
    /// The output limiter (when the desk has it on).
    pub limiter: bool,
    /// Only this lane (for object export), centred.
    pub only_lane: Option<u64>,
    /// Leave out lanes that leave as objects (for an ADM bed).
    pub without_objects: bool,
}

impl Default for MixRequest {
    fn default() -> Self {
        Self { layout: None, language: None, stem: None, normalise: true, limiter: true, only_lane: None, without_objects: false }
    }
}

impl MixRequest {
    /// The mix as delivered, in `layout`.
    pub fn delivered(layout: Layout) -> Self {
        Self { layout: Some(layout), ..Default::default() }
    }
}

/// A curve from automation keys, or a constant.
pub fn curve(keys: &Keys, rest: f32) -> Curve {
    if keys.is_empty() {
        return Curve::constant(rest);
    }
    Curve::new(keys.iter().map(|&(t, v)| Key::new(t, v)).collect(), rest)
}

/// Default surround shape and position of a role, for lanes nobody placed.
fn role_shape(role: LaneRole) -> (f32, f32, PanShape) {
    match role {
        // Dialogue lives in the centre speaker.
        LaneRole::Dialogue => (0.0, 1.0, PanShape { width: 0.0, centre: true, lfe_db: f32::NEG_INFINITY }),
        // Music spreads across the front with a phantom centre, leaving the
        // centre channel to dialogue.
        LaneRole::Music => (0.0, 1.0, PanShape { width: 60.0, centre: false, lfe_db: f32::NEG_INFINITY }),
        // Backgrounds wrap the room.
        LaneRole::Ambience => (0.0, 0.0, PanShape { width: 120.0, centre: false, lfe_db: f32::NEG_INFINITY }),
        LaneRole::Other => (0.0, 1.0, PanShape::default()),
    }
}

/// The lane's spec for the mixer.
pub fn lane_spec(lane: &m::Lane, any_solo: bool) -> LaneSpec {
    let mix = LaneMix::of(lane);
    let role = lane_role(lane);
    let mut spec = LaneSpec::new(lane.id.0, role);
    spec.volume_db = curve(&mix.volume_keys, lane.volume_db as f32);
    spec.pan = lane.pan as f32;
    if !mix.pan_keys.is_empty() {
        spec.pan_curve = Some(curve(&mix.pan_keys, lane.pan as f32));
    }
    spec.muted = lane.muted || (any_solo && !lane.solo);
    spec.solo = lane.solo;
    spec.effects = mix.effects.iter().filter(|e| e.enabled).filter_map(crate::sound_bridge::audio_effect).collect();
    spec.bus = mix.bus;
    spec.sends = mix.sends.iter().map(|s| SendSpec { bus: s.bus, level_db: curve(&s.keys, s.level_db), pre_fader: s.pre }).collect();
    let (dx, dy, mut shape) = role_shape(role);
    let s = &mix.surround;
    let (x, y) = if s.placed || !s.x_keys.is_empty() || !s.y_keys.is_empty() { (s.x, s.y) } else { (dx, dy) };
    if let Some(w) = s.width {
        shape.width = w;
    }
    if let Some(c) = s.centre {
        shape.centre = c;
    }
    if let Some(l) = s.lfe_db {
        shape.lfe_db = l;
    }
    spec.surround = SurroundSpec { x: curve(&s.x_keys, x), y: curve(&s.y_keys, y), z: curve(&s.z_keys, s.z), shape };
    spec
}

/// The desk's buses for the mixer.
pub fn bus_specs(desk: &Desk) -> Vec<BusSpec> {
    desk.buses
        .iter()
        .map(|b| {
            let mut s = BusSpec::new(b.id, b.kind(), b.name.clone());
            s.volume_db = curve(&b.keys, b.volume_db);
            s.effects = b.effects.iter().filter(|e| e.enabled).filter_map(crate::sound_bridge::audio_effect).collect();
            s.muted = b.muted;
            s.solo = b.solo;
            s
        })
        .collect()
}

/// Whether `lane` plays in the mix of `language` (`None` = the original).
pub fn plays_in(desk: &Desk, lane: &m::Lane, language: Option<&str>) -> bool {
    use studio_text::subtitles::base_lang;
    let mix = LaneMix::of(lane);
    let same = |a: &str, b: &str| base_lang(a) == base_lang(b);
    let lang = language.filter(|l| !same(l, &desk.original_language));
    match (&mix.dub, lang) {
        // A dub plays only in its own language.
        (Some(d), Some(l)) => same(d, l),
        (Some(_), None) => false,
        // Original dialogue gives way to a dub of the language asked for.
        (None, Some(l)) => !(lane_role(lane) == LaneRole::Dialogue && desk.languages.iter().any(|x| same(&x.lang, l))),
        (None, None) => true,
    }
}

/// Applies the desk to a plan whose clips and lanes the bridge built.
pub fn apply(project: &m::Project, plan: &mut MixPlan, req: &MixRequest) {
    let desk = Desk::of(project);
    let Some(seq) = project.sequence(project.active_sequence) else { return };
    let any_solo = seq.lanes.iter().any(|l| l.solo);
    plan.lanes = seq.lanes.iter().map(|l| lane_spec(l, any_solo)).collect();
    for (spec, lane) in plan.lanes.iter_mut().zip(&seq.lanes) {
        let mix = LaneMix::of(lane);
        if !plays_in(&desk, lane, req.language.as_deref()) || (req.without_objects && mix.object) {
            spec.muted = true;
        }
        if let Some(only) = req.only_lane {
            spec.muted = lane.id.0 != only || lane.muted;
            spec.solo = false;
            spec.bus = None;
            spec.sends.clear();
            spec.pan = 0.0;
            spec.pan_curve = None;
        }
    }
    plan.buses = if req.only_lane.is_some() { Vec::new() } else { bus_specs(&desk) };
    plan.layout = req.layout.unwrap_or_else(|| desk.layout());
    let target = desk.delivery.as_ref().and_then(|d| LoudnessTarget::from_id(&d.target));
    let gain = if req.normalise { desk.delivery.as_ref().map_or(0.0, |d| d.gain_db) } else { 0.0 };
    plan.master.volume_db = desk.master.volume_db + gain;
    plan.master.effects = desk.master.effects.iter().filter(|e| e.enabled).filter_map(crate::sound_bridge::audio_effect).collect();
    let ceiling = target.map_or(desk.master.ceiling_db, |t| t.ceiling_dbtp() as f32);
    plan.master.limiter = (desk.master.limiter && req.limiter && req.only_lane.is_none()).then(|| LimiterParams { ceiling_db: ceiling, ..Default::default() });
    plan.master.ducking = desk.master.ducking.then(studio_sound::ducking::DuckingParams::default);
    if let Some(kinds) = &req.stem {
        plan.stem(kinds);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sound_desk::{Language, Send};

    fn lane(id: u64, name: &str, mix: Option<LaneMix>) -> m::Lane {
        let mut l = m::Lane::new(m::LaneId(id), m::LaneKind::Audio, name);
        if let Some(x) = mix {
            x.store(&mut l);
        }
        l
    }

    #[test]
    fn roles_set_surround_defaults_and_placement_wins() {
        let dx = lane_spec(&lane(1, "Dialogue", None), false);
        assert_eq!(dx.surround.shape.width, 0.0);
        assert!(dx.surround.shape.centre);
        let mx = lane_spec(&lane(2, "Music", None), false);
        assert!(!mx.surround.shape.centre);
        let bg = lane_spec(&lane(3, "Room tone", None), false);
        assert_eq!(bg.role, LaneRole::Ambience);
        assert_eq!(bg.surround.y.eval(0), 0.0);
        let mut placed = LaneMix::default();
        placed.surround.x = -1.0;
        placed.surround.y = -1.0;
        placed.surround.placed = true;
        placed.sends.push(Send { bus: 9, level_db: -10.0, ..Default::default() });
        placed.volume_keys = vec![(0, -6.0), (1_000_000, 0.0)];
        let fx = lane_spec(&lane(4, "Car", Some(placed)), false);
        assert_eq!((fx.surround.x.eval(0), fx.surround.y.eval(0)), (-1.0, -1.0));
        assert_eq!(fx.sends[0].bus, 9);
        assert_eq!(fx.volume_db.eval(500_000), -3.0);
    }

    #[test]
    fn dubs_replace_the_original_dialogue_in_their_language_only() {
        let mut desk = Desk { original_language: "en".into(), ..Default::default() };
        desk.languages.push(Language { lang: "es".into(), name: "Español".into(), lane: Some(7) });
        let orig = lane(1, "Dialogue", None);
        let music = lane(2, "Music", None);
        let es = lane(7, "Dub es", Some(LaneMix { dub: Some("es".into()), role: Some("dialogue".into()), ..Default::default() }));
        assert!(plays_in(&desk, &orig, None) && !plays_in(&desk, &es, None));
        assert!(!plays_in(&desk, &orig, Some("es")) && plays_in(&desk, &es, Some("es-MX")) && plays_in(&desk, &music, Some("es")));
        // A language with no dub (subtitles only) keeps the original.
        assert!(plays_in(&desk, &orig, Some("fr")) && !plays_in(&desk, &es, Some("fr")));
        assert!(plays_in(&desk, &orig, Some("en")));
    }
}
