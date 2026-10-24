//! Ring sessions and devices (one session is one device), sign-in flows and device endpoints.

use std::time::Duration;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use ring_proto::{
    Account, AccountKind, CarbonLogin, Device, DevicePatch, DeviceUpdate, Event, Items, Me,
    Platform, PushEnvironment, PushInfo, PushKind, PushRegistration, Session, SiliconLogin, limits,
};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use silicon_accounts_client::{AccountForApp, TokenResponse};

use crate::accounts;
use crate::app::{App, AppState};
use crate::error::{ApiError, ApiResult};
use crate::extract::{
    ApiJson, ApiQuery, Auth, LOGIN_COOKIE, Peer, SESSION_COOKIE, client_ip, cookie,
};
use crate::util::{
    b64url, b64url_decode, check_len, clean, new_id, new_session_token, now_ms, sha256_hex, time,
};

/// Sessions without an Accounts expiry fall back to the longest Accounts sign-in (900 days).
const DEFAULT_SESSION_MS: i64 = 900 * 24 * 3600 * 1000;
const LOGIN_COOKIE_TTL_MS: i64 = 10 * 60 * 1000;
pub const NATIVE_REDIRECT_URI: &str = "com.teamofsilicons.ring:/auth/callback";
pub const LOOPBACK_REDIRECT_URI: &str = "http://127.0.0.1/auth/callback";

#[derive(Clone, Debug)]
pub struct SessionRow {
    pub device_id: String,
    pub account_uuid: String,
    pub name: Option<String>,
    pub platform: Option<Platform>,
    pub app_version: Option<String>,
    pub ring_enabled: bool,
    pub created_at: i64,
    pub last_seen_at: Option<i64>,
    pub expires_at: i64,
    pub revoked_at: Option<i64>,
}

pub fn parse_platform(s: &str) -> Option<Platform> {
    serde_json::from_value(serde_json::Value::String(s.to_string())).ok()
}

const SESSION_COLUMNS: &str = "s.device_id, s.account_uuid, s.name, s.platform, s.app_version, s.ring_enabled, s.created_at, s.last_seen_at, s.expires_at, s.revoked_at";

fn row_to_session(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionRow> {
    let platform: Option<String> = row.get(3)?;
    Ok(SessionRow {
        device_id: row.get(0)?,
        account_uuid: row.get(1)?,
        name: row.get(2)?,
        platform: platform.as_deref().and_then(parse_platform),
        app_version: row.get(4)?,
        ring_enabled: row.get::<_, i64>(5)? != 0,
        created_at: row.get(6)?,
        last_seen_at: row.get(7)?,
        expires_at: row.get(8)?,
        revoked_at: row.get(9)?,
    })
}

pub fn by_token_hash(
    conn: &Connection,
    hash: &str,
) -> rusqlite::Result<Option<(SessionRow, Account)>> {
    let session = conn
        .query_row(
            &format!("SELECT {SESSION_COLUMNS} FROM sessions s WHERE s.token_hash = ?1"),
            [hash],
            row_to_session,
        )
        .optional()?;
    let Some(session) = session else {
        return Ok(None);
    };
    let account = accounts::get(conn, &session.account_uuid)?
        .unwrap_or_else(|| accounts::placeholder(&session.account_uuid));
    Ok(Some((session, account)))
}

pub fn by_device(conn: &Connection, device_id: &str) -> rusqlite::Result<Option<SessionRow>> {
    conn.query_row(
        &format!("SELECT {SESSION_COLUMNS} FROM sessions s WHERE s.device_id = ?1"),
        [device_id],
        row_to_session,
    )
    .optional()
}

/// Live (not revoked, not expired) sessions of an account.
pub fn live_for_account(conn: &Connection, uuid: &str) -> rusqlite::Result<Vec<SessionRow>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {SESSION_COLUMNS} FROM sessions s WHERE s.account_uuid = ?1 AND s.revoked_at IS NULL AND s.expires_at > ?2 ORDER BY s.created_at"
    ))?;
    let rows = stmt.query_map(params![uuid, now_ms()], row_to_session)?;
    rows.collect()
}

