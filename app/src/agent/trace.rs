// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The flight recorder: every tool call an agent makes, as one line of
//! JSON, so the people working on Zornov see what the agents see.
//!
//! Each call records where it came from (stdio, attach, panel, eval, agent,
//! dev), the tool and its arguments, whether it worked and its error, how
//! long it took, how big the reply was (bytes and estimated tokens), the
//! project revision before and after, and the clips it added, removed or
//! changed. Lines go to `.zornov/agent-trace/<session>.jsonl` in the
//! project folder (Zornov's cache when there is none).
//!
//! **Cheap.** The call itself only takes what it must (sizes, the first
//! text of the reply, the arguments, a shared pointer to the view before
//! and after) and hands it to one writer thread over a channel; building the line, redacting, comparing clips, writing and
//! telling the live taps all happen there. Switched off, a call pays two
//! atomic loads and one revision read. Each line carries `record_us`, what
//! the call path paid, and `queue_us`, how long it waited for the writer.
//!
//! **Private.** Files are `0600` in a `0700` folder (an owner-only ACL on
//! Windows), rotated at 16 MB, at most 8 per session and 256 MB per folder.
//! Media, file contents and anything that looks like a secret (keys, tokens,
//! `Authorization` headers, JWTs, private keys) become `[redacted N bytes]`.
//! Nothing leaves the machine.
//!
//! **Opt-in in release.** On by default in dev builds; in release only
//! with `zornov trace on` or `ZORNOV_TRACE=1`.
//!
//! The live taps (`zornov mcp --observe`) and the in-memory list of recent
//! calls that `zornov dev retry` reads exist only in `agent-dev` builds.

use super::{Content, Session};
use crate::view;
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// The format of a trace line (bumped when a field changes meaning).
pub const VERSION: u32 = 2;

/// The argument key the window's socket adds to say who called; taken off
/// before the tool sees its arguments.
pub(crate) const VIA: &str = "_via";

/// The agent's own report of a surprise.
pub const FEEDBACK: &str = "feedback";

/// Most of one text field of `feedback` that is kept.
pub const FEEDBACK_CAP: usize = 2_000;

/// A file is rotated past this size.
const FILE_CAP: u64 = 16 << 20;
/// Files kept per session (the current one and its rotations).
const FILES_PER_SESSION: usize = 8;
/// The most a trace folder holds; the oldest files go first.
const DIR_CAP: u64 = 256 << 20;
/// Arguments larger than this, once redacted, are kept as their size.
const ARGS_CAP: usize = 64 << 10;
/// A reply larger than this is measured but not excerpted.
const REPLY_CAP: usize = 256 << 10;

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
    /// `zornov dev retry` from the source side (agent-dev builds).
    Dev,
}

impl Source {
    pub fn name(self) -> &'static str {
        match self {
            Source::Stdio => "stdio",
            Source::Attach => "attach",
            Source::Panel => "panel",
            Source::Eval => "eval",
            Source::Agent => "agent",
            Source::Dev => "dev",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "stdio" => Source::Stdio,
            "attach" => Source::Attach,
            "panel" => Source::Panel,
            "eval" => Source::Eval,
            "agent" => Source::Agent,
            "dev" => Source::Dev,
            _ => return None,
        })
    }

    fn from_u8(v: u8) -> Self {
        match v {
            1 => Source::Stdio,
            2 => Source::Attach,
            3 => Source::Eval,
            4 => Source::Agent,
            5 => Source::Dev,
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
            Source::Dev => 5,
        }
    }
}

/// The source of calls that carry no tag: what this process is (the
/// window unless an entry point says otherwise).
static SOURCE: AtomicU8 = AtomicU8::new(0);

pub fn set_source(s: Source) {
    SOURCE.store(s.to_u8(), Ordering::Relaxed);
}

pub fn process_source() -> Source {
    Source::from_u8(SOURCE.load(Ordering::Relaxed))
}

/// A number only this process knows (from the OS's random seed), so a tag
/// in the arguments is believed only when this process put it there: an
/// agent that writes `_via` itself changes nothing.
fn nonce() -> u64 {
    static N: OnceLock<u64> = OnceLock::new();
    *N.get_or_init(|| {
        use std::hash::{BuildHasher, Hasher};
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u64(std::process::id() as u64);
        h.finish()
    })
}

