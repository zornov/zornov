// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Gives the Windows program its icon, the one Explorer and the taskbar show.
//! Other systems get theirs from the app bundle or the desktop entry.
//!
//! On Windows it also puts DXC, the DirectX shader compiler, next to the
//! program: Zornov draws through DirectX 12 with it (see `src/main.rs`),
//! because Vulkan crashes in some drivers (Intel UHD) and FXC, the compiler
//! DirectX 12 has built in, cannot compile every shader. Packages take it
//! from there.

use std::path::{Path, PathBuf};

/// The redistributable DXC files, as the Windows SDK ships them.
const DXC: [&str; 2] = ["dxcompiler.dll", "dxil.dll"];

fn main() {
    println!("cargo:rerun-if-changed=../packaging/brand/zornov.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon("../packaging/brand/zornov.ico");
    if let Err(e) = res.compile() {
        println!("cargo:warning=the Windows icon was not embedded: {e}");
    }
    // Only a Windows machine has the SDK; a check from another system
    // (the container's `cargo check --target x86_64-pc-windows-gnu`) skips it.
    if cfg!(windows) {
        copy_dxc();
    }
}

fn copy_dxc() {
    println!("cargo:rerun-if-env-changed=STUDIO_DXC_DIR");
    let Some(src) = dxc_dir() else {
        println!("cargo:warning=DXC was not found (install the Windows SDK, or set STUDIO_DXC_DIR): this build draws with Vulkan, which crashes on some GPUs");
        return;
    };
    // OUT_DIR is <target>/<profile>/build/studio-app-<hash>/out; the program
    // goes in <target>/<profile>.
    let Some(bin) = std::env::var_os("OUT_DIR").map(PathBuf::from).and_then(|o| o.ancestors().nth(3).map(Path::to_path_buf)) else {
        return;
    };
    for f in DXC {
        println!("cargo:rerun-if-changed={}", src.join(f).display());
        // A Zornov running from this folder keeps its copy locked; the one
        // already there is the same file.
        if let Err(e) = std::fs::copy(src.join(f), bin.join(f)) {
            println!("cargo:warning={f} was not copied next to the program: {e}");
        }
    }
}

/// `STUDIO_DXC_DIR`, or the Windows SDK's redistributable folder.
fn dxc_dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("STUDIO_DXC_DIR") {
        return Some(d.into());
    }
    let arch = if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("aarch64") { "arm64" } else { "x64" };
    let x86 = std::env::var_os("ProgramFiles(x86)").map_or_else(|| PathBuf::from(r"C:\Program Files (x86)"), PathBuf::from);
    let kits = x86.join(r"Windows Kits\10\Redist\D3D").join(arch);
    DXC.iter().all(|f| kits.join(f).is_file()).then_some(kits)
}
