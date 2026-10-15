//! fal.ai credentials: the desktop UI never puts keys in dialog fields, command params,
//! documents or preferences. Windows stores the key in the current user's Credential Manager.
//! Headless and other native platforms can still use FAL_KEY for that process.

#[cfg(windows)]
fn entry() -> Result<keyring::Entry, String> {
    keyring::Entry::new("ai.storyteller.photocraft.generative.fal", "fal.ai").map_err(|_| "Could not access Windows Credential Manager".to_string())
}

/// True when the user has saved a key for this fork. Does not expose its value to the UI.
#[cfg(windows)]
pub fn has_saved_key() -> Result<bool, String> {
    match entry()?.get_password() {
        Ok(key) => Ok(!key.trim().is_empty()),
        Err(keyring::Error::NoEntry) => Ok(false),
        Err(_) => Err("Could not read fal.ai key from Windows Credential Manager".into()),
    }
}

/// Store only after explicit UI confirmation. Never return the key or provider errors that echo it.
#[cfg(windows)]
pub fn save_key(key: &str) -> Result<(), String> {
    if key.is_empty() || key.len() > 2048 || key.chars().any(char::is_control) || key.trim() != key {
        return Err("fal.ai key must be 1–2048 characters without whitespace at either end".into());
    }
    entry()?.set_password(key).map_err(|_| "Could not save fal.ai key in Windows Credential Manager".into())
}

pub(crate) fn read_key() -> Result<String, String> {
    if let Ok(key) = std::env::var("FAL_KEY")
        && !key.trim().is_empty()
    {
        return Ok(key);
    }
    #[cfg(windows)]
    {
        let key = match entry()?.get_password() {
            Ok(key) => key,
            Err(keyring::Error::NoEntry) => return Err("Enter your fal.ai key in the Generative Fill dialog first".into()),
            Err(_) => return Err("Could not read fal.ai key from Windows Credential Manager".into()),
        };
        if key.trim().is_empty() {
            return Err("Saved fal.ai key is empty; replace it in the Generative Fill dialog".into());
        }
        Ok(key)
    }
    #[cfg(not(windows))]
    Err("Set FAL_KEY before using Generative Fill".into())
}

/// Validate before creating a paid job so missing credentials produce an actionable error.
pub fn ensure_key() -> Result<(), String> {
    read_key().map(|_| ())
}

#[cfg(test)]
mod tests {
    #[test]
    fn missing_key_is_a_regular_error_without_leaking_secrets() {
        // Do not edit FAL_KEY (global shared state): this test validates error handling only.
        if let Err(error) = super::ensure_key() {
            assert!(!error.contains("Authorization"));
            assert!(error.contains("fal.ai") || error.contains("FAL_KEY") || error.contains("Credential Manager"));
        }
    }
    #[cfg(windows)]
    #[test]
    fn malformed_keys_fail_before_any_credential_write() {
        for key in ["", " a", "a ", "a\nb", &"x".repeat(2049)] {
            assert!(super::save_key(key).is_err());
        }
    }
}
