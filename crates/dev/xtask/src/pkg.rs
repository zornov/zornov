// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Linux and Windows packages: `.deb`, AppImage and MSI, plus the portable
//! archives and their update archives. Inputs are sorted and dated
//! `SOURCE_DATE_EPOCH`, so the same sources give the same package.

use crate::licenses::Notices;
use crate::release::{VERSION, release_dir, rel, sign_update};
use crate::repro::{self, Item, Made};
use std::path::Path;
use std::process::Command;

fn deb_arch() -> Result<&'static str, String> {
    match std::env::consts::ARCH {
        "x86_64" => Ok("amd64"),
        "aarch64" => Ok("arm64"),
        a => Err(format!("no .deb architecture for {a}")),
    }
}

/// The content library as archive items under `content-build/core/`.
fn content_items(content: Option<&Path>) -> Result<Vec<Item>, String> {
    let Some(core) = content else { return Ok(Vec::new()) };
    Ok(repro::collect(core)?.into_iter().map(|i| Item { rel: format!("content-build/core/{}", i.rel), ..i }).collect())
}

fn desktop_file() -> Result<Vec<u8>, String> {
    let p = crate::root().join("packaging/linux/zornov.desktop");
    std::fs::read(&p).map_err(|e| format!("{}: {e}", p.display()))
}

/// The app icon at 256 pixels (`packaging/brand`), for Linux packages.
pub fn icon_png() -> Vec<u8> {
    include_bytes!("../../../../packaging/brand/hicolor/256x256/apps/zornov.png").to_vec()
}

// ------------------------------------------------------------------ .deb

fn ar(members: &[(&str, Vec<u8>)], mtime: u64) -> Vec<u8> {
    let mut out = b"!<arch>\n".to_vec();
    for (name, data) in members {
        out.extend(format!("{:<16}{:<12}{:<6}{:<6}{:<8}{:<10}`\n", name, mtime, 0, 0, "100644", data.len()).bytes());
        out.extend(data);
        if data.len() % 2 == 1 {
            out.push(b'\n');
        }
    }
    out
}

fn gz_tar(items: &[Item], made: &[Made], prefix: Option<&str>, tmp: &Path) -> Result<Vec<u8>, String> {
    repro::tar_gz(tmp, prefix, items, made)?;
    let b = std::fs::read(tmp).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(tmp);
    Ok(b)
}

/// Where Linux packages put the licence notices.
pub const LINUX_DOC: &str = "usr/share/doc/zornov";

/// Builds `out` (a `.deb`) from the release binary, content and licence
/// notices (in `/usr/share/doc/zornov`).
pub fn deb_from(exe: &Path, content: Option<&Path>, notices: &Notices, out: &Path) -> Result<(), String> {
    let arch = deb_arch()?;
    let mut items = content_items(content)?;
    items.push(Item { rel: "zornov".into(), src: exe.to_owned(), exec: true });
    let items: Vec<Item> = items.into_iter().map(|i| Item { rel: format!("usr/lib/zornov/{}", i.rel), ..i }).collect();
    let size_kb = (items.iter().map(|i| std::fs::metadata(&i.src).map(|m| m.len()).unwrap_or(0)).sum::<u64>() + notices.iter().map(|n| n.1.len() as u64).sum::<u64>() + 1023) / 1024;
    let mut made = vec![
        Made::link("usr/bin/zornov", "../lib/zornov/zornov"),
        Made::file("usr/share/applications/zornov.desktop", desktop_file()?, false),
        Made::file("usr/share/icons/hicolor/256x256/apps/zornov.png", icon_png(), false),
    ];
    made.extend(notices.iter().map(|(name, data)| Made::file(&format!("{LINUX_DOC}/{name}"), data.clone(), false)));
    let tmp = out.with_extension("tmp");
    let data = gz_tar(&items, &made, Some("."), &tmp)?;
    let control = format!(
        "Package: zornov\nVersion: {VERSION}\nSection: video\nPriority: optional\nArchitecture: {arch}\nInstalled-Size: {size_kb}\nMaintainer: Ashrox LLC <noreply@ashrox.invalid>\nDepends: libc6 (>= 2.31), libasound2t64 | libasound2, libvulkan1\nDescription: Zornov video editor\n A video editor with an AI editor, 3D, effects and export, in Rust.\n"
    );
    let ctl = gz_tar(&[], &[Made::file("control", control.into_bytes(), false)], Some("."), &tmp)?;
    let deb = ar(&[("debian-binary", b"2.0\n".to_vec()), ("control.tar.gz", ctl), ("data.tar.gz", data)], repro::epoch());
    std::fs::write(out, deb).map_err(|e| format!("{}: {e}", out.display()))
}

