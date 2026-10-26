// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Hard-coded interface text is caught here.
//!
//! The scanner finds string literals handed to the interface (a button's
//! label, a tooltip, a heading, a `label:` field…) in the app's UI sources.
//! Each one has to be a message in `locales/en.ftl`, so it can be
//! translated. When you add UI text and this test fails, run
//!
//! ```text
//! cargo run -p studio-i18n --bin i18n-scan -- --add
//! ```
//!
//! which adds the missing messages to `en.ftl` and lists them in
//! `pending.txt` (shown in English until a translator reaches them).
//!
//! `hardcoded_baseline.txt` lists literals that predate the rule or are not
//! really interface text; the second test makes sure it only shrinks.

use std::collections::BTreeSet;
use std::path::PathBuf;
use studio_i18n::scan;

fn app_src() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../app/src")
}

fn baseline() -> BTreeSet<String> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/hardcoded_baseline.txt");
    std::fs::read_to_string(p).unwrap_or_default().lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).map(str::to_owned).collect()
}

/// `file<TAB>text` of every literal that is interface text but has no message.
fn violations() -> BTreeSet<String> {
    let root = app_src();
    let mut out = BTreeSet::new();
    for f in scan::ui_files(&root) {
        let rel = f.strip_prefix(&root).unwrap_or(&f).display().to_string();
        // The component gallery is a developer tool.
        if rel == "gallery.rs" {
            continue;
        }
        let Ok(src) = std::fs::read_to_string(&f) else { continue };
        for hit in scan::scan_source(&rel, &src) {
            if scan::is_prose(&hit.text) && !studio_i18n::covered(&hit.text) {
                out.insert(format!("{}\t{}", hit.file, hit.text.escape_debug()));
            }
        }
    }
    out
}

#[test]
fn interface_text_is_in_the_catalogue() {
    let base = baseline();
    let new: Vec<_> = violations().into_iter().filter(|v| !base.contains(v)).collect();
    assert!(
        new.is_empty(),
        "{} interface string(s) are not in crates/project/i18n/locales/en.ftl:\n  {}\n\nAdd them with:  cargo run -p studio-i18n --bin i18n-scan -- --add\n(see docs/localisation.md)",
        new.len(),
        new.join("\n  ")
    );
}

#[test]
fn the_baseline_only_shrinks() {
    let now = violations();
    let stale: Vec<_> = baseline().into_iter().filter(|b| !now.contains(b)).collect();
    assert!(stale.is_empty(), "these baseline entries are fixed (or gone); delete them from tests/hardcoded_baseline.txt:\n  {}", stale.join("\n  "));
}

#[test]
fn the_scanner_sees_the_interface() {
    // Guards the scanner itself: the app has well over a hundred literal
    // arguments to interface calls.
    let root = app_src();
    let mut n = 0;
    for f in scan::ui_files(&root) {
        if let Ok(src) = std::fs::read_to_string(&f) {
            n += scan::scan_source("x", &src).len();
        }
    }
    assert!(n > 150, "only {n} interface literals found: is {} the app's source?", root.display());
}
