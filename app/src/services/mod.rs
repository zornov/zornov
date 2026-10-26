// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Services: everything the interface calls that is not drawing. Each
//! module works on plain data (`Doc`, `view` structs, paths) and returns
//! results the UI shows however it likes; none of them paints, reads input
//! or opens dialogs. `HANDOFF.md` lists the functions, their threading and
//! known bugs.
//!
//! | module | owns |
//! |---|---|
//! | `library` | the nine Library panels: categories, search, item data, thumbnails and previews as recipes, apply / add / adjustment |
//! | `project` | canvas presets, new / open / save, media and timeline import, OTIO export |
//! | `transport` | position, length and timecodes from the model; frame stepping |
//! | `export` | export presets, requests, running jobs with progress events, reveal |
//! | `models` | Settings ▸ Models and AI feature availability from `studio-ai` |
//! | `ai_run` | AI actions that change the edit (captions, voice cleanup, beat markers) as jobs |
//! | `tracking` | motion tracks on clips, layers that follow them, stabilizing |
//! | `cutout` | background removal, object cut-outs (matte files) and roto masks |
//! | `backdrop_fit` | characters sitting in their backdrop: its light read, ground shadows, light match |
//! | `licences` | Settings ▸ About ▸ Licences from `ATTRIBUTIONS.md` and the bundled assets |
//! | `media_meta` | ProRes, EXR / DPX / TIFF / 16-bit PNG and frame sequences played natively; source timecode and reel names |
//! | `proxies` | proxy files: making them (ProRes 422 LT) and switching between them and the originals |
//! | `multicam` | multicam clips: syncing angles by sound or timecode, cutting between angles |
//! | `consolidate` | collecting a project's media into one folder and relinking |
//! | `grade_stills` | the stills gallery: grades kept with a picture, applied to other shots |

// The UI port wires its panels to these; until then parts are reached
// only from tests.
#![allow(dead_code)]

#[cfg(feature = "render")]
pub mod export;
pub mod ai_run;
pub mod journal;
pub mod layers;
pub mod rigging;
pub mod character;
pub mod cast;
#[cfg(feature = "render")]
pub mod mocap;
pub mod library;
pub mod licences;
pub mod models;
pub mod project;
pub mod transport;
#[cfg(feature = "render")]
pub mod tracking;
#[cfg(feature = "render")]
pub mod cutout;
#[cfg(feature = "render")]
pub mod colour_match;
#[cfg(feature = "render")]
pub mod backdrop_fit;
#[cfg(feature = "render")]
pub mod slowmo;
#[cfg(feature = "render")]
pub mod vfx;
#[cfg(feature = "render")]
pub mod genmedia;
pub mod media_meta;
#[cfg(feature = "render")]
pub mod proxies;
#[cfg(feature = "render")]
pub mod multicam;
pub mod consolidate;
#[cfg(feature = "render")]
pub mod grade_stills;

#[cfg(all(test, feature = "render"))]
mod finish_line;

/// A change that went into the project.
#[derive(Clone, Debug, PartialEq)]
pub struct Outcome {
    /// The undo label ("Add clip").
    pub label: String,
    /// What to tell the user ("Added Lake at dawn").
    pub message: String,
    /// Clips to select afterwards.
    pub select: Vec<u32>,
}

/// Why nothing changed, worded for the user.
#[derive(Clone, Debug, PartialEq)]
pub struct Refusal(pub String);

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
