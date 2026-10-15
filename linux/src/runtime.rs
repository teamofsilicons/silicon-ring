//! The bridge between Tokio and the GLib main loop.
//!
//! ring-core (HTTP, the realtime WebSocket, LiveKit media), oo7 and ashpd all need a Tokio
//! runtime, while every GTK object lives on the main thread. Work is spawned onto a small
//! multi-threaded runtime and its result is awaited from `glib::spawn_future_local`; Tokio's
//! `JoinHandle` and its channels can be awaited from any executor, so no extra plumbing is needed.

use std::future::Future;
use std::sync::OnceLock;

use tokio::runtime::Runtime;

fn runtime() -> &'static Runtime {
    static RUNTIME: OnceLock<Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("ring-io")
            .enable_all()
            .build()
            .expect("failed to start the Tokio runtime")
    })
}

/// Starts the runtime early so the first network request does not pay for it.
pub fn init() {
    runtime();
}

/// Runs `fut` on the Tokio runtime. The returned future resolves wherever it is awaited,
/// typically on the GTK main thread.
pub fn run<F>(fut: F) -> impl Future<Output = F::Output> + 'static
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    let handle = runtime().spawn(fut);
    async move {
        match handle.await {
            Ok(value) => value,
            Err(err) if err.is_panic() => std::panic::resume_unwind(err.into_panic()),
            Err(err) => panic!("background task was cancelled: {err}"),
        }
    }
}

/// Fire-and-forget work on the Tokio runtime.
pub fn spawn<F>(fut: F)
where
    F: Future<Output = ()> + Send + 'static,
{
    runtime().spawn(fut);
}

/// Enters the runtime context, for synchronous ring-core calls that may spawn Tokio tasks
/// internally (LiveKit's mute and device switching).
pub fn enter() -> tokio::runtime::EnterGuard<'static> {
    runtime().enter()
}
