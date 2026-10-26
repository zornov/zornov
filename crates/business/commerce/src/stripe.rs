// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Stripe Checkout, in TEST MODE ONLY.
//!
//! [`Stripe::test`] refuses any key that is not a test key (`sk_test_` or
//! `rk_test_`), and every reply that says `"livemode": true` is treated as
//! an error, so this code cannot take a real payment. The key is read at
//! run time (the Keychain or the server's environment) and only ever sent
//! in the `Authorization` header.

use ring::hmac;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;

/// Stripe's API.
pub const API: &str = "https://api.stripe.com";

pub struct Stripe {
    key: String,
    base: String,
}

/// A line to pay for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    pub name: String,
    pub amount_minor: u64,
    pub currency: String,
    #[serde(default = "one")]
    pub quantity: u32,
}

fn one() -> u32 {
    1
}

/// A Checkout Session, as far as Studio needs it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    /// Where the buyer pays (Stripe's hosted page).
    #[serde(default)]
    pub url: Option<String>,
    /// `paid`, `unpaid` or `no_payment_required`.
    #[serde(default)]
    pub payment_status: String,
    #[serde(default)]
    pub client_reference_id: Option<String>,
    #[serde(default)]
    pub amount_total: Option<u64>,
    #[serde(default)]
    pub currency: Option<String>,
}

pub fn is_test_key(key: &str) -> bool {
    key.starts_with("sk_test_") || key.starts_with("rk_test_")
}

impl Stripe {
    /// A test-mode client. `base` is [`API`] (or a local stand-in in tests).
    pub fn test(key: &str, base: &str) -> Result<Stripe, String> {
        let key = key.trim();
        if !is_test_key(key) {
            return Err("only Stripe test keys (sk_test_...) are accepted: Studio never takes real payments here".into());
        }
        Ok(Stripe { key: key.to_owned(), base: base.trim_end_matches('/').to_owned() })
    }

    fn auth(&self) -> String {
        format!("Bearer {}", self.key)
    }

    fn reply(r: Result<studio_net::Reply, String>) -> Result<Value, String> {
        let r = r.map_err(|e| format!("Stripe: {e}"))?;
        let status = r.status;
        let body = r.bytes(1 << 20).map_err(|e| format!("Stripe: {e}"))?;
        let v: Value = serde_json::from_slice(&body).map_err(|_| format!("Stripe answered {status} with something that is not JSON"))?;
        if !(200..300).contains(&status) {
            let msg = v.pointer("/error/message").and_then(Value::as_str).unwrap_or("no message");
            return Err(format!("Stripe answered {status}: {msg}"));
        }
        if v.get("livemode").and_then(Value::as_bool) == Some(true) {
            return Err("Stripe answered in live mode; Studio only uses test mode".into());
        }
        Ok(v)
    }

    /// Starts a one-off payment for `items`. `reference` comes back on the
    /// session (Studio's order id); `success_url` may hold
    /// `{CHECKOUT_SESSION_ID}`.
    pub fn checkout(&self, items: &[Item], reference: &str, email: Option<&str>, success_url: &str, cancel_url: &str) -> Result<Session, String> {
        if items.is_empty() {
            return Err("nothing to pay for".into());
        }
        let mut form: Vec<(String, String)> = vec![("mode".into(), "payment".into()), ("success_url".into(), success_url.into()), ("cancel_url".into(), cancel_url.into()), ("client_reference_id".into(), reference.into()), ("metadata[order]".into(), reference.into())];
        if let Some(e) = email.filter(|e| !e.is_empty()) {
            form.push(("customer_email".into(), e.into()));
        }
        for (i, it) in items.iter().enumerate() {
            let p = format!("line_items[{i}]");
            form.push((format!("{p}[quantity]"), it.quantity.max(1).to_string()));
            form.push((format!("{p}[price_data][currency]"), it.currency.to_ascii_lowercase()));
            form.push((format!("{p}[price_data][unit_amount]"), it.amount_minor.to_string()));
            form.push((format!("{p}[price_data][product_data][name]"), it.name.clone()));
        }
        let body = form_encode(&form);
        let auth = self.auth();
        let v = Self::reply(studio_net::post(&format!("{}/v1/checkout/sessions", self.base), &[("Authorization", &auth), ("Content-Type", "application/x-www-form-urlencoded"), ("Idempotency-Key", reference)], body.as_bytes(), Duration::from_secs(30)))?;
        serde_json::from_value(v).map_err(|e| format!("Stripe's session is not what Studio expected: {e}"))
    }

    /// Reads a session back (to confirm payment without a webhook).
    pub fn session(&self, id: &str) -> Result<Session, String> {
        if !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err("not a Checkout Session id".into());
        }
        let auth = self.auth();
        let v = Self::reply(studio_net::get(&format!("{}/v1/checkout/sessions/{id}", self.base), &[("Authorization", &auth)], Duration::from_secs(30)))?;
        serde_json::from_value(v).map_err(|e| format!("Stripe's session is not what Studio expected: {e}"))
    }
}

