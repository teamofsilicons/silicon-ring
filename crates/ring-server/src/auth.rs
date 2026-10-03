use crate::{model::*, provider_error, App};
use serde_json::{json, Value};

pub fn storage_error(_: std::io::Error) -> Fault {
    Fault::new(
        "CREDENTIAL_STORAGE_FAILED",
        "Encrypted credential storage was not confirmed.",
        "Check the server encryption key and disk permissions.",
    )
}
pub fn credential_key(i: &Identity) -> String {
    format!("iam:{}", key(&i.realm, &i.org_id, &i.actor))
}
pub fn save(app: &App, s: &ring_providers::IamSession) -> Result<()> {
    let realm = if s.identity.realm == "production" {
        "production"
    } else {
        "test"
    };
    let key = format!(
        "iam:{}",
        key(realm, &s.identity.org_id, &s.identity.actor_id)
    );
    app.vault.set(&key,&json!({"access_token":s.access_token,"refresh_token":s.refresh_token,"expires_at":s.identity.expires_at,"refresh_request_id":id("refresh")})).map_err(storage_error)
}
/// Introspection on every control operation closes membership and permission changes promptly.
/// Microphone frames use the session expiry and periodic revalidation, not 50 IAM calls per second.
pub async fn verify(app: &App, i: &Identity) -> Result<String> {
    if i.realm == "test" && !app.test_tokens.is_empty() {
        return Ok(String::new());
    }
    let k = credential_key(i);
    let saved = app.vault.get(&k).map_err(storage_error)?.ok_or_else(|| {
        Fault::new(
            "IAM_LOGIN_REQUIRED",
            "No IAM credential is retained for this Ring session.",
            "Log in with a fresh IAM token.",
        )
    })?;
    let iam = ring_providers::Iam::from_env(i.realm == "test").map_err(provider_error)?;
    let token = if saved["expires_at"].as_i64().unwrap_or(0) <= chrono::Utc::now().timestamp() + 30
    {
        let refresh = required(&saved, "refresh_token")?;
        let refreshed = iam
            .refresh(refresh, &i.org_id, required(&saved, "refresh_request_id")?)
            .await
            .map_err(provider_error)?;
        if refreshed.identity.actor_id != i.actor {
            return Err(forbidden());
        }
        save(app, &refreshed)?;
        refreshed.access_token
    } else {
        required(&saved, "access_token")?.to_string()
    };
    let verified = iam
        .verify(&token, &i.org_id)
        .await
        .map_err(provider_error)?;
    if verified.actor_id != i.actor || verified.org_id != i.org_id {
        return Err(forbidden());
    }
    let mut e = app.engine.lock().unwrap();
    for s in e.state.sessions.values_mut().filter(|s| {
        s.identity.actor == i.actor && s.identity.org_id == i.org_id && s.identity.realm == i.realm
    }) {
        s.identity.admin = verified.admin;
        s.identity.display_name = verified.display_name.clone();
    }
    Ok(token)
}
/// A retained profile is not evidence of current organization membership.
pub async fn verify_recipient(app: &App, caller: &Identity, actor: &str) -> Result<()> {
    let recipient = Identity {
        actor: actor.into(),
        org_id: caller.org_id.clone(),
        realm: caller.realm.clone(),
        display_name: actor.into(),
        admin: false,
    };
    if !app.engine.lock().unwrap().state.profiles.contains_key(&key(
        &recipient.realm,
        &recipient.org_id,
        actor,
    )) {
        return Err(recipient_unavailable());
    }
    verify(app, &recipient).await.map(|_| ()).map_err(|e| {
        let mut error = recipient_unavailable();
        error.retryable = e.retryable;
        error
    })
}
fn recipient_unavailable() -> Fault {
    Fault::new(
        "RECIPIENT_UNAVAILABLE",
        "The recipient is not currently authorized for Ring in this organization.",
        "Confirm the recipient has logged in to Ring with current IAM membership, then retry.",
    )
}
pub async fn notifications(app: &App, i: &Identity, rid: &str, p: &Value) -> Result<Value> {
    if !silicon(&i.actor) {
        return Err(invalid("Ting authorization applies to silicon identities"));
    }
    if i.realm == "test" && !app.test_tokens.is_empty() {
        return Ok(
            json!({"status":"isolated_test","authorized":false,"message":"Local test notifications are retained in the outbox and never sent to production Ting."}),
        );
    }
    let cache_key = format!("{}|{rid}", key(&i.realm, &i.org_id, &i.actor));
    let fingerprint = digest(&format!("notifications.authorize:{p}"));
    if let Some(result) = notification_retry(app, &cache_key, &fingerprint)? {
        return Ok(result);
    }
    let iam = ring_providers::Iam::from_env(i.realm == "test").map_err(provider_error)?;
    let owner = key(&i.realm, &i.org_id, &i.actor);
    if let Some(code) = p["code"].as_str() {
        let aid = required(p, "authorization_id")?;
        let pending = app
            .vault
            .get(&format!("ting-consent:{owner}"))
            .map_err(storage_error)?
            .ok_or_else(|| invalid("Request a Ting consent URL before exchanging its code"))?;
        if pending["authorization_id"] != aid {
            return Err(forbidden());
        }
        let stored = app
            .vault
            .get(&format!("ting:{owner}"))
            .map_err(storage_error)?;
        let value = if let Some(stored) = stored.filter(|v| v["authorization_id"] == aid) {
            // A completed code exchange survives a failed subscription registration.
            stored["grants"].clone()
        } else {
            let grants = iam
                .exchange_ting_consent(aid, code, rid)
                .await
                .map_err(provider_error)?;
            let value =
                serde_json::to_value(grants).map_err(|_| invalid("Invalid IAM grant response"))?;
            validate_grants(i, &value)?;
            app.vault.set(&format!("ting:{owner}"), &json!({"authorization_id":aid,"grants":value,"refresh_request_id":id("ting_refresh")})).map_err(storage_error)?;
            value
        };
        let registration = value["items"]
            .as_array()
            .and_then(|items| {
                items.iter().find(|v| {
                    v["audience"] == "ting" && v["endpoint_id"] == "subscriptions.register"
                })
            })
            .and_then(|v| v["access_token"].as_str())
            .ok_or_else(|| {
                Fault::new(
                    "TING_SUBSCRIPTION_AUTH_REQUIRED",
                    "IAM must grant recipient subscription registration.",
                    "Request notification consent including subscriptions.register.",
                )
            })?;
        ring_providers::Ting::for_realm(i.realm == "test")
            .map_err(provider_error)?
            .register_subscription(registration, &i.org_id, &i.actor)
            .await
            .map_err(provider_error)?;
        let mut e = app.engine.lock().unwrap();
        let before = e.state.clone();
        let mut queued = 0;
        for n in e.state.publications.values_mut().filter(|n| {
            n.realm == i.realm && n.org_id == i.org_id && n.actor == i.actor && n.status == "failed"
        }) {
            n.status = "pending".into();
            queued += 1;
        }
        let result = json!({"authorized":true,"queued":queued});
        e.state.requests.insert(
            cache_key,
            Cached {
                fingerprint,
                response: json!({"id":rid,"ok":true,"result":result}),
            },
        );
        if let Err(error) = e.persist() {
            e.state = before;
            return Err(error);
        }
        drop(e);
        // Keep the completed response durable before retiring the one-time code receipt.
        app.vault
            .remove(&format!("ting-consent:{owner}"))
            .map_err(storage_error)?;
        return Ok(result);
    }
    let token = verify(app, i).await?;
    let consent = iam
        .request_ting_consent(&token, &i.org_id, rid)
        .await
        .map_err(provider_error)?;
    app.vault
        .set(&format!("ting-consent:{owner}"), &consent)
        .map_err(storage_error)?;
    let mut e = app.engine.lock().unwrap();
    e.state.requests.insert(
        cache_key.clone(),
        Cached {
            fingerprint,
            response: json!({"id":rid,"ok":true,"result":consent}),
        },
    );
    if let Err(error) = e.persist() {
        e.state.requests.remove(&cache_key);
        return Err(error);
    }
    Ok(consent)
}
fn notification_retry(app: &App, cache_key: &str, fingerprint: &str) -> Result<Option<Value>> {
    let e = app.engine.lock().unwrap();
    e.state
        .requests
        .get(cache_key)
        .map(|cached| {
            if cached.fingerprint == fingerprint {
                Ok(cached.response["result"].clone())
            } else {
                Err(Fault::new(
                    "REQUEST_ID_REUSED",
                    "Request ID was already used with different input.",
                    "Use a new request ID for changed input.",
                ))
            }
        })
        .transpose()
}
fn validate_grants(i: &Identity, value: &Value) -> Result<()> {
    let items = value["items"]
        .as_array()
        .ok_or_else(|| invalid("IAM returned no Ting grants"))?;
    if !items.iter().any(|v| {
        v["audience"] == "ting" && v["endpoint_id"] == "tings.send" && v["org_id"] == i.org_id
    }) {
        return Err(forbidden());
    }
    for grant in items {
        if grant["org_id"] != i.org_id || grant["audience"] != "ting" {
            return Err(forbidden());
        }
        if let Some(actor) = grant["actor"]["public_id"].as_str() {
            if actor != i.actor {
                return Err(forbidden());
            }
        }
        if i.realm == "production" && !grant["testing_context"].is_null() {
            return Err(forbidden());
        }
    }
    Ok(())
}
pub async fn ting_token(app: &App, n: &Publication) -> Result<String> {
    let owner = key(&n.realm, &n.org_id, &n.actor);
    let k = format!("ting:{owner}");
    let saved = app.vault.get(&k).map_err(storage_error)?.ok_or_else(|| {
        Fault::new(
            "TING_CONSENT_REQUIRED",
            "Ring notification consent is required for this silicon.",
            "Run ring notifications authorize, approve the IAM consent, and exchange its code.",
        )
    })?;
    let mut grants = saved["grants"].clone();
    let item = grants["items"]
        .as_array()
        .and_then(|a| {
            a.iter()
                .find(|v| v["audience"] == "ting" && v["endpoint_id"] == "tings.send")
        })
        .ok_or_else(forbidden)?;
    let expiry = item["expires_at"]
        .as_str()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|t| t.timestamp())
        .unwrap_or(0);
    if expiry <= chrono::Utc::now().timestamp() + 30 {
        let iam = ring_providers::Iam::from_env(n.realm == "test").map_err(provider_error)?;
        let refreshed = iam
            .refresh_ting_consent(
                required(item, "refresh_token")?,
                required(&saved, "refresh_request_id")?,
            )
            .await
            .map_err(provider_error)?;
        grants =
            serde_json::to_value(refreshed).map_err(|_| invalid("Invalid Ting grant refresh"))?;
        let i = Identity {
            actor: n.actor.clone(),
            org_id: n.org_id.clone(),
            realm: n.realm.clone(),
            display_name: String::new(),
            admin: false,
        };
        validate_grants(&i, &grants)?;
        app.vault
            .set(
                &k,
                &json!({"grants":grants,"refresh_request_id":id("ting_refresh")}),
            )
            .map_err(storage_error)?;
    }
    grants["items"]
        .as_array()
        .and_then(|a| {
            a.iter()
                .find(|v| v["audience"] == "ting" && v["endpoint_id"] == "tings.send")
        })
        .and_then(|v| v["access_token"].as_str())
        .map(String::from)
        .ok_or_else(forbidden)
}

