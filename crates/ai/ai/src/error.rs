// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Error type for every AI feature.

use crate::registry::ModelId;
use std::fmt;

#[derive(Clone, Debug, PartialEq)]
pub enum Error {
    /// The feature needs a model that is not installed. The UI shows
    /// [`ModelId::spec`]'s name and size and offers the download; nothing else
    /// breaks.
    ModelMissing(ModelId),
    /// The job was cancelled by the user.
    Cancelled,
    /// The inference engine failed (bad model file, unsupported op, ...).
    Inference(String),
    /// A download failed (network, HTTP status, checksum mismatch).
    Download(String),
    Io(String),
    /// The caller passed something unusable (empty frame, wrong sizes, ...).
    Invalid(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::ModelMissing(id) => {
                let s = id.spec();
                write!(f, "{} needs the model \"{}\" ({}); download it in Settings > AI models", s.used_for, s.name, crate::registry::human_size(s.download_size()))
            }
            Error::Cancelled => f.write_str("cancelled"),
            Error::Inference(m) => write!(f, "inference: {m}"),
            Error::Download(m) => write!(f, "download: {m}"),
            Error::Io(m) => write!(f, "io: {m}"),
            Error::Invalid(m) => write!(f, "invalid input: {m}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<studio_nn::Error> for Error {
    fn from(e: studio_nn::Error) -> Self {
        Error::Inference(e.0)
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;

pub(crate) fn invalid<T>(msg: impl Into<String>) -> Result<T> {
    Err(Error::Invalid(msg.into()))
}
