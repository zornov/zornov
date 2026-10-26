// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The marketplace: listings (templates, plugins, LUTs, sounds, assets),
//! installing what was bought (checked against its SHA-256), and the
//! revenue-share sums for creators.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const KINDS: &[&str] = &["template", "plugin", "lut", "sound", "asset"];

/// Something for sale (or free).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Listing {
    pub id: String,
    pub kind: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    /// The creator's account (an email) and shown name.
    pub creator: String,
    #[serde(default)]
    pub creator_name: String,
    /// 0 is free.
    pub price_minor: u64,
    pub currency: String,
    /// The file people get, once uploaded.
    #[serde(default)]
    pub file_name: String,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub sha256: String,
    #[serde(default)]
    pub tags: Vec<String>,
    /// Hidden until it has a file.
    #[serde(default)]
    pub published: bool,
}

/// How a sale is split: the creator's share in basis points.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Share {
    pub creator_bps: u32,
}

/// One sale, split.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sale {
    pub order: String,
    pub listing: String,
    pub creator: String,
    pub buyer: String,
    pub gross_minor: u64,
    pub creator_minor: u64,
    pub platform_minor: u64,
    pub currency: String,
    pub at: u64,
}

/// Splits `gross` (the creator's part rounded down; the platform keeps the
/// rest, so the two always add up to the gross).
pub fn split(gross_minor: u64, share: Share) -> (u64, u64) {
    let creator = (gross_minor as u128 * share.creator_bps.min(10_000) as u128 / 10_000) as u64;
    (creator, gross_minor - creator)
}

/// A creator's totals per currency.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Statement {
    pub creator: String,
    pub sales: u64,
    /// `(currency, gross, creator, platform)`.
    pub totals: Vec<(String, u64, u64, u64)>,
}

pub fn statement(creator: &str, sales: &[Sale]) -> Statement {
    let mut s = Statement { creator: creator.into(), ..Default::default() };
    for x in sales.iter().filter(|x| x.creator == creator) {
        s.sales += 1;
        match s.totals.iter_mut().find(|t| t.0 == x.currency) {
            Some(t) => {
                t.1 += x.gross_minor;
                t.2 += x.creator_minor;
                t.3 += x.platform_minor;
            }
            None => s.totals.push((x.currency.clone(), x.gross_minor, x.creator_minor, x.platform_minor)),
        }
    }
    s
}

/// Where an installed item of `kind` lives under the Studio folder.
pub fn install_dir(studio: &Path, kind: &str, id: &str) -> PathBuf {
    let folder = match kind {
        "template" => "Templates",
        "plugin" => "Plugins",
        "lut" => "LUTs",
        "sound" => "Sounds",
        _ => "Assets",
    };
    studio.join("Marketplace").join(folder).join(safe_name(id))
}

/// A file or folder name with nothing that could leave its folder.
pub fn safe_name(s: &str) -> String {
    let n: String = s.chars().map(|c| if c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | ' ') { c } else { '_' }).collect();
    let n = n.trim_matches(|c| c == '.' || c == ' ').to_owned();
    if n.is_empty() { "item".into() } else { n }
}

/// Puts downloaded `bytes` in place after checking their hash, with a
/// `listing.json` beside them. Returns the installed file.
pub fn install(studio: &Path, l: &Listing, bytes: &[u8]) -> Result<PathBuf, String> {
    let got = sha256_hex(bytes);
    if !l.sha256.is_empty() && !got.eq_ignore_ascii_case(&l.sha256) {
        return Err(format!("the download of \"{}\" is damaged (its checksum does not match); nothing was installed", l.title));
    }
    let dir = install_dir(studio, &l.kind, &l.id);
    let file = dir.join(safe_name(if l.file_name.is_empty() { &l.id } else { &l.file_name }));
    studio_entitlements::write_atomic(&file, bytes)?;
    studio_entitlements::write_atomic(&dir.join("listing.json"), serde_json::to_string_pretty(l).expect("serialisable").as_bytes())?;
    Ok(file)
}

/// What is installed, read from the `listing.json` files.
pub fn installed(studio: &Path) -> Vec<Listing> {
    let mut out = Vec::new();
    let Ok(kinds) = std::fs::read_dir(studio.join("Marketplace")) else { return out };
    for k in kinds.flatten() {
        for item in std::fs::read_dir(k.path()).into_iter().flatten().flatten() {
            if let Some(l) = crate::credits::read::<Listing>(&item.path(), "listing.json") {
                out.push(l);
            }
        }
    }
    out.sort_by(|a, b| a.title.cmp(&b.title));
    out
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    studio_entitlements::hex(ring::digest::digest(&ring::digest::SHA256, bytes).as_ref())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_always_add_up() {
        for g in [0, 1, 99, 1_000, 1_999, 123_457] {
            let (c, p) = split(g, Share { creator_bps: 7_000 });
            assert_eq!(c + p, g);
            assert!(c * 10_000 <= g * 7_000);
        }
        assert_eq!(split(1_000, Share { creator_bps: 7_000 }), (700, 300));
        assert_eq!(split(1_000, Share { creator_bps: 20_000 }), (1_000, 0));
    }

    #[test]
    fn statements_total_per_currency() {
        let sale = |c: &str, g, cur: &str| {
            let (cm, pm) = split(g, Share { creator_bps: 8_000 });
            Sale { order: "o".into(), listing: "l".into(), creator: c.into(), buyer: "b".into(), gross_minor: g, creator_minor: cm, platform_minor: pm, currency: cur.into(), at: 0 }
        };
        let s = statement("ana", &[sale("ana", 500, "usd"), sale("ana", 1_000, "usd"), sale("bo", 900, "usd"), sale("ana", 300, "eur")]);
        assert_eq!(s.sales, 3);
        assert_eq!(s.totals, vec![("usd".into(), 1_500, 1_200, 300), ("eur".into(), 300, 240, 60)]);
    }

    #[test]
    fn installs_check_the_hash_and_stay_in_their_folder() {
        let dir = std::env::temp_dir().join(format!("studio-market-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let bytes = b"TITLE \"warm\"\nLUT_3D_SIZE 2\n";
        let mut l = Listing { id: "../../etc".into(), kind: "lut".into(), title: "Warm".into(), description: String::new(), creator: "ana@example.com".into(), creator_name: "Ana".into(), price_minor: 0, currency: "usd".into(), file_name: "../warm.cube".into(), size: bytes.len() as u64, sha256: sha256_hex(bytes), tags: vec![], published: true };
        let f = install(&dir, &l, bytes).unwrap();
        assert!(f.starts_with(dir.join("Marketplace/LUTs")), "{}", f.display());
        assert_eq!(std::fs::read(&f).unwrap(), bytes);
        assert_eq!(installed(&dir), vec![l.clone()]);
        l.sha256 = sha256_hex(b"other");
        assert!(install(&dir, &l, bytes).unwrap_err().contains("damaged"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
