// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Preferences and remembered layout, saved with eframe's storage.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ThemeChoice {
    #[default]
    Dark,
    Light,
    System,
}

/// Colour-blind-safe palettes (see `access.rs`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ColourVision {
    #[default]
    Standard = 0,
    /// Red-green safe (protan and deutan).
    RedGreen = 1,
    /// Blue-yellow safe (tritan).
    BlueYellow = 2,
}

impl ColourVision {
    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::RedGreen,
            2 => Self::BlueYellow,
            _ => Self::Standard,
        }
    }
}

/// Region sizes. `None` means "never dragged": the default is used and the
/// region follows the window (spec §2: sizes persist only after a drag).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct Layout {
    pub library_w: Option<f32>,
    pub inspector_w: Option<f32>,
    pub timeline_h: Option<f32>,
    pub library_open: Option<bool>,
    pub inspector_open: Option<bool>,
    pub timeline_open: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub theme: ThemeChoice,
    /// 0.8 ..= 1.5
    pub ui_scale: f32,
    pub reduce_motion: bool,
    pub hover_scrub: bool,
    pub snapping: bool,
    pub magnetic: bool,
    pub cache_gb: f32,
    pub cache_dir: String,
    pub proxy_dir: String,
    pub autosave: bool,
    /// 0: frames from `studio-render`; 1: the simple built-in preview.
    pub preview_engine: u8,
    pub library_list: bool,
    /// Before docking: the fixed regions' sizes. Still written (from the
    /// dock) for an older Studio, and read when there is no `dock` yet.
    pub layout: Layout,
    /// Where every panel is docked (`crate::dock`); `None` until saved.
    pub dock: Option<crate::dock::Tree>,
    /// Action id -> custom shortcut text.
    pub shortcuts: Vec<(String, String)>,
    /// Projects opened or saved, most recent first.
    pub recents: Vec<crate::files::Recent>,
    /// The AI editor: 0 the Claude API, 1 Claude Code, 2 the OpenAI API,
    /// 3 Codex.
    pub ai_provider: u8,
    /// Claude API model id.
    pub ai_model: String,
    /// OpenAI API model id.
    pub ai_openai_model: String,
    /// Model passed to Claude Code; empty: the CLI's own default.
    pub ai_claude_code_model: String,
    /// Model passed to Codex; empty: the CLI's own default.
    pub ai_codex_model: String,
    /// Ask before the AI editor changes the project.
    pub ai_ask_first: bool,
    /// The user has read what the AI editor sends to Anthropic.
    pub ai_cloud_ok: bool,
    /// The AI editor works as a crew: a director plans, specialists work in
    /// parallel, the director reviews.
    pub ai_crew: bool,
    /// Checkpoints in a plan wait for the person's approval.
    pub ai_checkpoints: bool,
    /// Interface language tag (`fr`, `zh-Hans`…); empty follows the system.
    pub language: String,
    /// High-contrast colours (black or white grounds, 7:1 text).
    pub high_contrast: bool,
    /// Palette for colour blindness.
    pub colour_vision: ColourVision,
    /// Speak status changes (undo, export done…) through the screen reader.
    pub announce: bool,
    /// The first-run tour has been seen or skipped.
    pub tour_done: bool,
    /// Show contextual tips.
    pub tips_on: bool,
    /// Tips already shown (ids).
    pub tips_seen: Vec<String>,
    /// The workspace last used (see `workspace.rs`); empty is Edit.
    pub workspace: String,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            theme: ThemeChoice::Dark,
            ui_scale: 1.0,
            reduce_motion: false,
            hover_scrub: false,
            snapping: true,
            magnetic: true,
            cache_gb: 8.0,
            cache_dir: studio_paths::cache().display().to_string(),
            proxy_dir: studio_paths::videos().join("Studio Proxies").display().to_string(),
            autosave: true,
            preview_engine: 0,
            library_list: false,
            layout: Layout::default(),
            dock: None,
            shortcuts: Vec::new(),
            recents: Vec::new(),
            ai_provider: 0,
            ai_model: crate::agent::brain::MODELS[0].0.to_owned(),
            ai_openai_model: crate::agent::openai::MODELS[0].0.to_owned(),
            ai_claude_code_model: String::new(),
            ai_codex_model: String::new(),
            ai_ask_first: false,
            ai_cloud_ok: false,
            ai_crew: false,
            ai_checkpoints: true,
            language: String::new(),
            high_contrast: false,
            colour_vision: ColourVision::Standard,
            announce: true,
            tour_done: false,
            tips_on: true,
            tips_seen: Vec::new(),
            workspace: String::new(),
        }
    }
}

const KEY: &str = "studio-prefs";

impl Prefs {
    pub fn load(storage: Option<&dyn eframe::Storage>) -> Self {
        storage
            .and_then(|s| s.get_string(KEY))
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, storage: &mut dyn eframe::Storage) {
        if let Ok(s) = serde_json::to_string(self) {
            storage.set_string(KEY, s);
        }
    }
}
