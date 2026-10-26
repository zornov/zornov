// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Settings ▸ Models and the AI panel's availability, from `studio-ai`.
//!
//! The registry lists every model (name, use, licence, source, download
//! size); the store says what is installed in `~/Movies/Studio/Models`.
//! Downloads run on `studio_ai::Jobs` worker threads: the store fetches each
//! file, checks its SHA-256 and unpacks archives; a wrong checksum fails the
//! job and leaves nothing half-installed. Nothing here touches the network
//! unless `download` is called (only on a user click).

use studio_ai::features::{Availability, Feature};
use studio_ai::registry::{self, ModelId};
use studio_ai::{JobHandle, JobState, Jobs, ModelStatus, ModelStore};

/// One row of Settings ▸ Models.
#[derive(Clone, Debug, PartialEq)]
pub struct ModelRow {
    pub id: ModelId,
    pub name: &'static str,
    pub used_for: &'static str,
    pub licence: &'static str,
    pub source: &'static str,
    /// Bytes to download.
    pub size: u64,
    /// `size` as "148 MB".
    pub size_label: String,
    pub status: ModelStatus,
    /// Bytes on disk now.
    pub on_disk: u64,
}

/// One AI feature as the AI panel shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct FeatureRow {
    pub feature: Feature,
    pub name: &'static str,
    /// Runs now (with its model, or with no model needed).
    pub ready: bool,
    /// The model to offer for download, if any.
    pub model: Option<ModelId>,
    /// One line for the tile: "Ready", "Works now; install X for better
    /// results", "Needs X (148 MB)".
    pub note: String,
}

/// Owns the store and the download workers.
pub struct Models {
    pub store: ModelStore,
    jobs: Jobs,
    downloads: Vec<(ModelId, JobHandle<()>)>,
    verifies: Vec<(ModelId, JobHandle<Vec<String>>)>,
    verified: Vec<(ModelId, VerifyState)>,
}

/// Result of checking a model's files against their pinned SHA-256.
#[derive(Clone, Debug, PartialEq)]
pub enum VerifyState {
    Running { fraction: f32 },
    /// Every file is present and matches.
    Ok,
    /// These files are missing or do not match (a repair re-downloads them).
    Damaged(Vec<String>),
    Failed(String),
}

/// Progress of a download, polled each frame.
#[derive(Clone, Debug, PartialEq)]
pub enum DownloadState {
    Running { fraction: f32, message: String },
    Done,
    Failed(String),
    Cancelled,
}

impl Default for Models {
    fn default() -> Self {
        Self::new(ModelStore::default())
    }
}

impl Models {
    pub fn new(store: ModelStore) -> Self {
        Self { store, jobs: Jobs::new(1), downloads: Vec::new(), verifies: Vec::new(), verified: Vec::new() }
    }

    /// Every model with its install state (reads the models folder: cheap,
    /// but not free; call when the screen opens or after a job ends).
    pub fn list(&self) -> Vec<ModelRow> {
        registry::all()
            .iter()
            .map(|s| ModelRow {
                id: s.id,
                name: s.name,
                used_for: s.used_for,
                licence: s.licence,
                source: s.source,
                size: s.download_size(),
                size_label: registry::human_size(s.download_size()),
                status: self.store.status(s.id),
                on_disk: self.store.disk_usage(s.id),
            })
            .collect()
    }

    /// Start downloading (or repairing) a model. Network access: only on a
    /// user click. A second call while one runs for the same model does
    /// nothing.
    pub fn download(&mut self, id: ModelId) {
        if self.downloads.iter().any(|(m, h)| *m == id && !h.is_finished()) {
            return;
        }
        let store = self.store.clone();
        let h = self.jobs.spawn(format!("Download {}", id.spec().name), move |ctx| store.download(id, ctx));
        self.downloads.retain(|(m, _)| *m != id);
        self.downloads.push((id, h));
    }

    /// State of the latest download of `id`, if one was started.
    pub fn download_state(&self, id: ModelId) -> Option<DownloadState> {
        let (_, h) = self.downloads.iter().find(|(m, _)| *m == id)?;
        Some(match h.state() {
            JobState::Done | JobState::Cached => DownloadState::Done,
            JobState::Failed(e) => DownloadState::Failed(e.to_string()),
            JobState::Cancelled => DownloadState::Cancelled,
            _ => DownloadState::Running { fraction: h.progress(), message: h.message() },
        })
    }

    /// Check every installed file of `id` against its pinned SHA-256, in
    /// the background. No network.
    pub fn verify(&mut self, id: ModelId) {
        if self.verifies.iter().any(|(m, h)| *m == id && !h.is_finished()) {
            return;
        }
        let store = self.store.clone();
        let h = self.jobs.spawn(format!("Verify {}", id.spec().name), move |ctx| store.verify(id, ctx));
        self.verifies.retain(|(m, _)| *m != id);
        self.verified.retain(|(m, _)| *m != id);
        self.verifies.push((id, h));
    }

