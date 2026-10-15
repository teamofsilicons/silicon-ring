//! Voicemail: storage, delivery (WebSocket, alert push, Ting) and the voicemail endpoints.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use ring_proto::{
    AudioStatus, EntryKind, Event, Page, Time, TranscriptEntry, TranscriptStatus, Voicemail,
    VoicemailPatch, VoicemailReason,
};
use rusqlite::{Connection, params, params_from_iter};
use serde::Deserialize;
use serde_json::json;

use crate::accounts;
use crate::app::AppState;
use crate::calls::api::{decode_cursor, encode_cursor, resolve_uuid};
use crate::calls::enum_from;
use crate::error::{ApiError, ApiResult};
use crate::extract::{ApiJson, ApiQuery, Auth};
use crate::ting;
use crate::util::{now_ms, time};

/// Recordings shorter than this with no words are a hang-up during the greeting, not a message.
const MIN_MESSAGE_MS: u64 = 1500;

struct Row {
    vm: Voicemail,
    audio_key: Option<String>,
    delivered: bool,
    deleted: bool,
    recorded: bool,
}

const COLUMNS: &str = "voicemail_id, ringid, from_uuid, to_uuid, reason, decline_reason, created_at, duration_ms, audio_status, transcript, \
     transcript_status, read, audio_key, delivered, deleted_at, text";

fn row(conn: &Connection, r: &rusqlite::Row<'_>) -> rusqlite::Result<Row> {
    let from: String = r.get(2)?;
    let to: String = r.get(3)?;
    let reason: String = r.get(4)?;
    let audio: String = r.get(8)?;
    let transcript_status: String = r.get(10)?;
    let text: Option<String> = r.get(15)?;
    Ok(Row {
        vm: Voicemail {
            voicemail_id: r.get(0)?,
            ringid: r.get(1)?,
            from: accounts::get(conn, &from)?.unwrap_or_else(|| accounts::placeholder(&from)),
            to: accounts::get(conn, &to)?.unwrap_or_else(|| accounts::placeholder(&to)),
            reason: enum_from(&reason).unwrap_or(VoicemailReason::Missed),
            decline_reason: r.get(5)?,
            created_at: time(r.get(6)?),
            duration_ms: r.get::<_, Option<i64>>(7)?.map(|d| d.max(0) as u64),
            audio_status: enum_from(&audio).unwrap_or(AudioStatus::Failed),
            transcript: r.get(9)?,
            transcript_status: enum_from(&transcript_status).unwrap_or(TranscriptStatus::Pending),
            read: r.get::<_, i64>(11)? != 0,
            audio_url: None,
        },
        audio_key: r.get(12)?,
        delivered: r.get::<_, i64>(13)? != 0,
        deleted: r.get::<_, Option<i64>>(14)?.is_some(),
        recorded: text.is_none(),
    })
}

fn load_row(conn: &Connection, id: &str) -> rusqlite::Result<Option<Row>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM voicemails WHERE voicemail_id = ?1"
    ))?;
    let mut rows = stmt.query([id])?;
    match rows.next()? {
        Some(r) => Ok(Some(row(conn, r)?)),
        None => Ok(None),
    }
}

pub fn load(conn: &Connection, id: &str) -> rusqlite::Result<Option<Voicemail>> {
    Ok(load_row(conn, id)?.map(|r| r.vm))
}

/// Marks a voicemail delivered and queues the Ting for a silicon recipient. In a transaction.
pub fn deliver_tx(conn: &Connection, id: &str) -> rusqlite::Result<bool> {
    let Some(r) = load_row(conn, id)? else {
        return Ok(false);
    };
    if r.delivered || r.deleted {
        return Ok(false);
    }
    conn.execute(
        "UPDATE voicemails SET delivered = 1 WHERE voicemail_id = ?1",
        [id],
    )?;
    let payload = json!({
        "voicemail_id": id,
        "ringid": r.vm.ringid,
        "from": r.vm.from,
        "reason": r.vm.reason,
        "transcript": r.vm.transcript,
    });
    ting::enqueue(
        conn,
        &r.vm.to,
        "ring.voicemail.received",
        &format!("voicemail:{id}"),
        payload,
        ting::PRIORITY_INCOMING,
    )
}

