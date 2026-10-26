// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The budget: accounts of lines (quantity × rate), a top sheet, costs per
//! scene and shot worked out from the breakdown and the schedule, actuals
//! against budget, any currency, and the money AI generation spends,
//! recorded as each run is paid for.
//!
//! The template rates are rough indicative defaults to be replaced with
//! real quotes; they are not a price list.

use crate::breakdown::{Category, facts};
use crate::{Production, now};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use crate::story::Story;

/// Where an account sits on the top sheet.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Section {
    /// Story, producers, director, principal cast.
    Atl,
    /// Crew, equipment, locations, art, production.
    Btl,
    /// Editing, VFX, sound, music, finishing.
    Post,
    /// Insurance, legal, overheads, delivery.
    Other,
}

impl Section {
    /// Every section in top-sheet order.
    pub const ALL: [Section; 4] = [Section::Atl, Section::Btl, Section::Post, Section::Other];

    /// Heading.
    pub fn title(self) -> &'static str {
        match self {
            Section::Atl => "Above the line",
            Section::Btl => "Below the line",
            Section::Post => "Post-production",
            Section::Other => "Other",
        }
    }

    /// Reads `atl`, `btl`, `post`, `other`.
    pub fn parse(s: &str) -> Option<Section> {
        Some(match s.trim().to_lowercase().as_str() {
            "atl" | "above" | "above_the_line" => Section::Atl,
            "btl" | "below" | "below_the_line" | "production" => Section::Btl,
            "post" | "post_production" | "post-production" => Section::Post,
            "other" | "misc" | "overhead" => Section::Other,
            _ => return None,
        })
    }
}

/// A line whose actuals come from somewhere else.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Feed {
    /// The recorded AI generation spend ([`Production::spend`]).
    AiSpend,
}

/// Money actually spent on a line.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Actual {
    /// Seconds since the epoch.
    pub at: u64,
    /// How much, in `currency`.
    pub amount: f64,
    /// The currency (the line's, else the budget's).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub currency: Option<String>,
    /// What it was (invoice, receipt).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
}

/// One line of an account.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Line {
    /// Unique id.
    pub id: u64,
    /// What it pays for.
    pub desc: String,
    /// How many units.
    pub qty: f64,
    /// Of what: `days`, `weeks`, `flat`, `shots`, `seconds`…
    pub unit: String,
    /// Times (people, cameras…).
    #[serde(default = "one")]
    pub mult: f64,
    /// Price of one unit in `currency`.
    pub rate: f64,
    /// Currency of the rate; the budget's when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub currency: Option<String>,
    /// Pay of people: fringes (payroll taxes, pension…) are added.
    #[serde(default)]
    pub labour: bool,
    /// The scene this is for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scene: Option<String>,
    /// The shot this is for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shot: Option<String>,
    /// The breakdown element it pays for: spread over its scenes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub element: Option<String>,
    /// Actuals come from elsewhere.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feed: Option<Feed>,
    /// Money spent.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actuals: Vec<Actual>,
    /// Finished: the actuals are the final cost, whatever the estimate said.
    #[serde(default)]
    pub closed: bool,
    /// Anything to remember.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
}

fn one() -> f64 {
    1.0
}

/// A budget account.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Account {
    /// Code, `1100`.
    pub code: String,
    /// Name.
    pub name: String,
    /// Top-sheet section.
    pub section: Section,
    /// Lines.
    #[serde(default)]
    pub lines: Vec<Line>,
}

/// One AI generation run, paid for.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Spend {
    /// Unique id.
    #[serde(default)]
    pub id: u64,
    /// Seconds since the epoch (0 = now).
    #[serde(default)]
    pub at: u64,
    /// `openai`, `codex`…
    pub provider: String,
    /// The model id.
    pub model: String,
    /// `picture`, `video`, `music`, `sound`.
    #[serde(default)]
    pub kind: String,
    /// What it cost, in US cents.
    pub cents: u64,
    /// The prompt, shortened.
    #[serde(default)]
    pub prompt: String,
    /// The scene it was for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scene: Option<String>,
    /// The shot it was for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shot: Option<String>,
    /// Who ran it (`app`, `claude`…).
    #[serde(default)]
    pub by: String,
    /// The files it made.
    #[serde(default)]
    pub files: Vec<String>,
}

/// The whole budget.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Budget {
    /// Name (`Locked budget v2`).
    #[serde(default)]
    pub name: String,
    /// The currency totals are in.
    pub currency: String,
    /// What one unit of each currency is worth in `currency`.
    #[serde(default)]
    pub rates: BTreeMap<String, f64>,
    /// Added to labour lines (payroll taxes, pension, insurance), percent.
    #[serde(default)]
    pub fringe_pct: f64,
    /// Held back for the unforeseen, percent of the rest.
    #[serde(default)]
    pub contingency_pct: f64,
    /// The accounts.
    #[serde(default)]
    pub accounts: Vec<Account>,
}

/// Approximate values of one unit in US dollars, for a starting point.
/// Replace them with the day's rates for a real budget.
pub const DEFAULT_USD_RATES: &[(&str, f64)] =
    &[("USD", 1.0), ("EUR", 1.08), ("GBP", 1.27), ("INR", 0.012), ("AED", 0.272), ("CAD", 0.73), ("AUD", 0.65), ("JPY", 0.0067), ("CNY", 0.14), ("KRW", 0.00073), ("MXN", 0.055), ("BRL", 0.18), ("SGD", 0.74), ("CHF", 1.12)];

impl Default for Budget {
    fn default() -> Self {
        Budget::new("USD")
    }
}

