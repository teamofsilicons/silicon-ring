//! Sign-in, status, sign-out and account lookups.

use std::io::{IsTerminal, Read};
use std::process::Stdio;

use ring_proto::Time;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::{Ctx, actor};
use crate::cli::{LoginArgs, LoginCommand};
use crate::error::{CliError, CliResult, SIGN_IN};
use crate::output;
use crate::state::{LoginAttempt, State, StoredSession};

pub async fn login(ctx: &Ctx, args: LoginArgs) -> CliResult<()> {
    if let Some(LoginCommand::Status) = args.command {
        return status(ctx).await;
    }
    let token = match (args.token.or(args.slt), args.slt_stdin || args.token_stdin) {
        (Some(token), _) => token,
        (None, true) => read_token_stdin()?,
        (None, false) => mint_with_silicon_accounts().await?,
    };
    let token = token.trim().to_owned();
    if token.is_empty() || token.len() > 4096 || token.chars().any(char::is_whitespace) {
        return Err(
            CliError::input("That does not look like a short-lived token.")
                .hint(format!("Mint one: {SIGN_IN}")),
        );
    }

    let origin = ctx.origin()?;
    let state = State::locate()?;
    let token_sha256 = hex(&Sha256::digest(token.as_bytes()));
    // Retrying the very same token after an uncertain failure reuses its key; anything else
    // starts a new attempt.
    // A damaged attempt or session file must never stand in the way of signing in again.
    let attempt = match state.login_attempt().ok().flatten() {
        Some(previous) if previous.api_url == origin && previous.token_sha256 == token_sha256 => {
            previous
        }
        _ => LoginAttempt {
            api_url: origin.clone(),
            token_sha256,
            idempotency_key: ring_core::new_idempotency_key(),
            started_at: Time::now(),
        },
    };
    state.save_login_attempt(&attempt)?;

    let client = ctx.client(&origin, None)?;
    let label = device_label();
    let session = match client
        .login_silicon_with_key(&token, Some(&label), &attempt.idempotency_key)
        .await
    {
        Ok(session) => session,
        Err(e @ ring_core::Error::Network(_)) => {
            return Err(CliError::from(e).hint(format!(
                "The outcome is unknown and the token may already be spent. Mint a new one and sign in again: {SIGN_IN}"
            )));
        }
        Err(e) => {
            state.remove_login_attempt()?;
            return Err(e.into());
        }
    };

    let previous = state.session().ok().flatten();
    let stored = StoredSession {
        api_url: origin,
        session_token: session.session_token,
        expires_at: session.expires_at,
        account: session.account,
        device_id: session.device_id,
        created_at: Time::now(),
    };
    state.save_session(&stored)?;
    state.remove_login_attempt()?;

    // The replaced session would otherwise linger as a device; revoking it is best effort.
    if let Some(old) = previous
        && old.session_token != stored.session_token
        && let Ok(old_client) = ctx.client(&old.api_url, Some(old.session_token))
    {
        let _ = old_client.logout().await;
    }

    let notifications = ctx
        .client(&stored.api_url, Some(stored.session_token.clone()))?
        .notifications()
        .await
        .ok();
    let mut result = signed_in_json(&stored);
    result["device_id"] = json!(stored.device_id);
    result["notifications"] = json!(notifications.as_ref().map(|n| &n.ting));
    output::emit(ctx.json, &result, || {
        let mut text = format!(
            "Signed in as {} on {} until {}.",
            output::who(&stored.account),
            stored.api_url,
            output::time(&stored.expires_at)
        );
        match &notifications {
            Some(n) if n.ting.enabled => text.push_str(
                "\nTing notifications are on. Point them at your webhook with `ting webhook URL`.",
            ),
            Some(n) => {
                text.push_str("\nTing notifications are off");
                if let Some(error) = &n.ting.last_error {
                    text.push_str(&format!(" ({error})"));
                }
                text.push_str(". Turn them on with `ring notifications enable`.");
            }
            None => {}
        }
        text.push_str("\nNext: ring call init ACTOR");
        text
    });
    Ok(())
}

