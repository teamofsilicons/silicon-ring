//! Realtime events over `GET /v1/ws`.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use rand::Rng;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot};
use tokio::time::{Instant, MissedTickBehavior};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::{HeaderValue, StatusCode, header};
use tokio_tungstenite::tungstenite::protocol::Message;
use tokio_tungstenite::{Connector, MaybeTlsStream, WebSocketStream, tungstenite};

use crate::proto::Event;
use crate::{Client, Error, Result};

const PING_EVERY: Duration = Duration::from_secs(25);
/// The server pings every 20 s, so this much silence means the connection is gone.
const IDLE_LIMIT: Duration = Duration::from_secs(60);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const BACKOFF_START: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(30);
/// A connection that lasted this long resets the backoff; shorter ones keep backing off, so a
/// server that accepts and then drops the socket straight away is not hammered.
const STABLE_AFTER: Duration = Duration::from_secs(30);
const SESSION_REVOKED_CLOSE: u16 = 4401;

/// What a [`Subscription`] yields.
#[derive(Clone, Debug)]
#[allow(clippy::large_enum_variant)] // events dominate the stream, so boxing them buys nothing
pub enum Update {
    /// Connected (or reconnected). Callers should reconcile state via REST
    /// (`GET /v1/calls?state=ringing,active,voicemail`), since events may have been missed.
    Connected,
    /// A server event.
    Event(Event),
    /// The connection dropped; a reconnect is scheduled after `retry_in_ms`.
    Disconnected { reason: String, retry_in_ms: u64 },
    /// The session is no longer valid (401 or `session.revoked`). No further reconnects.
    SignedOut,
}

/// A live subscription. Dropping it (or calling [`Subscription::close`]) closes the socket.
pub struct Subscription {
    pub(crate) rx: mpsc::UnboundedReceiver<Update>,
    pub(crate) shutdown: Option<tokio::sync::oneshot::Sender<()>>,
}

impl Subscription {
    /// Next update; `None` after close or `SignedOut`.
    pub async fn next(&mut self) -> Option<Update> {
        self.rx.recv().await
    }

    pub fn close(mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
    }
}

impl Client {
    /// Opens `/v1/ws` with this client's session and keeps it open: sends `{"type":"ping"}`
    /// every 25 s, reconnects with backoff (1 s → 30 s, jittered) and yields [`Update`]s.
    /// Must be called inside a Tokio runtime.
    ///
    /// Connection problems, including the first attempt, arrive as [`Update::Disconnected`] or
    /// [`Update::SignedOut`]; this only fails when there is no session.
    pub async fn subscribe(&self) -> Result<Subscription> {
        let token = self.session_token.clone().ok_or(Error::NotSignedIn)?;
        let mut url = self.api_url.clone();
        let scheme = if url.scheme() == "https" { "wss" } else { "ws" };
        url.set_scheme(scheme).map_err(|_| {
            Error::Input("The API URL cannot be turned into a WebSocket URL.".into())
        })?;
        url.set_path("/v1/ws");
        let target = Target {
            url: url.to_string(),
            token,
            user_agent: self.user_agent.clone(),
        };

        let (tx, rx) = mpsc::unbounded_channel();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        tokio::spawn(run(target, tx, shutdown_rx));
        Ok(Subscription {
            rx,
            shutdown: Some(shutdown_tx),
        })
    }
}

struct Target {
    url: String,
    token: String,
    user_agent: String,
}

enum Failure {
    Unauthorized,
    Other(String),
}

enum End {
    /// The subscriber closed or went away.
    Closed,
    SignedOut,
    Dropped(String),
}

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

async fn run(
    target: Target,
    tx: mpsc::UnboundedSender<Update>,
    mut shutdown: oneshot::Receiver<()>,
) {
    let mut attempt: u32 = 0;
    loop {
        let connected = tokio::select! {
            _ = &mut shutdown => return,
            result = connect(&target) => result,
        };
        let reason = match connected {
            Ok(socket) => {
                if tx.send(Update::Connected).is_err() {
                    return;
                }
                let connected_at = Instant::now();
                match session(socket, &tx, &mut shutdown).await {
                    End::Closed => return,
                    End::SignedOut => {
                        let _ = tx.send(Update::SignedOut);
                        return;
                    }
                    End::Dropped(reason) => {
                        if connected_at.elapsed() >= STABLE_AFTER {
                            attempt = 0;
                        }
                        reason
                    }
                }
            }
            Err(Failure::Unauthorized) => {
                let _ = tx.send(Update::SignedOut);
                return;
            }
            Err(Failure::Other(reason)) => reason,
        };

        let delay = backoff(attempt);
        attempt = attempt.saturating_add(1);
        tracing::debug!(%reason, retry_in_ms = delay.as_millis() as u64, "ring websocket disconnected");
        if tx
            .send(Update::Disconnected {
                reason,
                retry_in_ms: delay.as_millis() as u64,
            })
            .is_err()
        {
            return;
        }
        tokio::select! {
            _ = &mut shutdown => return,
            _ = tx.closed() => return,
            _ = tokio::time::sleep(delay) => {}
        }
    }
}

