//! `ring call …`

use ring_core::CallFilter;
use ring_proto::{
    AcceptCall, CallState, ContextAction, ContextPreview, ContextPreviewRequest, CreateCall,
    DeclineCall, EndReason, InvitationState, RecordingStatus, limits,
};
use serde_json::json;

use super::{Ctx, SignedIn, actor, check_text, id, watch, with_retry_key};
use crate::cli::{CallCommand, ContextArgs, DirectionArg};
use crate::error::{CliError, CliResult};
use crate::output;
use crate::state::write_output;

pub async fn run(ctx: &Ctx, command: CallCommand) -> CliResult<()> {
    match command {
        CallCommand::Init {
            actor: target,
            context,
            start,
            live,
        } => init(ctx, &target, context, start, live).await,
        CallCommand::Accept {
            ringid,
            invitation,
            context,
            start,
            live,
        } => accept(ctx, &ringid, invitation, context, start, live).await,
        CallCommand::Decline {
            ringid,
            invitation,
            reason,
            give_no_reason,
        } => decline(ctx, &ringid, invitation, reason, give_no_reason).await,
        CallCommand::Cut { ringid } => cut(ctx, &ringid).await,
        CallCommand::Invite {
            actor: target,
            ringid,
        } => invite(ctx, &target, &ringid).await,
        CallCommand::Silence { ringid, invitation } => silence(ctx, &ringid, invitation).await,
        CallCommand::Show { ringid } => show(ctx, &ringid).await,
        CallCommand::History {
            state,
            direction,
            with,
            since,
            until,
            limit,
            cursor,
        } => {
            let filter = CallFilter {
                states: state.into_iter().map(Into::into).collect(),
                direction: match direction {
                    Some(DirectionArg::Incoming) => Some(ring_proto::Direction::Incoming),
                    Some(DirectionArg::Outgoing) => Some(ring_proto::Direction::Outgoing),
                    Some(DirectionArg::All) | None => None,
                },
                with: with.as_deref().map(actor).transpose()?,
                since,
                until,
                limit,
                cursor,
            };
            history(ctx, filter).await
        }
        CallCommand::Transcript {
            ringid,
            after_seq,
            limit,
        } => transcript(ctx, &ringid, after_seq, limit).await,
        CallCommand::Recording {
            ringid,
            audio_out,
            overwrite,
        } => recording(ctx, &ringid, audio_out, overwrite).await,
        CallCommand::Watch { ringid } => {
            let ringid = id("call id", &ringid)?;
            let signed = ctx.signed_in()?;
            watch::watch(ctx, &signed, &ringid).await
        }
    }
}

fn check_context(context: &ContextArgs, start: &Option<String>) -> CliResult<()> {
    if let Some(text) = &context.context {
        check_text("--context", text, limits::CONTEXT)?;
    }
    if let Some(text) = start {
        check_text("--start", text, limits::START)?;
    }
    Ok(())
}

