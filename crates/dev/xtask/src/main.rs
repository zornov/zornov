// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Studio's build and packaging tasks, in Rust (they were shell, PowerShell
//! and Python scripts in `tools/`):
//!
//! ```text
//! cargo xtask bundle            dist/Studio.app (macOS)
//! cargo xtask package-linux     dist/studio-linux-<arch>.tar.gz
//! cargo xtask package-windows   dist\studio-windows-<arch>.zip
//! cargo xtask sbs REF SHOT OUT [WIDTH]   a reference above a Studio shot
//! cargo xtask signing           the local signing identity (made once)
//! cargo xtask release-mac       Developer ID signed, notarized DMG (docs/releasing.md)
//! cargo xtask package-deb       dist/release/studio_<ver>_<arch>.deb
//! cargo xtask package-appimage  dist/release/Studio-<ver>-<arch>.AppImage
//! cargo xtask package-msi       dist/release/Studio-<ver>-windows-<arch>.msi
//! cargo xtask update-key        the update signing key (made once)
//! cargo xtask feed              dist/release/feed.json, signed
//! cargo xtask headers [--check] the copyright header on every Studio file
//! cargo xtask licenses          dist/THIRD-PARTY-LICENSES.txt
//! cargo xtask assets-lock       assets.lock from every committed asset
//! cargo xtask assets-check      fails on an asset added, changed or gone
//!                               without updating assets.lock
//! ```
//!
//! `CARGO_TARGET_DIR` picks the build folder (default `target`);
//! `STUDIO_CONTENT_CORE` the core content library (default
//! `content-build/core` in the repository).

mod assets;
mod licenses;
mod pkg;
mod release;
mod repro;

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let r = match args.first().map(String::as_str) {
        Some("bundle") => bundle(),
        Some("package-linux") => package_linux(),
        Some("package-windows") => package_windows(),
        Some("sbs") => sbs(&args[1..]),
        Some("release-mac") => release::release_mac(&args[1..]),
        Some("package-deb") => pkg::package_deb(),
        Some("package-appimage") => pkg::package_appimage(),
        Some("package-msi") => pkg::package_msi(),
        Some("update-key") => release::update_key_cmd(),
        Some("feed") => release::feed_cmd(&args[1..]),
        Some("licenses") => licenses::licenses_cmd(),
        Some("assets-lock") => assets::lock_cmd(&root()),
        Some("assets-check") => assets::check_cmd(&root()),
        Some("headers") => headers(args.get(1).is_some_and(|a| a == "--check")),
        #[cfg(target_os = "macos")]
        Some("signing") => signing::identity().map(|s| format!("signing with \"{}\" ({}) from {}", signing::NAME, s.hash, signing::keychain().display())),
        _ => Err("usage: cargo xtask bundle | release-mac | package-linux | package-deb | package-appimage | package-windows | package-msi | update-key | feed | licenses | assets-lock | assets-check | signing | headers [--check] | sbs REF SHOT OUT [WIDTH]".into()),
    };
    match r {
        Ok(out) => {
            println!("{out}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("xtask: {e}");
            ExitCode::FAILURE
        }
    }
}

/// The repository root (this crate is `crates/dev/xtask`).
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).ancestors().nth(3).expect("repository root").to_path_buf()
}

fn target_dir() -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR").map(PathBuf::from).map(|p| if p.is_absolute() { p } else { root().join(p) }).unwrap_or_else(|| root().join("target"))
}

/// `cargo build --release -p studio-app`, into the target folder.
fn build_app() -> Result<PathBuf, String> {
    build(false)
}

/// The same for a release: `--locked`, and the build and cargo folders
/// replaced by fixed names in the binary, so the same sources give the same
/// bytes on any machine.
fn build_app_repro() -> Result<PathBuf, String> {
    build(true)
}

