// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The person's account on a Studio cloud server, and everything kept for
//! it on this computer (one folder):
//!
//! ```text
//! cloud.json         whether the cloud is on, and the server's address
//! me.json            who is signed in, plan and balance as last seen
//! licence.json       the signed licence (checked offline)
//! trusted-keys.json  licence issuers the person chose to trust
//! plans.json         the server's plan list (with prices)
//! prices.json        the server's credit prices per task
//! usage.jsonl        the usage ledger
//! budget.json        the monthly credit budget
//! ```
//!
//! The session token is not in the folder: it goes to [`Secrets`] (the
//! Keychain in the app).

use crate::credits::{self, Budget, Ledger, PriceTable, Quote, Usage};
use crate::market::{self, Listing};
use crate::{Client, server_address};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use studio_entitlements::{Catalogue, Entitlements, SignedLicence, Store, TrustedKey, write_atomic};

/// Where secrets (the session token) are kept.
pub trait Secrets: Send {
    fn get(&self, name: &str) -> Option<String>;
    fn set(&self, name: &str, value: &str) -> Result<(), String>;
    fn remove(&self, name: &str);
}

/// Secrets in a file only this user can read (for computers without a
/// keychain, and for tests).
pub struct FileSecrets {
    pub path: PathBuf,
}

impl FileSecrets {
    fn all(&self) -> serde_json::Map<String, Value> {
        std::fs::read_to_string(&self.path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    fn save(&self, m: &serde_json::Map<String, Value>) -> Result<(), String> {
        write_atomic(&self.path, Value::Object(m.clone()).to_string().as_bytes())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&self.path, std::fs::Permissions::from_mode(0o600));
        }
        Ok(())
    }
}

impl Secrets for FileSecrets {
    fn get(&self, name: &str) -> Option<String> {
        self.all().get(name).and_then(Value::as_str).map(str::to_owned)
    }
    fn set(&self, name: &str, value: &str) -> Result<(), String> {
        let mut m = self.all();
        m.insert(name.into(), json!(value));
        self.save(&m)
    }
    fn remove(&self, name: &str) {
        let mut m = self.all();
        if m.remove(name).is_some() {
            let _ = self.save(&m);
        }
    }
}

const TOKEN: &str = "studio-cloud-token";

/// The cloud switch: off until the person turns it on.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub server: String,
}

/// Who is signed in, as the server last said.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Me {
    pub email: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub plan: String,
    #[serde(default)]
    pub balance: i64,
    /// Unix seconds when this was read.
    #[serde(default)]
    pub seen: u64,
}

/// Everything about the account that is known offline.
#[derive(Clone, Debug, Serialize)]
pub struct Status {
    pub cloud: Settings,
    pub signed_in: bool,
    pub me: Option<Me>,
    pub entitlements: Entitlements,
    pub month_used: u64,
    pub budget: Budget,
}

pub struct Account {
    pub dir: PathBuf,
    secrets: Box<dyn Secrets>,
}

fn st<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

impl Account {
    pub fn open(dir: impl Into<PathBuf>, secrets: Box<dyn Secrets>) -> Account {
        Account { dir: dir.into(), secrets }
    }

    /// The account folder in Studio's data folder, secrets in a file there.
    pub fn default_dir() -> PathBuf {
        crate::data_dir().join("account")
    }

    pub fn store(&self) -> Store {
        Store::new(&self.dir)
    }

    pub fn ledger(&self) -> Ledger {
        Ledger::new(&self.dir)
    }

    pub fn settings(&self) -> Settings {
        credits::read(&self.dir, "cloud.json").unwrap_or_default()
    }

    fn save_settings(&self, s: &Settings) -> Result<(), String> {
        write_atomic(&self.dir.join("cloud.json"), serde_json::to_string_pretty(s).expect("serialisable").as_bytes())
    }

    pub fn me(&self) -> Option<Me> {
        credits::read(&self.dir, "me.json")
    }

    pub fn entitlements(&self) -> Entitlements {
        self.store().entitlements()
    }

    pub fn catalogue(&self) -> Catalogue {
        self.store().catalogue()
    }

    pub fn signed_in(&self) -> bool {
        self.secrets.get(TOKEN).is_some()
    }

    pub fn status(&self) -> Status {
        let l = self.ledger();
        Status { cloud: self.settings(), signed_in: self.signed_in(), me: self.me(), entitlements: self.entitlements(), month_used: l.used_this_month(studio_entitlements::now()), budget: l.budget() }
    }

