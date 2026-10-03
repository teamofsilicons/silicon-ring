use crate::{auth::storage_error, model::*, App};
use serde_json::{json, Value};
use std::time::Duration;

pub fn start(app: App) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(5));
        loop {
            interval.tick().await;
            if app.providers_disabled {
                continue;
            }
            sync(&app).await;
            retention(&app).await;
        }
    });
}
async fn sync(app: &App) {
    let assets = {
        let e = app.engine.lock().unwrap();
        e.state
            .assets
            .values()
            .filter(|a| {
                a.complete
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
        let owner = a.owner.split('|').collect::<Vec<_>>();
        if owner.len() != 3 {
            continue;
        }
        let realm = owner[0];
        let org = owner[1];
        if realm == "test" && !app.test_tokens.is_empty() {
            continue;
        }
        let bucket = crate::settings::s3(app, realm, org).await;
        let Some(store) = (match bucket {
            Ok(v) => v,
            Err(error) => {
                failed(app, &a, &error.message);
                continue;
            }
        }) else {
            continue;
        };
        let object = format!("{realm}/{}/{}/{}", digest(org), a.purpose, a.asset_id);
        let bytes = match std::fs::read(&a.path) {
            Ok(v) => v,
            Err(_) => {
                failed(
                    app,
                    &a,
                    "Local audio is missing; no S3 upload was attempted.",
                );
                continue;
            }
        };
        let result = store.put(&object, &a.mime_type, bytes).await;
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
                e.state.storage.insert(
                    a.asset_id.clone(),
                    json!({"status":"ready","object_key":object,"stored_at":now()}),
                );
                let _ = e.persist();
            }
            Err(error) => failed(app, &a, &error.message),
        }
    }
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
    e.state.storage.insert(
        a.asset_id.clone(),
        json!({"status":"failed","error":message,"attempts":attempts,"retry_at":after(wait)}),
    );
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
    let store = crate::settings::s3(app, &i.realm, &i.org_id)
        .await
        .map_err(crate::provider_error)?
        .ok_or_else(|| {
            Fault::new(
                "STORAGE_NOT_CONFIGURED",
                "S3 storage is not configured.",
                "Configure the recording's organization storage.",
            )
        })?;
    let bytes = store
        .get(required(&stored, "object_key")?)
        .await
        .map_err(crate::provider_error)?;
    std::fs::write(&a.path, bytes).map_err(storage_error)?;
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
        for v in e
            .state
            .voicemails
            .values()
            .filter(|v| v.state == "delivered" || v.state == "deleted" || v.state == "aborted")
        {
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
        if let Some(aid) = &aid {
            let stored = app.engine.lock().unwrap().state.storage.get(aid).cloned();
            if let Some(stored) = stored {
                let store = match crate::settings::s3(app, &realm, &org).await {
                    Ok(Some(s)) => s,
                    _ => continue,
                };
                if let Some(key) = stored["object_key"].as_str() {
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
            if let Some(a) = e.state.assets.get(aid) {
                if let Err(error) = std::fs::remove_file(&a.path) {
                    if error.kind() != std::io::ErrorKind::NotFound {
                        continue;
                    }
                }
            }
        }
        let mut e = app.engine.lock().unwrap();
        if let Some(aid) = aid {
            e.state.assets.remove(&aid);
            e.state.storage.remove(&aid);
        }
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
