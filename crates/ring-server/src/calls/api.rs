//! HTTP handlers for `/v1/calls…` and `/v1/people/recent`.

use std::collections::HashMap;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use base64::Engine as _;
use ring_proto::{
    AcceptCall, Call, CallEnvelope, CallState, CallWithMedia, CreateCall, CreatedCall, DeclineCall,
    InviteToCall, InvitedCall, Items, LeaveVoicemail, Page, RecentPerson, Recording,
    RecordingStatus, SilenceCall, Time, Voicemail,
};
use rusqlite::params_from_iter;
use serde::Deserialize;

use super::{engine, load, render};
use crate::accounts;
use crate::app::AppState;
use crate::error::{ApiError, ApiResult};
use crate::extract::{ApiJson, ApiQuery, Auth, JsonOrEmpty};
use crate::util::time;

pub async fn create(
    State(app): State<AppState>,
    auth: Auth,
    ApiJson(req): ApiJson<CreateCall>,
) -> ApiResult<(StatusCode, Json<CreatedCall>)> {
    Ok((
        StatusCode::CREATED,
        Json(engine::create(&app, &auth, req).await?),
    ))
}

pub async fn accept(
    State(app): State<AppState>,
    auth: Auth,
    Path(ringid): Path<String>,
    JsonOrEmpty(req): JsonOrEmpty<AcceptCall>,
) -> ApiResult<Json<CallWithMedia>> {
    Ok(Json(engine::accept(&app, &auth, &ringid, req).await?))
}

pub async fn decline(
    State(app): State<AppState>,
    auth: Auth,
    Path(ringid): Path<String>,
    JsonOrEmpty(req): JsonOrEmpty<DeclineCall>,
) -> ApiResult<Json<CallEnvelope>> {
    Ok(Json(CallEnvelope {
        call: engine::decline(&app, &auth, &ringid, req).await?,
    }))
}

pub async fn silence(
    State(app): State<AppState>,
    auth: Auth,
    Path(ringid): Path<String>,
    JsonOrEmpty(req): JsonOrEmpty<SilenceCall>,
) -> ApiResult<Json<CallEnvelope>> {
    Ok(Json(CallEnvelope {
        call: engine::silence(&app, &auth, &ringid, req.invitation_id).await?,
    }))
}

pub async fn leave(
    State(app): State<AppState>,
    auth: Auth,
    Path(ringid): Path<String>,
) -> ApiResult<Json<CallEnvelope>> {
    Ok(Json(CallEnvelope {
        call: engine::leave(&app, &auth, &ringid).await?,
    }))
}

pub async fn invite(
    State(app): State<AppState>,
    auth: Auth,
    Path(ringid): Path<String>,
    ApiJson(req): ApiJson<InviteToCall>,
) -> ApiResult<(StatusCode, Json<InvitedCall>)> {
    Ok((
        StatusCode::CREATED,
        Json(engine::invite(&app, &auth, &ringid, req).await?),
    ))
}

pub async fn media(
    State(app): State<AppState>,
    auth: Auth,
    Path(ringid): Path<String>,
) -> ApiResult<Json<CallWithMedia>> {
    Ok(Json(engine::media(&app, &auth, &ringid).await?))
}

pub async fn leave_voicemail(
    State(app): State<AppState>,
    auth: Auth,
    Path(ringid): Path<String>,
    ApiJson(req): ApiJson<LeaveVoicemail>,
) -> ApiResult<(StatusCode, Json<Voicemail>)> {
    Ok((
        StatusCode::CREATED,
        Json(engine::leave_text_voicemail(&app, &auth, &ringid, req.text).await?),
    ))
}

pub async fn get(
    State(app): State<AppState>,
    auth: Auth,
    Path(ringid): Path<String>,
) -> ApiResult<Json<Call>> {
    let (data, call) = super::load_rendered(&app, &ringid, auth.uuid())?
        .ok_or_else(|| ApiError::not_found("Call"))?;
    if !data.can_view(auth.uuid()) {
        return Err(ApiError::not_participant());
    }
    Ok(Json(call))
}

pub async fn recording(
    State(app): State<AppState>,
    auth: Auth,
    Path(ringid): Path<String>,
) -> ApiResult<Json<Recording>> {
    let data = app
        .db
        .with(|c| load(c, &ringid))?
        .ok_or_else(|| ApiError::not_found("Call"))?;
    if !data.joined_ever(auth.uuid()) {
        return Err(ApiError::not_participant());
    }
    let mut recording = Recording {
        status: data.call.recording_status,
        duration_ms: data.call.recording_duration_ms.map(|d| d.max(0) as u64),
        url: None,
        expires_at: None,
        mime_type: Some("audio/wav".into()),
    };
    if recording.status == RecordingStatus::Ready
        && let Some(key) = &data.call.recording_key
    {
        let s3 = app
            .s3
            .as_ref()
            .ok_or_else(|| ApiError::dependency("Recording storage"))?;
        let (url, expires_at) = s3.presign_get(key).await.map_err(|error| {
            tracing::error!(%error, "could not presign a recording");
            ApiError::dependency("Recording storage")
        })?;
        recording.url = Some(url);
        recording.expires_at = Some(expires_at);
    }
    Ok(Json(recording))
}

