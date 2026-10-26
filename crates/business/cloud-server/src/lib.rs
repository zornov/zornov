// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A Studio cloud server anyone can run (`studio-cloud-server`):
//! accounts and signed licences, hosted AI credits (and a metered proxy to
//! AI providers), cloud renders, review links, project sync and the
//! marketplace, with payments through Stripe Checkout in test mode.
//!
//! It keeps everything in one data folder and holds the licence signing
//! key there (`issuer.pk8`). Studio trusts its licences only when the
//! person connecting chooses to (see `studio_commerce::Account::connect`).

pub mod http;
pub mod state;

use http::{Request, Response};
use ring::{digest, pbkdf2, rand::SecureRandom};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use state::*;
use std::collections::BTreeMap;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use studio_commerce::credits::PriceTable;
use studio_commerce::market::{self, Listing, Sale, Share};
use studio_commerce::stripe::{self, Stripe};
use studio_entitlements::{Catalogue, Issuer, Licence, hex, now, write_atomic};

/// A credit pack for sale.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pack {
    pub id: String,
    pub credits: u64,
    pub price_minor: u64,
    pub currency: String,
}

/// An AI provider the server forwards metered calls to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Upstream {
    /// `https://api.openai.com`.
    pub base: String,
    /// The header the key goes in, and what goes before it.
    #[serde(default = "authorization")]
    pub header: String,
    #[serde(default)]
    pub prefix: String,
    /// The environment variable holding the server's key.
    pub key_env: String,
}

fn authorization() -> String {
    "Authorization".into()
}

/// `server.json`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConfigFile {
    #[serde(default = "default_name")]
    pub name: String,
    /// How people reach this server (links in review pages and checkouts).
    #[serde(default)]
    pub public_url: String,
    /// A plan list file (with prices); the shipped plans when absent.
    #[serde(default)]
    pub plans: Option<PathBuf>,
    /// The credit price table file; hosted AI is off without one.
    #[serde(default)]
    pub prices: Option<PathBuf>,
    #[serde(default)]
    pub packs: Vec<Pack>,
    /// The creator's share of a marketplace sale, in basis points.
    #[serde(default = "creator_bps")]
    pub creator_bps: u32,
    /// The least margin over provider cost a price table may have.
    #[serde(default)]
    pub margin_bps: u64,
    #[serde(default = "licence_days")]
    pub licence_days: u64,
    #[serde(default = "grace_days")]
    pub grace_days: u32,
    #[serde(default = "stripe_api")]
    pub stripe_api: String,
    /// Runs a render: `{project}` and `{out}` are replaced by paths.
    #[serde(default)]
    pub render_command: Vec<String>,
    #[serde(default)]
    pub upstreams: BTreeMap<String, Upstream>,
    #[serde(default = "yes")]
    pub signup: bool,
    #[serde(default = "max_upload_mb")]
    pub max_upload_mb: usize,
}

fn default_name() -> String {
    "Studio cloud".into()
}
fn creator_bps() -> u32 {
    7_000
}
fn licence_days() -> u64 {
    35
}
fn grace_days() -> u32 {
    14
}
fn stripe_api() -> String {
    stripe::API.into()
}
fn yes() -> bool {
    true
}
fn max_upload_mb() -> usize {
    2_048
}

impl Default for ConfigFile {
    fn default() -> Self {
        serde_json::from_str("{}").expect("all fields have defaults")
    }
}

/// Everything the server runs with.
pub struct Config {
    pub data: PathBuf,
    pub file: ConfigFile,
    pub plans: Catalogue,
    pub prices: Option<PriceTable>,
    /// A Stripe test key (`sk_test_...`), read at start-up.
    pub stripe_key: Option<String>,
    pub webhook_secret: Option<String>,
}

impl Config {
    /// Reads `server.json` from `data` (defaults when there is none).
    pub fn load(data: &Path) -> Result<Config, String> {
        let file: ConfigFile = match std::fs::read_to_string(data.join("server.json")) {
            Ok(t) => serde_json::from_str(&t).map_err(|e| format!("server.json is not valid: {e}"))?,
            Err(_) => ConfigFile::default(),
        };
        let rel = |p: &Path| if p.is_absolute() { p.to_path_buf() } else { data.join(p) };
        let plans = match &file.plans {
            Some(p) => Catalogue::parse(&std::fs::read_to_string(rel(p)).map_err(|e| format!("cannot read the plans file: {e}"))?)?,
            None => Catalogue::builtin(),
        };
        let prices = match &file.prices {
            Some(p) => Some(PriceTable::parse(&std::fs::read_to_string(rel(p)).map_err(|e| format!("cannot read the prices file: {e}"))?)?),
            None => None,
        };
        Ok(Config { data: data.to_path_buf(), file, plans, prices, stripe_key: None, webhook_secret: None })
    }

    fn check(&self) -> Result<(), String> {
        if let Some(t) = &self.prices {
            let bad = t.uncovered(self.file.margin_bps);
            if !bad.is_empty() {
                return Err(format!("these prices do not cover the providers' costs:\n  {}", bad.join("\n  ")));
            }
        }
        if let Some(k) = &self.stripe_key {
            Stripe::test(k, &self.file.stripe_api)?;
        }
        Ok(())
    }
}

pub struct Server {
    /// Itself, for render workers that finish after the request.
    me: std::sync::Weak<Server>,
    cfg: Config,
    issuer: Issuer,
    state: Mutex<State>,
    /// Serialises writes of `state.json`.
    saving: Mutex<()>,
}

/// A running server.
pub struct Running {
    pub addr: std::net::SocketAddr,
    pub server: Arc<Server>,
    stop: Arc<AtomicBool>,
}

impl Running {
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = std::net::TcpStream::connect(self.addr);
    }
}

