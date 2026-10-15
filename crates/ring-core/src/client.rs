//! Typed HTTP client for every endpoint in `udd/api.md`.

use std::time::Duration;

use reqwest::{Method, StatusCode, header};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::proto::*;
use crate::{Error, Result};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Recordings can run to a hundred megabytes, so downloads get longer than API calls.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// HTTP client bound to one API origin and (optionally) one Ring session.
/// Cheap to clone; share one per process.
#[derive(Clone)]
pub struct Client {
    pub(crate) http: reqwest::Client,
    pub(crate) api_url: url::Url,
    pub(crate) session_token: Option<String>,
    pub(crate) user_agent: String,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("api_url", &self.api_url.as_str())
            .field("signed_in", &self.session_token.is_some())
            .field("user_agent", &self.user_agent)
            .finish()
    }
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

/// What a request carries besides its method and path.
#[derive(Default)]
struct Req<'a> {
    query: Vec<(&'static str, String)>,
    body: Option<Vec<u8>>,
    idempotency_key: Option<&'a str>,
    public: bool,
}

impl<'a> Req<'a> {
    fn json<B: Serialize + ?Sized>(body: &B) -> Self {
        Req {
            body: Some(serde_json::to_vec(body).expect("request bodies always serialize")),
            ..Req::default()
        }
    }

    fn query(query: Vec<(&'static str, String)>) -> Self {
        Req {
            query,
            ..Req::default()
        }
    }

    fn idempotent(mut self, key: &'a str) -> Self {
        self.idempotency_key = Some(key);
        self
    }

    fn public(mut self) -> Self {
        self.public = true;
        self
    }
}

impl Client {
    /// `api_url` is an origin such as `https://backend.ring.teamofsilicons.com` (https, or http on
    /// loopback/LAN for development). `user_agent` identifies the client, e.g. `ring-cli/1.0.0`.
    pub fn new(api_url: &str, session_token: Option<String>, user_agent: &str) -> Result<Self> {
        let api_url = crate::parse_api_url(api_url)?;
        let http = reqwest::Client::builder()
            .use_preconfigured_tls(crate::tls::http())
            .user_agent(user_agent)
            .timeout(REQUEST_TIMEOUT)
            .connect_timeout(CONNECT_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| Error::Network(error_chain(&e)))?;
        Ok(Client {
            http,
            api_url,
            session_token,
            user_agent: user_agent.to_owned(),
        })
    }

    pub fn api_url(&self) -> &url::Url {
        &self.api_url
    }

    pub fn session_token(&self) -> Option<&str> {
        self.session_token.as_deref()
    }

    /// Returns a copy using `token` as the session.
    pub fn with_session(&self, token: Option<String>) -> Self {
        Client {
            session_token: token,
            ..self.clone()
        }
    }

    // ---- plumbing ---------------------------------------------------------------------------

    fn url(&self, segments: &[&str]) -> url::Url {
        let mut url = self.api_url.clone();
        url.path_segments_mut()
            .expect("API origins are base URLs")
            .clear()
            .extend(segments);
        url
    }

    /// Sends a request and returns the success status and body, or the mapped error.
    async fn execute(
        &self,
        method: Method,
        segments: &[&str],
        req: Req<'_>,
    ) -> Result<(StatusCode, Vec<u8>)> {
        let url = self.url(segments);
        let mut builder = self
            .http
            .request(method, url)
            .header(header::ACCEPT, "application/json");
        if !req.public {
            let token = self.session_token.as_deref().ok_or(Error::NotSignedIn)?;
            builder = builder.bearer_auth(token);
        }
        if !req.query.is_empty() {
            builder = builder.query(&req.query);
        }
        if let Some(key) = req.idempotency_key {
            crate::validate_idempotency_key(key)?;
            builder = builder.header("Idempotency-Key", key);
        }
        if let Some(body) = req.body {
            builder = builder
                .header(header::CONTENT_TYPE, "application/json")
                .body(body);
        }
        let response = builder
            .send()
            .await
            .map_err(|e| network_error(&self.api_url, &e))?;
        let status = response.status();
        let bytes = response
            .bytes()
            .await
            .map_err(|e| network_error(&self.api_url, &e))?;
        if status.is_success() {
            Ok((status, bytes.to_vec()))
        } else {
            Err(api_error(status, &bytes))
        }
    }

