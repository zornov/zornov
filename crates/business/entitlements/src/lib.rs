// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What this copy of Studio is entitled to: a plan, its feature flags and
//! the licence file that proves it.
//!
//! * The editor itself is never gated. Feature flags name only the hosted
//!   and paid extras (`ai.hosted`, `cloud.render`, `cloud.sync`, ...);
//!   with no licence at all Studio runs on the free plan.
//! * Plans are data ([`Catalogue`], `plans.json`), not code: a server can
//!   publish new plans, and a licence can add or remove features on top of
//!   its plan, so neither needs an app update.
//! * A licence is a JSON payload signed with Ed25519 by an issuer whose
//!   public key this app trusts. It verifies offline: nothing phones home
//!   to use the app. An expired licence keeps working for its grace days,
//!   then falls back to the free plan (the editor keeps working).

use ring::signature::{self, Ed25519KeyPair, KeyPair, UnparsedPublicKey};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// The plans shipped with the app (features only; prices come from the
/// server the person signs in to).
pub const DEFAULT_PLANS: &str = include_str!("../plans.json");

/// The plan used with no licence.
pub const FREE: &str = "free";

/// What a licence signature covers: this prefix, then the payload bytes.
const DOMAIN: &[u8] = b"studio-licence/1\n";

// ------------------------------------------------------------------ plans

/// A price, as a server publishes it (minor units: cents).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Price {
    pub amount_minor: u64,
    pub currency: String,
    /// `month`, `year` or `once`.
    pub interval: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub summary: String,
    pub features: Vec<String>,
    /// Hosted AI credits included each month.
    #[serde(default)]
    pub monthly_credits: u64,
    #[serde(default = "one")]
    pub seats: u32,
    #[serde(default)]
    pub storage_gb: u32,
    /// Absent in the shipped catalogue; a server fills it in.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prices: Vec<Price>,
}

fn one() -> u32 {
    1
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Catalogue {
    pub schema: u32,
    pub plans: Vec<Plan>,
}

impl Catalogue {
    pub fn parse(json: &str) -> Result<Catalogue, String> {
        let c: Catalogue = serde_json::from_str(json).map_err(|e| format!("the plan list is not valid: {e}"))?;
        if c.schema != 1 {
            return Err(format!("the plan list is schema {}, this Studio reads 1", c.schema));
        }
        if !c.plans.iter().any(|p| p.id == FREE) {
            return Err("the plan list has no free plan".into());
        }
        Ok(c)
    }

    /// The shipped plans.
    pub fn builtin() -> Catalogue {
        Catalogue::parse(DEFAULT_PLANS).expect("plans.json is valid")
    }

    pub fn plan(&self, id: &str) -> Option<&Plan> {
        self.plans.iter().find(|p| p.id == id)
    }

    pub fn free(&self) -> &Plan {
        self.plan(FREE).expect("checked in parse")
    }
}

// --------------------------------------------------------------- licences

/// What a licence says.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Licence {
    pub schema: u32,
    pub id: String,
    pub plan: String,
    /// Who it is for (a name or organisation) and their sign-in.
    pub holder: String,
    #[serde(default)]
    pub email: String,
    #[serde(default = "one")]
    pub seats: u32,
    /// Unix seconds.
    pub issued: u64,
    /// Unix seconds; `None` never expires (a perpetual licence).
    #[serde(default)]
    pub expires: Option<u64>,
    /// Days it keeps working after `expires` (time to renew offline).
    #[serde(default)]
    pub grace_days: u32,
    /// Features on top of the plan's, and features taken away.
    #[serde(default)]
    pub add: Vec<String>,
    #[serde(default)]
    pub remove: Vec<String>,
    /// Overrides the plan's monthly credits.
    #[serde(default)]
    pub monthly_credits: Option<u64>,
    /// The issuer's key that signed it ([`key_id`]).
    pub key_id: String,
    /// The server that issued it, for renewals (may be empty).
    #[serde(default)]
    pub server: String,
}

/// A licence file: the payload exactly as signed, and the signature.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedLicence {
    /// The [`Licence`] as JSON; signed byte for byte, so it is never
    /// re-serialised before checking.
    pub payload: String,
    /// Ed25519 over `studio-licence/1\n` + payload, hex.
    pub signature: String,
}

