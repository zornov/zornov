// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The default build (no `commerce` feature) has no account tool, and the
//! AI is told nothing about plans, credits, prices, the marketplace or
//! cloud services: not in the tool list, the capability map, the guide, the
//! core prompt, the craft book, or any tool's description or long form.

use super::*;

/// Phrases that would tell the AI (or the person) about paid or hosted
/// services. Matched in lower case.
const FORBIDDEN: &[&str] = &[
    "`account`",
    "account {",
    "an account",
    "your account",
    "sign up",
    "sign_up",
    "sign_in",
    "marketplace",
    "stripe checkout",
    "stripe test",
    "stripe.com",
    "upgrade",
    "subscription",
    "entitlement",
    "hosted ai",
    "ai credits",
    "credit balance",
    "credits a month",
    "studio cloud",
    "cloud render",
    "cloud server",
    "review link",
    "checkout",
    "free plan",
    "pro plan",
    "enterprise",
    "licence file",
    "byok",
    "tool:account",
];

fn clean(what: &str, text: &str) {
    let low = text.to_lowercase();
    for w in FORBIDDEN {
        if let Some(i) = low.find(w) {
            let from = low[..i].char_indices().rev().nth(60).map_or(0, |(j, _)| j);
            panic!("{what} mentions \"{w}\": …{}…", &low[from..(i + w.len() + 60).min(low.len())]);
        }
    }
}

#[test]
fn there_is_no_account_tool() {
    let tools = mcp::tools();
    assert!(tools.iter().all(|t| t["name"] != "account"), "the account tool is registered");
    let mut s = Session::default();
    assert!(s.call("account", &json!({})).is_err());
    assert!(guide_topic("account").is_err());
    assert!(guide_topic("tool:account").is_err());
}

#[test]
fn the_capability_map_follows_the_tools() {
    clean("the capability map", &map::capability_map());
    assert!(map::missing().is_empty(), "{:?}", map::missing());
    assert!(map::stale().is_empty(), "{:?}", map::stale());
    for q in ["account", "credits", "plan upgrade", "marketplace", "cloud render"] {
        for (_, name, does) in capabilities::search(q, 20) {
            clean(&format!("capabilities {q:?} hit {name}"), &format!("{name} {does}"));
        }
    }
}

#[test]
fn the_guide_and_prompts_say_nothing_of_money_or_the_cloud() {
    clean("the guide", GUIDE);
    clean("guide_core", GUIDE_CORE);
    clean("the craft book", craft_book::BOOK);
    clean("video_craft", brain::VIDEO_CRAFT);
    clean("the tool reference", TOOL_REFERENCE);
    for k in skills::all() {
        clean(&format!("skill {}", k.name), &format!("{} {} {}", k.use_when, k.procedure, k.refs.iter().map(|(t, b)| format!("{t} {b}")).collect::<Vec<_>>().join(" ")));
    }
    clean("the plan habit", brain::PLAN_HABIT);
    clean("the system prompt", &brain::system_prompt());
    for t in mcp::tools() {
        clean(&format!("tool {}", t["name"]), &format!("{} {}", t["description"], t["inputSchema"]));
    }
    for name in map::all_tools() {
        if let Ok(long) = guide_topic(&format!("tool:{name}")) {
            clean(&format!("the long form of {name}"), &long);
        }
    }
}
