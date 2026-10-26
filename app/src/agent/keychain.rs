// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! API keys (Anthropic, OpenAI, Fish Audio, ElevenLabs) in the login
//! Keychain, through the Security framework from Rust (it ran
//! `/usr/bin/security` before). The key never appears on a command line
//! or in a file. Builds are signed with the local signing identity
//! (`cargo xtask bundle`), so a rebuilt Studio is the same app to the
//! Keychain and its keys open without asking again. A key saved by the
//! old way asks once ("Always Allow").

/// Tests use their own (empty) service: they never read the person's keys,
/// which would put a Keychain prompt on their screen.
#[cfg(not(test))]
const SERVICE: &str = "Studio API keys";
#[cfg(test)]
const SERVICE: &str = "Studio API keys (tests)";
/// Where keys were saved before, through `/usr/bin/security`. Those items
/// belong to that tool, so Studio could neither update nor reliably read
/// them ("The Keychain did not keep the key"); keys are saved afresh under
/// [`SERVICE`] and the old entry is only a fallback for reading.
#[cfg(not(test))]
const OLD_SERVICE: &str = "Studio AI";
#[cfg(test)]
const OLD_SERVICE: &str = "Studio AI (tests, old)";
/// The Claude API key's Keychain account.
pub const ANTHROPIC: &str = "anthropic-api-key";
/// The OpenAI API key's Keychain account.
pub const OPENAI: &str = "openai-api-key";
/// Fish Audio's (voices).
pub const FISH: &str = "fish-audio-api-key";
/// ElevenLabs' (voices and sounds).
pub const ELEVENLABS: &str = "elevenlabs-api-key";

/// The saved key, if any.
pub fn api_key(account: &str) -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        let read = |service: &str| {
            let bytes = security_framework::passwords::get_generic_password(service, account).ok()?;
            let key = String::from_utf8(bytes).ok()?.trim().to_owned();
            (!key.is_empty()).then_some(key)
        };
        read(SERVICE).or_else(|| read(OLD_SERVICE))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = account;
        None
    }
}

/// Saves (or replaces) the key.
pub fn set_api_key(account: &str, key: &str) -> Result<(), String> {
    let key = key.trim();
    if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == ':') {
        return Err("That does not look like an API key (letters, digits, -, _ and : only).".into());
    }
    #[cfg(target_os = "macos")]
    {
        // Replaced, not updated: a fresh item belongs to Studio.
        let _ = security_framework::passwords::delete_generic_password(SERVICE, account);
        security_framework::passwords::set_generic_password(SERVICE, account, key.as_bytes()).map_err(|e| format!("The Keychain did not save the key: {e}"))?;
        let back = security_framework::passwords::get_generic_password(SERVICE, account).ok().and_then(|b| String::from_utf8(b).ok());
        if back.as_deref().map(str::trim) == Some(key) { Ok(()) } else { Err("The Keychain did not keep the key.".into()) }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = account;
        Err("Saving keys needs the macOS Keychain.".into())
    }
}

pub fn remove_api_key(account: &str) {
    #[cfg(target_os = "macos")]
    {
        let _ = security_framework::passwords::delete_generic_password(SERVICE, account);
        let _ = security_framework::passwords::delete_generic_password(OLD_SERVICE, account);
    }
    #[cfg(not(target_os = "macos"))]
    let _ = account;
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    /// Saving keeps the key, saving again replaces it, removing removes it
    /// (a dummy key under the tests' own service; never the real keys).
    /// It touches the person's real keychain, which pops up a dialog when
    /// tests run with HOME pointed elsewhere, so it runs only when asked:
    /// `cargo test -p studio-app keys_are_kept -- --ignored`.
    #[test]
    #[ignore = "uses the login keychain; run with --ignored"]
    fn keys_are_kept_replaced_and_removed() {
        let account = format!("studio-test-{}", std::process::id());
        super::remove_api_key(&account);
        super::set_api_key(&account, "test-key-one").unwrap();
        assert_eq!(super::api_key(&account).as_deref(), Some("test-key-one"));
        super::set_api_key(&account, "test-key-two").unwrap();
        assert_eq!(super::api_key(&account).as_deref(), Some("test-key-two"));
        super::remove_api_key(&account);
        assert_eq!(super::api_key(&account), None);
    }
}