fn build(repro: bool) -> Result<PathBuf, String> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut c = Command::new(cargo);
    c.current_dir(root()).env("CARGO_TARGET_DIR", target_dir()).args(["build", "--release", "-p", "studio-app"]);
    if repro {
        let mut flags = std::env::var("RUSTFLAGS").unwrap_or_default();
        flags += &format!(" --remap-path-prefix={}=/studio", root().display());
        if let Some(home) = std::env::var_os("CARGO_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(|h| PathBuf::from(h).join(".cargo"))) {
            flags += &format!(" --remap-path-prefix={}=/cargo", home.display());
        }
        c.env("RUSTFLAGS", flags.trim()).env("SOURCE_DATE_EPOCH", repro::epoch().to_string()).arg("--locked");
    }
    let ok = c.status().map_err(|e| format!("cannot run cargo: {e}"))?.success();
    if !ok {
        return Err("the build failed".into());
    }
    let exe = target_dir().join("release").join(if cfg!(windows) { "studio.exe" } else { "studio" });
    exe.is_file().then_some(exe).ok_or_else(|| "the build made no studio binary".into())
}

/// The core content library, if it is there (the Assets and Audio panels
/// are empty without it).
fn content_core() -> Option<PathBuf> {
    let core = std::env::var_os("STUDIO_CONTENT_CORE").map(PathBuf::from).unwrap_or_else(|| root().join("content-build/core"));
    if core.join("index.json").is_file() {
        Some(core)
    } else {
        eprintln!("warning: no content library at {}; the Assets and Audio panels will be empty", core.display());
        None
    }
}

/// Copies a folder recursively (files replaced, links followed).
fn copy_dir(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|e| format!("{}: {e}", to.display()))?;
    for e in std::fs::read_dir(from).map_err(|e| format!("{}: {e}", from.display()))? {
        let e = e.map_err(|e| e.to_string())?;
        let (src, dst) = (e.path(), to.join(e.file_name()));
        if src.is_dir() {
            copy_dir(&src, &dst)?;
        } else {
            std::fs::copy(&src, &dst).map_err(|e| format!("{}: {e}", src.display()))?;
        }
    }
    Ok(())
}

/// Writes the licence notices into `dir`.
fn write_notices(notices: &[(String, Vec<u8>)], dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for (name, data) in notices {
        std::fs::write(dir.join(name), data).map_err(|e| format!("{}: {e}", dir.join(name).display()))?;
    }
    Ok(())
}

fn fresh_dir(p: &Path) -> Result<(), String> {
    if p.exists() {
        std::fs::remove_dir_all(p).map_err(|e| format!("cannot clear {}: {e}", p.display()))?;
    }
    std::fs::create_dir_all(p).map_err(|e| format!("{}: {e}", p.display()))
}

/// `git describe --always --dirty`, or "dev".
fn version() -> String {
    Command::new("git").current_dir(root()).args(["describe", "--always", "--dirty"]).output().ok().filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned()).filter(|v| !v.is_empty()).unwrap_or_else(|| "dev".into())
}

/// dist/Studio.app: the release binary, Info.plist and the core content
/// library, signed for this Mac.
fn bundle() -> Result<String, String> {
    let exe = build_app()?;
    let app = root().join("dist/Studio.app");
    let v = assemble_app(&exe, content_core().as_deref(), &licenses::notices()?, &app)?;
    sign(&app)?;
    Ok(format!("dist/Studio.app ({v})"))
}

/// The unsigned `.app` made from `exe` at `app`, with the licence notices
/// in `Contents/Resources/Licenses`; its build label.
fn assemble_app(exe: &Path, content: Option<&Path>, notices: &[(String, Vec<u8>)], app: &Path) -> Result<String, String> {
    fresh_dir(app)?;
    let macos = app.join("Contents/MacOS");
    std::fs::create_dir_all(&macos).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(app.join("Contents/Resources")).map_err(|e| e.to_string())?;
    std::fs::copy(exe, macos.join("studio")).map_err(|e| format!("cannot copy the binary: {e}"))?;
    write_notices(notices, &app.join("Contents/Resources/Licenses"))?;
    std::fs::copy(root().join("packaging/brand/zornov.icns"), app.join("Contents/Resources/AppIcon.icns")).map_err(|e| format!("cannot copy the app icon: {e}"))?;
    if let Some(core) = content {
        copy_dir(core, &app.join("Contents/content-build/core"))?;
    }
    let v = version();
    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>Studio</string>
  <key>CFBundleDisplayName</key><string>Studio</string>
  <key>CFBundleIdentifier</key><string>org.studio.editor</string>
  <key>CFBundleExecutable</key><string>studio</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>{short}</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.video</string>
  <key>CFBundleVersion</key><string>{v}</string>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSMicrophoneUsageDescription</key><string>Studio records voice-overs and sound from your microphone when you press Record.</string>
  <key>NSCameraUsageDescription</key><string>Studio records from your camera when you press Record.</string>
</dict></plist>
"#,
        short = release::VERSION
    );
    std::fs::write(app.join("Contents/Info.plist"), plist).map_err(|e| e.to_string())?;
    Ok(v)
}