fn tag(args: &mut Value, s: Source) {
    if let Some(o) = args.as_object_mut() {
        o.insert(VIA.into(), json!(format!("{}:{:016x}", s.name(), nonce())));
    }
}

/// The source a tag names, if this process wrote it.
fn parse_tag(v: &Value) -> Option<Source> {
    let (name, n) = v.as_str()?.split_once(':')?;
    (u64::from_str_radix(n, 16).ok()? == nonce()).then(|| Source::parse(name)).flatten()
}

/// Marks a call that came through the window's socket: the panel's own
/// agents when a panel request is running, an outside client otherwise.
/// `zornov eval` keeps its own source.
pub(crate) fn tag_socket_call(args: &mut Value, in_turn: bool) {
    if process_source() == Source::Eval {
        if let Some(o) = args.as_object_mut() {
            o.remove(VIA);
        }
        return;
    }
    tag(args, if in_turn { Source::Panel } else { Source::Attach });
}

/// Marks a call `zornov dev retry` makes.
#[cfg(feature = "agent-dev")]
pub(crate) fn tag_dev_call(args: &mut Value) {
    tag(args, Source::Dev);
}

// ------------------------------------------------------------ switching

/// 0: as the build and setting say; 1: on; 2: off.
static FORCED: AtomicU8 = AtomicU8::new(0);
static SEQ: AtomicU64 = AtomicU64::new(0);

struct Place {
    /// Write here instead of the project's `.zornov/agent-trace`.
    dir: Option<PathBuf>,
    /// The file name, without `.jsonl`.
    session: Option<Arc<str>>,
}

static PLACE: Mutex<Place> = Mutex::new(Place { dir: None, session: None });

fn place() -> std::sync::MutexGuard<'static, Place> {
    PLACE.lock().unwrap_or_else(|e| e.into_inner())
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
/// setting, else on in dev builds (never in unit tests, off in release).
pub fn enabled() -> bool {
    match FORCED.load(Ordering::Relaxed) {
        1 => return true,
        2 => return false,
        _ => {}
    }
    static DEFAULT: OnceLock<bool> = OnceLock::new();
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
    FORCED.store(
        match on {
            None => 0,
            Some(true) => 1,
            Some(false) => 2,
        },
        Ordering::Relaxed,
    );
}

fn session_arc() -> Arc<str> {
    place()
        .session
        .get_or_insert_with(|| {
            let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
            Arc::from(format!("{secs}-{}", std::process::id()))
        })
        .clone()
}

/// This process's session name: when it started and its process id.
pub fn session_id() -> String {
    session_arc().to_string()
}

/// Starts a session of its own, written to `dir` (eval: one per task).
/// Returns the file it goes to.
pub fn begin_session(dir: &Path, name: &str) -> PathBuf {
    flush();
    let mut p = place();
    p.dir = Some(dir.to_path_buf());
    p.session = Some(Arc::from(name));
    force(Some(true));
    dir.join(format!("{}.jsonl", safe_name(name)))
}

/// Back to the project's own folder and a fresh session name, with every
/// line of the last one on disk.
pub fn end_session() {
    flush();
    let mut p = place();
    p.dir = None;
    p.session = None;
    force(None);
}

/// The folder traces of a project go to.
pub fn dir_of(project: &Path) -> PathBuf {
    project.join(".zornov").join("agent-trace")
}

/// A session name as a file name (no separators, no dots in front).
fn safe_name(s: &str) -> String {
    let n: String = s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).take(80).collect();
    if n.is_empty() { "session".into() } else { n }
}

// ------------------------------------------------------------ live taps

#[cfg(feature = "agent-dev")]
static TAPS: Mutex<Vec<Sender<Arc<str>>>> = Mutex::new(Vec::new());
#[cfg(feature = "agent-dev")]
static TAP_COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Every call from now on, as its trace line, pushed the moment the writer
/// has built it (`zornov mcp --observe`).
#[cfg(feature = "agent-dev")]
pub fn tap() -> mpsc::Receiver<Arc<str>> {
    let (tx, rx) = mpsc::channel();
    let mut t = TAPS.lock().unwrap_or_else(|e| e.into_inner());
    t.push(tx);
    TAP_COUNT.store(t.len(), Ordering::Relaxed);
    rx
}

fn tapped() -> bool {
    #[cfg(feature = "agent-dev")]
    return TAP_COUNT.load(Ordering::Relaxed) > 0;
    #[cfg(not(feature = "agent-dev"))]
    false
}

