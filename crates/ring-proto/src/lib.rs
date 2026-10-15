//! Wire types for the Silicon Ring API v1. `udd/api.md` is the contract; these types mirror it exactly.
//!
//! Every type serializes to the JSON shown in the contract. Optional fields that the contract
//! shows as `null` serialize as `null` (never omitted) so that every client sees a stable shape.

use std::fmt;

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub mod internal;

/// API version spoken by this crate.
pub const API_VERSION: &str = "v1";

/// Text limits, counted in Unicode code points.
pub mod limits {
    pub const CONTEXT: usize = 400;
    pub const START: usize = 100;
    pub const MESSAGE: usize = 160;
    pub const DECLINE_REASON: usize = 200;
    pub const VOICEMAIL_TEXT: usize = 600;
    pub const GREETING: usize = 300;
    pub const DEVICE_NAME: usize = 80;
    /// Separator used when merging default and supplied context.
    pub const CONTEXT_SEPARATOR: &str = "\n\n";
}

/// Ringing timeout for every invitation.
pub const RINGING_SECONDS: u64 = 60;
/// Maximum recorded voicemail length.
pub const VOICEMAIL_MAX_SECONDS: u64 = 120;

/// Counts Unicode code points, which is how every limit is measured.
pub fn text_len(text: &str) -> usize {
    text.chars().count()
}

// ---------------------------------------------------------------------------------------------
// Time
// ---------------------------------------------------------------------------------------------

/// A UTC timestamp serialized as RFC 3339 with exactly three fractional digits
/// (`2026-10-15T09:30:00.000Z`). Accepts any RFC 3339 timestamp when parsing.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Time(pub DateTime<Utc>);

impl Time {
    pub fn now() -> Self {
        Time(Utc::now())
    }

    pub fn from_millis(ms: i64) -> Option<Self> {
        DateTime::<Utc>::from_timestamp_millis(ms).map(Time)
    }

    pub fn millis(&self) -> i64 {
        self.0.timestamp_millis()
    }

    pub fn parse(s: &str) -> Option<Self> {
        DateTime::parse_from_rfc3339(s).ok().map(|d| Time(d.with_timezone(&Utc)))
    }

    pub fn to_rfc3339(&self) -> String {
        self.0.to_rfc3339_opts(SecondsFormat::Millis, true)
    }
}

impl fmt::Debug for Time {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_rfc3339())
    }
}

impl fmt::Display for Time {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_rfc3339())
    }
}

impl From<DateTime<Utc>> for Time {
    fn from(d: DateTime<Utc>) -> Self {
        Time(d)
    }
}

impl Serialize for Time {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_rfc3339())
    }
}

impl<'de> Deserialize<'de> for Time {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Time::parse(&s).ok_or_else(|| serde::de::Error::custom(format!("invalid RFC 3339 timestamp: {s}")))
    }
}

// ---------------------------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------------------------

/// `{"error": {...}}` body of every non-2xx response.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ErrorBody {
    pub error: ApiError,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ApiError {
    pub code: String,
    pub message: String,
    #[serde(default)]
    pub hint: Option<String>,
    #[serde(default)]
    pub retryable: bool,
    #[serde(default = "empty_object")]
    pub details: serde_json::Value,
}

fn empty_object() -> serde_json::Value {
    serde_json::Value::Object(Default::default())
}

/// Stable error codes from the contract.
pub mod codes {
    pub const INVALID_INPUT: &str = "invalid_input";
    pub const TEXT_TOO_LONG: &str = "text_too_long";
    pub const CONTEXT_REQUIRED: &str = "context_required";
    pub const UNAUTHENTICATED: &str = "unauthenticated";
    pub const SESSION_EXPIRED: &str = "session_expired";
    pub const FORBIDDEN: &str = "forbidden";
    pub const CSRF: &str = "csrf";
    pub const CARBON_USE_APP: &str = "carbon_use_app";
    pub const SILICON_USE_CLI: &str = "silicon_use_cli";
    pub const NOT_PARTICIPANT: &str = "not_participant";
    pub const NOT_FOUND: &str = "not_found";
    pub const ACCOUNT_NOT_FOUND: &str = "account_not_found";
    pub const CONFLICT: &str = "conflict";
    pub const INVITATION_NOT_RINGING: &str = "invitation_not_ringing";
    pub const ALREADY_IN_CALL: &str = "already_in_call";
    pub const CONTEXT_APPROVAL_REQUIRED: &str = "context_approval_required";
    pub const IDEMPOTENCY_CONFLICT: &str = "idempotency_conflict";
    pub const CALL_ENDED: &str = "call_ended";
    pub const INVITATION_EXPIRED: &str = "invitation_expired";
    pub const RATE_LIMITED: &str = "rate_limited";
    pub const DEPENDENCY_UNAVAILABLE: &str = "dependency_unavailable";
}

