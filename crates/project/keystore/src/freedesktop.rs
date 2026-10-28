// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The freedesktop Secret Service over the D-Bus session bus: what GNOME
//! Keyring, KWallet and KeePassXC offer to Linux apps (the same store
//! libsecret talks to). Each key is an item in the default collection
//! with the attributes `service` and `account`, so `secret-tool lookup
//! service <service> account <account>` finds it too.
//!
//! The session is opened with the "plain" algorithm: the secret travels
//! over the person's own session bus, which only their programs can reach.
//! A locked keyring is unlocked by the keyring's own prompt; the person
//! dismissing it is a refusal, not a reason to write the key elsewhere.

use crate::Fail;
use std::collections::HashMap;
use std::time::Duration;
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};

const BUS: &str = "org.freedesktop.secrets";
const SERVICE_PATH: &str = "/org/freedesktop/secrets";
const SERVICE_IFACE: &str = "org.freedesktop.Secret.Service";
const COLLECTION_IFACE: &str = "org.freedesktop.Secret.Collection";
const ITEM_IFACE: &str = "org.freedesktop.Secret.Item";
const PROMPT_IFACE: &str = "org.freedesktop.Secret.Prompt";

/// How long a keyring's unlock prompt may wait for the person.
const PROMPT_WAIT: Duration = Duration::from_secs(120);

/// A secret as the Secret Service carries it: (session, parameters, value, content type).
type Secret = (OwnedObjectPath, Vec<u8>, Vec<u8>, String);

fn unavailable(e: impl std::fmt::Display) -> Fail {
    Fail::Unavailable(format!("the system keyring (Secret Service) is not available: {e}"))
}

fn refused(e: impl std::fmt::Display) -> Fail {
    Fail::Refused(format!("the system keyring did not save the key: {e}"))
}

struct Session {
    conn: Connection,
    path: OwnedObjectPath,
}

impl Session {
    fn open() -> Result<Session, Fail> {
        let conn = Connection::session().map_err(unavailable)?;
        let service = Proxy::new(&conn, BUS, SERVICE_PATH, SERVICE_IFACE).map_err(unavailable)?;
        let (_, path): (OwnedValue, OwnedObjectPath) = service.call("OpenSession", &("plain", Value::from(""))).map_err(unavailable)?;
        Ok(Session { conn, path })
    }

    fn service(&self) -> Result<Proxy<'_>, Fail> {
        Proxy::new(&self.conn, BUS, SERVICE_PATH, SERVICE_IFACE).map_err(unavailable)
    }

    fn proxy(&self, path: &ObjectPath<'_>, iface: &'static str) -> Result<Proxy<'_>, Fail> {
        Proxy::new(&self.conn, BUS, path.to_owned(), iface).map_err(unavailable)
    }

    /// Items with these attributes, unlocked (asking the person to unlock
    /// a locked keyring).
    fn find(&self, attrs: &HashMap<&str, &str>) -> Result<Vec<OwnedObjectPath>, Fail> {
        let (mut unlocked, locked): (Vec<OwnedObjectPath>, Vec<OwnedObjectPath>) = self.service()?.call("SearchItems", &(attrs,)).map_err(unavailable)?;
        if !locked.is_empty() {
            unlocked.extend(self.unlock(locked)?);
        }
        Ok(unlocked)
    }

    /// Unlocks objects; the keyring may ask the person first.
    fn unlock(&self, objects: Vec<OwnedObjectPath>) -> Result<Vec<OwnedObjectPath>, Fail> {
        let (done, prompt): (Vec<OwnedObjectPath>, OwnedObjectPath) = self.service()?.call("Unlock", &(objects.clone(),)).map_err(unavailable)?;
        if prompt.as_str() == "/" {
            return Ok(done);
        }
        self.prompt(prompt)?;
        Ok(objects)
    }

    /// Shows a prompt and waits for the person (or [`PROMPT_WAIT`]).
    fn prompt(&self, path: OwnedObjectPath) -> Result<(), Fail> {
        let conn = self.conn.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let run = || -> Result<bool, Fail> {
                let p = Proxy::new(&conn, BUS, path, PROMPT_IFACE).map_err(unavailable)?;
                // Listening before the prompt shows, so its answer is not missed.
                let mut done = p.receive_signal("Completed").map_err(unavailable)?;
                p.call_method("Prompt", &("",)).map_err(unavailable)?;
                let msg = done.next().ok_or_else(|| unavailable("the prompt ended without an answer"))?;
                let (dismissed, _): (bool, OwnedValue) = msg.body().deserialize().map_err(unavailable)?;
                Ok(dismissed)
            };
            let _ = tx.send(run());
        });
        match rx.recv_timeout(PROMPT_WAIT) {
            Ok(Ok(false)) => Ok(()),
            Ok(Ok(true)) => Err(Fail::Refused("the keyring stayed locked (its prompt was dismissed)".into())),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(Fail::Refused("the keyring's prompt was not answered".into())),
        }
    }

    /// The default collection, unlocked.
    fn collection(&self) -> Result<OwnedObjectPath, Fail> {
        let (path,): (OwnedObjectPath,) = self.service()?.call("ReadAlias", &("default",)).map_err(unavailable)?;
        if path.as_str() == "/" {
            return Err(unavailable("there is no default keyring"));
        }
        self.unlock(vec![path.clone()])?;
        Ok(path)
    }
}

fn attributes<'a>(service: &'a str, account: &'a str) -> HashMap<&'a str, &'a str> {
    HashMap::from([("service", service), ("account", account)])
}

pub fn get(service: &str, account: &str) -> Result<Option<String>, Fail> {
    let s = Session::open()?;
    let Some(item) = s.find(&attributes(service, account))?.into_iter().next() else { return Ok(None) };
    let ((_, _, value, _),): (Secret,) = s.proxy(&item, ITEM_IFACE)?.call("GetSecret", &(&s.path,)).map_err(unavailable)?;
    Ok(String::from_utf8(value).ok())
}

pub fn set(service: &str, account: &str, secret: &str) -> Result<(), Fail> {
    let s = Session::open()?;
    let collection = s.collection()?;
    let label = format!("Zornov: {account}");
    let mut props: HashMap<&str, Value<'_>> = HashMap::new();
    props.insert("org.freedesktop.Secret.Item.Label", Value::from(label.as_str()));
    props.insert("org.freedesktop.Secret.Item.Attributes", Value::from(attributes(service, account)));
    let value: Secret = (s.path.clone(), Vec::new(), secret.as_bytes().to_vec(), "text/plain".to_owned());
    let (_item, prompt): (OwnedObjectPath, OwnedObjectPath) = s.proxy(&collection, COLLECTION_IFACE)?.call("CreateItem", &(props, value, true)).map_err(refused)?;
    if prompt.as_str() != "/" {
        s.prompt(prompt)?;
    }
    match get(service, account) {
        Ok(Some(back)) if back == secret => Ok(()),
        Ok(_) => Err(refused("it was not there when read back")),
        Err(e) => Err(e),
    }
}

pub fn remove(service: &str, account: &str) {
    let Ok(s) = Session::open() else { return };
    let Ok(items) = s.find(&attributes(service, account)) else { return };
    for item in items {
        if let Ok(p) = s.proxy(&item, ITEM_IFACE)
            && let Ok((prompt,)) = p.call::<_, _, (OwnedObjectPath,)>("Delete", &())
            && prompt.as_str() != "/"
        {
            let _ = s.prompt(prompt);
        }
    }
}
