// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The `account` tool: the plan and what it allows, hosted AI credits
//! (quote before spending, the budget, usage), the cloud (sign-in, renders,
//! review links), receipts and the marketplace. The work is in
//! `studio-commerce`. This module is only built with the `commerce`
//! feature (off by default): without it the tool does not exist.

use super::Session;
use serde_json::{Value, json};

/// The tool's description and schema.
pub fn schema() -> Value {
    let s = |d: &str| json!({"type": "string", "description": d});
    let num = |d: &str| json!({"type": "number", "description": d});
    json!({
        "name": "account",
        "description": "The person's plan, hosted AI credits, cloud and marketplace. Before any task that spends credits call `quote` (never repeat its `say` line or prices in your messages: the panel shows the total); go ahead when `allowed` is true, and ask first, in one short line, only when the action would pass the set budget or about $5. Own API keys cost no credits. Fields: action (status, plans, quote, budget, usage, receipts, upgrade, buy_credits, confirm, connect, sign_in, render, review, market, buy, install, sell...), task, units, seconds, characters, count, tokens, byok. More: guide {topic: \"tool:account\"}.",
        "inputSchema": {"type": "object", "properties": {
            "action": s("status, plans, quote, budget, usage, receipts, packs, upgrade, buy_credits, confirm, connect, disconnect, sign_up, sign_in, sign_out, refresh, licence, render, render_status, render_fetch, review, review_notes, market, buy, install, installed, sell, statement"),
            "task": s("quote: transcribe, voice, llm, ... (plans lists them)"),
            "units": num("quote: units of the task (the price table's unit)"),
            "seconds": num("quote: media seconds (transcribe)"),
            "characters": {"type": "integer", "description": "quote: characters to speak (voice)"},
            "count": {"type": "integer", "description": "quote: how many (pictures, sounds)"},
            "tokens": {"type": "integer", "description": "quote: model tokens (llm)"},
            "byok": {"type": "boolean", "description": "quote: run with the person's own key (default: when one is saved)"},
            "monthly_limit": {"type": "integer", "description": "budget: credits a month (0 removes the limit)"},
            "warn_at_percent": {"type": "integer"},
            "stop_at_limit": {"type": "boolean"},
            "plan": s("upgrade: a plan id"), "pack": s("buy_credits: a credit pack id"), "session": s("confirm: the checkout session"),
            "server": s("connect: https://... or http://localhost:PORT"), "trust_licences": {"type": "boolean", "description": "connect: trust licences this server signs (your own server)"},
            "email": s("sign_up / sign_in"), "password": s("sign_up / sign_in (never repeated back)"), "name": s("sign_up: shown name; render: job name"),
            "path": s("licence file; render_fetch / review / sell file"), "title": s("review / sell"), "format": s("render: mp4 (default), mov..."),
            "id": s("render job or listing id"), "token": s("review link token"),
            "kind": s("market / sell: template, plugin, lut, sound, asset"), "query": s("market: words to look for"),
            "description": s("sell"), "price_minor": {"type": "integer", "description": "sell: price in cents (0 is free)"}, "currency": s("sell: usd (default)")
        }, "required": []}
    })
}

impl Session {
    pub(super) fn account_tool(&mut self, a: &Value) -> Result<Value, String> {
        on::run(self, a)
    }
}

pub mod on {
    use super::super::{Session, keychain};
    use serde_json::{Value, json};
    use std::path::PathBuf;
    use studio_commerce::{Account, FileSecrets, Secrets, credits};

    /// Secrets in the Keychain (Studio's own item), or in a private file
    /// where there is none or `STUDIO_NO_KEYCHAIN=1` (headless runs).
    struct KeychainSecrets;

    impl Secrets for KeychainSecrets {
        fn get(&self, name: &str) -> Option<String> {
            keychain::api_key(name)
        }
        fn set(&self, name: &str, value: &str) -> Result<(), String> {
            keychain::set_api_key(name, value)
        }
        fn remove(&self, name: &str) {
            keychain::remove_api_key(name)
        }
    }

    fn no_keychain() -> bool {
        !cfg!(target_os = "macos") || cfg!(test) || std::env::var("STUDIO_NO_KEYCHAIN").is_ok_and(|v| v == "1")
    }

