// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The flight recorder: every tool call an agent makes, as one line of
//! JSON, so the people working on Zornov see what the agents see.
//!
//! Each call records where it came from (stdio, attach, panel, eval, agent),
//! the tool and its arguments, whether it worked and its error, how long it
//! took, how big the reply was (bytes and estimated tokens), the project
//! revision before and after, and the clips it added, removed or changed.
//! Lines go to `.zornov/agent-trace/<session>.jsonl` in the project folder
//! (Zornov's cache when there is none). Nothing leaves the machine; media,
//! file contents and secrets in the arguments are replaced by their size.
//!
//! On by default in dev builds; in release, `zornov trace on` (or
//! `ZORNOV_TRACE=1`). `zornov trace report` reads the traces
//! ([`super::trace_report`]), `zornov replay` runs them again
//! ([`super::replay`]) and `zornov mcp --observe` follows a window's calls
//! as they happen (every recorded line also goes to the taps here).
//!
//! The `feedback` tool lands here too: the agent says what surprised it,
//! and that goes into the trace and the report.

use super::{Content, Session};
use crate::view;
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Instant;

/// The format of a trace line (bumped when a field changes meaning).
pub const VERSION: u32 = 1;

/// The argument key the window's socket adds to say who called; taken off
/// before the tool sees its arguments.
pub(crate) const VIA: &str = "_via";

/// The agent's own report of a surprise.
pub const FEEDBACK: &str = "feedback";

/// Where a call came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// `zornov mcp` on stdin / stdout, headless.
    Stdio,
    /// An MCP client in the window through its socket (`zornov mcp --attach`).
    Attach,
    /// The AI panel in the window.
    Panel,
    /// `zornov eval`.
    Eval,
    /// `zornov agent` one-off calls, headless.
    Agent,
}

impl Source {
    pub fn name(self) -> &'static str {
        match self {
            Source::Stdio => "stdio",
            Source::Attach => "attach",
            Source::Panel => "panel",
            Source::Eval => "eval",
            Source::Agent => "agent",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "stdio" => Source::Stdio,
            "attach" => Source::Attach,
            "panel" => Source::Panel,
            "eval" => Source::Eval,
            "agent" => Source::Agent,
            _ => return None,
        })
    }

    fn from_u8(v: u8) -> Self {
        match v {
            1 => Source::Stdio,
            2 => Source::Attach,
            3 => Source::Eval,
            4 => Source::Agent,
            _ => Source::Panel,
        }
    }

    fn to_u8(self) -> u8 {
        match self {
            Source::Panel => 0,
            Source::Stdio => 1,
            Source::Attach => 2,
            Source::Eval => 3,
            Source::Agent => 4,
        }
    }
}

/// The source of calls that carry no tag: what this process is (the
/// window unless an entry point says otherwise).
static SOURCE: AtomicU8 = AtomicU8::new(0);

pub fn set_source(s: Source) {
    SOURCE.store(s.to_u8(), Ordering::SeqCst);
}

pub fn process_source() -> Source {
    Source::from_u8(SOURCE.load(Ordering::SeqCst))
}

/// Marks a call that came through the window's socket: the panel's own
/// agents when a panel request is running, an outside client otherwise.
/// `zornov eval` keeps its own source.
pub(crate) fn tag_socket_call(args: &mut Value, in_turn: bool) {
    if process_source() == Source::Eval {
        return;
    }
    if let Some(o) = args.as_object_mut() {
        o.insert(VIA.into(), json!(if in_turn { Source::Panel } else { Source::Attach }.name()));
    }
}

// ------------------------------------------------------------ switching

struct State {
    /// On or off whatever the build and setting say (eval, tests).
    forced: Option<bool>,
    /// Write here instead of the project's `.zornov/agent-trace`.
    dir: Option<PathBuf>,
    /// The file name, without `.jsonl`.
    session: Option<String>,
}

static STATE: Mutex<State> = Mutex::new(State { forced: None, dir: None, session: None });
static SEQ: AtomicU64 = AtomicU64::new(0);
/// Appends are one at a time, so lines from several threads never mix.
static WRITE: Mutex<()> = Mutex::new(());

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// The release setting: `zornov trace on|off`.
fn setting_path() -> PathBuf {
    studio_paths::data_dir().join("agent-trace.json")
}

