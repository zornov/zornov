// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! API keys (Anthropic, OpenAI, Fish Audio, ElevenLabs) in the system's
//! own secret store, through `studio-keystore`: the login Keychain on macOS,
//! Windows Credential Manager, the Secret Service (GNOME Keyring, KWallet)
//! on Linux, and a file only the person can read where none answers. The
//! key never appears on a command line. On macOS, builds are signed with
//! the local signing identity (`cargo xtask bundle`), so a rebuilt Studio
//! is the same app to the Keychain and its keys open without asking again;
//! a key saved by the old way asks once ("Always Allow").

/// Tests use their own (empty) service: they never read the person's keys,
/// which would put a Keychain prompt on their screen.
#[cfg(not(test))]
const SERVICE: &str = "Studio API keys";
#[cfg(test)]
const SERVICE: &str = "Studio API keys (tests)";
/// Where keys were saved before on macOS, through `/usr/bin/security`.
/// Those items belong to that tool, so Studio could neither update nor
/// reliably read them ("The Keychain did not keep the key"); keys are saved
/// afresh under [`SERVICE`] and the old entry is only a fallback for reading.
#[cfg(not(test))]
const OLD_SERVICE: &str = "Studio AI";
#[cfg(test)]
const OLD_SERVICE: &str = "Studio AI (tests, old)";
/// The Claude API key's account.
pub const ANTHROPIC: &str = "anthropic-api-key";
/// The OpenAI API key's account.
pub const OPENAI: &str = "openai-api-key";
/// Fish Audio's (voices).
pub const FISH: &str = "fish-audio-api-key";
/// ElevenLabs' (voices and sounds).
pub const ELEVENLABS: &str = "elevenlabs-api-key";

/// The saved key, if any.
pub fn api_key(account: &str) -> Option<String> {
    studio_keystore::get(SERVICE, account).or_else(|| if cfg!(target_os = "macos") { studio_keystore::get(OLD_SERVICE, account) } else { None })
}

/// Whether a key looks like one (letters, digits, -, _ and :).
fn looks_like_a_key(key: &str) -> bool {
    !key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == ':')
}

/// Saves (or replaces) the key; says where it went.
pub fn save_api_key(account: &str, key: &str) -> Result<studio_keystore::Kept, String> {
    let key = key.trim();
    if !looks_like_a_key(key) {
        return Err("That does not look like an API key (letters, digits, -, _ and : only).".into());
    }
    studio_keystore::set(SERVICE, account, key)
}

/// Saves (or replaces) the key.
pub fn set_api_key(account: &str, key: &str) -> Result<(), String> {
    save_api_key(account, key).map(|_| ())
}

pub fn remove_api_key(account: &str) {
    studio_keystore::remove(SERVICE, account);
    if cfg!(target_os = "macos") {
        studio_keystore::remove(OLD_SERVICE, account);
    }
}

/// Where keys are kept, for a note: "Kept in your login Keychain".
pub fn kept_in() -> String {
    format!("Kept in {}", studio_keystore::system_name())
}

/// The status of a saved key, for its row: where it is.
pub fn saved_label(account: &str) -> String {
    match studio_keystore::kept(SERVICE, account) {
        Some(studio_keystore::Kept::File(_)) => "Saved in a private file".to_owned(),
        _ if cfg!(target_os = "macos") => "Saved in the Keychain".to_owned(),
        _ => format!("Saved in {}", studio_keystore::system_name()),
    }
}

/// What to say after a key was saved.
pub fn saved_note(kept: &studio_keystore::Kept) -> String {
    match kept {
        studio_keystore::Kept::System => format!("Key saved in {}.", studio_keystore::system_name()),
        studio_keystore::Kept::File(p) => studio_keystore::file_notice(p),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn only_what_looks_like_a_key_is_saved() {
        assert!(super::looks_like_a_key("sk-ant-api03_abc:DEF"));
        for bad in ["", "two words", "key\nline", "quote\"d"] {
            assert!(!super::looks_like_a_key(bad), "{bad:?}");
            assert!(super::save_api_key("studio-test-shape", bad).is_err());
        }
    }

    /// Every platform has a store to name; none says it needs macOS.
    #[test]
    fn keys_can_be_kept_on_every_platform() {
        let src = include_str!("keychain.rs");
        let code = src.split("#[cfg(test)]").next().unwrap();
        assert!(!code.contains("needs the macOS Keychain"), "saving is not refused off macOS");
        assert!(super::kept_in().starts_with("Kept in "));
    }

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
