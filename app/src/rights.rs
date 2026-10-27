// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Rights and licences: what each asset may be used for, where it came
//! from, who to credit and until when, and the check an export makes
//! against what the timeline really uses.
//!
//! Studio's own bundled content brings its licence and credit line along
//! (`ATTRIBUTIONS.md` is written from the same data); everything else the
//! person (or the AI, from what they say) records with [`set`].

use crate::assets::{Asset, Store};
use serde_json::{Value, json};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;
use studio_model as m;

/// A licence Studio understands.
pub struct Kind {
    pub id: &'static str,
    pub name: &'static str,
    pub commercial: bool,
    pub attribution: bool,
    /// Editing (cutting, grading, mixing) the work is allowed.
    pub derivatives: bool,
    pub share_alike: bool,
}

const KINDS: &[Kind] = &[
    Kind { id: "owned", name: "Own work", commercial: true, attribution: false, derivatives: true, share_alike: false },
    Kind { id: "licensed", name: "Licensed (paid or by agreement)", commercial: true, attribution: false, derivatives: true, share_alike: false },
    Kind { id: "royalty-free", name: "Royalty free", commercial: true, attribution: false, derivatives: true, share_alike: false },
    Kind { id: "permission", name: "Used with permission", commercial: true, attribution: false, derivatives: true, share_alike: false },
    Kind { id: "public-domain", name: "Public domain", commercial: true, attribution: false, derivatives: true, share_alike: false },
    Kind { id: "open", name: "Open licence (MIT, Apache, OFL…)", commercial: true, attribution: false, derivatives: true, share_alike: false },
    Kind { id: "cc0", name: "CC0 1.0", commercial: true, attribution: false, derivatives: true, share_alike: false },
    Kind { id: "cc-by", name: "CC BY", commercial: true, attribution: true, derivatives: true, share_alike: false },
    Kind { id: "cc-by-sa", name: "CC BY-SA", commercial: true, attribution: true, derivatives: true, share_alike: true },
    Kind { id: "cc-by-nc", name: "CC BY-NC (non-commercial)", commercial: false, attribution: true, derivatives: true, share_alike: false },
    Kind { id: "cc-by-nd", name: "CC BY-ND (no derivatives)", commercial: true, attribution: true, derivatives: false, share_alike: false },
    Kind { id: "editorial", name: "Editorial use only", commercial: false, attribution: true, derivatives: true, share_alike: false },
    Kind { id: "rights-managed", name: "Rights managed", commercial: true, attribution: false, derivatives: true, share_alike: false },
    Kind { id: "generated", name: "Made by Zornov's generators", commercial: true, attribution: false, derivatives: true, share_alike: false },
    Kind { id: "unknown", name: "Not recorded", commercial: true, attribution: false, derivatives: true, share_alike: false },
];

/// A licence name as people write it ("CC BY 4.0", "cc-by-nc-sa", "stock")
/// to its id.
pub fn normalise(s: &str) -> String {
    let l = s.trim().to_lowercase().replace(['_', ' '], "-");
    if l.starts_with("cc0") || l.starts_with("cc-0") {
        return "cc0".into();
    }
    let l = l.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.' || c == '-').to_owned();
    match l.as_str() {
        "" | "unknown" | "none" | "not-recorded" => "unknown",
        "own" | "owned" | "own-work" | "mine" | "original" | "self" => "owned",
        "public-domain" | "pd" | "pdm" => "public-domain",
        "cc-by" | "ccby" => "cc-by",
        "mit" | "apache" | "apache-" | "ofl" | "bsd" | "isc" | "mit-or-apache" | "open" => "open",
        "unsplash" | "pexels" | "pixabay" => "royalty-free",
        "cc-by-sa" | "ccbysa" => "cc-by-sa",
        "cc-by-nc" | "cc-by-nc-sa" | "cc-by-nc-nd" | "ccbync" | "non-commercial" | "noncommercial" | "nc" => "cc-by-nc",
        "cc-by-nd" | "ccbynd" | "no-derivatives" => "cc-by-nd",
        "editorial" | "editorial-use" | "editorial-only" => "editorial",
        "licensed" | "license" | "licence" | "stock" | "paid" | "purchased" | "commercial" | "standard" | "extended" => "licensed",
        "royalty-free" | "rf" | "royaltyfree" => "royalty-free",
        "rights-managed" | "rm" | "rightsmanaged" => "rights-managed",
        "permission" | "permitted" | "with-permission" | "released" => "permission",
        "generated" | "ai" | "ai-generated" | "synthetic" => "generated",
        other => other,
    }
    .into()
}

pub fn kind(id: &str) -> Option<&'static Kind> {
    KINDS.iter().find(|k| k.id == id)
}

