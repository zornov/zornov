// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Finds user-facing string literals in Rust source, for the tests that
//! stop hard-coded text from creeping in and for the tool that lists what
//! still needs a message.
//!
//! A literal counts when it is an argument of a known UI call
//! (`Button::primary("Export")`, `ui.label("…")`, `.on_hover_text("…")`) or
//! the value of a known field (`label: "Undo"`). Anything routed through
//! [`crate::t`], [`crate::tr`] or [`crate::shown`] is by construction fine
//! and is not reported.

use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Found {
    pub file: String,
    pub line: usize,
    pub text: String,
    /// The call or field that takes it.
    pub sink: String,
}

/// `(call name, argument indices that are text)`. A name matches when the
/// code before the `(` ends with it.
const CALLS: &[(&str, &[usize])] = &[
    ("Button::primary", &[0]),
    ("Button::secondary", &[0]),
    ("Button::ghost", &[0]),
    ("Button::new", &[0]),
    ("IconButton::new", &[1]),
    ("MenuItem::new", &[0]),
    ("kit::menu_heading", &[1]),
    ("menu_heading", &[1]),
    ("labeled_row", &[1]),
    ("kit::labeled_row", &[1]),
    ("kit::tip", &[1]),
    ("kit::chip", &[1]),
    ("kit::text_field", &[2]),
    ("text_field", &[2]),
    ("kit::toggle", &[2]),
    ("kit::empty_state", &[2]),
    ("empty_state", &[2]),
    ("kit::modal", &[3]),
    ("modal", &[3]),
    ("Section::new", &[0]),
    ("FieldRow::new", &[0]),
    ("NumberField::new", &[0]),
    ("Slider::new", &[0]),
    (".on_hover_text", &[0]),
    (".hint_text", &[0]),
    (".label", &[0]),
    (".heading", &[0]),
    ("RichText::new", &[0]),
    ("toast::push", &[1]),
    ("kit::toast::push", &[1]),
];

/// Struct fields whose string value is shown to people.
const FIELDS: &[&str] = &["label", "title", "hint", "tooltip", "caption", "description", "sentence", "heading"];

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == ':' || c == '.'
}

/// Blank out comments, keeping every offset and newline.
fn strip_comments(src: &str) -> Vec<char> {
    let c: Vec<char> = src.chars().collect();
    let mut out = c.clone();
    let mut i = 0;
    while i < c.len() {
        if c[i] == '"' {
            // Skip the literal so `//` inside it is not a comment.
            i += 1;
            while i < c.len() && c[i] != '"' {
                if c[i] == '\\' {
                    i += 1;
                }
                i += 1;
            }
            i += 1;
            continue;
        }
        if c[i] == '\'' && i + 2 < c.len() && (c[i + 2] == '\'' || (c[i + 1] == '\\' && c.get(i + 3) == Some(&'\''))) {
            i += if c[i + 1] == '\\' { 4 } else { 3 };
            continue;
        }
        if c[i] == '/' && c.get(i + 1) == Some(&'/') {
            while i < c.len() && c[i] != '\n' {
                out[i] = ' ';
                i += 1;
            }
            continue;
        }
        if c[i] == '/' && c.get(i + 1) == Some(&'*') {
            let mut depth = 0;
            while i < c.len() {
                if c[i] == '/' && c.get(i + 1) == Some(&'*') {
                    depth += 1;
                } else if c[i] == '*' && c.get(i + 1) == Some(&'/') {
                    depth -= 1;
                    if depth == 0 {
                        out[i] = ' ';
                        out[i + 1] = ' ';
                        i += 2;
                        break;
                    }
                }
                if c[i] != '\n' {
                    out[i] = ' ';
                }
                i += 1;
            }
            continue;
        }
        i += 1;
    }
    out
}

