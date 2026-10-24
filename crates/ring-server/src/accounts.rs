//! Silicon Accounts: the local profile cache keyed by uuid, callee resolution, liveness checks
//! (introspection without rotation), refresh under a per-session lock, and sign-in revocation.

use std::collections::HashMap;
use std::time::Duration;

use ring_proto::{Account, AccountKind};
use rusqlite::{Connection, OptionalExtension, params};
use silicon_accounts_client::{AccountForApp, AccountsClient, AppClient, Secret};

use crate::app::{App, KeyedLocks};
use crate::config::Config;
use crate::error::{ApiError, ApiResult, accounts_error};
use crate::util::now_ms;

const LOOKUP_FRESH_MS: i64 = 60_000;
const PROFILE_FRESH_MS: i64 = 10 * 60_000;

pub struct Accounts {
    client: AccountsClient,
    app_id: String,
    secret: String,
    pub session_locks: KeyedLocks,
}

impl Accounts {
    pub fn new(config: &Config) -> Result<Accounts, String> {
        let client = AccountsClient::builder()
            .base_url(config.accounts_url.clone())
            .user_agent(format!("ring-server/{}", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|e| format!("Silicon Accounts client: {e}"))?;
        Ok(Accounts {
            client,
            app_id: config.accounts_app_id.clone(),
            secret: config.accounts_app_secret.clone(),
            session_locks: KeyedLocks::default(),
        })
    }

    pub fn app(&self) -> AppClient<'_> {
        self.client.as_app(&self.app_id, &self.secret)
    }

    pub fn app_id(&self) -> &str {
        &self.app_id
    }

    pub fn authorize_url(
        &self,
        redirect_uri: &str,
        state: &str,
        pkce: &silicon_accounts_client::PkcePair,
    ) -> String {
        let params = silicon_accounts_client::AuthorizeParams::new(
            self.app_id.clone(),
            redirect_uri.to_string(),
        )
        .state(state.to_string())
        .pkce(pkce)
        .scopes(["profile"]);
        self.client.authorize_url(&params).to_string()
    }
}

pub fn kind_from_str(s: &str) -> AccountKind {
    if s == "silicon" {
        AccountKind::Silicon
    } else {
        AccountKind::Carbon
    }
}

fn non_empty(s: &str) -> Option<String> {
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_string())
}

// ---- local cache ------------------------------------------------------------------------------

pub struct AccountRow {
    pub account: Account,
    pub on_ring: bool,
    pub status: String,
    pub profile_checked_at: i64,
    pub lookup_checked_at: i64,
}

const ACCOUNT_COLUMNS: &str =
    "uuid, id, kind, display_name, pfp_url, on_ring, status, profile_checked_at, lookup_checked_at";

fn row_to_account(row: &rusqlite::Row<'_>) -> rusqlite::Result<AccountRow> {
    let kind: String = row.get(2)?;
    Ok(AccountRow {
        account: Account {
            uuid: row.get(0)?,
            id: row.get(1)?,
            kind: kind_from_str(&kind),
            display_name: row.get(3)?,
            pfp_url: row.get(4)?,
        },
        on_ring: row.get::<_, i64>(5)? != 0,
        status: row.get(6)?,
        profile_checked_at: row.get(7)?,
        lookup_checked_at: row.get(8)?,
    })
}

pub fn get_row(conn: &Connection, uuid: &str) -> rusqlite::Result<Option<AccountRow>> {
    conn.query_row(
        &format!("SELECT {ACCOUNT_COLUMNS} FROM accounts WHERE uuid = ?1"),
        [uuid],
        row_to_account,
    )
    .optional()
}

pub fn find_by_id(conn: &Connection, id: &str) -> rusqlite::Result<Option<AccountRow>> {
    conn.query_row(
        &format!("SELECT {ACCOUNT_COLUMNS} FROM accounts WHERE id = ?1 ORDER BY lookup_checked_at DESC LIMIT 1"),
        [id],
        row_to_account,
    )
    .optional()
}

pub fn get(conn: &Connection, uuid: &str) -> rusqlite::Result<Option<Account>> {
    Ok(get_row(conn, uuid)?.map(|r| r.account))
}