pub fn render_device(
    app: &App,
    conn: &Connection,
    row: &SessionRow,
    current_device: &str,
) -> rusqlite::Result<Device> {
    let mut stmt = conn
        .prepare("SELECT kind, environment FROM push_tokens WHERE device_id = ?1 ORDER BY kind")?;
    let push = stmt
        .query_map([&row.device_id], |r| {
            let kind: String = r.get(0)?;
            let env: Option<String> = r.get(1)?;
            Ok((kind, env))
        })?
        .filter_map(|r| r.ok())
        .filter_map(|(kind, env)| {
            let kind: PushKind = serde_json::from_value(serde_json::Value::String(kind)).ok()?;
            let environment: Option<PushEnvironment> =
                env.and_then(|e| serde_json::from_value(serde_json::Value::String(e)).ok());
            Some(PushInfo { kind, environment })
        })
        .collect();
    Ok(Device {
        device_id: row.device_id.clone(),
        name: row.name.clone(),
        platform: row.platform,
        app_version: row.app_version.clone(),
        created_at: time(row.created_at),
        last_seen_at: row.last_seen_at.map(time),
        online: app.hub.is_online(&row.device_id),
        ring_enabled: row.ring_enabled,
        current: row.device_id == current_device,
        push,
    })
}

/// Creates a session for a fresh Accounts sign-in and returns it with the stored account.
fn create_session(
    app: &AppState,
    tokens: &TokenResponse,
    account: &AccountForApp,
    platform: Option<Platform>,
    name: Option<String>,
) -> ApiResult<(Session, Account)> {
    let token = new_session_token();
    let device_id = new_id(ring_proto::prefix::DEVICE);
    let now = now_ms();
    let expires_at = tokens
        .refresh_token_expires_at
        .map(|t| (t.unix_timestamp_nanos() / 1_000_000) as i64)
        .unwrap_or(now + DEFAULT_SESSION_MS);
    let refresh = tokens
        .refresh_token
        .as_ref()
        .ok_or_else(|| ApiError::dependency("Silicon Accounts"))?;
    let sealed = accounts::seal_refresh(app, &device_id, refresh.expose());
    let stored = app.db.tx(|tx| -> ApiResult<Account> {
        // Checked in the transaction, so a deletion that has begun always signs this session out.
        if crate::deletion::pending(tx, &account.uuid)? {
            accounts::spawn_revoke(app.clone(), refresh.expose().to_string());
            return Err(ApiError::conflict("Your Ring account is still being deleted.")
                .hint("Try again in a minute.")
                .retryable());
        }
        let stored = accounts::upsert_from_login(tx, account)?;
        tx.execute(
            "INSERT INTO sessions (device_id, token_hash, account_uuid, name, platform, ring_enabled, created_at, last_seen_at,
               expires_at, refresh_sealed, signed_in_at, introspected_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, ?6, ?7, ?8, ?6, ?6)",
            params![device_id, sha256_hex(token.as_bytes()), stored.uuid, name, platform.map(|p| p.as_str()), now, expires_at, sealed],
        )?;
        crate::settings::ensure_row(tx, &stored.uuid)?;
        if stored.kind == AccountKind::Silicon {
            tx.execute(
                "INSERT INTO ting_subscriptions (account_uuid, enabled, updated_at) VALUES (?1, 1, ?2)
                 ON CONFLICT(account_uuid) DO UPDATE SET enabled = 1, updated_at = excluded.updated_at",
                params![stored.uuid, now],
            )?;
        }
        Ok(stored)
    })?;
    tracing::info!(device_id = %device_id, kind = stored.kind.as_str(), "session created");
    Ok((
        Session {
            session_token: token,
            expires_at: time(expires_at),
            account: stored.clone(),
            device_id,
        },
        stored,
    ))
}