pub fn kinds_json() -> Value {
    Value::Array(KINDS.iter().map(|k| json!({"id": k.id, "name": k.name, "commercial_use": k.commercial, "credit_required": k.attribution, "editing_allowed": k.derivatives, "share_alike": k.share_alike})).collect())
}

#[derive(Clone, Debug, Default)]
pub struct Rights {
    pub licence: String,
    pub source: String,
    pub author: String,
    pub attribution: String,
    pub url: String,
    pub expires: String,
    pub cost: String,
    pub terms: String,
}

impl Rights {
    pub fn json(&self) -> Value {
        let mut v = json!({"licence": self.licence});
        for (k, s) in [("source", &self.source), ("author", &self.author), ("credit", &self.attribution), ("url", &self.url), ("expires", &self.expires), ("cost", &self.cost), ("terms", &self.terms)] {
            if !s.is_empty() {
                v[k] = json!(s);
            }
        }
        v
    }
}

pub fn get(store: &Store, id: i64) -> Result<Option<Rights>, String> {
    Ok(store.db().query("SELECT licence, source, author, attribution, url, expires, cost, terms FROM rights WHERE asset = ?", &[id.into()])?.first().map(|r| Rights {
        licence: r[0].text(),
        source: r[1].text(),
        author: r[2].text(),
        attribution: r[3].text(),
        url: r[4].text(),
        expires: r[5].text(),
        cost: r[6].text(),
        terms: r[7].text(),
    }))
}

/// Records rights for an asset; only the fields given change.
pub fn set(store: &Store, id: i64, a: &Value) -> Result<Rights, String> {
    let mut r = get(store, id)?.unwrap_or(Rights { licence: "unknown".into(), ..Default::default() });
    let s = |k: &[&str]| k.iter().find_map(|k| a.get(*k).and_then(Value::as_str)).map(|s| s.trim().to_owned());
    if let Some(l) = s(&["licence", "license"]) {
        let id = normalise(&l);
        if kind(&id).is_none() {
            return Err(format!("unknown licence \"{l}\"; use one of: {}", KINDS.iter().map(|k| k.id).collect::<Vec<_>>().join(", ")));
        }
        r.licence = id;
    }
    if let Some(x) = s(&["source"]) {
        r.source = x;
    }
    if let Some(x) = s(&["author", "creator"]) {
        r.author = x;
    }
    if let Some(x) = s(&["credit", "attribution"]) {
        r.attribution = x;
    }
    if let Some(x) = s(&["url"]) {
        r.url = x;
    }
    if let Some(x) = s(&["expires", "expiry"]) {
        if !x.is_empty() && parse_date(&x).is_none() {
            return Err(format!("expires must be a date like 2027-03-31, not \"{x}\""));
        }
        r.expires = x;
    }
    if let Some(x) = s(&["cost"]) {
        r.cost = x;
    }
    if let Some(x) = s(&["terms", "restrictions"]) {
        r.terms = x;
    }
    store.db().run(
        "INSERT OR REPLACE INTO rights (asset, licence, source, author, attribution, url, expires, cost, terms, updated) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, strftime('%s','now'))",
        &[id.into(), r.licence.clone().into(), r.source.clone().into(), r.author.clone().into(), r.attribution.clone().into(), r.url.clone().into(), r.expires.clone().into(), r.cost.clone().into(), r.terms.clone().into()],
    )?;
    store.reindex(id)?;
    Ok(r)
}

// ------------------------------------------------------------------ dates

