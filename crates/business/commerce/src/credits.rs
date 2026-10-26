// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Hosted AI credits: what each task costs ([`PriceTable`]), a local usage
//! ledger, a monthly budget with warnings, and a [`Quote`] the AI shows the
//! person before it spends anything.
//!
//! Bring-your-own-key is always allowed: a task run with the person's own
//! key costs no credits and is never blocked by the plan or the budget (it
//! is billed by their provider; the quote says roughly how much).

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// One task's price: what the provider charges per unit and how many
/// credits Studio charges for it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskPrice {
    /// `transcribe`, `voice`, `llm`, ...
    pub task: String,
    /// What a unit is: `minute`, `image`, `second`, `1k characters`, ...
    pub unit: String,
    pub provider: String,
    #[serde(default)]
    pub model: String,
    /// The provider's cost per unit, in millionths of the currency.
    pub cost_micros: u64,
    /// Credits charged per unit.
    pub credits: u64,
}

/// What a credit is worth and every task's price. Served by the cloud
/// server (`GET /v1/prices`) and cached for offline quotes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PriceTable {
    pub schema: u32,
    /// What one credit sells for, in millionths of `currency`.
    pub credit_value_micros: u64,
    pub currency: String,
    pub tasks: Vec<TaskPrice>,
}

impl PriceTable {
    pub fn parse(json: &str) -> Result<PriceTable, String> {
        let t: PriceTable = serde_json::from_str(json).map_err(|e| format!("the price table is not valid: {e}"))?;
        if t.schema != 1 {
            return Err(format!("the price table is schema {}, this Zornov reads 1", t.schema));
        }
        Ok(t)
    }

    pub fn task(&self, task: &str) -> Option<&TaskPrice> {
        self.tasks.iter().find(|t| t.task == task)
    }

    /// Tasks whose credits do not cover the provider's cost plus
    /// `margin_bps` (basis points; 0 means at least break even). A server
    /// refuses to start with any.
    pub fn uncovered(&self, margin_bps: u64) -> Vec<String> {
        self.tasks
            .iter()
            .filter(|t| (t.credits as u128) * (self.credit_value_micros as u128) * 10_000 < (t.cost_micros as u128) * (10_000 + margin_bps as u128))
            .map(|t| format!("{} ({}): {} credits = {} micros, the provider charges {} micros", t.task, t.unit, t.credits, t.credits * self.credit_value_micros, t.cost_micros))
            .collect()
    }
}

/// The person's monthly limit on credits (and when to warn).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Budget {
    /// Credits a month; `None` is no limit beyond the balance.
    #[serde(default)]
    pub monthly_limit: Option<u64>,
    /// Warn when a task would take the month past this share of the limit.
    #[serde(default = "eighty")]
    pub warn_at_percent: u8,
    /// Refuse tasks past the limit (else warn and go on).
    #[serde(default = "yes")]
    pub stop_at_limit: bool,
}

fn eighty() -> u8 {
    80
}
fn yes() -> bool {
    true
}

impl Default for Budget {
    fn default() -> Self {
        Budget { monthly_limit: None, warn_at_percent: 80, stop_at_limit: true }
    }
}

/// One line of the usage ledger.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    /// Unix seconds.
    pub at: u64,
    pub task: String,
    pub units: f64,
    pub credits: u64,
    /// Run with the person's own key (no credits).
    #[serde(default)]
    pub byok: bool,
    #[serde(default)]
    pub provider: String,
    /// The server's hold id, when metered there.
    #[serde(default)]
    pub hold: String,
    #[serde(default)]
    pub note: String,
}

/// The ledger (`usage.jsonl`) and budget (`budget.json`) in a folder.
pub struct Ledger {
    pub dir: PathBuf,
}

impl Ledger {
    pub fn new(dir: impl Into<PathBuf>) -> Ledger {
        Ledger { dir: dir.into() }
    }

    pub fn record(&self, u: &Usage) -> Result<(), String> {
        use std::io::Write;
        std::fs::create_dir_all(&self.dir).map_err(|e| e.to_string())?;
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(self.dir.join("usage.jsonl")).map_err(|e| format!("cannot write the usage ledger: {e}"))?;
        writeln!(f, "{}", serde_json::to_string(u).expect("serialisable")).map_err(|e| e.to_string())
    }

    pub fn entries(&self) -> Vec<Usage> {
        std::fs::read_to_string(self.dir.join("usage.jsonl")).unwrap_or_default().lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
    }

    /// Credits used in the calendar month (UTC) containing `now`.
    pub fn used_this_month(&self, now: u64) -> u64 {
        let m = month_of(now);
        self.entries().iter().filter(|u| month_of(u.at) == m).map(|u| u.credits).sum()
    }

