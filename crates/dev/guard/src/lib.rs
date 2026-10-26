// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Checks for text that comes from outside the project (issues, comments,
//! pull requests, email, and anything an automated agent is about to post)
//! and for pull requests that change what runs or what instructs automation.
//!
//! * [`scan`] finds hidden text, text written to steer an AI, smuggled data
//!   and canary values. A [`Severity::High`] finding means a person must look
//!   before any automation reads the text.
//! * [`pr_risk`] turns a pull request's changed files and added lines into
//!   review labels.
//!
//! These are alarms, not walls: they catch the common tricks so a person
//! looks. What automation is allowed to do is limited elsewhere.

/// How much a finding matters.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// Worth a note in the run summary.
    Medium,
    /// A person must look before automation acts on the text.
    High,
    /// A canary value showed up: something leaked. Stop automation.
    Critical,
}

/// One thing [`scan`] found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    pub severity: Severity,
    /// Short rule name, e.g. `hidden-unicode`.
    pub rule: &'static str,
    /// 1-based line number.
    pub line: usize,
    pub detail: String,
}

/// What the text is, which changes how strict links and images are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Text coming in: issues, comments, pull request descriptions, email.
    Incoming,
    /// Text automation is about to post. Links and images to hosts outside
    /// [`ALLOWED_HOSTS`] are high, since showing them can send data out.
    Outgoing,
}

/// Hosts agent output may link to or show images from.
pub const ALLOWED_HOSTS: &[&str] = &["github.com", "githubusercontent.com", "crates.io", "docs.rs", "rust-lang.org"];

/// Phrases that try to override or impersonate an AI's instructions.
const OVERRIDE: &[&str] = &[
    "ignore previous instructions",
    "ignore all previous",
    "ignore the previous",
    "ignore the above",
    "ignore your instructions",
    "ignore all instructions",
    "disregard previous",
    "disregard all previous",
    "disregard all prior",
    "disregard the above",
    "disregard your instructions",
    "forget your instructions",
    "forget all previous",
    "override your instructions",
    "new instructions:",
    "updated instructions:",
    "your new task is",
    "you are now in ",
    "you are now a ",
    "you are now an ",
    "do not tell the user",
    "don't tell the user",
    "without telling the user",
    "without informing the user",
    "do not mention this",
    "<|im_start|>",
    "<|im_end|>",
    "<|system|>",
    "[inst]",
    "[/inst]",
    "<system>",
    "</system>",
    "begin system prompt",
    "end of system prompt",
];

/// Phrases that address an AI reader. Fine in a discussion, worth a note.
const ADDRESSED: &[&str] = &[
    "dear ai",
    "to the ai",
    "attention ai",
    "note to ai",
    "ai assistant:",
    "ai agent:",
    "assistant:",
    "system prompt",
    "as an ai",
    "as a language model",
    "llm:",
    "if you are an ai",
    "if you're an ai",
    "if you are a language model",
];

/// Requests to do something with secrets or send data somewhere.
const EXFIL: &[&str] = &[
    "exfiltrate",
    "send the token",
    "send your token",
    "print the token",
    "print your token",
    "github_token",
    "secrets.",
    "env | ",
    "printenv",
    "cat ~/.ssh",
    ".aws/credentials",
    "curl -d",
    "curl --data",
    "| sh",
    "| bash",
];

/// Characters that are invisible or reorder text.
fn hidden_kind(c: char) -> Option<(&'static str, Severity)> {
    let u = c as u32;
    match u {
        0xE0000..=0xE007F => Some(("tag characters (invisible text)", Severity::High)),
        0x202A..=0x202E | 0x2066..=0x2069 => Some(("text-direction controls", Severity::High)),
        0xE0100..=0xE01EF => Some(("variation selectors (can hide data)", Severity::High)),
        0x200B | 0x200C | 0x200E | 0x200F | 0x2060..=0x2064 | 0x180E => Some(("zero-width characters", Severity::Medium)),
        0xFEFF => Some(("zero-width no-break space", Severity::Medium)),
        0x00AD => Some(("soft hyphens", Severity::Medium)),
        _ => None,
    }
}

