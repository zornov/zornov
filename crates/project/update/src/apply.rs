// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Unpacking an update and swapping it in.
//!
//! The archive is a `.tar.gz` of the install folder (the `.app` bundle's
//! contents on macOS, the folder holding `studio` or `studio.exe` elsewhere)
//! with a `.studio-manifest` listing every file. It is unpacked beside the
//! install ("staged") when it has been downloaded and checked, and swapped
//! in at the next start, before the window opens: each file is renamed to
//! `<name>.studio-old` and the new one moved in (a running program can be
//! renamed on every platform, which a folder swap cannot do on Windows),
//! with the old ones put back if anything fails. Files the new version does
//! not list are removed, so a signed macOS bundle stays whole.
//!
//! Two installs update another way ([`Target`]): a per-machine Windows
//! install runs the new `.msi` once Studio has quit, and a Linux AppImage
//! has its file replaced by the new one.

use crate::feed::{self, Entry};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

pub const MANIFEST: &str = ".studio-manifest";
/// Present beside `studio` / `studio.exe` in a folder install that may
/// update itself (the portable packages carry it).
pub const MARKER: &str = "studio.install";
const OLD: &str = ".studio-old";

pub fn updates_dir() -> PathBuf {
    crate::data_dir().join("updates")
}

/// The folder an update replaces, if this Studio is a real install that can
/// update itself: the `.app` on macOS, a marked folder elsewhere. A build
/// run from `target/` never is.
pub fn install_root() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
    let root = if cfg!(target_os = "macos") {
        exe.ancestors().find(|a| a.extension().is_some_and(|e| e == "app"))?.to_path_buf()
    } else {
        let dir = exe.parent()?.to_path_buf();
        dir.join(MARKER).is_file().then_some(dir)?
    };
    writable(&root).then_some(root)
}

/// How this copy of Studio updates itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// Files swapped in place: the `.app`, or a marked, writable folder.
    Folder(PathBuf),
    /// A per-machine Windows install (the MSI): the new `.msi` runs after
    /// Studio quits.
    Msi,
    /// A Linux AppImage: the new file takes this one's place.
    AppImage(PathBuf),
}

impl Target {
    /// The feed key of this kind of build: `macos-aarch64`,
    /// `windows-x86_64-msi`, `linux-x86_64-appimage`...
    pub fn feed_key(&self) -> String {
        match self {
            Target::Folder(_) => feed::platform(),
            Target::Msi => format!("{}-msi", feed::platform()),
            Target::AppImage(_) => format!("{}-appimage", feed::platform()),
        }
    }

    /// What the feed's archive is for this target: unpacked, or used as is.
    pub fn kind(&self) -> &'static str {
        match self {
            Target::Folder(_) => "",
            Target::Msi => "msi",
            Target::AppImage(_) => "appimage",
        }
    }
}

/// How this copy updates, if it can: a development build, a `.deb` or a
/// folder it cannot write cannot (it only learns a newer version exists).
pub fn target() -> Option<Target> {
    if let Some(root) = install_root() {
        return Some(Target::Folder(root));
    }
    if cfg!(target_os = "linux") {
        // Set by the AppImage runtime to the file that was started.
        let file = PathBuf::from(std::env::var_os("APPIMAGE")?).canonicalize().ok()?;
        return (file.is_file() && writable(file.parent()?)).then_some(Target::AppImage(file));
    }
    if cfg!(windows) {
        // The MSI installs the marked folder under Program Files, which
        // only the installer may write.
        let dir = std::env::current_exe().ok()?.canonicalize().ok()?.parent()?.to_path_buf();
        let under = |v: &str| std::env::var_os(v).and_then(|p| PathBuf::from(p).canonicalize().ok()).is_some_and(|p| dir.starts_with(p));
        return (dir.join(MARKER).is_file() && (under("ProgramFiles") || under("ProgramW6432"))).then_some(Target::Msi);
    }
    None
}

fn writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".studio-write-test-{}", std::process::id()));
    let ok = std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

