// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `zornov-cli`: Zornov in a terminal on Windows.
//!
//! `zornov.exe` is a window program there, so a double click or the Start
//! menu opens the editor with no console beside it. A window program cannot
//! hold a terminal, though: the prompt, `zornov-cli new`, `zornov-cli agent`…
//! go through this small console program, which starts the `zornov` beside it
//! on the same console and waits for it.

#[cfg(windows)]
unsafe extern "system" fn keep_waiting(_event: u32) -> i32 {
    // Ctrl+C reaches zornov on the same console; this one stays to wait for it.
    1
}

fn main() {
    let name = if cfg!(windows) { "zornov.exe" } else { "zornov" };
    let exe = std::env::current_exe().map(|p| p.with_file_name(name)).unwrap_or_else(|_| name.into());
    #[cfg(windows)]
    // SAFETY: a handler that only returns; set before anything else runs.
    unsafe {
        windows_sys::Win32::System::Console::SetConsoleCtrlHandler(Some(keep_waiting), 1);
    }
    match std::process::Command::new(&exe).args(std::env::args_os().skip(1)).env("ZORNOV_CONSOLE", "1").status() {
        Ok(s) => std::process::exit(s.code().unwrap_or(1)),
        Err(e) => {
            eprintln!("zornov-cli: cannot start {}: {e}", exe.display());
            std::process::exit(1)
        }
    }
}
