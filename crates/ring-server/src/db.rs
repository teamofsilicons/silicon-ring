//! SQLite in WAL mode with versioned migrations. A handful of connections are shared round-robin;
//! WAL lets readers proceed while one writer commits, and every write is a short transaction.

use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, Transaction};

const POOL_SIZE: usize = 6;

pub struct Db {
    conns: Vec<Mutex<Connection>>,
    next: AtomicUsize,
}

impl Db {
    pub fn open(dir: &Path) -> anyhow_like::Result<Db> {
        create_private_dir(dir)?;
        let path = dir.join("ring.sqlite3");
        let mut conns = Vec::with_capacity(POOL_SIZE);
        for i in 0..POOL_SIZE {
            let conn = Connection::open_with_flags(
                &path,
                OpenFlags::SQLITE_OPEN_READ_WRITE
                    | OpenFlags::SQLITE_OPEN_CREATE
                    | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )?;
            conn.busy_timeout(Duration::from_secs(10))?;
            conn.pragma_update(None, "journal_mode", "WAL")?;
            conn.pragma_update(None, "synchronous", "NORMAL")?;
            conn.pragma_update(None, "foreign_keys", "ON")?;
            if i == 0 {
                migrate(&conn)?;
            }
            conns.push(Mutex::new(conn));
        }
        restrict_file(&path);
        Ok(Db {
            conns,
            next: AtomicUsize::new(0),
        })
    }

    fn conn(&self) -> std::sync::MutexGuard<'_, Connection> {
        for conn in &self.conns {
            if let Ok(guard) = conn.try_lock() {
                return guard;
            }
        }
        let i = self.next.fetch_add(1, Ordering::Relaxed) % self.conns.len();
        self.conns[i]
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Runs `f` with a connection (reads, single statements).
    pub fn with<T>(
        &self,
        f: impl FnOnce(&Connection) -> rusqlite::Result<T>,
    ) -> rusqlite::Result<T> {
        let conn = self.conn();
        f(&conn)
    }

    /// Runs `f` inside an IMMEDIATE transaction and commits when it returns `Ok`.
    pub fn tx<T, E: From<rusqlite::Error>>(
        &self,
        f: impl FnOnce(&Transaction<'_>) -> Result<T, E>,
    ) -> Result<T, E> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let value = f(&tx)?;
        tx.commit()?;
        Ok(value)
    }
}

/// Minimal error plumbing for startup (no external error crate).
pub mod anyhow_like {
    pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
}

fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }
    Ok(())
}