/// When the account last deleted its Ring account. Calls created before then are no longer its
/// own, even after it signs in to Ring again.
pub fn ring_deleted_at(conn: &Connection, uuid: &str) -> rusqlite::Result<Option<i64>> {
    conn.query_row(
        "SELECT ring_deleted_at FROM accounts WHERE uuid = ?1",
        [uuid],
        |r| r.get(0),
    )
    .optional()
    .map(Option::flatten)
}

/// Accounts for every uuid in `uuids`; unknown uuids get a placeholder so rendering never fails.
pub fn load_many<'a>(
    conn: &Connection,
    uuids: impl IntoIterator<Item = &'a str>,
) -> rusqlite::Result<HashMap<String, Account>> {
    let mut out = HashMap::new();
    for uuid in uuids {
        if out.contains_key(uuid) {
            continue;
        }
        let account = get(conn, uuid)?.unwrap_or_else(|| placeholder(uuid));
        out.insert(uuid.to_string(), account);
    }
    Ok(out)
}

pub fn placeholder(uuid: &str) -> Account {
    Account {
        uuid: uuid.to_string(),
        id: uuid.to_string(),
        kind: AccountKind::Carbon,
        display_name: None,
        pfp_url: None,
    }
}

/// Stores the profile from a token response (sign-in): the account is now on Ring.
pub fn upsert_from_login(conn: &Connection, account: &AccountForApp) -> rusqlite::Result<Account> {
    let now = now_ms();
    let display_name = non_empty(&account.display_name);
    let pfp_url = non_empty(&account.pfp_url);
    conn.execute(
        "INSERT INTO accounts (uuid, id, kind, display_name, pfp_url, on_ring, status, profile_checked_at, lookup_checked_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, 1, 'active', ?6, ?6, ?6)
         ON CONFLICT(uuid) DO UPDATE SET id = excluded.id, kind = excluded.kind, display_name = excluded.display_name,
           pfp_url = excluded.pfp_url, on_ring = 1, status = 'active', profile_checked_at = excluded.profile_checked_at,
           lookup_checked_at = excluded.lookup_checked_at, updated_at = excluded.updated_at",
        params![account.uuid, account.id.to_ascii_lowercase(), account.kind.as_str(), display_name, pfp_url, now],
    )?;
    Ok(Account {
        uuid: account.uuid.clone(),
        id: account.id.to_ascii_lowercase(),
        kind: kind_from_str(account.kind.as_str()),
        display_name,
        pfp_url,
    })
}

fn upsert_identity(
    conn: &Connection,
    uuid: &str,
    id: &str,
    kind: &str,
    status: &str,
) -> rusqlite::Result<()> {
    let now = now_ms();
    conn.execute(
        "INSERT INTO accounts (uuid, id, kind, status, lookup_checked_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?5)
         ON CONFLICT(uuid) DO UPDATE SET id = excluded.id, kind = excluded.kind, status = excluded.status,
           lookup_checked_at = excluded.lookup_checked_at, updated_at = excluded.updated_at",
        params![uuid, id.to_ascii_lowercase(), kind, status, now],
    )?;
    Ok(())
}

// ---- resolution -------------------------------------------------------------------------------

/// Resolves an actor reference (`c:x`, `si:x`, `@c:x`, uuid) to an active account.
pub async fn resolve(app: &App, reference: &str) -> ApiResult<Account> {
    let normalized = ring_proto::normalize_actor_ref(reference).ok_or_else(|| {
        ApiError::invalid(format!("{reference:?} is not an account reference."))
            .hint("Use c:handle, si:handle, @c:handle or an account uuid.")
    })?;
    let is_id = normalized.starts_with("c:") || normalized.starts_with("si:");
    let local = app.db.with(|c| {
        if is_id {
            find_by_id(c, &normalized)
        } else {
            get_row(c, &normalized)
        }
    })?;
    if let Some(row) = &local
        && row.status == "active"
        && now_ms() - row.lookup_checked_at < LOOKUP_FRESH_MS
    {
        return Ok(refresh_profile_if_stale(app, row).await);
    }
    let summary = match app.accounts.app().resolve(&normalized).await {
        Ok(summary) => summary,
        // Accounts refuses ids that break its handle rules (a half-typed `c:sh`) with
        // `invalid_id`; to the caller that is simply an account that does not exist.
        Err(e) if e.is_not_found() || e.code() == "invalid_id" => {
            return Err(ApiError::account_not_found(&normalized));
        }
        Err(e) => {
            // Serve a known active account through an Accounts outage rather than failing the call.
            if let Some(row) = local.filter(|r| r.status == "active") {
                tracing::warn!(
                    code = e.code(),
                    "account lookup failed; using cached identity"
                );
                return Ok(row.account);
            }
            return Err(accounts_error(&e));
        }
    };
    let kind = summary.kind.as_str();
    app.db
        .with(|c| upsert_identity(c, &summary.uuid, &summary.id, kind, &summary.status))?;
    if summary.status != "active" || summary.id.is_empty() {
        return Err(ApiError::account_not_found(&normalized));
    }
    let row = app
        .db
        .with(|c| get_row(c, &summary.uuid))?
        .ok_or_else(ApiError::internal)?;
    Ok(refresh_profile_if_stale(app, &row).await)
}