impl SignedLicence {
    pub fn parse(text: &str) -> Result<SignedLicence, String> {
        serde_json::from_str(text).map_err(|e| format!("this is not a Studio licence file: {e}"))
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("serialisable")
    }

    /// The payload, unchecked (for showing what a bad file claims).
    pub fn claims(&self) -> Result<Licence, String> {
        serde_json::from_str(&self.payload).map_err(|e| format!("the licence payload is not valid: {e}"))
    }
}

/// A public key this app accepts licences from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustedKey {
    /// 64 hex characters.
    pub public_key: String,
    /// Who it belongs to, for people ("Ashrox LLC", "our own server").
    #[serde(default)]
    pub name: String,
}

/// A short id for a public key: the first 16 hex digits of its SHA-256.
pub fn key_id(public_key: &[u8]) -> String {
    hex(&ring::digest::digest(&ring::digest::SHA256, public_key).as_ref()[..8])
}

/// An issuer: holds the private key and signs licences (a server, or the
/// `studio-cloud-server issue` command).
pub struct Issuer {
    pair: Ed25519KeyPair,
}

impl Issuer {
    /// A new key, as PKCS#8 bytes to keep secret.
    pub fn generate() -> Result<Vec<u8>, String> {
        Ed25519KeyPair::generate_pkcs8(&ring::rand::SystemRandom::new()).map(|d| d.as_ref().to_vec()).map_err(|_| "could not make a signing key".to_owned())
    }

    pub fn from_pkcs8(pkcs8: &[u8]) -> Result<Issuer, String> {
        Ed25519KeyPair::from_pkcs8(pkcs8).map(|pair| Issuer { pair }).map_err(|_| "the signing key is not a valid Ed25519 key".to_owned())
    }

    pub fn public_key_hex(&self) -> String {
        hex(self.pair.public_key().as_ref())
    }

    pub fn key_id(&self) -> String {
        key_id(self.pair.public_key().as_ref())
    }

    /// Signs `licence` (its `key_id` is set to this key's).
    pub fn sign(&self, licence: &Licence) -> SignedLicence {
        let mut l = licence.clone();
        l.key_id = self.key_id();
        let payload = serde_json::to_string(&l).expect("serialisable");
        let mut msg = DOMAIN.to_vec();
        msg.extend_from_slice(payload.as_bytes());
        SignedLicence { payload, signature: hex(self.pair.sign(&msg).as_ref()) }
    }
}

/// Why a licence was not accepted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Rejected {
    /// No trusted key has this id.
    UnknownIssuer(String),
    /// The signature does not match (the file was changed or forged).
    BadSignature,
    Malformed(String),
}

impl std::fmt::Display for Rejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Rejected::UnknownIssuer(id) => write!(f, "the licence was issued by a key this Studio does not trust ({id})"),
            Rejected::BadSignature => f.write_str("the licence's signature does not match: the file was changed or not issued for Studio"),
            Rejected::Malformed(e) => f.write_str(e),
        }
    }
}

/// Checks the signature against `trusted` (offline). Expiry is not checked
/// here; see [`resolve`].
pub fn verify(signed: &SignedLicence, trusted: &[TrustedKey]) -> Result<Licence, Rejected> {
    let l = signed.claims().map_err(Rejected::Malformed)?;
    if l.schema != 1 {
        return Err(Rejected::Malformed(format!("the licence is schema {}, this Studio reads 1", l.schema)));
    }
    let key = trusted
        .iter()
        .filter_map(|k| unhex(&k.public_key).filter(|b| b.len() == 32))
        .find(|b| key_id(b) == l.key_id)
        .ok_or_else(|| Rejected::UnknownIssuer(l.key_id.clone()))?;
    let sig = unhex(&signed.signature).ok_or(Rejected::BadSignature)?;
    let mut msg = DOMAIN.to_vec();
    msg.extend_from_slice(signed.payload.as_bytes());
    UnparsedPublicKey::new(&signature::ED25519, key).verify(&msg, &sig).map_err(|_| Rejected::BadSignature)?;
    Ok(l)
}

// ------------------------------------------------------------ entitlements

