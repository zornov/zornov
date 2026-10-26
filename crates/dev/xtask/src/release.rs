// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Releases: the update signing key, the signed feed, and (macOS) Developer
//! ID signing, notarization and the DMG. Secrets are read from the
//! environment or the keychain when needed and are never printed.

use crate::repro;
use ring::rand::SystemRandom;
use ring::signature::{Ed25519KeyPair, KeyPair};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use studio_update::feed::{self, Entry, Feed};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
const DOCS: &str = "docs/releasing.md";

pub fn release_dir() -> PathBuf {
    crate::root().join("dist/release")
}

// ---------------------------------------------------------------- update key

fn key_file() -> PathBuf {
    studio_update::data_dir().join("signing/update-key.hex")
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
    o.open(path).and_then(|mut f| f.write_all(bytes)).map_err(|e| format!("{}: {e}", path.display()))
}

/// The update signing key: `STUDIO_UPDATE_KEY` (hex PKCS#8, for CI
/// secrets), `STUDIO_UPDATE_KEY_FILE`, or the key file `update-key` made.
pub fn update_key() -> Result<Ed25519KeyPair, String> {
    let hex = if let Some(v) = std::env::var("STUDIO_UPDATE_KEY").ok().filter(|v| !v.trim().is_empty()) {
        v
    } else {
        let path = std::env::var_os("STUDIO_UPDATE_KEY_FILE").map(PathBuf::from).unwrap_or_else(key_file);
        std::fs::read_to_string(&path).map_err(|_| format!("no update signing key (looked for STUDIO_UPDATE_KEY, STUDIO_UPDATE_KEY_FILE and {}); run `cargo xtask update-key` once ({DOCS})", path.display()))?
    };
    let der = studio_update::unhex(&hex).ok_or("the update signing key is not hex")?;
    Ed25519KeyPair::from_pkcs8(&der).map_err(|_| "the update signing key is not a valid Ed25519 key".into())
}

/// `cargo xtask update-key`: makes the key once; prints only the public half.
pub fn update_key_cmd() -> Result<String, String> {
    let path = key_file();
    let made = if path.is_file() {
        false
    } else {
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).map_err(|_| "no randomness")?;
        write_private(&path, studio_update::hex(pkcs8.as_ref()).as_bytes())?;
        true
    };
    let kp = update_key()?;
    let public = studio_update::hex(kp.public_key().as_ref());
    Ok(format!(
        "{} the update signing key in {} (keep it private and backed up: losing it means shipping a new app to change the key).\nPublic key, for `public_key =` in packaging/release.conf:\n{public}",
        if made { "made" } else { "using" },
        path.display()
    ))
}

// ------------------------------------------------------------------- the feed

/// Signs an update archive for `platform` and records its feed entry in
/// `dist/release/entries/`.
pub fn sign_update(archive: &Path, platform: &str) -> Result<PathBuf, String> {
    let kp = update_key()?;
    let size = std::fs::metadata(archive).map_err(|e| format!("{}: {e}", archive.display()))?.len();
    let sha256 = feed::sha256_file(archive).map_err(|e| e.to_string())?;
    let signature = studio_update::hex(kp.sign(&feed::message(platform, VERSION, &sha256, size)).as_ref());
    let url = archive.file_name().unwrap().to_string_lossy().into_owned();
    let entry = Entry { url, size, sha256, signature };
    let dir = release_dir().join("entries");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let file = dir.join(format!("{platform}.json"));
    let body = serde_json::json!({ "version": VERSION, "platform": platform, "entry": entry });
    std::fs::write(&file, serde_json::to_vec_pretty(&body).unwrap()).map_err(|e| e.to_string())?;
    Ok(file)
}

/// Builds `feed.json` from the entries, checking every signature against
/// the public key it will ship with. Archive URLs are made absolute with
/// `--base-url` / `STUDIO_UPDATE_BASE_URL`; with neither and a GitHub
/// Releases `feed_url`, they point at this version's tagged release (so a
/// newer release published mid-download cannot mix files); else they are
/// left beside the feed.
/// The content library download's version: raise it when the library changes.
const LIBRARY_VERSION: u32 = 1;

