//! `ring config …`, `ring voice ls`, `ring device …`, `ring notifications …`

use std::collections::BTreeMap;

use ring_proto::{
    ContextMode, GreetingsPatch, Notifications, RepresentativePatch, Settings, SettingsPatch,
    VoicemailPatchSettings, limits,
};
use serde_json::{Map, Value, json};

use super::{Ctx, check_text, id};
use crate::cli::{ConfigCommand, DeviceCommand, NotificationsCommand};
use crate::error::{CliError, CliResult};
use crate::output;

const KEYS: &str = "voice, representative.default_context, representative.context_mode, \
representative.context_required, voicemail.enabled, voicemail.greetings.busy, voicemail.greetings.declined, \
voicemail.greetings.missed";

pub async fn config(ctx: &Ctx, command: ConfigCommand) -> CliResult<()> {
    match command {
        ConfigCommand::Show => {
            let signed = ctx.signed_in()?;
            let settings = signed.client.settings().await?;
            output::emit(ctx.json, &settings, || output::settings(&settings));
        }
        ConfigCommand::Set { settings: input } => {
            let patch = parse_patch(&input)?;
            let signed = ctx.signed_in()?;
            let settings = signed.client.update_settings(&patch).await?;
            print_updated(ctx, &settings, &patch);
        }
        ConfigCommand::Reset { keys } => {
            let patch = reset_patch(&keys)?;
            let signed = ctx.signed_in()?;
            let settings = signed.client.update_settings(&patch).await?;
            print_updated(ctx, &settings, &patch);
        }
    }
    Ok(())
}

fn print_updated(ctx: &Ctx, settings: &Settings, patch: &SettingsPatch) {
    let withdraws = patch
        .representative
        .as_ref()
        .is_some_and(|r| r.default_context.is_some() || r.context_mode.is_some());
    output::emit(ctx.json, settings, || {
        let mut text = output::settings(settings);
        if withdraws {
            text.push_str(
                "\nChanging the default context or its mode withdrew your context approvals.",
            );
        }
        text
    });
}

/// Accepts dotted keys (`{"representative.context_mode":"append"}`) and nested objects alike.
fn parse_patch(input: &str) -> CliResult<SettingsPatch> {
    let value: Value = serde_json::from_str(input).map_err(|e| {
        CliError::input(format!("That is not valid JSON: {e}"))
            .hint("Example: ring config set '{\"voice\":\"vesper\"}'")
    })?;
    let Value::Object(object) = value else {
        return Err(CliError::input("Settings must be a JSON object.")
            .hint("Example: ring config set '{\"voice\":\"vesper\"}'"));
    };
    let mut flat = BTreeMap::new();
    flatten("", object, &mut flat);
    if flat.is_empty() {
        return Err(CliError::input("No settings given.").hint(format!("Keys: {KEYS}")));
    }

    let mut patch = SettingsPatch::default();
    for (key, value) in flat {
        let wrong = |expected: &str| CliError::input(format!("{key} must be {expected}."));
        match key.as_str() {
            "voice" => {
                let voice = value
                    .as_str()
                    .filter(|v| !v.trim().is_empty())
                    .ok_or_else(|| wrong("a voice id from `ring voice ls`"))?;
                patch.voice = Some(voice.trim().to_owned());
            }
            "representative.default_context" => {
                let text = value
                    .as_str()
                    .ok_or_else(|| wrong("a string (\"\" clears it)"))?;
                let length = ring_proto::text_len(text);
                if length > limits::CONTEXT {
                    return Err(CliError::text_too_long(&key, limits::CONTEXT, length));
                }
                representative(&mut patch).default_context = Some(text.to_owned());
            }
            "representative.context_mode" => {
                let mode = match value.as_str() {
                    Some("append") => ContextMode::Append,
                    Some("prepend") => ContextMode::Prepend,
                    Some("overwrite") => ContextMode::Overwrite,
                    _ => return Err(wrong("append, prepend or overwrite")),
                };
                representative(&mut patch).context_mode = Some(mode);
            }
            "representative.context_required" => {
                representative(&mut patch).context_required =
                    Some(value.as_bool().ok_or_else(|| wrong("true or false"))?);
            }
            "voicemail.enabled" => {
                voicemail(&mut patch).enabled =
                    Some(value.as_bool().ok_or_else(|| wrong("true or false"))?);
            }
            "voicemail.greetings.busy"
            | "voicemail.greetings.declined"
            | "voicemail.greetings.missed" => {
                let greeting = match &value {
                    Value::Null => None,
                    Value::String(text) => {
                        check_text(&key, text, limits::GREETING)?;
                        Some(text.clone())
                    }
                    _ => return Err(wrong("a string, or null for the default")),
                };
                let greetings = greetings(&mut patch);
                match key.rsplit('.').next() {
                    Some("busy") => greetings.busy = Some(greeting),
                    Some("declined") => greetings.declined = Some(greeting),
                    _ => greetings.missed = Some(greeting),
                }
            }
            "revision" => return Err(CliError::input("revision is set by the server.")),
            _ => {
                return Err(CliError::input(format!("Unknown setting {key:?}."))
                    .hint(format!("Keys: {KEYS}")));
            }
        }
    }
    Ok(patch)
}

fn flatten(prefix: &str, object: Map<String, Value>, out: &mut BTreeMap<String, Value>) {
    for (key, value) in object {
        let full = if prefix.is_empty() {
            key
        } else {
            format!("{prefix}.{key}")
        };
        match value {
            Value::Object(inner) => flatten(&full, inner, out),
            other => {
                out.insert(full, other);
            }
        }
    }
}