/// Where the licence stands.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Standing {
    /// No licence: the free plan.
    None,
    Active { expires: Option<u64> },
    /// Expired, still honoured for `days_left`.
    Grace { days_left: u32 },
    /// Expired past its grace: the free plan until renewed.
    Expired { at: u64 },
    /// The file did not verify: the free plan.
    Invalid { reason: String },
    /// The licence names a plan this app does not know.
    UnknownPlan { plan: String },
}

/// What this copy may use, worked out from the plans and the licence.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Entitlements {
    pub plan: Plan,
    pub features: BTreeSet<String>,
    pub monthly_credits: u64,
    pub seats: u32,
    pub standing: Standing,
    pub licence: Option<Licence>,
}

impl Entitlements {
    /// The free plan, no licence.
    pub fn free(catalogue: &Catalogue) -> Entitlements {
        let plan = catalogue.free().clone();
        Entitlements { features: plan.features.iter().cloned().collect(), monthly_credits: plan.monthly_credits, seats: plan.seats, plan, standing: Standing::None, licence: None }
    }

    pub fn allows(&self, feature: &str) -> bool {
        self.features.contains(feature)
    }

    /// `Ok` when `feature` is on, else a sentence saying which plans have it.
    pub fn require(&self, feature: &str, catalogue: &Catalogue) -> Result<(), String> {
        if self.allows(feature) {
            return Ok(());
        }
        let with: Vec<&str> = catalogue.plans.iter().filter(|p| p.features.iter().any(|f| f == feature)).map(|p| p.name.as_str()).collect();
        Err(if with.is_empty() { format!("{} is not available on any plan", describe(feature)) } else { format!("{} needs the {} plan (you are on {})", describe(feature), with.join(" or "), self.plan.name) })
    }
}

/// A feature flag in words.
pub fn describe(feature: &str) -> &str {
    match feature {
        "ai.byok" => "AI with your own keys",
        "ai.hosted" => "Hosted AI (credits)",
        "ai.priority" => "Priority hosted AI",
        "market.buy" => "Buying from the marketplace",
        "market.sell" => "Selling on the marketplace",
        "cloud.review" => "Review links",
        "cloud.render" => "Cloud rendering",
        "cloud.storage" => "Cloud storage",
        "cloud.sync" => "Cloud project sync",
        "team.admin" => "Team seats and roles",
        "team.sso" => "Single sign-on",
        "support.priority" => "Priority support",
        other => other,
    }
}

/// The entitlements for `licence` (already parsed, maybe not verified) at
/// `now` (unix seconds).
pub fn resolve(catalogue: &Catalogue, licence: Option<&SignedLicence>, trusted: &[TrustedKey], now: u64) -> Entitlements {
    let Some(signed) = licence else { return Entitlements::free(catalogue) };
    let with = |standing: Standing| Entitlements { standing, ..Entitlements::free(catalogue) };
    let l = match verify(signed, trusted) {
        Ok(l) => l,
        Err(e) => return with(Standing::Invalid { reason: e.to_string() }),
    };
    let standing = match l.expires {
        Some(at) if now >= at => {
            let end = at + l.grace_days as u64 * 86_400;
            if now >= end {
                return with(Standing::Expired { at });
            }
            Standing::Grace { days_left: ((end - now).div_ceil(86_400)) as u32 }
        }
        e => Standing::Active { expires: e },
    };
    let Some(plan) = catalogue.plan(&l.plan).cloned() else { return with(Standing::UnknownPlan { plan: l.plan.clone() }) };
    let mut features: BTreeSet<String> = plan.features.iter().chain(&l.add).cloned().collect();
    for r in &l.remove {
        features.remove(r);
    }
    Entitlements { monthly_credits: l.monthly_credits.unwrap_or(plan.monthly_credits), seats: l.seats.max(1), features, plan, standing, licence: Some(l) }
}

// ----------------------------------------------------------------- on disk

/// Keys trusted without asking: compiled in (`STUDIO_LICENCE_PUBKEYS` at
/// build time, comma-separated hex) and at run time (the same variable).
pub fn builtin_keys() -> Vec<TrustedKey> {
    let mut v = Vec::new();
    for src in [option_env!("STUDIO_LICENCE_PUBKEYS").map(str::to_owned), std::env::var("STUDIO_LICENCE_PUBKEYS").ok()].into_iter().flatten() {
        for k in src.split(',').map(str::trim).filter(|k| k.len() == 64 && unhex(k).is_some()) {
            if !v.iter().any(|t: &TrustedKey| t.public_key == k) {
                v.push(TrustedKey { public_key: k.to_ascii_lowercase(), name: "Studio".into() });
            }
        }
    }
    v
}

