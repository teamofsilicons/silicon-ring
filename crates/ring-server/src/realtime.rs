//! Realtime fan-out: one broadcast channel per account, and the `/v1/ws` socket.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use futures_util::{SinkExt, StreamExt};
use ring_proto::{Event, EventFrame, Time};
use tokio::sync::broadcast;

use crate::accounts::{self, Liveness};
use crate::app::AppState;
use crate::error::ApiError;
use crate::extract::{authenticate, session_token};

const CHANNEL_CAPACITY: usize = 512;
const PING_EVERY: Duration = Duration::from_secs(20);
const IDLE_LIMIT: Duration = Duration::from_secs(90);
const INTROSPECT_EVERY: Duration = Duration::from_secs(10 * 60);

#[derive(Debug)]
pub struct Outbound {
    /// Only this device receives the frame (`None`: every device of the account).
    pub device: Option<String>,
    pub frame: Arc<str>,
    /// Close the socket after sending (session revoked).
    pub close: bool,
}

#[derive(Default)]
pub struct Hub {
    channels: Mutex<HashMap<String, broadcast::Sender<Arc<Outbound>>>>,
    /// device id -> (account uuid, open sockets)
    online: Mutex<HashMap<String, (String, usize)>>,
}

pub fn frame(event: &Event) -> Arc<str> {
    let frame = EventFrame {
        event: event.clone(),
        at: Time::now(),
    };
    serde_json::to_string(&frame)
        .unwrap_or_else(|_| "{}".into())
        .into()
}

impl Hub {
    fn subscribe(&self, uuid: &str) -> broadcast::Receiver<Arc<Outbound>> {
        let mut channels = self.channels.lock().unwrap_or_else(|p| p.into_inner());
        channels
            .entry(uuid.to_string())
            .or_insert_with(|| broadcast::channel(CHANNEL_CAPACITY).0)
            .subscribe()
    }

    fn publish(&self, uuid: &str, outbound: Outbound) {
        let mut channels = self.channels.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(sender) = channels.get(uuid) {
            if sender.receiver_count() == 0 {
                channels.remove(uuid);
            } else {
                let _ = sender.send(Arc::new(outbound));
            }
        }
    }

    pub fn send_account(&self, uuid: &str, event: &Event) {
        self.publish(
            uuid,
            Outbound {
                device: None,
                frame: frame(event),
                close: false,
            },
        );
    }

    pub fn send_frame(&self, uuid: &str, frame: Arc<str>) {
        self.publish(
            uuid,
            Outbound {
                device: None,
                frame,
                close: false,
            },
        );
    }

    /// Tells one device its session ended and closes its sockets.
    pub fn revoke_device(&self, device_id: &str) {
        let owner = self
            .online
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(device_id)
            .map(|(uuid, _)| uuid.clone());
        if let Some(uuid) = owner {
            self.publish(
                &uuid,
                Outbound {
                    device: Some(device_id.to_string()),
                    frame: frame(&Event::SessionRevoked {}),
                    close: true,
                },
            );
        }
    }

    pub fn is_online(&self, device_id: &str) -> bool {
        self.online
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(device_id)
            .is_some_and(|(_, n)| *n > 0)
    }

    /// Returns true when this is the device's first socket.
    fn connect(&self, uuid: &str, device_id: &str) -> bool {
        let mut online = self.online.lock().unwrap_or_else(|p| p.into_inner());
        let entry = online
            .entry(device_id.to_string())
            .or_insert_with(|| (uuid.to_string(), 0));
        entry.1 += 1;
        entry.1 == 1
    }

    /// Returns true when this was the device's last socket.
    fn disconnect(&self, device_id: &str) -> bool {
        let mut online = self.online.lock().unwrap_or_else(|p| p.into_inner());
        match online.get_mut(device_id) {
            Some(entry) if entry.1 > 1 => {
                entry.1 -= 1;
                false
            }
            Some(_) => {
                online.remove(device_id);
                true
            }
            None => false,
        }
    }
}

fn origin_allowed(app: &AppState, headers: &HeaderMap) -> bool {
    let Some(origin) = headers
        .get(axum::http::header::ORIGIN)
        .and_then(|v| v.to_str().ok())
    else {
        return true; // native clients send no Origin
    };
    let origin = origin.trim_end_matches('/');
    app.config.allowed_origins.iter().any(|o| o == origin) || app.config.public_url == origin
}

