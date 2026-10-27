// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `zornov trace report <file|dir>`: the friction in recorded sessions,
//! found and ranked without reading a transcript.
//!
//! What it looks for, in each session's calls (see [`super::trace`]):
//! * an error, then the same tool again with different fields: the agent
//!   guessed its way past a vocabulary or docs gap;
//! * fields sent in an edit that the read-back never mentions, and ops the
//!   read-back says left their clip exactly as it was: silent drops;
//! * an edit, then `undo`: the result was not what was wanted;
//! * the same read again with nothing changed in between;
//! * oversized replies and slow calls;
//! * `check` findings the agent left as they were to the end, or said were
//!   intended (`feedback` kind intended): noise;
//! * the agent's own `feedback`.
//!
//! The report is markdown, by tool, worst first, with counts and one
//! example call each. [`metrics`] gives the same signals as rates for
//! `zornov eval`.

use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

/// Replies past this many tokens are oversized.
pub const BIG_REPLY: u64 = 4_000;
/// Calls slower than this are slow.
pub const SLOW_MS: u64 = 5_000;
/// How many calls after an error a retry may come.
const RETRY_WINDOW: usize = 3;
/// How many calls after an edit an undo counts against it.
const UNDO_WINDOW: usize = 3;

/// One recorded session: its file, its calls in order, and the lines that
/// are not calls (hot swaps).
pub struct Trace {
    pub name: String,
    pub file: PathBuf,
    pub calls: Vec<Value>,
    pub events: Vec<Value>,
}

/// A trace is input from anywhere: files past this are not read,
/// lines past this are skipped, and no more calls than this are kept.
const FILE_MAX: u64 = 256 << 20;
const LINE_MAX: usize = 1 << 20;
const CALLS_MAX: usize = 200_000;

/// The traces in `path`: one file, or every `.jsonl` under a folder.
pub fn load(path: &Path) -> Result<Vec<Trace>, String> {
    let mut files = Vec::new();
    if path.is_dir() {
        collect(path, &mut files);
        files.sort();
    } else if path.exists() {
        files.push(path.to_path_buf());
    } else {
        return Err(format!("no trace at {}", path.display()));
    }
    let mut out = Vec::new();
    for f in files {
        let len = std::fs::symlink_metadata(&f).map_err(|e| format!("{}: {e}", f.display()))?.len();
        if len > FILE_MAX {
            eprintln!("{}: {} MB, more than a trace holds; skipped", f.display(), len >> 20);
            continue;
        }
        let text = std::fs::read_to_string(&f).map_err(|e| format!("{}: {e}", f.display()))?;
        let lines: Vec<Value> = text.lines().filter(|l| l.len() <= LINE_MAX).filter_map(|l| serde_json::from_str::<Value>(l).ok()).filter(Value::is_object).take(CALLS_MAX).collect();
        let (mut calls, events): (Vec<Value>, Vec<Value>) = lines.into_iter().partition(|v| v["tool"].is_string() && v["ok"].is_boolean());
        let events: Vec<Value> = events.into_iter().filter(|v| v["event"].is_string()).collect();
        if calls.is_empty() && events.is_empty() {
            continue;
        }
        calls.sort_by_key(|c| (c["at"].as_u64().unwrap_or(0), c["seq"].as_u64().unwrap_or(0)));
        let name = f.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        out.push(Trace { name, file: f, calls, events });
    }
    Ok(out)
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        // Links are not followed (no loops, no reading outside the folder given).
        if e.file_type().is_ok_and(|t| t.is_dir()) {
            collect(&p, out);
        } else if p.extension().is_some_and(|x| x == "jsonl") && !p.file_name().is_some_and(|n| n == "agent-usage.jsonl" || n == "history.jsonl") && !std::fs::symlink_metadata(&p).is_ok_and(|m| m.file_type().is_symlink()) {
            out.push(p);
        }
    }
}

// ------------------------------------------------------------ findings

/// One kind of friction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    RetryAfterError,
    SilentDrop,
    UndoneEdit,
    RepeatedRead,
    Oversized,
    Slow,
    CheckNoise,
    Feedback,
}

