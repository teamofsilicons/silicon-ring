//! The loopback link to ring-media (`GET /internal/media`). Commands issued while ring-media is
//! away are queued; on its `hello` the server re-issues the state of every live call (ring-media
//! treats repeated starts as no-ops), then delivers the queue.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use futures_util::{SinkExt, StreamExt};
use ring_proto::internal::{FromMedia, ToMedia};
use ring_proto::{AccountKind, CallState, RepresentativeStatus};
use tokio::sync::mpsc;

use crate::app::AppState;
use crate::calls::{self, Effects};
use crate::error::ApiError;
use crate::extract::{Peer, is_direct_loopback};
use crate::util::secret_eq;

const MAX_QUEUE: usize = 5000;

#[derive(Default)]
pub struct MediaLink {
    state: Mutex<LinkState>,
    next_id: AtomicU64,
}

#[derive(Default)]
struct LinkState {
    tx: Option<(u64, mpsc::UnboundedSender<String>)>,
    queue: VecDeque<ToMedia>,
}

/// Commands that describe call state; they are rebuilt from the database on reconnect instead of
/// being queued.
fn is_state_command(command: &ToMedia) -> bool {
    matches!(
        command,
        ToMedia::RepresentativeStart { .. }
            | ToMedia::RecorderStart { .. }
            | ToMedia::VoicemailStart { .. }
            | ToMedia::RosterChanged { .. }
            | ToMedia::AuthorityChanged { .. }
    )
}

impl MediaLink {
    pub fn send(&self, command: ToMedia) {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if let Some((_, tx)) = &state.tx
            && let Ok(text) = serde_json::to_string(&command)
            && tx.send(text).is_ok()
        {
            return;
        }
        if is_state_command(&command) {
            return;
        }
        if state.queue.len() >= MAX_QUEUE {
            state.queue.pop_front();
        }
        state.queue.push_back(command);
    }

    pub fn is_connected(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .tx
            .as_ref()
            .is_some_and(|(_, tx)| !tx.is_closed())
    }

    fn attach(&self, id: u64, tx: mpsc::UnboundedSender<String>) -> Vec<ToMedia> {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        state.tx = Some((id, tx));
        state.queue.drain(..).collect()
    }

    fn detach(&self, id: u64) {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if state.tx.as_ref().is_some_and(|(current, _)| *current == id) {
            state.tx = None;
        }
    }
}

