//! Request extractors: JSON and query bodies with contract-shaped errors, and the signed-in
//! session (Bearer token or `ring_session` cookie with the CSRF header).

use std::net::{IpAddr, SocketAddr};

use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{ConnectInfo, FromRequest, FromRequestParts, Query, Request};
use axum::http::{HeaderMap, Method, request::Parts};
use ring_proto::Account;
use serde::de::DeserializeOwned;

use crate::app::AppState;
use crate::error::ApiError;
use crate::sessions::{self, SessionRow};
use crate::util::{looks_like_session_token, now_ms, sha256_hex};

pub const SESSION_COOKIE: &str = "ring_session";
pub const LOGIN_COOKIE: &str = "ring_login";
pub const CLIENT_HEADER: &str = "x-ring-client";

/// `Json<T>` whose rejection is the contract's `400 invalid_input`.
pub struct ApiJson<T>(pub T);

impl<S: Send + Sync, T: DeserializeOwned> FromRequest<S> for ApiJson<T> {
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        match axum::Json::<T>::from_request(req, state).await {
            Ok(axum::Json(value)) => Ok(ApiJson(value)),
            Err(rejection) => Err(json_rejection(rejection)),
        }
    }
}

/// A JSON body that may also be empty (then `T::default()`), for requests whose fields are all
/// optional.
pub struct JsonOrEmpty<T>(pub T);

impl<S: Send + Sync, T: DeserializeOwned + Default> FromRequest<S> for JsonOrEmpty<T> {
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let bytes = axum::body::Bytes::from_request(req, state)
            .await
            .map_err(|_| ApiError::invalid("The request body could not be read."))?;
        if bytes.iter().all(u8::is_ascii_whitespace) {
            return Ok(JsonOrEmpty(T::default()));
        }
        serde_json::from_slice(&bytes)
            .map(JsonOrEmpty)
            .map_err(|e| {
                ApiError::invalid(format!(
                    "The request body does not match the expected shape: {e}"
                ))
            })
    }
}

fn json_rejection(rejection: JsonRejection) -> ApiError {
    let message = match &rejection {
        JsonRejection::JsonDataError(e) => format!(
            "The request body does not match the expected shape: {}",
            e.body_text()
        ),
        JsonRejection::JsonSyntaxError(_) => "The request body is not valid JSON.".to_string(),
        JsonRejection::MissingJsonContentType(_) => {
            "Send the body as JSON with Content-Type: application/json.".to_string()
        }
        _ => "The request body could not be read.".to_string(),
    };
    ApiError::invalid(message)
}

/// `Query<T>` whose rejection is the contract's `400 invalid_input`.
pub struct ApiQuery<T>(pub T);

impl<S: Send + Sync, T: DeserializeOwned> FromRequestParts<S> for ApiQuery<T> {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        match Query::<T>::from_request_parts(parts, state).await {
            Ok(Query(value)) => Ok(ApiQuery(value)),
            Err(rejection) => Err(query_rejection(rejection)),
        }
    }
}

fn query_rejection(rejection: QueryRejection) -> ApiError {
    ApiError::invalid(format!(
        "Invalid query parameters: {}",
        rejection.body_text()
    ))
}

pub fn cookie<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get_all(axum::http::header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v.trim())
}

/// Where a session token came from.
pub fn session_token(headers: &HeaderMap) -> Option<(String, bool)> {
    if let Some(value) = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    {
        let token = value
            .strip_prefix("Bearer ")
            .or_else(|| value.strip_prefix("bearer "))
            .unwrap_or("")
            .trim();
        return Some((token.to_string(), false));
    }
    cookie(headers, SESSION_COOKIE).map(|t| (t.to_string(), true))
}

/// The signed-in device making the request.
pub struct Auth {
    pub session: SessionRow,
    pub account: Account,
    pub via_cookie: bool,
}

impl Auth {
    pub fn uuid(&self) -> &str {
        &self.account.uuid
    }

    pub fn device_id(&self) -> &str {
        &self.session.device_id
    }

    pub fn is_silicon(&self) -> bool {
        self.account.kind == ring_proto::AccountKind::Silicon
    }
}

impl FromRequestParts<AppState> for Auth {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        app: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let (token, via_cookie) =
            session_token(&parts.headers).ok_or_else(ApiError::unauthenticated)?;
        if via_cookie && is_mutation(&parts.method) && !parts.headers.contains_key(CLIENT_HEADER) {
            return Err(ApiError::csrf());
        }
        authenticate(app, &token, via_cookie)
    }
}

pub fn is_mutation(method: &Method) -> bool {
    matches!(
        *method,
        Method::POST | Method::PUT | Method::PATCH | Method::DELETE
    )
}

pub fn authenticate(app: &AppState, token: &str, via_cookie: bool) -> Result<Auth, ApiError> {
    if !looks_like_session_token(token) {
        return Err(ApiError::unauthenticated());
    }
    let hash = sha256_hex(token.as_bytes());
    let found = app.db.with(|c| sessions::by_token_hash(c, &hash))?;
    let (session, account) = found.ok_or_else(ApiError::unauthenticated)?;
    let now = now_ms();
    if session.revoked_at.is_some() || session.expires_at <= now {
        return Err(ApiError::session_expired());
    }
    if session.last_seen_at.is_none_or(|t| now - t > 60_000) {
        let _ = app.db.with(|c| {
            c.execute(
                "UPDATE sessions SET last_seen_at = ?2 WHERE device_id = ?1",
                rusqlite::params![session.device_id, now],
            )
        });
    }
    Ok(Auth {
        session,
        account,
        via_cookie,
    })
}

/// The caller's address: the socket peer, or the proxy's X-Forwarded-For when the peer is a
/// loopback proxy (Caddy).
pub fn client_ip(parts_headers: &HeaderMap, peer: Option<SocketAddr>) -> Option<IpAddr> {
    let peer_ip = peer.map(|p| p.ip());
    if peer_ip.is_some_and(|ip| ip.is_loopback())
        && let Some(forwarded) = parts_headers
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
        && let Some(first) = forwarded.split(',').next()
        && let Ok(ip) = first.trim().parse()
    {
        return Some(ip);
    }
    peer_ip
}

/// True for a request that came straight from this host, not through the reverse proxy.
pub fn is_direct_loopback(headers: &HeaderMap, peer: Option<SocketAddr>) -> bool {
    let proxied = [
        "x-forwarded-for",
        "forwarded",
        "x-real-ip",
        "x-forwarded-host",
    ]
    .iter()
    .any(|h| headers.contains_key(*h));
    !proxied && peer.is_some_and(|p| p.ip().is_loopback())
}

/// The peer address, when the server was started with connect info.
pub struct Peer(pub Option<SocketAddr>);

impl<S: Send + Sync> FromRequestParts<S> for Peer {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        Ok(Peer(
            parts
                .extensions
                .get::<ConnectInfo<SocketAddr>>()
                .map(|c| c.0),
        ))
    }
}
