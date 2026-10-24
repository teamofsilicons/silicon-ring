//! Settings, voices, greetings, and the representative context with its explicit approvals.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use ring_proto::{
    Account, AccountKind, ApproveContext, ContextAction, ContextApproval, ContextMode,
    ContextPreview, ContextPreviewRequest, Greetings, Items, RepresentativeSettings, Settings,
    SettingsPatch, VoiceList, VoicemailReason, VoicemailSettings, limits, merge_context,
};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::json;

use crate::app::AppState;
use crate::calls::{enum_from, enum_str};
use crate::error::{ApiError, ApiResult};
use crate::extract::{ApiJson, Auth};
use crate::util::{check_len, new_id, now_ms, time};

const PREPARATION_TTL_MS: i64 = 15 * 60 * 1000;
const DEFAULT_APPROVAL_SECONDS: u64 = 3600;
const MAX_APPROVAL_SECONDS: u64 = 86_400;

pub struct StoredSettings {
    pub settings: Settings,
    pub context_revision: u64,
}

pub fn ensure_row(conn: &Connection, uuid: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO settings (account_uuid, voice, updated_at) VALUES (?1, ?2, ?3)",
        params![uuid, ring_proto::DEFAULT_VOICE, now_ms()],
    )?;
    Ok(())
}

pub fn load(conn: &Connection, uuid: &str) -> rusqlite::Result<StoredSettings> {
    let row = conn
        .query_row(
            "SELECT voice, default_context, context_mode, context_required, voicemail_enabled, greeting_busy, greeting_declined,
               greeting_missed, revision, context_revision FROM settings WHERE account_uuid = ?1",
            [uuid],
            |r| {
                let mode: String = r.get(2)?;
                Ok(StoredSettings {
                    settings: Settings {
                        voice: r.get(0)?,
                        representative: RepresentativeSettings {
                            default_context: r.get(1)?,
                            context_mode: enum_from(&mode).unwrap_or(ContextMode::Append),
                            context_required: r.get::<_, i64>(3)? != 0,
                        },
                        voicemail: VoicemailSettings {
                            enabled: r.get::<_, i64>(4)? != 0,
                            greetings: Greetings { busy: r.get(5)?, declined: r.get(6)?, missed: r.get(7)? },
                        },
                        revision: r.get::<_, i64>(8)? as u64,
                    },
                    context_revision: r.get::<_, i64>(9)? as u64,
                })
            },
        )
        .optional()?;
    Ok(row.unwrap_or_else(|| StoredSettings {
        settings: Settings {
            voice: ring_proto::DEFAULT_VOICE.into(),
            representative: RepresentativeSettings::default(),
            voicemail: VoicemailSettings::default(),
            revision: 1,
        },
        context_revision: 1,
    }))
}

/// What the caller hears before the beep, in the callee's voice.
pub fn greeting_text(settings: &Settings, callee: &Account, reason: VoicemailReason) -> String {
    if let Some(custom) = settings
        .voicemail
        .greetings
        .get(reason)
        .filter(|g| !g.trim().is_empty())
    {
        return custom.to_string();
    }
    let name = callee
        .display_name
        .clone()
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| {
            callee
                .id
                .split_once(':')
                .map(|(_, h)| h.to_string())
                .unwrap_or_else(|| callee.id.clone())
        });
    match reason {
        VoicemailReason::Busy if callee.kind == AccountKind::Silicon => {
            "The silicon you're trying to reach is currently talking to someone else. Please try again later, or leave a message after the beep.".into()
        }
        VoicemailReason::Busy => format!("{name} is on another call. Leave a message after the beep."),
        VoicemailReason::Declined => format!("{name} can't take your call right now. Leave a message after the beep."),
        VoicemailReason::Missed => format!("{name} isn't available right now. Leave a message after the beep."),
    }
}

/// Has ring-media prepare the account's three effective greetings in its voice, so a caller who
/// reaches voicemail hears the greeting at once instead of waiting for it to be synthesized.
pub fn prepare_greetings(app: &AppState, settings: &Settings, account: &Account) {
    if !settings.voicemail.enabled {
        return;
    }
    for reason in [
        VoicemailReason::Busy,
        VoicemailReason::Declined,
        VoicemailReason::Missed,
    ] {
        app.media
            .prepare_greeting(&settings.voice, &greeting_text(settings, account, reason));
    }
}

pub async fn get_settings(State(app): State<AppState>, auth: Auth) -> ApiResult<Json<Settings>> {
    Ok(Json(app.db.with(|c| load(c, auth.uuid()))?.settings))
}

