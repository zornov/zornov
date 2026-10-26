// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Claude Code reaches the window through a local socket.
//!
//! The window listens on `$TMPDIR/studio-agent-PID.sock` (only this user
//! can connect). On Windows, which has no Unix sockets here, that file holds
//! a port on 127.0.0.1 and a random token instead: only someone who can read
//! the file (this user) can connect. `studio mcp --attach SOCKET` is the MCP server Claude Code
//! starts, and it forwards each tool call there as one line of JSON
//! (`{"name", "arguments"}`), getting back one line: the MCP result.
//!
//! Every open window also leaves a note in `windows/` under Studio's cache
//! (its socket and the project folder it has open), so `studio mcp` and
//! `studio agent` from any terminal find it and work on the project in the
//! window rather than a copy on disk: the person and the agent edit the
//! same project, and each sees the other's changes.

use super::brain::Gate;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// One connection to a window: what it says, and where to write.
type Link = (BufReader<Box<dyn Read + Send>>, Box<dyn Write + Send>);

/// Listens for proxies; each call goes through `gate`.
pub fn serve(gate: Arc<Gate>) -> std::io::Result<PathBuf> {
    // One per listener (a window may open another, and tests run side by side).
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let path = std::env::temp_dir().join(format!("studio-agent-{}{}.sock", std::process::id(), if n == 0 { String::new() } else { format!("-{n}") }));
    let _ = std::fs::remove_file(&path);
    listen(&path, gate)?;
    Ok(path)
}

#[cfg(unix)]
fn listen(path: &Path, gate: Arc<Gate>) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::UnixListener;
    let listener = UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    std::thread::Builder::new().name("studio-agent-socket".into()).spawn(move || {
        for stream in listener.incoming().map_while(Result::ok) {
            let gate = gate.clone();
            std::thread::spawn(move || {
                if let Ok(out) = stream.try_clone() {
                    connection(BufReader::new(Box::new(stream)), Box::new(out), &gate);
                }
            });
        }
    })?;
    Ok(())
}

/// Windows: 127.0.0.1 on a free port; the first line of each connection
/// must be the token in the file at `path` (in this user's temp folder).
#[cfg(windows)]
fn listen(path: &Path, gate: Arc<Gate>) -> std::io::Result<()> {
    use std::net::TcpListener;
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let port = listener.local_addr()?.port();
    let token = token()?;
    std::fs::OpenOptions::new().write(true).create_new(true).open(path)?.write_all(format!("{port} {token}\n").as_bytes())?;
    std::thread::Builder::new().name("studio-agent-socket".into()).spawn(move || {
        for stream in listener.incoming().map_while(Result::ok) {
            let (gate, token) = (gate.clone(), token.clone());
            std::thread::spawn(move || {
                // The token must come first, and soon.
                let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));
                let Ok(out) = stream.try_clone() else { return };
                let mut r: BufReader<Box<dyn Read + Send>> = BufReader::new(Box::new(stream));
                let mut first = String::new();
                if r.read_line(&mut first).is_err() || !same(first.trim_end().as_bytes(), token.as_bytes()) {
                    return;
                }
                let _ = out.set_read_timeout(None);
                connection(r, Box::new(out), &gate);
            });
        }
    })?;
    Ok(())
}

/// 32 random bytes as hex, from the system's generator.
#[cfg(windows)]
fn token() -> std::io::Result<String> {
    use windows_sys::Win32::Security::Cryptography::{BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom};
    let mut b = [0u8; 32];
    let status = unsafe { BCryptGenRandom(std::ptr::null_mut(), b.as_mut_ptr(), b.len() as u32, BCRYPT_USE_SYSTEM_PREFERRED_RNG) };
    if status != 0 {
        return Err(std::io::Error::other(format!("no random token (status {status:#x})")));
    }
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
}

/// Compares in constant time, so the token cannot be guessed by timing.
#[cfg(windows)]
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Connects to the window listening at `socket`.
#[cfg(unix)]
fn connect(socket: &Path) -> std::io::Result<Link> {
    let stream = std::os::unix::net::UnixStream::connect(socket)?;
    let out = stream.try_clone()?;
    Ok((BufReader::new(Box::new(stream)), Box::new(out)))
}

#[cfg(windows)]
fn connect(socket: &Path) -> std::io::Result<Link> {
    let text = std::fs::read_to_string(socket)?;
    let mut it = text.split_whitespace();
    let port: u16 = it.next().and_then(|p| p.parse().ok()).ok_or_else(|| std::io::Error::other("no port in the window's file"))?;
    let token = it.next().ok_or_else(|| std::io::Error::other("no token in the window's file"))?;
    let stream = std::net::TcpStream::connect(("127.0.0.1", port))?;
    let mut out = stream.try_clone()?;
    writeln!(out, "{token}")?;
    Ok((BufReader::new(Box::new(stream)), Box::new(out)))
}