/// Starts serving on `addr` (`127.0.0.1:0` picks a free port).
pub fn start(mut cfg: Config, addr: &str) -> Result<Running, String> {
    cfg.check()?;
    std::fs::create_dir_all(&cfg.data).map_err(|e| format!("cannot make {}: {e}", cfg.data.display()))?;
    let key_path = cfg.data.join("issuer.pk8");
    let pkcs8 = match std::fs::read(&key_path) {
        Ok(k) => k,
        Err(_) => {
            let k = Issuer::generate()?;
            write_atomic(&key_path, &k)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600));
            }
            k
        }
    };
    let issuer = Issuer::from_pkcs8(&pkcs8)?;
    let state: State = match std::fs::read_to_string(cfg.data.join("state.json")) {
        Ok(t) => serde_json::from_str(&t).map_err(|e| format!("state.json is not valid: {e}"))?,
        Err(_) => State::default(),
    };
    let listener = std::net::TcpListener::bind(addr).map_err(|e| format!("cannot listen on {addr}: {e}"))?;
    let local = listener.local_addr().map_err(|e| e.to_string())?;
    if cfg.file.public_url.is_empty() {
        cfg.file.public_url = format!("http://{local}");
    }
    let max = cfg.file.max_upload_mb << 20;
    let server = Arc::new_cyclic(|me| Server { me: me.clone(), cfg, issuer, state: Mutex::new(state), saving: Mutex::new(()) });
    let stop = Arc::new(AtomicBool::new(false));
    let (s, st) = (server.clone(), stop.clone());
    std::thread::spawn(move || http::serve(listener, Arc::new(move |r| s.handle(r)), st, max));
    Ok(Running { addr: local, server, stop })
}

// ------------------------------------------------------------------ helpers

fn random_hex(n: usize) -> String {
    let mut b = vec![0u8; n];
    ring::rand::SystemRandom::new().fill(&mut b).expect("the system has randomness");
    hex(&b)
}

fn sha(s: &[u8]) -> String {
    hex(digest::digest(&digest::SHA256, s).as_ref())
}

const ITER: NonZeroU32 = NonZeroU32::new(120_000).unwrap();

fn hash_password(salt: &[u8], pw: &str) -> Vec<u8> {
    let mut out = [0u8; 32];
    pbkdf2::derive(pbkdf2::PBKDF2_HMAC_SHA256, ITER, salt, pw.as_bytes(), &mut out);
    out.to_vec()
}

fn body(req: &Request) -> Result<Value, Response> {
    if req.body.is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_slice(&req.body).map_err(|_| Response::error(400, "the body is not JSON"))
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&#39;")
}

type R = Result<Response, Response>;

fn ok(v: Value) -> R {
    Ok(Response::json(200, &v))
}

fn page(title: &str, inner: &str) -> String {
    format!(
        "<!doctype html><html><head><meta charset=utf-8><meta name=viewport content=\"width=device-width,initial-scale=1\"><title>{}</title><style>body{{font:16px/1.5 -apple-system,system-ui,sans-serif;max-width:860px;margin:2rem auto;padding:0 16px;color:#1d1d1f;background:#fafafa}}video,img{{width:100%;border-radius:8px;background:#000}}li{{margin:.3rem 0}}input,textarea,button{{font:inherit;padding:.4rem}}textarea{{width:100%}}@media(prefers-color-scheme:dark){{body{{background:#161617;color:#f5f5f7}}}}</style></head><body>{inner}</body></html>",
        esc(title)
    )
}

impl Server {
    pub fn issuer_public_key(&self) -> String {
        self.issuer.public_key_hex()
    }

    /// Runs `f` on the state and saves it afterwards.
    fn change<T>(&self, f: impl FnOnce(&mut State) -> T) -> T {
        // The save lock first, so snapshots reach the disk in the order
        // they were taken.
        let _g = self.saving.lock().expect("save lock");
        let (out, snapshot) = {
            let mut st = self.state.lock().expect("state lock");
            let out = f(&mut st);
            (out, serde_json::to_vec(&*st).expect("serialisable"))
        };
        if let Err(e) = write_atomic(&self.cfg.data.join("state.json"), &snapshot) {
            eprintln!("studio-cloud-server: {e}");
        }
        out
    }

    fn read<T>(&self, f: impl FnOnce(&State) -> T) -> T {
        f(&self.state.lock().expect("state lock"))
    }

    fn user(&self, req: &Request) -> Result<String, Response> {
        let t = req.header("authorization").and_then(|h| h.strip_prefix("Bearer ")).ok_or_else(|| Response::error(401, "sign in first"))?;
        self.read(|st| st.sessions.get(&sha(t.trim().as_bytes())).cloned()).ok_or_else(|| Response::error(401, "the session has ended: sign in again"))
    }

    /// The features of `email`'s current plan.
    fn allows(&self, st: &State, email: &str, feature: &str) -> bool {
        let plan = st.users.get(email).map(|u| self.effective_plan(u)).unwrap_or_else(|| "free".into());
        self.cfg.plans.plan(&plan).is_some_and(|p| p.features.iter().any(|f| f == feature))
    }

    fn effective_plan(&self, u: &User) -> String {
        match u.plan_expires {
            Some(e) if now() >= e + self.cfg.file.grace_days as u64 * 86_400 => "free".into(),
            _ => u.plan.clone(),
        }
    }

    fn need(&self, email: &str, feature: &str) -> Result<(), Response> {
        if self.read(|st| self.allows(st, email, feature)) {
            Ok(())
        } else {
            Err(Response::error(402, format!("{} is not on your plan", studio_entitlements::describe(feature))))
        }
    }

    fn licence_for(&self, u: &User) -> Option<Value> {
        let plan = self.effective_plan(u);
        if plan == "free" {
            return None;
        }
        let expires = u.plan_expires.map_or(now() + self.cfg.file.licence_days * 86_400, |e| e.min(now() + self.cfg.file.licence_days * 86_400).max(now()));
        let seats = self.cfg.plans.plan(&plan).map_or(1, |p| p.seats);
        let l = Licence { schema: 1, id: format!("L-{}", &sha(format!("{}:{plan}", u.email).as_bytes())[..12]), plan, holder: u.name.clone(), email: u.email.clone(), seats, issued: now(), expires: Some(expires), grace_days: self.cfg.file.grace_days, add: vec![], remove: vec![], monthly_credits: None, key_id: String::new(), server: self.cfg.file.public_url.clone() };
        serde_json::to_value(self.issuer.sign(&l)).ok()
    }

    fn stripe(&self) -> Result<Stripe, Response> {
        let key = self.cfg.stripe_key.as_deref().ok_or_else(|| Response::error(503, "payments are not set up on this server (no Stripe test key)"))?;
        Stripe::test(key, &self.cfg.file.stripe_api).map_err(|e| Response::error(503, e))
    }

