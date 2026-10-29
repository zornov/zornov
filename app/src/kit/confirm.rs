// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Confirmation for what undo cannot cover: a button that asks for a second
//! click instead of a dialog. Use it only for actions that cannot be undone
//! (deleting files, removing a saved key); everything else gets an Undo toast.
//!
//! ```ignore
//! let label = if kit::armed(ctx, id) { "Click again to remove" } else { "Remove" };
//! if button(label).clicked() && kit::confirm_click(ctx, id) { remove(); }
//! ```

use eframe::egui::{Context, Id};

/// Seconds the second click stays valid.
pub const WINDOW: f64 = 4.0;

#[derive(Clone, Copy, Default)]
struct Armed(Option<(Id, f64)>);

fn check(a: &mut Armed, id: Id, now: f64) -> bool {
    match a.0 {
        Some((who, at)) if who == id && now - at < WINDOW => {
            a.0 = None;
            true
        }
        _ => {
            a.0 = Some((id, now));
            false
        }
    }
}

/// Call on a click. The first click arms and returns false; a second click on
/// the same `id` within [`WINDOW`] returns true.
pub fn confirm_click(ctx: &Context, id: Id) -> bool {
    let now = ctx.input(|i| i.time);
    ctx.request_repaint_after(std::time::Duration::from_secs_f64(WINDOW + 0.1));
    ctx.data_mut(|d| check(d.get_temp_mut_or_default::<Armed>(Id::new("kit-confirm")), id, now))
}

/// Whether `id` is waiting for its second click (draw the "click again" label).
pub fn armed(ctx: &Context, id: Id) -> bool {
    let now = ctx.input(|i| i.time);
    ctx.data(|d| d.get_temp::<Armed>(Id::new("kit-confirm")).and_then(|a| a.0)).is_some_and(|(who, at)| who == id && now - at < WINDOW)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_click_confirms_within_the_window() {
        let mut a = Armed::default();
        let id = Id::new("x");
        assert!(!check(&mut a, id, 0.0));
        assert!(check(&mut a, id, 1.0));
        assert!(!check(&mut a, id, 1.5), "confirming disarms");
    }

    #[test]
    fn late_or_other_clicks_only_arm() {
        let mut a = Armed::default();
        assert!(!check(&mut a, Id::new("a"), 0.0));
        assert!(!check(&mut a, Id::new("b"), 1.0), "another button re-arms");
        assert!(!check(&mut a, Id::new("b"), 1.0 + WINDOW + 1.0), "too late");
        assert!(check(&mut a, Id::new("b"), 1.0 + WINDOW + 2.0));
    }
}