pub async fn patch_settings(
    State(app): State<AppState>,
    auth: Auth,
    ApiJson(patch): ApiJson<SettingsPatch>,
) -> ApiResult<Json<Settings>> {
    if let Some(voice) = &patch.voice
        && !ring_proto::natural_voices().iter().any(|v| &v.id == voice)
    {
        return Err(ApiError::invalid(format!("{voice:?} is not a Ring voice."))
            .hint("List voices with GET /v1/voices."));
    }
    if let Some(rep) = &patch.representative {
        if !auth.is_silicon() {
            return Err(ApiError::invalid(
                "Representative settings apply to silicons only.",
            ));
        }
        if let Some(text) = &rep.default_context {
            check_len(
                "representative.default_context",
                text.trim(),
                limits::CONTEXT,
            )?;
        }
    }
    if let Some(greetings) = patch.voicemail.as_ref().and_then(|v| v.greetings.as_ref()) {
        for (field, value) in [
            ("busy", &greetings.busy),
            ("declined", &greetings.declined),
            ("missed", &greetings.missed),
        ] {
            if let Some(Some(text)) = value {
                check_len(
                    &format!("voicemail.greetings.{field}"),
                    text.trim(),
                    limits::GREETING,
                )?;
            }
        }
    }
    let uuid = auth.uuid().to_string();
    let (settings, greetings_changed) = app.db.tx(|tx| -> ApiResult<(Settings, bool)> {
        // A request still in flight when its Ring account was deleted must not bring its settings
        // back: they are what marks an account as using Ring again.
        if crate::deletion::gone(tx, &uuid)? {
            return Err(ApiError::session_expired());
        }
        ensure_row(tx, &uuid)?;
        let before = load(tx, &uuid)?;
        let mut s = before.settings.clone();
        if let Some(voice) = &patch.voice {
            s.voice = voice.clone();
        }
        let mut context_changed = false;
        if let Some(rep) = &patch.representative {
            if let Some(text) = &rep.default_context {
                let text = text.trim().to_string();
                context_changed |= text != s.representative.default_context;
                s.representative.default_context = text;
            }
            if let Some(mode) = rep.context_mode {
                context_changed |= mode != s.representative.context_mode;
                s.representative.context_mode = mode;
            }
            if let Some(required) = rep.context_required {
                s.representative.context_required = required;
            }
        }
        if let Some(vm) = &patch.voicemail {
            if let Some(enabled) = vm.enabled {
                s.voicemail.enabled = enabled;
            }
            if let Some(g) = &vm.greetings {
                let set = |slot: &mut Option<String>, value: &Option<Option<String>>| {
                    if let Some(v) = value {
                        *slot = v.as_ref().map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
                    }
                };
                set(&mut s.voicemail.greetings.busy, &g.busy);
                set(&mut s.voicemail.greetings.declined, &g.declined);
                set(&mut s.voicemail.greetings.missed, &g.missed);
            }
        }
        let greetings_changed = s.voice != before.settings.voice || s.voicemail != before.settings.voicemail;
        let mut revision = before.settings.revision;
        if s != before.settings {
            revision += 1;
        }
        s.revision = revision;
        let context_revision = before.context_revision + context_changed as u64;
        let now = now_ms();
        tx.execute(
            "UPDATE settings SET voice = ?2, default_context = ?3, context_mode = ?4, context_required = ?5, voicemail_enabled = ?6,
               greeting_busy = ?7, greeting_declined = ?8, greeting_missed = ?9, revision = ?10, context_revision = ?11, updated_at = ?12
             WHERE account_uuid = ?1",
            params![
                uuid,
                s.voice,
                s.representative.default_context,
                s.representative.context_mode.as_str(),
                s.representative.context_required as i64,
                s.voicemail.enabled as i64,
                s.voicemail.greetings.busy,
                s.voicemail.greetings.declined,
                s.voicemail.greetings.missed,
                revision as i64,
                context_revision as i64,
                now
            ],
        )?;
        if context_changed {
            tx.execute("UPDATE context_approvals SET revoked_at = ?2 WHERE account_uuid = ?1 AND revoked_at IS NULL", params![uuid, now])?;
        }
        Ok((s, greetings_changed))
    })?;
    if greetings_changed {
        prepare_greetings(&app, &settings, &auth.account);
    }
    Ok(Json(settings))
}

pub async fn voices() -> Json<VoiceList> {
    Json(VoiceList {
        items: ring_proto::natural_voices(),
        default: ring_proto::DEFAULT_VOICE.into(),
    })
}

// ---- context ----------------------------------------------------------------------------------

fn approval_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ContextApproval> {
    let mode: String = r.get(2)?;
    Ok(ContextApproval {
        approval_id: r.get(0)?,
        default_revision: r.get::<_, i64>(1)? as u64,
        mode: enum_from(&mode).unwrap_or(ContextMode::Append),
        created_at: time(r.get(3)?),
        expires_at: time(r.get(4)?),
    })
}

