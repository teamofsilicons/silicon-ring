//! Command implementations on top of ring-core.

mod calls;
mod context;
mod help;
mod login;
mod messages;
mod settings;
mod voicemail;
mod watch;

use std::io::Read;

use ring_core::Client;
use serde_json::json;

use crate::cli::{Command, Group};
use crate::error::{CliError, CliResult, SIGN_IN};
use crate::state::{State, StoredSession};

pub use help::print_help;

/// Options every command shares.
pub struct Ctx {
    pub json: bool,
    api_url: Option<String>,
    idempotency_key: Option<String>,
}

/// A client that carries the saved session, with the session itself.
pub struct SignedIn {
    pub client: Client,
    pub session: StoredSession,
    pub state: State,
}

impl Ctx {
    pub fn new(json: bool, api_url: Option<String>, idempotency_key: Option<String>) -> Self {
        // An empty RING_API_URL means "not set", as it does for most tools.
        let api_url = api_url.filter(|url| !url.trim().is_empty());
        Ctx {
            json,
            api_url,
            idempotency_key,
        }
    }

    /// The origin named by --api-url or RING_API_URL, normalized.
    pub fn explicit_origin(&self) -> CliResult<Option<String>> {
        self.api_url.as_deref().map(normalize_origin).transpose()
    }

    /// The origin for commands that need no session.
    pub fn origin(&self) -> CliResult<String> {
        Ok(self
            .explicit_origin()?
            .unwrap_or_else(|| ring_core::DEFAULT_API_URL.to_owned()))
    }

    pub fn client(&self, origin: &str, token: Option<String>) -> CliResult<Client> {
        Ok(Client::new(origin, token, &user_agent())?)
    }

    /// The saved session and a client for it. The session is only ever sent to the origin that
    /// issued it; naming another origin is an error rather than a silent switch.
    pub fn signed_in(&self) -> CliResult<SignedIn> {
        let state = State::locate()?;
        let session = state.session()?.ok_or_else(|| {
            CliError::new("not_signed_in", "Not signed in.").hint(format!("Sign in: {SIGN_IN}"))
        })?;
        if let Some(explicit) = self.explicit_origin()?
            && explicit != session.api_url
        {
            return Err(CliError::new(
                "session_api_mismatch",
                format!("The saved session belongs to {}, not {explicit}.", session.api_url),
            )
            .hint(format!(
                "Leave out --api-url and RING_API_URL to use {}, or sign in for {explicit}: \
                 silicon-accounts login --app ring -q | ring login --slt-stdin --api-url {explicit}",
                session.api_url
            )));
        }
        let client = self.client(&session.api_url, Some(session.session_token.clone()))?;
        Ok(SignedIn {
            client,
            session,
            state,
        })
    }

    /// The caller's --idempotency-key, or a fresh one.
    pub fn idempotency_key(&self) -> CliResult<String> {
        match &self.idempotency_key {
            Some(key) => {
                ring_core::validate_idempotency_key(key)?;
                Ok(key.clone())
            }
            None => Ok(ring_core::new_idempotency_key()),
        }
    }
}

pub fn normalize_origin(raw: &str) -> CliResult<String> {
    let url = ring_core::parse_api_url(raw)?;
    Ok(url.as_str().trim_end_matches('/').to_owned())
}

pub fn user_agent() -> String {
    format!(
        "ring-cli/{} ({}; {})",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH
    )
}

/// A mutation that may have reached the server: tell the caller how to retry it safely.
pub fn with_retry_key(error: ring_core::Error, key: &str) -> CliError {
    let uncertain = matches!(error, ring_core::Error::Network(_));
    let mut error = CliError::from(error);
    if uncertain {
        error.hint = Some(format!(
            "The request may have reached the server. Retry with --idempotency-key {key} so it cannot happen twice."
        ));
        error.details = json!({ "idempotency_key": key });
    }
    error
}

// ---- echoing the command line -------------------------------------------------------------

/// The command line as typed, quoted for a POSIX shell, with the program shown as `ring`.
pub fn original_command() -> String {
    command_line(std::env::args().skip(1).collect())
}

/// The command as typed with `flag` set to `value`: the "More:" hint for the next page, which
/// keeps every filter (a cursor carries only the position).
pub fn next_page_command(flag: &str, value: &str) -> String {
    let inline = format!("{flag}=");
    let mut args = Vec::new();
    let mut typed = std::env::args().skip(1);
    while let Some(arg) = typed.next() {
        if arg == flag {
            typed.next();
        } else if !arg.starts_with(&inline) {
            args.push(arg);
        }
    }
    args.extend([flag.to_owned(), value.to_owned()]);
    command_line(args)
}

