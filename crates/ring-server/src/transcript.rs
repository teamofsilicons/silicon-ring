//! Transcript entries, delegations and representative messages, plus the 10 s Ting batches.

use std::collections::HashMap;
use std::time::Duration;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use ring_proto::internal::ToMedia;
use ring_proto::{
    Account, CallState, Delegation, DelegationReply, DelegationStatus, EntryKind, Event, Items,
    MessageKind, MessageStatus, RepresentativeStatus, SendMessage, SentMessage, TranscriptEntry,
    TranscriptPage, limits,
};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::accounts;
use crate::app::AppState;
use crate::calls::{self, CallData, Effects, enum_from, enum_str};
use crate::error::{ApiError, ApiResult};
use crate::extract::{ApiJson, ApiQuery, Auth};
use crate::ting;
use crate::util::{check_len, new_id, now_ms, time};

/// Delegations carry the transcript of the last 30 seconds as context.
const DELEGATION_CONTEXT_MS: i64 = 30_000;
/// Ting bodies must stay below 256 KiB; batches are split well below that.
const BATCH_LIMIT_BYTES: usize = 190 * 1024;

/// Appends one entry with the call's next sequence number. Must run inside a transaction.
pub fn append(
    conn: &Connection,
    ringid: &str,
    kind: EntryKind,
    actor: Option<&Account>,
    text: Option<&str>,
    data: Value,
    owner: Option<&str>,
) -> rusqlite::Result<TranscriptEntry> {
    let (seq, answered_at): (i64, Option<i64>) = conn.query_row(
        "SELECT next_seq, answered_at FROM calls WHERE ringid = ?1",
        [ringid],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let at = now_ms();
    let offset_ms = answered_at.map(|a| (at - a).max(0));
    conn.execute(
        "UPDATE calls SET next_seq = ?2 WHERE ringid = ?1",
        params![ringid, seq + 1],
    )?;
    conn.execute(
        "INSERT INTO transcript_entries (ringid, seq, at, offset_ms, kind, actor_uuid, text, data, owner_uuid)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![ringid, seq, at, offset_ms, kind.as_str(), actor.map(|a| a.uuid.as_str()), text, data.to_string(), owner],
    )?;
    Ok(TranscriptEntry {
        seq: seq as u64,
        at: time(at),
        offset_ms,
        kind,
        actor: actor.cloned(),
        text: text.map(String::from),
        data,
    })
}

/// Entries visible to `viewer` after `after_seq`.
pub fn entries(
    conn: &Connection,
    ringid: &str,
    viewer: &str,
    after_seq: i64,
    limit: i64,
) -> rusqlite::Result<Vec<TranscriptEntry>> {
    entries_where(conn, ringid, viewer, after_seq, i64::MAX, limit, true)
}

fn entries_where(
    conn: &Connection,
    ringid: &str,
    viewer: &str,
    after_seq: i64,
    since_at: i64,
    limit: i64,
    with_words: bool,
) -> rusqlite::Result<Vec<TranscriptEntry>> {
    let mut stmt = conn.prepare(
        "SELECT seq, at, offset_ms, kind, actor_uuid, text, data FROM transcript_entries
         WHERE ringid = ?1 AND seq > ?2 AND (owner_uuid IS NULL OR owner_uuid = ?3) AND (?4 = 9223372036854775807 OR at >= ?4)
           AND (?6 OR kind NOT IN ('speech', 'note'))
         ORDER BY seq LIMIT ?5",
    )?;
    let rows = stmt
        .query_map(
            params![ringid, after_seq, viewer, since_at, limit, with_words],
            |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, Option<i64>>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, Option<String>>(5)?,
                    r.get::<_, String>(6)?,
                ))
            },
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut cache: HashMap<String, Account> = HashMap::new();
    let mut out = Vec::with_capacity(rows.len());
    for (seq, at, offset_ms, kind, actor, text, data) in rows {
        let actor = match actor {
            Some(uuid) => {
                if !cache.contains_key(&uuid) {
                    let account =
                        accounts::get(conn, &uuid)?.unwrap_or_else(|| accounts::placeholder(&uuid));
                    cache.insert(uuid.clone(), account);
                }
                cache.get(&uuid).cloned()
            }
            None => None,
        };
        out.push(TranscriptEntry {
            seq: seq as u64,
            at: time(at),
            offset_ms,
            kind: enum_from(&kind).unwrap_or(EntryKind::Note),
            actor,
            text,
            data: serde_json::from_str(&data).unwrap_or_else(|_| json!({})),
        });
    }
    Ok(out)
}

