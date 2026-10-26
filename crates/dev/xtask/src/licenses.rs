// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The licence notices every package carries: Studio's `LICENSE`, `NOTICE`
//! and `ATTRIBUTIONS.md`, the licences of the fonts, icons and colour
//! transforms built into the program, and `THIRD-PARTY-LICENSES.txt`, made
//! from `cargo metadata --locked`: every crate compiled into Studio for the
//! release targets, with its licence and the licence and notice files it
//! ships (or the licence's standard text when it ships none). The same
//! lock file always gives the same bytes.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// The platforms Studio is released for.
pub const TARGETS: &[&str] = &["aarch64-apple-darwin", "x86_64-apple-darwin", "x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu", "x86_64-pc-windows-msvc", "aarch64-pc-windows-msvc"];

/// The file name of the generated list.
pub const THIRD_PARTY: &str = "THIRD-PARTY-LICENSES.txt";

/// Notice files as (file name, contents), sorted by name.
pub type Notices = Vec<(String, Vec<u8>)>;

/// Standard licence texts, for crates that ship none of their own.
const STANDARD: &[(&str, &str)] = &[
    ("0BSD", include_str!("../licenses/0BSD.txt")),
    ("Apache-2.0", include_str!("../licenses/Apache-2.0.txt")),
    ("BSD-2-Clause", include_str!("../licenses/BSD-2-Clause.txt")),
    ("BSD-3-Clause", include_str!("../licenses/BSD-3-Clause.txt")),
    ("BSL-1.0", include_str!("../licenses/BSL-1.0.txt")),
    ("CC0-1.0", include_str!("../licenses/CC0-1.0.txt")),
    ("ISC", include_str!("../licenses/ISC.txt")),
    ("LLVM-exception", include_str!("../licenses/LLVM-exception.txt")),
    ("MIT", include_str!("../licenses/MIT.txt")),
    ("Unicode-3.0", include_str!("../licenses/Unicode-3.0.txt")),
    ("Unlicense", include_str!("../licenses/Unlicense.txt")),
    ("Zlib", include_str!("../licenses/Zlib.txt")),
];

/// One crate in the program.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Crate {
    pub name: String,
    pub version: String,
    pub license: String,
    /// (where the text came from, the text), in file-name order.
    pub texts: Vec<(String, String)>,
    /// Who holds the copyright, when the texts are standard ones that leave
    /// it out (the crate's authors).
    pub copyright: Option<String>,
}

