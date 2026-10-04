use super::*;
use std::{
    ffi::OsString,
    sync::atomic::{AtomicBool, Ordering},
};
use tokio::sync::Semaphore;
use tokio_tungstenite::{connect_async, tungstenite::Message as ClientMessage};

type ClientSocket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

struct Environment(Vec<(&'static str, Option<OsString>)>);
impl Environment {
    fn set(values: &[(&'static str, String)]) -> Self {
        Self(
            values
                .iter()
                .map(|(key, value)| {
                    let old = std::env::var_os(key);
                    std::env::set_var(key, value);
                    (*key, old)
                })
                .collect(),
        )
    }
}
impl Drop for Environment {
    fn drop(&mut self) {
        for (key, value) in &self.0 {
            if let Some(value) = value {
                std::env::set_var(key, value);
            } else {
                std::env::remove_var(key);
            }
        }
    }
}
#[derive(Clone)]
struct IamGate {
    block_next: Arc<AtomicBool>,
    entered: Arc<Semaphore>,
    release: Arc<Semaphore>,
}
async fn introspect(AxumState(gate): AxumState<IamGate>) -> Json<Value> {
    if gate.block_next.swap(false, Ordering::SeqCst) {
        gate.entered.add_permits(1);
        gate.release.acquire().await.unwrap().forget();
    }
    Json(
        json!({"active":true,"public_id":"c:alice","actor_type":"carbon","client_id":"ring","audience":"ring","expires_at":chrono::Utc::now().timestamp()+3600,
        "authorization":{"organization_id":"00000000-0000-4000-8000-000000000001","org_id":"org","membership_id":"c:alice[org]","membership_version":1,"authorization_epoch":1,"audience":"ring","scopes":[],"org_role":"member","actor_type":"carbon","public_id":"c:alice"}}),
    )
}
async fn send(socket: &mut ClientSocket, id: &str, method: &str, params: Value) {
    socket
        .send(ClientMessage::Text(
            json!({"id":id,"method":method,"params":params})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
}
async fn next_json(socket: &mut ClientSocket) -> Value {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match socket.next().await.unwrap().unwrap() {
                ClientMessage::Text(text) => return serde_json::from_str(&text).unwrap(),
                ClientMessage::Ping(bytes) => {
                    socket.send(ClientMessage::Pong(bytes)).await.unwrap()
                }
                _ => {}
            }
        }
    })
    .await
    .expect("WebSocket response timed out")
}
async fn reply(socket: &mut ClientSocket, id: &str) -> Value {
    let value = next_json(socket).await;
    assert_eq!(value["id"], id, "unexpected response: {value}");
    assert_eq!(value["ok"], true, "request failed: {value}");
    value["result"].clone()
}

#[tokio::test]
async fn slow_iam_preserves_media_events_request_order_and_disconnect_cleanup() {
    let gate = IamGate {
        block_next: Arc::new(AtomicBool::new(false)),
        entered: Arc::new(Semaphore::new(0)),
        release: Arc::new(Semaphore::new(0)),
    };
    let iam_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let iam_url = format!("http://{}", iam_listener.local_addr().unwrap());
    let iam_router = Router::new()
        .route("/api/v1/oauth/introspect", axum::routing::post(introspect))
        .route(
            "/api/v1/me",
            get(|| async { Json(json!({"display_name":"Alice"})) }),
        )
        .with_state(gate.clone());
    let iam_task =
        tokio::spawn(async move { axum::serve(iam_listener, iam_router).await.unwrap() });
    let _env = Environment::set(&[
        ("RING_IAM_URL", iam_url),
        ("RING_IAM_APP_ID", "ring".into()),
        ("RING_IAM_APP_SECRET", "connection-test-app-secret".into()),
    ]);
    let dir = tempfile::tempdir().unwrap();
    let mut engine = Engine::open(dir.path()).unwrap();
    let identity = Identity {
        actor: "c:alice".into(),
        org_id: "org".into(),
        realm: "production".into(),
        display_name: "Alice".into(),
        admin: false,
    };
    let login = engine.login(identity.clone(), None).unwrap();
    let token = login["session_token"].as_str().unwrap();
    let alice = engine.session(token).unwrap();
    let bob_login = engine
        .login(
            Identity {
                actor: "c:bob".into(),
                display_name: "Bob".into(),
                ..identity.clone()
            },
            None,
        )
        .unwrap();
    let bob = engine
        .session(bob_login["session_token"].as_str().unwrap())
        .unwrap();
    let ring = engine
        .dispatch(&alice, "calls.init", &json!({"target":"c:bob"}))
        .unwrap()["ringid"]
        .clone();
    engine
        .dispatch(&bob, "calls.accept", &json!({"ringid":ring}))
        .unwrap();
    let app = App {
        testing: None,
        environments: None,
        activity: Arc::default(),
        engine: Arc::new(Mutex::new(engine)),
        media: Arc::new(Mutex::new(media::Media::default())),
        test_tokens: Arc::new(BTreeMap::new()),
        test_secret: None,
        providers_disabled: true,
        vault: Arc::new(vault::Vault::open(dir.path()).unwrap()),
        telemetry: Arc::new(None),
        web_analytics: Arc::new(None),
        web_events: Arc::new(None),
        cli_telemetry: Arc::new(None),
    };
    app.vault
        .set(
            &auth::credential_key(&identity),
            &json!({"access_token":"test-token","expires_at":chrono::Utc::now().timestamp()+3600}),
        )
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}/ws", listener.local_addr().unwrap());
    let router = Router::new().route("/ws", get(ws)).with_state(app.clone());
    let server_task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let (mut socket, _) = connect_async(url).await.unwrap();
    // Pipelined authentication must observe the preceding hello, even though controls run separately.
    send(
        &mut socket,
        "hello",
        "protocol.hello",
        json!({"versions":[1],"org_id":"org"}),
    )
    .await;
    send(
        &mut socket,
        "resume",
        "auth.resume",
        json!({"session_token":token}),
    )
    .await;
    let hello = reply(&mut socket, "hello").await;
    reply(&mut socket, "resume").await;
    send(&mut socket, "info", "app.info", json!({})).await;
    let info = reply(&mut socket, "info").await;
    assert_eq!(info["capabilities"], hello["capabilities"]);
    assert_eq!(info["capabilities"], json!(CAPABILITIES));
    send(
        &mut socket,
        "subscribe",
        "events.subscribe",
        json!({"topics":["test."]}),
    )
    .await;
    reply(&mut socket, "subscribe").await;
    send(
        &mut socket,
        "attach",
        "media.attach",
        json!({"ringid":ring,"device_id":alice.device_id}),
    )
    .await;
    let attached = reply(&mut socket, "attach").await;
    let stream_id = attached["stream_id"].as_str().unwrap().to_string();
    app.engine.lock().unwrap().iam_verified.clear();
    gate.block_next.store(true, Ordering::SeqCst);
    send(&mut socket, "slow", "auth.status", json!({})).await;
    tokio::time::timeout(Duration::from_secs(3), gate.entered.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    // Two exact retries wait behind the first control request and must return the same result.
    send(
        &mut socket,
        "same-operation",
        "config.set",
        json!({"values":{"telemetry.enabled":false}}),
    )
    .await;
    send(
        &mut socket,
        "same-operation",
        "config.set",
        json!({"values":{"telemetry.enabled":false}}),
    )
    .await;
    socket.send(ClientMessage::Text(json!({"type":"media.audio","data":{"stream_id":stream_id,"seq":0,"offset_ms":0,"audio_base64":STANDARD.encode(vec![0;960])}}).to_string().into())).await.unwrap();
    socket
        .send(ClientMessage::Ping(b"during-iam".to_vec().into()))
        .await
        .unwrap();
    app.engine.lock().unwrap().state.event(
        &identity,
        vec![identity.actor.clone()],
        "test.live",
        json!({"value":1}),
    );
    let mut pong = false;
    let mut event = false;
    tokio::time::timeout(Duration::from_secs(2), async {
        while !pong || !event {
            match socket.next().await.unwrap().unwrap() {
                ClientMessage::Pong(bytes) if bytes.as_ref() == b"during-iam" => pong = true,
                ClientMessage::Ping(bytes) => {
                    socket.send(ClientMessage::Pong(bytes)).await.unwrap()
                }
                ClientMessage::Text(raw) => {
                    let value: Value = serde_json::from_str(&raw).unwrap();
                    assert_eq!(
                        value["type"], "test.live",
                        "control escaped its IAM wait: {value}"
                    );
                    event = true;
                }
                other => panic!("unexpected frame: {other:?}"),
            }
        }
    })
    .await
    .expect("IAM blocked pings or event polling");
    assert_eq!(
        app.media.lock().unwrap().streams[&stream_id].next_seq,
        1,
        "IAM blocked microphone input"
    );
    {
        let mut engine = app.engine.lock().unwrap();
        app.media.lock().unwrap().tick(&mut engine);
    }
    let audio = tokio::time::timeout(Duration::from_secs(2), next_json(&mut socket))
        .await
        .expect("IAM blocked audio output");
    assert_eq!(audio["type"], "media.audio");
    gate.release.add_permits(1);
    reply(&mut socket, "slow").await;
    let first = reply(&mut socket, "same-operation").await;
    assert_eq!(first, reply(&mut socket, "same-operation").await);
    // A snapshot completion must not rewind the cursor and deliver test.live a second time.
    assert!(
        tokio::time::timeout(Duration::from_millis(250), socket.next())
            .await
            .is_err(),
        "event cursor was rewound after IAM completed"
    );
    // Closing during an attach must abort it before cleanup, so it cannot create a live orphan stream.
    app.engine.lock().unwrap().iam_verified.clear();
    gate.block_next.store(true, Ordering::SeqCst);
    send(
        &mut socket,
        "late-attach",
        "media.attach",
        json!({"ringid":ring,"device_id":alice.device_id}),
    )
    .await;
    tokio::time::timeout(Duration::from_secs(3), gate.entered.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    socket.close(None).await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while app.media.lock().unwrap().streams[&stream_id].connected {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("closed socket did not clean up media");
    gate.release.add_permits(1);
    tokio::time::sleep(Duration::from_millis(100)).await;
    let media = app.media.lock().unwrap();
    assert_eq!(media.streams.len(), 1);
    assert!(!media.streams[&stream_id].connected);
    drop(media);
    server_task.abort();
    iam_task.abort();
}

#[tokio::test]
async fn managed_cleanup_drains_notification_authorization_after_disconnect() {
    const CHILD: &str = "RING_NOTIFICATION_DRAIN_TEST_CHILD";
    const ENVIRONMENT: &str = "00000000-0000-4000-8000-000000000011";
    const CONTROL: &str = "ring-notification-drain-control-secret-00001";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "connection_tests::managed_cleanup_drains_notification_authorization_after_disconnect", "--nocapture"])
            .env(CHILD, "1")
            .env("RING_ENV", "development")
            .env("RING_DISABLE_PROVIDERS", "1")
            .env_remove("RING_ENCRYPTION_KEY")
            .output().unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let gate = IamGate {
        block_next: Arc::new(AtomicBool::new(false)),
        entered: Arc::new(Semaphore::new(0)),
        release: Arc::new(Semaphore::new(0)),
    };
    let iam_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let iam_url = format!("http://{}", iam_listener.local_addr().unwrap());
    let iam_router = Router::new()
        .route("/api/v1/application/testing-context", get(|| async {
            Json(json!({"environment_id":ENVIRONMENT,"application":{"app_id":"ring","base_url":"https://ring.example","app_scope":{"iam":[],"external":[]},"webhook_scope":[],"testing_idle_days":7}}))
        }))
        .route("/api/v1/oauth/introspect", axum::routing::post(|| async {
            Json(json!({"active":true,"public_id":"si:alice","actor_type":"silicon","client_id":"ring","expires_at":chrono::Utc::now().timestamp()+3600,"authorization":{"public_id":"si:alice","actor_type":"silicon","organization_id":ENVIRONMENT,"org_id":"org","membership_id":"si:alice[org]","membership_version":1,"authorization_epoch":1,"audience":"ring","testing_environment_id":ENVIRONMENT,"scopes":[],"org_role":"owner"}}))
        }))
        .route("/api/v1/me", get(|| async { Json(json!({"display_name":"Alice"})) }))
        .route("/api/v1/obo-access/authorizations", axum::routing::post(|AxumState(gate): AxumState<IamGate>| async move {
            gate.entered.add_permits(1);
            gate.release.acquire().await.unwrap().forget();
            Json(json!({"id":"00000000-0000-4000-8000-000000000022","app_id":"ring","app_name":"Ring","actor":{"type":"silicon","public_id":"si:alice"},"org_id":"org","status":"pending","version":1,"expires_at":after(600),"endpoints":[],"authorization_url":"https://iam.example/consent"}))
        }))
        .with_state(gate.clone());
    let iam_task =
        tokio::spawn(async move { axum::serve(iam_listener, iam_router).await.unwrap() });
    std::env::set_var("RING_IAM_URL", iam_url);
    std::env::set_var("RING_HONEYCOMB_CONTROL_TOKEN", CONTROL);
    let dir = tempfile::tempdir().unwrap();
    let manager = Arc::new(lifecycle::Manager::open(dir.path()).unwrap());
    let root = App {
        testing: None,
        environments: Some(manager.clone()),
        activity: Arc::default(),
        engine: Arc::new(Mutex::new(Engine::open(dir.path()).unwrap())),
        media: Arc::default(),
        test_tokens: Arc::default(),
        test_secret: None,
        providers_disabled: true,
        vault: Arc::new(vault::Vault::open(dir.path()).unwrap()),
        telemetry: Arc::new(None),
        web_analytics: Arc::new(None),
        web_events: Arc::new(None),
        cli_telemetry: Arc::new(None),
    };
    async fn lifecycle_request(root: App, action: &str, revision: u64) -> axum::response::Response {
        let operation = uuid::Uuid::new_v4().to_string();
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            "authorization",
            format!("Bearer {CONTROL}").parse().unwrap(),
        );
        lifecycle::handle(
            AxumState(root),
            axum::extract::Path(("owner".into(), ENVIRONMENT.into(), operation.clone())),
            headers,
            axum::body::Bytes::from(json!({"operation_id":operation,"environment_id":ENVIRONMENT,"org_id":"owner","app_id":"ring","environment_revision":revision,"generation":revision,"key_version":1,"action":action,"testing_key":"A".repeat(32)}).to_string()),
        ).await
    }
    assert_eq!(
        lifecycle_request(root.clone(), "prepare", 1).await.status(),
        StatusCode::OK
    );
    let service = manager.get(ENVIRONMENT).await.unwrap();
    let identity = Identity {
        actor: "si:alice".into(),
        org_id: "org".into(),
        realm: ENVIRONMENT.into(),
        display_name: "Alice".into(),
        admin: false,
    };
    let login = service
        .engine
        .lock()
        .unwrap()
        .login(identity.clone(), None)
        .unwrap();
    service.vault.set(&auth::credential_key(&identity), &json!({"access_token":"test-actor-token","expires_at":chrono::Utc::now().timestamp()+3600})).unwrap();
    let original_directory = service.engine.lock().unwrap().data_dir.clone();
    let activity = service.activity.clone();
    drop(service);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}/ws", listener.local_addr().unwrap());
    let router = Router::new().route("/ws", get(ws)).with_state(root.clone());
    let server_task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let (mut socket, _) = connect_async(url).await.unwrap();
    send(&mut socket, "hello", "protocol.hello", json!({"versions":[1],"org_id":"org","realm":ENVIRONMENT,"test_app_secret":"test-ring-secret"})).await;
    reply(&mut socket, "hello").await;
    send(
        &mut socket,
        "resume",
        "auth.resume",
        json!({"session_token":login["session_token"]}),
    )
    .await;
    reply(&mut socket, "resume").await;
    send(&mut socket, "consent", "notifications.authorize", json!({})).await;
    tokio::time::timeout(Duration::from_secs(3), gate.entered.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    socket.close(None).await.unwrap();
    let mut clean = tokio::spawn(lifecycle_request(root, "clean", 2));
    tokio::time::timeout(Duration::from_secs(3), activity.cancelled.cancelled())
        .await
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(100), &mut clean)
            .await
            .is_err(),
        "cleanup acknowledged while an external authorization was still in flight"
    );
    assert!(
        original_directory.exists(),
        "cleanup erased the generation before its request drained"
    );
    gate.release.add_permits(1);
    let response = tokio::time::timeout(Duration::from_secs(3), clean)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let receipt: Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), 65536)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(receipt["state"], "completed");
    assert!(!original_directory.exists());
    let fresh = manager.get(ENVIRONMENT).await.unwrap();
    assert_eq!(fresh.testing.as_ref().unwrap().generation, 2);
    assert!(fresh.engine.lock().unwrap().state.sessions.is_empty());
    assert!(fresh.engine.lock().unwrap().state.requests.is_empty());
    assert!(fresh
        .vault
        .get(&auth::credential_key(&identity))
        .unwrap()
        .is_none());
    assert!(fresh
        .vault
        .get(&format!(
            "ting-consent:{}",
            key(ENVIRONMENT, "org", "si:alice")
        ))
        .unwrap()
        .is_none());
    assert!(fresh.vault.get("testing:app-secret").unwrap().is_none());
    server_task.abort();
    iam_task.abort();
}
