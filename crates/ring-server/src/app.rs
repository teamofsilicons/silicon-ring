//! Shared server state.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use tokio::sync::OwnedMutexGuard;
use tokio::task::AbortHandle;

use crate::accounts::Accounts;
use crate::config::Config;
use crate::db::Db;
use crate::livekit::LiveKit;
use crate::media::MediaLink;
use crate::push::Push;
use crate::ratelimit::RateLimiter;
use crate::realtime::Hub;
use crate::s3::S3;
use crate::ting::Ting;
use crate::util::Sealer;

pub type AppState = Arc<App>;

pub struct App {
    pub config: Config,
    pub db: Db,
    pub sealer: Sealer,
    pub accounts: Accounts,
    pub hub: Hub,
    pub media: MediaLink,
    pub livekit: LiveKit,
    pub push: Push,
    pub ting: Ting,
    pub s3: Option<S3>,
    pub call_locks: KeyedLocks,
    pub timers: Timers,
    pub limiter: RateLimiter,
}

/// One async mutex per key (call id, session id), created on demand and dropped when unused.
#[derive(Default)]
pub struct KeyedLocks {
    locks: Mutex<HashMap<String, Weak<tokio::sync::Mutex<()>>>>,
}

impl KeyedLocks {
    pub async fn lock(&self, key: &str) -> OwnedMutexGuard<()> {
        let lock = {
            let mut map = self.locks.lock().unwrap_or_else(|p| p.into_inner());
            if map.len() > 1024 {
                map.retain(|_, weak| weak.strong_count() > 0);
            }
            match map.get(key).and_then(Weak::upgrade) {
                Some(lock) => lock,
                None => {
                    let lock = Arc::new(tokio::sync::Mutex::new(()));
                    map.insert(key.to_string(), Arc::downgrade(&lock));
                    lock
                }
            }
        };
        lock.lock_owned().await
    }
}

/// Named one-shot timers that can be replaced or cancelled (invitation expiry, media grace, …).
#[derive(Default)]
pub struct Timers {
    tasks: Mutex<HashMap<String, AbortHandle>>,
}

impl Timers {
    /// Runs `f` after `delay` unless cancelled or replaced under the same `name` first.
    pub fn schedule<F>(&self, name: String, delay: Duration, f: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let mut tasks = self.tasks.lock().unwrap_or_else(|p| p.into_inner());
        tasks.retain(|_, handle| !handle.is_finished());
        // The work runs as its own task so that cancelling timers from inside it (for example an
        // expiry that ends the call) can never abort the work half-way.
        let handle = tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            tokio::spawn(f);
        })
        .abort_handle();
        if let Some(previous) = tasks.insert(name, handle) {
            previous.abort();
        }
    }

    pub fn cancel(&self, name: &str) {
        if let Some(handle) = self
            .tasks
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(name)
        {
            handle.abort();
        }
    }

    pub fn cancel_prefix(&self, prefix: &str) {
        let mut tasks = self.tasks.lock().unwrap_or_else(|p| p.into_inner());
        tasks.retain(|name, handle| {
            if name.starts_with(prefix) {
                handle.abort();
                false
            } else {
                true
            }
        });
    }
}
