use crate::{auth::storage_error, model::*, App};
use serde_json::{json, Value};
const SECRET_FIELDS: &[&str] = &[
    "api_key",
    "access_key_id",
    "secret_access_key",
    "session_token",
];
pub fn prepare_config(app: &App, i: &Identity, p: &mut Value) -> Result<()> {
    if p["scope"] != "org" {
        return Ok(());
    }
    if !i.admin {
        return Err(forbidden());
    }
    let values = p["values"]
        .as_object_mut()
        .ok_or_else(|| invalid("values must be an object"))?;
    for (k, v) in values
        .iter_mut()
        .filter(|(k, _)| k.starts_with("providers."))
    {
        if !matches!(
            k.as_str(),
            "providers.live" | "providers.tts" | "providers.transcription" | "providers.storage"
        ) {
            return Err(invalid(format!("Unsupported provider setting {k}")));
        }
        let object = v
            .as_object_mut()
            .ok_or_else(|| invalid(format!("{k} must be an object")))?;
        for (field, value) in object {
            let allowed = if k == "providers.storage" {
                [
                    "bucket",
                    "region",
                    "prefix",
                    "access_key_id",
                    "secret_access_key",
                    "session_token",
                ]
                .as_slice()
            } else {
                ["api_key", "model"].as_slice()
            };
            if !allowed.contains(&field.as_str()) {
                return Err(invalid(format!("Unsupported {k}.{field}")));
            }
            let text = value
                .as_str()
                .filter(|s| s.len() <= 8192 && !s.trim().is_empty())
                .ok_or_else(|| {
                    invalid(format!(
                        "{k}.{field} must be a nonempty string up to 8192 bytes"
                    ))
                })?;
            if text.starts_with("sealed:") {
                return Err(invalid(
                    "Supply the new credential value, not a stored ciphertext",
                ));
            }
            if field == "model"
                && text
                    != match k.as_str() {
                        "providers.live" => "gpt-live-1",
                        "providers.tts" => "gpt-live-1",
                        "providers.transcription" => "nova-3",
                        _ => "",
                    }
            {
                return Err(invalid("Use the supported provider model: gpt-live-1 for voice or nova-3 for transcription"));
            }
            if SECRET_FIELDS.contains(&field.as_str()) {
                let label = format!("{}|{k}.{field}", key(&i.realm, &i.org_id, "*"));
                *value = json!(app.vault.seal(&label, value).map_err(storage_error)?);
            }
        }
    }
    Ok(())
}
pub fn provider(app: &App, realm: &str, org: &str, name: &str) -> Result<Value> {
    let k = key(realm, org, "*");
    let mut value = app
        .engine
        .lock()
        .unwrap()
        .state
        .configs
        .get(&k)
        .and_then(|c| c.values.get(name))
        .cloned()
        .unwrap_or(json!({}));
    if let Some(obj) = value.as_object_mut() {
        for (field, v) in obj.iter_mut() {
            if SECRET_FIELDS.contains(&field.as_str()) {
                let label = format!("{k}|{name}.{field}");
                *v = app
                    .vault
                    .unseal(
                        &label,
                        v.as_str()
                            .ok_or_else(|| invalid("Invalid stored provider credential"))?,
                    )
                    .map_err(storage_error)?;
            }
        }
    }
    Ok(value)
}
pub fn openai(
    app: &App,
    realm: &str,
    org: &str,
    kind: &str,
) -> std::result::Result<ring_providers::OpenAi, ring_providers::Error> {
    let config = provider(app, realm, org, kind)
        .map_err(|e| ring_providers::Error::new(&e.code, &e.message, false))?;
    if let Some(key) = config["api_key"].as_str() {
        ring_providers::OpenAi::new(key.into())
    } else {
        ring_providers::OpenAi::from_env()
    }
}
pub fn deepgram(
    app: &App,
    realm: &str,
    org: &str,
) -> std::result::Result<ring_providers::Deepgram, ring_providers::Error> {
    let config = provider(app, realm, org, "providers.transcription")
        .map_err(|e| ring_providers::Error::new(&e.code, &e.message, false))?;
    if let Some(key) = config["api_key"].as_str() {
        ring_providers::Deepgram::new(key.into())
    } else {
        ring_providers::Deepgram::from_env()
    }
}
// Location is immutable; credentials are resolved from the original provider context
// on every operation so key rotation does not strand existing recordings.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StorageLocation {
    pub bucket: String,
    pub region: String,
    pub object_key: String,
    pub credentials: StorageCredentials,
}
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageCredentials {
    Managed,
    Organization,
}
fn storage_location(
    config: &Value,
    bucket: Option<String>,
    region: Option<String>,
    object_key: &str,
    pinned: Option<&StorageLocation>,
) -> std::result::Result<Option<StorageLocation>, ring_providers::Error> {
    if let Some(location) = pinned {
        return Ok(Some(location.clone()));
    }
    let bucket = config["bucket"].as_str().map(String::from).or(bucket);
    let Some(bucket) = bucket else {
        return Ok(None);
    };
    let credentials = match (
        config["access_key_id"].as_str(),
        config["secret_access_key"].as_str(),
    ) {
        (Some(_), Some(_)) => StorageCredentials::Organization,
        (None, None) => StorageCredentials::Managed,
        _ => {
            return Err(ring_providers::Error::new(
                "INVALID_STORAGE",
                "Supply both S3 access key ID and secret access key.",
                false,
            ))
        }
    };
    Ok(Some(StorageLocation {
        bucket,
        region: config["region"]
            .as_str()
            .map(String::from)
            .or(region)
            .unwrap_or_else(|| "us-west-1".into()),
        object_key: format!(
            "{}{}",
            config["prefix"].as_str().unwrap_or("ring/"),
            object_key
        ),
        credentials,
    }))
}
pub async fn s3(
    app: &App,
    realm: &str,
    org: &str,
    object_key: &str,
    pinned: Option<&StorageLocation>,
) -> std::result::Result<
    Option<(ring_providers::storage::S3, StorageLocation)>,
    ring_providers::Error,
