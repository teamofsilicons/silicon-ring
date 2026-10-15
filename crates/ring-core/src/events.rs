//! Realtime events over `GET /v1/ws`.
//!
//! SKELETON: the signatures are the contract; the ring-core owner implements them.

use tokio::sync::mpsc;

use crate::proto::Event;
use crate::{Client, Result};

/// What a [`Subscription`] yields.
#[derive(Clone, Debug)]
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
    pub async fn subscribe(&self) -> Result<Subscription> {
        let _ = self;
        todo!()
    }
}
