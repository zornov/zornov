// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The models folder: which models are installed, verifying them, and the
//! explicit, user-initiated download.
//!
//! This module holds the crate's only network access: [`ModelStore::download`]
//! runs the system `curl` (shipped with macOS) to fetch a model's files from
//! the upstream URLs in the [registry](crate::registry). It is only ever
//! called when the user presses a download button. Downloads go to a
//! `.part` file (resumed if present), are checked against the registry's
//! SHA-256 and only then moved into place, so a half-finished or tampered
//! file is never loaded.

use crate::error::{Error, Result};
use crate::jobs::JobCtx;
use crate::registry::{FileSpec, ModelId, ModelSpec, Unpack};
use crate::sha256::{hex, sha256, sha256_file};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Install state shown in Settings > AI models.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModelStatus {
    Installed,
    NotInstalled,
    /// Some files are present but others are missing or have the wrong size
    /// (an interrupted download); downloading again repairs it.
    Incomplete { missing: Vec<String> },
}

/// The models folder.
#[derive(Clone, Debug)]
pub struct ModelStore {
    root: PathBuf,
}

impl Default for ModelStore {
    fn default() -> Self {
        ModelStore::new(Self::default_root())
    }
}

impl ModelStore {
    /// `$HOME/Movies/Studio/Models`.
    pub fn default_root() -> PathBuf {
        studio_paths::studio().join("Models")
    }

