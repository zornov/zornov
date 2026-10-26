// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Packs and their licence documents.
//!
//! A pack is a folder with an `index.json`, the item files and a
//! `LICENSES.md` carrying every attribution the pack needs, grouped by
//! licence with the exact wording each licence asks for. `ATTRIBUTIONS.md`
//! at the repository root is the same document across every pack.

use crate::item::{Item, Kind, Licence, Source};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// What a pack is, as written into its `index.json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackSpec {
    /// Folder name (`core`, `music-incompetech`, ...).
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// Ships inside the app rather than as a download.
    #[serde(default)]
    pub core: bool,
    /// Every item is CC BY-SA: the pack is a ShareAlike set.
    #[serde(default)]
    pub share_alike: bool,
}

/// A pack's contents. Item paths are relative to the pack folder.
#[derive(Clone, Debug)]
pub struct Pack {
    pub spec: PackSpec,
    pub items: Vec<Item>,
    /// Total bytes of the item files.
    pub bytes: u64,
}

impl Pack {
    pub fn new(spec: PackSpec) -> Pack {
        Pack { spec, items: Vec::new(), bytes: 0 }
    }

    pub fn licences(&self) -> BTreeSet<Licence> {
        self.items.iter().map(|i| i.licence).collect()
    }
}

/// A verbatim licence text shipped with the packs (MIT, OFL, Apache, the CC
/// deeds), read from the upstream source folders by the pack tool.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LicenceText {
    pub licence: Licence,
    /// Upstream it belongs to (`fluent-emoji`, `tabler`, `google-fonts`, ...), or empty
    /// when the text is the generic licence.
    pub source_id: String,
    pub title: String,
    pub text: String,
}

struct Credits {
    source: Source,
    lines: BTreeSet<String>,
    count: usize,
    modified: bool,
    kinds: BTreeSet<Kind>,
}

fn group(items: &[Item]) -> BTreeMap<Licence, BTreeMap<String, Credits>> {
    let mut out: BTreeMap<Licence, BTreeMap<String, Credits>> = BTreeMap::new();
    for it in items {
        let c = out.entry(it.licence).or_default().entry(it.source.id.clone()).or_insert_with(|| Credits {
            source: it.source.clone(),
            lines: BTreeSet::new(),
            count: 0,
            modified: false,
            kinds: BTreeSet::new(),
        });
        c.count += 1;
        c.modified |= it.details.modified;
        c.kinds.insert(it.kind);
        let line = if it.kind == Kind::Font && !it.attribution.is_empty() {
            format!("{}: {}", it.name, it.attribution)
        } else {
            it.attribution.clone()
        };
        if !line.trim().is_empty() {
            c.lines.insert(line);
        }
    }
    out
}

/// What each licence requires, in the words we show.
fn statement(licence: Licence) -> &'static str {
    match licence {
        Licence::Studio => "These items were made for Studio: generated with Midjourney under the owner's paid plan, then selected, named, cut out, keyed and resized by the Studio team. They are distributed with Studio under the Apache License 2.0 like the rest of the project.",
        Licence::CcBy4 => "The works below are licensed under the Creative Commons Attribution 4.0 International licence. Credit must be given to the creator as written below wherever the work is used; the licence text is at the URL above. Where an item is marked as modified, it was resized, cut out, keyed or re-encoded for Studio and no other change was made.",
        Licence::CcBySa4 => "SHAREALIKE SET. The works below are licensed under the Creative Commons Attribution-ShareAlike 4.0 International licence. Credit must be given as written below, and anything you make by adapting these items must be shared under the same licence. Studio keeps these items in their own pack so that the ShareAlike condition is clear.",
        Licence::Cc0 => "The works below were dedicated to the public domain by their authors under CC0 1.0 Universal. No attribution is required; the authors are credited here with thanks.",
        Licence::Mit => "The works below are licensed under the MIT License. The copyright notice and permission notice are reproduced in full under Licence texts.",
        Licence::Ofl11 => "The font families below are licensed under the SIL Open Font License, Version 1.1. Each family's copyright notice is listed; the licence text is reproduced in full under Licence texts. The fonts are distributed unmodified.",
        Licence::Apache2 => "The font families below are licensed under the Apache License, Version 2.0. Each family's copyright notice is listed; the licence text is reproduced in full under Licence texts. The fonts are distributed unmodified.",
    }
}