/// All user-facing literals in one source text.
pub fn scan_source(file: &str, src: &str) -> Vec<Found> {
    let c = strip_comments(src);
    let mut found = Vec::new();
    // Cut `#[cfg(test)] mod tests { … }`: test text is not UI.
    let end = src.find("#[cfg(test)]").map(|b| src[..b].chars().count()).unwrap_or(c.len());
    let mut i = 0;
    let mut line = 1;
    while i < end {
        let ch = c[i];
        if ch == '\n' {
            line += 1;
            i += 1;
            continue;
        }
        if ch == '\'' && i + 2 < c.len() && (c[i + 2] == '\'' || (c[i + 1] == '\\' && c.get(i + 3) == Some(&'\''))) {
            i += if c[i + 1] == '\\' { 4 } else { 3 };
            continue;
        }
        if ch != '"' {
            i += 1;
            continue;
        }
        // A string literal starting at i.
        let start = i;
        let start_line = line;
        let mut text = String::new();
        i += 1;
        while i < c.len() && c[i] != '"' {
            if c[i] == '\\' {
                i += 1;
                match c.get(i) {
                    Some('n') => text.push('\n'),
                    Some('"') => text.push('"'),
                    Some('\\') => text.push('\\'),
                    Some('u') => {
                        // \u{…}
                        let mut hex = String::new();
                        i += 2;
                        while i < c.len() && c[i] != '}' {
                            hex.push(c[i]);
                            i += 1;
                        }
                        if let Some(ch) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                            text.push(ch);
                        }
                    }
                    Some('\n') => {
                        line += 1;
                        // A line continuation swallows the next indentation.
                        while i + 1 < c.len() && c[i + 1].is_whitespace() {
                            if c[i + 1] == '\n' {
                                line += 1;
                            }
                            i += 1;
                        }
                    }
                    Some(o) => text.push(*o),
                    None => {}
                }
            } else {
                if c[i] == '\n' {
                    line += 1;
                }
                text.push(c[i]);
            }
            i += 1;
        }
        i += 1;
        if let Some(sink) = sink_of(&c, start) {
            found.push(Found { file: file.to_owned(), line: start_line, text, sink });
        }
    }
    found
}

/// Which UI call or field takes the literal that starts at `pos`?
fn sink_of(c: &[char], pos: usize) -> Option<String> {
    // Field: `label: "x"`.
    let mut k = pos;
    while k > 0 && c[k - 1].is_whitespace() {
        k -= 1;
    }
    if k > 0 && c[k - 1] == ':' && !(k > 1 && c[k - 2] == ':') {
        let mut s = k - 1;
        while s > 0 && (c[s - 1].is_alphanumeric() || c[s - 1] == '_') {
            s -= 1;
        }
        let name: String = c[s..k - 1].iter().collect();
        if FIELDS.contains(&name.as_str()) {
            return Some(format!("{name}:"));
        }
    }
    // Call argument: walk back to the unmatched `(`, counting commas.
    let mut depth = 0i32;
    let mut commas = 0usize;
    let mut j = pos;
    let mut guard = 0;
    while j > 0 && guard < 600 {
        guard += 1;
        j -= 1;
        match c[j] {
            ')' | ']' | '}' => depth += 1,
            '(' | '[' | '{' if depth > 0 => depth -= 1,
            '(' => {
                let mut e = j;
                while e > 0 && c[e - 1].is_whitespace() {
                    e -= 1;
                }
                let mut s = e;
                while s > 0 && is_ident(c[s - 1]) {
                    s -= 1;
                }
                let name: String = c[s..e].iter().collect();
                for (call, idxs) in CALLS {
                    if name.ends_with(call) && idxs.contains(&commas) {
                        return Some((*call).to_owned());
                    }
                }
                return None;
            }
            '[' | '{' => return None,
            ',' if depth == 0 => commas += 1,
            ';' if depth == 0 => return None,
            _ => {}
        }
    }
    None
}

/// Is this literal worth translating (has letters, not a format-only or
/// identifier-like string)?
pub fn is_prose(s: &str) -> bool {
    let t = s.trim();
    let letters = t.chars().filter(|c| c.is_alphabetic()).count();
    letters >= 2 && (t.contains(' ') || t.chars().next().is_some_and(char::is_uppercase))
}

