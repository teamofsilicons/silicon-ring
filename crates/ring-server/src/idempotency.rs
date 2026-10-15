//! `Idempotency-Key` for call creation, invites, messages and voicemail: a retry with the same
//! key and body within 24 h returns the original response; a different body is a conflict.

use axum::body::{Body, to_bytes};
use axum::extract::{Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use rusqlite::{OptionalExtension, params};

use crate::app::AppState;
use crate::error::ApiError;
use crate::extract::{authenticate, session_token};
use crate::util::{now_ms, sha256_hex};

/// (request hash, stored status, stored body)
type Stored = (String, Option<i64>, Option<Vec<u8>>);

const MAX_BODY: usize = 1024 * 1024;
const RETENTION_MS: i64 = 24 * 3600 * 1000;
/// An in-flight marker older than this belongs to a request that never finished.
const ABANDONED_MS: i64 = 2 * 60 * 1000;

fn valid_key(key: &str) -> bool {
    (8..=200).contains(&key.len()) && key.bytes().all(|b| (0x20..=0x7e).contains(&b))
}

pub async fn layer(State(app): State<AppState>, request: Request, next: Next) -> Response {
    let Some(key) = request
        .headers()
        .get("idempotency-key")
        .map(|v| v.to_str().map(str::to_string))
    else {
        return next.run(request).await;
    };
    let Some(key) = key.ok().filter(|k| valid_key(k)) else {
        return ApiError::invalid("Idempotency-Key must be 8–200 printable ASCII characters.")
            .into_response();
    };
    // Unauthenticated requests go straight to the handler, which rejects them.
    let Some(auth) = session_token(request.headers())
        .and_then(|(token, cookie)| authenticate(&app, &token, cookie).ok())
    else {
        return next.run(request).await;
    };
    let (parts, body) = request.into_parts();
    let Ok(bytes) = to_bytes(body, MAX_BODY).await else {
        return ApiError::invalid("The request body is too large.").into_response();
    };
    let scope = format!(
        "{}:{}:{}:{}",
        auth.account.uuid,
        parts.method,
        parts.uri.path(),
        key
    );
    let hash = sha256_hex(
        &[
            parts.method.as_str().as_bytes(),
            b" ",
            parts.uri.path().as_bytes(),
            b"\n",
            &bytes,
        ]
        .concat(),
    );
    let now = now_ms();

    let existing = app.db.tx(|tx| -> rusqlite::Result<Option<Stored>> {
        tx.execute(
            "DELETE FROM idempotency WHERE scope = ?1 AND (created_at < ?2 OR (status IS NULL AND created_at < ?3))",
            params![scope, now - RETENTION_MS, now - ABANDONED_MS],
        )?;
        let row = tx
            .query_row("SELECT request_hash, status, body FROM idempotency WHERE scope = ?1", [&scope], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .optional()?;
        if row.is_none() {
            tx.execute("INSERT INTO idempotency (scope, request_hash, created_at) VALUES (?1, ?2, ?3)", params![scope, hash, now])?;
        }
        Ok(row)
    });
    match existing {
        Err(error) => {
            tracing::error!(%error, "idempotency lookup failed");
            return ApiError::internal().into_response();
        }
        Ok(Some((stored_hash, _, _))) if stored_hash != hash => {
            return ApiError::new(
                StatusCode::CONFLICT,
                ring_proto::codes::IDEMPOTENCY_CONFLICT,
                "This Idempotency-Key was used with a different request.",
            )
            .hint("Use a new key for a new request.")
            .into_response();
        }
        Ok(Some((_, Some(status), Some(body)))) => {
            let mut response = Response::new(Body::from(body));
            *response.status_mut() = StatusCode::from_u16(status as u16).unwrap_or(StatusCode::OK);
            response.headers_mut().insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            );
            response
                .headers_mut()
                .insert("idempotent-replayed", HeaderValue::from_static("true"));
            return response;
        }
        Ok(Some(_)) => {
            return ApiError::conflict(
                "A request with this Idempotency-Key is still being processed.",
            )
            .retryable()
            .into_response();
        }
        Ok(None) => {}
    }

    let response = next
        .run(Request::from_parts(parts, Body::from(bytes)))
        .await;
    let (parts, body) = response.into_parts();
    let body = to_bytes(body, 16 * MAX_BODY).await.unwrap_or_default();
    let stored = if parts.status.is_success() {
        app.db.with(|c| {
            c.execute(
                "UPDATE idempotency SET status = ?2, body = ?3 WHERE scope = ?1",
                params![scope, parts.status.as_u16(), body.to_vec()],
            )
        })
    } else {
        // Failures are not remembered, so the client can retry with the same key.
        app.db
            .with(|c| c.execute("DELETE FROM idempotency WHERE scope = ?1", [&scope]))
    };
    if let Err(error) = stored {
        tracing::error!(%error, "could not store idempotent response");
    }
    Response::from_parts(parts, Body::from(body))
}
