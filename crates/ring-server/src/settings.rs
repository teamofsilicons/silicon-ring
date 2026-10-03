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
pub async fn s3(
    app: &App,
    realm: &str,
    org: &str,
) -> std::result::Result<Option<ring_providers::storage::S3>, ring_providers::Error> {
    let config = provider(app, realm, org, "providers.storage")
        .map_err(|e| ring_providers::Error::new(&e.code, &e.message, false))?;
    let bucket = config["bucket"]
        .as_str()
        .map(String::from)
        .or_else(|| std::env::var("RING_S3_BUCKET").ok());
    let Some(bucket) = bucket else {
        return Ok(None);
    };
    let region = config["region"]
        .as_str()
        .map(String::from)
        .or_else(|| std::env::var("AWS_REGION").ok())
        .unwrap_or_else(|| "us-west-1".into());
    let prefix = config["prefix"]
        .as_str()
        .map(String::from)
        .unwrap_or_else(|| "ring/".into());
    let store = if let (Some(access), Some(secret)) = (
        config["access_key_id"].as_str(),
        config["secret_access_key"].as_str(),
    ) {
        ring_providers::storage::S3::new_with_credentials(
            bucket,
            region,
            prefix,
            access.into(),
            secret.into(),
            config["session_token"].as_str().map(String::from),
        )
        .await?
    } else {
        ring_providers::storage::S3::new(bucket, Some(region), None, prefix).await?
    };
    Ok(Some(store))
}
