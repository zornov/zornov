// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Settings ▸ About ▸ Licences, as structured data.
//!
//! The content credits come from the repository's `ATTRIBUTIONS.md`
//! (written by `tools/pack-content` alongside the bundle), parsed into
//! packs, licences, sources and credit lines; the full licence texts are
//! kept for the detail view. The app's own bundled fonts and icons are
//! listed first.

use std::sync::OnceLock;

/// The whole Licences screen.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Licences {
    /// Fonts and icons shipped inside the app itself.
    pub app: Vec<Bundled>,
    pub packs: Vec<Pack>,
    pub licences: Vec<Licence>,
    /// Full texts, by title ("SIL Open Font License, Version 1.1").
    pub texts: Vec<(String, String)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Bundled {
    pub name: &'static str,
    pub licence: &'static str,
    pub text: &'static str,
}

/// A content pack ("Studio core library", "More photos", …).
#[derive(Clone, Debug, PartialEq)]
pub struct Pack {
    pub name: String,
    pub id: String,
    /// "10507 items, 291 MB — core bundle, ships with the app. …"
    pub summary: String,
}

/// One licence and everything under it.
#[derive(Clone, Debug, PartialEq)]
pub struct Licence {
    pub name: String,
    /// SPDX id, e.g. "CC-BY-4.0".
    pub spdx: String,
    pub url: String,
    /// What the licence asks for, in plain words.
    pub summary: String,
    /// ShareAlike licences are kept apart in the UI.
    pub share_alike: bool,
    pub sources: Vec<Source>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Source {
    pub name: String,
    pub url: String,
    pub version: Option<String>,
    /// "380 items: music."
    pub items: String,
    /// Credit lines, as they must be shown.
    pub credits: Vec<String>,
}

const APP_ASSETS: &[Bundled] = &[
    Bundled { name: "Manrope (interface type)", licence: "SIL Open Font License 1.1", text: include_str!("../../assets/fonts/Manrope-LICENSE.txt") },
    Bundled { name: "JetBrains Mono (timecode and numbers)", licence: "SIL Open Font License 1.1", text: include_str!("../../assets/fonts/JetBrainsMono-LICENSE.txt") },
    Bundled { name: "Phosphor Icons", licence: "MIT License", text: include_str!("../../assets/icons/Phosphor-LICENSE.txt") },
];

/// The attribution file shipped next to the content.
const ATTRIBUTIONS: &str = include_str!("../../../ATTRIBUTIONS.md");

/// Parsed once.
pub fn all() -> &'static Licences {
    static L: OnceLock<Licences> = OnceLock::new();
    L.get_or_init(|| {
        let mut l = parse(ATTRIBUTIONS);
        l.app = APP_ASSETS.to_vec();
        l
    })
}

/// Credits matching `query` (case-insensitive), as (licence, source, line).
pub fn search(query: &str) -> Vec<(&'static str, &'static str, &'static str)> {
    let q = query.to_lowercase();
    let mut out = Vec::new();
    for l in &all().licences {
        for s in &l.sources {
            for c in &s.credits {
                if q.is_empty() || c.to_lowercase().contains(&q) || s.name.to_lowercase().contains(&q) {
                    out.push((l.name.as_str(), s.name.as_str(), c.as_str()));
                }
            }
        }
    }
    out
}

/// Parse `ATTRIBUTIONS.md`.
pub fn parse(md: &str) -> Licences {
    let mut out = Licences::default();
    #[derive(PartialEq)]
    enum Sec {
        None,
        Packs,
        Licence,
        Texts,
    }
    let mut sec = Sec::None;
    let mut lines = md.lines().peekable();
    while let Some(line) = lines.next() {
        if let Some(h) = line.strip_prefix("## ") {
            sec = match h.trim() {
                "Packs" => Sec::Packs,
                "Licence texts" => Sec::Texts,
                _ => {
                    out.licences.push(licence_heading(h));
                    Sec::Licence
                }
            };
            continue;
        }
        match sec {
            Sec::Packs => {
                // - **Name** (`id`): summary
                if let Some(rest) = line.strip_prefix("- **") {
                    let (name, rest) = rest.split_once("**").unwrap_or((rest, ""));
                    let id = rest.split('`').nth(1).unwrap_or_default();
                    let summary = rest.split_once("):").map_or("", |(_, s)| s).trim();
                    out.packs.push(Pack { name: name.into(), id: id.into(), summary: summary.into() });
                }
            }
            Sec::Licence => {
                let Some(l) = out.licences.last_mut() else { continue };
                if let Some(h) = line.strip_prefix("### ") {
                    l.sources.push(source_heading(h));
                } else if let Some(c) = line.strip_prefix("- ") {
                    if let Some(s) = l.sources.last_mut() {
                        s.credits.push(c.trim().into());
                    }
                } else if line.starts_with('<') && l.url.is_empty() {
                    l.url = line.trim().trim_start_matches('<').trim_end_matches('>').into();
                } else if !line.trim().is_empty() {
                    match l.sources.last_mut() {
                        Some(s) if s.items.is_empty() => s.items = line.trim().into(),
                        None if l.summary.is_empty() => l.summary = line.trim().into(),
                        _ => {}
                    }
                }
            }
            Sec::Texts => {
                if let Some(h) = line.strip_prefix("### ") {
                    // The text is the next fenced block.
                    let mut text = String::new();
                    let mut inside = false;
                    while let Some(&next) = lines.peek() {
                        if next.starts_with("### ") || next.starts_with("## ") {
                            break;
                        }
                        lines.next();
                        if next.starts_with("```") {
                            if inside {
                                break;
                            }
                            inside = true;
                            continue;
                        }
                        if inside {
                            text.push_str(next);
                            text.push('\n');
                        }
                    }
                    out.texts.push((h.trim().into(), text));
                }
            }
            Sec::None => {}
        }
    }
    out
}