async fn refresh_profile_if_stale(app: &App, row: &AccountRow) -> Account {
    if now_ms() - row.profile_checked_at < PROFILE_FRESH_MS {
        return row.account.clone();
    }
    refresh_profile(app, &row.account.uuid)
        .await
        .unwrap_or_else(|| row.account.clone())
}

/// Re-reads name and photo from Ring's user base (`GET /v1/apps/ring/users/{uuid}`). Returns the
/// updated account, or `None` when Accounts could not be reached.
pub async fn refresh_profile(app: &App, uuid: &str) -> Option<Account> {
    let now = now_ms();
    let result = app.accounts.app().user(uuid).await;
    let updated = app.db.with(|c| {
        match &result {
            Ok(user) => {
                let member = user.status == "active";
                let display_name = non_empty(&user.display_name);
                let pfp_url = user.pfp_url.as_deref().and_then(non_empty);
                // Accounts still lists someone who deleted their Ring account as a member, so
                // they are on Ring again only once they sign in again (which recreates settings).
                c.execute(
                    "UPDATE accounts SET display_name = COALESCE(?2, display_name), pfp_url = COALESCE(?3, pfp_url),
                       id = COALESCE(?4, id),
                       on_ring = ?5 AND (ring_deleted_at IS NULL OR EXISTS (SELECT 1 FROM settings s WHERE s.account_uuid = ?1)),
                       profile_checked_at = ?6, updated_at = ?6 WHERE uuid = ?1",
                    params![uuid, display_name, pfp_url, user.id.as_deref().map(str::to_ascii_lowercase), member as i64, now],
                )?;
            }
            Err(e) if e.is_not_found() => {
                c.execute("UPDATE accounts SET on_ring = 0, profile_checked_at = ?2 WHERE uuid = ?1", params![uuid, now])?;
            }
            Err(_) => return Ok(None),
        }
        get(c, uuid)
    });
    updated.unwrap_or_else(|error| {
        tracing::error!(%error, "could not store profile");
        None
    })
}

// ---- liveness, refresh, revocation ------------------------------------------------------------

pub enum Liveness {
    Active,
    Inactive,
    Unknown,
}

/// Introspects a stored refresh token. Introspection never spends it.
pub async fn introspect(app: &App, refresh_token: &str, uuid: &str) -> Liveness {
    match app.accounts.app().introspect(refresh_token).await {
        Ok(i) if i.active => {
            let sub_ok = i.sub.as_deref().is_none_or(|s| s == uuid);
            let type_ok = i.token_type.as_deref().is_none_or(|t| t == "refresh_token");
            if sub_ok && type_ok {
                Liveness::Active
            } else {
                Liveness::Inactive
            }
        }
        Ok(_) => Liveness::Inactive,
        Err(e) => {
            tracing::warn!(code = e.code(), "introspection failed; keeping the session");
            Liveness::Unknown
        }
    }
}

