// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Two-way contact between a coding agent in this repository and the
//! agent inside a running window (`agent-dev` builds only):
//!
//! ```text
//! zornov dev ask "TEXT"            tell the agent inside something (as a correction)
//! zornov dev retry N               run recent call N again, and say what differs
//! zornov dev swap [--retry N]      rebuild crates/dev/agent-hot, load it, retry N
//! zornov dev status                what is hot-swapped
//! ```
//!
//! Each goes through the window's agent socket (this user only) and then
//! through the same gate as every tool call: a retry is a tool call like
//! any other (paused, stopped and step-limited the same way, and marked
//! `dev` in the trace), and `ask` is the panel's own correction path.
//! Requests are rate-limited (five at once, then one a second), text is
//! capped, and a retry runs only a call this window made recently, from
//! memory, never one read from a file. The agent inside does not wait for
//! any of it: its calls keep their place in the queue, and a retry is one
//! more call in it.

#[cfg(not(debug_assertions))]
compile_error!("agent-dev is for development builds: leave the feature out of release builds");

use super::brain::Gate;
use super::replay::Outcome;
use serde_json::{Value, json};
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Instant;

/// Requests at once, and per second after that.
const BURST: f64 = 5.0;
const PER_SECOND: f64 = 1.0;
/// Most of an `ask` that reaches the agent.
const ASK_CAP: usize = 2_000;

fn allowed() -> bool {
    static BUCKET: Mutex<Option<(f64, Instant)>> = Mutex::new(None);
    let mut b = BUCKET.lock().unwrap_or_else(|e| e.into_inner());
    let now = Instant::now();
    let (tokens, at) = b.unwrap_or((BURST, now));
    let tokens = (tokens + now.duration_since(at).as_secs_f64() * PER_SECOND).min(BURST);
    if tokens < 1.0 {
        *b = Some((tokens, now));
        return false;
    }
    *b = Some((tokens - 1.0, now));
    true
}

/// The window's side: one request, one answer.
pub fn handle(gate: &Gate, req: &Value) -> Value {
    if !allowed() {
        return json!({"error": "too many dev requests: five at once, then one a second"});
    }
    match req["do"].as_str().unwrap_or("") {
        "ask" => {
            let Some(text) = req["text"].as_str().map(str::trim).filter(|t| !t.is_empty()) else { return json!({"error": "ask needs text"}) };
            if !gate.in_turn() {
                return json!({"error": "no request is running in the panel; there is no agent inside to ask"});
            }
            let text: String = text.chars().take(ASK_CAP).collect();
            gate.steer(&format!("From the developer working on Zornov's source, who is watching this session: {text}"));
            json!({"ok": true, "delivered": "as a correction, before the agent's next call"})
        }
        "retry" => match req["call"].as_u64() {
            Some(n) => retry(gate, n),
            None => json!({"error": "retry needs call: the seq of a recent call (zornov mcp --observe shows them)"}),
        },
        "swap" => {
            let t0 = Instant::now();
            let loaded = match super::hot::load(req["sha256"].as_str().unwrap_or("")) {
                Ok(v) => v,
                Err(e) => return json!({"error": e}),
            };
            let load_ms = t0.elapsed().as_secs_f64() * 1000.0;
            let retried = req["retry"].as_u64().map(|n| retry(gate, n));
            let retry_ms = retried.as_ref().and_then(|r| r["ms"].as_f64()).unwrap_or(0.0);
            let build_ms = req["build_ms"].as_f64().filter(|b| b.is_finite() && *b >= 0.0).unwrap_or(0.0).min(3.6e6);
            let fix = build_ms + load_ms + retry_ms;
            super::trace::event(gate.project().as_deref(), json!({"event": "swap", "at": now_ms(), "sha256": loaded["sha256"], "build_ms": round(build_ms), "load_ms": round(load_ms), "retry_ms": round(retry_ms), "fix_to_retry_ms": round(fix)}));
            json!({"loaded": loaded, "load_ms": round(load_ms), "retry": retried, "fix_to_retry_ms": round(fix)})
        }
        "status" => super::hot::status(),
        other => json!({"error": format!("unknown dev request \"{other}\"; ask, retry, swap or status")}),
    }
}