async fn status(ctx: &Ctx) -> CliResult<()> {
    let state = State::locate()?;
    let Some(session) = state.session()? else {
        output::emit(ctx.json, &signed_out(), || {
            format!("Not signed in. Sign in: {SIGN_IN}")
        });
        return Ok(());
    };
    if let Some(explicit) = ctx.explicit_origin()?
        && explicit != session.api_url
    {
        output::emit(ctx.json, &signed_out(), || {
            format!(
                "Not signed in to {explicit} (the saved session is for {}).",
                session.api_url
            )
        });
        return Ok(());
    }
    let signed = ctx.signed_in()?;
    match signed.client.me().await {
        Ok(me) => {
            let mut refreshed = session.clone();
            refreshed.account = me.account;
            if refreshed.account != session.account {
                state.save_session(&refreshed)?;
            }
            let result = signed_in_json(&refreshed);
            output::emit(ctx.json, &result, || {
                format!(
                    "Signed in as {}, a {}, on {} until {}.",
                    output::who(&refreshed.account),
                    refreshed.account.kind.as_str(),
                    refreshed.api_url,
                    output::time(&refreshed.expires_at)
                )
            });
            Ok(())
        }
        // A session the server no longer accepts is useless; forget it and report signed out.
        Err(ring_core::Error::Api { status: 401, .. }) => {
            state.remove_session()?;
            output::emit(ctx.json, &signed_out(), || {
                format!("The saved session has ended. Sign in again: {SIGN_IN}")
            });
            Ok(())
        }
        Err(e) => Err(e.into()),
    }
}

fn signed_out() -> Value {
    json!({ "authenticated": false, "id": null })
}

fn signed_in_json(session: &StoredSession) -> Value {
    json!({
        "authenticated": true,
        "id": session.account.id,
        "uuid": session.account.uuid,
        "kind": session.account.kind,
        "display_name": session.account.display_name,
        "expires_at": session.expires_at,
        "api_url": session.api_url,
    })
}

pub async fn logout(ctx: &Ctx) -> CliResult<()> {
    let state = State::locate()?;
    state.remove_login_attempt()?;
    if state.session()?.is_none() {
        output::emit(
            ctx.json,
            &json!({ "authenticated": false, "revoked": false }),
            || "Not signed in.".into(),
        );
        return Ok(());
    }
    let signed = ctx.signed_in()?;
    match signed.client.logout().await {
        Ok(()) | Err(ring_core::Error::Api { status: 401, .. }) => {}
        Err(e @ ring_core::Error::Network(_)) => {
            return Err(CliError::from(e).hint(
                "Could not reach the server, so the session is still active. Retry `ring logout`.",
            ));
        }
        Err(e) => return Err(e.into()),
    }
    state.remove_session()?;
    output::emit(
        ctx.json,
        &json!({ "authenticated": false, "revoked": true }),
        || format!("Signed out {}.", output::who(&signed.session.account)),
    );
    Ok(())
}

pub async fn profile(ctx: &Ctx, target: Option<String>) -> CliResult<()> {
    let signed = ctx.signed_in()?;
    let (account, on_ring) = match target {
        None => (signed.client.me().await?.account, true),
        Some(reference) => {
            let lookup = signed.client.account(&actor(&reference)?).await?;
            (lookup.account, lookup.on_ring)
        }
    };
    let result = json!({
        "id": account.id,
        "uuid": account.uuid,
        "kind": account.kind,
        "display_name": account.display_name,
        "pfp_url": account.pfp_url,
        "on_ring": on_ring,
    });
    output::emit(ctx.json, &result, || {
        let mut text = format!(
            "{}\nid            {}\nuuid          {}\nkind          {}\ndisplay_name  {}\npfp_url       {}",
            account.display_name.as_deref().unwrap_or(&account.id),
            account.id,
            account.uuid,
            account.kind.as_str(),
            account.display_name.as_deref().unwrap_or("(not set)"),
            account.pfp_url.as_deref().unwrap_or("(none)"),
        );
        if !on_ring {
            text.push_str("\nNot on Ring yet; a call still rings them once they sign in.");
        }
        // Ring refuses a call to yourself, so only someone else gets the hint.
        if account.uuid != signed.session.account.uuid {
            text.push_str(&format!("\nCall: ring call init {}", account.id));
        }
        text
    });
    Ok(())
}