    pub fn budget(&self) -> Budget {
        std::fs::read_to_string(self.dir.join("budget.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn set_budget(&self, b: &Budget) -> Result<(), String> {
        studio_entitlements::write_atomic(&self.dir.join("budget.json"), serde_json::to_string_pretty(b).expect("serialisable").as_bytes())
    }

    /// The cached price table.
    pub fn prices(&self) -> Option<PriceTable> {
        std::fs::read_to_string(self.dir.join("prices.json")).ok().and_then(|t| PriceTable::parse(&t).ok())
    }

    pub fn save_prices(&self, t: &PriceTable) -> Result<(), String> {
        studio_entitlements::write_atomic(&self.dir.join("prices.json"), serde_json::to_string_pretty(t).expect("serialisable").as_bytes())
    }
}

/// `(year, month)` of unix seconds, UTC.
pub fn month_of(secs: u64) -> (i64, u32) {
    // Howard Hinnant's civil-from-days.
    let z = (secs / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + if m <= 2 { 1 } else { 0 }, m)
}

/// What a task will cost, said before anything is spent.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Quote {
    pub task: String,
    pub units: f64,
    pub unit: String,
    pub provider: String,
    /// Credits it takes (0 with the person's own key).
    pub credits: u64,
    pub byok: bool,
    /// Roughly what the provider charges, in the table's currency.
    pub provider_cost: f64,
    pub currency: String,
    /// The balance before and after, when known.
    pub balance: Option<i64>,
    pub balance_after: Option<i64>,
    pub month_used: u64,
    pub monthly_limit: Option<u64>,
    pub warnings: Vec<String>,
    /// Whether it may run now.
    pub allowed: bool,
    /// One sentence for the person.
    pub say: String,
}

/// Prices `units` of `task`. `balance` is the hosted credit balance (None
/// when unknown, e.g. offline); `byok` when the person's own key will run it.
pub fn quote(table: &PriceTable, budget: &Budget, month_used: u64, balance: Option<i64>, task: &str, units: f64, byok: bool) -> Result<Quote, String> {
    let p = table.task(task).ok_or_else(|| format!("no price for \"{task}\"; tasks: {}", table.tasks.iter().map(|t| t.task.as_str()).collect::<Vec<_>>().join(", ")))?;
    if !(units.is_finite() && units >= 0.0) {
        return Err("units must be a number, 0 or more".into());
    }
    let provider_cost = units * p.cost_micros as f64 / 1e6;
    let mut q = Quote { task: task.into(), units, unit: p.unit.clone(), provider: p.provider.clone(), credits: 0, byok, provider_cost, currency: table.currency.clone(), balance, balance_after: balance, month_used, monthly_limit: budget.monthly_limit, warnings: vec![], allowed: true, say: String::new() };
    if byok {
        describe(&mut q);
        return Ok(q);
    }
    // Whole credits, never less than one for a non-empty task.
    let credits = ((units * p.credits as f64).ceil() as u64).max(if units > 0.0 { 1 } else { 0 });
    q.credits = credits;
    q.balance_after = balance.map(|b| b - credits as i64);
    if let Some(b) = balance
        && (credits as i64) > b {
            q.allowed = false;
            q.warnings.push(format!("you have {b} credits; this needs {credits}. Add credits, or use your own {} key.", p.provider));
        }
    if let Some(limit) = budget.monthly_limit {
        let after = month_used + credits;
        if after > limit {
            q.warnings.push(format!("this takes the month to {after} of your {limit}-credit budget"));
            if budget.stop_at_limit {
                q.allowed = false;
            }
        } else if after * 100 >= limit * budget.warn_at_percent as u64 {
            q.warnings.push(format!("this takes the month to {after} of your {limit}-credit budget ({}%)", after * 100 / limit.max(1)));
        }
    }
    describe(&mut q);
    Ok(q)
}

/// Writes the quote's sentence for the person (again, after warnings were
/// added).
pub fn describe(q: &mut Quote) {
    let what = format!("{} ({} × {})", q.task, fmt_units(q.units), q.unit);
    q.say = if q.byok {
        format!("{what} with your own {} key: no credits, about {} {:.2} billed by {}.", q.provider, q.currency.to_uppercase(), q.provider_cost, q.provider)
    } else {
        let left = match q.balance_after {
            Some(a) if q.allowed => format!(", leaving {a}"),
            _ => String::new(),
        };
        let warn = if q.warnings.is_empty() { String::new() } else { format!(" {}.", q.warnings.join("; ")) };
        format!("{what} costs {} credit{}{left}.{warn}{}", q.credits, if q.credits == 1 { "" } else { "s" }, if q.allowed { "" } else { " It will not run until that is sorted." })
    };
}

fn fmt_units(u: f64) -> String {
    if u.fract() == 0.0 { format!("{u:.0}") } else { format!("{u:.2}") }
}

/// Rough units a task will use, from what the AI knows about it.
pub fn estimate_units(task: &str, seconds: Option<f64>, characters: Option<u64>, count: Option<u64>, tokens: Option<u64>) -> Option<f64> {
    match task {
        "transcribe" => seconds.map(|s| s / 60.0),
        "voice" => characters.map(|c| c as f64 / 1000.0),
        "llm" => tokens.map(|t| t as f64 / 1000.0),
        _ => count.map(|c| c as f64).or(Some(1.0)),
    }
}

/// Reads a JSON file from `dir`.
pub(crate) fn read<T: for<'de> Deserialize<'de>>(dir: &Path, name: &str) -> Option<T> {
    std::fs::read_to_string(dir.join(name)).ok().and_then(|t| serde_json::from_str(&t).ok())
}

#[cfg(test)]
pub mod tests {
    use super::*;