/// A live approval covering this account, default revision and mode (a specific one if given).
fn valid_approval(
    conn: &Connection,
    uuid: &str,
    revision: u64,
    mode: ContextMode,
    approval_id: Option<&str>,
) -> rusqlite::Result<Option<ContextApproval>> {
    conn.query_row(
        "SELECT approval_id, default_revision, mode, created_at, expires_at FROM context_approvals
         WHERE account_uuid = ?1 AND default_revision = ?2 AND mode = ?3 AND revoked_at IS NULL AND expires_at > ?4
           AND (?5 IS NULL OR approval_id = ?5)
         ORDER BY expires_at DESC LIMIT 1",
        params![uuid, revision as i64, mode.as_str(), now_ms(), approval_id],
        approval_from_row,
    )
    .optional()
}

struct Prepared {
    preview: ContextPreview,
}

fn prepare(
    conn: &Connection,
    uuid: &str,
    action: ContextAction,
    target: Option<&str>,
    supplied: Option<String>,
    mode: Option<ContextMode>,
    approval_id: Option<&str>,
) -> ApiResult<Prepared> {
    let supplied = supplied
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    if let Some(text) = &supplied {
        check_len("context", text, limits::CONTEXT)?;
    }
    let stored = load(conn, uuid)?;
    let rep = &stored.settings.representative;
    let mode = mode.unwrap_or(rep.context_mode);
    let (effective, uses_default) = merge_context(&rep.default_context, supplied.as_deref(), mode);
    check_len("context", &effective, limits::CONTEXT)?;
    let approval = if uses_default {
        valid_approval(conn, uuid, stored.context_revision, mode, approval_id)?
    } else {
        None
    };
    let now = now_ms();
    let preparation_id = new_id(ring_proto::prefix::PREPARATION);
    let expires_at = now + PREPARATION_TTL_MS;
    conn.execute(
        "INSERT INTO context_preparations (preparation_id, account_uuid, action, target, default_context, default_revision, supplied_context,
           mode, effective_context, uses_default, created_at, expires_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            preparation_id,
            uuid,
            enum_str(&action),
            target,
            rep.default_context,
            stored.context_revision as i64,
            supplied,
            mode.as_str(),
            effective,
            uses_default as i64,
            now,
            expires_at
        ],
    )?;
    Ok(Prepared {
        preview: ContextPreview {
            preparation_id,
            action,
            default_context: rep.default_context.clone(),
            default_revision: stored.context_revision,
            supplied_context: supplied,
            mode,
            effective_context: effective,
            uses_default,
            approval_required: uses_default && approval.is_none(),
            approval,
            expires_at: time(expires_at),
        },
    })
}

pub struct ResolvedContext {
    pub text: String,
}

/// The effective context for `init`/`accept`. Fails with `context_required` or
/// `context_approval_required` (whose details are the preview to approve).
pub fn resolve_context(
    app: &AppState,
    uuid: &str,
    action: ContextAction,
    target: &str,
    supplied: Option<String>,
    mode: Option<ContextMode>,
    approval_id: Option<String>,
) -> ApiResult<ResolvedContext> {
    let required = app
        .db
        .with(|c| load(c, uuid))?
        .settings
        .representative
        .context_required;
    let supplied_clean = supplied
        .as_ref()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    if required && supplied_clean.is_none() {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            ring_proto::codes::CONTEXT_REQUIRED,
            "Your settings require context for every call.",
        )
        .hint("Pass --context with what the representative should know."));
    }
    // A preview is stored only when approval is needed, so plain calls leave no rows behind.
    let outcome = app
        .db
        .tx(|tx| -> ApiResult<Result<String, ContextPreview>> {
            let prepared = prepare(
                tx,
                uuid,
                action,
                Some(target),
                supplied.clone(),
                mode,
                approval_id.as_deref(),
            )?;
            if prepared.preview.approval_required {
                Ok(Err(prepared.preview))
            } else {
                tx.execute(
                    "DELETE FROM context_preparations WHERE preparation_id = ?1",
                    [&prepared.preview.preparation_id],
                )?;
                Ok(Ok(prepared.preview.effective_context))
            }
        })?;
    match outcome {
        Ok(text) => Ok(ResolvedContext { text }),
        Err(preview) => {
            let approve = format!("ring context approve {} --for 1h", preview.preparation_id);
            Err(ApiError::new(
                StatusCode::CONFLICT,
                ring_proto::codes::CONTEXT_APPROVAL_REQUIRED,
                "Approve your stored default context first.",
            )
            .hint(format!(
                "Review the effective context, run `{approve}`, then repeat the command."
            ))
            .details(serde_json::to_value(&preview).unwrap_or_else(|_| json!({}))))
        }
    }
}

