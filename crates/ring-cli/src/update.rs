use crate::store::{invalid, io_error};
use base64::Engine;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use ring_client::{Result, RingError};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs;

/// Release signatures bind version, protocol, platform, architecture, digest and HTTPS URL.
/// Trust is pinned by deployment via SILICON_RING_RELEASE_PUBLIC_KEY (32-byte Ed25519 hex).
pub async fn apply(release: &Value, active_media: bool) -> Result<Value> {
    let Some(version) = release["version"].as_str() else {
        return Ok(
            json!({"updated":false,"reason":"No published compatible release","release":release}),
        );
    };
    if version == env!("CARGO_PKG_VERSION") {
        return Ok(json!({"updated":false,"version":version,"reason":"Already current"}));
    }
    if active_media {
        return Err(RingError::new(
            "UPDATE_DEFERRED",
            "Active native audio prevents a disruptive update",
            "update",
            "Retry after local media ends; hourly daemon checks will retry.",
        ));
    }
    if release["protocol_major"] != 1
        || release["platform"] != std::env::consts::OS
        || release["arch"] != std::env::consts::ARCH
    {
        return Err(invalid(
            "Release protocol/platform/architecture does not match this client",
            "update",
        ));
    }
    let url = release["url"]
        .as_str()
        .filter(|u| u.starts_with("https://"))
        .ok_or_else(|| invalid("Release URL must use HTTPS", "update"))?;
    let sha = release["sha256"]
        .as_str()
        .ok_or_else(|| invalid("Release has no SHA-256 digest", "update"))?;
    let key = std::env::var("SILICON_RING_RELEASE_PUBLIC_KEY").unwrap_or_else(|_| {
        include_str!("../../../deploy/release-public-key.txt")
            .trim()
            .to_owned()
    });
    let key: [u8; 32] = hex::decode(key)
        .map_err(|_| invalid("Release public key must be hex", "update"))?
        .try_into()
        .map_err(|_| invalid("Release key must be exactly 32 bytes", "update"))?;
    let signature = base64::engine::general_purpose::STANDARD
        .decode(release["signature"].as_str().unwrap_or(""))
        .map_err(|_| invalid("Invalid release signature encoding", "update"))?;
    let signature = Signature::from_slice(&signature)
        .map_err(|_| invalid("Invalid release signature", "update"))?;
    let message = format!(
        "{version}\n1\n{}\n{}\n{sha}\n{url}",
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    VerifyingKey::from_bytes(&key)
        .map_err(|_| invalid("Invalid release key", "update"))?
        .verify(message.as_bytes(), &signature)
        .map_err(|_| {
            RingError::new(
                "RELEASE_SIGNATURE_INVALID",
                "Release signature verification failed",
                "update",
                "Do not install this release. Check the configured key and publisher.",
            )
        })?;
    let response = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(|e| io_error(e, "update"))?
        .get(url)
        .send()
        .await
        .map_err(|e| io_error(e, "update download"))?
        .error_for_status()
        .map_err(|e| io_error(e, "update download"))?;
    if response
        .content_length()
        .is_some_and(|n| n > 200 * 1024 * 1024)
    {
        return Err(invalid("Release exceeds 200 MiB", "update"));
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|e| io_error(e, "update download"))?;
    if bytes.len() > 200 * 1024 * 1024 || hex::encode(Sha256::digest(&bytes)) != sha {
        return Err(RingError::new(
            "RELEASE_HASH_INVALID",
            "Downloaded binary did not match the signed digest",
            "update",
            "Retry from the verified publisher.",
        ));
    }
    let exe = std::env::current_exe().map_err(|e| io_error(e, "update"))?;
    let candidate = exe.with_extension("ring-update");
    crate::store::atomic_write(&candidate, &bytes)?;
    crate::platform::executable(&candidate)?;
    self_replace::self_replace(&candidate).map_err(|e| io_error(e, "update install"))?;
    let _ = fs::remove_file(candidate);
    Ok(
        json!({"updated":true,"version":version,"daemon_restart_required":true,"next_action":"The new CLI is installed; restart the daemon when idle."}),
    )
}
