//! `ring send` and `ring delegation …`: talking to your representative.

use ring_proto::{DelegationStatus, MessageKind, MessageStatus, SendMessage, limits};

use super::{Ctx, check_text, id, read_text_file, with_retry_key};
use crate::cli::{DelegationCommand, SendArgs};
use crate::error::{CliError, CliResult};
use crate::output;

pub async fn send(ctx: &Ctx, args: SendArgs) -> CliResult<()> {
    let ringid = id("call id", &args.ringid)?;
    let text = match (args.text, args.text_file) {
        (Some(text), None) => text,
        (None, Some(path)) => read_text_file(&path)?,
        _ => {
            return Err(CliError::input(
                "Give the message as TEXT or with --text-file, not both.",
            ));
        }
    };
    check_text("text", &text, limits::MESSAGE)?;
    let delegation_id = args
        .delegation_id
        .as_deref()
        .map(|d| id("delegation id", d))
        .transpose()?;
    let kind: MessageKind = args.kind.into();

    let signed = ctx.signed_in()?;
    let key = ctx.idempotency_key()?;
    let request = SendMessage {
        kind,
        text,
        delegation_id: delegation_id.clone(),
    };
    let sent = match signed
        .client
        .send_message(&ringid, &request, Some(&key))
        .await
    {
        Ok(sent) => sent,
        Err(e) if e.code() == "conflict" => {
            return Err(CliError::from(e).hint(format!(
                "Your representative is not live on {ringid}. Check it with `ring call show {ringid}`."
            )));
        }
        Err(e) => return Err(with_retry_key(e, &key)),
    };
    output::emit(ctx.json, &sent, || {
        let how = match sent.status {
            MessageStatus::Delivered => "Delivered",
            MessageStatus::Queued => "Queued",
        };
        let to = delegation_id
            .map(|d| format!(" for {d}"))
            .unwrap_or_default();
        format!(
            "{how} {}{to} ({}, seq {}).",
            kind.as_str(),
            sent.message_id,
            sent.seq
        )
    });
    Ok(())
}

pub async fn delegation(ctx: &Ctx, command: DelegationCommand) -> CliResult<()> {
    let signed = ctx.signed_in()?;
    match command {
        DelegationCommand::Ls { ringid, status } => {
            let ringid = id("call id", &ringid)?;
            let items = signed
                .client
                .delegations(&ringid, status.map(Into::into))
                .await?;
            output::emit(ctx.json, &serde_json::json!({ "items": items }), || {
                if items.is_empty() {
                    return "No delegations.".into();
                }
                items
                    .iter()
                    .map(|d| output::delegation(d, false))
                    .collect::<Vec<_>>()
                    .join("\n")
            });
        }
        DelegationCommand::Show { id: delegation_id } => {
            let delegation_id = id("delegation id", &delegation_id)?;
            let delegation = signed.client.delegation(&delegation_id).await?;
            output::emit(ctx.json, &delegation, || {
                let mut text = output::delegation(&delegation, true);
                // A delegation closes when its call ends; only an open one can be answered.
                if delegation.status == DelegationStatus::Open {
                    text.push_str(&format!(
                        "\nAnswer: ring send {} commentary \"…\" --delegation-id {}",
                        delegation.ringid, delegation.delegation_id
                    ));
                }
                text
            });
        }
    }
    Ok(())
}
