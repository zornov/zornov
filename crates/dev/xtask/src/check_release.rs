// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `cargo xtask check-release`: what must be true of a finished
//! `Zornov.app` before it ships. Every failure is collected and listed
//! together, each with how to fix it.

use crate::release::{rel, run, DOCS, VERSION};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// A stripped release binary has no local text symbols; a few the linker
/// adds are fine. An unstripped one has over a hundred thousand.
const MAX_LOCAL_SYMBOLS: usize = 100;

/// The longest example shown for a host path.
const EXAMPLE_LEN: usize = 100;

/// `cargo xtask check-release [APP] [--no-notarize]`
pub fn check_release(args: &[String]) -> Result<String, String> {
    let (mut app, mut notarized) = (None, true);
    for a in args {
        match a.as_str() {
            "--no-notarize" => notarized = false,
            o if o.starts_with("--") => return Err(format!("unknown option {o}; usage: cargo xtask check-release [APP] [--no-notarize]")),
            _ => app = Some(PathBuf::from(a)),
        }
    }
    if !cfg!(target_os = "macos") {
        return Err("check-release runs on a Mac".into());
    }
    let app = app.unwrap_or_else(|| crate::root().join("dist/release/macos/Zornov.app"));
    let exe = app.join("Contents/MacOS/zornov");
    if !exe.is_file() {
        return Err(format!("{}: no Contents/MacOS/zornov (usage: cargo xtask check-release [APP] [--no-notarize])", app.display()));
    }
    let bin = std::fs::read(&exe).map_err(|e| format!("{}: {e}", exe.display()))?;
    let public_key = studio_update::config::load().public_key;
    let checks: [(&str, Vec<String>); 6] = [
        ("host paths", host_paths(&bin)),
        ("symbols", symbols(&exe)),
        ("entitlements", entitlements(&app)),
        ("signature", signature(&app, notarized)),
        ("linked libraries", libraries(&exe)),
        ("update key and version", key_and_version(&bin, &public_key, &app)),
    ];
    let failed: Vec<String> = checks.iter().flat_map(|(_, f)| f.iter().cloned()).collect();
    if failed.is_empty() {
        return Ok(format!("{} passes all {} release checks", app.display(), checks.len()));
    }
    let passed: Vec<&str> = checks.iter().filter(|(_, f)| f.is_empty()).map(|(n, _)| *n).collect();
    Err(format!(
        "{} fails {} release check{}:\n- {}\n(passed: {}; see {DOCS})",
        rel(&app),
        failed.len(),
        if failed.len() == 1 { "" } else { "s" },
        failed.join("\n- "),
        if passed.is_empty() { "none".into() } else { passed.join(", ") }
    ))
}

// ------------------------------------------------------------- host paths

/// Every place `needles` occur in `bytes`, each shown from its start to the
/// end of the printable text around it. Longer needles win where they start
/// at the same byte, so `/Users/ash/...` is one hit, not two.
fn find_paths(bytes: &[u8], needles: &[String]) -> Vec<String> {
    let mut needles: Vec<&[u8]> = needles.iter().filter(|n| !n.is_empty()).map(|n| n.as_bytes()).collect();
    needles.sort_by_key(|n| std::cmp::Reverse(n.len()));
    let mut found = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match needles.iter().find(|n| bytes[i..].starts_with(n)) {
            Some(n) => {
                let end = bytes[i..].iter().take(EXAMPLE_LEN).position(|b| !(0x20..0x7f).contains(b)).map_or((i + EXAMPLE_LEN).min(bytes.len()), |p| i + p);
                found.push(String::from_utf8_lossy(&bytes[i..end.max(i + n.len())]).into_owned());
                i += n.len();
            }
            None => i += 1,
        }
    }
    found
}

