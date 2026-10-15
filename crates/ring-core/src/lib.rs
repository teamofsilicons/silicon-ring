//! Silicon Ring client SDK.
//!
//! - [`Client`]: every HTTP endpoint of `udd/api.md`, typed with [`proto`].
//! - [`events`]: the realtime WebSocket (`/v1/ws`) with automatic reconnect.
//! - [`auth`]: PKCE and a loopback redirect receiver for desktop carbon sign-in.
//! - [`media`] (feature `media`): join a call's LiveKit room with platform audio (mic + speaker,
//!   echo cancellation), mute, pick devices, observe speakers.
//!
//! The CLI uses this crate without `media`; the Linux app and `ring-ffi` (Windows) enable it.

pub use ring_proto as proto;

pub mod auth;
mod client;
pub mod events;
#[cfg(feature = "media")]
pub mod media;
mod tls;

pub use client::{CallFilter, Client, VoicemailFilter};

/// Production API origin.
pub const DEFAULT_API_URL: &str = "https://backend.ring.teamofsilicons.com";

/// Every failure a client can see. API errors keep the server's code/hint so callers can act on them.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The server answered with an error body.
    #[error("{} ({}): {}", .error.code, .status, .error.message)]
    Api { status: u16, error: proto::ApiError },
    /// No session token is configured.
    #[error("not signed in")]
    NotSignedIn,
    /// Network, TLS or timeout failure. The request may or may not have reached the server.
    #[error("network error: {0}")]
    Network(String),
    /// The server's answer could not be understood.
    #[error("unexpected response: {0}")]
    Protocol(String),
    /// Invalid input detected before sending (e.g. a text limit).
    #[error("{0}")]
    Input(String),
    /// Media (LiveKit/audio device) failure.
    #[error("media error: {0}")]
    Media(String),
}

impl Error {
    /// Stable error code: the server's code for API errors, otherwise a local one
    /// (`not_signed_in`, `network`, `protocol`, `invalid_input`, `media`).
    pub fn code(&self) -> &str {
        match self {
            Error::Api { error, .. } => &error.code,
            Error::NotSignedIn => "not_signed_in",
            Error::Network(_) => "network",
            Error::Protocol(_) => "protocol",
            Error::Input(_) => "invalid_input",
            Error::Media(_) => "media",
        }
    }

    pub fn hint(&self) -> Option<&str> {
        match self {
            Error::Api { error, .. } => error.hint.as_deref(),
            Error::NotSignedIn => Some("Sign in first."),
            _ => None,
        }
    }

    pub fn retryable(&self) -> bool {
        match self {
            Error::Api { error, .. } => error.retryable,
            Error::Network(_) => true,
            _ => false,
        }
    }

    /// HTTP status for API errors.
    pub fn status(&self) -> Option<u16> {
        match self {
            Error::Api { status, .. } => Some(*status),
            _ => None,
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// A fresh `Idempotency-Key` value (`ring-<32 hex>`), valid for every idempotent endpoint.
pub fn new_idempotency_key() -> String {
    format!("ring-{}", uuid::Uuid::new_v4().simple())
}

/// Checks an `Idempotency-Key`: 8–200 printable ASCII characters.
pub fn validate_idempotency_key(key: &str) -> Result<()> {
    let printable = key.bytes().all(|b| (0x20..=0x7e).contains(&b));
    if (8..=200).contains(&key.len()) && printable && key.trim() == key {
        Ok(())
    } else {
        Err(Error::Input(
            "An idempotency key must be 8 to 200 printable ASCII characters.".into(),
        ))
    }
}

/// Checks a text against its limit in Unicode code points, the way the server counts.
/// Text over the limit is rejected, never truncated.
pub fn check_text(field: &str, text: &str, limit: usize) -> Result<()> {
    let length = proto::text_len(text);
    if length <= limit {
        Ok(())
    } else {
        Err(Error::Input(format!(
            "{field} is {length} characters; the limit is {limit}."
        )))
    }
}

/// Parses an API origin such as `https://backend.ring.teamofsilicons.com`.
///
/// It must be `https`, or plain `http` only for loopback and private development hosts. It may
/// not carry credentials, a path, a query or a fragment.
pub fn parse_api_url(input: &str) -> Result<url::Url> {
    let invalid = |why: &str| Error::Input(format!("Invalid API URL {input:?}: {why}."));
    let mut url = url::Url::parse(input.trim()).map_err(|_| invalid("not an absolute URL"))?;
    let host = url
        .host_str()
        .ok_or_else(|| invalid("it has no host"))?
        .to_owned();
    match url.scheme() {
        "https" => {}
        "http" if is_development_host(&host) => {}
        "http" => {
            return Err(invalid(
                "plain http is only allowed for loopback and private network hosts",
            ));
        }
        _ => return Err(invalid("use https://")),
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(invalid("it must not contain credentials"));
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(invalid("it must not contain a query or fragment"));
    }
    if url.path() != "/" && !url.path().is_empty() {
        return Err(invalid("give only the origin, without a path"));
    }
    url.set_path("/");
    Ok(url)
}

/// Hosts where plain `http` is acceptable: loopback, RFC 1918 and link-local networks, and
/// development-only names.
pub(crate) fn is_development_host(host: &str) -> bool {
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        return match ip {
            std::net::IpAddr::V4(v4) => v4.is_loopback() || v4.is_private() || v4.is_link_local(),
            std::net::IpAddr::V6(v6) => {
                v6.is_loopback() || v6.is_unique_local() || v6.is_unicast_link_local()
            }
        };
    }
    let host = host.to_ascii_lowercase();
    host == "localhost"
        || [".localhost", ".local", ".test"]
            .iter()
            .any(|suffix| host.ends_with(suffix))
}