/// Queues `ring.transcript.batch` tings for each silicon in `silicons` with the entries they have
/// not been sent yet. Must run inside a transaction.
pub fn flush_batches(
    conn: &Connection,
    data: &CallData,
    silicons: &[String],
) -> rusqlite::Result<bool> {
    let mut queued = false;
    for uuid in silicons {
        let Some(p) = data.participant(uuid) else {
            continue;
        };
        let pending = entries(conn, &data.call.ringid, uuid, p.ting_seq, 2000)?;
        // Entries the silicon may not see still advance its cursor.
        let newest: i64 = conn.query_row(
            "SELECT next_seq - 1 FROM calls WHERE ringid = ?1",
            [&data.call.ringid],
            |r| r.get(0),
        )?;
        let last = pending.last().map(|e| e.seq as i64).unwrap_or(newest);
        if last <= p.ting_seq {
            continue;
        }
        let recipient = data.account(uuid);
        let mut batch: Vec<TranscriptEntry> = Vec::new();
        let mut size = 0usize;
        let mut send = |batch: &mut Vec<TranscriptEntry>| -> rusqlite::Result<()> {
            if batch.is_empty() {
                return Ok(());
            }
            let from_seq = batch.first().map(|e| e.seq).unwrap_or_default();
            let to_seq = batch.last().map(|e| e.seq).unwrap_or_default();
            let key = format!(
                "transcript:{}:{}:{}-{}",
                data.call.ringid, uuid, from_seq, to_seq
            );
            let payload = json!({"ringid": data.call.ringid, "from_seq": from_seq, "to_seq": to_seq, "entries": batch});
            queued |= ting::enqueue(
                conn,
                &recipient,
                "ring.transcript.batch",
                &key,
                payload,
                ting::PRIORITY_BATCH,
            )?;
            batch.clear();
            Ok(())
        };
        for entry in pending {
            let entry_size = serde_json::to_vec(&entry).map(|v| v.len()).unwrap_or(0);
            if size + entry_size > BATCH_LIMIT_BYTES {
                send(&mut batch)?;
                size = 0;
            }
            size += entry_size;
            batch.push(entry);
        }
        send(&mut batch)?;
        conn.execute(
            "UPDATE participants SET ting_seq = ?3 WHERE ringid = ?1 AND account_uuid = ?2",
            params![data.call.ringid, uuid, last],
        )?;
    }
    Ok(queued)
}

/// Every 10 seconds, silicons on live calls get the transcript entries since their last batch.
pub async fn batch_worker(app: AppState) {
    let mut tick = tokio::time::interval(Duration::from_secs(10));
    loop {
        tick.tick().await;
        let pending: Vec<(String, String)> = app
            .db
            .with(|c| {
                let mut stmt = c.prepare(
                    "SELECT p.ringid, p.account_uuid FROM participants p JOIN calls c ON c.ringid = p.ringid
                     JOIN accounts a ON a.uuid = p.account_uuid
                     WHERE c.state != 'ended' AND p.state = 'joined' AND a.kind = 'silicon' AND c.next_seq - 1 > p.ting_seq",
                )?;
                let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
                rows.collect()
            })
            .unwrap_or_default();
        let mut queued = false;
        for (ringid, uuid) in pending {
            let _guard = app.call_locks.lock(&ringid).await;
            let result = app.db.tx(|tx| -> rusqlite::Result<bool> {
                let Some(data) = calls::load(tx, &ringid)? else {
                    return Ok(false);
                };
                flush_batches(tx, &data, std::slice::from_ref(&uuid))
            });
            match result {
                Ok(q) => queued |= q,
                Err(error) => tracing::error!(%error, "transcript batch failed"),
            }
        }
        if queued {
            app.ting.wake();
        }
    }
}

// ---- inbound from ring-media ------------------------------------------------------------------

