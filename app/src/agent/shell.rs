// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `zornov` in a terminal: a prompt like any other terminal tool.
//!
//! ```text
//! zornov                       the Zornov prompt in this project folder
//! zornov new my-video          a new project in ./my-video
//! zornov see                   one command and out (any command below)
//! zornov card colour=red at=0 duration=2
//! zornov make "a 10 second intro about the ocean"
//! zornov app                   the editor window (zornov open DIR: on that project)
//! zornov help [command]
//! ```
//!
//! At the prompt, a line that starts with a command runs it; anything else
//! is said to the AI editor, which works in the same project with the same
//! tools. Commands take `key=value` (values as JSON when they read as JSON:
//! numbers, true, [1, 2]) and the first field without a key. The project is
//! the folder you are in, or the nearest one above it with a project.

use super::{Content, Session};
use serde_json::{Value, json};
use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};

/// Every edit op, by the name you type.
pub(super) const OPS: &[&str] = &[
    "key_background", "polygon_mask", "scene", "music", "narrate", "place", "add", "text", "card", "sfx", "sing", "fx_layer", "split", "trim", "delete", "move", "reorder", "roll", "slide", "slip", "duplicate", "freeze", "replace", "detach_audio", "link", "unlink", "group", "ungroup", "merge", "unmerge", "set", "animate", "keys", "apply", "effect", "copy_look", "mask", "particles", "living", "puppet", "lip_sync", "scissor", "mask_puppet", "corner_pin", "three_d", "camera", "light", "atmosphere", "model", "mesh", "parallax", "motion", "transition", "reframe", "stagger", "modifier", "physics", "driver", "flatten", "unflatten", "align", "distribute", "parent", "matte", "restack", "audio", "use_move", "use_design", "lane", "marker", "keyframe", "raw",
];

/// The field a bare word fills, per command (plural fields take several).
fn positional(cmd: &str) -> &'static [&'static str] {
    match cmd {
        "guide" => &["topic"],
        "skill" => &["name"],
        "howto" => &["query"],
        "inspect" => &["clip", "at"],
        "frame" => &["t"],
        "graph" => &["clip", "property"],
        "scopes" => &["t", "compare"],
        "viewport" => &["action"],
        "export" => &["path"],
        "assets" => &["action", "query"],
        "project_care" => &["action", "to"],
        "import" => &["paths"],
        "download" => &["url"],
        "search_media" => &["query"],
        "generate" => &["prompt"],
        "picture" => &["ops"],
        "enhance" => &["clip"],
        "catalogue" => &["panel", "query"],
        "pick" => &["x", "y", "at"],
        "canvas_layout" => &["view", "clip", "t"],
        "notes" => &["add"],
        "listen" => &["media"],
        "made" => &["kind"],
        "open" => &["path"],
        "summary" | "script" => &["since"],
        op => match super::args::op_fields(op) {
            Some(f) if !f.is_empty() => match op {
                "delete" | "reframe" | "group" | "merge" | "link" | "unlink" => &["clips"],
                _ => std::slice::from_ref(&f[0]),
            },
            _ => &[],
        },
    }
}

fn tools() -> Vec<String> {
    super::mcp::tools().iter().filter_map(|t| t["name"].as_str().map(str::to_owned)).collect()
}

fn is_command(w: &str) -> bool {
    OPS.contains(&w) || w == "repeat" || tools().iter().any(|t| t == w) || matches!(w, "help" | "new" | "make" | "ask" | "app" | "exit" | "quit" | "model" | "clear")
}

/// What a typed value means: JSON when it reads as JSON, else text.
fn value(s: &str) -> Value {
    match serde_json::from_str::<Value>(s) {
        Ok(v @ (Value::Number(_) | Value::Bool(_) | Value::Array(_) | Value::Object(_) | Value::Null)) => v,
        _ => Value::String(s.to_owned()),
    }
}