/// Unpacks the `.tar.gz` at `archive` into `dest` (emptied first). Entries
/// that would land outside `dest` are refused.
pub fn stage(archive: &Path, dest: &Path) -> Result<(), String> {
    if dest.exists() {
        std::fs::remove_dir_all(dest).map_err(|e| format!("cannot clear {}: {e}", dest.display()))?;
    }
    std::fs::create_dir_all(dest).map_err(|e| format!("{}: {e}", dest.display()))?;
    let gz = flate2::read::GzDecoder::new(std::fs::File::open(archive).map_err(|e| format!("{}: {e}", archive.display()))?);
    let mut tar = tar::Archive::new(gz);
    tar.set_preserve_permissions(true);
    for e in tar.entries().map_err(|e| format!("the update archive is damaged: {e}"))? {
        let mut e = e.map_err(|e| format!("the update archive is damaged: {e}"))?;
        let ok = e.unpack_in(dest).map_err(|e| format!("cannot unpack the update: {e}"))?;
        if !ok {
            return Err("the update archive has a file outside its folder".into());
        }
    }
    if !dest.join(MANIFEST).is_file() {
        return Err("the update archive has no manifest".into());
    }
    Ok(())
}

fn manifest(dir: &Path) -> Result<BTreeSet<String>, String> {
    let text = std::fs::read_to_string(dir.join(MANIFEST)).map_err(|e| format!("no manifest: {e}"))?;
    let set: BTreeSet<String> = text.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_owned).collect();
    if set.is_empty() || set.iter().any(|p| p.starts_with('/') || p.split('/').any(|c| c == ".." || c.is_empty())) {
        return Err("the update's manifest is not valid".into());
    }
    Ok(set)
}

fn files_under(dir: &Path) -> Result<Vec<String>, String> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) -> std::io::Result<()> {
        for e in std::fs::read_dir(dir)? {
            let p = e?.path();
            if std::fs::symlink_metadata(&p)?.is_dir() {
                walk(root, &p, out)?;
            } else {
                out.push(p.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/"));
            }
        }
        Ok(())
    }
    let mut v = Vec::new();
    walk(dir, dir, &mut v).map_err(|e| format!("{}: {e}", dir.display()))?;
    v.sort();
    Ok(v)
}

fn move_in(from: &Path, to: &Path) -> std::io::Result<()> {
    let meta = std::fs::symlink_metadata(from)?;
    if meta.file_type().is_symlink() {
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(std::fs::read_link(from)?, to)?;
            return std::fs::remove_file(from);
        }
        #[cfg(not(unix))]
        return Err(std::io::Error::other("symbolic links are not supported"));
    }
    std::fs::rename(from, to).or_else(|_| std::fs::copy(from, to).map(|_| ()))
}

/// Replaces `install`'s files with `staged`'s, all or nothing.
pub fn swap(staged: &Path, install: &Path) -> Result<(), String> {
    let keep = manifest(staged)?;
    let new_files = files_under(staged)?;
    // (the file now at `dest`, where its predecessor went)
    let mut done: Vec<(PathBuf, Option<PathBuf>)> = Vec::new();
    let mut run = || -> Result<(), String> {
        for rel in &new_files {
            let (src, dest) = (staged.join(rel), install.join(rel));
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
            }
            let old = if std::fs::symlink_metadata(&dest).is_ok() {
                let mut o = dest.clone().into_os_string();
                o.push(OLD);
                let o = PathBuf::from(o);
                let _ = std::fs::remove_file(&o);
                std::fs::rename(&dest, &o).map_err(|e| format!("cannot move aside {}: {e}", dest.display()))?;
                Some(o)
            } else {
                None
            };
            done.push((dest.clone(), old));
            move_in(&src, &dest).map_err(|e| format!("cannot install {}: {e}", dest.display()))?;
        }
        Ok(())
    };
    if let Err(e) = run() {
        for (dest, old) in done.into_iter().rev() {
            let _ = std::fs::remove_file(&dest);
            if let Some(o) = old {
                let _ = std::fs::rename(o, dest);
            }
        }
        return Err(e);
    }
    // Files the new version no longer has.
    for rel in files_under(install)? {
        if !rel.ends_with(OLD) && !keep.contains(&rel) && rel != MANIFEST && rel != MARKER {
            let _ = std::fs::remove_file(install.join(&rel));
        }
    }
    Ok(())
}