#[cfg(feature = "agent-dev")]
fn broadcast(line: &Value) {
    let mut taps = TAPS.lock().unwrap_or_else(|e| e.into_inner());
    if taps.is_empty() {
        return;
    }
    let text: Arc<str> = Arc::from(line.to_string());
    taps.retain(|t| t.send(text.clone()).is_ok());
    TAP_COUNT.store(taps.len(), Ordering::Relaxed);
}

/// The last calls, with their arguments as they were (memory only, never
/// written): what `zornov dev retry N` runs again.
#[cfg(feature = "agent-dev")]
static RECENT: Mutex<std::collections::VecDeque<(u64, String, Value, Value)>> = Mutex::new(std::collections::VecDeque::new());

/// A recent call by its `seq`: tool, arguments and its trace line.
#[cfg(feature = "agent-dev")]
pub fn recent(seq: u64) -> Option<(String, Value, Value)> {
    RECENT.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|r| r.0 == seq).map(|r| (r.1.clone(), r.2.clone(), r.3.clone()))
}

// ------------------------------------------------------------ the writer

/// What the call path hands over; everything costly is done from it on the
/// writer thread.
pub(crate) struct Pending {
    seq: u64,
    at: u64,
    session: Arc<str>,
    dir: Option<PathBuf>,
    project: Option<PathBuf>,
    source: Source,
    tool: String,
    args: Value,
    ok: bool,
    error: Option<String>,
    first_text: Option<String>,
    reply_bytes: usize,
    reply_tokens: usize,
    images: usize,
    ms: u64,
    rev_before: u64,
    rev_after: u64,
    before: Option<Arc<view::Project>>,
    after: Option<Arc<view::Project>>,
    record_us: u64,
    sent: Instant,
    write: bool,
}

enum Msg {
    Call(Box<Pending>),
    Event(Option<PathBuf>, Value),
    Flush(Sender<()>),
}

static WRITER: OnceLock<Sender<Msg>> = OnceLock::new();

fn writer() -> &'static Sender<Msg> {
    WRITER.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Msg>();
        let _ = std::thread::Builder::new().name("zornov-agent-trace".into()).spawn(move || {
            let mut files = Files::default();
            loop {
                match rx.recv_timeout(Duration::from_millis(200)) {
                    Ok(Msg::Call(p)) => {
                        let queue_us = p.sent.elapsed().as_micros() as u64;
                        let write = p.write;
                        let path = file_for(p.dir.as_deref(), p.project.as_deref(), &p.session);
                        let line = line_of(&p, queue_us);
                        #[cfg(feature = "agent-dev")]
                        {
                            broadcast(&line);
                            let mut r = RECENT.lock().unwrap_or_else(|e| e.into_inner());
                            r.push_back((p.seq, p.tool.clone(), p.args.clone(), line.clone()));
                            while r.len() > 256 {
                                r.pop_front();
                            }
                        }
                        if write {
                            files.write(&path, &line);
                        }
                    }
                    Ok(Msg::Event(project, v)) => {
                        let path = {
                            let pl = place();
                            let session = pl.session.clone();
                            let dir = pl.dir.clone();
                            drop(pl);
                            file_for(dir.as_deref(), project.as_deref(), &session.unwrap_or_else(session_arc))
                        };
                        files.write(&path, &v);
                    }
                    Ok(Msg::Flush(done)) => {
                        files.flush();
                        let _ = done.send(());
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => files.flush(),
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        files.flush();
                        return;
                    }
                }
            }
        });
        tx
    })
}

/// Waits (up to two seconds) until every line handed over is on disk.
pub fn flush() {
    // Nothing was recorded, so there is nothing to write (and no writer to start).
    let Some(w) = WRITER.get() else { return };
    let (tx, rx) = mpsc::channel();
    if w.send(Msg::Flush(tx)).is_ok() {
        let _ = rx.recv_timeout(Duration::from_secs(2));
    }
}

/// Ends the process once what was recorded is on disk: the writer thread
/// does not outlive `exit`, and a short `zornov mcp` or `zornov agent`
/// session would otherwise leave an empty file.
pub fn exit(code: i32) -> ! {
    flush();
    std::process::exit(code)
}

/// A line that is not a call (a hot swap, a timing), in the session's file.
pub fn event(project: Option<&Path>, v: Value) {
    if enabled() {
        let _ = writer().send(Msg::Event(project.map(Path::to_path_buf), v));
    }
}