async fn init(
    ctx: &Ctx,
    target: &str,
    context: ContextArgs,
    start: Option<String>,
    live: bool,
) -> CliResult<()> {
    let to = actor(target)?;
    check_context(&context, &start)?;
    let signed = ctx.signed_in()?;
    let key = ctx.idempotency_key()?;
    let request = CreateCall {
        to: to.clone(),
        start,
        context: context.context.clone(),
        context_mode: context.context_mode.map(Into::into),
        approval_id: None,
    };
    let created = match signed.client.create_call(&request, Some(&key)).await {
        Ok(created) => created,
        Err(e) if is_code(&e, "context_approval_required") => {
            let preview = ContextPreviewRequest {
                action: ContextAction::Init,
                to: Some(to),
                ringid: None,
                context: context.context,
                context_mode: context.context_mode.map(Into::into),
            };
            return Err(approval_follow_up(&signed, e, preview).await);
        }
        Err(e) if is_code(&e, "already_in_call") => {
            return Err(CliError::from(e).hint(
                "A silicon is on one call at a time. Find it with `ring call history --state ringing,active,voicemail`, \
                 then end it (`ring call cut RINGID`) or invite this person into it (`ring call invite ACTOR RINGID`).",
            ));
        }
        Err(e) => return Err(with_retry_key(e, &key)),
    };

    let ringid = created.call.ringid.clone();
    let callee = output::who(&created.invitation.to);
    let busy = created.invitation.state == InvitationState::Busy
        || created.call.state == CallState::Voicemail;
    if live && ctx.json {
        output::event_line("call.created", &created);
    } else {
        output::emit(ctx.json, &created, || {
            if busy {
                format!(
                    "{callee} is busy, so {ringid} went to voicemail.\n\
                     Leave a message: ring voicemail leave {ringid} --text \"…\""
                )
            } else {
                format!(
                    "Ringing {callee}: {ringid}\nIt rings for up to 60 s; Ting tells you when it is answered.\n\
                     Watch:  ring call watch {ringid}\nCancel: ring call cut {ringid}"
                )
            }
        });
    }
    if live {
        watch::follow(ctx, &signed, &ringid).await
    } else {
        Ok(())
    }
}

async fn accept(
    ctx: &Ctx,
    ringid: &str,
    invitation: Option<String>,
    context: ContextArgs,
    start: Option<String>,
    live: bool,
) -> CliResult<()> {
    let ringid = id("call id", ringid)?;
    let invitation_id = invitation
        .as_deref()
        .map(|i| id("invitation id", i))
        .transpose()?;
    check_context(&context, &start)?;
    let signed = ctx.signed_in()?;
    let request = AcceptCall {
        invitation_id,
        start,
        context: context.context.clone(),
        context_mode: context.context_mode.map(Into::into),
        approval_id: None,
    };
    let accepted = match signed.client.accept(&ringid, &request).await {
        Ok(accepted) => accepted,
        Err(e) if is_code(&e, "context_approval_required") => {
            let preview = ContextPreviewRequest {
                action: ContextAction::Accept,
                to: None,
                ringid: Some(ringid.clone()),
                context: context.context,
                context_mode: context.context_mode.map(Into::into),
            };
            return Err(approval_follow_up(&signed, e, preview).await);
        }
        Err(e) if is_code(&e, "invitation_not_ringing") => {
            return Err(CliError::from(e).hint(format!(
                "It was already answered, declined or cancelled. Check: ring call show {ringid}"
            )));
        }
        Err(e) if is_code(&e, "invitation_expired") => {
            return Err(CliError::from(e)
                .hint("It stopped ringing. Call back with `ring call init ACTOR`."));
        }
        Err(e) if is_code(&e, "already_in_call") => {
            return Err(CliError::from(e).hint(
                "A silicon is on one call at a time. End the other call first; find it with \
                 `ring call history --state ringing,active,voicemail`.",
            ));
        }
        Err(e) => return Err(e.into()),
    };
    if live && ctx.json {
        output::event_line("call.accepted", &accepted);
    } else {
        output::emit(ctx.json, &accepted, || {
            format!(
                "Answered {ringid}. Your representative is joining and speaks first.\n\
                 Watch:   ring call watch {ringid}\nHang up: ring call cut {ringid}"
            )
        });
    }
    if live {
        watch::follow(ctx, &signed, &ringid).await
    } else {
        Ok(())
    }
}

async fn decline(
    ctx: &Ctx,
    ringid: &str,
    invitation: Option<String>,
    reason: Option<String>,
    give_no_reason: bool,
) -> CliResult<()> {
    let ringid = id("call id", ringid)?;
    let invitation_id = invitation
        .as_deref()
        .map(|i| id("invitation id", i))
        .transpose()?;
    if let Some(text) = &reason {
        check_text("--reason", text, limits::DECLINE_REASON)?;
    }
    let signed = ctx.signed_in()?;
    let request = DeclineCall {
        invitation_id,
        reason: reason.clone(),
        give_no_reason,
    };
    let call = signed.client.decline(&ringid, &request).await?;
    output::emit(ctx.json, &json!({ "call": call }), || match &reason {
        Some(reason) => format!("Declined {ringid}: {reason}"),
        None => format!("Declined {ringid} without a reason."),
    });
    Ok(())
}