async fn connect(target: &Target) -> std::result::Result<Socket, Failure> {
    let mut request = target
        .url
        .as_str()
        .into_client_request()
        .map_err(|e| Failure::Other(format!("invalid WebSocket request: {e}")))?;
    let headers = request.headers_mut();
    let bearer = HeaderValue::from_str(&format!("Bearer {}", target.token))
        .map_err(|_| Failure::Other("the session token is not a valid header value".into()))?;
    headers.insert(header::AUTHORIZATION, bearer);
    if let Ok(agent) = HeaderValue::from_str(&target.user_agent) {
        headers.insert(header::USER_AGENT, agent);
    }

    let connector = Connector::Rustls(crate::tls::websocket());
    let attempt =
        tokio_tungstenite::connect_async_tls_with_config(request, None, true, Some(connector));
    match tokio::time::timeout(CONNECT_TIMEOUT, attempt).await {
        Err(_) => Err(Failure::Other("timed out connecting".into())),
        Ok(Ok((socket, _))) => Ok(socket),
        Ok(Err(tungstenite::Error::Http(response)))
            if response.status() == StatusCode::UNAUTHORIZED =>
        {
            Err(Failure::Unauthorized)
        }
        Ok(Err(tungstenite::Error::Http(response))) => Err(Failure::Other(format!(
            "the server refused the WebSocket with HTTP {}",
            response.status().as_u16()
        ))),
        Ok(Err(e)) => Err(Failure::Other(e.to_string())),
    }
}

async fn session(
    socket: Socket,
    tx: &mpsc::UnboundedSender<Update>,
    shutdown: &mut oneshot::Receiver<()>,
) -> End {
    let (mut sink, mut stream) = socket.split();
    let mut ping = tokio::time::interval_at(Instant::now() + PING_EVERY, PING_EVERY);
    ping.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut last_frame = Instant::now();

    loop {
        tokio::select! {
            _ = &mut *shutdown => {
                let _ = sink.send(Message::Close(None)).await;
                return End::Closed;
            }
            _ = tx.closed() => {
                let _ = sink.send(Message::Close(None)).await;
                return End::Closed;
            }
            _ = ping.tick() => {
                if last_frame.elapsed() > IDLE_LIMIT {
                    return End::Dropped("no frames from the server for 60 s".into());
                }
                if let Err(e) = sink.send(Message::text(r#"{"type":"ping"}"#)).await {
                    return End::Dropped(e.to_string());
                }
            }
            frame = stream.next() => {
                let message = match frame {
                    None => return End::Dropped("the server closed the connection".into()),
                    Some(Err(e)) => return End::Dropped(e.to_string()),
                    Some(Ok(message)) => message,
                };
                last_frame = Instant::now();
                match message {
                    Message::Text(text) => {
                        let Some(event) = parse_event(text.as_str()) else { continue };
                        let revoked = matches!(event, Event::SessionRevoked {});
                        if tx.send(Update::Event(event)).is_err() {
                            return End::Closed;
                        }
                        if revoked {
                            return End::SignedOut;
                        }
                    }
                    // 4401 is the server closing a revoked session; reconnecting would only get a 401.
                    Message::Close(Some(frame)) if u16::from(frame.code) == SESSION_REVOKED_CLOSE => {
                        return End::SignedOut;
                    }
                    Message::Close(frame) => {
                        let reason = frame
                            .map(|f| format!("the server closed the connection ({}: {})", u16::from(f.code), f.reason))
                            .unwrap_or_else(|| "the server closed the connection".into());
                        return End::Dropped(reason);
                    }
                    _ => {}
                }
            }
        }
    }
}

/// Parses a server frame. Pongs and event types this build does not know are skipped, so a newer
/// server can add events without breaking older clients.
fn parse_event(text: &str) -> Option<Event> {
    let mut value: serde_json::Value = serde_json::from_str(text).ok()?;
    let object = value.as_object_mut()?;
    let kind = object.get("type")?.as_str()?.to_owned();
    if kind == "pong" {
        return None;
    }
    let data = object
        .remove("data")
        .filter(|d| !d.is_null())
        .unwrap_or_else(|| serde_json::json!({}));
    match serde_json::from_value(serde_json::json!({ "type": kind, "data": data })) {
        Ok(event) => Some(event),
        Err(e) => {
            tracing::debug!(event = %kind, error = %e, "skipping a WebSocket event this client cannot read");
            None
        }
    }
}

fn backoff(attempt: u32) -> Duration {
    let base = BACKOFF_START
        .saturating_mul(1u32 << attempt.min(5))
        .min(BACKOFF_MAX);
    let jitter = rand::rng().random_range(0.8..=1.2);
    base.mul_f64(jitter).clamp(BACKOFF_START, BACKOFF_MAX)
}