fn file_for(dir: Option<&Path>, project: Option<&Path>, session: &str) -> PathBuf {
    let dir = dir.map(Path::to_path_buf).unwrap_or_else(|| match project {
        Some(p) => dir_of(p),
        None => studio_paths::cache().join("agent-trace"),
    });
    dir.join(format!("{}.jsonl", safe_name(session)))
}

/// Open trace files, buffered; rotated and pruned as they grow.
#[derive(Default)]
struct Files {
    open: HashMap<PathBuf, (std::io::BufWriter<std::fs::File>, u64)>,
}

impl Files {
    fn write(&mut self, path: &Path, line: &Value) {
        let text = format!("{line}\n");
        if let Some((_, size)) = self.open.get(path) {
            if size + text.len() as u64 > FILE_CAP {
                if let Some((mut w, _)) = self.open.remove(path) {
                    let _ = w.flush();
                }
                rotate(path);
            }
        }
        if !self.open.contains_key(path) {
            let Some(dir) = path.parent() else { return };
            if super::local_only::private_dir(dir).is_err() {
                return;
            }
            prune_dir(dir, path);
            let Ok(f) = super::local_only::append(path) else { return };
            let size = f.metadata().map_or(0, |m| m.len());
            self.open.insert(path.to_path_buf(), (std::io::BufWriter::with_capacity(64 << 10, f), size));
        }
        if let Some((w, size)) = self.open.get_mut(path) {
            if w.write_all(text.as_bytes()).is_ok() {
                *size += text.len() as u64;
            }
        }
    }

    fn flush(&mut self) {
        for (w, _) in self.open.values_mut() {
            let _ = w.flush();
        }
    }
}

/// `<session>.jsonl` → `<session>.<ms>.jsonl`, keeping the newest few.
fn rotate(path: &Path) {
    let (Some(dir), Some(stem)) = (path.parent(), path.file_stem().map(|s| s.to_string_lossy().into_owned())) else { return };
    let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
    let _ = std::fs::rename(path, dir.join(format!("{stem}.{ms}.jsonl")));
    let mut old: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with(&format!("{stem}.")))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    old.sort();
    while old.len() + 1 > FILES_PER_SESSION {
        let (_, p) = old.remove(0);
        let _ = std::fs::remove_file(p);
    }
}

/// The folder kept under its cap, oldest files first (never `keep`).
fn prune_dir(dir: &Path, keep: &Path) {
    let mut files: Vec<(std::time::SystemTime, u64, PathBuf)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "jsonl") && e.path() != keep)
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            Some((m.modified().ok()?, m.len(), e.path()))
        })
        .collect();
    let mut total: u64 = files.iter().map(|f| f.1).sum();
    files.sort();
    for (_, len, p) in files {
        if total <= DIR_CAP {
            break;
        }
        if std::fs::remove_file(&p).is_ok() {
            total -= len;
        }
    }
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
    /// Recording or tapped.
    watching: bool,
    /// The view as it was: a shared pointer, not a copy (the document
    /// replaces its view on change, so holding it costs nothing here).
    clips: Option<Arc<view::Project>>,
    started: Instant,
    new_us: u64,
}

impl<'a> Mark<'a> {
    pub fn new(s: &Session, tool: &str, a: &'a Value) -> Self {
        let t0 = Instant::now();
        let watching = enabled() || tapped();
        let tagged = a.get(VIA);
        let stripped = tagged.map(|_| {
            let mut b = a.clone();
            if let Some(o) = b.as_object_mut() {
                o.remove(VIA);
            }
            b
        });
        // Only a call that may change the project pays for a look at its clips.
        let clips = (watching && !reads_only(tool)).then(|| s.doc.project.clone());
        Mark {
            original: a,
            stripped,
            source: tagged.and_then(parse_tag).unwrap_or_else(process_source),
            rev: s.doc.history.revision(),
            watching,
            clips,
            started: Instant::now(),
            new_us: if watching { t0.elapsed().as_micros() as u64 } else { 0 },
        }
    }

    /// What the tool is given.
    pub fn args(&self) -> &Value {
        self.stripped.as_ref().unwrap_or(self.original)
    }

