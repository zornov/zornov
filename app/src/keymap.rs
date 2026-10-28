// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Keyboard shortcuts as the person set them: the action table in
//! Settings ▸ Shortcuts (`SHORTCUTS`), with the person's own keys from the
//! preferences in place of the defaults. The editor asks [`pressed`] which
//! actions were triggered this frame.

use eframe::egui::{self, Key, KeyboardShortcut, Modifiers};

use crate::prefs::Prefs;
use crate::screens::settings::SHORTCUTS;

/// Reads a shortcut as Settings writes it: modifier symbols (⌃ ⌥ ⇧ ⌘) then
/// a key name or symbol ("⇧⌘Z", "Space", "←", "⌘=", "⌫").
pub fn parse(s: &str) -> Option<KeyboardShortcut> {
    let mut m = Modifiers::NONE;
    let mut rest = s.trim();
    loop {
        let Some(c) = rest.chars().next() else { return None };
        match c {
            '⌃' => m.ctrl = true,
            '⌥' => m.alt = true,
            '⇧' => m.shift = true,
            '⌘' => {
                m.mac_cmd = cfg!(target_os = "macos");
                m.command = true;
                if !cfg!(target_os = "macos") {
                    m.ctrl = true;
                }
            }
            _ => break,
        }
        rest = &rest[c.len_utf8()..];
    }
    let key = match rest {
        "←" => Key::ArrowLeft,
        "→" => Key::ArrowRight,
        "↑" => Key::ArrowUp,
        "↓" => Key::ArrowDown,
        "⌫" => Key::Backspace,
        "⌦" => Key::Delete,
        "⏎" | "↩" => Key::Enter,
        "⎋" => Key::Escape,
        "⇥" => Key::Tab,
        "=" => Key::Equals,
        "-" => Key::Minus,
        "+" => Key::Plus,
        "," => Key::Comma,
        "." => Key::Period,
        "\\" => Key::Backslash,
        "/" => Key::Slash,
        ";" => Key::Semicolon,
        "[" => Key::OpenBracket,
        "]" => Key::CloseBracket,
        "`" => Key::Backtick,
        "'" => Key::Quote,
        other => Key::from_name(other).or_else(|| Key::from_name(&other.to_uppercase()))?,
    };
    Some(KeyboardShortcut::new(m, key))
}

/// The keys of an action: the person's, or the default.
pub fn keys<'a>(prefs: &'a Prefs, id: &str) -> Option<&'a str> {
    match prefs.shortcuts.iter().find(|(a, _)| a == id) {
        Some((_, k)) => Some(k.as_str()).filter(|k| !k.is_empty()),
        None => SHORTCUTS.iter().find(|(a, _, _)| *a == id).map(|(_, _, d)| *d).filter(|d| !d.is_empty()),
    }
}

/// Which of the actions `ids` had their shortcut pressed this frame (each
/// pressed key goes to one action; keys of other actions are left for
/// their own handlers). Shortcuts with more modifiers are tried first, so
/// ⇧⌘Z is redo, not undo.
pub fn pressed(ctx: &egui::Context, prefs: &Prefs, ids: &[&str]) -> Vec<&'static str> {
    let mut table: Vec<(&'static str, KeyboardShortcut)> = SHORTCUTS.iter().filter(|(id, _, _)| ids.contains(id)).filter_map(|(id, _, _)| Some((*id, parse(keys(prefs, id)?)?))).collect();
    let count = |m: Modifiers| m.ctrl as u8 + m.alt as u8 + m.shift as u8 + (m.command || m.mac_cmd) as u8;
    table.sort_by_key(|(_, k)| std::cmp::Reverse(count(k.modifiers)));
    let mut out = Vec::new();
    ctx.input_mut(|i| {
        for (id, k) in table {
            if i.consume_shortcut(&k) {
                out.push(id);
            }
        }
    });
    out
}

/// Whether one action's shortcut was pressed (consumed if so).
pub fn hit(ctx: &egui::Context, prefs: &Prefs, id: &str) -> bool {
    !pressed(ctx, prefs, &[id]).is_empty()
}

/// Actions sharing a shortcut: (keys, actions) for every clash.
pub fn conflicts(prefs: &Prefs) -> Vec<(String, Vec<&'static str>)> {
    let mut by: Vec<(KeyboardShortcut, String, Vec<&'static str>)> = Vec::new();
    for (id, _, _) in SHORTCUTS {
        let Some(k) = keys(prefs, id) else { continue };
        let Some(s) = parse(k) else { continue };
        match by.iter_mut().find(|(x, _, _)| *x == s) {
            Some((_, _, ids)) => ids.push(id),
            None => by.push((s, k.to_owned(), vec![id])),
        }
    }
    by.into_iter().filter(|(_, _, ids)| ids.len() > 1).map(|(_, k, ids)| (k, ids)).collect()
}

/// The other actions on the same keys as `id`, for the shortcut editor.
pub fn clashes_with(prefs: &Prefs, id: &str) -> Vec<&'static str> {
    conflicts(prefs).into_iter().filter(|(_, ids)| ids.contains(&id)).flat_map(|(_, ids)| ids).filter(|x| *x != id).collect()
}

