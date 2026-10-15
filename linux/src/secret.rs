//! Storage for the Ring session token (`rst_…`).
//!
//! The token goes to the Secret Service (GNOME Keyring, KWallet) through oo7, or to the secret
//! portal's encrypted file when sandboxed. When neither is reachable (minimal window managers,
//! headless sessions) it falls back to a 0600 file in the config directory. Every function here
//! must run on the Tokio runtime.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use crate::{APP_ID, config};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Long enough for the user to type their keyring password when the collection is locked.
const UNLOCK_TIMEOUT: Duration = Duration::from_secs(120);

fn attributes(api_url: &str) -> HashMap<&'static str, String> {
    HashMap::from([
        ("application", APP_ID.to_string()),
        ("api_url", api_url.to_string()),
    ])
}

fn fallback_path() -> PathBuf {
    config::config_dir().join("session.json")
}

async fn keyring() -> Option<oo7::Keyring> {
    match tokio::time::timeout(CONNECT_TIMEOUT, oo7::Keyring::new()).await {
        Ok(Ok(keyring)) => Some(keyring),
        Ok(Err(err)) => {
            tracing::info!("secret service unavailable ({err}); using the private file store");
            None
        }
        Err(_) => {
            tracing::info!("secret service did not answer; using the private file store");
            None
        }
    }
}

fn read_fallback() -> HashMap<String, String> {
    fs::read(fallback_path())
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn write_fallback(map: &HashMap<String, String>) -> std::io::Result<()> {
    let path = fallback_path();
    if map.is_empty() {
        return match fs::remove_file(&path) {
            Err(err) if err.kind() != std::io::ErrorKind::NotFound => Err(err),
            _ => Ok(()),
        };
    }
    config::write_private(
        &path,
        &serde_json::to_vec(map).expect("token map serializes"),
    )
}

/// The stored session token for `api_url`, if any.
pub async fn load(api_url: &str) -> Option<String> {
    if let Some(keyring) = keyring().await {
        let attrs = attributes(api_url);
        match keyring.search_items(&attrs).await {
            Ok(items) => {
                if let Some(item) = items.into_iter().next() {
                    if item.is_locked().await.unwrap_or(false) {
                        let _ = tokio::time::timeout(UNLOCK_TIMEOUT, item.unlock()).await;
                    }
                    match item.secret().await {
                        Ok(secret) => {
                            if let Ok(token) = std::str::from_utf8(secret.as_ref()) {
                                return Some(token.to_string());
                            }
                        }
                        Err(err) => {
                            tracing::warn!("could not read the session from the keyring: {err}")
                        }
                    }
                }
            }
            Err(err) => tracing::warn!("keyring search failed: {err}"),
        }
    }
    read_fallback().remove(api_url)
}

/// Stores `token` for `api_url`, preferring the keyring.
pub async fn store(api_url: &str, token: &str) -> Result<(), String> {
    if let Some(keyring) = keyring().await {
        let _ = tokio::time::timeout(UNLOCK_TIMEOUT, keyring.unlock()).await;
        let attrs = attributes(api_url);
        match keyring
            .create_item("Silicon Ring session", &attrs, token, true)
            .await
        {
            Ok(()) => {
                // Do not leave an older plaintext copy behind once the keyring has it.
                let mut map = read_fallback();
                if map.remove(api_url).is_some() {
                    let _ = write_fallback(&map);
                }
                return Ok(());
            }
            Err(err) => tracing::warn!(
                "could not save the session to the keyring ({err}); using the private file store"
            ),
        }
    }
    let mut map = read_fallback();
    map.insert(api_url.to_string(), token.to_string());
    write_fallback(&map).map_err(|err| format!("Could not save the session: {err}"))
}

/// Removes the token for `api_url` from every store.
pub async fn clear(api_url: &str) {
    if let Some(keyring) = keyring().await {
        if let Err(err) = keyring.delete(&attributes(api_url)).await {
            tracing::warn!("could not remove the session from the keyring: {err}");
        }
    }
    let mut map = read_fallback();
    if map.remove(api_url).is_some() {
        if let Err(err) = write_fallback(&map) {
            tracing::warn!("could not remove the stored session: {err}");
        }
    }
}
