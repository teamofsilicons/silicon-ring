//! Ting notifications to silicons: a transactional outbox (exact body bytes, stable keys,
//! exponential backoff), the App verification proof used to send, and subscription
//! registration with a User verification proof.

use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::extract::State;
use ring_proto::{Account, AccountKind, Notifications, SetNotifications, TingStatus};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};
use silicon_accounts_client::{IssueAppVerification, IssueUserVerification};
use tokio::sync::{Mutex, Notify};

use crate::app::{App, AppState};
use crate::error::{ApiError, ApiResult};
use crate::extract::{ApiJson, Auth};
use crate::util::now_ms;

pub const PRIORITY_DELEGATION: i64 = 0;
pub const PRIORITY_INCOMING: i64 = 1;
pub const PRIORITY_NORMAL: i64 = 5;
pub const PRIORITY_BATCH: i64 = PRIORITY_NORMAL;

const PROOF_TTL_SECONDS: u32 = 1800;
/// Mint a fresh App verification proof when fewer than 3 minutes remain.
const PROOF_REFRESH_MARGIN_MS: i64 = 3 * 60 * 1000;
const MAX_ATTEMPTS: i64 = 12;
const MAX_BACKOFF_MS: i64 = 10 * 60 * 1000;

pub struct Ting {
    http: reqwest::Client,
    url: String,
    proof: Mutex<Option<(String, i64)>>,
    notify: Notify,
}