#[derive(Deserialize)]
pub struct ListQuery {
    state: Option<String>,
    direction: Option<String>,
    with: Option<String>,
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

pub fn encode_cursor(at: i64, id: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(format!("{at}|{id}"))
}

pub fn decode_cursor(cursor: Option<&str>) -> ApiResult<Option<(i64, String)>> {
    let Some(cursor) = cursor.filter(|c| !c.is_empty()) else {
        return Ok(None);
    };
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(cursor)
        .ok()
        .and_then(|b| String::from_utf8(b).ok())
        .ok_or_else(|| ApiError::invalid("cursor is invalid."))?;
    let (at, id) = raw
        .split_once('|')
        .ok_or_else(|| ApiError::invalid("cursor is invalid."))?;
    Ok(Some((
        at.parse()
            .map_err(|_| ApiError::invalid("cursor is invalid."))?,
        id.to_string(),
    )))
}

/// Resolves an actor reference to a uuid, preferring the local cache.
pub async fn resolve_uuid(app: &AppState, reference: &str) -> ApiResult<String> {
    let normalized = ring_proto::normalize_actor_ref(reference)
        .ok_or_else(|| ApiError::invalid(format!("{reference:?} is not an account reference.")))?;
    if !(normalized.starts_with("c:") || normalized.starts_with("si:")) {
        return Ok(normalized);
    }
    if let Some(row) = app.db.with(|c| accounts::find_by_id(c, &normalized))? {
        return Ok(row.account.uuid);
    }
    Ok(accounts::resolve(app, &normalized).await?.uuid)
}

pub async fn list(
    State(app): State<AppState>,
    auth: Auth,
    ApiQuery(q): ApiQuery<ListQuery>,
) -> ApiResult<Json<Page<Call>>> {
    let states: Vec<String> = match q.state.as_deref().filter(|s| !s.is_empty()) {
        None => vec![],
        Some(list) => list
            .split(',')
            .map(|s| {
                CallState::parse(s.trim())
                    .map(|c| c.as_str().to_string())
                    .ok_or_else(|| ApiError::invalid(format!("Unknown call state {s:?}.")))
            })
            .collect::<ApiResult<_>>()?,
    };
    let direction = match q.direction.as_deref() {
        None | Some("") | Some("all") => None,
        Some(d @ ("incoming" | "outgoing")) => Some(d.to_string()),
        Some(_) => {
            return Err(ApiError::invalid(
                "direction must be incoming, outgoing or all.",
            ));
        }
    };
    let with = match q.with.as_deref().filter(|w| !w.is_empty()) {
        Some(w) => Some(resolve_uuid(&app, w).await?),
        None => None,
    };
    let since = parse_time("since", q.since.as_deref())?;
    let until = parse_time("until", q.until.as_deref())?;
    let limit = q.limit.unwrap_or(50).clamp(1, 200);
    let cursor = decode_cursor(q.cursor.as_deref())?;
    let viewer = auth.uuid().to_string();
    // Calls from before the viewer deleted a previous Ring account are not theirs.
    let mine_since = app
        .db
        .with(|c| accounts::ring_deleted_at(c, &viewer))?
        .unwrap_or(0);

    let mut sql = String::from(
        "SELECT ringid, created_at FROM calls c WHERE (c.ringid IN (SELECT ringid FROM participants WHERE account_uuid = ?1)
           OR c.ringid IN (SELECT ringid FROM invitations WHERE to_uuid = ?1 AND hidden = 0)) AND c.created_at >= ?2",
    );
    let mut args: Vec<rusqlite::types::Value> = vec![viewer.clone().into(), mine_since.into()];
    if !states.is_empty() {
        sql.push_str(&format!(
            " AND c.state IN ({})",
            vec!["?"; states.len()].join(",")
        ));
        args.extend(states.iter().cloned().map(Into::into));
    }
    match direction.as_deref() {
        Some("outgoing") => {
            sql.push_str(" AND c.created_by = ?1");
        }
        Some("incoming") => {
            sql.push_str(" AND c.created_by != ?1");
        }
        _ => {}
    }
    if let Some(with) = with {
        sql.push_str(" AND (c.ringid IN (SELECT ringid FROM participants WHERE account_uuid = ?) OR c.ringid IN (SELECT ringid FROM invitations WHERE to_uuid = ?))");
        args.push(with.clone().into());
        args.push(with.into());
    }
    if let Some(since) = since {
        sql.push_str(" AND c.created_at >= ?");
        args.push(since.into());
    }
    if let Some(until) = until {
        sql.push_str(" AND c.created_at < ?");
        args.push(until.into());
    }
    if let Some((at, id)) = &cursor {
        sql.push_str(" AND (c.created_at < ? OR (c.created_at = ? AND c.ringid < ?))");
        args.push((*at).into());
        args.push((*at).into());
        args.push(id.clone().into());
    }
    sql.push_str(" ORDER BY c.created_at DESC, c.ringid DESC LIMIT ?");
    args.push((limit + 1).into());

