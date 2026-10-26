// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Error type shared by every parser stage.

use std::fmt;
use std::io;

/// Everything that can go wrong while demuxing.
#[derive(Debug)]
pub enum Error {
    /// The underlying reader failed.
    Io(io::Error),
    /// A box or field ended before its declared contents (`what` names it).
    Truncated(&'static str),
    /// A field holds a value that is impossible or inconsistent.
    Malformed(String),
    /// A mandatory box is absent.
    Missing(&'static str),
    /// A size or count exceeds the demuxer's safety limits.
    TooLarge(&'static str),
}

/// `Result` alias using [`Error`].
pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub(crate) fn malformed(msg: impl Into<String>) -> Self {
        Error::Malformed(msg.into())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "I/O error: {e}"),
            Error::Truncated(what) => write!(f, "truncated {what}"),
            Error::Malformed(msg) => write!(f, "malformed file: {msg}"),
            Error::Missing(what) => write!(f, "missing mandatory box {what}"),
            Error::TooLarge(what) => write!(f, "{what} exceeds safety limits"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        if e.kind() == io::ErrorKind::UnexpectedEof {
            Error::Truncated("file")
        } else {
            Error::Io(e)
        }
    }
}
