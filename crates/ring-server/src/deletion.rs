//! Deleting a Ring account: `DELETE /v1/me`, and Accounts' `account.deleted`. The account leaves
//! its live calls, every device is signed out, and its own data is deleted. A call it shared with
//! someone who still uses Ring stays for them unchanged; any other call is deleted entirely. The
//! Silicon Account itself is never touched.
//!
//! A deletion is recorded (`pending_deletions`) before it starts and the record goes in the same
//! transaction that deletes the data, so one interrupted by an error or a restart is retried until
//! it completes. While it is pending the account cannot sign in, start, answer or join a call (the
//! checks run inside those transactions, so the deletion always sees what they did). Stored audio
//! is deleted from S3 the same way: its keys are recorded (`pending_files`) with the change that
//! made it unneeded and retried until S3 confirms.

use std::time::Duration;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use ring_proto::{DeleteAccount, VoicemailReason};
use rusqlite::{Connection, OptionalExtension, params};

use crate::app::AppState;
use crate::error::{ApiError, ApiResult};
use crate::extract::{CLIENT_HEADER, SESSION_COOKIE, authenticate, session_token};
use crate::util::{now_ms, sha256_hex};

/// How often pending deletions and stored audio are retried.
const RETRY_EVERY: Duration = Duration::from_secs(60);

/// Who deleted the account. It decides the sessions' revoke reason and whether their Accounts
/// sign-ins are revoked (after `account.deleted` Accounts has already ended them).
#[derive(Clone, Copy, PartialEq)]
pub enum Initiator {
    /// The person, with `DELETE /v1/me`.
    User,
    /// Silicon Accounts, which deleted the whole account.
    Accounts,
}

impl Initiator {
    fn revoke_reason(self) -> &'static str {
        match self {
            Initiator::User => "account_deleted",
            Initiator::Accounts => "accounts_deleted",
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Initiator::User => "user",
            Initiator::Accounts => "accounts",
        }
    }
}

// ---- DELETE /v1/me ----------------------------------------------------------------------------

pub async fn delete_me(
    State(app): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Response> {
    let (token, via_cookie) = session_token(&headers).ok_or_else(ApiError::unauthenticated)?;
    if via_cookie && !headers.contains_key(CLIENT_HEADER) {
        return Err(ApiError::csrf());
    }
    // A device this deletion signed out may repeat the request (its response was lost): that
    // succeeds again, and finishes the deletion if it is still pending.
    let target = match authenticate(&app, &token, via_cookie) {
        Ok(auth) => Some(auth.account.uuid),
        Err(error) => match deleted_session(&app, &token)? {
            Some(uuid) => app.db.with(|c| pending(c, &uuid))?.then_some(uuid),
            None => return Err(error),
        },
    };
    let confirmed = serde_json::from_slice::<DeleteAccount>(&body)
        .is_ok_and(|request| request.confirm == DeleteAccount::CONFIRM);
    if !confirmed {
        return Err(ApiError::invalid(
            "Send {\"confirm\":\"delete\"} to delete your Ring account.",
        )
        .hint("This cannot be undone."));
    }
    if let Some(uuid) = target {
        delete_account(&app, &uuid, Initiator::User).await?;
    }
    let mut response = StatusCode::NO_CONTENT.into_response();
    if via_cookie {
        let cookie = format!(
            "{SESSION_COOKIE}=; {}",
            crate::sessions::cookie_attrs(&app, "/", 0)
        );
        if let Ok(v) = HeaderValue::from_str(&cookie) {
            response.headers_mut().insert(header::SET_COOKIE, v);
        }
    }
    Ok(response)
}

/// For a session that an account deletion signed out: the account.
fn deleted_session(app: &AppState, token: &str) -> rusqlite::Result<Option<String>> {
    let hash = sha256_hex(token.as_bytes());
    app.db.with(|c| {
        c.query_row(
            "SELECT account_uuid FROM sessions
             WHERE token_hash = ?1 AND revoked_at IS NOT NULL AND revoke_reason IN ('account_deleted', 'accounts_deleted')",
            [hash],
            |r| r.get(0),
        )
        .optional()
    })
}

// ---- the queue --------------------------------------------------------------------------------

/// True while the account's Ring account is being deleted.
pub fn pending(conn: &Connection, uuid: &str) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM pending_deletions WHERE account_uuid = ?1)",
        [uuid],
        |r| r.get(0),
    )
}