/// Signs the app with the local signing identity, the same one every
/// build: macOS then knows a rebuilt Studio as the same app, so its
/// Keychain items (API keys) open without asking again. Ad hoc (a new
/// identity each build) only when the identity cannot be made.
#[cfg(target_os = "macos")]
fn sign(app: &Path) -> Result<(), String> {
    let codesign = |args: &[&str]| Command::new("codesign").args(args).arg(app).output().map_err(|e| format!("cannot run codesign: {e}"));
    match signing::identity() {
        Ok(id) => {
            let kc = signing::keychain();
            let out = codesign(&["--force", "--deep", "--keychain", &kc.to_string_lossy(), "--sign", &id.hash])?;
            if !out.status.success() {
                return Err(format!("codesign failed: {}", String::from_utf8_lossy(&out.stderr).trim()));
            }
        }
        Err(e) => {
            eprintln!("warning: signing ad hoc ({e}); saved API keys may ask for Keychain access after this build");
            let _ = codesign(&["--force", "--deep", "--sign", "-"]);
        }
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn sign(_: &Path) -> Result<(), String> {
    Ok(())
}

fn arch() -> &'static str {
    match std::env::consts::ARCH {
        "aarch64" if cfg!(windows) => "arm64",
        "x86_64" if cfg!(windows) => "x64",
        a => a,
    }
}

/// dist/studio-linux-<arch>.tar.gz: binary, desktop entry and content (and
/// its signed update archive, when there is an update key).
fn package_linux() -> Result<String, String> {
    let exe = build_app_repro()?;
    let dist = root().join("dist");
    std::fs::create_dir_all(&dist).map_err(|e| e.to_string())?;
    pkg::portable_linux_from(&exe, content_core().as_deref(), &licenses::notices()?, &dist, true)
}

/// dist\studio-windows-<arch>.zip: studio.exe with the content beside it.
fn package_windows() -> Result<String, String> {
    let exe = build_app_repro()?;
    let dist = root().join("dist");
    std::fs::create_dir_all(&dist).map_err(|e| e.to_string())?;
    pkg::portable_windows_from(&exe, content_core().as_deref(), &licenses::notices()?, &dist, true)
}

/// The header every source file made for Studio starts with.
const HEADER: &str = "// Copyright 2026 Ashrox LLC and contributors\n// SPDX-License-Identifier: AGPL-3.0-or-later\n";

/// Whether `file` is Studio's own code (not vendored from another
/// project): its crate is licensed AGPL-3.0-or-later (or the workspace's), and it
/// does not say it was vendored.
fn ours(file: &Path, text: &str) -> bool {
    let head: String = text.lines().take(20).collect::<Vec<_>>().join("\n");
    if head.contains("Vendored from") || head.contains("adapted from") || head.contains("Adapted from") {
        return false;
    }
    let Some(manifest) = file.ancestors().skip(1).map(|d| d.join("Cargo.toml")).find(|m| m.is_file()) else { return true };
    let toml = std::fs::read_to_string(manifest).unwrap_or_default();
    match toml.lines().find_map(|l| l.trim().strip_prefix("license").map(str::trim)) {
        Some(rest) if rest.starts_with(".workspace") => true,
        Some(rest) => rest.trim_start_matches('=').trim().trim_matches('"') == "AGPL-3.0-or-later",
        None => true,
    }
}

/// Adds the copyright header to every Rust and WGSL file of Studio's own
/// that lacks it (`check`: only lists them, failing if any).
fn headers(check: bool) -> Result<String, String> {
    let out = Command::new("git").current_dir(root()).args(["ls-files", "-co", "--exclude-standard", "--", "crates", "tools", "fuzz"]).output().map_err(|e| format!("cannot run git: {e}"))?;
    let files: Vec<PathBuf> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|f| (f.ends_with(".rs") || f.ends_with(".wgsl")) && !f.split('/').any(|part| part == "target" || part.starts_with("target-")))
        .map(|f| root().join(f))
        .collect();
    let (mut missing, mut skipped) = (Vec::new(), 0);
    for f in &files {
        let Ok(text) = std::fs::read_to_string(f) else { continue };
        if text.lines().take(5).any(|l| l.contains("SPDX-License-Identifier")) {
            continue;
        }
        if !ours(f, &text) {
            skipped += 1;
            continue;
        }
        missing.push(f.clone());
        if !check {
            std::fs::write(f, format!("{HEADER}\n{text}")).map_err(|e| format!("{}: {e}", f.display()))?;
        }
    }
    let rel = |p: &PathBuf| p.strip_prefix(root()).unwrap_or(p).display().to_string();
    if check {
        if missing.is_empty() {
            Ok(format!("all {} Studio files have the header ({skipped} third-party files keep their own)", files.len() - skipped))
        } else {
            Err(format!("{} files lack the copyright header:\n{}", missing.len(), missing.iter().map(rel).collect::<Vec<_>>().join("\n")))
        }
    } else {
        Ok(format!("added the header to {} files ({skipped} third-party files keep their own)", missing.len()))
    }
}

