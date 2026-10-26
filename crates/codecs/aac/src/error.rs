// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Error type shared by every part of the decoder.

use std::fmt;

/// Why a configuration or an access unit could not be decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The bitstream ended before the syntax it announced was complete.
    Truncated,
    /// The bitstream violates ISO/IEC 14496-3 (illegal code word, value out of
    /// range, inconsistent element, ...).
    InvalidData(&'static str),
    /// Valid MPEG-4 audio that this decoder does not implement (Main/LTP/SSR
    /// tools, 960-sample frames, ER object types, ...).
    Unsupported(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Truncated => f.write_str("aac: bitstream truncated"),
            Error::InvalidData(what) => write!(f, "aac: invalid data: {what}"),
            Error::Unsupported(what) => write!(f, "aac: unsupported: {what}"),
        }
    }
}

impl std::error::Error for Error {}

/// Result alias used throughout the crate.
pub type Result<T> = std::result::Result<T, Error>;

/// Shorthand for an [`Error::InvalidData`] early return.
macro_rules! invalid {
    ($msg:expr) => {
        return Err($crate::error::Error::InvalidData($msg))
    };
}
pub(crate) use invalid;
