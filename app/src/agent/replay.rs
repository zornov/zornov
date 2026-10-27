// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `zornov replay <trace> [--project DIR] [--call N | --upto N] [--emit-test]`:
//! a recorded session (or one call of it) run again on this build, and
//! what came out compared with what came out then.
//!
//! The calls run on a copy of `--project` (a blank project without it),
//! never the project itself, with the recorder off. For each call it says
//! whether the outcome is the same: worked or failed, the error, whether
//! the project changed, and the read-back of the clips.
//!
//! A trace is untrusted input (anyone could hand you one), so every call is
//! checked before it runs: only tools on a short list that stay inside the
//! project (edit, undo, the reads and looks), no op that reaches outside
//! (voices, generation, scripts, plugins), arguments of a sane size, and
//! every path inside the project copy (a path into the original project is
//! moved into the copy; any other path, URL or `..` and the call is
//! skipped). No shell, no external program, nothing from the network; a
//! skipped call says why.
//!
//! `--emit-test` writes each call that failed in the recording as a
//! fixture in `app/tests/agent_replays/`: the calls before it that changed
//! the project, the call, and its outcome on this build from a blank
//! project. The test at the end of this file runs every fixture, so a
//! behaviour agents ran into stays fixed (or stays understood).

use super::trace_report::{Trace, load};
use super::{Content, Session};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// The only tools a replay runs: they act on the project or read it.
const ALLOWED: &[&str] = &["edit", "undo", "redo", "summary", "inspect", "media", "catalogue", "guide", "edit_schema", "check", "frame", "contact_sheet", "capabilities", "howto", "editorial", "canvas_layout", "scopes", "fonts"];
/// Ops that reach outside the project (voices, generation, code), never replayed.
const DENIED_OPS: &[&str] = &["narrate", "sing", "dub", "voice", "generate", "download", "plugin", "script", "shell", "run", "exec", "import_camera", "export_camera", "raw"];
/// Keys whose strings are paths.
const PATH_KEYS: &[&str] = &["path", "paths", "file", "files", "out", "dir", "folder", "image", "images", "end_image", "style_ref", "refs", "media", "audio", "video", "font", "lut", "camera_file"];
/// Most arguments one replayed call may carry.
const ARGS_MAX: usize = 256 << 10;

/// The outcome of one call, as the trace keeps it.
#[derive(Clone, Debug, PartialEq)]
pub struct Outcome {
    pub ok: bool,
    pub error: Option<String>,
    pub changed: bool,
    pub read_back: Value,
    pub unchanged: Value,
}

impl Outcome {
    pub(crate) fn of_recorded(c: &Value) -> Self {
        Outcome {
            ok: c["ok"].as_bool().unwrap_or(false),
            error: c["error"].as_str().map(first_line),
            changed: c["rev_before"] != c["rev_after"],
            read_back: strip_ids(&c["reply"]["read_back"]["clips"]),
            unchanged: c["reply"]["read_back"]["unchanged"].clone(),
        }
    }

    pub(crate) fn of_run(r: &Result<Vec<Content>, String>, changed: bool) -> Self {
        let reply = match r {
            Ok(parts) => parts.iter().find_map(|p| if let Content::Text(t) = p { Some(super::trace::excerpt(t)) } else { None }).unwrap_or(Value::Null),
            Err(_) => Value::Null,
        };
        Outcome {
            ok: r.is_ok(),
            error: r.as_ref().err().map(|e| first_line(e)),
            changed,
            read_back: strip_ids(&reply["read_back"]["clips"]),
            unchanged: reply["read_back"]["unchanged"].clone(),
        }
    }

    /// What differs, in words (empty: the same).
    pub fn diff(&self, now: &Outcome) -> Vec<String> {
        let mut d = Vec::new();
        if self.ok != now.ok {
            d.push(if now.ok { format!("failed then ({}), works now", self.error.as_deref().unwrap_or("")) } else { format!("worked then, fails now: {}", now.error.as_deref().unwrap_or("")) });
        } else if self.error != now.error {
            d.push(format!("error was \"{}\", now \"{}\"", self.error.as_deref().unwrap_or(""), now.error.as_deref().unwrap_or("")));
        }
        if self.changed != now.changed {
            d.push(format!("{} the project then, {} now", if self.changed { "changed" } else { "left" }, if now.changed { "changes it" } else { "leaves it" }));
        }
        if self.read_back != now.read_back && !(self.read_back.is_null() && now.read_back.is_null()) {
            d.push(format!("read-back was {}, now {}", short(&self.read_back), short(&now.read_back)));
        }
        if self.unchanged != now.unchanged && !(self.unchanged.is_null() && now.unchanged.is_null()) {
            d.push(format!("ops that did nothing: then {}, now {}", short(&self.unchanged), short(&now.unchanged)));
        }
        d
    }
}

fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or("").chars().take(200).collect()
}

fn short(v: &Value) -> String {
    let s = v.to_string();
    if s.chars().count() <= 160 { s } else { format!("{}…", s.chars().take(160).collect::<String>()) }
}

/// Clip ids are numbered afresh in a copy; the read-back is compared without them.
fn strip_ids(v: &Value) -> Value {
    match v {
        Value::Array(a) => Value::Array(a.iter().map(strip_ids).collect()),
        Value::Object(o) => Value::Object(o.iter().filter(|(k, _)| k.as_str() != "clip").map(|(k, x)| (k.clone(), strip_ids(x))).collect()),
        _ => v.clone(),
    }
}

/// Where the recorded project was, and its copy (paths move from one to the other).
#[derive(Clone, Default)]
pub struct Confine {
    pub from: Option<PathBuf>,
    pub to: Option<PathBuf>,
}

/// The call to run, checked: its tool and arguments with every path inside
/// the copy; `Err` says why it is skipped.
pub fn checked(c: &Value, confine: &Confine) -> Result<(String, Value), String> {
    let tool = c["tool"].as_str().ok_or("no tool")?;
    if tool.len() > 64 || !tool.bytes().all(|b| b.is_ascii_lowercase() || b == b'_') {
        return Err("not a tool name".into());
    }
    if tool == super::trace::FEEDBACK {
        return Err("feedback (nothing to run)".into());
    }
    if !ALLOWED.contains(&tool) {
        return Err(format!("{tool} is not replayed (only tools that stay inside the project are)"));
    }
    let mut args = c.get("args").cloned().unwrap_or_else(|| json!({}));
    if !args.is_object() {
        return Err("its arguments are not an object".into());
    }
    let text = args.to_string();
    if text.len() > ARGS_MAX {
        return Err(format!("its arguments are {} bytes", text.len()));
    }
    if text.contains("[redacted") || args.get("_truncated_bytes").is_some() {
        return Err("its arguments were redacted".into());
    }
    if let Some(op) = args["ops"].as_array().into_iter().flatten().filter_map(|o| o["op"].as_str()).find(|o| DENIED_OPS.contains(o)) {
        return Err(format!("op {op} reaches outside the project"));
    }
    confine_paths(&mut args, None, confine)?;
    Ok((tool.to_owned(), args))
}

