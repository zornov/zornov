// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Tools for the message catalogues.
//!
//! ```text
//! i18n-scan [APP_SRC]            list UI-looking literals with no message
//! i18n-scan --add [APP_SRC]      add interface literals that lack a message
//!                                to en.ftl and pending.txt (makes the
//!                                hard-coded-string test pass)
//! i18n-scan --new-ftl [APP_SRC]  the same as `id = text` lines on stdout
//! i18n-scan --todo LANG          numbered English messages LANG still lacks
//! i18n-scan --merge LANG FILE    add `N<TAB>translation` lines (N from --todo)
//!                                to LANG.ftl, checking each one
//! i18n-scan --fix LANG FILE      replace (or add) messages: `id<TAB>text` lines, checked
//! i18n-scan --show ID...         the English of those messages, as `id<TAB>text` lines
//! i18n-scan --pending            ids still listed in pending.txt that every
//!                                language already has (to delete from it)
//! ```

use studio_i18n::{catalogue, ftl, scan, Lang};

fn slug(text: &str) -> String {
    let mut s = String::new();
    for c in text.chars() {
        if c.is_ascii_alphanumeric() {
            s.push(c.to_ascii_lowercase());
        } else if !s.ends_with('-') && !s.is_empty() {
            s.push('-');
        }
    }
    let s = s.trim_end_matches('-');
    let mut out: String = s.chars().take(44).collect();
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() { "text".into() } else { out }
}

fn scan_missing(root: &std::path::Path) -> std::collections::BTreeMap<String, String> {
    let mut missing = std::collections::BTreeMap::<String, String>::new();
    for f in scan::ui_files(root) {
        let rel = scan::relative(root, &f);
        if rel == "gallery.rs" {
            continue;
        }
        let Ok(src) = std::fs::read_to_string(&f) else { continue };
        for hit in scan::prose_literals(&rel, &src) {
            if studio_i18n::id_for_source(&hit.text).is_none() {
                missing.entry(hit.text).or_insert(format!("{}:{}", hit.file, hit.line));
            }
        }
    }
    missing
}

/// Messages of the English catalogue in file order.
fn english_order() -> Vec<String> {
    let src = studio_i18n::SOURCES[0].1;
    ftl::parse(src).unwrap_or_default().into_iter().map(|(id, _)| id).collect()
}

fn locales_dir() -> String {
    format!("{}/locales", env!("CARGO_MANIFEST_DIR"))
}

/// Plural categories (and exact keys) a selector may use in `lang`.
fn allowed_keys(lang: Lang) -> &'static [&'static str] {
    match lang {
        Lang::Ja | Lang::ZhHans => &["other"],
        Lang::Ar => &["zero", "one", "two", "few", "many", "other"],
        Lang::Es | Lang::Fr => &["one", "many", "other"],
        _ => &["one", "other"],
    }
}