impl Budget {
    /// An empty budget in a currency, with the default rate table.
    pub fn new(currency: &str) -> Budget {
        let mut b = Budget { name: String::new(), currency: "USD".into(), rates: DEFAULT_USD_RATES.iter().map(|(c, r)| (c.to_string(), *r)).collect(), fringe_pct: 0.0, contingency_pct: 0.0, accounts: Vec::new() };
        let _ = b.set_currency(currency);
        b
    }

    /// Changes the currency totals are shown in; the rate table is
    /// re-expressed in it and all amounts stay worth the same.
    pub fn set_currency(&mut self, new: &str) -> Result<(), String> {
        let new = new.trim().to_uppercase();
        if new.len() != 3 || !new.chars().all(|c| c.is_ascii_alphabetic()) {
            return Err(format!("\"{new}\" is not a currency code (like USD, EUR, INR)"));
        }
        let per_new = if new == self.currency { 1.0 } else { *self.rates.get(&new).ok_or_else(|| format!("no rate for {new}: set one with rates first"))? };
        let old = self.currency.clone();
        let mut rates: BTreeMap<String, f64> = self.rates.iter().map(|(c, r)| (c.clone(), r / per_new)).collect();
        rates.insert(new.clone(), 1.0);
        rates.entry(old).or_insert(1.0 / per_new);
        self.rates = rates;
        self.currency = new;
        Ok(())
    }

    /// Sets what one unit of `code` is worth in the budget currency.
    pub fn set_rate(&mut self, code: &str, value: f64) -> Result<(), String> {
        let code = code.trim().to_uppercase();
        if value <= 0.0 || !value.is_finite() {
            return Err("a rate must be above zero".into());
        }
        if code == self.currency {
            return Err(format!("{code} is the budget currency, always 1"));
        }
        self.rates.insert(code, value);
        Ok(())
    }

    /// Converts an amount in `from` to the budget currency.
    pub fn convert(&self, amount: f64, from: Option<&str>) -> f64 {
        match from {
            None => amount,
            Some(c) if c.eq_ignore_ascii_case(&self.currency) => amount,
            Some(c) => amount * self.rates.get(&c.to_uppercase()).copied().unwrap_or(1.0),
        }
    }

    /// An account by code or name.
    pub fn account(&self, key: &str) -> Option<&Account> {
        self.accounts.iter().find(|a| a.code == key || a.name.eq_ignore_ascii_case(key))
    }

    /// The estimated cost of a line, in the budget currency.
    pub fn line_budget(&self, l: &Line) -> f64 {
        self.convert(l.qty * l.mult * l.rate, l.currency.as_deref())
    }

    /// The money spent on a line (not from a feed), in the budget currency.
    pub fn line_actual(&self, l: &Line) -> f64 {
        l.actuals.iter().map(|a| self.convert(a.amount, a.currency.as_deref().or(l.currency.as_deref()))).sum()
    }
}

/// Formats money: thousands separators (lakh grouping for rupees), a
/// symbol where there is a common one.
pub fn money(amount: f64, currency: &str) -> String {
    let neg = amount < 0.0;
    let decimals = if matches!(currency, "JPY" | "KRW") || amount.abs() >= 1000.0 { 0 } else { 2 };
    let s = format!("{:.*}", decimals, amount.abs());
    let (int, frac) = s.split_once('.').map(|(a, b)| (a.to_owned(), format!(".{b}"))).unwrap_or((s.clone(), String::new()));
    let grouped = if currency == "INR" && int.len() > 3 {
        let (head, tail) = int.split_at(int.len() - 3);
        let mut parts: Vec<String> = Vec::new();
        let h: Vec<char> = head.chars().collect();
        let mut i = h.len();
        while i > 0 {
            let a = i.saturating_sub(2);
            parts.push(h[a..i].iter().collect());
            i = a;
        }
        parts.reverse();
        format!("{},{tail}", parts.join(","))
    } else {
        let mut out = String::new();
        for (i, c) in int.chars().enumerate() {
            if i > 0 && (int.len() - i) % 3 == 0 {
                out.push(',');
            }
            out.push(c);
        }
        out
    };
    let sym = match currency {
        "USD" => "$",
        "EUR" => "€",
        "GBP" => "£",
        "INR" => "₹",
        "JPY" | "CNY" => "¥",
        _ => "",
    };
    let prefix = if sym.is_empty() { format!("{currency} ") } else { sym.to_owned() };
    format!("{}{prefix}{grouped}{frac}", if neg { "-" } else { "" })
}

/// Money in the plain Latin set a PDF's built-in fonts can draw.
pub fn money_ascii(amount: f64, currency: &str) -> String {
    let m = money(amount, currency);
    m.replace('€', "EUR ").replace('£', "GBP ").replace('₹', "INR ").replace('¥', "JPY ")
}

/// One account on the top sheet.
#[derive(Clone, Debug, PartialEq)]
pub struct AccountRow {
    /// Code.
    pub code: String,
    /// Name.
    pub name: String,
    /// Section.
    pub section: Section,
    /// Budgeted (with fringes).
    pub budget: f64,
    /// Spent.
    pub actual: f64,
    /// Estimate at completion: what it will cost.
    pub eac: f64,
}

impl AccountRow {
    /// Budget minus estimate at completion.
    pub fn variance(&self) -> f64 {
        self.budget - self.eac
    }
}