fn section_order() -> [Licence; 7] {
    [Licence::Studio, Licence::Cc0, Licence::Mit, Licence::Ofl11, Licence::Apache2, Licence::CcBy4, Licence::CcBySa4]
}

fn write_credits(out: &mut String, items: &[Item], texts: &[LicenceText]) {
    let grouped = group(items);
    for licence in section_order() {
        let Some(sources) = grouped.get(&licence) else { continue };
        let mark = if licence.share_alike() { " (ShareAlike)" } else { "" };
        out.push_str(&format!("\n## {} ({}){mark}\n\n<{}>\n\n{}\n", licence.name(), licence.spdx(), licence.url(), statement(licence)));
        for (_, c) in sources {
            let kinds: Vec<&str> = c.kinds.iter().map(|k| k.label()).collect();
            out.push_str(&format!("\n### {}", c.source.name));
            if !c.source.url.is_empty() {
                out.push_str(&format!(" — <{}>", c.source.url));
            }
            if !c.source.version.is_empty() {
                out.push_str(&format!(" (version {})", c.source.version));
            }
            out.push('\n');
            out.push_str(&format!("\n{} item{}: {}.{}\n\n", c.count, if c.count == 1 { "" } else { "s" }, kinds.join(", ").to_lowercase(), if c.modified { " Modified for Studio (resized, cut out, keyed or re-encoded)." } else { "" }));
            for line in &c.lines {
                out.push_str(&format!("- {line}\n"));
            }
        }
    }
    let used: BTreeSet<Licence> = items.iter().map(|i| i.licence).collect();
    let used_sources: BTreeSet<String> = items.iter().map(|i| i.source.id.clone()).collect();
    let relevant: Vec<&LicenceText> = texts.iter().filter(|t| used.contains(&t.licence) && (t.source_id.is_empty() || used_sources.contains(&t.source_id))).collect();
    if !relevant.is_empty() {
        out.push_str("\n## Licence texts\n");
        for t in relevant {
            out.push_str(&format!("\n### {}\n\n```text\n{}\n```\n", t.title, t.text.trim_end()));
        }
    }
}

/// `LICENSES.md` for one pack.
pub fn licenses_md(pack: &Pack, texts: &[LicenceText]) -> String {
    let mut out = String::new();
    out.push_str(&format!("# {} — licences and attributions\n\n", pack.spec.name));
    if pack.spec.share_alike {
        out.push_str("**This pack is a ShareAlike set.** Every item in it is licensed CC BY-SA 4.0; anything made by adapting these items must be shared under the same licence.\n\n");
    }
    out.push_str(&format!("{} items, {} MB. {}\n", pack.items.len(), pack.bytes / (1024 * 1024), pack.spec.description));
    write_credits(&mut out, &pack.items, texts);
    out
}

