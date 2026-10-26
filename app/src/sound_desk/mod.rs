// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The sound department: what a film mix needs beyond clip sound, kept in
//! the project so the window, the command line and every AI agent see the
//! same mix.
//!
//! - Per lane (`Lane.extra["mix"]`, [`LaneMix`]): role (dialogue, music,
//!   effects, ambience), the bus it plays into, sends, fader and pan
//!   automation, lane effects, the surround position and its automation,
//!   whether it is a dub of another language or an ADR lane, and whether
//!   it leaves as an object in an immersive export.
//! - Per project (`settings.extra["sound"]`, [`Desk`]): buses, the master
//!   (gain, effects, limiter ceiling), the speaker layout, the delivery
//!   loudness target with its normalisation gain, the ADR cue list,
//!   subtitle tracks per language, characters with their voices per
//!   language, and the dubbed languages.
//!
//! Every change goes through the model (`SetLane` / `SetSettings`), so it
//! is one undo step like any edit. [`mix`] turns all of it into a
//! `studio-sound` mix plan (any layout, any language, any stem);
//! [`render`] measures, normalises and writes stems; [`subs`] handles
//! subtitles; [`adr`] and [`dub`] the dialogue workflows.

pub mod adr;
pub mod dialogue;
pub mod dub;
pub mod mix;
pub mod render;
pub mod subs;

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use studio_model as m;
use studio_sound::mixer::{BusKind, LaneRole};
use studio_sound::surround::Layout;

/// A time and a value: automation keys in timeline microseconds.
pub type Keys = Vec<(i64, f32)>;

/// A send from a lane to an aux bus.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Send {
    pub bus: u64,
    pub level_db: f32,
    /// Level automation; replaces `level_db` when not empty.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub keys: Keys,
    /// Before the fader.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub pre: bool,
}

/// A lane's place in a surround or immersive mix.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Surround {
    /// −1 left .. 1 right.
    pub x: f32,
    /// −1 back .. 1 front.
    pub y: f32,
    /// 0 ear level .. 1 overhead (objects).
    pub z: f32,
    /// Spread of a stereo lane, degrees; `None` = the role's default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<f32>,
    /// Use the centre speaker; `None` = the role's default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub centre: Option<bool>,
    /// LFE send in dB.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lfe_db: Option<f32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub x_keys: Keys,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub y_keys: Keys,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub z_keys: Keys,
    /// Set when the position was chosen (not the role's default).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub placed: bool,
}

impl Default for Surround {
    fn default() -> Self {
        Self { x: 0.0, y: 1.0, z: 0.0, width: None, centre: None, lfe_db: None, x_keys: Vec::new(), y_keys: Vec::new(), z_keys: Vec::new(), placed: false }
    }
}

/// The mix side of a lane.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LaneMix {
    /// `dialogue`, `music`, `effects`, `ambience`; `None` = from the name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// Bus id; `None` = the bus of the role's kind.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bus: Option<u64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sends: Vec<Send>,
    /// Fader automation (dB); replaces the lane volume when not empty.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub volume_keys: Keys,
    /// Balance automation (−1..1).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub pan_keys: Keys,
    /// Lane inserts (same kinds and settings as clip sound effects).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub effects: Vec<m::Effect>,
    #[serde(skip_serializing_if = "is_default_surround")]
    pub surround: Surround,
    /// The language this lane's dialogue is dubbed in (only plays in that
    /// language's mix).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dub: Option<String>,
    /// ADR lane (recorded replacement dialogue).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub adr: bool,
    /// Leaves as a moving object in an ADM (immersive) export.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub object: bool,
}

fn is_default_surround(s: &Surround) -> bool {
    *s == Surround::default()
}

impl LaneMix {
    /// The lane's mix settings (defaults when it has none).
    pub fn of(lane: &m::Lane) -> LaneMix {
        lane.extra.get("mix").and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default()
    }

    /// Writes the settings into the lane (removing the key when default).
    pub fn store(&self, lane: &mut m::Lane) {
        if *self == LaneMix::default() {
            lane.extra.remove("mix");
        } else if let Ok(v) = serde_json::to_value(self) {
            lane.extra.insert("mix".into(), v);
        }
    }
}