/// `card colour=red at=0` → the tool to run and its arguments.
pub(crate) fn parse(words: &[String]) -> Result<(String, Value), String> {
    let cmd = words.first().ok_or("nothing to run")?.clone();
    let mut args = serde_json::Map::new();
    let mut loose = Vec::new();
    for w in &words[1..] {
        match w.split_once('=') {
            Some((k, v)) if !k.is_empty() && !k.contains(' ') => {
                args.insert(k.to_owned(), value(v));
            }
            _ => loose.push(value(w)),
        }
    }
    let slots = positional(&cmd);
    let mut it = loose.into_iter();
    for (i, slot) in slots.iter().enumerate() {
        let plural = matches!(*slot, "clips" | "paths");
        if plural && i == slots.len() - 1 {
            let rest: Vec<Value> = it.by_ref().collect();
            if !rest.is_empty() && !args.contains_key(*slot) {
                args.insert((*slot).to_owned(), if rest.len() == 1 && rest[0].is_array() { rest[0].clone() } else { Value::Array(rest) });
            }
        } else if let Some(v) = it.next() {
            args.insert((*slot).to_owned(), v);
        }
    }
    if let Some(extra) = it.next() {
        return Err(format!("\"{}\" is extra: say which field it is (field=value); `zornov help {cmd}` lists them", extra.as_str().map(str::to_owned).unwrap_or(extra.to_string())));
    }
    // An edit op on its own is an edit of one op.
    if OPS.contains(&cmd.as_str()) || cmd == "repeat" {
        args.insert("op".into(), json!(cmd));
        return Ok(("edit".into(), json!({"ops": [Value::Object(args)]})));
    }
    if cmd == "edit" {
        if let Some(Value::String(path)) = args.get("ops").cloned() {
            let text = std::fs::read_to_string(&path).map_err(|e| format!("ops={path}: {e}"))?;
            args.insert("ops".into(), serde_json::from_str(&text).map_err(|e| format!("{path} is not a JSON list of ops: {e}"))?);
        }
    }
    Ok((cmd, Value::Object(args)))
}

/// The project folder for `here`: itself or the nearest parent with one.
fn find_project(here: &Path) -> Option<PathBuf> {
    here.ancestors().find(|d| d.join("project.json").is_file()).map(Path::to_path_buf)
}

struct Style {
    on: bool,
}

impl Style {
    fn paint(&self, code: &str, s: &str) -> String {
        if self.on { format!("\x1b[{code}m{s}\x1b[0m") } else { s.to_owned() }
    }
    fn dim(&self, s: &str) -> String {
        self.paint("2", s)
    }
    fn bold(&self, s: &str) -> String {
        self.paint("1", s)
    }
    fn red(&self, s: &str) -> String {
        self.paint("31", s)
    }
    fn accent(&self, s: &str) -> String {
        self.paint("36", s)
    }
}

/// Runs one command and prints what came back. False on an error.
fn run_one(s: &mut Session, dir: Option<&Path>, cmd: &str, args: &Value, st: &Style, raw: bool) -> bool {
    let out_dir = dir.map(|d| d.join("agent-frames")).unwrap_or_else(|| std::env::temp_dir().join("studio-agent"));
    // For a person, the timeline as lines rather than data.
    let cmd = if !raw && cmd == "summary" { "script" } else { cmd };
    // With the project open in a Studio window, the command runs there:
    // the person sees it happen and nothing is overwritten later.
    let r = match dir.and_then(|d| super::attach::find_window(Some(d))) {
        Some(sock) => super::attach::call_window(&sock, cmd, args),
        None => super::guard::run(cmd, || s.run(cmd, args)),
    };
    match r {
        Ok(parts) => {
            // Next to a picture, its small bookkeeping says nothing new.
            let pictured = parts.iter().any(|p| matches!(p, Content::Jpeg(_)));
            for p in parts {
                if let (true, false, Content::Text(t)) = (pictured, raw, &p) {
                    if t.len() < 80 && t.starts_with('{') {
                        continue;
                    }
                }
                match p {
                    Content::Text(t) => {
                        let v = serde_json::from_str::<Value>(&t).unwrap_or(Value::String(t));
                        if raw {
                            println!("{v}");
                        } else {
                            print!("{}", human(&v, st));
                        }
                    }
                    Content::Jpeg(b) => match crate::files::write_once(&out_dir, cmd, "jpg", &b) {
                        Ok(path) => println!("{} {}", st.dim("picture:"), path.display()),
                        Err(e) => eprintln!("{} could not write the picture: {e}", st.red("error:")),
                    },
                }
            }
            true
        }
        Err(e) => {
            if raw {
                println!("{}", json!({"error": e}));
            } else {
                eprintln!("{} {e}", st.red("error:"));
            }
            false
        }
    }
}