pub async fn internal_media(
    State(app): State<AppState>,
    Peer(peer): Peer,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Response {
    if !is_direct_loopback(&headers, peer) {
        return ApiError::not_found("Route").into_response();
    }
    let presented = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");
    if !secret_eq(presented, &app.config.media_secret) {
        return ApiError::unauthenticated().into_response();
    }
    upgrade
        .max_message_size(4 * 1024 * 1024)
        .on_upgrade(move |socket| serve(app, socket))
}

async fn serve(app: AppState, socket: WebSocket) {
    let (mut sink, mut stream) = socket.split();
    let hello = match tokio::time::timeout(Duration::from_secs(15), stream.next()).await {
        Ok(Some(Ok(Message::Text(text)))) => serde_json::from_str::<FromMedia>(text.as_str()).ok(),
        _ => None,
    };
    let Some(FromMedia::Hello {
        version,
        active_rings,
    }) = hello
    else {
        tracing::warn!("ring-media connected without a hello");
        return;
    };
    tracing::info!(
        version = version.as_str(),
        active = active_rings.len(),
        "ring-media connected"
    );

    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let id = app.media.next_id.fetch_add(1, Ordering::Relaxed);
    let queued = app.media.attach(id, tx.clone());
    for command in reissue(&app, &active_rings).into_iter().chain(queued) {
        if let Ok(text) = serde_json::to_string(&command) {
            let _ = tx.send(text);
        }
    }
    drop(tx);

    let writer = tokio::spawn(async move {
        let mut ping = tokio::time::interval(Duration::from_secs(20));
        loop {
            tokio::select! {
                text = rx.recv() => match text {
                    Some(text) => if sink.send(Message::Text(text.into())).await.is_err() { break },
                    None => break,
                },
                _ = ping.tick() => if sink.send(Message::Ping(Default::default())).await.is_err() { break },
            }
        }
    });

    while let Some(message) = stream.next().await {
        match message {
            Ok(Message::Text(text)) => match serde_json::from_str::<FromMedia>(text.as_str()) {
                Ok(message) => handle(&app, message).await,
                Err(error) => tracing::warn!(%error, "unreadable message from ring-media"),
            },
            Ok(Message::Close(_)) | Err(_) => break,
            Ok(_) => {}
        }
    }
    app.media.detach(id);
    writer.abort();
    tracing::warn!("ring-media disconnected");
}

/// Commands that bring a (re)connected ring-media up to date with every live call.
fn reissue(app: &AppState, active_rings: &[String]) -> Vec<ToMedia> {
    let mut out = Vec::new();
    let live: Vec<String> = app
        .db
        .with(|c| {
            let mut stmt =
                c.prepare("SELECT ringid FROM calls WHERE state IN ('active', 'voicemail')")?;
            let rows = stmt.query_map([], |r| r.get(0))?;
            rows.collect()
        })
        .unwrap_or_default();
    for ringid in active_rings {
        if !live.contains(ringid) {
            out.push(ToMedia::CallClosed {
                ringid: ringid.clone(),
            });
        }
    }
    for ringid in live {
        let Ok(Some(data)) = app.db.with(|c| calls::load(c, &ringid)) else {
            continue;
        };
        let roster = data.roster();
        match data.call.state {
            CallState::Active => {
                if data.call.recording_status == ring_proto::RecordingStatus::Recording
                    && let (Some(key), Ok(token)) = (
                        data.call.recording_key.clone(),
                        app.livekit.recorder_token(&ringid),
                    )
                {
                    out.push(ToMedia::RecorderStart {
                        ringid: ringid.clone(),
                        token,
                        roster: roster.clone(),
                        s3_key: key,
                    });
                }
                for p in data.joined() {
                    let account = data.account(&p.account_uuid);
                    if account.kind == AccountKind::Silicon {
                        if !matches!(
                            p.rep_status,
                            Some(RepresentativeStatus::Starting | RepresentativeStatus::Live)
                        ) {
                            continue;
                        }
                        let Ok(token) = app.livekit.representative_token(&ringid, &account) else {
                            continue;
                        };
                        out.push(ToMedia::RepresentativeStart {
                            ringid: ringid.clone(),
                            silicon: account,
                            voice: p
                                .rep_voice
                                .clone()
                                .unwrap_or_else(|| ring_proto::DEFAULT_VOICE.into()),
                            context: p.rep_context.clone().unwrap_or_default(),
                            start: None,
                            roster: roster.clone(),
                            token,
                        });
                    } else if let Some(identity) = &p.media_identity {
                        out.push(ToMedia::AuthorityChanged {
                            ringid: ringid.clone(),
                            account_uuid: account.uuid.clone(),
                            identity: identity.clone(),
                        });
                    }
                }
                out.push(ToMedia::RosterChanged {
                    ringid: ringid.clone(),
                    roster,
                    joined: None,
                    left: None,
                });
            }
            CallState::Voicemail => {
                let Some(voicemail_id) = data.call.voicemail_id.clone() else {
                    continue;
                };
                if active_rings.contains(&ringid) {
                    continue;
                }
                let recording: Option<String> = app
                    .db
                    .with(|c| {
                        use rusqlite::OptionalExtension;
                        c.query_row(
                            "SELECT audio_status FROM voicemails WHERE voicemail_id = ?1",
                            [&voicemail_id],
                            |r| r.get(0),
                        )
                        .optional()
                    })
                    .ok()
                    .flatten();
                if recording.as_deref() != Some("recording") {
                    continue;
                }
                let caller = data.call.created_by.clone();
                let callee = data
                    .invitations
                    .first()
                    .map(|i| i.to_uuid.clone())
                    .unwrap_or_default();
                let Ok(settings) = app.db.with(|c| crate::settings::load(c, &callee)) else {
                    continue;
                };
                let reason = data
                    .call
                    .voicemail_reason
                    .as_deref()
                    .and_then(calls::enum_from)
                    .unwrap_or(ring_proto::VoicemailReason::Missed);
                let Ok(token) = app.livekit.voicemail_token(&ringid) else {
                    continue;
                };
                out.push(ToMedia::VoicemailStart {
                    ringid: ringid.clone(),
                    voicemail_id: voicemail_id.clone(),
                    caller_identity: data
                        .participant(&caller)
                        .and_then(|p| p.media_identity.clone())
                        .unwrap_or_default(),
                    greeting_text: crate::settings::greeting_text(
                        &settings.settings,
                        &data.account(&callee),
                        reason,
                    ),
                    voice: settings.settings.voice,
                    max_seconds: ring_proto::VOICEMAIL_MAX_SECONDS,
                    token,
                    s3_key: app.config.s3_key("voicemail", &voicemail_id, "wav"),
                });
            }
            _ => {}
        }
    }
    out
}

async fn handle(app: &AppState, message: FromMedia) {
    match message {
        FromMedia::Hello { .. } => {}
        FromMedia::SpeechPartial {
            ringid,
            account_uuid,
            segment_id,
            text,
        } => {
            crate::transcript::speech_partial(app, &ringid, &account_uuid, &segment_id, &text);
        }
        FromMedia::SpeechFinal {
            ringid,
            account_uuid,
            segment_id,
            text,
            ..
        } => {
            crate::transcript::speech_final(app, &ringid, &account_uuid, &segment_id, &text).await;
        }
        FromMedia::DelegationCreated {
            ringid,
            silicon_uuid,
            delegation_id,
            request,
        } => {
            crate::transcript::delegation_created(
                app,
                &ringid,
                &silicon_uuid,
                &delegation_id,
                request,
            )
            .await;
        }
        FromMedia::RepresentativeStatus {
            ringid,
            silicon_uuid,
            status,
            error,
        } => {
            calls::engine::representative_status(app, &ringid, &silicon_uuid, status, error).await;
        }
        FromMedia::MessageDelivered { message_id, .. } => {
            crate::transcript::message_delivered(app, &message_id)
        }
        FromMedia::RecordingFinished {
            ringid,
            s3_key,
            duration_ms,
            error,
        } => recording_finished(app, &ringid, s3_key, duration_ms, error).await,
        FromMedia::VoicemailFinished {
            voicemail_id,
            s3_key,
            duration_ms,
            transcript,
            error,
        } => {
            crate::voicemail::finished(app, &voicemail_id, s3_key, duration_ms, transcript, error)
                .await;
        }
    }
}

async fn recording_finished(
    app: &AppState,
    ringid: &str,
    s3_key: Option<String>,
    duration_ms: Option<u64>,
    error: Option<String>,
) {
    let _guard = app.call_locks.lock(ringid).await;
    let ready = s3_key.is_some() && error.is_none();
    if let Some(error) = &error {
        tracing::warn!(ringid, error = error.as_str(), "recording failed");
    }
    let mut fx = Effects::default();
    let result = app.db.tx(|tx| -> rusqlite::Result<()> {
        let Some(data) = calls::load(tx, ringid)? else { return Ok(()) };
        tx.execute(
            "UPDATE calls SET recording_status = ?2, recording_key = COALESCE(?3, recording_key), recording_duration_ms = ?4 WHERE ringid = ?1",
            rusqlite::params![ringid, if ready { "ready" } else { "failed" }, s3_key, duration_ms.map(|d| d as i64)],
        )?;
        let status = if ready { "ready" } else { "failed" };
        for p in &data.participants {
            if data.is_silicon(&p.account_uuid) {
                let recipient = data.account(&p.account_uuid);
                let payload = serde_json::json!({"ringid": ringid, "status": status});
                fx.ting |= crate::ting::enqueue(tx, &recipient, "ring.recording.updated", &format!("recording:{ringid}:{}", recipient.uuid), payload, crate::ting::PRIORITY_NORMAL)?;
            }
        }
        fx.call_updated = true;
        Ok(())
    });
    if let Err(error) = result {
        tracing::error!(%error, "could not store recording result");
        return;
    }
    drop(_guard);
    fx.apply(app, ringid).await;
}
