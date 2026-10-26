// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The release configuration: `packaging/release.conf` compiled in, with
//! environment overrides at run time.

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Config {
    pub feed_url: String,
    /// Ed25519 public key, 64 hex characters.
    pub public_key: String,
    pub crash_endpoint: String,
}

/// `key = value` lines; `#` starts a comment.
pub fn parse(text: &str) -> Config {
    let mut c = Config::default();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else { continue };
        let v = v.trim().to_owned();
        match k.trim() {
            "feed_url" => c.feed_url = v,
            "public_key" => c.public_key = v,
            "crash_endpoint" => c.crash_endpoint = v,
            _ => {}
        }
    }
    c
}

/// The configuration this build was made with.
pub fn built_in() -> Config {
    parse(include_str!("../../../../packaging/release.conf"))
}

/// The built-in configuration, overridden by `STUDIO_UPDATE_URL`,
/// `STUDIO_UPDATE_PUBKEY` and `STUDIO_CRASH_ENDPOINT`.
pub fn load() -> Config {
    let mut c = built_in();
    let env = |n: &str| std::env::var(n).ok().map(|v| v.trim().to_owned()).filter(|v| !v.is_empty());
    if let Some(v) = env("STUDIO_UPDATE_URL") {
        c.feed_url = v;
    }
    if let Some(v) = env("STUDIO_UPDATE_PUBKEY") {
        c.public_key = v;
    }
    if let Some(v) = env("STUDIO_CRASH_ENDPOINT") {
        c.crash_endpoint = v;
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_values_and_skips_comments() {
        let c = parse("# feed_url = no\nfeed_url = https://x/feed.json\npublic_key=ab\n\ncrash_endpoint =\n");
        assert_eq!(c, Config { feed_url: "https://x/feed.json".into(), public_key: "ab".into(), crash_endpoint: String::new() });
    }

    #[test]
    fn shipped_file_parses() {
        let _ = built_in();
    }
}
