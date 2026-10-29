// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The agent entry points on the command line.
//!
//! ```text
//! zornov mcp [--project DIR]                    MCP server on stdin / stdout
//! zornov mcp --observe [SOCKET] [--text]        follow an open window's calls
//! zornov agent [--project DIR] TOOL [JSON]      one tool call, result on stdout
//! zornov agent [--project DIR] -                calls from stdin, each
//!                                               {"tool": "...", "args": {...}}
//! ```
//!
//! With `--project` the folder is opened (or a new project is started in
//! it) and saved after every change, so separate calls build on each
//! other. Pictures are written as JPEG files and their paths printed.

use super::{Content, Session};
use serde_json::{Value, json};
use std::io::Write;
use std::path::PathBuf;
use studio_model as m;

fn usage() -> i32 {
    eprintln!("usage: zornov agent [--project DIR] [--out DIR] TOOL [JSON-ARGS]\n       zornov agent [--project DIR] -        (one {{\"tool\", \"args\"}} per stdin line)\n       zornov mcp [--project DIR]\ntools: guide, summary, inspect, media, catalogue, edit, check, undo, redo, new_project, open, save, import, download, ai, analyse, frame, contact_sheet, export, export_status, qc, deliver, queue, bundle, cutdown, edit_schema");
    2
}

struct Opts {
    project: Option<PathBuf>,
    out: Option<PathBuf>,
    rest: Vec<String>,
}

fn opts(args: &[String]) -> Result<Opts, String> {
    let mut o = Opts { project: None, out: None, rest: Vec::new() };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--project" | "-p" => o.project = Some(PathBuf::from(it.next().ok_or("--project needs a folder")?)),
            "--headless" => {}
            "--out" | "-o" => o.out = Some(PathBuf::from(it.next().ok_or("--out needs a folder")?)),
            _ => o.rest.push(a.clone()),
        }
    }
    Ok(o)
}

fn session(project: &Option<PathBuf>) -> Result<Session, String> {
    // The content library (music, sound effects, stickers, frames,
    // overlays…): the window loads it in the background; agents need it
    // before their first call.
    let _ = crate::content::get_blocking();
    match project {
        Some(dir) => Session::at(dir),
        None => Ok(Session::default()),
    }
}

/// The open Studio window to work in, unless `--headless` was given:
/// the one with `project` open, or with no project, the last one opened.
fn open_window(args: &[String], project: &Option<PathBuf>) -> Option<PathBuf> {
    if args.iter().any(|a| a == "--headless") {
        return None;
    }
    super::attach::find_window(project.as_deref())
}

/// `zornov mcp …`
pub fn mcp(args: &[String]) -> i32 {
    // A read-only tap on an open window's calls (observe.rs).
    if args.first().map(String::as_str) == Some("--observe") {
        #[cfg(feature = "agent-dev")]
        return super::observe::main(&args[1..]);
        #[cfg(not(feature = "agent-dev"))]
        {
            eprintln!("zornov mcp --observe: this build has no live tap (it is in development builds made with --features agent-dev)");
            return 2;
        }
    }
    super::trace::set_source(super::trace::Source::Stdio);
    // Claude Code driving the editor window: tools run there.
    if args.first().map(String::as_str) == Some("--attach") {
        return match args.get(1) {
            Some(sock) => super::attach::proxy(std::path::Path::new(sock), args.iter().position(|a| a == "--role").and_then(|i| args.get(i + 1)).map(String::as_str)),
            None => {
                eprintln!("zornov mcp: --attach needs the window's socket");
                2
            }
        };
    }
    let all = args;
    let args: Vec<String> = args.iter().filter(|a| *a != "--headless").cloned().collect();
    if let Ok(o) = opts(&args) {
        if let Some(sock) = open_window(all, &o.project) {
            // The person has this project open: work on it there.
            eprintln!("zornov mcp: working in the open Zornov window");
            return super::attach::proxy(&sock, None);
        }
    }
    match opts(&args).and_then(|o| session(&o.project)) {
        Ok(s) => {
            super::mcp::serve_stdio(s);
            0
        }
        Err(e) => {
            eprintln!("zornov mcp: {e}");
            1
        }
    }
}

/// `zornov agent …`
pub fn agent(args: &[String]) -> i32 {
    super::trace::set_source(super::trace::Source::Agent);
    let o = match opts(args) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("zornov agent: {e}");
            return usage();
        }
    };
    let Some(tool) = o.rest.first().cloned() else { return usage() };
    // The person has the project open: the calls run in that window.
    let window = open_window(args, &o.project);
    if window.is_some() {
        eprintln!("zornov agent: working in the open Zornov window");
    }
    let mut s = match session(&o.project) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("zornov agent: {e}");
            return 1;
        }
    };
    let out_dir = o.out.clone().map(|p| std::path::absolute(&p).unwrap_or(p)).or_else(|| o.project.as_ref().map(|p| m::file::ProjectFolder::new(std::path::absolute(p).unwrap_or_else(|_| p.clone())).agent_frames_dir())).unwrap_or_else(|| std::env::temp_dir().join("studio-agent"));
    let mut stdout = std::io::stdout().lock();
    let mut one = |s: &mut Session, tool: &str, args: &Value| -> bool {
        let r = match &window {
            Some(sock) => super::attach::call_window(sock, tool, args),
            None => super::guard::run(tool, || s.run(tool, args)),
        };
        let line = match r {
            Ok(parts) => {
                let mut texts = Vec::new();
                let mut images = Vec::new();
                for p in parts {
                    match p {
                        Content::Text(t) => texts.push(serde_json::from_str::<Value>(&t).unwrap_or(Value::String(t))),
                        Content::Jpeg(b) => {
                            match crate::files::write_once(&out_dir, tool, "jpg", &b) {
                                Ok(path) => images.push(path.display().to_string()),
                                Err(e) => texts.push(json!({"image_not_written": e.to_string()})),
                            }
                        }
                    }
                }
                let mut v = if texts.len() == 1 { texts.remove(0) } else { json!(texts) };
                if !images.is_empty() {
                    v = json!({"result": v, "images": images});
                }
                (v, true)
            }
            Err(e) => (json!({"error": e}), false),
        };
        // The guide reads best as plain text.
        match &line.0 {
            Value::String(t) => { let _ = writeln!(stdout, "{t}"); }
            v => { let _ = writeln!(stdout, "{v}"); }
        }
        let _ = stdout.flush();
        line.1
    };
    if tool == "-" {
        // Any stream of JSON values (one per line, pretty-printed, or one
        // array of calls): agents write all three. Each call is answered as
        // soon as it arrives, so a caller can read a result before deciding
        // the next call (it used to wait for the end of input).
        let mut ok = true;
        for v in serde_json::Deserializer::from_reader(std::io::stdin().lock()).into_iter::<Value>() {
            let calls = match v {
                Ok(Value::Array(a)) => a,
                Ok(v) => vec![v],
                Err(e) => {
                    println!("{}", json!({"error": format!("stdin is not JSON: {e}")}));
                    ok = false;
                    break;
                }
            };
            for v in calls {
                let t = v.get("tool").and_then(Value::as_str).unwrap_or("").to_owned();
                ok &= one(&mut s, &t, &v.get("args").cloned().unwrap_or(json!({})));
            }
        }
        return if ok { 0 } else { 1 };
    }
    let args: Value = match o.rest.get(1) {
        None => json!({}),
        Some(j) => match serde_json::from_str(j) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("zornov agent: the arguments are not JSON ({e})");
                return 2;
            }
        },
    };
    if one(&mut s, &tool, &args) { 0 } else { 1 }
}