pub fn package_deb() -> Result<String, String> {
    let exe = crate::build_app_repro()?;
    std::fs::create_dir_all(release_dir()).map_err(|e| e.to_string())?;
    let out = release_dir().join(format!("zornov_{VERSION}_{}.deb", deb_arch()?));
    deb_from(&exe, crate::content_core().as_deref(), &crate::licenses::notices()?, &out)?;
    Ok(rel(&out))
}

// -------------------------------------------------------------- AppImage

/// The AppDir: `AppRun` is a link to the program, which finds its content
/// beside itself; the licence notices are in `usr/share/doc/zornov`.
pub fn appdir_from(exe: &Path, content: Option<&Path>, notices: &Notices, dir: &Path) -> Result<(), String> {
    crate::fresh_dir(dir)?;
    let w = |rel: &str, data: &[u8]| -> Result<(), String> {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).map_err(|e| e.to_string())?;
        std::fs::write(&p, data).map_err(|e| format!("{}: {e}", p.display()))
    };
    std::fs::create_dir_all(dir.join("usr/lib/zornov")).map_err(|e| e.to_string())?;
    std::fs::copy(exe, dir.join("usr/lib/zornov/zornov")).map_err(|e| e.to_string())?;
    if let Some(core) = content {
        crate::copy_dir(core, &dir.join("usr/lib/zornov/content-build/core"))?;
    }
    w("zornov.desktop", &desktop_file()?)?;
    w("zornov.png", &icon_png())?;
    crate::write_notices(notices, &dir.join(LINUX_DOC))?;
    #[cfg(unix)]
    std::os::unix::fs::symlink("usr/lib/zornov/zornov", dir.join("AppRun")).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn package_appimage() -> Result<String, String> {
    let tool = std::env::var("APPIMAGETOOL").unwrap_or_else(|_| "appimagetool".into());
    let exe = crate::build_app_repro()?;
    let dir = crate::target_dir().join("appdir/Zornov.AppDir");
    appdir_from(&exe, crate::content_core().as_deref(), &crate::licenses::notices()?, &dir)?;
    std::fs::create_dir_all(release_dir()).map_err(|e| e.to_string())?;
    let arch = std::env::consts::ARCH;
    let out = release_dir().join(format!("Zornov-{VERSION}-{arch}.AppImage"));
    let st = Command::new(&tool).env("ARCH", arch).env("SOURCE_DATE_EPOCH", repro::epoch().to_string()).arg(&dir).arg(&out).status().map_err(|e| format!("cannot run {tool} ({e}); install appimagetool or set APPIMAGETOOL"))?;
    if !st.success() {
        return Err("appimagetool failed".into());
    }
    // An AppImage updates by replacing its own file with this one.
    let signed = sign_update(&out, &format!("linux-{}-appimage", std::env::consts::ARCH)).is_ok();
    Ok(format!("{}{}", rel(&out), if signed { " (signed for updates)" } else { "" }))
}

// ------------------------------------------------------------- portable

/// The folder a portable package (or the MSI) holds: program, marker for
/// updates, content, and the licence notices beside the program.
fn portable_tree(exe: &Path, exe_name: &str, content: Option<&Path>, notices: &Notices, dest: &Path) -> Result<(), String> {
    crate::fresh_dir(dest)?;
    std::fs::copy(exe, dest.join(exe_name)).map_err(|e| format!("cannot copy the binary: {e}"))?;
    std::fs::write(dest.join(studio_update::apply::MARKER), b"").map_err(|e| e.to_string())?;
    crate::write_notices(notices, dest)?;
    if let Some(core) = content {
        crate::copy_dir(core, &dest.join("content-build/core"))?;
    }
    Ok(())
}