/// Claims a single-use credential by hash before it is sent anywhere, so it is never replayed.
fn claim_once(app: &App, kind: &str, secret: &str) -> ApiResult<()> {
    let hash = sha256_hex(format!("{kind}:{secret}").as_bytes());
    let inserted = app.db.with(|c| {
        c.execute(
            "INSERT OR IGNORE INTO auth_claims (hash, kind, created_at) VALUES (?1, ?2, ?3)",
            params![hash, kind, now_ms()],
        )
    })?;
    if inserted == 0 {
        return Err(
            ApiError::conflict("This sign-in token was already used.").hint(if kind == "slt" {
                "Mint a new one: silicon-accounts login --app ring -q | ring login --slt-stdin"
            } else {
                "Start the sign-in again."
            }),
        );
    }
    Ok(())
}

fn exchange_error(e: &silicon_accounts_client::Error, slt: bool) -> ApiError {
    if e.is_transport() || matches!(e.status(), Some(429) | Some(500..=599)) {
        tracing::warn!(code = e.code(), "Accounts exchange failed");
        let error = ApiError::dependency("Silicon Accounts");
        return if slt {
            error.hint("Mint a new token and try again; a token is never reused.")
        } else {
            error
        };
    }
    tracing::info!(code = e.code(), "Accounts refused an exchange");
    let error = ApiError::new(
        StatusCode::UNAUTHORIZED,
        ring_proto::codes::UNAUTHENTICATED,
        "Silicon Accounts did not accept this sign-in.",
    );
    if slt {
        error
            .hint("Mint a new token: silicon-accounts login --app ring -q | ring login --slt-stdin")
    } else {
        error.hint("Start the sign-in again.")
    }
}

fn rate_limit_auth(app: &App, headers: &HeaderMap, peer: Peer) -> ApiResult<()> {
    let ip = client_ip(headers, peer.0)
        .map(|ip| ip.to_string())
        .unwrap_or_default();
    app.limiter
        .check(&format!("auth:{ip}"), 30, Duration::from_secs(60))
}

// ---- POST /v1/auth/silicon --------------------------------------------------------------------

pub async fn silicon_login(
    State(app): State<AppState>,
    peer: Peer,
    headers: HeaderMap,
    ApiJson(body): ApiJson<SiliconLogin>,
) -> ApiResult<Json<Session>> {
    rate_limit_auth(&app, &headers, peer)?;
    let slt = body.slt.trim().to_string();
    if slt.is_empty() || slt.len() > 512 {
        return Err(ApiError::invalid("slt is required."));
    }
    let label = clean(body.label);
    if let Some(label) = &label {
        check_len("label", label, limits::DEVICE_NAME)?;
    }
    claim_once(&app, "slt", &slt)?;
    let tokens = app
        .accounts
        .app()
        .exchange_slt(&slt)
        .await
        .map_err(|e| exchange_error(&e, true))?;
    let account = tokens
        .account
        .clone()
        .ok_or_else(|| ApiError::dependency("Silicon Accounts"))?;
    if account.kind.as_str() != "silicon" {
        if let Some(refresh) = &tokens.refresh_token {
            accounts::spawn_revoke(app.clone(), refresh.expose().to_string());
        }
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            ring_proto::codes::CARBON_USE_APP,
            "The ring CLI is for silicons.",
        )
        .hint("Carbons use the Ring apps or https://ring.teamofsilicons.com."));
    }
    let (session, stored) = create_session(
        &app,
        &tokens,
        &account,
        Some(Platform::Cli),
        Some(label.unwrap_or_else(|| "ring CLI".into())),
    )?;
    crate::ting::spawn_register(
        app.clone(),
        stored.uuid.clone(),
        tokens.access_token.expose().to_string(),
    );
    Ok(Json(session))
}

// ---- POST /v1/auth/carbon ---------------------------------------------------------------------

fn allowed_native_redirect(uri: &str) -> bool {
    if uri == NATIVE_REDIRECT_URI {
        return true;
    }
    let Ok(parsed) = url::Url::parse(uri) else {
        return false;
    };
    parsed.scheme() == "http"
        && parsed.host_str() == Some("127.0.0.1")
        && parsed.path() == "/auth/callback"
        && parsed.query().is_none()
        && parsed.fragment().is_none()
        && parsed.username().is_empty()
}

