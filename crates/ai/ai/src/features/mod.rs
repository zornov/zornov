// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The AI features. Each one loads its model through the [`ModelStore`]
//! (returning [`Error::ModelMissing`] when it is not installed) or works
//! without a model; [`Feature::availability`] tells the UI which case
//! applies so it can show a download button instead of failing.
//!
//! [`Error::ModelMissing`]: crate::Error::ModelMissing

pub mod beats;
pub mod bpe;
pub mod denoise;
pub mod depth;
pub mod detect;
pub mod embed;
pub mod faces;
pub mod fill;
pub mod hearing;
pub mod image23d;
pub mod interpolate;
pub mod look;
pub mod matte_file;
pub mod matting;
pub mod ocr;
pub mod parallax;
pub mod parts;
pub mod reframe;
pub mod regions;
pub mod relight;
pub mod restore;
pub mod roto;
pub mod scenes;
pub mod segment;
pub mod silence;
pub mod track;
pub mod transcribe;
pub mod unigram;
pub mod upscale;

use crate::registry::ModelId;
use crate::store::ModelStore;

/// Every AI feature, as listed in the AI panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Feature {
    Captions,
    RemoveBackground,
    ObjectCutout,
    VoiceCleanup,
    SmoothSlowMotion,
    Depth,
    FaceTracking,
    BodyTracking,
    SceneCuts,
    AutoReframe,
    SilenceRemoval,
    FillerRemoval,
    BeatSync,
    Upscale,
}

/// Whether a feature can run with the installed models.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Availability {
    /// Runs at full quality.
    Ready,
    /// Runs with a classical method; installing `better` improves it.
    Fallback { better: ModelId },
    /// Needs this model first (the button offers the download).
    NeedsModel(ModelId),
}

impl Feature {
    pub const ALL: [Feature; 14] = [
        Feature::Captions,
        Feature::RemoveBackground,
        Feature::ObjectCutout,
        Feature::VoiceCleanup,
        Feature::SmoothSlowMotion,
        Feature::Depth,
        Feature::FaceTracking,
        Feature::BodyTracking,
        Feature::SceneCuts,
        Feature::AutoReframe,
        Feature::SilenceRemoval,
        Feature::FillerRemoval,
        Feature::BeatSync,
        Feature::Upscale,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Feature::Captions => "Captions",
            Feature::RemoveBackground => "Remove background",
            Feature::ObjectCutout => "Object cut-out",
            Feature::VoiceCleanup => "Voice cleanup",
            Feature::SmoothSlowMotion => "Smooth slow motion",
            Feature::Depth => "Depth effects",
            Feature::FaceTracking => "Face tracking",
            Feature::BodyTracking => "Body tracking",
            Feature::SceneCuts => "Split at scene cuts",
            Feature::AutoReframe => "Auto reframe",
            Feature::SilenceRemoval => "Remove silences",
            Feature::FillerRemoval => "Remove filler words",
            Feature::BeatSync => "Beat sync",
            Feature::Upscale => "Upscale and enhance",
        }
    }

    pub fn availability(self, store: &ModelStore) -> Availability {
        let need = |id: ModelId| if store.is_installed(id) { Availability::Ready } else { Availability::NeedsModel(id) };
        let better = |id: ModelId| if store.is_installed(id) { Availability::Ready } else { Availability::Fallback { better: id } };
        let any_whisper = || {
            if transcribe::WHISPER_MODELS.iter().any(|&m| store.is_installed(m)) { Availability::Ready } else { Availability::NeedsModel(ModelId::WhisperTiny) }
        };
        match self {
            Feature::Captions | Feature::FillerRemoval => any_whisper(),
            Feature::RemoveBackground => need(ModelId::Modnet),
            Feature::ObjectCutout => need(ModelId::Sam21Tiny),
            Feature::VoiceCleanup => better(ModelId::DeepFilterNet3),
            Feature::SmoothSlowMotion => better(ModelId::Rife49),
            Feature::Depth => need(ModelId::DepthAnythingV2Small),
            Feature::FaceTracking => need(ModelId::FaceDetector),
            Feature::BodyTracking => need(ModelId::PoseLandmarker),
            Feature::AutoReframe => better(ModelId::FaceDetector),
            Feature::Upscale => better(ModelId::RealEsrganGeneralX4v3),
            Feature::SceneCuts | Feature::SilenceRemoval | Feature::BeatSync => Availability::Ready,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn availability_without_models() {
        let store = ModelStore::new(std::env::temp_dir().join("studio-ai-no-models"));
        for f in Feature::ALL {
            let a = f.availability(&store);
            if matches!(f, Feature::SceneCuts | Feature::SilenceRemoval | Feature::BeatSync) {
                assert_eq!(a, Availability::Ready);
                continue;
            }
            assert_ne!(a, Availability::Ready, "{} should not be fully ready", f.name());
            if let Availability::NeedsModel(m) | Availability::Fallback { better: m } = a {
                assert!(!m.spec().files.is_empty());
            }
        }
        // model-free features still run
        let n = Feature::ALL.iter().filter(|f| matches!(f.availability(&store), Availability::Fallback { .. })).count();
        assert_eq!(n, 4);
    }
}
