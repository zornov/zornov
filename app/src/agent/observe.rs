// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The live tap: `zornov mcp --observe [SOCKET]` follows the tool calls of
//! an open window as they happen, one trace line each (see [`super::trace`]),
//! tagged with the Rust module that answered.
//!
//! It goes through the window's agent socket (only this user can connect;
//! on Windows the token file) and only listens: the first line it sends is
//! `{"observe": true}`, and from then on the window writes and never reads
//! that connection, so an observer cannot run a tool. A coding agent in the
//! repository can watch what an agent inside the app is doing (Claude
//! Code's Monitor tool on the command, or `| jq`).
//!
//! Lines are pushed the moment the recorder's writer builds them (no
//! polling): a call is on the observer's screen well under 100 ms after it
//! ends, usually under one. `--text` shows that delay per line.
//!
//! Only in `agent-dev` builds; release builds have no tap at all.

use serde_json::{Value, json};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The window's side: every call from now on, until the observer leaves.
pub fn stream(mut out: Box<dyn Write + Send>) {
    let rx = super::trace::tap();
    let hello = json!({"observing": true, "pid": std::process::id(), "session": super::trace::session_id(), "recording": super::trace::enabled()});
    if writeln!(out, "{hello}").and_then(|_| out.flush()).is_err() {
        return;
    }
    loop {
        let line: std::sync::Arc<str> = match rx.recv_timeout(Duration::from_secs(15)) {
            Ok(l) => l,
            // A blank line now and then finds out whether the observer left.
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => std::sync::Arc::from(""),
            Err(_) => return,
        };
        if writeln!(out, "{line}").and_then(|_| out.flush()).is_err() {
            return;
        }
    }
}

/// One line for a person: time, source, tool, outcome, cost, module.
pub fn text_line(v: &Value) -> String {
    let at = v["at"].as_u64().unwrap_or(0) / 1000;
    let (h, m, s) = ((at / 3600) % 24, (at / 60) % 60, at % 60);
    let mut l = format!(
        "{h:02}:{m:02}:{s:02} UTC {:<6} {:<14} {} {:>6} ms {:>6} tok  {}",
        v["source"].as_str().unwrap_or("?"),
        v["tool"].as_str().unwrap_or("?"),
        if v["ok"].as_bool().unwrap_or(false) { "ok " } else { "ERR" },
        v["ms"].as_u64().unwrap_or(0),
        v["reply_tokens"].as_u64().unwrap_or(0),
        v["module"].as_str().unwrap_or(""),
    );
    // From the call's end to this line on screen.
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
    if let Some(end) = v["at"].as_u64() {
        l.push_str(&format!("  +{} ms", now.saturating_sub(end)));
    }
    let (b, a) = (v["rev_before"].as_u64(), v["rev_after"].as_u64());
    if b != a {
        l.push_str(&format!("  rev {}→{}", b.unwrap_or(0), a.unwrap_or(0)));
    }
    if let Some(e) = v["error"].as_str() {
        l.push_str(&format!("\n    error: {}", e.lines().next().unwrap_or("")));
    }
    if v["tool"] == super::trace::FEEDBACK {
        l.push_str(&format!("\n    expected: {}\n    got: {}", v["args"]["expected"].as_str().unwrap_or(""), v["args"]["got"].as_str().unwrap_or("")));
    }
    l
}

/// `zornov mcp --observe [SOCKET] [--project DIR] [--text]`.
pub fn main(args: &[String]) -> i32 {
    let text = args.iter().any(|a| a == "--text");
    let project = args.iter().position(|a| a == "--project" || a == "-p").and_then(|i| args.get(i + 1)).map(PathBuf::from);
    let explicit = args.iter().enumerate().find(|(i, a)| !a.starts_with('-') && (*i == 0 || !matches!(args[i - 1].as_str(), "--project" | "-p"))).map(|(_, a)| PathBuf::from(a));
    let Some(socket) = explicit.or_else(|| super::attach::find_window(project.as_deref())) else {
        eprintln!("zornov mcp --observe: no open Zornov window (open one, or give its socket)");
        return 1;
    };
    match follow(&socket, &mut |v| {
        let mut out = std::io::stdout().lock();
        let ok = if text { writeln!(out, "{}", text_line(v)) } else { writeln!(out, "{v}") };
        ok.and_then(|_| out.flush()).is_ok()
    }) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("zornov mcp --observe: {e}");
            1
        }
    }
}

