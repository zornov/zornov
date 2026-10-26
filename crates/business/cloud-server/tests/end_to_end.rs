// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Studio's account client against the reference server on localhost, with
//! a local stand-in for Stripe's test API (no network, no real payments):
//! sign-up, buying a plan, the licence verifying offline, credits, review
//! links, cloud renders, project sync, the marketplace and webhooks.

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use studio_cloud_server::{Config, ConfigFile, Pack, Upstream, start};
use studio_commerce::{Account, FileSecrets, credits::PriceTable};
use studio_entitlements::{Catalogue, Price};

const KEY: &str = "sk_test_studio_e2e_not_a_real_key";

/// A tiny Stripe test-mode stand-in: Checkout Sessions, and `/pay/<id>`
/// where the "buyer" pays.
fn fake_stripe() -> String {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", l.local_addr().unwrap());
    let sessions: Arc<Mutex<Vec<Value>>> = Arc::default();
    let b2 = base.clone();
    std::thread::spawn(move || {
        for s in l.incoming() {
            let mut s = s.unwrap();
            let mut r = BufReader::new(s.try_clone().unwrap());
            let mut line = String::new();
            r.read_line(&mut line).unwrap();
            let mut auth = String::new();
            let mut len = 0;
            loop {
                let mut h = String::new();
                r.read_line(&mut h).unwrap();
                if h.trim().is_empty() {
                    break;
                }
                let lower = h.to_ascii_lowercase();
                if let Some(v) = lower.strip_prefix("authorization:") {
                    auth = v.trim().to_owned();
                }
                if let Some(v) = lower.strip_prefix("content-length:") {
                    len = v.trim().parse().unwrap();
                }
            }
            let mut body = vec![0; len];
            r.read_exact(&mut body).unwrap();
            let target = line.split_whitespace().nth(1).unwrap().to_owned();
            let mut ss = sessions.lock().unwrap();
            let (code, v) = if !target.starts_with("/pay/") && auth != format!("bearer {}", KEY.to_ascii_lowercase()) {
                (401, json!({"error": {"message": "Invalid API Key"}}))
            } else if line.starts_with("POST /v1/checkout/sessions") {
                let form: Vec<(String, String)> = studio_commerce::stripe::form_decode(&String::from_utf8_lossy(&body));
                let get = |k: &str| form.iter().find(|(a, _)| a == k).map(|(_, b)| b.clone()).unwrap_or_default();
                assert_eq!(get("mode"), "payment");
                assert!(get("success_url").contains("{CHECKOUT_SESSION_ID}"));
                let id = format!("cs_test_{}", ss.len() + 1);
                let amount: u64 = get("line_items[0][price_data][unit_amount]").parse().unwrap();
                let v = json!({"id": id, "object": "checkout.session", "livemode": false, "url": format!("{b2}/pay/{id}"), "payment_status": "unpaid", "client_reference_id": get("client_reference_id"), "amount_total": amount, "currency": get("line_items[0][price_data][currency]")});
                ss.push(v.clone());
                (200, v)
            } else if let Some(id) = target.strip_prefix("/v1/checkout/sessions/") {
                match ss.iter().find(|x| x["id"] == id) {
                    Some(v) => (200, v.clone()),
                    None => (404, json!({"error": {"message": "No such checkout.session"}})),
                }
            } else if let Some(id) = target.strip_prefix("/pay/") {
                match ss.iter_mut().find(|x| x["id"] == id) {
                    Some(v) => {
                        v["payment_status"] = json!("paid");
                        (200, json!({"paid": id}))
                    }
                    None => (404, json!({})),
                }
            } else {
                (404, json!({}))
            };
            let b = v.to_string();
            write!(s, "HTTP/1.1 {code} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{b}", b.len()).unwrap();
        }
    });
    base
}