/// True while the account's Ring account is being deleted, and after it was deleted until the
/// account signs in to Ring again (signing in creates its settings anew). A call or invitation to
/// it then goes as to an account that is not on Ring: it reaches none of its devices (it may still
/// have some while the deletion runs) and times out unanswered, and nobody can leave it a
/// voicemail either.
pub fn gone(conn: &Connection, uuid: &str) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM pending_deletions WHERE account_uuid = ?1)
           OR EXISTS (SELECT 1 FROM accounts WHERE uuid = ?1 AND ring_deleted_at IS NOT NULL
                        AND NOT EXISTS (SELECT 1 FROM settings WHERE account_uuid = ?1))",
        [uuid],
        |r| r.get(0),
    )
}

/// Records that the account's Ring account is to be deleted. Accounts' deletion wins over the
/// person's own if both are pending.
pub fn enqueue(conn: &Connection, uuid: &str, initiator: Initiator) -> rusqlite::Result<()> {
    let now = now_ms();
    conn.execute(
        "INSERT INTO pending_deletions (account_uuid, initiator, requested_at, next_attempt_at) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(account_uuid) DO UPDATE SET
           initiator = CASE WHEN excluded.initiator = 'accounts' THEN 'accounts' ELSE initiator END",
        params![uuid, initiator.as_str(), now, now + RETRY_EVERY.as_millis() as i64],
    )?;
    Ok(())
}

/// Retries after 1, 2, 4 … minutes, at most an hour apart.
fn backoff_ms(attempts: i64) -> i64 {
    (60_000i64 << attempts.clamp(0, 6)).min(3_600_000)
}

/// Deletes an account's Ring data: records the deletion, then runs it. When it fails part-way the
/// error is returned and the deletion is retried in the background until it completes.
pub async fn delete_account(app: &AppState, uuid: &str, initiator: Initiator) -> ApiResult<()> {
    app.db.with(|c| enqueue(c, uuid, initiator))?;
    attempt(app, uuid).await
}

/// Runs a pending deletion; on failure, schedules the next attempt.
pub async fn attempt(app: &AppState, uuid: &str) -> ApiResult<()> {
    let result = run(app, uuid).await;
    if let Err(error) = &result {
        tracing::warn!(error = %error.message, "Ring account deletion failed; it will be retried");
        let _ = app.db.with(|c| {
            let attempts: i64 = c
                .query_row(
                    "SELECT attempts FROM pending_deletions WHERE account_uuid = ?1",
                    [uuid],
                    |r| r.get(0),
                )
                .optional()?
                .unwrap_or(0);
            c.execute(
                "UPDATE pending_deletions SET attempts = attempts + 1, next_attempt_at = ?2, last_error = ?3 WHERE account_uuid = ?1",
                params![uuid, now_ms() + backoff_ms(attempts), error.message],
            )
        });
    }
    result
}

/// Retries pending deletions and stored audio deletions, at startup and every minute.
pub async fn worker(app: AppState) {
    let mut tick = tokio::time::interval(RETRY_EVERY);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    if app.s3.is_none() {
        let waiting: i64 = app
            .db
            .with(|c| c.query_row("SELECT COUNT(*) FROM pending_files", [], |r| r.get(0)))
            .unwrap_or(0);
        if waiting > 0 {
            tracing::warn!(
                files = waiting,
                "storage is not configured; stored audio waiting to be deleted stays queued"
            );
        }
    }
    loop {
        tick.tick().await;
        let due: Vec<String> = app
            .db
            .with(|c| {
                let mut stmt = c.prepare(
                    "SELECT account_uuid FROM pending_deletions WHERE next_attempt_at <= ?1 ORDER BY requested_at",
                )?;
                let rows = stmt.query_map([now_ms()], |r| r.get(0))?;
                rows.collect()
            })
            .unwrap_or_else(|error| {
                tracing::error!(%error, "could not read pending deletions");
                vec![]
            });
        for uuid in due {
            let _ = attempt(&app, &uuid).await;
        }
        delete_due_files(&app).await;
    }
}

// ---- the deletion -----------------------------------------------------------------------------

