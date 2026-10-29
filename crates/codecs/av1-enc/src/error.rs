// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

/// Encoder errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The feature or module is not built yet (or not supported by this configuration).
    Unsupported(&'static str),
    /// The input or configuration is invalid.
    Invalid(&'static str),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Unsupported(s) => write!(f, "unsupported AV1 encoder feature: {s}"),
            Error::Invalid(s) => write!(f, "invalid AV1 encoder input: {s}"),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;