/// Every finding in `text`. `canaries` are decoy values (fake secrets, marker
/// strings) that must never appear anywhere; empty ones are ignored.
pub fn scan(text: &str, mode: Mode, canaries: &[String]) -> Vec<Finding> {
    let mut out = Vec::new();
    let mut add = |severity, rule, line, detail: String| out.push(Finding { severity, rule, line, detail });

    for (i, raw) in text.lines().enumerate() {
        let n = i + 1;
        // Hidden characters: count each kind once per line.
        let mut seen: Vec<&str> = Vec::new();
        for c in raw.chars() {
            if let Some((kind, sev)) = hidden_kind(c)
                && !seen.contains(&kind)
            {
                seen.push(kind);
                let count = raw.chars().filter(|&d| hidden_kind(d).map(|h| h.0) == Some(kind)).count();
                add(sev, "hidden-unicode", n, format!("{count} {kind}"));
            }
        }
        // Read the line as a person would see it: hidden characters removed,
        // so "ig\u{200b}nore previous" still counts.
        let visible: String = raw.chars().filter(|&c| hidden_kind(c).is_none()).collect();
        let lower = visible.to_lowercase();
        let squashed = squash(&lower);
        let compact: String = lower.chars().filter(|c| !c.is_whitespace()).collect();

        for p in OVERRIDE {
            // Without spaces only for long phrases, so "you are now able"
            // does not read as "you are now a".
            let sq = squash(p);
            if lower.contains(p) || (sq.len() >= 16 && squashed.contains(&sq)) {
                add(Severity::High, "instruction-override", n, format!("\"{p}\""));
                break;
            }
        }
        for p in ADDRESSED {
            if lower.contains(p) {
                add(Severity::Medium, "addresses-ai", n, format!("\"{p}\""));
                break;
            }
        }
        for p in EXFIL {
            if lower.contains(p) {
                add(Severity::Medium, "secrets-or-sending", n, format!("\"{}\"", p.trim()));
                break;
            }
        }
        for canary in canaries.iter().filter(|c| c.len() >= 6) {
            if visible.contains(canary.as_str()) || raw.contains(canary.as_str()) {
                add(Severity::Critical, "canary", n, "a canary value appeared: something leaked".into());
            }
        }
        if let Some(len) = longest_blob(&visible) {
            add(Severity::Medium, "encoded-blob", n, format!("{len}-character encoded run"));
        }
        if ["color:white", "color:#fff", "color:#ffffff", "font-size:0", "display:none", "visibility:hidden", "opacity:0"]
            .iter()
            .any(|s| has_token(&compact, s))
        {
            add(Severity::High, "hidden-styling", n, "styling that hides text".into());
        }
        for url in urls(&visible) {
            let host = host_of(url);
            let allowed = ALLOWED_HOSTS.iter().any(|a| host == *a || host.ends_with(&format!(".{a}")));
            let query = url.split_once('?').map_or(0, |(_, q)| q.len());
            if url.starts_with("data:") {
                add(Severity::High, "data-url", n, "data: URL".into());
            } else if mode == Mode::Outgoing && !allowed {
                add(Severity::High, "outside-link", n, format!("link or image to {host}"));
            } else if query > 100 {
                add(Severity::Medium, "long-query", n, format!("{query}-character query string to {host}"));
            }
        }
    }

    // HTML comments are invisible once rendered: anything that steers an AI
    // inside one is high, whatever the line rules found.
    let mut rest = text;
    while let Some(start) = rest.find("<!--") {
        let after = &rest[start + 4..];
        let end = after.find("-->").unwrap_or(after.len());
        let body = after[..end].to_lowercase();
        let line = text.len() - rest.len() + start;
        let n = text[..line].matches('\n').count() + 1;
        if OVERRIDE.iter().chain(ADDRESSED).chain(EXFIL).any(|p| body.contains(p)) {
            out.push(Finding { severity: Severity::High, rule: "hidden-comment", line: n, detail: "an HTML comment speaks to an AI".into() });
        }
        rest = &after[end.min(after.len())..];
    }
    out.sort_by_key(|f| (std::cmp::Reverse(f.severity), f.line));
    out
}