/// A result as a person reads it.
fn human(v: &Value, st: &Style) -> String {
    let mut out = String::new();
    match v {
        Value::String(t) => {
            out.push_str(t);
            if !t.ends_with('\n') {
                out.push('\n');
            }
        }
        Value::Object(o) if o.contains_key("problems") => {
            let probs = o["problems"].as_array().cloned().unwrap_or_default();
            if probs.is_empty() {
                out.push_str(&format!("{} nothing to fix", st.accent("✓")));
            } else {
                out.push_str(&format!("{} {} to fix\n", st.bold("check:"), probs.len()));
                for p in probs {
                    let at = p.get("at").and_then(Value::as_f64).map(|t| format!("{t:>6.2}s ")).unwrap_or_else(|| "        ".into());
                    out.push_str(&format!("  {}{}\n          {} {}\n", st.dim(&at), p["problem"].as_str().unwrap_or(""), st.dim("fix:"), p["fix"].as_str().unwrap_or("")));
                }
            }
            if let Some(snd) = o.get("sound").filter(|s| !s.is_null()) {
                out.push_str(&format!("\n{} {} LUFS, peak {} dB", st.dim("sound:"), snd["loudness_lufs"], snd["peak_db"]));
            }
            out.push('\n');
        }
        Value::Object(o) if o.contains_key("notes") || o.contains_key("revision") && o.contains_key("changed") => {
            for n in o.get("notes").and_then(Value::as_array).into_iter().flatten() {
                out.push_str(&format!("{} {}\n", st.accent("•"), n.as_str().unwrap_or(&n.to_string())));
            }
            let dur = o.get("duration").and_then(Value::as_f64).map(|d| format!(" · {d:.2}s long")).unwrap_or_default();
            out.push_str(&st.dim(&format!("revision {}{dur}\n", o.get("revision").map(|r| r.to_string()).unwrap_or_default())));
        }
        Value::Object(o) if o.contains_key("path") && o.contains_key("took_seconds") => {
            out.push_str(&format!("{} {} ({}s long, made in {}s)\n", st.accent("exported"), o["path"].as_str().unwrap_or(""), o["duration"], o["took_seconds"]));
        }
        Value::Object(o) if o.contains_key("undone") || o.contains_key("redone") => {
            let (what, label) = if o.contains_key("undone") { ("undid", &o["undone"]) } else { ("redid", &o["redone"]) };
            out.push_str(&format!("{} {}\n", st.accent(what), label.as_str().unwrap_or("")));
        }
        other => outline(other, 0, &mut out, st),
    }
    out
}

/// Data as indented `key: value` lines: lists of plain values on one line,
/// lists of records as `-` items.
fn outline(v: &Value, depth: usize, out: &mut String, st: &Style) {
    let pad = "  ".repeat(depth);
    let plain = |v: &Value| !matches!(v, Value::Object(_) | Value::Array(_)) || v.as_array().is_some_and(|a| a.iter().all(|x| !matches!(x, Value::Object(_) | Value::Array(_))) && a.len() <= 12) || v.as_array().is_some_and(|a| a.is_empty()) || v.as_object().is_some_and(|o| o.is_empty());
    let line = |v: &Value| match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    match v {
        Value::Object(o) => {
            for (k, x) in o {
                if plain(x) {
                    out.push_str(&format!("{pad}{} {}\n", st.dim(&format!("{k}:")), line(x)));
                } else {
                    out.push_str(&format!("{pad}{}\n", st.dim(&format!("{k}:"))));
                    outline(x, depth + 1, out, st);
                }
            }
        }
        Value::Array(a) => {
            for x in a {
                if plain(x) {
                    out.push_str(&format!("{pad}- {}\n", line(x)));
                } else {
                    out.push_str(&format!("{pad}-\n"));
                    outline(x, depth + 1, out, st);
                }
            }
        }
        other => out.push_str(&format!("{pad}{}\n", line(other))),
    }
}