    /// Hands the call to the writer (and so to the taps).
    pub fn end(mut self, s: &Session, tool: &str, r: &Result<Vec<Content>, String>) {
        if !self.watching {
            return;
        }
        let ms = self.started.elapsed().as_millis() as u64;
        let t0 = Instant::now();
        let rev_after = s.doc.history.revision();
        let after = (self.clips.is_some() && (rev_after != self.rev || matches!(tool, "new_project" | "open"))).then(|| s.doc.project.clone());
        let before = if after.is_some() { self.clips.take() } else { None };
        let project = s.home.clone().or_else(|| s.doc.folder.as_ref().map(|f| f.root().to_path_buf()));
        let args = self.stripped.take().unwrap_or_else(|| self.original.clone());
        let mut p = pending(self.source, tool, args, r, ms, self.rev, rev_after, project);
        p.before = before;
        p.after = after;
        p.record_us = self.new_us + t0.elapsed().as_micros() as u64;
        let _ = writer().send(Msg::Call(Box::new(p)));
    }
}

#[allow(clippy::too_many_arguments)]
fn pending(source: Source, tool: &str, args: Value, r: &Result<Vec<Content>, String>, ms: u64, rev_before: u64, rev_after: u64, project: Option<PathBuf>) -> Pending {
    let (mut bytes, mut tokens, mut images) = (0usize, 0usize, 0usize);
    let mut first_text = None;
    match r {
        Ok(parts) => {
            for part in parts {
                match part {
                    Content::Text(t) => {
                        bytes += t.len();
                        tokens += text_tokens(t);
                        if first_text.is_none() && t.len() <= REPLY_CAP {
                            first_text = Some(t.clone());
                        }
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
    let (dir, session) = {
        let pl = place();
        (pl.dir.clone(), pl.session.clone())
    };
    Pending {
        seq: SEQ.fetch_add(1, Ordering::Relaxed) + 1,
        at: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64),
        session: session.unwrap_or_else(session_arc),
        dir,
        project,
        source,
        tool: tool.to_owned(),
        args,
        ok: r.is_ok(),
        error: r.as_ref().err().map(|e| clip_text(e, 600)),
        first_text,
        reply_bytes: bytes,
        reply_tokens: tokens,
        images,
        ms,
        rev_before,
        rev_after,
        before: None,
        after: None,
        record_us: 0,
        sent: Instant::now(),
        write: enabled(),
    }
}

/// Undo and redo the window answers itself (they never reach a session).
pub fn on_window(tool: &str, args: &Value, r: &Result<Vec<Content>, String>, rev_before: u64, rev_after: u64, project: Option<&Path>) {
    if !(enabled() || tapped()) {
        return;
    }
    let source = args.get(VIA).and_then(parse_tag).unwrap_or_else(process_source);
    let mut a = args.clone();
    if let Some(o) = a.as_object_mut() {
        o.remove(VIA);
    }
    let p = pending(source, tool, a, r, 0, rev_before, rev_after, project.map(Path::to_path_buf));
    let _ = writer().send(Msg::Call(Box::new(p)));
}

/// Tools that never change the project (no clip snapshot for them).
fn reads_only(tool: &str) -> bool {
    matches!(tool, "summary" | "inspect" | "media" | "catalogue" | "guide" | "edit_schema" | "frame" | "contact_sheet" | "capabilities" | "howto" | "see" | "objects" | "listen" | "scopes" | "editorial" | "made" | "fonts" | "voices" | "job" | "export_status" | "check" | "review" | FEEDBACK)
}

fn line_of(p: &Pending, queue_us: u64) -> Value {
    let ops: Vec<&str> = p.args.get("ops").and_then(Value::as_array).map(|v| v.iter().filter_map(|o| o.get("op").and_then(Value::as_str)).collect()).unwrap_or_default();
    let mut args = if p.tool == FEEDBACK { cap_strings(&p.args, FEEDBACK_CAP) } else { redact(&p.args, None) };
    let size = args.to_string().len();
    if size > ARGS_CAP {
        args = json!({"_truncated_bytes": size, "keys": p.args.as_object().map(|o| o.keys().take(20).cloned().collect::<Vec<_>>())});
    }
    let mut o = json!({
        "v": VERSION,
        "seq": p.seq,
        "at": p.at,
        "session": &*p.session,
        "source": p.source.name(),
        "tool": p.tool,
        "module": module_of(&p.tool, &ops),
        "args": args,
        "ok": p.ok,
        "ms": p.ms,
        "reply_bytes": p.reply_bytes,
        "reply_tokens": p.reply_tokens,
        "rev_before": p.rev_before,
        "rev_after": p.rev_after,
        "record_us": p.record_us,
        "queue_us": queue_us,
    });
    if !ops.is_empty() {
        o["ops"] = json!(ops);
    }
    if p.images > 0 {
        o["images"] = json!(p.images);
    }
    if let (Some(b), Some(a)) = (&p.before, &p.after) {
        let c = clip_changes(&b.clips, &a.clips);
        if c.as_object().is_some_and(|c| !c.is_empty()) {
            o["clips"] = c;
        }
    }
    if let Some(e) = &p.error {
        o["error"] = json!(redact_text(e));
    } else if let Some(reply) = p.first_text.as_deref().map(excerpt) {
        o["reply"] = reply;
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

/// Which clips a call added, removed or changed (ids).
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

/// Keys whose string values are media, file contents or code.
const CONTENT_KEYS: &[&str] = &["data", "bytes", "base64", "content", "contents", "source", "pixel", "wgsl", "image_data", "audio_data", "body_text", "file_text"];
/// Words that make a key secret wherever they are in it.
const SECRET_WORDS: &[&str] = &["secret", "token", "password", "passwd", "passphrase", "api_key", "apikey", "authorization", "cookie", "credential", "private_key", "access_key", "session_key", "signature"];
/// Keys that are secret on their own.
const SECRET_KEYS: &[&str] = &["key", "auth", "bearer", "pin", "otp"];
/// How secrets usually start.
const SECRET_PREFIXES: &[&str] = &["sk-", "sk_live_", "sk_test_", "rk_live_", "pk_live_", "ghp_", "gho_", "ghs_", "ghu_", "github_pat_", "glpat-", "xoxb-", "xoxp-", "xoxa-", "AKIA", "ASIA", "AIza", "ya29.", "eyJ", "Bearer ", "Basic ", "-----BEGIN"];

fn secret_key(k: &str) -> bool {
    let k = k.to_lowercase();
    SECRET_KEYS.contains(&k.as_str()) || SECRET_WORDS.iter().any(|w| k.contains(w))
}

/// Whether a string looks like a key, a token or a private key.
fn looks_secret(s: &str) -> bool {
    let t = s.trim();
    if SECRET_PREFIXES.iter().any(|p| t.starts_with(p)) && t.len() >= 16 {
        return true;
    }
    // One long word of letters and digits (no path, no sentence, no name).
    t.len() >= 32
        && !t.contains(|c: char| c.is_whitespace() || c == '/' || c == '\\' || c == '.')
        && t.chars().any(|c| c.is_ascii_digit())
        && t.chars().any(|c| c.is_ascii_alphabetic())
        && t.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '+' | '=' | '~'))
}

/// The arguments as they can be kept: media, file contents and secrets
/// become their size, so a trace never holds a picture, a sound, a file's
/// text or a key.
pub fn redact(v: &Value, key: Option<&str>) -> Value {
    match v {
        Value::String(s) => {
            let by_key = key.is_some_and(|k| secret_key(k) || CONTENT_KEYS.contains(&k.to_lowercase().as_str()));
            let looks_like_media = s.starts_with("data:") || (s.len() > 256 && !s.contains(char::is_whitespace) && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'=' | b'-' | b'_')));
            if by_key || looks_like_media || looks_secret(s) || s.len() > 4_000 {
                json!(format!("[redacted {} bytes]", s.len()))
            } else {
                // Short text too: a token inside a one-line note is still a token.
                json!(redact_text(s))
            }
        }
        Value::Array(a) => Value::Array(a.iter().map(|x| redact(x, key)).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, x)| (k.clone(), redact(x, Some(k)))).collect()),
        // A number under a secret key (a PIN) goes too.
        Value::Number(_) if key.is_some_and(secret_key) => json!("[redacted]"),
        _ => v.clone(),
    }
}

