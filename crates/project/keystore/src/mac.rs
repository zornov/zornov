// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The login Keychain, through the Security framework. A key is saved as
//! a fresh generic password (replaced, not updated: a fresh item belongs to
//! this app) and read back to check the Keychain kept it.

use crate::Fail;
use security_framework::passwords::{delete_generic_password, get_generic_password, set_generic_password};

pub fn get(service: &str, account: &str) -> Result<Option<String>, Fail> {
    match get_generic_password(service, account) {
        Ok(bytes) => Ok(String::from_utf8(bytes).ok()),
        Err(_) => Ok(None),
    }
}

pub fn set(service: &str, account: &str, secret: &str) -> Result<(), Fail> {
    let _ = delete_generic_password(service, account);
    set_generic_password(service, account, secret.as_bytes()).map_err(|e| Fail::Refused(format!("The Keychain did not save the key: {e}")))?;
    let back = get_generic_password(service, account).ok().and_then(|b| String::from_utf8(b).ok());
    if back.as_deref().map(str::trim) == Some(secret.trim()) { Ok(()) } else { Err(Fail::Refused("The Keychain did not keep the key.".into())) }
}

pub fn remove(service: &str, account: &str) {
    let _ = delete_generic_password(service, account);
}