/// Every path-like string moved into the copy, or an error.
fn confine_paths(v: &mut Value, key: Option<&str>, c: &Confine) -> Result<(), String> {
    match v {
        Value::String(s) => {
            let lower = s.to_lowercase();
            if lower.starts_with("http:") || lower.starts_with("https:") || lower.starts_with("ftp:") || lower.starts_with("file:") {
                return Err(format!("it names a link ({})", short(&json!(s))));
            }
            let pathy = key.is_some_and(|k| PATH_KEYS.contains(&k)) && (s.contains('/') || s.contains('\'));
            let absolute = s.starts_with('/') || s.starts_with('~') || s.starts_with('\') || (s.len() > 2 && s.as_bytes()[1] == b':' && s.as_bytes()[0].is_ascii_alphabetic());
            if !(pathy || absolute) {
                return Ok(());
            }
            let p = std::path::Path::new(s.as_str());
            if p.components().any(|x| matches!(x, std::path::Component::ParentDir)) {
                return Err(format!("it names a path with .. ({s})"));
            }
            let moved = match (&c.from, &c.to) {
                (Some(from), Some(to)) if p.is_absolute() => p.strip_prefix(from).ok().map(|rest| to.join(rest)),
                (_, Some(to)) if p.is_relative() && !s.starts_with('~') => Some(to.join(p)),
                _ => None,
            };
            match moved {
                Some(m) => {
                    *s = m.display().to_string();
                    Ok(())
                }
                None => Err(format!("it names a path outside the project copy ({s})")),
            }
        }
        Value::Array(a) => a.iter_mut().try_for_each(|x| confine_paths(x, key, c)),
        Value::Object(o) => o.iter_mut().try_for_each(|(k, x)| confine_paths(x, Some(k.as_str()), c)),
        _ => Ok(()),
    }
}

/// A copy of a project folder to replay on (its traces left behind).
fn copy_project(from: &Path) -> Result<PathBuf, String> {
    let to = std::env::temp_dir().join(format!("zornov-replay-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis())));
    copy_dir(from, &to).map_err(|e| format!("cannot copy {}: {e}", from.display()))?;
    Ok(to)
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)?.flatten() {
        let p = e.path();
        if p.file_name().is_some_and(|n| n == "agent-trace" || n == "agent-usage.jsonl") {
            continue;
        }
        // Links are not followed: a copy holds the project's own files only.
        let Ok(kind) = std::fs::symlink_metadata(&p).map(|m| m.file_type()) else { continue };
        if kind.is_symlink() {
            continue;
        }
        let dest = to.join(e.file_name());
        if kind.is_dir() {
            copy_dir(&p, &dest)?;
        } else {
            std::fs::copy(&p, &dest)?;
        }
    }
    Ok(())
}

fn session_on(project: Option<&Path>) -> Result<(Session, Option<PathBuf>), String> {
    let _ = crate::content::get_blocking();
    match project {
        Some(dir) => {
            let copy = copy_project(dir)?;
            let doc = crate::services::project::open(&copy)?;
            let mut s = Session::new(doc);
            s.home = Some(copy.clone());
            Ok((s, Some(copy)))
        }
        None => Ok((Session::default(), None)),
    }
}

/// Runs one call and reads its outcome.
pub fn run_call(s: &mut Session, tool: &str, args: &Value) -> Outcome {
    let before = s.doc.history.revision();
    let r = super::guard::run(tool, || s.run(tool, args));
    Outcome::of_run(&r, s.doc.history.revision() != before)
}

/// One replayed call.
pub struct Step {
    pub call: usize,
    pub tool: String,
    pub skipped: Option<String>,
    pub diff: Vec<String>,
}

/// Replays `trace`'s calls `from..=to` (1-based) on `s`.
pub fn replay(s: &mut Session, trace: &Trace, from: usize, to: usize, confine: &Confine) -> Vec<Step> {
    let mut steps = Vec::new();
    for (i, c) in trace.calls.iter().enumerate().skip(from.saturating_sub(1)).take(to + 1 - from.max(1)) {
        let tool: String = c["tool"].as_str().unwrap_or("?").chars().take(64).collect();
        let (tool, args) = match checked(c, confine) {
            Ok(x) => x,
            Err(why) => {
                steps.push(Step { call: i + 1, tool, skipped: Some(why), diff: Vec::new() });
                continue;
            }
        };
        let now = run_call(s, &tool, &args);
        let diff = Outcome::of_recorded(c).diff(&now);
        steps.push(Step { call: i + 1, tool, skipped: None, diff });
    }
    steps
}

// ------------------------------------------------------------ fixtures

/// Where `--emit-test` writes.
pub fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests").join("agent_replays")
}

/// A fixture for call `n` (1-based) of `trace`: the calls before it that
/// changed the project, the call, and what it does on this build from a
/// blank project.
pub fn fixture(trace: &Trace, n: usize) -> Value {
    let c = &trace.calls[n - 1];
    let setup: Vec<Value> = trace.calls[..n - 1]
        .iter()
        .filter(|p| p["ok"] == json!(true) && p["rev_before"] != p["rev_after"] && !matches!(p["tool"].as_str(), Some("undo" | "redo")))
        .filter_map(|p| checked(p, &Confine::default()).ok())
        .map(|(tool, args)| json!({"tool": tool, "args": args}))
        .collect();
    let mut s = Session::default();
    let setup_ok = setup.iter().all(|p| run_call(&mut s, p["tool"].as_str().unwrap_or(""), &p["args"]).ok);
    let now = match checked(c, &Confine::default()) {
        Ok((tool, args)) => run_call(&mut s, &tool, &args),
        Err(why) => Outcome { ok: false, error: Some(format!("not replayable: {why}")), changed: false, read_back: Value::Null, unchanged: Value::Null },
    };
    json!({
        "name": format!("{}-{}-{}", c["tool"].as_str().unwrap_or("call"), trace.name, n),
        "from": {"session": trace.name, "call": n, "source": c["source"]},
        "recorded": {"ok": c["ok"], "error": c["error"]},
        "setup": setup,
        "setup_reproduced": setup_ok,
        "call": {"tool": c["tool"], "args": c["args"]},
        // What this build does: the test keeps it so. Edit `expect` to
        // what it should do once the tool is fixed.
        "expect": {"ok": now.ok, "error_starts": now.error.map(|e| e.chars().take(80).collect::<String>())},
    })
}

fn write_fixture(f: &Value) -> Result<PathBuf, String> {
    let dir = fixtures_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let name: String = f["name"].as_str().unwrap_or("call").chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    let path = dir.join(format!("{name}.json"));
    std::fs::write(&path, serde_json::to_string_pretty(f).unwrap_or_default()).map_err(|e| e.to_string())?;
    Ok(path)
}

/// Runs a fixture; `Err` says how this build differs from what it expects.
pub fn check_fixture(f: &Value) -> Result<(), String> {
    let mut s = Session::default();
    // Fixtures are checked like traces: no paths, only the listed tools.
    let none = Confine::default();
    for p in f["setup"].as_array().into_iter().flatten() {
        let (tool, args) = checked(p, &none)?;
        let o = run_call(&mut s, &tool, &args);
        if !o.ok && f["setup_reproduced"] == json!(true) {
            return Err(format!("setup call {} failed: {}", p["tool"], o.error.unwrap_or_default()));
        }
    }
    let now = match checked(&f["call"], &none) {
        Ok((tool, args)) => run_call(&mut s, &tool, &args),
        Err(why) => Outcome { ok: false, error: Some(format!("not replayable: {why}")), changed: false, read_back: Value::Null, unchanged: Value::Null },
    };
    let want_ok = f["expect"]["ok"].as_bool().unwrap_or(true);
    if now.ok != want_ok {
        return Err(format!("expected it to {}, it {}{}", if want_ok { "work" } else { "fail" }, if now.ok { "worked" } else { "failed: " }, now.error.unwrap_or_default()));
    }
    if let (Some(want), Some(got)) = (f["expect"]["error_starts"].as_str(), now.error.as_deref()) {
        if !got.starts_with(want) {
            return Err(format!("the error changed: expected \"{want}…\", got \"{got}\""));
        }
    }
    Ok(())
}

// ------------------------------------------------------------ command

/// `zornov replay …`
pub fn main(args: &[String]) -> i32 {
    let get = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    let flagged = |i: usize| i > 0 && matches!(args[i - 1].as_str(), "--project" | "-p" | "--call" | "--upto" | "--session");
    let Some(path) = args.iter().enumerate().find(|(i, a)| !a.starts_with('-') && !flagged(*i)).map(|(_, a)| PathBuf::from(a)) else {
        eprintln!("usage: zornov replay <trace.jsonl|dir> [--session NAME] [--project DIR] [--call N | --upto N] [--emit-test]");
        return 2;
    };
    let traces = match load(&path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("zornov replay: {e}");
            return 1;
        }
    };
    // A folder: the session asked for, or the last one.
    let want = get("--session");
    let Some(trace) = traces.into_iter().filter(|t| want.as_ref().is_none_or(|w| &t.name == w)).last() else {
        eprintln!("zornov replay: no recorded session there{}", want.map(|w| format!(" named {w}")).unwrap_or_default());
        return 1;
    };
    // The replay itself is not recorded.
    super::trace::force(Some(false));
    let n = trace.calls.len();
    let num = |k: &str| get(k).and_then(|v| v.parse::<usize>().ok()).map(|v| v.clamp(1, n));
    let (from, to) = match (num("--call"), num("--upto")) {
        (Some(c), _) => (c, c),
        (None, Some(u)) => (1, u),
        _ => (1, n),
    };
    let project = get("--project").map(PathBuf::from);
    let (mut s, copy) = match session_on(project.as_deref()) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("zornov replay: {e}");
            return 1;
        }
    };
    println!("replaying {} calls {from}..{to} of {n} on {}", trace.name, copy.as_ref().map_or("a blank project".into(), |c| format!("a copy at {}", c.display())));
    let confine = Confine { from: project.as_ref().and_then(|p| std::fs::canonicalize(p).ok()), to: copy.as_ref().and_then(|c| std::fs::canonicalize(c).ok()) };
    let steps = replay(&mut s, &trace, from, to, &confine);
    let (mut same, mut differ, mut skipped) = (0, 0, 0);
    for st in &steps {
        match (&st.skipped, st.diff.is_empty()) {
            (Some(why), _) => {
                skipped += 1;
                println!("  {:>4} {:<14} skipped: {why}", st.call, st.tool);
            }
            (None, true) => {
                same += 1;
                println!("  {:>4} {:<14} same", st.call, st.tool);
            }
            (None, false) => {
                differ += 1;
                println!("  {:>4} {:<14} DIFFERS", st.call, st.tool);
                for d in &st.diff {
                    println!("         {d}");
                }
            }
        }
    }
    println!("{same} same, {differ} differ, {skipped} skipped");
    if args.iter().any(|a| a == "--emit-test") {
        for i in from..=to {
            let c = &trace.calls[i - 1];
            if c["ok"] == json!(true) || checked(c, &Confine::default()).is_err() {
                continue;
            }
            match write_fixture(&fixture(&trace, i)) {
                Ok(p) => println!("fixture: {}", p.display()),
                Err(e) => eprintln!("zornov replay: fixture for call {i}: {e}"),
            }
        }
    }
    if let Some(c) = copy {
        let _ = std::fs::remove_dir_all(c);
    }
    if differ > 0 { 1 } else { 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every fixture `zornov replay --emit-test` wrote still behaves as it says.
    #[test]
    fn recorded_agent_calls_still_behave() {
        let Ok(rd) = std::fs::read_dir(fixtures_dir()) else { return };
        let mut bad = Vec::new();
        for e in rd.flatten().filter(|e| e.path().extension().is_some_and(|x| x == "json")) {
            let f: Value = serde_json::from_str(&std::fs::read_to_string(e.path()).unwrap()).unwrap();
            if let Err(why) = check_fixture(&f) {
                bad.push(format!("{}: {why}", e.path().display()));
            }
        }
        assert!(bad.is_empty(), "{}", bad.join("\n"));
    }

    #[test]
    fn a_replayed_call_is_compared_with_the_recording() {
        let rec = json!({"tool": "edit", "args": {"ops": [{"op": "no_such_op"}]}, "ok": true, "rev_before": 0, "rev_after": 1});
        let t = Trace { name: "t".into(), file: PathBuf::new(), calls: vec![rec, json!({"tool": "feedback", "args": {}, "ok": true})], events: Vec::new() };
        let mut s = Session::default();
        let steps = replay(&mut s, &t, 1, 2, &Confine::default());
        assert!(steps[0].diff.iter().any(|d| d.contains("fails now")), "{:?}", steps[0].diff);
        assert!(steps[1].skipped.is_some());
    }

    #[test]
    fn a_trace_cannot_reach_outside_the_copy() {
        let to = std::env::temp_dir().join("zornov-replay-confine");
        let c = Confine { from: Some(PathBuf::from("/home/me/film")), to: Some(to.clone()) };
        let call = |tool: &str, args: Value| json!({"tool": tool, "args": args});
        let (_, a) = checked(&call("edit", json!({"ops": [{"op": "place", "media": "/home/me/film/media/a.mp4"}]})), &c).unwrap();
        assert_eq!(a["ops"][0]["media"], to.join("media/a.mp4").display().to_string(), "moved into the copy");
        for (tool, args) in [
            ("edit", json!({"ops": [{"op": "place", "media": "/etc/passwd"}]})),
            ("edit", json!({"ops": [{"op": "place", "media": "media/../../x"}]})),
            ("edit", json!({"ops": [{"op": "place", "media": "~/secret.mov"}]})),
            ("edit", json!({"ops": [{"op": "place", "media": "https://example.com/a.mp4"}]})),
            ("edit", json!({"ops": [{"op": "narrate", "text": "hi"}]})),
            ("plugins", json!({"action": "create", "source": "fn main() {}"})),
            ("automation", json!({})),
            ("Edit; rm -rf /", json!({})),
            ("edit", json!("not an object")),
        ] {
            assert!(checked(&call(tool, args.clone()), &c).is_err(), "{tool} {args} should be skipped");
        }
        assert!(checked(&call("edit", json!({"ops": [{"op": "text", "text": "1/2 price"}]})), &Confine::default()).is_ok(), "words with a slash are not paths");
    }

    #[test]
    fn a_fixture_keeps_what_this_build_does() {
        let t = Trace { name: "s".into(), file: PathBuf::new(), calls: vec![json!({"tool": "edit", "args": {"ops": [{"op": "no_such_op"}]}, "ok": false, "error": "unknown op", "rev_before": 0, "rev_after": 0, "source": "stdio"})], events: Vec::new() };
        let f = fixture(&t, 1);
        assert_eq!(f["expect"]["ok"], false);
        check_fixture(&f).unwrap();
    }
}