impl Kind {
    pub fn title(self) -> &'static str {
        match self {
            Kind::RetryAfterError => "error, then a retry with changed fields",
            Kind::SilentDrop => "fields sent but not in the read-back (silent drop)",
            Kind::UndoneEdit => "edit followed by undo",
            Kind::RepeatedRead => "the same read again, nothing changed",
            Kind::Oversized => "oversized reply",
            Kind::Slow => "slow call",
            Kind::CheckNoise => "check finding left as intended (noise)",
            Kind::Feedback => "agent feedback",
        }
    }

    /// What it usually means, for the person fixing it.
    pub fn meaning(self) -> &'static str {
        match self {
            Kind::RetryAfterError => "a vocabulary or docs gap: the error did not say what to send, or the field names are not the ones agents reach for",
            Kind::SilentDrop => "a field was accepted and did nothing; reject it or make it land",
            Kind::UndoneEdit => "the result was not what the agent meant",
            Kind::RepeatedRead => "the agent could not keep or trust the first answer (resources with change notices would save these)",
            Kind::Oversized => "the reply costs more than it tells",
            Kind::Slow => "the agent waited; a job with progress may serve better",
            Kind::CheckNoise => "check reports something the agent means; tune it or let it be acknowledged",
            Kind::Feedback => "what the agent itself said surprised it",
        }
    }

    /// How much one costs an agent, for the ranking.
    fn weight(self) -> f64 {
        match self {
            Kind::RetryAfterError | Kind::SilentDrop => 3.0,
            Kind::UndoneEdit | Kind::Feedback => 2.0,
            Kind::CheckNoise | Kind::Slow | Kind::Oversized => 1.0,
            Kind::RepeatedRead => 0.5,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Finding {
    pub kind: Kind,
    pub tool: String,
    pub session: String,
    /// The call's number in its session (from 1).
    pub call: usize,
    pub detail: String,
}

/// Every finding in `traces`.
pub fn findings(traces: &[Trace]) -> Vec<Finding> {
    let mut out = Vec::new();
    for t in traces {
        find_in(t, &mut out);
    }
    out
}

fn tool(c: &Value) -> &str {
    c["tool"].as_str().unwrap_or("")
}

fn ok(c: &Value) -> bool {
    c["ok"].as_bool().unwrap_or(false)
}

fn changed_project(c: &Value) -> bool {
    c["rev_before"] != c["rev_after"]
}

fn short(v: &Value, n: usize) -> String {
    let s = v.to_string();
    if s.len() <= n {
        return s;
    }
    let mut end = n;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

fn find_in(t: &Trace, out: &mut Vec<Finding>) {
    let calls = &t.calls;
    let mut push = |kind: Kind, i: usize, tool: &str, detail: String| out.push(Finding { kind, tool: tool.to_owned(), session: t.name.clone(), call: i + 1, detail });
    let mut retried_errors = BTreeSet::new();
    for (i, c) in calls.iter().enumerate() {
        let name = tool(c);
        // An error, then the same tool with other fields.
        if !ok(c) {
            if let Some((j, d)) = calls.iter().enumerate().skip(i + 1).take(RETRY_WINDOW).find(|(_, n)| tool(n) == name).map(|(j, n)| (j, field_diff(&c["args"], &n["args"]))) {
                if !d.is_empty() && retried_errors.insert(i) {
                    let then = if ok(&calls[j]) { "worked" } else { "failed again" };
                    push(Kind::RetryAfterError, i, name, format!("error: {}\nretried {} call(s) later with {} → {then}", short(&c["error"], 240), j - i, d.join(", ")));
                }
            }
        }
        // Silent drops: the read-back says the op did nothing, or never
        // mentions a field that was sent.
        if ok(c) && name == "edit" {
            for u in c["reply"]["read_back"]["unchanged"].as_array().into_iter().flatten() {
                push(Kind::SilentDrop, i, name, format!("read-back: {}", u.as_str().unwrap_or("")));
            }
            for (op, fields) in absent_fields(c) {
                push(Kind::SilentDrop, i, &format!("edit:{op}"), format!("op {op}: sent {} but the read-back does not mention {}", short(&json!(fields.0), 200), fields.1.join(", ")));
            }
            for n in c["reply"]["notes"].as_array().into_iter().flatten().filter_map(Value::as_str) {
                let l = n.to_lowercase();
                if ["ignored", "not used", "unknown field", "has no effect", "no effect"].iter().any(|w| l.contains(w)) {
                    push(Kind::SilentDrop, i, name, format!("note: {n}"));
                }
            }
        }
        // An edit, then undo.
        if ok(c) && changed_project(c) && name != "undo" && name != "redo" {
            if let Some(j) = calls.iter().enumerate().skip(i + 1).take(UNDO_WINDOW).find(|(_, n)| tool(n) == "undo" && ok(n)).map(|(j, _)| j) {
                // Only the edit right before the undo is the one undone.
                if !calls[i + 1..j].iter().any(|m| ok(m) && changed_project(m)) {
                    let what = if c["ops"].is_array() { format!("ops {}", short(&c["ops"], 160)) } else { short(&c["args"], 160) };
                    push(Kind::UndoneEdit, i, name, format!("{what}, undone {} call(s) later", j - i));
                }
            }
        }
        if c["reply_tokens"].as_u64().unwrap_or(0) > BIG_REPLY {
            push(Kind::Oversized, i, name, format!("{} tokens ({} bytes), args {}", c["reply_tokens"], c["reply_bytes"], short(&c["args"], 120)));
        }
        if c["ms"].as_u64().unwrap_or(0) > SLOW_MS {
            push(Kind::Slow, i, name, format!("{:.1} s, args {}", c["ms"].as_u64().unwrap_or(0) as f64 / 1000.0, short(&c["args"], 120)));
        }
        if name == super::trace::FEEDBACK {
            let a = &c["args"];
            let about = a["tool"].as_str().filter(|s| !s.is_empty()).unwrap_or("(no tool named)");
            if a["kind"] == "intended" {
                push(Kind::CheckNoise, i, "check", format!("the agent says this finding is intended: {}", a["finding"].as_str().unwrap_or(a["got"].as_str().unwrap_or(""))));
            } else {
                push(Kind::Feedback, i, about, format!("[{}] expected: {}\ngot: {}", a["kind"].as_str().unwrap_or("surprise"), a["expected"].as_str().unwrap_or(""), a["got"].as_str().unwrap_or("")));
            }
        }
    }
    repeated_reads(t, &mut push);
    check_noise(t, &mut push);
}

/// Fields that differ between two calls' arguments, as `path: old → new`.
pub fn field_diff(a: &Value, b: &Value) -> Vec<String> {
    let (mut fa, mut fb) = (BTreeMap::new(), BTreeMap::new());
    flatten("", a, &mut fa);
    flatten("", b, &mut fb);
    let mut out = Vec::new();
    for (k, v) in &fa {
        match fb.get(k) {
            None => out.push(format!("-{k}")),
            Some(w) if w != v => out.push(format!("{k}: {v} → {w}")),
            _ => {}
        }
    }
    for (k, w) in &fb {
        if !fa.contains_key(k) {
            out.push(format!("+{k}={w}"));
        }
    }
    out
}

fn flatten(prefix: &str, v: &Value, out: &mut BTreeMap<String, String>) {
    match v {
        Value::Object(o) => {
            for (k, x) in o {
                flatten(&if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") }, x, out);
            }
        }
        // Ops are compared by position (the same op tried again).
        Value::Array(a) if a.iter().all(Value::is_object) && !a.is_empty() => {
            for (i, x) in a.iter().enumerate() {
                flatten(&format!("{prefix}[{i}]"), x, out);
            }
        }
        _ => {
            out.insert(prefix.to_owned(), v.to_string());
        }
    }
}

/// Fields an op names that say which clip and when, not what to change.
const PLACE: &[&str] = &["op", "clip", "clips", "id", "ids", "parent", "left", "right", "at", "start", "duration", "dur", "lane", "name", "media", "item", "panel", "ref", "select", "t", "to", "from", "track", "index"];

/// For each op whose clip is read back: the fields it sent that the
/// read-back never mentions (by name or a word of it).
fn absent_fields(c: &Value) -> Vec<(String, (Value, Vec<String>))> {
    let rb = &c["reply"]["read_back"];
    let Some(clips) = rb["clips"].as_array() else { return Vec::new() };
    let mut out = Vec::new();
    for op in c["args"]["ops"].as_array().into_iter().flatten() {
        let Some(name) = op["op"].as_str() else { continue };
        let ids: Vec<u64> = match &op["clip"] {
            Value::Number(n) => n.as_u64().into_iter().collect(),
            Value::Array(a) => a.iter().filter_map(Value::as_u64).collect(),
            _ => Vec::new(),
        };
        let lines: Vec<String> = clips.iter().filter(|l| l["clip"].as_u64().is_some_and(|id| ids.contains(&id))).map(|l| l.to_string().to_lowercase()).collect();
        if lines.is_empty() {
            continue;
        }
        let text = lines.join(" ");
        let missing: Vec<String> = op
            .as_object()
            .into_iter()
            .flatten()
            .filter(|(k, _)| !PLACE.contains(&k.as_str()))
            .filter(|(k, _)| {
                let k = k.to_lowercase();
                // `motion_in` is read back as `motion`, `font_size` as `size`.
                !text.contains(&k) && !k.split('_').filter(|w| w.len() > 2).any(|w| text.contains(w))
            })
            .map(|(k, _)| k.clone())
            .collect();
        if !missing.is_empty() {
            out.push((name.to_owned(), (op.clone(), missing)));
        }
    }
    out
}

/// Tools that only read.
fn is_read(c: &Value) -> bool {
    matches!(tool(c), "summary" | "inspect" | "media" | "catalogue" | "guide" | "edit_schema" | "frame" | "contact_sheet" | "capabilities" | "howto" | "see" | "objects" | "listen" | "scopes" | "editorial" | "made" | "fonts" | "voices" | "job" | "export_status")
}

fn repeated_reads(t: &Trace, push: &mut dyn FnMut(Kind, usize, &str, String)) {
    // (tool, args) → the call that last read it, at which revision.
    let mut seen: HashMap<(String, String), (usize, Value)> = HashMap::new();
    for (i, c) in t.calls.iter().enumerate() {
        if changed_project(c) {
            seen.clear();
            continue;
        }
        if !ok(c) || !is_read(c) || tool(c) == "job" || tool(c) == "export_status" {
            continue;
        }
        let key = (tool(c).to_owned(), c["args"].to_string());
        if let Some((first, rev)) = seen.get(&key) {
            if *rev == c["rev_before"] {
                push(Kind::RepeatedRead, i, tool(c), format!("same as call {} ({}), revision {} both times", first + 1, short(&c["args"], 100), rev));
                continue;
            }
        }
        seen.insert(key, (i, c["rev_before"].clone()));
    }
}

/// `check` findings still there in the session's last check, after the
/// agent had seen them in an earlier one: it chose to leave them.
fn check_noise(t: &Trace, push: &mut dyn FnMut(Kind, usize, &str, String)) {
    let checks: Vec<(usize, &Value)> = t.calls.iter().enumerate().filter(|(_, c)| tool(c) == "check" && ok(c)).collect();
    if checks.len() < 2 {
        return;
    }
    let key = |p: &Value| format!("{} {}", p["kind"].as_str().unwrap_or("?"), p["clip"]);
    let first: BTreeSet<String> = checks[0].1["reply"]["problems"].as_array().into_iter().flatten().map(key).collect();
    let (li, last) = checks[checks.len() - 1];
    for p in last["reply"]["problems"].as_array().into_iter().flatten() {
        if first.contains(&key(p)) {
            let what = p["message"].as_str().or(p["what"].as_str()).unwrap_or("");
            push(Kind::CheckNoise, li, &format!("check:{}", p["kind"].as_str().unwrap_or("?")), format!("left as is through {} checks: clip {} {what}", checks.len(), p["clip"]));
        }
    }
}

// ------------------------------------------------------------ report

/// The markdown report.
pub fn report(traces: &[Trace]) -> String {
    let fs = findings(traces);
    let calls: usize = traces.iter().map(|t| t.calls.len()).sum();
    let errors: usize = traces.iter().flat_map(|t| &t.calls).filter(|c| !ok(c)).count();
    let tokens: u64 = traces.iter().flat_map(|t| &t.calls).filter_map(|c| c["reply_tokens"].as_u64()).sum();
    let mut sources: BTreeMap<String, usize> = BTreeMap::new();
    for c in traces.iter().flat_map(|t| &t.calls) {
        *sources.entry(c["source"].as_str().unwrap_or("?").to_owned()).or_default() += 1;
    }
    let mut s = String::from("# Agent friction report\n\n");
    s.push_str(&format!(
        "{calls} calls in {} session(s) ({}), {errors} errors, {tokens} reply tokens.\n\n",
        traces.len(),
        sources.iter().map(|(k, v)| format!("{k} {v}")).collect::<Vec<_>>().join(", ")
    ));
    for t in traces.iter().take(10) {
        s.push_str(&format!("- `{}`: {} calls ({})\n", t.name, t.calls.len(), t.file.display()));
    }
    if traces.len() > 10 {
        s.push_str(&format!("- and {} more sessions\n", traces.len() - 10));
    }
    s.push('\n');
    if fs.is_empty() {
        s.push_str("No friction found.\n");
        return s;
    }
    // By tool, worst first.
    let mut by_tool: BTreeMap<&str, Vec<&Finding>> = BTreeMap::new();
    for f in &fs {
        by_tool.entry(f.tool.as_str()).or_default().push(f);
    }
    let score = |v: &[&Finding]| v.iter().map(|f| f.kind.weight()).sum::<f64>();
    let mut ranked: Vec<(&str, Vec<&Finding>)> = by_tool.into_iter().collect();
    ranked.sort_by(|a, b| score(b.1.as_slice()).total_cmp(&score(a.1.as_slice())).then(a.0.cmp(b.0)));
    s.push_str("| # | tool | score | findings |\n|---|---|---|---|\n");
    for (i, (t, v)) in ranked.iter().enumerate() {
        let mut kinds: BTreeMap<Kind, usize> = BTreeMap::new();
        for f in v {
            *kinds.entry(f.kind).or_default() += 1;
        }
        s.push_str(&format!("| {} | `{t}` | {:.1} | {} |\n", i + 1, score(v.as_slice()), kinds.iter().map(|(k, n)| format!("{} ×{n}", k.title())).collect::<Vec<_>>().join("; ")));
    }
    for (t, v) in &ranked {
        s.push_str(&format!("\n## `{t}`\n"));
        let mut kinds: BTreeMap<Kind, Vec<&&Finding>> = BTreeMap::new();
        for f in v {
            kinds.entry(f.kind).or_default().push(f);
        }
        for (k, list) in kinds {
            let ex = list[0];
            s.push_str(&format!("\n**{}** ×{} ({})\n\nExample, session `{}` call {}:\n\n```\n{}\n```\n", k.title(), list.len(), k.meaning(), ex.session, ex.call, ex.detail));
        }
    }
    s.push_str("\nReplay a call against this build: `zornov replay <trace> --call N`.\n");
    s
}

// ------------------------------------------------------------ metrics

/// Friction as numbers, for `zornov eval`: calls, calls to goal (to the
/// last call that changed the project), and the rates of retries, silent
/// drops and undone edits, with reply tokens and wall time.
pub fn metrics(t: &Trace) -> Value {
    let calls = &t.calls;
    let n = calls.len();
    let fs = {
        let mut v = Vec::new();
        find_in(t, &mut v);
        v
    };
    let count = |k: Kind| fs.iter().filter(|f| f.kind == k).map(|f| (f.session.clone(), f.call)).collect::<BTreeSet<_>>().len();
    let edits = calls.iter().filter(|c| tool(c) == "edit").count();
    let changing = calls.iter().filter(|c| ok(c) && changed_project(c) && !matches!(tool(c), "undo" | "redo")).count();
    let to_goal = calls.iter().rposition(|c| ok(c) && changed_project(c)).map_or(0, |i| i + 1);
    let rate = |a: usize, b: usize| if b == 0 { 0.0 } else { (a as f64 / b as f64 * 1000.0).round() / 1000.0 };
    let wall = match (calls.first(), calls.last()) {
        (Some(a), Some(b)) => (b["at"].as_u64().unwrap_or(0).saturating_sub(a["at"].as_u64().unwrap_or(0)) + b["ms"].as_u64().unwrap_or(0)) as f64 / 1000.0,
        _ => 0.0,
    };
    json!({
        "calls": n,
        "calls_to_goal": to_goal,
        "errors": calls.iter().filter(|c| !ok(c)).count(),
        "retry_rate": rate(count(Kind::RetryAfterError), n),
        "silent_drop_rate": rate(count(Kind::SilentDrop), edits),
        "undo_rate": rate(count(Kind::UndoneEdit), changing),
        "repeated_reads": count(Kind::RepeatedRead),
        "feedback": count(Kind::Feedback),
        "reply_tokens": calls.iter().filter_map(|c| c["reply_tokens"].as_u64()).sum::<u64>(),
        "wall_seconds": (wall * 10.0).round() / 10.0,
        "speed": speed(t),
    })
}

/// How tight the loop is: what recording cost each call (`record_us`), how
/// long a call waited to reach the writer and so the live tap (`queue_us`),
/// and how long a hot-swapped fix took from build to retried call.
pub fn speed(t: &Trace) -> Value {
    let stats = |mut v: Vec<f64>| -> Value {
        if v.is_empty() {
            return Value::Null;
        }
        v.sort_by(f64::total_cmp);
        let mean = v.iter().sum::<f64>() / v.len() as f64;
        let p99 = v[((v.len() as f64 * 0.99).ceil() as usize).clamp(1, v.len()) - 1];
        json!({"mean": (mean * 10.0).round() / 10.0, "p99": p99, "max": v[v.len() - 1]})
    };
    let field = |k: &str| t.calls.iter().filter_map(|c| c[k].as_f64()).collect::<Vec<_>>();
    let fixes: Vec<f64> = t.events.iter().filter(|e| e["event"] == "swap").filter_map(|e| e["fix_to_retry_ms"].as_f64()).map(|ms| ms / 1000.0).collect();
    json!({"record_us": stats(field("record_us")), "stream_queue_us": stats(field("queue_us")), "fix_to_retry_s": stats(fixes)})
}

// ------------------------------------------------------------ command

/// `zornov trace report <file|dir> [--out FILE]`, `zornov trace on|off|status`.
pub fn main(args: &[String]) -> i32 {
    let usage = || {
        eprintln!("usage: zornov trace report <file|dir> [--out FILE] [--json]\n       zornov trace on|off|status");
        2
    };
    match args.first().map(String::as_str) {
        Some("on") | Some("off") => match super::trace::set_setting(args[0] == "on") {
            Ok(p) => {
                println!("agent trace {} ({})", args[0], p.display());
                0
            }
            Err(e) => {
                eprintln!("zornov trace: {e}");
                1
            }
        },
        Some("status") => {
            println!("agent trace {} (traces go to <project>/.zornov/agent-trace/)", if super::trace::enabled() { "on" } else { "off" });
            0
        }
        Some("report") => {
            let Some(path) = args.get(1).filter(|a| !a.starts_with('-')) else { return usage() };
            let traces = match load(Path::new(path)) {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("zornov trace: {e}");
                    return 1;
                }
            };
            let text = if args.iter().any(|a| a == "--json") {
                serde_json::to_string_pretty(&json!({
                    "sessions": traces.iter().map(|t| json!({"name": t.name, "metrics": metrics(t)})).collect::<Vec<_>>(),
                    "findings": findings(&traces).iter().map(|f| json!({"kind": format!("{:?}", f.kind), "tool": f.tool, "session": f.session, "call": f.call, "detail": f.detail})).collect::<Vec<_>>(),
                }))
                .unwrap_or_default()
            } else {
                report(&traces)
            };
            match args.iter().position(|a| a == "--out").and_then(|i| args.get(i + 1)) {
                Some(out) => match std::fs::write(out, &text) {
                    Ok(()) => {
                        println!("{out}");
                        0
                    }
                    Err(e) => {
                        eprintln!("zornov trace: {e}");
                        1
                    }
                },
                None => {
                    print!("{text}");
                    0
                }
            }
        }
        _ => usage(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(seq: u64, tool: &str, args: Value, ok: bool, rev: (u64, u64), extra: Value) -> Value {
        let mut c = json!({"seq": seq, "at": seq * 1000, "source": "stdio", "tool": tool, "args": args, "ok": ok, "ms": 10, "reply_tokens": 50, "reply_bytes": 200, "rev_before": rev.0, "rev_after": rev.1});
        for (k, v) in extra.as_object().into_iter().flatten() {
            c[k.as_str()] = v.clone();
        }
        c
    }

    fn trace(calls: Vec<Value>) -> Trace {
        Trace { name: "t".into(), file: PathBuf::new(), calls, events: Vec::new() }
    }

    #[test]
    fn speed_is_measured_from_the_lines() {
        let mut t = trace(vec![call(1, "summary", json!({}), true, (0, 0), json!({"record_us": 4, "queue_us": 30})), call(2, "summary", json!({}), true, (0, 0), json!({"record_us": 8, "queue_us": 50}))]);
        t.events.push(json!({"event": "swap", "fix_to_retry_ms": 4200.0}));
        let s = speed(&t);
        assert_eq!(s["record_us"]["mean"], 6.0);
        assert_eq!(s["stream_queue_us"]["max"], 50.0);
        assert_eq!(s["fix_to_retry_s"]["mean"], 4.2);
    }

    #[test]
    fn each_kind_of_friction_is_found() {
        let t = trace(vec![
            call(1, "edit", json!({"ops": [{"op": "text", "txt": "Hi"}]}), false, (0, 0), json!({"error": "op 1 (text): unknown field txt"})),
            call(2, "edit", json!({"ops": [{"op": "text", "text": "Hi"}]}), true, (0, 1), json!({})),
            call(3, "edit", json!({"ops": [{"op": "set", "clip": 1, "glow": 3, "opacity": 0.5}]}), true, (1, 2), json!({"reply": {"read_back": {"clips": [{"clip": 1, "name": "Hi", "opacity": 0.5}], "unchanged": []}}})),
            call(4, "undo", json!({}), true, (2, 3), json!({})),
            call(5, "summary", json!({}), true, (3, 3), json!({})),
            call(6, "summary", json!({}), true, (3, 3), json!({"reply_tokens": 9000, "ms": 9000})),
            call(7, "feedback", json!({"expected": "glow to show", "got": "nothing", "tool": "edit"}), true, (3, 3), json!({})),
        ]);
        let fs = findings(&[t]);
        let has = |k: Kind, tool: &str| fs.iter().any(|f| f.kind == k && f.tool == tool);
        assert!(has(Kind::RetryAfterError, "edit"), "{fs:#?}");
        assert!(has(Kind::SilentDrop, "edit:set"), "glow is not read back: {fs:#?}");
        assert!(!fs.iter().any(|f| f.kind == Kind::SilentDrop && f.detail.contains("opacity") && !f.detail.contains("glow")));
        assert!(has(Kind::UndoneEdit, "edit"));
        assert!(has(Kind::RepeatedRead, "summary"));
        assert!(has(Kind::Oversized, "summary") && has(Kind::Slow, "summary"));
        assert!(has(Kind::Feedback, "edit"));
    }

    #[test]
    fn a_check_finding_left_to_the_end_is_noise() {
        let p = json!({"reply": {"problems": [{"kind": "gap", "clip": 4, "message": "a gap at 3 s"}]}});
        let t = trace(vec![call(1, "check", json!({}), true, (1, 1), p.clone()), call(2, "edit", json!({}), true, (1, 2), json!({})), call(3, "check", json!({}), true, (2, 2), p)]);
        assert!(findings(&[t]).iter().any(|f| f.kind == Kind::CheckNoise && f.tool == "check:gap"));
    }

    #[test]
    fn metrics_are_rates() {
        let t = trace(vec![
            call(1, "edit", json!({"ops": [{"op": "x", "a": 1}]}), false, (0, 0), json!({})),
            call(2, "edit", json!({"ops": [{"op": "x", "b": 1}]}), true, (0, 1), json!({})),
            call(3, "summary", json!({}), true, (1, 1), json!({})),
        ]);
        let m = metrics(&t);
        assert_eq!(m["calls"], 3);
        assert_eq!(m["calls_to_goal"], 2);
        assert_eq!(m["retry_rate"], 0.333);
        assert_eq!(m["reply_tokens"], 150);
    }

    #[test]
    fn the_report_ranks_by_tool_with_an_example() {
        let t = trace(vec![call(1, "frame", json!({"t": 1}), true, (0, 0), json!({"reply_tokens": 20000}))]);
        let r = report(&[t]);
        assert!(r.contains("| 1 | `frame`"), "{r}");
        assert!(r.contains("Example, session `t` call 1"));
    }
}