fn help(topic: Option<&str>, st: &Style) {
    if let Some(cmd) = topic.filter(|t| !t.is_empty()) {
        if let Some(t) = super::mcp::tools().into_iter().find(|t| t["name"] == cmd) {
            println!("{}  {}\n", st.bold(cmd), t["description"].as_str().unwrap_or(""));
            if let Some(props) = t["inputSchema"]["properties"].as_object() {
                for (k, p) in props {
                    println!("  {:<12} {}", st.accent(k), p["description"].as_str().unwrap_or(""));
                }
            }
            let pos = positional(cmd);
            if !pos.is_empty() {
                println!("\n  {} zornov {cmd} {}", st.dim("short:"), pos.iter().map(|p| format!("<{p}>")).collect::<Vec<_>>().join(" "));
            }
            return;
        }
        if let Some(fields) = super::args::op_fields(cmd) {
            println!("{}  an edit\n\n  fields: {}", st.bold(cmd), fields.join(", "));
            let pos = positional(cmd);
            if let Some(p) = pos.first() {
                println!("  {} zornov {cmd} <{p}> field=value …", st.dim("short:"));
            }
            println!("\n  `zornov guide ops` explains every field.");
            return;
        }
        println!("no command \"{cmd}\"; `zornov help` lists them");
        return;
    }
    println!(
        "{}

  zornov                 the Zornov prompt in this project
  zornov new NAME        a new project in ./NAME  (--vertical, --square, --size 1080x1920, --fps 60)
  zornov make \"...\"      ask the AI editor to make or change something
  zornov app             the editor window   (zornov open DIR: on that project)
  zornov COMMAND ...     one command, e.g.  zornov see   zornov check   zornov export out.mp4

{}  summary  script  see  listen  inspect CLIP  pick X Y  frame T  check  media  catalogue PANEL [QUERY]
{}  import FILE…  text \"Hi\" at=1  card colour=red at=0  place MEDIA at=2  set CLIP x=100
          animate clip=CLIP property=x keys=[[0,0],[1,500]]  trim  split  delete  move  transition …
          undo  redo  export [PATH]   (`zornov help COMMAND` shows its fields)
{}  guide [TOPIC]   the full reference
",
        st.bold("Zornov, from the terminal"),
        st.dim("look  "),
        st.dim("change"),
        st.dim("learn ")
    );
}

/// `zornov new [NAME] [--vertical | --square | --size WxH] [--fps N]`
fn new_project(args: &[String], st: &Style) -> i32 {
    let mut name = None;
    let (mut w, mut h, mut fps) = (1920u32, 1080u32, 30.0f64);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--vertical" | "-v" => (w, h) = (1080, 1920),
            "--square" => (w, h) = (1080, 1080),
            "--size" => {
                let s = it.next().cloned().unwrap_or_default();
                match s.split_once('x').and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?))) {
                    Some(v) => (w, h) = v,
                    None => {
                        eprintln!("{} --size is WIDTHxHEIGHT, like 1080x1920", st.red("error:"));
                        return 2;
                    }
                }
            }
            "--fps" => fps = it.next().and_then(|v| v.parse().ok()).unwrap_or(30.0),
            other if !other.starts_with('-') => name = Some(other.to_owned()),
            other => {
                eprintln!("{} unknown option {other}", st.red("error:"));
                return 2;
            }
        }
    }
    let here = std::env::current_dir().unwrap_or_default();
    let dir = match &name {
        Some(n) => here.join(n),
        None => here.clone(),
    };
    if dir.join("project.json").is_file() {
        eprintln!("{} there is already a project in {}", st.red("error:"), dir.display());
        return 1;
    }
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!("{} {e}", st.red("error:"));
        return 1;
    }
    let title = name.clone().unwrap_or_else(|| dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "Untitled".into()));
    let mut s = match session(Some(&dir)) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{} {e}", st.red("error:"));
            return 1;
        }
    };
    match s.run("new_project", &json!({"name": title, "width": w, "height": h, "fps": fps})) {
        Ok(_) => {
            println!("{} {} {}", st.accent("made"), st.bold(&title), st.dim(&format!("({w}×{h}, {fps} fps) in {}", dir.display())));
            match &name {
                Some(n) => println!("next: {} then {}", st.bold(&format!("cd {n}")), st.bold("zornov")),
                None => println!("next: {}", st.bold("zornov")),
            }
            0
        }
        Err(e) => {
            eprintln!("{} {e}", st.red("error:"));
            1
        }
    }
}

fn session(dir: Option<&Path>) -> Result<Session, String> {
    let _ = crate::content::get_blocking();
    match dir {
        Some(d) => Session::at(d),
        None => Ok(Session::default()),
    }
}