/// The top sheet.
#[derive(Clone, Debug, PartialEq)]
pub struct BudgetReport {
    /// Currency of every figure.
    pub currency: String,
    /// Accounts.
    pub accounts: Vec<AccountRow>,
    /// Section totals: budget, actual, eac.
    pub sections: Vec<(Section, f64, f64, f64)>,
    /// Sum of the accounts.
    pub subtotal: f64,
    /// Contingency.
    pub contingency: f64,
    /// Budget with contingency.
    pub total: f64,
    /// All money spent, AI included.
    pub actual: f64,
    /// Estimate at completion, contingency not included.
    pub eac: f64,
    /// Total minus eac.
    pub variance: f64,
    /// AI generation spend so far.
    pub ai_spend: f64,
    /// AI spend no budget line takes.
    pub ai_unbudgeted: f64,
}

/// Costs of one shot.
#[derive(Clone, Debug, PartialEq)]
pub struct ShotCost {
    /// Shot id.
    pub shot: String,
    /// Budgeted share.
    pub budget: f64,
    /// Spent share.
    pub actual: f64,
    /// Seconds of the shot.
    pub secs: f64,
}

/// Costs of one scene.
#[derive(Clone, Debug, PartialEq)]
pub struct SceneCost {
    /// Scene id.
    pub scene: String,
    /// Budgeted: what lines name the scene or its elements, plus a share
    /// of everything shared.
    pub budget: f64,
    /// Of which named directly (the scene, its shots, its elements).
    pub direct: f64,
    /// Spent.
    pub actual: f64,
    /// Seconds of film.
    pub secs: f64,
    /// Its shots.
    pub shots: Vec<ShotCost>,
}

impl Production {
    /// The top sheet.
    pub fn budget_report(&self) -> BudgetReport {
        let b = &self.budget;
        let ai_total = self.spend.iter().map(|s| s.cents as f64 / 100.0).sum::<f64>();
        let ai = b.convert(ai_total, Some("USD"));
        let mut ai_left = ai;
        let mut accounts = Vec::new();
        let mut fed = false;
        for a in &b.accounts {
            let (mut bud, mut act, mut eac, mut lab) = (0.0, 0.0, 0.0, 0.0);
            for l in &a.lines {
                let est = b.line_budget(l);
                let mut spent = b.line_actual(l);
                if l.feed == Some(Feed::AiSpend) && ai_left > 0.0 {
                    spent += ai_left;
                    ai_left = 0.0;
                    fed = true;
                }
                bud += est;
                act += spent;
                eac += if l.closed { spent } else { est.max(spent) };
                if l.labour {
                    lab += est;
                }
            }
            let fringe = lab * b.fringe_pct / 100.0;
            accounts.push(AccountRow { code: a.code.clone(), name: a.name.clone(), section: a.section, budget: bud + fringe, actual: act, eac: eac + fringe });
        }
        let unbudgeted = if fed || ai_left <= 0.0 { 0.0 } else { ai_left };
        let sections: Vec<(Section, f64, f64, f64)> = Section::ALL
            .iter()
            .map(|s| {
                let rows = accounts.iter().filter(|a| a.section == *s);
                (*s, rows.clone().map(|a| a.budget).sum::<f64>(), rows.clone().map(|a| a.actual).sum::<f64>(), rows.map(|a| a.eac).sum::<f64>())
            })
            .collect();
        let subtotal: f64 = accounts.iter().map(|a| a.budget).sum();
        let contingency = subtotal * b.contingency_pct / 100.0;
        let total = subtotal + contingency;
        let actual = accounts.iter().map(|a| a.actual).sum::<f64>() + unbudgeted;
        let eac = accounts.iter().map(|a| a.eac).sum::<f64>() + unbudgeted;
        BudgetReport { currency: b.currency.clone(), accounts, sections, subtotal, contingency, total, actual, eac, variance: total - eac, ai_spend: ai, ai_unbudgeted: unbudgeted }
    }

    /// Adds a line to an account (making the account when it is new).
    #[allow(clippy::too_many_arguments)]
    pub fn add_line(&mut self, account: &str, desc: &str, qty: f64, unit: &str, rate: f64, currency: Option<&str>, labour: bool) -> Result<u64, String> {
        if desc.trim().is_empty() {
            return Err("a line needs a description".into());
        }
        if !(qty.is_finite() && rate.is_finite()) {
            return Err("qty and rate must be numbers".into());
        }
        let ai = self.alloc_id();
        let id = ai;
        let b = &mut self.budget;
        let idx = match b.accounts.iter().position(|a| a.code == account || a.name.eq_ignore_ascii_case(account)) {
            Some(i) => i,
            None => {
                b.accounts.push(Account { code: account.to_owned(), name: account.to_owned(), section: Section::Btl, lines: Vec::new() });
                b.accounts.len() - 1
            }
        };
        let cur = currency.map(|c| c.to_uppercase()).filter(|c| *c != b.currency);
        if let Some(c) = &cur {
            if !b.rates.contains_key(c) {
                return Err(format!("no exchange rate for {c}: set one first (budget currency {})", b.currency));
            }
        }
        b.accounts[idx].lines.push(Line { id, desc: desc.to_owned(), qty, unit: unit.to_owned(), mult: 1.0, rate, currency: cur, labour, scene: None, shot: None, element: None, feed: None, actuals: Vec::new(), closed: false, note: String::new() });
        Ok(id)
    }

    /// A line by id.
    pub fn line_mut(&mut self, id: u64) -> Option<&mut Line> {
        self.budget.accounts.iter_mut().flat_map(|a| a.lines.iter_mut()).find(|l| l.id == id)
    }