    let (items, next) = app
        .db
        .with(|c| -> rusqlite::Result<(Vec<Call>, Option<String>)> {
            let mut stmt = c.prepare(&sql)?;
            let mut rows: Vec<(String, i64)> = stmt
                .query_map(params_from_iter(args.iter()), |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })?
                .collect::<rusqlite::Result<_>>()?;
            let next = if rows.len() as i64 > limit {
                rows.truncate(limit as usize);
                rows.last().map(|(id, at)| encode_cursor(*at, id))
            } else {
                None
            };
            let mut items = Vec::with_capacity(rows.len());
            for (ringid, _) in rows {
                if let Some(data) = load(c, &ringid)? {
                    items.push(render(c, &data, &viewer)?);
                }
            }
            Ok((items, next))
        })?;
    Ok(Json(Page {
        items,
        next_cursor: next,
    }))
}

#[derive(Deserialize)]
pub struct RecentQuery {
    q: Option<String>,
    limit: Option<i64>,
}

pub async fn recent_people(
    State(app): State<AppState>,
    auth: Auth,
    ApiQuery(q): ApiQuery<RecentQuery>,
) -> ApiResult<Json<Items<RecentPerson>>> {
    let limit = q.limit.unwrap_or(50).clamp(1, 200) as usize;
    let needle =
        q.q.map(|s| s.trim().trim_start_matches('@').to_lowercase())
            .filter(|s| !s.is_empty());
    let viewer = auth.uuid().to_string();
    let items = app.db.with(|c| -> rusqlite::Result<Vec<RecentPerson>> {
        let mut stmt = c.prepare(
            "WITH mine AS (
               SELECT m.ringid FROM (
                 SELECT ringid FROM participants WHERE account_uuid = ?1
                 UNION SELECT ringid FROM invitations WHERE to_uuid = ?1 AND hidden = 0
               ) m JOIN calls c ON c.ringid = m.ringid WHERE c.created_at >= ?2
             ), others AS (
               SELECT p.ringid, p.account_uuid AS other FROM participants p JOIN mine m ON m.ringid = p.ringid
               UNION SELECT i.ringid, i.to_uuid FROM invitations i JOIN mine m ON m.ringid = i.ringid
               UNION SELECT i.ringid, i.from_uuid FROM invitations i JOIN mine m ON m.ringid = i.ringid
             )
             SELECT o.other, MAX(c.created_at), COUNT(DISTINCT c.ringid),
               (SELECT c2.ringid FROM calls c2 JOIN others o2 ON o2.ringid = c2.ringid WHERE o2.other = o.other ORDER BY c2.created_at DESC LIMIT 1)
             FROM others o JOIN calls c ON c.ringid = o.ringid
             WHERE o.other != ?1
             GROUP BY o.other ORDER BY MAX(c.created_at) DESC LIMIT 1000",
        )?;
        let mine_since = accounts::ring_deleted_at(c, &viewer)?.unwrap_or(0);
        let rows: Vec<(String, i64, i64, String)> =
            stmt.query_map(rusqlite::params![viewer, mine_since], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?.collect::<rusqlite::Result<_>>()?;
        let accounts: HashMap<String, ring_proto::Account> = accounts::load_many(c, rows.iter().map(|r| r.0.as_str()))?;
        let mut out = Vec::new();
        for (uuid, last, count, ringid) in rows {
            let account = accounts.get(&uuid).cloned().unwrap_or_else(|| accounts::placeholder(&uuid));
            if let Some(n) = &needle {
                let name = account.display_name.clone().unwrap_or_default().to_lowercase();
                if !account.id.contains(n.as_str()) && !name.contains(n.as_str()) {
                    continue;
                }
            }
            out.push(RecentPerson { account, last_call_at: time(last), last_ringid: ringid, calls: count as u64 });
            if out.len() >= limit {
                break;
            }
        }
        Ok(out)
    })?;
    Ok(Json(Items { items }))
}
