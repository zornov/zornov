// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The optional, paid side of Studio, kept apart from the editor so the
//! two can be licensed differently: the editor builds and runs fully
//! without this crate (`studio-app` without its `commerce` feature).
//!
//! * [`Account`]: sign-in to a Studio cloud server (any address: the
//!   hosted one or `studio-cloud-server` run by anyone), the licence it
//!   issues (checked offline by `studio-entitlements`), credits, receipts.
//! * [`credits`]: prices per task, the usage ledger, budgets, quotes.
//! * Cloud clients: renders, review links, project sync, the marketplace.
//! * [`stripe`]: Checkout in test mode only (used by the server).
//!
//! Nothing is sent anywhere until the person turns the cloud on and picks
//! a server ([`Settings::enabled`]); every network call checks that first.

pub mod account;
pub mod credits;
pub mod market;
pub mod stripe;

pub use account::{Account, FileSecrets, Secrets, Settings, Status};

use serde_json::Value;
use std::time::Duration;

/// Studio's per-user data folder (as `studio-update` has it).
pub fn data_dir() -> std::path::PathBuf {
    let home = studio_paths::home();
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support/Studio")
    } else if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").filter(|v| !v.is_empty()).map(std::path::PathBuf::from).unwrap_or_else(|| home.join("AppData/Local")).join("Studio")
    } else {
        std::env::var_os("XDG_STATE_HOME").filter(|v| !v.is_empty()).map(std::path::PathBuf::from).unwrap_or_else(|| home.join(".local/state")).join("studio")
    }
}

/// Checks a server address: `https://...`, or plain `http://` to this
/// computer only (a server on localhost).
pub fn server_address(s: &str) -> Result<String, String> {
    let s = s.trim().trim_end_matches('/');
    let local = |rest: &str| {
        let host = rest.split('/').next().unwrap_or("");
        let host = host.rsplit_once(':').filter(|(h, p)| !h.is_empty() && p.chars().all(|c| c.is_ascii_digit())).map_or(host, |(h, _)| h);
        matches!(host, "localhost" | "127.0.0.1" | "[::1]") || host.ends_with(".localhost")
    };
    match s.split_once("://") {
        Some(("https", rest)) if !rest.is_empty() => Ok(s.to_owned()),
        Some(("http", rest)) if local(rest) => Ok(s.to_owned()),
        Some(("http", _)) => Err("a server on another computer needs https://".into()),
        _ => Err("a server address starts with https:// (or http://localhost)".into()),
    }
}

/// A JSON client for one server, with the person's session token.
#[derive(Clone)]
pub struct Client {
    pub base: String,
    pub token: Option<String>,
}

const QUIET: Duration = Duration::from_secs(60);

impl Client {
    fn headers<'a>(&'a self, auth: &'a mut String, extra: &[(&'a str, &'a str)]) -> Vec<(&'a str, &'a str)> {
        let mut h: Vec<(&str, &str)> = extra.to_vec();
        if let Some(t) = &self.token {
            *auth = format!("Bearer {t}");
            h.push(("Authorization", auth.as_str()));
        }
        h
    }

    fn json(r: Result<studio_net::Reply, String>) -> Result<Value, String> {
        let r = r?;
        let status = r.status;
        let body = r.bytes(64 << 20).map_err(|e| e.to_string())?;
        let v: Value = serde_json::from_slice(&body).unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&body).chars().take(300).collect()));
        if (200..300).contains(&status) {
            Ok(v)
        } else {
            Err(v.get("error").and_then(Value::as_str).map(str::to_owned).unwrap_or_else(|| format!("the server answered {status}")))
        }
    }

    pub fn get(&self, path: &str) -> Result<Value, String> {
        let mut a = String::new();
        let h = self.headers(&mut a, &[]);
        Self::json(studio_net::get(&format!("{}{path}", self.base), &h, QUIET))
    }

    pub fn post(&self, path: &str, body: &Value) -> Result<Value, String> {
        let mut a = String::new();
        let h = self.headers(&mut a, &[("Content-Type", "application/json")]);
        Self::json(studio_net::post(&format!("{}{path}", self.base), &h, body.to_string().as_bytes(), QUIET))
    }

    /// Uploads raw bytes.
    pub fn upload(&self, path: &str, content_type: &str, bytes: &[u8]) -> Result<Value, String> {
        let mut a = String::new();
        let h = self.headers(&mut a, &[("Content-Type", content_type)]);
        Self::json(studio_net::post(&format!("{}{path}", self.base), &h, bytes, Duration::from_secs(600)))
    }

    /// Downloads raw bytes (up to `limit`).
    pub fn bytes(&self, path: &str, limit: u64) -> Result<Vec<u8>, String> {
        let mut a = String::new();
        let h = self.headers(&mut a, &[]);
        let r = studio_net::get(&format!("{}{path}", self.base), &h, Duration::from_secs(600))?;
        let status = r.status;
        let b = r.bytes(limit).map_err(|e| e.to_string())?;
        if (200..300).contains(&status) {
            Ok(b)
        } else {
            Err(serde_json::from_slice::<Value>(&b).ok().and_then(|v| v.get("error").and_then(Value::as_str).map(str::to_owned)).unwrap_or_else(|| format!("the server answered {status}")))
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn server_addresses_need_https_off_this_computer() {
        assert_eq!(super::server_address("https://cloud.example.com/").unwrap(), "https://cloud.example.com");
        assert!(super::server_address("http://127.0.0.1:8787").is_ok());
        assert!(super::server_address("http://localhost").is_ok());
        assert!(super::server_address("http://studio.localhost:9000/x").is_ok());
        assert!(super::server_address("http://cloud.example.com").is_err());
        assert!(super::server_address("http://localhost.evil.com").is_err());
        assert!(super::server_address("ftp://x").is_err());
        assert!(super::server_address("").is_err());
    }
}