    // ------------------------------------------------------------- routing

    pub fn handle(&self, req: Request) -> Response {
        let seg: Vec<String> = req.path.split('/').filter(|p| !p.is_empty()).map(stripe::url_decode).collect();
        let seg: Vec<&str> = seg.iter().map(String::as_str).collect();
        let r = match (req.method.as_str(), seg.as_slice()) {
            ("GET", []) => Ok(Response::html(200, page(&self.cfg.file.name, &format!("<h1>{}</h1><p>A Studio cloud server. Point Studio at <code>{}</code> in Settings ▸ Account.</p>", esc(&self.cfg.file.name), esc(&self.cfg.file.public_url))))),
            ("GET", ["v1", "issuer"]) => ok(json!({"name": self.cfg.file.name, "public_key": self.issuer.public_key_hex(), "key_id": self.issuer.key_id()})),
            ("GET", ["v1", "plans"]) => ok(serde_json::to_value(&self.cfg.plans).expect("serialisable")),
            ("GET", ["v1", "prices"]) => match &self.cfg.prices {
                Some(t) => ok(serde_json::to_value(t).expect("serialisable")),
                None => Err(Response::error(404, "this server has no hosted AI")),
            },
            ("GET", ["v1", "credits", "packs"]) => ok(json!(self.cfg.file.packs)),
            ("POST", ["v1", "auth", "signup"]) => self.signup(&req),
            ("POST", ["v1", "auth", "signin"]) => self.signin(&req),
            ("POST", ["v1", "auth", "signout"]) => self.signout(&req),
            ("GET", ["v1", "me"]) => self.me(&req),
            ("POST", ["v1", "credits", "reserve"]) => self.reserve(&req),
            ("POST", ["v1", "credits", "commit"]) => self.commit(&req),
            ("POST", ["v1", "credits", "release"]) => self.release(&req),
            ("GET", ["v1", "credits", "usage"]) => self.usage(&req),
            ("POST", ["v1", "ai", provider, rest @ ..]) => self.proxy(&req, provider, rest),
            ("POST", ["v1", "billing", "checkout"]) => self.checkout(&req),
            ("POST", ["v1", "billing", "confirm"]) => self.confirm(&req),
            ("POST", ["v1", "stripe", "webhook"]) => self.webhook(&req),
            ("GET", ["billing", "done"]) => self.done_page(&req),
            ("GET", ["billing", "cancelled"]) => Ok(Response::html(200, page("Cancelled", "<h1>Payment cancelled</h1><p>Nothing was charged. You can close this page.</p>"))),
            ("GET", ["v1", "receipts"]) => self.receipts(&req),
            ("POST", ["v1", "render", "jobs"]) => self.render_submit(&req),
            ("GET", ["v1", "render", "jobs", id]) => self.render_status(&req, id),
            ("GET", ["v1", "render", "jobs", id, "output"]) => self.render_output(&req, id),
            ("POST", ["v1", "review"]) => self.review_create(&req),
            ("GET", ["v1", "review", token, "comments"]) => self.review_notes(token),
            ("GET", ["r", token]) => self.review_page(token),
            ("GET", ["r", token, "media"]) => self.review_media(token),
            ("POST", ["r", token, "comments"]) => self.review_comment(&req, token),
            ("POST", ["v1", "sync", project, "create"]) => self.sync_create(&req, project),
            (m, ["v1", "sync", project, rest @ ..]) => self.sync(&req, m, project, rest),
            ("GET", ["v1", "market"]) => self.market(&req),
            ("GET", ["v1", "market", "owned"]) => self.owned(&req),
            ("POST", ["v1", "market"]) => self.list(&req),
            ("GET", ["v1", "market", id]) => self.listing(id),
            ("POST", ["v1", "market", id, "file"]) => self.listing_file(&req, id),
            ("GET", ["v1", "market", id, "download"]) => self.download(&req, id),
            ("GET", ["v1", "creator", "statement"]) => self.statement(&req),
            _ => Err(Response::error(404, "no such endpoint")),
        };
        r.unwrap_or_else(|e| e)
    }

    // ------------------------------------------------------------ accounts

    fn session(&self, email: &str) -> Value {
        let token = random_hex(32);
        self.change(|st| st.sessions.insert(sha(token.as_bytes()), email.to_owned()));
        json!({"token": token, "email": email})
    }

    fn signup(&self, req: &Request) -> R {
        if !self.cfg.file.signup {
            return Err(Response::error(403, "this server does not take new accounts"));
        }
        let b = body(req)?;
        let email = s(&b, "email").trim().to_ascii_lowercase();
        let pw = s(&b, "password");
        if !email.contains('@') || email.len() > 200 {
            return Err(Response::error(400, "that is not an email address"));
        }
        if pw.chars().count() < 8 {
            return Err(Response::error(400, "a password needs 8 characters or more"));
        }
        let salt = random_hex(16);
        let user = User { email: email.clone(), name: if s(&b, "name").is_empty() { email.clone() } else { s(&b, "name").chars().take(100).collect() }, hash: hex(&hash_password(salt.as_bytes(), pw)), salt, plan: "free".into(), plan_expires: None, balance: 0, created: now() };
        let added = self.change(|st| {
            if st.users.contains_key(&email) {
                return false;
            }
            st.users.insert(email.clone(), user);
            true
        });
        if !added {
            return Err(Response::error(409, "there is already an account with that email; sign in instead"));
        }
        ok(self.session(&email))
    }

    fn signin(&self, req: &Request) -> R {
        let b = body(req)?;
        let email = s(&b, "email").trim().to_ascii_lowercase();
        let u = self.read(|st| st.users.get(&email).cloned());
        let good = u.as_ref().is_some_and(|u| studio_entitlements::unhex(&u.hash).is_some_and(|h| pbkdf2::verify(pbkdf2::PBKDF2_HMAC_SHA256, ITER, u.salt.as_bytes(), s(&b, "password").as_bytes(), &h).is_ok()));
        if !good {
            // The same work either way, so timing does not tell which part was wrong.
            if u.is_none() {
                let _ = hash_password(b"x", s(&b, "password"));
            }
            return Err(Response::error(401, "the email or password is wrong"));
        }
        ok(self.session(&email))
    }

