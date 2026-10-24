//! The call state machine. Every change takes the call's own lock, runs in one transaction, and
//! applies its side effects after commit.

use std::time::Duration;

use ring_proto::internal::ToMedia;
use ring_proto::{
    AcceptCall, AccountKind, Call, CallState, CallWithMedia, ContextAction, CreateCall,
    CreatedCall, DeclineCall, EndReason, EntryKind, Event, InvitationState, InviteToCall,
    InvitedCall, MediaGrant, Platform, RepresentativeStatus, RingingStoppedReason, VoicemailReason,
    limits,
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde_json::json;

use super::{
    CallData, Effects, InvitationRow, ParticipantRow, enum_str, insert_invitation, load,
    render_invitation, set_invitation_state,
};
use crate::accounts;
use crate::app::AppState;
use crate::error::{ApiError, ApiResult};
use crate::extract::Auth;
use crate::livekit;
use crate::settings;
use crate::ting;
use crate::transcript::{append, flush_batches};
use crate::util::{check_len, clean, new_id, now_ms};

pub const RINGING_MS: i64 = ring_proto::RINGING_SECONDS as i64 * 1000;
const MEDIA_GRACE: Duration = Duration::from_secs(30);
const JOIN_DEADLINE: Duration = Duration::from_secs(45);
const HANDOFF_REMOVE_AFTER: Duration = Duration::from_secs(2);
/// A silicon caller has this long to leave text after its call goes to voicemail.
const SILICON_VOICEMAIL_WINDOW: Duration = Duration::from_secs(180);
/// Safety net after a carbon's recording should have stopped on its own.
const CARBON_VOICEMAIL_SLACK: Duration = Duration::from_secs(60);

fn voicemail_reason_to_end(reason: VoicemailReason) -> EndReason {
    match reason {
        VoicemailReason::Busy => EndReason::Busy,
        VoicemailReason::Declined => EndReason::Declined,
        VoicemailReason::Missed => EndReason::Missed,
    }
}

fn new_invitation(ringid: &str, from: &str, to: &str, state: InvitationState) -> InvitationRow {
    let now = now_ms();
    InvitationRow {
        invitation_id: new_id(ring_proto::prefix::INVITATION),
        ringid: ringid.to_string(),
        from_uuid: from.to_string(),
        to_uuid: to.to_string(),
        state,
        reason: None,
        created_at: now,
        expires_at: now + RINGING_MS,
        answered_at: None,
        silenced: false,
        call_uuid: uuid::Uuid::new_v4().to_string(),
        hidden: false,
    }
}

// ---- silicon reservations ---------------------------------------------------------------------

/// Reserves a silicon for `ringid` (one call at a time). Stale reservations of ended calls are
/// cleared first.
fn reserve(conn: &Connection, uuid: &str, ringid: &str) -> rusqlite::Result<bool> {
    conn.execute(
        "DELETE FROM silicon_reservations WHERE account_uuid = ?1 AND ringid IN (SELECT ringid FROM calls WHERE state = 'ended')",
        [uuid],
    )?;
    let inserted =
        conn.execute("INSERT OR IGNORE INTO silicon_reservations (account_uuid, ringid, created_at) VALUES (?1, ?2, ?3)", params![uuid, ringid, now_ms()])?;
    if inserted == 1 {
        return Ok(true);
    }
    let current: Option<String> = conn
        .query_row(
            "SELECT ringid FROM silicon_reservations WHERE account_uuid = ?1",
            [uuid],
            |r| r.get(0),
        )
        .optional()?;
    Ok(current.as_deref() == Some(ringid))
}

fn reserved_elsewhere(conn: &Connection, uuid: &str, ringid: &str) -> rusqlite::Result<bool> {
    let current: Option<String> = conn
        .query_row(
            "SELECT r.ringid FROM silicon_reservations r JOIN calls c ON c.ringid = r.ringid WHERE r.account_uuid = ?1 AND c.state != 'ended'",
            [uuid],
            |r| r.get(0),
        )
        .optional()?;
    Ok(current.is_some_and(|r| r != ringid))
}

fn release(conn: &Connection, uuid: &str, ringid: &str) -> rusqlite::Result<()> {
    conn.execute(
        "DELETE FROM silicon_reservations WHERE account_uuid = ?1 AND ringid = ?2",
        params![uuid, ringid],
    )?;
    Ok(())
}

// ---- helpers used under the call lock ---------------------------------------------------------

fn ting_to_silicons(
    tx: &Connection,
    data: &CallData,
    except: Option<&str>,
    ty: &str,
    key: &str,
    payload: serde_json::Value,
    fx: &mut Effects,
) -> rusqlite::Result<()> {
    for p in data.joined() {
        if Some(p.account_uuid.as_str()) == except || !data.is_silicon(&p.account_uuid) {
            continue;
        }
        let recipient = data.account(&p.account_uuid);
        fx.ting |= ting::enqueue(
            tx,
            &recipient,
            ty,
            &format!("{key}:{}", recipient.uuid),
            payload.clone(),
            ting::PRIORITY_NORMAL,
        )?;
    }
    Ok(())
}

fn ting_incoming(
    tx: &Connection,
    data: &CallData,
    inv: &InvitationRow,
    fx: &mut Effects,
) -> rusqlite::Result<()> {
    let recipient = data.account(&inv.to_uuid);
    let participants: Vec<_> = data
        .joined()
        .map(|p| data.account(&p.account_uuid))
        .collect();
    let payload = json!({
        "ringid": inv.ringid,
        "invitation_id": inv.invitation_id,
        "from": data.account(&inv.from_uuid),
        "participants": participants,
        "expires_at": crate::util::time(inv.expires_at),
        "hint": format!("ring call accept {} --start …", inv.ringid),
    });
    fx.ting |= ting::enqueue(
        tx,
        &recipient,
        "ring.call.incoming",
        &format!("incoming:{}", inv.invitation_id),
        payload,
        ting::PRIORITY_INCOMING,
    )?;
    Ok(())
}

fn ting_busy(
    tx: &Connection,
    data: &CallData,
    inv: &InvitationRow,
    fx: &mut Effects,
) -> rusqlite::Result<()> {
    let recipient = data.account(&inv.to_uuid);
    let payload = json!({"ringid": inv.ringid, "invitation_id": inv.invitation_id, "from": data.account(&inv.from_uuid)});
    fx.ting |= ting::enqueue(
        tx,
        &recipient,
        "ring.call.invitation_busy",
        &format!("busy:{}", inv.invitation_id),
        payload,
        ting::PRIORITY_INCOMING,
    )?;
    Ok(())
}

/// A representative that was started and has not stopped.
fn rep_running(p: &ParticipantRow) -> bool {
    matches!(
        p.rep_status,
        Some(RepresentativeStatus::Starting | RepresentativeStatus::Live)
    )
}

/// Stops the running representatives of a call nobody answered (a silicon caller's, started while
/// ringing so it is ready when the callee answers).
fn stop_unanswered_representatives(
    tx: &Connection,
    data: &CallData,
    fx: &mut Effects,
) -> rusqlite::Result<()> {
    if data.call.answered_at.is_some() {
        return Ok(());
    }
    let ringid = data.call.ringid.as_str();
    for p in data
        .joined()
        .filter(|p| data.is_silicon(&p.account_uuid) && rep_running(p))
    {
        tx.execute(
            "UPDATE participants SET rep_status = 'ended' WHERE ringid = ?1 AND account_uuid = ?2",
            params![ringid, p.account_uuid],
        )?;
        fx.media.push(ToMedia::RepresentativeStop {
            ringid: ringid.to_string(),
            silicon_uuid: p.account_uuid.clone(),
        });
    }
    Ok(())
}

/// Starts the representative of each listed silicon (status `starting`) with fresh tokens.
fn start_representatives(
    app: &AppState,
    tx: &Connection,
    ringid: &str,
    silicons: &[String],
    fx: &mut Effects,
) -> rusqlite::Result<()> {
    if silicons.is_empty() {
        return Ok(());
    }
    for uuid in silicons {
        tx.execute(
            "UPDATE participants SET rep_status = 'starting' WHERE ringid = ?1 AND account_uuid = ?2",
            params![ringid, uuid],
        )?;
    }
    let Some(data) = load(tx, ringid)? else {
        return Ok(());
    };
    let roster = data.roster();
    for uuid in silicons {
        let Some(p) = data.participant(uuid) else {
            continue;
        };
        let silicon = data.account(uuid);
        match app.livekit.representative_token(ringid, &silicon) {
            Ok(token) => fx.media.push(ToMedia::RepresentativeStart {
                ringid: ringid.to_string(),
                silicon,
                voice: p
                    .rep_voice
                    .clone()
                    .unwrap_or_else(|| ring_proto::DEFAULT_VOICE.into()),
                context: p.rep_context.clone().unwrap_or_default(),
                start: p.rep_start.clone(),
                roster: roster.clone(),
                token,
            }),
            Err(error) => tracing::error!(%error, "could not mint a representative token"),
        }
    }
    Ok(())
}

fn start_recorder(
    app: &AppState,
    tx: &Connection,
    data: &CallData,
    fx: &mut Effects,
) -> rusqlite::Result<()> {
    let key = app.config.s3_key("recordings", &data.call.ringid, "wav");
    tx.execute(
        "UPDATE calls SET recording_status = 'recording', recording_key = ?2 WHERE ringid = ?1",
        params![data.call.ringid, key],
    )?;
    match app.livekit.recorder_token(&data.call.ringid) {
        Ok(token) => fx.media.push(ToMedia::RecorderStart {
            ringid: data.call.ringid.clone(),
            token,
            roster: data.roster(),
            s3_key: key,
        }),
        Err(error) => tracing::error!(%error, "could not mint a recorder token"),
    }
    Ok(())
}

/// Ends the call: cancels ringing, closes delegations, flushes silicons' transcript, tells
/// ring-media and LiveKit, and notifies everyone.
pub fn end_call_tx(
    app: &AppState,
    tx: &Connection,
    data: &CallData,
    reason: EndReason,
    fx: &mut Effects,
) -> rusqlite::Result<()> {
    let ringid = data.call.ringid.as_str();
    let now = now_ms();
    for inv in data.ringing() {
        set_invitation_state(
            tx,
            &inv.invitation_id,
            InvitationState::Cancelled,
            None,
            None,
        )?;
        fx.stop(inv, RingingStoppedReason::Cancelled, None);
    }
    stop_unanswered_representatives(tx, data, fx)?;
    let was_recording = data.call.recording_status == ring_proto::RecordingStatus::Recording;
    tx.execute(
        "UPDATE calls SET state = 'ended', ended_at = ?2, end_reason = ?3,
           recording_status = CASE recording_status WHEN 'recording' THEN 'processing' ELSE recording_status END WHERE ringid = ?1",
        params![ringid, now, enum_str(&reason)],
    )?;
    tx.execute(
        "UPDATE participants SET state = 'left', left_at = ?2 WHERE ringid = ?1 AND state = 'joined'",
        params![ringid, now],
    )?;
    tx.execute(
        "UPDATE participants SET rep_status = 'ended' WHERE ringid = ?1 AND rep_status IN ('starting', 'live')",
        [ringid],
    )?;
    tx.execute(
        "DELETE FROM silicon_reservations WHERE ringid = ?1",
        [ringid],
    )?;
    tx.execute(
        "UPDATE delegations SET status = 'closed' WHERE ringid = ?1",
        [ringid],
    )?;
    if let Some(vm) = &data.call.voicemail_id {
        let recording: Option<String> = tx
            .query_row(
                "SELECT audio_status FROM voicemails WHERE voicemail_id = ?1",
                [vm],
                |r| r.get(0),
            )
            .optional()?;
        if recording.as_deref() == Some("recording") {
            tx.execute(
                "UPDATE voicemails SET audio_status = 'processing' WHERE voicemail_id = ?1",
                [vm],
            )?;
            fx.media.push(ToMedia::VoicemailStop {
                voicemail_id: vm.clone(),
            });
        }
    }
    let entry = append(
        tx,
        ringid,
        EntryKind::CallEnded,
        None,
        None,
        json!({"reason": reason}),
        None,
    )?;
    fx.entry(entry, None);

    // Silicons that were ever on the call get the rest of the transcript, then the end.
    let silicons: Vec<String> = data
        .participants
        .iter()
        .filter(|p| data.is_silicon(&p.account_uuid))
        .map(|p| p.account_uuid.clone())
        .collect();
    if let Some(after) = load(tx, ringid)? {
        fx.ting |= flush_batches(tx, &after, &silicons)?;
    }
    let duration_ms = data.call.answered_at.map(|a| (now - a).max(0)).unwrap_or(0);
    for uuid in &silicons {
        let recipient = data.account(uuid);
        let payload = json!({"ringid": ringid, "reason": reason, "duration_ms": duration_ms});
        fx.ting |= ting::enqueue(
            tx,
            &recipient,
            "ring.call.ended",
            &format!("ended:{ringid}:{uuid}"),
            payload,
            ting::PRIORITY_NORMAL,
        )?;
    }
    if was_recording {
        fx.media.push(ToMedia::RecorderStop {
            ringid: ringid.to_string(),
        });
    }
    fx.media.push(ToMedia::CallClosed {
        ringid: ringid.to_string(),
    });
    fx.delete_room = true;
    fx.call_ended = true;
    app.timers.cancel_prefix(&format!("grace:{ringid}:"));
    app.timers.cancel(&format!("voicemail:{ringid}"));
    tracing::info!(ringid, reason = enum_str(&reason), "call ended");
    Ok(())
}

/// Moves a direct call whose invitation failed into voicemail (or ends it when the callee has
/// voicemail off).
fn enter_voicemail(
    app: &AppState,
    tx: &Connection,
    data: &CallData,
    reason: VoicemailReason,
    decline_reason: Option<&str>,
    fx: &mut Effects,
) -> rusqlite::Result<()> {
    let ringid = data.call.ringid.as_str();
    let caller_uuid = data.call.created_by.as_str();
    let Some(callee_invitation) = data.invitations.first() else {
        return end_call_tx(app, tx, data, voicemail_reason_to_end(reason), fx);
    };
    // The callee blocked the caller, or its Ring account is deleted (or being deleted): no
    // voicemail, as if the callee had it off.
    if callee_invitation.hidden || crate::deletion::gone(tx, &callee_invitation.to_uuid)? {
        return end_call_tx(app, tx, data, voicemail_reason_to_end(reason), fx);
    }
    let callee_uuid = callee_invitation.to_uuid.clone();
    let callee = data.account(&callee_uuid);
    let callee_settings = settings::load(tx, &callee_uuid)?.settings;
    let caller_joined = data.participant(caller_uuid).is_some_and(|p| p.joined());
    if !callee_settings.voicemail.enabled || !caller_joined {
        return end_call_tx(app, tx, data, voicemail_reason_to_end(reason), fx);
    }
    tx.execute(
        "UPDATE calls SET state = 'voicemail', voicemail_reason = ?2, decline_reason = ?3 WHERE ringid = ?1",
        params![ringid, reason.as_str(), decline_reason],
    )?;
    stop_unanswered_representatives(tx, data, fx)?;
    if data.is_silicon(caller_uuid) {
        let caller = data.account(caller_uuid);
        let payload = json!({
            "ringid": ringid,
            "reason": reason,
            "callee": callee,
            "decline_reason": decline_reason,
            "hint": format!("ring voicemail leave {ringid} --text …"),
        });
        fx.ting |= ting::enqueue(
            tx,
            &caller,
            "ring.voicemail.offered",
            &format!("vm-offer:{ringid}"),
            payload,
            ting::PRIORITY_INCOMING,
        )?;
        let app2 = app.clone();
        let id = ringid.to_string();
        app.timers.schedule(
            format!("voicemail:{ringid}"),
            SILICON_VOICEMAIL_WINDOW,
            async move { finish_voicemail(&app2, &id).await },
        );
    } else {
        let voicemail_id = new_id(ring_proto::prefix::VOICEMAIL);
        let now = now_ms();
        tx.execute(
            "INSERT INTO voicemails (voicemail_id, ringid, from_uuid, to_uuid, reason, decline_reason, created_at, audio_status, transcript_status)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'recording', 'pending')",
            params![voicemail_id, ringid, caller_uuid, callee_uuid, reason.as_str(), decline_reason, now],
        )?;
        tx.execute(
            "UPDATE calls SET voicemail_id = ?2 WHERE ringid = ?1",
            params![ringid, voicemail_id],
        )?;
        let participant = data.participant(caller_uuid);
        let caller_identity = participant
            .and_then(|p| p.media_identity.clone())
            .or_else(|| {
                participant
                    .and_then(|p| p.device_id.as_deref())
                    .map(|d| livekit::carbon_identity(caller_uuid, d))
            })
            .unwrap_or_default();
        match app.livekit.voicemail_token(ringid) {
            Ok(token) => fx.media.push(ToMedia::VoicemailStart {
                ringid: ringid.to_string(),
                voicemail_id: voicemail_id.clone(),
                caller_identity,
                greeting_text: settings::greeting_text(&callee_settings, &callee, reason),
                voice: callee_settings.voice.clone(),
                max_seconds: ring_proto::VOICEMAIL_MAX_SECONDS,
                token,
                s3_key: app.config.s3_key("voicemail", &voicemail_id, "wav"),
            }),
            Err(error) => tracing::error!(%error, "could not mint a voicemail token"),
        }
        let app2 = app.clone();
        let id = ringid.to_string();
        let limit = Duration::from_secs(ring_proto::VOICEMAIL_MAX_SECONDS) + CARBON_VOICEMAIL_SLACK;
        app.timers
            .schedule(format!("voicemail:{ringid}"), limit, async move {
                finish_voicemail(&app2, &id).await
            });
    }
    fx.call_updated = true;
    tracing::info!(ringid, reason = reason.as_str(), "call went to voicemail");
    Ok(())
}

/// After an invitation stops ringing (declined, missed, busy) or someone leaves: decide whether
/// the call goes on, goes to voicemail, or ends.
fn settle(
    app: &AppState,
    tx: &Connection,
    ringid: &str,
    reason: VoicemailReason,
    decline_reason: Option<&str>,
    fx: &mut Effects,
) -> rusqlite::Result<()> {
    let Some(data) = load(tx, ringid)? else {
        return Ok(());
    };
    match data.call.state {
        CallState::Ringing => {
            if data.ringing().next().is_some() {
                return Ok(());
            }
            if data.is_direct() {
                enter_voicemail(app, tx, &data, reason, decline_reason, fx)
            } else {
                end_call_tx(app, tx, &data, voicemail_reason_to_end(reason), fx)
            }
        }
        CallState::Active => continuation(app, tx, &data, fx),
        _ => Ok(()),
    }
}

/// An active call goes on while 2+ people remain, or 1 remains with someone still ringing.
fn continuation(
    app: &AppState,
    tx: &Connection,
    data: &CallData,
    fx: &mut Effects,
) -> rusqlite::Result<()> {
    let joined = data.joined().count();
    let ringing = data.ringing().count();
    if joined >= 2 || (joined == 1 && ringing > 0) {
        return Ok(());
    }
    end_call_tx(app, tx, data, EndReason::Completed, fx)
}

// ---- create -----------------------------------------------------------------------------------

pub async fn create(app: &AppState, auth: &Auth, req: CreateCall) -> ApiResult<CreatedCall> {
    let caller = auth.account.clone();
    let silicon = auth.is_silicon();
    app.limiter.check(
        &format!("calls:{}", caller.uuid),
        30,
        Duration::from_secs(60),
    )?;
    if !silicon
        && (req.start.is_some()
            || req.context.is_some()
            || req.context_mode.is_some()
            || req.approval_id.is_some())
    {
        return Err(ApiError::invalid(
            "start, context, context_mode and approval_id are for silicons.",
        ));
    }
    if !silicon && auth.session.platform == Some(Platform::Cli) {
        return Err(ApiError::forbidden("Carbons call from the Ring apps."));
    }
    let start = clean(req.start);
    if let Some(start) = &start {
        check_len("start", start, limits::START)?;
    }
    let callee = accounts::resolve(app, &req.to).await?;
    if callee.uuid == caller.uuid {
        return Err(ApiError::invalid("You can't call yourself."));
    }
    let rep = if silicon {
        let context = settings::resolve_context(
            app,
            &caller.uuid,
            ContextAction::Init,
            &callee.id,
            req.context,
            req.context_mode,
            req.approval_id,
        )?;
        let voice = app
            .db
            .with(|c| settings::load(c, &caller.uuid))?
            .settings
            .voice;
        Some((context.text, voice))
    } else {
        None
    };
    let ringid = new_id(ring_proto::prefix::CALL);
    let _guard = app.call_locks.lock(&ringid).await;
    if silicon && !app.db.with(|c| reserve(c, &caller.uuid, &ringid))? {
        return Err(ApiError::already_in_call());
    }
    if let Err(error) = app.livekit.create_room(&ringid).await {
        tracing::error!(%error, "could not create the LiveKit room");
        let _ = app.db.with(|c| release(c, &caller.uuid, &ringid));
        return Err(ApiError::dependency("Ring media"));
    }
    let media: Option<MediaGrant> = if silicon {
        None
    } else {
        Some(
            app.livekit
                // The call only rings: nobody records the caller's first words yet.
                .device_grant(&ringid, &caller, auth.device_id(), false)
                .map_err(|error| {
                    tracing::error!(%error, "could not mint a media grant");
                    ApiError::internal()
                })?,
        )
    };
    let device_id = (!silicon).then(|| auth.device_id().to_string());
    let media_identity = media.as_ref().map(|m| m.identity.clone());
    let mut fx = Effects::default();
    let created = app.db.tx(|tx| -> ApiResult<(InvitationRow, bool)> {
        // Checked in the transaction, so a deletion that has begun always sees this call.
        if crate::deletion::pending(tx, &caller.uuid)? {
            return Err(ApiError::session_expired());
        }
        // A call from someone the callee blocked rings silently until it times out unanswered.
        let hidden = crate::blocks::is_blocked(tx, &callee.uuid, &caller.uuid)?;
        // A callee whose Ring account is deleted (or being deleted) is like one not on Ring:
        // nothing rings, and the call times out unanswered without voicemail.
        let gone = crate::deletion::gone(tx, &callee.uuid)?;
        let busy = !hidden && !gone && callee.kind == AccountKind::Silicon && reserved_elsewhere(tx, &callee.uuid, &ringid)?;
        let now = now_ms();
        tx.execute(
            "INSERT INTO calls (ringid, state, created_by, created_at) VALUES (?1, 'ringing', ?2, ?3)",
            params![ringid, caller.uuid, now],
        )?;
        tx.execute(
            "INSERT INTO participants (ringid, account_uuid, state, joined_at, device_id, media_identity, rep_voice, rep_context, rep_start)
             VALUES (?1, ?2, 'joined', ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                ringid,
                caller.uuid,
                now,
                device_id,
                media_identity,
                rep.as_ref().map(|r| r.1.clone()),
                rep.as_ref().map(|r| r.0.clone()),
                start
            ],
        )?;
        let mut inv = new_invitation(&ringid, &caller.uuid, &callee.uuid, if busy { InvitationState::Busy } else { InvitationState::Ringing });
        inv.hidden = hidden;
        insert_invitation(tx, &inv)?;
        let entry = append(tx, &ringid, EntryKind::CallStarted, Some(&caller), None, json!({}), None)?;
        fx.entry(entry, None);
        fx.call_updated = true;
        let data = load(tx, &ringid)?.ok_or(rusqlite::Error::QueryReturnedNoRows)?;
        if busy {
            let entry = append(tx, &ringid, EntryKind::Busy, Some(&callee), None, json!({}), None)?;
            fx.entry(entry, None);
            ting_busy(tx, &data, &inv, &mut fx)?;
            settle(app, tx, &ringid, VoicemailReason::Busy, None, &mut fx)?;
        } else {
            if !hidden && !gone {
                if callee.kind == AccountKind::Silicon {
                    ting_incoming(tx, &data, &inv, &mut fx)?;
                }
                fx.incoming.push(inv.invitation_id.clone());
            }
            fx.schedule_expiry.push(inv.clone());
            // A silicon caller's representative connects while the call rings, so it is ready
            // to speak the moment the callee answers (ring-media holds the start text until then).
            if silicon {
                start_representatives(app, tx, &ringid, std::slice::from_ref(&caller.uuid), &mut fx)?;
            }
        }
        Ok((inv, gone))
    });
    let (invitation, callee_gone) = match created {
        Ok(created) => created,
        Err(error) => {
            if silicon {
                let _ = app.db.with(|c| release(c, &caller.uuid, &ringid));
            }
            let ringid = ringid.clone();
            let app = app.clone();
            tokio::spawn(async move {
                let _ = app.livekit.delete_room(&ringid).await;
            });
            return Err(error);
        }
    };
    tracing::info!(ringid = %ringid, caller_kind = caller.kind.as_str(), callee_kind = callee.kind.as_str(), "call created");
    if !silicon
        && invitation.state == InvitationState::Ringing
        && !invitation.hidden
        && !callee_gone
    {
        // A carbon caller who reaches voicemail hears the callee's greeting: have it ready.
        match app.db.with(|c| settings::load(c, &callee.uuid)) {
            Ok(stored) => settings::prepare_greetings(app, &stored.settings, &callee),
            Err(error) => tracing::warn!(%error, "could not read the callee's settings"),
        }
    }
    fx.apply(app, &ringid).await;
    if let Some(grant) = &media {
        expect_media(app, &ringid, &caller.uuid, &grant.identity);
    }
    let (data, call) =
        super::load_rendered(app, &ringid, &caller.uuid)?.ok_or_else(ApiError::internal)?;
    let invitation = data
        .invitations
        .iter()
        .find(|i| i.invitation_id == invitation.invitation_id)
        .map(|i| render_invitation(&data, i))
        .ok_or_else(ApiError::internal)?;
    Ok(CreatedCall {
        call,
        invitation,
        media,
    })
}

pub fn schedule_expiry(app: &AppState, inv: &InvitationRow) {
    let delay = Duration::from_millis((inv.expires_at - now_ms()).max(0) as u64);
    let app2 = app.clone();
    let ringid = inv.ringid.clone();
    let invitation_id = inv.invitation_id.clone();
    app.timers
        .schedule(format!("expiry:{}", inv.invitation_id), delay, async move {
            expire_invitation(&app2, &ringid, &invitation_id).await;
        });
}

// ---- answering ---------------------------------------------------------------------------------

fn invitation_for<'a>(
    data: &'a CallData,
    uuid: &str,
    invitation_id: Option<&str>,
) -> ApiResult<&'a InvitationRow> {
    let mine = data.visible_invitations().filter(|i| i.to_uuid == uuid);
    let inv = match invitation_id {
        Some(id) => mine.clone().find(|i| i.invitation_id == id),
        None => mine
            .clone()
            .find(|i| i.state == InvitationState::Ringing)
            .or_else(|| mine.clone().next_back()),
    };
    let inv = inv.ok_or_else(|| ApiError::not_found("Invitation"))?;
    if inv.state != InvitationState::Ringing {
        return Err(ApiError::new(
            axum::http::StatusCode::CONFLICT,
            ring_proto::codes::INVITATION_NOT_RINGING,
            "This invitation is no longer ringing.",
        )
        .details(json!({"state": inv.state})));
    }
    if inv.expires_at <= now_ms() {
        return Err(ApiError::new(
            axum::http::StatusCode::GONE,
            ring_proto::codes::INVITATION_EXPIRED,
            "The invitation expired.",
        )
        .hint("Start a new call."));
    }
    Ok(inv)
}

