//! Stateless protocol transport shared by the CLI and daemon. Credentials are supplied by callers.
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{collections::HashMap, fmt, sync::Arc, time::Duration};
use tokio::sync::{broadcast, mpsc, oneshot, Mutex};
use tokio_tungstenite::{connect_async, tungstenite::Message};

pub const PROTOCOL_MAJOR: u32 = 1;
pub type Result<T> = std::result::Result<T, RingError>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RingError {
    pub code: String,
    pub message: String,
    pub step: String,
    pub request_id: Option<String>,
    pub retryable: bool,
    pub next_action: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}
impl RingError {
    pub fn new(code: &str, message: impl Into<String>, step: &str, next: &str) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            step: step.into(),
            request_id: None,
            retryable: false,
            next_action: next.into(),
            details: None,
        }
    }
    pub fn connection(message: impl Into<String>, step: &str) -> Self {
        let mut e = Self::new(
            "CONNECTION_UNCERTAIN",
            message,
            step,
            "Inspect the resource, or retry the exact request with the same --request-id.",
        );
        e.retryable = true;
        e
    }
    pub fn exit_code(&self) -> i32 {
        match self.code.as_str() {
            "INVALID_INPUT"
            | "INVALID_PARAMS"
            | "TEXT_TOO_LONG"
            | "SILICON_HOME_REQUIRED"
            | "INVALID_ACTOR" => 2,
            s if s.contains("AUTH")
                || s.contains("FORBIDDEN")
                || s.contains("PERMISSION")
                || s.contains("SESSION")
                || s.contains("TOKEN") =>
            {
                3
            }
            s if s.contains("CONNECTION")
                || s.contains("DAEMON")
                || s.contains("OFFLINE")
                || s.contains("AUDIO") =>
            {
                5
            }
            s if s.contains("REQUIRED")
                || s.contains("CONFLICT")
                || s.contains("BUSY")
                || s.contains("EXPIRED")
                || s.contains("NOT_READY")
                || s.contains("PENDING") =>
            {
                4
            }
            "INTERRUPTED" => 130,
            _ => 1,
        }
    }
}
impl fmt::Display for RingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}: {} (step: {}). {}",
            self.code, self.message, self.step, self.next_action
        )
    }
}
impl std::error::Error for RingError {}