/// `dist/zornov-linux-<arch>.tar.gz` plus its signed update archive.
pub fn portable_linux_from(exe: &Path, content: Option<&Path>, notices: &Notices, out_dir: &Path, sign: bool) -> Result<String, String> {
    let name = format!("zornov-linux-{}", crate::arch());
    let tree = out_dir.join(&name);
    portable_tree(exe, "zornov", content, notices, &tree)?;
    std::fs::copy(crate::root().join("packaging/linux/zornov.desktop"), tree.join("zornov.desktop")).map_err(|e| format!("packaging/linux/zornov.desktop: {e}"))?;
    let items = repro::collect(&tree)?;
    repro::tar_gz(&out_dir.join(format!("{name}.tar.gz")), Some(&name), &items, &[])?;
    let upd = out_dir.join(format!("Zornov-{VERSION}-linux-{}.update.tar.gz", std::env::consts::ARCH));
    repro::update_archive(&tree, &upd, true)?;
    let _ = std::fs::remove_dir_all(&tree);
    let signed = sign && sign_update(&upd, &format!("linux-{}", std::env::consts::ARCH)).is_ok();
    Ok(format!("{}{}", rel(&out_dir.join(format!("{name}.tar.gz"))), if signed { " (+ signed update archive)" } else { "" }))
}

/// The console program beside `zornov.exe`, built with it: `zornov.exe` is a
/// window program on Windows, so the terminal side goes through this one.
const WINDOWS_CLI: &str = "zornov-cli.exe";

/// DXC, the DirectX shader compiler, which `app/build.rs` puts beside the
/// program: Zornov draws through DirectX 12 with it on Windows.
const DXC: [&str; 2] = ["dxcompiler.dll", "dxil.dll"];

/// Microsoft's notice for [`DXC`], shipped beside it.
const DXC_NOTICE: &str = "DirectXShaderCompiler-NOTICE.md";

/// [`portable_tree`] for Windows: `zornov.exe` with `zornov-cli.exe`, DXC
/// and DXC's notice beside it.
fn windows_tree(exe: &Path, content: Option<&Path>, notices: &Notices, dest: &Path) -> Result<(), String> {
    portable_tree(exe, "zornov.exe", content, notices, dest)?;
    for f in std::iter::once(WINDOWS_CLI).chain(DXC) {
        std::fs::copy(exe.with_file_name(f), dest.join(f)).map_err(|e| format!("cannot copy {f} (DXC comes from the Windows SDK; see docs/windows.md): {e}"))?;
    }
    let notice = crate::root().join("packaging/windows").join(DXC_NOTICE);
    std::fs::copy(&notice, dest.join(DXC_NOTICE)).map_err(|e| format!("{}: {e}", notice.display()))?;
    Ok(())
}

pub fn portable_windows_from(exe: &Path, content: Option<&Path>, notices: &Notices, out_dir: &Path, sign: bool) -> Result<String, String> {
    let name = format!("zornov-windows-{}", crate::arch());
    let tree = out_dir.join(&name);
    windows_tree(exe, content, notices, &tree)?;
    let items = repro::collect(&tree)?;
    repro::zip(&out_dir.join(format!("{name}.zip")), &name, &items, &[])?;
    let upd = out_dir.join(format!("Zornov-{VERSION}-windows-{}.update.tar.gz", std::env::consts::ARCH));
    repro::update_archive(&tree, &upd, true)?;
    let _ = std::fs::remove_dir_all(&tree);
    let signed = sign && sign_update(&upd, &format!("windows-{}", std::env::consts::ARCH)).is_ok();
    Ok(format!("{}{}", rel(&out_dir.join(format!("{name}.zip"))), if signed { " (+ signed update archive)" } else { "" }))
}

// ------------------------------------------------------------------- MSI

