//! Calls: stored rows, rendering to the contract's `Call`, and the side effects a state change
//! produces (events, pushes, media commands), applied after the transaction commits.

pub mod api;
pub mod engine;

use std::collections::{HashMap, HashSet};

use ring_proto::internal::{RosterMember, ToMedia};
use ring_proto::{
    Account, AccountKind, Call, CallState, DeviceRef, Direction, EndReason, Event, Invitation,
    InvitationState, Participant, ParticipantState, RecordingStatus, RecordingSummary,
    RepresentativeInfo, RepresentativeStatus, RingingStoppedReason, TranscriptEntry,
};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::accounts;
use crate::app::AppState;
use crate::realtime::frame;
use crate::util::{opt_time, time};

pub fn enum_from<T: DeserializeOwned>(s: &str) -> Option<T> {
    serde_json::from_value(serde_json::Value::String(s.to_string())).ok()
}

pub fn enum_str<T: Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_default()
}

#[derive(Clone, Debug)]
pub struct CallRow {
    pub ringid: String,
    pub state: CallState,
    pub created_by: String,
    pub created_at: i64,
    pub answered_at: Option<i64>,
    pub ended_at: Option<i64>,
    pub end_reason: Option<EndReason>,
    pub voicemail_id: Option<String>,
    pub voicemail_reason: Option<String>,
    pub decline_reason: Option<String>,
    pub recording_status: RecordingStatus,
    pub recording_duration_ms: Option<i64>,
    pub recording_key: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ParticipantRow {
    pub account_uuid: String,
    pub state: ParticipantState,
    pub joined_at: i64,
    pub left_at: Option<i64>,
    pub invited_by: Option<String>,
    pub device_id: Option<String>,
    pub media_identity: Option<String>,
    pub rep_status: Option<RepresentativeStatus>,
    pub rep_voice: Option<String>,
    pub rep_context: Option<String>,
    pub rep_start: Option<String>,
    pub ting_seq: i64,
}

impl ParticipantRow {
    pub fn joined(&self) -> bool {
        self.state == ParticipantState::Joined
    }
}

#[derive(Clone, Debug)]
pub struct InvitationRow {
    pub invitation_id: String,
    pub ringid: String,
    pub from_uuid: String,
    pub to_uuid: String,
    pub state: InvitationState,
    pub reason: Option<String>,
    pub created_at: i64,
    pub expires_at: i64,
    pub answered_at: Option<i64>,
    pub silenced: bool,
    pub call_uuid: String,
}

/// A call with everything needed to render or change it.
#[derive(Clone, Debug)]
pub struct CallData {
    pub call: CallRow,
    pub participants: Vec<ParticipantRow>,
    pub invitations: Vec<InvitationRow>,
    pub accounts: HashMap<String, Account>,
}

impl CallData {
    pub fn participant(&self, uuid: &str) -> Option<&ParticipantRow> {
        self.participants.iter().find(|p| p.account_uuid == uuid)
    }

    pub fn joined(&self) -> impl Iterator<Item = &ParticipantRow> {
        self.participants.iter().filter(|p| p.joined())
    }

    pub fn ringing(&self) -> impl Iterator<Item = &InvitationRow> {
        self.invitations
            .iter()
            .filter(|i| i.state == InvitationState::Ringing)
    }

    pub fn account(&self, uuid: &str) -> Account {
        self.accounts
            .get(uuid)
            .cloned()
            .unwrap_or_else(|| accounts::placeholder(uuid))
    }

    pub fn is_silicon(&self, uuid: &str) -> bool {
        self.account(uuid).kind == AccountKind::Silicon
    }

    /// Everyone who should hear about changes: participants (any state), invitees and the creator.
    pub fn audience(&self) -> Vec<String> {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        let all = std::iter::once(self.call.created_by.as_str())
            .chain(self.participants.iter().map(|p| p.account_uuid.as_str()))
            .chain(self.invitations.iter().map(|i| i.to_uuid.as_str()));
        for uuid in all {
            if seen.insert(uuid) {
                out.push(uuid.to_string());
            }
        }
        out
    }