/// Removes the `.studio-old` files a swap left (they were in use when it
/// ran). Cheap when there are none.
pub fn clean_old(install: &Path) {
    if !updates_dir().join("cleanup").is_file() {
        return;
    }
    for rel in files_under(install).unwrap_or_default() {
        if rel.ends_with(OLD) {
            let _ = std::fs::remove_file(install.join(rel));
        }
    }
    let _ = std::fs::remove_file(updates_dir().join("cleanup"));
}

/// A downloaded, checked update waiting for the next start.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pending {
    pub version: String,
    pub platform: String,
    /// The unpacked folder, or for an `.msi` or AppImage the file itself.
    pub staged: PathBuf,
    pub entry: Entry,
    /// [`Target::kind`] of the install it was fetched for.
    #[serde(default)]
    pub kind: String,
}

pub fn pending_file() -> PathBuf {
    updates_dir().join("pending.json")
}

pub fn pending() -> Option<Pending> {
    serde_json::from_slice(&std::fs::read(pending_file()).ok()?).ok()
}

pub fn save_pending(p: &Pending) -> Result<(), String> {
    std::fs::create_dir_all(updates_dir()).map_err(|e| e.to_string())?;
    std::fs::write(pending_file(), serde_json::to_vec_pretty(p).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}

/// What `apply_pending` did.
#[derive(Debug, PartialEq, Eq)]
pub enum Applied {
    /// Nothing waiting (or this is not an installed copy).
    Nothing,
    /// Version now installed: start it.
    Installed(String),
    /// The installer for this version is starting: quit now (it starts
    /// Studio again when it has finished).
    Handed(String),
    /// It failed; the old version stays (the reason is in the log file).
    Failed(String),
}

/// At start: puts in a waiting update for `target`. The signature is
/// checked again here, so a tampered `pending.json` or staged files from
/// another source cannot be installed without Studio's key. Returns
/// [`Applied::Installed`] when the caller should [`relaunch`] and exit, and
/// [`Applied::Handed`] when it should just exit.
pub fn apply_pending(public_key_hex: &str, current_version: &str, target: Option<&Target>) -> Applied {
    let Some(p) = pending() else {
        if let Some(Target::Folder(i)) = target {
            clean_old(i);
        }
        clean_downloads();
        return Applied::Nothing;
    };
    // Whatever happens, the same update is not tried at every start.
    let _ = std::fs::remove_file(pending_file());
    let Some(target) = target else { return Applied::Nothing };
    let fail = |why: String| {
        let _ = std::fs::write(updates_dir().join("last-error.txt"), &why);
        if p.staged.is_dir() {
            let _ = std::fs::remove_dir_all(&p.staged);
        } else {
            let _ = std::fs::remove_file(&p.staged);
        }
        Applied::Failed(why)
    };
    if let Err(e) = feed::verify(public_key_hex, &p.platform, &p.version, &p.entry) {
        return fail(e);
    }
    if p.platform != target.feed_key() || p.kind != target.kind() || !feed::is_newer(&p.version, current_version) {
        return fail(format!("update {} is not an upgrade for this {current_version} ({})", p.version, target.feed_key()));
    }
    let done = match target {
        Target::Folder(install) => swap(&p.staged, install).map(|()| {
            let _ = std::fs::remove_dir_all(&p.staged);
            let _ = std::fs::write(updates_dir().join("cleanup"), b"");
            Applied::Installed(p.version.clone())
        }),
        Target::AppImage(file) => check_file(&p).and_then(|()| replace_file(&p.staged, file)).map(|()| {
            let _ = std::fs::remove_file(&p.staged);
            Applied::Installed(p.version.clone())
        }),
        Target::Msi => check_file(&p).and_then(|()| run_msi(&p.staged)).map(|()| Applied::Handed(p.version.clone())),
    };
    match done {
        Ok(a) => {
            let _ = std::fs::remove_file(updates_dir().join("last-error.txt"));
            a
        }
        Err(e) => fail(e),
    }
}

/// A staged installer or AppImage is used as downloaded, so its bytes are
/// checked against the signed hash once more.
fn check_file(p: &Pending) -> Result<(), String> {
    let size = std::fs::metadata(&p.staged).map_err(|e| format!("{}: {e}", p.staged.display()))?.len();
    let hash = feed::sha256_file(&p.staged).map_err(|e| e.to_string())?;
    if size != p.entry.size || !hash.eq_ignore_ascii_case(&p.entry.sha256) {
        return Err("the waiting update does not match its signature; it was discarded".into());
    }
    Ok(())
}

/// Puts `new` in `file`'s place in one rename, executable, so `file` is
/// either the old program or the new one.
pub fn replace_file(new: &Path, file: &Path) -> Result<(), String> {
    let name = file.file_name().ok_or("the program has no file name")?.to_string_lossy().into_owned();
    let tmp = file.with_file_name(format!(".{name}.studio-new"));
    std::fs::copy(new, &tmp).map_err(|e| format!("cannot write beside {}: {e}", file.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755));
    }
    std::fs::rename(&tmp, file).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("cannot replace {}: {e}", file.display())
    })
}