pub async fn carbon_login(
    State(app): State<AppState>,
    peer: Peer,
    headers: HeaderMap,
    ApiJson(body): ApiJson<CarbonLogin>,
) -> ApiResult<Json<Session>> {
    rate_limit_auth(&app, &headers, peer)?;
    let code = body.code.trim().to_string();
    if code.is_empty() || code.len() > 512 {
        return Err(ApiError::invalid("code is required."));
    }
    let verifier = body.code_verifier.trim();
    if !(43..=128).contains(&verifier.len()) {
        return Err(ApiError::invalid(
            "code_verifier must be 43–128 characters.",
        ));
    }
    if !allowed_native_redirect(&body.redirect_uri) {
        return Err(
            ApiError::invalid("redirect_uri is not one of Ring's native redirect URIs.").hint(
                format!("Use {NATIVE_REDIRECT_URI} or http://127.0.0.1:<port>/auth/callback."),
            ),
        );
    }
    let label = clean(body.label);
    if let Some(label) = &label {
        check_len("label", label, limits::DEVICE_NAME)?;
    }
    claim_once(&app, "code", &code)?;
    let tokens = app
        .accounts
        .app()
        .exchange_code(&code, &body.redirect_uri, Some(verifier))
        .await
        .map_err(|e| exchange_error(&e, false))?;
    let account = tokens
        .account
        .clone()
        .ok_or_else(|| ApiError::dependency("Silicon Accounts"))?;
    if account.kind.as_str() != "carbon" {
        if let Some(refresh) = &tokens.refresh_token {
            accounts::spawn_revoke(app.clone(), refresh.expose().to_string());
        }
        return Err(silicon_use_cli());
    }
    let (session, _) = create_session(&app, &tokens, &account, None, label)?;
    Ok(Json(session))
}

fn silicon_use_cli() -> ApiError {
    ApiError::new(
        StatusCode::FORBIDDEN,
        ring_proto::codes::SILICON_USE_CLI,
        "Silicons use the ring CLI.",
    )
    .hint("silicon-accounts login --app ring -q | ring login --slt-stdin")
}

// ---- web sign-in ------------------------------------------------------------------------------

/// `302 Found` to `location`.
fn found(location: &str) -> Response {
    let mut response = StatusCode::FOUND.into_response();
    if let Ok(value) = HeaderValue::from_str(location) {
        response.headers_mut().insert(header::LOCATION, value);
    }
    response
}

#[derive(Serialize, Deserialize)]
struct LoginCookie {
    s: String,
    v: String,
    n: String,
    e: i64,
}

#[derive(Deserialize)]
pub struct WebStartQuery {
    next: Option<String>,
}

/// Only same-site relative paths are allowed as `next`.
fn safe_next(next: Option<&str>) -> String {
    match next {
        Some(n)
            if n.starts_with('/')
                && !n.starts_with("//")
                && !n.contains('\\')
                && !n.chars().any(char::is_control) =>
        {
            n.to_string()
        }
        _ => "/".to_string(),
    }
}

pub fn cookie_attrs(app: &App, path: &str, max_age: i64) -> String {
    let secure = if app.config.web_secure {
        "; Secure"
    } else {
        ""
    };
    format!("Path={path}; HttpOnly; SameSite=Lax; Max-Age={max_age}{secure}")
}

pub async fn web_start(
    State(app): State<AppState>,
    peer: Peer,
    headers: HeaderMap,
    ApiQuery(q): ApiQuery<WebStartQuery>,
) -> ApiResult<Response> {
    rate_limit_auth(&app, &headers, peer)?;
    let pkce = silicon_accounts_client::pkce_pair();
    let state = silicon_accounts_client::random_state();
    let payload = LoginCookie {
        s: state.clone(),
        v: pkce.verifier.clone(),
        n: safe_next(q.next.as_deref()),
        e: now_ms() + LOGIN_COOKIE_TTL_MS,
    };
    let sealed = app.sealer.seal(
        &serde_json::to_vec(&payload).unwrap_or_default(),
        b"ring:login",
    );
    let url = app
        .accounts
        .authorize_url(&app.config.web_callback_uri(), &state, &pkce);
    let cookie = format!(
        "{LOGIN_COOKIE}={}; {}",
        b64url(&sealed),
        cookie_attrs(&app, "/v1/auth/web", LOGIN_COOKIE_TTL_MS / 1000)
    );
    let mut response = found(&url);
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&cookie).map_err(|_| ApiError::internal())?,
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(response)
}