async fn cut(ctx: &Ctx, ringid: &str) -> CliResult<()> {
    let ringid = id("call id", ringid)?;
    let signed = ctx.signed_in()?;
    let call = signed.client.leave(&ringid).await?;
    output::emit(ctx.json, &json!({ "call": call }), || {
        if call.end_reason == Some(EndReason::Cancelled) {
            format!("Cancelled {ringid}.")
        } else if call.state == CallState::Ended {
            format!("Left {ringid}; the call has ended.")
        } else {
            format!("Left {ringid}; the others are still talking.")
        }
    });
    Ok(())
}

async fn invite(ctx: &Ctx, target: &str, ringid: &str) -> CliResult<()> {
    let to = actor(target)?;
    let ringid = id("call id", ringid)?;
    let signed = ctx.signed_in()?;
    let key = ctx.idempotency_key()?;
    let invited = signed
        .client
        .invite(&ringid, &to, Some(&key))
        .await
        .map_err(|e| with_retry_key(e, &key))?;
    output::emit(ctx.json, &invited, || {
        format!(
            "Inviting {} into {ringid} ({}). It rings until {}.",
            output::who(&invited.invitation.to),
            invited.invitation.invitation_id,
            output::time(&invited.invitation.expires_at)
        )
    });
    Ok(())
}

async fn silence(ctx: &Ctx, ringid: &str, invitation: Option<String>) -> CliResult<()> {
    let ringid = id("call id", ringid)?;
    let invitation_id = invitation
        .as_deref()
        .map(|i| id("invitation id", i))
        .transpose()?;
    let signed = ctx.signed_in()?;
    let call = signed
        .client
        .silence(&ringid, invitation_id.as_deref())
        .await?;
    output::emit(ctx.json, &json!({ "call": call }), || {
        format!("Silenced {ringid}. The caller hears it ring until it expires or you answer.")
    });
    Ok(())
}

async fn show(ctx: &Ctx, ringid: &str) -> CliResult<()> {
    let ringid = id("call id", ringid)?;
    let signed = ctx.signed_in()?;
    let call = signed.client.call(&ringid).await?;
    output::emit(ctx.json, &call, || output::call_details(&call));
    Ok(())
}

async fn history(ctx: &Ctx, filter: CallFilter) -> CliResult<()> {
    let signed = ctx.signed_in()?;
    let page = signed.client.calls(&filter).await?;
    let me = signed.session.account.uuid.clone();
    output::emit(ctx.json, &page, || {
        if page.items.is_empty() {
            return "No calls.".into();
        }
        let mut lines: Vec<String> = page
            .items
            .iter()
            .map(|c| output::call_row(c, &me))
            .collect();
        if let Some(cursor) = &page.next_cursor {
            lines.push(format!("More: ring call history --cursor {cursor}"));
        }
        lines.join("\n")
    });
    Ok(())
}

async fn transcript(ctx: &Ctx, ringid: &str, after_seq: u64, limit: Option<u32>) -> CliResult<()> {
    let ringid = id("call id", ringid)?;
    let signed = ctx.signed_in()?;
    let page = signed.client.transcript(&ringid, after_seq, limit).await?;
    output::emit(ctx.json, &page, || {
        if page.items.is_empty() {
            return "No transcript entries.".into();
        }
        let mut lines: Vec<String> = page.items.iter().map(output::entry).collect();
        if page.items.len() as u32 >= limit.unwrap_or(200) {
            lines.push(format!(
                "More: ring call transcript {ringid} --after-seq {}",
                page.next_after_seq
            ));
        }
        lines.join("\n")
    });
    Ok(())
}