    /// Turns the cloud on with `server`. With `trust_issuer` the server's
    /// licence key is trusted too (a self-hosted server the person runs).
    pub fn connect(&self, server: &str, trust_issuer: bool) -> Result<Value, String> {
        let server = server_address(server)?;
        let c = Client { base: server.clone(), token: None };
        let issuer = c.get("/v1/issuer").map_err(|e| format!("{server} is not a Studio cloud server ({e})"))?;
        if trust_issuer {
            self.store().trust(TrustedKey { public_key: st(&issuer, "public_key").into(), name: format!("{} ({server})", st(&issuer, "name")) })?;
        }
        let old = self.settings();
        if old.server != server {
            // Another server: the old session means nothing there.
            self.secrets.remove(TOKEN);
            let _ = std::fs::remove_file(self.dir.join("me.json"));
        }
        self.save_settings(&Settings { enabled: true, server })?;
        let _ = self.refresh_public();
        Ok(issuer)
    }

    /// Turns the cloud off: nothing more is sent. The licence stays.
    pub fn disconnect(&self) -> Result<(), String> {
        let s = Settings { enabled: false, ..self.settings() };
        self.save_settings(&s)
    }

    /// A client for the server, or why there is none.
    pub fn client(&self) -> Result<Client, String> {
        let s = self.settings();
        if !s.enabled || s.server.is_empty() {
            return Err("the cloud is off: turn it on in Settings ▸ Account (nothing is sent until then)".into());
        }
        Ok(Client { base: server_address(&s.server)?, token: self.secrets.get(TOKEN) })
    }

    fn authed(&self) -> Result<Client, String> {
        let c = self.client()?;
        if c.token.is_none() {
            return Err("sign in first (Settings ▸ Account)".into());
        }
        Ok(c)
    }

    fn signed(&self, reply: Value) -> Result<Value, String> {
        let token = st(&reply, "token");
        if token.is_empty() {
            return Err("the server did not start a session".into());
        }
        self.secrets.set(TOKEN, token)?;
        self.refresh()
    }

    pub fn sign_up(&self, email: &str, password: &str, name: &str) -> Result<Value, String> {
        let r = self.client()?.post("/v1/auth/signup", &json!({"email": email, "password": password, "name": name}))?;
        self.signed(r)
    }

    pub fn sign_in(&self, email: &str, password: &str) -> Result<Value, String> {
        let r = self.client()?.post("/v1/auth/signin", &json!({"email": email, "password": password}))?;
        self.signed(r)
    }

    /// Ends the session here (the licence stays, so paid features keep
    /// working offline until it expires).
    pub fn sign_out(&self) -> Result<(), String> {
        if let Ok(c) = self.authed() {
            let _ = c.post("/v1/auth/signout", &json!({}));
        }
        self.secrets.remove(TOKEN);
        let _ = std::fs::remove_file(self.dir.join("me.json"));
        Ok(())
    }

    /// Plans and prices (no sign-in needed).
    fn refresh_public(&self) -> Result<(), String> {
        let c = self.client()?;
        let plans = c.get("/v1/plans")?;
        let cat = Catalogue::parse(&plans.to_string())?;
        self.store().save_catalogue(&cat)?;
        let prices = PriceTable::parse(&c.get("/v1/prices")?.to_string())?;
        self.ledger().save_prices(&prices)
    }

    /// Reads the account from the server: profile, balance, the latest
    /// licence (installed when it verifies), plans and prices.
    pub fn refresh(&self) -> Result<Value, String> {
        let c = self.authed()?;
        self.refresh_public()?;
        let v = c.get("/v1/me")?;
        let me = Me { email: st(&v, "email").into(), name: st(&v, "name").into(), plan: st(&v, "plan").into(), balance: v.get("balance").and_then(Value::as_i64).unwrap_or(0), seen: studio_entitlements::now() };
        write_atomic(&self.dir.join("me.json"), serde_json::to_string_pretty(&me).expect("serialisable").as_bytes())?;
        let mut out = json!({"email": me.email, "plan": me.plan, "balance": me.balance});
        match v.get("licence").filter(|l| !l.is_null()) {
            Some(l) => {
                let signed: SignedLicence = serde_json::from_value(l.clone()).map_err(|e| format!("the server's licence is not valid: {e}"))?;
                match self.store().install(&signed) {
                    Ok(l) => out["licence"] = json!({"plan": l.plan, "expires": l.expires}),
                    Err(e) => out["licence_problem"] = json!(e),
                }
            }
            // The plan ended: drop the licence this server issued (one
            // installed from a file, or from another server, stays).
            None => {
                let server = self.settings().server;
                if self.store().licence().and_then(|l| l.claims().ok()).is_some_and(|l| !server.is_empty() && l.server == server) {
                    self.store().remove_licence();
                }
            }
        }
        Ok(out)
    }