#[derive(Deserialize)]
pub struct WebCallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

pub async fn web_callback(
    State(app): State<AppState>,
    headers: HeaderMap,
    ApiQuery(q): ApiQuery<WebCallbackQuery>,
) -> Response {
    let clear_login = format!("{LOGIN_COOKIE}=; {}", cookie_attrs(&app, "/v1/auth/web", 0));
    let result = web_callback_inner(&app, &headers, q).await;
    let (location, session_cookie) = match result {
        Ok((session, next)) => {
            let max_age = ((session.expires_at.millis() - now_ms()) / 1000).max(0);
            let cookie = format!(
                "{SESSION_COOKIE}={}; {}",
                session.session_token,
                cookie_attrs(&app, "/", max_age)
            );
            (format!("{}{}", app.config.web_url, next), Some(cookie))
        }
        Err(code) => (format!("{}/?signin_error={code}", app.config.web_url), None),
    };
    let mut response = found(&location);
    let headers = response.headers_mut();
    if let Ok(v) = HeaderValue::from_str(&clear_login) {
        headers.append(header::SET_COOKIE, v);
    }
    if let Some(cookie) = session_cookie
        && let Ok(v) = HeaderValue::from_str(&cookie)
    {
        headers.append(header::SET_COOKIE, v);
    }
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

async fn web_callback_inner(
    app: &AppState,
    headers: &HeaderMap,
    q: WebCallbackQuery,
) -> Result<(Session, String), &'static str> {
    let sealed = cookie(headers, LOGIN_COOKIE)
        .and_then(b64url_decode)
        .ok_or("login_expired")?;
    let login: LoginCookie = app
        .sealer
        .open(&sealed, b"ring:login")
        .and_then(|b| serde_json::from_slice(&b).ok())
        .ok_or("login_expired")?;
    if login.e < now_ms() {
        return Err("login_expired");
    }
    if q.state.as_deref() != Some(login.s.as_str()) {
        return Err("state_mismatch");
    }
    if let Some(error) = q.error {
        return Err(if error == "access_denied" {
            "access_denied"
        } else {
            "signin_failed"
        });
    }
    let code = q
        .code
        .filter(|c| !c.is_empty() && c.len() <= 512)
        .ok_or("signin_failed")?;
    claim_once(app, "code", &code).map_err(|_| "signin_failed")?;
    let tokens = app
        .accounts
        .app()
        .exchange_code(&code, &app.config.web_callback_uri(), Some(&login.v))
        .await
        .map_err(|e| {
            tracing::info!(code = e.code(), "web sign-in exchange failed");
            if e.is_transport() {
                "dependency_unavailable"
            } else {
                "signin_failed"
            }
        })?;
    let account = tokens.account.clone().ok_or("signin_failed")?;
    if account.kind.as_str() != "carbon" {
        if let Some(refresh) = &tokens.refresh_token {
            accounts::spawn_revoke(app.clone(), refresh.expose().to_string());
        }
        return Err("silicon_use_cli");
    }
    let (session, _) = create_session(
        app,
        &tokens,
        &account,
        Some(Platform::Web),
        Some("Web browser".into()),
    )
    .map_err(|_| "signin_failed")?;
    Ok((session, login.n))
}

// ---- session endpoints ------------------------------------------------------------------------

pub async fn me(State(app): State<AppState>, auth: Auth) -> ApiResult<Json<Me>> {
    let device = app
        .db
        .with(|c| render_device(&app, c, &auth.session, auth.device_id()))?;
    Ok(Json(Me {
        account: auth.account,
        device,
    }))
}

