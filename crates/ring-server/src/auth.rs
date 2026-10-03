use crate::{model::*, provider_error, App};
use serde_json::{json, Value};

// ponytail: serialize Ting credential updates globally; use per-owner locks if throughput requires it.
static TING_CREDENTIALS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

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
/// Resolve a global actor through their own current IAM grant in this realm.
pub async fn verify_recipient(app: &App, caller: &Identity, actor: &str) -> Result<Identity> {
    let candidates = {
        let e = app.engine.lock().unwrap();
        e.state.registered_identities(&caller.realm, actor)
    };
    let mut error = recipient_unavailable();
    for recipient in candidates {
        if recipient.realm != caller.realm || recipient.actor != actor {
            continue;
        }
        match verify(app, &recipient).await {
            Ok(_) => return Ok(recipient),
            Err(fault) => error.retryable |= fault.retryable,
        }
    }
    Err(error)
}
fn recipient_unavailable() -> Fault {
    Fault::new(
        "RECIPIENT_UNAVAILABLE",
        "The recipient is not currently authorized for Ring.",
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
    let _credentials = TING_CREDENTIALS.lock().await;
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
        // A completed code exchange survives a failed subscription registration and grant refresh.
        if !stored.is_some_and(|v| v["authorization_id"] == aid) {
            let grants = iam
                .exchange_ting_consent(aid, code, rid)
                .await
                .map_err(provider_error)?;
            let value =
                serde_json::to_value(grants).map_err(|_| invalid("Invalid IAM grant response"))?;
            validate_grants(i, &value)?;
            app.vault.set(&format!("ting:{owner}"), &json!({"authorization_id":aid,"grants":value,"refresh_request_id":id("ting_refresh")})).map_err(storage_error)?;
        }
        let registration = ting_endpoint_token(app, i, "subscriptions.register").await?;
        ring_providers::Ting::for_realm(i.realm == "test")
            .map_err(provider_error)?
            .register_subscription(&registration, &i.org_id, &i.actor)
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
    let _credentials = TING_CREDENTIALS.lock().await;
    let i = Identity {
        actor: n.actor.clone(),
        org_id: n.org_id.clone(),
        realm: n.realm.clone(),
        display_name: String::new(),
        admin: false,
    };
    ting_endpoint_token(app, &i, "tings.send").await
}
// Callers hold TING_CREDENTIALS across read, refresh and durable replacement.
async fn ting_endpoint_token(app: &App, i: &Identity, endpoint: &str) -> Result<String> {
    let owner = key(&i.realm, &i.org_id, &i.actor);
    let k = format!("ting:{owner}");
    let mut saved = app.vault.get(&k).map_err(storage_error)?.ok_or_else(|| {
        Fault::new(
            "TING_CONSENT_REQUIRED",
            "Ring notification consent is required for this silicon.",
            "Run ring notifications authorize, approve the IAM consent, and exchange its code.",
        )
    })?;
    let index = saved["grants"]["items"]
        .as_array()
        .and_then(|items| {
            items
                .iter()
                .position(|v| v["audience"] == "ting" && v["endpoint_id"] == endpoint)
        })
        .ok_or_else(|| {
            if endpoint == "subscriptions.register" {
                Fault::new(
                    "TING_SUBSCRIPTION_AUTH_REQUIRED",
                    "IAM must grant recipient subscription registration.",
                    "Request notification consent including subscriptions.register.",
                )
            } else {
                forbidden()
            }
        })?;
    let item = saved["grants"]["items"][index].clone();
    let expiry = item["expires_at"]
        .as_str()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|t| t.timestamp())
        .unwrap_or(0);
    if expiry <= chrono::Utc::now().timestamp() + 30 {
        // Preserve legacy send-refresh IDs, including a response lost before this upgrade.
        if saved["refresh_request_ids"][endpoint].as_str().is_none() {
            saved["refresh_request_ids"][endpoint] = if endpoint == "tings.send" {
                saved["refresh_request_id"].as_str().map(String::from)
            } else {
                None
            }
            .unwrap_or_else(|| id("ting_refresh"))
            .into();
            app.vault.set(&k, &saved).map_err(storage_error)?;
        }
        let iam = ring_providers::Iam::from_env(i.realm == "test").map_err(provider_error)?;
        let refreshed = iam
            .refresh_ting_consent(
                required(&item, "refresh_token")?,
                required(&saved["refresh_request_ids"], endpoint)?,
            )
            .await
            .map_err(provider_error)?;
        let refreshed =
            serde_json::to_value(refreshed).map_err(|_| invalid("Invalid Ting grant refresh"))?;
        let items = refreshed["items"].as_array().ok_or_else(forbidden)?;
        let [replacement] = items.as_slice() else {
            return Err(forbidden());
        };
        if replacement["grant_id"] != item["grant_id"]
            || replacement["audience"] != item["audience"]
            || replacement["endpoint_id"] != item["endpoint_id"]
        {
            return Err(forbidden());
        }
        // A root refresh returns only its own pair, not the complete consent grant set.
        saved["grants"]["items"][index] = replacement.clone();
        validate_grants(i, &saved["grants"])?;
        saved["refresh_request_ids"][endpoint] = id("ting_refresh").into();
        app.vault.set(&k, &saved).map_err(storage_error)?;
    }
    required(&saved["grants"]["items"][index], "access_token").map(String::from)
}