    pub fn table() -> PriceTable {
        PriceTable::parse(
            r#"{"schema":1,"credit_value_micros":10000,"currency":"usd","tasks":[
            {"task":"transcribe","unit":"minute","provider":"openai","model":"whisper-1","cost_micros":6000,"credits":1},
            {"task":"describe.image","unit":"image","provider":"openai","cost_micros":30000,"credits":4},
            {"task":"voice","unit":"1k characters","provider":"elevenlabs","cost_micros":150000,"credits":10}]}"#,
        )
        .unwrap()
    }

    #[test]
    fn the_table_shows_which_tasks_lose_money() {
        let t = table();
        assert!(t.uncovered(0).iter().any(|s| s.starts_with("voice")), "{:?}", t.uncovered(0));
        assert_eq!(t.uncovered(0).len(), 1);
        // With a 50% margin the image price (4 credits = 0.04 for 0.03) is short too.
        assert_eq!(t.uncovered(5_000).len(), 2);
    }

    #[test]
    fn quotes_count_credits_balance_and_budget() {
        let t = table();
        let b = Budget { monthly_limit: Some(100), warn_at_percent: 80, stop_at_limit: true };
        let q = quote(&t, &b, 0, Some(50), "transcribe", 3.2, false).unwrap();
        assert_eq!(q.credits, 4);
        assert_eq!(q.balance_after, Some(46));
        assert!(q.allowed && q.warnings.is_empty(), "{q:?}");
        assert!(q.say.contains("4 credits"), "{}", q.say);

        let q = quote(&t, &b, 0, Some(3), "describe.image", 1.0, false).unwrap();
        assert!(!q.allowed);
        assert!(q.warnings[0].contains("own openai key"));

        let q = quote(&t, &b, 78, Some(500), "describe.image", 1.0, false).unwrap();
        assert!(q.allowed && q.warnings[0].contains("82 of your 100"), "{q:?}");
        let q = quote(&t, &b, 98, Some(500), "describe.image", 1.0, false).unwrap();
        assert!(!q.allowed);
        let q = quote(&t, &Budget { stop_at_limit: false, ..b.clone() }, 98, Some(500), "describe.image", 1.0, false).unwrap();
        assert!(q.allowed && !q.warnings.is_empty());

        // Own key: no credits, never blocked, the provider's cost said.
        let q = quote(&t, &b, 1_000, Some(0), "voice", 2.0, true).unwrap();
        assert!(q.allowed && q.credits == 0 && q.byok);
        assert!((q.provider_cost - 0.30).abs() < 1e-9 && q.say.contains("USD 0.30"), "{}", q.say);
        assert!(quote(&t, &b, 0, None, "teleport", 1.0, false).is_err());
    }

    #[test]
    fn the_ledger_sums_this_month_only() {
        let dir = std::env::temp_dir().join(format!("studio-ledger-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let l = Ledger::new(&dir);
        let jan = 1_767_225_600; // 2026-01-01
        assert_eq!(month_of(jan), (2026, 1));
        assert_eq!(month_of(jan - 1), (2025, 12));
        assert_eq!(month_of(951_782_400), (2000, 2)); // 2000-02-29
        for (at, c) in [(jan - 10, 7), (jan + 10, 3), (jan + 86_400 * 20, 5)] {
            l.record(&Usage { at, task: "transcribe".into(), units: 1.0, credits: c, byok: false, provider: "openai".into(), hold: String::new(), note: String::new() }).unwrap();
        }
        assert_eq!(l.used_this_month(jan + 100), 8);
        assert_eq!(l.entries().len(), 3);
        l.set_budget(&Budget { monthly_limit: Some(9), ..Budget::default() }).unwrap();
        assert_eq!(l.budget().monthly_limit, Some(9));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