> {
    let config = if pinned.is_some_and(|p| p.credentials == StorageCredentials::Managed) {
        json!({})
    } else {
        provider(app, realm, org, "providers.storage")
            .map_err(|e| ring_providers::Error::new(&e.code, &e.message, false))?
    };
    let Some(location) = storage_location(
        &config,
        std::env::var("RING_S3_BUCKET").ok(),
        std::env::var("AWS_REGION").ok(),
        object_key,
        pinned,
    )?
    else {
        return Ok(None);
    };
    let store = match location.credentials {
        StorageCredentials::Organization => {
            let (Some(access), Some(secret)) = (
                config["access_key_id"].as_str(),
                config["secret_access_key"].as_str(),
            ) else {
                return Err(ring_providers::Error::new("STORAGE_CREDENTIALS_MISSING",
                    "Restore this organization's S3 credentials with access to the original recording bucket.", false));
            };
            ring_providers::storage::S3::new_with_credentials(
                location.bucket.clone(),
                location.region.clone(),
                String::new(),
                access.into(),
                secret.into(),
                config["session_token"].as_str().map(String::from),
            )
            .await?
        }
        StorageCredentials::Managed => {
            ring_providers::storage::S3::new(
                location.bucket.clone(),
                Some(location.region.clone()),
                None,
                String::new(),
            )
            .await?
        }
    };
    Ok(Some((store, location)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_locations_pin_full_keys_and_credential_provenance_without_secrets() {
        let original = json!({"bucket":"original", "region":"us-east-1", "prefix":"old/",
            "access_key_id":"access", "secret_access_key":"secret"});
        let location = storage_location(&original, None, None, "production/asset", None)
            .unwrap()
            .unwrap();
        assert_eq!(location.bucket, "original");
        assert_eq!(location.region, "us-east-1");
        assert_eq!(location.object_key, "old/production/asset");
        assert_eq!(location.credentials, StorageCredentials::Organization);
        let persisted = serde_json::to_value(&location).unwrap();
        assert_eq!(persisted.as_object().unwrap().len(), 4);
        assert!(!persisted.to_string().contains("secret"));
        // A storage destination change must not rewrite old GET/DELETE/retry targets.
        let replacement = json!({"bucket":"new", "region":"eu-west-1", "prefix":"new/",
            "access_key_id":"rotated-access", "secret_access_key":"rotated-secret"});
        let pinned = storage_location(&replacement, None, None, "ignored", Some(&location))
            .unwrap()
            .unwrap();
        assert_eq!(pinned, location);
        assert_eq!(pinned.credentials, StorageCredentials::Organization);
        let restored: StorageLocation = serde_json::from_value(persisted).unwrap();
        assert_eq!(restored, location);
        let managed = storage_location(&json!({}), Some("managed".into()), None, "asset", None)
            .unwrap()
            .unwrap();
        assert_eq!(managed.credentials, StorageCredentials::Managed);
        assert_eq!(managed.object_key, "ring/asset");
        assert!(storage_location(
            &json!({"bucket":"b", "access_key_id":"partial"}),
            None,
            None,
            "a",
            None
        )
        .is_err());
    }
}