pub fn revoke_identity(app: &App, i: &Identity, reason: &str) -> Result<()> {
    let mut e = app.engine.lock().unwrap();
    let rings: Vec<_> = e
        .state
        .calls
        .values()
        .filter(|c| {
            c.realm == i.realm && c.identity(&i.actor).org_id == i.org_id && c.active(&i.actor)
        })
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
        .filter(|c| matches!(c.state.as_str(), "active" | "ringing" | "connecting"))
    {
        for p in call.participants.iter().filter(|p| p.left_at.is_none()) {
            let i = call.identity(&p.actor);
            ids.entry(key(&i.realm, &i.org_id, &i.actor)).or_insert(i);
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
        login_in(app, actor, "org", "production")
    }
    fn login_in(app: &App, actor: &str, org: &str, realm: &str) -> Session {
        let mut e = app.engine.lock().unwrap();
        let value = e
            .login(
                Identity {
                    actor: actor.into(),
                    org_id: org.into(),
                    realm: realm.into(),
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
        let bob = login_in(&app, "si:bob", "recipient-org", "production");
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
        assert_eq!(
            monitored_identities(&app)
                .iter()
                .find(|i| i.actor == "si:bob")
                .unwrap()
                .org_id,
            "recipient-org"
        );
        let mut unrelated_context = bob.identity.clone();
        unrelated_context.org_id = "unrelated-org".into();
        revoke_identity(&app, &unrelated_context, "membership_removed").unwrap();
        assert!(app.engine.lock().unwrap().state.calls[&ring].active("si:bob"));
        revoke_identity(&app, &bob.identity, "membership_removed").unwrap();
        assert!(!app.engine.lock().unwrap().state.calls[&ring].active("si:bob"));
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
    async fn global_recipient_uses_their_own_context_and_preserves_realm_isolation() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app(dir.path());
        let alice = login_in(&app, "si:alice", "caller-org", "test");
        let bob = login_in(&app, "si:bob", "recipient-org", "test");
        login_in(&app, "si:production-only", "caller-org", "production");
        app.test_tokens = Arc::new(BTreeMap::from([("isolated-token".into(), bob.identity)]));
        let recipient = verify_recipient(&app, &alice.identity, "si:bob")
            .await
            .unwrap();
        assert_eq!(recipient.actor, "si:bob");
        assert_eq!(recipient.org_id, "recipient-org");
        assert_eq!(recipient.realm, "test");
        assert_eq!(
            verify_recipient(&app, &alice.identity, "si:production-only")
                .await
                .unwrap_err()
                .code,
            "RECIPIENT_UNAVAILABLE"
        );
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

    #[tokio::test]
    async fn interrupted_notification_consent_refreshes_only_its_endpoint_and_replays_safely() {
        // Isolate provider configuration from the other tests; every request stays on loopback.
        const CHILD: &str = "RING_AUTH_REFRESH_TEST_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "auth::tests::interrupted_notification_consent_refreshes_only_its_endpoint_and_replays_safely", "--nocapture"])
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
        use axum::{extract::State, http::HeaderMap, routing::post, Json, Router};
        #[derive(Default)]
        struct Provider {
            refreshes: Vec<(String, String)>,
            receipts: BTreeMap<String, (String, Value)>,
            registrations: Vec<Value>,
        }
        fn pair(endpoint: &str, refreshed: bool) -> Value {
            json!({
                "grant_id": if endpoint == "tings.send" { "00000000-0000-4000-8000-000000000001" } else { "00000000-0000-4000-8000-000000000002" },
                "audience":"ting", "endpoint_id":endpoint, "org_id":"org",
                "actor":{"type":"silicon","public_id":"si:alice"}, "scope":"",
                "access_token":format!("{endpoint}-access-{refreshed}"),
                "refresh_token":format!("{endpoint}-refresh-{refreshed}"),
                "token_type":"Bearer", "expires_in":3600,
                "expires_at": if refreshed { after(3600) } else { after(-3600) }
            })
        }
        async fn refresh(
            State(state): State<Arc<Mutex<Provider>>>,
            headers: HeaderMap,
            Json(body): Json<Value>,
        ) -> Json<Value> {
            assert!(
                body["authorization_code"].is_null(),
                "must not exchange the code again"
            );
            let token = body["refresh_token"].as_str().unwrap().to_string();
            let request = headers["idempotency-key"].to_str().unwrap().to_string();
            let endpoint = token.strip_suffix("-refresh-false").unwrap();
            let mut state = state.lock().unwrap();
            state.refreshes.push((token.clone(), request.clone()));
            if let Some((original, response)) = state.receipts.get(&token) {
                assert_eq!(
                    &request, original,
                    "rotated tokens must replay the original mutation"
                );
                return Json(response.clone());
            }
            let response = json!({"items":[pair(endpoint, true)]});
            state
                .receipts
                .insert(token.clone(), (request, response.clone()));
            if endpoint == "subscriptions.register" {
                // IAM rotated the token but its response was lost/unreadable.
                Json(json!({}))
            } else {
                Json(response)
            }
        }
        async fn register(
            State(state): State<Arc<Mutex<Provider>>>,
            headers: HeaderMap,
            Json(body): Json<Value>,
        ) -> Json<Value> {
            assert_eq!(
                headers["authorization"],
                "Bearer subscriptions.register-access-true"
            );
            assert_eq!(
                body,
                json!({"org_id":"org","app_id":"ring","for":"si:alice"})
            );
            let mut state = state.lock().unwrap();
            state.registrations.push(body.clone());
            if state.registrations.len() == 1 {
                // Ting committed this tuple but the confirmation was lost/unreadable.
                Json(json!({}))
            } else {
                Json(json!({"app_id":"ring","for":"si:alice","active":true}))
            }
        }
        let provider = Arc::new(Mutex::new(Provider::default()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let router = Router::new()
            .route("/api/v1/obo-access/tokens", post(refresh))
            .route("/v1/subscriptions", post(register))
            .with_state(provider.clone());
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        std::env::set_var("RING_IAM_URL", &url);
        std::env::set_var("RING_TING_URL", &url);
        std::env::set_var("RING_IAM_APP_ID", "ring");
        std::env::set_var("RING_IAM_APP_SECRET", "mock-app-secret");
        let dir = tempfile::tempdir().unwrap();
        let service = app(dir.path());
        let identity = login(&service, "si:alice").identity;
        let owner = "ting:production|org|si:alice";
        let consent = "ting-consent:production|org|si:alice";
        let params = json!({"authorization_id":"consent-1","code":"one-use-code"});
        let untouched = pair("tings.read", true);
        let saved = json!({"authorization_id":"consent-1", "refresh_request_id":"legacy-send-refresh", "grants":{"items":[pair("tings.send", false), pair("subscriptions.register", false), untouched]}});
        service.vault.set(owner, &saved).unwrap();
        service.vault.set(consent, &params).unwrap();
        assert!(notifications(&service, &identity, "resume", &params)
            .await
            .is_err());
        let pending = service.vault.get(owner).unwrap().unwrap();
        assert_eq!(pending["grants"], saved["grants"]);
        assert_eq!(pending["authorization_id"], "consent-1");
        let register_request = pending["refresh_request_ids"]["subscriptions.register"].clone();
        drop(service);

        // The outbox refreshes another endpoint between registration retries, after a restart.
        let service = app(dir.path());
        let publication = Publication {
            notification_id: "pending".into(),
            actor: identity.actor.clone(),
            org_id: identity.org_id.clone(),
            realm: identity.realm.clone(),
            event_type: "call.incoming".into(),
            data: json!({}),
            status: "failed".into(),
            attempts: 0,
            retry_at: None,
            error: None,
        };
        assert_eq!(
            ting_token(&service, &publication).await.unwrap(),
            "tings.send-access-true"
        );
        let pending = service.vault.get(owner).unwrap().unwrap();
        assert_eq!(pending["authorization_id"], "consent-1");
        assert_eq!(
            pending["refresh_request_ids"]["subscriptions.register"],
            register_request
        );
        assert_eq!(pending["grants"]["items"][1], saved["grants"]["items"][1]);
        assert!(notifications(&service, &identity, "resume", &params)
            .await
            .is_err());
        let refreshed = service.vault.get(owner).unwrap().unwrap();
        assert_eq!(
            refreshed["grants"]["items"][0],
            pending["grants"]["items"][0]
        );
        assert_eq!(refreshed["grants"]["items"][2], untouched);
        assert_ne!(
            refreshed["refresh_request_ids"]["subscriptions.register"],
            register_request
        );
        assert_eq!(
            refreshed["grants"]["items"][1]["refresh_token"],
            "subscriptions.register-refresh-true"
        );
        assert!(service.vault.get(consent).unwrap().is_some());
        drop(service);

        let service = app(dir.path());
        let result = notifications(&service, &identity, "resume", &params)
            .await
            .unwrap();
        assert_eq!(result["authorized"], true);
        assert!(service.vault.get(consent).unwrap().is_none());
        assert_eq!(
            notifications(&service, &identity, "resume", &params)
                .await
                .unwrap(),
            result
        );
        let provider = provider.lock().unwrap();
        assert_eq!(provider.refreshes.len(), 3);
        assert_eq!(provider.refreshes[0], provider.refreshes[2]);
        assert_eq!(provider.refreshes[1].1, digest("legacy-send-refresh"));
        assert_ne!(provider.refreshes[0].1, provider.refreshes[1].1);
        assert_eq!(provider.registrations.len(), 2);
        assert_eq!(provider.registrations[0], provider.registrations[1]);
        server.abort();
    }
}