/// Free text (an error, a note) with secret-looking words taken out,
/// `Authorization: …` and `Bearer …` included.
pub fn redact_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut hide_next = false;
    for (i, word) in s.split(' ').enumerate() {
        if i > 0 {
            out.push(' ');
        }
        let bare = word.trim_matches(|c: char| matches!(c, '"' | '\'' | ',' | ';' | '(' | ')' | '[' | ']' | '{' | '}'));
        let lower = bare.to_lowercase();
        if hide_next || looks_secret(bare) || (bare.len() >= 16 && SECRET_PREFIXES.iter().any(|p| bare.starts_with(p.trim_end()))) {
            out.push_str("[redacted]");
            hide_next = false;
        } else {
            out.push_str(word);
            // The value after these words is the secret.
            hide_next = matches!(lower.as_str(), "bearer" | "basic" | "authorization:" | "token:" | "token=" | "password:" | "password=" | "api_key:" | "apikey:" | "x-api-key:");
            if let Some((k, v)) = bare.split_once('=').or_else(|| bare.split_once(':')) {
                if !v.is_empty() && secret_key(k) {
                    out.truncate(out.len() - word.len());
                    out.push_str(&format!("{k}=[redacted]"));
                }
            }
        }
    }
    out
}

/// Every string cut to `n` bytes (the `feedback` text: words, kept short).
fn cap_strings(v: &Value, n: usize) -> Value {
    match v {
        Value::String(s) => json!(redact_text(&clip_text(s, n))),
        Value::Array(a) => Value::Array(a.iter().take(20).map(|x| cap_strings(x, n)).collect()),
        Value::Object(o) => Value::Object(o.iter().take(20).map(|(k, x)| (k.clone(), cap_strings(x, n))).collect()),
        _ => v.clone(),
    }
}

