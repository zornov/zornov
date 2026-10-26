// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Settings ▸ Account: the plan and upgrading, the cloud and signing in,
//! hosted AI credits (balance, budget, usage), receipts and the
//! marketplace. Only built with the `commerce` feature (off by default).
//! Server calls run on a background thread, so the window never waits.

use crate::icons::Icon;
use crate::kit::{self, Button, Section};
use crate::theme;
use eframe::egui;

#[derive(Default)]
pub struct AccountState {
    inner: on::State,
}

pub fn page(st: &mut AccountState, ui: &mut egui::Ui) {
    on::page(&mut st.inner, ui);
}

fn note(ui: &mut egui::Ui, text: &str) {
    let t = theme::tokens(ui.ctx());
    ui.add(egui::Label::new(egui::RichText::new(text).font(theme::caption()).color(t.text_2)).wrap());
}

mod on {
    use super::{Button, Icon, Section, kit, note, theme};
    use crate::agent::account_tools::on as acct;
    use eframe::egui;
    use serde_json::Value;
    use std::sync::mpsc;

    /// A server call on its way.
    struct Job {
        what: &'static str,
        rx: mpsc::Receiver<Result<Value, String>>,
    }

    #[derive(Default)]
    pub struct State {
        /// `account` status, read once per visit and after each call.
        status: Option<Value>,
        server: String,
        trust: bool,
        email: String,
        password: String,
        name: String,
        budget: String,
        job: Option<Job>,
        note: Option<String>,
        /// The checkout waiting to be confirmed.
        session: Option<String>,
        receipts: Option<Vec<Value>>,
        packs: Option<Vec<Value>>,
        market_kind: usize,
        market_query: String,
        market: Option<Vec<Value>>,
    }

    const KINDS: [&str; 6] = ["all", "template", "plugin", "lut", "sound", "asset"];

    impl State {
        fn start(&mut self, what: &'static str, f: impl FnOnce(&studio_commerce::Account) -> Result<Value, String> + Send + 'static) {
            if self.job.is_some() {
                return;
            }
            let (tx, rx) = mpsc::channel();
            std::thread::spawn(move || {
                let _ = tx.send(f(&acct::account()));
            });
            self.job = Some(Job { what, rx });
            self.note = None;
        }

        /// Takes a finished call's answer.
        fn poll(&mut self, ctx: &egui::Context) {
            let Some(job) = &self.job else { return };
            match job.rx.try_recv() {
                Ok(r) => {
                    let what = job.what;
                    self.job = None;
                    self.status = None;
                    match r {
                        Ok(v) => self.done(ctx, what, v),
                        Err(e) => self.note = Some(e),
                    }
                }
                Err(mpsc::TryRecvError::Empty) => ctx.request_repaint_after(std::time::Duration::from_millis(100)),
                Err(mpsc::TryRecvError::Disconnected) => self.job = None,
            }
        }

        fn done(&mut self, ctx: &egui::Context, what: &str, v: Value) {
            match what {
                "checkout" | "buy" => {
                    if let Some(url) = v.get("url").and_then(Value::as_str) {
                        ctx.open_url(egui::OpenUrl::new_tab(url));
                        self.session = v.get("session").and_then(Value::as_str).map(str::to_owned);
                        self.note = Some("The checkout opened in your browser (Stripe test mode). When you have paid, press Check payment.".into());
                    } else if let Some(p) = v.get("installed").and_then(Value::as_str) {
                        self.note = Some(crate::i18n::tf(format!("Installed: {p}")));
                    } else {
                        self.note = Some("Done.".into());
                    }
                }
                "confirm" => {
                    let paid = v.get("status").and_then(Value::as_str) == Some("paid");
                    if paid {
                        self.session = None;
                    }
                    self.note = Some(if paid { "Payment received. Your account is up to date.".into() } else { "Not paid yet. Finish the checkout, then check again.".into() });
                }
                "receipts" => self.receipts = v.as_array().cloned(),
                "packs" => self.packs = v.as_array().cloned(),
                "market" => self.market = v.as_array().cloned(),
                "sign_in" => {
                    self.password.clear();
                    self.note = Some("Signed in.".into());
                }
                _ => self.note = Some("Done.".into()),
            }
        }
    }