/// Tells the recipient (and the sender) about a new or changed voicemail.
pub fn announce(app: &AppState, id: &str, received: bool) {
    let Ok(Some(vm)) = app.db.with(|c| load(c, id)) else {
        return;
    };
    if received {
        app.hub.send_account(
            &vm.to.uuid,
            &Event::VoicemailReceived {
                voicemail: vm.clone(),
            },
        );
        if vm.to.kind == ring_proto::AccountKind::Carbon {
            crate::push::voicemail(app, &vm);
        }
        let from = vm.from.uuid.clone();
        app.hub
            .send_account(&from, &Event::VoicemailUpdated { voicemail: vm });
    } else {
        app.hub.send_account(
            &vm.to.uuid,
            &Event::VoicemailUpdated {
                voicemail: vm.clone(),
            },
        );
        let from = vm.from.uuid.clone();
        app.hub
            .send_account(&from, &Event::VoicemailUpdated { voicemail: vm });
    }
}

/// ring-media finished recording/transcribing or synthesising a voicemail.
pub async fn finished(
    app: &AppState,
    id: &str,
    s3_key: Option<String>,
    duration_ms: Option<u64>,
    transcript: Option<String>,
    error: Option<String>,
) {
    let transcript = transcript
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty());
    // (announce as received?, None when nothing is announced; Ting queued; ringid; new entry)
    type Done = (Option<bool>, bool, String, Option<TranscriptEntry>);
    let result = app.db.tx(|tx| -> rusqlite::Result<Option<Done>> {
        let Some(r) = load_row(tx, id)? else { return Ok(None) };
        if r.deleted || r.vm.audio_status == AudioStatus::Ready {
            return Ok(None);
        }
        let empty = r.recorded && transcript.is_none() && duration_ms.is_none_or(|d| d < MIN_MESSAGE_MS);
        if r.recorded && (empty || (s3_key.is_none() && transcript.is_none())) {
            // A hang-up during the greeting leaves nothing worth delivering.
            tx.execute("UPDATE voicemails SET deleted_at = ?2, audio_status = 'failed', transcript_status = 'failed' WHERE voicemail_id = ?1", params![id, now_ms()])?;
            tx.execute("UPDATE calls SET voicemail_id = NULL WHERE ringid = ?1 AND voicemail_id = ?2", params![r.vm.ringid, id])?;
            return Ok(Some((None, false, r.vm.ringid, None)));
        }
        let audio_status = if s3_key.is_some() { "ready" } else { "failed" };
        let transcript_status = match (r.recorded, &transcript) {
            (false, _) => "ready",
            (true, Some(_)) => "ready",
            (true, None) => "failed",
        };
        tx.execute(
            "UPDATE voicemails SET audio_status = ?2, audio_key = COALESCE(?3, audio_key), duration_ms = COALESCE(?4, duration_ms),
               transcript = CASE WHEN text IS NULL THEN ?5 ELSE transcript END, transcript_status = ?6 WHERE voicemail_id = ?1",
            params![id, audio_status, s3_key, duration_ms.map(|d| d as i64), transcript, transcript_status],
        )?;
        let entry = if r.recorded {
            let from = r.vm.from.clone();
            Some(crate::transcript::append(tx, &r.vm.ringid, EntryKind::Voicemail, Some(&from), None, json!({"voicemail_id": id}), None)?)
        } else {
            None
        };
        let first = !r.delivered;
        let queued = if first { deliver_tx(tx, id)? } else { false };
        Ok(Some((Some(first), queued, r.vm.ringid, entry)))
    });
    if let Some(error) = &error {
        tracing::warn!(
            voicemail_id = id,
            error = error.as_str(),
            "ring-media reported a voicemail problem"
        );
    }
    match result {
        Ok(Some((received, queued, ringid, entry))) => {
            if queued {
                app.ting.wake();
            }
            if let Some(entry) = entry {
                let mut fx = crate::calls::Effects::default();
                fx.entry(entry, None);
                fx.apply(app, &ringid).await;
            }
            if let Some(first) = received {
                announce(app, id, first);
            }
            let Ok(Some(data)) = app.db.with(|c| crate::calls::load(c, &ringid)) else {
                return;
            };
            if data.call.state == ring_proto::CallState::Voicemail {
                // ring-media stopped on its own (time limit reached): the message is complete.
                crate::calls::engine::finish_voicemail(app, &ringid).await;
            } else {
                crate::calls::broadcast_call(
                    app,
                    &data,
                    data.call.state == ring_proto::CallState::Ended,
                );
            }
        }
        Ok(None) => {}
        Err(e) => tracing::error!(error = %e, "could not store finished voicemail"),
    }
}