/// An AI provider stand-in that checks the server's key and echoes.
fn fake_provider() -> String {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", l.local_addr().unwrap());
    std::thread::spawn(move || {
        for s in l.incoming() {
            let mut s = s.unwrap();
            let mut r = BufReader::new(s.try_clone().unwrap());
            let mut head = String::new();
            loop {
                let mut h = String::new();
                r.read_line(&mut h).unwrap();
                if h.trim().is_empty() {
                    break;
                }
                head.push_str(&h);
            }
            // Read the body before replying: closing a socket with unread
            // input makes the OS reset the connection, and the caller can
            // see the reset before the reply.
            let len = head.lines().find_map(|l| l.split_once(':').filter(|(k, _)| k.trim().eq_ignore_ascii_case("content-length")).and_then(|(_, v)| v.trim().parse::<usize>().ok())).unwrap_or(0);
            let mut body = vec![0; len];
            r.read_exact(&mut body).unwrap();
            let ok = head.contains("Authorization: Bearer provider-key-for-tests");
            let b = if ok { r#"{"text":"hello from the provider"}"# } else { r#"{"error":"bad key"}"# };
            write!(s, "HTTP/1.1 {} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{b}", if ok { 200 } else { 401 }, b.len()).unwrap();
        }
    });
    base
}

fn prices() -> PriceTable {
    PriceTable::parse(
        r#"{"schema":1,"credit_value_micros":10000,"currency":"usd","tasks":[
        {"task":"transcribe","unit":"minute","provider":"openai","model":"whisper-1","cost_micros":6000,"credits":1},
        {"task":"describe.image","unit":"image","provider":"openai","cost_micros":30000,"credits":4},
        {"task":"llm","unit":"1k tokens","provider":"openai","cost_micros":5000,"credits":1}]}"#,
    )
    .unwrap()
}

fn plans() -> Catalogue {
    let mut c = Catalogue::builtin();
    for p in &mut c.plans {
        if p.id != "free" {
            p.prices = vec![Price { amount_minor: 1_200, currency: "usd".into(), interval: "month".into() }];
        }
    }
    c
}