fn connection(reader: BufReader<Box<dyn Read + Send>>, mut out: Box<dyn Write + Send>, gate: &Gate) {
    let mut first = true;
    for line in reader.lines().map_while(Result::ok) {
        if std::mem::take(&mut first) && !gate.in_turn() {
            announce(gate);
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            // The helper asks which tools this Studio has: its own list is
            // the truth, not the helper's.
            Ok(v) if v.get("list_tools").is_some() => json!({"tools": super::mcp::tools()}),
            Ok(v) => {
                let name = v.get("name").and_then(Value::as_str).unwrap_or("");
                let args = v.get("arguments").cloned().unwrap_or(json!({}));
                let role = v.get("role").and_then(Value::as_str).and_then(super::crew::Role::parse);
                super::mcp::result_of(gate.call_as(role, name, &args))
            }
            Err(e) => json!({"content": [{"type": "text", "text": format!("bad request: {e}")}], "isError": true}),
        };
        if writeln!(out, "{reply}").and_then(|_| out.flush()).is_err() {
            break;
        }
    }
}

/// Says once in a while that an agent outside the app is at work (each
/// `studio agent` call is a connection of its own).
fn announce(gate: &Gate) {
    static LAST: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    if last.is_some_and(|t| t.elapsed() < std::time::Duration::from_secs(60)) {
        return;
    }
    *last = Some(std::time::Instant::now());
    gate.emit(super::brain::AgentEvent::Note("An agent outside the app is working on this project (Claude Code, studio agent or another MCP client). Its changes appear here and on the timeline.".into()));
}

/// `studio mcp --attach SOCKET [--role ROLE]`: MCP on stdin / stdout,
/// tools run in the window (as that crew member).
pub fn proxy(socket: &Path, role: Option<&str>) -> i32 {
    let (from_window, to_window) = match connect(socket) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("studio mcp: cannot reach the Studio window at {}: {e}", socket.display());
            return 1;
        }
    };
    let link = std::cell::RefCell::new((to_window, from_window));
    let ask = |req: &Value| -> Option<Value> {
        let mut l = link.borrow_mut();
        let mut line = String::new();
        if writeln!(l.0, "{req}").and_then(|_| l.0.flush()).is_err() || l.1.read_line(&mut line).unwrap_or(0) == 0 {
            return None;
        }
        Some(serde_json::from_str(&line).unwrap_or_else(|e| json!({"content": [{"type": "text", "text": format!("bad reply from the window: {e}")}], "isError": true})))
    };
    // The tools of the Studio that is running (this helper may be newer
    // or older than it); the helper's own list when it does not answer.
    let app_tools = || -> Option<Vec<Value>> { ask(&json!({"list_tools": true}))?.get("tools")?.as_array().cloned() };
    let known: Option<Vec<String>> = app_tools().map(|t| t.iter().filter_map(|t| t["name"].as_str().map(str::to_owned)).collect());
    let mut call = |name: &str, args: &Value| -> Value {
        if let Some(why) = missing_in_app(known.as_deref(), name) {
            return json!({"content": [{"type": "text", "text": why}], "isError": true});
        }
        let mut req = json!({"name": name, "arguments": args});
        if let Some(r) = role {
            req["role"] = json!(r);
        }
        ask(&req).unwrap_or_else(|| json!({"content": [{"type": "text", "text": "the Studio window closed"}], "isError": true}))
    };
    let mut list = || app_tools().unwrap_or_else(super::mcp::tools);
    super::mcp::serve_lines_listing(&mut call, &mut list);
    0
}

/// Why a tool cannot run in the Studio that is open, when this helper
/// knows it but that Studio does not (it is an older build).
fn missing_in_app(app: Option<&[String]>, name: &str) -> Option<String> {
    let app = app?;
    if app.iter().any(|t| t == name) || !super::mcp::tools().iter().any(|t| t["name"] == name) {
        return None;
    }
    Some(format!("The open Studio is an older version than this helper and does not have the \"{name}\" tool. Restart Studio to update it, then try again; use only the tools the open Studio lists meanwhile."))
}

fn windows_dir() -> PathBuf {
    studio_paths::cache().join("windows")
}

fn note_path() -> PathBuf {
    windows_dir().join(format!("{}.json", std::process::id()))
}

/// Says that this window is open, with its socket and project folder.
pub fn register(socket: &Path, project: Option<&Path>) {
    let _ = std::fs::create_dir_all(windows_dir());
    let project = project.map(|p| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf()));
    let v = json!({"pid": std::process::id(), "socket": socket, "project": project});
    let _ = std::fs::write(note_path(), v.to_string());
}

