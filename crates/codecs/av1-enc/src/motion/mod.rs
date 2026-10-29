// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

#![allow(unused_variables, dead_code)] // stubs: the owning module removes this when it implements the file

//! M7 (phase b): motion search.

pub mod mvpred;

/// A motion vector in 1/8 pel units: `[row, col]`.
pub type Mv = [i32; 2];
