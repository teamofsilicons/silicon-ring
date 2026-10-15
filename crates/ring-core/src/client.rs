//! Typed HTTP client for every endpoint in `udd/api.md`.
//!
//! SKELETON: signatures are the contract for the CLI, the Linux app and ring-ffi. Bodies are
//! implemented by the ring-core owner; keep the signatures stable.

use crate::proto::*;
use crate::{Error, Result};

/// HTTP client bound to one API origin and (optionally) one Ring session.
/// Cheap to clone; share one per process.
#[derive(Clone)]
pub struct Client {
    pub(crate) http: reqwest::Client,
    pub(crate) api_url: url::Url,
    pub(crate) session_token: Option<String>,
    pub(crate) user_agent: String,
}

/// Filters for `GET /v1/calls`.
#[derive(Clone, Debug, Default)]
pub struct CallFilter {
    pub states: Vec<CallState>,
    pub direction: Option<Direction>,
    pub with: Option<String>,
    pub since: Option<Time>,
    pub until: Option<Time>,
    pub limit: Option<u32>,
    pub cursor: Option<String>,
}

/// Filters for `GET /v1/voicemails`.
#[derive(Clone, Debug, Default)]
pub struct VoicemailFilter {
    pub unread_only: bool,
    pub from: Option<String>,
    pub since: Option<Time>,
    pub until: Option<Time>,
    pub limit: Option<u32>,
    pub cursor: Option<String>,
}

#[allow(unused_variables)]
impl Client {
    /// `api_url` is an origin such as `https://backend.ring.teamofsilicons.com` (https, or http on
    /// loopback/LAN for development). `user_agent` identifies the client, e.g. `ring-cli/1.0.0`.
    pub fn new(api_url: &str, session_token: Option<String>, user_agent: &str) -> Result<Self> {
        todo!()
    }

    pub fn api_url(&self) -> &url::Url {
        &self.api_url
    }

    pub fn session_token(&self) -> Option<&str> {
        self.session_token.as_deref()
    }

    /// Returns a copy using `token` as the session.
    pub fn with_session(&self, token: Option<String>) -> Self {
        Client { session_token: token, ..self.clone() }
    }

    // ---- public -----------------------------------------------------------------------------

    pub async fn health(&self) -> Result<Health> {
        todo!()
    }

    pub async fn meta(&self) -> Result<Meta> {
        todo!()
    }

    // ---- auth -------------------------------------------------------------------------------

    /// `POST /v1/auth/silicon` with an `Idempotency-Key`.
    pub async fn login_silicon(&self, slt: &str, label: Option<&str>) -> Result<Session> {
        todo!()
    }

    /// `POST /v1/auth/carbon`.
    pub async fn login_carbon(&self, login: &CarbonLogin) -> Result<Session> {
        todo!()
    }

    pub async fn me(&self) -> Result<Me> {
        todo!()
    }

    /// `DELETE /v1/session`.
    pub async fn logout(&self) -> Result<()> {
        todo!()
    }

    // ---- devices ----------------------------------------------------------------------------

    pub async fn update_current_device(&self, update: &DeviceUpdate) -> Result<Device> {
        todo!()
    }

    pub async fn devices(&self) -> Result<Vec<Device>> {
        todo!()
    }

    pub async fn patch_device(&self, device_id: &str, patch: &DevicePatch) -> Result<Device> {
        todo!()
    }

    pub async fn revoke_device(&self, device_id: &str) -> Result<()> {
        todo!()
    }

    // ---- people -----------------------------------------------------------------------------

    /// `GET /v1/accounts/{ref}`; `reference` is `c:x`, `si:x`, `@c:x` or a uuid.
    pub async fn account(&self, reference: &str) -> Result<AccountLookup> {
        todo!()
    }

    pub async fn recent_people(&self, query: Option<&str>, limit: Option<u32>) -> Result<Vec<RecentPerson>> {
        todo!()
    }

    // ---- calls ------------------------------------------------------------------------------

    /// `POST /v1/calls`. `idempotency_key` is sent as `Idempotency-Key` (generate one if `None`).
    pub async fn create_call(&self, req: &CreateCall, idempotency_key: Option<&str>) -> Result<CreatedCall> {
        todo!()
    }

