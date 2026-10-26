// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The message catalogues are consistent: every file parses, every language
//! has every message with the same variables, plurals use categories the
//! language has, and nothing is defined twice.

use std::collections::{BTreeSet, HashSet};
use studio_i18n::{catalogue, ftl, Lang, SOURCES};

fn parse(l: Lang) -> Vec<(String, ftl::Pattern)> {
    let src = SOURCES.iter().find(|(x, _)| *x == l).unwrap().1;
    ftl::parse(src).unwrap_or_else(|e| panic!("{}.ftl: {e}", l.tag()))
}

#[test]
fn every_locale_file_parses() {
    studio_i18n::check_sources().unwrap();
}

#[test]
fn no_message_is_defined_twice() {
    for l in Lang::ALL {
        let mut seen = HashSet::new();
        for (id, _) in parse(l) {
            assert!(seen.insert(id.clone()), "{}.ftl defines `{id}` twice", l.tag());
        }
    }
}

#[test]
fn message_references_resolve() {
    for l in Lang::ALL {
        let c = catalogue(l);
        for (id, p) in parse(l) {
            for r in ftl::references(&p) {
                assert!(c.contains(&r), "{}.ftl: `{id}` refers to missing message `{r}`", l.tag());
            }
        }
    }
}

#[test]
fn nothing_is_translated_that_english_does_not_have() {
    let en: BTreeSet<_> = parse(Lang::En).into_iter().map(|(id, _)| id).collect();
    for l in Lang::ALL.into_iter().filter(|l| *l != Lang::En) {
        for (id, _) in parse(l) {
            assert!(en.contains(&id), "{}.ftl has `{id}`, which en.ftl does not", l.tag());
        }
    }
}

#[test]
fn every_language_has_every_message() {
    let en: Vec<_> = parse(Lang::En).into_iter().map(|(id, _)| id).collect();
    for l in Lang::ALL.into_iter().filter(|l| *l != Lang::En) {
        let have: BTreeSet<_> = parse(l).into_iter().map(|(id, _)| id).collect();
        let missing: Vec<_> = en.iter().filter(|id| !have.contains(*id) && !studio_i18n::is_pending(id)).collect();
        assert!(
            missing.is_empty(),
            "{} lacks {} message(s), for example {:?}.\nTranslate them (cargo run -p studio-i18n --bin i18n-scan -- --todo {}), or list their ids in crates/project/i18n/locales/pending.txt until a translator gets to them.",
            l.tag(),
            missing.len(),
            &missing[..missing.len().min(5)],
            l.tag()
        );
    }
}

#[test]
fn pending_lists_only_real_messages() {
    let en: BTreeSet<_> = parse(Lang::En).into_iter().map(|(id, _)| id).collect();
    for line in studio_i18n::PENDING.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        assert!(en.contains(line), "pending.txt lists `{line}`, which is not a message");
    }
}

#[test]
fn translations_use_the_same_variables_as_english() {
    let en = parse(Lang::En);
    for l in Lang::ALL.into_iter().filter(|l| *l != Lang::En) {
        let c = catalogue(l);
        for (id, p) in &en {
            if let Some(t) = c.pattern(id) {
                assert_eq!(ftl::variables(p), ftl::variables(t), "{}: `{id}` must use the same {{ $variables }} as English", l.tag());
            }
        }
    }
}

#[test]
fn selectors_use_categories_the_language_has() {
    // Walk the source: every `[key]` after `->` must be a number or a category.
    let cats = ["zero", "one", "two", "few", "many", "other", "yes", "no"];
    for (l, src) in SOURCES {
        for (n, line) in src.lines().enumerate() {
            let t = line.trim_start().trim_start_matches('*');
            if let Some(rest) = t.strip_prefix('[') {
                if let Some(key) = rest.split(']').next() {
                    let ok = key.parse::<i64>().is_ok() || cats.contains(&key);
                    assert!(ok, "{}.ftl line {}: unknown selector key `[{key}]`", l.tag(), n + 1);
                    // Categories a language never produces are dead text.
                    if l == Lang::Ja || l == Lang::ZhHans {
                        assert!(!["one", "two", "few", "many", "zero"].contains(&key), "{}.ftl line {}: {} has no plural `[{key}]`", l.tag(), n + 1, l.english_name());
                    }
                }
            }
        }
    }
}

#[test]
fn plural_messages_cover_the_languages_categories() {
    // Arabic needs the categories its numbers fall in.
    for (id, p) in parse(Lang::En) {
        if ftl::variables(&p).is_empty() {
            continue;
        }
        let Some(ar) = catalogue(Lang::Ar).pattern(&id) else { continue };
        let raw = ftl::raw(ar);
        // Only plural selectors (the English one has [one]), not yes/no ones.
        if ftl::raw(&p).contains("[one]") && raw.contains("->") {
            for cat in ["one", "other"] {
                assert!(raw.contains(&format!("[{cat}]")), "ar: `{id}` needs a [{cat}] variant: {raw}");
            }
        }
    }
}

#[test]
fn arabic_and_persian_have_no_stray_latin_only_messages() {
    // A right-to-left language whose message is still English is a hole,
    // except for names, numbers and symbols.
    for l in [Lang::Ar, Lang::Fa] {
        let c = catalogue(l);
        let en = catalogue(Lang::En);
        let mut same = 0;
        let mut total = 0;
        for id in en.ids() {
            let (Some(a), Some(b)) = (en.pattern(id).and_then(|p| p.plain()), c.pattern(id).and_then(|p| p.plain())) else { continue };
            total += 1;
            if a == b && a.chars().any(|ch| ch.is_alphabetic()) && a.len() > 14 {
                same += 1;
            }
        }
        assert!(same * 20 <= total.max(1), "{}: {same} of {total} messages are still English", l.tag());
    }
}

/// Cover the sources with no placeholder text left in a translation.
#[test]
fn no_translation_is_a_placeholder() {
    for l in Lang::ALL.into_iter().filter(|l| *l != Lang::En) {
        for (id, p) in parse(l) {
            let raw = ftl::raw(&p);
            for bad in ["TODO", "FIXME", "XXX", "lorem"] {
                assert!(!raw.contains(bad), "{}: `{id}` contains {bad}", l.tag());
            }
        }
    }
}

/// Settings > Account and its messages belong to the `commerce` feature:
/// the default catalogue holds none of them.
#[cfg(not(feature = "commerce"))]
#[test]
fn the_default_catalogue_has_no_account_or_commerce_text() {
    let en = catalogue(Lang::En);
    for id in ["create-account", "for-a-new-account", "upgrade", "checkout", "check-payment", "balance", "this-month", "monthly-budget", "show-packs", "signed-in-as", "sign-out", "sign-in", "licence-file", "server", "used"] {
        assert!(!en.contains(id), "`{id}` is account text; it belongs in locales/commerce/");
    }
    for l in Lang::ALL {
        for (id, p) in parse(l) {
            let _ = p;
            for w in ["marketplace", "stripe", "credits a month", "hosted ai", "studio cloud", "monthly budget", "check payment"] {
                assert!(!id.contains(&w.replace(' ', "-")), "{}.ftl: `{id}` is commerce text", l.tag());
            }
        }
    }
    for text in ["Marketplace", "Stripe", "Upgrade", "Check payment", "Create account", "Monthly budget", "Show packs"] {
        assert!(studio_i18n::id_for_source(text).is_none(), "\"{text}\" is in the default catalogue");
    }
}