    /// Records money spent on a line.
    pub fn add_actual(&mut self, line: u64, amount: f64, currency: Option<&str>, note: &str) -> Result<(), String> {
        let base = self.budget.currency.clone();
        let known: Vec<String> = self.budget.rates.keys().cloned().collect();
        let cur = currency.map(|c| c.to_uppercase()).filter(|c| *c != base);
        if let Some(c) = &cur {
            if !known.contains(c) {
                return Err(format!("no exchange rate for {c}"));
            }
        }
        let l = self.line_mut(line).ok_or_else(|| format!("no budget line {line}"))?;
        l.actuals.push(Actual { at: now(), amount, currency: cur, note: note.to_owned() });
        let d = l.desc.clone();
        self.log("studio", format!("actual {amount:.2} on {d}"));
        Ok(())
    }

    /// Where the money goes: costs per scene and per shot, budgeted and spent.
    ///
    /// A line naming a shot goes to it; one naming a scene goes to the
    /// scene (and is shared out over its shots by length); one naming a
    /// breakdown element is spread over the scenes it is in by length;
    /// everything else is shared over the whole film by length. AI spend
    /// goes to the shot or scene it was made for, else the whole film.
    pub fn scene_costs(&self, story: &Story) -> Vec<SceneCost> {
        let f = facts(story);
        let mut out: Vec<SceneCost> = story
            .scenes
            .iter()
            .zip(&f)
            .map(|(s, fc)| SceneCost {
                scene: s.id.clone(),
                budget: 0.0,
                direct: 0.0,
                actual: 0.0,
                secs: fc.secs,
                shots: s.shots.iter().map(|h| ShotCost { shot: h.id.clone(), budget: 0.0, actual: 0.0, secs: h.duration as f64 / crate::story::SECOND as f64 }).collect(),
            })
            .collect();
        if out.is_empty() {
            return out;
        }
        let total_secs: f64 = out.iter().map(|s| s.secs).sum::<f64>().max(1e-9);
        // Put `(budget, actual)` on one scene (by index), shared out over its shots.
        fn put(out: &mut [SceneCost], si: usize, bud: f64, act: f64, direct: bool) {
            let sc = &mut out[si];
            sc.budget += bud;
            sc.actual += act;
            if direct {
                sc.direct += bud;
            }
            let secs: f64 = sc.shots.iter().map(|h| h.secs).sum();
            let n = sc.shots.len() as f64;
            for h in &mut sc.shots {
                let w = if secs > 0.0 { h.secs / secs } else { 1.0 / n.max(1.0) };
                h.budget += bud * w;
                h.actual += act * w;
            }
        }
        let by_secs = |out: &mut [SceneCost], idx: &[usize], bud: f64, act: f64, direct: bool| {
            let w: f64 = idx.iter().map(|i| out[*i].secs).sum::<f64>().max(1e-9);
            let shares: Vec<(usize, f64)> = idx.iter().map(|i| (*i, out[*i].secs / w)).collect();
            for (i, share) in shares {
                put(out, i, bud * share, act * share, direct);
            }
        };
        let all: Vec<usize> = (0..out.len()).collect();
        let scene_idx = |id: &str| story.scenes.iter().position(|s| s.id.eq_ignore_ascii_case(id));
        let b = &self.budget;
        let mut ai_left = true;
        let ai_total_line_target = b.accounts.iter().flat_map(|a| a.lines.iter()).find(|l| l.feed == Some(Feed::AiSpend)).map(|l| l.id);
        for l in b.accounts.iter().flat_map(|a| a.lines.iter()) {
            let bud = b.line_budget(l);
            let mut act = b.line_actual(l);
            if Some(l.id) == ai_total_line_target && ai_left {
                // AI spend without a shot or scene is shared over the film below.
                ai_left = false;
            }
            if l.feed == Some(Feed::AiSpend) {
                act = 0.0; // placed per run below
            }
            if let Some(sh) = &l.shot {
                if let Some((si, hi)) = story.find_shot(sh) {
                    let sc = &mut out[si];
                    sc.budget += bud;
                    sc.direct += bud;
                    sc.actual += act;
                    sc.shots[hi].budget += bud;
                    sc.shots[hi].actual += act;
                    continue;
                }
            }
            if let Some(si) = l.scene.as_deref().and_then(scene_idx) {
                put(&mut out, si, bud, act, true);
                continue;
            }
            if let Some(e) = &l.element {
                let idx: Vec<usize> = self.scenes_of(e).iter().filter_map(|s| scene_idx(s)).collect();
                if !idx.is_empty() {
                    by_secs(&mut out, &idx, bud, act, true);
                    continue;
                }
            }
            by_secs(&mut out, &all, bud, act, false);
        }
        for s in &self.spend {
            let usd = s.cents as f64 / 100.0;
            let amt = b.convert(usd, Some("USD"));
            if let Some((si, hi)) = s.shot.as_deref().and_then(|x| story.find_shot(x)) {
                out[si].actual += amt;
                out[si].shots[hi].actual += amt;
            } else if let Some(si) = s.scene.as_deref().and_then(scene_idx) {
                put(&mut out, si, 0.0, amt, false);
            } else {
                by_secs(&mut out, &all, 0.0, amt, false);
            }
        }
        let _ = total_secs;
        out
    }
}

// ------------------------------------------------------------- templates

/// Budget template names.
pub const TEMPLATES: &[(&str, &str)] = &[
    ("live_action_feature", "A live-action feature: full crew, cast by day, locations, art, camera, stunts, post, insurance"),
    ("live_action_short", "A live-action short: a small crew, a few shoot days, light post"),
    ("animated_short", "An animated film: design, layout, animation, lighting, comp, sound, music, per shot-second"),
    ("ai_film", "A film generated with AI models: writing, generation spend per shot with retakes, voice, music, edit and finish"),
];

