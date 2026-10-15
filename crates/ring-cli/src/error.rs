//! The CLI's error envelope and exit codes (`udd/cli.md`).

use serde_json::{Value, json};

pub const SIGN_IN: &str = "silicon-accounts login --app ring -q | ring login --slt-stdin";

/// Everything a failed command reports: `{"error":{"code","message","hint","retryable","details"}}`
/// plus the process exit code.
#[derive(Debug)]
pub struct CliError {
    pub code: String,
    pub message: String,
    pub hint: Option<String>,
    pub retryable: bool,
    pub details: Value,
    pub exit: i32,
    /// Text printed before the error in text mode, such as a context preview.
    pub body: Option<String>,
}

pub type CliResult<T> = Result<T, CliError>;

impl CliError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        CliError {
            code: code.to_owned(),
            message: message.into(),
            hint: None,
            retryable: false,
            details: json!({}),
            exit: exit_code(code, None),
            body: None,
        }
    }

    pub fn input(message: impl Into<String>) -> Self {
        CliError::new("invalid_input", message)
    }

    pub fn io(what: &str, error: std::io::Error) -> Self {
        CliError::new("local_io", format!("{what}: {error}"))
            .hint("Check the path, its permissions and the free disk space.")
    }

    pub fn text_too_long(field: &str, limit: usize, length: usize) -> Self {
        CliError::new(
            "text_too_long",
            format!("{field} is {length} characters; the limit is {limit}."),
        )
        .hint("Shorten it. Limits count Unicode code points, and Ring never truncates text.")
        .details(json!({ "field": field, "limit": limit, "length": length }))
    }

    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    pub fn details(mut self, details: Value) -> Self {
        self.details = details;
        self
    }

    pub fn retryable(mut self, retryable: bool) -> Self {
        self.retryable = retryable;
        self
    }

    pub fn body(mut self, body: String) -> Self {
        self.body = Some(body);
        self
    }

    pub fn exit(mut self, code: i32) -> Self {
        self.exit = code;
        self
    }

    pub fn to_json(&self) -> Value {
        json!({ "error": {
            "code": self.code,
            "message": self.message,
            "hint": self.hint,
            "retryable": self.retryable,
            "details": self.details,
        }})
    }
}

impl From<ring_core::Error> for CliError {
    fn from(error: ring_core::Error) -> Self {
        use ring_core::Error as E;
        match error {
            E::Api { status, error } => {
                let mut out = CliError {
                    exit: exit_code(&error.code, Some(status)),
                    code: error.code,
                    message: error.message,
                    hint: error.hint,
                    retryable: error.retryable,
                    details: error.details,
                    body: None,
                };
                if out.hint.is_none() && matches!(out.code.as_str(), "unauthenticated" | "session_expired") {
                    out.hint = Some(format!("Sign in again: {SIGN_IN}"));
                }
                out
            }
            E::NotSignedIn => CliError::new("not_signed_in", "Not signed in.").hint(format!("Sign in: {SIGN_IN}")),
            E::Network(message) => CliError::new("network", message)
                .hint("Check the connection and the API URL (--api-url or RING_API_URL), then retry.")
                .retryable(true),
            E::Protocol(message) => CliError::new("protocol", message)
                .hint("This ring may be older than the server. Update it with `silicon-apps update ring`."),
            E::Input(message) => CliError::input(message),
            E::Media(message) => CliError::new("media", message),
        }
    }
}

/// Exit codes: 0 ok, 1 failed or network, 2 invalid input, 3 not signed in or not allowed,
/// 4 not found, 5 conflict or a required follow-up, 6 rate limited, 130 interrupted.
pub fn exit_code(code: &str, status: Option<u16>) -> i32 {
    match code {
        "invalid_input" | "text_too_long" | "context_required" | "usage" => 2,
        "not_signed_in"
        | "unauthenticated"
        | "session_expired"
        | "forbidden"
        | "csrf"
        | "carbon_use_app"
        | "silicon_use_cli"
        | "not_participant"
        | "session_api_mismatch" => 3,
        "not_found" | "account_not_found" => 4,
        "conflict"
        | "invitation_not_ringing"
        | "already_in_call"
        | "context_approval_required"
        | "idempotency_conflict"
        | "call_ended"
        | "invitation_expired"
        | "file_exists" => 5,
        "rate_limited" => 6,
        "interrupted" => 130,
        _ => match status {
            Some(400 | 422) => 2,
            Some(401 | 403) => 3,
            Some(404) => 4,
            Some(409 | 410) => 5,
            Some(429) => 6,
            _ => 1,
        },
    }
}
