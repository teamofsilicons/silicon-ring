//! `ring context …`: preview and approve the stored default context.

use ring_proto::{ApproveContext, ContextAction, ContextPreviewRequest, limits};
use serde_json::json;

use super::{Ctx, actor, check_text, id};
use crate::cli::ContextCommand;
use crate::error::CliResult;
use crate::output;

pub async fn run(ctx: &Ctx, command: ContextCommand) -> CliResult<()> {
    match command {
        ContextCommand::Show {
            init,
            accept,
            context,
        } => {
            if let Some(text) = &context.context {
                check_text("--context", text, limits::CONTEXT)?;
            }
            let request = match (init, accept) {
                (Some(target), _) => ContextPreviewRequest {
                    action: ContextAction::Init,
                    to: Some(actor(&target)?),
                    ringid: None,
                    context: context.context,
                    context_mode: context.context_mode.map(Into::into),
                },
                (None, Some(ringid)) => ContextPreviewRequest {
                    action: ContextAction::Accept,
                    to: None,
                    ringid: Some(id("call id", &ringid)?),
                    context: context.context,
                    context_mode: context.context_mode.map(Into::into),
                },
                (None, None) => unreachable!("clap requires --init or --accept"),
            };
            let signed = ctx.signed_in()?;
            let preview = signed.client.context_preview(&request).await?;
            output::emit(ctx.json, &preview, || {
                output::context_preview(&preview, true)
            });
        }
        ContextCommand::Approve {
            preparation_id,
            valid_for,
        } => {
            let preparation_id = id("preparation id", &preparation_id)?;
            let signed = ctx.signed_in()?;
            let approval = signed
                .client
                .approve_context(&ApproveContext {
                    preparation_id,
                    valid_for_seconds: Some(valid_for),
                })
                .await?;
            output::emit(ctx.json, &approval, || {
                format!(
                    "Approved your default context (revision {}, {}) until {}.\nNow repeat the command that asked for it.",
                    approval.default_revision,
                    approval.mode.as_str(),
                    output::time(&approval.expires_at)
                )
            });
        }
        ContextCommand::Ls => {
            let signed = ctx.signed_in()?;
            let items = signed.client.context_approvals().await?;
            output::emit(ctx.json, &json!({ "items": items }), || {
                if items.is_empty() {
                    "No approvals.".into()
                } else {
                    items
                        .iter()
                        .map(output::approval)
                        .collect::<Vec<_>>()
                        .join("\n")
                }
            });
        }
        ContextCommand::Revoke { approval_id } => {
            let approval_id = id("approval id", &approval_id)?;
            let signed = ctx.signed_in()?;
            signed.client.revoke_context_approval(&approval_id).await?;
            output::emit(
                ctx.json,
                &json!({ "approval_id": approval_id, "revoked": true }),
                || format!("Revoked {approval_id}."),
            );
        }
    }
    Ok(())
}
