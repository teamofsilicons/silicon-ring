mod auth;
mod engine;
mod greetings;
mod iam_webhook;
mod media;
mod model;
mod push;
mod settings;
mod storage;
mod vault;
mod workers;

use axum::{
    extract::{
        ws::{Message, WebSocket},
        State as AxumState, WebSocketUpgrade,
    },
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use engine::Engine;
use futures_util::{SinkExt, StreamExt};
use model::*;
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    io::{Seek, SeekFrom, Write},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::mpsc;

#[derive(Clone)]
pub struct App {
    pub engine: Arc<Mutex<Engine>>,
    pub media: Arc<Mutex<media::Media>>,
    pub test_tokens: Arc<BTreeMap<String, Identity>>,
    pub test_secret: Option<String>,
    pub providers_disabled: bool,
    pub vault: Arc<vault::Vault>,
    pub telemetry: Arc<Option<ring_providers::Telemetry>>,
}
struct Peer {
    hello: bool,
    realm: String,
    org: String,
    token: Option<String>,
    subscriptions: BTreeMap<String, Subscription>,
}
struct Subscription {
    ring: Option<String>,
    topics: Vec<String>,
    after_seq: u64,
}
fn provider_error(e: ring_providers::Error) -> Fault {
    Fault{code:e.code,message:e.message,retryable:e.retryable,next_action:"Check the configured provider and app authorization, then retry the original request ID.".into(),details:None}
}
#[tokio::main]
async fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "ring_server=info".into()),
        )
        .init();
    let dir = PathBuf::from(std::env::var("RING_DATA_DIR").unwrap_or_else(|_| "data".into()));
    let mut engine = Engine::open(&dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
        std::fs::set_permissions(
            dir.join("ring.sqlite3"),
            std::fs::Permissions::from_mode(0o600),
        )?;
    }
    // A process restart cannot recover a live provider socket or a partially buffered microphone.
    let recovered: Vec<_> = engine
        .state
        .calls
        .values_mut()
        .filter(|c| matches!(c.state.as_str(), "active" | "connecting" | "ringing"))
        .map(|c| {
            if c.recording_status == "recording" {
                c.recording_status = "failed".into();
            }
            engine::end_call(c);
            c.clone()
        })
        .collect();
    for c in recovered {
        engine.state.call_event(
            &c,
            "call.ended",
            json!({"ringid":c.ringid,"reason":"server_restart"}),
        );
    }
    engine.persist().map_err(|e| e.message)?;
    let test_secret = std::env::var("RING_TEST_APP_SECRET").ok();
    let mut test_tokens = BTreeMap::new();
    if let Ok(path) = std::env::var("RING_TEST_TOKENS_FILE") {
        if test_secret.as_ref().is_none_or(|s| s.len() < 24) {
            return Err(
                "Test tokens require RING_TEST_APP_SECRET with at least 24 characters".into(),
            );
        }
        let values: BTreeMap<String, Value> = serde_json::from_slice(&std::fs::read(path)?)?;
        for (token, v) in values {
            let actor = required(&v, "actor").map_err(|e| e.message)?.to_string();
            let org = required(&v, "org_id").map_err(|e| e.message)?.to_string();
            actor_id(&actor, &org).map_err(|e| e.message)?;
            test_tokens.insert(
                digest(&token),
                Identity {
                    display_name: v["display_name"].as_str().unwrap_or(&actor).into(),
                    actor,
                    org_id: org,
                    realm: "test".into(),
                    admin: v["admin"].as_bool().unwrap_or(false),
                },
            );
        }
    }
    let telemetry = ring_providers::Telemetry::new(
        std::env::var("RING_TELEMETRY_BACKEND_KEY").ok().as_deref(),
        std::env::var("RING_TELEMETRY_ENABLED").as_deref() != Ok("false"),
    )
    .ok();
    let app = App {
        engine: Arc::new(Mutex::new(engine)),
        media: Arc::new(Mutex::new(media::Media::default())),
        test_tokens: Arc::new(test_tokens),
        test_secret,
        providers_disabled: std::env::var("RING_DISABLE_PROVIDERS").as_deref() == Ok("1"),
        vault: Arc::new(vault::Vault::open(&dir)?),
        telemetry: Arc::new(telemetry),
    };
    workers::start(app.clone());
    storage::start(app.clone());
    auth::start_revalidation(app.clone());
    push::start(app.clone());
    let web = std::env::var("RING_WEB_DIR").unwrap_or_else(|_| "web/dist".into());
    let router=Router::new().route("/health",get(||async{Json(json!({"status":"ok","app":"ring","version":env!("CARGO_PKG_VERSION"),"protocol_major":1}))})).route("/ws",get(ws)).route("/webhook/",axum::routing::post(iam_webhook::handle)).layer(axum::extract::DefaultBodyLimit::max(1024*1024)).fallback_service(tower_http::services::ServeDir::new(web.clone()).not_found_service(tower_http::services::ServeFile::new(format!("{web}/index.html")))).with_state(app);
    let bind = std::env::var("RING_BIND").unwrap_or_else(|_| "127.0.0.1:8765".into());
    let listener = tokio::net::TcpListener::bind(&bind).await?;
    tracing::info!(%bind,"Ring server listening");
    axum::serve(listener, router)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
