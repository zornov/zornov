// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What the server remembers, kept as one JSON file (`state.json`, written
//! atomically after each change) plus uploaded files beside it.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use studio_commerce::market::{Listing, Sale};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct User {
    pub email: String,
    pub name: String,
    pub salt: String,
    pub hash: String,
    pub plan: String,
    /// When the paid plan ends (unix seconds).
    #[serde(default)]
    pub plan_expires: Option<u64>,
    pub balance: i64,
    pub created: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Hold {
    pub email: String,
    pub task: String,
    pub credits: u64,
    pub at: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UsageRow {
    pub email: String,
    pub at: u64,
    pub task: String,
    pub units: f64,
    pub credits: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Order {
    pub id: String,
    pub email: String,
    /// `plan`, `credits` or `listing`.
    pub what: String,
    pub item: String,
    pub title: String,
    pub amount_minor: u64,
    pub currency: String,
    #[serde(default)]
    pub session: String,
    /// `open` or `paid`.
    pub status: String,
    pub at: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Receipt {
    pub order: String,
    pub email: String,
    pub what: String,
    pub title: String,
    pub amount_minor: u64,
    pub currency: String,
    pub at: u64,
    #[serde(default)]
    pub session: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    pub email: String,
    pub name: String,
    /// `queued`, `running`, `done` or `failed`.
    pub status: String,
    #[serde(default)]
    pub error: String,
    pub at: u64,
    #[serde(default)]
    pub output: Option<String>,
    #[serde(default)]
    pub format: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReviewNote {
    pub name: String,
    pub at: f64,
    pub note: String,
    pub when: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Review {
    pub email: String,
    pub title: String,
    pub file: String,
    pub content_type: String,
    pub created: u64,
    #[serde(default)]
    pub notes: Vec<ReviewNote>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Space {
    pub name: String,
    pub owner: String,
    /// Accounts allowed in (the owner adds people by joining them).
    #[serde(default)]
    pub accounts: BTreeSet<String>,
    #[serde(default)]
    pub head: Option<Value>,
    #[serde(default)]
    pub log: Vec<Value>,
    #[serde(default)]
    pub members: BTreeMap<String, u32>,
    #[serde(default)]
    pub notes: BTreeMap<String, Value>,
    #[serde(default)]
    pub team: Option<Value>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct State {
    #[serde(default)]
    pub users: BTreeMap<String, User>,
    /// SHA-256 of a session token → the account.
    #[serde(default)]
    pub sessions: BTreeMap<String, String>,
    #[serde(default)]
    pub holds: BTreeMap<String, Hold>,
    #[serde(default)]
    pub usage: Vec<UsageRow>,
    #[serde(default)]
    pub orders: BTreeMap<String, Order>,
    #[serde(default)]
    pub receipts: Vec<Receipt>,
    #[serde(default)]
    pub jobs: BTreeMap<String, Job>,
    #[serde(default)]
    pub reviews: BTreeMap<String, Review>,
    #[serde(default)]
    pub spaces: BTreeMap<String, Space>,
    #[serde(default)]
    pub listings: BTreeMap<String, Listing>,
    #[serde(default)]
    pub owned: BTreeMap<String, BTreeSet<String>>,
    #[serde(default)]
    pub sales: Vec<Sale>,
    #[serde(default)]
    pub next: u64,
}

impl State {
    pub fn id(&mut self, prefix: &str) -> String {
        self.next += 1;
        format!("{prefix}-{}", self.next)
    }

    /// Balance minus credits on hold.
    pub fn available(&self, email: &str) -> i64 {
        let held: u64 = self.holds.values().filter(|h| h.email == email).map(|h| h.credits).sum();
        self.users.get(email).map_or(0, |u| u.balance) - held as i64
    }
}