impl Production {
    /// Replaces the budget with a template sized to this film: cast lines
    /// by the days each actor works, locations by days at each, art by the
    /// props, wardrobe and vehicles in the breakdown, VFX by the VFX
    /// elements, animation by shot-seconds.
    ///
    /// `scale` multiplies every rate (0.5 for a half-price production).
    pub fn apply_budget_template(&mut self, story: &Story, name: &str, scale: f64) -> Result<usize, String> {
        let kind = match name.trim().to_lowercase().replace([' ', '-'], "_").as_str() {
            "live_action_feature" | "feature" => "live_action_feature",
            "live_action_short" | "short" | "live_short" => "live_action_short",
            "animated_short" | "animation" | "animated" => "animated_short",
            "ai_film" | "ai" | "generated" => "ai_film",
            other => return Err(format!("no template \"{other}\" (live_action_feature, live_action_short, animated_short, ai_film)")),
        };
        if !(scale.is_finite() && scale > 0.0) {
            return Err("scale must be above zero".into());
        }
        let fx = facts(story);
        let minutes = (fx.iter().map(|f| f.secs).sum::<f64>() / 60.0).max(0.25);
        let shots: Vec<(String, f64)> = story.scenes.iter().flat_map(|s| s.shots.iter().map(|h| (h.id.clone(), h.duration as f64 / crate::story::SECOND as f64))).collect();
        let shot_secs: f64 = shots.iter().map(|s| s.1).sum::<f64>().max(minutes * 60.0);
        let shot_count = shots.len().max(fx.len()) as f64;
        let days = if self.schedule.days.is_empty() { (minutes / self.settings.minutes_per_day()).ceil().max(1.0) } else { self.schedule.days.len() as f64 };
        let weeks = (days / 5.0).ceil().max(1.0);
        let currency = self.budget.currency.clone();
        let mut b = Budget { name: kind.replace('_', " "), currency: currency.clone(), rates: self.budget.rates.clone(), fringe_pct: 0.0, contingency_pct: 10.0, accounts: Vec::new() };
        let mut next = self.next_id;
        let mut acct = |code: &str, name: &str, section| b.accounts.push(Account { code: code.into(), name: name.into(), section, lines: Vec::new() });
        let mut lines: Vec<(usize, Line)> = Vec::new();
        let mut mk = |a: usize, desc: &str, qty: f64, unit: &str, rate: f64, labour: bool, scene: Option<&str>, element: Option<&str>, feed: Option<Feed>| {
            next += 1;
            lines.push((
                a,
                Line {
                    id: next,
                    desc: desc.to_owned(),
                    qty: (qty * 100.0).round() / 100.0,
                    unit: unit.to_owned(),
                    mult: 1.0,
                    rate: (rate * scale * 100.0).round() / 100.0,
                    currency: Some("USD".into()),
                    labour,
                    scene: scene.map(str::to_owned),
                    shot: None,
                    element: element.map(str::to_owned),
                    feed,
                    actuals: Vec::new(),
                    closed: false,
                    note: String::new(),
                },
            ));
        };
        let cast: Vec<&crate::Element> = self.of(Category::Cast).collect();
        let count = |c: Category| self.of(c).count() as f64;
        match kind {
            "live_action_feature" | "live_action_short" => {
                let feature = kind == "live_action_feature";
                let k = if feature { 1.0 } else { 0.3 };
                for (code, n, s) in [
                    ("1100", "Story and rights", Section::Atl),
                    ("1200", "Producers", Section::Atl),
                    ("1300", "Direction", Section::Atl),
                    ("1400", "Cast", Section::Atl),
                    ("2000", "Production staff", Section::Btl),
                    ("2100", "Art department", Section::Btl),
                    ("2200", "Camera", Section::Btl),
                    ("2300", "Lighting and grip", Section::Btl),
                    ("2400", "Production sound", Section::Btl),
                    ("2500", "Locations", Section::Btl),
                    ("2600", "Transport", Section::Btl),
                    ("2700", "Catering", Section::Btl),
                    ("2800", "Wardrobe, make-up and hair", Section::Btl),
                    ("2900", "Stunts and special effects", Section::Btl),
                    ("3000", "Editing", Section::Post),
                    ("3100", "Visual effects", Section::Post),
                    ("3200", "Sound and music", Section::Post),
                    ("3300", "Colour and finishing", Section::Post),
                    ("4000", "Insurance, legal and fees", Section::Other),
                    ("4100", "AI generation", Section::Other),
                ] {
                    acct(code, n, s);
                }
                mk(0, "Screenplay", 1.0, "flat", 30_000.0 * k, true, None, None, None);
                mk(1, "Producers", weeks * 2.0, "weeks", 4_000.0 * k, true, None, None, None);
                mk(2, "Director", weeks + 4.0, "weeks", 6_000.0 * k, true, None, None, None);
                for (i, c) in cast.iter().enumerate() {
                    let d = self.work_days(&c.id).max(1) as f64;
                    let lead = i < 2;
                    mk(3, &format!("{} ({})", c.name, if lead { "lead" } else { "supporting" }), d, "days", (if lead { 1500.0 } else { 600.0 }) * k, true, None, Some(&c.id), None);
                }
                if count(Category::Extras) > 0.0 {
                    mk(3, "Background extras", days * 10.0, "days", 120.0 * k, true, None, None, None);
                }
                mk(4, "Line producer and coordinators", weeks, "weeks", 5_000.0 * k, true, None, None, None);
                mk(4, "Assistant directors", days, "days", 900.0 * k, true, None, None, None);
                for (n, f, p) in [("Props", Category::Props, 150.0), ("Set dressing and build", Category::Location, 400.0)] {
                    if f == Category::Props {
                        for e in self.of(f) {
                            mk(5, &format!("Prop: {}", e.name), 1.0, "flat", p * k, false, None, Some(&e.id), None);
                        }
                    } else {
                        mk(5, n, days, "days", p * k, false, None, None, None);
                    }
                }
                mk(6, "Camera package and operators", days, "days", 2_500.0 * k, false, None, None, None);
                mk(7, "Lighting and grip package", days, "days", 1_800.0 * k, false, None, None, None);
                mk(8, "Sound recordist and kit", days, "days", 700.0 * k, true, None, None, None);
                for e in self.of(Category::Location) {
                    let sc = self.scenes_of(&e.id).len().max(1) as f64;
                    let d = (self.schedule.days.iter().filter(|d| d.scenes.iter().any(|s| self.scenes_of(&e.id).contains(&s.as_str()))).count() as f64).max((sc / 2.0).ceil());
                    mk(9, &format!("Location: {}", e.name), d, "days", 900.0 * k, false, None, Some(&e.id), None);
                }
                for e in self.of(Category::Vehicles) {
                    mk(10, &format!("Vehicle: {}", e.name), 1.0, "days", 400.0 * k, false, None, Some(&e.id), None);
                }
                mk(10, "Unit transport", days, "days", 600.0 * k, false, None, None, None);
                mk(11, "Catering", days * 40.0 * if feature { 1.0 } else { 0.4 }, "meals", 18.0, false, None, None, None);
                for e in self.of(Category::Wardrobe) {
                    mk(12, &format!("Costume: {}", e.name), 1.0, "flat", 120.0 * k, false, None, Some(&e.id), None);
                }
                mk(12, "Make-up and hair", days, "days", 700.0 * k, true, None, None, None);
                for e in self.of(Category::Stunts).chain(self.of(Category::Sfx)) {
                    mk(13, &format!("{}: {}", e.category.title(), e.name), 1.0, "flat", 1_500.0 * k, false, None, Some(&e.id), None);
                }
                mk(14, "Editor and edit suite", weeks * 3.0 + minutes / 4.0, "weeks", 3_000.0 * k, true, None, None, None);
                for e in self.of(Category::Vfx) {
                    mk(15, &format!("VFX: {}", e.name), 1.0, "shots", 6_000.0 * k, false, None, Some(&e.id), None);
                }
                mk(16, "Sound design and mix", minutes, "minutes", 2_500.0 * k, false, None, None, None);
                mk(16, "Score and songs", minutes, "minutes", 3_000.0 * k, false, None, None, None);
                mk(17, "Colour grade and conform", minutes, "minutes", 1_200.0 * k, false, None, None, None);
                mk(18, "Production insurance", 1.0, "flat", 12_000.0 * k, false, None, None, None);
                mk(18, "Legal and clearances", 1.0, "flat", 8_000.0 * k, false, None, None, None);
                mk(19, "AI generation", 1.0, "flat", 0.0, false, None, None, Some(Feed::AiSpend));
                b.fringe_pct = if feature { 18.0 } else { 10.0 };
            }
            "animated_short" => {
                for (code, n, s) in [
                    ("1100", "Story and direction", Section::Atl),
                    ("1200", "Voice cast", Section::Atl),
                    ("2000", "Design", Section::Btl),
                    ("2100", "Layout", Section::Btl),
                    ("2200", "Animation", Section::Btl),
                    ("2300", "Lighting and rendering", Section::Btl),
                    ("2400", "Effects and compositing", Section::Btl),
                    ("2500", "Production management", Section::Btl),
                    ("3000", "Editing", Section::Post),
                    ("3100", "Sound and music", Section::Post),
                    ("4000", "Pipeline, compute and storage", Section::Other),
                    ("4100", "AI generation", Section::Other),
                ] {
                    acct(code, n, s);
                }
                mk(0, "Writer / director", weeks * 4.0, "weeks", 3_000.0, true, None, None, None);
                for c in &cast {
                    mk(1, &format!("Voice: {}", c.name), self.scenes_of(&c.id).len().max(1) as f64, "sessions", 400.0, true, None, Some(&c.id), None);
                }
                for e in self.of(Category::Cast).chain(self.of(Category::Props)).chain(self.of(Category::Vehicles)).chain(self.of(Category::Animals)) {
                    mk(2, &format!("Design and model: {}", e.name), 1.0, "assets", 900.0, true, None, Some(&e.id), None);
                }
                for e in self.of(Category::Location) {
                    mk(2, &format!("Environment: {}", e.name), 1.0, "sets", 1_800.0, true, None, Some(&e.id), None);
                }
                mk(3, "Layout and camera", shot_secs, "seconds", 90.0, true, None, None, None);
                mk(4, "Animation", shot_secs, "seconds", 420.0, true, None, None, None);
                mk(5, "Lighting", shot_secs, "seconds", 110.0, true, None, None, None);
                mk(5, "Render time", shot_secs, "seconds", 25.0, false, None, None, None);
                mk(6, "Compositing", shot_secs, "seconds", 100.0, true, None, None, None);
                for e in self.of(Category::Vfx).chain(self.of(Category::Sfx)) {
                    mk(6, &format!("Effects: {}", e.name), 1.0, "shots", 1_500.0, true, None, Some(&e.id), None);
                }
                mk(7, "Producer and coordinator", weeks * 4.0, "weeks", 2_200.0, true, None, None, None);
                mk(8, "Editor", weeks * 2.0, "weeks", 2_200.0, true, None, None, None);
                mk(9, "Sound design and mix", minutes, "minutes", 2_500.0, false, None, None, None);
                mk(9, "Score and songs", minutes, "minutes", 3_000.0, false, None, None, None);
                mk(10, "Render farm, storage and software", weeks * 4.0, "weeks", 450.0, false, None, None, None);
                mk(11, "AI generation", 1.0, "flat", 0.0, false, None, None, Some(Feed::AiSpend));
                b.fringe_pct = 12.0;
            }
            _ => {
                for (code, n, s) in [
                    ("1100", "Writing and direction", Section::Atl),
                    ("2000", "AI generation", Section::Btl),
                    ("2100", "Voice", Section::Btl),
                    ("2200", "Music and sound", Section::Btl),
                    ("3000", "Editing and finishing", Section::Post),
                    ("4000", "Software, storage and delivery", Section::Other),
                ] {
                    acct(code, n, s);
                }
                mk(0, "Writer / director / supervising producer", weeks.max(2.0), "weeks", 2_500.0, true, None, None, None);
                // A video shot costs a few dollars and takes three tries.
                mk(1, "Generated video, with three tries a shot", shot_count * 3.0, "runs", 2.5, false, None, None, Some(Feed::AiSpend));
                mk(1, "Generated pictures (boards, keyframes, backgrounds)", shot_count * 4.0, "runs", 0.12, false, None, None, None);
                mk(2, "Voice-over and dialogue", minutes, "minutes", 60.0, false, None, None, None);
                mk(3, "Music and sound effects", minutes, "minutes", 80.0, false, None, None, None);
                mk(4, "Editing, grade and mix", weeks.max(1.0), "weeks", 1_800.0, true, None, None, None);
                mk(5, "Storage, software and delivery", 1.0, "flat", 300.0, false, None, None, None);
                b.contingency_pct = 20.0;
            }
        }
        let n = lines.len();
        for (a, l) in lines {
            b.accounts[a].lines.push(l);
        }
        // Accounts with nothing in them are not shown.
        b.accounts.retain(|a| !a.lines.is_empty());
        self.next_id = next;
        self.budget = b;
        self.log("studio", format!("budget from template {kind}: {n} lines"));
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::story::SECOND;
    use crate::story::{Character, Line as SLine, Place, Scene, Shot};

    fn film() -> Story {
        let mut st = Story::default();
        st.cast.push(Character { id: "a".into(), name: "Asha".into(), ..Default::default() });
        st.cast.push(Character { id: "b".into(), name: "Bilal".into(), ..Default::default() });
        for (id, i) in [("field", false), ("hut", true)] {
            st.places.push(Place { id: id.into(), name: id.into(), interior: i, ..Default::default() });
        }
        let mk = |id: &str, place: &str, head: &str, cast: &[&str], shots: &[(&str, i64)], text: &str| Scene {
            id: id.into(),
            heading: head.into(),
            place: Some(place.into()),
            cast: cast.iter().map(|c| c.to_string()).collect(),
            lines: vec![SLine { text: text.into(), ..Default::default() }],
            shots: shots.iter().map(|(i, s)| Shot { id: (*i).into(), duration: s * SECOND, ..Default::default() }).collect(),
            ..Default::default()
        };
        st.scenes.push(mk("1", "field", "EXT. FIELD - DAY", &["a"], &[("1A", 30), ("1B", 30)], "Asha rides a horse across the field."));
        st.scenes.push(mk("2", "hut", "INT. HUT - NIGHT", &["a", "b"], &[("2A", 60)], "A dragon glows in the smoke."));
        st
    }

    fn near(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6 * b.abs().max(1.0)
    }

    #[test]
    fn currencies_convert_and_rebase_without_changing_worth() {
        let mut b = Budget::new("USD");
        assert!(near(b.convert(1000.0, Some("INR")), 12.0));
        b.set_currency("INR").unwrap();
        assert_eq!(b.currency, "INR");
        assert!(near(b.convert(1.0, Some("USD")), 83.3333333333), "{}", b.convert(1.0, Some("USD")));
        assert!(near(b.convert(1.0, Some("EUR")), 1.08 / 0.012));
        assert!(b.set_currency("ZZZ").is_err());
        assert!(b.set_currency("dollars").is_err());
        assert_eq!(money(12_345_678.0, "INR"), "₹1,23,45,678");
        assert_eq!(money(1234.6, "USD"), "$1,235");
        assert_eq!(money(12.5, "EUR"), "€12.50");
        assert_eq!(money(-5.0, "CHF"), "-CHF 5.00");
        assert_eq!(money_ascii(7.0, "GBP"), "GBP 7.00");
    }

    #[test]
    fn the_top_sheet_adds_fringes_contingency_and_variance() {
        let mut p = Production::default();
        p.budget.fringe_pct = 10.0;
        p.budget.contingency_pct = 10.0;
        p.budget.accounts.push(Account { code: "1100".into(), name: "Cast".into(), section: Section::Atl, lines: vec![] });
        p.budget.accounts.push(Account { code: "2100".into(), name: "Art".into(), section: Section::Btl, lines: vec![] });
        let cast = p.add_line("1100", "Lead", 5.0, "days", 1000.0, None, true).unwrap();
        let art = p.add_line("2100", "Props", 1.0, "flat", 500.0, Some("EUR"), false).unwrap();
        assert!(p.add_line("2100", "x", 1.0, "flat", 1.0, Some("ZZZ"), false).is_err());
        let r = p.budget_report();
        assert!(near(r.accounts[0].budget, 5500.0), "labour plus 10%: {}", r.accounts[0].budget);
        assert!(near(r.accounts[1].budget, 540.0), "500 EUR at 1.08");
        assert!(near(r.subtotal, 6040.0));
        assert!(near(r.contingency, 604.0));
        assert!(near(r.total, 6644.0));
        // The lead cost more than planned; the props came in under.
        p.add_actual(cast, 6000.0, None, "invoice 1").unwrap();
        p.add_actual(art, 400.0, Some("EUR"), "receipt").unwrap();
        p.line_mut(art).unwrap().closed = true;
        let r = p.budget_report();
        assert!(near(r.accounts[0].actual, 6000.0));
        assert!(near(r.accounts[0].eac, 6500.0), "overspent labour: 6000 plus fringes on the 5000 planned: {}", r.accounts[0].eac);
        assert!(near(r.accounts[1].eac, 432.0), "a closed line is what was spent");
        assert!(near(r.actual, 6000.0 + 432.0));
        assert!(near(r.variance, 6644.0 - (6500.0 + 432.0)));
        assert!(p.add_actual(999, 1.0, None, "").is_err());
    }

    #[test]
    fn templates_are_sized_from_the_breakdown_and_schedule() {
        let st = film();
        let mut p = Production::default();
        p.auto_tag(&st);
        p.settings.start_date = Some("2026-11-02".into());
        p.auto_schedule(&st);
        let n = p.apply_budget_template(&st, "live_action_short", 1.0).unwrap();
        assert!(n > 10);
        let desc: Vec<&str> = p.budget.accounts.iter().flat_map(|a| a.lines.iter()).map(|l| l.desc.as_str()).collect();
        assert!(desc.iter().any(|d| d.starts_with("Asha")), "{desc:?}");
        assert!(desc.iter().any(|d| d.starts_with("VFX: dragon")), "a VFX line per VFX element: {desc:?}");
        assert!(desc.iter().any(|d| d.starts_with("Location: field")));
        let asha = p.budget.accounts.iter().flat_map(|a| a.lines.iter()).find(|l| l.desc.starts_with("Asha")).unwrap();
        assert_eq!(asha.qty, p.work_days("cast:asha") as f64);
        assert!(p.budget_report().total > 0.0);
        let half = {
            let mut q = p.clone();
            q.apply_budget_template(&st, "live_action_short", 0.5).unwrap();
            q.budget_report().total
        };
        assert!(half < p.budget_report().total * 0.6, "scale halves the rates");
        assert!(p.apply_budget_template(&st, "weird", 1.0).is_err());
        for (t, _) in TEMPLATES {
            let mut q = Production::default();
            q.auto_tag(&st);
            q.apply_budget_template(&st, t, 1.0).unwrap();
            assert!(q.budget_report().total > 0.0, "{t}");
        }
    }

    #[test]
    fn money_flows_to_scenes_and_shots_and_ai_spend_is_tracked() {
        let st = film();
        let mut p = Production::default();
        p.auto_tag(&st);
        p.budget.accounts.push(Account { code: "1".into(), name: "All".into(), section: Section::Btl, lines: vec![] });
        let horse = p.add_line("1", "Horse wrangler", 1.0, "flat", 600.0, None, false).unwrap();
        p.line_mut(horse).unwrap().element = Some("animals:horse".into());
        let shot = p.add_line("1", "Crane for 2A", 1.0, "flat", 300.0, None, false).unwrap();
        p.line_mut(shot).unwrap().shot = Some("2A".into());
        let _ = p.add_line("1", "Office", 1.0, "flat", 120.0, None, false).unwrap();
        let ai = p.add_line("1", "AI generation", 1.0, "flat", 50.0, None, false).unwrap();
        p.line_mut(ai).unwrap().feed = Some(Feed::AiSpend);
        let c = p.scene_costs(&st);
        // Scene 1 is 60 s, scene 2 is 60 s: the office is shared evenly.
        assert!(near(c[0].budget, 685.0), "scene 1: horse + half the office + half the ai line: {}", c[0].budget);
        assert!(near(c[0].shots[0].budget, 342.5), "{}", c[0].shots[0].budget);
        assert!(near(c[1].shots[0].budget, 385.0));
        let total: f64 = c.iter().map(|s| s.budget).sum();
        assert!(near(total, 600.0 + 300.0 + 120.0 + 50.0), "every dollar lands somewhere: {total}");
        // Spend recorded by the app lands on the shot it was made for.
        p.record_spend(Spend { provider: "openai".into(), model: "video-model".into(), kind: "video".into(), cents: 250, shot: Some("1B".into()), by: "claude".into(), ..Default::default() });
        p.record_spend(Spend { provider: "openai".into(), model: "image-model".into(), kind: "picture".into(), cents: 10, ..Default::default() });
        let c = p.scene_costs(&st);
        assert!(near(c[0].shots[1].actual, 2.50 + 0.10 * 0.25), "{:?}", c[0].shots[1]);
        let r = p.budget_report();
        assert!(near(r.ai_spend, 2.60));
        assert!(near(r.accounts[0].actual, 2.60), "the AI line takes the spend as actuals: {}", r.accounts[0].actual);
        assert!(near(r.ai_unbudgeted, 0.0));
        assert_eq!(p.spend[0].id + 1, p.spend[1].id);
        // With no AI line the spend still counts, flagged as unbudgeted.
        p.budget.accounts[0].lines.retain(|l| l.feed.is_none());
        let r = p.budget_report();
        assert!(near(r.ai_unbudgeted, 2.60));
        assert!(near(r.actual, 2.60));
    }
}