/// Deletes a pending account's Ring data. Running it again is safe: each step only finds what is
/// left, and it does nothing once the deletion has completed.
async fn run(app: &AppState, uuid: &str) -> ApiResult<()> {
    let _guard = app.call_locks.lock(&format!("account:{uuid}")).await;
    let initiator: Option<String> = app.db.with(|c| {
        c.query_row(
            "SELECT initiator FROM pending_deletions WHERE account_uuid = ?1",
            [uuid],
            |r| r.get(0),
        )
        .optional()
    })?;
    let Some(initiator) = initiator else {
        return Ok(());
    };
    let initiator = if initiator == "accounts" {
        Initiator::Accounts
    } else {
        Initiator::User
    };
    // Its voicemail greetings, read before its settings go, so ring-media forgets them too.
    let greetings = app.db.with(|c| greetings(c, uuid))?;

    // 1. Off every live call, on every device.
    crate::calls::engine::withdraw(app, uuid).await?;

    // 2. Every device signed out: no requests, push tokens or Web Push subscriptions, sockets
    //    closed, and (for silicons) no Ting subscription.
    let devices: Vec<String> = app.db.with(|c| {
        let mut stmt = c.prepare(
            "SELECT device_id FROM sessions WHERE account_uuid = ?1 AND revoked_at IS NULL",
        )?;
        let rows = stmt.query_map([uuid], |r| r.get(0))?;
        rows.collect()
    })?;
    for device in &devices {
        crate::sessions::revoke_session(
            app,
            device,
            initiator.revoke_reason(),
            matches!(initiator, Initiator::User),
        )
        .await;
    }
    crate::ting::forget(app, uuid).await;

    // 3. Its own data, and the calls nobody else keeps, in one transaction that also records
    //    their stored audio for deletion and completes the pending deletion.
    let removed = app.db.tx(|tx| -> rusqlite::Result<Removed> {
        let removed = delete_data(tx, uuid, initiator, now_ms())?;
        queue_files(tx, removed.files.iter())?;
        tx.execute(
            "DELETE FROM pending_deletions WHERE account_uuid = ?1",
            [uuid],
        )?;
        Ok(removed)
    })?;
    tracing::info!(
        devices = devices.len(),
        calls_deleted = removed.calls_deleted,
        calls_kept = removed.calls_kept,
        files = removed.files.len(),
        "Ring account deleted"
    );

    // 4. Stored audio and cached greetings, after the commit.
    flush_files(app);
    if let Some((voice, texts)) = greetings {
        app.media.forget_greetings(&voice, texts);
    }
    Ok(())
}

/// The account's voice and its three effective voicemail greetings.
fn greetings(conn: &Connection, uuid: &str) -> rusqlite::Result<Option<(String, Vec<String>)>> {
    let Some(account) = crate::accounts::get(conn, uuid)? else {
        return Ok(None);
    };
    let settings = crate::settings::load(conn, uuid)?.settings;
    let texts = [
        VoicemailReason::Busy,
        VoicemailReason::Declined,
        VoicemailReason::Missed,
    ]
    .into_iter()
    .map(|reason| crate::settings::greeting_text(&settings, &account, reason))
    .collect();
    Ok(Some((settings.voice, texts)))
}

struct Removed {
    files: Vec<String>,
    calls_deleted: usize,
    calls_kept: usize,
}