    /// Installs a licence file someone was given (works offline).
    pub fn install_licence_file(&self, path: &Path) -> Result<Value, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        let l = self.store().install(&SignedLicence::parse(&text)?)?;
        Ok(json!({"plan": l.plan, "holder": l.holder, "expires": l.expires}))
    }

    // ------------------------------------------------------------- credits

    /// What `units` of `task` will cost, from the cached prices and
    /// balance (no network). `byok`: the person's own key will run it.
    pub fn quote(&self, task: &str, units: f64, byok: bool) -> Result<Quote, String> {
        let l = self.ledger();
        let table = l.prices().ok_or("no credit prices yet: turn the cloud on and sign in once (prices are kept for offline use)")?;
        let mut q = credits::quote(&table, &l.budget(), l.used_this_month(studio_entitlements::now()), self.me().map(|m| m.balance), task, units, byok)?;
        if !byok && !self.entitlements().allows("ai.hosted") {
            q.allowed = false;
            q.warnings.insert(0, self.entitlements().require("ai.hosted", &self.catalogue()).unwrap_err());
            credits::describe(&mut q);
        }
        Ok(q)
    }

    /// Holds the credits for a hosted task on the server (after the
    /// person saw the quote). Returns the hold id.
    pub fn reserve(&self, task: &str, units: f64) -> Result<Value, String> {
        let q = self.quote(task, units, false)?;
        if !q.allowed {
            return Err(q.warnings.join("; "));
        }
        self.authed()?.post("/v1/credits/reserve", &json!({"task": task, "units": units}))
    }

    /// Charges a hold for what was really used, and records it.
    pub fn commit(&self, hold: &str, task: &str, units: f64) -> Result<Value, String> {
        let r = self.authed()?.post("/v1/credits/commit", &json!({"hold": hold, "units": units}))?;
        let credits = r.get("credits").and_then(Value::as_u64).unwrap_or(0);
        let provider = self.ledger().prices().and_then(|t| t.task(task).map(|p| p.provider.clone())).unwrap_or_default();
        self.ledger().record(&Usage { at: studio_entitlements::now(), task: task.into(), units, credits, byok: false, provider, hold: hold.into(), note: String::new() })?;
        if let (Some(mut me), Some(b)) = (self.me(), r.get("balance").and_then(Value::as_i64)) {
            me.balance = b;
            let _ = write_atomic(&self.dir.join("me.json"), serde_json::to_string_pretty(&me).expect("serialisable").as_bytes());
        }
        Ok(r)
    }

    /// Gives a hold back (the task failed or was cancelled).
    pub fn release(&self, hold: &str) -> Result<Value, String> {
        self.authed()?.post("/v1/credits/release", &json!({"hold": hold}))
    }

    /// Records a task run with the person's own key (no credits; for the
    /// usage view only).
    pub fn record_byok(&self, task: &str, units: f64, provider: &str) -> Result<(), String> {
        self.ledger().record(&Usage { at: studio_entitlements::now(), task: task.into(), units, credits: 0, byok: true, provider: provider.into(), hold: String::new(), note: String::new() })
    }

    // ------------------------------------------------------------- billing

    /// Starts paying for a plan, a credit pack or a listing. A free thing
    /// is done at once; otherwise the reply has the checkout `url`.
    pub fn checkout(&self, what: &str, id: &str) -> Result<Value, String> {
        if !matches!(what, "plan" | "credits" | "listing") {
            return Err("checkout is for a plan, credits or a listing".into());
        }
        self.authed()?.post("/v1/billing/checkout", &json!({"what": what, "id": id}))
    }

    /// Asks the server whether a checkout was paid, then refreshes.
    pub fn confirm(&self, session: &str) -> Result<Value, String> {
        let mut r = self.authed()?.post("/v1/billing/confirm", &json!({"session": session}))?;
        if let Ok(me) = self.refresh() {
            r["account"] = me;
        }
        Ok(r)
    }

    pub fn receipts(&self) -> Result<Value, String> {
        self.authed()?.get("/v1/receipts")
    }

    pub fn packs(&self) -> Result<Value, String> {
        self.client()?.get("/v1/credits/packs")
    }

    // -------------------------------------------------------------- render

    fn feature(&self, f: &str) -> Result<(), String> {
        self.entitlements().require(f, &self.catalogue())
    }

    /// Sends a project to be rendered on the server.
    pub fn render_submit(&self, name: &str, project: &Value, settings: &Value) -> Result<Value, String> {
        self.feature("cloud.render")?;
        self.authed()?.post("/v1/render/jobs", &json!({"name": name, "project": project, "settings": settings}))
    }

    pub fn render_status(&self, id: &str) -> Result<Value, String> {
        self.authed()?.get(&format!("/v1/render/jobs/{}", crate::stripe::url_encode(id)))
    }

    /// Downloads a finished render to `dest`.
    pub fn render_fetch(&self, id: &str, dest: &Path) -> Result<u64, String> {
        let b = self.authed()?.bytes(&format!("/v1/render/jobs/{}/output", crate::stripe::url_encode(id)), 16 << 30)?;
        write_atomic(dest, &b)?;
        Ok(b.len() as u64)
    }

    // -------------------------------------------------------------- review

    /// Uploads a rendered file and returns a link anyone can open to watch
    /// it and leave notes.
    pub fn review_link(&self, file: &Path, title: &str) -> Result<Value, String> {
        self.feature("cloud.review")?;
        let bytes = std::fs::read(file).map_err(|e| format!("cannot read {}: {e}", file.display()))?;
        let ct = match file.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
            Some("mp4" | "m4v") => "video/mp4",
            Some("mov") => "video/quicktime",
            Some("webm") => "video/webm",
            Some("png") => "image/png",
            Some("jpg" | "jpeg") => "image/jpeg",
            Some("wav") => "audio/wav",
            _ => "application/octet-stream",
        };
        let name = file.file_name().and_then(|n| n.to_str()).unwrap_or("file");
        self.authed()?.upload(&format!("/v1/review?title={}&name={}", crate::stripe::url_encode(title), crate::stripe::url_encode(name)), ct, &bytes)
    }

    pub fn review_notes(&self, token: &str) -> Result<Value, String> {
        self.client()?.get(&format!("/v1/review/{}/comments", crate::stripe::url_encode(token)))
    }

    // ---------------------------------------------------------------- sync

    /// A project's sync space on the server.
    pub fn sync(&self, project: &str) -> Result<SyncClient, String> {
        self.feature("cloud.sync")?;
        Ok(SyncClient { client: self.authed()?, project: market::safe_name(project) })
    }

    // ---------------------------------------------------------- marketplace

    pub fn market(&self, kind: Option<&str>, query: Option<&str>) -> Result<Value, String> {
        let mut q = Vec::new();
        if let Some(k) = kind {
            q.push(format!("kind={}", crate::stripe::url_encode(k)));
        }
        if let Some(s) = query {
            q.push(format!("q={}", crate::stripe::url_encode(s)));
        }
        self.client()?.get(&format!("/v1/market?{}", q.join("&")))
    }

    pub fn listing(&self, id: &str) -> Result<Listing, String> {
        let v = self.client()?.get(&format!("/v1/market/{}", crate::stripe::url_encode(id)))?;
        serde_json::from_value(v).map_err(|e| format!("the listing is not valid: {e}"))
    }

    /// Buys a listing: free or already owned installs now; otherwise the
    /// reply has the checkout `url` (install after paying).
    pub fn buy(&self, id: &str, studio: &Path) -> Result<Value, String> {
        self.feature("market.buy")?;
        let l = self.listing(id)?;
        let owned = self.authed()?.get("/v1/market/owned")?;
        let have = owned.as_array().is_some_and(|a| a.iter().any(|x| x.as_str() == Some(&l.id)));
        if l.price_minor == 0 || have {
            let file = self.install(id, studio)?;
            return Ok(json!({"installed": file.display().to_string(), "title": l.title}));
        }
        self.checkout("listing", id)
    }

    /// Downloads an owned (or free) listing and installs it.
    pub fn install(&self, id: &str, studio: &Path) -> Result<PathBuf, String> {
        let l = self.listing(id)?;
        let b = self.authed()?.bytes(&format!("/v1/market/{}/download", crate::stripe::url_encode(id)), 4 << 30)?;
        market::install(studio, &l, &b)
    }

    /// Lists something for sale (`market.sell`).
    pub fn sell(&self, kind: &str, title: &str, description: &str, price_minor: u64, currency: &str, file: &Path) -> Result<Listing, String> {
        self.feature("market.sell")?;
        if !market::KINDS.contains(&kind) {
            return Err(format!("a listing is one of: {}", market::KINDS.join(", ")));
        }
        let bytes = std::fs::read(file).map_err(|e| format!("cannot read {}: {e}", file.display()))?;
        let c = self.authed()?;
        let l = c.post("/v1/market", &json!({"kind": kind, "title": title, "description": description, "price_minor": price_minor, "currency": currency}))?;
        let name = file.file_name().and_then(|n| n.to_str()).unwrap_or("file");
        let v = c.upload(&format!("/v1/market/{}/file?name={}", crate::stripe::url_encode(st(&l, "id")), crate::stripe::url_encode(name)), "application/octet-stream", &bytes)?;
        serde_json::from_value(v).map_err(|e| format!("the listing is not valid: {e}"))
    }

    pub fn statement(&self) -> Result<Value, String> {
        self.authed()?.get("/v1/creator/statement")
    }
}

