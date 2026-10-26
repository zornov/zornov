// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Error type for the HEVC decoder.

use std::fmt;

/// Errors returned by the decoder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The bitstream ended before a syntax structure was complete.
    UnexpectedEof,
    /// A syntax element has a value that is not allowed by the specification.
    Invalid(&'static str),
    /// A feature that this decoder does not implement (e.g. 4:2:2, range extensions).
    Unsupported(&'static str),
    /// A parameter set referenced by a slice is missing.
    MissingParameterSet(&'static str),
    /// Internal decoder failure (a bug); the decoder state was reset.
    Internal(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::UnexpectedEof => write!(f, "hevc: unexpected end of bitstream"),
            Error::Invalid(s) => write!(f, "hevc: invalid bitstream: {s}"),
            Error::Unsupported(s) => write!(f, "hevc: unsupported feature: {s}"),
            Error::MissingParameterSet(s) => write!(f, "hevc: missing parameter set: {s}"),
            Error::Internal(s) => write!(f, "hevc: internal error: {s}"),
        }
    }
}

impl std::error::Error for Error {}

/// Result alias.
pub type Result<T> = std::result::Result<T, Error>;
