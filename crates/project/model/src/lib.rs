// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Studio's project data model.
//!
//! This crate holds everything about a project except pixels and samples:
//! the [`Project`] with its media bin, [`Sequence`]s of [`Lane`]s and
//! [`Clip`]s, transitions, markers and keyframes; the reversible [`Edit`]
//! commands that change it, with [`History`] for undo/redo; the on-disk
//! `project.json` format with atomic saves, autosave backups and version
//! migrations ([`file`](mod@file)); the [`Story`] (script, scenes, shots,
//! places and cast) the cut is planned from; and import/export of OpenTimelineIO, FCPXML and
//! CMX3600 EDL ([`interchange`]).
//!
//! Times are integer microseconds ([`Micros`]); see [`time`] for the frame
//! grid helpers.

#![warn(missing_docs)]

pub mod clip;
pub mod clipboard;
pub mod driver;
pub mod edit;
pub mod editorial;
pub mod file;
pub mod history;
pub mod id;
pub mod interchange;
pub mod keyframe;
pub mod media;
pub mod modifier;
pub mod project;
pub mod puppet;
pub mod sequence;
pub mod skeleton;
pub mod snap;
pub mod space;
pub mod story;
pub mod time;
pub mod timecode;
pub mod value;

pub use clip::{
    AudioProps, BlendMode, Clip, ClipSource, Crop, Effect, Fade, FadeCurve, Generator, GeneratorKind, Grade, GradeNode, Mask, MaskMode, MaskShape, MediaRef, Motion, MotionPreset, Parent, PathPoint,
    SequenceRef, Speed, TextRun, Transform,
};
pub use clipboard::{Clipboard, ClipboardClip};
pub use driver::{Driver, DriverVar};
pub use edit::{AttrSet, Edge, Edit, EditError, PasteMode, Stack, Violation};
pub use history::{Author, History, SavedHistory, Step};
pub use id::*;
pub use keyframe::{Curve, CurveEnd, Handle, Interp, Keyframe, PathHandles, Track, TrackError};
pub use modifier::{Modifier, Repeat};
pub use media::{AudioInfo, ColourInfo, MediaInfo, MediaItem, MediaKind, VideoInfo};
pub use project::{ColourManagement, ColourSpace, Preset, Project, ProjectSettings};
pub use puppet::{CornerPin, Cut, CutFill, Pin, PinKind, Puppet};
pub use skeleton::{Affine, Bone, BoneLink, IkHandle, Pose, Posed, PosedBone, Skeleton, wrap};
pub use sequence::{Lane, LaneKind, Marker, Sequence, Transition, TransitionAlign};
pub use snap::SnapTargets;
pub use story::{CameraMove, Character, Line, LineKind, Place, ScreenSide, Scene, Shot, ShotKind, Story, TimeOfDay};
pub use space::{AutoOrient, CameraSettings, LightKind, LightSettings, Material};
pub use time::{FrameRate, Micros, SECOND, TimeRange};
pub use timecode::Timecode;
pub use value::{Extra, ParamMap, Rgba, Value, Vec2};