pub fn set_setting(on: bool) -> Result<PathBuf, String> {
    let p = setting_path();
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    std::fs::write(&p, json!({"on": on}).to_string()).map_err(|e| e.to_string())?;
    Ok(p)
}

fn setting() -> Option<bool> {
    let t = std::fs::read_to_string(setting_path()).ok()?;
    serde_json::from_str::<Value>(&t).ok()?["on"].as_bool()
}

/// Whether calls are written down: forced, else `ZORNOV_TRACE`, else the
/// setting, else on in dev builds (never in unit tests).
pub fn enabled() -> bool {
    if let Some(f) = state().forced {
        return f;
    }
    static DEFAULT: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *DEFAULT.get_or_init(|| {
        if let Ok(v) = std::env::var("ZORNOV_TRACE") {
            return !matches!(v.trim(), "" | "0" | "off" | "false" | "no");
        }
        if cfg!(test) {
            return false;
        }
        setting().unwrap_or(cfg!(debug_assertions))
    })
}

pub fn force(on: Option<bool>) {
    state().forced = on;
}

/// This process's session name: when it started and its process id.
pub fn session_id() -> String {
    let mut s = state();
    s.session
        .get_or_insert_with(|| {
            let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
            format!("{secs}-{}", std::process::id())
        })
        .clone()
}

/// Starts a session of its own, written to `dir` (eval: one per task).
/// Returns the file it goes to.
pub fn begin_session(dir: &Path, name: &str) -> PathBuf {
    let mut s = state();
    s.forced = Some(true);
    s.dir = Some(dir.to_path_buf());
    s.session = Some(name.to_owned());
    dir.join(format!("{name}.jsonl"))
}

/// Back to the project's own folder and a fresh session name.
pub fn end_session() {
    let mut s = state();
    s.dir = None;
    s.session = None;
    s.forced = None;
}

/// The folder traces of a project go to.
pub fn dir_of(project: &Path) -> PathBuf {
    project.join(".zornov").join("agent-trace")
}

fn file_for(project: Option<&Path>) -> PathBuf {
    let session = session_id();
    let dir = state().dir.clone().unwrap_or_else(|| match project {
        Some(p) => dir_of(p),
        None => studio_paths::cache().join("agent-trace"),
    });
    dir.join(format!("{session}.jsonl"))
}

fn write(project: Option<&Path>, line: &Value) {
    let path = file_for(project);
    let _one = WRITE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(d) = path.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        use std::io::Write;
        let _ = writeln!(f, "{line}");
    }
}

// ------------------------------------------------------------ live taps

static TAPS: Mutex<Vec<Sender<String>>> = Mutex::new(Vec::new());

/// Every call from now on, as its trace line (`zornov mcp --observe`).
pub fn tap() -> Receiver<String> {
    let (tx, rx) = mpsc::channel();
    TAPS.lock().unwrap_or_else(|e| e.into_inner()).push(tx);
    rx
}

fn tapped() -> bool {
    !TAPS.lock().unwrap_or_else(|e| e.into_inner()).is_empty()
}

fn broadcast(line: &Value) {
    let mut taps = TAPS.lock().unwrap_or_else(|e| e.into_inner());
    if taps.is_empty() {
        return;
    }
    let text = line.to_string();
    taps.retain(|t| t.send(text.clone()).is_ok());
}

// ------------------------------------------------------------ one call

/// A call as it begins: its arguments without the source tag, and what the
/// project was, to say afterwards what it changed.
pub struct Mark<'a> {
    original: &'a Value,
    /// The arguments without [`VIA`], when it was there.
    stripped: Option<Value>,
    source: Source,
    rev: u64,
    clips: Option<Vec<view::Clip>>,
    started: Instant,
}

