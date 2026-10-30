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

use crate::screens::settings::pill;

/// One width for every text field on the page.
const FIELD_W: f32 = 240.0;

/// One line of help under a row, the rest on hover.
fn hint(ui: &mut egui::Ui, text: &str) {
    crate::screens::settings::hint(ui, text, None);
}

/// A status line (the answer to what the person just did), wrapped.
fn note(ui: &mut egui::Ui, text: &str) {
    let t = theme::tokens(ui.ctx());
    ui.add(egui::Label::new(egui::RichText::new(text).font(theme::caption()).color(t.text_2)).wrap());
}

mod on {
    use super::{Button, FIELD_W, Icon, Section, hint, kit, note, pill, theme};
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
                        self.note = Some("Opened the checkout in your browser. Press Check payment once you have paid".into());
                    } else if let Some(p) = v.get("installed").and_then(Value::as_str) {
                        let name = std::path::Path::new(p).file_name().map_or_else(|| p.to_owned(), |n| n.to_string_lossy().into_owned());
                        self.note = Some(crate::i18n::tf(format!("Installed {name}")));
                    } else {
                        self.note = Some("Done".into());
                    }
                }
                "confirm" => {
                    let paid = v.get("status").and_then(Value::as_str) == Some("paid");
                    if paid {
                        self.session = None;
                    }
                    self.note = Some(if paid { "Payment received".into() } else { "Not paid yet. Finish the checkout, then check again".into() });
                }
                "receipts" => self.receipts = v.as_array().cloned(),
                "packs" => self.packs = v.as_array().cloned(),
                "market" => self.market = v.as_array().cloned(),
                "sign_in" => {
                    self.password.clear();
                    self.note = Some("Signed in".into());
                }
                _ => self.note = Some("Done".into()),
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
                Some(e) => crate::i18n::tf(format!("Active, {} days left", e.saturating_sub(studio_entitlements::now()) / 86_400)),
                None => "Active".into(),
            },
            "grace" => crate::i18n::tf(format!("Expired, works {} more days", v.get("days_left").and_then(Value::as_u64).unwrap_or(0))),
            "expired" => "Expired, on the free plan".into(),
            "invalid" => crate::i18n::tf(format!("Licence not accepted ({})", s(v, "reason"))),
            "unknown_plan" => "Plan not recognised".into(),
            _ => "Free plan".into(),
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
            kit::labeled_row(ui, "Plan", |ui| {
                ui.label(egui::RichText::new(s(&status, "plan_name")).font(theme::body()).strong());
                pill(ui, &standing(&status["standing"]), s(&status["standing"], "state") == "active");
            });
            let features: Vec<&str> = status["features"].as_array().map(|f| f.iter().filter_map(Value::as_str).map(studio_entitlements::describe).collect()).unwrap_or_default();
            if !features.is_empty() {
                hint(ui, &crate::i18n::tf(format!("Also includes {}", features.join(", "))));
            }
            kit::labeled_row(ui, "Licence file", |ui| {
                if Button::secondary("Install…").icon(Icon::DownloadSimple).show(ui).clicked() {
                    if let Some(p) = rfd::FileDialog::new().add_filter("Zornov licence", &["json"]).pick_file() {
                        match acct::account().install_licence_file(&p) {
                            Ok(_) => st.note = Some("Installed the licence".into()),
                            Err(e) => st.note = Some(e),
                        }
                        st.status = None;
                    }
                }
            });
            hint(ui, "Checked on this computer, so paid features work offline.");
        });

        Section::new("set-account-cloud", "Cloud and sign-in").icon(Icon::LinkSimple).fixed().show(ui, |ui| {
            kit::labeled_row(ui, "Server", |ui| {
                ui.add(egui::TextEdit::singleline(&mut st.server).hint_text("https://… or http://localhost:8787").desired_width(FIELD_W));
                if cloud_on {
                    if Button::secondary("Turn off").show(ui).clicked() {
                        let _ = acct::account().disconnect();
                        st.status = None;
                    }
                    pill(ui, "Connected", true);
                } else if ui.add_enabled(!busy, egui::Button::new("Connect")).clicked() {
                    let (server, trust) = (st.server.clone(), st.trust);
                    st.start("connect", move |a| a.connect(&server, trust));
                }
            });
            hint(ui, "Off until you connect a server: nothing is sent before then.");
            kit::labeled_row(ui, "", |ui| {
                ui.checkbox(&mut st.trust, "Trust licences it signs").on_hover_text("Only for a server you run yourself.")
            });
            if cloud_on {
                if signed_in {
                    kit::labeled_row(ui, "Account", |ui| {
                        ui.label(egui::RichText::new(status["email"].as_str().unwrap_or("")).font(theme::body()));
                        pill(ui, "Signed in", true);
                        if ui.add_enabled(!busy, egui::Button::new("Refresh")).clicked() {
                            st.start("refresh", |a| a.refresh());
                        }
                        if ui.add_enabled(!busy, egui::Button::new("Sign out")).clicked() {
                            st.start("sign_out", |a| a.sign_out().map(|_| Value::Null));
                        }
                    });
                } else {
                    kit::labeled_row(ui, "Email", |ui| ui.add(egui::TextEdit::singleline(&mut st.email).desired_width(FIELD_W)));
                    kit::labeled_row(ui, "Password", |ui| ui.add(egui::TextEdit::singleline(&mut st.password).password(true).desired_width(FIELD_W)));
                    kit::labeled_row(ui, "Name", |ui| ui.add(egui::TextEdit::singleline(&mut st.name).hint_text("for a new account").desired_width(FIELD_W)));
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
                    let price = p.prices.first().map(|x| format!("{} / {}", money(x.amount_minor, &x.currency), x.interval)).unwrap_or_else(|| if p.id == "free" { "Free".into() } else { "Price from your server".into() });
                    ui.label(egui::RichText::new(price).font(theme::body()));
                    if p.id == current {
                        pill(ui, "Current", true);
                    } else if !p.prices.is_empty() && signed_in && ui.add_enabled(!busy, egui::Button::new("Upgrade")).clicked() {
                        let id = p.id.clone();
                        st.start("checkout", move |a| a.checkout("plan", &id));
                    }
                });
                hint(ui, &format!("{} credits a month, {} seat{}. {}", p.monthly_credits, p.seats, if p.seats == 1 { "" } else { "s" }, p.summary));
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
                        Ok(()) => "Saved the monthly budget".into(),
                        Err(e) => e,
                    });
                    st.status = None;
                }
            });
            hint(ui, "The AI tells you what a hosted task costs. Your own keys cost none.");
            let entries = acct::account().ledger().entries();
            for u in entries.iter().rev().take(8) {
                kit::labeled_row(ui, &u.task, |ui| {
                    let cost = if u.byok { "Own key, no credits".to_owned() } else { format!("{} credits", u.credits) };
                    ui.label(egui::RichText::new(cost).font(theme::body()));
                });
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
                kit::labeled_row(ui, "Receipts", |ui| {
                    if ui.add_enabled(!busy, egui::Button::new(if st.receipts.is_some() { "Reload" } else { "Show" })).clicked() {
                        st.start("receipts", |a| a.receipts());
                    }
                });
                for r in st.receipts.clone().unwrap_or_default() {
                    kit::labeled_row(ui, s(&r, "title"), |ui| {
                        let paid = ui.label(egui::RichText::new(money(r["amount_minor"].as_u64().unwrap_or(0), s(&r, "currency"))).font(theme::body()));
                        paid.on_hover_text(format!("Order {}", s(&r, "order")));
                    });
                }
            });
        }

        if cloud_on {
            Section::new("set-account-market", "Marketplace").icon(Icon::SquaresFour).fixed().show(ui, |ui| {
                kit::labeled_row(ui, "Find", |ui| {
                    egui::ComboBox::from_id_salt("set-market-kind").selected_text(KINDS[st.market_kind]).show_index(ui, &mut st.market_kind, KINDS.len(), |i| KINDS[i]);
                    ui.add(egui::TextEdit::singleline(&mut st.market_query).hint_text("templates, LUTs, sounds…").desired_width(FIELD_W - 70.0));
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
                hint(ui, &format!("{} installed from the marketplace.", studio_commerce::market::installed(&studio_paths::studio()).len()));
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

        /// One frame of the page, drawn on the CPU: its text with where it sits.
        fn frame() -> (eframe::egui::Context, eframe::egui::FullOutput, Vec<(String, eframe::egui::Rect, f32)>) {
            use eframe::egui;
            let ctx = egui::Context::default();
            let mut st = super::State::default();
            let mut out = Default::default();
            for i in 0..2 {
                let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(760.0, 1400.0))), time: Some(f64::from(i) / 60.0 + 1.0), ..Default::default() };
                out = ctx.run_ui(input, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| super::page(&mut st, ui));
                });
                out.textures_delta.clear();
            }
            let mut texts = vec![];
            fn walk(shape: &egui::epaint::Shape, texts: &mut Vec<(String, egui::Rect, f32)>) {
                match shape {
                    egui::epaint::Shape::Text(t) => {
                        let size = t.galley.job.sections.first().map_or(0.0, |s| s.format.font_id.size);
                        texts.push((t.galley.text().to_owned(), t.galley.rect.translate(t.pos.to_vec2()), size));
                    }
                    egui::epaint::Shape::Vec(v) => v.iter().for_each(|s| walk(s, texts)),
                    _ => {}
                }
            }
            for cs in &out.shapes {
                walk(&cs.shape, &mut texts);
            }
            (ctx, out, texts)
        }

        /// Every help line is one line of at most `HELP_MAX` characters and
        /// they all start in the control column, right of where the labels
        /// start; nothing is cut off and no raw path or id is in the text.
        #[test]
        fn the_page_is_a_grid_with_one_line_help() {
            let (ctx, out, texts) = frame();
            let caption = crate::theme::caption().size;
            let label_x = texts.iter().find(|(t, ..)| t == "Licence file").expect("the Licence file row").1.left();
            let col_x = label_x + crate::kit::label_width(&ctx);
            let help: Vec<_> = texts.iter().filter(|(t, _, size)| (*size - caption).abs() < 0.1 && t.chars().count() > 20).collect();
            assert!(!help.is_empty(), "{texts:?}");
            for (t, r, _) in &help {
                assert!(t.chars().count() <= crate::screens::settings::HELP_MAX, "help too long: {t}");
                assert!((r.left() - col_x).abs() < 1.5, "help {t:?} starts at {} not {col_x}", r.left());
                assert!(r.height() < 2.0 * caption, "help wraps: {t}");
            }
            for (t, ..) in &texts {
                assert!(!t.contains("studio-cloud-server") && !t.contains("/Users/") && !t.contains("Stripe"), "developer text: {t}");
            }
            let cuts = crate::ui_shot::cut_text(&ctx, &out, &[], 1.0);
            assert!(cuts.is_empty(), "{cuts:?}");
        }

        #[test]
        fn one_line_keeps_short_text_and_cuts_long_text_with_the_whole_in_a_tooltip() {
            assert_eq!(crate::screens::settings::one_line("Short."), ("Short.".to_owned(), None));
            let long = "x".repeat(200);
            let (shown, full) = crate::screens::settings::one_line(&long);
            assert!(shown.chars().count() <= crate::screens::settings::HELP_MAX);
            assert_eq!(full.as_deref(), Some(long.as_str()));
        }
    }
}