/// A host path in the binary: the repository, the home folder, the cargo
/// folder, or any `/Users/` path.
fn host_paths(bin: &[u8]) -> Vec<String> {
    let home = std::env::var("HOME").unwrap_or_default();
    let cargo = std::env::var("CARGO_HOME").unwrap_or_default();
    let needles = [crate::root().display().to_string(), home, cargo, "/Users/".into()];
    let found = find_paths(bin, &needles);
    if found.is_empty() {
        return Vec::new();
    }
    let mut examples: Vec<&String> = Vec::new();
    for f in &found {
        if examples.len() < 5 && !examples.contains(&f) {
            examples.push(f);
        }
    }
    vec![format!(
        "the binary holds {} host path{} (e.g. {}). Build with --remap-path-prefix for the repository and cargo folders (`cargo xtask release-mac` does), and replace env!(\"CARGO_MANIFEST_DIR\") in release code, which the remap does not touch",
        found.len(),
        if found.len() == 1 { "" } else { "s" },
        examples.iter().map(|e| format!("\"{e}\"")).collect::<Vec<_>>().join(", ")
    )]
}

// ---------------------------------------------------------------- symbols

/// The local text symbols (`t`) in `nm -U` output.
fn local_symbols(nm: &str) -> usize {
    nm.lines().filter(|l| l.split_whitespace().nth(1) == Some("t")).count()
}

fn symbols(exe: &Path) -> Vec<String> {
    let out = match run("nm", Command::new("nm").arg("-U").arg(exe)) {
        Ok(o) => o,
        Err(e) => return vec![e],
    };
    let n = local_symbols(&String::from_utf8_lossy(&out.stdout));
    if n > MAX_LOCAL_SYMBOLS {
        vec![format!("the binary is not stripped: {n} local text symbols (more than {MAX_LOCAL_SYMBOLS}). Add `strip = true` to [profile.release] or run `strip -x` on it before signing")]
    } else {
        Vec::new()
    }
}

// ----------------------------------------------------------- entitlements