/// One event from the AI editor, as lines to print, and what it is doing.
fn ai_event(v: &Value, resume: &mut Option<String>, st: &Style, doing: &mut String, ok: &mut bool) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(id) = v.get("session_id").and_then(Value::as_str) {
        *resume = Some(id.to_owned());
    }
    match v["type"].as_str() {
        Some("assistant") => {
            for c in v["message"]["content"].as_array().into_iter().flatten() {
                match c["type"].as_str() {
                    Some("text") => {
                        let t = c["text"].as_str().unwrap_or("").trim();
                        if !t.is_empty() {
                            out.push(format!("{} {}", st.accent("●"), t.replace('\n', "\n  ")));
                        }
                    }
                    Some("tool_use") => {
                        let name = c["name"].as_str().unwrap_or("").trim_start_matches("mcp__studio__");
                        let what = match name {
                            "edit" => {
                                let ops: Vec<&str> = c["input"]["ops"].as_array().into_iter().flatten().filter_map(|o| o["op"].as_str()).collect();
                                format!("edit  {}", if ops.len() > 6 { format!("{} changes", ops.len()) } else { ops.join(", ") })
                            }
                            "frame" | "contact_sheet" | "see" => format!("{name}  looking"),
                            n => n.to_owned(),
                        };
                        *doing = match name {
                            "edit" => "editing".into(),
                            "see" | "frame" | "contact_sheet" => "looking".into(),
                            "check" => "checking".into(),
                            "export" => "exporting".into(),
                            "listen" => "listening".into(),
                            "guide" => "reading the guide".into(),
                            _ => "working".into(),
                        };
                        out.push(format!("  {}", st.dim(&format!("⎿ {what}"))));
                    }
                    _ => {}
                }
            }
        }
        Some("result") => {
            *ok = !v["is_error"].as_bool().unwrap_or(false);
            let secs = v["duration_ms"].as_f64().unwrap_or(0.0) / 1000.0;
            out.push(st.dim(&format!("  done in {secs:.0}s")));
        }
        _ => {}
    }
    out
}

