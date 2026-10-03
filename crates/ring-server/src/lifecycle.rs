//! Honeycomb's protected participant protocol. Receipts outlive the data they fence.
use crate::{auth::storage_error, engine::Engine, model::*, runtime, vault::Vault, App};
use axum::{
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    path::{Path as FilePath, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use uuid::Uuid;

#[derive(Clone, Serialize, Deserialize)]
pub struct TestingContext {
    pub environment_id: String,
    pub org_id: String,
    pub generation: u64,
    pub key_version: u64,
    pub testing_key: String,
}
#[derive(Clone, Serialize, Deserialize)]
struct Record {
    context: TestingContext,
    revision: u64,
    status: String,
    pending: Option<String>,
    snapshot: Value,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Operation {
    operation_id: String,
    environment_id: String,
    org_id: String,
    app_id: String,
    environment_revision: u64,
    generation: u64,
    key_version: u64,
    action: String,
    testing_key: String,
    #[serde(default)]
    snapshot: Value,
    #[serde(default)]
    reason: Value,
    #[serde(default)]
    retired_apps: Vec<String>,
}
impl Operation {
    fn validate(&self, path: &(String, String, String)) -> Result<()> {
        if self.org_id != path.0
            || self.environment_id != path.1
            || self.operation_id != path.2
            || self.app_id != "ring"
            || !canonical_uuid(&self.environment_id)
            || !canonical_uuid(&self.operation_id)
            || self.org_id.is_empty()
            || self.org_id.len() > 160
            || self.org_id.contains(['|', '/', '\\'])
            || self.environment_revision == 0
            || self.generation == 0
            || self.key_version == 0
            || self.testing_key.len() != 32
            || !self.testing_key.bytes().all(|b| b.is_ascii_alphanumeric())
            || !matches!(
                self.action.as_str(),
                "prepare"
                    | "import"
                    | "rotate-key"
                    | "clean"
                    | "disable"
                    | "restore"
                    | "purge"
                    | "retire-applications"
            )
            || self
                .retired_apps
                .iter()
                .any(|v| v.is_empty() || v.len() > 160)
        {
            return Err(invalid("Invalid Honeycomb participant operation"));
        }
        if self.action == "import"
            && (!self.snapshot.is_object()
                || self.snapshot["app_id"] != "ring"
                || self.snapshot["org_id"].as_str().is_none_or(str::is_empty)
                || [
                    "source_revision",
                    "source_iam_revision",
                    "configuration_revision",
                ]
                .iter()
                .any(|key| self.snapshot[key].as_u64().is_none_or(|v| v == 0))
                || self.snapshot["source_visibility"]
                    .as_str()
                    .is_none_or(str::is_empty)
                || self.snapshot["visibility"] != "private"
                || !self.snapshot["configuration"].is_object()
                || self
                    .snapshot
                    .get("selected_release")
                    .is_none_or(|v| !v.is_null() && !v.is_string()))
        {
            return Err(invalid(
                "Ring import requires its pinned private catalog snapshot",
            ));
        }
        Ok(())
    }
    fn receipt(&self, state: &str) -> Value {
        let mut value = json!({"state":state,"operation_id":self.operation_id,"environment_id":self.environment_id,
            "app_id":"ring","environment_revision":self.environment_revision,"generation":self.generation,"key_version":self.key_version});
        if self.action == "retire-applications" {
            value["retired_apps"] = json!(self.retired_apps);
        }
        value
    }
}
fn canonical_uuid(value: &str) -> bool {
    Uuid::parse_str(value).is_ok_and(|id| !id.is_nil() && id.to_string() == value)
}
pub fn unavailable() -> Fault {
    Fault::new(
        "TEST_ENVIRONMENT_UNAVAILABLE",
        "The selected test environment is unavailable.",
        "Use an active imported environment and its current application secret.",
    )
}
fn conflict() -> Fault {
    Fault::new(
        "LIFECYCLE_CONFLICT",
        "The lifecycle operation conflicts with a retained operation or environment fence.",
        "Retry the original operation or refresh the Honeycomb environment revision.",
    )
}
fn database_error(_: rusqlite::Error) -> Fault {
    Fault::new(
        "LIFECYCLE_STORAGE_FAILED",
        "Lifecycle state could not be persisted.",
        "Restore disk access and retry the same operation ID.",
    )
}
struct Inner {
    db: Connection,
    runtimes: BTreeMap<String, App>,
}
pub struct Manager {
    directory: PathBuf,
    vault: Vault,
    providers_disabled: bool,
    // ponytail: serialize lifecycle operations globally; partition by UUID if import traffic warrants it.
    inner: tokio::sync::Mutex<Inner>,
}
impl Manager {
    pub fn open(root: &FilePath) -> std::result::Result<Self, Box<dyn std::error::Error>> {
        let directory = root.join("testing");
        std::fs::create_dir_all(&directory)?;
        let vault = Vault::open(&directory)?;
        let db = Connection::open(directory.join("lifecycle.sqlite3"))?;
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
            CREATE TABLE IF NOT EXISTS environments(id TEXT PRIMARY KEY, record TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS operations(id TEXT PRIMARY KEY, environment_id TEXT NOT NULL, fingerprint TEXT NOT NULL, receipt TEXT);")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
            std::fs::set_permissions(
                directory.join("lifecycle.sqlite3"),
                std::fs::Permissions::from_mode(0o600),
            )?;
        }
        Ok(Self {
            directory,
            vault,
            providers_disabled: std::env::var("RING_DISABLE_PROVIDERS").as_deref() == Ok("1"),
            inner: tokio::sync::Mutex::new(Inner {
                db,
                runtimes: BTreeMap::new(),
            }),
        })
    }
    fn record(&self, db: &Connection, id: &str) -> Result<Option<Record>> {
        let sealed: Option<String> = db
            .query_row("SELECT record FROM environments WHERE id=?1", [id], |r| {
                r.get(0)
            })
            .optional()
            .map_err(database_error)?;
        sealed
            .map(|sealed| {
                let value = self.vault.unseal(id, &sealed).map_err(storage_error)?;
                serde_json::from_value(value)
                    .map_err(|_| invalid("Invalid retained lifecycle state"))
            })
            .transpose()
    }
    fn save(&self, db: &Connection, record: &Record) -> Result<()> {
        let sealed = self
            .vault
            .seal(&record.context.environment_id, &json!(record))
            .map_err(storage_error)?;
        db.execute("INSERT INTO environments(id,record) VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET record=excluded.record", params![record.context.environment_id, sealed]).map_err(database_error)?;
        Ok(())
    }
    fn data_directory(&self, record: &Record) -> PathBuf {
        self.directory
            .join(&record.context.environment_id)
            .join(record.context.generation.to_string())
    }
    fn open_runtime(&self, record: &Record) -> Result<App> {
        let dir = self.data_directory(record);
        let mut engine = Engine::open(&dir).map_err(|_| unavailable())?;
        runtime::recover_calls(&mut engine);
        engine.persist()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
                .map_err(storage_error)?;
            std::fs::set_permissions(
                dir.join("ring.sqlite3"),
                std::fs::Permissions::from_mode(0o600),
            )
            .map_err(storage_error)?;
        }
        Ok(App {
            testing: Some(Arc::new(record.context.clone())),
            environments: None,
            activity: Arc::default(),
            engine: Arc::new(Mutex::new(engine)),
            media: Arc::default(),
            test_tokens: Arc::default(),
            test_secret: None,
            providers_disabled: self.providers_disabled,
            vault: Arc::new(Vault::open(&dir).map_err(storage_error)?),
            telemetry: Arc::new(None),
            web_analytics: Arc::new(None),
            web_events: Arc::new(None),
            cli_telemetry: Arc::new(None),
        })
    }
    pub async fn get(&self, id: &str) -> Result<App> {
        if !canonical_uuid(id) {
            return Err(unavailable());
        }
        let mut inner = self.inner.lock().await;
        let record = self.record(&inner.db, id)?.ok_or_else(unavailable)?;
        if record.status != "active" || record.pending.is_some() {
            return Err(unavailable());
        }
        if let Some(app) = inner.runtimes.get(id) {
            return Ok(app.clone());
        }
        let app = self.open_runtime(&record)?;
        inner.runtimes.insert(id.into(), app.clone());
        Ok(app)
    }
    async fn apply(&self, operation: &Operation) -> Result<Value> {
        let mut inner = self.inner.lock().await;
        let fingerprint =
            digest(&serde_json::to_string(operation).map_err(|_| invalid("Invalid operation"))?);
        let previous: Option<(String, Option<String>)> = inner
            .db
            .query_row(
                "SELECT fingerprint,receipt FROM operations WHERE id=?1",
                [&operation.operation_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(database_error)?;
        if let Some((expected, receipt)) = &previous {
            if *expected != fingerprint {
                return Err(conflict());
            }
            if let Some(receipt) = receipt {
                return serde_json::from_str(receipt).map_err(|_| conflict());
            }
        }
        let existing = self.record(&inner.db, &operation.environment_id)?;
        let mut record = match existing {
            Some(record) => record,
            None => {
                if !matches!(operation.action.as_str(), "prepare" | "import") {
                    return Err(unavailable());
                }
                Record {
                    context: TestingContext {
                        environment_id: operation.environment_id.clone(),
                        org_id: operation.org_id.clone(),
                        generation: operation.generation,
                        key_version: operation.key_version,
                        testing_key: operation.testing_key.clone(),
                    },
                    revision: 0,
                    status: "new".into(),
                    pending: None,
                    snapshot: Value::Null,
                }
            }
        };
        if record.context.org_id != operation.org_id
            || record.status == "purged"
            || record
                .pending
                .as_ref()
                .is_some_and(|id| id != &operation.operation_id)
        {
            return Err(conflict());
        }
        if previous.is_none() {
            if operation.environment_revision <= record.revision
                || operation.generation < record.context.generation
                || operation.key_version < record.context.key_version
                || (operation.action == "clean"
                    && operation.generation <= record.context.generation)
                || (operation.action == "rotate-key"
                    && operation.key_version <= record.context.key_version)
                || (operation.key_version == record.context.key_version
                    && operation.testing_key != record.context.testing_key)
                || (record.status == "disabled"
                    && !matches!(operation.action.as_str(), "restore" | "purge"))
                || (record.status == "retired"
                    && !matches!(operation.action.as_str(), "import" | "purge"))
                || (operation.action == "restore" && record.status != "disabled")
            {
                return Err(conflict());
            }
            record.pending = Some(operation.operation_id.clone());
            let tx = inner.db.transaction().map_err(database_error)?;
            self.save(&tx, &record)?;
            tx.execute(
                "INSERT INTO operations(id,environment_id,fingerprint) VALUES(?1,?2,?3)",
                params![
                    operation.operation_id,
                    operation.environment_id,
                    fingerprint
                ],
            )
            .map_err(database_error)?;
            tx.commit().map_err(database_error)?;
        }
        // Dependency retirement only removes that dependency's authority. Ring's own
        // sessions, calls and provider workers keep running under unchanged realm fences.
        if operation.action == "retire-applications"
            && !operation.retired_apps.iter().any(|app| app == "ring")
            && operation.generation == record.context.generation
            && operation.key_version == record.context.key_version
        {
            if operation.retired_apps.iter().any(|app| app == "ting") {
                let app = if let Some(app) = inner.runtimes.get(&operation.environment_id) {
                    app.clone()
                } else {
                    let app = self.open_runtime(&record)?;
                    inner
                        .runtimes
                        .insert(operation.environment_id.clone(), app.clone());
                    app
                };
                crate::auth::retire_ting(&app).await?;
            }
            record.revision = operation.environment_revision;
            record.pending = None;
            return self.complete(&mut inner.db, &record, operation);
        }
        // Keep the stopped runtime cached until draining completes, including across a timeout.
        let app = if let Some(app) = inner.runtimes.get(&operation.environment_id) {
            app.clone()
        } else {
            let app = self.open_runtime(&record)?;
            inner
                .runtimes
                .insert(operation.environment_id.clone(), app.clone());
            app
        };
        app.stop().await?;
        let erase = matches!(operation.action.as_str(), "clean" | "purge")
            || operation.generation > record.context.generation;
        if erase {
            crate::storage::purge_environment(&app).await?;
        }
        // Revocation preserves the vault key needed to decrypt retained BYO provider settings.
        app.vault.clear_records().map_err(storage_error)?;
        {
            let mut e = app.engine.lock().unwrap();
            for publication in e
                .state
                .publications
                .values_mut()
                .filter(|p| matches!(p.status.as_str(), "pending" | "sending" | "failed"))
            {
                publication.status = "cancelled".into();
            }
            e.persist()?;
        }
        inner.runtimes.remove(&operation.environment_id);
        drop(app);
        if erase {
            let directory = self.data_directory(&record);
            match std::fs::remove_dir_all(&directory) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(storage_error(e)),
            }
        }
        record.context.generation = operation.generation;
        record.context.key_version = operation.key_version;
        record.context.testing_key = operation.testing_key.clone();
        record.revision = operation.environment_revision;
        record.pending = None;
        match operation.action.as_str() {
            "prepare" | "import" | "restore" => {
                record.status = "active".into();
            }
            "disable" => {
                record.status = "disabled".into();
            }
            "purge" => {
                record.status = "purged".into();
                record.context.testing_key.clear();
                record.snapshot = Value::Null;
            }
            "clean" => {
                record.status = "active".into();
                record.snapshot = Value::Null;
            }
            "retire-applications" => {
                if operation.retired_apps.iter().any(|v| v == "ring") {
                    record.status = "retired".into();
                }
            }
            _ => {}
        }
        if operation.action == "import" {
            record.snapshot = operation.snapshot.clone();
        }
        let next = if record.status == "active" {
            Some(self.open_runtime(&record)?)
        } else {
            None
        };
        let receipt = self.complete(&mut inner.db, &record, operation)?;
        if let Some(app) = next {
            inner.runtimes.insert(operation.environment_id.clone(), app);
        }
        Ok(receipt)
    }
    fn complete(
        &self,
        db: &mut Connection,
        record: &Record,
        operation: &Operation,
    ) -> Result<Value> {
        let receipt = operation.receipt("completed");
        let tx = db.transaction().map_err(database_error)?;
        self.save(&tx, record)?;
        tx.execute(
            "UPDATE operations SET receipt=?1 WHERE id=?2",
            params![receipt.to_string(), operation.operation_id],
        )
        .map_err(database_error)?;
        tx.commit().map_err(database_error)?;
        Ok(receipt)
    }
}
fn authorized(headers: &HeaderMap, token: Option<&str>) -> bool {
    let Some(token) =
        token.filter(|t| t.len() >= 32 && t.bytes().all(|b| (0x21..=0x7e).contains(&b)))
    else {
        return false;
    };
    let Some(supplied) = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
    else {
        return false;
    };
    let left = digest(token);
    let right = digest(supplied);
    left.bytes()
        .zip(right.bytes())
        .fold(0u8, |d, (a, b)| d | (a ^ b))
        == 0
}
pub async fn handle(
    State(app): State<App>,
    Path(path): Path<(String, String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let token = std::env::var("RING_HONEYCOMB_CONTROL_TOKEN").ok();
    if !authorized(&headers, token.as_deref()) {
        return (
            StatusCode::UNAUTHORIZED,
            "Protected lifecycle authorization required",
        )
            .into_response();
    }
    let operation: Operation = match serde_json::from_slice(&body) {
        Ok(operation) => operation,
        Err(_) => return (StatusCode::BAD_REQUEST, "Invalid lifecycle envelope").into_response(),
    };
    if let Err(error) = operation.validate(&path) {
        return (StatusCode::BAD_REQUEST, Json(json!({"error":error}))).into_response();
    }
    let Some(manager) = &app.environments else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    match tokio::time::timeout(Duration::from_secs(25), manager.apply(&operation)).await {
        Ok(Ok(receipt)) => Json(receipt).into_response(),
        Ok(Err(error)) => (StatusCode::CONFLICT, Json(json!({"error":error}))).into_response(),
        Err(_) => Json(operation.receipt("pending")).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn operation(
        environment: &str,
        action: &str,
        revision: u64,
        generation: u64,
        key_version: u64,
    ) -> Operation {
        Operation {
            operation_id: Uuid::new_v4().to_string(),
            environment_id: environment.into(),
            org_id: "test-owner".into(),
            app_id: "ring".into(),
            environment_revision: revision,
            generation,
            key_version,
            action: action.into(),
            testing_key: "a".repeat(32),
            snapshot: json!({"app_id":"ring","org_id":"catalog-owner","source_revision":2,"source_iam_revision":19,"configuration_revision":1,"source_visibility":"public","visibility":"private","selected_release":"0.1.2","configuration":{}}),
            reason: Value::Null,
            retired_apps: vec![],
        }
    }
    fn manager(dir: &FilePath) -> Manager {
        let mut manager = Manager::open(dir).unwrap();
        manager.providers_disabled = true;
        manager
    }
    fn marker(app: &App, value: &str) {
        let mut engine = app.engine.lock().unwrap();
        engine.state.bugs.insert("marker".into(), json!(value));
        engine.persist().unwrap();
    }
    #[tokio::test]
    async fn lifecycle_isolates_scopes_and_replays_receipts_after_restart() {
        let dir = tempfile::tempdir().unwrap();
        let a = Uuid::new_v4().to_string();
        let b = Uuid::new_v4().to_string();
        let manager = manager(dir.path());
        let import = operation(&a, "import", 1, 1, 1);
        let receipt = manager.apply(&import).await.unwrap();
        manager
            .apply(&operation(&b, "prepare", 3, 1, 1))
            .await
            .unwrap();
        let app_a = manager.get(&a).await.unwrap();
        let app_b = manager.get(&b).await.unwrap();
        marker(&app_a, "A");
        marker(&app_b, "B");
        app_a
            .vault
            .set("testing:app-secret", &json!("private"))
            .unwrap();
        let activity_a = app_a.activity.clone();
        drop(app_a);
        let clean = operation(&a, "clean", 4, 2, 1);
        manager.apply(&clean).await.unwrap();
        assert!(activity_a.cancelled.is_cancelled());
        assert!(activity_a.enter().is_none());
        let fresh = manager.get(&a).await.unwrap();
        assert!(fresh.engine.lock().unwrap().state.bugs.is_empty());
        assert!(fresh.vault.get("testing:app-secret").unwrap().is_none());
        assert_eq!(app_b.engine.lock().unwrap().state.bugs["marker"], "B");
        marker(&fresh, "new-generation");
        assert_eq!(manager.apply(&import).await.unwrap(), receipt);
        assert_eq!(manager.apply(&clean).await.unwrap()["state"], "completed");
        assert_eq!(
            fresh.engine.lock().unwrap().state.bugs["marker"],
            "new-generation"
        );
        fresh.stop().await.unwrap();
        app_b.stop().await.unwrap();
        drop(fresh);
        drop(app_b);
        drop(manager);
        let restarted = super::tests::manager(dir.path());
        assert_eq!(restarted.apply(&clean).await.unwrap()["state"], "completed");
        let restored = restarted.get(&a).await.unwrap();
        assert_eq!(
            restored.engine.lock().unwrap().state.bugs["marker"],
            "new-generation"
        );
        restored.stop().await.unwrap();
    }
    #[tokio::test]
    async fn fences_rotation_disable_restore_retirement_and_purge() {
        let dir = tempfile::tempdir().unwrap();
        let manager = manager(dir.path());
        let id = Uuid::new_v4().to_string();
        let prepare = operation(&id, "prepare", 1, 1, 1);
        manager.apply(&prepare).await.unwrap();
        let mut changed = prepare.clone();
        changed.reason = json!("changed");
        assert!(manager.apply(&changed).await.is_err());
        assert!(manager
            .apply(&operation(&id, "clean", 2, 1, 1))
            .await
            .is_err());
        assert!(manager
            .apply(&operation(&id, "restore", 2, 1, 1))
            .await
            .is_err());
        let old = manager.get(&id).await.unwrap();
        marker(&old, "retained");
        let old_activity = old.activity.clone();
        drop(old);
        let mut rotate = operation(&id, "rotate-key", 10, 1, 2);
        rotate.testing_key = "b".repeat(32);
        manager.apply(&rotate).await.unwrap();
        assert!(old_activity.cancelled.is_cancelled());
        let current = manager.get(&id).await.unwrap();
        assert_eq!(
            current.testing.as_ref().unwrap().testing_key,
            "b".repeat(32)
        );
        assert_eq!(
            current.engine.lock().unwrap().state.bugs["marker"],
            "retained"
        );
        drop(current);
        let next = |action, revision| {
            let mut o = operation(&id, action, revision, 1, 2);
            o.testing_key = "b".repeat(32);
            o
        };
        manager.apply(&next("disable", 11)).await.unwrap();
        assert!(manager.get(&id).await.is_err());
        assert!(manager.apply(&next("import", 12)).await.is_err());
        manager.apply(&next("restore", 14)).await.unwrap();
        let running = manager.get(&id).await.unwrap();
        let (session_token, active_ring) = {
            let mut e = running.engine.lock().unwrap();
            let mut login = |actor: &str| {
                let result = e
                    .login(
                        Identity {
                            actor: actor.into(),
                            org_id: "member-org".into(),
                            realm: id.clone(),
                            display_name: actor.into(),
                            admin: false,
                        },
                        None,
                    )
                    .unwrap();
                let token = result["session_token"].as_str().unwrap().to_string();
                (e.session(&token).unwrap(), token)
            };
            let (alice, token) = login("c:alice");
            let (bob, _) = login("c:bob");
            let ring = e
                .dispatch(&alice, "calls.init", &json!({"target":"c:bob"}))
                .unwrap()["ringid"]
                .as_str()
                .unwrap()
                .to_string();
            e.dispatch(&bob, "calls.accept", &json!({"ringid":ring}))
                .unwrap();
            e.persist().unwrap();
            (token, ring)
        };
        let owner = key(&id, "member-org", "c:alice");
        running
            .vault
            .set(
                &format!("iam:{owner}"),
                &json!({"access_token":"Ring-remains-authorized"}),
            )
            .unwrap();
        running
            .vault
            .set(&format!("ting:{owner}"), &json!({"grants":{}}))
            .unwrap();
        let mut retire = next("retire-applications", 15);
        retire.retired_apps = vec!["ting".into()];
        assert_eq!(
            manager.apply(&retire).await.unwrap()["retired_apps"],
            json!(["ting"])
        );
        assert!(!running.activity.cancelled.is_cancelled());
        assert!(running
            .engine
            .lock()
            .unwrap()
            .session(&session_token)
            .is_ok());
        assert_eq!(
            running.engine.lock().unwrap().state.calls[&active_ring].state,
            "active"
        );
        assert!(running
            .vault
            .get(&format!("iam:{owner}"))
            .unwrap()
            .is_some());
        assert!(running
            .vault
            .get(&format!("ting:{owner}"))
            .unwrap()
            .is_none());
        drop(running);
        retire.operation_id = Uuid::new_v4().to_string();
        retire.environment_revision = 16;
        retire.retired_apps = vec!["ring".into()];
        manager.apply(&retire).await.unwrap();
        assert!(manager.get(&id).await.is_err());
        manager.apply(&next("import", 18)).await.unwrap();
        let purge = next("purge", 20);
        manager.apply(&purge).await.unwrap();
        assert!(manager.get(&id).await.is_err());
        assert!(manager.apply(&next("import", 21)).await.is_err());
        assert_eq!(manager.apply(&purge).await.unwrap()["state"], "completed");
        let inner = manager.inner.lock().await;
        assert!(manager
            .record(&inner.db, &id)
            .unwrap()
            .unwrap()
            .context
            .testing_key
            .is_empty());
    }
    #[tokio::test]
    async fn incomplete_remote_cleanup_retains_fences_and_credentials_for_retry() {
        let dir = tempfile::tempdir().unwrap();
        let manager = manager(dir.path());
        let id = Uuid::new_v4().to_string();
        manager
            .apply(&operation(&id, "prepare", 1, 1, 1))
            .await
            .unwrap();
        let app = manager.get(&id).await.unwrap();
        app.vault
            .set(
                "testing:app-secret",
                &json!("retained-until-remote-cleanup"),
            )
            .unwrap();
        {
            let mut e = app.engine.lock().unwrap();
            e.state.storage.insert(
                "asset".into(),
                json!({"object_key":"unknown-original-bucket"}),
            );
            e.persist().unwrap();
        }
        let clean = operation(&id, "clean", 2, 2, 1);
        assert!(manager.apply(&clean).await.is_err());
        assert!(manager.get(&id).await.is_err());
        assert!(app.vault.get("testing:app-secret").unwrap().is_some());
        assert!(manager
            .apply(&operation(&id, "purge", 3, 2, 1))
            .await
            .is_err());
        {
            let mut e = app.engine.lock().unwrap();
            e.state.storage.clear();
            e.persist().unwrap();
        }
        drop(app);
        assert_eq!(manager.apply(&clean).await.unwrap()["state"], "completed");
        manager.get(&id).await.unwrap().stop().await.unwrap();
    }
    #[test]
    fn envelope_and_service_authority_cannot_be_replaced_by_test_keys() {
        let id = Uuid::new_v4().to_string();
        let op = operation(&id, "import", 1, 1, 1);
        let path = (op.org_id.clone(), id, op.operation_id.clone());
        assert!(op.validate(&path).is_ok());
        let mut invalid = op.clone();
        invalid.app_id = "ting".into();
        assert!(invalid.validate(&path).is_err());
        invalid = op.clone();
        invalid.environment_id = Uuid::nil().to_string();
        assert!(invalid.validate(&path).is_err());
        invalid = op;
        invalid.testing_key = "short".into();
        assert!(invalid.validate(&path).is_err());
        let mut headers = HeaderMap::new();
        headers.insert(
            "authorization",
            format!("Bearer {}", "a".repeat(32)).parse().unwrap(),
        );
        assert!(!authorized(&headers, None));
        assert!(!authorized(&headers, Some(&"b".repeat(32))));
        assert!(authorized(&headers, Some(&"a".repeat(32))));
    }
}