#[derive(Clone, Debug)]
pub struct ConnectOptions {
    pub server_url: String,
    pub org_id: Option<String>,
    pub realm: String,
    pub test_app_secret: Option<String>,
    pub client_name: String,
}
impl Default for ConnectOptions {
    fn default() -> Self {
        Self {
            server_url: "ws://127.0.0.1:8765/ws".into(),
            org_id: None,
            realm: "production".into(),
            test_app_secret: None,
            client_name: "ring-client".into(),
        }
    }
}
type Pending = Arc<Mutex<HashMap<String, oneshot::Sender<Result<Value>>>>>;
#[derive(Clone)]
pub struct Client {
    outgoing: mpsc::Sender<Value>,
    pending: Pending,
    events: broadcast::Sender<Value>,
}
impl Client {
    pub async fn connect(options: &ConnectOptions) -> Result<Self> {
        let url = url::Url::parse(&options.server_url).map_err(|_| {
            RingError::new(
                "INVALID_INPUT",
                "server_url must be a WebSocket URL",
                "connect",
                "Use wss:// for remote servers or ws:// on loopback.",
            )
        })?;
        if !url.username().is_empty() || url.password().is_some() {
            return Err(RingError::new(
                "INVALID_INPUT",
                "Credentials must not appear in server_url",
                "connect",
                "Use auth.login or auth.resume for authentication.",
            ));
        }
        if url.scheme() != "wss"
            && !(url.scheme() == "ws"
                && matches!(
                    url.host_str(),
                    Some("localhost" | "127.0.0.1" | "[::1]" | "::1")
                ))
        {
            return Err(RingError::new(
                "INVALID_INPUT",
                "Unencrypted remote WebSockets are forbidden",
                "connect",
                "Use wss:// or a local loopback address.",
            ));
        }
        let (ws, _) =
            tokio::time::timeout(Duration::from_secs(15), connect_async(&options.server_url))
                .await
                .map_err(|_| RingError::connection("WebSocket connection timed out", "connect"))?
                .map_err(|e| RingError::connection(format!("Could not connect: {e}"), "connect"))?;
        let (mut sink, mut stream) = ws.split();
        let (outgoing, mut receiver) = mpsc::channel::<Value>(128);
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let (events, _) = broadcast::channel(2048);
        let p = pending.clone();
        let ev = events.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    msg=receiver.recv()=> match msg { Some(v)=>if sink.send(Message::Text(v.to_string().into())).await.is_err(){break;},None=>break },
                    msg=stream.next()=>match msg {
                        Some(Ok(Message::Text(text)))=>if let Ok(v)=serde_json::from_str::<Value>(&text) {
                            if let Some(id)=v.get("id").and_then(Value::as_str) {
                                if let Some(tx)=p.lock().await.remove(id) {
                                    let result=if v["ok"]==true {Ok(v["result"].clone())} else {
                                        let e=&v["error"]; Err(RingError {
                                            code:e["code"].as_str().unwrap_or("OPERATION_FAILED").into(),
                                            message:e["message"].as_str().unwrap_or("Server rejected operation").into(),
                                            step:e["step"].as_str().unwrap_or("server").into(), request_id:Some(id.into()),
                                            retryable:e["retryable"].as_bool().unwrap_or(false),
                                            next_action:e["next_action"].as_str().unwrap_or("Inspect command help and resource state.").into(),
                                            details:e.get("details").cloned(),
                                        })
                                    }; let _=tx.send(result);
                                }
                            } else {let _=ev.send(v);}
                        },
                        Some(Ok(Message::Ping(data)))=>if sink.send(Message::Pong(data)).await.is_err(){break;},
                        Some(Ok(Message::Close(_)))|Some(Err(_))|None=>break,
                        _=>{},
                    }
                }
            }
            for (id, tx) in p.lock().await.drain() {
                let mut e =
                    RingError::connection("Socket closed before a confirmed reply", "websocket");
                e.request_id = Some(id);
                let _ = tx.send(Err(e));
            }
            let _ = ev.send(json!({"type":"connection.closed","data":{}}));
        });
        let client = Self {
            outgoing,
            pending,
            events,
        };
        let mut hello = json!({"versions":[PROTOCOL_MAJOR],"client":{"name":options.client_name,"version":env!("CARGO_PKG_VERSION")},"realm":options.realm,"capabilities":["events","audio.pcm_s16le","context.approval"]});
        if let Some(org) = &options.org_id {
            hello["org_id"] = json!(org);
        }
        if let Some(secret) = &options.test_app_secret {
            hello["test_app_secret"] = json!(secret);
        }
        client.request("protocol.hello", hello).await?;
        Ok(client)
    }
    pub fn events(&self) -> broadcast::Receiver<Value> {
        self.events.subscribe()
    }
    pub async fn request(&self, method: &str, params: Value) -> Result<Value> {
        self.request_with_id(&uuid::Uuid::new_v4().to_string(), method, params, None)
            .await
    }
    pub async fn request_with_id(
        &self,
        id: &str,
        method: &str,
        params: Value,
        isi: Option<&str>,
    ) -> Result<Value> {
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending.lock().await;
            if pending.contains_key(id) {
                return Err(RingError::new(
                    "REQUEST_CONFLICT",
                    "Request ID already in flight",
                    method,
                    "Wait for the original request before retrying.",
                ));
            }
            pending.insert(id.into(), tx);
        }
        let mut frame = json!({"id":id,"method":method,"params":params});
        if let Some(isi) = isi {
            frame["metadata"] = json!({"isi":isi});
        }
        if self.outgoing.send(frame).await.is_err() {
            self.pending.lock().await.remove(id);
            return Err(RingError::connection("Connection has closed", method));
        }
        let timeout = if method == "voicemail.begin" {
            Duration::from_secs(120)
        } else {
            Duration::from_secs(45)
        };
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(result)) => result,
            _ => {
                self.pending.lock().await.remove(id);
                let mut e =
                    RingError::connection("No confirmed reply before the request deadline", method);
                e.request_id = Some(id.into());
                Err(e)
            }
        }
    }
    pub async fn frame(&self, kind: &str, data: Value) -> Result<()> {
        self.outgoing
            .send(json!({"type":kind,"data":data}))
            .await
            .map_err(|_| RingError::connection("Connection has closed", kind))
    }
    pub async fn upload(&self, id: &str, purpose: &str, mime: &str, bytes: &[u8]) -> Result<Value> {
        use base64::Engine;
        let upload = self
            .request_with_id(
                &format!("{id}:begin"),
                "assets.begin",
                json!({"purpose":purpose,"mime_type":mime,"size_bytes":bytes.len()}),
                None,
            )
            .await?;
        let asset = upload["asset_id"].as_str().ok_or_else(|| {
            RingError::new(
                "PROTOCOL_ERROR",
                "Upload returned no asset ID",
                "assets.begin",
                "Check server compatibility.",
            )
        })?;
        let progress = self
            .request("assets.get", json!({"asset_id":asset}))
            .await?;
        if progress["complete"] == true {
            return Ok(json!({"asset_id":asset,"complete":true}));
        }
        let next = progress["next_seq"].as_u64().unwrap_or(0) as usize;
        let mut events = self.events();
        for (seq, chunk) in bytes.chunks(32 * 1024).enumerate().skip(next) {
            self.frame("assets.chunk",json!({"asset_id":asset,"seq":seq,"data_base64":base64::engine::general_purpose::STANDARD.encode(chunk)})).await?;
            tokio::time::timeout(Duration::from_secs(30), async {
                loop {
                    let v = events.recv().await.map_err(|_| {
                        RingError::connection("Upload acknowledgement unavailable", "assets.chunk")
                    })?;
                    if v["type"] == "assets.ack" && v["data"]["asset_id"] == asset {
                        if v["data"]["next_seq"].as_u64().unwrap_or(seq as u64 + 1) > seq as u64 {
                            return Ok(());
                        }
                    }
                    if v["type"] == "protocol.error"
                        || v["type"] == "error"
                        || v["type"] == "stream.error"
                    {
                        return Err(RingError::new(
                            "UPLOAD_FAILED",
                            "Server rejected an upload chunk",
                            "assets.chunk",
                            "Inspect assets.get before retrying.",
                        ));
                    }
                }
            })
            .await
            .map_err(|_| {
                RingError::connection("Upload acknowledgement timed out", "assets.chunk")
            })??;
        }
        self.request_with_id(
            &format!("{id}:complete"),
            "assets.complete",
            json!({"asset_id":asset}),
            None,
        )
        .await
    }
    pub async fn download(&self, asset: &str) -> Result<Vec<u8>> {
        use base64::Engine;
        let mut events = self.events();
        let v = self
            .request("assets.get", json!({"asset_id":asset}))
            .await?;
        if let Some(data) = v["data_base64"].as_str() {
            return base64::engine::general_purpose::STANDARD
                .decode(data)
                .map_err(|_| {
                    RingError::new(
                        "PROTOCOL_ERROR",
                        "Invalid asset encoding",
                        "assets.get",
                        "Retry the download.",
                    )
                });
        }
        let transfer = v["transfer_id"].as_str().ok_or_else(|| {
            RingError::new(
                "ASSET_NOT_READY",
                "Asset is not ready for download",
                "assets.get",
                "Wait for processing, then retry.",
            )
        })?;
        tokio::time::timeout(Duration::from_secs(120), async {
            let mut bytes = Vec::new();
            let mut next = 0u64;
            loop {
                let frame = events.recv().await.map_err(|_| {
                    RingError::connection("Asset transfer interrupted", "assets.chunk")
                })?;
                if frame["type"] != "assets.chunk" || frame["data"]["transfer_id"] != transfer {
                    continue;
                }
                let d = &frame["data"];
                if d["seq"].as_u64() != Some(next) {
                    return Err(RingError::new(
                        "TRANSFER_INCOMPLETE",
                        "Asset chunks were lost or reordered",
                        "assets.chunk",
                        "Restart this download.",
                    ));
                }
                let chunk = base64::engine::general_purpose::STANDARD
                    .decode(d["data_base64"].as_str().unwrap_or(""))
                    .map_err(|_| {
                        RingError::new(
                            "PROTOCOL_ERROR",
                            "Invalid asset encoding",
                            "assets.chunk",
                            "Restart this download.",
                        )
                    })?;
                bytes.extend(chunk);
                next += 1;
                if bytes.len() > 512 * 1024 * 1024 {
                    return Err(RingError::new(
                        "ASSET_TOO_LARGE",
                        "Download exceeds 512 MiB local memory limit",
                        "assets.get",
                        "Use a streaming native client for this asset.",
                    ));
                }
                if d["final"] == true {
                    return Ok(bytes);
                }
            }
        })
        .await
        .map_err(|_| RingError::connection("Asset transfer timed out", "assets.get"))?
    }
}

