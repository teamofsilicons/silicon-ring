//! Routes ring-server commands to per-call sessions. Every representative, recorder and
//! voicemail runs in its own task; a failing session reports and ends without affecting others.

use std::{
    collections::{BTreeSet, HashMap},
    future::Future,
    panic::AssertUnwindSafe,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use futures_util::FutureExt;
use ring_proto::{
    MessageKind, RepresentativeStatus,
    internal::{FromMedia, RosterMember, ToMedia},
};
use tokio::sync::mpsc;
use tokio_util::{sync::CancellationToken, task::TaskTracker};

use crate::{
    config::Config, link::Reporter, recorder, representative, services::Services, voicemail,
};

/// The error reported for a session that panicked.
const CRASHED: &str = "ring-media hit an internal error";

fn voicemail_crashed(voicemail_id: String) -> FromMedia {
    FromMedia::VoicemailFinished {
        voicemail_id,
        s3_key: None,
        duration_ms: None,
        transcript: None,
        error: Some(CRASHED.into()),
    }
}

/// Commands for a running representative.
pub enum RepControl {
    Message {
        message_id: String,
        kind: MessageKind,
        text: String,
        delegation_id: Option<String>,
    },
    Roster {
        roster: Vec<RosterMember>,
        joined: Option<ring_proto::Account>,
        left: Option<ring_proto::Account>,
    },
    Authority {
        account_uuid: String,
        identity: String,
    },
}

/// Commands for a running recorder.
pub enum RecControl {
    Roster(Vec<RosterMember>),
    Authority {
        account_uuid: String,
        identity: String,
    },
}

/// What a session task receives: its stop signal (also cancelled on shutdown) and commands.
pub struct Session<C> {
    pub stop: CancellationToken,
    pub control: mpsc::UnboundedReceiver<C>,
}

struct Handle<C> {
    id: u64,
    ringid: String,
    stop: CancellationToken,
    control: mpsc::UnboundedSender<C>,
}

impl<C> Handle<C> {
    fn live(&self) -> bool {
        !self.stop.is_cancelled()
    }
}

#[derive(Default)]
struct Registry {
    representatives: HashMap<(String, String), Handle<RepControl>>,
    recorders: HashMap<String, Handle<RecControl>>,
    voicemails: HashMap<String, Handle<()>>,
    syntheses: BTreeSet<String>,
}

pub struct Supervisor {
    pub config: Arc<Config>,
    pub reporter: Reporter,
    pub services: Services,
    pub greetings: voicemail::Greetings,
    /// Cancelled on SIGTERM: every session finishes its work and ends.
    pub shutdown: CancellationToken,
    pub tracker: TaskTracker,
    registry: Mutex<Registry>,
    next_id: AtomicU64,
}

impl Supervisor {
    pub fn new(config: Arc<Config>, reporter: Reporter, shutdown: CancellationToken) -> Self {
        Self {
            services: Services::new(config.clone()),
            greetings: voicemail::Greetings::default(),
            config,
            reporter,
            shutdown,
            tracker: TaskTracker::new(),
            registry: Mutex::default(),
            next_id: AtomicU64::new(1),
        }
    }

    /// Rings with any running session, for `hello` after a reconnect.
    pub fn active_rings(&self) -> Vec<String> {
        let registry = self.registry.lock().unwrap();
        let rings: BTreeSet<String> = registry
            .representatives
            .values()
            .map(|h| h.ringid.clone())
            .chain(registry.recorders.values().map(|h| h.ringid.clone()))
            .chain(registry.voicemails.values().map(|h| h.ringid.clone()))
            .collect();
        rings.into_iter().collect()
    }

    fn handle<C>(&self, ringid: &str) -> (Handle<C>, Session<C>) {
        let stop = self.shutdown.child_token();
        let (tx, rx) = mpsc::unbounded_channel();
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        (
            Handle {
                id,
                ringid: ringid.to_owned(),
                stop: stop.clone(),
                control: tx,
            },
            Session { stop, control: rx },
        )
    }

    /// Runs a session task. `cleanup` always runs afterwards, told whether the task panicked, so
    /// a bug in one session can neither leave a stale registry entry (which would refuse every
    /// later start for that call) nor go unreported to ring-server.
    fn spawn(
        self: &Arc<Self>,
        task: impl Future<Output = ()> + Send + 'static,
        cleanup: impl FnOnce(&Self, bool) + Send + 'static,
    ) {
        let this = self.clone();
        self.tracker.spawn(async move {
            let panicked = AssertUnwindSafe(task).catch_unwind().await.is_err();
            if panicked {
                tracing::error!("a media session crashed");
            }
            cleanup(&this, panicked);
        });
    }

    pub fn dispatch(self: &Arc<Self>, command: ToMedia) {
        // While draining for a restart, new sessions are declined without a result: ring-server
        // still counts them as pending and reissues them when the next worker says hello.
        let shutting_down = self.shutdown.is_cancelled();
        match command {
            ToMedia::RepresentativeStart {
                ringid,
                silicon,
                voice,
                context,
                start,
                roster,
                token,
            } => {
                let key = (ringid.clone(), silicon.uuid.clone());
                if shutting_down {
                    tracing::info!(%ringid, "not starting a representative while shutting down");
                    return;
                }
                let mut registry = self.registry.lock().unwrap();
                if registry.representatives.get(&key).is_some_and(Handle::live) {
                    tracing::debug!(%ringid, "representative already running");
                    return;
                }
                let (handle, session) = self.handle(&ringid);
                let id = handle.id;
                registry.representatives.insert(key.clone(), handle);
                drop(registry);
                let params = representative::Params {
                    ringid,
                    silicon,
                    voice,
                    context,
                    start,
                    roster,
                    token,
                };
                self.spawn(
                    representative::run(self.clone(), params, session),
                    move |this, panicked| {
                        let mut registry = this.registry.lock().unwrap();
                        if registry
                            .representatives
                            .get(&key)
                            .is_some_and(|h| h.id == id)
                        {
                            registry.representatives.remove(&key);
                        }
                        drop(registry);
                        if panicked {
                            let (ringid, silicon_uuid) = key;
                            this.reporter.send(FromMedia::RepresentativeStatus {
                                ringid,
                                silicon_uuid,
                                status: RepresentativeStatus::Failed,
                                error: Some(CRASHED.into()),
                            });
                        }
                    },
                );
            }
            ToMedia::RepresentativeStop {
                ringid,
                silicon_uuid,
            } => {
                let registry = self.registry.lock().unwrap();
                match registry
                    .representatives
                    .get(&(ringid.clone(), silicon_uuid))
                {
                    Some(handle) => handle.stop.cancel(),
                    None => tracing::debug!(%ringid, "no representative to stop"),
                }
            }
            ToMedia::RepresentativeMessage {
                ringid,
                silicon_uuid,
                message_id,
                kind,
                text,
                delegation_id,
            } => {
                let registry = self.registry.lock().unwrap();
                match registry
                    .representatives
                    .get(&(ringid.clone(), silicon_uuid))
                    .filter(|h| h.live())
                {
                    Some(handle) => {
                        let _ = handle.control.send(RepControl::Message {
                            message_id,
                            kind,
                            text,
                            delegation_id,
                        });
                    }
                    None => {
                        tracing::warn!(%ringid, %message_id, "message for a representative that is not running")
                    }
                }
            }
            ToMedia::RosterChanged {
                ringid,
                roster,
                joined,
                left,
            } => {
                let registry = self.registry.lock().unwrap();
                for handle in registry
                    .representatives
                    .values()
                    .filter(|h| h.ringid == ringid)
                {
                    let _ = handle.control.send(RepControl::Roster {
                        roster: roster.clone(),
                        joined: joined.clone(),
                        left: left.clone(),
                    });
                }
                if let Some(handle) = registry.recorders.get(&ringid) {
                    let _ = handle.control.send(RecControl::Roster(roster));
                }
            }
            ToMedia::AuthorityChanged {
                ringid,
                account_uuid,
                identity,
            } => {
                let registry = self.registry.lock().unwrap();
                for handle in registry
                    .representatives
                    .values()
                    .filter(|h| h.ringid == ringid)
                {
                    let _ = handle.control.send(RepControl::Authority {
                        account_uuid: account_uuid.clone(),
                        identity: identity.clone(),
                    });
                }
                if let Some(handle) = registry.recorders.get(&ringid) {
                    let _ = handle.control.send(RecControl::Authority {
                        account_uuid,
                        identity,
                    });
                }
            }
            ToMedia::RecorderStart {
                ringid,
                token,
                roster,
                s3_key,
            } => {
                if shutting_down {
                    tracing::info!(%ringid, "not starting a recorder while shutting down");
                    return;
                }
                let mut registry = self.registry.lock().unwrap();
                if registry.recorders.get(&ringid).is_some_and(Handle::live) {
                    tracing::debug!(%ringid, "recorder already running");
                    return;
                }
                let (handle, session) = self.handle(&ringid);
                let id = handle.id;
                registry.recorders.insert(ringid.clone(), handle);
                drop(registry);
                let key = ringid.clone();
                let params = recorder::Params {
                    ringid,
                    token,
                    roster,
                    s3_key,
                };
                self.spawn(
                    recorder::run(self.clone(), params, session),
                    move |this, panicked| {
                        let mut registry = this.registry.lock().unwrap();
                        if registry.recorders.get(&key).is_some_and(|h| h.id == id) {
                            registry.recorders.remove(&key);
                        }
                        drop(registry);
                        if panicked {
                            this.reporter.send(FromMedia::RecordingFinished {
                                ringid: key,
                                s3_key: None,
                                duration_ms: None,
                                error: Some(CRASHED.into()),
                            });
                        }
                    },
                );
            }
            ToMedia::RecorderStop { ringid } => {
                if let Some(handle) = self.registry.lock().unwrap().recorders.get(&ringid) {
                    handle.stop.cancel();
                }
            }
            ToMedia::VoicemailStart {
                ringid,
                voicemail_id,
                caller_identity,
                greeting_text,
                voice,
                max_seconds,
                token,
                s3_key,
            } => {
                if shutting_down {
                    tracing::info!(%voicemail_id, "not starting a voicemail while shutting down");
                    return;
                }
                let mut registry = self.registry.lock().unwrap();
                if registry.voicemails.contains_key(&voicemail_id) {
                    tracing::debug!(%voicemail_id, "voicemail already running");
                    return;
                }
                let (handle, session) = self.handle(&ringid);
                let id = handle.id;
                registry.voicemails.insert(voicemail_id.clone(), handle);
                drop(registry);
                let key = voicemail_id.clone();
                let params = voicemail::Params {
                    ringid,
                    voicemail_id,
                    caller_identity,
                    greeting_text,
                    voice,
                    max_seconds,
                    token,
                    s3_key,
                };
                self.spawn(
                    voicemail::run(self.clone(), params, session.stop),
                    move |this, panicked| {
                        let mut registry = this.registry.lock().unwrap();
                        if registry.voicemails.get(&key).is_some_and(|h| h.id == id) {
                            registry.voicemails.remove(&key);
                        }
                        drop(registry);
                        if panicked {
                            this.reporter.send(voicemail_crashed(key));
                        }
                    },
                );
            }
            ToMedia::VoicemailStop { voicemail_id } => {
                match self.registry.lock().unwrap().voicemails.get(&voicemail_id) {
                    Some(handle) => handle.stop.cancel(),
                    None => tracing::debug!(%voicemail_id, "no voicemail to stop"),
                }
            }
            ToMedia::GreetingPrepare { voice, text } => {
                if shutting_down {
                    return;
                }
                self.spawn(voicemail::prepare(self.clone(), voice, text), |_, _| {});
            }
            ToMedia::VoicemailSynthesize {
                voicemail_id,
                text,
                voice,
                s3_key,
            } => {
                if !self
                    .registry
                    .lock()
                    .unwrap()
                    .syntheses
                    .insert(voicemail_id.clone())
                {
                    tracing::debug!(%voicemail_id, "voicemail synthesis already running");
                    return;
                }
                let key = voicemail_id.clone();
                self.spawn(
                    voicemail::synthesize(self.clone(), voicemail_id, text, voice, s3_key),
                    move |this, panicked| {
                        this.registry.lock().unwrap().syntheses.remove(&key);
                        if panicked {
                            this.reporter.send(voicemail_crashed(key));
                        }
                    },
                );
            }
            ToMedia::CallClosed { ringid } => {
                let registry = self.registry.lock().unwrap();
                let stops = registry
                    .representatives
                    .values()
                    .filter(|h| h.ringid == ringid)
                    .map(|h| &h.stop)
                    .chain(
                        registry
                            .voicemails
                            .values()
                            .filter(|h| h.ringid == ringid)
                            .map(|h| &h.stop),
                    )
                    .chain(registry.recorders.get(&ringid).map(|h| &h.stop));
                for stop in stops {
                    stop.cancel();
                }
            }
        }
    }
}
