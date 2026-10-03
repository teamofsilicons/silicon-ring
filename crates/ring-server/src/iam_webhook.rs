//! Signed IAM deliveries are durably retained before acknowledgment. Request-time introspection
//! remains the authority, so delivery delay cannot extend a user's permission.
use crate::App;
use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};

pub async fn handle(State(app): State<App>, headers: HeaderMap, body: Bytes) -> Response {
    let verified = match ring_providers::iam::verify_webhook(&headers, &body) {
        Ok(v) => v,
        Err(_) => {
            return (StatusCode::UNAUTHORIZED, "IAM webhook verification failed").into_response()
        }
    };
    let Some(event_id) = verified["event"]["event_id"].as_str() else {
        return (StatusCode::BAD_REQUEST, "Missing event ID").into_response();
    };
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
        let realm = if verified["testing"] == true {
            "test"
        } else {
            "production"
        };
        let identities = crate::auth::monitored_identities(&app)
            .into_iter()
            .filter(|i| i.realm == realm)
            .collect::<Vec<_>>();
        for identity in identities {
            if crate::auth::verify(&app, &identity).await.is_err()
                && crate::auth::revoke_identity(&app, &identity, "iam_webhook_authority_changed")
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