    fn signout(&self, req: &Request) -> R {
        if let Some(t) = req.header("authorization").and_then(|h| h.strip_prefix("Bearer ")) {
            let h = sha(t.trim().as_bytes());
            self.change(|st| st.sessions.remove(&h));
        }
        ok(json!({"signed_out": true}))
    }

    fn me(&self, req: &Request) -> R {
        let email = self.user(req)?;
        let (u, balance) = self.read(|st| (st.users.get(&email).cloned(), st.available(&email)));
        let u = u.ok_or_else(|| Response::error(401, "no such account"))?;
        ok(json!({"email": u.email, "name": u.name, "plan": self.effective_plan(&u), "plan_expires": u.plan_expires, "balance": balance, "licence": self.licence_for(&u)}))
    }

    // ------------------------------------------------------------- credits

    fn credits_for(&self, task: &str, units: f64) -> Result<u64, Response> {
        let t = self.cfg.prices.as_ref().ok_or_else(|| Response::error(404, "this server has no hosted AI"))?;
        let p = t.task(task).ok_or_else(|| Response::error(400, format!("no price for \"{task}\"")))?;
        if !(units.is_finite() && units >= 0.0) {
            return Err(Response::error(400, "units must be 0 or more"));
        }
        Ok(((units * p.credits as f64).ceil() as u64).max(if units > 0.0 { 1 } else { 0 }))
    }

    fn hold(&self, email: &str, task: &str, units: f64) -> Result<(String, u64, i64), Response> {
        self.need(email, "ai.hosted")?;
        let credits = self.credits_for(task, units)?;
        self.change(|st| {
            let avail = st.available(email);
            if credits as i64 > avail {
                return Err(Response::error(402, format!("you have {avail} credits; this needs {credits}")));
            }
            let id = st.id("H");
            st.holds.insert(id.clone(), Hold { email: email.into(), task: task.into(), credits, at: now() });
            Ok((id, credits, avail - credits as i64))
        })
    }

    fn settle(&self, email: &str, hold: &str, units: Option<f64>) -> Result<(u64, i64), Response> {
        let h = self.read(|st| st.holds.get(hold).cloned()).filter(|h| h.email == email).ok_or_else(|| Response::error(404, "no such hold"))?;
        // Never more than was held.
        let charge = match units {
            Some(u) => self.credits_for(&h.task, u)?.min(h.credits),
            None => 0,
        };
        Ok(self.change(|st| {
            st.holds.remove(hold);
            if charge > 0 {
                if let Some(u) = st.users.get_mut(email) {
                    u.balance -= charge as i64;
                }
                st.usage.push(UsageRow { email: email.into(), at: now(), task: h.task.clone(), units: units.unwrap_or(0.0), credits: charge });
            }
            (charge, st.available(email))
        }))
    }

    fn reserve(&self, req: &Request) -> R {
        let email = self.user(req)?;
        let b = body(req)?;
        let (hold, credits, after) = self.hold(&email, s(&b, "task"), b.get("units").and_then(Value::as_f64).unwrap_or(1.0))?;
        ok(json!({"hold": hold, "credits": credits, "balance": after}))
    }

    fn commit(&self, req: &Request) -> R {
        let email = self.user(req)?;
        let b = body(req)?;
        let (credits, balance) = self.settle(&email, s(&b, "hold"), Some(b.get("units").and_then(Value::as_f64).unwrap_or(0.0)))?;
        ok(json!({"credits": credits, "balance": balance}))
    }

    fn release(&self, req: &Request) -> R {
        let email = self.user(req)?;
        let b = body(req)?;
        let (_, balance) = self.settle(&email, s(&b, "hold"), None)?;
        ok(json!({"balance": balance}))
    }

    fn usage(&self, req: &Request) -> R {
        let email = self.user(req)?;
        ok(json!(self.read(|st| st.usage.iter().filter(|u| u.email == email).cloned().collect::<Vec<_>>())))
    }

    /// Forwards a call to an AI provider with the server's key, charging
    /// the credits for `x-studio-task` × `x-studio-units` on success.
    fn proxy(&self, req: &Request, provider: &str, rest: &[&str]) -> R {
        let email = self.user(req)?;
        let up = self.cfg.file.upstreams.get(provider).cloned().ok_or_else(|| Response::error(404, format!("this server does not host {provider}")))?;
        let key = std::env::var(&up.key_env).ok().filter(|k| !k.is_empty()).ok_or_else(|| Response::error(503, format!("the server has no {provider} key")))?;
        let task = req.header("x-studio-task").unwrap_or("").to_owned();
        let units: f64 = req.header("x-studio-units").and_then(|u| u.parse().ok()).unwrap_or(1.0);
        let (hold, _, _) = self.hold(&email, &task, units)?;
        let auth = format!("{}{key}", up.prefix);
        let ct = req.header("content-type").unwrap_or("application/json").to_owned();
        let url = format!("{}/{}", up.base.trim_end_matches('/'), rest.iter().map(|p| stripe::url_encode(p)).collect::<Vec<_>>().join("/"));
        let reply = studio_net::post(&url, &[(up.header.as_str(), auth.as_str()), ("Content-Type", ct.as_str())], &req.body, std::time::Duration::from_secs(300));
        match reply {
            Ok(r) if (200..300).contains(&r.status) => {
                let ct = r.content_type.clone().unwrap_or_else(|| "application/json".into());
                let b = r.bytes(512 << 20).map_err(|e| Response::error(502, e.to_string()))?;
                self.settle(&email, &hold, Some(units))?;
                Ok(Response::bytes(&ct, b))
            }
            Ok(r) => {
                let status = r.status;
                let b = r.bytes(1 << 20).unwrap_or_default();
                self.settle(&email, &hold, None)?;
                Ok(Response { status, content_type: "application/json".into(), body: b })
            }
            Err(e) => {
                self.settle(&email, &hold, None)?;
                Err(Response::error(502, e))
            }
        }
    }

    // ------------------------------------------------------------- billing

