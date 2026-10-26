// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! ```text
//! studio-cloud-server serve --data DIR [--addr 127.0.0.1:8787]
//! studio-cloud-server key   --data DIR          the licence public key
//! studio-cloud-server issue --data DIR --plan P --holder NAME [--email E] [--days N] [--seats N]
//!                                               a signed licence file on stdout
//! ```
//!
//! Payments use a Stripe TEST key, read at start-up from
//! `STUDIO_STRIPE_TEST_KEY` or, on macOS, the Keychain item "Studio API
//! keys" / "stripe-test-secret-key". Live keys are refused. The webhook
//! secret is `STUDIO_STRIPE_WEBHOOK_SECRET` (optional: payments are also
//! confirmed by reading the session back from Stripe).

use std::path::PathBuf;
use studio_cloud_server::{Config, start};
use studio_entitlements::{Issuer, Licence, now, write_atomic};

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

fn keychain(account: &str) -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        let b = security_framework::passwords::get_generic_password("Studio API keys", account).ok()?;
        Some(String::from_utf8(b).ok()?.trim().to_owned()).filter(|k| !k.is_empty())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = account;
        None
    }
}

fn issuer(data: &std::path::Path) -> Result<Issuer, String> {
    let path = data.join("issuer.pk8");
    let k = match std::fs::read(&path) {
        Ok(k) => k,
        Err(_) => {
            let k = Issuer::generate()?;
            write_atomic(&path, &k)?;
            k
        }
    };
    Issuer::from_pkcs8(&k)
}

fn run(args: &[String]) -> Result<(), String> {
    let cmd = args.first().map(String::as_str).unwrap_or("");
    let data = PathBuf::from(arg(args, "--data").ok_or("--data DIR is needed (where the server keeps everything)")?);
    match cmd {
        "serve" => {
            let mut cfg = Config::load(&data)?;
            let env = |n: &str| std::env::var(n).ok().filter(|v| !v.is_empty());
            cfg.stripe_key = env("STUDIO_STRIPE_TEST_KEY").or_else(|| keychain("stripe-test-secret-key"));
            cfg.webhook_secret = env("STUDIO_STRIPE_WEBHOOK_SECRET");
            let addr = arg(args, "--addr").unwrap_or_else(|| "127.0.0.1:8787".into());
            let pay = if cfg.stripe_key.is_some() { "Stripe test mode" } else { "off (no Stripe test key)" };
            let r = start(cfg, &addr)?;
            println!("studio-cloud-server on {} (data {}; payments {pay}; licence key {})", r.url(), data.display(), r.server.issuer_public_key());
            loop {
                std::thread::park();
            }
        }
        "key" => {
            println!("{}", issuer(&data)?.public_key_hex());
            Ok(())
        }
        "issue" => {
            let i = issuer(&data)?;
            let days: u64 = arg(args, "--days").map(|d| d.parse().map_err(|_| "--days is a number")).transpose()?.unwrap_or(365);
            let l = Licence {
                schema: 1,
                id: format!("L-{}", now()),
                plan: arg(args, "--plan").ok_or("--plan is needed")?,
                holder: arg(args, "--holder").ok_or("--holder is needed")?,
                email: arg(args, "--email").unwrap_or_default(),
                seats: arg(args, "--seats").and_then(|s| s.parse().ok()).unwrap_or(1),
                issued: now(),
                expires: (days > 0).then(|| now() + days * 86_400),
                grace_days: 14,
                add: vec![],
                remove: vec![],
                monthly_credits: None,
                key_id: String::new(),
                server: String::new(),
            };
            println!("{}", i.sign(&l).to_json());
            Ok(())
        }
        _ => Err("commands: serve, key, issue".into()),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Err(e) = run(&args) {
        eprintln!("studio-cloud-server: {e}");
        std::process::exit(1);
    }
}