/// `YYYY-MM-DD` to days since 1970-01-01.
pub fn parse_date(s: &str) -> Option<i64> {
    let mut it = s.trim().split('-');
    let (y, mo, d): (i64, i64, i64) = (it.next()?.parse().ok()?, it.next()?.parse().ok()?, it.next()?.parse().ok()?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || it.next().is_some() {
        return None;
    }
    let y = if mo <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let doy = (153 * (if mo > 2 { mo - 3 } else { mo + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146_097 + doe - 719_468)
}

pub fn today() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64 / 86400)
}

// ------------------------------------------------------------------ check

#[derive(Clone, Debug)]
pub struct Finding {
    /// "blocked" cannot be used as it stands, "warning" needs a look, "credit" needs a credit line.
    pub level: &'static str,
    pub code: &'static str,
    pub asset: String,
    pub path: String,
    pub message: String,
    pub fix: String,
}

impl Finding {
    pub fn json(&self) -> Value {
        json!({"level": self.level, "code": self.code, "asset": self.asset, "path": self.path, "message": self.message, "fix": self.fix})
    }
}

#[derive(Default)]
pub struct Report {
    pub findings: Vec<Finding>,
    /// Credit lines to show (deduplicated).
    pub credits: Vec<String>,
    pub used: usize,
}

impl Report {
    pub fn blocked(&self) -> usize {
        self.findings.iter().filter(|f| f.level == "blocked").count()
    }

    pub fn json(&self) -> Value {
        let n = |l: &str| self.findings.iter().filter(|f| f.level == l).count();
        json!({
            "ok": self.blocked() == 0,
            "used_assets": self.used,
            "blocked": n("blocked"),
            "warnings": n("warning"),
            "findings": self.findings.iter().map(Finding::json).collect::<Vec<_>>(),
            "credits": self.credits,
        })
    }

    /// One line per problem, for a toast or a refusal message.
    pub fn summary(&self) -> String {
        self.findings.iter().filter(|f| f.level != "credit").map(|f| format!("{}: {} ({})", f.asset, f.message, f.level)).collect::<Vec<_>>().join("\n")
    }
}

pub struct Options {
    /// Will the result be used commercially (ads, sales, monetised)?
    pub commercial: bool,
    /// Days since 1970 the film is shown (default: today). Licences ending before this are expired.
    pub on: i64,
}

impl Default for Options {
    fn default() -> Self {
        Options { commercial: true, on: today() }
    }
}

/// The media the timeline uses (all sequences), by id.
pub fn used_media(p: &m::Project) -> HashSet<m::MediaId> {
    p.sequences.iter().flat_map(|s| s.clips.iter()).filter(|c| c.enabled).filter_map(|c| match &c.source {
        m::ClipSource::Media(r) => Some(r.media),
        _ => None,
    }).collect()
}

/// Bundled content: path to its licence and credit, when the library is installed.
fn bundled() -> HashMap<std::path::PathBuf, (String, String)> {
    crate::content::get().map(|c| c.entries.iter().map(|e| (e.path.clone(), (normalise(e.licence), e.attribution.clone()))).collect()).unwrap_or_default()
}

/// Checks what the timeline uses against the recorded rights.
/// Media found on the web or downloaded keeps its source page and is not
/// nagged about a missing licence.
fn wants_licence_note(r: &Rights, asset_source: &str) -> bool {
    asset_source != "downloaded" && r.url.is_empty()
}

/// Folders Zornov writes what it makes into (sound effects, sung and
/// spoken lines, generated pictures and video), in the project or, with no
/// project, under the cache as `made-…`.
const MADE_DIRS: &[&str] = &["sfx", "sung", "voice", "foley", "music", "Generated"];

/// Whether Zornov made this file itself: its own sounds, voices and
/// generations are the project's to use, and were reported as "no licence".
pub fn made_here(path: &Path, folder: &Path) -> bool {
    let cache = studio_paths::cache();
    let under = |root: &Path| path.parent().is_some_and(|p| p.starts_with(root));
    MADE_DIRS.iter().any(|d| under(&folder.join(d)))
        || under(&studio_paths::studio().join("Generated"))
        || path.parent().and_then(|p| p.strip_prefix(&cache).ok()).and_then(|r| r.components().next()).is_some_and(|c| c.as_os_str().to_string_lossy().starts_with("made-"))
}

pub fn check(store: &Store, folder: &Path, project: &m::Project, opts: &Options) -> Result<Report, String> {
    store.sync_model(folder, project)?;
    let used = used_media(project);
    let bundled = bundled();
    let mut rep = Report::default();
    let mut credits = BTreeSet::new();
    for item in project.media.iter().filter(|i| used.contains(&i.id)) {
        let a: Asset = match store.resolve(&json!(item.path)) {
            Ok(a) => a,
            Err(_) => continue,
        };
        rep.used += 1;
        let mut r = get(store, a.id)?.unwrap_or(Rights { licence: "unknown".into(), ..Default::default() });
        let mut from_bundle = false;
        if r.licence == "unknown" {
            if let Some((l, credit)) = bundled.get(Path::new(&a.path)) {
                r.licence = if kind(l).is_some() { l.clone() } else { "licensed".into() };
                if r.attribution.is_empty() {
                    r.attribution = credit.clone();
                }
                from_bundle = true;
            } else if a.source == "generated" || made_here(Path::new(&a.path), folder) {
                r.licence = "generated".into();
            }
        }
        let k = kind(&r.licence).unwrap_or(&KINDS[KINDS.len() - 1]);
        let mut add = |level, code, message: String, fix: &str| {
            rep.findings.push(Finding { level, code, asset: a.name.clone(), path: a.path.clone(), message, fix: fix.into() });
        };
        if a.missing || !Path::new(&a.path).is_file() {
            add("blocked", "missing-file", "the file is not on disk, so it cannot be exported".into(), "relink it: assets action \"relink\"");
        }
        if r.licence == "unknown" && wants_licence_note(&r, &a.source) {
            add("warning", "no-licence", "no licence recorded: nothing says this may be used".into(), "record it: assets action \"licence\" with licence (owned, licensed, cc-by, …)");
        }
        if !r.expires.is_empty() {
            if let Some(end) = parse_date(&r.expires) {
                if end < opts.on {
                    add("blocked", "expired", format!("the licence ran out on {}", r.expires), "renew it, replace the asset, or record the new expiry date");
                } else if end - opts.on <= 30 {
                    add("warning", "expires-soon", format!("the licence runs out on {} ({} days)", r.expires, end - opts.on), "renew it if the film will be shown after that");
                }
            }
        }
        if opts.commercial && !k.commercial {
            add("blocked", "not-commercial", format!("{} does not allow commercial use", k.name), "replace it, get a commercial licence, or export as non-commercial");
        }
        if !k.derivatives {
            add("warning", "no-derivatives", format!("{} does not allow changing the work; cutting and mixing it may break the licence", k.name), "ask the author or use another asset");
        }
        if k.share_alike {
            add("warning", "share-alike", "ShareAlike: the finished film must be shared under the same licence".into(), "use another asset if the film cannot be shared that way");
        }
        if r.licence == "rights-managed" && r.terms.is_empty() && r.expires.is_empty() {
            add("warning", "terms-missing", "rights-managed: record the territory, term and media it covers".into(), "set terms and expires");
        }
        if k.attribution || !r.attribution.is_empty() {
            let line = if !r.attribution.is_empty() {
                r.attribution.clone()
            } else {
                let by = if r.author.is_empty() { String::new() } else { format!(" by {}", r.author) };
                let at = if r.url.is_empty() { String::new() } else { format!(" ({})", r.url) };
                format!("\"{}\"{by}{at}, {}", a.name, k.name)
            };
            if r.attribution.is_empty() && k.attribution && !from_bundle && r.author.is_empty() {
                add("warning", "credit-missing", format!("{} needs a credit line but none was recorded", k.name), "record credit (or author and url)");
            }
            credits.insert(line);
        }
    }
    rep.credits = credits.into_iter().collect();
    rep.findings.sort_by_key(|f| match f.level {
        "blocked" => 0,
        "warning" => 1,
        _ => 2,
    });
    Ok(rep)
}

/// Assets with a licence ending within `days` (or already over).
pub fn expiring(store: &Store, days: i64) -> Result<Vec<(Asset, String, i64)>, String> {
    let mut out = Vec::new();
    for r in store.db().query("SELECT asset, expires FROM rights WHERE expires != ''", &[])? {
        let Some(end) = parse_date(&r[1].text()) else { continue };
        if end - today() <= days {
            out.push((store.get(r[0].int())?, r[1].text(), end - today()));
        }
    }
    out.sort_by_key(|x| x.2);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(tag: &str) -> (Store, std::path::PathBuf) {
        let d = std::env::temp_dir().join(format!("studio-rights-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        (Store::open(&d.join("a.db")).unwrap(), d)
    }

    #[test]
    fn licence_names() {
        assert_eq!(normalise("CC BY 4.0"), "cc-by");
        assert_eq!(normalise("cc-by-nc-sa"), "cc-by-nc");
        assert_eq!(normalise("CC0 1.0"), "cc0");
        assert_eq!(normalise("Stock"), "licensed");
        assert_eq!(normalise(""), "unknown");
        assert_eq!(parse_date("1970-01-02"), Some(1));
        assert_eq!(parse_date("2000-03-01"), Some(11017));
        assert_eq!(parse_date("2024-13-01"), None);
    }

    #[test]
    fn web_media_is_not_asked_for_a_licence() {
        let none = Rights { licence: "unknown".into(), ..Default::default() };
        assert!(wants_licence_note(&none, ""));
        assert!(!wants_licence_note(&none, "downloaded"));
        assert!(!wants_licence_note(&Rights { url: "https://example.org/p".into(), ..none }, ""));
    }

    #[test]
    fn setting_and_reading() {
        let (s, d) = store("set");
        let p = d.join("song.wav");
        std::fs::write(&p, vec![1u8; 2000]).unwrap();
        let id = s.add_file(&p, None).unwrap();
        assert_eq!(get(&s, id).unwrap().is_none(), true);
        let r = set(&s, id, &json!({"licence": "CC BY 4.0", "author": "Kevin", "expires": "2099-01-01", "source": "incompetech"})).unwrap();
        assert_eq!(r.licence, "cc-by");
        assert!(set(&s, id, &json!({"licence": "made-up"})).is_err());
        assert!(set(&s, id, &json!({"expires": "soon"})).is_err());
        // Rights are searchable too.
        let q = crate::assets::Query::from_json(&json!({"licence": "cc-by"}));
        assert_eq!(s.find(&q).unwrap().len(), 1);
        let q = crate::assets::Query::from_json(&json!({"query": "incompetech"}));
        assert_eq!(s.find(&q).unwrap().len(), 1);
        assert!(expiring(&s, 30).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }
}