/// Every `.rs` file under `dir`, sorted.
pub fn rust_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    fn walk(d: &Path, out: &mut Vec<PathBuf>) {
        let Ok(rd) = std::fs::read_dir(d) else { return };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }
    walk(dir, &mut out);
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(src: &str) -> Vec<String> {
        scan_source("x.rs", src).into_iter().map(|f| f.text).collect()
    }

    #[test]
    fn finds_calls_and_fields() {
        let src = r#"
            Button::primary("Export").show(ui);
            kit::tip(resp, "Rename project", Some("⌘R"));
            let it = Item { label: "Undo", shortcut: "⌘Z" };
            ui.label(RichText::new("Not saved"));
        "#;
        assert_eq!(texts(src), ["Export", "Rename project", "Undo", "Not saved"]);
    }

    #[test]
    fn ignores_other_strings_comments_and_tests() {
        let src = r#"
            // Button::primary("In a comment")
            let id = Id::new("topbar-layout");
            println!("Hello");
            Button::primary(crate::i18n::t("Export"));
            #[cfg(test)] mod tests { fn f() { ui.label("In tests"); } }
        "#;
        assert!(texts(src).is_empty(), "{:?}", texts(src));
    }

    #[test]
    fn argument_position_matters() {
        // Only argument 1 of `tip` is text; the shortcut is not.
        assert_eq!(texts(r#"kit::tip(r, "Name", Some("Space"));"#), ["Name"]);
    }

    #[test]
    fn handles_escapes_and_lines() {
        let f = scan_source("x.rs", "\n\nui.label(\"a \\\"b\\\"\\nc\");");
        assert_eq!(f[0].text, "a \"b\"\nc");
        assert_eq!(f[0].line, 3);
    }

    #[test]
    fn prose_filter() {
        assert!(is_prose("Export"));
        assert!(is_prose("Not saved"));
        assert!(!is_prose("⌘E"));
        assert!(!is_prose("12"));
    }
}

/// Does `s` read like text a person is shown, as opposed to an id, a key
/// or a path? Deliberately generous: an extra entry in the catalogue
/// costs a translation, a missed one costs a hard-coded string.
pub fn looks_like_ui_text(s: &str) -> bool {
    let t = s.trim();
    let Some(first) = t.chars().next() else { return false };
    if t.chars().count() < 2 || t.chars().count() > 140 {
        return false;
    }
    if !(first.is_uppercase() || first.is_ascii_digit() && t.contains(' ')) {
        return false;
    }
    if t.contains("::") || t.contains('_') || t.contains('/') && !t.contains(' ') || t.starts_with("http") {
        return false;
    }
    // ALL-CAPS short words are format or unit names (RGB, LUT, MP4).
    let letters: Vec<char> = t.chars().filter(|c| c.is_alphabetic()).collect();
    if letters.len() < 2 || (letters.iter().all(|c| c.is_uppercase()) && letters.len() <= 6) {
        return false;
    }
    // Identifier-like single tokens with inner capitals (RichText, SemiBold).
    if !t.contains(' ') && t.chars().skip(1).any(|c| c.is_uppercase()) {
        return false;
    }
    true
}

/// Every literal in `src` that looks like UI text, with its line.
pub fn prose_literals(file: &str, src: &str) -> Vec<Found> {
    let c = strip_comments(src);
    let end = src.find("#[cfg(test)]").map(|b| src[..b].chars().count()).unwrap_or(c.len());
    let mut out = Vec::new();
    let mut i = 0;
    let mut line = 1;
    while i < end {
        let ch = c[i];
        if ch == '\n' {
            line += 1;
            i += 1;
            continue;
        }
        if ch == '\'' && i + 2 < c.len() && (c[i + 2] == '\'' || (c[i + 1] == '\\' && c.get(i + 3) == Some(&'\''))) {
            i += if c[i + 1] == '\\' { 4 } else { 3 };
            continue;
        }
        if ch != '"' {
            i += 1;
            continue;
        }
        let start_line = line;
        let line_start = (0..i).rev().find(|&k| c[k] == '\n').map_or(0, |k| k + 1);
        let before: String = c[line_start..i].iter().collect();
        let mut text = String::new();
        i += 1;
        while i < c.len() && c[i] != '"' {
            if c[i] == '\\' {
                i += 1;
                match c.get(i) {
                    Some('n') => text.push('\n'),
                    Some('u') => {
                        let mut hex = String::new();
                        i += 2;
                        while i < c.len() && c[i] != '}' {
                            hex.push(c[i]);
                            i += 1;
                        }
                        if let Some(ch) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                            text.push(ch);
                        }
                    }
                    Some('\n') => {
                        line += 1;
                        while i + 1 < c.len() && c[i + 1].is_whitespace() {
                            if c[i + 1] == '\n' {
                                line += 1;
                            }
                            i += 1;
                        }
                    }
                    Some(o) => text.push(*o),
                    None => {}
                }
            } else {
                if c[i] == '\n' {
                    line += 1;
                }
                text.push(c[i]);
            }
            i += 1;
        }
        i += 1;
        let skip_ctx = ["Id::new", "id_salt", "println!", "eprintln!", "panic!", "assert", "expect(", "unwrap_or", "env!", "include_str", "FontFamily::Name", "log::", "debug_assert", "write!("];
        if !skip_ctx.iter().any(|s| before.contains(s)) && looks_like_ui_text(&text) {
            out.push(Found { file: file.to_owned(), line: start_line, text, sink: String::new() });
        }
    }
    out
}