pub async fn logout(State(app): State<AppState>, auth: Auth) -> ApiResult<Response> {
    revoke_session(&app, auth.device_id(), "signed_out_by_user", true).await;
    let mut response = StatusCode::NO_CONTENT.into_response();
    if auth.via_cookie {
        let cookie = format!("{SESSION_COOKIE}=; {}", cookie_attrs(&app, "/", 0));
        if let Ok(v) = HeaderValue::from_str(&cookie) {
            response.headers_mut().insert(header::SET_COOKIE, v);
        }
    }
    Ok(response)
}

/// Ends a session: no more requests, no more pushes, open sockets close. When `revoke_accounts`
/// is set, the device's Accounts sign-in is revoked too.
pub async fn revoke_session(app: &AppState, device_id: &str, reason: &str, revoke_accounts: bool) {
    let now = now_ms();
    let sealed = app.db.tx(|tx| -> rusqlite::Result<Option<Vec<u8>>> {
        let sealed: Option<Option<Vec<u8>>> = tx
            .query_row("SELECT refresh_sealed FROM sessions WHERE device_id = ?1 AND revoked_at IS NULL", [device_id], |r| r.get(0))
            .optional()?;
        tx.execute(
            "UPDATE sessions SET revoked_at = ?2, revoke_reason = ?3, refresh_sealed = NULL WHERE device_id = ?1 AND revoked_at IS NULL",
            params![device_id, now, reason],
        )?;
        tx.execute("DELETE FROM push_tokens WHERE device_id = ?1", [device_id])?;
        Ok(sealed.flatten())
    });
    let sealed = match sealed {
        Ok(s) => s,
        Err(error) => {
            tracing::error!(%error, "could not revoke session");
            return;
        }
    };
    tracing::info!(device_id, reason, "session revoked");
    app.hub.revoke_device(device_id);
    if revoke_accounts
        && let Some(sealed) = sealed
        && let Some(refresh) = accounts::open_refresh(app, device_id, &sealed)
    {
        accounts::spawn_revoke(app.clone(), refresh);
    }
}