/// The window is closing.
pub fn unregister(socket: &Path) {
    let _ = std::fs::remove_file(note_path());
    let _ = std::fs::remove_file(socket);
}

/// The socket of an open window: the one with `project` open, or with no
/// project asked for, the window opened last. Notes of windows that are
/// gone are cleared on the way.
pub fn find_window(project: Option<&Path>) -> Option<PathBuf> {
    let want = project.map(|p| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf()));
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    for e in std::fs::read_dir(windows_dir()).ok()?.flatten() {
        let Ok(text) = std::fs::read_to_string(e.path()) else { continue };
        let Ok(v) = serde_json::from_str::<Value>(&text) else { continue };
        let Some(socket) = v["socket"].as_str().map(PathBuf::from) else { continue };
        if connect(&socket).is_err() {
            let _ = std::fs::remove_file(e.path());
            continue;
        }
        let open = v["project"].as_str().map(PathBuf::from);
        if want.is_some() && open != want {
            continue;
        }
        let when = e.metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH);
        if best.as_ref().is_none_or(|(t, _)| when > *t) {
            best = Some((when, socket));
        }
    }
    best.map(|(_, s)| s)
}

/// One tool call on an open window (for `studio agent`).
pub fn call_window(socket: &Path, name: &str, args: &Value) -> Result<Vec<super::Content>, String> {
    let (mut r, mut w) = connect(socket).map_err(|e| format!("cannot reach the Studio window: {e}"))?;
    writeln!(w, "{}", json!({"name": name, "arguments": args})).and_then(|_| w.flush()).map_err(|e| e.to_string())?;
    let mut line = String::new();
    r.read_line(&mut line).map_err(|e| e.to_string())?;
    let v: Value = serde_json::from_str(&line).map_err(|e| format!("bad reply from the window: {e}"))?;
    let parts: Vec<super::Content> = v["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| match c["type"].as_str()? {
            "text" => Some(super::Content::Text(c["text"].as_str()?.to_owned())),
            "image" => Some(super::Content::Jpeg(super::mcp::unbase64(c["data"].as_str()?)?)),
            _ => None,
        })
        .collect();
    if v["isError"].as_bool().unwrap_or(false) {
        let text: Vec<String> = parts.into_iter().filter_map(|p| if let super::Content::Text(t) = p { Some(t) } else { None }).collect();
        return Err(text.join("\n"));
    }
    Ok(parts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::live::ToolCall;
    use crate::agent::{Content, Session};
    use std::sync::mpsc;

    #[test]
    fn the_helper_lists_the_tools_of_the_running_studio() {
        let (calls_tx, _calls_rx) = mpsc::channel::<ToolCall>();
        let (events_tx, _events_rx) = mpsc::channel();
        let gate = Arc::new(Gate::new(calls_tx, events_tx, || {}));
        let path = serve(gate).unwrap();
        let (mut r, mut w) = connect(&path).unwrap();
        writeln!(w, "{}", json!({"list_tools": true})).unwrap();
        let mut line = String::new();
        r.read_line(&mut line).unwrap();
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["tools"].as_array().unwrap().len(), crate::agent::mcp::tools().len());
    }

    /// A newer helper must not offer or run a tool the open Studio lacks:
    /// it says to restart Studio.
    #[test]
    fn a_tool_the_open_studio_lacks_says_to_restart() {
        let old: Vec<String> = ["guide", "summary", "edit"].map(String::from).to_vec();
        let why = missing_in_app(Some(&old), "download").expect("refused");
        assert!(why.contains("Restart Studio"), "{why}");
        assert!(missing_in_app(Some(&old), "edit").is_none());
        assert!(missing_in_app(Some(&old), "no_such_tool").is_none(), "the app words its own unknown-tool error");
        assert!(missing_in_app(None, "download").is_none(), "no list from the window: let it answer");
    }

    #[test]
    fn a_proxy_call_runs_through_the_gate() {
        let (calls_tx, calls_rx) = mpsc::channel::<ToolCall>();
        let (events_tx, _events_rx) = mpsc::channel();
        let gate = Arc::new(Gate::new(calls_tx, events_tx, || {}));
        let path = serve(gate).unwrap();
        // A stand-in window: runs calls on a session.
        std::thread::spawn(move || {
            let mut s = Session::default();
            for c in calls_rx {
                let _ = c.reply.send(s.run(&c.tool, &c.args));
            }
        });
        let (mut r, mut w) = connect(&path).unwrap();
        writeln!(w, "{}", json!({"name": "guide", "arguments": {}})).unwrap();
        let mut line = String::new();
        r.read_line(&mut line).unwrap();
        let v: Value = serde_json::from_str(&line).unwrap();
        assert!(v["content"][0]["text"].as_str().unwrap().contains("Studio"));
        let _ = Content::Text(String::new());
    }
}