/// Keys the app handles itself, outside Settings ▸ Shortcuts (they
/// cannot be changed there): (keys, what they do, the file under `src/`
/// that handles them). A hint on a button or menu item shows an action's
/// keys from [`hint`], or one of these.
pub const FIXED: &[(&str, &str, &str)] = &[
    ("⌘O", "Open a project", "app.rs"),
    ("⌘S", "Save", "app.rs"),
    ("⇧⌘S", "Save as", "app.rs"),
    ("⌘I", "Import media", "app.rs"),
    ("⇧⌘P", "Paint", "app.rs"),
    ("F1", "Search help", "app.rs"),
    ("⌘L", "Loop playback", "editor/viewer.rs"),
    ("⇧G", "Graph editor", "editor/timeline.rs"),
    ("⇧C", "Compositor", "editor/timeline.rs"),
    ("⇧X", "Sound (and back to the timeline)", "editor/timeline.rs"),
    ("Esc", "Close a popover, menu or the Inspector sheet", "editor/mod.rs"),
    ("⌘↩", "Export now; add a review note", "screens/export.rs"),
    // 3D model editing, while a model is open in edit mode.
    ("Tab", "Edit mode on and off", "editor/model_edit/input.rs"),
    ("W", "Next selection tool", "editor/model_edit/input.rs"),
    ("O", "Proportional editing", "editor/model_edit/input.rs"),
    (".", "Frame the selection", "editor/model_edit/input.rs"),
    ("/", "Local view", "editor/model_edit/input.rs"),
    ("F3", "Search operations", "editor/model_edit/input.rs"),
    ("U", "Unwrap UVs", "editor/model_edit/input.rs"),
    // The 3D viewport.
    ("⇧Tab", "3D workspace on and off", "editor/viewport/ui.rs"),
    ("0", "Look through the camera", "editor/viewport/ui.rs"),
    ("5", "Perspective or orthographic", "editor/viewport/ui.rs"),
    (".", "Frame the selection", "editor/viewport/ui.rs"),
    ("Home", "Frame everything", "editor/viewport/ui.rs"),
    ("`", "Fly mode", "editor/viewport/ui.rs"),
    ("⇧`", "Walk mode", "editor/viewport/ui.rs"),
    ("V", "Third person", "editor/viewport/ui.rs"),
];

/// The 3D editing contexts: their fixed keys are theirs alone, so a hint
/// elsewhere on the same keys means something else.
pub const CONTEXTS: &[&str] = &["editor/model_edit/", "editor/viewport/"];

/// The keys to show next to an action (a button's or menu item's hint):
/// the person's own, else the default; empty when it has none.
pub fn hint(prefs: &Prefs, id: &str) -> String {
    keys(prefs, id).unwrap_or("").to_owned()
}