/// The licence file, trusted keys and plan list in `dir`.
pub struct Store {
    pub dir: PathBuf,
}

impl Store {
    pub fn new(dir: impl Into<PathBuf>) -> Store {
        Store { dir: dir.into() }
    }

    fn licence_path(&self) -> PathBuf {
        self.dir.join("licence.json")
    }

    pub fn licence(&self) -> Option<SignedLicence> {
        std::fs::read_to_string(self.licence_path()).ok().and_then(|t| SignedLicence::parse(&t).ok())
    }

    /// Installs a licence file after checking it verifies (an invalid file
    /// is refused, not saved).
    pub fn install(&self, signed: &SignedLicence) -> Result<Licence, String> {
        let l = verify(signed, &self.trusted()).map_err(|e| e.to_string())?;
        write_atomic(&self.licence_path(), signed.to_json().as_bytes())?;
        Ok(l)
    }

    pub fn remove_licence(&self) {
        let _ = std::fs::remove_file(self.licence_path());
    }

    /// Built-in keys plus the ones the person added (`trusted-keys.json`).
    pub fn trusted(&self) -> Vec<TrustedKey> {
        let mut v = builtin_keys();
        let added: Vec<TrustedKey> = std::fs::read_to_string(self.dir.join("trusted-keys.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        for k in added {
            if !v.iter().any(|t| t.public_key.eq_ignore_ascii_case(&k.public_key)) {
                v.push(k);
            }
        }
        v
    }

    /// Trusts another issuer (a self-hosted server the person chose).
    pub fn trust(&self, key: TrustedKey) -> Result<(), String> {
        if unhex(&key.public_key).is_none_or(|b| b.len() != 32) {
            return Err("a licence key is 64 hex characters".into());
        }
        let path = self.dir.join("trusted-keys.json");
        let mut added: Vec<TrustedKey> = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        if !added.iter().any(|t| t.public_key.eq_ignore_ascii_case(&key.public_key)) {
            added.push(TrustedKey { public_key: key.public_key.to_ascii_lowercase(), name: key.name });
        }
        write_atomic(&path, serde_json::to_string_pretty(&added).expect("serialisable").as_bytes())
    }

    /// The plan list a server last sent (`plans.json`), else the shipped one.
    pub fn catalogue(&self) -> Catalogue {
        std::fs::read_to_string(self.dir.join("plans.json")).ok().and_then(|t| Catalogue::parse(&t).ok()).unwrap_or_else(Catalogue::builtin)
    }

    pub fn save_catalogue(&self, c: &Catalogue) -> Result<(), String> {
        write_atomic(&self.dir.join("plans.json"), serde_json::to_string_pretty(c).expect("serialisable").as_bytes())
    }

    /// What this copy is entitled to now.
    pub fn entitlements(&self) -> Entitlements {
        resolve(&self.catalogue(), self.licence().as_ref(), &self.trusted(), now())
    }
}

/// Writes through a temporary file and a rename, so a crash never leaves
/// half a file.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p).map_err(|e| format!("cannot make {}: {e}", p.display()))?;
    }
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&tmp, bytes).and_then(|_| std::fs::rename(&tmp, path)).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

