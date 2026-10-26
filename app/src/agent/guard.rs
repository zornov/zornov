// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A tool call that panics reports why. The panic's message and the place
//! it came from are kept (the panic hook records them on the thread that
//! panicked) so the person and the AI read the cause, not just "crashed".

use std::cell::RefCell;
use std::panic::{AssertUnwindSafe, catch_unwind};

thread_local! {
    static LAST: RefCell<Option<String>> = const { RefCell::new(None) };
}

fn install() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let at = info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default();
            LAST.with(|l| *l.borrow_mut() = Some(at));
            prev(info);
        }));
    });
}

/// The words for a caught panic.
pub fn describe(tool: &str, payload: &(dyn std::any::Any + Send)) -> String {
    let msg = payload.downcast_ref::<&str>().map(|s| (*s).to_owned()).or_else(|| payload.downcast_ref::<String>().cloned()).unwrap_or_else(|| "no message".into());
    let at = LAST.with(|l| l.borrow_mut().take()).filter(|a| !a.is_empty());
    let msg: String = msg.chars().take(400).collect();
    format!("{tool} hit a bug in Studio and stopped: {msg}{}. The project is unchanged; this is not caused by the input, so report it.", at.map(|a| format!(" (at {a})")).unwrap_or_default())
}

/// Whether an error is a caught panic.
pub fn is_bug(e: &str) -> bool {
    e.contains(" hit a bug in Studio and stopped: ")
}

/// Runs a tool call; a panic becomes an error that says what panicked.
pub fn run<T>(tool: &str, f: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    install();
    catch_unwind(AssertUnwindSafe(f)).unwrap_or_else(|p| Err(describe(tool, p.as_ref())))
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_panic_says_what_and_where() {
        let r: Result<(), String> = super::run("download", || {
            let s = "ab\u{fffd}";
            let _ = &s[..3];
            Ok(())
        });
        let e = r.unwrap_err();
        assert!(e.starts_with("download hit a bug in Studio and stopped: ") && e.contains("char boundary") && e.contains("guard.rs"), "{e}");
        assert_eq!(super::run("x", || Ok::<_, String>(3)), Ok(3));
        assert_eq!(super::run::<()>("x", || Err("plain".into())), Err("plain".into()));
    }
}