pub fn checked_text(text: &str, max: usize, field: &str) -> Result<()> {
    let count = text.chars().count();
    if count > max {
        Err(RingError::new(
            "TEXT_TOO_LONG",
            format!("{field} has {count} Unicode code points; maximum is {max}"),
            field,
            "Shorten the text; input is never truncated.",
        ))
    } else {
        Ok(())
    }
}
/// Normalize a global actor ID. A legacy membership suffix never grants authority
/// or constrains the caller's independently authenticated organization context.
pub fn actor_id(input: &str, _org: Option<&str>) -> Result<String> {
    let id = input.strip_prefix('@').unwrap_or(input);
    let (public, membership) = match id.split_once('[') {
        Some((p, m)) => (
            p,
            Some(m.strip_suffix(']').ok_or_else(|| {
                RingError::new(
                    "INVALID_ACTOR",
                    "Membership ID is missing ]",
                    "actor",
                    "Use si:handle[org] or c:handle[org].",
                )
            })?),
        ),
        None => (id, None),
    };
    let handle = public
        .strip_prefix("si:")
        .or_else(|| public.strip_prefix("c:"));
    if handle.is_none_or(|h| {
        h.is_empty()
            || !h
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
    }) {
        return Err(RingError::new(
            "INVALID_ACTOR",
            "Actor must use si:handle or c:handle",
            "actor",
            "Supply a public IAM actor ID.",
        ));
    }
    if membership.is_some_and(|m| {
        m.is_empty()
            || !m
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
    }) {
        return Err(RingError::new(
            "INVALID_ACTOR",
            "Membership suffix must contain a valid organization ID",
            "actor",
            "Supply a global si:handle or c:handle; legacy [org] suffixes must be well formed.",
        ));
    }
    Ok(public.into())
}
pub fn duration(input: &str) -> Result<u64> {
    let invalid = || {
        RingError::new(
            "INVALID_INPUT",
            "Duration must be a positive integer followed by s, m, or h",
            "duration",
            "Use 30s, 15m, or 1h.",
        )
    };
    let split = input.len().checked_sub(1).ok_or_else(invalid)?;
    if !input.is_char_boundary(split) {
        return Err(invalid());
    }
    let (n, unit) = input.split_at(split);
    let n = n.parse::<u64>().map_err(|_| invalid())?;
    let factor = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3600,
        _ => return Err(invalid()),
    };
    n.checked_mul(factor).filter(|n| *n > 0).ok_or_else(invalid)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_boundaries() {
        assert!(checked_text(&"🦀".repeat(160), 160, "text").is_ok());
        assert!(checked_text(&"🦀".repeat(161), 160, "text").is_err());
        assert_eq!(actor_id("@si:alex[team]", Some("team")).unwrap(), "si:alex");
        assert_eq!(actor_id("c:alex[other]", Some("team")).unwrap(), "c:alex");
        assert_eq!(duration("1h").unwrap(), 3600);
        assert!(duration("0s").is_err());
    }
    #[test]
    fn actor_ids_are_global_and_legacy_suffixes_are_validated() {
        for selected in [None, Some("caller-org"), Some("other-org")] {
            for (input, expected) in [
                ("c:alex", "c:alex"),
                ("@si:assistant", "si:assistant"),
                ("c:alex[other-org]", "c:alex"),
                ("@si:assistant[team_2.test]", "si:assistant"),
            ] {
                assert_eq!(actor_id(input, selected).unwrap(), expected);
            }
        }
        for input in [
            "c:alex[]",
            "c:alex[team",
            "c:alex[team]extra",
            "c:alex[team][other]",
            "c:alex[[team]]",
            "c:alex[team]]",
            "c:alex[team org]",
            "c:[team]",
            "alex",
            "si:",
        ] {
            assert_eq!(
                actor_id(input, Some("team")).unwrap_err().code,
                "INVALID_ACTOR"
            );
        }
    }
    #[tokio::test]
    async fn transport_matches_interleaved_replies_and_events() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(socket).await.unwrap();
            let hello: Value =
                serde_json::from_str(ws.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();
            assert_eq!(hello["method"], "protocol.hello");
            ws.send(Message::Text(
                json!({"id":hello["id"],"ok":true,"result":{"protocol_major":1}})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
            let mut requests = Vec::new();
            for _ in 0..2 {
                let v: Value =
                    serde_json::from_str(ws.next().await.unwrap().unwrap().to_text().unwrap())
                        .unwrap();
                requests.push(v);
            }
            ws.send(Message::Text(
                json!({"type":"call.accepted","seq":7,"data":{"ringid":"call"}})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
            for request in requests.into_iter().rev() {
                ws.send(Message::Text(
                    json!({"id":request["id"],"ok":true,"result":{"method":request["method"]}})
                        .to_string()
                        .into(),
                ))
                .await
                .unwrap();
            }
        });
        let client = Client::connect(&ConnectOptions {
            server_url: format!("ws://{address}/ws"),
            ..Default::default()
        })
        .await
        .unwrap();
        let mut events = client.events();
        let (first, second) = tokio::join!(
            client.request_with_id("one", "first", json!({}), Some("brain")),
            client.request_with_id("two", "second", json!({}), None)
        );
        assert_eq!(first.unwrap()["method"], "first");
        assert_eq!(second.unwrap()["method"], "second");
        assert_eq!(events.recv().await.unwrap()["seq"], 7);
        server.await.unwrap();
    }
}