/// `f` relative to `app_src` with `/` between parts on every system, as the
/// baseline and the reports write it.
pub fn relative(app_src: &Path, f: &Path) -> String {
    f.strip_prefix(app_src).unwrap_or(f).components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/")
}

/// The app source files that draw the interface (not the AI prompts,
/// services or tools).
pub fn ui_files(app_src: &Path) -> Vec<PathBuf> {
    let mut v = Vec::new();
    for sub in ["kit", "screens", "editor"] {
        v.extend(rust_files(&app_src.join(sub)));
    }
    // The Account page is only part of a `commerce` build, and so are its messages.
    if !cfg!(feature = "commerce") {
        v.retain(|f| !f.ends_with("screens/account.rs"));
    }
    for f in ["app.rs", "view.rs", "catalogue.rs", "library.rs", "release.rs", "gallery.rs"] {
        let p = app_src.join(f);
        if p.exists() {
            v.push(p);
        }
    }
    v
}

/// Turn a Rust `format!` string into a message value: `Undid {}` becomes
/// `Undid { $v1 }`, `Added {name}` becomes `Added { $name }`, `{:.0}%`
/// becomes `{ $v1 }%`. `None` for text a message cannot express (escaped
/// braces, two placeholders with nothing between them).
pub fn to_template(text: &str) -> Option<String> {
    let mut out = String::new();
    let mut n = 0;
    let mut any = false;
    let mut last_was_var = false;
    let c: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < c.len() {
        match c[i] {
            '{' => {
                if c.get(i + 1) == Some(&'{') {
                    return None;
                }
                let mut j = i + 1;
                while j < c.len() && c[j] != '}' {
                    j += 1;
                }
                if j >= c.len() {
                    return None;
                }
                let inner: String = c[i + 1..j].iter().collect();
                let name = inner.split(':').next().unwrap_or("").trim();
                let var = if name.is_empty() || name.chars().all(|ch| ch.is_ascii_digit()) {
                    n += 1;
                    format!("v{n}")
                } else if name.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_') {
                    name.to_owned()
                } else {
                    return None;
                };
                if last_was_var {
                    return None;
                }
                out.push_str(&format!("{{ ${var} }}"));
                any = true;
                last_was_var = true;
                i = j + 1;
                continue;
            }
            '}' => return None,
            ch => {
                out.push(ch);
                last_was_var = false;
            }
        }
        i += 1;
    }
    (any || !text.contains('{')).then_some(out)
}

#[cfg(test)]
mod template_tests {
    use super::to_template;

    #[test]
    fn placeholders_become_variables() {
        assert_eq!(to_template("Undid {}").unwrap(), "Undid { $v1 }");
        assert_eq!(to_template("Added {name}").unwrap(), "Added { $name }");
        assert_eq!(to_template("{:.0}% of {}").unwrap(), "{ $v1 }% of { $v2 }");
        assert_eq!(to_template("Plain text").unwrap(), "Plain text");
    }

    #[test]
    fn what_cannot_be_expressed_is_refused() {
        assert!(to_template("a {{ b }}").is_none());
        assert!(to_template("{}{}").is_none());
        assert!(to_template("oops }").is_none());
    }
}