async fn recording(
    ctx: &Ctx,
    ringid: &str,
    audio_out: Option<std::path::PathBuf>,
    overwrite: bool,
) -> CliResult<()> {
    let ringid = id("call id", ringid)?;
    let signed = ctx.signed_in()?;
    let recording = signed.client.recording(&ringid).await?;
    let status = match recording.status {
        RecordingStatus::None => "none",
        RecordingStatus::Recording => "recording",
        RecordingStatus::Processing => "processing",
        RecordingStatus::Ready => "ready",
        RecordingStatus::Failed => "failed",
    };
    let mut saved = None;
    if let Some(path) = &audio_out {
        let url = match (&recording.status, &recording.url) {
            (RecordingStatus::Ready, Some(url)) => url,
            (RecordingStatus::Recording | RecordingStatus::Processing, _) => {
                return Err(CliError::new(
                    "recording_not_ready",
                    format!("The recording is still {status}."),
                )
                .hint(format!(
                    "Try again in a minute: ring call recording {ringid} --audio-out {}",
                    path.display()
                ))
                .retryable(true));
            }
            _ => {
                return Err(CliError::new(
                    "recording_unavailable",
                    format!("There is no recording to download ({status})."),
                )
                .exit(4));
            }
        };
        let bytes = signed.client.download(url).await?;
        write_output(path, &bytes, overwrite)?;
        saved = Some(path.display().to_string());
    }
    let result = json!({
        "ringid": ringid,
        "status": recording.status,
        "duration_ms": recording.duration_ms,
        "mime_type": recording.mime_type,
        "expires_at": recording.expires_at,
        "url": recording.url,
        "saved_to": saved,
    });
    output::emit(ctx.json, &result, || {
        let mut text = format!("Recording {status}");
        if let Some(ms) = recording.duration_ms {
            text.push_str(&format!(", {}", output::duration_ms(ms)));
        }
        text.push('.');
        match &saved {
            Some(path) => text.push_str(&format!("\nSaved to {path}.")),
            None if recording.status == RecordingStatus::Ready => text.push_str(&format!(
                "\nDownload: ring call recording {ringid} --audio-out call.wav"
            )),
            None => {}
        }
        text
    });
    Ok(())
}

fn is_code(error: &ring_core::Error, code: &str) -> bool {
    error.code() == code
}

/// Turns `409 context_approval_required` into the approval follow-up: the preview, the exact
/// approve command, and the original command to repeat. Exits 5.
async fn approval_follow_up(
    signed: &SignedIn,
    error: ring_core::Error,
    request: ContextPreviewRequest,
) -> CliError {
    let details = match &error {
        ring_core::Error::Api { error, .. } => error.details.clone(),
        _ => json!({}),
    };
    let from_details = serde_json::from_value::<ContextPreview>(details.clone())
        .or_else(|_| serde_json::from_value::<ContextPreview>(details["preview"].clone()));
    let preview = match from_details {
        Ok(preview) => preview,
        Err(_) => match signed.client.context_preview(&request).await {
            Ok(preview) => preview,
            Err(_) => {
                return CliError::from(error)
                    .hint("Preview it with `ring context show`, approve it with `ring context approve`, then repeat this command.");
            }
        },
    };
    let approve = format!("ring context approve {} --for 1h", preview.preparation_id);
    let repeat = original_command();
    CliError::new(
        "context_approval_required",
        "Your stored default context needs your approval before Ring uses it.",
    )
    .hint(format!("Run `{approve}`, then repeat: {repeat}"))
    .details(json!({ "preview": preview, "approve_command": approve, "repeat_command": repeat }))
    .body(format!(
        "Ring will seed your representative with your stored default context.\n\n{}\n\n\
             Approve it:\n  {approve}\nThen repeat your command:\n  {repeat}",
        output::context_preview(&preview, false)
    ))
    .exit(5)
}

/// The command line as typed, quoted for a POSIX shell, with the program shown as `ring`.
fn original_command() -> String {
    std::iter::once("ring".to_owned())
        .chain(std::env::args().skip(1).map(|arg| shell_quote(&arg)))
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