fn restrict_file(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// Each entry is one schema version; never edit an applied migration, append a new one.
const MIGRATIONS: &[&str] = &[
    // 1: initial schema
    r#"
    CREATE TABLE accounts (
        uuid TEXT PRIMARY KEY,
        id TEXT NOT NULL,
        kind TEXT NOT NULL,
        display_name TEXT,
        pfp_url TEXT,
        on_ring INTEGER NOT NULL DEFAULT 0,
        status TEXT NOT NULL DEFAULT 'active',
        profile_checked_at INTEGER NOT NULL DEFAULT 0,
        lookup_checked_at INTEGER NOT NULL DEFAULT 0,
        updated_at INTEGER NOT NULL
    );
    CREATE INDEX accounts_by_id ON accounts(id);

    CREATE TABLE sessions (
        device_id TEXT PRIMARY KEY,
        token_hash TEXT NOT NULL UNIQUE,
        account_uuid TEXT NOT NULL REFERENCES accounts(uuid),
        name TEXT,
        platform TEXT,
        app_version TEXT,
        ring_enabled INTEGER NOT NULL DEFAULT 1,
        created_at INTEGER NOT NULL,
        last_seen_at INTEGER,
        expires_at INTEGER NOT NULL,
        refresh_sealed BLOB,
        signed_in_at INTEGER NOT NULL,
        introspected_at INTEGER,
        revoked_at INTEGER,
        revoke_reason TEXT
    );
    CREATE INDEX sessions_by_account ON sessions(account_uuid);

    CREATE TABLE push_tokens (
        device_id TEXT NOT NULL REFERENCES sessions(device_id) ON DELETE CASCADE,
        kind TEXT NOT NULL,
        token TEXT NOT NULL,
        environment TEXT,
        created_at INTEGER NOT NULL,
        PRIMARY KEY (device_id, kind)
    );
    CREATE INDEX push_tokens_by_token ON push_tokens(token);

    CREATE TABLE auth_claims (
        hash TEXT PRIMARY KEY,
        kind TEXT NOT NULL,
        created_at INTEGER NOT NULL
    );

    CREATE TABLE calls (
        ringid TEXT PRIMARY KEY,
        state TEXT NOT NULL,
        created_by TEXT NOT NULL,
        created_at INTEGER NOT NULL,
        answered_at INTEGER,
        ended_at INTEGER,
        end_reason TEXT,
        voicemail_id TEXT,
        voicemail_reason TEXT,
        decline_reason TEXT,
        recording_status TEXT NOT NULL DEFAULT 'none',
        recording_duration_ms INTEGER,
        recording_key TEXT,
        next_seq INTEGER NOT NULL DEFAULT 1
    );
    CREATE INDEX calls_by_state ON calls(state);
    CREATE INDEX calls_by_created ON calls(created_at);

    CREATE TABLE participants (
        ringid TEXT NOT NULL REFERENCES calls(ringid),
        account_uuid TEXT NOT NULL,
        state TEXT NOT NULL,
        joined_at INTEGER NOT NULL,
        left_at INTEGER,
        invited_by TEXT,
        device_id TEXT,
        media_identity TEXT,
        rep_status TEXT,
        rep_voice TEXT,
        rep_context TEXT,
        rep_start TEXT,
        ting_seq INTEGER NOT NULL DEFAULT 0,
        PRIMARY KEY (ringid, account_uuid)
    );
    CREATE INDEX participants_by_account ON participants(account_uuid);

    CREATE TABLE invitations (
        invitation_id TEXT PRIMARY KEY,
        ringid TEXT NOT NULL REFERENCES calls(ringid),
        from_uuid TEXT NOT NULL,
        to_uuid TEXT NOT NULL,
        state TEXT NOT NULL,
        reason TEXT,
        created_at INTEGER NOT NULL,
        expires_at INTEGER NOT NULL,
        answered_at INTEGER,
        silenced INTEGER NOT NULL DEFAULT 0,
        call_uuid TEXT NOT NULL,
        acted_device_id TEXT
    );
    CREATE INDEX invitations_by_ring ON invitations(ringid);
    CREATE INDEX invitations_by_to ON invitations(to_uuid);

    CREATE TABLE silicon_reservations (
        account_uuid TEXT PRIMARY KEY,
        ringid TEXT NOT NULL,
        created_at INTEGER NOT NULL
    );

    CREATE TABLE transcript_entries (
        ringid TEXT NOT NULL REFERENCES calls(ringid),
        seq INTEGER NOT NULL,
        at INTEGER NOT NULL,
        offset_ms INTEGER,
        kind TEXT NOT NULL,
        actor_uuid TEXT,
        text TEXT,
        data TEXT NOT NULL DEFAULT '{}',
        owner_uuid TEXT,
        PRIMARY KEY (ringid, seq)
    );

    CREATE TABLE delegations (
        delegation_id TEXT PRIMARY KEY,
        ringid TEXT NOT NULL REFERENCES calls(ringid),
        silicon_uuid TEXT NOT NULL,
        created_at INTEGER NOT NULL,
        status TEXT NOT NULL,
        request TEXT,
        context TEXT NOT NULL DEFAULT '[]'
    );
    CREATE INDEX delegations_by_ring ON delegations(ringid, silicon_uuid);

    CREATE TABLE delegation_replies (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        delegation_id TEXT NOT NULL REFERENCES delegations(delegation_id),
        message_id TEXT NOT NULL,
        kind TEXT NOT NULL,
        text TEXT NOT NULL,
        at INTEGER NOT NULL
    );
    CREATE INDEX delegation_replies_by_delegation ON delegation_replies(delegation_id);

    CREATE TABLE messages (
        message_id TEXT PRIMARY KEY,
        ringid TEXT NOT NULL REFERENCES calls(ringid),
        silicon_uuid TEXT NOT NULL,
        kind TEXT NOT NULL,
        text TEXT NOT NULL,
        delegation_id TEXT,
        seq INTEGER NOT NULL,
        status TEXT NOT NULL,
        created_at INTEGER NOT NULL,
        delivered_at INTEGER
    );

    CREATE TABLE voicemails (
        voicemail_id TEXT PRIMARY KEY,
        ringid TEXT NOT NULL REFERENCES calls(ringid),
        from_uuid TEXT NOT NULL,
        to_uuid TEXT NOT NULL,
        reason TEXT NOT NULL,
        decline_reason TEXT,
        created_at INTEGER NOT NULL,
        duration_ms INTEGER,
        audio_status TEXT NOT NULL,
        audio_key TEXT,
        transcript TEXT,
        transcript_status TEXT NOT NULL,
        text TEXT,
        read INTEGER NOT NULL DEFAULT 0,
        delivered INTEGER NOT NULL DEFAULT 0,
        deleted_at INTEGER
    );
    CREATE INDEX voicemails_by_to ON voicemails(to_uuid, created_at);

    CREATE TABLE settings (
        account_uuid TEXT PRIMARY KEY,
        voice TEXT NOT NULL,
        default_context TEXT NOT NULL DEFAULT '',
        context_mode TEXT NOT NULL DEFAULT 'append',
        context_required INTEGER NOT NULL DEFAULT 0,
        voicemail_enabled INTEGER NOT NULL DEFAULT 1,
        greeting_busy TEXT,
        greeting_declined TEXT,
        greeting_missed TEXT,
        revision INTEGER NOT NULL DEFAULT 1,
        context_revision INTEGER NOT NULL DEFAULT 1,
        updated_at INTEGER NOT NULL
    );

    CREATE TABLE context_preparations (
        preparation_id TEXT PRIMARY KEY,
        account_uuid TEXT NOT NULL,
        action TEXT NOT NULL,
        target TEXT,
        default_context TEXT NOT NULL,
        default_revision INTEGER NOT NULL,
        supplied_context TEXT,
        mode TEXT NOT NULL,
        effective_context TEXT NOT NULL,
        uses_default INTEGER NOT NULL,
        created_at INTEGER NOT NULL,
        expires_at INTEGER NOT NULL
    );

    CREATE TABLE context_approvals (
        approval_id TEXT PRIMARY KEY,
        account_uuid TEXT NOT NULL,
        default_revision INTEGER NOT NULL,
        mode TEXT NOT NULL,
        created_at INTEGER NOT NULL,
        expires_at INTEGER NOT NULL,
        revoked_at INTEGER
    );
    CREATE INDEX context_approvals_by_account ON context_approvals(account_uuid);

    CREATE TABLE ting_subscriptions (
        account_uuid TEXT PRIMARY KEY,
        enabled INTEGER NOT NULL,
        subscription_id TEXT,
        last_error TEXT,
        updated_at INTEGER NOT NULL
    );

    CREATE TABLE ting_outbox (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        key TEXT NOT NULL UNIQUE,
        recipient_uuid TEXT NOT NULL,
        type TEXT NOT NULL,
        body BLOB NOT NULL,
        priority INTEGER NOT NULL,
        status TEXT NOT NULL,
        attempts INTEGER NOT NULL DEFAULT 0,
        next_attempt_at INTEGER NOT NULL,
        last_error TEXT,
        created_at INTEGER NOT NULL,
        updated_at INTEGER NOT NULL
    );
    CREATE INDEX ting_outbox_due ON ting_outbox(status, next_attempt_at);

    CREATE TABLE idempotency (
        scope TEXT PRIMARY KEY,
        request_hash TEXT NOT NULL,
        status INTEGER,
        body BLOB,
        created_at INTEGER NOT NULL
    );

    CREATE TABLE webhook_events (
        event_id TEXT PRIMARY KEY,
        source TEXT NOT NULL,
        received_at INTEGER NOT NULL
    );
    "#,
];

fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    let current: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    for (index, sql) in MIGRATIONS.iter().enumerate() {
        let version = index as i64 + 1;
        if version <= current {
            continue;
        }
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", version)?;
        tx.commit()?;
        tracing::info!(version, "applied database migration");
    }
    Ok(())
}
