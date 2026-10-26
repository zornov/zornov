// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The project: settings, media bin, sequences and presets.

use crate::id::{LaneId, MediaId, PresetId, SequenceId};
use crate::media::MediaItem;
use crate::sequence::Sequence;
use crate::story::Story;
use crate::time::FrameRate;
use crate::value::{Extra, Rgba};
use serde::{Deserialize, Serialize};

/// A whole editing project.
///
/// Read fields directly; change them only through [`Project::apply`] (or a
/// [`crate::History`]) so every change is undoable and checked.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Project {
    /// Output and working settings.
    pub settings: ProjectSettings,
    /// Media bin.
    #[serde(default)]
    pub media: Vec<MediaItem>,
    /// Sequences; the first one is created with the project.
    pub sequences: Vec<Sequence>,
    /// The sequence shown when the project opens.
    pub active_sequence: SequenceId,
    /// Saved settings bundles (effect stacks, grades, text styles, …).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub presets: Vec<Preset>,
    /// The production plan: script, beats, scenes, shots, places and cast.
    #[serde(default, skip_serializing_if = "Story::is_empty")]
    pub story: Story,
    /// Next value of the shared id counter.
    pub next_id: u64,
    /// Fields this version does not know about.
    #[serde(flatten)]
    pub extra: Extra,
}

impl Project {
    /// A new project with one empty sequence.
    pub fn new(settings: ProjectSettings) -> Self {
        let seq = SequenceId(1);
        Self {
            settings,
            media: Vec::new(),
            sequences: vec![Sequence::new(seq, "Sequence 1", LaneId(2))],
            active_sequence: seq,
            presets: Vec::new(),
            story: Story::default(),
            next_id: 3,
            extra: Extra::new(),
        }
    }

    /// Takes the next id from the shared counter.
    ///
    /// The counter is not rolled back by undo: ids are never reused, so an
    /// id in an old undo step can never clash with a newer object.
    pub fn alloc_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Looks up a sequence.
    pub fn sequence(&self, id: SequenceId) -> Option<&Sequence> {
        self.sequences.iter().find(|s| s.id == id)
    }

    /// Looks up a media item.
    pub fn media_item(&self, id: MediaId) -> Option<&MediaItem> {
        self.media.iter().find(|m| m.id == id)
    }

    /// Looks up a preset.
    pub fn preset(&self, id: PresetId) -> Option<&Preset> {
        self.presets.iter().find(|p| p.id == id)
    }

    /// The active sequence.
    ///
    /// # Panics
    /// Panics if `active_sequence` names no sequence (the invariants forbid it).
    pub fn active(&self) -> &Sequence {
        self.sequence(self.active_sequence).expect("active sequence exists")
    }

    /// Compares two projects ignoring the id counter, which undo does not
    /// rewind. Handy for undo round-trip checks.
    pub fn content_eq(&self, other: &Project) -> bool {
        self.settings == other.settings
            && self.media == other.media
            && self.sequences == other.sequences
            && self.active_sequence == other.active_sequence
            && self.presets == other.presets
            && self.story == other.story
            && self.extra == other.extra
    }

    pub(crate) fn sequence_mut(&mut self, id: SequenceId) -> Option<&mut Sequence> {
        self.sequences.iter_mut().find(|s| s.id == id)
    }
}

impl Default for Project {
    fn default() -> Self {
        Self::new(ProjectSettings::default())
    }
}

/// Canvas, timing and colour settings shared by all sequences.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProjectSettings {
    /// Canvas width in pixels.
    pub width: u32,
    /// Canvas height in pixels.
    pub height: u32,
    /// Frame rate; also the grid frame-accurate edits snap to.
    pub frame_rate: FrameRate,
    /// Colour shown where no clip covers the canvas.
    #[serde(default = "black")]
    pub background: Rgba,
    /// Output colour space.
    #[serde(default)]
    pub colour_space: ColourSpace,
    /// Audio mixing rate in Hz.
    #[serde(default = "default_rate")]
    pub sample_rate: u32,
    /// Colour management: view transform and OCIO config.
    #[serde(default, skip_serializing_if = "ColourManagement::is_default")]
    pub colour: ColourManagement,
    /// Fields this version does not know about.
    #[serde(flatten)]
    pub extra: Extra,
}

/// How a project manages colour. Media input colour spaces are on each
/// [`crate::MediaItem`]; this holds what is shared.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ColourManagement {
    /// View (display) transform: `""` (none: display-referred), `"aces"`,
    /// `"aces:<peak cd/m²>"`, or `"ocio:<display>/<view>"`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub view: String,
    /// OpenColorIO config file used by `ocio:` input spaces and views.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ocio_config: Option<String>,
    /// Peak luminance of HDR (PQ) output, cd/m² (default 1000).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hdr_peak: Option<u32>,
}

impl ColourManagement {
    /// Whether nothing is set.
    pub fn is_default(&self) -> bool {
        *self == ColourManagement::default()
    }
}

fn black() -> Rgba {
    Rgba::BLACK
}
fn default_rate() -> u32 {
    48_000
}

impl Default for ProjectSettings {
    /// 1920×1080 at 30 fps, Rec. 709, 48 kHz.
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
            frame_rate: FrameRate::FPS_30,
            background: Rgba::BLACK,
            colour_space: ColourSpace::Rec709,
            sample_rate: 48_000,
            colour: ColourManagement::default(),
            extra: Extra::new(),
        }
    }
}

/// Output colour spaces.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColourSpace {
    /// BT.709 primaries, sRGB/BT.1886 display (standard dynamic range).
    #[default]
    Rec709,
    /// Display P3 primaries with the sRGB curve.
    DisplayP3,
    /// BT.2020 primaries, PQ transfer (HDR).
    Rec2020Pq,
    /// BT.2020 primaries, HLG transfer (HDR).
    Rec2020Hlg,
}

/// A saved settings bundle. `kind` says what it holds (e.g. `"effects"`,
/// `"grade"`, `"text_style"`, `"transition"`); `data` is that thing's JSON.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Preset {
    /// Unique id.
    pub id: PresetId,
    /// Display name.
    pub name: String,
    /// What the preset holds.
    pub kind: String,
    /// The saved settings.
    pub data: serde_json::Value,
}