/// Ends every session of an account (Accounts sign-out, access removed, account deleted).
pub async fn revoke_account(
    app: &AppState,
    uuid: &str,
    reason: &str,
    signed_in_before: Option<i64>,
) {
    let devices: Vec<String> = app
        .db
        .with(|c| {
            let mut stmt = c.prepare("SELECT device_id, signed_in_at FROM sessions WHERE account_uuid = ?1 AND revoked_at IS NULL")?;
            let rows = stmt.query_map([uuid], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
        })
        .unwrap_or_default()
        .into_iter()
        .filter(|(_, signed_in)| signed_in_before.is_none_or(|t| *signed_in <= t))
        .map(|(d, _)| d)
        .collect();
    for device in devices {
        revoke_session(app, &device, reason, false).await;
    }
}

/// `membership.signed_out` reports that one (or every) Accounts sign-in of an account ended, for
/// example when Ring itself revoked one device. Each Ring session is its own sign-in, so only the
/// sessions whose refresh token Accounts now reports inactive are ended.
pub async fn revoke_ended_sign_ins(app: &AppState, uuid: &str, signed_in_before: Option<i64>) {
    let devices: Vec<String> = app
        .db
        .with(|c| {
            let mut stmt = c.prepare("SELECT device_id, signed_in_at FROM sessions WHERE account_uuid = ?1 AND revoked_at IS NULL")?;
            let rows = stmt.query_map([uuid], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
        })
        .unwrap_or_default()
        .into_iter()
        .filter(|(_, signed_in)| signed_in_before.is_none_or(|t| *signed_in <= t))
        .map(|(d, _)| d)
        .collect();
    for device in devices {
        if matches!(
            accounts::session_liveness(app, &device, uuid).await,
            accounts::Liveness::Inactive
        ) {
            revoke_session(app, &device, "accounts_signed_out", false).await;
        }
    }
}

// ---- devices ----------------------------------------------------------------------------------

#[derive(Deserialize)]
struct WebPushSubscription {
    endpoint: String,
    keys: WebPushKeys,
}

#[derive(Deserialize)]
struct WebPushKeys {
    p256dh: String,
    auth: String,
}

fn validate_push(reg: &PushRegistration) -> ApiResult<()> {
    let token = reg.token.trim();
    if token.is_empty() || token.len() > 4096 {
        return Err(ApiError::invalid("push token is empty or too long."));
    }
    match reg.kind {
        PushKind::ApnsVoip | PushKind::Apns => {
            if token.len() < 32 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(ApiError::invalid("APNs tokens are hex strings."));
            }
        }
        PushKind::Fcm => {}
        PushKind::Webpush => {
            let sub: WebPushSubscription = serde_json::from_str(token).map_err(|_| {
                ApiError::invalid("A webpush token is the JSON of the browser's PushSubscription.")
            })?;
            let endpoint = url::Url::parse(&sub.endpoint)
                .map_err(|_| ApiError::invalid("The PushSubscription endpoint is not a URL."))?;
            if endpoint.scheme() != "https" {
                return Err(ApiError::invalid(
                    "The PushSubscription endpoint must be https.",
                ));
            }
            if b64url_decode(&sub.keys.p256dh).is_none_or(|k| k.len() != 65)
                || b64url_decode(&sub.keys.auth).is_none_or(|k| k.len() != 16)
            {
                return Err(ApiError::invalid("The PushSubscription keys are invalid."));
            }
        }
    }
    Ok(())
}

pub async fn update_current(
    State(app): State<AppState>,
    auth: Auth,
    ApiJson(update): ApiJson<DeviceUpdate>,
) -> ApiResult<Json<Device>> {
    let name = clean(update.name);
    if let Some(name) = &name {
        check_len("name", name, limits::DEVICE_NAME)?;
    }
    if let Some(version) = &update.app_version
        && version.len() > 64
    {
        return Err(ApiError::invalid("app_version is too long."));
    }
    match update.platform {
        Some(Platform::Cli) if !auth.is_silicon() => {
            return Err(ApiError::invalid("The cli platform is for silicons."));
        }
        Some(platform) if platform != Platform::Cli && auth.is_silicon() => {
            return Err(ApiError::invalid("Silicons use the cli platform."));
        }
        _ => {}
    }
    if auth.is_silicon() && update.push.as_ref().is_some_and(|p| !p.is_empty()) {
        return Err(ApiError::invalid(
            "Silicons are notified through Ting, not push.",
        ));
    }
    if let Some(push) = &update.push {
        if push.len() > 8 {
            return Err(ApiError::invalid("Too many push registrations."));
        }
        for reg in push {
            validate_push(reg)?;
        }
    }
    let device_id = auth.device_id().to_string();
    let now = now_ms();
    app.db.tx(|tx| -> rusqlite::Result<()> {
        tx.execute(
            "UPDATE sessions SET name = COALESCE(?2, name), platform = COALESCE(?3, platform), app_version = COALESCE(?4, app_version),
               ring_enabled = COALESCE(?5, ring_enabled), last_seen_at = ?6 WHERE device_id = ?1",
            params![device_id, name, update.platform.map(|p| p.as_str()), update.app_version, update.ring_enabled.map(|b| b as i64), now],
        )?;
        if let Some(push) = &update.push {
            tx.execute("DELETE FROM push_tokens WHERE device_id = ?1", [&device_id])?;
            for reg in push {
                let token = reg.token.trim();
                // A token belongs to one install; drop it from any older session that still holds it.
                tx.execute("DELETE FROM push_tokens WHERE token = ?1 AND device_id != ?2", params![token, device_id])?;
                let environment = match reg.kind {
                    PushKind::ApnsVoip | PushKind::Apns => Some(reg.environment.unwrap_or(PushEnvironment::Production)),
                    _ => None,
                };
                tx.execute(
                    "INSERT OR REPLACE INTO push_tokens (device_id, kind, token, environment, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        device_id,
                        serde_json::to_value(reg.kind).ok().and_then(|v| v.as_str().map(String::from)),
                        token,
                        environment.map(|e| if e == PushEnvironment::Sandbox { "sandbox" } else { "production" }),
                        now
                    ],
                )?;
            }
        }
        Ok(())
    })?;
    let device = device_changed(&app, auth.uuid(), &device_id, &device_id)?;
    Ok(Json(device))
}

