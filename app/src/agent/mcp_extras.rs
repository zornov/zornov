// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What `zornov mcp` (headless, on stdin / stdout) adds to the plain
//! JSON-RPC of [`super::mcp`], so an agent waits and re-reads less:
//!
//! * **Progress for exports.** A `tools/call` of `export` that carries
//!   `_meta.progressToken` starts the render in the background and sends
//!   `notifications/progress` (0..100, with the time left) until it is done,
//!   then answers as usual. Without a token nothing changes.
//! * **Project state as resources.** `studio://project/summary` and
//!   `studio://project/media` can be read and subscribed to; after any call
//!   that moves the project's revision, each subscribed one gets
//!   `notifications/resources/updated`, so an agent re-reads only when the
//!   project really changed instead of calling `summary` to find out.
//! * **Pictures as files.** A local client that asks for it (`initialize`
//!   with `capabilities.experimental.zornov.frame_paths: true`, or
//!   `ZORNOV_FRAMES=paths`) gets each picture as the path of a JPEG file
//!   instead of base64 in the reply.
//!
//! Through a window (`zornov mcp --attach`) the calls run in the window's
//! queue, which has jobs of its own; these are not offered there yet.

use super::mcp::{handle, result_of, unbase64};
use super::Session;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::time::Duration;

/// The project resources.
const SUMMARY: &str = "studio://project/summary";
const MEDIA: &str = "studio://project/media";

/// Tools that can take minutes in the foreground: with a progress token
/// their stages are sent as progress while they run.
const LONG: &[&str] = &["check", "review", "qc", "see", "objects", "listen", "analyse", "deliver", "bundle", "cutdown", "enhance", "download", "edit"];

/// The longest an export with progress holds the server before it answers
/// with the export to poll.
const PROGRESS_HOLD: Duration = Duration::from_secs(60);

#[derive(Default)]
pub struct Link {
    subscribed: BTreeSet<String>,
    frame_paths: bool,
    revision: Option<u64>,
    frames_written: u32,
}

/// Serves MCP on stdin / stdout until stdin closes.
pub fn serve_stdio(mut session: Session) {
    let mut link = Link { frame_paths: std::env::var("ZORNOV_FRAMES").is_ok_and(|v| v.trim() == "paths"), ..Default::default() };
    // agent-dev builds: a listen-only socket for `zornov mcp --observe`.
    #[cfg(feature = "agent-dev")]
    match super::attach::serve_tap() {
        Ok(p) => eprintln!("zornov mcp: live tap at {} (zornov mcp --observe {} --text)", p.display(), p.display()),
        Err(e) => eprintln!("zornov mcp: no live tap: {e}"),
    }
    let stdin = std::io::stdin();
    // Locked per line, not for the whole session: progress for a long call
    // is written from its own thread while the call runs.
    let mut send = |v: &Value| write_line(v);
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Value>(&line) {
            Ok(Value::Array(batch)) => {
                let rs: Vec<Value> = batch.iter().filter_map(|m| answer(&mut session, &mut link, m, &mut send)).collect();
                if !rs.is_empty() {
                    send(&Value::Array(rs));
                }
            }
            Ok(msg) => {
                if let Some(r) = answer(&mut session, &mut link, &msg, &mut send) {
                    send(&r);
                }
            }
            Err(e) => send(&json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": format!("parse error: {e}")}})),
        }
    }
}

/// One JSON-RPC line on stdout, whole.
fn write_line(v: &Value) {
    let mut o = std::io::stdout().lock();
    let _ = writeln!(o, "{v}");
    let _ = o.flush();
}

/// Runs `call` with a progress the tools report into (`jobs::report`)
/// and, while it runs, sends `notifications/progress` for `token` when the
/// stage moves on, or every ten seconds with the time so far, so a client
/// sees that a minutes-long `check` or `review` is alive and where it is.
fn with_progress<T>(token: &Value, send: impl Fn(&Value) + Send + 'static, call: impl FnOnce() -> T) -> T {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    let progress = Arc::new(super::jobs::Progress::default());
    super::jobs::install(progress.clone());
    let done = Arc::new(AtomicBool::new(false));
    let watcher = {
        let (progress, done, token) = (progress.clone(), done.clone(), token.clone());
        std::thread::spawn(move || {
            let begun = std::time::Instant::now();
            let (mut last_msg, mut last_at, mut sent) = (String::new(), begun, 0.0_f64);
            while !done.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(250));
                let (fraction, msg) = progress.get();
                if msg == last_msg && last_at.elapsed() < Duration::from_secs(10) {
                    continue;
                }
                // MCP wants progress to grow: the fraction when known, else a step.
                sent = ((fraction as f64 * 100.0).round()).max(sent + 0.1);
                let message = format!("{}{:.0} s", if msg.is_empty() { String::new() } else { format!("{msg}, ") }, begun.elapsed().as_secs_f64());
                send(&json!({"jsonrpc": "2.0", "method": "notifications/progress", "params": {"progressToken": token, "progress": sent, "total": 100, "message": message}}));
                (last_msg, last_at) = (msg, std::time::Instant::now());
            }
        })
    };
    let r = call();
    done.store(true, Ordering::SeqCst);
    let _ = watcher.join();
    super::jobs::install(Arc::new(super::jobs::Progress::default()));
    r
}