    fn checkout(&self, req: &Request) -> R {
        let email = self.user(req)?;
        let b = body(req)?;
        let (what, id) = (s(&b, "what"), s(&b, "id"));
        let (title, amount, currency) = match what {
            "plan" => {
                let p = self.cfg.plans.plan(id).ok_or_else(|| Response::error(404, "no such plan"))?;
                let price = p.prices.first().ok_or_else(|| Response::error(400, format!("{} has no price on this server", p.name)))?;
                (format!("Studio {} ({})", p.name, price.interval), price.amount_minor, price.currency.clone())
            }
            "credits" => {
                let p = self.cfg.file.packs.iter().find(|p| p.id == id).ok_or_else(|| Response::error(404, "no such credit pack"))?;
                (format!("{} Studio credits", p.credits), p.price_minor, p.currency.clone())
            }
            "listing" => {
                let l = self.read(|st| st.listings.get(id).cloned()).filter(|l| l.published).ok_or_else(|| Response::error(404, "no such listing"))?;
                (l.title, l.price_minor, l.currency)
            }
            _ => return Err(Response::error(400, "checkout is for a plan, credits or a listing")),
        };
        let order = self.change(|st| {
            let oid = st.id("O");
            let o = Order { id: oid.clone(), email: email.clone(), what: what.into(), item: id.into(), title: title.clone(), amount_minor: amount, currency: currency.clone(), session: String::new(), status: "open".into(), at: now() };
            st.orders.insert(oid, o.clone());
            o
        });
        if amount == 0 {
            self.fulfil(&order.id, "");
            return ok(json!({"order": order.id, "done": true}));
        }
        let stripe = self.stripe()?;
        let base = &self.cfg.file.public_url;
        let sess = stripe
            .checkout(&[stripe::Item { name: title, amount_minor: amount, currency, quantity: 1 }], &order.id, Some(&email), &format!("{base}/billing/done?session_id={{CHECKOUT_SESSION_ID}}"), &format!("{base}/billing/cancelled"))
            .map_err(|e| Response::error(502, e))?;
        self.change(|st| {
            if let Some(o) = st.orders.get_mut(&order.id) {
                o.session = sess.id.clone();
            }
        });
        ok(json!({"order": order.id, "session": sess.id, "url": sess.url}))
    }

    /// Checks a session with Stripe; fulfils its order once, when paid.
    fn check_session(&self, session: &str) -> Result<Value, Response> {
        let order = self.read(|st| st.orders.values().find(|o| o.session == session && !session.is_empty()).cloned()).ok_or_else(|| Response::error(404, "no order for that checkout"))?;
        if order.status == "paid" {
            return Ok(json!({"order": order.id, "status": "paid"}));
        }
        let sess = self.stripe()?.session(session).map_err(|e| Response::error(502, e))?;
        let matches = sess.client_reference_id.as_deref() == Some(order.id.as_str()) && sess.amount_total.is_none_or(|a| a == order.amount_minor);
        if sess.payment_status == "paid" && matches {
            self.fulfil(&order.id, session);
            Ok(json!({"order": order.id, "status": "paid"}))
        } else {
            Ok(json!({"order": order.id, "status": if matches { sess.payment_status } else { "mismatch".into() }}))
        }
    }

    fn confirm(&self, req: &Request) -> R {
        let email = self.user(req)?;
        let b = body(req)?;
        let sess = s(&b, "session");
        if self.read(|st| st.orders.values().any(|o| o.session == sess && o.email != email)) {
            return Err(Response::error(404, "no order for that checkout"));
        }
        ok(self.check_session(sess)?)
    }

    fn done_page(&self, req: &Request) -> R {
        let sess = req.query("session_id").unwrap_or("");
        let msg = match self.check_session(sess) {
            Ok(v) if s(&v, "status") == "paid" => "Thank you: the payment went through. Go back to Studio; it picks this up when you press Check payment (or on its own next time it refreshes).".to_owned(),
            Ok(v) => format!("The payment is {}. If you finished paying, give it a moment and reload.", esc(s(&v, "status"))),
            Err(_) => "That checkout is not known here.".to_owned(),
        };
        Ok(Response::html(200, page("Payment", &format!("<h1>Payment</h1><p>{msg}</p><p><small>Test mode: no real money moves.</small></p>"))))
    }

    fn webhook(&self, req: &Request) -> R {
        let secret = self.cfg.webhook_secret.as_deref().ok_or_else(|| Response::error(503, "no webhook secret is set"))?;
        stripe::verify_webhook(&req.body, req.header("stripe-signature").unwrap_or(""), secret, now(), 300).map_err(|e| Response::error(400, e))?;
        let ev: Value = serde_json::from_slice(&req.body).map_err(|_| Response::error(400, "not JSON"))?;
        if ev.get("livemode").and_then(Value::as_bool) == Some(true) {
            return Err(Response::error(400, "live-mode events are refused"));
        }
        if s(&ev, "type") == "checkout.session.completed" {
            let id = ev.pointer("/data/object/id").and_then(Value::as_str).unwrap_or("");
            // Read back from Stripe rather than trusting the event's body.
            let _ = self.check_session(id);
        }
        ok(json!({"received": true}))
    }

    /// Gives the buyer what they paid for (once).
    fn fulfil(&self, order: &str, session: &str) {
        let plans = &self.cfg.plans;
        let packs = &self.cfg.file.packs;
        let share = Share { creator_bps: self.cfg.file.creator_bps };
        self.change(|st| {
            let Some(o) = st.orders.get(order).cloned() else { return };
            if o.status == "paid" {
                return;
            }
            match o.what.as_str() {
                "plan" => {
                    if let (Some(p), Some(u)) = (plans.plan(&o.item), st.users.get_mut(&o.email)) {
                        let days = match p.prices.first().map(|x| x.interval.as_str()) {
                            Some("year") => 366,
                            Some("once") => 36_500,
                            _ => 31,
                        };
                        let from = u.plan_expires.filter(|e| *e > now() && u.plan == p.id).unwrap_or(now());
                        u.plan = p.id.clone();
                        u.plan_expires = Some(from + days * 86_400);
                        u.balance += p.monthly_credits as i64;
                    }
                }
                "credits" => {
                    if let (Some(p), Some(u)) = (packs.iter().find(|p| p.id == o.item), st.users.get_mut(&o.email)) {
                        u.balance += p.credits as i64;
                    }
                }
                "listing" => {
                    st.owned.entry(o.email.clone()).or_default().insert(o.item.clone());
                    if let Some(l) = st.listings.get(&o.item) {
                        let (c, p) = market::split(o.amount_minor, share);
                        st.sales.push(Sale { order: o.id.clone(), listing: l.id.clone(), creator: l.creator.clone(), buyer: o.email.clone(), gross_minor: o.amount_minor, creator_minor: c, platform_minor: p, currency: o.currency.clone(), at: now() });
                    }
                }
                _ => {}
            }
            st.receipts.push(Receipt { order: o.id.clone(), email: o.email.clone(), what: o.what.clone(), title: o.title.clone(), amount_minor: o.amount_minor, currency: o.currency.clone(), at: now(), session: session.into() });
            if let Some(o) = st.orders.get_mut(order) {
                o.status = "paid".into();
            }
        });
    }

