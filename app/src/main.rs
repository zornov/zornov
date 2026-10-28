// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Zornov desktop application.
//!
//! Command-line options (mostly for development and visual checks):
//!
//! ```text
//! zornov mcp [--project DIR]          agent server (MCP over stdio, headless)
//! zornov agent [--project DIR] TOOL [JSON]   one agent call from a shell
//! zornov eval --out DIR [--solo]       the AI editor on scored jobs, headless
//! zornov [--screen start|editor|export|settings|gallery] [--theme dark|light]
//!        [--size 1440x900] [--project demo|empty|one|big] [--select N]
//!        [--shot out.png] [--frames N] [--fresh]
//! ```
//!
//! `--shot` renders `--frames` frames (default 40), saves a screenshot of the
//! window as PNG and exits.
//!
//! On Windows this is a window program, so a double click or the Start menu
//! opens the editor with no console beside it; the terminal side runs through
//! `zornov-cli.exe` (`src/bin/zornov-cli.rs`).

#![cfg_attr(all(windows, not(test)), windows_subsystem = "windows")]

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
mod library_download;
mod sqlite;
mod text_look;
mod theme;
mod thumbs;

fn main() -> eframe::Result {
    // Started by `zornov-cli.exe`: use its console for the prompt and the
    // commands. Programs started from here must not inherit the request.
    #[cfg(windows)]
    if std::env::var_os("ZORNOV_CONSOLE").is_some() {
        use windows_sys::Win32::Storage::FileSystem::{FILE_TYPE_DISK, FILE_TYPE_PIPE, GetFileType};
        use windows_sys::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, SetStdHandle};
        // SAFETY: first thing in main, before any other thread exists.
        unsafe {
            std::env::remove_var("ZORNOV_CONSOLE");
            // Pipes and files zornov-cli passed on (an MCP client's stdin
            // and stdout, `> out.json`) stay where they point: attaching to
            // the console must not swap them for it.
            let kept: Vec<_> = [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE].into_iter().map(|n| (n, GetStdHandle(n))).filter(|&(_, h)| matches!(GetFileType(h), FILE_TYPE_PIPE | FILE_TYPE_DISK)).collect();
            AttachConsole(ATTACH_PARENT_PROCESS);
            for (n, h) in kept {
                SetStdHandle(n, h);
            }
        }
    }
    // DirectX 12 with DXC when it is next to the program (build.rs puts it
    // there): Vulkan crashes in some drivers (Intel UHD) and FXC, DirectX 12's
    // own compiler, cannot compile every shader. WGPU_BACKEND or
    // WGPU_DX12_COMPILER set by hand still win.
    #[cfg(windows)]
    if std::env::var_os("WGPU_BACKEND").is_none() && std::env::current_exe().is_ok_and(|e| e.with_file_name("dxcompiler.dll").is_file()) {
        // SAFETY: still before any other thread exists.
        unsafe {
            std::env::set_var("WGPU_BACKEND", "dx12");
            if std::env::var_os("WGPU_DX12_COMPILER").is_none() {
                std::env::set_var("WGPU_DX12_COMPILER", "dxc");
            }
        }
    }
    // `zornov ofx-worker BUNDLE`: hosts one OpenFX bundle for the editor.
    studio_plugins::ofx::worker_if_asked();
    // Folders kept under the old name (Studio) move to Zornov before
    // anything reads settings, the signing key or the window layout.
    studio_paths::migrate_legacy();
    studio_crash::install(env!("CARGO_PKG_VERSION"));
    plugins::start();
    // `zornov mcp`: the agent server on stdin / stdout, no window.
    let argv: Vec<String> = std::env::args().skip(1).collect();
    match argv.first().map(String::as_str) {
        Some("mcp") => agent::trace::exit(agent::cli::mcp(&argv[1..])),
        // `zornov agent TOOL JSON`: one call from a shell or script.
        Some("agent") => agent::trace::exit(agent::cli::agent(&argv[1..])),
        // `zornov eval`: the AI editor on scored jobs, headless.
        Some("eval") => std::process::exit(agent::eval::main(&argv[1..])),
        // `zornov trace report|on|off`: what agents ran into (the flight recorder).
        Some("trace") => std::process::exit(agent::trace_report::main(&argv[1..])),
        // `zornov replay TRACE`: a recorded agent session again, on this build.
        Some("replay") => std::process::exit(agent::replay::main(&argv[1..])),
        // `zornov dev ask|retry|swap`: talk to the agent inside a window (development builds).
        #[cfg(feature = "agent-dev")]
        Some("dev") => std::process::exit(agent::dev::main(&argv[1..])),
        #[cfg(not(feature = "agent-dev"))]
        Some("dev") => {
            eprintln!("zornov dev: this build has no developer tools (they are in development builds made with --features agent-dev)");
            std::process::exit(2)
        }
        // `zornov node`: a render node other Studios on the network send pieces of a render to.
        #[cfg(feature = "render")]
        Some("node") => std::process::exit(deliver_run::node_main(&argv[1..])),
        // `zornov queue run|list`: the render queue from a shell.
        #[cfg(feature = "render")]
        Some("queue") => std::process::exit(deliver_run::queue_main(&argv[1..])),
        _ => {}
    }
    // `zornov` in a terminal: the prompt, `zornov new`, `zornov see`…
    if let Some(code) = agent::shell::main(&argv) {
        agent::trace::exit(code);
    }
    // Programs a previous Studio left behind (it was killed or crashed).
    agent::procs::reap_orphans();
    let cli = cli::Cli::parse(std::env::args().skip(1));
    if cli.shot.is_none() {
        release::startup();
    }
    let (w, h) = cli.size.unwrap_or((1440.0, 900.0));
    let viewport = eframe::egui::ViewportBuilder::default()
        .with_title("Zornov")
        .with_app_id("zornov")
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
    eframe::run_native("Zornov", options, Box::new(move |cc| Ok(Box::new(app::StudioApp::new(cc, cli)))))
}