impl<'a> Mark<'a> {
    pub fn new(s: &Session, a: &'a Value) -> Self {
        let via = a.get(VIA).and_then(Value::as_str).and_then(Source::parse);
        let stripped = a.get(VIA).map(|_| {
            let mut b = a.clone();
            if let Some(o) = b.as_object_mut() {
                o.remove(VIA);
            }
            b
        });
        let watching = enabled() || tapped();
        Mark {
            original: a,
            stripped,
            source: via.unwrap_or_else(process_source),
            rev: s.doc.history.revision(),
            clips: watching.then(|| s.doc.project.clips.clone()),
            started: Instant::now(),
        }
    }

    /// What the tool is given.
    pub fn args(&self) -> &Value {
        self.stripped.as_ref().unwrap_or(self.original)
    }

    /// Writes the call down (and tells the taps).
    pub fn end(self, s: &Session, tool: &str, r: &Result<Vec<Content>, String>) {
        let Some(before) = &self.clips else { return };
        let rev_after = s.doc.history.revision();
        let clips = if rev_after != self.rev || tool == "new_project" || tool == "open" { clip_changes(before, &s.doc.project.clips) } else { json!({}) };
        let project = s.home.clone().or_else(|| s.doc.folder.as_ref().map(|f| f.root().to_path_buf()));
        let line = line_of(self.source, tool, self.args(), r, self.started.elapsed().as_millis() as u64, self.rev, rev_after, clips);
        if enabled() {
            write(project.as_deref(), &line);
        }
        broadcast(&line);
    }
}

/// Undo and redo the window answers itself (they never reach a session).
pub fn on_window(tool: &str, args: &Value, r: &Result<Vec<Content>, String>, rev_before: u64, rev_after: u64, project: Option<&Path>) {
    if !(enabled() || tapped()) {
        return;
    }
    let source = args.get(VIA).and_then(Value::as_str).and_then(Source::parse).unwrap_or_else(process_source);
    let mut a = args.clone();
    if let Some(o) = a.as_object_mut() {
        o.remove(VIA);
    }
    let line = line_of(source, tool, &a, r, 0, rev_before, rev_after, json!({}));
    if enabled() {
        write(project, &line);
    }
    broadcast(&line);
}

#[allow(clippy::too_many_arguments)]
fn line_of(source: Source, tool: &str, a: &Value, r: &Result<Vec<Content>, String>, ms: u64, rev_before: u64, rev_after: u64, clips: Value) -> Value {
    let (mut bytes, mut tokens, mut images) = (0usize, 0usize, 0usize);
    let mut first_text: Option<&str> = None;
    match r {
        Ok(parts) => {
            for p in parts {
                match p {
                    Content::Text(t) => {
                        bytes += t.len();
                        tokens += text_tokens(t);
                        first_text.get_or_insert(t);
                    }
                    Content::Jpeg(b) => {
                        // Sent as base64.
                        bytes += b.len().div_ceil(3) * 4;
                        tokens += image_tokens(b);
                        images += 1;
                    }
                }
            }
        }
        Err(e) => {
            bytes = e.len();
            tokens = text_tokens(e);
        }
    }
    let ops: Vec<&str> = a.get("ops").and_then(Value::as_array).map(|v| v.iter().filter_map(|o| o.get("op").and_then(Value::as_str)).collect()).unwrap_or_default();
    let mut o = json!({
        "v": VERSION,
        "seq": SEQ.fetch_add(1, Ordering::SeqCst) + 1,
        "at": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64),
        "session": session_id(),
        "source": source.name(),
        "tool": tool,
        "module": module_of(tool, &ops),
        "args": redact(a, None),
        "ok": r.is_ok(),
        "ms": ms,
        "reply_bytes": bytes,
        "reply_tokens": tokens,
        "rev_before": rev_before,
        "rev_after": rev_after,
    });
    if !ops.is_empty() {
        o["ops"] = json!(ops);
    }
    if images > 0 {
        o["images"] = json!(images);
    }
    if clips.as_object().is_some_and(|c| !c.is_empty()) {
        o["clips"] = clips;
    }
    match r {
        Err(e) => o["error"] = json!(clip_text(e, 600)),
        Ok(_) => {
            if let Some(reply) = first_text.map(excerpt) {
                o["reply"] = reply;
            }
        }
    }
    o
}

