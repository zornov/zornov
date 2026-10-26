// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Crashes a child process on purpose and reads what it left behind.

use std::process::Command;

fn child(kind: &str) -> Option<String> {
    std::env::var("STUDIO_CRASH_CHILD").ok().filter(|v| v == kind)
}

#[test]
fn child_panic() {
    if child("panic").is_some() {
        studio_crash::install("9.9.9-test");
        panic!("boom from the test at /secret");
    }
}

#[test]
fn child_segv() {
    if let Some(k) = child("segv").or_else(|| child("abort")) {
        studio_crash::install("9.9.9-test");
        studio_crash::crash_for_test(&k);
    }
}

fn run(kind: &str, test: &str) -> (std::process::ExitStatus, Vec<(String, String)>) {
    let home = std::env::temp_dir().join(format!("studio-crash-home-{kind}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    let st = Command::new(std::env::current_exe().unwrap())
        .args([test, "--exact", "--nocapture", "--test-threads=1"])
        .env("STUDIO_CRASH_CHILD", kind)
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .env("LOCALAPPDATA", home.join("AppData"))
        .env("XDG_STATE_HOME", home.join("state"))
        .output()
        .unwrap();
    let mut found = Vec::new();
    let mut stack = vec![home.clone()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("crash-")) {
                found.push((p.file_name().unwrap().to_string_lossy().into_owned(), std::fs::read_to_string(&p).unwrap()));
            }
        }
    }
    let _ = std::fs::remove_dir_all(&home);
    (st.status, found)
}

#[test]
fn a_panic_is_saved_with_its_message_and_a_backtrace() {
    let (_, found) = run("panic", "child_panic");
    let (name, text) = found.first().expect("a report").clone();
    assert!(name.contains("panic"), "{name}");
    assert!(text.contains("version: 9.9.9-test") && text.contains("message: boom from the test"), "{text}");
    assert!(text.contains("backtrace:"));
}

#[cfg(unix)]
#[test]
fn a_native_crash_is_saved_and_still_kills_the_program() {
    let (status, found) = run("segv", "child_segv");
    assert!(!status.success());
    let (name, text) = found.first().expect("a report").clone();
    assert!(name.contains("native"), "{name}");
    assert!(text.contains("signal: SIGSEGV") && text.contains("fault_address: 0x8") && text.contains("version: 9.9.9-test"), "{text}");
    assert!(text.contains("backtrace"), "{text}");
}

#[cfg(unix)]
#[test]
fn an_abort_is_saved() {
    let (_, found) = run("abort", "child_segv");
    let text = &found.first().expect("a report").1;
    assert!(text.contains("signal: SIGABRT"), "{text}");
}