fn round(ms: f64) -> f64 {
    (ms * 10.0).round() / 10.0
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// Runs recent call `n` again through the gate and compares.
fn retry(gate: &Gate, n: u64) -> Value {
    let Some((tool, mut args, line)) = super::trace::recent(n) else {
        return json!({"error": format!("call {n} is not among this window's last 256 calls")});
    };
    if tool == super::trace::FEEDBACK {
        return json!({"error": "feedback has nothing to retry"});
    }
    super::trace::tag_dev_call(&mut args);
    let t0 = Instant::now();
    let r = gate.call_as(None, &tool, &args);
    let ms = t0.elapsed().as_secs_f64() * 1000.0;
    let then = Outcome::of_recorded(&line);
    let mut now = Outcome::of_run(&r, then.changed);
    // Whether the project moved is the window's to say, not this answer's.
    now.changed = then.changed;
    let diff = then.diff(&now);
    json!({"call": n, "tool": tool, "same": diff.is_empty(), "diff": diff, "ok": now.ok, "error": now.error, "ms": round(ms)})
}

// ------------------------------------------------------------ the source side

fn socket(args: &[String]) -> Option<PathBuf> {
    let get = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).map(PathBuf::from);
    get("--socket").or_else(|| super::attach::find_window(get("--project").as_deref()))
}

fn ask_window(sock: &std::path::Path, dev: Value) -> Result<Value, String> {
    let (mut r, mut w) = super::attach::connect(sock).map_err(|e| format!("cannot reach the Zornov window: {e}"))?;
    writeln!(w, "{}", json!({"dev": dev})).and_then(|_| w.flush()).map_err(|e| e.to_string())?;
    let mut line = String::new();
    r.read_line(&mut line).map_err(|e| e.to_string())?;
    serde_json::from_str(&line).map_err(|e| format!("bad answer from the window: {e}"))
}

/// `zornov dev …`
pub fn main(args: &[String]) -> i32 {
    let usage = || {
        eprintln!("usage: zornov dev ask TEXT | retry N | swap [--retry N] [--no-build] | status   [--socket S | --project DIR]");
        2
    };
    let Some(sock) = socket(args) else {
        eprintln!("zornov dev: no open Zornov window (an agent-dev build) to talk to");
        return 1;
    };
    let num = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).and_then(|v| v.parse::<u64>().ok());
    let req = match args.first().map(String::as_str) {
        Some("ask") => match args.get(1) {
            Some(t) => json!({"do": "ask", "text": t}),
            None => return usage(),
        },
        Some("retry") => match args.get(1).and_then(|v| v.parse::<u64>().ok()) {
            Some(n) => json!({"do": "retry", "call": n}),
            None => return usage(),
        },
        Some("status") => json!({"do": "status"}),
        Some("swap") => {
            let mut build_ms = 0.0;
            if !args.iter().any(|a| a == "--no-build") {
                let Some(root) = super::hot::repo_root() else {
                    eprintln!("zornov dev: this build does not know its checkout");
                    return 1;
                };
                let t0 = Instant::now();
                // The developer's own build of this checkout: a fixed command, nothing from a trace.
                let ok = std::process::Command::new("cargo").args(["build", "-p", "zornov-agent-hot"]).current_dir(&root).status().is_ok_and(|s| s.success());
                build_ms = t0.elapsed().as_secs_f64() * 1000.0;
                if !ok {
                    eprintln!("zornov dev: cargo build -p zornov-agent-hot failed");
                    return 1;
                }
            }
            let Some(lib) = super::hot::library_path() else { return 1 };
            let sha = match studio_update::feed::sha256_file(&lib) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("zornov dev: {}: {e}", lib.display());
                    return 1;
                }
            };
            let mut r = json!({"do": "swap", "sha256": sha, "build_ms": round(build_ms)});
            if let Some(n) = num("--retry") {
                r["retry"] = json!(n);
            }
            r
        }
        _ => return usage(),
    };
    match ask_window(&sock, req) {
        Ok(v) => {
            println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
            if v.get("error").is_some() { 1 } else { 0 }
        }
        Err(e) => {
            eprintln!("zornov dev: {e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_are_rate_limited() {
        let n = (0..20).filter(|_| allowed()).count();
        assert!(n <= BURST as usize + 1, "{n} let through at once");
    }
}