/// "Creative Commons Attribution 4.0 International (CC-BY-4.0) (ShareAlike)"
fn licence_heading(h: &str) -> Licence {
    let h = h.trim();
    let share_alike = h.ends_with("(ShareAlike)");
    let h = h.trim_end_matches("(ShareAlike)").trim();
    let (name, spdx) = match h.rfind(" (") {
        Some(i) if h.ends_with(')') => (&h[..i], &h[i + 2..h.len() - 1]),
        _ => (h, ""),
    };
    Licence { name: name.into(), spdx: spdx.into(), url: String::new(), summary: String::new(), share_alike, sources: Vec::new() }
}

/// "Kenney — <https://kenney.nl> (version v17)"
fn source_heading(h: &str) -> Source {
    let (name, rest) = h.split_once(" — ").unwrap_or((h, ""));
    let url = rest.split('<').nth(1).and_then(|r| r.split('>').next()).unwrap_or_default();
    let version = rest.split_once("(version ").map(|(_, v)| v.trim_end_matches(')').to_owned());
    Source { name: name.trim().into(), url: url.into(), version, items: String::new(), credits: Vec::new() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_shipped_attributions() {
        let l = all();
        assert_eq!(l.app.len(), 3);
        assert!(l.app.iter().all(|a| !a.text.is_empty()));
        assert!(l.packs.iter().any(|p| p.id == "core" && p.summary.contains("items")));
        let cc_by = l.licences.iter().find(|x| x.spdx == "CC-BY-4.0").expect("CC BY section");
        assert!(cc_by.url.starts_with("https://creativecommons.org/"));
        assert!(!cc_by.summary.is_empty());
        let kevin = cc_by.sources.iter().find(|s| s.name.starts_with("Kevin MacLeod")).expect("Kevin MacLeod");
        assert_eq!(kevin.url, "https://incompetech.com");
        assert!(kevin.items.contains("music"));
        assert!(kevin.credits.len() > 100);
        assert!(l.licences.iter().any(|x| x.share_alike && x.spdx == "CC-BY-SA-4.0"));
        assert!(l.texts.iter().any(|(t, body)| t.contains("Open Font License") && body.contains("PERMISSION")));
        assert!(!search("aces high").is_empty());
    }

    #[test]
    fn parses_a_small_file() {
        let md = "# A\n\n## Packs\n\n- **Core** (`core`): 3 items, ships.\n\n## MIT License (MIT)\n\n<https://x.org/mit>\n\nPlain words.\n\n### Icons — <https://icons.example> (version 1.2)\n\n4 items: shape.\n\n- Icons, Copyright Me\n\n## Licence texts\n\n### Icons — MIT License\n\n```text\nMIT body\n```\n";
        let l = parse(md);
        assert_eq!(l.packs, vec![Pack { name: "Core".into(), id: "core".into(), summary: "3 items, ships.".into() }]);
        assert_eq!(l.licences.len(), 1);
        let x = &l.licences[0];
        assert_eq!((x.name.as_str(), x.spdx.as_str(), x.url.as_str(), x.summary.as_str()), ("MIT License", "MIT", "https://x.org/mit", "Plain words."));
        assert_eq!(x.sources[0], Source { name: "Icons".into(), url: "https://icons.example".into(), version: Some("1.2".into()), items: "4 items: shape.".into(), credits: vec!["Icons, Copyright Me".into()] });
        assert_eq!(l.texts, vec![("Icons — MIT License".into(), "MIT body\n".into())]);
    }
}
