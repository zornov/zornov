// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Localisation for Studio.
//!
//! Messages live in `locales/<tag>.ftl` (a Fluent-style format, see
//! [`ftl`]). English is the source; every other language must have every
//! message (a test enforces it, with `locales/pending.txt` listing the ones
//! a translator has not reached yet, which fall back to English).
//!
//! ```
//! use studio_i18n::{tr, tr_in, Lang};
//! assert_eq!(tr_in(Lang::En, "app-name", &[]), "Studio");
//! // The UI language is a process-wide setting:
//! studio_i18n::set_lang(Lang::En);
//! assert_eq!(tr("app-name"), "Studio");
//! ```
//!
//! Two ways to ask for text:
//! * [`tr`] / [`tr_args`] by message id, for strings with variables or
//!   plurals, and for all new code;
//! * [`t`] by English source text, which looks up the message whose English
//!   value is exactly that text. The shared widgets use it so every label
//!   passed to them is translated without touching the call sites.

pub mod ftl;
pub mod lang;
pub mod plural;
pub mod rtl;
pub mod scan;

pub use ftl::Value;
pub use lang::Lang;

use ftl::{Env, Pattern};
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::OnceLock;

/// One locale file; with the `commerce` feature the Settings > Account
/// messages (`locales/commerce/<tag>.ftl`) follow it.
#[cfg(feature = "commerce")]
macro_rules! locale {
    ($tag:literal) => {
        concat!(include_str!(concat!("../locales/", $tag, ".ftl")), "\n", include_str!(concat!("../locales/commerce/", $tag, ".ftl")))
    };
}
#[cfg(not(feature = "commerce"))]
macro_rules! locale {
    ($tag:literal) => {
        include_str!(concat!("../locales/", $tag, ".ftl"))
    };
}

/// The locale files, compiled in.
pub const SOURCES: [(Lang, &str); 9] = [
    (Lang::En, locale!("en")),
    (Lang::Hi, locale!("hi")),
    (Lang::Es, locale!("es")),
    (Lang::Fr, locale!("fr")),
    (Lang::De, locale!("de")),
    (Lang::Ja, locale!("ja")),
    (Lang::ZhHans, locale!("zh-Hans")),
    (Lang::Ar, locale!("ar")),
    (Lang::Fa, locale!("fa")),
];

/// Messages not translated into every language yet (one id per line).
#[cfg(feature = "commerce")]
pub const PENDING: &str = concat!(include_str!("../locales/pending.txt"), "\n", include_str!("../locales/commerce/pending.txt"));
#[cfg(not(feature = "commerce"))]
pub const PENDING: &str = include_str!("../locales/pending.txt");

/// One language's messages.
pub struct Catalogue {
    pub lang: Lang,
    messages: HashMap<&'static str, Pattern>,
}

impl Catalogue {
    fn build(lang: Lang, src: &'static str) -> Result<Catalogue, ftl::ParseError> {
        let mut messages = HashMap::new();
        for (id, pat) in ftl::parse(src)? {
            messages.insert(&*Box::leak(id.into_boxed_str()), pat);
        }
        Ok(Catalogue { lang, messages })
    }

    pub fn ids(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.messages.keys().copied()
    }

    pub fn contains(&self, id: &str) -> bool {
        self.messages.contains_key(id)
    }

    pub fn pattern(&self, id: &str) -> Option<&Pattern> {
        self.messages.get(id)
    }

    fn format(&self, id: &str, args: &[(&str, Value)]) -> Option<String> {
        let p = self.messages.get(id)?;
        let mut out = String::new();
        ftl::format(p, &Env { lang: self.lang, args, messages: &self.messages }, 0, &mut out);
        Some(out)
    }
}

static CATALOGUES: OnceLock<Vec<Catalogue>> = OnceLock::new();
static BY_SOURCE: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();

/// All catalogues, parsed on first use. A locale file that does not parse is
/// a build error caught by the tests, never a runtime panic: the language
/// then simply has no messages and English is shown.
pub fn catalogues() -> &'static [Catalogue] {
    CATALOGUES.get_or_init(|| {
        SOURCES
            .iter()
            .map(|(l, s)| Catalogue::build(*l, s).unwrap_or(Catalogue { lang: *l, messages: HashMap::new() }))
            .collect()
    })
}

