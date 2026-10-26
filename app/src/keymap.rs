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

    #[test]
    fn defaults_have_no_clashes_and_custom_keys_are_found() {
        let mut p = Prefs::default();
        assert!(conflicts(&p).is_empty(), "{:?}", conflicts(&p));
        p.shortcuts.push(("split".into(), "B".into()));
        assert_eq!(clashes_with(&p, "split"), vec!["blade"]);
        assert_eq!(keys(&p, "split"), Some("B"));
    }
}