fn delete_data(
    tx: &Connection,
    uuid: &str,
    initiator: Initiator,
    now: i64,
) -> rusqlite::Result<Removed> {
    let mut files = Vec::new();

    // Voicemails left for it. A call that keeps going for its caller no longer points at one.
    let voicemails: Vec<(String, Option<String>)> = {
        let mut stmt =
            tx.prepare("SELECT voicemail_id, audio_key FROM voicemails WHERE to_uuid = ?1")?;
        let rows = stmt.query_map([uuid], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    for (voicemail_id, audio_key) in voicemails {
        tx.execute(
            "UPDATE calls SET voicemail_id = NULL WHERE voicemail_id = ?1",
            [&voicemail_id],
        )?;
        files.extend(audio_key);
    }
    tx.execute("DELETE FROM voicemails WHERE to_uuid = ?1", [uuid])?;

    // What only it could see: private transcript entries (a silicon's thinking, commentary and
    // delegations), delegations with their replies, representative messages and context.
    tx.execute(
        "DELETE FROM transcript_entries WHERE owner_uuid = ?1",
        [uuid],
    )?;
    tx.execute(
        "DELETE FROM delegation_replies WHERE delegation_id IN (SELECT delegation_id FROM delegations WHERE silicon_uuid = ?1)",
        [uuid],
    )?;
    tx.execute("DELETE FROM delegations WHERE silicon_uuid = ?1", [uuid])?;
    tx.execute("DELETE FROM messages WHERE silicon_uuid = ?1", [uuid])?;
    tx.execute(
        "UPDATE participants SET rep_context = NULL WHERE account_uuid = ?1",
        [uuid],
    )?;

    // Settings (voice, greetings, default context), context approvals, notifications and
    // remembered responses.
    tx.execute("DELETE FROM settings WHERE account_uuid = ?1", [uuid])?;
    tx.execute(
        "DELETE FROM context_preparations WHERE account_uuid = ?1",
        [uuid],
    )?;
    tx.execute(
        "DELETE FROM context_approvals WHERE account_uuid = ?1",
        [uuid],
    )?;
    tx.execute(
        "DELETE FROM ting_subscriptions WHERE account_uuid = ?1",
        [uuid],
    )?;
    tx.execute("DELETE FROM ting_outbox WHERE recipient_uuid = ?1", [uuid])?;
    tx.execute(
        "DELETE FROM silicon_reservations WHERE account_uuid = ?1",
        [uuid],
    )?;
    tx.execute(
        "DELETE FROM idempotency WHERE substr(scope, 1, length(?1) + 1) = ?1 || ':'",
        [uuid],
    )?;
    // The blocks it made. Blocks others made of it are theirs and stay.
    tx.execute("DELETE FROM blocks WHERE blocker_uuid = ?1", [uuid])?;

    // Devices stay only as signed-out rows without names, so a repeated request is recognised.
    tx.execute(
        "DELETE FROM push_tokens WHERE device_id IN (SELECT device_id FROM sessions WHERE account_uuid = ?1)",
        [uuid],
    )?;
    tx.execute(
        "DELETE FROM invitation_pushes WHERE device_id IN (SELECT device_id FROM sessions WHERE account_uuid = ?1)",
        [uuid],
    )?;
    tx.execute(
        "UPDATE sessions SET name = NULL, app_version = NULL, refresh_sealed = NULL WHERE account_uuid = ?1",
        [uuid],
    )?;
    // A session that step 2 could not revoke (a database error there is only logged).
    tx.execute(
        "UPDATE sessions SET revoked_at = ?2, revoke_reason = ?3 WHERE account_uuid = ?1 AND revoked_at IS NULL",
        params![uuid, now, initiator.revoke_reason()],
    )?;

    // Its calls (since any earlier deletion). One that someone else still has stays for them;
    // the rest are deleted with everything in them.
    let since = crate::accounts::ring_deleted_at(tx, uuid)?.unwrap_or(0);
    let calls: Vec<(String, i64, String)> = {
        let mut stmt = tx.prepare(
            "SELECT ringid, created_at, state FROM calls WHERE created_at >= ?2 AND (created_by = ?1
               OR ringid IN (SELECT ringid FROM participants WHERE account_uuid = ?1)
               OR ringid IN (SELECT ringid FROM invitations WHERE to_uuid = ?1 OR from_uuid = ?1))",
        )?;
        let rows = stmt.query_map(params![uuid, since], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    let (mut calls_deleted, mut calls_kept) = (0, 0);
    for (ringid, created_at, state) in calls {
        // A call still live here has other people on it (the account just left it).
        if state != "ended" || kept_by_others(tx, &ringid, uuid, created_at)? {
            calls_kept += 1;
            continue;
        }
        delete_call(tx, &ringid, &mut files)?;
        calls_deleted += 1;
    }

    // The profile: no longer on Ring, and nothing from before now is its own. Name and photo stay
    // as the Silicon Account shows them, for the calls the others keep. After Accounts deleted the
    // account it is `deleted` too.
    tx.execute(
        "UPDATE accounts SET on_ring = 0, ring_deleted_at = ?2, updated_at = ?2,
           status = CASE WHEN ?3 THEN 'deleted' ELSE status END WHERE uuid = ?1",
        params![uuid, now, initiator == Initiator::Accounts],
    )?;
    Ok(Removed {
        files,
        calls_deleted,
        calls_kept,
    })
}

/// True when someone else on the call still has it: they can see it (an invitation from someone
/// they blocked does not count), they use Ring (they have settings, which signing in creates) and
/// they have not deleted their Ring account since the call.
fn kept_by_others(
    tx: &Connection,
    ringid: &str,
    uuid: &str,
    created_at: i64,
) -> rusqlite::Result<bool> {
    tx.query_row(
        "WITH members(uuid) AS (
           SELECT created_by FROM calls WHERE ringid = ?1
           UNION SELECT account_uuid FROM participants WHERE ringid = ?1
           UNION SELECT to_uuid FROM invitations WHERE ringid = ?1 AND hidden = 0
           UNION SELECT from_uuid FROM invitations WHERE ringid = ?1
         )
         SELECT EXISTS (
           SELECT 1 FROM members m JOIN settings s ON s.account_uuid = m.uuid LEFT JOIN accounts a ON a.uuid = m.uuid
           WHERE m.uuid != ?2 AND (a.ring_deleted_at IS NULL OR a.ring_deleted_at <= ?3)
         )",
        params![ringid, uuid, created_at],
        |r| r.get(0),
    )
}

/// Deletes a call with its transcript, delegations, messages, voicemails, invitations and
/// participants. Its recording and voicemail audio are added to `files`.
fn delete_call(tx: &Connection, ringid: &str, files: &mut Vec<String>) -> rusqlite::Result<()> {
    let recording: Option<String> = tx
        .query_row(
            "SELECT recording_key FROM calls WHERE ringid = ?1",
            [ringid],
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    files.extend(recording);
    {
        let mut stmt = tx.prepare(
            "SELECT audio_key FROM voicemails WHERE ringid = ?1 AND audio_key IS NOT NULL",
        )?;
        let keys = stmt.query_map([ringid], |r| r.get::<_, String>(0))?;
        for key in keys {
            files.push(key?);
        }
    }
    tx.execute(
        "DELETE FROM delegation_replies WHERE delegation_id IN (SELECT delegation_id FROM delegations WHERE ringid = ?1)",
        [ringid],
    )?;
    tx.execute(
        "DELETE FROM invitation_pushes WHERE invitation_id IN (SELECT invitation_id FROM invitations WHERE ringid = ?1)",
        [ringid],
    )?;
    for table in [
        "delegations",
        "messages",
        "transcript_entries",
        "voicemails",
        "invitations",
        "participants",
        "silicon_reservations",
        "calls",
    ] {
        tx.execute(&format!("DELETE FROM {table} WHERE ringid = ?1"), [ringid])?;
    }
    Ok(())
}

// ---- stored audio -----------------------------------------------------------------------------

/// Records stored audio (S3 keys) to delete, in the transaction that made it unneeded.
pub fn queue_files<'a>(
    conn: &Connection,
    keys: impl IntoIterator<Item = &'a String>,
) -> rusqlite::Result<()> {
    let now = now_ms();
    for key in keys {
        conn.execute(
            "INSERT OR IGNORE INTO pending_files (key, created_at, next_attempt_at) VALUES (?1, ?2, ?2)",
            params![key, now],
        )?;
    }
    Ok(())
}

/// Records stored audio to delete and starts deleting it.
pub fn delete_stored(app: &AppState, keys: Vec<String>) {
    if let Err(error) = app.db.tx(|tx| queue_files(tx, keys.iter())) {
        tracing::error!(%error, "could not queue stored audio for deletion");
    }
    flush_files(app);
}

/// Starts deleting the stored audio that is due, in the background.
pub fn flush_files(app: &AppState) {
    let app = app.clone();
    tokio::spawn(async move { delete_due_files(&app).await });
}

/// Deletes the due stored audio from S3. A key goes from the queue once S3 confirms (deleting an
/// object that is already gone succeeds); a failure is retried later.
async fn delete_due_files(app: &AppState) {
    static SWEEPING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let Some(s3) = app.s3.clone() else {
        return;
    };
    let _sweeping = SWEEPING.lock().await;
    loop {
        let due: Vec<(String, i64)> = match app.db.with(|c| {
            let mut stmt = c.prepare(
                "SELECT key, attempts FROM pending_files WHERE next_attempt_at <= ?1 ORDER BY next_attempt_at LIMIT 100",
            )?;
            let rows = stmt.query_map([now_ms()], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
        }) {
            Ok(due) => due,
            Err(error) => {
                tracing::error!(%error, "could not read stored audio to delete");
                return;
            }
        };
        if due.is_empty() {
            return;
        }
        for (key, attempts) in due {
            let result = match s3.delete(&key).await {
                Ok(()) => app
                    .db
                    .with(|c| c.execute("DELETE FROM pending_files WHERE key = ?1", [&key])),
                Err(error) => {
                    tracing::warn!(%error, attempts, "could not delete stored audio; it will be retried");
                    app.db.with(|c| {
                        c.execute(
                            "UPDATE pending_files SET attempts = attempts + 1, next_attempt_at = ?2, last_error = ?3 WHERE key = ?1",
                            params![key, now_ms() + backoff_ms(attempts), error],
                        )
                    })
                }
            };
            if let Err(error) = result {
                tracing::error!(%error, "could not update stored audio to delete");
                return;
            }
        }
    }
}