    fn receipts(&self, req: &Request) -> R {
        let email = self.user(req)?;
        ok(json!(self.read(|st| st.receipts.iter().filter(|r| r.email == email).cloned().collect::<Vec<_>>())))
    }

    // -------------------------------------------------------------- render

    fn render_submit(&self, req: &Request) -> R {
        let email = self.user(req)?;
        self.need(&email, "cloud.render")?;
        let b = body(req)?;
        let project = b.get("project").filter(|p| p.is_object()).ok_or_else(|| Response::error(400, "send the project"))?;
        let format = b.pointer("/settings/format").and_then(Value::as_str).filter(|f| f.chars().all(|c| c.is_ascii_alphanumeric()) && !f.is_empty()).unwrap_or("mp4").to_owned();
        let job = self.change(|st| {
            let id = st.id("J");
            let j = Job { id: id.clone(), email: email.clone(), name: s(&b, "name").chars().take(200).collect(), status: "queued".into(), error: String::new(), at: now(), output: None, format: format.clone() };
            st.jobs.insert(id, j.clone());
            j
        });
        let dir = self.cfg.data.join("render").join(&job.id);
        write_atomic(&dir.join("project.json"), serde_json::to_string_pretty(project).expect("serialisable").as_bytes()).map_err(|e| Response::error(500, e))?;
        write_atomic(&dir.join("settings.json"), b.get("settings").cloned().unwrap_or(json!({})).to_string().as_bytes()).map_err(|e| Response::error(500, e))?;
        if !self.cfg.file.render_command.is_empty() {
            self.run_render(job.id.clone(), dir, format);
        }
        ok(serde_json::to_value(job).expect("serialisable"))
    }

    fn run_render(&self, id: String, dir: PathBuf, format: String) {
        let out = dir.join(format!("output.{format}"));
        let args: Vec<String> = self.cfg.file.render_command.iter().map(|a| a.replace("{project}", &dir.join("project.json").display().to_string()).replace("{dir}", &dir.display().to_string()).replace("{out}", &out.display().to_string())).collect();
        let set = move |srv: &Server, status: &str, error: String, output: Option<String>| {
            srv.change(|st| {
                if let Some(j) = st.jobs.get_mut(&id) {
                    j.status = status.into();
                    j.error = error;
                    j.output = output;
                }
            })
        };
        set(self, "running", String::new(), None);
        let me = self.me.clone();
        std::thread::spawn(move || {
            let r = std::process::Command::new(&args[0]).args(&args[1..]).stdin(std::process::Stdio::null()).output();
            let Some(srv) = me.upgrade() else { return };
            match r {
                Ok(o) if o.status.success() && out.exists() => set(&srv, "done", String::new(), Some(out.display().to_string())),
                Ok(o) => {
                    let err = String::from_utf8_lossy(&o.stderr);
                    let tail: String = err.chars().skip(err.chars().count().saturating_sub(2000)).collect();
                    set(&srv, "failed", if tail.trim().is_empty() { "the render command made no output".into() } else { tail }, None)
                }
                Err(e) => set(&srv, "failed", format!("the render command did not start: {e}"), None),
            }
        });
    }

    fn render_status(&self, req: &Request, id: &str) -> R {
        let email = self.user(req)?;
        let j = self.read(|st| st.jobs.get(id).cloned()).filter(|j| j.email == email).ok_or_else(|| Response::error(404, "no such render"))?;
        ok(json!({"id": j.id, "name": j.name, "status": j.status, "error": j.error, "format": j.format}))
    }

    fn render_output(&self, req: &Request, id: &str) -> R {
        let email = self.user(req)?;
        let j = self.read(|st| st.jobs.get(id).cloned()).filter(|j| j.email == email).ok_or_else(|| Response::error(404, "no such render"))?;
        let path = j.output.filter(|_| j.status == "done").ok_or_else(|| Response::error(409, format!("the render is {}", j.status)))?;
        let b = std::fs::read(&path).map_err(|e| Response::error(500, e.to_string()))?;
        Ok(Response::bytes("application/octet-stream", b))
    }

    // -------------------------------------------------------------- review

    fn review_create(&self, req: &Request) -> R {
        let email = self.user(req)?;
        self.need(&email, "cloud.review")?;
        if req.body.is_empty() {
            return Err(Response::error(400, "send the file to review"));
        }
        let token = random_hex(16);
        let name = market::safe_name(req.query("name").unwrap_or("file"));
        let file = self.cfg.data.join("review").join(&token).join(&name);
        write_atomic(&file, &req.body).map_err(|e| Response::error(500, e))?;
        let ct = req.header("content-type").unwrap_or("application/octet-stream");
        let ct = if ct.starts_with("video/") || ct.starts_with("image/") || ct.starts_with("audio/") { ct } else { "application/octet-stream" }.to_owned();
        let title: String = req.query("title").unwrap_or(&name).chars().take(200).collect();
        self.change(|st| st.reviews.insert(token.clone(), Review { email, title, file: file.display().to_string(), content_type: ct, created: now(), notes: vec![] }));
        ok(json!({"token": token, "url": format!("{}/r/{token}", self.cfg.file.public_url)}))
    }

    fn review(&self, token: &str) -> Result<Review, Response> {
        self.read(|st| st.reviews.get(token).cloned()).ok_or_else(|| Response::error(404, "no such review link"))
    }

    fn review_notes(&self, token: &str) -> R {
        ok(json!(self.review(token)?.notes))
    }