/// `text` without invisible and text-reordering characters, and how many
/// were removed. Emoji joiners and variation selector 16 stay, so emoji
/// still show.
pub fn strip_hidden(text: &str) -> (String, usize) {
    let mut removed = 0;
    let out = text
        .chars()
        .filter(|&c| {
            let hidden = hidden_kind(c).is_some();
            removed += hidden as usize;
            !hidden
        })
        .collect();
    (out, removed)
}

/// Whether a finding means the text tries to steer an AI (as opposed to a
/// note such as a long blob or a mention of "system prompt").
pub fn is_steering(f: &Finding) -> bool {
    f.severity >= Severity::High && matches!(f.rule, "instruction-override" | "hidden-comment" | "hidden-styling" | "hidden-unicode" | "canary")
}

/// The worst severity in `findings`, if any.
pub fn worst(findings: &[Finding]) -> Option<Severity> {
    findings.iter().map(|f| f.severity).max()
}

/// Whether `needle` is in `hay` and not just the start of a longer value
/// ("font-size:0" but not "font-size:0.9em").
fn has_token(hay: &str, needle: &str) -> bool {
    hay.match_indices(needle).any(|(i, _)| {
        !hay[i + needle.len()..].starts_with(|c: char| c.is_ascii_alphanumeric() || c == '.')
    })
}

/// Letters and digits only, so spacing and punctuation tricks still match.
fn squash(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric() || matches!(c, '<' | '|' | '[' | ']' | '/' | ':' | '#')).collect()
}

/// Length of the longest base64- or hex-looking run, if it is long enough to
/// carry data (well past a SHA-256 in hex).
fn longest_blob(s: &str) -> Option<usize> {
    // A run counts if it mixes upper case, lower case and digits (base64) or
    // is all hex digits; long paths and words do not.
    s.split(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=' | '_')))
        .filter(|run| {
            let hex = run.len() >= 128 && run.chars().all(|c| c.is_ascii_hexdigit());
            let b64 = run.len() >= 120
                && run.chars().any(|c| c.is_ascii_uppercase())
                && run.chars().any(|c| c.is_ascii_lowercase())
                && run.chars().any(|c| c.is_ascii_digit());
            hex || b64
        })
        .map(str::len)
        .max()
}

fn urls(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    for scheme in ["https://", "http://", "data:"] {
        let mut from = 0;
        while let Some(i) = s[from..].find(scheme) {
            let start = from + i;
            // "data:" only as a URL of its own ("data:image/png;base64,…"),
            // not inside "metadata:".
            let boundary = !s[..start].ends_with(|c: char| c.is_alphanumeric());
            let mime = s[start + scheme.len()..].split([',', ';']).next().unwrap_or("");
            if scheme == "data:" && !(boundary && mime.contains('/') && !mime.contains(' ') && mime.len() < 60) {
                from = start + scheme.len();
                continue;
            }
            let end = s[start..].find(|c: char| c.is_whitespace() || matches!(c, ')' | '>' | '"' | '\'' | ']')).map_or(s.len(), |e| start + e);
            out.push(&s[start..end]);
            from = end.max(start + scheme.len());
        }
    }
    out
}

fn host_of(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = host.rsplit_once('@').map_or(host, |(_, h)| h);
    host.split(':').next().unwrap_or("").to_lowercase()
}

/// One changed file in a pull request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub path: String,
    /// Git could not show it as text.
    pub binary: bool,
    /// Lines the pull request adds to it.
    pub added: Vec<String>,
}