    /// State of the latest check of `id`, if one was started.
    pub fn verify_state(&mut self, id: ModelId) -> Option<VerifyState> {
        if let Some(i) = self.verifies.iter().position(|(m, _)| *m == id) {
            let h = &self.verifies[i].1;
            if !h.is_finished() {
                return Some(VerifyState::Running { fraction: h.progress() });
            }
            let done = match h.take_result() {
                Some(Ok(bad)) if bad.is_empty() => VerifyState::Ok,
                Some(Ok(bad)) => VerifyState::Damaged(bad),
                Some(Err(e)) => VerifyState::Failed(e.to_string()),
                None => VerifyState::Failed("cancelled".into()),
            };
            self.verifies.remove(i);
            self.verified.retain(|(m, _)| *m != id);
            self.verified.push((id, done));
        }
        self.verified.iter().find(|(m, _)| *m == id).map(|(_, s)| s.clone())
    }

    /// Forget a check (after a repair or a removal); a running one stops.
    pub fn clear_verify(&mut self, id: ModelId) {
        for (_, h) in self.verifies.iter().filter(|(m, _)| *m == id) {
            h.cancel();
        }
        self.verifies.retain(|(m, _)| *m != id);
        self.verified.retain(|(m, _)| *m != id);
    }

    pub fn cancel(&self, id: ModelId) {
        if let Some((_, h)) = self.downloads.iter().find(|(m, _)| *m == id) {
            h.cancel();
        }
    }

    /// Delete an installed model.
    pub fn remove(&self, id: ModelId) -> Result<(), String> {
        self.store.remove(id).map_err(|e| e.to_string())
    }

    /// Every feature with what it needs.
    pub fn features(&self) -> Vec<FeatureRow> {
        Feature::ALL.iter().map(|&f| feature_with(&self.store, f)).collect()
    }
}

/// A feature's row against the default models folder.
pub fn feature(f: Feature) -> FeatureRow {
    static STORE: std::sync::OnceLock<ModelStore> = std::sync::OnceLock::new();
    feature_with(STORE.get_or_init(ModelStore::default), f)
}

fn feature_with(store: &ModelStore, f: Feature) -> FeatureRow {
    let label = |id: ModelId| format!("{} ({})", id.spec().name, registry::human_size(id.spec().download_size()));
    let (ready, model, note) = match f.availability(store) {
        Availability::Ready => (true, None, "Ready".to_owned()),
        Availability::Fallback { better } => (true, Some(better), format!("Works now; install {} for better results", label(better))),
        Availability::NeedsModel(id) => (false, Some(id), format!("Needs {}", label(id))),
    };
    FeatureRow { feature: f, name: f.name(), ready, model, note }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_store() -> Models {
        Models::new(ModelStore::new(std::env::temp_dir().join(format!("studio-app-models-{}", std::process::id()))))
    }

    #[test]
    fn lists_every_registered_model_as_not_installed() {
        let m = empty_store();
        let rows = m.list();
        assert_eq!(rows.len(), registry::all().len());
        for r in &rows {
            assert_eq!(r.status, ModelStatus::NotInstalled, "{}", r.name);
            assert!(r.size > 0 && !r.licence.is_empty() && !r.source.is_empty(), "{}", r.name);
            assert!(r.size_label.ends_with("B"), "{}", r.size_label);
        }
        assert!(m.download_state(rows[0].id).is_none());
        // Removing something that is not there is fine.
        m.remove(rows[0].id).unwrap();
    }

    #[test]
    fn features_say_what_they_need() {
        let m = empty_store();
        let rows = m.features();
        assert_eq!(rows.len(), Feature::ALL.len());
        let scene = rows.iter().find(|r| r.feature == Feature::SceneCuts).unwrap();
        assert!(scene.ready && scene.model.is_none());
        let bg = rows.iter().find(|r| r.feature == Feature::RemoveBackground).unwrap();
        assert!(!bg.ready && bg.model.is_some() && bg.note.starts_with("Needs "));
        let voice = rows.iter().find(|r| r.feature == Feature::VoiceCleanup).unwrap();
        assert!(voice.ready && voice.note.starts_with("Works now"));
    }

    #[test]
    fn verify_reports_missing_files_and_can_be_cleared() {
        let mut m = empty_store();
        let id = studio_ai::ModelId::FaceDetector;
        m.verify(id);
        let t = std::time::Instant::now();
        let state = loop {
            match m.verify_state(id) {
                Some(VerifyState::Running { .. }) if t.elapsed().as_secs() < 30 => std::thread::sleep(std::time::Duration::from_millis(20)),
                other => break other,
            }
        };
        assert!(matches!(&state, Some(VerifyState::Damaged(bad)) if !bad.is_empty()), "{state:?}");
        m.clear_verify(id);
        assert_eq!(m.verify_state(id), None);
    }
}
