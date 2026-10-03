//! Admission and shutdown shared by every task belonging to one environment.
use crate::{engine, model::*, App};
use std::{
    future::Future,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
};
use tokio_util::{
    sync::CancellationToken,
    task::{task_tracker::TaskTrackerToken, TaskTracker},
};

pub struct Activity {
    accepting: Mutex<bool>,
    started: AtomicBool,
    pub cancelled: CancellationToken,
    tasks: TaskTracker,
}
impl Default for Activity {
    fn default() -> Self {
        Self {
            accepting: Mutex::new(true),
            started: AtomicBool::new(false),
            cancelled: CancellationToken::new(),
            tasks: TaskTracker::new(),
        }
    }
}
impl Activity {
    pub fn enter(&self) -> Option<TaskTrackerToken> {
        let accepting = self.accepting.lock().unwrap();
        accepting.then(|| self.tasks.token())
    }
    fn begin_start(&self) -> bool {
        let accepting = self.accepting.lock().unwrap();
        *accepting && !self.started.swap(true, Ordering::SeqCst)
    }
    async fn stop(&self) {
        {
            let mut accepting = self.accepting.lock().unwrap();
            *accepting = false;
            self.cancelled.cancel();
            self.tasks.close();
        }
        self.tasks.wait().await;
    }
}
impl App {
    pub fn spawn(&self, future: impl Future<Output = ()> + Send + 'static) {
        let Some(guard) = self.activity.enter() else {
            return;
        };
        let cancelled = self.activity.cancelled.clone();
        tokio::spawn(async move {
            tokio::select! { biased; _ = cancelled.cancelled() => {}, _ = future => {} }
            drop(guard);
        });
    }
    // External writes already in flight must settle before destructive cleanup begins.
    pub fn spawn_draining(&self, future: impl Future<Output = ()> + Send + 'static) {
        let Some(guard) = self.activity.enter() else {
            return;
        };
        tokio::spawn(async move {
            future.await;
            drop(guard);
        });
    }
    pub fn start(&self) {
        if !self.activity.begin_start() {
            return;
        }
        crate::workers::start(self.clone());
        crate::storage::start(self.clone());
        crate::auth::start_revalidation(self.clone());
        crate::push::start(self.clone());
    }
    pub async fn stop(&self) -> Result<()> {
        self.activity.stop().await;
        *self.media.lock().unwrap() = Default::default();
        let mut e = self.engine.lock().unwrap();
        recover_calls(&mut e);
        e.state.sessions.clear();
        e.state.pushes.clear();
        e.persist()
    }
}
pub fn recover_calls(engine: &mut engine::Engine) {
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
            serde_json::json!({"ringid":c.ringid,"reason":"runtime_stopped"}),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn shutdown_closes_admission_and_waits_for_inflight_effects() {
        let activity = std::sync::Arc::new(Activity::default());
        let guard = activity.enter().unwrap();
        let stopping = activity.clone();
        let task = tokio::spawn(async move {
            stopping.stop().await;
        });
        activity.cancelled.cancelled().await;
        assert!(activity.enter().is_none());
        assert!(!task.is_finished());
        drop(guard);
        task.await.unwrap();
    }
}