    async fn request<T: DeserializeOwned>(
        &self,
        method: Method,
        segments: &[&str],
        req: Req<'_>,
    ) -> Result<T> {
        let (_, body) = self.execute(method, segments, req).await?;
        serde_json::from_slice(&body).map_err(|e| {
            Error::Protocol(format!(
                "/{} answered JSON this client does not understand: {e}",
                segments.join("/")
            ))
        })
    }

    async fn request_empty(&self, method: Method, segments: &[&str], req: Req<'_>) -> Result<()> {
        self.execute(method, segments, req).await.map(|_| ())
    }

    // ---- public -----------------------------------------------------------------------------

    pub async fn health(&self) -> Result<Health> {
        self.request(Method::GET, &["healthz"], Req::default().public())
            .await
    }

    pub async fn meta(&self) -> Result<Meta> {
        self.request(Method::GET, &["v1", "meta"], Req::default().public())
            .await
    }

    // ---- auth -------------------------------------------------------------------------------

    /// `POST /v1/auth/silicon` with an `Idempotency-Key`.
    pub async fn login_silicon(&self, slt: &str, label: Option<&str>) -> Result<Session> {
        self.login_silicon_with_key(slt, label, &crate::new_idempotency_key())
            .await
    }

    /// `POST /v1/auth/silicon` with a caller-chosen `Idempotency-Key`, so a retry of the same
    /// exchange after an uncertain failure carries the same key.
    pub async fn login_silicon_with_key(
        &self,
        slt: &str,
        label: Option<&str>,
        idempotency_key: &str,
    ) -> Result<Session> {
        let body = SiliconLogin {
            slt: slt.to_owned(),
            label: label.map(str::to_owned),
        };
        self.request(
            Method::POST,
            &["v1", "auth", "silicon"],
            Req::json(&body).public().idempotent(idempotency_key),
        )
        .await
    }

    /// `POST /v1/auth/carbon`.
    pub async fn login_carbon(&self, login: &CarbonLogin) -> Result<Session> {
        self.request(
            Method::POST,
            &["v1", "auth", "carbon"],
            Req::json(login).public(),
        )
        .await
    }

    pub async fn me(&self) -> Result<Me> {
        self.request(Method::GET, &["v1", "me"], Req::default())
            .await
    }

    /// `DELETE /v1/session`.
    pub async fn logout(&self) -> Result<()> {
        self.request_empty(Method::DELETE, &["v1", "session"], Req::default())
            .await
    }

    // ---- devices ----------------------------------------------------------------------------

    pub async fn update_current_device(&self, update: &DeviceUpdate) -> Result<Device> {
        self.request(
            Method::PUT,
            &["v1", "devices", "current"],
            Req::json(update),
        )
        .await
    }

    pub async fn devices(&self) -> Result<Vec<Device>> {
        let page: Items<Device> = self
            .request(Method::GET, &["v1", "devices"], Req::default())
            .await?;
        Ok(page.items)
    }

    pub async fn patch_device(&self, device_id: &str, patch: &DevicePatch) -> Result<Device> {
        self.request(
            Method::PATCH,
            &["v1", "devices", device_id],
            Req::json(patch),
        )
        .await
    }

    pub async fn revoke_device(&self, device_id: &str) -> Result<()> {
        self.request_empty(
            Method::DELETE,
            &["v1", "devices", device_id],
            Req::default(),
        )
        .await
    }

    // ---- people -----------------------------------------------------------------------------

    /// `GET /v1/accounts/{ref}`; `reference` is `c:x`, `si:x`, `@c:x` or a uuid.
    pub async fn account(&self, reference: &str) -> Result<AccountLookup> {
        self.request(Method::GET, &["v1", "accounts", reference], Req::default())
            .await
    }

    pub async fn recent_people(
        &self,
        query: Option<&str>,
        limit: Option<u32>,
    ) -> Result<Vec<RecentPerson>> {
        let mut q = Vec::new();
        if let Some(text) = query {
            q.push(("q", text.to_owned()));
        }
        if let Some(limit) = limit {
            q.push(("limit", limit.to_string()));
        }
        let page: Items<RecentPerson> = self
            .request(Method::GET, &["v1", "people", "recent"], Req::query(q))
            .await?;
        Ok(page.items)
    }

    // ---- calls ------------------------------------------------------------------------------

    /// `POST /v1/calls`. `idempotency_key` is sent as `Idempotency-Key` (generate one if `None`).
    pub async fn create_call(
        &self,
        req: &CreateCall,
        idempotency_key: Option<&str>,
    ) -> Result<CreatedCall> {
        let key = key_or_new(idempotency_key);
        self.request(
            Method::POST,
            &["v1", "calls"],
            Req::json(req).idempotent(&key),
        )
        .await
    }