pub async fn speech_final(
    app: &AppState,
    ringid: &str,
    account_uuid: &str,
    segment_id: &str,
    text: &str,
) {
    let text = text.trim();
    if text.is_empty() {
        return;
    }
    let _guard = app.call_locks.lock(ringid).await;
    let mut fx = Effects::default();
    let result = app.db.tx(|tx| -> rusqlite::Result<()> {
        let Some(data) = calls::load(tx, ringid)? else {
            return Ok(());
        };
        if data.call.state == CallState::Ended || data.participant(account_uuid).is_none() {
            return Ok(());
        }
        let actor = data.account(account_uuid);
        let entry = append(
            tx,
            ringid,
            EntryKind::Speech,
            Some(&actor),
            Some(text),
            json!({"segment_id": segment_id}),
            None,
        )?;
        fx.entry(entry, None);
        Ok(())
    });
    if let Err(error) = result {
        tracing::error!(%error, "could not store speech");
        return;
    }
    fx.apply(app, ringid).await;
}

pub fn speech_partial(
    app: &AppState,
    ringid: &str,
    account_uuid: &str,
    segment_id: &str,
    text: &str,
) {
    let Ok(Some(data)) = app.db.with(|c| calls::load(c, ringid)) else {
        return;
    };
    if data.call.state == CallState::Ended {
        return;
    }
    let event = Event::TranscriptPartial {
        ringid: ringid.to_string(),
        speaker: data.account(account_uuid),
        segment_id: segment_id.to_string(),
        text: text.to_string(),
    };
    let frame = crate::realtime::frame(&event);
    for uuid in data.joined().map(|p| p.account_uuid.clone()) {
        app.hub.send_frame(&uuid, frame.clone());
    }
}

pub async fn delegation_created(
    app: &AppState,
    ringid: &str,
    silicon_uuid: &str,
    delegation_id: &str,
    request: Option<String>,
) {
    let _guard = app.call_locks.lock(ringid).await;
    let mut fx = Effects::default();
    let request = request
        .map(|r| r.trim().to_string())
        .filter(|r| !r.is_empty());
    let result = app.db.tx(|tx| -> rusqlite::Result<Option<Delegation>> {
        let Some(data) = calls::load(tx, ringid)? else { return Ok(None) };
        let joined = data.participant(silicon_uuid).is_some_and(|p| p.joined());
        if data.call.state == CallState::Ended || !joined || !data.is_silicon(silicon_uuid) {
            return Ok(None);
        }
        let exists: Option<String> =
            tx.query_row("SELECT delegation_id FROM delegations WHERE delegation_id = ?1", [delegation_id], |r| r.get(0)).optional()?;
        if exists.is_some() {
            return Ok(None);
        }
        let context = entries_where(tx, ringid, silicon_uuid, 0, now_ms() - DELEGATION_CONTEXT_MS, 500, true)?;
        let now = now_ms();
        tx.execute(
            "INSERT INTO delegations (delegation_id, ringid, silicon_uuid, created_at, status, request, context) VALUES (?1, ?2, ?3, ?4, 'open', ?5, ?6)",
            params![delegation_id, ringid, silicon_uuid, now, request, serde_json::to_string(&context).unwrap_or_else(|_| "[]".into())],
        )?;
        let silicon = data.account(silicon_uuid);
        let entry = append(
            tx,
            ringid,
            EntryKind::Delegation,
            Some(&silicon),
            request.as_deref(),
            json!({"delegation_id": delegation_id, "request": request}),
            Some(silicon_uuid),
        )?;
        fx.entry(entry, Some(silicon_uuid.to_string()));
        let delegation = Delegation {
            delegation_id: delegation_id.to_string(),
            ringid: ringid.to_string(),
            silicon: silicon.clone(),
            created_at: time(now),
            status: DelegationStatus::Open,
            request: request.clone(),
            context: context.clone(),
            replies: vec![],
        };
        let payload = json!({"ringid": ringid, "delegation_id": delegation_id, "request": request, "context": context});
        fx.ting |= ting::enqueue(tx, &silicon, "ring.delegation.created", &format!("delegation:{delegation_id}"), payload, ting::PRIORITY_DELEGATION)?;
        Ok(Some(delegation))
    });
    match result {
        Ok(Some(delegation)) => {
            fx.events.push((
                silicon_uuid.to_string(),
                Event::DelegationCreated { delegation },
            ));
            fx.apply(app, ringid).await;
        }
        Ok(None) => {}
        Err(error) => tracing::error!(%error, "could not store delegation"),
    }
}

