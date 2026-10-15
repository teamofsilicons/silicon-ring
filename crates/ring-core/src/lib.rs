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