fn representative(patch: &mut SettingsPatch) -> &mut RepresentativePatch {
    patch.representative.get_or_insert_with(Default::default)
}

fn voicemail(patch: &mut SettingsPatch) -> &mut VoicemailPatchSettings {
    patch.voicemail.get_or_insert_with(Default::default)
}

fn greetings(patch: &mut SettingsPatch) -> &mut GreetingsPatch {
    voicemail(patch)
        .greetings
        .get_or_insert_with(Default::default)
}

fn reset_patch(keys: &[String]) -> CliResult<SettingsPatch> {
    let mut patch = SettingsPatch::default();
    for key in keys {
        match key.trim() {
            "voice" => patch.voice = Some(ring_proto::DEFAULT_VOICE.to_owned()),
            "representative.default_context" => {
                representative(&mut patch).default_context = Some(String::new())
            }
            "representative.context_mode" => {
                representative(&mut patch).context_mode = Some(ContextMode::Append)
            }
            "representative.context_required" => {
                representative(&mut patch).context_required = Some(false)
            }
            "representative" => {
                *representative(&mut patch) = RepresentativePatch {
                    default_context: Some(String::new()),
                    context_mode: Some(ContextMode::Append),
                    context_required: Some(false),
                };
            }
            "voicemail.enabled" => voicemail(&mut patch).enabled = Some(true),
            "voicemail.greetings.busy" => greetings(&mut patch).busy = Some(None),
            "voicemail.greetings.declined" => greetings(&mut patch).declined = Some(None),
            "voicemail.greetings.missed" => greetings(&mut patch).missed = Some(None),
            "voicemail.greetings" | "voicemail" => {
                *greetings(&mut patch) = GreetingsPatch {
                    busy: Some(None),
                    declined: Some(None),
                    missed: Some(None),
                };
                if key == "voicemail" {
                    voicemail(&mut patch).enabled = Some(true);
                }
            }
            other => {
                return Err(
                    CliError::input(format!("Unknown setting {other:?}.")).hint(format!(
                        "Keys: {KEYS}, or the groups representative, voicemail, voicemail.greetings"
                    )),
                );
            }
        }
    }
    Ok(patch)
}

pub async fn voices(ctx: &Ctx) -> CliResult<()> {
    let signed = ctx.signed_in()?;
    let list = signed.client.voices().await?;
    let current = signed.client.settings().await.ok().map(|s| s.voice);
    let value = json!({ "items": list.items, "default": list.default, "current": current });
    output::emit(ctx.json, &value, || {
        let mut lines: Vec<String> = list
            .items
            .iter()
            .map(|v| {
                let mark = if current.as_deref() == Some(v.id.as_str()) {
                    "*"
                } else {
                    " "
                };
                let default = if v.id == list.default {
                    "  (default)"
                } else {
                    ""
                };
                format!(
                    "{mark} {:<10} {:<10} {}{default}",
                    v.id, v.name, v.description
                )
            })
            .collect();
        lines.push("Choose one: ring config set '{\"voice\":\"ID\"}'".into());
        lines.join("\n")
    });
    Ok(())
}

pub async fn device(ctx: &Ctx, command: DeviceCommand) -> CliResult<()> {
    let signed = ctx.signed_in()?;
    match command {
        DeviceCommand::Ls => {
            let devices = signed.client.devices().await?;
            output::emit(ctx.json, &json!({ "items": devices }), || {
                if devices.is_empty() {
                    "No devices.".into()
                } else {
                    devices
                        .iter()
                        .map(output::device)
                        .collect::<Vec<_>>()
                        .join("\n")
                }
            });
        }
        DeviceCommand::Revoke { id: device_id } => {
            let device_id = id("device id", &device_id)?;
            signed.client.revoke_device(&device_id).await?;
            let this_one = device_id == signed.session.device_id;
            if this_one {
                signed.state.remove_session()?;
            }
            output::emit(
                ctx.json,
                &json!({ "device_id": device_id, "revoked": true, "signed_out": this_one }),
                || {
                    if this_one {
                        format!(
                            "Revoked {device_id}, which was this CLI's session; you are signed out."
                        )
                    } else {
                        format!("Revoked {device_id}.")
                    }
                },
            );
        }
    }
    Ok(())
}

pub async fn notifications(ctx: &Ctx, command: NotificationsCommand) -> CliResult<()> {
    let signed = ctx.signed_in()?;
    let status: Notifications = match command {
        NotificationsCommand::Status => signed.client.notifications().await?,
        NotificationsCommand::Enable => signed.client.set_notifications(true).await?,
        NotificationsCommand::Disable => signed.client.set_notifications(false).await?,
    };
    output::emit(ctx.json, &status, || {
        let ting = &status.ting;
        let mut text = if ting.enabled {
            format!(
                "Ting notifications are on{}.",
                ting.subscription_id
                    .as_deref()
                    .map(|s| format!(" ({s})"))
                    .unwrap_or_default()
            )
        } else {
            "Ting notifications are off.".to_owned()
        };
        if let Some(error) = &ting.last_error {
            text.push_str(&format!("\nLast error: {error}"));
        }
        text.push_str(if ting.enabled {
            "\nTing delivers to your webhook; set it with `ting webhook URL`."
        } else {
            "\nTurn them on: ring notifications enable"
        });
        text
    });
    Ok(())
}
