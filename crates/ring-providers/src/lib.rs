//! Service adapters. Credentials stay in the server; errors never include response bodies or keys.
use serde::{Deserialize, Serialize};
use std::{fmt, time::Duration};

pub mod iam;
#[cfg(feature = "s3")]
pub mod storage;
pub mod voice;
pub use iam::{Iam, IamSession, Identity, Ting};
pub use voice::{Deepgram, LiveSocket, OpenAi, DEFAULT_VOICE, NATURAL_VOICES};

pub type Result<T> = std::result::Result<T, Error>;
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Error {
    pub code: String,
    pub message: String,
    pub retryable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}
impl Error {
    pub fn new(code: &str, message: &str, retryable: bool) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            retryable,
            details: None,
        }
    }
    pub(crate) fn network(provider: &str) -> Self {
        Self::new("PROVIDER_UNAVAILABLE", &format!("{provider} did not confirm the operation; retry only with the original operation identity."), true)
    }
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for Error {}

pub fn init_tls() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

pub(crate) fn http() -> Result<reqwest::Client> {
    init_tls();
    reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .connect_timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| Error::network("HTTP client"))
}
pub(crate) async fn checked(
    response: reqwest::Response,
    service: &str,
) -> Result<reqwest::Response> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    Err(Error::new(
        if status.as_u16() == 401 || status.as_u16() == 403 {
            "PROVIDER_AUTH_FAILED"
        } else {
            "PROVIDER_REJECTED"
        },
        &format!(
            "{service} returned HTTP {}. Check provider configuration and account access.",
            status.as_u16()
        ),
        status.as_u16() == 429 || status.is_server_error(),
    ))
}
pub(crate) fn required_env(key: &str) -> Result<String> {
    std::env::var(key)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .ok_or_else(|| {
            Error::new(
                "PROVIDER_NOT_CONFIGURED",
                &format!("Set {key} in the server environment."),
                false,
            )
        })
}

/// Space Station's SDK owns durable delivery. Instantiate one recorder for each source table.
pub struct Telemetry(Option<space_station::SpaceClient>);
impl Telemetry {
    pub fn new(key: Option<&str>, enabled: bool) -> Result<Self> {
        init_tls();
        if !enabled {
            return Ok(Self(None));
        }
        let key = key.ok_or_else(|| {
            Error::new(
                "TELEMETRY_NOT_CONFIGURED",
                "Provision the source's Space Station table and key.",
                false,
            )
        })?;
        space_station::SpaceClient::new(key)
            .map(|v| Self(Some(v)))
            .map_err(|_| {
                Error::new(
                    "TELEMETRY_NOT_CONFIGURED",
                    "Invalid Space Station table key.",
                    false,
                )
            })
    }
    /// Deliberately accepts a narrow metadata schema, never arbitrary conversation payloads.
    pub fn record(
        &self,
        source: &str,
        step: &str,
        progress: &str,
        trace_id: &str,
        duration_ms: Option<u64>,
    ) {
        if let Some(client) = &self.0 {
            client.record(serde_json::json!({"app":"ring","source":source,"step":step,"progress":progress,"trace_id":trace_id,"duration_ms":duration_ms}));
        }
    }
    pub fn flush(&self) -> bool {
        self.0.as_ref().is_none_or(|client| client.flush())
    }
}