    pub fn new(root: impl Into<PathBuf>) -> ModelStore {
        ModelStore { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The model's folder.
    pub fn dir(&self, id: ModelId) -> PathBuf {
        self.root.join(id.spec().dir)
    }

    /// Path of one installed file of a model.
    pub fn file(&self, id: ModelId, name: &str) -> PathBuf {
        self.dir(id).join(name)
    }

    /// Quick check (existence and size; no hashing).
    pub fn status(&self, id: ModelId) -> ModelStatus {
        let dir = self.dir(id);
        let mut missing = Vec::new();
        let mut present = 0;
        for f in id.spec().files {
            for (name, _, size) in f.installed_files() {
                match std::fs::metadata(dir.join(name)) {
                    Ok(m) if m.len() == size => present += 1,
                    _ => missing.push(name.to_string()),
                }
            }
        }
        match (missing.is_empty(), present) {
            (true, _) => ModelStatus::Installed,
            (false, 0) => ModelStatus::NotInstalled,
            _ => ModelStatus::Incomplete { missing },
        }
    }

    pub fn is_installed(&self, id: ModelId) -> bool {
        self.status(id) == ModelStatus::Installed
    }

    /// The model folder, or [`Error::ModelMissing`] (which the UI turns into
    /// a "download" prompt).
    pub fn require(&self, id: ModelId) -> Result<PathBuf> {
        if self.is_installed(id) { Ok(self.dir(id)) } else { Err(Error::ModelMissing(id)) }
    }

    /// The first installed model of `ids` (e.g. the largest Whisper the user
    /// has), or `ModelMissing` for the first one.
    pub fn first_installed(&self, ids: &[ModelId]) -> Result<ModelId> {
        ids.iter().copied().find(|&id| self.is_installed(id)).ok_or(Error::ModelMissing(ids[0]))
    }

    /// Hashes every installed file. Returns the names of files that are
    /// missing or whose SHA-256 does not match.
    pub fn verify(&self, id: ModelId, ctx: &JobCtx) -> Result<Vec<String>> {
        let dir = self.dir(id);
        let all: Vec<_> = id.spec().files.iter().flat_map(|f| f.installed_files()).collect();
        let total: u64 = all.iter().map(|f| f.2).sum::<u64>().max(1);
        let mut done = 0u64;
        let mut bad = Vec::new();
        for (name, sha, size) in all {
            let r = sha256_file(&dir.join(name), |n| {
                ctx.set_progress((done + n) as f32 / total as f32);
                !ctx.is_cancelled()
            });
            ctx.check()?;
            match r {
                Ok(Some(h)) if hex(&h) == sha => {}
                _ => bad.push(name.to_string()),
            }
            done += size;
        }
        Ok(bad)
    }

    /// Downloads (or repairs) a model. **Network access**: call only on an
    /// explicit user request. Files already present with the right hash are
    /// kept.
    pub fn download(&self, id: ModelId, ctx: &JobCtx) -> Result<()> {
        self.download_spec(id.spec(), &Net, ctx)
    }

    pub(crate) fn download_spec(&self, spec: &ModelSpec, fetch: &dyn Fetch, ctx: &JobCtx) -> Result<()> {
        let dir = self.root.join(spec.dir);
        std::fs::create_dir_all(&dir)?;
        let total: u64 = spec.download_size().max(1);
        let mut before = 0u64;
        for f in spec.files {
            let sub = ctx.range(before as f32 / total as f32, (before + f.size) as f32 / total as f32);
            ctx.set_message(format!("Downloading {} ({})", spec.name, f.name));
            if !installed_ok(&dir, f) {
                fetch_file(&dir, f, fetch, &sub)?;
            }
            before += f.size;
        }
        ctx.set_progress(1.0);
        Ok(())
    }

    /// Deletes a model's folder.
    pub fn remove(&self, id: ModelId) -> Result<()> {
        match std::fs::remove_dir_all(self.dir(id)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        }
    }

    /// Bytes used by a model's folder.
    pub fn disk_usage(&self, id: ModelId) -> u64 {
        std::fs::read_dir(self.dir(id)).map(|rd| rd.flatten().filter_map(|e| e.metadata().ok()).map(|m| m.len()).sum()).unwrap_or(0)
    }
}

/// True when every file this download produces exists with the right size
/// and hash.
fn installed_ok(dir: &Path, f: &FileSpec) -> bool {
    f.installed_files().iter().all(|(name, sha, size)| {
        let p = dir.join(name);
        std::fs::metadata(&p).map(|m| m.len() == *size).unwrap_or(false) && matches!(sha256_file(&p, |_| true), Ok(Some(h)) if hex(&h) == *sha)
    })
}

fn fetch_file(dir: &Path, f: &FileSpec, fetch: &dyn Fetch, ctx: &JobCtx) -> Result<()> {
    let part = dir.join(format!("{}.part", f.name));
    // A .part larger than the file is junk; anything else is resumed.
    if std::fs::metadata(&part).map(|m| m.len() > f.size).unwrap_or(false) {
        std::fs::remove_file(&part)?;
    }
    let dl = ctx.range(0.0, 0.9);
    fetch.fetch(f.url, &part, &mut |n| {
        dl.set_progress(n as f32 / f.size.max(1) as f32);
        !ctx.is_cancelled()
    })?;
    ctx.check()?;
    let vc = ctx.range(0.9, 1.0);
    let got = sha256_file(&part, |n| {
        vc.set_progress(n as f32 / f.size.max(1) as f32);
        !ctx.is_cancelled()
    })?;
    ctx.check()?;
    let got = got.map(|h| hex(&h)).unwrap_or_default();
    if got != f.sha256 {
        let _ = std::fs::remove_file(&part);
        return Err(Error::Download(format!("{}: checksum mismatch (expected {}, got {got}); the file was deleted, try again", f.name, f.sha256)));
    }
    match f.unpack {
        Unpack::None => std::fs::rename(&part, dir.join(f.name))?,
        Unpack::TarGz(members) => {
            let bytes = std::fs::read(&part)?;
            let entries = studio_nn::zip::untar(&bytes);
            for &(member, dest, sha, _) in members {
                let (_, data) = entries.iter().find(|(p, _)| p.trim_start_matches("./") == member).ok_or_else(|| Error::Download(format!("{}: {member} missing from archive", f.name)))?;
                if hex(&sha256(data)) != sha {
                    return Err(Error::Download(format!("{}: {member} checksum mismatch", f.name)));
                }
                let tmp = dir.join(format!("{dest}.part"));
                std::fs::write(&tmp, data)?;
                std::fs::rename(tmp, dir.join(dest))?;
            }
            std::fs::remove_file(&part)?;
        }
    }
    Ok(())
}

/// Fetches a URL into a file, appending to what is already there.
/// `progress(bytes_in_file)` returns `false` to cancel.
pub(crate) trait Fetch {
    fn fetch(&self, url: &str, dest: &Path, progress: &mut dyn FnMut(u64) -> bool) -> Result<()>;
}

/// HTTPS in Rust (`studio-net`): the OS certificate store, redirects,
/// resume of a partial file, retries, and a stall limit.
struct Net;

impl Fetch for Net {
    fn fetch(&self, url: &str, dest: &Path, progress: &mut dyn FnMut(u64) -> bool) -> Result<()> {
        let opts = studio_net::Download { stall: Duration::from_secs(60), ..Default::default() };
        studio_net::download(url, dest, opts, progress).map_err(|e| match e {
            studio_net::Failure::Cancelled => Error::Cancelled,
            studio_net::Failure::Failed(m) => Error::Download(format!("{url}: {m}")),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::ModelSpec;

    /// Copies from a local file (the URL is a path), in small steps so
    /// progress and cancellation are exercised.
    struct LocalFetch;
    impl Fetch for LocalFetch {
        fn fetch(&self, url: &str, dest: &Path, progress: &mut dyn FnMut(u64) -> bool) -> Result<()> {
            let data = std::fs::read(url)?;
            let mut have = std::fs::read(dest).unwrap_or_default();
            while have.len() < data.len() {
                let n = (have.len() + 1000).min(data.len());
                have.extend_from_slice(&data[have.len()..n]);
                std::fs::write(dest, &have)?;
                if !progress(have.len() as u64) {
                    return Err(Error::Cancelled);
                }
            }
            Ok(())
        }
    }

    fn leak(s: String) -> &'static str {
        Box::leak(s.into_boxed_str())
    }

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("studio-ai-store-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn spec_for(src: &Path, data: &[u8], sha: Option<&str>) -> &'static ModelSpec {
        std::fs::write(src, data).unwrap();
        let files: &'static [FileSpec] = Box::leak(Box::new([FileSpec {
            url: leak(src.display().to_string()),
            name: "weights.onnx",
            sha256: leak(sha.map(str::to_string).unwrap_or_else(|| hex(&sha256(data)))),
            size: data.len() as u64,
            unpack: Unpack::None,
        }]));
        Box::leak(Box::new(ModelSpec { id: ModelId::Modnet, dir: "fake", name: "Fake", used_for: "Tests", licence: "MIT", source: "", files }))
    }

