use crate::{auth::storage_error, model::*, settings::StorageLocation, App};
use serde_json::{json, Value};
use std::time::Duration;

pub fn start(app: App) {
    app.clone().spawn_draining(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(5));
        loop {
            tokio::select! { biased; _ = app.activity.cancelled.cancelled() => break, _ = interval.tick() => {} }
            if !app.providers_disabled {
                sync(&app).await;
                evict_cache(&app).await;
            }
            retention(&app).await;
        }
    });
}
/// The runtime is stopped before this runs. Never infer a bucket or delete a prefix.
pub async fn purge_environment(app: &App) -> Result<()> {
    let records = {
        let e = app.engine.lock().unwrap();
        e.state
            .storage
            .iter()
            .map(|(id, record)| (e.state.assets.get(id).cloned(), record.clone()))
            .collect::<Vec<_>>()
    };
    for (asset, record) in records {
        // A failed upload may still have reached S3; every pinned location needs deletion.
        let Some(value) = record.get("location") else {
            if record.get("object_key").is_some() {
                return Err(invalid(
                    "Test cleanup requires the original pinned storage location",
                ));
            }
            continue;
        };
        let asset = asset.ok_or_else(|| invalid("Stored test asset is missing its owner"))?;
        let location: StorageLocation = serde_json::from_value(value.clone())
            .map_err(|_| invalid("Invalid pinned storage location"))?;
        let owner = asset.owner.split('|').collect::<Vec<_>>();
        if owner.len() != 3
            || app
                .testing
                .as_ref()
                .is_none_or(|t| t.environment_id != owner[0])
        {
            return Err(forbidden());
        }
        let (store, _) = crate::settings::s3(
            app,
            owner[0],
            owner[1],
            &location.object_key,
            Some(&location),
        )
        .await
        .map_err(crate::provider_error)?
        .ok_or_else(|| invalid("Original test asset storage is unavailable"))?;
        store
            .delete(&location.object_key)
            .await
            .map_err(crate::provider_error)?;
        if asset.ringid.is_some() {
            store
                .delete(&format!("{}.transcript.json", location.object_key))
                .await
                .map_err(crate::provider_error)?;
        }
    }
    Ok(())
}
async fn sync(app: &App) {
    let assets = {
        let e = app.engine.lock().unwrap();
        e.state
            .assets
            .values()
            .filter(|a| {
                a.complete
                    && a.voicemail_id.as_ref().is_none_or(|vid| {
                        e.state.voicemails.get(vid).is_some_and(|v| {
                            matches!(v.state.as_str(), "draft" | "delivered")
                                && (v.state != "draft" || v.expires_at > now())
                        })
                    })
                    && e.state.storage.get(&a.asset_id).is_none_or(|s| {
                        s["status"] != "ready"
                            && s["retry_at"].as_str().is_none_or(|t| t <= now().as_str())
                    })
            })
            .take(10)
            .cloned()
            .collect::<Vec<_>>()
    };
    for a in assets {
        if app.activity.cancelled.is_cancelled() {
            break;
        }
        if a.ringid
            .as_ref()
            .is_some_and(|ring| app.media.lock().unwrap().transcribers.contains_key(ring))
        {
            continue;
        }
        let owner = a.owner.split('|').collect::<Vec<_>>();
        if owner.len() != 3 {
            continue;
        }
        let realm = owner[0];
        let org = owner[1];
        if realm == "test" && !app.test_tokens.is_empty() {
            continue;
        }
        let object = if let Some(testing) = &app.testing {
            format!(
                "testing/{}/{}/{}/{}/{}",
                testing.environment_id,
                testing.generation,
                digest(org),
                a.purpose,
                a.asset_id
            )
        } else {
            format!("{realm}/{}/{}/{}", digest(org), a.purpose, a.asset_id)
        };
        let bucket = asset_store(app, realm, org, &a.asset_id, &object).await;
        let Some((store, location)) = (match bucket {
            Ok(v) => v,
            Err(error) => {
                failed(app, &a, &error.message);
                continue;
            }
        }) else {
            continue;
        };
        if app.activity.cancelled.is_cancelled() {
            break;
        }
        let pinned = pin_location(&mut app.engine.lock().unwrap(), &a.asset_id, &location);
        if let Err(error) = pinned {
            failed(app, &a, &error.message);
            continue;
        }
        let object = &location.object_key;
        let result = store.put_file(object, &a.mime_type, &a.path).await;
        match result {
            Ok(()) => {
                let transcript = if let Some(ring) = &a.ringid {
                    app.engine.lock().unwrap().state.calls.get(ring).map(|c|json!({"ringid":ring,"transcript":c.transcript,"participants":c.participants,"invitations":c.invitations}))
                } else {
                    None
                };
                if let Some(transcript) = transcript {
                    if let Err(error) = store
                        .put(
                            &format!("{object}.transcript.json"),
                            "application/json",
                            transcript.to_string().into_bytes(),
                        )
                        .await
                    {
                        failed(app, &a, &error.message);
                        continue;
                    }
                }
                let mut e = app.engine.lock().unwrap();
                let record = e
                    .state
                    .storage
                    .entry(a.asset_id.clone())
                    .or_insert(json!({}));
                record["status"] = json!("ready");
                record["stored_at"] = json!(now());
                record.as_object_mut().unwrap().remove("error");
                record.as_object_mut().unwrap().remove("retry_at");
                let _ = e.persist();
            }
            Err(error) => failed(app, &a, &error.message),
        }
    }
}
// Older records stored only a relative key. They can use the current settings,
// but readers must verify the object before pinning this inferred location.
async fn asset_store(
    app: &App,
    realm: &str,
    org: &str,
    asset_id: &str,
    default_object: &str,
) -> Result<Option<(ring_providers::storage::S3, StorageLocation)>> {
    let stored = app
        .engine
        .lock()
        .unwrap()
        .state
        .storage
        .get(asset_id)
        .cloned();
    let pinned = stored
        .as_ref()
        .and_then(|s| s.get("location"))
        .map(|v| serde_json::from_value::<StorageLocation>(v.clone()))
        .transpose()
        .map_err(|_| invalid("Invalid stored recording location"))?;
    let object = stored
        .as_ref()
        .and_then(|s| s["object_key"].as_str())
        .unwrap_or(default_object);
    let Some((store, location)) = crate::settings::s3(app, realm, org, object, pinned.as_ref())
        .await
        .map_err(crate::provider_error)?
    else {
        return Ok(None);
    };
    Ok(Some((store, location)))
}
fn pin_location(
    e: &mut crate::engine::Engine,
    asset_id: &str,
    location: &StorageLocation,
) -> Result<()> {
    let previous = e.state.storage.get(asset_id).cloned();
    let location = json!(location);
    if let Some(pinned) = previous.as_ref().and_then(|s| s.get("location")) {
        if pinned != &location {
            return Err(invalid(
                "Storage location changed concurrently; retry the operation",
            ));
        }
        return Ok(());
    }
    let record = e.state.storage.entry(asset_id.into()).or_insert(json!({}));
    record["object_key"] = location["object_key"].clone();
    record["location"] = location;
    // Pin before the first PUT, including a partial audio/transcript upload.
    if let Err(error) = e.persist() {
        match previous {
            Some(previous) => {
                e.state.storage.insert(asset_id.into(), previous);
            }
            None => {
                e.state.storage.remove(asset_id);
            }
        }
        return Err(error);
    }
    Ok(())
}
fn failed(app: &App, a: &Asset, message: &str) {
    let mut e = app.engine.lock().unwrap();
    let attempts = e
        .state
        .storage
        .get(&a.asset_id)
        .and_then(|s| s["attempts"].as_u64())
        .unwrap_or(0)
        + 1;
    let wait = 2i64.pow(attempts.min(10) as u32);
    let record = e
        .state
        .storage
        .entry(a.asset_id.clone())
        .or_insert(json!({}));
    record["status"] = json!("failed");
    record["error"] = json!(message);
    record["attempts"] = json!(attempts);
    record["retry_at"] = json!(after(wait));
    if let Some(ring) = &a.ringid {
        if let Some(c) = e.state.calls.get(ring).cloned() {
            e.state.call_event(
                &c,
                "recording.storage_failed",
                json!({"ringid":ring,"asset_id":a.asset_id,"message":message}),
            );
        }
    }
    let _ = e.persist();
}
pub async fn ensure_local(app: &App, i: &Identity, asset_id: &str) -> Result<()> {
    let (a, stored) = {
        let e = app.engine.lock().unwrap();
        (
            e.authorized_asset(i, asset_id)?.clone(),
            e.state.storage.get(asset_id).cloned(),
        )
    };
    if std::path::Path::new(&a.path).exists() {
        return Ok(());
    }
    let stored = stored.ok_or_else(|| missing("Stored audio"))?;
    let mut owner = a.owner.splitn(3, '|');
    let realm = owner
        .next()
        .ok_or_else(|| invalid("Asset realm is missing"))?;
    let org = owner
        .next()
        .ok_or_else(|| invalid("Asset storage context is missing"))?;
    let (store, location) =
        asset_store(app, realm, org, asset_id, required(&stored, "object_key")?)
            .await?
            .ok_or_else(|| {
                Fault::new(
                    "STORAGE_NOT_CONFIGURED",
                    "S3 storage is not configured.",
                    "Configure the recording's organization storage.",
                )
            })?;
    let temporary = format!("{}.{}", a.path, id("download"));
    let result = store
        .get_to_file(&location.object_key, &temporary)
        .await
        .map_err(crate::provider_error);
    match result {
        Ok(length) if length == a.size_bytes as u64 => {
            let pinned = pin_location(&mut app.engine.lock().unwrap(), asset_id, &location);
            if let Err(error) = pinned {
                let _ = tokio::fs::remove_file(&temporary).await;
                return Err(error);
            }
            tokio::fs::rename(&temporary, &a.path)
                .await
                .map_err(storage_error)?;
        }
        Ok(_) => {
            let _ = tokio::fs::remove_file(&temporary).await;
            return Err(invalid(
                "Stored audio length does not match its committed metadata",
            ));
        }
        Err(error) => {
            let _ = tokio::fs::remove_file(&temporary).await;
            return Err(error);
        }
    }
    Ok(())
}
async fn retention(app: &App) {
    let expired = {
        let e = app.engine.lock().unwrap();
        let mut ids = Vec::new();
        for c in e
            .state
            .calls
            .values()
            .filter(|c| c.state == "ended" && c.recording_status != "purged")
        {
            let days = e
                .state
                .configs
                .get(&key(&c.realm, &c.org_id, "*"))
                .and_then(|c| c.values.get("retention.calls_days"))
                .and_then(Value::as_i64)
                .unwrap_or(30);
            if c.ended_at
                .as_ref()
                .is_some_and(|time| time < &after(-days * 86400))
            {
                ids.push((
                    c.realm.clone(),
                    c.org_id.clone(),
                    c.ringid.clone(),
                    c.recording_asset_id.clone(),
                    false,
                ));
            }
        }
        for v in e.state.voicemails.values().filter(|v| {
            matches!(v.state.as_str(), "delivered" | "deleted" | "aborted")
                || v.state == "draft" && v.expires_at <= now()
        }) {
            let days = e
                .state
                .configs
                .get(&key(&v.realm, &v.org_id, "*"))
                .and_then(|c| c.values.get("retention.voicemail_days"))
                .and_then(Value::as_i64)
                .unwrap_or(30);
            if v.state != "delivered" || v.created_at < after(-days * 86400) {
                ids.push((
                    v.realm.clone(),
                    v.org_id.clone(),
                    v.voicemail_id.clone(),
                    v.audio_asset_id.clone(),
                    true,
                ));
            }
        }
        ids
    };
    for (realm, org, id, aid, vm) in expired {
        if app.activity.cancelled.is_cancelled() {
            break;
        }
        if let Some(aid) = &aid {
            let stored = app.engine.lock().unwrap().state.storage.get(aid).cloned();
            if let Some(stored) = stored {
                if app.providers_disabled {
                    continue;
                }
                if let Some(key) = stored["object_key"].as_str() {
                    let (store, location) = match asset_store(app, &realm, &org, aid, key).await {
                        Ok(Some(s)) => s,
                        _ => continue,
                    };
                    let key = &location.object_key;
                    if stored.get("location").is_none() {
                        // DELETE also succeeds when the guessed key does not exist.
                        // Verify legacy data before adopting a location or purging it.
                        let size = app
                            .engine
                            .lock()
                            .unwrap()
                            .state
                            .assets
                            .get(aid)
                            .map(|a| a.size_bytes as u64);
                        if store.size(key).await.ok() != size || size.is_none() {
                            continue;
                        }
                        if pin_location(&mut app.engine.lock().unwrap(), aid, &location).is_err() {
                            continue;
                        }
                    }
                    if store.delete(key).await.is_err() {
                        continue;
                    }
                    if !vm
                        && store
                            .delete(&format!("{key}.transcript.json"))
                            .await
                            .is_err()
                    {
                        continue;
                    }
                }
            }
            let e = app.engine.lock().unwrap();
            app.media.lock().unwrap().prune_voicemail_streams(&e);
            if let Some(a) = e.state.assets.get(aid) {
                if let Err(error) = std::fs::remove_file(&a.path) {
                    if error.kind() != std::io::ErrorKind::NotFound {
                        continue;
                    }
                }
            }
        }
        let mut e = app.engine.lock().unwrap();
        e.state.event_seq = e.state.latest_event_seq();
        if vm {
            e.state.events.retain(|v| v.data["voicemail_id"] != id);
            e.state
                .publications
                .retain(|_, v| v.data["voicemail_id"] != id);
            e.state.greetings.remove(&id);
        } else {
            let message_ids = e
                .state
                .calls
                .get(&id)
                .map(|c| {
                    c.transcript
                        .iter()
                        .filter_map(|t| t.data["message_id"].as_str().map(String::from))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            for mid in message_ids {
                e.state.sent_messages.remove(&mid);
            }
            e.state.events.retain(|v| v.data["ringid"] != id);
            e.state.publications.retain(|_, v| v.data["ringid"] != id);
            e.state.delegations.retain(|_, v| v.ringid != id);
            e.state.recording_gaps.remove(&id);
            e.state
                .transcript_cursors
                .retain(|k, _| !k.starts_with(&format!("{id}|")));
        }
        if let Some(aid) = aid {
            e.state.assets.remove(&aid);
            e.state.storage.remove(&aid);
        }
        e.state.requests.retain(|_, cached| {
            cached.response["result"]["ringid"] != id
                && cached.response["result"]["voicemail_id"] != id
        });
        if vm {
            if let Some(v) = e.state.voicemails.get_mut(&id) {
                v.state = "purged".into();
                v.audio_asset_id = None;
                v.transcript = None;
                v.text = None;
            }
        } else if let Some(c) = e.state.calls.get_mut(&id) {
            c.transcript.clear();
            c.recording_status = "purged".into();
            c.recording_asset_id = None;
            for p in &mut c.participants {
                p.context.clear();
                p.start.clear();
            }
        }
        let _ = e.persist();
    }
}

// S3 is the durable store. Keep only a bounded, recently used cache of completed audio.
async fn evict_cache(app: &App) {
    let assets = {
        let e = app.engine.lock().unwrap();
        e.state
            .assets
            .values()
            .filter(|a| {
                a.complete
                    && e.state
                        .storage
                        .get(&a.asset_id)
                        .is_some_and(|s| s["status"] == "ready")
            })
            .cloned()
            .collect::<Vec<_>>()
    };
    let limit = std::env::var("RING_CACHE_BYTES")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(512 * 1024 * 1024);
    let mut rows = Vec::new();
    let mut total = 0u64;
    for a in assets {
        if let Ok(metadata) = tokio::fs::metadata(&a.path).await {
            total = total.saturating_add(metadata.len());
            rows.push((
                metadata
                    .modified()
                    .unwrap_or(std::time::SystemTime::UNIX_EPOCH),
                metadata.len(),
                a.path,
            ));
        }
    }
    rows.sort_by_key(|r| r.0);
    for (modified, size, path) in rows {
        let age = modified.elapsed().unwrap_or_default().as_secs();
        if age < 60 {
            continue;
        }
        if total <= limit && age < 3600 {
            continue;
        }
        if tokio::fs::remove_file(path).await.is_ok() {
            total = total.saturating_sub(size);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{engine::Engine, media::Media, settings::StorageCredentials, vault::Vault};
    use std::{
        collections::BTreeMap,
        sync::{Arc, Mutex},
    };

    fn app(dir: &std::path::Path) -> App {
        App {
            testing: None,
            environments: None,
            activity: Arc::default(),
            engine: Arc::new(Mutex::new(Engine::open(dir).unwrap())),
            media: Arc::new(Mutex::new(Media::default())),
            test_tokens: Arc::new(BTreeMap::new()),
            test_secret: None,
            providers_disabled: true,
            vault: Arc::new(Vault::open(dir).unwrap()),
            telemetry: Arc::new(None),
            web_analytics: Arc::new(None),
            web_events: Arc::new(None),
            cli_telemetry: Arc::new(None),
        }
    }
    fn login(e: &mut Engine, actor: &str) -> (Session, String) {
        let response = e
            .login(
                Identity {
                    actor: actor.into(),
                    org_id: "org".into(),
                    realm: "test".into(),
                    display_name: actor.into(),
                    admin: true,
                },
                None,
            )
            .unwrap();
        let token = response["session_token"].as_str().unwrap().to_owned();
        (e.session(&token).unwrap(), token)
    }

    #[test]
    fn partial_upload_location_survives_failure_restart_and_configuration_changes() {
        let dir = tempfile::tempdir().unwrap();
        let app = app(dir.path());
        let a = Asset {
            asset_id: "asset".into(),
            owner: "test|org|c:alice".into(),
            purpose: "voicemail".into(),
            mime_type: "audio/wav".into(),
            size_bytes: 44,
            received_bytes: 44,
            next_seq: 0,
            complete: true,
            path: "unused".into(),
            ringid: None,
            voicemail_id: None,
        };
        let location = StorageLocation {
            bucket: "original".into(),
            region: "us-east-1".into(),
            object_key: "old/full/key".into(),
            credentials: StorageCredentials::Organization,
        };
        pin_location(&mut app.engine.lock().unwrap(), &a.asset_id, &location).unwrap();
        failed(
            &app,
            &a,
            "Transcript PUT timed out after audio PUT succeeded",
        );
        let mut reopened = Engine::open(dir.path()).unwrap();
        assert_eq!(reopened.state.storage["asset"]["status"], "failed");
        assert_eq!(reopened.state.storage["asset"]["location"], json!(location));
        assert_eq!(
            reopened.state.storage["asset"]["object_key"],
            "old/full/key"
        );
        let replacement = StorageLocation {
            bucket: "replacement".into(),
            object_key: "new/full/key".into(),
            ..location.clone()
        };
        assert!(pin_location(&mut reopened, "asset", &replacement).is_err());
        assert_eq!(reopened.state.storage["asset"]["location"], json!(location));
    }

    #[tokio::test]
    async fn s3_requests_keep_locations_rotate_credentials_and_verify_legacy_objects() {
        const CHILD: &str = "RING_S3_REQUEST_TEST_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "storage::tests::s3_requests_keep_locations_rotate_credentials_and_verify_legacy_objects", "--nocapture"])
                .env(CHILD, "1")
                .env("RING_ENV", "development")
                .env_remove("RING_ENCRYPTION_KEY")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        use axum::{
            body::{to_bytes, Body},
            extract::{Request, State},
            http::Response,
            Router,
        };
        #[derive(Default)]
        struct MockS3 {
            calls: Vec<(String, String, String)>,
            objects: BTreeMap<String, Vec<u8>>,
        }
        async fn request(
            State(mock): State<Arc<Mutex<MockS3>>>,
            request: Request,
        ) -> Response<Body> {
            let method = request.method().to_string();
            let path = request.uri().path().to_owned();
            let authorization = request.headers()["authorization"]
                .to_str()
                .unwrap()
                .to_owned();
            let body = to_bytes(request.into_body(), 4096).await.unwrap();
            let mut mock = mock.lock().unwrap();
            mock.calls
                .push((method.clone(), path.clone(), authorization));
            match method.as_str() {
                "PUT" => {
                    assert!(!body.is_empty());
                    mock.objects
                        .insert(path, crate::media::wav_header(0).to_vec());
                    Response::builder()
                        .status(200)
                        .header("etag", "\"mock\"")
                        .body(Body::empty())
                        .unwrap()
                }
                "GET" | "HEAD" => match mock.objects.get(&path) {
                    Some(bytes) => Response::builder()
                        .status(200)
                        .header("content-length", bytes.len())
                        .body(if method == "HEAD" {
                            Body::empty()
                        } else {
                            Body::from(bytes.clone())
                        })
                        .unwrap(),
                    None => Response::builder()
                        .status(404)
                        .header("content-type", "application/xml")
                        .body(Body::from("<Error><Code>NoSuchKey</Code></Error>"))
                        .unwrap(),
                },
                "DELETE" => {
                    mock.objects.remove(&path);
                    Response::builder().status(204).body(Body::empty()).unwrap()
                }
                _ => panic!("unexpected S3 method {method}"),
            }
        }
        fn configure(app: &App, alice: &Session, bucket: &str, prefix: &str, access: &str) {
            let mut params = json!({"scope":"org","values":{"providers.storage":{
                "bucket":bucket,"region":"us-east-1","prefix":prefix,
                "access_key_id":access,"secret_access_key":"mock-secret"}}});
            crate::settings::prepare_config(app, &alice.identity, &mut params).unwrap();
            app.engine
                .lock()
                .unwrap()
                .dispatch(alice, "config.set", &params)
                .unwrap();
        }
        let mock = Arc::new(Mutex::new(MockS3::default()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let router = Router::new().fallback(request).with_state(mock.clone());
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let dir = tempfile::tempdir().unwrap();
        // Isolated process, explicit fake credentials, and only a loopback endpoint.
        std::env::set_var("AWS_ENDPOINT_URL_S3", &endpoint);
        std::env::set_var("AWS_ENDPOINT_URL", &endpoint);
        std::env::set_var("AWS_IGNORE_CONFIGURED_ENDPOINT_URLS", "false");
        std::env::set_var("AWS_EC2_METADATA_DISABLED", "true");
        std::env::set_var("AWS_CONFIG_FILE", dir.path().join("no-aws-config"));
        std::env::set_var(
            "AWS_SHARED_CREDENTIALS_FILE",
            dir.path().join("no-aws-credentials"),
        );
        let mut app = app(dir.path());
        app.providers_disabled = false;
        let alice = login(&mut app.engine.lock().unwrap(), "c:alice").0;
        configure(&app, &alice, "original", "old/", "INITIALACCESS");
        let path = dir.path().join("assets").join("audio");
        std::fs::write(&path, crate::media::wav_header(0)).unwrap();
        {
            let mut e = app.engine.lock().unwrap();
            e.state.assets.insert(
                "audio".into(),
                Asset {
                    asset_id: "audio".into(),
                    owner: "test|org|c:alice".into(),
                    purpose: "voicemail".into(),
                    mime_type: "audio/wav".into(),
                    size_bytes: 44,
                    received_bytes: 44,
                    next_seq: 0,
                    complete: true,
                    path: path.to_string_lossy().into(),
                    ringid: None,
                    voicemail_id: Some("vm".into()),
                },
            );
            e.state.voicemails.insert("vm".into(), serde_json::from_value(json!({
                "voicemail_id":"vm","ringid":"ring","invitation_id":"invite","org_id":"org",
                "realm":"test","sender":"c:alice","recipient":"c:bob","format":"audio",
                "state":"delivered","created_at":now(),"expires_at":after(600),"reason":null,
                "text":null,"transcript":null,"audio_asset_id":"audio","read":false,
                "complete_audio":true,"transcription_status":"pending","synthesis_status":"pending","error":null
            })).unwrap());
        }
        sync(&app).await;
        assert_eq!(
            app.engine.lock().unwrap().state.storage["audio"]["status"],
            "ready"
        );
        configure(&app, &alice, "replacement", "new/", "ROTATEDACCESS");
        app.engine
            .lock()
            .unwrap()
            .state
            .storage
            .get_mut("audio")
            .unwrap()["status"] = json!("failed");
        sync(&app).await;
        assert_eq!(
            app.engine.lock().unwrap().state.storage["audio"]["status"],
            "ready"
        );
        std::fs::remove_file(&path).unwrap();
        ensure_local(&app, &alice.identity, "audio").await.unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), crate::media::wav_header(0));

        // A failed guess for a legacy key must remain recoverable by restoring settings.
        let relative = format!("test/{}/voicemail/audio", digest("org"));
        let legacy = json!({"status":"ready","object_key":relative});
        app.engine
            .lock()
            .unwrap()
            .state
            .storage
            .insert("audio".into(), legacy.clone());
        std::fs::remove_file(&path).unwrap();
        assert!(ensure_local(&app, &alice.identity, "audio").await.is_err());
        assert_eq!(app.engine.lock().unwrap().state.storage["audio"], legacy);
        app.engine
            .lock()
            .unwrap()
            .state
            .voicemails
            .get_mut("vm")
            .unwrap()
            .state = "deleted".into();
        retention(&app).await;
        {
            let e = app.engine.lock().unwrap();
            assert_eq!(e.state.storage["audio"], legacy);
            assert!(e.state.assets.contains_key("audio"));
            assert_eq!(e.state.voicemails["vm"].state, "deleted");
        }
        configure(&app, &alice, "original", "old/", "ROTATEDACCESS");
        retention(&app).await;
        assert_eq!(
            app.engine.lock().unwrap().state.voicemails["vm"].state,
            "purged"
        );
        let mock = mock.lock().unwrap();
        let original = format!("/original/old/{relative}");
        let wrong = format!("/replacement/new/{relative}");
        let addresses: Vec<_> = mock
            .calls
            .iter()
            .map(|(method, path, _)| (method.as_str(), path.as_str()))
            .collect();
        assert_eq!(
            addresses,
            vec![
                ("PUT", original.as_str()),
                ("PUT", &original),
                ("GET", &original),
                ("GET", &wrong),
                ("HEAD", &wrong),
                ("HEAD", &original),
                ("DELETE", &original)
            ]
        );
        assert!(mock.calls[0].2.contains("Credential=INITIALACCESS/"));
        assert!(mock.calls[1..]
            .iter()
            .all(|(_, _, signature)| signature.contains("Credential=ROTATEDACCESS/")));
        assert!(mock.objects.is_empty());
        server.abort();
    }

    #[tokio::test]
    async fn expired_voicemail_can_be_replaced_and_private_audio_is_purged() {
        let dir = tempfile::tempdir().unwrap();
        let app = app(dir.path());
        let (vid, aid, path, sid, alice) = {
            let mut e = app.engine.lock().unwrap();
            let (alice, token) = login(&mut e, "c:alice");
            let (bob, _) = login(&mut e, "c:bob");
            let ring = e
                .dispatch(&alice, "calls.init", &json!({"target":"c:bob"}))
                .unwrap()["ringid"]
                .clone();
            e.dispatch(
                &bob,
                "calls.decline",
                &json!({"ringid":ring,"give_no_reason":true}),
            )
            .unwrap();
            let draft = e
                .dispatch(
                    &alice,
                    "voicemail.begin",
                    &json!({"ringid":ring,"format":"audio"}),
                )
                .unwrap();
            let vid = draft["voicemail_id"].as_str().unwrap().to_owned();
            let (tx, _rx) = tokio::sync::mpsc::channel(8);
            let mut media = app.media.lock().unwrap();
            let attached = media.attach(&mut e, &alice, &token,
                &json!({"purpose":"voicemail","voicemail_id":vid,"ringid":ring,"device_id":alice.device_id}), tx).unwrap();
            let sid = attached["stream_id"].as_str().unwrap().to_owned();
            let aid = e.state.voicemails[&vid].audio_asset_id.clone().unwrap();
            let path = e.state.assets[&aid].path.clone();
            assert!(std::path::Path::new(&path).exists());
            e.state.voicemails.get_mut(&vid).unwrap().expires_at = after(-1);
            assert!(e
                .dispatch(&alice, "voicemail.commit", &json!({"voicemail_id":vid}))
                .is_err());
            // Begin must recover even before the periodic lifecycle tick runs.
            let replacement = e
                .dispatch(
                    &alice,
                    "voicemail.begin",
                    &json!({"ringid":ring,"format":"audio"}),
                )
                .unwrap();
            assert_ne!(replacement["voicemail_id"], vid);
            assert_eq!(e.state.voicemails[&vid].state, "aborted");
            e.dispatch(&alice, "voicemail.abort", &json!({"voicemail_id":vid}))
                .unwrap();
            media.tick(&mut e);
            assert!(!media.streams.contains_key(&sid));
            let replacement = replacement["voicemail_id"].as_str().unwrap();
            e.state.voicemails.get_mut(replacement).unwrap().expires_at = after(-1);
            assert!(e.tick(false));
            assert_eq!(e.state.voicemails[replacement].state, "aborted");
            (vid, aid, path, sid, alice)
        };
        retention(&app).await;
        let mut e = app.engine.lock().unwrap();
        assert_eq!(e.state.voicemails[&vid].state, "purged");
        assert!(!e.state.assets.contains_key(&aid));
        assert!(!std::path::Path::new(&path).exists());
        assert!(!app.media.lock().unwrap().streams.contains_key(&sid));
        assert!(app
            .media
            .lock()
            .unwrap()
            .audio(&mut e, &alice, &json!({"stream_id":sid}))
            .is_err());
    }
}