fn command_line(args: Vec<String>) -> String {
    std::iter::once("ring".to_owned())
        .chain(args.iter().map(|arg| shell_quote(arg)))
        .collect::<Vec<_>>()
        .join(" ")
}

fn shell_quote(arg: &str) -> String {
    let plain = !arg.is_empty()
        && arg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./:=@,+%".contains(c));
    if plain {
        arg.to_owned()
    } else {
        format!("'{}'", arg.replace('\'', r"'\''"))
    }
}

// ---- input checks -------------------------------------------------------------------------

/// Text over its limit is rejected before sending, counted in code points like the server.
pub fn check_text(field: &str, text: &str, limit: usize) -> CliResult<()> {
    if text.trim().is_empty() {
        return Err(CliError::input(format!("{field} is empty.")));
    }
    let length = ring_proto::text_len(text);
    if length > limit {
        return Err(CliError::text_too_long(field, limit, length));
    }
    Ok(())
}

pub fn actor(input: &str) -> CliResult<String> {
    ring_proto::normalize_actor_ref(input).ok_or_else(|| {
        CliError::input(format!("{input:?} is not an actor reference."))
            .hint("Use c:handle, si:handle, @c:handle or an account uuid.")
    })
}

pub fn id(kind: &str, input: &str) -> CliResult<String> {
    let trimmed = input.trim();
    if trimmed.is_empty()
        || trimmed.len() > 200
        || trimmed
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || c == '/')
    {
        return Err(CliError::input(format!("{input:?} is not a valid {kind}.")));
    }
    Ok(trimmed.to_owned())
}

/// Reads message text from a file, or stdin for `-`. One trailing newline is dropped.
pub fn read_text_file(path: &str) -> CliResult<String> {
    let mut text = String::new();
    if path == "-" {
        std::io::stdin()
            .read_to_string(&mut text)
            .map_err(|e| CliError::io("Could not read stdin", e))?;
    } else {
        text = std::fs::read_to_string(path)
            .map_err(|e| CliError::io(&format!("Could not read {path}"), e))?;
    }
    let text = text
        .strip_suffix('\n')
        .map(|t| t.strip_suffix('\r').unwrap_or(t))
        .unwrap_or(&text);
    Ok(text.to_owned())
}

// ---- dispatch -----------------------------------------------------------------------------

pub async fn run(ctx: &Ctx, command: Option<Command>) -> CliResult<()> {
    let Some(command) = command else {
        return print_help(&[]);
    };
    match command {
        Command::Accounts => help::accounts(ctx),
        Command::Login(args) => login::login(ctx, args).await,
        Command::Logout => login::logout(ctx).await,
        Command::Profile(Group { command }) => match command {
            None => print_help(&["profile"]),
            Some(crate::cli::ProfileCommand::Show { actor }) => login::profile(ctx, actor).await,
        },
        Command::Call(Group { command }) => match command {
            None => print_help(&["call"]),
            Some(command) => calls::run(ctx, command).await,
        },
        Command::Send(args) => messages::send(ctx, args).await,
        Command::Delegation(Group { command }) => match command {
            None => print_help(&["delegation"]),
            Some(command) => messages::delegation(ctx, command).await,
        },
        Command::Context(Group { command }) => match command {
            None => print_help(&["context"]),
            Some(command) => context::run(ctx, command).await,
        },
        Command::Voicemail(Group { command }) => match command {
            None => print_help(&["voicemail"]),
            Some(command) => voicemail::run(ctx, command).await,
        },
        Command::Config(Group { command }) => match command {
            None => print_help(&["config"]),
            Some(command) => settings::config(ctx, command).await,
        },
        Command::Voice(Group { command }) => match command {
            None => print_help(&["voice"]),
            Some(crate::cli::VoiceCommand::Ls) => settings::voices(ctx).await,
        },
        Command::Device(Group { command }) => match command {
            None => print_help(&["device"]),
            Some(command) => settings::device(ctx, command).await,
        },
        Command::Notifications(Group { command }) => match command {
            None => print_help(&["notifications"]),
            Some(command) => settings::notifications(ctx, command).await,
        },
        Command::Docs(args) => help::docs(ctx, args),
        Command::Report(args) => help::report(ctx, args).await,
    }
}
