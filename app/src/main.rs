// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Studio desktop application.
//!
//! Command-line options (mostly for development and visual checks):
//!
//! ```text
//! studio mcp [--project DIR]          agent server (MCP over stdio, headless)
//! studio agent [--project DIR] TOOL [JSON]   one agent call from a shell
//! studio eval --out DIR [--solo]       the AI editor on scored jobs, headless
//! studio [--screen start|editor|export|settings|gallery] [--theme dark|light]
//!        [--size 1440x900] [--project demo|empty|one|big] [--select N]
//!        [--shot out.png] [--frames N] [--fresh]
//! ```
//!
//! `--shot` renders `--frames` frames (default 40), saves a screenshot of the
//! window as PNG and exits.

mod agent;
mod convert;
mod transcode;
mod psd_import;
mod release;
#[cfg(feature = "render")]
mod lottie_export;
mod app;
mod bench;
mod bridge;
mod catalogue;
mod chime;
mod audio_preview;
mod cli;
mod content;
mod collab;
mod doc;
mod editor;
#[cfg(feature = "render")]
mod deliver_run;
mod export_pro;
#[cfg(feature = "render")]
mod export_run;
mod files;
mod formats;
mod gallery;
mod a11y;
mod help;
mod onboarding;
mod templates;
#[cfg(test)]
mod ui_tests;
#[cfg(test)]
mod ui_shot;
#[cfg(test)]
mod ui_overflow;
mod access;
mod i18n;
mod icons;
mod kit;
mod library;
mod asset_meta;
mod assets;
mod rights;
mod safety;
mod recover;
mod media_io;
mod paint;
mod mesh_store;
mod my_models;
mod view;
mod prefs;
mod screens;
mod script_import;
mod services;
#[cfg(test)]
mod perf;
mod physics3d;
mod plugins;
mod keymap;
mod macros;
mod workspaces;
mod dock;
mod sound_bridge;
mod sound_desk;
mod sqlite;
mod text_look;
mod theme;
mod thumbs;

fn main() -> eframe::Result {
    // `studio ofx-worker BUNDLE`: hosts one OpenFX bundle for the editor.
    studio_plugins::ofx::worker_if_asked();
    studio_crash::install(env!("CARGO_PKG_VERSION"));
    plugins::start();
    // `studio mcp`: the agent server on stdin / stdout, no window.
    let argv: Vec<String> = std::env::args().skip(1).collect();
    match argv.first().map(String::as_str) {
        Some("mcp") => std::process::exit(agent::cli::mcp(&argv[1..])),
        // `studio agent TOOL JSON`: one call from a shell or script.
        Some("agent") => std::process::exit(agent::cli::agent(&argv[1..])),
        // `studio eval`: the AI editor on scored jobs, headless.
        Some("eval") => std::process::exit(agent::eval::main(&argv[1..])),
        // `studio node`: a render node other Studios on the network send pieces of a render to.
        #[cfg(feature = "render")]
        Some("node") => std::process::exit(deliver_run::node_main(&argv[1..])),
        // `studio queue run|list`: the render queue from a shell.
        #[cfg(feature = "render")]
        Some("queue") => std::process::exit(deliver_run::queue_main(&argv[1..])),
        _ => {}
    }
    // `studio` in a terminal: the prompt, `studio new`, `studio see`…
    if let Some(code) = agent::shell::main(&argv) {
        std::process::exit(code);
    }
    // Programs a previous Studio left behind (it was killed or crashed).
    agent::procs::reap_orphans();
    let cli = cli::Cli::parse(std::env::args().skip(1));
    if cli.shot.is_none() {
        release::startup();
    }
    let (w, h) = cli.size.unwrap_or((1440.0, 900.0));
    let viewport = eframe::egui::ViewportBuilder::default()
        .with_title("Studio")
        .with_app_id("studio")
        .with_icon(std::sync::Arc::new(eframe::icon_data::from_png_bytes(include_bytes!("../../packaging/brand/hicolor/256x256/apps/zornov.png")).unwrap_or_default()))
        .with_inner_size([w, h])
        .with_min_inner_size([960.0, 600.0])
        .with_fullsize_content_view(true)
        .with_titlebar_shown(false)
        .with_title_shown(false)
        // Screenshot runs must not take focus from whatever the person
        // is doing.
        .with_active(cli.shot.is_none());
    let options = eframe::NativeOptions {
        viewport,
        persist_window: cli.size.is_none() && !cli.fresh,
        // A fresh run (and every screenshot run) must not read or write the
        // user's saved state, including egui's own memory.
        persistence_path: cli.fresh.then(|| std::env::temp_dir().join(format!("studio-fresh-{}", std::process::id()))),
        ..Default::default()
    };
    eframe::run_native("Studio", options, Box::new(move |cc| Ok(Box::new(app::StudioApp::new(cc, cli)))))
}