pub async fn ws(
    State(app): State<AppState>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Response {
    if !origin_allowed(&app, &headers) {
        return ApiError::forbidden("This origin may not open the Ring socket.").into_response();
    }
    let Some((token, via_cookie)) = session_token(&headers) else {
        return ApiError::unauthenticated().into_response();
    };
    let auth = match authenticate(&app, &token, via_cookie) {
        Ok(auth) => auth,
        Err(e) => return e.into_response(),
    };
    if !check_live(&app, &auth.session.device_id, &auth.account.uuid).await {
        return ApiError::session_expired().into_response();
    }
    let device_id = auth.session.device_id.clone();
    let account = auth.account.clone();
    upgrade
        .max_message_size(64 * 1024)
        .on_upgrade(move |socket| serve(app, socket, account, device_id))
}

/// Introspects the session's refresh token; ends the session when Accounts says it is over.
/// Network trouble keeps the session.
async fn check_live(app: &AppState, device_id: &str, uuid: &str) -> bool {
    match accounts::session_liveness(app, device_id, uuid).await {
        Liveness::Inactive => {
            crate::sessions::revoke_session(app, device_id, "signed_out", false).await;
            false
        }
        Liveness::Active | Liveness::Unknown => true,
    }
}

async fn serve(app: AppState, socket: WebSocket, account: ring_proto::Account, device_id: String) {
    let uuid = account.uuid.clone();
    let mut rx = app.hub.subscribe(&uuid);
    if app.hub.connect(&uuid, &device_id) {
        announce_device(&app, &uuid, &device_id);
    }
    let (mut sink, mut stream) = socket.split();
    let ready = frame(&Event::Ready {
        account,
        device_id: device_id.clone(),
    });
    let mut alive = sink
        .send(Message::Text(ready.as_ref().into()))
        .await
        .is_ok();

    let mut ping = tokio::time::interval(PING_EVERY);
    ping.tick().await;
    let mut introspect = tokio::time::interval(INTROSPECT_EVERY);
    introspect.tick().await;
    let mut last_inbound = Instant::now();

    while alive {
        tokio::select! {
            outbound = rx.recv() => match outbound {
                Ok(out) => {
                    if out.device.as_deref().is_some_and(|d| d != device_id) {
                        continue;
                    }
                    if sink.send(Message::Text(out.frame.as_ref().into())).await.is_err() {
                        break;
                    }
                    if out.close {
                        let _ = sink.send(Message::Close(Some(CloseFrame { code: 4401, reason: "session revoked".into() }))).await;
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    // The client missed events; closing makes it reconnect and reconcile over REST.
                    let _ = sink.send(Message::Close(Some(CloseFrame { code: 1013, reason: "reconnect".into() }))).await;
                    break;
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },
            inbound = stream.next() => match inbound {
                Some(Ok(Message::Text(text))) => {
                    last_inbound = Instant::now();
                    if is_ping(text.as_str()) {
                        let pong = frame(&Event::Pong {});
                        alive = sink.send(Message::Text(pong.as_ref().into())).await.is_ok();
                    }
                }
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                Some(Ok(_)) => last_inbound = Instant::now(),
            },
            _ = ping.tick() => {
                if last_inbound.elapsed() > IDLE_LIMIT {
                    let _ = sink.send(Message::Close(Some(CloseFrame { code: 1001, reason: "idle".into() }))).await;
                    break;
                }
                alive = sink.send(Message::Ping(Default::default())).await.is_ok();
            },
            _ = introspect.tick() => {
                if !check_live(&app, &device_id, &uuid).await {
                    let revoked = frame(&Event::SessionRevoked {});
                    let _ = sink.send(Message::Text(revoked.as_ref().into())).await;
                    let _ = sink.send(Message::Close(Some(CloseFrame { code: 4401, reason: "session revoked".into() }))).await;
                    break;
                }
            },
        }
    }
    if app.hub.disconnect(&device_id) {
        announce_device(&app, &uuid, &device_id);
    }
}

fn is_ping(text: &str) -> bool {
    serde_json::from_str::<ring_proto::ClientFrame>(text)
        .is_ok_and(|f| matches!(f, ring_proto::ClientFrame::Ping))
}

fn announce_device(app: &AppState, uuid: &str, device_id: &str) {
    let device = app.db.with(|c| {
        let Some(row) = crate::sessions::by_device(c, device_id)? else {
            return Ok(None);
        };
        crate::sessions::render_device(app, c, &row, "").map(Some)
    });
    if let Ok(Some(device)) = device {
        app.hub.send_account(uuid, &Event::DeviceUpdated { device });
    }
}