/// `cargo metadata --locked` for the release targets.
pub fn metadata() -> Result<serde_json::Value, String> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut c = std::process::Command::new(cargo);
    c.current_dir(crate::root()).args(["metadata", "--locked", "--format-version", "1"]);
    for t in TARGETS {
        c.args(["--filter-platform", t]);
    }
    let out = c.output().map_err(|e| format!("cannot run cargo metadata: {e}"))?;
    if !out.status.success() {
        return Err(format!("cargo metadata failed: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    serde_json::from_slice(&out.stdout).map_err(|e| format!("cargo metadata: {e}"))
}

/// Line ends made `\n`, trailing spaces and the blank lines around the text
/// dropped, so copies that differ only in that are one text.
fn normalise(text: &str) -> String {
    let lines: Vec<&str> = text.lines().map(str::trim_end).collect();
    let start = lines.iter().position(|l| !l.is_empty()).unwrap_or(lines.len());
    let end = lines.iter().rposition(|l| !l.is_empty()).map_or(start, |e| e + 1);
    lines[start..end].join("\n")
}

const KEYWORDS: &[&str] = &["LICENSE", "LICENCE", "COPYING", "NOTICE", "UNLICENSE", "COPYRIGHT"];

/// Whether a file at the top of a crate is a licence or notice file.
fn is_notice(name: &str) -> bool {
    let n = name.to_ascii_uppercase();
    KEYWORDS.iter().any(|p| n.starts_with(p))
}

/// Whether a file deeper in a crate is the licence of something bundled
/// there (vendored C code, fonts, data): a text file named like a licence
/// (`LICENSE`, `OFL.txt`, `zlib/COPYING`), or a font's text file beside it
/// (`fonts/Hack-Regular.txt` for `Hack-Regular.ttf`).
fn is_bundled_notice(name: &str, siblings: &BTreeSet<String>) -> bool {
    let (stem, ext) = name.rsplit_once('.').map_or((name, ""), |(s, e)| (s, e));
    if !["", "txt", "md", "markdown", "rst"].contains(&ext.to_ascii_lowercase().as_str()) {
        return false;
    }
    let n = name.to_ascii_uppercase();
    KEYWORDS.iter().chain(&["OFL", "UFL"]).any(|k| n.contains(k)) || (!ext.is_empty() && ["ttf", "otf", "ttc", "woff", "woff2"].iter().any(|f| siblings.contains(&format!("{stem}.{f}"))))
}

/// Folders whose files are not built into the program.
const UNSHIPPED: &[&str] = &["tests", "test", "benches", "examples", "target", "fuzz", "docs", "doc", ".github", ".git", "ci"];

/// The licence and notice files a crate ships: those at the top of its
/// folder (and inside a top-level `LICENSES` folder), the licences of what
/// it bundles further down, and its `license-file`.
fn shipped(dir: &Path, license_file: Option<&str>) -> Vec<(String, String)> {
    fn walk(d: &Path, rel: &str, depth: usize, files: &mut BTreeMap<String, std::path::PathBuf>) {
        let Ok(rd) = std::fs::read_dir(d) else { return };
        let entries: Vec<(String, std::path::PathBuf)> = rd.flatten().map(|e| (e.file_name().to_string_lossy().into_owned(), e.path())).collect();
        let siblings: BTreeSet<String> = entries.iter().map(|e| e.0.clone()).collect();
        for (name, p) in &entries {
            let here = format!("{rel}{name}");
            if p.is_dir() {
                if depth == 0 && is_notice(name) {
                    for (n, f) in entries_of(p) {
                        files.insert(format!("{here}/{n}"), f);
                    }
                } else if depth < 4 && !UNSHIPPED.contains(&name.to_ascii_lowercase().as_str()) {
                    walk(p, &format!("{here}/"), depth + 1, files);
                }
            } else if if depth == 0 { is_notice(name) } else { is_bundled_notice(name, &siblings) } {
                files.insert(here, p.clone());
            }
        }
    }
    fn entries_of(d: &Path) -> Vec<(String, std::path::PathBuf)> {
        std::fs::read_dir(d).into_iter().flatten().flatten().filter(|e| e.path().is_file()).map(|e| (e.file_name().to_string_lossy().into_owned(), e.path())).collect()
    }
    let mut files: BTreeMap<String, std::path::PathBuf> = BTreeMap::new();
    walk(dir, "", 0, &mut files);
    if let Some(f) = license_file {
        let p = dir.join(f);
        if p.is_file() {
            files.entry(f.replace('\\', "/")).or_insert(p);
        }
    }
    files.into_iter().filter_map(|(name, p)| std::fs::read(&p).ok().map(|b| (name, normalise(&String::from_utf8_lossy(&b))))).filter(|(_, t)| !t.is_empty()).collect()
}

/// The licence identifiers an SPDX expression names (`MIT OR Apache-2.0`,
/// `Apache-2.0 WITH LLVM-exception`, the old `MIT/Apache-2.0`).
pub fn ids(expr: &str) -> BTreeSet<String> {
    expr.split(|c: char| c.is_whitespace() || "()/".contains(c)).filter(|t| !t.is_empty() && !["OR", "AND", "WITH"].contains(t)).map(|t| t.trim_end_matches('+').to_owned()).collect()
}

/// The standard texts for a crate that ships none.
fn standard(name: &str, expr: &str) -> Result<Vec<(String, String)>, String> {
    ids(expr)
        .into_iter()
        .map(|id| {
            let text = STANDARD.iter().find(|(i, _)| *i == id).map(|(_, t)| t).ok_or_else(|| format!("{name} ships no licence file and there is no standard text for {id} in crates/dev/xtask/licenses"))?;
            Ok((format!("standard {id} text"), normalise(text)))
        })
        .collect()
}

/// Every crate compiled into `studio-app` (normal and build dependencies,
/// not dev-only ones) except Studio's own: crates from outside the
/// workspace, and workspace crates under a licence other than Studio's
/// (vendored from other projects). Sorted by name and version.
pub fn crates(meta: &serde_json::Value) -> Result<Vec<Crate>, String> {
    let packages = meta["packages"].as_array().ok_or("cargo metadata has no packages")?;
    let by_id: BTreeMap<&str, &serde_json::Value> = packages.iter().filter_map(|p| Some((p["id"].as_str()?, p))).collect();
    let members: BTreeSet<&str> = meta["workspace_members"].as_array().into_iter().flatten().filter_map(|m| m.as_str()).collect();
    let app = members.iter().copied().find(|id| by_id.get(id).is_some_and(|p| p["name"] == "studio-app")).ok_or("no studio-app in the workspace")?;
    let nodes: BTreeMap<&str, &serde_json::Value> = meta["resolve"]["nodes"].as_array().ok_or("cargo metadata has no resolve graph")?.iter().filter_map(|n| Some((n["id"].as_str()?, n))).collect();
    let (mut seen, mut todo) = (BTreeSet::new(), vec![app]);
    while let Some(id) = todo.pop() {
        if !seen.insert(id) {
            continue;
        }
        for d in nodes.get(id).and_then(|n| n["deps"].as_array()).into_iter().flatten() {
            let used = d["dep_kinds"].as_array().into_iter().flatten().any(|k| k["kind"].as_str() != Some("dev"));
            if let (true, Some(pkg)) = (used, d["pkg"].as_str()) {
                todo.push(pkg);
            }
        }
    }
    let ours = |p: &serde_json::Value| p["license"].as_str() == Some("AGPL-3.0-or-later");
    let mut out = Vec::new();
    for id in seen {
        let p = by_id[id];
        if members.contains(id) && ours(p) {
            continue;
        }
        let name = p["name"].as_str().unwrap_or_default().to_owned();
        let license = p["license"].as_str().unwrap_or_default().to_owned();
        let dir = Path::new(p["manifest_path"].as_str().unwrap_or_default()).parent().map(Path::to_path_buf).unwrap_or_default();
        let mut texts = shipped(&dir, p["license_file"].as_str());
        let mut copyright = None;
        if texts.is_empty() {
            if license.is_empty() {
                return Err(format!("{name} has neither a licence expression nor a licence file"));
            }
            // The standard MIT, BSD and ISC texts leave the holder out.
            let authors: Vec<&str> = p["authors"].as_array().into_iter().flatten().filter_map(|a| a.as_str()).collect();
            copyright = Some(if authors.is_empty() { format!("the {name} authors") } else { authors.join(", ") });
            texts = standard(&name, &license)?;
        }
        let license = if license.is_empty() { format!("see {}", texts.iter().map(|t| t.0.as_str()).collect::<Vec<_>>().join(", ")) } else { license };
        out.push(Crate { name, version: p["version"].as_str().unwrap_or_default().to_owned(), license, texts, copyright });
    }
    out.sort_by(|a, b| (&a.name, version_key(&a.version)).cmp(&(&b.name, version_key(&b.version))));
    Ok(out)
}

fn version_key(v: &str) -> Vec<u64> {
    v.split(['.', '-', '+']).map(|p| p.parse().unwrap_or(0)).collect()
}

/// The text of `THIRD-PARTY-LICENSES.txt`: the crates with their licences,
/// then each distinct licence text once with the crates it covers.
pub fn render(crates: &[Crate]) -> String {
    // Each text, with who ships it; numbered in order of first use.
    let mut order: Vec<&str> = Vec::new();
    let mut users: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for c in crates {
        for (from, text) in &c.texts {
            if !users.contains_key(text.as_str()) {
                order.push(text);
            }
            users.entry(text).or_default().push(format!("{} {} ({from})", c.name, c.version));
        }
    }
    let number = |t: &str| order.iter().position(|o| *o == t).unwrap() + 1;
    let mut s = String::new();
    s += "Third-party software in Zornov\n==============================\n\n";
    s += &format!(
        "Zornov includes the {} open-source crates listed below, each used under\nthe licence shown. Their licence and notice texts follow the list; a text\nshared by several crates is given once. Zornov's own licence is in LICENSE\nand NOTICE; fonts, icons and library content are credited in their own\nlicence files and in ATTRIBUTIONS.md.\n\nRelease targets: {}.\n\n",
        crates.len(),
        TARGETS.join(", ")
    );
    s += "Crates\n------\n\n";
    for c in crates {
        let nums: BTreeSet<usize> = c.texts.iter().map(|(_, t)| number(t)).collect();
        s += &format!("{} {}: {} [text {}]\n", c.name, c.version, c.license, nums.iter().map(usize::to_string).collect::<Vec<_>>().join(", "));
        if let Some(who) = &c.copyright {
            s += &format!("    Copyright (c) {who}\n");
        }
    }
    s += "\nLicence texts\n-------------\n";
    for (i, t) in order.iter().enumerate() {
        s += &format!("\n{}\nText {}, for:\n", "=".repeat(78), i + 1);
        for u in &users[t] {
            s += &format!("  {u}\n");
        }
        s += &format!("{}\n\n{t}\n", "=".repeat(78));
    }
    s
}

/// `THIRD-PARTY-LICENSES.txt` for this lock file.
pub fn third_party() -> Result<String, String> {
    Ok(render(&crates(&metadata()?)?))
}

/// Every notice file a package carries: Studio's `LICENSE`, `NOTICE` and
/// `ATTRIBUTIONS.md`, the licence files of the fonts and icons in
/// `app/assets`, the ACES licence (`crates/picture/colour`) and
/// `THIRD-PARTY-LICENSES.txt`.
pub fn notices() -> Result<Notices, String> {
    let root = crate::root();
    let read = |p: &Path| std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()));
    let mut out: Notices = Vec::new();
    for f in ["LICENSE", "NOTICE", "ATTRIBUTIONS.md", "crates/picture/colour/LICENSE-ACES.md"] {
        let p = root.join(f);
        out.push((p.file_name().unwrap().to_string_lossy().into_owned(), read(&p)?));
    }
    for i in crate::repro::collect(&root.join("app/assets"))? {
        let name = i.rel.rsplit('/').next().unwrap_or(&i.rel).to_owned();
        if name.to_ascii_uppercase().contains("LICENSE") {
            out.push((name, read(&i.src)?));
        }
    }
    out.push((THIRD_PARTY.into(), third_party()?.into_bytes()));
    out.sort();
    if let Some(w) = out.windows(2).find(|w| w[0].0 == w[1].0) {
        return Err(format!("two notice files are named {}", w[0].0));
    }
    Ok(out)
}