fn scratch(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("studio-cloud-e2e-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn account(dir: &Path) -> Account {
    Account::open(dir, Box::new(FileSecrets { path: dir.join("secrets.json") }))
}

fn config(data: &Path, stripe: &str, provider: &str) -> Config {
    let mut file = ConfigFile {
        name: "Test cloud".into(),
        packs: vec![Pack { id: "c500".into(), credits: 500, price_minor: 500, currency: "usd".into() }],
        stripe_api: stripe.into(),
        render_command: vec!["/bin/cp".into(), "{project}".into(), "{out}".into()],
        ..Default::default()
    };
    file.upstreams.insert("openai".into(), Upstream { base: provider.into(), header: "Authorization".into(), prefix: "Bearer ".into(), key_env: "STUDIO_E2E_PROVIDER_KEY".into() });
    Config { data: data.into(), file, plans: plans(), prices: Some(prices()), stripe_key: Some(KEY.into()), webhook_secret: Some("whsec_e2e".into()) }
}

/// The "buyer" opens the checkout page and pays.
fn pay(url: &str) {
    let r = studio_net::get(url, &[], Duration::from_secs(5)).unwrap();
    assert_eq!(r.status, 200);
}

#[test]
fn the_whole_paid_side_works_against_the_reference_server() {
    // SAFETY: set before any thread reads it; only this test uses it.
    unsafe { std::env::set_var("STUDIO_E2E_PROVIDER_KEY", "provider-key-for-tests") };
    let (stripe, provider) = (fake_stripe(), fake_provider());
    let data = scratch("server");
    let server = start(config(&data, &stripe, &provider), "127.0.0.1:0").unwrap();
    let url = server.url();

    // ---- sign-up: the cloud is off until connected; then free.
    let dir = scratch("ana");
    let ana = account(&dir);
    assert!(ana.sign_up("ana@example.com", "correct horse", "Ana").unwrap_err().contains("cloud is off"));
    let issuer = ana.connect(&url, true).unwrap();
    assert_eq!(issuer["public_key"], server.server.issuer_public_key());
    ana.sign_up("ana@example.com", "correct horse", "Ana").unwrap();
    assert!(ana.sign_up("ana@example.com", "correct horse", "Ana").is_err(), "a second sign-up with the same email");
    let st = ana.status();
    assert!(st.signed_in && st.entitlements.plan.id == "free");
    assert!(ana.account_catalogue_has_prices());

    // ---- quotes before paying: hosted AI is not on the free plan, own keys always are.
    let q = ana.quote("transcribe", 2.0, false).unwrap();
    assert!(!q.allowed && q.warnings.iter().any(|w| w.contains("Hosted AI")), "{q:?}");
    let q = ana.quote("transcribe", 2.0, true).unwrap();
    assert!(q.allowed && q.credits == 0);
    assert!(ana.reserve("transcribe", 1.0).is_err());

    // ---- buying Pro through Stripe test mode.
    let c = ana.checkout("plan", "pro").unwrap();
    let session = c["session"].as_str().unwrap().to_owned();
    assert!(c["url"].as_str().unwrap().starts_with(&stripe));
    let r = ana.confirm(&session).unwrap();
    assert_eq!(r["status"], "unpaid", "confirmed before paying");
    assert_eq!(ana.entitlements().plan.id, "free");
    pay(c["url"].as_str().unwrap());
    let r = ana.confirm(&session).unwrap();
    assert_eq!(r["status"], "paid");
    let e = ana.entitlements();
    assert_eq!(e.plan.id, "pro");
    assert!(e.allows("cloud.render") && e.allows("ai.hosted"));
    assert_eq!(ana.me().unwrap().balance, 1_000);
    // Confirming twice does not give the plan's credits twice.
    ana.confirm(&session).unwrap();
    assert_eq!(ana.me().unwrap().balance, 1_000);
    // The landing page Stripe sends the buyer back to.
    let page = String::from_utf8(studio_net::get(&format!("{url}/billing/done?session_id={session}"), &[], Duration::from_secs(5)).unwrap().bytes(1 << 20).unwrap()).unwrap();
    assert!(page.contains("went through"), "{page}");

    // ---- the licence works offline: the cloud off, the server gone.
    ana.disconnect().unwrap();
    assert_eq!(account(&dir).entitlements().plan.id, "pro");
    assert!(ana.refresh().unwrap_err().contains("cloud is off"));
    // An edited licence file is not honoured.
    let lic = std::fs::read_to_string(dir.join("licence.json")).unwrap();
    std::fs::write(dir.join("licence.json"), lic.replace("\\\"pro\\\"", "\\\"enterprise\\\"")).unwrap();
    let e = account(&dir).entitlements();
    assert_eq!(e.plan.id, "free", "{:?}", e.standing);
    std::fs::write(dir.join("licence.json"), &lic).unwrap();
    ana.connect(&url, true).unwrap();
    ana.refresh().unwrap();

    // ---- credits: quote, hold, charge what was used, record it.
    let q = ana.quote("transcribe", 3.5, false).unwrap();
    assert!(q.allowed && q.credits == 4 && q.balance_after == Some(996), "{q:?}");
    let h = ana.reserve("transcribe", 3.5).unwrap();
    assert_eq!(h["credits"], 4);
    let r = ana.commit(h["hold"].as_str().unwrap(), "transcribe", 2.2).unwrap();
    assert_eq!(r["credits"], 3);
    assert_eq!(r["balance"], 997);
    let h = ana.reserve("describe.image", 2.0).unwrap();
    assert_eq!(ana.release(h["hold"].as_str().unwrap()).unwrap()["balance"], 997);
    ana.record_byok("describe.image", 1.0, "openai").unwrap();
    let used = ana.ledger().entries();
    assert_eq!(used.len(), 2);
    assert_eq!(ana.status().month_used, 3);
    // A budget stops the next task.
    ana.ledger().set_budget(&studio_commerce::credits::Budget { monthly_limit: Some(5), warn_at_percent: 80, stop_at_limit: true }).unwrap();
    assert!(ana.reserve("describe.image", 1.0).unwrap_err().contains("budget"));
    ana.ledger().set_budget(&Default::default()).unwrap();

    // ---- the metered proxy: the server's key, the caller's credits.
    let c = studio_commerce::Client { base: url.clone(), token: ana.client().unwrap().token };
    let mut auth = String::new();
    auth.push_str("Bearer ");
    auth.push_str(c.token.as_deref().unwrap());
    let r = studio_net::post(&format!("{url}/v1/ai/openai/v1/chat"), &[("Authorization", &auth), ("x-studio-task", "llm"), ("x-studio-units", "2"), ("Content-Type", "application/json")], b"{}", Duration::from_secs(5)).unwrap();
    assert_eq!(r.status, 200);
    assert!(String::from_utf8(r.bytes(1000).unwrap()).unwrap().contains("hello from the provider"));
    ana.refresh().unwrap();
    assert_eq!(ana.me().unwrap().balance, 995);

    // ---- a credit pack.
    let c = ana.checkout("credits", "c500").unwrap();
    pay(c["url"].as_str().unwrap());
    ana.confirm(c["session"].as_str().unwrap()).unwrap();
    assert_eq!(ana.me().unwrap().balance, 1_495);

    // ---- a review link anyone can open and leave notes on.
    let film = dir.join("cut.mp4");
    std::fs::write(&film, b"\0\0\0\x18ftypmp42 not really a film").unwrap();
    let link = ana.review_link(&film, "Cut <3>").unwrap();
    let rurl = link["url"].as_str().unwrap().to_owned();
    let html = String::from_utf8(studio_net::get(&rurl, &[], Duration::from_secs(5)).unwrap().bytes(1 << 20).unwrap()).unwrap();
    assert!(html.contains("Cut &lt;3&gt;") && html.contains("<video"), "{html}");
    let media = studio_net::get(&format!("{rurl}/media"), &[], Duration::from_secs(5)).unwrap();
    assert_eq!(media.content_type.as_deref(), Some("video/mp4"));
    let r = studio_net::post(&format!("{rurl}/comments"), &[("Content-Type", "application/json")], json!({"name": "Bo", "at": 4.5, "note": "trim the <b>end</b>"}).to_string().as_bytes(), Duration::from_secs(5)).unwrap();
    assert_eq!(r.status, 200);
    let notes = ana.review_notes(link["token"].as_str().unwrap()).unwrap();
    assert_eq!(notes[0]["note"], "trim the <b>end</b>");
    let html = String::from_utf8(studio_net::get(&rurl, &[], Duration::from_secs(5)).unwrap().bytes(1 << 20).unwrap()).unwrap();
    assert!(html.contains("trim the &lt;b&gt;end&lt;/b&gt;") && !html.contains("<b>end</b>"));

    // ---- a cloud render (the test's "renderer" copies the project).
    let job = ana.render_submit("Trailer", &json!({"name": "Trailer", "clips": []}), &json!({"format": "mp4"})).unwrap();
    let id = job["id"].as_str().unwrap().to_owned();
    let mut status = String::new();
    for _ in 0..100 {
        status = ana.render_status(&id).unwrap()["status"].as_str().unwrap().to_owned();
        if status == "done" || status == "failed" {
            break;
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    assert_eq!(status, "done");
    let out = dir.join("Trailer.mp4");
    ana.render_fetch(&id, &out).unwrap();
    let back: Value = serde_json::from_slice(&std::fs::read(&out).unwrap()).unwrap();
    assert_eq!(back["name"], "Trailer");

    // ---- project sync needs the Studio plan; two people, one project.
    assert!(ana.sync("film").err().unwrap().contains("needs the Studio"));
    server.server.grant("ana@example.com", 0, Some(("studio", 30))).unwrap();
    ana.refresh().unwrap();
    let bo_dir = scratch("bo");
    let bo = account(&bo_dir);
    bo.connect(&url, true).unwrap();
    bo.sign_up("bo@example.com", "another password", "Bo").unwrap();
    server.server.grant("bo@example.com", 0, Some(("studio", 30))).unwrap();
    bo.refresh().unwrap();
    let a = ana.sync("film").unwrap();
    a.create("Film").unwrap();
    assert_eq!(a.head().unwrap(), None);
    assert_eq!(a.join("ana").unwrap(), 0);
    let b = bo.sync("film").unwrap();
    assert!(b.head().unwrap_err().contains("invite"), "a stranger read the project");
    assert!(b.create("Film").unwrap_err().contains("someone else"));
    a.invite("Bo@example.com").unwrap();
    assert_eq!(b.join("bo").unwrap(), 1);
    assert_eq!(b.join("bo").unwrap(), 1, "joining twice keeps the id block");
    use studio_commerce::account::Published;
    let Published::Done(h) = a.publish(0, &json!({"v": 1}), "ana", "first cut").unwrap() else { panic!("stale") };
    assert_eq!(h["rev"], 1);
    let Published::Stale(h) = b.publish(0, &json!({"v": 2}), "bo", "late").unwrap() else { panic!("not stale") };
    assert_eq!(h["by"], "ana");
    assert!(matches!(b.publish(1, &json!({"v": 2}), "bo", "after syncing").unwrap(), Published::Done(_)));
    assert_eq!(a.fetch(2).unwrap(), json!({"v": 2}));
    b.publish_notes("bo", &json!({"notes": [1]})).unwrap();
    assert_eq!(a.notes().unwrap()[0], json!(["bo", {"notes": [1]}]));
    a.publish_team(&json!({"roles": {"ana": "owner"}})).unwrap();
    assert_eq!(b.team().unwrap().unwrap()["roles"]["ana"], "owner");

    // ---- the marketplace: Ana sells a LUT, Bo buys it, a free sound.
    let lut = dir.join("warm.cube");
    std::fs::write(&lut, "TITLE \"Warm\"\nLUT_3D_SIZE 2\n0 0 0\n1 0 0\n0 1 0\n1 1 0\n0 0 1\n1 0 1\n0 1 1\n1 1 1\n").unwrap();
    let listing = ana.sell("lut", "Warm film", "A warm look", 500, "usd", &lut).unwrap();
    assert!(listing.published && listing.sha256.len() == 64);
    let free = ana.sell("sound", "Door knock", "", 0, "usd", &film).unwrap();
    let found = bo.market(Some("lut"), Some("warm")).unwrap();
    assert_eq!(found.as_array().unwrap().len(), 1);
    let studio = bo_dir.join("Studio");
    let r = bo.buy(&listing.id, &studio).unwrap();
    assert!(bo.install(&listing.id, &studio).unwrap_err().contains("buy it first"));
    pay(r["url"].as_str().unwrap());
    bo.confirm(r["session"].as_str().unwrap()).unwrap();
    let r = bo.buy(&listing.id, &studio).unwrap();
    let installed = PathBuf::from(r["installed"].as_str().unwrap());
    assert!(installed.starts_with(studio.join("Marketplace/LUTs")));
    assert_eq!(std::fs::read(&installed).unwrap(), std::fs::read(&lut).unwrap());
    let r = bo.buy(&free.id, &studio).unwrap();
    assert!(r["installed"].as_str().unwrap().contains("Sounds"));
    assert_eq!(studio_commerce::market::installed(&studio).len(), 2);
    let s = ana.statement().unwrap();
    assert_eq!(s["statement"]["sales"], 1);
    assert_eq!(s["statement"]["totals"][0], json!(["usd", 500, 350, 150]));

    // ---- receipts.
    let rc = ana.receipts().unwrap();
    let titles: Vec<&str> = rc.as_array().unwrap().iter().map(|r| r["title"].as_str().unwrap()).collect();
    assert_eq!(titles, ["Studio Pro (month)", "500 Studio credits"]);
    assert_eq!(bo.receipts().unwrap().as_array().unwrap().len(), 1);

    // ---- a signed webhook fulfils an order without the buyer coming back.
    let c = bo.checkout("credits", "c500").unwrap();
    let sess = c["session"].as_str().unwrap().to_owned();
    pay(c["url"].as_str().unwrap());
    let ev = json!({"type": "checkout.session.completed", "livemode": false, "data": {"object": {"id": sess}}}).to_string();
    let bad = studio_net::post(&format!("{url}/v1/stripe/webhook"), &[("Stripe-Signature", "t=1,v1=00")], ev.as_bytes(), Duration::from_secs(5)).unwrap();
    assert_eq!(bad.status, 400);
    let sig = studio_commerce::stripe::sign_webhook(ev.as_bytes(), "whsec_e2e", studio_entitlements::now());
    let good = studio_net::post(&format!("{url}/v1/stripe/webhook"), &[("Stripe-Signature", &sig)], ev.as_bytes(), Duration::from_secs(5)).unwrap();
    assert_eq!(good.status, 200);
    bo.refresh().unwrap();
    assert_eq!(bo.me().unwrap().balance, 500);

    // ---- sign-out ends the session; the licence stays for offline use.
    let token = ana.client().unwrap().token.unwrap();
    ana.sign_out().unwrap();
    assert!(!ana.signed_in());
    assert_eq!(ana.entitlements().plan.id, "studio");
    let r = studio_net::get(&format!("{url}/v1/me"), &[("Authorization", &format!("Bearer {token}"))], Duration::from_secs(5)).unwrap();
    assert_eq!(r.status, 401);
    ana.sign_in("ana@example.com", "correct horse").unwrap();
    assert!(ana.sign_in("ana@example.com", "wrong horse").unwrap_err().contains("wrong"));

    // ---- the state survives a restart.
    server.stop();
    let again = start(config(&data, &stripe, &provider), "127.0.0.1:0").unwrap();
    let ana2 = account(&dir);
    ana2.connect(&again.url(), true).unwrap();
    ana2.sign_in("ana@example.com", "correct horse").unwrap();
    assert_eq!(ana2.me().unwrap().plan, "studio");
    again.stop();

    for d in [data, dir, bo_dir] {
        let _ = std::fs::remove_dir_all(d);
    }
}

#[test]
fn a_server_refuses_prices_that_lose_money_and_live_keys() {
    let data = scratch("refuse");
    let mut cfg = config(&data, "http://127.0.0.1:9", "http://127.0.0.1:9");
    cfg.prices.as_mut().unwrap().tasks[0].credits = 0;
    assert!(start(cfg, "127.0.0.1:0").err().unwrap().contains("do not cover"));
    let mut cfg = config(&data, "http://127.0.0.1:9", "http://127.0.0.1:9");
    cfg.stripe_key = Some("sk_live_nope".into());
    assert!(start(cfg, "127.0.0.1:0").err().unwrap().contains("test keys"));
    let _ = std::fs::remove_dir_all(data);
}

trait CatalogueCheck {
    fn account_catalogue_has_prices(&self) -> bool;
}

impl CatalogueCheck for Account {
    fn account_catalogue_has_prices(&self) -> bool {
        self.catalogue().plan("pro").is_some_and(|p| !p.prices.is_empty())
    }
}