fn reply(id: &Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

/// One message; notifications for the client go through `notify`.
pub fn answer(session: &mut Session, link: &mut Link, msg: &Value, notify: &mut dyn FnMut(&Value)) -> Option<Value> {
    let id = msg.get("id").cloned().unwrap_or(Value::Null);
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    match msg.get("method").and_then(Value::as_str).unwrap_or("") {
        "initialize" => {
            if params["capabilities"]["experimental"]["zornov"]["frame_paths"] == json!(true) {
                link.frame_paths = true;
            }
            let mut r = handle(session, msg)?;
            r["result"]["capabilities"]["resources"] = json!({"subscribe": true, "listChanged": false});
            r["result"]["capabilities"]["experimental"] = json!({"zornov": {"frame_paths": link.frame_paths, "export_progress": true}});
            link.revision = Some(session.doc.history.revision());
            Some(r)
        }
        "resources/list" => {
            let mut r = handle(session, msg)?;
            if let Some(list) = r["result"]["resources"].as_array_mut() {
                list.push(json!({"uri": SUMMARY, "name": "The project: canvas, lanes, clips, markers (as summary)", "mimeType": "application/json"}));
                list.push(json!({"uri": MEDIA, "name": "The media bin (as media)", "mimeType": "application/json"}));
            }
            Some(r)
        }
        "resources/read" => {
            let uri = params["uri"].as_str().unwrap_or("");
            let tool = match uri {
                SUMMARY => "summary",
                MEDIA => "media",
                _ => return handle(session, msg),
            };
            let text = match session.call(tool, &json!({})) {
                Ok(parts) => parts.into_iter().filter_map(|p| if let super::Content::Text(t) = p { Some(t) } else { None }).collect::<Vec<_>>().join("\n"),
                Err(e) => return Some(json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32603, "message": e}})),
            };
            Some(reply(&id, json!({"contents": [{"uri": uri, "mimeType": "application/json", "text": text}]})))
        }
        "resources/subscribe" | "resources/unsubscribe" => {
            let uri = params["uri"].as_str().unwrap_or("").to_owned();
            if msg["method"] == "resources/subscribe" {
                link.subscribed.insert(uri);
            } else {
                link.subscribed.remove(&uri);
            }
            Some(reply(&id, json!({})))
        }
        "tools/call" => {
            let name = params["name"].as_str().unwrap_or("").to_owned();
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            let token = params["_meta"]["progressToken"].clone();
            let r = session.run_headless(&name, &args, |session| {
                if name == "export" && !token.is_null() && args.get("wait").and_then(Value::as_bool) != Some(false) {
                    super::guard::run(&name, || export_with_progress(session, &args, &token, notify))
                } else if !token.is_null() && LONG.contains(&name.as_str()) {
                    with_progress(&token, write_line, || super::guard::run(&name, || session.run(&name, &args)))
                } else {
                    super::guard::run(&name, || session.run(&name, &args))
                }
            });
            let mut result = result_of(r);
            if link.frame_paths {
                frames_to_files(&mut result, link);
            }
            changed(session, link, notify);
            Some(reply(&id, result))
        }
        _ => handle(session, msg),
    }
}

/// Tells subscribers when the project's revision moved.
fn changed(session: &Session, link: &mut Link, notify: &mut dyn FnMut(&Value)) {
    let now = session.doc.history.revision();
    if link.revision == Some(now) {
        return;
    }
    link.revision = Some(now);
    for uri in link.subscribed.iter().filter(|u| *u == SUMMARY || *u == MEDIA) {
        notify(&json!({"jsonrpc": "2.0", "method": "notifications/resources/updated", "params": {"uri": uri}}));
    }
}