/// Starts the `.msi` once this Studio has had a moment to quit, then
/// Studio again when it succeeds. Windows asks for permission itself.
#[cfg(windows)]
fn run_msi(msi: &Path) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let line = format!("/C ping -n 3 127.0.0.1 >nul & start \"\" /wait msiexec /i \"{}\" /passive /norestart && start \"\" \"{}\"", msi.display(), exe.display());
    std::process::Command::new("cmd").raw_arg(line).creation_flags(CREATE_NO_WINDOW).spawn().map(|_| ()).map_err(|e| format!("cannot start the installer: {e}"))
}

#[cfg(not(windows))]
fn run_msi(_: &Path) -> Result<(), String> {
    Err("an .msi installs only on Windows".into())
}

/// Removes downloads no update is waiting on (an installer already run, an
/// abandoned download).
fn clean_downloads() {
    for e in std::fs::read_dir(updates_dir()).into_iter().flatten().flatten() {
        if e.file_type().is_ok_and(|t| t.is_dir()) && feed::parse_version(&e.file_name().to_string_lossy()).is_some() {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

/// Starts the installed Studio again with the same arguments (for an
/// AppImage, the AppImage file rather than the program inside it).
pub fn relaunch() -> Result<(), String> {
    let exe = match std::env::var_os("APPIMAGE").filter(|_| cfg!(target_os = "linux")) {
        Some(a) => PathBuf::from(a),
        None => std::env::current_exe().map_err(|e| e.to_string())?,
    };
    std::process::Command::new(exe).args(std::env::args_os().skip(1)).spawn().map(|_| ()).map_err(|e| format!("cannot start the new Studio: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("studio-update-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn archive(dir: &Path, files: &[(&str, &str)], manifest: Option<&str>) -> PathBuf {
        let path = dir.join("u.tar.gz");
        let gz = flate2::write::GzEncoder::new(std::fs::File::create(&path).unwrap(), flate2::Compression::fast());
        let mut tar = tar::Builder::new(gz);
        let mut add = |name: &str, data: &str| {
            let mut h = tar::Header::new_gnu();
            h.set_size(data.len() as u64);
            h.set_mode(0o644);
            h.set_cksum();
            tar.append_data(&mut h, name, data.as_bytes()).unwrap();
        };
        if let Some(m) = manifest {
            add(MANIFEST, m);
        }
        for (n, d) in files {
            add(n, d);
        }
        tar.into_inner().unwrap().finish().unwrap().flush().unwrap();
        path
    }

    #[test]
    fn stage_and_swap_replace_add_and_remove_files() {
        let d = scratch("swap");
        let arc = archive(&d, &[("studio", "new"), ("content/a.txt", "A2"), ("content/b.txt", "B")], Some("studio\ncontent/a.txt\ncontent/b.txt\n"));
        let (staged, install) = (d.join("staged"), d.join("install"));
        std::fs::create_dir_all(install.join("content")).unwrap();
        std::fs::write(install.join("studio"), "old").unwrap();
        std::fs::write(install.join("content/a.txt"), "A").unwrap();
        std::fs::write(install.join("content/gone.txt"), "x").unwrap();
        stage(&arc, &staged).unwrap();
        swap(&staged, &install).unwrap();
        assert_eq!(std::fs::read_to_string(install.join("studio")).unwrap(), "new");
        assert_eq!(std::fs::read_to_string(install.join("content/a.txt")).unwrap(), "A2");
        assert_eq!(std::fs::read_to_string(install.join("content/b.txt")).unwrap(), "B");
        assert!(!install.join("content/gone.txt").exists(), "stale file removed");
        assert_eq!(std::fs::read_to_string(install.join("studio.studio-old")).unwrap(), "old");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_failed_swap_puts_everything_back() {
        let d = scratch("rollback");
        let arc = archive(&d, &[("a", "A2"), ("dir/b", "B2")], Some("a\ndir/b\n"));
        let (staged, install) = (d.join("staged"), d.join("install"));
        std::fs::create_dir_all(&install).unwrap();
        std::fs::write(install.join("a"), "A").unwrap();
        // `dir` is a file in the install, so `dir/b` cannot be made.
        std::fs::write(install.join("dir"), "in the way").unwrap();
        stage(&arc, &staged).unwrap();
        assert!(swap(&staged, &install).is_err());
        assert_eq!(std::fs::read_to_string(install.join("a")).unwrap(), "A");
        assert_eq!(std::fs::read_to_string(install.join("dir")).unwrap(), "in the way");
        assert!(!install.join("a.studio-old").exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn an_appimage_is_replaced_whole_and_stays_executable() {
        let d = scratch("appimage");
        let (old, new) = (d.join("Studio.AppImage"), d.join("new"));
        std::fs::write(&old, "old").unwrap();
        std::fs::write(&new, "new").unwrap();
        replace_file(&new, &old).unwrap();
        assert_eq!(std::fs::read_to_string(&old).unwrap(), "new");
        assert!(!d.join(".Studio.AppImage.studio-new").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&old).unwrap().permissions().mode() & 0o111, 0o111);
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_staged_file_is_checked_against_its_signed_hash() {
        let d = scratch("filecheck");
        let staged = d.join("Studio.msi");
        std::fs::write(&staged, "installer").unwrap();
        let entry = Entry { url: String::new(), size: 9, sha256: feed::sha256_hex(b"installer"), signature: String::new() };
        let mut p = Pending { version: "0.2.0".into(), platform: "windows-x86_64-msi".into(), staged: staged.clone(), entry, kind: "msi".into() };
        check_file(&p).unwrap();
        std::fs::write(&staged, "tampered!").unwrap();
        assert!(check_file(&p).is_err());
        p.entry.size = 1;
        assert!(check_file(&p).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn each_kind_of_install_has_its_own_feed_key() {
        let p = feed::platform();
        assert_eq!(Target::Folder(PathBuf::new()).feed_key(), p);
        assert_eq!(Target::Msi.feed_key(), format!("{p}-msi"));
        assert_eq!(Target::AppImage(PathBuf::new()).feed_key(), format!("{p}-appimage"));
    }

    #[test]
    fn archives_without_a_manifest_or_escaping_are_refused() {
        let d = scratch("bad");
        let arc = archive(&d, &[("studio", "x")], None);
        assert!(stage(&arc, &d.join("s")).unwrap_err().contains("no manifest"));
        let m = d.join("m");
        std::fs::create_dir_all(&m).unwrap();
        std::fs::write(m.join(MANIFEST), "../evil\n").unwrap();
        assert!(manifest(&m).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }
}