    /// The person's account (Studio's data folder).
    pub fn account() -> Account {
        let dir = Account::default_dir();
        let secrets: Box<dyn Secrets> = if no_keychain() { Box::new(FileSecrets { path: dir.join("secrets.json") }) } else { Box::new(KeychainSecrets) };
        Account::open(dir, secrets)
    }

    /// Whether the person saved their own key for `provider`.
    pub fn has_own_key(provider: &str) -> bool {
        if no_keychain() {
            return false;
        }
        let acct = match provider {
            "openai" => keychain::OPENAI,
            "anthropic" => keychain::ANTHROPIC,
            "elevenlabs" => keychain::ELEVENLABS,
            "fish" => keychain::FISH,
            _ => return false,
        };
        keychain::api_key(acct).is_some()
    }

    fn st<'a>(a: &'a Value, k: &str) -> Option<&'a str> {
        a.get(k).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty())
    }

    fn need<'a>(a: &'a Value, k: &str, action: &str) -> Result<&'a str, String> {
        st(a, k).ok_or_else(|| format!("{action} needs \"{k}\""))
    }

    /// The status the person sees, in small JSON.
    pub fn status(acct: &Account) -> Value {
        let s = acct.status();
        let e = &s.entitlements;
        json!({
            "plan": e.plan.id, "plan_name": e.plan.name, "standing": e.standing,
            "features": e.features, "monthly_credits": e.monthly_credits, "seats": e.seats,
            "cloud": {"on": s.cloud.enabled, "server": s.cloud.server},
            "signed_in": s.signed_in, "email": s.me.as_ref().map(|m| m.email.clone()),
            "balance": s.me.as_ref().map(|m| m.balance), "month_used": s.month_used,
            "budget": s.budget,
        })
    }

    pub(in crate::agent) fn run(sess: &mut Session, a: &Value) -> Result<Value, String> {
        let acct = account();
        let action = st(a, "action").unwrap_or("status");
        let studio = studio_paths::studio();
        match action {
            "status" => Ok(status(&acct)),
            "plans" => {
                let cur = acct.entitlements().plan.id;
                let c = acct.catalogue();
                let plans: Vec<Value> = c.plans.iter().map(|p| json!({"id": p.id, "name": p.name, "summary": p.summary, "features": p.features.iter().map(|f| studio_entitlements::describe(f)).collect::<Vec<_>>(), "monthly_credits": p.monthly_credits, "seats": p.seats, "prices": p.prices, "current": p.id == cur})).collect();
                let tasks = acct.ledger().prices().map(|t| t.tasks.iter().map(|x| json!({"task": x.task, "unit": x.unit, "credits": x.credits, "provider": x.provider})).collect::<Vec<_>>());
                Ok(json!({"plans": plans, "credit_prices": tasks}))
            }
            "quote" => {
                let task = need(a, "task", "quote")?;
                let n = |k: &str| a.get(k).and_then(Value::as_f64);
                let units = n("units").or_else(|| credits::estimate_units(task, n("seconds"), a.get("characters").and_then(Value::as_u64), a.get("count").and_then(Value::as_u64), a.get("tokens").and_then(Value::as_u64))).ok_or("quote needs units (or seconds / characters / count / tokens)")?;
                let provider = acct.ledger().prices().and_then(|t| t.task(task).map(|p| p.provider.clone())).unwrap_or_default();
                let byok = a.get("byok").and_then(Value::as_bool).unwrap_or_else(|| has_own_key(&provider));
                serde_json::to_value(acct.quote(task, units, byok)?).map_err(|e| e.to_string())
            }
            "budget" => {
                let mut b = acct.ledger().budget();
                if let Some(l) = a.get("monthly_limit").and_then(Value::as_u64) {
                    b.monthly_limit = (l > 0).then_some(l);
                }
                if let Some(w) = a.get("warn_at_percent").and_then(Value::as_u64) {
                    b.warn_at_percent = w.clamp(1, 100) as u8;
                }
                if let Some(s) = a.get("stop_at_limit").and_then(Value::as_bool) {
                    b.stop_at_limit = s;
                }
                acct.ledger().set_budget(&b)?;
                Ok(json!({"budget": b, "month_used": acct.status().month_used}))
            }
            "usage" => {
                let mut e = acct.ledger().entries();
                let total = e.len();
                e.reverse();
                e.truncate(50);
                Ok(json!({"month_used": acct.status().month_used, "entries": e, "total": total}))
            }
            "receipts" => acct.receipts(),
            "packs" => acct.packs(),
            "upgrade" => acct.checkout("plan", need(a, "plan", "upgrade")?).map(checkout_reply),
            "buy_credits" => acct.checkout("credits", need(a, "pack", "buy_credits")?).map(checkout_reply),
            "confirm" => acct.confirm(need(a, "session", "confirm")?),
            "connect" => {
                let issuer = acct.connect(need(a, "server", "connect")?, a.get("trust_licences").and_then(Value::as_bool).unwrap_or(false))?;
                Ok(json!({"connected": acct.settings().server, "server_name": issuer["name"], "status": status(&acct)}))
            }
            "disconnect" => acct.disconnect().map(|_| json!({"cloud": "off"})),
            "sign_up" => acct.sign_up(need(a, "email", "sign_up")?, need(a, "password", "sign_up")?, st(a, "name").unwrap_or("")),
            "sign_in" => acct.sign_in(need(a, "email", "sign_in")?, need(a, "password", "sign_in")?),
            "sign_out" => acct.sign_out().map(|_| json!({"signed_out": true, "plan": acct.entitlements().plan.id})),
            "refresh" => acct.refresh(),
            "licence" => acct.install_licence_file(&PathBuf::from(need(a, "path", "licence")?)),
            "render" => {
                let project = serde_json::to_value(&sess.doc.model).map_err(|e| e.to_string())?;
                let name = st(a, "name").map(str::to_owned).unwrap_or_else(|| sess.doc.folder.as_ref().and_then(|f| f.root().file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "Studio project".into()));
                acct.render_submit(&name, &project, &json!({"format": st(a, "format").unwrap_or("mp4")}))
            }
            "render_status" => acct.render_status(need(a, "id", "render_status")?),
            "render_fetch" => {
                let path = PathBuf::from(need(a, "path", "render_fetch")?);
                let n = acct.render_fetch(need(a, "id", "render_fetch")?, &path)?;
                Ok(json!({"saved": path.display().to_string(), "bytes": n}))
            }
            "review" => {
                let path = PathBuf::from(need(a, "path", "review")?);
                let title = st(a, "title").map(str::to_owned).unwrap_or_else(|| path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default());
                acct.review_link(&path, &title)
            }
            "review_notes" => acct.review_notes(need(a, "token", "review_notes")?),
            "market" => acct.market(st(a, "kind"), st(a, "query")),
            "buy" => acct.buy(need(a, "id", "buy")?, &studio).map(checkout_reply),
            "install" => acct.install(need(a, "id", "install")?, &studio).map(|p| json!({"installed": p.display().to_string()})),
            "installed" => Ok(json!(studio_commerce::market::installed(&studio))),
            "sell" => {
                let l = acct.sell(need(a, "kind", "sell")?, need(a, "title", "sell")?, st(a, "description").unwrap_or(""), a.get("price_minor").and_then(Value::as_u64).unwrap_or(0), st(a, "currency").unwrap_or("usd"), &PathBuf::from(need(a, "path", "sell")?))?;
                serde_json::to_value(l).map_err(|e| e.to_string())
            }
            "statement" => acct.statement(),
            other => Err(format!("account has no action \"{other}\"")),
        }
    }

    /// A checkout reply, with what to tell the person.
    fn checkout_reply(mut v: Value) -> Value {
        if v.get("url").and_then(Value::as_str).is_some() {
            v["say"] = json!("Open the checkout link to pay (Stripe test mode: use a test card such as 4242 4242 4242 4242; nothing real is charged), then confirm with the session.");
        }
        v
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_schema_lists_the_actions() {
        let s = super::schema();
        assert_eq!(s["name"], "account");
        assert!(s["description"].as_str().unwrap().contains("quote"));
    }
}