pub fn catalogue(lang: Lang) -> &'static Catalogue {
    catalogues().iter().find(|c| c.lang == lang).expect("every language has a catalogue")
}

/// Parse every locale file, reporting the first error (for tests and tools).
pub fn check_sources() -> Result<(), String> {
    for (l, s) in SOURCES {
        ftl::parse(s).map_err(|e| format!("{}.ftl: {e}", l.tag()))?;
    }
    Ok(())
}

/// Is `id` listed as not yet translated?
pub fn is_pending(id: &str) -> bool {
    PENDING.lines().map(str::trim).any(|l| l == id)
}

static LANG: AtomicU8 = AtomicU8::new(0);

pub fn lang() -> Lang {
    Lang::ALL[LANG.load(Ordering::Relaxed) as usize % Lang::ALL.len()]
}

pub fn set_lang(l: Lang) {
    let i = Lang::ALL.iter().position(|x| *x == l).unwrap_or(0);
    LANG.store(i as u8, Ordering::Relaxed);
}

pub fn is_rtl() -> bool {
    lang().is_rtl()
}

/// Format message `id` in `lang`; English when `lang` lacks it; the id
/// itself when nothing has it (visible in the UI, caught by tests).
pub fn tr_in(lang: Lang, id: &str, args: &[(&str, Value)]) -> String {
    catalogue(lang).format(id, args).or_else(|| catalogue(Lang::En).format(id, args)).unwrap_or_else(|| id.to_owned())
}

/// Message `id` in the UI language.
pub fn tr(id: &str) -> Cow<'static, str> {
    tr_args(id, &[])
}

/// Message `id` with variables, in the UI language.
pub fn tr_args(id: &str, args: &[(&str, Value)]) -> Cow<'static, str> {
    let l = lang();
    // Plain messages are returned without allocating.
    if args.is_empty() {
        if let Some(t) = catalogue(l).pattern(id).and_then(|p| p.plain()) {
            return Cow::Borrowed(t);
        }
    }
    Cow::Owned(tr_in(l, id, args))
}

/// The message whose English text is exactly `src`, if there is one.
pub fn id_for_source(src: &str) -> Option<&'static str> {
    BY_SOURCE
        .get_or_init(|| {
            let en = catalogue(Lang::En);
            let mut m = HashMap::new();
            // Deterministic: the lexicographically first id wins for
            // duplicate English texts.
            let mut ids: Vec<_> = en.ids().collect();
            ids.sort_unstable();
            for id in ids {
                if let Some(t) = en.pattern(id).and_then(|p| p.plain()) {
                    m.entry(t).or_insert(id);
                }
            }
            m
        })
        .get(src)
        .copied()
}

/// Translate English UI text. Text that is not a known message (project
/// names, file names, text the AI wrote) comes back unchanged.
pub fn t(src: &str) -> Cow<'_, str> {
    let l = lang();
    if l == Lang::En {
        return Cow::Borrowed(src);
    }
    match id_for_source(src) {
        Some(id) => match catalogue(l).pattern(id).and_then(|p| p.plain()) {
            Some(s) => Cow::Borrowed(s),
            None => Cow::Borrowed(src),
        },
        None => Cow::Borrowed(src),
    }
}

/// Translate and prepare for drawing: shaped and reordered when the text
/// is right-to-left. Use this on every string handed to the painter.
pub fn shown(src: &str) -> Cow<'_, str> {
    match t(src) {
        Cow::Borrowed(s) => rtl::display(s, is_rtl()),
        Cow::Owned(s) => Cow::Owned(rtl::display(&s, is_rtl()).into_owned()),
    }
}

/// Prepare text that is already in the right language (a [`tr_args`]
/// result, a project name) for drawing.
pub fn visual(s: &str) -> Cow<'_, str> {
    rtl::display(s, is_rtl())
}

// --- text built at run time ---------------------------------------------------

struct Tpl {
    id: &'static str,
    lits: Vec<String>,
    vars: Vec<String>,
    weight: usize,
}

static TEMPLATES: OnceLock<Vec<Tpl>> = OnceLock::new();

