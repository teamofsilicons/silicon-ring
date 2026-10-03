use crate::platform::{self, OpenOptionsExt};
use crate::{
    audio::Audio,
    store::{io_error, Store},
};
use base64::Engine;
use ring_client::{Client, Result, RingError};
use serde_json::{json, Value};
use std::{
    fs,
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};
#[cfg(windows)]
use tokio::net::{TcpListener as IpcListener, TcpStream as IpcStream};
#[cfg(unix)]
use tokio::net::{UnixListener as IpcListener, UnixStream as IpcStream};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    sync::{broadcast, Mutex},
};

struct Media {
    audio: Audio,
    ringid: Option<String>,
    stream_id: Option<String>,
    seq: u64,
    muted: bool,
    standby: bool,
}
struct Runtime {
    store: Store,
    client: Mutex<Option<Client>>,
    media: Mutex<Option<Media>>,
    events: broadcast::Sender<Value>,
    shutdown: tokio::sync::Notify,
    generation: AtomicU64,
    telemetry_enabled: AtomicBool,
    restart: AtomicBool,
    #[cfg(windows)]
    ipc_secret: String,
}

pub async fn ipc(store: &Store, id: &str, method: &str, params: Value) -> Result<Value> {
    let stream = connect_endpoint(store).await?;
    let (read, mut write) = stream.into_split();
    write
        .write_all(
            format!(
                "{}\n",
                json!({"id":id,"method":method,"params":params,"isi":std::env::var("ISI").ok()})
            )
            .as_bytes(),
        )
        .await
        .map_err(|e| io_error(e, "daemon IPC"))?;
    let mut line = String::new();
    tokio::time::timeout(
        Duration::from_secs(150),
        BufReader::new(read).read_line(&mut line),
    )
    .await
    .map_err(|_| RingError::connection("Daemon response timed out", "daemon IPC"))?
    .map_err(|e| io_error(e, "daemon IPC"))?;
    let value: Value = serde_json::from_str(&line).map_err(|_| {
        RingError::connection("Daemon closed before confirming result", "daemon IPC")
    })?;
    if value["ok"] == true {
        Ok(value["result"].clone())
    } else {
        Err(serde_json::from_value(value["error"].clone())
            .map_err(|e| io_error(e, "daemon IPC"))?)
    }
}
pub async fn ensure(store: &Store) -> Result<()> {
    if ipc(store, "daemon-probe", "local.status", json!({}))
        .await
        .is_ok()
    {
        return Ok(());
    }
    let exe = std::env::current_exe().map_err(|e| io_error(e, "daemon start"))?;
    let log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(store.path("daemon.stderr.log"))
        .map_err(|e| io_error(e, "daemon start"))?;
    let mut cmd = Command::new(exe);
    if let Some(org) = &store.org {
        cmd.args(["--org", org]);
    }
    if store.test {
        cmd.arg("--test");
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() == -1 {
                    Err(std::io::Error::last_os_error())
                } else {
                    Ok(())
                }
            });
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000200);
    }
    cmd.args(["daemon", "run"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(log))
        .spawn()
        .map_err(|e| io_error(e, "daemon start"))?;
    for _ in 0..200 {
        tokio::time::sleep(Duration::from_millis(50)).await;
        if ipc(store, "daemon-probe", "local.status", json!({}))
            .await
            .is_ok()
        {
            return Ok(());
        }
    }
    Err(RingError::new(
        "DAEMON_UNAVAILABLE",
        "Daemon did not become ready within ten seconds",
        "daemon start",
        "Run ring daemon logs and inspect the private daemon.stderr.log file.",
    ))
}
pub async fn watch(store: &Store, id: &str, params: Value, json_output: bool) -> Result<()> {
    let mut stream = connect_endpoint(store).await?;
    stream
        .write_all(
            format!(
                "{}\n",
                json!({"id":id,"method":"local.watch","params":params})
            )
            .as_bytes(),
        )
        .await
        .map_err(|e| io_error(e, "watch"))?;
    let mut lines = BufReader::new(stream).lines();
    loop {
        tokio::select! {
            _=tokio::signal::ctrl_c()=>return Ok(()),
            line=lines.next_line()=>match line.map_err(|e|io_error(e,"watch"))? {Some(line)=>{let parsed:Value=serde_json::from_str(&line).map_err(|e|io_error(e,"watch"))?;if parsed["ok"]==false{return Err(serde_json::from_value(parsed["error"].clone()).map_err(|e|io_error(e,"watch"))?);}if json_output{println!("{line}");}else{let v:Value=serde_json::from_str(&line).map_err(|e|io_error(e,"watch"))?;println!("{}",serde_json::to_string_pretty(&v).unwrap_or(line));}},None=>return Err(RingError::connection("Daemon event stream closed","watch"))}
        }
    }
}
pub async fn run(store: Store) -> Result<()> {
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(store.path("daemon.lock"))
        .map_err(|e| io_error(e, "daemon lock"))?;
    if lock.try_lock().is_err() {
        return Ok(());
    }
    if connect_endpoint(&store).await.is_ok() {
        return Ok(());
    }
    #[cfg(unix)]
    if store.socket().exists() {
        fs::remove_file(store.socket()).map_err(|e| io_error(e, "daemon bind"))?;
    }
    #[cfg(unix)]
    let listener = IpcListener::bind(store.socket()).map_err(|e| io_error(e, "daemon bind"))?;
    #[cfg(unix)]
    platform::private_file(&store.socket())?;
    #[cfg(windows)]
    let listener = IpcListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| io_error(e, "daemon bind"))?;
    #[cfg(windows)]
    let ipc_secret = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    let metadata = json!({"pid":std::process::id(),"started_at":chrono::Utc::now()});
    #[cfg(windows)]
    let metadata = {
        let mut v = metadata;
        v["port"] = json!(listener
            .local_addr()
            .map_err(|e| io_error(e, "daemon address"))?
            .port());
        v["ipc_secret"] = json!(ipc_secret);
        v
    };
    store.write("daemon.json", &metadata)?;
    let (events, _) = broadcast::channel(2048);
    let telemetry_enabled = store.local()?["telemetry.enabled"] != false;
    let runtime = Arc::new(Runtime {
        store: store.clone(),
        client: Mutex::new(None),
        media: Mutex::new(None),
        events,
        shutdown: tokio::sync::Notify::new(),
        generation: AtomicU64::new(0),
        telemetry_enabled: AtomicBool::new(telemetry_enabled),
        restart: AtomicBool::new(false),
        #[cfg(windows)]
        ipc_secret,
    });
    log(&store, "daemon.started", None);
    let rt = runtime.clone();
    tokio::spawn(async move {
        background(rt).await;
    });
    loop {
        tokio::select! {
            _=runtime.shutdown.notified()=>break,
            _=async {if tokio::signal::ctrl_c().await.is_err(){std::future::pending::<()>().await;}}=>break,
            accept=listener.accept()=>{let (stream,_)=accept.map_err(|e|io_error(e,"daemon accept"))?;let rt=runtime.clone();tokio::spawn(async move{let _=serve(rt,stream).await;});}
        }
    }
    if let Some(media) = runtime.media.lock().await.take() {
        if let (Some(stream), Some(client)) = (media.stream_id, runtime.client.lock().await.clone())
        {
            let _ = client
                .request("media.detach", json!({"stream_id":stream}))
                .await;
        }
    }
    let _ = fs::remove_file(store.socket());
    let _ = fs::remove_file(store.path("daemon.json"));
    log(&store, "daemon.stopped", None);
    drop(listener);
    drop(lock);
    if runtime.restart.load(Ordering::SeqCst) {
        ensure(&store).await?;
    }
    Ok(())
}
async fn client(rt: &Arc<Runtime>, resume: bool) -> Result<Client> {
    let mut slot = rt.client.lock().await;
    if !resume {
        *slot = None;
    }
    if let Some(client) = slot.as_ref() {
        return Ok(client.clone());
    }
    let generation = rt.generation.fetch_add(1, Ordering::SeqCst) + 1;
    let c = Client::connect(&rt.store.options()?).await?;
    let session = rt.store.read("session.json")?;
    if let Some(token) = session["session_token"].as_str().filter(|_| resume) {
        c.request(
            "auth.resume",
            json!({"session_token":token,"device_id":session["device_id"]}),
        )
        .await?;
        c.request("events.subscribe", json!({})).await?;
        if let Ok(config) = c.request("config.get", json!({"scope":"actor"})).await {
            rt.telemetry_enabled.store(
                config["effective"]["telemetry.enabled"] != false,
                Ordering::Relaxed,
            );
        }
    }
    let mut events = c.events();
    let copy = rt.clone();
    tokio::spawn(async move {
        while let Ok(event) = events.recv().await {
            if event["type"] == "connection.closed" {
                if copy.generation.load(Ordering::SeqCst) != generation {
                    break;
                }
                *copy.client.lock().await = None;
                if let Some(media) = copy.media.lock().await.as_mut() {
                    media.stream_id = None;
                    media.seq = 0;
                }
                let _ = copy.events.send(event);
                break;
            }
            if event["type"] == "media.audio" {
                let media = copy.media.lock().await;
                if let Some(media) = media.as_ref() {
                    if media.stream_id.as_deref() == event["data"]["stream_id"].as_str() {
                        if let Some(data) = event["data"]["audio_base64"].as_str() {
                            if let Ok(bytes) =
                                base64::engine::general_purpose::STANDARD.decode(data)
                            {
                                let mut output =
                                    media.audio.output.lock().unwrap_or_else(|e| e.into_inner());
                                output.extend(
                                    bytes
                                        .chunks_exact(2)
                                        .map(|p| i16::from_le_bytes([p[0], p[1]])),
                                );
                            }
                        }
                    }
                }
            }
            let _ = copy.events.send(event);
        }
    });
    *slot = Some(c.clone());
    Ok(c)
}
async fn serve(rt: Arc<Runtime>, stream: IpcStream) -> Result<()> {
    #[cfg(windows)]
    let stream = {
        let mut stream = stream;
        server_handshake(&mut stream, &rt.ipc_secret).await?;
        stream
    };
    let (read, mut write) = stream.into_split();
    let mut reader = BufReader::new(read);
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .await
        .map_err(|e| io_error(e, "daemon read"))?;
    if line.len() > 100 * 1024 * 1024 {
        return Err(crate::store::invalid(
            "IPC request exceeds 100 MiB",
            "daemon read",
        ));
    }
    let req: Value = serde_json::from_str(&line).map_err(|e| io_error(e, "daemon decode"))?;
    let id = req["id"].as_str().unwrap_or("missing");
    let method = req["method"].as_str().unwrap_or("");
    let params = req["params"].clone();
    if method == "local.watch" {
        let mut params = params;
        let mut last_seq = params["after_seq"].as_u64();
        let mut disconnect_line = String::new();
        let mut attempt = 0u64;
        loop {
            let connected = client(&rt, true).await;
            let c = match connected {
                Ok(c) => c,
                Err(e) if e.retryable => {
                    write
                        .write_all(
                            format!(
                                "{}\n",
                                json!({"type":"connection.retrying","data":{"error":e}})
                            )
                            .as_bytes(),
                        )
                        .await
                        .map_err(|e| io_error(e, "watch"))?;
                    tokio::select! {_=tokio::time::sleep(Duration::from_secs(2))=>continue,_=reader.read_line(&mut disconnect_line)=>return Ok(())}
                }
                Err(e) => {
                    write
                        .write_all(format!("{}\n", json!({"ok":false,"error":e})).as_bytes())
                        .await
                        .map_err(|e| io_error(e, "watch"))?;
                    return Ok(());
                }
            };
            if let Some(seq) = last_seq {
                params["after_seq"] = json!(seq);
            }
            let mut events = rt.events.subscribe();
            let result = match c
                .request_with_id(
                    &format!("{id}:subscription:{attempt}"),
                    "events.subscribe",
                    params.clone(),
                    None,
                )
                .await
            {
                Ok(v) => v,
                Err(e) => {
                    write
                        .write_all(format!("{}\n", json!({"ok":false,"error":e})).as_bytes())
                        .await
                        .map_err(|e| io_error(e, "watch"))?;
                    return Ok(());
                }
            };
            if last_seq.is_none() {
                last_seq = result["cursor"].as_u64();
            }
            write
                .write_all(format!("{result}\n").as_bytes())
                .await
                .map_err(|e| io_error(e, "watch"))?;
            let disconnected = loop {
                tokio::select! {
                    v=events.recv()=>match v {
                        Ok(v)=>{
                            if v["type"]=="connection.closed"{break false;}
                            if v.get("subscription_id").is_none()||v["subscription_id"]!=result["subscription_id"]{continue;}
                            if write.write_all(format!("{v}\n").as_bytes()).await.is_err(){break true;}
                            if let Some(seq)=v["seq"].as_u64(){last_seq=Some(seq);}
                        },
                        Err(broadcast::error::RecvError::Lagged(_))=>break false,
                        Err(_)=>break true,
                    },
                    _=reader.read_line(&mut disconnect_line)=>break true,
                }
            };
            if disconnected {
                let _ = c
                    .request(
                        "events.unsubscribe",
                        json!({"subscription_id":result["subscription_id"]}),
                    )
                    .await;
                return Ok(());
            }
            attempt += 1;
        }
    }
    let started = std::time::Instant::now();
    let result = dispatch(&rt, id, method, params, req["isi"].as_str()).await;
    if id != "daemon-probe"
        && rt.telemetry_enabled.load(Ordering::Relaxed)
        && rt
            .store
            .local()
            .is_ok_and(|c| c["telemetry.enabled"] != false)
    {
        if let Some(connection) = rt.client.try_lock().ok().and_then(|client| client.clone()) {
            // The authenticated server chooses the realm's private CLI table and enforces opt-outs.
            // Only fixed operation labels and a fresh trace ID leave the daemon, never CLI arguments.
            let event = json!({
                "source": "cli", "step": "command",
                "progress": if result.is_ok() { "succeeded" } else { "failed" },
                "trace_id": uuid::Uuid::new_v4().to_string(),
                "duration_ms": started.elapsed().as_millis() as u64
            });
            tokio::spawn(async move {
                let _ = tokio::time::timeout(
                    Duration::from_secs(2),
                    connection.request("telemetry.record", event),
                )
                .await;
            });
        }
    }

    if let Err(error) = &result {
        log(
            &rt.store,
            "operation.failed",
            Some(json!({"method":method,"code":error.code,"request_id":id})),
        );
    }
    let value = match result {
        Ok(v) => json!({"ok":true,"result":v}),
        Err(e) => json!({"ok":false,"error":e}),
    };
    write
        .write_all(format!("{value}\n").as_bytes())
        .await
        .map_err(|e| io_error(e, "daemon reply"))?;
    if method == "local.stop" {
        rt.shutdown.notify_one();
    }
    Ok(())
}
async fn dispatch(
    rt: &Arc<Runtime>,
    id: &str,
    method: &str,
    params: Value,
    isi: Option<&str>,
) -> Result<Value> {
    match method {
        "local.status" => {
            let media = rt.media.lock().await;
            return Ok(
                json!({"running":true,"pid":std::process::id(),"connected":rt.client.lock().await.is_some(),"active_media":media.as_ref().map(|m|json!({"ringid":m.ringid,"stream_id":m.stream_id,"muted":m.muted}))}),
            );
        }
        "local.stop" => return Ok(json!({"stopping":true})),
        "local.audio.prepare" => {
            if rt.media.lock().await.is_some() {
                return Err(RingError::new(
                    "AUDIO_BUSY",
                    "This device already owns or is preparing media",
                    "media prepare",
                    "Finish or hand off current media before starting another carbon call.",
                ));
            }
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            let audio = Audio::start(&rt.store.local()?, tx)?;
            *rt.media.lock().await = Some(Media {
                audio,
                ringid: None,
                stream_id: None,
                seq: 0,
                muted: false,
                standby: false,
            });
            let copy = rt.clone();
            tokio::spawn(async move {
                while let Some(samples) = rx.recv().await {
                    let (stream, seq, muted) = {
                        let mut media = copy.media.lock().await;
                        match media.as_mut() {
                            Some(m) => {
                                let seq = m.seq;
                                m.seq += 1;
                                (m.stream_id.clone(), seq, m.muted)
                            }
                            None => break,
                        }
                    };
                    if let (Some(stream), Some(c)) = (stream, copy.client.lock().await.clone()) {
                        let bytes = samples
                            .iter()
                            .flat_map(|s| {
                                if muted {
                                    0i16.to_le_bytes()
                                } else {
                                    s.to_le_bytes()
                                }
                            })
                            .collect::<Vec<_>>();
                        let _=c.frame("media.audio",json!({"stream_id":stream,"seq":seq,"offset_ms":seq*20,"audio_base64":base64::engine::general_purpose::STANDARD.encode(bytes)})).await;
                        // ponytail: conservative energy VAD; replace with an on-device speech model when needed.
                        let energy = samples
                            .iter()
                            .map(|s| (*s as f64 / 32768.).powi(2))
                            .sum::<f64>()
                            / samples.len() as f64;
                        if !muted && energy.sqrt() > 0.025 {
                            let _=c.frame("media.speech",json!({"stream_id":stream,"seq":seq,"start_ms":seq*20,"end_ms":(seq+1)*20})).await;
                        }
                    }
                }
            });
            return Ok(json!({"ready":true}));
        }
        "local.audio.cancel" => {
            let old = rt.media.lock().await.take();
            if let Some(media) = old {
                if let Some(stream) = media.stream_id {
                    client(rt, true)
                        .await?
                        .request("media.detach", json!({"stream_id":stream}))
                        .await?;
                }
            }
            return Ok(json!({"released":true}));
        }
        "local.audio.attach" => {
            let session = rt.store.read("session.json")?;
            let c = client(rt, true).await?;
            let result=c.request_with_id(id,"media.attach",json!({"ringid":params["ringid"],"device_id":session["device_id"],"purpose":"call"}),isi).await?;
            if let Some(media) = rt.media.lock().await.as_mut() {
                media.ringid = params["ringid"].as_str().map(str::to_owned);
                media.stream_id = result["stream_id"].as_str().map(str::to_owned);
                media.seq = 0;
                media.standby = result["state"] == "standby";
            }
            return Ok(result);
        }
        "local.audio.bind" => {
            let mut media = rt.media.lock().await;
            let m = media.as_mut().ok_or_else(|| {
                RingError::new(
                    "AUDIO_NOT_READY",
                    "No prepared audio",
                    "media bind",
                    "Prepare native audio before calling.",
                )
            })?;
            m.ringid = params["ringid"].as_str().map(str::to_owned);
            return Ok(json!({"bound":true}));
        }
        "local.audio.mute" => {
            let (stream, muted) = {
                let mut media = rt.media.lock().await;
                let m = media
                    .as_mut()
                    .filter(|m| m.ringid.as_deref() == params["ringid"].as_str())
                    .ok_or_else(|| {
                        RingError::new(
                            "AUDIO_NOT_READY",
                            "This daemon does not own that call's microphone",
                            "media mute",
                            "Run on the device that owns active call audio.",
                        )
                    })?;
                let stream = m.stream_id.clone().ok_or_else(|| {
                    RingError::new(
                        "AUDIO_NOT_READY",
                        "Media has not connected",
                        "media mute",
                        "Wait for call connection.",
                    )
                })?;
                m.muted = params["muted"] == true;
                (stream, m.muted)
            };
            return client(rt, true)
                .await?
                .request_with_id(
                    id,
                    "media.state",
                    json!({"stream_id":stream,"muted":muted}),
                    isi,
                )
                .await;
        }
        _ => {}
    }
    let c = client(rt, method != "auth.login").await?;
    if method == "local.voicemail.audio" {
        let session = rt.store.read("session.json")?;
        let stream=c.request_with_id(&format!("{id}:attach"),"media.attach",json!({"ringid":params["ringid"],"device_id":session["device_id"],"purpose":"voicemail","voicemail_id":params["voicemail_id"]}),isi).await?;
        let stream = stream["stream_id"].as_str().ok_or_else(|| {
            crate::store::invalid("No private stream ID returned", "voicemail media")
        })?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(params["data_base64"].as_str().unwrap_or(""))
            .map_err(|_| crate::store::invalid("Invalid PCM encoding", "voicemail media"))?;
        let chunks = bytes.len().div_ceil(960);
        if chunks == 0 {
            return Err(crate::store::invalid(
                "Voicemail recording is empty",
                "voicemail media",
            ));
        }
        for (seq, chunk) in bytes.chunks(960).enumerate() {
            let mut frame = chunk.to_vec();
            frame.resize(960, 0);
            c.frame("media.audio",json!({"stream_id":stream,"seq":seq,"offset_ms":seq*20,"audio_base64":base64::engine::general_purpose::STANDARD.encode(frame)})).await?;
        }
        let detached = c
            .request_with_id(
                &format!("{id}:detach"),
                "media.detach",
                json!({"stream_id":stream,"last_seq":chunks-1}),
                isi,
            )
            .await?;
        if detached["complete"] == false || detached["persisted"] == false {
            return Err(RingError::new(
                "RECORDING_INCOMPLETE",
                "Some voicemail chunks were not persisted",
                "media.detach",
                "Abort this draft and record again.",
            ));
        }
        return Ok(detached);
    }
    if method == "local.upload" {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(params["data_base64"].as_str().unwrap_or(""))
            .map_err(|_| crate::store::invalid("Invalid upload encoding", "upload"))?;
        return c
            .upload(
                id,
                params["purpose"].as_str().unwrap_or(""),
                params["mime_type"]
                    .as_str()
                    .unwrap_or("application/octet-stream"),
                &bytes,
            )
            .await;
    }
    if method == "local.download" {
        let bytes = c
            .download(params["asset_id"].as_str().unwrap_or(""))
            .await?;
        return Ok(json!({"data_base64":base64::engine::general_purpose::STANDARD.encode(bytes)}));
    }
    let result = c.request_with_id(id, method, params, isi).await?;
    if matches!(method, "config.set" | "config.reset" | "config.get") {
        rt.telemetry_enabled.store(
            result["effective"]["telemetry.enabled"] != false,
            Ordering::Relaxed,
        );
    }

    if method == "auth.login" {
        if result["session_token"].as_str().is_none() {
            return Err(RingError::new(
                "PROTOCOL_ERROR",
                "Login returned no session token",
                "auth.login",
                "Check server compatibility.",
            ));
        }
        rt.store.write("session.json", &result)?;
        let _ = c.request("events.subscribe", json!({})).await;
        if let Ok(config) = c.request("config.get", json!({"scope":"actor"})).await {
            rt.telemetry_enabled.store(
                config["effective"]["telemetry.enabled"] != false,
                Ordering::Relaxed,
            );
        }
    }
    if method == "auth.logout" {
        let _ = fs::remove_file(rt.store.path("session.json"));
        *rt.client.lock().await = None;
        rt.media.lock().await.take();
    }
    Ok(result)
}
async fn background(rt: Arc<Runtime>) {
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    let mut update_at = std::time::Instant::now();
    let mut retry_at = std::time::Instant::now();
    loop {
        interval.tick().await;
        if rt
            .store
            .read("session.json")
            .ok()
            .and_then(|s| s["session_token"].as_str().map(str::to_owned))
            .is_none()
        {
            continue;
        }
        let c = match client(&rt, true).await {
            Ok(c) => c,
            Err(e) => {
                if std::time::Instant::now() >= retry_at {
                    log(&rt.store, "connection.failed", Some(json!({"code":e.code})));
                    retry_at = std::time::Instant::now() + Duration::from_secs(30);
                }
                continue;
            }
        };
        let ringid = rt
            .media
            .lock()
            .await
            .as_ref()
            .and_then(|m| m.ringid.clone());
        if let Some(ringid) = ringid {
            match c.request("calls.get", json!({"ringid":ringid})).await {
                Ok(call) => {
                    let state = call["state"].as_str().unwrap_or("");
                    let session = rt.store.read("session.json").unwrap_or(json!({}));
                    let owns = call["participants"].as_array().is_some_and(|members| {
                        members.iter().any(|m| {
                            m["actor"] == session["actor"]
                                && m["device_id"] == session["device_id"]
                                && m["left_at"].is_null()
                        })
                    });
                    let moved = state == "active"
                        && !owns
                        && rt
                            .media
                            .lock()
                            .await
                            .as_ref()
                            .is_some_and(|m| !m.standby && m.stream_id.is_some());
                    if owns {
                        if let Some(media) = rt.media.lock().await.as_mut() {
                            media.standby = false;
                        }
                    }
                    if state == "ended" || moved || state == "voicemail" {
                        if let Some(media) = rt.media.lock().await.take() {
                            if let Some(stream) = media.stream_id {
                                let _ =
                                    c.request("media.detach", json!({"stream_id":stream})).await;
                            }
                        }
                    } else if matches!(state, "active" | "connecting")
                        && rt
                            .media
                            .lock()
                            .await
                            .as_ref()
                            .is_some_and(|m| m.stream_id.is_none())
                    {
                        let session = rt.store.read("session.json").unwrap_or(json!({}));
                        match c.request("media.attach",json!({"ringid":ringid,"device_id":session["device_id"],"purpose":"call"})).await {
                            Ok(result)=>{if let Some(m)=rt.media.lock().await.as_mut(){m.stream_id=result["stream_id"].as_str().map(str::to_owned);m.seq=0;m.standby=result["state"]=="standby";}},
                            Err(e)=>log(&rt.store,"media.attach.failed",Some(json!({"code":e.code}))),
                        }
                    }
                }
                Err(e) => log(
                    &rt.store,
                    "media.inspect.failed",
                    Some(json!({"code":e.code})),
                ),
            }
        }
        if std::time::Instant::now() >= update_at {
            update_at = std::time::Instant::now() + Duration::from_secs(3600);
            match c.request("release.info",json!({"current_version":env!("CARGO_PKG_VERSION"),"platform":std::env::consts::OS,"arch":std::env::consts::ARCH,"channel":rt.store.local().ok().and_then(|c|c["updates.channel"].as_str().map(str::to_owned)).unwrap_or("stable".into())})).await {
                Ok(release)=>{let active=rt.media.lock().await.is_some();match crate::update::apply(&release,active).await{Ok(result)=>{if result["updated"]==true{rt.restart.store(true,Ordering::SeqCst);rt.shutdown.notify_one();}log(&rt.store,"update.checked",Some(result));},Err(e)=>log(&rt.store,"update.failed",Some(json!({"code":e.code}))) }},
                Err(e)=>log(&rt.store,"update.check.failed",Some(json!({"code":e.code}))),
            }
        }
    }
}
pub fn log(store: &Store, event: &str, details: Option<Value>) {
    use std::io::Write;
    let mut entry =
        json!({"time":chrono::Utc::now(),"source":"ring-daemon","event":event,"details":details});
    crate::store::redact(&mut entry);
    if let Ok(mut file) = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(store.path("events.jsonl"))
    {
        let _ = writeln!(file, "{entry}");
    }
}

