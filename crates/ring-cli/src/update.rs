use crate::store::{invalid, io_error};
use base64::Engine;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use ring_client::{Result, RingError};
use semver::Version;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs;

pub const RELEASE_INDEX_URL: &str = "https://ring.teamofsilicons.com/releases/release-index.json";

/// Public release discovery needs no IAM session. The index is only a catalogue;
/// each selected entry must still verify against the independently pinned key.
pub async fn check(channel: &str) -> Result<Value> {
    let url =
        std::env::var("RING_RELEASE_MANIFEST_URL").unwrap_or_else(|_| RELEASE_INDEX_URL.into());
    let client = reqwest::Client::builder()
        .https_only(true)
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| io_error(e, "update index"))?;
    let bytes = fetch(&client, &url, 1024 * 1024).await?;
    let index = serde_json::from_slice(&bytes).map_err(|e| io_error(e, "update index"))?;
    select_release(&index, channel, &signing_key()?, &current_version())
}

fn current_version() -> Version {
    Version::parse(env!("CARGO_PKG_VERSION")).expect("Cargo package version is valid semver")
}

fn select_release(
    index: &Value,
    channel: &str,
    key: &VerifyingKey,
    current: &Version,
) -> Result<Value> {
    if !matches!(channel, "stable" | "beta") {
        return Err(invalid("Unknown update channel", "update index"));
    }
    let entries = index["releases"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| vec![index.clone()]);
    let selected = entries
        .into_iter()
        .filter(|release| {
            release["protocol_major"] == 1
                && release["platform"] == std::env::consts::OS
                && release["arch"] == std::env::consts::ARCH
                && release["channel"].as_str().unwrap_or("stable") == channel
        })
        .map(|release| {
            let version = Version::parse(release["version"].as_str().unwrap_or(""))
                .map_err(|_| invalid("Release version is not valid semver", "update index"))?;
            Ok((version, release))
        })
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .filter(|(version, _)| channel != "stable" || version.pre.is_empty())
        .max_by(|(a, _), (b, _)| a.cmp_precedence(b));
    let Some((_, mut release)) = selected else {
        return Ok(
            json!({"available":false,"reason":"No published compatible release","channel":channel}),
        );
    };
    let version = verify_candidate(&release, key, current)?;
    release["update_available"] = json!(version.cmp_precedence(current).is_gt());
    release["current_version"] = json!(current.to_string());
    Ok(release)
}

fn signing_key() -> Result<VerifyingKey> {
    let key = std::env::var("SILICON_RING_RELEASE_PUBLIC_KEY").unwrap_or_else(|_| {
        include_str!("../../../deploy/release-public-key.txt")
            .trim()
            .to_owned()
    });
    let key: [u8; 32] = hex::decode(key)
        .map_err(|_| invalid("Release public key must be hex", "update"))?
        .try_into()
        .map_err(|_| invalid("Release key must be exactly 32 bytes", "update"))?;
    VerifyingKey::from_bytes(&key).map_err(|_| invalid("Invalid release key", "update"))
}

/// Signatures bind version, protocol, platform, architecture, digest and HTTPS URL.
fn verify_candidate(release: &Value, key: &VerifyingKey, current: &Version) -> Result<Version> {
    let version = release["version"]
        .as_str()
        .ok_or_else(|| invalid("Release has no version", "update"))?;
    let parsed = Version::parse(version)
        .map_err(|_| invalid("Release version is not valid semver", "update"))?;
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
    if sha.len() != 64 || hex::decode(sha).is_err() {
        return Err(invalid(
            "Release SHA-256 must be 64 hex characters",
            "update",
        ));
    }
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
    key.verify(message.as_bytes(), &signature).map_err(|_| {
        RingError::new(
            "RELEASE_SIGNATURE_INVALID",
            "Release signature verification failed",
            "update",
            "Do not install this release. Check the configured key and publisher.",
        )
    })?;
    if parsed.cmp_precedence(current).is_lt() {
        return Err(RingError::new(
            "RELEASE_DOWNGRADE_FORBIDDEN",
            "The signed release is older than this installation",
            "update",
            "Keep the installed version; ask the publisher for a current release index.",
        ));
    }
    Ok(parsed)
}