pub async fn preview(
    State(app): State<AppState>,
    auth: Auth,
    ApiJson(req): ApiJson<ContextPreviewRequest>,
) -> ApiResult<Json<ContextPreview>> {
    if !auth.is_silicon() {
        return Err(ApiError::forbidden(
            "Representative context is for silicons.",
        ));
    }
    let target = match req.action {
        ContextAction::Init => {
            let to = req
                .to
                .as_deref()
                .ok_or_else(|| ApiError::invalid("`to` is required for action init."))?;
            crate::accounts::resolve(&app, to).await?.id
        }
        ContextAction::Accept => {
            let ringid = req
                .ringid
                .clone()
                .ok_or_else(|| ApiError::invalid("`ringid` is required for action accept."))?;
            let data = app
                .db
                .with(|c| crate::calls::load(c, &ringid))?
                .ok_or_else(|| ApiError::not_found("Call"))?;
            if !data.can_view(auth.uuid()) {
                return Err(ApiError::not_participant());
            }
            ringid
        }
    };
    let preview = app
        .db
        .tx(|tx| {
            prepare(
                tx,
                auth.uuid(),
                req.action,
                Some(&target),
                req.context.clone(),
                req.context_mode,
                None,
            )
        })?
        .preview;
    Ok(Json(preview))
}

pub async fn approve(
    State(app): State<AppState>,
    auth: Auth,
    ApiJson(req): ApiJson<ApproveContext>,
) -> ApiResult<(StatusCode, Json<ContextApproval>)> {
    if !auth.is_silicon() {
        return Err(ApiError::forbidden(
            "Representative context is for silicons.",
        ));
    }
    let seconds = req.valid_for_seconds.unwrap_or(DEFAULT_APPROVAL_SECONDS);
    if seconds == 0 || seconds > MAX_APPROVAL_SECONDS {
        return Err(ApiError::invalid(
            "valid_for_seconds must be between 1 and 86400.",
        ));
    }
    let uuid = auth.uuid().to_string();
    let approval = app.db.tx(|tx| -> ApiResult<ContextApproval> {
        let prep: Option<(i64, String, i64)> = tx
            .query_row(
                "SELECT default_revision, mode, expires_at FROM context_preparations WHERE preparation_id = ?1 AND account_uuid = ?2",
                params![req.preparation_id, uuid],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let (revision, mode, expires_at) = prep.ok_or_else(|| ApiError::not_found("Preparation"))?;
        if expires_at <= now_ms() {
            return Err(ApiError::conflict("This preview expired.").hint("Run the preview again."));
        }
        let current = load(tx, &uuid)?.context_revision as i64;
        if current != revision {
            return Err(ApiError::conflict("Your default context changed after this preview.").hint("Preview again and approve the new text."));
        }
        let now = now_ms();
        let approval = ContextApproval {
            approval_id: new_id(ring_proto::prefix::APPROVAL),
            default_revision: revision as u64,
            mode: enum_from(&mode).unwrap_or(ContextMode::Append),
            created_at: time(now),
            expires_at: time(now + seconds as i64 * 1000),
        };
        tx.execute(
            "INSERT INTO context_approvals (approval_id, account_uuid, default_revision, mode, created_at, expires_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![approval.approval_id, uuid, revision, mode, now, approval.expires_at.millis()],
        )?;
        Ok(approval)
    })?;
    Ok((StatusCode::CREATED, Json(approval)))
}

pub async fn list_approvals(
    State(app): State<AppState>,
    auth: Auth,
) -> ApiResult<Json<Items<ContextApproval>>> {
    let items = app.db.with(|c| {
        let mut stmt = c.prepare(
            "SELECT approval_id, default_revision, mode, created_at, expires_at FROM context_approvals
             WHERE account_uuid = ?1 AND revoked_at IS NULL AND expires_at > ?2 ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map(params![auth.uuid(), now_ms()], approval_from_row)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
    })?;
    Ok(Json(Items { items }))
}

pub async fn revoke_approval(
    State(app): State<AppState>,
    auth: Auth,
    Path(approval_id): Path<String>,
) -> ApiResult<StatusCode> {
    let changed = app.db.with(|c| {
        c.execute(
            "UPDATE context_approvals SET revoked_at = ?3 WHERE approval_id = ?1 AND account_uuid = ?2 AND revoked_at IS NULL",
            params![approval_id, auth.uuid(), now_ms()],
        )
    })?;
    if changed == 0 {
        return Err(ApiError::not_found("Approval"));
    }
    Ok(StatusCode::NO_CONTENT)
}
