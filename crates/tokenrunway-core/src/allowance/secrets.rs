//! Secrets (pasted API/admin keys and session cookies, from the login window or pasted) live in the OS credential store:
//! macOS Keychain, Windows Credential Manager, Secret Service on Linux. Never in a file, never in
//! a log. As in Rattleboard, a missing Secret Service is an error that says what to install,
//! never a plain-text fallback.

const SERVICE: &str = "io.github.slastrina.tokenrunway";

/// The keychain account name for an account's key.
pub fn key_name(account_id: &str) -> String {
    format!("{account_id}-key")
}

/// The keychain account name for a session from the in-app login window (a Cookie header
/// value such as "WorkosCursorSessionToken=…", nothing else from the session).
pub fn session_name(account_id: &str) -> String {
    format!("{account_id}-session")
}

/// The secret, or None if there's none (or the store can't be read).
pub fn get(name: &str) -> Option<String> {
    keyring::Entry::new(SERVICE, name).and_then(|e| e.get_password()).ok().filter(|s| !s.is_empty())
}

/// Saves `value`; an empty value deletes the entry.
pub fn set(name: &str, value: &str) -> Result<(), String> {
    let entry = keyring::Entry::new(SERVICE, name).map_err(store_error)?;
    if value.is_empty() {
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(store_error(e)),
        }
    } else {
        entry.set_password(value).map_err(store_error)
    }
}

pub fn delete(name: &str) {
    let _ = set(name, "");
}

fn store_error(e: keyring::Error) -> String {
    #[cfg(target_os = "linux")]
    if matches!(e, keyring::Error::PlatformFailure(_) | keyring::Error::NoStorageAccess(_)) {
        return format!(
            "No Secret Service keyring is available ({e}). Install and unlock GNOME Keyring or KWallet; \
             Tokenrunway never saves keys in plain text."
        );
    }
    format!("Couldn't use the keychain: {e}")
}
