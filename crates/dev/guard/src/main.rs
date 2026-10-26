// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! ```text
//! studio-guard scan [--outgoing] [FILE...]   text from FILEs (or stdin)
//! studio-guard pr-risk BASE HEAD             review labels for BASE...HEAD
//! ```
//!
//! `scan` prints one finding per line and exits 0 (nothing, or notes only),
//! 1 (a person must look) or 2 (a canary appeared: stop automation).
//! `--outgoing` is for text automation is about to post. Canary values come
//! from `STUDIO_GUARD_CANARIES` (comma or newline separated). With
//! `GITHUB_STEP_SUMMARY` set, findings are also added to the run summary.
//!
//! `pr-risk` prints `label<TAB>reason` lines; it only reads the diff and never
//! runs anything from the pull request.

use std::io::{Read, Write};
use std::process::{Command, ExitCode};

use studio_guard::{Mode, Severity, parse_diff, pr_risk, scan, worst};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("scan") => scan_cmd(&args[1..]),
        Some("pr-risk") if args.len() == 3 => pr_risk_cmd(&args[1], &args[2]),
        _ => Err("usage: studio-guard scan [--outgoing] [FILE...] | pr-risk BASE HEAD".into()),
    };
    match result {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            eprintln!("studio-guard: {e}");
            ExitCode::from(3)
        }
    }
}

fn scan_cmd(args: &[String]) -> Result<u8, String> {
    let mode = if args.iter().any(|a| a == "--outgoing") { Mode::Outgoing } else { Mode::Incoming };
    let files: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    let mut text = String::new();
    if files.is_empty() {
        std::io::stdin().read_to_string(&mut text).map_err(|e| format!("stdin: {e}"))?;
    } else {
        for f in files {
            let bytes = std::fs::read(f).map_err(|e| format!("{f}: {e}"))?;
            text.push_str(&String::from_utf8_lossy(&bytes));
            text.push('\n');
        }
    }
    let canaries: Vec<String> = std::env::var("STUDIO_GUARD_CANARIES")
        .unwrap_or_default()
        .split([',', '\n'])
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    let findings = scan(&text, mode, &canaries);
    let name = |s: Severity| match s {
        Severity::Medium => "note",
        Severity::High => "LOOK",
        Severity::Critical => "CANARY",
    };
    for f in &findings {
        println!("{} {} line {}: {}", name(f.severity), f.rule, f.line, f.detail);
    }
    let code = match worst(&findings) {
        Some(Severity::Critical) => 2,
        Some(Severity::High) => 1,
        _ => 0,
    };
    if let Ok(path) = std::env::var("GITHUB_STEP_SUMMARY")
        && !path.is_empty()
    {
        let mut md = String::from("### Text check\n\n");
        if findings.is_empty() {
            md.push_str("Nothing found.\n");
        }
        for f in &findings {
            md.push_str(&format!("- **{}** `{}` line {}: {}\n", name(f.severity), f.rule, f.line, f.detail));
        }
        if let Ok(mut file) = std::fs::OpenOptions::new().append(true).create(true).open(path) {
            let _ = file.write_all(md.as_bytes());
        }
    }
    Ok(code)
}

fn git(args: &[&str]) -> Result<String, String> {
    let out = Command::new("git").args(args).output().map_err(|e| format!("git: {e}"))?;
    if !out.status.success() {
        return Err(format!("git {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn pr_risk_cmd(base: &str, head: &str) -> Result<u8, String> {
    let range = format!("{base}...{head}");
    let numstat = git(&["diff", "--no-renames", "--numstat", &range])?;
    let patch = git(&["diff", "--no-renames", "--no-color", "-U0", &range])?;
    for (label, why) in pr_risk(&parse_diff(&numstat, &patch)) {
        println!("{label}\t{why}");
    }
    Ok(0)
}