/// The role a lane plays: its own setting, else guessed from its name and
/// kind (the main lane carries the production sound, which is dialogue).
pub fn lane_role(lane: &m::Lane) -> LaneRole {
    if let Some(r) = LaneMix::of(lane).role.as_deref().and_then(role_from_id) {
        return r;
    }
    let n = lane.name.to_ascii_lowercase();
    let has = |w: &[&str]| w.iter().any(|x| n.contains(x));
    if has(&["music", "song", "score"]) {
        LaneRole::Music
    } else if has(&["ambien", "room tone", "roomtone", "atmos", "background", "backgrounds", "bg ", "walla"]) || n == "bg" {
        LaneRole::Ambience
    } else if has(&["voice", "dialog", "narrat", "adr", "dub", "vo"]) || lane.kind == m::LaneKind::Spine {
        LaneRole::Dialogue
    } else {
        LaneRole::Other
    }
}

/// `dialogue` → [`LaneRole::Dialogue`] and so on (also `dx`, `mx`, `fx`, `bg`).
pub fn role_from_id(s: &str) -> Option<LaneRole> {
    Some(match BusKind::from_id(s)? {
        BusKind::Dialogue => LaneRole::Dialogue,
        BusKind::Music => LaneRole::Music,
        BusKind::Ambience => LaneRole::Ambience,
        BusKind::Effects | BusKind::Aux => LaneRole::Other,
    })
}

/// The id of a role, for the AI and the file.
pub fn role_id(r: LaneRole) -> &'static str {
    match r {
        LaneRole::Dialogue => "dialogue",
        LaneRole::Music => "music",
        LaneRole::Ambience => "ambience",
        LaneRole::Other => "effects",
    }
}

/// A bus.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Bus {
    pub id: u64,
    /// `dialogue`, `music`, `effects`, `ambience` or `aux`.
    pub kind: String,
    pub name: String,
    pub volume_db: f32,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub keys: Keys,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub effects: Vec<m::Effect>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub muted: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub solo: bool,
}

impl Default for Bus {
    fn default() -> Self {
        Self { id: 0, kind: "effects".into(), name: String::new(), volume_db: 0.0, keys: Vec::new(), effects: Vec::new(), muted: false, solo: false }
    }
}

impl Bus {
    pub fn kind(&self) -> BusKind {
        BusKind::from_id(&self.kind).unwrap_or_default()
    }
}

/// The master bus.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Master {
    pub volume_db: f32,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub effects: Vec<m::Effect>,
    /// True-peak ceiling of the output limiter, dBTP.
    pub ceiling_db: f32,
    /// The output limiter; off for an unprotected mix.
    pub limiter: bool,
    /// Duck music under dialogue automatically.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub ducking: bool,
}

impl Default for Master {
    fn default() -> Self {
        Self { volume_db: 0.0, effects: Vec::new(), ceiling_db: -1.0, limiter: true, ducking: false }
    }
}

/// The delivery loudness target and what normalising to it found.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Delivery {
    /// A `LoudnessTarget` id: `ebu_r128`, `atsc_a85`, `netflix`, `amazon`,
    /// `streaming`, `podcast`.
    pub target: String,
    /// Gain the normalisation put on the master, dB.
    pub gain_db: f32,
    /// Loudness measured after normalising (LUFS; dialogue-gated for the
    /// specifications that ask for it).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub measured_lufs: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub true_peak_dbtp: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range_lu: Option<f64>,
}

/// One ADR cue: a line to re-record.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AdrCue {
    pub id: u32,
    pub character: String,
    /// Timeline microseconds.
    pub start: i64,
    pub end: i64,
    /// The line as it should be said.
    pub text: String,
    /// Why (noise, performance, line change, technical, added line).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub reason: String,
    /// `to do`, `recorded`, `approved`.
    pub status: String,
    /// The clip of the chosen take.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub take: Option<u64>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub notes: String,
}

/// One subtitle.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SubCue {
    /// Seconds on the timeline.
    pub start: f32,
    pub end: f32,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speaker: Option<String>,
}