/// A GUID that depends only on `seed` (so every build gives the same one).
fn guid(seed: &str) -> String {
    let h = studio_update::feed::sha256_hex(seed.as_bytes());
    format!("{}-{}-4{}-a{}-{}", &h[0..8], &h[8..12], &h[13..16], &h[17..20], &h[20..32]).to_uppercase()
}

/// The WiX source. `UpgradeCode` never changes, so a new MSI upgrades the
/// old install. Files come in with one `Files` harvest; the program itself
/// is its own component, for the Start menu shortcut.
pub fn wxs() -> String {
    let (upgrade, shortcut) = (guid("studio-upgrade-code"), guid("studio-start-menu-shortcut"));
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<Wix xmlns="http://wixtoolset.org/schemas/v4/wxs">
  <Package Name="Zornov" Manufacturer="Ashrox LLC" Version="{VERSION}" UpgradeCode="{upgrade}" Scope="perMachine" Compressed="yes">
    <MajorUpgrade DowngradeErrorMessage="A newer version of Zornov is already installed." />
    <Icon Id="AppIcon.ico" SourceFile="$(var.IconFile)" />
    <Property Id="ARPPRODUCTICON" Value="AppIcon.ico" />
    <MediaTemplate EmbedCab="yes" />
    <StandardDirectory Id="ProgramFiles64Folder">
      <Directory Id="INSTALLFOLDER" Name="Zornov">
        <Files Include="$(var.SourceDir)\**">
          <Exclude Files="$(var.SourceDir)\zornov.exe" />
        </Files>
        <Component Id="ZornovExeComponent" Guid="{shortcut}">
          <File Id="ZornovExe" Source="$(var.SourceDir)\zornov.exe" KeyPath="yes">
            <Shortcut Id="StartMenuZornov" Directory="ProgramMenuFolder" Name="Zornov" WorkingDirectory="INSTALLFOLDER" Icon="AppIcon.ico" Advertise="yes" />
          </File>
        </Component>
      </Directory>
    </StandardDirectory>
  </Package>
</Wix>
"#
    )
}