/// Files that instruct AI tools; a change to one can steer every agent.
fn is_agent_file(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    matches!(name, "AGENTS.md" | "CLAUDE.md" | "CLAUDE.local.md" | "GEMINI.md" | ".cursorrules" | ".windsurfrules" | "copilot-instructions.md")
        || [".claude/", ".cursor/", ".codex/", ".gemini/", ".continue/", ".github/prompts/", ".github/instructions/"]
            .iter()
            .any(|d| path.starts_with(d) || path.contains(&format!("/{d}")))
}

/// Review labels for a pull request, each with the reason.
pub fn pr_risk(changes: &[Change]) -> Vec<(&'static str, String)> {
    let mut out: Vec<(&'static str, String)> = Vec::new();
    let mut add = |label: &'static str, why: String| {
        if !out.iter().any(|(l, w)| *l == label && *w == why) {
            out.push((label, why));
        }
    };
    for c in changes {
        let p = c.path.as_str();
        if is_agent_file(p) {
            add("risk: agent-files", format!("{p} instructs AI tools"));
        }
        if p.starts_with(".github/") || p.starts_with("crates/dev/guard/") || p == "deny.toml" {
            add("risk: ci", format!("{p} changes checks or automation"));
        }
        if p.ends_with("/build.rs") || p == "build.rs" {
            add("risk: build-script", format!("{p} runs code while building"));
        }
        if p.ends_with("Cargo.toml") {
            for l in &c.added {
                let t = l.replace(' ', "");
                if t.starts_with("proc-macro=true") || t.contains("proc_macro=true") {
                    add("risk: build-script", format!("{p} adds a procedural macro (runs while building)"));
                }
                if t.contains("git=\"") {
                    add("risk: dependencies", format!("{p} adds a git dependency"));
                }
                if t.contains("build=\"") {
                    add("risk: build-script", format!("{p} names a build script"));
                }
            }
        }
        if p == "Cargo.lock" {
            add("risk: dependencies", "Cargo.lock changes".into());
        }
        if c.binary {
            add("risk: binary", format!("{p} is binary"));
        }
        // The guard's own tests hold examples of these on purpose.
        if !p.starts_with("crates/dev/guard/") {
            let text = c.added.join("\n");
            for f in scan(&text, Mode::Incoming, &[]) {
                if f.rule == "hidden-unicode" && f.severity == Severity::High {
                    add("risk: hidden-text", format!("{p}:{} {}", f.line, f.detail));
                }
                if matches!(f.rule, "instruction-override" | "hidden-comment") {
                    add("risk: hidden-text", format!("{p}: text that speaks to an AI ({})", f.detail));
                }
            }
        }
    }
    out
}

/// Parses `git diff --numstat` and `git diff -U0` output into [`Change`]s.
pub fn parse_diff(numstat: &str, patch: &str) -> Vec<Change> {
    let mut changes: Vec<Change> = numstat
        .lines()
        .filter_map(|l| {
            let mut it = l.splitn(3, '\t');
            let (a, d, path) = (it.next()?, it.next()?, it.next()?);
            let path = path.rsplit(" => ").next().unwrap_or(path).trim_end_matches('}').to_string();
            Some(Change { path, binary: a == "-" && d == "-", added: Vec::new() })
        })
        .collect();
    let mut current: Option<usize> = None;
    for l in patch.lines() {
        if let Some(p) = l.strip_prefix("+++ ") {
            let p = p.strip_prefix("b/").unwrap_or(p);
            current = changes.iter().position(|c| c.path == p);
        } else if let Some(added) = l.strip_prefix('+')
            && let Some(i) = current
        {
            changes[i].added.push(added.to_string());
        } else if l.starts_with("diff --git") {
            current = None;
        }
    }
    changes
}
