// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Command-line options.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Screen {
    #[default]
    Start,
    Editor,
    Export,
    Settings,
    Gallery,
    Paint,
}

#[derive(Clone, Debug, Default)]
pub struct Cli {
    pub screen: Option<Screen>,
    pub theme: Option<bool>,
    pub size: Option<(f32, f32)>,
    pub project: Option<String>,
    /// A project folder to open at launch (`--open DIR`, or the folder
    /// itself as the argument).
    pub open: Option<std::path::PathBuf>,
    pub select: Option<usize>,
    pub shot: Option<String>,
    pub frames: u32,
    pub fresh: bool,
    /// Initial scroll offset for long screens (visual checks).
    pub scroll: Option<f32>,
    /// Extra free-form flags for visual checks, e.g. `--narrow-inspector`.
    pub flags: Vec<String>,
    /// `--ui-script FILE --ui-out DIR`: the app drives itself with no window
    /// (see `ui_script.rs`).
    pub ui_script: Option<std::path::PathBuf>,
    pub ui_out: Option<std::path::PathBuf>,
    /// `--ui-log FILE`: a line a step, `ok` or `failed` and why.
    pub ui_log: Option<std::path::PathBuf>,
    /// Pixels per point of the scripted shots (default 2).
    pub ui_ppp: Option<f32>,
}

impl Cli {
    pub fn parse(args: impl Iterator<Item = String>) -> Self {
        let mut c = Cli { frames: 40, ..Default::default() };
        let mut args = args.peekable();
        while let Some(a) = args.next() {
            let mut val = || args.next().unwrap_or_default();
            match a.as_str() {
                "--screen" => {
                    c.screen = Some(match val().as_str() {
                        "editor" => Screen::Editor,
                        "export" => Screen::Export,
                        "settings" => Screen::Settings,
                        "gallery" => Screen::Gallery,
                        "paint" => Screen::Paint,
                        _ => Screen::Start,
                    })
                }
                "--theme" => c.theme = Some(val() != "light"),
                "--size" => {
                    let v = val();
                    if let Some((w, h)) = v.split_once('x') {
                        if let (Ok(w), Ok(h)) = (w.parse(), h.parse()) {
                            c.size = Some((w, h));
                        }
                    }
                }
                "--project" => c.project = Some(val()),
                "--open" => c.open = Some(val().into()),
                path if !path.starts_with("--") && studio_model::file::ProjectFolder::is_project(std::path::Path::new(path)) => c.open = Some(path.into()),
                "--select" => c.select = val().parse().ok(),
                "--shot" => c.shot = Some(val()),
                "--frames" => c.frames = val().parse().unwrap_or(40),
                "--fresh" => c.fresh = true,
                "--ui-script" => c.ui_script = Some(val().into()),
                "--ui-out" => c.ui_out = Some(val().into()),
                "--ui-log" => c.ui_log = Some(val().into()),
                "--ui-ppp" => c.ui_ppp = val().parse().ok(),
                "--scroll" => c.scroll = val().parse().ok(),
                other => c.flags.push(other.trim_start_matches("--").to_owned()),
            }
        }
        // A scripted run is a screenshot run in every way the app cares
        // about: no dialogs, no saved state, no sounds, no tour.
        if c.ui_script.is_some() && c.shot.is_none() {
            c.shot = Some(String::new());
        }
        if c.shot.is_some() {
            c.fresh = true;
        }
        c
    }

    pub fn flag(&self, f: &str) -> bool {
        self.flags.iter().any(|x| x == f)
    }
}