/// A subtitle track in one language.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SubTrack {
    /// Language code (`en`, `es`, `hi`…).
    pub lang: String,
    /// Menu name.
    pub name: String,
    /// `subtitles`, `sdh` (for the deaf and hard of hearing) or `forced`
    /// (only foreign-language lines).
    pub kind: String,
    pub cues: Vec<SubCue>,
}

/// A character's voices for dubbing, per language.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Character {
    /// Language → voice id of the chosen speech service (Fish model id,
    /// ElevenLabs voice id, OpenAI voice name, Mac voice id).
    pub voices: BTreeMap<String, String>,
    /// Words a minute when speaking, if not the service's own pace.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pace: Option<f32>,
}

/// A dubbed language.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Language {
    pub lang: String,
    pub name: String,
    /// The lane its dubbed lines sit on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lane: Option<u64>,
}

/// Everything the sound department keeps for the project.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Desk {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub buses: Vec<Bus>,
    pub master: Master,
    /// `stereo`, `5.1` or `7.1`.
    pub layout: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivery: Option<Delivery>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub adr: Vec<AdrCue>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub subtitles: Vec<SubTrack>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub characters: BTreeMap<String, Character>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub languages: Vec<Language>,
    /// The language of the production dialogue.
    pub original_language: String,
}

impl Desk {
    /// The project's desk (defaults when it has none).
    pub fn of(project: &m::Project) -> Desk {
        let mut d: Desk = project.settings.extra.get("sound").and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default();
        if d.layout.is_empty() {
            d.layout = "stereo".into();
        }
        if d.original_language.is_empty() {
            d.original_language = "en".into();
        }
        d
    }

    /// The settings with this desk written in.
    pub fn stored(&self, settings: &m::ProjectSettings) -> m::ProjectSettings {
        let mut s = settings.clone();
        match serde_json::to_value(self) {
            Ok(v) => {
                s.extra.insert("sound".into(), v);
            }
            Err(_) => {
                s.extra.remove("sound");
            }
        }
        s
    }

    pub fn layout(&self) -> Layout {
        Layout::from_id(&self.layout).unwrap_or_default()
    }

    pub fn bus(&self, id: u64) -> Option<&Bus> {
        self.buses.iter().find(|b| b.id == id)
    }

    /// A bus by id or name (`"DX"`, `"Reverb"`) or kind (`"music"`).
    pub fn find_bus(&self, v: &serde_json::Value) -> Option<u64> {
        if let Some(id) = v.as_u64() {
            return self.bus(id).map(|b| b.id);
        }
        let s = v.as_str()?.trim().to_ascii_lowercase();
        self.buses.iter().find(|b| b.name.to_ascii_lowercase() == s).or_else(|| BusKind::from_id(&s).and_then(|k| self.buses.iter().find(|b| b.kind() == k))).map(|b| b.id)
    }

    /// The next free bus id.
    pub fn next_bus_id(&self) -> u64 {
        self.buses.iter().map(|b| b.id).max().unwrap_or(0) + 1
    }

    /// Adds the film buses (DX, MX, FX, BG and a reverb return) that are not
    /// there yet. Returns the names added.
    pub fn add_film_buses(&mut self) -> Vec<String> {
        let mut added = Vec::new();
        for (kind, name) in [(BusKind::Dialogue, "DX"), (BusKind::Music, "MX"), (BusKind::Effects, "FX"), (BusKind::Ambience, "BG")] {
            if !self.buses.iter().any(|b| b.kind() == kind) {
                let id = self.next_bus_id();
                let mut b = Bus { id, kind: kind.id().into(), name: name.into(), ..Default::default() };
                if kind == BusKind::Dialogue {
                    // A dialogue bus is gently compressed and de-essed.
                    b.effects.push(effect("compressor", &[("threshold", -20.0), ("ratio", 2.5), ("attack", 10.0), ("release", 120.0)]));
                    b.effects.push(effect("de_esser", &[("threshold", -28.0)]));
                }
                self.buses.push(b);
                added.push(name.to_owned());
            }
        }
        if !self.buses.iter().any(|b| b.kind() == BusKind::Aux) {
            let id = self.next_bus_id();
            let mut verb = effect("reverb", &[("mix", 1.0)]);
            verb.params.insert("room".into(), m::Value::Choice("living_room".into()));
            self.buses.push(Bus { id, kind: "aux".into(), name: "Reverb".into(), effects: vec![verb], ..Default::default() });
            added.push("Reverb".into());
        }
        added
    }