fn load_for(app: &AppState, ringid: &str, uuid: &str) -> ApiResult<CallData> {
    let data = app
        .db
        .with(|c| load(c, ringid))?
        .ok_or_else(|| ApiError::not_found("Call"))?;
    if !data.can_view(uuid) {
        return Err(ApiError::not_participant());
    }
    Ok(data)
}

pub async fn accept(
    app: &AppState,
    auth: &Auth,
    ringid: &str,
    req: AcceptCall,
) -> ApiResult<CallWithMedia> {
    let account = auth.account.clone();
    let silicon = auth.is_silicon();
    if !silicon
        && (req.start.is_some()
            || req.context.is_some()
            || req.context_mode.is_some()
            || req.approval_id.is_some())
    {
        return Err(ApiError::invalid(
            "start, context, context_mode and approval_id are for silicons.",
        ));
    }
    if !silicon && auth.session.platform == Some(Platform::Cli) {
        return Err(ApiError::forbidden("Carbons answer from the Ring apps."));
    }
    let start = clean(req.start);
    if let Some(start) = &start {
        check_len("start", start, limits::START)?;
    }
    // Validate the invitation before asking the silicon to approve anything.
    {
        let data = load_for(app, ringid, &account.uuid)?;
        if data.call.state == CallState::Ended {
            return Err(ApiError::call_ended());
        }
        invitation_for(&data, &account.uuid, req.invitation_id.as_deref())?;
    }
    let rep = if silicon {
        let context = settings::resolve_context(
            app,
            &account.uuid,
            ContextAction::Accept,
            ringid,
            req.context,
            req.context_mode,
            req.approval_id,
        )?;
        let voice = app
            .db
            .with(|c| settings::load(c, &account.uuid))?
            .settings
            .voice;
        Some((context.text, voice))
    } else {
        None
    };

    let _guard = app.call_locks.lock(ringid).await;
    let data = load_for(app, ringid, &account.uuid)?;
    if data.call.state == CallState::Ended {
        return Err(ApiError::call_ended());
    }
    let inv = invitation_for(&data, &account.uuid, req.invitation_id.as_deref())?.clone();
    let media = if silicon {
        None
    } else {
        Some(
            app.livekit
                .device_grant(ringid, &account, auth.device_id(), true)
                .map_err(|error| {
                    tracing::error!(%error, "could not mint a media grant");
                    ApiError::internal()
                })?,
        )
    };
    let mut fx = Effects::default();
    let device_id = auth.device_id().to_string();
    app.db.tx(|tx| -> ApiResult<()> {
        if crate::deletion::pending(tx, &account.uuid)? {
            return Err(ApiError::session_expired());
        }
        if silicon && !reserve(tx, &account.uuid, ringid)? {
            return Err(ApiError::already_in_call());
        }
        if !set_invitation_state(tx, &inv.invitation_id, InvitationState::Accepted, None, Some(&device_id))? {
            return Err(ApiError::new(axum::http::StatusCode::CONFLICT, ring_proto::codes::INVITATION_NOT_RINGING, "This invitation is no longer ringing."));
        }
        let now = now_ms();
        tx.execute(
            "INSERT INTO participants (ringid, account_uuid, state, joined_at, invited_by, device_id, media_identity, rep_voice, rep_context, rep_start)
             VALUES (?1, ?2, 'joined', ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(ringid, account_uuid) DO UPDATE SET state = 'joined', joined_at = excluded.joined_at, left_at = NULL,
               invited_by = excluded.invited_by, device_id = excluded.device_id, media_identity = excluded.media_identity,
               rep_voice = excluded.rep_voice, rep_context = excluded.rep_context, rep_start = excluded.rep_start, rep_status = NULL",
            params![
                ringid,
                account.uuid,
                now,
                inv.from_uuid,
                (!silicon).then_some(device_id.clone()),
                media.as_ref().map(|m| m.identity.clone()),
                rep.as_ref().map(|r| r.1.clone()),
                rep.as_ref().map(|r| r.0.clone()),
                start
            ],
        )?;
        let first_answer = data.call.state == CallState::Ringing;
        if first_answer {
            tx.execute("UPDATE calls SET state = 'active', answered_at = ?2 WHERE ringid = ?1", params![ringid, now])?;
            let entry = append(tx, ringid, EntryKind::Answered, Some(&account), None, json!({"after_ms": now - inv.created_at}), None)?;
            fx.entry(entry, None);
        } else {
            let entry = append(tx, ringid, EntryKind::Joined, Some(&account), None, json!({}), None)?;
            fx.entry(entry, None);
        }
        let after = load(tx, ringid)?.ok_or_else(ApiError::internal)?;
        // A silicon caller's representative is usually running already (started while ringing).
        let prewarmed = after.joined().any(|p| after.is_silicon(&p.account_uuid) && rep_running(p));
        let to_start: Vec<String> = if first_answer {
            after.joined().filter(|p| after.is_silicon(&p.account_uuid) && !rep_running(p)).map(|p| p.account_uuid.clone()).collect()
        } else if silicon {
            vec![account.uuid.clone()]
        } else {
            vec![]
        };
        if first_answer {
            start_recorder(app, tx, &after, &mut fx)?;
        }
        if !first_answer || prewarmed {
            fx.media.push(ToMedia::RosterChanged { ringid: ringid.to_string(), roster: after.roster(), joined: Some(account.clone()), left: None });
        }
        start_representatives(app, tx, ringid, &to_start, &mut fx)?;
        ting_to_silicons(tx, &after, Some(&account.uuid), "ring.call.accepted", &format!("accepted:{}", inv.invitation_id), json!({"ringid": ringid, "by": account}), &mut fx)?;
        fx.stop(&inv, RingingStoppedReason::AcceptedElsewhere, Some(&device_id));
        fx.call_updated = true;
        Ok(())
    })?;
    app.timers.cancel(&format!("expiry:{}", inv.invitation_id));
    tracing::info!(ringid, kind = account.kind.as_str(), "invitation accepted");
    drop(_guard);
    fx.apply(app, ringid).await;
    if let Some(grant) = &media {
        expect_media(app, ringid, &account.uuid, &grant.identity);
    }
    let (_, call) =
        super::load_rendered(app, ringid, &account.uuid)?.ok_or_else(ApiError::internal)?;
    Ok(CallWithMedia { call, media })
}

pub async fn decline(
    app: &AppState,
    auth: &Auth,
    ringid: &str,
    req: DeclineCall,
) -> ApiResult<Call> {
    let account = auth.account.clone();
    let reason = clean(req.reason);
    if let Some(reason) = &reason {
        check_len("reason", reason, limits::DECLINE_REASON)?;
    }
    if auth.is_silicon() && reason.is_none() && !req.give_no_reason {
        return Err(ApiError::invalid(
            "Silicons must give a reason or decline with give_no_reason.",
        )
        .hint("Pass --reason TEXT or --give-no-reason."));
    }
    let _guard = app.call_locks.lock(ringid).await;
    let data = load_for(app, ringid, &account.uuid)?;
    if data.call.state == CallState::Ended {
        return Err(ApiError::call_ended());
    }
    let inv = invitation_for(&data, &account.uuid, req.invitation_id.as_deref())?.clone();
    let device_id = auth.device_id().to_string();
    let mut fx = Effects::default();
    app.db.tx(|tx| -> ApiResult<()> {
        if !set_invitation_state(
            tx,
            &inv.invitation_id,
            InvitationState::Declined,
            reason.as_deref(),
            Some(&device_id),
        )? {
            return Err(ApiError::new(
                axum::http::StatusCode::CONFLICT,
                ring_proto::codes::INVITATION_NOT_RINGING,
                "This invitation is no longer ringing.",
            ));
        }
        let entry = append(
            tx,
            ringid,
            EntryKind::Declined,
            Some(&account),
            None,
            json!({"reason": reason}),
            None,
        )?;
        fx.entry(entry, None);
        ting_to_silicons(
            tx,
            &data,
            Some(&account.uuid),
            "ring.call.declined",
            &format!("declined:{}", inv.invitation_id),
            json!({"ringid": ringid, "by": account, "reason": reason}),
            &mut fx,
        )?;
        fx.stop(&inv, RingingStoppedReason::Declined, Some(&device_id));
        fx.call_updated = true;
        settle(
            app,
            tx,
            ringid,
            VoicemailReason::Declined,
            reason.as_deref(),
            &mut fx,
        )?;
        Ok(())
    })?;
    app.timers.cancel(&format!("expiry:{}", inv.invitation_id));
    drop(_guard);
    fx.apply(app, ringid).await;
    Ok(super::load_rendered(app, ringid, &account.uuid)?
        .ok_or_else(ApiError::internal)?
        .1)
}

pub async fn silence(
    app: &AppState,
    auth: &Auth,
    ringid: &str,
    invitation_id: Option<String>,
) -> ApiResult<Call> {
    let account = auth.account.clone();
    let _guard = app.call_locks.lock(ringid).await;
    let data = load_for(app, ringid, &account.uuid)?;
    if data.call.state == CallState::Ended {
        return Err(ApiError::call_ended());
    }
    let inv = invitation_for(&data, &account.uuid, invitation_id.as_deref())?.clone();
    let mut fx = Effects::default();
    if !inv.silenced {
        app.db.with(|c| {
            c.execute(
                "UPDATE invitations SET silenced = 1 WHERE invitation_id = ?1",
                [&inv.invitation_id],
            )
        })?;
        fx.stop(&inv, RingingStoppedReason::Silenced, Some(auth.device_id()));
        fx.call_updated = true;
    }
    drop(_guard);
    fx.apply(app, ringid).await;
    Ok(super::load_rendered(app, ringid, &account.uuid)?
        .ok_or_else(ApiError::internal)?
        .1)
}

// ---- leaving -----------------------------------------------------------------------------------

/// Removes `uuid` from the call (hang up, cancel, or finish a voicemail). Runs in a transaction.
fn leave_tx(
    app: &AppState,
    tx: &Transaction<'_>,
    data: &CallData,
    uuid: &str,
    fx: &mut Effects,
) -> ApiResult<()> {
    let ringid = data.call.ringid.as_str();
    let participant = data
        .participant(uuid)
        .filter(|p| p.joined())
        .ok_or_else(ApiError::not_participant)?;
    let is_creator = data.call.created_by == uuid;
    match data.call.state {
        CallState::Ringing if is_creator => {
            for inv in data.ringing() {
                set_invitation_state(
                    tx,
                    &inv.invitation_id,
                    InvitationState::Cancelled,
                    None,
                    None,
                )?;
                fx.stop(inv, RingingStoppedReason::Cancelled, None);
                app.timers.cancel(&format!("expiry:{}", inv.invitation_id));
            }
            let after = load(tx, ringid)?.ok_or_else(ApiError::internal)?;
            end_call_tx(app, tx, &after, EndReason::Cancelled, fx)?;
        }
        CallState::Voicemail if is_creator => {
            let reason = data
                .call
                .voicemail_reason
                .as_deref()
                .and_then(super::enum_from::<VoicemailReason>)
                .unwrap_or(VoicemailReason::Missed);
            end_call_tx(app, tx, data, voicemail_reason_to_end(reason), fx)?;
        }
        _ => {
            let now = now_ms();
            tx.execute(
                "UPDATE participants SET state = 'left', left_at = ?3,
                   rep_status = CASE WHEN rep_status IN ('starting', 'live') THEN 'ended' ELSE rep_status END
                 WHERE ringid = ?1 AND account_uuid = ?2",
                params![ringid, uuid, now],
            )?;
            let account = data.account(uuid);
            let entry = append(
                tx,
                ringid,
                EntryKind::Left,
                Some(&account),
                None,
                json!({}),
                None,
            )?;
            fx.entry(entry, None);
            if account.kind == AccountKind::Silicon {
                release(tx, uuid, ringid)?;
                if participant.rep_status.is_some() {
                    fx.media.push(ToMedia::RepresentativeStop {
                        ringid: ringid.to_string(),
                        silicon_uuid: uuid.to_string(),
                    });
                }
                if let Some(after) = load(tx, ringid)? {
                    fx.ting |= flush_batches(tx, &after, &[uuid.to_string()])?;
                }
            }
            app.timers.cancel(&format!("grace:{ringid}:{uuid}"));
            let after = load(tx, ringid)?.ok_or_else(ApiError::internal)?;
            fx.media.push(ToMedia::RosterChanged {
                ringid: ringid.to_string(),
                roster: after.roster(),
                joined: None,
                left: Some(account.clone()),
            });
            ting_to_silicons(
                tx,
                &after,
                None,
                "ring.participant.left",
                &format!("left:{ringid}:{uuid}:{now}"),
                json!({"ringid": ringid, "account": account}),
                fx,
            )?;
            fx.call_updated = true;
            if after.call.state == CallState::Active {
                continuation(app, tx, &after, fx)?;
            } else if after.joined().next().is_none() {
                end_call_tx(app, tx, &after, EndReason::Completed, fx)?;
            }
        }
    }
    Ok(())
}

pub async fn leave(app: &AppState, auth: &Auth, ringid: &str) -> ApiResult<Call> {
    let uuid = auth.uuid().to_string();
    let _guard = app.call_locks.lock(ringid).await;
    let data = load_for(app, ringid, &uuid)?;
    if data.call.state == CallState::Ended {
        return Err(ApiError::call_ended());
    }
    let mut fx = Effects::default();
    app.db.tx(|tx| leave_tx(app, tx, &data, &uuid, &mut fx))?;
    drop(_guard);
    fx.apply(app, ringid).await;
    Ok(super::load_rendered(app, ringid, &uuid)?
        .ok_or_else(ApiError::internal)?
        .1)
}

// ---- inviting -----------------------------------------------------------------------------------

pub async fn invite(
    app: &AppState,
    auth: &Auth,
    ringid: &str,
    req: InviteToCall,
) -> ApiResult<InvitedCall> {
    let inviter = auth.account.clone();
    let invitee = accounts::resolve(app, &req.to).await?;
    if invitee.uuid == inviter.uuid {
        return Err(ApiError::invalid("You are already on this call."));
    }
    let _guard = app.call_locks.lock(ringid).await;
    let data = load_for(app, ringid, &inviter.uuid)?;
    match data.call.state {
        CallState::Ended => return Err(ApiError::call_ended()),
        CallState::Voicemail => return Err(ApiError::conflict("This call is in voicemail.")),
        _ => {}
    }
    if !data.participant(&inviter.uuid).is_some_and(|p| p.joined()) {
        return Err(ApiError::not_participant());
    }
    if data.participant(&invitee.uuid).is_some_and(|p| p.joined()) {
        return Err(ApiError::conflict(format!(
            "{} is already on this call.",
            invitee.id
        )));
    }
    if data.ringing().any(|i| i.to_uuid == invitee.uuid) {
        return Err(ApiError::conflict(format!(
            "{} is already being rung.",
            invitee.id
        )));
    }
    // This call stopped being theirs when they deleted their Ring account.
    if data.departed.contains(&invitee.uuid) {
        return Err(ApiError::conflict(format!(
            "{} can't be added back to this call.",
            invitee.id
        ))
        .hint("Start a new call."));
    }
    let mut fx = Effects::default();
    let inv = app.db.tx(|tx| -> ApiResult<InvitationRow> {
        if crate::deletion::pending(tx, &inviter.uuid)? {
            return Err(ApiError::session_expired());
        }
        // An invitation from someone the invitee blocked rings silently until it times out.
        let hidden = crate::blocks::is_blocked(tx, &invitee.uuid, &inviter.uuid)?;
        // So does one to an account whose Ring account is deleted (or being deleted).
        let gone = crate::deletion::gone(tx, &invitee.uuid)?;
        let busy = !hidden
            && !gone
            && invitee.kind == AccountKind::Silicon
            && reserved_elsewhere(tx, &invitee.uuid, ringid)?;
        let mut inv = new_invitation(
            ringid,
            &inviter.uuid,
            &invitee.uuid,
            if busy {
                InvitationState::Busy
            } else {
                InvitationState::Ringing
            },
        );
        inv.hidden = hidden;
        insert_invitation(tx, &inv)?;
        let entry = append(
            tx,
            ringid,
            EntryKind::Invited,
            Some(&inviter),
            None,
            json!({"invitee": invitee}),
            None,
        )?;
        fx.entry(entry, None);
        let after = load(tx, ringid)?.ok_or(rusqlite::Error::QueryReturnedNoRows)?;
        if busy {
            let entry = append(
                tx,
                ringid,
                EntryKind::Busy,
                Some(&invitee),
                None,
                json!({}),
                None,
            )?;
            fx.entry(entry, None);
            ting_busy(tx, &after, &inv, &mut fx)?;
        } else {
            if !hidden && !gone {
                if invitee.kind == AccountKind::Silicon {
                    ting_incoming(tx, &after, &inv, &mut fx)?;
                }
                fx.incoming.push(inv.invitation_id.clone());
            }
            fx.schedule_expiry.push(inv.clone());
        }
        fx.call_updated = true;
        Ok(inv)
    })?;
    drop(_guard);
    fx.apply(app, ringid).await;
    let (data, call) =
        super::load_rendered(app, ringid, &inviter.uuid)?.ok_or_else(ApiError::internal)?;
    let invitation = data
        .invitations
        .iter()
        .find(|i| i.invitation_id == inv.invitation_id)
        .map(|i| render_invitation(&data, i))
        .ok_or_else(ApiError::internal)?;
    Ok(InvitedCall { call, invitation })
}

// ---- media: rejoin and handoff -------------------------------------------------------------------

pub async fn media(app: &AppState, auth: &Auth, ringid: &str) -> ApiResult<CallWithMedia> {
    if auth.is_silicon() {
        return Err(ApiError::forbidden(
            "Silicons are represented on the server and need no media.",
        ));
    }
    let account = auth.account.clone();
    let device_id = auth.device_id().to_string();
    let _guard = app.call_locks.lock(ringid).await;
    let data = load_for(app, ringid, &account.uuid)?;
    if data.call.state == CallState::Ended {
        return Err(ApiError::call_ended());
    }
    let participant = data
        .participant(&account.uuid)
        .filter(|p| p.joined())
        .ok_or_else(ApiError::not_participant)?
        .clone();
    let grant = app
        .livekit
        .device_grant(
            ringid,
            &account,
            &device_id,
            data.call.state == CallState::Active,
        )
        .map_err(|error| {
            tracing::error!(%error, "could not mint a media grant");
            ApiError::internal()
        })?;
    app.timers
        .cancel(&format!("grace:{ringid}:{}", account.uuid));
    let mut fx = Effects::default();
    if participant.device_id.as_deref() != Some(device_id.as_str()) {
        let old_identity = participant.media_identity.clone();
        let device = app.db.with(|c| {
            c.execute(
                "UPDATE participants SET device_id = ?3, media_identity = ?4 WHERE ringid = ?1 AND account_uuid = ?2",
                params![ringid, account.uuid, device_id, grant.identity],
            )?;
            super::device_ref(c, &device_id)
        })?;
        fx.events.push((
            account.uuid.clone(),
            Event::CallHandoff {
                ringid: ringid.to_string(),
                device_id: device_id.clone(),
                device: Some(device),
            },
        ));
        fx.media.push(ToMedia::AuthorityChanged {
            ringid: ringid.to_string(),
            account_uuid: account.uuid.clone(),
            identity: grant.identity.clone(),
        });
        fx.call_updated = true;
        if let Some(old) = old_identity.filter(|o| *o != grant.identity) {
            let app2 = app.clone();
            let room = ringid.to_string();
            tokio::spawn(async move {
                tokio::time::sleep(HANDOFF_REMOVE_AFTER).await;
                if let Err(error) = app2.livekit.remove_participant(&room, &old).await {
                    tracing::debug!(%error, "old device was already gone after handoff");
                }
            });
        }
        tracing::info!(ringid, "call handed off to another device");
    }
    drop(_guard);
    fx.apply(app, ringid).await;
    expect_media(app, ringid, &account.uuid, &grant.identity);
    let (_, call) =
        super::load_rendered(app, ringid, &account.uuid)?.ok_or_else(ApiError::internal)?;
    Ok(CallWithMedia {
        call,
        media: Some(grant),
    })
}

// ---- timers and media events -------------------------------------------------------------------

pub async fn expire_invitation(app: &AppState, ringid: &str, invitation_id: &str) {
    let _guard = app.call_locks.lock(ringid).await;
    let Ok(Some(data)) = app.db.with(|c| load(c, ringid)) else {
        return;
    };
    let Some(inv) = data
        .invitations
        .iter()
        .find(|i| i.invitation_id == invitation_id && i.state == InvitationState::Ringing)
        .cloned()
    else {
        return;
    };
    let mut fx = Effects::default();
    let result = app.db.tx(|tx| -> rusqlite::Result<()> {
        if !set_invitation_state(tx, &inv.invitation_id, InvitationState::Missed, None, None)? {
            return Ok(());
        }
        let invitee = data.account(&inv.to_uuid);
        let entry = append(
            tx,
            ringid,
            EntryKind::Missed,
            Some(&invitee),
            None,
            json!({}),
            None,
        )?;
        fx.entry(entry, None);
        // The invitation never rang an invitee who blocked the inviter, or whose Ring account is
        // deleted (or being deleted), and nothing tells it that it missed a call.
        let unreachable = inv.hidden || crate::deletion::gone(tx, &inv.to_uuid)?;
        if !unreachable {
            fx.stop(&inv, RingingStoppedReason::Missed, None);
            fx.missed.push(inv.clone());
        }
        let payload =
            json!({"ringid": ringid, "invitation_id": inv.invitation_id, "invitee": invitee});
        let notified = if unreachable {
            vec![&inv.from_uuid]
        } else {
            vec![&inv.to_uuid, &inv.from_uuid]
        };
        for uuid in notified {
            if data.is_silicon(uuid) {
                let recipient = data.account(uuid);
                fx.ting |= ting::enqueue(
                    tx,
                    &recipient,
                    "ring.call.timeout",
                    &format!("timeout:{}:{uuid}", inv.invitation_id),
                    payload.clone(),
                    ting::PRIORITY_NORMAL,
                )?;
            }
        }
        fx.call_updated = true;
        settle(app, tx, ringid, VoicemailReason::Missed, None, &mut fx)
    });
    if let Err(error) = result {
        tracing::error!(%error, "could not expire invitation");
        return;
    }
    drop(_guard);
    fx.apply(app, ringid).await;
}

/// The voicemail is over: the recording finished, the silicon caller never left text, or a carbon
/// recording ran past its limit. Ends the call if it is still in voicemail.
pub async fn finish_voicemail(app: &AppState, ringid: &str) {
    let _guard = app.call_locks.lock(ringid).await;
    let Ok(Some(data)) = app.db.with(|c| load(c, ringid)) else {
        return;
    };
    if data.call.state != CallState::Voicemail {
        return;
    }
    let reason = data
        .call
        .voicemail_reason
        .as_deref()
        .and_then(super::enum_from::<VoicemailReason>)
        .unwrap_or(VoicemailReason::Missed);
    let mut fx = Effects::default();
    if let Err(error) = app
        .db
        .tx(|tx| end_call_tx(app, tx, &data, voicemail_reason_to_end(reason), &mut fx))
    {
        tracing::error!(%error, "could not end voicemail call");
        return;
    }
    drop(_guard);
    fx.apply(app, ringid).await;
}

/// LiveKit says a participant connected: a carbon's media is back, so cancel its grace timer.
pub fn media_joined(app: &AppState, ringid: &str, identity: &str) {
    if let Some((uuid, _)) = livekit::parse_carbon_identity(identity) {
        app.timers.cancel(&format!("grace:{ringid}:{uuid}"));
    }
}

/// LiveKit says a carbon's authoritative device dropped: give it 30 s to come back.
pub fn media_left(app: &AppState, ringid: &str, identity: &str) {
    let Some((uuid, _)) = livekit::parse_carbon_identity(identity) else {
        return;
    };
    let Ok(Some(data)) = app.db.with(|c| load(c, ringid)) else {
        return;
    };
    let authoritative = data
        .participant(uuid)
        .is_some_and(|p| p.joined() && p.media_identity.as_deref() == Some(identity));
    if data.call.state == CallState::Ended || !authoritative {
        return;
    }
    schedule_media_check(app, ringid, uuid, identity, MEDIA_GRACE, false);
}

/// A device was just handed a media grant: if it never shows up in the room (the app crashed or
/// lost the network right after answering), treat its account as gone once the deadline passes.
/// `media_joined` cancels the check.
pub fn expect_media(app: &AppState, ringid: &str, uuid: &str, identity: &str) {
    schedule_media_check(app, ringid, uuid, identity, JOIN_DEADLINE, true);
}

fn schedule_media_check(
    app: &AppState,
    ringid: &str,
    uuid: &str,
    identity: &str,
    delay: Duration,
    only_if_absent: bool,
) {
    let app2 = app.clone();
    let ringid = ringid.to_string();
    let uuid = uuid.to_string();
    let identity = identity.to_string();
    app.timers
        .schedule(format!("grace:{ringid}:{uuid}"), delay, async move {
            media_lost(&app2, &ringid, &uuid, &identity, only_if_absent).await;
        });
}

/// Removes a carbon whose media is gone. LiveKit is asked first, so a device that is in fact
/// connected is kept; with `only_if_absent`, an unanswered question also keeps it.
async fn media_lost(
    app: &AppState,
    ringid: &str,
    uuid: &str,
    identity: &str,
    only_if_absent: bool,
) {
    match app.livekit.has_participant(ringid, identity).await {
        Some(true) => return,
        None if only_if_absent => return,
        _ => {}
    }
    let _guard = app.call_locks.lock(ringid).await;
    let Ok(Some(data)) = app.db.with(|c| load(c, ringid)) else {
        return;
    };
    let still = data
        .participant(uuid)
        .is_some_and(|p| p.joined() && p.media_identity.as_deref() == Some(identity));
    if data.call.state == CallState::Ended || !still {
        return;
    }
    tracing::info!(ringid, "carbon media did not come back; treating as left");
    let mut fx = Effects::default();
    if let Err(error) = app.db.tx(|tx| leave_tx(app, tx, &data, uuid, &mut fx)) {
        tracing::error!(error = %error.message, "could not remove disconnected participant");
        return;
    }
    drop(_guard);
    fx.apply(app, ringid).await;
}

/// A representative changed status in ring-media.
pub async fn representative_status(
    app: &AppState,
    ringid: &str,
    silicon_uuid: &str,
    status: RepresentativeStatus,
    error: Option<String>,
) {
    let _guard = app.call_locks.lock(ringid).await;
    let mut fx = Effects::default();
    let result = app.db.tx(|tx| -> rusqlite::Result<()> {
        let Some(data) = load(tx, ringid)? else { return Ok(()) };
        let Some(p) = data.participant(silicon_uuid) else { return Ok(()) };
        if data.call.state == CallState::Ended || !p.joined() || p.rep_status == Some(status) {
            return Ok(());
        }
        // A representative stopped when the call went to voicemail may still report in late.
        if data.call.state == CallState::Voicemail
            && matches!(status, RepresentativeStatus::Starting | RepresentativeStatus::Live)
        {
            return Ok(());
        }
        tx.execute(
            "UPDATE participants SET rep_status = ?3 WHERE ringid = ?1 AND account_uuid = ?2",
            params![ringid, silicon_uuid, enum_str(&status)],
        )?;
        if status == RepresentativeStatus::Failed {
            let silicon = data.account(silicon_uuid);
            let payload = json!({"ringid": ringid, "error": error.clone().unwrap_or_else(|| "The representative stopped.".into())});
            fx.ting |= ting::enqueue(tx, &silicon, "ring.representative.failed", &format!("rep-failed:{ringid}:{silicon_uuid}:{}", now_ms()), payload, ting::PRIORITY_INCOMING)?;
        }
        fx.call_updated = true;
        Ok(())
    });
    if let Err(error) = result {
        tracing::error!(%error, "could not store representative status");
        return;
    }
    drop(_guard);
    fx.apply(app, ringid).await;
}

// ---- account deletion ----------------------------------------------------------------------------

/// Takes an account whose Ring account is being deleted off every live call: it leaves the calls
/// it is on, invitations to or from it stop ringing, and a voicemail being left for it ends.
pub async fn withdraw(app: &AppState, uuid: &str) -> ApiResult<()> {
    let ringids: Vec<String> = app.db.with(|c| {
        let mut stmt = c.prepare(
            "SELECT ringid FROM calls WHERE state != 'ended' AND (created_by = ?1
               OR ringid IN (SELECT ringid FROM participants WHERE account_uuid = ?1)
               OR ringid IN (SELECT ringid FROM invitations WHERE to_uuid = ?1 OR from_uuid = ?1))",
        )?;
        let rows = stmt.query_map([uuid], |r| r.get(0))?;
        rows.collect()
    })?;
    for ringid in ringids {
        let _guard = app.call_locks.lock(&ringid).await;
        let Some(data) = app.db.with(|c| load(c, &ringid))? else {
            continue;
        };
        if data.call.state == CallState::Ended {
            continue;
        }
        let mut fx = Effects::default();
        app.db.tx(|tx| withdraw_tx(app, tx, &data, uuid, &mut fx))?;
        drop(_guard);
        fx.apply(app, &ringid).await;
    }
    Ok(())
}