/// Checks a webhook's `Stripe-Signature` header (`t=...,v1=...`) against the
/// endpoint secret, within `tolerance` seconds of `now`.
pub fn verify_webhook(payload: &[u8], header: &str, secret: &str, now: u64, tolerance: u64) -> Result<(), String> {
    let mut t = None;
    let mut sigs = Vec::new();
    for part in header.split(',') {
        match part.trim().split_once('=') {
            Some(("t", v)) => t = v.parse::<u64>().ok(),
            Some(("v1", v)) => sigs.push(v.to_owned()),
            _ => {}
        }
    }
    let t = t.ok_or("the webhook has no timestamp")?;
    if now.abs_diff(t) > tolerance {
        return Err("the webhook is too old (or the clock is off)".into());
    }
    let key = hmac::Key::new(hmac::HMAC_SHA256, secret.as_bytes());
    let mut msg = format!("{t}.").into_bytes();
    msg.extend_from_slice(payload);
    for s in sigs {
        if let Some(sig) = studio_entitlements::unhex(&s)
            && hmac::verify(&key, &msg, &sig).is_ok() {
                return Ok(());
            }
    }
    Err("the webhook's signature does not match".into())
}

/// Signs a webhook payload the way Stripe does (for tests and local tools).
pub fn sign_webhook(payload: &[u8], secret: &str, t: u64) -> String {
    let key = hmac::Key::new(hmac::HMAC_SHA256, secret.as_bytes());
    let mut msg = format!("{t}.").into_bytes();
    msg.extend_from_slice(payload);
    format!("t={t},v1={}", studio_entitlements::hex(hmac::sign(&key, &msg).as_ref()))
}

pub fn form_encode(pairs: &[(String, String)]) -> String {
    pairs.iter().map(|(k, v)| format!("{}={}", url_encode(k), url_encode(v))).collect::<Vec<_>>().join("&")
}

pub fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

pub fn url_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => {
                match u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("zz"), 16) {
                    Ok(v) => {
                        out.push(v);
                        i += 2;
                    }
                    Err(_) => out.push(b'%'),
                }
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Parses `a=1&b=2` (form bodies and query strings).
pub fn form_decode(s: &str) -> Vec<(String, String)> {
    s.split('&').filter(|p| !p.is_empty()).map(|p| p.split_once('=').map_or((url_decode(p), String::new()), |(k, v)| (url_decode(k), url_decode(v)))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_keys_are_refused() {
        assert!(Stripe::test("sk_live_abc", API).is_err());
        assert!(Stripe::test("pk_test_abc", API).is_err());
        assert!(Stripe::test("whatever", API).is_err());
        assert!(Stripe::test("sk_test_abc", API).is_ok());
        assert!(Stripe::test("rk_test_abc", API).is_ok());
    }

    #[test]
    fn webhooks_verify_and_tampering_fails() {
        let body = br#"{"type":"checkout.session.completed"}"#;
        let h = sign_webhook(body, "whsec_test", 1_000);
        verify_webhook(body, &h, "whsec_test", 1_010, 300).unwrap();
        assert!(verify_webhook(b"{}", &h, "whsec_test", 1_010, 300).is_err());
        assert!(verify_webhook(body, &h, "whsec_other", 1_010, 300).is_err());
        assert!(verify_webhook(body, &h, "whsec_test", 2_000, 300).unwrap_err().contains("too old"));
    }

    /// Against Stripe's real test mode (no charge is possible with a test
    /// key): `STUDIO_STRIPE_TEST_KEY=sk_test_... cargo test -- --ignored`.
    #[test]
    #[ignore = "needs STUDIO_STRIPE_TEST_KEY and the network"]
    fn a_session_in_stripes_test_mode() {
        let key = std::env::var("STUDIO_STRIPE_TEST_KEY").expect("STUDIO_STRIPE_TEST_KEY");
        let s = Stripe::test(&key, API).unwrap();
        let item = Item { name: "Studio test item".into(), amount_minor: 500, currency: "usd".into(), quantity: 1 };
        let reference = format!("studio-test-{}", studio_entitlements::now());
        let sess = s.checkout(&[item], &reference, None, "http://localhost:8787/billing/done?session_id={CHECKOUT_SESSION_ID}", "http://localhost:8787/billing/cancelled").unwrap();
        assert!(sess.id.starts_with("cs_test_"), "{}", sess.id);
        assert!(sess.url.as_deref().is_some_and(|u| u.starts_with("https://checkout.stripe.com/")));
        let back = s.session(&sess.id).unwrap();
        assert_eq!(back.payment_status, "unpaid");
        assert_eq!(back.client_reference_id.as_deref(), Some(reference.as_str()));
    }

    #[test]
    fn forms_round_trip() {
        let pairs = vec![("line_items[0][price_data][product_data][name]".to_owned(), "Film LUT & grain 100%".to_owned())];
        let enc = form_encode(&pairs);
        assert!(!enc.contains(' ') && !enc.contains('&'), "{enc}");
        assert_eq!(form_decode(&enc), pairs);
        assert_eq!(url_decode("a+b%2Fc%"), "a b/c%");
    }
}