    fn review_page(&self, token: &str) -> R {
        let r = self.review(token)?;
        let media = if r.content_type.starts_with("image/") {
            format!("<img src=\"/r/{token}/media\" alt=\"\">")
        } else if r.content_type.starts_with("audio/") {
            format!("<audio controls src=\"/r/{token}/media\" id=v></audio>")
        } else {
            format!("<video controls playsinline src=\"/r/{token}/media\" id=v></video>")
        };
        let notes: String = r.notes.iter().map(|n| format!("<li><b>{}</b> at {:.1}s: {}</li>", esc(&n.name), n.at, esc(&n.note))).collect();
        let inner = format!(
            "<h1>{}</h1>{media}<h2>Notes</h2><ul id=notes>{notes}</ul><form id=f><input id=n placeholder=\"Your name\" required> <textarea id=t rows=3 placeholder=\"A note at the current time\" required></textarea><button>Add note</button></form>\
             <script>document.getElementById('f').onsubmit=async e=>{{e.preventDefault();const v=document.getElementById('v');const at=v?v.currentTime:0;\
             const r=await fetch(location.pathname+'/comments',{{method:'POST',headers:{{'Content-Type':'application/json'}},body:JSON.stringify({{name:n.value,at,note:t.value}})}});if(r.ok)location.reload();}};</script>",
            esc(&r.title)
        );
        Ok(Response::html(200, page(&r.title, &inner)))
    }

    fn review_media(&self, token: &str) -> R {
        let r = self.review(token)?;
        let b = std::fs::read(&r.file).map_err(|e| Response::error(500, e.to_string()))?;
        Ok(Response::bytes(&r.content_type, b))
    }

    fn review_comment(&self, req: &Request, token: &str) -> R {
        self.review(token)?;
        let b = body(req)?;
        let note = ReviewNote { name: s(&b, "name").chars().take(80).collect(), at: b.get("at").and_then(Value::as_f64).unwrap_or(0.0).max(0.0), note: s(&b, "note").chars().take(4000).collect(), when: now() };
        if note.note.trim().is_empty() || note.name.trim().is_empty() {
            return Err(Response::error(400, "a note needs a name and some words"));
        }
        let added = self.change(|st| {
            let r = st.reviews.get_mut(token)?;
            if r.notes.len() >= 2_000 {
                return None;
            }
            r.notes.push(note);
            Some(r.notes.len())
        });
        added.map(|n| Response::json(200, &json!({"notes": n}))).ok_or_else(|| Response::error(409, "this link has too many notes"))
    }

    // ---------------------------------------------------------------- sync

    fn rev_path(&self, project: &str, rev: u64) -> PathBuf {
        self.cfg.data.join("sync").join(market::safe_name(project)).join("revs").join(format!("{rev}.json"))
    }

    fn sync_create(&self, req: &Request, project: &str) -> R {
        let email = self.user(req)?;
        self.need(&email, "cloud.sync")?;
        if market::safe_name(project) != project {
            return Err(Response::error(400, "a project name is letters, digits, spaces, - _ and ."));
        }
        let b = body(req)?;
        self.change(|st| {
            let sp = st.spaces.entry(project.to_owned()).or_insert_with(|| Space { name: s(&b, "name").into(), owner: email.clone(), ..Default::default() });
            if sp.owner != email && !sp.accounts.contains(&email) {
                return Err(Response::error(403, "that project belongs to someone else"));
            }
            sp.accounts.insert(email.clone());
            Ok(Response::json(200, &json!({"created": project})))
        })
    }

    fn sync(&self, req: &Request, method: &str, project: &str, rest: &[&str]) -> R {
        let email = self.user(req)?;
        self.need(&email, "cloud.sync")?;
        let sp = self.read(|st| st.spaces.get(project).cloned()).ok_or_else(|| Response::error(404, "that project is not shared here yet"))?;
        // Only accounts the owner (or a member) invited.
        if !sp.accounts.contains(&email) {
            return Err(Response::error(403, "you are not in this project: ask someone in it to invite you"));
        }
        match (method, rest) {
            ("POST", ["invite"]) => {
                let b = body(req)?;
                let who = s(&b, "email").trim().to_ascii_lowercase();
                if !who.contains('@') {
                    return Err(Response::error(400, "invite needs an email"));
                }
                self.change(|st| st.spaces.get_mut(project).map(|sp| sp.accounts.insert(who.clone())));
                ok(json!({"invited": who}))
            }
            ("GET", ["head"]) => ok(sp.head.unwrap_or(Value::Null)),
            ("GET", ["revs", n]) => {
                let n: u64 = n.parse().map_err(|_| Response::error(400, "not a revision"))?;
                let t = std::fs::read(self.rev_path(project, n)).map_err(|_| Response::error(404, "no such revision"))?;
                Ok(Response::bytes("application/json", t))
            }
            ("POST", ["publish"]) => {
                let b = body(req)?;
                let after = b.get("after").and_then(Value::as_u64).unwrap_or(0);
                let p = b.get("project").ok_or_else(|| Response::error(400, "send the project"))?;
                let bytes = serde_json::to_vec(p).expect("serialisable");
                let fingerprint = sha(&bytes)[..16].to_owned();
                // The check and the write happen under one lock: atomic.
                let mut st = self.state.lock().expect("state lock");
                let sp = st.spaces.get_mut(project).ok_or_else(|| Response::error(404, "gone"))?;
                let cur = sp.head.as_ref().and_then(|h| h.get("rev")).and_then(Value::as_u64).unwrap_or(0);
                if cur != after {
                    return ok(json!({"stale": sp.head}));
                }
                let rev = cur + 1;
                write_atomic(&self.rev_path(project, rev), &bytes).map_err(|e| Response::error(500, e))?;
                let by = if s(&b, "by").is_empty() { email.clone() } else { s(&b, "by").to_owned() };
                let head = json!({"rev": rev, "by": by, "at": now() * 1000, "fingerprint": fingerprint});
                sp.head = Some(head.clone());
                sp.log.push(json!({"rev": rev, "by": by, "at": now() * 1000, "summary": s(&b, "summary")}));
                sp.accounts.insert(email.clone());
                drop(st);
                self.change(|_| ());
                ok(json!({"done": head}))
            }
            ("POST", ["join"]) => {
                let b = body(req)?;
                let user = s(&b, "user").to_owned();
                if user.is_empty() {
                    return Err(Response::error(400, "join needs a user"));
                }
                ok(self.change(|st| {
                    let sp = st.spaces.get_mut(project).expect("checked");
                    sp.accounts.insert(email.clone());
                    let next = sp.members.values().map(|v| v + 1).max().unwrap_or(0);
                    let slot = *sp.members.entry(user).or_insert(next);
                    json!({"slot": slot})
                }))
            }
            ("GET", ["members"]) => ok(json!(sp.members)),
            ("GET", ["log"]) => ok(json!(sp.log)),
            ("GET", ["notes"]) => ok(json!(sp.notes.iter().map(|(u, n)| json!([u, n])).collect::<Vec<_>>())),
            ("POST", ["notes"]) => {
                let b = body(req)?;
                let user = s(&b, "user").to_owned();
                let notes = b.get("notes").cloned().unwrap_or(Value::Null);
                self.change(|st| st.spaces.get_mut(project).map(|sp| sp.notes.insert(user, notes)));
                ok(json!({"saved": true}))
            }
            ("GET", ["team"]) => ok(sp.team.unwrap_or(Value::Null)),
            ("POST", ["team"]) => {
                let b = body(req)?;
                self.change(|st| st.spaces.get_mut(project).map(|sp| sp.team = b.get("team").cloned()));
                ok(json!({"saved": true}))
            }
            _ => Err(Response::error(404, "no such sync call")),
        }
    }

