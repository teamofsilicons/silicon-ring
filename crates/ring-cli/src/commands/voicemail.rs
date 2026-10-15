//! `ring voicemail …`

use ring_core::VoicemailFilter;
use ring_proto::{
    AudioStatus, GreetingsPatch, SettingsPatch, VoicemailPatchSettings, VoicemailReason, limits,
};
use serde_json::json;

use super::{Ctx, actor, check_text, id, read_text_file, with_retry_key};
use crate::cli::{GreetingCommand, Group, VoicemailCommand};
use crate::error::{CliError, CliResult};
use crate::output;
use crate::state::write_output;

pub async fn run(ctx: &Ctx, command: VoicemailCommand) -> CliResult<()> {
    match command {
        VoicemailCommand::Ls {
            all,
            from,
            since,
            until,
            limit,
            cursor,
        } => {
            let filter = VoicemailFilter {
                unread_only: !all,
                from: from.as_deref().map(actor).transpose()?,
                since,
                until,
                limit,
                cursor,
            };
            let signed = ctx.signed_in()?;
            let page = signed.client.voicemails(&filter).await?;
            output::emit(ctx.json, &page, || {
                if page.items.is_empty() {
                    return if all {
                        "No voicemail.".into()
                    } else {
                        "No unread voicemail.".into()
                    };
                }
                let mut lines: Vec<String> = page.items.iter().map(output::voicemail_row).collect();
                if let Some(cursor) = &page.next_cursor {
                    lines.push(format!(
                        "More: ring voicemail ls{} --cursor {cursor}",
                        if all { " --all" } else { "" }
                    ));
                }
                lines.join("\n")
            });
        }
        VoicemailCommand::Show {
            id: voicemail_id,
            audio_out,
            overwrite,
        } => {
            let voicemail_id = id("voicemail id", &voicemail_id)?;
            let signed = ctx.signed_in()?;
            let voicemail = signed.client.voicemail(&voicemail_id).await?;
            let mut saved = None;
            if let Some(path) = &audio_out {
                let url = match (voicemail.audio_status, &voicemail.audio_url) {
                    (AudioStatus::Ready, Some(url)) => url,
                    (AudioStatus::Failed, _) => {
                        return Err(CliError::new(
                            "audio_unavailable",
                            "This voicemail's audio could not be produced.",
                        )
                        .hint("Its transcript, if any, is shown by `ring voicemail show`.")
                        .exit(4));
                    }
                    _ => {
                        return Err(CliError::new("audio_not_ready", "The audio is not ready yet.")
                            .hint(format!("Try again in a minute: ring voicemail show {voicemail_id} --audio-out {}", path.display()))
                            .retryable(true));
                    }
                };
                let bytes = signed.client.download(url).await?;
                write_output(path, &bytes, overwrite)?;
                saved = Some(path.display().to_string());
            }
            let mut value = serde_json::to_value(&voicemail).expect("voicemail serializes");
            value["saved_to"] = json!(saved);
            output::emit(ctx.json, &value, || {
                let mut text = output::voicemail_details(&voicemail);
                if let Some(path) = &saved {
                    text.push_str(&format!("\nSaved the audio to {path}."));
                }
                if !voicemail.read {
                    text.push_str(&format!(
                        "\nMark it read: ring voicemail read {voicemail_id}"
                    ));
                }
                text
            });
        }
        VoicemailCommand::Read { id: voicemail_id } => mark(ctx, &voicemail_id, true).await?,
        VoicemailCommand::Unread { id: voicemail_id } => mark(ctx, &voicemail_id, false).await?,
        VoicemailCommand::Delete { id: voicemail_id } => {
            let voicemail_id = id("voicemail id", &voicemail_id)?;
            let signed = ctx.signed_in()?;
            signed.client.delete_voicemail(&voicemail_id).await?;
            output::emit(
                ctx.json,
                &json!({ "voicemail_id": voicemail_id, "deleted": true }),
                || format!("Deleted {voicemail_id}."),
            );
        }
        VoicemailCommand::Leave {
            ringid,
            text,
            text_file,
        } => {
            let ringid = id("call id", &ringid)?;
            let text = match (text, text_file) {
                (Some(text), None) => text,
                (None, Some(path)) => read_text_file(&path)?,
                _ => {
                    return Err(CliError::input(
                        "Give the message with --text or --text-file, not both.",
                    ));
                }
            };
            check_text("--text", &text, limits::VOICEMAIL_TEXT)?;
            let signed = ctx.signed_in()?;
            let key = ctx.idempotency_key()?;
            let voicemail = match signed
                .client
                .leave_voicemail(&ringid, &text, Some(&key))
                .await
            {
                Ok(voicemail) => voicemail,
                Err(e) if matches!(e.code(), "conflict" | "forbidden" | "call_ended") => {
                    return Err(CliError::from(e).hint(
                        "You can leave a message only on a call you made while it is in voicemail state \
                         (Ting sends ring.voicemail.offered).",
                    ));
                }
                Err(e) => return Err(with_retry_key(e, &key)),
            };
            output::emit(ctx.json, &voicemail, || {
                format!(
                    "Left {} for {}. It is being spoken in your voice and delivered.",
                    voicemail.voicemail_id,
                    output::who(&voicemail.to)
                )
            });
        }
        VoicemailCommand::Greeting(Group { command }) => match command {
            None => crate::commands::print_help(&["voicemail", "greeting"])?,
            Some(command) => greeting(ctx, command).await?,
        },
    }
    Ok(())
}