/// A reference capture above a Studio shot, both `width` wide with an
/// 8-pixel magenta gap, for visual review (PNG or JPEG in, PNG out).
fn sbs(args: &[String]) -> Result<String, String> {
    let [reference, shot, out] = [0, 1, 2].map(|i| args.get(i).cloned());
    let (Some(reference), Some(shot), Some(out)) = (reference, shot, out) else {
        return Err("usage: cargo xtask sbs REF SHOT OUT [WIDTH]".into());
    };
    let width: u32 = args.get(3).and_then(|w| w.parse().ok()).unwrap_or(1400);
    let read = |p: &str| {
        let path = Path::new(p);
        let png = std::fs::read(path).map(|b| b.starts_with(b"\x89PNG")).unwrap_or(false);
        if png { studio_media::png::read(path) } else { studio_media::jpeg::read(path) }
    };
    let a = scaled(&read(&reference)?, width);
    let b = scaled(&read(&shot)?, width);
    let gap = 8;
    let h = a.height + gap + b.height;
    let mut px = Vec::with_capacity((width * h * 4) as usize);
    px.extend_from_slice(&a.pixels);
    px.extend((0..width * gap).flat_map(|_| [255u8, 0, 255, 255]));
    px.extend_from_slice(&b.pixels);
    let img = studio_media::RgbaImage { width, height: h, pixels: px };
    std::fs::write(&out, studio_media::png::encode(&img)).map_err(|e| format!("{out}: {e}"))?;
    Ok(out)
}