/// A property list as JSON, through `plutil`. `file` is a path, or `-` to
/// read `stdin`.
fn plist_json(file: &str, stdin: Option<&[u8]>) -> Result<serde_json::Value, String> {
    let mut c = Command::new("/usr/bin/plutil");
    c.args(["-convert", "json", "-o", "-", "--", file]).stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() }).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = c.spawn().map_err(|e| format!("plutil: cannot run: {e}"))?;
    if let (Some(bytes), Some(mut pipe)) = (stdin, child.stdin.take()) {
        pipe.write_all(bytes).map_err(|e| format!("plutil: {e}"))?;
    }
    let out = child.wait_with_output().map_err(|e| format!("plutil: {e}"))?;
    if !out.status.success() {
        return Err(format!("plutil failed on {file}: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    serde_json::from_slice(&out.stdout).map_err(|e| format!("plutil gave no JSON for {file}: {e}"))
}

/// What differs between the entitlements asked for and the signed ones.
fn entitlement_diff(want: &serde_json::Value, got: &serde_json::Value) -> Vec<String> {
    let (empty, w, g) = (serde_json::Map::new(), want.as_object(), got.as_object());
    let (w, g) = (w.unwrap_or(&empty), g.unwrap_or(&empty));
    let mut diff = Vec::new();
    for (k, v) in w {
        match g.get(k) {
            None => diff.push(format!("{k} is missing")),
            Some(x) if x != v => diff.push(format!("{k} is {x}, not {v}")),
            Some(_) => {}
        }
    }
    diff.extend(g.keys().filter(|k| !w.contains_key(*k)).map(|k| format!("{k} is extra")));
    diff
}

fn entitlements(app: &Path) -> Vec<String> {
    let file = crate::root().join("packaging/macos/Zornov.entitlements");
    let result = (|| {
        let want = plist_json(&file.to_string_lossy(), None)?;
        let signed = run("codesign entitlements", Command::new("/usr/bin/codesign").args(["-d", "--entitlements", ":-"]).arg(app))?;
        if signed.stdout.is_empty() {
            return Ok(vec!["the app is signed with no entitlements".to_owned()]);
        }
        let got = plist_json("-", Some(&signed.stdout))?;
        Ok::<_, String>(entitlement_diff(&want, &got))
    })();
    match result {
        Ok(diff) if diff.is_empty() => Vec::new(),
        Ok(diff) => vec![format!("the entitlements differ from packaging/macos/Zornov.entitlements: {}. Sign with `--entitlements` from that file (`cargo xtask release-mac` does)", diff.join("; "))],
        Err(e) => vec![e],
    }
}

// -------------------------------------------------------------- signature

/// What is wrong in `codesign -dvv` output (it prints on stderr).
fn signature_problems(details: &str) -> Vec<&'static str> {
    let mut bad = Vec::new();
    if !details.lines().any(|l| l.starts_with("Authority=Developer ID Application:")) {
        bad.push("not signed with a Developer ID Application identity (ad hoc or another authority)");
    }
    if !details.lines().any(|l| l.starts_with("Timestamp=")) {
        bad.push("no secure timestamp (sign with `--timestamp`)");
    }
    if !details.lines().any(|l| l.contains("flags=") && l.contains("runtime")) {
        bad.push("no hardened runtime (sign with `--options runtime`)");
    }
    bad
}

fn signature(app: &Path, notarized: bool) -> Vec<String> {
    let mut fails = Vec::new();
    match run("codesign details", Command::new("/usr/bin/codesign").arg("-dvv").arg(app)) {
        Ok(o) => fails.extend(signature_problems(&String::from_utf8_lossy(&o.stderr)).into_iter().map(|p| format!("signature: {p}"))),
        Err(e) => fails.push(e),
    }
    if let Err(e) = run("codesign verify", Command::new("/usr/bin/codesign").args(["--verify", "--deep", "--strict"]).arg(app)) {
        fails.push(format!("signature: {e}. Sign the binary, then the app, after the last change to the bundle"));
    }
    if notarized {
        if let Err(e) = run("stapler validate", Command::new("xcrun").args(["stapler", "validate"]).arg(app)) {
            fails.push(format!("signature: {e}. Notarize and staple the app (or pass --no-notarize)"));
        }
        if let Err(e) = run("spctl", Command::new("/usr/sbin/spctl").args(["-a", "-vv"]).arg(app)) {
            fails.push(format!("signature: Gatekeeper does not accept the app ({e}). Notarize and staple it (or pass --no-notarize)"));
        }
    }
    fails
}

// -------------------------------------------------------------- libraries

/// The libraries in `otool -L` output outside `/usr/lib/` and `/System/`
/// (its first line names the file itself).
fn foreign_libraries(otool: &str) -> Vec<String> {
    otool.lines().skip(1).filter_map(|l| l.split_whitespace().next()).filter(|p| !p.starts_with("/usr/lib/") && !p.starts_with("/System/")).map(str::to_owned).collect()
}

fn libraries(exe: &Path) -> Vec<String> {
    match run("otool", Command::new("otool").arg("-L").arg(exe)) {
        Ok(o) => {
            let bad = foreign_libraries(&String::from_utf8_lossy(&o.stdout));
            if bad.is_empty() {
                Vec::new()
            } else {
                vec![format!("the binary links libraries outside /usr/lib and /System: {}. Link them statically", bad.join(", "))]
            }
        }
        Err(e) => vec![e],
    }
}

// ------------------------------------------------- update key and version

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && hay.windows(needle.len()).any(|w| w == needle)
}

/// The update public key is in the binary (hex, either case).
fn has_key(bin: &[u8], key: &str) -> bool {
    contains(bin, key.to_ascii_lowercase().as_bytes()) || contains(bin, key.to_ascii_uppercase().as_bytes())
}