/// Follows the window at `socket`, each call to `each` (false: stop).
pub fn follow(socket: &Path, each: &mut dyn FnMut(&Value) -> bool) -> Result<(), String> {
    let (mut r, mut w) = super::attach::connect(socket).map_err(|e| format!("cannot reach the Zornov window at {}: {e}", socket.display()))?;
    writeln!(w, "{}", json!({"observe": true})).and_then(|_| w.flush()).map_err(|e| e.to_string())?;
    let mut line = String::new();
    loop {
        line.clear();
        if r.read_line(&mut line).map_err(|e| e.to_string())? == 0 {
            return Ok(());
        }
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(t) else { continue };
        if v.get("event").is_some() {
            continue;
        }
        if v.get("observing").is_some() {
            eprintln!("observing Zornov (pid {}){}", v["pid"], if v["recording"] == json!(false) { "; the recorder is off, so nothing is written to disk" } else { "" });
            continue;
        }
        if !each(&v) {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Session;
    use crate::agent::brain::Gate;
    use crate::agent::live::ToolCall;
    use std::sync::{Arc, mpsc};

    #[test]
    fn an_observer_sees_calls_and_cannot_make_them() {
        let (calls_tx, calls_rx) = mpsc::channel::<ToolCall>();
        let (events_tx, _events_rx) = mpsc::channel();
        let gate = Arc::new(Gate::new(calls_tx, events_tx, || {}));
        let path = crate::agent::attach::serve(gate).unwrap();
        std::thread::spawn(move || {
            let mut s = Session::default();
            for c in calls_rx {
                let _ = c.reply.send(s.run(&c.tool, &c.args));
            }
        });
        let (seen_tx, seen_rx) = mpsc::channel::<Value>();
        let p = path.clone();
        std::thread::spawn(move || {
            let _ = follow(&p, &mut |v| seen_tx.send(v.clone()).is_ok());
        });
        // An outside client makes a call; the observer hears it, tagged.
        std::thread::sleep(Duration::from_millis(200));
        crate::agent::attach::call_window(&path, "guide", &json!({"topic": "observe-test"})).ok();
        let v = (0..50).find_map(|_| seen_rx.recv_timeout(Duration::from_millis(200)).ok().filter(|v| v["args"]["topic"] == "observe-test")).expect("the observer heard the call");
        assert_eq!(v["source"], "attach");
        assert_eq!(v["module"], "agent::guides");
        // Anything an observer writes after it asked to observe is never read.
        let (mut r, mut w) = crate::agent::attach::connect(&path).unwrap();
        writeln!(w, "{}", json!({"observe": true})).unwrap();
        writeln!(w, "{}", json!({"name": "edit", "arguments": {"ops": [{"op": "card", "colour": "#ff0000", "at": 0, "duration": 1}]}})).unwrap();
        let mut hello = String::new();
        r.read_line(&mut hello).unwrap();
        assert!(hello.contains("observing"));
    }

    #[test]
    fn a_line_reads_for_a_person() {
        let v = json!({"at": 3_600_000, "source": "panel", "tool": "edit", "ok": false, "ms": 12, "reply_tokens": 40, "module": "agent::mod (edit: set)", "rev_before": 2, "rev_after": 2, "error": "no clip 9"});
        let l = text_line(&v);
        assert!(l.starts_with("01:00:00 UTC panel"), "{l}");
        assert!(l.contains("ERR") && l.contains("no clip 9"));
    }
}