pub fn message_delivered(app: &AppState, message_id: &str) {
    let result = app.db.with(|c| {
        c.execute(
            "UPDATE messages SET status = 'delivered', delivered_at = ?2 WHERE message_id = ?1 AND status != 'delivered'",
            params![message_id, now_ms()],
        )
    });
    if let Err(error) = result {
        tracing::error!(%error, "could not mark message delivered");
    }
}

// ---- HTTP -------------------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct TranscriptQuery {
    after_seq: Option<i64>,
    limit: Option<i64>,
}

pub async fn get_transcript(
    State(app): State<AppState>,
    auth: Auth,
    Path(ringid): Path<String>,
    ApiQuery(q): ApiQuery<TranscriptQuery>,
) -> ApiResult<Json<TranscriptPage>> {
    let after = q.after_seq.unwrap_or(0).max(0);
    let limit = q.limit.unwrap_or(200).clamp(1, 1000);
    let page = app
        .db
        .with(|c| -> rusqlite::Result<Option<TranscriptPage>> {
            let Some(data) = calls::load(c, &ringid)? else {
                return Ok(None);
            };
            if !data.can_view(auth.uuid()) {
                return Ok(Some(TranscriptPage {
                    items: vec![],
                    next_after_seq: u64::MAX,
                }));
            }
            // An invitee who never joined sees what happened to the call, not what was said.
            let joined_ever = data.participant(auth.uuid()).is_some();
            let items =
                entries_where(c, &ringid, auth.uuid(), after, i64::MAX, limit, joined_ever)?;
            let next_after_seq = items.last().map(|e| e.seq).unwrap_or(after as u64);
            Ok(Some(TranscriptPage {
                items,
                next_after_seq,
            }))
        })?;
    match page {
        None => Err(ApiError::not_found("Call")),
        Some(p) if p.next_after_seq == u64::MAX => Err(ApiError::not_participant()),
        Some(p) => Ok(Json(p)),
    }
}

