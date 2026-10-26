// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Error type for the decoder.

use std::fmt;

/// Errors produced while decoding an H.264 stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The bitstream is malformed (syntax element out of range, truncated data, ...).
    Bitstream(&'static str),
    /// A valid stream that uses a feature this decoder does not implement.
    Unsupported(String),
    /// A slice references a parameter set that has not been received.
    MissingParameterSet(&'static str),
    /// Configuration (avcC) could not be parsed.
    InvalidConfig(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Bitstream(s) => write!(f, "h264: malformed bitstream: {s}"),
            Error::Unsupported(s) => write!(f, "h264: unsupported stream: {s}"),
            Error::MissingParameterSet(s) => write!(f, "h264: missing parameter set: {s}"),
            Error::InvalidConfig(s) => write!(f, "h264: invalid decoder configuration: {s}"),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

/// Return `Err(Error::Bitstream(msg))` unless `cond` holds.
#[inline]
pub(crate) fn check(cond: bool, msg: &'static str) -> Result<()> {
    if cond { Ok(()) } else { Err(Error::Bitstream(msg)) }
}