async fn fetch(client: &reqwest::Client, url: &str, limit: usize) -> Result<Vec<u8>> {
    let mut response = client
        .get(url)
        .send()
        .await
        .map_err(|e| io_error(e, "update download"))?
        .error_for_status()
        .map_err(|e| io_error(e, "update download"))?;
    if response.content_length().is_some_and(|n| n > limit as u64) {
        return Err(invalid("Release download exceeds its size limit", "update"));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| io_error(e, "update download"))?
    {
        if bytes.len().saturating_add(chunk.len()) > limit {
            return Err(invalid("Release download exceeds its size limit", "update"));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

pub async fn apply(
    release: &Value,
    active_media: bool,
    executable: &std::path::Path,
) -> Result<Value> {
    if release["version"].as_str().is_none() {
        return Ok(
            json!({"updated":false,"reason":"No published compatible release","release":release}),
        );
    }
    let current = current_version();
    let version = verify_candidate(release, &signing_key()?, &current)?;
    if version.cmp_precedence(&current).is_eq() {
        return Ok(
            json!({"updated":false,"version":version.to_string(),"reason":"Already current"}),
        );
    }
    if active_media {
        return Err(RingError::new(
            "UPDATE_DEFERRED",
            "Active native audio prevents a disruptive update",
            "update",
            "Retry after local media ends; hourly daemon checks will retry.",
        ));
    }
    let client = reqwest::Client::builder()
        .https_only(true)
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(|e| io_error(e, "update"))?;
    let bytes = fetch(&client, release["url"].as_str().unwrap(), 200 * 1024 * 1024).await?;
    if hex::encode(Sha256::digest(&bytes)) != release["sha256"] {
        return Err(RingError::new(
            "RELEASE_HASH_INVALID",
            "Downloaded binary did not match the signed digest",
            "update",
            "Retry from the verified publisher.",
        ));
    }
    let candidate = executable.with_extension("ring-update");
    crate::store::atomic_write(&candidate, &bytes)?;
    crate::platform::executable(&candidate)?;
    self_replace::self_replace(&candidate).map_err(|e| io_error(e, "update install"))?;
    let _ = fs::remove_file(candidate);
    Ok(
        json!({"updated":true,"version":version.to_string(),"daemon_restart_required":true,"next_action":"The new CLI is installed; restart the daemon when idle."}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn signed(version: &str, channel: &str, key: &SigningKey) -> Value {
        let sha = "ab".repeat(32);
        let url = "https://releases.example.invalid/ring";
        let message = format!(
            "{version}\n1\n{}\n{}\n{sha}\n{url}",
            std::env::consts::OS,
            std::env::consts::ARCH
        );
        json!({"version":version,"channel":channel,"platform":std::env::consts::OS,"arch":std::env::consts::ARCH,"protocol_major":1,"sha256":sha,"url":url,"signature":base64::engine::general_purpose::STANDARD.encode(key.sign(message.as_bytes()).to_bytes())})
    }

    #[test]
    fn signed_index_selects_compatible_channel_and_rejects_replay_or_tampering() {
        let key = SigningKey::from_bytes(&[7; 32]);
        let current = Version::parse("1.2.3").unwrap();
        let older = signed("1.2.2", "stable", &key);
        assert_eq!(
            verify_candidate(&older, &key.verifying_key(), &current)
                .unwrap_err()
                .code,
            "RELEASE_DOWNGRADE_FORBIDDEN"
        );
        let mut wrong_arch = signed("9.0.0", "stable", &key);
        wrong_arch["arch"] = json!("another-architecture");
        let index = json!({"releases":[signed("1.2.9", "stable", &key), signed("1.2.10", "stable", &key), signed("1.3.0-beta.2", "beta", &key), wrong_arch]});
        assert_eq!(
            select_release(&index, "stable", &key.verifying_key(), &current).unwrap()["version"],
            "1.2.10"
        );
        assert_eq!(
            select_release(&index, "beta", &key.verifying_key(), &current).unwrap()["version"],
            "1.3.0-beta.2"
        );
        let same = signed("1.2.3+build", "stable", &key);
        assert_eq!(
            select_release(&same, "stable", &key.verifying_key(), &current).unwrap()
                ["update_available"],
            false
        );
        let mut forged = signed("1.2.4", "stable", &key);
        forged["sha256"] = json!("cd".repeat(32));
        assert_eq!(
            verify_candidate(&forged, &key.verifying_key(), &current)
                .unwrap_err()
                .code,
            "RELEASE_SIGNATURE_INVALID"
        );
        assert_eq!(
            verify_candidate(
                &signed("1.2.4", "stable", &key),
                &SigningKey::from_bytes(&[8; 32]).verifying_key(),
                &current
            )
            .unwrap_err()
            .code,
            "RELEASE_SIGNATURE_INVALID"
        );
    }

    #[tokio::test]
    async fn public_index_fetch_needs_no_authentication_and_bounds_response_size() {
        let key = SigningKey::from_bytes(&[7; 32]);
        let index = json!({"releases":[signed("1.2.4", "stable", &key)]}).to_string();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!(
            "http://{}/releases/release-index.json",
            listener.local_addr().unwrap()
        );
        let server = tokio::spawn(async move {
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    request.push(stream.read_u8().await.unwrap());
                    assert!(request.len() < 8192);
                }
                let request = String::from_utf8(request).unwrap().to_lowercase();
                assert!(!request.contains("authorization:") && !request.contains("cookie:"));
                stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{index}", index.len()).as_bytes()).await.unwrap();
            }
        });
        // Only the loopback fixture permits HTTP; the production client enforces HTTPS.
        let client = reqwest::Client::new();
        let bytes = fetch(&client, &url, 1024 * 1024).await.unwrap();
        let index = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            select_release(
                &index,
                "stable",
                &key.verifying_key(),
                &Version::parse("1.2.3").unwrap()
            )
            .unwrap()["update_available"],
            true
        );
        assert!(fetch(&client, &url, 10).await.is_err());
        server.await.unwrap();
        assert!(reqwest::Client::builder()
            .https_only(true)
            .build()
            .unwrap()
            .get(url)
            .send()
            .await
            .is_err());
    }
}