    #[test]
    fn download_verifies_and_installs() {
        let d = tmp("ok");
        let data: Vec<u8> = (0..10_000u32).map(|i| (i * 31) as u8).collect();
        let spec = spec_for(&d.join("src.bin"), &data, None);
        let store = ModelStore::new(d.join("models"));
        let ctx = JobCtx::detached();
        store.download_spec(spec, &LocalFetch, &ctx).unwrap();
        assert_eq!(std::fs::read(d.join("models/fake/weights.onnx")).unwrap(), data);
        assert!(!d.join("models/fake/weights.onnx.part").exists());
        assert_eq!(ctx.progress(), 1.0);
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn checksum_mismatch_is_rejected_and_cleaned_up() {
        let d = tmp("bad");
        let spec = spec_for(&d.join("src.bin"), b"not the model", Some(&"0".repeat(64)));
        let store = ModelStore::new(d.join("models"));
        let e = store.download_spec(spec, &LocalFetch, &JobCtx::detached()).unwrap_err();
        assert!(matches!(e, Error::Download(m) if m.contains("checksum")));
        assert!(!d.join("models/fake/weights.onnx").exists());
        assert!(!d.join("models/fake/weights.onnx.part").exists());
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn cancel_leaves_a_resumable_part() {
        let d = tmp("cancel");
        let data = vec![7u8; 5000];
        let spec = spec_for(&d.join("src.bin"), &data, None);
        let store = ModelStore::new(d.join("models"));
        let ctx = JobCtx::detached();
        ctx.cancel();
        assert_eq!(store.download_spec(spec, &LocalFetch, &ctx), Err(Error::Cancelled));
        let part = d.join("models/fake/weights.onnx.part");
        assert_eq!(std::fs::metadata(&part).unwrap().len(), 1000);
        // resume
        store.download_spec(spec, &LocalFetch, &JobCtx::detached()).unwrap();
        assert_eq!(std::fs::read(d.join("models/fake/weights.onnx")).unwrap(), data);
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn status_of_an_empty_folder() {
        let d = tmp("status");
        let store = ModelStore::new(&d);
        for id in ModelId::ALL {
            assert_eq!(store.status(id), ModelStatus::NotInstalled);
            assert_eq!(store.require(id), Err(Error::ModelMissing(id)));
        }
        // one of two Whisper files present → incomplete
        std::fs::create_dir_all(store.dir(ModelId::WhisperTiny)).unwrap();
        std::fs::write(store.file(ModelId::WhisperTiny, "generation_config.json"), vec![0u8; 3772]).unwrap();
        assert!(matches!(store.status(ModelId::WhisperTiny), ModelStatus::Incomplete { missing } if missing.len() == 3));
        let bad = store.verify(ModelId::WhisperTiny, &JobCtx::detached()).unwrap();
        assert_eq!(bad.len(), 4);
        let _ = std::fs::remove_dir_all(d);
    }
}