fn withdraw_tx(
    app: &AppState,
    tx: &Transaction<'_>,
    data: &CallData,
    uuid: &str,
    fx: &mut Effects,
) -> ApiResult<()> {
    let ringid = data.call.ringid.as_str();
    let mut stopped = false;
    for inv in data
        .ringing()
        .filter(|i| i.to_uuid == uuid || i.from_uuid == uuid)
    {
        set_invitation_state(
            tx,
            &inv.invitation_id,
            InvitationState::Cancelled,
            None,
            None,
        )?;
        fx.stop(inv, RingingStoppedReason::Cancelled, None);
        app.timers.cancel(&format!("expiry:{}", inv.invitation_id));
        stopped = true;
    }
    let data = load(tx, ringid)?.ok_or_else(ApiError::internal)?;
    if data.participant(uuid).is_some_and(|p| p.joined()) {
        return leave_tx(app, tx, &data, uuid, fx);
    }
    let callee = data.is_direct() && data.invitations.first().is_some_and(|i| i.to_uuid == uuid);
    match data.call.state {
        // Nobody would receive the message.
        CallState::Voicemail if callee => {
            let reason = data
                .call
                .voicemail_reason
                .as_deref()
                .and_then(super::enum_from::<VoicemailReason>)
                .unwrap_or(VoicemailReason::Missed);
            end_call_tx(app, tx, &data, voicemail_reason_to_end(reason), fx)?;
        }
        // Like a decline with voicemail off.
        CallState::Ringing if stopped && data.ringing().next().is_none() => {
            end_call_tx(app, tx, &data, EndReason::Declined, fx)?;
        }
        CallState::Active if stopped => continuation(app, tx, &data, fx)?,
        _ => {}
    }
    fx.call_updated |= stopped;
    Ok(())
}