/// After a restart, asks ring-media again for text voicemails whose audio never arrived.
pub fn resume_synthesis(app: &AppState) {
    let pending: Vec<(String, String, String)> = app
        .db
        .with(|c| {
            let mut stmt = c.prepare(
                "SELECT v.voicemail_id, v.text, COALESCE(s.voice, ?1) FROM voicemails v LEFT JOIN settings s ON s.account_uuid = v.from_uuid
                 WHERE v.text IS NOT NULL AND v.audio_status = 'processing' AND v.deleted_at IS NULL",
            )?;
            let rows = stmt.query_map([ring_proto::DEFAULT_VOICE], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            rows.collect()
        })
        .unwrap_or_default();
    for (voicemail_id, text, voice) in pending {
        let s3_key = app.config.s3_key("voicemail", &voicemail_id, "wav");
        app.media
            .send(ring_proto::internal::ToMedia::VoicemailSynthesize {
                voicemail_id,
                text,
                voice,
                s3_key,
            });
    }
}

// ---- HTTP -------------------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct ListQuery {
    unread: Option<bool>,
    from: Option<String>,
    since: Option<String>,
    until: Option<String>,
    limit: Option<i64>,
    cursor: Option<String>,
}

fn parse_time(field: &str, value: Option<&str>) -> ApiResult<Option<i64>> {
    match value.filter(|v| !v.is_empty()) {
        None => Ok(None),
        Some(v) => Time::parse(v)
            .map(|t| Some(t.millis()))
            .ok_or_else(|| ApiError::invalid(format!("{field} must be an RFC 3339 timestamp."))),
    }
}

pub async fn list(
    State(app): State<AppState>,
    auth: Auth,
    ApiQuery(q): ApiQuery<ListQuery>,
) -> ApiResult<Json<Page<Voicemail>>> {
    let limit = q.limit.unwrap_or(50).clamp(1, 200);
    let from = match q.from.as_deref().filter(|f| !f.is_empty()) {
        Some(f) => Some(resolve_uuid(&app, f).await?),
        None => None,
    };
    let since = parse_time("since", q.since.as_deref())?;
    let until = parse_time("until", q.until.as_deref())?;
    let cursor = decode_cursor(q.cursor.as_deref())?;
    let mut sql = format!(
        "SELECT {COLUMNS} FROM voicemails WHERE to_uuid = ? AND deleted_at IS NULL AND delivered = 1"
    );
    let mut args: Vec<rusqlite::types::Value> = vec![auth.uuid().to_string().into()];
    if q.unread == Some(true) {
        sql.push_str(" AND read = 0");
    }
    if let Some(from) = from {
        sql.push_str(" AND from_uuid = ?");
        args.push(from.into());
    }
    if let Some(since) = since {
        sql.push_str(" AND created_at >= ?");
        args.push(since.into());
    }
    if let Some(until) = until {
        sql.push_str(" AND created_at < ?");
        args.push(until.into());
    }
    if let Some((at, id)) = cursor {
        sql.push_str(" AND (created_at < ? OR (created_at = ? AND voicemail_id < ?))");
        args.push(at.into());
        args.push(at.into());
        args.push(id.into());
    }
    sql.push_str(" ORDER BY created_at DESC, voicemail_id DESC LIMIT ?");
    args.push((limit + 1).into());
    let (items, next) = app
        .db
        .with(|c| -> rusqlite::Result<(Vec<Voicemail>, Option<String>)> {
            let mut stmt = c.prepare(&sql)?;
            let mut rows = stmt.query(params_from_iter(args.iter()))?;
            let mut items = Vec::new();
            while let Some(r) = rows.next()? {
                items.push(row(c, r)?.vm);
            }
            let next = if items.len() as i64 > limit {
                items.truncate(limit as usize);
                items
                    .last()
                    .map(|v| encode_cursor(v.created_at.millis(), &v.voicemail_id))
            } else {
                None
            };
            Ok((items, next))
        })?;
    Ok(Json(Page {
        items,
        next_cursor: next,
    }))
}