/// About four bytes a token for English and JSON.
pub fn text_tokens(t: &str) -> usize {
    t.len().div_ceil(4)
}

/// What a picture costs a model: about width × height / 750 tokens (the
/// size is read from the JPEG; a guess when it cannot be).
pub fn image_tokens(jpeg: &[u8]) -> usize {
    match jpeg_size(jpeg) {
        Some((w, h)) => ((w as usize * h as usize) / 750).max(1),
        None => 1_500,
    }
}

/// Width and height from a JPEG's frame header.
fn jpeg_size(j: &[u8]) -> Option<(u32, u32)> {
    let mut i = 2;
    while i + 9 < j.len() {
        if j[i] != 0xFF {
            i += 1;
            continue;
        }
        let marker = j[i + 1];
        let len = u16::from_be_bytes([j[i + 2], j[i + 3]]) as usize;
        if matches!(marker, 0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF) {
            let h = u16::from_be_bytes([j[i + 5], j[i + 6]]) as u32;
            let w = u16::from_be_bytes([j[i + 7], j[i + 8]]) as u32;
            return Some((w, h));
        }
        i += 2 + len;
    }
    None
}

fn clip_text(s: &str, n: usize) -> String {
    if s.len() <= n {
        return s.to_owned();
    }
    let mut end = n;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

/// Which clips a call added, removed or changed (ids, and for a change
/// the fields that moved).
fn clip_changes(before: &[view::Clip], after: &[view::Clip]) -> Value {
    use std::collections::BTreeMap;
    let b: BTreeMap<u32, &view::Clip> = before.iter().map(|c| (c.id, c)).collect();
    let a: BTreeMap<u32, &view::Clip> = after.iter().map(|c| (c.id, c)).collect();
    let added: Vec<u32> = a.keys().filter(|k| !b.contains_key(k)).copied().collect();
    let removed: Vec<u32> = b.keys().filter(|k| !a.contains_key(k)).copied().collect();
    let changed: Vec<u32> = a.iter().filter(|(k, c)| b.get(k).is_some_and(|was| was != *c)).map(|(k, _)| *k).collect();
    let mut o = Map::new();
    if !added.is_empty() {
        o.insert("added".into(), json!(added));
    }
    if !removed.is_empty() {
        o.insert("removed".into(), json!(removed));
    }
    if !changed.is_empty() {
        o.insert("changed".into(), json!(changed));
    }
    Value::Object(o)
}

// ------------------------------------------------------------ redaction

/// Keys whose string values are never written down: media, file contents,
/// code and secrets.
const SECRET: &[&str] = &["data", "bytes", "base64", "content", "contents", "source", "pixel", "wgsl", "image_data", "audio_data", "token", "api_key", "apikey", "password", "secret", "authorization", "cookie"];

/// The arguments as they can be kept: media and file contents become
/// their size, so a trace never holds a picture, a sound, a file's text or
/// a key.
pub fn redact(v: &Value, key: Option<&str>) -> Value {
    match v {
        Value::String(s) => {
            let secret = key.is_some_and(|k| SECRET.contains(&k.to_lowercase().as_str()));
            let looks_like_media = s.starts_with("data:") || (s.len() > 256 && !s.contains(char::is_whitespace) && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'=' | b'-' | b'_')));
            if secret || looks_like_media || s.len() > 4_000 {
                json!(format!("[redacted {} bytes]", s.len()))
            } else {
                v.clone()
            }
        }
        Value::Array(a) => Value::Array(a.iter().map(|x| redact(x, key)).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, x)| (k.clone(), redact(x, Some(k)))).collect()),
        _ => v.clone(),
    }
}