    /// The subtitle track of a language.
    pub fn subtitles_of(&self, lang: &str) -> Option<&SubTrack> {
        let b = studio_text::subtitles::base_lang(lang);
        self.subtitles.iter().find(|t| t.lang == lang).or_else(|| self.subtitles.iter().find(|t| studio_text::subtitles::base_lang(&t.lang) == b))
    }
}

/// A sound effect setting as the model stores it.
pub fn effect(kind: &str, params: &[(&str, f64)]) -> m::Effect {
    let mut e = m::Effect::new(m::EffectId(0), kind);
    for (k, v) in params {
        e.params.insert((*k).into(), m::Value::Scalar(*v));
    }
    e
}

/// Applies a change to the desk as one undo step.
pub fn edit_desk(doc: &mut crate::doc::Doc, label: &str, f: impl FnOnce(&mut Desk) -> Result<String, String>) -> Result<String, String> {
    let mut d = Desk::of(&doc.model);
    let said = f(&mut d)?;
    let settings = d.stored(&doc.model.settings);
    if settings == doc.model.settings {
        return Ok(said);
    }
    if doc.apply(label, m::Edit::SetSettings { settings: Box::new(settings) }) {
        Ok(said)
    } else {
        Err(doc.error.take().unwrap_or_else(|| "the change was refused".into()))
    }
}

/// The lane with id `id` in the active sequence.
pub fn model_lane(doc: &crate::doc::Doc, id: u64) -> Option<m::Lane> {
    doc.model.sequence(doc.model.active_sequence).and_then(|s| s.lanes.iter().find(|l| l.id.0 == id)).cloned()
}

/// Applies a change to a lane's mix as one undo step.
pub fn edit_lane(doc: &mut crate::doc::Doc, lane: u64, label: &str, f: impl FnOnce(&mut LaneMix, &mut m::Lane) -> Result<String, String>) -> Result<String, String> {
    let mut l = model_lane(doc, lane).ok_or_else(|| format!("no lane {lane}"))?;
    let before = l.clone();
    let mut mix = LaneMix::of(&l);
    let said = f(&mut mix, &mut l)?;
    mix.store(&mut l);
    if l == before {
        return Ok(said);
    }
    let seq = doc.model.active_sequence;
    if doc.apply(label, m::Edit::SetLane { seq, lane: l }) {
        Ok(said)
    } else {
        Err(doc.error.take().unwrap_or_else(|| "the change was refused".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lane_mix_round_trips_and_defaults_leave_no_trace() {
        let mut lane = m::Lane::new(m::LaneId(3), m::LaneKind::Audio, "Score");
        assert_eq!(lane_role(&lane), LaneRole::Music);
        let mut mix = LaneMix::of(&lane);
        mix.role = Some("ambience".into());
        mix.sends.push(Send { bus: 5, level_db: -12.0, ..Default::default() });
        mix.surround.y = 0.0;
        mix.store(&mut lane);
        assert_eq!(lane_role(&lane), LaneRole::Ambience);
        let back = LaneMix::of(&lane);
        assert_eq!(back, mix);
        LaneMix::default().store(&mut lane);
        assert!(!lane.extra.contains_key("mix"));
        let spine = m::Lane::new(m::LaneId(1), m::LaneKind::Spine, "Main");
        assert_eq!(lane_role(&spine), LaneRole::Dialogue, "production sound is dialogue");
    }

    #[test]
    fn film_buses_are_added_once() {
        let mut d = Desk::default();
        assert_eq!(d.add_film_buses(), vec!["DX", "MX", "FX", "BG", "Reverb"]);
        assert!(d.add_film_buses().is_empty());
        assert_eq!(d.find_bus(&serde_json::json!("music")), Some(2));
        assert_eq!(d.find_bus(&serde_json::json!("reverb")), Some(5));
        let p = m::Project::default();
        let s = d.stored(&p.settings);
        let mut p2 = p.clone();
        p2.settings = s;
        assert_eq!(Desk::of(&p2).buses.len(), 5);
        assert_eq!(Desk::of(&p).layout(), Layout::Stereo);
    }
}