/// `cargo xtask library-pack`: the core content library as one archive
/// (`dist/release/Zornov-library-N.tar.gz`, files under `core/`) for a
/// release tagged `library-N`, and `packaging/library.conf`, which the app
/// is built with, naming its address, size and SHA-256.
pub fn library_pack_cmd() -> Result<String, String> {
    let core = crate::library_core().ok_or("no content library to pack (content-build/core, or STUDIO_CONTENT_CORE)")?;
    let items: Vec<repro::Item> = repro::collect(&core)?.into_iter().map(|i| repro::Item { rel: format!("core/{}", i.rel), ..i }).collect();
    std::fs::create_dir_all(release_dir()).map_err(|e| e.to_string())?;
    let name = format!("Zornov-library-{LIBRARY_VERSION}.tar.gz");
    let out = release_dir().join(&name);
    repro::tar_gz(&out, None, &items, &[])?;
    let sha256 = feed::sha256_file(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    let size = std::fs::metadata(&out).map_err(|e| e.to_string())?.len();
    let conf = format!(
        "# The content library the app offers to download when it has none\n\
         # (`cargo xtask library-pack` writes this; upload the archive to a\n\
         # release tagged library-{LIBRARY_VERSION} on zornov/zornov, not marked latest).\n\
         version = {LIBRARY_VERSION}\n\
         url = https://github.com/zornov/zornov/releases/download/library-{LIBRARY_VERSION}/{name}\n\
         size = {size}\n\
         sha256 = {sha256}\n"
    );
    let conf_path = crate::root().join("packaging/library.conf");
    std::fs::write(&conf_path, conf).map_err(|e| format!("{}: {e}", conf_path.display()))?;
    Ok(format!("{} ({} files, {} MB); packaging/library.conf updated", rel(&out), items.len(), size >> 20))
}

pub fn feed_cmd(args: &[String]) -> Result<String, String> {
    let mut base = std::env::var("STUDIO_UPDATE_BASE_URL").ok().filter(|v| !v.is_empty()).or_else(|| github_base(&studio_update::config::load().feed_url, VERSION));
    let mut notes = std::env::var("STUDIO_RELEASE_NOTES").unwrap_or_default();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--base-url" => base = it.next().cloned(),
            "--notes" => notes = it.next().map(|f| std::fs::read_to_string(f).map_err(|e| format!("{f}: {e}"))).transpose()?.unwrap_or_default(),
            other => return Err(format!("unknown option {other}; usage: cargo xtask feed [--base-url URL] [--notes FILE]")),
        }
    }
    let kp = update_key()?;
    let public = studio_update::hex(kp.public_key().as_ref());
    let shipped = studio_update::config::load().public_key;
    if !shipped.is_empty() && !shipped.eq_ignore_ascii_case(&public) {
        return Err("the update signing key is not the one in packaging/release.conf: apps built with that file would refuse this feed".into());
    }
    let dir = release_dir().join("entries");
    let mut platforms = BTreeMap::new();
    for e in std::fs::read_dir(&dir).map_err(|_| format!("no update archives yet in {}: run a release or package command first", dir.display()))? {
        let p = e.map_err(|e| e.to_string())?.path();
        if p.extension().is_none_or(|e| e != "json") {
            continue;
        }
        let v: serde_json::Value = serde_json::from_slice(&std::fs::read(&p).map_err(|e| e.to_string())?).map_err(|e| format!("{}: {e}", p.display()))?;
        let (version, platform) = (v["version"].as_str().unwrap_or_default().to_owned(), v["platform"].as_str().unwrap_or_default().to_owned());
        if version != VERSION {
            return Err(format!("{} is for version {version}, not {VERSION}: remove old entries", p.display()));
        }
        let mut entry: Entry = serde_json::from_value(v["entry"].clone()).map_err(|e| format!("{}: {e}", p.display()))?;
        feed::verify(&public, &platform, &version, &entry)?;
        if let Some(b) = &base {
            entry.url = format!("{}/{}", b.trim_end_matches('/'), entry.url);
        }
        platforms.insert(platform, entry);
    }
    if platforms.is_empty() {
        return Err("no update entries to put in the feed".into());
    }
    let n = platforms.len();
    let (y, m, d, ..) = repro::civil(repro::epoch());
    let f = Feed { schema: 1, version: VERSION.into(), notes: notes.trim().into(), date: format!("{y:04}-{m:02}-{d:02}"), platforms };
    let out = release_dir().join("feed.json");
    std::fs::write(&out, serde_json::to_vec_pretty(&f).unwrap()).map_err(|e| e.to_string())?;
    Ok(format!("{} ({n} platform{}, version {VERSION}); upload it with the archives it lists", rel(&out), if n == 1 { "" } else { "s" }))
}