/// `export` in the background, with progress notifications until it ends.
fn export_with_progress(session: &mut Session, args: &Value, token: &Value, notify: &mut dyn FnMut(&Value)) -> Result<Vec<super::Content>, String> {
    let mut a = args.clone();
    a["wait"] = json!(false);
    let started = session.run("export", &a)?;
    let first: Value = started.iter().find_map(|p| if let super::Content::Text(t) = p { serde_json::from_str(t).ok() } else { None }).unwrap_or_default();
    // A Lottie export, or anything else that answered at once.
    let Some(id) = first["export"].as_u64() else { return Ok(started) };
    let begun = std::time::Instant::now();
    // Sent when the export moves on, or every ten seconds so the client
    // knows it is alive; never backwards (MCP wants progress to grow).
    let (mut sent, mut sent_at) = (-1.0_f64, begun);
    loop {
        std::thread::sleep(Duration::from_millis(750));
        // Straight to the tool: the polls are not calls the agent made.
        let parts = session.call("export_status", &json!({"export": id}))?;
        let st: Value = parts.iter().find_map(|p| if let super::Content::Text(t) = p { serde_json::from_str(t).ok() } else { None }).unwrap_or_default();
        if st["done"] == json!(true) {
            let took = (begun.elapsed().as_secs_f64() * 10.0).round() / 10.0;
            return Ok(vec![super::Content::Text(json!({"ok": true, "path": st["path"], "took_seconds": took, "next": "qc {\"path\": …} checks the file (levels, flashes, black, frozen frames, sound)"}).to_string())]);
        }
        // This server answers one call at a time: past a minute the agent
        // gets the export back to poll, so it can keep working meanwhile.
        if begun.elapsed() > PROGRESS_HOLD {
            return Ok(vec![super::Content::Text(json!({"export": id, "still_rendering": true, "progress": st["progress"], "eta_secs": st["eta_secs"], "next": format!("the render goes on in the background; export_status {{\"export\": {id}}} says when it is done (export {{\"wait\": false}} answers at once)")}).to_string())]);
        }
        if let Some(why) = st["failed"].as_str() {
            return Err(format!("export failed: {why}"));
        }
        if st["cancelled"] == json!(true) {
            return Err("export was cancelled".into());
        }
        let p = ((st["progress"].as_f64().unwrap_or(0.0) * 100.0).clamp(0.0, 100.0) * 10.0).round() / 10.0;
        if p < sent + 0.5 && sent_at.elapsed() < Duration::from_secs(10) {
            continue;
        }
        let p = p.max(sent);
        (sent, sent_at) = (p, std::time::Instant::now());
        let message = match st["eta_secs"].as_f64() {
            Some(eta) if eta.is_finite() && eta > 0.0 => format!("exporting, about {:.0} s left", eta),
            _ => "exporting".to_owned(),
        };
        notify(&json!({"jsonrpc": "2.0", "method": "notifications/progress", "params": {"progressToken": token, "progress": p, "total": 100, "message": message}}));
    }
}