/// On startup no call can still be live: end each leftover one and delete its room.
pub async fn end_leftover_calls(app: &AppState) {
    let ringids: Vec<String> = app
        .db
        .with(|c| {
            let mut stmt = c.prepare("SELECT ringid FROM calls WHERE state != 'ended'")?;
            let rows = stmt.query_map([], |r| r.get(0))?;
            rows.collect()
        })
        .unwrap_or_default();
    for ringid in ringids {
        let _guard = app.call_locks.lock(&ringid).await;
        let Ok(Some(data)) = app.db.with(|c| load(c, &ringid)) else {
            continue;
        };
        let mut fx = Effects::default();
        if let Err(error) = app
            .db
            .tx(|tx| end_call_tx(app, tx, &data, EndReason::ServerRestart, &mut fx))
        {
            tracing::error!(%error, "could not end leftover call");
            continue;
        }
        drop(_guard);
        fx.apply(app, &ringid).await;
    }
    // No call is live now, so no silicon can still be reserved (a crash mid-create can leave one).
    if let Err(error) = app
        .db
        .with(|c| c.execute("DELETE FROM silicon_reservations", []))
    {
        tracing::error!(%error, "could not clear silicon reservations");
    }
}

/// Silicon caller leaves a text voicemail (`POST /calls/{ringid}/voicemail`).
pub async fn leave_text_voicemail(
    app: &AppState,
    auth: &Auth,
    ringid: &str,
    text: String,
) -> ApiResult<ring_proto::Voicemail> {
    if !auth.is_silicon() {
        return Err(ApiError::forbidden(
            "Carbons leave voicemail by speaking after the beep.",
        ));
    }
    let text = text.trim().to_string();
    if text.is_empty() {
        return Err(ApiError::invalid("text is required."));
    }
    check_len("text", &text, limits::VOICEMAIL_TEXT)?;
    let uuid = auth.uuid().to_string();
    let _guard = app.call_locks.lock(ringid).await;
    let data = load_for(app, ringid, &uuid)?;
    if data.call.created_by != uuid {
        return Err(ApiError::forbidden(
            "Only the caller can leave a voicemail.",
        ));
    }
    if data.call.state != CallState::Voicemail {
        return Err(if data.call.state == CallState::Ended {
            ApiError::call_ended()
        } else {
            ApiError::conflict("This call is not in voicemail.")
        });
    }
    if data.call.voicemail_id.is_some() {
        return Err(ApiError::conflict(
            "You already left a voicemail on this call.",
        ));
    }
    let reason = data
        .call
        .voicemail_reason
        .as_deref()
        .and_then(super::enum_from::<VoicemailReason>)
        .unwrap_or(VoicemailReason::Missed);
    let callee_uuid = data
        .invitations
        .first()
        .map(|i| i.to_uuid.clone())
        .ok_or_else(ApiError::internal)?;
    let voicemail_id = new_id(ring_proto::prefix::VOICEMAIL);
    let voice = app.db.with(|c| settings::load(c, &uuid))?.settings.voice;
    let mut fx = Effects::default();
    let left = app.db.tx(|tx| -> rusqlite::Result<bool> {
        // The callee began deleting its Ring account after the call went to voicemail: it takes
        // no voicemail, so the call just ends.
        if crate::deletion::gone(tx, &callee_uuid)? {
            end_call_tx(app, tx, &data, voicemail_reason_to_end(reason), &mut fx)?;
            return Ok(false);
        }
        let now = now_ms();
        tx.execute(
            "INSERT INTO voicemails (voicemail_id, ringid, from_uuid, to_uuid, reason, decline_reason, created_at, audio_status, transcript,
               transcript_status, text) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'processing', ?8, 'ready', ?8)",
            params![voicemail_id, ringid, uuid, callee_uuid, reason.as_str(), data.call.decline_reason, now, text],
        )?;
        tx.execute("UPDATE calls SET voicemail_id = ?2 WHERE ringid = ?1", params![ringid, voicemail_id])?;
        let entry = append(tx, ringid, EntryKind::Voicemail, Some(&data.account(&uuid)), None, json!({"voicemail_id": voicemail_id}), None)?;
        fx.entry(entry, None);
        fx.media.push(ToMedia::VoicemailSynthesize {
            voicemail_id: voicemail_id.clone(),
            text: text.clone(),
            voice: voice.clone(),
            s3_key: app.config.s3_key("voicemail", &voicemail_id, "wav"),
        });
        let after = load(tx, ringid)?.ok_or(rusqlite::Error::QueryReturnedNoRows)?;
        end_call_tx(app, tx, &after, voicemail_reason_to_end(reason), &mut fx)?;
        fx.ting |= crate::voicemail::deliver_tx(tx, &voicemail_id)?;
        Ok(true)
    })?;
    drop(_guard);
    fx.apply(app, ringid).await;
    if !left {
        return Err(ApiError::call_ended());
    }
    crate::voicemail::announce(app, &voicemail_id, true);
    let vm = app
        .db
        .with(|c| crate::voicemail::load(c, &voicemail_id))?
        .ok_or_else(ApiError::internal)?;
    Ok(vm)
}