/// `cargo xtask licenses`: writes `dist/THIRD-PARTY-LICENSES.txt`.
pub fn licenses_cmd() -> Result<String, String> {
    let list = crates(&metadata()?)?;
    let dist = crate::root().join("dist");
    std::fs::create_dir_all(&dist).map_err(|e| e.to_string())?;
    let out = dist.join(THIRD_PARTY);
    std::fs::write(&out, render(&list)).map_err(|e| format!("{}: {e}", out.display()))?;
    let standard = list.iter().filter(|c| c.texts.iter().any(|t| t.0.starts_with("standard "))).count();
    Ok(format!("{} ({} crates, {standard} with the standard text of their licence)", crate::release::rel(&out), list.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn krate(name: &str, version: &str, texts: &[(&str, &str)]) -> Crate {
        Crate { name: name.into(), version: version.into(), license: "MIT".into(), texts: texts.iter().map(|(f, t)| ((*f).into(), (*t).into())).collect(), copyright: None }
    }

    #[test]
    fn identical_texts_are_given_once() {
        let c = Crate { copyright: Some("Cy".into()), ..krate("c", "0.1.0", &[("standard MIT text", "the MIT terms")]) };
        let s = render(&[krate("a", "1.0.0", &[("LICENSE", "the MIT terms")]), krate("b", "2.0.0", &[("LICENSE-MIT", "the MIT terms"), ("NOTICE", "b notice")]), c]);
        assert_eq!(s.matches("the MIT terms").count(), 1, "{s}");
        assert!(s.contains("a 1.0.0: MIT [text 1]") && s.contains("b 2.0.0: MIT [text 1, 2]"), "{s}");
        assert!(s.contains("c 0.1.0: MIT [text 1]\n    Copyright (c) Cy\n"), "{s}");
        assert!(s.contains("  a 1.0.0 (LICENSE)\n  b 2.0.0 (LICENSE-MIT)\n"), "{s}");
    }

    #[test]
    fn expressions_and_texts_are_read_plainly() {
        assert_eq!(ids("(MIT OR Apache-2.0) AND Unicode-3.0"), ["Apache-2.0", "MIT", "Unicode-3.0"].map(String::from).into());
        assert_eq!(ids("MIT/Apache-2.0"), ["Apache-2.0", "MIT"].map(String::from).into());
        assert_eq!(ids("Apache-2.0 WITH LLVM-exception"), ["Apache-2.0", "LLVM-exception"].map(String::from).into());
        assert_eq!(normalise("\r\n\r\nline one  \r\nline two\r\n\r\n"), "line one\nline two");
        let t = standard("foo", "MIT OR Zlib").unwrap();
        assert_eq!(t.len(), 2);
        assert!(t[0].1.starts_with("MIT License") && t[1].0 == "standard Zlib text", "{t:?}");
        assert!(standard("foo", "GPL-3.0").is_err());
    }

    /// The real list, from this lock file: every crate has a text, the
    /// common ones are there, and two runs give the same bytes.
    #[test]
    fn the_third_party_list_covers_every_crate_and_is_stable() {
        let meta = metadata().unwrap();
        let list = crates(&meta).unwrap();
        assert!(list.len() > 50, "{} crates", list.len());
        assert!(list.iter().all(|c| !c.texts.is_empty()));
        for name in ["serde", "serde_json", "egui", "wgpu"] {
            assert!(list.iter().any(|c| c.name == name), "{name} is missing");
        }
        assert!(list.iter().all(|c| !c.name.starts_with("studio-") || c.license != "AGPL-3.0-or-later"), "Zornov's own crates are not third-party");
        assert_eq!(render(&list), render(&crates(&meta).unwrap()));
    }
}
