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
    /// No chime when an export finishes (`chime::play_unless`).
    pub quiet_chime: bool,
    /// Inspector sections the person opened or collapsed, by section kind
    /// (`true`: collapsed). A kind not listed keeps its own default.
    pub collapsed_sections: Vec<(String, bool)>,
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
            proxy_dir: studio_paths::videos().join("Zornov Proxies").display().to_string(),
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
            quiet_chime: false,
            collapsed_sections: Vec::new(),
        }
    }
}

const KEY: &str = "studio-prefs";

static NOTICE: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// What the person should hear about the settings they started with, once.
pub fn take_notice() -> Option<String> {
    NOTICE.lock().unwrap_or_else(|e| e.into_inner()).take()
}

/// Settings that no longer read as a whole: every setting that still does
/// is kept (the tour stays done, the shortcuts stay), the rest is default.
fn salvage(raw: &str) -> Prefs {
    let Ok(serde_json::Value::Object(found)) = serde_json::from_str::<serde_json::Value>(raw) else { return Prefs::default() };
    let Ok(serde_json::Value::Object(mut base)) = serde_json::to_value(Prefs::default()) else { return Prefs::default() };
    for (key, value) in found {
        let mut trial = base.clone();
        trial.insert(key, value);
        if serde_json::from_value::<Prefs>(serde_json::Value::Object(trial.clone())).is_ok() {
            base = trial;
        }
    }
    serde_json::from_value(serde_json::Value::Object(base)).unwrap_or_default()
}

/// Reads the settings; an unreadable copy is kept in `dir` and reported.
fn read_in(raw: Option<String>, dir: &std::path::Path) -> (Prefs, Option<String>) {
    let Some(raw) = raw else { return (Prefs::default(), None) };
    match serde_json::from_str(&raw) {
        Ok(p) => (p, None),
        Err(e) => {
            let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
            let copy = dir.join(format!("settings-unreadable-{stamp}.json"));
            let kept = std::fs::create_dir_all(dir).and_then(|()| std::fs::write(&copy, &raw)).is_ok();
            let tail = if kept { format!("; the old settings are kept in {}", copy.display()) } else { String::new() };
            (salvage(&raw), Some(format!("Some settings could not be read ({e}) and went back to their defaults{tail}")))
        }
    }
}

impl Prefs {
    pub fn load(storage: Option<&dyn eframe::Storage>) -> Self {
        let (prefs, note) = read_in(storage.and_then(|s| s.get_string(KEY)), &studio_paths::studio());
        if note.is_some() {
            *NOTICE.lock().unwrap_or_else(|e| e.into_inner()) = note;
        }
        prefs
    }

    pub fn save(&self, storage: &mut dyn eframe::Storage) {
        match serde_json::to_string(self) {
            Ok(s) => storage.set_string(KEY, s),
            Err(e) => eprintln!("zornov: settings were not saved: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quiet_chime_is_off_by_default_and_old_settings_still_read() {
        assert!(!Prefs::default().quiet_chime, "the chime plays unless asked to be quiet");
        let (p, note) = read_in(Some(r#"{"tour_done":true}"#.into()), &std::env::temp_dir().join("zornov-prefs-quiet"));
        assert!(note.is_none() && !p.quiet_chime && p.tour_done, "settings saved before the option exist read as before");
        let quiet = serde_json::to_string(&Prefs { quiet_chime: true, ..Prefs::default() }).unwrap();
        assert!(read_in(Some(quiet), &std::env::temp_dir()).0.quiet_chime, "the choice survives a save");
    }

    #[test]
    fn collapsed_sections_survive_a_save_and_old_settings_have_none() {
        assert!(Prefs::default().collapsed_sections.is_empty());
        let (old, note) = read_in(Some(r#"{"tour_done":true}"#.into()), &std::env::temp_dir().join("zornov-prefs-sections"));
        assert!(note.is_none() && old.collapsed_sections.is_empty(), "settings saved before the option exist still load");
        let saved = serde_json::to_string(&Prefs { collapsed_sections: vec![("transform".into(), true), ("crop".into(), false)], ..Prefs::default() }).unwrap();
        let back = read_in(Some(saved), &std::env::temp_dir()).0;
        assert_eq!(back.collapsed_sections, vec![("transform".to_owned(), true), ("crop".to_owned(), false)]);
    }

    #[test]
    fn unreadable_settings_keep_what_still_reads_and_are_reported() {
        let dir = std::env::temp_dir().join(format!("zornov-prefs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // One wrong value among good ones: the tour stays done.
        let raw = r#"{"tour_done":true,"autosave":"yes please","tips_on":false}"#.to_owned();
        let (p, note) = read_in(Some(raw.clone()), &dir);
        assert!(p.tour_done && !p.tips_on, "settings that read are kept");
        assert!(p.autosave, "the one that did not read is the default");
        let note = note.expect("the person is told");
        assert!(note.contains("settings-unreadable-"), "{note}");
        let kept: Vec<_> = std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).collect();
        assert_eq!(kept.len(), 1);
        assert_eq!(std::fs::read_to_string(kept[0].path()).unwrap(), raw, "the old file is kept as it was");
        // Whole nonsense gives defaults and still tells.
        let (p, note) = read_in(Some("not json".into()), &dir);
        assert_eq!(p.tour_done, Prefs::default().tour_done);
        assert!(note.is_some());
        // Nothing stored, or a good copy: no note.
        assert!(read_in(None, &dir).1.is_none());
        assert!(read_in(serde_json::to_string(&Prefs::default()).ok(), &dir).1.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