    pub async fn calls(&self, filter: &CallFilter) -> Result<Page<Call>> {
        let mut q = Vec::new();
        if !filter.states.is_empty() {
            q.push((
                "state",
                filter
                    .states
                    .iter()
                    .map(CallState::as_str)
                    .collect::<Vec<_>>()
                    .join(","),
            ));
        }
        if let Some(direction) = filter.direction {
            q.push(("direction", direction_str(direction).to_owned()));
        }
        push_opt(&mut q, "with", filter.with.clone());
        push_opt(&mut q, "since", filter.since.map(|t| t.to_rfc3339()));
        push_opt(&mut q, "until", filter.until.map(|t| t.to_rfc3339()));
        push_opt(&mut q, "limit", filter.limit.map(|n| n.to_string()));
        push_opt(&mut q, "cursor", filter.cursor.clone());
        self.request(Method::GET, &["v1", "calls"], Req::query(q))
            .await
    }

    pub async fn call(&self, ringid: &str) -> Result<Call> {
        self.request(Method::GET, &["v1", "calls", ringid], Req::default())
            .await
    }

    pub async fn accept(&self, ringid: &str, req: &AcceptCall) -> Result<CallWithMedia> {
        self.request(
            Method::POST,
            &["v1", "calls", ringid, "accept"],
            Req::json(req),
        )
        .await
    }

    pub async fn decline(&self, ringid: &str, req: &DeclineCall) -> Result<Call> {
        let envelope: CallEnvelope = self
            .request(
                Method::POST,
                &["v1", "calls", ringid, "decline"],
                Req::json(req),
            )
            .await?;
        Ok(envelope.call)
    }

    pub async fn silence(&self, ringid: &str, invitation_id: Option<&str>) -> Result<Call> {
        let body = SilenceCall {
            invitation_id: invitation_id.map(str::to_owned),
        };
        let envelope: CallEnvelope = self
            .request(
                Method::POST,
                &["v1", "calls", ringid, "silence"],
                Req::json(&body),
            )
            .await?;
        Ok(envelope.call)
    }

    /// `POST /v1/calls/{ringid}/leave`: hang up, or cancel an unanswered outgoing call.
    pub async fn leave(&self, ringid: &str) -> Result<Call> {
        let envelope: CallEnvelope = self
            .request(
                Method::POST,
                &["v1", "calls", ringid, "leave"],
                Req::json(&serde_json::json!({})),
            )
            .await?;
        Ok(envelope.call)
    }

    pub async fn invite(
        &self,
        ringid: &str,
        to: &str,
        idempotency_key: Option<&str>,
    ) -> Result<InvitedCall> {
        let key = key_or_new(idempotency_key);
        let body = InviteToCall { to: to.to_owned() };
        self.request(
            Method::POST,
            &["v1", "calls", ringid, "invite"],
            Req::json(&body).idempotent(&key),
        )
        .await
    }

    /// `POST /v1/calls/{ringid}/media`: (re)join, or hand the call off to this device.
    pub async fn media(&self, ringid: &str) -> Result<CallWithMedia> {
        self.request(
            Method::POST,
            &["v1", "calls", ringid, "media"],
            Req::json(&serde_json::json!({})),
        )
        .await
    }

    pub async fn transcript(
        &self,
        ringid: &str,
        after_seq: u64,
        limit: Option<u32>,
    ) -> Result<TranscriptPage> {
        let mut q = vec![("after_seq", after_seq.to_string())];
        push_opt(&mut q, "limit", limit.map(|n| n.to_string()));
        self.request(
            Method::GET,
            &["v1", "calls", ringid, "transcript"],
            Req::query(q),
        )
        .await
    }

    pub async fn recording(&self, ringid: &str) -> Result<Recording> {
        self.request(
            Method::GET,
            &["v1", "calls", ringid, "recording"],
            Req::default(),
        )
        .await
    }