/// The part of a reply the report and replay compare: whether it worked,
/// the revision, notes, the read-back, `check`'s problems; a head of the
/// text when it is not JSON.
pub(crate) fn excerpt(text: &str) -> Value {
    let Ok(Value::Object(o)) = serde_json::from_str::<Value>(text) else {
        return json!({"head": clip_text(text, 300)});
    };
    const KEEP: &[&str] = &["ok", "revision", "notes", "warnings", "ignored", "unknown", "undone", "redone", "job", "state", "fixed", "error", "export", "duration"];
    let mut out = Map::new();
    for k in KEEP {
        if let Some(v) = o.get(*k) {
            out.insert((*k).into(), redact(v, None));
        }
    }
    if let Some(rb) = o.get("read_back") {
        out.insert("read_back".into(), redact(rb, None));
    }
    if let Some(Value::Array(ps)) = o.get("problems") {
        let short: Vec<Value> = ps
            .iter()
            .take(40)
            .map(|p| {
                let mut q = Map::new();
                for k in ["kind", "clip", "t", "at", "message", "what"] {
                    if let Some(v) = p.get(k) {
                        q.insert(k.into(), v.clone());
                    }
                }
                Value::Object(q)
            })
            .collect();
        out.insert("problems".into(), json!(short));
    }
    if out.is_empty() {
        out.insert("keys".into(), json!(o.keys().take(12).collect::<Vec<_>>()));
    }
    Value::Object(out)
}

// ------------------------------------------------------------ modules