/// The part of a reply the report and replay compare: whether it worked,
/// the revision, notes, the read-back, `check`'s problems; a head of the
/// text when it is not JSON.
pub(crate) fn excerpt(text: &str) -> Value {
    let Ok(Value::Object(o)) = serde_json::from_str::<Value>(text) else {
        return json!({"head": redact_text(&clip_text(text, 300))});
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
                        q.insert(k.into(), redact(v, None));
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
    static OWN: OnceLock<Vec<(String, &'static str)>> = OnceLock::new();
    let own = OWN.get_or_init(|| {
        let lists: [(&'static str, Vec<Value>); 5] = [("care", super::care::tools()), ("search_media", super::search_media::tools()), ("crew", super::crew::tools()), ("skills", super::skills::tools()), ("sound_dept", super::sound_dept::tools())];
        lists.into_iter().flat_map(|(m, ts)| ts.into_iter().filter_map(move |t| Some((t["name"].as_str()?.to_owned(), m)))).collect()
    });
    #[cfg(feature = "agent-dev")]
    if super::hot::serves(tool) {
        return "zornov_agent_hot (hot-swapped)".into();
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
        // A module that lists a tool may only hold its schema (`crew` lists
        // `capabilities`), so the names above come first.
        _ => own.iter().find(|(t, _)| t == tool).map_or("mod", |(_, m)| *m),
    };
    format!("agent::{m}")
}

// ------------------------------------------------------------ feedback

/// The `feedback` tool: cheap, changes nothing, lands in the trace.
pub fn tools() -> Vec<Value> {
    let s = |d: &str| json!({"type": "string", "description": d});
    vec![json!({
        "name": FEEDBACK,
        "description": "Tell Zornov's developers what surprised you. Changes nothing.",
        "inputSchema": {"type": "object", "properties": {
            "expected": s("expected"),
            "got": s("got instead"),
            "tool": s("the tool"),
            "kind": s("surprise, ignored_field, unclear_error, missing, docs, slow, intended"),
            "finding": s("for intended: the finding"),
        }, "required": ["expected", "got"]},
    })]
}

/// Answers `feedback`: the recorder appends it (its text capped) to the
/// trace, and that is all it does.
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
        let a = json!({"ops": [{"op": "text", "text": "NIGHT RIDE", "at": 1, "keys": [[0, 1]]}], "data": "abc", "image": format!("data:image/png;base64,{}", "A".repeat(50)), "api_key": "sk-123", "path": "/tmp/x.mp4", "blob": "Q".repeat(400), "headers": {"Authorization": "Bearer abcdef"}, "note": "use ghp_aaaaaaaaaaaaaaaaaaaaaaaa1 here", "github_token": 12});
        let r = redact(&a, None);
        assert_eq!(r["ops"][0]["text"], "NIGHT RIDE");
        assert_eq!(r["ops"][0]["keys"], json!([[0, 1]]), "keyframes are not keys");
        assert_eq!(r["path"], "/tmp/x.mp4");
        for k in ["data", "image", "api_key", "blob"] {
            assert!(r[k].as_str().unwrap().starts_with("[redacted"), "{k}: {}", r[k]);
        }
        assert!(r["headers"]["Authorization"].as_str().unwrap().starts_with("[redacted"));
        assert!(!r["note"].as_str().unwrap().contains("ghp_"), "{}", r["note"]);
        assert_eq!(r["github_token"], "[redacted]");
    }

    #[test]
    fn secrets_are_taken_out_of_errors() {
        let e = redact_text("request failed: Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.e30.abc token=s3cr3tvalue x");
        assert!(!e.contains("eyJ") && !e.contains("s3cr3t"), "{e}");
        assert!(e.contains("request failed"));
    }

    #[test]
    fn the_source_tag_is_believed_only_from_this_process() {
        let s = Session::default();
        let mut a = json!({"t": 1});
        tag(&mut a, Source::Attach);
        let m = Mark::new(&s, "summary", &a);
        assert_eq!(m.source, Source::Attach);
        assert!(m.args().get(VIA).is_none());
        assert_eq!(m.args()["t"], 1);
        // An agent that writes the tag itself is not believed, and the tool never sees it.
        let forged = json!({"t": 1, VIA: "panel:0000000000000000"});
        let m = Mark::new(&s, "summary", &forged);
        assert_eq!(m.source, process_source());
        assert!(m.args().get(VIA).is_none());
    }

    #[test]
    fn a_line_has_what_the_report_needs() {
        let r: Result<Vec<Content>, String> = Ok(vec![Content::Text(json!({"ok": true, "revision": 4, "read_back": {"clips": [{"clip": 1}], "unchanged": ["op 1 (set) named clip 1, but clip 1 is exactly as it was"]}, "big": "x"}).to_string())]);
        let p = pending(Source::Stdio, "edit", json!({"ops": [{"op": "set", "clip": 1, "blur": 3}]}), &r, 12, 3, 4, None);
        let l = line_of(&p, 7);
        assert_eq!(l["source"], "stdio");
        assert_eq!(l["ops"], json!(["set"]));
        assert_eq!(l["rev_before"], 3);
        assert_eq!(l["queue_us"], 7);
        assert!(l["reply"]["read_back"]["unchanged"].is_array());
        assert!(l["reply"].get("big").is_none(), "only what is compared is kept");
        assert!(l["reply_tokens"].as_u64().unwrap() > 0);
    }

    #[test]
    fn feedback_is_capped_and_needs_both_sides() {
        assert!(feedback(&json!({"expected": "a"})).is_err());
        assert!(feedback(&json!({"expected": "a", "got": "b"})).is_ok());
        let p = pending(Source::Stdio, FEEDBACK, json!({"expected": "x".repeat(10_000), "got": "b"}), &Ok(vec![]), 0, 0, 0, None);
        assert!(line_of(&p, 0)["args"]["expected"].as_str().unwrap().len() <= FEEDBACK_CAP + 4);
    }

    #[test]
    fn a_call_switched_off_costs_next_to_nothing() {
        force(Some(false));
        let s = Session::default();
        let a = json!({});
        let t0 = Instant::now();
        for _ in 0..1_000 {
            let m = Mark::new(&s, "summary", &a);
            m.end(&s, "summary", &Ok(vec![]));
        }
        force(None);
        assert!(t0.elapsed() < Duration::from_millis(50), "{:?} for 1000 calls", t0.elapsed());
    }

    #[test]
    fn files_rotate_and_stay_private() {
        let dir = std::env::temp_dir().join(format!("zornov-trace-rot-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("s.jsonl");
        let mut f = Files::default();
        f.write(&path, &json!({"a": 1}));
        f.open.get_mut(&path).unwrap().1 = FILE_CAP;
        f.write(&path, &json!({"a": 2}));
        f.flush();
        let names: Vec<String> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(names.len(), 2, "{names:?}");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(feature = "agent-dev")]
    #[test]
    fn a_tap_hears_every_call() {
        let rx = tap();
        let s = Session::default();
        let a = json!({"tap_test": true});
        let m = Mark::new(&s, "summary", &a);
        m.end(&s, "summary", &Ok(vec![Content::Text("{}".into())]));
        // Other tests run calls at the same time: find this one.
        let line: Value = (0..20).filter_map(|_| rx.recv_timeout(Duration::from_millis(100)).ok()).filter_map(|l| serde_json::from_str::<Value>(&l).ok()).find(|l| l["args"]["tap_test"] == true).expect("heard");
        assert_eq!(line["tool"], "summary");
        assert_eq!(line["module"], "agent::mod");
    }
}