    pub fn can_view(&self, uuid: &str) -> bool {
        self.call.created_by == uuid
            || self.participant(uuid).is_some()
            || self.invitations.iter().any(|i| i.to_uuid == uuid)
    }

    pub fn roster(&self) -> Vec<RosterMember> {
        self.joined()
            .map(|p| {
                let account = self.account(&p.account_uuid);
                let identity = if account.kind == AccountKind::Silicon {
                    Some(crate::livekit::representative_identity(&account.uuid))
                } else {
                    p.media_identity.clone()
                };
                RosterMember {
                    uuid: account.uuid,
                    id: account.id,
                    display_name: account.display_name,
                    kind: account.kind,
                    identity,
                }
            })
            .collect()
    }

    /// A direct call has exactly one invitation: the callee of `POST /v1/calls`.
    pub fn is_direct(&self) -> bool {
        self.invitations.len() == 1
    }
}

const CALL_COLUMNS: &str = "ringid, state, created_by, created_at, answered_at, ended_at, end_reason, voicemail_id, voicemail_reason, \
     decline_reason, recording_status, recording_duration_ms, recording_key";

fn row_to_call(r: &rusqlite::Row<'_>) -> rusqlite::Result<CallRow> {
    let state: String = r.get(1)?;
    let end_reason: Option<String> = r.get(6)?;
    let recording: String = r.get(10)?;
    Ok(CallRow {
        ringid: r.get(0)?,
        state: CallState::parse(&state).unwrap_or(CallState::Ended),
        created_by: r.get(2)?,
        created_at: r.get(3)?,
        answered_at: r.get(4)?,
        ended_at: r.get(5)?,
        end_reason: end_reason.as_deref().and_then(enum_from),
        voicemail_id: r.get(7)?,
        voicemail_reason: r.get(8)?,
        decline_reason: r.get(9)?,
        recording_status: enum_from(&recording).unwrap_or(RecordingStatus::None),
        recording_duration_ms: r.get(11)?,
        recording_key: r.get(12)?,
    })
}

pub fn load_call_row(conn: &Connection, ringid: &str) -> rusqlite::Result<Option<CallRow>> {
    conn.query_row(
        &format!("SELECT {CALL_COLUMNS} FROM calls WHERE ringid = ?1"),
        [ringid],
        row_to_call,
    )
    .optional()
}

pub fn load(conn: &Connection, ringid: &str) -> rusqlite::Result<Option<CallData>> {
    let Some(call) = load_call_row(conn, ringid)? else {
        return Ok(None);
    };
    let mut stmt = conn.prepare(
        "SELECT account_uuid, state, joined_at, left_at, invited_by, device_id, media_identity, rep_status, rep_voice, rep_context,
           rep_start, ting_seq FROM participants WHERE ringid = ?1 ORDER BY joined_at, account_uuid",
    )?;
    let participants = stmt
        .query_map([ringid], |r| {
            let state: String = r.get(1)?;
            let rep: Option<String> = r.get(7)?;
            Ok(ParticipantRow {
                account_uuid: r.get(0)?,
                state: if state == "joined" {
                    ParticipantState::Joined
                } else {
                    ParticipantState::Left
                },
                joined_at: r.get(2)?,
                left_at: r.get(3)?,
                invited_by: r.get(4)?,
                device_id: r.get(5)?,
                media_identity: r.get(6)?,
                rep_status: rep.as_deref().and_then(enum_from),
                rep_voice: r.get(8)?,
                rep_context: r.get(9)?,
                rep_start: r.get(10)?,
                ting_seq: r.get(11)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut stmt = conn.prepare(
        "SELECT invitation_id, ringid, from_uuid, to_uuid, state, reason, created_at, expires_at, answered_at, silenced, call_uuid
         FROM invitations WHERE ringid = ?1 ORDER BY created_at, invitation_id",
    )?;
    let invitations = stmt
        .query_map([ringid], row_to_invitation)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let uuids: Vec<&str> =
        std::iter::once(call.created_by.as_str())
            .chain(participants.iter().flat_map(|p| {
                std::iter::once(p.account_uuid.as_str()).chain(p.invited_by.as_deref())
            }))
            .chain(
                invitations
                    .iter()
                    .flat_map(|i| [i.from_uuid.as_str(), i.to_uuid.as_str()]),
            )
            .collect();
    let accounts = accounts::load_many(conn, uuids)?;
    Ok(Some(CallData {
        call,
        participants,
        invitations,
        accounts,
    }))
}

pub fn row_to_invitation(r: &rusqlite::Row<'_>) -> rusqlite::Result<InvitationRow> {
    let state: String = r.get(4)?;
    Ok(InvitationRow {
        invitation_id: r.get(0)?,
        ringid: r.get(1)?,
        from_uuid: r.get(2)?,
        to_uuid: r.get(3)?,
        state: enum_from(&state).unwrap_or(InvitationState::Cancelled),
        reason: r.get(5)?,
        created_at: r.get(6)?,
        expires_at: r.get(7)?,
        answered_at: r.get(8)?,
        silenced: r.get::<_, i64>(9)? != 0,
        call_uuid: r.get(10)?,
    })
}

pub fn render_invitation(data: &CallData, inv: &InvitationRow) -> Invitation {
    Invitation {
        invitation_id: inv.invitation_id.clone(),
        ringid: inv.ringid.clone(),
        from: data.account(&inv.from_uuid),
        to: data.account(&inv.to_uuid),
        state: inv.state,
        reason: inv.reason.clone(),
        created_at: time(inv.created_at),
        expires_at: time(inv.expires_at),
        answered_at: opt_time(inv.answered_at),
        silenced: inv.silenced,
        call_uuid: inv.call_uuid.clone(),
    }
}

/// Renders the call as seen by `viewer` (only `direction` depends on the viewer).
pub fn render(conn: &Connection, data: &CallData, viewer: &str) -> rusqlite::Result<Call> {
    let mut participants = Vec::with_capacity(data.participants.len());
    for p in &data.participants {
        let account = data.account(&p.account_uuid);
        let device = match &p.device_id {
            Some(device_id) => {
                let row = crate::sessions::by_device(conn, device_id)?;
                Some(DeviceRef {
                    device_id: device_id.clone(),
                    name: row.as_ref().and_then(|r| r.name.clone()),
                    platform: row.as_ref().and_then(|r| r.platform),
                })
            }
            None => None,
        };
        let representative = (account.kind == AccountKind::Silicon).then(|| RepresentativeInfo {
            status: p.rep_status.unwrap_or(if p.joined() {
                RepresentativeStatus::Starting
            } else {
                RepresentativeStatus::Ended
            }),
            voice: p
                .rep_voice
                .clone()
                .unwrap_or_else(|| ring_proto::DEFAULT_VOICE.to_string()),
        });
        let media_identity = if account.kind == AccountKind::Silicon {
            p.rep_status
                .filter(|s| {
                    matches!(
                        s,
                        RepresentativeStatus::Starting | RepresentativeStatus::Live
                    )
                })
                .map(|_| crate::livekit::representative_identity(&account.uuid))
        } else {
            p.media_identity.clone()
        };
        participants.push(Participant {
            account,
            state: p.state,
            joined_at: time(p.joined_at),
            left_at: opt_time(p.left_at),
            invited_by: p.invited_by.as_deref().map(|u| data.account(u)),
            device,
            representative,
            media_identity: media_identity.filter(|_| p.joined()),
        });
    }
    Ok(Call {
        ringid: data.call.ringid.clone(),
        state: data.call.state,
        created_at: time(data.call.created_at),
        answered_at: opt_time(data.call.answered_at),
        ended_at: opt_time(data.call.ended_at),
        end_reason: data.call.end_reason,
        created_by: data.account(&data.call.created_by),
        direction: if data.call.created_by == viewer {
            Direction::Outgoing
        } else {
            Direction::Incoming
        },
        participants,
        invitations: data
            .invitations
            .iter()
            .map(|i| render_invitation(data, i))
            .collect(),
        recording: RecordingSummary {
            status: data.call.recording_status,
            duration_ms: data.call.recording_duration_ms.map(|d| d.max(0) as u64),
        },
        voicemail_id: data.call.voicemail_id.clone(),
    })
}

pub fn load_rendered(
    app: &AppState,
    ringid: &str,
    viewer: &str,
) -> rusqlite::Result<Option<(CallData, Call)>> {
    app.db.with(|c| {
        let Some(data) = load(c, ringid)? else {
            return Ok(None);
        };
        let call = render(c, &data, viewer)?;
        Ok(Some((data, call)))
    })
}

/// Who should receive a transcript entry written at `at`: joined participants, plus anyone who
/// left in the same change (so the `left`/`call_ended` entries reach them), and only the owner
/// for private kinds.
pub fn entry_recipients(data: &CallData, owner: Option<&str>, at: i64) -> Vec<String> {
    data.participants
        .iter()
        .filter(|p| p.joined() || p.left_at.is_some_and(|left| left >= at - 5_000))
        .map(|p| p.account_uuid.clone())
        .filter(|uuid| owner.is_none_or(|o| o == uuid))
        .collect()
}

/// Side effects of one state change, applied after the transaction commits.
#[derive(Default)]
pub struct Effects {
    pub call_updated: bool,
    pub call_ended: bool,
    pub entries: Vec<(TranscriptEntry, Option<String>)>,
    pub incoming: Vec<String>,
    pub stopped: Vec<(InvitationRow, RingingStoppedReason, Option<String>)>,
    pub missed: Vec<InvitationRow>,
    pub events: Vec<(String, Event)>,
    pub media: Vec<ToMedia>,
    pub delete_room: bool,
    pub ting: bool,
    pub schedule_expiry: Vec<InvitationRow>,
}

impl Effects {
    pub fn entry(&mut self, entry: TranscriptEntry, owner: Option<String>) {
        self.entries.push((entry, owner));
    }

    pub fn stop(
        &mut self,
        inv: &InvitationRow,
        reason: RingingStoppedReason,
        acting_device: Option<&str>,
    ) {
        self.stopped
            .push((inv.clone(), reason, acting_device.map(String::from)));
    }

    pub async fn apply(self, app: &AppState, ringid: &str) {
        let data = match app.db.with(|c| load(c, ringid)) {
            Ok(Some(data)) => data,
            Ok(None) => return,
            Err(error) => {
                tracing::error!(%error, "could not load call for effects");
                return;
            }
        };
        for (entry, owner) in &self.entries {
            let frame = frame(&Event::TranscriptEntry {
                ringid: ringid.to_string(),
                entry: entry.clone(),
            });
            for uuid in entry_recipients(&data, owner.as_deref(), entry.at.millis()) {
                app.hub.send_frame(&uuid, frame.clone());
            }
        }
        for invitation_id in &self.incoming {
            if let Some(inv) = data
                .invitations
                .iter()
                .find(|i| &i.invitation_id == invitation_id)
            {
                ring_invitee(app, &data, inv);
            }
        }
        for (uuid, event) in &self.events {
            app.hub.send_account(uuid, event);
        }
        if self.call_updated || self.call_ended {
            broadcast_call(app, &data, self.call_ended);
        }
        for (inv, reason, acting_device) in &self.stopped {
            let event = Event::CallRingingStopped {
                ringid: ringid.to_string(),
                invitation_id: inv.invitation_id.clone(),
                reason: *reason,
            };
            app.hub.send_account(&inv.to_uuid, &event);
            if !data.is_silicon(&inv.to_uuid) {
                crate::push::cancel(app, inv, *reason, acting_device.as_deref());
            }
        }
        for inv in &self.missed {
            if !data.is_silicon(&inv.to_uuid) {
                crate::push::missed(app, &data, inv);
            }
        }
        for command in self.media {
            app.media.send(command);
        }
        for inv in &self.schedule_expiry {
            engine::schedule_expiry(app, inv);
        }
        if self.delete_room {
            let app = app.clone();
            let ringid = ringid.to_string();
            tokio::spawn(async move {
                for attempt in 0..3u64 {
                    match app.livekit.delete_room(&ringid).await {
                        Ok(()) => return,
                        Err(error) if error.contains("not_found") || error.contains("404") => {
                            return;
                        }
                        Err(error) => {
                            tracing::warn!(%error, attempt, "could not delete LiveKit room");
                            tokio::time::sleep(std::time::Duration::from_secs(2 << attempt)).await;
                        }
                    }
                }
            });
        }
        if self.ting {
            app.ting.wake();
        }
    }
}

/// Sends `call.updated` (or `call.ended`) to everyone involved, each with their own direction.
pub fn broadcast_call(app: &AppState, data: &CallData, ended: bool) {
    let rendered = match app.db.with(|c| render(c, data, &data.call.created_by)) {
        Ok(call) => call,
        Err(error) => {
            tracing::error!(%error, "could not render call");
            return;
        }
    };
    let make = |call: Call| {
        if ended {
            Event::CallEnded { call }
        } else {
            Event::CallUpdated { call }
        }
    };
    let outgoing = frame(&make(rendered.clone()));
    let mut incoming_call = rendered;
    incoming_call.direction = Direction::Incoming;
    let incoming = frame(&make(incoming_call));
    for uuid in data.audience() {
        let f = if uuid == data.call.created_by {
            outgoing.clone()
        } else {
            incoming.clone()
        };
        app.hub.send_frame(&uuid, f);
    }
}

/// Rings one invitee: `call.incoming` on every device, push to devices that can ring.
fn ring_invitee(app: &AppState, data: &CallData, inv: &InvitationRow) {
    let call = match app.db.with(|c| render(c, data, &inv.to_uuid)) {
        Ok(call) => call,
        Err(error) => {
            tracing::error!(%error, "could not render call");
            return;
        }
    };
    let invitation = render_invitation(data, inv);
    app.hub
        .send_account(&inv.to_uuid, &Event::CallIncoming { call, invitation });
    if !data.is_silicon(&inv.to_uuid) {
        crate::push::incoming(app, data, inv);
    }
}

/// Inserts an invitation row.
pub fn insert_invitation(conn: &Connection, inv: &InvitationRow) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO invitations (invitation_id, ringid, from_uuid, to_uuid, state, reason, created_at, expires_at, answered_at, silenced, call_uuid)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            inv.invitation_id,
            inv.ringid,
            inv.from_uuid,
            inv.to_uuid,
            inv.state.as_str(),
            inv.reason,
            inv.created_at,
            inv.expires_at,
            inv.answered_at,
            inv.silenced as i64,
            inv.call_uuid
        ],
    )?;
    Ok(())
}

pub fn set_invitation_state(
    conn: &Connection,
    invitation_id: &str,
    state: InvitationState,
    reason: Option<&str>,
    acted_device: Option<&str>,
) -> rusqlite::Result<bool> {
    let now = crate::util::now_ms();
    let answered =
        matches!(state, InvitationState::Accepted | InvitationState::Declined).then_some(now);
    let changed = conn.execute(
        "UPDATE invitations SET state = ?2, reason = COALESCE(?3, reason), answered_at = COALESCE(?4, answered_at),
           acted_device_id = COALESCE(?5, acted_device_id) WHERE invitation_id = ?1 AND state = 'ringing'",
        params![invitation_id, state.as_str(), reason, answered, acted_device],
    )?;
    Ok(changed == 1)
}