fn render_delegation(
    conn: &Connection,
    delegation_id: &str,
) -> rusqlite::Result<Option<Delegation>> {
    let row = conn
        .query_row(
            "SELECT ringid, silicon_uuid, created_at, status, request, context FROM delegations WHERE delegation_id = ?1",
            [delegation_id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, String>(5)?,
                ))
            },
        )
        .optional()?;
    let Some((ringid, silicon_uuid, created_at, status, request, context)) = row else {
        return Ok(None);
    };
    let mut stmt = conn.prepare(
        "SELECT kind, text, at FROM delegation_replies WHERE delegation_id = ?1 ORDER BY id",
    )?;
    let replies = stmt
        .query_map([delegation_id], |r| {
            let kind: String = r.get(0)?;
            Ok(DelegationReply {
                kind: enum_from(&kind).unwrap_or(MessageKind::Thinking),
                text: r.get(1)?,
                at: time(r.get(2)?),
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(Some(Delegation {
        delegation_id: delegation_id.to_string(),
        ringid,
        silicon: accounts::get(conn, &silicon_uuid)?
            .unwrap_or_else(|| accounts::placeholder(&silicon_uuid)),
        created_at: time(created_at),
        status: if status == "closed" {
            DelegationStatus::Closed
        } else {
            DelegationStatus::Open
        },
        request,
        context: serde_json::from_str(&context).unwrap_or_default(),
        replies,
    }))
}

#[derive(Deserialize)]
pub struct DelegationQuery {
    status: Option<String>,
}

pub async fn list_delegations(
    State(app): State<AppState>,
    auth: Auth,
    Path(ringid): Path<String>,
    ApiQuery(q): ApiQuery<DelegationQuery>,
) -> ApiResult<Json<Items<Delegation>>> {
    let status = match q.status.as_deref() {
        None | Some("") => None,
        Some(s @ ("open" | "closed")) => Some(s.to_string()),
        Some(_) => return Err(ApiError::invalid("status must be open or closed.")),
    };
    let items = app.db.with(|c| -> rusqlite::Result<Option<Vec<Delegation>>> {
        let Some(data) = calls::load(c, &ringid)? else { return Ok(None) };
        if !data.can_view(auth.uuid()) {
            return Ok(Some(vec![]));
        }
        let mut stmt = c.prepare(
            "SELECT delegation_id FROM delegations WHERE ringid = ?1 AND silicon_uuid = ?2 AND (?3 IS NULL OR status = ?3) ORDER BY created_at",
        )?;
        let ids = stmt.query_map(params![ringid, auth.uuid(), status], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
        let mut out = Vec::new();
        for id in ids {
            if let Some(d) = render_delegation(c, &id)? {
                out.push(d);
            }
        }
        Ok(Some(out))
    })?;
    items
        .map(|items| Json(Items { items }))
        .ok_or_else(|| ApiError::not_found("Call"))
}

pub async fn get_delegation(
    State(app): State<AppState>,
    auth: Auth,
    Path(delegation_id): Path<String>,
) -> ApiResult<Json<Delegation>> {
    let delegation = app.db.with(|c| render_delegation(c, &delegation_id))?;
    match delegation {
        Some(d) if d.silicon.uuid == auth.account.uuid => Ok(Json(d)),
        _ => Err(ApiError::not_found("Delegation")),
    }
}

pub async fn send_message(
    State(app): State<AppState>,
    auth: Auth,
    Path(ringid): Path<String>,
    ApiJson(body): ApiJson<SendMessage>,
) -> ApiResult<(StatusCode, Json<SentMessage>)> {
    if !auth.is_silicon() {
        return Err(ApiError::forbidden("Only silicons feed a representative."));
    }
    app.limiter.check(
        &format!("messages:{}", auth.uuid()),
        120,
        Duration::from_secs(60),
    )?;
    let text = body.text.trim().to_string();
    if text.is_empty() {
        return Err(ApiError::invalid("text is required."));
    }
    check_len("text", &text, limits::MESSAGE)?;
    let delegation_id = body
        .delegation_id
        .map(|d| d.trim().to_string())
        .filter(|d| !d.is_empty());
    let uuid = auth.uuid().to_string();
    let _guard = app.call_locks.lock(&ringid).await;
    let mut fx = Effects::default();
    let message_id = new_id(ring_proto::prefix::MESSAGE);
    let connected = app.media.is_connected();
    let sent = app.db.tx(|tx| -> ApiResult<SentMessage> {
        let data = calls::load(tx, &ringid)?.ok_or_else(|| ApiError::not_found("Call"))?;
        if data.call.state == CallState::Ended {
            return Err(ApiError::call_ended());
        }
        let p = data.participant(&uuid).filter(|p| p.joined()).ok_or_else(ApiError::not_participant)?;
        if p.rep_status != Some(RepresentativeStatus::Live) {
            return Err(ApiError::conflict("Your representative on this call is not live.")
                .details(json!({"representative": p.rep_status.map(|s| enum_str(&s))})));
        }
        if let Some(id) = &delegation_id {
            let owner: Option<(String, String)> = tx
                .query_row("SELECT ringid, silicon_uuid FROM delegations WHERE delegation_id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?)))
                .optional()?;
            if owner.is_none_or(|(r, s)| r != ringid || s != uuid) {
                return Err(ApiError::not_found("Delegation"));
            }
        }
        let kind = match body.kind {
            MessageKind::Thinking => EntryKind::Thinking,
            MessageKind::Commentary => EntryKind::Commentary,
        };
        let actor = data.account(&uuid);
        let entry = append(tx, &ringid, kind, Some(&actor), Some(&text), json!({"delegation_id": delegation_id}), Some(&uuid))?;
        let now = now_ms();
        let status = if connected { MessageStatus::Delivered } else { MessageStatus::Queued };
        tx.execute(
            "INSERT INTO messages (message_id, ringid, silicon_uuid, kind, text, delegation_id, seq, status, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![message_id, ringid, uuid, body.kind.as_str(), text, delegation_id, entry.seq as i64, if connected { "sent" } else { "queued" }, now],
        )?;
        if let Some(id) = &delegation_id {
            tx.execute(
                "INSERT INTO delegation_replies (delegation_id, message_id, kind, text, at) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![id, message_id, body.kind.as_str(), text, now],
            )?;
        }
        let seq = entry.seq;
        fx.entry(entry, Some(uuid.clone()));
        fx.media.push(ToMedia::RepresentativeMessage {
            ringid: ringid.clone(),
            silicon_uuid: uuid.clone(),
            message_id: message_id.clone(),
            kind: body.kind,
            text: text.clone(),
            delegation_id: delegation_id.clone(),
        });
        Ok(SentMessage { message_id: message_id.clone(), seq, status })
    })?;
    drop(_guard);
    fx.apply(&app, &ringid).await;
    Ok((StatusCode::CREATED, Json(sent)))
}