/// The Rust module that answers a tool (for the live tap), relative to
/// `app/src/agent/`. Tools listed by a module of their own say so; the
/// rest were read off `Session::call`.
pub fn module_of(tool: &str, ops: &[&str]) -> String {
    // Read once: (tool, module) for the tools other modules list.
    static OWN: std::sync::OnceLock<Vec<(String, &'static str)>> = std::sync::OnceLock::new();
    let own = OWN.get_or_init(|| {
        let lists: [(&'static str, Vec<Value>); 5] = [("care", super::care::tools()), ("search_media", super::search_media::tools()), ("crew", super::crew::tools()), ("skills", super::skills::tools()), ("sound_dept", super::sound_dept::tools())];
        lists.into_iter().flat_map(|(m, ts)| ts.into_iter().filter_map(move |t| Some((t["name"].as_str()?.to_owned(), m)))).collect()
    });
    if let Some((_, m)) = own.iter().find(|(t, _)| t == tool) {
        return format!("agent::{m}");
    }
    let m = match tool {
        FEEDBACK => "trace",
        "edit" => return format!("agent::mod (edit: {})", if ops.is_empty() { "no ops".into() } else { ops.join(", ") }),
        "capabilities" => "capabilities",
        "fonts" => "fonts",
        "parts_check" => "parts",
        "taste" => "taste",
        "plan" => "crew",
        "howto" => "crate::help",
        "guide" => "guides",
        "skill" => "skills",
        "listen" => "listen",
        "capture" => "capture",
        "graph" => "graph",
        "editorial" => "editorial",
        "plugins" => "plugins",
        "automation" => "automation",
        "layout" => "live (window)",
        "save_move" | "save_design" | "made" => "made",
        "match" => "matcher",
        "script" | "notes" => "script",
        "see" => "sight",
        "objects" => "objects",
        "story" => "story",
        "comments" | "versions" | "share" | "team" => "collab_tools",
        "production" => "production",
        "account" => "account_tools",
        "check" => "quality",
        "review" => "review",
        "pick" => "select",
        "canvas_layout" => "layout",
        "generate" => "generate",
        "picture" => "picture",
        "vector" | "illustrate" => "vector",
        "enhance" => "generative",
        "read" => "web",
        "job" => "jobs",
        "download" => "download",
        "edit_schema" => "schema",
        "analyse" | "analyze" => "analyse",
        "scopes" => "scopes",
        "viewport" => "viewport",
        "colour" | "conform" | "media_manage" => "post",
        "frame" | "contact_sheet" | "animatic" => "render",
        "compare_cameras" => "shots",
        "queue" | "qc" | "deliver" | "bundle" | "cutdown" => "deliver_tools",
        "undo" | "redo" | "open" | "save" | "new_project" => "mod (document)",
        _ => "mod",
    };
    format!("agent::{m}")
}

// ------------------------------------------------------------ feedback

/// The `feedback` tool: cheap, changes nothing, lands in the trace.
pub fn tools() -> Vec<Value> {
    let s = |d: &str| json!({"type": "string", "description": d});
    vec![json!({
        "name": FEEDBACK,
        "description": "Tell Zornov's developers what surprised you (an ignored field, an unclear error, a missing op, a check finding that is intended). Free, changes nothing; then carry on.",
        "inputSchema": {"type": "object", "properties": {
            "expected": s("what you expected"),
            "got": s("what happened"),
            "tool": s("the tool"),
            "kind": s("surprise, ignored_field, unclear_error, missing, docs, slow, or intended (a check finding you mean)"),
            "finding": s("intended: the finding"),
        }, "required": ["expected", "got"]},
    })]
}

/// Answers `feedback` (the recorder writes it down with the call).
pub fn feedback(a: &Value) -> Result<Vec<Content>, String> {
    let has = |k: &str| a.get(k).and_then(Value::as_str).is_some_and(|s| !s.trim().is_empty());
    if !has("expected") || !has("got") {
        return Err("feedback needs expected and got: what you expected, and what happened instead".into());
    }
    let kept = enabled() || tapped();
    Ok(vec![Content::Text(json!({"ok": true, "noted": kept, "message": "Thanks: noted for Zornov's developers. Carry on."}).to_string())])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_and_secrets_are_redacted_and_the_rest_kept() {
        let a = json!({"ops": [{"op": "text", "text": "NIGHT RIDE", "at": 1}], "data": "abc", "image": format!("data:image/png;base64,{}", "A".repeat(50)), "api_key": "sk-123", "path": "/tmp/x.mp4", "blob": "Q".repeat(400)});
        let r = redact(&a, None);
        assert_eq!(r["ops"][0]["text"], "NIGHT RIDE");
        assert_eq!(r["path"], "/tmp/x.mp4");
        for k in ["data", "image", "api_key", "blob"] {
            assert!(r[k].as_str().unwrap().starts_with("[redacted"), "{k}: {}", r[k]);
        }
    }

    #[test]
    fn the_source_tag_is_taken_off_before_the_tool_sees_it() {
        let s = Session::default();
        let a = json!({"t": 1, VIA: "attach"});
        let m = Mark::new(&s, &a);
        assert_eq!(m.source, Source::Attach);
        assert!(m.args().get(VIA).is_none());
        assert_eq!(m.args()["t"], 1);
    }

    #[test]
    fn a_line_has_what_the_report_needs() {
        let r: Result<Vec<Content>, String> = Ok(vec![Content::Text(json!({"ok": true, "revision": 4, "read_back": {"clips": [{"clip": 1}], "unchanged": ["op 1 (set) named clip 1, but clip 1 is exactly as it was"]}, "big": "x"}).to_string())]);
        let l = line_of(Source::Stdio, "edit", &json!({"ops": [{"op": "set", "clip": 1, "blur": 3}]}), &r, 12, 3, 4, json!({"changed": [1]}));
        assert_eq!(l["source"], "stdio");
        assert_eq!(l["ops"], json!(["set"]));
        assert_eq!(l["rev_before"], 3);
        assert_eq!(l["clips"]["changed"], json!([1]));
        assert!(l["reply"]["read_back"]["unchanged"].is_array());
        assert!(l["reply"].get("big").is_none(), "only what is compared is kept");
        assert!(l["reply_tokens"].as_u64().unwrap() > 0);
    }

    #[test]
    fn feedback_needs_both_sides() {
        assert!(feedback(&json!({"expected": "a"})).is_err());
        assert!(feedback(&json!({"expected": "a", "got": "b"})).is_ok());
    }

    #[test]
    fn a_tap_hears_every_call() {
        let rx = tap();
        let s = Session::default();
        let a = json!({"tap_test": true});
        let m = Mark::new(&s, &a);
        m.end(&s, "summary", &Ok(vec![Content::Text("{}".into())]));
        // Other tests run calls at the same time: find this one.
        let line: Value = rx.try_iter().filter_map(|l| serde_json::from_str::<Value>(&l).ok()).find(|l| l["args"]["tap_test"] == true).expect("heard");
        assert_eq!(line["tool"], "summary");
        assert_eq!(line["module"], "agent::mod");
    }
}