/// Says `brief` to the AI editor (Claude Code, driving this project's
/// tools); `resume` carries the conversation from one turn to the next.
/// In a terminal a spinner runs while it works and Esc stops it.
fn ai_turn(dir: &Path, brief: &str, resume: &mut Option<String>, model: Option<&str>, st: &Style) -> bool {
    let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_else(|_| "zornov".into());
    let cfg = json!({"mcpServers": {"studio": {"command": exe, "args": ["mcp", "--project", dir.display().to_string()]}}}).to_string();
    let mut cmd = std::process::Command::new("claude");
    cmd.current_dir(dir).args(["-p", brief, "--mcp-config", &cfg, "--strict-mcp-config", "--tools", "", "--allowedTools", "mcp__studio", "--output-format", "stream-json", "--verbose"]);
    if let Some(id) = resume.as_deref() {
        cmd.args(["--resume", id]);
    }
    if let Some(m) = model {
        cmd.args(["--model", m]);
    }
    cmd.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(_) => {
            eprintln!("{} the AI editor needs Claude Code (`claude`) installed; every command works without it", st.red("error:"));
            return false;
        }
    };
    let (tx, rx) = std::sync::mpsc::channel::<Value>();
    if let Some(out) = child.stdout.take() {
        std::thread::spawn(move || {
            for line in std::io::BufReader::new(out).lines().map_while(Result::ok) {
                if let Ok(v) = serde_json::from_str::<Value>(&line) {
                    if tx.send(v).is_err() {
                        break;
                    }
                }
            }
        });
    }
    let live = st.on && std::io::stdin().is_terminal();
    let raw = if live { super::term::Raw::on() } else { None };
    let started = std::time::Instant::now();
    let frames = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
    let mut tick = 0usize;
    let mut doing = String::from("thinking");
    let mut ok = true;
    let mut stopped = false;
    let clear = |live: bool| {
        if live {
            print!("\r\x1b[2K");
        }
    };
    loop {
        match rx.recv_timeout(std::time::Duration::from_millis(90)) {
            Ok(v) => {
                let lines = ai_event(&v, resume, st, &mut doing, &mut ok);
                if !lines.is_empty() {
                    clear(live);
                    for l in lines {
                        println!("{l}");
                    }
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
        if live {
            if super::term::ready(0) {
                if let Some(super::term::Key::Esc | super::term::Key::CtrlC) = super::term::key() {
                    let _ = child.kill();
                    stopped = true;
                }
            }
            tick += 1;
            print!("\r\x1b[2K{} {}… {}", st.accent(&frames[tick % frames.len()].to_string()), doing, st.dim(&format!("{}s · esc to stop", started.elapsed().as_secs())));
            let _ = std::io::stdout().flush();
        }
    }
    clear(live);
    drop(raw);
    let status = child.wait();
    if stopped {
        println!("{}", st.dim("  stopped; what it changed so far is kept (undo takes it back)"));
        return false;
    }
    if !status.as_ref().is_ok_and(|s| s.success()) && ok {
        let mut err = String::new();
        if let Some(mut e) = child.stderr.take() {
            let _ = std::io::Read::read_to_string(&mut e, &mut err);
        }
        eprintln!("{} the AI editor stopped: {}", st.red("error:"), err.lines().last().unwrap_or("no reason given"));
        return false;
    }
    ok
}

/// What the input box knows: every command with a line about it, and the
/// fields each takes.
fn catalog() -> super::term::Catalog {
    let mut commands: Vec<(String, String)> = Vec::new();
    for (n, d) in [("help", "every command, or one in detail"), ("new", "a new project: new NAME [--vertical]"), ("app", "open the editor window on this project"), ("clear", "clear the screen"), ("model", "which model the AI editor uses: model haiku | sonnet | opus"), ("exit", "leave")] {
        commands.push((n.into(), d.into()));
    }
    for t in super::mcp::tools() {
        let name = t["name"].as_str().unwrap_or("").to_owned();
        if matches!(name.as_str(), "edit_schema" | "open" | "new_project" | "export_status" | "ai") {
            continue;
        }
        let d = t["description"].as_str().unwrap_or("");
        // The gist: up to the first full stop, bracket, colon or semicolon.
        let first = d.split([':', ';', '(']).next().unwrap_or(d).split(". ").next().unwrap_or(d).trim().trim_end_matches(['.', ',']).to_owned();
        commands.push((name, first));
    }
    for op in OPS.iter().chain(["repeat"].iter()) {
        let fields = super::args::op_fields(op).map(|f| f.join(" ")).unwrap_or_default();
        commands.push(((*op).into(), format!("change: {fields}")));
    }
    let fields = |cmd: &str| -> Vec<String> {
        if let Some(f) = super::args::op_fields(cmd) {
            return f.iter().map(|s| (*s).to_owned()).collect();
        }
        super::mcp::tools().into_iter().find(|t| t["name"] == cmd).and_then(|t| t["inputSchema"]["properties"].as_object().map(|o| o.keys().cloned().collect())).unwrap_or_default()
    };
    super::term::Catalog { commands, fields: Box::new(fields) }
}

fn banner(s: &Session, dir: Option<&Path>, st: &Style) {
    let w = (super::term::width().saturating_sub(1)).clamp(30, 100);
    let line = |t: &str, plain: usize| format!("{} {}{}{}", st.dim("│"), t, " ".repeat((w - 3).saturating_sub(plain)), st.dim("│"));
    println!("{}", st.dim(&format!("╭{}╮", "─".repeat(w - 2))));
    match dir {
        Some(d) => {
            let p = &s.doc.project;
            let head = format!("✻ Zornov  {}", p.name);
            println!("{}", line(&format!("{} {}", st.accent("✻"), st.bold(&format!("Zornov  {}", p.name))), head.chars().count()));
            let info = format!("{}×{} · {:.1}s · {} clips · {}", p.width, p.height, p.duration() as f64 / 1e6, p.clips.len(), d.display());
            let info: String = info.chars().take(w - 6).collect();
            println!("{}", line(&format!("  {}", st.dim(&info)), info.chars().count() + 2));
        }
        None => {
            println!("{}", line(&format!("{} {}", st.accent("✻"), st.bold("Zornov")), 8));
            println!("{}", line(&format!("  {}", st.dim("no project here: /new NAME makes one")), 39));
        }
    }
    println!("{}", line("", 0));
    let tip = "type what you want made · / for commands · ↑ history";
    println!("{}", line(&format!("  {}", st.dim(tip)), tip.chars().count() + 2));
    println!("{}", st.dim(&format!("╰{}╯", "─".repeat(w - 2))));
}

/// The interactive prompt.
fn repl(dir: Option<PathBuf>, st: &Style) -> i32 {
    let mut s = match session(dir.as_deref()) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{} {e}", st.red("error:"));
            return 1;
        }
    };
    banner(&s, dir.as_deref(), st);
    let cat = catalog();
    let history = std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".studio_history"));
    let mut ed = super::term::Editor::new(history);
    let paint = super::term::Paint { on: st.on };
    let mut dir = dir;
    let mut resume: Option<String> = None;
    let mut model: Option<String> = None;
    loop {
        let p = &s.doc.project;
        let status = match &dir {
            Some(_) => format!("{} · {:.1}s · {} clips · revision {}", p.name, p.duration() as f64 / 1e6, p.clips.len(), s.doc.history.revision()),
            None => "no project · /new NAME".to_owned(),
        };
        let line = match ed.read(&cat, &status, &paint) {
            super::term::Outcome::Exit => return 0,
            super::term::Outcome::Line(l) => l,
        };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let slash = line.starts_with('/');
        let line = line.trim_start_matches('/');
        let words = match split_words(line) {
            Ok(w) => w,
            Err(e) => {
                eprintln!("{} {e}", st.red("error:"));
                continue;
            }
        };
        // Only `/…` is a command (and a bare exit); every other line is
        // said to the AI editor, whatever word it starts with.
        let first = if slash || matches!(line, "exit" | "quit" | "q" | "help" | "?" | "clear") { words.first().map(String::as_str).unwrap_or("") } else { "" };
        match first {
            "exit" | "quit" | "q" => return 0,
            "help" | "?" => help(words.get(1).map(String::as_str), st),
            "clear" => print!("\x1b[2J\x1b[H"),
            "model" => {
                model = words.get(1).cloned();
                println!("{}", st.dim(&format!("  the AI editor uses {}", model.as_deref().unwrap_or("Claude Code's default model"))));
            }
            "app" => {
                open_app(dir.as_deref());
                println!("{}", st.dim("  the editor window is opening"));
            }
            "new" => {
                let code = new_project(&words[1..], st);
                if code == 0 {
                    let here = std::env::current_dir().unwrap_or_default();
                    let d = words.get(1).filter(|w| !w.starts_with('-')).map(|n| here.join(n)).unwrap_or(here);
                    if let Ok(ns) = session(Some(&d)) {
                        s = ns;
                        dir = Some(d);
                        resume = None;
                    }
                }
            }
            w if !w.is_empty() && !matches!(w, "make" | "ask") => match parse(&words) {
                Ok((cmd, args)) => {
                    if dir.is_none() && !matches!(cmd.as_str(), "guide" | "howto" | "catalogue" | "made") {
                        eprintln!("{} make a project first: /new NAME", st.red("error:"));
                        continue;
                    }
                    run_one(&mut s, dir.as_deref(), &cmd, &args, st, false);
                }
                Err(e) => eprintln!("{} {e}", st.red("error:")),
            },
            _ => {
                let Some(d) = dir.clone() else {
                    eprintln!("{} make a project first: /new NAME", st.red("error:"));
                    continue;
                };
                let brief = match first {
                    "make" | "ask" => line[first.len()..].trim().to_owned(),
                    _ => line.to_owned(),
                };
                // Changes made here are on disk before the AI reads them
                // (with a window open on the project, the AI works there).
                if super::attach::find_window(Some(&d)).is_none() {
                    let _ = s.run("save", &json!({}));
                }
                ai_turn(&d, &brief, &mut resume, model.as_deref(), st);
                // The AI changed the project on disk: read it again.
                if let Ok(ns) = session(Some(&d)) {
                    s = ns;
                }
            }
        }
    }
}