pub fn revoke_identity(app: &App, i: &Identity, reason: &str) -> Result<()> {
    let mut e = app.engine.lock().unwrap();
    let rings: Vec<_> = e
        .state
        .calls
        .values()
        .filter(|c| c.realm == i.realm && c.org_id == i.org_id && c.active(&i.actor))
        .map(|c| c.ringid.clone())
        .collect();
    let control = Session {
        identity: i.clone(),
        device_id: String::new(),
        expires_at: after(1),
    };
    for ring in rings {
        let _ = e.dispatch(&control, "calls.cut", &json!({"ringid":ring}));
    }
    let owner = key(&i.realm, &i.org_id, &i.actor);
    let mut devices = Vec::new();
    for d in e.state.devices.values_mut().filter(|d| d.owner == owner) {
        d.push_token = None;
        devices.push(d.device_id.clone());
    }
    e.state.pushes.retain(|_, p| {
        !p["device_id"]
            .as_str()
            .is_some_and(|id| devices.iter().any(|d| d == id))
    });
    e.state.sessions.retain(|_, s| {
        !(s.identity.realm == i.realm
            && s.identity.org_id == i.org_id
            && s.identity.actor == i.actor)
    });
    e.state.event(
        i,
        vec![i.actor.clone()],
        "session.revoked",
        json!({"reason":reason}),
    );
    e.persist()
}
pub fn start_revalidation(app: App) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
        loop {
            interval.tick().await;
            if app.providers_disabled {
                continue;
            }
            let identities = monitored_identities(&app);
            for i in identities {
                if verify(&app, &i).await.is_err() {
                    let _ = revoke_identity(&app, &i, "iam_authority_lost");
                }
            }
        }
    });
}