    /// Downloads a presigned URL (recording or voicemail audio) to memory.
    ///
    /// The session token is never sent: presigned links carry their own authorization and
    /// usually point at another host.
    pub async fn download(&self, url: &str) -> Result<Vec<u8>> {
        let parsed = url::Url::parse(url)
            .map_err(|_| Error::Input("The download link is not a valid URL.".into()))?;
        let allowed = match parsed.scheme() {
            "https" => true,
            "http" => parsed.host_str().is_some_and(crate::is_development_host),
            _ => false,
        };
        if !allowed {
            return Err(Error::Input(
                "Refusing to download from a link that is not https.".into(),
            ));
        }
        let response = self
            .http
            .get(parsed)
            .timeout(DOWNLOAD_TIMEOUT)
            .send()
            .await
            .map_err(|e| download_error(&e))?;
        let status = response.status();
        if !status.is_success() {
            return Err(Error::Api {
                status: status.as_u16(),
                error: ApiError {
                    code: "download_failed".into(),
                    message: format!("The audio download answered HTTP {}.", status.as_u16()),
                    hint: Some(
                        "Download links expire after 10 minutes. Request a fresh one and retry."
                            .into(),
                    ),
                    retryable: true,
                    details: serde_json::json!({}),
                },
            });
        }
        let bytes = response.bytes().await.map_err(|e| download_error(&e))?;
        Ok(bytes.to_vec())
    }

    pub async fn send_message(
        &self,
        ringid: &str,
        req: &SendMessage,
        idempotency_key: Option<&str>,
    ) -> Result<SentMessage> {
        let key = key_or_new(idempotency_key);
        self.request(
            Method::POST,
            &["v1", "calls", ringid, "messages"],
            Req::json(req).idempotent(&key),
        )
        .await
    }

    pub async fn delegations(
        &self,
        ringid: &str,
        status: Option<DelegationStatus>,
    ) -> Result<Vec<Delegation>> {
        let mut q = Vec::new();
        if let Some(status) = status {
            q.push((
                "status",
                match status {
                    DelegationStatus::Open => "open",
                    DelegationStatus::Closed => "closed",
                }
                .to_owned(),
            ));
        }
        let page: Items<Delegation> = self
            .request(
                Method::GET,
                &["v1", "calls", ringid, "delegations"],
                Req::query(q),
            )
            .await?;
        Ok(page.items)
    }

    pub async fn delegation(&self, delegation_id: &str) -> Result<Delegation> {
        self.request(
            Method::GET,
            &["v1", "delegations", delegation_id],
            Req::default(),
        )
        .await
    }

    /// `POST /v1/calls/{ringid}/voicemail` (silicon caller).
    pub async fn leave_voicemail(
        &self,
        ringid: &str,
        text: &str,
        idempotency_key: Option<&str>,
    ) -> Result<Voicemail> {
        let key = key_or_new(idempotency_key);
        let body = LeaveVoicemail {
            text: text.to_owned(),
        };
        self.request(
            Method::POST,
            &["v1", "calls", ringid, "voicemail"],
            Req::json(&body).idempotent(&key),
        )
        .await
    }

    // ---- voicemail --------------------------------------------------------------------------

    pub async fn voicemails(&self, filter: &VoicemailFilter) -> Result<Page<Voicemail>> {
        let mut q = Vec::new();
        if filter.unread_only {
            q.push(("unread", "true".to_owned()));
        }
        push_opt(&mut q, "from", filter.from.clone());
        push_opt(&mut q, "since", filter.since.map(|t| t.to_rfc3339()));
        push_opt(&mut q, "until", filter.until.map(|t| t.to_rfc3339()));
        push_opt(&mut q, "limit", filter.limit.map(|n| n.to_string()));
        push_opt(&mut q, "cursor", filter.cursor.clone());
        self.request(Method::GET, &["v1", "voicemails"], Req::query(q))
            .await
    }

    pub async fn voicemail(&self, voicemail_id: &str) -> Result<Voicemail> {
        self.request(
            Method::GET,
            &["v1", "voicemails", voicemail_id],
            Req::default(),
        )
        .await
    }

    pub async fn mark_voicemail(&self, voicemail_id: &str, read: bool) -> Result<Voicemail> {
        self.request(
            Method::PATCH,
            &["v1", "voicemails", voicemail_id],
            Req::json(&VoicemailPatch { read }),
        )
        .await
    }

    pub async fn delete_voicemail(&self, voicemail_id: &str) -> Result<()> {
        self.request_empty(
            Method::DELETE,
            &["v1", "voicemails", voicemail_id],
            Req::default(),
        )
        .await
    }

    // ---- settings ---------------------------------------------------------------------------

    pub async fn settings(&self) -> Result<Settings> {
        self.request(Method::GET, &["v1", "settings"], Req::default())
            .await
    }

    pub async fn update_settings(&self, patch: &SettingsPatch) -> Result<Settings> {
        self.request(Method::PATCH, &["v1", "settings"], Req::json(patch))
            .await
    }

    pub async fn voices(&self) -> Result<VoiceList> {
        self.request(Method::GET, &["v1", "voices"], Req::default())
            .await
    }

