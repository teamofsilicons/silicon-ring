//! Messages on the loopback WebSocket between ring-server and ring-media (`GET /internal/media`).
//! Not part of the public API. See `udd/api.md` § Internal.

use serde::{Deserialize, Serialize};

use crate::{Account, MessageKind};

/// One roster member as seen by a representative.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RosterMember {
    pub uuid: String,
    pub id: String,
    pub display_name: Option<String>,
    pub kind: crate::AccountKind,
    /// Current LiveKit identity carrying this member's audio (authoritative device for carbons,
    /// `rep:{uuid}` for silicons). `None` when no media is connected yet.
    pub identity: Option<String>,
}

/// ring-server → ring-media.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ToMedia {
    /// Join `ringid` as the representative of `silicon`, speaking `start` first.
    #[serde(rename = "representative.start")]
    RepresentativeStart {
        ringid: String,
        silicon: Account,
        voice: String,
        /// Effective context (already merged and validated, ≤ 400 code points).
        context: String,
        start: Option<String>,
        roster: Vec<RosterMember>,
        /// LiveKit join token for identity `rep:{silicon.uuid}`.
        token: String,
    },
    #[serde(rename = "representative.stop")]
    RepresentativeStop { ringid: String, silicon_uuid: String },
    #[serde(rename = "representative.message")]
    RepresentativeMessage {
        ringid: String,
        silicon_uuid: String,
        message_id: String,
        kind: MessageKind,
        text: String,
        delegation_id: Option<String>,
    },
    /// Roster changed; `joined`/`left` set when it was a join/leave (announce to representatives).
    #[serde(rename = "roster.changed")]
    RosterChanged { ringid: String, roster: Vec<RosterMember>, joined: Option<Account>, left: Option<Account> },
    /// The authoritative LiveKit identity for a carbon changed (handoff). Not announced.
    #[serde(rename = "authority.changed")]
    AuthorityChanged { ringid: String, account_uuid: String, identity: String },
    /// Start recording and live STT for `ringid` (hidden identity `ring:recorder`).
    #[serde(rename = "recorder.start")]
    RecorderStart { ringid: String, token: String, roster: Vec<RosterMember>, s3_key: String },
    #[serde(rename = "recorder.stop")]
    RecorderStop { ringid: String },
    /// Play greeting + beep to `caller_identity` in `ringid` and record them (identity `ring:voicemail`).
    #[serde(rename = "voicemail.start")]
    VoicemailStart {
        ringid: String,
        voicemail_id: String,
        caller_identity: String,
        greeting_text: String,
        voice: String,
        max_seconds: u64,
        token: String,
        s3_key: String,
    },
    /// Stop recording a voicemail now (caller hung up); finish and upload.
    #[serde(rename = "voicemail.stop")]
    VoicemailStop { voicemail_id: String },
    /// Synthesize a silicon's text voicemail in `voice` and upload it.
    #[serde(rename = "voicemail.synthesize")]
    VoicemailSynthesize { voicemail_id: String, text: String, voice: String, s3_key: String },
    /// The call is over: leave the room, stop everything for it.
    #[serde(rename = "call.closed")]
    CallClosed { ringid: String },
}

/// ring-media → ring-server.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum FromMedia {
    /// Sent once after connecting; lists rooms ring-media is still serving (after a reconnect).
    #[serde(rename = "hello")]
    Hello { version: String, active_rings: Vec<String> },
    #[serde(rename = "speech.partial")]
    SpeechPartial { ringid: String, account_uuid: String, segment_id: String, text: String },
    #[serde(rename = "speech.final")]
    SpeechFinal { ringid: String, account_uuid: String, segment_id: String, text: String, offset_ms: Option<i64> },
    #[serde(rename = "delegation.created")]
    DelegationCreated { ringid: String, silicon_uuid: String, delegation_id: String, request: Option<String> },
    #[serde(rename = "representative.status")]
    RepresentativeStatus {
        ringid: String,
        silicon_uuid: String,
        status: crate::RepresentativeStatus,
        error: Option<String>,
    },
    #[serde(rename = "message.delivered")]
    MessageDelivered { ringid: String, message_id: String },
    #[serde(rename = "recording.finished")]
    RecordingFinished { ringid: String, s3_key: Option<String>, duration_ms: Option<u64>, error: Option<String> },
    #[serde(rename = "voicemail.finished")]
    VoicemailFinished {
        voicemail_id: String,
        s3_key: Option<String>,
        duration_ms: Option<u64>,
        transcript: Option<String>,
        error: Option<String>,
    },
}