/// English messages made of text and plain variables, longest literal text
/// first: these recognise `format!`-built strings such as `Undid split`.
fn templates() -> &'static [Tpl] {
    TEMPLATES.get_or_init(|| {
        let en = catalogue(Lang::En);
        let mut v: Vec<Tpl> = en
            .ids()
            .filter_map(|id| {
                let (lits, vars) = ftl::template(en.pattern(id)?)?;
                // Two variables with no text between them cannot be told apart.
                if lits[1..lits.len() - 1].iter().any(|l| l.is_empty()) {
                    return None;
                }
                let weight = lits.iter().map(|l| l.chars().count()).sum();
                Some(Tpl { id, lits, vars, weight })
            })
            .collect();
        v.sort_by(|a, b| b.weight.cmp(&a.weight).then(a.id.cmp(b.id)));
        v
    })
}

/// The pieces of `src` that fill the template's variables.
fn capture<'a>(src: &'a str, t: &Tpl) -> Option<Vec<&'a str>> {
    let first = &t.lits[0];
    let last = &t.lits[t.lits.len() - 1];
    if src.len() < first.len() + last.len() + t.vars.len() || !src.starts_with(first.as_str()) || !src.ends_with(last.as_str()) {
        return None;
    }
    let mut rest = &src[first.len()..src.len() - last.len()];
    let mut out = Vec::with_capacity(t.vars.len());
    for (i, _) in t.vars.iter().enumerate() {
        let next = &t.lits[i + 1];
        if i + 1 == t.vars.len() {
            // The last variable takes what remains.
            if rest.is_empty() {
                return None;
            }
            out.push(rest);
            rest = "";
        } else {
            let at = rest.find(next.as_str())?;
            if at == 0 {
                return None;
            }
            out.push(&rest[..at]);
            rest = &rest[at + next.len()..];
        }
    }
    rest.is_empty().then_some(out)
}

/// A value inside a built string: translated when it is a known word
/// (`split` after `Undid`), keeping its leading capital or not.
fn translate_value(v: &str, lang: Lang) -> String {
    let direct = t_in(lang, v);
    if direct != v {
        return direct;
    }
    let mut cs = v.chars();
    if let Some(f) = cs.next().filter(|c| c.is_lowercase()) {
        let up: String = f.to_uppercase().chain(cs).collect();
        let tr = t_in(lang, &up);
        if tr != up {
            let mut tc = tr.chars();
            return tc.next().map(|c| c.to_lowercase().chain(tc).collect()).unwrap_or(tr);
        }
    }
    v.to_owned()
}

fn t_in(lang: Lang, src: &str) -> String {
    if lang == Lang::En {
        return src.to_owned();
    }
    match id_for_source(src).and_then(|id| catalogue(lang).pattern(id)).and_then(|p| p.plain()) {
        Some(s) => s.to_owned(),
        None => src.to_owned(),
    }
}

/// Translate text built with `format!` (or plain English text). Matches
/// the English messages that have variables (`Undid { $v1 }`), translates
/// the pieces that are known words, and lays them out the way the UI
/// language wants. Anything else comes back unchanged.
pub fn t_fmt(src: &str) -> Cow<'_, str> {
    let l = lang();
    if l == Lang::En {
        return Cow::Borrowed(src);
    }
    if let Some(id) = id_for_source(src) {
        if let Some(s) = catalogue(l).pattern(id).and_then(|p| p.plain()) {
            return Cow::Borrowed(s);
        }
    }
    if src.len() > 240 {
        return Cow::Borrowed(src);
    }
    for t in templates() {
        if let Some(parts) = capture(src, t) {
            let vals: Vec<String> = parts.iter().map(|p| translate_value(p, l)).collect();
            let args: Vec<(&str, Value)> = t.vars.iter().map(String::as_str).zip(vals.into_iter().map(Value::Str)).collect();
            if catalogue(l).contains(t.id) {
                return Cow::Owned(tr_in(l, t.id, &args));
            }
            return Cow::Borrowed(src);
        }
    }
    Cow::Borrowed(src)
}

/// Does the catalogue have this English text, either as a message or as a
/// message with variables that the text fits?
pub fn covered(src: &str) -> bool {
    id_for_source(src).is_some() || templates().iter().any(|t| capture(src, t).is_some())
}
