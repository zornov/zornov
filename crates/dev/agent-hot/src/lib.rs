// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Agent tools a development build of Zornov swaps in while it runs.
//!
//! The `app` crate is large, so a fix there takes minutes to rebuild. A
//! tool that lives here rebuilds in seconds (`cargo build -p
//! zornov-agent-hot`), and `zornov dev swap --retry N` loads it into the
//! open window (an `agent-dev` build) and runs call N again, while the
//! agent inside carries on.
//!
//! The window loads this library only from this checkout's `target/debug`,
//! only when its SHA-256 matches what `zornov dev swap` built, and only
//! from a private copy. A tool here gets JSON in and gives JSON out: it
//! never touches the project. It can only take over a tool the app
//! already has (the app checks the list) and reword arguments before the
//! app's own tool sees them, so every change still goes through the app's
//! gate, its argument checks and its undo.
//!
//! The C ABI (version 1):
//! * `zornov_hot_abi() -> u32`: 1.
//! * `zornov_hot_tools() -> *mut c_char`: JSON list of the tools taken over.
//! * `zornov_hot_call(tool, args) -> *mut c_char`: `{"handled", "ok", "text" | "error"}`.
//! * `zornov_hot_normalise(tool, args) -> *mut c_char`: new arguments, or null.
//! * `zornov_hot_free(p)`: frees a string this library returned.
//!
//! Here so far: `feedback`, and argument aliases for `edit` ops (the
//! vocabulary gaps `zornov trace report` finds, fixed in seconds; move each
//! into `app/src/agent/args.rs` once it has proved itself). Still in the
//! app, to move next: `edit_schema`'s op checks, `capabilities`, `howto`,
//! and the op field tables of `args.rs`, which are pure data and functions.

use serde_json::{Value, json};
use std::ffi::{CStr, CString, c_char};

/// The ABI this library speaks; the app refuses any other.
pub const ABI: u32 = 1;

/// Field names agents reach for, and the ones Zornov uses: (op or `*`, sent, meant).
const ALIASES: &[(&str, &str, &str)] = &[("text", "txt", "text"), ("*", "color", "colour")];

/// The tools taken over.
const TOOLS: &[&str] = &["feedback"];

fn out(v: &Value) -> *mut c_char {
    CString::new(v.to_string()).map_or(std::ptr::null_mut(), CString::into_raw)
}

/// # Safety
/// `p` is null or a NUL-terminated string that lives for the call.
unsafe fn arg<'a>(p: *const c_char) -> Option<&'a str> {
    if p.is_null() {
        return None;
    }
    unsafe { CStr::from_ptr(p) }.to_str().ok()
}

#[unsafe(no_mangle)]
pub extern "C" fn zornov_hot_abi() -> u32 {
    ABI
}

#[unsafe(no_mangle)]
pub extern "C" fn zornov_hot_tools() -> *mut c_char {
    out(&json!(TOOLS))
}

/// # Safety
/// `tool` and `args` are NUL-terminated strings; the result is freed with
/// [`zornov_hot_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn zornov_hot_call(tool: *const c_char, args: *const c_char) -> *mut c_char {
    let (tool, args) = unsafe { (arg(tool), arg(args)) };
    let r = std::panic::catch_unwind(|| {
        let (Some(tool), Some(args)) = (tool, args) else { return json!({"handled": false}) };
        let args: Value = serde_json::from_str(args).unwrap_or(Value::Null);
        call(tool, &args)
    });
    out(&r.unwrap_or_else(|_| json!({"handled": true, "ok": false, "error": "the hot-swapped tool panicked"})))
}

/// # Safety
/// As [`zornov_hot_call`]; null means the arguments are fine as they are.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn zornov_hot_normalise(tool: *const c_char, args: *const c_char) -> *mut c_char {
    let (tool, args) = unsafe { (arg(tool), arg(args)) };
    let r = std::panic::catch_unwind(|| {
        let (Some(tool), Some(args)) = (tool, args) else { return None };
        let mut v: Value = serde_json::from_str(args).ok()?;
        normalise(tool, &mut v).then_some(v)
    });
    match r {
        Ok(Some(v)) => out(&v),
        _ => std::ptr::null_mut(),
    }
}

/// # Safety
/// `p` came from this library (or is null), and is freed once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn zornov_hot_free(p: *mut c_char) {
    if !p.is_null() {
        drop(unsafe { CString::from_raw(p) });
    }
}

/// One tool call.
pub fn call(tool: &str, args: &Value) -> Value {
    match tool {
        "feedback" => feedback(args),
        _ => json!({"handled": false}),
    }
}

/// `feedback`: says it was noted. The app records the call (its text capped);
/// nothing else happens.
fn feedback(a: &Value) -> Value {
    let has = |k: &str| a.get(k).and_then(Value::as_str).is_some_and(|s| !s.trim().is_empty());
    if !has("expected") || !has("got") {
        return json!({"handled": true, "ok": false, "error": "feedback needs expected and got: what you expected, and what happened instead"});
    }
    json!({"handled": true, "ok": true, "text": json!({"ok": true, "noted": true, "message": "Thanks: noted for Zornov's developers. Carry on."}).to_string()})
}

/// Renames fields agents reach for to Zornov's own, where the op does not
/// already have them. True when something changed.
pub fn normalise(tool: &str, args: &mut Value) -> bool {
    if tool != "edit" {
        return false;
    }
    let mut changed = false;
    for op in args.get_mut("ops").and_then(Value::as_array_mut).into_iter().flatten() {
        let name = op.get("op").and_then(Value::as_str).unwrap_or("").to_owned();
        let Some(o) = op.as_object_mut() else { continue };
        for (which, sent, meant) in ALIASES {
            if (*which == "*" || *which == name) && o.contains_key(*sent) && !o.contains_key(*meant) {
                if let Some(v) = o.remove(*sent) {
                    o.insert((*meant).to_owned(), v);
                    changed = true;
                }
            }
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_alias_becomes_the_field_zornov_reads() {
        let mut a = json!({"ops": [{"op": "text", "txt": "Hi", "color": "#fff"}, {"op": "card", "colour": "#000", "color": "#111"}]});
        assert!(normalise("edit", &mut a));
        assert_eq!(a["ops"][0]["text"], "Hi");
        assert_eq!(a["ops"][0]["colour"], "#fff");
        assert_eq!(a["ops"][1]["colour"], "#000", "a field it has wins");
        assert!(!normalise("summary", &mut json!({"color": 1})));
    }

    #[test]
    fn the_abi_round_trips() {
        let tool = CString::new("feedback").unwrap();
        let args = CString::new(r#"{"expected":"a","got":"b"}"#).unwrap();
        let p = unsafe { zornov_hot_call(tool.as_ptr(), args.as_ptr()) };
        let v: Value = serde_json::from_str(unsafe { CStr::from_ptr(p) }.to_str().unwrap()).unwrap();
        unsafe { zornov_hot_free(p) };
        assert_eq!(v["ok"], true);
        assert_eq!(zornov_hot_abi(), ABI);
    }
}