pub fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn unhex(s: &str) -> Option<Vec<u8>> {
    let s = s.trim();
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| s.get(i..i + 2).and_then(|b| u8::from_str_radix(b, 16).ok())).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn issuer() -> Issuer {
        Issuer::from_pkcs8(&Issuer::generate().unwrap()).unwrap()
    }

    fn licence(plan: &str) -> Licence {
        Licence { schema: 1, id: "L-1".into(), plan: plan.into(), holder: "Sam".into(), email: "sam@example.com".into(), seats: 1, issued: 1_000, expires: Some(10_000), grace_days: 2, add: vec![], remove: vec![], monthly_credits: None, key_id: String::new(), server: String::new() }
    }

    fn trust(i: &Issuer) -> Vec<TrustedKey> {
        vec![TrustedKey { public_key: i.public_key_hex(), name: "test".into() }]
    }

    #[test]
    fn the_shipped_plans_parse_and_have_free() {
        let c = Catalogue::builtin();
        for id in ["free", "pro", "studio", "education", "enterprise"] {
            assert!(c.plan(id).is_some(), "{id}");
        }
        assert!(c.free().features.contains(&"ai.byok".to_owned()));
        assert!(Catalogue::parse(r#"{"schema":1,"plans":[]}"#).is_err());
    }

    #[test]
    fn a_signed_licence_verifies_offline_and_a_changed_one_does_not() {
        let i = issuer();
        let s = i.sign(&licence("pro"));
        let l = verify(&s, &trust(&i)).unwrap();
        assert_eq!(l.plan, "pro");
        assert_eq!(l.key_id, i.key_id());
        // Upgrading oneself by editing the file breaks the signature.
        let forged = SignedLicence { payload: s.payload.replace("\"pro\"", "\"enterprise\""), ..s.clone() };
        assert_eq!(verify(&forged, &trust(&i)), Err(Rejected::BadSignature));
        // Another issuer's key is not trusted.
        let other = issuer();
        assert!(matches!(verify(&s, &trust(&other)), Err(Rejected::UnknownIssuer(_))));
        // A licence signed by an untrusted key that claims a trusted key id.
        let mut fake = other.sign(&licence("pro"));
        fake.payload = fake.payload.replace(&other.key_id(), &i.key_id());
        assert_eq!(verify(&fake, &trust(&i)), Err(Rejected::BadSignature));
    }

    #[test]
    fn entitlements_follow_the_plan_the_licence_and_its_expiry() {
        let c = Catalogue::builtin();
        let i = issuer();
        let t = trust(&i);
        let free = resolve(&c, None, &t, 5_000);
        assert_eq!(free.plan.id, "free");
        assert!(!free.allows("cloud.render"));
        assert!(free.require("cloud.render", &c).unwrap_err().contains("Pro"));

        let mut l = licence("pro");
        l.add = vec!["cloud.sync".into()];
        l.remove = vec!["market.sell".into()];
        l.monthly_credits = Some(42);
        let s = i.sign(&l);
        let e = resolve(&c, Some(&s), &t, 5_000);
        assert_eq!(e.standing, Standing::Active { expires: Some(10_000) });
        assert!(e.allows("cloud.render") && e.allows("cloud.sync") && !e.allows("market.sell"));
        assert_eq!(e.monthly_credits, 42);

        let g = resolve(&c, Some(&s), &t, 10_000 + 3_600);
        assert_eq!(g.standing, Standing::Grace { days_left: 2 });
        assert!(g.allows("cloud.render"));
        let x = resolve(&c, Some(&s), &t, 10_000 + 2 * 86_400);
        assert_eq!(x.standing, Standing::Expired { at: 10_000 });
        assert_eq!(x.plan.id, "free");

        let bad = resolve(&c, Some(&s), &[], 5_000);
        assert!(matches!(bad.standing, Standing::Invalid { .. }));
        assert_eq!(bad.plan.id, "free");
        let odd = resolve(&c, Some(&i.sign(&licence("platinum"))), &t, 5_000);
        assert_eq!(odd.standing, Standing::UnknownPlan { plan: "platinum".into() });
    }

    #[test]
    fn the_store_installs_only_licences_that_verify() {
        let dir = std::env::temp_dir().join(format!("studio-ent-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let st = Store::new(&dir);
        let i = issuer();
        let mut l = licence("studio");
        l.expires = None;
        let s = i.sign(&l);
        assert!(st.install(&s).is_err(), "untrusted issuer installed");
        assert_eq!(st.entitlements().plan.id, "free");
        st.trust(TrustedKey { public_key: i.public_key_hex(), name: "self-hosted".into() }).unwrap();
        st.trust(TrustedKey { public_key: i.public_key_hex(), name: "again".into() }).unwrap();
        assert_eq!(st.trusted().iter().filter(|k| k.public_key == i.public_key_hex()).count(), 1);
        st.install(&s).unwrap();
        let e = st.entitlements();
        assert_eq!(e.plan.id, "studio");
        assert!(e.allows("cloud.sync"));
        assert_eq!(e.seats, 1);
        st.remove_licence();
        assert_eq!(st.entitlements().plan.id, "free");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