fn key_and_version(bin: &[u8], public_key: &str, app: &Path) -> Vec<String> {
    let mut fails = Vec::new();
    if public_key.is_empty() {
        fails.push("packaging/release.conf has no public_key, so the app refuses every update (`cargo xtask update-key`)".to_owned());
    } else if !has_key(bin, public_key) {
        fails.push("the update public_key from packaging/release.conf is not in the binary: it was built with another key or config. Rebuild".to_owned());
    }
    match plist_json(&app.join("Contents/Info.plist").to_string_lossy(), None) {
        Ok(p) => {
            let v = p["CFBundleShortVersionString"].as_str().unwrap_or("");
            if v != VERSION {
                fails.push(format!("Info.plist CFBundleShortVersionString is \"{v}\", not {VERSION}. Rebuild the app from this version"));
            }
        }
        Err(e) => fails.push(e),
    }
    fails
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn host_paths_are_counted_once_and_shown_whole() {
        let bin = b"\0\0/Users/ash/Projects/zornov/app/src/x.rs\0abc/Users/ash/.cargo/registry/y.rs\x01/usr/lib/ok";
        let found = find_paths(bin, &s(&["/Users/ash/Projects/zornov", "/Users/ash", "", "/Users/"]));
        assert_eq!(found, ["/Users/ash/Projects/zornov/app/src/x.rs", "/Users/ash/.cargo/registry/y.rs"]);
        assert!(find_paths(b"/zornov/app/src/x.rs", &s(&["/Users/"])).is_empty());
    }

    #[test]
    fn long_paths_are_cut() {
        let mut bin = b"/Users/".to_vec();
        bin.extend(std::iter::repeat_n(b'a', 500));
        assert_eq!(find_paths(&bin, &s(&["/Users/"]))[0].len(), EXAMPLE_LEN);
    }

    #[test]
    fn only_local_text_symbols_count() {
        let nm = "0000000100000000 T __mh_execute_header\n0000000100001000 t __ZN4core3fmt\n0000000100002000 t _foo\n0000000100003000 s _data\n0000000100004000 S _g\n";
        assert_eq!(local_symbols(nm), 2);
        assert_eq!(local_symbols(""), 0);
    }

    #[test]
    fn entitlements_must_match_exactly() {
        use serde_json::json;
        let want = json!({"a": true, "b": true});
        assert!(entitlement_diff(&want, &json!({"b": true, "a": true})).is_empty());
        assert_eq!(entitlement_diff(&want, &json!({"a": true, "c": true})), ["b is missing", "c is extra"]);
        assert_eq!(entitlement_diff(&want, &json!({"a": false, "b": true})), ["a is false, not true"]);
    }

    #[test]
    fn signature_details_are_judged() {
        let good = "Authority=Developer ID Application: Ashrox LLC (ABCDE12345)\nTimestamp=Oct 1, 2026 at 10:00:00\nCodeDirectory v=20500 size=1 flags=0x10000(runtime) hashes=1\n";
        assert!(signature_problems(good).is_empty());
        let adhoc = "CodeDirectory v=20400 size=1 flags=0x2(adhoc) hashes=1\nSigned Time=Oct 1\nSignature=adhoc\n";
        assert_eq!(signature_problems(adhoc).len(), 3);
        assert_eq!(signature_problems(&good.replace("Timestamp=", "Signed Time=")).len(), 1);
    }

    #[test]
    fn only_system_libraries_are_allowed() {
        let ok = "/x/zornov:\n\t/usr/lib/libSystem.B.dylib (compatibility version 1.0.0)\n\t/System/Library/Frameworks/Metal.framework/Versions/A/Metal (compatibility version 1.0.0)\n";
        assert!(foreign_libraries(ok).is_empty());
        let bad = format!("{ok}\t/opt/homebrew/lib/libx.dylib (compatibility version 1.0.0)\n\t@rpath/liby.dylib (compatibility version 1.0.0)\n");
        assert_eq!(foreign_libraries(&bad), ["/opt/homebrew/lib/libx.dylib", "@rpath/liby.dylib"]);
    }

    #[test]
    fn the_key_is_found_in_either_case() {
        let key = "55cc3001f11c6d06";
        assert!(has_key(b"xx55cc3001f11c6d06yy", key));
        assert!(has_key(b"xx55CC3001F11C6D06yy", key));
        assert!(!has_key(b"xx55cc3001f11c6d07yy", key));
        assert!(!has_key(b"anything", ""));
    }
}