async fn ws(
    AxumState(app): AxumState<App>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> axum::response::Response {
    if let Some(origin) = headers.get("origin").and_then(|h| h.to_str().ok()) {
        let allowed=std::env::var("RING_ALLOWED_ORIGINS").unwrap_or_else(|_|"http://localhost:5173,http://127.0.0.1:5173,http://localhost:1420,http://127.0.0.1:1420,http://localhost:8765,http://127.0.0.1:8765,https://ring.teamofsilicons.com,tauri://localhost,http://tauri.localhost,https://tauri.localhost".into());
        if !allowed.split(',').any(|o| o == origin) {
            return (StatusCode::FORBIDDEN, "Origin is not allowed").into_response();
        }
    }
    upgrade
        .max_message_size(128 * 1024)
        .on_upgrade(move |socket| connection(app, socket))
}
async fn connection(app: App, socket: WebSocket) {
    let (mut sink, mut source) = socket.split();
    let (tx, mut rx) = mpsc::channel::<Value>(256);
    let mut peer = Peer {
        hello: false,
        realm: "production".into(),
        org: String::new(),
        token: None,
        subscriptions: BTreeMap::new(),
    };
    let mut poll = tokio::time::interval(Duration::from_millis(100));
    let mut heartbeat = tokio::time::interval(Duration::from_secs(20));
    let mut last_seen = Instant::now();
    let mut window = Instant::now();
    let mut controls = 0u32;
    loop {
        tokio::select! {
        incoming=source.next()=>{let Some(Ok(message))=incoming else{break};last_seen=Instant::now();match message{Message::Text(raw)=>{let value=match serde_json::from_str::<Value>(&raw){Ok(v)=>v,Err(_)=>{let _=sink.send(Message::Text(json!({"ok":false,"error":invalid("Malformed JSON frame")}).to_string().into())).await;continue}};if value.get("method").is_some(){if window.elapsed()>Duration::from_secs(1){window=Instant::now();controls=0}controls+=1;if controls>60{let _=sink.send(Message::Text(json!({"id":value["id"],"ok":false,"error":Fault::new("RATE_LIMITED","Too many control requests.","Wait a second before retrying.")}).to_string().into())).await;continue}let response=request(&app,&mut peer,value,tx.clone()).await;if sink.send(Message::Text(response.to_string().into())).await.is_err(){break}}else{let result=frame(&app,&peer,&value);if let Some(response)=result{if sink.send(Message::Text(response.to_string().into())).await.is_err(){break}}}},Message::Ping(data)=>{if sink.send(Message::Pong(data)).await.is_err(){break}},Message::Pong(_)=>{},Message::Close(_)=>break,_=>{}}},
        outgoing=rx.recv()=>{if let Some(v)=outgoing{let valid=peer.token.as_ref().is_some_and(|t|app.engine.lock().unwrap().session(t).is_ok());if valid&&sink.send(Message::Text(v.to_string().into())).await.is_err(){break}}},
        _=poll.tick()=>{let events={let e=app.engine.lock().unwrap();let session=peer.token.as_ref().and_then(|t|e.session(t).ok());let mut out=Vec::new();if let Some(s)=session{for(id,sub)in &mut peer.subscriptions{for event in e.state.events.iter().filter(|v|v.seq>sub.after_seq){if event.org_id==s.identity.org_id&&event.realm==s.identity.realm&&event.recipients.contains(&s.identity.actor)&&sub.ring.as_ref().is_none_or(|r|event.data["ringid"]==*r)&&(sub.topics.is_empty()||sub.topics.iter().any(|t|event.kind.starts_with(t))){let mut v=json!(event);v["subscription_id"]=json!(id);v.as_object_mut().unwrap().remove("recipients");v.as_object_mut().unwrap().remove("org_id");v.as_object_mut().unwrap().remove("realm");out.push(v);}}sub.after_seq=e.state.events.last().map_or(sub.after_seq,|x|x.seq);}}out};for event in events{if sink.send(Message::Text(event.to_string().into())).await.is_err(){return}}},
        _=heartbeat.tick()=>{if last_seen.elapsed()>Duration::from_secs(90){break}if sink.send(Message::Ping(vec![].into())).await.is_err(){break}},
        }
    }
    app.media.lock().unwrap().disconnect(&tx);
}
async fn request(app: &App, peer: &mut Peer, v: Value, out: mpsc::Sender<Value>) -> Value {
    let rid = v["id"].as_str().filter(|s| !s.is_empty() && s.len() <= 160);
    let Some(rid) = rid else {
        return json!({"id":v["id"],"ok":false,"error":invalid("A request requires a nonempty string id, at most 160 bytes")});
    };
    let method = v["method"].as_str().unwrap_or("");
    let p = v.get("params").cloned().unwrap_or(json!({}));
    let result = handle(app, peer, rid, method, p, out).await;
    match result {
        Ok(result) => json!({"id":rid,"ok":true,"result":result}),
        Err(error) => json!({"id":rid,"ok":false,"error":error}),
    }
}
async fn handle(
    app: &App,
    peer: &mut Peer,
    rid: &str,
    m: &str,
    p: Value,
    out: mpsc::Sender<Value>,
) -> Result<Value> {
    if !p.is_object() {
        return Err(invalid("params must be an object"));
    }
    if m == "protocol.hello" {
        if peer.hello {
            return Err(invalid(
                "Protocol hello can be sent only once per connection",
            ));
        }
        if !p["versions"]
            .as_array()
            .is_some_and(|v| v.contains(&json!(1)))
        {
            return Err(Fault::new(
                "PROTOCOL_UNSUPPORTED",
                "Ring requires protocol major 1.",
                "Update the client to a compatible protocol.",
            ));
        }
        let realm = p["realm"].as_str().unwrap_or("production");
        if !matches!(realm, "production" | "test") {
            return Err(invalid("realm must be production or test"));
        }
        if realm == "test" {
            let secret = p["test_app_secret"].as_str().unwrap_or("");
            if app
                .test_secret
                .as_ref()
                .is_none_or(|expected| digest(secret) != digest(expected))
            {
                return Err(Fault::new(
                    "TEST_APP_AUTH_FAILED",
                    "The test application secret is missing or invalid.",
                    "Use the deployment's isolated test secret.",
                ));
            }
        }
        peer.realm = realm.into();
        peer.org = p["org_id"]
            .as_str()
            .map(String::from)
            .or_else(|| std::env::var("RING_DEFAULT_ORG").ok())
            .unwrap_or_default();
        if peer.org.contains('|') {
            return Err(invalid("Invalid organization ID"));
        }
        peer.hello = true;
        return Ok(
            json!({"version":1,"protocol_major":1,"capabilities":["calls","groups","voicemail","recordings","handoff","representatives","context-approval"],"limits":{"context":400,"start":100,"message":160,"audio_chunk_bytes":960,"asset_chunk_bytes":65536,"upload_bytes":20971520},"heartbeat_seconds":20,"realm":realm}),
        );
    }
    if !peer.hello {
        return Err(Fault::new(
            "HELLO_REQUIRED",
            "Call protocol.hello before other operations.",
            "Negotiate protocol major 1 first.",
        ));
    }
    if m == "app.info" {
        return Ok(
            json!({"app_id":"ring","org_id":std::env::var("RING_OWNER_ORG").unwrap_or_else(|_|"teamofsilicons".into()),"selected_org":peer.org,"version":env!("CARGO_PKG_VERSION"),"protocol_major":1,"endpoint":"wss://backend.ring.teamofsilicons.com/ws","repository":"https://github.com/teamofsilicons/silicon-ring","docs":"https://ring.teamofsilicons.com/docs","rust_package":"ring-client","install_url":"https://ring.teamofsilicons.com/install.sh","compatibility":{"supported_protocol_majors":[1],"sunset":null}}),
        );
    }
    if m == "auth.login" {
        if peer.token.is_some() {
            return Err(invalid(
                "Logout or open a new connection before logging in as another identity",
            ));
        }
        let token = required(&p, "token")?;
        let i = if peer.realm == "test" && !app.test_tokens.is_empty() {
            let i = app
                .test_tokens
                .get(&digest(token))
                .filter(|i| peer.org.is_empty() || i.org_id == peer.org)
                .cloned()
                .ok_or_else(|| {
                    Fault::new(
                        "IAM_TOKEN_INVALID",
                        "Test IAM token is invalid for this organization.",
                        "Use an authorized token from the configured test realm.",
                    )
                })?;
            i
        } else {
            if peer.org.is_empty() {
                return Err(invalid(
                    "org_id is required in protocol.hello for IAM login",
                ));
            }
            let iam =
                ring_providers::Iam::from_env(peer.realm == "test").map_err(provider_error)?;
            let session = iam
                .login(token, &peer.org, rid)
                .await
                .map_err(provider_error)?;
            let i = Identity {
                actor: session.identity.actor_id.clone(),
                org_id: session.identity.org_id.clone(),
                realm: peer.realm.clone(),
                display_name: session.identity.display_name.clone(),
                admin: session.identity.admin,
            };
            auth::save(app, &session)?;
            i
        };
        let login = app
            .engine
            .lock()
            .unwrap()
            .login(i, p["device_id"].as_str().map(String::from))?;
        peer.token = login["session_token"].as_str().map(String::from);
        return Ok(login);
    }
    if m == "auth.resume" {
        let token = required(&p, "session_token")?;
        let s = app.engine.lock().unwrap().session(token)?;
        auth::verify(app, &s.identity).await?;
        if s.identity.realm != peer.realm
            || !peer.org.is_empty() && s.identity.org_id != peer.org
            || p["device_id"].as_str().is_some_and(|d| d != s.device_id)
        {
            return Err(forbidden());
        }
        peer.token = Some(token.into());
        return Ok(
            json!({"authenticated":true,"actor":s.identity.actor,"actor_id":s.identity.actor,"display_name":s.identity.display_name,"org_id":s.identity.org_id,"realm":s.identity.realm,"device_id":s.device_id,"expires_at":s.expires_at}),
        );
    }
    if m == "auth.status" && peer.token.is_none() {
        return Ok(json!({"authenticated":false,"realm":peer.realm}));
    }
    let token = peer.token.as_deref().ok_or_else(|| {
        Fault::new(
            "AUTH_REQUIRED",
            "Authentication is required for this operation.",
            "Use auth.login with an IAM token.",
        )
    })?;
    let session = app.engine.lock().unwrap().session(token)?;
    auth::verify(app, &session.identity).await?;
    let session = app.engine.lock().unwrap().session(token)?;
    let i = &session.identity;
    if m == "notifications.authorize" {
        return auth::notifications(app, i, rid, &p).await;
    }
    if m == "events.subscribe" {
        let e = app.engine.lock().unwrap();
        if peer.subscriptions.len() >= 20 {
            return Err(invalid("At most 20 subscriptions per connection"));
        }
        let ring = p["ringid"].as_str().map(String::from);
        if let Some(r) = &ring {
            e.state.call(i, r)?;
        }
        let latest = e.state.events.last().map_or(0, |v| v.seq);
        let after = p["after_seq"].as_u64().unwrap_or(latest);
        if after > latest {
            return Err(invalid("after_seq is ahead of the server event cursor"));
        }
        let sid = id("sub");
        peer.subscriptions.insert(
            sid.clone(),
            Subscription {
                ring,
                topics: p["topics"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default(),
                after_seq: after,
            },
        );
        return Ok(json!({"subscription_id":sid,"cursor":latest,"latest_seq":latest}));
    }
    if m == "events.unsubscribe" {
        let sid = required(&p, "subscription_id")?;
        return Ok(json!({"unsubscribed":peer.subscriptions.remove(sid).is_some()}));
    }
    if m == "media.attach" {
        let mut e = app.engine.lock().unwrap();
        return app
            .media
            .lock()
            .unwrap()
            .attach(&mut e, &session, token, &p, out);
    }
    if m == "media.state" {
        return app.media.lock().unwrap().state(&session, &p);
    }
    if m == "media.detach" {
        let mut e = app.engine.lock().unwrap();
        return app.media.lock().unwrap().detach(&mut e, &session, &p);
    }
    if m == "calls.handoff"
        && !app.media.lock().unwrap().handoff_ready(
            i,
            required(&p, "ringid")?,
            required(&p, "to_device_id")?,
        )
    {
        return Err(Fault::new(
            "DEVICE_NOT_READY",
            "The target device has not prepared a standby media stream.",
            "Open Ring on that device and attach its microphone before switching.",
        ));
    }
    let fingerprint = digest(&format!("{m}:{p}"));
    let mut p = p;
    if m == "config.set" {
        settings::prepare_config(app, i, &mut p)?;
    }
    let response = {
        app.engine.lock().unwrap().request_with_fingerprint(
            &session,
            rid,
            m,
            p.clone(),
            fingerprint,
        )
    };
    if response["ok"] != true {
        return Err(serde_json::from_value(response["error"].clone())
            .unwrap_or_else(|_| invalid("Unknown server error")));
    }
    let mut result = response["result"].clone();
    if m == "voicemail.begin" {
        greetings::prepare(app, i, &mut result).await?;
    }
    if m == "assets.get" && result["complete"] == true {
        storage::ensure_local(app, i, required(&p, "asset_id")?).await?;
        let download = {
            let e = app.engine.lock().unwrap();
            let a = e.authorized_asset(i, required(&p, "asset_id")?)?;
            media::authorized_download(&e, i, a)?
        };
        result["size_bytes"] = json!(download.size_bytes);
        tokio::spawn(download.stream(
            out,
            result["transfer_id"].clone(),
            result["asset_id"].clone(),
        ));
    }
    if m == "auth.logout" {
        app.media.lock().unwrap().disconnect_token(token);
        peer.token = None;
    }
    if let Some(t) = app.telemetry.as_ref() {
        let e = app.engine.lock().unwrap();
        if e.state.config(i).values.get("telemetry.enabled") != Some(&Value::Bool(false))
            && e.state
                .configs
                .get(&key(&i.realm, &i.org_id, "*"))
                .is_none_or(|c| c.values.get("telemetry.enabled") != Some(&Value::Bool(false)))
        {
            t.record("backend", m, "completed", rid, None);
        }
    }
    Ok(result)
}
fn frame(app: &App, peer: &Peer, value: &Value) -> Option<Value> {
    let result = (|| -> Result<Option<Value>> {
        let token = peer.token.as_ref().ok_or_else(forbidden)?;
        let mut e = app.engine.lock().unwrap();
        let s = e.session(token)?;
        let p = &value["data"];
        match value["type"].as_str() {
            Some("media.audio") => {
                app.media.lock().unwrap().audio(&mut e, &s, p)?;
                Ok(None)
            }
            Some("media.speech") => {
                app.media.lock().unwrap().speech(&e, &s, p)?;
                Ok(None)
            }
            Some("assets.chunk") => {
                let aid = required(p, "asset_id")?;
                let a = e.owned_asset(&s.identity, aid)?;
                if a.complete {
                    return Err(invalid("Asset is already complete"));
                }
                let seq = p["seq"]
                    .as_u64()
                    .ok_or_else(|| invalid("Asset seq must be unsigned"))?;
                if seq != a.next_seq {
                    return Err(Fault::new(
                        "UPLOAD_SEQUENCE",
                        "Chunk sequence does not match the next expected sequence.",
                        "Use assets.get to resume at next_seq.",
                    ));
                }
                let data = STANDARD
                    .decode(required(p, "data_base64")?)
                    .map_err(|_| invalid("Malformed base64 upload"))?;
                if data.is_empty()
                    || data.len() > 65536
                    || a.received_bytes + data.len() > a.size_bytes
                {
                    return Err(invalid(
                        "Chunk exceeds upload size or 65536-byte frame limit",
                    ));
                }
                let previous = a.clone();
                std::fs::OpenOptions::new()
                    .create(true)
                    .write(true)
                    .open(&a.path)
                    .and_then(|mut f| {
                        f.seek(SeekFrom::Start(a.received_bytes as u64))?;
                        f.write_all(&data)?;
                        f.sync_data()
                    })
                    .map_err(|_| {
                        Fault::new(
                            "STORAGE_FAILED",
                            "Could not persist upload chunk.",
                            "Check disk space and retry.",
                        )
                    })?;
                let a = e.state.assets.get_mut(aid).unwrap();
                a.next_seq += 1;
                a.received_bytes += data.len();
                let response = json!({"type":"assets.ack","data":{"asset_id":aid,"next_seq":a.next_seq,"received_bytes":a.received_bytes}});
                if let Err(error) = e.persist() {
                    e.state.assets.insert(aid.to_owned(), previous);
                    return Err(error);
                }
                Ok(Some(response))
            }
            _ => Err(invalid("Unknown stream frame type")),
        }
    })();
    match result {
        Ok(v) => v,
        Err(error) => Some(
            json!({"type":"stream.error","data":{"stream_id":value["data"]["stream_id"],"asset_id":value["data"]["asset_id"],"error":error}}),
        ),
    }
}