/// The display name of an action.
pub fn name(id: &str) -> &'static str {
    SHORTCUTS.iter().find(|(a, _, _)| *a == id).map_or("", |(_, n, _)| *n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_settings_writes() {
        let k = parse("⇧⌘Z").unwrap();
        assert!(k.modifiers.shift && k.modifiers.command && k.logical_key == Key::Z);
        assert_eq!(parse("Space").unwrap().logical_key, Key::Space);
        assert_eq!(parse("←").unwrap().logical_key, Key::ArrowLeft);
        assert_eq!(parse("⌘=").unwrap().logical_key, Key::Equals);
        assert_eq!(parse("⌫").unwrap().logical_key, Key::Backspace);
        assert_eq!(parse("⌥X").unwrap().modifiers.alt, true);
        assert_eq!(parse("⌥1").unwrap().logical_key, Key::Num1);
        // What Settings records goes back through.
        for k in [Key::A, Key::F5, Key::ArrowUp, Key::Comma, Key::Num7] {
            assert_eq!(parse(&format!("⌘{}", k.symbol_or_name())).map(|s| s.logical_key), Some(k), "{k:?}");
        }
        assert!(parse("").is_none() && parse("⌘").is_none());
    }

    /// The literal key hints in the interface's source: `.shortcut("…")`
    /// on buttons and menu items, `Some("…")` handed to a tooltip, and the
    /// keys column of the top bar's menus. (file, hint)
    fn literal_hints() -> Vec<(String, String)> {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut out = Vec::new();
        let mut stack = vec![root.clone()];
        while let Some(dir) = stack.pop() {
            for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                let p = e.path();
                let rel = p.strip_prefix(&root).unwrap_or(&p).to_string_lossy().replace('\\', "/");
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                // The component gallery is a developer tool; tests set up their own.
                let stem = p.file_stem().and_then(|f| f.to_str()).unwrap_or("");
                if p.extension().is_none_or(|x| x != "rs") || rel == "gallery.rs" || stem == "tests" || stem.ends_with("_tests") {
                    continue;
                }
                let src = std::fs::read_to_string(&p).unwrap_or_default();
                let src = src.split("#[cfg(test)]").next().unwrap_or("");
                let take = |line: &str, pat: &str| -> Vec<String> {
                    let mut got = Vec::new();
                    let mut rest = line;
                    while let Some(at) = rest.find(pat) {
                        rest = &rest[at + pat.len()..];
                        if let Some(end) = rest.find('"') {
                            got.push(rest[..end].replace("\\\\", "\\"));
                        }
                    }
                    got
                };
                for line in src.lines() {
                    out.extend(take(line, ".shortcut(\"").into_iter().map(|h| (rel.clone(), h)));
                    if line.contains("tip(") || line.contains("tooltip_body(") {
                        out.extend(take(line, "Some(\"").into_iter().map(|h| (rel.clone(), h)));
                    }
                    // `item("Open…", "⌘O", …)` in the top bar, `item(ui, "Duplicate", "⌘D", …)` in the clip menu.
                    for start in ["item(\"", "item(ui, \""] {
                        if let Some(at) = line.find(start) {
                            let after = &line[at + start.len()..];
                            if let Some(keys) = after.split_once("\", \"").and_then(|(_, k)| k.split_once('"')).map(|(k, _)| k) {
                                if !keys.is_empty() {
                                    out.push((rel.clone(), keys.to_owned()));
                                }
                            }
                        }
                    }
                }
            }
        }
        out
    }

    /// Every key hint the interface shows is bound: it comes from the
    /// person's keys ([`keys`], so it follows what they set), or it is one
    /// of the fixed keys ([`FIXED`]) and the file named there handles that key.
    #[test]
    fn every_shown_key_hint_is_bound() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let hints = literal_hints();
        assert!(hints.len() >= 20, "the scan found only {hints:?}");
        // Hints that are words, not keys.
        let words = ["double-click", "Esc cancels"];
        let mut wrong = Vec::new();
        for (file, h) in &hints {
            if words.contains(&h.as_str()) {
                continue;
            }
            let Some(k) = parse(h) else {
                wrong.push(format!("{file}: \"{h}\" does not read as keys"));
                continue;
            };
            let fixed = FIXED.iter().any(|(f, _, handler)| {
                // A 3D context's own keys count only inside that context.
                let in_scope = CONTEXTS.iter().find(|c| handler.starts_with(*c)).is_none_or(|c| file.starts_with(c));
                parse(f) == Some(k) && in_scope
            });
            if fixed {
                continue;
            }
            match SHORTCUTS.iter().find(|(_, _, d)| parse(d) == Some(k)) {
                Some((id, _, _)) => wrong.push(format!("{file}: \"{h}\" is the action \"{id}\": show crate::keymap::hint(prefs, \"{id}\") so it follows the person's keys")),
                None => wrong.push(format!("{file}: \"{h}\" is bound to nothing (not in Settings ▸ Shortcuts nor keymap::FIXED)")),
            }
        }
        assert!(wrong.is_empty(), "key hints that are not bound:\n  {}", wrong.join("\n  "));
        // Each fixed key is handled where FIXED says.
        for (keys, what, file) in FIXED {
            let k = parse(keys).unwrap_or_else(|| panic!("FIXED \"{keys}\" does not read as keys"));
            let src = std::fs::read_to_string(root.join(file)).unwrap_or_else(|e| panic!("FIXED \"{keys}\" ({what}): {file}: {e}"));
            let name = format!("Key::{:?}", k.logical_key);
            assert!(src.contains(&name), "FIXED \"{keys}\" ({what}): {file} never looks at {name}");
        }
        // The actions a hint names are bound by default.
        for id in ["agent", "overlays", "all-projects"] {
            assert!(keys(&Prefs::default(), id).and_then(parse).is_some(), "{id} has no default keys");
        }
    }

    #[test]
    fn defaults_have_no_clashes_and_custom_keys_are_found() {
        let mut p = Prefs::default();
        assert!(conflicts(&p).is_empty(), "{:?}", conflicts(&p));
        p.shortcuts.push(("split".into(), "B".into()));
        assert_eq!(clashes_with(&p, "split"), vec!["blade"]);
        assert_eq!(keys(&p, "split"), Some("B"));
    }
}