// ---------------------------------------------------------------------------------------------
// Accounts and devices
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AccountKind {
    Carbon,
    Silicon,
}

impl AccountKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            AccountKind::Carbon => "carbon",
            AccountKind::Silicon => "silicon",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    pub uuid: String,
    /// `c:handle` or `si:handle`.
    pub id: String,
    pub kind: AccountKind,
    pub display_name: Option<String>,
    pub pfp_url: Option<String>,
}

impl Account {
    /// Display name if known, otherwise the id.
    pub fn label(&self) -> &str {
        self.display_name.as_deref().filter(|s| !s.trim().is_empty()).unwrap_or(&self.id)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AccountLookup {
    pub account: Account,
    pub on_ring: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Ios,
    Ipados,
    Macos,
    Android,
    Windows,
    Linux,
    Web,
    Cli,
}

impl Platform {
    pub fn as_str(&self) -> &'static str {
        match self {
            Platform::Ios => "ios",
            Platform::Ipados => "ipados",
            Platform::Macos => "macos",
            Platform::Android => "android",
            Platform::Windows => "windows",
            Platform::Linux => "linux",
            Platform::Web => "web",
            Platform::Cli => "cli",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PushKind {
    ApnsVoip,
    Apns,
    Fcm,
    Webpush,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PushEnvironment {
    Sandbox,
    Production,
}

/// A push registration as sent by a client (`PUT /v1/devices/current`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PushRegistration {
    pub kind: PushKind,
    pub token: String,
    #[serde(default)]
    pub environment: Option<PushEnvironment>,
}

/// A push registration as returned by the server (token never returned).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PushInfo {
    pub kind: PushKind,
    pub environment: Option<PushEnvironment>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Device {
    pub device_id: String,
    pub name: Option<String>,
    pub platform: Option<Platform>,
    pub app_version: Option<String>,
    pub created_at: Time,
    pub last_seen_at: Option<Time>,
    pub online: bool,
    pub ring_enabled: bool,
    pub current: bool,
    #[serde(default)]
    pub push: Vec<PushInfo>,
}

/// `PUT /v1/devices/current`. All fields optional; `push` replaces this device's tokens.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DeviceUpdate {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<Platform>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ring_enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub push: Option<Vec<PushRegistration>>,
}

/// `PATCH /v1/devices/{device_id}`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DevicePatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ring_enabled: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Session {
    pub session_token: String,
    pub expires_at: Time,
    pub account: Account,
    pub device_id: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Me {
    pub account: Account,
    pub device: Device,
}

/// `POST /v1/auth/silicon`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SiliconLogin {
    pub slt: String,
    #[serde(default)]
    pub label: Option<String>,
}

/// `POST /v1/auth/carbon`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CarbonLogin {
    pub code: String,
    pub code_verifier: String,
    pub redirect_uri: String,
    #[serde(default)]
    pub label: Option<String>,
}

// ---------------------------------------------------------------------------------------------
// Meta
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Health {
    pub status: String,
    pub app: String,
    pub version: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Limits {
    pub context: usize,
    pub start: usize,
    pub message: usize,
    pub decline_reason: usize,
    pub voicemail_text: usize,
    pub greeting: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            context: limits::CONTEXT,
            start: limits::START,
            message: limits::MESSAGE,
            decline_reason: limits::DECLINE_REASON,
            voicemail_text: limits::VOICEMAIL_TEXT,
            greeting: limits::GREETING,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RedirectUris {
    pub native: String,
    pub loopback: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Voice {
    pub id: String,
    pub name: String,
    pub description: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Meta {
    pub app_id: String,
    pub version: String,
    pub api_version: String,
    pub accounts_url: String,
    pub web_url: String,
    pub redirect_uris: RedirectUris,
    pub limits: Limits,
    pub ringing_seconds: u64,
    pub voicemail_max_seconds: u64,
    pub voices: Vec<Voice>,
    pub default_voice: String,
    pub webpush_public_key: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VoiceList {
    pub items: Vec<Voice>,
    pub default: String,
}

/// The Natural Voices Ring offers for GPT-Live representatives and greetings.
pub fn natural_voices() -> Vec<Voice> {
    [
        ("gleam", "Gleam", "Bright and friendly"),
        ("ripple", "Ripple", "Light and playful"),
        ("vesper", "Vesper", "Calm and warm"),
        ("willow", "Willow", "Soft and gentle"),
        ("stone", "Stone", "Deep and steady"),
        ("meridian", "Meridian", "Clear and balanced"),
        ("bossa", "Bossa", "Relaxed and smooth"),
        ("tempo", "Tempo", "Energetic and quick"),
    ]
    .into_iter()
    .map(|(id, name, description)| Voice { id: id.into(), name: name.into(), description: description.into() })
    .collect()
}

pub const DEFAULT_VOICE: &str = "gleam";

// ---------------------------------------------------------------------------------------------
// Calls
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallState {
    Ringing,
    Active,
    Voicemail,
    Ended,
}

impl CallState {
    pub fn as_str(&self) -> &'static str {
        match self {
            CallState::Ringing => "ringing",
            CallState::Active => "active",
            CallState::Voicemail => "voicemail",
            CallState::Ended => "ended",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "ringing" => CallState::Ringing,
            "active" => CallState::Active,
            "voicemail" => CallState::Voicemail,
            "ended" => CallState::Ended,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndReason {
    Completed,
    Cancelled,
    Declined,
    Missed,
    Busy,
    Failed,
    ServerRestart,
}

impl EndReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            EndReason::Completed => "completed",
            EndReason::Cancelled => "cancelled",
            EndReason::Declined => "declined",
            EndReason::Missed => "missed",
            EndReason::Busy => "busy",
            EndReason::Failed => "failed",
            EndReason::ServerRestart => "server_restart",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Incoming,
    Outgoing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordingStatus {
    None,
    Recording,
    Processing,
    Ready,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RecordingSummary {
    pub status: RecordingStatus,
    pub duration_ms: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParticipantState {
    Joined,
    Left,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeviceRef {
    pub device_id: String,
    pub name: Option<String>,
    pub platform: Option<Platform>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepresentativeStatus {
    Starting,
    Live,
    Failed,
    Ended,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RepresentativeInfo {
    pub status: RepresentativeStatus,
    pub voice: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Participant {
    pub account: Account,
    pub state: ParticipantState,
    pub joined_at: Time,
    pub left_at: Option<Time>,
    pub invited_by: Option<Account>,
    pub device: Option<DeviceRef>,
    pub representative: Option<RepresentativeInfo>,
    pub media_identity: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InvitationState {
    Ringing,
    Accepted,
    Declined,
    Missed,
    Cancelled,
    Busy,
}

impl InvitationState {
    pub fn as_str(&self) -> &'static str {
        match self {
            InvitationState::Ringing => "ringing",
            InvitationState::Accepted => "accepted",
            InvitationState::Declined => "declined",
            InvitationState::Missed => "missed",
            InvitationState::Cancelled => "cancelled",
            InvitationState::Busy => "busy",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Invitation {
    pub invitation_id: String,
    pub ringid: String,
    pub from: Account,
    pub to: Account,
    pub state: InvitationState,
    pub reason: Option<String>,
    pub created_at: Time,
    pub expires_at: Time,
    pub answered_at: Option<Time>,
    pub silenced: bool,
    pub call_uuid: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Call {
    pub ringid: String,
    pub state: CallState,
    pub created_at: Time,
    pub answered_at: Option<Time>,
    pub ended_at: Option<Time>,
    pub end_reason: Option<EndReason>,
    pub created_by: Account,
    pub direction: Direction,
    pub participants: Vec<Participant>,
    pub invitations: Vec<Invitation>,
    pub recording: RecordingSummary,
    pub voicemail_id: Option<String>,
}

impl Call {
    /// Joined participants (not left).
    pub fn joined(&self) -> impl Iterator<Item = &Participant> {
        self.participants.iter().filter(|p| p.state == ParticipantState::Joined)
    }

    /// The ringing invitation addressed to `uuid`, if any.
    pub fn ringing_invitation_for(&self, uuid: &str) -> Option<&Invitation> {
        self.invitations.iter().find(|i| i.to.uuid == uuid && i.state == InvitationState::Ringing)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MediaGrant {
    pub url: String,
    pub token: String,
    pub room: String,
    pub identity: String,
    pub expires_at: Time,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextMode {
    Append,
    Prepend,
    Overwrite,
}

impl ContextMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            ContextMode::Append => "append",
            ContextMode::Prepend => "prepend",
            ContextMode::Overwrite => "overwrite",
        }
    }
}

/// `POST /v1/calls`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CreateCall {
    pub to: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_mode: Option<ContextMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CreatedCall {
    pub call: Call,
    pub invitation: Invitation,
    pub media: Option<MediaGrant>,
}

/// `POST /v1/calls/{ringid}/accept`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AcceptCall {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invitation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_mode: Option<ContextMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_id: Option<String>,
}

/// Response of accept and `POST /v1/calls/{ringid}/media`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CallWithMedia {
    pub call: Call,
    pub media: Option<MediaGrant>,
}

/// `POST /v1/calls/{ringid}/decline`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DeclineCall {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invitation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default)]
    pub give_no_reason: bool,
}

/// `POST /v1/calls/{ringid}/silence`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SilenceCall {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invitation_id: Option<String>,
}

/// `POST /v1/calls/{ringid}/invite`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InviteToCall {
    pub to: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CallEnvelope {
    pub call: Call,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InvitedCall {
    pub call: Call,
    pub invitation: Invitation,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Items<T> {
    pub items: Vec<T>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RecentPerson {
    pub account: Account,
    pub last_call_at: Time,
    pub last_ringid: String,
    pub calls: u64,
}

// ---------------------------------------------------------------------------------------------
// Transcript, delegation, messages
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    CallStarted,
    Answered,
    Joined,
    Left,
    Invited,
    Declined,
    Missed,
    Busy,
    Speech,
    Delegation,
    Thinking,
    Commentary,
    Voicemail,
    CallEnded,
    Note,
}

impl EntryKind {
    /// Private kinds are visible only to the silicon that owns them.
    pub fn is_private(&self) -> bool {
        matches!(self, EntryKind::Delegation | EntryKind::Thinking | EntryKind::Commentary)
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            EntryKind::CallStarted => "call_started",
            EntryKind::Answered => "answered",
            EntryKind::Joined => "joined",
            EntryKind::Left => "left",
            EntryKind::Invited => "invited",
            EntryKind::Declined => "declined",
            EntryKind::Missed => "missed",
            EntryKind::Busy => "busy",
            EntryKind::Speech => "speech",
            EntryKind::Delegation => "delegation",
            EntryKind::Thinking => "thinking",
            EntryKind::Commentary => "commentary",
            EntryKind::Voicemail => "voicemail",
            EntryKind::CallEnded => "call_ended",
            EntryKind::Note => "note",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TranscriptEntry {
    pub seq: u64,
    pub at: Time,
    pub offset_ms: Option<i64>,
    pub kind: EntryKind,
    pub actor: Option<Account>,
    pub text: Option<String>,
    #[serde(default = "empty_object")]
    pub data: serde_json::Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TranscriptPage {
    pub items: Vec<TranscriptEntry>,
    pub next_after_seq: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageKind {
    Thinking,
    Commentary,
}

impl MessageKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            MessageKind::Thinking => "thinking",
            MessageKind::Commentary => "commentary",
        }
    }
}

/// `POST /v1/calls/{ringid}/messages`. `delegation_id` is always serialized (JSON `null` when absent).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SendMessage {
    pub kind: MessageKind,
    pub text: String,
    pub delegation_id: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageStatus {
    Delivered,
    Queued,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SentMessage {
    pub message_id: String,
    pub seq: u64,
    pub status: MessageStatus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DelegationStatus {
    Open,
    Closed,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DelegationReply {
    pub kind: MessageKind,
    pub text: String,
    pub at: Time,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Delegation {
    pub delegation_id: String,
    pub ringid: String,
    pub silicon: Account,
    pub created_at: Time,
    pub status: DelegationStatus,
    pub request: Option<String>,
    pub context: Vec<TranscriptEntry>,
    pub replies: Vec<DelegationReply>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Recording {
    pub status: RecordingStatus,
    pub duration_ms: Option<u64>,
    pub url: Option<String>,
    pub expires_at: Option<Time>,
    pub mime_type: Option<String>,
}

// ---------------------------------------------------------------------------------------------
// Voicemail
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VoicemailReason {
    Busy,
    Declined,
    Missed,
}

impl VoicemailReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            VoicemailReason::Busy => "busy",
            VoicemailReason::Declined => "declined",
            VoicemailReason::Missed => "missed",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioStatus {
    Recording,
    Processing,
    Ready,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptStatus {
    Pending,
    Ready,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Voicemail {
    pub voicemail_id: String,
    pub ringid: String,
    pub from: Account,
    pub to: Account,
    pub reason: VoicemailReason,
    pub decline_reason: Option<String>,
    pub created_at: Time,
    pub duration_ms: Option<u64>,
    pub audio_status: AudioStatus,
    pub transcript: Option<String>,
    pub transcript_status: TranscriptStatus,
    pub read: bool,
    /// Present only on `GET /v1/voicemails/{id}` when the audio is ready.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_url: Option<String>,
}

/// `POST /v1/calls/{ringid}/voicemail`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LeaveVoicemail {
    pub text: String,
}

/// `PATCH /v1/voicemails/{id}`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VoicemailPatch {
    pub read: bool,
}

// ---------------------------------------------------------------------------------------------
// Settings and context
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RepresentativeSettings {
    pub default_context: String,
    pub context_mode: ContextMode,
    pub context_required: bool,
}

impl Default for RepresentativeSettings {
    fn default() -> Self {
        RepresentativeSettings { default_context: String::new(), context_mode: ContextMode::Append, context_required: false }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Greetings {
    pub busy: Option<String>,
    pub declined: Option<String>,
    pub missed: Option<String>,
}

impl Greetings {
    pub fn get(&self, reason: VoicemailReason) -> Option<&str> {
        match reason {
            VoicemailReason::Busy => self.busy.as_deref(),
            VoicemailReason::Declined => self.declined.as_deref(),
            VoicemailReason::Missed => self.missed.as_deref(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VoicemailSettings {
    pub enabled: bool,
    pub greetings: Greetings,
}

impl Default for VoicemailSettings {
    fn default() -> Self {
        VoicemailSettings { enabled: true, greetings: Greetings::default() }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    pub voice: String,
    pub representative: RepresentativeSettings,
    pub voicemail: VoicemailSettings,
    pub revision: u64,
}

/// `PATCH /v1/settings`: any subset. A greeting set to JSON `null` resets it to the default,
/// which is why greetings use `Option<Option<String>>` (absent vs null).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SettingsPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub representative: Option<RepresentativePatch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voicemail: Option<VoicemailPatchSettings>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RepresentativePatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_context: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_mode: Option<ContextMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_required: Option<bool>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct VoicemailPatchSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub greetings: Option<GreetingsPatch>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GreetingsPatch {
    #[serde(default, deserialize_with = "double_option", skip_serializing_if = "Option::is_none")]
    pub busy: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option", skip_serializing_if = "Option::is_none")]
    pub declined: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option", skip_serializing_if = "Option::is_none")]
    pub missed: Option<Option<String>>,
}

/// Distinguishes an absent field (`None`) from an explicit JSON `null` (`Some(None)`).
pub fn double_option<'de, T, D>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: Deserializer<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextAction {
    Init,
    Accept,
}

/// `POST /v1/context/preview`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ContextPreviewRequest {
    pub action: ContextAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ringid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_mode: Option<ContextMode>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ContextApproval {
    pub approval_id: String,
    pub default_revision: u64,
    pub mode: ContextMode,
    pub created_at: Time,
    pub expires_at: Time,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ContextPreview {
    pub preparation_id: String,
    pub action: ContextAction,
    pub default_context: String,
    pub default_revision: u64,
    pub supplied_context: Option<String>,
    pub mode: ContextMode,
    pub effective_context: String,
    pub uses_default: bool,
    pub approval_required: bool,
    pub approval: Option<ContextApproval>,
    pub expires_at: Time,
}

/// `POST /v1/context/approvals`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ApproveContext {
    pub preparation_id: String,
    #[serde(default)]
    pub valid_for_seconds: Option<u64>,
}

/// Merges default and supplied context per mode. Returns (effective, uses_default).
pub fn merge_context(default: &str, supplied: Option<&str>, mode: ContextMode) -> (String, bool) {
    let default = default.trim();
    let supplied = supplied.map(str::trim).filter(|s| !s.is_empty());
    match (mode, supplied) {
        (ContextMode::Overwrite, s) => (s.unwrap_or("").to_string(), false),
        (_, None) => (default.to_string(), !default.is_empty()),
        (_, Some(s)) if default.is_empty() => (s.to_string(), false),
        (ContextMode::Append, Some(s)) => (format!("{default}{}{s}", limits::CONTEXT_SEPARATOR), true),
        (ContextMode::Prepend, Some(s)) => (format!("{s}{}{default}", limits::CONTEXT_SEPARATOR), true),
    }
}

// ---------------------------------------------------------------------------------------------
// Notifications
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TingStatus {
    pub enabled: bool,
    pub subscription_id: Option<String>,
    pub last_error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Notifications {
    pub ting: TingStatus,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SetNotifications {
    pub enabled: bool,
}

// ---------------------------------------------------------------------------------------------
// Realtime events (`GET /v1/ws`)
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RingingStoppedReason {
    AcceptedElsewhere,
    Declined,
    Silenced,
    Cancelled,
    Missed,
}

impl RingingStoppedReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            RingingStoppedReason::AcceptedElsewhere => "accepted_elsewhere",
            RingingStoppedReason::Declined => "declined",
            RingingStoppedReason::Silenced => "silenced",
            RingingStoppedReason::Cancelled => "cancelled",
            RingingStoppedReason::Missed => "missed",
        }
    }
}

/// A server frame on `/v1/ws`: `{"type": "...", "at": "...", "data": {...}}`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EventFrame {
    #[serde(flatten)]
    pub event: Event,
    pub at: Time,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum Event {
    #[serde(rename = "ready")]
    Ready { account: Account, device_id: String },
    #[serde(rename = "call.incoming")]
    CallIncoming { call: Call, invitation: Invitation },
    #[serde(rename = "call.updated")]
    CallUpdated { call: Call },
    #[serde(rename = "call.ringing_stopped")]
    CallRingingStopped { ringid: String, invitation_id: String, reason: RingingStoppedReason },
    #[serde(rename = "call.handoff")]
    CallHandoff { ringid: String, device_id: String },
    #[serde(rename = "call.ended")]
    CallEnded { call: Call },
    #[serde(rename = "transcript.entry")]
    TranscriptEntry { ringid: String, entry: TranscriptEntry },
    #[serde(rename = "transcript.partial")]
    TranscriptPartial { ringid: String, speaker: Account, segment_id: String, text: String },
    #[serde(rename = "delegation.created")]
    DelegationCreated { delegation: Delegation },
    #[serde(rename = "voicemail.received")]
    VoicemailReceived { voicemail: Voicemail },
    #[serde(rename = "voicemail.updated")]
    VoicemailUpdated { voicemail: Voicemail },
    #[serde(rename = "device.updated")]
    DeviceUpdated { device: Device },
    #[serde(rename = "session.revoked")]
    SessionRevoked {},
    #[serde(rename = "pong")]
    Pong {},
}

impl Event {
    pub fn type_name(&self) -> &'static str {
        match self {
            Event::Ready { .. } => "ready",
            Event::CallIncoming { .. } => "call.incoming",
            Event::CallUpdated { .. } => "call.updated",
            Event::CallRingingStopped { .. } => "call.ringing_stopped",
            Event::CallHandoff { .. } => "call.handoff",
            Event::CallEnded { .. } => "call.ended",
            Event::TranscriptEntry { .. } => "transcript.entry",
            Event::TranscriptPartial { .. } => "transcript.partial",
            Event::DelegationCreated { .. } => "delegation.created",
            Event::VoicemailReceived { .. } => "voicemail.received",
            Event::VoicemailUpdated { .. } => "voicemail.updated",
            Event::DeviceUpdated { .. } => "device.updated",
            Event::SessionRevoked {} => "session.revoked",
            Event::Pong {} => "pong",
        }
    }

    /// The call this event concerns, if any.
    pub fn ringid(&self) -> Option<&str> {
        match self {
            Event::CallIncoming { call, .. } | Event::CallUpdated { call } | Event::CallEnded { call } => Some(&call.ringid),
            Event::CallRingingStopped { ringid, .. }
            | Event::CallHandoff { ringid, .. }
            | Event::TranscriptEntry { ringid, .. }
            | Event::TranscriptPartial { ringid, .. } => Some(ringid),
            Event::DelegationCreated { delegation } => Some(&delegation.ringid),
            Event::VoicemailReceived { voicemail } | Event::VoicemailUpdated { voicemail } => Some(&voicemail.ringid),
            _ => None,
        }
    }
}

/// A client frame on `/v1/ws`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientFrame {
    Ping,
}

// ---------------------------------------------------------------------------------------------
// Push payloads
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PushPerson {
    pub uuid: String,
    pub id: String,
    pub display_name: Option<String>,
    pub pfp_url: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PushParticipant {
    pub id: String,
    pub display_name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum PushPayload {
    #[serde(rename = "call.incoming")]
    CallIncoming {
        ringid: String,
        invitation_id: String,
        call_uuid: String,
        from: PushPerson,
        participants: Vec<PushParticipant>,
        expires_at: Time,
    },
    #[serde(rename = "call.cancelled")]
    CallCancelled { ringid: String, invitation_id: String, call_uuid: String, reason: RingingStoppedReason },
    #[serde(rename = "call.missed")]
    CallMissed { ringid: String, from: PushPerson },
    #[serde(rename = "voicemail.received")]
    VoicemailReceived { voicemail_id: String, from: PushPerson, transcript: Option<String> },
}

// ---------------------------------------------------------------------------------------------
// Ids
// ---------------------------------------------------------------------------------------------

/// Id prefixes used by the server.
pub mod prefix {
    pub const CALL: &str = "ring_";
    pub const INVITATION: &str = "inv_";
    pub const DEVICE: &str = "dev_";
    pub const VOICEMAIL: &str = "vm_";
    pub const DELEGATION: &str = "dlg_";
    pub const MESSAGE: &str = "msg_";
    pub const PREPARATION: &str = "prep_";
    pub const APPROVAL: &str = "apr_";
    pub const SESSION_TOKEN: &str = "rst_";
}

/// Normalizes an actor reference: strips a leading `@`, lowercases `c:`/`si:` handles.
/// Returns the input unchanged when it looks like a uuid.
pub fn normalize_actor_ref(input: &str) -> Option<String> {
    let s = input.trim().trim_start_matches('@').trim();
    if s.is_empty() {
        return None;
    }
    let lower = s.to_ascii_lowercase();
    if let Some(handle) = lower.strip_prefix("c:").or_else(|| lower.strip_prefix("si:")) {
        if handle.is_empty() || !handle.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.') {
            return None;
        }
        return Some(lower);
    }
    let is_uuid = lower.len() == 36
        && lower.chars().enumerate().all(|(i, c)| if [8, 13, 18, 23].contains(&i) { c == '-' } else { c.is_ascii_hexdigit() });
    if is_uuid { Some(lower) } else { None }
}