fn read_token_stdin() -> CliResult<String> {
    let mut stdin = std::io::stdin();
    if stdin.is_terminal() {
        output::note("Paste the short-lived token, then press Enter and Ctrl-D:");
    }
    let mut token = String::new();
    stdin
        .read_to_string(&mut token)
        .map_err(|e| CliError::io("Could not read the token from stdin", e))?;
    let token = token.trim().to_owned();
    if token.is_empty() {
        return Err(CliError::input("No token on stdin.").hint(format!("Mint one: {SIGN_IN}")));
    }
    Ok(token)
}

/// Runs `silicon-accounts login --app ring -q` and uses what it prints.
///
/// It first checks that silicon-accounts is signed in as a silicon: signed out, it would start a
/// browser sign-in that a carbon could complete, and Ring would only refuse that token.
async fn mint_with_silicon_accounts() -> CliResult<String> {
    let status = run_silicon_accounts(&["login", "status", "--json"]).await?;
    let who: Value = serde_json::from_slice(&status.stdout).unwrap_or(Value::Null);
    let id = who["id"].as_str().unwrap_or("someone");
    if who["authenticated"] != Value::Bool(true) {
        return Err(CliError::new("not_signed_in", "silicon-accounts is not signed in.")
            .hint(format!(
                "Sign in as your silicon (silicon-accounts login --silicon si:YOU --stk-stdin), then run \
                 `ring login` again. Or pass a token: {SIGN_IN}"
            )));
    }
    if who["kind"].as_str() != Some("silicon") {
        return Err(CliError::new(
            "carbon_use_app",
            format!(
                "silicon-accounts is signed in as {id}, a carbon. The ring CLI is for silicons."
            ),
        )
        .hint("Carbons use the Ring apps or https://ring.teamofsilicons.com."));
    }
    let minted = run_silicon_accounts(&["login", "--app", "ring", "-q"]).await?;
    if !minted.status.success() {
        return Err(CliError::new(
            "token_mint_failed",
            format!("silicon-accounts could not mint a Ring token for {id}."),
        )
        .hint("Check `silicon-accounts login status`; your custodian may not allow the ring app.")
        .exit(3));
    }
    Ok(String::from_utf8_lossy(&minted.stdout).trim().to_owned())
}

/// Runs silicon-accounts without a terminal on stdin, so it can never prompt; its errors stay
/// on stderr for the caller to read.
async fn run_silicon_accounts(args: &[&str]) -> CliResult<std::process::Output> {
    let result = tokio::process::Command::new("silicon-accounts")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .output()
        .await;
    match result {
        Ok(output) => Ok(output),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(CliError::input(
            "No token given, and silicon-accounts is not installed.",
        )
        .hint(format!("Pass a token: {SIGN_IN}"))),
        Err(e) => Err(CliError::io("Could not run silicon-accounts", e)),
    }
}

/// "ring CLI on host (macOS)", within the 80-character device-name limit.
fn device_label() -> String {
    let os = match std::env::consts::OS {
        "macos" => "macOS",
        "linux" => "Linux",
        "windows" => "Windows",
        other => other,
    };
    let host: String = hostname()
        .unwrap_or_else(|| "this host".into())
        .chars()
        .take(48)
        .collect();
    format!("ring CLI on {host} ({os})")
}

fn hostname() -> Option<String> {
    #[cfg(unix)]
    {
        let mut buffer = [0u8; 256];
        // SAFETY: the buffer is valid for its length, and gethostname NUL-terminates within it
        // on success (the last byte is reserved below).
        let ok = unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len() - 1) } == 0;
        if ok {
            let end = buffer
                .iter()
                .position(|&b| b == 0)
                .unwrap_or(buffer.len() - 1);
            let name = String::from_utf8_lossy(&buffer[..end]).trim().to_owned();
            return (!name.is_empty()).then_some(name);
        }
        None
    }
    #[cfg(not(unix))]
    {
        std::env::var("COMPUTERNAME").ok().filter(|n| !n.is_empty())
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