/// `img` resized to `width` (box filter down, bilinear up), opaque.
fn scaled(img: &studio_media::RgbaImage, width: u32) -> studio_media::RgbaImage {
    let (sw, sh) = (img.width.max(1) as f64, img.height.max(1) as f64);
    let height = ((sh * width as f64 / sw).round() as u32).max(1);
    let (fx, fy) = (sw / width as f64, sh / height as f64);
    let at = |x: i64, y: i64, c: usize| img.pixels[((y.clamp(0, sh as i64 - 1) * sw as i64 + x.clamp(0, sw as i64 - 1)) * 4) as usize + c] as f64;
    let mut px = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            let (x0, y0) = (x as f64 * fx, y as f64 * fy);
            for c in 0..3 {
                let v = if fx >= 1.0 && fy >= 1.0 {
                    // Average the source box.
                    let (xa, xb, ya, yb) = (x0 as i64, ((x0 + fx).ceil() as i64).max(x0 as i64 + 1), y0 as i64, ((y0 + fy).ceil() as i64).max(y0 as i64 + 1));
                    let mut s = 0.0;
                    for yy in ya..yb {
                        for xx in xa..xb {
                            s += at(xx, yy, c);
                        }
                    }
                    s / ((xb - xa) * (yb - ya)) as f64
                } else {
                    let (cx, cy) = (x0 + fx / 2.0 - 0.5, y0 + fy / 2.0 - 0.5);
                    let (ix, iy) = (cx.floor(), cy.floor());
                    let (tx, ty) = (cx - ix, cy - iy);
                    let (ix, iy) = (ix as i64, iy as i64);
                    (at(ix, iy, c) * (1.0 - tx) + at(ix + 1, iy, c) * tx) * (1.0 - ty) + (at(ix, iy + 1, c) * (1.0 - tx) + at(ix + 1, iy + 1, c) * tx) * ty
                };
                px.push(v.round().clamp(0.0, 255.0) as u8);
            }
            px.push(255);
        }
    }
    studio_media::RgbaImage { width, height, pixels: px }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vendored_files_are_not_claimed() {
        let dir = std::env::temp_dir().join(format!("xtask-hdr-{}", std::process::id()));
        let (ours_dir, theirs_dir) = (dir.join("ours/src"), dir.join("theirs/src"));
        std::fs::create_dir_all(&ours_dir).unwrap();
        std::fs::create_dir_all(&theirs_dir).unwrap();
        std::fs::write(dir.join("ours/Cargo.toml"), "[package]\nlicense.workspace = true\n").unwrap();
        std::fs::write(dir.join("theirs/Cargo.toml"), "[package]\nlicense = \"MIT OR Apache-2.0\"\n").unwrap();
        assert!(ours(&ours_dir.join("a.rs"), "//! Studio code"));
        assert!(!ours(&ours_dir.join("b.rs"), "//! Vendored from FilmCraft"));
        assert!(!ours(&theirs_dir.join("c.rs"), "//! codec"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sbs_stacks_two_pictures_at_one_width() {
        let dir = std::env::temp_dir().join(format!("xtask-sbs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let red = studio_media::RgbaImage { width: 200, height: 100, pixels: [255, 0, 0, 255].repeat(200 * 100) };
        let blue = studio_media::RgbaImage { width: 50, height: 50, pixels: [0, 0, 255, 255].repeat(50 * 50) };
        let (a, b, out) = (dir.join("a.png"), dir.join("b.png"), dir.join("out.png"));
        std::fs::write(&a, studio_media::png::encode(&red)).unwrap();
        std::fs::write(&b, studio_media::png::encode(&blue)).unwrap();
        sbs(&[a, b, out.clone()].map(|p| p.display().to_string()).into_iter().chain(["100".to_owned()]).collect::<Vec<_>>()).unwrap();
        let img = studio_media::png::read(&out).unwrap();
        assert_eq!((img.width, img.height), (100, 50 + 8 + 100));
        assert_eq!(&img.pixels[..4], &[255, 0, 0, 255]);
        assert_eq!(&img.pixels[(54 * 100 * 4)..(54 * 100 * 4 + 4)], &[255, 0, 255, 255]);
        assert_eq!(&img.pixels[(100 * 100 * 4)..(100 * 100 * 4 + 4)], &[0, 0, 255, 255]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// The local signing identity: a self-signed code-signing certificate made
/// here, in Rust, kept in its own keychain under Studio's Application
/// Support folder. The login keychain, the keychain search list and trust
/// settings are not touched (codesign uses the certificate untrusted; the
/// signature names its exact certificate, which is what keeps the app's
/// identity the same from one build to the next).
#[cfg(target_os = "macos")]
mod signing {
    use security_framework::os::macos::keychain::{CreateOptions, KeychainSettings, SecKeychain};
    use std::io::Read;
    use std::path::PathBuf;
    use std::process::Command;

    pub const NAME: &str = "Studio Local Signing";

    pub struct Identity {
        /// SHA-1 of the certificate, as codesign takes it.
        pub hash: String,
    }

    fn dir() -> PathBuf {
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
        home.join("Library/Application Support/Studio/signing")
    }

    pub fn keychain() -> PathBuf {
        dir().join("studio-signing.keychain-db")
    }

    fn random_hex(bytes: usize) -> Result<String, String> {
        let mut b = vec![0u8; bytes];
        std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut b)).map_err(|e| format!("no randomness: {e}"))?;
        Ok(b.iter().map(|x| format!("{x:02x}")).collect())
    }

    fn write_private(path: &std::path::Path, bytes: &[u8]) -> Result<(), String> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        f.write_all(bytes).map_err(|e| e.to_string())
    }

    /// The identity, made (once) if it is not there yet; its keychain is
    /// unlocked for codesign.
    pub fn identity() -> Result<Identity, String> {
        let (kc, pw_file) = (keychain(), dir().join("keychain-password"));
        if !kc.is_file() || !pw_file.is_file() {
            create(&kc, &pw_file)?;
        }
        let pw = std::fs::read_to_string(&pw_file).map_err(|e| e.to_string())?;
        SecKeychain::open(&kc).and_then(|mut k| k.unlock(Some(pw.trim()))).map_err(|e| format!("cannot unlock the signing keychain: {e}"))?;
        let out = Command::new("/usr/bin/security").args(["find-identity", "-p", "codesigning"]).arg(&kc).output().map_err(|e| e.to_string())?;
        let text = String::from_utf8_lossy(&out.stdout);
        let hash = text.lines().find(|l| l.contains(&format!("\"{NAME}\""))).and_then(|l| l.split_whitespace().nth(1)).map(str::to_owned);
        hash.map(|hash| Identity { hash }).ok_or_else(|| format!("no \"{NAME}\" identity in {}", kc.display()))
    }

    fn create(kc: &std::path::Path, pw_file: &std::path::Path) -> Result<(), String> {
        std::fs::create_dir_all(dir()).map_err(|e| e.to_string())?;
        let _ = std::fs::remove_file(kc);
        // The keychain, with a random password kept beside it (readable by
        // this user only); never locks by itself.
        let pw = random_hex(24)?;
        write_private(pw_file, pw.as_bytes())?;
        let mut k = CreateOptions::new().password(&pw).create(kc).map_err(|e| format!("cannot make the signing keychain: {e}"))?;
        let mut settings = KeychainSettings::new();
        settings.set_lock_on_sleep(false);
        settings.set_lock_interval(None);
        k.set_settings(&settings).map_err(|e| e.to_string())?;
        // A code-signing certificate (P-256), valid for twenty years.
        let mut params = rcgen::CertificateParams::new(Vec::<String>::new()).map_err(|e| e.to_string())?;
        params.distinguished_name.push(rcgen::DnType::CommonName, NAME);
        params.distinguished_name.push(rcgen::DnType::OrganizationName, "Studio (this Mac only)");
        params.key_usages = vec![rcgen::KeyUsagePurpose::DigitalSignature];
        params.extended_key_usages = vec![rcgen::ExtendedKeyUsagePurpose::CodeSigning];
        params.is_ca = rcgen::IsCa::ExplicitNoCa;
        params.not_before = rcgen::date_time_ymd(2026, 1, 1);
        params.not_after = rcgen::date_time_ymd(2046, 1, 1);
        let key = rcgen::KeyPair::generate().map_err(|e| e.to_string())?;
        let cert = params.self_signed(&key).map_err(|e| e.to_string())?;
        // Into the keychain as one identity, usable by codesign without a
        // prompt.
        let p12_pw = random_hex(16)?;
        let pfx = p12::PFX::new(cert.der(), &key.serialize_der(), None, &p12_pw, NAME).ok_or("cannot pack the identity")?;
        let p12_file = dir().join("identity.p12");
        write_private(&p12_file, &pfx.to_der())?;
        let import = Command::new("/usr/bin/security").arg("import").arg(&p12_file).arg("-k").arg(kc).args(["-P", &p12_pw, "-T", "/usr/bin/codesign"]).output().map_err(|e| e.to_string());
        let _ = std::fs::remove_file(&p12_file);
        let import = import?;
        if !import.status.success() {
            return Err(format!("cannot import the identity: {}", String::from_utf8_lossy(&import.stderr).trim()));
        }
        let part = Command::new("/usr/bin/security").args(["set-key-partition-list", "-S", "apple-tool:,apple:", "-s", "-k", &pw]).arg(kc).output().map_err(|e| e.to_string())?;
        if !part.status.success() {
            return Err(format!("cannot open the key to codesign: {}", String::from_utf8_lossy(&part.stderr).trim()));
        }
        Ok(())
    }
}
