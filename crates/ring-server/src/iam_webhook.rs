//! Signed IAM deliveries are durably retained before acknowledgment. Each delivery forces fresh
//! introspection, bypassing the short positive cache used by control requests.
use crate::model::{forbidden, Result};
use crate::App;
use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde_json::json;

fn testing_binding(
    verified: &ring_providers::iam::VerifiedIamWebhook,
) -> Result<Option<(String, u64)>> {
    let aggregate = &verified.event().aggregate;
    match (aggregate.get("environment_id"), aggregate.get("generation")) {
        (None, None) => Ok(None),
        (Some(environment), Some(generation)) => {
            let environment = environment.as_str().ok_or_else(forbidden)?;
            let id = uuid::Uuid::parse_str(environment).map_err(|_| forbidden())?;
            let generation = generation
                .as_u64()
                .filter(|value| *value > 0)
                .ok_or_else(forbidden)?;
            if id.is_nil() || id.to_string() != environment {
                return Err(forbidden());
            }
            Ok(Some((environment.into(), generation)))
        }
        _ => Err(forbidden()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    use serde_json::Value;
    use std::sync::{Arc, Mutex};

    #[tokio::test]
    async fn signed_webhooks_bind_environment_generation_and_key_before_scoped_effects() {
        const CHILD: &str = "RING_MANAGED_WEBHOOK_TEST_CHILD";
        const SECRET: &str = "ring-webhook-test-signing-secret-00001";
        if std::env::var_os(CHILD).is_none() {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "iam_webhook::tests::signed_webhooks_bind_environment_generation_and_key_before_scoped_effects", "--nocapture"])
                .env(CHILD, "1").env("RING_ENV", "development").env("RING_DISABLE_PROVIDERS", "1")
                .env_remove("RING_ENCRYPTION_KEY").output().unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        fn signed(body: &Value, event: &str) -> (HeaderMap, Bytes) {
            let body = body.to_string();
            let timestamp = chrono::Utc::now().timestamp().to_string();
            let signature = jsonwebtoken::crypto::sign(
                format!("{timestamp}.{body}").as_bytes(),
                &jsonwebtoken::EncodingKey::from_secret(SECRET.as_bytes()),
                jsonwebtoken::Algorithm::HS256,
            )
            .unwrap();
            let signature = URL_SAFE_NO_PAD
                .decode(signature)
                .unwrap()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            let mut headers = HeaderMap::new();
            for (name, value) in [
                ("x-silicon-iam-event-id", event.into()),
                ("x-silicon-iam-timestamp", timestamp),
                ("x-silicon-iam-key-version", "1".into()),
                ("x-silicon-iam-signature", format!("v1={signature}")),
            ] {
                headers.insert(name, value.parse().unwrap());
            }
            (headers, Bytes::from(body))
        }
        std::env::set_var("RING_IAM_WEBHOOK_SECRET", SECRET);
        std::env::set_var("RING_IAM_WEBHOOK_VERSION", "1");
        std::env::set_var(
            "RING_HONEYCOMB_CONTROL_TOKEN",
            "dedicated-ring-lifecycle-control-00001",
        );
        let dir = tempfile::tempdir().unwrap();
        let manager = Arc::new(crate::lifecycle::Manager::open(dir.path()).unwrap());
        let app = App {
            testing: None,
            environments: Some(manager.clone()),
            activity: Arc::default(),
            engine: Arc::new(Mutex::new(crate::engine::Engine::open(dir.path()).unwrap())),
            media: Arc::default(),
            test_tokens: Arc::default(),
            test_secret: None,
            providers_disabled: true,
            vault: Arc::new(crate::vault::Vault::open(dir.path()).unwrap()),
            telemetry: Arc::new(None),
            web_analytics: Arc::new(None),
            web_events: Arc::new(None),
            cli_telemetry: Arc::new(None),
        };
        let environment = uuid::Uuid::new_v4().to_string();
        let operation = uuid::Uuid::new_v4().to_string();
        let key = "A".repeat(32);
        let command = json!({"operation_id":operation,"environment_id":environment,"org_id":"owner","app_id":"ring","environment_revision":1,"generation":1,"key_version":1,"action":"prepare","testing_key":key});
        let mut headers = HeaderMap::new();
        headers.insert(
            "authorization",
            "Bearer dedicated-ring-lifecycle-control-00001"
                .parse()
                .unwrap(),
        );
        assert_eq!(
            crate::lifecycle::handle(
                State(app.clone()),
                axum::extract::Path(("owner".into(), environment.clone(), operation)),
                headers,
                Bytes::from(command.to_string())
            )
            .await
            .status(),
            StatusCode::OK
        );
        let event = uuid::Uuid::new_v4().to_string();
        let metadata = json!({"environment_id":environment,"generation":1,"spec_version":"1.0","event_id":event,"event_type":"organization.membership.created.v1","occurred_at":"2026-09-04T00:00:00Z","organization_id":null,"aggregate":{"type":"membership","id":"00000000-0000-4000-8000-000000000007","version":1}});
        let body = json!({"test":{"testing_key":key,"metadata":metadata,"data":{}}});
        for mutate in 0..4 {
            let mut invalid = body.clone();
            match mutate {
                0 => invalid["test"]["metadata"]["environment_id"] = json!(uuid::Uuid::new_v4()),
                1 => invalid["test"]["metadata"]["generation"] = json!(2),
                2 => invalid["test"]["testing_key"] = json!("B".repeat(32)),
                _ => {
                    invalid["test"]["metadata"]
                        .as_object_mut()
                        .unwrap()
                        .remove("environment_id");
                    invalid["test"]["metadata"]
                        .as_object_mut()
                        .unwrap()
                        .remove("generation");
                }
            }
            let (headers, bytes) = signed(&invalid, &event);
            assert_eq!(
                handle(State(app.clone()), headers, bytes).await.status(),
                StatusCode::UNAUTHORIZED
            );
        }
        let (headers, bytes) = signed(&body, &event);
        let mut tampered = body.clone();
        tampered["test"]["data"] = json!({"changed":true});
        assert_eq!(
            handle(
                State(app.clone()),
                headers.clone(),
                Bytes::from(tampered.to_string())
            )
            .await
            .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            handle(State(app.clone()), headers.clone(), bytes.clone())
                .await
                .status(),
            StatusCode::NO_CONTENT
        );
        assert_eq!(
            handle(State(app.clone()), headers, bytes).await.status(),
            StatusCode::NO_CONTENT
        );
        assert!(!dir.path().join("iam-webhooks.sqlite3").exists());
        let target = manager.get(&environment).await.unwrap();
        let path = target
            .engine
            .lock()
            .unwrap()
            .data_dir
            .join("iam-webhooks.sqlite3");
        let db = rusqlite::Connection::open(path).unwrap();
        let rows: i64 = db
            .query_row("SELECT COUNT(*) FROM webhooks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 1);
        target.stop().await.unwrap();
        let (headers, bytes) = signed(&body, &event);
        assert_eq!(
            handle(State(app), headers, bytes).await.status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
    }
}

async fn destination(
    app: App,
    verified: &ring_providers::iam::VerifiedIamWebhook,
) -> Result<(App, String)> {
    if !verified.is_testing() {
        if app.testing.is_some() {
            return Err(forbidden());
        }
        return Ok((app, "production".into()));
    }
    if let Some((environment, generation)) = testing_binding(verified)? {
        let target = app
            .environments
            .as_ref()
            .ok_or_else(forbidden)?
            .get(&environment)
            .await?;
        let context = target.testing.as_ref().ok_or_else(forbidden)?;
        if context.environment_id != environment || context.generation != generation {
            return Err(forbidden());
        }
        ring_providers::iam::verify_webhook_environment(verified, &context.testing_key)
            .map_err(crate::provider_error)?;
        Ok((target, environment))
    } else {
        // Old isolated deployments had no UUID metadata. Only their explicitly configured
        // root key can authenticate this legacy route; managed realms require both fences.
        if app.testing.is_some() || app.test_secret.is_none() {
            return Err(forbidden());
        }
        let key = std::env::var("RING_IAM_TEST_ENVIRONMENT_KEY").map_err(|_| forbidden())?;
        ring_providers::iam::verify_webhook_environment(verified, &key)
            .map_err(crate::provider_error)?;
        Ok((app, "test".into()))
    }
}

pub async fn handle(State(app): State<App>, headers: HeaderMap, body: Bytes) -> Response {
    let verified = match ring_providers::iam::verify_webhook(&headers, &body) {
        Ok(v) => v,
        Err(_) => {
            return (StatusCode::UNAUTHORIZED, "IAM webhook verification failed").into_response()
        }
    };
    // Reverse drop order keeps this guard alive until the selected App's storage closes.
    let _activity;
    let (app, realm) = match destination(app, &verified).await {
        Ok(target) => target,
        Err(_) => {
            return (
                StatusCode::UNAUTHORIZED,
                "IAM webhook environment verification failed",
            )
                .into_response()
        }
    };
    _activity = match app.activity.enter() {
        Some(activity) => activity,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "Testing environment is changing",
            )
                .into_response();
        }
    };
    let event_id = verified.event_id().to_string();
    let verified = json!({"testing":verified.is_testing(),"event":verified.event()});
    let dir = app.engine.lock().unwrap().data_dir.clone();
    let saved = (|| -> rusqlite::Result<()> {
        let db = rusqlite::Connection::open(dir.join("iam-webhooks.sqlite3"))?;
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; CREATE TABLE IF NOT EXISTS webhooks(event_id TEXT PRIMARY KEY, payload TEXT NOT NULL, received_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP);")?;
        db.execute(
            "INSERT OR IGNORE INTO webhooks(event_id,payload) VALUES(?1,?2)",
            rusqlite::params![event_id, verified.to_string()],
        )?;
        Ok(())
    })();
    if saved.is_ok() {
        let identities = crate::auth::monitored_identities(&app)
            .into_iter()
            .filter(|i| i.realm == realm)
            .collect::<Vec<_>>();
        for identity in identities {
            if crate::auth::revalidate(&app, &identity, "iam_webhook_authority_changed")
                .await
                .is_err()
            {
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "IAM revocation persistence was not confirmed",
                )
                    .into_response();
            }
        }
    }
    match saved {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "IAM webhook persistence was not confirmed",
        )
            .into_response(),
    }
}