/// `ATTRIBUTIONS.md` for the repository root: every pack together.
pub fn attributions_md(packs: &[Pack], texts: &[LicenceText]) -> String {
    let mut out = String::new();
    out.push_str("# Attributions\n\nStudio's library content is either made for Studio or openly licensed. This file lists every third-party source and licence across the core bundle and every optional pack; each pack also ships its own `LICENSES.md`. The in-app Licences screen shows the same information.\n\n## Packs\n\n");
    for p in packs {
        let kind = if p.spec.core { "core bundle, ships with the app" } else { "optional pack" };
        let sa = if p.spec.share_alike { ", ShareAlike set" } else { "" };
        out.push_str(&format!("- **{}** (`{}`): {} items, {} MB — {kind}{sa}. {}\n", p.spec.name, p.spec.id, p.items.len(), p.bytes / (1024 * 1024), p.spec.description));
    }
    let all: Vec<Item> = packs.iter().flat_map(|p| p.items.iter().cloned()).collect();
    write_credits(&mut out, &all, texts);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item::Details;

    fn item(name: &str, kind: Kind, licence: Licence, source: Source, attribution: &str, modified: bool) -> Item {
        Item {
            id: name.to_lowercase(),
            name: name.into(),
            kind,
            category: "X".into(),
            set: "X".into(),
            tags: vec![],
            file: format!("{name}.png").into(),
            preview: None,
            licence,
            attribution: attribution.into(),
            source,
            details: Details { modified, ..Details::default() },
        }
    }

    #[test]
    fn licences_document_groups_by_licence_and_source() {
        let tw = Source::new("twemoji", "Twemoji", "https://github.com/jdecked/twemoji", "v17");
        let mut pack = Pack::new(PackSpec { id: "core".into(), name: "Core".into(), description: "Ships with the app.".into(), core: true, share_alike: false });
        pack.items.push(item("A", Kind::Sticker, Licence::Studio, Source::studio(), "Made for Zornov", true));
        pack.items.push(item("B", Kind::Emoji, Licence::CcBy4, tw.clone(), "Twemoji graphics, CC BY 4.0", false));
        pack.items.push(item("C", Kind::Emoji, Licence::CcBy4, tw, "Twemoji graphics, CC BY 4.0", false));
        pack.items.push(item("Inter", Kind::Font, Licence::Ofl11, Source::new("google-fonts", "Google Fonts", "", ""), "Copyright 2020 The Inter Project Authors", false));
        let texts = vec![
            LicenceText { licence: Licence::Ofl11, source_id: String::new(), title: "SIL Open Font License 1.1".into(), text: "OFL TEXT".into() },
            LicenceText { licence: Licence::Mit, source_id: "tabler".into(), title: "Tabler MIT".into(), text: "MIT TEXT".into() },
        ];
        let md = licenses_md(&pack, &texts);
        assert!(md.contains("## Made for Zornov (Studio)"));
        assert!(md.contains("## Creative Commons Attribution 4.0 International (CC-BY-4.0)"));
        assert!(md.contains("### Twemoji — <https://github.com/jdecked/twemoji> (version v17)"));
        assert_eq!(md.matches("- Twemoji graphics, CC BY 4.0").count(), 1, "credit lines are deduplicated");
        assert!(md.contains("2 items: emoji."));
        assert!(md.contains("- Inter: Copyright 2020 The Inter Project Authors"));
        assert!(md.contains("OFL TEXT"));
        assert!(!md.contains("MIT TEXT"), "unused licence texts stay out");
        assert!(md.contains("Modified for Studio"));
        let all = attributions_md(&[pack], &texts);
        assert!(all.contains("- **Core** (`core`): 4 items"));
        assert!(all.contains("core bundle, ships with the app"));
    }

    #[test]
    fn share_alike_pack_is_marked() {
        let om = Source::new("openmoji", "OpenMoji", "https://openmoji.org", "17");
        let mut pack = Pack::new(PackSpec { id: "openmoji".into(), name: "OpenMoji".into(), description: String::new(), core: false, share_alike: true });
        pack.items.push(item("A", Kind::Emoji, Licence::CcBySa4, om, "OpenMoji, CC BY-SA 4.0", false));
        let md = licenses_md(&pack, &[]);
        assert!(md.starts_with("# OpenMoji — licences and attributions\n\n**This pack is a ShareAlike set.**"));
        assert!(md.contains("(CC-BY-SA-4.0) (ShareAlike)"));
        assert!(md.contains("SHAREALIKE SET"));
    }
}