#[cfg(unix)]
async fn connect_endpoint(store: &Store) -> Result<IpcStream> {
    IpcStream::connect(store.socket()).await.map_err(|e| {
        RingError::connection(format!("Local daemon is unavailable: {e}"), "daemon IPC")
    })
}
#[cfg(windows)]
async fn connect_endpoint(store: &Store) -> Result<IpcStream> {
    let endpoint = store.read("daemon.json")?;
    let port = endpoint["port"]
        .as_u64()
        .filter(|p| *p > 0 && *p <= 65535)
        .ok_or_else(|| RingError::connection("No local daemon endpoint is saved", "daemon IPC"))?;
    let secret = endpoint["ipc_secret"]
        .as_str()
        .ok_or_else(|| RingError::connection("No local IPC credential is saved", "daemon IPC"))?;
    let mut stream = IpcStream::connect((std::net::Ipv4Addr::LOCALHOST, port as u16))
        .await
        .map_err(|e| {
            RingError::connection(format!("Local daemon unavailable: {e}"), "daemon IPC")
        })?;
    client_handshake(&mut stream, secret).await?;
    Ok(stream)
}
#[cfg(any(windows, test))]
async fn server_handshake(stream: &mut tokio::net::TcpStream, secret: &str) -> Result<()> {
    let request = handshake_read(stream).await?;
    let challenge = request["challenge"]
        .as_str()
        .filter(|s| s.len() <= 100)
        .ok_or_else(|| crate::store::invalid("Invalid IPC challenge", "daemon authentication"))?;
    let nonce = uuid::Uuid::new_v4().to_string();
    stream
        .write_all(
            format!(
                "{}\n",
                json!({"challenge":nonce,"proof":proof(secret,challenge)})
            )
            .as_bytes(),
        )
        .await
        .map_err(|e| io_error(e, "daemon authentication"))?;
    let response = handshake_read(stream).await?;
    verify_proof(secret, &nonce, response["proof"].as_str().unwrap_or(""))?;
    stream
        .write_all(b"{\"ready\":true}\n")
        .await
        .map_err(|e| io_error(e, "daemon authentication"))?;
    Ok(())
}
#[cfg(any(windows, test))]
async fn handshake_read(stream: &mut tokio::net::TcpStream) -> Result<Value> {
    use tokio::io::AsyncReadExt;
    let mut line = String::new();
    tokio::time::timeout(
        Duration::from_secs(5),
        BufReader::new(stream.take(8192)).read_line(&mut line),
    )
    .await
    .map_err(|_| RingError::connection("IPC authentication timed out", "daemon IPC"))?
    .map_err(|e| io_error(e, "daemon authentication"))?;
    serde_json::from_str(&line).map_err(|_| {
        crate::store::invalid("Invalid IPC authentication frame", "daemon authentication")
    })
}
#[cfg(any(windows, test))]
fn proof(secret: &str, challenge: &str) -> String {
    use hmac::{Hmac, Mac};
    let mut mac =
        Hmac::<sha2::Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key size");
    mac.update(challenge.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}
#[cfg(any(windows, test))]
fn verify_proof(secret: &str, challenge: &str, proof: &str) -> Result<()> {
    use hmac::{Hmac, Mac};
    let mut mac =
        Hmac::<sha2::Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key size");
    mac.update(challenge.as_bytes());
    let bytes = hex::decode(proof).map_err(|_| {
        RingError::new(
            "DAEMON_AUTH_FAILED",
            "Invalid daemon authentication proof",
            "daemon IPC",
            "Restart the owner-only local daemon.",
        )
    })?;
    mac.verify_slice(&bytes).map_err(|_| {
        RingError::new(
            "DAEMON_AUTH_FAILED",
            "Local IPC peer failed authentication",
            "daemon IPC",
            "Restart the daemon; no session credential was sent.",
        )
    })
}
#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn tcp_ipc_requires_mutual_authentication() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            super::server_handshake(&mut socket, "secret")
                .await
                .unwrap();
        });
        let mut socket = tokio::net::TcpStream::connect(address).await.unwrap();
        super::client_handshake(&mut socket, "secret")
            .await
            .unwrap();
        server.await.unwrap();
    }
    #[test]
    fn ipc_authentication_binds_secret_and_fresh_challenge() {
        let signature = super::proof("private-secret", "nonce");
        assert!(super::verify_proof("private-secret", "nonce", &signature).is_ok());
        assert!(super::verify_proof("different-secret", "nonce", &signature).is_err());
        assert!(super::verify_proof("private-secret", "replay", &signature).is_err());
    }
}

#[cfg(any(windows, test))]
async fn client_handshake(stream: &mut tokio::net::TcpStream, secret: &str) -> Result<()> {
    let nonce = uuid::Uuid::new_v4().to_string();
    stream
        .write_all(format!("{}\n", json!({"challenge":nonce})).as_bytes())
        .await
        .map_err(|e| io_error(e, "daemon authentication"))?;
    let response = handshake_read(stream).await?;
    verify_proof(secret, &nonce, response["proof"].as_str().unwrap_or(""))?;
    let challenge = response["challenge"].as_str().ok_or_else(|| {
        RingError::connection("Daemon authentication challenge missing", "daemon IPC")
    })?;
    stream
        .write_all(format!("{}\n", json!({"proof":proof(secret,challenge)})).as_bytes())
        .await
        .map_err(|e| io_error(e, "daemon authentication"))?;
    if handshake_read(stream).await?["ready"] != true {
        return Err(RingError::connection(
            "Daemon did not authenticate IPC",
            "daemon IPC",
        ));
    }
    Ok(())
}