/// Re-reads a device, tells the account's devices, and returns it as seen from `current`.
fn device_changed(app: &AppState, uuid: &str, device_id: &str, current: &str) -> ApiResult<Device> {
    let (device, for_others) = app.db.with(|c| {
        let row = by_device(c, device_id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)?;
        Ok((
            render_device(app, c, &row, current)?,
            render_device(app, c, &row, "")?,
        ))
    })?;
    app.hub
        .send_account(uuid, &Event::DeviceUpdated { device: for_others });
    Ok(device)
}

pub async fn list_devices(
    State(app): State<AppState>,
    auth: Auth,
) -> ApiResult<Json<Items<Device>>> {
    let items = app.db.with(|c| {
        live_for_account(c, auth.uuid())?
            .iter()
            .map(|row| render_device(&app, c, row, auth.device_id()))
            .collect::<rusqlite::Result<Vec<_>>>()
    })?;
    Ok(Json(Items { items }))
}

fn own_device(app: &App, auth: &Auth, device_id: &str) -> ApiResult<SessionRow> {
    let row = app.db.with(|c| by_device(c, device_id))?;
    match row {
        Some(row) if row.account_uuid == auth.account.uuid && row.revoked_at.is_none() => Ok(row),
        _ => Err(ApiError::not_found("Device")),
    }
}

pub async fn patch_device(
    State(app): State<AppState>,
    auth: Auth,
    Path(device_id): Path<String>,
    ApiJson(patch): ApiJson<DevicePatch>,
) -> ApiResult<Json<Device>> {
    own_device(&app, &auth, &device_id)?;
    let name = clean(patch.name);
    if let Some(name) = &name {
        check_len("name", name, limits::DEVICE_NAME)?;
    }
    app.db.with(|c| {
        c.execute(
            "UPDATE sessions SET name = COALESCE(?2, name), ring_enabled = COALESCE(?3, ring_enabled) WHERE device_id = ?1",
            params![device_id, name, patch.ring_enabled.map(|b| b as i64)],
        )
    })?;
    Ok(Json(device_changed(
        &app,
        auth.uuid(),
        &device_id,
        auth.device_id(),
    )?))
}

pub async fn delete_device(
    State(app): State<AppState>,
    auth: Auth,
    Path(device_id): Path<String>,
) -> ApiResult<StatusCode> {
    own_device(&app, &auth, &device_id)?;
    revoke_session(&app, &device_id, "revoked_by_user", true).await;
    Ok(StatusCode::NO_CONTENT)
}

/// Devices of an account that can be reached by push, with their tokens.
pub struct PushTarget {
    pub device_id: String,
    pub platform: Option<Platform>,
    pub ring_enabled: bool,
    pub kind: PushKind,
    pub token: String,
    pub environment: Option<String>,
}

pub fn push_targets(conn: &Connection, uuid: &str) -> rusqlite::Result<Vec<PushTarget>> {
    let mut stmt = conn.prepare(
        "SELECT s.device_id, s.platform, s.ring_enabled, p.kind, p.token, p.environment
         FROM sessions s JOIN push_tokens p ON p.device_id = s.device_id
         WHERE s.account_uuid = ?1 AND s.revoked_at IS NULL AND s.expires_at > ?2",
    )?;
    let rows = stmt.query_map(params![uuid, now_ms()], |r| {
        let platform: Option<String> = r.get(1)?;
        let kind: String = r.get(3)?;
        Ok((
            r.get::<_, String>(0)?,
            platform,
            r.get::<_, i64>(2)? != 0,
            kind,
            r.get::<_, String>(4)?,
            r.get::<_, Option<String>>(5)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (device_id, platform, ring_enabled, kind, token, environment) = row?;
        let Ok(kind) = serde_json::from_value::<PushKind>(serde_json::Value::String(kind)) else {
            continue;
        };
        out.push(PushTarget {
            device_id,
            platform: platform.as_deref().and_then(parse_platform),
            ring_enabled,
            kind,
            token,
            environment,
        });
    }
    Ok(out)
}
