// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Localisation glue for the interface (see the `studio-i18n` crate).
//!
//! How text gets translated here:
//! * Every shared widget in `kit` runs the text it is given through
//!   [`t`] (English source text to the UI language), so a plain
//!   `Button::primary("Export")` is already translated. Add the English
//!   text to `crates/project/i18n/locales/en.ftl` and it shows up everywhere.
//! * Text that is built at run time (counts, names, errors) goes through
//!   [`tr_args`] with a message id so word order and plurals can differ per
//!   language: `tr_args("clips-selected", &[("n", n.into())])`.
//! * Anything handed straight to egui's painter or `RichText` must go
//!   through [`shown`] so right-to-left text is shaped and ordered.
//!
//! See `docs/localisation.md`.

use eframe::egui::{self, Align2, Rect};

pub use studio_i18n::{id_for_source, is_rtl, lang, set_lang, shown, t, tr, tr_args, tr_in, visual, Lang, Value};

/// The x coordinate `offset` in from the start edge of `r`: the left edge
/// in left-to-right languages, the right edge in right-to-left ones.
pub fn start_x(r: Rect, offset: f32) -> f32 {
    if is_rtl() { r.right() - offset } else { r.left() + offset }
}

/// The x coordinate `offset` in from the end edge of `r`.
pub fn end_x(r: Rect, offset: f32) -> f32 {
    if is_rtl() { r.left() + offset } else { r.right() - offset }
}

/// Text alignment at the start edge, vertically centred.
pub fn start_center() -> Align2 {
    if is_rtl() { Align2::RIGHT_CENTER } else { Align2::LEFT_CENTER }
}

/// Text alignment at the end edge, vertically centred.
pub fn end_center() -> Align2 {
    if is_rtl() { Align2::LEFT_CENTER } else { Align2::RIGHT_CENTER }
}

/// Layout for a row whose first item sits at the start edge.
pub fn row_layout() -> egui::Layout {
    if is_rtl() {
        egui::Layout::right_to_left(egui::Align::Center)
    } else {
        egui::Layout::left_to_right(egui::Align::Center)
    }
}

/// Mirror `r` horizontally inside `outer` (used for panel placement).
pub fn mirror(r: Rect, outer: Rect) -> Rect {
    Rect::from_min_max(egui::pos2(outer.left() + outer.right() - r.right(), r.top()), egui::pos2(outer.left() + outer.right() - r.left(), r.bottom()))
}

/// `1 clip`, `3 clips`: message `id` with a count in `n`.
pub fn count(id: &str, n: usize) -> String {
    tr_args(id, &[("n", n.into())]).into_owned()
}

/// Translate text built with `format!`: `tf(format!("Undid {}", what))`.
/// Known patterns (see `t_fmt` in `studio-i18n`) are translated and
/// reordered for the UI language; anything else is returned as it is.
pub fn tf(s: String) -> String {
    match studio_i18n::t_fmt(&s) {
        std::borrow::Cow::Borrowed(b) if b.len() == s.len() && b == s => s,
        other => other.into_owned(),
    }
}