    pub async fn calls(&self, filter: &CallFilter) -> Result<Page<Call>> {
        todo!()
    }

    pub async fn call(&self, ringid: &str) -> Result<Call> {
        todo!()
    }

    pub async fn accept(&self, ringid: &str, req: &AcceptCall) -> Result<CallWithMedia> {
        todo!()
    }

    pub async fn decline(&self, ringid: &str, req: &DeclineCall) -> Result<Call> {
        todo!()
    }

    pub async fn silence(&self, ringid: &str, invitation_id: Option<&str>) -> Result<Call> {
        todo!()
    }

    /// `POST /v1/calls/{ringid}/leave`: hang up, or cancel an unanswered outgoing call.
    pub async fn leave(&self, ringid: &str) -> Result<Call> {
        todo!()
    }

    pub async fn invite(&self, ringid: &str, to: &str, idempotency_key: Option<&str>) -> Result<InvitedCall> {
        todo!()
    }

    /// `POST /v1/calls/{ringid}/media`: (re)join, or hand the call off to this device.
    pub async fn media(&self, ringid: &str) -> Result<CallWithMedia> {
        todo!()
    }

    pub async fn transcript(&self, ringid: &str, after_seq: u64, limit: Option<u32>) -> Result<TranscriptPage> {
        todo!()
    }

    pub async fn recording(&self, ringid: &str) -> Result<Recording> {
        todo!()
    }

    /// Downloads a presigned URL (recording or voicemail audio) to memory.
    pub async fn download(&self, url: &str) -> Result<Vec<u8>> {
        todo!()
    }

    pub async fn send_message(&self, ringid: &str, req: &SendMessage, idempotency_key: Option<&str>) -> Result<SentMessage> {
        todo!()
    }

    pub async fn delegations(&self, ringid: &str, status: Option<DelegationStatus>) -> Result<Vec<Delegation>> {
        todo!()
    }

    pub async fn delegation(&self, delegation_id: &str) -> Result<Delegation> {
        todo!()
    }

    /// `POST /v1/calls/{ringid}/voicemail` (silicon caller).
    pub async fn leave_voicemail(&self, ringid: &str, text: &str, idempotency_key: Option<&str>) -> Result<Voicemail> {
        todo!()
    }

    // ---- voicemail --------------------------------------------------------------------------

    pub async fn voicemails(&self, filter: &VoicemailFilter) -> Result<Page<Voicemail>> {
        todo!()
    }

    pub async fn voicemail(&self, voicemail_id: &str) -> Result<Voicemail> {
        todo!()
    }

    pub async fn mark_voicemail(&self, voicemail_id: &str, read: bool) -> Result<Voicemail> {
        todo!()
    }

    pub async fn delete_voicemail(&self, voicemail_id: &str) -> Result<()> {
        todo!()
    }

    // ---- settings ---------------------------------------------------------------------------

    pub async fn settings(&self) -> Result<Settings> {
        todo!()
    }

    pub async fn update_settings(&self, patch: &SettingsPatch) -> Result<Settings> {
        todo!()
    }

    pub async fn voices(&self) -> Result<VoiceList> {
        todo!()
    }

    // ---- context (silicons) -----------------------------------------------------------------

    pub async fn context_preview(&self, req: &ContextPreviewRequest) -> Result<ContextPreview> {
        todo!()
    }

    pub async fn approve_context(&self, req: &ApproveContext) -> Result<ContextApproval> {
        todo!()
    }

    pub async fn context_approvals(&self) -> Result<Vec<ContextApproval>> {
        todo!()
    }

    pub async fn revoke_context_approval(&self, approval_id: &str) -> Result<()> {
        todo!()
    }

    // ---- notifications (silicons) -----------------------------------------------------------

    pub async fn notifications(&self) -> Result<Notifications> {
        todo!()
    }

    pub async fn set_notifications(&self, enabled: bool) -> Result<Notifications> {
        todo!()
    }
}

#[allow(dead_code)]
fn _unused(_: Error) {}
