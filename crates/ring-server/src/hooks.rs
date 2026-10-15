//! Inbound webhooks: Silicon Accounts (HMAC-signed) and LiveKit (JWT-signed, loopback only).

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use rusqlite::params;
use silicon_accounts_client::{
    DEFAULT_WEBHOOK_TOLERANCE, SIGNATURE_HEADER, TIMESTAMP_HEADER, WebhookPayload,
    verify_and_parse_webhook,
};

use crate::accounts;
use crate::app::AppState;
use crate::calls::engine;
use crate::error::ApiError;
use crate::extract::{Peer, is_direct_loopback};
use crate::util::now_ms;

/// Records an event id; false when it was already handled (retries and replays reuse it).
fn first_delivery(app: &AppState, source: &str, event_id: &str) -> bool {
    app.db
        .with(|c| {
            c.execute(
                "INSERT OR IGNORE INTO webhook_events (event_id, source, received_at) VALUES (?1, ?2, ?3)",
                params![format!("{source}:{event_id}"), source, now_ms()],
            )
        })
        .map(|n| n == 1)
        .unwrap_or(false)
}

pub async fn accounts_webhook(
    State(app): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Some(secret) = app.config.accounts_webhook_secret.clone() else {
        return ApiError::not_found("Route").into_response();
    };
    let header = |name: &str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string()
    };
    let event = match verify_and_parse_webhook(
        &secret,
        &header(TIMESTAMP_HEADER),
        &header(SIGNATURE_HEADER),
        &body,
        DEFAULT_WEBHOOK_TOLERANCE,
    ) {
        Ok(event) => event,
        Err(error) => {
            tracing::warn!(%error, "rejected an Accounts webhook");
            return ApiError::new(
                StatusCode::UNAUTHORIZED,
                ring_proto::codes::UNAUTHENTICATED,
                "Invalid webhook signature.",
            )
            .into_response();
        }
    };
    if event
        .app_id
        .as_deref()
        .is_some_and(|a| a != app.accounts.app_id())
    {
        return ApiError::forbidden("This webhook is for another app.").into_response();
    }
    if !first_delivery(&app, "accounts", &event.event_id) {
        return StatusCode::NO_CONTENT.into_response();
    }
    tracing::info!(event_type = event.event_type.as_str(), "Accounts webhook");
    let occurred = event
        .occurred_at
        .map(|t| (t.unix_timestamp_nanos() / 1_000_000) as i64);
    let app2 = app.clone();
    tokio::spawn(async move {
        let app = app2;
        match event.payload {
            WebhookPayload::AccountIdChanged(e) => {
                if !e.new_id.trim().is_empty() {
                    let _ = app.db.with(|c| {
                        c.execute(
                            "UPDATE accounts SET id = ?2, updated_at = ?3 WHERE uuid = ?1",
                            params![e.uuid, e.new_id.trim().to_ascii_lowercase(), now_ms()],
                        )
                    });
                }
                accounts::refresh_profile(&app, &e.uuid).await;
            }
            WebhookPayload::AccountUpdated(e) => {
                accounts::refresh_profile(&app, &e.uuid).await;
            }
            WebhookPayload::MembershipSignedOut(e) => {
                // One sign-in ended (Ring revoking a single device sends this too), so check each
                // session instead of signing the account out everywhere.
                crate::sessions::revoke_ended_sign_ins(&app, &e.uuid, occurred).await;
            }
            WebhookPayload::MembershipAccessRemoved(e) => {
                crate::sessions::revoke_account(&app, &e.uuid, "accounts_access_removed", occurred)
                    .await;
                let _ = app.db.with(|c| {
                    c.execute("UPDATE accounts SET on_ring = 0 WHERE uuid = ?1", [&e.uuid])
                });
            }
            WebhookPayload::AccountDeleted(e) => {
                crate::sessions::revoke_account(&app, &e.uuid, "accounts_deleted", None).await;
                let _ = app.db.with(|c| {
                    c.execute(
                        "UPDATE accounts SET on_ring = 0, status = 'deleted' WHERE uuid = ?1",
                        [&e.uuid],
                    )
                });
            }
            _ => {}
        }
    });
    StatusCode::NO_CONTENT.into_response()
}

pub async fn livekit_webhook(
    State(app): State<AppState>,
    Peer(peer): Peer,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !is_direct_loopback(&headers, peer) {
        return ApiError::not_found("Route").into_response();
    }
    let authorization = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let Ok(body) = std::str::from_utf8(&body) else {
        return ApiError::invalid("Body is not UTF-8.").into_response();
    };
    let event = match app.livekit.verify_webhook(body, authorization) {
        Ok(event) => event,
        Err(error) => {
            tracing::warn!(%error, "rejected a LiveKit webhook");
            return ApiError::unauthenticated().into_response();
        }
    };
    if !event.id.is_empty() && !first_delivery(&app, "livekit", &event.id) {
        return StatusCode::OK.into_response();
    }
    if let Some(identity) = event.identity.as_deref() {
        match event.event.as_str() {
            "participant_joined" => engine::media_joined(&app, &event.room, identity),
            "participant_left" | "participant_connection_aborted" => {
                engine::media_left(&app, &event.room, identity)
            }
            _ => {}
        }
    }
    StatusCode::OK.into_response()
}