    // ---------------------------------------------------------- marketplace

    fn market(&self, req: &Request) -> R {
        let kind = req.query("kind").unwrap_or("");
        let q = req.query("q").unwrap_or("").to_lowercase();
        let v: Vec<Listing> = self.read(|st| {
            st.listings
                .values()
                .filter(|l| l.published && (kind.is_empty() || l.kind == kind))
                .filter(|l| q.is_empty() || l.title.to_lowercase().contains(&q) || l.description.to_lowercase().contains(&q) || l.tags.iter().any(|t| t.to_lowercase() == q))
                .cloned()
                .collect()
        });
        ok(json!(v))
    }

    fn listing(&self, id: &str) -> R {
        let l = self.read(|st| st.listings.get(id).cloned()).filter(|l| l.published).ok_or_else(|| Response::error(404, "no such listing"))?;
        ok(serde_json::to_value(l).expect("serialisable"))
    }

    fn owned(&self, req: &Request) -> R {
        let email = self.user(req)?;
        ok(json!(self.read(|st| st.owned.get(&email).cloned().unwrap_or_default())))
    }

    fn list(&self, req: &Request) -> R {
        let email = self.user(req)?;
        self.need(&email, "market.sell")?;
        let b = body(req)?;
        let kind = s(&b, "kind");
        if !market::KINDS.contains(&kind) {
            return Err(Response::error(400, format!("a listing is one of: {}", market::KINDS.join(", "))));
        }
        let title: String = s(&b, "title").trim().chars().take(120).collect();
        if title.is_empty() {
            return Err(Response::error(400, "a listing needs a title"));
        }
        let currency = if s(&b, "currency").is_empty() { "usd".to_owned() } else { s(&b, "currency").to_ascii_lowercase() };
        let l = self.change(|st| {
            let id = st.id("M");
            let name = st.users.get(&email).map(|u| u.name.clone()).unwrap_or_default();
            let l = Listing { id: id.clone(), kind: kind.into(), title, description: s(&b, "description").chars().take(4000).collect(), creator: email.clone(), creator_name: name, price_minor: b.get("price_minor").and_then(Value::as_u64).unwrap_or(0), currency, file_name: String::new(), size: 0, sha256: String::new(), tags: b.get("tags").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).take(12).map(str::to_owned).collect()).unwrap_or_default(), published: false };
            st.listings.insert(id, l.clone());
            l
        });
        ok(serde_json::to_value(l).expect("serialisable"))
    }

    fn listing_file(&self, req: &Request, id: &str) -> R {
        let email = self.user(req)?;
        let l = self.read(|st| st.listings.get(id).cloned()).filter(|l| l.creator == email).ok_or_else(|| Response::error(404, "no such listing of yours"))?;
        if req.body.is_empty() {
            return Err(Response::error(400, "send the file"));
        }
        let name = market::safe_name(req.query("name").unwrap_or("file"));
        write_atomic(&self.cfg.data.join("market").join(market::safe_name(&l.id)).join("file"), &req.body).map_err(|e| Response::error(500, e))?;
        let sha = market::sha256_hex(&req.body);
        let size = req.body.len() as u64;
        let l = self.change(|st| {
            let l = st.listings.get_mut(id).expect("checked");
            l.file_name = name;
            l.size = size;
            l.sha256 = sha;
            l.published = true;
            l.clone()
        });
        ok(serde_json::to_value(l).expect("serialisable"))
    }

    fn download(&self, req: &Request, id: &str) -> R {
        let email = self.user(req)?;
        let (l, owns) = self.read(|st| (st.listings.get(id).cloned(), st.owned.get(&email).is_some_and(|o| o.contains(id))));
        let l = l.filter(|l| l.published).ok_or_else(|| Response::error(404, "no such listing"))?;
        if !(l.price_minor == 0 || owns || l.creator == email) {
            return Err(Response::error(402, "buy it first"));
        }
        let b = std::fs::read(self.cfg.data.join("market").join(market::safe_name(&l.id)).join("file")).map_err(|e| Response::error(500, e.to_string()))?;
        Ok(Response::bytes("application/octet-stream", b))
    }

    fn statement(&self, req: &Request) -> R {
        let email = self.user(req)?;
        let (sales, listings): (Vec<Sale>, Vec<Listing>) = self.read(|st| (st.sales.clone(), st.listings.values().filter(|l| l.creator == email).cloned().collect()));
        let s = market::statement(&email, &sales);
        ok(json!({"statement": s, "share_bps": self.cfg.file.creator_bps, "listings": listings, "sales": sales.iter().filter(|x| x.creator == email).collect::<Vec<_>>()}))
    }
}

/// Adds credits or sets a plan by hand (the `grant` command, for admins
/// and tests).
impl Server {
    pub fn grant(&self, email: &str, credits: i64, plan: Option<(&str, u64)>) -> Result<(), String> {
        let email = email.to_ascii_lowercase();
        self.change(|st| {
            let u = st.users.get_mut(&email).ok_or("no such account")?;
            u.balance += credits;
            if let Some((p, days)) = plan {
                u.plan = p.into();
                u.plan_expires = Some(now() + days * 86_400);
            }
            Ok(())
        })
    }
}
