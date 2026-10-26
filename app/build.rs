// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Gives the Windows program its icon, the one Explorer and the taskbar show.
//! Other systems get theirs from the app bundle or the desktop entry.

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
}
