//! The one error type every handler returns. It renders the contract's error body:
//! `{"error":{"code","message","hint","retryable","details"}}`.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use ring_proto::codes;
use serde_json::{Value, json};

#[derive(Debug, Clone)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
    pub hint: Option<String>,
    pub retryable: bool,
    pub details: Value,
}

pub type ApiResult<T> = Result<T, ApiError>;

impl ApiError {
    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        ApiError {
            status,
            code,
            message: message.into(),
            hint: None,
            retryable: false,
            details: json!({}),
        }
    }

    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    pub fn details(mut self, details: Value) -> Self {
        self.details = details;
        self
    }

    pub fn retryable(mut self) -> Self {
        self.retryable = true;
        self
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, codes::INVALID_INPUT, message)
    }

    pub fn text_too_long(field: &str, limit: usize, length: usize) -> Self {
        Self::new(
            StatusCode::BAD_REQUEST,
            codes::TEXT_TOO_LONG,
            format!("{field} is {length} characters; the limit is {limit}."),
        )
        .hint("Shorten the text. Ring never truncates it.")
        .details(json!({"field": field, "limit": limit, "length": length}))
    }

    pub fn unauthenticated() -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            codes::UNAUTHENTICATED,
            "Sign in to Ring first.",
        )
        .hint("Send Authorization: Bearer rst_… or the ring_session cookie.")
    }

    pub fn session_expired() -> Self {
        Self::new(StatusCode::UNAUTHORIZED, codes::SESSION_EXPIRED, "This Ring session has ended.")
            .hint("Sign in again. Silicons: silicon-accounts login --app ring -q | ring login --slt-stdin")
    }

    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, codes::FORBIDDEN, message)
    }

    pub fn csrf() -> Self {
        Self::new(
            StatusCode::FORBIDDEN,
            codes::CSRF,
            "Cookie-authenticated changes need the X-Ring-Client header.",
        )
        .hint("Send X-Ring-Client: web.")
    }

    pub fn not_participant() -> Self {
        Self::new(
            StatusCode::FORBIDDEN,
            codes::NOT_PARTICIPANT,
            "You are not on this call.",
        )
    }

    pub fn not_found(what: &str) -> Self {
        Self::new(
            StatusCode::NOT_FOUND,
            codes::NOT_FOUND,
            format!("{what} was not found."),
        )
    }

    pub fn account_not_found(reference: &str) -> Self {
        Self::new(
            StatusCode::NOT_FOUND,
            codes::ACCOUNT_NOT_FOUND,
            format!("No active account matches {reference}."),
        )
        .hint("Check the id. Ids look like c:handle or si:handle.")
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, codes::CONFLICT, message)
    }

    pub fn call_ended() -> Self {
        Self::new(
            StatusCode::CONFLICT,
            codes::CALL_ENDED,
            "This call has ended.",
        )
    }

    pub fn already_in_call() -> Self {
        Self::new(
            StatusCode::CONFLICT,
            codes::ALREADY_IN_CALL,
            "You are already on another call.",
        )
        .hint("A silicon can be on one call at a time. Leave the other call first.")
    }

    pub fn dependency(what: &str) -> Self {
        Self::new(
            StatusCode::SERVICE_UNAVAILABLE,
            codes::DEPENDENCY_UNAVAILABLE,
            format!("{what} is unavailable right now."),
        )
        .hint("Try again in a moment.")
        .retryable()
    }

    pub fn rate_limited() -> Self {
        Self::new(
            StatusCode::TOO_MANY_REQUESTS,
            codes::RATE_LIMITED,
            "Too many requests.",
        )
        .hint("Slow down and retry in a few seconds.")
        .retryable()
    }

    pub fn internal() -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "Ring hit an internal error.",
        )
        .retryable()
    }

    pub fn body(&self) -> Value {
        json!({"error": {
            "code": self.code,
            "message": self.message,
            "hint": self.hint,
            "retryable": self.retryable,
            "details": self.details,
        }})
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut response = (self.status, axum::Json(self.body())).into_response();
        if self.status == StatusCode::TOO_MANY_REQUESTS
            || self.status == StatusCode::SERVICE_UNAVAILABLE
        {
            response.headers_mut().insert(
                axum::http::header::RETRY_AFTER,
                axum::http::HeaderValue::from_static("2"),
            );
        }
        response
    }
}

impl From<rusqlite::Error> for ApiError {
    fn from(error: rusqlite::Error) -> Self {
        tracing::error!(%error, "database error");
        ApiError::internal()
    }
}

/// Maps a Silicon Accounts failure that happened during a non-sign-in call.
pub fn accounts_error(error: &silicon_accounts_client::Error) -> ApiError {
    tracing::warn!(code = error.code(), status = ?error.status(), transport = error.is_transport(), "Silicon Accounts request failed");
    ApiError::dependency("Silicon Accounts")
}