/// `https://github.com/o/r/releases/latest/download/feed.json` ->
/// `https://github.com/o/r/releases/download/v<version>`.
pub fn github_base(feed_url: &str, version: &str) -> Option<String> {
    let (repo, rest) = feed_url.split_once("/releases/")?;
    (repo.starts_with("https://github.com/") && rest.starts_with("latest/download/")).then(|| format!("{repo}/releases/download/v{version}"))
}

pub fn rel(p: &Path) -> String {
    p.strip_prefix(crate::root()).unwrap_or(p).display().to_string()
}

// ------------------------------------------------------------------ macOS

/// How notarytool signs in. None of these values is printed.
#[derive(Debug, PartialEq, Eq)]
pub enum Notary {
    /// A profile made with `xcrun notarytool store-credentials`.
    Profile { name: String, keychain: Option<String> },
    /// An App Store Connect API key (.p8) with its key id and issuer.
    ApiKey { path: String, key_id: String, issuer: String },
}

impl Notary {
    fn args(&self) -> Vec<String> {
        match self {
            Notary::Profile { name, keychain } => {
                let mut v = vec!["--keychain-profile".to_owned(), name.clone()];
                if let Some(k) = keychain {
                    v.extend(["--keychain".to_owned(), k.clone()]);
                }
                v
            }
            Notary::ApiKey { path, key_id, issuer } => vec!["--key".into(), path.clone(), "--key-id".into(), key_id.clone(), "--issuer".into(), issuer.clone()],
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct MacConfig {
    pub identity: String,
    pub notary: Option<Notary>,
}

/// The signing identity and notarization sign-in from the environment
/// (`STUDIO_SIGN_IDENTITY`, `STUDIO_NOTARY_PROFILE` or `STUDIO_NOTARY_KEY` +
/// `STUDIO_NOTARY_KEY_ID` + `STUDIO_NOTARY_ISSUER`), or the keychain's one
/// Developer ID identity. Everything missing is listed at once.
pub fn mac_config(env: &dyn Fn(&str) -> Option<String>, keychain_identities: &dyn Fn() -> Vec<(String, String)>, notarize: bool, identity_flag: Option<String>) -> Result<MacConfig, String> {
    let mut missing: Vec<String> = Vec::new();
    let identity = identity_flag.or_else(|| env("STUDIO_SIGN_IDENTITY")).or_else(|| {
        let found = keychain_identities();
        match found.len() {
            1 => Some(found[0].0.clone()),
            0 => {
                missing.push("a \"Developer ID Application\" certificate in your keychain (or set STUDIO_SIGN_IDENTITY to its name or SHA-1)".into());
                None
            }
            _ => {
                missing.push(format!("STUDIO_SIGN_IDENTITY: the keychain has several Developer ID identities ({}); say which", found.iter().map(|f| f.1.as_str()).collect::<Vec<_>>().join("; ")));
                None
            }
        }
    });
    let notary = if !notarize {
        None
    } else if let Some(name) = env("STUDIO_NOTARY_PROFILE") {
        Some(Notary::Profile { name, keychain: env("STUDIO_NOTARY_KEYCHAIN") })
    } else if let Some(path) = env("STUDIO_NOTARY_KEY") {
        let (key_id, issuer) = (env("STUDIO_NOTARY_KEY_ID"), env("STUDIO_NOTARY_ISSUER"));
        if key_id.is_none() {
            missing.push("STUDIO_NOTARY_KEY_ID (the API key's id)".into());
        }
        if issuer.is_none() {
            missing.push("STUDIO_NOTARY_ISSUER (the issuer id from App Store Connect)".into());
        }
        if !Path::new(&path).is_file() {
            missing.push("STUDIO_NOTARY_KEY must be the path of the .p8 API key file".into());
        }
        Some(Notary::ApiKey { path, key_id: key_id.unwrap_or_default(), issuer: issuer.unwrap_or_default() })
    } else {
        missing.push("notarization sign-in: STUDIO_NOTARY_PROFILE (a profile from `xcrun notarytool store-credentials`) or STUDIO_NOTARY_KEY + STUDIO_NOTARY_KEY_ID + STUDIO_NOTARY_ISSUER".into());
        None
    };
    if !missing.is_empty() {
        return Err(format!("release-mac cannot start; missing:\n  - {}\nOne-time setup: {DOCS}. (--no-notarize builds a signed DMG without notarizing; --check only tests the setup.)", missing.join("\n  - ")));
    }
    Ok(MacConfig { identity: identity.unwrap(), notary })
}

/// (SHA-1, description) of each Developer ID Application identity.
fn keychain_identities() -> Vec<(String, String)> {
    let Ok(out) = Command::new("/usr/bin/security").args(["find-identity", "-v", "-p", "codesigning"]).output() else { return Vec::new() };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| l.contains("Developer ID Application"))
        .filter_map(|l| Some((l.split_whitespace().nth(1)?.to_owned(), l.split('"').nth(1)?.to_owned())))
        .collect()
}

/// Runs a tool, hiding its arguments from the output (they can name keys).
fn run(what: &str, cmd: &mut Command) -> Result<std::process::Output, String> {
    let out = cmd.output().map_err(|e| format!("{what}: cannot run: {e}"))?;
    if out.status.success() {
        Ok(out)
    } else {
        Err(format!("{what} failed: {}", String::from_utf8_lossy(if out.stderr.is_empty() { &out.stdout } else { &out.stderr }).trim()))
    }
}

fn codesign(cfg: &MacConfig, target: &Path, app: bool) -> Result<(), String> {
    let adhoc = cfg.identity == "-";
    let mut c = Command::new("/usr/bin/codesign");
    c.args(["--force", "--sign", &cfg.identity, if adhoc { "--timestamp=none" } else { "--timestamp" }]);
    if app {
        c.args(["--options", "runtime", "--entitlements"]).arg(crate::root().join("packaging/macos/Zornov.entitlements"));
    }
    run("codesign", c.arg(target)).map(|_| ())
}

/// Submits to Apple and waits; on rejection prints Apple's reasons.
fn notarize(notary: &Notary, file: &Path) -> Result<(), String> {
    let mut c = Command::new("xcrun");
    c.args(["notarytool", "submit"]).arg(file).args(["--wait", "--timeout", "45m", "--output-format", "json"]).args(notary.args());
    let out = c.output().map_err(|e| format!("notarytool: cannot run: {e}"))?;
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_default();
    let (status, id) = (v["status"].as_str().unwrap_or(""), v["id"].as_str().unwrap_or(""));
    if status == "Accepted" {
        return Ok(());
    }
    let mut why = format!("Apple's notary service answered \"{}\" for {}", if status.is_empty() { "no status" } else { status }, file.file_name().unwrap().to_string_lossy());
    if !id.is_empty() {
        let log = Command::new("xcrun").args(["notarytool", "log", id]).args(notary.args()).output();
        if let Ok(l) = log {
            why += &format!("\nits log:\n{}", String::from_utf8_lossy(&l.stdout).trim());
        }
    } else {
        why += &format!("\n{}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Err(why)
}

fn staple(file: &Path) -> Result<(), String> {
    run("stapler", Command::new("xcrun").args(["stapler", "staple"]).arg(file))?;
    run("stapler validate", Command::new("xcrun").args(["stapler", "validate"]).arg(file)).map(|_| ())
}

/// `cargo xtask release-mac [--check] [--no-notarize] [--identity ID] [--exe PATH]`
pub fn release_mac(args: &[String]) -> Result<String, String> {
    let (mut check, mut do_notarize, mut identity, mut exe) = (false, true, None, None);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--check" => check = true,
            "--no-notarize" => do_notarize = false,
            "--identity" => identity = it.next().cloned(),
            "--exe" => exe = it.next().map(PathBuf::from),
            other => return Err(format!("unknown option {other}; usage: cargo xtask release-mac [--check] [--no-notarize] [--identity ID] [--exe PATH]")),
        }
    }
    if !cfg!(target_os = "macos") {
        return Err("release-mac runs on a Mac".into());
    }
    let env = |n: &str| std::env::var(n).ok().map(|v| v.trim().to_owned()).filter(|v| !v.is_empty());
    let cfg = mac_config(&env, &keychain_identities, do_notarize, identity)?;
    for tool in ["codesign", "hdiutil", "ditto"] {
        if !["/usr/bin", "/usr/sbin"].iter().any(|d| Path::new(d).join(tool).exists()) {
            return Err(format!("{tool} is not installed"));
        }
    }
    if cfg.identity != "-" {
        // The identity must really be there, or codesign fails after a long build.
        let ids = Command::new("/usr/bin/security").args(["find-identity", "-v", "-p", "codesigning"]).output().map_err(|e| e.to_string())?;
        let text = String::from_utf8_lossy(&ids.stdout);
        if !text.contains(&cfg.identity) {
            return Err(format!("the signing identity \"{}\" is not in your keychain (security find-identity -v -p codesigning); see {DOCS}", cfg.identity));
        }
    }
    if let Some(n) = &cfg.notary {
        // Signs in without submitting anything: wrong credentials show now, not after the build.
        let mut c = Command::new("xcrun");
        c.args(["notarytool", "history", "--output-format", "json"]).args(n.args());
        run("notarytool sign-in", &mut c).map_err(|e| format!("{e}\nCheck the notarization credentials ({DOCS})."))?;
    }
    if check {
        return Ok(format!("release setup is ready: signing as \"{}\"{}", cfg.identity, if cfg.notary.is_some() { ", notarization sign-in works" } else { ", notarization skipped" }));
    }
    if cfg.identity == "-" {
        eprintln!("warning: signing ad hoc (-): fine to test the steps, but Gatekeeper rejects it on other Macs");
    }

    let notices = crate::licenses::notices()?;
    let out = release_dir().join("macos");
    crate::fresh_dir(&out)?;
    let exe = match exe {
        Some(e) => e,
        None => crate::build_app_repro()?,
    };
    let app = out.join("Zornov.app");
    let v = crate::assemble_app(&exe, crate::content_core().as_deref(), &notices, &app)?;
    codesign(&cfg, &app.join("Contents/MacOS/zornov"), true)?;
    codesign(&cfg, &app, true)?;
    run("codesign verify", Command::new("/usr/bin/codesign").args(["--verify", "--deep", "--strict"]).arg(&app))?;
    let details = run("codesign details", Command::new("/usr/bin/codesign").args(["-dvv"]).arg(&app))?;
    if !String::from_utf8_lossy(&details.stderr).contains("runtime") {
        return Err("the signed app does not have the hardened runtime".into());
    }
    if let Some(n) = &cfg.notary {
        let zip = out.join("Zornov-notarize.zip");
        run("ditto", Command::new("ditto").args(["-c", "-k", "--keepParent"]).arg(&app).arg(&zip))?;
        notarize(n, &zip)?;
        let _ = std::fs::remove_file(&zip);
        staple(&app)?;
    }

    // The update archive comes from the finished (stapled) app.
    let arch = std::env::consts::ARCH;
    let update = release_dir().join(format!("Zornov-{VERSION}-macos-{arch}.update.tar.gz"));
    repro::update_archive(&app, &update, false)?;
    let signed = sign_update(&update, &format!("macos-{arch}")).map(|_| "update archive signed").unwrap_or("update archive NOT signed (no update key; run `cargo xtask update-key`)");

    let dmg = release_dir().join(format!("Zornov-{VERSION}-macos-{arch}.dmg"));
    let stage = out.join("dmg");
    crate::fresh_dir(&stage)?;
    run("ditto", Command::new("ditto").arg(&app).arg(stage.join("Zornov.app")))?;
    #[cfg(unix)]
    std::os::unix::fs::symlink("/Applications", stage.join("Applications")).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&dmg);
    run("hdiutil", Command::new("hdiutil").args(["create", "-volname", "Zornov", "-srcfolder"]).arg(&stage).args(["-fs", "HFS+", "-format", "UDZO", "-ov"]).arg(&dmg))?;
    let _ = std::fs::remove_dir_all(&stage);
    codesign(&cfg, &dmg, false)?;
    if let Some(n) = &cfg.notary {
        notarize(n, &dmg)?;
        staple(&dmg)?;
    }
    // A copy under a name that never changes, so zornov.com can link to
    // releases/latest/download/Zornov-mac.dmg and start the download.
    std::fs::copy(&dmg, release_dir().join("Zornov-mac.dmg")).map_err(|e| e.to_string())?;
    Ok(format!(
        "{} ({v}), {}; {signed}: {}{}",
        rel(&dmg),
        if cfg.notary.is_some() { "signed, notarized and stapled" } else { "signed, not notarized" },
        rel(&update),
        if cfg.identity == "-" { " [ad hoc test build]" } else { "" }
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_of<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |k| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| (*v).to_owned())
    }

    #[test]
    fn missing_credentials_are_listed_together_without_values() {
        let none = || Vec::new();
        let e = mac_config(&env_of(&[]), &none, true, None).unwrap_err();
        assert!(e.contains("Developer ID Application") && e.contains("STUDIO_NOTARY_PROFILE") && e.contains(DOCS), "{e}");
        let e = mac_config(&env_of(&[("STUDIO_SIGN_IDENTITY", "Dev"), ("STUDIO_NOTARY_KEY", "/nope/AuthKey.p8")]), &none, true, None).unwrap_err();
        assert!(e.contains("STUDIO_NOTARY_KEY_ID") && e.contains("STUDIO_NOTARY_ISSUER") && e.contains(".p8"), "{e}");
        assert!(!e.contains("/nope/AuthKey.p8"), "paths of keys are not echoed");
    }

    #[test]
    fn a_profile_or_one_keychain_identity_is_enough() {
        let one = || vec![("ABC123".to_owned(), "Developer ID Application: Ashrox LLC (TEAM)".to_owned())];
        let c = mac_config(&env_of(&[("STUDIO_NOTARY_PROFILE", "studio")]), &one, true, None).unwrap();
        assert_eq!(c, MacConfig { identity: "ABC123".into(), notary: Some(Notary::Profile { name: "studio".into(), keychain: None }) });
        let two = || vec![("A".to_owned(), "x".to_owned()), ("B".to_owned(), "y".to_owned())];
        assert!(mac_config(&env_of(&[("STUDIO_NOTARY_PROFILE", "p")]), &two, true, None).unwrap_err().contains("several"));
        // --no-notarize needs no notary sign-in; the flag beats the keychain.
        let c = mac_config(&env_of(&[]), &two, false, Some("-".into())).unwrap();
        assert_eq!(c, MacConfig { identity: "-".into(), notary: None });
    }

    #[test]
    fn github_feeds_pin_archives_to_the_tag() {
        assert_eq!(github_base("https://github.com/o/r/releases/latest/download/feed.json", "1.2.0").as_deref(), Some("https://github.com/o/r/releases/download/v1.2.0"));
        assert_eq!(github_base("https://downloads.example.com/feed.json", "1.2.0"), None);
    }

    #[test]
    fn notary_arguments() {
        let n = Notary::ApiKey { path: "k.p8".into(), key_id: "K".into(), issuer: "I".into() };
        assert_eq!(n.args(), ["--key", "k.p8", "--key-id", "K", "--issuer", "I"]);
        assert_eq!(Notary::Profile { name: "p".into(), keychain: Some("kc".into()) }.args(), ["--keychain-profile", "p", "--keychain", "kc"]);
    }
}
