// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Media bin items: files the project references.

use crate::id::MediaId;
use crate::time::{FrameRate, Micros};
use crate::value::Extra;
use serde::{Deserialize, Serialize};

/// A file in the project's media bin.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MediaItem {
    /// Unique id.
    pub id: MediaId,
    /// Display name (defaults to the file name).
    pub name: String,
    /// Absolute path when last saved or imported.
    pub path: String,
    /// Path relative to the project folder, used to find the file again
    /// after the whole folder is moved. Filled in on save.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relative_path: Option<String>,
    /// What probing the file found.
    #[serde(default)]
    pub info: MediaInfo,
    /// Lower-resolution stand-in used for smooth editing, if generated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_path: Option<String>,
    /// Input colour space chosen for this file, over what it says about
    /// itself: a camera space such as `"slog3_sgamut3cine"`, `"rec709"`,
    /// `"acescg"`, or `"ocio:<colour space>"` from the project's config.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_colour: Option<String>,
    /// Source timecode of the file's first frame (`"14:22:10:05"`, `;`
    /// before frames for drop-frame), from the file or set by hand.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timecode: Option<String>,
    /// Reel / tape / camera roll name (`"A001C003"`), used by EDLs and
    /// conform.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reel: Option<String>,
    /// Cached audio peak file, relative to the project folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waveform_cache: Option<String>,
    /// Cached thumbnail strip, relative to the project folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumbnail_cache: Option<String>,
    /// Import time, seconds since the Unix epoch.
    #[serde(default)]
    pub imported_at: i64,
    /// Bin folder path such as `"Footage/Day 1"`; `None` is the bin root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    /// Fields this version does not know about, kept for newer versions.
    #[serde(flatten)]
    pub extra: Extra,
}

impl MediaItem {
    /// A media item with probed `info`; the name is taken from the path.
    pub fn new(id: MediaId, path: impl Into<String>, info: MediaInfo) -> Self {
        let path = path.into();
        let name = path.rsplit(['/', '\\']).next().unwrap_or(&path).to_string();
        Self {
            id,
            name,
            path,
            relative_path: None,
            info,
            proxy_path: None,
            input_colour: None,
            timecode: None,
            reel: None,
            waveform_cache: None,
            thumbnail_cache: None,
            imported_at: 0,
            folder: None,
            extra: Extra::new(),
        }
    }

    /// Source duration; `None` for stills, which can be held for any length.
    pub fn duration(&self) -> Option<Micros> {
        match self.info.kind {
            MediaKind::Image => None,
            _ => self.info.duration,
        }
    }

    /// Frame rate of the file's timecode: its video rate, else the
    /// project's.
    pub fn timecode_rate(&self, project: crate::time::FrameRate) -> crate::time::FrameRate {
        self.info.video.as_ref().and_then(|v| v.frame_rate).unwrap_or(project)
    }

    /// Source time (from the file's start) of the first frame's timecode,
    /// as time of day; 0 when the file has none.
    pub fn timecode_start(&self, project: crate::time::FrameRate) -> Micros {
        let rate = self.timecode_rate(project);
        let tc = crate::timecode::Timecode::new(rate, self.timecode.as_deref().is_some_and(|t| t.contains(';')));
        self.timecode.as_deref().and_then(|t| tc.parse_lenient(t)).map_or(0, |f| rate.frame_to_micros(f))
    }

    /// Whether the file has a picture.
    pub fn has_video(&self) -> bool {
        self.info.video.is_some()
    }

    /// Whether the file has sound.
    pub fn has_audio(&self) -> bool {
        self.info.audio.is_some()
    }
}

/// Broad kind of a media file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaKind {
    /// Moving pictures, possibly with sound.
    #[default]
    Video,
    /// Sound only.
    Audio,
    /// A single still picture.
    Image,
}

/// Probed facts about a media file.
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct MediaInfo {
    /// Broad kind.
    pub kind: MediaKind,
    /// Container name, e.g. `"mp4"`, `"mov"`, `"wav"`, `"png"`.
    #[serde(default)]
    pub container: String,
    /// Total duration (absent for stills).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<Micros>,
    /// Picture stream facts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video: Option<VideoInfo>,
    /// Sound stream facts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio: Option<AudioInfo>,
    /// Fields this version does not know about.
    #[serde(flatten)]
    pub extra: Extra,
}

/// Facts about a picture stream.
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct VideoInfo {
    /// Codec name, e.g. `"h264"`, `"hevc"`, `"png"`.
    pub codec: String,
    /// Coded width in pixels (before rotation).
    pub width: u32,
    /// Coded height in pixels (before rotation).
    pub height: u32,
    /// Nominal frame rate (absent for stills).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame_rate: Option<FrameRate>,
    /// Display rotation in degrees clockwise (0, 90, 180 or 270).
    #[serde(default)]
    pub rotation: u32,
    /// Whether the stream carries alpha.
    #[serde(default)]
    pub has_alpha: bool,
    /// Colour description from the stream.
    #[serde(default)]
    pub colour: ColourInfo,
}

/// Colour description using ITU-T H.273 code point names.
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct ColourInfo {
    /// Colour primaries, e.g. `"bt709"`, `"bt2020"`, `"display_p3"`.
    #[serde(default)]
    pub primaries: String,
    /// Transfer function, e.g. `"bt709"`, `"srgb"`, `"pq"`, `"hlg"`.
    #[serde(default)]
    pub transfer: String,
    /// YCbCr matrix, e.g. `"bt601"`, `"bt709"`, `"bt2020_ncl"`.
    #[serde(default)]
    pub matrix: String,
    /// Full-range (0..255) rather than video-range (16..235) samples.
    #[serde(default)]
    pub full_range: bool,
}

/// Facts about a sound stream.
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct AudioInfo {
    /// Codec name, e.g. `"aac"`, `"pcm_s16le"`.
    pub codec: String,
    /// Samples per second.
    pub sample_rate: u32,
    /// Channel count.
    pub channels: u16,
    /// Channel layout name, e.g. `"mono"`, `"stereo"`, `"5.1"`.
    #[serde(default)]
    pub layout: String,
}