/// Server-hosted representatives outlive CLI sessions and still require current IAM authority.
pub fn monitored_identities(app: &App) -> Vec<Identity> {
    let e = app.engine.lock().unwrap();
    let mut ids = std::collections::BTreeMap::new();
    for session in e.state.sessions.values().filter(|s| s.expires_at > now()) {
        let i = &session.identity;
        ids.insert(key(&i.realm, &i.org_id, &i.actor), i.clone());
    }
    for call in e
        .state
        .calls
        .values()
        .filter(|c| matches!(c.state.as_str(), "active" | "ringing"))
    {
        for p in call.participants.iter().filter(|p| p.left_at.is_none()) {
            ids.entry(key(&call.realm, &call.org_id, &p.actor))
                .or_insert(Identity {
                    actor: p.actor.clone(),
                    realm: call.realm.clone(),
                    org_id: call.org_id.clone(),
                    display_name: p.display_name.clone(),
                    admin: false,
                });
        }
    }
    ids.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{engine::Engine, media::Media, vault::Vault};
    use std::{
        collections::BTreeMap,
        sync::{Arc, Mutex},
    };

    fn app(dir: &std::path::Path) -> App {
        App {
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
    fn login(app: &App, actor: &str) -> Session {
        let mut e = app.engine.lock().unwrap();
        let value = e
            .login(
                Identity {
                    actor: actor.into(),
                    org_id: "org".into(),
                    realm: "production".into(),
                    display_name: actor.into(),
                    admin: false,
                },
                None,
            )
            .unwrap();
        e.session(value["session_token"].as_str().unwrap()).unwrap()
    }

    #[test]
    fn revoked_representatives_are_removed_even_after_client_sessions_expire() {
        let dir = tempfile::tempdir().unwrap();
        let app = app(dir.path());
        let alice = login(&app, "si:alice");
        let bob = login(&app, "si:bob");
        let ring = {
            let mut e = app.engine.lock().unwrap();
            let ring = e
                .dispatch(&alice, "calls.init", &json!({"target":"si:bob"}))
                .unwrap()["ringid"]
                .as_str()
                .unwrap()
                .to_string();
            e.dispatch(&bob, "calls.accept", &json!({"ringid":ring}))
                .unwrap();
            e.state.sessions.clear();
            e.state
                .devices
                .get_mut(&alice.device_id)
                .unwrap()
                .push_token = Some("private-token".into());
            e.state
                .pushes
                .insert("pending".into(), json!({"device_id":alice.device_id}));
            ring
        };
        assert_eq!(monitored_identities(&app).len(), 2);
        revoke_identity(&app, &alice.identity, "membership_removed").unwrap();
        let e = app.engine.lock().unwrap();
        assert!(!e.state.calls[&ring].active("si:alice"));
        assert!(e.state.devices[&alice.device_id].push_token.is_none());
        assert!(e.state.pushes.is_empty());
        assert!(e
            .state
            .events
            .iter()
            .any(|event| event.kind == "session.revoked"));
    }

    #[tokio::test]
    async fn stored_profiles_cannot_authorize_a_recipient_without_current_credentials() {
        let dir = tempfile::tempdir().unwrap();
        let app = app(dir.path());
        let alice = login(&app, "si:alice");
        login(&app, "si:bob");
        let error = verify_recipient(&app, &alice.identity, "si:bob")
            .await
            .unwrap_err();
        assert_eq!(error.code, "RECIPIENT_UNAVAILABLE");
        assert!(!error.retryable);
    }

    #[tokio::test]
    async fn completed_notification_consent_survives_lost_reply_and_restart() {
        let dir = tempfile::tempdir().unwrap();
        let service = app(dir.path());
        let identity = login(&service, "si:alice").identity;
        let params = json!({"authorization_id":"consent-1","code":"one-use-code"});
        let result = json!({"authorized":true,"queued":3});
        {
            let mut e = service.engine.lock().unwrap();
            e.state.requests.insert(
                "production|org|si:alice|retry-consent".into(),
                Cached {
                    fingerprint: digest(&format!("notifications.authorize:{params}")),
                    response: json!({"id":"retry-consent","ok":true,"result":result}),
                },
            );
            e.persist().unwrap();
        }
        drop(service);
        let restarted = app(dir.path());
        assert_eq!(
            notifications(&restarted, &identity, "retry-consent", &params)
                .await
                .unwrap(),
            result
        );
        let changed = json!({"authorization_id":"consent-2","code":"different-code"});
        assert_eq!(
            notifications(&restarted, &identity, "retry-consent", &changed)
                .await
                .unwrap_err()
                .code,
            "REQUEST_ID_REUSED"
        );
    }
}