impl Ting {
    pub fn new(url: &str) -> Ting {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("ring-server/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("HTTP client");
        Ting {
            http,
            url: url.trim_end_matches('/').to_string(),
            proof: Mutex::new(None),
            notify: Notify::new(),
        }
    }

    /// Wakes the outbox worker after new rows were committed.
    pub fn wake(&self) {
        self.notify.notify_one();
    }

    async fn app_proof(&self, app: &App, force: bool) -> Result<String, String> {
        let mut cached = self.proof.lock().await;
        if !force
            && let Some((token, expires)) = cached.as_ref()
            && *expires - now_ms() > PROOF_REFRESH_MARGIN_MS
        {
            return Ok(token.clone());
        }
        let request = IssueAppVerification {
            receiving_app: "ting".into(),
            scopes: vec![
                "tings.send".into(),
                "subscriptions.revoke".into(),
                "subscriptions.read".into(),
            ],
            access_ttl_seconds: Some(PROOF_TTL_SECONDS),
        };
        let key = uuid::Uuid::new_v4().to_string();
        let issued = app
            .accounts
            .app()
            .issue_app_verification(&request, Some(&key))
            .await
            .map_err(|e| format!("proof: {}", e.code()))?;
        let expires = issued
            .expires_at
            .map(|t| (t.unix_timestamp_nanos() / 1_000_000) as i64)
            .unwrap_or_else(|| now_ms() + PROOF_TTL_SECONDS as i64 * 1000);
        let token = issued.proof_token.expose().to_string();
        *cached = Some((token.clone(), expires));
        Ok(token)
    }
}

/// Queues one ting for a silicon with notifications on. Must run in the same transaction as the
/// change it reports. `data` always gains `to` (the silicon's id). Returns whether it was queued.
pub fn enqueue(
    conn: &Connection,
    recipient: &Account,
    ty: &str,
    key: &str,
    data: Value,
    priority: i64,
) -> rusqlite::Result<bool> {
    if recipient.kind != AccountKind::Silicon {
        return Ok(false);
    }
    let enabled: Option<i64> = conn
        .query_row(
            "SELECT enabled FROM ting_subscriptions WHERE account_uuid = ?1",
            [&recipient.uuid],
            |r| r.get(0),
        )
        .optional()?;
    if enabled != Some(1) {
        return Ok(false);
    }
    let mut data = data;
    if let Value::Object(map) = &mut data {
        map.insert("to".into(), Value::String(recipient.id.clone()));
    }
    // Keys are unique per sending app across all recipients, and at most 200 bytes.
    let key = if key.len() > 200 {
        crate::util::sha256_hex(key.as_bytes())
    } else {
        key.to_string()
    };
    let body = serde_json::to_vec(
        &json!({"type": ty, "for": recipient.uuid, "key": key, "data": data, "metadata": {}}),
    )
    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    let now = now_ms();
    let inserted = conn.execute(
        "INSERT OR IGNORE INTO ting_outbox (key, recipient_uuid, type, body, priority, status, next_attempt_at, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, 'pending', ?6, ?6, ?6)",
        params![key, recipient.uuid, ty, body, priority, now],
    )?;
    Ok(inserted == 1)
}

struct OutboxRow {
    id: i64,
    recipient: String,
    ty: String,
    body: Vec<u8>,
    attempts: i64,
}

enum Outcome {
    Delivered,
    Retry(String),
    Terminal(String),
    ProofRejected(String),
}

pub async fn worker(app: AppState) {
    let _ = app.db.with(|c| {
        c.execute(
            "UPDATE ting_outbox SET status = 'pending' WHERE status = 'sending'",
            [],
        )
    });
    loop {
        let sent = drain(&app).await;
        if !sent {
            tokio::select! {
                _ = app.ting.notify.notified() => {}
                _ = tokio::time::sleep(Duration::from_secs(2)) => {}
            }
        }
    }
}

/// Sends due rows; returns true when it did any work.
async fn drain(app: &AppState) -> bool {
    let rows: Vec<OutboxRow> = match app.db.tx(|tx| -> rusqlite::Result<Vec<OutboxRow>> {
        let mut stmt = tx.prepare(
            "SELECT id, recipient_uuid, type, body, attempts FROM ting_outbox WHERE status = 'pending' AND next_attempt_at <= ?1
             ORDER BY priority, id LIMIT 20",
        )?;
        let rows = stmt
            .query_map([now_ms()], |r| Ok(OutboxRow { id: r.get(0)?, recipient: r.get(1)?, ty: r.get(2)?, body: r.get(3)?, attempts: r.get(4)? }))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for row in &rows {
            tx.execute("UPDATE ting_outbox SET status = 'sending', attempts = attempts + 1, updated_at = ?2 WHERE id = ?1", params![row.id, now_ms()])?;
        }
        Ok(rows)
    }) {
        Ok(rows) => rows,
        Err(error) => {
            tracing::error!(%error, "could not read the Ting outbox");
            return false;
        }
    };
    if rows.is_empty() {
        return false;
    }
    for row in rows {
        let mut outcome = send(app, &row.body, false).await;
        if let Outcome::ProofRejected(_) = outcome {
            outcome = send(app, &row.body, true).await;
        }
        let attempts = row.attempts + 1;
        let now = now_ms();
        let update = match &outcome {
            Outcome::Delivered => {
                tracing::debug!(id = row.id, ty = row.ty.as_str(), "ting delivered");
                app.db.with(|c| c.execute("UPDATE ting_outbox SET status = 'delivered', last_error = NULL, updated_at = ?2 WHERE id = ?1", params![row.id, now]))
            }
            Outcome::Retry(error) if attempts < MAX_ATTEMPTS => {
                let backoff = (1000i64 << attempts.min(20)).min(MAX_BACKOFF_MS);
                tracing::warn!(
                    id = row.id,
                    ty = row.ty.as_str(),
                    attempts,
                    error = error.as_str(),
                    "ting will be retried"
                );
                app.db.with(|c| {
                    c.execute(
                        "UPDATE ting_outbox SET status = 'pending', next_attempt_at = ?2, last_error = ?3, updated_at = ?4 WHERE id = ?1",
                        params![row.id, now + backoff, error, now],
                    )
                })
            }
            Outcome::Retry(error) | Outcome::Terminal(error) | Outcome::ProofRejected(error) => {
                tracing::warn!(
                    id = row.id,
                    ty = row.ty.as_str(),
                    error = error.as_str(),
                    "ting failed"
                );
                if error.contains("recipient_not_registered") {
                    let _ = app.db.with(|c| {
                        c.execute(
                            "UPDATE ting_subscriptions SET last_error = 'recipient_not_registered', updated_at = ?2 WHERE account_uuid = ?1",
                            params![row.recipient, now],
                        )
                    });
                }
                app.db.with(|c| c.execute("UPDATE ting_outbox SET status = 'failed', last_error = ?2, updated_at = ?3 WHERE id = ?1", params![row.id, error, now]))
            }
        };
        if let Err(error) = update {
            tracing::error!(%error, "could not update the Ting outbox");
        }
    }
    true
}

async fn send(app: &AppState, body: &[u8], fresh_proof: bool) -> Outcome {
    let proof = match app.ting.app_proof(app, fresh_proof).await {
        Ok(p) => p,
        Err(error) => return Outcome::Retry(error),
    };
    let response = app
        .ting
        .http
        .post(format!("{}/v1/tings", app.ting.url))
        .bearer_auth(proof)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(body.to_vec())
        .send()
        .await;
    let response = match response {
        Ok(r) => r,
        Err(e) => {
            return Outcome::Retry(if e.is_timeout() {
                "timeout".into()
            } else {
                "network".into()
            });
        }
    };
    let status = response.status().as_u16();
    if (200..300).contains(&status) {
        return Outcome::Delivered;
    }
    let code = response
        .json::<Value>()
        .await
        .ok()
        .and_then(|v| v["error"]["code"].as_str().map(String::from))
        .unwrap_or_default();
    let error = format!("{status} {code}");
    match status {
        429 | 500..=599 => Outcome::Retry(error),
        401 | 403 if code == "permission_denied" && !fresh_proof => Outcome::ProofRejected(error),
        _ => Outcome::Terminal(error),
    }
}

// ---- subscriptions ----------------------------------------------------------------------------

fn set_status(
    app: &App,
    uuid: &str,
    enabled: bool,
    subscription_id: Option<&str>,
    last_error: Option<&str>,
) {
    let result = app.db.with(|c| {
        c.execute(
            "INSERT INTO ting_subscriptions (account_uuid, enabled, subscription_id, last_error, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(account_uuid) DO UPDATE SET enabled = excluded.enabled, subscription_id = excluded.subscription_id,
               last_error = excluded.last_error, updated_at = excluded.updated_at",
            params![uuid, enabled as i64, subscription_id, last_error, now_ms()],
        )
    });
    if let Err(error) = result {
        tracing::error!(%error, "could not store Ting subscription status");
    }
}

/// Registers the silicon's Ting subscription with a User verification proof minted from its
/// fresh access token. Returns the subscription id.
async fn register(app: &App, uuid: &str, access_token: &str) -> Result<String, String> {
    let request = IssueUserVerification {
        subject_token: access_token.to_string(),
        receiving_app: "ting".into(),
        scopes: vec!["subscriptions.register".into()],
        access_ttl_seconds: Some(60),
    };
    let key = uuid::Uuid::new_v4().to_string();
    let proof = app
        .accounts
        .app()
        .issue_user_verification(&request, Some(&key))
        .await
        .map_err(|e| format!("proof_failed: {}", e.code()))?;
    let response = app
        .ting
        .http
        .post(format!("{}/v1/subscriptions", app.ting.url))
        .bearer_auth(proof.proof_token.expose())
        .json(&json!({"app_id": app.accounts.app_id(), "for": uuid}))
        .send()
        .await
        .map_err(|e| {
            if e.is_timeout() {
                "ting_timeout".to_string()
            } else {
                "ting_unreachable".to_string()
            }
        })?;
    let status = response.status().as_u16();
    let body: Value = response.json().await.unwrap_or(Value::Null);
    if !(200..300).contains(&status) {
        let code = body["error"]["code"].as_str().unwrap_or("ting_error");
        return Err(format!("{code} ({status})"));
    }
    body["id"]
        .as_str()
        .map(String::from)
        .ok_or_else(|| "ting_unexpected_response".to_string())
}

/// Best-effort registration right after a silicon signs in. Signing in turns notifications on
/// (udd/cli.md), even if the silicon had turned them off before.
pub fn spawn_register(app: Arc<App>, uuid: String, access_token: String) {
    tokio::spawn(async move {
        enable(&app, &uuid, &access_token).await;
    });
}

/// Registers the subscription and records the outcome; notifications stay enabled either way so
/// a later sign-in or `POST /v1/notifications` can retry.
async fn enable(app: &App, uuid: &str, access_token: &str) {
    match register(app, uuid, access_token).await {
        Ok(id) => {
            tracing::info!("Ting subscription registered");
            set_status(app, uuid, true, Some(&id), None);
        }
        Err(error) => {
            tracing::warn!(
                error = error.as_str(),
                "Ting subscription registration failed"
            );
            let current: Option<String> = app
                .db
                .with(|c| {
                    c.query_row(
                        "SELECT subscription_id FROM ting_subscriptions WHERE account_uuid = ?1",
                        [uuid],
                        |r| r.get(0),
                    )
                    .optional()
                })
                .ok()
                .flatten()
                .flatten();
            set_status(app, uuid, true, current.as_deref(), Some(&error));
        }
    }
}

async fn revoke(app: &App, subscription_id: &str) -> Result<(), String> {
    let proof = app.ting.app_proof(app, false).await?;
    let response = app
        .ting
        .http
        .post(format!("{}/v1/subscriptions/revoke", app.ting.url))
        .bearer_auth(proof)
        .json(&json!({"app_id": app.accounts.app_id(), "id": subscription_id}))
        .send()
        .await
        .map_err(|_| "ting_unreachable".to_string())?;
    let status = response.status().as_u16();
    if (200..300).contains(&status) || status == 404 {
        Ok(())
    } else {
        let body: Value = response.json().await.unwrap_or(Value::Null);
        Err(format!(
            "{} ({status})",
            body["error"]["code"].as_str().unwrap_or("ting_error")
        ))
    }
}

fn status(app: &App, uuid: &str) -> ApiResult<Notifications> {
    let row: Option<(i64, Option<String>, Option<String>)> = app.db.with(|c| {
        c.query_row("SELECT enabled, subscription_id, last_error FROM ting_subscriptions WHERE account_uuid = ?1", [uuid], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .optional()
    })?;
    let (enabled, subscription_id, last_error) = row.unwrap_or((0, None, None));
    Ok(Notifications {
        ting: TingStatus {
            enabled: enabled == 1,
            subscription_id,
            last_error,
        },
    })
}

pub async fn get_notifications(
    State(app): State<AppState>,
    auth: Auth,
) -> ApiResult<Json<Notifications>> {
    if !auth.is_silicon() {
        return Err(ApiError::forbidden(
            "Ring notifies silicons through Ting; carbons get push notifications.",
        ));
    }
    Ok(Json(status(&app, auth.uuid())?))
}

pub async fn set_notifications(
    State(app): State<AppState>,
    auth: Auth,
    ApiJson(req): ApiJson<SetNotifications>,
) -> ApiResult<Json<Notifications>> {
    if !auth.is_silicon() {
        return Err(ApiError::forbidden(
            "Ring notifies silicons through Ting; carbons get push notifications.",
        ));
    }
    let uuid = auth.uuid().to_string();
    if req.enabled {
        let access = crate::accounts::fresh_access_token(&app, auth.device_id()).await?;
        enable(&app, &uuid, access.expose()).await;
    } else {
        let current = status(&app, &uuid)?.ting.subscription_id;
        let mut last_error = None;
        if let Some(id) = current
            && let Err(error) = revoke(&app, &id).await
        {
            tracing::warn!(error = error.as_str(), "Ting subscription revoke failed");
            last_error = Some(error);
        }
        set_status(&app, &uuid, false, None, last_error.as_deref());
    }
    Ok(Json(status(&app, &uuid)?))
}
