// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Why a page's media could not be brought in, in plain words.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The page does not openly expose any media Studio can take.
    NotExposed(String),
    /// The media is protected (DRM, a licence, a cipher): Studio does not
    /// work around that.
    Protected(String),
    /// The server wants a signed-in person or a licence.
    SignIn(String),
    /// A format Studio does not take apart yet.
    Unsupported(String),
    /// The network failed.
    Net(String),
    /// Something in the playlist or the segments is broken.
    Bad(String),
    /// The caller stopped it. What was fetched stays for a later resume.
    Cancelled,
    /// A file could not be written.
    Io(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NotExposed(s) | Error::Protected(s) | Error::SignIn(s) | Error::Unsupported(s) | Error::Net(s) | Error::Bad(s) | Error::Io(s) => f.write_str(s),
            Error::Cancelled => f.write_str("the download was cancelled"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e.to_string())
    }
}

impl Error {
    /// How much a failure should count when several routes failed: the
    /// most informative one is reported.
    pub(crate) fn weight(&self) -> u8 {
        match self {
            Error::Protected(_) => 6,
            Error::SignIn(_) => 5,
            Error::Unsupported(_) => 4,
            Error::Bad(_) => 3,
            Error::Net(_) | Error::Io(_) => 2,
            Error::NotExposed(_) => 1,
            Error::Cancelled => 9,
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;