    fn s<'a>(v: &'a Value, k: &str) -> &'a str {
        v.get(k).and_then(Value::as_str).unwrap_or("")
    }

    fn money(minor: u64, currency: &str) -> String {
        format!("{} {}.{:02}", currency.to_uppercase(), minor / 100, minor % 100)
    }

    fn standing(v: &Value) -> String {
        match s(v, "state") {
            "active" => match v.get("expires").and_then(Value::as_u64) {
                Some(e) => crate::i18n::tf(format!("Active, renews or ends in {} days", e.saturating_sub(studio_entitlements::now()) / 86_400)),
                None => "Active".into(),
            },
            "grace" => crate::i18n::tf(format!("Expired: still working for {} more days. Renew to keep it.", v.get("days_left").and_then(Value::as_u64).unwrap_or(0))),
            "expired" => "Expired: back on the free plan until renewed.".into(),
            "invalid" => crate::i18n::tf(format!("The licence file was not accepted ({}).", s(v, "reason"))),
            "unknown_plan" => "The licence names a plan this Studio does not know.".into(),
            _ => "No licence: the free plan.".into(),
        }
    }

    pub fn page(st: &mut State, ui: &mut egui::Ui) {
        st.poll(ui.ctx());
        let status = st.status.get_or_insert_with(|| acct::status(&acct::account())).clone();
        if st.server.is_empty() {
            st.server = s(&status["cloud"], "server").to_owned();
        }
        let busy = st.job.is_some();
        let cloud_on = status["cloud"]["on"].as_bool().unwrap_or(false);
        let signed_in = status["signed_in"].as_bool().unwrap_or(false);

        Section::new("set-account-plan", "Plan").icon(Icon::Star).fixed().show(ui, |ui| {
            kit::labeled_row(ui, "Plan", |ui| ui.label(egui::RichText::new(s(&status, "plan_name")).font(theme::body()).strong()));
            note(ui, &standing(&status["standing"]));
            let features: Vec<&str> = status["features"].as_array().map(|f| f.iter().filter_map(Value::as_str).map(studio_entitlements::describe).collect()).unwrap_or_default();
            note(ui, &crate::i18n::tf(format!("Includes: the whole editor, and {}.", features.join(", "))));
            kit::labeled_row(ui, "Licence file", |ui| {
                if Button::secondary("Install…").icon(Icon::DownloadSimple).show(ui).clicked() {
                    if let Some(p) = rfd::FileDialog::new().add_filter("Studio licence", &["json"]).pick_file() {
                        match acct::account().install_licence_file(&p) {
                            Ok(_) => st.note = Some("Licence installed.".into()),
                            Err(e) => st.note = Some(e),
                        }
                        st.status = None;
                    }
                }
            });
            note(ui, "A licence is checked on this computer, so paid features work offline.");
        });

        Section::new("set-account-cloud", "Cloud and sign-in").icon(Icon::LinkSimple).fixed().show(ui, |ui| {
            note(ui, "Off until you turn it on: nothing is sent anywhere before you connect a server. Use the Studio cloud or a server you run yourself (studio-cloud-server).");
            kit::labeled_row(ui, "Server", |ui| {
                ui.add(egui::TextEdit::singleline(&mut st.server).hint_text("https://… or http://localhost:8787").desired_width(240.0));
                if cloud_on {
                    if Button::secondary("Turn off").show(ui).clicked() {
                        let _ = acct::account().disconnect();
                        st.status = None;
                    }
                } else if ui.add_enabled(!busy, egui::Button::new("Connect")).clicked() {
                    let (server, trust) = (st.server.clone(), st.trust);
                    st.start("connect", move |a| a.connect(&server, trust));
                }
            });
            kit::labeled_row(ui, "", |ui| ui.checkbox(&mut st.trust, "Trust licences this server signs (your own server)"));
            if cloud_on {
                if signed_in {
                    kit::labeled_row(ui, "Signed in as", |ui| {
                        ui.label(egui::RichText::new(status["email"].as_str().unwrap_or("")).font(theme::body()));
                        if ui.add_enabled(!busy, egui::Button::new("Refresh")).clicked() {
                            st.start("refresh", |a| a.refresh());
                        }
                        if ui.add_enabled(!busy, egui::Button::new("Sign out")).clicked() {
                            st.start("sign_out", |a| a.sign_out().map(|_| Value::Null));
                        }
                    });
                } else {
                    kit::labeled_row(ui, "Email", |ui| ui.add(egui::TextEdit::singleline(&mut st.email).desired_width(240.0)));
                    kit::labeled_row(ui, "Password", |ui| ui.add(egui::TextEdit::singleline(&mut st.password).password(true).desired_width(240.0)));
                    kit::labeled_row(ui, "Name", |ui| ui.add(egui::TextEdit::singleline(&mut st.name).hint_text("for a new account").desired_width(240.0)));
                    kit::labeled_row(ui, "", |ui| {
                        let (e, p, n) = (st.email.clone(), st.password.clone(), st.name.clone());
                        if ui.add_enabled(!busy, egui::Button::new("Sign in")).clicked() {
                            st.start("sign_in", move |a| a.sign_in(&e, &p));
                        }
                        let (e, p) = (st.email.clone(), st.password.clone());
                        if ui.add_enabled(!busy, egui::Button::new("Create account")).clicked() {
                            st.start("sign_in", move |a| a.sign_up(&e, &p, &n));
                        }
                    });
                }
            }
        });

        let catalogue = acct::account().catalogue();
        let current = s(&status, "plan").to_owned();
        Section::new("set-account-upgrade", "Upgrade").icon(Icon::Sparkle).fixed().show(ui, |ui| {
            for p in &catalogue.plans {
                kit::labeled_row(ui, &p.name, |ui| {
                    let price = p.prices.first().map(|x| format!("{} / {}", money(x.amount_minor, &x.currency), x.interval)).unwrap_or_else(|| if p.id == "free" { "Free".into() } else { "price from your server".into() });
                    ui.label(egui::RichText::new(price).font(theme::body()));
                    if p.id == current {
                        ui.label(egui::RichText::new(crate::i18n::shown("Current")).font(theme::caption()));
                    } else if !p.prices.is_empty() && signed_in && ui.add_enabled(!busy, egui::Button::new("Upgrade")).clicked() {
                        let id = p.id.clone();
                        st.start("checkout", move |a| a.checkout("plan", &id));
                    }
                });
                note(ui, &format!("{} {} credits a month, {} seat{}.", p.summary, p.monthly_credits, p.seats, if p.seats == 1 { "" } else { "s" }));
            }
            if let Some(sess) = st.session.clone() {
                kit::labeled_row(ui, "Checkout", |ui| {
                    if ui.add_enabled(!busy, egui::Button::new("Check payment")).clicked() {
                        st.start("confirm", move |a| a.confirm(&sess));
                    }
                });
            }
        });

        Section::new("set-account-credits", "Credits and usage").icon(Icon::Gauge).fixed().show(ui, |ui| {
            let used = status["month_used"].as_u64().unwrap_or(0);
            let limit = status["budget"]["monthly_limit"].as_u64();
            kit::labeled_row(ui, "Balance", |ui| ui.label(egui::RichText::new(status["balance"].as_i64().map_or("—".into(), |b| format!("{b} credits"))).font(theme::body())));
            kit::labeled_row(ui, "This month", |ui| ui.label(egui::RichText::new(match limit { Some(l) => format!("{used} of {l} credits"), None => format!("{used} credits") }).font(theme::body())));
            kit::labeled_row(ui, "Monthly budget", |ui| {
                if st.budget.is_empty() {
                    st.budget = limit.map(|l| l.to_string()).unwrap_or_default();
                }
                ui.add(egui::TextEdit::singleline(&mut st.budget).hint_text("no limit").desired_width(90.0));
                if Button::secondary("Set").show(ui).clicked() {
                    let a = acct::account();
                    let mut b = a.ledger().budget();
                    b.monthly_limit = st.budget.trim().parse::<u64>().ok().filter(|l| *l > 0);
                    st.note = Some(match a.ledger().set_budget(&b) {
                        Ok(()) => "Budget saved. The AI tells you before a task would pass it.".into(),
                        Err(e) => e,
                    });
                    st.status = None;
                }
            });
            note(ui, "AI with your own keys never uses credits. Before a hosted task the AI tells you what it costs.");
            let entries = acct::account().ledger().entries();
            for u in entries.iter().rev().take(8) {
                note(ui, &format!("{} · {} × {:.2} · {}", u.task, if u.byok { "own key" } else { "credits" }, u.units, if u.byok { "no credits".to_owned() } else { format!("{} credits", u.credits) }));
            }
            if signed_in {
                kit::labeled_row(ui, "Add credits", |ui| {
                    if st.packs.is_none() && ui.add_enabled(!busy, egui::Button::new("Show packs")).clicked() {
                        st.start("packs", |a| a.packs());
                    }
                    for p in st.packs.clone().unwrap_or_default() {
                        let label = format!("{} for {}", p["credits"], money(p["price_minor"].as_u64().unwrap_or(0), s(&p, "currency")));
                        if ui.add_enabled(!busy, egui::Button::new(label)).clicked() {
                            let id = s(&p, "id").to_owned();
                            st.start("checkout", move |a| a.checkout("credits", &id));
                        }
                    }
                });
            }
        });

        if signed_in {
            Section::new("set-account-receipts", "Receipts").icon(Icon::List).fixed().show(ui, |ui| {
                if ui.add_enabled(!busy, egui::Button::new(if st.receipts.is_some() { "Reload" } else { "Show receipts" })).clicked() {
                    st.start("receipts", |a| a.receipts());
                }
                for r in st.receipts.clone().unwrap_or_default() {
                    note(ui, &format!("{} · {} · order {}", s(&r, "title"), money(r["amount_minor"].as_u64().unwrap_or(0), s(&r, "currency")), s(&r, "order")));
                }
            });
        }

        if cloud_on {
            Section::new("set-account-market", "Marketplace").icon(Icon::SquaresFour).fixed().show(ui, |ui| {
                kit::labeled_row(ui, "Find", |ui| {
                    egui::ComboBox::from_id_salt("set-market-kind").selected_text(KINDS[st.market_kind]).show_index(ui, &mut st.market_kind, KINDS.len(), |i| KINDS[i]);
                    ui.add(egui::TextEdit::singleline(&mut st.market_query).hint_text("templates, LUTs, sounds…").desired_width(180.0));
                    if ui.add_enabled(!busy, egui::Button::new("Search")).clicked() {
                        let kind = (st.market_kind > 0).then(|| KINDS[st.market_kind].to_owned());
                        let q = Some(st.market_query.trim().to_owned()).filter(|q| !q.is_empty());
                        st.start("market", move |a| a.market(kind.as_deref(), q.as_deref()));
                    }
                });
                for l in st.market.clone().unwrap_or_default() {
                    kit::labeled_row(ui, s(&l, "kind"), |ui| {
                        let price = l["price_minor"].as_u64().unwrap_or(0);
                        ui.label(egui::RichText::new(format!("{} by {} · {}", s(&l, "title"), s(&l, "creator_name"), if price == 0 { "free".into() } else { money(price, s(&l, "currency")) })).font(theme::body()));
                        if signed_in && ui.add_enabled(!busy, egui::Button::new(if price == 0 { "Get" } else { "Buy" })).clicked() {
                            let id = s(&l, "id").to_owned();
                            st.start("buy", move |a| a.buy(&id, &studio_paths::studio()));
                        }
                    });
                }
                note(ui, &format!("{} items installed in the Studio folder's Marketplace.", studio_commerce::market::installed(&studio_paths::studio()).len()));
            });
        }

        if busy {
            ui.horizontal(|ui| {
                ui.spinner();
                note(ui, "Talking to the server…");
            });
        }
        if let Some(n) = &st.note {
            note(ui, n);
        }
    }

    #[cfg(test)]
    mod tests {
        /// The page draws without a window or a network (the cloud is off).
        #[test]
        fn the_account_page_draws_headless() {
            let ctx = eframe::egui::Context::default();
            let mut st = super::State::default();
            for _ in 0..2 {
                crate::editor::test_frame(&ctx, Default::default(), |ui| super::page(&mut st, ui));
            }
            assert!(st.status.is_some());
            assert!(st.job.is_none());
        }
    }
}