/// Introspects one session's stored refresh token. Runs under the session's lock so it never reads
/// a token that a concurrent refresh is about to spend. A session without a readable token is
/// reported as `Unknown`.
pub async fn session_liveness(app: &App, device_id: &str, uuid: &str) -> Liveness {
    let _guard = app.accounts.session_locks.lock(device_id).await;
    let sealed: Option<Vec<u8>> = app
        .db
        .with(|c| {
            c.query_row(
                "SELECT refresh_sealed FROM sessions WHERE device_id = ?1 AND revoked_at IS NULL",
                [device_id],
                |r| r.get(0),
            )
            .optional()
        })
        .ok()
        .flatten()
        .flatten();
    let Some(refresh) = sealed.and_then(|s| open_refresh(app, device_id, &s)) else {
        return Liveness::Unknown;
    };
    let liveness = introspect(app, &refresh, uuid).await;
    if matches!(liveness, Liveness::Active) {
        let _ = app.db.with(|c| {
            c.execute(
                "UPDATE sessions SET introspected_at = ?2 WHERE device_id = ?1",
                params![device_id, now_ms()],
            )
        });
    }
    liveness
}

pub fn open_refresh(app: &App, device_id: &str, sealed: &[u8]) -> Option<String> {
    app.sealer
        .open_str(sealed, refresh_aad(device_id).as_bytes())
}

pub fn seal_refresh(app: &App, device_id: &str, refresh_token: &str) -> Vec<u8> {
    app.sealer
        .seal_str(refresh_token, refresh_aad(device_id).as_bytes())
}

fn refresh_aad(device_id: &str) -> String {
    format!("ring:refresh:{device_id}")
}

/// A fresh Accounts access token for a session. Refreshes under the session's lock and stores the
/// rotated refresh token before handing out the access token, so a crash never loses it.
pub async fn fresh_access_token(app: &crate::app::AppState, device_id: &str) -> ApiResult<Secret> {
    let _guard = app.accounts.session_locks.lock(device_id).await;
    let sealed: Option<Vec<u8>> = app
        .db
        .with(|c| {
            c.query_row(
                "SELECT refresh_sealed FROM sessions WHERE device_id = ?1 AND revoked_at IS NULL",
                [device_id],
                |r| r.get(0),
            )
            .optional()
        })?
        .flatten();
    let refresh = sealed
        .and_then(|s| open_refresh(app, device_id, &s))
        .ok_or_else(ApiError::session_expired)?;
    let tokens = match app.accounts.app().refresh(&refresh).await {
        Ok(tokens) => tokens,
        Err(e) if e.code() == "invalid_grant" => {
            crate::sessions::revoke_session(app, device_id, "signed_out", false).await;
            return Err(ApiError::session_expired());
        }
        Err(e) => {
            // The refresh may or may not have rotated the token; never present the old one again.
            tracing::warn!(code = e.code(), "refresh failed with an uncertain outcome");
            if e.is_transport() {
                app.db.with(|c| {
                    c.execute(
                        "UPDATE sessions SET refresh_sealed = NULL WHERE device_id = ?1",
                        [device_id],
                    )
                })?;
                return Err(ApiError::session_expired()
                    .hint("Ring could not confirm a token refresh. Sign in again to restore notifications."));
            }
            return Err(accounts_error(&e));
        }
    };
    let Some(new_refresh) = tokens.refresh_token.as_ref() else {
        return Err(ApiError::dependency("Silicon Accounts"));
    };
    let sealed = seal_refresh(app, device_id, new_refresh.expose());
    let expires = tokens
        .refresh_token_expires_at
        .map(|t| (t.unix_timestamp_nanos() / 1_000_000) as i64);
    app.db.with(|c| {
        c.execute(
            "UPDATE sessions SET refresh_sealed = ?2, expires_at = COALESCE(?3, expires_at) WHERE device_id = ?1",
            params![device_id, sealed, expires],
        )
    })?;
    Ok(tokens.access_token)
}

/// Ends an Accounts sign-in. Best effort with a few retries; revocation always answers 200.
pub fn spawn_revoke(app: std::sync::Arc<App>, token: String) {
    tokio::spawn(async move {
        for attempt in 0..4u32 {
            match app.accounts.app().revoke(&token).await {
                Ok(()) => return,
                Err(e) => {
                    tracing::warn!(
                        code = e.code(),
                        attempt,
                        "revoking an Accounts sign-in failed"
                    );
                    tokio::time::sleep(Duration::from_secs(2u64.pow(attempt + 1))).await;
                }
            }
        }
    });
}