/// Pictures in a result become JPEG files, named in a text block.
fn frames_to_files(result: &mut Value, link: &mut Link) {
    let Some(content) = result["content"].as_array_mut() else { return };
    let dir = std::env::temp_dir().join("zornov-frames");
    for c in content.iter_mut() {
        if c["type"] != "image" {
            continue;
        }
        let Some(bytes) = c["data"].as_str().and_then(unbase64) else { continue };
        link.frames_written += 1;
        let path: PathBuf = dir.join(format!("frame-{}-{}.jpg", std::process::id(), link.frames_written));
        if std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&path, &bytes)).is_err() {
            continue;
        }
        *c = json!({"type": "text", "text": json!({"picture_file": path.display().to_string(), "bytes": bytes.len(), "note": "the picture is this JPEG file; open it to look"}).to_string()});
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::mcp::base64;

    fn ask(s: &mut Session, link: &mut Link, msg: Value) -> (Option<Value>, Vec<Value>) {
        let mut sent = Vec::new();
        let r = answer(s, link, &msg, &mut |v| sent.push(v.clone()));
        (r, sent)
    }

    #[test]
    fn a_subscriber_hears_when_the_project_changes() {
        let (mut s, mut link) = (Session::default(), Link::default());
        let (init, _) = ask(&mut s, &mut link, json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}));
        assert_eq!(init.unwrap()["result"]["capabilities"]["resources"]["subscribe"], true);
        ask(&mut s, &mut link, json!({"jsonrpc": "2.0", "id": 2, "method": "resources/subscribe", "params": {"uri": SUMMARY}}));
        let (_, quiet) = ask(&mut s, &mut link, json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "summary", "arguments": {}}}));
        assert!(quiet.is_empty(), "a read changes nothing: {quiet:?}");
        let (_, sent) = ask(&mut s, &mut link, json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {"name": "edit", "arguments": {"ops": [{"op": "card", "colour": "#ff0000", "at": 0, "duration": 1}]}}}));
        assert!(sent.iter().any(|n| n["method"] == "notifications/resources/updated" && n["params"]["uri"] == SUMMARY), "{sent:?}");
        let (read, _) = ask(&mut s, &mut link, json!({"jsonrpc": "2.0", "id": 5, "method": "resources/read", "params": {"uri": SUMMARY}}));
        assert!(read.unwrap()["result"]["contents"][0]["text"].as_str().unwrap().contains("clips"));
    }

    #[test]
    fn project_resources_are_listed_with_the_guide() {
        let (mut s, mut link) = (Session::default(), Link::default());
        let (r, _) = ask(&mut s, &mut link, json!({"jsonrpc": "2.0", "id": 1, "method": "resources/list"}));
        let uris: Vec<String> = r.unwrap()["result"]["resources"].as_array().unwrap().iter().filter_map(|r| r["uri"].as_str().map(str::to_owned)).collect();
        assert!(uris.contains(&"studio://guide".to_owned()) && uris.contains(&SUMMARY.to_owned()), "{uris:?}");
    }

    #[test]
    fn a_picture_becomes_a_file_when_asked() {
        let mut link = Link { frame_paths: true, ..Default::default() };
        let mut r = json!({"content": [{"type": "image", "mimeType": "image/jpeg", "data": base64(&[0xFF, 0xD8, 0xFF, 0xD9])}]});
        frames_to_files(&mut r, &mut link);
        let t: Value = serde_json::from_str(r["content"][0]["text"].as_str().unwrap()).unwrap();
        let p = PathBuf::from(t["picture_file"].as_str().unwrap());
        assert_eq!(std::fs::read(&p).unwrap(), vec![0xFF, 0xD8, 0xFF, 0xD9]);
        let _ = std::fs::remove_file(p);
    }

    fn refused(r: Option<Value>) -> String {
        let r = r.unwrap()["result"].clone();
        assert_eq!(r["isError"], true, "a call that needs the person's OK must not run headless: {r}");
        r["content"][0]["text"].as_str().unwrap().to_owned()
    }

    // Run with HOME pointing at an empty folder: today this call really
    // switches OpenFX on in the person's settings.
    #[test]
    fn headless_does_not_run_what_the_window_would_ask_about() {
        let (mut s, mut link) = (Session::default(), Link::default());
        let call = |id: u32, name: &str, args: Value| json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": name, "arguments": args}});
        let (r, _) = ask(&mut s, &mut link, call(1, "plugins", json!({"action": "ofx", "on": true})));
        let t = refused(r);
        assert!(t.starts_with("NOT RUN") && t.contains("Load OpenFX"), "{t}");
        let (r, _) = ask(&mut s, &mut link, call(2, "account", json!({"action": "buy_credits", "pack": "small"})));
        let t = refused(r);
        assert!(t.starts_with("NOT RUN") && t.contains("Buy credits"), "{t}");
        // The agent cannot answer for the person.
        let (r, _) = ask(&mut s, &mut link, call(3, "plugins", json!({"action": "ofx", "on": true, "user_approved": true})));
        assert!(refused(r).starts_with("NOT RUN"));
    }

    #[test]
    fn after_outside_text_the_lasting_actions_are_refused_too() {
        let (mut s, mut link) = (Session::default(), Link::default());
        let call = |id: u32, name: &str, args: Value| json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": name, "arguments": args}});
        let (r, _) = ask(&mut s, &mut link, call(1, "notes", json!({"write": "first note"})));
        assert!(r.unwrap()["result"]["isError"].is_null(), "no outside text yet: the note is written");
        // `plugins list` carries the outside-content note, as in the window.
        let (r, _) = ask(&mut s, &mut link, call(2, "plugins", json!({"action": "list"})));
        assert!(r.unwrap()["result"]["isError"].is_null());
        let (r, _) = ask(&mut s, &mut link, call(3, "notes", json!({"write": "second note"})));
        let t = refused(r);
        assert!(t.starts_with("NOT RUN") && t.contains("read outside content"), "{t}");
        // Another session starts clean.
        let (mut s2, mut link2) = (Session::default(), Link::default());
        let (r, _) = ask(&mut s2, &mut link2, call(4, "notes", json!({"write": "fresh"})));
        assert!(r.unwrap()["result"]["isError"].is_null());
    }

    #[test]
    fn the_refusal_is_the_windows_own_question() {
        use crate::agent::brain::needs_ok;
        for (tool, args) in [
            ("plugins", json!({"action": "install", "id": "x"})),
            ("account", json!({"action": "sign_in"})),
            ("project_care", json!({"action": "clean", "apply": true})),
            ("save", json!({"path": "/tmp/elsewhere"})),
        ] {
            let why = needs_ok(tool, &args, false).unwrap_or_else(|| panic!("{tool} {args} should ask"));
            let (mut s, mut link) = (Session::default(), Link::default());
            let (r, _) = ask(&mut s, &mut link, json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": tool, "arguments": args}}));
            assert!(refused(r).contains(&why));
        }
        // The client's own request, as in the window: not asked about.
        assert!(needs_ok("export", &json!({}), false).is_none() && needs_ok("open", &json!({"path": "p"}), false).is_none());
        assert!(needs_ok("plugins", &json!({"action": "ofx", "on": false}), false).is_none());
    }
}