/// Check a translation of message `id` and return the FTL text for it.
fn checked(lang: Lang, id: &str, translation: &str) -> Result<String, String> {
    let expanded = ftl::expand_selectors(&translation.trim().replace("\\n", "\n    "));
    let text = format!("{id} = {expanded}\n");
    let parsed = ftl::parse(&text).map_err(|e| format!("does not parse: {e}"))?;
    let [(_, pat)] = parsed.as_slice() else { return Err("not exactly one message".into()) };
    let en = catalogue(Lang::En).pattern(id).ok_or("no such English message")?;
    let (want, got) = (ftl::variables(en), ftl::variables(pat));
    if want != got {
        return Err(format!("variables {got:?}, English has {want:?}"));
    }
    // Selector keys: compare with what the language has.
    for line in text.lines() {
        let t = line.trim_start().trim_start_matches('*');
        if let Some(rest) = t.strip_prefix('[') {
            if let Some(key) = rest.split(']').next() {
                let numeric = key.parse::<i64>().is_ok();
                let yes_no = key == "yes" || key == "no";
                if !numeric && !yes_no && !allowed_keys(lang).contains(&key) {
                    return Err(format!("[{key}] is not a plural category of {}", lang.english_name()));
                }
            }
        }
    }
    // The English text had `\n` paragraph breaks: the translation must keep them.
    let breaks = |s: &str| s.matches('\n').count();
    let en_raw = ftl::raw(en);
    if en_raw.matches("\\n").count() != translation.matches("\\n").count() && breaks(&en_raw) == 0 {
        return Err("paragraph breaks (\\n) differ from English".into());
    }
    if translation.trim().is_empty() {
        return Err("empty".into());
    }
    Ok(text)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let default_root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../app/src");
    match args.first().map(String::as_str) {
        Some("--new-ftl") => {
            let root = std::path::PathBuf::from(args.get(1).map(String::as_str).unwrap_or(default_root));
            let skip = ["Manrope", "JetBrains", "Phosphor", "MIT License", "SIL Open", "Apache"];
            let mut used: std::collections::HashSet<String> = catalogue(Lang::En).ids().map(str::to_owned).collect();
            for (text, _) in scan_missing(&root) {
                if skip.iter().any(|s| text.contains(s)) || text.contains("SELECT") || text.contains("INSERT") {
                    continue;
                }
                let Some(value) = scan::to_template(&text) else { continue };
                let base = slug(&text);
                let mut id = base.clone();
                let mut n = 2;
                while !used.insert(id.clone()) {
                    id = format!("{base}-{n}");
                    n += 1;
                }
                let value = value.replace('\n', "\n    ");
                println!("{id} = {value}");
            }
        }
        Some("--add") => {
            let root = std::path::PathBuf::from(args.get(1).map(String::as_str).unwrap_or(default_root));
            let mut used: std::collections::HashSet<String> = catalogue(Lang::En).ids().map(str::to_owned).collect();
            let (mut text, mut pending) = (String::new(), String::new());
            let mut seen = std::collections::BTreeSet::new();
            let mut count = 0;
            let baseline = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/hardcoded_baseline.txt")).unwrap_or_default();
            for f in scan::ui_files(&root) {
                let rel = scan::relative(&root, &f);
                if rel == "gallery.rs" {
                    continue;
                }
                let Ok(src) = std::fs::read_to_string(&f) else { continue };
                for hit in scan::scan_source(&rel, &src) {
                    // Literals the test already allows are not interface text.
                    let key = format!("{}\t{}", hit.file, hit.text.escape_debug());
                    if baseline.lines().any(|l| l == key) {
                        continue;
                    }
                    if !scan::is_prose(&hit.text) || studio_i18n::covered(&hit.text) || !seen.insert(hit.text.clone()) {
                        continue;
                    }
                    let Some(value) = scan::to_template(&hit.text) else { continue };
                    let base = slug(&hit.text);
                    let mut id = base.clone();
                    let mut n = 2;
                    while !used.insert(id.clone()) {
                        id = format!("{base}-{n}");
                        n += 1;
                    }
                    text.push_str(&format!("{id} = {}\n", value.replace('\n', "\n    ")));
                    pending.push_str(&format!("{id}\n"));
                    count += 1;
                }
            }
            use std::io::Write;
            let dir = locales_dir();
            let mut en = std::fs::OpenOptions::new().append(true).open(format!("{dir}/en.ftl")).expect("en.ftl");
            write!(en, "{text}").expect("write en.ftl");
            let mut pe = std::fs::OpenOptions::new().append(true).create(true).open(format!("{dir}/pending.txt")).expect("pending.txt");
            write!(pe, "{pending}").expect("write pending.txt");
            eprintln!("{count} message(s) added to en.ftl and pending.txt");
        }
        Some("--todo") => {
            let lang = args.get(1).and_then(|t| Lang::from_tag(t)).expect("a language tag");
            let have = catalogue(lang);
            for (i, id) in english_order().iter().enumerate() {
                if !have.contains(id) {
                    println!("{}\t{}", i + 1, ftl::raw(catalogue(Lang::En).pattern(id).unwrap()));
                }
            }
        }
        Some("--merge") => {
            let lang = args.get(1).and_then(|t| Lang::from_tag(t)).expect("a language tag");
            let file = args.get(2).expect("a file");
            let order = english_order();
            let text = std::fs::read_to_string(file).expect("readable file");
            let path = format!("{}/{}.ftl", locales_dir(), lang.tag());
            let mut ftl_out = std::fs::read_to_string(&path).unwrap_or_default();
            if !ftl_out.ends_with('\n') && !ftl_out.is_empty() {
                ftl_out.push('\n');
            }
            let have = catalogue(lang);
            let (mut added, mut bad) = (0, 0);
            let mut seen = std::collections::HashSet::new();
            for line in text.lines() {
                let Some((n, tr)) = line.split_once('\t') else { continue };
                let Ok(n) = n.trim().parse::<usize>() else { continue };
                let Some(id) = order.get(n - 1) else {
                    eprintln!("{n}: no such message");
                    bad += 1;
                    continue;
                };
                if have.contains(id) || !seen.insert(n) {
                    continue;
                }
                match checked(lang, id, tr) {
                    Ok(t) => {
                        ftl_out.push_str(&t);
                        added += 1;
                    }
                    Err(e) => {
                        eprintln!("{n} ({id}): {e}\n    {}", tr.chars().take(120).collect::<String>());
                        bad += 1;
                    }
                }
            }
            std::fs::write(&path, ftl_out).expect("writable");
            eprintln!("{added} message(s) added to {path}; {bad} rejected");
        }
        Some("--fix") => {
            // Replace existing messages: lines `id<TAB>text[<TAB>reason]`.
            let lang = args.get(1).and_then(|t| Lang::from_tag(t)).expect("a language tag");
            let text = std::fs::read_to_string(args.get(2).expect("a file")).expect("readable file");
            let path = format!("{}/{}.ftl", locales_dir(), lang.tag());
            let mut src = std::fs::read_to_string(&path).expect("locale file");
            let (mut done, mut bad) = (0, 0);
            for line in text.lines() {
                let mut cols = line.split('\t');
                let (Some(id), Some(tr)) = (cols.next(), cols.next()) else { continue };
                let id = id.trim();
                let new = match checked(lang, id, tr) {
                    Ok(t) => t,
                    Err(e) => {
                        eprintln!("{id}: {e}\n    {}", tr.chars().take(120).collect::<String>());
                        bad += 1;
                        continue;
                    }
                };
                let mut out = String::new();
                let mut skipping = false;
                let mut found = false;
                for l in src.lines() {
                    if l.starts_with(&format!("{id} = ")) {
                        out.push_str(&new);
                        skipping = true;
                        found = true;
                        continue;
                    }
                    if skipping && (l.starts_with(' ') || l.is_empty()) {
                        continue;
                    }
                    skipping = false;
                    out.push_str(l);
                    out.push('\n');
                }
                if found {
                    src = out;
                } else {
                    // A new message: add it at the end.
                    if !src.ends_with('\n') {
                        src.push('\n');
                    }
                    src.push_str(&new);
                }
                done += 1;
            }
            std::fs::write(&path, src).expect("writable");
            eprintln!("{done} message(s) replaced in {path}; {bad} rejected");
        }
        Some("--show") => {
            for id in &args[1..] {
                if let Some(p) = catalogue(Lang::En).pattern(id) {
                    println!("{id}\t{}", ftl::raw(p));
                }
            }
        }
        Some("--pending") => {
            let all: Vec<_> = Lang::ALL.into_iter().filter(|l| *l != Lang::En).collect();
            for id in studio_i18n::PENDING.lines().map(str::trim).filter(|l| !l.is_empty()) {
                if all.iter().all(|l| catalogue(*l).contains(id)) {
                    println!("{id}");
                }
            }
        }
        _ => {
            let root = std::path::PathBuf::from(args.first().map(String::as_str).unwrap_or(default_root));
            let missing = scan_missing(&root);
            for (text, at) in &missing {
                println!("{}\t{at}", text.escape_debug());
            }
            eprintln!("{} distinct strings without a message", missing.len());
        }
    }
}