fn visible(auth: &Auth, r: &Row) -> bool {
    !r.deleted
        && (r.vm.to.uuid == auth.account.uuid && r.delivered || r.vm.from.uuid == auth.account.uuid)
}

pub async fn get(
    State(app): State<AppState>,
    auth: Auth,
    Path(id): Path<String>,
) -> ApiResult<Json<Voicemail>> {
    let r = app
        .db
        .with(|c| load_row(c, &id))?
        .filter(|r| visible(&auth, r))
        .ok_or_else(|| ApiError::not_found("Voicemail"))?;
    let mut vm = r.vm;
    if vm.audio_status == AudioStatus::Ready
        && let (Some(key), Some(s3)) = (&r.audio_key, &app.s3)
    {
        match s3.presign_get(key).await {
            Ok((url, _)) => vm.audio_url = Some(url),
            Err(error) => tracing::warn!(%error, "could not presign voicemail audio"),
        }
    }
    Ok(Json(vm))
}

pub async fn patch(
    State(app): State<AppState>,
    auth: Auth,
    Path(id): Path<String>,
    ApiJson(patch): ApiJson<VoicemailPatch>,
) -> ApiResult<Json<Voicemail>> {
    let r = app
        .db
        .with(|c| load_row(c, &id))?
        .filter(|r| !r.deleted && r.delivered && r.vm.to.uuid == auth.account.uuid);
    if r.is_none() {
        return Err(ApiError::not_found("Voicemail"));
    }
    app.db.with(|c| {
        c.execute(
            "UPDATE voicemails SET read = ?2 WHERE voicemail_id = ?1",
            params![id, patch.read as i64],
        )
    })?;
    announce(&app, &id, false);
    let vm = app
        .db
        .with(|c| load(c, &id))?
        .ok_or_else(|| ApiError::not_found("Voicemail"))?;
    Ok(Json(vm))
}

pub async fn delete(
    State(app): State<AppState>,
    auth: Auth,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let r = app
        .db
        .with(|c| load_row(c, &id))?
        .filter(|r| !r.deleted && r.delivered && r.vm.to.uuid == auth.account.uuid)
        .ok_or_else(|| ApiError::not_found("Voicemail"))?;
    app.db.with(|c| {
        c.execute(
            "UPDATE voicemails SET deleted_at = ?2 WHERE voicemail_id = ?1",
            params![id, now_ms()],
        )
    })?;
    if let (Some(key), Some(s3)) = (r.audio_key, app.s3.clone()) {
        tokio::spawn(async move {
            if let Err(error) = s3.delete(&key).await {
                tracing::warn!(%error, "could not delete voicemail audio");
            }
        });
    }
    Ok(StatusCode::NO_CONTENT)
}
