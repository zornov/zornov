// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The update feed and its signatures.

use ring::digest;
use ring::signature::{self, UnparsedPublicKey};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// One platform's archive.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// Absolute, or relative to the feed's URL.
    pub url: String,
    pub size: u64,
    /// SHA-256 of the archive, hex.
    pub sha256: String,
    /// Ed25519 signature (hex) over [`message`].
    pub signature: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Feed {
    pub schema: u32,
    pub version: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub date: String,
    /// Keyed by [`platform`] (`macos-aarch64`, `windows-x86_64`, ...).
    pub platforms: BTreeMap<String, Entry>,
}

/// This build's platform key.
pub fn platform() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

/// What is signed: the platform, version, archive hash and size (the URL is
/// not, because the hash pins the bytes wherever they are fetched from).
pub fn message(platform: &str, version: &str, sha256: &str, size: u64) -> Vec<u8> {
    format!("studio-update/1\n{platform}\n{version}\n{}\n{size}\n", sha256.to_ascii_lowercase()).into_bytes()
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    crate::hex(digest::digest(&digest::SHA256, bytes).as_ref())
}

/// SHA-256 of a file, hex.
pub fn sha256_file(path: &std::path::Path) -> std::io::Result<String> {
    use std::io::Read;
    let mut ctx = digest::Context::new(&digest::SHA256);
    let mut f = std::fs::File::open(path)?;
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        ctx.update(&buf[..n]);
    }
    Ok(crate::hex(ctx.finish().as_ref()))
}

/// Checks `entry`'s signature for `version` on `platform` against the hex
/// public key.
pub fn verify(public_key_hex: &str, platform: &str, version: &str, entry: &Entry) -> Result<(), String> {
    let key = crate::unhex(public_key_hex).filter(|k| k.len() == 32).ok_or("the app has no valid update key (public_key in packaging/release.conf)")?;
    let sig = crate::unhex(&entry.signature).ok_or("the update's signature is not hex")?;
    UnparsedPublicKey::new(&signature::ED25519, key)
        .verify(&message(platform, version, &entry.sha256, entry.size), &sig)
        .map_err(|_| "the update's signature does not match: it was not signed with Zornov's update key".to_owned())
}

/// `1.2.3` (a leading `v` and a `-pre` or `+build` tail are ignored).
pub fn parse_version(v: &str) -> Option<(u64, u64, u64)> {
    let v = v.trim().trim_start_matches('v');
    let core = v.split(['-', '+']).next()?;
    let mut it = core.split('.');
    let n = |s: Option<&str>| s.map_or(Some(0), |s| s.parse().ok());
    let r = (n(it.next())?, n(it.next())?, n(it.next())?);
    it.next().is_none().then_some(r)
}

pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse_version(candidate), parse_version(current)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

/// The URL of `entry` against the feed's own URL.
pub fn resolve(feed_url: &str, url: &str) -> String {
    if url.contains("://") {
        url.to_owned()
    } else {
        let base = feed_url.rsplit_once('/').map_or(feed_url, |(b, _)| b);
        format!("{base}/{}", url.trim_start_matches('/'))
    }
}

pub fn parse(bytes: &[u8]) -> Result<Feed, String> {
    let f: Feed = serde_json::from_slice(bytes).map_err(|e| format!("the update feed is not valid: {e}"))?;
    if f.schema != 1 {
        return Err(format!("the update feed is schema {}, this Zornov reads 1", f.schema));
    }
    Ok(f)
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use ring::rand::SystemRandom;
    use ring::signature::{Ed25519KeyPair, KeyPair};

    pub fn keypair() -> (Ed25519KeyPair, String) {
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).unwrap();
        let kp = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap();
        let public = crate::hex(kp.public_key().as_ref());
        (kp, public)
    }

    pub fn signed(kp: &Ed25519KeyPair, platform: &str, version: &str, data: &[u8]) -> Entry {
        let sha256 = sha256_hex(data);
        let size = data.len() as u64;
        let signature = crate::hex(kp.sign(&message(platform, version, &sha256, size)).as_ref());
        Entry { url: "studio.tar.gz".into(), size, sha256, signature }
    }

    #[test]
    fn a_good_signature_passes_and_any_change_fails() {
        let (kp, public) = keypair();
        let e = signed(&kp, "macos-aarch64", "0.2.0", b"archive");
        verify(&public, "macos-aarch64", "0.2.0", &e).unwrap();
        assert!(verify(&public, "macos-aarch64", "0.2.1", &e).is_err(), "version is signed");
        assert!(verify(&public, "linux-x86_64", "0.2.0", &e).is_err(), "platform is signed");
        assert!(verify(&public, "macos-aarch64", "0.2.0", &Entry { sha256: sha256_hex(b"other"), ..e.clone() }).is_err(), "hash is signed");
        assert!(verify(&public, "macos-aarch64", "0.2.0", &Entry { size: 1, ..e.clone() }).is_err(), "size is signed");
        let (_, other) = keypair();
        assert!(verify(&other, "macos-aarch64", "0.2.0", &e).is_err(), "another key");
        assert!(verify("", "macos-aarch64", "0.2.0", &e).unwrap_err().contains("no valid update key"));
    }

    #[test]
    fn versions_order_numerically() {
        assert!(is_newer("0.10.0", "0.9.9"));
        assert!(is_newer("v1.0.0", "0.99.0"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.2.0"), "never a downgrade");
        assert!(!is_newer("junk", "0.1.0"));
        assert_eq!(parse_version("1.2.3-beta+5"), Some((1, 2, 3)));
    }

    #[test]
    fn urls_resolve_against_the_feed() {
        assert_eq!(resolve("https://a.b/studio/feed.json", "x.tar.gz"), "https://a.b/studio/x.tar.gz");
        assert_eq!(resolve("https://a.b/f.json", "https://c.d/x"), "https://c.d/x");
    }

    #[test]
    fn feed_round_trips() {
        let (kp, _) = keypair();
        let mut platforms = BTreeMap::new();
        platforms.insert("linux-x86_64".to_owned(), signed(&kp, "linux-x86_64", "0.2.0", b"a"));
        let f = Feed { schema: 1, version: "0.2.0".into(), notes: "n".into(), date: "2026-10-24".into(), platforms };
        assert_eq!(parse(&serde_json::to_vec(&f).unwrap()).unwrap(), f);
        assert!(parse(br#"{"schema":2,"version":"1","platforms":{}}"#).is_err());
    }
}