/// `cargo xtask package-msi`: builds the program, lays out the folder, then
/// `wix build` (WiX 5: `dotnet tool install --global wix --version 5.0.2`). With
/// `STUDIO_WIN_SIGN_THUMBPRINT` the MSI is signed with that certificate from
/// the Windows store.
pub fn package_msi() -> Result<String, String> {
    let exe = crate::build_app_repro()?;
    let src = crate::target_dir().join("msi/src");
    windows_tree(&exe, crate::content_core().as_deref(), &crate::licenses::notices()?, &src)?;
    let wxs_file = crate::target_dir().join("msi/zornov.wxs");
    std::fs::write(&wxs_file, wxs()).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(release_dir()).map_err(|e| e.to_string())?;
    let arch = if cfg!(target_arch = "aarch64") { "arm64" } else { "x64" };
    let out = release_dir().join(format!("Zornov-{VERSION}-windows-{arch}.msi"));
    let st = Command::new("wix")
        .env("SOURCE_DATE_EPOCH", repro::epoch().to_string())
        .args(["build", "-arch", arch, "-d"])
        .arg(format!("SourceDir={}", src.display()))
        .arg("-d")
        .arg(format!("IconFile={}", crate::root().join("packaging/brand/zornov.ico").display()))
        .arg("-o")
        .arg(&out)
        .arg(&wxs_file)
        .status()
        .map_err(|e| format!("cannot run wix ({e}); install it with `dotnet tool install --global wix --version 5.0.2`"))?;
    if !st.success() {
        return Err("wix build failed".into());
    }
    // A workflow passes an unset secret as an empty value: that is no certificate.
    if let Some(thumb) = std::env::var("STUDIO_WIN_SIGN_THUMBPRINT").ok().filter(|t| !t.trim().is_empty()) {
        let st = Command::new("signtool").args(["sign", "/fd", "SHA256", "/tr", "http://timestamp.digicert.com", "/td", "SHA256", "/sha1"]).arg(thumb.trim()).arg(&out).status().map_err(|e| format!("cannot run signtool: {e}"))?;
        if !st.success() {
            return Err("signtool failed".into());
        }
    }
    // An MSI install updates by running the next MSI (Program Files is the
    // installer's to write).
    let signed = sign_update(&out, &format!("windows-{}-msi", std::env::consts::ARCH)).is_ok();
    Ok(format!("{}{}", rel(&out), if signed { " (signed for updates)" } else { "" }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn notices() -> Notices {
        vec![("LICENSE".into(), b"licence".to_vec()), ("NOTICE".into(), b"notice".to_vec())]
    }

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("xtask-pkg-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("core")).unwrap();
        std::fs::write(d.join("zornov-bin"), "binary").unwrap();
        for f in std::iter::once(WINDOWS_CLI).chain(DXC) {
            std::fs::write(d.join(f), f).unwrap();
        }
        std::fs::write(d.join("core/index.json"), "{}").unwrap();
        d
    }

    #[test]
    fn deb_is_an_ar_with_three_members_and_is_reproducible() {
        let d = scratch("deb");
        let (a, b) = (d.join("a.deb"), d.join("b.deb"));
        deb_from(&d.join("zornov-bin"), Some(&d.join("core")), &notices(), &a).unwrap();
        deb_from(&d.join("zornov-bin"), Some(&d.join("core")), &notices(), &b).unwrap();
        let bytes = std::fs::read(&a).unwrap();
        assert_eq!(bytes, std::fs::read(&b).unwrap(), "same sources, same package");
        assert!(bytes.starts_with(b"!<arch>\ndebian-binary   "));
        let s = String::from_utf8_lossy(&bytes);
        assert!(s.contains("control.tar.gz") && s.contains("data.tar.gz"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn appdir_has_apprun_desktop_icon_and_content() {
        let d = scratch("appdir");
        appdir_from(&d.join("zornov-bin"), Some(&d.join("core")), &notices(), &d.join("AppDir")).unwrap();
        for f in ["AppRun", "zornov.desktop", "zornov.png", "usr/lib/zornov/zornov", "usr/lib/zornov/content-build/core/index.json"] {
            assert!(d.join("AppDir").join(f).exists(), "{f}");
        }
        assert!(std::fs::read_to_string(d.join("AppDir/zornov.desktop")).unwrap().contains("Icon=zornov"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn portable_packages_are_reproducible_and_updatable() {
        let d = scratch("portable");
        let out1 = d.join("o1");
        let out2 = d.join("o2");
        std::fs::create_dir_all(&out1).unwrap();
        std::fs::create_dir_all(&out2).unwrap();
        portable_linux_from(&d.join("zornov-bin"), Some(&d.join("core")), &notices(), &out1, false).unwrap();
        portable_linux_from(&d.join("zornov-bin"), Some(&d.join("core")), &notices(), &out2, false).unwrap();
        let n = format!("zornov-linux-{}.tar.gz", crate::arch());
        assert_eq!(std::fs::read(out1.join(&n)).unwrap(), std::fs::read(out2.join(&n)).unwrap());
        portable_windows_from(&d.join("zornov-bin"), None, &notices(), &out1, false).unwrap();
        assert!(out1.join(format!("zornov-windows-{}.zip", crate::arch())).is_file());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// The notice files every package must carry.
    const REQUIRED: &[&str] = &["ATTRIBUTIONS.md", "JetBrainsMono-LICENSE.txt", "LICENSE", "LICENSE-ACES.md", "Manrope-LICENSE.txt", "NOTICE", "Phosphor-LICENSE.txt", crate::licenses::THIRD_PARTY];

    /// The paths in a `.tar.gz`.
    fn tar_paths(gz: &[u8]) -> Vec<String> {
        let mut t = tar::Archive::new(flate2::read::GzDecoder::new(gz));
        t.entries().unwrap().map(|e| e.unwrap().path().unwrap().to_string_lossy().into_owned()).collect()
    }

    /// A member of an `ar` archive (a `.deb`).
    fn ar_member(ar: &[u8], name: &str) -> Vec<u8> {
        let mut at = 8;
        while at + 60 <= ar.len() {
            let h = &ar[at..at + 60];
            let size: usize = String::from_utf8_lossy(&h[48..58]).trim().parse().unwrap();
            if String::from_utf8_lossy(&h[..16]).trim() == name {
                return ar[at + 60..at + 60 + size].to_vec();
            }
            at += 60 + size + size % 2;
        }
        panic!("no {name} in the archive")
    }

    /// Every package (.app, .deb, AppImage folder, portable Linux and
    /// Windows archives, the MSI's folder and the update archives) carries
    /// the real licence notices, and the packages stay reproducible.
    #[test]
    fn every_package_carries_the_licence_notices() {
        let d = scratch("notices");
        let (bin, core) = (d.join("zornov-bin"), d.join("core"));
        let notices = crate::licenses::notices().unwrap();
        let names: Vec<&str> = notices.iter().map(|n| n.0.as_str()).collect();
        for r in REQUIRED {
            assert!(names.contains(r), "{r} is not among the notices {names:?}");
        }
        let third = String::from_utf8(notices.iter().find(|n| n.0 == crate::licenses::THIRD_PARTY).unwrap().1.clone()).unwrap();
        assert!(third.contains("Permission is hereby granted") && third.contains("Apache License"));
        let has_all = |paths: &[String], dir: &str| {
            for r in REQUIRED {
                let want = if dir.is_empty() { (*r).to_owned() } else { format!("{dir}/{r}") };
                assert!(paths.iter().any(|p| p.trim_start_matches("./") == want), "{want} missing from {paths:?}");
            }
        };

        // macOS
        let app = d.join("Zornov.app");
        crate::assemble_app(&bin, Some(&core), &notices, &app).unwrap();
        let app_files: Vec<String> = repro::collect(&app).unwrap().into_iter().map(|i| i.rel).collect();
        has_all(&app_files, "Contents/Resources/Licenses");
        let upd = d.join("mac.update.tar.gz");
        repro::update_archive(&app, &upd, false).unwrap();
        has_all(&tar_paths(&std::fs::read(&upd).unwrap()), "Contents/Resources/Licenses");

        // .deb
        let deb = d.join("zornov.deb");
        deb_from(&bin, Some(&core), &notices, &deb).unwrap();
        has_all(&tar_paths(&ar_member(&std::fs::read(&deb).unwrap(), "data.tar.gz")), LINUX_DOC);

        // AppImage
        appdir_from(&bin, Some(&core), &notices, &d.join("AppDir")).unwrap();
        let appdir: Vec<String> = repro::collect(&d.join("AppDir")).unwrap().into_iter().map(|i| i.rel).collect();
        has_all(&appdir, LINUX_DOC);

        // Portable Linux and Windows, with their update archives
        let (o1, o2) = (d.join("o1"), d.join("o2"));
        std::fs::create_dir_all(&o1).unwrap();
        std::fs::create_dir_all(&o2).unwrap();
        portable_linux_from(&bin, Some(&core), &notices, &o1, false).unwrap();
        portable_linux_from(&bin, Some(&core), &notices, &o2, false).unwrap();
        let linux = format!("zornov-linux-{}", crate::arch());
        let tgz = std::fs::read(o1.join(format!("{linux}.tar.gz"))).unwrap();
        assert_eq!(tgz, std::fs::read(o2.join(format!("{linux}.tar.gz"))).unwrap(), "same sources, same archive");
        has_all(&tar_paths(&tgz), &linux);
        let linux_upd = o1.join(format!("Zornov-{VERSION}-linux-{}.update.tar.gz", std::env::consts::ARCH));
        has_all(&tar_paths(&std::fs::read(linux_upd).unwrap()), "");
        portable_windows_from(&bin, None, &notices, &o1, false).unwrap();
        let win = format!("zornov-windows-{}", crate::arch());
        let z = zip::ZipArchive::new(std::fs::File::open(o1.join(format!("{win}.zip"))).unwrap()).unwrap();
        let zipped: Vec<String> = z.file_names().map(str::to_owned).collect();
        has_all(&zipped, &win);
        for f in ["zornov.exe", WINDOWS_CLI, DXC_NOTICE].into_iter().chain(DXC) {
            assert!(zipped.contains(&format!("{win}/{f}")), "{f} missing from the zip");
        }

        // The MSI is harvested from this folder: the notices sit next to zornov.exe.
        windows_tree(&bin, None, &notices, &d.join("msi")).unwrap();
        let msi: Vec<String> = repro::collect(&d.join("msi")).unwrap().into_iter().map(|i| i.rel).collect();
        has_all(&msi, "");
        for f in ["zornov.exe", WINDOWS_CLI, DXC_NOTICE].into_iter().chain(DXC) {
            assert!(msi.contains(&f.to_owned()), "{f} missing from the MSI folder");
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn msi_source_is_stable_and_valid_looking() {
        assert_eq!(wxs(), wxs());
        let g = guid("x");
        assert_eq!(g.len(), 36);
        assert_eq!(g, guid("x"));
        assert_ne!(g, guid("y"));
        assert!(wxs().contains("UpgradeCode=\"") && wxs().contains(&format!("Version=\"{VERSION}\"")));
    }

    #[test]
    fn the_app_is_named_zornov_in_every_package() {
        let d = scratch("names");
        let (bin, core) = (d.join("zornov-bin"), d.join("core"));
        let notices = notices();

        // macOS: bundle names, the identifier and the permission texts.
        let app = d.join("Zornov.app");
        crate::assemble_app(&bin, Some(&core), &notices, &app).unwrap();
        let plist = std::fs::read_to_string(app.join("Contents/Info.plist")).unwrap();
        for want in [
            "<key>CFBundleName</key><string>Zornov</string>",
            "<key>CFBundleDisplayName</key><string>Zornov</string>",
            "<key>CFBundleIdentifier</key><string>com.zornov.app</string>",
            "<key>CFBundleExecutable</key><string>zornov</string>",
            "<string>Zornov records voice-overs",
            "<string>Zornov records from your camera",
        ] {
            assert!(plist.contains(want), "{want} missing from the Info.plist");
        }
        assert!(!plist.contains("Studio") && !plist.contains("org.studio"), "{plist}");
        assert!(app.join("Contents/MacOS/zornov").is_file());

        // Linux: the desktop entry and the .deb control file.
        let desktop = String::from_utf8(desktop_file().unwrap()).unwrap();
        for want in ["Name=Zornov\n", "Exec=zornov\n", "Icon=zornov\n", "StartupWMClass=zornov\n"] {
            assert!(desktop.contains(want), "{want} missing from {desktop}");
        }
        assert!(!desktop.contains("Studio"), "{desktop}");
        let deb = d.join("zornov.deb");
        deb_from(&bin, Some(&core), &notices, &deb).unwrap();
        let ctl = tar_paths(&ar_member(&std::fs::read(&deb).unwrap(), "control.tar.gz"));
        assert!(ctl.iter().any(|p| p.ends_with("control")));
        let data = tar_paths(&ar_member(&std::fs::read(&deb).unwrap(), "data.tar.gz"));
        for want in ["usr/bin/zornov", "usr/lib/zornov/zornov", "usr/share/applications/zornov.desktop", "usr/share/icons/hicolor/256x256/apps/zornov.png"] {
            assert!(data.iter().any(|p| p.trim_start_matches("./") == want), "{want} missing from {data:?}");
        }

        // Windows: the package name, folder, shortcut and program, with the old UpgradeCode.
        let w = wxs();
        for want in ["Package Name=\"Zornov\"", "Name=\"Zornov\"", "zornov.exe"] {
            assert!(w.contains(want), "{want} missing from the WiX source");
        }
        assert!(!w.contains("Studio") && !w.contains("studio.exe"), "{w}");
        assert!(w.contains(&format!("UpgradeCode=\"{}\"", guid("studio-upgrade-code"))), "the UpgradeCode must not change");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_icon_is_a_png() {
        assert!(icon_png().starts_with(b"\x89PNG"));
    }
}
