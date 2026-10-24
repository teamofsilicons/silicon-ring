//! Blocking: `GET /v1/blocks`, `PUT /v1/blocks/{ref}`, `DELETE /v1/blocks/{ref}`.
//!
//! A blocked account can still call the blocker, but the call rings none of the blocker's devices
//! (no push, no Ting, no voicemail, no missed-call notice) and ends unanswered when it times out;
//! an invitation from it into another call goes the same way. To the blocked account it looks
//! like any call nobody answers. The blocker's own calls are unaffected.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use ring_proto::{Account, Items, RingingStoppedReason};
use rusqlite::{Connection, params};

use crate::accounts;
use crate::app::AppState;
use crate::calls::{self, Effects};
use crate::error::{ApiError, ApiResult};
use crate::extract::Auth;
use crate::util::now_ms;

/// True when `blocker` has blocked `blocked`.
pub fn is_blocked(conn: &Connection, blocker: &str, blocked: &str) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM blocks WHERE blocker_uuid = ?1 AND blocked_uuid = ?2)",
        params![blocker, blocked],
        |r| r.get(0),
    )
}

pub async fn list(State(app): State<AppState>, auth: Auth) -> ApiResult<Json<Items<Account>>> {
    let items = app.db.with(|c| -> rusqlite::Result<Vec<Account>> {
        let mut stmt = c.prepare(
            "SELECT blocked_uuid FROM blocks WHERE blocker_uuid = ?1 ORDER BY created_at DESC, blocked_uuid",
        )?;
        let uuids: Vec<String> = stmt
            .query_map([auth.uuid()], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let found = accounts::load_many(c, uuids.iter().map(String::as_str))?;
        Ok(uuids
            .iter()
            .map(|uuid| {
                found
                    .get(uuid)
                    .cloned()
                    .unwrap_or_else(|| accounts::placeholder(uuid))
            })
            .collect())
    })?;
    Ok(Json(Items { items }))
}

pub async fn put(
    State(app): State<AppState>,
    auth: Auth,
    Path(reference): Path<String>,
) -> ApiResult<StatusCode> {
    let blocked = accounts::resolve(&app, &reference).await?;
    if blocked.uuid == auth.uuid() {
        return Err(ApiError::invalid("You can't block yourself."));
    }
    app.db.with(|c| {
        c.execute(
            "INSERT OR IGNORE INTO blocks (blocker_uuid, blocked_uuid, created_at) VALUES (?1, ?2, ?3)",
            params![auth.uuid(), blocked.uuid, now_ms()],
        )
    })?;
    silence_ringing(&app, auth.uuid(), &blocked.uuid).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn delete(
    State(app): State<AppState>,
    auth: Auth,
    Path(reference): Path<String>,
) -> ApiResult<StatusCode> {
    // Known accounts are looked up here, so an account that no longer exists can be unblocked.
    // One Accounts does not know either was never blocked: nothing to do.
    let uuid = match known(&app, &reference)? {
        Some(uuid) => uuid,
        None => match accounts::resolve(&app, &reference).await {
            Ok(account) => account.uuid,
            Err(error) if error.code == ring_proto::codes::ACCOUNT_NOT_FOUND => {
                return Ok(StatusCode::NO_CONTENT);
            }
            Err(error) => return Err(error),
        },
    };
    app.db.with(|c| {
        c.execute(
            "DELETE FROM blocks WHERE blocker_uuid = ?1 AND blocked_uuid = ?2",
            params![auth.uuid(), uuid],
        )
    })?;
    Ok(StatusCode::NO_CONTENT)
}

/// The uuid of an account Ring already knows by this reference.
fn known(app: &AppState, reference: &str) -> ApiResult<Option<String>> {
    let Some(normalized) = ring_proto::normalize_actor_ref(reference) else {
        return Ok(None);
    };
    let row = app.db.with(|c| {
        if normalized.starts_with("c:") || normalized.starts_with("si:") {
            accounts::find_by_id(c, &normalized)
        } else {
            accounts::get_row(c, &normalized)
        }
    })?;
    Ok(row.map(|r| r.account.uuid))
}

/// Calls from `blocked` that are ringing `blocker` right now stop ringing on the blocker's devices
/// and go on as if unanswered.
async fn silence_ringing(app: &AppState, blocker: &str, blocked: &str) -> ApiResult<()> {
    let ringing: Vec<(String, String)> = app.db.with(|c| {
        let mut stmt = c.prepare(
            "SELECT ringid, invitation_id FROM invitations WHERE to_uuid = ?1 AND from_uuid = ?2 AND state = 'ringing' AND hidden = 0",
        )?;
        let rows = stmt.query_map(params![blocker, blocked], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect()
    })?;
    for (ringid, invitation_id) in ringing {
        let guard = app.call_locks.lock(&ringid).await;
        let Some(data) = app.db.with(|c| calls::load(c, &ringid))? else {
            continue;
        };
        let Some(inv) = data
            .ringing()
            .find(|i| i.invitation_id == invitation_id && !i.hidden)
        else {
            continue;
        };
        let mut fx = Effects::default();
        let hidden = app.db.with(|c| {
            c.execute(
                "UPDATE invitations SET hidden = 1 WHERE invitation_id = ?1 AND state = 'ringing'",
                [&invitation_id],
            )
        })?;
        if hidden == 1 {
            // The invitation as it was, so the blocker's devices hear that it stopped.
            fx.stop(inv, RingingStoppedReason::Cancelled, None);
        }
        drop(guard);
        fx.apply(app, &ringid).await;
    }
    Ok(())
}