/// Splits a line like a shell does: spaces, "quotes", 'quotes'.
pub(crate) fn split_words(line: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut any = false;
    for ch in line.chars() {
        match (quote, ch) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => cur.push(c),
            (None, '"' | '\'') => {
                quote = Some(ch);
                any = true;
            }
            (None, c) if c.is_whitespace() => {
                if !cur.is_empty() || any {
                    out.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            (None, c) => cur.push(c),
        }
    }
    if quote.is_some() {
        return Err("a quote is not closed".into());
    }
    if !cur.is_empty() || any {
        out.push(cur);
    }
    Ok(out)
}

fn open_app(dir: Option<&Path>) {
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("zornov"));
    let mut c = std::process::Command::new(exe);
    c.arg("--window");
    if let Some(d) = dir {
        c.arg("--open").arg(d);
    }
    let _ = c.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn();
}

/// The terminal side of `zornov`. `None`: open the window instead.
pub fn main(argv: &[String]) -> Option<i32> {
    let tty = std::io::stdout().is_terminal();
    let st = Style { on: tty && std::env::var_os("NO_COLOR").is_none() };
    let here = std::env::current_dir().unwrap_or_default();
    let first = argv.first().map(String::as_str);
    // The window: from Finder / the Dock (no terminal), `zornov app`, or
    // its own flags.
    match first {
        None if !std::io::stdin().is_terminal() => return None,
        Some("--window") => return None,
        Some(f) if f.starts_with("--") && !matches!(f, "--help" | "--json" | "--project") => return None,
        Some(p) if Path::new(p).join("project.json").is_file() => return None,
        _ => {}
    }
    // --project DIR and --json may come anywhere.
    let mut raw = false;
    let mut project: Option<PathBuf> = None;
    let mut words: Vec<String> = Vec::new();
    let mut it = argv.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--json" => raw = true,
            "--project" | "-p" => project = it.next().map(PathBuf::from),
            _ => words.push(a.clone()),
        }
    }
    let dir = project.map(|p| std::path::absolute(&p).unwrap_or(p)).or_else(|| find_project(&here));
    let cmd = words.first().cloned();
    Some(match cmd.as_deref() {
        None => repl(dir, &st),
        Some("help" | "-h" | "--help") => {
            help(words.get(1).map(String::as_str), &st);
            0
        }
        Some("new") => new_project(&words[1..], &st),
        Some("app") => {
            open_app(dir.as_deref());
            0
        }
        Some("open") if words.len() <= 2 => {
            let d = words.get(1).map(|p| std::path::absolute(Path::new(p)).unwrap_or_else(|_| PathBuf::from(p))).or(dir);
            open_app(d.as_deref());
            0
        }
        Some("make" | "ask") => {
            let Some(d) = dir else {
                eprintln!("{} no project here: `zornov new NAME` makes one", st.red("error:"));
                return Some(1);
            };
            let mut model = None;
            let mut brief = Vec::new();
            let mut ws = words[1..].iter();
            while let Some(w) = ws.next() {
                if w == "--model" {
                    model = ws.next().cloned();
                } else {
                    brief.push(w.clone());
                }
            }
            if brief.is_empty() {
                eprintln!("{} say what to make: zornov make \"a 10 second intro about the ocean\"", st.red("error:"));
                return Some(2);
            }
            let mut resume = None;
            if ai_turn(&d, &brief.join(" "), &mut resume, model.as_deref(), &st) { 0 } else { 1 }
        }
        Some(c) if is_command(c) => {
            let needs_project = !matches!(c, "guide" | "howto" | "catalogue" | "edit_schema" | "made");
            if needs_project && dir.is_none() {
                eprintln!("{} no Zornov project here. Make one: {}", st.red("error:"), st.bold("zornov new my-video"));
                return Some(1);
            }
            let (tool, args) = match parse(&words) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("{} {e}", st.red("error:"));
                    return Some(2);
                }
            };
            let mut s = match session(dir.as_deref()) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("{} {e}", st.red("error:"));
                    return Some(1);
                }
            };
            if run_one(&mut s, dir.as_deref(), &tool, &args, &st, raw || !tty) { 0 } else { 1 }
        }
        Some(other) => {
            eprintln!("{} no command \"{other}\". {}", st.red("error:"), st.dim("`zornov help` lists them; to ask the AI editor: zornov make \"…\""));
            2
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(s: &str) -> Vec<String> {
        split_words(s).unwrap()
    }

    #[test]
    fn commands_read_like_a_shell() {
        assert_eq!(w(r#"text "Hello there" at=1 colour='#ff0000'"#), vec!["text", "Hello there", "at=1", "colour=#ff0000"]);
        let (tool, args) = parse(&w(r#"text "Hello" at=1.5 size=90"#)).unwrap();
        assert_eq!(tool, "edit");
        assert_eq!(args, json!({"ops": [{"op": "text", "text": "Hello", "at": 1.5, "size": 90}]}));
        let (tool, args) = parse(&w("inspect 9 1.5")).unwrap();
        assert_eq!((tool.as_str(), args), ("inspect", json!({"clip": 9, "at": 1.5})));
        let (_, args) = parse(&w("import a.mp4 b.mp3")).unwrap();
        assert_eq!(args, json!({"paths": ["a.mp4", "b.mp3"]}));
        let (_, args) = parse(&w("delete 3 4")).unwrap();
        assert_eq!(args["ops"][0]["clips"], json!([3, 4]));
        let (_, args) = parse(&w("animate clip=9 property=x keys=[[0,0],[1,500]]")).unwrap();
        assert_eq!(args["ops"][0]["keys"], json!([[0, 0], [1, 500]]));
        assert!(parse(&w("see 1 2 3")).is_err());
    }
}
