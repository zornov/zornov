// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The project's rules for the agent, enforced by the server rather than
//! asked of the model: `generation: off` refuses every AI-made picture,
//! video or model (the `generate` tool, generative fill in `picture`, a
//! `character` the AI generates) whatever the prompt says.
//!
//! * The rules live beside the project in `agent-policy.json`
//!   (`{"generation": "off"}`); `ZORNOV_GENERATION=off` sets the same for
//!   every project a process opens. A project with no folder keeps them for
//!   the session.
//! * The agent can make the rules stricter (`policy {generation: "off"}`)
//!   but never looser: turning generation back on is the person's, by
//!   editing or deleting the file.
//! * Footage, the library, 3D built from shapes, sound made by the models
//!   here and everything else stay allowed.

use serde_json::{Value, json};
use std::cell::Cell;
use std::path::{Path, PathBuf};
use studio_model as m;

thread_local! {
    /// The rule for a project with no folder (per thread: tests run side by side).
    static OFF_HERE: Cell<bool> = const { Cell::new(false) };
}

fn file(home: Option<&Path>) -> Option<PathBuf> {
    home.map(|h| m::file::ProjectFolder::new(h).state(m::file::State::AgentPolicy))
}

/// Whether AI generation is off for this project.
pub fn generation_off(home: Option<&Path>) -> bool {
    if std::env::var("ZORNOV_GENERATION").is_ok_and(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "off" | "0" | "false" | "no")) {
        return true;
    }
    if let Some(p) = file(home)
        && let Ok(text) = std::fs::read_to_string(p)
        && let Ok(v) = serde_json::from_str::<Value>(&text)
    {
        return v["generation"] == json!("off") || v["generation"] == json!(false);
    }
    OFF_HERE.with(Cell::get)
}

/// What in this call would make something with AI generation, if anything.
pub fn generative(tool: &str, a: &Value) -> Option<String> {
    let yes = |k: &str| a.get(k).is_some_and(|v| v.as_bool() == Some(true));
    match tool {
        "generate" => {
            // Reading what is possible, a batch's progress, the looks, an estimate: not making.
            let reads = ["list", "estimate", "looks"].iter().any(|k| yes(k)) || a.get("forget_look").is_some() || (a.get("job").is_some() && a.get("items").is_none() && a.get("prompt").is_none());
            (!reads).then(|| "generate".to_owned())
        }
        "picture" => a.get("ops").and_then(Value::as_array).and_then(|ops| ops.iter().find_map(|o| o.get("op").and_then(Value::as_str).filter(|n| n.starts_with("generative")).map(|n| format!("picture op {n}")))),
        "edit" => a.get("ops").and_then(Value::as_array).and_then(|ops| {
            ops.iter().map(super::args::unwrap_op).find_map(|o| {
                let name = o.get("op").and_then(Value::as_str).unwrap_or("");
                (name == "character" && o.get("generate").is_some_and(|g| !g.is_null() && g.as_bool() != Some(false))).then(|| "edit op character with generate".to_owned())
            })
        }),
        _ => None,
    }
}

/// The refusal for a call the policy stops.
pub fn refusal(what: &str, home: Option<&Path>) -> String {
    let where_ = file(home).map_or_else(|| "for this session".to_owned(), |p| format!("in {}", p.display()));
    format!(
        "{what} is refused: AI generation is off for this project (policy {where_}). Use the person's footage, `search_media`, the library (`catalogue`), 3D built with model / asset / build_world, shapes, text and the sound models here instead; only the person can turn generation back on"
    )
}

/// The `policy` tool: no arguments reads the rules; `generation: "off"`
/// makes generation off.
pub fn tool(home: Option<&Path>, a: &Value) -> Result<String, String> {
    match a.get("generation") {
        None => {}
        Some(v) if v == &json!("off") || v == &json!(false) => {
            match file(home) {
                Some(p) => {
                    let mut now: Value = std::fs::read_to_string(&p).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_else(|| json!({}));
                    now["generation"] = json!("off");
                    std::fs::write(&p, serde_json::to_string_pretty(&now).unwrap_or_default()).map_err(|e| format!("could not write {}: {e}", p.display()))?;
                }
                None => OFF_HERE.with(|c| c.set(true)),
            }
        }
        Some(v) if v == &json!("on") || v == &json!(true) => {
            if generation_off(home) {
                return Err(format!("generation stays off: only the person can turn it back on ({}), not the agent", file(home).map_or_else(|| "a new session".to_owned(), |p| format!("by editing or deleting {}", p.display()))));
            }
        }
        Some(other) => return Err(format!("generation is \"on\" or \"off\", not {other}")),
    }
    let off = generation_off(home);
    Ok(json!({
        "generation": if off { "off" } else { "on" },
        "means": if off { "AI-made pictures, video and models are refused (generate, picture generative fill, character generate); footage, the library, 3D built from shapes and sound made here stay" } else { "generation is allowed where the person asked for it" },
        "file": file(home).map(|p| p.display().to_string()),
    })
    .to_string())
}

/// The tool's definition (short: it rides in every request).
pub fn tools() -> Vec<Value> {
    vec![json!({
        "name": "policy",
        "description": "The project's rules, enforced by Zornov. No arguments reads them; generation \"off\" refuses AI-made pictures, video and models (only the person turns it on).",
        "inputSchema": {"type": "object", "properties": {"generation": {"type": "string"}}}
    })]
}
