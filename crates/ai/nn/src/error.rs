// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Error type shared by the whole crate.

use std::fmt;

#[derive(Clone, Debug, PartialEq)]
pub struct Error(pub String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error(format!("io: {e}"))
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// `bail!("fmt", args)` returns an `Err(Error(..))`.
#[macro_export]
macro_rules! bail {
    ($($t:tt)*) => { return Err($crate::error::Error(format!($($t)*))) };
}

/// `ensure!(cond, "fmt", args)`.
#[macro_export]
macro_rules! ensure {
    ($c:expr, $($t:tt)*) => { if !$c { $crate::bail!($($t)*); } };
}

pub fn err<T>(msg: impl Into<String>) -> Result<T> {
    Err(Error(msg.into()))
}
