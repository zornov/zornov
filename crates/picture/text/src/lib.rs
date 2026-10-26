// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Text for Studio: font discovery, shaping, layout, the styling stack,
//! text styles and animated templates, captions and subtitle export.
//!
//! See `README.md` for the tour. The entry point is [`TextEngine`].

pub mod anim;
pub mod animator;
pub mod captions;
pub mod directory;
pub mod emoji;
pub mod engine;
pub mod font;
pub mod geom;
pub mod layout;
pub mod outline;
pub mod paint;
pub mod png_io;
pub mod raster;
pub mod rgba;
pub mod scene;
pub mod style;
pub mod styles;
pub mod subtitles;
pub mod templates;

pub use animator::{Animator, Keyed, Range, RangeShape, Wiggly};
pub use anim::{LoopKind, LoopMotion, Order, Reveal, RevealKind, TextMotion, UnitMotion};
pub use captions::{CaptionFrame, CaptionLook, CaptionPosition, CaptionStyle, CaptionTrack, Cue, Speaker, SpeakerTint, Word};
pub use directory::{FontEntry, FontHit};
pub use emoji::{EmojiSet, EmojiStyle};
pub use engine::{SpecimenFace, TextEngine};
pub use font::{FaceId, FaceInfo, FontLibrary, FontMatch, FontRequest, FontSource};
pub use geom::{Affine, Ease, Rect};
pub use layout::{Layout, Line, PlacedGlyph};
pub use paint::{Hooks, TextRaster, UnitPose};
pub use rgba::Rgba;
pub use scene::{Field, Key, Layer, LayerFrame, LayerKind, Phase, Property, Scene, TemplateCategory, Track};
pub use studio_colour::Image;
pub use style::*;
pub use styles::{StyleCategory, StylePreset};