    // ---- context (silicons) -----------------------------------------------------------------

    pub async fn context_preview(&self, req: &ContextPreviewRequest) -> Result<ContextPreview> {
        self.request(Method::POST, &["v1", "context", "preview"], Req::json(req))
            .await
    }

    pub async fn approve_context(&self, req: &ApproveContext) -> Result<ContextApproval> {
        self.request(
            Method::POST,
            &["v1", "context", "approvals"],
            Req::json(req),
        )
        .await
    }

    pub async fn context_approvals(&self) -> Result<Vec<ContextApproval>> {
        let page: Items<ContextApproval> = self
            .request(Method::GET, &["v1", "context", "approvals"], Req::default())
            .await?;
        Ok(page.items)
    }

    pub async fn revoke_context_approval(&self, approval_id: &str) -> Result<()> {
        self.request_empty(
            Method::DELETE,
            &["v1", "context", "approvals", approval_id],
            Req::default(),
        )
        .await
    }

    // ---- notifications (silicons) -----------------------------------------------------------

    pub async fn notifications(&self) -> Result<Notifications> {
        self.request(Method::GET, &["v1", "notifications"], Req::default())
            .await
    }

    pub async fn set_notifications(&self, enabled: bool) -> Result<Notifications> {
        self.request(
            Method::POST,
            &["v1", "notifications"],
            Req::json(&SetNotifications { enabled }),
        )
        .await
    }
}

fn key_or_new(key: Option<&str>) -> String {
    key.map(str::to_owned)
        .unwrap_or_else(crate::new_idempotency_key)
}

fn push_opt(query: &mut Vec<(&'static str, String)>, name: &'static str, value: Option<String>) {
    if let Some(value) = value {
        query.push((name, value));
    }
}

fn direction_str(direction: Direction) -> &'static str {
    match direction {
        Direction::Incoming => "incoming",
        Direction::Outgoing => "outgoing",
    }
}

/// Maps a non-2xx answer to [`Error::Api`]. Answers without a Ring error body (a proxy's 502,
/// for instance) get a code derived from the status so callers can still branch on it.
fn api_error(status: StatusCode, body: &[u8]) -> Error {
    if let Ok(parsed) = serde_json::from_slice::<ErrorBody>(body) {
        return Error::Api {
            status: status.as_u16(),
            error: parsed.error,
        };
    }
    let (code, retryable) = match status.as_u16() {
        400 => (codes::INVALID_INPUT, false),
        401 => (codes::UNAUTHENTICATED, false),
        403 => (codes::FORBIDDEN, false),
        404 => (codes::NOT_FOUND, false),
        409 => (codes::CONFLICT, false),
        429 => (codes::RATE_LIMITED, true),
        502..=504 => ("server_unavailable", true),
        500..=599 => ("server_error", true),
        _ => ("http_error", false),
    };
    Error::Api {
        status: status.as_u16(),
        error: ApiError {
            code: code.into(),
            message: format!(
                "The server answered HTTP {} without a Ring error body.",
                status.as_u16()
            ),
            hint: retryable.then(|| "Try again shortly.".to_owned()),
            retryable,
            details: serde_json::json!({}),
        },
    }
}

fn network_error(origin: &url::Url, error: &reqwest::Error) -> Error {
    let origin = origin.as_str().trim_end_matches('/');
    if error.is_timeout() {
        return Error::Network(format!("{origin} did not answer in time"));
    }
    Error::Network(format!("could not reach {origin}: {}", error_chain(error)))
}

/// Download errors leave the URL out: presigned links carry signatures.
fn download_error(error: &reqwest::Error) -> Error {
    if error.is_timeout() {
        return Error::Network("the audio download timed out".into());
    }
    Error::Network(format!("the audio download failed: {}", error_chain(error)))
}

/// Joins an error and its sources into one line without repeating the URL.
fn error_chain(error: &reqwest::Error) -> String {
    let mut parts = Vec::new();
    let mut current: Option<&dyn std::error::Error> = Some(error);
    while let Some(e) = current {
        let text = e.to_string();
        let text = match e.downcast_ref::<reqwest::Error>() {
            Some(re) if re.url().is_some() => {
                let url = re.url().map(|u| u.as_str()).unwrap_or_default();
                text.replace(&format!(" for url ({url})"), "")
                    .replace(url, "")
            }
            _ => text,
        };
        if !text.is_empty() && parts.last() != Some(&text) {
            parts.push(text);
        }
        current = e.source();
    }
    parts.join(": ")
}