async fn mark(ctx: &Ctx, voicemail_id: &str, read: bool) -> CliResult<()> {
    let voicemail_id = id("voicemail id", voicemail_id)?;
    let signed = ctx.signed_in()?;
    let voicemail = signed.client.mark_voicemail(&voicemail_id, read).await?;
    output::emit(ctx.json, &voicemail, || {
        format!(
            "Marked {voicemail_id} {}.",
            if read { "read" } else { "unread" }
        )
    });
    Ok(())
}

async fn greeting(ctx: &Ctx, command: GreetingCommand) -> CliResult<()> {
    let signed = ctx.signed_in()?;
    let settings = match command {
        GreetingCommand::Show => signed.client.settings().await?,
        GreetingCommand::Set { when, text } => {
            check_text("--text", &text, limits::GREETING)?;
            signed
                .client
                .update_settings(&greeting_patch(when.into(), Some(text)))
                .await?
        }
        GreetingCommand::Reset { when } => {
            signed
                .client
                .update_settings(&greeting_patch(when.into(), None))
                .await?
        }
    };
    let greetings = &settings.voicemail.greetings;
    output::emit(
        ctx.json,
        &json!({ "enabled": settings.voicemail.enabled, "greetings": greetings }),
        || {
            let show = |reason: VoicemailReason| {
                greetings
                    .get(reason)
                    .map(|t| format!("\"{t}\""))
                    .unwrap_or_else(|| "(built-in default)".into())
            };
            let mut text = format!(
                "busy      {}\ndeclined  {}\nmissed    {}",
                show(VoicemailReason::Busy),
                show(VoicemailReason::Declined),
                show(VoicemailReason::Missed)
            );
            if !settings.voicemail.enabled {
                text.push_str("\nVoicemail is off. Turn it on: ring config set '{\"voicemail.enabled\":true}'");
            }
            text
        },
    );
    Ok(())
}

/// `Some(text)` sets the greeting; `None` sends JSON null, which restores the default.
fn greeting_patch(reason: VoicemailReason, text: Option<String>) -> SettingsPatch {
    let mut greetings = GreetingsPatch::default();
    let slot = match reason {
        VoicemailReason::Busy => &mut greetings.busy,
        VoicemailReason::Declined => &mut greetings.declined,
        VoicemailReason::Missed => &mut greetings.missed,
    };
    *slot = Some(text);
    SettingsPatch {
        voicemail: Some(VoicemailPatchSettings {
            enabled: None,
            greetings: Some(greetings),
        }),
        ..SettingsPatch::default()
    }
}