/// A project's shared space on the server: the same calls as a shared
/// folder (head, revisions, an atomic publish, members, notes, team), so
/// the editor's sync works through it unchanged. Projects and notes travel
/// as JSON.
pub struct SyncClient {
    pub client: Client,
    pub project: String,
}

/// The answer to a publish.
#[derive(Debug, PartialEq)]
pub enum Published {
    Done(Value),
    /// Someone published after `after`: sync again.
    Stale(Value),
}

impl SyncClient {
    fn path(&self, rest: &str) -> String {
        format!("/v1/sync/{}/{rest}", crate::stripe::url_encode(&self.project))
    }

    pub fn describe(&self) -> String {
        format!("{} (project {})", self.client.base, self.project)
    }
    pub fn create(&self, name: &str) -> Result<(), String> {
        self.client.post(&self.path("create"), &json!({"name": name})).map(drop)
    }
    /// The latest head, or `None` before anyone published.
    pub fn head(&self) -> Result<Option<Value>, String> {
        Ok(Some(self.client.get(&self.path("head"))?).filter(|v| !v.is_null()))
    }
    pub fn fetch(&self, rev: u64) -> Result<Value, String> {
        self.client.get(&self.path(&format!("revs/{rev}")))
    }
    pub fn publish(&self, after: u64, project: &Value, by: &str, summary: &str) -> Result<Published, String> {
        let r = self.client.post(&self.path("publish"), &json!({"after": after, "project": project, "by": by, "summary": summary}))?;
        match (r.get("done"), r.get("stale")) {
            (Some(h), _) => Ok(Published::Done(h.clone())),
            (_, Some(h)) => Ok(Published::Stale(h.clone())),
            _ => Err("the server's answer to publish is not valid".into()),
        }
    }
    /// Lets another account into the project (by its email).
    pub fn invite(&self, email: &str) -> Result<(), String> {
        self.client.post(&self.path("invite"), &json!({"email": email})).map(drop)
    }
    pub fn join(&self, user: &str) -> Result<u32, String> {
        self.client.post(&self.path("join"), &json!({"user": user}))?.get("slot").and_then(Value::as_u64).map(|s| s as u32).ok_or_else(|| "no id block came back".into())
    }
    pub fn members(&self) -> Result<Value, String> {
        self.client.get(&self.path("members"))
    }
    pub fn notes(&self) -> Result<Value, String> {
        self.client.get(&self.path("notes"))
    }
    pub fn publish_notes(&self, user: &str, notes: &Value) -> Result<(), String> {
        self.client.post(&self.path("notes"), &json!({"user": user, "notes": notes})).map(drop)
    }
    pub fn team(&self) -> Result<Option<Value>, String> {
        Ok(Some(self.client.get(&self.path("team"))?).filter(|v| !v.is_null()))
    }
    pub fn publish_team(&self, team: &Value) -> Result<(), String> {
        self.client.post(&self.path("team"), &json!({"team": team})).map(drop)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(tag: &str) -> (Account, PathBuf) {
        let dir = std::env::temp_dir().join(format!("studio-acct-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        (Account::open(&dir, Box::new(FileSecrets { path: dir.join("secrets.json") })), dir)
    }

    #[test]
    fn nothing_is_sent_while_the_cloud_is_off() {
        let (a, dir) = account("off");
        let e = a.client().err().unwrap();
        assert!(e.contains("the cloud is off"), "{e}");
        assert!(a.sign_in("a@b.c", "x").unwrap_err().contains("the cloud is off"));
        assert!(a.market(None, None).is_err());
        // Offline, everything local still answers.
        let s = a.status();
        assert!(!s.cloud.enabled && !s.signed_in);
        assert_eq!(s.entitlements.plan.id, "free");
        assert!(a.quote("transcribe", 1.0, true).unwrap_err().contains("no credit prices"));
        assert!(a.connect("http://example.com", false).unwrap_err().contains("https"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_secrets_are_private_and_removable() {
        let (_, dir) = account("secrets");
        let s = FileSecrets { path: dir.join("secrets.json") };
        s.set("t", "abc").unwrap();
        assert_eq!(s.get("t").as_deref(), Some("abc"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&s.path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        s.remove("t");
        assert_eq!(s.get("t"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
