//! The loopback WebSocket to ring-server (`GET /internal/media`): commands in, results out.
//! Results produced while disconnected are queued and delivered after reconnecting.

use std::{collections::VecDeque, sync::Arc, time::Duration};

use anyhow::{Context, Result, anyhow, bail};
use futures_util::{SinkExt, StreamExt};
use ring_proto::internal::{FromMedia, ToMedia};
use tokio::{sync::mpsc, time::Instant};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{Message, client::IntoClientRequest},
};
use tokio_util::sync::CancellationToken;

use crate::{config::Config, supervisor::Supervisor};

/// Sends results to ring-server. Cloned into every session.
#[derive(Clone)]
pub struct Reporter {
    tx: mpsc::UnboundedSender<FromMedia>,
}

impl Reporter {
    pub fn channel() -> (Self, mpsc::UnboundedReceiver<FromMedia>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Self { tx }, rx)
    }

    pub fn send(&self, message: FromMedia) {
        let _ = self.tx.send(message);
    }
}

const PING_INTERVAL: Duration = Duration::from_secs(20);
const SILENCE_LIMIT: Duration = Duration::from_secs(60);
/// Bound on results held while ring-server is unreachable.
const MAX_PENDING: usize = 10_000;

/// Runs until `stop` is cancelled and every queued result has been delivered (or delivery
/// became impossible).
pub async fn run(
    config: Arc<Config>,
    supervisor: Arc<Supervisor>,
    mut outgoing: mpsc::UnboundedReceiver<FromMedia>,
    stop: CancellationToken,
) {
    let mut pending: VecDeque<FromMedia> = VecDeque::new();
    let mut failures = 0u32;
    loop {
        let connected = tokio::select! {
            connected = connect(&config) => connected,
            _ = stop.cancelled(), if pending.is_empty() && outgoing.is_empty() => return,
        };
        match connected {
            Ok(socket) => {
                tracing::info!("connected to ring-server");
                let opened = Instant::now();
                match serve(socket, &supervisor, &mut outgoing, &mut pending, &stop).await {
                    Ok(()) => return,
                    Err(e) => tracing::warn!("ring-server connection lost: {e:#}"),
                }
                if opened.elapsed() > Duration::from_secs(30) {
                    failures = 0;
                }
            }
            Err(e) => tracing::warn!("cannot reach ring-server: {e:#}"),
        }
        let stopping = stop.is_cancelled();
        if stopping && pending.is_empty() && outgoing.is_empty() {
            return;
        }
        // While stopping, keep trying briefly to hand over the last results; the caller bounds
        // how long that may take.
        failures += 1;
        let delay = if stopping {
            Duration::from_millis(500)
        } else {
            Duration::from_millis((500u64 << failures.min(5)).min(10_000))
        };
        let deadline = Instant::now() + delay;
        loop {
            tokio::select! {
                _ = tokio::time::sleep_until(deadline) => break,
                _ = stop.cancelled(), if !stopping => break,
                message = outgoing.recv() => match message {
                    Some(message) => hold(&mut pending, message),
                    None => break,
                },
            }
        }
    }
}

/// Queues a result for later. Interim speech is only useful live, so it is dropped.
fn hold(pending: &mut VecDeque<FromMedia>, message: FromMedia) {
    if matches!(message, FromMedia::SpeechPartial { .. }) {
        return;
    }
    if pending.len() >= MAX_PENDING {
        pending.pop_front();
    }
    pending.push_back(message);
}

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn connect(config: &Config) -> Result<Socket> {
    let mut request = config.server_url.as_str().into_client_request()?;
    request.headers_mut().insert(
        "Authorization",
        format!("Bearer {}", config.media_secret)
            .parse()
            .context("RING_MEDIA_SECRET is not a valid header value")?,
    );
    let (socket, _) = tokio::time::timeout(Duration::from_secs(10), connect_async(request))
        .await
        .context("timed out")?
        .map_err(|e| anyhow!("{e}"))?;
    Ok(socket)
}

async fn serve(
    socket: Socket,
    supervisor: &Arc<Supervisor>,
    outgoing: &mut mpsc::UnboundedReceiver<FromMedia>,
    pending: &mut VecDeque<FromMedia>,
    stop: &CancellationToken,
) -> Result<()> {
    let (mut sink, mut stream) = socket.split();
    let hello = FromMedia::Hello {
        version: crate::VERSION.into(),
        active_rings: supervisor.active_rings(),
    };
    sink.send(encode(&hello)?).await?;
    // Results from the previous connection, then anything produced since.
    while let Some(message) = pending.front() {
        sink.send(encode(message)?).await?;
        pending.pop_front();
    }
    let mut ping = tokio::time::interval(PING_INTERVAL);
    ping.reset();
    let mut last_heard = Instant::now();
    let mut stopping = false;
    loop {
        tokio::select! {
            message = outgoing.recv() => {
                let Some(message) = message else { break };
                if let Err(e) = sink.send(encode(&message)?).await {
                    hold(pending, message);
                    return Err(e.into());
                }
            }
            frame = stream.next() => {
                last_heard = Instant::now();
                match frame {
                    Some(Ok(Message::Text(text))) => match serde_json::from_str::<ToMedia>(&text) {
                        Ok(command) => supervisor.dispatch(command),
                        Err(e) => tracing::warn!("ignoring a message from ring-server: {e}"),
                    },
                    Some(Ok(Message::Close(_))) | None => bail!("closed by ring-server"),
                    Some(Ok(_)) => {}
                    Some(Err(e)) => return Err(e.into()),
                }
            }
            _ = ping.tick() => {
                if last_heard.elapsed() > SILENCE_LIMIT {
                    bail!("no traffic from ring-server for {} s", SILENCE_LIMIT.as_secs());
                }
                sink.send(Message::Ping(Default::default())).await?;
            }
            _ = stop.cancelled(), if !stopping => stopping = true,
        }
        if stopping {
            // Sessions have all finished; deliver what they reported and leave.
            while let Ok(message) = outgoing.try_recv() {
                if let Err(e) = sink.send(encode(&message)?).await {
                    hold(pending, message);
                    return Err(e.into());
                }
            }
            break;
        }
    }
    let _ = sink.send(Message::Close(None)).await;
    Ok(())
}

fn encode(message: &FromMedia) -> Result<Message> {
    Ok(Message::Text(serde_json::to_string(message)?.into()))
}
